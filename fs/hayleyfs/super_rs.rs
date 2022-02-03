//! Test module to mess around with Rust in the kernel

#![allow(non_camel_case_types)]

use core::ptr;
use kernel::bindings::{
    d_make_root, dentry, file_system_type, fs_context, get_next_ino, inc_nlink, init_user_ns,
    inode, inode_init_owner, inode_operations, kill_block_super, mount_bdev, new_inode, ram_aops,
    register_filesystem, super_block, super_operations, umode_t, unregister_filesystem, ENOMEM,
    S_IFDIR, S_IFMT,
};
use kernel::c_types::{c_char, c_int, c_uint, c_ulong, c_void};
use kernel::prelude::*;
use kernel::{c_default_struct, c_str};

module! {
    type: HayleyFS,
    name: b"hayleyfs",
    author: b"Hayley LeBlanc",
    description: b"Rust test fs module",
    license: b"GPL v2",
}

struct HayleyFS {
    val: c_int,
}

// try to set it up to do something on mount
// this is a hacky thing stolen from RamFS. if RfL folks say anything
// about that, you should also implement any changes
#[rustfmt::skip]
#[allow(unused)]
mod __anon__ {
    struct file_system_type;
    struct super_operations;
}

#[repr(C)]
/// cbindgen:ignore
pub struct hayleyfs_mount_opts {
    init: bool,
}

#[repr(C)]
/// cbindgen:ignore
pub struct hayleyfs_fs_info {
    mount_opts: hayleyfs_mount_opts,
}

// TODO: what does no_mangle do?
#[no_mangle]
static mut hayleyfs_fs_type: file_system_type = file_system_type {
    name: c_str!("hayleyfs").as_char_ptr(),
    // mount: Some(hayleyfs_mount),
    init_fs_context: Some(hayleyfs_init_fs_context),
    kill_sb: Some(kill_block_super),
    ..c_default_struct!(file_system_type)
};

#[no_mangle]
static mut hayleyfs_super_ops: super_operations = super_operations {
    put_super: Some(hayleyfs_put_super),
    ..c_default_struct!(super_operations)
};

static hayleyfs_dir_inode_operations: inode_operations = inode_operations {
    ..c_default_struct!(inode_operations)
};

pub fn hayleyfs_get_inode(sb: *mut super_block, dir: *const inode, mode: umode_t) -> *mut inode {
    // TODO: obviously this does not do enough for most cases but it might
    // be enough to get it to mount
    let inode = unsafe { new_inode(sb) };
    if !ptr::eq(inode, ptr::null_mut()) {
        let inode = unsafe { inode.as_mut().unwrap() };
        inode.i_ino = unsafe { get_next_ino() } as c_ulong;
        unsafe {
            inode_init_owner(&mut init_user_ns, inode, dir, mode);
        }
        // unsafe {
        //     inode.i_mapping.as_mut().unwrap().a_ops = &ram_aops;
        // }
        // unsafe {
        //     ramfs_mapping_set_gfp_mask(inode.i_mapping, ramfs_get_gfp_highuser());
        // }
        // unsafe {
        //     ramfs_mapping_set_unevictable(inode.i_mapping);
        // }
        match mode as c_uint & S_IFMT {
            S_IFDIR => {
                inode.i_op = unsafe { &hayleyfs_dir_inode_operations };
                /* directory inodes start off with i_nlink == 2 (for "." entry) */
                unsafe {
                    inc_nlink(inode);
                }
            }
            _ => {} // do nothing for now
        }
    }
    inode
}

#[no_mangle]
pub unsafe extern "C" fn hayleyfs_fill_super(sb: *mut super_block, fc: *mut fs_context) -> i32 {
    pr_info!("Mounting the file system!\n");
    let inode = unsafe { hayleyfs_get_inode(sb, ptr::null_mut(), S_IFDIR as umode_t) };

    unsafe {
        (*sb).s_op = &hayleyfs_super_ops;
        (*sb).s_root = d_make_root(inode);
    }
    let s_root = unsafe { (*sb).s_root };
    if ptr::eq(s_root, ptr::null_mut()) {
        -(ENOMEM as c_int)
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn hayleyfs_put_super(sb: *mut super_block) {
    pr_info!("Unmounting the file system! Goodbye!\n");
}

#[no_mangle]
pub extern "C" fn hayleyfs_get_tree_rust(fc: *mut fs_context) -> i32 {
    pr_info!("get tree\n");
    unsafe { hayleyfs_get_tree(fc) }
}

#[no_mangle]
pub unsafe extern "C" fn hayleyfs_init_fs_context(fc: *mut fs_context) -> c_int {
    // just copying the stuff from ramfs here for now
    let fsi = Box::<hayleyfs_fs_info>::try_new_zeroed();
    match fsi {
        Ok(fsi) => {
            let mut fsi = unsafe { fsi.assume_init() };
            fsi.mount_opts.init = true;
            unsafe {
                hayleyfs_fs_context_set_fs_info(fc, &fsi);
                hayleyfs_fs_context_set_ops(fc, &hayleyfs_context_ops);
            }
            0
        }
        Err(_) => -(ENOMEM as c_int),
    }
}

// extra attributes here replicate the __init macro
// taken from RamFS port
#[no_mangle]
#[link_section = ".init.text"]
#[cold]
pub extern "C" fn init_hayleyfs() -> c_int {
    unsafe { register_filesystem(&mut hayleyfs_fs_type) }
}

impl KernelModule for HayleyFS {
    fn init(_name: &'static CStr, _module: &'static ThisModule) -> Result<Self> {
        pr_info!("Hello! This is Hayley's Rust module!\n");

        let ret = init_hayleyfs();

        Ok(HayleyFS { val: ret })
    }
}

impl Drop for HayleyFS {
    fn drop(&mut self) {
        // pr_info!("My message is {}\n", self.message);
        unsafe { unregister_filesystem(&mut hayleyfs_fs_type) };
        pr_info!("Module is unloading. Goodbye!\n");
    }
}

#[repr(C)]
/// cbindgen:ignore
struct fs_context_operations {
    /* same thing that bindgen generates for seemingly opaque types */
    _unused: [u8; 0],
}

extern "C" {
    #[allow(improper_ctypes)]
    static hayleyfs_context_ops: fs_context_operations;
    #[allow(improper_ctypes)]
    fn hayleyfs_get_tree(fc: *mut fs_context) -> i32;
    #[allow(improper_ctypes)]
    fn hayleyfs_fs_context_set_ops(fc: *mut fs_context, ops: *const fs_context_operations);
    #[allow(improper_ctypes)]
    fn hayleyfs_fs_context_set_fs_info(fc: *mut fs_context, info: *const hayleyfs_fs_info);
}
