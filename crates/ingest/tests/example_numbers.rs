// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Every number an example's README states about its own evidence, checked
//! against that evidence (#158).
//!
//! The survey that opened #158 found `examples/cpp-lib`'s README claiming 35
//! gaps and 479 instructions where the build on disk produced 36 and 554,
//! and `examples/c-lib`'s 18 functions / 87 of 112 lines / 31 gaps asserted
//! by no test at all. A selling page that quotes numbers must quote numbers
//! a test guards, or the numbers are decoration.
//!
//! # How it is tied together
//!
//! Each example README carries one machine-read block:
//!
//! ```text
//! <!-- butterfly:numbers -->
//! | functions | 18 |
//! | source files | 8 |
//! | lines covered | 87 / 112 |
//! | branch outcomes | 25 / 40 |
//! | instructions | host-dependent |
//! <!-- /butterfly:numbers -->
//! ```
//!
//! and this file holds the same numbers in `EXPECTED`. The two are compared
//! with each other and both are compared with a fresh ingest, so a figure
//! cannot drift in the README, in the test, or in the tool without a
//! failure that names which of the three moved.
//!
//! # What is not asserted, and why
//!
//! **Instruction counts.** They are a property of the host's compiler and
//! linker, not of the example: the same source built by two clang releases
//! gives two totals. The block records the word `host-dependent` instead of
//! a number, and the test checks only that there is object code at all.
//!
//! **Gap totals**, for the same reason and with a measurement behind it:
//! the review of #237 recorded `c-callgraph` at 26 on Apple clang 21 where
//! this host's clang 17 gives 25. A gap is a statement about object code,
//! so its count carries the compiler's choices with it. `GapShape` below
//! says what is asserted instead, and why an equality on either host's
//! figure would be the same defect this file was written against.
//!
//! Both are held to the same prose rule, which needs no compiler at all: a
//! README may record either figure, but only in a sentence that says whose
//! toolchain produced it.
//!
//! **Anything below the summary.** Per-function tables, stack figures and
//! the dead-code lists are asserted by the suites that exist for them
//! (`e2e_demo_c`, `e2e_rust_qemu`, `dup_static_symbols`, ...). This file is
//! the summary, which is what the READMEs and `examples/README.md` quote.
//!
//! # Which examples this file can check where
//!
//! Two kinds of check live here, and they need different things:
//!
//! * the **prose** checks read `examples/*/README.md` and nothing else.
//!   They run for all twelve examples anywhere, including a checkout with
//!   no `build/` directory at all. They are the ones that catch the drift
//!   #158 found.
//! * the **evidence** checks ingest a built example. They can only run
//!   where that example has been built.
//!
//! `.github/workflows/ci.yml` builds eleven of the twelve (`CI_PROVISIONED`
//! below, and that list is checked against the workflow rather than merely
//! written here). Among the last to arrive were `cpp` and `cpp-lib` (#162),
//! `rust` and `rust-lib` (#152), the Rust pair with a dated nightly for
//! `-Zcoverage-options=condition`, and `rust-constructs` (#167) on the
//! same nightly. The one it does not build is
//! `rust-qemu`, which needs QEMU and a bare-metal target and is declared
//! `[[local_only]]` in `support/matrix.toml`: `scripts/ci-local.sh` builds
//! it wherever those exist.
//!
//! So the evidence checks are scoped to what the gate actually provisions,
//! and the scope is stated out loud rather than hidden in a skip: an
//! unprovisioned example whose fixture is absent prints a line saying its
//! evidence went unchecked and why, and the run's last line reports how
//! many of the twelve were reached. An example that *is* provisioned and
//! still missing prints the `skipping:` line both entry points turn into a
//! failure, which is unchanged. An unprovisioned example that happens to be
//! built -- after `examples/run-all.sh`, or a ci-local run on a host with
//! QEMU -- is checked anyway; the scope is a floor, not a ceiling.

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// One example's summary, as its README states it.
struct Expected {
    dir: &'static str,
    functions: usize,
    files: usize,
    lines: (usize, usize),
    branches: (usize, usize),
}

const EXPECTED: &[Expected] = &[
    Expected { dir: "c", functions: 10, files: 1, lines: (57, 69), branches: (27, 34) },
    Expected { dir: "c-lib", functions: 18, files: 8, lines: (87, 112), branches: (25, 40) },
    Expected { dir: "cpp", functions: 11, files: 1, lines: (55, 67), branches: (26, 34) },
    Expected { dir: "cpp-lib", functions: 24, files: 8, lines: (80, 108), branches: (26, 44) },
    Expected { dir: "c-header-inline", functions: 10, files: 7, lines: (22, 23), branches: (3, 8) },
    Expected { dir: "cpp-header-inline", functions: 6, files: 4, lines: (14, 15), branches: (3, 4) },
    Expected { dir: "cpp-constructs", functions: 21, files: 1, lines: (83, 86), branches: (19, 24) },
    Expected { dir: "c-callgraph", functions: 13, files: 1, lines: (59, 61), branches: (5, 6) },
    Expected { dir: "c-switch", functions: 2, files: 1, lines: (16, 27), branches: (25, 36) },
    Expected { dir: "cpp-exceptions", functions: 9, files: 1, lines: (34, 36), branches: (5, 6) },
    Expected { dir: "c-constructs", functions: 15, files: 1, lines: (120, 134), branches: (20, 30) },
    Expected { dir: "c-per-test", functions: 14, files: 2, lines: (64, 77), branches: (43, 64) },
    Expected { dir: "rust", functions: 8, files: 1, lines: (47, 56), branches: (11, 14) },
    Expected { dir: "rust-lib", functions: 18, files: 6, lines: (85, 108), branches: (13, 22) },
    Expected { dir: "rust-constructs", functions: 15, files: 1, lines: (55, 60), branches: (7, 12) },
    Expected { dir: "rust-qemu", functions: 9, files: 2, lines: (67, 79), branches: (21, 30) },
];

/// The examples `.github/workflows/ci.yml`'s `test` job builds, which is
/// the set whose evidence this file can require anywhere.
///
/// This is not a wish: `the_provisioned_list_is_what_ci_actually_builds`
/// reads the workflow and fails if the two disagree. When a build step is
/// added, that test is what says so, and moving the name here is the whole
/// change needed to promote an example from "prose checked" to "evidence
/// required" -- which is how `rust` and `rust-lib` arrived (#152).
const CI_PROVISIONED: &[&str] = &[
    "c",
    "c-callgraph",
    "c-constructs",
    "c-header-inline",
    "c-lib",
    "c-per-test",
    "c-switch",
    "cpp",
    "cpp-constructs",
    "cpp-exceptions",
    "cpp-header-inline",
    "cpp-lib",
    "rust",
    "rust-constructs",
    "rust-lib",
];

/// What the gap classifier has to keep true of each example.
///
/// # Why this is not a table of exact totals
///
/// It was one, and the totals were not portable. A gap is a statement about
/// object code, so a gap total carries the compiler's choices with it. The
/// independently recorded review of #237 measured `c-callgraph` at **26**
/// on Apple clang 21 where this host's Apple clang 17 measures **25**, and
/// after #237 the same two hosts give 11 and 10. An `assert_eq!` on either
/// figure is green on one machine and red on the other, which is the drift
/// #158 exists to stop, wearing a reviewer's hat instead of a README's.
///
/// Three shapes were offered. A **pinned fixture producer** (a container
/// with one archived clang, its provenance recorded) is the one that would
/// let the exact integers come back, and it is the right long-term answer
/// for a DO-178C tool qualification argument -- but it is a build-system
/// change with a billed-minutes cost, and #147 removed a whole runner leg
/// to save those minutes. **Per-environment expectations** cannot be
/// written honestly from here: this host has one clang, so a clang-21
/// column would be copied from a review comment and asserted by nobody --
/// a number in a table that no run on this machine can confirm is the
/// thing #158 was opened about.
///
/// So: **stable semantic assertions**, and the exact totals are recorded
/// instead of asserted (they are printed on every run, see
/// `every_example_keeps_the_gap_shape_pinned_for_it`). Measured here on
/// Apple clang 17.0.0 / arm64-apple-darwin25, 2026-09-21: `c` 28, `c-lib`
/// 31, `cpp` 30, `cpp-lib` 36, `c-header-inline` 14, `cpp-header-inline`
/// 5, `c-callgraph` 25, `c-per-test` 38, `rust` 63, `rust-lib` 78,
/// `rust-qemu` 56; and on 2026-09-25, `cpp-constructs` 23 (#166), then 31
/// once its constructor/destructor variants were analysed (#266). Ubuntu
/// clang 18.1.3 / x86-64, hosted CI 2026-10-01: `cpp-exceptions` 31 (#325).
///
/// # What is asserted instead
///
/// * `must_produce` -- the gap kinds the example's *source* guarantees: it
///   calls into libc/`core` (`helper-call`), or it contains a call through
///   a function pointer (`indirect-branch`). These are properties of the
///   example, not of the optimiser, and they are what each fixture exists
///   to demonstrate. The list is short on purpose: `line-zero`,
///   `folded-call`, `predicated-decision`, `extra-object-branch` and
///   `shared-line` are all the compiler's choice on the day, and requiring
///   them would re-import exactly the fragility this replaces.
/// * `ceiling` -- the total stays accountable. This is the repo's own
///   idiom: `butterfly check --max-unresolved-gaps 40` in ci.yml's
///   end-to-end step bounds the same quantity the same way. Each ceiling is
///   the next multiple of ten at least a quarter above the highest figure
///   any recorded environment has produced; the largest cross-release delta
///   on record is +1 on 25, so the margin is some twenty times what has
///   been observed.
/// * the structural invariants in
///   `the_gap_records_are_internally_consistent`, which hold on any
///   toolchain.
/// * and, in `no_readme_prints_an_object_derived_count_as_a_fact`, the
///   thing #158 actually found: a README may not print a gap count as fact
///   any more than an instruction count, for the same reason.
///
/// A bound is weaker than an equality. It is also the strongest check here
/// that is true on more than one machine, and it does not pretend
/// otherwise.
///
/// # For #237
///
/// Nothing in this table needs to move. #237 lowers six totals
/// (`cpp-header-inline` 5 -> 3, `c-header-inline` 14 -> 11, `c-callgraph`
/// 25 -> 10, `c-per-test` 38 -> 30, `c-lib` 31 -> 30, `cpp-lib` 36 -> 35;
/// `c` unchanged), every one of them further below its ceiling, and every
/// one still producing the kinds required of it. A new example --
/// `c-unprofiled-static` -- needs a row here and in `EXPECTED`, and a
/// numbers block in its README, the same as any other example.
struct GapShape {
    dir: &'static str,
    must_produce: &'static [&'static str],
    ceiling: usize,
}

const GAP_SHAPE: &[GapShape] = &[
    GapShape { dir: "c", must_produce: &["helper-call"], ceiling: 40 },
    GapShape { dir: "c-lib", must_produce: &["helper-call"], ceiling: 40 },
    GapShape { dir: "cpp", must_produce: &["helper-call"], ceiling: 40 },
    GapShape { dir: "cpp-lib", must_produce: &["helper-call"], ceiling: 50 },
    GapShape { dir: "c-header-inline", must_produce: &["helper-call"], ceiling: 20 },
    GapShape { dir: "cpp-header-inline", must_produce: &["helper-call"], ceiling: 10 },
    // 23 on Apple clang 17 / arm64 (#166), 31 once the constructor and
    // destructor variants were analysed (#266). `new` is a helper call, a
    // virtual call and a call through a lambda's function pointer are
    // indirect branches, and a virtual destructor's deleting variant (D0) is
    // an uncounted variant, whatever the compiler.
    GapShape { dir: "cpp-constructs", must_produce: &["helper-call", "indirect-branch", "uncounted-variant"], ceiling: 40 },
    // The one fixture with a call through a function pointer, which is why
    // it exists (#100): no indirect branch means the Calls graph lost the
    // node state this example is here to draw.
    GapShape { dir: "c-callgraph", must_produce: &["helper-call", "indirect-branch"], ceiling: 40 },
    // 25 on Apple clang 17 / arm64 (#171): the jump table's indirect branch
    // is the point of the example, and `puts` the helper call
    GapShape { dir: "c-switch", must_produce: &["helper-call", "indirect-branch"], ceiling: 40 },
    // 22 on Apple clang 17 / arm64 (#165): the throw, catch and dynamic_cast
    // all call into the C++ runtime. 31 on Ubuntu clang 18.1.3 / x86-64 ELF
    // (hosted CI, 2026-10-01, #325): helper-call 17, line-zero 11,
    // extra-object-branch 2, uncounted-variant 1 -- more runtime calls and
    // line-0 landing-pad code than arm64, no indirect branch. The first
    // hosted run to reach this test; the ceiling follows the rule above.
    GapShape { dir: "cpp-exceptions", must_produce: &["helper-call"], ceiling: 40 },
    // 26 on Apple clang 17 / arm64 (#168). Only the libc calls are the
    // source's own guarantee: the macro's inferred `shared-line` blocks and
    // the stack-probe call AArch64 Darwin makes for a dynamic frame are both
    // the target's choice, and x86-64's va_arg adds extra-object-branches.
    GapShape { dir: "c-constructs", must_produce: &["helper-call"], ceiling: 40 },
    GapShape { dir: "c-per-test", must_produce: &["helper-call"], ceiling: 50 },
    GapShape { dir: "rust", must_produce: &["helper-call"], ceiling: 80 },
    GapShape { dir: "rust-lib", must_produce: &["helper-call"], ceiling: 100 },
    // The one Rust fixture with a call through `dyn` (#167): no indirect
    // branch means the vtable call it exists to show was lost. Measured at
    // 91 on rustc 1.97.1 / aarch64-apple-darwin, 2026-09-25.
    GapShape { dir: "rust-constructs", must_produce: &["helper-call", "indirect-branch"], ceiling: 120 },
    GapShape { dir: "rust-qemu", must_produce: &["helper-call"], ceiling: 70 },
];

/// Ingest one example exactly as its README tells a reader to, or say why
/// not.
///
/// For an example CI provisions, absence is the `skipping:` line both
/// `.github/workflows/ci.yml` and `scripts/ci-local.sh` grep for and turn
/// into a failure: a suite that quietly tested nothing is what this file
/// exists to prevent (#227).
///
/// For one it does not, absence is *not* that line -- it would fail every
/// clean-checkout run of a gate that was never asked to build the fixture.
/// It is a line naming the example that went unchecked and who owns
/// provisioning it, printed on every run, so "not checked here" stays
/// visible instead of becoming silence.
fn evidence(dir: &str) -> Option<butterfly_model::Evidence> {
    let root = repo().join("examples").join(dir);
    let build = root.join("build");
    if !build.join("coverage.json").exists() {
        if CI_PROVISIONED.contains(&dir) {
            // A provisioned fixture is required where the fixtures are (#242):
            // absent, this fails rather than returning without asserting.
            butterfly_testsupport::fixture_missing(&format!("examples/{dir}/build not present (run examples/{dir}/build.sh)"));
        } else {
            // Not provisioned by the hosted gate (rust-qemu, declared
            // [[local_only]] in support/matrix.toml): intentionally a soft
            // skip, not the "skipping:" the gate greps for. The README prose
            // is still checked.
            eprintln!(
                "example_numbers: examples/{dir} is not built by .github/workflows/ci.yml and is not built here, so its evidence went unchecked in this run (see [[local_only]] in support/matrix.toml). Its README prose was still checked."
            );
        }
        return None;
    }
    let mut opts = butterfly_ingest::discover(&build).unwrap_or_else(|e| panic!("discover examples/{dir}/build: {e}"));
    opts.source_root = Some(root);
    Some(butterfly_ingest::ingest(&opts).unwrap_or_else(|e| panic!("ingest examples/{dir}: {e}")))
}

/// The rows of one README's `butterfly:numbers` block, as `(label, value)`.
fn numbers_block(readme: &Path) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(readme).unwrap_or_else(|e| panic!("{}: {e}", readme.display()));
    let (_, rest) = text.split_once("<!-- butterfly:numbers -->").unwrap_or_else(|| panic!("{} has no <!-- butterfly:numbers --> block", readme.display()));
    let (block, _) = rest.split_once("<!-- /butterfly:numbers -->").unwrap_or_else(|| panic!("{}: the numbers block is not closed", readme.display()));
    block
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('|') && !l.contains("---"))
        .filter_map(|l| {
            let cells: Vec<&str> = l.trim_matches('|').split('|').map(str::trim).collect();
            (cells.len() == 2 && !cells[0].is_empty()).then(|| (cells[0].to_string(), cells[1].replace("**", "")))
        })
        .collect()
}

fn row<'a>(rows: &'a [(String, String)], label: &str, readme: &Path) -> &'a str {
    rows.iter().find(|(k, _)| k == label).map(|(_, v)| v.as_str()).unwrap_or_else(|| panic!("{}: the numbers block has no `{label}` row", readme.display()))
}

/// The evidence agrees with `EXPECTED`.
#[test]
fn every_example_produces_the_summary_its_readme_states() {
    let mut checked = 0;
    for e in EXPECTED {
        let Some(ev) = evidence(e.dir) else { continue };
        checked += 1;
        let s = &ev.summary;
        let got = (s.functions, s.files, (s.lines_covered, s.lines_total), (s.branches_covered, s.branches_total));
        let want = (e.functions, e.files, e.lines, e.branches);
        assert_eq!(
            got, want,
            "examples/{}: (functions, files, lines, branch outcomes) moved. Update the README's numbers block and EXPECTED together, or find out what changed.",
            e.dir
        );
        assert!(s.instructions_total > 0, "examples/{}: no object code was analysed at all", e.dir);
    }
    eprintln!(
        "example_numbers: checked the evidence of {checked} of {} examples ({} of them provisioned by ci.yml: {})",
        EXPECTED.len(),
        CI_PROVISIONED.len(),
        CI_PROVISIONED.join(", ")
    );
}

/// `CI_PROVISIONED` is what the workflow builds, not what this file hopes
/// it builds.
///
/// Without this the scope above rots the moment either side moves: a build
/// could be added and the evidence checks would still be scoped away from
/// it, or a build step could be dropped from the gate and this
/// file would go on requiring a fixture nothing produces. Reading the
/// workflow costs nothing and needs no fixture, so it runs everywhere the
/// prose checks do.
#[test]
fn the_provisioned_list_is_what_ci_actually_builds() {
    let ci = repo().join(".github/workflows/ci.yml");
    let text = std::fs::read_to_string(&ci).unwrap_or_else(|e| panic!("{}: {e}", ci.display()));
    // Only a step that actually runs one: `run: ./examples/<dir>/build.sh`.
    // The workflow also *names* `examples/rust-qemu/build.sh` inside the
    // skip-filter grep, and matching that would claim a fixture nobody
    // builds.
    let mut built: Vec<&str> =
        text.lines().map(str::trim).filter_map(|l| l.strip_prefix("run: ./examples/")).filter_map(|l| l.strip_suffix("/build.sh")).filter(|d| !d.contains('/')).collect();
    built.sort_unstable();
    built.dedup();
    // c-gcc is built by CI and has no README numbers block: the intersection
    // with EXPECTED is what this file can scope by.
    let want: Vec<&str> = built.into_iter().filter(|d| EXPECTED.iter().any(|e| e.dir == *d)).collect();
    let mut have = CI_PROVISIONED.to_vec();
    have.sort_unstable();
    assert_eq!(
        have, want,
        "CI_PROVISIONED and .github/workflows/ci.yml disagree about which examples the gate builds. If you just added a build step, add its name to CI_PROVISIONED and that example's evidence becomes required; if you removed one, remove the name."
    );
}

/// The prose checks are wired to a workflow that runs on prose.
///
/// `ci.yml` ignores `**/*.md` for both events (#147), so the two checks
/// that read `examples/*/README.md` would never see a documentation-only
/// pull request -- the exact change that can introduce the drift they
/// exist to catch. `.github/workflows/docs-numbers.yml` is what runs them,
/// and this is what stops that wiring rotting: rename a check without
/// renaming it there and the job becomes a green no-op, which is the
/// failure mode #227 is about.
#[test]
fn the_readme_checks_run_on_readme_only_changes() {
    let wf = repo().join(".github/workflows/docs-numbers.yml");
    let text = std::fs::read_to_string(&wf)
        .unwrap_or_else(|e| panic!("{}: {e} -- the README number checks need a workflow that is triggered by README changes; ci.yml ignores **/*.md", wf.display()));
    for needed in ["examples/**/*.md", "every_readme_states_the_numbers_this_test_asserts", "no_readme_prints_an_object_derived_count_as_a_fact"] {
        assert!(text.contains(needed), "{}: does not mention `{needed}`, so a change to an example README would not run the check that reads it", wf.display());
    }
}

/// The README states the same numbers this file does. Without this the test
/// pins the tool and the prose drifts anyway, which is what happened to
/// `cpp-lib`.
#[test]
fn every_readme_states_the_numbers_this_test_asserts() {
    for e in EXPECTED {
        let readme = repo().join("examples").join(e.dir).join("README.md");
        let rows = numbers_block(&readme);
        let get = |label: &str| row(&rows, label, &readme);
        assert_eq!(get("functions"), e.functions.to_string(), "examples/{}: README function count", e.dir);
        assert_eq!(get("source files"), e.files.to_string(), "examples/{}: README source-file count", e.dir);
        assert_eq!(get("lines covered"), format!("{} / {}", e.lines.0, e.lines.1), "examples/{}: README line coverage", e.dir);
        assert_eq!(get("branch outcomes"), format!("{} / {}", e.branches.0, e.branches.1), "examples/{}: README branch outcomes", e.dir);
        // Host-dependent figures are marked, never printed as fact.
        assert_eq!(
            get("instructions"),
            "host-dependent",
            "examples/{}: an instruction count is a property of this host's toolchain, so the README must say so rather than print one",
            e.dir
        );
    }
}

/// Gaps, from the one table that owns them. See `GapShape` for why this
/// bounds and characterises the total instead of equalling it.
#[test]
fn every_example_keeps_the_gap_shape_pinned_for_it() {
    assert_eq!(GAP_SHAPE.len(), EXPECTED.len(), "every example needs a pinned gap shape");
    let mut measured: Vec<String> = Vec::new();
    for s in GAP_SHAPE {
        assert!(EXPECTED.iter().any(|e| e.dir == s.dir), "{} is not an example", s.dir);
        let Some(ev) = evidence(s.dir) else { continue };
        let total = ev.summary.gaps_total;
        let by_kind = &ev.summary.gaps_by_kind;
        assert!(
            total <= s.ceiling,
            "examples/{}: {total} gaps, above the ceiling of {}. The exact total is the host toolchain's figure and is not asserted, but it stays accountable: either the classifier has started producing gaps it should not, or the fixture grew and the ceiling in GAP_SHAPE has to be raised deliberately.\n  by kind: {by_kind:?}",
            s.dir,
            s.ceiling
        );
        for kind in s.must_produce {
            assert!(
                by_kind.get(*kind).copied().unwrap_or(0) > 0,
                "examples/{}: no `{kind}` gap at all. This example's source guarantees one, so its absence is the classifier no longer seeing what the fixture is here to demonstrate -- not a number moving.\n  by kind: {by_kind:?}",
                s.dir
            );
        }
        measured.push(format!("{} {total}", s.dir));
    }
    // Recorded, not asserted: the log of every run carries this host's
    // figures, so a reviewer on another toolchain can compare without a
    // test having claimed their machine would produce ours.
    eprintln!("example_numbers: gap totals measured on this host ({}): {}", std::env::consts::ARCH, measured.join(", "));
}

/// The invariants a gap record has to satisfy on any toolchain.
///
/// None of these is a count, so none of them moves with the compiler. They
/// are also the ones with teeth: a duplicate key silently merges two
/// reviewers' 6.4.4.3 justifications onto one gap, and a `helper-call` with
/// no class is a gap the viewer cannot bucket.
#[test]
fn the_gap_records_are_internally_consistent() {
    for e in EXPECTED {
        let Some(ev) = evidence(e.dir) else { continue };
        let gaps: Vec<&butterfly_model::Gap> = ev.functions.iter().flat_map(|f| f.objects()).flat_map(|o| o.gaps.iter()).collect();
        assert_eq!(gaps.len(), ev.summary.gaps_total, "examples/{}: the summary counts {} gaps, the functions carry {}", e.dir, ev.summary.gaps_total, gaps.len());
        assert_eq!(
            ev.summary.gaps_by_kind.values().sum::<usize>(),
            ev.summary.gaps_total,
            "examples/{}: gaps_by_kind does not sum to gaps_total, so some gap has a kind the summary did not count",
            e.dir
        );
        let mut keys: Vec<&str> = gaps.iter().map(|g| g.key.as_str()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "examples/{}: two gaps share a resolution key. A justification written against one would silently apply to both.", e.dir);
        for g in &gaps {
            assert!(!g.key.is_empty(), "examples/{}: a gap has no resolution key, so nothing can be justified against it", e.dir);
            assert!(!g.description.is_empty(), "examples/{}: a gap with key {} has no description for a reviewer to read", e.dir, g.key);
            assert!(g.addr_end >= g.addr_start, "examples/{}: gap {} spans backwards", e.dir, g.key);
            if g.kind == butterfly_model::GapKind::HelperCall {
                assert!(
                    g.class.as_deref().is_some_and(|c| !c.is_empty()),
                    "examples/{}: the helper call at {} is classified as nothing at all. `unknown` is a class; absent is the classifier declining to answer.",
                    e.dir,
                    g.key
                );
            }
        }
    }
}

/// Is `chunk` printing a count of `noun`?
///
/// The unit is "a figure, then up to two qualifying words, then the noun":
/// prose puts adjectives in between ("12 `runtime-check` instructions",
/// "33 of this example's 63 gaps"), and looking only at the word
/// immediately before the noun missed every one of those. No regex:
/// `regex` is a dependency of the library, not of its integration tests.
fn counts(chunk: &str, noun: &str) -> bool {
    // A figure, once Markdown's wrappers are off: `**8`, `(7`, `~50`, `20`.
    // Only the wrappers -- stripping every non-digit instead would read the
    // `2` out of `-O2` and the `9` out of "A-7 objective 9", and call
    // "at `-O2` the gap is still there" a gap count.
    let bare_number = |t: &str| {
        let t = t.trim_matches(|c: char| "`*()[]{}<>,.;:!?\"'~—–|".contains(c));
        !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())
    };
    let tokens: Vec<&str> = chunk.split_whitespace().collect();
    tokens.iter().enumerate().any(|(i, t)| {
        let word = t.trim_matches(|c: char| !c.is_ascii_alphabetic());
        (word == noun || word.strip_suffix('s') == Some(noun)) && tokens[i.saturating_sub(3)..i].iter().any(|p| bare_number(p))
    })
}

/// No example README may print a bare instruction or gap count as fact.
///
/// This is the drift #158 found: `cpp-lib` said 479 instructions at `-O0`
/// long after the build on disk produced 554, and 35 gaps where it produced
/// 36, and nothing noticed because nothing read it.
///
/// Both figures are the host toolchain's, not the example's. The
/// instruction count obviously so; the gap count just as much, because a
/// gap is a statement about object code -- the review of #237 measured the
/// same `c-callgraph` fixture at 25 on one Apple clang and 26 on another.
/// That is exactly why `GapShape` bounds the total rather than equalling
/// it, and it is why a README may not quote one flat either. This check is
/// the portable half of that answer: it needs no compiler, no fixture and
/// no agreement about which clang a reader has, and it is the one that
/// would have caught the original defect.
#[test]
fn no_readme_prints_an_object_derived_count_as_a_fact() {
    // Every offender, not the first: a README with three of them should
    // take one edit pass, not three runs.
    let mut found: Vec<String> = Vec::new();
    for e in EXPECTED {
        let readme = repo().join("examples").join(e.dir).join("README.md");
        let text = std::fs::read_to_string(&readme).unwrap_or_else(|err| panic!("{}: {err}", readme.display()));
        // Markdown wraps a sentence over several lines, so the unit here is
        // the paragraph (or the single table row, or the one bullet), not
        // the line: a figure and the words that qualify it are routinely on
        // either side of a line break. A bullet or a heading does start a
        // fresh one -- a list read as a single paragraph reports the line of
        // the first bullet for a figure twenty lines below it, and lets a
        // "on this host" in one item excuse a figure in another.
        let mut chunks: Vec<(usize, String)> = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            let bullet =
                t.starts_with("* ") || t.starts_with("- ") || t.starts_with("#") || t.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
            let fresh = line.trim().is_empty() || line.starts_with('|') || bullet || chunks.last().is_some_and(|(_, c)| c.starts_with('|'));
            match chunks.last_mut() {
                Some((_, c)) if !fresh => {
                    c.push(' ');
                    c.push_str(line.trim());
                }
                _ => chunks.push((i + 1, line.trim().to_string())),
            }
        }
        for (at, chunk) in &chunks {
            let Some(noun) = ["instruction", "gap"].into_iter().find(|n| counts(chunk, n)) else { continue };
            // A figure is allowed when the sentence that carries it says it
            // is this host's, in the sentence itself -- not in a paragraph
            // somewhere above it that a reader may not have reached.
            // Case-folded: a sentence may open with "On this host, ...".
            let lower = chunk.to_lowercase();
            let marked = lower.contains("host-dependent") || lower.contains("on this host") || lower.contains("this host's") || lower.contains("this toolchain's");
            if !marked {
                found.push(format!("examples/{}/README.md:{at}: a {noun} count is printed as fact.\n  {chunk}", e.dir));
            }
        }
    }
    assert!(
        found.is_empty(),
        "an object-derived count is printed as fact. It is the host toolchain's figure, not the example's -- the same source built by two clang releases gives two totals: mark it host-dependent in the same sentence, or drop it.\n\n{}",
        found.join("\n")
    );
}
