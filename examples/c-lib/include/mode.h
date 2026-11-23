/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#ifndef FLIGHT_MODE_H
#define FLIGHT_MODE_H
#include <stdint.h>

/* @satisfies LLR-MODE-020 */
int mode_permits_engage(int gear_down, int speed_ok, int wow);

/* @satisfies LLR-MODE-021 */
const char *mode_phase_name(uint8_t phase);

/* @satisfies LLR-MODE-022 */
void mode_advance(uint8_t phase);

#endif
