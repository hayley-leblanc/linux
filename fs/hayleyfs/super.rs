// SPDX-License-Identifier: GPL-2.0

//! Rust file system sample.

use core::{ffi, ptr};
use kernel::prelude::*;
use kernel::{bindings, c_str, fs};

module_fs! {
    type: HayleyFs,
    name: "hayley_fs",
    author: "Hayley LeBlanc",
    description: "hayley_fs",
    license: "GPL",
}

struct HayleyFs;

/// A volatile structure containing information about the file system superblock.
///
/// It uses typestates to ensure callers use the right sequence of calls.
///
/// # Invariants
/// `dax_dev` is the only active pointer to the dax device in use.
#[repr(C)]
pub(crate) struct SbInfo {
    pub(crate) dax_dev: *mut bindings::dax_device,
    pub(crate) virt_addr: *mut ffi::c_void,
    pub(crate) size: i64,
    // TODO: should this have a reference to the real SB?
    // would that break an invariant somewhere?
}

impl SbInfo {
    fn new() -> Self {
        SbInfo {
            dax_dev: ptr::null_mut(),
            virt_addr: ptr::null_mut(),
            size: 0,
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

    fn fill_super(
        mut data: Box<SbInfo>,
        sb: fs::NewSuperBlock<'_, Self>,
    ) -> Result<&fs::SuperBlock<Self>> {
        pr_info!("fill super");
        // obtain virtual address and size of PM device
        // TODO: does the benefit of typestate on SbInfo outweigh
        // the cost of allocating new space for the updated obj?
        data.get_pm_info(&sb)?;
        let sb = sb.init(
            data,
            &fs::SuperParams {
                magic: 0x72757374,
                ..fs::SuperParams::DEFAULT
            },
        )?;
        let sb = sb.init_root()?;
        Ok(sb)
    }
}
