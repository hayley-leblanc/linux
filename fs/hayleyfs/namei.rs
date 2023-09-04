use crate::balloc::*;
use crate::defs::*;
use crate::h_dir::*;
use crate::h_file::*;
use crate::h_inode::*;
use crate::h_symlink::*;
use crate::pm::*;
use crate::typestate::*;
use crate::volatile::*;
use crate::{
    end_timing, fence_all, fence_all_vecs, fence_obj, fence_vec, init_timing, start_timing,
};

use core::sync::atomic::Ordering;
use kernel::prelude::*;
use kernel::{
    bindings, dir, error, file, fs, inode, io_buffer::IoBufferReader, symlink,
    user_ptr::UserSlicePtr, ForeignOwnable,
};

// TODO: should use .borrow() to get the SbInfo structure out?

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

        let (_parent_inode, parent_inode_info) =
            sbi.get_init_dir_inode_by_vfs_inode(dir.get_inner())?;
        move_dir_inode_tree_to_map(sbi, parent_inode_info)?;
        let result = parent_inode_info.lookup_dentry(dentry.d_name());
        // let result = sbi
        //     .ino_dentry_map
        //     .lookup_dentry(&dir.i_ino(), dentry.d_name());
        if let Some(dentry_info) = result {
            // the dentry exists in the specified directory
            Ok(Some(hayleyfs_iget(sb, sbi, dentry_info.get_ino())?))
        } else {
            // the dentry does not exist in this directory
            Ok(None)
        }
    }

    fn create(
        mnt_idmap: *mut bindings::mnt_idmap,
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

        let (_new_dentry, new_inode) = hayleyfs_create(sbi, dir, dentry, umode, excl)?;

        let vfs_inode = new_vfs_inode(sb, sbi, mnt_idmap, dir, dentry, &new_inode, umode)?;
        unsafe { insert_vfs_inode(vfs_inode, dentry)? };
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
        mnt_idmap: *mut bindings::mnt_idmap,
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

        let (_new_dentry, _parent_inode, new_inode) = hayleyfs_mkdir(sbi, dir, dentry, umode)?;

        dir.inc_nlink();

        let vfs_inode = new_vfs_inode(sb, sbi, mnt_idmap, dir, dentry, &new_inode, umode)?;
        unsafe { insert_vfs_inode(vfs_inode, dentry)? };
        Ok(0)
    }

    fn rename(
        _mnt_idmap: *const bindings::mnt_idmap,
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

    fn symlink(
        mnt_idmap: *mut bindings::mnt_idmap,
        dir: &fs::INode,
        dentry: &fs::DEntry,
        symname: *const core::ffi::c_char,
    ) -> Result<()> {
        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        let perm: u16 = 0777;
        let flag: u16 = bindings::S_IFLNK.try_into().unwrap();
        let mode: u16 = flag | perm; // TODO: correct mode

        let result = hayleyfs_symlink(sbi, dir, dentry, symname, mode);
        if let Err(e) = result {
            return Err(e);
        } else if let Ok((mut new_inode, _, new_page)) = result {
            let vfs_inode = new_vfs_inode(sb, sbi, mnt_idmap, dir, dentry, &new_inode, mode)?;
            new_inode.set_vfs_inode(vfs_inode)?;
            let pi_info = new_inode.get_inode_info()?;
            pi_info.insert(&new_page)?;
            unsafe { insert_vfs_inode(vfs_inode, dentry)? };
            Ok(())
        } else {
            unreachable!();
        }
    }
}

// TODO: shouldn't really be generic but HayleyFs isn't accessible here
/// hayleyfs_iget is for obtaining the VFS inode for an inode that already
/// exists persistently. new_vfs_inode is for setting up VFS inodes for
/// completely new inodes
pub(crate) fn hayleyfs_iget(
    sb: *mut bindings::super_block,
    sbi: &SbInfo,
    ino: InodeNum,
) -> Result<*mut bindings::inode> {
    init_timing!(inode_exists);
    start_timing!(inode_exists);
    init_timing!(full_iget);
    start_timing!(full_iget);
    // obtain an inode from VFS
    let inode = unsafe { bindings::iget_locked(sb, ino) };
    if inode.is_null() {
        return Err(ENOMEM);
    }
    // if we don't need to set up the inode, just return it
    let i_new: u64 = bindings::I_NEW.into();

    unsafe {
        if (*inode).i_state & i_new == 0 {
            end_timing!(IgetInodeExists, inode_exists);
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
        (*inode).i_atime = pi.get_atime();
        (*inode).i_ctime = pi.get_ctime();
        (*inode).i_mtime = pi.get_mtime();
        (*inode).i_blkbits = bindings::blksize_bits(sbi.blocksize.try_into()?).try_into()?;
        // TODO: set the rest of the fields!
    }

    let inode_type = pi.get_type();
    match inode_type {
        InodeType::REG => unsafe {
            init_timing!(init_reg_inode);
            start_timing!(init_reg_inode);
            (*inode).i_op = inode::OperationsVtable::<InodeOps>::build();
            (*inode).__bindgen_anon_3.i_fop = file::OperationsVtable::<Adapter, FileOps>::build();

            let pages = sbi.ino_data_page_tree.remove(ino);
            // if the inode has any pages associated with it, remove them from the
            // global tree and put them in this inode's i_private
            if let Some(pages) = pages {
                let inode_info = Box::try_new(HayleyFsRegInodeInfo::new_from_tree(ino, pages))?;
                (*inode).i_private = inode_info.into_foreign() as *mut _;
            } else {
                let inode_info = Box::try_new(HayleyFsRegInodeInfo::new(ino))?;
                (*inode).i_private = inode_info.into_foreign() as *mut _;
            }
            end_timing!(InitRegInode, init_reg_inode);
        },
        InodeType::DIR => unsafe {
            init_timing!(init_dir_inode);
            start_timing!(init_dir_inode);
            (*inode).i_op = inode::OperationsVtable::<InodeOps>::build();
            (*inode).__bindgen_anon_3.i_fop = dir::OperationsVtable::<DirOps>::build();

            let pages = sbi.ino_dir_page_tree.remove(ino);
            // if the inode has any pages associated with it, remove them from the
            // global tree and put them in this inode's i_private
            if let Some(pages) = pages {
                let dentries = sbi.ino_dentry_tree.remove(ino);
                let inode_info = if let Some(dentries) = dentries {
                    Box::try_new(HayleyFsDirInodeInfo::new_from_vec(ino, pages, dentries))?
                } else {
                    Box::try_new(HayleyFsDirInodeInfo::new_from_vec(ino, pages, Vec::new()))?
                };
                (*inode).i_private = inode_info.into_foreign() as *mut _;
            } else {
                let inode_info = Box::try_new(HayleyFsDirInodeInfo::new(ino))?;
                (*inode).i_private = inode_info.into_foreign() as *mut _;
            }
            end_timing!(InitDirInode, init_dir_inode);
        },
        InodeType::SYMLINK => unsafe {
            // unimplemented!();
            (*inode).i_op = symlink::OperationsVtable::<SymlinkOps>::build();
            let pages = sbi.ino_data_page_tree.remove(ino);
            // if the inode has any pages associated with it, remove them from the
            // global tree and put them in this inode's i_private
            if let Some(pages) = pages {
                let inode_info = Box::try_new(HayleyFsRegInodeInfo::new_from_tree(ino, pages))?;
                (*inode).i_private = inode_info.into_foreign() as *mut _;
            } else {
                let inode_info = Box::try_new(HayleyFsRegInodeInfo::new(ino))?;
                (*inode).i_private = inode_info.into_foreign() as *mut _;
            }
        },
        InodeType::NONE => panic!("Inode type is NONE"),
    }
    unsafe { bindings::unlock_new_inode(inode) };
    end_timing!(FullIget, full_iget);
    Ok(inode)
}

// TODO: add type
/// new_vfs_inode is used to set up the VFS inode for a completely new HayleyFsInode.
/// if the HayleyFsInode already exists, you should use hayleyfs_iget
fn new_vfs_inode<'a, Type>(
    sb: *mut bindings::super_block,
    sbi: &SbInfo,
    mnt_idmap: *mut bindings::mnt_idmap,
    dir: &fs::INode,
    _dentry: &fs::DEntry,
    new_inode: &InodeWrapper<'a, Clean, Complete, Type>,
    umode: bindings::umode_t,
) -> Result<*mut bindings::inode> {
    init_timing!(full_vfs_inode);
    start_timing!(full_vfs_inode);
    // set up VFS structures
    let vfs_inode = unsafe { &mut *(bindings::new_inode(sb) as *mut bindings::inode) };

    // TODO: could this be moved out to the callback?
    unsafe {
        bindings::inode_init_owner(mnt_idmap, vfs_inode, dir.get_inner(), umode);
    }

    let ino = new_inode.get_ino();
    vfs_inode.i_ino = ino;

    // we don't have access to ZST Type, but inode wrapper constructors check types
    // so we can rely on these being correct
    let inode_type = new_inode.get_type();
    match inode_type {
        InodeType::REG => {
            init_timing!(init_reg_vfs_inode);
            start_timing!(init_reg_vfs_inode);
            vfs_inode.i_mode = umode;
            // initialize the DRAM info and save it in the private pointer
            let inode_info = Box::try_new(HayleyFsRegInodeInfo::new(ino))?;
            vfs_inode.i_private = inode_info.into_foreign() as *mut _;
            unsafe {
                vfs_inode.i_op = inode::OperationsVtable::<InodeOps>::build();
                vfs_inode.__bindgen_anon_3.i_fop =
                    file::OperationsVtable::<Adapter, FileOps>::build();
                bindings::set_nlink(vfs_inode, 1);
            }
            end_timing!(InitRegVfsInode, init_reg_vfs_inode);
        }
        InodeType::DIR => {
            init_timing!(init_dir_vfs_inode);
            start_timing!(init_dir_vfs_inode);
            vfs_inode.i_mode = umode | bindings::S_IFDIR as u16;
            // initialize the DRAM info and save it in the private pointer
            let inode_info = Box::try_new(HayleyFsDirInodeInfo::new(ino))?;
            vfs_inode.i_private = inode_info.into_foreign() as *mut _;
            unsafe {
                vfs_inode.i_op = inode::OperationsVtable::<InodeOps>::build();
                vfs_inode.__bindgen_anon_3.i_fop = dir::OperationsVtable::<DirOps>::build();
                bindings::set_nlink(vfs_inode, 2);
            }
            end_timing!(InitDirVfsInode, init_dir_vfs_inode);
        }
        InodeType::SYMLINK => {
            vfs_inode.i_mode = umode;
            // initialize the DRAM info and save it in the private pointer
            let inode_info = Box::try_new(HayleyFsRegInodeInfo::new(ino))?;
            vfs_inode.i_private = inode_info.into_foreign() as *mut _;
            unsafe {
                vfs_inode.i_op = symlink::OperationsVtable::<SymlinkOps>::build();
            }
        }
        InodeType::NONE => panic!("Inode type is none"),
    }

    vfs_inode.i_mtime = new_inode.get_mtime();
    vfs_inode.i_ctime = new_inode.get_ctime();
    vfs_inode.i_atime = new_inode.get_atime();
    vfs_inode.i_size = new_inode.get_size().try_into()?;
    vfs_inode.i_blocks = new_inode.get_blocks();
    vfs_inode.i_blkbits = unsafe { bindings::blksize_bits(sbi.blocksize.try_into()?).try_into()? };

    unsafe {
        let uid = bindings::le32_to_cpu(new_inode.get_uid());
        let gid = bindings::le32_to_cpu(new_inode.get_gid());
        // TODO: https://elixir.bootlin.com/linux/latest/source/fs/ext2/inode.c#L1395 ?
        bindings::i_uid_write(vfs_inode, uid);
        bindings::i_gid_write(vfs_inode, gid);

        // let ret = bindings::insert_inode_locked(vfs_inode);
        // if ret < 0 {
        //     // TODO: from_kernel_errno should really only be pub(crate)
        //     // probably because you aren't supposed to directly call C fxns from modules
        //     // but there's no good code to call this stuff from the kernel yet
        //     // once there is, return from_kernel_errno to pub(crate)
        //     return Err(error::Error::from_kernel_errno(ret));
        // }
        // bindings::d_instantiate(dentry.get_inner(), vfs_inode);
        // bindings::unlock_new_inode(vfs_inode);
    }
    end_timing!(FullVfsInode, full_vfs_inode);
    Ok(vfs_inode)
}

unsafe fn insert_vfs_inode(vfs_inode: *mut bindings::inode, dentry: &fs::DEntry) -> Result<()> {
    // TODO: check that the inode is fully set up and doesn't already exist
    // until then this is unsafe
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
    sbi: &'a SbInfo,
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
    let (_parent_inode, parent_inode_info) =
        sbi.get_init_dir_inode_by_vfs_inode(dir.get_inner())?;
    let pd = get_free_dentry(sbi, dir, parent_inode_info)?;
    let pd = pd.set_name(dentry.d_name())?.flush().fence();
    let (dentry, inode) = init_dentry_with_new_reg_inode(sbi, dir, pd, umode)?;
    dentry.index(parent_inode_info)?;

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
    // TODO: why do we do this twice...??
    let (_parent_inode, parent_inode_info) =
        sbi.get_init_dir_inode_by_vfs_inode(dir.get_inner())?;
    let pd = get_free_dentry(sbi, dir, parent_inode_info)?;
    let _pd = pd.set_name(dentry.d_name())?.flush().fence();

    // old dentry is the dentry for the target name,
    // dir is the PARENT inode,
    // dentry is the dentry for the new name

    // first, obtain the inode that's getting the link from old_dentry
    let (target_inode, _) = sbi.get_init_reg_inode_by_vfs_inode(old_dentry.d_inode())?;
    let target_inode = target_inode.inc_link_count()?.flush().fence();
    let (_parent_inode, parent_inode_info) =
        sbi.get_init_dir_inode_by_vfs_inode(dir.get_inner())?;
    let pd = get_free_dentry(sbi, dir, parent_inode_info)?;
    let pd = pd.set_name(dentry.d_name())?.flush().fence();

    let (pd, target_inode) = pd.set_file_ino(target_inode);
    let pd = pd.flush().fence();

    pd.index(parent_inode_info)?;

    Ok((pd, target_inode))
}

fn hayleyfs_mkdir<'a>(
    sbi: &'a SbInfo,
    dir: &fs::INode,
    dentry: &fs::DEntry,
    mode: u16,
) -> Result<(
    DentryWrapper<'a, Clean, Complete>,
    InodeWrapper<'a, Clean, Complete, DirInode>, // parent
    InodeWrapper<'a, Clean, Complete, DirInode>, // new inode
)> {
    let (parent_inode, parent_inode_info) = sbi.get_init_dir_inode_by_vfs_inode(dir.get_inner())?;
    let parent_inode = parent_inode.inc_link_count()?.flush().fence();
    let pd = get_free_dentry(sbi, dir, parent_inode_info)?;
    let pd = pd.set_name(dentry.d_name())?.flush().fence();
    let (dentry, parent, inode) = init_dentry_with_new_dir_inode(sbi, dir, pd, parent_inode, mode)?;
    dentry.index(parent_inode_info)?;
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
    init_timing!(unlink_full_declink);
    start_timing!(unlink_full_declink);
    init_timing!(unlink_full_delete);
    start_timing!(unlink_full_delete);
    let inode = dentry.d_inode();
    init_timing!(unlink_lookup);
    start_timing!(unlink_lookup);
    let (_parent_inode, parent_inode_info) =
        sbi.get_init_dir_inode_by_vfs_inode(dir.get_inner())?;

    // use volatile index to find the persistent dentry
    let dentry_info = parent_inode_info.lookup_dentry(dentry.d_name());
    end_timing!(UnlinkLookup, unlink_lookup);
    if let Some(dentry_info) = dentry_info {
        // FIXME?: right now we don't enforce that the dentry has to have pointed
        // to the inode - theoretically an unrelated directory entry being
        // deallocated could be used to decrement an inode's link count

        init_timing!(dec_link_count);
        start_timing!(dec_link_count);

        // obtain target inode and then invalidate the directory entry
        let pd = DentryWrapper::get_init_dentry(dentry_info)?;
        parent_inode_info.delete_dentry(dentry_info)?;

        let (pi, _) = sbi.get_init_reg_inode_by_vfs_inode(inode)?;
        let pd = pd.clear_ino().flush().fence();

        // decrement the inode's link count
        // according to Alloy we can share the fence with dentry deallocation
        let pi = pi.dec_link_count(&pd)?.flush();

        // deallocate the dentry
        let pd = pd.dealloc_dentry().flush();

        let (pi, pd) = fence_all!(pi, pd);

        let result = pi.try_complete_unlink(sbi)?;
        end_timing!(DecLinkCount, dec_link_count);

        if let Ok(result) = result {
            unsafe {
                bindings::drop_nlink(inode);
            }
            end_timing!(UnlinkFullDecLink, unlink_full_declink);
            Ok((result, pd))
        } else if let Err((pi, mut pages)) = result {
            // go through each page and deallocate it
            // we can drain the vector since missing a page will result in
            // a runtime panic
            init_timing!(dealloc_pages);
            start_timing!(dealloc_pages);
            let mut to_dealloc = Vec::new();

            for page in pages.drain(..) {
                // the pages have already been removed from the inode's page vector
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
            end_timing!(DeallocPages, dealloc_pages);
            end_timing!(UnlinkFullDelete, unlink_full_delete);
            Ok((pi, pd))
        } else {
            Err(EINVAL)
        }
    } else {
        Err(ENOENT)
    }
}

fn hayleyfs_symlink<'a>(
    sbi: &'a SbInfo,
    dir: &fs::INode,
    dentry: &fs::DEntry,
    symname: *const core::ffi::c_char,
    mode: u16,
) -> Result<(
    InodeWrapper<'a, Clean, Complete, RegInode>,
    DentryWrapper<'a, Clean, Complete>,
    DataPageWrapper<'a, Clean, Written>,
)> {
    // obtain and allocate a new persistent dentry
    let (_parent_inode, parent_inode_info) =
        sbi.get_init_dir_inode_by_vfs_inode(dir.get_inner())?;
    let pd = get_free_dentry(sbi, dir, parent_inode_info)?;
    let name = unsafe { CStr::from_char_ptr(symname) };
    let pd = pd.set_name(dentry.d_name())?.flush().fence();

    // obtain and allocate an inode for the symlink
    let pi = sbi.inode_allocator.alloc_ino()?;
    let pi = InodeWrapper::get_free_reg_inode_by_ino(sbi, pi)?;

    let pi = pi.allocate_symlink_inode(dir, mode)?.flush().fence();
    sbi.inc_inodes_in_use();

    // allocate a page for the symlink
    let page = DataPageWrapper::alloc_data_page(sbi, 0)?.flush().fence();
    sbi.inc_blocks_in_use();

    // set data page backpointer - at this point, inode and page are still orphaned
    let page = page.set_data_page_backpointer(&pi).flush().fence();
    // let pi_info = pi.get_inode_info()?;
    // pi_info.insert(&page)?;

    // need to set file size also which will require writing to the page I think

    // Safety: symname has to temporarily be cast to a mutable raw pointer in order to create
    // the reader. This is safe because 1) a UserSlicePtrReader does not provide any methods
    // that mutate the buffer, 2) we immediately convert the UserSlicePtr into a UserSlicePtrReader,
    // and 3) the UserSlicePtr constructor does not mutate the buffer.
    let mut name_reader =
        unsafe { UserSlicePtr::new(symname as *mut core::ffi::c_void, name.len()).reader() };
    let name_len: u64 = name_reader.len().try_into()?;
    let (bytes_written, page) = page.write_to_page(sbi, &mut name_reader, 0, name_len)?;
    let page = page.fence();

    // set the file size. we'll create the VFS inode based on the persistent inode after
    // this method returns
    let (_size, pi) = pi.set_size(bytes_written, 0, &page);

    let (pd, pi) = pd.set_file_ino(pi);
    let pd = pd.flush().fence();
    pd.index(parent_inode_info)?;

    Ok((pi, pd, page))
}

fn get_free_dentry<'a>(
    sbi: &'a SbInfo,
    parent_inode: &fs::INode,
    parent_inode_info: &HayleyFsDirInodeInfo,
) -> Result<DentryWrapper<'a, Clean, Free>> {
    let result = parent_inode_info.find_page_with_free_dentry(sbi)?;
    let result = if let Some(page_info) = result {
        let dir_page = DirPageWrapper::from_dir_page_info(sbi, &page_info)?;
        dir_page.get_free_dentry(sbi)
    } else {
        // no pages have any free dentries
        alloc_page_for_dentry(sbi, parent_inode)
    };
    result
}

fn alloc_page_for_dentry<'a>(
    sbi: &'a SbInfo,
    parent_inode: &fs::INode,
) -> Result<DentryWrapper<'a, Clean, Free>> {
    // allocate a page
    let dir_page = DirPageWrapper::alloc_dir_page(sbi)?.flush().fence();
    sbi.inc_blocks_in_use();
    let result = sbi.get_init_dir_inode_by_vfs_inode(parent_inode.get_inner());
    if let Ok((parent_inode, parent_inode_info)) = result {
        let dir_page = dir_page
            .set_dir_page_backpointer(parent_inode)
            .flush()
            .fence();
        parent_inode_info.insert(&dir_page)?;
        let pd = dir_page.get_free_dentry(sbi)?;
        Ok(pd)
    } else {
        pr_info!("ERROR: parent inode is not initialized");
        return Err(EPERM);
    }
}

fn init_dentry_with_new_reg_inode<'a>(
    sbi: &'a SbInfo,
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
    let inode = inode.allocate_file_inode(dir, mode)?.flush().fence();
    sbi.inc_inodes_in_use();

    // set the ino in the dentry
    let (dentry, inode) = dentry.set_file_ino(inode);
    let dentry = dentry.flush().fence();

    Ok((dentry, inode))
}

fn init_dentry_with_new_dir_inode<'a>(
    sbi: &'a SbInfo,
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
    let new_inode = new_inode.allocate_dir_inode(inode, mode)?.flush().fence();

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
