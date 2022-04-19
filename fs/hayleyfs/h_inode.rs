#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

use crate::def::*;
use crate::dir::hayleyfs_dir::*;
use crate::dir::*;
use crate::file::hayleyfs_file::*;
use crate::pm::*;
use crate::super_def::hayleyfs_bitmap::*;
use crate::super_def::*;
// use crate::{fence_all_vecs, fence_vec};
use core::marker::PhantomData;
use core::mem::size_of;
use core::ptr::write_bytes;
use kernel::bindings::{
    current_time, from_kgid, from_kuid, init_user_ns, inode, super_block, user_namespace, S_IFDIR,
};
use kernel::prelude::*;
use kernel::PAGE_SIZE;

pub(crate) type InodeNum = usize;

// reserved inode nums
pub(crate) const HAYLEYFS_ROOT_INO: InodeNum = 1;

pub(crate) enum NewInodeType {
    Create,
    Mkdir,
}

pub(crate) mod hayleyfs_inode {
    use super::*;

    // inode that lives in PM
    #[repr(C)]
    #[derive(Debug)]
    struct HayleyfsInode {
        mode: u16, // file mode (directory, regular, etc.)
        link_count: u32,
        uid: u32,
        gid: u32,
        num_blks: i64,
        flags: u32,
        ctime: i64, // inode change time
        mtime: i64, // modification time
        atime: i64, // access time
        size: i64,  // size of data in bytes
        ino: InodeNum,
        // pages are set to 0 when there is no page associated with them because
        // reading Rust Options from PM doesn't work very well
        // 12 direct pointers, one indirect pointer
        // TODO: pointers to pm pages could be smaller than usize since we don't
        // really need 64 bits to represent pages
        direct_pages: [PmPage; DIRECT_PAGES_PER_INODE],
        indirect_page: PmPage,
    }

    impl HayleyfsInode {
        fn inc_links(&mut self) {
            self.link_count += 1;
        }

        fn dec_links(&mut self) {
            self.link_count -= 1;
        }
    }

    // we should only be able to modify inodes via an InodeWrapper that
    // handles flushing it and keeping track of the last operation
    // so the private/public stuff has to be set up so the compiler enforces that
    #[must_use]
    #[derive(Debug)]
    pub(crate) struct InodeWrapper<'a, State, Op, Type: ?Sized> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        inode_type: PhantomData<Type>,
        ino: InodeNum,
        inode: &'a mut HayleyfsInode,
    }

    impl<'a, State, Op, Type> PmObjWrapper for InodeWrapper<'a, State, Op, Type> {}

    impl<'a, State, Op, Type> PmObjWrapper for Vec<InodeWrapper<'a, State, Op, Type>> {}

    impl<'a, State, Op, Type> InodeWrapper<'a, State, Op, Type> {
        fn new(inode: &'a mut HayleyfsInode) -> Self {
            Self {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                ino: inode.ino,
                inode,
            }
        }

        pub(crate) fn has_data_page(&self) -> bool {
            self.inode.direct_pages[0] != 0
        }

        /// Applies a closure to each direct page in the inode.
        /// Primarily used for readdir
        /// NOTE: this function reads pages as DATA PAGES, not dir pages. if you
        /// want to operate on dir pages, you need to convert them to dir pages
        /// yourself in the closure you pass in.
        /// TODO: the data page wrapper state stuff might get funky?
        pub(crate) fn read_direct_pages<F>(&self, sbi: &SbInfo, mut f: F) -> Result<()>
        where
            F: FnMut(DataPageWrapper<'a, Clean, Read>),
        {
            let direct_pages_in_use: usize = (self.inode.size / PAGE_SIZE as i64).try_into()?;
            for index in 0..direct_pages_in_use {
                let direct_page_no = self.inode.direct_pages[index];
                let page = DataPageWrapper::read_data_page(sbi, direct_page_no)?;
                f(page);
            }

            Ok(())
        }

        pub(crate) fn get_ino(&self) -> InodeNum {
            self.ino
        }

        pub(crate) fn is_dir(&self) -> bool {
            (self.inode.mode as u32 & S_IFDIR) != 0
        }

        pub(crate) fn get_flags(&self) -> u32 {
            self.inode.flags
        }

        pub(crate) fn get_mode(&self) -> u16 {
            self.inode.mode
        }

        pub(crate) fn get_uid(&self) -> u32 {
            self.inode.uid
        }

        pub(crate) fn get_gid(&self) -> u32 {
            self.inode.gid
        }

        pub(crate) fn get_size(&self) -> i64 {
            self.inode.size
        }

        pub(crate) fn get_link_count(&self) -> u32 {
            self.inode.link_count
        }

        pub(crate) fn get_ctime(&self) -> i64 {
            self.inode.ctime
        }

        pub(crate) fn get_mtime(&self) -> i64 {
            self.inode.mtime
        }

        pub(crate) fn get_num_blks(&self) -> i64 {
            self.inode.num_blks
        }

        pub(crate) fn get_direct_pages(&self) -> &[PmPage] {
            &self.inode.direct_pages
        }

        // TODO: can we invalidate without overwriting the whole thing?
        // or implement a memset with nontemporal stores at least
        pub(crate) fn zero_inode(
            self,
            _: &DentryWrapper<'a, Clean, Zero>,
        ) -> InodeWrapper<'a, Flushed, Zero, Type> {
            unsafe { write_bytes(self.inode, 0, 1) };
            clwb(&self.inode, size_of::<HayleyfsInode>(), false);
            InodeWrapper::new(self.inode)
        }
    }

    impl<'a, State, Op> InodeWrapper<'a, State, Op, Dir> {
        pub(crate) fn get_new_dentry(
            self,
            sbi: &'a SbInfo,
        ) -> Result<(
            DentryWrapper<'a, Clean, Alloc>,
            InodeWrapper<'a, Flushed, AddPage, Dir>,
        )> {
            // search for the first available dentry. if there are no free dentries on
            // the currently allocated pages, try to allocate a new page.
            // TODO: linear search through the dentries seems slow?
            // TODO: handle indirect pages
            let num_blks = self.inode.num_blks;
            for i in 0..num_blks {
                let i: usize = i.try_into()?;
                let page_no = self.inode.direct_pages[i];
                let dir_page = DirPage::read_dir_page(sbi, page_no)?;
                let res = dir_page.get_next_free_dentry();
                if let Ok(dentry) = res {
                    let pi = unsafe { self.coerce_to_addpage_flushed() };
                    return Ok((dentry, pi));
                }
            }
            // if we get here, we need to allocate a new page
            // TODO: check for enospc BEFORE iterating over the entire directory
            let (page_no, bitmap) =
                BitmapWrapper::read_data_bitmap(sbi).find_and_set_next_zero_bit()?;
            let bitmap = bitmap.flush().fence();
            let pi = self.add_dir_page(page_no, bitmap)?;
            // TODO: should read dir page require proof that the page is allocated?
            let dir_page = DirPage::read_dir_page(sbi, page_no)?;
            // let dentry = dir_page.dentries[0];
            let dentry = dir_page.get_next_free_dentry()?;
            Ok((dentry, pi))
        }

        pub(crate) fn coerce_to_addpage(self) -> InodeWrapper<'a, Clean, AddPage, Dir> {
            // runtime check to make sure the coercion is valid
            // TODO: this isn't GREAT, but should be ok since can't
            // check at compile time?
            assert!(self.has_data_page());
            InodeWrapper::new(self.inode)
        }
    }

    // TODO: be careful with this - could accidentally use it to mark an inode flushed
    // when it is actually dirty. that is why it is marked unsafe. i think you could get
    // this to work without unsafe fns if you just go over the addpage coercion once
    // direct/indirect blocks are worked out
    impl<'a, State, Op> InodeWrapper<'a, State, Op, Dir> {
        pub(crate) unsafe fn coerce_to_addpage_flushed(
            self,
        ) -> InodeWrapper<'a, Flushed, AddPage, Dir> {
            // runtime check to make sure the coercion is valid
            // TODO: this isn't GREAT, but should be ok since can't
            // check at compile time?
            assert!(self.has_data_page());
            InodeWrapper::new(self.inode)
        }
    }

    impl<'a> InodeWrapper<'a, Clean, Init, Dir> {
        // pub(crate) fn add_dir_page(
        //     self,
        //     sbi: &SbInfo,
        //     inode: &mut inode,
        //     page: PmPage,
        //     // _self_dentry: DentryWrapper<'a, Clean, Init>,
        //     // _parent_dentry: DentryWrapper<'a, Clean, Init>,
        // ) -> Result<InodeWrapper<'a, Flushed, AddPage, Dir>> {
        //     check_page_no(sbi, page)?;
        //     // TODO: should probably have some wrappers that return the dirty inode and force
        //     // some clearer flush/fence ordering to make sure you remember to actually do it
        //     let current_size = self.inode.size;
        //     let page_size_i64: i64 = PAGE_SIZE.try_into()?;
        //     let pages_per_inode_i64 = DIRECT_PAGES_PER_INODE.try_into()?;
        //     // if we are initializing the inode
        //     if current_size == page_size_i64 && self.inode.direct_pages[0] == 0 {
        //         self.inode.direct_pages[0] = page;
        //     } else {
        //         let index = current_size / page_size_i64;
        //         if index >= pages_per_inode_i64 {
        //             pr_alert!("All direct pages are full, need to set up indirect\n");
        //             return Err(ENOSPC);
        //         }
        //         self.inode.size += page_size_i64;
        //         unsafe { hayleyfs_i_size_write(inode, self.inode.size) };
        //     }
        //     self.inode.num_blks += 1;

        //     // TODO: just flush the page you modified and the size of the inode
        //     clwb(&self.inode, CACHELINE_SIZE, false);
        //     Ok(InodeWrapper::new(self.inode))
        // }

        pub(crate) fn add_dir_page_fence(
            self,
            sbi: &SbInfo,
            inode: &mut inode,
            page: PmPage,
            _: DentryWrapper<'a, Clean, Init>,
            _: DentryWrapper<'a, Clean, Init>,
        ) -> Result<InodeWrapper<'a, Clean, AddPage, Dir>> {
            check_page_no(sbi, page)?;
            // TODO: should probably have some wrappers that return the dirty inode and force
            // some clearer flush/fence ordering to make sure you remember to actually do it
            let current_size = self.inode.size;
            let page_size_i64: i64 = PAGE_SIZE.try_into()?;
            let pages_per_inode_i64 = DIRECT_PAGES_PER_INODE.try_into()?;
            // if we are initializing the inode
            if current_size == page_size_i64 && self.inode.direct_pages[0] == 0 {
                self.inode.direct_pages[0] = page;
            } else {
                let index = current_size / page_size_i64;
                if index >= pages_per_inode_i64 {
                    pr_alert!("All direct pages are full, need to set up indirect\n");
                    return Err(ENOSPC);
                }
                self.inode.size += page_size_i64;
                unsafe { hayleyfs_i_size_write(inode, self.inode.size) };
            }
            self.inode.num_blks += 1;

            // TODO: just flush the page you modified and the size of the inode
            clwb(&self.inode, CACHELINE_SIZE, true);
            Ok(InodeWrapper::new(self.inode))
        }

        // pub(crate) fn get_new_dentry(
        //     self,
        //     sbi: &'a SbInfo,
        // ) -> Result<(
        //     DentryWrapper<'a, Clean, Read>,
        //     InodeWrapper<'a, Clean, AddPage, Dir>,
        // )> {
        //     // search for the first available dentry. if there are no free dentries on
        //     // the currently allocated pages, try to allocate a new page.
        //     // TODO: linear search through the dentries seems slow?
        //     // TODO: handle indirect pages
        //     let num_blks = self.inode.num_blks;
        //     for i in 0..num_blks {
        //         let i: usize = i.try_into()?;
        //         let page_no = self.inode.direct_pages[i];
        //         let dir_page = DirPage::read_dir_page(sbi, page_no)?;
        //         for dentry in dir_page.iter_mut() {
        //             if !dentry.is_valid() {
        //                 return Ok((dentry, InodeWrapper::new(self.inode)));
        //             }
        //         }
        //     }
        //     // if we get here, we need to allocate a new page
        //     // TODO: check for enospc BEFORE iterating over the entire directory
        //     let (page_no, bitmap) =
        //         BitmapWrapper::read_data_bitmap(sbi).find_and_set_next_zero_bit()?;
        //     let bitmap = bitmap.flush().fence();
        //     let pi = self.add_dir_page(page_no, bitmap)?;
        //     // TODO: should read dir page require proof that the page is allocated?
        //     let dir_page = DirPage::read_dir_page(sbi, page_no)?;
        //     // let dentry = dir_page.dentries[0];
        //     let dentry = dir_page.get_next_free_dentry()?;
        //     Ok((dentry, pi))
        // }
    }

    // TODO: some redundant code here because we need different read methods
    // for different types of inodes. Can we get rid of that/reuse the code somehow?
    impl<'a> InodeWrapper<'a, Clean, Read, Data> {
        pub(crate) fn read_file_inode(sbi: &SbInfo, ino: &InodeNum) -> Self {
            let addr = (PAGE_SIZE * INODE_PAGE) + (ino * size_of::<HayleyfsInode>());
            let addr = sbi.virt_addr as usize + addr;
            let inode = unsafe { &mut *(addr as *mut HayleyfsInode) };
            Self {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                ino: *ino,
                inode,
            }
        }

        // TODO: the primitive type converstions in here are atrocious
        pub(crate) fn add_data_pages(
            self,
            pages: Vec<PmPage>,
            _: BitmapWrapper<'a, Clean, Alloc, Data>,
        ) -> Result<InodeWrapper<'a, Clean, AddPage, Data>> {
            let num_pages: i64 = pages.len().try_into()?;
            if num_pages + self.inode.num_blks > DIRECT_PAGES_PER_INODE.try_into()? {
                // this should actually never happen since we adjust the number of blocks
                // to allocate and add based on available capacity prior to calling this
                // function
                Err(ENOSPC)
            } else {
                let index: usize = (self.inode.num_blks - 1) as usize;
                for i in index..pages.len() {
                    self.inode.direct_pages[i] = pages[i - index];
                }
                self.inode.num_blks += num_pages;
                // TODO: you don't have to flush the whole thing
                clwb(&self.inode, size_of::<HayleyfsInode>(), true);
                Ok(InodeWrapper::new(self.inode))
            }
        }

        pub(crate) fn add_data_page(
            self,
            page_no: PmPage,
            _: BitmapWrapper<'a, Clean, Alloc, Data>,
        ) -> Result<InodeWrapper<'a, Clean, AddPage, Data>> {
            if (self.inode.num_blks + 1) > DIRECT_PAGES_PER_INODE.try_into()? {
                Err(ENOSPC)
            } else {
                let num_pages: usize = self.inode.num_blks.try_into()?;
                self.inode.direct_pages[num_pages - 1] = page_no;
                self.inode.num_blks += 1;
                clwb(&self.inode, size_of::<HayleyfsInode>(), true);
                Ok(InodeWrapper::new(self.inode))
            }
        }

        pub(crate) fn coerce_to_addpage(self) -> InodeWrapper<'a, Clean, AddPage, Data> {
            // runtime check to make sure the coercion is valid
            // TODO: this isn't GREAT, but should be ok since can't
            // check at compile time?
            assert!(self.has_data_page());
            InodeWrapper::new(self.inode)
        }

        // /// TODO: return something other than the vector of emptied pages
        // /// Maybe a DataPages wrapper or something?
        // pub(crate) fn clear_data_pages(
        //     &self,
        //     sbi: &SbInfo,
        // ) -> Result<Vec<DataPageWrapper<'a, Clean, Zero>>> {
        //     // TODO: update for indirect blocks
        //     let num_pages = self.inode.num_blks;
        //     let zeroed_pages = Vec::new();
        //     assert!(num_pages <= DIRECT_PAGES_PER_INODE.try_into()?);
        //     for i in 0..num_pages {
        //         let i: usize = i.try_into()?;
        //         let data_page = DataPageWrapper::read_data_page(sbi, self.inode.direct_pages[i])?;
        //         let data_page = data_page.zero_page();
        //         zeroed_pages.try_push(data_page)?;
        //     }
        //     // let zeroed_pages = fence_all_vecs!(zeroed_pages);
        //     let zeroed_pages = fence_pages_vec(zeroed_pages)?;
        //     Ok(zeroed_pages)
        // }
    }

    impl<'a, State, Op> InodeWrapper<'a, State, Op, Data> {
        /// TODO: should probably return clean inode with updated access time?
        pub(crate) fn read_data(
            &self,
            sbi: &SbInfo,
            len: i64,
            offset: i64,
            buf: *mut i8,
        ) -> Result<i64> {
            let page_size_i64: i64 = PAGE_SIZE.try_into()?;
            let mut bytes_read = 0;
            let mut current_page_index = offset / page_size_i64; // this is an index into the direct pages array
                                                                 // TODO: this logic will change for indirect pages
            let mut page_offset = offset - ((current_page_index - 1) * page_size_i64);
            while bytes_read < len {
                let bytes_to_read = if (page_size_i64 - page_offset) < len {
                    page_size_i64 - page_offset
                } else {
                    len
                };
                let data_page = DataPageWrapper::read_data_page(
                    sbi,
                    self.inode.direct_pages[current_page_index as usize],
                )?;
                let read = data_page.read_data(bytes_to_read, page_offset, buf, bytes_read)?;
                bytes_read += read;
                page_offset = 0;
            }
            Ok(bytes_read)
        }
    }

    impl<'a, State, Op> InodeWrapper<'a, State, Op, Dir> {
        pub(crate) fn add_dir_page(
            self,
            page_no: PmPage,
            _: BitmapWrapper<'a, Clean, Alloc, Data>,
        ) -> Result<InodeWrapper<'a, Flushed, AddPage, Dir>> {
            if (self.inode.num_blks + 1) > DIRECT_PAGES_PER_INODE.try_into()? {
                Err(ENOSPC)
            } else {
                let num_pages: usize = self.inode.num_blks.try_into()?;
                self.inode.direct_pages[num_pages - 1] = page_no;
                self.inode.num_blks += 1;
                clwb(&self.inode, size_of::<HayleyfsInode>(), false);
                Ok(InodeWrapper::new(self.inode))
            }
        }
    }

    impl<'a> InodeWrapper<'a, Clean, AddPage, Data> {
        pub(crate) fn set_size(
            self,
            pos: i64,
            pages_allocated: i64,
            _: &Vec<DataPageWrapper<'a, Clean, WriteData>>,
        ) -> InodeWrapper<'a, Clean, Size, Data> {
            self.inode.size = pos;
            self.inode.num_blks = pages_allocated;
            // TODO: we don't have to flush the whole thing
            clwb(&self.inode, size_of::<HayleyfsInode>(), true);
            InodeWrapper::new(self.inode)
        }

        pub(crate) fn write_data(
            &self,
            sbi: &SbInfo,
            len: i64,
            offset: i64,
            buf: *const i8,
        ) -> Result<(Vec<DataPageWrapper<'a, Clean, WriteData>>, i64)> {
            let mut page_wrapper_vec = Vec::new();
            let page_size_i64: i64 = PAGE_SIZE.try_into()?;
            let mut bytes_written = 0;
            let mut current_page_index = offset / page_size_i64; // this is an index into the direct pages array
                                                                 // TODO: this logic will change for indirect pages
            let mut page_offset = offset - ((current_page_index - 1) * page_size_i64);

            while bytes_written < len {
                let bytes_to_write = if (page_size_i64 - page_offset) < len {
                    page_size_i64 - page_offset
                } else {
                    len
                };
                let data_page = DataPageWrapper::read_data_page(
                    sbi,
                    self.inode.direct_pages[current_page_index as usize],
                )?;
                // TODO: what happens if we don't write enough bytes to one page for some reason?
                // we could end up with a weird hole?
                let (data_page, written) =
                    data_page.write_data(bytes_to_write, page_offset, buf, bytes_written)?;
                bytes_written += written;
                page_offset = 0;
                page_wrapper_vec.try_push(data_page)?;
            }
            sfence();
            Ok((page_wrapper_vec, bytes_written))
        }
    }

    impl<'a> InodeWrapper<'a, Clean, Read, Dir> {
        pub(crate) fn read_dir_inode(sbi: &SbInfo, ino: &InodeNum) -> Self {
            let addr = (PAGE_SIZE * INODE_PAGE) + (ino * size_of::<HayleyfsInode>());
            let addr = sbi.virt_addr as usize + addr;
            let inode = unsafe { &mut *(addr as *mut HayleyfsInode) };
            Self {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                ino: *ino,
                inode,
            }
        }

        pub(crate) fn lookup_dentry_by_name(
            &self,
            sbi: &'a SbInfo,
            name: &[u8],
        ) -> Result<DentryWrapper<'a, Clean, Read>> {
            let direct_pages_in_use: usize = (self.inode.size / PAGE_SIZE as i64).try_into()?;
            for index in 0..direct_pages_in_use {
                let direct_page_no = self.inode.direct_pages[index];
                let dir_page = DirPage::read_dir_page(sbi, direct_page_no)?;
                for dentry in dir_page.iter_mut() {
                    if dentry.is_valid() && compare_dentry_name(dentry.get_name(), name) {
                        return Ok(dentry);
                    }
                }
            }
            Err(ENOENT)
        }

        // pub(crate) fn get_new_dentry(
        //     self,
        //     sbi: &'a SbInfo,
        // ) -> Result<(
        //     DentryWrapper<'a, Clean, Read>,
        //     InodeWrapper<'a, Clean, AddPage, Dir>,
        // )> {
        //     // search for the first available dentry. if there are no free dentries on
        //     // the currently allocated pages, try to allocate a new page.
        //     // TODO: linear search through the dentries seems slow?
        //     // TODO: handle indirect pages
        //     let num_blks = self.inode.num_blks;
        //     for i in 0..num_blks {
        //         let i: usize = i.try_into()?;
        //         let page_no = self.inode.direct_pages[i];
        //         let dir_page = DirPage::read_dir_page(sbi, page_no)?;
        //         for dentry in dir_page.iter_mut() {
        //             if !dentry.is_valid() {
        //                 return Ok((dentry, InodeWrapper::new(self.inode)));
        //             }
        //         }
        //     }
        //     // if we get here, we need to allocate a new page
        //     // TODO: check for enospc BEFORE iterating over the entire directory
        //     let (page_no, bitmap) =
        //         BitmapWrapper::read_data_bitmap(sbi).find_and_set_next_zero_bit()?;
        //     let bitmap = bitmap.flush().fence();
        //     let pi = self.add_dir_page(page_no, bitmap)?;
        //     // TODO: should read dir page require proof that the page is allocated?
        //     let dir_page = DirPage::read_dir_page(sbi, page_no)?;
        //     // let dentry = dir_page.dentries[0];
        //     let dentry = dir_page.get_next_free_dentry()?;
        //     Ok((dentry, pi))
        // }

        // pub(crate) fn add_dir_page(
        //     self,
        //     page_no: PmPage,
        //     _: BitmapWrapper<'a, Clean, Alloc, Data>,
        // ) -> Result<InodeWrapper<'a, Clean, AddPage, Dir>> {
        //     if (self.inode.num_blks + 1) > DIRECT_PAGES_PER_INODE.try_into()? {
        //         Err(ENOSPC)
        //     } else {
        //         let num_pages: usize = self.inode.num_blks.try_into()?;
        //         self.inode.direct_pages[num_pages - 1] = page_no;
        //         self.inode.num_blks += 1;
        //         clwb(&self.inode, size_of::<HayleyfsInode>(), true);
        //         Ok(InodeWrapper::new(self.inode))
        //     }
        // }
    }

    impl<'a> InodeWrapper<'a, Clean, Read, Unknown> {
        pub(crate) fn read_unknown_inode(sbi: &SbInfo, ino: &InodeNum) -> Self {
            let addr = (PAGE_SIZE * INODE_PAGE) + (ino * size_of::<HayleyfsInode>());
            let addr = sbi.virt_addr as usize + addr;
            let inode = unsafe { &mut *(addr as *mut HayleyfsInode) };
            Self {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                ino: *ino,
                inode,
            }
        }

        // these are unsafe because they should only be called if you are absolutely
        // sure of the type of an unknown inode. using the wrong one could cause weird
        // memory issues
        pub(crate) unsafe fn unknown_to_file(self) -> InodeWrapper<'a, Clean, Read, Data> {
            InodeWrapper::new(self.inode)
        }

        pub(crate) unsafe fn unknown_to_dir(self) -> InodeWrapper<'a, Clean, Read, Dir> {
            InodeWrapper::new(self.inode)
        }
    }

    impl<'a, Type> InodeWrapper<'a, Clean, Read, Type> {
        // // TODO: this might need to go in a different impl
        // pub(crate) fn inc_links(self) -> InodeWrapper<'a, Flushed, Link, Type> {
        //     self.inode.inc_links();
        //     clwb(self.inode, size_of::<HayleyfsInode>(), false);
        //     InodeWrapper::new(self.inode)
        // }

        pub(crate) fn dec_links(self) -> InodeWrapper<'a, Flushed, Link, Type> {
            self.inode.dec_links();
            clwb(self.inode, size_of::<HayleyfsInode>(), false);
            InodeWrapper::new(self.inode)
        }

        /// TODO: return something other than the vector of emptied pages
        /// Maybe a DataPages wrapper or something?
        pub(crate) fn clear_pages(
            &self,
            sbi: &SbInfo,
            _: &DentryWrapper<'a, Clean, Zero>,
        ) -> Result<Vec<DataPageWrapper<'a, Clean, Zero>>> {
            // TODO: update for indirect blocks
            let num_pages = self.inode.num_blks;
            let mut zeroed_pages = Vec::new();
            assert!(num_pages <= DIRECT_PAGES_PER_INODE.try_into()?);
            for i in 0..num_pages {
                let i: usize = i.try_into()?;
                let data_page = DataPageWrapper::read_data_page(sbi, self.inode.direct_pages[i])?;
                let data_page = data_page.zero_page();
                zeroed_pages.try_push(data_page)?;
            }
            let zeroed_pages = fence_pages_vec(zeroed_pages)?;
            Ok(zeroed_pages)
        }
    }

    impl<'a, Op, Type> InodeWrapper<'a, Clean, Op, Type> {
        // TODO: this might need to go in a different impl
        pub(crate) fn inc_links(self) -> InodeWrapper<'a, Flushed, Link, Type> {
            self.inode.inc_links();
            clwb(self.inode, size_of::<HayleyfsInode>(), false);
            InodeWrapper::new(self.inode)
        }
    }

    impl<'a, Op, Type> InodeWrapper<'a, Flushed, Op, Type> {
        // intentionally does NOT call fence here; this is an unsafe function that
        // should only be used when fencing multiple objects on a single fence call
        // using the batch fence function/macro(whatever you end up using)
        pub(crate) unsafe fn fence_unsafe(self) -> InodeWrapper<'a, Clean, Op, Type> {
            InodeWrapper::new(self.inode)
        }

        pub(crate) fn fence(self) -> InodeWrapper<'a, Clean, Op, Type> {
            sfence();
            InodeWrapper::new(self.inode)
        }
    }

    impl<'a, Type> InodeWrapper<'a, Clean, Read, Type> {
        pub(crate) fn initialize_inode(
            self,
            new_mode: u16,
            parent_flags: u32,
            inode: &mut inode,
            _: &BitmapWrapper<'a, Clean, Alloc, Inode>,
        ) -> InodeWrapper<'a, Flushed, Init, Type> {
            // TODO: do these numbers make sense? do you have to do something with them to
            // make them make sense?
            self.inode.ino = inode.i_ino as usize;
            self.inode.mode = inode.i_mode;
            self.inode.num_blks = 0;
            self.inode.link_count = unsafe { inode.__bindgen_anon_1.i_nlink };
            self.inode.direct_pages = [0; DIRECT_PAGES_PER_INODE];
            self.inode.indirect_page = 0;
            self.inode.ctime = inode.i_ctime.tv_sec;
            self.inode.mtime = inode.i_mtime.tv_sec;
            self.inode.atime = inode.i_atime.tv_sec;
            self.inode.size = inode.i_size;
            self.inode.flags = hayleyfs_mask_flags(new_mode, parent_flags);
            self.inode.uid = unsafe { hayleyfs_uid_read(inode) } as u32;
            self.inode.gid = unsafe { hayleyfs_gid_read(inode) } as u32;
            clwb(self.inode, size_of::<HayleyfsInode>(), false);
            InodeWrapper::new(self.inode)
        }

        pub(crate) fn initialize_root_inode(
            self,
            sb: &super_block,
            sbi: &SbInfo,
            root_inode: &mut inode,
            _: &BitmapWrapper<'a, Clean, Alloc, Inode>,
        ) -> InodeWrapper<'a, Flushed, Init, Dir> {
            let current_time = unsafe { current_time(root_inode) };
            self.inode.mode = root_inode.i_mode;
            self.inode.num_blks = 0;
            self.inode.direct_pages = [0; DIRECT_PAGES_PER_INODE];
            self.inode.indirect_page = 0;
            unsafe {
                self.inode.uid = from_kuid(&mut init_user_ns as *mut user_namespace, sbi.uid);
                self.inode.gid = from_kgid(&mut init_user_ns as *mut user_namespace, sbi.gid);
            }
            self.inode.link_count = unsafe { root_inode.__bindgen_anon_1.i_nlink };
            self.inode.size = sb.s_blocksize as i64;
            self.inode.flags = 0;
            self.inode.ino = ROOT_INO;
            self.inode.ctime = current_time.tv_sec;
            self.inode.atime = current_time.tv_sec;
            self.inode.mtime = current_time.tv_sec;
            clwb(self.inode, size_of::<HayleyfsInode>(), false);
            InodeWrapper::new(self.inode)
        }
    }
}

// mode is new inode's mode, flags are its parent's flags. it inherits some flags
// but we need to mask out some depending on the file types
// this logic and the inherited flags is 100% stolen from NOVA
fn hayleyfs_mask_flags(mode: u16, flags: u32) -> u32 {
    let flags = flags & HAYLEYFS_FL_INHERITED;
    if unsafe { hayleyfs_isdir(mode) } {
        flags
    } else if unsafe { hayleyfs_isreg(mode) } {
        flags & HAYLEYFS_REG_FLMASK
    } else {
        flags & HAYLEYFS_OTHER_FLMASK
    }
}
