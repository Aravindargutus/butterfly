/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include <stdint.h>

/* INTERNAL linkage `scale`, instrumented: llvm-cov names it
   `covered.c:scale`. Two coverage functions now answer to the bare name
   `scale`, which is what keeps it out of the plain-name lookup. */
static int32_t scale(int32_t v) { return v / 2; }

int32_t covered_use(int32_t v) { return scale(v); }
