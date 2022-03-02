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
    // TODO: right now we look up the same inodes more than once. should
    // look them up ONCE, store the relevant information in memory, and use
    // DRAM structures to organize them for fast access
    // TODO: right now this only works with directories. when we can create files,
    // need to handle that differently

    let mut inuse_inos = Vec::<InodeNum>::new();
    let mut inuse_pages = Vec::<PmPage>::new();
    let mut valid_inos = Vec::<InodeNum>::new();
    let mut invalid_inos = Vec::<InodeNum>::new();
    let mut valid_pages = Vec::<PmPage>::new();
    let mut invalid_pages = Vec::<PmPage>::new();

    let inode_bitmap = get_inode_bitmap(&sbi) as *mut _ as *mut c_void;
    let data_bitmap = get_data_bitmap(&sbi) as *mut _ as *mut c_void;
    for bit in 0..PAGE_SIZE * 8 {
        if unsafe { hayleyfs_test_bit(bit, inode_bitmap) } == 1 {
            inuse_inos.try_push(bit)?;
        }
        if unsafe { hayleyfs_test_bit(bit, data_bitmap) } == 1 {
            inuse_pages.try_push(bit)?;
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
            // just because this page number is pointed to by a well-formed inode does NOT mean
            // that the page is actually valid and in-use. an inode isn't valid until its parent
            // points to it, but its internal directory contents are set up before the parent
            // points to it
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
    let cacheline_offset_mask = (1 << CACHELINE_BIT_SHIFT) - 1;
    let mut bitmap = get_inode_bitmap(&sbi);
    for token in zero_token_vec {
        let ino = token.get_ino();
        let cache_line = bitmap.get_bitmap_cacheline(ino);
        let cache_line_num = ino >> CACHELINE_BIT_SHIFT;
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

    let inode_bitmap_token = BitmapToken::new(bitmap, modified_cache_lines);

    // since we aren't considering crashes during initialization right now,
    // assume that the reserved pages are always valid
    valid_pages.try_push(SUPER_BLOCK_PAGE);
    valid_pages.try_push(INODE_BITMAP_PAGE);
    valid_pages.try_push(INODE_PAGE);
    valid_pages.try_push(DATA_BITMAP_PAGE);

    // now that we actually know which inodes are valid, we can determine which pages are actually in use
    // we know that if an inode points to a page, that page has been filled in with the . and .. dentries
    // and we know that all inodes left in the system are valid. so we can just scan them to find the valid
    // data pages
    for ino in valid_inos {
        // TODO: don't look the inode up again from PM - get it from DRAM
        let mut pi = hayleyfs_get_inode_by_ino(&sbi, ino);
        let page_no = pi.get_data_page_no();
        if let Some(page_no) = page_no {
            valid_pages.try_push(page_no);
        }
    }

    pr_info!("valid pages: {:?}\n", valid_pages);

    for page in inuse_pages {
        if !valid_pages.contains(&page) {
            invalid_pages.try_push(page);
        }
    }

    pr_info!("invalid pages: {:?}\n", invalid_pages);

    // TODO: do we need to do anything with invalid pages other than clear their bits
    // in the bitmap?

    let mut modified_cache_lines = Vec::<usize>::new();
    let mut bitmap = get_data_bitmap(&sbi);
    for page in invalid_pages {
        let cache_line = bitmap.get_bitmap_cacheline(page);
        let cache_line_num = page >> CACHELINE_BIT_SHIFT;
        let cacheline_offset = cacheline_offset_mask & page;
        cache_line.set_at_offset(cacheline_offset);

        if !modified_cache_lines.contains(&cache_line_num) {
            modified_cache_lines.try_push(cache_line_num);
        }
    }

    let data_bitmap_token = BitmapToken::new(bitmap, modified_cache_lines);

    // TODO: do something with the bitmap tokens

    Ok(())
}
