// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#pragma once
#include <cstdint>

namespace flight {

/// @satisfies LLR-UTIL-001
std::int32_t clamp_i32(std::int32_t v, std::int32_t lo, std::int32_t hi);

/// @satisfies LLR-UTIL-002
std::int32_t saturating_add(std::int32_t a, std::int32_t b, std::int32_t limit);

/// Defined in the header, so it is instantiated into every translation unit
/// that uses it and the linker folds the copies: one source range, one
/// weak symbol reached through a stub.
/// @satisfies LLR-UTIL-003
template <typename T>
T scale_by(T v, T num, T den) {
    if (den == T{0}) {
        return v;
    }
    return static_cast<T>(v * num / den);
}

/// Exported but never called: a dead-code candidate in the linked image.
std::int32_t unused_scale(std::int32_t x);

}  // namespace flight
