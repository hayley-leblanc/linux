use crate::defs::*;
use core::ffi;
use kernel::prelude::*;
use kernel::rbtree::RBTree;

#[allow(dead_code)]
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
    // fn look_up(ino: InodeNum) -> Result<()>;
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

    fn delete(&mut self, _ino: InodeNum, _dentry: DentryInfo) -> Result<()> {
        // TODO
        unimplemented!();
    }
}
