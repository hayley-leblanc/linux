use crate::def::*;
use crate::inode_def::*;
use crate::pm::*;
use core::marker::PhantomData;
use kernel::bindings::{
    dax_device, fs_context, fs_parameter_spec, inode, kgid_t, kuid_t, set_nlink, super_block,
    umode_t,
};
use kernel::c_types::c_void;
use kernel::prelude::*;
use kernel::{c_default_struct, fsparam_flag, fsparam_string, fsparam_u32, PAGE_SIZE};

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

// TODO: order structs low to high
#[repr(C)]
pub(crate) struct HayleyfsSuperBlock {
    pub(crate) blocksize: u32,
    pub(crate) magic: u32,
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

pub(crate) mod hayleyfs_bitmap {
    use super::*;
    // persistent cache lines making up a bitmap
    struct CacheLine {
        bits: [u8; 64],
    }

    // persistent bitmap
    struct Bitmap {
        lines: [CacheLine; NUM_BITMAP_CACHELINES],
    }

    pub(crate) struct BitmapWrapper<'a, State = Clean, Op = Read> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        bitmap: &'a mut Bitmap,
    }

    // TODO: should potentially use mutexes so only one thread can read these
    // things at a time
    impl<'a> BitmapWrapper<'a, Clean, Read> {
        pub(crate) fn read_inode_bitmap(sbi: &SbInfo) -> Self {
            Self {
                state: PhantomData,
                op: PhantomData,
                bitmap: unsafe {
                    &mut *((sbi.virt_addr as usize + (INODE_BITMAP_PAGE * PAGE_SIZE))
                        as *mut Bitmap)
                },
            }
        }

        fn get_cacheline(self, index: usize) -> CacheLineWrapper<'a, Clean, Read> {
            let cacheline_num = index >> CACHELINE_BIT_SHIFT;
            CacheLineWrapper {
                state: PhantomData,
                op: PhantomData,
                line: &mut self.bitmap.lines[cacheline_num],
                ino: index,
            }
        }

        // TODO: may want to relax return value at some point
        pub(crate) fn find_and_set_next_zero_bit(
            mut self,
        ) -> Result<CacheLineWrapper<'a, Clean, Alloc>> {
            // starts at bit 1 to ignore bit 0 since we don't use inode 0
            let ino = unsafe {
                hayleyfs_find_next_zero_bit(
                    self.bitmap as *mut _ as *mut u64,
                    (PAGE_SIZE * 8).try_into().unwrap(),
                    2,
                )
            };

            if ino == (PAGE_SIZE * 8) {
                return Err(Error::ENOSPC);
            }

            let cache_line = self.get_cacheline(ino);

            // unsafe { hayleyfs_set_bit(ino, bitmap as *mut _ as *mut c_void) };
            let cache_line = cache_line.set_bit_persist(ino);

            Ok(cache_line)
        }
    }

    // TODO: extend to allow allocation/deallocation of multiple inodes
    // that reside on the same cache line
    // TODO: give this a better name
    // TODO: we might want to use a combo token/generic type thing; sometimes
    // use tokens as proof of small operations, plus generic types for larger
    // persistent objects. because this whole thing with the cache lines is kind of weird
    pub(crate) struct CacheLineWrapper<'a, State = Clean, Op = Read> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        line: &'a mut CacheLine,
        ino: InodeNum, // TODO: this could also be page
                       // TODO: there might be cases where we want to store a set of inodes, or no inodes
    }

    // methods that can be called on any cache line regardless of state
    impl<'a, State, Op> CacheLineWrapper<'a, State, Op> {
        // TODO: could also be pm page, not just inode num
        pub(crate) fn set_bit(mut self, bit: InodeNum) -> CacheLineWrapper<'a, Dirty, Alloc> {
            let offset = bit & CACHELINE_MASK;
            unsafe { hayleyfs_set_bit(offset, &mut self.line as *mut _ as *mut c_void) };
            CacheLineWrapper {
                state: PhantomData,
                op: PhantomData,
                line: self.line,
                ino: bit,
            }
        }

        pub(crate) fn set_bit_persist(
            mut self,
            bit: InodeNum,
        ) -> CacheLineWrapper<'a, Clean, Alloc> {
            // TODO: is it faster to re-implement with flush and fence, or to call
            // the existing set bit and then flush and fence?
            // TODO: have some copy constructors for different wrapper variants
            // maybe ones that take a dirty/flushed variant and flush/fence it
            let wrapper = self.set_bit(bit);
            clwb(wrapper.line, CACHELINE_SIZE, true);
            CacheLineWrapper {
                state: PhantomData,
                op: PhantomData,
                line: wrapper.line,
                ino: bit,
            }
        }

        pub(crate) fn get_ino(&self) -> InodeNum {
            self.ino
        }
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
