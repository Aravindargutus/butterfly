// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// C++ exceptions and RTTI (#165). The same program built twice: with them
// (build/) and with -fno-exceptions -fno-rtti (build-noexc/), which is how
// every other C++ example is built. The compiler's own feature macros pick
// the path, so the two builds differ only in what the flags change.
#include <cstdio>

struct Sensor {
    virtual ~Sensor() = default;
    virtual int read() const = 0;
};

struct Altimeter : Sensor {
    int feet;
    explicit Altimeter(int f) : feet(f) {}
    int read() const override { return feet; }
};

struct Airspeed : Sensor {
    int knots;
    explicit Airspeed(int k) : knots(k) {}
    int read() const override { return knots; }
};

#if __cpp_exceptions
struct OutOfRange {
    int value;
};
#endif

/* @satisfies LLR-EXC-001 */
int checked_altitude(int feet) {
    if (feet < 0 || feet > 60000) {
#if __cpp_exceptions
        throw OutOfRange{feet};
#else
        return -1;
#endif
    }
    return feet;
}

/* @satisfies LLR-EXC-002 */
int safe_altitude(int feet) {
#if __cpp_exceptions
    try {
        return checked_altitude(feet);
    } catch (const OutOfRange &e) {
        std::printf("rejected %d\n", e.value);
        return 0;
    }
#else
    int v = checked_altitude(feet);
    if (v < 0) {
        std::printf("rejected %d\n", feet);
        return 0;
    }
    return v;
#endif
}

/* @satisfies LLR-EXC-003 */
int altitude_of(const Sensor &s) {
#if __cpp_rtti
    if (const Altimeter *a = dynamic_cast<const Altimeter *>(&s)) {
        return a->feet;
    }
    return -1;
#else
    // without RTTI the program has to know the type another way
    return s.read() >= 0 && s.read() <= 60000 ? s.read() : -1;
#endif
}

int main() {
    Altimeter alt(12000);
    Airspeed spd(250);
    int total = safe_altitude(35000) + safe_altitude(-5);
    total += altitude_of(alt) + altitude_of(spd);
    std::printf("%d\n", total);
    return 0;
}
