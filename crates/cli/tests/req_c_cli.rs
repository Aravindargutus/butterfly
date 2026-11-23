// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the CLI surface (area C): the command list,
//! `--version`/`--help`, exit-status conventions, `ingest` defaults,
//! `doctor`, `run`, and `tags`' default output.
//!
//! Expected results come from README.md "Check it works" (67-89), "Commands"
//! (114-136), "Checking a build before you trust it" (976-1042), the
//! handbook's "Gating a pipeline", and each command's `--help` text -- never
//! from what the code currently does.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_butterfly")
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tmp(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("reqc-cli-{tag}"))
}

fn bf(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin()).current_dir(cwd).args(args).output().expect("run butterfly");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn have(example: &str) -> bool {
    if repo().join("examples").join(example).join("build/coverage.json").exists() {
        return true;
    }
    butterfly_testsupport::fixture_missing(&format!("run examples/{example}/build.sh first"));
    false
}

/// A private copy of an example's build directory, with the `.dSYM`
/// bundles linked rather than copied. The coverage export names the sources
/// by their absolute paths, so the sources stay where they are and the
/// example directory remains the source root.
fn copy_build(example: &str, d: &Path) -> PathBuf {
    let ex = repo().join("examples").join(example).join("build");
    let b = d.join("build");
    std::fs::create_dir_all(&b).unwrap();
    for e in std::fs::read_dir(&ex).unwrap() {
        let s = e.unwrap().path();
        let t = b.join(s.file_name().unwrap());
        if s.is_file() {
            std::fs::copy(&s, &t).unwrap();
        } else {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&s, &t).unwrap();
        }
    }
    let _ = std::fs::remove_file(b.join("evidence.json"));
    let _ = std::fs::remove_file(b.join("report.html"));
    b
}

fn src(example: &str) -> PathBuf {
    repo().join("examples").join(example)
}

// ---------------------------------------------------------------- top level

// README.md:69 "butterfly --version" is the first check that the install works.
#[test]
fn version_prints_the_name_and_exits_zero() {
    let (code, out, err) = bf(&tmp("version"), &["--version"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("butterfly "), "{out}");
    assert!(out.trim().len() > "butterfly ".len(), "and a version: {out}");
}

// README.md:116 "butterfly <command> --help for the full option list", and the Commands table
// (README.md:118-130) plus `check` (README.md:491) and `run` (README.md:425): each is a command.
#[test]
fn every_documented_command_exists_and_has_help() {
    let d = tmp("help");
    let (code, top, _) = bf(&d, &["--help"]);
    assert_eq!(code, 0);
    for c in ["ingest", "serve", "summary", "report", "baseline", "build", "changes", "impact", "tags", "doctor", "claims", "requirements", "check", "run"] {
        assert!(top.lines().any(|l| l.trim_start().starts_with(&format!("{c} "))), "`{c}` listed in --help: {top}");
        let (code, out, err) = bf(&d, &[c, "--help"]);
        assert_eq!(code, 0, "{c} --help: {err}");
        assert!(out.contains("Usage: butterfly"), "{c}: {out}");
    }
}

// Handbook "Gating a pipeline": "1 is an error, 2 a usage mistake". No command, or a command that
// does not exist, is a usage mistake.
#[test]
fn no_command_or_an_unknown_one_is_a_usage_mistake() {
    let d = tmp("usage");
    assert_eq!(bf(&d, &[]).0, 2);
    assert_eq!(bf(&d, &["frobnicate"]).0, 2);
    assert_eq!(bf(&d, &["summary"]).0, 2, "a missing required argument");
}

// README.md:1312-1313 "Exit status 3 means the threshold was exceeded (1 is an error, 2 a usage
// mistake)"; impact/changes --help: "--fail-on ... none | blocker | major | any". A value outside that
// list is a usage mistake.
#[test]
fn an_invalid_fail_on_level_is_a_usage_mistake() {
    let d = tmp("failon");
    std::fs::write(d.join("evidence.json"), "{}").unwrap();
    for cmd in ["impact", "changes"] {
        let (code, _, err) = bf(&d, &[cmd, "evidence.json", "--fail-on", "critical"]);
        assert_eq!(code, 2, "`{cmd} --fail-on critical`: {err}");
    }
}

// ---------------------------------------------------------------- ingest

// README.md:122 "`-o` defaults to `./evidence.json`, not into the build directory."
#[test]
fn ingest_writes_to_the_working_directory_by_default() {
    if !have("c") {
        return;
    }
    let d = tmp("ingest-o");
    let b = copy_build("c", &d);
    let cwd = d.join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (code, _, err) = bf(&cwd, &["ingest", b.to_str().unwrap(), "--source-root", src("c").to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(cwd.join("evidence.json").is_file(), "./evidence.json: {err}");
    assert!(!b.join("evidence.json").exists(), "not into the build directory");
}

// README.md:122 "read a build directory (or explicit --coverage/--object)": with neither there is
// nothing to read, which is an error naming what to pass.
#[test]
fn ingest_with_nothing_to_read_is_an_error() {
    let d = tmp("ingest-none");
    let (code, _, err) = bf(&d, &["ingest"]);
    assert_ne!(code, 0, "{err}");
    assert!(err.contains("--coverage"), "{err}");
    assert!(!d.join("evidence.json").exists(), "and nothing is written");
}

// ingest --help: "--tests: One coverage export per test ... Repeatable, or point it at a directory and
// every *.json inside is taken as one test, named by its file stem."
#[test]
fn ingest_tests_by_file_and_by_directory_name_the_same_tests() {
    if !have("c-per-test") {
        return;
    }
    let ex = repo().join("examples/c-per-test");
    let d = tmp("ingest-tests");
    let (a, b) = (d.join("dir.json"), d.join("files.json"));
    let (code, _, err) = bf(&ex, &["ingest", "build", "--source-root", ".", "--tests", "build/tests", "-o", a.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let mut files: Vec<String> = std::fs::read_dir(ex.join("build/tests"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .map(|p| p.display().to_string())
        .collect();
    files.sort();
    let mut args = vec!["ingest".to_string(), "build".into(), "--source-root".into(), ".".into(), "-o".into(), b.display().to_string()];
    for f in &files {
        args.push("--tests".into());
        args.push(f.clone());
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, _, err) = bf(&ex, &args);
    assert_eq!(code, 0, "{err}");
    let names = |p: &Path| -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        let mut n: Vec<String> = v["tests"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        n.sort();
        n
    };
    let stems: Vec<String> = {
        let mut s: Vec<String> = files.iter().map(|f| Path::new(f).file_stem().unwrap().to_string_lossy().into_owned()).collect();
        s.sort();
        s
    };
    assert_eq!(names(&a), stems, "a directory: one test per *.json, named by its stem");
    assert_eq!(names(&b), stems, "the same exports named one by one");
}

// ingest --help: "point it at a directory and every *.json inside is taken as one test". A directory
// with none is not "no tests" silently: it is an error naming the directory.
#[test]
fn ingest_tests_directory_without_exports_is_an_error() {
    if !have("c") {
        return;
    }
    let d = tmp("ingest-empty-tests");
    std::fs::create_dir_all(d.join("tests")).unwrap();
    let ex = repo().join("examples/c");
    let (code, _, err) = bf(&d, &["ingest", ex.join("build").to_str().unwrap(), "--source-root", ex.to_str().unwrap(), "--tests", "tests"]);
    assert_ne!(code, 0, "{err}");
    assert!(err.contains("tests"), "{err}");
    assert!(!d.join("evidence.json").exists());
}

/// examples/c ingested into `d`, with a baseline frozen from it and left in
/// a private copy of the build directory. Returns (build dir, baseline).
fn baseline_in_build_dir(d: &Path) -> (PathBuf, PathBuf) {
    let bd = copy_build("c", d);
    let ev = d.join("ev.json");
    let (code, _, err) = bf(d, &["ingest", bd.to_str().unwrap(), "--source-root", src("c").to_str().unwrap(), "-o", ev.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let b = bd.join("butterfly-baseline.json");
    let (code, _, err) = bf(d, &["baseline", ev.to_str().unwrap(), "-o", b.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    (bd, b)
}

fn ingest_baseline(d: &Path, bd: &Path, extra: &[&str]) -> serde_json::Value {
    let out = d.join("again.json");
    let mut args = vec!["ingest", bd.to_str().unwrap(), "--source-root"];
    let root = src("c");
    args.push(root.to_str().unwrap());
    args.extend(extra);
    args.extend(["-o", out.to_str().unwrap()]);
    let (code, _, err) = bf(d, &args);
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    v["baseline"].clone()
}

// README.md:124 "an ingest given it with --baseline compares against it and flags stale analyses";
// the same build compared with its own baseline matches.
#[test]
fn an_ingest_given_a_baseline_compares_against_it() {
    if !have("c") {
        return;
    }
    let d = tmp("baseline-flag");
    let (bd, b) = baseline_in_build_dir(&d);
    let v = ingest_baseline(&d, &bd, &["--baseline", b.to_str().unwrap()]);
    assert!(!v.is_null(), "--baseline compares");
    assert_eq!(v["status"], "matches", "the build it was frozen from: {v}");
}

// README.md:124 "an ingest compares against it ... when given it with `--baseline`, or when it finds a
// `butterfly-baseline.json` in the build directory it reads (#238, fixed by #241). With neither, nothing
// is compared"; README.md:1126-1129, docs/butterfly.html:603, CHANGELOG.md:128-132 and README.md:132-135
// ("ingest <dir> discovers ... butterfly-baseline.json") now all say the same. (Until #325 C5 the first
// four said the file was not discovered, contradicting the last and the code.)
#[test]
fn a_baseline_left_in_the_build_directory_is_discovered_without_the_flag() {
    if !have("c") {
        return;
    }
    let d = tmp("baseline-discovered");
    let (bd, _) = baseline_in_build_dir(&d);
    let v = ingest_baseline(&d, &bd, &[]);
    assert!(!v.is_null(), "the baseline in the build directory is compared with no flag: {v}");
    assert_eq!(v["status"], "matches", "the build it was frozen from: {v}");
}

// ---------------------------------------------------------------- doctor

// README.md:983-988 the doctor header: directory, coverage, object, stack; README.md:1004-1008
// only WRONG exits 3. A complete clang build exits 0.
#[test]
fn doctor_names_what_it_found_and_passes_a_complete_build() {
    if !have("c") {
        return;
    }
    let b = repo().join("examples/c/build");
    let (code, out, err) = bf(&repo(), &["doctor", b.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("directory : "), "{out}");
    assert!(out.lines().any(|l| l.starts_with("coverage  : ") && l.ends_with("coverage.json")), "{out}");
    assert!(out.lines().any(|l| l.starts_with("object    : ") && l.ends_with("demo")), "the non-instrumented binary: {out}");
    assert!(out.lines().any(|l| l.starts_with("stack     : 1 .su file(s)")), "{out}");
    assert!(!out.contains("WRONG"), "{out}");
}

// README.md:1007-1008 "WEAKER | the evidence is weaker than it looks"; "note | worth knowing; nothing is
// misreported"; doctor --help: "-v Report the notes as well as the problems". Missing .su files make
// the evidence thinner, never wrong: not exit 3.
#[test]
fn doctor_missing_stack_files_is_not_wrong() {
    if !have("c") {
        return;
    }
    let d = tmp("doctor-su");
    let b = copy_build("c", &d);
    std::fs::remove_file(b.join("demo.su")).unwrap();
    let (code, out, err) = bf(&d, &["doctor", "-v", b.to_str().unwrap()]);
    assert_eq!(code, 0, "thinner, not wrong: {out}{err}");
    assert!(out.contains(".su"), "the missing stack files are named: {out}");
    assert!(out.contains("stack     : 0 .su file(s)"), "{out}");
}

// README.md:1018-1030 "no MC/DC records | -fcoverage-mcdc was not passed" (clang): the expensive one,
// "worth calling out", and on clang "a flag away". A problem worth reporting is shown without -v.
#[test]
fn doctor_calls_out_a_clang_export_without_mcdc_records() {
    if !have("c") {
        return;
    }
    let d = tmp("doctor-mcdc");
    let b = copy_build("c", &d);
    let cp = b.join("coverage.json");
    let mut cov: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cp).unwrap()).unwrap();
    fn strip(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                m.remove("mcdc_records");
                m.values_mut().for_each(strip);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut cov);
    std::fs::write(&cp, cov.to_string()).unwrap();
    let (code, out, err) = bf(&d, &["doctor", b.to_str().unwrap()]);
    assert!(out.contains("-fcoverage-mcdc"), "the clang fix is named without -v: {out}{err}");
    assert_ne!(code, 1, "{err}");
}

// README.md:997-1003 doctor exists to catch evidence that "looks complete and is wrong" before ingest;
// an export ingest cannot read must not pass. README.md:1004 WRONG exits 3.
#[test]
fn doctor_does_not_pass_a_coverage_export_that_cannot_be_read() {
    if !have("c") {
        return;
    }
    let d = tmp("doctor-bad");
    let b = copy_build("c", &d);
    std::fs::write(b.join("coverage.json"), "{\"data\": [ truncated").unwrap();
    let (code, out, err) = bf(&d, &["doctor", b.to_str().unwrap()]);
    assert_eq!(code, 3, "{out}{err}");
    assert!(out.contains("WRONG") && out.contains("coverage.json"), "{out}");
}

// Handbook "Gating a pipeline": "1 is an error". A directory that does not exist is an error, not a
// clean bill of health.
#[test]
fn doctor_on_a_missing_directory_is_an_error() {
    let d = tmp("doctor-missing");
    let (code, _, err) = bf(&d, &["doctor", "no-such-build"]);
    assert_eq!(code, 1, "{err}");
}

// ---------------------------------------------------------------- run

// run --help: "Build, check, ingest, report and serve in one go"; "--no-build Skip the build and use what
// is already in the output directory"; "--no-serve Stop after writing the report instead of serving it".
#[test]
fn run_without_build_or_serve_writes_evidence_and_report_into_the_output_directory() {
    if !have("c") {
        return;
    }
    let d = tmp("run-ok");
    // the project is examples/c itself (the coverage export names its
    // sources there); the output directory is a private copy, given as an
    // absolute --out-dir so nothing is written into the fixture
    let b = copy_build("c", &d);
    let (code, out, err) = bf(&d, &["run", src("c").to_str().unwrap(), "--no-build", "--no-serve", "--out-dir", b.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(out.is_empty(), "the run narrates on stderr: {out}");
    assert!(b.join("evidence.json").is_file(), "{err}");
    let html = std::fs::read_to_string(b.join("report.html")).expect("the report");
    assert!(html.contains("Software Verification Results") || html.contains("<h2>1. Summary"), "a report");
    for phase in ["[1/4] build", "[2/4] doctor", "[3/4] ingest", "[4/4] report"] {
        assert!(err.contains(phase), "{phase}: {err}");
    }
    assert!(!err.contains("[5/"), "no serve phase under --no-serve: {err}");
    assert!(err.contains("butterfly serve"), "says how to serve it: {err}");
}

// run --help "--no-build ... use what is already in the output directory": when there is nothing there,
// the run stops before ingest with a usage-mistake status (handbook: 2) and says what to do.
#[test]
fn run_no_build_without_an_output_directory_stops_and_explains() {
    let d = tmp("run-nodir");
    let (code, _, err) = bf(&d, &["run", d.to_str().unwrap(), "--no-build", "--no-serve"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--no-build"), "{err}");
    assert!(!err.contains("[3/4] ingest"), "{err}");
}

// run --help "--out-dir Where the build script puts its output": a build that succeeds but writes no
// output directory stops the run, naming the directory it expected.
#[test]
fn run_stops_when_the_build_produces_no_output_directory() {
    let d = tmp("run-noout");
    let s = d.join("build.sh");
    std::fs::write(&s, "#!/bin/sh\necho built\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&s, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (code, _, err) = bf(&d, &["run", d.to_str().unwrap(), "--no-serve", "--out-dir", "out"]);
    assert_ne!(code, 0, "{err}");
    assert!(err.contains("out"), "{err}");
    assert!(!err.contains("[3/4] ingest"), "{err}");
}

// run --help "--force Carry on even when doctor says the evidence would be wrong": without it a WRONG
// finding stops the run with exit 3 (README.md:1004, doctor's own WRONG status).
#[test]
fn run_stops_on_a_wrong_doctor_finding_unless_forced() {
    if !have("c") {
        return;
    }
    let d = tmp("run-wrong");
    let b = copy_build("c", &d);
    std::fs::write(b.join("coverage.json"), "{\"data\": [ truncated").unwrap();
    let (c, bs) = (src("c"), b.display().to_string());
    let (code, _, err) = bf(&d, &["run", c.to_str().unwrap(), "--no-build", "--no-serve", "--out-dir", &bs]);
    assert_eq!(code, 3, "{err}");
    assert!(err.contains("--force"), "names the override: {err}");
    assert!(!err.contains("[3/4] ingest"), "{err}");
    let (_, _, err) = bf(&d, &["run", c.to_str().unwrap(), "--no-build", "--no-serve", "--out-dir", &bs, "--force"]);
    assert!(err.contains("[3/4] ingest"), "--force carries on past doctor: {err}");
}

// run --help: "--requirements <REQUIREMENTS>  A requirements document to fold into the report." A
// requirements document is what `butterfly requirements` reads (YAML, CSV or Markdown; README.md:130),
// e.g. examples/c-per-test/llr.yaml; the report then quotes each statement.
#[test]
fn run_folds_a_requirements_document_into_the_report() {
    if !have("c-per-test") {
        return;
    }
    let d = tmp("run-req");
    let b = copy_build("c-per-test", &d);
    let ex = src("c-per-test");
    let (code, _, err) =
        bf(&d, &["run", ex.to_str().unwrap(), "--no-build", "--no-serve", "--out-dir", b.to_str().unwrap(), "--requirements", ex.join("llr.yaml").to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let html = std::fs::read_to_string(b.join("report.html")).unwrap();
    assert!(html.contains("An altitude presented for limiting shall be returned unchanged"), "the statement of LLR-LIM-001 is quoted");
}

// ---------------------------------------------------------------- tags

// tags --help: "-o, --out <OUT>  Where to write the index. Default: <evidence-stem>.tags.json next to
// the evidence (evidence.tags.json for evidence.json), where serve looks for it." README.md:128 "write a
// source definition index (`<evidence-stem>.tags.json`, e.g. `evidence.tags.json`) beside the evidence";
// README.md:140-144. (The docs said `tags.json` until #325 C4; the stem keeps two evidence files in one
// directory from sharing an index, #6.)
#[test]
fn tags_writes_stem_tags_json_beside_the_evidence_by_default() {
    if !have("c") {
        return;
    }
    let d = tmp("tags");
    let ex = repo().join("examples/c");
    let ev = d.join("evidence.json");
    let (code, _, err) = bf(&d, &["ingest", ex.join("build").to_str().unwrap(), "--source-root", ex.to_str().unwrap(), "-o", ev.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = bf(&repo(), &["tags", ev.to_str().unwrap(), "--source-root", ex.to_str().unwrap()]);
    if code != 0 && err.contains("Universal Ctags") {
        butterfly_testsupport::not_applicable("Universal Ctags is not installed");
        return;
    }
    assert_eq!(code, 0, "{err}");
    assert!(d.join("evidence.tags.json").is_file(), "the documented default output: {err}");
    assert!(!d.join("tags.json").exists(), "not a bare tags.json: {err}");
}
