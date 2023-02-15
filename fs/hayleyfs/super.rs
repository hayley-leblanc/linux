// SPDX-License-Identifier: GPL-2.0

//! Rust file system sample.

use balloc::*;
use core::{ffi, ptr};
use defs::*;
use h_inode::*;
use kernel::prelude::*;
use kernel::{bindings, c_str, fs, rbtree::RBTree};
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

    kernel::define_fs_params! {Box<SbInfo>,
        {flag, "init", |s, v| {s.mount_opts.init = Some(v); Ok(())}},
    }

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
        pr_info!("fill super\n");

        // obtain virtual address and size of PM device
        data.get_pm_info(&sb)?;

        if let Some(true) = data.mount_opts.init {
            // initialize the file system
            // zero out PM device with non-temporal stores
            pr_info!("initializing file system...\n");

            unsafe { init_fs(&mut data)? };
        } else {
            // remount
            pr_info!("mounting existing file system...\n");
            remount_fs(&mut data)?;
        }

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
            (*buf).f_bsize = sbi.blocksize.try_into()?;
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
        memset_nt(
            sbi.get_virt_addr() as *mut ffi::c_void,
            0,
            sbi.get_size().try_into()?,
            true,
        );

        let root_ino = HayleyFsInode::init_root_inode(sbi)?;
        let super_block = HayleyFsSuperBlock::init_super_block(sbi.get_virt_addr(), sbi.get_size());

        flush_buffer(root_ino, INODE_SIZE.try_into()?, false);
        flush_buffer(super_block, SB_SIZE.try_into()?, true);
    }

    Ok(())
}

fn remount_fs(sbi: &mut SbInfo) -> Result<()> {
    let mut alloc_inode_vec: Vec<InodeNum> = Vec::new();
    let mut alloc_page_vec: Vec<PageNum> = Vec::new();
    let mut init_dir_pages: RBTree<InodeNum, Vec<PageNum>> = RBTree::new();
    let mut live_inode_vec: Vec<InodeNum> = Vec::new();
    let mut processed_live_inodes: RBTree<InodeNum, ()> = RBTree::new(); // rbtree as a set

    live_inode_vec.try_push(1)?;

    // 1. check the super block to make sure it is a valid fs and to fill in sbi
    let _sb = sbi.get_super_block()?;

    // 2. scan the inode table to determine which inodes are allocated
    // TODO: this scan will change significantly if the inode table is ever
    // not a single contiguous array
    let inode_table = sbi.get_inode_table()?;

    for inode in inode_table {
        if !inode.is_free() {
            alloc_inode_vec.try_push(inode.get_ino())?;
        }
    }
    pr_info!("allocated inodes: {:?}\n", alloc_inode_vec);

    // 3. scan the page descriptor table to determine which pages are live
    let page_desc_table = sbi.get_page_desc_table()?;
    for (i, desc) in page_desc_table.iter().enumerate() {
        if !desc.is_free() {
            let index: u64 = i.try_into()?;
            if desc.get_page_type() == PageType::DIR {
                let dir_desc: &DirPageHeader = desc.try_into()?;
                if dir_desc.is_initialized() {
                    let parent = dir_desc.get_ino();
                    if let Some(node) = init_dir_pages.get_mut(&parent) {
                        node.try_push(index + DATA_PAGE_START)?;
                    } else {
                        let mut vec = Vec::new();
                        vec.try_push(index + DATA_PAGE_START)?;
                        init_dir_pages.try_insert(parent, vec)?;
                    }
                }
            }
            alloc_page_vec.try_push(index + DATA_PAGE_START)?;
        }
    }
    pr_info!("allocated pages: {:?}\n", alloc_page_vec);

    // 4. scan the directory entries in live pages to determine which inodes are live

    while !live_inode_vec.is_empty() {
        let live_inode = live_inode_vec.pop().unwrap();
        let owned_dir_pages = init_dir_pages.get(&live_inode);
        pr_info!("live inode: {:?}\n", live_inode);
        pr_info!("pages owned by inode: {:?}\n", owned_dir_pages);

        // iterate over pages owned by this inode, find valid dentries in those
        // pages, and add their inodes to the live inode list
        if let Some(pages) = owned_dir_pages {
            for page in pages {
                // TODO: figure out safest way to get the dir page
                let dir_page_wrapper = DirPageWrapper::from_page_no(sbi, *page)?;
                let live_inodes = dir_page_wrapper.get_live_inodes(sbi);
                pr_info!("live inodes: {:?}\n", live_inodes);
            }
        }

        processed_live_inodes.try_insert(live_inode, ())?;
    }

    // TODO: fill in inodes_in_use
    // TODO: fill in blocks_in_use
    // ^^ these two are based on real usage, not live objects
    // TODO: fill in ino_dentry_map
    // TODO: fill in ino_dir_page_map
    // TODO: fill in ino_data_page_map
    // TODO: set up page_allocator
    // TODO: set up inode_allocator

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
                (u64::MAX / HAYLEYFS_PAGESIZE).try_into()?,
                bindings::dax_access_mode_DAX_ACCESS,
                &mut virt_addr,
                ptr::null_mut(),
            )
        };

        unsafe {
            self.set_dax_dev(dax_dev);
            self.set_virt_addr(virt_addr as *mut u8);
        }
        let pgsize_i64: i64 = HAYLEYFS_PAGESIZE.try_into()?;
        self.size = num_blocks * pgsize_i64;
        self.num_blocks = num_blocks.try_into()?;

        Ok(())
    }
}
