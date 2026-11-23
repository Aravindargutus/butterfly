#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# A synthetic evidence file of a requested size, for measuring how butterfly
# behaves on projects larger than any example in this repository.
#
# It replicates one real ingested function, so the *shape* is real -- the
# branches, decisions, conditions and object gaps are whatever the C example
# actually produced -- while the count is ours to choose. What it is not is a
# real project: every function is identical, so anything sensitive to the
# variety of real code (name lengths, MC/DC width, gap density) will read
# differently here. Treat the numbers as a scaling curve, not a forecast.
#
#   ./scripts/bench/gen-evidence.py 10000 /tmp/ev.json

import copy
import json
import os
import sys


def build(src, n):
    proto = src["functions"][0]
    f0 = src["files"][0]
    # Every generated function claims file 0, so that file has to list exactly
    # them: Evidence::validate() holds a file's function list to precisely the
    # functions naming it, and ids to their own position.
    max_line = max(1, max((fn.get("lineEnd") or 1) for fn in src["functions"]))
    lines = (f0["lines"] * ((n // max(1, len(f0["lines"]))) + 2))[: max_line + 8]
    text = (f0["text"] * ((n // max(1, len(f0["text"]))) + 2))[: len(lines)]

    functions = []
    for i in range(n):
        fn = copy.deepcopy(proto)
        fn["id"] = i
        fn["name"] = f"synthetic_fn_{i}"
        fn["fileId"] = 0
        if fn.get("object"):
            fn["object"]["symbol"] = f"_synthetic_fn_{i}"
            for g in fn["object"].get("gaps", []):
                # Gap keys embed an address; keep them distinct per function
                # so resolution lookups face realistic key cardinality.
                g["key"] = f"_synthetic_fn_{i}@{g.get('kind', 'k')}@{100 + i}"
        functions.append(fn)

    doc = copy.deepcopy(src)
    doc["functions"] = functions
    doc["files"] = [dict(f0, id=0, lines=lines, text=text, functionIds=list(range(n)))]
    return doc


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: gen-evidence.py <functions> <output.json>")
    n, out = int(sys.argv[1]), sys.argv[2]
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    seed = os.path.join(root, "examples/c/build/evidence.json")
    if not os.path.exists(seed):
        sys.exit(f"{seed} is missing: run ./examples/c/build.sh and ingest it first")
    with open(seed) as f:
        src = json.load(f)
    with open(out, "w") as f:
        json.dump(build(src, n), f)
    print(f"{n} functions -> {os.path.getsize(out) / 1048576:.1f} MB  {out}")


if __name__ == "__main__":
    main()
