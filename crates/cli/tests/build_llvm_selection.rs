// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Which `llvm-profdata` and `llvm-cov` a `butterfly build` uses.
//!
//! A raw profile carries a format version that `llvm-profdata` must match
//! exactly, so the tools have to be the ones belonging to the compiler that
//! wrote the profile. `butterfly build` went straight to whatever rustup had
//! installed and ignored both the documented `LLVM_BIN` and the compiler's
//! own tools, which on Ubuntu means clang 18 writes version 9 and the merge
//! fails with "no profile can be merged" (#7, review of #20).

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The example, copied so the test never writes into the repository.
fn staged(tag: &str) -> Option<PathBuf> {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    if !src.join("butterfly-build.toml").exists() {
        butterfly_testsupport::fixture_missing("examples/c is not present");
        return None;
    }
    if Command::new("clang").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        butterfly_testsupport::fixture_missing("clang is not installed");
        return None;
    }
    if real_tool("llvm-profdata").is_none() || real_tool("llvm-cov").is_none() {
        butterfly_testsupport::not_applicable("this clang does not ship llvm-profdata/llvm-cov");
        return None;
    }
    let d = butterfly_testsupport::scratch_dir(&format!("llvmsel-{tag}"));
    for f in ["demo.c", "butterfly-build.toml", "butterfly.toml"] {
        let s = src.join(f);
        if s.exists() {
            std::fs::copy(&s, d.join(f)).unwrap();
        }
    }
    Some(d)
}

/// Where this clang keeps the tool, which is the pairing that can read its
/// profiles.
fn real_tool(name: &str) -> Option<PathBuf> {
    let out = Command::new("clang").arg(format!("-print-prog-name={name}")).output().ok()?;
    let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
    (p.is_absolute() && p.is_file()).then_some(p)
}

/// A directory of tools that record having been run, then do the real work.
/// If the build finishes *and* the marker exists, this directory is the one
/// that was chosen -- no need to guess from a log line. It holds an
/// `llvm-objdump` too, as a real LLVM directory does: a configured directory
/// is where ingest's disassembler comes from as well, with no fallback (#246).
fn shim_dir(at: &Path, marker: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(at).unwrap();
    for name in ["llvm-profdata", "llvm-cov", "llvm-objdump"] {
        let real = if name == "llvm-objdump" { butterfly_ingest::objectcode::find_llvm_tool(name, None).ok() } else { real_tool(name) }.unwrap();
        let p = at.join(name);
        std::fs::write(&p, format!("#!/bin/sh\necho '{name}' >> {marker:?}\nexec {real:?} \"$@\"\n", marker = marker.display().to_string(), real = real.display().to_string()))
            .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    at.to_path_buf()
}

fn build(dir: &Path, env: &[(&str, &Path)]) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    // Each case owns its overrides. Inheriting the CI runner's LLVM_BIN
    // would make the default-selection case test an override instead.
    c.env_remove("LLVM_BIN").env_remove("BUTTERFLY_LLVM_BIN");
    c.args(["build", dir.join("butterfly-build.toml").to_str().unwrap()]);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().expect("spawn butterfly")
}

/// The documented override. The README tells people to set `LLVM_BIN` for
/// `examples/c/build.sh`, and CI exports it precisely so the LLVM matching
/// the system clang is used -- and `butterfly build` ignored it.
#[test]
fn llvm_bin_from_the_environment_beats_whatever_rustup_installed() {
    let Some(d) = staged("env") else { return };
    let marker = d.join("used");
    let shim = shim_dir(&d.join("shim"), &marker);

    let out = build(&d, &[("LLVM_BIN", &shim)]);
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(marker.is_file(), "the tools named by LLVM_BIN were never run; butterfly chose its own");
    let used = std::fs::read_to_string(&marker).unwrap();
    assert!(used.contains("llvm-profdata"), "the profile was merged with another llvm-profdata: {used:?}");
    let _ = std::fs::remove_dir_all(d);
}

/// butterfly's own variable keeps working, and keeps precedence over rustup.
#[test]
fn butterfly_llvm_bin_is_honoured_too() {
    let Some(d) = staged("butterflyenv") else { return };
    let marker = d.join("used");
    let shim = shim_dir(&d.join("shim"), &marker);

    let out = build(&d, &[("BUTTERFLY_LLVM_BIN", &shim)]);
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(marker.is_file(), "BUTTERFLY_LLVM_BIN was ignored");
    let _ = std::fs::remove_dir_all(d);
}

/// The config is the most specific statement there is, so it outranks the
/// environment.
#[test]
fn the_config_outranks_the_environment() {
    let Some(d) = staged("config") else { return };
    let from_config = d.join("config-marker");
    let from_env = d.join("env-marker");
    let config_shim = shim_dir(&d.join("config-shim"), &from_config);
    let env_shim = shim_dir(&d.join("env-shim"), &from_env);

    let toml = d.join("butterfly-build.toml");
    let text = std::fs::read_to_string(&toml).unwrap().replace("[coverage]", &format!("[coverage]\nllvm_bin = {:?}", config_shim.display().to_string()));
    std::fs::write(&toml, text).unwrap();

    let out = build(&d, &[("LLVM_BIN", &env_shim)]);
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(from_config.is_file(), "the directory the config named was not used");
    assert!(!from_env.is_file(), "the environment overrode an explicit config setting");
    let _ = std::fs::remove_dir_all(d);
}

/// With nothing configured, the tools must still be ones that can read this
/// compiler's profiles -- which is the whole failure: rustup's LLVM cannot
/// merge clang 18's output at all.
#[test]
fn with_nothing_configured_the_compilers_own_tools_are_used() {
    let Some(d) = staged("default") else { return };
    let out = build(&d, &[]);
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));

    let err = String::from_utf8_lossy(&out.stderr);
    let chosen = err.lines().find_map(|l| l.strip_prefix("llvm     : ")).map(str::trim).expect("the build says which LLVM it chose");
    let expected = real_tool("llvm-profdata").unwrap();
    let expected = expected.parent().unwrap();
    assert_eq!(Path::new(chosen), expected, "the build used LLVM tools that do not belong to the coverage compiler");
    let _ = std::fs::remove_dir_all(d);
}

/// A directory named in the config is the operator's choice for every LLVM
/// tool, the disassembler included. One without `llvm-objdump` is an error
/// that says so, not a quiet switch to some other copy (#246).
#[test]
fn a_configured_llvm_dir_without_llvm_objdump_is_an_error_not_a_fallback() {
    use std::os::unix::fs::PermissionsExt;
    let Some(d) = staged("cfg-no-objdump") else { return };
    let shim = d.join("partial");
    std::fs::create_dir_all(&shim).unwrap();
    for name in ["llvm-profdata", "llvm-cov"] {
        let p = shim.join(name);
        std::fs::write(&p, format!("#!/bin/sh\nexec {:?} \"$@\"\n", real_tool(name).unwrap().display().to_string())).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let toml = d.join("butterfly-build.toml");
    let text = std::fs::read_to_string(&toml).unwrap().replace("[coverage]", &format!("[coverage]\nllvm_bin = {:?}", shim.display().to_string()));
    std::fs::write(&toml, text).unwrap();

    let out = build(&d, &[]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a disassembler nobody chose was used: {err}");
    assert!(err.contains("llvm-objdump") && err.contains("does not fall back"), "{err}");
    let _ = std::fs::remove_dir_all(d);
}
