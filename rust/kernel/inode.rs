// SPDX-License-Identifier: GPL-2.0

//! Inodes
//!
//! C headers: [`include/linux/fs.h`](../../../../include/linux/fs.h) and
//! [`include/linux/file.h`](../../../../include/linux/file.h)

use crate::{
    bindings,
    error::from_kernel_result,
    error::Result,
    fs::{DEntry, INode, UserNamespace},
};
use core::{ffi, marker, ptr};
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
        let dentry_arg = unsafe { &mut *dentry.cast() };
        let flags = flags as u32;
        let result = T::lookup(dir_arg, dentry_arg, flags);
        match result {
            Err(e) => unsafe {
                bindings::ERR_PTR(e.to_kernel_errno().into()) as *mut bindings::dentry
            },
            Ok(inode) => {
                if let Some(_ino) = inode {
                    // TODO: iget and d_splice_alias
                    unimplemented!();
                } else {
                    unsafe { bindings::d_splice_alias(ptr::null_mut(), dentry) }
                }
            }
        }
    }

    unsafe extern "C" fn create_callback(
        mnt_userns: *mut bindings::user_namespace,
        dir: *mut bindings::inode,
        dentry: *mut bindings::dentry,
        umode: bindings::umode_t,
        excl: bool,
    ) -> ffi::c_int {
        // FIXME: error output is weird and incorrect in terminal
        from_kernel_result! {
            // TODO: safety notes
            let mnt_userns = unsafe { &*mnt_userns.cast()};
            let dir = unsafe { &*dir.cast() };
            let dentry = unsafe { &mut *dentry.cast()};
            T::create(mnt_userns, dir, dentry, umode, excl)
        }
    }

    unsafe extern "C" fn link_callback(
        old_dentry: *mut bindings::dentry,
        dir: *mut bindings::inode,
        dentry: *mut bindings::dentry,
    ) -> ffi::c_int {
        from_kernel_result! {
            let old_dentry = unsafe { &*old_dentry.cast() };
            let dir = unsafe { &mut *dir.cast() };
            let dentry = unsafe { &*dentry.cast() };
            T::link(old_dentry, dir, dentry)
        }
    }

    unsafe extern "C" fn mkdir_callback(
        mnt_userns: *mut bindings::user_namespace,
        dir: *mut bindings::inode,
        dentry: *mut bindings::dentry,
        umode: bindings::umode_t,
    ) -> ffi::c_int {
        from_kernel_result! {
            // TODO: safety notes
            let mnt_userns = unsafe { &*mnt_userns.cast()};
            let dir = unsafe { &mut *dir.cast() };
            let dentry = unsafe { &mut *dentry.cast()};
            T::mkdir(mnt_userns, dir, dentry, umode)
        }
    }

    const VTABLE: bindings::inode_operations = bindings::inode_operations {
        lookup: Some(Self::lookup_callback),
        get_link: None,
        permission: None,
        get_acl: None,
        readlink: None,
        create: Some(Self::create_callback),
        link: Some(Self::link_callback),
        unlink: None,
        symlink: None,
        mkdir: Some(Self::mkdir_callback),
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
    fn lookup(dir: &INode, dentry: &mut DEntry, flags: u32) -> Result<Option<core::ffi::c_ulong>>;
    /// Corresponds to the `create` function pointer in `struct inode_operations`.
    fn create(
        mnt_userns: &UserNamespace,
        dir: &INode,
        dentry: &DEntry,
        umode: bindings::umode_t,
        excl: bool,
    ) -> Result<i32>;
    /// Corresponds to the `mkdir` function pointer in `struct inode_operations`.
    fn mkdir(
        mnt_userns: &UserNamespace,
        dir: &mut INode,
        dentry: &DEntry,
        umode: bindings::umode_t,
    ) -> Result<i32>;
    /// Corresponds to the ``link` function pointer in `struct inode_operations`.
    fn link(old_dentry: &DEntry, dir: &mut INode, dentry: &DEntry) -> Result<i32>;
}
