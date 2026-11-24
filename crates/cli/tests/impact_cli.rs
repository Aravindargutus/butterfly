// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly impact` and `changes --fail-on` through the binary, on
//! examples/c/build/evidence.json (skipped when the example is not built).
//! This repository's commit messages carry no ticket ids, so every changed
//! function has an `unticketed` blocker -- which is what the exit code tests
//! rely on. Nothing is written into the example directory.

mod common;

use butterfly_model::{Changes, Commit, Evidence};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

fn evidence() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build/evidence.json");
    if p.exists() {
        Some(p.canonicalize().unwrap())
    } else {
        butterfly_testsupport::fixture_missing("build examples/c first");
        None
    }
}

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("impact-{tag}"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_butterfly")).args(args).output().expect("spawn butterfly")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}
fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

/// Evidence with change history attached, in a scratch dir.
fn with_changes(tag: &str) -> Option<(PathBuf, PathBuf)> {
    let ev = evidence()?;
    let d = scratch(tag);
    let e = d.join("ev.json");
    let o = run(&["changes", ev.to_str().unwrap(), "-o", e.to_str().unwrap(), "--cache", d.join("cache.json").to_str().unwrap()]);
    assert!(o.status.success(), "{}", err(&o));
    Some((d, e))
}

#[test]
fn t01_impact_prints_obligations_and_exits_zero_by_default() {
    let Some(input) = evidence() else { return };
    let mut ev: Evidence = serde_json::from_slice(&fs::read(input).unwrap()).unwrap();
    // One unticketed edit keeps both the blocker and the advisory inside the
    // 300-row text preview, regardless of the history of this checkout.
    let function = ev.functions.iter_mut().find(|f| f.name == "clamp_altitude").unwrap();
    function.commits = vec![0];
    let commit = Commit { subject: "Adjust altitude limit".into(), functions: vec![function.id], ..Default::default() };
    ev.changes = Some(Changes { commits: vec![commit], unticketed: vec![0], ..Default::default() });
    let d = scratch("print");
    let e = d.join("ev.json");
    fs::write(&e, serde_json::to_vec(&ev).unwrap()).unwrap();
    let o = run(&["impact", e.to_str().unwrap()]);
    assert!(o.status.success(), "{}", err(&o));
    let s = out(&o);
    assert!(s.contains("unticketed"), "{s}");
    assert!(s.contains("retest-requirement"), "{s}");
    assert!(s.contains("blocker"), "{s}");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn t02_fail_on_blocker_exits_three() {
    let Some((d, e)) = with_changes("fail") else { return };
    let o = run(&["impact", e.to_str().unwrap(), "--fail-on", "blocker"]);
    assert_eq!(o.status.code(), Some(3), "stdout: {} stderr: {}", out(&o), err(&o));
    let _ = fs::remove_dir_all(d);
}

#[test]
fn t03_json_output_is_the_impact_record() {
    let Some((d, e)) = with_changes("json") else { return };
    let o = run(&["impact", e.to_str().unwrap(), "--json"]);
    assert!(o.status.success(), "{}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).expect("stdout is JSON");
    assert!(v["obligations"].as_array().unwrap().iter().any(|row| row["kind"] == "retest-requirement"), "full JSON should include the requirement advisory");
    assert_eq!(v["bounded"], false);
    assert!(v["bySeverity"]["blocker"].as_u64().unwrap() > 0);
    let _ = fs::remove_dir_all(d);
}

#[test]
fn t04_evidence_without_change_history_fails_and_says_what_to_run() {
    let Some(ev) = evidence() else { return };
    let o = run(&["impact", ev.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("butterfly changes"), "{}", err(&o));
}

#[test]
fn t05_write_stores_the_impact_in_the_evidence() {
    let Some((d, e)) = with_changes("write") else { return };
    let before: serde_json::Value = serde_json::from_str(&fs::read_to_string(&e).unwrap()).unwrap();
    // `changes` already computed an impact; remove it to prove --write puts it back
    let mut stripped = before.clone();
    stripped["changes"]["impact"] = serde_json::Value::Null;
    fs::write(&e, serde_json::to_string(&stripped).unwrap()).unwrap();
    let o = run(&["impact", e.to_str().unwrap(), "--write"]);
    assert!(o.status.success(), "{}", err(&o));
    let after: serde_json::Value = serde_json::from_str(&fs::read_to_string(&e).unwrap()).unwrap();
    assert!(after["changes"]["impact"]["obligations"].as_array().map(|a| !a.is_empty()).unwrap_or(false));
    let _ = fs::remove_dir_all(d);
}

#[test]
fn t06_bogus_level_is_rejected_by_name() {
    let Some((d, e)) = with_changes("level") else { return };
    let o = run(&["impact", e.to_str().unwrap(), "--fail-on", "bogus"]);
    assert!(!o.status.success());
    assert_ne!(o.status.code(), Some(3), "a usage error is not a threshold failure");
    assert!(err(&o).contains("bogus"), "{}", err(&o));
    let _ = fs::remove_dir_all(d);
}

#[test]
fn t07_changes_fail_on_still_writes_the_evidence_then_exits_three() {
    let Some(ev) = evidence() else { return };
    let d = scratch("changes-fail");
    let e = d.join("ev.json");
    let o = run(&["changes", ev.to_str().unwrap(), "-o", e.to_str().unwrap(), "--cache", d.join("c.json").to_str().unwrap(), "--fail-on", "blocker"]);
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&e).unwrap()).unwrap();
    assert!(v["changes"]["impact"]["obligations"].as_array().map(|a| !a.is_empty()).unwrap_or(false), "impact is written even when the threshold fails");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn t08_resolutions_sidecar_makes_reviews_stale() {
    let Some((d, e)) = with_changes("res") else { return };
    let r = d.join("resolutions.json");
    fs::write(&r, r#"{"schema":"butterfly-resolutions/1","entries":[{"key":"s2o:main","category":"traceable","justification":"reviewed","author":"me","date":"2026-01-01"}]}"#)
        .unwrap();
    let o = run(&["impact", e.to_str().unwrap(), "--resolutions", r.to_str().unwrap()]);
    assert!(o.status.success(), "{}", err(&o));
    let s = out(&o);
    assert!(s.contains("stale-review") && s.contains("main"), "{s}");
    let _ = fs::remove_dir_all(d);
}

/// The viewer binds the persisted impact to what it loaded, across a restart
/// (#249). A freshly written impact over an unchanged sidecar is not reported
/// as drifted; a sidecar edited while no server ran makes `/api/freshness`
/// say so from the first request (the in-process edit counter is zero); and
/// recomputing the impact against that sidecar clears it again.
#[test]
fn t09_the_viewer_reports_a_persisted_impact_that_no_longer_binds_after_a_restart() {
    let Some((d, e)) = with_changes("serve") else { return };
    let r = d.join("resolutions.json");
    let drift = "since this impact was recorded";
    let freshness = |what: &str| -> String {
        let mut srv = common::serve(Command::new(env!("CARGO_BIN_EXE_butterfly")).args(["serve", e.to_str().unwrap(), "--resolutions", r.to_str().unwrap()]));
        let (code, body) = common::get(&mut srv, "/api/freshness");
        srv.stop();
        assert_eq!(code, 200, "{what}: {body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|x| panic!("{what}: not JSON ({x}): {body}"));
        assert_eq!(v["resolutionEdits"], 0, "{what}: a fresh process has written nothing: {body}");
        body
    };

    // written against an empty sidecar, served with the same one: bound
    fs::write(&r, r#"{"schema":"butterfly-resolutions/1","entries":[]}"#).unwrap();
    let o = run(&["impact", e.to_str().unwrap(), "--write", "--resolutions", r.to_str().unwrap()]);
    assert!(o.status.success(), "{}", err(&o));
    let body = freshness("freshly written");
    assert!(!body.contains(drift) && !body.contains("rebuilt or its change history re-walked"), "a freshly written impact is not stale: {body}");

    // the sidecar changes while no server runs: stale on the first request
    fs::write(&r, r#"{"schema":"butterfly-resolutions/1","entries":[{"key":"s2o:main","category":"traceable","justification":"reviewed","author":"me","date":"2026-01-01"}]}"#)
        .unwrap();
    let body = freshness("sidecar edited between runs");
    assert!(body.contains("the resolutions have changed since this impact was recorded"), "{body}");
    assert!(body.contains("\"stale\":true"), "{body}");

    // recomputed against that sidecar: bound again
    let o = run(&["impact", e.to_str().unwrap(), "--write", "--resolutions", r.to_str().unwrap()]);
    assert!(o.status.success(), "{}", err(&o));
    let body = freshness("recomputed");
    assert!(!body.contains(drift), "a recomputed impact binds again: {body}");
    let _ = fs::remove_dir_all(d);
}
