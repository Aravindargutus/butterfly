// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! butterfly Rust fixture: decisions, MC/DC, match, loops, helper calls.
#![allow(dead_code)]

/// @satisfies LLR-ALT-001
#[inline(never)]
pub fn clamp_altitude(alt: i32, lo: i32, hi: i32) -> i32 {
    if alt < lo {
        return lo;
    }
    if alt > hi {
        return hi;
    }
    alt
}

/// @satisfies LLR-MODE-002
#[inline(never)]
pub fn mode_permits_engage(gear_down: bool, speed_ok: bool, wow: bool) -> bool {
    // MC/DC decision with three conditions
    if (gear_down && speed_ok) || wow {
        return true;
    }
    false
}

/// @satisfies LLR-DISP-003
#[inline(never)]
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

/// @satisfies LLR-CRC-004
#[inline(never)]
pub fn checksum(buf: &[u8]) -> u32 {
    let mut acc: u32 = 0;
    for &b in buf {
        acc = (acc << 3) ^ (b as u32);
        if acc & 0x8000_0000 != 0 {
            acc ^= 0x04C1_1DB7;
        }
    }
    acc
}

/// @satisfies LLR-DIV-005
#[inline(never)]
pub fn average(v: &[i32]) -> i32 {
    if v.is_empty() {
        return 0;
    }
    let sum: i64 = v.iter().map(|&x| x as i64).sum();
    (sum / v.len() as i64) as i32
}

/// never called: candidate dead code
#[inline(never)]
pub fn legacy_scale(x: i32) -> i32 {
    x * 3 / 2
}

fn main() {
    let alts = [-100, 500, 90_000];
    for a in alts {
        println!("{}", clamp_altitude(a, 0, 60_000));
    }
    println!("{}", mode_permits_engage(true, true, false));
    println!("{}", mode_permits_engage(false, true, false));
    println!("{}", mode_permits_engage(false, false, true));
    println!("{} {} {}", phase_name(0), phase_name(3), phase_name(9));
    println!("{:08x}", checksum(b"ARINC653"));
    println!("{}", average(&alts));
}
