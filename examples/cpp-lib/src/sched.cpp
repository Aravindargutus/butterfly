// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#include "flight/sched.hpp"
#include "flight/mode.hpp"
#include "flight/state.hpp"
#include "flight/util.hpp"

namespace flight {
namespace {

std::uint32_t s_dispatches = 0;

class ClimbTask final : public Task {
public:
    std::int32_t run(std::int32_t step) const override {
        state_set_altitude(state_altitude() + step);   // -> state -> util
        mode_advance(2);
        return state_altitude();
    }
    const char *name() const override { return "climb"; }
};

class HoldTask final : public Task {
public:
    std::int32_t run(std::int32_t) const override { return state_altitude(); }
    const char *name() const override { return "hold"; }
};

class TrimTask final : public Task {
public:
    std::int32_t run(std::int32_t delta) const override {
        // two template instantiations of scale_by, at int32_t and int16_t
        const std::int32_t d = scale_by<std::int32_t>(delta, 3, 2);
        return saturating_add(state_altitude(), d, kAltMax);
    }
    const char *name() const override { return "trim"; }
};

const ClimbTask kClimb;
const HoldTask  kHold;
const TrimTask  kTrim;
const Task *const kTasks[kTaskCount] = { &kClimb, &kHold, &kTrim };

}  // namespace

/// @satisfies LLR-SCHED-040 : dispatch through the task table
std::int32_t sched_run(int idx, std::int32_t arg) {
    if (idx < 0 || idx >= kTaskCount) {
        state_raise_fault(kFaultBadTask);
        return -1;
    }
    s_dispatches++;
    return kTasks[idx]->run(arg);   // virtual call: an indirect-branch gap
}

/// @satisfies LLR-SCHED-041
std::uint32_t sched_dispatches() { return s_dispatches; }

}  // namespace flight
