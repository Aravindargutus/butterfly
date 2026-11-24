#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# The template's failure paths, exercised for real. A build script's whole
# job on a bad day is to fail loudly; a harness that only ever watches the
# good day proves nothing about the day that matters.
set -eu
cd "$(dirname "$0")"

command -v clang >/dev/null 2>&1 || { echo "template-test: no clang; skipping" >&2; exit 0; }

W=$(mktemp -d "${TMPDIR:-/tmp}/butterfly-template-test.XXXXXX")
trap 'rm -rf "$W"' EXIT
fail() { echo "template-test: FAIL — $*" >&2; exit 1; }

mkdir -p "$W/proj/src" "$W/shim"
cp build.sh "$W/proj/"
sed -i.bak 's/^PROJECT=myproject/PROJECT=demo/' "$W/proj/build.sh" && rm -f "$W/proj/build.sh.bak"
printf 'int f(void){return 1;}\n' > "$W/proj/src/foo.c"
printf 'int f(void); int main(void){return f() - 1;}\n' > "$W/proj/src/bar.c"

# 1. the good day, so the failures below fail for the injected reason only
( cd "$W/proj" && ./build.sh ) >"$W/good.log" 2>&1 || fail "clean build broke: $(tail -3 "$W/good.log")"
[ -e "$W/proj/build/demo" ] || fail "no analysis binary from a clean build"

# 2. dsymutil fails -> the build must fail, keep its diagnostic, and show no
#    success banner. Piped-to-sed status once swallowed exactly this
#    (review of #75): the formatter succeeded, so set -e saw nothing.
printf '#!/bin/sh\necho "shim: injected dsymutil failure" >&2\nexit 42\n' > "$W/shim/dsymutil"
chmod +x "$W/shim/dsymutil"
if [ "$(uname -s)" = "Darwin" ]; then
  if ( cd "$W/proj" && PATH="$W/shim:$PATH" ./build.sh ) >"$W/dsym.log" 2>&1; then
    fail "a dsymutil failure sailed through to success"
  fi
  grep -q "injected dsymutil failure" "$W/dsym.log" || fail "the tool's own diagnostic was discarded"
  grep -q "dsymutil exited 42" "$W/dsym.log"        || fail "the failing status was not named"
  grep -q "^ok: " "$W/dsym.log" && fail "a failed build printed the success banner"
fi

# 3. two sources sharing a basename must refuse, not overwrite
mkdir -p "$W/proj/lib" && cp "$W/proj/src/foo.c" "$W/proj/lib/foo.c"
sed -i.bak 's|^SRC=.*|SRC="src/foo.c src/bar.c lib/foo.c"|' "$W/proj/build.sh" && rm -f "$W/proj/build.sh.bak"
if ( cd "$W/proj" && ./build.sh ) >"$W/dupe.log" 2>&1; then
  fail "two sources sharing a basename were silently allowed to overwrite"
fi
grep -q "share a basename" "$W/dupe.log" || fail "the collision was not named"

echo "template-test: ok (clean build, dsymutil failure, basename collision)"
