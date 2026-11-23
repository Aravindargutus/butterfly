<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# C++ example

`flight.cpp` — the same shape as the C example, plus the three things that make
C++ harder to certify at Level A: **name mangling**, **templates** and
**virtual dispatch**.

```sh
./examples/cpp/build.sh
cargo run -- ingest examples/cpp/build --source-root examples/cpp -o examples/cpp/build/evidence.json
cargo run -- serve examples/cpp/build/evidence.json     # http://127.0.0.1:8787/
```

## Compiled with a certification-shaped subset

```
clang++ -std=c++17 -fno-exceptions -fno-rtti
```

No exceptions and no RTTI, because both generate object code with no source
counterpart — landing pads, unwind tables, `typeinfo` — which is exactly the
"additional code not directly traceable to Source Code" that DO-178C
§6.4.4.2.b makes you account for. [`examples/cpp-exceptions`](../cpp-exceptions)
is one program built both ways, and shows side by side what turning them
back on adds and how butterfly reports it.

## What is in the program

| construct | function | what to look at |
|---|---|---|
| decisions / MC/DC | `clamp_altitude`, `mode_permits_engage` | same as the C example |
| switch | `phase_name` | jump table or compare chain, depending on `-O` |
| line-level requirement | `checksum` | `@satisfies LLR-CRC-004.1` **inside** the body — requires `scope = "both"` |
| **template** | `saturating_add<T>` | instantiated at `int` and `short` → **two entries** in the function list, one source range. Each has its own object code and its own coverage, which is the right answer: the two instantiations are different machine code and must each be covered |
| **virtual dispatch** | `Limiter` / `HardLimit` / `SoftLimit`, called through `LIMITERS[]` in `limit_with` | one **indirect call site** in the Calls tab, with `HardLimit::apply` and `SoftLimit::apply` as *data table* callers reached through the vtable |
| dead code | `legacy_scale` | the only genuine dead-code candidate |

## Mangled names

Symbols in the binary are Itanium-mangled — `HardLimit::apply` is
`_ZNK6flight9HardLimit5applyEi`, and the two `saturating_add` instantiations are
`_ZN6flight14saturating_addIiEET_S1_S1_S1_` and `..._addIsEE...`. butterfly
demangles them for display but matches on the mangled form, so a template at
two types stays two distinct symbols over the same source lines. The Object
view shows the mangled symbol; the sidebar shows the readable name.

On macOS, calls to weak/`linkonce` symbols such as template instantiations go
through `__stubs`, so a naive "which address does this `bl` target" reads them
as unresolved. butterfly resolves the stub back to its symbol — without that,
template callees show as dead code and drop out of the stack graph.

## Configuration

`scope = "both"` in `butterfly.toml`, because `checksum` tags a specific line
rather than the whole function:

```toml
[requirements]
patterns = ['@satisfies\s+([A-Za-z0-9_.:/-]+)', '@verifies\s+([A-Za-z0-9_.:/-]+)']
scope    = "both"
lookback = 30
```

Compiler-generated instructions (prologue, epilogue, stack-protector, range
checks) are marked `⚙` in the Object view and deliberately carry **no**
requirement chip — they trace to the compiler, not to a requirement, and
claiming otherwise would be the wrong answer at a stage-of-involvement audit.

## Optimisation comparison

```sh
cargo run -- build examples/cpp/butterfly-build.toml --serve
```

`-O0`/`-O1`/`-O2`/`-Os` side by side with a Compare page.

Watch `limit_with`. At `-O0` the vtable call is an `indirect-branch` gap **and**
butterfly resolves one static target candidate for it, so you can say something
about where control goes. At `-O2` the gap is still there but the candidate
list is **empty**: the optimiser reworked the address computation past what
static analysis can follow.

That is the honest limit of the projected route, and the point at which you
stop arguing and measure — ingest a QEMU trace with `--trace` and the indirect
edges come from what actually executed. Marked EXPLORATORY.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 11 |
| source files | 1 |
| lines covered | 55 / 67 |
| branch outcomes | 26 / 34 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
pinned there too, in the one table that holds every example's.
