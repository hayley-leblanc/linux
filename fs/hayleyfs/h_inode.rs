use crate::def::*;
use crate::inode_def::*;
use crate::super_def::hayleyfs_bitmap::*;
use crate::super_def::*;
use core::ptr::{eq, null_mut};
use kernel::bindings::{
    dentry, iget_failed, iget_locked, inode, inode_operations, set_nlink, super_block, umode_t,
    unlock_new_inode, user_namespace, I_NEW, S_IFDIR,
};
use kernel::c_default_struct;
use kernel::c_types::c_char;
use kernel::prelude::*;

pub(crate) static HayleyfsDirInodeOps: inode_operations = inode_operations {
    mkdir: Some(hayleyfs_mkdir),
    // lookup: Some(hayleyfs_lookup),
    ..c_default_struct!(inode_operations)
};

// TODO: this probably should not be the static lifetime?
pub(crate) fn hayleyfs_iget(sb: *mut super_block, ino: usize) -> Result<&'static mut inode> {
    let inode = unsafe { &mut *(iget_locked(sb, ino as u64) as *mut inode) };
    if eq(inode, null_mut()) {
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

#[no_mangle]
unsafe extern "C" fn hayleyfs_mkdir(
    mnt_userns_raw: *mut user_namespace,
    dir_raw: *mut inode,
    dentry_raw: *mut dentry,
    mode: umode_t,
) -> i32 {
    // convert arguments to mutable references rather than raw pointers
    // TODO: I bet you could write a macro to do this a bit more cleanly?
    let mnt_userns = unsafe { &mut *(mnt_userns_raw as *mut user_namespace) };
    let dir = unsafe { &mut *(dir_raw as *mut inode) };
    let dentry = unsafe { &mut *(dentry_raw as *mut dentry) };

    let result = _hayleyfs_mkdir(mnt_userns, dir, dentry, mode);
    match result {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno(),
    }
}

#[no_mangle]
fn _hayleyfs_mkdir(
    mnt_userns: &mut user_namespace,
    dir: &mut inode,
    dentry: &mut dentry,
    mode: umode_t,
) -> Result<()> {
    pr_info!("creating a new directory\n");

    let sb = dir.i_sb;
    let sbi = hayleyfs_get_sbi(sb);

    let dentry_name = unsafe { CStr::from_char_ptr((*dentry).d_name.name as *const c_char) };
    if dentry_name.len() > MAX_FILENAME_LEN {
        pr_info!("dentry name {:?} is too long", dentry_name);
        return Err(Error::ENAMETOOLONG);
    }

    // let ino = allocate_inode(sbi)?;
    // pr_info!("{:?}\n", ino.get_val());

    Ok(())
}

#[no_mangle]
fn allocate_inode(sbi: &SbInfo) -> Result<()> {
    // let bitmap = InodeBitmapWrapper::read_bitmap(sbi);

    // let ino = bitmap.find_and_set_next_zero_bit()?;
    // Ok(ino)
    Ok(())
}
