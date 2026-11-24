/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include <stdint.h>

/* the other `tally`: same name, same file basename, different directory */
static int32_t tally(int32_t v) { return v - 1; }

int32_t beta_tally(int32_t v) { return tally(v); }
