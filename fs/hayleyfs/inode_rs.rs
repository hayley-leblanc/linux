#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]
#![allow(unused)]

use crate::data::*;
use crate::defs::*;
use crate::pm::*;
use crate::super_def::*;
use core::mem::size_of;
use core::ptr;
use kernel::bindings::{
    dentry, iget_failed, iget_locked, inc_nlink, inode, inode_init_owner, inode_operations,
    new_inode, set_nlink, simple_lookup, super_block, umode_t, unlock_new_inode, user_namespace,
    ENAMETOOLONG, S_IFDIR,
};
use kernel::c_types::{c_char, c_int, c_void};
use kernel::prelude::*;
use kernel::str::CStr;
use kernel::{c_default_struct, PAGE_SIZE};

// reserved inode nums
pub(crate) const HAYLEYFS_ROOT_INO: usize = 1;

// pub(crate) makes it visible to the whole crate
// not sure why it is not already visible with in the crate...?
pub(crate) static hayleyfs_dir_inode_ops: inode_operations = inode_operations {
    mkdir: Some(hayleyfs_mkdir),
    lookup: Some(hayleyfs_lookup),
    ..c_default_struct!(inode_operations)
};

enum new_inode_type {
    TYPE_CREATE,
    TYPE_MKDIR,
}

// inode that lives in PM
// TODO: should this actually be packed?
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

fn get_inode_bitmap_addr(sbi: &hayleyfs_sb_info) -> *mut c_void {
    (sbi.virt_addr as usize + (INODE_BITMAP_PAGE * PAGE_SIZE)) as *mut c_void
}

fn hayleyfs_allocate_inode(sbi: &hayleyfs_sb_info) -> Result<usize> {
    let bitmap_addr = get_inode_bitmap_addr(&sbi);

    let ino = unsafe {
        hayleyfs_find_next_zero_bit(
            bitmap_addr as *mut u64,
            0,
            (PAGE_SIZE * 8).try_into().unwrap(),
        )
    };

    if ino == (PAGE_SIZE * 8) {
        Err(Error::ENOSPC)
    } else {
        Ok(ino)
    }
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

    // TODO: have this function use nicer Rust errors and convert to something
    // C can understand when it's done
    _hayleyfs_mkdir(mnt_userns, dir, dentry, mode)
}

// TODO: actual error handling
fn _hayleyfs_mkdir(
    mnt_userns: &mut user_namespace,
    dir: &mut inode,
    dentry: &mut dentry,
    mode: umode_t,
) -> i32 {
    pr_info!("creating a new directory!\n");

    let dentry_name = unsafe { (*dentry).d_name.name } as *const c_char;
    let dentry_name = unsafe { CStr::from_char_ptr(dentry_name) };
    if dentry_name.len() > MAX_FILENAME_LEN {
        pr_info!("dentry name {:?} is too long", dentry_name);
        return -(ENAMETOOLONG as c_int);
    }
    unsafe { pr_info!("dentry name in mkdir: {:?}", dentry_name) };

    let sb = dir.i_sb;
    let sbi = hayleyfs_get_sbi(sb);

    // ignore soft updates stuff for now; this is all going to be very crash unsafe

    // TODO: handle out of inodes case
    let ino = hayleyfs_allocate_inode(&sbi).unwrap();

    // the inode actually probably shouldn't be flushed until later
    let mut pi = hayleyfs_get_inode_by_ino(&sbi, ino);
    pi.ino = ino;
    pi.data0 = None;
    pi.mode = S_IFDIR;
    pi.link_count = 2;
    clflush(&pi, size_of::<hayleyfs_inode>(), true);

    // allocate a data page and set up its dentries
    initialize_dir(&sbi, &mut pi, ino, dir.i_ino.try_into().unwrap()).unwrap();

    // add a dentry to the parent
    let parent_dir = hayleyfs_get_inode_by_ino(&sbi, dir.i_ino.try_into().unwrap());
    add_dentry_to_parent(&sbi, &parent_dir, ino, dentry_name).unwrap();

    // set up vfs inode
    let inode = hayleyfs_new_vfs_inode(sb, dir, ino, mnt_userns, mode, new_inode_type::TYPE_MKDIR);

    // increment parent vfs inode's link count
    unsafe { inc_nlink(dir as *mut inode) };

    0
}

fn hayleyfs_new_vfs_inode(
    sb: *mut super_block,
    dir: &inode,
    ino: usize,
    mnt_userns: &mut user_namespace,
    mode: umode_t,
    new_type: new_inode_type,
) -> *mut inode {
    // TODO: handle errors in here
    let inode = unsafe { new_inode(sb) };

    unsafe {
        inode_init_owner(mnt_userns as *mut user_namespace, inode, dir, mode);
        (*inode).i_ino = ino as u64;
    }

    match new_type {
        new_inode_type::TYPE_MKDIR => unsafe {
            (*inode).i_mode = S_IFDIR as u16;
            (*inode).i_op = &hayleyfs_dir_inode_ops;
            (*inode).__bindgen_anon_3.i_fop = &hayleyfs_file_ops;
            set_nlink(inode, 2);
        },
        new_inode_type::TYPE_CREATE => {
            pr_info!("implement me!");
        }
    }

    inode
}

unsafe extern "C" fn hayleyfs_lookup(
    dir: *mut inode,
    dentry: *mut dentry,
    flags: u32,
) -> *mut dentry {
    pr_info!("lookup\n");
    // let dentry_name = unsafe { &*((*dentry).d_name.name as *[u8] as *[i8])};

    let dentry_name = unsafe { (*dentry).d_name.name } as *const c_char;
    let dentry_name = unsafe { CStr::from_char_ptr(dentry_name) };

    unsafe { pr_info!("inode num: {}", (*dir).i_ino) };
    unsafe { pr_info!("dentry name: {:?}", dentry_name) };
    unsafe { simple_lookup(dir, dentry, flags) }
}
