/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#ifndef FLIGHT_CRC_H
#define FLIGHT_CRC_H
#include <stdint.h>

/* @satisfies LLR-CRC-030 */
uint32_t crc_block(const uint8_t *buf, uint32_t len);

#endif
