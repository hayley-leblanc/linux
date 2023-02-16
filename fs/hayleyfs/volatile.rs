use crate::balloc::*;
use crate::defs::*;
use crate::typestate::*;
use core::ffi;
use kernel::prelude::*;
use kernel::{
    rbtree::RBTree,
    sync::{smutex::Mutex, Arc},
};

#[allow(dead_code)]
#[derive(Debug, Copy, Clone)]
pub(crate) struct DentryInfo {
    ino: InodeNum,
    virt_addr: *const ffi::c_void,
    name: *const ffi::c_char,
}

#[allow(dead_code)]
impl DentryInfo {
    pub(crate) fn new(
        ino: InodeNum,
        virt_addr: *const ffi::c_void,
        name: *const ffi::c_char,
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
        if let Some(node) = map.get_mut(&ino) {
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
                let dentry_name = unsafe { CStr::from_char_ptr(dentry.name) };
                if str_equals(name, dentry_name) {
                    return Some(dentry.clone());
                }
            }
        }
        None
    }

    fn delete(&self, _ino: InodeNum, _dentry: DentryInfo) -> Result<()> {
        // TODO
        unimplemented!();
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
    full: bool,
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
        full: bool,
    ) -> Result<()>;
    fn find_page_with_free_dentry(&self, ino: &InodeNum) -> Option<DirPageInfo>;
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
        full: bool,
    ) -> Result<()> {
        let map = Arc::clone(&self.map);
        let mut map = map.lock();
        let page_no = page.get_page_no();
        let page_info = DirPageInfo {
            owner: ino,
            page_no,
            full,
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

    fn find_page_with_free_dentry<'a>(&self, ino: &InodeNum) -> Option<DirPageInfo> {
        let map = Arc::clone(&self.map);
        let map = map.lock();
        let pages = map.get(&ino);
        if let Some(pages) = pages {
            for page in pages {
                // TODO: we never actually set page.full to true
                if !page.full {
                    return Some(page.clone());
                }
            }
        }
        None
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
    pub(crate) fn get_page_no(&self) -> PageNum {
        self.page_no
    }
}

/// maps file inodes to info about their pages
pub(crate) trait InoDataPageMap {
    fn new() -> Result<Self>
    where
        Self: Sized;
    fn insert<'a, State: Initialized>(
        &self,
        ino: InodeNum,
        page: &DataPageWrapper<'a, Clean, State>,
    ) -> Result<()>;
    fn find(&self, ino: &InodeNum, offset: u64) -> Option<DataPageInfo>;
    fn delete(&self, ino: &InodeNum, offset: u64) -> Result<()>;
}

#[allow(dead_code)]
pub(crate) struct BasicInoDataPageMap {
    map: Arc<Mutex<RBTree<InodeNum, Vec<DataPageInfo>>>>,
}

#[allow(dead_code)]
impl InoDataPageMap for BasicInoDataPageMap {
    fn new() -> Result<Self> {
        Ok(Self {
            map: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    fn insert<'a, State: Initialized>(
        &self,
        ino: InodeNum,
        page: &DataPageWrapper<'a, Clean, State>,
    ) -> Result<()> {
        let map = Arc::clone(&self.map);
        let mut map = map.lock();
        let page_no = page.get_page_no();
        let page_info = DataPageInfo {
            owner: ino,
            page_no,
            offset: page.get_offset(),
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

    fn find(&self, ino: &InodeNum, offset: u64) -> Option<DataPageInfo> {
        let map = Arc::clone(&self.map);
        let map = map.lock();
        let pages = map.get(&ino);
        if let Some(pages) = pages {
            for page in pages {
                if page.offset == offset {
                    return Some(page.clone());
                }
            }
        }
        None
    }

    fn delete(&self, _ino: &InodeNum, _offset: u64) -> Result<()> {
        unimplemented!();
    }
}
