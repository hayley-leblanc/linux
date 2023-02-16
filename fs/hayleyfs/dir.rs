use crate::defs::*;
use crate::h_inode::*;
use crate::pm::*;
use crate::typestate::*;
use crate::volatile::*;
use core::{ffi, marker::PhantomData, mem};
use kernel::prelude::*;

#[repr(C)]
#[derive(Debug)]
pub(crate) struct HayleyFsDentry {
    ino: InodeNum,
    name: [u8; MAX_FILENAME_LEN],
    rename_ptr: *mut HayleyFsDentry,
}

impl HayleyFsDentry {
    // Getters are not unsafe; only modifying HayleyFsDentry is unsafe
    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    pub(crate) fn is_rename_ptr_null(&self) -> bool {
        self.rename_ptr.is_null()
    }

    pub(crate) fn has_name(&self) -> bool {
        for char in self.name {
            if char != 0 {
                return true;
            }
        }
        false
    }

    pub(crate) fn get_name(&self) -> [u8; MAX_FILENAME_LEN] {
        self.name
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct DentryWrapper<'a, State, Op> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    dentry: &'a mut HayleyFsDentry,
}

impl<'a> DentryWrapper<'a, Clean, Free> {
    /// Safety
    /// The provided dentry must be free (completely zeroed out).
    pub(crate) unsafe fn wrap_free_dentry(dentry: &'a mut HayleyFsDentry) -> Self {
        Self {
            state: PhantomData,
            op: PhantomData,
            dentry: dentry,
        }
    }

    /// CStr are guaranteed to have a `NUL` byte at the end, so we don't have to check
    /// for that.
    pub(crate) fn set_name(self, name: &CStr) -> Result<DentryWrapper<'a, Dirty, Alloc>> {
        if name.len() > MAX_FILENAME_LEN {
            return Err(ENAMETOOLONG);
        }
        // copy only the number of bytes in the name
        let num_bytes = if name.len() < MAX_FILENAME_LEN {
            name.len()
        } else {
            MAX_FILENAME_LEN
        };
        let name = name.as_bytes_with_nul();
        self.dentry.name[..num_bytes].clone_from_slice(&name[..num_bytes]);

        Ok(DentryWrapper {
            state: PhantomData,
            op: PhantomData,
            dentry: self.dentry,
        })
    }
}

impl<'a> DentryWrapper<'a, Clean, Alloc> {
    // TODO: update alloy model to reflect dentry being in complete instead of init
    // after setting its ino
    pub(crate) fn set_file_ino<State: AddLink>(
        self,
        inode: InodeWrapper<'a, Clean, State, RegInode>,
    ) -> (
        DentryWrapper<'a, Dirty, Complete>,
        InodeWrapper<'a, Clean, Complete, RegInode>,
    ) {
        self.dentry.ino = inode.get_ino();
        (
            DentryWrapper {
                state: PhantomData,
                op: PhantomData,
                dentry: self.dentry,
            },
            InodeWrapper::new(inode),
        )
    }

    pub(crate) fn set_dir_ino(
        self,
        new_inode: InodeWrapper<'a, Clean, Alloc, DirInode>,
        parent_inode: InodeWrapper<'a, Clean, IncLink, DirInode>,
    ) -> (
        DentryWrapper<'a, Dirty, Complete>,
        InodeWrapper<'a, Clean, Complete, DirInode>,
        InodeWrapper<'a, Clean, Complete, DirInode>,
    ) {
        self.dentry.ino = new_inode.get_ino();
        (
            DentryWrapper {
                state: PhantomData,
                op: PhantomData,
                dentry: self.dentry,
            },
            InodeWrapper::new(new_inode),
            InodeWrapper::new(parent_inode),
        )
    }
}

impl<'a> DentryWrapper<'a, Clean, Complete> {
    // TODO: maybe should take completed inode as well? or ino dentry insert should
    // take the wrappers directly
    pub(crate) fn index(&self, parent: InodeNum, sb: &SbInfo) -> Result<()> {
        let dentry_info = DentryInfo::new(
            self.dentry.ino,
            self.dentry as *const _ as *const ffi::c_void,
            &self.dentry.name as *const _ as *const ffi::c_char,
        );
        sb.ino_dentry_map.insert(parent, dentry_info)
    }
}

impl<'a, Op> DentryWrapper<'a, Dirty, Op> {
    pub(crate) fn flush(self) -> DentryWrapper<'a, InFlight, Op> {
        flush_buffer(self.dentry, mem::size_of::<HayleyFsDentry>(), false);
        DentryWrapper {
            state: PhantomData,
            op: PhantomData,
            dentry: self.dentry,
        }
    }
}

impl<'a, Op> DentryWrapper<'a, InFlight, Op> {
    pub(crate) fn fence(self) -> DentryWrapper<'a, Clean, Op> {
        sfence();
        DentryWrapper {
            state: PhantomData,
            op: PhantomData,
            dentry: self.dentry,
        }
    }
}
