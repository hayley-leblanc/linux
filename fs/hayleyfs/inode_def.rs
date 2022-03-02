use crate::defs::*;
use core::ptr;
use kernel::bindings::inode_operations;
use kernel::bindings::{
    iget_failed, iget_locked, inode, set_nlink, super_block, unlock_new_inode, I_NEW, S_IFDIR,
};
use kernel::c_default_struct;
use kernel::prelude::*;

pub(crate) type InodeNum = usize;

// reserved inode nums
pub(crate) const HAYLEYFS_ROOT_INO: InodeNum = 1;

pub(crate) static HayleyfsDirInodeOps: inode_operations = inode_operations {
    // mkdir: Some(hayleyfs_mkdir),
    // lookup: Some(hayleyfs_lookup),
    ..c_default_struct!(inode_operations)
};

enum NewInodeType {
    Create,
    Mkdir,
}

// TODO: this probably should not be the static lifetime?
pub(crate) fn hayleyfs_iget(sb: *mut super_block, ino: usize) -> Result<&'static mut inode> {
    let inode = unsafe { &mut *(iget_locked(sb, ino as u64) as *mut inode) };
    if ptr::eq(inode, ptr::null_mut()) {
        unsafe { iget_failed(inode) };
        return Err(Error::ENOMEM);
    }
    if (inode.i_state & I_NEW as u64) == 0 {
        return Ok(inode);
    }
    inode.i_ino = ino as u64;
    // TODO: right now this is hardcoded for directories because
    // that's all we have. but it should be read from the persistent inode
    // and set depending on the type of inode
    inode.i_mode = S_IFDIR as u16;
    inode.i_op = &HayleyfsDirInodeOps;
    unsafe {
        // inode.__bindgen_anon_3.i_fop = &HayleyfsFileOps; // fileOps has to be mutable so this has to be unsafe. Why does it have to be mutable???
        set_nlink(inode, 2);
    }
    unsafe { unlock_new_inode(inode) };

    Ok(inode)
}

mod HayleyfsInode {
    use super::*;

    // inode that lives in PM
    // TODO: reorganize for best memory representation
    #[repr(C)]
    struct HayleyfsInode {
        // valid: bool,
        ino: InodeNum,
        data0: Option<PmPage>,
        mode: u32,
        link_count: u16,
    }

    // we should only be able to modify inodes via an InodeWrapper that
    // handles flushing it and keeping track of the last operation
    // so the private/public stuff has to be set up so the compiler enforces that
    pub(crate) struct InodeWrapper<'a, State = Clean, Op = Read> {
        state: core::marker::PhantomData<State>,
        op: core::marker::PhantomData<Op>,
        inode: &'a mut HayleyfsInode,
    }
}
