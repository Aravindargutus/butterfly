// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for `butterfly claims` and the requirements
//! matrix `summary` prints (area C).
//!
//! Expected results come from README.md "Checking the claim against the
//! measurement" (lines 840-908), "Requirements coverage" (537-574), the
//! `claims --help` text, and examples/c-per-test/README.md, which states what
//! each of its requirements is by design. Each requirements document below is
//! written for the one claim it tests.
//!
//! Runs only when examples/c-per-test has been built.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_butterfly")
}

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-per-test")
}

fn tmp(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("reqc-claims-{tag}"))
}

fn bf(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin()).current_dir(cwd).args(args).output().expect("run butterfly");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// The documented ingest of c-per-test (examples/c-per-test/README.md), into
/// a scratch directory. `per_test` adds `--tests build/tests`.
fn evidence(tag: &str, per_test: bool) -> Option<(PathBuf, PathBuf)> {
    let ex = example();
    if !ex.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c-per-test/build.sh first");
        return None;
    }
    let d = tmp(tag);
    let ev = d.join("evidence.json");
    let mut args = vec!["ingest", "build", "--source-root", ".", "--requirements", "requirements.txt", "-o", ev.to_str().unwrap()];
    if per_test {
        args.extend(["--tests", "build/tests"]);
    }
    let (code, _, err) = bf(&ex, &args);
    assert_eq!(code, 0, "ingest c-per-test: {err}");
    Some((d, ev))
}

/// `butterfly requirements <yaml>` into the scratch directory.
fn sidecar(d: &Path, yaml: &str) -> PathBuf {
    let y = d.join("llr.yaml");
    std::fs::write(&y, yaml).unwrap();
    let out = d.join("requirements.json");
    let (code, _, err) = bf(d, &["requirements", y.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert_eq!(code, 0, "requirements: {err}");
    out
}

fn claims(d: &Path, ev: &Path, req: &Path) -> (i32, Value, String) {
    let (code, out, err) = bf(d, &["claims", ev.to_str().unwrap(), "-r", req.to_str().unwrap(), "--json"]);
    let v: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("--json is a document on stdout ({e}): {out}\n{err}"));
    (code, v, err)
}

fn kinds_for(v: &Value, req: &str) -> Vec<String> {
    v["disagreements"].as_array().unwrap().iter().filter(|d| d["requirement"] == req).map(|d| d["kind"].as_str().unwrap().to_string()).collect()
}

fn list(v: &Value, k: &str) -> Vec<String> {
    v[k].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
}

fn req(id: &str, status: &str, code: &[&str], tests: &[&str]) -> String {
    let l = |xs: &[&str]| format!("[{}]", xs.join(", "));
    format!("  - id: {id}\n    text: statement of {id}\n    type: low-level\n    status: {status}\n    traceability:\n      code: {}\n      tests: {}\n", l(code), l(tests))
}

fn doc(reqs: &[String]) -> String {
    format!("requirements:\n{}", reqs.concat())
}

/// Every requirement ID the fixture's source is tagged with (examples/c-per-test/src/limits.c and
/// inc/limits.h).
const TAGGED: [&str; 5] = ["LLR-LIM-001", "LLR-LIM-002", "LLR-LIM-003", "LLR-LIM-005", "LLR-LIM-006"];

/// `doc`, plus an entry claiming nothing for each tagged ID the given entries leave out -- so a
/// document written for one claim still holds every ID the source cites, and README.md:907
/// `tag-not-in-document` has nothing to say about it. An entry with no claim cannot be wrong about
/// one: it lands in `agreed`, which is why the tests that use this filter those IDs back out.
fn doc_holding_every_tag(reqs: &[String]) -> String {
    let mut all = reqs.to_vec();
    for id in TAGGED {
        if !reqs.iter().any(|r| r.starts_with(&format!("  - id: {id}\n"))) {
            all.push(format!("  - id: {id}\n    text: statement of {id}\n    type: low-level\n    status: implemented\n"));
        }
    }
    doc(&all)
}

/// `agreed`, without the entries `doc_holding_every_tag` added.
fn agreed_of(v: &Value, written: &[&str]) -> Vec<String> {
    list(v, "agreed").into_iter().filter(|id| !TAGGED.contains(&id.as_str()) || written.contains(&id.as_str())).collect()
}

// README.md:876-892 the documented run over the fixture's own document: 4 checked, 2 agreed,
// 1 with nothing claimed, and 3 disagreements -- test-does-not-reach on LLR-LIM-002, naming the
// document's claim, and tag-not-in-document for LLR-LIM-005 and LLR-LIM-006, which the source cites
// and the document does not hold (examples/c-per-test/README.md). README.md:894 "Exit status 3 on
// any disagreement".
#[test]
fn the_documented_example_reports_what_the_readme_shows() {
    let Some((d, ev)) = evidence("doc", true) else { return };
    let req = d.join("requirements.json");
    let (code, _, err) = bf(&example(), &["requirements", "llr.yaml", "-o", req.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = bf(&d, &["claims", ev.to_str().unwrap(), "--requirements", req.to_str().unwrap()]);
    assert_eq!(code, 3, "{err}");
    assert!(out.is_empty(), "without --json the account is on stderr: {out}");
    let checked = err.lines().find(|l| l.starts_with("checked")).unwrap_or_else(|| panic!("a `checked` line: {err}"));
    assert!(checked.contains("4 requirement(s)") && checked.contains("2 agreed") && checked.contains("1 with nothing claimed"), "{checked}");
    assert!(err.contains("3 disagreement(s): tag-not-in-document 2, test-does-not-reach 1"), "{err}");
    assert!(err.contains("[test-does-not-reach] LLR-LIM-002"), "{err}");
    assert!(err.contains("document: src/tests.c::t_clamp"), "{err}");
    assert!(err.contains("[tag-not-in-document] LLR-LIM-005") && err.contains("[tag-not-in-document] LLR-LIM-006"), "{err}");
    assert!(err.contains("evidence: `band_of` is tagged with it in the source"), "{err}");
}

// README.md:871 exit 3 only on disagreement; README.md:893-895 "Agreement is reported as plainly as
// disagreement".
#[test]
fn a_document_that_agrees_exits_zero_and_says_so() {
    let Some((d, ev)) = evidence("agree", true) else { return };
    let r = sidecar(&d, &doc_holding_every_tag(&[req("LLR-LIM-001", "implemented", &["src/limits.c::clamp_alt"], &["src/tests.c::t_clamp", "src/tests.c::t_ceiling"])]));
    let (code, out, err) = bf(&d, &["claims", ev.to_str().unwrap(), "-r", r.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("5 agreed"), "agreement is stated: {err}");
    assert!(err.contains("agree everywhere they both speak"), "{err}");
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 0);
    assert_eq!(agreed_of(&v, &["LLR-LIM-001"]), vec!["LLR-LIM-001"]);
    assert!(v["disagreements"].as_array().unwrap().is_empty());
}

// README.md:880 "code-not-found | the claim names code that was not analysed — renamed, deleted, never built"
#[test]
fn a_claim_naming_code_that_does_not_exist_is_code_not_found() {
    let Some((d, ev)) = evidence("cnf", true) else { return };
    let r = sidecar(&d, &doc(&[req("LLR-LIM-001", "implemented", &["src/limits.c::clamp_alt", "src/limits.c::clamp_altitude_v2"], &[])]));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 3);
    assert_eq!(kinds_for(&v, "LLR-LIM-001"), vec!["code-not-found"]);
    assert!(!list(&v, "agreed").contains(&"LLR-LIM-001".to_string()));
}

// README.md:881 "claimed-not-tagged | the named code exists but carries no tag for this requirement"
#[test]
fn a_claim_naming_untagged_code_is_claimed_not_tagged() {
    let Some((d, ev)) = evidence("cnt", true) else { return };
    let r = sidecar(&d, &doc(&[req("LLR-LIM-002", "implemented", &["src/limits.c::rate_ok", "src/limits.c::clamp_alt"], &[])]));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 3);
    assert_eq!(kinds_for(&v, "LLR-LIM-002"), vec!["claimed-not-tagged"]);
    let d0 = &v["disagreements"][0];
    assert_eq!(d0["claimed"], "src/limits.c::clamp_alt", "names the document's own words: {d0}");
}

// README.md:882 "tagged-not-claimed | the source is tagged with it; the document's traceability does not say so"
#[test]
fn tagged_code_the_document_omits_is_tagged_not_claimed() {
    let Some((d, ev)) = evidence("tnc", true) else { return };
    // LLR-LIM-003 is tagged on fault_name; the document names clamp_alt instead
    let r = sidecar(&d, &doc(&[req("LLR-LIM-003", "implemented", &["src/limits.c::clamp_alt"], &[])]));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 3);
    let mut k = kinds_for(&v, "LLR-LIM-003");
    k.sort();
    assert_eq!(k, vec!["claimed-not-tagged", "tagged-not-claimed"]);
    assert!(v["disagreements"].as_array().unwrap().iter().any(|x| x["kind"] == "tagged-not-claimed" && x["measured"].as_str().unwrap().contains("fault_name")));
}

// README.md:907 "tag-not-in-document | the source is tagged with an ID the requirements document does
// not hold". LLR-LIM-005 and LLR-LIM-006 are tagged on band_of's lines; a document without them has
// a hole in its traceability, reported with the same output, JSON and exit status as any other
// disagreement (README.md:894 "Exit status 3 on any disagreement").
#[test]
fn a_tag_citing_an_id_the_document_does_not_hold_is_tag_not_in_document() {
    let Some((d, ev)) = evidence("tnd", true) else { return };
    let written = [req("LLR-LIM-001", "implemented", &["src/limits.c::clamp_alt"], &["src/tests.c::t_clamp", "src/tests.c::t_ceiling"])];
    let r = sidecar(&d, &doc(&written));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 3, "a tag the document does not hold fails the gate: {v}");
    for id in ["LLR-LIM-002", "LLR-LIM-003", "LLR-LIM-005", "LLR-LIM-006"] {
        assert_eq!(kinds_for(&v, id), vec!["tag-not-in-document"], "{id}: {v}");
    }
    let five = v["disagreements"].as_array().unwrap().iter().find(|x| x["requirement"] == "LLR-LIM-005").unwrap();
    assert_eq!(five["claimed"], "not in the requirements document", "{five}");
    assert_eq!(five["measured"], "`band_of` is tagged with it in the source", "names the tagged code: {five}");
    assert_eq!(list(&v, "agreed"), vec!["LLR-LIM-001"], "the requirement the document does hold still agrees: {v}");
    // the terminal says the same, line for line with the other kinds
    let (code, out, err) = bf(&d, &["claims", ev.to_str().unwrap(), "-r", r.to_str().unwrap()]);
    assert_eq!(code, 3, "{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("4 disagreement(s): tag-not-in-document 4"), "{err}");
    assert!(err.contains("[tag-not-in-document] LLR-LIM-005"), "{err}");
    // and a document holding every tagged ID has nothing of this kind to report
    let r = sidecar(&d, &doc_holding_every_tag(&written));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 0, "{v}");
    assert!(v["disagreements"].as_array().unwrap().is_empty(), "{v}");
}

// README.md:883 "implemented-no-code | status: implemented, and nothing implements it"
// README.md:886-889 "planned requirements are not faulted for having no code"
#[test]
fn implemented_with_no_code_is_faulted_and_planned_with_no_code_is_not() {
    let Some((d, ev)) = evidence("inc", true) else { return };
    let r = sidecar(&d, &doc(&[req("LLR-NEW-001", "implemented", &[], &[]), req("LLR-NEW-002", "planned", &[], &[])]));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 3);
    assert_eq!(kinds_for(&v, "LLR-NEW-001"), vec!["implemented-no-code"]);
    assert!(kinds_for(&v, "LLR-NEW-002").is_empty(), "planned is the expected state: {v}");
    assert!(!list(&v, "agreed").contains(&"LLR-NEW-002".to_string()), "nothing claimed is not agreement either");
    // a planned requirement on its own: nothing to disagree with, exit 0
    let r = sidecar(&d, &doc_holding_every_tag(&[req("LLR-NEW-002", "planned", &[], &[])]));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 0, "{v}");
    assert_eq!(list(&v, "unclaimed"), vec!["LLR-NEW-002"]);
}

// README.md:900-903 "Paths match on whole components from the right (limits.c matches
// src/limits.c; mylimits.c does not)".
#[test]
fn paths_match_on_whole_components_from_the_right() {
    let Some((d, ev)) = evidence("paths", true) else { return };
    let r = sidecar(&d, &doc_holding_every_tag(&[req("LLR-LIM-001", "implemented", &["limits.c::clamp_alt"], &[])]));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 0, "limits.c is a component suffix of src/limits.c: {v}");
    assert_eq!(agreed_of(&v, &["LLR-LIM-001"]), vec!["LLR-LIM-001"]);
    for wrong in ["mylimits.c::clamp_alt", "src/limits.c::clamp", "src/limits.c::clamp_alt_x"] {
        let r = sidecar(&d, &doc(&[req("LLR-LIM-001", "implemented", &[wrong], &[])]));
        let (code, v, _) = claims(&d, &ev, &r);
        assert_eq!(code, 3, "{wrong} must not resolve: {v}");
        assert!(kinds_for(&v, "LLR-LIM-001").contains(&"code-not-found".to_string()), "{wrong}: {v}");
    }
}

// README.md:898-905 "Matching is exact. ... Paths match on whole components from the right ... A
// near-miss that resolved would report an unverified requirement as traced — the one outcome worse
// than a false alarm." The analysed file is src/limits.c (its recorded path); a claim naming
// other/limits.c names a file that was never built, and must not resolve to it.
#[test]
fn a_claim_naming_a_different_directory_does_not_resolve() {
    let Some((d, ev)) = evidence("paths-dir", true) else { return };
    let r = sidecar(&d, &doc(&[req("LLR-LIM-001", "implemented", &["other/limits.c::clamp_alt"], &[])]));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 3, "other/limits.c was never analysed: {v}");
    assert!(kinds_for(&v, "LLR-LIM-001").contains(&"code-not-found".to_string()), "{v}");
    assert!(!list(&v, "agreed").contains(&"LLR-LIM-001".to_string()), "a false agreement: {v}");
}

// README.md:897-900 "A test is judged against the lines the requirement claims, not the function they
// sit in. @satisfies on one line claims that line; a test that enters the function elsewhere and never
// executes it has verified nothing". LLR-LIM-006 is tagged on line 25 of band_of; t_band_low enters
// band_of and returns at line 24.
#[test]
fn a_test_that_enters_the_function_but_not_the_tagged_line_does_not_reach() {
    let Some((d, ev)) = evidence("scope", true) else { return };
    let r = sidecar(&d, &doc(&[req("LLR-LIM-006", "implemented", &["src/limits.c::band_of"], &["src/tests.c::t_band_low"])]));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 3, "{v}");
    assert_eq!(kinds_for(&v, "LLR-LIM-006"), vec!["test-does-not-reach"]);
    // and the test that does take that line agrees
    let r = sidecar(&d, &doc_holding_every_tag(&[req("LLR-LIM-006", "implemented", &["src/limits.c::band_of"], &["src/tests.c::t_band_high"])]));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 0, "{v}");
}

// README.md:903 "test-not-found | the document says a test verifies it; the evidence has per-test
// data, and no test of that name ran — renamed, deleted, never run". README.md:922-923 agreement is
// reported only where it was established, so it is not agreement either.
#[test]
fn a_claimed_test_that_never_ran_is_test_not_found() {
    let Some((d, ev)) = evidence("tnf", true) else { return };
    let r = sidecar(&d, &doc(&[req("LLR-LIM-001", "implemented", &["src/limits.c::clamp_alt"], &["src/tests.c::t_does_not_exist"])]));
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 3, "a disagreement: {v}");
    assert!(!list(&v, "agreed").contains(&"LLR-LIM-001".to_string()), "{v}");
    assert_eq!(kinds_for(&v, "LLR-LIM-001"), vec!["test-not-found"], "{v}");
    assert_eq!(v["disagreements"][0]["claimed"], "src/tests.c::t_does_not_exist");
    // the README's condition: only where the evidence has per-test data. Without it, the same
    // claim is unchecked, not a test that was not found.
    let Some((d, ev)) = evidence("tnf-noper", false) else { return };
    let r = sidecar(&d, &doc(&[req("LLR-LIM-001", "implemented", &["src/limits.c::clamp_alt"], &["src/tests.c::t_does_not_exist"])]));
    let (_, v, _) = claims(&d, &ev, &r);
    assert!(kinds_for(&v, "LLR-LIM-001").is_empty(), "no per-test data, no test-not-found: {v}");
    assert_eq!(v["notChecked"][0]["kind"], "no-per-test-data", "{v}");
}

// README.md:897-900, for a requirement no tag locates: the claimed test is judged against the code the
// document claims, so a test that never enters it does not reach. rate_ok is reached by no test
// (examples/c-per-test/README.md: "rate_ok is exercised by no test at all").
#[test]
fn an_untagged_requirement_is_judged_against_the_code_the_document_claims() {
    let Some((d, ev)) = evidence("noscope", true) else { return };
    let r = sidecar(
        &d,
        &doc(&[
            req("LLR-NEW-010", "implemented", &["src/limits.c::rate_ok"], &["src/tests.c::t_clamp"]),
            req("LLR-NEW-011", "implemented", &["src/limits.c::clamp_alt"], &["src/tests.c::t_clamp"]),
        ]),
    );
    let (code, v, _) = claims(&d, &ev, &r);
    assert_eq!(code, 3);
    let mut k = kinds_for(&v, "LLR-NEW-010");
    k.sort();
    assert_eq!(k, vec!["claimed-not-tagged", "test-does-not-reach"], "{v}");
    assert_eq!(kinds_for(&v, "LLR-NEW-011"), vec!["claimed-not-tagged"], "t_clamp does enter clamp_alt: {v}");
}

// README.md:895-897 "Where the evidence carries no per-test data, test claims are not judged at all,
// and the command says so rather than passing them in silence."
#[test]
fn without_per_test_data_test_claims_are_not_judged_and_that_is_said() {
    let Some((d, ev)) = evidence("noper", false) else { return };
    let r = sidecar(&d, &doc_holding_every_tag(&[req("LLR-LIM-002", "implemented", &["src/limits.c::rate_ok"], &["src/tests.c::t_clamp"])]));
    let (code, v, err) = claims(&d, &ev, &r);
    assert!(!list(&v, "agreed").contains(&"LLR-LIM-002".to_string()), "not judged is not agreed: {v}");
    assert!(kinds_for(&v, "LLR-LIM-002").is_empty(), "nor a disagreement: {v}");
    assert!(err.contains("per-test"), "and the command says so: {err}");
    assert_eq!(code, 0, "no disagreement, so no exit 3: {err}");
}

// README.md:906-907 "butterfly never edits the requirements file."
#[test]
fn the_requirements_file_is_never_edited() {
    let Some((d, ev)) = evidence("ro", true) else { return };
    let r = sidecar(&d, &doc(&[req("LLR-LIM-002", "implemented", &["src/limits.c::rate_ok"], &["src/tests.c::t_clamp"])]));
    let before = (std::fs::read(&r).unwrap(), std::fs::read(d.join("llr.yaml")).unwrap());
    let (code, _, _) = claims(&d, &ev, &r);
    assert_eq!(code, 3);
    let _ = bf(&d, &["claims", ev.to_str().unwrap(), "-r", r.to_str().unwrap()]);
    assert_eq!(before, (std::fs::read(&r).unwrap(), std::fs::read(d.join("llr.yaml")).unwrap()));
}

// claims --help: `-r, --requirements <REQUIREMENTS>` is required; omitting it is a usage mistake (2).
// A requirements file that is not a sidecar is an error (1), never a pass.
#[test]
fn claims_argument_errors() {
    let Some((d, ev)) = evidence("args", true) else { return };
    let (code, _, err) = bf(&d, &["claims", ev.to_str().unwrap()]);
    assert_eq!(code, 2, "{err}");
    std::fs::write(d.join("junk.json"), "[1,2,3]").unwrap();
    let (code, _, err) = bf(&d, &["claims", ev.to_str().unwrap(), "-r", "junk.json"]);
    assert_eq!(code, 1, "{err}");
    let (code, _, err) = bf(&d, &["claims", ev.to_str().unwrap(), "-r", "absent.json"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("absent.json"), "{err}");
}

// README.md:556-564 the matrix in `summary`: "The orphans are the point, in three directions: a
// requirement in the supplied --requirements universe that no code is tagged with; a requirement
// implemented but exercised by no test; and functions carrying no requirement tag at all".
// examples/c-per-test/README.md: LLR-LIM-001 fully covered by named tests, LLR-LIM-002 implemented
// but tested by nothing, LLR-LIM-003 partially reached, LLR-LIM-004 no code at all.
#[test]
fn summary_shows_the_matrix_and_its_three_orphans() {
    let Some((d, ev)) = evidence("matrix", true) else { return };
    let (code, _, err) = bf(&d, &["summary", ev.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let row = |id: &str| err.lines().find(|l| l.trim_start().starts_with(id)).unwrap_or_else(|| panic!("a row for {id}: {err}")).to_string();
    let frac = |l: &str| -> (u32, u32) {
        let f = l.split_whitespace().find(|w| w.contains('/') && w.chars().all(|c| c.is_ascii_digit() || c == '/')).expect("a lines fraction");
        let (a, b) = f.split_once('/').unwrap();
        (a.parse().unwrap(), b.parse().unwrap())
    };
    let r1 = row("LLR-LIM-001");
    let (a, b) = frac(&r1);
    assert!(a == b && b > 0, "LLR-LIM-001 fully covered by named tests: {r1}");
    assert!(r1.contains("t_clamp") && r1.contains("t_ceiling"), "{r1}");
    assert!(row("LLR-LIM-002").contains("NO TEST"), "implemented, unverified: {err}");
    let (a, b) = frac(&row("LLR-LIM-003"));
    assert!(a > 0 && a < b, "LLR-LIM-003 partially reached: {err}");
    assert!(row("LLR-LIM-004").contains("NO CODE"), "in the universe, tagged nowhere: {err}");
    assert!(err.contains("carry no requirement tag"), "untagged functions: {err}");
    // README.md:548-550: per-test, so the matrix says the lines come from the named tests
    assert!(err.contains("covered by the named tests"), "{err}");
}

// README.md:550-551 "Without per-test data the matrix still renders, labelled as the aggregate proxy it
// then is."
#[test]
fn without_per_test_data_the_matrix_is_labelled_a_proxy() {
    let Some((d, ev)) = evidence("matrix-agg", false) else { return };
    let (code, _, err) = bf(&d, &["summary", ev.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let reqs = err.lines().find(|l| l.starts_with("reqs")).unwrap_or_else(|| panic!("the matrix renders: {err}"));
    assert!(reqs.contains("aggregate") && reqs.contains("proxy"), "{reqs}");
    assert!(!err.contains("NO TEST"), "no per-test data says nothing about tests: {err}");
}
