#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# Per-test coverage (#41). One instrumented binary, run once per test, each
# run writing its own profile:
#
#   * the merge of every profile is the aggregate -- exactly the coverage.json
#     any other example produces, and the source of truth for every count;
#   * each profile is also exported on its own, so butterfly can say which
#     test exercised what.
#
# This is the ordinary llvm-profdata workflow, not a butterfly invention: a
# test framework does the same with more ceremony.
set -eu
cd "$(dirname "$0")"
# The tools that read the profile must come from the compiler that wrote
# it, so one variable names it and both the lookup and the build use that.
CC="${CC:-clang}"
LLVM_FROM="$CC"
. ../adapter.sh
. ../llvm-env.sh
begin_output build build/prof build/tests
COMMON="-std=c11 -Wall -Wextra -g -Iinc"
TESTS="t_clamp t_ceiling t_faults t_band_low t_band_high t_bank_shallow t_bank_steep_and_turning t_bank_steep_no_turn"

"$CC" $COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc \
      src/limits.c src/tests.c -o build/tests-instr

# one run, one profile, one export per test
for t in $TESTS; do
  LLVM_PROFILE_FILE="build/prof/$t.profraw" ./build/tests-instr "$t" || {
    echo "test $t failed" >&2; exit 1; }
  "$LLVM_PROFDATA" merge -sparse "build/prof/$t.profraw" -o "build/prof/$t.profdata"
  "$LLVM_COV" export -format=text -instr-profile="build/prof/$t.profdata" \
      build/tests-instr > "build/tests/$t.json"
done

# the aggregate is the merge of exactly those runs
"$LLVM_PROFDATA" merge -sparse build/prof/*.profraw -o build/all.profdata
"$LLVM_COV" export -format=text -instr-profile=build/all.profdata build/tests-instr > build/coverage.json

# the image under analysis: one object per unit. Compiled and linked in one
# step, both units' .su came out under one name and limits.c's was
# overwritten -- its frames were silently estimated from prologues while
# build-info.txt declared -fstack-usage (#245).
compile_each build/obj "$CC" "$COMMON -O0 -fstack-usage" src/limits.c src/tests.c
# shellcheck disable=SC2086
"$CC" $COMMON -O0 $OBJECTS -o build/app
link_debug "$CC" build/app
collect_su build/obj build

write_build_info build/build-info.txt "$CC" \
  "$COMMON -O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc" \
  "$COMMON -O0 -fstack-usage" \
  "tests=$(echo $TESTS | tr ' ' ',')"
end_output
echo "ok: $(ls build) | tests: $(ls build/tests)"
