/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include "state.h"
#include "util.h"

/* the one definition; every other module reaches it through state.h */
flight_state_t g_state;

/* @satisfies LLR-STATE-010 */
void state_reset(void) {
    g_state.altitude = 0;
    g_state.phase = 0;
    g_state.faults = 0;
}

/* @satisfies LLR-STATE-011 */
void state_set_altitude(int32_t raw) {
    int32_t a = clamp_i32(raw, ALT_MIN, ALT_MAX);   /* cross-module call into util */
    if (a != raw) {
        state_raise_fault(FAULT_ALT_CLAMPED);
    }
    g_state.altitude = a;
}

/* @satisfies LLR-STATE-012 */
int32_t state_altitude(void) {
    return g_state.altitude;
}

/* @satisfies LLR-STATE-013 */
void state_raise_fault(uint32_t bit) {
    g_state.faults |= bit;
}
