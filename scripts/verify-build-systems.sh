#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# Assert the identity claim in examples/build-systems/README.md: CMake, make
# and Bazel build the SAME sources as examples/c-lib, so the evidence butterfly
# produces from each must match what c-lib's own build.sh produces (#170).
#
# The reference is computed live from a fresh c-lib build, never hard-coded:
# the claim this checks is "every build system agrees with build.sh", and a
# typed-in number could not tell agreement from two matching mistakes.
#
# A build system whose tool is not installed is SKIPPED and named, never
# passed (a skip that reads as success is exactly the hole this repo refuses
# elsewhere -- review of #45). Bazel's first build is slow; it is the last
# step so the fast ones report first.
#
# Usage: ./scripts/verify-build-systems.sh
#   exit 0 = every installed build system produced evidence identical to c-lib
#   exit 1 = a mismatch, or a build failure
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || { echo "cannot cd to repo root $ROOT"; exit 1; }
# The comparison reads the evidence JSON with python3. Without it every build
# system would compare as FAIL ("no evidence produced") when the real problem
# is a missing tool -- an error, not a verdict on the build systems. Say so.
command -v python3 >/dev/null 2>&1 || { echo "this script needs python3 to compare evidence; install it and re-run"; exit 1; }
WORK="$(mktemp -d "${TMPDIR:-/tmp}/butterfly-bs.XXXXXX")"
trap 'chmod -R u+w "$WORK" 2>/dev/null; rm -rf "$WORK"' EXIT
fail=0

# The whole comparison in one place: the fields that make evidence "identical"
# -- counts, not the symbol *order*, which is a build system's own business
# (link order differs and that is expected). Prints eight space-separated
# integers, or nothing on error.
tuple() {
  python3 - "$1" <<'PY' 2>/dev/null
import json, sys
d = json.load(open(sys.argv[1]))
s = d["summary"]
print(len(d["files"]), len(d["functions"]), len(d.get("symbols", [])),
      s["linesCovered"], s["linesTotal"], s["branchesCovered"], s["branchesTotal"], s["gapsTotal"])
PY
}

LABELS=(files functions symbols linesCovered linesTotal branchesCovered branchesTotal gaps)

# Compare a build system's evidence against the reference, field by field.
compare() {
  local name="$1" got="$2"
  printf '%-30s' "$name"
  if [ -z "$got" ]; then
    echo "FAIL (no evidence produced)"; fail=1; return 1
  fi
  if [ "$got" = "$REF" ]; then
    echo "PASS (identical to c-lib)"; return 0
  fi
  echo "FAIL (differs from c-lib)"; fail=1
  local -a r g
  read -ra r <<<"$REF"; read -ra g <<<"$got"
  for i in "${!LABELS[@]}"; do
    [ "${r[$i]}" != "${g[$i]}" ] && printf '      %-16s c-lib %s  vs  %s %s\n' "${LABELS[$i]}" "${r[$i]}" "$name" "${g[$i]}"
  done
  return 1
}

# The altered mode (#245): the same directory with one .su taken away must
# be refused, naming the -fstack-usage its build-info declares. Identical
# numbers show an adapter can produce the evidence; this shows the evidence
# stays attributable when the adapter loses a piece of it.
altered() {
  local name="$1" dir="$2" root="$3" alt="$WORK/alt-$1"
  printf '%-30s' "$name (one .su removed)"
  rm -rf "$alt"; cp -R "$dir" "$alt"
  # Bazel's outputs are read-only, and a copy keeps their modes
  chmod -R u+w "$alt"
  local su; su="$(find "$alt" -maxdepth 1 -name '*.su' | sort | head -1)"
  if [ -z "$su" ]; then echo "FAIL (no .su to remove)"; fail=1; return 1; fi
  rm -f "$su"
  if ( cd "$root" && "$BF" ingest "$alt" --source-root . -o "$WORK/alt-$name.json" ) >"$WORK/alt-$name.log" 2>&1; then
    echo "FAIL (accepted evidence with a stack record missing)"; fail=1; return 1
  fi
  if grep -q -- '-fstack-usage' "$WORK/alt-$name.log"; then
    echo "PASS (refused)"
  else
    echo "FAIL (refused, but not for the missing .su)"; tail -5 "$WORK/alt-$name.log" | sed 's/^/      /'; fail=1
  fi
}

# The tools that shaped the evidence are named in build-info.txt by the path
# that ran (#289): the LLVM that read the profile and, on macOS, the dsymutil
# that made the line table. PATH chose them, so a bare name says nothing.
recorded() {
  local name="$1" info="$2/build-info.txt" bin dsym
  printf '%-30s' "$name (tools recorded)"
  bin="$(sed -n 's/^llvm_bin=//p' "$info" 2>/dev/null)"
  dsym="$(sed -n 's/^dsymutil_path=//p' "$info" 2>/dev/null)"
  if [[ "$bin" != /* || ! -x "$bin/llvm-profdata" ]]; then
    echo "FAIL (no llvm_bin naming the profile reader)"; fail=1; return 1
  fi
  if [ "$(uname -s)" = Darwin ] && [[ "$dsym" != /* || ! -x "$dsym" ]]; then
    echo "FAIL (no dsymutil_path naming the dSYM's maker)"; fail=1; return 1
  fi
  echo "PASS"
}

# What produced coverage.json, under the keys `butterfly build` writes
# (#298): the compiler that wrote the profile, and the llvm-profdata and
# llvm-cov that read it, each by the path that ran and by its version. Asked
# of Bazel too, which `recorded` is not: it runs the dsymutil on PATH.
coverage_recorded() {
  local name="$1" info="$2/build-info.txt" bin key val
  printf '%-30s' "$name (coverage tools)"
  bin="$(sed -n 's/^llvm_bin=//p' "$info" 2>/dev/null)"
  for key in compiler_instr compiler_instr_path llvm_profdata llvm_cov; do
    val="$(sed -n "s/^$key=//p" "$info" 2>/dev/null)"
    if [ -z "$val" ]; then
      echo "FAIL (no $key naming what produced coverage.json)"; fail=1; return 1
    fi
  done
  for key in llvm_profdata llvm_cov; do
    val="$(sed -n "s/^${key}_path=//p" "$info" 2>/dev/null)"
    if [ "$val" != "$bin/$(printf '%s' "$key" | tr _ -)" ]; then
      echo "FAIL (${key}_path is '$val', not the one in llvm_bin)"; fail=1; return 1
    fi
  done
  echo "PASS"
}

# A clang whose own installation has no llvm-profdata answers
# -print-prog-name with one found on PATH -- another LLVM, of any version. The
# adapters must refuse that pair rather than read the profile with it (#289).
# The stand-in is the real clang in every respect but that answer.
UNPAIRED="$WORK/unpaired"
mkdir -p "$UNPAIRED/elsewhere"
for t in llvm-profdata llvm-cov; do printf '#!/bin/sh\nexit 0\n' >"$UNPAIRED/elsewhere/$t"; chmod +x "$UNPAIRED/elsewhere/$t"; done
printf '#!/bin/sh\ncase "$1" in\n  -print-prog-name=llvm-*) echo "%s/${1#-print-prog-name=}" ;;\n  *) exec "%s" "$@" ;;\nesac\n' \
  "$UNPAIRED/elsewhere" "$(command -v clang)" >"$UNPAIRED/clang"
chmod +x "$UNPAIRED/clang"
unpaired() {
  local name="$1"; shift
  printf '%-30s' "$name (unpaired LLVM)"
  if "$@" >"$WORK/unpaired-$name.log" 2>&1; then
    echo "FAIL (accepted $UNPAIRED/elsewhere as the compiler's LLVM)"; fail=1; return 1
  elif grep -q 'llvm-profdata' "$WORK/unpaired-$name.log"; then
    echo "PASS (refused)"
  else
    echo "FAIL (refused, but not for the profile reader)"; tail -5 "$WORK/unpaired-$name.log" | sed 's/^/      /'; fail=1
  fi
}

# Build butterfly once; every ingest below uses this binary.
echo "== building butterfly =="
cargo build --locked -q || { echo "cargo build failed"; exit 1; }
BF="$ROOT/target/debug/butterfly"

# ---- the reference: c-lib's own build.sh -----------------------------------
# Built from a copy, in the scratch directory: c-lib's build.sh starts with
# `rm -rf build`, and running it in place deleted the evidence the rest of
# the tree -- and the showcase capture that runs this script -- was using.
echo "== reference: examples/c-lib =="
mkdir -p "$WORK/ref/c-lib"
cp -R examples/c-lib/src examples/c-lib/include examples/c-lib/build.sh "$WORK/ref/c-lib/"
cp examples/llvm-env.sh examples/adapter.sh "$WORK/ref/"
if ! "$WORK/ref/c-lib/build.sh" >"$WORK/clib.log" 2>&1; then
  echo "c-lib build.sh failed (see below); cannot establish a reference"; tail -15 "$WORK/clib.log" | sed 's/^/  /'; exit 1
fi
"$BF" ingest "$WORK/ref/c-lib/build" --source-root "$WORK/ref/c-lib" -o "$WORK/ref.json" >/dev/null 2>&1
REF="$(tuple "$WORK/ref.json")"
[ -z "$REF" ] && { echo "could not read the reference evidence"; exit 1; }
printf '%-30s%s\n' "reference tuple" "$REF"
printf '   (%s)\n' "${LABELS[*]}"
altered "c-lib" "$WORK/ref/c-lib/build" "$WORK/ref/c-lib"
coverage_recorded "c-lib" "$WORK/ref/c-lib/build"

echo "== build systems =="

# ---- CMake ------------------------------------------------------------------
if command -v cmake >/dev/null 2>&1; then
  if cmake -S examples/build-systems/cmake -B "$WORK/cm" -DCMAKE_C_COMPILER=clang >"$WORK/cmake.log" 2>&1 \
     && cmake --build "$WORK/cm" >>"$WORK/cmake.log" 2>&1 \
     && "$BF" ingest "$WORK/cm" --source-root examples/c-lib -o "$WORK/cm.json" >>"$WORK/cmake.log" 2>&1; then
    compare "cmake" "$(tuple "$WORK/cm.json")"
    altered "cmake" "$WORK/cm" examples/c-lib
    recorded "cmake" "$WORK/cm"
    coverage_recorded "cmake" "$WORK/cm"
    unpaired "cmake" cmake -S examples/build-systems/cmake -B "$WORK/cm-unpaired" -DCMAKE_C_COMPILER="$UNPAIRED/clang"
  else
    printf '%-30sFAIL (build/ingest error)\n' "cmake"; tail -12 "$WORK/cmake.log" | sed 's/^/      /'; fail=1
  fi
else
  printf '%-30sSKIP (cmake not installed)\n' "cmake"
fi

# ---- make -------------------------------------------------------------------
# Built in its own (gitignored) build/ directory, not a temp one: the Makefile
# runs the instrumented binary as `./$(BUILD)/flight-instr`, which an absolute
# BUILD path turns into a broken `.//abs/path`. `make clean` first so the run
# is fresh, as CI would be.
if command -v make >/dev/null 2>&1; then
  if make -C examples/build-systems/make clean >"$WORK/make.log" 2>&1 \
     && make -C examples/build-systems/make >>"$WORK/make.log" 2>&1 \
     && "$BF" ingest examples/build-systems/make/build --source-root examples/c-lib -o "$WORK/mk.json" >>"$WORK/make.log" 2>&1; then
    compare "make" "$(tuple "$WORK/mk.json")"
    altered "make" examples/build-systems/make/build examples/c-lib
    recorded "make" examples/build-systems/make/build
    coverage_recorded "make" examples/build-systems/make/build
    # a dry run: the pairing is decided when the Makefile is read. Strip any
    # inherited LLVM_BIN from just this subprocess: the Makefile skips its
    # discovery when that override is set, so the stand-in compiler's unrelated
    # reader would be accepted and this negative check would pass falsely (#289
    # review). The positive build above keeps whatever LLVM_BIN the operator set.
    unpaired "make" env -u LLVM_BIN make -n -C examples/build-systems/make CC="$UNPAIRED/clang"
  else
    printf '%-30sFAIL (build/ingest error)\n' "make"; tail -12 "$WORK/make.log" | sed 's/^/      /'; fail=1
  fi
else
  printf '%-30sSKIP (make not installed)\n' "make"
fi

# ---- Bazel (slowest; last) --------------------------------------------------
# A valid install may expose only `bazelisk` (the version launcher), not
# `bazel`. Accepting either but then always running `bazel` reported a spurious
# build failure on such a host, so record which binary the check found and
# invoke that one throughout.
BAZEL=""
if command -v bazel >/dev/null 2>&1; then BAZEL=bazel
elif command -v bazelisk >/dev/null 2>&1; then BAZEL=bazelisk; fi
if [ -n "$BAZEL" ]; then
  # the same pairing rule as the shell adapters: the tools installed beside
  # clang, or the LLVM_BIN the operator names (#289)
  llvm_bin="$(LLVM_FROM=clang; . examples/llvm-env.sh; dirname "$LLVM_PROFDATA")" || { echo "no LLVM paired with clang for Bazel"; exit 1; }
  if ( cd examples/build-systems/bazel \
       && "$BAZEL" build //flight:evidence -c dbg --strip=never --spawn_strategy=local --define llvm_bin="$llvm_bin" ) >"$WORK/bazel.log" 2>&1 \
     && ( cd examples/c-lib \
          && "$BF" ingest ../build-systems/bazel/bazel-bin/flight/evidence --source-root . -o "$WORK/bz.json" ) >>"$WORK/bazel.log" 2>&1; then
    compare "$BAZEL" "$(tuple "$WORK/bz.json")"
    altered "$BAZEL" examples/build-systems/bazel/bazel-bin/flight/evidence examples/c-lib
    coverage_recorded "$BAZEL" examples/build-systems/bazel/bazel-bin/flight/evidence
    # and the rule itself refuses a build missing a mandatory option, at
    # analysis time, before anything is assembled
    for opt in "compilation mode:-c fastbuild --define llvm_bin=$llvm_bin" "llvm_bin:-c dbg"; do
      label="${opt%%:*}"; flags="${opt#*:}"
      printf '%-30s' "$BAZEL (without $label)"
      # shellcheck disable=SC2086
      if ( cd examples/build-systems/bazel && "$BAZEL" build //flight:evidence $flags --strip=never --spawn_strategy=local ) >"$WORK/bz-$label.log" 2>&1; then
        echo "FAIL (built without it)"; fail=1
      elif grep -q 'butterfly_evidence_dir:' "$WORK/bz-$label.log"; then
        echo "PASS (refused)"
      else
        echo "FAIL (failed for another reason)"; tail -5 "$WORK/bz-$label.log" | sed 's/^/      /'; fail=1
      fi
    done
  else
    printf '%-30sFAIL (build/ingest error)\n' "$BAZEL"; tail -12 "$WORK/bazel.log" | sed 's/^/      /'; fail=1
  fi
else
  printf '%-30sSKIP (bazel not installed)\n' "bazel"
fi

echo
[ $fail -eq 0 ] && echo "ALL INSTALLED BUILD SYSTEMS MATCH c-lib" || echo "A BUILD SYSTEM DIFFERS FROM c-lib (see above)"
exit $fail
