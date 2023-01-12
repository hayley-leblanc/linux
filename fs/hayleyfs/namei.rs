use crate::balloc::*;
use crate::defs::*;
use crate::dir::*;
use crate::h_inode::*;
use crate::typestate::*;
use crate::volatile::*;
use core::ffi;
use kernel::prelude::*;
use kernel::{bindings, fs, inode};

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
        _mnt_userns: &fs::UserNamespace,
        dir: &fs::INode,
        dentry: &fs::DEntry,
        _umode: bindings::umode_t,
        _excl: bool,
    ) -> Result<i32> {
        pr_info!("creating {:?} in {:?}\n", dentry.d_name(), dir.i_ino());

        let sb = dir.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        // get dir pages associated with the parent (if any)
        let parent_ino = dir.i_ino();
        let result = sbi.ino_dir_page_map.lookup_ino(&parent_ino);
        if let Some(_pages) = result {
            unimplemented!();
        } else {
            pr_info!("no pages associated with the parent\n");
            // allocate a page
            let dir_page = DirPageWrapper::alloc_dir_page(sbi)?.flush().fence();
            let parent_inode = InodeWrapper::get_init_inode_by_ino(sbi, parent_ino);
            if let Ok(parent_inode) = parent_inode {
                let mut dir_page = dir_page
                    .set_dir_page_backpointer(parent_inode)
                    .flush()
                    .fence();
                // TODO: get_free_dentry() should never return an error since all dentries
                // in the newly-allocated page should be free - but check on that and confirm
                let pd = dir_page.get_free_dentry()?;
                add_new_dentry(sbi, pd, dir.i_ino(), dentry.d_name());
            } else {
                pr_info!("ERROR: parent inode is not initialized");
                return Err(EPERM);
            }
        }

        Err(EINVAL)
    }
}

fn add_new_dentry<'a>(
    sbi: &mut SbInfo,
    dentry: DentryWrapper<'a, Clean, Free>,
    parent_ino: InodeNum,
    name: &CStr,
) -> Result<(
    DentryWrapper<'a, Clean, Init>,
    InodeWrapper<'a, Clean, Complete>,
)> {
    // allocate the dentry
    let dentry = dentry.set_name(name)?.flush().fence();

    // set up the new inode
    let new_ino = sbi.inode_allocator.alloc_ino()?;
    let inode = InodeWrapper::get_free_inode_by_ino(sbi, new_ino)?;
    let inode = inode.allocate_file_inode().flush().fence();
}
