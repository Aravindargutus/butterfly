<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# Worked examples

Six worked projects that show how to configure butterfly and what each view is
for: one **single-file** program and one **multi-file library** per language.
They are deliberately parallel, so you can compare how the same construct
compiles in C, C++ and Rust.

Which constructs each example demonstrates, which of them CI builds, and what
none of them shows is stated once, in the support table generated from
[`support/matrix.toml`](../support/matrix.toml) and rendered in the
[README](../README.md#what-is-verified-and-where); it is not repeated here.

## Run everything at once

```sh
./run-all.sh          # build -> ingest -> report -> serve, one port per example
./run-all.sh status   # who is serving what, checked live
./run-all.sh stop     # kill the servers, and confirm each one died
```

Every example — the `build.sh` ones and the
[`build-systems/`](build-systems) integrations (make, CMake, bazel) — built
end to end and served side by side from port 8820 up, so the same construct
can be compared across languages and build systems in adjacent browser tabs.
One example failing does not take the rest down; failures are named at the
end, with a log each under `.run-all/`.

## Single file — one function at a time

| | [`c/`](c) | [`cpp/`](cpp) | [`rust/`](rust) |
|---|---|---|---|
| entry point | `demo.c` | `flight.cpp` | `demo.rs` |
| compiler | `clang` | `clang++` (no exceptions/RTTI) | `rustc` (+`nightly` for coverage) |
| MC/DC | yes (`-fcoverage-mcdc`) | yes | **no** — see the Rust note below |
| function pointers | table of `op_fn` | vtable (virtual dispatch) | — |
| instantiations | — | one template, two types | — |

Each demonstrates the same core set: a three-condition decision, a
`switch`/`match`, a loop, an uncalled function, and requirement tags. The
uncalled function is instructive precisely because the three toolchains
disagree about it — clang and clang++ leave it in the binary as a dead-code
candidate you must justify, while rustc removes it outright.

**Start with [`c/`](c)** if you are new to the tool.

## Multiple files — a library

| | [`c-lib/`](c-lib) | [`cpp-lib/`](cpp-lib) | [`rust-lib/`](rust-lib) |
|---|---|---|---|
| units | 6 `.c` + 5 headers | 6 `.cpp` + 5 headers | 6 modules, one crate |
| functions | 18 | 24 | 18 |
| cross-file calls | 14 | 9 | 14 |
| dispatch | function-pointer table | vtable, anonymous namespace | function-pointer table |
| stack frames from | six `.su` files | six `.su` files | prologue analysis (no `.su` in Rust) |
| link step | yes — needs `dsymutil` | yes — needs `dsymutil` | none |

Each example's README ends with a **numbers** block — functions, source
files, lines, branch outcomes — that
`crates/ingest/tests/example_numbers.rs` ingests the example and checks
(#158). Instruction counts are not in it: they belong to the host's
toolchain, not to the example, and a README that prints one says so.

The same five-module flight library in all three: `util` (leaf helpers),
`state` (the shared global), `mode` (the MC/DC decision), `crc` (a leaf loop),
`sched` (a dispatch table), plus a driver. All three print identical output.

These exist for what a single file cannot show — the objectives that are about
*components talking to each other*:

* **Data coupling** (§6.4.4.2.c, A-7 obj 8) — a global one module owns and
  another writes.
* **Control coupling** — calls that cross a file boundary, labelled by file in
  the Calls tab.
* **Stack chains spanning modules**, reported **unbounded** where the dispatch
  table defeats static analysis (§6.3.4.f).
* **Uncovered vs. dead** — functions that are reachable but untested, next to
  one that genuinely nothing calls.

> **macOS:** linking separate `.o` files does not run `dsymutil`, and without
> the dSYM the binary has no line table — every instruction lands in "no source
> line" and the projection is worthless. The C and C++ library build scripts
> call `dsymutil` for you; see [`c-lib`](c-lib#the-two-builds-and-one-macos-trap).

### Same-named local symbols

[`c-header-inline/`](c-header-inline) is the smallest example of a shape the
libraries avoid: **two local symbols with the same name** in one image, at
different addresses, with different coverage. It carries both ways that
arises — a header `static inline` compiled into two units, and a `static`
sitting beside an extern of the same name — because the second is the one a
qualifier-only rule misses. butterfly tells the copies apart through the
DWARF, by the compilation unit that holds each subprogram. It doubles as the
regression fixture for exactly that resolution (#36).

[`cpp-header-inline/`](cpp-header-inline) is the C++ counterpart: two units
each holding an anonymous-namespace `scale`, plus a header `inline` compiled
into all three. llvm-cov qualifies an internal-linkage C++ function the same
way it does in C — `geom.cpp:_ZN12_GLOBAL__N_15scaleEi` — so the qualifier
has to be separated from the `::` of C++ namespacing rather than confused
with it.

[`c-unprofiled-static/`](c-unprofiled-static) is the converse case, and the
one that keeps the resolution honest in the *negative* direction: a third
unit's `static scale` that the profile has no record for at all, beside a
covered extern of the same name. Its caller's call really does leave the
covered source set, and the extern must not be allowed to say otherwise
(#237).

### What makes Rust different from C

[`rust-constructs/`](rust-constructs) has the Rust constructs the flight
programs leave out: a generic function monomorphised at two types, a trait
called through `dyn`, closures that capture and are passed to generic
functions, and a `macro_rules!` macro that expands to a decision. The README
records what butterfly shows for each one. That includes the two places
where rustc's coverage gives less than the source suggests: a closure with no
coverage record, and a macro's decision with no branch region, whose object
code lands on the macro's definition line.

### Which test exercised what

[`c-per-test/`](c-per-test) is the same program run three times, once per
test, each run writing its own profile. The merge of them is the ordinary
aggregate; each profile exported on its own is what lets the evidence say
which test reached which function, and which lines of it. Every other example
here answers "was this covered"; this one answers "by what".

### A library with no `main`

A component that cannot be run has no coverage of its own, so the tests live
outside it. [`c-lib/build-lib.sh`](c-lib/build-lib.sh) shows the arrangement:
build `libflight.a` as delivered, drive it from a harness in `tests/` to get
coverage, and **link** the archive with that harness to get the image whose
object code is analysed.

Analyse the linked image. A `.a` archive is refused outright, and unlinked
`.o` files lose data coupling entirely, cannot establish dead-code
reachability, and stop stack chains at the unit boundary — an unlinked object
simply does not have addresses yet. Ingesting one prints a caveat saying so.
See [`c-lib`](c-lib#a-library-with-no-main) for the numbers side by side.

### Measured, not projected

[`rust-qemu/`](rust-qemu) is the only example with an execution trace. A
`no_std` AArch64 image runs under QEMU, the log of what it executed is
imported with `ingest --trace`, and the viewer's **Measured** overlay shows
what ran beside what the projection says ran. They differ in the one place
that matters: the panic path behind a bounds check borrows the source line of
the arithmetic it guards, so it is projected covered — and the trace never
entered it. That is 21 instructions the projection calls covered and nothing
executed, in functions whose source coverage is complete. It needs
`qemu-system-aarch64` and a bare-metal Rust target; its `build.sh` names
whichever is missing.

## Your build system, not this one

Every example above builds through a hand-written `build.sh`, which is clear
about *what* butterfly needs but is nobody's actual build.
[`build-systems/`](build-systems) shows the same library built by **CMake**,
**make** and **Bazel**, with the six-point contract any integration has to
meet, a checklist to walk, and the four mistakes that produce evidence which
looks complete and is wrong. All three produce evidence identical to `build.sh` — Bazel only once
four non-obvious flags are set, each of which is otherwise a silent failure.

## Prerequisites

The build scripts need `llvm-profdata`, `llvm-cov` and `llvm-objdump`. They
locate them through the active rustup toolchain:

```sh
rustup component add llvm-tools        # once, for whichever toolchain is active
```

Inside this repo that is already handled — `rust-toolchain.toml` pins `1.97.1`
with the `llvm-tools` component, which is why `build.sh` works here with no
setup. **If you copy a `build.sh` into your own project and its toolchain has
no `llvm-tools`, the script fails with `llvm-profdata: command not found`.**
Either add the component, or point `PATH`/`--llvm-bin` at any LLVM install of a
version at least as new as the compiler that wrote the profile.

The C and C++ examples also need `clang`/`clang++`; the Rust example needs a
`nightly` toolchain for the instrumented build (see [`rust/`](rust)).

## The workflow, in three commands

```sh
# 1. build twice: instrumented, plus the binary you actually analyse
./examples/c/build.sh

# 2. artifacts -> evidence.json  (-o is required: it defaults to ./evidence.json in the CWD)
butterfly ingest examples/c/build --source-root examples/c -o examples/c/build/evidence.json

# 3. browse it
butterfly serve examples/c/build/evidence.json          # http://127.0.0.1:8787/
```

`ingest` on a directory discovers everything by itself: the llvm-cov export,
the non-instrumented binary, `build-info.txt`, any `.su` stack files, a
`*trace*.log`, `butterfly.toml` and `butterfly-baseline.json`. Pass
`--coverage/--object/--trace/--config/--baseline` to be explicit.

## Comparing optimisation levels

```sh
butterfly build examples/c/butterfly-build.toml --serve
```

builds `-O0 -O1 -O2 -Os` from one instrumented run, ingests each, and serves
them as switchable variants with a **Compare** page. Everything it produces is
stamped **EXPLORATORY** — it exists to show what the optimiser did, never for
certification credit. The binary you certify is the one your own build system
produced; feed that to `ingest`.

## Configuration

`butterfly.toml` sits next to the sources and is picked up automatically:

```toml
[requirements]
patterns = ['@satisfies\s+([A-Za-z0-9_.:/-]+)', 'REQ-[0-9]+', 'Implements:\s*(\S+)']
scope    = "both"   # header = comment block above the function | body | both
lookback = 30       # comment lines to scan above a function
```

Each pattern is a regex; capture group 1 is the requirement ID, or the whole
match when the pattern has no group (so bare `REQ-[0-9]+` works as written).
Match whatever your project already writes. The patterns in force are recorded
in the evidence provenance, so a reviewer can see how tags were found.

## What to look at first

1. **Overview** — the Annex A roll-up, the gap kinds, and how much of the
   object code is compiler-generated.
2. A function → **Flow** — the object-level CFG. Edge labels are the *machine*
   condition (`w8 == 0`), which is often the inverse of the source `if`.
3. **Object** — the disassembly interleaved with source, `⚙` marking
   compiler-generated instructions and requirement chips on the rest.
4. **Gaps** — every place source coverage cannot describe the object code,
   each resolvable with a DO-178C §6.4.4.3 category.
5. **File view** — press `f`, or click a file in the sidebar, to leave the
   one-function view and read the whole file: coverage minimap down the right
   edge, sticky function headers, and a requirement lens that highlights every
   line implementing a tag. `]` / `[` jump between uncovered regions, and
   **Show: uncovered only** collapses everything else so a long file reads as a
   short one. This is the view for "what is left to test in this file".
6. **Compare** (after `butterfly build`) — the same function at `-O0` vs `-O2`.

## A note on Rust and MC/DC

Current nightlies accept `-Zcoverage-options=branch|condition` but **not**
`mcdc`, and stable emits no branch regions at all. So the Rust example has
branch data but no MC/DC records. For Level A on Rust the practical route is
measured object-level branch coverage from an execution trace (`--trace`).
