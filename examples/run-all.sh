#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# Every example, end to end: build -> ingest -> report -> serve, one viewer
# per example on its own port.
#
#   ./run-all.sh            build everything, serve everything, print the table
#   ./run-all.sh stop       kill the servers -- and confirm each one died
#   ./run-all.sh status     who is serving what, checked live
#
# This drives the build.sh examples AND the build-system integrations
# (build-systems/{make,cmake,bazel}) with the exact commands their READMEs
# document. One example failing must not take the rest down: failures are
# collected and reported at the end, per example.
set -u
cd "$(dirname "$0")"
say() { echo "run-all: $*" >&2; }

BUTTERFLY="${BUTTERFLY:-butterfly}"
STATE=".run-all"
BASE_PORT="${BASE_PORT:-8820}"

EXAMPLES="
c
c-lib
c-header-inline
c-per-test
cpp
cpp-lib
cpp-header-inline
rust
rust-lib
rust-qemu
c-gcc
build-systems/make
build-systems/cmake
build-systems/bazel
"

# How each example builds, ingests and serves. The build-system integrations
# build the SAME sources as c-lib (one copy, four build systems -- see
# build-systems/README.md), so their source root is ../../c-lib; and bazel's
# outputs live under bazel-bin, so its evidence lands beside its BUILD file.
build_cmd() {
  case "$1" in
    build-systems/make)  (cd "$1" && make clean >/dev/null 2>&1; make) ;;
    build-systems/cmake) (cd "$1" && rm -rf build && cmake -S . -B build -DCMAKE_C_COMPILER=clang >/dev/null && cmake --build build) ;;
    build-systems/bazel) (cd "$1" && bazel build //flight:evidence -c dbg --strip=never --spawn_strategy=local \
                            --define llvm_bin="$(dirname "$(clang -print-prog-name=llvm-profdata)")") ;;
    *)                   (cd "$1" && ./build.sh) ;;
  esac
}
ingest_cmd() {
  case "$1" in
    build-systems/bazel) (cd "$1" && "$BUTTERFLY" ingest bazel-bin/flight/evidence --source-root ../../c-lib -o evidence.json) ;;
    build-systems/*)     (cd "$1" && "$BUTTERFLY" ingest build --source-root ../../c-lib -o build/evidence.json) ;;
    *)                   (cd "$1" && "$BUTTERFLY" ingest build --source-root . -o build/evidence.json) ;;
  esac
}
evidence_of() {
  case "$1" in
    build-systems/bazel) echo "evidence.json" ;;
    *)                   echo "build/evidence.json" ;;
  esac
}
slug() { echo "$1" | tr '/' '-'; }

status_cmd() {
  [ -d "$STATE" ] || { say "nothing recorded; not started here"; exit 0; }
  for f in "$STATE"/*.port; do
    [ -e "$f" ] || continue
    id=$(basename "$f" .port); port=$(cat "$f"); pid=$(cat "$STATE/$id.pid" 2>/dev/null || echo '?')
    if kill -0 "$pid" 2>/dev/null && curl -sf -o /dev/null "http://127.0.0.1:$port/api/variants"; then
      printf '  %-22s http://127.0.0.1:%s/   (pid %s)\n' "$id" "$port" "$pid"
    else
      printf '  %-22s DOWN (was port %s, pid %s)\n' "$id" "$port" "$pid"
    fi
  done
}

stop_cmd() {
  [ -d "$STATE" ] || { say "nothing to stop"; exit 0; }
  failed=0
  for f in "$STATE"/*.pid; do
    [ -e "$f" ] || continue
    id=$(basename "$f" .pid); pid=$(cat "$f")
    kill "$pid" 2>/dev/null || true
    # a kill nobody checked is how orphan servers happen: wait, then verify
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.3
    done
    if kill -0 "$pid" 2>/dev/null; then
      say "$id (pid $pid) did NOT die"; failed=1
    else
      say "$id stopped"
    fi
  done
  [ "$failed" = 0 ] && rm -rf "$STATE"
  exit "$failed"
}

case "${1:-start}" in
  stop) stop_cmd ;;
  status) status_cmd; exit 0 ;;
  start) ;;
  *) say "usage: $0 [start|stop|status]"; exit 2 ;;
esac

# the binary is needed to start; stop and status must work without it, or the
# one state you cannot leave is "servers up, binary missing"
command -v "$BUTTERFLY" >/dev/null 2>&1 || { say "no '$BUTTERFLY' on PATH (or set BUTTERFLY=/path/to/it)"; exit 1; }
# State left by a crashed run -- a reboot, a killed shell -- would otherwise
# refuse every future start. Recorded servers that are actually alive still
# block; state whose processes are all gone is just litter, and is cleared.
if [ -d "$STATE" ]; then
  alive=0
  for f in "$STATE"/*.pid; do
    [ -e "$f" ] || continue
    kill -0 "$(cat "$f")" 2>/dev/null && alive=$((alive + 1))
  done
  if [ "$alive" -gt 0 ]; then
    say "already started, $alive server(s) live (./run-all.sh status). Run ./run-all.sh stop first."
    exit 1
  fi
  say "clearing stale state from a previous run (nothing was still running)"
  rm -rf "$STATE"
fi
mkdir -p "$STATE"

port=$BASE_PORT
FAILURES=""
SERVED=""
for ex in $EXAMPLES; do
  id=$(slug "$ex")
  if [ ! -d "$ex" ]; then
    say "SKIP $ex (not in this checkout)"
    continue
  fi

  # c-gcc needs a real GCC; Apple's clang-in-a-gcc-suit does not count.
  if [ "$ex" = "c-gcc" ]; then
    real=""
    for c in gcc-16 gcc-15 gcc-14 gcc; do
      command -v "$c" >/dev/null 2>&1 || continue
      "$c" --version 2>/dev/null | head -1 | grep -qi clang && continue
      real="$c"; break
    done
    [ -z "$real" ] && { say "SKIP $ex (no real gcc)"; continue; }
    export CC="$real"
  fi
  # rust-qemu's build.sh refuses to run without QEMU, loudly; here, where one
  # example must not take the rest down, that is a named skip instead.
  case "$ex" in rust-qemu) command -v "${QEMU:-qemu-system-aarch64}" >/dev/null || { say "SKIP $ex (no qemu-system-aarch64)"; continue; } ;; esac
  case "$ex" in build-systems/cmake) command -v cmake >/dev/null || { say "SKIP $ex (no cmake)"; continue; } ;; esac
  case "$ex" in build-systems/bazel) command -v bazel >/dev/null || { say "SKIP $ex (no bazel)"; continue; } ;; esac

  say "── $ex ─────────────────────────────────────"
  if ! build_cmd "$ex" >"$STATE/$id.build.log" 2>&1; then
    say "BUILD FAILED: $ex (log: $STATE/$id.build.log)"
    FAILURES="$FAILURES $ex:build"
    [ "$ex" = "c-gcc" ] && unset CC
    continue
  fi
  [ "$ex" = "c-gcc" ] && unset CC

  if ! ingest_cmd "$ex" >"$STATE/$id.ingest.log" 2>&1; then
    say "INGEST FAILED: $ex (log: $STATE/$id.ingest.log)"
    FAILURES="$FAILURES $ex:ingest"
    continue
  fi
  ev=$(evidence_of "$ex")
  # Checked like the phases before it. Discarding this one meant a failing
  # report still produced the success table and advertised a report that was
  # never written, with the error nowhere in the logs.
  if ! (cd "$ex" && "$BUTTERFLY" report "$ev" -o "$(dirname "$ev")/report.html") >"$STATE/$id.report.log" 2>&1; then
    say "REPORT FAILED: $ex (log: $STATE/$id.report.log)"
    FAILURES="$FAILURES $ex:report"
    continue
  fi

  # An answer on the port is not proof our server is the one answering: a
  # viewer left over from an earlier run binds first, our process exits, and
  # the table would advertise the old one's stale evidence under this
  # example's name. Refuse the port instead of racing for it.
  while curl -sf -o /dev/null "http://127.0.0.1:$port/api/variants" 2>/dev/null; do
    say "port $port is already serving something; skipping past it"
    port=$((port + 1))
  done

  (cd "$ex" && exec "$BUTTERFLY" serve "$ev" --listen "127.0.0.1:$port") >"$STATE/$id.serve.log" 2>&1 &
  pid=$!
  echo "$pid" > "$STATE/$id.pid"
  echo "$port" > "$STATE/$id.port"

  # Started is not serving. Both conditions are required: the port answers,
  # and the process we started is still the one alive to be answering.
  ok=""
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if ! kill -0 "$pid" 2>/dev/null; then
      break # it died; whatever may answer that port now is not ours
    fi
    curl -sf -o /dev/null "http://127.0.0.1:$port/api/variants" && { ok=1; break; }
    sleep 0.3
  done
  if [ -z "$ok" ]; then
    say "SERVE FAILED: $ex (log: $STATE/$id.serve.log)"
    FAILURES="$FAILURES $ex:serve"
    # A server that never answered may still be running. Deleting its record
    # first is how it becomes an orphan nothing can stop -- which is the bug
    # this script's own `stop` exists to prevent. Kill it, confirm, and keep
    # the record if it will not die, so `stop` can try again.
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      for _ in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.3
      done
    fi
    if kill -0 "$pid" 2>/dev/null; then
      say "  its process $pid would not die; keeping the record so stop can retry"
    else
      rm -f "$STATE/$id.pid" "$STATE/$id.port"
    fi
  else
    SERVED="$SERVED $id:$port"
  fi
  port=$((port + 1))
done

echo
echo "───────────────────────── serving ─────────────────────────"
for s in $SERVED; do
  printf '  %-22s http://127.0.0.1:%s/\n' "${s%%:*}" "${s##*:}"
done
echo "───────────────────────────────────────────────────────────"
echo "reports:   beside each evidence.json (report.html)"
echo "stop all:  ./run-all.sh stop      status: ./run-all.sh status"
if [ -n "$FAILURES" ]; then
  echo "FAILED:   $FAILURES"
  exit 1
fi
