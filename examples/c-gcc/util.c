/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj
 *
 * A second translation unit, on purpose. A gcov export lists one entry per
 * file, and a reader that indexed a function's filename by its position in
 * that list resolved every function after the first file to an empty name --
 * which drops it from the evidence silently. One source file could never
 * have shown that, and a real project always has more than one.
 */

/* @satisfies LLR-GCC-004 : a rate is inside its bound in either direction */
int rate_ok(int rate, int max_rate) {
    if (rate < 0 && -rate > max_rate) return 0;
    if (rate > max_rate) return 0;
    return 1;
}
