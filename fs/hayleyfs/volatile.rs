use crate::balloc::*;
use crate::defs::*;
use crate::typestate::*;
use core::ffi;
use kernel::prelude::*;
use kernel::{
    bindings,
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
}

/// maps inodes to info about dentries for inode's children
pub(crate) trait InoDentryMap {
    fn new() -> Result<Self>
    where
        Self: Sized;
    fn insert(&self, ino: InodeNum, dentry: DentryInfo) -> Result<()>;
    fn lookup_dentry(&self, ino: &InodeNum, name: &CStr) -> Option<DentryInfo>;
    fn delete(&self, ino: InodeNum, dentry: DentryInfo) -> Result<()>;
}

#[allow(dead_code)]
pub(crate) struct BasicInoDentryMap {
    map: Arc<Mutex<RBTree<InodeNum, Vec<DentryInfo>>>>,
}

impl InoDentryMap for BasicInoDentryMap {
    fn new() -> Result<Self> {
        Ok(Self {
            map: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    fn insert(&self, ino: InodeNum, dentry: DentryInfo) -> Result<()> {
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

    fn lookup_dentry(&self, ino: &InodeNum, name: &CStr) -> Option<DentryInfo> {
        let map = Arc::clone(&self.map);
        let map = map.lock();
        let dentry_vec = map.get(&ino);
        if let Some(dentry_vec) = dentry_vec {
            for dentry in dentry_vec {
                let dentry_name = unsafe { CStr::from_char_ptr(dentry.name.as_ptr() as *const i8) };
                if str_equals(name, dentry_name) {
                    return Some(dentry.clone());
                }
            }
        }
        None
    }

    fn delete(&self, ino: InodeNum, dentry: DentryInfo) -> Result<()> {
        let map = Arc::clone(&self.map);
        let mut map = map.lock();
        let mut dentry_vec = map.get_mut(&ino);
        if let Some(ref mut dentry_vec) = dentry_vec {
            dentry_vec.retain(|x| x.virt_addr != dentry.virt_addr);
        }
        Ok(())
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

#[allow(dead_code)]
#[derive(Debug, Copy, Clone)]
pub(crate) struct DirPageInfo {
    owner: InodeNum,
    page_no: PageNum,
    // full: bool,
    // virt_addr: *mut ffi::c_void,
}

impl DirPageInfo {
    pub(crate) fn get_page_no(&self) -> PageNum {
        self.page_no
    }
}

/// maps dir inodes to info about their pages
pub(crate) trait InoDirPageMap {
    fn new() -> Result<Self>
    where
        Self: Sized;
    fn insert<'a, State: Initialized>(
        &self,
        ino: InodeNum,
        page: &DirPageWrapper<'a, Clean, State>,
    ) -> Result<()>;
    fn find_page_with_free_dentry(
        &self,
        sbi: &SbInfo,
        ino: &InodeNum,
    ) -> Result<Option<DirPageInfo>>;
    fn delete(&self, ino: InodeNum, page: DirPageInfo) -> Result<()>;
}

pub(crate) struct BasicInoDirPageMap {
    map: Arc<Mutex<RBTree<InodeNum, Vec<DirPageInfo>>>>,
}

impl InoDirPageMap for BasicInoDirPageMap {
    fn new() -> Result<Self> {
        Ok(Self {
            map: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    fn insert<'a, State: Initialized>(
        &self,
        ino: InodeNum,
        page: &DirPageWrapper<'a, Clean, State>,
    ) -> Result<()> {
        let map = Arc::clone(&self.map);
        let mut map = map.lock();
        let page_no = page.get_page_no();
        let page_info = DirPageInfo {
            owner: ino,
            page_no,
        };
        if let Some(node) = map.get_mut(&ino) {
            node.try_push(page_info)?;
        } else {
            let mut vec = Vec::new();
            vec.try_push(page_info)?;
            map.try_insert(ino, vec)?;
        }
        Ok(())
    }

    fn find_page_with_free_dentry<'a>(
        &self,
        sbi: &SbInfo,
        ino: &InodeNum,
    ) -> Result<Option<DirPageInfo>> {
        let map = Arc::clone(&self.map);
        let map = map.lock();
        let pages = map.get(&ino);
        if let Some(pages) = pages {
            for page in pages {
                let p = DirPageWrapper::from_page_no(sbi, page.get_page_no())?;
                if p.has_free_space(sbi)? {
                    return Ok(Some(page.clone()));
                }
            }
        }

        Ok(None)
    }

    fn delete(&self, _ino: InodeNum, _page: DirPageInfo) -> Result<()> {
        unimplemented!();
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

    pub(crate) fn get_offset(&self) -> u64 {
        self.offset
    }
}

#[repr(C)]
pub(crate) struct HayleyFsInodeInfoHeader {
    ino: InodeNum,
    pages: Arc<Mutex<Vec<DataPageInfo>>>,
}

#[repr(C)]
pub(crate) struct HayleyFsInodeInfo {
    header: HayleyFsInodeInfoHeader,
    pub(crate) vfs_inode: bindings::inode, // has to be public for container_of! lookups
}

impl HayleyFsInodeInfo {
    // takes a reference to self because alloc_inode allocates
    // the header structures - we just want to make sure the
    // values are filled in properly
    pub(crate) fn set_up_header(&mut self) -> Result<()> {
        self.header.ino = 0; // TODO: at some point we need to set this!
        self.header.pages = Arc::try_new(Mutex::new(Vec::new()))?;
        Ok(())
    }

    pub(crate) fn set_ino(&mut self, ino: InodeNum) {
        self.header.ino = ino;
    }
}

// impl HayleyFsRegInodeInfo {
//     pub(crate) fn new(ino: InodeNum) -> Result<Self> {
//         Ok(Self {
//             ino,
//             pages: Arc::try_new(Mutex::new(Vec::new()))?,
//         })
//     }

//     pub(crate) fn new_from_vec(ino: InodeNum, vec: Vec<DataPageInfo>) -> Result<Self> {
//         Ok(Self {
//             ino,
//             pages: Arc::try_new(Mutex::new(vec))?,
//         })
//     }
// }

/// maps file inodes to info about their pages
pub(crate) trait InoDataPageMap {
    // fn new(ino: InodeNum) -> Result<Self>
    // where
    //     Self: Sized;
    fn insert<'a, State: Initialized>(
        &self,
        page: &DataPageWrapper<'a, Clean, State>,
    ) -> Result<()>;
    fn insert_pages(&self, vec: Vec<DataPageInfo>) -> Result<()>;
    fn find(&self, offset: u64) -> Option<DataPageInfo>;
    fn remove_all_pages(&self) -> Result<Vec<DataPageInfo>>;
    // fn delete(&self) -> Result<DataPageInfo>;
}

impl InoDataPageMap for HayleyFsInodeInfo {
    fn insert<'a, State: Initialized>(
        &self,
        page: &DataPageWrapper<'a, Clean, State>,
    ) -> Result<()> {
        let pages = Arc::clone(&self.header.pages);
        let mut pages = pages.lock();
        let offset = page.get_offset();
        let page_no = page.get_page_no();
        // check that we aren't trying to insert a page at an offset that
        // already exists or in a way that will create a hole
        let index = offset / HAYLEYFS_PAGESIZE;
        if index != pages.len().try_into()? {
            pr_info!(
                    "ERROR: inode {:?} attempted to insert page {:?} at index {:?} (offset {:?}) but pages vector has length {:?}\n",
                    self.header.ino,
                    page_no,
                    index,
                    offset,
                    pages.len()
                );
            pr_info!("{:?}\n", pages[index as usize]);
            return Err(EINVAL);
        }
        // pr_info!("inserting at offset {:?} for inode {:?}\n", offset, self.ino);
        pages.try_push(DataPageInfo {
            owner: self.header.ino,
            page_no,
            offset,
        })?;
        Ok(())
    }

    fn insert_pages(&self, vec: Vec<DataPageInfo>) -> Result<()> {
        let pages = Arc::clone(&self.header.pages);
        let mut pages = pages.lock();
        for page in vec {
            pages.try_push(page)?;
        }
        Ok(())
    }

    fn find(&self, offset: u64) -> Option<DataPageInfo> {
        let pages = Arc::clone(&self.header.pages);
        let pages = pages.lock();
        let index: usize = (offset / HAYLEYFS_PAGESIZE).try_into().unwrap();
        let result = pages.get(index);
        match result {
            Some(page) => Some(page.clone()),
            None => None,
        }
    }

    fn remove_all_pages(&self) -> Result<Vec<DataPageInfo>> {
        let pages = Arc::clone(&self.header.pages);
        let mut pages = pages.lock();
        let mut return_vec = Vec::new();
        // TODO: can you do this without copying all of the pages?
        for page in &*pages {
            return_vec.try_push(page.clone())?;
        }
        pages.clear();
        Ok(return_vec)
    }

    //     /// Deletes the last page in the file from the index and returns it
    //     fn delete(&self) -> Result<DataPageInfo> {
    //         let pages = Arc::clone(&self.pages);
    //         let mut pages = pages.lock();
    //         pages.pop().ok_or(EINVAL)
    //     }
}

pub(crate) struct InoDataPageTree {
    tree: Arc<Mutex<RBTree<InodeNum, Vec<DataPageInfo>>>>,
}

impl InoDataPageTree {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            tree: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    pub(crate) fn insert(&self, ino: InodeNum, pages: Vec<DataPageInfo>) -> Result<()> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        tree.try_insert(ino, pages)?;
        Ok(())
    }

    pub(crate) fn remove(&self, ino: InodeNum) -> Option<Vec<DataPageInfo>> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        tree.remove(&ino)
    }
}
