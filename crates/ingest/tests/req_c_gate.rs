// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the gate behind `butterfly check` (area C).
//!
//! The counts are set by hand on a real evidence file so the boundaries the
//! README states -- "a floor of 100 is compared as a count", "a floor over
//! records the evidence does not carry is a breach", "nothing may be worse"
//! -- can be hit exactly. Expected results come from README.md "Gating a
//! build (`butterfly check`)", lines 491-536.
//!
//! Runs only when examples/c has been built; skipped, not failed, otherwise.

use butterfly_ingest::gate::{evaluate, regressions, Thresholds};
use butterfly_model::{Evidence, Resolutions};
use std::path::PathBuf;

fn evidence() -> Option<Evidence> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c/build.sh first");
        return None;
    }
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root);
    Some(butterfly_ingest::ingest(&o).expect("ingest"))
}

fn none() -> Resolutions {
    Resolutions::default()
}

fn kinds(b: &[butterfly_ingest::gate::Breach]) -> Vec<&str> {
    b.iter().map(|x| x.kind.as_str()).collect()
}

// README.md:514-515 "A floor of 100 is compared as a count, so rounding can neither grant nor deny
// the last item." 1999/2000 is 99.95%, which prints as 100.0% at one decimal.
#[test]
fn a_floor_of_100_is_a_count_so_one_item_short_breaches() {
    let Some(mut ev) = evidence() else { return };
    ev.summary.lines_total = 2000;
    ev.summary.lines_covered = 1999;
    let th = Thresholds { min_lines: Some(100.0), ..Default::default() };
    assert_eq!(kinds(&evaluate(&ev, &none(), &th)), vec!["lines"], "one line short of everything is not everything");
    ev.summary.lines_covered = 2000;
    assert!(evaluate(&ev, &none(), &th).is_empty(), "everything meets 100");
}

// README.md:514 the same rule for outcomes and MC/DC; and a fractional floor is a percentage:
// exactly at it passes, a hair under fails.
#[test]
fn fractional_floors_are_inclusive_percentages() {
    let Some(mut ev) = evidence() else { return };
    ev.summary.branches_total = 200;
    ev.summary.branches_covered = 199; // 99.5%
    let at = Thresholds { min_outcomes: Some(99.5), ..Default::default() };
    assert!(evaluate(&ev, &none(), &at).is_empty(), "99.5% meets a floor of 99.5");
    let over = Thresholds { min_outcomes: Some(99.51), ..Default::default() };
    assert_eq!(kinds(&evaluate(&ev, &none(), &over)), vec!["outcomes"]);
    let all = Thresholds { min_outcomes: Some(100.0), ..Default::default() };
    assert_eq!(kinds(&evaluate(&ev, &none(), &all)), vec!["outcomes"], "199/200 is not 100");
}

// README.md:515-518 "a floor over records the evidence does not carry ... is a breach, not a
// vacuous pass" -- for every floor, and even a floor of 0.
#[test]
fn a_floor_over_records_that_do_not_exist_is_a_breach_even_at_zero() {
    let Some(mut ev) = evidence() else { return };
    ev.summary.lines_total = 0;
    ev.summary.lines_covered = 0;
    ev.summary.branches_total = 0;
    ev.summary.branches_covered = 0;
    for f in &mut ev.functions {
        f.decisions.clear();
    }
    ev.summary.decisions_unplaced = 0;
    for th in [
        Thresholds { min_lines: Some(0.0), ..Default::default() },
        Thresholds { min_outcomes: Some(0.0), ..Default::default() },
        Thresholds { min_mcdc: Some(0.0), ..Default::default() },
        Thresholds { min_mcdc: Some(100.0), ..Default::default() },
    ] {
        let b = evaluate(&ev, &none(), &th);
        assert_eq!(b.len(), 1, "{th:?} over nothing must breach: {b:?}");
        assert_eq!(b[0].kind, "no-records", "{b:?}");
    }
}

// check --help: "All optional: only what the caller asked for is checked" -- a floor that was not
// given is not judged, so an evidence file with no MC/DC is not refused by --min-lines alone.
#[test]
fn only_the_floors_given_are_judged() {
    let Some(mut ev) = evidence() else { return };
    for f in &mut ev.functions {
        f.decisions.clear();
    }
    assert!(evaluate(&ev, &none(), &Thresholds { min_lines: Some(0.0), ..Default::default() }).is_empty());
    assert!(evaluate(&ev, &none(), &Thresholds::default()).is_empty(), "no thresholds, nothing to breach");
}

// README.md:521-523 "--no-regression ... nothing may be worse: line and outcome percentages, MC/DC,
// the unresolved-gap count, and any individual function whose covered lines fell while its shape
// was unchanged".
#[test]
fn the_same_evidence_is_not_worse_than_itself() {
    let Some(ev) = evidence() else { return };
    assert!(regressions(&ev, &none(), &ev.clone(), &none()).is_empty());
}

#[test]
fn one_line_less_is_a_line_regression() {
    let Some(ev) = evidence() else { return };
    let mut prior = ev.clone();
    prior.summary.lines_covered += 1;
    assert!(kinds(&regressions(&ev, &none(), &prior, &none())).contains(&"regression-lines"));
}

#[test]
fn one_outcome_less_is_an_outcome_regression() {
    let Some(ev) = evidence() else { return };
    let mut prior = ev.clone();
    prior.summary.branches_covered += 1;
    assert!(kinds(&regressions(&ev, &none(), &prior, &none())).contains(&"regression-outcomes"));
}

// README.md:521 percentages, not counts: more lines in total and more covered can still be worse.
#[test]
fn a_lower_percentage_over_a_larger_build_is_still_a_regression() {
    let Some(ev) = evidence() else { return };
    let mut now = ev.clone();
    now.summary.lines_total = ev.summary.lines_total * 2;
    now.summary.lines_covered = ev.summary.lines_covered * 2 - 1;
    assert!(kinds(&regressions(&now, &none(), &ev, &none())).contains(&"regression-lines"));
    // and a higher percentage over a smaller build is not
    let mut better = ev.clone();
    better.summary.lines_covered = better.summary.lines_total;
    assert!(!kinds(&regressions(&better, &none(), &ev, &none())).contains(&"regression-lines"));
}

// README.md:515-518 (instrumentation silently absent must not pass) applied to the comparison:
// a build that measures nothing where the prior measured something is worse, not "unchanged".
#[test]
fn losing_every_measurement_is_a_regression() {
    let Some(ev) = evidence() else { return };
    let mut now = ev.clone();
    now.summary.lines_total = 0;
    now.summary.lines_covered = 0;
    now.summary.branches_total = 0;
    now.summary.branches_covered = 0;
    let k = regressions(&now, &none(), &ev, &none());
    assert!(kinds(&k).contains(&"regression-lines") && kinds(&k).contains(&"regression-outcomes"), "{k:?}");
}

// README.md:522 "any individual function whose covered lines fell while its shape was unchanged"
#[test]
fn a_function_that_fell_is_named_but_only_while_its_shape_is_unchanged() {
    let Some(ev) = evidence() else { return };
    let i = ev.functions.iter().position(|f| f.lines_covered < f.lines_total).expect("a partly covered function in examples/c");
    let name = ev.functions[i].name.clone();
    let mut prior = ev.clone();
    prior.functions[i].lines_covered += 1;
    let b = regressions(&ev, &none(), &prior, &none());
    let named: Vec<_> = b.iter().filter(|x| x.kind == "regression-function").collect();
    assert_eq!(named.len(), 1, "{b:?}");
    assert_eq!(named[0].function.as_deref(), Some(name.as_str()));
    assert!(named[0].message.contains(&name), "{b:?}");
    // shape changed: a different number of lines is a different function, not a regression of it
    let mut moved = ev.clone();
    moved.functions[i].lines_total += 5;
    moved.functions[i].lines_covered += 5;
    assert!(regressions(&ev, &none(), &moved, &none()).iter().all(|x| x.kind != "regression-function"), "churn is not regression");
}

// README.md:521 the unresolved-gap count may not rise, and README.md:525-527 justified gaps count
// as resolved on both sides.
#[test]
fn unresolved_gaps_rising_is_a_regression_and_reviews_count_on_each_side() {
    let Some(ev) = evidence() else { return };
    let key = ev.functions.iter().flat_map(|f| f.objects()).flat_map(|o| o.gaps.iter()).map(|g| g.key.clone()).next().expect("a gap");
    let reviewed = Resolutions {
        schema: butterfly_model::RESOLUTIONS_SCHEMA.into(),
        entries: vec![butterfly_model::Resolution { key, category: "a".into(), justification: "reviewed".into(), author: "r".into(), date: "2026-01-15".into() }],
    };
    // reviewed back then, not now: one more unresolved gap now
    assert!(kinds(&regressions(&ev, &none(), &ev.clone(), &reviewed)).contains(&"regression-gaps"));
    // reviewed now, not back then: fewer, which is not worse
    assert!(!kinds(&regressions(&ev, &reviewed, &ev.clone(), &none())).contains(&"regression-gaps"));
    // and the ceiling counts the review as resolved
    let total: usize = ev.functions.iter().flat_map(|f| f.objects()).map(|o| o.gaps.len()).sum();
    let th = Thresholds { max_unresolved_gaps: Some(total - 1), ..Default::default() };
    assert!(evaluate(&ev, &reviewed, &th).is_empty(), "one of {total} justified leaves {} unresolved", total - 1);
    assert_eq!(kinds(&evaluate(&ev, &none(), &th)), vec!["unresolved-gaps"]);
}
