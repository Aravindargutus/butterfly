// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#include "flight/mode.hpp"
#include "flight/state.hpp"

namespace flight {

/// @satisfies LLR-MODE-020
bool mode_permits_engage(bool gear_down, bool speed_ok, bool wow) {
    // MC/DC decision with three conditions, in its own translation unit
    if ((gear_down && speed_ok) || wow) {
        return true;
    }
    return false;
}

/// @satisfies LLR-MODE-021
const char *mode_phase_name(std::uint8_t phase) {
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

/// @satisfies LLR-MODE-022
void mode_advance(std::uint8_t phase) {
    g_state.phase = phase;   // writes another module's global: data coupling
}

}  // namespace flight
