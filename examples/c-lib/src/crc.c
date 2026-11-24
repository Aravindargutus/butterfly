/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include "crc.h"

/* @satisfies LLR-CRC-030 */
uint32_t crc_block(const uint8_t *buf, uint32_t len) {
    uint32_t acc = 0;
    for (uint32_t i = 0; i < len; i++) {
        acc = (acc << 3) ^ buf[i];
        if (acc & 0x80000000u) {
            acc ^= 0x04C11DB7u;   /* @satisfies LLR-CRC-030.1 polynomial fold */
        }
    }
    return acc;
}
