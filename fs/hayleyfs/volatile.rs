use crate::balloc::*;
use crate::defs::*;
use crate::typestate::*;
use core::ffi;
use kernel::prelude::*;
use kernel::rbtree::RBTree;

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct DentryInfo {
    parent: InodeNum,
    virt_addr: *mut ffi::c_void,
    name: *mut ffi::c_char,
}

#[allow(dead_code)]
impl DentryInfo {
    pub(crate) fn new(
        parent: InodeNum,
        name: *mut ffi::c_char,
        virt_addr: *mut ffi::c_void,
    ) -> Self {
        Self {
            parent,
            virt_addr,
            name,
        }
    }
}

pub(crate) trait InoDentryMap {
    fn new() -> Self;
    fn insert(&mut self, ino: InodeNum, dentry: DentryInfo) -> Result<()>;
    fn lookup_ino(&self, ino: &InodeNum) -> Option<&Vec<DentryInfo>>;
    fn delete(&mut self, ino: InodeNum, dentry: DentryInfo) -> Result<()>;
}

#[allow(dead_code)]
pub(crate) struct BasicInoDentryMap {
    map: RBTree<InodeNum, Vec<DentryInfo>>,
}

#[allow(dead_code)]
impl BasicInoDentryMap {
    pub(crate) fn new() -> Self {
        Self { map: RBTree::new() }
    }
}

impl InoDentryMap for BasicInoDentryMap {
    fn new() -> Self {
        Self { map: RBTree::new() }
    }

    fn insert(&mut self, ino: InodeNum, dentry: DentryInfo) -> Result<()> {
        if let Some(node) = self.map.get_mut(&ino) {
            node.try_push(dentry)?;
        } else {
            let mut vec = Vec::new();
            vec.try_push(dentry)?;
            self.map.try_insert(ino, vec)?;
        }
        Ok(())
    }

    fn lookup_ino(&self, ino: &InodeNum) -> Option<&Vec<DentryInfo>> {
        self.map.get(&ino)
    }

    fn delete(&mut self, _ino: InodeNum, _dentry: DentryInfo) -> Result<()> {
        // TODO
        unimplemented!();
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct DirPageInfo {
    owner: InodeNum,
    page_no: PageNum,
    // virt_addr: *mut ffi::c_void,
}

impl DirPageInfo {
    pub(crate) fn get_page_no(&self) -> PageNum {
        self.page_no
    }
}

pub(crate) trait InoDirPageMap {
    fn new() -> Self;
    fn insert<'a>(&mut self, ino: InodeNum, page: &DirPageWrapper<'a, Clean, Init>) -> Result<()>;
    fn lookup_ino(&self, ino: &InodeNum) -> Option<&Vec<DirPageInfo>>;
    fn delete(&mut self, ino: InodeNum, page: DirPageInfo) -> Result<()>;
}

pub(crate) struct BasicInoDirPageMap {
    map: RBTree<InodeNum, Vec<DirPageInfo>>,
}

impl InoDirPageMap for BasicInoDirPageMap {
    fn new() -> Self {
        Self { map: RBTree::new() }
    }

    fn insert<'a>(&mut self, ino: InodeNum, page: &DirPageWrapper<'a, Clean, Init>) -> Result<()> {
        let page_no = page.get_page_no();
        let page_info = DirPageInfo {
            owner: ino,
            page_no,
        };
        if let Some(node) = self.map.get_mut(&ino) {
            node.try_push(page_info)?;
        } else {
            let mut vec = Vec::new();
            vec.try_push(page_info)?;
            self.map.try_insert(ino, vec)?;
        }
        Ok(())
    }

    fn lookup_ino(&self, ino: &InodeNum) -> Option<&Vec<DirPageInfo>> {
        self.map.get(&ino)
    }

    fn delete(&mut self, _ino: InodeNum, _page: DirPageInfo) -> Result<()> {
        unimplemented!();
    }
}
