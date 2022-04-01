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
        data0: Option<PmPage>,
    }

    impl HayleyfsInode {
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

        // TODO: THIS NEEDS TO BE REWRITTEN
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
        // pub(crate) fn initialize_inode(
        //     self,
        //     ino: InodeNum,
        //     page: Option<PmPage>,
        //     mode: u32,
        //     link_count: u16,
        //     _: &BitmapWrapper<'_, Clean, Alloc, Inode>,
        // ) -> InodeWrapper<'a, Flushed, Init, Type> {
        //     self.inode.set_up(ino, page, mode, link_count);
        //     clwb(self.inode, size_of::<HayleyfsInode>(), false);
        //     InodeWrapper::new(self.inode)
        // }

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
            self.inode.mode = inode.i_mode;
            self.inode.link_count = unsafe { inode.__bindgen_anon_1.i_nlink };
            self.inode.data0 = None;
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
            let ifdir_16: u16 = S_IFDIR.try_into().unwrap();
            self.inode.mode = sbi.mode | ifdir_16;
            self.inode.data0 = None;
            unsafe {
                self.inode.uid = from_kuid(&mut init_user_ns as *mut user_namespace, sbi.uid);
                self.inode.gid = from_kgid(&mut init_user_ns as *mut user_namespace, sbi.gid);
            }
            self.inode.link_count = 2;
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
