#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

use crate::def::*;
use crate::dir::hayleyfs_dir::*;
use crate::inode_def::hayleyfs_inode::*;
use crate::super_def::hayleyfs_bitmap::*;

pub(crate) struct RmdirFinalizeToken;

impl<'a> RmdirFinalizeToken {
    pub(crate) fn new(
        _: InodeWrapper<'a, Clean, Link, Dir>,    // parent inode
        _: DentryWrapper<'a, Clean, Zero>,        // parent dentry
        _: DentryWrapper<'a, Clean, Zero>,        // child . dentry
        _: DentryWrapper<'a, Clean, Zero>,        // child .. dentry
        _: InodeWrapper<'a, Clean, Zero, Dir>,    // child inode
        _: BitmapWrapper<'a, Clean, Zero, Inode>, // inode bitmap
        _: BitmapWrapper<'a, Clean, Zero, Data>,  // data bitmap
    ) -> Self {
        Self {}
    }
}
