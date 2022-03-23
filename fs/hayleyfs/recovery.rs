use crate::def::*;
use crate::dir::hayleyfs_dir::*;
use crate::dir::*;
use crate::inode_def::hayleyfs_inode::*;
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

    let mut valid_inos = RBTree::<InodeNum, ()>::new();
    let mut valid_pages = RBTree::<PmPage, ()>::new();
    let mut search_stack = Vec::<InodeNum>::new();
    search_stack.try_push(ROOT_INO)?;
    valid_inos.try_insert(ROOT_INO, ())?;
    // TODO: there should potentially be checks that we didn't crash during
    // init (in which case we would have to re-initialize the system)
    valid_pages.try_insert(SUPER_BLOCK_PAGE, ())?;
    valid_pages.try_insert(INODE_BITMAP_PAGE, ())?;
    valid_pages.try_insert(INODE_PAGE, ())?;
    valid_pages.try_insert(DATA_BITMAP_PAGE, ())?;

    while !search_stack.is_empty() {
        // pop an inode off the stack; if it's a directory, read its dentries and
        // add them to the search stack if they aren't already marked valid
        let ino = search_stack.pop().unwrap();
        let pi = InodeWrapper::read_inode(sbi, &ino);
        if pi.is_dir() {
            let page_no = pi.get_data_page_no();
            match page_no {
                Some(page_no) => {
                    // iterate over children, skipping . and .. entries
                    // check that the . and .. entries are valid and well formed;
                    // if they aren't, indicates a bug
                    if valid_pages.get(&page_no).is_none() {
                        valid_pages.try_insert(page_no, ())?;
                        let mut dir_page = DirPageWrapper::read_dir_page(sbi, page_no);
                        // used to sanity check that there is exactly one . and exactly one .. in each directory
                        let mut self_dentry = false;
                        let mut parent_dentry = false;
                        for dentry in dir_page.iter_mut() {
                            if dentry.is_valid() {
                                if compare_dentry_name(dentry.get_name(), b".") {
                                    if self_dentry {
                                        pr_alert!(
                                            "ERROR: Inode {:?} has more than one . entry",
                                            ino
                                        );
                                        return Err(Error::EPERM);
                                    }
                                    self_dentry = true;
                                } else if compare_dentry_name(dentry.get_name(), b"..") {
                                    if parent_dentry {
                                        pr_alert!(
                                            "ERROR: Inode {:?} has more than one .. entry",
                                            ino
                                        );
                                        return Err(Error::EPERM);
                                    }
                                    parent_dentry = true;
                                } else {
                                    let child_ino = dentry.get_ino();
                                    if valid_inos.get(&child_ino).is_none() {
                                        search_stack.try_push(child_ino)?;
                                        valid_inos.try_insert(child_ino, ())?;
                                    }
                                }
                            }
                        }
                        if !self_dentry {
                            pr_alert!("ERROR: Inode {:?} does not contain . entry", ino);
                            return Err(Error::EPERM);
                        }
                        if !parent_dentry {
                            pr_alert!("ERROR: Inode {:?} does not contain .. entry", ino);
                            return Err(Error::EPERM);
                        }
                    }
                }
                None => {
                    pr_info!("ERROR: Corrupted inode {:?} - directory is pointed to by parent but does not point to a data page", ino);
                    return Err(Error::EPERM);
                }
            }
        }
    }

    // now - we know if there are any invalid inodes and pages.
    // let's invalidate dentries in invalid pages (if there are any valid ones) and
    // zero out invalid inodes
    // TODO: do you have to do that for correctness?

    for ino in in_use_inos.keys() {
        if valid_inos.get(ino).is_none() {
            let pi = InodeWrapper::read_inode(sbi, ino);
            pi.zero_inode();
            // TODO: we need a way to make sure we don't clear the bits
            // in the bitmap until after this has been done
        }
    }

    Ok(())
}
