//! Test module to mess around with Rust in the kernel

#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]

use core::ptr;
use kernel::bindings::{
    d_make_root, dax_device, dax_direct_access, file_system_type, fs_context,
    fs_context_operations, fs_dax_get_by_bdev, get_next_ino, get_tree_bdev, inc_nlink,
    init_user_ns, inode, inode_init_owner, inode_operations, kill_block_super, new_inode, pfn_t,
    register_filesystem, super_block, super_operations, umode_t, unregister_filesystem, EINVAL,
    ENOMEM, PAGE_SHIFT, S_IFDIR, S_IFMT,
};
use kernel::c_types::{c_int, c_uint, c_ulong, c_void};
use kernel::prelude::*;
use kernel::{c_default_struct, c_str, PAGE_SIZE};

module! {
    type: HayleyFS,
    name: b"hayleyfs",
    author: b"Hayley LeBlanc",
    description: b"Rust test fs module",
    license: b"GPL v2",
}

struct HayleyFS {}

const LONG_MAX: usize = 9223372036854775807;

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
#[derive(Debug)]
pub struct hayleyfs_mount_opts {
    init: bool,
}

#[repr(C)]
#[derive(Debug)]
pub struct hayleyfs_sb_info {
    sb: *mut super_block, // raw pointer to the VFS super block
    mount_opts: hayleyfs_mount_opts,
    s_daxdev: *mut dax_device, // raw pointer to the dax device we are mounted on
    s_dev_offset: u64,         // no idea what this is used for but a dax fxn needs it
    virt_addr: *mut c_void,    // raw pointer virtual address of beginning of FS instance
    phys_addr: u64,            // physical address of beginning of FS instance
    pm_size: i64,              // size of the PM device
}

#[no_mangle]
static mut hayleyfs_fs_type: file_system_type = file_system_type {
    name: c_str!("hayleyfs").as_char_ptr(),
    init_fs_context: Some(hayleyfs_init_fs_context),
    kill_sb: Some(kill_block_super),
    ..c_default_struct!(file_system_type)
};

static hayleyfs_super_ops: super_operations = super_operations {
    put_super: Some(hayleyfs_put_super),
    ..c_default_struct!(super_operations)
};

static hayleyfs_dir_inode_operations: inode_operations = inode_operations {
    ..c_default_struct!(inode_operations)
};

static hayleyfs_context_ops: fs_context_operations = fs_context_operations {
    get_tree: Some(hayleyfs_get_tree),
    ..c_default_struct!(fs_context_operations)
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
        match mode as c_uint & S_IFMT {
            S_IFDIR => {
                inode.i_op = &hayleyfs_dir_inode_operations;
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

fn hayleyfs_get_pm_info(
    sb: *mut super_block,
    sbi: &mut hayleyfs_sb_info,
) -> core::result::Result<(), u32> {
    // TODO: what happens if this isn't a dax dev?

    sbi.s_daxdev = unsafe { fs_dax_get_by_bdev((*sb).s_bdev, &mut sbi.s_dev_offset as *mut u64) };

    // check for errors on getting the daxdev
    // TODO: what else do you have to check?
    if ptr::eq(sbi.s_daxdev, ptr::null_mut()) {
        pr_alert!("Bad DAX device\n");
        Err(EINVAL)
    } else {
        // device: sbi.s_daxdev
        // page offset: 0
        // number of pages:
        // kaddr output
        // pfn output
        let mut virt_addr: *mut c_void = ptr::null_mut();
        let pfn: *mut pfn_t = ptr::null_mut();
        // TODO: LONG_MAX and PAGE_SIZE are usizes (and we can't change PAGE_SIZE's type)
        // but we need them to be i64s, so we have to convert. this PROBABLY won't fail,
        // but it COULD. figure out a way to better way to deal with this
        let mut size = unsafe {
            dax_direct_access(
                sbi.s_daxdev,
                0,
                (LONG_MAX / PAGE_SIZE).try_into().unwrap(),
                &mut virt_addr,
                pfn,
            )
        };
        if size <= 0 {
            pr_alert!("direct access failed\n");
            Err(EINVAL)
        } else {
            size = size * (PAGE_SIZE as i64);
            sbi.pm_size = size;
            sbi.virt_addr = virt_addr;

            // this calculation taken from NOVA
            // pfn is an absolute PFN translation of our address
            // pfn_t_to_pfn translates from a pfn_t type (which is really a struct)
            // to an unsigned long.
            // not quite sure what the shift is doing here.
            // TODO: figure out if this is correct
            sbi.phys_addr = unsafe { hayleyfs_pfn_t_to_pfn(*pfn) << PAGE_SHIFT };

            Ok(())
        }
    }
}

fn hayleyfs_alloc_sbi(sb: *mut super_block, fc: *mut fs_context) -> core::result::Result<(), u32> {
    // according to ramfs port, this is allocated the same as if we
    // used kzalloc with GFP_KERNEL
    let sbi = Box::<hayleyfs_sb_info>::try_new_zeroed();

    // TODO: need to set stuff here or this won't get mounted
    match sbi {
        Ok(sbi) => {
            let mut sbi = unsafe { sbi.assume_init() };
            sbi.s_dev_offset = 0; // TODO: what should this be?
            sbi.sb = sb;
            let sbi_ptr = Box::into_raw(sbi) as *mut c_void; // this seems wrong but it works
            unsafe { (*fc).s_fs_info = sbi_ptr };
            unsafe { (*sb).s_fs_info = sbi_ptr };
            Ok(())
        }
        Err(_) => Err(ENOMEM),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hayleyfs_fill_super(sb: *mut super_block, fc: *mut fs_context) -> i32 {
    pr_info!("Mounting the file system!\n");
    let inode = hayleyfs_get_inode(sb, ptr::null_mut(), S_IFDIR as umode_t);

    match hayleyfs_alloc_sbi(sb, fc) {
        Ok(_) => {}
        Err(e) => return -(e as c_int),
    }
    let sbi = hayleyfs_get_sbi(sb);
    let res = hayleyfs_get_pm_info(sb, sbi);
    match res {
        Ok(()) => {}
        Err(e) => return -(e as c_int),
    }

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

fn hayleyfs_get_sbi(sb: *mut super_block) -> &'static mut hayleyfs_sb_info {
    let sbi: &mut hayleyfs_sb_info = unsafe { &mut *((*sb).s_fs_info as *mut hayleyfs_sb_info) };
    sbi
}

#[no_mangle]
pub unsafe extern "C" fn hayleyfs_put_super(sb: *mut super_block) {
    pr_info!("Unmounting the file system! Goodbye!\n");
    unsafe {
        // TODO: is this correct? it's from stack overflow
        // need to cast a c_void into hayleyfs_sb_info
        // let sbi: &mut hayleyfs_sb_info = &mut *((*sb).s_fs_info as *mut hayleyfs_sb_info);
        let sbi = hayleyfs_get_sbi(sb);
        hayleyfs_fs_put_dax(sbi.s_daxdev);
    }
}

#[no_mangle]
pub extern "C" fn hayleyfs_get_tree(fc: *mut fs_context) -> i32 {
    unsafe { get_tree_bdev(fc, Some(hayleyfs_fill_super)) }
}

#[no_mangle]
pub unsafe extern "C" fn hayleyfs_init_fs_context(fc: *mut fs_context) -> c_int {
    // TODO: handle parameters

    unsafe {
        (*fc).ops = &hayleyfs_context_ops;
    }
    0
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

        Ok(HayleyFS {})
    }
}

impl Drop for HayleyFS {
    fn drop(&mut self) {
        // pr_info!("My message is {}\n", self.message);
        unsafe { unregister_filesystem(&mut hayleyfs_fs_type) };
        pr_info!("Module is unloading. Goodbye!\n");
    }
}

extern "C" {
    #[allow(improper_ctypes)]
    fn hayleyfs_fs_put_dax(dax_dev: *mut dax_device);
    #[allow(improper_ctypes)]
    fn hayleyfs_pfn_t_to_pfn(pfn: pfn_t) -> u64;
}
