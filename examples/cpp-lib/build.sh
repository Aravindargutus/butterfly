#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
# Multi-file C++ library. Same two builds as the C library example.
# -fno-exceptions -fno-rtti keeps out object code with no source counterpart.
set -eu
cd "$(dirname "$0")"
# The tools that read the profile must come from the compiler that wrote
# it, so one variable names it and both the lookup and the build use that.
CXX="${CXX:-clang++}"
LLVM_FROM="$CXX"
. ../adapter.sh
. ../llvm-env.sh
begin_output build build/obj
SRC="src/util.cpp src/state.cpp src/mode.cpp src/crc.cpp src/sched.cpp src/main.cpp"
COMMON="-std=c++17 -Wall -Wextra -g -Iinclude -fno-exceptions -fno-rtti"

# 1. instrumented
"$CXX" $COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc \
     $SRC -o build/flight-instr
( cd build && LLVM_PROFILE_FILE=flight.profraw ./flight-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/flight.profraw -o build/flight.profdata
"$LLVM_COV" export -format=text -instr-profile=build/flight.profdata build/flight-instr > build/coverage.json

# 2. the image under analysis, one object per translation unit so each keeps
#    its own .su, linked with its debug information (examples/adapter.sh)
# shellcheck disable=SC2086
compile_each build/obj "$CXX" "$COMMON -O0 -fstack-usage" $SRC
# shellcheck disable=SC2086
"$CXX" $COMMON -O0 $OBJECTS -o build/flight
link_debug "$CXX" build/flight
collect_su build/obj build
write_build_info build/build-info.txt "$CXX" \
  "$COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc" \
  "$COMMON -O0 -fstack-usage" \
  "units=$(echo $SRC | tr ' ' ',')"
end_output
echo "ok: $(ls build)"
