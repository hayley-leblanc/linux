use crate::balloc::*;
use crate::defs::*;
use crate::dir::*;
use crate::h_inode::*;
use crate::typestate::*;
use crate::volatile::*;
use core::ffi;
use kernel::prelude::*;
use kernel::{bindings, error, fs, inode};

pub(crate) struct InodeOps;
#[vtable]
impl inode::Operations for InodeOps {
    fn lookup(
        dir: &fs::INode,
        dentry: &mut fs::DEntry,
        _flags: u32,
    ) -> Result<Option<ffi::c_ulong>> {
        // TODO: handle flags
        // TODO: reorganize so that system call logic is separate from
        // conversion from raw pointers

        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };
        let ino_dentry_map = &sbi.ino_dentry_map;

        pr_info!(
            "looking up name {:?} in inode {:?}\n",
            dentry.d_name(),
            dir.i_ino()
        );

        let dentry_vec = ino_dentry_map.lookup_ino(&dir.i_ino());

        if let Some(_dentry_vec) = dentry_vec {
            pr_info!("there is some stuff in the directory\n");
            // TODO: implement lookup in this case
            Err(ENOTSUPP)
        } else {
            Ok(None)
        }
    }

    fn create(
        mnt_userns: &fs::UserNamespace,
        dir: &fs::INode,
        dentry: &fs::DEntry,
        umode: bindings::umode_t,
        excl: bool,
    ) -> Result<i32> {
        pr_info!("creating {:?} in {:?}\n", dentry.d_name(), dir.i_ino());

        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        let (_new_dentry, new_inode) = hayleyfs_create(sbi, mnt_userns, dir, dentry, umode, excl)?;
        // TODO: turn this into a new_vfs_inode function
        // TODO: add some functions/methods to the kernel crate so we don't have
        // to call them directly here

        new_vfs_inode(sb, mnt_userns, dir, dentry, new_inode, umode)?;

        Ok(0)
    }

    fn mkdir(
        _mnt_userns: &fs::UserNamespace,
        dir: &fs::INode,
        dentry: &fs::DEntry,
        _umode: bindings::umode_t,
    ) -> Result<i32> {
        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        // TODO: call hayleyfs_mkdir

        let (_new_dentry, _parent_inode, _new_inode) = hayleyfs_mkdir(sbi, dir, dentry)?;

        // TODO: call new_vfs_inode

        Ok(0)
    }
}

// TODO: add type
fn new_vfs_inode<'a>(
    sb: *mut bindings::super_block,
    mnt_userns: &fs::UserNamespace,
    dir: &fs::INode,
    dentry: &fs::DEntry,
    new_inode: InodeWrapper<'a, Clean, Complete>,
    umode: bindings::umode_t,
) -> Result<()> {
    // set up VFS structures
    let vfs_inode = unsafe { &mut *(bindings::new_inode(sb) as *mut bindings::inode) };

    // TODO: could this be moved out to the callback?
    unsafe {
        bindings::inode_init_owner(mnt_userns.get_inner(), vfs_inode, dir.get_inner(), umode);
    }

    vfs_inode.i_mode = umode;
    vfs_inode.i_ino = new_inode.get_ino();

    unsafe {
        vfs_inode.i_op = inode::OperationsVtable::<InodeOps>::build();
        vfs_inode.__bindgen_anon_3.i_fop = &bindings::simple_dir_operations;
    }

    let current_time = unsafe { bindings::current_time(vfs_inode) };
    vfs_inode.i_mtime = current_time;
    vfs_inode.i_ctime = current_time;
    vfs_inode.i_atime = current_time;
    vfs_inode.i_size = 0;

    unsafe {
        let ret = bindings::insert_inode_locked(vfs_inode);
        if ret < 0 {
            // TODO: from_kernel_errno should really only be pub(crate)
            // probably because you aren't supposed to directly call C fxns from modules
            // but there's no good code to call this stuff from the kernel yet
            // once there is, return from_kernel_errno to pub(crate)
            return Err(error::Error::from_kernel_errno(ret));
        }
        bindings::d_instantiate(dentry.get_inner(), vfs_inode);
        bindings::unlock_new_inode(vfs_inode);
    }

    Ok(())
}

fn hayleyfs_create<'a>(
    sbi: &'a mut SbInfo,
    _mnt_userns: &fs::UserNamespace,
    dir: &fs::INode,
    dentry: &fs::DEntry,
    _umode: bindings::umode_t,
    _excl: bool,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete>,
)> {
    // // get dir pages associated with the parent (if any)
    // let parent_ino = dir.i_ino();
    // let result = sbi.ino_dir_page_map.lookup_ino(&parent_ino);
    // if let Some(_pages) = result {
    //     unimplemented!();
    // } else {
    //     pr_info!("no pages associated with the parent\n");
    //     // allocate a page
    //     let dir_page = DirPageWrapper::alloc_dir_page(sbi)?.flush().fence();
    //     // let parent_inode = InodeWrapper::get_init_inode_by_ino(sbi, parent_ino);
    //     let parent_inode = sbi.get_init_inode_by_ino(parent_ino);
    //     if let Ok(parent_inode) = parent_inode {
    //         let dir_page = dir_page
    //             .set_dir_page_backpointer(parent_inode)
    //             .flush()
    //             .fence();
    //         // TODO: get_free_dentry() should never return an error since all dentries
    //         // in the newly-allocated page should be free - but check on that and confirm
    //         let pd = dir_page.get_free_dentry()?;
    //         create_new_file(sbi, pd, dentry.d_name())
    //     } else {
    //         pr_info!("ERROR: parent inode is not initialized");
    //         return Err(EPERM);
    //     }
    // }

    // TODO: should perhaps take inode wrapper to the parent so that we know
    // the parent is initialized
    let pd = get_free_dentry(sbi, dir.i_ino())?;
    create_new_file(sbi, pd, dentry.d_name())
}

fn hayleyfs_mkdir<'a>(
    sbi: &'a mut SbInfo,
    dir: &fs::INode,
    _dentry: &fs::DEntry,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete>, // parent
    InodeWrapper<'a, Clean, Complete>, // new inode
)> {
    let _pd = get_free_dentry(sbi, dir.i_ino())?;

    Err(EINVAL)
}

fn get_free_dentry<'a>(
    sbi: &mut SbInfo,
    parent_ino: InodeNum,
) -> Result<DentryWrapper<'a, Clean, Free>> {
    let result = sbi.ino_dir_page_map.lookup_ino(&parent_ino);
    if let Some(_pages) = result {
        // TODO: implement
        unimplemented!();
    } else {
        pr_info!("no pages associated with the parent\n");
        // allocate a page
        let dir_page = DirPageWrapper::alloc_dir_page(sbi)?.flush().fence();
        // let parent_inode = InodeWrapper::get_init_inode_by_ino(sbi, parent_ino);
        let parent_inode = sbi.get_init_inode_by_ino(parent_ino);
        if let Ok(parent_inode) = parent_inode {
            let dir_page = dir_page
                .set_dir_page_backpointer(parent_inode)
                .flush()
                .fence();
            // TODO: get_free_dentry() should never return an error since all dentries
            // in the newly-allocated page should be free - but check on that and confirm
            let pd = dir_page.get_free_dentry()?;
            Ok(pd)
        } else {
            pr_info!("ERROR: parent inode is not initialized");
            return Err(EPERM);
        }
    }
}

fn create_new_file<'a>(
    sbi: &'a mut SbInfo,
    dentry: DentryWrapper<'a, Clean, Free>,
    name: &CStr,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete>,
)> {
    // allocate the dentry
    let dentry = dentry.set_name(name)?.flush().fence();

    // set up the new inode
    let new_ino = sbi.inode_allocator.alloc_ino()?;
    let inode = InodeWrapper::get_free_inode_by_ino(sbi, new_ino)?;
    let inode = inode.allocate_file_inode().flush().fence();
    let (dentry, inode) = dentry.set_file_ino(inode);
    let dentry = dentry.flush().fence();

    Ok((dentry, inode))
}
