#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

use crate::def::*;
use crate::dir::hayleyfs_dir::*;
use crate::file::hayleyfs_file::*;
use crate::h_inode::hayleyfs_inode::*;
use crate::super_def::hayleyfs_bitmap::*;
use kernel::prelude::*;

pub(crate) struct RmdirFinalizeToken;
pub(crate) struct WriteFinalizeToken;
pub(crate) struct UnlinkFinalizeToken;

impl<'a> RmdirFinalizeToken {
    pub(crate) fn new(
        _parent_inode: InodeWrapper<'a, Clean, Link, Dir>,
        _parent_dentry: DentryWrapper<'a, Clean, Zero>,
        _child_dir_page: DataPageWrapper<'a, Clean, Zero>,
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
        _page: Vec<DataPageWrapper<'a, Clean, WriteData>>,
    ) -> Self {
        Self {}
    }
}

impl<'a> UnlinkFinalizeToken {
    pub(crate) fn new(
        _parent_inode: InodeWrapper<'a, Clean, Read, Dir>,
        _deleted_inode: InodeWrapper<'a, Clean, Zero, Data>,
        _deleted_page: Vec<DataPageWrapper<'a, Clean, Zero>>,
        _inode_bitmap: BitmapWrapper<'a, Clean, Zero, Inode>,
        _data_bitmap: BitmapWrapper<'a, Clean, Zero, Data>,
    ) -> Self {
        Self {}
    }
}
