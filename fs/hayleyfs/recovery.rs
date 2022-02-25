use crate::data::*;
use crate::defs::*;
use crate::inode_rs::*;
use crate::super_def::*;
use crate::tokens::*;
use kernel::bindings::S_IFDIR;
use kernel::c_types::c_void;
use kernel::prelude::*;
use kernel::PAGE_SIZE;

pub(crate) fn hayleyfs_recovery(sbi: &mut SbInfo) -> Result<()> {
    // first, put together information about which inodes and data pages
    // are expected to be in use.
    // TODO: store in a structure other than a vector - in memory set would
    // be easiest from programming perspective. Not sure what would be fastest

    let mut inuse_inos = Vec::<InodeNum>::new();
    let mut inuse_pages = Vec::<PmPage>::new();
    let mut valid_inos = Vec::<InodeNum>::new();
    let mut invalid_inos = Vec::<InodeNum>::new();

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

    pr_info!("in use inodes: {:?}\n", inuse_inos);
    pr_info!("in use pages: {:?}\n", inuse_pages);

    // an inode is valid if there is a pointer to it in in a dentry.
    // TODO: is it possible for an inode to be pointed to by a dentry
    // but the parent inode is invalid? I don't THINK so as long as
    // we do recursive deletion of directories, but keep it in mind.
    // so we need to scan the dentries of each file and make a list of
    // inodes that have at least one pointer

    // then, zero out all inodes that do NOT have a pointer to them,
    // make a token, and use that token to set the bitmap properly

    for ino in &inuse_inos {
        let pi = hayleyfs_get_inode_by_ino(&sbi, *ino);
        if pi.get_mode() & S_IFDIR != 0 {
            let page_no = pi.get_data_page_no();
            match page_no {
                Some(page_no) => {
                    let dir_page = get_dir_page(*sbi, page_no);
                    let mut inos = dir_page.get_inos();
                    // TODO: there is DEFINITELY a more efficient built in way to do this
                    let mut i = 0;
                    while i < inos.len() {
                        let val = inos.remove(i);
                        // TODO: use a set so you don't have to do this
                        if !valid_inos.contains(&val) {
                            valid_inos.try_push(val);
                        }
                    }
                }
                None => continue,
            }
        }
    }

    pr_info!("valid inodes: {:?}\n", valid_inos);

    for ino in &inuse_inos {
        if !valid_inos.contains(&ino) {
            invalid_inos.try_push(*ino);
        }
    }

    pr_info!("invalid inodes: {:?}\n", invalid_inos);

    // TODO: could we use the same token type as we use for allocation?

    let mut zero_token_vec = Vec::<InodeZeroToken<'_>>::new();
    for ino in &invalid_inos {
        let mut pi = hayleyfs_get_inode_by_ino(&sbi, *ino);
        let zero_token = pi.zero_inode();
        zero_token_vec.try_push(zero_token);
    }

    // now that we have zeroed the inodes, we can set them as unused in the bitmap

    // make a list of which cache lines have been modified and flush them at the end
    // TODO: should be a set or something, not a vector
    let mut modified_cache_lines = Vec::<usize>::new();
    let cacheline_offset_mask = 0x1ff; // 511
    let mut bitmap = get_inode_bitmap(&sbi);
    for token in zero_token_vec {
        let ino = token.get_ino();
        let cache_line = bitmap.get_bitmap_cacheline(ino);
        let cache_line_num = ino >> 9;
        let cacheline_offset = cacheline_offset_mask & ino;
        cache_line.set_at_offset(cacheline_offset);
        // there doesn't seem to be a nice way to check if two cache line pointers are the same
        // even if there were, Rust's borrow checker might not let us do it
        // so instead, save the cache line num within the bitmap
        // and we'll go back and set up tokens at the end
        if !modified_cache_lines.contains(&cache_line_num) {
            modified_cache_lines.try_push(cache_line_num);
        }
    }

    pr_info!("modified cache lines: {:?}\n", modified_cache_lines);

    let bitmap_token = BitmapToken::new(bitmap, modified_cache_lines);

    Ok(())
}
