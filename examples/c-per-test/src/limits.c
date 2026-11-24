/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include "limits.h"

/* @satisfies LLR-LIM-001 : altitude is held inside the envelope */
int32_t clamp_alt(int32_t v, int32_t lo, int32_t hi) {
    if (v < lo) return lo;
    if (v > hi) return hi;
    return v;
}

/* @satisfies LLR-LIM-002 : a climb rate is accepted only inside its bound */
int32_t rate_ok(int32_t rate, int32_t max_rate) {
    if (rate < 0 && -rate > max_rate) return 0;
    if (rate > max_rate) return 0;
    return 1;
}

/* Two requirements on distinct lines of one function, each reached by a
   different test. A join that scoped a line-level tag to the whole function
   would report both tests against both requirements, and both fully
   covered -- so this is where that error shows itself. */
int32_t band_of(int32_t v) {
    if (v < 0) return -1;      /* @satisfies LLR-LIM-005 */
    if (v > 1000) return 1;    /* @satisfies LLR-LIM-006 */
    return 0;
}

/* No requirement tags this one on purpose (#48): a compound decision whose
   independence pairs are split across separate tests, to show the MC/DC
   panel naming the tests that proved each pair rather than two anonymous
   vector numbers. */
int32_t bank_angle_ok(int32_t bank_deg, int32_t turn_rate) {
    if (bank_deg > 60 && turn_rate > 3) return 0;
    return 1;
}

/* @satisfies LLR-LIM-003 : every fault code has a name */
const char *fault_name(uint32_t code) {
    switch (code) {
        case 0u: return "none";
        case 1u: return "alt";
        case 2u: return "rate";
        default: return "unknown";
    }
}
