// SPDX-License-Identifier: GPL-2.0

//! Inode operations
//!
//! C headers: [`include/linux/fs.h`](../../../../include/linux/fs.h) and
//! [`include/linux/file.h`](../../../../include/linux/file.h)

// TODO: move INode wrapper and related structures to this file

use crate::error::{from_kernel_result, Result};
use crate::fs::{DEntry, INode, Type};
// use crate::types::PointerWrapper;
use core::{marker, ptr};
use macros::vtable;

pub(crate) struct OperationsVtable<T: Type + ?Sized, O>(
    marker::PhantomData<T>,
    marker::PhantomData<O>,
);

/// Corresponds to the kernel's `struct inode_operations`.
#[vtable]
pub trait Operations<T: Type + ?Sized> {
    /// Creates a new inode
    fn create() -> Result<()>; // TODO: args and return values
    /// TODO: doc
    fn lookup(dir: &INode<T>, dentry: &DEntry<T>, flags: u32) -> Result<DEntry<T>>;
}

impl<T: Type + ?Sized, O: Operations<T>> OperationsVtable<T, O> {
    /// Called by the VFS when an inode should be created.
    /// right?
    unsafe extern "C" fn create_callback(
        _user_ns: *mut bindings::user_namespace,
        _inode: *mut bindings::inode,
        _dentry: *mut bindings::dentry,
        _mode: bindings::umode_t,
        _excl: bool,
    ) -> core::ffi::c_int {
        from_kernel_result! {
            crate::pr_info!("create inode callback");
            O::create().unwrap();
            Ok(0)
        }
    }

    /// called by VFS to perform a lookup
    unsafe extern "C" fn lookup_callback(
        dir: *mut bindings::inode,
        dentry: *mut bindings::dentry,
        flags: u32,
    ) -> *mut bindings::dentry {
        crate::pr_info!("lookup callback");
        let dir = unsafe { INode::from_ptr(dir) };
        let dentry = unsafe { DEntry::from_ptr(dentry) };
        let result = O::lookup(dir, &dentry, flags);
        if let Ok(dentry) = result {
            unsafe { dentry.to_ptr() }
        } else {
            ptr::null_mut()
        }
    }

    const VTABLE: bindings::inode_operations = bindings::inode_operations {
        lookup: Some(Self::lookup_callback),
        get_link: None,
        permission: None,
        get_acl: None,
        readlink: None,
        create: Some(Self::create_callback),
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
    pub(crate) const unsafe fn build() -> &'static bindings::inode_operations {
        &Self::VTABLE
    }
}
