// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the requirements x tests matrix and the
//! claims comparison, at the model level (area C).
//!
//! README.md "Requirements coverage" (537-564) and "Checking the claim
//! against the measurement" (840-908) state the rules: a tag's scope is
//! respected -- a line tag claims its line, a tag above the function claims
//! the function -- and a requirement's coverage comes from the tests that
//! reached what it claims. These cases combine the two scopes, which the
//! example sources do not.
//!
//! Runs only when examples/c-per-test has been built.

use butterfly_model::{reqmatrix, Evidence};
use std::path::PathBuf;

fn evidence() -> Option<Evidence> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-per-test");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c-per-test/build.sh first");
        return None;
    }
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root.clone());
    let mut tests: Vec<PathBuf> = std::fs::read_dir(root.join("build/tests")).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    tests.sort();
    o.tests = tests;
    o.requirements_list = Some(root.join("requirements.txt"));
    Some(butterfly_ingest::ingest(&o).expect("ingest"))
}

fn func(ev: &Evidence, name: &str) -> usize {
    ev.functions.iter().position(|f| f.name == name).unwrap_or_else(|| panic!("{name} in c-per-test"))
}

fn instrumented_lines(ev: &Evidence, f: usize) -> usize {
    let func = &ev.functions[f];
    let file = &ev.files[func.file_id];
    (func.line_start..=func.line_end).filter(|&l| file.lines.get(l as usize - 1).is_some_and(|x| x.count.is_some())).count()
}

/// Tests that executed any line of function `f`.
fn tests_entering(ev: &Evidence, f: usize) -> Vec<usize> {
    let func = &ev.functions[f];
    let mut v: Vec<usize> =
        ev.tests.iter().filter(|t| t.lines.get(&func.file_id).is_some_and(|rs| rs.iter().any(|&(a, b)| a <= func.line_end && b >= func.line_start))).map(|t| t.id).collect();
    v.sort();
    v
}

// README.md:553-555 "A tag's scope is respected: @satisfies on one line claims that line, not the whole
// function ... A tag above the function claims the function." With both for the same requirement, the
// function-level tag claims the whole function, so the line tag cannot narrow it.
#[test]
fn a_function_level_tag_claims_the_whole_function_even_with_a_line_tag_for_the_same_requirement() {
    let Some(mut ev) = evidence() else { return };
    let band = func(&ev, "band_of");
    assert!(ev.functions[band].line_requirements.values().flatten().any(|r| r == "LLR-LIM-005"), "band_of carries a line tag for LLR-LIM-005");
    ev.functions[band].requirements.push("LLR-LIM-005".into());
    let m = reqmatrix::compute(&ev);
    let row = m.rows.iter().find(|r| r.requirement == "LLR-LIM-005").unwrap();
    assert_eq!(row.functions, vec![band]);
    assert_eq!(row.lines_total, instrumented_lines(&ev, band), "the whole function's instrumented lines");
    let mut t = row.tests.clone();
    t.sort();
    assert_eq!(t, tests_entering(&ev, band), "every test that reached any of the function");
}

// README.md:545-548 coverage of "the implementation" per requirement: a requirement tagged twice on the
// same function is still that function once -- no line counted twice.
#[test]
fn a_repeated_function_level_tag_counts_the_function_once() {
    let Some(mut ev) = evidence() else { return };
    let clamp = func(&ev, "clamp_alt");
    let before = reqmatrix::compute(&ev).rows.into_iter().find(|r| r.requirement == "LLR-LIM-001").unwrap();
    ev.functions[clamp].requirements.push("LLR-LIM-001".into());
    let after = reqmatrix::compute(&ev).rows.into_iter().find(|r| r.requirement == "LLR-LIM-001").unwrap();
    assert_eq!((after.lines_total, after.lines_covered), (before.lines_total, before.lines_covered));
    assert_eq!(after.lines_total, instrumented_lines(&ev, clamp));
}

// README.md:545-548 "how much of the implementation those named tests reached -- never aggregate
// coverage"; examples/c-per-test/README.md: LLR-LIM-001 is fully covered by named tests, LLR-LIM-002
// implemented but tested by nothing, LLR-LIM-004 has no code; README.md:560-562 the three orphans.
#[test]
fn the_matrix_names_its_three_kinds_of_orphan() {
    let Some(ev) = evidence() else { return };
    let m = reqmatrix::compute(&ev);
    assert!(m.per_test_data);
    let r1 = m.rows.iter().find(|r| r.requirement == "LLR-LIM-001").unwrap();
    assert!(r1.lines_total > 0 && r1.lines_covered == r1.lines_total, "{r1:?}");
    assert!(m.requirements_without_tests.contains(&"LLR-LIM-002".to_string()), "{:?}", m.requirements_without_tests);
    assert!(!m.requirements_without_tests.contains(&"LLR-LIM-001".to_string()));
    assert_eq!(m.requirements_without_code, vec!["LLR-LIM-004".to_string()]);
    let untagged: Vec<&str> = m.functions_without_requirement.iter().map(|&f| ev.functions[f].name.as_str()).collect();
    assert!(untagged.contains(&"bank_angle_ok"), "untagged on purpose: {untagged:?}");
    assert!(!untagged.contains(&"clamp_alt"));
}

// README.md:550-551 "Without per-test data the matrix still renders, labelled as the aggregate proxy it
// then is" -- and the no-test orphan list says nothing then, since no test identity exists.
#[test]
fn without_per_test_data_the_matrix_is_the_aggregate_proxy() {
    let Some(mut ev) = evidence() else { return };
    ev.tests.clear();
    for f in &mut ev.functions {
        f.tests.clear();
    }
    let m = reqmatrix::compute(&ev);
    assert!(!m.per_test_data, "labelled as the proxy");
    assert!(m.requirements_without_tests.is_empty(), "absence of per-test data proves nothing about tests");
    let r1 = m.rows.iter().find(|r| r.requirement == "LLR-LIM-002").unwrap();
    assert_eq!(r1.lines_covered, 0, "rate_ok never ran in the aggregate either");
    assert!(r1.tests.is_empty());
}
