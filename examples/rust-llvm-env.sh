# shellcheck shell=sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# The Rust examples' counterpart of llvm-env.sh: name the nightly's own LLVM
# tools, which are the only ones that can read the profile its
# -Cinstrument-coverage wrote, as $LLVM_PROFDATA and $LLVM_COV, and their
# directory as $LLVM_BIN_USED for build-info.txt. Sourced after NIGHTLY is set.
#
# The builds used to put nightly's tool directory first on PATH and call
# `llvm-profdata` by name. A nightly without the llvm-tools component then
# handed the profile to the next one PATH found -- the pinned stable's, a
# different LLVM -- and the merge failed with "no profile can be merged", or
# to any other llvm-profdata on PATH, and build-info.txt named none (#289).
: "${NIGHTLY:?rust-llvm-env.sh: set NIGHTLY to the toolchain that writes the profile}"

HOST="$(rustc -vV | sed -n 's/^host: //p')"
LLVM_BIN_USED="$(rustc +"$NIGHTLY" --print sysroot)/lib/rustlib/$HOST/bin"
if [ ! -x "$LLVM_BIN_USED/llvm-profdata" ] || [ ! -x "$LLVM_BIN_USED/llvm-cov" ]; then
  # `nightly` is whatever was last installed under that name and need not
  # have the component. Name the installed nightlies that do, so the fix can
  # be an environment variable rather than a download.
  _have=""
  for _tc in $(rustup toolchain list 2>/dev/null | awk '/^nightly/ {print $1}'); do
    [ -x "$(rustc +"$_tc" --print sysroot 2>/dev/null)/lib/rustlib/$HOST/bin/llvm-profdata" ] && _have="$_have ${_tc%"-$HOST"}"
  done
  echo "build: $LLVM_BIN_USED/llvm-profdata is missing: the '$NIGHTLY' toolchain has no llvm-tools component." >&2
  echo "build:   rustup component add llvm-tools --toolchain $NIGHTLY" >&2
  echo "build: or pick one that has it: BUTTERFLY_NIGHTLY=<toolchain> $0" >&2
  echo "build: installed nightlies with llvm-tools:${_have:- none}" >&2
  exit 1
fi
# shellcheck disable=SC2034 # read by the build that sources this
LLVM_PROFDATA="$LLVM_BIN_USED/llvm-profdata"
# shellcheck disable=SC2034
LLVM_COV="$LLVM_BIN_USED/llvm-cov"
# Name what produces coverage.json in build-info.txt when examples/adapter.sh
# is collecting it (sourced first for that reason), under the keys `butterfly
# build` writes (#298): the nightly rustc that writes the profile, and the
# llvm-profdata and llvm-cov that read it, by the path that ran and version.
if command -v record_profile_tools >/dev/null 2>&1; then
  record_tool compiler_instr "$(rustc +"$NIGHTLY" -V)"
  # the nightly's own rustc, not rustup's proxy on PATH: the proxy is one
  # file for every toolchain, so its path names none of them (#305 review)
  record_tool compiler_instr_path "$(rustc +"$NIGHTLY" --print sysroot)/bin/rustc"
  record_profile_tools "$LLVM_PROFDATA" "$LLVM_COV"
fi
