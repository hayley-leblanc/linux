// SPDX-License-Identifier: GPL-2.0

//! Inode operations
//!
//! C headers: [`include/linux/fs.h`](../../../../include/linux/fs.h) and
//! [`include/linux/file.h`](../../../../include/linux/file.h)

// TODO: move INode wrapper and related structures to this file

use crate::error::{code::*, from_kernel_result, Result};
use crate::fs::{DEntry, INode, INodeParams, SuperBlock, Type};
use crate::str::CStr;
// use crate::types::PointerWrapper;
use core::marker;
use macros::vtable;

pub(crate) struct OperationsVtable<T: Type + ?Sized, O>(
    marker::PhantomData<T>,
    marker::PhantomData<O>,
);

/// Corresponds to the kernel's `struct inode_operations`.
#[vtable]
pub trait Operations<T: Type + ?Sized> {
    /// Creates a new inode. Returns the assigned inode number and INodeData
    fn create(
        _sb: &SuperBlock<T>,
        _dir: &INode<T>,
        _file_name: &CStr,
    ) -> Result<(core::ffi::c_ulong, T::INodeData)> {
        Err(ENOSYS)
    }
    /// Looks up a directory entry
    fn lookup(
        _sb: &SuperBlock<T>,
        _dir: &INode<T>,
        _dentry: &DEntry<T>,
        _flags: u32,
    ) -> Result<DEntry<T>> {
        Err(ENOSYS)
    }
}

impl<T: Type + ?Sized, O: Operations<T>> OperationsVtable<T, O> {
    /// Called by VFS to create an inode
    unsafe extern "C" fn create_callback(
        _user_ns: *mut bindings::user_namespace,
        dir: *mut bindings::inode,
        dentry: *mut bindings::dentry,
        mode: bindings::umode_t,
        _excl: bool,
    ) -> core::ffi::c_int {
        from_kernel_result! {
            let sb: &SuperBlock<T> = unsafe { SuperBlock::from_ptr((*dir).i_sb) };
            let dir: &INode<T> = unsafe { INode::from_ptr(dir) };
            let file_name = unsafe { CStr::from_char_ptr((*dentry).d_name.name as *const core::ffi::c_char) };
            let (ino, data) = O::create(sb, dir, file_name)?;
            let _inode_params = INodeParams {
                mode,
                ino,
                value: data,
            };

            // TODO: finish VFS inode setup

            Ok(ino.try_into()?)
        }
    }

    /// Called by VFS to look up a dentry
    unsafe extern "C" fn lookup_callback(
        dir: *mut bindings::inode,
        dentry: *mut bindings::dentry,
        flags: core::ffi::c_uint,
    ) -> *mut bindings::dentry {
        let sb: &SuperBlock<T> = unsafe { SuperBlock::from_ptr((*dir).i_sb) };
        let dir: &INode<T> = unsafe { INode::from_ptr(dir) };
        let dentry: &DEntry<T> = unsafe { DEntry::from_ptr(dentry) };
        let result = O::lookup(sb, dir, dentry, flags);

        unsafe {
            match result {
                Err(e) => bindings::ERR_PTR(from_kernel_result!(Err(e))) as *mut bindings::dentry,
                Ok(dentry) => dentry.to_ptr(),
            }
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
