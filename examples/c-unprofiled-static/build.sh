#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
# Three units define a function named `scale`: an instrumented extern, an
# instrumented static, and a static the profile has no record for at all.
# The third one's caller must still be reported as leaving the covered
# source set -- the negative case of #206 (#237). Built like examples/c:
#   1. host, instrumented -> runs, produces app.profraw -> coverage.json
#   2. host, NOT instrumented, -O0 -g -> the object code we project onto
# -O0 on purpose: the three copies must survive as real symbols.
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
      src/ext.c src/covered.c src/quiet.c src/main.c -o build/app-instr
( cd build && LLVM_PROFILE_FILE=app.profraw ./app-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/app.profraw -o build/app.profdata
"$LLVM_COV" export -format=text -instr-profile=build/app.profdata build/app-instr > build/coverage.json
# 2. the image under analysis: one object per unit, so each keeps its own
#    .su, linked with its debug information (examples/adapter.sh, #245)
TARGET="$CFLAGS_COMMON -O0 -fstack-usage"
compile_each build/obj "$CC" "$TARGET" src/ext.c src/covered.c src/quiet.c src/main.c
# shellcheck disable=SC2086
"$CC" $TARGET $OBJECTS -o build/app
link_debug "$CC" build/app
collect_su build/obj build
write_build_info build/build-info.txt "$CC" \
  "$CFLAGS_COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc" \
  "$TARGET"
echo "ok: $(ls build)"
