// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The what-to-test-next worklist (#44), against the C example.
//!
//! Runs only when examples/c has been built; skipped, not failed, otherwise.

use butterfly_ingest::worklist;
use butterfly_model::{Evidence, Resolution, Resolutions, RESOLUTIONS_SCHEMA};
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

fn no_res() -> Resolutions {
    Resolutions { schema: RESOLUTIONS_SCHEMA.into(), entries: Vec::new() }
}

fn resolve(keys: &[String]) -> Resolutions {
    Resolutions {
        schema: RESOLUTIONS_SCHEMA.into(),
        entries: keys.iter().map(|k| Resolution { key: k.clone(), category: "a".into(), justification: "t".into(), author: "t".into(), date: "2026-09-02".into() }).collect(),
    }
}

/// Leverage first: a function nothing calls outranks any single outcome,
/// because one test that reaches it may close everything behind it.
#[test]
fn uncovered_functions_lead_the_list() {
    let Some(ev) = evidence() else { return };
    let w = worklist::compute(&ev, &no_res());
    assert!(!w.is_empty());
    assert_eq!(w[0].kind, "function");
    assert_eq!(w[0].function, "legacy_scale", "the biggest uncalled function leads");
    assert!(w[0].closes >= w.last().unwrap().closes, "sorted by what one test closes");
    // and every entry carries a next step, not only a deficit
    assert!(w.iter().all(|i| !i.next.is_empty()));
}

/// The per-condition step names the condition from the source, in the same
/// direction the MC/DC panel gives: what to reach, and with what value.
#[test]
fn the_mcdc_step_names_the_condition_and_the_value() {
    let Some(ev) = evidence() else { return };
    let w = worklist::compute(&ev, &no_res());
    let d = w.iter().find(|i| i.kind == "decision").expect("the fixture has an unproven decision");
    assert_eq!(d.function, "mode_permits_engage");
    assert!(d.next.contains("`speed_ok`") && d.next.contains("false"), "{}", d.next);
}

/// An untaken outcome whose sibling ran is one input away, and says so.
#[test]
fn a_half_taken_branch_says_one_input_away() {
    let Some(ev) = evidence() else { return };
    let w = worklist::compute(&ev, &no_res());
    let o = w.iter().find(|i| i.kind == "outcome").expect("the fixture has untaken outcomes");
    assert!(o.next.contains("one input away"), "{}", o.next);
}

/// Nothing inside a never-executed function is listed on its own: the call
/// subsumes it, or the list would count the same test many times.
#[test]
fn items_inside_an_uncalled_function_are_subsumed_by_the_call() {
    let Some(ev) = evidence() else { return };
    let w = worklist::compute(&ev, &no_res());
    let uncalled: Vec<&str> = ev.functions.iter().filter(|f| f.count == 0).map(|f| f.name.as_str()).collect();
    for item in &w {
        if uncalled.contains(&item.function.as_str()) {
            assert_eq!(item.kind, "function", "only the call itself is listed for `{}`", item.function);
        }
    }
}

/// Justified items drop out, so the list can reach zero -- the difference
/// between a worklist and a complaint.
#[test]
fn justified_items_leave_the_list() {
    let Some(ev) = evidence() else { return };
    let before = worklist::compute(&ev, &no_res());

    let legacy = ev.functions.iter().find(|f| f.name == "legacy_scale").unwrap();
    let dec_fn = ev.functions.iter().find(|f| f.name == "mode_permits_engage").unwrap();
    let d = &dec_fn.decisions[0];
    let open_cond = d.conditions.iter().position(|c| !c.independence_covered).unwrap();
    let res = resolve(&[format!("fn:{}", legacy.id), format!("cond:{}:{}:{}:{}", dec_fn.id, d.line, d.col, open_cond)]);

    let after = worklist::compute(&ev, &res);
    assert!(after.len() < before.len());
    assert!(!after.iter().any(|i| i.function == "legacy_scale"), "the justified function is gone");
    assert!(!after.iter().any(|i| i.kind == "decision" && i.function == "mode_permits_engage"), "and the justified condition closed its decision entry");
}

/// An uncalled function's score counts only the *open* work behind it: a
/// line someone already justified is closed, whichever function holds it.
#[test]
fn a_partially_justified_uncalled_function_scores_only_open_items() {
    let Some(ev) = evidence() else { return };
    let legacy = ev.functions.iter().find(|f| f.name == "legacy_scale").unwrap();
    let before = worklist::compute(&ev, &no_res());
    let entry = |w: &[worklist::Item]| w.iter().find(|i| i.function == "legacy_scale").map(|i| i.closes);

    // justify one of its instrumented lines
    let file = &ev.files[legacy.file_id];
    let ln = (legacy.line_start..=legacy.line_end).find(|&l| file.lines.get(l as usize - 1).map(|c| c.count.is_some()).unwrap_or(false)).expect("an instrumented line");
    let after = worklist::compute(&ev, &resolve(&[format!("line:{}:{ln}", legacy.file_id)]));

    let (b, a) = (entry(&before).unwrap(), entry(&after).unwrap());
    assert_eq!(a, b - 1, "one justified line, one fewer obligation ({b} -> {a})");
    assert!(after.iter().find(|i| i.function == "legacy_scale").unwrap().what.contains(&format!("{} line(s)", b - 2)), "and the message agrees");
}
