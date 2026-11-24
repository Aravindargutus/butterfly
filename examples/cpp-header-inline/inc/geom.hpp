// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#ifndef GEOM_HPP
#define GEOM_HPP
#include <cstdint>

namespace flight {

/// Defined in a header, so every translation unit that includes it compiles
/// its own copy -- the C++ counterpart of examples/c-header-inline (#36).
inline std::int32_t clamp_to(std::int32_t v, std::int32_t lo, std::int32_t hi) {
    if (v < lo) return lo;
    if (v > hi) return hi;
    return v;
}

std::int32_t span(std::int32_t a, std::int32_t b);
std::int32_t drive(std::int32_t v);

}  // namespace flight
#endif
