use kernel::bindings::{
    dax_device, dir_context, file, fs_context, fs_parameter, fs_parameter_spec, fs_parse_result,
    inode, kgid_t, kuid_t, pfn_t,
};
use kernel::c_types::{c_char, c_int, c_ulong, c_void};
use kernel::PAGE_SIZE;

pub(crate) const __LOG_PREFIX: &[u8] = b"hayleyfs\0";

pub(crate) const ROOT_INO: usize = 1;

pub(crate) const SUPER_BLOCK_PAGE: usize = 0;
pub(crate) const INODE_BITMAP_PAGE: usize = 1;
pub(crate) const INODE_PAGE: usize = 2;
pub(crate) const DATA_BITMAP_PAGE: usize = 3;
pub(crate) const DATA_START: usize = 4;

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

// semantic types used to indicate the type of a bitmap (data or inode)
// to reduce some code repetition and prevent mistakes
pub(crate) struct InoBmap;
pub(crate) struct DataBmap;

pub(crate) trait PmObjWrapper {}

pub(crate) type PmPage = usize; // TODO: move this somewhere else

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

extern "C" {
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_fs_put_dax(dax_dev: *mut dax_device);
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_pfn_t_to_pfn(pfn: pfn_t) -> u64;
    #[allow(improper_ctypes)]
    pub(crate) fn hayleyfs_set_bit(nr: usize, addr: *mut c_void) -> i32;
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
}
