use crate::super_def::*;
use kernel::rbtree::RBTree;

#[repr(C)]
pub(crate) struct Dentry {
    filename: String,
    ino: InodeNum,
}

pub(crate) trait DirectoryTree {
    fn insert(&mut self, dentry: Dentry) -> Result<()>;
    fn remove_dentry(&mut self, parent: InodeNum, child: String) -> Result<()>;
    fn remove_directory(&mut self, dir: InodeNum) -> Result<()>;
    fn lookup(&self, parent: InodeNum, child: String) -> Result<Dentry>;
}

pub(crate) struct RBDirTree(RBTree<InodeNum, Vec<Dentry>>);

// TODO: implement DirectoryTree for RBDirTree based on the kernel's RBTree
