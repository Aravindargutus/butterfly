# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# Name the LLVM tools belonging to the compiler that will write the profile,
# as $LLVM_PROFDATA and $LLVM_COV. Sourced by the C and C++ example builds;
# the Rust ones pair with rustc instead and do their own.
#
# A raw profile carries a format version, and llvm-profdata refuses one it
# does not recognise with the famously unhelpful "error: no profile can be
# merged". So the tools that read the profile must come from the same LLVM as
# the compiler that wrote it.
#
# The sourcing script must set LLVM_FROM to the compiler it actually compiles
# with, and then compile with that same variable. That is the whole point: if
# the lookup asks one binary and the build runs another, the two can disagree
# and we are back to the bug this file exists to remove. An earlier version
# read $CC while the scripts ran `clang` regardless, so `CC=gcc` in the
# environment sent the lookup down the fallback path while the build still
# used clang -- the mismatch reintroduced by the very code meant to prevent
# it (review of #54). Requiring LLVM_FROM makes that unstatable.
: "${LLVM_FROM:?llvm-env.sh: set LLVM_FROM to the compiler this script builds with, and build with it}"

if [ -n "${LLVM_BIN:-}" ]; then
  # An explicit override: the operator has said which LLVM to read with, and
  # is answerable for the pairing.
  _llvm_bin="$LLVM_BIN"
else
  # Asking the compiler where its own tools live is the one answer that
  # cannot disagree with it, whether the toolchain is Apple's, Homebrew's or
  # a distro's.
  #
  # But only an answer from the compiler's own installation is that. When
  # its directory has no llvm-profdata, clang goes on to search PATH and
  # still answers with an absolute path -- to whatever LLVM, of whatever
  # version, came first there (#289). So the answer must sit in the
  # InstalledDir the compiler reports for itself.
  _pd=$("$LLVM_FROM" -print-prog-name=llvm-profdata 2>/dev/null || true)
  _own=$("$LLVM_FROM" --version 2>/dev/null | sed -n 's/^InstalledDir: //p')
  case "$_pd" in
    /*)
      _llvm_bin=$(dirname "$_pd")
      if [ -z "$_own" ] || [ "$_llvm_bin" != "$_own" ]; then
        echo "build: $LLVM_FROM has no llvm-profdata of its own (installed in: ${_own:-unknown});" >&2
        echo "build: it named $_pd, which it found on PATH, so the pair cannot be verified." >&2
        echo "build: Install the LLVM tools that go with $LLVM_FROM, or set LLVM_BIN" >&2
        echo "build: to the directory holding the llvm-profdata that matches it." >&2
        exit 1
      fi
      ;;
    *)
      # Anything else is the bare name echoed back: the compiler could not
      # find it. There is deliberately no fallback here. Reaching for some
      # other LLVM -- the rustup llvm-tools, say -- pairs a reader with a
      # writer it has no relation to, which is a coin flip that comes up
      # "no profile can be merged" the moment the two versions drift. Better
      # to stop and say so than to guess and be wrong later, quietly.
      echo "build: $LLVM_FROM cannot say where its llvm-profdata is, so the pair cannot be" >&2
      echo "build: verified. Install the LLVM tools that go with $LLVM_FROM, or set LLVM_BIN" >&2
      echo "build: to the directory holding the llvm-profdata that matches it." >&2
      exit 1
      ;;
  esac
  unset _pd _own
fi

LLVM_PROFDATA="$_llvm_bin/llvm-profdata"
LLVM_COV="$_llvm_bin/llvm-cov"
export LLVM_PROFDATA LLVM_COV

for _t in "$LLVM_PROFDATA" "$LLVM_COV"; do
  if [ ! -x "$_t" ]; then
    echo "build: $_t is missing or not executable" >&2
    echo "build: install the LLVM tools matching $LLVM_FROM, or set LLVM_BIN to their directory" >&2
    exit 1
  fi
done
# Name the reader in build-info.txt when examples/adapter.sh is writing one
# (sourced first for that reason); ci.yml sources this file on its own. So
# is the writer: LLVM_FROM is, by the contract above, the compiler that
# builds the instrumented binary (#298).
if command -v record_tool >/dev/null 2>&1; then
  record_tool llvm_bin "$_llvm_bin"
  record_coverage_compiler "$LLVM_FROM"
  record_profile_tools "$LLVM_PROFDATA" "$LLVM_COV"
fi
unset _t _llvm_bin
