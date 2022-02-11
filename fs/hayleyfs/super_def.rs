use kernel::bindings::{dax_device, super_block};
use kernel::c_types::c_void;

#[repr(C)]
#[derive(Debug)]
pub(crate) struct hayleyfs_mount_opts {
    pub(crate) init: bool,
}

// TODO: packed?
#[derive(Debug)]
pub(crate) struct hayleyfs_super_block {
    pub(crate) size: u64,
    pub(crate) blocksize: u32,
    pub(crate) magic: u32,
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct hayleyfs_sb_info {
    pub(crate) sb: *mut super_block, // raw pointer to the VFS super block
    pub(crate) hayleyfs_sb: hayleyfs_super_block,
    pub(crate) mount_opts: hayleyfs_mount_opts,
    pub(crate) s_daxdev: *mut dax_device, // raw pointer to the dax device we are mounted on
    pub(crate) s_dev_offset: u64,         // no idea what this is used for but a dax fxn needs it
    pub(crate) virt_addr: *mut c_void,    // raw pointer virtual address of beginning of FS instance
    pub(crate) phys_addr: u64,            // physical address of beginning of FS instance
    pub(crate) pm_size: u64,              // size of the PM device (TODO: make unsigned)
}
