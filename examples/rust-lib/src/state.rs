// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//! The shared state. Every other module reads or writes it, which makes this
//! module the subject of the data-coupling analysis (DO-178C 6.4.4.2.c,
//! table A-7 objective 8). Atomics rather than `static mut`: they are real
//! globals in the image, so the Data tab can attribute reads and writes.

use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU8, Ordering};

use crate::util::clamp_i32;

pub const ALT_MIN: i32 = 0;
pub const ALT_MAX: i32 = 60000;
pub const FAULT_ALT_CLAMPED: u32 = 0x1;
pub const FAULT_BAD_TASK: u32 = 0x2;

pub static ALTITUDE: AtomicI32 = AtomicI32::new(0);
pub static PHASE: AtomicU8 = AtomicU8::new(0);
pub static FAULTS: AtomicU32 = AtomicU32::new(0);

/// @satisfies LLR-STATE-010
pub fn reset() {
    ALTITUDE.store(0, Ordering::Relaxed);
    PHASE.store(0, Ordering::Relaxed);
    FAULTS.store(0, Ordering::Relaxed);
}

/// @satisfies LLR-STATE-011
pub fn set_altitude(raw: i32) {
    let a = clamp_i32(raw, ALT_MIN, ALT_MAX);   // cross-module call into util
    if a != raw {
        raise_fault(FAULT_ALT_CLAMPED);
    }
    ALTITUDE.store(a, Ordering::Relaxed);
}

/// @satisfies LLR-STATE-012
pub fn altitude() -> i32 {
    ALTITUDE.load(Ordering::Relaxed)
}

/// @satisfies LLR-STATE-013
pub fn raise_fault(bit: u32) {
    FAULTS.fetch_or(bit, Ordering::Relaxed);
}
