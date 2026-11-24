#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# The jump-table fixture (#171): one switch dense enough that clang lowers
# it to a table and an indirect branch at every optimisation level.
#
# Built like examples/c: an instrumented run for coverage, and a separate
# uninstrumented build for the object code the coverage is projected onto.
set -eu
cd "$(dirname "$0")"
CC="${CC:-clang}"
LLVM_FROM="$CC"
. ../adapter.sh
. ../llvm-env.sh
rm -rf build && mkdir -p build
CFLAGS_COMMON="-std=c11 -Wall -Wextra -g"
# 1. instrumented
"$CC" $CFLAGS_COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc \
      switch.c -o build/switch-instr
( cd build && LLVM_PROFILE_FILE=switch.profraw ./switch-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/switch.profraw -o build/switch.profdata
"$LLVM_COV" export -format=text -instr-profile=build/switch.profdata build/switch-instr > build/coverage.json
# 2. the build the evidence describes, through examples/adapter.sh: its own
#    object so its .su is its own, linked with its debug information
TARGET="$CFLAGS_COMMON -O0 -fstack-usage"
compile_each build/obj "$CC" "$TARGET" switch.c
# shellcheck disable=SC2086
"$CC" $TARGET $OBJECTS -o build/switch
link_debug "$CC" build/switch
collect_su build/obj build
write_build_info build/build-info.txt "$CC" \
  "$CFLAGS_COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc" \
  "$TARGET"
# 3. the same image at -O2, where the table holds byte offsets scaled by
#    four instead of words. The coverage is the -O0 run's, projected: it is
#    per source line, whatever the build.
rm -rf build-o2 && mkdir -p build-o2
compile_each build-o2/obj "$CC" "$CFLAGS_COMMON -O2" switch.c
# shellcheck disable=SC2086
"$CC" $CFLAGS_COMMON -O2 $OBJECTS -o build-o2/switch
link_debug "$CC" build-o2/switch
cp build/coverage.json build-o2/
write_build_info build-o2/build-info.txt "$CC" \
  "$CFLAGS_COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc" \
  "$CFLAGS_COMMON -O2"
echo "ok: $(ls build) | $(ls build-o2)"
