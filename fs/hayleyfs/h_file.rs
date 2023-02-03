use crate::balloc::*;
use crate::defs::*;
use crate::h_inode::*;
use crate::typestate::*;
use crate::volatile::*;
use core::{marker::Sync, ptr};
use kernel::prelude::*;
use kernel::{
    bindings, error, file, fs,
    io_buffer::{IoBufferReader, IoBufferWriter},
    sync::RwSemaphore,
};

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
        let (bytes_written, _) = hayleyfs_write(sbi, inode, reader, offset)?;

        Ok(bytes_written)
    }

    fn read(
        _data: (),
        file: &file::File,
        writer: &mut impl IoBufferWriter,
        offset: u64,
    ) -> Result<usize> {
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

        hayleyfs_read(sbi, inode, writer, offset)
    }
}

fn hayleyfs_write<'a>(
    sbi: &SbInfo,
    inode: RwSemaphore<&mut fs::INode>,
    reader: &mut impl IoBufferReader,
    offset: u64,
) -> Result<(usize, InodeWrapper<'a, Clean, IncSize, RegInode>)> {
    // TODO: give a way out if reader.len() is 0
    let mut count = reader.len();
    pr_info!("writing {:?} bytes to offset {:?}\n", count, offset);
    let mut inode = inode.write();
    let ino = inode.i_ino();
    let pi = sbi.get_init_reg_inode_by_ino(ino)?;

    // TODO: update timestamp

    let bytes_per_page = bytes_per_page();
    let mut written = 0;
    while count > 0 {
        // this is the value of the `offset` field of the page that
        // we want to write to
        let page_offset = page_offset(offset)?;

        // does this page exist yet? if not, allocate it
        let result = sbi.ino_data_page_map.find(&ino, page_offset.try_into()?);
        let data_page = if let Some(page_info) = result {
            DataPageWrapper::from_data_page_info(sbi, page_info)?
        } else {
            pr_info!("Page does not exist\n");
            DataPageWrapper::alloc_data_page(sbi, offset)?
                .flush()
                .fence()
        };
        let offset_in_page = data_page.get_offset() - offset;
        let offset: usize = offset.try_into()?;
        let bytes_after_offset = bytes_per_page - offset;
        // either write the rest of the count or write to the end of the page
        let to_write = if count < bytes_after_offset {
            count
        } else {
            bytes_after_offset
        };

        let (bytes_written, data_page) =
            data_page.write_to_page(reader, offset_in_page, to_write)?;
        let data_page = data_page.fence();

        let data_page = data_page.set_data_page_backpointer(&pi);

        // add page to the index
        sbi.ino_data_page_map.insert(ino, &data_page)?;

        if bytes_written < to_write {
            pr_info!(
                "WARNING: wrote {:?} out of {:?} bytes\n",
                bytes_written,
                to_write
            );
            break;
        }

        count -= bytes_written;
        written += bytes_written;
    }

    let written_u64: u64 = written.try_into()?;
    let pos = offset + written_u64;
    let pi = pi.inc_size(pos);

    // update the VFS inode's size
    inode.i_size_write(pos.try_into()?);

    Ok((written, pi))
}

fn hayleyfs_read(
    sbi: &SbInfo,
    inode: RwSemaphore<&mut fs::INode>,
    writer: &mut impl IoBufferWriter,
    offset: u64,
) -> Result<usize> {
    let count = writer.len();
    pr_info!("reading {:?} bytes at offset {:?}\n", count, offset);

    // TODO: update timestamp

    // acquire shared read lock
    let inode = inode.read();
    let _size = inode.i_size_read();
    let ino = inode.i_ino();
    let mut count = writer.len();

    let _bytes_per_page = bytes_per_page();
    // let mut read = 0;
    while count > 0 {
        let page_offset = page_offset(offset)?;
        pr_info!("offset {:?}\n", page_offset);

        // if the page exists, read from it. Otherwise, return zeroes
        let result = sbi.ino_data_page_map.find(&ino, page_offset.try_into()?);
        if let Some(page_info) = result {
            pr_info!("Page exists {:?}\n", page_info);
        } else {
            pr_info!("page does not exist\n");
        }

        count = 0;
    }

    Err(EINVAL)
}
