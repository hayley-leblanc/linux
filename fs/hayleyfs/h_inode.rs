use crate::balloc::*;
use crate::defs::*;
use crate::pm::*;
use crate::typestate::*;
use core::{
    marker::PhantomData,
    mem,
    sync::atomic::{AtomicU64, Ordering},
};
use kernel::prelude::*;

// ZSTs for representing inode types
// These are not typestate since they don't change, but they are a generic
// parameter for inodes so that the compiler can check that we are using
// the right kind of inode
pub(crate) struct RegInode {}
pub(crate) struct DirInode {}

pub(crate) trait AnyInode {}
impl AnyInode for RegInode {}
impl AnyInode for DirInode {}

/// Persistent inode structure
/// It is always unsafe to access this structure directly
/// TODO: add the rest of the fields
#[repr(C)]
#[derive(Debug)]
pub(crate) struct HayleyFsInode {
    link_count: u16,
    inode_type: InodeType,
    size: u64,
    ino: InodeNum,
    _padding: u64,
}

#[allow(dead_code)]
pub(crate) struct InodeWrapper<'a, State, Op, Type> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    inode_type: PhantomData<Type>,
    ino: InodeNum,
    inode: &'a mut HayleyFsInode,
}

impl HayleyFsInode {
    /// Unsafe inode constructor for temporary use with init_fs only
    /// Does not flush the root inode
    pub(crate) unsafe fn init_root_inode(sbi: &SbInfo) -> Result<&HayleyFsInode> {
        let mut root_ino = unsafe { sbi.get_inode_by_ino(ROOT_INO)? };
        root_ino.ino = ROOT_INO;
        root_ino.link_count = 2;
        root_ino.size = 4096; // dir size always set to 4KB
        root_ino.inode_type = InodeType::DIR;
        Ok(root_ino)
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    pub(crate) fn get_link_count(&self) -> u16 {
        self.link_count
    }

    pub(crate) fn get_type(&self) -> InodeType {
        self.inode_type
    }

    pub(crate) unsafe fn inc_link_count(&mut self) {
        self.link_count += 1
    }

    // TODO: update as fields are added
    pub(crate) fn is_initialized(&self) -> bool {
        self.ino != 0 && self.link_count != 0 && self.inode_type != InodeType::NONE
    }

    // TODO: update as fields are added
    pub(crate) fn is_free(&self) -> bool {
        self.ino == 0 && self.link_count == 0 && self.inode_type == InodeType::NONE
    }
}

impl<'a, State, Op, Type> InodeWrapper<'a, State, Op, Type> {
    pub(crate) fn get_ino(&self) -> InodeNum {
        self.inode.get_ino()
    }
}

impl<'a, State, Op, Type> InodeWrapper<'a, State, Op, Type> {
    // TODO: this needs to be handled specially for types so that type generic cannot be incorrect
    pub(crate) fn wrap_inode(
        ino: InodeNum,
        inode: &'a mut HayleyFsInode,
    ) -> InodeWrapper<'a, State, Op, Type> {
        InodeWrapper {
            state: PhantomData,
            op: PhantomData,
            inode_type: PhantomData,
            ino,
            inode,
        }
    }

    pub(crate) fn new<NewState, NewOp>(
        i: InodeWrapper<'a, State, Op, Type>,
    ) -> InodeWrapper<'a, NewState, NewOp, Type> {
        InodeWrapper {
            state: PhantomData,
            op: PhantomData,
            ino: i.ino,
            inode_type: i.inode_type,
            inode: i.inode,
        }
    }

    pub(crate) fn get_type(&self) -> InodeType {
        self.inode.get_type()
    }
}

impl<'a, Type> InodeWrapper<'a, Clean, Start, Type> {
    pub(crate) fn inc_link_count(self) -> Result<InodeWrapper<'a, Dirty, IncLink, Type>> {
        if self.inode.get_link_count() == MAX_LINKS {
            Err(EMLINK)
        } else {
            unsafe { self.inode.inc_link_count() };
            Ok(Self::new(self))
        }
    }

    // TODO: get the number of bytes written from the page itself, somehow?
    pub(crate) fn inc_size(
        self,
        bytes_written: u64,
        page: DataPageWrapper<'a, Clean, Written>,
    ) -> (u64, InodeWrapper<'a, Clean, IncSize, Type>) {
        let total_size = bytes_written + page.get_offset();
        if self.inode.size < total_size {
            self.inode.size = total_size;
            flush_buffer(self.inode, mem::size_of::<HayleyFsInode>(), true);
        }
        (
            self.inode.size,
            InodeWrapper {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                ino: self.ino,
                inode: self.inode,
            },
        )
    }
}

impl<'a> InodeWrapper<'a, Clean, Free, RegInode> {
    pub(crate) fn get_free_reg_inode_by_ino(sbi: &'a SbInfo, ino: InodeNum) -> Result<Self> {
        let raw_inode = unsafe { sbi.get_inode_by_ino(ino)? };
        if raw_inode.is_free() {
            Ok(InodeWrapper {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                ino,
                inode: raw_inode,
            })
        } else {
            Err(EPERM)
        }
    }

    pub(crate) fn allocate_file_inode(self) -> InodeWrapper<'a, Dirty, Alloc, RegInode> {
        self.inode.link_count = 1;
        self.inode.ino = self.ino;
        self.inode.inode_type = InodeType::REG;
        Self::new(self)
    }
}

impl<'a> InodeWrapper<'a, Clean, Free, DirInode> {
    pub(crate) fn get_free_dir_inode_by_ino(sbi: &'a SbInfo, ino: InodeNum) -> Result<Self> {
        let raw_inode = unsafe { sbi.get_inode_by_ino(ino)? };
        if raw_inode.is_free() {
            Ok(InodeWrapper {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                ino,
                inode: raw_inode,
            })
        } else {
            Err(EPERM)
        }
    }

    pub(crate) fn allocate_dir_inode(self) -> InodeWrapper<'a, Dirty, Alloc, DirInode> {
        self.inode.link_count = 2;
        self.inode.ino = self.ino;
        self.inode.inode_type = InodeType::DIR;
        Self::new(self)
    }
}

impl<'a, Op, Type> InodeWrapper<'a, Dirty, Op, Type> {
    pub(crate) fn flush(self) -> InodeWrapper<'a, InFlight, Op, Type> {
        flush_buffer(self.inode, mem::size_of::<HayleyFsInode>(), false);
        Self::new(self)
    }
}

impl<'a, Op, Type> InodeWrapper<'a, InFlight, Op, Type> {
    pub(crate) fn fence(self) -> InodeWrapper<'a, Clean, Op, Type> {
        sfence();
        Self::new(self)
    }
}

/// Interface for volatile inode allocator structures
pub(crate) trait InodeAllocator {
    fn new(val: u64) -> Self;
    fn alloc_ino(&self) -> Result<InodeNum>;
    fn dealloc_ino(&self, ino: InodeNum) -> Result<()>;
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

impl InodeAllocator for BasicInodeAllocator {
    fn new(val: u64) -> Self {
        BasicInodeAllocator {
            next_inode: AtomicU64::new(val),
        }
    }

    fn alloc_ino(&self) -> Result<InodeNum> {
        if self.next_inode.load(Ordering::SeqCst) == NUM_INODES {
            Err(ENOSPC)
        } else {
            Ok(self.next_inode.fetch_add(1, Ordering::SeqCst))
        }
    }

    fn dealloc_ino(&self, _: InodeNum) -> Result<()> {
        unimplemented!();
    }
}
