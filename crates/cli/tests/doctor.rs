// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly doctor` over real build directories.
//!
//! Runs only when the examples have been built; skipped otherwise.

use std::path::PathBuf;
use std::process::Command;

fn doctor(dir: &std::path::Path) -> (Option<i32>, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_butterfly")).arg("doctor").arg(dir).output().expect("doctor");
    (o.status.code(), String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr))
}

fn example(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name).join("build");
    if p.join("coverage.json").exists() {
        Some(p)
    } else {
        butterfly_testsupport::fixture_missing(&format!("build examples/{name} first"));
        None
    }
}

/// A complete clang build passes, and says which instrumented binary it
/// ignored rather than staying quiet about it.
#[test]
fn a_complete_clang_build_passes_and_names_what_it_skipped() {
    let Some(d) = example("c") else { return };
    let (code, out) = doctor(&d);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("instrumented object(s) ignored"), "{out}");
    assert!(out.contains("demo-instr"), "names it: {out}");
}

/// The GCC fixture, which exists only where a real GCC does: CI builds it
/// on Linux only, because a Mac's `gcc` is Apple clang and its gcov cannot
/// write the JSON the front end reads.
///
/// Absent here, this is a **failure, not a skip**. The two tests below are
/// compiled only on Linux precisely so that this fixture is guaranteed --
/// so a missing one means the fixture contract broke, and skipping would
/// let the gcov front end go untested while the suite still said "ok"
/// (#89).
#[cfg(target_os = "linux")]
fn gcc_example() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-gcc/build");
    assert!(p.join("coverage.json").exists(), "examples/c-gcc is not built, and on this host it is mandatory: run ./examples/c-gcc/build.sh (#89)");
    p
}

/// The GCC example: proves the instrumented-object detection is not
/// clang-only. A `gcc --coverage` binary has no `__llvm_covmap` section at
/// all -- only `__gcov0.<fn>` counter symbols -- so a section-only test waved
/// it through and butterfly analysed the counters as the delivered code.
#[cfg(target_os = "linux")]
#[test]
fn a_gcc_instrumented_binary_is_recognised_and_not_analysed() {
    let d = gcc_example();
    let (code, out) = doctor(&d);
    assert_eq!(code, Some(0), "the analysis binary is there, so this passes: {out}");
    assert!(out.contains("instrumented object(s) ignored"), "{out}");
    assert!(out.contains("limits-instr"), "the GCC-instrumented binary is named: {out}");
    // and the one actually chosen is the clean build
    assert!(out.contains("object    :") && out.contains("/limits"), "{out}");
}

/// A directory holding only the instrumented binary is the failure this
/// command exists for: it must exit 3 rather than produce evidence.
#[cfg(target_os = "linux")]
#[test]
fn only_an_instrumented_binary_exits_three_with_a_fix_per_build_system() {
    let src = gcc_example();
    let d = butterfly_testsupport::scratch_dir("doctor-instr");
    for f in ["coverage.json", "limits-instr"] {
        let from = src.join(f);
        if from.exists() {
            std::fs::copy(&from, d.join(f)).unwrap();
        }
    }

    let (code, out) = doctor(&d);
    assert_eq!(code, Some(3), "this must fail the check: {out}");
    assert!(out.contains("WRONG"), "{out}");
    for system in ["build.sh", "make", "cmake", "bazel"] {
        assert!(out.contains(system), "a fix for {system}: {out}");
    }
    let _ = std::fs::remove_dir_all(&d);
}
