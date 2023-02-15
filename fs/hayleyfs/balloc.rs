use crate::defs::*;
use crate::dir::*;
use crate::h_inode::*;
use crate::pm::*;
use crate::typestate::*;
use crate::volatile::*;
use core::{
    ffi,
    marker::PhantomData,
    mem, slice,
    sync::atomic::{AtomicU64, Ordering},
};
use kernel::io_buffer::{IoBufferReader, IoBufferWriter};
use kernel::prelude::*;

pub(crate) trait PageAllocator {
    fn new(val: u64) -> Self;
    fn alloc_page(&self) -> Result<PageNum>;
    fn dealloc_page(&self, page: PageNum) -> Result<()>;
}

/// Allocates by keeping a counter and returning the next number in the counter.
/// Does not support page deallocation.
///
/// # Safety
/// BasicPageAllocator is implemented with AtomicU64 so it is safe to share
/// across threads.
pub(crate) struct BasicPageAllocator {
    next_page: AtomicU64,
}

impl PageAllocator for BasicPageAllocator {
    fn new(val: u64) -> Self {
        BasicPageAllocator {
            next_page: AtomicU64::new(val),
        }
    }

    fn alloc_page(&self) -> Result<PageNum> {
        if self.next_page.load(Ordering::SeqCst) == MAX_PAGES {
            Err(ENOSPC)
        } else {
            Ok(self.next_page.fetch_add(1, Ordering::SeqCst).try_into()?)
        }
    }

    fn dealloc_page(&self, _: PageNum) -> Result<()> {
        unimplemented!();
    }
}

// placeholder page descriptor that can represent either a dir or data page descriptor
// mainly useful so that we can represent the page descriptor table as a slice and index
// into it. fields cannot be accessed directly - we can only convert this type into a
// narrower page descriptor type
#[derive(Debug)]
#[repr(C)]
pub(crate) struct PageDescriptor {
    page_type: PageType,
    ino: InodeNum,
    offset: u64,
    _padding0: u64,
}

impl PageDescriptor {
    pub(crate) fn is_free(&self) -> bool {
        self.page_type == PageType::NONE && self.ino == 0 && self.offset == 0
    }

    pub(crate) fn get_page_type(&self) -> PageType {
        self.page_type
    }
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct DirPageHeader {
    page_type: PageType,
    ino: InodeNum,
    _padding0: u64,
    _padding1: u64,
}

impl TryFrom<&mut PageDescriptor> for &mut DirPageHeader {
    type Error = Error;

    fn try_from(value: &mut PageDescriptor) -> Result<Self> {
        if value.page_type == PageType::DIR || value.page_type == PageType::NONE {
            Ok(unsafe { &mut *(value as *mut PageDescriptor as *mut DirPageHeader) })
        } else {
            Err(ENOTDIR)
        }
    }
}

impl TryFrom<&PageDescriptor> for &DirPageHeader {
    type Error = Error;

    fn try_from(value: &PageDescriptor) -> Result<Self> {
        if value.page_type == PageType::DIR || value.page_type == PageType::NONE {
            Ok(unsafe { &*(value as *const PageDescriptor as *const DirPageHeader) })
        } else {
            Err(ENOTDIR)
        }
    }
}

// be careful here... slice should have size DENTRIES_PER_PAGE
// i can't figure out how to just make this be an array
struct DirPage<'a> {
    dentries: &'a mut [HayleyFsDentry],
}

impl DirPage<'_> {
    pub(crate) fn get_live_inodes_from_dentries(self) -> Result<Vec<InodeNum>> {
        let mut inode_vec = Vec::new();
        let live_inodes = self.dentries.iter().filter_map(|d| {
            let ino = d.get_ino();
            if ino != 0 {
                Some(ino)
            } else {
                None
            }
        });
        // TODO: a more efficient way? kernel doesn't provide collect()
        for ino in live_inodes {
            inode_vec.try_push(ino)?;
        }
        Ok(inode_vec)
    }
}

impl DirPageHeader {
    pub(crate) fn is_initialized(&self) -> bool {
        self.page_type != PageType::NONE && self.ino != 0
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }
}

#[allow(dead_code)]
pub(crate) struct DirPageWrapper<'a, State, Op> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    page_no: PageNum,
    page: &'a mut DirPageHeader,
}

impl<'a, State, Op> DirPageWrapper<'a, State, Op> {
    pub(crate) fn get_page_no(&self) -> PageNum {
        self.page_no
    }
}

impl<'a> DirPageWrapper<'a, Clean, Start> {
    unsafe fn wrap_dir_page_header(ph: &'a mut DirPageHeader, page_no: PageNum) -> Self {
        Self {
            state: PhantomData,
            op: PhantomData,
            page_no,
            page: ph,
        }
    }

    pub(crate) fn from_page_no(sbi: &'a SbInfo, page_no: PageNum) -> Result<Self> {
        let ph = page_no_to_dir_header(&sbi, page_no)?;
        if !ph.is_initialized() {
            Err(EPERM)
        } else {
            // Safety: it's safe to wrap the page header since we check that it is
            // initialized
            unsafe { Ok(Self::wrap_dir_page_header(ph, page_no)) }
        }
    }

    /// This method returns a DirPageWrapper ONLY if the page is initialized
    /// Otherwise it returns an error
    pub(crate) fn from_dir_page_info(sbi: &'a SbInfo, info: &DirPageInfo) -> Result<Self> {
        let page_no = info.get_page_no();
        Self::from_page_no(sbi, page_no)
    }
}

// TODO: safety
fn page_no_to_dir_header<'a>(sbi: &'a SbInfo, page_no: PageNum) -> Result<&'a mut DirPageHeader> {
    let page_desc_table = sbi.get_page_desc_table()?;
    let page_index: usize = (page_no - DATA_PAGE_START).try_into()?;
    let ph: &mut PageDescriptor = &mut page_desc_table[page_index];
    let ph: &mut DirPageHeader = ph.try_into()?;
    Ok(ph)
}

// TODO: safety
unsafe fn page_no_to_page(sbi: &SbInfo, page_no: PageNum) -> Result<*mut u8> {
    if page_no > MAX_PAGES {
        Err(ENOSPC)
    } else {
        let virt_addr: *mut u8 = sbi.get_virt_addr();
        let res = Ok(unsafe { virt_addr.offset((HAYLEYFS_PAGESIZE * page_no).try_into()?) });
        res
    }
}

impl<'a> DirPageWrapper<'a, Dirty, Alloc> {
    /// Allocate a new page and set it to be a directory page.
    /// Does NOT flush the allocated page.
    pub(crate) fn alloc_dir_page(sbi: &'a SbInfo) -> Result<Self> {
        // TODO: should we zero the page here?
        let page_no = sbi.page_allocator.alloc_page()?;
        let ph = page_no_to_dir_header(sbi, page_no)?;

        ph.page_type = PageType::DIR;
        Ok(DirPageWrapper {
            state: PhantomData,
            op: PhantomData,
            page_no,
            page: ph,
        })
    }
}

impl<'a> DirPageWrapper<'a, Clean, Alloc> {
    /// Requires Initialized inode only as proof that the inode number we are setting points
    /// to an initialized inode
    pub(crate) fn set_dir_page_backpointer<InoState: Initialized>(
        self,
        inode: InodeWrapper<'a, Clean, InoState, DirInode>,
    ) -> DirPageWrapper<'a, Dirty, Init> {
        self.page.ino = inode.get_ino();
        DirPageWrapper {
            state: PhantomData,
            op: PhantomData,
            page_no: self.page_no,
            page: self.page,
        }
    }
}

impl<'a, Op: Initialized> DirPageWrapper<'a, Clean, Op> {
    pub(crate) fn get_live_inodes(&self, sbi: &SbInfo) -> Result<Vec<InodeNum>> {
        let dir_page = self.get_dir_page(sbi)?;
        dir_page.get_live_inodes_from_dentries()
    }

    fn get_dir_page(&self, sbi: &SbInfo) -> Result<DirPage<'a>> {
        let page_addr = unsafe { page_no_to_page(sbi, self.get_page_no())? as *mut HayleyFsDentry };
        let dentries = unsafe { slice::from_raw_parts_mut(page_addr, DENTRIES_PER_PAGE) };
        Ok(DirPage { dentries })
    }

    /// Obtains a wrapped pointer to a free dentry.
    /// This does NOT allocate the dentry - just obtains a pointer to a free dentry
    /// This requires a mutable reference to self because we need to acquire a
    /// mutable reference to a dentry, but it doesn't actually modify the DirPageWrapper
    pub(crate) fn get_free_dentry(self, sbi: &SbInfo) -> Result<DentryWrapper<'a, Clean, Free>> {
        let page = self.get_dir_page(&sbi)?;
        // iterate until we find a free dentry
        // VFS *should* have locked the parent, so there is no possibility of
        // this racing with another operation trying to create in the same directory
        // TODO: confirm that
        // TODO: safety notes based on VFS locking.
        for dentry in page.dentries.iter_mut() {
            // if any part of a dentry is NOT zeroed out, that dentry is allocated; we need
            // an unallocated dentry
            if dentry.get_ino() == 0 && dentry.is_rename_ptr_null() && !dentry.has_name() {
                return Ok(unsafe { DentryWrapper::wrap_free_dentry(dentry) });
            }
        }
        // if we can't find a free dentry in this page, return an error
        Err(ENOSPC)
    }
}

impl<'a, Op> DirPageWrapper<'a, Dirty, Op> {
    pub(crate) fn flush(self) -> DirPageWrapper<'a, InFlight, Op> {
        flush_buffer(self.page, mem::size_of::<DirPageHeader>(), false);
        DirPageWrapper {
            state: PhantomData,
            op: PhantomData,
            page_no: self.page_no,
            page: self.page,
        }
    }
}

impl<'a, Op> DirPageWrapper<'a, InFlight, Op> {
    pub(crate) fn fence(self) -> DirPageWrapper<'a, Clean, Op> {
        sfence();
        DirPageWrapper {
            state: PhantomData,
            op: PhantomData,
            page_no: self.page_no,
            page: self.page,
        }
    }
}

#[allow(dead_code)]
#[repr(C)]
pub(crate) struct DataPageHeader {
    page_type: PageType,
    ino: InodeNum,
    offset: u64,
    _padding: u64,
}

impl TryFrom<&mut PageDescriptor> for &mut DataPageHeader {
    type Error = Error;

    fn try_from(value: &mut PageDescriptor) -> Result<Self> {
        if value.page_type == PageType::DATA || value.page_type == PageType::NONE {
            Ok(unsafe { &mut *(value as *mut PageDescriptor as *mut DataPageHeader) })
        } else {
            Err(EISDIR)
        }
    }
}

/// Given the offset into a file, returns the offset of the
/// DataPageHeader that includes that offset
pub(crate) fn page_offset(offset: u64) -> Result<u64> {
    // integer division removes the remainder; multiplying by HAYLEYFS_PAGESIZE
    // gives us the offset of the page to read/write
    Ok((offset / HAYLEYFS_PAGESIZE) * HAYLEYFS_PAGESIZE)
}

#[allow(dead_code)]
impl DataPageHeader {
    pub(crate) fn is_initialized(&self) -> bool {
        self.page_type != PageType::NONE && self.ino != 0
    }
}

#[allow(dead_code)]
pub(crate) struct DataPageWrapper<'a, State, Op> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    page_no: PageNum,
    page: &'a mut DataPageHeader,
}

// TODO: we may be able to combine some DataPageWrapper methods with DirPageWrapper methods
// by making them implement some shared trait - but need to be careful of dynamic dispatch.
// dynamic dispatch may or may not be safe for us there
#[allow(dead_code)]
impl<'a, State, Op> DataPageWrapper<'a, State, Op> {
    pub(crate) fn get_page_no(&self) -> PageNum {
        self.page_no
    }

    pub(crate) fn get_offset(&self) -> u64 {
        self.page.offset
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.page.ino
    }
}

#[allow(dead_code)]
fn page_no_to_data_header(sbi: &SbInfo, page_no: PageNum) -> Result<&mut DataPageHeader> {
    let page_desc_table = sbi.get_page_desc_table()?;
    let page_index: usize = (page_no - DATA_PAGE_START).try_into()?;
    let ph: &mut PageDescriptor = &mut page_desc_table[page_index];
    let ph: &mut DataPageHeader = ph.try_into()?;
    Ok(ph)
}

#[allow(dead_code)]
impl<'a> DataPageWrapper<'a, Dirty, Alloc> {
    /// Allocate a new page and set it to be a directory page.
    /// Does NOT flush the allocated page.
    pub(crate) fn alloc_data_page(sbi: &'a SbInfo, offset: u64) -> Result<Self> {
        // TODO: should we zero the page here?
        let page_no = sbi.page_allocator.alloc_page()?;
        let ph = page_no_to_data_header(sbi, page_no)?;

        ph.page_type = PageType::DATA;
        ph.offset = offset.try_into()?;
        Ok(DataPageWrapper {
            state: PhantomData,
            op: PhantomData,
            page_no,
            page: ph,
        })
    }
}

impl<'a> DataPageWrapper<'a, Clean, Writeable> {
    // TODO: this doesn't need to be unsafe I think
    unsafe fn wrap_data_page_header(ph: &'a mut DataPageHeader, page_no: PageNum) -> Result<Self> {
        if !ph.is_initialized() {
            Err(EPERM)
        } else {
            Ok(Self {
                state: PhantomData,
                op: PhantomData,
                page_no,
                page: ph,
            })
        }
    }

    /// This method returns a DataPageWrapper ONLY if the page is initialized
    /// Otherwise it returns an error
    pub(crate) fn from_data_page_info(sbi: &'a SbInfo, info: DataPageInfo) -> Result<Self> {
        let page_no = info.get_page_no();
        let ph = page_no_to_data_header(sbi, page_no)?;
        // wrap_data_page_header checks whether the page is initialized
        unsafe { Self::wrap_data_page_header(ph, page_no) }
    }

    pub(crate) fn read_from_page(
        &self,
        sbi: &SbInfo,
        writer: &mut impl IoBufferWriter,
        offset: u64,
        len: u64,
    ) -> Result<u64> {
        let ptr = self.get_page_addr(sbi)? as *mut u8;
        let ptr = unsafe { ptr.offset(offset.try_into()?) };
        // FIXME: same problem as write_to_page - write_raw returns an error if
        // the bytes are not all written, which is not what we want.
        unsafe { writer.write_raw(ptr, len.try_into()?) }?;

        Ok(len)
    }

    pub(crate) fn write_to_page(
        self,
        sbi: &SbInfo,
        reader: &mut impl IoBufferReader,
        offset: u64,
        len: u64,
    ) -> Result<(u64, DataPageWrapper<'a, InFlight, Written>)> {
        let ptr = self.get_page_addr(sbi)? as *mut u8;
        let ptr = unsafe { ptr.offset(offset.try_into()?) };

        // FIXME: read_raw and read_raw_nt return a Result that does NOT include the
        // number of bytes actually read. they return an error if all bytes are not
        // read. this is not the behavior we expect or want here. It does return an
        // error if all bytes are not written so we can safely return len if
        // the read does succeed though
        unsafe { reader.read_raw_nt(ptr, len.try_into()?) }?;
        unsafe { flush_edge_cachelines(ptr as *mut ffi::c_void, len) }?;

        Ok((
            len,
            DataPageWrapper {
                state: PhantomData,
                op: PhantomData,
                page_no: self.page_no,
                page: self.page,
            },
        ))
    }

    fn get_page_addr(&self, sbi: &SbInfo) -> Result<*mut u8> {
        let page_addr = unsafe { page_no_to_page(sbi, self.get_page_no())? };
        Ok(page_addr)
    }
}

impl<'a> DataPageWrapper<'a, Clean, Alloc> {
    /// NOTE: this method returns a clean backpointer, since some pages
    /// will not actually need to be modified here. when they do, this method
    /// flushes and fences
    pub(crate) fn set_data_page_backpointer(
        self,
        inode: &InodeWrapper<'a, Clean, Start, RegInode>,
    ) -> DataPageWrapper<'a, Dirty, Writeable> {
        self.page.ino = inode.get_ino();
        DataPageWrapper {
            state: PhantomData,
            op: PhantomData,
            page_no: self.page_no,
            page: self.page,
        }
    }
}

impl<'a, Op> DataPageWrapper<'a, Dirty, Op> {
    pub(crate) fn flush(self) -> DataPageWrapper<'a, InFlight, Op> {
        flush_buffer(self.page, mem::size_of::<DataPageHeader>(), false);
        DataPageWrapper {
            state: PhantomData,
            op: PhantomData,
            page_no: self.page_no,
            page: self.page,
        }
    }
}

impl<'a, Op> DataPageWrapper<'a, InFlight, Op> {
    pub(crate) fn fence(self) -> DataPageWrapper<'a, Clean, Op> {
        sfence();
        DataPageWrapper {
            state: PhantomData,
            op: PhantomData,
            page_no: self.page_no,
            page: self.page,
        }
    }
}
