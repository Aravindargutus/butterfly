/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#ifndef GEOM_H
#define GEOM_H
#include <stdint.h>

#define GEOM_MAX_SPAN 4096

typedef struct { int32_t x, y; } point_t;

/* A static inline function DEFINED IN A HEADER. Every translation unit that
   includes this compiles its own local copy, so the linked image carries one
   `geom_clamp` symbol per unit -- same name, different addresses, different
   coverage. That is the shape butterfly must keep apart (#36). */
static inline int32_t geom_clamp(int32_t v, int32_t lo, int32_t hi) {
    if (v < lo) return lo;
    if (v > hi) return hi;
    return v;
}

int32_t geom_span(point_t a, point_t b);
int32_t aux_halve(int32_t v);
int32_t alpha_tally(int32_t v);
int32_t beta_tally(int32_t v);
#endif
