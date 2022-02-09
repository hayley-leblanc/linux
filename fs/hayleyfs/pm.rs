#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]

// use crate::defs::*;
use kernel::prelude::*;

// TODO: figure out how to get this from super_rs so
// you don't have to declare it here
pub(crate) const __LOG_PREFIX: &[u8] = b"hayleyfs\0";

/// Taken from Corundum
/// Flushes cache line back to memory
pub(crate) fn clflush<T: ?Sized>(ptr: *const T, len: usize, fence: bool) {
    #[cfg(not(feature = "no_persist"))]
    {
        let ptr = ptr as *const u8 as *mut u8;
        let mut start = ptr as usize;
        // start = (start >> 9) << 9;
        start = (start >> 6) << 6; // TODO: i think this properly aligns it
        let end = start + len;

        // #[cfg(feature = "stat_print_flushes")]
        pr_info!("start {:#X}, end {:#X}\n", start, end);

        while start < end {
            unsafe {
                // #[cfg(not(any(feature = "use_clflushopt", feature = "use_clwb")))]
                // {

                //     asm!("clflush [{}]", in(reg) (start as *const u8), options(nostack));
                // }
                // #[cfg(all(feature = "use_clflushopt", not(feature = "use_clwb")))]
                // {
                //     asm!("clflushopt [{}]", in(reg) (start as *const u8), options(nostack));
                // }
                // #[cfg(all(feature = "use_clwb", not(feature = "use_clflushopt")))]
                // {
                asm!("clwb [{}]", in(reg) (start as *const u8), options(nostack));
                // }
                // #[cfg(all(feature = "use_clwb", feature = "use_clflushopt"))]
                // {
                //     compile_error!("Please Select only one from clflushopt and clwb")
                // }
            }
            start += 64;
        }
    }
    if fence {
        sfence();
    }
}

/// Store fence (from Corundum)
// #[inline(always)]
pub(crate) fn sfence() {
    #[cfg(any(feature = "use_clwb", feature = "use_clflushopt"))]
    unsafe {
        _mm_sfence();
    }
}
