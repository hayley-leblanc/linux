#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]
#![allow(unused)]
#![allow(clippy::needless_borrow)]

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

pub(crate) type PmPage = usize;

// TODO: probably a better way to manage this
pub(crate) struct DataPage {
    data: *const c_void,
}

// TODO: do you want to use an array? or something else?
pub(crate) struct DirPage {
    pub(crate) dentries: [HayleyfsDentry; DENTRIES_PER_PAGE],
}

#[no_mangle]
pub(crate) static mut HayleyfsFileOps: file_operations = file_operations {
    iterate: Some(hayleyfs_readdir),
    ..c_default_struct!(file_operations)
};

#[repr(packed)]
pub(crate) struct HayleyfsDentry {
    pub(crate) valid: bool,
    pub(crate) ino: InodeNum,
    pub(crate) name: [u8; MAX_FILENAME_LEN],
    // is this going to live in the correct place?
    // TODO: what's the best way to handle file names here? they need to live
    // IN this struct, not be pointed to by something else
    pub(crate) link_count: u16,
    pub(crate) name_len: usize,
}

fn get_data_bitmap_addr(sbi: &SbInfo) -> *mut c_void {
    (sbi.virt_addr as usize + (DATA_BITMAP_PAGE * PAGE_SIZE)) as *mut c_void
}

pub(crate) fn get_data_page_addr(sbi: &SbInfo, page_no: usize) -> *mut c_void {
    (sbi.virt_addr as usize + ((DATA_START + page_no) * PAGE_SIZE)) as *mut c_void
}

/// page_no should be a relative page number, NOT absolute.
/// so the first valid data page number is 0, but it's actually
/// the 5th page (since the first four pages are reserved)
/// this will make it easier to manage the bitmap
fn set_data_bitmap_bit(sbi: &mut SbInfo, page_no: PmPage) -> Result<()> {
    // TODO: return an error if the page number is not valid
    let addr = get_data_bitmap_addr(sbi);
    unsafe { hayleyfs_set_bit(page_no, addr) };
    // TODO: only flush the updated cache line
    clflush(addr as *const c_void, PAGE_SIZE, true);
    Ok(())
}

/// this lives here for now because it deals with allocating and managing
/// data pages
pub(crate) fn initialize_dir(
    sbi: &SbInfo,
    pi: &mut HayleyfsInode,
    self_ino: InodeNum,
    parent_ino: InodeNum,
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
    let dir_page = unsafe { &mut *(get_data_page_addr(sbi, page_no) as *mut DirPage) };

    // TODO: dropping arrays might get weird. you might need to rethink this
    let mut dentry = &mut dir_page.dentries[0];

    set_dentry_name(".", dentry);
    dentry.ino = self_ino;
    dentry.valid = true;
    dentry.link_count = 1;

    clflush(dentry, size_of::<HayleyfsDentry>(), false);

    let mut dentry = &mut dir_page.dentries[1];
    set_dentry_name("..", dentry);
    dentry.ino = parent_ino;
    dentry.valid = true;
    dentry.link_count = 2;

    clflush(dentry, size_of::<HayleyfsDentry>(), false);

    // TODO: don't do this unsafely right here. or at least manage the
    // dependencies properly
    unsafe { pi.set_data_page_no(Some(page_no)) };

    clflush(pi, size_of::<HayleyfsInode>(), true);

    let data_bitmap_addr = get_data_bitmap_addr(sbi);
    unsafe { hayleyfs_set_bit(page_no, data_bitmap_addr) };

    clflush(data_bitmap_addr, PAGE_SIZE, true);

    Ok(())
}

fn set_dentry_name(name: &str, dentry: &mut HayleyfsDentry) {
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
    // TODO: this is suggested by the compiler, does it work properly?
    dentry.name[..num_bytes].clone_from_slice(&name[..num_bytes]);
    // for i in 0..num_bytes {
    //     dentry.name[i] = name[i];
    // }
}

// TODO: use a better way to handle these slices so things don't get weird
// when there are different lengths
// there has to be a nicer way to handle these strings in general
pub(crate) fn compare_dentry_name(name1: &[u8], name2: &[u8]) -> bool {
    let (min_len, longer_name) = if name1.len() > name2.len() {
        (name2.len(), name1)
    } else {
        (name1.len(), name2)
    };
    for i in 0..MAX_FILENAME_LEN {
        if (i < min_len) {
            if name1[i] != name2[i] {
                return false;
            }
        } else if longer_name[i] != 0 {
            return false;
        }
    }
    true
}

// TODO: there's probably a better way to handle the name string here?
// current way works though. but there is a lot of conversion going on
pub(crate) fn add_dentry_to_parent(
    sbi: &SbInfo,
    parent_dir: &mut HayleyfsInode,
    ino: usize,
    name: &kernel::str::CStr,
) -> Result<()> {
    // find the next open dentry slot
    match parent_dir.get_data_page_no() {
        Some(page_no) => {
            let dir_page = unsafe { &mut *(get_data_page_addr(sbi, page_no) as *mut DirPage) };
            for i in 0..DENTRIES_PER_PAGE {
                let mut dentry = &mut dir_page.dentries[i];
                if !dentry.valid {
                    // we can't actually take ownership of this dentry since it lives in PM
                    // can you?? i have no clue
                    // TODO: is that going to be a problem...
                    set_dentry_name(name.to_str().unwrap(), &mut dentry);
                    dentry.ino = ino;
                    dentry.valid = true;
                    dentry.link_count = 2;
                    clflush(dentry, size_of::<HayleyfsDentry>(), true);

                    // TODO: fix safety
                    unsafe { parent_dir.inc_links() };
                    clflush(parent_dir, size_of::<HayleyfsInode>(), true);

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

    // TODO: do these conversions and then call a safe function to do the actual op
    let inode = unsafe { &mut *(hayleyfs_file_inode(file) as *mut inode) };
    let sb = unsafe { (*inode).i_sb };
    let sbi = hayleyfs_get_sbi(sb);
    let pi = hayleyfs_get_inode_by_ino(&sbi, inode.i_ino.try_into().unwrap());
    let ctx = unsafe { &mut *(ctx_raw as *mut dir_context) };

    if ctx.pos == READDIR_END {
        return 0;
    }

    match pi.get_data_page_no() {
        Some(page) => {
            // iterate over the dentries in the file and feed them to dir_emit
            // right now there can only be one page of directory entries
            let dir_page = unsafe { &mut *(get_data_page_addr(sbi, page) as *mut DirPage) };

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
                        pi.get_ino().try_into().unwrap(),
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
