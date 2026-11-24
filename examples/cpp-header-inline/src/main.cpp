// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#include <cstdio>
#include "geom.hpp"

int main() {
    std::printf("span %d\n", flight::span(10, 90000));
    std::printf("drive %d\n", flight::drive(-7));
    std::printf("clamped %d\n", flight::clamp_to(50000, 0, 4096));
    return 0;
}
