use crate::balloc::*;
use crate::defs::*;
use crate::pm::*;
use crate::h_dir::*;
use crate::typestate::*;
use crate::volatile::*;
use core::{
    marker::PhantomData,
    mem,
};
use kernel::prelude::*;
use kernel::{bindings, ForeignOwnable, fs, sync::{Arc, smutex::Mutex}, rbtree::RBTree};

// ZSTs for representing inode types
// These are not typestate since they don't change, but they are a generic
// parameter for inodes so that the compiler can check that we are using
// the right kind of inode
#[derive(Debug)]
pub(crate) struct RegInode {}
#[derive(Debug)]
pub(crate) struct DirInode {}

pub(crate) trait AnyInode {}
impl AnyInode for RegInode {}
impl AnyInode for DirInode {}

/// Persistent inode structure
/// It is always unsafe to access this structure directly
/// TODO: add the rest of the fields
#[repr(C)]
pub(crate) struct HayleyFsInode {
    inode_type: InodeType, // TODO: currently 2 bytes? could be 1
    link_count: u16,
    mode: u16,
    uid: u32,
    gid: u32,
    ctime: bindings::timespec64,
    atime: bindings::timespec64,
    mtime: bindings::timespec64,
    blocks: u64, // TODO: not properly updated right now. do we even need to store this persistently?
    size: u64,
    ino: InodeNum,
    _padding: u64,
}

#[allow(dead_code)]
pub(crate) struct InodeWrapper<'a, State, Op, Type> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    inode_type: PhantomData<Type>,
    ino: InodeNum,
    vfs_inode: Option<*mut bindings::inode>, // TODO: make this an fs::INode? or point it directly to the inode info structure?
    inode: &'a mut HayleyFsInode,
}

impl<'a, State, Op, Type> PmObjWrapper for InodeWrapper<'a, State, Op, Type> {}

impl HayleyFsInode {
    /// Unsafe inode constructor for temporary use with init_fs only
    /// Does not flush the root inode
    pub(crate) unsafe fn init_root_inode(sbi: &SbInfo, inode: *mut bindings::inode) -> Result<&HayleyFsInode> {
        let mut root_ino = unsafe { sbi.get_inode_by_ino_mut(ROOT_INO)? };
        root_ino.ino = ROOT_INO;
        root_ino.link_count = 2;
        root_ino.size = 4096; // dir size always set to 4KB
        root_ino.inode_type = InodeType::DIR;
        root_ino.uid = unsafe {
            bindings::from_kuid(
                &mut bindings::init_user_ns as *mut bindings::user_namespace,
                sbi.uid,
            )
        };
        root_ino.gid = unsafe {
            bindings::from_kgid(
                &mut bindings::init_user_ns as *mut bindings::user_namespace,
                sbi.gid,
            )
        };
        root_ino.blocks = 0;
        root_ino.mode = sbi.mode | bindings::S_IFDIR as u16;

        let time = unsafe { bindings::current_time(inode) };
        root_ino.ctime = time;
        root_ino.atime = time;
        root_ino.mtime = time;

        Ok(root_ino)
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    pub(crate) fn get_link_count(&self) -> u16 {
        self.link_count
    }

    pub(crate) fn get_type(&self) -> InodeType {
        self.inode_type
    }

    pub(crate) fn get_size(&self) -> u64 {
        self.size
    }

    pub(crate) fn get_mode(&self) -> u16 {
        self.mode
    }

    pub(crate) fn get_uid(&self) -> u32 {
        self.uid
    }

    pub(crate) fn get_gid(&self) -> u32 {
        self.gid
    }

    pub(crate) fn get_mtime(&self) -> bindings::timespec64 {
        self.mtime
    }

    pub(crate) fn get_ctime(&self) -> bindings::timespec64 {
        self.ctime
    }

    pub(crate) fn get_atime(&self) -> bindings::timespec64 {
        self.atime
    }

    pub(crate) fn get_blocks(&self) -> u64 {
        self.blocks
    }

    pub(crate) unsafe fn inc_link_count(&mut self) {
        self.link_count += 1
    }

    pub(crate) unsafe fn dec_link_count(&mut self) {
        self.link_count -= 1
    }

    pub(crate) unsafe fn update_atime(&mut self, atime: bindings::timespec64) {
        self.atime = atime;
    }

    // TODO: update as fields are added
    pub(crate) fn is_initialized(&self) -> bool {
        self.inode_type != InodeType::NONE && 
        self.link_count != 0 &&
        self.mode != 0 &&
        // uid/gid == 0 is root
        // TODO: check timestamps?
        self.ino != 0
    }

    // TODO: update as fields are added
    pub(crate) fn is_free(&self) -> bool {
        // if ANY field is non-zero, the inode is not free
        self.inode_type == InodeType::NONE &&
        self.link_count == 0 &&
        self.mode == 0 &&
        self.uid == 0 &&
        self.gid == 0 &&
        self.ctime.tv_sec == 0 &&
        self.ctime.tv_nsec == 0 &&
        self.atime.tv_sec == 0 &&
        self.atime.tv_nsec == 0 &&
        self.atime.tv_sec == 0 &&
        self.atime.tv_nsec == 0 &&
        self.blocks == 0 &&
        self.size == 0 &&
        self.ino == 0
    }
}

impl<'a, State, Op, Type> InodeWrapper<'a, State, Op, Type> {
    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    pub(crate) fn get_size(&self) -> u64 {
        self.inode.get_size()
    }

    pub(crate) fn get_uid(&self) -> u32 {
        self.inode.get_uid()
    }

    pub(crate) fn get_gid(&self) -> u32 {
        self.inode.get_gid()
    }

    pub(crate) fn get_mtime(&self) -> bindings::timespec64 {
        self.inode.get_mtime()
    }

    pub(crate) fn get_ctime(&self) -> bindings::timespec64 {
        self.inode.get_ctime()
    }

    pub(crate) fn get_atime(&self) -> bindings::timespec64 {
        self.inode.get_atime()
    }

    pub(crate) fn get_blocks(&self) -> u64 {
        self.inode.get_blocks()
    }
}

impl<'a, State, Op, Type> InodeWrapper<'a, State, Op, Type> {
    // TODO: this needs to be handled specially for types so that type generic cannot be incorrect
    pub(crate) fn wrap_inode(
        vfs_inode: *mut bindings::inode,
        pi: &'a mut HayleyFsInode,
    ) -> InodeWrapper<'a, State, Op, Type> {
        InodeWrapper {
            state: PhantomData,
            op: PhantomData,
            inode_type: PhantomData,
            vfs_inode: Some(vfs_inode),
            ino: unsafe {(*vfs_inode).i_ino},
            inode: pi,
        }
    }

    pub(crate) fn new<NewState, NewOp>(
        i: InodeWrapper<'a, State, Op, Type>,
    ) -> InodeWrapper<'a, NewState, NewOp, Type> {
        InodeWrapper {
            state: PhantomData,
            op: PhantomData,
            ino: i.ino,
            inode_type: i.inode_type,
            vfs_inode: i.vfs_inode,
            inode: i.inode,
        }
    }

    pub(crate) fn get_type(&self) -> InodeType {
        self.inode.get_type()
    }
}

impl<'a, State, Op> InodeWrapper<'a, State, Op, RegInode> {
    fn get_inode_info(&self) -> Result<&HayleyFsRegInodeInfo> {
        match self.vfs_inode {
            Some(vfs_inode) => unsafe {Ok(<Box::<HayleyFsRegInodeInfo> as ForeignOwnable>::borrow((*vfs_inode).i_private))},
            None => {pr_info!("ERROR: inode is uninitialized\n"); Err(EPERM)}
        }
    }
}

impl<'a, Type> InodeWrapper<'a, Clean, Start, Type> {
    // this is only called in dirty_inode, so it consumes itself
    pub(crate) fn update_atime(self, atime: bindings::timespec64) {
        unsafe { self.inode.update_atime(atime) };
    }

    pub(crate) fn inc_link_count(self) -> Result<InodeWrapper<'a, Dirty, IncLink, Type>> {
        if self.inode.get_link_count() == MAX_LINKS {
            Err(EMLINK)
        } else {
            unsafe { self.inode.inc_link_count() };
            // also update the inode's ctime. the time update may be reordered with the link change 
            // we make no guarantees about ordering of these two updates
            if let Some(vfs_inode) = self.vfs_inode {
                self.inode.ctime = unsafe { bindings::current_time(vfs_inode)};
            } else {
                pr_info!("ERROR: no vfs inode for inode {:?} in dec_link_count\n", self.ino);
                return Err(EINVAL);
            }
            Ok(Self::new(self))
        }
    }

    #[allow(dead_code)]
    pub(crate) fn dec_link_count(self, _dentry: &DentryWrapper<'a, Clean, ClearIno>) -> Result<InodeWrapper<'a, Dirty, DecLink, Type>> {
        if self.inode.get_link_count() == 0 {
            Err(ENOENT)
        } else {
            unsafe { self.inode.dec_link_count() };
            // also update the inode's ctime. the time update may be reordered with the link change 
            // we make no guarantees about ordering of these two updates
            if let Some(vfs_inode) = self.vfs_inode {
                self.inode.ctime = unsafe { bindings::current_time(vfs_inode)};
            } else {
                pr_info!("ERROR: no vfs inode for inode {:?} in dec_link_count\n", self.ino);
                return Err(EINVAL);
            }
            Ok(Self::new(self))
        }
    }

    // TODO: get the number of bytes written from the page itself, somehow?
    #[allow(dead_code)]
    pub(crate) fn inc_size(
        self,
        bytes_written: u64,
        current_offset: u64,
        _page: DataPageWrapper<'a, Clean, Written>,
    ) -> (u64, InodeWrapper<'a, Clean, IncSize, Type>) {
        let total_size = bytes_written + current_offset;
        // also update the inode's ctime and mtime. the time update may be reordered with the size change
        // we make no guarantees about ordering of these two updates
        if let Some(vfs_inode) = self.vfs_inode {
            let time = unsafe { bindings::current_time(vfs_inode) };
            self.inode.ctime = time;
            self.inode.mtime = time;
        } else {
            panic!("ERROR: no vfs inode for inode {:?} in dec_link_count\n", self.ino);
        }
        if self.inode.size < total_size {
            self.inode.size = total_size;
            flush_buffer(self.inode, mem::size_of::<HayleyFsInode>(), true);
        }
        (
            self.inode.size,
            InodeWrapper {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                vfs_inode: self.vfs_inode,
                ino: self.ino,
                inode: self.inode,
            },
        )
    }
}

impl<'a> InodeWrapper<'a, Clean, DecLink, RegInode> {
    // this is horrifying
    pub(crate) fn try_complete_unlink(self, sbi: &'a SbInfo) -> Result<core::result::Result<InodeWrapper<'a, Clean, Complete, RegInode>, (InodeWrapper<'a, Clean, Dealloc, RegInode>, Vec<DataPageWrapper<'a, Clean, ToUnmap>>)>> {
        if self.inode.get_link_count() > 0 {
            Ok(Ok(InodeWrapper {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                vfs_inode: self.vfs_inode,
                ino: self.ino,
                inode: self.inode,
            }))
        } else {
            // get the list of pages associated with this inode and convert them into 
            // ToUnmap wrappers
            // let pages = sbi.ino_data_page_map.get_all_pages(&self.get_ino())?;
            let info = self.get_inode_info()?;
            let pages = info.get_all_pages()?;
            let mut unmap_vec = Vec::new();
            for page in pages.values() {
                let p = DataPageWrapper::mark_to_unmap(sbi, page)?;
                unmap_vec.try_push(p)?;
            }
            Ok(
                Err(
                    (InodeWrapper {
                        state: PhantomData,
                        op: PhantomData,
                        inode_type: PhantomData,
                        vfs_inode: self.vfs_inode,
                        ino: self.ino,
                        inode: self.inode,
                    }, 
                    unmap_vec)
                )
            )
        }
    }
}

impl<'a> InodeWrapper<'a, Clean, Dealloc, RegInode> {
    // NOTE: data page wrappers don't actually need to be free, they just need to be in ClearIno
    pub(crate) fn dealloc(self, _freed_pages: Vec<DataPageWrapper<'a, Clean, Free>>) -> InodeWrapper<'a, Dirty, Complete, RegInode> {
        self.inode.inode_type = InodeType::NONE;
        // link count should already be 0
        assert!(self.inode.link_count == 0);
        self.inode.mode = 0;
        self.inode.uid = 0;
        self.inode.gid = 0;
        self.inode.ctime.tv_sec = 0;
        self.inode.ctime.tv_nsec = 0;
        self.inode.atime.tv_sec = 0;
        self.inode.atime.tv_nsec = 0;
        self.inode.mtime.tv_sec = 0;
        self.inode.mtime.tv_nsec = 0;
        self.inode.blocks = 0;
        self.inode.size = 0;
        self.inode.ino = 0;

        InodeWrapper {
            state: PhantomData,
            op: PhantomData,
            inode_type: PhantomData,
            vfs_inode: self.vfs_inode,
            ino: self.ino,
            inode: self.inode
        }
    }
}

impl<'a> InodeWrapper<'a, Clean, Free, RegInode> {
    pub(crate) fn get_free_reg_inode_by_ino(sbi: &'a SbInfo, ino: InodeNum) -> Result<Self> {
        let raw_inode = unsafe { sbi.get_inode_by_ino_mut(ino)? };
        if raw_inode.is_free() {
            Ok(InodeWrapper {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                vfs_inode: None,
                ino,
                inode: raw_inode,
            })
        } else {
            pr_info!("ERROR: regular inode {:?} is not free\n", ino);
            Err(EPERM)
        }
    }

    pub(crate) fn allocate_file_inode(
        self,
        inode: &fs::INode,
        mode: u16,
    ) -> Result<InodeWrapper<'a, Dirty, Alloc, RegInode>> {
        self.inode.link_count = 1;
        self.inode.ino = self.ino;
        self.inode.inode_type = InodeType::REG;
        self.inode.mode = mode;
        self.inode.blocks = 0;
        self.inode.uid = unsafe { (*inode.get_inner()).i_uid.val };
        self.inode.gid = unsafe { (*inode.get_inner()).i_gid.val };
        let time = unsafe { bindings::current_time(inode.get_inner()) };
        self.inode.ctime = time;
        self.inode.atime = time;
        self.inode.mtime = time;
        Ok(Self::new(self))
    }
}

impl<'a> InodeWrapper<'a, Clean, Free, DirInode> {
    pub(crate) fn get_free_dir_inode_by_ino(sbi: &'a SbInfo, ino: InodeNum) -> Result<Self> {
        let raw_inode = unsafe { sbi.get_inode_by_ino_mut(ino)? };
        if raw_inode.is_free() {
            Ok(InodeWrapper {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                vfs_inode: None,
                ino,
                inode: raw_inode,
            })
        } else {
            pr_info!("ERROR: dir inode {:?} is not free\n", ino);
            Err(EPERM)
        }
    }

    pub(crate) fn allocate_dir_inode(
        self,
        parent: &fs::INode,
        mode: u16,
    ) -> Result<InodeWrapper<'a, Dirty, Alloc, DirInode>> {
        self.inode.link_count = 2;
        self.inode.ino = self.ino;
        self.inode.blocks = 0;
        self.inode.inode_type = InodeType::DIR;
        self.inode.mode = mode | bindings::S_IFDIR as u16;
        self.inode.uid = unsafe { (*parent.get_inner()).i_uid.val };
        self.inode.gid = unsafe { (*parent.get_inner()).i_gid.val };
        let time = unsafe { bindings::current_time(parent.get_inner()) };
        self.inode.ctime = time;
        self.inode.atime = time;
        self.inode.mtime = time;
        Ok(Self::new(self))
    }
}

impl<'a, Op, Type> InodeWrapper<'a, Dirty, Op, Type> {
    pub(crate) fn flush(self) -> InodeWrapper<'a, InFlight, Op, Type> {
        flush_buffer(self.inode, mem::size_of::<HayleyFsInode>(), false);
        Self::new(self)
    }
}

impl<'a, Op, Type> InodeWrapper<'a, InFlight, Op, Type> {
    pub(crate) fn fence(self) -> InodeWrapper<'a, Clean, Op, Type> {
        sfence();
        Self::new(self)
    }
}

/// Interface for volatile inode allocator structures
pub(crate) trait InodeAllocator {
    fn new(val: u64) -> Result<Self> where Self: Sized;
    fn new_from_alloc_vec(alloc_inodes: Vec<InodeNum>, start: u64) -> Result<Self> where Self: Sized;
    fn alloc_ino(&self) -> Result<InodeNum>;
    // TODO: should this be unsafe or require a free inode wrapper?
    fn dealloc_ino(&self, ino: InodeNum) -> Result<()>;
}

pub(crate) struct RBInodeAllocator {
    map: Arc<Mutex<RBTree<InodeNum, ()>>>,
}

impl InodeAllocator for RBInodeAllocator {
    fn new(val: u64) -> Result<Self> {
        let mut rb = RBTree::new();
        for i in val..NUM_INODES {
            rb.try_insert(i, ())?;
        }
        Ok(Self {
            map: Arc::try_new(Mutex::new(rb))?
        })
    }

    fn new_from_alloc_vec(alloc_inodes: Vec<InodeNum>, start: u64) -> Result<Self> {
        let mut rb = RBTree::new();
        let mut cur_ino = start;
        let mut i = 0;
        while cur_ino < NUM_INODES && i < alloc_inodes.len() {
            if cur_ino < alloc_inodes[i] {
                rb.try_insert(cur_ino, ())?;
                cur_ino += 1;
            } else if cur_ino == alloc_inodes[i] {
                cur_ino += 1;
                i += 1;
            } else {
                // cur_ino > alloc_pages[i]
                // shouldn't ever happen
                pr_info!("ERROR: cur_ino {:?}, i {:?}, alloc_inodes[i] {:?}\n", cur_ino, i, alloc_inodes[i]);
                return Err(EINVAL);
            }
        }
        // add all remaining inodes to the allocator
        if i < NUM_INODES.try_into()? {
            for j in i..NUM_INODES.try_into()? {
                rb.try_insert(j.try_into()?, ())?;
            }
        }
        Ok(Self {
            map: Arc::try_new(Mutex::new(rb))?
        })
    }

    fn alloc_ino(&self) -> Result<InodeNum> {
        let map = Arc::clone(&self.map);
        let mut map = map.lock();
        let iter = map.iter().next();
        let ino = match iter {
            None => {
                pr_info!("ERROR: ran out of inodes in RB inode allocator\n");
                return Err(ENOSPC);
            } 
            Some(ino) => *ino.0
        };
        map.remove(&ino);
        Ok(ino)
    }

    fn dealloc_ino(&self, ino: InodeNum) -> Result<()> {
        let map = Arc::clone(&self.map);
        let mut map = map.lock();
        let res = map.try_insert(ino, ())?;
        if res.is_some() {
            pr_info!("ERROR: inode {:?} was deallocated but is already in allocator\n", ino);
            Err(EINVAL)
        } else {
            Ok(())
        }
    }
}