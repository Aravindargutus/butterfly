// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! An MC/DC decision that could not be placed is an unproven obligation, and
//! an MC/DC gate must not pass while any exists (#284).
//!
//! When LLVM 18 cannot tell two macro-use records apart, ingest drops the
//! ambiguous decision from `Function.decisions` and records the count in
//! `summary.decisions_unplaced`. The danger the gate has to rule out: a fully
//! covered *remainder* would otherwise satisfy `--min-mcdc 100` over records
//! that were never in the denominator.

mod common;

use butterfly_ingest::gate::{self, Thresholds};
use butterfly_model::*;

fn res() -> Resolutions {
    Resolutions { schema: RESOLUTIONS_SCHEMA.into(), entries: vec![] }
}

fn only_mcdc(min: f64) -> Thresholds {
    Thresholds { max_unresolved_gaps: None, min_lines: None, min_outcomes: None, min_mcdc: Some(min) }
}

/// One function with a single, fully covered decision: on its own it is 100%.
fn ev_with_one_covered_decision() -> Evidence {
    let f = common::file(&["int f(int a){", "  return a?1:0;", "}"], &[Some(1), Some(1), Some(1)]);
    let mut fun = common::function(0, "f", 1, 3);
    fun.decisions = vec![Decision {
        line: 2,
        col: 10,
        line_end: 2,
        col_end: 14,
        true_count: 1,
        false_count: 1,
        conditions: vec![Condition { index: 0, line: 2, col: 10, true_count: 1, false_count: 1, independence_covered: true, pair: Some((1, 2)) }],
        vectors: vec![],
        mcdc_covered: true,
        decision_ordinal: None,
    }];
    common::evidence(vec![f], vec![fun])
}

#[test]
fn an_unplaced_mcdc_decision_blocks_a_full_mcdc_gate() {
    let mut ev = ev_with_one_covered_decision();

    // The control: with nothing unplaced, the covered decision meets --min-mcdc 100.
    ev.summary.decisions_unplaced = 0;
    let clean = gate::evaluate(&ev, &res(), &only_mcdc(100.0));
    assert!(!clean.iter().any(|b| b.kind.starts_with("mcdc")), "a fully covered decision meets the floor: {clean:?}");

    // The finding: one decision could not be placed. The retained decision is
    // still 100%, but the gate must now fail -- the unproven obligation is not
    // in the denominator, so a pass would be over records never accounted for.
    ev.summary.decisions_unplaced = 1;
    let breaches = gate::evaluate(&ev, &res(), &only_mcdc(100.0));
    let inc = breaches.iter().find(|b| b.kind == "mcdc-incomplete").unwrap_or_else(|| panic!("an unplaced obligation must fail the MC/DC gate: {breaches:?}"));
    assert!(inc.message.contains("could not be placed"), "the breach says why: {}", inc.message);
}

/// It holds at any floor, not only 100%: an unaccounted obligation means the
/// floor cannot be attested at all.
#[test]
fn an_unplaced_mcdc_decision_blocks_even_a_low_floor() {
    let mut ev = ev_with_one_covered_decision();
    ev.summary.decisions_unplaced = 1;
    let breaches = gate::evaluate(&ev, &res(), &only_mcdc(10.0));
    assert!(breaches.iter().any(|b| b.kind == "mcdc-incomplete"), "even a 10% floor cannot be attested with an unplaced obligation: {breaches:?}");
}

/// And it is distinct from "no records at all": unplaced records are not the
/// same as a build without -fcoverage-mcdc.
#[test]
fn unplaced_records_are_not_reported_as_no_records() {
    let mut ev = common::evidence(vec![common::file(&["x"], &[Some(1)])], vec![common::function(0, "f", 1, 1)]);
    ev.summary.decisions_unplaced = 2;
    let breaches = gate::evaluate(&ev, &res(), &only_mcdc(100.0));
    assert!(breaches.iter().any(|b| b.kind == "mcdc-incomplete"), "{breaches:?}");
    assert!(!breaches.iter().any(|b| b.kind == "no-records"), "unplaced records exist, so this is not `no-records`: {breaches:?}");
}
