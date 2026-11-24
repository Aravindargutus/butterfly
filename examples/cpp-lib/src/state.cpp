// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#include "flight/state.hpp"
#include "flight/util.hpp"

namespace flight {

FlightState g_state{};

/// @satisfies LLR-STATE-010
void state_reset() {
    g_state.altitude = 0;
    g_state.phase = 0;
    g_state.faults = 0;
}

/// @satisfies LLR-STATE-011
void state_set_altitude(std::int32_t raw) {
    const std::int32_t a = clamp_i32(raw, kAltMin, kAltMax);   // cross-module call
    if (a != raw) {
        state_raise_fault(kFaultAltClamped);
    }
    g_state.altitude = a;
}

/// @satisfies LLR-STATE-012
std::int32_t state_altitude() { return g_state.altitude; }

/// @satisfies LLR-STATE-013
void state_raise_fault(std::uint32_t bit) { g_state.faults |= bit; }

}  // namespace flight
