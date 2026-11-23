// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//! Mode logic: the MC/DC decision and the phase table, in their own module.

use core::sync::atomic::Ordering;

use crate::state::PHASE;

/// @satisfies LLR-MODE-020
pub fn permits_engage(gear_down: bool, speed_ok: bool, wow: bool) -> bool {
    // decision with three conditions
    if (gear_down && speed_ok) || wow {
        return true;
    }
    false
}

/// @satisfies LLR-MODE-021
pub fn phase_name(phase: u8) -> &'static str {
    match phase {
        0 => "TAXI",
        1 => "TAKEOFF",
        2 => "CLIMB",
        3 => "CRUISE",
        4 => "DESCENT",
        5 => "LANDING",
        _ => "UNKNOWN",
    }
}

/// @satisfies LLR-MODE-022
pub fn advance(phase: u8) {
    PHASE.store(phase, Ordering::Relaxed);   // writes another module's global
}
