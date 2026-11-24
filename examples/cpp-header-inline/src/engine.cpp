// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#include "geom.hpp"

namespace {
/// The other `scale`: same name, same anonymous namespace, different unit.
std::int32_t scale(std::int32_t v) { return v * 3; }
}  // namespace

namespace flight {
std::int32_t drive(std::int32_t v) { return scale(clamp_to(v, 0, 4096)); }
}  // namespace flight
