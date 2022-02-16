#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]
#![allow(unused)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::mut_from_ref)]

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

// impl BitmapIndex for PmPage {}

// TODO: probably a better way to manage this
pub(crate) struct DataPage {
    data: *const c_void,
}

// TODO: do you want to use an array? or something else?
pub(crate) struct DirPage {
    pub(crate) dentries: [HayleyfsDentry; DENTRIES_PER_PAGE],
}

pub(crate) struct DataAllocToken {
    page_no: PmPage,
    cache_line: *mut CacheLine,
}

impl DataAllocToken {
    pub(crate) unsafe fn new(p: PmPage, line: *mut CacheLine) -> Self {
        Self {
            page_no: p,
            cache_line: line,
        }
    }

    pub(crate) fn page_no(&self) -> PmPage {
        self.page_no
    }
}

impl Drop for DataAllocToken {
    fn drop(&mut self) {
        pr_info!("dropping alloc token for page {:?}\n", self.page_no);
        clflush(self.cache_line, CACHELINE_SIZE, false);
    }
}

pub(crate) struct DirInitToken<'a> {
    self_dentry: &'a HayleyfsDentry,
    parent_dentry: &'a HayleyfsDentry,
}

impl<'a> DirInitToken<'a> {
    pub(crate) unsafe fn new(s: &'a mut HayleyfsDentry, p: &'a mut HayleyfsDentry) -> Self {
        Self {
            self_dentry: s,
            parent_dentry: p,
        }
    }
}

impl Drop for DirInitToken<'_> {
    fn drop(&mut self) {
        pr_info!("dropping dir init token!\n");
        // flush them separately in case there is some unexpected padding
        // this could cause redundant flushes
        clflush(self.self_dentry, size_of::<HayleyfsDentry>(), false);
        clflush(self.parent_dentry, size_of::<HayleyfsDentry>(), false);
    }
}

#[no_mangle]
pub(crate) static mut HayleyfsFileOps: file_operations = file_operations {
    iterate: Some(hayleyfs_readdir),
    ..c_default_struct!(file_operations)
};

#[repr(packed)]
pub(crate) struct HayleyfsDentry {
    valid: bool,
    ino: InodeNum,
    name: [u8; MAX_FILENAME_LEN],
    // is this going to live in the correct place?
    // TODO: what's the best way to handle file names here? they need to live
    // IN this struct, not be pointed to by something else
    link_count: u16,
    name_len: usize,
}

impl HayleyfsDentry {
    // TODO: why does link count live in dentries?
    fn set_up(&mut self, ino: InodeNum, name: &str, link_count: u16) {
        self.ino = ino;
        self.link_count = link_count;
        self.set_dentry_name(name);
        self.valid = true; // TODO: might not actually want to do this here
    }

    fn set_dentry_name(&mut self, name: &str) {
        // initialize the name array with zeroes, then set the name
        self.name = [0; MAX_FILENAME_LEN];
        // ensure it's null terminated by only copying at most MAX_FILENAME_LEN-1 bytes
        let num_bytes = if name.len() < MAX_FILENAME_LEN - 1 {
            name.len()
        } else {
            MAX_FILENAME_LEN - 1
        };
        self.name_len = num_bytes + 1;
        // TODO: this will not work with non-ascii characters
        let name = name.as_bytes();
        self.name[..num_bytes].clone_from_slice(&name[..num_bytes]);
    }

    pub(crate) fn is_valid(&self) -> bool {
        self.valid
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    pub(crate) fn get_name(&self) -> &[u8] {
        &self.name
    }
}

// TODO: can you phase this out and replace with nicer functions
fn get_data_bitmap_addr(sbi: &SbInfo) -> *mut c_void {
    (sbi.virt_addr as usize + (DATA_BITMAP_PAGE * PAGE_SIZE)) as *mut c_void
}

fn get_data_bitmap(sbi: &SbInfo) -> &mut PersistentBitmap {
    unsafe {
        &mut *((sbi.virt_addr as usize + (DATA_BITMAP_PAGE * PAGE_SIZE)) as *mut PersistentBitmap)
    }
}

pub(crate) fn get_data_page_addr(sbi: &SbInfo, page_no: PmPage) -> *mut c_void {
    (sbi.virt_addr as usize + ((DATA_START + page_no) * PAGE_SIZE)) as *mut c_void
}

/// page_no should be a relative page number, NOT absolute.
/// so the first valid data page number is 0, but it's actually
/// the 5th page (since the first four pages are reserved)
/// this will make it easier to manage the bitmap
/// TODO: phase this out or make it unsafe - doesn't work well with tokens
fn set_data_bitmap_bit(sbi: &mut SbInfo, page_no: PmPage) -> Result<()> {
    // TODO: return an error if the page number is not valid
    let addr = get_data_bitmap_addr(sbi);
    unsafe { hayleyfs_set_bit(page_no, addr) };
    // TODO: only flush the updated cache line
    clflush(addr as *const c_void, PAGE_SIZE, true);
    Ok(())
}

pub(crate) fn hayleyfs_alloc_page(sbi: &SbInfo) -> Result<DataAllocToken> {
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

    // TODO: like in inode, this is redundant. take care of it
    let mut bitmap = get_data_bitmap(&sbi);
    unsafe { hayleyfs_set_bit(page_no, bitmap as *mut _ as *mut c_void) };
    let cacheline = get_bitmap_cacheline(&mut bitmap, page_no);

    let token = unsafe { DataAllocToken::new(page_no, cacheline) };

    Ok(token)
}

/// this lives here for now because it deals with allocating and managing
/// data pages
pub(crate) fn initialize_dir<'a>(
    sbi: &SbInfo,
    // pi: &mut HayleyfsInode,
    ino_token: &mut InodeInitToken<'_>,
    // self_ino: InodeNum,
    parent_ino: InodeNum,
    alloc_token: &'a DataAllocToken,
) -> Result<DirInitToken<'a>> {
    let dir_page = unsafe { &mut *(get_data_page_addr(sbi, alloc_token.page_no) as *mut DirPage) };

    // TODO: confirm that split_at_mut is just an ownership/mutability thing
    // and doesn't make copies
    let (d1, d2) = dir_page.dentries.split_at_mut(1);

    let mut self_dentry = &mut d1[0];

    self_dentry.set_up(ino_token.get_ino(), ".", 1);

    let mut parent_dentry = &mut d2[0];

    parent_dentry.set_up(parent_ino, "..", 2);

    let init_token = DirInitToken {
        self_dentry,
        parent_dentry,
    };

    // add the data page we have just set up to the inode
    // TODO: i don't THINK doing this here will cause issues with dependencies,
    // but do some testing to be sure
    ino_token.add_data_page(alloc_token.page_no);

    Ok(init_token)
}

// // TODO: where should the lifetime come from here
// pub(crate) fn add_data_page_to_inode<'a>(
//     inode_token: &'a mut InodeInitToken<'_>,
//     data_token: &DataAllocToken,
// ) -> Result<InodeDoneToken<'a>> {
// }

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

// // TODO: there's probably a better way to handle the name string here?
// // current way works though. but there is a lot of conversion going on
// pub(crate) fn add_dentry_to_parent(
//     sbi: &SbInfo,
//     parent_dir: &mut HayleyfsInode,
//     ino: usize,
//     name: &kernel::str::CStr,
// ) -> Result<()> {
//     // find the next open dentry slot
//     match parent_dir.get_data_page_no() {
//         Some(page_no) => {
//             let dir_page = unsafe { &mut *(get_data_page_addr(sbi, page_no) as *mut DirPage) };
//             for i in 0..DENTRIES_PER_PAGE {
//                 let mut dentry = &mut dir_page.dentries[i];
//                 if !dentry.valid {
//                     // we can't actually take ownership of this dentry since it lives in PM
//                     // can you?? i have no clue
//                     // TODO: is that going to be a problem...
//                     set_dentry_name(name.to_str().unwrap(), &mut dentry);
//                     dentry.ino = ino;
//                     dentry.valid = true;
//                     dentry.link_count = 2;
//                     clflush(dentry, size_of::<HayleyfsDentry>(), true);

//                     // TODO: fix safety
//                     unsafe { parent_dir.inc_links() };
//                     clflush(parent_dir, size_of::<HayleyfsInode>(), true);

//                     return Ok(());
//                 }
//             }
//             Err(Error::ENOSPC)
//         }
//         None => Err(Error::ENOTDIR),
//     }
// }

// deal with soft updates
#[no_mangle]
unsafe extern "C" fn hayleyfs_readdir(file: *mut file, ctx_raw: *mut dir_context) -> i32 {
    // TODO: check that the file is actually a directory?
    // TODO: maybe should use in-memory inodes

    // TODO: do these conversions and then call a safe function to do the actual op
    let inode = unsafe { &mut *(hayleyfs_file_inode(file) as *mut inode) };
    let sb = unsafe { (*inode).i_sb };
    let sbi = hayleyfs_get_sbi(sb);
    let pi = unsafe { hayleyfs_get_inode_by_ino(&sbi, inode.i_ino.try_into().unwrap()) };
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
