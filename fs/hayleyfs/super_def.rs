use kernel::bindings::{dax_device, super_block};
use kernel::c_types::c_void;

#[repr(C)]
#[derive(Debug)]
pub(crate) struct hayleyfs_mount_opts {
    pub(crate) init: bool,
}

// TODO: packed?
#[repr(packed)]
pub(crate) struct hayleyfs_super_block {
    pub(crate) size: u64,
    pub(crate) blocksize: u32,
    pub(crate) magic: u32,
}

#[repr(C)]
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

// probably shouldn't return with a static lifetime
pub(crate) fn hayleyfs_get_sbi(sb: *mut super_block) -> &'static mut hayleyfs_sb_info {
    let sbi: &mut hayleyfs_sb_info = unsafe { &mut *((*sb).s_fs_info as *mut hayleyfs_sb_info) };
    sbi
}
