// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

// butterfly C++ example.
//
// Deliberately exercises the constructs a DO-178C reviewer cares about, and
// the ones that make source and object code diverge:
//   * a decision with three conditions            -> MC/DC
//   * a switch                                    -> jump table / compare chain
//   * a loop with an untaken branch               -> uncovered outcome
//   * a template used at two types                -> two instantiations, two functions
//   * virtual dispatch through a table            -> indirect call, vtable
//   * a function nothing calls                    -> dead-code candidate
//
// Built without exceptions or RTTI, as avionics C++ subsets usually require.

#include <cstdint>
#include <cstdio>

namespace flight {

/* @satisfies LLR-ALT-001 */
int32_t clamp_altitude(int32_t alt, int32_t lo, int32_t hi) {
    if (alt < lo) {
        return lo;
    }
    if (alt > hi) {
        return hi;
    }
    return alt;
}

/* @satisfies LLR-MODE-002 : one decision, three conditions */
bool mode_permits_engage(bool gear_down, bool speed_ok, bool wow) {
    if ((gear_down && speed_ok) || wow) {
        return true;
    }
    return false;
}

/* @satisfies LLR-DISP-003 */
const char *phase_name(uint8_t phase) {
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

/* @satisfies LLR-CRC-004 */
uint32_t checksum(const uint8_t *buf, uint32_t len) {
    uint32_t acc = 0;
    for (uint32_t i = 0; i < len; i++) {
        acc = (acc << 3) ^ buf[i];
        if (acc & 0x80000000u) {
            acc ^= 0x04C11DB7u;   /* @satisfies LLR-CRC-004.1 polynomial fold */
        }
    }
    return acc;
}

/* @satisfies LLR-SAT-005 : one template, instantiated for two types.
   llvm-cov emits a coverage record per instantiation; each is its own function. */
template <typename T>
T saturating_add(T a, T b, T limit) {
    T sum = static_cast<T>(a + b);
    if (sum > limit) {
        return limit;
    }
    return sum;
}

/* @satisfies LLR-LIM-006 : dispatch through a table of function pointers
   (a vtable). The call site is indirect; only HardLimit is ever selected. */
struct Limiter {
    virtual int32_t apply(int32_t v) const = 0;
};

struct HardLimit final : Limiter {
    int32_t apply(int32_t v) const override { return clamp_altitude(v, 0, 60000); }
};

struct SoftLimit final : Limiter {
    int32_t apply(int32_t v) const override { return v / 2; }
};

static const HardLimit HARD;
static const SoftLimit SOFT;
static const Limiter *const LIMITERS[] = { &HARD, &SOFT };

/* @satisfies LLR-LIM-006 */
int32_t limit_with(uint8_t which, int32_t v) {
    return LIMITERS[which]->apply(v);
}

/* never called: candidate dead code */
int32_t legacy_scale(int32_t x) {
    return x * 3 / 2;
}

}  // namespace flight

int main() {
    const int32_t alts[] = { -100, 500, 90000 };
    for (int i = 0; i < 3; i++) {
        printf("%d\n", flight::clamp_altitude(alts[i], 0, 60000));
    }
    printf("%d\n", flight::mode_permits_engage(true, true, false));
    printf("%d\n", flight::mode_permits_engage(false, true, false));
    printf("%d\n", flight::mode_permits_engage(false, false, true));
    printf("%s %s %s\n", flight::phase_name(0), flight::phase_name(3), flight::phase_name(9));
    const uint8_t msg[] = "ARINC653";
    printf("%08x\n", flight::checksum(msg, 8));
    printf("%d %d\n", flight::saturating_add<int32_t>(2000000000, 2000000000, 2147483000),
                      static_cast<int>(flight::saturating_add<int16_t>(100, 200, 30000)));
    printf("%d\n", flight::limit_with(0, 70000));   /* only HardLimit is reached */
    return 0;
}
