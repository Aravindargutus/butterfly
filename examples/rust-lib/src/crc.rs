// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//! A leaf module: a loop with an inner decision, and nothing else.

/// @satisfies LLR-CRC-030
pub fn block(buf: &[u8]) -> u32 {
    let mut acc: u32 = 0;
    for &b in buf {
        acc = (acc << 3) ^ (b as u32);
        if acc & 0x8000_0000 != 0 {
            acc ^= 0x04C1_1DB7;   // @satisfies LLR-CRC-030.1 polynomial fold
        }
    }
    acc
}
