// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! butterfly measured-coverage fixture.
//!
//! One crate, built twice from the same sources:
//!
//! * for the HOST, instrumented, to get the source-level coverage export that
//!   projected coverage is derived from;
//! * for `aarch64-unknown-none-softfloat`, not instrumented, as the image
//!   under analysis -- run under QEMU with `-d in_asm,exec,nochain` to get
//!   the execution trace that measured coverage is derived from.
//!
//! Both run the same `campaign()`, which is what makes comparing the two
//! legitimate: where they differ, the difference is the compiler's, not the
//! test set's.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

mod monitor;

use monitor::Band;

/// The whole test campaign. Returns 0 when every expectation held, otherwise
/// the number of the first one that did not; that number becomes the exit
/// status on both sides, so a wrong answer on the target fails the build.
#[inline(never)]
pub fn campaign() -> i32 {
    // clamp: below, above, inside
    if monitor::clamp(-5, 0, 100) != 0 {
        return 1;
    }
    if monitor::clamp(500, 0, 100) != 100 {
        return 2;
    }
    if monitor::clamp(42, 0, 100) != 42 {
        return 3;
    }
    // band: Ground, Low, Cruise -- never Over
    if monitor::band(0) != Band::Ground {
        return 4;
    }
    if monitor::band(3_000) != Band::Low {
        return 5;
    }
    if monitor::band(35_000) != Band::Cruise {
        return 6;
    }
    // sum: a short loop, in range, no overflow
    let samples = [100, 200, 300, 400];
    if monitor::sum(&samples, 4) != 1_000 {
        return 7;
    }
    // dispatch: climb and descend, plus an out-of-range command; never hold
    let mut alt = 39_500;
    alt = monitor::dispatch(0, alt); // climb, clamped at the ceiling
    alt = monitor::dispatch(1, alt); // descend
    alt = monitor::dispatch(9, alt); // rejected
    if alt != 39_000 {
        return 8;
    }
    0
}

#[cfg(not(target_os = "none"))]
fn main() {
    std::process::exit(campaign());
}

// ---- bare-metal side -------------------------------------------------------

#[cfg(target_os = "none")]
core::arch::global_asm!(
    ".section .text._start,\"ax\"",
    ".global _start",
    "_start:",
    "  adrp x0, __stack_top",
    "  add  x0, x0, :lo12:__stack_top",
    "  mov  sp, x0",
    "  bl   kmain",
    "0: wfe",
    "  b    0b",
);

/// Arm semihosting SYS_EXIT (0x18). On AArch64 the parameter block carries
/// the reason and the exit status, so QEMU exits with `status`.
#[cfg(target_os = "none")]
fn semihost_exit(status: i32) -> ! {
    const SYS_EXIT: u64 = 0x18;
    const ADP_STOPPED_APPLICATION_EXIT: u64 = 0x20026;
    let block: [u64; 2] = [ADP_STOPPED_APPLICATION_EXIT, status as u64];
    unsafe {
        core::arch::asm!(
            "hlt #0xf000",
            in("x0") SYS_EXIT,
            in("x1") block.as_ptr(),
        );
    }
    loop {}
}

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn kmain() -> ! {
    semihost_exit(campaign())
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    semihost_exit(101)
}
