/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include <stdint.h>

/* INTERNAL linkage. llvm-cov names this `aux.c:scale`, and the symbol table
   calls it plain `scale` -- the same bare name as the external `scale` in
   scale.c. The two claimants do not even share a name shape, so a rule that
   only looked at the `unit:name` qualifier would miss this pair (#36). */
static int32_t scale(int32_t v) { return v / 2; }

int32_t aux_halve(int32_t v) { return scale(v); }
