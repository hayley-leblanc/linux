#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

use crate::def::*;
use crate::dir::hayleyfs_dir::*;
use crate::file::hayleyfs_file::*;
use crate::h_inode::hayleyfs_inode::*;
use crate::super_def::hayleyfs_bitmap::*;

pub(crate) struct RmdirFinalizeToken;
pub(crate) struct WriteFinalizeToken;

impl<'a> RmdirFinalizeToken {
    pub(crate) fn new(
        _parent_inode: InodeWrapper<'a, Clean, Link, Dir>,
        _parent_dentry: DentryWrapper<'a, Clean, Zero>,
        _child_self_dentry: DentryWrapper<'a, Clean, Zero>,
        _child_parent_dentry: DentryWrapper<'a, Clean, Zero>,
        _child_inode: InodeWrapper<'a, Clean, Zero, Dir>,
        _inode_bitmap: BitmapWrapper<'a, Clean, Zero, Inode>,
        _data_bitmap: BitmapWrapper<'a, Clean, Zero, Data>,
    ) -> Self {
        Self {}
    }
}

impl<'a> WriteFinalizeToken {
    pub(crate) fn new(
        _inode: InodeWrapper<'a, Clean, Size, Data>,
        _page: DataPageWrapper<'a, Clean, WriteData>,
    ) -> Self {
        Self {}
    }
}
