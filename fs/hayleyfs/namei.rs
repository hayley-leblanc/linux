use crate::balloc::*;
use crate::defs::*;
use crate::h_dir::*;
use crate::h_file::*;
use crate::h_inode::*;
use crate::pm::*;
use crate::typestate::*;
use crate::volatile::*;
use crate::{fence_all, fence_all_vecs, fence_obj, fence_vec};
// use core::ffi;
use kernel::prelude::*;
use kernel::{bindings, dir, error, file, fs, inode};

pub(crate) struct InodeOps;
#[vtable]
impl inode::Operations for InodeOps {
    fn lookup(
        dir: &fs::INode,
        dentry: &mut fs::DEntry,
        _flags: u32,
    ) -> Result<Option<*mut bindings::inode>> {
        // TODO: handle flags
        // TODO: reorganize so that system call logic is separate from
        // conversion from raw pointers

        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        let result = sbi
            .ino_dentry_map
            .lookup_dentry(&dir.i_ino(), dentry.d_name());
        if let Some(dentry_info) = result {
            // the dentry exists in the specified directory
            Ok(Some(hayleyfs_iget(sb, sbi, dentry_info.get_ino())?))
        } else {
            // the dentry does not exist in this directory
            Ok(None)
        }
    }

    fn create(
        mnt_userns: *mut bindings::user_namespace,
        dir: &fs::INode,
        dentry: &fs::DEntry,
        umode: bindings::umode_t,
        excl: bool,
    ) -> Result<i32> {
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

        new_vfs_inode(sb, sbi, mnt_userns, dir, dentry, new_inode, umode)?;
        Ok(0)
    }

    fn link(old_dentry: &fs::DEntry, dir: &mut fs::INode, dentry: &fs::DEntry) -> Result<i32> {
        let inode = old_dentry.d_inode();
        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        let result = hayleyfs_link(sbi, old_dentry, dir, dentry);

        unsafe { bindings::ihold(inode) };

        if result.is_ok() {
            // TODO: safe wrappers
            unsafe {
                let ctime = bindings::current_time(inode);
                (*inode).i_ctime = ctime;
                bindings::inc_nlink(inode);
                bindings::d_instantiate(dentry.get_inner(), old_dentry.d_inode());
            }
        }

        if let Err(e) = result {
            unsafe { bindings::iput(inode) };
            Err(e)
        } else {
            Ok(0)
        }
    }

    fn mkdir(
        mnt_userns: *mut bindings::user_namespace,
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

        let (_new_dentry, _parent_inode, new_inode) =
            hayleyfs_mkdir(sbi, mnt_userns, dir, dentry, umode)?;

        dir.inc_nlink();

        new_vfs_inode(sb, sbi, mnt_userns, dir, dentry, new_inode, umode)?;
        Ok(0)
    }

    fn rename(
        _mnt_userns: *const bindings::user_namespace,
        _old_dir: &fs::INode,
        _old_dentry: &fs::DEntry,
        _new_dir: &fs::INode,
        _new_dentry: &fs::DEntry,
        _flags: u32,
    ) -> Result<()> {
        unimplemented!();
    }

    // TODO: if this unlink results in its dir page being emptied, we should
    // deallocate the dir page (at some point)
    fn unlink(dir: &fs::INode, dentry: &fs::DEntry) -> Result<()> {
        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        let result = hayleyfs_unlink(sbi, dir, dentry);
        if let Err(e) = result {
            return Err(e);
        }

        Ok(())
    }
}

// TODO: shouldn't really be generic but HayleyFs isn't accessible here
pub(crate) fn hayleyfs_iget(
    sb: *mut bindings::super_block,
    sbi: &SbInfo,
    ino: InodeNum,
) -> Result<*mut bindings::inode> {
    // obtain an inode from VFS
    let inode = unsafe { bindings::iget_locked(sb, ino) };
    if inode.is_null() {
        return Err(ENOMEM);
    }
    // if we don't need to set up the inode, just return it
    let i_new: u64 = bindings::I_NEW.into();

    unsafe {
        if (*inode).i_state & i_new == 0 {
            return Ok(inode);
        }
    }

    // set up the new inode
    let pi = sbi.get_inode_by_ino(ino)?;
    unsafe {
        (*inode).i_size = bindings::le64_to_cpu(pi.get_size()).try_into()?;
        bindings::set_nlink(inode, bindings::le16_to_cpu(pi.get_link_count()).into());
        (*inode).i_mode = bindings::le16_to_cpu(pi.get_mode());
        (*inode).i_blocks = bindings::le64_to_cpu(pi.get_blocks());
        let uid = bindings::le32_to_cpu(pi.get_uid());
        let gid = bindings::le32_to_cpu(pi.get_gid());
        // TODO: https://elixir.bootlin.com/linux/latest/source/fs/ext2/inode.c#L1395 ?
        bindings::i_uid_write(inode, uid);
        bindings::i_gid_write(inode, gid);
        (*inode).i_atime.tv_sec = bindings::le32_to_cpu(pi.get_atime()).try_into()?;
        (*inode).i_ctime.tv_sec = bindings::le32_to_cpu(pi.get_ctime()).try_into()?;
        (*inode).i_mtime.tv_sec = bindings::le32_to_cpu(pi.get_mtime()).try_into()?;
        (*inode).i_atime.tv_nsec = 0;
        (*inode).i_ctime.tv_nsec = 0;
        (*inode).i_mtime.tv_nsec = 0;
        (*inode).i_blkbits = bindings::blksize_bits(sbi.blocksize.try_into()?).try_into()?;
        // TODO: set the rest of the fields!
    }

    let inode_type = pi.get_type();
    match inode_type {
        InodeType::REG => unsafe {
            (*inode).i_op = inode::OperationsVtable::<InodeOps>::build();
            (*inode).__bindgen_anon_3.i_fop = file::OperationsVtable::<Adapter, FileOps>::build();
        },
        InodeType::DIR => unsafe {
            (*inode).i_op = inode::OperationsVtable::<InodeOps>::build();
            (*inode).__bindgen_anon_3.i_fop = dir::OperationsVtable::<DirOps>::build();
        },
        InodeType::NONE => panic!("Inode type is NONE"),
    }
    unsafe { bindings::unlock_new_inode(inode) };
    Ok(inode)
}

// TODO: add type
fn new_vfs_inode<'a, Type>(
    sb: *mut bindings::super_block,
    sbi: &SbInfo,
    mnt_userns: *mut bindings::user_namespace,
    dir: &fs::INode,
    dentry: &fs::DEntry,
    new_inode: InodeWrapper<'a, Clean, Complete, Type>,
    umode: bindings::umode_t,
) -> Result<()> {
    // set up VFS structures
    let vfs_inode = unsafe { &mut *(bindings::new_inode(sb) as *mut bindings::inode) };

    // TODO: could this be moved out to the callback?
    unsafe {
        bindings::inode_init_owner(mnt_userns, vfs_inode, dir.get_inner(), umode);
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
                vfs_inode.__bindgen_anon_3.i_fop =
                    file::OperationsVtable::<Adapter, FileOps>::build();
                bindings::set_nlink(vfs_inode, 1);
            }
        }
        InodeType::DIR => {
            vfs_inode.i_mode = umode | bindings::S_IFDIR as u16;
            unsafe {
                vfs_inode.i_op = inode::OperationsVtable::<InodeOps>::build();
                vfs_inode.__bindgen_anon_3.i_fop = dir::OperationsVtable::<DirOps>::build();
                bindings::set_nlink(vfs_inode, 2);
            }
        }
        InodeType::NONE => panic!("Inode type is none"),
    }

    // let current_time = unsafe { bindings::current_time(vfs_inode) };
    vfs_inode.i_mtime.tv_sec = new_inode.get_mtime().try_into()?;
    vfs_inode.i_ctime.tv_sec = new_inode.get_ctime().try_into()?;
    vfs_inode.i_atime.tv_sec = new_inode.get_atime().try_into()?;
    vfs_inode.i_mtime.tv_nsec = 0;
    vfs_inode.i_atime.tv_nsec = 0;
    vfs_inode.i_mtime.tv_nsec = 0;
    vfs_inode.i_size = new_inode.get_size().try_into()?;
    vfs_inode.i_blocks = new_inode.get_blocks();
    vfs_inode.i_blkbits = unsafe { bindings::blksize_bits(sbi.blocksize.try_into()?).try_into()? };

    unsafe {
        let uid = bindings::le32_to_cpu(new_inode.get_uid());
        let gid = bindings::le32_to_cpu(new_inode.get_gid());
        // TODO: https://elixir.bootlin.com/linux/latest/source/fs/ext2/inode.c#L1395 ?
        bindings::i_uid_write(vfs_inode, uid);
        bindings::i_gid_write(vfs_inode, gid);

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
    sbi: &'a SbInfo,
    mnt_userns: *mut bindings::user_namespace,
    dir: &fs::INode,
    dentry: &fs::DEntry,
    umode: bindings::umode_t,
    _excl: bool,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete, RegInode>,
)> {
    // TODO: should perhaps take inode wrapper to the parent so that we know
    // the parent is initialized
    let pd = get_free_dentry(sbi, dir.i_ino())?;
    let pd = pd.set_name(dentry.d_name())?.flush().fence();
    let (dentry, inode) = init_dentry_with_new_reg_inode(sbi, mnt_userns, dir, pd, umode)?;
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
    sbi: &'a SbInfo,
    mnt_userns: *mut bindings::user_namespace,
    dir: &fs::INode,
    dentry: &fs::DEntry,
    mode: u16,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete, DirInode>, // parent
    InodeWrapper<'a, Clean, Complete, DirInode>, // new inode
)> {
    let parent_ino = dir.i_ino();
    let parent_inode = sbi.get_init_dir_inode_by_ino(parent_ino)?;
    let parent_inode = parent_inode.inc_link_count()?.flush().fence();

    let pd = get_free_dentry(sbi, parent_ino)?;
    let pd = pd.set_name(dentry.d_name())?.flush().fence();

    let (dentry, parent, inode) =
        init_dentry_with_new_dir_inode(sbi, mnt_userns, dir, pd, parent_inode, mode)?;
    dentry.index(dir.i_ino(), sbi)?;
    Ok((dentry, parent, inode))
}

#[allow(dead_code)]
fn hayleyfs_unlink<'a>(
    sbi: &'a SbInfo,
    dir: &fs::INode,
    dentry: &fs::DEntry,
) -> Result<(
    InodeWrapper<'a, Clean, Complete, RegInode>,
    DentryWrapper<'a, Clean, Free>,
)> {
    let inode = dentry.d_inode();
    let parent_ino = dir.i_ino();
    let _parent_inode = sbi.get_init_dir_inode_by_ino(parent_ino)?;

    // use volatile index to find the persistent dentry
    let dentry_info = sbi
        .ino_dentry_map
        .lookup_dentry(&parent_ino, dentry.d_name());
    if let Some(dentry_info) = dentry_info {
        // FIXME?: right now we don't enforce that the dentry has to have pointed
        // to the inode - theoretically an unrelated directory entry being
        // deallocated could be used to decrement an inode's link count

        // obtain target inode and then invalidate the directory entry
        let pd = DentryWrapper::get_init_dentry(dentry_info)?;
        let ino = pd.get_ino();
        sbi.ino_dentry_map.delete(parent_ino, dentry_info)?;

        let pi = sbi.get_init_reg_inode_by_ino(ino)?;
        let pd = pd.clear_ino().flush().fence();

        // decrement the inode's link count
        // according to Alloy we can share the fence with dentry deallocation
        let pi = pi.dec_link_count(&pd)?.flush();

        // deallocate the dentry
        let pd = pd.dealloc_dentry().flush();

        let (pi, pd) = fence_all!(pi, pd);

        let result = pi.try_complete_unlink(sbi)?;

        if let Ok(result) = result {
            unsafe {
                bindings::drop_nlink(inode);
            }

            Ok((result, pd))
        } else if let Err((pi, mut pages)) = result {
            // go through each page and deallocate it
            // we can drain the vector since missing a page will result in
            // a runtime panic
            let mut to_dealloc = Vec::new();
            for page in pages.drain(..) {
                sbi.ino_data_page_map.delete(&ino, page.get_offset())?;
                let page = page.unmap().flush();
                to_dealloc.try_push(page)?;
            }
            let mut to_dealloc = fence_all_vecs!(to_dealloc);
            let mut deallocated = Vec::new();
            for page in to_dealloc.drain(..) {
                let page = page.dealloc().flush();
                deallocated.try_push(page)?;
            }
            let deallocated = fence_all_vecs!(deallocated);
            for page in &deallocated {
                sbi.page_allocator.dealloc_data_page(page)?;
            }
            let freed_pages = DataPageWrapper::mark_pages_free(deallocated)?;

            unsafe {
                bindings::drop_nlink(inode);
            }

            // pages are now deallocated and we can use the freed pages vector
            // to deallocate the inode.
            let pi = pi.dealloc(freed_pages).flush().fence();
            Ok((pi, pd))
        } else {
            Err(EINVAL)
        }
    } else {
        Err(ENOENT)
    }
}

fn get_free_dentry<'a>(
    sbi: &'a SbInfo,
    parent_ino: InodeNum,
) -> Result<DentryWrapper<'a, Clean, Free>> {
    let result = sbi
        .ino_dir_page_map
        .find_page_with_free_dentry(sbi, &parent_ino)?;
    let result = if let Some(page_info) = result {
        let dir_page = DirPageWrapper::from_dir_page_info(sbi, &page_info)?;
        dir_page.get_free_dentry(sbi)
    } else {
        // no pages have any free dentries
        alloc_page_for_dentry(sbi, parent_ino)
    };
    result
}

fn alloc_page_for_dentry<'a>(
    sbi: &'a SbInfo,
    parent_ino: InodeNum,
) -> Result<DentryWrapper<'a, Clean, Free>> {
    // allocate a page
    let dir_page = DirPageWrapper::alloc_dir_page(sbi)?.flush().fence();
    sbi.inc_blocks_in_use();
    let parent_inode = sbi.get_init_dir_inode_by_ino(parent_ino);
    if let Ok(parent_inode) = parent_inode {
        let dir_page = dir_page
            .set_dir_page_backpointer(parent_inode)
            .flush()
            .fence();
        sbi.ino_dir_page_map.insert(parent_ino, &dir_page)?;
        let pd = dir_page.get_free_dentry(sbi)?;
        Ok(pd)
    } else {
        pr_info!("ERROR: parent inode is not initialized");
        return Err(EPERM);
    }
}

fn init_dentry_with_new_reg_inode<'a>(
    sbi: &'a SbInfo,
    mnt_userns: *mut bindings::user_namespace,
    dir: &fs::INode,
    dentry: DentryWrapper<'a, Clean, Alloc>,
    mode: u16,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete, RegInode>,
)> {
    // set up the new inode
    let new_ino = sbi.inode_allocator.alloc_ino()?;
    let inode = InodeWrapper::get_free_reg_inode_by_ino(sbi, new_ino)?;
    let inode = inode
        .allocate_file_inode(sbi, mnt_userns, dir, mode)?
        .flush()
        .fence();
    sbi.inc_inodes_in_use();

    // set the ino in the dentry
    let (dentry, inode) = dentry.set_file_ino(inode);
    let dentry = dentry.flush().fence();

    Ok((dentry, inode))
}

fn init_dentry_with_new_dir_inode<'a>(
    sbi: &'a SbInfo,
    mnt_userns: *mut bindings::user_namespace,
    inode: &fs::INode,
    dentry: DentryWrapper<'a, Clean, Alloc>,
    parent_inode: InodeWrapper<'a, Clean, IncLink, DirInode>,
    mode: u16,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete, DirInode>, // parent
    InodeWrapper<'a, Clean, Complete, DirInode>, // new inode
)> {
    // set up the new inode
    let new_ino = sbi.inode_allocator.alloc_ino()?;
    let new_inode = InodeWrapper::get_free_dir_inode_by_ino(sbi, new_ino)?;
    let new_inode = new_inode
        .allocate_dir_inode(sbi, mnt_userns, inode, mode)?
        .flush()
        .fence();

    // set the ino in the dentry
    let (dentry, new_inode, parent_inode) = dentry.set_dir_ino(new_inode, parent_inode);
    let dentry = dentry.flush().fence();
    Ok((dentry, parent_inode, new_inode))
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
