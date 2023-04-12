use crate::balloc::*;
use crate::defs::*;
use crate::h_inode::*;
use crate::typestate::*;
use crate::volatile::*;
use crate::{end_timing, init_timing, start_timing};
use core::{marker::Sync, ptr, sync::atomic::Ordering};
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

        Ok(bytes_written.try_into()?)
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

        let result = hayleyfs_read(sbi, inode, writer, offset)?;
        Ok(result.try_into()?)
    }

    fn seek(_data: (), f: &file::File, offset: file::SeekFrom) -> Result<u64> {
        let (offset, whence) = match offset {
            file::SeekFrom::Start(off) => (off.try_into()?, bindings::SEEK_SET),
            file::SeekFrom::End(off) => (off, bindings::SEEK_END),
            file::SeekFrom::Current(off) => (off, bindings::SEEK_CUR),
        };
        let result =
            unsafe { bindings::generic_file_llseek(f.get_inner(), offset, whence.try_into()?) };
        if result < 0 {
            Err(error::Error::from_kernel_errno(result.try_into()?))
        } else {
            Ok(result.try_into()?)
        }
    }

    fn ioctl(data: (), file: &file::File, cmd: &mut file::IoctlCommand) -> Result<i32> {
        cmd.dispatch::<Self>(data, file)
    }
}

fn hayleyfs_write<'a>(
    sbi: &'a SbInfo,
    inode: RwSemaphore<&mut fs::INode>,
    reader: &mut impl IoBufferReader,
    offset: u64,
) -> Result<(u64, InodeWrapper<'a, Clean, IncSize, RegInode>)> {
    // TODO: give a way out if reader.len() is 0
    let len: u64 = reader.len().try_into()?;
    let count = if HAYLEYFS_PAGESIZE < len {
        HAYLEYFS_PAGESIZE
    } else {
        len
    };
    let mut inode = inode.write();
    let (pi, pi_info) = sbi.get_init_reg_inode_by_vfs_inode(inode.get_inner())?;

    // TODO: update timestamp

    // let offset: usize = offset.try_into()?;

    // this is the value of the `offset` field of the page that
    // we want to write to
    let page_offset = page_offset(offset)?;

    // does this page exist yet? if not, allocate it
    let result = pi_info.find(page_offset);
    let data_page = if let Some(page_info) = result {
        DataPageWrapper::from_data_page_info(sbi, &page_info)?
    } else {
        let page = DataPageWrapper::alloc_data_page(sbi, offset)?
            .flush()
            .fence();
        sbi.inc_blocks_in_use();
        let page = page.set_data_page_backpointer(&pi).flush().fence();
        // add page to the index
        // this is safe to do here because we hold a lock on this inode
        pi_info.insert(&page)?;
        page
    };
    let offset_in_page = offset - page_offset;
    let bytes_after_offset = HAYLEYFS_PAGESIZE - offset_in_page;
    // either write the rest of the count or write to the end of the page
    let to_write = if count < bytes_after_offset {
        count
    } else {
        bytes_after_offset
    };

    let (bytes_written, data_page) =
        data_page.write_to_page(sbi, reader, offset_in_page, to_write)?;

    let data_page = data_page.fence();

    if bytes_written < to_write {
        pr_info!(
            "WARNING: wrote {:?} out of {:?} bytes\n",
            bytes_written,
            to_write
        );
    }

    let (inode_size, pi) = pi.inc_size(bytes_written.try_into()?, data_page);

    // update the VFS inode's size
    inode.i_size_write(inode_size.try_into()?);

    Ok((bytes_written, pi))
}

fn hayleyfs_read(
    sbi: &SbInfo,
    inode: RwSemaphore<&mut fs::INode>,
    writer: &mut impl IoBufferWriter,
    mut offset: u64,
) -> Result<u64> {
    init_timing!(full_read);
    start_timing!(full_read);
    let mut count: u64 = writer.len().try_into()?;
    // TODO: update timestamp

    // acquire shared read lock
    let inode = inode.read();
    let (_, pi_info) = sbi.get_init_reg_inode_by_vfs_inode(inode.get_inner())?;
    let size: u64 = inode.i_size_read().try_into()?;

    count = if count < size { count } else { size };
    if offset >= size {
        return Ok(0);
    }
    let bytes_left_in_file = size - offset; // # of bytes that can be read

    let mut bytes_read = 0;

    while count > 0 {
        init_timing!(read_loop);
        start_timing!(read_loop);
        let page_offset = page_offset(offset)?;

        let offset_in_page = offset - page_offset;
        let bytes_after_offset = if bytes_left_in_file < HAYLEYFS_PAGESIZE {
            bytes_left_in_file - offset_in_page
        } else {
            HAYLEYFS_PAGESIZE - offset_in_page
        };
        // either read the rest of the count or write to the end of the page
        let to_read = if count < bytes_after_offset {
            count
        } else {
            bytes_after_offset
        };
        if to_read == 0 {
            break;
        }
        init_timing!(page_lookup);
        start_timing!(page_lookup);
        // if the page exists, read from it. Otherwise, return zeroes
        let result = pi_info.find(page_offset.try_into()?);
        end_timing!(LookupDataPage, page_lookup);
        if let Some(page_info) = result {
            let data_page = DataPageWrapper::from_data_page_info(sbi, &page_info)?;
            init_timing!(read_page);
            start_timing!(read_page);
            let read = data_page.read_from_page(sbi, writer, offset_in_page, to_read)?;
            end_timing!(ReadDataPage, read_page);
            bytes_read += read;
            offset += read;
            count -= read;
        } else {
            init_timing!(read_page);
            start_timing!(read_page);
            writer.clear(to_read.try_into()?)?;
            end_timing!(ReadDataPage, read_page);
            bytes_read += to_read;
            offset += to_read;
            count -= to_read;
        }
        end_timing!(ReadLoop, read_loop);
    }
    end_timing!(FullRead, full_read);
    Ok(bytes_read)
}
