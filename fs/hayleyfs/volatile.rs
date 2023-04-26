use crate::balloc::*;
use crate::defs::*;
use crate::typestate::*;
use core::ffi;
use kernel::prelude::*;
use kernel::{
    rbtree::RBTree,
    sync::{smutex::Mutex, Arc},
};

// TODO: how should name be represented here? array is probably not the best?
#[allow(dead_code)]
#[derive(Debug, Copy, Clone)]
pub(crate) struct DentryInfo {
    ino: InodeNum,
    virt_addr: *const ffi::c_void,
    name: [u8; MAX_FILENAME_LEN],
}

#[allow(dead_code)]
impl DentryInfo {
    pub(crate) fn new(
        ino: InodeNum,
        virt_addr: *const ffi::c_void,
        name: [u8; MAX_FILENAME_LEN],
    ) -> Self {
        Self {
            ino,
            virt_addr,
            name,
        }
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    pub(crate) fn get_virt_addr(&self) -> *const ffi::c_void {
        self.virt_addr
    }

    pub(crate) fn get_name(&self) -> &[u8; MAX_FILENAME_LEN] {
        &self.name
    }
}

// /// maps inodes to info about dentries for inode's children
// pub(crate) trait InoDentryMap {
//     fn new() -> Result<Self>
//     where
//         Self: Sized;
//     fn insert(&self, ino: InodeNum, dentry: DentryInfo) -> Result<()>;
//     fn lookup_dentry(&self, ino: &InodeNum, name: &CStr) -> Option<DentryInfo>;
//     fn delete(&self, ino: InodeNum, dentry: DentryInfo) -> Result<()>;
// }

#[allow(dead_code)]
pub(crate) struct InoDentryTree {
    map: Arc<Mutex<RBTree<InodeNum, Vec<DentryInfo>>>>,
}

impl InoDentryTree {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            map: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    pub(crate) fn insert(&self, ino: InodeNum, dentry: DentryInfo) -> Result<()> {
        let map = Arc::clone(&self.map);
        let mut map = map.lock();
        if let Some(ref mut node) = map.get_mut(&ino) {
            node.try_push(dentry)?;
        } else {
            let mut vec = Vec::new();
            vec.try_push(dentry)?;
            map.try_insert(ino, vec)?;
        }
        Ok(())
    }

    pub(crate) fn remove(&self, ino: InodeNum) -> Option<Vec<DentryInfo>> {
        let map = Arc::clone(&self.map);
        let mut map = map.lock();
        map.remove(&ino)
    }
}

fn str_equals(str1: &CStr, str2: &CStr) -> bool {
    if str1.len_with_nul() != str2.len_with_nul() {
        return false;
    }
    let len = str1.len_with_nul();
    let str1 = str1.as_bytes_with_nul();
    let str2 = str2.as_bytes_with_nul();
    for i in 0..len {
        if str1[i] != str2[i] {
            return false;
        }
    }
    return true;
}

#[derive(Debug, Copy, Clone, PartialOrd, Eq, PartialEq, Ord)]
pub(crate) struct DirPageInfo {
    // owner: InodeNum,
    page_no: PageNum,
    // full: bool,
    // virt_addr: *mut ffi::c_void,
}

impl DirPageInfo {
    pub(crate) fn get_page_no(&self) -> PageNum {
        self.page_no
    }

    pub(crate) fn new(page_no: PageNum) -> Self {
        Self { page_no }
    }
}

#[derive(Debug, Copy, Clone)]
#[allow(dead_code)]
pub(crate) struct DataPageInfo {
    owner: InodeNum,
    page_no: PageNum,
    offset: u64,
}

impl DataPageInfo {
    pub(crate) fn new(owner: InodeNum, page_no: PageNum, offset: u64) -> Self {
        Self {
            owner,
            page_no,
            offset,
        }
    }

    pub(crate) fn get_page_no(&self) -> PageNum {
        self.page_no
    }
}

// TODO: could just be offset and page number - storing whole DataPageInfo is redundant...
#[repr(C)]
pub(crate) struct HayleyFsRegInodeInfo {
    ino: InodeNum,
    // pages: Arc<Mutex<Vec<DataPageInfo>>>,
    pages: Arc<Mutex<RBTree<u64, DataPageInfo>>>,
    // num_pages: usize,
}

impl HayleyFsRegInodeInfo {
    pub(crate) fn new(ino: InodeNum) -> Result<Self> {
        Ok(Self {
            ino,
            // pages: Arc::try_new(Mutex::new(Vec::new()))?,
            pages: Arc::try_new(Mutex::new(RBTree::new()))?,
            // num_pages: 0,
        })
    }

    pub(crate) fn new_from_tree(ino: InodeNum, tree: RBTree<u64, DataPageInfo>) -> Result<Self> {
        // let rb_tree = RBTree::new();
        // for page in vec {
        //     rb_tree.try_insert(page.offset, page)?;
        // }
        // let num_pages = tree.keys().len();
        Ok(Self {
            ino,
            pages: Arc::try_new(Mutex::new(tree))?,
            // num_pages,
        })
    }
}

/// maps file inodes to info about their pages
pub(crate) trait InoDataPageMap {
    fn new(ino: InodeNum) -> Result<Self>
    where
        Self: Sized;
    fn insert<'a, State: Initialized>(
        &self,
        page: &DataPageWrapper<'a, Clean, State>,
    ) -> Result<()>;
    fn find(&self, offset: u64) -> Option<DataPageInfo>;
    fn get_all_pages(&self) -> Result<RBTree<u64, DataPageInfo>>;
    // fn delete(&self) -> Result<DataPageInfo>;
}

impl InoDataPageMap for HayleyFsRegInodeInfo {
    fn new(ino: InodeNum) -> Result<Self> {
        HayleyFsRegInodeInfo::new(ino)
    }

    fn insert<'a, State: Initialized>(
        &self,
        page: &DataPageWrapper<'a, Clean, State>,
    ) -> Result<()> {
        let pages = Arc::clone(&self.pages);
        let mut pages = pages.lock();
        let offset = page.get_offset();
        pages.try_insert(
            offset,
            DataPageInfo {
                owner: self.ino,
                page_no: page.get_page_no(),
                offset,
            },
        )?;
        Ok(())
    }

    fn find(&self, offset: u64) -> Option<DataPageInfo> {
        let pages = Arc::clone(&self.pages);
        let pages = pages.lock();
        // let index: usize = (offset / HAYLEYFS_PAGESIZE).try_into().unwrap();
        let result = pages.get(&offset);
        match result {
            Some(page) => Some(page.clone()),
            None => None,
        }
    }

    fn get_all_pages(&self) -> Result<RBTree<u64, DataPageInfo>> {
        let pages = Arc::clone(&self.pages);
        let pages = pages.lock();
        let mut return_tree = RBTree::new();
        // TODO: can you do this without copying all of the pages?
        for offset in pages.keys() {
            return_tree.try_insert(*offset, pages.get(offset).unwrap().clone())?;
        }
        // let mut return_vec = Vec::new();
        // // TODO: can you do this without copying all of the pages?
        // for page in &*pages {
        //     return_vec.try_push(page.clone())?;
        // }
        // pages.clear();
        // Ok(return_vec)
        Ok(return_tree)
    }

    // /// Deletes the last page in the file from the index and returns it
    // fn delete(&self) -> Result<DataPageInfo> {
    //     let pages = Arc::clone(&self.pages);
    //     let mut pages = pages.lock();
    //     pages.pop().ok_or(EINVAL)
    // }
}

/// maps dir inodes to info about their pages
pub(crate) trait InoDirPageMap {
    fn new(ino: InodeNum) -> Result<Self>
    where
        Self: Sized;
    fn insert<'a, State: Initialized>(&self, page: &DirPageWrapper<'a, Clean, State>)
        -> Result<()>;
    fn find_page_with_free_dentry(&self, sbi: &SbInfo) -> Result<Option<DirPageInfo>>;
    fn get_all_pages(&self) -> Result<RBTree<DirPageInfo, ()>>;
    fn delete(&self, page: DirPageInfo) -> Result<()>;
}

#[repr(C)]
pub(crate) struct HayleyFsDirInodeInfo {
    ino: InodeNum,
    pages: Arc<Mutex<RBTree<DirPageInfo, ()>>>,
    dentries: Arc<Mutex<Vec<DentryInfo>>>,
}

impl HayleyFsDirInodeInfo {
    pub(crate) fn new(ino: InodeNum) -> Result<Self> {
        Ok(Self {
            ino,
            pages: Arc::try_new(Mutex::new(RBTree::new()))?,
            dentries: Arc::try_new(Mutex::new(Vec::new()))?,
        })
    }

    pub(crate) fn new_from_vec(
        ino: InodeNum,
        page_tree: RBTree<DirPageInfo, ()>,
        dentry_vec: Vec<DentryInfo>,
    ) -> Result<Self> {
        Ok(Self {
            ino,
            pages: Arc::try_new(Mutex::new(page_tree))?,
            dentries: Arc::try_new(Mutex::new(dentry_vec))?,
        })
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }
}

impl InoDirPageMap for HayleyFsDirInodeInfo {
    fn new(ino: InodeNum) -> Result<Self> {
        Self::new(ino)
    }

    fn insert<'a, State: Initialized>(
        &self,
        page: &DirPageWrapper<'a, Clean, State>,
    ) -> Result<()> {
        let pages = Arc::clone(&self.pages);
        let mut pages = pages.lock();
        // TODO: ordering?
        let page_info = DirPageInfo {
            // owner: self.ino,
            page_no: page.get_page_no(),
        };
        pages.try_insert(page_info, ())?;
        Ok(())
    }

    // TODO: this only works because we don't ever deallocate dir pages right now
    // there could be a race between a process that is deleting a dir page and a
    // process trying to add a dentry to it. this method should just add the dentry
    fn find_page_with_free_dentry<'a>(&self, sbi: &SbInfo) -> Result<Option<DirPageInfo>> {
        let pages = Arc::clone(&self.pages);
        let pages = pages.lock();
        for page in pages.keys() {
            let p = DirPageWrapper::from_page_no(sbi, page.get_page_no())?;
            if p.has_free_space(sbi)? {
                return Ok(Some(page.clone()));
            }
        }

        Ok(None)
    }

    fn get_all_pages(&self) -> Result<RBTree<DirPageInfo, ()>> {
        let pages = Arc::clone(&self.pages);
        let pages = pages.lock();
        let mut return_tree = RBTree::new();
        // TODO: can you do this without copying all of the pages?
        for page in pages.keys() {
            return_tree.try_insert(page.clone(), ())?;
        }
        // pages.clear();
        Ok(return_tree)
    }

    // TODO: implement
    fn delete(&self, _page: DirPageInfo) -> Result<()> {
        unimplemented!();
    }
}

pub(crate) trait InoDentryMap {
    fn insert_dentry(&self, dentry: DentryInfo) -> Result<()>;
    fn lookup_dentry(&self, name: &CStr) -> Option<DentryInfo>;
    fn get_all_dentries(&self) -> Result<Vec<DentryInfo>>;
    fn delete_dentry(&self, dentry: DentryInfo) -> Result<()>;
}

impl InoDentryMap for HayleyFsDirInodeInfo {
    fn insert_dentry(&self, dentry: DentryInfo) -> Result<()> {
        let dentries = Arc::clone(&self.dentries);
        let mut dentries = dentries.lock();
        dentries.try_push(dentry)?;
        Ok(())
    }

    fn lookup_dentry(&self, name: &CStr) -> Option<DentryInfo> {
        let dentries = Arc::clone(&self.dentries);
        let dentries = dentries.lock();
        for dentry in &*dentries {
            let dentry_name = unsafe { CStr::from_char_ptr(dentry.name.as_ptr() as *const i8) };
            if str_equals(name, dentry_name) {
                return Some(dentry.clone());
            }
        }
        None
    }

    fn get_all_dentries(&self) -> Result<Vec<DentryInfo>> {
        let dentries = Arc::clone(&self.dentries);
        let dentries = dentries.lock();
        let mut return_vec = Vec::new();
        // TODO: don't clone all of the dentries to return?
        for dentry in &*dentries {
            return_vec.try_push(dentry.clone())?
        }
        Ok(return_vec)
    }

    fn delete_dentry(&self, dentry: DentryInfo) -> Result<()> {
        let dentries = Arc::clone(&self.dentries);
        let mut dentries = dentries.lock();
        dentries.retain(|x| x.virt_addr != dentry.virt_addr);
        Ok(())
    }
}

pub(crate) trait PageInfo {}
impl PageInfo for DataPageInfo {}
impl PageInfo for DirPageInfo {}

// pub(crate) struct InoPageTree<T: PageInfo> {
//     tree: Arc<Mutex<RBTree<InodeNum, Vec<T>>>>,
// }

// impl<T: PageInfo> InoPageTree<T> {
//     pub(crate) fn new() -> Result<Self> {
//         Ok(Self {
//             tree: Arc::try_new(Mutex::new(RBTree::new()))?,
//         })
//     }

//     pub(crate) fn insert_vec(&self, ino: InodeNum, pages: Vec<T>) -> Result<()> {
//         let tree = Arc::clone(&self.tree);
//         let mut tree = tree.lock();
//         tree.try_insert(ino, pages)?;
//         Ok(())
//     }

//     pub(crate) fn insert_one(&self, ino: InodeNum, page: T) -> Result<()> {
//         let tree = Arc::clone(&self.tree);
//         let mut tree = tree.lock();
//         let entry = tree.get_mut(&ino);
//         if let Some(entry) = entry {
//             entry.try_push(page)?;
//         } else {
//             let mut new_vec = Vec::new();
//             new_vec.try_push(page)?;
//             tree.try_insert(ino, new_vec)?;
//         }
//         Ok(())
//     }

//     pub(crate) fn remove(&self, ino: InodeNum) -> Option<Vec<T>> {
//         let tree = Arc::clone(&self.tree);
//         let mut tree = tree.lock();
//         tree.remove(&ino)
//     }
// }

pub(crate) struct InoDataPageTree {
    tree: Arc<Mutex<RBTree<InodeNum, RBTree<u64, DataPageInfo>>>>,
}

impl InoDataPageTree {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            tree: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    pub(crate) fn insert_inode(
        &self,
        ino: InodeNum,
        pages: RBTree<u64, DataPageInfo>,
    ) -> Result<()> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        tree.try_insert(ino, pages)?;
        Ok(())
    }

    pub(crate) fn remove(&self, ino: InodeNum) -> Option<RBTree<u64, DataPageInfo>> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        tree.remove(&ino)
    }
}

pub(crate) struct InoDirPageTree {
    tree: Arc<Mutex<RBTree<InodeNum, RBTree<DirPageInfo, ()>>>>,
}

impl InoDirPageTree {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            tree: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    pub(crate) fn insert_inode(&self, ino: InodeNum, pages: RBTree<DirPageInfo, ()>) -> Result<()> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        tree.try_insert(ino, pages)?;
        Ok(())
    }

    pub(crate) fn insert_one(&self, ino: InodeNum, page: DirPageInfo) -> Result<()> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        let entry = tree.get_mut(&ino);
        if let Some(entry) = entry {
            // entry.try_push(page)?;
            entry.try_insert(page, ())?;
        } else {
            let mut new_tree = RBTree::new();
            // new_tree.try_push(page)?;
            new_tree.try_insert(page, ())?;
            tree.try_insert(ino, new_tree)?;
        }
        Ok(())
    }

    pub(crate) fn remove(&self, ino: InodeNum) -> Option<RBTree<DirPageInfo, ()>> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        tree.remove(&ino)
    }
}
