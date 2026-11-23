/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include <stdint.h>

/* `tally` is static here and static in ../beta/dup.c. Both units are named
   dup.c, so their *basenames* are identical and only the directory tells
   them apart -- the case that a basename-only comparison collapses (#36). */
static int32_t tally(int32_t v) { return v + 1; }

int32_t alpha_tally(int32_t v) { return tally(v); }
