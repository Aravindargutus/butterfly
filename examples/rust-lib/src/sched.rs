// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//! Dispatch through a table of function pointers. The tasks are private and
//! never called directly, so the Calls tab has to reach them through the
//! table -- they are not dead code.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::mode;
use crate::state::{self, ALT_MAX, FAULT_BAD_TASK};
use crate::util::saturating_add;

pub const TASK_COUNT: usize = 3;

static DISPATCHES: AtomicU32 = AtomicU32::new(0);

fn task_climb(step: i32) -> i32 {
    state::set_altitude(state::altitude() + step);   // -> state -> util
    mode::advance(2);
    state::altitude()
}

fn task_hold(_unused: i32) -> i32 {
    state::altitude()
}

fn task_trim(delta: i32) -> i32 {
    saturating_add(state::altitude(), delta, ALT_MAX)   // -> util
}

static TASKS: [fn(i32) -> i32; TASK_COUNT] = [task_climb, task_hold, task_trim];

/// @satisfies LLR-SCHED-040 : dispatch through the task table
pub fn run(idx: usize, arg: i32) -> i32 {
    if idx >= TASK_COUNT {
        state::raise_fault(FAULT_BAD_TASK);
        return -1;
    }
    DISPATCHES.fetch_add(1, Ordering::Relaxed);
    TASKS[idx](arg)   // indirect call through the table
}

/// @satisfies LLR-SCHED-041
pub fn dispatches() -> u32 {
    DISPATCHES.load(Ordering::Relaxed)
}
