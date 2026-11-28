#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# Every job in .github/workflows/ci.yml, run here, step for step and in
# order. Two uses: check a branch before pushing, and stand in for CI when
# CI cannot run -- as during a billing outage, when "I ran it locally" has
# to mean something a reviewer can reproduce rather than something asserted.
#
# It is NOT a replacement for CI. This runs one host; CI runs Ubuntu and
# macOS, and butterfly has target-specific code (see `path_of` in
# crates/ingest/src/lib.rs). A green run here is evidence, not a gate.
#
# Usage: ./scripts/ci-local.sh    (exit 0 = every check this host can run passed)
set -uo pipefail
export CARGO_TERM_COLOR=never RUSTFLAGS="-D warnings"
# This script builds every fixture the tests need, so a suite that reports
# "the fixture is not here" is a broken contract, not an environment. Saying
# so up front makes the failure name the suite and the build script, instead
# of arriving as a `skipping:` line the grep below has to notice afterwards.
# The grep stays: it still catches anything that skips without going through
# the guard. Skips no build could fix -- the wrong OS, root, a toolchain that
# is not installed -- are unaffected by this (#227).
export BUTTERFLY_REQUIRE_FIXTURES=1

# One private working directory for every log and artifact, removed on exit.
# Fixed names in /tmp (`/tmp/cil.log`) are a symlink target an attacker can
# preplant to redirect our truncating writes, and two runs would clobber each
# other's logs -- `mktemp -d` is neither guessable nor shared.
WORK="$(mktemp -d "${TMPDIR:-/tmp}/butterfly-ci-local.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
LOG="$WORK/step.log"

# ...except the log of anything that failed, which is copied here before the
# trap can remove it. A red run used to leave fifteen lines on the terminal
# and nothing on disk, so an intermittent failure -- the kind a rerun will
# not reproduce -- was lost with its only occurrence (#98). Under target/,
# so it is already gitignored and `cargo clean` clears it; one directory per
# run, never reused, so a later run cannot overwrite an earlier failure.
# Created on the first failure only: a green run leaves nothing behind.
KEPT="target/ci-local/$(date +%Y%m%d-%H%M%S)-$$"
nstep=0
keep() {  # keep <file-or-dir> <name>: copy it into $KEPT and say where
  [ -e "$1" ] || return 0
  mkdir -p "$KEPT" && cp -R "$1" "$KEPT/$2" && echo "      kept: $KEPT/$2"
}
# A step's name as a file name: "cargo clippy -D warnings" -> "03-cargo-clippy-D-warnings".
logname() { printf '%02d-%s' "$nstep" "$(printf '%s' "$1" | tr -cs 'A-Za-z0-9._' '-')"; }

fail=0
step() {
  nstep=$((nstep + 1))
  printf '%-46s' "$1"
  local name="$1"; shift
  if "$@" >"$LOG" 2>&1; then echo "PASS"; return 0; fi
  echo "FAIL"; fail=1
  tail -15 "$LOG" | sed 's/^/      /'
  keep "$LOG" "$(logname "$name").log"
  return 1
}

# What a failed `cargo test` log is for: which tests failed and why. The
# last lines of it are only the harness's count and cargo's "to rerun pass"
# line; the panic message, the assertion and the test's name are all further
# up, among every other suite's output. This picks out, per test binary that
# failed: its name, each panic with the few lines of message after it, each
# FAILED test, and cargo's own error lines (which also cover a compile
# error, with its --> location). The whole log is kept beside it.
summarise_test_failure() {
  awk '
    /^ *Running / { bin = $0; sub(/^ */, "", bin); sub(/ \(.*\)$/, "", bin); shown = 0; next }
    function header() { if (!shown && bin != "") { print bin; shown = 1 } }
    /^thread .* panicked at / { header(); print; msg = 1; n = 0; next }
    msg {
      # The message ends at the backtrace note, a blank line, the harness
      # reporting a result, another panic -- or after eight lines, since
      # with --nocapture other tests can print into the middle of it.
      if ($0 == "" || /^note: / || /^test / || /^thread / || n >= 8) msg = 0
      else { print "  " $0; n++; next }
    }
    /^test .* \.\.\. FAILED$/ { header(); print; next }
    /^test result: FAILED/ { print; next }
    /^error(\[E[0-9]+\])?: / || /^ *--> / { print; next }
  ' "$1" >"$WORK/failures.txt"
  if [ -s "$WORK/failures.txt" ]; then
    head -40 "$WORK/failures.txt" | sed 's/^/      /'
    local more; more=$(( $(wc -l <"$WORK/failures.txt") - 40 ))
    [ "$more" -gt 0 ] && echo "      ... $more more lines; see the kept log"
  else
    # Nothing recognisable (cargo itself died, say): the tail beats nothing.
    tail -15 "$1" | sed 's/^/      /'
  fi
  return 0
}

echo "host: $(uname -s) $(uname -m)   rustc: $(rustc --version 2>/dev/null)"
echo "note: CI also runs ubuntu-24.04; this covers one host only."
echo

echo "== job: rustfmt + clippy =="
step "PR requirements checker tests" python3 scripts/test_pr_requirements.py
step "cargo fmt --all --check"            cargo fmt --all --check
# The viewer's own tests. They run the real app.js in a VM, so they catch
# things Rust cannot -- a resolution key written by one pane and read by
# another, for instance. Skipped with a *reported* skip where node is
# absent, never silently (review of #45).
if command -v node >/dev/null 2>&1; then
  for t in ui/*.test.cjs; do
    [ -e "$t" ] || continue
    step "node --test $t" node --test "$t"
  done
else
  printf '%-46s %s\n' "node --test ui/*.test.cjs" "SKIP (no node; install it to run the viewer tests)"
fi
step "cargo clippy -D warnings"           cargo clippy --workspace --all-targets --locked -- -D warnings

echo "== job: test =="
# CI builds every example it tests against before testing (see
# .github/workflows/ci.yml), and so does this script, in the same order;
# support/matrix.toml's checker holds the two lists equal. Building only
# examples/c here once left serve_requirements, the duplicate-static, the
# duplicate-C++ and the tags suites silently unexercised.
step "./examples/ozone/build.sh"            ./examples/ozone/build.sh
step "./examples/c/build.sh"                ./examples/c/build.sh
step "./examples/c-lib/build.sh"            ./examples/c-lib/build.sh
# the library as delivered, and the objects it was built from (#169)
step "./examples/c-lib/build-lib.sh"        ./examples/c-lib/build-lib.sh
# a switch that compiles to a jump table, at -O0 and -O2 (#171)
step "./examples/c-switch/build.sh"         ./examples/c-switch/build.sh
# C++ with and without exceptions and RTTI (#165)
step "./examples/cpp-exceptions/build.sh"   ./examples/cpp-exceptions/build.sh
step "./examples/c-header-inline/build.sh"  ./examples/c-header-inline/build.sh
# The converse fixture: a static with no coverage record at all, beside a
# covered extern of the same name. Without it dup_unprofiled_static.rs skips
# and the negative case of #206 goes unexercised (#237).
step "./examples/c-unprofiled-static/build.sh" ./examples/c-unprofiled-static/build.sh
step "./examples/cpp-header-inline/build.sh" ./examples/cpp-header-inline/build.sh
# The C++ single-file and library examples, so e2e_cpp_constructs runs rather
# than reporting a skip the "no skipped suites" check would fail on (#162).
step "./examples/cpp/build.sh"              ./examples/cpp/build.sh
step "./examples/cpp-lib/build.sh"          ./examples/cpp-lib/build.sh
# constructors/destructors, operators, lambdas, new/delete (#166)
step "./examples/cpp-constructs/build.sh"   ./examples/cpp-constructs/build.sh
# The Rust examples (#152). Their coverage build needs a nightly with
# llvm-tools; ci.yml installs nightly-2026-08-10 and passes it as
# BUTTERFLY_NIGHTLY, and this script honours the same variable (default:
# the toolchain named `nightly`). Their suites -- e2e_rust_constructs and
# example_numbers -- require these fixtures, so a host without a usable
# nightly cannot produce a complete run: a FAIL naming the remedy, as for
# ctags and gcc, not a skip.
RUST_NIGHTLY="${BUTTERFLY_NIGHTLY:-nightly}"
RUST_NIGHTLY_BIN="$(rustc +"$RUST_NIGHTLY" --print sysroot 2>/dev/null)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
if [ -x "$RUST_NIGHTLY_BIN/llvm-profdata" ]; then
  echo "rust nightly: $(rustc +"$RUST_NIGHTLY" -V)   (ci.yml pins nightly-2026-08-10)"
  step "./examples/rust/build.sh"           ./examples/rust/build.sh
  step "./examples/rust-lib/build.sh"       ./examples/rust-lib/build.sh
  # generics, dyn, closures, macro_rules! (#167); its suite is rust_constructs
  step "./examples/rust-constructs/build.sh" ./examples/rust-constructs/build.sh
else
  printf '%-46s%s\n' "./examples/rust{,-lib,-constructs}/build.sh" "FAIL — no '$RUST_NIGHTLY' toolchain with llvm-tools"
  echo "      rustup toolchain install nightly-2026-08-10 --profile minimal --component llvm-tools"
  echo "      and run with BUTTERFLY_NIGHTLY=nightly-2026-08-10 (the toolchain ci.yml uses)"
  fail=1
fi
step "./examples/c-per-test/build.sh"       ./examples/c-per-test/build.sh
# The call-graph fixture: depth, recursion, mutual recursion, an indirect
# call and an external one, so the expandable Calls graph has every node
# state to draw. Every other C example is flat (#100).
step "./examples/c-callgraph/build.sh"      ./examples/c-callgraph/build.sh
# dynamic frames, goto, a union, a bitfield, varargs and a control-flow
# macro: the C constructs no other example has (#168)
step "./examples/c-constructs/build.sh"     ./examples/c-constructs/build.sh
# The GCC example, on Linux only. Its one consumer is the pair of GCC doctor
# tests in crates/cli/tests/doctor.rs, which are compiled on Linux alone
# (#89) -- and that stays the decision even on a Mac with a real Homebrew
# GCC that would build this and pass them (#92; the reasoning is in ci.yml).
# So off Linux nothing would read the fixture, and building it there was
# work that tested nothing. The skip is still *reported*, because a check
# that quietly did not run is the thing this script exists to avoid (#45).
if [ "$(uname -s)" = "Linux" ]; then
  GCC_FOR_EXAMPLE=""
  for c in "${CC_GCC:-}" gcc-16 gcc-15 gcc-14 gcc; do
    [ -n "$c" ] || continue
    command -v "$c" >/dev/null 2>&1 || continue
    # A `gcc` that is really clang has a gcov that cannot write the JSON
    # this needs.
    "$c" --version 2>/dev/null | head -1 | grep -qi "clang" && continue
    GCC_FOR_EXAMPLE="$c"; break
  done
  if [ -n "$GCC_FOR_EXAMPLE" ]; then
    step "./examples/c-gcc/build.sh (CC=$GCC_FOR_EXAMPLE)" env CC="$GCC_FOR_EXAMPLE" ./examples/c-gcc/build.sh
  else
    # The GCC doctor tests are compiled in here and require this fixture,
    # so a host with no real gcc cannot produce a complete run -- that is a
    # failure, not a skip (#89).
    printf '%-45s %s\n' "./examples/c-gcc/build.sh" "FAIL - no real gcc, and this host runs the GCC doctor tests"
    echo "      install gcc, or set CC_GCC=<gcc>."
    fail=1
  fi
else
  printf '%-45s %s\n' "./examples/c-gcc/build.sh" "SKIP (nothing consumes it here: the GCC doctor tests are Linux-only by decision -- #92)"
fi
# The measured-coverage fixture (#164): a bare-metal AArch64 image run under
# QEMU, its trace imported beside the host's coverage export. It is the only
# thing in the repository that exercises `ingest --trace` on a real trace.
# ci.yml does NOT build it -- QEMU, a second Rust toolchain and a cross target
# are minutes nobody has asked to be billed for -- so this script is where it
# is verified, and a host that cannot run it says so by name. QEMU_SKIP is
# read again below: the e2e_rust_qemu suite prints a `skipping:` line when
# this fixture is absent, and that line is excused only when this host could
# not have built it.
QEMU_SKIP=""
QEMU_TARGET=aarch64-unknown-none-softfloat
if ! command -v "${QEMU:-qemu-system-aarch64}" >/dev/null 2>&1; then
  QEMU_SKIP="no qemu-system-aarch64; brew install qemu, or apt-get install qemu-system-arm"
elif [ ! -d "$(rustc --print sysroot)/lib/rustlib/$QEMU_TARGET/lib" ]; then
  QEMU_SKIP="no $QEMU_TARGET target for the pinned toolchain; rustup target add $QEMU_TARGET"
elif ! rustc +"${BUTTERFLY_NIGHTLY:-nightly}" -V >/dev/null 2>&1; then
  QEMU_SKIP="no nightly toolchain for the coverage export; rustup toolchain install nightly --component llvm-tools"
fi
if [ -z "$QEMU_SKIP" ]; then
  step "./examples/rust-qemu/build.sh"      ./examples/rust-qemu/build.sh
else
  printf '%-46s %s\n' "./examples/rust-qemu/build.sh" "SKIP ($QEMU_SKIP) -- measured coverage is NOT verified by this run"
fi
step "./templates/test.sh"                  ./templates/test.sh

# examples/c/build/evidence.json is what impact_cli, changes_cli,
# resolutions_loading, serve_auth and serve_startup look for (see their
# `fn evidence()`): CI's own "documented workflow" step writes it there, but
# that step runs *after* `cargo test`, and build.sh above just deleted
# whatever an earlier run had left. Without it those suites skip -- quietly,
# since `cargo test` only shows a passing test's stdout with --nocapture.
# Producing it before the test step, at the exact path they check, is what
# variant_sidecars.rs and evidence_limits.rs already do per-test to avoid the
# same gap; doing it once here means every suite gets the real thing instead.
step "ingest evidence for the CLI/server suites" \
  cargo run --locked -q -- ingest examples/c/build --source-root examples/c -o examples/c/build/evidence.json

# CI installs Universal Ctags so the `butterfly tags` suite runs rather than
# skips. Absence is a FAILED prerequisite here too, for the same reason as
# cargo-audit below: a script that quietly tests less than CI is not a mirror.
if command -v ctags >/dev/null 2>&1; then
  step "ctags --version" ctags --version
else
  printf '%-46s%s\n' "universal-ctags" "FAIL — not installed"
  echo "      CI runs the tags suite against it; install it to mirror CI (e.g. brew install universal-ctags)"
  fail=1
fi

# CI installs Subversion so the SVN source-revision suite (#301) runs rather
# than skips. Absence is a FAILED prerequisite, for the same reason as ctags.
if command -v svn >/dev/null 2>&1 && command -v svnadmin >/dev/null 2>&1 && command -v svnversion >/dev/null 2>&1; then
  step "svn --version" svn --version --quiet
else
  printf '%-46s%s\n' "subversion" "FAIL — not installed"
  echo "      CI runs the SVN source-revision suite against it; install it to mirror CI (e.g. brew install subversion)"
  fail=1
fi

step "cargo build --locked --all-targets" cargo build --locked --all-targets

# The real-browser checks (ui/browser, #69). They need the workspace binary,
# the c-per-test fixture, the exploratory variants, and Playwright's Chromium.
# The first three exist by now; the browser is an opt-in install, so its
# absence is a *reported* skip naming what to run -- never silent, and never
# counted as a pass.
if [ -d ui/browser/node_modules/playwright ]; then
  # The variants suite needs the exploratory multi-variant output, which
  # neither this script nor ci.yml's test job builds for the Rust suites.
  # Build it here so the suite runs instead of reporting a skip.
  [ -d examples/c/variants ] || step "butterfly build (variants fixture)" \
    ./target/debug/butterfly build examples/c/butterfly-build.toml
  # Its screenshots, server logs and results.json are written under $WORK
  # too; on a failure they are the evidence, so they are kept with the log.
  step "node ui/browser/run.cjs" env BROWSER_CHECK_OUT="$WORK/browser" \
    node ui/browser/run.cjs || keep "$WORK/browser" "$(logname browser-checks)"
else
  printf '%-46s %s\n' "ui/browser real-browser checks" "SKIP (cd ui/browser && npm ci && npx playwright install chromium)"
fi
printf '%-46s' "cargo test --locked"
# --nocapture: a skipped-but-passing test prints "skipping: ..." to stdout,
# and the harness only shows a passing test's output with --nocapture. Without
# it, this grep would run against a log that never contains the lines it is
# looking for -- a check that always passes is not a check.
if cargo test --locked -- --nocapture >"$WORK/test.log" 2>&1; then
  # One known exception: `the_entry_symbol_is_never_a_dead_code_candidate`
  # needs `butterfly build examples/c/butterfly-build.toml`'s exploratory
  # multi-variant output, which neither this script nor ci.yml's `test` job
  # produces -- it is not part of the certified fixture set either place, so
  # this is not a local/CI gap, just a suite this gate does not cover yet.
  #
  # Materialise the filtered skip lines to a file and test emptiness, rather
  # than `grep -v ... | grep -q`: `grep -q` exits at its first match, the
  # upstream producer then takes SIGPIPE, and under `set -o pipefail` the
  # pipeline's status is 141 -- so a real skip could read as "none" and pass.
  #
  # The second exception is conditional. The measured-coverage suite skips
  # when examples/rust-qemu is not built, and that is excused only on a host
  # this script already reported, above, as unable to build it. With QEMU
  # present the fixture was built, so the same line is a failure.
  grep "skipping:" "$WORK/test.log" | grep -Fv "butterfly build examples/c/butterfly-build.toml" >"$WORK/skips.all" || true
  if [ -n "$QEMU_SKIP" ]; then
    grep -Fv "examples/rust-qemu/build.sh" "$WORK/skips.all" >"$WORK/skips.txt" || true
  else
    cp "$WORK/skips.all" "$WORK/skips.txt"
  fi
  if [ -s "$WORK/skips.txt" ]; then
    echo "FAIL (a suite was skipped)"; sed 's/^/      /' "$WORK/skips.txt"; fail=1
    keep "$WORK/test.log" cargo-test.log
  else
    echo "PASS ($(grep -c 'test result: ok' "$WORK/test.log") suites, $(grep 'test result:' "$WORK/test.log" | awk '{s+=$4} END {print s}') tests)"
    grep -Fq "butterfly build examples/c/butterfly-build.toml" "$WORK/test.log" && echo "      note: the exploratory multi-variant suite skipped -- not part of ci.yml's fixture set either"
    [ -n "$QEMU_SKIP" ] && echo "      note: the measured-coverage suite skipped -- $QEMU_SKIP"
  fi
else
  echo "FAIL"; fail=1
  summarise_test_failure "$WORK/test.log"
  keep "$WORK/test.log" cargo-test.log
fi

echo "== job: documented workflow, end to end =="
D="$WORK/e2e"; mkdir -p "$D"
step "ingest"   cargo run --locked -q -- ingest examples/c/build --source-root examples/c -o "$D/e.json"
step "summary"  cargo run --locked -q -- summary "$D/e.json"
step "baseline" cargo run --locked -q -- baseline "$D/e.json" -o "$D/b.json"
step "changes"  bash scripts/ci-changes.sh "$D/e.json" "$D/b.json"
step "impact"   cargo run --locked -q -- impact "$D/e.json" --json
step "report"   cargo run --locked -q -- report "$D/e.json" -o "$D/r.html"
printf '%-46s' "report is non-empty"; [ -s "$D/r.html" ] && echo "PASS" || { echo "FAIL"; fail=1; }

echo "== job: build-system identity (#170) =="
# CMake, make and Bazel build the same sources as examples/c-lib; the evidence
# must be identical. verify-build-systems.sh SKIPs an absent build tool rather
# than passing it. Not in ci.yml: Bazel's first build is minutes and fetches a
# toolchain, which the billed hosted runners should not carry (the repo trims
# CI minutes deliberately); cmake+make are cheap enough to add there once
# billing is restored, Bazel is not.
# Run outside step(): step() prints only PASS and discards the log on success,
# which would hide exactly what #170 requires to be visible -- which build
# systems were verified and which were SKIPped for an absent tool. A silent
# "PASS" here reads as "all build systems agree" when it can mean "the only one
# installed agrees". So surface each system's own verdict even on success.
if [ -x scripts/verify-build-systems.sh ]; then
  nstep=$((nstep + 1))
  printf '%-46s' "verify-build-systems.sh"
  if ./scripts/verify-build-systems.sh >"$LOG" 2>&1; then
    echo "PASS"
    grep -E '^(cmake|make|bazel|bazelisk) +(PASS|SKIP|FAIL)' "$LOG" | sed 's/^/      /'
  else
    echo "FAIL"; fail=1
    tail -20 "$LOG" | sed 's/^/      /'
    keep "$LOG" "$(logname verify-build-systems.sh).log"
  fi
else
  printf '%-46s%s\n' "verify-build-systems.sh" "FAIL (missing or not executable)"; fail=1
fi

echo "== job: cargo audit =="
# CI runs cargo audit as a required job, so a mirror that skips it is not a
# mirror. Absence is a FAILED prerequisite, not a pass -- otherwise the whole
# green result understates what CI checks, which is the one thing this script
# exists to avoid.
if command -v cargo-audit >/dev/null 2>&1; then
  step "cargo audit" cargo audit
else
  printf '%-46s%s\n' "cargo audit" "FAIL — cargo-audit not installed"
  echo "      CI runs this job; install it to mirror CI:  cargo install cargo-audit --locked"
  fail=1
fi

echo
[ $fail -eq 0 ] && echo "ALL LOCAL CHECKS PASSED" || echo "SOME CHECKS FAILED"
[ -d "$KEPT" ] && echo "logs of what failed are kept in $KEPT/"
exit $fail
