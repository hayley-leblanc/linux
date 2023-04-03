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
use volatile::*;

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

        let sb = if let Some(true) = data.mount_opts.init {
            // initialize the file system
            // zero out PM device with non-temporal stores
            pr_info!("initializing file system...\n");

            let inode = unsafe { init_fs(&mut data, &sb)? };

            // initialize superblock
            let sb = sb.init(
                data,
                &fs::SuperParams {
                    magic: SUPER_MAGIC.try_into()?,
                    ..fs::SuperParams::DEFAULT
                },
            )?;

            sb.init_root_from_inode(inode)?
        } else {
            // remount
            pr_info!("mounting existing file system...\n");
            remount_fs(&mut data)?;

            // grab the persistent root inode up here to avoid ownership problems

            // initialize superblock
            let sb = sb.init(
                data,
                &fs::SuperParams {
                    magic: SUPER_MAGIC.try_into()?,
                    ..fs::SuperParams::DEFAULT
                },
            )?;

            let sbi = unsafe { &mut *((*sb.get_inner()).s_fs_info as *mut SbInfo) };

            let pi = sbi.get_inode_by_ino(ROOT_INO)?;

            // TODO: this is so janky. fix the kernel code so that this is cleaner
            // obtain the root inode we just created and fill it in with correct values
            // TODO: does this allocate a new inode?
            let inode = unsafe { bindings::new_inode(sb.get_inner()) };
            if inode.is_null() {
                return Err(ENOMEM);
            }

            // fill in the new raw inode with info from our persistent inode
            // TODO: safer way of doing this
            unsafe {
                (*inode).i_ino = ROOT_INO;
                (*inode).i_size = bindings::le64_to_cpu(pi.get_size()).try_into()?;
                bindings::set_nlink(inode, bindings::le16_to_cpu(pi.get_link_count()).into());
                (*inode).i_mode = bindings::le16_to_cpu(pi.get_mode());
                (*inode).i_blocks = bindings::le64_to_cpu(pi.get_blocks());
                let uid = bindings::le32_to_cpu(pi.get_uid());
                let gid = bindings::le32_to_cpu(pi.get_gid());
                // TODO: https://elixir.bootlin.com/linux/latest/source/fs/ext2/inode.c#L1395 ?
                bindings::i_uid_write(inode, uid);
                bindings::i_gid_write(inode, gid);
                (*inode).i_atime.tv_sec = bindings::le32_to_cpu(pi.get_atime()).try_into()?;
                (*inode).i_ctime.tv_sec = bindings::le32_to_cpu(pi.get_ctime()).try_into()?;
                (*inode).i_mtime.tv_sec = bindings::le32_to_cpu(pi.get_mtime()).try_into()?;
                (*inode).i_atime.tv_nsec = 0;
                (*inode).i_ctime.tv_nsec = 0;
                (*inode).i_mtime.tv_nsec = 0;
                (*inode).i_blkbits =
                    bindings::blksize_bits(sbi.blocksize.try_into()?).try_into()?;
                // TODO: set the rest of the fields!
            }

            sb.init_root_from_inode(inode)?
        };

        Ok(sb)
    }

    fn statfs(sb: &fs::SuperBlock<Self>, buf: *mut bindings::kstatfs) -> Result<()> {
        // TODO: better support in rust/ so we don't have to do this all via raw pointers
        let sbi = unsafe { &*(sb.s_fs_info() as *const SbInfo) };
        unsafe {
            (*buf).f_type = SUPER_MAGIC;
            (*buf).f_bsize = sbi.blocksize.try_into()?;
            (*buf).f_blocks = sbi.num_blocks;
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
/// TODO: should it be NeedsRoot? ownership needs work if so
unsafe fn init_fs<T: fs::Type + ?Sized>(
    sbi: &mut SbInfo,
    sb: &fs::NewSuperBlock<'_, T, fs::NeedsInit>,
) -> Result<*mut bindings::inode> {
    pr_info!("init fs\n");

    unsafe {
        memset_nt(
            sbi.get_virt_addr() as *mut ffi::c_void,
            0,
            sbi.get_size().try_into()?,
            true,
        );

        // TODO: this is so janky. fix the kernel code so that this is cleaner
        // obtain the root inode we just created and fill it in with correct values
        // TODO: does this allocate a new inode?
        let inode = bindings::new_inode(sb.get_inner());
        if inode.is_null() {
            return Err(ENOMEM);
        }

        let pi = HayleyFsInode::init_root_inode(sbi, inode)?;
        let super_block = HayleyFsSuperBlock::init_super_block(sbi.get_virt_addr(), sbi.get_size());

        flush_buffer(pi, INODE_SIZE.try_into()?, false);
        flush_buffer(super_block, SB_SIZE.try_into()?, true);

        // fill in the new raw inode with info from our persistent inode
        // TODO: safer way of doing this
        (*inode).i_ino = ROOT_INO;
        (*inode).i_size = bindings::le64_to_cpu(pi.get_size()).try_into()?;
        bindings::set_nlink(inode, bindings::le16_to_cpu(pi.get_link_count()).into());
        (*inode).i_mode = bindings::le16_to_cpu(pi.get_mode());
        (*inode).i_blocks = bindings::le64_to_cpu(pi.get_blocks());
        let uid = bindings::le32_to_cpu(pi.get_uid());
        let gid = bindings::le32_to_cpu(pi.get_gid());
        // TODO: https://elixir.bootlin.com/linux/latest/source/fs/ext2/inode.c#L1395 ?
        bindings::i_uid_write(inode, uid);
        bindings::i_gid_write(inode, gid);
        (*inode).i_atime.tv_sec = bindings::le32_to_cpu(pi.get_atime()).try_into()?;
        (*inode).i_ctime.tv_sec = bindings::le32_to_cpu(pi.get_ctime()).try_into()?;
        (*inode).i_mtime.tv_sec = bindings::le32_to_cpu(pi.get_mtime()).try_into()?;
        (*inode).i_atime.tv_nsec = 0;
        (*inode).i_ctime.tv_nsec = 0;
        (*inode).i_mtime.tv_nsec = 0;
        (*inode).i_blkbits = bindings::blksize_bits(sbi.blocksize.try_into()?).try_into()?;
        // TODO: set the rest of the fields!

        Ok(inode)
    }
}

fn remount_fs(sbi: &mut SbInfo) -> Result<()> {
    let mut alloc_inode_vec: Vec<InodeNum> = Vec::new();
    let mut alloc_page_vec: Vec<PageNum> = Vec::new(); // TODO: do we use this?
    let mut init_dir_pages: RBTree<InodeNum, Vec<PageNum>> = RBTree::new();
    let mut init_data_pages: RBTree<InodeNum, Vec<PageNum>> = RBTree::new();
    let mut live_inode_vec: Vec<InodeNum> = Vec::new();
    let mut processed_live_inodes: RBTree<InodeNum, ()> = RBTree::new(); // rbtree as a set

    // keeps track of maximum inode/page number in use to recreate the allocator
    // TODO: this will have to change when inodes/pages can be reused
    let mut max_inode = 0;
    let mut max_page = DATA_PAGE_START;

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
            sbi.inc_inodes_in_use();
        }
    }
    pr_info!("allocated inodes: {:?}\n", alloc_inode_vec);

    // 3. scan the page descriptor table to determine which pages are in use
    let page_desc_table = sbi.get_page_desc_table()?;
    for (i, desc) in page_desc_table.iter().enumerate() {
        if !desc.is_free() {
            if i > max_page.try_into()? {
                max_page = i.try_into()?;
            }
            sbi.inc_blocks_in_use();
            let index: u64 = i.try_into()?;
            // add pages to maps that associate inodes with the pages they own
            // we don't add them to the index yet because an initialized page
            // is not necessarily live (right?)
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
            } else if desc.get_page_type() == PageType::DATA {
                let data_desc: &DataPageHeader = desc.try_into()?;
                if data_desc.is_initialized() {
                    let parent = data_desc.get_ino();
                    if let Some(node) = init_data_pages.get_mut(&parent) {
                        node.try_push(index + DATA_PAGE_START)?;
                    } else {
                        let mut vec = Vec::new();
                        vec.try_push(index + DATA_PAGE_START)?;
                        init_data_pages.try_insert(parent, vec)?;
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
        if live_inode > max_inode {
            max_inode = live_inode;
        }
        let owned_dir_pages = init_dir_pages.get(&live_inode);
        let owned_data_pages = init_data_pages.get(&live_inode);
        pr_info!("live inode: {:?}\n", live_inode);
        pr_info!("dir pages owned by inode: {:?}\n", owned_dir_pages);
        pr_info!("data pages owned by inode: {:?}\n", owned_data_pages);

        // iterate over pages owned by this inode, find valid dentries in those
        // pages, and add their inodes to the live inode list. also add the dir pages
        // to the volatile index
        if let Some(pages) = owned_dir_pages {
            for page in pages {
                let dir_page_wrapper = DirPageWrapper::from_page_no(sbi, *page)?;
                let live_dentries = dir_page_wrapper.get_live_dentry_info(sbi)?;
                pr_info!("live dentries: {:?}\n", live_dentries);
                // add these live dentries to the index
                for dentry in live_dentries {
                    sbi.ino_dentry_map.insert(live_inode, dentry)?;
                }

                sbi.ino_dir_page_map.insert(live_inode, &dir_page_wrapper)?;
            }
        }

        // add data page to the volatile index
        if let Some(pages) = owned_data_pages {
            for page in pages {
                let data_page_wrapper = DataPageWrapper::from_page_no(sbi, *page)?;
                sbi.ino_data_page_map
                    .insert(live_inode, &data_page_wrapper)?;
            }
        }

        processed_live_inodes.try_insert(live_inode, ())?;
    }

    // TODO: update when allocators change
    sbi.page_allocator = BasicPageAllocator::new((max_page + 1).try_into()?, sbi.num_blocks);
    sbi.inode_allocator = BasicInodeAllocator::new(max_inode + 1);

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
        pr_info!("setting self.size to {:x}\n", self.size);
        self.num_blocks = num_blocks.try_into()?;
        self.page_allocator.update_max_pages(self.num_blocks);

        Ok(())
    }
}
