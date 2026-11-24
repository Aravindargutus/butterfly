// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The code under test: an altitude monitor small enough to read in one
//! sitting. Nothing here knows whether it runs on the host or on the target;
//! that is the point -- the same functions are instrumented on the host and
//! traced on the target.

/// Altitude band reported to the display.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Ground,
    Low,
    Cruise,
    /// Above the certified ceiling. The campaign never flies this high, so
    /// the outcome that produces it is never taken -- on either side.
    Over,
}

/// @satisfies LLR-ALT-001
/// Both decisions go both ways in the campaign.
#[inline(never)]
pub fn clamp(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        return lo;
    }
    if v > hi {
        return hi;
    }
    v
}

/// @satisfies LLR-ALT-002
/// `alt > 45_000` is never true in the campaign: one branch outcome that no
/// run takes.
#[inline(never)]
pub fn band(alt: i32) -> Band {
    if alt > 45_000 {
        return Band::Over;
    }
    if alt > 10_000 {
        return Band::Cruise;
    }
    if alt > 0 {
        return Band::Low;
    }
    Band::Ground
}

/// @satisfies LLR-SUM-003
/// A loop with an indexed read and a checked add. At `-Copt-level=0` rustc
/// keeps the bounds check and the overflow check, each with a panic path
/// that shares its source line with the arithmetic it guards.
#[inline(never)]
pub fn sum(samples: &[i32], n: usize) -> i32 {
    let mut acc = 0;
    let mut i = 0;
    while i < n {
        acc = acc + samples[i];
        i += 1;
    }
    acc
}

/// @satisfies LLR-CMD-004
#[inline(never)]
pub fn cmd_climb(alt: i32) -> i32 {
    clamp(alt + 1_000, 0, 40_000)
}

/// @satisfies LLR-CMD-005
#[inline(never)]
pub fn cmd_descend(alt: i32) -> i32 {
    clamp(alt - 1_000, 0, 40_000)
}

/// @satisfies LLR-CMD-006
/// In the table, so in the image -- and never dispatched. Static analysis
/// can only say "address taken, may be called"; the trace says it was not.
#[inline(never)]
pub fn cmd_hold(alt: i32) -> i32 {
    alt
}

/// The command table: three candidates for one indirect call.
pub static COMMANDS: [fn(i32) -> i32; 3] = [cmd_climb, cmd_descend, cmd_hold];

/// @satisfies LLR-CMD-007
#[inline(never)]
pub fn dispatch(cmd: usize, alt: i32) -> i32 {
    if cmd >= COMMANDS.len() {
        return alt;
    }
    COMMANDS[cmd](alt)
}
