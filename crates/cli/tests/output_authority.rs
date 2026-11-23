// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Where butterfly writes, a symlink at the destination is replaced, never
//! written through (#248).
//!
//! The change cache sits next to the evidence by default, usually in a shared
//! build directory, and `--cache` / `-o` name it explicitly. It was written
//! with `fs::write`, which follows a final symlink: anyone who could plant a
//! link there had the next `butterfly changes` overwrite whatever the link
//! pointed at. The resolutions sidecar had been hardened against exactly
//! this (#2); the other outputs had not. Each case below plants a link to a
//! file outside the output directory and checks the file is untouched and
//! the destination became a regular file.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
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
    butterfly_testsupport::scratch_dir(&format!("outauth-{tag}"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_butterfly")).args(args).output().expect("spawn butterfly")
}

/// An output directory holding a link at `name` to a victim outside it.
fn planted(tag: &str, name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let d = scratch(tag);
    let out = d.join("out");
    fs::create_dir_all(&out).unwrap();
    let victim = d.join("victim.txt");
    fs::write(&victim, "PRECIOUS").unwrap();
    let link = out.join(name);
    std::os::unix::fs::symlink(&victim, &link).unwrap();
    (d, link, victim)
}

fn replaced_not_followed(link: &Path, victim: &Path) {
    assert_eq!(fs::read_to_string(victim).unwrap(), "PRECIOUS", "the file the link pointed at was written through");
    let md = fs::symlink_metadata(link).unwrap();
    assert!(md.file_type().is_file(), "the destination is now a regular file, not the link");
}

/// The fixture-free half: the cache writer itself.
#[test]
fn saving_the_change_cache_replaces_a_symlink_instead_of_following_it() {
    let (d, link, victim) = planted("save", "butterfly-changes.json");
    let cache: butterfly_ingest::change::Cache =
        serde_json::from_str(r#"{"schema":"butterfly-changes/1","repo":"/r","head":"h","since":null,"sinceSha":null,"createdUnix":0,"patterns":[],"commits":[],"files":[]}"#)
            .unwrap();
    butterfly_ingest::change::save(&cache, &link).unwrap();
    replaced_not_followed(&link, &victim);
    assert!(fs::read_to_string(&link).unwrap().contains("butterfly-changes/1"), "and it holds the cache");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn an_explicit_cache_destination_that_is_a_symlink_is_replaced() {
    let Some(ev) = evidence() else { return };
    let (d, link, victim) = planted("cache", "cache.json");
    let o = run(&["changes", ev.to_str().unwrap(), "-o", d.join("ev.json").to_str().unwrap(), "--cache", link.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    replaced_not_followed(&link, &victim);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("symlink") && stderr.contains("victim.txt"), "the operator is told what was displaced: {stderr}");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn the_evidence_written_by_changes_replaces_a_symlink() {
    let Some(ev) = evidence() else { return };
    let (d, link, victim) = planted("changes-out", "ev.json");
    let o = run(&["changes", ev.to_str().unwrap(), "-o", link.to_str().unwrap(), "--cache", d.join("cache.json").to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    replaced_not_followed(&link, &victim);
    let _ = fs::remove_dir_all(d);
}

#[test]
fn the_report_replaces_a_symlink() {
    let Some(ev) = evidence() else { return };
    let (d, link, victim) = planted("report", "report.html");
    let o = run(&["report", ev.to_str().unwrap(), "-o", link.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    replaced_not_followed(&link, &victim);
    let _ = fs::remove_dir_all(d);
}

#[test]
fn the_baseline_replaces_a_symlink() {
    let Some(ev) = evidence() else { return };
    let (d, link, victim) = planted("baseline", "baseline.json");
    let o = run(&["baseline", ev.to_str().unwrap(), "-o", link.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    replaced_not_followed(&link, &victim);
    let _ = fs::remove_dir_all(d);
}

/// `butterfly build` writes its own outputs -- the coverage export, each
/// variant's build-info, and each variant's evidence -- and every one of them
/// must replace a planted symlink rather than write through it, the same rule
/// the other commands follow (#248). These used `std::fs::write`, which does
/// not. A link planted at any of these destinations before the build is
/// exactly the case; since #287 the build removes an earlier run's outputs
/// before writing, and a link it removes is announced just the same.
///
/// Needs clang + the C example; skipped otherwise.
#[test]
fn butterfly_build_outputs_replace_symlinks() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    if !src.join("butterfly-build.toml").exists() {
        butterfly_testsupport::fixture_missing("examples/c is not present");
        return;
    }
    if Command::new("clang").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        butterfly_testsupport::not_applicable("clang is not installed");
        return;
    }
    let d = scratch("build");
    for f in ["demo.c", "butterfly-build.toml", "butterfly.toml"] {
        let s = src.join(f);
        if s.exists() {
            fs::copy(&s, d.join(f)).unwrap();
        }
    }
    // out_dir is "variants"; coverage.json sits at its root, and the O0
    // variant's build-info and evidence are the per-variant outputs.
    let out = d.join("variants");
    fs::create_dir_all(out.join("O0")).unwrap();
    let victim = d.join("victim.txt");
    fs::write(&victim, "PRECIOUS").unwrap();
    let links = [out.join("coverage.json"), out.join("O0/build-info.txt"), out.join("evidence-O0.json")];
    for l in &links {
        std::os::unix::fs::symlink(&victim, l).unwrap();
    }

    let o = run(&["build", d.join("butterfly-build.toml").to_str().unwrap()]);
    assert!(o.status.success(), "build failed: {}", String::from_utf8_lossy(&o.stderr));

    for l in &links {
        replaced_not_followed(l, &victim);
    }
    // and the operator is told each output displaced a link
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(stderr.matches("was a symlink").count(), links.len(), "each replaced link is announced: {stderr}");
    let _ = fs::remove_dir_all(d);
}
