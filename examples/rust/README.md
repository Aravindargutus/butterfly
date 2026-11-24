<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# Rust example

`demo.rs` — the same program again, built with `rustc`. Read the MC/DC note
below before using this as a template for real Level A work.

```sh
./examples/rust/build.sh
cargo run -- ingest examples/rust/build --source-root examples/rust -o examples/rust/build/evidence.json
cargo run -- serve examples/rust/build/evidence.json    # http://127.0.0.1:8787/
```

## Toolchain: two of them, on purpose

The instrumented build needs **nightly** (`-Zcoverage-options=condition`); the
analysed binary is built with whatever toolchain you ship. That creates a trap:
a `.profraw` written by nightly's profiler runtime **cannot be merged by
stable's `llvm-profdata`** — you get `error: no profile can be merged` even
though the file is there and non-empty.

Both entry points handle it:

* `build.sh` puts the nightly toolchain's
  `lib/rustlib/<host>/bin` first on `PATH`.
* `butterfly build` derives the LLVM tool directory from the coverage
  compiler's own sysroot for `kind = "rustc"` projects, and prints the
  directory it chose. Override it with `llvm_bin` under `[coverage]` if your
  tools live somewhere else.

## MC/DC is not available

Current nightlies accept `-Zcoverage-options=block|branch|condition` but
**reject `mcdc`**, and stable emits no branch regions at all. So this example
produces statement and branch coverage but **no MC/DC records**. Nothing is
invented to fill the gap, but nothing names the toolchain as the reason
either: the decision list is empty, so the MC/DC tab shows its generic
**Branch regions without MC/DC records** table, or `No decisions in this
function.` for a function with no branch regions.

For Level A on Rust the practical route is measured object-level branch
coverage from an execution trace:

```sh
qemu-system-... -d in_asm,exec,nochain -D trace.log -kernel <your image>
cargo run -- ingest examples/rust/build --source-root examples/rust \
  --trace trace.log -o examples/rust/build/evidence.json
```

Switch the overlay to **Measured** and the branch outcomes come from what
actually executed, not from a projection. That is the stronger evidence
anyway — the projection is a prediction; the trace is a measurement.

This example has no trace of its own: it is a host program. The route is
worked end to end in [`rust-qemu/`](../rust-qemu) — a bare-metal image, the
QEMU run, the ingest, where the two views disagree, and what a trace still
does not prove.

## What is in the program

`clamp_altitude`, `mode_permits_engage`, `phase_name` (a `match`), `checksum`,
`average` and an uncalled `legacy_scale`, mirroring the C example so you can
compare the generated code across languages.

Rust-specific things to look at:

* **Symbol names** are v0/legacy mangled and demangled for display.
* **`legacy_scale` has no object code at all.** In C and C++ the same uncalled
  function survives into the binary and shows up as a dead-code candidate to
  justify; here rustc dropped it, so the function page says *no object code in
  the analysed binary*. Nothing to justify — but you have to be able to show
  **why**, and "the compiler removed it" is the answer, not an omission.
* **Runtime checks** — `average` carries `runtime-check` instructions (`⚙`) for
  the panic paths; 12 of them on this host. Object code with no source counterpart: §6.4.4.2.b
  material.
* The panic machinery pulls in `core` symbols that no test covers. Most of this
  example's gaps — on this host, 33 of 63 — are `helper-call` classified
  **`core-lib`**, so they sit in their own bucket instead of being confused with
  uncovered project code.

## Dead code and statically linked std

`butterfly serve` will list ~50 dead-code candidates, essentially all of them
`core`/`std`/`addr2line` internals dragged in by the standard library and the
panic/backtrace machinery. That is not a bug in the analysis — with a
statically linked `std` those symbols really are unreached in this binary.

It is a signal about the shape of the project rather than about the code: real
Level A Rust is `no_std`, and there the dead list collapses to the handful of
symbols you actually have to justify. Treat this example's list as a
demonstration of what linking `std` costs you at the object-code level.

## Optimisation comparison

```sh
cargo run -- build examples/rust/butterfly-build.toml --serve
```

Builds `-Copt-level=0/1/2/s` from one instrumented run. Marked EXPLORATORY.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 8 |
| source files | 1 |
| lines covered | 47 / 56 |
| branch outcomes | 11 / 14 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
pinned there too, in the one table that holds every example's.
