#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

use crate::def::*;
use crate::dir::*;
use crate::file::*;
use crate::finalize::*;
use crate::h_inode::hayleyfs_inode::*;
use crate::h_inode::*;
use crate::pm::*;
use crate::super_def::hayleyfs_bitmap::*;
use crate::super_def::*;
use crate::{fence_all, fence_obj};
use core::ptr::{eq, null_mut};
use kernel::bindings::{
    current_time, d_instantiate, d_splice_alias, dentry, iget_failed, iget_locked, inc_nlink,
    inode, inode_init_owner, inode_operations, insert_inode_locked, new_inode, set_nlink,
    super_block, umode_t, unlock_new_inode, user_namespace, FS_APPEND_FL, FS_DIRSYNC_FL,
    FS_IMMUTABLE_FL, FS_NOATIME_FL, FS_SYNC_FL, I_NEW, S_APPEND, S_DAX, S_DIRSYNC, S_IFDIR,
    S_IMMUTABLE, S_NOATIME, S_SYNC,
};
use kernel::c_types::c_char;
use kernel::prelude::*;
use kernel::{c_default_struct, PAGE_SIZE};

pub(crate) static HayleyfsDirInodeOps: inode_operations = inode_operations {
    mkdir: Some(hayleyfs_mkdir),
    lookup: Some(hayleyfs_lookup),
    create: Some(hayleyfs_create),
    rmdir: Some(hayleyfs_rmdir),
    ..c_default_struct!(inode_operations)
};

// TODO: this probably should not be the static lifetime?
pub(crate) fn hayleyfs_iget(sb: *mut super_block, ino: usize) -> Result<&'static mut inode> {
    let inode = unsafe { &mut *(iget_locked(sb, ino as u64) as *mut inode) };
    if eq(inode, null_mut()) {
        unsafe { iget_failed(inode) };
        return Err(Error::ENOMEM);
    }
    if (inode.i_state & I_NEW as u64) == 0 {
        return Ok(inode);
    }
    inode.i_ino = ino as u64;
    let sbi = hayleyfs_get_sbi(sb);

    // read the persistent inode to fill in info
    let pi = InodeWrapper::read_unknown_inode(sbi, &ino);
    inode.i_mode = pi.get_mode();
    inode.i_size = pi.get_size();
    unsafe {
        set_nlink(inode, pi.get_link_count());
        hayleyfs_write_uid(inode, pi.get_uid());
        hayleyfs_write_gid(inode, pi.get_gid());
    }

    // this logic is taken from NOVA's nova_set_inode_flags function
    let flags = pi.get_flags();
    inode.i_flags &= !(S_SYNC | S_APPEND | S_IMMUTABLE | S_NOATIME | S_DIRSYNC);
    if flags & FS_SYNC_FL != 0 {
        inode.i_flags |= S_SYNC;
    }
    if flags & FS_APPEND_FL != 0 {
        inode.i_flags |= S_APPEND;
    }
    if flags & FS_IMMUTABLE_FL != 0 {
        inode.i_flags |= S_IMMUTABLE;
    }
    if flags & FS_NOATIME_FL != 0 {
        inode.i_flags |= S_NOATIME;
    }
    if flags & FS_DIRSYNC_FL != 0 {
        inode.i_flags |= S_DIRSYNC;
    }
    inode.i_flags |= S_DAX;

    unsafe {
        if hayleyfs_isdir(inode.i_mode) {
            inode.i_op = &HayleyfsDirInodeOps;
        } else if hayleyfs_isreg(inode.i_mode) {
            inode.__bindgen_anon_3.i_fop = &HayleyfsFileOps; // fileOps has to be mutable so this has to be unsafe. Why does it have to be mutable???
        }
    }

    // TODO: should update persistent access time here?
    inode.i_ctime.tv_sec = pi.get_ctime();
    inode.i_ctime.tv_nsec = 0;
    inode.i_mtime.tv_sec = pi.get_mtime();
    inode.i_mtime.tv_nsec = 0;
    inode.i_atime = unsafe { current_time(inode) };

    unsafe { unlock_new_inode(inode) };

    Ok(inode)
}

#[no_mangle]
unsafe extern "C" fn hayleyfs_mkdir(
    mnt_userns_raw: *mut user_namespace,
    dir_raw: *mut inode,
    dentry_raw: *mut dentry,
    mode: umode_t,
) -> i32 {
    // convert arguments to mutable references rather than raw pointers
    let mnt_userns = unsafe { &mut *(mnt_userns_raw as *mut user_namespace) };
    let dir = unsafe { &mut *(dir_raw as *mut inode) };
    let dentry = unsafe { &mut *(dentry_raw as *mut dentry) };

    let result = _hayleyfs_mkdir(mnt_userns, dir, dentry, mode);
    match result {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno(),
    }
}

/* THIS IS NOT UP TO DATE
 * TODO: update with more detailed dentry initialization dependencies
 *  ┌────────────────┐               ┌───────────────────┐
 *  │                │               │                   │
 *  │ allocate inode │               │ allocate dir page │
 *  │                │               │                   │
 *  └───────┬──────┬─┘               └─────────┬─────────┘
 *          │      │                           │
 *          │      │                           │
 *          │      └───────────────────┐       │
 *          │                          │       │
 *       ───┼──────────────────────────┼───────┼────
 *          │                          │       │
 * ┌────────▼─────────┐             ┌──▼───────▼──────────┐
 * │                  │             │                     │
 * │ initialize inode │             │ initialize dentries │
 * │                  │             │                     │
 * └────────────────┬─┘             └─┬───────────────────┘
 *                  │                 │
 *                ──┼─────────────────┼──
 *                  │                 │
 *                 ┌▼─────────────────▼┐             ┌───────────────────────┐
 *                 │                   │             │                       │
 *                 │ add page to inode │             │ inc parent link count │
 *                 │                   │             │                       │
 *                 └────────────────┬──┘             └──┬────────────────────┘
 *                                  │                   │
 *                                ──┼───────────────────┼────
 *                                  │                   │
 *                               ┌──▼───────────────────▼───┐
 *                               │                          │
 *                               │ add new dentry to parent │
 *                               │                          │
 *                               └──────────────────────────┘
 */

// if sbi.mount_opts.crash_point == 1 {
//     return Err(Error::EINVAL);
// }

/// NOTE: link count is conservatively high. we may crash with an incremented link count
/// before the child has been added. this is causes a space leak but is NOT incorrect.
#[no_mangle]
fn _hayleyfs_mkdir(
    mnt_userns: &mut user_namespace,
    dir: &mut inode,
    dentry: &mut dentry,
    mode: umode_t,
) -> Result<()> {
    // TODO: more graceful way of checking crashes. find a way that doesn't introduce so much redundant code

    let sb = unsafe { &mut *(dir.i_sb as *mut super_block) };
    let sbi = hayleyfs_get_sbi(sb);

    let dentry_name = unsafe { CStr::from_char_ptr((*dentry).d_name.name as *const c_char) };
    if dentry_name.len() > MAX_FILENAME_LEN {
        pr_info!("dentry name {:?} is too long\n", dentry_name);
        return Err(Error::ENAMETOOLONG);
    }

    // get an inode
    let inode_bitmap = BitmapWrapper::read_inode_bitmap(sbi);
    let data_bitmap = BitmapWrapper::read_data_bitmap(sbi);
    let (ino, inode_bitmap) = inode_bitmap.find_and_set_next_zero_bit()?;
    let inode_bitmap = inode_bitmap.flush();

    let (page_no, data_bitmap) = data_bitmap.find_and_set_next_zero_bit()?;
    let data_bitmap = data_bitmap.flush();
    // TODO: do we need to use data bitmap again?
    let (inode_bitmap, _data_bitmap) = fence_all!(inode_bitmap, data_bitmap);

    let pi = InodeWrapper::read_dir_inode(sbi, &ino);
    let parent_ino: InodeNum = dir.i_ino.try_into()?;
    let parent_pi = InodeWrapper::read_dir_inode(sbi, &parent_ino);

    // set up vfs inode
    // TODO: at what point should this actually happen?
    let mut inode = hayleyfs_new_vfs_inode(
        sb,
        dir,
        ino,
        mnt_userns,
        mode | S_IFDIR as u16,
        NewInodeType::Mkdir,
        PAGE_SIZE.try_into()?,
    );
    unsafe {
        d_instantiate(dentry, inode); // instantiate VFS dentry with the inode
        inc_nlink(dir as *mut inode);
        unlock_new_inode(inode);
    };

    let pi = pi.initialize_inode(
        mode.into(),
        parent_pi.get_flags(),
        &mut inode,
        &inode_bitmap,
    );

    let self_dentry = hayleyfs_dir::initialize_self_dentry(sbi, page_no, ino)?;
    let parent_dentry = hayleyfs_dir::initialize_parent_dentry(sbi, page_no, ino)?;
    let (pi, self_dentry, parent_dentry) = fence_all!(pi, self_dentry, parent_dentry);

    // increment parent link count
    let parent_pi = parent_pi.inc_links(); // TODO: increment vfs link count as well?

    // add page with newly initialized dentries to the new inode
    let pi = pi.add_dir_page(Some(page_no), self_dentry, parent_dentry);
    let (pi, parent_pi) = fence_all!(pi, parent_pi);

    // add new dentry to parent
    // we can read the dentry at any time, but we can't actually modify it without methods
    // that require proof of link inc and new inode init
    // TODO: handle panic
    let new_dentry =
        hayleyfs_dir::DentryWrapper::get_new_dentry(sbi, parent_pi.get_data_page_no().unwrap())?;

    // TODO: do something with last new_dentry variable
    // TODO: this should rely on the vfs inode being valid?
    let _new_dentry = new_dentry
        .initialize_mkdir_dentry(ino, dentry_name.to_str()?, &pi, &parent_pi)
        .fence();

    Ok(())
}

fn hayleyfs_new_vfs_inode(
    sb: &mut super_block,
    dir: &inode,
    ino: InodeNum,
    mnt_userns: &mut user_namespace,
    mode: umode_t,
    new_type: NewInodeType,
    size: i64,
) -> &'static mut inode {
    // TODO: handle errors properly
    let inode = unsafe { &mut *(new_inode(sb) as *mut inode) };

    unsafe {
        inode_init_owner(mnt_userns as *mut user_namespace, inode, dir, mode);
    }

    inode.i_mode = mode;
    inode.i_ino = ino as u64;

    match new_type {
        NewInodeType::Mkdir => {
            inode.i_op = &HayleyfsDirInodeOps;
            unsafe {
                inode.__bindgen_anon_3.i_fop = &HayleyfsDirOps;
                set_nlink(inode, 2);
            }
        }
        NewInodeType::Create => unsafe {
            // TODO: finish
            inode.__bindgen_anon_3.i_fop = &HayleyfsFileOps;
            set_nlink(inode, 1);
        },
    }

    let current_time = unsafe { current_time(inode) };
    inode.i_mtime = current_time;
    inode.i_ctime = current_time;
    inode.i_atime = current_time;
    inode.i_size = size;

    unsafe { insert_inode_locked(inode) };

    inode
}

#[no_mangle]
unsafe extern "C" fn hayleyfs_lookup(
    dir_raw: *mut inode,
    dentry_raw: *mut dentry,
    flags: u32,
) -> *mut dentry {
    let dir = unsafe { &mut *(dir_raw as *mut inode) };
    let dentry = unsafe { &mut *(dentry_raw as *mut dentry) };

    let result = _hayleyfs_lookup(dir, dentry, flags);
    match result {
        Ok(dentry) => dentry,
        Err(e) => unsafe { hayleyfs_err_ptr(e.to_kernel_errno().into()) as *mut dentry },
    }
}

#[no_mangle]
pub(crate) fn _hayleyfs_lookup(
    dir: &mut inode,
    dentry: &mut dentry,
    _flags: u32,
) -> Result<*mut dentry> {
    let dentry_name = unsafe { CStr::from_char_ptr((*dentry).d_name.name as *const c_char) };

    let dir = unsafe { &mut *(dir as *mut inode) };

    let sb = dir.i_sb;
    let sbi = hayleyfs_get_sbi(sb);

    // look up parent inode
    // TODO: check that this is actually a directory and return an error if it isn't
    let parent_pi = InodeWrapper::read_dir_inode(sbi, &(dir.i_ino.try_into()?));

    match parent_pi.get_data_page_no() {
        Some(page_no) => {
            let lookup_res =
                hayleyfs_dir::lookup_ino_by_name(sbi, page_no, dentry_name.as_bytes_with_nul());
            match lookup_res {
                Ok(ino) => {
                    let inode = hayleyfs_iget(sb, ino)?;
                    Ok(unsafe { d_splice_alias(inode, dentry) })
                }
                Err(Error::ENOENT) => Ok(unsafe { d_splice_alias(core::ptr::null_mut(), dentry) }),
                Err(e) => Err(e),
            }
        }
        None => Err(Error::EACCES),
    }
}

#[no_mangle]
unsafe extern "C" fn hayleyfs_create(
    mnt_userns_raw: *mut user_namespace,
    inode_raw: *mut inode,
    dentry_raw: *mut dentry,
    mode: umode_t,
    excl: bool,
) -> i32 {
    let mnt_userns = unsafe { &mut *(mnt_userns_raw as *mut user_namespace) };
    let inode = unsafe { &mut *(inode_raw as *mut inode) };
    let dentry = unsafe { &mut *(dentry_raw as *mut dentry) };

    let result = _hayleyfs_create(mnt_userns, inode, dentry, mode, excl);

    match result {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno(),
    }
}

fn _hayleyfs_create(
    mnt_userns: &mut user_namespace,
    inode: &mut inode,
    dentry: &mut dentry,
    mode: umode_t,
    _excl: bool,
) -> Result<()> {
    // TODO: handle excl case - if true, create should only succeed if the file doesn't already exist

    let sb = unsafe { &mut *(inode.i_sb as *mut super_block) };
    let sbi = hayleyfs_get_sbi(sb);

    // inode parameter is the parent's inode
    let file_name = unsafe { CStr::from_char_ptr((*dentry).d_name.name as *const c_char) };
    if file_name.len() > MAX_FILENAME_LEN {
        pr_info!("file_name name {:?} is too long\n", file_name);
        return Err(Error::ENAMETOOLONG);
    }

    let inode_bitmap = BitmapWrapper::read_inode_bitmap(sbi);
    let (ino, inode_bitmap) = inode_bitmap.find_and_set_next_zero_bit()?;
    let inode_bitmap = inode_bitmap.flush().fence();

    let pi = InodeWrapper::read_file_inode(sbi, &ino);
    let parent_ino = inode.i_ino.try_into()?;
    let parent_pi = InodeWrapper::read_dir_inode(sbi, &parent_ino);

    let new_inode =
        hayleyfs_new_vfs_inode(sb, inode, ino, mnt_userns, mode, NewInodeType::Create, 0);
    unsafe {
        d_instantiate(dentry, new_inode); // instantiate VFS dentry with the inode
        unlock_new_inode(new_inode);
    };

    let pi = pi
        .initialize_inode(mode, parent_pi.get_flags(), new_inode, &inode_bitmap)
        .fence();

    let new_dentry =
        hayleyfs_dir::DentryWrapper::get_new_dentry(sbi, parent_pi.get_data_page_no().unwrap())?;

    // TODO: do something with this
    let _new_dentry = new_dentry
        .initialize_file_dentry(ino, file_name.to_str()?, &pi)
        .fence();

    Ok(())
}

#[no_mangle]
unsafe extern "C" fn hayleyfs_rmdir(inode_raw: *mut inode, dentry_raw: *mut dentry) -> i32 {
    let inode = unsafe { &mut *(inode_raw as *mut inode) };
    let dentry = unsafe { &mut *(dentry_raw as *mut dentry) };
    let sb = unsafe { &mut *(inode.i_sb as *mut super_block) };
    let sbi = hayleyfs_get_sbi(sb);

    let result = _hayleyfs_rmdir(sbi, inode, dentry);

    match result {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno(),
    }
}

/// IMPORTANT NOTE: as in original soft updates, link count is conservatively high.
/// we decrement parent's link count AFTER deleting dentry for the deleted child
/// (which implicitly removes the link from the child from the parent).
/// this means that a poorly timed crash could result in a parent with a link count
/// that is too high. In the original soft updates paper, this is not considered
/// a consistency issue, so we don't consider it an issue either. It can be easily
/// fixed with fsck. The main issue with this is that it results in undeletable directories,
/// which is not ideal, but it's just a space leak issue
///
/// inode is the parent inode, dentry is the dentry of the file to delete
fn _hayleyfs_rmdir(
    sbi: &SbInfo,
    dir: &mut inode,
    dentry: &mut dentry,
) -> Result<RmdirFinalizeToken> {
    // TODO: do we have to check the child's link count? or does VFS do that?

    // 1. delete child dentry from parent
    let dentry_name = unsafe { CStr::from_char_ptr(dentry.d_name.name as *const c_char) };

    let parent_ino = dir.i_ino as usize;

    // read the parent inode from PM
    let parent_pi = InodeWrapper::read_dir_inode(sbi, &parent_ino);
    // obtain the dentry to delete
    let delete_dentry = parent_pi.lookup_dentry_in_inode(sbi, dentry_name)?;
    let child_ino = delete_dentry.get_ino();
    // and set it invalid
    let delete_dentry = unsafe { delete_dentry.set_invalid() }.fence();

    // 2. decrement parent link count
    // TODO: do something with parent pi to finalize it and make sure you've used
    // everything up
    let parent_pi = parent_pi.dec_links();

    // 3. now that there isn't a pointer to the deleted directory, we can clean it up
    let child_inode = InodeWrapper::read_dir_inode(sbi, &child_ino);
    // TODO: we actually can probably ignore errors reading . and .. here
    let self_dentry = child_inode.lookup_dentry_in_inode(sbi, b".")?;
    let parent_dentry = child_inode.lookup_dentry_in_inode(sbi, b"..")?;

    let self_dentry = unsafe { self_dentry.set_invalid() };
    let parent_dentry = unsafe { parent_dentry.set_invalid() };

    // 4. zero the deleted inode
    // better handling in case the child directory doesn't have dir page for some reason
    let child_dir_page = child_inode.get_data_page_no().unwrap();

    let child_inode = child_inode.zero_inode();

    let (parent_pi, self_dentry, parent_dentry, child_inode) =
        fence_all!(parent_pi, self_dentry, parent_dentry, child_inode);

    // 5. clear bits in the bitmaps
    // TODO: make it harder to clear the wrong bits? that would hopefully show
    // up in regular testing though
    let inode_bitmap = BitmapWrapper::read_inode_bitmap(sbi)
        .clear_bit(child_ino)?
        .flush();
    let data_bitmap = BitmapWrapper::read_data_bitmap(sbi)
        .clear_bit(child_dir_page)?
        .flush();

    let (inode_bitmap, data_bitmap) = fence_all!(inode_bitmap, data_bitmap);

    let token = RmdirFinalizeToken::new(
        parent_pi,
        delete_dentry,
        self_dentry,
        parent_dentry,
        child_inode,
        inode_bitmap,
        data_bitmap,
    );
    Ok(token)
}
