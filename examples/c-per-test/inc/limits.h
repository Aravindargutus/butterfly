/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#ifndef LIMITS_H
#define LIMITS_H
#include <stdint.h>

/* @satisfies LLR-LIM-001 */
int32_t clamp_alt(int32_t v, int32_t lo, int32_t hi);

/* @satisfies LLR-LIM-002 */
int32_t rate_ok(int32_t rate, int32_t max_rate);

/* @satisfies LLR-LIM-003 */
const char *fault_name(uint32_t code);

int32_t band_of(int32_t v);

/* No requirement tags it: pure per-test coverage demonstration (#48), kept
   apart from the LLR-LIM-002 scaffold above, whose whole point is a
   traceability claim that stays wrong (#57) -- a function this file needs
   to stay genuinely untested. This one may be tested as thoroughly as the
   demonstration wants. */
int32_t bank_angle_ok(int32_t bank_deg, int32_t turn_rate);

#endif
