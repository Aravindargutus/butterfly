// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The reported exploit (#66), against the real HTTP write endpoint.
//!
//! `upsert_resolution` used to accept anything with a non-empty key and
//! category, so a POST could turn an unresolved safety-verification
//! obligation into a "resolved" one with a bogus category and blank
//! justification/author/date. This is the endpoint's own regression test
//! the finding asked for: the exact payload must come back 400, and neither
//! the in-memory state nor the sidecar on disk may change because of it.

mod common;

use std::path::PathBuf;
use std::process::Command;

fn evidence() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build/evidence.json");
    if p.exists() {
        Some(p.canonicalize().unwrap())
    } else {
        butterfly_testsupport::fixture_missing("build examples/c first");
        None
    }
}

/// A server, and the scratch directory it runs in -- removed only once the
/// server has stopped.
struct Server {
    srv: common::Server,
    dir: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.srv.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `butterfly serve` over `dir/evidence.json`, run from `dir`, once it has
/// answered a request.
///
/// Its port is its own choice and its stderr is kept. These servers were
/// handed a port picked by binding and releasing a socket, and called ready
/// when a TCP connect succeeded -- which a listener inherited by another
/// test's child would also satisfy, while this server died on `Address
/// already in use` (#91, #99; see `common`).
fn start(dir: PathBuf) -> Server {
    let srv = common::serve(
        Command::new(env!("CARGO_BIN_EXE_butterfly"))
            .arg("serve")
            .arg(dir.join("evidence.json"))
            // The default --resolutions is the relative name "resolutions.json",
            // resolved against the process's cwd -- not the evidence's directory.
            // Without this, the sidecar a test just wrote lands in whatever
            // directory happened to be cargo test's cwd (crates/cli, in
            // practice), not the scratch directory this test cleans up.
            .current_dir(&dir),
    );
    Server { srv, dir }
}

fn serve(tag: &str) -> Option<Server> {
    let ev = evidence()?;
    // One directory per call: cargo runs the tests in this binary
    // concurrently, and a shared directory means one test's teardown
    // (`remove_dir_all`) races another's still-active server -- a
    // test-isolation bug, not a product one, and worth naming so it does not
    // get relearned.
    let dir = butterfly_testsupport::scratch_dir(&format!("res-api-{tag}"));
    let local_ev = dir.join("evidence.json");
    std::fs::copy(&ev, &local_ev).unwrap();

    Some(start(dir))
}

/// One request with a JSON body. Returns (status, body); a failed or
/// truncated response fails with the server's own account of itself.
fn request(srv: &mut Server, method: &str, path: &str, json: &str) -> (u16, String) {
    common::request(&mut srv.srv, method, path, &[("Content-Type", "application/json")], json)
}

/// The literal payload from the finding: a real gap key
/// (`_phase_name@line-zero@1000005d4` in the original report; here, `fn:0`,
/// which any evidence with at least one function has, so this test does not
/// depend on exactly which gaps this build happens to produce), an
/// unrecognised category, and blank justification/author/date.
const EXPLOIT: &str = r#"{"key":"fn:0","category":"totally-invalid","justification":"","author":"","date":""}"#;

#[test]
fn the_exploit_payload_is_refused_and_changes_nothing() {
    let Some(mut srv) = serve("exploit") else { return };

    let (status, body) = request(&mut srv, "POST", "/api/resolutions", EXPLOIT);
    assert_eq!(status, 400, "the exploit record must be refused: {body}");
    assert!(body.contains("category"), "the error should say what is wrong: {body}");

    // memory: a GET must show no such entry
    let (status, body) = request(&mut srv, "GET", "/api/resolutions", "");
    assert_eq!(status, 200, "{body}");
    assert!(!body.contains("totally-invalid"), "the rejected record must not be in memory: {body}");

    // disk: no sidecar was ever created by the rejected write
    let sidecar = srv.dir.join("resolutions.json");
    assert!(!sidecar.exists(), "a rejected write must not create the sidecar file at all");
}

/// The same key, every other field well-formed: proves the test above is
/// rejecting the invalidity, not merely refusing every POST.
#[test]
fn a_well_formed_record_over_the_same_key_is_accepted_and_persisted() {
    let Some(mut srv) = serve("wellformed") else { return };
    let good = r#"{"key":"fn:0","category":"a","justification":"covered by analysis","author":"me","date":"2026-01-01"}"#;

    let (status, body) = request(&mut srv, "POST", "/api/resolutions", good);
    assert_eq!(status, 200, "{body}");

    let (status, body) = request(&mut srv, "GET", "/api/resolutions", "");
    assert_eq!(status, 200);
    assert!(body.contains("\"fn:0\""), "{body}");

    let sidecar = srv.dir.join("resolutions.json");
    assert!(sidecar.exists(), "a valid write must persist");
    let on_disk = std::fs::read_to_string(&sidecar).unwrap();
    assert!(on_disk.contains("\"fn:0\""), "{on_disk}");
}

/// A key that is shaped exactly like a real one but names nothing the
/// evidence has -- the "maps to an existing evidence obligation" half of the
/// finding, exercised over HTTP rather than as a unit test.
#[test]
fn a_key_naming_no_real_obligation_is_refused() {
    let Some(mut srv) = serve("nowhere") else { return };
    let nowhere = r#"{"key":"fn:999999","category":"a","justification":"j","author":"me","date":"2026-01-01"}"#;

    let (status, body) = request(&mut srv, "POST", "/api/resolutions", nowhere);
    assert_eq!(status, 400, "{body}");
    assert!(!srv.dir.join("resolutions.json").exists());
}

/// Two stale-but-well-formed reviews on file, as any rebuild produces. The
/// viewer must still work: a new review saves, and the stale ones can be
/// deleted one at a time to clean up.
///
/// Checking obligation-membership over every surviving entry instead of just
/// the record being written made the viewer read-only exactly when a reviewer
/// needed it most -- the save refused on the first stale key, and the
/// deletion that would have fixed it refused on the second (review of #66).
#[test]
fn stale_history_does_not_freeze_the_viewer() {
    let Some(mut srv) = serve_with_stale("frozen") else { return };

    // a new, valid review saves despite the stale entries beside it
    let good = r#"{"key":"fn:0","category":"a","justification":"reviewed after the rebuild","author":"me","date":"2026-09-09"}"#;
    let (status, body) = request(&mut srv, "POST", "/api/resolutions", good);
    assert_eq!(status, 200, "a valid review must save while stale history sits beside it: {body}");
    assert!(body.contains("\"fn:0\""), "{body}");

    // and each stale entry can be removed, one at a time
    for k in ["_gone@line-zero@dead1", "_gone@line-zero@dead2"] {
        let (status, body) = request(&mut srv, "POST", "/api/resolutions/delete", &format!(r#"{{"key":"{k}"}}"#));
        assert_eq!(status, 200, "deleting stale {k} must be allowed: {body}");
        assert!(!body.contains(k), "and it is gone: {body}");
    }

    // the real review survived the cleanup
    let (_, body) = request(&mut srv, "GET", "/api/resolutions", "");
    assert!(body.contains("\"fn:0\""), "{body}");
    assert!(!body.contains("_gone@"), "{body}");
}

/// ...but a *new* record naming nothing is still refused: an unmatched key
/// cannot be staleness when the client derived it from this very evidence.
#[test]
fn a_new_record_naming_nothing_is_still_refused_even_beside_stale_history() {
    let Some(mut srv) = serve_with_stale("newbogus") else { return };
    let bogus = r#"{"key":"fn:999999","category":"a","justification":"j","author":"me","date":"2026-09-09"}"#;
    let (status, body) = request(&mut srv, "POST", "/api/resolutions", bogus);
    assert_eq!(status, 400, "{body}");
}

/// A server whose sidecar already holds two stale reviews -- well-formed in
/// every field, naming gaps this evidence does not have, exactly what a
/// rebuild leaves behind.
fn serve_with_stale(tag: &str) -> Option<Server> {
    let ev = evidence()?;
    let dir = butterfly_testsupport::scratch_dir(&format!("res-stale-{tag}"));
    let local_ev = dir.join("evidence.json");
    std::fs::copy(&ev, &local_ev).unwrap();
    std::fs::write(
        dir.join("resolutions.json"),
        serde_json::json!({"schema":"butterfly-resolutions/1","entries":[
            {"key":"_gone@line-zero@dead1","category":"analysis","justification":"reviewed before the rebuild","author":"me","date":"2026-09-01"},
            {"key":"_gone@line-zero@dead2","category":"analysis","justification":"reviewed before the rebuild","author":"me","date":"2026-09-01"}]})
        .to_string(),
    )
    .unwrap();

    Some(start(dir))
}

/// A gcov-shaped review, over the real endpoint: saved, persisted, and read
/// back under the very key the model generates for it.
///
/// gcov gives no columns, so a decision is identified by an ordinal and its
/// outcomes are `A`/`B` rather than `T`/`F`. The membership check parsed the
/// key itself and demanded a numeric column and a T/F outcome, so a
/// reviewer working on a GCC build had every save refused as naming no
/// obligation (#88).
///
/// The branch is synthesised from the clang fixture rather than taken from
/// `examples/c-gcc`, so this runs on every host CI builds on -- the GCC
/// fixture is Linux-only (#89), and a test that silently skipped there
/// would prove nothing on the machine most reviewers use.
#[test]
fn a_gcov_style_key_is_accepted_over_the_api_and_persists() {
    let Some((mut srv, key)) = serve_with_gcov_branch("gcov") else { return };

    let good = format!(r#"{{"key":"{key}","category":"analysis","justification":"unreachable on this target","author":"me","date":"2026-09-10"}}"#);
    let (status, body) = request(&mut srv, "POST", "/api/resolutions", &good);
    assert_eq!(status, 200, "a key the model itself generates must be accepted: {body}");

    let (status, body) = request(&mut srv, "GET", "/api/resolutions", "");
    assert_eq!(status, 200);
    assert!(body.contains(&format!("\"{key}\"")), "it must read back: {body}");

    let on_disk = std::fs::read_to_string(srv.dir.join("resolutions.json")).expect("sidecar written");
    assert!(on_disk.contains(&key), "and persist: {on_disk}");

    // The polarity must not be upgraded: gcov never said which edge is true.
    let t_key = key.replace(":B", ":T");
    let upgraded = format!(r#"{{"key":"{t_key}","category":"analysis","justification":"j","author":"me","date":"2026-09-10"}}"#);
    let (status, body) = request(&mut srv, "POST", "/api/resolutions", &upgraded);
    assert_eq!(status, 400, "a T/F key against an unknown-polarity branch asserts what nothing supports: {body}");
}

/// The clang fixture with one branch rewritten into what gcov produces --
/// no column, an ordinal, and unknown polarity. Returns the server and the
/// key the model generates for that branch's second outcome.
fn serve_with_gcov_branch(tag: &str) -> Option<(Server, String)> {
    let ev = evidence()?;
    let dir = butterfly_testsupport::scratch_dir(&format!("res-gcov-{tag}"));

    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&ev).unwrap()).unwrap();
    let mut key = None;
    for f in doc["functions"].as_array_mut().unwrap() {
        let fid = f["id"].as_u64().unwrap();
        let Some(b) = f["branches"].as_array_mut().and_then(|bs| bs.first_mut()) else { continue };
        b["col"] = serde_json::json!(0);
        b["directionKnown"] = serde_json::json!(false);
        b["decisionOrdinal"] = serde_json::json!(0);
        key = Some(format!("branch:{fid}:{}:#0:B", b["line"].as_u64().unwrap()));
        break;
    }
    let key = key.expect("the fixture has at least one branch");
    let local_ev = dir.join("evidence.json");
    std::fs::write(&local_ev, serde_json::to_string(&doc).unwrap()).unwrap();

    Some((start(dir), key))
}
