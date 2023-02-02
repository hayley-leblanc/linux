use crate::balloc::*;
use crate::defs::*;
use crate::volatile::*;
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
        let sb = inode.i_sb();
        // TODO: safety
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };
        let inode = unsafe { RwSemaphore::new_with_sem(inode, sem) };
        hayleyfs_write(sbi, inode, reader, offset)
    }
}

fn hayleyfs_write(
    sbi: &SbInfo,
    inode: RwSemaphore<&mut fs::INode>,
    reader: &mut impl IoBufferReader,
    offset: u64,
) -> Result<usize> {
    let count = reader.len();
    if count == 0 {
        return Ok(0);
    }
    pr_info!("writing {:?} bytes to offset {:?}\n", count, offset);
    let inode = inode.write();
    let ino = inode.i_ino();

    pr_info!("bytes per page: {:?}\n", bytes_per_page());

    // TODO: update timestamp

    let mut bytes_to_write = count;
    while bytes_to_write > 0 {
        // this is the value of the `offset` field of the page that
        // we want to write to
        let page_offset = page_offset(offset);
        pr_info!("offset {:?}\n", page_offset);

        // does this page exist yet? if not, allocate it
        let result = sbi.ino_data_page_map.find(&ino, offset);
        let _data_page = if let Some(page_info) = result {
            pr_info!("page info {:?}\n", page_info);
            DataPageWrapper::from_data_page_info(sbi, page_info)?
        } else {
            pr_info!("Page does not exist\n");
            DataPageWrapper::alloc_data_page(sbi, offset)?
                .flush()
                .fence()
        };

        bytes_to_write = 0;
    }

    Err(EINVAL)
}
