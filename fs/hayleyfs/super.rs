// SPDX-License-Identifier: GPL-2.0

//! Rust file system sample.

use core::{ffi, ptr};
use defs::*;
use inode::*;
use kernel::prelude::*;
use kernel::{bindings, c_str, fs};
use pm::*;

mod defs;
mod inode;
mod pm;
mod typestate;

module_fs! {
    type: HayleyFs,
    name: "hayley_fs",
    author: "Hayley LeBlanc",
    description: "hayley_fs",
    license: "GPL",
}

struct HayleyFs;

impl SbInfo {
    fn new() -> Self {
        SbInfo {
            dax_dev: ptr::null_mut(),
            virt_addr: ptr::null_mut(),
            size: 0, // total size of the PM device
        }
    }

    fn get_pm_info(&mut self, sb: &fs::NewSuperBlock<'_, HayleyFs>) -> Result<()> {
        // obtain the dax_device struct
        let dax_dev = sb.get_dax_dev()?;

        // obtain virtual address and size of the dax device
        // SAFETY: The type invariant of `sb` guarantees that `sb.sb` is the only pointer to
        // a newly-allocated superblock. The safety condition of `get_dax_dev` guarantees
        // that `dax_dev` is the only active pointer to the associated `dax_device`, so it is
        // safe to mutably dereference it.
        let size = unsafe {
            bindings::dax_direct_access(
                dax_dev,
                0,
                (usize::MAX / kernel::PAGE_SIZE).try_into()?,
                bindings::dax_access_mode_DAX_ACCESS,
                &mut self.virt_addr,
                ptr::null_mut(),
            )
        };

        self.dax_dev = dax_dev;
        self.size = size;

        Ok(())
    }

    fn get_size(&self) -> i64 {
        self.size
    }

    /// obtaining the virtual address is safe - dereferencing it is not
    fn get_virt_addr(&self) -> *mut ffi::c_void {
        self.virt_addr
    }

    unsafe fn get_inode_by_ino(&self, ino: InodeNum) -> Result<&mut HayleyFsInode> {
        // we don't use inode 0
        if ino >= NUM_INODES || ino == 0 {
            return Err(EINVAL);
        }

        // for now, assume that sb and inodes are 64 bytes
        // TODO: update that with the final size
        let inode_size = 64;
        let sb_size = 64;
        // let sb_size = mem::size_of::<HayleyFsSuperBlock>();
        // let inode_size = mem::size_of::<HayleyFsInode>();

        let inode_offset: usize = (ino * inode_size).try_into()?;
        unsafe {
            let inode_addr = self.virt_addr.offset((sb_size + inode_offset).try_into()?);
            Ok(&mut *(inode_addr as *mut HayleyFsInode))
        }
    }
}

// SbInfo must be Send and Sync for it to be used as the Context's data.
// However, raw pointers are not Send or Sync because they are not safe to
// access across threads. This is a lint - they aren't safe to access within a
// single thread either - and we know that the raw pointer will be immutable,
// so it's ok to mark it Send + Sync here
unsafe impl Send for SbInfo {}
unsafe impl Sync for SbInfo {}

#[vtable]
impl fs::Context<Self> for HayleyFs {
    type Data = Box<SbInfo>;

    fn try_new() -> Result<Self::Data> {
        pr_info!("Context created");
        Ok(Box::try_new(SbInfo::new())?)
    }
}

impl fs::Type for HayleyFs {
    type Context = Self;
    type Data = Box<SbInfo>;
    const SUPER_TYPE: fs::Super = fs::Super::BlockDev; // TODO: or BlockDev?
    const NAME: &'static CStr = c_str!("hayleyfs");
    const FLAGS: i32 = fs::flags::REQUIRES_DEV | fs::flags::USERNS_MOUNT;

    // TODO: take init argument and only initialize new FS if it is given
    fn fill_super(
        mut data: Box<SbInfo>,
        sb: fs::NewSuperBlock<'_, Self>,
    ) -> Result<&fs::SuperBlock<Self>> {
        pr_info!("fill super");

        // obtain virtual address and size of PM device
        data.get_pm_info(&sb)?;

        // zero out PM device with non-temporal stores
        unsafe {
            memset_nt(data.get_virt_addr(), 0, data.get_size().try_into()?, true);
        }

        unsafe { init_fs(&data)? };

        // initialize superblock
        let sb = sb.init(
            data,
            &fs::SuperParams {
                magic: 0xabcdef,
                ..fs::SuperParams::DEFAULT
            },
        )?;

        let sb = sb.init_root()?;
        Ok(sb)
    }
}

/// # Safety
/// This function is intentionally unsafe. It needs to be modified once the safe persistent object
/// APIs are in place
/// TODO: make safe
unsafe fn init_fs(sbi: &SbInfo) -> Result<()> {
    pr_info!("init fs\n");

    unsafe {
        let root_ino = HayleyFsInode::init_root_inode(sbi)?;
        let super_block = HayleyFsSuperBlock::init_super_block(sbi);

        flush_buffer(root_ino, INODE_SIZE, false);
        flush_buffer(super_block, SB_SIZE, true);
    }

    Ok(())
}
