use kernel::bindings::{dax_device, kgid_t, kuid_t, super_block, umode_t};
use kernel::c_types::c_void;
use kernel::PAGE_SIZE;

use crate::defs::*;
use crate::inode_rs::*;

// TODO: packed?
#[repr(packed)]
pub(crate) struct hayleyfs_super_block {
    pub(crate) size: u64,
    pub(crate) blocksize: u32,
    pub(crate) magic: u32,
}

#[repr(C)]
pub(crate) struct SbInfo {
    pub(crate) sb: *mut super_block, // raw pointer to the VFS super block
    pub(crate) hayleyfs_sb: hayleyfs_super_block,
    pub(crate) s_daxdev: *mut dax_device, // raw pointer to the dax device we are mounted on
    pub(crate) s_dev_offset: u64,         // no idea what this is used for but a dax fxn needs it
    pub(crate) virt_addr: *mut c_void,    // raw pointer virtual address of beginning of FS instance
    pub(crate) phys_addr: u64,            // physical address of beginning of FS instance
    pub(crate) pm_size: u64,              // size of the PM device (TODO: make unsigned)
    pub(crate) uid: kuid_t,
    pub(crate) gid: kgid_t,
    pub(crate) mode: umode_t,
}

// TODO: do CacheLine and PersistentBitmap have to be packed?
// p sure Rust makes arrays contiguous so they shouldn't need to be
// compiler warning indicates making them packed could have weird consequences
pub(crate) struct CacheLine {
    pub(crate) bits: [u64; 8],
}

pub(crate) struct PersistentBitmap {
    pub(crate) bits: [CacheLine; PAGE_SIZE / CACHELINE_SIZE],
}

pub(crate) fn get_bitmap_cacheline(bitmap: &mut PersistentBitmap, index: usize) -> *mut CacheLine {
    let cacheline_num = index >> 6;
    &mut bitmap.bits[cacheline_num] as *mut _ as *mut CacheLine
}

// probably shouldn't return with a static lifetime
pub(crate) fn hayleyfs_get_sbi(sb: *mut super_block) -> &'static mut SbInfo {
    let sbi: &mut SbInfo = unsafe { &mut *((*sb).s_fs_info as *mut SbInfo) };
    sbi
}
