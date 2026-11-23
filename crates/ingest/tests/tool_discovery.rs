// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! How the shipped build adapters find the tools that shape the evidence
//! (#289).
//!
//! A profile is only readable by the LLVM that wrote it, and a `.gcda` by
//! the gcov of the GCC that wrote it. The adapters ask the compiler for its
//! own tools -- but `clang -print-prog-name=NAME` does not only look beside
//! the compiler: when its own directory has no NAME it falls back to PATH and
//! still answers with an absolute path. `examples/llvm-env.sh` took any
//! absolute answer as proof of a pair, so a clang with no LLVM tools of its
//! own was paired with whatever LLVM came first on PATH -- a different
//! version, or a directory someone put there. And `examples/c-gcc` took the
//! first `gcov-NN` on PATH without asking which GCC it came from.
//!
//! These run the real scripts against stand-in compilers in a private
//! directory; nothing here touches the examples' own build directories.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").canonicalize().unwrap()
}

fn scratch(tag: &str) -> PathBuf {
    let d = butterfly_testsupport::scratch_dir(&format!("tools-{tag}"));
    d.canonicalize().unwrap()
}

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A clang stand-in that says it is installed in `installed` and answers
/// `-print-prog-name` with `answer_dir/NAME` -- exactly what a real clang
/// answers when `installed` has no NAME and `answer_dir` is on PATH.
fn fake_clang(installed: &Path, answer_dir: &Path) -> PathBuf {
    let cc = installed.join("clang");
    script(
        &cc,
        &format!(
            r#"case "$1" in
  -print-prog-name=*) echo "{}/${{1#-print-prog-name=}}" ;;
  --version|-v) echo "clang version 99.0.0"; echo "InstalledDir: {}" ;;
  *) exit 1 ;;
esac"#,
            answer_dir.display(),
            installed.display()
        ),
    );
    for dir in [installed, answer_dir] {
        for t in ["llvm-profdata", "llvm-cov"] {
            script(&dir.join(t), "exit 0");
        }
    }
    cc
}

/// Source llvm-env.sh the way the example builds do, and print what it chose.
fn llvm_env(cc: &Path, llvm_bin: Option<&Path>) -> Output {
    let mut c = Command::new("sh");
    c.arg("-c").arg(r#"LLVM_FROM="$1"; . "$2/llvm-env.sh"; echo "LLVM_PROFDATA=$LLVM_PROFDATA""#).arg("sh").arg(cc).arg(examples());
    c.env_remove("LLVM_BIN");
    if let Some(b) = llvm_bin {
        c.env("LLVM_BIN", b);
    }
    c.output().unwrap()
}

#[test]
fn llvm_env_takes_the_tools_installed_beside_the_compiler() {
    let d = scratch("paired");
    let own = d.join("llvm-own/bin");
    let cc = fake_clang(&own, &own);
    let out = llvm_env(&cc, None);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains(&format!("LLVM_PROFDATA={}/llvm-profdata", own.display())), "{stdout}");
    let _ = fs::remove_dir_all(d);
}

/// The compiler's answer came from PATH, not from its own installation:
/// nothing says that LLVM wrote nor can read this compiler's profiles.
#[test]
fn llvm_env_refuses_a_profile_reader_the_compiler_only_found_on_path() {
    let d = scratch("path-fallback");
    let own = d.join("llvm-own/bin");
    let elsewhere = d.join("shadow");
    let cc = fake_clang(&own, &elsewhere);
    let out = llvm_env(&cc, None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a reader from outside the compiler's installation was accepted as its pair: {}", String::from_utf8_lossy(&out.stdout));
    assert!(stderr.contains(&elsewhere.display().to_string()) && stderr.contains("LLVM_BIN"), "names what it found and the override: {stderr}");
    let _ = fs::remove_dir_all(d);
}

/// LLVM_BIN is the operator saying which LLVM reads the profile; it is not
/// second-guessed (and write_build_info records it).
#[test]
fn llvm_env_honours_an_explicit_llvm_bin() {
    let d = scratch("override");
    let own = d.join("llvm-own/bin");
    let elsewhere = d.join("chosen");
    let cc = fake_clang(&own, &elsewhere);
    let out = llvm_env(&cc, Some(&elsewhere));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains(&format!("LLVM_PROFDATA={}/llvm-profdata", elsewhere.display())));
    let _ = fs::remove_dir_all(d);
}

/// The shell adapters record the tools that produced `coverage.json` under
/// the keys `butterfly build` uses (#298): the compiler that wrote the
/// profile, and the `llvm-profdata` and `llvm-cov` that read it, each by the
/// path that ran and by its version -- the line of `--version` that carries
/// one, as butterfly reads it, not the URL banner LLVM prints first.
#[test]
fn the_shell_adapters_record_the_coverage_toolchain() {
    let d = scratch("covtools");
    let own = d.join("llvm-own/bin");
    let cc = fake_clang(&own, &own);
    for t in ["llvm-profdata", "llvm-cov"] {
        script(&own.join(t), &format!("[ \"$1\" = --version ] || exit 0\necho 'LLVM (http://llvm.org/):'\necho '  {t} LLVM version 99.1.0 '\necho '  Optimized build.'"));
    }
    let out = Command::new("sh")
        .arg("-c")
        .arg(r#"LLVM_FROM="$1"; . "$2/adapter.sh"; . "$2/llvm-env.sh"; write_build_info "$3/build-info.txt" "$1" "-fprofile-instr-generate" "-g" && cat "$3/build-info.txt""#)
        .arg("sh")
        .arg(&cc)
        .arg(examples())
        .arg(&d)
        .env_remove("LLVM_BIN")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let info = String::from_utf8_lossy(&out.stdout).into_owned();
    for line in [
        "compiler_instr=clang version 99.0.0".to_string(),
        format!("compiler_instr_path={}", cc.display()),
        "llvm_profdata=llvm-profdata LLVM version 99.1.0".to_string(),
        format!("llvm_profdata_path={}/llvm-profdata", own.display()),
        "llvm_cov=llvm-cov LLVM version 99.1.0".to_string(),
        format!("llvm_cov_path={}/llvm-cov", own.display()),
    ] {
        assert!(info.lines().any(|l| l == line), "build-info.txt has `{line}`:\n{info}");
    }
    let _ = fs::remove_dir_all(d);
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type().unwrap().is_dir() {
            if e.file_name() != "build" {
                copy_tree(&src, &dst);
            }
        } else {
            fs::copy(&src, &dst).unwrap();
        }
    }
}

/// `examples/c-gcc` with a gcov of another GCC release first on PATH. A
/// `.gcda` carries the version of the GCC that wrote it, and gcov's
/// complaint about a mismatch went to the /dev/null its stderr is sent to.
/// The build must stop before it reads anything.
#[test]
fn c_gcc_refuses_a_gcov_from_another_gcc_release() {
    let Some(clang) = ["clang", "cc"].into_iter().find(|c| Command::new(c).arg("--version").output().is_ok_and(|o| o.status.success())) else {
        butterfly_testsupport::not_applicable("no C compiler to stand behind the fake gcc");
        return;
    };
    let d = scratch("gcov");
    let ex = d.join("examples");
    copy_tree(&examples().join("c-gcc"), &ex.join("c-gcc"));
    fs::copy(examples().join("adapter.sh"), ex.join("adapter.sh")).unwrap();
    let bin = d.join("bin");
    // A GCC 99.1 whose compiling is done by the host's C compiler: enough
    // for the script to reach its gcov, which is what is under test.
    script(
        &bin.join("gcc-99"),
        &format!(
            r#"case "$1" in
  --version) echo "gcc-99 (Test GCC 99.1.0) 99.1.0" ;;
  -print-prog-name=*) echo "${{1#-print-prog-name=}}" ;;
  -dumpmachine) {clang} -dumpmachine ;;
  *) exec {clang} "$@" ;;
esac"#
        ),
    );
    script(
        &bin.join("gcov-99"),
        r#"case "$1" in
  --version) echo "gcov-99 (Test GCC 98.3.0) 98.3.0" ;;
  *) exit 0 ;;
esac"#,
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let out = Command::new("sh").arg(ex.join("c-gcc/build.sh")).env("PATH", path).env("CC", "gcc-99").env_remove("GCOV").output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "built with a gcov from GCC 98.3.0 for a GCC 99.1.0 build:\n{}", String::from_utf8_lossy(&out.stdout));
    assert!(stderr.contains("98.3.0") && stderr.contains("99.1.0"), "names both releases: {stderr}");
    let _ = fs::remove_dir_all(d);
}
