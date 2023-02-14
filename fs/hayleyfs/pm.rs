use core::{arch::asm, ffi::c_void};
use kernel::bindings;
use kernel::prelude::*;

#[allow(dead_code)]
pub(crate) const CACHELINE_BYTE_SHIFT: usize = 6;

/// Taken from Corundum
/// Flushes cache line back to memory
#[allow(dead_code)]
pub(crate) fn flush_buffer<T: ?Sized>(ptr: *const T, len: usize, fence: bool) {
    // #[cfg(not(feature = "no_persist"))]
    {
        let ptr = ptr as *const u8 as *mut u8;
        let mut start = ptr as usize;
        start = (start >> CACHELINE_BYTE_SHIFT) << CACHELINE_BYTE_SHIFT; // TODO: confirm
        let end = start + len;
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
#[allow(dead_code)]
pub(crate) fn sfence() {
    unsafe {
        asm!("sfence");
    }
}

#[allow(dead_code)]
pub(crate) unsafe fn memcpy_nt<T: ?Sized>(
    src: *const T,
    dst: *mut T,
    size: usize,
    fence: bool,
) -> Result<u64> {
    let src = src as *const c_void as *mut c_void;
    let dst = dst as *mut c_void;
    let size: u64 = size.try_into()?;

    let ret = unsafe { bindings::copy_from_user_inatomic_nocache(src, dst, size) };

    // copy_from_user_inatomic_nocache uses __copy_user_nocache
    // (https://elixir.bootlin.com/linux/latest/source/arch/x86/lib/copy_user_64.S#L272)
    // which uses non-temporal stores EXCEPT for non-8-byte-aligned sections at the beginning
    // and end of the buffer. We need to flush the edge cache lines to make sure these
    // regions are persistent.
    unsafe { flush_edge_cachelines(dst, size.try_into()?)? };

    if fence {
        sfence();
    }

    Ok(ret)
}

pub(crate) unsafe fn flush_edge_cachelines(ptr: *mut c_void, size: u64) -> Result<()> {
    let raw_ptr = ptr as u64;
    if raw_ptr & 0x7 != 0 {
        flush_buffer(ptr, 1, false);
    }
    if (raw_ptr + size) & 0x7 != 0 {
        unsafe { flush_buffer(ptr.offset(size.try_into()?), 1, false) };
    }

    Ok(())
}

/// Adapted from PMFS. Uses non-temporal stores to memset a region.
///
/// # Safety
/// Assumes length and dst+length to be 4-byte aligned. Truncates the region to the
/// last 4-byte boundary. dst does not have to be 4-byte aligned. dst must be the only
/// active pointer to the specified region of memory.
pub(crate) unsafe fn memset_nt(dst: *mut c_void, dword: u32, size: usize, fence: bool) {
    let qword: u64 = ((dword as u64) << 32) | dword as u64;

    unsafe {
        asm!(
            "movl %edx, %ecx",
            "andl $63, %edx",
            "shrl $6, %ecx",
            "jz 9f",
            "1:",
            "movnti %rax, (%rdi)",
            "2:",
            "movnti %rax, 1*8(%rdi)",
            "3:",
            "movnti %rax, 2*8(%rdi)",
            "4:",
            "movnti %rax, 3*8(%rdi)",
            "5:",
            "movnti %rax, 4*8(%rdi)",
            "6:",
            "movnti %rax, 5*8(%rdi)",
            "7:",
            "movnti %rax, 6*8(%rdi)",
            "8:",
            "movnti %rax, 7*8(%rdi)",
            "leaq 64(%rdi), %rdi",
            "decl %ecx",
            "jnz 1b",
            "9:",
            "movl %edx, %ecx",
            "andl $7, %edx",
            "shrl $3, %ecx",
            "jz 11f",
            "10:",
            "movnti %rax, (%rdi)",
            "leaq 8(%rdi), %rdi",
            "decl %ecx",
            "jnz 10b",
            "11:",
            "movl %edx, %ecx",
            "shrl $2, %ecx",
            "jz 12f",
            "movnti %eax, (%rdi)",
            "12:",
            in("edi") dst,
            in("eax") qword,
            in("edx") size,
            lateout("edi") _,
            lateout("edx") _,
            out("rcx") _,
            options(att_syntax)
        );
    }

    if fence {
        sfence();
    }
}
