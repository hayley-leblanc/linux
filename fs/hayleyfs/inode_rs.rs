#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]
#![allow(unused)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::mut_from_ref)]

use crate::data::*;
use crate::defs::*;
use crate::pm::*;
use crate::super_def::*;
use crate::tokens::*;
use core::mem::size_of;
use core::ptr;
use kernel::bindings::{
    d_instantiate, d_splice_alias, dentry, iget_failed, iget_locked, inc_nlink, inode,
    inode_init_owner, inode_operations, insert_inode_locked, new_inode, set_nlink, simple_lookup,
    super_block, umode_t, unlock_new_inode, user_namespace, ENAMETOOLONG, I_NEW, S_IFDIR,
};
use kernel::c_types::{c_char, c_int, c_void};
use kernel::prelude::*;
use kernel::str::CStr;
use kernel::{c_default_struct, PAGE_SIZE};

pub(crate) type InodeNum = usize;
// impl BitmapIndex for InodeNum {}

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
// TODO: organize this better
// #[repr(packed)]
pub(crate) struct HayleyfsInode {
    // valid: bool,
    ino: InodeNum,
    data0: Option<PmPage>,
    mode: u32,
    link_count: u16,
}

impl HayleyfsInode {
    // no constructor

    /// unsafe because we should only modify the inode directly in specific circumstances
    /// TODO: should take an inode alloc token
    /// TODO: this could be safe if it returns an inode init token?
    pub(crate) unsafe fn set_up_inode(
        &mut self,
        ino: InodeNum,
        data: Option<PmPage>,
        mode: u32,
        link_count: u16,
        _: &InodeAllocToken,
    ) {
        // self.valid = false;
        self.ino = ino;
        self.data0 = data;
        self.mode = mode;
        self.link_count = link_count;
    }

    pub(crate) fn zero_inode(&mut self) -> InodeZeroToken<'_> {
        self.ino = 0;
        self.data0 = None;
        self.mode = 0;
        self.link_count = 0;
        InodeZeroToken::new(self)
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    pub(crate) fn get_data_page_no(&self) -> Option<PmPage> {
        self.data0
    }

    pub(crate) fn set_data_page_no(&mut self, page_no: Option<PmPage>) {
        self.data0 = page_no;
    }

    pub(crate) fn inc_links(&mut self) {
        self.link_count += 1;
    }

    pub(crate) fn get_mode(&self) -> u32 {
        self.mode
    }
}

pub(crate) fn hayleyfs_get_inode_by_ino(sbi: &SbInfo, ino: InodeNum) -> &mut HayleyfsInode {
    let addr = (PAGE_SIZE * 2) + (ino * size_of::<HayleyfsInode>());
    // TODO: check that this address does not exceed the inode page
    // TODO: handle possible panic on converting usize to isize here
    let addr = sbi.virt_addr as usize + addr;
    unsafe { &mut *(addr as *mut HayleyfsInode) }
}

// TODO: can you replace this with the get inode bitmap and get cacheline functions
fn get_inode_bitmap_addr(sbi: &SbInfo) -> *mut c_void {
    (sbi.virt_addr as usize + (INODE_BITMAP_PAGE * PAGE_SIZE)) as *mut c_void
}

pub(crate) fn get_inode_bitmap(sbi: &SbInfo) -> &mut PersistentBitmap {
    unsafe {
        &mut *((sbi.virt_addr as usize + (INODE_BITMAP_PAGE * PAGE_SIZE)) as *mut PersistentBitmap)
    }
}

#[no_mangle]
fn hayleyfs_allocate_inode(sbi: &SbInfo) -> Result<InodeAllocToken> {
    let mut bitmap = get_inode_bitmap(&sbi);

    // starts at bit 1 to ignore bit 0 since we don't use inode 0
    let ino = unsafe {
        hayleyfs_find_next_zero_bit(
            bitmap as *mut _ as *mut u64,
            (PAGE_SIZE * 8).try_into().unwrap(),
            2,
        )
    };

    if ino == (PAGE_SIZE * 8) {
        return Err(Error::ENOSPC);
    }

    // TODO: this doesn't work without the double cast - why though?
    // TODO: safe abstraction around this - maybe one that produces the inode alloc token for you
    unsafe { hayleyfs_set_bit(ino, bitmap as *mut _ as *mut c_void) };
    let cacheline = bitmap.get_bitmap_cacheline(ino);

    let token = InodeAllocToken::new(ino, cacheline);

    Ok(token)
}

/// this requires a super init token not because it really needs it but primarily to make sure
/// it is only used to set up reserved inodes
pub(crate) fn hayleyfs_allocate_inode_by_ino(
    sbi: &SbInfo,
    ino: InodeNum,
    super_token: &SuperInitToken<'_>,
) -> Result<InodeAllocToken> {
    let mut bitmap = get_inode_bitmap(&sbi);

    if ino == (PAGE_SIZE * 8) {
        pr_info!("ino is too big\n");
        return Err(Error::ENOSPC);
    }

    // test and set the requested bit in the bitmap
    // return an error if it is already in use
    let bit_test = unsafe { hayleyfs_set_bit(ino, bitmap as *mut _ as *mut c_void) };
    if bit_test != 0 {
        pr_info!("bitmap is already set\n");
        return Err(Error::EEXIST);
    }

    let cacheline = bitmap.get_bitmap_cacheline(ino);

    let token = InodeAllocToken::new(ino, cacheline);

    Ok(token)
}

// TODO: should lifetime come from sbi or token?
#[no_mangle]
pub(crate) fn hayleyfs_initialize_inode<'a>(
    sbi: SbInfo,
    token: &InodeAllocToken,
) -> Result<InodeInitToken<'a>> {
    // TODO: ideally these next few lines where we get the inode would be in a function that
    // is passed a token, but that runs into issues where Rust thinks we are returning a
    // reference to a function parameter because it can't tell that the init token doesn't
    // actually borrow anything from the alloc token (I think)
    let ino = token.ino();
    let addr = (PAGE_SIZE * 2) + (ino * size_of::<HayleyfsInode>());
    // TODO: check that this address does not exceed the inode page
    // TODO: handle possible panic on converting usize to isize here
    let addr = sbi.virt_addr as usize + addr;
    // unsafe { &mut *(addr as *mut HayleyfsInode) }
    let inode = unsafe { &mut *(addr as *mut HayleyfsInode) };

    // this is a set up function, not a constructor, because the inodes already exist
    // on PM and we just need to set their values
    unsafe { inode.set_up_inode(token.ino(), None, S_IFDIR, 2, &token) };

    let init_token = InodeInitToken::new(inode);

    Ok(init_token)
}

fn inc_parent_links(sbi: &SbInfo, parent_ino: InodeNum) -> ParentLinkToken<'_> {
    // 1. obtain the parent inode
    // 2. increment link count
    // 3. return it in a parent link token
    let mut parent_dir = hayleyfs_get_inode_by_ino(&sbi, parent_ino);
    unsafe { parent_dir.inc_links() };

    let link_token = unsafe { ParentLinkToken::new(parent_dir) };
    link_token
}

// TODO: this probably should not be the static lifetime?
pub(crate) fn hayleyfs_iget(sb: *mut super_block, ino: usize) -> Result<&'static mut inode> {
    let inode = unsafe { &mut *(iget_locked(sb, ino as u64) as *mut inode) };
    if ptr::eq(inode, ptr::null_mut()) {
        unsafe { iget_failed(inode) };
        return Err(Error::ENOMEM);
    }
    if (inode.i_state & I_NEW as u64) == 0 {
        return Ok(inode);
    }
    inode.i_ino = ino as u64;
    // TODO: right now this is hardcoded for directories because
    // that's all we have. but it should be read from the persistent inode
    // and set depending on the type of inode
    inode.i_mode = S_IFDIR as u16;
    inode.i_op = &HayleyfsDirInodeOps;
    unsafe {
        inode.__bindgen_anon_3.i_fop = &HayleyfsFileOps; // fileOps has to be mutable so this has to be unsafe. Why does it have to be mutable???
        set_nlink(inode, 2);
    }
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

    let result = _hayleyfs_mkdir(mnt_userns, dir, dentry, mode);
    match result {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno(),
    }
}

// TODO: actual error handling - you'll need to roll back changes
// if something goes wrong?
// TODO: you need to test this, in both this implementation and other possible ones
// (e.g., changing the ordering of operations that aren't dependent on each other)
// how could you do that systematically?
#[no_mangle]
fn _hayleyfs_mkdir<'a>(
    mnt_userns: &mut user_namespace,
    dir: &mut inode,
    dentry: &'a mut dentry,
    mode: umode_t,
) -> Result<DentryAddToken<'a>> {
    pr_info!("---------------------------------------\n");
    pr_info!("creating a new directory!\n");

    let sb = dir.i_sb;
    let sbi = hayleyfs_get_sbi(sb);

    let dentry_name = unsafe { (*dentry).d_name.name } as *const c_char;
    let dentry_name = unsafe { CStr::from_char_ptr(dentry_name) };
    if dentry_name.len() > MAX_FILENAME_LEN {
        pr_info!("dentry name {:?} is too long", dentry_name);
        return Err(Error::ENAMETOOLONG);
    }
    unsafe { pr_info!("dentry name in mkdir: {:?}", dentry_name) };

    // TODO: handle out of inodes case
    let ino_token = hayleyfs_allocate_inode(&sbi).unwrap();

    if sbi.mount_opts.crash_point == 1 {
        return Err(Error::EINVAL);
    }

    let mut inode_init_token = hayleyfs_initialize_inode(*sbi, &ino_token)?;

    if sbi.mount_opts.crash_point == 2 {
        return Err(Error::EINVAL);
    }

    // allocate a data page
    let data_alloc_token = hayleyfs_alloc_page(&sbi).unwrap();

    if sbi.mount_opts.crash_point == 3 {
        return Err(Error::EINVAL);
    }

    // set up the data page with dentries for the new directory
    let (dir_init_token, page_add_token) = initialize_dir(
        &sbi,
        inode_init_token,
        dir.i_ino.try_into().unwrap(),
        data_alloc_token.page_no(),
    )?;

    // setting link count does not require any tokens BUT it produces
    // a token that is required to add a dentry to the parent
    let parent_link_token = inc_parent_links(&sbi, dir.i_ino.try_into().unwrap());

    if sbi.mount_opts.crash_point == 6 {
        return Err(Error::EINVAL);
    }

    // set up vfs inode
    // TODO: what if this fails?
    let inode = hayleyfs_new_vfs_inode(
        sb,
        dir,
        &page_add_token,
        mnt_userns,
        mode,
        NewInodeType::Mkdir,
    );
    unsafe {
        d_instantiate(dentry, inode);
        inc_nlink(dir as *mut inode);
        unlock_new_inode(inode);
    };

    let dentry_add_token = add_dentry_to_parent(
        &sbi,
        dir.i_ino.try_into().unwrap(),
        &page_add_token,
        &dir_init_token,
        &parent_link_token,
        dentry_name,
    )?;

    if sbi.mount_opts.crash_point == 7 {
        return Err(Error::EINVAL);
    }

    Ok(dentry_add_token)
}

fn hayleyfs_new_vfs_inode(
    sb: *mut super_block,
    dir: &inode,
    ino: &DirPageAddToken<'_>,
    mnt_userns: &mut user_namespace,
    mode: umode_t,
    new_type: NewInodeType,
) -> *mut inode {
    // TODO: handle errors in here
    let inode = unsafe { new_inode(sb) };
    let ino = ino.get_ino();

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

// TODO: deal with soft updates
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
            let read_token = dir_page.lookup_name(dentry_name.as_bytes_with_nul());
            // TODO: figure out how to handle errors here
            match read_token {
                Ok(token) => {
                    let inode = hayleyfs_iget(sb, token.get_ino()).unwrap();
                    unsafe { d_splice_alias(inode, dentry) }
                }
                Err(_) => unsafe { simple_lookup(dir, dentry, flags) },
            }
        }
        None => {
            // TODO: figure out how to return the correct error type here
            // for now just fall back to making the kernel do that for us
            unsafe { simple_lookup(dir, dentry, flags) }
        }
    }
}
