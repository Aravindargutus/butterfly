// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly run` drives the whole pipeline, and its value is that it stops
//! at the first phase that would make everything after it untrustworthy.

use std::fs;
use std::path::Path;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_butterfly")
}

/// The same temp-directory idiom as the other CLI tests: a fresh directory
/// under a random name, so a previous run -- or anyone else -- cannot leak
/// into this one (#344).
fn tmp(tag: &str) -> std::path::PathBuf {
    butterfly_testsupport::scratch_dir(&format!("run-{tag}"))
}

fn project(dir: &Path, script: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.c"), "int clamp(int v,int hi){ if(v>hi) return hi; return v; }\nint main(void){ return clamp(1,10); }\n").unwrap();
    let s = dir.join("build.sh");
    fs::write(&s, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&s, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// The failure that motivated the command: a build script that dies without
/// printing anything. The run must stop there and say so, not carry on and
/// blame the missing coverage file three phases later.
#[test]
fn a_silent_build_failure_stops_the_run() {
    let t = tmp("silent");
    project(&t, "#!/bin/sh\nexit 1\n");
    let out = Command::new(bin()).arg("run").arg(&t).arg("--no-serve").output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a failed build must fail the run: {err}");
    assert!(err.contains("without printing anything"), "must name the silence: {err}");
    assert!(err.contains("/dev/null"), "must suggest the usual cause: {err}");
    assert!(err.contains("FAIL") && err.contains("build"), "summary must mark the phase: {err}");
    // and nothing downstream should have been attempted
    assert!(!err.contains("[3/4] ingest"), "must not reach ingest: {err}");
}

#[test]
fn a_missing_build_script_says_what_to_do() {
    let t = tmp("noscript");
    fs::create_dir_all(t.join("src")).unwrap();
    let out = Command::new(bin()).arg("run").arg(&t).arg("--no-serve").output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(err.contains("--no-build"), "must offer the escape hatch: {err}");
    assert!(err.contains("templates/build.sh"), "must point at the template: {err}");
}

/// --no-build is honest about what it means: the evidence describes whatever
/// was lying in the directory, not this run.
#[test]
fn no_build_is_recorded_as_an_assumption() {
    let t = tmp("nobuild");
    project(&t, "#!/bin/sh\nexit 0\n");
    // Nothing has been built, so this must fail -- but with the reason, not
    // a bare ENOENT from somewhere inside discovery.
    let out = Command::new(bin()).arg("run").arg(&t).arg("--no-build").arg("--no-serve").output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("skipped"), "{err}");
    assert!(err.contains("--no-build says to use an existing build"), "must explain: {err}");
    assert!(!err.contains("os error 2"), "must not leak a raw errno: {err}");

    // With a build directory present, the skip is recorded as the assumption
    // it is: the evidence describes an earlier build, not this run.
    fs::create_dir_all(t.join("build")).unwrap();
    fs::write(t.join("build/coverage.json"), "{}").unwrap();
    let out = Command::new(bin()).arg("run").arg(&t).arg("--no-build").arg("--no-serve").output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("earlier build, not this run"), "must not let this pass silently: {err}");
}

/// The build script's own stderr is the thing most worth seeing, and the bug
/// that started this was it being discarded. It must reach the terminal.
#[test]
fn the_build_scripts_diagnostics_are_shown() {
    let t = tmp("stderr");
    project(&t, "#!/bin/sh\necho 'a distinctive diagnostic' >&2\nexit 1\n");
    let out = Command::new(bin()).arg("run").arg(&t).arg("--no-serve").output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("a distinctive diagnostic"), "stderr must pass through: {err}");
    // it printed something, so the "silent" advice would be wrong here
    assert!(!err.contains("without printing anything"), "must not claim silence: {err}");

    // stdout-only output is still output: the silence advice below a screen
    // of the script's own stdout would be a diagnostic that lies
    let t2 = tmp("stdout-only");
    project(&t2, "#!/bin/sh\necho 'progress on stdout'\nexit 1\n");
    let out = Command::new(bin()).arg("run").arg(&t2).arg("--no-serve").output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("progress on stdout"), "stdout must pass through: {err}");
    assert!(!err.contains("without printing anything"), "stdout counts as printing: {err}");
}

#[test]
fn a_build_script_that_is_not_executable_says_so() {
    let t = tmp("noexec");
    fs::create_dir_all(t.join("src")).unwrap();
    fs::write(t.join("build.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(t.join("build.sh"), fs::Permissions::from_mode(0o644)).unwrap();
    }
    let out = Command::new(bin()).arg("run").arg(&t).arg("--no-serve").output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(err.contains("chmod +x"), "must name the fix: {err}");
}

/// Inserting a subcommand above another one silently stole its doc comment:
/// clap took both paragraphs as `run`'s description and left `doctor` with a
/// blank one. `--help` is the first thing anyone reads, so every subcommand
/// must have its own summary and no subcommand may wear another's.
#[test]
fn every_subcommand_describes_itself() {
    let out = Command::new(bin()).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);

    // the commands listing runs to the options block
    let listing = help.split("Options:").next().unwrap_or(&help);
    let mut named = 0;
    for line in listing.lines() {
        // "  <name>   <summary>", two-space indent, from the Commands: block
        let Some(rest) = line.strip_prefix("  ") else { continue };
        if rest.starts_with(' ') || rest.is_empty() {
            continue;
        }
        let mut it = rest.splitn(2, char::is_whitespace);
        let name = it.next().unwrap_or("");
        if !name.chars().all(|c| c.is_ascii_lowercase() || c == '-') || name.is_empty() {
            continue;
        }
        let summary = it.next().unwrap_or("").trim();
        assert!(!summary.is_empty(), "`{name}` has no description in --help");
        named += 1;
    }
    assert!(named >= 10, "expected the full command listing, saw {named}");

    // and the two that actually collided stay distinct
    let run_line = listing.lines().find(|l| l.trim_start().starts_with("run ")).expect("run is listed");
    assert!(!run_line.contains("Check a build directory"), "run is wearing doctor's description: {run_line}");
}

/// `butterfly run /path/to/project` from somewhere else must use the
/// *project's* sidecar -- not silently drop its recorded reviews, and not
/// pick up whatever resolutions.json happens to sit where the command was
/// typed. The caller-side file here is deliberately malformed: if any code
/// path consults discovery again, the strict loader turns this test red.
#[test]
fn run_from_outside_the_project_uses_the_projects_reviews() {
    let t = tmp("outside");
    let caller = t.join("somewhere-else");
    let project = t.join("project");
    fs::create_dir_all(&caller).unwrap();
    fs::create_dir_all(project.join("build")).unwrap();
    // a sidecar where the command is typed, wrong in both senses
    fs::write(caller.join("resolutions.json"), "{ not json").unwrap();

    // the smallest ingestable build: one gcov document over one real source
    fs::write(project.join("a.c"), "int f(void){return 1;}\n").unwrap();
    let cov = format!(
        r#"{{"format_version":"2","gcc_version":"16","current_working_directory":"{}",
          "files":[{{"file":"a.c","functions":[{{"name":"f","start_line":1,"end_line":1,"execution_count":1}}],
            "lines":[{{"line_number":1,"count":1}}]}}]}}"#,
        project.display()
    );
    fs::write(project.join("build/coverage.json"), cov).unwrap();
    let review = serde_json::json!({ "schema": "butterfly-resolutions/1", "entries": [{
        "key": "line:0:1", "category": "a", "justification": "recorded in the project",
        "author": "outside-test", "date": "2026-09-07" }] });
    fs::write(project.join("resolutions.json"), review.to_string()).unwrap();

    let run = |proj: &std::path::Path| Command::new(bin()).current_dir(&caller).arg("run").arg(proj).args(["--no-build", "--no-serve", "--force"]).output().unwrap();
    let out = run(&project);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "the pipeline must complete: {err}");
    let expected = project.join("resolutions.json");
    assert!(err.contains(&format!("resolutions: {}", expected.display())), "the project's reviews must be the ones read: {err}");
    assert!(!err.contains("no resolutions sidecar yet"), "its reviews must not be silently dropped: {err}");

    // and with no project sidecar at all, the caller's file must still be
    // ignored: first run means no reviews, not somebody else's
    fs::remove_file(project.join("resolutions.json")).unwrap();
    let out = run(&project);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "a first run has no sidecar and that is fine: {err}");
    assert!(err.contains("no resolutions sidecar yet"), "{err}");
}

/// A review naming nothing in this evidence must be *reported* by `run`,
/// exactly as it is by `report`, `check` and `impact`.
///
/// The pipeline loaded its sidecar without handing over the evidence, so
/// the staleness check had nothing to compare against and silently passed
/// every key. A gap key embeds an address, so a rebuild makes stale reviews
/// ordinary -- and `run` is the command that *does* the rebuild, which makes
/// it the one place the warning matters most (#93).
#[test]
fn run_reports_a_review_that_names_nothing() {
    let t = tmp("stale");
    let project = t.join("project");
    fs::create_dir_all(project.join("build")).unwrap();
    fs::write(project.join("a.c"), "int f(void){return 1;}\n").unwrap();
    let cov = format!(
        r#"{{"format_version":"2","gcc_version":"16","current_working_directory":"{}",
          "files":[{{"file":"a.c","functions":[{{"name":"f","start_line":1,"end_line":1,"execution_count":1}}],
            "lines":[{{"line_number":1,"count":1}}]}}]}}"#,
        project.display()
    );
    fs::write(project.join("build/coverage.json"), cov).unwrap();

    // one real review and one that names a line this evidence does not have
    let review = serde_json::json!({ "schema": "butterfly-resolutions/1", "entries": [
        { "key": "line:0:1", "category": "a", "justification": "a real one",
          "author": "stale-test", "date": "2026-09-11" },
        { "key": "line:0:999", "category": "a", "justification": "left over from an older build",
          "author": "stale-test", "date": "2026-09-11" }] });
    fs::write(project.join("resolutions.json"), review.to_string()).unwrap();

    let out = Command::new(bin()).arg("run").arg(&project).args(["--no-build", "--no-serve", "--force"]).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);

    // reported, and not fatal: staleness is the ordinary result of a rebuild
    assert!(out.status.success(), "a stale review must not fail the run: {err}");
    assert!(err.contains("name nothing in this evidence"), "the run must say so: {err}");
    assert!(err.contains("line:0:999"), "and name the offending key: {err}");
    assert!(!err.contains("line:0:1,") && !err.contains(": line:0:1"), "without accusing the valid one: {err}");
}

/// A symlinked `--out-dir` (`build`) must be refused before clearing or writing
/// outputs, leaving the symlink target's files untouched.
#[cfg(unix)]
#[test]
fn a_symlinked_out_dir_is_refused_and_its_target_is_untouched() {
    let t = tmp("symlink-outdir");
    let project = t.join("project");
    let victim = t.join("victim");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&victim).unwrap();
    fs::write(victim.join("evidence.json"), "PRECIOUS_EVIDENCE").unwrap();
    fs::write(victim.join("report.html"), "PRECIOUS_REPORT").unwrap();
    std::os::unix::fs::symlink(&victim, project.join("build")).unwrap();

    let out = Command::new(bin()).arg("run").arg(&project).args(["--no-build", "--no-serve"]).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a symlinked build dir must be refused: {err}");
    assert!(err.contains("symlink"), "{err}");
    assert_eq!(fs::read_to_string(victim.join("evidence.json")).unwrap(), "PRECIOUS_EVIDENCE");
    assert_eq!(fs::read_to_string(victim.join("report.html")).unwrap(), "PRECIOUS_REPORT");
}

/// `--build-script scripts/build.sh` must execute with `cwd` set to the project
/// directory `dir`, not `dir/scripts`.
#[cfg(unix)]
#[test]
fn a_build_script_in_a_subdirectory_runs_in_the_project_root() {
    use std::os::unix::fs::PermissionsExt;
    let t = tmp("subdir-script");
    let project = t.join("project");
    fs::create_dir_all(project.join("scripts")).unwrap();
    let script = project.join("scripts/build.sh");
    // Fail after printing pwd so we verify the working directory without needing a toolchain.
    fs::write(&script, "#!/bin/sh\necho \"PWD=$(pwd)\" >&2\nexit 1\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let out = Command::new(bin()).arg("run").arg(&project).args(["--build-script", "scripts/build.sh", "--no-serve"]).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    let expected = fs::canonicalize(&project).unwrap();
    assert!(err.contains(&format!("PWD={}", expected.display())), "script must run in project root {}, got:\n{err}", expected.display());
}
