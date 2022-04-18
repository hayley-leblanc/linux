#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

use crate::def::*;
use crate::file::hayleyfs_file::*;
use crate::h_inode::hayleyfs_inode::*;
use crate::h_inode::*;
use crate::pm::*;
use core::marker::PhantomData;
use core::mem::size_of;
use kernel::bindings::{
    dax_device, fs_parameter_spec, inode, kgid_t, kuid_t, set_nlink, super_block, umode_t,
};
use kernel::c_types::c_void;
use kernel::prelude::*;
use kernel::rbtree::RBTree;
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
    pub(crate) magic: u32,
    pub(crate) blocksize: u64,
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
        bits: [u64; 8],
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

    #[must_use]
    pub(crate) struct BitmapWrapper<'a, State, Op, Type> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        bm_type: PhantomData<Type>,
        bitmap: &'a mut Bitmap,
        dirty_cache_lines: RBTree<usize, ()>,
    }

    impl<'a, State, Op, Type> PmObjWrapper for BitmapWrapper<'a, State, Op, Type> {}

    impl<'a, State, Op, Type> BitmapWrapper<'a, State, Op, Type> {
        fn new(bitmap: &'a mut Bitmap, dirty_cache_lines: RBTree<usize, ()>) -> Self {
            Self {
                state: PhantomData,
                op: PhantomData,
                bm_type: PhantomData,
                bitmap,
                dirty_cache_lines,
            }
        }

        // clippy tells me not to use the return, but the regular compiler doesn't understand if I don't
        #[allow(clippy::needless_return)]
        pub(crate) fn check_bit(&self, bit: usize) -> bool {
            return unsafe { hayleyfs_test_bit(bit, self.bitmap as *const _ as *const c_void) }
                == 1;
        }

        /// Safety: only safe to use if you will subsequently call set_bit on another bit,
        /// as in the set_bits! macro
        pub(crate) unsafe fn set_bit_unsafe(&mut self, bit: usize) -> Result<()> {
            if bit > PAGE_SIZE * 8 {
                return Err(EINVAL);
            }
            unsafe { hayleyfs_set_bit(bit, self.bitmap as *mut _ as *mut c_void) };
            self.dirty_cache_lines
                .try_insert(get_cacheline_num(bit), ())?;
            Ok(())
        }

        pub(crate) fn set_bit(
            mut self,
            bit: usize,
        ) -> Result<BitmapWrapper<'a, Dirty, Alloc, Type>> {
            if bit > PAGE_SIZE * 8 {
                return Err(EINVAL);
            }
            self.dirty_cache_lines
                .try_insert(get_cacheline_num(bit), ())?;
            unsafe { hayleyfs_set_bit(bit, self.bitmap as *mut _ as *mut c_void) };

            Ok(BitmapWrapper::new(self.bitmap, self.dirty_cache_lines))
        }

        // pub(crate) fn clear_bit(
        //     mut self,
        //     bit: usize,
        // ) -> Result<BitmapWrapper<'a, Dirty, Zero, Type>> {
        //     if bit > PAGE_SIZE * 8 {
        //         return Err(EINVAL);
        //     }
        //     self.dirty_cache_lines
        //         .try_insert(get_cacheline_num(bit), ())?;
        //     unsafe { hayleyfs_clear_bit(bit, self.bitmap as *mut _ as *mut c_void) };

        //     Ok(BitmapWrapper::new(self.bitmap, self.dirty_cache_lines))
        // }

        // pub(crate) fn clear_bit(
        //     mut self,
        //     bit: usize,
        // ) -> Result<BitmapWrapper<'a, Dirty, Zero, Type>> {
        //     if bit > PAGE_SIZE * 8 {
        //         return Err(Error::EINVAL);
        //     }
        //     self.dirty_cache_lines
        //         .try_insert(get_cacheline_num(bit), ())?;
        //     unsafe { hayleyfs_clear_bit(bit, self.bitmap as *mut _ as *mut c_void) };

        //     Ok(BitmapWrapper::new(self.bitmap, self.dirty_cache_lines))
        // }
    }

    impl<'a, State, Op> BitmapWrapper<'a, State, Op, Inode> {
        // pub(crate) fn clear_invalid_ino_bits(
        //     self,
        //     invalid_inos: Vec<InodeNum>,
        //     _: Vec<InodeWrapper<'a, Clean, Zero, Unknown>>,
        // ) -> Result<BitmapWrapper<'a, Flushed, Zero, Inode>> {
        //     if invalid_inos.len() == 0 {
        //         return Ok(BitmapWrapper::new(self.bitmap, self.dirty_cache_lines));
        //     }
        //     for ino in invalid_inos.iter() {
        //         unsafe { hayleyfs_set_bit(*ino, self.bitmap as *mut _ as *mut c_void) };
        //     }
        //     // redundant, but make sure that we get a dirty bitmap out of this without re-creating
        //     // it every time we set a bit
        //     let bitmap = self.set_bit(*(invalid_inos.last()).unwrap())?;
        //     let bitmap = bitmap.flush();
        //     Ok(BitmapWrapper::new(bitmap.bitmap, bitmap.dirty_cache_lines))
        // }

        //     pub(crate) fn clear_bit<Type>(
        //         mut self,
        //         // bit: usize,
        //         inode: &InodeWrapper<'a, Clean, Zero, Type>,
        //     ) -> Result<BitmapWrapper<'a, Dirty, Zero, Inode>> {
        //         let bit = inode.get_ino();
        //         if bit > PAGE_SIZE * 8 {
        //             return Err(EINVAL);
        //         }
        //         self.dirty_cache_lines
        //             .try_insert(get_cacheline_num(bit), ())?;
        //         unsafe { hayleyfs_clear_bit(bit, self.bitmap as *mut _ as *mut c_void) };

        //         Ok(BitmapWrapper::new(self.bitmap, self.dirty_cache_lines))
        //     }

        pub(crate) fn clear_bit<Type>(
            mut self,
            pi: &InodeWrapper<'a, Clean, Zero, Type>,
        ) -> Result<BitmapWrapper<'a, Dirty, Zero, Inode>> {
            let bit = pi.get_ino();
            if bit > PAGE_SIZE * 8 {
                return Err(EINVAL);
            }
            self.dirty_cache_lines
                .try_insert(get_cacheline_num(bit), ())?;
            unsafe { hayleyfs_clear_bit(bit, self.bitmap as *mut _ as *mut c_void) };

            Ok(BitmapWrapper::new(self.bitmap, self.dirty_cache_lines))
        }
    }

    impl<'a, State, Op> BitmapWrapper<'a, State, Op, Data> {
        pub(crate) fn clear_bits(
            self,
            pages: Vec<DataPageWrapper<'a, Clean, Zero>>,
        ) -> Result<BitmapWrapper<'a, Flushed, Zero, Data>> {
            if pages.len() == 0 {
                return Ok(BitmapWrapper::new(self.bitmap, self.dirty_cache_lines));
            }
            for pg in pages.iter() {
                unsafe {
                    hayleyfs_clear_bit(pg.get_page_no(), self.bitmap as *mut _ as *mut c_void)
                };
            }
            // redundant, but make sure that we get a dirty bitmap out of this without re-creating
            // it every time we set a bit
            let bitmap = self.clear_bit(*(pages.last()).unwrap())?;
            let bitmap = bitmap.flush();
            Ok(BitmapWrapper::new(bitmap.bitmap, bitmap.dirty_cache_lines))
        }

        // /// this returns a dirty bitmap even if there isn't actually
        // /// a bit to clear. this is a bit inefficient, since we still have
        // /// to call flush and fence, but it's easier (for now at least) than
        // /// messing with dynamic dispatch.
        // /// we do end up incurring an extra fence because I don't know how to manage
        // /// page cleanliness with the trait object if the file does have pages
        // pub(crate) fn clear_bit(
        //     mut self,
        //     page: &dyn EmptyFilePage,
        // ) -> Result<BitmapWrapper<'a, Dirty, Zero, Data>> {
        //     let bit = page.get_page_no();
        //     match bit {
        //         None => Ok(BitmapWrapper::new(self.bitmap, self.dirty_cache_lines)),
        //         Some(bit) => {
        //             if bit > PAGE_SIZE * 8 {
        //                 return Err(EINVAL);
        //             }
        //             self.dirty_cache_lines
        //                 .try_insert(get_cacheline_num(bit), ())?;
        //             unsafe { hayleyfs_clear_bit(bit, self.bitmap as *mut _ as *mut c_void) };

        //             Ok(BitmapWrapper::new(self.bitmap, self.dirty_cache_lines))
        //         }
        //     }
        // }
    }

    impl<'a> BitmapWrapper<'a, Clean, Read, Inode> {
        pub(crate) fn read_inode_bitmap(sbi: &SbInfo) -> Self {
            BitmapWrapper::new(
                unsafe {
                    &mut *((sbi.virt_addr as usize + (INODE_BITMAP_PAGE * PAGE_SIZE))
                        as *mut Bitmap)
                },
                RBTree::new(),
            )
        }
    }

    impl<'a> BitmapWrapper<'a, Clean, Read, Data> {
        pub(crate) fn read_data_bitmap(sbi: &SbInfo) -> Self {
            BitmapWrapper::new(
                unsafe {
                    &mut *((sbi.virt_addr as usize + (DATA_BITMAP_PAGE * PAGE_SIZE)) as *mut Bitmap)
                },
                RBTree::new(),
            )
        }
    }

    /// sets multiple bits in a bitmap, ultimately returning a dirty bitmap wrapper
    /// or an error if any of the set bits are invalid
    /// TODO: need some way to roll back if we've already set some of the other bits
    #[macro_export]
    macro_rules! set_bits {
        ($bitmap:ident, $b:ident) => {
            $bitmap.set_bit($b)
        };
        ($bitmap:ident, $b0:ident, $($b1:ident),+) => { {
            let res = unsafe { $bitmap.set_bit_unsafe($b0) };
            match res {
                Ok(_) => set_bits!{$bitmap, $($b1),+},
                Err(e) => Err(e)
            }
        }
        };
    }

    impl<'a> BitmapWrapper<'a, Clean, Alloc, Data> {
        // TODO: this should also be allowed to take in a clean zeroed wrapper
        pub(crate) fn alloc_root_ino_page(
            self,
            _: &BitmapWrapper<'a, Clean, Zero, Inode>,
        ) -> Result<(PmPage, BitmapWrapper<'a, Flushed, Alloc, Data>)> {
            let (page_no, bitmap) = self.find_and_set_next_zero_bit()?;
            let bitmap = bitmap.flush();
            Ok((page_no, bitmap))
        }
    }

    impl<'a, Op, Type> BitmapWrapper<'a, Flushed, Op, Type> {
        pub(crate) unsafe fn fence_unsafe(self) -> BitmapWrapper<'a, Clean, Op, Type> {
            BitmapWrapper::new(self.bitmap, self.dirty_cache_lines)
        }
        pub(crate) fn fence(self) -> BitmapWrapper<'a, Clean, Op, Type> {
            sfence();
            BitmapWrapper::new(self.bitmap, self.dirty_cache_lines)
        }
    }

    impl<'a> BitmapWrapper<'a, Clean, Zero, Inode> {
        pub(crate) fn alloc_root_ino(
            mut self,
            _: &BitmapWrapper<'a, Flushed, Alloc, Data>,
        ) -> Result<(InodeNum, BitmapWrapper<'a, Flushed, Alloc, Inode>)> {
            let reserved_bit = 0;
            // set bits zero and one
            let bitmap = set_bits!(self, reserved_bit, ROOT_INO)?.flush();
            Ok((ROOT_INO, bitmap))
        }
    }
    impl<'a, Op, Type> BitmapWrapper<'a, Clean, Op, Type> {
        // TODO: this should probably be allowed for other ops and persistence states
        pub(crate) fn find_and_set_next_zero_bit(
            self,
        ) -> Result<(usize, BitmapWrapper<'a, Dirty, Alloc, Type>)> {
            let bit = unsafe {
                hayleyfs_find_next_zero_bit(
                    self.bitmap as *mut _ as *mut u64,
                    (PAGE_SIZE * 8).try_into().unwrap(),
                    1,
                )
            };

            if bit == (PAGE_SIZE * 8) {
                pr_info!("no space, ran out of bits to allocate\n");
                return Err(ENOSPC);
            }

            Ok((bit, self.set_bit(bit)?))
        }

        pub(crate) fn allocate_bits(
            self,
            nr: i64,
        ) -> Result<(Vec<PmPage>, BitmapWrapper<'a, Flushed, Alloc, Type>)> {
            if nr <= 0 {
                return Err(EINVAL);
            }
            let bits_vec = Vec::<PmPage>::new();
            for i in 0..nr {
                let bit = unsafe {
                    hayleyfs_find_next_zero_bit(
                        self.bitmap as *mut _ as *mut u64,
                        (PAGE_SIZE * 8).try_into().unwrap(),
                        1,
                    )
                };

                if bit == (PAGE_SIZE * 8) {
                    pr_info!("no space, ran out of bits to allocate\n");
                    return Err(ENOSPC);
                }

                unsafe { self.set_bit_unsafe(bit)? };

                bits_vec.try_push(bit)?;
            }
            // quick hack to get the bitmap in the correct state
            // TODO: this is inefficient, do it better
            let bitmap = self.set_bit(bits_vec[0])?;
            Ok((bits_vec, bitmap.flush()))
        }
    }

    impl<'a, Type> BitmapWrapper<'a, Clean, Read, Type> {
        pub(crate) fn zero_bitmap(mut self) -> Result<BitmapWrapper<'a, Clean, Zero, Type>> {
            for (i, cache_line) in self.bitmap.iter_mut().enumerate() {
                for j in 0..8 {
                    if cache_line.bits[j] != 0 {
                        cache_line.bits[j] = 0;
                        self.dirty_cache_lines.try_insert(i, ())?;
                    }
                }
            }
            for num in self.dirty_cache_lines.keys() {
                clwb(&self.bitmap.lines[*num], CACHELINE_SIZE, false);
            }
            sfence();
            Ok(BitmapWrapper::new(self.bitmap, self.dirty_cache_lines))
        }
    }

    impl<'a, Op, Type> BitmapWrapper<'a, Dirty, Op, Type> {
        pub(crate) fn flush(self) -> BitmapWrapper<'a, Flushed, Op, Type> {
            for num in self.dirty_cache_lines.keys() {
                clwb(&self.bitmap.lines[*num], CACHELINE_SIZE, false);
            }
            BitmapWrapper::new(self.bitmap, self.dirty_cache_lines)
        }

        pub(crate) fn persist(self) -> BitmapWrapper<'a, Clean, Op, Type> {
            for num in self.dirty_cache_lines.keys() {
                clwb(&self.bitmap.lines[*num], CACHELINE_SIZE, false);
            }
            sfence();
            BitmapWrapper::new(self.bitmap, self.dirty_cache_lines)
        }
    }
}

pub(crate) mod hayleyfs_sb {
    use super::*;

    #[repr(C)]
    struct HayleyfsSuperBlock {
        blocksize: u32,
        magic: u32,
        size: u64,
    }

    pub(crate) struct SuperBlockWrapper<'a, State, Op> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        sb: &'a mut HayleyfsSuperBlock,
    }

    impl<'a, State, Op> PmObjWrapper for SuperBlockWrapper<'a, State, Op> {}

    impl<'a, State, Op> SuperBlockWrapper<'a, State, Op> {
        fn new(sb: &'a mut HayleyfsSuperBlock) -> Self {
            Self {
                state: PhantomData,
                op: PhantomData,
                sb,
            }
        }

        fn new_flush(sb: &'a mut HayleyfsSuperBlock) -> SuperBlockWrapper<'a, Clean, Op> {
            clwb(sb, size_of::<HayleyfsSuperBlock>(), false);
            SuperBlockWrapper::new(sb)
        }
    }

    impl<'a> SuperBlockWrapper<'a, Clean, Read> {
        pub(crate) fn init(
            sbi: &SbInfo,
            _: &hayleyfs_bitmap::BitmapWrapper<'a, Clean, Alloc, Data>,
        ) -> SuperBlockWrapper<'a, Clean, Alloc> {
            let sb = unsafe { &mut *(sbi.virt_addr as *mut HayleyfsSuperBlock) };
            sb.size = sbi.pm_size;
            sb.blocksize = u32::try_from(PAGE_SIZE).unwrap(); // can be reasonably confident this won't panic
            sb.magic = HAYLEYFS_MAGIC;
            SuperBlockWrapper::<'a, Clean, _>::new_flush(sb)
        }
    }
}

pub(crate) fn hayleyfs_get_sbi(sb: *mut super_block) -> &'static mut SbInfo {
    let sbi: &mut SbInfo = unsafe { &mut *((*sb).s_fs_info as *mut SbInfo) };
    sbi
}

pub(crate) fn set_nlink_safe(inode: &mut inode, n: u32) {
    unsafe { set_nlink(inode, n) };
}

pub(crate) fn get_cacheline_num(val: usize) -> usize {
    val >> CACHELINE_BIT_SHIFT
}
