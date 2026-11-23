// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Evidence with an impossible source range must be refused, not iterated
//! (review of #42).
//!
//! The requirements matrix walks a function's line range one line at a time,
//! so line zero underflows the 1-based conversion and an oversized end is a
//! loop count chosen by whoever wrote the file. Both `summary` and `report`
//! consume it, so both are driven here.

use std::path::PathBuf;
use std::process::Command;

fn doctored(tag: &str, mutate: impl Fn(&mut serde_json::Value)) -> Option<(PathBuf, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-per-test");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c-per-test/build.sh first");
        return None;
    }
    let d = butterfly_testsupport::scratch_dir(&format!("badrange-{tag}"));
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root);
    let ev = butterfly_ingest::ingest(&o).expect("ingest");
    let mut doc: serde_json::Value = serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
    let name = doc["functions"][0]["name"].as_str().unwrap().to_string();
    mutate(&mut doc["functions"][0]);
    std::fs::write(d.join("evidence.json"), serde_json::to_string(&doc).unwrap()).unwrap();
    Some((d, name))
}

fn run(cmd: &str, dir: &std::path::Path) -> (i32, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.arg(cmd).arg(dir.join("evidence.json"));
    if cmd == "report" {
        c.args(["-o", dir.join("r.html").to_str().unwrap()]);
    }
    let out = c.output().expect("run butterfly");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn line_zero_is_refused_by_every_consumer() {
    let Some((d, name)) = doctored("zero", |f| {
        f["lineStart"] = serde_json::json!(0);
        f["lineEnd"] = serde_json::json!(0);
    }) else {
        return;
    };
    for cmd in ["summary", "report"] {
        let (code, err) = run(cmd, &d);
        assert_ne!(code, 0, "{cmd} refuses it");
        assert!(!err.contains("panicked"), "{cmd} with a sentence, not an underflow: {err}");
        assert!(err.contains("source range") && err.contains(&name), "{cmd}: {err}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// An end past the file is a loop count the document chose. Refused at the
/// door, so neither command ever starts iterating it.
#[test]
fn an_oversized_range_is_refused_before_it_is_walked() {
    let Some((d, name)) = doctored("huge", |f| f["lineEnd"] = serde_json::json!(4_000_000_000u64)) else { return };
    for cmd in ["summary", "report"] {
        let (code, err) = run(cmd, &d);
        assert_ne!(code, 0, "{cmd} refuses it");
        assert!(!err.contains("panicked"), "{cmd}: {err}");
        assert!(err.contains("source range") && err.contains(&name), "{cmd}: {err}");
    }
    let _ = std::fs::remove_dir_all(&d);
}
