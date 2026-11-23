// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The report carries its requirement statements (#56).
//!
//! Unlike the viewer it is read away from the tool -- it is what a
//! Compliance Verification Engineer signs against -- so it cannot fetch a
//! statement on demand and must embed what it shows.
//!
//! Runs only when examples/c-per-test has been built; skipped otherwise.

use std::path::PathBuf;
use std::process::Command;

fn build(tag: &str) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-per-test");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c-per-test/build.sh first");
        return None;
    }
    let d = butterfly_testsupport::scratch_dir(&format!("rptreq-{tag}"));
    let bin = env!("CARGO_BIN_EXE_butterfly");
    let (ev, req) = (d.join("evidence.json"), d.join("requirements.json"));

    let o = Command::new(bin)
        .args([
            "ingest",
            root.join("build").to_str().unwrap(),
            "--source-root",
            root.to_str().unwrap(),
            "--tests",
            root.join("build/tests").to_str().unwrap(),
            "--requirements",
            root.join("requirements.txt").to_str().unwrap(),
            "-o",
            ev.to_str().unwrap(),
        ])
        .output()
        .expect("ingest");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = Command::new(bin).args(["requirements", root.join("llr.yaml").to_str().unwrap(), "-o", req.to_str().unwrap()]).output().expect("requirements");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    Some((d, ev, req))
}

fn report(d: &std::path::Path, ev: &std::path::Path, extra: &[&str]) -> String {
    let out = d.join("report.html");
    let mut args = vec!["report", ev.to_str().unwrap(), "-o", out.to_str().unwrap()];
    args.extend_from_slice(extra);
    let o = Command::new(env!("CARGO_BIN_EXE_butterfly")).args(&args).output().expect("report");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    std::fs::read_to_string(&out).unwrap()
}

/// The statement is in the file, not a lookup the reader has to perform.
#[test]
fn the_report_quotes_each_requirement_and_names_its_baseline() {
    let Some((d, ev, _)) = build("quote") else { return };
    let html = report(&d, &ev, &[]);

    assert!(html.contains("An altitude presented for limiting"), "LLR-LIM-001's statement is embedded");
    assert!(html.contains("A climb rate shall be accepted"), "and LLR-LIM-002's");
    // the baseline the text was quoted from, by digest
    assert!(html.contains("llr.yaml"), "names the source document");
    assert!(html.contains("quoted from"), "and says the text is quoted: {}", &html[..0]);
    // derived is called out, because 5.1.2.h wants it in front of the safety
    // assessment and nothing else in the report would say so
    assert!(html.contains("derived") && html.contains("5.1.2.h"), "derived requirements are flagged");
    let _ = std::fs::remove_dir_all(&d);
}

/// `planned` is the expected state of something unbuilt. Reporting it like an
/// `implemented` requirement with no code would bury the real findings --
/// most of a real programme is planned.
#[test]
fn a_planned_requirement_with_no_code_is_not_reported_as_a_fault() {
    let Some((d, ev, _)) = build("planned") else { return };
    let html = report(&d, &ev, &[]);
    assert!(html.contains("planned, so none is expected yet"), "LLR-LIM-004 is planned: {}", &html[..0]);
    let _ = std::fs::remove_dir_all(&d);
}

/// Without the sidecar the report still renders, with IDs alone.
#[test]
fn without_requirement_text_the_report_still_renders() {
    let Some((d, ev, req)) = build("bare") else { return };
    std::fs::remove_file(&req).unwrap();
    let html = report(&d, &ev, &[]);
    assert!(html.contains("LLR-LIM-001"), "the matrix is still there");
    assert!(!html.contains("An altitude presented for limiting"), "but no statements");
    assert!(!html.contains("quoted from"), "and no baseline claim it cannot support");
    let _ = std::fs::remove_dir_all(&d);
}

/// A statement the evidence never references is the other direction of
/// "unmatched", and has to be visible too.
#[test]
fn a_requirement_the_evidence_does_not_reference_is_named() {
    let Some((d, ev, req)) = build("orphan") else { return };
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&req).unwrap()).unwrap();
    doc["entries"].as_array_mut().unwrap().push(serde_json::json!({
        "id": "LLR-ZZZ-999", "text": "Implemented in a sibling component.", "source": 0
    }));
    std::fs::write(&req, serde_json::to_string(&doc).unwrap()).unwrap();

    let html = report(&d, &ev, &[]);
    assert!(html.contains("LLR-ZZZ-999"), "named");
    assert!(html.contains("not referenced by this evidence"), "and explained");
    let _ = std::fs::remove_dir_all(&d);
}

/// An explicitly named sidecar that will not load must fail, not silently
/// produce a report without the text that was asked for.
#[test]
fn an_explicitly_named_sidecar_that_cannot_load_fails_the_command() {
    let Some((d, ev, _)) = build("bad") else { return };
    let bad = d.join("bad.json");
    std::fs::write(&bad, "{ not json").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_butterfly"))
        .args(["report", ev.to_str().unwrap(), "--requirements", bad.to_str().unwrap(), "-o", d.join("r.html").to_str().unwrap()])
        .output()
        .expect("report");
    assert!(!o.status.success(), "asking for text and not getting it is not a success");
    let _ = std::fs::remove_dir_all(&d);
}

/// A sidecar is someone else's bytes, and the report path must hold the same
/// auxiliary-input boundary as every other reader (review of #56).
#[test]
fn an_oversized_sidecar_is_refused_by_the_auxiliary_cap() {
    let Some((d, ev, req)) = build("big") else { return };
    // comfortably past a 1 MB cap. (`0` is treated as unset by `from_env`,
    // so the cap has to be a real number for this to mean anything.)
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&req).unwrap()).unwrap();
    doc["generator"] = serde_json::json!("x".repeat(2 * 1024 * 1024));
    std::fs::write(&req, serde_json::to_string(&doc).unwrap()).unwrap();
    assert!(std::fs::metadata(&req).unwrap().len() > 1024 * 1024, "the fixture must exceed the cap");

    let o = Command::new(env!("CARGO_BIN_EXE_butterfly"))
        .args(["report", ev.to_str().unwrap(), "-o", d.join("r.html").to_str().unwrap()])
        .env("BUTTERFLY_MAX_AUX_MB", "1")
        .output()
        .expect("report");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "an oversized sidecar is refused, not read: {err}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A dangling sidecar symlink is a broken baseline, not an absent one.
///
/// `exists()` follows the link and answers false, which would quietly turn
/// "the baseline cannot be read" into "there is no baseline" and hand back a
/// complete-looking report without it. That is precisely the silent failure
/// this feature exists to prevent.
#[cfg(unix)]
#[test]
fn a_dangling_sidecar_symlink_is_an_error_not_an_absence() {
    let Some((d, ev, req)) = build("dangle") else { return };
    std::fs::remove_file(&req).unwrap();
    std::os::unix::fs::symlink(d.join("nowhere.json"), &req).unwrap();

    let o = Command::new(env!("CARGO_BIN_EXE_butterfly")).args(["report", ev.to_str().unwrap(), "-o", d.join("r.html").to_str().unwrap()]).output().expect("report");
    assert!(!o.status.success(), "a broken baseline is reported, not skipped");
    let _ = std::fs::remove_dir_all(&d);
}

/// With several documents, each statement says which one it came from --
/// "one of these digests" cannot tell a reviewer which baseline backs the
/// sentence in front of them.
#[test]
fn with_two_documents_each_statement_names_its_own_source() {
    let Some((d, ev, req)) = build("multi") else { return };
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&req).unwrap()).unwrap();
    // a second document, and one entry attributed to it
    doc["sources"].as_array_mut().unwrap().push(serde_json::json!({
        "path": "other/llr.yaml", "sha256": "b".repeat(64), "count": 1
    }));
    for e in doc["entries"].as_array_mut().unwrap() {
        if e["id"] == "LLR-LIM-003" {
            e["source"] = serde_json::json!(1);
        }
    }
    std::fs::write(&req, serde_json::to_string(&doc).unwrap()).unwrap();

    let html = report(&d, &ev, &[]);
    assert!(html.contains("quoted from 2 documents"), "the header defers to the rows: {}", &html[..0]);
    assert!(html.contains("other/llr.yaml"), "the second document is named on its statement");
    assert!(html.contains("bbbbbbbbbbbb"), "with its digest");
    assert!(html.contains("llr.yaml"), "and the first is still named on its own");
    let _ = std::fs::remove_dir_all(&d);
}
