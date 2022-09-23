#![allow(dead_code)] // TODO: remove

use crate::{cdefs::*, super_def::*};
use core::{ffi, marker::PhantomData};
use kernel::prelude::*;
use kernel::sync::smutex::Mutex;
use kernel::PAGE_SIZE;

/// Trait for inode allocators to implement. Using a trait should make it easy
/// to experiment with different allocator implementations.
pub(crate) trait InodeAllocator {
    fn alloc_ino(&mut self) -> Result<InodeNum>;
    fn dealloc_ino(&mut self, ino: InodeNum) -> Result<()>;
}

/// Simple bitmap of inodes for allocation. This lives in DRAM
#[repr(C)]
pub(crate) struct InodeBitmap(Mutex<[u8; INODE_BITMAP_SIZE]>);

impl InodeBitmap {
    pub(crate) fn new() -> Self {
        let bitmap: [u8; INODE_BITMAP_SIZE] = [0; INODE_BITMAP_SIZE];
        // set bits for reserved inodes
        // SAFETY: We just initialized the bitmap as an array of zeros so it is
        // safe to index into. ROOT_INO is 1 and the bitmap will always be non-zero-sized.
        unsafe {
            set_bit_helper(
                ROOT_INO.try_into().unwrap(),
                bitmap.as_ptr() as *const ffi::c_void,
            )
        };
        Self(Mutex::new(bitmap))
    }
}

impl InodeAllocator for InodeBitmap {
    fn alloc_ino(&mut self) -> Result<InodeNum> {
        let bitmap = self.0.lock();
        // find a zero bit in the bitmap
        let ino = unsafe {
            find_next_zero_bit_le_helper(
                bitmap.as_ptr() as *const ffi::c_ulong,
                INODE_BITMAP_SIZE.try_into()?,
                0,
            )
        };
        if ino > TOTAL_INODES.try_into()? {
            Err(ENOSPC)
        } else {
            let set = unsafe {
                test_and_set_bit_le_helper(ino.try_into()?, bitmap.as_ptr() as *const ffi::c_void)
            };
            if set != 0 {
                pr_err!("ERROR: ino {} is already set", ino);
                Err(EINVAL)
            } else {
                Ok(ino)
            }
        }
    }

    fn dealloc_ino(&mut self, ino: InodeNum) -> Result<()> {
        let bitmap = self.0.lock();
        let set = unsafe {
            test_and_clear_bit_le_helper(ino.try_into()?, bitmap.as_ptr() as *const ffi::c_void)
        };
        if set == 0 {
            pr_err!("ERROR: ino {} is already free", ino);
            Err(EINVAL)
        } else {
            Ok(())
        }
    }
}

/// Persistent inode structure
#[derive(Debug)]
#[repr(C)]
struct HInode {
    mode: u16,
    link_count: u16,
    ctime: u32,
    mtime: u32,
    atime: u32,
    flags: u32,
    uid: u32,
    gid: u32,
    rdev: u32,
    size: u64,
    ino: InodeNum,
}

#[must_use]
#[derive(Debug)]
pub(crate) struct InodeWrapper<'a, State, Op> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    inode: &'a HInode,
}

impl<'a> InodeWrapper<'a, Clean, Free> {
    /// Given an inode number, reads that inode from PM.
    /// If the inode is not actually free, returns an error.
    pub(crate) fn get_free_inode(sbi: SbInfo, ino: InodeNum) -> Result<Self> {
        // Safety:
        // - TODO: need sanity checks on the size of the device; make sure the inode table is not
        //   greater than the device
        if ino > TOTAL_INODES.try_into()? {
            Err(ENOSPC)
        } else {
            let inode = unsafe {
                let pm_addr = sbi.danger_get_pm_addr();
                // get address of the beginning of the inode table
                let inode_table_addr: *mut ffi::c_void =
                    pm_addr.offset((PAGE_SIZE * INODE_TABLE_START).try_into()?);
                let ino_usize: usize = ino.try_into()?; // required to satisfy the type system in the next step
                let offset: isize = (INODE_ENTRY_SIZE * ino_usize).try_into()?;
                let inode_raw = inode_table_addr.offset(offset);
                &*(inode_raw as *mut HInode)
            };
            Ok(InodeWrapper {
                state: PhantomData,
                op: PhantomData,
                inode,
            })
        }
    }
}
