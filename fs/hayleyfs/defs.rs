#![allow(unused)]

use kernel::bindings::{dax_device, dir_context, file, inode, pfn_t};
use kernel::c_types::{c_char, c_ulong, c_void};

pub(crate) const __LOG_PREFIX: &[u8] = b"hayleyfs\0";

pub(crate) const SUPER_BLOCK_PAGE: usize = 0;
pub(crate) const INODE_BITMAP_PAGE: usize = 1;
pub(crate) const INODE_PAGE: usize = 2;
pub(crate) const DATA_BITMAP_PAGE: usize = 3;
pub(crate) const DATA_START: usize = 4;

pub(crate) const MAX_FILENAME_LEN: usize = 32;
pub(crate) const DENTRIES_PER_PAGE: usize = 32;

pub(crate) const LONG_MAX: usize = 9223372036854775807;
pub(crate) const HAYLEYFS_MAGIC: u32 = 0xaaaaaaaa;
pub(crate) const READDIR_END: i64 = !0;

extern "C" {
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_fs_put_dax(dax_dev: *mut dax_device);
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_pfn_t_to_pfn(pfn: pfn_t) -> u64;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_set_bit(nr: usize, addr: *mut c_void);
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
}
