// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! What `butterfly serve` does with a definition index it should not trust.
//!
//! The sidecar sits beside the evidence and is served verbatim to the
//! browser, so it is untrusted on the same footing as everything else. Every
//! bad case must end the same way -- the index is dropped, the reason is on
//! stderr, and the viewer still serves the evidence -- because the index is a
//! navigation aid and must never be able to stop the tool (review of #32).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

/// Ingest the C example into a scratch directory.
///
/// Deliberately *not* `examples/c/build/evidence.json`: that file is not a
/// build artifact -- `build.sh` clears the directory and never writes it --
/// so depending on it makes every test here skip while still reporting
/// "ok", which is the failure mode these tests exist to catch in the
/// product.
fn scratch(tag: &str) -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c/build.sh first");
        return None;
    }
    let d = butterfly_testsupport::scratch_dir(&format!("tags-{tag}"));
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root.clone());
    let ev = butterfly_ingest::ingest(&o).expect("ingest");
    std::fs::write(d.join("evidence.json"), serde_json::to_string(&ev).unwrap()).unwrap();
    Some(d)
}

/// Start the server against `dir/evidence.json`, collect stderr up to the
/// listening banner, stop it. Port 0 so no free port is needed.
fn banner(dir: &Path) -> String {
    banner_with(dir, &[])
}

/// The same, with environment for the child -- the resource caps are read
/// from the environment, so an oversized case is set up by lowering the cap
/// rather than by writing a file large enough to hit the default.
fn banner_with(dir: &Path, env: &[(&str, &str)]) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child =
        cmd.arg("serve").arg(dir.join("evidence.json")).args(["--listen", "127.0.0.1:0"]).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().expect("spawn butterfly serve");
    let err = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = String::new();
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            // the URL is the last line of the banner, and the variant
            // lines -- including anything said about the sidecar -- come
            // after "listening on", so stopping there would miss them
            let done = line.contains("http://");
            seen.push_str(&line);
            seen.push('\n');
            if done {
                break;
            }
        }
        let _ = tx.send(seen);
    });
    let out = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap_or_default();
    let _ = child.kill();
    let _ = child.wait();
    out
}

fn write_tags(dir: &Path, body: &str) {
    std::fs::write(dir.join("evidence.tags.json"), body).unwrap();
}

/// A sidecar larger than the ceiling is refused on its size, before it is
/// parsed -- the point being that it is never read whole into memory.
#[test]
fn an_oversized_sidecar_is_refused_before_it_is_parsed() {
    let Some(d) = scratch("big") else { return };
    // 2 MiB of valid JSON against a 1 MiB auxiliary cap. Lowering the cap
    // beats writing a file big enough to hit the default: same code path,
    // no 70 MiB of I/O in the suite.
    let pad = "x".repeat(2 * 1024 * 1024);
    write_tags(&d, &format!(r#"{{"schema":"butterfly-tags/1","generator":"{pad}","evidenceSha256":"","entries":[]}}"#));
    let b = banner_with(&d, &[("BUTTERFLY_MAX_AUX_MB", "1")]);
    assert!(b.contains("BUTTERFLY_MAX_AUX_MB"), "refused on size, naming the cap to raise: {b}");
    assert!(b.contains("no definition jumps"), "and the viewer keeps serving: {b}");
    assert!(b.contains("listening on"), "the server still came up: {b}");
    let _ = std::fs::remove_dir_all(&d);
}

/// An id past the end of the evidence's arrays would reach the browser and
/// throw when the reader hovered the identifier carrying it.
#[test]
fn an_out_of_range_id_is_refused() {
    let Some(d) = scratch("range") else { return };
    write_tags(
        &d,
        r#"{"schema":"butterfly-tags/1","generator":"t","evidenceSha256":"","entries":[
             {"name":"x","kind":"function","language":"C","file":"demo.c","line":1,"end":1,"fileId":9999,"functionId":null,"scope":null}]}"#,
    );
    let b = banner(&d);
    assert!(b.contains("no definition jumps"), "dropped: {b}");
    assert!(b.contains("listening on"), "the server still came up: {b}");
    let _ = std::fs::remove_dir_all(&d);
}

/// An index with no evidence hash was not written by `butterfly tags`, which
/// always writes one. Treating absent as "matches" let an index built against
/// different evidence route go-to-definition into unrelated code.
#[test]
fn an_empty_evidence_hash_is_not_a_match() {
    let Some(d) = scratch("nohash") else { return };
    write_tags(&d, r#"{"schema":"butterfly-tags/1","generator":"t","evidenceSha256":"","entries":[]}"#);
    let b = banner(&d);
    assert!(b.contains("carries no evidence hash"), "says why: {b}");
    assert!(b.contains("no definition jumps"), "and drops it: {b}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A hash that is present but wrong is refused with the other message.
#[test]
fn a_sidecar_for_other_evidence_is_refused() {
    let Some(d) = scratch("otherev") else { return };
    write_tags(&d, &format!(r#"{{"schema":"butterfly-tags/1","generator":"t","evidenceSha256":"{}","entries":[]}}"#, "a".repeat(64)));
    let b = banner(&d);
    assert!(b.contains("built against different evidence"), "says why: {b}");
    let _ = std::fs::remove_dir_all(&d);
}
