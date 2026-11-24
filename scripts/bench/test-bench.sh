#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# The benchmark's refusals, tested: every way a sample can be unavailable has
# to stop the run, not turn into a number (review of #96).
#
# Each case runs the real scripts/bench.sh in a throwaway copy of the tree,
# against a stand-in `butterfly` whose commands succeed or fail on request and
# whose `serve` is a small loopback HTTP server that answers, errors or drops
# the connection on request. No build, no fixture, a few seconds.
#
# A failure case passes only if the run exits 3, says it aborted, leaves an
# existing results file byte-for-byte as it was, and leaves no server process
# or work directory behind. A control case with everything answering checks
# that the stand-ins themselves produce a valid row, so a refusal is the
# thing under test and not a broken harness.
#
# Usage: ./scripts/bench/test-bench.sh    (exit 0 = every case behaved)
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
T="$(mktemp -d "${TMPDIR:-/tmp}/butterfly-bench-test.XXXXXX")"
trap 'rm -rf "$T"' EXIT

# --- the throwaway tree -----------------------------------------------------
TREE="$T/tree"
mkdir -p "$TREE/scripts/bench" "$TREE/target/release" "$TREE/examples/c/build" "$T/bin"
cp "$ROOT/scripts/bench.sh" "$TREE/scripts/"
cp "$ROOT/scripts/bench/measure.py" "$ROOT/scripts/bench/gen-evidence.py" "$TREE/scripts/bench/"

# The smallest seed gen-evidence.py can replicate.
cat >"$TREE/examples/c/build/evidence.json" <<'JSON'
{"functions":[{"id":0,"name":"f","fileId":0,"lineEnd":3}],
 "files":[{"id":0,"lines":[1,2,3],"text":["int f(void) {","  return 0;","}"],"functionIds":[0]}]}
JSON

# bench.sh always builds; here there is nothing to build.
printf '#!/bin/sh\nexit 0\n' >"$T/bin/cargo"
printf '#!/bin/sh\necho "rustc stub"\n' >"$T/bin/rustc"
chmod +x "$T/bin/cargo" "$T/bin/rustc"

# The stand-in CLI. STUB_REFUSE names a command that fails the way the real
# one does on an over-cap fixture: non-zero, with its reason on stderr.
cat >"$TREE/target/release/butterfly" <<'SH'
#!/usr/bin/env bash
cmd="$1"
if [ -n "${STUB_REFUSE:-}" ] && [ "$cmd" = "$STUB_REFUSE" ]; then
  echo "error: evidence is larger than the cap (stub)" >&2
  exit 1
fi
case "$cmd" in
  summary|check|report) exit 0 ;;
  serve) shift; exec python3 "$STUB_SERVE" "$@" ;;
  *) echo "stub: unknown command $cmd" >&2; exit 2 ;;
esac
SH
chmod +x "$TREE/target/release/butterfly"

# The stand-in server. STUB_PATH is the endpoint that misbehaves and
# STUB_HOW is how: `503` answers with an error status, `empty` closes the
# connection without a response, `refused` shuts down once the readiness
# probe has been answered so the next request finds nothing listening, and
# `never` exits without ever listening. Anything else answers 200 throughout.
export STUB_SERVE="$T/stub-serve.py"
cat >"$STUB_SERVE" <<'PY'
import http.server
import os
import socketserver
import sys

how = os.environ.get("STUB_HOW", "")
bad = os.environ.get("STUB_PATH", "")
with open(os.environ["STUB_PIDFILE"], "w") as f:
    f.write(str(os.getpid()))
if how == "never":
    sys.exit("stub: not listening, by request")
host, port = sys.argv[sys.argv.index("--listen") + 1].rsplit(":", 1)


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == bad and how == "503":
            self.send_error(503, "stub: unavailable, by request")
            return
        if self.path == bad and how == "empty":
            self.close_connection = True
            return
        body = b'{"ok":true}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        if how == "refused" and self.path == "/api/variants":
            self.wfile.flush()
            os._exit(0)


# socketserver.TCPServer, not http.server.HTTPServer: HTTPServer.server_bind
# looks up its own name with socket.getfqdn(), a reverse DNS query that took
# 35s on a Mac with a slow resolver, and every readiness probe hung behind it.
class Server(socketserver.TCPServer):
    allow_reuse_address = True


Server((host, int(port)), Handler).serve_forever()
PY

# --- one run ----------------------------------------------------------------
fail=0
SENTINEL='{"previous":"run"}'

# run <label> <expect: ok|abort> [VAR=value ...]
run() {
  local label="$1" expect="$2"; shift 2
  local d="$T/case"
  rm -rf "$d"; mkdir -p "$d/tmp"
  printf '%s\n' "$SENTINEL" >"$d/out.json"
  env PATH="$T/bin:$PATH" TMPDIR="$d/tmp" BENCH_OUT="$d/out.json" BENCH_REPEATS=1 \
    STUB_PIDFILE="$d/serve.pid" "$@" "$TREE/scripts/bench.sh" 10 >"$d/log" 2>&1
  local rc=$? why=""

  if [ "$expect" = abort ]; then
    [ "$rc" -eq 3 ] || why="exited $rc, expected 3"
    [ -z "$why" ] && ! grep -q "benchmark aborted" "$d/log" && why="no abort message"
    [ -z "$why" ] && [ "$(cat "$d/out.json")" != "$SENTINEL" ] && why="the results file was rewritten"
  else
    [ "$rc" -eq 0 ] || why="exited $rc, expected 0"
    [ -z "$why" ] && ! python3 - "$d/out.json" <<'PY' && why="results are not one complete numeric row"
import json, sys
runs = json.load(open(sys.argv[1]))["runs"]
assert len(runs) == 1, runs
assert all(isinstance(v, (int, float)) for v in runs[0].values()), runs[0]
PY
  fi
  # Cleanup happens on every path: the server is gone and so is the work dir.
  if [ -z "$why" ] && [ -f "$d/serve.pid" ] && kill -0 "$(cat "$d/serve.pid")" 2>/dev/null; then
    why="the server was left running"
    kill "$(cat "$d/serve.pid")" 2>/dev/null
  fi
  [ -z "$why" ] && [ -n "$(ls -A "$d/tmp")" ] && why="the work directory was left behind"

  if [ -z "$why" ]; then
    printf '%-52s PASS\n' "$label"
  else
    printf '%-52s FAIL (%s)\n' "$label" "$why"
    sed 's/^/      /' "$d/log"
    fail=1
  fi
}

run "control: everything answers"                 ok
run "CLI: check rejects its input"                abort STUB_REFUSE=check
run "HTTP: /api/report answers 503"               abort STUB_PATH=/api/report STUB_HOW=503
run "HTTP: /api/evidence answers 503"             abort STUB_PATH=/api/evidence STUB_HOW=503
run "HTTP: /api/report drops the connection"      abort STUB_PATH=/api/report STUB_HOW=empty
run "HTTP: connection refused after readiness"    abort STUB_HOW=refused
run "HTTP: the server never listens"              abort STUB_HOW=never

echo
[ $fail -eq 0 ] && echo "all benchmark refusal cases behaved" || echo "SOME CASES FAILED"
exit $fail
