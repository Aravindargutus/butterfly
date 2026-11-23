#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
# Multi-file C library, built the same two ways as the single-file example:
#   1. every TU instrumented -> run -> one merged profile -> coverage.json
#   2. every TU compiled clean with -fstack-usage -> the image under analysis
# Needs clang and llvm-tools (rustup component add llvm-tools).
set -eu
cd "$(dirname "$0")"
# The tools that read the profile must come from the compiler that wrote
# it, so one variable names it and both the lookup and the build use that.
CC="${CC:-clang}"
LLVM_FROM="$CC"
. ../adapter.sh
. ../llvm-env.sh
begin_output build build/obj
SRC="src/util.c src/state.c src/mode.c src/crc.c src/sched.c src/main.c"
COMMON="-std=c11 -Wall -Wextra -g -Iinclude"

# 1. instrumented: one binary from all six translation units
"$CC" $COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc \
      $SRC -o build/flight-instr
( cd build && LLVM_PROFILE_FILE=flight.profraw ./flight-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/flight.profraw -o build/flight.profdata
"$LLVM_COV" export -format=text -instr-profile=build/flight.profdata build/flight-instr > build/coverage.json

# 2. the image under analysis: compile per TU so each gets its own .su, then
#    link with the debug information. On macOS a *link* of separate .o files
#    leaves the debug info behind a debug map, and only dsymutil turns it into
#    the line table; link_debug fails the build if that does not happen
#    (examples/adapter.sh, #245).
# shellcheck disable=SC2086
compile_each build/obj "$CC" "$COMMON -O0 -fstack-usage" $SRC
# shellcheck disable=SC2086
"$CC" $COMMON -O0 $OBJECTS -o build/flight
link_debug "$CC" build/flight
collect_su build/obj build

write_build_info build/build-info.txt "$CC" \
  "$COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc" \
  "$COMMON -O0 -fstack-usage" \
  "units=$(echo $SRC | tr ' ' ',')"
end_output
echo "ok: $(ls build)"
