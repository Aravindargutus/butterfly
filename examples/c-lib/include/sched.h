/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#ifndef FLIGHT_SCHED_H
#define FLIGHT_SCHED_H
#include <stdint.h>

#define SCHED_TASKS 3

/* @satisfies LLR-SCHED-040 : dispatch through a task table */
int32_t sched_run(int idx, int32_t arg);

/* @satisfies LLR-SCHED-041 */
uint32_t sched_dispatches(void);

#endif
