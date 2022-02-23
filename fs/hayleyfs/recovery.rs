use crate::data::*;
use crate::defs::*;
use crate::inode_rs::*;
use crate::super_def::*;
use kernel::c_types::c_void;
use kernel::prelude::*;
use kernel::PAGE_SIZE;

pub(crate) fn hayleyfs_recovery(sbi: &mut SbInfo) -> Result<()> {
    // first, put together information about which inodes and data pages
    // are expected to be in use.
    // TODO: store in a structure other than a vector - in memory bitmap,
    // or free list, or something

    let mut inuse_inos = Vec::<InodeNum>::new();
    let mut inuse_pages = Vec::<PmPage>::new();

    let mut bad_inodes = Vec::<&mut HayleyfsInode>::new();

    let inode_bitmap = get_inode_bitmap(&sbi) as *mut _ as *mut c_void;
    let data_bitmap = get_data_bitmap(&sbi) as *mut _ as *mut c_void;
    for bit in 0..PAGE_SIZE * 8 {
        if unsafe { hayleyfs_test_bit(bit, inode_bitmap) } == 1 {
            inuse_inos.try_push(bit)?;
        }
        if unsafe { hayleyfs_test_bit(bit, data_bitmap) } == 1 {
            inuse_pages.try_push(bit + DATA_START)?;
        }
    }

    pr_info!("{:?}\n", inuse_inos);
    pr_info!("{:?}\n", inuse_pages);

    for ino in inuse_inos {
        // obtain the inode and check whether it is valid. IF IT IS NOT,
        // zero it out, flush that, then mark it free in the bitmap
        // would be fastest to zero bad inodes first, then update the bitmap;
        // would require the fewest fences
        // TODO: should have a token to enforce this ordering
        let pi = hayleyfs_get_inode_by_ino(&sbi, ino);
        if !pi.is_valid() {
            bad_inodes.try_push(pi);
        }
    }

    Ok(())
}
