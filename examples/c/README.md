<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# C example

`demo.c` — a small avionics-flavoured module compiled with clang, with MC/DC
instrumentation and stack usage. This is the example the end-to-end test runs
against, so it is the one to copy when setting up a real C project.

```sh
./examples/c/build.sh
cargo run -- ingest examples/c/build --source-root examples/c -o examples/c/build/evidence.json
cargo run -- serve examples/c/build/evidence.json      # http://127.0.0.1:8787/
```

## What is in the program

| function | what it shows |
|---|---|
| `clamp_altitude` | two simple decisions; the `-O1`+ variants turn them into `csel`, so the object CFG has fewer branches than the source — a `predicated-decision` gap |
| `mode_permits_engage` | three conditions in one `&&` chain → MC/DC records; the demo's inputs prove 2 of 3 independence pairs, so the decision is **not** MC/DC covered and says so |
| `phase_name` | a `switch` over 7 cases, compiled here as a compare chain — on this host, 20 object blocks for source lines 30-40, with 7 `predicated-decision`, 5 `line-zero` and 1 `extra-object-branch` gap. The block and gap counts are this toolchain's; the shape is the point. A switch large enough to compile to a jump table is [`examples/c-switch`](../c-switch) |
| `checksum` | a loop, and a line-level `@satisfies LLR-CRC-004.1` tag inside the body |
| `average` | `if (n == 0)` — `main` only ever calls it with `n = 3`, so that outcome is never taken and the branch shows uncovered |
| `apply_op` + `OPS[]` | a function-pointer table: the call is the example's one `indirect-branch` gap, and `op_double`/`op_negate` have no direct callers — the Calls tab reaches them as *data table* callers, so they are correctly **not** dead code |
| `legacy_scale` | never called and never address-taken → the one genuine **dead-code candidate** |

## The two builds, and why

`build.sh` compiles the program **twice**, and this split is the point of the
whole tool:

1. `demo-instr` — `-fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc`.
   You run this to get coverage. It is *not* the code you certify: the
   instrumentation changes it.
2. `demo` — `-O0 -g -fstack-usage`, no instrumentation. This is the object code
   under analysis.

butterfly reads the coverage from (1) and *projects* it onto the disassembly of
(2), then reports every place the projection cannot be justified. Change the
target flags to whatever your project actually ships; `-O0` is only the default
here because it keeps the mapping easy to read the first time.

`build-info.txt` records both flag sets and lands in the evidence provenance,
which is what DO-178C §4.4.2 asks you to be able to show.

## Configuration

`butterfly.toml` (auto-discovered by `ingest`):

```toml
[requirements]
patterns = ['@satisfies\s+([A-Za-z0-9_.:/-]+)', '@verifies\s+([A-Za-z0-9_.:/-]+)']
scope    = "header"
lookback = 30
```

`scope = "header"` reads the comment block above each function. Line-level tags
are collected regardless — `checksum` carries `@satisfies LLR-CRC-004.1` on
line 48, and that chip shows on the instructions for that line in the Object
view either way. What `scope` controls is the function's own requirement list:
under `header` (here) `checksum` lists only `LLR-CRC-004`; under `both` (as the
C++ example uses) it lists `LLR-CRC-004.1` as well.

## Optimisation comparison

```sh
cargo run -- build examples/c/butterfly-build.toml --serve
```

`-O0`, `-O1`, `-O2` and `-Os` from one instrumented run, with a Compare page.

`clamp_altitude` is the one to look at. On this host, at `-O0` it is **8 object
blocks with no gaps** — the object CFG mirrors the source and the projection is
straightforward. At `-O2` it is **1 block with 2 `predicated-decision` gaps**:
the compiler made it branchless, so the source-level decisions have no
corresponding object branches to cover, and each needs a §6.4.4.3 justification
instead. `phase_name` tells the same story, 20 blocks down to 3.

That is the argument for object-code coverage at Level A in one screen: source
coverage looks identical at both levels, and the object code is not.

Marked EXPLORATORY throughout — this drives the compiler itself, so nothing it
produces is certification credit.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 10 |
| source files | 1 |
| lines covered | 57 / 69 |
| branch outcomes | 27 / 34 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
pinned there too, in the one table that holds every example's.
