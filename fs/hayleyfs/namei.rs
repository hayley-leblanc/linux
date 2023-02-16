use crate::balloc::*;
use crate::defs::*;
use crate::dir::*;
use crate::h_file::*;
use crate::h_inode::*;
use crate::typestate::*;
use crate::volatile::*;
use core::ffi;
use kernel::prelude::*;
use kernel::{bindings, error, file, fs, inode};

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

        pr_info!(
            "looking up name {:?} in inode {:?}\n",
            dentry.d_name(),
            dir.i_ino()
        );

        let result = sbi
            .ino_dentry_map
            .lookup_dentry(&dir.i_ino(), dentry.d_name());
        if let Some(dentry_info) = result {
            // the dentry exists in the specified directory
            Ok(Some(dentry_info.get_ino()))
        } else {
            // the dentry does not exist in this directory
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

    fn link(old_dentry: &fs::DEntry, dir: &mut fs::INode, dentry: &fs::DEntry) -> Result<i32> {
        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        hayleyfs_link(sbi, old_dentry, dir, dentry)?;

        // TODO: need to increment VFS inode's link count, update its ctime
        // and d_instantiate the inode with the dentry

        dir.update_ctime();
        dir.inc_nlink();
        // TODO: safe wrapper for d_instantiate
        unsafe {
            bindings::d_instantiate(dentry.get_inner(), old_dentry.d_inode());
        }

        Ok(0)
    }

    fn mkdir(
        mnt_userns: &fs::UserNamespace,
        dir: &mut fs::INode,
        dentry: &fs::DEntry,
        umode: bindings::umode_t,
    ) -> Result<i32> {
        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        let (_new_dentry, _parent_inode, new_inode) = hayleyfs_mkdir(sbi, dir, dentry)?;

        dir.inc_nlink();

        new_vfs_inode(sb, mnt_userns, dir, dentry, new_inode, umode)?;

        Ok(0)
    }

    fn rename(
        _mnt_userns: &fs::UserNamespace,
        _old_dir: &fs::INode,
        _old_dentry: &fs::DEntry,
        _new_dir: &fs::INode,
        _new_dentry: &fs::DEntry,
        _flags: u32,
    ) -> Result<()> {
        unimplemented!();

        // TODO: decrement the inode's link count and delete it if link count == 0
    }
}

// TODO: add type
fn new_vfs_inode<'a, Type>(
    sb: *mut bindings::super_block,
    mnt_userns: &fs::UserNamespace,
    dir: &fs::INode,
    dentry: &fs::DEntry,
    new_inode: InodeWrapper<'a, Clean, Complete, Type>,
    umode: bindings::umode_t,
) -> Result<()> {
    // set up VFS structures
    let vfs_inode = unsafe { &mut *(bindings::new_inode(sb) as *mut bindings::inode) };

    // TODO: could this be moved out to the callback?
    unsafe {
        bindings::inode_init_owner(mnt_userns.get_inner(), vfs_inode, dir.get_inner(), umode);
    }

    vfs_inode.i_ino = new_inode.get_ino();

    // we don't have access to ZST Type, but inode wrapper constructors check types
    // so we can rely on these being correct
    // TODO: what should i_fop be set to?
    let inode_type = new_inode.get_type();
    match inode_type {
        InodeType::REG => {
            vfs_inode.i_mode = umode;
            unsafe {
                vfs_inode.i_op = inode::OperationsVtable::<InodeOps>::build();
                // vfs_inode.__bindgen_anon_3.i_fop = &bindings::simple_dir_operations;
                vfs_inode.__bindgen_anon_3.i_fop =
                    file::OperationsVtable::<Adapter, FileOps>::build();
                bindings::set_nlink(vfs_inode, 1);
            }
        }
        InodeType::DIR => {
            vfs_inode.i_mode = umode | bindings::S_IFDIR as u16;
            unsafe {
                vfs_inode.i_op = inode::OperationsVtable::<InodeOps>::build();
                vfs_inode.__bindgen_anon_3.i_fop = &bindings::simple_dir_operations;
                bindings::set_nlink(vfs_inode, 2);
            }
        }
        InodeType::NONE => panic!("Inode type is none"),
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
    InodeWrapper<'a, Clean, Complete, RegInode>,
)> {
    pr_info!("hayleyfs_create\n");
    // TODO: should perhaps take inode wrapper to the parent so that we know
    // the parent is initialized
    let pd = get_free_dentry(sbi, dir.i_ino())?;
    let pd = pd.set_name(dentry.d_name())?.flush().fence();
    let (dentry, inode) = init_dentry_with_new_reg_inode(sbi, pd)?;

    dentry.index(dir.i_ino(), sbi)?;

    Ok((dentry, inode))
}

fn hayleyfs_link<'a>(
    sbi: &'a mut SbInfo,
    old_dentry: &fs::DEntry,
    dir: &fs::INode,
    dentry: &fs::DEntry,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete, RegInode>,
)> {
    let pd = get_free_dentry(sbi, dir.i_ino())?;
    let _pd = pd.set_name(dentry.d_name())?.flush().fence();

    // old dentry is the dentry for the target name,
    // dir is the PARENT inode,
    // dentry is the dentry for the new name

    pr_info!(
        "old dentry: {:?}, inode number {:?}, new dentry: {:?}\n",
        old_dentry.d_name(),
        dir.i_ino(),
        dentry.d_name()
    );

    // first, obtain the inode that's getting the link from old_dentry
    let target_ino = old_dentry.d_ino();

    let target_inode = sbi.get_init_reg_inode_by_ino(target_ino)?;
    let target_inode = target_inode.inc_link_count()?.flush().fence();
    let pd = get_free_dentry(sbi, dir.i_ino())?;
    let pd = pd.set_name(dentry.d_name())?.flush().fence();

    let (pd, target_inode) = pd.set_file_ino(target_inode);
    let pd = pd.flush().fence();

    pd.index(dir.i_ino(), sbi)?;

    Ok((pd, target_inode))
}

fn hayleyfs_mkdir<'a>(
    sbi: &'a mut SbInfo,
    dir: &fs::INode,
    dentry: &fs::DEntry,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete, DirInode>, // parent
    InodeWrapper<'a, Clean, Complete, DirInode>, // new inode
)> {
    pr_info!("hayleyfs mkdir\n");
    let parent_ino = dir.i_ino();
    let parent_inode = sbi.get_init_dir_inode_by_ino(parent_ino)?;
    let parent_inode = parent_inode.inc_link_count()?.flush().fence();

    let pd = get_free_dentry(sbi, parent_ino)?;
    let pd = pd.set_name(dentry.d_name())?.flush().fence();

    let (dentry, parent, inode) = init_dentry_with_new_dir_inode(sbi, pd, parent_inode)?;
    dentry.index(dir.i_ino(), sbi)?;

    Ok((dentry, parent, inode))
}

fn get_free_dentry<'a>(
    sbi: &'a SbInfo,
    parent_ino: InodeNum,
) -> Result<DentryWrapper<'a, Clean, Free>> {
    let result = sbi.ino_dir_page_map.find_page_with_free_dentry(&parent_ino);
    if let Some(page_info) = result {
        let dir_page = DirPageWrapper::from_dir_page_info(sbi, &page_info)?;
        dir_page.get_free_dentry(sbi)
    } else {
        // no pages have any free dentries
        alloc_page_for_dentry(sbi, parent_ino)
    }
}

fn alloc_page_for_dentry<'a>(
    sbi: &'a SbInfo,
    parent_ino: InodeNum,
) -> Result<DentryWrapper<'a, Clean, Free>> {
    pr_info!("allocating new dir page for inode {:?}\n", parent_ino);
    // allocate a page
    let dir_page = DirPageWrapper::alloc_dir_page(sbi)?.flush().fence();
    sbi.inc_blocks_in_use();
    let parent_inode = sbi.get_init_dir_inode_by_ino(parent_ino);
    if let Ok(parent_inode) = parent_inode {
        let dir_page = dir_page
            .set_dir_page_backpointer(parent_inode)
            .flush()
            .fence();
        sbi.ino_dir_page_map.insert(parent_ino, &dir_page, false)?;
        // TODO: get_free_dentry() should never return an error since all dentries
        // in the newly-allocated page should be free - but check on that and confirm
        let pd = dir_page.get_free_dentry(sbi)?;
        Ok(pd)
    } else {
        pr_info!("ERROR: parent inode is not initialized");
        return Err(EPERM);
    }
}

fn init_dentry_with_new_reg_inode<'a>(
    sbi: &'a SbInfo,
    dentry: DentryWrapper<'a, Clean, Alloc>,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete, RegInode>,
)> {
    // set up the new inode
    let new_ino = sbi.inode_allocator.alloc_ino()?;
    let inode = InodeWrapper::get_free_reg_inode_by_ino(sbi, new_ino)?;
    let inode = inode.allocate_file_inode().flush().fence();
    sbi.inc_inodes_in_use();

    // set the ino in the dentry
    let (dentry, inode) = dentry.set_file_ino(inode);
    let dentry = dentry.flush().fence();

    Ok((dentry, inode))
}

fn init_dentry_with_new_dir_inode<'a>(
    sbi: &'a SbInfo,
    dentry: DentryWrapper<'a, Clean, Alloc>,
    parent_inode: InodeWrapper<'a, Clean, IncLink, DirInode>,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete, DirInode>,
    InodeWrapper<'a, Clean, Complete, DirInode>,
)> {
    // set up the new inode
    let new_ino = sbi.inode_allocator.alloc_ino()?;
    let new_inode = InodeWrapper::get_free_dir_inode_by_ino(sbi, new_ino)?;
    let new_inode = new_inode.allocate_dir_inode().flush().fence();

    // set the ino in the dentry
    let (dentry, new_inode, parent_inode) = dentry.set_dir_ino(new_inode, parent_inode);
    let dentry = dentry.flush().fence();
    Ok((dentry, new_inode, parent_inode))
}

// fn init_dentry_hard_link<'a>(
//     sbi: &'a SbInfo,
//     dentry: DentryWrapper<'a, Clean, Alloc>,
//     inode: InodeWrapper<'a, Clean, IncLink, RegInode>,
// ) -> Result<(
//     DentryWrapper<'a, Clean, Complete>,
//     InodeWrapper<'a, Clean, Complete, RegInode>,
// )> {
//     // set the inode in the dentrty
// }
