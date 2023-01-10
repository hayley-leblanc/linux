use core::ffi;
use kernel::bindings;

/// Reserved inodes
pub(crate) const ROOT_INO: InodeNum = 1;

/// Type definitions
pub(crate) type InodeNum = u64;
pub(crate) type PageNum = u64;

pub(crate) const NUM_INODES: u64 = 64; // max inodes in the FS
pub(crate) const MAX_PAGES: u64 = 64; // TODO: remove (or make much bigger)

/// Sizes of persistent objects
/// Update these if they get bigger or are permanently smaller
pub(crate) const INODE_SIZE: usize = 64;
pub(crate) const SB_SIZE: usize = 64;

/// Persistent super block
/// TODO: add stuff
#[repr(C)]
pub(crate) struct HayleyFsSuperBlock {
    size: i64,
}

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
}

impl HayleyFsSuperBlock {
    pub(crate) unsafe fn init_super_block(sbi: &SbInfo) -> &HayleyFsSuperBlock {
        // we already zeroed out the entire device, so no need to zero out the superblock
        let super_block = unsafe { &mut *(sbi.get_virt_addr() as *mut HayleyFsSuperBlock) };
        super_block.size = sbi.get_size();
        super_block
    }
}
