/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include <stdio.h>
#include "geom.h"

/* declared here, not in geom.h: aux.c must not see this, or its own
   `static scale` becomes a redeclaration and will not compile. */
int32_t scale(int32_t v);

/* THIS unit's copy of geom_clamp takes the `v < lo` branch and no other. */
int main(void) {
    point_t a = { 10, 0 }, b = { 90000, 0 };
    printf("span %d\n", geom_span(a, b));
    printf("clamped %d\n", geom_clamp(-7, 0, GEOM_MAX_SPAN));
    printf("halved %d  scaled %d\n", aux_halve(9), scale(9));
    printf("tally %d %d\n", alpha_tally(10), beta_tally(10));
    return 0;
}
