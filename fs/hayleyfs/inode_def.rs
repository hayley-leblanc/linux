use crate::def::*;
use crate::super_def::*;
use core::marker::PhantomData;
use core::mem::size_of;
use kernel::bindings::S_IFDIR;
use kernel::PAGE_SIZE;

pub(crate) type InodeNum = usize;

// reserved inode nums
pub(crate) const HAYLEYFS_ROOT_INO: InodeNum = 1;

enum NewInodeType {
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
    }

    impl<'a> InodeWrapper<'a, Clean, Read> {
        pub(crate) fn read_inode(ino: InodeNum, sbi: &SbInfo) -> Self {
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
        pub(crate) fn initialize_inode(self, ino: InodeNum) -> InodeWrapper<'a, Flushed, Init> {
            self.inode.set_up(ino, None, S_IFDIR, 2);
            InodeWrapper::new(self.inode)
        }
    }

    impl<'a, Op> InodeWrapper<'a, Flushed, Op> {
        pub(crate) unsafe fn fence_unsafe(self) -> InodeWrapper<'a, Clean, Op> {
            InodeWrapper::new(self.inode)
        }
    }
}
