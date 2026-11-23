#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# C++ exceptions and RTTI (#165): one program, built twice.
#   build/        with exceptions and RTTI (clang++'s defaults)
#   build-noexc/  with -fno-exceptions -fno-rtti, as every other C++ example
# Each has its own instrumented run, because the two builds are different
# programs to the compiler: the feature macros choose different code.
set -eu
cd "$(dirname "$0")"
CXX="${CXX:-clang++}"
LLVM_FROM="$CXX"
. ../adapter.sh
. ../llvm-env.sh
COMMON="-std=c++17 -Wall -Wextra -g"

one() {
  out="$1"; shift
  rm -rf "$out" && mkdir -p "$out"
  "$CXX" $COMMON "$@" -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc flight.cpp -o "$out/flight-instr"
  ( cd "$out" && LLVM_PROFILE_FILE=flight.profraw ./flight-instr > run.txt )
  "$LLVM_PROFDATA" merge -sparse "$out/flight.profraw" -o "$out/flight.profdata"
  "$LLVM_COV" export -format=text -instr-profile="$out/flight.profdata" "$out/flight-instr" > "$out/coverage.json"
  # the analysed image, through examples/adapter.sh: its own object so its
  # .su is its own, linked with its debug information
  target="$COMMON $* -O0 -fstack-usage"
  compile_each "$out/obj" "$CXX" "$target" flight.cpp
  # shellcheck disable=SC2086
  "$CXX" $target $OBJECTS -o "$out/flight"
  link_debug "$CXX" "$out/flight"
  collect_su "$out/obj" "$out"
  write_build_info "$out/build-info.txt" "$CXX" \
    "$COMMON $* -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc" \
    "$target"
}

one build
one build-noexc -fno-exceptions -fno-rtti
echo "ok: $(ls build) | $(ls build-noexc)"
