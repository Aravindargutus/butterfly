/* SPDX-License-Identifier: MIT OR Apache-2.0
 * Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
#include <stdio.h>
#include <string.h>
#include "crc.h"
#include "mode.h"
#include "sched.h"
#include "state.h"

int main(void) {
    state_reset();

    /* climb twice, then hold; task_trim is never dispatched */
    printf("alt %d\n", sched_run(0, 15000));
    printf("alt %d\n", sched_run(0, 50000));   /* clamps -> raises a fault */
    printf("alt %d\n", sched_run(1, 0));
    printf("bad %d\n", sched_run(9, 0));       /* out-of-range index */

    printf("%d %d %d\n", mode_permits_engage(1, 1, 0),
                         mode_permits_engage(0, 1, 0),
                         mode_permits_engage(0, 0, 1));
    printf("%s %s\n", mode_phase_name(g_state.phase), mode_phase_name(9));

    const uint8_t msg[] = "ARINC653";
    printf("%08x\n", crc_block(msg, (uint32_t)strlen((const char *)msg)));
    printf("faults %u  dispatches %u\n", g_state.faults, sched_dispatches());
    return 0;
}
