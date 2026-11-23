// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly claims` end to end, over the per-test example whose fixture
//! carries a deliberately wrong claim (#57).
//!
//! Runs only when examples/c-per-test has been built; skipped otherwise.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-per-test")
}

fn bf(args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_butterfly")).args(args).output().expect("run");
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn the_fixtures_wrong_test_claim_is_caught_and_exits_three() {
    let r = root();
    if !r.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c-per-test/build.sh first");
        return;
    }
    let d = butterfly_testsupport::scratch_dir("claims");
    let (ev, req) = (d.join("evidence.json"), d.join("requirements.json"));

    let (c, _, e) = bf(&["requirements", r.join("llr.yaml").to_str().unwrap(), "-o", req.to_str().unwrap()]);
    assert_eq!(c, Some(0), "{e}");
    let (c, _, e) =
        bf(&["ingest", r.join("build").to_str().unwrap(), "--source-root", r.to_str().unwrap(), "--tests", r.join("build/tests").to_str().unwrap(), "-o", ev.to_str().unwrap()]);
    assert_eq!(c, Some(0), "{e}");

    let (code, _, err) = bf(&["claims", ev.to_str().unwrap(), "--requirements", req.to_str().unwrap()]);
    assert_eq!(code, Some(3), "a disagreement fails the gate: {err}");

    // LLR-LIM-002 says t_clamp verifies it; t_clamp exercises clamp_alt and
    // never calls rate_ok. That is the claim the fixture plants.
    assert!(err.contains("test-does-not-reach"), "{err}");
    assert!(err.contains("LLR-LIM-002"), "{err}");
    assert!(err.contains("t_clamp"), "{err}");

    // and the true claims are not dragged in with it
    assert!(!err.contains("LLR-LIM-001"), "a claim the measurement bears out is not a disagreement: {err}");
    assert!(err.contains("agreed"), "agreement is stated too: {err}");

    // --json carries the same verdict for a machine
    let (code, out, _) = bf(&["claims", ev.to_str().unwrap(), "--requirements", req.to_str().unwrap(), "--json"]);
    assert_eq!(code, Some(3));
    let v: serde_json::Value = serde_json::from_str(&out).expect("json on stdout");
    assert_eq!(v["disagreements"][0]["kind"], "test-does-not-reach");
    assert_eq!(v["disagreements"][0]["requirement"], "LLR-LIM-002");
    // Per-test data was supplied, so every claim here really was checked and
    // nothing is marked otherwise. The counterpart run below is only
    // meaningful against this (#233).
    assert_eq!(v["notChecked"].as_array().map(Vec::len), Some(0), "with --tests every claim is checkable: {out}");

    let _ = std::fs::remove_dir_all(&d);
}

/// The same fixture, ingested *without* `--tests`, is where #233 bit.
///
/// Without per-test data no test claim can be checked -- including
/// LLR-LIM-002's, which this fixture plants as deliberately false. The
/// planted disagreement simply vanishes, and the three requirements that
/// name a test were reported as `agreed`, under the line "the document and
/// the measurement agree everywhere they both speak", exit 0. A CI gate
/// reading `--json` saw an `agreed` list and an empty `disagreements` list
/// and had nothing to tell it that the strongest claim in the document had
/// not been looked at.
///
/// So this asserts the negative that matters -- LLR-LIM-002 is NOT in
/// `agreed` -- as well as the marker that replaced it.
#[test]
fn without_per_test_data_the_same_claims_are_reported_unchecked_not_agreed() {
    let r = root();
    if !r.join("build/coverage.json").exists() {
        // c-per-test is built by ci.yml and ci-local, so it is required where
        // the fixtures are (#242): absent, fail rather than pass silently.
        butterfly_testsupport::fixture_missing("run examples/c-per-test/build.sh first");
        return;
    }
    let d = butterfly_testsupport::scratch_dir("claims-nopt");
    let (ev, req) = (d.join("evidence.json"), d.join("requirements.json"));

    let (c, _, e) = bf(&["requirements", r.join("llr.yaml").to_str().unwrap(), "-o", req.to_str().unwrap()]);
    assert_eq!(c, Some(0), "{e}");
    // the same build, the same document -- only `--tests` is withheld
    let (c, _, e) = bf(&["ingest", r.join("build").to_str().unwrap(), "--source-root", r.to_str().unwrap(), "-o", ev.to_str().unwrap()]);
    assert_eq!(c, Some(0), "{e}");

    let (code, out, err) = bf(&["claims", ev.to_str().unwrap(), "--requirements", req.to_str().unwrap(), "--json"]);
    // An unchecked claim is not a disagreement. The gate still fails, but
    // only on what needs no per-test data: the source's LLR-LIM-005 and
    // LLR-LIM-006 tags, which the fixture's document does not hold (#325).
    assert_eq!(code, Some(3), "{err}");

    let v: serde_json::Value = serde_json::from_str(&out).expect("json on stdout");
    let found: Vec<(&str, &str)> = v["disagreements"].as_array().unwrap().iter().map(|d| (d["kind"].as_str().unwrap(), d["requirement"].as_str().unwrap())).collect();
    assert_eq!(found, [("tag-not-in-document", "LLR-LIM-005"), ("tag-not-in-document", "LLR-LIM-006")], "no unchecked claim became a disagreement: {out}");
    let agreed: Vec<&str> = v["agreed"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    assert!(agreed.is_empty(), "no requirement whose test claim went unchecked may be called agreed (#233): {agreed:?}");

    let nc = v["notChecked"].as_array().expect("--json carries the marker (#233)");
    assert!(!nc.is_empty(), "the claims that could not be checked are listed: {out}");
    assert!(nc.iter().all(|n| n["kind"] == "no-per-test-data"), "{out}");
    let reqs: Vec<&str> = nc.iter().map(|n| n["requirement"].as_str().unwrap()).collect();
    for id in ["LLR-LIM-001", "LLR-LIM-002", "LLR-LIM-003"] {
        assert!(reqs.contains(&id), "{id} names a test, so its claim is unchecked here: {out}");
    }
    // LLR-LIM-004 claims nothing at all, which is a different thing again
    // and must not be swept into the unchecked list.
    assert!(!reqs.contains(&"LLR-LIM-004"), "a requirement that claims nothing is `unclaimed`, not `notChecked`: {out}");
    assert_eq!(v["unclaimed"][0], "LLR-LIM-004", "{out}");

    // and the terminal stops short of claiming agreement
    let (_, _, err) = bf(&["claims", ev.to_str().unwrap(), "--requirements", req.to_str().unwrap()]);
    assert!(err.contains("unchecked"), "the terminal says what went unchecked: {err}");
    assert!(err.contains("0 agreed"), "and does not count an unchecked claim as agreement: {err}");
    assert!(!err.contains("agree everywhere they both speak"), "the unqualified agreement sentence is exactly what must not be printed here: {err}");

    let _ = std::fs::remove_dir_all(&d);
}
