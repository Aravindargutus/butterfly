#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# Measured-coverage fixture: one crate, built twice, run twice.
#
#   1. HOST, instrumented (nightly, for branch regions) -> build/coverage.json
#      A bare-metal image has no filesystem to write a profile to, so the
#      source-level coverage comes from the host. Functions are matched to the
#      target image by demangled name, which is the same on both sides.
#   2. TARGET, aarch64-unknown-none-softfloat, NOT instrumented -> build/flight.elf
#      The image under analysis.
#   3. QEMU runs that image with -d in_asm,exec,nochain -> build/trace.log
#      `nochain` matters: without it QEMU chains translation blocks together
#      and stops logging the ones it jumps straight into, so counts and
#      branch-outcome edges would silently go missing.
#
# Both runs execute the same campaign() and both must exit 0.
#
# Nothing here is skipped quietly. A missing qemu or target is an error with
# the command that fixes it; callers that want to tolerate the absence (CI
# helpers) check for the tool themselves, before calling this.
set -eu
cd "$(dirname "$0")"
TARGET=aarch64-unknown-none-softfloat
QEMU="${QEMU:-qemu-system-aarch64}"
NIGHTLY="${BUTTERFLY_NIGHTLY:-nightly}"
# `-d in_asm,exec,nochain` is what makes this a measured-coverage fixture, and
# `nochain` is not optional (see the note above). Kept here, with the rest of
# the configuration, because support/matrix.toml cites this line as the
# evidence for measured coverage: near the top it does not move when a check
# is added further down, and a row whose evidence line drifts fails the build
# in whichever commit lands second (self-review of #208).
QEMU_FLAGS="-machine virt -cpu cortex-a53 -m 128M -nographic -no-reboot -semihosting-config enable=on,target=native -d in_asm,exec,nochain"

die() { echo "build: $*" >&2; exit 1; }

command -v "$QEMU" >/dev/null 2>&1 || die "'$QEMU' is not on PATH. This example cannot run without it:
build:   macOS:  brew install qemu
build:   Debian: sudo apt-get install -y qemu-system-arm
build: or set QEMU=/path/to/qemu-system-aarch64."
# Ask the pinned toolchain itself whether it can build the target, rather than
# rustup: a toolchain installed some other way has no rustup to ask.
[ -d "$(rustc --print sysroot)/lib/rustlib/$TARGET/lib" ] || die "the Rust target '$TARGET' is not installed for $(rustc -V).
build:   rustup target add $TARGET
build: (run it from inside this repository so the pinned toolchain is the one that gets it)"
rustc +"$NIGHTLY" -V >/dev/null 2>&1 || die "no '$NIGHTLY' toolchain: branch regions in the coverage export need one.
build:   rustup toolchain install nightly --component llvm-tools
build: or set BUTTERFLY_NIGHTLY to the one you have."

# The nightly's own llvm-profdata and llvm-cov, or a stop naming the fix;
# adapter.sh first, so build-info.txt names them (#298).
. ../adapter.sh
. ../rust-llvm-env.sh
rm -rf build && mkdir -p build

# 1. host, instrumented. The LLVM tools are nightly's own: a profile is only
#    readable by the LLVM that wrote it.
rustc +"$NIGHTLY" --edition 2021 --crate-name flight_qemu -g -Copt-level=0 \
      -Cinstrument-coverage -Zcoverage-options=condition \
      src/main.rs -o build/flight-instr
( cd build && LLVM_PROFILE_FILE=flight.profraw ./flight-instr ) || die "the host campaign failed (exit $?)"
"$LLVM_PROFDATA" merge -sparse build/flight.profraw -o build/flight.profdata
"$LLVM_COV" export -format=text -instr-profile=build/flight.profdata build/flight-instr > build/coverage.json

# 2. the image under analysis: pinned stable toolchain, no instrumentation.
#    rust-lld ships with the toolchain, so no cross linker is needed.
rustc --edition 2021 --crate-name flight_qemu --target "$TARGET" -g -Copt-level=0 \
      -Cpanic=abort -Clink-arg=-Tlink.ld \
      src/main.rs -o build/flight.elf

# 3. run it. The program ends with a semihosting SYS_EXIT carrying the
#    campaign's verdict, so QEMU's exit status IS the test result. The timeout
#    is for the day the image hangs instead: a build that never returns is
#    worse than one that fails.
# A marker the run itself removes on the way out. The watchdog below kills by
# PID, and a PID is only safe to signal while its owner still holds it: once
# this shell has reaped the child, the number belongs to whoever the kernel
# gives it to next. So the run is wrapped, the wrapper clears the marker
# BEFORE it exits -- before `wait` here can return and free the PID -- and the
# watchdog re-reads the marker every second and again before it fires. A
# finished run therefore stops the watchdog rather than being outlived by it
# (self-review of #208).
: > build/.running
# shellcheck disable=SC2086
( r=0; "$QEMU" $QEMU_FLAGS -D build/trace.log -kernel build/flight.elf </dev/null || r=$?; rm -f build/.running; exit "$r" ) &
qpid=$!
# The watchdog holds none of this script's descriptors: a caller piping our
# output would otherwise wait on the sleep long after QEMU had finished.
( i=0
  while [ "$i" -lt 60 ]; do
    [ -e build/.running ] || exit 0
    sleep 1; i=$((i + 1))
  done
  [ -e build/.running ] && kill "$qpid" 2>/dev/null
  exit 0 ) </dev/null >/dev/null 2>&1 &
wpid=$!
rc=0; wait "$qpid" || rc=$?
rm -f build/.running
{ kill "$wpid" && wait "$wpid"; } 2>/dev/null || true
[ "$rc" = 0 ] || die "the campaign failed on the target: $QEMU exited $rc (101 = panic, 1-8 = the failed expectation, 143 = killed after 60 s)"
[ -s build/trace.log ] || die "QEMU wrote no trace to build/trace.log"
grep -q '^Trace ' build/trace.log || die "build/trace.log has no 'Trace' exec records; this QEMU may have been built without TCG logging"

{
  printf 'compiler=%s\nflags_instr=%s\nflags_target=%s\ntarget=%s\nemulator=%s\nemulator_flags=%s\nllvm_bin=%s\n' \
    "$(rustc -V)" \
    "-g -Copt-level=0 -Cinstrument-coverage -Zcoverage-options=condition" \
    "--target $TARGET -g -Copt-level=0 -Cpanic=abort -Clink-arg=-Tlink.ld" \
    "$TARGET" "$("$QEMU" --version | head -1)" "$QEMU_FLAGS" "$LLVM_BIN_USED"
  recorded_tools
} > build/build-info.txt
echo "ok: $(ls build | tr '\n' ' ')"
