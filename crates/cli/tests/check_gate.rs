// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly check` (#43): the gate must refuse for the right reasons,
//! stay silent for the right reasons, and exit 3 exactly when it says FAIL.
//!
//! Runs only when examples/c has been built; skipped, not failed, otherwise.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(tag: &str) -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c/build.sh first");
        return None;
    }
    let d = butterfly_testsupport::scratch_dir(&format!("check-{tag}"));
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root);
    let ev = butterfly_ingest::ingest(&o).expect("ingest");
    std::fs::write(d.join("evidence.json"), serde_json::to_string(&ev).unwrap()).unwrap();
    Some(d)
}

/// Run `check`, returning (exit code, stderr, stdout).
fn check(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_butterfly")).arg("check").arg(dir.join("evidence.json")).args(args).output().expect("run butterfly check");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).into_owned(), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn passing_thresholds_are_silent_and_exit_zero() {
    let Some(d) = scratch("pass") else { return };
    let (code, err, out) = check(&d, &["--min-lines", "50", "--min-outcomes", "50", "--max-unresolved-gaps", "100"]);
    assert_eq!(code, 0);
    assert!(!err.contains("BREACH"), "silent on success: {err}");
    assert!(out.is_empty(), "and nothing on stdout: {out}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_inconsistent_no_regression_prior_is_rejected_by_its_path() {
    let Some(d) = scratch("prior") else { return };
    // Forge a prior whose summary no longer matches its own records. It still
    // passes validate() (covered <= total), so only the derivation check binds
    // it to its primitives; gate::regressions() would otherwise trust its
    // summary for the line/outcome comparison (#249).
    let text = std::fs::read_to_string(d.join("evidence.json")).unwrap();
    let mut prior: butterfly_model::Evidence = serde_json::from_str(&text).unwrap();
    prior.summary.lines_total += 1;
    let pp = d.join("prior.json");
    std::fs::write(&pp, serde_json::to_string(&prior).unwrap()).unwrap();
    let (code, err, _) = check(&d, &["--min-lines", "1", "--min-outcomes", "1", "--no-regression", pp.to_str().unwrap()]);
    assert_eq!(code, 1, "an inconsistent prior document is an error, not a pass or a gate breach: {err}");
    assert!(err.contains("prior.json"), "the error names the prior document: {err}");
    assert!(err.contains("does not match"), "and says the summary does not match its records: {err}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_gap_ceiling_breach_names_the_worst_functions_and_exits_three() {
    let Some(d) = scratch("gaps") else { return };
    let (code, err, _) = check(&d, &["--max-unresolved-gaps", "0"]);
    assert_eq!(code, 3);
    assert!(err.contains("unresolved gap(s) against a ceiling of 0"), "{err}");
    assert!(err.contains("worst:"), "names where to start: {err}");
    assert!(err.contains("6.4.4.3"), "and what closing one means: {err}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A justified gap is a resolved gap: the same ceiling that fails without
/// the sidecar passes with it.
#[test]
fn a_recorded_justification_counts_as_resolved() {
    let Some(d) = scratch("justified") else { return };
    let ev: butterfly_model::Evidence = serde_json::from_str(&std::fs::read_to_string(d.join("evidence.json")).unwrap()).unwrap();
    let total: usize = ev.functions.iter().flat_map(|f| f.objects()).map(|o| o.gaps.len()).sum();
    let one_key = ev.functions.iter().flat_map(|f| f.objects()).flat_map(|o| o.gaps.iter()).next().expect("the fixture has gaps").key.clone();

    // ceiling = total - 1: fails while nothing is resolved
    let ceiling = (total - 1).to_string();
    let (code, _, _) = check(&d, &["--max-unresolved-gaps", &ceiling]);
    assert_eq!(code, 3, "one gap over the ceiling");

    // resolve exactly one gap, and the same ceiling passes
    let res = serde_json::json!({
        "schema": "butterfly-resolutions/1",
        "entries": [{ "key": one_key, "category": "analysis", "justification": "test", "author": "t", "date": "2026-09-02" }]
    });
    let rp = d.join("res.json");
    std::fs::write(&rp, serde_json::to_string(&res).unwrap()).unwrap();
    let (code, err, _) = check(&d, &["--max-unresolved-gaps", &ceiling, "--resolutions", rp.to_str().unwrap()]);
    assert_eq!(code, 0, "the justification counted: {err}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_mcdc_floor_over_missing_records_is_a_breach_not_a_pass() {
    let Some(d) = scratch("norecords") else { return };
    // strip every decision: the shape of a build without -fcoverage-mcdc. The
    // summary's decision/condition counts go with them, so the evidence stays
    // internally consistent (the load-time summary check would reject it
    // otherwise, #249) -- a real no-mcdc build has both at zero.
    let mut ev: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("evidence.json")).unwrap()).unwrap();
    for f in ev["functions"].as_array_mut().unwrap() {
        f["decisions"] = serde_json::json!([]);
    }
    for k in ["decisionsTotal", "decisionsMcdcCovered", "conditionsTotal", "conditionsIndependenceCovered"] {
        ev["summary"][k] = serde_json::json!(0);
    }
    std::fs::write(d.join("evidence.json"), serde_json::to_string(&ev).unwrap()).unwrap();
    let (code, err, _) = check(&d, &["--min-mcdc", "100"]);
    assert_eq!(code, 3, "a gate over nothing must fail, not vacuously pass");
    assert!(err.contains("no MC/DC records"), "{err}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_regression_names_the_function_that_fell() {
    let Some(d) = scratch("regress") else { return };
    // The prior run covered one more line of some function. Flip an actual
    // uncovered line record to covered and bump the function, file and summary
    // counters together, so the prior stays internally consistent -- the
    // per-primitive derivation check would reject a bumped counter with no
    // matching record (#249).
    let mut prior: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("evidence.json")).unwrap()).unwrap();
    let (fi, file_id, li, name) = {
        let funcs = prior["functions"].as_array().unwrap();
        let mut hit = None;
        for (fi, f) in funcs.iter().enumerate() {
            let file_id = f["fileId"].as_u64().unwrap() as usize;
            let (ls, le) = (f["lineStart"].as_u64().unwrap(), f["lineEnd"].as_u64().unwrap());
            let lines = prior["files"][file_id]["lines"].as_array().unwrap();
            for l in ls..=le {
                if lines.get((l - 1) as usize).and_then(|r| r["count"].as_u64()) == Some(0) {
                    hit = Some((fi, file_id, (l - 1) as usize, f["name"].as_str().unwrap().to_string()));
                    break;
                }
            }
            if hit.is_some() {
                break;
            }
        }
        hit.expect("the fixture has a function with an uncovered executable line")
    };
    prior["files"][file_id]["lines"][li]["count"] = serde_json::json!(1);
    let bump = |v: &mut serde_json::Value| *v = serde_json::json!(v.as_u64().unwrap() + 1);
    bump(&mut prior["files"][file_id]["linesCovered"]);
    bump(&mut prior["functions"][fi]["linesCovered"]);
    bump(&mut prior["summary"]["linesCovered"]);
    let pp = d.join("prior.json");
    std::fs::write(&pp, serde_json::to_string(&prior).unwrap()).unwrap();

    let (code, err, _) = check(&d, &["--no-regression", pp.to_str().unwrap()]);
    assert_eq!(code, 3);
    assert!(err.contains("line coverage fell"), "{err}");
    assert!(err.contains(&format!("`{name}`")), "the function is named: {err}");

    // and against itself, nothing regresses
    let (code, err, _) = check(&d, &["--no-regression", d.join("evidence.json").to_str().unwrap()]);
    assert_eq!(code, 0, "self-comparison is clean: {err}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn json_mode_reports_the_breaches_and_still_exits_three() {
    let Some(d) = scratch("json") else { return };
    let (code, _, out) = check(&d, &["--max-unresolved-gaps", "0", "--json"]);
    assert_eq!(code, 3, "machine-readable output does not soften the exit code");
    let v: serde_json::Value = serde_json::from_str(&out).expect("stdout is one JSON document");
    assert_eq!(v["passed"], serde_json::json!(false));
    assert_eq!(v["breaches"][0]["kind"], serde_json::json!("unresolved-gaps"));
    let _ = std::fs::remove_dir_all(&d);
}

/// Losing the measurements entirely must read as a regression on every
/// metric, not slip through a zero-denominator hole: a prior that measured
/// something against a current that measures nothing is instrumentation
/// loss, the exact case the gate exists to refuse.
#[test]
fn losing_all_measurements_is_a_regression_on_every_metric() {
    let Some(d) = scratch("vanish") else { return };
    let prior = d.join("prior.json");
    std::fs::copy(d.join("evidence.json"), &prior).unwrap();
    // current evidence: the summary measures nothing, and no decision
    // carries records -- the shape of a build whose instrumentation broke
    let mut ev: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("evidence.json")).unwrap()).unwrap();
    // Zero the measurements at the primitive level too -- the files' line
    // counts and the functions' branches/decisions -- so the zeroed summary
    // still derives from the records and passes the load-time check (#249). A
    // build whose instrumentation broke measures nothing, consistently.
    for f in ev["files"].as_array_mut().unwrap() {
        f["linesTotal"] = serde_json::json!(0);
        f["linesCovered"] = serde_json::json!(0);
        // clear the line records themselves, or the file cache (now 0) would
        // disagree with them and the current document is rejected before the
        // regression comparison runs (#249): instrumentation loss means the
        // primitives measured nothing, consistently.
        for l in f["lines"].as_array_mut().unwrap() {
            l["count"] = serde_json::Value::Null;
        }
    }
    for f in ev["functions"].as_array_mut().unwrap() {
        f["branches"] = serde_json::json!([]);
        f["decisions"] = serde_json::json!([]);
        // and the per-function line caches, for the same reason
        f["linesTotal"] = serde_json::json!(0);
        f["linesCovered"] = serde_json::json!(0);
    }
    for k in ["linesTotal", "linesCovered", "branchesTotal", "branchesCovered", "decisionsTotal", "decisionsMcdcCovered", "conditionsTotal", "conditionsIndependenceCovered"] {
        ev["summary"][k] = serde_json::json!(0);
    }
    std::fs::write(d.join("evidence.json"), serde_json::to_string(&ev).unwrap()).unwrap();

    let (code, err, _) = check(&d, &["--no-regression", prior.to_str().unwrap()]);
    assert_eq!(code, 3);
    for kind in ["regression-lines", "regression-outcomes", "regression-mcdc"] {
        assert!(err.contains(kind), "{kind} must fire when its measurements vanish: {err}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The note a passing run is allowed to print, verbatim. The README quotes
/// this line and `check`'s `--help` describes it; changing it means changing
/// both (#213).
const SIDECAR_NOTE: &str = "note     : no resolutions sidecar found; the report records no reviews or justifications";

/// What a passing run prints, pinned in both directions (#213).
///
/// "Silent on success" was the documented contract and was not true: the
/// sidecar lookup speaks. The lookup is worth keeping -- `--max-unresolved-gaps`
/// and `--min-mcdc` are judged against the sidecar, so a gate that passed
/// having read no reviews must not be indistinguishable from one that read
/// some -- so the note stays and the docs say so. What must not happen is
/// the note under `--no-resolutions`, where the operator has said not to go
/// looking.
///
/// The existing `passing_thresholds_are_silent_and_exit_zero` above asserts
/// only that no BREACH appears, which is why this went unnoticed.
#[test]
fn a_passing_run_prints_the_sidecar_note_and_nothing_else() {
    let Some(d) = scratch("note") else { return };
    let (code, err, out) = check(&d, &["--min-lines", "50", "--min-outcomes", "50", "--max-unresolved-gaps", "100"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.is_empty(), "nothing on stdout: {out:?}");
    assert_eq!(err.trim_end(), SIDECAR_NOTE, "exactly one line, and it is the documented one");
    let _ = std::fs::remove_dir_all(&d);
}

/// With `--no-resolutions` a passing run says nothing at all.
#[test]
fn no_resolutions_prints_nothing_about_sidecars() {
    let Some(d) = scratch("nores") else { return };
    let (code, err, out) = check(&d, &["--no-resolutions", "--min-lines", "50", "--min-outcomes", "50", "--max-unresolved-gaps", "100"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.is_empty(), "nothing on stdout: {out:?}");
    assert!(err.is_empty(), "and nothing on stderr: {err:?}");
    let _ = std::fs::remove_dir_all(&d);
}

/// `--no-resolutions` reaches the `--no-regression` prior too.
///
/// This is the half that changes a verdict rather than only the noise. The
/// prior's sidecar was discovered and loaded whatever the flag said, so the
/// unresolved-gap count was taken with no reviews on this side and with
/// every review on the other: `check` reported `regression-gaps` and exited
/// 3 on two identical evidence files, because of the flag rather than
/// because of the build.
#[test]
fn no_resolutions_does_not_go_looking_beside_the_prior_either() {
    let Some(d) = scratch("nores-prior") else { return };
    // The prior is this very evidence, with its own sidecar resolving every
    // gap. Nothing about the build differs, so nothing may regress.
    let prior_dir = d.join("prior");
    std::fs::create_dir_all(&prior_dir).unwrap();
    let prior = prior_dir.join("evidence.json");
    std::fs::copy(d.join("evidence.json"), &prior).unwrap();
    let ev: butterfly_model::Evidence = serde_json::from_str(&std::fs::read_to_string(d.join("evidence.json")).unwrap()).unwrap();
    let entries: Vec<serde_json::Value> = ev
        .functions
        .iter()
        .flat_map(|f| f.objects())
        .flat_map(|o| o.gaps.iter())
        .map(|g| serde_json::json!({ "key": g.key, "category": "analysis", "justification": "reviewed", "author": "t", "date": "2026-09-20" }))
        .collect();
    assert!(!entries.is_empty(), "the fixture has gaps to resolve");
    std::fs::write(prior_dir.join("resolutions.json"), serde_json::to_string(&serde_json::json!({ "schema": "butterfly-resolutions/1", "entries": entries })).unwrap()).unwrap();

    let (code, err, out) = check(&d, &["--no-resolutions", "--no-regression", prior.to_str().unwrap()]);
    assert_eq!(code, 0, "both sides judged with no reviews, so nothing regressed: {err}");
    assert!(!err.contains("regression-gaps"), "the prior's sidecar was not read: {err}");
    assert!(err.is_empty() && out.is_empty(), "and it went looking for nothing: {err:?} {out:?}");

    // Without the flag the asymmetry is real and must still be reported:
    // this side genuinely has no sidecar and the prior genuinely has one.
    let (code, err, _) = check(&d, &["--no-regression", prior.to_str().unwrap()]);
    assert_eq!(code, 3, "a real difference in recorded reviews is still a breach: {err}");
    assert!(err.contains("regression-gaps"), "{err}");
    let _ = std::fs::remove_dir_all(&d);
}

/// The behaviour above and `--help` must say the same thing, since the
/// README quotes both (#213). README.md itself is deliberately not read
/// here: CI skips prose-only changes (`paths-ignore: **/*.md`) on the
/// stated grounds that nothing under crates/ reads a markdown file, and a
/// test that did would make that skip unsound.
#[test]
fn the_help_text_describes_what_a_passing_run_prints() {
    let out = Command::new(env!("CARGO_BIN_EXE_butterfly")).args(["check", "--help"]).output().expect("run butterfly check --help");
    let help = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(!help.contains("Silent on success"), "the old claim is gone: {help}");
    assert!(help.contains("no resolutions sidecar was discovered"), "it says when the one line appears: {help}");
    assert!(help.contains("--no-resolutions silences it"), "and what turns it off: {help}");
    assert!(help.contains("Applies to the --no-regression prior as well"), "and that the flag reaches the prior: {help}");
}

/// A floor that is not a percentage is a configuration error, refused at
/// the flag: -1 passes anything, 101 quietly means 100, NaN breaches
/// everything nonempty.
#[test]
fn a_floor_outside_zero_to_one_hundred_is_refused_at_the_flag() {
    let Some(d) = scratch("badflag") else { return };
    // `=` form, so a leading minus reaches the value parser rather than
    // being read as an unknown flag (both ways exit 2 either way)
    for bad in ["--min-lines=-1", "--min-lines=101", "--min-lines=NaN", "--min-lines=inf", "--min-outcomes=100.5", "--min-mcdc=-0.1"] {
        let (code, err, _) = check(&d, &[bad]);
        assert_eq!(code, 2, "clap refuses {bad}: {err}");
        assert!(err.contains("percentage between 0 and 100"), "{bad}: {err}");
    }
    // the boundaries themselves are legitimate policy
    let (code, _, _) = check(&d, &["--min-lines", "0"]);
    assert_eq!(code, 0);
    let _ = std::fs::remove_dir_all(&d);
}

/// `--no-regression` reads a sidecar twice -- for this evidence and for the
/// prior -- and the note is about the run, not about either file, so it is
/// printed once (#213).
///
/// Without this the README's "prints one line on stderr" is false for every
/// regression run: the two tests above pass a single evidence file and so
/// only ever reach one lookup. The count is asserted, not the presence,
/// because two notes and one note both "contain" the note.
#[test]
fn the_sidecar_note_is_printed_once_however_many_sidecars_are_looked_for() {
    let Some(d) = scratch("noteonce") else { return };
    let prior = d.join("evidence.json");
    let (code, err, out) = check(&d, &["--min-lines", "50", "--min-outcomes", "50", "--max-unresolved-gaps", "100", "--no-regression", prior.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(out.is_empty(), "nothing on stdout: {out:?}");
    assert_eq!(err.matches(SIDECAR_NOTE).count(), 1, "once for the run, not once per lookup: {err:?}");
    assert_eq!(err.trim_end(), SIDECAR_NOTE, "and still nothing else: {err:?}");

    // and the flag still silences it across both lookups
    let (code, err, _) = check(&d, &["--no-resolutions", "--min-lines", "50", "--min-outcomes", "50", "--max-unresolved-gaps", "100", "--no-regression", prior.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(err.is_empty(), "--no-resolutions says nothing at all: {err:?}");
    let _ = std::fs::remove_dir_all(&d);
}
