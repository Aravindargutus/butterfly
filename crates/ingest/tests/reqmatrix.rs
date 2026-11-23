// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The requirements × tests matrix (#42), against the per-test example.
//!
//! Most cases run only when examples/c-per-test has been built and are
//! skipped otherwise; the last one is synthetic and always runs.

mod common;

use butterfly_model::reqmatrix;
use butterfly_model::{Evidence, TestRun};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn ingest(with_tests: bool, with_universe: bool) -> Option<Evidence> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-per-test");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c-per-test/build.sh first");
        return None;
    }
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root.clone());
    if with_tests {
        let mut v: Vec<PathBuf> =
            std::fs::read_dir(root.join("build/tests")).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
        v.sort();
        o.tests = v;
    }
    if with_universe {
        o.requirements_list = Some(root.join("requirements.txt"));
    }
    Some(butterfly_ingest::ingest(&o).expect("ingest"))
}

/// The filled rows: what implements each requirement, what exercises it,
/// and what those tests actually reached.
#[test]
fn a_requirement_knows_its_functions_its_tests_and_their_reach() {
    let Some(ev) = ingest(true, true) else { return };
    let m = reqmatrix::compute(&ev);
    let row = |r: &str| m.rows.iter().find(|x| x.requirement == r).unwrap_or_else(|| panic!("missing {r}"));
    let names = |ids: &[usize]| {
        let mut v: Vec<&str> = ids.iter().map(|&t| ev.tests[t].name.as_str()).collect();
        v.sort_unstable();
        v
    };

    let r1 = row("LLR-LIM-001");
    assert_eq!(r1.functions.iter().map(|&f| ev.functions[f].name.as_str()).collect::<Vec<_>>(), ["clamp_alt"]);
    assert_eq!(names(&r1.tests), ["t_ceiling", "t_clamp"], "both edge tests exercise it");
    assert_eq!((r1.lines_covered, r1.lines_total), (5, 5), "and together they reach all of it");

    // fault_name's `case 1` arm is deliberately untested: the row must show
    // what the named test reached, not everything the merged run touched
    let r3 = row("LLR-LIM-003");
    assert_eq!(names(&r3.tests), ["t_faults"]);
    assert!(r3.lines_covered > 0 && r3.lines_covered < r3.lines_total, "partial by design: {}/{}", r3.lines_covered, r3.lines_total);
}

/// The three orphan classes, each explicit.
#[test]
fn all_three_orphan_classes_are_named() {
    let Some(ev) = ingest(true, true) else { return };
    let m = reqmatrix::compute(&ev);

    // implemented, unverified
    assert_eq!(m.requirements_without_tests, ["LLR-LIM-002"]);
    let r2 = m.rows.iter().find(|x| x.requirement == "LLR-LIM-002").unwrap();
    assert_eq!(r2.lines_covered, 0, "no test reached any of it");
    assert!(r2.lines_total > 0, "but the implementation exists");

    // in the universe, tagged nowhere
    assert_eq!(m.requirements_without_code, ["LLR-LIM-004"]);

    // code no requirement asks for
    let untagged: Vec<&str> = m.functions_without_requirement.iter().map(|&f| ev.functions[f].name.as_str()).collect();
    assert!(untagged.contains(&"main"), "{untagged:?}");
    assert!(untagged.contains(&"bank_angle_ok"), "a function with no requirement tag, on purpose (#48): {untagged:?}");
    assert_eq!(untagged.len(), 10, "main, bank_angle_ok, and the eight test drivers: {untagged:?}");
}

/// Without per-test data the matrix still renders, but says its coverage is
/// the aggregate proxy — and it does not accuse anything of being untested,
/// because it cannot know.
#[test]
fn without_per_test_data_the_matrix_is_a_labelled_proxy() {
    let Some(ev) = ingest(false, true) else { return };
    let m = reqmatrix::compute(&ev);
    assert!(!m.per_test_data);
    assert!(m.requirements_without_tests.is_empty(), "cannot claim 'no test' without test identity");
    let r1 = m.rows.iter().find(|x| x.requirement == "LLR-LIM-001").unwrap();
    assert_eq!((r1.lines_covered, r1.lines_total), (5, 5), "aggregate fallback");
    assert!(r1.tests.is_empty());
    // the no-code orphan needs only the universe, not test identity
    assert_eq!(m.requirements_without_code, ["LLR-LIM-004"]);
}

/// No universe supplied: the no-code list is empty rather than guessed,
/// because tags cannot prove absence.
#[test]
fn absence_is_only_claimed_when_a_universe_says_so() {
    let Some(ev) = ingest(true, false) else { return };
    let m = reqmatrix::compute(&ev);
    assert!(m.requirements_without_code.is_empty());
    assert_eq!(m.rows.len(), 5, "the discovered rows are unchanged");
}

/// A line-level tag claims its lines, not its function.
///
/// `band_of` carries two requirements on two different lines, and two tests
/// reach different parts of it. Scoping a line tag to the whole function
/// would count all five of the function's lines for each requirement and
/// credit every test that entered the function anywhere -- turning an
/// unverified requirement into a verified-looking one, which is the worst
/// thing this table can get wrong (review of #42).
#[test]
fn a_line_level_tag_scopes_to_its_lines_not_its_function() {
    let Some(ev) = ingest(true, true) else { return };
    let m = reqmatrix::compute(&ev);
    let row = |r: &str| m.rows.iter().find(|x| x.requirement == r).unwrap_or_else(|| panic!("missing {r}"));
    let names = |ids: &[usize]| {
        let mut v: Vec<&str> = ids.iter().map(|&t| ev.tests[t].name.as_str()).collect();
        v.sort_unstable();
        v
    };
    let band = ev.functions.iter().find(|f| f.name == "band_of").expect("band_of");
    let band_lines = band.lines_total;
    assert!(band_lines > 2, "the function is bigger than either tag's line: {band_lines}");

    let r5 = row("LLR-LIM-005");
    let r6 = row("LLR-LIM-006");
    // one tagged line each, not the whole function
    assert_eq!(r5.lines_total, 1, "LLR-LIM-005 claims its own line only");
    assert_eq!(r6.lines_total, 1, "LLR-LIM-006 claims its own line only");

    // and the tests are those that reached *that* line. t_band_low enters
    // band_of and returns before LLR-LIM-006's line, so it must not appear
    // against it -- the function-scoped join listed it.
    let t6 = names(&r6.tests);
    assert_eq!(t6, ["t_band_high"], "only the test that reached the line");
    let entered_band: Vec<&str> = names(&band.tests);
    assert!(entered_band.contains(&"t_band_low"), "though t_band_low did enter the function: {entered_band:?}");
}

/// The review's own reproduction, synthetic so it runs without the example
/// build: a two-line function where only line 2 carries the tag, and the one
/// test covers only line 1.
///
/// Before the scope fix this reported that test against `REQ-LINE-2` and 1/2
/// coverage -- an implemented-but-unverified requirement dressed up as a
/// verified one, which is the failure this whole table exists to prevent.
#[test]
fn the_reviews_two_line_reproduction_reports_no_test_and_one_open_line() {
    let f0 = common::file(&["a();", "b();"], &[Some(1), Some(1)]);
    let mut fun = common::function(0, "f", 1, 2);
    fun.line_requirements.insert(2, vec!["REQ-LINE-2".into()]);
    fun.lines_total = 2;
    fun.lines_covered = 2;
    let mut ev = common::evidence(vec![f0], vec![fun]);
    ev.files[0].function_ids = vec![0];

    let mut lines = BTreeMap::new();
    lines.insert(0usize, vec![(1u32, 1u32)]); // the test reaches line 1 only
    ev.tests = vec![TestRun { id: 0, name: "t".into(), path: "t.json".into(), functions: vec![0], lines }];
    ev.functions[0].tests = vec![0];

    ev.validate().expect("a well-formed document");
    let m = reqmatrix::compute(&ev);
    let row = m.rows.iter().find(|r| r.requirement == "REQ-LINE-2").expect("the row");

    assert_eq!(row.lines_total, 1, "the tag claims its line, not the function's two");
    assert!(row.tests.is_empty(), "entering the function elsewhere verifies nothing here");
    // covered counts only what a *named* test reached: line 2 ran, but no
    // test claims it, so this requirement is unverified -- 0 of 1.
    assert_eq!(row.lines_covered, 0);
    assert!(m.requirements_without_tests.iter().any(|r| r == "REQ-LINE-2"), "and it is listed as implemented-but-unverified");
}
