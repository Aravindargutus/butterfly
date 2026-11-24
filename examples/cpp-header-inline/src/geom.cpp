// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#include "geom.hpp"

namespace {
/// Internal linkage, and a function of this name exists in engine.cpp too.
/// llvm-cov names it `geom.cpp:(anonymous namespace)::scale(int)`; the
/// symbol table knows only the demangled `(anonymous namespace)::scale(int)`.
std::int32_t scale(std::int32_t v) { return v * 2; }
}  // namespace

namespace flight {
std::int32_t span(std::int32_t a, std::int32_t b) { return scale(clamp_to(b - a, 0, 4096)); }
}  // namespace flight
