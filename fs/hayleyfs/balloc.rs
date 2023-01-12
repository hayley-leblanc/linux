use crate::defs::*;
use crate::dir::*;
use crate::h_inode::*;
use crate::pm::*;
use crate::typestate::*;
use core::{
    marker::PhantomData,
    mem,
    sync::atomic::{AtomicU64, Ordering},
};
use kernel::prelude::*;
use kernel::PAGE_SIZE;

pub(crate) trait PageAllocator {
    fn new(val: u64) -> Self;
    fn alloc_page(&mut self) -> Result<PageNum>;
    fn dealloc_page(&mut self, page: PageNum) -> Result<()>;
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

    fn alloc_page(&mut self) -> Result<PageNum> {
        if self.next_page.load(Ordering::SeqCst) == MAX_PAGES {
            Err(ENOSPC)
        } else {
            Ok(self.next_page.fetch_add(1, Ordering::SeqCst).try_into()?)
        }
    }

    fn dealloc_page(&mut self, _: PageNum) -> Result<()> {
        unimplemented!();
    }
}

#[allow(dead_code)]
struct DirPageHeader {
    page_type: PageType,
    ino: InodeNum,
    dentries: [HayleyFsDentry; DENTRIES_PER_PAGE],
}

#[allow(dead_code)]
pub(crate) struct DirPageWrapper<'a, State, Op> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    page: &'a mut DirPageHeader,
}

impl<'a> DirPageWrapper<'a, Dirty, Alloc> {
    /// Allocate a new page and set it to be a directory page.
    /// Does NOT flush the allocated page.
    pub(crate) fn alloc_dir_page(sbi: &mut SbInfo) -> Result<Self> {
        // TODO: should we zero the page here?
        let page_no = sbi.page_allocator.alloc_page()?;
        let virt_addr = sbi.get_virt_addr();
        let page_size_u64: u64 = PAGE_SIZE.try_into()?;
        let page_addr = unsafe { virt_addr.offset((page_size_u64 * page_no).try_into()?) };
        // cast raw page address to dir page header
        let ph: &mut DirPageHeader = unsafe { &mut *page_addr.cast() };
        ph.page_type = PageType::DIR;
        Ok(DirPageWrapper {
            state: PhantomData,
            op: PhantomData,
            page: ph,
        })
    }
}

impl<'a> DirPageWrapper<'a, Clean, Alloc> {
    /// Requires Initialized inode only as proof that the inode number we are setting points
    /// to an initialized inode
    pub(crate) fn set_dir_page_backpointer<InoState: Initialized>(
        self,
        inode: InodeWrapper<'a, Clean, InoState>,
    ) -> DirPageWrapper<'a, Dirty, Init> {
        self.page.ino = inode.get_ino();
        DirPageWrapper {
            state: PhantomData,
            op: PhantomData,
            page: self.page,
        }
    }
}

impl<'a, Op: Initialized> DirPageWrapper<'a, Clean, Op> {
    /// Obtains a wrapped pointer to a free dentry.
    /// This does NOT allocate the dentry - just obtains a pointer to a free dentry
    /// This requires a mutable reference to self because we need to acquire a
    /// mutable reference to a dentry, but it doesn't actually modify the DirPageWrapper
    pub(crate) fn get_free_dentry(&'a mut self) -> Result<DentryWrapper<'a, Clean, Free>> {
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
            page: self.page,
        }
    }
}
