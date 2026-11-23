// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The header-inline, per-test and library-deliverable adapters, held to one
//! build transaction (#286).
//!
//! #245 made `butterfly ingest` hold a build directory to what its
//! `build-info.txt` declares. That only protects evidence the adapter
//! finished: the checks have nothing to hold a directory to when the adapter
//! stopped before writing `build-info.txt`, and a compiler record is only
//! worth something if it names the compiler that ran.
//!
//! These run the real scripts in a private copy of `examples/`, with stand-in
//! compilers that hand the work to the host's clang; nothing here touches the
//! examples' own build directories.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").canonicalize().unwrap()
}

fn scratch(tag: &str) -> PathBuf {
    let d = butterfly_testsupport::scratch_dir(&format!("adapter-{tag}"));
    d.canonicalize().unwrap()
}

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// The example's sources and scripts, without anything a build left behind.
fn copy_sources(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type().unwrap().is_dir() {
            if !name.starts_with("build") && name != "variants" {
                copy_sources(&src, &dst);
            }
        } else {
            fs::copy(&src, &dst).unwrap();
        }
    }
}

/// A private `examples/` holding the named examples and the shared scripts.
fn stage(d: &Path, names: &[&str]) -> PathBuf {
    let ex = d.join("examples");
    for n in names {
        copy_sources(&examples().join(n), &ex.join(n));
    }
    for f in ["adapter.sh", "llvm-env.sh", "rust-llvm-env.sh"] {
        fs::copy(examples().join(f), ex.join(f)).unwrap();
    }
    ex
}

/// The host's clang and clang++ by absolute path, or None where there are none.
fn host_compilers() -> Option<(PathBuf, PathBuf)> {
    let find = |name: &str| -> Option<PathBuf> {
        let out = Command::new("sh").arg("-c").arg(format!("command -v {name}")).output().ok()?;
        let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        (out.status.success() && p.is_absolute()).then_some(p)
    };
    match (find("clang"), find("clang++")) {
        (Some(c), Some(cxx)) => Some((c, cxx)),
        _ => {
            butterfly_testsupport::not_applicable("no clang and clang++ to stand behind the stand-in compilers");
            None
        }
    }
}

/// `examples/cpp-lib` given a compiler whose path has a space in it. The
/// script used to expand `$CXX` unquoted in two places -- the instrumented
/// build and the analysed image's link -- so the path split there: the first
/// word ran as the compiler, and the rest became its arguments. When a
/// program sits at that first word, it builds the coverage and links the
/// image, while `compile_each` and `build-info.txt` name the compiler the
/// operator asked for. The stand-in at the first word only records that it
/// ran, and then does the work, so the build still succeeds -- which is what
/// made this invisible.
#[test]
fn cpp_lib_runs_only_the_compiler_it_records() {
    let Some((_, cxx)) = host_compilers() else { return };
    let d = scratch("cxx-space");
    let ex = stage(&d, &["cpp-lib"]);
    let tools = d.join("tools");
    let named = tools.join("cxx real");
    let log = d.join("first-word.log");
    script(&named, &format!("exec '{}' \"$@\"", cxx.display()));
    script(&tools.join("cxx"), &format!("echo \"$*\" >> '{}'\nshift\nexec '{}' \"$@\"", log.display(), cxx.display()));

    let out = Command::new("sh").arg(ex.join("cpp-lib/build.sh")).env("CXX", &named).output().unwrap();
    assert!(out.status.success(), "the build fails: {}", String::from_utf8_lossy(&out.stderr));
    let info = fs::read_to_string(ex.join("cpp-lib/build/build-info.txt")).unwrap();
    assert!(info.lines().any(|l| l == format!("compiler_path={}", named.display())), "records the compiler it was given: {info}");
    let ran = fs::read_to_string(&log).unwrap_or_default();
    assert!(ran.is_empty(), "{} ran part of the build that build-info.txt says {} did:\n{ran}", tools.join("cxx").display(), named.display());
    let _ = fs::remove_dir_all(d);
}

/// Every adapter here writes `build-info.txt` last. A step that fails after
/// the analysed image is linked -- here, a compiler that silently ignores
/// `-fstack-usage`, which `collect_su` catches -- stopped the build, but used
/// to leave the coverage export and the linked image where the documented
/// `butterfly ingest examples/<name>/build` reads them. With no
/// `build-info.txt`, ingest has no declared mode to hold them to, and accepted
/// them without a caveat: evidence of a build that failed, with every frame
/// estimated from its prologue. The failed output is now set aside under a
/// name that says so, and the documented directory does not exist.
#[test]
fn a_build_that_fails_leaves_nothing_where_its_evidence_is_read() {
    let Some((cc, cxx)) = host_compilers() else { return };
    let d = scratch("failed");
    let ex = stage(&d, &["c-header-inline", "c-per-test", "c-lib", "cpp-lib"]);
    // Compiles everything, but never with -fstack-usage: the build has asked
    // for stack usage, and the compiler writes none.
    let drop_su = |real: &Path| format!("for a; do shift; [ \"$a\" = -fstack-usage ] || set -- \"$@\" \"$a\"; done\nexec '{}' \"$@\"", real.display());
    let tools = d.join("tools");
    script(&tools.join("clang"), &drop_su(&cc));
    script(&tools.join("clang++"), &drop_su(&cxx));

    for (script_path, out_dir) in [
        ("c-header-inline/build.sh", "c-header-inline/build"),
        ("c-per-test/build.sh", "c-per-test/build"),
        ("c-lib/build.sh", "c-lib/build"),
        ("c-lib/build-lib.sh", "c-lib/build-lib"),
        ("cpp-lib/build.sh", "cpp-lib/build"),
    ] {
        let out = Command::new("sh").arg(ex.join(script_path)).env("CC", tools.join("clang")).env("CXX", tools.join("clang++")).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{script_path}: succeeded without any .su");
        assert!(stderr.contains("no .su files were written"), "{script_path}: stops for the missing .su: {stderr}");
        let out_dir = ex.join(out_dir);
        assert!(
            !out_dir.exists(),
            "{script_path}: the failed build left {} for ingest to read: {:?}",
            out_dir.display(),
            fs::read_dir(&out_dir).map(|r| r.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
        );
        let kept = PathBuf::from(format!("{}.incomplete", out_dir.display()));
        assert!(kept.join("coverage.json").exists(), "{script_path}: keeps what it made for diagnosis, under {}", kept.display());
    }
    let _ = fs::remove_dir_all(d);
}
