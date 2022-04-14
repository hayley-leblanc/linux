#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

use crate::def::*;
use crate::dir::hayleyfs_dir::*;
use crate::file::hayleyfs_file::*;
use crate::pm::*;
use crate::super_def::hayleyfs_bitmap::*;
use crate::super_def::*;
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
        // fn set_page(&mut self, page: PmPage) {
        //     pr_info!("setting inode {:?} to page {:?}\n", self.ino, page);
        //     self.data0 = page;
        // }

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

        // pub(crate) fn get_data_page_no(&self) -> PmPage {
        //     if self.inode.data0 == 0 {
        //         pr_info!("inode {:?} has no data page\n", self.ino);
        //     }
        //     self.inode.data0
        // }

        /// Applies a closure to each direct page in the inode.
        /// Primarily used for readdir
        /// NOTE: this function reads pages as DATA PAGES, not dir pages. if you
        /// want to operate on dir pages, you need to convert them to dir pages
        /// yourself in the closure you pass in.
        /// TODO: the data page wrapper state stuff might get funky?
        pub(crate) fn read_direct_pages<F>(&self, sbi: &SbInfo, f: F) -> Result<()>
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
            self.inode.ino
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

    impl<'a> InodeWrapper<'a, Clean, Init, Dir> {
        pub(crate) fn add_dir_page(
            self,
            sbi: &SbInfo,
            inode: &mut inode,
            page: PmPage,
            _self_dentry: DentryWrapper<'a, Clean, Init>,
            _parent_dentry: DentryWrapper<'a, Clean, Init>,
        ) -> Result<InodeWrapper<'a, Flushed, AddPage, Dir>> {
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

            // TODO: just flush the page you modified and the size of the inode
            clwb(&self.inode, CACHELINE_SIZE, false);
            Ok(InodeWrapper::new(self.inode))
        }

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

            // TODO: just flush the page you modified and the size of the inode
            clwb(&self.inode, CACHELINE_SIZE, true);
            Ok(InodeWrapper::new(self.inode))
        }
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

        pub(crate) fn add_data_page_fence(
            self,
            page: PmPage,
            _: BitmapWrapper<'a, Clean, Alloc, Data>,
        ) -> InodeWrapper<'a, Clean, AddPage, Data> {
            self.inode.set_page(page);
            clwb(&self.inode.data0, CACHELINE_SIZE, true);
            InodeWrapper::new(self.inode)
        }

        pub(crate) fn coerce_to_addpage(self) -> InodeWrapper<'a, Clean, AddPage, Data> {
            // runtime check to make sure the coercion is valid
            // TODO: this isn't GREAT, but should be ok since can't
            // check at compile time?
            assert!(self.get_data_page_no() != 0);
            InodeWrapper::new(self.inode)
        }

        pub(crate) fn clear_data_page(&self, sbi: &SbInfo) -> Result<Box<dyn EmptyFilePage>> {
            if self.inode.data0 == 0 {
                let page_no = self.inode.data0;
                let data_page = DataPageWrapper::read_data_page(sbi, page_no)?;
                let data_page = data_page.zero_page().fence();
                Ok(Box::try_new(data_page)?)
            } else {
                Ok(Box::try_new(EmptyPage {})?)
            }
        }
    }

    impl<'a> InodeWrapper<'a, Clean, AddPage, Data> {
        pub(crate) fn set_size(
            self,
            pos: i64,
            _: &DataPageWrapper<'a, Clean, WriteData>,
        ) -> InodeWrapper<'a, Clean, Size, Data> {
            self.inode.size = pos;
            clwb(&self.inode.size, CACHELINE_SIZE, true);
            InodeWrapper::new(self.inode)
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

        pub(crate) fn lookup_dentry_in_inode(
            &self,
            sbi: &'a SbInfo,
            child_name: &[u8],
        ) -> Result<DentryWrapper<'a, Clean, Read>> {
            DentryWrapper::lookup_dentry_by_name(sbi, child_name, self)
        }
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
        // TODO: this might need to go in a different impl
        pub(crate) fn inc_links(self) -> InodeWrapper<'a, Flushed, Link, Type> {
            self.inode.inc_links();
            clwb(self.inode, size_of::<HayleyfsInode>(), false);
            InodeWrapper::new(self.inode)
        }

        pub(crate) fn dec_links(self) -> InodeWrapper<'a, Flushed, Link, Type> {
            self.inode.dec_links();
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
