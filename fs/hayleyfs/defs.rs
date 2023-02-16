use crate::balloc::*;
use crate::h_inode::*;
use crate::typestate::*;
use crate::volatile::*;
use core::{
    ptr, slice,
    sync::atomic::{AtomicU64, Ordering},
};
use kernel::bindings;
use kernel::prelude::*;

// TODO: different magic value
pub(crate) const SUPER_MAGIC: i64 = 0xabcdef;

/// Reserved inodes
pub(crate) const ROOT_INO: InodeNum = 1;

/// Type definitions
pub(crate) type InodeNum = u64;
pub(crate) type PageNum = u64;

pub(crate) const HAYLEYFS_PAGESIZE: u64 = 4096;

pub(crate) const MAX_FILENAME_LEN: usize = 64; // TODO: increase
pub(crate) const NUM_INODES: u64 = INODE_TABLE_SIZE / INODE_SIZE; // max inodes in the FS
pub(crate) const MAX_PAGES: u64 = u64::MAX;
pub(crate) const MAX_LINKS: u16 = u16::MAX;
pub(crate) const DENTRIES_PER_PAGE: usize = 16; // TODO: update with true dentry size
pub(crate) const INODE_TABLE_SIZE: u64 = 1024 * 1024 * 2; // 2MB
pub(crate) const NUM_INODE_PAGES: u64 = INODE_TABLE_SIZE / HAYLEYFS_PAGESIZE;
pub(crate) const DESCRIPTOR_TABLE_SIZE: u64 = 1024 * 1024 * 2; // 2MB
pub(crate) const NUM_DESCRIPTOR_TABLE_PAGES: u64 = DESCRIPTOR_TABLE_SIZE / HAYLEYFS_PAGESIZE;
pub(crate) const NUM_PAGE_DESCRIPTORS: u64 = DESCRIPTOR_TABLE_SIZE / PAGE_DESCRIPTOR_SIZE;

/// Reserved pages
/// TODO: update these
#[allow(dead_code)]
pub(crate) const SB_PAGE: PageNum = 0;
#[allow(dead_code)]
pub(crate) const INO_PAGE_START: PageNum = 1;
pub(crate) const PAGE_DESCRIPTOR_TABLE_START: PageNum = INO_PAGE_START + NUM_INODE_PAGES;
pub(crate) const DATA_PAGE_START: PageNum =
    PAGE_DESCRIPTOR_TABLE_START + NUM_DESCRIPTOR_TABLE_PAGES;

/// Sizes of persistent objects
/// Update these if they get bigger or are permanently smaller
pub(crate) const INODE_SIZE: u64 = 32;
pub(crate) const PAGE_DESCRIPTOR_SIZE: u64 = 32; // TODO: can we reduce this?
pub(crate) const SB_SIZE: u64 = HAYLEYFS_PAGESIZE;

#[repr(C)]
#[allow(dead_code)]
#[derive(PartialEq, Copy, Clone, Debug)]
pub(crate) enum PageType {
    NONE = 0,
    DIR,
    DATA,
}

#[repr(C)]
#[allow(dead_code)]
#[derive(PartialEq, Debug, Copy, Clone)]
pub(crate) enum InodeType {
    NONE = 0,
    REG,
    DIR,
}
/// Persistent super block
/// TODO: add stuff
#[repr(C)]
pub(crate) struct HayleyFsSuperBlock {
    magic: i64,
    block_size: u64,
    size: i64,
    // TODO: mount and write timestamps
    // TODO: make sure remounted file systems use the page size specified in the superblock
}

impl HayleyFsSuperBlock {
    pub(crate) unsafe fn init_super_block(
        virt_addr: *mut u8,
        size: i64,
    ) -> &'static HayleyFsSuperBlock {
        // we already zeroed out the entire device, so no need to zero out the superblock
        let super_block = unsafe { &mut *(virt_addr as *mut HayleyFsSuperBlock) };
        super_block.size = size;
        super_block.magic = SUPER_MAGIC;
        super_block.block_size = HAYLEYFS_PAGESIZE;
        super_block
    }
}

#[derive(Default)]
pub(crate) struct HayleyfsParams {
    pub(crate) init: Option<bool>,
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
    virt_addr: *mut u8,
    pub(crate) size: i64,

    pub(crate) blocksize: u64,
    pub(crate) num_blocks: u64,

    pub(crate) inodes_in_use: AtomicU64,
    pub(crate) blocks_in_use: AtomicU64,

    // volatile index structures
    // these should really be trait objects,
    // but writing it this way would cause SbInfo to be !Sized which causes
    // all kinds of problems elsewhere. Next best solution is to manually make
    // sure that each field's type implements the associated trait.
    // TODO: fix this.
    pub(crate) ino_dentry_map: BasicInoDentryMap, // InoDentryMap
    pub(crate) ino_dir_page_map: BasicInoDirPageMap, // InoDirPageMap
    pub(crate) ino_data_page_map: BasicInoDataPageMap, // InoDataPageMap

    // volatile allocators
    // again, these should really be trait objects, but the system won't compile
    // if they are.
    // TODO: fix this.
    pub(crate) page_allocator: BasicPageAllocator,
    pub(crate) inode_allocator: BasicInodeAllocator,

    pub(crate) mount_opts: HayleyfsParams,
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
            blocksize: HAYLEYFS_PAGESIZE.try_into().unwrap(),
            num_blocks: 0,
            inodes_in_use: AtomicU64::new(1),
            blocks_in_use: AtomicU64::new(0), // TODO: mark reserved pages as in use
            ino_dentry_map: InoDentryMap::new().unwrap(), // TODO: handle possible panic
            ino_dir_page_map: InoDirPageMap::new().unwrap(), // TODO: handle possible panic
            ino_data_page_map: InoDataPageMap::new().unwrap(), // TODO: handle possible panic
            page_allocator: PageAllocator::new(DATA_PAGE_START),
            inode_allocator: InodeAllocator::new(ROOT_INO + 1),
            mount_opts: HayleyfsParams::default(),
        }
    }

    pub(crate) fn inc_inodes_in_use(&self) {
        self.inodes_in_use.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn get_inodes_in_use(&self) -> u64 {
        self.inodes_in_use.load(Ordering::SeqCst)
    }

    pub(crate) fn inc_blocks_in_use(&self) {
        self.blocks_in_use.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn get_pages_in_use(&self) -> u64 {
        self.blocks_in_use.load(Ordering::SeqCst)
    }

    pub(crate) fn get_size(&self) -> i64 {
        self.size
    }

    /// obtaining the virtual address is safe - dereferencing it is not
    pub(crate) fn get_virt_addr(&self) -> *mut u8 {
        self.virt_addr
    }

    pub(crate) unsafe fn set_virt_addr(&mut self, virt_addr: *mut u8) {
        self.virt_addr = virt_addr;
    }

    pub(crate) unsafe fn set_dax_dev(&mut self, dax_dev: *mut bindings::dax_device) {
        self.dax_dev = dax_dev;
    }

    pub(crate) fn get_super_block(&mut self) -> Result<&HayleyFsSuperBlock> {
        let super_block = unsafe { &mut *(self.virt_addr as *mut HayleyFsSuperBlock) };
        // assume for now that if the magic is fine, the rest of the super block is fine
        if super_block.magic != SUPER_MAGIC {
            pr_err!(
                "Magic should be {:?} but found {:?}\n",
                SUPER_MAGIC,
                super_block.magic
            );
            return Err(EINVAL);
        }
        if super_block.size != self.size {
            pr_err!(
                "Device size should be {:?} but found {:?}\n",
                self.size,
                super_block.size
            );
            return Err(EINVAL);
        }
        self.blocksize = super_block.block_size;
        Ok(super_block)
    }

    pub(crate) fn get_page_desc_table<'a>(&self) -> Result<&'a mut [PageDescriptor]> {
        let page_desc_table_addr = unsafe {
            self.virt_addr
                .offset((HAYLEYFS_PAGESIZE * PAGE_DESCRIPTOR_TABLE_START).try_into()?)
        } as *mut PageDescriptor;
        let table = unsafe {
            slice::from_raw_parts_mut(page_desc_table_addr, NUM_PAGE_DESCRIPTORS.try_into()?)
        };
        Ok(table)
    }

    pub(crate) fn get_inode_table<'a>(&self) -> Result<&'a mut [HayleyFsInode]> {
        let inode_table_addr: *mut HayleyFsInode = unsafe {
            self.virt_addr
                .offset((HAYLEYFS_PAGESIZE * INO_PAGE_START).try_into()?)
                as *mut HayleyFsInode
        };
        let table = unsafe { slice::from_raw_parts_mut(inode_table_addr, NUM_INODES.try_into()?) };
        Ok(table)
    }

    pub(crate) fn get_inode_by_ino<'a>(&self, ino: InodeNum) -> Result<&'a HayleyFsInode> {
        // we don't use inode 0
        if ino >= NUM_INODES || ino == 0 {
            return Err(EINVAL);
        }

        let table = self.get_inode_table()?;
        let ino_usize: usize = ino.try_into()?;
        let inode = &table[ino_usize];
        Ok(inode)
    }

    pub(crate) unsafe fn get_inode_by_ino_mut<'a>(
        &self,
        ino: InodeNum,
    ) -> Result<&'a mut HayleyFsInode> {
        // we don't use inode 0
        if ino >= NUM_INODES || ino == 0 {
            return Err(EINVAL);
        }

        let table = self.get_inode_table()?;
        let ino_usize: usize = ino.try_into()?;
        let inode = &mut table[ino_usize];
        Ok(inode)
    }

    #[allow(dead_code)]
    pub(crate) fn get_init_reg_inode_by_ino<'a>(
        &self,
        ino: InodeNum,
    ) -> Result<InodeWrapper<'a, Clean, Start, RegInode>> {
        // we don't use inode 0
        if ino >= NUM_INODES || ino == 0 {
            return Err(EINVAL);
        }

        let inode = unsafe { self.get_inode_by_ino_mut(ino)? };

        if inode.get_type() != InodeType::REG {
            pr_info!("ERROR: inode {:?} is not a regular inode\n", ino);
            return Err(EPERM);
        }
        if inode.is_initialized() {
            Ok(InodeWrapper::wrap_inode(ino, inode))
        } else {
            pr_info!("ERROR: inode {:?} is not initialized\n", ino);
            Err(EPERM)
        }
    }

    pub(crate) fn get_init_dir_inode_by_ino<'a>(
        &self,
        ino: InodeNum,
    ) -> Result<InodeWrapper<'a, Clean, Start, DirInode>> {
        // we don't use inode 0
        if ino >= NUM_INODES || ino == 0 {
            return Err(EINVAL);
        }

        let inode = unsafe { self.get_inode_by_ino_mut(ino)? };

        if inode.get_type() != InodeType::DIR {
            return Err(EPERM);
        }
        if inode.is_initialized() {
            Ok(InodeWrapper::wrap_inode(ino, inode))
        } else {
            pr_info!("ERROR: inode {:?} is not initialized\n", ino);
            Err(EPERM)
        }
    }
}
