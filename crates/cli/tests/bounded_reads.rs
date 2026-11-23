// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The size caps are not an `ingest` feature (#231).
//!
//! `ingest` was thorough -- every read there goes through
//! `limits::read_bounded{,_string}` -- and the README, the manual and the
//! showcase all say the same absolute about *every* untrusted artifact. Five
//! reads outside those paths did not honour it: `tags` and
//! `claims --requirements` on the CLI, `build`'s config, the viewer's
//! re-hash of each requirement document, and `/api/freshness`, which read
//! every analysed source at whatever size the evidence named it, on a live
//! HTTP route.
//!
//! One test per path, each one an oversized artifact against a lowered cap.
//! Each is paired with the same file under the default cap, so a test cannot
//! pass because the command failed for some other reason.
//!
//! Nothing here needs a built example: every fixture is written by the test.
//! A suite about refusing input should not be able to skip.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MIB: usize = 1024 * 1024;

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("bounded-{tag}"))
}

fn run(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().expect("spawn butterfly")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A valid, empty evidence document. Written rather than ingested so this
/// suite has no fixture to be missing.
fn evidence(dir: &Path) -> PathBuf {
    let ev = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
    ev.validate().expect("the empty document is a valid one");
    let p = dir.join("evidence.json");
    std::fs::write(&p, serde_json::to_string(&ev).unwrap()).unwrap();
    p
}

/// `text` padded past `mb` with spaces inserted after the first byte, so the
/// result is still exactly the document it was -- the refusal has to be on
/// size, not on content.
fn padded_json(path: &Path, text: &str, mb: usize) {
    std::fs::write(path, format!("{}{}{}", &text[..1], " ".repeat(mb * MIB), &text[1..])).unwrap();
}

/// `butterfly tags` read the whole evidence file with `fs::read_to_string`
/// while nine other evidence reads in the same file went through the capped
/// helper beside it.
#[test]
fn tags_refuses_an_oversized_evidence_file() {
    let d = scratch("tags");
    let ev = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
    let big = d.join("big-evidence.json");
    padded_json(&big, &serde_json::to_string(&ev).unwrap(), 2);
    let args = &["tags", big.to_str().unwrap(), "--source-root", d.to_str().unwrap()];

    let o = run(args, &[("BUTTERFLY_MAX_EVIDENCE_MB", "1")]);
    let e = stderr(&o);
    assert!(!o.status.success(), "2 MiB against a 1 MiB cap must be refused: {e}");
    assert!(e.contains("evidence file"), "the refusal says what it was reading: {e}");
    assert!(e.contains("BUTTERFLY_MAX_EVIDENCE_MB"), "and which variable would have let it through: {e}");

    // the control: under the default cap the same file gets past the read,
    // so the assertions above are about the cap and not about `tags` always
    // failing on this document
    let e = stderr(&run(args, &[]));
    assert!(!e.contains("BUTTERFLY_MAX_EVIDENCE_MB"), "2 MiB is far within the 512 MiB default: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// `butterfly claims --requirements` read the sidecar uncapped, next to an
/// evidence read that was capped.
#[test]
fn claims_refuses_an_oversized_requirements_sidecar() {
    let d = scratch("claims");
    let ev = evidence(&d);
    let reqs = butterfly_model::reqtext::Requirements { schema: butterfly_model::reqtext::REQTEXT_SCHEMA.into(), ..Default::default() };
    let big = d.join("big-requirements.json");
    padded_json(&big, &serde_json::to_string(&reqs).unwrap(), 2);
    let args = &["claims", ev.to_str().unwrap(), "--requirements", big.to_str().unwrap()];

    let o = run(args, &[("BUTTERFLY_MAX_AUX_MB", "1")]);
    let e = stderr(&o);
    assert!(!o.status.success(), "2 MiB against a 1 MiB cap must be refused: {e}");
    assert!(e.contains("BUTTERFLY_MAX_AUX_MB"), "naming the variable that would have let it through: {e}");

    // the control: the same sidecar under the default cap is read and the
    // command succeeds, so the refusal above was the cap's doing
    let o = run(args, &[]);
    assert!(o.status.success(), "under the default cap the same file is fine: {}", stderr(&o));
    let _ = std::fs::remove_dir_all(d);
}

/// `butterfly build` read its config with `fs::read_to_string` while
/// `ingest` read `butterfly.toml` -- the same kind of file -- under the cap.
#[test]
fn build_refuses_an_oversized_config() {
    let d = scratch("build");
    // A config that is valid TOML and names a compiler that does not exist,
    // so the control run fails at the spawn rather than by compiling
    // anything.
    let cfg = "[project]\nname = \"t\"\nsource_root = \".\"\nout_dir = \"out\"\n\n[compiler]\nkind = \"clang\"\ncc = \"butterfly-no-such-cc\"\nsources = [\"nope.c\"]\n\n[coverage]\nflags = []\nrun = [\"{exe}\"]\n\n[[variant]]\nname = \"O0\"\nflags = [\"-O0\"]\n";
    let big = d.join("butterfly-build.toml");
    // padded with comment lines, which TOML ignores
    std::fs::write(&big, format!("{}{cfg}", "# pad\n".repeat(2 * MIB / 6))).unwrap();
    let args = &["build", big.to_str().unwrap()];

    let o = run(args, &[("BUTTERFLY_MAX_AUX_MB", "1")]);
    let e = stderr(&o);
    assert!(!o.status.success(), "2 MiB against a 1 MiB cap must be refused: {e}");
    assert!(e.contains("BUTTERFLY_MAX_AUX_MB"), "naming the variable that would have let it through: {e}");

    // the control: under the default cap the config is read and parsed, and
    // the build fails later, on the compiler
    let e = stderr(&run(args, &[]));
    assert!(!e.contains("BUTTERFLY_MAX_AUX_MB"), "2 MiB is far within the 64 MiB default: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// `butterfly serve` re-hashes every requirement document at startup to say
/// whether the text on screen is still the text that was baselined. That
/// read was uncapped, and the paths come out of the sidecar.
#[test]
fn serve_refuses_an_oversized_requirement_document() {
    let d = scratch("reqdoc");
    let ev = evidence(&d);
    let doc = d.join("llr.md");
    std::fs::write(&doc, "x".repeat(2 * MIB)).unwrap();
    let reqs = butterfly_model::reqtext::Requirements {
        schema: butterfly_model::reqtext::REQTEXT_SCHEMA.into(),
        sources: vec![butterfly_model::reqtext::Source { path: doc.display().to_string(), sha256: "0".repeat(64), count: 0 }],
        ..Default::default()
    };
    std::fs::write(d.join("requirements.json"), serde_json::to_string(&reqs).unwrap()).unwrap();

    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.arg("serve").arg(&ev).arg("--source-root").arg(&d).env("BUTTERFLY_MAX_AUX_MB", "1");
    let mut srv = common::serve(&mut c);
    let log = srv.stderr();
    srv.stop();
    assert!(log.contains("BUTTERFLY_MAX_AUX_MB"), "the refused document names the cap: {log}");
    assert!(log.contains("was not checked"), "and says the staleness check did not happen: {log}");

    // the control: under the default cap the document is read, and since its
    // hash is not the recorded one the viewer says so instead
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.arg("serve").arg(&ev).arg("--source-root").arg(&d);
    let mut srv = common::serve(&mut c);
    let log = srv.stderr();
    srv.stop();
    assert!(!log.contains("BUTTERFLY_MAX_AUX_MB"), "2 MiB is far within the 64 MiB default: {log}");
    assert!(log.contains("has changed since the requirement text was read"), "it was read, and compared: {log}");
    let _ = std::fs::remove_dir_all(d);
}

/// `/api/freshness` hashes every analysed source on every request, at
/// whatever size the evidence names. This is the only one of the five on a
/// live HTTP route.
#[test]
fn the_freshness_route_refuses_an_oversized_analysed_source() {
    let d = scratch("freshness");
    let src = d.join("big.c");
    std::fs::write(&src, "x".repeat(2 * MIB)).unwrap();
    let ev = butterfly_model::Evidence {
        schema: butterfly_model::SCHEMA.into(),
        files: vec![butterfly_model::SourceFile { id: 0, path: src.display().to_string(), display: "big.c".into(), sha256: "0".repeat(64), ..Default::default() }],
        // The one empty file record derives files=1 with no lines or functions;
        // the summary must say so, or the derivation gate serve runs on load
        // (#255) rejects it before the freshness route is ever reached.
        summary: butterfly_model::Summary { files: 1, ..Default::default() },
        changes: Some(butterfly_model::Changes { repo: d.display().to_string(), head: "0".repeat(40), ..Default::default() }),
        ..Default::default()
    };
    ev.validate().expect("a one-file document with no coverage is valid");
    ev.check_summary_derivation().expect("the summary matches its one-file records");
    let p = d.join("evidence.json");
    std::fs::write(&p, serde_json::to_string(&ev).unwrap()).unwrap();

    // the source's own directory is the approved tree: outside it the
    // viewer would not read the file at all (#248)
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.arg("serve").arg(&p).arg("--source-root").arg(&d).env("BUTTERFLY_MAX_SOURCE_MB", "1");
    let mut srv = common::serve(&mut c);
    let (code, body) = common::get(&mut srv, "/api/freshness");
    srv.stop();
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("BUTTERFLY_MAX_SOURCE_MB"), "the source is refused by size, on the route: {body}");
    assert!(body.contains("sourcesUnreadable"), "and reported as unread rather than as matching: {body}");
    assert!(!body.contains("\"sourcesModified\":[\"big.c\"]"), "a file that was never read is not a file that differs: {body}");

    // the control: under the default cap the file is read and hashed, and
    // its hash is not the recorded one, so it comes back as modified
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.arg("serve").arg(&p).arg("--source-root").arg(&d);
    let mut srv = common::serve(&mut c);
    let (code, body) = common::get(&mut srv, "/api/freshness");
    srv.stop();
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("\"sourcesModified\":[\"big.c\"]"), "2 MiB is far within the 64 MiB default: {body}");
    let _ = std::fs::remove_dir_all(d);
}

/// One policy, every door (#246). The same over-budget evidence file is
/// refused, with the same variable named, by every subcommand that takes an
/// evidence file -- before anything is parsed or served. A subcommand that
/// read it some other way would be the bypass #231 was about.
#[test]
fn every_subcommand_that_reads_evidence_refuses_the_same_oversized_file() {
    let d = scratch("every");
    let ev = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
    let big = d.join("big-evidence.json");
    padded_json(&big, &serde_json::to_string(&ev).unwrap(), 2);
    let reqs = d.join("requirements.json");
    std::fs::write(&reqs, "{}").unwrap();
    let b = big.to_str().unwrap();
    let out = |name: &str| d.join(name).display().to_string();
    let cases: Vec<Vec<String>> = vec![
        vec!["summary".into(), b.into()],
        vec!["report".into(), b.into(), "-o".into(), out("r.html")],
        vec!["baseline".into(), b.into(), "-o".into(), out("b.json")],
        vec!["changes".into(), b.into(), "-o".into(), out("c.json"), "--cache".into(), out("cache.json")],
        vec!["impact".into(), b.into()],
        vec!["check".into(), b.into()],
        vec!["claims".into(), b.into(), "-r".into(), reqs.display().to_string()],
        vec!["tags".into(), b.into(), "--source-root".into(), d.display().to_string()],
        vec!["serve".into(), b.into(), "--listen".into(), "127.0.0.1:0".into()],
    ];
    for args in &cases {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let o = run(&a, &[("BUTTERFLY_MAX_EVIDENCE_MB", "1")]);
        let e = stderr(&o);
        assert!(!o.status.success(), "`butterfly {}` accepted 2 MiB against a 1 MiB cap: {e}", args[0]);
        assert!(e.contains("BUTTERFLY_MAX_EVIDENCE_MB"), "`butterfly {}` refused it some other way: {e}", args[0]);
    }
    let _ = std::fs::remove_dir_all(d);
}
