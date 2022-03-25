use crate::def::*;
use crate::dir::hayleyfs_dir::*;
use crate::pm::*;
use crate::super_def::hayleyfs_bitmap::*;
use crate::super_def::*;
use core::marker::PhantomData;
use core::mem::size_of;
use kernel::bindings::S_IFDIR;
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
    pub(crate) struct InodeWrapper<'a, State, Op> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        inode: &'a mut HayleyfsInode,
    }

    impl<'a, State, Op> PmObjWrapper for InodeWrapper<'a, State, Op> {}

    impl<'a, State, Op> InodeWrapper<'a, State, Op> {
        fn new(inode: &'a mut HayleyfsInode) -> Self {
            Self {
                state: PhantomData,
                op: PhantomData,
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

        pub(crate) fn zero_inode(self) -> InodeWrapper<'a, Clean, Zero> {
            self.inode.ino = 0;
            self.inode.data0 = None;
            self.inode.mode = 0;
            self.inode.link_count = 0;
            clwb(&self.inode, size_of::<HayleyfsInode>(), true);
            InodeWrapper::new(self.inode)
        }
    }

    impl<'a> InodeWrapper<'a, Clean, Init> {
        // TODO: this should have a different soft updates indicator than Valid
        // but it works for mkdir since we can't use the inode until it points to a valid page
        pub(crate) fn add_dir_page(self, page: Option<PmPage>) -> InodeWrapper<'a, Flushed, Valid> {
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
        ) -> InodeWrapper<'a, Clean, Valid> {
            // TODO: should probably have some wrappers that return the dirty inode and force
            // some clearer flush/fence ordering to make sure you remember to actually do it
            self.inode.set_page(page);
            clwb(&self.inode.data0, CACHELINE_SIZE, true);
            InodeWrapper::new(self.inode)
        }
    }

    impl<'a> InodeWrapper<'a, Clean, Read> {
        pub(crate) fn read_inode(sbi: &SbInfo, ino: &InodeNum) -> Self {
            let addr = (PAGE_SIZE * INODE_PAGE) + (ino * size_of::<HayleyfsInode>());
            let addr = sbi.virt_addr as usize + addr;
            let inode = unsafe { &mut *(addr as *mut HayleyfsInode) };
            Self {
                state: PhantomData,
                op: PhantomData,
                inode,
            }
        }

        // TODO: add arguments for different types of files; right now this only does dirs
        pub(crate) fn initialize_inode(
            self,
            ino: InodeNum,
            _: &BitmapWrapper<'_, Clean, Alloc, InoBmap>,
        ) -> InodeWrapper<'a, Flushed, Init> {
            self.inode.set_up(ino, None, S_IFDIR, 2);
            clwb(self.inode, size_of::<HayleyfsInode>(), false);
            InodeWrapper::new(self.inode)
        }

        // TODO: this might need to go in a different impl
        pub(crate) fn inc_links(self) -> InodeWrapper<'a, Flushed, Link> {
            self.inode.inc_links();
            clwb(self.inode, size_of::<HayleyfsInode>(), false);
            InodeWrapper::new(self.inode)
        }
    }

    impl<'a, Op> InodeWrapper<'a, Flushed, Op> {
        // intentionally does NOT call fence here; this is an unsafe function that
        // should only be used when fencing multiple objects on a single fence call
        // using the batch fence function/macro(whatever you end up using)
        pub(crate) unsafe fn fence_unsafe(self) -> InodeWrapper<'a, Clean, Op> {
            InodeWrapper::new(self.inode)
        }
    }
}
