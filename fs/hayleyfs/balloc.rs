use crate::defs::*;
use core::sync::atomic::{AtomicU64, Ordering};
use kernel::prelude::*;

pub(crate) trait PageAllocator {
    fn alloc_page(&mut self) -> Result<PageNum>;
    fn dealloc_page(&mut self, page: PageNum) -> Result<()>;
}

pub(crate) struct BasicPageAllocator {
    next_page: AtomicU64,
}

/// Allocates by keeping a counter and returning the next number in the counter.
/// Does not support page deallocation.
///
/// # Safety
/// BasicPageAllocator is implemented with AtomicU64 so it is safe to share
/// across threads.
impl BasicPageAllocator {
    #[allow(dead_code)]
    fn new(val: u64) -> Self {
        BasicPageAllocator {
            next_page: AtomicU64::new(val),
        }
    }
}

impl PageAllocator for BasicPageAllocator {
    fn alloc_page(&mut self) -> Result<PageNum> {
        if self.next_page.load(Ordering::SeqCst) == MAX_PAGES {
            Err(ENOSPC)
        } else {
            Ok(self.next_page.fetch_add(1, Ordering::SeqCst))
        }
    }

    fn dealloc_page(&mut self, _: PageNum) -> Result<()> {
        unimplemented!();
    }
}
