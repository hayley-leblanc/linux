#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]
#![allow(unused_imports)]

use crate::defs::*;
use kernel::prelude::*;

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

        pr_info!("start {:#X}, end {:#X}, len {:?}\n", start, end, len);

        // TODO: properly check architecture and choose correct cache line flush instruction
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
        pr_info!("fence\n");
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
