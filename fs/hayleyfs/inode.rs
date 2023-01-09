use crate::defs::*;
use core::{
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};
use kernel::prelude::*;

/// Persistent inode structure
/// It is always unsafe to access this structure directly
/// TODO: add the rest of the fields
#[repr(C)]
pub(crate) struct HayleyFsInode {
    link_count: u16,
}

#[allow(dead_code)]
pub(crate) struct InodeWrapper<'a, State, Op> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    inode: &'a HayleyFsInode,
}

impl HayleyFsInode {
    /// Unsafe inode constructor for temporary use with init_fs only
    pub(crate) unsafe fn init_root_inode(sbi: &SbInfo) -> Result<&HayleyFsInode> {
        let mut root_ino = unsafe { sbi.get_inode_by_ino(ROOT_INO)? };
        root_ino.link_count = 2;
        Ok(root_ino)
    }
}

/// Interface for volatile inode allocator structures
pub(crate) trait InodeAllocator {
    fn alloc_ino(&mut self) -> Result<InodeNum>;
    fn dealloc_ino(&mut self, ino: InodeNum) -> Result<()>;
}

/// Allocates inodes by keeping a counter and returning the next number in the
/// counter. Does not currently support inode deletion.
///
/// # Safety
/// BasicInodeAllocator is implemented with AtomicU64 so it is safe to share
/// across threads.
pub(crate) struct BasicInodeAllocator {
    next_inode: AtomicU64,
}

impl BasicInodeAllocator {
    #[allow(dead_code)]
    fn new(val: u64) -> Self {
        BasicInodeAllocator {
            next_inode: AtomicU64::new(val),
        }
    }
}

impl InodeAllocator for BasicInodeAllocator {
    fn alloc_ino(&mut self) -> Result<InodeNum> {
        if self.next_inode.load(Ordering::SeqCst) == NUM_INODES {
            Err(ENOSPC)
        } else {
            Ok(self.next_inode.fetch_add(1, Ordering::SeqCst))
        }
    }
    fn dealloc_ino(&mut self, _: InodeNum) -> Result<()> {
        unimplemented!();
    }
}
