#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# The call-graph fixture: a program with depth, recursion, mutual recursion,
# an indirect call and an external call, so the expandable Calls graph (#100)
# has every node state to draw.
#
# Built the same way as examples/c: an instrumented run for coverage, and a
# separate unintrumented build for the object code the coverage is projected
# onto. -O0 throughout, because the point is the call structure -- at -O1 the
# optimiser inlines the chain and there is nothing left to expand.
set -eu
cd "$(dirname "$0")"
CC="${CC:-clang}"
LLVM_FROM="$CC"
. ../adapter.sh
. ../llvm-env.sh
rm -rf build && mkdir -p build
# _FORTIFY_SOURCE is pinned off: with it on, some SDKs rewrite the `strncpy`
# in copy_len to `__builtin___strncpy_chk`, whose object-size check adds branch
# outcomes that others do not emit -- Apple clang 17 and 21 disagreed 6 vs 8,
# and example_numbers asserts an exact branch total. The external call to
# strncpy/strlen (this fixture's point) is unchanged; only the fortify wrapper,
# which is not what the call graph is demonstrating, is normalised away.
CFLAGS_COMMON="-std=c11 -Wall -Wextra -g -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0"
# 1. instrumented
"$CC" $CFLAGS_COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc \
      depth.c -o build/depth-instr
( cd build && LLVM_PROFILE_FILE=depth.profraw ./depth-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/depth.profraw -o build/depth.profdata
"$LLVM_COV" export -format=text -instr-profile=build/depth.profdata build/depth-instr > build/coverage.json
# 2. the image under analysis: one object per unit, so each keeps its own
#    .su, linked with its debug information (examples/adapter.sh, #245)
TARGET="$CFLAGS_COMMON -O0 -fstack-usage"
compile_each build/obj "$CC" "$TARGET" depth.c
# shellcheck disable=SC2086
"$CC" $TARGET $OBJECTS -o build/depth
link_debug "$CC" build/depth
collect_su build/obj build
write_build_info build/build-info.txt "$CC" \
  "$CFLAGS_COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc" \
  "$TARGET"
test -x build/depth || { echo "build/depth was not produced" >&2; exit 1; }
echo "ok: $(ls build)"
