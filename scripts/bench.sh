#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# How butterfly scales, measured rather than guessed.
#
# Every example in this repository is tiny -- examples/c is ten functions --
# so nothing in the normal test run says what happens on a project with tens
# of thousands. This builds synthetic evidence at several sizes and times the
# commands and the server endpoints against each, so a change that makes
# something quadratic shows up as a number instead of as a complaint from
# somebody with a real codebase.
#
# Usage:
#   ./scripts/bench.sh                     # the default ladder
#   ./scripts/bench.sh 1000 20000          # your own sizes
#   BENCH_BASELINE=bench.json ./scripts/bench.sh   # compare against a saved run
#
# Release build, always: a debug build is ~6x slower here and measuring it
# tells you about rustc's codegen, not about butterfly.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
SIZES=("$@")
[ ${#SIZES[@]} -eq 0 ] && SIZES=(1000 10000 50000)
OUT="${BENCH_OUT:-$ROOT/bench.json}"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/butterfly-bench.XXXXXX")"
# Cleanup runs on every exit, aborts included: the server a failed run was
# measuring is stopped, not left listening, and the work directory goes.
srv=""
cleanup() {
  if [ -n "$srv" ]; then kill "$srv" 2>/dev/null; wait "$srv" 2>/dev/null; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

# Results are written into the work directory as the run goes and only
# replace $OUT once every size has been measured, so a run that aborts leaves
# the previous results where they were instead of a truncated file.
RESULTS="$WORK/results.json"
if [ ! -d "$(dirname "$OUT")" ]; then
  echo "cannot write $OUT: $(dirname "$OUT") does not exist"
  exit 2
fi

# The run replaces its output file, so a baseline that names the same file is
# gone by the time it would be compared against -- and the default output is
# `bench.json`, which is exactly what the usage line above tells people to
# pass as a baseline. Read the baseline into the work directory first, and
# compare against that copy.
BASELINE=""
if [ -n "${BENCH_BASELINE:-}" ]; then
  if [ ! -f "$BENCH_BASELINE" ]; then
    echo "baseline $BENCH_BASELINE does not exist"
    exit 2
  fi
  BASELINE="$WORK/baseline.json"
  cp "$BENCH_BASELINE" "$BASELINE"
fi

BIN="$ROOT/target/release/butterfly"
# Always build, never "only if missing": an existing binary may have been
# built from another branch, and then the numbers describe that branch's code
# while appearing to describe this one. Cargo is a no-op when it is current.
echo "building release (the only build worth measuring)..."
cargo build --release --locked -q || { echo "release build failed"; exit 1; }

if [ ! -f examples/c/build/evidence.json ]; then
  echo "examples/c/build/evidence.json is missing."
  echo "  ./examples/c/build.sh && $BIN ingest examples/c/build --source-root examples/c -o examples/c/build/evidence.json"
  exit 2
fi

# Timing lives in scripts/bench/measure.py: the interpreter has to start
# once, before the clock, or its ~30-50ms startup lands inside every sample
# and the small end of the ladder measures Python rather than butterfly.
# It also refuses to report a time for a command that failed.
# peak RSS in MB from BSD time(1)
peak_mb() { grep "maximum resident" "$1" | awk '{printf "%.0f", $1/1048576}'; }

free_port() { python3 -c "import socket;s=socket.socket();s.bind(('127.0.0.1',0));print(s.getsockname()[1]);s.close()"; }

# Every measurement that cannot be taken ends here. A missing number is not
# a zero, and a row with a gap in it is not a result: the run stops, says
# what failed and why, and the EXIT trap stops the server and removes the
# work directory. Called by the main shell after `||`, never from inside a
# command substitution, where `exit` would leave only the subshell.
abort() {  # $1 = what failed, $2 = file holding the reason
  echo
  echo "benchmark aborted: $1 on the $n-function fixture"
  [ -s "$2" ] && sed 's/^/      /' "$2"
  exit 3
}

# Plain variables, not an associative array: macOS ships bash 3.2, which
# has no `declare -A`, and this script has to run where ci-local.sh runs.
# A failed command is not a sample: measure.py exits non-zero and prints
# the program's own error, and the run stops rather than recording the time
# it took to reject its input as though that were the work.
# Returns non-zero on failure rather than exiting: `exit` inside a command
# substitution leaves the subshell, not the script, so the caller would
# have carried on and written the abort message into the results as though
# it were a timing.
time_cmd() { python3 scripts/bench/measure.py "${BENCH_REPEATS:-3}" "$BIN" "$@" 2>"$WORK/measure.err"; }

# One request to the server under test, printing the curl `-w` variable
# asked for. curl's own clock, so no interpreter sits in the sample. The same
# contract as time_cmd: a request that did not succeed is not a sample, so a
# transport failure (curl's exit status) or any status but 2xx returns
# non-zero with the reason in $WORK/http.err. Without this an HTTP 503 was
# timed and tabulated as the report endpoint's speed.
http_sample() {  # $1 = path, $2 = curl -w variable, e.g. %{time_total}
  local got rc code value
  got=$(curl -sS -o /dev/null -w "%{http_code} $2" "http://127.0.0.1:$port$1" 2>"$WORK/http.err")
  rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "curl exited $rc on GET $1" >>"$WORK/http.err"
    return 1
  fi
  code="${got%% *}"; value="${got#* }"
  case "$code" in
    2??) ;;
    *) echo "GET $1 answered HTTP $code" >>"$WORK/http.err"; return 1 ;;
  esac
  case "$value" in
    ''|*[!0-9.]*) echo "GET $1: curl reported no $2 (got '$value')" >>"$WORK/http.err"; return 1 ;;
  esac
  echo "$value"
}
to_ms() { awk '{printf "%.0f", $1*1000}' <<<"$1"; }

printf '%-9s %9s %9s %9s %9s %9s %10s %10s\n' \
  "functions" "MB" "summary" "check" "report" "peakRSS" "report/2nd"
echo "{\"host\":\"$(uname -s) $(uname -m)\",\"rustc\":\"$(rustc --version 2>/dev/null)\",\"runs\":[" > "$RESULTS"
first=1

for n in "${SIZES[@]}"; do
  ev="$WORK/ev-$n.json"
  # A size that cannot be generated is a hole in the curve, not a size to
  # skip quietly: the table and the comparison would read as complete.
  python3 scripts/bench/gen-evidence.py "$n" "$ev" >/dev/null 2>"$WORK/gen.err" \
    || abort "generating the fixture failed" "$WORK/gen.err"
  mb=$(python3 -c 'import os,sys;print(round(os.path.getsize(sys.argv[1])/1048576))' "$ev" 2>"$WORK/gen.err") \
    || abort "sizing the fixture failed" "$WORK/gen.err"
  # One discarded run first, outside the measurement. The first touch of a
  # freshly written 100 MB file pays for page cache and process start, and
  # that showed up as the first row of every table being slower than the row
  # for ten times the work.
  "$BIN" summary "$ev" >/dev/null 2>"$WORK/warm.err" \
    || abort "butterfly cannot read the fixture" "$WORK/warm.err"
  ms_summary=$(time_cmd summary "$ev") || abort "butterfly summary failed" "$WORK/measure.err"
  ms_check=$(time_cmd check "$ev" --min-lines 1) || abort "butterfly check failed" "$WORK/measure.err"
  ms_report=$(time_cmd report "$ev" -o "$WORK/r.html") || abort "butterfly report failed" "$WORK/measure.err"
  # `impact` is deliberately not measured. It requires change history from
  # `butterfly changes`, which reads git over a file's line range -- and the
  # synthetic fixture's source does not exist in any repository. Timing it
  # measured how fast the command rejects its input, which is precisely the
  # kind of number this harness exists not to produce.

  # Best of N for memory too, for the same reason as for time. Peak RSS on
  # this machine varied 765-951 MB across five identical runs -- a 24% spread
  # -- which was enough for a single sample to report a 29% "regression" from
  # a change that strictly removed allocations. The allocator returns pages
  # on its own schedule; the minimum is the closest thing to the demand.
  rss=""
  for _ in $(seq 1 "${BENCH_REPEATS:-3}"); do
    /usr/bin/time -l "$BIN" summary "$ev" >/dev/null 2>"$WORK/mem.txt" \
      || abort "butterfly summary failed under /usr/bin/time" "$WORK/mem.txt"
    one=$(peak_mb "$WORK/mem.txt")
    case "$one" in
      ''|*[!0-9]*) abort "/usr/bin/time reported no peak RSS" "$WORK/mem.txt" ;;
    esac
    if [ -z "$rss" ] || [ "$one" -lt "$rss" ]; then rss="$one"; fi
  done

  # The server: a cold report and a repeat, which is where caching shows.
  port=$(free_port 2>"$WORK/port.err") || abort "no free port for the server" "$WORK/port.err"
  "$BIN" serve "$ev" --listen "127.0.0.1:$port" --read-only --resolutions "$WORK/res.json" >"$WORK/serve.log" 2>&1 &
  srv=$!
  ready=0
  for _ in $(seq 1 120); do
    curl -sf -o /dev/null "http://127.0.0.1:$port/api/variants" && { ready=1; break; }
    # A server that has exited will not become ready; say so now, not in a
    # minute.
    kill -0 "$srv" 2>/dev/null || break
    sleep 0.5
  done
  # A server that never answered has no timings; before, this fell through
  # and the row recorded zeros for all three.
  [ "$ready" = 1 ] || abort "the server never became ready" "$WORK/serve.log"
  cold=$(http_sample /api/report '%{time_total}') || abort "GET /api/report (cold) failed" "$WORK/http.err"
  warm=$(http_sample /api/report '%{time_total}') || abort "GET /api/report (repeat) failed" "$WORK/http.err"
  evbytes=$(http_sample /api/evidence '%{size_download}') || abort "GET /api/evidence failed" "$WORK/http.err"
  cold=$(to_ms "$cold"); warm=$(to_ms "$warm")
  kill "$srv" 2>/dev/null; wait "$srv" 2>/dev/null; srv=""

  printf '%-9s %9s %8sms %8sms %8sms %8sMB %9sms\n' \
    "$n" "$mb" "$ms_summary" "$ms_check" "$ms_report" "$rss" "$warm"

  # Every field above was checked on the way in; nothing here has a default.
  [ $first -eq 0 ] && echo "," >> "$RESULTS"; first=0
  printf '{"functions":%s,"evidence_mb":%s,"summary_ms":%s,"check_ms":%s,"report_ms":%s,"peak_rss_mb":%s,"api_report_cold_ms":%s,"api_report_warm_ms":%s,"api_evidence_bytes":%s}' \
    "$n" "$mb" "$ms_summary" "$ms_check" "$ms_report" "$rss" "$cold" "$warm" "$evbytes" >> "$RESULTS"
done
echo "]}" >> "$RESULTS"
cp "$RESULTS" "$OUT" || { echo "cannot write $OUT"; exit 1; }
echo
echo "wrote $OUT"

# A saved run to compare against: the point of keeping the numbers.
if [ -n "$BASELINE" ]; then
  echo
  echo "against ${BENCH_BASELINE}:"
  python3 - "$BASELINE" "$OUT" <<'PY'
import json, sys
base = {r["functions"]: r for r in json.load(open(sys.argv[1]))["runs"]}
now = {r["functions"]: r for r in json.load(open(sys.argv[2]))["runs"]}
fields = ["summary_ms", "check_ms", "report_ms", "peak_rss_mb"]
shared = sorted(set(base) & set(now))
if not shared:
    print("  no sizes in common with the baseline")
    sys.exit(0)

moved = []
for n in shared:
    for f in fields:
        b, c = base[n].get(f, 0), now[n].get(f, 0)
        if b <= 0:
            continue
        delta = (c - b) / b * 100
        # 20%: wide enough to absorb a noisy laptop, tight enough that a real
        # regression in an O(n) path still shows.
        if abs(delta) >= 20:
            moved.append((n, f, b, c, delta))

if not moved:
    print(f"  no change beyond 20% across {len(shared)} size(s)")
else:
    for n, f, b, c, delta in moved:
        arrow = "slower" if delta > 0 else "faster"
        print(f"  {n:>7} {f:<14} {b:>8} -> {c:>8}  {delta:+.0f}% {arrow}")
    if any(d > 0 for *_, d in moved):
        print("\n  something got slower; if that is expected, refresh the baseline")
PY
fi
