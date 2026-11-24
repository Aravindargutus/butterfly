<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# Rust library — multiple modules

The same five-module flight library again, as a Rust crate. Read the
[MC/DC note](../rust#mcdc-is-not-available) first: it applies here too.

```sh
./examples/rust-lib/build.sh
cargo run -- ingest examples/rust-lib/build --source-root examples/rust-lib -o examples/rust-lib/build/evidence.json
cargo run -- serve examples/rust-lib/build/evidence.json    # http://127.0.0.1:8787/
```

## One crate, six files, no link step

`src/main.rs` declares `mod util; mod state; mod mode; mod crc; mod sched;` and
rustc compiles the whole crate in one go. There is no per-file object and no
separate link, so:

* the build script is shorter than the C and C++ ones,
* there is no `dsymutil` trap — a single-step build writes the dSYM,
* and there are **no `.su` files**: `-fstack-usage` is a C/C++ flag. Frames are
  recovered from each function's prologue instead, which is why the Stack tab
  says the estimate came from the object code rather than the compiler.

The module structure still comes through: six source files, 18 functions, and
the same **14 cross-module calls** as the C version, including
`main → sched::run → task_climb → state::set_altitude → util::clamp_i32`.

## Globals are atomics, on purpose

Rust has no `extern` mutable global you can share without `unsafe`, so the
shared state is `AtomicI32`/`AtomicU8`/`AtomicU32` statics in `state.rs`. They
are ordinary globals in the image, so data coupling works exactly as in C:
`state::PHASE` is written by `mode::advance` and read by `main`, while
`sched::DISPATCHES` and `sched::TASKS` stay inside `sched.rs`.

Using `static mut` instead would need `unsafe` at every access and is an error
under the 2024 edition; atomics are the honest way to have shared mutable
state, and they cost nothing here.

## Two things the C version does not show

**The uncalled function has no object code at all.** `util::unused_scale` is
`pub`, but nothing in the binary calls it and rustc drops it. In C and C++ the
same function survives into the image as a dead-code candidate to justify;
here the function page says *no object code in the analysed binary*. Nothing to
justify — but you must be able to say **why**, and "the compiler removed it" is
the answer.

**The gap count is dominated by the standard library.** Most gaps are
`helper-call` — on this host, 45 of 78 — almost all classified `core-lib` — panic paths, formatting, and
the machinery `println!` pulls in. The dead-code list is ~50 `core`/`std`
internals for the same reason. That is a fact about linking `std`, not about
this library; real Level A Rust is `no_std`, where both lists collapse to the
handful of symbols you actually have to account for.

## Optimisation comparison

```sh
cargo run -- build examples/rust-lib/butterfly-build.toml --serve
```

644 instructions at `-O0`, 144 at `-O2` — host-dependent figures; the whole
library inlines away.
The variants are built with the pinned stable toolchain while coverage comes
from nightly; the LLVM tools are taken from the coverage compiler's own
sysroot so the profile can be merged. EXPLORATORY.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 18 |
| source files | 6 |
| lines covered | 85 / 108 |
| branch outcomes | 13 / 22 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
pinned there too, in the one table that holds every example's.
