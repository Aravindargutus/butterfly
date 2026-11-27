#!/usr/bin/env bash
# Exercise the changes CLI on the evidence and baseline from the CI example.
set -euo pipefail
if [[ $(git rev-parse --is-shallow-repository) == true ]]; then
    echo "The CI change-history check requires a full checkout." >&2
    exit 1
fi
parents=$(git log -1 --format=%P HEAD)
if [[ -n "$parents" ]]; then
    set -- "$1" --baseline "$2" --since HEAD~1
else
    # The initial import has no prior revision. Include its changes.
    set -- "$1" --baseline "$2"
fi
exec cargo run --locked -q -- changes "$@"
