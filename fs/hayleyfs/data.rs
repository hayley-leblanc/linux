#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]
#![allow(unused)]

use crate::defs::*;
use crate::inode_rs::*;
use crate::pm::*;
use crate::super_def::*;
use core::mem::size_of;
use kernel::bindings::{dentry, dir_context, file, file_operations, inode, ENOTDIR};
use kernel::c_types::{c_int, c_void};
use kernel::prelude::*;
use kernel::str::CStr;
use kernel::{c_default_struct, c_str, PAGE_SIZE};

pub(crate) type pm_page = usize;

// TODO: probably a better way to manage this
pub(crate) struct data_page {
    data: *const c_void,
}

// TODO: do you want to use an array? or something else?
pub(crate) struct dir_page {
    dentries: [hayleyfs_dentry; DENTRIES_PER_PAGE],
}

#[no_mangle]
pub(crate) static mut hayleyfs_file_ops: file_operations = file_operations {
    iterate: Some(hayleyfs_readdir),
    ..c_default_struct!(file_operations)
};

#[repr(packed)]
pub(crate) struct hayleyfs_dentry {
    pub(crate) valid: bool,
    pub(crate) ino: usize,
    pub(crate) name: [u8; MAX_FILENAME_LEN],
    // is this going to live in the correct place?
    // TODO: what's the best way to handle file names here? they need to live
    // IN this struct, not be pointed to by something else
    pub(crate) link_count: u16,
    pub(crate) name_len: usize,
}

fn get_data_bitmap_addr(sbi: &hayleyfs_sb_info) -> *mut c_void {
    (sbi.virt_addr as usize + (DATA_BITMAP_PAGE * PAGE_SIZE)) as *mut c_void
}

fn get_data_page_addr(sbi: &hayleyfs_sb_info, page_no: usize) -> *mut c_void {
    (sbi.virt_addr as usize + ((DATA_START + page_no) * PAGE_SIZE)) as *mut c_void
}

/// page_no should be a relative page number, NOT absolute.
/// so the first valid data page number is 0, but it's actually
/// the 5th page (since the first four pages are reserved)
/// this will make it easier to manage the bitmap
fn set_data_bitmap_bit(sbi: &mut hayleyfs_sb_info, page_no: usize) -> Result<()> {
    // TODO: return an error if the page number is not valid
    let addr = get_data_bitmap_addr(sbi);
    unsafe { hayleyfs_set_bit(page_no.try_into().unwrap(), addr) };
    // TODO: only flush the updated cache line
    clflush(addr as *const c_void, PAGE_SIZE, true);
    Ok(())
}

/// this lives here for now because it deals with allocating and managing
/// data pages
pub(crate) fn initialize_dir(
    sbi: &hayleyfs_sb_info,
    pi: &mut hayleyfs_inode,
    self_ino: usize,
    parent_ino: usize,
) -> Result<()> {
    let bitmap_addr = get_data_bitmap_addr(sbi);
    // we are initializing a directory, so allocate a data page for its dentries
    // find an empty page based on the bitmap
    // last argument is supposed to be bitmap size IN BITS
    let page_no = unsafe {
        hayleyfs_find_next_zero_bit(
            bitmap_addr as *mut u64,
            0,
            (PAGE_SIZE * 8).try_into().unwrap(),
        )
    };

    // if there are no zero bits left, page_no will be equal to the size parameter
    if page_no == (PAGE_SIZE * 8) {
        return Err(Error::ENOSPC);
    }

    // TODO: you should make sure that the page is zeroed out first?
    // I have no idea if Rust will do anything helpful there
    let dir_page = unsafe { &mut *(get_data_page_addr(sbi, page_no) as *mut dir_page) };

    // TODO: dropping arrays might get weird. you might need to rethink this
    let mut dentry = &mut dir_page.dentries[0];

    set_dentry_name(".", dentry);
    dentry.ino = self_ino;
    dentry.valid = true;
    dentry.link_count = 1;

    clflush(dentry, size_of::<hayleyfs_dentry>(), false);

    let mut dentry = &mut dir_page.dentries[1];
    set_dentry_name("..", dentry);
    dentry.ino = parent_ino;
    dentry.valid = true;
    dentry.link_count = 2;

    clflush(dentry, size_of::<hayleyfs_dentry>(), false);

    // don't set the block as in use until we are done setting up the dentries
    // there is a dependency between setting the pointer in the inode and setting the bitmap
    // but an fsck should be able to take care of that easily
    pi.data0 = Some(page_no);

    clflush(pi, size_of::<hayleyfs_inode>(), true);

    let data_bitmap_addr = get_data_bitmap_addr(sbi);
    unsafe { hayleyfs_set_bit(page_no, data_bitmap_addr) };

    clflush(data_bitmap_addr, PAGE_SIZE, true);

    Ok(())
}

fn set_dentry_name(name: &str, dentry: &mut hayleyfs_dentry) {
    // initialize the name array with zeroes, then set the name
    dentry.name = [0; MAX_FILENAME_LEN];
    // ensure it's null terminated by only copying at most MAX_FILENAME_LEN-1 bytes
    let num_bytes = if name.len() < MAX_FILENAME_LEN - 1 {
        name.len()
    } else {
        MAX_FILENAME_LEN - 1
    };
    dentry.name_len = num_bytes + 1;
    // TODO: this will not work with non-ascii characters
    // TODO: might be able to simplify this with a built in strcpy function
    let name = name.as_bytes();
    for i in 0..num_bytes {
        dentry.name[i] = name[i];
    }
}

// TODO: there's probably a better way to handle the name string here?
// current way works though. but there is a lot of conversion going on
pub(crate) fn add_dentry_to_parent(
    sbi: &hayleyfs_sb_info,
    parent_dir: &hayleyfs_inode,
    ino: usize,
    name: &kernel::str::CStr,
) -> Result<()> {
    // find the next open dentry slot
    match parent_dir.data0 {
        Some(page_no) => {
            let dir_page = unsafe { &mut *(get_data_page_addr(sbi, page_no) as *mut dir_page) };
            for i in 0..DENTRIES_PER_PAGE {
                let mut dentry = &mut dir_page.dentries[i];
                if !dentry.valid {
                    set_dentry_name(name.to_str().unwrap(), &mut dentry);
                    dentry.ino = ino;
                    dentry.valid = true;
                    dentry.link_count = 2;
                    clflush(dentry, size_of::<hayleyfs_dentry>(), true);

                    return Ok(());
                }
            }
            Err(Error::ENOSPC)
        }
        None => Err(Error::ENOTDIR),
    }
}

#[no_mangle]
unsafe extern "C" fn hayleyfs_readdir(file: *mut file, ctx_raw: *mut dir_context) -> i32 {
    // TODO: check that the file is actually a directory?
    // TODO: maybe should use in-memory inodes

    let inode = unsafe { &mut *(hayleyfs_file_inode(file) as *mut inode) };
    let sb = unsafe { (*inode).i_sb };
    let sbi = hayleyfs_get_sbi(sb);
    let pi = hayleyfs_get_inode_by_ino(&sbi, inode.i_ino.try_into().unwrap());
    let ctx = unsafe { &mut *(ctx_raw as *mut dir_context) };

    if ctx.pos == READDIR_END {
        return 0;
    }

    match pi.data0 {
        Some(page) => {
            // iterate over the dentries in the file and feed them to dir_emit
            // right now there can only be one page of directory entries
            let dir_page = unsafe { &mut *(get_data_page_addr(sbi, page) as *mut dir_page) };

            for i in 0..DENTRIES_PER_PAGE {
                let dentry = &dir_page.dentries[i];
                if !dentry.valid {
                    pr_info!("found invalid dentry at entry {}", i);
                    ctx.pos = READDIR_END;
                    return 0;
                }
                pr_info!("reading dentry {}\n", i);
                // TODO: what is the type parameter supposed to be? it doesn't seem to be
                // documented anywhere
                // TODO: the type conversions here are REAL weird and might break
                if unsafe {
                    !hayleyfs_dir_emit(
                        ctx,
                        dentry.name.as_ptr() as *const i8,
                        dentry.name_len.try_into().unwrap(),
                        pi.ino.try_into().unwrap(),
                        0,
                    )
                } {
                    return 0;
                }
            }
            ctx.pos = READDIR_END;
            0
        }
        None => -(ENOTDIR as c_int),
    }
}
