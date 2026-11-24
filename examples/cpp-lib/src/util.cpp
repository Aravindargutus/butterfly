// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#include "flight/util.hpp"

namespace flight {
namespace {
// internal linkage: not part of the interface
std::int32_t abs_i32(std::int32_t x) { return x < 0 ? -x : x; }
}  // namespace

/// @satisfies LLR-UTIL-001
std::int32_t clamp_i32(std::int32_t v, std::int32_t lo, std::int32_t hi) {
    if (v < lo) {
        return lo;
    }
    if (v > hi) {
        return hi;
    }
    return v;
}

/// @satisfies LLR-UTIL-002
std::int32_t saturating_add(std::int32_t a, std::int32_t b, std::int32_t limit) {
    std::int32_t sum = a + b;
    if (abs_i32(sum) > limit) {
        return sum < 0 ? -limit : limit;   // @satisfies LLR-UTIL-002.1 saturation
    }
    return sum;
}

std::int32_t unused_scale(std::int32_t x) { return x * 3 / 2; }

}  // namespace flight
