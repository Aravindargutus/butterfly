// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the "What to test next" worklist (area B).
//!
//! Every expectation here is taken from README.md, "What to test next"
//! (~L563-574), not from `worklist.rs`. The evidence is hand-built, so the
//! tests need no fixture and say exactly which obligation they are about.

use butterfly_ingest::worklist;
use butterfly_model::*;

fn res(keys: &[String]) -> Resolutions {
    Resolutions {
        schema: RESOLUTIONS_SCHEMA.into(),
        entries: keys.iter().map(|k| Resolution { key: k.clone(), category: "a".into(), justification: "t".into(), author: "t".into(), date: "2026-10-01".into() }).collect(),
    }
}

fn none() -> Resolutions {
    res(&[])
}

fn br(line: u32, col: u32, t: u64, f: u64) -> Branch {
    Branch { line, col, line_end: line, col_end: col + 1, true_count: t, false_count: f, direction_known: true, decision_ordinal: None }
}

fn cond(index: usize, line: u32, col: u32, t: u64, f: u64, covered: bool) -> Condition {
    Condition { index, line, col, true_count: t, false_count: f, independence_covered: covered, pair: None }
}

const TEXT: &[&str] = &[
    "int never_called(int v) {", // 1
    "  if (v) return 1;",        // 2
    "  return 0;",               // 3
    "}",                         // 4
    "",                          // 5
    "int busy(int a, int b) {",  // 6
    "  if (alpha && beta) {",    // 7
    "    a++;",                  // 8
    "  } if (a > 3) {",          // 9
    "  } if (b > 3) {",          // 10
    "  }",                       // 11
    "}",                         // 12
];

/// Two functions: `never_called` (count 0) and `busy` (ran 3 times), with
/// one MC/DC decision of two unproven conditions, one half-taken branch and
/// one branch neither of whose outcomes ran, plus a gap in its object code.
fn evidence() -> Evidence {
    let counts: Vec<Option<u64>> = vec![Some(0), Some(0), Some(0), None, None, Some(3), Some(3), Some(2), Some(3), Some(3), None, None];
    let file = SourceFile {
        id: 0,
        path: "/p/w.c".into(),
        display: "w.c".into(),
        text: TEXT.iter().map(|s| s.to_string()).collect(),
        lines: counts.iter().map(|c| LineCov { count: *c }).collect(),
        function_ids: vec![0, 1],
        ..Default::default()
    };
    let never = Function { id: 0, name: "never_called".into(), file_id: 0, line_start: 1, line_end: 4, count: 0, branches: vec![br(2, 7, 0, 0)], ..Default::default() };
    let decision = Decision {
        line: 7,
        col: 7,
        line_end: 7,
        col_end: 20,
        true_count: 2,
        false_count: 1,
        // `alpha` was seen both ways but never independently; `beta` was
        // never false
        conditions: vec![cond(0, 7, 7, 2, 1, false), cond(1, 7, 16, 2, 0, false)],
        vectors: vec![],
        mcdc_covered: false,
        decision_ordinal: None,
    };
    let gap = Gap {
        id: 0,
        kind: GapKind::ExtraObjectBranch,
        addr_start: 0x10,
        addr_end: 0x14,
        line: Some(9),
        description: "extra".into(),
        key: "busy@extra-object-branch@16".into(),
        class: None,
    };
    let busy = Function {
        id: 1,
        name: "busy".into(),
        file_id: 0,
        line_start: 6,
        line_end: 12,
        count: 3,
        // the decision's own condition branches, a half-taken branch and a
        // branch whose line was reached but neither outcome taken
        branches: vec![br(7, 7, 2, 1), br(7, 16, 2, 0), br(9, 9, 3, 0), br(10, 9, 0, 0)],
        decisions: vec![decision],
        object: Some(ObjectFn { symbol: "busy".into(), gaps: vec![gap], ..Default::default() }),
        ..Default::default()
    };
    Evidence { schema: "t".into(), files: vec![file], functions: vec![never, busy], ..Default::default() }
}

/// README.md:565-566 "every open **test** obligation, ranked by how much one
/// new test would close": the list is in non-increasing order of `closes`.
#[test]
fn the_list_is_ranked_by_what_one_test_closes() {
    let w = worklist::compute(&evidence(), &none());
    assert!(!w.is_empty());
    for pair in w.windows(2) {
        assert!(pair[0].closes >= pair[1].closes, "ranked best first: {} then {}", pair[0].closes, pair[1].closes);
    }
}

/// README.md:566-567 "A function nothing calls outranks a single untaken
/// outcome, because one test that reaches it may close everything behind it".
#[test]
fn a_function_nothing_calls_outranks_a_single_untaken_outcome() {
    let w = worklist::compute(&evidence(), &none());
    let f = w.iter().position(|i| i.kind == "function" && i.function == "never_called").expect("the uncalled function is listed");
    let o = w.iter().position(|i| i.kind == "outcome").expect("there are untaken outcomes");
    assert!(f < o, "the uncalled function ranks above every single outcome: {w:#?}");
    assert_eq!(f, 0, "and leads this list");
    // its inner obligations are reached through the call, not listed again
    assert!(w.iter().filter(|i| i.function == "never_called").all(|i| i.kind == "function"), "{w:#?}");
}

/// README.md:567 "conditions of one decision group together": one entry for
/// the decision, not one per condition, and its conditions' branch outcomes
/// are not listed a second time as untaken outcomes.
#[test]
fn conditions_of_one_decision_group_into_one_entry() {
    let w = worklist::compute(&evidence(), &none());
    let decs: Vec<_> = w.iter().filter(|i| i.kind == "decision").collect();
    assert_eq!(decs.len(), 1, "one entry for the one decision: {w:#?}");
    assert_eq!(decs[0].line, 7);
    assert_eq!(decs[0].closes, 2, "both unproven conditions sit behind it");
    // the per-condition steps name each condition from the source
    assert!(decs[0].next.contains("alpha") && decs[0].next.contains("beta"), "{}", decs[0].next);
    assert!(!w.iter().any(|i| i.kind == "outcome" && i.line == 7), "the decision's conditions are not repeated as outcomes: {w:#?}");
}

/// README.md:567-568 "an untaken outcome whose sibling ran says 'one input
/// away'", and (by the same sentence, read the other way) one whose sibling
/// did not run does not say so.
#[test]
fn an_untaken_outcome_whose_sibling_ran_is_one_input_away() {
    let w = worklist::compute(&evidence(), &none());
    let l9: Vec<_> = w.iter().filter(|i| i.kind == "outcome" && i.line == 9).collect();
    assert_eq!(l9.len(), 1, "only the false outcome at L9 is open: {w:#?}");
    assert!(l9[0].next.contains("one input away"), "{}", l9[0].next);

    let l10: Vec<_> = w.iter().filter(|i| i.kind == "outcome" && i.line == 10).collect();
    assert_eq!(l10.len(), 2, "both outcomes at L10 are open: {w:#?}");
    assert!(l10.iter().all(|i| !i.next.contains("one input away")), "neither sibling ran: {l10:#?}");
}

/// README.md:569-572 "Unresolved gaps are deliberately absent -- they are
/// reviewer work ... not something a test input closes".
#[test]
fn unresolved_gaps_are_not_test_obligations() {
    let w = worklist::compute(&evidence(), &none());
    assert!(w.iter().all(|i| matches!(i.kind.as_str(), "function" | "decision" | "outcome")), "{w:#?}");
    assert!(!w.iter().any(|i| i.what.contains("extra") || i.what.contains("gap")), "no entry is about the gap: {w:#?}");
}

/// README.md:568-569 "Justified items drop out, so the list can reach zero."
#[test]
fn justifying_every_item_empties_the_list() {
    let ev = evidence();
    let keys = vec!["dead:never_called".to_string(), "cond:1:7:7:0".into(), "cond:1:7:7:1".into(), "branch:1:9:9:F".into(), "branch:1:10:9:T".into(), "branch:1:10:9:F".into()];
    let w = worklist::compute(&ev, &res(&keys));
    assert!(w.is_empty(), "every open item was justified: {w:#?}");
}

/// README.md:568 -- a justified *part* drops out and the rest stays: one
/// condition of the decision justified leaves an entry closing one.
#[test]
fn a_partly_justified_decision_keeps_only_its_open_conditions() {
    let w = worklist::compute(&evidence(), &res(&["cond:1:7:7:0".into()]));
    let d = w.iter().find(|i| i.kind == "decision").expect("one condition is still open");
    assert_eq!(d.closes, 1);
    assert!(d.next.contains("beta") && !d.next.contains("alpha"), "{}", d.next);
}

/// README.md:959-966 "butterfly therefore reports both outcomes and names
/// neither for gcov ... the viewer says *direction not stated* rather than
/// inventing one". An untaken gcov outcome must not be called the true or the
/// false one, in what is open or in the next step.
#[test]
fn a_gcov_outcome_is_never_named_true_or_false() {
    let mut ev = evidence();
    for b in &mut ev.functions[1].branches {
        if b.line == 9 {
            b.direction_known = false;
            b.col = 0;
            b.decision_ordinal = Some(0);
        }
    }
    let w = worklist::compute(&ev, &none());
    let o: Vec<_> = w.iter().filter(|i| i.kind == "outcome" && i.line == 9).collect();
    assert_eq!(o.len(), 1, "{w:#?}");
    for s in [&o[0].what, &o[0].next] {
        assert!(!s.contains("true outcome") && !s.contains("false outcome"), "a gcov outcome named with a polarity: {s}");
    }
    assert!(o[0].next.contains("one input away"), "its sibling ran: {}", o[0].next);
    // and justifying it takes the neutral key, not T/F
    let w = worklist::compute(&ev, &res(&["branch:1:9:#0:B".into()]));
    assert!(!w.iter().any(|i| i.kind == "outcome" && i.line == 9), "{w:#?}");
}

/// README.md:565 "every open test obligation": an MC/DC condition that no
/// vector ever reached, one never true and one seen both ways are all open,
/// and each step names its condition from the source rather than by number.
#[test]
fn every_unproven_condition_is_named_whatever_its_counts() {
    let mut ev = evidence();
    let d = &mut ev.functions[1].decisions[0];
    d.conditions[0] = cond(0, 7, 7, 0, 0, false); // reached by no vector
    d.conditions[1] = cond(1, 7, 16, 0, 3, false); // never true
    let w = worklist::compute(&ev, &none());
    let d = w.iter().find(|i| i.kind == "decision").unwrap();
    assert!(d.next.contains("`alpha`") && d.next.contains("`beta`"), "{}", d.next);

    // both seen both ways and still unproven
    let mut ev = evidence();
    ev.functions[1].decisions[0].conditions[1] = cond(1, 7, 16, 2, 2, false);
    let w = worklist::compute(&ev, &none());
    let d = w.iter().find(|i| i.kind == "decision").unwrap();
    assert!(d.next.contains("`alpha`") && d.next.contains("`beta`"), "{}", d.next);
}

/// A decision written over several lines still names each condition: the
/// first condition is on a line of its own, so its text runs to that line's
/// end, and a parenthesis it opens does not become part of the name.
#[test]
fn a_decision_over_several_lines_names_its_conditions() {
    let mut ev = evidence();
    ev.files[0].text[6] = "  if ((alpha &&".into();
    ev.files[0].text[7] = "       beta)) {".into();
    let d = &mut ev.functions[1].decisions[0];
    d.line_end = 8;
    d.col_end = 13;
    d.conditions = vec![cond(0, 7, 7, 2, 1, false), cond(1, 8, 8, 2, 0, false)];
    let w = worklist::compute(&ev, &none());
    let d = w.iter().find(|i| i.kind == "decision").unwrap();
    assert!(d.next.contains("`alpha`"), "{}", d.next);
    assert!(d.next.contains("`beta`"), "{}", d.next);
}
