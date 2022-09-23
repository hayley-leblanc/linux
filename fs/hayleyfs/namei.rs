use crate::super_def::*;
use core::ffi;
use kernel::prelude::*;
use kernel::{fs, inode};

/// TODO: what should this be implemented for? wedsonaf/fs example has file operations
/// implemented for a separate FsFile type.
#[vtable]
impl inode::Operations<HayleyFS> for HayleyFS {
    fn create(
        _sb: &fs::SuperBlock<HayleyFS>,
        _dir: &fs::INode<HayleyFS>,
        _file_name: &CStr,
    ) -> Result<(ffi::c_ulong, INodeData)> {
        Err(ENOSYS)
    }
}
