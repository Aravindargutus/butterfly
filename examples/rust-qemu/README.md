<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# Rust on bare metal, under QEMU — measured coverage

Every other example shows **projected** object coverage: source coverage
pushed through the line table onto instructions nobody watched execute. This
one watches. A `no_std` AArch64 image runs under QEMU, QEMU logs every block
it executes, and `butterfly ingest` imports that log beside the coverage
export — so each instruction carries a projection *and* a measurement, and the
viewer's **Measured** overlay has something in it.

```sh
./examples/rust-qemu/build.sh
cargo run -- ingest examples/rust-qemu/build --source-root examples/rust-qemu -o examples/rust-qemu/build/evidence.json
cargo run -- serve examples/rust-qemu/build/evidence.json    # http://127.0.0.1:8787/
```

`ingest` finds `build/trace.log` by name; `--trace <file>` says it explicitly.

## Prerequisites

| | why | install |
|---|---|---|
| `qemu-system-aarch64` | runs the image and writes the trace | `brew install qemu` · `apt-get install qemu-system-arm` |
| `aarch64-unknown-none-softfloat` for the pinned toolchain | builds the image | `rustup target add aarch64-unknown-none-softfloat` (run inside this repository) |
| a `nightly` toolchain with `llvm-tools` | branch regions in the coverage export, as in [`rust/`](../rust#toolchain-two-of-them-on-purpose) | `rustup toolchain install nightly --component llvm-tools` |

**Choosing the nightly.** `build.sh` uses the toolchain named `nightly` unless
`BUTTERFLY_NIGHTLY` names another. Having *a* nightly is not enough: it needs
the `llvm-tools` component, because a profile is only readable by the
`llvm-profdata` of the LLVM that wrote it, and a `nightly` installed for some
other purpose usually lacks it. `build.sh` refuses such a toolchain and lists
the installed nightlies that do have the component; to check by hand:

```sh
rustup component list --installed --toolchain nightly | grep llvm-tools   # nothing printed = not this one
rustup component add llvm-tools --toolchain nightly                        # add it, or
BUTTERFLY_NIGHTLY=nightly-2026-06-22 ./examples/rust-qemu/build.sh         # use a dated nightly that has it
```

`scripts/ci-local.sh` honours the same variable. Any nightly that accepts
`-Zcoverage-options=condition` will do; this example has been built with
nightlies of 2026-06-21 (1.98) and 2026-08-09 (1.99).

Which one is chosen cannot move the **measured** column or the instruction
counts: the image under analysis is built by the pinned stable toolchain, and
the trace comes from running that image. It *can* move the **projected**
column, which is llvm-cov's reading of the instrumented host build and so is
the nightly's work — a release that maps coverage regions differently would
report different line and branch counts. With those two nightlies it did not,
and `build/build-info.txt` records which one produced the numbers in front of
you.

No cross linker: `rust-lld` ships with the toolchain. `build.sh` checks all
three before it builds anything and stops with the command that fixes the one
that is missing. It does not skip.

Verified with QEMU 11.0.3 on macOS (arm64). Ubuntu 24.04 packages QEMU 8.2.2,
which writes the same log format, but this example has not been run on it.

## The recipe, and why it has this shape

`ingest` always needs a coverage export: functions come from it, and the
object code is attached to them. A bare-metal image cannot write a profile —
there is no filesystem — so the export comes from the **host**:

1. **Host, instrumented** (nightly) → `build/coverage.json`. `main()` calls
   `campaign()` and exits with its verdict.
2. **Target, not instrumented** (pinned stable, `--target
   aarch64-unknown-none-softfloat -Copt-level=0`) → `build/flight.elf`, the
   image under analysis. `_start` sets the stack, `kmain()` calls the same
   `campaign()` and hands its verdict to a semihosting `SYS_EXIT`, so QEMU's
   exit status *is* the test result and a wrong answer on the target fails
   the build.
3. **QEMU** `-d in_asm,exec,nochain -D build/trace.log` → the trace.

The two builds are joined **by demangled name**. The mangled names differ —
the crate hash is part of a v0 symbol — but `flight_qemu::monitor::sum` is
the same on both sides, which is why both builds pass `--crate-name
flight_qemu`. Code that exists only on the target (`_start`, `kmain`,
`semihost_exit`, the panic handler), and code rustc does not instrument
(`Band`'s derived `PartialEq`, the `core` panic machinery), has no coverage
record and so no function page. The Overview lists it under *code in image
without source-coverage evidence*, with an *executed in trace* column: for
that code the trace is the only coverage there is.

**Both runs execute the same campaign on the same sources.** That is the
condition under which projected and measured may be compared at all, and it
is why a difference below is the compiler's doing and not the test set's.

`nochain` is not optional. Without it QEMU links translation blocks together
and jumps between them without returning to the loop that writes the log, so
executions — and with them branch outcomes — silently go missing.

## What measured shows that projected cannot

`monitor::sum` is one loop:

```rust
while i < n {
    acc = acc + samples[i];   // line 59
    i += 1;                   // line 60
}
```

At source level it is finished: **9/9 lines, 2/2 branch outcomes.** Open it
in the viewer, **Object** tab, and switch between the two overlays.

| `sum` | Projected | Measured |
|---|---|---|
| instructions covered | 54 of 63 (9 have no source line, so no projection) | **52 of 63** |
| `bl core::panicking::panic_bounds_check`, line 59 | **covered** — line 59 ran 4× | **never executed** |
| the `b.hs` guarding it | both outcomes covered | fall-through 4×, **taken 0×** |
| object branch outcomes | — | **5 of 8** |

The block that ends in that `bl` has **no successor at all**. `rustc` marks
every `fn` returning an uninhabited type `DW_AT_noreturn`, `ingest` reads the
attribute off the callee's own subprogram, and the CFG builder then ends the
block there instead of drawing a fall-through edge to the instruction after
the call — an edge no execution can take. The block says so: hover *does not
return* in the Object tab and it names the rule, the callee and the call
site. Six blocks in this image are marked that way — three in `sum`, one each
in `dispatch`, `cmd_climb` and `cmd_descend` — and they are the bounds check
and the arithmetic-overflow checks `-Copt-level=0` keeps (#210).

At `-Copt-level=0` rustc keeps a bounds check on `samples[i]` and an overflow
check on each `+`. Each is a conditional branch to a three-instruction panic
call, and each panic call carries the source line of the arithmetic it
guards. The line ran, so the projection colours the panic path green. The
trace says it never ran, and that three of the function's eight branch
outcomes were never taken — outcomes that no source-level metric can see,
because the source has one decision and the object code has four.

Across the image:

| | |
|---|---|
| instructions in functions with coverage records | 312 |
| projected covered / uncovered / no projection | 234 / 13 / 65 |
| **measured executed** | **257** |
| object branch outcomes measured | **27 of 42** |
| projection ≠ trace | 65 |
| — projected covered, never executed | **21**, every one a panic path: `sum` 9, `dispatch` 6, `cmd_climb` 3, `cmd_descend` 3 |
| — executed, no projection possible (line 0) | 44 |
| — executed, projected uncovered | 0 — as it must be, with one campaign on both sides |
| block executions in the trace | 148 |

| function | instr | projected | measured | outcomes measured |
|---|---|---|---|---|
| `campaign` | 117 | 81 | 93 | 8/16 |
| `monitor::clamp` | 27 | 20 | 27 | 4/4 |
| `monitor::band` | 31 | 20 | 28 | 5/6 |
| `monitor::sum` | 63 | 54 | 52 | 5/8 |
| `monitor::cmd_climb` | 18 | 15 | 15 | 1/2 |
| `monitor::cmd_descend` | 18 | 15 | 15 | 1/2 |
| `monitor::cmd_hold` | 4 | 0 | 0 | — |
| `monitor::dispatch` | 34 | 29 | 27 | 3/4 |

These numbers are pinned by `crates/ingest/tests/e2e_rust_qemu.rs`.

**None of them moved when the noreturn fall-through edges went (#210).** The
ticket expected them to, and it is worth saying why they did not: an object
branch outcome is counted only out of a block with two successors one of
which is `taken`, and a call's fall-through was never one of those, so 27 of
42 is what it was. The instruction counts cannot move — no instruction is
added or removed by an edge — and neither can the trace-side figures. What
does move is the edge count: `sum` goes from 16 object edges to 14 and
`dispatch` from 9 to 8. Cyclomatic complexity stays at 5 and 3, because each
edge that goes turns its block into an exit and `E − N + 2` counts both.

Two more things the trace settles:

* **Where they agree, they agree.** `band`'s `alt > 45_000` is never true:
  projected uncovered, measured 0×. `cmd_hold` is never called: 0 of 4
  either way.
* **Who the indirect call reached.** `dispatch` calls through a table of
  three. Static analysis can only list the candidates — `cmd_climb`,
  `cmd_descend`, `cmd_hold`. The trace resolves the call site to `cmd_climb`
  1× and `cmd_descend` 1×, and the **Calls** tab shows them as measured
  targets. `cmd_hold` stays what it was: address taken, never reached.

## Limits

* **A trace shows what executed. It does not show that the object code is
  what the source says.** Source-to-object correspondence (A-7 objective 9)
  is still a human review; the panic-path instructions above (21 on this host)
  are exactly
  the kind of object code with no source counterpart that review has to
  account for. Measurement tells you they were not run. It does not tell you
  whether they need to be.
* **QEMU is an emulator, not the target.** TCG executes the same
  instructions in the same order, which is what coverage needs, but there is
  no timing, no cache, no real peripheral and no real interrupt load. Credit
  for structural coverage comes from the target or an environment shown to
  be representative of it; a QEMU trace is a development aid until someone
  makes that argument. butterfly is not a qualified tool either.
* **The source coverage is from a different machine.** The projection maps
  *host* line counts onto *target* instructions through the target's DWARF.
  That is sound only while the two builds compile the same source the same
  way; anything behind `cfg(target_os = "none")` has no host counterpart.
* **Execution counts are an upper bound; coverage is exact.** QEMU logs a
  block when it enters it. A block entered with an exit request pending is
  abandoned before its first instruction and entered again, so it is logged
  twice and ran once. About one run in three of this example logs `_start`'s
  block twice. The importer discards a repeat it can prove impossible — the
  block ends in a direct branch that does not lead back to its own start —
  and records a caveat saying how many it discarded, which is why the count
  above is 148 on every run. A repeat separated from its original by an
  interrupt cannot be told from a real execution and is still counted — and
  it leaves a second mark: the edge out of the abandoned entry, which is an
  edge no branch took. Those are counted too and named in a caveat, so
  measured branch **outcomes** are an upper bound in the same way the counts
  are.
* **One CPU here; more than one is read per CPU.** QEMU's log interleaves
  every CPU's records, each tagged with its index (`Trace N:`). "This block
  ran after that one" is only true within one CPU's records, so both things
  the importer derives from neighbouring records — the impossible repeat
  above, and the block-to-block transitions behind measured branch outcomes
  and indirect-call targets — are derived per CPU and then summed. Two cores
  coming through `_start` back to back are two executions, not an abandoned
  entry. **Any** exec record the importer cannot read in full breaks
  adjacency, not just one kind of it: recognising a line as a `Trace` record
  is where the decision is taken, before anything else about the line is
  read. So a record whose CPU index cannot be read (older QEMU releases wrote
  none), one whose brackets are missing or the wrong way round, and one whose
  PC is not a number all do the same thing — no transition is drawn from or
  to it, **none is drawn across it either**, every CPU's stream restarts
  there because that record may be the missing successor of any of them,
  nothing is discarded from that log, and a caveat counts them and says what
  was lost. Bridging such a gap would invent a transition: a block ending in
  `b.hs`, an unreadable record holding the fall-through, and then the branch
  target, read as a taken edge that nobody took. Only the CPU-less kind still
  counts towards its block; one whose PC could not be read is in no block's
  count, so the counts are short rather than wrong, and the caveat says that
  too. A log where more than 1% of the records are unreadable in that last
  sense — no readable PC at all — is **refused** rather than caveated:
  `ingest` fails and produces no
  measured coverage, because a file that damaged has an unknown relationship
  to the run. This image is `-smp 1`, so all 148 records are `Trace 0:` and
  none of this arises.
* **Object branch outcomes are not MC/DC.** 27 of 42 is branch coverage
  measured at the object level. Whether that satisfies objective 5 for your
  compiler and options is the argument CAST-17 describes, and it is yours to
  make.
* **The trace grows with every block executed** — 36 KB here, for 148. A
  long-running image needs the `execlog` plugin format or a bounded run, and
  `BUTTERFLY_MAX_TRACE_MB` caps what `ingest` will read.
* **Not built by hosted CI.** `scripts/ci-local.sh` builds and tests it where
  QEMU is installed, and reports a named SKIP where it is not. That is a
  declared exception, not an omission: `support/matrix.toml` lists this
  example under `[[local_only]]` with the reason, and in exchange every row
  it backs is `caveats` and says "verified locally, not in hosted CI" (see
  [`support/README.md`](../../support/README.md#local-only-examples)).

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 9 |
| source files | 2 |
| lines covered | 67 / 79 |
| branch outcomes | 21 / 30 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
pinned there too, in the one table that holds every example's.
