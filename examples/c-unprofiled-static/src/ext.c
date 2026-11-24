/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include <stdint.h>

/* EXTERNAL linkage `scale`, instrumented. llvm-cov names it plain `scale`,
   with no unit qualifier, so the bare string `scale` is in the covered
   source set's name list. It is the decoy: nothing else in this program is
   this function, but every `scale` in the image shares its name. */
int32_t scale(int32_t v) { return v * 3; }
