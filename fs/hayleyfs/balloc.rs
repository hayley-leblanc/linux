use crate::defs::*;
use crate::h_dir::*;
use crate::h_inode::*;
use crate::pm::*;
use crate::typestate::*;
use crate::volatile::*;
use core::{
    ffi,
    marker::PhantomData,
    mem,
    slice,
    // sync::atomic::{AtomicU64, Ordering},
};
use kernel::prelude::*;
use kernel::{
    io_buffer::{IoBufferReader, IoBufferWriter},
    rbtree::RBTree,
    sync::{smutex::Mutex, Arc},
};

pub(crate) trait PageAllocator {
    fn new_from_range(val: u64, dev_pages: u64, cpus: u32) -> Result<Self>
    where
        Self: Sized;
    fn new_from_alloc_vec(
        alloc_pages: Vec<PageNum>,
        start: u64,
        dev_pages: u64,
        cpus: u32,
    ) -> Result<Self>
    where
        Self: Sized;
    fn alloc_page(&self) -> Result<PageNum>;
    fn dealloc_data_page<'a>(&self, page: &DataPageWrapper<'a, Clean, Dealloc>) -> Result<()>;
    fn dealloc_dir_page<'a>(&self, page: &DirPageWrapper<'a, Clean, Dealloc>) -> Result<()>;
}

pub(crate) struct RBPageAllocator {
    map: Arc<Mutex<RBTree<PageNum, ()>>>,
}

impl PageAllocator for Option<RBPageAllocator> {
    fn new_from_range(val: u64, dev_pages: u64, _cpus: u32) -> Result<Self> {
        let mut rb = RBTree::new();
        for i in val..dev_pages {
            rb.try_insert(i, ())?;
        }
        Ok(Some(RBPageAllocator {
            map: Arc::try_new(Mutex::new(rb))?,
        }))
    }

    // alloc_pages must be in sorted order. only pages between start and dev_pages
    // will be added to the allocator tree
    fn new_from_alloc_vec(
        alloc_pages: Vec<PageNum>,
        start: u64,
        dev_pages: u64,
        _cpus: u32,
    ) -> Result<Self> {
        let mut rb = RBTree::new();
        let mut cur_page = start;
        let mut i = 0;
        while cur_page < dev_pages && i < alloc_pages.len() {
            if cur_page < alloc_pages[i] {
                rb.try_insert(cur_page, ())?;
                cur_page += 1;
            } else if cur_page == alloc_pages[i] {
                cur_page += 1;
                i += 1;
            } else {
                // cur_page > alloc_pages[i]
                // i don't THINK this can ever happen?
                pr_info!(
                    "ERROR: cur_page {:?}, i {:?}, alloc_pages[i] {:?}\n",
                    cur_page,
                    i,
                    alloc_pages[i]
                );
                return Err(EINVAL);
            }
        }
        // add all remaining pages to the allocator
        let dev_pages_usize: usize = dev_pages.try_into()?;
        if i < dev_pages_usize {
            for j in i..dev_pages_usize {
                rb.try_insert(j.try_into()?, ())?;
            }
        }
        Ok(Some(RBPageAllocator {
            map: Arc::try_new(Mutex::new(rb))?,
        }))
    }

    fn alloc_page(&self) -> Result<PageNum> {
        if let Some(allocator) = self {
            let map = Arc::clone(&allocator.map);
            let mut map = map.lock();
            let iter = map.iter().next();

            let page = match iter {
                None => {
                    pr_info!("ERROR: ran out of pages in RB page allocator\n");
                    return Err(ENOSPC);
                }
                Some(page) => *page.0,
            };
            map.remove(&page);
            Ok(page)
        } else {
            pr_info!("ERROR: page allocator is uninitialized\n");
            Err(EINVAL)
        }
    }

    fn dealloc_data_page<'a>(&self, page: &DataPageWrapper<'a, Clean, Dealloc>) -> Result<()> {
        if let Some(allocator) = self {
            let map = Arc::clone(&allocator.map);
            let mut map = map.lock();
            let res = map.try_insert(page.get_page_no(), ());
            let res = match res {
                Ok(res) => res,
                Err(e) => {
                    pr_info!(
                        "ERROR: failed to insert {:?} into the page allocator, error {:?}\n",
                        page.get_page_no(),
                        e
                    );
                    return Err(e);
                }
            };
            // sanity check - the page was not already present in the tree
            if res.is_some() {
                pr_info!(
                    "ERROR: page {:?} was deallocated but was already in allocator\n",
                    page.get_page_no()
                );
                Err(EINVAL)
            } else {
                Ok(())
            }
        } else {
            pr_info!("ERROR: page allocator is uninitialized\n");
            Err(EINVAL)
        }
    }

    fn dealloc_dir_page<'a>(&self, page: &DirPageWrapper<'a, Clean, Dealloc>) -> Result<()> {
        if let Some(allocator) = self {
            let map = Arc::clone(&allocator.map);
            let mut map = map.lock();
            let res = map.try_insert(page.get_page_no(), ())?;
            // sanity check - the page was not already present in the tree
            if res.is_some() {
                pr_info!(
                    "ERROR: page {:?} was deallocated but was already in allocator\n",
                    page.get_page_no()
                );
                Err(EINVAL)
            } else {
                Ok(())
            }
        } else {
            pr_info!("ERROR: page allocator is uninitialized\n");
            Err(EINVAL)
        }
    }
}

// represents one CPU's pool of pages
pub(crate) struct PageFreeList {
    // fields can safely be made public because this structure should always
    // be wrapped in a mutex
    pub(crate) free_pages: u64, // number of free pages in this pool
    pub(crate) list: RBTree<PageNum, ()>,
}

pub(crate) struct PerCpuPageAllocator {
    free_lists: Vec<Arc<Mutex<PageFreeList>>>,
    pages_per_cpu: u64,
    cpus: u32,
    // first page the allocator is allowed to return. used to figure out
    // which cpu deallocated pages belong to
    start: u64,
}

impl PageAllocator for Option<PerCpuPageAllocator> {
    // TODO: test!
    fn new_from_range(val: u64, dev_pages: u64, cpus: u32) -> Result<Self> {
        let total_pages = dev_pages - val;
        let cpus_u64: u64 = cpus.into();
        let pages_per_cpu = total_pages / cpus_u64;
        pr_info!("pages per cpu: {:?}\n", pages_per_cpu);
        let mut current_page = val;
        let mut free_lists = Vec::new();
        for _ in 0..cpus {
            let mut rb_tree = RBTree::new();
            let upper = if (current_page + pages_per_cpu) < dev_pages {
                current_page + pages_per_cpu
            } else {
                dev_pages
            };
            for i in current_page..upper {
                rb_tree.try_insert(i, ())?;
            }
            current_page = current_page + pages_per_cpu;
            let free_list = PageFreeList {
                free_pages: pages_per_cpu,
                list: rb_tree,
            };
            free_lists.try_push(Arc::try_new(Mutex::new(free_list))?)?;
            if upper == dev_pages {
                break;
            }
        }

        Ok(Some(PerCpuPageAllocator {
            free_lists,
            pages_per_cpu,
            cpus,
            start: val,
        }))
    }

    // TODO: test!
    /// alloc_pages must be in sorted order. only pages between start and dev_pages
    /// will be added to the allocator
    fn new_from_alloc_vec(
        alloc_pages: Vec<PageNum>,
        start: u64,
        dev_pages: u64,
        cpus: u32,
    ) -> Result<Self> {
        let total_pages = dev_pages - start;
        let cpus_u64: u64 = cpus.into();
        let pages_per_cpu = total_pages / cpus_u64;
        let mut free_lists = Vec::new();
        let mut current_page = start;
        let mut current_cpu_start = start; // used to keep track of when to move to the next cpu pool
        let mut i = 0;
        let mut rb_tree = RBTree::new();
        while current_page < dev_pages && i < alloc_pages.len() {
            if current_page == current_cpu_start + pages_per_cpu {
                let free_list = PageFreeList {
                    free_pages: pages_per_cpu,
                    list: rb_tree,
                };
                free_lists.try_push(Arc::try_new(Mutex::new(free_list))?)?;
                rb_tree = RBTree::new();
                current_cpu_start += pages_per_cpu;
            }
            if current_page < alloc_pages[i] {
                rb_tree.try_insert(current_page, ())?;
                current_page += 1;
            } else if current_page == alloc_pages[i] {
                current_page += 1;
                i += 1;
            } else {
                // current_page > alloc_pages[i]
                // i don't THINK this can ever happen?
                pr_info!(
                    "ERROR: cur_page {:?}, i {:?}, alloc_pages[i] {:?}\n",
                    current_page,
                    i,
                    alloc_pages[i]
                );
                return Err(EINVAL);
            }
        }

        // add all remaining pages to the allocator
        let dev_pages_usize: usize = dev_pages.try_into()?;
        if i < dev_pages_usize {
            for current_page in i..dev_pages_usize {
                if current_page == (current_cpu_start + pages_per_cpu).try_into()? {
                    let free_list = PageFreeList {
                        free_pages: pages_per_cpu,
                        list: rb_tree,
                    };
                    free_lists.try_push(Arc::try_new(Mutex::new(free_list))?)?;
                    rb_tree = RBTree::new();
                    current_cpu_start += pages_per_cpu;
                }
                rb_tree.try_insert(current_page.try_into()?, ())?;
            }
        }

        Ok(Some(PerCpuPageAllocator {
            free_lists,
            pages_per_cpu,
            cpus,
            start,
        }))
    }

    // TODO: allow allocating multiple pages at once
    fn alloc_page(&self) -> Result<PageNum> {
        if let Some(allocator) = self {
            let cpu = get_cpuid(&allocator.cpus);

            let cpu_usize: usize = cpu.try_into()?;
            let free_list = Arc::clone(&allocator.free_lists[cpu_usize]);
            let mut free_list = free_list.lock();

            // does this pool have any free blocks?
            if free_list.free_pages > 0 {
                // TODO: is using an iterator the fastest way to do this?
                let iter = free_list.list.iter().next();
                let page = match iter {
                    None => {
                        pr_info!("ERROR: unable to get free page on CPU {:?}\n", cpu);
                        return Err(ENOSPC);
                    }
                    Some(page) => *page.0,
                };
                // pr_info!("allocating page {:?} on cpu {:?}\n", page, cpu);
                free_list.list.remove(&page);
                free_list.free_pages -= 1;
                Ok(page)
            } else {
                // drop the free_list lock so that we can't deadlock with other processes that might
                // be looking for free pages at that CPU
                drop(free_list);
                // find the free list with the most free blocks and allocate from there
                // TODO: can we do this without so much locking?
                let mut num_free_pages = 0;
                let mut cpuid = 0;
                for i in 0..allocator.cpus {
                    // skip the one we've already checked
                    if i != cpu {
                        let i_usize: usize = i.try_into()?;
                        let free_list = Arc::clone(&allocator.free_lists[i_usize]);
                        let free_list = free_list.lock();
                        if free_list.free_pages > num_free_pages {
                            num_free_pages = free_list.free_pages;
                            cpuid = i_usize;
                        }
                    }
                }

                // now grab a page from that cpu's pool
                let free_list = Arc::clone(&allocator.free_lists[cpuid]);
                let mut free_list = free_list.lock();
                if free_list.free_pages == 0 {
                    pr_info!("ERROR: no more pages\n");
                    Err(ENOSPC)
                } else {
                    // TODO: is using an iterator the fastest way to do this?
                    let iter = free_list.list.iter().next();
                    let page = match iter {
                        None => {
                            pr_info!("ERROR: unable to get free page on CPU {:?}\n", cpu);
                            return Err(ENOSPC);
                        }
                        Some(page) => *page.0,
                    };
                    free_list.list.remove(&page);
                    free_list.free_pages -= 1;
                    Ok(page)
                }
            }
        } else {
            pr_info!("ERROR: page allocator is uninitialized\n");
            Err(EINVAL)
        }
    }

    fn dealloc_data_page<'a>(&self, page: &DataPageWrapper<'a, Clean, Dealloc>) -> Result<()> {
        if let Some(allocator) = self {
            let page_no = page.get_page_no();
            allocator.dealloc_page(page_no)
        } else {
            pr_info!("ERROR: page allocator is uninitialized\n");
            Err(EINVAL)
        }
    }

    fn dealloc_dir_page<'a>(&self, page: &DirPageWrapper<'a, Clean, Dealloc>) -> Result<()> {
        if let Some(allocator) = self {
            let page_no = page.get_page_no();
            allocator.dealloc_page(page_no)
        } else {
            pr_info!("ERROR: page allocator is uninitialized\n");
            Err(EINVAL)
        }
    }
}

impl PerCpuPageAllocator {
    fn dealloc_page(&self, page_no: PageNum) -> Result<()> {
        // rust division rounds down
        let cpu: usize = ((page_no - self.start) / self.pages_per_cpu).try_into()?;
        // pr_info!("deallocating page {:?} on cpu {:?}\n", page_no, cpu);
        let free_list = Arc::clone(&self.free_lists[cpu]);
        let mut free_list = free_list.lock();
        let res = free_list.list.try_insert(page_no, ());
        free_list.free_pages += 1;
        // unwrap the error so we can get at the option
        let res = match res {
            Ok(res) => res,
            Err(e) => {
                pr_info!(
                    "ERROR: failed to insert {:?} into page allocator at CPU {:?}, error {:?}\n",
                    page_no,
                    cpu,
                    e
                );
                return Err(e);
            }
        };
        // check that the page was not already present in the tree
        if res.is_some() {
            pr_info!(
                "ERROR: page {:?} was already in the allocator at CPU {:?}\n",
                page_no,
                cpu
            );
            Err(EINVAL)
        } else {
            Ok(())
        }
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
#[derive(Debug)]
struct DirPage<'a> {
    dentries: &'a mut [HayleyFsDentry],
    // dentries: &'a mut [HayleyFsDentry; DENTRIES_PER_PAGE],
}

impl DirPage<'_> {
    pub(crate) fn get_dentry_info_from_dentries(self) -> Result<Vec<DentryInfo>> {
        let mut dentry_vec = Vec::new();
        let live_dentries = self.dentries.iter().filter_map(|d| {
            let ino = d.get_ino();
            if ino != 0 {
                let name = d.get_name();
                let virt_addr = d as *const HayleyFsDentry as *const ffi::c_void;
                Some(DentryInfo::new(ino, virt_addr, name))
            } else {
                None
            }
        });
        // TODO: a more efficient way? kernel doesn't provide collect()
        for d in live_dentries {
            dentry_vec.try_push(d)?;
        }
        Ok(dentry_vec)
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
            pr_info!("ERROR: page {:?} is uninitialized\n", page_no);
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
        pr_info!(
            "ERROR: page no {:?} is higher than max pages {:?}\n",
            page_no,
            MAX_PAGES
        );
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
        // pr_info!("alloc dir page\n");
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
    pub(crate) fn get_live_dentry_info(&self, sbi: &SbInfo) -> Result<Vec<DentryInfo>> {
        let dir_page = self.get_dir_page(sbi)?;
        dir_page.get_dentry_info_from_dentries()
    }

    fn get_dir_page(&self, sbi: &SbInfo) -> Result<DirPage<'a>> {
        let page_addr = unsafe { page_no_to_page(sbi, self.get_page_no())? as *mut HayleyFsDentry };
        let dentries = unsafe { slice::from_raw_parts_mut(page_addr, DENTRIES_PER_PAGE) };
        // let dentries = dentries.try_into();
        // let dentries: &'a mut [HayleyFsDentry; DENTRIES_PER_PAGE] = match dentries {
        //     Err(_) => return Err(EINVAL),
        //     Ok(dentries) => dentries,
        // };
        Ok(DirPage { dentries })
    }

    pub(crate) fn has_free_space(&self, sbi: &SbInfo) -> Result<bool> {
        let page = self.get_dir_page(&sbi)?;

        for dentry in page.dentries.iter_mut() {
            if dentry.is_free() {
                return Ok(true);
            }
        }
        Ok(false)
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
        for (_i, dentry) in page.dentries.iter_mut().enumerate() {
            // if any part of a dentry is NOT zeroed out, that dentry is allocated; we need
            // an unallocated dentry
            if dentry.is_free() {
                return Ok(unsafe { DentryWrapper::wrap_free_dentry(dentry) });
            }
        }
        // if we can't find a free dentry in this page, return an error
        pr_info!("could not find a free dentry in this page\n");
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
#[derive(Debug)]
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

impl TryFrom<&PageDescriptor> for &DataPageHeader {
    type Error = Error;

    fn try_from(value: &PageDescriptor) -> Result<Self> {
        if value.page_type == PageType::DATA || value.page_type == PageType::NONE {
            Ok(unsafe { &*(value as *const PageDescriptor as *const DataPageHeader) })
        } else {
            Err(EISDIR)
        }
    }
}

/// Given the offset into a file, returns the offset of the
/// DataPageHeader that includes that offset
#[allow(dead_code)]
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

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct DataPageWrapper<'a, State, Op> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    drop_type: DropType,
    page_no: PageNum,
    page: Option<&'a mut DataPageHeader>,
}

impl<'a, State, Op> PmObjWrapper for DataPageWrapper<'a, State, Op> {}

// TODO: we may be able to combine some DataPageWrapper methods with DirPageWrapper methods
// by making them implement some shared trait - but need to be careful of dynamic dispatch.
// dynamic dispatch may or may not be safe for us there
#[allow(dead_code)]
impl<'a, State, Op> DataPageWrapper<'a, State, Op> {
    pub(crate) fn get_page_no(&self) -> PageNum {
        self.page_no
    }

    pub(crate) fn get_offset(&self) -> u64 {
        match &self.page {
            Some(page) => page.offset,
            None => panic!("ERROR: wrapper does not have a page"),
        }
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        match &self.page {
            Some(page) => page.ino,
            None => panic!("ERROR: wrapper does not have a page"),
        }
    }

    // TODO: should this be unsafe?
    fn take(&mut self) -> Option<&'a mut DataPageHeader> {
        self.page.take()
    }

    fn take_and_make_drop_safe(&mut self) -> Option<&'a mut DataPageHeader> {
        self.drop_type = DropType::Ok;
        self.page.take()
    }
}

// TODO: should check page type?
fn page_no_to_data_header(sbi: &SbInfo, page_no: PageNum) -> Result<&mut DataPageHeader> {
    let page_desc_table = sbi.get_page_desc_table()?;
    let page_index: usize = (page_no - DATA_PAGE_START).try_into()?;
    let ph = page_desc_table.get_mut(page_index);
    if ph.is_none() {
        pr_info!(
            "No space left in page descriptor table - index {:?} out of bounds\n",
            page_index
        );
        Err(ENOSPC)
    } else if let Some(ph) = ph {
        let ph: &mut DataPageHeader = ph.try_into()?;
        Ok(ph)
    } else {
        unreachable!()
    }
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
            drop_type: DropType::Ok,
            page_no,
            page: Some(ph),
        })
    }
}

impl<'a> DataPageWrapper<'a, Clean, Writeable> {
    pub(crate) fn from_page_no(sbi: &'a SbInfo, page_no: PageNum) -> Result<Self> {
        let ph = page_no_to_data_header(&sbi, page_no)?;
        unsafe { Self::wrap_data_page_header(ph, page_no) }
    }

    // TODO: this doesn't need to be unsafe I think
    unsafe fn wrap_data_page_header(ph: &'a mut DataPageHeader, page_no: PageNum) -> Result<Self> {
        if !ph.is_initialized() {
            pr_info!("ERROR: page {:?} is uninitialized\n", page_no);
            Err(EPERM)
        } else {
            Ok(Self {
                state: PhantomData,
                op: PhantomData,
                drop_type: DropType::Ok,
                page_no,
                page: Some(ph),
            })
        }
    }

    /// This method returns a DataPageWrapper ONLY if the page is initialized
    /// Otherwise it returns an error
    pub(crate) fn from_data_page_info(sbi: &'a SbInfo, info: &DataPageInfo) -> Result<Self> {
        let page_no = info.get_page_no();
        let ph = page_no_to_data_header(sbi, page_no)?;
        // wrap_data_page_header checks whether the page is initialized
        unsafe { Self::wrap_data_page_header(ph, page_no) }
    }

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub(crate) fn write_to_page(
        mut self,
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
        let page = self.take();
        Ok((
            len,
            DataPageWrapper {
                state: PhantomData,
                op: PhantomData,
                drop_type: self.drop_type,
                page_no: self.page_no,
                page,
            },
        ))
    }

    #[allow(dead_code)]
    pub(crate) unsafe fn temp_make_written(mut self) -> DataPageWrapper<'a, InFlight, Written> {
        let page = self.take();
        DataPageWrapper {
            state: PhantomData,
            op: PhantomData,
            drop_type: self.drop_type,
            page_no: self.page_no,
            page,
        }
    }

    fn get_page_addr(&self, sbi: &SbInfo) -> Result<*mut u8> {
        let page_addr = unsafe { page_no_to_page(sbi, self.get_page_no())? };
        Ok(page_addr)
    }
}

impl<'a> DataPageWrapper<'a, Clean, ToUnmap> {
    /// Safety: ph must be a valid DataPageHeader reference
    #[allow(dead_code)]
    unsafe fn wrap_page_to_unmap(ph: &'a mut DataPageHeader, page_no: PageNum) -> Result<Self> {
        if !ph.is_initialized() {
            pr_info!("ERROR: page {:?} is uninitialized\n", page_no);
            Err(EPERM)
        } else {
            Ok(Self {
                state: PhantomData,
                op: PhantomData,
                drop_type: DropType::Panic,
                page_no,
                page: Some(ph),
            })
        }
    }

    #[allow(dead_code)]
    pub(crate) fn mark_to_unmap(sbi: &'a SbInfo, info: DataPageInfo) -> Result<Self> {
        let page_no = info.get_page_no();
        let ph = page_no_to_data_header(sbi, page_no)?;
        unsafe { Self::wrap_page_to_unmap(ph, page_no) }
    }

    #[allow(dead_code)]
    pub(crate) fn unmap(mut self) -> DataPageWrapper<'a, Dirty, ClearIno> {
        match &mut self.page {
            Some(page) => page.ino = 0,
            None => panic!("ERROR: Wrapper has no page"),
        };

        let page = self.take_and_make_drop_safe();
        // not ok to drop yet since we want to deallocate all of the
        // pages before dropping them
        DataPageWrapper {
            state: PhantomData,
            op: PhantomData,
            drop_type: DropType::Panic,
            page_no: self.page_no,
            page,
        }
    }
}

impl<'a> DataPageWrapper<'a, Clean, ClearIno> {
    /// Returns in Dealloc state, not Free state, because it's still not safe
    /// to drop the pages until they are all persisted
    pub(crate) fn dealloc(mut self) -> DataPageWrapper<'a, Dirty, Dealloc> {
        match &mut self.page {
            Some(page) => {
                page.page_type = PageType::NONE;
                page.ino = 0;
                page.offset = 0;
            }
            None => panic!("ERROR: Wrapper has no page"),
        }
        let page = self.take_and_make_drop_safe();
        DataPageWrapper {
            state: PhantomData,
            op: PhantomData,
            drop_type: DropType::Panic,
            page_no: self.page_no,
            page,
        }
    }
}

impl<'a> DataPageWrapper<'a, Clean, Free> {
    pub(crate) fn mark_pages_free(
        mut pages: Vec<DataPageWrapper<'a, Clean, Dealloc>>,
    ) -> Result<Vec<Self>> {
        let mut free_vec = Vec::new();
        for mut page in pages.drain(..) {
            let inner = page.take_and_make_drop_safe();
            free_vec.try_push(DataPageWrapper {
                state: PhantomData,
                op: PhantomData,
                drop_type: DropType::Ok,
                page_no: page.page_no,
                page: inner,
            })?;
        }
        Ok(free_vec)
    }
}

impl<'a> DataPageWrapper<'a, Clean, Alloc> {
    /// NOTE: this method returns a clean backpointer, since some pages
    /// will not actually need to be modified here. when they do, this method
    /// flushes and fences
    #[allow(dead_code)]
    pub(crate) fn set_data_page_backpointer(
        mut self,
        inode: &InodeWrapper<'a, Clean, Start, RegInode>,
    ) -> DataPageWrapper<'a, Dirty, Writeable> {
        match &mut self.page {
            Some(page) => page.ino = inode.get_ino(),
            None => panic!("ERROR: Wrapper does not have a page"),
        };
        let page = self.take();
        DataPageWrapper {
            state: PhantomData,
            op: PhantomData,
            drop_type: self.drop_type,
            page_no: self.page_no,
            page,
        }
    }
}

impl<'a, Op> DataPageWrapper<'a, Dirty, Op> {
    pub(crate) fn flush(mut self) -> DataPageWrapper<'a, InFlight, Op> {
        match &self.page {
            Some(page) => flush_buffer(page, mem::size_of::<DataPageHeader>(), false),
            None => panic!("ERROR: Wrapper does not have a page"),
        };

        let page = self.take_and_make_drop_safe();
        DataPageWrapper {
            state: PhantomData,
            op: PhantomData,
            drop_type: self.drop_type,
            page_no: self.page_no,
            page,
        }
    }
}

impl<'a, Op> DataPageWrapper<'a, InFlight, Op> {
    #[allow(dead_code)]
    pub(crate) fn fence(mut self) -> DataPageWrapper<'a, Clean, Op> {
        sfence();
        let page = self.take_and_make_drop_safe();
        DataPageWrapper {
            state: PhantomData,
            op: PhantomData,
            drop_type: self.drop_type,
            page_no: self.page_no,
            page,
        }
    }

    /// Safety: this is only safe to use if it is immediately preceded or
    /// followed by an sfence call. The ONLY place it should be used is in the
    /// macros to fence all objects in a vector.
    pub(crate) unsafe fn fence_unsafe(mut self) -> DataPageWrapper<'a, Clean, Op> {
        let page = self.take();
        DataPageWrapper {
            state: PhantomData,
            op: PhantomData,
            drop_type: self.drop_type,
            page_no: self.page_no,
            page,
        }
    }
}

impl<'a, State, Op> Drop for DataPageWrapper<'a, State, Op> {
    fn drop(&mut self) {
        match self.drop_type {
            DropType::Ok => {}
            DropType::Panic => panic!("ERROR: attempted to drop an undroppable object"),
        };
    }
}
