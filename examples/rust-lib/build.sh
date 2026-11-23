#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
# Multi-module Rust crate. rustc compiles the whole crate from src/main.rs,
# so unlike C and C++ there is no per-file object step and no separate link.
# The instrumented build needs nightly for branch/condition regions; its
# llvm-profdata must be the nightly one, named by path (../rust-llvm-env.sh).
set -eu
cd "$(dirname "$0")"
NIGHTLY="${BUTTERFLY_NIGHTLY:-nightly}"
. ../adapter.sh
. ../rust-llvm-env.sh
begin_output build

# 1. instrumented
rustc +$NIGHTLY --edition 2021 -g -Copt-level=0 \
      -Cinstrument-coverage -Zcoverage-options=condition \
      src/main.rs -o build/flight-instr
( cd build && LLVM_PROFILE_FILE=flight.profraw ./flight-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/flight.profraw -o build/flight.profdata
"$LLVM_COV" export -format=text -instr-profile=build/flight.profdata build/flight-instr > build/coverage.json

# 2. the image under analysis
rustc --edition 2021 -g -Copt-level=0 src/main.rs -o build/flight

{
  printf 'compiler=%s\nflags_instr=%s\nflags_target=%s\nmodules=%s\nllvm_bin=%s\n' \
    "$(rustc -V)" \
    "-g -Copt-level=0 -Cinstrument-coverage -Zcoverage-options=condition" \
    "-g -Copt-level=0" \
    "$(ls src/*.rs | tr '\n' ',')" "$LLVM_BIN_USED"
  recorded_tools
} > build/build-info.txt
end_output
echo "ok: $(ls build)"
