// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#include <cstdio>
#include <cstring>
#include "flight/crc.hpp"
#include "flight/mode.hpp"
#include "flight/sched.hpp"
#include "flight/state.hpp"
#include "flight/util.hpp"

int main() {
    using namespace flight;
    state_reset();

    std::printf("alt %d\n", sched_run(0, 15000));
    std::printf("alt %d\n", sched_run(0, 50000));   // clamps -> raises a fault
    std::printf("alt %d\n", sched_run(1, 0));
    std::printf("bad %d\n", sched_run(9, 0));       // out-of-range index

    std::printf("%d %d %d\n", mode_permits_engage(true, true, false),
                              mode_permits_engage(false, true, false),
                              mode_permits_engage(false, false, true));
    std::printf("%s %s\n", mode_phase_name(g_state.phase), mode_phase_name(9));

    // second instantiation of the header template, at a different type
    std::printf("scaled %d\n", scale_by<std::int16_t>(100, 3, 2));

    const std::uint8_t msg[] = "ARINC653";
    std::printf("%08x\n", crc_block(msg, static_cast<std::uint32_t>(std::strlen(reinterpret_cast<const char *>(msg)))));
    std::printf("faults %u  dispatches %u\n", g_state.faults, sched_dispatches());
    return 0;
}
