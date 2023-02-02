use crate::defs::*;
use crate::dir::*;
use crate::h_inode::*;
use crate::pm::*;
use crate::typestate::*;
use crate::volatile::*;
use core::{
    marker::PhantomData,
    mem,
    sync::atomic::{AtomicU64, Ordering},
};
use kernel::prelude::*;
use kernel::PAGE_SIZE;

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

#[allow(dead_code)]
#[repr(C)]
struct DirPageHeader {
    page_type: PageType,
    ino: InodeNum,
    dentries: [HayleyFsDentry; DENTRIES_PER_PAGE],
}

impl DirPageHeader {
    pub(crate) fn is_initialized(&self) -> bool {
        self.page_type != PageType::NONE && self.ino != 0
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

    /// This method returns a DirPageWrapper ONLY if the page is initialized
    /// Otherwise it returns an error
    pub(crate) fn from_dir_page_info(sbi: &'a SbInfo, info: &DirPageInfo) -> Result<Self> {
        let page_no = info.get_page_no();
        let ph = page_no_to_dir_header(sbi, page_no)?;
        if !ph.is_initialized() {
            Err(EPERM)
        } else {
            // Safety: it's safe to wrap the page header since we check that it is
            // initialized
            unsafe { Ok(Self::wrap_dir_page_header(ph, page_no)) }
        }
    }
}

// TODO: safety
fn page_no_to_dir_header(sbi: &SbInfo, page_no: PageNum) -> Result<&mut DirPageHeader> {
    let virt_addr = sbi.get_virt_addr();
    let page_size_u64: u64 = PAGE_SIZE.try_into()?;
    let page_addr = unsafe { virt_addr.offset((page_size_u64 * page_no).try_into()?) };
    // cast raw page address to dir page header
    let ph: &mut DirPageHeader = unsafe { &mut *page_addr.cast() };
    // check page type
    if !(ph.page_type == PageType::DIR || ph.page_type == PageType::NONE) {
        Err(EINVAL)
    } else {
        Ok(ph)
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
    /// Obtains a wrapped pointer to a free dentry.
    /// This does NOT allocate the dentry - just obtains a pointer to a free dentry
    /// This requires a mutable reference to self because we need to acquire a
    /// mutable reference to a dentry, but it doesn't actually modify the DirPageWrapper
    pub(crate) fn get_free_dentry(self) -> Result<DentryWrapper<'a, Clean, Free>> {
        // iterate until we find a free dentry
        // VFS *should* have locked the parent, so there is no possibility of
        // this racing with another operation trying to create in the same directory
        // TODO: confirm that
        // TODO: safety notes based on VFS locking.
        for dentry in self.page.dentries.iter_mut() {
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
struct DataPageHeader {
    page_type: PageType,
    ino: InodeNum,
    offset: u64,
}

// TODO: inline? macro?
pub(crate) fn bytes_per_page() -> usize {
    PAGE_SIZE - mem::size_of::<DataPageHeader>()
}

/// Given the offset into a file, returns the offset of the
/// DataPageHeader that includes that offset
pub(crate) fn page_offset(offset: u64) -> Result<usize> {
    let bytes_per_page = bytes_per_page();
    let offset_usize: usize = offset.try_into()?;
    // integer division removes the remainder; multiplying by bytes_per_page
    // gives us the offset of the page to read/write
    Ok((offset_usize / bytes_per_page) * bytes_per_page)
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
}

#[allow(dead_code)]
fn page_no_to_data_header(sbi: &SbInfo, page_no: PageNum) -> Result<&mut DataPageHeader> {
    let virt_addr = sbi.get_virt_addr();
    let page_size_u64: u64 = PAGE_SIZE.try_into()?;
    let page_addr = unsafe { virt_addr.offset((page_size_u64 * page_no).try_into()?) };
    // cast raw page address to data page header
    let ph: &mut DataPageHeader = unsafe { &mut *page_addr.cast() };
    // check page type
    if ph.page_type != PageType::DATA {
        Err(EINVAL)
    } else {
        Ok(ph)
    }
}

#[allow(dead_code)]
impl<'a> DataPageWrapper<'a, Dirty, Writeable> {
    /// Allocate a new page and set it to be a directory page.
    /// Does NOT flush the allocated page.
    pub(crate) fn alloc_data_page(sbi: &'a SbInfo, offset: u64) -> Result<Self> {
        // TODO: should we zero the page here?
        let page_no = sbi.page_allocator.alloc_page()?;
        let ph = page_no_to_data_header(sbi, page_no)?;

        ph.page_type = PageType::DATA;
        ph.offset = offset;
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
