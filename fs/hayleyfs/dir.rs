use crate::defs::*;
use crate::typestate::*;
use core::marker::PhantomData;

#[repr(C)]
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
}

#[allow(dead_code)]
pub(crate) struct DentryWrapper<'a, State, Op> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    dentry: &'a HayleyFsDentry,
}

impl<'a> DentryWrapper<'a, Clean, Free> {
    /// Safety
    /// The provided dentry must be free (completely zeroed out).
    pub(crate) unsafe fn wrap_free_dentry(dentry: &'a HayleyFsDentry) -> Self {
        Self {
            state: PhantomData,
            op: PhantomData,
            dentry: dentry,
        }
    }
}
