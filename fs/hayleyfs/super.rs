#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]
#![feature(new_uninit)]
#![allow(unused)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::missing_safety_doc)] // TODO: remove

// mod data;
mod defs;
// mod inode_rs;
// mod pm;
// mod recovery;
mod inode_def;
mod super_def;
// mod tokens;

// use crate::data::*;
use crate::defs::*;
// use crate::inode_rs::*;
// use crate::pm::*;
// use crate::recovery::*;
use crate::inode_def::*;
use crate::super_def::*;
// use crate::tokens::*;
use core::mem::size_of;
use core::ptr;

use kernel::bindings::{
    d_make_root, dax_device, dax_direct_access, file_system_type, fs_context,
    fs_context_operations, fs_dax_get_by_bdev, fs_parameter, fs_parameter__bindgen_ty_1,
    fs_parse_result, fs_parse_result__bindgen_ty_1, get_next_ino, get_tree_bdev, inc_nlink,
    init_user_ns, inode, inode_init_owner, kill_block_super, new_inode, pfn_t, register_filesystem,
    set_nlink, super_block, super_operations, umode_t, unregister_filesystem,
    vfs_parse_fs_param_source, ENOMEM, ENOPARAM, PAGE_SHIFT, S_IFDIR, S_IFMT,
};
use kernel::c_types::{c_char, c_int, c_uint, c_ulong, c_void};
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

#[no_mangle]
static mut HayleyfsFsType: file_system_type = file_system_type {
    name: c_str!("hayleyfs").as_char_ptr(),
    init_fs_context: Some(hayleyfs_init_fs_context),
    parameters: hayleyfs_fs_parameters.as_ptr(),
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
    parse_param: Some(hayleyfs_parse_params),
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

fn hayleyfs_alloc_sbi(fc: *mut fs_context, sb: *mut super_block) -> Result<*mut c_void> {
    // according to ramfs port, this is allocated the same as if we
    // used kzalloc with GFP_KERNEL
    let sbi = Box::<SbInfo>::try_new_zeroed();

    match sbi {
        Ok(sbi) => {
            let mut sbi = unsafe { sbi.assume_init() };
            sbi.s_dev_offset = 0; // TODO: what should this be?
            sbi.mount_opts = HayleyfsMountOpts::default();

            let sbi_ptr = Box::into_raw(sbi) as *mut c_void;
            unsafe { (*fc).s_fs_info = sbi_ptr };
            unsafe { (*sb).s_fs_info = sbi_ptr };
            Ok(sbi_ptr)
        }
        Err(_) => Err(Error::ENOMEM),
    }
}

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
fn _hayleyfs_fill_super(sb: &mut super_block, fc: &mut fs_context) -> Result<()> {
    hayleyfs_alloc_sbi(fc, sb)?;

    let mut sbi = hayleyfs_get_sbi(sb);
    sbi.mount_opts = unsafe { *((*fc).fs_private as *mut HayleyfsMountOpts) }; // TODO: abstraction
    hayleyfs_get_pm_info(sb, sbi)?;

    sbi.mode = 0o755;
    sbi.uid = unsafe { hayleyfs_current_fsuid() };
    sbi.gid = unsafe { hayleyfs_current_fsgid() };

    let root_i = hayleyfs_iget(sb, HAYLEYFS_ROOT_INO)?;
    let mut root_i = unsafe { &mut *(root_i as *mut inode) };
    // TODO: convert into a bindgen inode rather than doing this unsafe stuff

    root_i.i_mode = S_IFDIR as u16;
    root_i.i_op = &HayleyfsDirInodeOps;
    set_nlink_safe(root_i, 2);

    // TODO: hide in a function
    unsafe {
        // root_i.__bindgen_anon_3.i_fop = &HayleyfsFileOps;
        sb.s_op = &HayleyfsSuperOps;
        sb.s_root = d_make_root(root_i);
    }

    Ok(())
}

fn hayleyfs_get_super(sbi: &SbInfo) -> &'static mut HayleyfsSuperBlock {
    let hayleyfs_super: &mut HayleyfsSuperBlock =
        unsafe { &mut *(sbi.virt_addr as *mut HayleyfsSuperBlock) };
    hayleyfs_super
}

// TODO: lots of unsafe code here; make it nicer
#[no_mangle]
pub unsafe extern "C" fn hayleyfs_parse_params(
    fc: *mut fs_context,
    param: *mut fs_parameter,
) -> i32 {
    // TODO: put this in a function
    // this is using the bindgen version of fs_parse_result which is why
    // it looks weird
    let mut result = fs_parse_result {
        negated: false,
        __bindgen_anon_1: fs_parse_result__bindgen_ty_1 { uint_64: 0 },
    };

    let opt = unsafe { hayleyfs_fs_parse(fc, hayleyfs_fs_parameters.as_ptr(), param, &mut result) };

    // TODO: there's probably a macro or function that will do this for you
    let opt_init = hayleyfs_param::Opt_init as c_int;
    let opt_source = hayleyfs_param::Opt_source as c_int;
    let opt_crash = hayleyfs_param::Opt_crash as c_int;
    let enoparam = -(ENOPARAM as c_int);

    match opt {
        opt if opt == opt_init => {
            // let mut sbi = hayleyfs_get_sbi_from_fc(fc);
            // TODO: safe abstraction around this
            let mut mount_opts = unsafe { &mut *((*fc).fs_private as *mut HayleyfsMountOpts) };
            mount_opts.init = true;
            pr_info!("opt init done\n");
        }
        opt if opt == opt_source => {
            pr_info!("opt source\n");
            let result = unsafe { vfs_parse_fs_param_source(fc, param) };
            if result < 0 {
                return result;
            }
            pr_info!("opt source done\n");
        }
        opt if opt == opt_crash => {
            pr_info!("opt crash\n");
            // TODO: safe abstraction around this
            let mut mount_opts = unsafe { &mut *((*fc).fs_private as *mut HayleyfsMountOpts) };
            mount_opts.crash_point = unsafe { result.__bindgen_anon_1.uint_32 };
            pr_info!("crash point: {:?}\n", mount_opts.crash_point);
        }
        opt if opt == enoparam => pr_info!("enoparam\n"),
        _ => pr_info!("Unrecognized opt\n"),
    }

    0
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
    // pr_info!("init fs context, alloc sbi\n");
    // pr_info!("{:p}\n", fc);
    // hayleyfs_alloc_sbi(fc); // TODO: handle errors
    let mount_opts = Box::<HayleyfsMountOpts>::try_new_zeroed();
    match mount_opts {
        Ok(mount_opts) => {
            let mut mount_opts = unsafe { mount_opts.assume_init() };
            let opts_ptr = Box::into_raw(mount_opts) as *mut c_void;
            unsafe {
                (*fc).ops = &HayleyfsContextOps;
                (*fc).fs_private = opts_ptr;
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
