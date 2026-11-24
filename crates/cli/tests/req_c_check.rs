// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for `butterfly check` (area C).
//!
//! Every expected value here is taken from the documentation -- README
//! "Gating a build (`butterfly check`)", the handbook's "Gating a pipeline",
//! and `butterfly check --help` -- never from what the code happens to do.
//! The numbers a floor is compared against are read from the evidence the
//! test itself produced, so the tests hold on any host.
//!
//! Runs only when examples/c has been built; skipped, not failed, otherwise.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_butterfly")
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tmp(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("reqc-check-{tag}"))
}

/// Run butterfly in `cwd`, returning (exit code, stdout, stderr).
fn bf(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin()).current_dir(cwd).args(args).output().expect("run butterfly");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// A fresh evidence file from examples/c, ingested with the documented
/// command, in a directory of its own (so no sidecar is discovered).
fn evidence(tag: &str) -> Option<(PathBuf, PathBuf)> {
    let ex = repo().join("examples/c");
    if !ex.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c/build.sh first");
        return None;
    }
    let d = tmp(tag);
    let ev = d.join("evidence.json");
    let (code, _, err) = bf(&d, &["ingest", ex.join("build").to_str().unwrap(), "--source-root", ex.to_str().unwrap(), "-o", ev.to_str().unwrap()]);
    assert_eq!(code, 0, "ingest of examples/c: {err}");
    Some((d, ev))
}

fn summary(ev: &Path) -> Value {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(ev).unwrap()).unwrap();
    v["summary"].clone()
}

fn n(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or_else(|| panic!("summary.{k} missing"))
}

/// Percentage, computed the way a reader of the README would: covered over
/// total, times 100.
fn pct(c: u64, t: u64) -> f64 {
    100.0 * c as f64 / t as f64
}

fn breach_lines(err: &str) -> Vec<&str> {
    err.lines().filter(|l| l.starts_with("BREACH")).collect()
}

const NOTE: &str = "note     : no resolutions sidecar found; the report records no reviews or justifications";

// README.md:510 "A passing run exits 0, and its plain output writes nothing to stdout"
// README.md:513-517 "when none is discovered a passing run prints one line on stderr — note : no resolutions sidecar found; ..."
#[test]
fn a_passing_run_is_quiet_on_stdout_and_prints_exactly_the_one_sidecar_note() {
    let Some((d, ev)) = evidence("pass") else { return };
    let (code, out, err) = bf(&d, &["check", ev.to_str().unwrap(), "--min-lines", "1"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.is_empty(), "a passing run writes nothing to stdout: {out:?}");
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines, vec![NOTE], "exactly one line on stderr, the documented note: {err:?}");
}

// README.md:510-513 "with `--json` it prints its result object there on a pass as well —
// {"passed": true, "breaches": []}". The stderr note is the same as the plain run's.
#[test]
fn a_passing_run_with_json_prints_its_result_object() {
    let Some((d, ev)) = evidence("pass-json") else { return };
    let (code, out, err) = bf(&d, &["check", ev.to_str().unwrap(), "--min-lines", "1", "--json"]);
    assert_eq!(code, 0, "{err}");
    let doc: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("stdout is a JSON document on a pass too ({e}): {out:?}"));
    assert_eq!(doc, serde_json::json!({"passed": true, "breaches": []}), "{out}");
    assert_eq!(err.lines().collect::<Vec<_>>(), vec![NOTE], "{err:?}");
}

const NOTHING_GATED: &str = "note     : no thresholds given; nothing is gated";

// README.md:529-538 "With no threshold at all — no floor, no --max-unresolved-gaps and no
// --no-regression — ... check says so, in one line on stderr, whatever else it prints ... The exit
// status is still 0 and stdout is unchanged."
#[test]
fn a_run_with_no_thresholds_says_nothing_is_gated() {
    let Some((d, ev)) = evidence("ungated") else { return };
    let e = ev.to_str().unwrap();
    let (code, out, err) = bf(&d, &["check", e]);
    assert_eq!(code, 0, "{err}");
    assert!(out.is_empty(), "stdout is unchanged: {out:?}");
    assert_eq!(err.lines().filter(|l| *l == NOTHING_GATED).count(), 1, "one line says so: {err:?}");
    assert!(err.lines().any(|l| l == NOTE), "and the sidecar note is still printed beside it: {err:?}");
    // under --no-resolutions it is the only line: it is not about sidecars
    let (code, out, err) = bf(&d, &["check", e, "--no-resolutions"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.is_empty(), "{out:?}");
    assert_eq!(err.lines().collect::<Vec<_>>(), vec![NOTHING_GATED], "{err:?}");
    // --json: the same passing document on stdout, the same note on stderr
    let (code, out, err) = bf(&d, &["check", e, "--no-resolutions", "--json"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), serde_json::json!({"passed": true, "breaches": []}), "{out}");
    assert_eq!(err.lines().collect::<Vec<_>>(), vec![NOTHING_GATED], "{err:?}");
    // any one threshold, --no-regression included, is a gate, and the note goes
    for extra in [&["--min-lines", "1"][..], &["--max-unresolved-gaps", "100000"], &["--no-regression", e]] {
        let mut args = vec!["check", e, "--no-resolutions"];
        args.extend_from_slice(extra);
        let (code, _, err) = bf(&d, &args);
        assert_eq!(code, 0, "{extra:?}: {err}");
        assert!(!err.contains("nothing is gated"), "{extra:?} gates something: {err:?}");
    }
}

// README.md:506-508 "--no-resolutions says you meant it: nothing about sidecars is then printed,
// for the evidence or for the --no-regression prior, and a passing run really is silent."
#[test]
fn no_resolutions_makes_a_passing_run_silent_even_with_a_prior() {
    let Some((d, ev)) = evidence("silent") else { return };
    let e = ev.to_str().unwrap();
    let (code, out, err) = bf(&d, &["check", e, "--no-resolutions", "--min-lines", "1", "--no-regression", e]);
    assert_eq!(code, 0, "{err}");
    assert!(out.is_empty(), "{out:?}");
    assert!(err.is_empty(), "a passing run under --no-resolutions really is silent: {err:?}");
}

// README.md:503 the note is printed "once" for a passing run (one line), also when the
// --no-regression prior is read too.
#[test]
fn the_sidecar_note_is_one_line_even_when_a_prior_is_also_read() {
    let Some((d, ev)) = evidence("note-once") else { return };
    let e = ev.to_str().unwrap();
    let (code, _, err) = bf(&d, &["check", e, "--no-regression", e]);
    assert_eq!(code, 0, "nothing is worse than itself: {err}");
    assert_eq!(err.lines().filter(|l| l.contains("no resolutions sidecar")).count(), 1, "{err}");
}

// README.md:514-515 "A floor of 100 is compared as a count" / boundary: a floor exactly at the
// measured percentage is met ("floor" = minimum), one just above it is not.
#[test]
fn a_line_floor_exactly_at_the_measurement_passes_and_just_above_it_breaches() {
    let Some((d, ev)) = evidence("lines-edge") else { return };
    let s = summary(&ev);
    let (c, t) = (n(&s, "linesCovered"), n(&s, "linesTotal"));
    assert!(t > 0 && c < t, "examples/c is not fully line-covered by design");
    let exact = pct(c, t);
    let e = ev.to_str().unwrap();
    let (code, _, err) = bf(&d, &["check", e, "--no-resolutions", "--min-lines", &format!("{exact}")]);
    assert_eq!(code, 0, "a floor equal to the measurement is met: {err}");
    let above = format!("{}", exact + 0.01);
    let (code, out, err) = bf(&d, &["check", e, "--no-resolutions", "--min-lines", &above]);
    assert_eq!(code, 3, "just above the measurement breaches with exit 3: {err}");
    assert!(out.is_empty(), "breaches go to stderr, not stdout: {out}");
    let b = breach_lines(&err);
    assert_eq!(b.len(), 1, "one line per breach: {err}");
    // README.md:511-513: "--min-lines is line coverage — a proxy for statement coverage, not proof of it"
    assert!(b[0].contains("line coverage") && b[0].contains("proxy for statement coverage"), "{err}");
    assert!(b[0].contains(&format!("{c}/{t}")), "the breach names the counts it judged: {err}");
}

// README.md:493 "--min-outcomes 85" is a floor on branch outcomes; a floor above the measurement
// breaches with exit 3 (README.md:508-509).
#[test]
fn an_outcome_floor_above_the_measurement_breaches() {
    let Some((d, ev)) = evidence("outcomes") else { return };
    let s = summary(&ev);
    let (c, t) = (n(&s, "branchesCovered"), n(&s, "branchesTotal"));
    assert!(t > 0 && c < t, "examples/c leaves outcomes untaken by design");
    let e = ev.to_str().unwrap();
    let (code, _, err) = bf(&d, &["check", e, "--no-resolutions", "--min-outcomes", &format!("{}", pct(c, t))]);
    assert_eq!(code, 0, "floor exactly at the measurement: {err}");
    let (code, _, err) = bf(&d, &["check", e, "--no-resolutions", "--min-outcomes", "100"]);
    assert_eq!(code, 3, "{err}");
    let b = breach_lines(&err);
    assert_eq!(b.len(), 1, "{err}");
    assert!(b[0].contains("outcomes"), "{err}");
    assert!(b[0].contains(&format!("{c}/{t}")), "{err}");
}

// README.md:493 "--min-mcdc 100"; README.md:514 "A floor of 100 is compared as a count".
#[test]
fn an_mcdc_floor_is_met_at_the_measurement_and_breached_at_100() {
    let Some((d, ev)) = evidence("mcdc") else { return };
    let s = summary(&ev);
    let (c, t) = (n(&s, "conditionsIndependenceCovered"), n(&s, "conditionsTotal"));
    let e = ev.to_str().unwrap();
    if t == 0 {
        butterfly_testsupport::not_applicable("this toolchain built examples/c without MC/DC records");
        return;
    }
    assert!(c < t, "examples/c leaves an MC/DC condition unproven by design");
    let (code, _, err) = bf(&d, &["check", e, "--no-resolutions", "--min-mcdc", &format!("{}", pct(c, t))]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = bf(&d, &["check", e, "--no-resolutions", "--min-mcdc", "100"]);
    assert_eq!(code, 3, "{err}");
    let b = breach_lines(&err);
    assert_eq!(b.len(), 1, "{err}");
    assert!(b[0].contains("MC/DC") && b[0].contains(&format!("{c}/{t}")), "{err}");
}

// README.md:525-527 "Justified gaps and conditions count as resolved". A recorded justification
// for the one unproven MC/DC condition lets --min-mcdc 100 pass.
#[test]
fn a_justified_mcdc_condition_counts_toward_the_floor() {
    let Some((d, ev)) = evidence("mcdc-justified") else { return };
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&ev).unwrap()).unwrap();
    // the key format documented on the model: `cond:<fn_id>:<line>:<col>:<index>`
    let mut keys = Vec::new();
    for f in v["functions"].as_array().unwrap() {
        for dcs in f["decisions"].as_array().unwrap() {
            for c in dcs["conditions"].as_array().unwrap() {
                if !c["independenceCovered"].as_bool().unwrap() {
                    keys.push(format!("cond:{}:{}:{}:{}", f["id"], dcs["line"], dcs["col"], c["index"]));
                }
            }
        }
    }
    if keys.is_empty() {
        butterfly_testsupport::not_applicable("no unproven MC/DC condition in this build");
        return;
    }
    let entries: Vec<Value> =
        keys.iter().map(|k| serde_json::json!({"key": k, "category": "a", "justification": "reviewed: covered by analysis", "author": "req-c", "date": "2026-01-15"})).collect();
    std::fs::write(d.join("resolutions.json"), serde_json::json!({"schema": "butterfly-resolutions/1", "entries": entries}).to_string()).unwrap();
    let (code, out, err) = bf(&d, &["check", ev.to_str().unwrap(), "--min-mcdc", "100"]);
    assert_eq!(code, 0, "every condition proven or justified meets 100: {err}");
    assert!(out.is_empty() && err.is_empty(), "a sidecar was found, so no note either: {err}");
    // and --no-resolutions judges with no reviews at all: the same floor fails
    let (code, _, err) = bf(&d, &["check", ev.to_str().unwrap(), "--min-mcdc", "100", "--no-resolutions"]);
    assert_eq!(code, 3, "{err}");
}

// README.md:494 "--max-unresolved-gaps 0" is a ceiling: a count equal to it passes, one over it fails.
#[test]
fn the_gap_ceiling_is_inclusive() {
    let Some((d, ev)) = evidence("gap-edge") else { return };
    let gaps = n(&summary(&ev), "gapsTotal");
    assert!(gaps > 0, "examples/c carries gaps by design");
    let e = ev.to_str().unwrap();
    let (code, _, err) = bf(&d, &["check", e, "--no-resolutions", "--max-unresolved-gaps", &gaps.to_string()]);
    assert_eq!(code, 0, "{gaps} gaps against a ceiling of {gaps}: {err}");
    let (code, _, err) = bf(&d, &["check", e, "--no-resolutions", "--max-unresolved-gaps", &(gaps - 1).to_string()]);
    assert_eq!(code, 3, "{err}");
    assert_eq!(breach_lines(&err).len(), 1, "{err}");
}

// README.md:508-509 "On any breach it prints one line per breach on stderr and exits 3"
// README.md:525-526 "--json emits the breaches in that document on stdout instead, with "passed": false."
#[test]
fn several_breaches_print_one_line_each_and_json_carries_the_same_list_instead() {
    let Some((d, ev)) = evidence("multi") else { return };
    let e = ev.to_str().unwrap();
    let args = ["check", e, "--no-resolutions", "--min-lines", "100", "--min-outcomes", "100", "--max-unresolved-gaps", "0"];
    let (code, out, err) = bf(&d, &args);
    assert_eq!(code, 3, "{err}");
    assert!(out.is_empty(), "{out}");
    assert_eq!(breach_lines(&err).len(), 3, "three thresholds breached, three lines: {err}");

    let mut j = args.to_vec();
    j.push("--json");
    let (code, out, err) = bf(&d, &j);
    assert_eq!(code, 3, "--json does not change the exit status: {err}");
    let doc: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("stdout is a JSON document ({e}): {out}"));
    assert_eq!(doc["passed"], false, "{out}");
    let b = doc["breaches"].as_array().expect("a breaches list");
    assert_eq!(b.len(), 3, "{out}");
    let kinds: Vec<&str> = b.iter().map(|x| x["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"lines") && kinds.contains(&"outcomes") && kinds.contains(&"unresolved-gaps"), "{kinds:?}");
    assert!(breach_lines(&err).is_empty(), "--json emits the breaches on stdout *instead*: {err}");
}

// Handbook (docs/butterfly.html, "Gating a pipeline"): "Exit 3 means the threshold was exceeded —
// 1 is an error, 2 a usage mistake". A floor that is not a percentage is a usage mistake.
#[test]
fn a_floor_that_is_not_a_percentage_is_a_usage_mistake() {
    let Some((d, ev)) = evidence("usage") else { return };
    let e = ev.to_str().unwrap();
    for (flag, v) in [("--min-lines", "101"), ("--min-lines", "-1"), ("--min-outcomes", "NaN"), ("--min-mcdc", "inf"), ("--min-lines", "ninety"), ("--max-unresolved-gaps", "-1")] {
        let (code, out, err) = bf(&d, &["check", e, &format!("{flag}={v}")]);
        assert_eq!(code, 2, "{flag} {v} is a usage mistake (exit 2), not a pass or a breach: {err}");
        assert!(out.is_empty(), "{out}");
    }
}

// check --help: "--no-resolutions ... Distinct from omitting --resolutions"; giving both is a
// usage mistake (handbook: 2). A missing evidence argument is one too.
#[test]
fn contradictory_or_missing_arguments_are_usage_mistakes() {
    let d = tmp("usage2");
    let (code, _, err) = bf(&d, &["check", "evidence.json", "--resolutions", "r.json", "--no-resolutions"]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = bf(&d, &["check"]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = bf(&d, &["check", "evidence.json", "--min-coverage", "90"]);
    assert_eq!(code, 2, "an unknown flag: {err}");
}

// Handbook "Gating a pipeline": "1 is an error". An evidence file that does not exist, or a JSON
// file that is not evidence, is an error -- never a pass and never a gate verdict.
#[test]
fn an_unreadable_or_foreign_evidence_file_is_an_error_not_a_verdict() {
    let d = tmp("err");
    let (code, out, err) = bf(&d, &["check", "missing.json", "--min-lines", "0"]);
    assert_eq!(code, 1, "{err}");
    assert!(out.is_empty());
    assert!(err.contains("missing.json"), "the error names the file: {err}");
    std::fs::write(d.join("other.json"), r#"{"hello": "world"}"#).unwrap();
    let (code, _, err) = bf(&d, &["check", "other.json", "--min-lines", "0"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("other.json"), "{err}");
}

// README.md:520-523 "--no-regression compares against a prior evidence file (nothing may be worse...)"
// README.md:528-535 "...on both sides of the comparison. --no-resolutions applies to the prior too, so the
// two evidence files are always judged under the same set of reviews".
#[test]
fn a_prior_whose_reviews_closed_more_gaps_is_a_regression_unless_both_sides_are_judged_without_reviews() {
    let Some((d, ev)) = evidence("regress") else { return };
    // the prior: the same evidence, in its own directory, with a sidecar
    // justifying one gap -- so back then one fewer gap was unresolved
    let pd = d.join("prior");
    std::fs::create_dir_all(&pd).unwrap();
    let prior = pd.join("evidence.json");
    std::fs::copy(&ev, &prior).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&ev).unwrap()).unwrap();
    let key = v["functions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["object"]["gaps"].as_array().and_then(|g| g.first()).map(|g| g["key"].as_str().unwrap().to_string()))
        .next()
        .expect("a gap in examples/c");
    std::fs::write(
        pd.join("resolutions.json"),
        serde_json::json!({"schema": "butterfly-resolutions/1", "entries": [{"key": key, "category": "a", "justification": "reviewed", "author": "req-c", "date": "2026-01-15"}]})
            .to_string(),
    )
    .unwrap();
    let (e, p) = (ev.to_str().unwrap(), prior.to_str().unwrap());
    let (code, _, err) = bf(&d, &["check", e, "--no-regression", p]);
    assert_eq!(code, 3, "unresolved gaps rose relative to the prior's reviews: {err}");
    assert!(breach_lines(&err).iter().any(|l| l.contains("regression")), "{err}");
    let (code, out, err) = bf(&d, &["check", e, "--no-regression", p, "--no-resolutions"]);
    assert_eq!(code, 0, "judged under the same (empty) reviews, nothing is worse: {err}");
    assert!(out.is_empty() && err.is_empty(), "{err}");
}

// README.md:515-518 "a floor over records the evidence does not carry — --min-mcdc against a build
// without -fcoverage-mcdc — is a breach, not a vacuous pass". Any floor, even 0.
#[test]
fn an_mcdc_floor_over_a_build_without_mcdc_records_is_a_breach_at_any_level() {
    let ex = repo().join("examples/c");
    if !ex.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c/build.sh first");
        return;
    }
    let d = tmp("no-mcdc");
    let ev = no_mcdc_evidence(&ex, &d);
    let e = ev.to_str().unwrap();
    for floor in ["0", "50", "100"] {
        let (code, _, err) = bf(&d, &["check", e, "--no-resolutions", "--min-mcdc", floor]);
        assert_eq!(code, 3, "--min-mcdc {floor} over no MC/DC records: {err}");
        assert!(err.contains("-fcoverage-mcdc") || err.contains("no MC/DC"), "says why: {err}");
    }
}

/// examples/c re-ingested from a coverage export with every MC/DC record
/// removed: what a clang build without `-fcoverage-mcdc` produces.
fn no_mcdc_evidence(ex: &Path, d: &Path) -> PathBuf {
    let b = d.join("build");
    std::fs::create_dir_all(&b).unwrap();
    for e in std::fs::read_dir(ex.join("build")).unwrap() {
        let p = e.unwrap().path();
        if p.is_file() {
            std::fs::copy(&p, b.join(p.file_name().unwrap())).unwrap();
        }
    }
    // the .dSYM bundles carry the line table on macOS; link them in place
    #[cfg(unix)]
    for e in std::fs::read_dir(ex.join("build")).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            let _ = std::os::unix::fs::symlink(&p, b.join(p.file_name().unwrap()));
        }
    }
    let _ = std::fs::remove_file(b.join("evidence.json"));
    let mut cov: Value = serde_json::from_str(&std::fs::read_to_string(b.join("coverage.json")).unwrap()).unwrap();
    fn strip(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("mcdc_records");
                m.values_mut().for_each(strip);
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut cov);
    std::fs::write(b.join("coverage.json"), cov.to_string()).unwrap();
    let ev = d.join("evidence.json");
    let (code, _, err) = bf(d, &["ingest", b.to_str().unwrap(), "--source-root", ex.to_str().unwrap(), "-o", ev.to_str().unwrap()]);
    assert_eq!(code, 0, "ingest without MC/DC records: {err}");
    assert_eq!(n(&summary(&ev), "conditionsTotal"), 0, "the stripped export carries no MC/DC");
    ev
}
