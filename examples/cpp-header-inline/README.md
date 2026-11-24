# cpp-header-inline — same-named functions across units, the C++ shapes

The C++ counterpart of [`c-header-inline`](../c-header-inline). The symbol
table knows only a bare name, and in C++ two things make more than one
translation unit answer to it.

**An anonymous-namespace function of one name in two units.** `src/geom.cpp`
and `src/engine.cpp` each define a `scale(int)` inside `namespace { }` — one
doubles, the other triples. Both have internal linkage, so at `-O0` the image
carries **two local symbols that demangle to `(anonymous namespace)::scale(int)`**,
at different addresses. llvm-cov qualifies them the way it does a C `static`
— `geom.cpp:_ZN12_GLOBAL__N_15scaleEi` — and butterfly demangles the tail
while keeping the prefix: `geom.cpp:(anonymous namespace)::scale(int)` and
`engine.cpp:(anonymous namespace)::scale(int)`. The unit qualifier is a single
`:` in a name full of `::`, and a splitter that gave up on `::` took the whole
string for one bare name, so the pair never reached the unit resolver at all
(review of #36).

**A header `inline`.** `inc/geom.hpp` defines `flight::clamp_to`, and all
three units include and call it. Unlike the C `static inline`, a C++ `inline`
function has external linkage: every unit compiles its own copy, the linker
keeps **one**, and the coverage export carries one record whose count is the
sum over every caller. The surviving copy must still be attributed.

butterfly resolves each `scale` through the DWARF, by the compilation unit
that holds the symbol's `DW_TAG_subprogram` (#36).
`crates/ingest/tests/dup_cpp_symbols.rs` asserts that there are exactly two
`scale` symbols, that each belongs to the function of the unit that compiled
it, that the two carry their own object code at different addresses, that no
symbol whose name the evidence analysed is left unattributed, that
`clamp_to` carries object code, and that no call the call graph resolved is
reported as a gap outside the covered source set (#206). The test skips,
rather than fails, when the fixture has not been built; CI builds it.

```sh
./build.sh
cargo run -- ingest build --source-root . -o build/evidence.json
cargo run -- serve build/evidence.json
```

`build.sh` compiles at `-O0` so the copies survive as real symbols, and with
`-fno-exceptions -fno-rtti` to keep out object code with no source
counterpart. `CXX` names the compiler; the llvm tools are taken from the same
toolchain.

## What to look at

- **The two `scale` entries in the sidebar.** Open each: the Object tab shows
  a different address and a different body (`v * 2` against `v * 3`), and the
  Calls tab shows one caller apiece — `flight::span` for geom.cpp's copy,
  `flight::drive` for engine.cpp's. Neither borrows the other's object code.
- **`flight::clamp_to`.** One function, called from three units, executed 3
  times. `v < lo` went both ways (true once, false twice); `v > hi` was true
  twice and **never false**, so `return v;` is the one uncovered line and the
  summary's single open test obligation: no call passes a value already in
  range.
- **The Gaps tab of `span` and `drive`.** Empty. Each calls its own unit's
  `scale`, and a call that stays inside the covered source set is not a
  finding. It used to be one: the gap check resolved the callee by name, the
  bare symbol matched neither `engine.cpp:_ZN12_GLOBAL__N_15scaleEi` nor its
  demangling, and both calls were reported as leaving the covered source set
  while the Calls tab bound them to the right unit's copy. The two now ask
  the same resolver (#206). The only gaps left are `main`'s three `printf`
  calls, classified `libc`.

## Verified numbers

Read from the evidence of the build above. The source-level figures hold on
every host; the object-level ones are what Apple clang 17.0.0 emits for
aarch64 Mach-O and differ with the compiler and the target.

| | |
|---|---|
| files | 4 (`geom.hpp`, `geom.cpp`, `engine.cpp`, `main.cpp`) |
| functions | 6, all executed |
| lines | 14/15 (93.3%) |
| branch outcomes | 3/4 (75.0%) |
| MC/DC decisions | 0 — no decision has more than one condition |
| `scale` symbols | 2, each attributed to its own unit |
| text symbols left unattributed | 0 on Mach-O; on ELF only the C runtime's stubs, see below |
| gaps | 3, all helper-call: `main`'s three `printf` calls — *host-dependent* |
| object code | 104 instructions, 101 projected covered, 3 uncovered (all in `clamp_to`), 21 compiler-generated — *host-dependent* |
| stack | worst case 80 B, nothing unbounded — *host-dependent* |

On ELF the image also carries the C runtime's startup stubs (`_start`,
`_init`, `frame_dummy`, …). They have no coverage record and are legitimately
unattributed, which is why the test asserts the narrower property above
rather than an empty list.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 6 |
| source files | 4 |
| lines covered | 14 / 15 |
| branch outcomes | 3 / 4 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
pinned there too, in the one table that holds every example's.
