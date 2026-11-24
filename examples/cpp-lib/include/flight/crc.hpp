// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#pragma once
#include <cstdint>
namespace flight {
/// @satisfies LLR-CRC-030
std::uint32_t crc_block(const std::uint8_t *buf, std::uint32_t len);
}  // namespace flight
