// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#pragma once
#include <cstdint>

namespace flight {

/// @satisfies LLR-MODE-020
bool mode_permits_engage(bool gear_down, bool speed_ok, bool wow);
/// @satisfies LLR-MODE-021
const char *mode_phase_name(std::uint8_t phase);
/// @satisfies LLR-MODE-022
void mode_advance(std::uint8_t phase);

}  // namespace flight
