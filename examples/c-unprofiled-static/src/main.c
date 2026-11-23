/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include <stdint.h>
#include <stdio.h>

int32_t scale(int32_t v);
int32_t covered_use(int32_t v);
int32_t beta(int32_t v);

int main(void) {
    printf("%d %d %d\n", scale(4), covered_use(10), beta(7));
    return 0;
}
