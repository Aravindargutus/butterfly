<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# rust-constructs — what makes Rust different from C

Every other Rust example here is fully concrete: no generics, no traits, no
`dyn`. `constructs.rs` has one of each construct those examples leave out
(#167):

| construct | in the source | what it becomes in the image |
|---|---|---|
| generic function | `limit<T>`, called at `i32` and `u64` | two symbols, `limit::<i32>` and `limit::<u64>`, one source range |
| `dyn` dispatch | `run_law(law: &dyn Law, …)` calling `law.apply(x)` | an indirect call through a vtable; `Gain` and `Deadband` implement `Law`, `Bypass` implements it and is never built |
| closures | three in `closures()`: by reference with a decision (`FnMut`), by mutable reference, by move (`FnOnce`) | a symbol per closure, and a copy of the generic taking it per closure type (`each_sample::<{closure#0}>`, `each_sample::<{closure#1}>`) |
| `macro_rules!` | `within!` expands to `if $v >= $lo && $v <= $hi`, used twice in `in_envelope` | the expanded comparisons, inlined at both uses |

```sh
./examples/rust-constructs/build.sh
cargo run -- ingest examples/rust-constructs/build --source-root examples/rust-constructs -o examples/rust-constructs/build/evidence.json
cargo run -- serve examples/rust-constructs/build/evidence.json    # http://127.0.0.1:8787/
```

It is built like [`rust/`](../rust): the coverage build needs a nightly
(`-Zcoverage-options=condition`, with nightly's own `llvm-profdata`), and the
analysed image is the pinned stable toolchain's, `-g -Copt-level=0`, not
instrumented. The two builds are joined by demangled name. CI pins
`nightly-2026-08-10` through `BUTTERFLY_NIGHTLY`.

## What butterfly shows

Everything below was observed on macOS arm64 (rustc 1.97.1, coverage from
nightly-2026-08-10). `crates/ingest/tests/rust_constructs.rs` asserts it.

### A generic: two functions, one source range

`limit::<i32>` and `limit::<u64>` each have a coverage record, a symbol, a
branch record and object code of their own. They are not merged: they have
different names. `main` sends them down different paths. The `i32` copy takes
the low clamp and never evaluates `v > hi`. The `u64` copy takes the high clamp
and never returns `lo`.

- **Each copy's figures are its own.** The `i32` copy covers 4 of its 9 lines
  and the `u64` copy 6 of 9. `limit::<i32>`'s `return hi` is object code that
  never ran, and it is projected as such.
- **The file's source view is the sum.** llvm-cov adds every copy into the
  file's line counts, so in the source pane every line of `limit` except the
  final `v` reads as covered: each ran *in some copy*. Open the function to see
  a copy's own figures.

This was a defect, found and fixed with this example: each copy's line figures
and object code used to be projected from the file's summed counts. Both copies
showed 7 of 9 lines, and the `i32` copy's `return hi` read as covered object
code although that copy never ran it. Only its branch record (`v > hi`: 0 true,
0 false) contradicted it. A copy's own counts are now rebuilt from its own
regions with llvm-cov's segment rules. A test checks that the rebuild
reproduces llvm-cov's own file counts exactly, on every example here.

### `dyn`: an indirect call with no candidates, and impls that are not dead

- `run_law`'s `law.apply(x)` is an **indirect-branch gap**. It has **no static
  candidates**, because the vtable arrives as an argument: nothing in
  `run_law` takes an address or reads a table. Its target set is not
  resolved, the same as C++ virtual dispatch.
- `Gain::apply` and `Deadband::apply` have **no direct callers**, and they are
  **not dead-code candidates**. Their addresses are stored in data (their
  vtables, anonymous `__const` data on Mach-O), which is what keeps them off
  the dead list. On other hosts this is not asserted: reading the vtable has
  been verified on macOS/AArch64 only.
- **`Bypass::apply` has no object code.** No `Bypass` is ever turned into a
  `&dyn Law`, so rustc builds no vtable for it and emits no code. Like the
  rust example's `legacy_scale`, there is nothing in the image to justify, and
  the function page says so.

### Closures: static dispatch, and one closure rustc does not instrument

- `closures::{closure#0}` (`|s| if s > threshold { total += s }`) is a
  function with its own branch record (2 true, 1 false over the three samples).
  Its one caller is `each_sample::<closures::{closure#0}>`, which calls it
  directly. A closure passed to a generic is static dispatch, not an indirect
  call.
- `closures::{closure#2}` (`move || owned.into_iter().sum()`), consumed as
  `FnOnce`, is the same: a function called once by
  `once::<closures::{closure#2}>`.
- **`closures::{closure#1}` (`|_| count += 1`) has no coverage record.** rustc
  maps no region in its body, yet its code is in the image and runs. butterfly
  lists it as a function marked **no record**, inside `closures`, on line 93
  where the DWARF puts it. Its object code is analysed, with its overflow
  check and panic call. Nothing of it is projected covered or uncovered,
  because the count on line 93 belongs to `closures`. Every instruction with a
  source line carries a `no-coverage-record` gap. It is neither executed nor
  unexecuted, and the summary counts it apart (#273). The line it is written
  on still reads covered. That is a statement about `closures`, and the
  function page says so.
- The construct inventory counts `closures()`'s three closures. It used to count
  two, and to read `move ||` as a logical OR. That is fixed here.

### `macro_rules!`: no branch region, and the code lands on the definition

- rustc emits **no branch region** for the `if` inside `within!`, so
  `in_envelope` has no branch outcomes at all, although each use is a
  two-condition decision and `main` never makes `alt <= 45_000` or
  `speed <= 350` false.
- The two using lines read covered.
- The image's line table puts the expanded code on the **macro's definition**
  (`if $v >= $lo && $v <= $hi`), not on each use. So both uses land on that
  one line, outside `in_envelope`'s own range. The line has no coverage count.
  Its instructions are projected neither covered nor uncovered, and it carries
  `not-instrumented` gaps and one `extra-object-branch` gap for the
  conditional branches of both expansions together.
- Nothing claims the macro's decision was covered, and nothing can show which
  outcomes ran. Only a trace would ([`rust-qemu`](../rust-qemu)). The
  inventory does not see the `if` either, because it is in the macro, not in
  the function's text.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 15 |
| source files | 1 |
| lines covered | 55 / 60 |
| branch outcomes | 7 / 12 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
bounded there too, in the one table that holds every example's. The lines are
the file's view, summed over both copies of `limit`. The 15 functions include
`closures::{closure#1}`, which has no coverage record: it counts as a
function, but neither as executed nor as unexecuted (#273).
