#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# Regenerate the feature showcase (docs/showcase/) from this checkout (#154).
#
#   scripts/showcase.sh                 build, capture, write the page, check it
#   scripts/showcase.sh --only V02,V14  refresh some sections of an existing capture
#
# One command from a clean checkout:
#
#   1. cargo build            -- the page must show THIS checkout's binary,
#                                never an installed or stale one
#   2. capture.cjs            -- builds each example with the commands the page
#                                prints, runs every terminal command verbatim,
#                                drives the viewer and takes the screenshots
#   3. build.cjs              -- docs/showcase/index.html, and the single-file
#                                export docs/showcase/butterfly-showcase.html
#   4. verify.cjs             -- opens both from file:// with the network off,
#                                at 1460/1024/400 px, light and dark
#
# Needs: cargo, node, clang, and the browser checks' Playwright
# (cd ui/browser && npm ci && npx playwright install chromium). Universal
# Ctags is needed for V21 only; without it that section is rendered as not
# captured, with the reason.
#
# Exit status: 0 clean; 1 when a claim drifted (a command's exit status or a
# screenshot's expected text no longer matches support/features.toml) or the
# page fails its checks; 2 when a prerequisite is missing or the capture run
# did not finish, in which case docs/showcase/ is left untouched. A feature
# this HOST cannot show is not a failure: it is rendered as "not captured on
# this host".
set -eu
cd "$(dirname "$0")/.."
say() { echo "showcase: $*" >&2; }

command -v cargo >/dev/null 2>&1 || { say "cargo is not installed"; exit 2; }
command -v node  >/dev/null 2>&1 || { say "node is not installed"; exit 2; }
[ -d ui/browser/node_modules/playwright ] || {
  say "Playwright is not installed: cd ui/browser && npm ci && npx playwright install chromium"
  exit 2
}

say "cargo build --locked"
cargo build --locked

status=0
node scripts/showcase/capture.cjs "$@" || status=$?
# A capture that RAN to the end and found a drifted claim exits 1, and it has
# written capture.json: the page is still built from it, with the failures
# rendered as not captured, so that what went wrong can be read.
#
# A capture that did NOT finish exits 2 and has written nothing. Its first act
# was to empty docs/showcase/img/, so building now would publish the PREVIOUS
# run's capture.json -- 46 "captured" chips -- over an empty directory (#212
# review). Stop, and leave docs/showcase/ as it was.
if [ "$status" -gt 1 ]; then
  say "the capture run did not finish (status $status): docs/showcase/ is left as it was, and docs/showcase/img/ may be incomplete"
  exit "$status"
fi
[ -f docs/showcase/capture.json ] || { say "no capture was written"; exit 1; }
node scripts/showcase/build.cjs --single-file
node scripts/showcase/verify.cjs docs/showcase/index.html docs/showcase/butterfly-showcase.html || status=1

[ "$status" -eq 0 ] && say "ok: docs/showcase/index.html" || say "FAILED (status $status): read the lines above"
exit "$status"
