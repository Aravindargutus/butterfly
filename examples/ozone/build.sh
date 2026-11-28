#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
set -eu
cd "$(dirname "$0")"
mkdir -p build
output=$(mktemp "$PWD/build/profile.XXXXXX")
trap 'rm -f "$output"' EXIT
cargo run --locked --quiet --bin butterfly -- ozone report.txt > "$output"
mv "$output" build/profile.json
