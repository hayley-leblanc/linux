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

    impl Bitmap {
        fn iter_mut(&mut self) -> impl Iterator<Item = &mut CacheLine> {
            self.lines.iter_mut()
        }
    }

    pub(crate) struct BitmapWrapper<'a, State, Op, Type> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        bm_type: PhantomData<Type>,
        bitmap: &'a mut Bitmap,
    }

    impl<'a, State, Op, Type> BitmapWrapper<'a, State, Op, Type> {
        fn new(bitmap: &'a mut Bitmap) -> Self {
            Self {
                state: PhantomData,
                op: PhantomData,
                bm_type: PhantomData,
                bitmap,
            }
        }
    }

    impl<'a> BitmapWrapper<'a, Clean, Read, InoBmap> {
        pub(crate) fn read_inode_bitmap(sbi: &SbInfo) -> Self {
            BitmapWrapper::new(unsafe {
                &mut *((sbi.virt_addr as usize + (INODE_BITMAP_PAGE * PAGE_SIZE)) as *mut Bitmap)
            })
        }
    }

    impl<'a> BitmapWrapper<'a, Clean, Read, DataBmap> {
        pub(crate) fn read_data_bitmap(sbi: &SbInfo) -> Self {
            BitmapWrapper::new(unsafe {
                &mut *((sbi.virt_addr as usize + (DATA_BITMAP_PAGE * PAGE_SIZE)) as *mut Bitmap)
            })
        }
    }

    impl<'a, Type> BitmapWrapper<'a, Clean, Read, Type> {
        // TODO: this should also be implemented for non-clean bitmaps
        pub(crate) fn get_cacheline(self, index: usize) -> CacheLineWrapper<'a, Clean, Read, Type> {
            let cacheline_num = index >> CACHELINE_BIT_SHIFT;
            CacheLineWrapper::new(&mut self.bitmap.lines[cacheline_num], Some(index))
        }

        // TODO: may want to relax return value at some point
        // TODO: this should also be implemented for non-clean bitmaps
        pub(crate) fn find_and_set_next_zero_bit(
            self,
        ) -> Result<CacheLineWrapper<'a, Clean, Alloc, Type>> {
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

        // consumes the bitmap wrapper in return for a set of cacheline wrappers selected
        // via the filtering closure
        fn filter_cache_lines<F>(self, f: F) -> Result<Vec<CacheLineWrapper<'a, Clean, Read, Type>>>
        where
            F: Fn(&CacheLine) -> bool,
        {
            let mut vec = Vec::new();
            for line in self.bitmap.iter_mut() {
                if f(line) {
                    vec.try_push(CacheLineWrapper::new(line, None))?;
                }
            }
            Ok(vec)
        }

        pub(crate) fn zero_bitmap(mut self, sbi: &SbInfo) -> Result<()> {
            let mut modified_cache_lines = Vec::<CacheLineWrapper<'a, Dirty, Zero, Type>>::new();
            // figure out which cache lines have non-zero values
            let filter_closure = |c: &CacheLine| {
                for byte in c.bits.iter() {
                    if *byte != 0 {
                        return true;
                    }
                }
                false
            };
            let mut cache_lines = self.filter_cache_lines(filter_closure)?;
            // set the cachelines to zero and save them in the modified lines vector
            for line in cache_lines.drain(..) {
                let line = line.zero();
                modified_cache_lines.try_push(line)?;
            }
            // TODO: shouldn't this return something...?
            Ok(())
        }
    }

    impl<'a, Op> BitmapWrapper<'a, Clean, Op, InoBmap> {
        pub(crate) fn bitmap_coalesce_persist(
            cache_lines: Vec<CacheLineWrapper<'a, Dirty, Op, InoBmap>>,
            sbi: &SbInfo,
        ) -> Self {
            for line in cache_lines {
                line.flush();
            }
            sfence();
            BitmapWrapper::new(unsafe {
                &mut *((sbi.virt_addr as usize + (INODE_BITMAP_PAGE * PAGE_SIZE)) as *mut Bitmap)
            })
        }
    }

    impl<'a, Op> BitmapWrapper<'a, Clean, Op, DataBmap> {
        pub(crate) fn bitmap_coalesce_persist(
            cache_lines: Vec<CacheLineWrapper<'a, Dirty, Op, DataBmap>>,
            sbi: &SbInfo,
        ) -> Self {
            for line in cache_lines {
                line.flush();
            }
            sfence();
            BitmapWrapper::new(unsafe {
                &mut *((sbi.virt_addr as usize + (DATA_BITMAP_PAGE * PAGE_SIZE)) as *mut Bitmap)
            })
        }
    }

    // TODO: extend to allow allocation/deallocation of multiple inodes
    // that reside on the same cache line
    // TODO: give this a better name
    // TODO: we might want to use a combo token/generic type thing; sometimes
    // use tokens as proof of small operations, plus generic types for larger
    // persistent objects. because this whole thing with the cache lines is kind of weird
    pub(crate) struct CacheLineWrapper<'a, State, Op, Type> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        bmap_type: PhantomData<Type>,
        line: &'a mut CacheLine,
        val: Option<usize>, // TODO: this could also be page; might need to be a set/vector?
    }

    impl<'a, State, Op, Type> CacheLineWrapper<'a, State, Op, Type> {
        fn new(line: &'a mut CacheLine, val: Option<usize>) -> Self {
            Self {
                state: PhantomData,
                op: PhantomData,
                bmap_type: PhantomData,
                line,
                val,
            }
        }
    }

    // methods that can be called on any cache line regardless of state
    impl<'a, State, Op, Type> CacheLineWrapper<'a, State, Op, Type> {
        // TODO: could also be pm page, not just inode num
        pub(crate) fn set_bit(mut self, bit: InodeNum) -> CacheLineWrapper<'a, Dirty, Alloc, Type> {
            let offset = bit & CACHELINE_MASK;
            unsafe { hayleyfs_set_bit(offset, &mut self.line as *mut _ as *mut c_void) };
            CacheLineWrapper::new(self.line, Some(bit))
        }

        pub(crate) fn zero(mut self) -> CacheLineWrapper<'a, Dirty, Zero, Type> {
            for byte in self.line.bits.iter_mut() {
                if *byte != 0 {
                    *byte = 0;
                }
            }
            CacheLineWrapper::new(self.line, None)
        }

        pub(crate) fn set_bit_persist(
            self,
            bit: InodeNum,
        ) -> CacheLineWrapper<'a, Clean, Alloc, Type> {
            // TODO: is it faster to re-implement with flush and fence, or to call
            // the existing set bit and then flush and fence?
            // TODO: have some copy constructors for different wrapper variants
            // maybe ones that take a dirty/flushed variant and flush/fence it
            let wrapper = self.set_bit(bit);
            clwb(wrapper.line, CACHELINE_SIZE, true);
            CacheLineWrapper::new(wrapper.line, Some(bit))
        }

        pub(crate) fn get_val(&self) -> Option<usize> {
            self.val
        }
    }

    impl<'a, Op, Type> CacheLineWrapper<'a, Dirty, Op, Type> {
        pub(crate) fn flush(self) -> CacheLineWrapper<'a, Flushed, Op, Type> {
            clwb(&self.line, CACHELINE_SIZE, false);
            CacheLineWrapper::new(self.line, self.val)
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
