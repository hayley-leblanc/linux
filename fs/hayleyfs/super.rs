// SPDX-License-Identifier: GPL-2.0

//! Rust file system sample.

use core::{ffi, ptr};
use defs::*;
use h_inode::*;
use kernel::prelude::*;
use kernel::{bindings, c_str, fs, PAGE_SIZE};
use namei::*;
use pm::*;

mod balloc;
mod defs;
mod dir;
mod h_file;
mod h_inode;
mod namei;
mod pm;
mod typestate;
mod volatile;

module_fs! {
    type: HayleyFs,
    name: "hayley_fs",
    author: "Hayley LeBlanc",
    description: "hayley_fs",
    license: "GPL",
}

struct HayleyFs;

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
    type InodeOps = InodeOps;
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

        unsafe { init_fs(&mut data)? };

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

    fn statfs(sb: &fs::SuperBlock<Self>, buf: *mut bindings::kstatfs) -> Result<()> {
        pr_info!("statfs\n");
        // TODO: better support in rust/ so we don't have to do this all via raw pointers
        let sbi = unsafe { &*(sb.s_fs_info() as *const SbInfo) };
        unsafe {
            (*buf).f_type = SUPER_MAGIC;
            (*buf).f_bsize = sbi.blocksize;
            (*buf).f_blocks = sbi.num_blocks;
            pr_info!("num blocks: {:?}\n", sbi.num_blocks);
            pr_info!("fs size: {:?}\n", sbi.size);
            pr_info!("block size {:?}\n", sbi.blocksize);
            (*buf).f_bfree = sbi.num_blocks - sbi.get_pages_in_use();
            (*buf).f_bavail = sbi.num_blocks - sbi.get_pages_in_use();
            (*buf).f_files = NUM_INODES;
            (*buf).f_ffree = NUM_INODES - sbi.get_inodes_in_use();
            (*buf).f_namelen = MAX_FILENAME_LEN.try_into()?;
        }

        Ok(())
    }
}

/// # Safety
/// This function is intentionally unsafe. It needs to be modified once the safe persistent object
/// APIs are in place
/// TODO: make safe
unsafe fn init_fs(sbi: &mut SbInfo) -> Result<()> {
    pr_info!("init fs\n");

    unsafe {
        let root_ino = HayleyFsInode::init_root_inode(sbi)?;
        // let super_block = HayleyFsSuperBlock::init_super_block(sbi);
        let super_block = HayleyFsSuperBlock::init_super_block(sbi.get_virt_addr(), sbi.get_size());

        flush_buffer(root_ino, INODE_SIZE, false);
        flush_buffer(super_block, SB_SIZE, true);
    }

    // let _iops = unsafe { inode::OperationsVtable::<InodeData>::build() };

    Ok(())
}

pub(crate) trait PmDevice {
    fn get_pm_info(&mut self, sb: &fs::NewSuperBlock<'_, HayleyFs>) -> Result<()>;
}

impl PmDevice for SbInfo {
    fn get_pm_info(&mut self, sb: &fs::NewSuperBlock<'_, HayleyFs>) -> Result<()> {
        // obtain the dax_device struct
        let dax_dev = sb.get_dax_dev()?;

        let mut virt_addr: *mut ffi::c_void = ptr::null_mut();

        // obtain virtual address and size of the dax device
        // SAFETY: The type invariant of `sb` guarantees that `sb.sb` is the only pointer to
        // a newly-allocated superblock. The safety condition of `get_dax_dev` guarantees
        // that `dax_dev` is the only active pointer to the associated `dax_device`, so it is
        // safe to mutably dereference it.
        let num_blocks = unsafe {
            bindings::dax_direct_access(
                dax_dev,
                0,
                (usize::MAX / kernel::PAGE_SIZE).try_into()?,
                bindings::dax_access_mode_DAX_ACCESS,
                &mut virt_addr,
                ptr::null_mut(),
            )
        };

        unsafe {
            self.set_dax_dev(dax_dev);
            self.set_virt_addr(virt_addr);
        }
        let pgsize_i64: i64 = PAGE_SIZE.try_into()?;
        self.size = num_blocks * pgsize_i64;
        self.num_blocks = num_blocks.try_into()?;

        Ok(())
    }
}
