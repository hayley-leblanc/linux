//! Test module to mess around with Rust in the kernel
//! Current FS layout:
//! 1st page: superblock
//! 2nd page: inode bitmap
//! 3rd page: inode block
//! 4th page: data bitmap
//! 5th page: data blocks
//! this is a bad layout but it's fine for now

#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]
#![feature(new_uninit)]
#![allow(unused)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::missing_safety_doc)] // TODO: remove

mod data;
mod defs;
mod inode_rs;
mod pm;
mod super_def;
mod tokens;

use crate::data::*;
use crate::defs::*;
use crate::inode_rs::*;
use crate::pm::*;
use crate::super_def::*;
use crate::tokens::*;
use core::mem::size_of;
use core::ptr;

use kernel::bindings::{
    d_make_root, dax_device, dax_direct_access, file_system_type, fs_context,
    fs_context_operations, fs_dax_get_by_bdev, get_next_ino, get_tree_bdev, inc_nlink,
    init_user_ns, inode, inode_init_owner, kill_block_super, new_inode, pfn_t, register_filesystem,
    set_nlink, super_block, super_operations, umode_t, unregister_filesystem, PAGE_SHIFT, S_IFDIR,
    S_IFMT,
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

// try to set it up to do something on mount
// this is a hacky thing stolen from RamFS. if RfL folks say anything
// about that, you should also implement any changes
#[rustfmt::skip]
#[allow(unused)]
mod __anon__ {
    struct file_system_type;
    struct super_operations;
}

#[no_mangle]
static mut HayleyfsFsType: file_system_type = file_system_type {
    name: c_str!("hayleyfs").as_char_ptr(),
    init_fs_context: Some(hayleyfs_init_fs_context),
    kill_sb: Some(kill_block_super),
    ..c_default_struct!(file_system_type)
};

#[no_mangle]
static HayleyfsSuperOps: super_operations = super_operations {
    put_super: Some(hayleyfs_put_super),
    ..c_default_struct!(super_operations)
};

#[no_mangle]
static HayleyfsContextOps: fs_context_operations = fs_context_operations {
    get_tree: Some(hayleyfs_get_tree),
    ..c_default_struct!(fs_context_operations)
};

fn hayleyfs_get_pm_info(sb: *mut super_block, sbi: &mut SbInfo) -> Result<()> {
    // TODO: what happens if this isn't a dax dev?

    sbi.s_daxdev = unsafe { fs_dax_get_by_bdev((*sb).s_bdev, &mut sbi.s_dev_offset as *mut u64) };

    // check for errors on getting the daxdev
    // TODO: what else do you have to check?
    if ptr::eq(sbi.s_daxdev, ptr::null_mut()) {
        pr_alert!("Bad DAX device\n");
        Err(Error::EINVAL)
    } else {
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
            Err(Error::EINVAL)
        } else {
            size *= (PAGE_SIZE as i64);
            sbi.pm_size = u64::try_from(size).unwrap(); // should never fail - size is always positive
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

fn hayleyfs_alloc_sbi(sb: *mut super_block, fc: *mut fs_context) -> Result<()> {
    // according to ramfs port, this is allocated the same as if we
    // used kzalloc with GFP_KERNEL
    let sbi = Box::<SbInfo>::try_new_zeroed();

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
        Err(_) => Err(Error::ENOMEM),
    }
}

fn hayleyfs_set_up_super(sbi: &SbInfo) -> Result<SuperInitToken<'_>> {
    let mut hsb = hayleyfs_get_super(&sbi);

    hsb.size = sbi.pm_size;
    hsb.blocksize = u32::try_from(PAGE_SIZE).unwrap(); // TODO: handle this better
    hsb.magic = HAYLEYFS_MAGIC;

    let token = SuperInitToken::new(hsb);

    Ok(token)
}

// TODO: differentiate between remount and initalization, or at least make sure to wipe old stuff
// every time the file system is mounted for now
#[no_mangle]
pub unsafe extern "C" fn hayleyfs_fill_super(
    sb_raw: *mut super_block,
    fc_raw: *mut fs_context,
) -> i32 {
    let sb = unsafe { &mut *(sb_raw as *mut super_block) };
    let fc = unsafe { &mut *(fc_raw as *mut fs_context) };

    let result = _hayleyfs_fill_super(sb, fc);
    match result {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno(),
    }
}

#[no_mangle]
fn _hayleyfs_fill_super<'a>(
    sb: &'a mut super_block,
    fc: &mut fs_context,
) -> Result<(DirInitToken<'a>, DirPageAddToken<'a>)> {
    pr_info!("mounting the file system!\n");
    hayleyfs_alloc_sbi(sb, fc)?; // TODO: does this need to produce a token?
                                 // what the heck is this function for

    let mut sbi = hayleyfs_get_sbi(sb);
    hayleyfs_get_pm_info(sb, sbi)?;

    sbi.mode = 0o755;
    sbi.uid = unsafe { hayleyfs_current_fsuid() };
    sbi.gid = unsafe { hayleyfs_current_fsgid() };

    let super_token = hayleyfs_set_up_super(&sbi)?;

    let inode_alloc_token = hayleyfs_allocate_inode_by_ino(&sbi, HAYLEYFS_ROOT_INO, &super_token)?;

    let mut inode_init_token = hayleyfs_initialize_inode(*sbi, &inode_alloc_token)?;

    // this all happens in volatile memory and does not require tokens
    // TODO: although might need to be careful dealing with visibility?
    let root_i = hayleyfs_iget(sb, HAYLEYFS_ROOT_INO)?;
    let mut root_i = unsafe { &mut *(root_i as *mut inode) };
    // TODO: convert into a bindgen inode rather than doing this unsafe stuff

    root_i.i_mode = S_IFDIR as u16;
    root_i.i_op = &HayleyfsDirInodeOps;
    set_nlink_safe(root_i, 2);

    // TODO: hide in a function
    unsafe {
        root_i.__bindgen_anon_3.i_fop = &HayleyfsFileOps;
        sb.s_op = &HayleyfsSuperOps;
        sb.s_root = d_make_root(root_i);
    }

    // allocate a data page
    let data_token = hayleyfs_alloc_page(&sbi)?;

    let (dir_init_token, page_add_token) = initialize_dir(
        &sbi,
        inode_init_token,
        HAYLEYFS_ROOT_INO,
        data_token.page_no(),
    )?;

    Ok((dir_init_token, page_add_token))
}

fn hayleyfs_get_super(sbi: &SbInfo) -> &'static mut HayleyfsSuperBlock {
    let hayleyfs_super: &mut HayleyfsSuperBlock =
        unsafe { &mut *(sbi.virt_addr as *mut HayleyfsSuperBlock) };
    hayleyfs_super
}

#[no_mangle]
pub unsafe extern "C" fn hayleyfs_put_super(sb: *mut super_block) {
    pr_info!("Unmounting the file system! Goodbye!\n");
    unsafe {
        // TODO: is this correct? it's from stack overflow
        // need to cast a c_void into SbInfo
        // let sbi: &mut SbInfo = &mut *((*sb).s_fs_info as *mut SbInfo);
        let sbi = hayleyfs_get_sbi(sb);
        hayleyfs_fs_put_dax(sbi.s_daxdev);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hayleyfs_get_tree(fc: *mut fs_context) -> i32 {
    unsafe { get_tree_bdev(fc, Some(hayleyfs_fill_super)) }
}

#[no_mangle]
pub unsafe extern "C" fn hayleyfs_init_fs_context(fc: *mut fs_context) -> c_int {
    // TODO: handle parameters

    unsafe {
        (*fc).ops = &HayleyfsContextOps;
    }
    0
}

// extra attributes here replicate the __init macro
// taken from RamFS port
#[no_mangle]
#[link_section = ".init.text"]
#[cold]
pub extern "C" fn init_hayleyfs() -> c_int {
    unsafe { register_filesystem(&mut HayleyfsFsType) }
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
        unsafe { unregister_filesystem(&mut HayleyfsFsType) };
        pr_info!("Module is unloading. Goodbye!\n");
    }
}
