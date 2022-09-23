//! Main fs file

use crate::{pm::*, super_def::*};
use core::ptr;
use kernel::prelude::*;
use kernel::{c_str, fs};

mod cdefs;
mod dir;
mod inode;
mod namei;
mod pm;
mod super_def;

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

    fn try_new() -> Result<Self::Data> {
        pr_info!("creating context");
        Ok(Box::try_new(SbInfo::new()?)?)
    }
}

impl fs::Type for HayleyFS {
    type Data = Box<SbInfo>;
    type Context = Self;
    type INodeData = INodeData;
    const SUPER_TYPE: fs::Super = fs::Super::BlockDev;
    const NAME: &'static CStr = c_str!("hayleyfs");
    const FLAGS: i32 = fs::flags::USERNS_MOUNT | fs::flags::REQUIRES_DEV;

    /// Right now this function always initializes a new file system
    fn fill_super(
        mut data: Self::Data,
        mut sb: fs::NewSuperBlock<'_, Self>,
    ) -> Result<&fs::SuperBlock<Self>> {
        // obtain virtual address and size of PM device
        data.get_pm_info(&mut sb)?;
        // initialize superblock
        let sb = sb.init(
            data,
            &fs::SuperParams {
                magic: 0xabcdef,
                ..fs::SuperParams::DEFAULT
            },
        )?;

        let sbi = sb.get_fs_info();

        // zero out the PM device
        // TODO: should probably be done using non-temporal stores
        unsafe {
            let pm_size = sbi.pm_size;
            let virt_addr = sbi.danger_get_pm_addr();
            // docs: "write_bytes is similar to C’s memset, but sets count * size_of::<T>() bytes to val"
            // T here is c_void, so we have to divide pm_size by 8 to set the correct number of bytes
            ptr::write_bytes(virt_addr, 0, (pm_size / 8).try_into()?);
            clwb(virt_addr, pm_size.try_into().unwrap(), true);
        }

        let root_inode = sb.try_new_dcache_dir_inode::<HayleyFS>(fs::INodeParams {
            mode: 0o755,
            ino: ROOT_INO,
            value: (),
        })?;
        let root = sb.try_new_root_dentry(root_inode)?;

        Ok(sb.init_root(root)?)
    }
}
