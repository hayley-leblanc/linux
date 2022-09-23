#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]
#![feature(new_uninit)]
#![allow(clippy::missing_safety_doc)] // TODO: remove
#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

mod def;
mod dir;
mod file;
// mod finalize;
mod h_inode;
mod namei;
mod pm;
mod super_def;

// use core::ffi::c_void;
// use kernel::fs::SuperBlock;

use crate::def::*;
// use crate::dir::*;
// use crate::h_inode::hayleyfs_inode::*;
use crate::h_inode::*;
// use crate::namei::*;
use crate::pm::*;
// use crate::super_def::hayleyfs_bitmap::*;
// use crate::super_def::hayleyfs_sb::*;
use crate::super_def::*;
use core::ptr;
// use kernel::bindings::{
//     d_make_root, dax_access_mode_DAX_ACCESS, dax_direct_access, file_system_type, fs_context,
//     fs_context_operations, fs_dax_get_by_bdev, get_tree_bdev, inode, kill_block_super, pfn_t,
//     register_filesystem, super_block, super_operations, unregister_filesystem, ENOMEM, PAGE_SHIFT,
//     S_IFDIR,
// };

// use kernel::bindings;
use kernel::prelude::*;
// use kernel::{c_default_struct, c_str, fs, PAGE_SIZE};
use kernel::{c_str, fs};

module_fs! {
    type: HayleyFS,
    name: "hayleyfs",
    author: "Hayley LeBlanc",
    description: "Rust test fs module",
    license: "GPL v2",
}

#[vtable]
impl fs::Context<Self> for HayleyFS {
    type Data = Box<SbInfo>;

    kernel::define_fs_params! {Box<SbInfo>,
        {flag, "init", |s, v| {
            s.mount_opts.init = Some(v);
            Ok(())}},
    }

    fn try_new() -> Result<Self::Data> {
        pr_info!("creating context\n");
        // sbi.mode = 0o755;
        // sbi.uid = unsafe { hayleyfs_current_fsuid() };
        // sbi.gid = unsafe { hayleyfs_current_fsgid() };

        Ok(alloc_sbi()?)
    }
}

impl fs::Type for HayleyFS {
    type Data = Box<SbInfo>;
    type Context = Self;
    type INodeData = ();
    const SUPER_TYPE: fs::Super = fs::Super::BlockDev; // TODO: or SingleReconf or BlockDev?
    const NAME: &'static CStr = c_str!("hayleyfs");
    const FLAGS: i32 = fs::flags::USERNS_MOUNT | fs::flags::REQUIRES_DEV; // TODO: other options?

    fn fill_super(
        mut data: Self::Data,
        mut sb: fs::NewSuperBlock<'_, Self>,
    ) -> Result<&fs::SuperBlock<Self>> {
        let init = data.mount_opts.init;

        // obtain information about the DAX device
        // TODO: safety?
        data.set_pm_info(&mut sb)?;

        let sb = sb.init(
            data,
            &fs::SuperParams {
                magic: HAYLEYFS_MAGIC,
                ..fs::SuperParams::DEFAULT
            },
        )?;

        let root = if init.is_some() {
            let sbi = sb.get_fs_info();
            unsafe {
                // zero out the PM device to ensure data from previous runs is gone
                // TODO: we should probably use a smarter remount approach; this will be slow
                // TODO: use non-temporal stores to zero the device.
                // TODO: make sure you're writing the right amount of zeros
                let pm_size = sbi.pm_size;
                let virt_addr = sbi.danger_get_pm_addr();
                ptr::write_bytes(virt_addr, 0, (pm_size / 8).try_into()?);
                clwb(virt_addr, pm_size.try_into().unwrap(), true);
            }
            let root_inode = sb.try_new_dcache_dir_inode::<HayleyFS>(fs::INodeParams {
                mode: 0o755,
                ino: 1,
                value: (),
            })?;

            // set up persistent structures

            sb.try_new_root_dentry(root_inode)?
        } else {
            unimplemented!();
        };

        let sb = sb.init_root(root)?;

        Ok(sb)
    }
}

#[no_mangle]
fn alloc_sbi() -> Result<Box<SbInfo>> {
    Ok(Box::try_new(SbInfo::new())?)
}

// #[no_mangle]
// static mut HayleyfsFsType: file_system_type = file_system_type {
//     name: c_str!("hayleyfs").as_char_ptr(),
//     init_fs_context: Some(hayleyfs_init_fs_context),
//     // parameters: hayleyfs_fs_parameters.as_ptr(),
//     kill_sb: Some(kill_block_super),
//     ..c_default_struct!(file_system_type)
// };

// #[no_mangle]
// static HayleyfsSuperOps: super_operations = super_operations {
//     put_super: Some(hayleyfs_put_super),
//     ..c_default_struct!(super_operations)
// };

// #[no_mangle]
// static HayleyfsContextOps: fs_context_operations = fs_context_operations {
//     get_tree: Some(hayleyfs_get_tree),
//     // parse_param: Some(hayleyfs_parse_params),
//     ..c_default_struct!(fs_context_operations)
// };

// // /*
// //  * Initialization dependencies
// //  * Horizontal line through arrows means that the prior operation(s) must
// //  * be flushed and fenced before the subsequent ones are allowed to occur
// //  * THIS IS NOT UP TO DATE
// //  * TODO: update with more detailed directory page initialization
// //  *                             ┌──────────────┐
// //  *                             │              │
// //  *                             │ zero bitmaps │
// //  *                             │              │
// //  *                             └──────┬───────┘
// //  *                                    │
// //  *                                ────┼────
// //  *                                    │
// //  *                        ┌───────────▼─────────────┐
// //  *                        │                         │
// //  *           ┌────────────┤ allocate reserved pages ├───────────┐
// //  *           │            │                         │           │
// //  *           │            └───────────┬─────────────┘           │
// //  *           │                        │                         │
// //  *           │                      ──┼─────────────────────────┼──
// //  *           │                        │                         │
// //  * ┌─────────▼──────────┐   ┌─────────▼───────────┐   ┌─────────▼──────────┐
// //  * │                    │   │                     │   │                    │
// //  * │ set up super block │   │ allocate root inode │   │ allocate dir page  │
// //  * │                    │   │                     │   │                    │
// //  * └────────────────────┘   └─────────┬────────┬──┘   └─────────┬──────────┘
// //  *                                    │        │                │
// //  *                            ────────┼────────┼────────────────┼─────────
// //  *                                    │        │                │
// //  *                                    │        └────────┐       │
// //  *                                    │                 │       │
// //  *                          ┌─────────▼────────┐     ┌──▼───────▼──────────┐
// //  *                          │                  │     │                     │
// //  *                          │ initialize inode │     │ initialize root dir │
// //  *                          │                  │     │                     │
// //  *                          └───────────────┬──┘     └──┬──────────────────┘
// //  *                                          │           │
// //  *                                    ──────┼───────────┼─────
// //  *                                          │           │
// //  *                                      ┌───▼───────────▼───┐
// //  *                                      │                   │
// //  *                                      │ add page to inode │
// //  *                                      │                   │
// //  *                                      └───────────────────┘
// //  */
// // #[no_mangle]
// // fn _hayleyfs_fill_super(sb: &mut super_block, fc: &mut fs_context) -> Result<()> {
// //     let result = hayleyfs_alloc_sbi(fc, sb);
// //     // unsafe because we don't check whether the result is a valid error
// //     unsafe {
// //         if hayleyfs_is_err(result) {
// //             // let err = hayleyfs_ptr_err(result);
// //             // TODO: we should really use a more generic fxn to convert
// //             // from u64 to kernel error, but those fxns aren't accessible from here
// //             // right now. It should also be enomem, not einval, but for some reason
// //             // the compiler won't let us use enomem here
// //             return Err(EINVAL);
// //         }
// //     }

// //     let mut sbi = hayleyfs_get_sbi(sb);
// //     sbi.mount_opts = unsafe { *((*fc).fs_private as *mut HayleyfsMountOpts) }; // TODO: abstraction
// //     hayleyfs_get_pm_info(sb, sbi)?;

// //     sbi.mode = 0o755;
// //     sbi.uid = unsafe { hayleyfs_current_fsuid() };
// //     sbi.gid = unsafe { hayleyfs_current_fsgid() };

// //     let root_i = hayleyfs_iget(sb, HAYLEYFS_ROOT_INO)?;
// //     let mut root_i = unsafe { &mut *(root_i as *mut inode) };

// //     root_i.i_mode = sbi.mode | S_IFDIR as u16;
// //     pr_info!("setting root inode dir iops\n");
// //     root_i.i_op = &HayleyfsDirInodeOps;
// //     set_nlink_safe(root_i, 2);

// //     // pr_info!("{:?}\n", root_i);

// //     if sbi.mount_opts.init {
// //         // TODO: we probably shouldn't actually do this. very slow. but it will let
// //         // me procrastinate on how to actually make sure data from old mounts doesn't
// //         // stick around. unless this ends up being incredibly slow
// //         // if you wanted to be fancy about it you could use nontemporal stores
// //         // but then you'd have to implement that and that would defeat the point of this
// //         // janky quick workaround
// //         unsafe {
// //             // TODO: make sure you're writing the right amount of zeros
// //             ptr::write_bytes(sbi.virt_addr, 0, (sbi.pm_size / 8).try_into()?);
// //             clwb(sbi.virt_addr, sbi.pm_size.try_into().unwrap(), true);
// //         }
// //         // TODO: for some reason there are extra fences here

// //         // zero bitmaps
// //         let inode_bitmap = BitmapWrapper::read_inode_bitmap(sbi).zero_bitmap()?;
// //         let mut data_bitmap = BitmapWrapper::read_data_bitmap(sbi).zero_bitmap()?;

// //         // allocate reserved pages
// //         let data_bitmap = set_bits!(
// //             data_bitmap,
// //             SUPER_BLOCK_PAGE,
// //             INODE_BITMAP_PAGE,
// //             INODE_PAGE,
// //             DATA_BITMAP_PAGE
// //         )?
// //         .persist();

// //         // initialize super block
// //         let _sb = SuperBlockWrapper::init(sbi, &data_bitmap);

// //         // let (page_no, data_bitmap) = data_bitmap.alloc_root_ino_page(&inode_bitmap)?;
// //         let (root_ino, inode_bitmap) = inode_bitmap.alloc_root_ino(&data_bitmap)?;

// //         // TODO: do we need to use data bitmap again?
// //         // let (_data_bitmap, inode_bitmap) = fence_all!(data_bitmap, inode_bitmap);
// //         let inode_bitmap = inode_bitmap.fence();

// //         let pi = InodeWrapper::read_dir_inode(sbi, &root_ino).initialize_root_inode(
// //             sb,
// //             sbi,
// //             root_i,
// //             &inode_bitmap,
// //         );

// //         // initialize root dir page
// //         // let self_dentry = pi.get_new_dentry(sbi)?.initialize_dentry(root_ino, ".")?;
// //         // let parent_dentry = pi.get_new_dentry(sbi)?.initialize_dentry(root_ino, "..")?;
// //         let (self_dentry, pi) = pi.get_new_dentry(sbi, root_i)?;
// //         let self_dentry = self_dentry.initialize_dentry(root_ino, ".");
// //         let (parent_dentry, pi) = pi.get_new_dentry(sbi, root_i)?;
// //         let parent_dentry = parent_dentry.initialize_dentry(root_ino, "..");

// //         // TODO: finalize dentries
// //         let (_pi, _self_dentry, _parent_dentry) = fence_all!(pi, self_dentry, parent_dentry);

// //         // add page to inode
// //         // TODO: how do we enforce the use of the fence?
// //         // TODO: finalize inode wrapper more explicitly
// //         // let _pi = pi.add_dir_page_fence(sbi, root_i, page_no, self_dentry, parent_dentry)?;
// //         // let _pi = pi.add_dir_page(page_no, root_i, data_bitmap)?;
// //     } // else {
// //       // hayleyfs_recovery(sbi)?;
// //       //}

// //     // TODO: hide in a function
// //     unsafe {
// //         root_i.__bindgen_anon_3.i_fop = &HayleyfsDirOps;
// //         sb.s_op = &HayleyfsSuperOps;
// //         sb.s_root = d_make_root(root_i);
// //     }

// //     Ok(())
// // }

// // // TODO: lots of unsafe code here; make it nicer
// // #[no_mangle]
// // pub unsafe extern "C" fn hayleyfs_parse_params(
// //     fc: *mut fs_context,
// //     param: *mut fs_parameter,
// // ) -> i32 {
// //     // TODO: put this in a function
// //     // this is using the bindgen version of fs_parse_result which is why
// //     // it looks weird
// //     let mut result = fs_parse_result {
// //         negated: false,
// //         __bindgen_anon_1: fs_parse_result__bindgen_ty_1 { uint_64: 0 },
// //     };

// //     let opt = unsafe { hayleyfs_fs_parse(fc, hayleyfs_fs_parameters.as_ptr(), param, &mut result) };

// //     // TODO: there's probably a macro or function that will do this for you
// //     let opt_init = hayleyfs_param::Opt_init as c_int;
// //     let opt_source = hayleyfs_param::Opt_source as c_int;
// //     let opt_crash = hayleyfs_param::Opt_crash as c_int;
// //     let enoparam = -(ENOPARAM as c_int);

// //     match opt {
// //         opt if opt == opt_init => {
// //             // TODO: safe abstraction around this
// //             let mount_opts = unsafe { &mut *((*fc).fs_private as *mut HayleyfsMountOpts) };
// //             mount_opts.init = true;
// //             pr_info!("opt init done\n");
// //         }
// //         opt if opt == opt_source => {
// //             pr_info!("opt source\n");
// //             let result = unsafe { vfs_parse_fs_param_source(fc, param) };
// //             if result < 0 {
// //                 return result;
// //             }
// //             pr_info!("opt source done\n");
// //         }
// //         opt if opt == opt_crash => {
// //             pr_info!("opt crash\n");
// //             // TODO: safe abstraction around this
// //             let mut mount_opts = unsafe { &mut *((*fc).fs_private as *mut HayleyfsMountOpts) };
// //             mount_opts.crash_point = unsafe { result.__bindgen_anon_1.uint_32 };
// //             pr_info!("crash point: {:?}\n", mount_opts.crash_point);
// //         }
// //         opt if opt == enoparam => pr_info!("enoparam\n"),
// //         _ => pr_info!("Unrecognized opt\n"),
// //     }

// //     0
// // }

// #[no_mangle]
// pub unsafe extern "C" fn hayleyfs_put_super(sb: *mut super_block) {
//     pr_info!("Unmounting the file system! Goodbye!\n");
//     unsafe {
//         // TODO: is this correct? it's from stack overflow
//         // need to cast a c_void into SbInfo
//         // let sbi: &mut SbInfo = &mut *((*sb).s_fs_info as *mut SbInfo);
//         let sbi = hayleyfs_get_sbi(sb);
//         hayleyfs_fs_put_dax(sbi.s_daxdev);
//     }
// }
