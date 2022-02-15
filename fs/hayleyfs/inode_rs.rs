#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]
#![allow(unused)]
#![allow(clippy::needless_borrow)]

use crate::data::*;
use crate::defs::*;
use crate::pm::*;
use crate::super_def::*;
use core::mem::size_of;
use core::ptr;
use kernel::bindings::{
    d_instantiate, d_splice_alias, dentry, iget_failed, iget_locked, inc_nlink, inode,
    inode_init_owner, inode_operations, insert_inode_locked, new_inode, set_nlink, simple_lookup,
    super_block, umode_t, unlock_new_inode, user_namespace, ENAMETOOLONG, S_IFDIR,
};
use kernel::c_types::{c_char, c_int, c_void};
use kernel::prelude::*;
use kernel::str::CStr;
use kernel::{c_default_struct, PAGE_SIZE};

pub(crate) type InodeNum = usize;

// reserved inode nums
pub(crate) const HAYLEYFS_ROOT_INO: InodeNum = 1;

// pub(crate) makes it visible to the whole crate
// not sure why it is not already visible with in the crate...?
pub(crate) static HayleyfsDirInodeOps: inode_operations = inode_operations {
    mkdir: Some(hayleyfs_mkdir),
    lookup: Some(hayleyfs_lookup),
    ..c_default_struct!(inode_operations)
};

enum NewInodeType {
    Create,
    Mkdir,
}

// inode that lives in PM
// TODO: should this actually be packed?
#[repr(packed)]
pub(crate) struct HayleyfsInode {
    data0: Option<PmPage>,
    ino: InodeNum,
    mode: u32,
    link_count: u16,
}

impl HayleyfsInode {
    // no constructor

    /// unsafe because we should only modify the inode directly in specific circumstances
    /// TODO: should take an inode alloc token
    pub(crate) unsafe fn set_up_inode(
        &mut self,
        ino: InodeNum,
        data: Option<PmPage>,
        mode: u32,
        link_count: u16,
    ) {
        self.ino = ino;
        self.data0 = data;
        self.mode = mode;
        self.link_count = link_count;
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    // TODO: double check that the page number can't be modified
    // after being returned here. it shouldn't but double check
    pub(crate) fn get_data_page_no(&self) -> Option<PmPage> {
        self.data0
    }

    /// TODO: document safety
    pub(crate) unsafe fn set_data_page_no(&mut self, page_no: Option<PmPage>) {
        self.data0 = page_no;
    }

    /// TODO: document safety
    pub(crate) unsafe fn inc_links(&mut self) {
        self.link_count += 1;
    }
}

struct InodeAllocToken {
    ino: InodeNum,
}

impl InodeAllocToken {
    /// this constructor should only be called when getting a new
    /// inode number using the inode bitmap. it is unsafe to call
    /// anywhere else
    pub(crate) unsafe fn new(i: InodeNum) -> Self {
        Self { ino: i }
    }

    /// return the inode number associated with this token
    pub(crate) fn ino(&self) -> InodeNum {
        self.ino
    }
}

// TODO: this should require a token
// there can be an immutable version that doesn't require one
// TODO: think about ownership here
#[allow(clippy::mut_from_ref)]
pub(crate) fn hayleyfs_get_inode_by_ino(sbi: &SbInfo, ino: InodeNum) -> &mut HayleyfsInode {
    let addr = (PAGE_SIZE * 2) + (ino * size_of::<HayleyfsInode>());
    pr_info!("addr: {:#X}\n", addr);
    // TODO: check that this address does not exceed the inode page
    // TODO: handle possible panic on converting usize to isize here
    let addr = sbi.virt_addr as usize + addr;
    // unsafe { &mut *(addr as *mut HayleyfsInode) }
    unsafe { &mut *(addr as *mut HayleyfsInode) }
}

fn get_inode_bitmap_addr(sbi: &SbInfo) -> *mut c_void {
    (sbi.virt_addr as usize + (INODE_BITMAP_PAGE * PAGE_SIZE)) as *mut c_void
}

// TODO: this might need to be unsafe? any function that modifies PM directly
// should probably be unsafe and wrapped in a safe abstraction. this should
// only be called by hayleyfs_alloc_inode and when setting up reserved inodes
// at initialization
pub(crate) fn set_inode_bitmap_bit(sbi: &SbInfo, ino: InodeNum) -> Result<()> {
    let addr = get_inode_bitmap_addr(&sbi);
    // TODO: should check that the provided ino is valid and return an error if not
    unsafe { hayleyfs_set_bit(ino, addr as *mut c_void) };
    // TODO: only flush the updated cache line, not the whole bitmap
    clflush(addr as *const c_void, PAGE_SIZE, false);
    Ok(())
}

fn hayleyfs_allocate_inode(sbi: &SbInfo) -> Result<InodeAllocToken> {
    let bitmap_addr = get_inode_bitmap_addr(&sbi);

    // starts at bit 1 to ignore bit 0 since we don't use inode 0
    let ino = unsafe {
        hayleyfs_find_next_zero_bit(
            bitmap_addr as *mut u64,
            (PAGE_SIZE * 8).try_into().unwrap(),
            2,
        )
    };

    if ino == (PAGE_SIZE * 8) {
        return Err(Error::ENOSPC);
    }

    set_inode_bitmap_bit(sbi, ino)?;

    // unsafely generate an inode alloc token and use it to return
    // the inode number
    let token = unsafe { InodeAllocToken::new(ino) };

    Ok(token)
}

// TODO: this probably should not be the static lifetime
pub(crate) fn hayleyfs_iget(sb: *mut super_block, ino: usize) -> Result<&'static mut inode> {
    let inode = unsafe { &mut *(iget_locked(sb, ino as u64) as *mut inode) };
    if ptr::eq(inode, ptr::null_mut()) {
        unsafe { iget_failed(inode) };
        return Err(Error::EINVAL); // TODO: what error type should this actually return?
    }
    inode.i_ino = ino as u64;
    unsafe { unlock_new_inode(inode) };

    Ok(inode)
}

#[no_mangle]
unsafe extern "C" fn hayleyfs_mkdir(
    mnt_userns_raw: *mut user_namespace,
    dir_raw: *mut inode,
    dentry_raw: *mut dentry,
    mode: umode_t,
) -> i32 {
    // convert arguments to mutable references rather than raw pointers
    // TODO: I bet you could write a macro to do this a bit more cleanly?
    let mnt_userns = unsafe { &mut *(mnt_userns_raw as *mut user_namespace) };
    let dir = unsafe { &mut *(dir_raw as *mut inode) };
    let dentry = unsafe { &mut *(dentry_raw as *mut dentry) };

    // TODO: have this function use nicer Rust errors and convert to something
    // C can understand when it's done
    _hayleyfs_mkdir(mnt_userns, dir, dentry, mode)
}

// TODO: actual error handling
fn _hayleyfs_mkdir(
    mnt_userns: &mut user_namespace,
    dir: &mut inode,
    dentry: &mut dentry,
    mode: umode_t,
) -> i32 {
    pr_info!("creating a new directory!\n");

    let dentry_name = unsafe { (*dentry).d_name.name } as *const c_char;
    let dentry_name = unsafe { CStr::from_char_ptr(dentry_name) };
    if dentry_name.len() > MAX_FILENAME_LEN {
        pr_info!("dentry name {:?} is too long", dentry_name);
        return -(ENAMETOOLONG as c_int);
    }
    unsafe { pr_info!("dentry name in mkdir: {:?}", dentry_name) };

    let sb = dir.i_sb;
    let sbi = hayleyfs_get_sbi(sb);

    // TODO: handle out of inodes case
    let ino = hayleyfs_allocate_inode(&sbi).unwrap();
    // // set_inode_bitmap_bit(sbi, ino).unwrap();

    // // the inode actually probably shouldn't be flushed until later
    // let mut pi = hayleyfs_get_inode_by_ino(&sbi, ino);

    // pi.ino = ino;
    // pi.data0 = None;
    // pi.mode = S_IFDIR;
    // pi.link_count = 2;
    // clflush(&pi, size_of::<HayleyfsInode>(), true);

    // // allocate a data page and set up its dentries
    // initialize_dir(&sbi, &mut pi, ino, dir.i_ino.try_into().unwrap()).unwrap();

    // // add a dentry to the parent
    // let mut parent_dir = hayleyfs_get_inode_by_ino(&sbi, dir.i_ino.try_into().unwrap());
    // add_dentry_to_parent(&sbi, &mut parent_dir, ino, dentry_name).unwrap();

    // // set up vfs inode
    // let inode = hayleyfs_new_vfs_inode(sb, dir, ino, mnt_userns, mode, NewInodeType::Mkdir);
    // unsafe {
    //     d_instantiate(dentry, inode);
    //     inc_nlink(dir as *mut inode);
    //     unlock_new_inode(inode);
    // };

    0
}

fn hayleyfs_new_vfs_inode(
    sb: *mut super_block,
    dir: &inode,
    ino: usize,
    mnt_userns: &mut user_namespace,
    mode: umode_t,
    new_type: NewInodeType,
) -> *mut inode {
    // TODO: handle errors in here
    let inode = unsafe { new_inode(sb) };

    unsafe {
        inode_init_owner(mnt_userns as *mut user_namespace, inode, dir, mode);
        (*inode).i_ino = ino as u64;
    }

    match new_type {
        NewInodeType::Mkdir => unsafe {
            (*inode).i_mode = S_IFDIR as u16;
            (*inode).i_op = &HayleyfsDirInodeOps;
            (*inode).__bindgen_anon_3.i_fop = &HayleyfsFileOps;
            set_nlink(inode, 2);
        },
        NewInodeType::Create => {
            pr_info!("implement me!");
        }
    }

    unsafe { insert_inode_locked(inode) };

    inode
}

#[no_mangle]
unsafe extern "C" fn hayleyfs_lookup(
    dir: *mut inode,
    dentry: *mut dentry,
    flags: u32,
) -> *mut dentry {
    let dentry_name = unsafe { (*dentry).d_name.name } as *const c_char;
    let dentry_name = unsafe { CStr::from_char_ptr(dentry_name) };

    let dir = unsafe { &mut *(dir as *mut inode) };

    let sb = dir.i_sb;
    let sbi = hayleyfs_get_sbi(sb);

    // look up the parent's inode so that we can look at its directory entries
    let parent_pi = hayleyfs_get_inode_by_ino(sbi, dir.i_ino.try_into().unwrap());
    // TODO: check that this is actually a directory

    match parent_pi.get_data_page_no() {
        Some(page_no) => {
            // TODO: you do this same code a lot - might make more sense to have a function
            // that takes a closure describing what to do in the loop
            let dir_page = unsafe { &mut *(get_data_page_addr(sbi, page_no) as *mut DirPage) };
            for i in 0..DENTRIES_PER_PAGE {
                let mut p_dentry = &mut dir_page.dentries[i];
                if !p_dentry.valid {
                    // TODO: you need to return not found somewhere here
                    break;
                } else if compare_dentry_name(&p_dentry.name, dentry_name.as_bytes_with_nul()) {
                    let inode = hayleyfs_iget(sb, p_dentry.ino).unwrap();
                    // TODO: handle errors on the returned inode
                    return unsafe { d_splice_alias(inode, dentry) };
                }
            }
            unsafe { simple_lookup(dir, dentry, flags) }
        }
        None => {
            // TODO: figure out how to return the correct error type here
            // for now just fall back to making the kernel do that for us
            unsafe { simple_lookup(dir, dentry, flags) }
        }
    }
}
