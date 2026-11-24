/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include "geom.h"

/* The span in the demo run is far above GEOM_MAX_SPAN, so THIS unit's copy
   of geom_clamp takes the `v > hi` branch and no other. main.c's copy takes
   `v < lo` and no other: the two copies have opposite coverage, which makes
   a swapped attribution visible. */
int32_t geom_span(point_t a, point_t b) {
    return geom_clamp(b.x - a.x, 0, GEOM_MAX_SPAN);
}
