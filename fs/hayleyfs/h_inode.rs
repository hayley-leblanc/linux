use crate::defs::*;
use crate::typestate::*;
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
    ino: InodeNum,
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
    /// Does not flush the root inode
    pub(crate) unsafe fn init_root_inode(sbi: &SbInfo) -> Result<&HayleyFsInode> {
        let mut root_ino = unsafe { sbi.get_inode_by_ino(ROOT_INO)? };
        root_ino.ino = ROOT_INO;
        root_ino.link_count = 2;
        Ok(root_ino)
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    // TODO: update as fields are added
    pub(crate) fn is_initialized(&self) -> bool {
        self.ino != 0 && self.link_count != 0
    }
}

impl<'a, State, Op> InodeWrapper<'a, State, Op> {
    pub(crate) fn get_ino(&self) -> InodeNum {
        self.inode.get_ino()
    }
}

impl<'a> InodeWrapper<'a, Clean, Start> {
    /// This method assumes that the inode is actually clean - it hasn't been
    /// initialized but not flushed by some other operation. VFS locking should
    /// guarantee that, but
    /// TODO: check on this
    pub(crate) fn get_init_inode_by_ino(sbi: &'a SbInfo, ino: InodeNum) -> Result<Self> {
        let raw_inode = unsafe { sbi.get_inode_by_ino(ino)? };
        if raw_inode.is_initialized() {
            Ok(InodeWrapper {
                state: PhantomData,
                op: PhantomData,
                inode: raw_inode,
            })
        } else {
            Err(EPERM)
        }
    }
}

/// Interface for volatile inode allocator structures
pub(crate) trait InodeAllocator {
    fn alloc_ino(&mut self) -> Result<InodeNum>;
    fn dealloc_ino(&mut self, ino: InodeNum) -> Result<()>;
}

/// Allocates inodes by keeping a counter and returning the next number in the
/// counter. Does not support inode deallocation.
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
