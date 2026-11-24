/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include "util.h"

/* file-local helper: internal linkage, so it is not part of the interface */
static int32_t abs_i32(int32_t x) {
    return x < 0 ? -x : x;
}

/* @satisfies LLR-UTIL-001 */
int32_t clamp_i32(int32_t v, int32_t lo, int32_t hi) {
    if (v < lo) {
        return lo;
    }
    if (v > hi) {
        return hi;
    }
    return v;
}

/* @satisfies LLR-UTIL-002 */
int32_t saturating_add(int32_t a, int32_t b, int32_t limit) {
    int32_t sum = a + b;
    if (abs_i32(sum) > limit) {
        return sum < 0 ? -limit : limit;   /* @satisfies LLR-UTIL-002.1 saturation */
    }
    return sum;
}

int32_t unused_scale(int32_t x) {
    return x * 3 / 2;
}
