// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Preflight: the checks that turn a silent wrong answer into an instruction.

use butterfly_ingest::preflight::{check, Coverage, CoverageState, Level, Toolchain};
use std::path::PathBuf;

/// A directory no other test in this process is using.
///
/// A fresh name per call is the whole point. `cargo test` runs a suite's
/// tests concurrently in one process, so a name keyed by tag and process id
/// is unique per *process*, not per test: the two branchless tests below
/// both ask for `dir("branchless")` and were once handed one path, where
/// each one's `remove_dir_all` tore down the other's directory mid-test
/// (#148). A random, exclusively created name makes a repeated tag harmless
/// instead of merely unlikely, which a second distinct tag would not -- the
/// next test to copy a tag would bring the race straight back -- and nobody
/// else on the machine can plant it first (#344).
///
/// The duplicate `branchless` tag is deliberately left in place below, so
/// the real suite exercises this guarantee rather than only the unit test
/// that pins it.
fn dir(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("preflight-{tag}"))
}
fn ok_cov() -> Option<Coverage> {
    Some(Coverage::Read(CoverageState { is_gcov: true, has_branches: true, has_raw_branches: true, has_mcdc: false, has_decisions: true }))
}
fn clang_cov(branches: bool, mcdc: bool) -> Option<Coverage> {
    Some(Coverage::Read(CoverageState { is_gcov: false, has_branches: branches, has_raw_branches: branches, has_mcdc: mcdc, has_decisions: branches }))
}
fn gcov_cov(branches: bool, decisions: bool) -> Option<Coverage> {
    // A gcov export whose branches all became decisions: raw presence and
    // parsed presence agree. `gcov_unrepresented` is the case where they do not.
    Some(Coverage::Read(CoverageState { is_gcov: true, has_branches: branches, has_raw_branches: branches, has_mcdc: false, has_decisions: decisions }))
}

/// An export that *has* branch records none of which is a two-way decision:
/// a `switch`, or blocks whose other edges are exception edges. `-b` was
/// passed; the records are simply of a shape butterfly declines to force into
/// a decision.
fn gcov_unrepresented() -> Option<Coverage> {
    Some(Coverage::Read(CoverageState { is_gcov: true, has_branches: false, has_raw_branches: true, has_mcdc: false, has_decisions: true }))
}
fn find<'a>(fs: &'a [butterfly_ingest::preflight::Finding], kind: &str) -> Option<&'a butterfly_ingest::preflight::Finding> {
    fs.iter().find(|f| f.kind == kind)
}

/// Two callers handed the same tag must still get two directories, and the
/// second must not destroy the first's contents (#148).
///
/// This is the property, not the symptom. The symptom -- two concurrent
/// tests failing each other -- depends on thread scheduling and cannot be
/// asserted; what can be asserted is that the harness never hands the same
/// path out twice, which is what makes the scheduling irrelevant. On the
/// old harness the two paths were equal and the second call's
/// `remove_dir_all` had already deleted the first's file.
#[test]
fn one_tag_asked_for_twice_is_two_directories() {
    let a = dir("asked-twice");
    std::fs::write(a.join("build-info.txt"), "compiler=x\n").unwrap();
    let b = dir("asked-twice");
    assert_ne!(a, b, "a tag on its own must not decide the path");
    assert!(b.is_dir(), "the second directory exists: {}", b.display());
    assert!(a.join("build-info.txt").exists(), "and making it did not remove the first caller's file: {}", a.display());
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

/// The worst outcome on the list: only the instrumented binary is present, so
/// whatever is analysed measures the counters rather than the delivered code.
/// It must be `Wrong`, and it must name every build system, because the fix
/// is different in each.
#[test]
fn a_directory_of_only_instrumented_objects_is_wrong_not_empty() {
    let d = dir("instr");
    std::fs::write(d.join("build-info.txt"), "compiler=x\n").unwrap();
    let skipped = vec![(d.join("app-instr"), Toolchain::Gcc)];
    let fs = check(&d, &[], &skipped, Some(&d.join("coverage.json")), ok_cov(), &[d.join("a.su")]);

    let f = find(&fs, "no-analysis-object").expect("reported");
    assert_eq!(f.level, Level::Wrong);
    assert!(f.what.contains("app-instr"), "names the object: {}", f.what);
    assert!(f.what.to_lowercase().contains("gcc"), "and which toolchain instrumented it: {}", f.what);
    let systems: Vec<&str> = f.fixes.iter().map(|(s, _)| *s).collect();
    for want in ["build.sh", "make", "cmake", "bazel"] {
        assert!(systems.contains(&want), "a fix for {want}: {systems:?}");
    }
    let _ = std::fs::remove_dir_all(d);
}

/// An instrumented object beside a good one is correct behaviour, and is said
/// out loud so that "all of them were instrumented" and "there were none" do
/// not look identical.
#[test]
fn skipping_an_instrumented_object_is_a_note_when_a_real_one_remains() {
    let d = dir("both");
    std::fs::write(d.join("build-info.txt"), "compiler=x\n").unwrap();
    let fs = check(&d, &[d.join("app")], &[(d.join("app-instr"), Toolchain::Clang)], Some(&d.join("coverage.json")), ok_cov(), &[d.join("a.su")]);
    let f = find(&fs, "instrumented-skipped").expect("reported");
    assert_eq!(f.level, Level::Note);
    assert!(find(&fs, "no-analysis-object").is_none(), "not also reported as missing");
    let _ = std::fs::remove_dir_all(d);
}

/// gcov emits `"branches": []` when it was not asked for branches, so the key
/// is present either way. The caller parses; this only reads the verdict.
#[test]
fn a_gcov_export_without_branch_records_is_wrong() {
    let d = dir("nob");
    std::fs::write(d.join("build-info.txt"), "compiler=x\n").unwrap();
    let state = gcov_cov(false, true);
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), state, &[d.join("a.su")]);

    let f = find(&fs, "gcov-no-branches").expect("reported");
    assert_eq!(f.level, Level::Wrong);
    assert!(f.fixes.iter().any(|(s, fix)| *s == "gcc" && fix.contains("-b")), "the fix is the flag: {:?}", f.fixes);
    let _ = std::fs::remove_dir_all(d);
}

/// A clang export with no branches is a different situation and must not be
/// blamed on a gcov flag.
#[test]
fn a_clang_export_is_not_told_to_pass_a_gcov_flag() {
    let d = dir("clang");
    std::fs::write(d.join("build-info.txt"), "compiler=x\n").unwrap();
    let state = clang_cov(false, false);
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), state, &[d.join("a.su")]);
    assert!(find(&fs, "gcov-no-branches").is_none(), "not a gcov problem");
    let _ = std::fs::remove_dir_all(d);
}

/// No `.su` is weaker evidence, not wrong evidence -- the distinction the
/// levels exist to carry.
#[test]
fn missing_stack_usage_is_weaker_and_names_where_cmake_hides_it() {
    let d = dir("su");
    std::fs::write(d.join("build-info.txt"), "compiler=x\n").unwrap();
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), ok_cov(), &[]);

    let f = find(&fs, "no-stack-usage").expect("reported");
    assert_eq!(f.level, Level::Weaker);
    assert!(f.why.contains("6.3.4.f"), "says which objective it weakens");
    let cmake = f.fixes.iter().find(|(s, _)| *s == "cmake").expect("a cmake fix");
    assert!(cmake.1.contains("CMakeFiles"), "cmake's .su do not land in the build root: {}", cmake.1);
    let bazel = f.fixes.iter().find(|(s, _)| *s == "bazel").expect("a bazel fix");
    assert!(bazel.1.contains("output") || bazel.1.contains("sandbox"), "{}", bazel.1);
    let _ = std::fs::remove_dir_all(d);
}

/// No coverage export at all: the one case where neither toolchain is known,
/// so both are offered.
#[test]
fn no_coverage_export_offers_both_toolchains() {
    let d = dir("nocov");
    std::fs::write(d.join("build-info.txt"), "compiler=x\n").unwrap();
    let fs = check(&d, &[d.join("app")], &[], None, None, &[d.join("a.su")]);
    let f = find(&fs, "no-coverage").expect("reported");
    assert_eq!(f.level, Level::Wrong);
    let systems: Vec<&str> = f.fixes.iter().map(|(s, _)| *s).collect();
    assert!(systems.contains(&"clang") && systems.contains(&"gcc"), "{systems:?}");
    let _ = std::fs::remove_dir_all(d);
}

/// A directory with everything raises nothing, so the check is worth running.
#[test]
fn a_complete_directory_reports_nothing_worth_acting_on() {
    let d = dir("clean");
    std::fs::write(d.join("build-info.txt"), "compiler=x\n").unwrap();
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), ok_cov(), &[d.join("a.su")]);
    let actionable: Vec<_> = fs.iter().filter(|f| f.level != Level::Note).collect();
    // on macOS a missing .dSYM is legitimately reported; nothing else may be
    assert!(actionable.iter().all(|f| f.kind == "no-dsym"), "unexpected findings: {:?}", actionable.iter().map(|f| f.kind).collect::<Vec<_>>());
    let _ = std::fs::remove_dir_all(d);
}

/// Findings come out worst-first, so a long list still leads with what
/// matters.
#[test]
fn findings_are_ordered_worst_first() {
    let d = dir("order");
    let fs = check(&d, &[], &[(d.join("i"), Toolchain::Gcc)], None, None, &[]);
    let levels: Vec<Level> = fs.iter().map(|f| f.level).collect();
    let mut sorted = levels.clone();
    sorted.sort();
    assert_eq!(levels, sorted, "{levels:?}");
    let _ = std::fs::remove_dir_all(d);
}

/// The clang counterpart of the missing `-b`, and the more expensive one.
///
/// An export built without `-fcoverage-mcdc` carries branches and no MC/DC
/// records, so MC/DC reads 0/0 -- exactly what code with no decisions reads.
/// clang *can* do MC/DC, unlike GCC, so for Level A this is a flag away
/// rather than a limitation, and the two must not look the same.
#[test]
fn a_clang_export_without_mcdc_records_says_the_flag_is_missing() {
    let d = dir("nomcdc");
    std::fs::write(d.join("build-info.txt"), "compiler=clang\n").unwrap();
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), clang_cov(true, false), &[d.join("a.su")]);

    let f = find(&fs, "no-mcdc-instrumentation").expect("reported");
    assert_eq!(f.level, Level::Weaker);
    assert!(f.fixes.iter().any(|(s, fix)| *s == "clang" && fix.contains("-fcoverage-mcdc")), "{:?}", f.fixes);
    // and it says when it does not matter, rather than nagging a Level B project
    assert!(f.fixes.iter().any(|(_, fix)| fix.contains("Level B")), "{:?}", f.fixes);
    let _ = std::fs::remove_dir_all(d);
}

/// With the records present there is nothing to say.
#[test]
fn a_clang_export_with_mcdc_records_is_silent() {
    let d = dir("hasmcdc");
    std::fs::write(d.join("build-info.txt"), "compiler=clang\n").unwrap();
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), clang_cov(true, true), &[d.join("a.su")]);
    assert!(find(&fs, "no-mcdc-instrumentation").is_none());
    let _ = std::fs::remove_dir_all(d);
}

/// GCC records no MC/DC at all, so telling a GCC project to add a clang flag
/// would be advice it cannot take.
#[test]
fn a_gcov_export_is_never_told_to_add_a_clang_mcdc_flag() {
    let d = dir("gcovmcdc");
    std::fs::write(d.join("build-info.txt"), "compiler=gcc\n").unwrap();
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), gcov_cov(true, true), &[d.join("a.su")]);
    assert!(find(&fs, "no-mcdc-instrumentation").is_none(), "GCC cannot do MC/DC; the advice would be impossible");
    let _ = std::fs::remove_dir_all(d);
}

/// Stopping one step early looks exactly like never instrumenting, and the
/// fix is completely different. Both toolchains leave their own trace.
#[test]
fn raw_profile_data_with_no_export_names_the_step_that_was_skipped() {
    for (tag, raw, want) in [("clangraw", "d.profraw", "llvm-profdata"), ("gccraw", "d.gcda", "gcov")] {
        let d = dir(tag);
        std::fs::write(d.join(raw), b"x").unwrap();
        let fs = check(&d, &[d.join("app")], &[], None, None, &[d.join("a.su")]);
        let f = find(&fs, "profile-not-exported").unwrap_or_else(|| panic!("{tag}: reported"));
        assert_eq!(f.level, Level::Wrong);
        assert!(f.what.contains(raw), "names the file: {}", f.what);
        assert!(f.fixes.iter().any(|(_, fix)| fix.contains(want)), "{tag}: the right tool: {:?}", f.fixes);
        let _ = std::fs::remove_dir_all(d);
    }
}

/// A straight-line program exported *with* `-b` has no branch records either,
/// and telling its author to add the flag they already passed is advice that
/// cannot be acted on. The check needs evidence that a decision existed, not
/// merely that outcomes are absent (review of #45).
#[test]
fn a_branchless_program_is_not_told_to_add_a_flag_it_already_passed() {
    let d = dir("branchless");
    std::fs::write(d.join("build-info.txt"), "compiler=gcc\n").unwrap();
    // no branches AND no decisions: nothing to measure, nothing to advise
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), gcov_cov(false, false), &[d.join("a.su")]);
    assert!(find(&fs, "gcov-no-branches").is_none(), "{:?}", fs.iter().map(|f| f.kind).collect::<Vec<_>>());
    let _ = std::fs::remove_dir_all(d);
}

/// ...but where the producer recorded decisions and no outcomes, `-b` really
/// is missing. gcov lists a condition entry per decision whether or not `-b`
/// was passed, which is what tells the two apart.
#[test]
fn decisions_without_outcomes_is_the_missing_flag() {
    let d = dir("missingb");
    std::fs::write(d.join("build-info.txt"), "compiler=gcc\n").unwrap();
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), gcov_cov(false, true), &[d.join("a.su")]);
    let f = find(&fs, "gcov-no-branches").expect("reported");
    assert_eq!(f.level, Level::Wrong);
    let _ = std::fs::remove_dir_all(d);
}

/// An export that is present and unreadable is not "no coverage features".
/// ingest will refuse it, so doctor passing it would be advice that expires
/// the moment it is acted on.
#[test]
fn an_unreadable_export_is_wrong_not_merely_featureless() {
    let d = dir("unreadable");
    std::fs::write(d.join("build-info.txt"), "compiler=clang\n").unwrap();
    let state = Some(Coverage::Unreadable("coverage export is not valid JSON".into()));
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), state, &[d.join("a.su")]);

    let f = find(&fs, "coverage-unreadable").expect("reported");
    assert_eq!(f.level, Level::Wrong);
    assert!(f.what.contains("not valid JSON"), "carries the reason: {}", f.what);
    // and it is not also reported as missing, which would be contradictory
    assert!(find(&fs, "no-coverage").is_none());
    // nor is it silently mistaken for a missing -b
    assert!(find(&fs, "gcov-no-branches").is_none());
    let _ = std::fs::remove_dir_all(d);
}

/// A `switch` exports three non-throw edges from one block. butterfly reads a
/// decision only where a block has exactly two, so such an export has branch
/// records but yields no branches -- and the missing-`-b` rule, which looked
/// only at the parsed branches, told the operator to add a flag they had
/// already passed. Advice that cannot be acted on is worse than none: it
/// sends a reviewer to change a build that was correct.
///
/// The two situations must be diagnosed apart (review of #45).
#[test]
fn a_multiway_export_is_not_blamed_on_a_missing_flag() {
    let d = dir("multiway");
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), gcov_unrepresented(), &[d.join("a.su")]);

    assert!(find(&fs, "gcov-no-branches").is_none(), "must not tell them to add -b they already passed: {:?}", fs.iter().map(|f| f.kind).collect::<Vec<_>>());

    let f = find(&fs, "gcov-no-two-way-decisions").expect("the real situation is named");
    assert_eq!(f.level, Level::Weaker, "the build is right; the evidence is simply thinner");
    for (_, fix) in &f.fixes {
        assert!(!fix.contains("-b:"), "the fix must not be a flag change: {fix}");
    }
}

/// ...and the genuinely missing flag still is diagnosed. gcov lists a
/// `conditions` entry per decision whether or not `-b` was passed, so a
/// decision with no branch records at all, raw or parsed, really is the
/// omitted flag.
#[test]
fn an_omitted_b_flag_is_still_caught() {
    let d = dir("omitted-b");
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), gcov_cov(false, true), &[d.join("a.su")]);

    let f = find(&fs, "gcov-no-branches").expect("still caught");
    assert_eq!(f.level, Level::Wrong);
    assert!(f.fixes.iter().any(|(_, fix)| fix.contains("-b")), "and still names the flag");
    assert!(find(&fs, "gcov-no-two-way-decisions").is_none(), "only one diagnosis applies");
}

/// A straight-line program exported *with* `-b`: no decisions, no records,
/// nothing wrong. Neither rule may fire.
#[test]
fn a_branchless_program_draws_neither_complaint() {
    let d = dir("branchless");
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), gcov_cov(false, false), &[d.join("a.su")]);

    assert!(find(&fs, "gcov-no-branches").is_none(), "nothing to fix in a branchless program");
    assert!(find(&fs, "gcov-no-two-way-decisions").is_none());
    assert!(!fs.iter().any(|f| f.level == Level::Wrong), "{:?}", fs.iter().map(|f| f.kind).collect::<Vec<_>>());
}
