// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `/api/gates` — the gates, computed over evidence the server already has.
//!
//! The point of this endpoint is that it adds no new surface: every gate is a
//! pure function of the evidence and the resolutions, both loaded at startup.
//! These tests hold that, and hold that it cannot become a way round the
//! constraints the CLI enforces (#75).
//!
//! Runs only when examples/c has been built; skipped otherwise.

mod common;

use common::Server;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn evidence() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build/evidence.json");
    if p.exists() {
        Some(p)
    } else {
        butterfly_testsupport::fixture_missing("build examples/c and ingest it first");
        None
    }
}

/// A server over `ev` with `extra` flags, once it has answered a request.
///
/// It used to discard the server's stderr and call it ready when a TCP connect
/// succeeded, so a failure could say nothing but `ConnectionReset` (#99). Its
/// port is now its own choice too: a port picked here, by binding and
/// releasing a socket, could still be held by another test's child, and then
/// that child answered the connect while this server died on `Address already
/// in use` -- see `common` (#91).
fn serve(ev: &std::path::Path, extra: &[&str]) -> Server {
    common::serve(Command::new(env!("CARGO_BIN_EXE_butterfly")).arg("serve").arg(ev).args(extra))
}

/// One GET, bearing `bearer` if given. A failed or truncated response fails
/// the test with the server's own account of itself; it no longer reads as
/// status 0 with an empty body.
fn get(srv: &mut Server, path: &str, bearer: Option<&str>) -> (u16, String) {
    let auth = bearer.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.as_deref().map(|a| ("Authorization", a)).into_iter().collect();
    common::request(srv, "GET", path, &headers, "")
}

/// The endpoint and `butterfly check` must never disagree about whether a
/// build passes. They call the same function; this proves they still agree
/// through two very different front doors.
#[test]
fn the_endpoint_and_the_cli_reach_the_same_verdict() {
    let Some(ev) = evidence() else { return };
    let mut srv = serve(&ev, &[]);

    for floor in ["0", "100"] {
        let (code, body) = get(&mut srv, &format!("/api/gates?min_outcomes={floor}"), None);
        assert_eq!(code, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).expect("json");
        let would = v["wouldExit"].as_i64().unwrap();

        let out = Command::new(env!("CARGO_BIN_EXE_butterfly")).args(["check", ev.to_str().unwrap(), "--min-outcomes", floor]).output().expect("check");
        assert_eq!(would, out.status.code().unwrap_or(-1) as i64, "floor {floor}: endpoint said {would}, CLI said {:?}", out.status.code());
    }
}

/// A gate whose threshold is nonsense is worse than no gate, and the viewer
/// must not be the way round the constraint the CLI enforces at the flag.
#[test]
fn a_threshold_outside_zero_to_one_hundred_is_refused() {
    let Some(ev) = evidence() else { return };
    let mut srv = serve(&ev, &[]);
    for q in ["min_lines=-1", "min_lines=101", "min_lines=NaN", "min_outcomes=100.5", "min_mcdc=-0.1"] {
        let (code, body) = get(&mut srv, &format!("/api/gates?{q}"), None);
        assert_eq!(code, 400, "{q} must be refused: {body}");
        assert!(body.contains("percentage between 0 and 100"), "{q}: {body}");
    }
    // and the boundaries are legal
    for q in ["min_lines=0", "min_lines=100"] {
        let (code, _) = get(&mut srv, &format!("/api/gates?{q}"), None);
        assert_eq!(code, 200, "{q} is a valid floor");
    }
}

/// Asking with no floors reports the state and reaches no verdict — the same
/// thing `check` does when given no thresholds.
#[test]
fn with_no_thresholds_it_reports_but_does_not_judge() {
    let Some(ev) = evidence() else { return };
    let mut srv = serve(&ev, &[]);
    let (code, body) = get(&mut srv, "/api/gates", None);
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["thresholdsGiven"], false);
    assert_eq!(v["wouldExit"], 0);
    assert!(v["breaches"].as_array().unwrap().is_empty());
    // it still says what is open, which is the useful half without a gate
    assert!(v["worklistTotal"].as_u64().is_some());
}

/// It shows the command that does the same thing, so the GUI teaches the CLI
/// rather than replacing it.
#[test]
fn it_names_the_command_that_would_do_the_same() {
    let Some(ev) = evidence() else { return };
    let mut srv = serve(&ev, &[]);
    let (_, body) = get(&mut srv, "/api/gates?min_lines=90&max_unresolved_gaps=0", None);
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    let cmd = v["command"].as_str().unwrap();
    assert!(cmd.starts_with("butterfly check"), "{cmd}");
    assert!(cmd.contains("--min-lines 90"), "{cmd}");
    assert!(cmd.contains("--max-unresolved-gaps 0"), "{cmd}");
}

/// This is a read. A viewer serving finished evidence for review must still
/// answer it, and a token-protected one must still demand the token.
#[test]
fn it_is_a_read_route_under_both_postures() {
    let Some(ev) = evidence() else { return };

    let mut ro = serve(&ev, &["--read-only"]);
    let (code, _) = get(&mut ro, "/api/gates?min_lines=1", None);
    assert_eq!(code, 200, "--read-only must not refuse a read");

    const TOKEN: &str = "a-token-at-least-sixteen-characters";
    let mut tok = serve(&ev, &["--token", TOKEN]);
    let (bare, _) = get(&mut tok, "/api/gates", None);
    assert_eq!(bare, 401, "a token server must demand it on this route too");
    let (with, _) = get(&mut tok, "/api/gates", Some(TOKEN));
    assert_eq!(with, 200);
}

/// The command on screen must be the command the verdict describes. The
/// endpoint evaluates with the sidecar this server was started with; a
/// command that omits it would let `check` discover a different file -- or
/// none -- and exit differently than the page said it would (review of #75).
///
/// So: justify every gap in a sidecar with a non-default name, ask the
/// endpoint, then run the exact command string it returned through `sh` and
/// require the exit code it promised.
#[test]
fn the_shown_command_reaches_the_verdict_shown_beside_it() {
    let Some(ev) = evidence() else { return };
    let dir = butterfly_testsupport::scratch_dir("gates-sidecar");

    // justify every gap key the evidence carries, in a file `check` would
    // never discover on its own
    let text = std::fs::read_to_string(&ev).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    let entries: Vec<serde_json::Value> = parsed["functions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| f["object"]["gaps"].as_array().cloned().unwrap_or_default())
        .map(|g| {
            serde_json::json!({ "key": g["key"], "category": "analysis",
                "justification": "test fixture: every gap accounted for", "author": "gates-test", "date": "2026-09-07" })
        })
        .collect();
    assert!(!entries.is_empty(), "the fixture has gaps to justify, or this test proves nothing");
    let sidecar = dir.join("project-reviews.json");
    std::fs::write(&sidecar, serde_json::json!({ "schema": "butterfly-resolutions/1", "entries": entries }).to_string()).unwrap();

    let mut srv = serve(&ev, &["--resolutions", sidecar.to_str().unwrap()]);
    let (st, body) = get(&mut srv, "/api/gates?max_unresolved_gaps=0", None);
    assert_eq!(st, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["wouldExit"], 0, "every gap is justified in the active sidecar: {body}");
    let cmd = v["command"].as_str().unwrap();
    assert!(cmd.contains("--resolutions"), "the command must name the sidecar it was judged with: {cmd}");

    // the exact string, through a real shell, with only PATH supplied
    let bindir = std::path::Path::new(env!("CARGO_BIN_EXE_butterfly")).parent().unwrap();
    let run = |cmd: &str| Command::new("sh").arg("-c").arg(format!("PATH={}:$PATH; {cmd}", bindir.display())).output().expect("sh");
    let out = run(cmd);
    assert_eq!(out.status.code(), Some(0), "the shown command must exit as promised:\n{cmd}\n{}", String::from_utf8_lossy(&out.stderr));

    // and the same command without the sidecar really would have disagreed --
    // this is the wrong answer the finding demonstrated, kept here as proof
    // the flag is what carries the parity
    let stripped = format!("{} --max-unresolved-gaps 0", cmd.split(" --resolutions").next().unwrap());
    let out = run(&stripped);
    assert_eq!(out.status.code(), Some(3), "without the sidecar the verdict differs, which is why it must be named:\n{stripped}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The first-run case, and the mirror of the finding above: the server holds
/// *no* reviews, and the command must hold none either.
///
/// Omitting `--resolutions` does not say "no reviews" -- it says "go and
/// find some", and `check` then discovers whatever `resolutions.json` sits
/// beside the evidence. Justifications the endpoint is counting against the
/// build would silently excuse it, so the page would show a breach while the
/// command it printed exits 0 (review of #75).
#[test]
fn with_no_reviews_the_command_says_so_rather_than_going_looking() {
    let Some(ev) = evidence() else { return };
    let d = butterfly_testsupport::scratch_dir("gates-nores");

    // the evidence, and beside it a default sidecar justifying every gap --
    // the file `check` would discover if the command let it
    let text = fs::read_to_string(&ev).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    let entries: Vec<serde_json::Value> = parsed["functions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| f["object"]["gaps"].as_array().cloned().unwrap_or_default())
        .map(|g| {
            serde_json::json!({ "key": g["key"], "category": "analysis",
             "justification": "an adjacent file the server is not using", "author": "gates-test", "date": "2026-09-08" })
        })
        .collect();
    assert!(!entries.is_empty(), "the fixture has gaps, or this test proves nothing");
    let here = d.join("evidence.json");
    fs::write(&here, &text).unwrap();
    fs::write(d.join("resolutions.json"), serde_json::json!({ "schema": "butterfly-resolutions/1", "entries": entries }).to_string()).unwrap();

    // serve pointed at a sidecar that does not exist: the server has no reviews
    let missing = d.join("not-created-yet.json");
    let mut srv = serve(&here, &["--resolutions", missing.to_str().unwrap()]);
    let (st, body) = get(&mut srv, "/api/gates?max_unresolved_gaps=0", None);
    assert_eq!(st, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["wouldExit"], 3, "no reviews, so every gap is unresolved: {body}");

    let cmd = v["command"].as_str().unwrap();
    assert!(cmd.contains("--no-resolutions"), "it must refuse discovery outright: {cmd}");

    let bindir = std::path::Path::new(env!("CARGO_BIN_EXE_butterfly")).parent().unwrap();
    let run = |c: &str| Command::new("sh").arg("-c").arg(format!("PATH={}:$PATH; {c}", bindir.display())).output().expect("sh");
    assert_eq!(run(cmd).status.code(), Some(3), "the shown command must breach as promised:\n{cmd}");

    // ...and the flagless form really would have disagreed, which is why the
    // flag exists rather than an omission
    let flagless = cmd.replace(" --no-resolutions", "");
    assert_eq!(run(&flagless).status.code(), Some(0), "without it, discovery excuses the build:\n{flagless}");
    let _ = fs::remove_dir_all(&d);
}
