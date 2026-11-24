// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#pragma once
#include <cstdint>

namespace flight {

constexpr std::int32_t  kAltMin = 0;
constexpr std::int32_t  kAltMax = 60000;
constexpr std::uint32_t kFaultAltClamped = 0x1u;
constexpr std::uint32_t kFaultBadTask    = 0x2u;

/// The shared state, and the subject of the data-coupling analysis
/// (DO-178C 6.4.4.2.c, table A-7 objective 8).
struct FlightState {
    std::int32_t  altitude;
    std::uint8_t  phase;
    std::uint32_t faults;
};

extern FlightState g_state;

/// @satisfies LLR-STATE-010
void state_reset();
/// @satisfies LLR-STATE-011
void state_set_altitude(std::int32_t raw);
/// @satisfies LLR-STATE-012
std::int32_t state_altitude();
/// @satisfies LLR-STATE-013
void state_raise_fault(std::uint32_t bit);

}  // namespace flight
