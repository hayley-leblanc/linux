#![allow(dead_code)] // TODO: remove

use crate::{dir::*, file::*, inode::*};
use core::{ffi, fmt, ptr};
use kernel::prelude::*;
use kernel::{fs, PAGE_SIZE};

pub(crate) struct HayleyFS {}
pub(crate) type INodeData = ();

pub(crate) const ROOT_INO: InodeNum = 1;
pub(crate) const SUPER_BLOCK_PAGE: usize = 0; // currently unused
pub(crate) const INODE_TABLE_START: usize = 1;
pub(crate) const INODE_TABLE_SIZE: usize = 4; // pages of inode table
                                              // the rest of the PM is for data pages
pub(crate) const INODE_ENTRY_SIZE: usize = 64; // until we land on an inode structure, use this to index into the inode table
pub(crate) const TOTAL_INODES: usize = (PAGE_SIZE * INODE_TABLE_SIZE) / INODE_ENTRY_SIZE;
pub(crate) const INODE_BITMAP_SIZE: usize = TOTAL_INODES / 8;
pub(crate) const PAGE_BITMAP_SIZE: usize = (1024 * 1024 * 1024) / (4 * 1024); // sized for 1GB PM device, 4KB pages
                                                                              // TODO: don't statically size the page bitmap (or don't use a bitmap)

// we could make SbInfo generic in the allocator and index types, but
// SbInfo is passed around a lot and converted from a raw pointer to a nice
// type very frequently, so this will get messy. Use type aliases instead to
// make it easy to switch out allocator/index types.
type InodeAlloc = InodeBitmap; // should implement InodeAllocator trait
type PageAlloc = PageBitmap; // should implement PageAllocator trait
type DirIndex = RBVecDirTree; // should implement DirectoryIndex trait
type DataIndex = RBDataTree; // should implement DataIndex trait

/// TODO: make SbInfo generic in the type of allocators/indexes?
#[repr(C)]
pub(crate) struct SbInfo {
    pub(crate) virt_addr: *mut ffi::c_void,
    pub(crate) pm_size: i64,
    pub(crate) inode_allocator: Box<InodeAlloc>,
    pub(crate) page_allocator: Box<PageAlloc>,
    pub(crate) dir_index: Box<DirIndex>,
    pub(crate) data_index: Box<DataIndex>,
}

pub(crate) type InodeNum = u64;

// raw pointers are marked unsafe mainly as a lint; they are unsafe to access
// regardless of whether they're being sent across threads. we need to be able
// to send SbInfo across threads but it will be immutable after mount, so
// it's okay to mark it Send + Sync here
unsafe impl Send for SbInfo {}
unsafe impl Sync for SbInfo {}

impl SbInfo {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            virt_addr: ptr::null_mut(),
            pm_size: 0,
            inode_allocator: Box::try_new(InodeAlloc::new()?)?,
            page_allocator: Box::try_new(PageAlloc::new()?)?,
            dir_index: Box::try_new(DirIndex::new())?,
            data_index: Box::try_new(DataIndex::new())?,
        })
    }

    pub(crate) fn get_pm_info(
        &mut self,
        sb: &mut fs::NewSuperBlock<'_, HayleyFS, fs::NeedsInit>,
    ) -> Result<()> {
        let (pm_virt_addr, size) = sb.get_dax()?;
        self.virt_addr = pm_virt_addr;
        self.pm_size = size;
        Ok(())
    }

    /// Safety: this should not be used in most cases. We should almost never
    /// use a raw pointer to modify PM directly.
    pub(crate) unsafe fn danger_get_pm_addr(&self) -> *mut core::ffi::c_void {
        self.virt_addr
    }
}

impl fmt::Debug for SbInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "virt_addr: {:p}, pm_size: {:?}",
            self.virt_addr, self.pm_size
        )
    }
}

// ZSTs for typestate
// Persistence state
pub(crate) struct Dirty {}
pub(crate) struct InFlight {}
pub(crate) struct Clean {}
// Update state
pub(crate) struct Alloc {}
pub(crate) struct Free {}
