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
// #[repr(packed)]
pub(crate) struct HayleyfsInode {
    ino: InodeNum,
    data0: Option<PmPage>,
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
    /// or make it require a token
    pub(crate) unsafe fn set_data_page_no(&mut self, page_no: Option<PmPage>) {
        self.data0 = page_no;
    }

    /// TODO: document safety
    /// or make it require a token
    pub(crate) unsafe fn inc_links(&mut self) {
        self.link_count += 1;
    }
}

struct MkdirTokens<'a> {
    inode_alloc_token: InodeAllocToken,
    data_alloc_token: DataAllocToken,
    inode_init_token: InodeInitToken<'a>,
    parent_link_token: ParentLinkToken<'a>,
    dir_init_token: DirInitToken<'a>,
    dentry_add_token: DentryAddToken<'a>,
}

impl<'a> MkdirTokens<'a> {
    fn new(
        inode_alloc_token: InodeAllocToken,
        data_alloc_token: DataAllocToken,
        inode_init_token: InodeInitToken<'a>,
        parent_link_token: ParentLinkToken<'a>,
        dir_init_token: DirInitToken<'a>,
        dentry_add_token: DentryAddToken<'a>,
    ) -> Self {
        Self {
            inode_alloc_token,
            data_alloc_token,
            inode_init_token,
            parent_link_token,
            dir_init_token,
            dentry_add_token,
        }
    }
}

pub(crate) struct InodeAllocToken {
    ino: InodeNum,
    cache_line: *mut CacheLine,
    // cache_line: *mut c_void, // TODO: I would rather this be a CacheLine but there are ownership issues
}

impl InodeAllocToken {
    /// this constructor should only be called when getting a new
    /// inode number using the inode bitmap. it is unsafe to call
    /// anywhere else
    pub(crate) unsafe fn new(i: InodeNum, line: *mut CacheLine) -> Self {
        Self {
            ino: i,
            cache_line: line,
        }
    }

    /// return the inode number associated with this token
    pub(crate) fn ino(&self) -> InodeNum {
        self.ino
    }
}

impl Drop for InodeAllocToken {
    fn drop(&mut self) {
        pr_info!("dropping inode alloc token\n");
        clflush(self.cache_line, CACHELINE_SIZE, false);
    }
}

pub(crate) struct InodeInitToken<'a> {
    inode: &'a mut HayleyfsInode,
}

impl<'a> InodeInitToken<'a> {
    pub(crate) unsafe fn new(inode: &'a mut HayleyfsInode) -> Self {
        Self { inode }
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.inode.ino
    }

    // this is NOT unsafe because the fact that we have the token right now
    // means it will be correctly flushed in the future
    pub(crate) fn add_data_page(&mut self, page: PmPage) {
        unsafe { self.inode.set_data_page_no(Some(page)) };
    }
}

impl Drop for InodeInitToken<'_> {
    fn drop(&mut self) {
        pr_info!("dropping inode init token!\n");
        clflush(self.inode, size_of::<HayleyfsInode>(), true);
    }
}

pub(crate) struct ParentLinkToken<'a> {
    inode: &'a mut HayleyfsInode,
}

impl<'a> ParentLinkToken<'a> {
    pub(crate) unsafe fn new(inode: &'a mut HayleyfsInode) -> Self {
        Self { inode }
    }
}

impl<'a> Drop for ParentLinkToken<'a> {
    fn drop(&mut self) {
        pr_info!("Dropping parent link token\n");
        clflush(self.inode, size_of::<HayleyfsInode>(), true);
    }
}

// TODO: figure out if you actually need this
pub(crate) unsafe fn hayleyfs_get_inode_by_ino(sbi: &SbInfo, ino: InodeNum) -> &mut HayleyfsInode {
    let addr = (PAGE_SIZE * 2) + (ino * size_of::<HayleyfsInode>());
    // TODO: check that this address does not exceed the inode page
    // TODO: handle possible panic on converting usize to isize here
    let addr = sbi.virt_addr as usize + addr;
    // unsafe { &mut *(addr as *mut HayleyfsInode) }
    unsafe { &mut *(addr as *mut HayleyfsInode) }
}

// TODO: can you replace this with the get inode bitmap and get cacheline functions
fn get_inode_bitmap_addr(sbi: &SbInfo) -> *mut c_void {
    (sbi.virt_addr as usize + (INODE_BITMAP_PAGE * PAGE_SIZE)) as *mut c_void
}

fn get_inode_bitmap(sbi: &SbInfo) -> &mut PersistentBitmap {
    unsafe {
        &mut *((sbi.virt_addr as usize + (INODE_BITMAP_PAGE * PAGE_SIZE)) as *mut PersistentBitmap)
    }
}

// TODO: phase this out or make it unsafe or something. it doesn't really work the way i want
// with the tokens
pub(crate) unsafe fn set_inode_bitmap_bit(sbi: &SbInfo, ino: InodeNum) -> Result<()> {
    let addr = get_inode_bitmap_addr(&sbi);
    // TODO: should check that the provided ino is valid and return an error if not
    unsafe { hayleyfs_set_bit(ino, addr as *mut c_void) };
    // TODO: only flush the updated cache line, not the whole bitmap
    clflush(addr as *const c_void, PAGE_SIZE, false);
    Ok(())
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
    unsafe { hayleyfs_set_bit(ino, bitmap as *mut _ as *mut c_void) };
    let cacheline = get_bitmap_cacheline(&mut bitmap, ino);

    let token = unsafe { InodeAllocToken::new(ino, cacheline) };

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
        return Err(Error::ENOSPC);
    }

    // test and set the requested bit in the bitmap
    // return an error if it is already in use
    let bit_test = unsafe { hayleyfs_set_bit(ino, bitmap as *mut _ as *mut c_void) };
    if bit_test != 0 {
        return Err(Error::EEXIST);
    }

    let cacheline = get_bitmap_cacheline(&mut bitmap, ino);

    let token = unsafe { InodeAllocToken::new(ino, cacheline) };

    Ok(token)
}

// TODO: should lifetime come from sbi or token?
#[no_mangle]
pub(crate) fn hayleyfs_initialize_inode<'a>(
    sbi: &'a SbInfo,
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
    unsafe { inode.set_up_inode(token.ino(), None, S_IFDIR, 2) };

    let init_token = InodeInitToken { inode };

    Ok(init_token)
}

fn inc_parent_links(sbi: &SbInfo, parent_ino: InodeNum) -> ParentLinkToken<'_> {
    // 1. obtain the parent inode
    // 2. increment link count
    // 3. return it in a parent link token
    let mut parent_dir = unsafe { hayleyfs_get_inode_by_ino(&sbi, parent_ino) };
    unsafe { parent_dir.inc_links() };

    let link_token = unsafe { ParentLinkToken::new(parent_dir) };
    link_token
}

// TODO: this probably should not be the static lifetime
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

    // TODO: have this function use nicer Rust errors and convert to something
    // C can understand when it's done
    _hayleyfs_mkdir(mnt_userns, dir, dentry, mode)
}

// TODO: actual error handling
#[no_mangle]
fn _hayleyfs_mkdir(
    mnt_userns: &mut user_namespace,
    dir: &mut inode,
    dentry: &mut dentry,
    mode: umode_t,
) -> i32 {
    pr_info!("creating a new directory!\n");

    let sb = dir.i_sb;
    let sbi = hayleyfs_get_sbi(sb);

    let dentry_name = unsafe { (*dentry).d_name.name } as *const c_char;
    let dentry_name = unsafe { CStr::from_char_ptr(dentry_name) };
    if dentry_name.len() > MAX_FILENAME_LEN {
        pr_info!("dentry name {:?} is too long", dentry_name);
        return -(ENAMETOOLONG as c_int);
    }
    unsafe { pr_info!("dentry name in mkdir: {:?}", dentry_name) };

    // TODO: handle out of inodes case
    let ino_token = hayleyfs_allocate_inode(&sbi).unwrap();

    // TODO: add an init_inode function that uses the ino_token to
    // initialize the new inode
    let mut inode_init_token = hayleyfs_initialize_inode(&sbi, &ino_token).unwrap();

    // allocate a data page
    let data_alloc_token = hayleyfs_alloc_page(&sbi).unwrap();
    // set up the data page with dentries for the new directory
    let dir_init_token = initialize_dir(
        &sbi,
        &mut inode_init_token,
        dir.i_ino.try_into().unwrap(),
        data_alloc_token.page_no(),
    )
    .unwrap();

    // setting link count does not require any tokens BUT it produces
    // a token that is required to add a dentry to the parent
    // TODO: right?
    let parent_link_token = inc_parent_links(&sbi, dir.i_ino.try_into().unwrap());

    // set up vfs inode
    let inode = hayleyfs_new_vfs_inode(
        sb,
        dir,
        &inode_init_token,
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
        &inode_init_token,
        &dir_init_token,
        &parent_link_token,
        dentry_name,
    )
    .unwrap();

    let mkdir_tokens = MkdirTokens::new(
        ino_token,
        data_alloc_token,
        inode_init_token,
        parent_link_token,
        dir_init_token,
        dentry_add_token,
    );

    0
}

fn hayleyfs_new_vfs_inode(
    sb: *mut super_block,
    dir: &inode,
    ino: &InodeInitToken<'_>,
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
    let parent_pi = unsafe { hayleyfs_get_inode_by_ino(sbi, dir.i_ino.try_into().unwrap()) };
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
