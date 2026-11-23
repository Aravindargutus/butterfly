// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Change impact, phase 3: the re-verification ruleset. Pure -- no git --
//! so every case is a hand-built evidence with a hand-built change history.
//!
//! These encode the *intended* behaviour. A failing test here is a finding
//! about the implementation, not about the test.

mod common;

use butterfly_ingest::impact::{self, Level};
use butterfly_model::*;
use std::collections::BTreeMap;

// ---------------------------------------------------------------- helpers

/// `(subject, tickets, functions touched)` per commit; `bounded` = a --since
/// was given, so "changed" means "changed since the certified baseline".
fn changes(commits: &[(&str, &[&str], &[usize])], bounded: bool) -> Changes {
    let mut tickets: Vec<Ticket> = Vec::new();
    let mut out: Vec<Commit> = Vec::new();
    for (i, (subj, tks, fns)) in commits.iter().enumerate() {
        let mut tix = Vec::new();
        for t in tks.iter() {
            let ti = match tickets.iter().position(|x| x.id == *t) {
                Some(p) => p,
                None => {
                    tickets.push(Ticket { id: t.to_string(), ..Default::default() });
                    tickets.len() - 1
                }
            };
            tickets[ti].commits.push(i);
            for f in fns.iter() {
                if !tickets[ti].functions.contains(f) {
                    tickets[ti].functions.push(*f);
                }
            }
            tix.push(ti);
        }
        out.push(Commit {
            sha: format!("{i:040x}"),
            short: format!("{i:012x}"),
            author: "t".into(),
            date: "2026-01-01T00:00:00+00:00".into(),
            date_unix: 1000 + i as u64,
            subject: subj.to_string(),
            tickets: tix,
            functions: fns.to_vec(),
            files: vec![],
        });
    }
    for t in tickets.iter_mut() {
        t.functions.sort_unstable();
    }
    let unticketed = out.iter().enumerate().filter(|(_, c)| c.tickets.is_empty()).map(|(i, _)| i).collect();
    Changes {
        repo: "/r".into(),
        head: "h".repeat(40),
        since: bounded.then(|| "v1".to_string()),
        since_sha: bounded.then(|| "s".repeat(40)),
        created_unix: 1,
        patterns: vec![],
        commits: out,
        tickets,
        unticketed,
        files: vec![],
        caveats: vec![],
        impact: None,
        deleted: vec![],
    }
}

/// Evidence with `n` executed, fully covered, requirement-tagged functions
/// in one file, then the given change history attached.
fn ev(n: usize, commits: &[(&str, &[&str], &[usize])], bounded: bool) -> Evidence {
    let lines: Vec<String> = (0..n * 4).map(|i| format!("line {i}")).collect();
    let lref: Vec<&str> = lines.iter().map(String::as_str).collect();
    let counts: Vec<Option<u64>> = lref.iter().map(|_| Some(1)).collect();
    let mut file = common::file(&lref, &counts);
    let mut fns = Vec::new();
    for i in 0..n {
        let mut f = common::function(i, &format!("f{i}"), (i * 4 + 1) as u32, (i * 4 + 3) as u32);
        f.count = 1;
        f.lines_total = 3;
        f.lines_covered = 3;
        f.requirements = vec![format!("LLR-{i}")];
        file.function_ids.push(i);
        fns.push(f);
    }
    let mut e = common::evidence(vec![file], fns);
    let c = changes(commits, bounded);
    for (ci, cm) in c.commits.iter().enumerate() {
        for &f in &cm.functions {
            e.functions[f].commits.push(ci);
        }
    }
    e.changes = Some(c);
    e
}

fn res(keys: &[&str]) -> Resolutions {
    Resolutions {
        schema: RESOLUTIONS_SCHEMA.into(),
        entries: keys.iter().map(|k| Resolution { key: k.to_string(), category: "a".into(), justification: "j".into(), author: "a".into(), date: "d".into() }).collect(),
    }
}

fn kinds(imp: &Impact, fid: usize) -> Vec<String> {
    imp.obligations.iter().filter(|o| o.function_id == Some(fid)).map(|o| o.kind.clone()).collect()
}

fn of_kind<'a>(imp: &'a Impact, kind: &str) -> Vec<&'a Obligation> {
    imp.obligations.iter().filter(|o| o.kind == kind).collect()
}

const ONE_TICKETED: &[(&str, &[&str], &[usize])] = &[("ABC-1: change f0", &["ABC-1"], &[0])];

// ===================================================================== rules

#[test]
fn i01_changed_but_never_executed_is_a_blocker() {
    let mut e = ev(1, ONE_TICKETED, true);
    e.functions[0].count = 0;
    let imp = impact::compute(&e, &res(&[])).unwrap();
    let o = of_kind(&imp, "not-exercised");
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].severity, "blocker");
    assert_eq!(o[0].function_id, Some(0));
}

#[test]
fn i02_clean_change_yields_exactly_one_retest_obligation() {
    let e = ev(1, ONE_TICKETED, true);
    let imp = impact::compute(&e, &res(&[])).unwrap();
    assert_eq!(kinds(&imp, 0), vec!["retest-requirement"]);
    let o = &imp.obligations[0];
    assert_eq!(o.severity, "advisory");
    assert_eq!(o.requirement.as_deref(), Some("LLR-0"));
    assert_eq!(imp.functions_changed, vec![0]);
    assert!(imp.bounded);
}

#[test]
fn i03_uncovered_lines_in_a_changed_function_are_major() {
    let mut e = ev(1, ONE_TICKETED, true);
    e.functions[0].lines_total = 5;
    e.functions[0].lines_covered = 3;
    let imp = impact::compute(&e, &res(&[])).unwrap();
    let o = of_kind(&imp, "uncovered-lines");
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].severity, "major");
    assert!(o[0].detail.contains("2 of 5"), "{}", o[0].detail);
}

#[test]
fn i04_incomplete_mcdc_is_major_and_complete_is_silent() {
    let mut e = ev(1, ONE_TICKETED, true);
    e.functions[0].decisions.push(Decision { line: 2, mcdc_covered: false, ..Default::default() });
    let imp = impact::compute(&e, &res(&[])).unwrap();
    assert_eq!(of_kind(&imp, "mcdc-incomplete").len(), 1);
    assert_eq!(of_kind(&imp, "mcdc-incomplete")[0].severity, "major");
    e.functions[0].decisions[0].mcdc_covered = true;
    let imp2 = impact::compute(&e, &res(&[])).unwrap();
    assert!(of_kind(&imp2, "mcdc-incomplete").is_empty());
}

#[test]
fn i05_existing_source_to_object_review_becomes_stale() {
    let e = ev(2, ONE_TICKETED, true);
    let imp = impact::compute(&e, &res(&["s2o:f0", "s2o:f1"])).unwrap();
    let o = of_kind(&imp, "stale-review");
    assert_eq!(o.len(), 1, "only the changed function's review goes stale");
    assert_eq!(o[0].function_id, Some(0));
    assert_eq!(o[0].severity, "blocker");
    assert_eq!(o[0].key.as_deref(), Some("s2o:f0"));
}

#[test]
fn i06_resolutions_keyed_to_the_changed_function_become_stale() {
    let mut e = ev(2, ONE_TICKETED, true);
    e.functions[0].object = Some(ObjectFn { gaps: vec![Gap { key: "_f0@helperCall@100".into(), ..Default::default() }], ..Default::default() });
    e.functions[1].object = Some(ObjectFn { gaps: vec![Gap { key: "_f1@helperCall@200".into(), ..Default::default() }], ..Default::default() });
    let r = res(&[
        "_f0@helperCall@100", // gap of f0
        "_f1@helperCall@200", // gap of f1 -- unchanged
        "cond:0:2:5:1",       // condition in f0
        "branch:0:2:5:T",     // branch outcome in f0
        "fn:0",               // function-level in f0
        "line:0:2",           // line 2 is inside f0 (1..=3)
        "line:0:6",           // line 6 is inside f1 (5..=7)
        "dead:f0",
        "cond:1:6:1:0",
    ]);
    let imp = impact::compute(&e, &r).unwrap();
    let mut keys: Vec<&str> = of_kind(&imp, "stale-resolution").iter().map(|o| o.key.as_deref().unwrap()).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["_f0@helperCall@100", "branch:0:2:5:T", "cond:0:2:5:1", "dead:f0", "fn:0", "line:0:2"]);
    assert!(of_kind(&imp, "stale-resolution").iter().all(|o| o.function_id == Some(0) && o.severity == "blocker"));
}

#[test]
fn i07_change_without_a_requirement_tag_is_a_blocker() {
    let mut e = ev(1, ONE_TICKETED, true);
    e.functions[0].requirements.clear();
    let imp = impact::compute(&e, &res(&[])).unwrap();
    assert_eq!(of_kind(&imp, "no-requirement").len(), 1);
    assert_eq!(of_kind(&imp, "no-requirement")[0].severity, "blocker");
    assert!(of_kind(&imp, "retest-requirement").is_empty());
    // a line-level tag is a requirement too
    e.functions[0].line_requirements.insert(2, vec!["LLR-X.1".into()]);
    let imp2 = impact::compute(&e, &res(&[])).unwrap();
    assert!(of_kind(&imp2, "no-requirement").is_empty());
    assert_eq!(of_kind(&imp2, "retest-requirement")[0].requirement.as_deref(), Some("LLR-X.1"));
}

#[test]
fn i08_unticketed_commit_is_a_blocker_naming_the_commit() {
    let e = ev(1, &[("ABC-1: ok", &["ABC-1"], &[0]), ("no ticket", &[], &[0])], true);
    let imp = impact::compute(&e, &res(&[])).unwrap();
    let o = of_kind(&imp, "unticketed");
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].severity, "blocker");
    assert_eq!(o[0].commit, Some(1));
    assert!(o[0].detail.contains("no ticket"), "{}", o[0].detail);
}

#[test]
fn i09_callers_of_a_changed_function_must_be_retested() {
    let mut e = ev(3, ONE_TICKETED, true);
    e.functions[0].callers = vec![
        CallEdge { function_id: Some(1), ..Default::default() },
        CallEdge { function_id: Some(2), ..Default::default() },
        CallEdge { function_id: None, symbol: "external".into(), ..Default::default() },
        CallEdge { function_id: Some(0), ..Default::default() }, // recursion: not a coupling item
    ];
    let imp = impact::compute(&e, &res(&[])).unwrap();
    let o = of_kind(&imp, "retest-caller");
    let mut targets: Vec<usize> = o.iter().map(|x| x.target_function.unwrap()).collect();
    targets.sort_unstable();
    assert_eq!(targets, vec![1, 2]);
    assert!(o.iter().all(|x| x.severity == "advisory" && x.function_id == Some(0)));
    assert!(o.iter().any(|x| x.detail.contains("LLR-1")), "caller's requirements named: {:?}", o.iter().map(|x| &x.detail).collect::<Vec<_>>());
}

#[test]
fn i10_readers_of_a_global_the_change_writes_are_coupled() {
    let mut e = ev(3, ONE_TICKETED, true);
    e.globals.push(Global { id: 0, name: "g_state".into(), writers: vec![0], readers: vec![1, 0], ..Default::default() });
    e.globals.push(Global { id: 1, name: "only_read".into(), writers: vec![2], readers: vec![0], ..Default::default() });
    let imp = impact::compute(&e, &res(&[])).unwrap();
    let o = of_kind(&imp, "data-coupling");
    assert_eq!(o.len(), 1, "one reader of a global f0 writes; f0 reading its own global is not coupling; a global f0 only reads is not f0's impact");
    assert_eq!(o[0].target_function, Some(1));
    assert_eq!(o[0].key.as_deref(), Some("g_state"));
    assert_eq!(o[0].severity, "advisory");
}

#[test]
fn i11_changed_dead_code_is_a_blocker() {
    let mut e = ev(1, ONE_TICKETED, true);
    e.image = Some(Image { dead_candidates: vec![DeadCandidate { function_id: Some(0), name: "f0".into(), ..Default::default() }], ..Default::default() });
    let imp = impact::compute(&e, &res(&[])).unwrap();
    assert_eq!(of_kind(&imp, "dead-changed").len(), 1);
    assert_eq!(of_kind(&imp, "dead-changed")[0].severity, "blocker");
}

#[test]
fn i12_unchanged_functions_get_nothing() {
    let mut e = ev(3, ONE_TICKETED, true);
    e.functions[1].count = 0; // would be a blocker if it had changed
    e.functions[2].requirements.clear();
    let imp = impact::compute(&e, &res(&["s2o:f1", "s2o:f2"])).unwrap();
    assert!(imp.obligations.iter().all(|o| o.function_id == Some(0)), "{:?}", imp.obligations);
    assert_eq!(imp.functions_changed, vec![0]);
}

#[test]
fn i13_grouping_by_ticket_kind_and_severity() {
    let mut e = ev(3, &[("ABC-1: f0", &["ABC-1"], &[0]), ("ABC-2: f1 f2", &["ABC-2"], &[1, 2]), ("no ticket", &[], &[2])], true);
    e.functions[0].count = 0;
    e.functions[2].requirements.clear();
    let imp = impact::compute(&e, &res(&[])).unwrap();
    let c = e.changes.as_ref().unwrap();
    let t1 = imp.by_ticket.iter().find(|t| c.tickets[t.ticket].id == "ABC-1").unwrap();
    let t2 = imp.by_ticket.iter().find(|t| c.tickets[t.ticket].id == "ABC-2").unwrap();
    assert!(t1.obligations.iter().all(|&i| imp.obligations[i].function_id == Some(0)));
    assert_eq!(t1.blockers, 1, "f0 not exercised");
    assert!(t2.obligations.iter().all(|&i| [Some(1), Some(2)].contains(&imp.obligations[i].function_id)));
    assert_eq!(t2.blockers, 2, "f2: no requirement + unticketed commit");
    assert_eq!(imp.by_kind["not-exercised"], 1);
    assert_eq!(imp.by_kind["retest-requirement"], 2, "f0 and f1 carry requirements; f2 has none");
    assert_eq!(imp.by_severity["blocker"], 3);
    assert_eq!(imp.by_severity["advisory"], 2);
}

#[test]
fn i14_ordering_is_by_severity_then_function_then_kind_and_deterministic() {
    let mut e = ev(3, &[("ABC-1: all", &["ABC-1"], &[0, 1, 2])], true);
    e.functions[2].count = 0;
    e.functions[0].lines_total = 4;
    e.functions[0].lines_covered = 1;
    e.functions[1].callers = vec![CallEdge { function_id: Some(0), ..Default::default() }];
    let a = impact::compute(&e, &res(&[])).unwrap();
    let b = impact::compute(&e, &res(&[])).unwrap();
    assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap());
    let rank = |s: &str| match s {
        "blocker" => 0,
        "major" => 1,
        _ => 2,
    };
    let sev: Vec<i32> = a.obligations.iter().map(|o| rank(&o.severity)).collect();
    let mut sorted = sev.clone();
    sorted.sort_unstable();
    assert_eq!(sev, sorted, "blockers, then majors, then advisories: {:?}", a.obligations.iter().map(|o| (&o.severity, o.function_id, &o.kind)).collect::<Vec<_>>());
    assert_eq!(a.obligations[0].kind, "not-exercised");
    assert_eq!(a.obligations[1].kind, "uncovered-lines");
}

#[test]
fn i15_fail_levels() {
    let mut e = ev(2, &[("ABC-1: all", &["ABC-1"], &[0, 1])], true);
    let advisory_only = impact::compute(&e, &res(&[])).unwrap();
    assert!(!impact::fails(&advisory_only, Level::None));
    assert!(!impact::fails(&advisory_only, Level::Blocker));
    assert!(!impact::fails(&advisory_only, Level::Major));
    assert!(impact::fails(&advisory_only, Level::Any));
    e.functions[0].lines_total = 3;
    e.functions[0].lines_covered = 1;
    let with_major = impact::compute(&e, &res(&[])).unwrap();
    assert!(!impact::fails(&with_major, Level::Blocker));
    assert!(impact::fails(&with_major, Level::Major));
    e.functions[1].count = 0;
    let with_blocker = impact::compute(&e, &res(&[])).unwrap();
    assert!(impact::fails(&with_blocker, Level::Blocker));
    assert!(!impact::fails(&with_blocker, Level::None));
    assert_eq!(Level::parse("major").unwrap(), Level::Major);
    assert_eq!(Level::parse("BLOCKER").unwrap(), Level::Blocker);
    assert!(Level::parse("bogus").is_err());
}

// =================================================================== abnormal

#[test]
fn a16_evidence_without_change_history_is_an_error_that_says_what_to_run() {
    let e = common::evidence(vec![], vec![]);
    let err = impact::compute(&e, &res(&[])).unwrap_err();
    assert!(err.contains("butterfly changes"), "{err}");
}

#[test]
fn a17_unbounded_history_computes_but_says_every_function_counts_as_changed() {
    let e = ev(2, &[("ABC-1: all", &["ABC-1"], &[0, 1])], false);
    let imp = impact::compute(&e, &res(&[])).unwrap();
    assert!(!imp.bounded);
    assert_eq!(imp.functions_changed, vec![0, 1]);
    assert!(imp.caveats.iter().any(|c| c.contains("--since")), "{:?}", imp.caveats);
}

#[test]
fn a18_resolutions_for_unknown_functions_are_ignored_without_panicking() {
    let e = ev(1, ONE_TICKETED, true);
    let imp = impact::compute(&e, &res(&["fn:99", "cond:99:1:1:0", "line:7:3", "s2o:nobody", "garbage", "line:0:notanumber"])).unwrap();
    assert!(of_kind(&imp, "stale-resolution").is_empty() && of_kind(&imp, "stale-review").is_empty());
}

#[test]
fn a19_malformed_indices_do_not_panic() {
    let mut e = ev(2, ONE_TICKETED, true);
    e.functions[0].commits = vec![0, 99]; // out of range commit
    e.functions[0].callers = vec![CallEdge { function_id: Some(99), ..Default::default() }];
    e.globals.push(Global { id: 0, name: "g".into(), writers: vec![0, 99], readers: vec![99, 1], ..Default::default() });
    e.image = Some(Image { dead_candidates: vec![DeadCandidate { function_id: Some(99), ..Default::default() }], ..Default::default() });
    let r = impact::compute(&e, &res(&[]));
    assert!(r.is_ok(), "{:?}", r.err());
    let imp = r.unwrap();
    assert!(imp
        .obligations
        .iter()
        .all(|o| o.function_id.map(|f| f < 2).unwrap_or(true) && o.target_function.map(|t| t < 2).unwrap_or(true) && o.commit.map(|c| c < 1).unwrap_or(true)));
    assert_eq!(of_kind(&imp, "data-coupling").len(), 1, "the valid reader survives, the bogus one is dropped");
}

#[test]
fn a20_same_finding_reached_through_two_commits_is_reported_once() {
    let mut e = ev(1, &[("ABC-1: first", &["ABC-1"], &[0]), ("ABC-2: second", &["ABC-2"], &[0])], true);
    e.functions[0].count = 0;
    let imp = impact::compute(&e, &res(&["s2o:f0"])).unwrap();
    assert_eq!(of_kind(&imp, "not-exercised").len(), 1);
    assert_eq!(of_kind(&imp, "stale-review").len(), 1);
    assert_eq!(of_kind(&imp, "retest-requirement").len(), 1);
    // but both tickets see them
    assert_eq!(imp.by_ticket.len(), 2);
    assert!(imp.by_ticket.iter().all(|t| t.blockers == 2));
}

// ================================================================= robustness

#[test]
fn r21_impact_round_trips_and_validates() {
    let mut e = ev(2, &[("ABC-1: f0", &["ABC-1"], &[0]), ("none", &[], &[1])], true);
    e.functions[1].count = 0;
    let imp = impact::compute(&e, &res(&["s2o:f0"])).unwrap();
    e.changes.as_mut().unwrap().impact = Some(imp);
    e.validate().unwrap();
    let s = serde_json::to_string(&e).unwrap();
    let back: Evidence = serde_json::from_str(&s).unwrap();
    back.validate().unwrap();
    assert_eq!(serde_json::to_string(&back.changes).unwrap(), serde_json::to_string(&e.changes).unwrap());
}

#[test]
fn r22_validate_rejects_bad_impact_indices() {
    let mut e = ev(2, ONE_TICKETED, true);
    let imp = impact::compute(&e, &res(&[])).unwrap();
    e.changes.as_mut().unwrap().impact = Some(imp);
    e.validate().unwrap();
    let base = e.clone();
    let mut bad = base.clone();
    bad.changes.as_mut().unwrap().impact.as_mut().unwrap().obligations[0].function_id = Some(99);
    assert!(bad.validate().is_err());
    let mut bad = base.clone();
    bad.changes.as_mut().unwrap().impact.as_mut().unwrap().obligations[0].target_function = Some(99);
    assert!(bad.validate().is_err());
    let mut bad = base.clone();
    bad.changes.as_mut().unwrap().impact.as_mut().unwrap().obligations[0].commit = Some(99);
    assert!(bad.validate().is_err());
    let mut bad = base.clone();
    bad.changes.as_mut().unwrap().impact.as_mut().unwrap().functions_changed = vec![99];
    assert!(bad.validate().is_err());
    let mut bad = base.clone();
    bad.changes.as_mut().unwrap().impact.as_mut().unwrap().by_ticket[0].ticket = 99;
    assert!(bad.validate().is_err());
    let mut bad = base.clone();
    bad.changes.as_mut().unwrap().impact.as_mut().unwrap().by_ticket[0].obligations = vec![99];
    assert!(bad.validate().is_err());
}

#[test]
fn r23_line_keys_use_the_function_file_not_just_any_file() {
    // f0 is in file 0, lines 1..=3. A `line:1:2` key is file 1 -- not f0's.
    let e = ev(1, ONE_TICKETED, true);
    let imp = impact::compute(&e, &res(&["line:1:2"])).unwrap();
    assert!(of_kind(&imp, "stale-resolution").is_empty());
    let imp2 = impact::compute(&e, &res(&["line:0:3"])).unwrap();
    assert_eq!(of_kind(&imp2, "stale-resolution").len(), 1);
    let imp3 = impact::compute(&e, &res(&["line:0:4"])).unwrap();
    assert!(of_kind(&imp3, "stale-resolution").is_empty(), "line 4 is outside f0");
}

#[test]
fn r24_many_functions_stay_fast_and_bounded() {
    let n = 400;
    let fns: Vec<usize> = (0..n).collect();
    let mut e = ev(n, &[("ABC-1: everything", &["ABC-1"], &fns)], true);
    for i in 0..n {
        e.functions[i].callers = (0..n).step_by(50).map(|c| CallEdge { function_id: Some(c), ..Default::default() }).collect();
    }
    let t = std::time::Instant::now();
    let imp = impact::compute(&e, &res(&[])).unwrap();
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    assert_eq!(imp.by_kind["retest-requirement"], n);
    assert_eq!(imp.by_ticket[0].obligations.len(), imp.obligations.len());
    let _ = BTreeMap::<String, usize>::new();
}

/// #249: a persisted impact is bound to the resolution set and evidence
/// revision it was derived from, so a consumer can tell when it has gone stale
/// rather than present an old count as current.
#[test]
fn a_persisted_impact_is_bound_to_its_inputs() {
    let e = ev(2, &[("c0", &["ABC-1"], &[0])], true);
    let r = res(&["s2o:f0"]);
    let imp = impact::compute(&e, &r).unwrap();
    assert!(!imp.resolutions_digest.is_empty() && !imp.evidence_digest.is_empty());

    // Against the very inputs it was derived from, it is current.
    assert!(impact::staleness(&imp, &e, &r).is_none());

    // A changed resolution set is detected.
    let reason = impact::staleness(&imp, &e, &res(&["s2o:f0", "s2o:f1"])).unwrap();
    assert!(reason.contains("resolutions"), "{reason}");

    // A changed evidence revision (re-walked to a different HEAD) is detected.
    let mut e2 = e.clone();
    e2.changes.as_mut().unwrap().head = "d".repeat(40);
    let reason = impact::staleness(&imp, &e2, &r).unwrap();
    assert!(reason.contains("evidence"), "{reason}");

    // A changed derived record with the SAME build inputs and HEAD is detected
    // too: re-attributing a function (an analysis change, not a rebuild or
    // re-walk) must invalidate the cached impact (#249).
    let mut e3 = e.clone();
    e3.functions[1].commits.push(0);
    assert!(impact::staleness(&imp, &e3, &r).is_some(), "a changed function record with the same provenance/HEAD must invalidate the cache");

    // An impact written before the binding existed is unverifiable, not
    // silently trusted.
    let mut old = imp.clone();
    old.resolutions_digest.clear();
    old.evidence_digest.clear();
    assert!(impact::staleness(&old, &e, &r).unwrap().contains("recorded before"));
}

/// The data-coupling obligations compute() raises from `ev.globals` are part of
/// the impact, so a change to a global's reader/writer set -- an analysis
/// re-attribution with the same build inputs and HEAD -- must move the evidence
/// digest and make a cached impact stale (#249). Before globals were folded in,
/// the two revisions below shared a digest, so a persisted impact carrying the
/// old obligation read as current for the new relationship.
#[test]
fn a_changed_global_relationship_invalidates_the_cached_impact() {
    // f0 is the changed function; the global it writes is read by f1.
    let mut e = ev(3, &[("c0", &["ABC-1"], &[0])], true);
    e.globals.push(Global { id: 0, name: "g_state".into(), writers: vec![0], readers: vec![1], ..Default::default() });
    let r = res(&[]);
    let imp = impact::compute(&e, &r).unwrap();

    let coupling = |imp: &butterfly_model::Impact| -> Vec<(Option<usize>, Option<usize>)> {
        imp.obligations.iter().filter(|o| o.kind == "data-coupling").map(|o| (o.function_id, o.target_function)).collect()
    };
    // The obligation is f0 -> f1 against the inputs it was derived from.
    assert_eq!(coupling(&imp), vec![(Some(0), Some(1))]);
    assert!(impact::staleness(&imp, &e, &r).is_none());

    // Re-attribute the reader to f2, same provenance/HEAD, no other field
    // touched. The obligation changes...
    let mut e2 = e.clone();
    e2.globals[0].readers = vec![2];
    let imp2 = impact::compute(&e2, &r).unwrap();
    assert_eq!(coupling(&imp2), vec![(Some(0), Some(2))], "the new reader must produce a different obligation");

    // ...and the impact cached against the old relationship must now be stale.
    let reason = impact::staleness(&imp, &e2, &r).expect("a changed global reader with the same provenance/HEAD must invalidate the cache");
    assert!(reason.contains("evidence"), "{reason}");
}
