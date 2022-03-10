#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]

use crate::def::*;
use kernel::prelude::*;

/// Taken from Corundum
/// Flushes cache line back to memory
pub(crate) fn clwb<T: ?Sized>(ptr: *const T, len: usize, fence: bool) {
    // #[cfg(not(feature = "no_persist"))]
    {
        let ptr = ptr as *const u8 as *mut u8;
        let mut start = ptr as usize;
        // start = (start >> 9) << 9;
        start = (start >> CACHELINE_BYTE_SHIFT) << CACHELINE_BYTE_SHIFT; // TODO: i think this properly aligns it
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
        sfence();
    }
}

/// Store fence (from Corundum)
// #[inline(always)]
pub(crate) fn sfence() {
    pr_info!("fence\n");
    unsafe {
        asm!("sfence");
    }
}

// TODO: check if these work! we are probably going to end up with weird nested
// tuples with more than 2 objects being fenced. test that later.
#[macro_export]
macro_rules! fence_all {
    ($($args:tt),+) => { {
        sfence();
        fence_obj!($($args),+).flatten_tuple()
    }
    }
}

#[macro_export]
macro_rules! fence_obj {
    ($p_obj:ident) => {
        unsafe { $p_obj.fence_unsafe() }
    };

    ($p_obj0:ident, $($p_obj1:ident),+) => {
        (fence_obj!{$p_obj0}, fence_obj!{$($p_obj1),+})
    };
}
