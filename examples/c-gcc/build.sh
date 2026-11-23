#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# The GCC example: the same shape as examples/c, built with GCC so the gcov
# front end is exercised on real GCC output (#45).
#
#   1. instrumented with --coverage, run, then read back with `gcov`
#   2. a separate non-instrumented build: the object code we project onto
set -eu
cd "$(dirname "$0")"

# gcov must be the one that belongs to this gcc: a .gcda carries a format
# stamped by the compiler that wrote it, and a mismatched gcov refuses it --
# the same pairing rule as llvm-profdata and clang (#61). Ask the compiler
# where its own tools are rather than hoping PATH agrees.
CC="${CC:-gcc}"
if [ -n "${GCOV:-}" ]; then
  :
else
  GCOV=$("$CC" -print-prog-name=gcov 2>/dev/null || true)
  case "$GCOV" in
    /*) ;;
    # A bare name means the compiler could not say. Derive it from the
    # compiler's own name (gcc-16 -> gcov-16) before falling back, so a
    # Homebrew GCC beside Apple's clang-flavoured `gcov` still pairs up.
    *)
      base=$(basename "$CC")
      case "$base" in
        gcc-*) GCOV="gcov-${base#gcc-}" ;;
        *) GCOV=gcov ;;
      esac
      ;;
  esac
fi
GCOV=$(command -v "$GCOV" 2>/dev/null) || { echo "build: no $GCOV on PATH; set GCOV to the gcov matching $CC" >&2; exit 1; }
# A name found on PATH is not proof of a pair, and gcov's own complaint about
# a mismatch goes to the /dev/null its stderr is sent to below. GCC prints
# the same banner for both, bar the program name, so they must agree (#289).
release() { "$1" --version 2>/dev/null | head -1 | cut -d' ' -f2-; }
[ "$(release "$GCOV")" = "$(release "$CC")" ] || { echo "build: $GCOV is $(release "$GCOV") but $CC is $(release "$CC"); set GCOV to the gcov of the same release" >&2; exit 1; }

# Homebrew's GCC on macOS does not know where the SDK is, so linking fails
# with `library 'System' not found` unless it is told. Harmless elsewhere.
SDKFLAGS=""
if [ "$(uname -s)" = "Darwin" ] && command -v xcrun >/dev/null 2>&1; then
  SDK=$(xcrun --show-sdk-path)
  SDKFLAGS="-B$SDK/usr/lib -L$SDK/usr/lib"
fi

rm -rf build && mkdir -p build
COMMON="-std=c11 -Wall -Wextra -g"

# 1. instrumented, run, and read back.
#
#    -fcondition-coverage (GCC 14+) adds condition coverage where the
#    compiler supports it. It is *not* MC/DC and butterfly says so; it is
#    here because the difference is easier to see when the data is present.
COND=""
if "$CC" -fcondition-coverage -E - </dev/null >/dev/null 2>&1; then
  COND="-fcondition-coverage"
fi
# Compiled and linked in two steps on purpose. In one step GCC names the
# notes and data files after the *output binary* -- `limits-instr-limits.gcno`
# -- and gcov then cannot find them from the source name. Compiling to an
# object first gives `limits.gcno`/`limits.gcda` beside it, which is both
# predictable and what a real build does.
# shellcheck disable=SC2086
( cd build && "$CC" $COMMON -O0 --coverage $COND -c ../limits.c -o limits.o )
# shellcheck disable=SC2086
( cd build && "$CC" $COMMON -O0 --coverage $COND -c ../util.c -o util.o )
# shellcheck disable=SC2086
( cd build && "$CC" --coverage $SDKFLAGS limits.o util.o -o limits-instr )
( cd build && ./limits-instr > run.txt )

# `-b` for branch outcomes: without it gcov emits no branches at all and the
# evidence would quietly carry no decision coverage. `-g` adds the condition
# records where the compiler produced them.
# shellcheck disable=SC2086
# Both units in one export: a project is more than one file, and the reader
# has to place each function in the file it is actually in.
( cd build && "$GCOV" -b $( [ -n "$COND" ] && echo -g ) --json-format --stdout limits.gcda util.gcda > coverage.json 2>/dev/null )

# 2. the object code under analysis, built without instrumentation: one
#    object per unit so each keeps its own .su, linked with its debug
#    information (examples/adapter.sh, #245)
. ../adapter.sh
record_tool gcov_path "$GCOV"
# and what produced coverage.json, under the keys `butterfly build` writes
# (#298): the gcc that built the instrumented binary, and gcov's version,
# asked in build/, where it ran
record_coverage_compiler "$CC"
record_tool gcov "$(cd build && version_of "$GCOV")"
compile_each build/obj "$CC" "$COMMON -O0 -fstack-usage" limits.c util.c
# shellcheck disable=SC2086
"$CC" $SDKFLAGS $OBJECTS -o build/limits
link_debug "$CC" build/limits
collect_su build/obj build

write_build_info build/build-info.txt "$CC" "$COMMON -O0 --coverage $COND" "$COMMON -O0 -fstack-usage"

echo "ok: $(ls build)"
