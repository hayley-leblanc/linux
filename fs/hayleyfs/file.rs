#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

use crate::def::*;
use crate::dir::hayleyfs_dir::*;
use crate::finalize::*;
use crate::h_inode::hayleyfs_inode::*;
use crate::h_inode::*;
use crate::pm::*;
use crate::super_def::hayleyfs_bitmap::*;
use crate::super_def::*;
use core::marker::PhantomData;
use core::ptr;
use kernel::bindings::{address_space, file, file_operations, generic_file_open, inode, O_APPEND};
use kernel::c_types::{c_int, c_void};
use kernel::prelude::*;
use kernel::{c_default_struct, PAGE_SIZE};

#[no_mangle]
pub(crate) static mut HayleyfsFileOps: file_operations = file_operations {
    write: Some(hayleyfs_file::hayleyfs_file_write),
    read: Some(hayleyfs_file::hayleyfs_file_read),
    open: Some(hayleyfs_file::hayleyfs_open),
    ..c_default_struct!(file_operations)
};

pub(crate) mod hayleyfs_file {
    use super::*;

    // generic page structure that can be used to represent any page
    // without a known structure
    struct DataPage {
        data: [i8; PAGE_SIZE],
    }

    pub(crate) struct DataPageWrapper<'a, State, Op> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        page_no: PmPage,
        data_page: &'a mut DataPage,
    }

    pub(crate) trait EmptyFilePage {
        fn get_page_no(&self) -> Option<PmPage>;
    }

    impl<'a> EmptyFilePage for DataPageWrapper<'a, Clean, Zero> {
        fn get_page_no(&self) -> Option<PmPage> {
            Some(self.page_no)
        }
    }

    impl EmptyFilePage for EmptyPage {
        fn get_page_no(&self) -> Option<PmPage> {
            None
        }
    }

    impl<'a, State, Op> PmObjWrapper for DataPageWrapper<'a, State, Op> {}

    impl<'a, State, Op> PmObjWrapper for Vec<DataPageWrapper<'a, State, Op>> {}

    impl<'a, State, Op> DataPageWrapper<'a, State, Op> {
        fn new(page_no: PmPage, data_page: &'a mut DataPage) -> Self {
            Self {
                state: PhantomData,
                op: PhantomData,
                page_no,
                data_page,
            }
        }

        pub(crate) fn get_page_no(&self) -> PmPage {
            self.page_no
        }
    }

    impl<'a> DataPageWrapper<'a, Clean, Read> {
        pub(crate) fn read_data_page(sbi: &SbInfo, page_no: PmPage) -> Result<Self> {
            check_page_no(sbi, page_no)?;
            let addr = (sbi.virt_addr as usize) + (PAGE_SIZE * page_no);
            Ok(DataPageWrapper::new(page_no, unsafe {
                &mut *(addr as *mut DataPage)
            }))
        }

        pub(crate) fn zero_page(self) -> DataPageWrapper<'a, Flushed, Zero> {
            // unsafely zero the memory region associated with this page
            // TODO: do this with nontemporal stores rather than cache line flushes
            // TODO: make sure this is zeroing the right amount of space
            unsafe { ptr::write_bytes(&mut self.data_page.data, 0, 1) };
            clwb(&self.data_page.data, PAGE_SIZE, false);
            DataPageWrapper::new(self.page_no, self.data_page)
        }

        // pub(crate) fn write_data(
        //     self,
        //     buf: *const i8,
        //     len: usize,
        //     offset: usize,
        // ) -> (DataPageWrapper<'a, Flushed, WriteData>, usize) {
        //     // TODO: non-temporal stores
        //     // figure out how many bytes to write
        //     let bytes_to_write = if PAGE_SIZE - offset < len {
        //         PAGE_SIZE - offset
        //     } else {
        //         len
        //     };
        //     // TODO: do you end up writing to the correct place with these type conversions?
        //     // TODO: what does the return value here actually mean?????
        //     let data_ptr = self.data_page.data.as_ptr() as *mut i8;
        //     let bytes_written = bytes_to_write
        //         - unsafe {
        //             hayleyfs_copy_from_user_nt(
        //                 data_ptr.offset(offset.try_into().unwrap()) as *const c_void, // TODO: handle error properly
        //                 buf as *const c_void,
        //                 bytes_to_write.try_into().unwrap(), // TODO: handle error properly
        //             ) as usize
        //         };
        //     // TODO: MUST FLUSH FIRST AND LAST CACHE LINES (or check if they need to be flushed)
        //     clwb(
        //         unsafe { data_ptr.offset(offset.try_into().unwrap()) },
        //         bytes_to_write,
        //         false,
        //     );
        //     (
        //         DataPageWrapper::new(self.page_no, self.data_page),
        //         bytes_written.try_into().unwrap(), // TODO: handle error properly
        //     )
        // }

        // pub(crate) fn read_data(&self, buf: *mut i8, len: usize, offset: usize) -> usize {
        //     let data_ptr = self.data_page.data.as_ptr() as *mut i8;
        //     let bytes_copied = len
        //         - unsafe {
        //             hayleyfs_copy_to_user(
        //                 buf as *mut c_void,
        //                 data_ptr.offset(offset.try_into().unwrap()) as *const c_void, // TODO: include offset
        //                 len.try_into().unwrap(), // TODO: handle error properly
        //             ) as usize
        //         };
        //     bytes_copied.try_into().unwrap() // TODO: handle error properly
        // }
    }

    impl<'a> DataPageWrapper<'a, Clean, Zero> {}

    impl<'a, Op> DataPageWrapper<'a, Flushed, Op> {
        pub(crate) unsafe fn fence_unsafe(self) -> DataPageWrapper<'a, Clean, Op> {
            DataPageWrapper::new(self.page_no, self.data_page)
        }

        pub(crate) fn fence(self) -> DataPageWrapper<'a, Clean, Op> {
            sfence();
            DataPageWrapper::new(self.page_no, self.data_page)
        }
    }

    impl<'a, Op> DataPageWrapper<'a, Clean, Op> {
        // TODO: should this be unsafe? it feels very sketchy since we don't have dir
        // page wrappers, although we shouldn't be able to modify the returned dir page.
        // need to be careful about concurrency with this, potentially?
        pub(crate) unsafe fn convert_to_dir_page(self, sbi: &SbInfo) -> &'a mut DirPage {
            // TODO: can we do this without reading from PM?
            // the page number was already checked when we created the dir page wrapper,
            // so we don't need to check it again
            // let dir_page = sbi.virt_addr as usize + (self.page_no * PAGE_SIZE)) as *mut c_void;
            let dir_page = unsafe {
                &mut *((sbi.virt_addr as usize + (self.page_no + PAGE_SIZE)) as *mut DirPage)
            };
            dir_page
        }

        /// the len and page_offset to this method are relative to the current page,
        /// not to the file as a whole
        /// managing the size of the file should be handled in the caller
        pub(crate) fn write_data(
            self,
            len: i64,
            page_offset: i64,
            buf: *const i8,
            buf_offset: i64,
        ) -> Result<(DataPageWrapper<'a, Clean, WriteData>, i64)> {
            let dst = unsafe {
                self.data_page.data.as_ptr().offset(page_offset.try_into()?) as *const c_void
            };
            let src = unsafe { buf.offset(buf_offset.try_into()?) as *const c_void };
            let res: i64 =
                unsafe { hayleyfs_copy_from_user_nt(dst, src, len.try_into()?).try_into()? };
            let written = len - res;
            Ok((DataPageWrapper::new(self.page_no, self.data_page), written))
        }

        pub(crate) fn read_data(
            self,
            len: i64,
            page_offset: i64,
            buf: *mut i8,
            buf_offset: i64,
        ) -> Result<i64> {
            let dst = unsafe { buf.offset(buf_offset.try_into()?) as *mut c_void };
            let src = unsafe {
                self.data_page.data.as_ptr().offset(page_offset.try_into()?) as *const c_void
            };
            let res: i64 = unsafe { hayleyfs_copy_to_user(dst, src, len.try_into()?).try_into()? };
            let written = len - res;
            Ok(written)
        }
    }

    #[no_mangle]
    pub(crate) unsafe extern "C" fn hayleyfs_file_write(
        filep_raw: *mut file,
        buf: *const i8,
        len: usize,
        pos_raw: *mut i64,
    ) -> isize {
        let filep = unsafe { &mut *(filep_raw as *mut file) };
        let ppos = unsafe { &mut *(pos_raw as *mut i64) };

        // TODO: locks

        let mapping = unsafe { &mut *(filep.f_mapping as *mut address_space) };
        let inode = unsafe { &mut *(mapping.host as *mut inode) };

        let result = _hayleyfs_file_write(filep, buf, len, ppos, inode);
        match result {
            Ok((_token, bytes_written)) => bytes_written,
            Err(e) => e.to_kernel_errno().try_into().unwrap(), // TODO: error handling
        }
    }

    /// right now this is not atomic. soft updates does not provide a mechanism
    /// for atomic data writes. you will have to use COW to do that if we want
    /// to add it.
    fn _hayleyfs_file_write(
        filep: &mut file,
        buf: *const i8,
        mut len: usize,
        ppos: &mut i64,
        inode: &mut inode,
    ) -> Result<(WriteFinalizeToken, isize)> {
        // make sure we can access the user buffer
        if !unsafe { hayleyfs_access_ok(buf, len) } == 0 {
            return Err(EFAULT);
        }

        let sb = inode.i_sb;
        let sbi = hayleyfs_get_sbi(sb);

        let mut pos = *ppos;

        if filep.f_flags & O_APPEND != 0 {
            pos = unsafe { hayleyfs_i_size_read(inode) };
        }

        // TODO: remove this when file size can be bigger
        if pos >= PAGE_SIZE.try_into()? {
            return Err(ENOSPC);
        }

        let ino: InodeNum = inode.i_ino.try_into().unwrap();

        pr_info!(
            "writing: inode {:?}, count {:?}, offset {:?}\n",
            ino,
            len,
            pos
        );

        let pi = InodeWrapper::read_file_inode(sbi, &ino);
        let has_pages = pi.has_data_page();
        let pi_size = pi.get_size();
        let num_blks: i64 = pi.get_num_blks().try_into()?;

        // TODO: get rid of all these, this is ridiculous
        let mut required_capacity: i64 = len as i64 + pos;
        let current_capacity: i64 = PAGE_SIZE as i64 * num_blks;
        let page_size_i64: i64 = PAGE_SIZE.try_into()?;
        let blks_per_inode_i64: i64 = DIRECT_PAGES_PER_INODE.try_into()?;
        let max_file_size_i64: i64 = MAX_FILE_SIZE.try_into()?;

        // manage number of bytes we can write before moving on too far.
        // make sure the number of bytes to write is capped at the max size of the file
        if required_capacity > max_file_size_i64 {
            required_capacity = max_file_size_i64;
            len = (max_file_size_i64 - pi_size).try_into()?;
        }

        // let len_i64: i64 = len.try_into()?;

        let pi_temp;
        let mut num_pages_to_alloc = 0;
        // check if we have to allocate new pages for the inode
        if required_capacity >= current_capacity || !has_pages {
            // figure out how many new pages need to be allocated
            if has_pages {
                // if we already have pages, but not enough, subtract out any spare space
                // in the last block from the required capacity
                required_capacity -= current_capacity - pi.get_size();
            }
            num_pages_to_alloc = (required_capacity / page_size_i64) + 1;

            // TODO: update this when we can have more pages per inode
            // rather than returning enospc, we just need to limit the number of
            // blocks that we allocate to the number we actually have space for in the file

            if num_pages_to_alloc + num_blks >= blks_per_inode_i64 {
                // return Err(ENOSPC);
                num_pages_to_alloc = blks_per_inode_i64 - num_blks;
            }

            // now allocate the required number of pages
            let allocated_page_nos = Vec::new();
            // TODO: finalize or don't return bits
            let (_bits, data_bitmap) =
                BitmapWrapper::read_data_bitmap(sbi).allocate_bits(num_pages_to_alloc)?;
            let data_bitmap = data_bitmap.fence();

            // for now, returns ENOSPC if we run out of direct pages
            // it's safe to add the pages now because we haven't updated the
            // file size yet - they aren't accessible yet
            pi_temp = pi.add_data_pages(allocated_page_nos, data_bitmap)?;
        } else {
            pi_temp = pi.coerce_to_addpage();
        }
        let pi = pi_temp;

        let len_i64: i64 = len.try_into()?;
        let (pages_written, bytes_written) = pi.write_data(sbi, len_i64, pos, buf)?;

        // now update size stuff

        pos += bytes_written;
        *ppos = pos;
        if pos > pi_size {
            inode.i_blocks += num_pages_to_alloc as u64; // TODO: Handle conversion better
            unsafe { hayleyfs_i_size_write(inode, pos) };
        }

        let pi = pi.set_size(pos, num_pages_to_alloc, &pages_written);

        let token = WriteFinalizeToken::new(pi, pages_written);
        pr_info!("bytes written: {:?}\n", bytes_written);
        Ok((token, bytes_written.try_into()?))
    }

    #[no_mangle]
    pub(crate) unsafe extern "C" fn hayleyfs_file_read(
        filep_raw: *mut file,
        buf: *mut i8,
        len: usize,
        ppos_raw: *mut i64,
    ) -> isize {
        let filep = unsafe { &mut *(filep_raw as *mut file) };
        let ppos = unsafe { &mut *(ppos_raw as *mut i64) };
        let mapping = unsafe { &mut *(filep.f_mapping as *mut address_space) };
        let inode = unsafe { &mut *(mapping.host as *mut inode) };

        let result = _hayleyfs_file_read(buf, len, ppos, inode);

        match result {
            Ok(bytes_read) => bytes_read,
            Err(e) => e.to_kernel_errno().try_into().unwrap(), // TODO: error handling
        }
    }

    #[no_mangle]
    pub(crate) fn _hayleyfs_file_read(
        buf: *mut i8,
        mut len: usize,
        ppos: &mut i64,
        inode: &mut inode,
    ) -> Result<isize> {
        // TODO: mark the file as accessed, update access time, etc.

        // make sure we can access the user buffer
        if !unsafe { hayleyfs_access_ok(buf, len) } == 0 {
            return Err(EFAULT);
        }

        let mut pos = *ppos;
        let file_size = unsafe { hayleyfs_i_size_read(inode) };
        if file_size == 0 {
            return Ok(0);
        }

        if len > (file_size - pos).try_into()? {
            len = (file_size - pos).try_into()?;
        }
        if len <= 0 {
            return Ok(0);
        }

        let ino: InodeNum = inode.i_ino.try_into()?;

        let sb = inode.i_sb;
        let sbi = hayleyfs_get_sbi(sb);

        let pi = InodeWrapper::read_file_inode(sbi, &ino);
        let len_i64: i64 = len.try_into()?;
        let bytes_read = pi.read_data(sbi, len_i64, pos, buf)?;
        pos += bytes_read;
        *ppos = pos;

        Ok(bytes_read.try_into()?)
    }

    #[no_mangle]
    pub(crate) unsafe extern "C" fn hayleyfs_open(inode: *mut inode, filep: *mut file) -> c_int {
        unsafe { generic_file_open(inode, filep) }
    }

    pub(crate) fn fence_pages_vec<'a, Op>(
        vec: Vec<DataPageWrapper<'a, Flushed, Op>>,
    ) -> Result<Vec<DataPageWrapper<'a, Clean, Op>>> {
        sfence();
        let mut clean_vec = Vec::new();
        for wrapper in vec {
            let clean_wrapper = DataPageWrapper::new(wrapper.page_no, wrapper.data_page);
            clean_vec.try_push(clean_wrapper)?;
        }
        Ok(clean_vec)
    }
}
