<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# C library — multiple translation units

A small flight-control library across five modules plus a driver. Where the
single-file [`c`](../c) example is about one function at a time, this one is
about what only appears once a program is split up: **data coupling, control
coupling, and stack chains that cross module boundaries** (DO-178C §6.4.4.2.c,
Annex A table A-7 objective 8).

```sh
./examples/c-lib/build.sh
cargo run -- ingest examples/c-lib/build --source-root examples/c-lib -o examples/c-lib/build/evidence.json
cargo run -- serve examples/c-lib/build/evidence.json      # http://127.0.0.1:8787/
```

## The modules

```
include/util.h   src/util.c    clamp_i32, saturating_add, a static helper, one uncalled export
include/state.h  src/state.c   g_state + the accessors -- the coupling hub
include/mode.h   src/mode.c    the MC/DC decision, the phase switch
include/crc.h    src/crc.c     a leaf: loop with an inner decision
include/sched.h  src/sched.c   a task table of function pointers
                 src/main.c    the driver
```

`main → sched_run → task_climb → state_set_altitude → clamp_i32` reaches
through four files. That single chain is what the whole example exists to show.

## What to look at

**Data coupling — the Overview's coupling table.** Three globals, nine
function→global references:

| global | touched by |
|---|---|
| `g_state` (12 B) | written by `state.c` **and** `mode.c`, read by `main.c` |
| `s_dispatches` (4 B) | `sched.c` only — module-private, `static` |
| `TASKS` (24 B) | read by `sched_run` to dispatch |

`g_state` is the interesting one: `mode_advance` writes a field of a struct
that `state.c` owns. That is a coupling item a reviewer has to see, and it is
invisible in a single-file example.

**Control coupling — 14 calls cross a file boundary.** Open any function's
**Calls** tab; callers and callees are labelled with the file they live in.

**Stack across modules — the Stack tab.** Frames come from six separate `.su`
files, one per translation unit, and are auto-discovered:

```
main(144 B) -> sched_run(32 B) -> state_raise_fault(16 B)   192 B, NOT bounded
task_climb(32 B) -> state_set_altitude(32 B) -> state_raise_fault(16 B)   80 B
task_trim(32 B) -> saturating_add(48 B) -> abs_i32(16 B)   96 B
```

The worst-case chain is reported **unbounded**, and correctly so: `sched_run`
dispatches through `TASKS[idx]`, and static analysis cannot prove which task
runs. This is exactly the finding §6.3.4.f wants you to resolve — by analysis,
by a measured trace, or by removing the indirect call.

**Uncovered is not the same as dead.** `task_trim`, `saturating_add` and
`abs_i32` never execute — `main` never dispatches task 2 — but they are
reachable and are *not* dead-code candidates. `unused_scale` is the only
genuine candidate: exported, never called, never address-taken. Confusing
those two is a classic finding at a stage-of-involvement audit.

**The file view earns its keep here.** Press `f` (or click a file in the
sidebar) and the left pane becomes a dashboard for *that file*: its functions
ranked by risk, a **requirement lens** listing which requirements the file
implements and how well each is covered, its dead-code candidates, and —
the multi-file one — **coupling with other files**: which files it calls into,
which call back, and which globals they share. That panel is the per-file view
of the same evidence the Overview's coupling table shows for the whole image.

**Show: uncovered only** is what makes a multi-file project reviewable — it
hides every line the tests already reach, leaving what is left to do:

| file | all lines | uncovered only |
|---|---|---|
| `state.c` | 33 | **0** — fully covered, nothing to look at |
| `sched.c` | 47 | **5** |
| `mode.c` | 31 | 8 |
| `util.c` | 32 | 23 — the outlier |

`util.c` barely shrinks, and that is the point: at 31.8% it is *mostly*
untested (`saturating_add`, `abs_i32` and `unused_scale` never run), so it is
the file to open first. The fold turns "six files" into "one file and a
handful of lines elsewhere".

**Internal linkage.** `abs_i32`, `task_climb` and friends are `static`, so
llvm-cov names them `util.c:abs_i32`, `sched.c:task_climb` — the file prefix
keeps two same-named statics apart. They still appear in the graphs.

## The two builds, and one macOS trap

As in the single-file example, `build.sh` builds twice: everything
instrumented for coverage, and everything clean with `-fstack-usage` for the
image under analysis. The second build compiles **one object per translation
unit** so each yields its own `.su`, then links.

That link is the trap:

```sh
clang ... build/obj/*.o -o build/flight
dsymutil build/flight              # <-- required
```

On macOS, debug info stays in the `.o` files behind a debug map. A single-step
compile-and-link runs `dsymutil` for you; **a link of separate objects does
not.** Without the dSYM the binary has no line table, and the first run of this
example produced `456 instructions: 0 covered, 0 uncovered, 456 unknown` on
this host, with every one of the 404 that carried no source line filed under
"no source line". If your projection is entirely unknown, this is the first
thing to check — the totals are this host's, the symptom is not.

## Change impact

```sh
cargo run -- changes examples/c-lib/build/evidence.json          # whole history
cargo run -- changes examples/c-lib/build/evidence.json --since <tag-of-certified-baseline>
```

Every function gets the commits that touched its lines (`git log -L`, so the
attribution survives edits above it and the `fixtures/` → `examples/` rename
in this repository's history), tickets are pulled from the messages with the
`[change]` patterns in `butterfly.toml`, and the Overview shows ticket →
functions → requirements → unresolved gaps. This repository's commit messages
carry no Jira-style ids, so here every commit lands in the **no ticket**
table — which is itself the finding the section exists to make.

## A library with no `main`

`src/` is a library: it cannot be run, so it cannot produce coverage by
itself. `build-lib.sh` shows the real arrangement, where the tests live outside
the component under analysis:

```sh
./examples/c-lib/build-lib.sh
cargo run -- ingest examples/c-lib/build-lib --source-root examples/c-lib -o examples/c-lib/build-lib/evidence.json
cargo run -- serve examples/c-lib/build-lib/evidence.json
```

It produces three artefacts:

| | what it is |
|---|---|
| `libflight.a` | the deliverable, built exactly as it ships |
| `harness-instr` | library sources + `tests/test_flight.c`, instrumented — **coverage** |
| `harness` | the archive linked with the harness, **not** instrumented — the **image analysed** |

`tests/test_flight.c` is verification software, not airborne code. It exists
only so the library's sources can be executed.

**Analyse the linked image, not the archive.** Passing a `.a` is refused, and
passing the individual `.o` files is a poor substitute:

| | linked `harness` | unlinked `.o` files |
|---|---|---|
| object coverage | 346 covered / 102 uncovered / **8** unknown | 211 / 102 / 8 |
| data coupling | **3 globals, 9 refs** | **none** |
| stack worst case | **192 B**, 2 chains unbounded | 32 B: the first object named (`crc.o`) alone |
| dead code | reachability from the entry point | no entry point, not determinable |

The reason is that an unlinked object has no addresses yet: a cross-unit call
is a placeholder plus a relocation, and a global reference is a relocation too.
butterfly reads branch relocations, so calls between units *do* resolve — but
global-data references do not, so data coupling is silently under-reported, and
each object starts at address 0 so only the first one's symbol table survives
-- the first *named*, so the order you pass them in decides which unit is
analysed (the command below names them in file-name order: `crc.o`).
Ingesting an unlinked object prints a caveat saying exactly this, and with no
entry point no dead code is claimed. Nothing here is a substitute for
analysing the code at the addresses it will actually have.

The figures above are this example's with Apple clang 17 on arm64; another
compiler gives other counts. `crates/ingest/tests/unlinked_objects.rs` asserts
each difference rather than the numbers, on the objects `build-lib.sh` leaves
in `build-lib/obj`:

```sh
./examples/c-lib/build-lib.sh
butterfly ingest --coverage examples/c-lib/build-lib/coverage.json \
  $(for o in examples/c-lib/build-lib/obj/*.o; do case "$o" in *test_flight*) ;; *) echo "--object $o" ;; esac; done) \
  --source-root examples/c-lib -o unlinked.json
```

**One more macOS trap.** Link the harness from a *real* object file:

```sh
clang -c tests/test_flight.c -o build-lib/obj/test_flight.o   # keep the .o
clang build-lib/obj/test_flight.o build-lib/libflight.a -o build-lib/harness
dsymutil build-lib/harness
```

`clang tests/test_flight.c libflight.a -o harness` compiles to a *temporary*
object and deletes it, so `dsymutil` cannot read the debug map, the dSYM comes
out empty, and on this host 143 of 456 instructions land in "unknown".
Keeping the `.o` is what makes the line table survive the link.

## Optimisation comparison

```sh
cargo run -- build examples/c-lib/butterfly-build.toml --serve
```

456 instructions at `-O0` become 224 at `-O1` and above — host-dependent
figures — with the whole library inlined into far fewer functions.
EXPLORATORY — not credit.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 18 |
| source files | 8 |
| lines covered | 87 / 112 |
| branch outcomes | 25 / 40 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
pinned there too, in the one table that holds every example's.
