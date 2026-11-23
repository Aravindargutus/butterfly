// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//! Leaf helpers. Nothing in here calls another module.

/// file-private helper: not part of the module's interface
fn abs_i32(x: i32) -> i32 {
    if x < 0 { -x } else { x }
}

/// @satisfies LLR-UTIL-001
pub fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        return lo;
    }
    if v > hi {
        return hi;
    }
    v
}

/// @satisfies LLR-UTIL-002
pub fn saturating_add(a: i32, b: i32, limit: i32) -> i32 {
    let sum = a.wrapping_add(b);
    if abs_i32(sum) > limit {
        return if sum < 0 { -limit } else { limit };   // @satisfies LLR-UTIL-002.1 saturation
    }
    sum
}

/// Public, but nothing in this binary calls it. rustc drops it entirely, so
/// it has no object code rather than being a dead-code candidate -- see the
/// README for why that distinction matters.
pub fn unused_scale(x: i32) -> i32 {
    x * 3 / 2
}
