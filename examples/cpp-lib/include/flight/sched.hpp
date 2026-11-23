// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#pragma once
#include <cstdint>

namespace flight {

/// Abstract task. The vtable is emitted in the translation unit holding the
/// first non-inline virtual (the "key function"), so dispatch crosses files:
/// the call site is in sched.cpp, the implementations are too, and the
/// indirect branch is only resolvable through the vtable object.
class Task {
public:
    virtual ~Task() = default;
    virtual std::int32_t run(std::int32_t arg) const = 0;
    virtual const char *name() const = 0;
};

constexpr int kTaskCount = 3;

/// @satisfies LLR-SCHED-040 : dispatch through the task table
std::int32_t sched_run(int idx, std::int32_t arg);
/// @satisfies LLR-SCHED-041
std::uint32_t sched_dispatches();

}  // namespace flight
