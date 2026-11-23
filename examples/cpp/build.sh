#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# Two builds, as butterfly always wants:
#   1. instrumented, run once -> coverage.json   (what the tests measured)
#   2. the build you certify, not instrumented   (what you actually analyse)
set -eu
cd "$(dirname "$0")"
# The tools that read the profile must come from the compiler that wrote
# it, so one variable names it and both the lookup and the build use that.
CXX="${CXX:-clang++}"
LLVM_FROM="$CXX"
. ../adapter.sh
. ../llvm-env.sh
rm -rf build && mkdir -p build

COMMON="-std=c++17 -Wall -Wextra -g -fno-exceptions -fno-rtti"
INSTR="-O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc"
TARGET="-O0 -fstack-usage"

# 1. instrumented build + run
$CXX $COMMON $INSTR flight.cpp -o build/flight-instr
( cd build && LLVM_PROFILE_FILE=flight.profraw ./flight-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/flight.profraw -o build/flight.profdata
"$LLVM_COV" export -format=text -instr-profile=build/flight.profdata build/flight-instr > build/coverage.json

# 2. the binary under analysis: its own object, so its .su is its own, linked
#    with its debug information (examples/adapter.sh, #245)
compile_each build/obj "$CXX" "$COMMON $TARGET" flight.cpp
# shellcheck disable=SC2086
$CXX $COMMON $TARGET $OBJECTS -o build/flight
link_debug "$CXX" build/flight
collect_su build/obj build
write_build_info build/build-info.txt "$CXX" "$COMMON $INSTR" "$COMMON $TARGET" "project=flight (C++ example)"

echo "built: $(ls build | tr '\n' ' ')"
