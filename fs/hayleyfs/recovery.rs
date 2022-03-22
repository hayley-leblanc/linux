use crate::def::*;
use crate::inode_def::*;
use crate::super_def::hayleyfs_bitmap::*;
use crate::super_def::*;
use kernel::prelude::*;
use kernel::rbtree::RBTree;
use kernel::PAGE_SIZE;

pub(crate) fn hayleyfs_recovery(sbi: &mut SbInfo) -> Result<()> {
    // trying out using rbtrees, rather than vectors, to store info here

    let mut in_use_inos = RBTree::<InodeNum, ()>::new();
    let mut in_use_pages = RBTree::<PmPage, ()>::new();

    // figure out what inodes are marked in use
    let inode_bitmap = BitmapWrapper::read_inode_bitmap(sbi);
    let data_bitmap = BitmapWrapper::read_data_bitmap(sbi);
    for bit in 0..PAGE_SIZE * 8 {
        if inode_bitmap.check_bit(bit) {
            in_use_inos.try_insert(bit, ())?;
        }
        if data_bitmap.check_bit(bit) {
            in_use_pages.try_insert(bit, ())?;
        }
    }

    // traverse the file tree and mark any inode that is pointed to by a valid
    // dentry as valid
    // simultaneously mark pages that are pointed to by valid inodes as valid

    Ok(())
}
