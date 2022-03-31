#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::used_underscore_binding)]

use crate::def::*;
use crate::dir::hayleyfs_dir::*;
use crate::pm::*;
use crate::super_def::hayleyfs_bitmap::*;
use crate::super_def::*;
use core::marker::PhantomData;
use core::mem::size_of;
use kernel::bindings::S_IFDIR;
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
    struct HayleyfsInode {
        link_count: u16,
        mode: u32,
        ino: InodeNum,
        data0: Option<PmPage>,
    }

    impl HayleyfsInode {
        fn set_up(&mut self, ino: InodeNum, data: Option<PmPage>, mode: u32, link_count: u16) {
            self.ino = ino;
            self.data0 = data;
            self.mode = mode;
            self.link_count = link_count;
        }

        fn set_page(&mut self, page: Option<PmPage>) {
            self.data0 = page;
        }

        fn inc_links(&mut self) {
            self.link_count += 1;
        }
    }

    // we should only be able to modify inodes via an InodeWrapper that
    // handles flushing it and keeping track of the last operation
    // so the private/public stuff has to be set up so the compiler enforces that
    #[must_use]
    pub(crate) struct InodeWrapper<'a, State, Op, Type: ?Sized> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        inode_type: PhantomData<Type>,
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
                inode,
            }
        }

        pub(crate) fn get_data_page_no(&self) -> Option<PmPage> {
            self.inode.data0
        }

        pub(crate) fn get_ino(&self) -> InodeNum {
            self.inode.ino
        }

        pub(crate) fn is_dir(&self) -> bool {
            (self.inode.mode & S_IFDIR) != 0
        }

        pub(crate) fn zero_inode(self) -> InodeWrapper<'a, Flushed, Zero, Type> {
            self.inode.ino = 0;
            self.inode.data0 = None;
            self.inode.mode = 0;
            self.inode.link_count = 0;
            clwb(&self.inode, size_of::<HayleyfsInode>(), false);
            InodeWrapper::new(self.inode)
        }
    }

    impl<'a> InodeWrapper<'a, Clean, Init, Dir> {
        pub(crate) fn add_dir_page(
            self,
            page: Option<PmPage>,
            _self_dentry: DentryWrapper<'a, Clean, Init>,
            _parent_dentry: DentryWrapper<'a, Clean, Init>,
        ) -> InodeWrapper<'a, Flushed, Valid, Dir> {
            // TODO: should probably have some wrappers that return the dirty inode and force
            // some clearer flush/fence ordering to make sure you remember to actually do it
            self.inode.set_page(page);
            clwb(&self.inode.data0, CACHELINE_SIZE, false);
            InodeWrapper::new(self.inode)
        }

        pub(crate) fn add_dir_page_fence(
            self,
            page: Option<PmPage>,
            _: DentryWrapper<'a, Clean, Init>,
            _: DentryWrapper<'a, Clean, Init>,
        ) -> InodeWrapper<'a, Clean, Valid, Dir> {
            // TODO: should probably have some wrappers that return the dirty inode and force
            // some clearer flush/fence ordering to make sure you remember to actually do it
            self.inode.set_page(page);
            clwb(&self.inode.data0, CACHELINE_SIZE, true);
            InodeWrapper::new(self.inode)
        }
    }

    // TODO: some redundant code here because we need different read methods
    // for different types of indoes. Can we get rid of that/reuse the code somehow?
    impl<'a> InodeWrapper<'a, Clean, Read, Data> {
        pub(crate) fn read_file_inode(sbi: &SbInfo, ino: &InodeNum) -> Self {
            let addr = (PAGE_SIZE * INODE_PAGE) + (ino * size_of::<HayleyfsInode>());
            let addr = sbi.virt_addr as usize + addr;
            let inode = unsafe { &mut *(addr as *mut HayleyfsInode) };
            Self {
                state: PhantomData,
                op: PhantomData,
                inode_type: PhantomData,
                inode,
            }
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
                inode,
            }
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
        pub(crate) fn initialize_inode(
            self,
            ino: InodeNum,
            page: Option<PmPage>,
            mode: u32,
            link_count: u16,
            _: &BitmapWrapper<'_, Clean, Alloc, Inode>,
        ) -> InodeWrapper<'a, Flushed, Init, Type> {
            self.inode.set_up(ino, page, mode, link_count);
            clwb(self.inode, size_of::<HayleyfsInode>(), false);
            InodeWrapper::new(self.inode)
        }

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
}
