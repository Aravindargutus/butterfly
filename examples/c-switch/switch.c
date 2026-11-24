// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// A switch that compiles to a jump table (#171). Sixteen dense cases, each
// with its own side effect so the compiler cannot turn the switch into a
// lookup of values: it has to branch, and with this many cases it branches
// through a table -- an address computed from the case number and an
// indirect `br` (AArch64) or `jmp *` (x86-64) -- rather than a compare
// chain. examples/c's phase_name switch is too small and too regular for
// that, and compiles to compares.
//
// main runs cases 0 to 5 only, so the image has covered and uncovered case
// blocks behind the same indirect branch.
#include <stdio.h>

volatile int sink;

/* @satisfies LLR-SW-001 */
void dispatch(int op) {
    switch (op) {
    case 0: sink += 1; break;
    case 1: sink -= 4; puts("one"); break;
    case 2: sink ^= 7; break;
    case 3: sink |= 10; puts("three"); break;
    case 4: sink &= 13; break;
    case 5: sink += 16; break;
    case 6: sink -= 19; puts("six"); break;
    case 7: sink ^= 22; break;
    case 8: sink |= 25; break;
    case 9: sink &= 28; puts("nine"); break;
    case 10: sink += 31; break;
    case 11: sink -= 34; break;
    case 12: sink ^= 37; puts("twelve"); break;
    case 13: sink |= 40; break;
    case 14: sink &= 43; break;
    case 15: sink += 46; puts("fifteen"); break;
    default: sink = -1; break;
    }
}

int main(void) {
    for (int op = 0; op < 6; op++) {
        dispatch(op);
    }
    return 0;
}
