use crate::def::*;
use crate::inode_def::*;
use crate::pm::*;
use crate::super_def::*;
use core::marker::PhantomData;
use core::mem::size_of;
use kernel::prelude::*;
use kernel::PAGE_SIZE;

pub(crate) mod hayleyfs_dir {
    use super::*;

    struct DirPage {
        dentries: [HayleyfsDentry; DENTRIES_PER_PAGE],
    }

    struct HayleyfsDentry {
        valid: bool,
        ino: InodeNum,
        name_len: usize,
        name: [u8; MAX_FILENAME_LEN],
    }

    impl HayleyfsDentry {
        fn set_up(&mut self, ino: InodeNum, name: &str) {
            self.ino = ino;
            self.set_dentry_name(name);
            self.valid = false;
            clwb(self, size_of::<HayleyfsDentry>(), true);
            self.valid = true;
            clwb(&self.valid, CACHELINE_SIZE, false);
        }

        fn set_dentry_name(&mut self, name: &str) {
            // initialize the name array with zeroes, then set the name
            self.name = [0; MAX_FILENAME_LEN];
            // ensure it's null terminated by only copying at most MAX_FILENAME_LEN-1 bytes
            let num_bytes = if name.len() < MAX_FILENAME_LEN - 1 {
                name.len()
            } else {
                MAX_FILENAME_LEN - 1
            };
            self.name_len = num_bytes + 1;
            // TODO: this will not work with non-ascii characters
            let name = name.as_bytes();
            self.name[..num_bytes].clone_from_slice(&name[..num_bytes]);
        }
    }

    pub(crate) struct DentryWrapper<'a, State, Op> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        dentry: &'a mut HayleyfsDentry,
    }

    impl<'a, State, Op> PmObjWrapper for DentryWrapper<'a, State, Op> {}

    impl<'a, State, Op> DentryWrapper<'a, State, Op> {
        fn new(dentry: &'a mut HayleyfsDentry) -> Self {
            Self {
                state: PhantomData,
                op: PhantomData,
                dentry,
            }
        }
    }

    impl<'a> DentryWrapper<'a, Clean, Alloc> {
        /// returns the next unused dentry on the given page
        pub(crate) fn get_new_dentry(sbi: &SbInfo, page_no: PmPage) -> Result<Self> {
            let page_addr = sbi.virt_addr as usize + (page_no * PAGE_SIZE);
            let page = unsafe { &mut *(page_addr as *mut DirPage) };

            // obtain the next unused dentry
            for dentry in page.dentries.iter_mut() {
                if !dentry.valid {
                    return Ok(DentryWrapper::new(dentry));
                }
            }
            // if we get here, all dentries are in use
            Err(Error::ENOSPC)
        }

        pub(crate) fn initialize_dentry(
            self,
            ino: InodeNum,
            name: &str,
        ) -> DentryWrapper<'a, Flushed, Init> {
            self.dentry.set_up(ino, name);

            self.dentry.set_valid();
            DentryWrapper::new(self.dentry)
        }
    }

    impl<'a, Op> DentryWrapper<'a, Flushed, Op> {
        pub(crate) unsafe fn fence_unsafe(self) -> DentryWrapper<'a, Clean, Op> {
            DentryWrapper::new(self.dentry)
        }
    }
}
