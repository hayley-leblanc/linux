#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

use crate::super_def::SbInfo;
use kernel::bindings::{
    dax_device, dir_context, file, fs_context, fs_parameter, fs_parameter_spec, fs_parse_result,
    inode, kgid_t, kuid_t, pfn_t, FS_COMPRBLK_FL, FS_COMPR_FL, FS_DIRSYNC_FL, FS_JOURNAL_DATA_FL,
    FS_NOATIME_FL, FS_NOCOMP_FL, FS_NODUMP_FL, FS_NOTAIL_FL, FS_SECRM_FL, FS_SYNC_FL, FS_TOPDIR_FL,
    FS_UNRM_FL,
};
use kernel::c_types::{c_char, c_int, c_long, c_ulong, c_void};
use kernel::prelude::*;
use kernel::PAGE_SIZE;

pub(crate) const __LOG_PREFIX: &[u8] = b"hayleyfs\0";

pub(crate) const ROOT_INO: usize = 1;

pub(crate) const SUPER_BLOCK_PAGE: usize = 0;
pub(crate) const INODE_BITMAP_PAGE: usize = 1;
pub(crate) const INODE_PAGE: usize = 2;
pub(crate) const DATA_BITMAP_PAGE: usize = 3;
// pub(crate) const DATA_START: usize = 4;

pub(crate) const MAX_FILENAME_LEN: usize = 32;
pub(crate) const DENTRIES_PER_PAGE: usize = 32;
pub(crate) const CACHELINE_SIZE: usize = 64; // TODO: this should probably come from the kernel
pub(crate) const CACHELINE_BYTE_SHIFT: usize = 6;
pub(crate) const CACHELINE_BIT_SHIFT: usize = 9;
pub(crate) const NUM_BITMAP_CACHELINES: usize = PAGE_SIZE / CACHELINE_SIZE;
pub(crate) const CACHELINE_MASK: usize = (1 << CACHELINE_BIT_SHIFT) - 1;

pub(crate) const LONG_MAX: usize = 9223372036854775807;
pub(crate) const HAYLEYFS_MAGIC: u32 = 0xaaaaaaaa;
pub(crate) const READDIR_END: i64 = !0;

// set of flags that should be inherited by new nodes from parent
// taken from NOVA, not checked
pub(crate) const HAYLEYFS_FL_INHERITED: u32 = FS_SECRM_FL
    | FS_UNRM_FL
    | FS_COMPR_FL
    | FS_SYNC_FL
    | FS_NODUMP_FL
    | FS_NOATIME_FL
    | FS_COMPRBLK_FL
    | FS_NOCOMP_FL
    | FS_JOURNAL_DATA_FL
    | FS_NOTAIL_FL
    | FS_DIRSYNC_FL;
pub(crate) const HAYLEYFS_REG_FLMASK: u32 = !(FS_DIRSYNC_FL | FS_TOPDIR_FL);
pub(crate) const HAYLEYFS_OTHER_FLMASK: u32 = FS_NODUMP_FL | FS_NOATIME_FL;

// semantic types indicating the persistence state of an object
pub(crate) struct Dirty;
pub(crate) struct Flushed;
pub(crate) struct Clean;

// semantic types indicating the most recent type of modification to an object
// TODO: think more about what these should be once the fs works better
pub(crate) struct Read; // indicates no change since it was read. TODO: better name
pub(crate) struct Alloc; // TODO: might be more clear to have separate alloc, init, and uninit types
pub(crate) struct Init;
pub(crate) struct Valid;
pub(crate) struct Zero;
pub(crate) struct Link;
pub(crate) struct Flags;

// semantic types used to indicate the type of bitmaps and/or inodes
// to reduce some code repetition and prevent mistakes
pub(crate) struct Inode;
pub(crate) struct Data;
pub(crate) struct Dir;
pub(crate) struct Unknown;

// pub(crate) trait InodeType {}
// impl InodeType for Data {}
// impl InodeType for Dir {}
// impl InodeType for Unknown {}

pub(crate) trait PmObjWrapper {}

pub(crate) type PmPage = usize; // TODO: move this somewhere else

pub(crate) fn check_page_no(sbi: &SbInfo, page_no: PmPage) -> Result<()> {
    let max_pages = sbi.pm_size / PAGE_SIZE as u64;
    if page_no >= max_pages.try_into()? {
        Err(Error::EINVAL)
    } else {
        Ok(())
    }
}

pub(crate) trait Flatten {
    type Output;

    fn flatten_tuple(self) -> Self::Output;
}

impl<A, B> Flatten for (A, B)
where
    A: PmObjWrapper,
    B: PmObjWrapper,
{
    type Output = (A, B);

    fn flatten_tuple(self) -> Self::Output {
        self
    }
}

impl<A, B, C> Flatten for (A, (B, C))
where
    A: PmObjWrapper,
    B: PmObjWrapper,
    C: PmObjWrapper,
{
    type Output = (A, B, C);

    fn flatten_tuple(self) -> Self::Output {
        let (a, (b, c)) = self;
        (a, b, c)
    }
}

impl<A, B, C, D> Flatten for (A, (B, (C, D)))
where
    A: PmObjWrapper,
    B: PmObjWrapper,
    C: PmObjWrapper,
    D: PmObjWrapper,
{
    type Output = (A, B, C, D);

    fn flatten_tuple(self) -> Self::Output {
        let (a, (b, (c, d))) = self;
        (a, b, c, d)
    }
}

// TODO: what does allow improper ctypes do here?
extern "C" {
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_fs_put_dax(dax_dev: *mut dax_device);
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_pfn_t_to_pfn(pfn: pfn_t) -> u64;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_set_bit(nr: usize, addr: *mut c_void) -> i32;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_clear_bit(nr: usize, addr: *mut c_void);
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_test_bit(nr: usize, addr: *const c_void) -> i32;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_find_next_zero_bit(
        addr: *mut c_ulong,
        size: c_ulong,
        offset: c_ulong,
    ) -> usize;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_dir_emit(
        ctx: *mut dir_context,
        name: *const c_char,
        namelen: i32,
        ino: u64,
        t: u32,
    ) -> bool;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_file_inode(f: *const file) -> *mut inode;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_current_fsuid() -> kuid_t;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_current_fsgid() -> kgid_t;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_fs_parse(
        fc: *mut fs_context,
        desc: *const fs_parameter_spec,
        param: *mut fs_parameter,
        result: *mut fs_parse_result,
    ) -> c_int;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_uid_read(inode: *const inode) -> c_int;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_gid_read(inode: *const inode) -> c_int;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_isdir(flags: u16) -> bool;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_isreg(flags: u16) -> bool;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_write_uid(inode: &mut inode, uid: u32);
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_write_gid(inode: &mut inode, gid: u32);
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_err_ptr(err: c_long) -> *mut c_void;
}
