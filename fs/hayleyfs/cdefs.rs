use core::ffi;

#[allow(improper_ctypes)]
extern "C" {
    pub(crate) fn find_next_zero_bit_le_helper(
        addr: *const ffi::c_ulong,
        size: ffi::c_long,
        offset: ffi::c_long,
    ) -> ffi::c_ulong;
    pub(crate) fn test_and_set_bit_le_helper(
        nr: ffi::c_int,
        addr: *const ffi::c_void,
    ) -> ffi::c_int;
    pub(crate) fn set_bit_helper(nr: ffi::c_long, addr: *const ffi::c_void);
    pub(crate) fn test_and_clear_bit_le_helper(
        nr: ffi::c_int,
        addr: *const ffi::c_void,
    ) -> ffi::c_int;
}
