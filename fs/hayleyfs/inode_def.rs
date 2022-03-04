use crate::def::*;
use crate::super_def::*;
use core::marker::PhantomData;
use core::mem::size_of;
use kernel::PAGE_SIZE;

pub(crate) type InodeNum = usize;

// reserved inode nums
pub(crate) const HAYLEYFS_ROOT_INO: InodeNum = 1;

enum NewInodeType {
    Create,
    Mkdir,
}

mod hayleyfs_inode {
    use super::*;

    // inode that lives in PM
    #[repr(C)]
    struct HayleyfsInode {
        link_count: u16,
        mode: u32,
        ino: InodeNum,
        data0: Option<PmPage>,
    }

    // we should only be able to modify inodes via an InodeWrapper that
    // handles flushing it and keeping track of the last operation
    // so the private/public stuff has to be set up so the compiler enforces that
    pub(crate) struct InodeWrapper<'a, State = Clean, Op = Read> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        inode: &'a mut HayleyfsInode,
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
    }
}
