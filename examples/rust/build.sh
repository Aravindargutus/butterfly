#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
# Rust fixture: MC/DC needs a nightly rustc (-Zcoverage-options=condition). The
# "certified" build uses the pinned stable toolchain, non-instrumented, -Copt-level=0.
set -eu
cd "$(dirname "$0")"
NIGHTLY="${BUTTERFLY_NIGHTLY:-nightly}"
. ../adapter.sh
. ../rust-llvm-env.sh
rm -rf build && mkdir -p build
rustc +$NIGHTLY --edition 2021 -g -Copt-level=0 -Cinstrument-coverage -Zcoverage-options=condition demo.rs -o build/demo-instr
( cd build && LLVM_PROFILE_FILE=demo.profraw ./demo-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/demo.profraw -o build/demo.profdata
"$LLVM_COV" export -format=text -instr-profile=build/demo.profdata build/demo-instr > build/coverage.json
rustc --edition 2021 -g -Copt-level=0 demo.rs -o build/demo
{
  printf 'compiler=%s\nflags_instr=%s\nflags_target=%s\nllvm_bin=%s\n' \
    "$(rustc -V)" \
    "-g -Copt-level=0 -Cinstrument-coverage -Zcoverage-options=condition" \
    "-g -Copt-level=0" "$LLVM_BIN_USED"
  recorded_tools
} > build/build-info.txt
echo "ok: $(ls build)"
