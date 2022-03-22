use crate::def::*;
use crate::inode_def::*;
use crate::pm::*;
use crate::super_def::*;
use core::marker::PhantomData;
use core::mem::size_of;
use kernel::bindings::{dir_context, file, file_operations, inode, ENOTDIR};
use kernel::c_types::{c_int, c_void};
use kernel::prelude::*;
use kernel::{c_default_struct, PAGE_SIZE};

#[no_mangle]
pub(crate) static mut HayleyfsFileOps: file_operations = file_operations {
    iterate: Some(hayleyfs_dir::hayleyfs_readdir),
    ..c_default_struct!(file_operations)
};

pub(crate) mod hayleyfs_dir {
    use super::*;

    struct DirPage {
        dentries: [HayleyfsDentry; DENTRIES_PER_PAGE],
    }

    impl DirPage {
        fn lookup_name(&self, name: &[u8]) -> Result<InodeNum> {
            for dentry in self.dentries.iter() {
                if !dentry.is_valid() {
                    return Err(Error::ENOENT);
                } else if compare_dentry_name(dentry.get_name(), name) {
                    return Ok(dentry.get_ino());
                }
            }
            Err(Error::ENOENT)
        }
    }

    fn get_data_page_addr(sbi: &SbInfo, page_no: PmPage) -> *mut c_void {
        (sbi.virt_addr as usize + (page_no * PAGE_SIZE)) as *mut c_void
    }

    // TODO: should probably not have the static lifetime
    fn get_dir_page(sbi: &SbInfo, page_no: PmPage) -> &'static mut DirPage {
        unsafe { &mut *(get_data_page_addr(sbi, page_no) as *mut DirPage) }
    }

    #[no_mangle]
    pub(crate) unsafe extern "C" fn hayleyfs_readdir(
        file: *mut file,
        ctx_raw: *mut dir_context,
    ) -> i32 {
        // TODO: check that the file is actually a directory
        // TODO: use in-memory inodes
        // TODO: nicer abstractions for unsafe code here

        let inode = unsafe { &mut *(hayleyfs_file_inode(file) as *mut inode) };
        let sb = inode.i_sb;
        let sbi = hayleyfs_get_sbi(sb);
        let pi = hayleyfs_inode::InodeWrapper::read_inode(sbi, inode.i_ino.try_into().unwrap());
        let ctx = unsafe { &mut *(ctx_raw as *mut dir_context) };

        if ctx.pos == READDIR_END {
            return 0;
        }

        match pi.get_data_page_no() {
            Some(page_no) => {
                // iterate over dentries and give to dir_emit
                let dir_page = hayleyfs_dir::get_dir_page(sbi, page_no);
                for i in 0..DENTRIES_PER_PAGE {
                    // TODO: should make a function that iterates over dentries in a page
                    // and takes a closure to perform the operation you want
                    // instead of directly reading the dentries here
                    let dentry = &dir_page.dentries[i];
                    if !dentry.is_valid() {
                        ctx.pos = READDIR_END;
                        return 0;
                    }
                    if unsafe {
                        !hayleyfs_dir_emit(
                            ctx,
                            dentry.name.as_ptr() as *const i8,
                            dentry.name_len.try_into().unwrap(),
                            pi.get_ino().try_into().unwrap(),
                            0,
                        )
                    } {
                        return 0;
                    }
                }
                ctx.pos = READDIR_END;
                0
            }
            None => -(ENOTDIR as c_int),
        }
    }

    pub(crate) fn lookup_dentry(sbi: &SbInfo, page_no: PmPage, name: &[u8]) -> Result<InodeNum> {
        let dir_page = get_dir_page(sbi, page_no);
        dir_page.lookup_name(name)
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

        fn is_valid(&self) -> bool {
            self.valid
        }

        fn get_name(&self) -> &[u8] {
            &self.name
        }

        fn get_ino(&self) -> InodeNum {
            self.ino
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
                    // pr_info!("allocating dentry #{:?}\n", i);
                    return Ok(DentryWrapper::new(dentry));
                }
            }
            // if we get here, all dentries are in use
            Err(Error::ENOSPC)
        }

        fn initialize_dentry(self, ino: InodeNum, name: &str) -> DentryWrapper<'a, Flushed, Init> {
            pr_info!("initializing dentry {:?} for inode {:?}\n", name, ino);
            self.dentry.set_up(ino, name);
            DentryWrapper::new(self.dentry)
        }

        // the two inode wrappers are only used to enforce dependencies
        pub(crate) fn initialize_mkdir_dentry(
            self,
            ino: InodeNum,
            name: &str,
            _: &hayleyfs_inode::InodeWrapper<'a, Clean, Valid>,
            _: &hayleyfs_inode::InodeWrapper<'a, Clean, Link>,
        ) -> DentryWrapper<'a, Flushed, Init> {
            self.dentry.set_up(ino, name);
            DentryWrapper::new(self.dentry)
        }
    }

    pub(crate) fn initialize_self_and_parent_dentries<'a>(
        self_dentry: DentryWrapper<'a, Clean, Alloc>,
        self_ino: InodeNum,
        parent_dentry: DentryWrapper<'a, Clean, Alloc>,
        parent_ino: InodeNum,
    ) -> (
        DentryWrapper<'a, Flushed, Init>,
        DentryWrapper<'a, Flushed, Init>,
    ) {
        let self_dentry = self_dentry.initialize_dentry(self_ino, ".");
        let parent_dentry = parent_dentry.initialize_dentry(parent_ino, "..");
        (self_dentry, parent_dentry)
    }

    impl<'a, Op> DentryWrapper<'a, Flushed, Op> {
        pub(crate) unsafe fn fence_unsafe(self) -> DentryWrapper<'a, Clean, Op> {
            DentryWrapper::new(self.dentry)
        }
    }
}

// TODO: use a better way to handle these slices so things don't get weird
// when there are different lengths
// there has to be a nicer way to handle these strings in general
pub(crate) fn compare_dentry_name(name1: &[u8], name2: &[u8]) -> bool {
    let (min_len, longer_name) = if name1.len() > name2.len() {
        (name2.len(), name1)
    } else {
        (name1.len(), name2)
    };
    for i in 0..MAX_FILENAME_LEN {
        if i < min_len {
            if name1[i] != name2[i] {
                return false;
            }
        } else if longer_name[i] != 0 {
            return false;
        }
    }
    true
}
