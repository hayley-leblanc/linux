// SPDX-License-Identifier: GPL-2.0

//! Rust file system sample.

use balloc::*;
use core::{ffi, ptr, sync::atomic::Ordering};
use defs::*;
use h_dir::*;
use h_inode::*;
use kernel::prelude::*;
use kernel::{bindings, c_str, fs, rbtree::RBTree, types::ForeignOwnable};
use namei::*;
use pm::*;
use volatile::*;

mod balloc;
mod defs;
mod h_dir;
mod h_file;
mod h_inode;
mod ioctl;
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
    type DirOps = DirOps;
    const SUPER_TYPE: fs::Super = fs::Super::BlockDev;
    const NAME: &'static CStr = c_str!("hayleyfs");
    const FLAGS: i32 = fs::flags::REQUIRES_DEV | fs::flags::USERNS_MOUNT;

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

            // let inode_info = Box::try_new(HayleyFsDirInodeInfo::new(ROOT_INO))?;
            // root_ino.i_private = inode_info.into_foreign() as *mut _;

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
                (*inode).i_atime = pi.get_atime();
                (*inode).i_ctime = pi.get_ctime();
                (*inode).i_mtime = pi.get_mtime();
                (*inode).i_blkbits =
                    bindings::blksize_bits(sbi.blocksize.try_into()?).try_into()?;
                // TODO: set the rest of the fields!
            }

            sb.init_root_from_inode(inode)?
        };

        Ok(sb)
    }

    fn put_super(_sb: &fs::SuperBlock<Self>) {
        pr_info!("PUT SUPERBLOCK\n");
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

    fn evict_inode(inode: &fs::INode) {
        init_timing!(evict_inode_full);
        start_timing!(evict_inode_full);
        let sb = inode.i_sb();
        let ino = inode.i_ino();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };

        // store the inode's private page list in the global tree so that we
        // can access it later if the inode comes back into the cache
        let mode = unsafe { (*inode.get_inner()).i_mode };
        if unsafe { bindings::S_ISREG(mode.try_into().unwrap()) } {
            init_timing!(evict_reg_inode_pages);
            start_timing!(evict_reg_inode_pages);
            // using from_foreign should make sure the info structure is dropped here
            let inode_info = unsafe {
                <Box<HayleyFsRegInodeInfo> as ForeignOwnable>::from_foreign(
                    (*inode.get_inner()).i_private,
                )
            };
            unsafe { (*inode.get_inner()).i_private = core::ptr::null_mut() };
            let pages = inode_info.get_all_pages().unwrap();
            sbi.ino_data_page_tree.insert_inode(ino, pages).unwrap();
            end_timing!(EvictRegInodePages, evict_reg_inode_pages);
        } else if unsafe { bindings::S_ISDIR(mode.try_into().unwrap()) } {
            init_timing!(evict_dir_inode_pages);
            start_timing!(evict_dir_inode_pages);
            // using from_foreign should make sure the info structure is dropped here
            let inode_info = unsafe {
                <Box<HayleyFsDirInodeInfo> as ForeignOwnable>::from_foreign(
                    (*inode.get_inner()).i_private,
                )
            };
            unsafe { (*inode.get_inner()).i_private = core::ptr::null_mut() };
            let pages = inode_info.get_all_pages().unwrap();
            sbi.ino_dir_page_tree.insert_inode(ino, pages).unwrap();
            end_timing!(EvictDirInodePages, evict_dir_inode_pages);
        }
        // TODO: handle other cases

        let link_count = unsafe { (*inode.get_inner()).__bindgen_anon_1.i_nlink };

        unsafe { bindings::clear_inode(inode.get_inner()) };

        // TODO: we might want to make deallocating inode numbers unsafe or
        // require proof that the inode in question has actually been
        // persistently freed
        // inode should only be deallocated if the inode's link count is actually 0
        if link_count == 0 {
            sbi.inode_allocator.dealloc_ino(ino).unwrap();
        }
        end_timing!(EvictInodeFull, evict_inode_full);
    }

    // TODO: safety
    fn init_private(inode: *mut bindings::inode) -> Result<()> {
        let inode_info = Box::try_new(HayleyFsDirInodeInfo::new(ROOT_INO))?;
        unsafe { (*inode).i_private = inode_info.into_foreign() as *mut _ };
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
        (*inode).i_atime = pi.get_atime();
        (*inode).i_ctime = pi.get_ctime();
        (*inode).i_mtime = pi.get_mtime();
        (*inode).i_blkbits = bindings::blksize_bits(sbi.blocksize.try_into()?).try_into()?;
        // TODO: set the rest of the fields!

        pr_info!("init fs done\n");
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
        // TODO: implement a VecDeque to get better perf
        let live_inode = live_inode_vec.remove(0);
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
                    sbi.ino_dentry_tree.insert(live_inode, dentry)?;
                    live_inode_vec.try_push(dentry.get_ino())?;
                }
                let page_info = DirPageInfo::new(dir_page_wrapper.get_page_no());
                sbi.ino_dir_page_tree.insert_one(live_inode, page_info)?;
            }
        }

        // add data page to the volatile index
        if let Some(pages) = owned_data_pages {
            let pages = build_tree(sbi, live_inode, pages)?;
            sbi.ino_data_page_tree.insert_inode(live_inode, pages)?;
        }

        processed_live_inodes.try_insert(live_inode, ())?;
    }

    sbi.page_allocator = Option::<PerCpuPageAllocator>::new_from_alloc_vec(
        alloc_page_vec,
        DATA_PAGE_START,
        // sbi.num_blocks,
        if NUM_PAGE_DESCRIPTORS < sbi.num_blocks {
            NUM_PAGE_DESCRIPTORS
        } else {
            sbi.num_blocks
        },
        sbi.cpus,
    )?;
    sbi.inode_allocator = RBInodeAllocator::new_from_alloc_vec(alloc_inode_vec, ROOT_INO)?;

    Ok(())
}

fn build_tree(
    sbi: &SbInfo,
    ino: InodeNum,
    input_vec: &Vec<PageNum>,
) -> Result<RBTree<u64, DataPageInfo>> {
    let mut output_tree = RBTree::new();

    for page_no in input_vec {
        let data_page_wrapper = DataPageWrapper::from_page_no(sbi, *page_no)?;
        let offset = data_page_wrapper.get_offset();
        let page_info = DataPageInfo::new(ino, *page_no, offset);
        output_tree.try_insert(offset, page_info)?;
    }

    Ok(output_tree)
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
        // self.page_allocator =
        //     Option::<PerCpuPageAllocator>::new_from_range(DATA_PAGE_START, self.num_blocks, self.cpus)?;
        self.page_allocator = Option::<PerCpuPageAllocator>::new_from_range(
            DATA_PAGE_START,
            // NUM_PAGE_DESCRIPTORS, // TODO: have this be the actual number of blocks
            if NUM_PAGE_DESCRIPTORS < self.num_blocks {
                NUM_PAGE_DESCRIPTORS
            } else {
                self.num_blocks
            },
            self.cpus,
        )?;

        Ok(())
    }
}
