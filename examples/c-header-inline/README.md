# c-header-inline — same-named local symbols, in both shapes

The symbol table knows only a bare name, and more than one function can
answer to it. This example carries both ways that happens.

**A header `static inline`.** `inc/geom.h` defines `geom_clamp`, and both
`src/geom.c` and `src/main.c` include it. At `-O0` neither copy is inlined
away, so the image carries **two local symbols named `geom_clamp`** — one per
translation unit, at different addresses, with opposite coverage (geom.c's
copy only ever takes `v > hi`, main.c's only `v < lo`). Both claimants are
unit-qualified: `geom.c:geom_clamp`, `main.c:geom_clamp`.

**A static beside an extern of the same name.** `src/aux.c` has a `static
scale`; `src/scale.c` has an external `scale`. Here llvm-cov names them
`aux.c:scale` and plain `scale` — the claimants do not even share a name
shape, so a rule that only looked for a `unit:name` qualifier would miss the
pair and show the extern the static's body.

butterfly resolves each symbol through the DWARF: the compilation unit that
holds the symbol's `DW_TAG_subprogram` names the unit that emitted it, and a
function belongs to the unit its qualifier names or, unqualified, to the file
it was found in (#36). `crates/ingest/tests/dup_static_symbols.rs` asserts,
for both shapes, that each copy keeps its own object code, round-trips
through its own symbol, and that the call graph binds every caller to its own
unit's copy.

The gap check asks that same resolver. A call that the call graph binds to a
covered function is not also reported as a `helper-call` gap leaving the
covered source set — it used to be, because llvm-cov qualifies an
internal-linkage function's name with its unit (`geom.c:geom_clamp`) and the
gap check matched the bare symbol by name (#206). `geom_span`, `main` and
`aux_halve` each call their own unit's copy, and none of the three is a
finding. The `tally` below that llvm-cov keeps no record for still is: there
the call graph binds nothing either, and the two agree.

## Without a line table, butterfly refuses to guess

The resolution needs the DWARF. Build without `-g`, or lose the dSYM, and
butterfly cannot say which unit emitted a given copy — so it attributes
**neither**, and says so in the image findings:

```
symbol _scale @0x100000574 (28 B): 2 coverage functions share this name
(aux.c:scale, scale) and the DWARF does not say which translation unit
emitted this copy, so its object code is attributed to none of them and is
missing from every object-code total
```

Note the consequence: object-code coverage **falls** compared with guessing,
because code that used to be credited to an arbitrary claimant is now
credited to nobody. That is the intended direction — a percentage that
counts one unit's instructions against another unit's function is not
evidence — but it means a no-`-g` build of a project with same-named statics
will report lower object coverage than it did before, with a finding
explaining every byte of the difference.

```sh
./build.sh
cargo run -- ingest build --source-root . -o build/evidence.json
cargo run -- serve build/evidence.json
```

## A third shape: two units with the same file name

`src/alpha/dup.c` and `src/beta/dup.c` each hold a `static tally`. llvm-cov
qualifies both as `dup.c:tally` — the **basename only** — so the coverage
export cannot tell them apart, and in fact keeps a record for just one of
them. The DWARF unit paths can tell them apart, and butterfly compares whole
paths from the right rather than basenames.

The copy with no coverage record is therefore left **unattributed**, with a
finding saying why:

```
symbol _tally @0x10000060c (24 B): the only coverage function of this name
(dup.c:tally) belongs to another translation unit, so the coverage export
carries no record for this copy — llvm-cov names two same-named statics
identically and keeps one
```

That is the intended answer. Its 24 bytes are missing from the object-code
totals because nothing in the coverage data describes them, and the finding
accounts for every one of them.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 10 |
| source files | 7 |
| lines covered | 22 / 23 |
| branch outcomes | 3 / 8 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
pinned there too, in the one table that holds every example's.
