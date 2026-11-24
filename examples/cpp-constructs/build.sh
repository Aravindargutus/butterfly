#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# The everyday-C++ fixture (#166): constructors and destructors, overloaded
# operators, lambdas, new/delete. Built the way examples/cpp is: an
# instrumented run for coverage, and a separate uninstrumented build for the
# object code the coverage is projected onto. -fno-exceptions -fno-rtti, the
# certification-shaped subset; exceptions and RTTI are #165.
set -eu
cd "$(dirname "$0")"
CXX="${CXX:-clang++}"
LLVM_FROM="$CXX"
. ../adapter.sh
. ../llvm-env.sh
rm -rf build && mkdir -p build

COMMON="-std=c++17 -Wall -Wextra -g -fno-exceptions -fno-rtti"
INSTR="-O0 -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc"
TARGET="-O0 -fstack-usage"

# 1. instrumented build + run
# shellcheck disable=SC2086
$CXX $COMMON $INSTR constructs.cpp -o build/constructs-instr
( cd build && LLVM_PROFILE_FILE=constructs.profraw ./constructs-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/constructs.profraw -o build/constructs.profdata
"$LLVM_COV" export -format=text -instr-profile=build/constructs.profdata build/constructs-instr > build/coverage.json

# 2. the binary under analysis: its own object, so its .su is its own, linked
#    with its debug information (examples/adapter.sh, #245)
compile_each build/obj "$CXX" "$COMMON $TARGET" constructs.cpp
# shellcheck disable=SC2086
$CXX $COMMON $TARGET $OBJECTS -o build/constructs
link_debug "$CXX" build/constructs
collect_su build/obj build
write_build_info build/build-info.txt "$CXX" "$COMMON $INSTR" "$COMMON $TARGET" "project=constructs (C++ constructs example)"

test -x build/constructs || { echo "build/constructs was not produced" >&2; exit 1; }
echo "built: $(ls build | tr '\n' ' ')"
