<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# C++ library — multiple translation units

The same five-module flight library as [`c-lib`](../c-lib), rebuilt in C++ so
the multi-file problems C does not have show up: **headers that emit code**,
**vtables**, **anonymous namespaces** and **static initialisers**.

```sh
./examples/cpp-lib/build.sh
cargo run -- ingest examples/cpp-lib/build --source-root examples/cpp-lib -o examples/cpp-lib/build/evidence.json
cargo run -- serve examples/cpp-lib/build/evidence.json     # http://127.0.0.1:8787/
```

Compiled `-std=c++17 -fno-exceptions -fno-rtti`, for the reasons in the
[single-file C++ example](../cpp#compiled-with-a-certification-shaped-subset).

## What multiple files add

**A header that emits object code.** `scale_by<T>` is defined in
`include/flight/util.hpp` and instantiated at `int` (in `sched.cpp`) and
`short` (in `main.cpp`). Both instantiations are attributed to **`util.hpp`**,
not to the `.cpp` that triggered them, and each has its own object code. So
the function list contains two entries over one source range in a header — the
first thing to understand when reviewing a template-heavy codebase.

**Vtables, so the dispatch is a data structure.** `Task` is an abstract base in
`sched.hpp`; `ClimbTask`/`HoldTask`/`TrimTask` live in an anonymous namespace
in `sched.cpp`. `sched_run` calls `kTasks[idx]->run(arg)`, which is one
`indirect-branch` gap, and all six virtual methods are reached in the Calls tab
as **data table** callers through their vtables. None of them are dead.

**Anonymous namespaces get a file prefix.** Internal-linkage functions are
named `sched.cpp:flight::(anonymous namespace)::ClimbTask::run(int) const` —
the file keeps two same-named statics apart, and the symbol itself is still
demangled. (Building this example is what found the bug where the prefix
defeated the demangler and these showed as raw `_ZNK…`.)

**A static initialiser you did not write.** The dead-code list contains
`unused_scale` — the genuine candidate — and **`_GLOBAL__sub_I_sched.cpp`**,
the function the compiler emits to construct `kClimb`, `kHold` and `kTrim`
before `main`. Nothing in the image calls it; the runtime does, through
`__mod_init_func`. It is not dead, but it *is* object code with no source
statement behind it, and at Level A you have to be able to say what it is and
why it runs. Six globals appear rather than the C version's three, for the
same reason: the three task objects are real objects in the image.

## Data and control coupling

Same shape as the C library: `g_state` written by `state.cpp` and `mode.cpp`,
read by `main.cpp`; `s_dispatches` private to `sched.cpp`; nine
function→global references in all. The deepest stack chain is again
**unbounded**, here because the virtual call's target set cannot be proven
statically.

## Builds

Two builds, one object per translation unit, then `dsymutil` — see the
[C library README](../c-lib#the-two-builds-and-one-macos-trap) for why that
last step is not optional on macOS.

```sh
cargo run -- build examples/cpp-lib/butterfly-build.toml --serve
```

554 instructions at `-O0` and 237 at `-O1` and above — host-dependent
figures, from the clang this repository's CI pins. EXPLORATORY.

The README said 479 and 203 until #158: nobody had rebuilt the variants since
the library grew, and no test read the sentence. That is what the numbers
block below is for.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 24 |
| source files | 8 |
| lines covered | 80 / 108 |
| branch outcomes | 26 / 44 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
pinned there too, in the one table that holds every example's.
