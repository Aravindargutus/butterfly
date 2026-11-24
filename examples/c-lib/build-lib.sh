#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# The library case: src/ has no main and cannot be run, so coverage comes from
# a test harness in tests/ that links against it. Three artefacts:
#   1. libflight.a          the deliverable -- built exactly as it ships
#   2. harness-instr        library sources + harness, instrumented -> coverage
#   3. harness              library archive + harness, linked, NOT instrumented
#                           -> the image whose object code is analysed
# Analyse (3), not the archive: object-code evidence needs real addresses.
set -eu
cd "$(dirname "$0")"
# The tools that read the profile must come from the compiler that wrote
# it, so one variable names it and both the lookup and the build use that.
CC="${CC:-clang}"
LLVM_FROM="$CC"
. ../adapter.sh
. ../llvm-env.sh
begin_output build-lib build-lib/obj
COMMON="-std=c11 -Wall -Wextra -g -Iinclude"
LIB="util state mode crc sched"          # note: no main
HARNESS=tests/test_flight.c

# 1. the library, as delivered
# shellcheck disable=SC2086
compile_each build-lib/obj "$CC" "$COMMON -O0 -fstack-usage" $(for b in $LIB; do printf 'src/%s.c ' "$b"; done)
resolve_tool "$CC" ar
# shellcheck disable=SC2086
"$TOOL" rcs build-lib/libflight.a $OBJECTS

# 2. coverage of the library sources, driven by the harness
"$CC" $COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc \
      src/util.c src/state.c src/mode.c src/crc.c src/sched.c $HARNESS \
      -o build-lib/harness-instr
( cd build-lib && LLVM_PROFILE_FILE=h.profraw ./harness-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build-lib/h.profraw -o build-lib/h.profdata
"$LLVM_COV" export -format=text -instr-profile=build-lib/h.profdata build-lib/harness-instr \
      > build-lib/coverage.json

# 3. the linked image under analysis: the archive plus the harness.
# Compile the harness to a real object first. `clang file.c lib.a -o exe`
# compiles to a temporary object and deletes it, and dsymutil then cannot read
# the debug map -- the dSYM comes out empty and every instruction lands in
# "unknown". Keeping the .o is what makes the line table survive the link.
# The harness is built like the library, -fstack-usage included: build-info
# declares it for the whole image, and butterfly holds the build to that.
"$CC" $COMMON -O0 -fstack-usage -c $HARNESS -o build-lib/obj/test_flight.o
"$CC" $COMMON -O0 build-lib/obj/test_flight.o build-lib/libflight.a -o build-lib/harness
link_debug "$CC" build-lib/harness
collect_su build-lib/obj build-lib

write_build_info build-lib/build-info.txt "$CC" \
  "$COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc" \
  "$COMMON -O0 -fstack-usage" \
  "library=libflight.a (util,state,mode,crc,sched -- no main)" \
  "harness=$HARNESS"
end_output
echo "ok: $(ls build-lib)"
