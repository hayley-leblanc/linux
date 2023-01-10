// SPDX-License-Identifier: GPL-2.0

//! Inodes
//!
//! C headers: [`include/linux/fs.h`](../../../../include/linux/fs.h) and
//! [`include/linux/file.h`](../../../../include/linux/file.h)

use crate::{
    bindings,
    error::Result,
    fs::{DEntry, INode},
};
use core::marker;
use macros::vtable;

/// Vtable for inode operations
/// TODO: should this be pub(crate) and only accessible via some other
/// function/module like file::OperationsVtable?
pub struct OperationsVtable<T: Operations>(marker::PhantomData<T>);

#[allow(dead_code)]
impl<T: Operations> OperationsVtable<T> {
    unsafe extern "C" fn lookup_callback(
        dir: *mut bindings::inode,
        dentry: *mut bindings::dentry,
        flags: core::ffi::c_uint,
    ) -> *mut bindings::dentry {
        // TODO: safety notes
        let dir_arg = unsafe { &*dir.cast() };
        let dentry_arg = unsafe { &*dentry.cast() };
        let flags_arg = flags as u32;
        let result = T::lookup(dir_arg, dentry_arg, flags_arg);
        match result {
            Err(e) => unsafe {
                bindings::ERR_PTR(e.to_kernel_errno().into()) as *mut bindings::dentry
            },
            Ok(ptr) => {
                let ptr: *mut bindings::dentry = ptr.0.get();
                ptr
            }
        }
    }

    const VTABLE: bindings::inode_operations = bindings::inode_operations {
        lookup: Some(Self::lookup_callback),
        get_link: None,
        permission: None,
        get_acl: None,
        readlink: None,
        create: None,
        link: None,
        unlink: None,
        symlink: None,
        mkdir: None,
        rmdir: None,
        mknod: None,
        rename: None,
        setattr: None,
        getattr: None,
        listxattr: None,
        fiemap: None,
        update_time: None,
        atomic_open: None,
        tmpfile: None,
        set_acl: None,
        fileattr_set: None,
        fileattr_get: None,
    };

    /// Builds an instance of [`struct inode_operations`].
    ///
    /// # Safety
    /// TODO: safety
    pub const unsafe fn build() -> &'static bindings::inode_operations {
        &Self::VTABLE
    }
}

/// Corresponds to the kernel's `struct inode_operations`.
///
/// You implement this trait whenver you would create a `struct inode_operations`.
///
/// TODO: safety notes
/// TODO: context data as in file.rs? What is that? Do we need it?
#[vtable]
pub trait Operations {
    /// Corresponds to the `lookup` function pointer in `struct inode_operations`.
    fn lookup(dir: &INode, dentry: &DEntry, flags: u32) -> Result<DEntry>;
}
