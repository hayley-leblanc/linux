use core::{marker::Sync, ptr};
use kernel::prelude::*;
use kernel::{bindings, error, file, fs, io_buffer::IoBufferReader, sync::RwSemaphore};

pub(crate) struct Adapter {}

impl<T: Sync> file::OpenAdapter<T> for Adapter {
    unsafe fn convert(_inode: *mut bindings::inode, _file: *mut bindings::file) -> *const T {
        ptr::null_mut()
    }
}

pub(crate) struct FileOps;
#[vtable]
impl file::Operations for FileOps {
    fn open(_context: &(), file: &file::File) -> Result<()> {
        let ret = unsafe { bindings::generic_file_open(file.inode(), file.get_inner()) };
        if ret < 0 {
            Err(error::Error::from_kernel_errno(ret))
        } else {
            Ok(())
        }
    }

    fn release(_data: (), _file: &file::File) {}

    fn write(
        _data: (),
        file: &file::File,
        reader: &mut impl IoBufferReader,
        offset: u64,
    ) -> Result<usize> {
        pr_info!("write\n");
        // TODO: cleaner way to set up the semaphore with Rust RwSemaphore
        let sem = unsafe { (*file.inode()).i_rwsem };
        let inode: &mut fs::INode = unsafe { &mut *file.inode().cast() };
        let inode = unsafe { RwSemaphore::new_with_sem(inode, sem) };
        hayleyfs_write(inode, reader, offset)
    }
}

fn hayleyfs_write(
    inode: RwSemaphore<&mut fs::INode>,
    reader: &mut impl IoBufferReader,
    offset: u64,
) -> Result<usize> {
    pr_info!("writing {:?} bytes to offset {:?}\n", reader.len(), offset);
    let _inode = inode.write();
    Err(EINVAL)
}
