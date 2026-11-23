// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests, area D: what must be re-verified.
//!
//! Expected values come from README.md "What must be re-verified"
//! (README.md:1237-1269) and docs/butterfly.html "The obligations", not from
//! the implementation. Pure: hand-built evidence with a hand-built change
//! history, no git.

mod common;

use butterfly_ingest::impact::{self, Level};
use butterfly_model::*;
use std::collections::BTreeMap;

// ---------------------------------------------------------------- helpers

/// `(subject, tickets)` per commit, each touching the functions listed.
type C<'a> = (&'a str, &'a [&'a str], &'a [usize]);

fn changes(commits: &[C]) -> Changes {
    let mut tickets: Vec<Ticket> = Vec::new();
    let mut out = Vec::new();
    for (i, (subj, tks, fns)) in commits.iter().enumerate() {
        let mut tix = Vec::new();
        for t in tks.iter() {
            let ti = tickets.iter().position(|x| x.id == *t).unwrap_or_else(|| {
                tickets.push(Ticket { id: t.to_string(), ..Default::default() });
                tickets.len() - 1
            });
            tickets[ti].commits.push(i);
            for f in fns.iter() {
                if !tickets[ti].functions.contains(f) {
                    tickets[ti].functions.push(*f);
                    tickets[ti].modified.push(*f);
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
    let unticketed = out.iter().enumerate().filter(|(_, c)| c.tickets.is_empty()).map(|(i, _)| i).collect();
    Changes {
        repo: "/r".into(),
        head: "h".repeat(40),
        since: Some("v1".into()),
        since_sha: Some("s".repeat(40)),
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

/// `n` executed, fully covered functions of four lines each in one file
/// (`/src/demo.c`), each traced to `LLR-<i>`.
fn ev(n: usize, commits: &[C]) -> Evidence {
    let lines: Vec<String> = (0..n * 4).map(|i| format!("line {i}")).collect();
    let lref: Vec<&str> = lines.iter().map(String::as_str).collect();
    let counts: Vec<Option<u64>> = lref.iter().map(|_| Some(1)).collect();
    let mut file = common::file(&lref, &counts);
    let mut fns = Vec::new();
    for i in 0..n {
        let mut f = common::function(i, &format!("f{i}"), (i * 4 + 1) as u32, (i * 4 + 3) as u32);
        f.lines_total = 3;
        f.lines_covered = 3;
        f.requirements = vec![format!("LLR-{i}")];
        file.function_ids.push(i);
        fns.push(f);
    }
    let mut e = common::evidence(vec![file], fns);
    let c = changes(commits);
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
        entries: keys.iter().map(|k| Resolution { key: k.to_string(), category: "a".into(), justification: "j".into(), author: "a".into(), date: "2026-01-01".into() }).collect(),
    }
}

fn of_kind<'a>(imp: &'a Impact, kind: &str) -> Vec<&'a Obligation> {
    imp.obligations.iter().filter(|o| o.kind == kind).collect()
}

const ONE: &[C] = &[("ABC-1: change f0", &["ABC-1"], &[0])];

// ===================================================================== tests

/// README.md:1261-1262 -- `no-requirement` is "it traces to no requirement";
/// `retest-requirement` is "re-run the tests for this requirement". A
/// requirement tag found on a line inside the body (`scope = "body"`,
/// README.md:1164) is a trace like a header tag.
#[test]
fn m01_a_requirement_tagged_on_a_body_line_is_a_trace() {
    let mut e = ev(1, ONE);
    e.functions[0].requirements.clear();
    e.functions[0].line_requirements = BTreeMap::from([(2u32, vec!["LLR-BODY".to_string()])]);
    let imp = impact::compute(&e, &res(&[])).unwrap();
    assert!(of_kind(&imp, "no-requirement").is_empty(), "{:?}", imp.obligations);
    let rt = of_kind(&imp, "retest-requirement");
    assert_eq!(rt.len(), 1);
    assert_eq!(rt[0].requirement.as_deref(), Some("LLR-BODY"));
    assert_eq!(rt[0].severity, "advisory");
}

/// README.md:1261 -- with neither a header nor a body tag, the change is a
/// `no-requirement` blocker.
#[test]
fn m02_no_tag_anywhere_is_a_no_requirement_blocker() {
    let mut e = ev(1, ONE);
    e.functions[0].requirements.clear();
    let imp = impact::compute(&e, &res(&[])).unwrap();
    let o = of_kind(&imp, "no-requirement");
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].severity, "blocker");
    assert!(of_kind(&imp, "retest-requirement").is_empty());
}

/// README.md:1263 -- `stale-resolution` | blocker | "a gap / condition /
/// branch / line / dead-code justification on it predates the change"; and
/// README.md:1262 `stale-review` for a source-to-object review. Each key kind
/// the viewer writes is resolved to its function.
#[test]
fn m03_every_kind_of_justification_on_a_changed_function_becomes_stale() {
    let mut e = ev(2, ONE);
    e.image = Some(Image { dead_candidates: vec![], ..Default::default() });
    // f0 spans lines 1-3 of file 0; f1 spans 5-7 and did not change
    let keys = ["line:0:2", "cond:0:0:0", "branch:0:1", "dead:f0", "s2o:f0", "line:0:6", "s2o:f1", "fn:0"];
    let imp = impact::compute(&e, &res(&keys)).unwrap();
    let mut stale: Vec<&str> = of_kind(&imp, "stale-resolution").iter().map(|o| o.key.as_deref().unwrap()).collect();
    stale.sort();
    assert_eq!(stale, vec!["branch:0:1", "cond:0:0:0", "dead:f0", "fn:0", "line:0:2"], "f1's line is not f0's");
    let rv = of_kind(&imp, "stale-review");
    assert_eq!(rv.len(), 1);
    assert_eq!(rv[0].key.as_deref(), Some("s2o:f0"), "f1 did not change, so its review stands");
    assert!(of_kind(&imp, "stale-resolution").iter().chain(rv.iter()).all(|o| o.severity == "blocker" && o.function_id == Some(0)));
    // keys that name nothing are ignored, never a panic
    let imp = impact::compute(&e, &res(&["line:9:2", "line:0:x", "zzz:1", "nokind", "line:0:400"])).unwrap();
    assert!(of_kind(&imp, "stale-resolution").is_empty());
}

/// README.md:1264-1265 and 1270-1272 -- `deleted` is a blocker, and
/// "Obligations are grouped per ticket — *'ABC-123: 4 functions, 2 blockers,
/// 9 obligations'*". A deleting commit that carries two tickets deleted the
/// function for both, so both tickets' groups count the blocker.
#[test]
fn m04_a_deletion_counts_under_every_ticket_of_its_commit() {
    let mut e = ev(1, &[("ABC-1 ABC-2: remove g", &["ABC-1", "ABC-2"], &[])]);
    let c = e.changes.as_mut().unwrap();
    c.deleted.push(DeletedFunction { name: "g".into(), file: "demo.c".into(), line_start: 9, line_end: 12, commit: Some(0) });
    for t in c.tickets.iter_mut() {
        t.deleted = vec![0];
    }
    let imp = impact::compute(&e, &res(&[])).unwrap();
    let del = of_kind(&imp, "deleted");
    assert_eq!(del.len(), 1);
    assert_eq!(del[0].severity, "blocker");
    for (ti, id) in [(0usize, "ABC-1"), (1, "ABC-2")] {
        let g = imp.by_ticket.iter().find(|b| b.ticket == ti).unwrap();
        assert_eq!(g.blockers, 1, "{id} deleted g, so its group has that blocker: {:?}", imp.by_ticket);
    }
}

/// README.md:1265 -- `untraced-file` | major | "the ticket changed a file
/// holding no function in the evidence"; README.md:1270 -- grouped per
/// ticket. Major, not blocker: `--fail-on major` trips, `--fail-on blocker`
/// does not.
#[test]
fn m05_a_ticket_that_only_changed_a_header_is_a_major_finding_under_that_ticket() {
    let mut e = ev(1, &[("ABC-9: raise the limit", &["ABC-9"], &[])]);
    let c = e.changes.as_mut().unwrap();
    c.commits[0].files = vec![FileChange { path: "limits.h".into(), status: "modified".into() }];
    c.tickets[0].files = vec![TicketFile { path: "limits.h".into(), status: "modified".into(), in_evidence: false, functions: 0 }];
    let imp = impact::compute(&e, &res(&[])).unwrap();
    let u = of_kind(&imp, "untraced-file");
    assert_eq!(u.len(), 1, "{:?}", imp.obligations);
    assert_eq!((u[0].severity.as_str(), u[0].file.as_deref(), u[0].ticket), ("major", Some("limits.h"), Some(0)));
    assert!(u[0].detail.contains("limits.h"));
    let g = &imp.by_ticket[0];
    assert_eq!((g.obligations.len(), g.blockers), (1, 0));
    assert!(impact::fails(&imp, Level::Major));
    assert!(!impact::fails(&imp, Level::Blocker));
}

/// README.md:1249-1251 (`--fail-on blocker`, `--fail-on major`) and
/// README.md:1320-1321 (the threshold): the level names parse, a bogus one
/// is refused by name, and `none` never fails.
#[test]
fn m06_fail_on_levels() {
    assert!(matches!(Level::parse("blocker"), Ok(Level::Blocker)));
    assert!(matches!(Level::parse("major"), Ok(Level::Major)));
    assert!(matches!(Level::parse("none"), Ok(Level::None)));
    let err = Level::parse("critical").unwrap_err();
    assert!(err.contains("critical"), "{err}");
    let mut e = ev(1, ONE);
    e.functions[0].count = 0;
    let imp = impact::compute(&e, &res(&[])).unwrap();
    assert!(impact::fails(&imp, Level::Blocker) && impact::fails(&imp, Level::Major) && impact::fails(&imp, Level::Any));
    assert!(!impact::fails(&imp, Level::None));
}

/// README.md:1259-1261 -- an unticketed commit that only touched a file with
/// no function in it is still an audit finding: the file is reported.
#[test]
fn m07_an_unticketed_commit_to_a_file_with_no_function_is_reported() {
    let mut e = ev(1, &[("tweak the linker script", &[], &[])]);
    e.changes.as_mut().unwrap().commits[0].files = vec![FileChange { path: "link.ld".into(), status: "modified".into() }];
    let imp = impact::compute(&e, &res(&[])).unwrap();
    let u = of_kind(&imp, "untraced-file");
    assert_eq!(u.len(), 1, "{:?}", imp.obligations);
    assert_eq!(u[0].file.as_deref(), Some("link.ld"));
    assert_eq!(u[0].commit, Some(0));
}

/// README.md:1325-1328 (Staleness) -- the persisted obligations are bound to
/// the resolutions they were computed against: writing one more resolution
/// makes them stale, and the unchanged inputs do not.
#[test]
fn m08_a_new_resolution_makes_the_persisted_obligations_stale() {
    let e = ev(1, ONE);
    let r0 = res(&[]);
    let imp = impact::compute(&e, &r0).unwrap();
    assert!(impact::staleness(&imp, &e, &r0).is_none(), "same inputs: current");
    let r1 = res(&["s2o:f0"]);
    assert!(impact::staleness(&imp, &e, &r1).is_some(), "a resolution written since: stale");
    let mut e2 = e.clone();
    e2.changes.as_mut().unwrap().head = "e".repeat(40);
    assert!(impact::staleness(&imp, &e2, &r0).is_some(), "history read at another HEAD: stale");
}
