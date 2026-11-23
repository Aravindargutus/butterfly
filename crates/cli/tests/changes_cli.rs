// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly changes` through the binary. Uses examples/c/build/evidence.json
//! (skipped when the example has not been built) and never writes into the
//! example directory: output and cache always go to a scratch directory.

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
    butterfly_testsupport::scratch_dir(&format!("cli-{tag}"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_butterfly")).args(args).output().expect("spawn butterfly")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn c42_out_leaves_the_input_untouched_and_writes_changes() {
    let Some(ev) = evidence() else { return };
    let d = scratch("out");
    let before = fs::read(&ev).unwrap();
    let out = d.join("ev.json");
    let cache = d.join("cache.json");
    let o = run(&["changes", ev.to_str().unwrap(), "-o", out.to_str().unwrap(), "--cache", cache.to_str().unwrap()]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(fs::read(&ev).unwrap(), before, "input must not be rewritten when -o is given");
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
    assert!(v["changes"]["commits"].as_array().map(|a| !a.is_empty()).unwrap_or(false), "changes written to -o");
    assert!(cache.exists());
    let e = stderr(&o);
    assert!(e.contains("walked") && e.contains("history"), "{e}");
    let _ = fs::remove_dir_all(d);
}

/// The support matrix row `feature.changes` claims "the commits that touched
/// a function", and until the claims audit (#200) nothing on the example
/// checked that half: c42 above asserts only that some commit was recorded.
/// Each commit names the functions whose line range it touched, by the id
/// the evidence gives them; the commit that added `demo.c` touched every
/// function in it, so `clamp_altitude` must be named by at least one commit,
/// and no commit may name a function the evidence does not have.
#[test]
fn c42b_each_commit_names_the_functions_it_touched() {
    let Some(ev) = evidence() else { return };
    let d = scratch("touched");
    let out = d.join("ev.json");
    let o = run(&["changes", ev.to_str().unwrap(), "-o", out.to_str().unwrap(), "--cache", d.join("cache.json").to_str().unwrap()]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
    let ids: std::collections::HashMap<u64, &str> = v["functions"].as_array().unwrap().iter().map(|f| (f["id"].as_u64().unwrap(), f["name"].as_str().unwrap())).collect();
    let commits = v["changes"]["commits"].as_array().expect("commits");
    let mut named = std::collections::HashSet::new();
    for c in commits {
        for id in c["functions"].as_array().expect("every commit carries the functions it touched") {
            let id = id.as_u64().expect("a function is named by its evidence id");
            let name = ids.get(&id).unwrap_or_else(|| panic!("commit {} names function id {id}, which the evidence does not have", c["short"]));
            named.insert(*name);
        }
    }
    for f in ["clamp_altitude", "checksum", "average"] {
        assert!(named.contains(f), "no commit names {f} as touched: {named:?}");
    }
    let _ = fs::remove_dir_all(d);
}

#[test]
fn c43_missing_config_is_an_error_naming_the_path() {
    let Some(ev) = evidence() else { return };
    let d = scratch("cfg");
    let o = run(&[
        "changes",
        ev.to_str().unwrap(),
        "-o",
        d.join("ev.json").to_str().unwrap(),
        "--cache",
        d.join("c.json").to_str().unwrap(),
        "--config",
        "/nonexistent/butterfly.toml",
    ]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("/nonexistent/butterfly.toml"), "{}", stderr(&o));
    let _ = fs::remove_dir_all(d);
}

#[test]
fn c44_corrupt_cache_is_ignored_with_a_warning_not_fatal() {
    let Some(ev) = evidence() else { return };
    let d = scratch("corrupt");
    let cache = d.join("cache.json");
    fs::write(&cache, "{ this is not json").unwrap();
    let out = d.join("ev.json");
    let o = run(&["changes", ev.to_str().unwrap(), "-o", out.to_str().unwrap(), "--cache", cache.to_str().unwrap()]);
    let e = stderr(&o);
    assert!(o.status.success(), "a corrupt cache must not abort the run: {e}");
    assert!(e.to_lowercase().contains("cache") && (e.to_lowercase().contains("ignor") || e.to_lowercase().contains("rebuil")), "warn about it: {e}");
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
    assert!(v["changes"].is_object());
    // and the cache is now valid
    let c: serde_json::Value = serde_json::from_str(&fs::read_to_string(&cache).unwrap()).unwrap();
    assert_eq!(c["schema"], "butterfly-changes/1");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn c45_bogus_since_is_nonzero_and_names_the_revision() {
    let Some(ev) = evidence() else { return };
    let d = scratch("since");
    let o = run(&["changes", ev.to_str().unwrap(), "-o", d.join("ev.json").to_str().unwrap(), "--cache", d.join("c.json").to_str().unwrap(), "--since", "definitely-not-a-rev"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("definitely-not-a-rev"), "{}", stderr(&o));
    assert!(!d.join("ev.json").exists(), "no output on failure");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn c46_nonexistent_repo_is_an_error() {
    let Some(ev) = evidence() else { return };
    let d = scratch("repo");
    let o = run(&["changes", ev.to_str().unwrap(), "-o", d.join("ev.json").to_str().unwrap(), "--cache", d.join("c.json").to_str().unwrap(), "--repo", "/nonexistent/repo"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("/nonexistent/repo"), "{}", stderr(&o));
    let _ = fs::remove_dir_all(d);
}

#[test]
fn c47_jobs_zero_is_accepted() {
    let Some(ev) = evidence() else { return };
    let d = scratch("jobs");
    let o = run(&["changes", ev.to_str().unwrap(), "-o", d.join("ev.json").to_str().unwrap(), "--cache", d.join("c.json").to_str().unwrap(), "--jobs", "0"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let _ = fs::remove_dir_all(d);
}

#[test]
fn c48_second_run_reuses_the_cache() {
    let Some(ev) = evidence() else { return };
    let d = scratch("reuse");
    let out = d.join("ev.json");
    let cache = d.join("c.json");
    let o1 = run(&["changes", ev.to_str().unwrap(), "-o", out.to_str().unwrap(), "--cache", cache.to_str().unwrap()]);
    assert!(o1.status.success(), "{}", stderr(&o1));
    let o2 = run(&["changes", out.to_str().unwrap(), "--cache", cache.to_str().unwrap()]);
    assert!(o2.status.success(), "{}", stderr(&o2));
    let e = stderr(&o2);
    assert!(e.contains("walked   : 0 function(s)"), "second run should walk nothing: {e}");
    let _ = fs::remove_dir_all(d);
}
