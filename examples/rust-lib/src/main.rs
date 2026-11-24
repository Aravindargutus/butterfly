// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//! Multi-module Rust library plus a driver, mirroring the C and C++ library
//! examples: util -> state -> mode -> sched, with a shared global and a
//! dispatch table. Build with examples/rust-lib/build.sh.

mod crc;
mod mode;
mod sched;
mod state;
mod util;

use core::sync::atomic::Ordering;

fn main() {
    state::reset();

    println!("alt {}", sched::run(0, 15000));
    println!("alt {}", sched::run(0, 50000)); // clamps -> raises a fault
    println!("alt {}", sched::run(1, 0));
    println!("bad {}", sched::run(9, 0)); // out-of-range index

    println!(
        "{} {} {}",
        mode::permits_engage(true, true, false),
        mode::permits_engage(false, true, false),
        mode::permits_engage(false, false, true)
    );
    println!(
        "{} {}",
        mode::phase_name(state::PHASE.load(Ordering::Relaxed)),
        mode::phase_name(9)
    );

    println!("{:08x}", crc::block(b"ARINC653"));
    println!(
        "faults {}  dispatches {}",
        state::FAULTS.load(Ordering::Relaxed),
        sched::dispatches()
    );
}
