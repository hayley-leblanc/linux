#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

use crate::def::*;
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
            unsafe { ptr::write_bytes(&mut self.data_page.data, 0, PAGE_SIZE) };
            clwb(&self.data_page.data, PAGE_SIZE, false);
            DataPageWrapper::new(self.page_no, self.data_page)
        }

        pub(crate) fn write_data(
            self,
            buf: *const i8,
            len: usize,
            offset: usize,
        ) -> (DataPageWrapper<'a, Flushed, WriteData>, usize) {
            // TODO: non-temporal stores
            // figure out how many bytes to write
            let bytes_to_write = if PAGE_SIZE - offset < len {
                PAGE_SIZE - offset
            } else {
                len
            };
            // TODO: do you end up writing to the correct place with these type conversions?
            // TODO: what does the return value here actually mean?????
            let data_ptr = self.data_page.data.as_ptr() as *mut i8;
            let bytes_written = bytes_to_write
                - unsafe {
                    hayleyfs_copy_from_user_nt(
                        data_ptr.offset(offset.try_into().unwrap()) as *const c_void, // TODO: handle error properly
                        buf as *const c_void,
                        bytes_to_write.try_into().unwrap(), // TODO: handle error properly
                    ) as usize
                };
            // TODO: MUST FLUSH FIRST AND LAST CACHE LINES (or check if they need to be flushed)
            clwb(
                unsafe { data_ptr.offset(offset.try_into().unwrap()) },
                bytes_to_write,
                false,
            );
            (
                DataPageWrapper::new(self.page_no, self.data_page),
                bytes_written.try_into().unwrap(), // TODO: handle error properly
            )
        }

        pub(crate) fn read_data(&self, buf: *mut i8, len: usize, offset: usize) -> usize {
            let data_ptr = self.data_page.data.as_ptr() as *mut i8;
            let bytes_copied = len
                - unsafe {
                    hayleyfs_copy_to_user(
                        buf as *mut c_void,
                        data_ptr.offset(offset.try_into().unwrap()) as *const c_void, // TODO: include offset
                        len.try_into().unwrap(), // TODO: handle error properly
                    ) as usize
                };
            bytes_copied.try_into().unwrap() // TODO: handle error properly
        }
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
        len: usize,
        ppos: &mut i64,
        inode: &mut inode,
    ) -> Result<(WriteFinalizeToken, isize)> {
        // make sure we can access the user buffer
        if !unsafe { hayleyfs_access_ok(buf, len) } == 0 {
            return Err(Error::EFAULT);
        }

        let sb = inode.i_sb;
        let sbi = hayleyfs_get_sbi(sb);

        let mut pos = *ppos;

        if filep.f_flags & O_APPEND != 0 {
            pos = unsafe { hayleyfs_i_size_read(inode) };
        }

        let ino: InodeNum = inode.i_ino.try_into().unwrap();

        // obtain our inode and its data page
        // TODO: logic here will have to change when there is more than one page
        // associated with a file
        let pi = InodeWrapper::read_file_inode(sbi, &ino);

        // pi may or may not have a block already associated with it. if it doesn't,
        // we need to allocate a block for it

        let mut page_no = pi.get_data_page_no();

        // if page_no is none, we need to allocate a page and add it to the inode
        // which changes the inode's state. to make things easier to reason about,
        // lets coerce the inode into that same state even if we DON'T add a page,
        // since all that state tells us is that the inode has a valid allocated page
        // that can hold data
        // TODO: variables are weird here due to scoping and shadowing, try to figure
        // out a nicer way to handle it?
        let pi_temp;
        if page_no.is_none() {
            // allocate a page
            // save it in the inode
            let data_bitmap = BitmapWrapper::read_data_bitmap(sbi);
            let (page_no_temp, data_bitmap) = data_bitmap.find_and_set_next_zero_bit()?;
            let data_bitmap = data_bitmap.persist();
            pi_temp = pi.add_data_page_fence(page_no_temp, data_bitmap);
            page_no = Some(page_no_temp);
        } else {
            pi_temp = pi.coerce_to_addpage();
        }
        let pi = pi_temp;
        let page_no = page_no.unwrap();

        // TODO: should reading data page require an AddPage or higher inode?
        let data_page = DataPageWrapper::read_data_page(sbi, page_no)?;
        // TODO: if there's no more space in the file to write, return ENOSPC?
        let (data_page, bytes_written) = data_page.write_data(buf, len, pos.try_into()?);
        let data_page = data_page.fence();

        pos += bytes_written as i64;
        *ppos = pos;
        unsafe { hayleyfs_i_size_write(inode, pos) };
        inode.i_blocks = 1;
        // right now, we can just set the file size to pos + bytes written
        // TODO: in the future when the file can have multiple pages that won't be enough
        let pi = pi.set_size(pos, &data_page);

        let token = WriteFinalizeToken::new(pi, data_page);
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
        // filep: &mut file,
        buf: *mut i8,
        mut len: usize,
        ppos: &mut i64,
        inode: &mut inode,
    ) -> Result<isize> {
        // TODO: mark the file as accessed, update access time, etc.

        // make sure we can access the user buffer
        if !unsafe { hayleyfs_access_ok(buf, len) } == 0 {
            return Err(Error::EFAULT);
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

        // TODO: logic here will have to change when there is more than one page
        // associated with a file
        let pi = InodeWrapper::read_file_inode(sbi, &ino);
        let page_no = pi.get_data_page_no();
        if let Some(page_no) = page_no {
            let data_page = DataPageWrapper::read_data_page(sbi, page_no)?;
            let bytes_read = data_page.read_data(buf, len, pos.try_into()?);
            pos += bytes_read as i64;
            *ppos = pos;
            Ok(bytes_read.try_into()?)
        } else {
            Ok(0)
        }
    }

    #[no_mangle]
    pub(crate) unsafe extern "C" fn hayleyfs_open(inode: *mut inode, filep: *mut file) -> c_int {
        unsafe { generic_file_open(inode, filep) }
    }
}
