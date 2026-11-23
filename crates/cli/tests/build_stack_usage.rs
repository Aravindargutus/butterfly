// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly build` must ingest the `.su` files it just produced.
//!
//! The compiler's own stack-usage figures are authoritative; the prologue
//! estimate is the fallback for when they are absent. Producing the files and
//! then quietly ignoring them gives every variant the weaker evidence (#7).

use std::path::PathBuf;
use std::process::Command;

/// The C example, copied so the test never writes into the repository.
fn staged() -> Option<PathBuf> {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    if !src.join("butterfly-build.toml").exists() {
        butterfly_testsupport::fixture_missing("examples/c is not present");
        return None;
    }
    if Command::new("clang").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        butterfly_testsupport::fixture_missing("clang is not installed");
        return None;
    }
    let d = butterfly_testsupport::scratch_dir("build-su");
    for f in ["demo.c", "butterfly-build.toml", "butterfly.toml"] {
        let s = src.join(f);
        if s.exists() {
            std::fs::copy(&s, d.join(f)).unwrap();
        }
    }
    Some(d)
}

#[test]
fn every_variant_ingests_its_stack_usage_file() {
    let Some(d) = staged() else { return };
    let out = Command::new(env!("CARGO_BIN_EXE_butterfly")).args(["build", d.join("butterfly-build.toml").to_str().unwrap()]).output().expect("spawn butterfly");
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));

    let variants = d.join("variants");
    let mut checked = 0;
    for entry in std::fs::read_dir(&variants).expect("variants directory") {
        let p = entry.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if !name.starts_with("evidence-") || !name.ends_with(".json") {
            continue;
        }
        let ev: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();

        let su: Vec<&serde_json::Value> = ev["provenance"]["inputs"].as_array().unwrap().iter().filter(|i| i["role"] == "stack-usage").collect();
        assert!(!su.is_empty(), "{name}: the .su file the build produced is not among the evidence inputs");

        let from_su = ev["symbols"].as_array().unwrap().iter().filter(|s| s["frame"]["source"] == "stackUsageFile").count();
        let from_prologue = ev["symbols"].as_array().unwrap().iter().filter(|s| s["frame"]["source"] == "prologue").count();
        assert!(from_su > 0, "{name}: no symbol used the compiler's figure ({from_prologue} fell back to the prologue estimate)");
        checked += 1;
    }
    assert!(checked >= 2, "expected several variants, checked {checked}");
    let _ = std::fs::remove_dir_all(d);
}
