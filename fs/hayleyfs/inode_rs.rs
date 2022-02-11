#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]
#![allow(unused)]

use crate::data::*;
use crate::defs::*;
use crate::pm::*;
use crate::super_def::hayleyfs_sb_info;
use core::mem::size_of;
use core::ptr;
use kernel::bindings::{
    dentry, iget_failed, iget_locked, inode, inode_operations, simple_lookup, super_block, umode_t,
    unlock_new_inode, user_namespace,
};
use kernel::c_types::c_void;
use kernel::prelude::*;
use kernel::{c_default_struct, PAGE_SIZE};

// reserved inode nums
pub(crate) const HAYLEYFS_ROOT_INO: usize = 1;

// pub(crate) makes it visible to the whole crate
// not sure why it is not already visible with in the crate...?
pub(crate) static hayleyfs_dir_inode_ops: inode_operations = inode_operations {
    create: Some(hayleyfs_create),
    lookup: Some(hayleyfs_lookup),
    ..c_default_struct!(inode_operations)
};

// inode that lives in PM
// TODO: should this actually be packed?
// TODO: what are these enums going to look like on PM?
// need to be careful about that
#[repr(packed)]
pub(crate) struct hayleyfs_inode {
    pub(crate) data0: Option<pm_page>,
    pub(crate) ino: usize,
    pub(crate) mode: u32,
    pub(crate) link_count: u16,
}

pub(crate) fn hayleyfs_get_inode_by_ino(sbi: &hayleyfs_sb_info, ino: usize) -> &mut hayleyfs_inode {
    let addr = (PAGE_SIZE * 2) + (ino * size_of::<hayleyfs_inode>());
    pr_info!("addr: {:#X}\n", addr);
    // TODO: check that this address does not exceed the inode page
    // TODO: handle possible panic on converting usize to isize here
    let addr = sbi.virt_addr as usize + addr;
    unsafe { &mut *(addr as *mut hayleyfs_inode) }
}

// TODO: this probably should not be the static lifetime
pub(crate) fn hayleyfs_iget(sb: *mut super_block, ino: usize) -> Result<&'static mut inode> {
    let inode = unsafe { &mut *(iget_locked(sb, ino as u64) as *mut inode) };
    if ptr::eq(inode, ptr::null_mut()) {
        unsafe { iget_failed(inode) };
        return Err(Error::EINVAL); // TODO: what error type should this actually return?
    }
    inode.i_ino = ino as u64;
    unsafe { unlock_new_inode(inode) };

    Ok(inode)
}

// TODO: to try for the soft updates thing the bitmap will need a better
// representation in Rust
pub(crate) fn set_inode_bitmap_bit(sbi: &hayleyfs_sb_info, ino: usize) -> Result<()> {
    let addr = sbi.virt_addr as usize + (INODE_BITMAP_PAGE * PAGE_SIZE);
    // TODO: should check that the provided ino is valid and return an error if not
    unsafe { hayleyfs_set_bit(ino.try_into().unwrap(), addr as *mut c_void) };
    // TODO: only flush the updated cache line, not the whole bitmap
    clflush(addr as *const c_void, PAGE_SIZE, true);
    Ok(())
}

unsafe extern "C" fn hayleyfs_create(
    mnt_userns_raw: *mut user_namespace,
    dir_raw: *mut inode,
    dentry_raw: *mut dentry,
    mode: umode_t,
    excl: bool,
) -> i32 {
    // convert arguments to mutable references rather than raw pointers
    // TODO: I bet you could write a macro to do this a bit more cleanly?
    let mnt_userns = unsafe { &mut *(mnt_userns_raw as *mut user_namespace) };
    let dir = unsafe { &mut *(dir_raw as *mut inode) };
    let dentry = unsafe { &mut *(dentry_raw as *mut dentry) };

    // TODO: have this function use nicer Rust errors and convert to something
    // C can understand when it's done
    _hayleyfs_create(mnt_userns, dir, dentry, mode, excl)
}

fn _hayleyfs_create(
    mnt_userns: &mut user_namespace,
    dir: &mut inode,
    dentry: &mut dentry,
    mode: umode_t,
    excl: bool,
) -> i32 {
    pr_info!("creating a new file!\n");
    0
}

unsafe extern "C" fn hayleyfs_lookup(
    dir: *mut inode,
    dentry: *mut dentry,
    flags: u32,
) -> *mut dentry {
    pr_info!("lookup\n");
    unsafe { pr_info!("inode num: {}", (*dir).i_ino) };
    unsafe { pr_info!("dentry name: {:?}", (*dentry).d_name.name) };
    unsafe { simple_lookup(dir, dentry, flags) }
}
