/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj
 *
 * One binary, one test per invocation. That is what makes per-test coverage
 * possible: each run writes its own profile, the merge of them is the
 * aggregate, and butterfly can attribute the aggregate back to the runs.
 * A test framework does the same thing with more ceremony. */
#include <stdio.h>
#include <string.h>
#include "limits.h"

static int t_clamp(void) {
    if (clamp_alt(50, 0, 100) != 50) return 1;
    if (clamp_alt(-5, 0, 100) != 0) return 1;   /* the low edge */
    return 0;
}

static int t_ceiling(void) {
    if (clamp_alt(4000, 0, 100) != 100) return 1;  /* the high edge */
    return 0;
}

static int t_faults(void) {
    if (strcmp(fault_name(0u), "none") != 0) return 1;
    if (strcmp(fault_name(2u), "rate") != 0) return 1;
    return 0;
}

/* reaches only LLR-LIM-005's line */
static int t_band_low(void) {
    if (band_of(-3) != -1) return 1;
    return 0;
}

/* reaches only LLR-LIM-006's line */
static int t_band_high(void) {
    if (band_of(5000) != 1) return 1;
    return 0;
}

/* bank_angle_ok's decision is `bank_deg > 60 && turn_rate > 3` -- two
   independence pairs, each proved here by two *different* tests, so #48
   can name them instead of "vectors 1 and 2". */
static int t_bank_shallow(void) {
    /* bank_deg>60 false: short-circuits, turn_rate masked. Together with
       t_bank_steep_and_turning, proves the first condition's pair. */
    if (bank_angle_ok(30, 10) != 1) return 1;
    return 0;
}
static int t_bank_steep_and_turning(void) {
    /* both true. Proves the first pair with t_bank_shallow, and the second
       pair with t_bank_steep_no_turn. */
    if (bank_angle_ok(70, 5) != 0) return 1;
    return 0;
}
static int t_bank_steep_no_turn(void) {
    /* bank_deg>60 true, turn_rate>3 false: proves the second pair together
       with t_bank_steep_and_turning. */
    if (bank_angle_ok(70, 2) != 1) return 1;
    return 0;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s <t_clamp|t_ceiling|t_faults|t_band_low|t_band_high>\n", argv[0]);
        return 2;
    }
    if (strcmp(argv[1], "t_clamp") == 0) return t_clamp();
    if (strcmp(argv[1], "t_ceiling") == 0) return t_ceiling();
    if (strcmp(argv[1], "t_faults") == 0) return t_faults();
    if (strcmp(argv[1], "t_band_low") == 0) return t_band_low();
    if (strcmp(argv[1], "t_band_high") == 0) return t_band_high();
    if (strcmp(argv[1], "t_bank_shallow") == 0) return t_bank_shallow();
    if (strcmp(argv[1], "t_bank_steep_and_turning") == 0) return t_bank_steep_and_turning();
    if (strcmp(argv[1], "t_bank_steep_no_turn") == 0) return t_bank_steep_no_turn();
    fprintf(stderr, "no such test: %s\n", argv[1]);
    return 2;
}
