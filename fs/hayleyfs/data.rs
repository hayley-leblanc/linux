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
    dentries: [HayleyfsDentry; DENTRIES_PER_PAGE],
}

impl DirPage {
    fn get_next_invalid_dentry(&mut self) -> Result<DentryAddToken<'_>> {
        for dentry in self.dentries.iter_mut() {
            if !dentry.valid {
                let dentry_token = unsafe { DentryAddToken::new(dentry) };
                return Ok(dentry_token);
            }
        }
        Err(Error::ENOSPC)
    }

    pub(crate) fn lookup_name(&self, name: &[u8]) -> Result<DentryReadToken<'_>> {
        for dentry in self.dentries.iter() {
            if !dentry.is_valid() {
                return Err(Error::ENOENT);
            } else if compare_dentry_name(dentry.get_name(), name) {
                let token = unsafe { DentryReadToken::new(dentry) };
                return Ok(token);
            }
        }
        Err(Error::ENOENT)
    }
}

pub(crate) struct DataAllocToken {
    page_no: PmPage,
    cache_line: *mut CacheLine,
}

impl DataAllocToken {
    pub(crate) unsafe fn new(p: PmPage, line: *mut CacheLine) -> Self {
        // TODO: fencing
        pr_info!("flushing alloc token for page {:?}\n", p);
        clflush(line, CACHELINE_SIZE, false);
        Self {
            page_no: p,
            cache_line: line,
        }
    }

    pub(crate) fn page_no(&self) -> PmPage {
        self.page_no
    }
}

// impl Drop for DataAllocToken {
//     fn drop(&mut self) {
//         pr_info!("dropping alloc token for page {:?}\n", self.page_no);
//         clflush(self.cache_line, CACHELINE_SIZE, false);
//     }
// }

pub(crate) struct DirInitToken<'a> {
    self_dentry: &'a HayleyfsDentry,
    parent_dentry: &'a HayleyfsDentry,
}

impl<'a> DirInitToken<'a> {
    pub(crate) unsafe fn new(s: &'a mut HayleyfsDentry, p: &'a mut HayleyfsDentry) -> Self {
        // TODO: fencing
        pr_info!("flushing dir init token!\n");
        // flush them separately in case there is some unexpected padding
        // this could cause redundant flushes
        clflush(s, size_of::<HayleyfsDentry>(), false);
        clflush(p, size_of::<HayleyfsDentry>(), false);
        Self {
            self_dentry: s,
            parent_dentry: p,
        }
    }
}

// impl Drop for DirInitToken<'_> {
//     fn drop(&mut self) {
//         pr_info!("dropping dir init token!\n");
//         // flush them separately in case there is some unexpected padding
//         // this could cause redundant flushes
//         clflush(self.self_dentry, size_of::<HayleyfsDentry>(), false);
//         clflush(self.parent_dentry, size_of::<HayleyfsDentry>(), false);
//     }
// }

pub(crate) struct DentryAddToken<'a> {
    dentry: &'a mut HayleyfsDentry,
}

impl<'a> DentryAddToken<'a> {
    pub(crate) unsafe fn new(d: &'a mut HayleyfsDentry) -> Self {
        // TODO: fencing, and does this still have to be done in two flushes?
        // flush and fence the dentry
        pr_info!("flushing dentry add token\n");
        clflush(d, size_of::<HayleyfsDentry>(), true);
        // then make it valid
        unsafe { d.valid = true };
        // then flush and fence again
        clflush(d, size_of::<HayleyfsDentry>(), true);
        Self { dentry: d }
    }

    pub(crate) fn set_ino(&mut self, i: InodeNum) {
        unsafe { self.dentry.set_ino(i) };
    }

    pub(crate) fn set_dentry_name(&mut self, name: &str) {
        unsafe { self.dentry.set_dentry_name(name) };
    }
}

// impl Drop for DentryAddToken<'_> {
//     fn drop(&mut self) {
//         // flush and fence the dentry
//         pr_info!("dropping dentry add token\n");
//         clflush(self.dentry, size_of::<HayleyfsDentry>(), true);
//         // then make it valid
//         unsafe { self.dentry.valid = true };
//         // then flush and fence again
//         clflush(self.dentry, size_of::<HayleyfsDentry>(), true);
//     }
// }

// differs from dentry add token because this only provides an immutable
// reference to the dentry and does not flush on drop
pub(crate) struct DentryReadToken<'a> {
    dentry: &'a HayleyfsDentry,
}

impl<'a> DentryReadToken<'a> {
    pub(crate) unsafe fn new(d: &'a HayleyfsDentry) -> Self {
        Self { dentry: d }
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.dentry.get_ino()
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
    name_len: usize,
}

impl HayleyfsDentry {
    // TODO: why does link count live in dentries?
    fn set_up(&mut self, ino: InodeNum, name: &str) {
        self.ino = ino;
        unsafe { self.set_dentry_name(name) };
        self.valid = false;
    }

    unsafe fn set_dentry_name(&mut self, name: &str) {
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

    pub(crate) unsafe fn set_valid(&mut self, v: bool) {
        self.valid = v;
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    unsafe fn set_ino(&mut self, i: InodeNum) {
        self.ino = i;
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

#[no_mangle]
pub(crate) fn hayleyfs_alloc_page(sbi: &SbInfo) -> Result<DataAllocToken> {
    let bitmap_addr = get_data_bitmap_addr(sbi);
    // we are initializing a directory, so allocate a data page for its dentries
    // find an empty page based on the bitmap
    // last argument is supposed to be bitmap size IN BITS
    let page_no = unsafe {
        hayleyfs_find_next_zero_bit(
            bitmap_addr as *mut u64,
            (PAGE_SIZE * 8).try_into().unwrap(),
            0,
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
#[no_mangle]
pub(crate) fn initialize_dir<'a>(
    sbi: &SbInfo,
    // pi: &mut HayleyfsInode,
    ino_token: &mut InodeInitToken<'_>,
    // self_ino: InodeNum,
    parent_ino: InodeNum,
    // alloc_token: &'a DataAllocToken,
    page_no: PmPage,
) -> Result<DirInitToken<'a>> {
    let dir_page = unsafe { &mut *(get_data_page_addr(sbi, page_no) as *mut DirPage) };

    // TODO: confirm that split_at_mut is just an ownership/mutability thing
    // and doesn't make copies
    // TODO: use dentry tokens here (or at least have a nicer abstraction to
    // eliminate the unsafe calls to set_valid). we shouldn't really be accessing
    // the dentry list directly
    let (d1, d2) = dir_page.dentries.split_at_mut(1);

    let mut self_dentry = &mut d1[0];

    self_dentry.set_up(ino_token.get_ino(), ".");
    unsafe { self_dentry.set_valid(true) };

    let mut parent_dentry = &mut d2[0];

    parent_dentry.set_up(parent_ino, "..");
    unsafe { self_dentry.set_valid(true) };

    let init_token = DirInitToken {
        self_dentry,
        parent_dentry,
    };

    // add the data page we have just set up to the inode
    // TODO: i don't THINK doing this here will cause issues with dependencies,
    // but do some testing to be sure
    ino_token.add_data_page(page_no);

    Ok(init_token)
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

pub(crate) fn add_dentry_to_parent<'a>(
    sbi: &SbInfo,
    parent_ino: InodeNum,
    inode_token: &InodeInitToken<'_>,
    dir_token: &DirInitToken<'_>,
    link_token: &ParentLinkToken<'_>,
    name: &kernel::str::CStr,
) -> Result<DentryAddToken<'a>> {
    // TODO: you should set it up so that obtaining the dentry (and the inode?)
    // automatically give you guards that ensure things are dropped at the end.
    // the only safe way to interact with inodes and dentries and anything else
    // stored on PM should be via a token/guard

    // first, unsafely obtain the parent's inode
    let mut parent_dir = unsafe { hayleyfs_get_inode_by_ino(&sbi, parent_ino) };

    // then, obtain its data page and scan it for the first unused dentry slot
    match parent_dir.get_data_page_no() {
        Some(page_no) => {
            // TODO: safe abstraction for this
            let dir_page = unsafe { &mut *(get_data_page_addr(sbi, page_no) as *mut DirPage) };
            let mut dentry_token = dir_page.get_next_invalid_dentry().unwrap(); // TODO: handle error
            dentry_token.set_ino(inode_token.get_ino());
            dentry_token.set_dentry_name(name.to_str().unwrap());
            Ok(dentry_token)
        }
        None => Err(Error::ENOTDIR),
    }
}

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
            // TODO: none of this should work but since it does i'm not going to touch it for now
            // but it should be adjusted to use the safe interface to dentries
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
