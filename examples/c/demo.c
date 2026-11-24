// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

/* butterfly demo fixture: exercises decisions, MC/DC, switch, loops, helpers. */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

/* @satisfies LLR-ALT-001 */
int32_t clamp_altitude(int32_t alt, int32_t lo, int32_t hi) {
    if (alt < lo) {
        return lo;
    }
    if (alt > hi) {
        return hi;
    }
    return alt;
}

/* @satisfies LLR-MODE-002 */
int mode_permits_engage(int gear_down, int speed_ok, int wow) {
    /* MC/DC decision with three conditions */
    if ((gear_down && speed_ok) || wow) {
        return 1;
    }
    return 0;
}

/* @satisfies LLR-DISP-003 */
const char *phase_name(int phase) {
    switch (phase) {
    case 0: return "TAXI";
    case 1: return "TAKEOFF";
    case 2: return "CLIMB";
    case 3: return "CRUISE";
    case 4: return "DESCENT";
    case 5: return "LANDING";
    default: return "UNKNOWN";
    }
}

/* @satisfies LLR-CRC-004 */
uint32_t checksum(const uint8_t *buf, uint32_t len) {
    uint32_t acc = 0;
    for (uint32_t i = 0; i < len; i++) {
        acc = (acc << 3) ^ buf[i];
        if (acc & 0x80000000u) {
            acc ^= 0x04C11DB7u;   /* @satisfies LLR-CRC-004.1 polynomial fold */
        }
    }
    return acc;
}

/* @satisfies LLR-DIV-005 */
int32_t average(const int32_t *v, int32_t n) {
    int64_t sum = 0;
    for (int32_t i = 0; i < n; i++) {
        sum += v[i];
    }
    if (n == 0) {
        return 0;
    }
    return (int32_t)(sum / n);   /* division helper on some targets */
}

typedef int32_t (*op_fn)(int32_t);
static int32_t op_double(int32_t x) { return x * 2; }
static int32_t op_negate(int32_t x) { return -x; }
static const op_fn OPS[] = { op_double, op_negate };

/* @satisfies LLR-OPS-006 : dispatch through a function-pointer table */
int32_t apply_op(int idx, int32_t x) {
    return OPS[idx](x);
}

/* never called: candidate dead code */
int32_t legacy_scale(int32_t x) {
    return x * 3 / 2;
}

int main(void) {
    int32_t alts[] = { -100, 500, 90000 };
    for (int i = 0; i < 3; i++) {
        printf("%d\n", clamp_altitude(alts[i], 0, 60000));
    }
    printf("%d\n", mode_permits_engage(1, 1, 0));
    printf("%d\n", mode_permits_engage(0, 1, 0));
    printf("%d\n", mode_permits_engage(0, 0, 1));
    printf("%s %s %s\n", phase_name(0), phase_name(3), phase_name(9));
    const uint8_t msg[] = "ARINC653";
    printf("%08x\n", checksum(msg, (uint32_t)strlen((const char *)msg)));
    printf("%d\n", average(alts, 3));
    printf("%d\n", apply_op(0, 21));   /* only op_double is ever reached */
    return 0;
}
