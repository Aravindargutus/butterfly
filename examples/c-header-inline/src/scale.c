/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include <stdint.h>

/* EXTERNAL linkage, and the same bare name as aux.c's static. llvm-cov names
   this plain `scale`, with no unit qualifier at all. */
int32_t scale(int32_t v) { return v * 3; }
