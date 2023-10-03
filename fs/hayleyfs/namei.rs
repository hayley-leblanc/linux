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
    bindings, dir, error, file, fs, inode, io_buffer::IoBufferReader, rbtree::RBTree, symlink,
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

    fn rmdir(dir: &mut fs::INode, dentry: &fs::DEntry) -> Result<i32> {
        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        // TODO: is there a nice Result function you could use to remove the match?
        let result = hayleyfs_rmdir(sbi, dir, dentry);
        match result {
            Ok(_) => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn rename(
        _mnt_idmap: *const bindings::mnt_idmap,
        old_dir: &fs::INode,
        old_dentry: &fs::DEntry,
        new_dir: &fs::INode,
        new_dentry: &fs::DEntry,
        flags: u32,
    ) -> Result<()> {
        let sb = old_dir.i_sb();
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        let result = hayleyfs_rename(sbi, old_dir, old_dentry, new_dir, new_dentry, flags);

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
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

    fn setattr(
        mnt_idmap: *mut bindings::mnt_idmap,
        dentry: &fs::DEntry,
        iattr: *mut bindings::iattr,
    ) -> Result<()> {
        let inode = dentry.d_inode();

        unsafe {
            let ret = bindings::setattr_prepare(mnt_idmap, dentry.get_inner(), iattr);
            if ret < 0 {
                return Err(error::Error::from_kernel_errno(ret));
            }
            bindings::setattr_copy(mnt_idmap, inode, iattr);
        }
        Ok(())
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
                    Box::try_new(HayleyFsDirInodeInfo::new_from_tree(ino, pages, dentries))?
                } else {
                    Box::try_new(HayleyFsDirInodeInfo::new_from_tree(
                        ino,
                        pages,
                        RBTree::new(),
                    ))?
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
        InodeType::NONE => {
            pr_info!("Inode {:?} has type NONE\n", ino);
            panic!("Inode type is NONE")
        }
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

fn hayleyfs_rmdir<'a>(
    sbi: &'a SbInfo,
    dir: &mut fs::INode,
    dentry: &fs::DEntry,
) -> Result<(
    InodeWrapper<'a, Clean, Complete, DirInode>, // target
    InodeWrapper<'a, Clean, DecLink, DirInode>,  // parent
    DentryWrapper<'a, Clean, Free>,
)> {
    let inode = dentry.d_inode();
    let (_parent_inode, parent_inode_info) =
        sbi.get_init_dir_inode_by_vfs_inode(dir.get_inner())?;
    let dentry_info = parent_inode_info.lookup_dentry(dentry.d_name());
    match dentry_info {
        Some(dentry_info) => {
            // check if the directory we are trying to delete is empty
            let (pi, delete_dir_info) = sbi.get_init_dir_inode_by_vfs_inode(inode)?;
            if delete_dir_info.has_dentries() {
                return Err(ENOTEMPTY);
            }
            // if it is, start deleting
            let pd = DentryWrapper::get_init_dentry(dentry_info)?;

            // clear dentry inode
            parent_inode_info.delete_dentry(dentry_info)?;
            let pd = pd.clear_ino().flush().fence();

            // decrement parent link count
            // we should be able to reuse the regular dec_link_count function (it's a different
            // transition in Alloy). According to Alloy we can wait for the next fence.
            // but that is hard to coordinate with the vectors, so we just do an extra
            let (parent_pi, _) = sbi.get_init_dir_inode_by_vfs_inode(dir.get_inner())?;
            let parent_pi = parent_pi.dec_link_count(&pd)?.flush().fence();

            let pi = pi.set_unmap_page_state()?;

            let pi = rmdir_delete_pages(sbi, &delete_dir_info, pi)?;

            // deallocate the dentry
            let pd = pd.dealloc_dentry().flush();

            let (pi, pd) = fence_all!(pi, pd);

            // if the page that the freed dentry belongs to is now empty, free it
            let parent_page = pd.try_dealloc_parent_page(sbi);
            if let Ok(parent_page) = parent_page {
                let parent_page = parent_page.unmap().flush().fence();
                let parent_page = parent_page.dealloc().flush().fence();
                sbi.page_allocator.dealloc_dir_page(&parent_page)?;
            }

            unsafe {
                bindings::clear_nlink(inode);
            }

            Ok((pi, parent_pi, pd))
        }
        None => Err(ENOENT),
    }
}

fn rmdir_delete_pages<'a>(
    sbi: &'a SbInfo,
    delete_dir_info: &HayleyFsDirInodeInfo,
    pi: InodeWrapper<'a, Clean, UnmapPages, DirInode>,
) -> Result<InodeWrapper<'a, InFlight, Complete, DirInode>> {
    match sbi.mount_opts.write_type {
        Some(WriteType::Iterator) | None => {
            let pages = iterator_rmdir_delete_pages(sbi, delete_dir_info, &pi)?;
            Ok(pi.iterator_dealloc(pages).flush())
        }
        _ => {
            let pages = runtime_rmdir_delete_pages(sbi, delete_dir_info, &pi)?;
            Ok(pi.runtime_dealloc(pages).flush())
        }
    }
}

fn iterator_rmdir_delete_pages<'a>(
    sbi: &'a SbInfo,
    delete_dir_info: &HayleyFsDirInodeInfo,
    pi: &InodeWrapper<'a, Clean, UnmapPages, DirInode>,
) -> Result<DirPageListWrapper<Clean, Free>> {
    if delete_dir_info.get_ino() != pi.get_ino() {
        pr_info!(
            "ERROR: delete_dir_info inode {:?} does not match pi inode {:?}\n",
            delete_dir_info.get_ino(),
            pi.get_ino()
        );
        return Err(EINVAL);
    }
    let pages = DirPageListWrapper::get_dir_pages_to_unmap(delete_dir_info)?;
    let pages = pages.unmap(sbi)?.fence().dealloc(sbi)?.fence().mark_free();
    Ok(pages)
}

fn runtime_rmdir_delete_pages<'a>(
    sbi: &'a SbInfo,
    delete_dir_info: &HayleyFsDirInodeInfo,
    pi: &InodeWrapper<'a, Clean, UnmapPages, DirInode>,
) -> Result<Vec<DirPageWrapper<'a, Clean, Free>>> {
    if delete_dir_info.get_ino() != pi.get_ino() {
        pr_info!(
            "ERROR: delete_dir_info inode {:?} does not match pi inode {:?}\n",
            delete_dir_info.get_ino(),
            pi.get_ino()
        );
        return Err(EINVAL);
    }
    // deallocate pages (if any) belonging to the inode
    // NOTE: we do this in a series of vectors to reduce the number of
    // total flushes. Unclear if this saves us time, or if the overhead
    // of more flushes is less than the time it takes to manage the vecs.
    // We need to do some evaluation of this
    let pages = delete_dir_info.get_all_pages()?;
    let mut unmap_vec = Vec::new();
    let mut to_dealloc = Vec::new();
    let mut deallocated = Vec::new();
    for page in pages.keys() {
        // the pages have already been removed from the inode's page vector
        let page = DirPageWrapper::mark_to_unmap(sbi, page)?;
        unmap_vec.try_push(page)?;
    }
    for page in unmap_vec.drain(..) {
        let page = page.unmap().flush();
        to_dealloc.try_push(page)?;
    }
    let mut to_dealloc = fence_all_vecs!(to_dealloc);
    for page in to_dealloc.drain(..) {
        let page = page.dealloc().flush();
        deallocated.try_push(page)?;
    }
    let deallocated = fence_all_vecs!(deallocated);
    for page in &deallocated {
        sbi.page_allocator.dealloc_dir_page(page)?;
    }
    let freed_pages = DirPageWrapper::mark_pages_free(deallocated)?;
    Ok(freed_pages)
}

fn hayleyfs_rename<'a>(
    sbi: &'a SbInfo,
    old_dir: &fs::INode,
    old_dentry: &fs::DEntry,
    new_dir: &fs::INode,
    new_dentry: &fs::DEntry,
    _flags: u32,
) -> Result<(
    DentryWrapper<'a, Clean, Free>,
    DentryWrapper<'a, Clean, Complete>,
)> {
    let old_name = old_dentry.d_name();
    let _new_name = new_dentry.d_name();

    let (parent_inode, parent_inode_info) =
        sbi.get_init_dir_inode_by_vfs_inode(old_dir.get_inner())?;
    let old_dentry_info = parent_inode_info.lookup_dentry(old_name);
    match old_dentry_info {
        None => Err(ENOENT),
        Some(old_dentry_info) => {
            // not cross dir
            if old_dir.i_ino() == new_dir.i_ino() {
                single_dir_rename(
                    sbi,
                    &old_dentry_info,
                    old_dentry,
                    new_dentry,
                    old_dir,
                    new_dir,
                    parent_inode,
                    parent_inode_info,
                )
            } else {
                // pr_info!("ERROR: cross-directory rename not supported\n");
                let (new_parent_inode, new_parent_inode_info) =
                    sbi.get_init_dir_inode_by_vfs_inode(new_dir.get_inner())?;
                crossdir_rename(
                    sbi,
                    &old_dentry_info,
                    old_dentry,
                    new_dentry,
                    old_dir,
                    new_dir,
                    parent_inode,
                    parent_inode_info,
                    new_parent_inode,
                    new_parent_inode_info,
                )
            }
        }
    }
}

fn single_dir_rename<'a>(
    sbi: &'a SbInfo,
    old_dentry_info: &DentryInfo,
    old_dentry: &fs::DEntry,
    new_dentry: &fs::DEntry,
    old_dir: &fs::INode,
    _new_dir: &fs::INode,
    parent_inode: InodeWrapper<'a, Clean, Start, DirInode>,
    parent_inode_info: &HayleyFsDirInodeInfo,
) -> Result<(
    DentryWrapper<'a, Clean, Free>,
    DentryWrapper<'a, Clean, Complete>,
)> {
    // map to a kernel error type since as_bytes_with_nul() returns a core error
    let old_name = match old_dentry.d_name().as_bytes_with_nul().try_into() {
        Ok(arr) => Ok(arr),
        Err(_) => Err(EINVAL),
    }?;
    let new_name = new_dentry.d_name();
    let old_inode = old_dentry.d_inode();
    let new_inode = new_dentry.d_inode();

    let new_dentry_info = parent_inode_info.lookup_dentry(new_name);
    let inode_type = sbi.check_inode_type_by_vfs_inode(old_dentry.d_inode());
    match inode_type {
        Ok(InodeType::REG) | Ok(InodeType::SYMLINK) | Ok(InodeType::DIR) => {
            // TODO: refactor - there is some repeated code here
            match new_dentry_info {
                Some(new_dentry_info) => {
                    let new_inode_type = sbi.check_inode_type_by_vfs_inode(new_inode)?;

                    // if overwriting a directory, is that directory empty?
                    if new_inode_type == InodeType::DIR {
                        let (_, delete_dir_info) =
                            sbi.get_init_dir_inode_by_vfs_inode(new_inode)?;

                        if delete_dir_info.has_dentries() {
                            return Err(ENOTEMPTY);
                        }
                    }

                    let (src_dentry, dst_dentry) = rename_overwrite_dentry_standard(
                        sbi,
                        old_dentry_info,
                        &new_dentry_info,
                        &parent_inode,
                    )?;

                    match new_inode_type {
                        InodeType::REG | InodeType::SYMLINK => {
                            let (src_dentry, dst_dentry) = rename_overwrite_inode(
                                sbi,
                                new_inode,
                                old_inode,
                                old_name,
                                dst_dentry,
                                src_dentry,
                                parent_inode_info,
                            )?;

                            Ok((src_dentry, dst_dentry))
                        }
                        InodeType::DIR => {
                            let (new_pi, delete_dir_info) =
                                sbi.get_init_dir_inode_by_vfs_inode(new_inode)?;
                            // we DO need to decrement the parent link count because a directory is being deleted
                            // since dst inode is empty, we don't decrement its link count here
                            let parent_inode =
                                parent_inode.dec_link_count_rename(&dst_dentry)?.flush();
                            let dst_dentry = dst_dentry.clear_rename_pointer(&src_dentry).flush();
                            let (_parent_inode, dst_dentry) = fence_all!(parent_inode, dst_dentry);

                            let new_pi = new_pi.set_unmap_page_state()?;
                            // let freed_pages = rmdir_delete_pages(sbi, &delete_dir_info, &new_pi)?;
                            let _new_pi = rmdir_delete_pages(sbi, &delete_dir_info, new_pi)?;

                            let src_dentry = src_dentry.dealloc_dentry().flush().fence();
                            // if the page that the freed dentry belongs to is now empty, free it
                            let parent_page = src_dentry.try_dealloc_parent_page(sbi);
                            if let Ok(parent_page) = parent_page {
                                let parent_page = parent_page.unmap().flush().fence();
                                let parent_page = parent_page.dealloc().flush().fence();
                                sbi.page_allocator.dealloc_dir_page(&parent_page)?;
                            }
                            parent_inode_info
                                .atomic_add_and_delete_dentry(&dst_dentry, old_name)?;

                            unsafe {
                                bindings::drop_nlink(old_dir.get_inner());
                            }

                            Ok((src_dentry, dst_dentry))
                        }
                        _ => {
                            pr_info!("ERROR: bad inode type\n");
                            Err(EINVAL)
                        }
                    }
                }
                None => {
                    // TODO reimplement with new functions
                    Err(EPERM)
                    // // not overwriting a dentry - allocate a new one
                    // // this is the same regardless of whether we are using a reg or dir inode
                    // // allocate new persistent dentry
                    // let dst_dentry = get_free_dentry(sbi, old_dir, parent_inode_info)?;
                    // let dst_dentry = dst_dentry.set_name(new_name)?.flush().fence();
                    // let src_dentry = DentryWrapper::get_init_dentry(*old_dentry_info)?;
                    // let old_dentry_name = src_dentry.get_name();

                    // // set and initialize rename pointer to atomically switch the live dentry
                    // let (dst_dentry, src_dentry) = dst_dentry.set_rename_pointer(sbi, src_dentry);
                    // let dst_dentry = dst_dentry.flush().fence();
                    // let (dst_dentry, src_dentry) = dst_dentry.init_rename_pointer(src_dentry);
                    // let dst_dentry = dst_dentry.flush().fence();

                    // // clear src dentry's inode
                    // let src_dentry = src_dentry.clear_ino().flush().fence();

                    // // clear the rename pointer, using the invalid src dentry as proof that it is
                    // // safe to do so
                    // let dst_dentry = dst_dentry.clear_rename_pointer(&src_dentry).flush().fence();

                    // // deallocate the src dentry
                    // // this fully deallocates the dentry - it can now be used again
                    // let src_dentry = src_dentry.dealloc_dentry().flush().fence();

                    // // atomically update the volatile index
                    // parent_inode_info
                    //     .atomic_add_and_delete_dentry(&dst_dentry, &old_dentry_name)?;

                    // // since we are creating a new dentry, there is no inode to deallocate
                    // Ok((dst_dentry, src_dentry))
                }
            }
        }
        Ok(InodeType::NONE) => {
            pr_info!("ERROR: inode has type None\n");
            Err(ENOENT)
        }
        Err(e) => Err(e),
    }
}

fn crossdir_rename<'a>(
    sbi: &'a SbInfo,
    old_dentry_info: &DentryInfo,
    old_dentry: &fs::DEntry,
    new_dentry: &fs::DEntry,
    _old_dir: &fs::INode,
    _new_dir: &fs::INode,
    _old_parent_inode: InodeWrapper<'a, Clean, Start, DirInode>,
    old_parent_inode_info: &HayleyFsDirInodeInfo,
    new_parent_inode: InodeWrapper<'a, Clean, Start, DirInode>,
    new_parent_inode_info: &HayleyFsDirInodeInfo,
) -> Result<(
    DentryWrapper<'a, Clean, Free>,
    DentryWrapper<'a, Clean, Complete>,
)> {
    // map to a kernel error type since as_bytes_with_nul() returns a core error
    let old_name = match old_dentry.d_name().as_bytes_with_nul().try_into() {
        Ok(arr) => Ok(arr),
        Err(_) => Err(EINVAL),
    }?;
    let new_name = new_dentry.d_name();
    let old_inode = old_dentry.d_inode();
    let new_inode = new_dentry.d_inode();

    let new_dentry_info = new_parent_inode_info.lookup_dentry(new_name);
    let inode_type = sbi.check_inode_type_by_vfs_inode(old_dentry.d_inode());

    match inode_type {
        Ok(InodeType::REG) | Ok(InodeType::SYMLINK) | Ok(InodeType::DIR) => {
            match new_dentry_info {
                Some(new_dentry_info) => {
                    // overwriting an existing dentry
                    let new_inode_type = sbi.check_inode_type_by_vfs_inode(new_inode)?;

                    // we can't overwrite the new dentry yet because if we are renaming a directory,
                    // we need to increment the new parent's link count first.

                    match new_inode_type {
                        InodeType::REG | InodeType::SYMLINK => {
                            let (src_dentry, dst_dentry) = rename_overwrite_dentry_standard(
                                sbi,
                                old_dentry_info,
                                &new_dentry_info,
                                &new_parent_inode,
                            )?;
                            let (src_dentry, dst_dentry) = rename_overwrite_inode(
                                sbi,
                                new_inode,
                                old_inode,
                                old_name,
                                dst_dentry,
                                src_dentry,
                                old_parent_inode_info,
                            )?;
                            Ok((src_dentry, dst_dentry))
                        }
                        InodeType::DIR => {
                            let (_, delete_dir_info) =
                                sbi.get_init_dir_inode_by_vfs_inode(new_inode)?;

                            if delete_dir_info.has_dentries() {
                                return Err(ENOTEMPTY);
                            }

                            Err(EPERM)
                        }
                        _ => Err(EINVAL),
                    }
                }
                None => {
                    // create a new dentry
                    // TODO implement
                    Err(EPERM)
                }
            }
        }
        Ok(InodeType::NONE) => {
            pr_info!("ERROR: inode has type None\n");
            Err(ENOENT)
        }
        Err(e) => Err(e),
    }
}

// TODO: clean up and refactor these functions once rename is fully implemented

fn rename_overwrite_dentry_crossdir_create<'a>(
    sbi: &'a SbInfo,
    old_dentry_info: &DentryInfo,
    new_dentry_info: &DentryInfo,
    _src_parent_info: &HayleyFsDirInodeInfo,
    dst_parent_inode: InodeWrapper<'a, Clean, Start, DirInode>,
) -> Result<(
    DentryWrapper<'a, Clean, ClearIno>,
    DentryWrapper<'a, Clean, InitRenamePointer>,
    InodeWrapper<'a, Clean, IncLink, DirInode>,
)> {
    // increment the dst parent link count if necessary. regardless of whether we actually
    // increment it, we still change the typestate so that we can call init_rename_pointer
    let dst_parent_inode = dst_parent_inode.inc_link_count()?.flush();
    // overwriting another file, potentially deleting its inode
    let src_dentry = DentryWrapper::get_init_dentry(*old_dentry_info)?;
    let dst_dentry = DentryWrapper::get_init_dentry(*new_dentry_info)?;
    // set and initialize rename pointer to atomically switch the live dentry
    let (dst_dentry, src_dentry) = dst_dentry.set_rename_pointer(sbi, src_dentry);
    let dst_dentry = dst_dentry.flush();
    let (dst_parent_inode, dst_dentry) = fence_all!(dst_parent_inode, dst_dentry);
    let (dst_dentry, src_dentry) =
        dst_dentry.init_rename_pointer_crossdir_create(src_dentry, &dst_parent_inode);
    let dst_dentry = dst_dentry.flush().fence();

    // clear src dentry's inode
    let src_dentry = src_dentry.clear_ino().flush().fence();

    Ok((src_dentry, dst_dentry, dst_parent_inode))
}

fn rename_overwrite_dentry_standard<'a>(
    sbi: &'a SbInfo,
    old_dentry_info: &DentryInfo,
    new_dentry_info: &DentryInfo,
    dst_parent_inode: &InodeWrapper<'a, Clean, Start, DirInode>,
) -> Result<(
    DentryWrapper<'a, Clean, ClearIno>,
    DentryWrapper<'a, Clean, InitRenamePointer>,
)> {
    // overwriting another file, potentially deleting its inode
    let src_dentry = DentryWrapper::get_init_dentry(*old_dentry_info)?;
    let dst_dentry = DentryWrapper::get_init_dentry(*new_dentry_info)?;
    // set and initialize rename pointer to atomically switch the live dentry
    let (dst_dentry, src_dentry) = dst_dentry.set_rename_pointer(sbi, src_dentry);
    let dst_dentry = dst_dentry.flush().fence();
    let (dst_dentry, src_dentry) =
        dst_dentry.init_rename_pointer_standard(src_dentry, &dst_parent_inode);
    let dst_dentry = dst_dentry.flush().fence();

    // clear src dentry's inode
    let src_dentry = src_dentry.clear_ino().flush().fence();

    Ok((src_dentry, dst_dentry))
}

fn rename_overwrite_inode<'a>(
    sbi: &SbInfo,
    new_inode: *mut bindings::inode,
    old_inode: *mut bindings::inode,
    old_name: &[u8; MAX_FILENAME_LEN],
    dst_dentry: DentryWrapper<'a, Clean, InitRenamePointer>,
    src_dentry: DentryWrapper<'a, Clean, ClearIno>,
    old_parent_inode_info: &HayleyFsDirInodeInfo,
) -> Result<(
    DentryWrapper<'a, Clean, Free>,
    DentryWrapper<'a, Clean, Complete>,
)> {
    let (new_pi, _) = sbi.get_init_reg_inode_by_vfs_inode(new_inode)?;
    // decrement link count of the inode whose dentry is being overwritten
    // this is the inode being unlinked, not the parent directory
    let new_pi = new_pi.dec_link_count_rename(&dst_dentry)?.flush();
    // clear the rename pointer in the dst dentry, since the src has been invalidated
    let dst_dentry = dst_dentry.clear_rename_pointer(&src_dentry).flush();
    let (new_pi, dst_dentry) = fence_all!(new_pi, dst_dentry);
    // deallocate the src dentry
    // this fully deallocates the dentry - it can now be used again
    let src_dentry = src_dentry.dealloc_dentry().flush().fence();
    // if the page that the freed dentry belongs to is now empty, free it
    let parent_page = src_dentry.try_dealloc_parent_page(sbi);
    if let Ok(parent_page) = parent_page {
        let parent_page = parent_page.unmap().flush().fence();
        let parent_page = parent_page.dealloc().flush().fence();
        sbi.page_allocator.dealloc_dir_page(&parent_page)?;
    }
    // atomically update the volatile index
    // TODO is this still right?
    old_parent_inode_info.atomic_add_and_delete_dentry(&dst_dentry, old_name)?;
    // finish deallocating the old inode and its pages
    finish_unlink(sbi, old_inode, new_pi)?;

    Ok((src_dentry, dst_dentry))
}

// TODO: delete the dir page if this dentry was the last one in it
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

        end_timing!(DecLinkCount, dec_link_count);

        // if the page that the freed dentry belongs to is now empty, free it
        let parent_page = pd.try_dealloc_parent_page(sbi);
        if let Ok(parent_page) = parent_page {
            let parent_page = parent_page.unmap().flush().fence();
            let parent_page = parent_page.dealloc().flush().fence();
            sbi.page_allocator.dealloc_dir_page(&parent_page)?;
        }

        let pi = finish_unlink(sbi, inode, pi)?;

        end_timing!(UnlinkFullDecLink, unlink_full_declink);

        Ok((pi, pd))
    } else {
        Err(ENOENT)
    }
}

fn finish_unlink<'a>(
    sbi: &'a SbInfo,
    inode: *mut bindings::inode,
    pi: InodeWrapper<'a, Clean, DecLink, RegInode>,
) -> Result<InodeWrapper<'a, Clean, Complete, RegInode>> {
    match sbi.mount_opts.write_type {
        Some(WriteType::Iterator) | None => iterator_finish_unlink(sbi, inode, pi),
        _ => runtime_finish_unlink(sbi, inode, pi),
    }
}

fn iterator_finish_unlink<'a>(
    sbi: &'a SbInfo,
    inode: *mut bindings::inode,
    pi: InodeWrapper<'a, Clean, DecLink, RegInode>,
) -> Result<InodeWrapper<'a, Clean, Complete, RegInode>> {
    let result = pi.try_complete_unlink_iterator()?;
    if let Ok(result) = result {
        // there are still links left - just decrement VFS link count and return
        unsafe {
            bindings::drop_nlink(inode);
        }
        Ok(result)
    } else if let Err((pi, pages)) = result {
        // no links left - we need to deallocate all of the pages
        let pages = pages.unmap(sbi)?.fence().dealloc(sbi)?.fence().mark_free();
        unsafe {
            bindings::drop_nlink(inode);
        }
        let pi = pi.iterator_dealloc(pages).flush().fence();
        Ok(pi)
    } else {
        Err(EINVAL)
    }
}

fn runtime_finish_unlink<'a>(
    sbi: &'a SbInfo,
    inode: *mut bindings::inode,
    pi: InodeWrapper<'a, Clean, DecLink, RegInode>,
) -> Result<InodeWrapper<'a, Clean, Complete, RegInode>> {
    let result = pi.try_complete_unlink_runtime(sbi)?;
    if let Ok(result) = result {
        unsafe {
            bindings::drop_nlink(inode);
        }
        Ok(result)
    } else if let Err((pi, mut pages)) = result {
        // go through each page and deallocate it
        // we can drain the vector since missing a page will result in
        // a runtime panic
        // NOTE: we do this in a series of vectors to reduce the number of
        // total flushes. Unclear if this saves us time, or if the overhead
        // of more flushes is less than the time it takes to manage the vecs.
        // We need to do some evaluation of this
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
        let pi = pi.runtime_dealloc(freed_pages).flush().fence();
        end_timing!(DeallocPages, dealloc_pages);
        Ok(pi)
    } else {
        Err(EINVAL)
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
    // we always use single DirPageWrapper here, rather than an iterator,
    // regardless of the mount options selected, because we are only
    // allocating one page at a time. We could implement a special
    // StaticDirPageWrapper for just this case but it probably will not make
    // a noticeable difference
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
