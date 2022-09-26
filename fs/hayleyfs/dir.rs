use crate::super_def::*;
use kernel::prelude::*;
use kernel::rbtree::RBTree;
use kernel::sync::smutex::Mutex;

#[derive(PartialEq)]
#[repr(C)]
pub(crate) struct Dentry {
    filename: String,
    ino: InodeNum,
}

pub(crate) trait DirectoryIndex {
    fn insert_dentry(&mut self, parent: InodeNum, dentry: Dentry) -> Result<()>;
    fn insert_directory(&mut self, dir: InodeNum) -> Result<()>;
    fn remove_dentry(&mut self, parent: InodeNum, child: String) -> Result<()>;
    fn remove_directory(&mut self, dir: InodeNum) -> Result<()>;
    fn lookup(&self, parent: InodeNum, child: String) -> Option<InodeNum>;
}

/// simple directory entry tree structure where we use kernel's red black tree
/// to map directory inode numbers to vectors of their dentries.
pub(crate) struct RBVecDirTree(Mutex<RBTree<InodeNum, Vec<Dentry>>>);

impl RBVecDirTree {
    pub(crate) fn new() -> Self {
        Self(Mutex::new(RBTree::new()))
    }
}

// TODO: implement DirectoryTree for RBDirTree based on the kernel's RBTree
impl DirectoryIndex for RBVecDirTree {
    fn insert_dentry(&mut self, parent: InodeNum, dentry: Dentry) -> Result<()> {
        let mut tree = self.0.lock();
        let node = tree.get_mut(&parent);
        if node.is_none() {
            return Err(ENOENT);
        } else if let Some(dentries) = node {
            if dentries.contains(&dentry) {
                return Err(EEXIST);
            }
            dentries.try_push(dentry)?;
        }
        Ok(())
    }

    fn insert_directory(&mut self, dir: InodeNum) -> Result<()> {
        let mut tree = self.0.lock();
        let vec = Vec::new();
        let result = tree.try_insert(dir, vec)?;
        if result.is_some() {
            Err(EEXIST)
        } else {
            Ok(())
        }
    }

    fn remove_dentry(&mut self, parent: InodeNum, child: String) -> Result<()> {
        let mut tree = self.0.lock();
        let node = tree.get_mut(&parent);
        if node.is_none() {
            return Err(ENOENT);
        } else if let Some(dentries) = node {
            // obtain the index of the dentry we want to remove
            let mut index = None;
            for (i, dentry) in dentries.iter().enumerate() {
                if dentry.filename == child {
                    index = Some(i);
                    break;
                }
            }
            if index.is_none() {
                return Err(ENOENT);
            } else if let Some(index) = index {
                // swap_remove gives us O(1) removal but does not preserve ordering
                dentries.swap_remove(index);
            }
        }
        Ok(())
    }

    fn remove_directory(&mut self, dir: InodeNum) -> Result<()> {
        // TODO: check that the directory is empty before removing it
        let mut tree = self.0.lock();
        let result = tree.remove(&dir);
        if result.is_none() {
            Err(ENOENT)
        } else {
            Ok(())
        }
    }

    fn lookup(&self, parent: InodeNum, child: String) -> Option<InodeNum> {
        let tree = self.0.lock();
        let node = tree.get(&parent);
        if let Some(dentries) = node {
            for dentry in dentries {
                if dentry.filename == child {
                    return Some(dentry.ino);
                }
            }
        }
        None
    }
}
