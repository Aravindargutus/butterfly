/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#ifndef FLIGHT_STATE_H
#define FLIGHT_STATE_H
#include <stdint.h>

#define ALT_MIN        0
#define ALT_MAX    60000
#define FAULT_ALT_CLAMPED  0x1u

/* The shared state every other module reads or writes. This is the object of
 * the data-coupling analysis (DO-178C 6.4.4.2.c, table A-7 objective 8): the
 * Data tab shows, per function, which of these fields it touches and how. */
typedef struct {
    int32_t  altitude;
    uint8_t  phase;
    uint32_t faults;
} flight_state_t;

extern flight_state_t g_state;

/* @satisfies LLR-STATE-010 */
void state_reset(void);

/* @satisfies LLR-STATE-011 */
void state_set_altitude(int32_t raw);

/* @satisfies LLR-STATE-012 */
int32_t state_altitude(void);

/* @satisfies LLR-STATE-013 */
void state_raise_fault(uint32_t bit);

#endif
