use kernel::bindings::{
    dax_device, fs_context, fs_context_operations, fs_parameter, fs_parameter_spec, inode, kgid_t,
    kuid_t, set_nlink, super_block, umode_t,
};
use kernel::c_types::c_void;
use kernel::prelude::*;
use kernel::{c_default_struct, fsparam_flag, fsparam_string, fsparam_u32, PAGE_SIZE};

use crate::data::*;
use crate::defs::*;
use crate::inode_rs::*;
use crate::pm::*;
use crate::tokens::*;
use core::mem::size_of;

#[repr(C)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum hayleyfs_param {
    Opt_init,   // flag to indicate whether to initialize the FS or remount existing system
    Opt_source, // flag indicating source device to mount on
    Opt_crash, // flag for testing remount/recovery; tells us a point to inject a crash (by returning an error early)
}

#[no_mangle]
pub(crate) static hayleyfs_fs_parameters: [fs_parameter_spec; 4] = [
    fsparam_string!("source", hayleyfs_param::Opt_source),
    fsparam_flag!("init", hayleyfs_param::Opt_init),
    fsparam_u32!("crash", hayleyfs_param::Opt_crash),
    c_default_struct!(fs_parameter_spec),
];

// TODO: packed?
// TODO: order structs low to high
#[repr(packed)]
pub(crate) struct HayleyfsSuperBlock {
    pub(crate) blocksize: u32,
    pub(crate) magic: u32,
    pub(crate) size: u64,
}

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
pub(crate) struct HayleyfsMountOpts {
    pub(crate) init: bool,
    pub(crate) crash_point: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub(crate) struct SbInfo {
    pub(crate) sb: *mut super_block, // raw pointer to the VFS super block
    pub(crate) s_daxdev: *mut dax_device, // raw pointer to the dax device we are mounted on
    pub(crate) s_dev_offset: u64,    // no idea what this is used for but a dax fxn needs it
    pub(crate) virt_addr: *mut c_void, // raw pointer virtual address of beginning of FS instance
    pub(crate) phys_addr: u64,       // physical address of beginning of FS instance
    pub(crate) pm_size: u64,         // size of the PM device (TODO: make unsigned)
    pub(crate) uid: kuid_t,
    pub(crate) gid: kgid_t,
    pub(crate) mode: umode_t,
    pub(crate) mount_opts: HayleyfsMountOpts,
}

// TODO: do CacheLine and PersistentBitmap have to be packed?
// p sure Rust makes arrays contiguous so they shouldn't need to be
// compiler warning indicates making them packed could have weird consequences
pub(crate) struct CacheLine {
    pub(crate) bits: [u8; 64],
}

impl CacheLine {
    pub(crate) fn set_at_offset(&mut self, offset: usize) {
        // TODO: return error if offset is not less than 64
        unsafe { hayleyfs_set_bit(offset, self as *mut _ as *mut c_void) };
    }

    // TODO: this is not really the right place to do this I think; find a better location
    // this function assumes that the bitmap this is set on is the data bitmap, but we don't
    // actually do anything to enforce that right now
    // TODO: do we really need to pass in the bitmap token?
    pub(crate) fn set_reserved_page_bits(
        &mut self,
        bitmap_token: BitmapFenceToken<'_>,
    ) -> CacheLineToken {
        self.set_at_offset(SUPER_BLOCK_PAGE);
        self.set_at_offset(INODE_BITMAP_PAGE);
        self.set_at_offset(INODE_PAGE);
        self.set_at_offset(DATA_BITMAP_PAGE);

        CacheLineToken::new(self)
    }

    fn fill(&mut self, value: u8) -> bool {
        let mut ret = false;
        for byte in self.bits.iter_mut() {
            if *byte != value {
                *byte = value;
            }
        }
        ret
    }
}

pub(crate) struct PersistentBitmap {
    contents: [CacheLine; PAGE_SIZE / CACHELINE_SIZE],
}

impl PersistentBitmap {
    pub(crate) fn get_bitmap_cacheline(&mut self, index: usize) -> &mut CacheLine {
        // each cache line has 64 bytes - 64*8 = 512
        // 512 inodes/pages per cache line
        let cacheline_num = index >> CACHELINE_BIT_SHIFT;
        &mut self.contents[cacheline_num]
    }

    pub(crate) fn get_cacheline_by_index(&mut self, index: usize) -> &mut CacheLine {
        &mut self.contents[index]
    }

    pub(crate) fn zero_bitmap(&mut self) -> BitmapToken<'_> {
        // keep track of modified cache lines so we can use them to create a
        // single bitmap token that flushes only the cache lines that actually
        // were changed
        let mut modified_cache_lines = Vec::<usize>::new();
        let mut i = 0;
        for (i, line) in self.contents.iter_mut().enumerate() {
            let res = line.fill(0);
            if res {
                modified_cache_lines.try_push(i);
            }
        }
        BitmapToken::new(self, modified_cache_lines)
    }
}

pub(crate) fn hayleyfs_get_sbi(sb: *mut super_block) -> &'static mut SbInfo {
    let sbi: &mut SbInfo = unsafe { &mut *((*sb).s_fs_info as *mut SbInfo) };
    sbi
}

pub(crate) fn hayleyfs_get_sbi_from_fc(fc: *mut fs_context) -> &'static mut SbInfo {
    let sbi: &mut SbInfo = unsafe { &mut *((*fc).s_fs_info as *mut SbInfo) };
    sbi
}

pub(crate) fn set_nlink_safe(inode: &mut inode, n: u32) {
    unsafe { set_nlink(inode, n) };
}
