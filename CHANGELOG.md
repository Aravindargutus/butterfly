<!--
SPDX-License-Identifier: MIT OR Apache-2.0
Copyright (c) 2026 Dhakshnamoorthy Durgaraj
-->

# Changelog

All notable changes to butterfly are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Version numbers follow the rules in
[`docs/qualification/versioning.md`](docs/qualification/versioning.md), which
is this project's reading of [Semantic Versioning](https://semver.org/).

**About this first entry.** `0.1.0` was never released. It was the version in
`Cargo.toml` from the first commit (2026-08-28) and it never moved, so every
evidence file butterfly has ever written says `0.1.0` regardless of which
build produced it — the DO-330 assessment records that as a gap
(`docs/qualification/DO-330-assessment.md`, T-8 objective 2). There is no
`0.1.0` tag and no `0.1.0` release to diff against. `0.2.0` is therefore the
first release, and its entry describes everything in the tree rather than a
delta: the initial direct commits (`162c544`..`14814ef`, 2026-08-28 to
2026-08-30) and the 78 pull requests merged since (#16 to #228).

Entries are grouped by area, not by pull request. Where a line names a pull
request, that is where to read the reasoning.

## [0.2.0] — unreleased

The date is filled in when `v0.2.0` is tagged. Everything below is on `main`;
nothing below is waiting on the tag.

### Ingest and the evidence database

- `butterfly ingest` reads an llvm-cov export, the linked binary, its DWARF
  and `llvm-objdump` output, and writes one self-describing `evidence.json`
  (`butterfly-evidence/1`). Every number the viewer shows is derived from that
  file, and every value is traceable to an input listed in
  `provenance.inputs` by SHA-256.
- Every evidence file names the build of butterfly that wrote it, not only
  the version: `provenance.toolCommit` is the commit the tool was built from
  and `provenance.toolDirty` whether its work tree was modified at the time,
  captured when the `ingest` crate is compiled (#117). A build that is not
  from a checkout records `unknown`, or the id given in
  `BUTTERFLY_BUILD_COMMIT`; it never guesses. The report, the viewer's
  Provenance panel and `butterfly --version` show the same words, and the
  release workflow refuses a binary whose answer is not the tagged commit on
  a clean tree. Evidence written before the field existed still loads and
  reads `commit not recorded`. An additive field: the schema string stays
  `butterfly-evidence/1`, and by `docs/qualification/versioning.md` the
  change is a minor bump, which `0.2.0` already is.
- GCC coverage via `gcov`, without inventing the fields GCC does not record
  (#70, issue #45).
- Requirement text read into a sidecar from YAML (#58), and from CSV and
  Markdown, chosen by the file's extension (#110).
- `.su` stack-usage files from `-fstack-usage` (#20).
- Per-test coverage: one export per test, so evidence can say which test
  exercised what, with attribution rules that refuse to guess (#47, issue
  #41).
- Execution traces (QEMU `-d in_asm,exec,nochain`, execlog CSV, or one
  address per line) for measured — not projected — object coverage.
- Resource limits on untrusted artifacts and on evidence loading (#31, issue
  #5). Five file reads outside `ingest` that skipped them — the evidence
  `tags` and `claims` read, the `claims` requirements sidecar, the `build`
  config, the requirement documents `serve` re-hashes, and the sources
  `/api/freshness` re-hashes — now go through the same bounded readers
  (#240, issue #231). Helper programs (`git`, `llvm-objdump`, `llvm-cov`,
  `ctags`, `rustc`) now run under one policy: resolved absolute path, a
  cleared environment, a deadline, per-stream output ceilings, a process-wide
  concurrency bound, and no silent fallback from a configured tool to another
  copy (issue #246).
- Evidence is now a function of its inputs alone. Iteration order that
  depended on the process's `HashMap` seed leaked into the output, so two
  ingests of the same build could differ; that is fixed and tested (#222,
  issue #215).

### Object code, DWARF and disassembly

- ELF and Mach-O. Mach-O `LC_MAIN` `entryoff` is normalised to a virtual
  address (#29, issue #8); chained fixups and `__DATA_CONST` are decoded;
  stub call targets are named by a two-pass `objdump`.
- Same-named local symbols in different compilation units are told apart by
  their DWARF compilation unit (#37, issue #36).
- Instruction origins — prologue, epilogue, stack protector, runtime check,
  line 0, no line, padding, inlined — with per-function and overall
  compiler-generated counts.
- Inlined subroutines from DWARF, including folded ones; folded-call detection
  for project functions and for library builtins.
- Line attribution fixed where a DWARF sequence terminator shared an address
  with a real row and hid it.
- Variable access read from DWARF and the disassembly, never from the source
  text (#80, issue #52), extended to a variable's declaration and every line
  the object code touches it on, in static instruction order (#109, issue
  #53). x86-64 global reads and writes are tracked through a materialised
  address (#16).
- Indirect call sites with static candidates (address-taken functions and
  tables that are read) and trace-resolved targets.
- Object-level dead-code candidates from address-taken analysis, the call
  graph and unreachable blocks.
- A block ending in a call to a function that does not return is no longer
  given a fall-through edge. The rule is the callee's own `DW_AT_noreturn` —
  C/C++ `_Noreturn` and `[[noreturn]]`, and any Rust `fn` returning an
  uninhabited type — read at the callee's `DW_AT_low_pc`; no symbol name is
  read as a statement about behaviour. The block records the rule, the callee
  and the call site, and the viewer shows it as *does not return*. In
  `examples/rust-qemu` this removes three edges — the fall-throughs out of
  `rustc`'s bounds-check and overflow panics — and no summary figure moves
  with them: an object branch outcome was only ever counted out of a
  two-successor block, so 27 of 42 is unchanged (issue #210).

### Coverage analysis

- MC/DC from clang's `-fcoverage-mcdc`, with the independence pairs stated in
  words — what MC/DC proved and what it did not (#40). Each pair is named by
  the tests that produced it (#84, issue #48).
- Branch-outcome coverage measured from trace transitions, and
  branch-outcome inference for blocks reachable only through a never-taken
  outcome.
- A line is drawn as partly covered when a statement on it never ran (#76,
  issue #74).
- CAST-12 sample-adequacy view; source-construct inventory and a
  source-to-object review record.
- Coverage records of the same source function are merged, and a function with
  no object code says why.

### Requirements and traceability

- Requirement tags per instruction, per line and per function, with the tag
  patterns configurable per project (`butterfly.toml`).
- A requirements universe (`ingest --requirements`) so that a requirement no
  code is tagged with is *knowable* rather than merely absent (issue #42).
- Requirements × tests matrix for DO-178C A-7 objectives 3 and 4 (#51).
- `butterfly claims`: the requirements document's claims checked against the
  measurement (#59, issue #57).
- Requirement statements in the source pane (#60), in the report (#62), and a
  requirements viewer (#68).

### Baselines and change impact

- `butterfly baseline` freezes an analysis baseline (DO-178C 4.4.2.c). An
  ingest given `--baseline <file>` compares against it and marks the affected
  reviews stale. A `butterfly-baseline.json` placed in the build directory is
  discovered and compared the same way (#238, fixed by #241); an ingest with
  neither emits `baseline: null`. The baseline
  fingerprints the actual object code (#18) — which is why
  `butterfly-baseline` is at schema `/2` and a `/1` baseline is rejected with
  an instruction rather than silently compared.
- `butterfly changes` attributes function changes by git ancestry, not author
  timestamps (#17).
- `butterfly impact`: what to test next, ranked by what one test would close
  (#50, issue #44).

### Gates, reports and the CLI

- `butterfly check`: fails a build on a coverage breach or a regression
  against a baseline (#49, issue #43). A passing run now says what it
  checked, and `--no-resolutions` means what it says (#228, issue #213).
- `butterfly run`: run the project's build script, check it with `doctor`,
  ingest, report and serve — one command, saying what it did at each step
  (#79).
- `butterfly doctor`: check a build directory before trusting it, for clang
  and for GCC (#71).
- `butterfly build`: exploratory multi-variant builds (clang/gcc/rustc/custom)
  with a Compare page, stamped "exploratory" — not certification evidence.
- `butterfly summary`, `butterfly tags`, `butterfly requirements`,
  `butterfly serve`.
- A self-contained HTML report (`butterfly report`, `/api/report`, and a
  button in the viewer). Its sections are numbered by position, so a missing
  one is visible (#202, issue #156).
- `llvm-profdata` and `llvm-cov` are paired with the clang that wrote the
  profile, rather than whatever is first on `PATH` (#54).

### Viewer

- Ten per-function tabs — Flow, Calls, Object, Gaps, MC/DC, Stack, Data,
  Variables, Changes, Requirements (`ui/app.js:852`) — beside an overview
  dashboard, a file view and a variant Compare page.
- Control-flow graph with a Sugiyama-style layout, orthogonal routed edges,
  machine-condition edge labels and a coverage overlay (none / projected /
  measured). The footer now names the overlay that is on instead of saying
  "Projected" in every mode (#211, issue #209).
- Call graph that grows outward on demand instead of one hop at a time (#101,
  issue #100).
- File view: sticky function headers, a minimap heat strip, an
  instruction-density gutter, a fold mode, a requirement lens and a file
  dashboard (risk table, coupling, dead code, baseline marks).
- Source ↔ graph highlighting is bidirectional (#39); the numbers navigate and
  explain themselves (#35).
- Gap resolutions: recorded in a sidecar, validated against the evidence they
  claim to resolve (#83, issue #66).
- Gates answered in the viewer — "does this pass?" (#77, issue #75).
- Optional "go to definition" index from Universal Ctags.
- The Stack tab's standing note no longer claims interrupt frames are
  accounted for — nothing in the stack model detects one — and now names
  dynamic frames (`alloca` / VLA), which it does detect. Interrupts and
  alternate stacks are stated to be outside the figure. The categories the
  note names are checked against the reasons `crates/ingest/src/stack.rs` can
  actually report (issue #234).
- Fixes: a leaked window-listener pair on every graph redraw (#85, issue #72);
  five review defects BF-QA-01..05 (#68); a `glob()` shadowing that blanked
  files carrying global data references; a panic in the config header
  lookback (#87, issue #86); the tab segment in the URL is validated.

### Security and robustness

Most of this landed as a hardening pass against issues #1–#11 and a later one
against #65/#67.

- Source reads are bounded to `--source-root`, with
  `--allow-external-source` as the explicit opt-out (#21, issue #1).
- The resolutions sidecar is replaced atomically and never through a symlink
  (#22, issue #2); updates are serialised with their write (#23, issue #4); a
  sidecar that cannot be read fails closed rather than being treated as empty
  (#19); multi-variant resolution filenames can no longer collide (#27, issue
  #6).
- `butterfly serve` is read-only by default off loopback and requires a bearer
  token otherwise (#24, issue #3). The token is kept out of URLs, out of
  `argv` and off plain-HTTP listeners unless the operator says otherwise
  (#106, issue #65). The expensive routes are bounded by a semaphore and
  timeouts (#107, issue #67).
- `SourceFile.functionIds` and the positional ID invariants are validated on
  load (#26, issue #11).
- Freshness is marked stale when analysed sources are missing or unreadable
  (#30, issue #10).
- Build adapters are held to what they declare (issue #245): `ingest`
  refuses a build whose `build-info.txt` declares `-g` with no line table in
  the image, has no effective `-g`, or declares `-fstack-usage` with an
  analysed function left without a `.su` record. The shell examples share
  `examples/adapter.sh` (per-unit compiles, collision-checked `.su`
  collection, mandatory `dsymutil`, compiler path and target recorded);
  CMake and Make stop on a missing or colliding `.su`; the Bazel rule
  enforces `-c dbg`, `--define llvm_bin`, `--spawn_strategy=local` and
  `--strip=never` and reads its compiler and flags from the C++ toolchain.
  This fixed `c-per-test`, `c-header-inline` and `c-unprofiled-static`,
  whose single-command builds had let one `.su` overwrite the others.
- Filesystem authority (issue #248): a hard-linked source is refused by
  `ingest` like one outside `--source-root`; the viewer's freshness check
  reads sources only under its own `--source-root` and runs `git` only in
  `--repo`, never where the evidence points; and every output (evidence,
  change cache, report, baseline) is replaced atomically through one
  directory descriptor, never written through a symlink.

### Performance

- Evidence is served from one shared buffer instead of a copy per request
  (#104, issue #73).
- `validate`'s error messages are built only when validation fails (#97).
- A benchmark harness, so scaling is measured rather than guessed (#96).

### Examples and build-system integration

- Worked examples in C, C++ and Rust, including multi-file libraries,
  header-inline duplicates in C and C++, a per-test example, and a call-graph
  example with depth, recursion, mutual recursion and indirect and external
  calls.
- A GCC example driving the `gcov` path.
- `examples/rust-qemu`: a bare-metal AArch64 image run under QEMU with its
  trace ingested — the only example in the tree that exercises `ingest
  --trace` on a real trace (#208, issue #164). It is declared local-only:
  hosted CI does not build it, and `scripts/ci-local.sh` reports a named skip
  where the host cannot.
- CMake, make and Bazel integration shown end to end (#33, issue #32).
- A build template the project owns, and the reason butterfly will not run it
  for you (#78).
- `run-all.sh`: every example built, ingested, reported and served at once
  (#81).
- Every example README now carries a `butterfly:numbers` block — functions,
  source files, lines, branch outcomes — and
  `crates/ingest/tests/example_numbers.rs` ingests each example and compares
  the block with the evidence, so a figure cannot drift in the prose, in the
  test or in the tool without a failure that names which moved. Gap totals
  are pinned in one table in that file rather than in eleven READMEs, since
  they move whenever gap classification improves. Instruction counts are not
  asserted at all: they are the host toolchain's figure, and a README that
  prints one has to say so. `examples/cpp-lib` said 479 instructions at
  `-O0` where the build produces 554, and its gap total had drifted too
  (issue #158).

### Project infrastructure

- `scripts/ci-local.sh`: every job in `ci.yml`, step for step, on one host
  (#28). It keeps the log of anything that failed and summarises why a test
  failed (#102, issue #98). It reports skips by name rather than passing
  silently.
- `support/matrix.toml`: one checked file stating what butterfly supports,
  one row per construct, toolchain and architecture. A claim with no example,
  or whose evidence has moved, fails the build (#207, issue #151). At this
  release it carries 90 rows: 21 `supported`, 41 `caveats`, 28
  `not-supported`.
- `docs/showcase/`: a generated page built from a captured run rather than
  written by hand, with the generator and the capture both in the tree, and a
  workflow that rebuilds the page from the committed capture and requires it
  to be byte-identical (#212, issue #154).
- Real-browser regression checks under Playwright, run from the repository
  instead of one laptop (#95, issue #69), sweeping every tab including
  Variables against real evidence (#205).
- rustfmt and Clippy enforced across the workspace with `-D warnings` (#25,
  issue #15).
- Every GitHub Action pinned to a reviewed commit SHA (#82, issue #64);
  `actions/checkout` and `actions/upload-artifact` bumped to 7.0.1 (#90,
  #149).
- CI Actions consumption cut by about 90% per pull request (#63), then again:
  the macOS leg dropped, prose-only changes skipped, superseded runs cancelled
  (#147). `cargo audit` moved to its own workflow so it is never cancelled.
- Test isolation fixes: HTTP test servers choose their own port and say why a
  request failed (#94, #105, issues #91 and #99); the GCC fixture is built
  only where something reads it (#103, issue #92); preflight gets a directory
  per caller rather than per process (#226, issue #148).

### Documentation and qualification

- A DO-330 assessment: where butterfly stands, which TQL applies, and what
  qualification would take (#111). It is what asked for this release's work.
- Version rules written down — this release (#120, step 1):
  `docs/qualification/versioning.md`.
- Keyboard shortcuts documented in all three places `viewerKeydown` is
  described (#203, issue #157); `examples/cpp-header-inline` explained with
  the numbers it produces (#204, issue #159).
- Corrections. Three false statements on the showcase page and a rendering
  leak (#220, issue #219), three more confirmed false statements and their
  copies elsewhere (#225, issue #221), and a plain statement that C++ and Rust
  are not checked by CI (#223). These were documentation claiming more than
  the code does; they are listed because the pattern matters more than any one
  of them.

### Known limits at this release

Stated so the entry above is not read as more than it is. The full list, with
its evidence, is `support/matrix.toml`.

- Of 90 surveyed capabilities, 21 are `supported` (worked example plus a test
  that reads it), 41 carry written caveats, and 28 are `not-supported`.
- MC/DC is supported for C. For C++ it carries caveats; **for Rust it is
  `not-supported`** (`rust.mcdc`). Rust generics, `dyn` dispatch and
  `macro_rules!` are `not-supported` too.
- Hosted CI runs Linux only. C++ and Rust are not checked by CI (#223), and
  neither is the QEMU measured-coverage example — both are verified by
  `scripts/ci-local.sh` on a developer host.
- Object coverage is *projected* from source coverage unless `ingest --trace`
  is given a real trace. Where it is projected the evidence carries no
  `measured` block, and the viewer's coverage overlay names which of the two
  it is showing (#211).
- `provenance.toolDirty` is decided when cargo runs the `ingest` build
  script, which it does on a commit, checkout, add or `git status` and on any
  change under `crates/`, `ui/`, `Cargo.toml` or `Cargo.lock`. A change
  outside those paths does not go into the binary and is not watched, so a
  developer's own build can say `clean tree` while, say, a script differs. A
  release is built from a fresh checkout and checked, so a released binary's
  answer is exact.
- butterfly is **not qualified** under DO-330 and this release does not change
  that. See `docs/qualification/DO-330-assessment.md`.

### Changed in this release

- Workspace version `0.1.0` → `0.2.0`. `butterfly --version` and
  `provenance.toolVersion` in newly written evidence change with it, and
  `--version` now adds the commit and tree state after the version (#117).
Two committed artifacts were generated by the old binary and said
`butterfly 0.1.0`. One has since been recaptured; the other is left alone
deliberately, and is named rather than left to be found:

- `docs/showcase/capture.json`, and `docs/showcase/index.html` built from it,
  **now say `0.2.0`** — the recapture #234 needed for `v07-stack.jpg` was the
  next capture this entry was waiting for. Three provenance-bearing
  screenshots moved with the version, and the page's commit line now names a
  clean tree rather than one with an uncommitted change in it.
- `examples/build-systems/bazel/report.html`, a sample report committed for
  the Bazel integration example (against `.gitignore:27`, which ignores
  `report.html`). Its provenance line and footer say "Generated by butterfly
  0.1.0". Nothing reads, tests or regenerates it — no test, no workflow, not
  even the example's own README — so nothing breaks; it is simply stale, and
  will be until someone re-runs the Bazel example. An artifact that no longer
  matches the tool that claims to have written it is the exact failure the
  DO-330 assessment describes, on a smaller scale.
