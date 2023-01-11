use crate::balloc::*;
use crate::defs::*;
use crate::volatile::*;
use core::{
    ffi,
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};
use kernel::prelude::*;
use kernel::{bindings, fs, inode};

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

// TODO: move to namei file?
pub(crate) struct InodeOps;
#[vtable]
impl inode::Operations for InodeOps {
    fn lookup(
        dir: &fs::INode,
        dentry: &mut fs::DEntry,
        _flags: u32,
    ) -> Result<Option<ffi::c_ulong>> {
        // TODO: handle flags
        // TODO: reorganize so that system call logic is separate from
        // conversion from raw pointers

        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };
        let ino_dentry_map = &sbi.ino_dentry_map;

        pr_info!(
            "looking up name {:?} in inode {:?}\n",
            dentry.d_name(),
            dir.i_ino()
        );

        let dentry_vec = ino_dentry_map.lookup_ino(&dir.i_ino());

        if let Some(_dentry_vec) = dentry_vec {
            pr_info!("there is some stuff in the directory\n");
            // TODO: implement lookup in this case
            Err(ENOTSUPP)
        } else {
            Ok(None)
        }
    }

    fn create(
        _mnt_userns: &fs::UserNamespace,
        dir: &fs::INode,
        dentry: &fs::DEntry,
        _umode: bindings::umode_t,
        _excl: bool,
    ) -> Result<i32> {
        pr_info!("creating {:?} in {:?}\n", dentry.d_name(), dir.i_ino());

        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        // get dir pages associated with the parent (if any)
        let parent_ino = dir.i_ino();
        let result = sbi.ino_dir_page_map.lookup_ino(&parent_ino);
        if let Some(_pages) = result {
            unimplemented!();
        } else {
            pr_info!("no pages associated with the parent\n");
            // allocate a page
            let _dir_page = DirPageWrapper::alloc_dir_page(sbi)?.flush().fence();
        }

        Err(EINVAL)
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
