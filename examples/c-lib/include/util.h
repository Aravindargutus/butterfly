/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#ifndef FLIGHT_UTIL_H
#define FLIGHT_UTIL_H
#include <stdint.h>

/* @satisfies LLR-UTIL-001 */
int32_t clamp_i32(int32_t v, int32_t lo, int32_t hi);

/* @satisfies LLR-UTIL-002 */
int32_t saturating_add(int32_t a, int32_t b, int32_t limit);

/* Exported by the library but never called by this executable: a dead-code
 * candidate that only shows up once you analyse the linked image. */
int32_t unused_scale(int32_t x);

#endif
