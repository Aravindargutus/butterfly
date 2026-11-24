#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# The C constructs fixture (#168): dynamic stack frames (a VLA and an
# alloca), goto, a union, a bitfield struct, a variadic function and a macro
# that expands to control flow.
#
# Built like examples/c: an instrumented run for coverage, and a separate
# uninstrumented build for the object code the coverage is projected onto.
# -O0, so each construct survives into the object code as written.
set -eu
cd "$(dirname "$0")"
CC="${CC:-clang}"
LLVM_FROM="$CC"
. ../adapter.sh
. ../llvm-env.sh
rm -rf build && mkdir -p build
# _FORTIFY_SOURCE is pinned off, as in examples/c-callgraph: with it on, the
# SDK rewrites alloca_fill's memset to __memset_chk, and the memset written
# in the source is then reported as a call folded away. Not this example's
# subject.
CFLAGS_COMMON="-std=c11 -Wall -Wextra -g -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0"
# 1. instrumented
"$CC" $CFLAGS_COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc \
      constructs.c -o build/constructs-instr
( cd build && LLVM_PROFILE_FILE=constructs.profraw ./constructs-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/constructs.profraw -o build/constructs.profdata
"$LLVM_COV" export -format=text -instr-profile=build/constructs.profdata build/constructs-instr > build/coverage.json
# 2. the image under analysis, with its .su: the stack-usage file is what
#    marks vla_sum and alloca_fill as dynamic
TARGET="$CFLAGS_COMMON -O0 -fstack-usage"
compile_each build/obj "$CC" "$TARGET" constructs.c
# shellcheck disable=SC2086
"$CC" $TARGET $OBJECTS -o build/constructs
link_debug "$CC" build/constructs
collect_su build/obj build
write_build_info build/build-info.txt "$CC" \
  "$CFLAGS_COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc" \
  "$TARGET"
test -x build/constructs || { echo "build/constructs was not produced" >&2; exit 1; }
echo "ok: $(ls build)"
