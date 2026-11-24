// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the checks `doctor` and `ingest` make on a
//! build directory before trusting it (area A). Expected values come from
//! README.md "Checking a build before you trust it (`butterfly doctor`)",
//! README.md:976-1042.

use butterfly_ingest::preflight::{check, instrumented_with, render, systems_named, Coverage, CoverageState, Level};
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("req-a-preflight-{tag}"))
}

/// Compile one C file to an object with the host's C compiler, or `None`
/// where there is none (the fixtures need one, so a development host has it).
fn object_of(dir: &Path, name: &str, c: &str) -> Option<Vec<u8>> {
    let src = dir.join(format!("{name}.c"));
    let obj = dir.join(format!("{name}.o"));
    std::fs::write(&src, c).unwrap();
    let ok = Command::new("cc").arg("-c").arg(&src).arg("-o").arg(&obj).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: no working `cc` on this host");
        return None;
    }
    Some(std::fs::read(&obj).unwrap())
}

/// README.md:1017-1018 "analysing the instrumented binary | `__llvm_covmap`
/// section | `__gcov0.<fn>` symbols" and README.md:1035-1039 "A GCC
/// `--coverage` binary has no such section: it carries `__gcov0.<function>`
/// counter symbols instead ... Both are recognised now".
#[test]
fn a_gcov_counter_symbol_marks_an_object_as_gcc_instrumented() {
    let d = scratch("gcov0");
    let Some(instr) = object_of(&d, "instr", "int counter __asm__(\"__gcov0.main\") = 1;\nint main(void) { return counter; }\n") else { return };
    let Some(plain) = object_of(&d, "plain", "int main(void) { return 0; }\n") else { return };
    let f = object::File::parse(&*instr).unwrap();
    assert_eq!(instrumented_with(&f).map(|t| format!("{t:?}")), Some("Gcc".to_string()), "a __gcov0.<fn> symbol is GCC instrumentation");
    let f = object::File::parse(&*plain).unwrap();
    assert!(instrumented_with(&f).is_none(), "and an ordinary object is not instrumented");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:986-994 the rendered form -- the level word, the finding, a
/// `why:` line, then one line per build system's fix -- and README.md:1003-1006
/// "Each finding names the fix for **`build.sh`, make, CMake and Bazel**".
#[test]
fn a_finding_renders_as_the_readme_shows_and_names_every_build_system() {
    let d = scratch("render");
    std::fs::write(d.join("build-info.txt"), "compiler=gcc\n").unwrap();
    std::fs::write(d.join("app"), "not really an object").unwrap();
    // The program has decisions, and the export carries no branch records for
    // them: gcov was run without -b.
    let cov = Some(Coverage::Read(CoverageState { is_gcov: true, has_branches: false, has_raw_branches: false, has_mcdc: false, has_decisions: true }));
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), cov, &[]);
    let wrong: Vec<_> = fs.iter().filter(|f| f.level == Level::Wrong).collect();
    assert!(!wrong.is_empty(), "a gcov export with no branch records is WRONG (README.md:988)");
    let out = render(&fs, true);
    let first = wrong[0];
    assert!(out.lines().any(|l| l.starts_with("WRONG") && l.contains(&first.what)), "{out}");
    assert!(out.lines().any(|l| l.trim_start().starts_with("why:") && l.contains(first.why)), "{out}");
    for (system, fix) in &first.fixes {
        assert!(out.lines().any(|l| l.trim_start().starts_with(system) && l.contains(fix.as_str())), "a line for {system}: {out}");
    }
    let named = systems_named(&fs);
    assert!(!named.is_empty(), "the summary names the build systems the fixes are for");

    // An empty set renders as a sentence, never as nothing.
    assert!(!render(&[], false).trim().is_empty());
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:996-1001 the three levels in order: WRONG, then WEAKER, then
/// note ("findings come in three levels, and the distinction is the point").
#[test]
fn findings_render_worst_first() {
    let d = scratch("order");
    // no build-info, no .su, a gcov export without branches: several levels at once
    std::fs::write(d.join("app"), "x").unwrap();
    let cov = Some(Coverage::Read(CoverageState { is_gcov: true, has_branches: false, has_raw_branches: false, has_mcdc: false, has_decisions: true }));
    let fs = check(&d, &[d.join("app")], &[], Some(&d.join("coverage.json")), cov, &[]);
    let levels: Vec<Level> = fs.iter().map(|f| f.level).collect();
    let mut sorted = levels.clone();
    sorted.sort();
    assert_eq!(levels, sorted, "worst first: {levels:?}");
    assert_eq!(Level::Wrong.word(), "WRONG");
    assert_eq!(Level::Weaker.word(), "WEAKER");
    assert_eq!(Level::Note.word(), "note");
    let _ = std::fs::remove_dir_all(d);
}
