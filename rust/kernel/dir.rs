//! Inodes
//!
//! C headers: [`include/linux/fs.h`](../../../../include/linux/fs.h) and
//! [`include/linux/file.h`](../../../../include/linux/file.h)

use crate::{
    error::{code::*, from_kernel_result, Result},
    file::{File, IoctlCommand},
    types::ForeignOwnable,
};
use core::{marker, ptr};
use macros::vtable;

/// Vtable for dir operations
/// TODO: should this be pub(crate) and only accessible via some other
/// function/module like file::OperationsVtable?
pub struct OperationsVtable<T: Operations>(marker::PhantomData<T>);

#[allow(dead_code)]
impl<T: Operations> OperationsVtable<T> {
    unsafe extern "C" fn unlocked_ioctl_callback(
        file: *mut bindings::file,
        cmd: core::ffi::c_uint,
        arg: core::ffi::c_ulong,
    ) -> core::ffi::c_long {
        from_kernel_result! {
            // SAFETY: `private_data` was initialised by `open_callback` with a value returned by
            // `T::Data::into_foreign`. `T::Data::from_foreign` is only called by the
            // `release` callback, which the C API guarantees that will be called only when all
            // references to `file` have been released, so we know it can't be called while this
            // function is running.
            let f = unsafe { T::Data::borrow((*file).private_data) };
            let mut cmd = IoctlCommand::new(cmd as _, arg as _);
            let ret = T::ioctl(f, unsafe { File::from_ptr(file) }, &mut cmd)?;
            Ok(ret as _)
        }
    }

    const VTABLE: bindings::file_operations = bindings::file_operations {
        open: None,
        release: None,
        read: None,
        write: None,
        llseek: None,
        check_flags: None,
        compat_ioctl: None,
        copy_file_range: None,
        fallocate: None,
        fadvise: None,
        fasync: None,
        flock: None,
        flush: None,
        fsync: None,
        get_unmapped_area: None,
        iterate: None,
        iterate_shared: None,
        iopoll: None,
        lock: None,
        mmap: None,
        mmap_supported_flags: 0,
        owner: ptr::null_mut(),
        poll: None,
        read_iter: None,
        remap_file_range: None,
        sendpage: None,
        setlease: None,
        show_fdinfo: None,
        splice_read: None,
        splice_write: None,
        unlocked_ioctl: if T::HAS_IOCTL {
            Some(Self::unlocked_ioctl_callback)
        } else {
            None
        },
        uring_cmd: None,
        uring_cmd_iopoll: None,
        write_iter: None,
    };

    /// Builds an instance of [`struct file_operations`].
    ///
    /// # Safety
    /// TODO: safety
    pub const unsafe fn build() -> &'static bindings::file_operations {
        &Self::VTABLE
    }
}

/// Corresponds to the kernel's `struct file_operations`. Specifically applied to
/// directory inodes.
#[vtable]
pub trait Operations {
    /// The type of the context data returned by [`Operations::open`] and made available to
    /// other methods.
    type Data: ForeignOwnable + Send + Sync = ();

    /// The type of the context data passed to [`Operations::open`].
    type OpenData: Sync = ();

    /// Performs IO control operations that are specific to the file.
    ///
    /// Corresponds to the `unlocked_ioctl` function pointer in `struct file_operations`.
    fn ioctl(
        _data: <Self::Data as ForeignOwnable>::Borrowed<'_>,
        _file: &File,
        _cmd: &mut IoctlCommand,
    ) -> Result<i32> {
        Err(ENOTTY)
    }
}
