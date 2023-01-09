use crate::defs::*;
use core::marker::PhantomData;
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
