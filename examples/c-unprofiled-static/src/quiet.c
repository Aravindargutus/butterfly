/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include <stdint.h>

/* INTERNAL linkage `scale` with NO coverage record at all: the attribute
   keeps the instrumentation off this one function, so llvm-cov exports
   nothing for it while `beta`, in the same unit, is profiled normally.

   This is the third unit the resolver has to get right. The DWARF says the
   code at this address was emitted by quiet.c, and quiet.c has no covered
   function named `scale` -- so `beta`'s call below leaves the covered source
   set, however many other units happen to define a `scale` (#237). */
__attribute__((no_profile_instrument_function)) static int32_t scale(int32_t v) { return v - 1; }

int32_t beta(int32_t v) { return scale(v); }
