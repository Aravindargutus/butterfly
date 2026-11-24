<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# c-per-test — which test exercised what

Every other example produces one merged profile, so its evidence can say a
line ran but not **what ran it**. That matters more than it sounds: DO-178C
§6.4.4 defines structural coverage analysis as determining what the
*requirements-based test procedures* did not exercise, so coverage that
cannot name the tests behind it is asking a reviewer to take that on trust.

This example is the same program run three times, once per test, each run
writing its own profile:

```
build/prof/t_clamp.profraw ─┐
build/prof/t_ceiling.profraw├─ llvm-profdata merge ─> build/coverage.json   (the aggregate)
build/prof/t_faults.profraw ─┘
       │
       └─ llvm-cov export, one per profile ────────> build/tests/*.json     (the attribution)
```

Nothing here is a butterfly invention — it is the ordinary `llvm-profdata`
workflow, and a test framework does the same with more ceremony.

```sh
./build.sh
butterfly ingest build --source-root . --tests build/tests -o build/evidence.json
```

```
tests      3 run · 6/6 executed function(s) attributed to a named test
           t_ceiling                   3 function(s)
           t_clamp                     3 function(s)
           t_faults                    3 function(s)
```

## What the attribution is good for

`clamp_alt` is entered by **two** tests, and they do not cover the same lines
of it: `t_clamp` takes the low edge (`v < lo`), `t_ceiling` the high one
(`v > hi`). Attribution that could not tell those apart would be attribution
in name only, so the evidence records covered lines per test, as ranges.

`rate_ok` is exercised by no test at all. Its `tests` list is empty — which
is *not* the same as an evidence file that had no per-test exports to begin
with, where every list is empty and the absence means nothing.

## The aggregate is the source of truth

Per-test exports only add attribution. They never change a count: the same
build ingested with and without `--tests` produces identical coverage, and
`attribution_changes_no_number` in `crates/ingest/tests/per_test.rs` asserts
exactly that, function by function.

An export describing a **different build** cannot be attributed, and that is
reported rather than quietly joined:

```
test `t_foreign` (…): 1 file(s) and 1 executed function(s) in this export are
not in the merged coverage. A per-test export from a different build cannot
be attributed, and its coverage is not counted here.
```

The same applies to an export that names the right files and functions but
contradicts the aggregate — a function the merged export never executed, or a
line count higher than every run combined. One impossible claim means the
export is not of this run, so the **whole export** is refused with the
contradiction named, rather than keeping the parts that happened to agree.

## The requirements matrix

The sources carry `@satisfies LLR-LIM-00x` tags and `requirements.txt` lists
the universe, including `LLR-LIM-004`, which is implemented nowhere — on
purpose. Ingest with both and the report's §2 shows one of each thing the
matrix exists to catch: a requirement fully covered by named tests
(LLR-LIM-001), one implemented but tested by nothing (LLR-LIM-002), one
partially reached (LLR-LIM-003's `case 1` arm is untested by design), one
with no code at all (LLR-LIM-004), and the untagged functions.

```sh
butterfly ingest build --source-root . --tests build/tests --requirements requirements.txt
```

## The document's claims against the measurement

`llr.yaml` is the requirements document, with its own traceability claims,
and they are deliberately not all true. `butterfly claims` compares them with
what the tags and the per-test exports measured:

```sh
butterfly requirements llr.yaml -o build/requirements.json
butterfly claims build/evidence.json --requirements build/requirements.json
```

```
checked    4 requirement(s): 2 agreed, 0 not checked, 1 with nothing claimed
claims     3 disagreement(s): tag-not-in-document 2, test-does-not-reach 1
           [test-does-not-reach] LLR-LIM-002
                       document: src/tests.c::t_clamp
                       evidence: `t_clamp` ran, but executed none of the lines this requirement claims
           [tag-not-in-document] LLR-LIM-005
                       document: not in the requirements document
                       evidence: `band_of` is tagged with it in the source
           [tag-not-in-document] LLR-LIM-006
                       document: not in the requirements document
                       evidence: `band_of` is tagged with it in the source
```

LLR-LIM-002 claims a test that never reaches its code. LLR-LIM-005 and
LLR-LIM-006 tag `band_of`'s two edges in the source and are in
`requirements.txt`, but the document has no entry for either: code claiming
a requirement the document does not hold. It exits 3, as any disagreement
does. The run is asserted by `crates/cli/tests/req_c_claims.rs`.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 14 |
| source files | 2 |
| lines covered | 64 / 77 |
| branch outcomes | 43 / 64 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`, which ingests this
example and compares the block above with the evidence. The gap total is
pinned there too, in the one table that holds every example's.
