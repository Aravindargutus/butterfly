#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# butterfly build template — copy this into your project and edit the block
# below. Nothing else should need changing; it drives clang or GCC from the
# same file and produces everything `butterfly` looks for.
#
# butterfly never runs this script for you. You run it, or your CI does. That
# is deliberate: this file is yours, it can do anything, and no tool of ours
# is going to execute it on your behalf from a web page. See SECURITY.md.
#
# Two builds come out of one source tree, and they are not the same build:
#
#   1. INSTRUMENTED — compiled with coverage, run once, read back. This tells
#      us which lines, branches and conditions your tests actually exercised.
#   2. UNDER ANALYSIS — the same sources with no instrumentation. This is the
#      object code the evidence is about. Instrumented code is not the code
#      you ship, so it is never the code we project coverage onto.
#
# Check the result with:  butterfly doctor build

set -eu
cd "$(dirname "$0")"

# Everything this script says goes through say(), to stderr, so that stdout
# stays clean for anything that wants to pipe it. It narrates each step: a
# build you cannot watch is a build you cannot diagnose.
say() { echo "build: $*" >&2; }

# ─────────────────────────── EDIT THIS BLOCK ───────────────────────────
PROJECT=myproject                  # names the binary and the build info
SRC="src/foo.c src/bar.c"          # every translation unit, space separated
INCLUDE="-Iinclude"                # -I flags, or empty
STD="-std=c11"                     # -std=c++17 for C++, etc.
WARN="-Wall -Wextra"
OPT="-O0"                          # -O0 keeps source and object aligned;
                                   # raise it only when you mean to analyse
                                   # what the optimiser did
OUT=build                          # everything lands here
# ───────────────────────────── END OF BLOCK ────────────────────────────

# The toolchain is whatever CC says, defaulting to clang. `CC=gcc ./build.sh`
# switches the whole script over.
CC="${CC:-clang}"
CCVER=$("$CC" --version 2>&1 | head -1)
case "$CCVER" in
  *clang*) KIND=clang ;;
  *)       KIND=gcc ;;
esac

# On a Mac, `gcc` is Apple clang wearing a different name. Left alone that
# produces a build that half works: clang accepts --coverage and writes
# .gcno/.gcda, but `gcc -print-prog-name=gcov` then hands back *LLVM's* gcov,
# which has no --json-format and cannot read them. Say so plainly, because the
# failure otherwise surfaces three steps later as an empty coverage file.
case "$CC:$KIND" in
  *gcc*:clang)
    say "note -- '$CC' is not GCC. It reports: $CCVER"
    say "building with the clang toolchain instead. For real GCC, name it"
    say "explicitly:  CC=gcc-16 $0   (or whatever your GCC is called)"
    ;;
esac

say "toolchain  $CC -> $CCVER [$KIND]"
COMMON="$STD $WARN -g $INCLUDE"

# Homebrew GCC on macOS knows nothing about the Apple SDK: it fails to find
# <stdio.h> when compiling and `library 'System' not found` when linking. Apple
# clang locates the SDK by itself, so this is applied only to GCC, and only on
# a Mac -- everywhere else both variables stay empty.
SDKFLAGS=""
if [ "$KIND" = gcc ] && [ "$(uname -s)" = "Darwin" ] && command -v xcrun >/dev/null 2>&1; then
  SDK=$(xcrun --show-sdk-path)
  COMMON="$COMMON -isysroot $SDK"        # compiling: find the system headers
  SDKFLAGS="-B$SDK/usr/lib -L$SDK/usr/lib"  # linking: find libSystem
  say "macOS SDK   $SDK (GCC needs telling; clang finds it itself)"
fi

# Object and coverage files are named after the source *basename*, so two
# sources sharing one (src/a.c and lib/a.c) would silently overwrite each
# other and the evidence would quietly be missing a translation unit. Refuse
# rather than produce a build that is wrong in a way nobody would notice.
dupes=$(for f in $SRC; do basename "${f%.*}"; done | sort | uniq -d)
if [ -n "$dupes" ]; then
  echo "build: two sources share a basename, which would overwrite: $dupes" >&2
  echo "build: rename one, or build them as separate butterfly variants." >&2
  exit 1
fi

say "output     $OUT/ (removing and recreating)"
rm -rf "$OUT" && mkdir -p "$OUT/obj"

if [ "$KIND" = clang ]; then
  # ── clang ──────────────────────────────────────────────────────────────
  # The tools that read a profile must come from the compiler that wrote it:
  # a .profraw carries a version stamp, and a newer llvm-profdata reading an
  # older one (or the reverse) fails with "no profile can be merged". Ask the
  # compiler where its own tools live rather than trusting PATH.
  bindir=$("$CC" -print-prog-name=llvm-profdata 2>/dev/null)
  case "$bindir" in
    /*) LLVM_PROFDATA="$bindir"; LLVM_COV=$("$CC" -print-prog-name=llvm-cov) ;;
    *)  LLVM_PROFDATA="${LLVM_PROFDATA:-llvm-profdata}"; LLVM_COV="${LLVM_COV:-llvm-cov}" ;;
  esac
  command -v "$LLVM_PROFDATA" >/dev/null 2>&1 || {
    say "no llvm-profdata matching $CC."
    say "install it (rustup component add llvm-tools) or set LLVM_PROFDATA."
    exit 1; }
  say "profile rdr $LLVM_PROFDATA"

  # -fcoverage-mcdc is what makes DO-178C level A objective 5 answerable.
  # Without it you get statement and branch coverage and nothing else.
  MCDC=""
  if echo 'int main(void){return 0;}' | "$CC" -fcoverage-mcdc -fprofile-instr-generate -fcoverage-mapping -x c - -o /dev/null 2>/dev/null; then
    MCDC="-fcoverage-mcdc"
    say "mc/dc      supported, and enabled"
  else
    say "mc/dc      NOT supported by $CC -- the evidence will carry no MC/DC,"
    say "           so DO-178C level A objective 5 cannot be answered from it."
  fi
  INSTR="-fprofile-instr-generate -fcoverage-mapping $MCDC"

  say "[1/4] compiling instrumented:$INSTR"
  for f in $SRC; do say "        $f"; done
  # shellcheck disable=SC2086
  "$CC" $COMMON $OPT $INSTR $SRC -o "$OUT/$PROJECT-instr"
  say "[2/4] running $OUT/$PROJECT-instr (stdout -> $OUT/run.txt)"
  ( cd "$OUT" && LLVM_PROFILE_FILE="$PROJECT.profraw" "./$PROJECT-instr" > run.txt )
  say "[3/4] merging the profile and exporting coverage"
  "$LLVM_PROFDATA" merge -sparse "$OUT/$PROJECT.profraw" -o "$OUT/$PROJECT.profdata"
  "$LLVM_COV" export -format=text -instr-profile="$OUT/$PROJECT.profdata" \
      "$OUT/$PROJECT-instr" > "$OUT/coverage.json"
  say "      coverage.json is $(wc -c < "$OUT/coverage.json" | tr -d ' ') bytes"
else
  # ── GCC ────────────────────────────────────────────────────────────────
  # Same pairing rule: a .gcda is stamped by the gcc that wrote it and a
  # mismatched gcov refuses it. Derive gcov from the compiler, so a Homebrew
  # gcc-16 beside Apple's clang-flavoured `gcov` still pairs up.
  if [ -z "${GCOV:-}" ]; then
    GCOV=$("$CC" -print-prog-name=gcov 2>/dev/null || true)
    case "$GCOV" in
      /*) ;;
      *) base=$(basename "$CC")
         case "$base" in gcc-*) GCOV="gcov-${base#gcc-}" ;; *) GCOV=gcov ;; esac ;;
    esac
  fi
  command -v "$GCOV" >/dev/null 2>&1 || {
    say "no $GCOV on PATH; set GCOV to the gcov matching $CC."
    exit 1; }
  say "coverage rdr $GCOV -> $("$GCOV" --version 2>&1 | head -1)"

  # GCC 14+ can record condition coverage. It is *not* MC/DC and butterfly
  # reports it as what it is, but the data is worth having when present.
  COND=""
  if "$CC" -fcondition-coverage -E - </dev/null >/dev/null 2>&1; then
    COND="-fcondition-coverage"
    say "conditions supported, and enabled (this is NOT MC/DC)"
  else
    say "conditions not supported by $CC (GCC 14+ only); branches only"
  fi
  say "mc/dc      not available from gcov at all, whatever the GCC version"

  # Compile and link in two steps, on purpose. Done in one step GCC names the
  # notes and data after the *output binary* (foo-instr-foo.gcno) and gcov
  # cannot then find them from the source name. Per-object gives foo.gcno
  # beside foo.o, which is predictable and is what a real build does.
  #
  # Everything below runs from the project root, never from inside $OUT: the
  # -I paths in $INCLUDE are relative to the root, and compiling from
  # elsewhere silently turns -Iinclude into a directory that does not exist.
  # `-o "$OUT/$b.o"` is what places the .gcno/.gcda in $OUT.
  say "[1/4] compiling instrumented: --coverage $COND"
  IOBJ=""
  for f in $SRC; do
    b=$(basename "${f%.*}")
    say "        $f"
    # shellcheck disable=SC2086
    "$CC" $COMMON $OPT --coverage $COND -c "$f" -o "$OUT/$b.o"
    IOBJ="$IOBJ $OUT/$b.o"
  done
  # shellcheck disable=SC2086
  "$CC" --coverage $SDKFLAGS $IOBJ -o "$OUT/$PROJECT-instr"
  say "[2/4] running $OUT/$PROJECT-instr (stdout -> $OUT/run.txt)"
  ( cd "$OUT" && "./$PROJECT-instr" > run.txt )
  say "[3/4] reading coverage with $GCOV -b"

  # -b is not optional: without it gcov emits no branch records at all and the
  # evidence silently carries no decision coverage. -g adds condition records.
  # Every unit in one export, so each function lands in its real file. Run
  # from the root as well, so the source paths recorded in the .gcno resolve.
  # gcov's stderr is captured and then always printed, pass or fail. Nothing in
  # this script discards a diagnostic: an error hidden here leaves an empty
  # coverage.json, and the mistake resurfaces three steps later as evidence
  # that is silently missing everything.
  # shellcheck disable=SC2086
  "$GCOV" -b $( [ -n "$COND" ] && echo -g ) --json-format --stdout "$OUT"/*.gcda \
      > "$OUT/coverage.json" 2>"$OUT/gcov.err" && rc=0 || rc=$?
  if [ -s "$OUT/gcov.err" ]; then
    echo "build: $GCOV said:" >&2
    sed 's/^/build:   /' "$OUT/gcov.err" >&2
  fi
  if [ "$rc" -ne 0 ]; then
    echo "build: $GCOV exited $rc; no coverage was read." >&2
    exit 1
  fi
  if [ ! -s "$OUT/coverage.json" ]; then
    echo "build: $GCOV produced no output, so it is probably not GNU gcov." >&2
    echo "build:   $GCOV is: $("$GCOV" --version 2>&1 | head -1)" >&2
    echo "build: on a Mac, plain 'gcov' is LLVM's and has no --json-format." >&2
    echo "build: set GCOV to the gcov matching $CC (e.g. gcov-16)." >&2
    exit 1
  fi
  rm -f "$OUT/gcov.err"
  say "      coverage.json is $(wc -c < "$OUT/coverage.json" | tr -d ' ') bytes"
  INSTR="--coverage $COND"
fi

# ── the object code under analysis: no instrumentation, plus stack usage ──
say "[4/4] building the object code under analysis (no instrumentation)"
for f in $SRC; do
  b=$(basename "${f%.*}")
  say "        $f"
  # shellcheck disable=SC2086
  "$CC" $COMMON $OPT -fstack-usage -c "$f" -o "$OUT/obj/$b.o"
  if [ -f "$OUT/obj/$b.su" ]; then
    mv "$OUT/obj/$b.su" "$OUT/$b.su"
  else
    echo "build: note -- $CC wrote no $b.su, so $b contributes no stack-usage" >&2
    echo "build: evidence (DO-178C 6.3.4.f). Check that -fstack-usage is supported." >&2
  fi
done
# shellcheck disable=SC2086
"$CC" $COMMON $OPT $SDKFLAGS "$OUT"/obj/*.o -o "$OUT/$PROJECT"

# On macOS, linking separate .o files leaves debug info behind a debug map and
# only a single-step compile+link runs dsymutil for you. Without the .dSYM the
# binary has no line table, every instruction lands in "no source line", and
# the projection is worthless. Gate on Darwin, not on dsymutil being present:
# LLVM ships a dsymutil on Linux too, and running it on an ELF binary (which
# keeps its debug info inline and needs no .dSYM) fails the build (#113). This
# mirrors examples/adapter.sh's link_debug.
if [ "$(uname -s)" = Darwin ] && command -v dsymutil >/dev/null 2>&1; then
  say "      dsymutil: linking the debug map into $OUT/$PROJECT.dSYM"
  # Status captured before the output is formatted: this is plain sh under
  # set -e, where a pipeline's status is its *last* command's -- piping
  # straight into sed would report whether sed worked, and a dsymutil
  # failure would sail through to the success banner (same rule as the
  # gcov step above).
  dsymutil "$OUT/$PROJECT" >"$OUT/dsymutil.log" 2>&1 && rc=0 || rc=$?
  [ -s "$OUT/dsymutil.log" ] && sed 's/^/build:   /' "$OUT/dsymutil.log" >&2
  rm -f "$OUT/dsymutil.log"
  if [ "$rc" -ne 0 ]; then
    say "dsymutil exited $rc. Without a .dSYM the binary has no line table,"
    say "every instruction lands in \"no source line\", and the object-code"
    say "projection is worthless. Fix this before trusting the evidence."
    exit 1
  fi
fi

# butterfly reads this to record how the evidence was produced (DO-178C
# §11.2: the build must be reproducible from what you wrote down).
printf 'compiler=%s\nflags_instr=%s\nflags_target=%s\nunits=%s\n' \
  "$("$CC" --version | head -1)" \
  "$COMMON $OPT $INSTR" \
  "$COMMON $OPT -fstack-usage" \
  "$(echo $SRC | tr ' ' ',')" > "$OUT/build-info.txt"

say "wrote      $(ls "$OUT" | wc -l | tr -d ' ') files into $OUT/"
say "next       butterfly doctor $OUT"
echo "ok: $OUT/"
