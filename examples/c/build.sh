#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
# Builds the butterfly C demo twice:
#   1. host, instrumented  -> runs, produces default.profraw -> coverage.json
#   2. host, NOT instrumented, -O1 -g  -> the "certified" object code we project onto
set -eu
cd "$(dirname "$0")"
# The tools that read the profile must come from the compiler that wrote
# it, so one variable names it and both the lookup and the build use that.
CC="${CC:-clang}"
LLVM_FROM="$CC"
. ../adapter.sh
. ../llvm-env.sh
rm -rf build && mkdir -p build
CFLAGS_COMMON="-std=c11 -Wall -Wextra -g"
# 1. instrumented
"$CC" $CFLAGS_COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc \
      demo.c -o build/demo-instr
( cd build && LLVM_PROFILE_FILE=demo.profraw ./demo-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/demo.profraw -o build/demo.profdata
"$LLVM_COV" export -format=text -instr-profile=build/demo.profdata build/demo-instr > build/coverage.json
# 2. the image under analysis: one object per unit, so each keeps its own
#    .su, linked with its debug information (examples/adapter.sh, #245)
TARGET="$CFLAGS_COMMON -O0 -fstack-usage"
compile_each build/obj "$CC" "$TARGET" demo.c
# shellcheck disable=SC2086
"$CC" $TARGET $OBJECTS -o build/demo
link_debug "$CC" build/demo
collect_su build/obj build
write_build_info build/build-info.txt "$CC" \
  "$CFLAGS_COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc" \
  "$TARGET"
echo "ok: $(ls build)"
