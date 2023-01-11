use crate::defs::*;
// use crate::typestate::*;
use core::marker::PhantomData;

#[repr(C)]
pub(crate) struct HayleyFsDentry {
    ino: InodeNum,
    name: [u8; MAX_FILENAME_LEN],
    rename_ptr: *mut HayleyFsDentry,
}

#[allow(dead_code)]
pub(crate) struct DentryWrapper<'a, State, Op> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    dentry: &'a HayleyFsDentry,
}
