/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include "sched.h"
#include "mode.h"
#include "state.h"
#include "util.h"

typedef int32_t (*task_fn)(int32_t);

/* module-private counter: internal linkage, still a data-coupling item */
static uint32_t s_dispatches;

/* Each task reaches into another module, so the deepest stack chain and the
 * control-coupling graph both span several files. */
static int32_t task_climb(int32_t step) {
    state_set_altitude(state_altitude() + step);   /* -> state -> util */
    mode_advance(2);
    return state_altitude();
}

static int32_t task_hold(int32_t unused) {
    (void)unused;
    return state_altitude();
}

static int32_t task_trim(int32_t delta) {
    return saturating_add(state_altitude(), delta, ALT_MAX);   /* -> util */
}

/* Address-taken, never called directly: the Calls tab reaches these through
 * the table, so they are NOT dead code. */
static const task_fn TASKS[SCHED_TASKS] = { task_climb, task_hold, task_trim };

/* @satisfies LLR-SCHED-040 : dispatch through a task table */
int32_t sched_run(int idx, int32_t arg) {
    if (idx < 0 || idx >= SCHED_TASKS) {
        state_raise_fault(0x2u);
        return -1;
    }
    s_dispatches++;
    return TASKS[idx](arg);   /* indirect call: an indirect-branch gap */
}

/* @satisfies LLR-SCHED-041 */
uint32_t sched_dispatches(void) {
    return s_dispatches;
}
