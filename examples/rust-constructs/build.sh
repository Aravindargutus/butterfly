#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
# Rust constructs fixture (#167): generics, dyn dispatch, closures,
# macro_rules!. As examples/rust: the coverage build needs a nightly rustc
# (-Zcoverage-options=condition), and the profile it writes can only be read by
# that nightly's own llvm-tools. rust-llvm-env.sh names them by absolute path so
# a nightly without the component fails loudly, not silently through PATH (#289,
# #291 review). The image under analysis is the pinned stable toolchain's,
# non-instrumented, -Copt-level=0.
set -eu
cd "$(dirname "$0")"
NIGHTLY="${BUTTERFLY_NIGHTLY:-nightly}"
. ../adapter.sh
. ../rust-llvm-env.sh
rm -rf build && mkdir -p build

# 1. instrumented
rustc +$NIGHTLY --edition 2021 -g -Copt-level=0 \
      -Cinstrument-coverage -Zcoverage-options=condition \
      constructs.rs -o build/constructs-instr
( cd build && LLVM_PROFILE_FILE=constructs.profraw ./constructs-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/constructs.profraw -o build/constructs.profdata
"$LLVM_COV" export -format=text -instr-profile=build/constructs.profdata build/constructs-instr > build/coverage.json

# 2. the image under analysis
rustc --edition 2021 -g -Copt-level=0 constructs.rs -o build/constructs

# the coverage toolchain (compiler_instr, llvm-profdata, llvm-cov) comes from
# rust-llvm-env.sh through recorded_tools, as in examples/rust (#298)
{
  printf 'compiler=%s\nflags_instr=%s\nflags_target=%s\nllvm_bin=%s\n' \
    "$(rustc -V)" \
    "-g -Copt-level=0 -Cinstrument-coverage -Zcoverage-options=condition" \
    "-g -Copt-level=0" "$LLVM_BIN_USED"
  recorded_tools
} > build/build-info.txt
echo "ok: $(ls build)"
