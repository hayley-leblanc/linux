use crate::balloc::*;
use crate::h_inode::*;
use crate::volatile::*;
use core::{ffi, ptr};
use kernel::bindings;
use kernel::prelude::*;

/// Reserved inodes
pub(crate) const ROOT_INO: InodeNum = 1;

/// Type definitions
pub(crate) type InodeNum = u64;
pub(crate) type PageNum = u64;

pub(crate) const MAX_FILENAME_LEN: usize = 64; // TODO: increase
pub(crate) const NUM_INODES: u64 = 64; // max inodes in the FS
pub(crate) const MAX_PAGES: u64 = 64; // TODO: remove (or make much bigger)
pub(crate) const DENTRIES_PER_PAGE: usize = 16; // TODO: update with true dentry size

/// Reserved pages
/// TODO: update these
#[allow(dead_code)]
pub(crate) const SB_PAGE: PageNum = 0;
#[allow(dead_code)]
pub(crate) const INO_PAGE_START: PageNum = 1;
pub(crate) const DATA_PAGE_START: PageNum = 4;

/// Sizes of persistent objects
/// Update these if they get bigger or are permanently smaller
pub(crate) const INODE_SIZE: usize = 64;
pub(crate) const SB_SIZE: usize = 64;

#[repr(C)]
#[allow(dead_code)]
pub(crate) enum PageType {
    DIR,
    DATA,
}

/// Persistent super block
/// TODO: add stuff
#[repr(C)]
pub(crate) struct HayleyFsSuperBlock {
    size: i64,
}

impl HayleyFsSuperBlock {
    // pub(crate) unsafe fn init_super_block(sbi: &SbInfo) -> &HayleyFsSuperBlock {
    pub(crate) unsafe fn init_super_block(
        virt_addr: *mut ffi::c_void,
        size: i64,
    ) -> &'static HayleyFsSuperBlock {
        // we already zeroed out the entire device, so no need to zero out the superblock
        let super_block = unsafe { &mut *(virt_addr as *mut HayleyFsSuperBlock) };
        super_block.size = size;
        super_block
    }
}

/// A volatile structure containing information about the file system superblock.
///
/// It uses typestates to ensure callers use the right sequence of calls.
///
/// # Invariants
/// `dax_dev` is the only active pointer to the dax device in use.
#[repr(C)]
pub(crate) struct SbInfo {
    // FIXME: these should really not be public, but we need to define SbInfo
    // in defs.rs so that it's accessible in other files, but SbInfo's
    // impl depends on HayleyFs definition and fs::Type impl, which logically
    // needs to live in super.rs. These fields aren't available for methods impled
    // in super.rs. Maybe could do something smart with modules or traits to fix this?
    // make get_pm_info (the problematic function right now) part of a trait that SbInfo
    // implements in super.rs? That would probably be a good idea regardless because
    // CXL PM might be obtained in a different way.
    sb: *mut bindings::super_block,
    dax_dev: *mut bindings::dax_device,
    virt_addr: *mut ffi::c_void,
    pub(crate) size: i64,

    // volatile index structures
    // these should really be trait objects,
    // but writing it this way would cause SbInfo to be !Sized which causes
    // all kinds of problems elsewhere. Next best solution is to manually make
    // sure that each field's type implements the associated trait.
    // TODO: fix this.
    pub(crate) ino_dentry_map: BasicInoDentryMap, // InoDentryMap
    pub(crate) ino_dir_page_map: BasicInoDirPageMap, // InoDirPageMap

    // volatile allocators
    // again, these should really be trait objects, but the system won't compile
    // if they are.
    // TODO: fix this.
    pub(crate) page_allocator: BasicPageAllocator,
    pub(crate) inode_allocator: BasicInodeAllocator,
}

// SbInfo must be Send and Sync for it to be used as the Context's data.
// However, raw pointers are not Send or Sync because they are not safe to
// access across threads. This is a lint - they aren't safe to access within a
// single thread either - and we know that the raw pointer will be immutable,
// so it's ok to mark it Send + Sync here
unsafe impl Send for SbInfo {}
unsafe impl Sync for SbInfo {}

impl SbInfo {
    // TODO: the constructor should either not leave a bunch of pointers NULL
    // or it should make it clear in its name that that is what it does
    pub(crate) fn new() -> Self {
        SbInfo {
            sb: ptr::null_mut(),
            dax_dev: ptr::null_mut(),
            virt_addr: ptr::null_mut(),
            size: 0, // total size of the PM device
            ino_dentry_map: InoDentryMap::new(),
            ino_dir_page_map: InoDirPageMap::new(),
            page_allocator: PageAllocator::new(DATA_PAGE_START),
            inode_allocator: InodeAllocator::new(ROOT_INO + 1),
        }
    }

    pub(crate) fn get_size(&self) -> i64 {
        self.size
    }

    /// obtaining the virtual address is safe - dereferencing it is not
    pub(crate) fn get_virt_addr(&self) -> *mut ffi::c_void {
        self.virt_addr
    }

    pub(crate) unsafe fn set_virt_addr(&mut self, virt_addr: *mut ffi::c_void) {
        self.virt_addr = virt_addr;
    }

    pub(crate) unsafe fn set_dax_dev(&mut self, dax_dev: *mut bindings::dax_device) {
        self.dax_dev = dax_dev;
    }

    pub(crate) unsafe fn get_inode_by_ino(&self, ino: InodeNum) -> Result<&mut HayleyFsInode> {
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
