// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for ingest's optional inputs (area A): the
//! build-info file, `--trace` and `--stack-usage`, each read under the bounds
//! README.md "Security posture" sets. Run against examples/c's build, which
//! must have been built (`./examples/c/build.sh`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture() -> Option<PathBuf> {
    let b = root().join("examples/c/build");
    if !b.join("coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run ./examples/c/build.sh first");
        return None;
    }
    Some(b)
}

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("req-a-ingest-{tag}"))
}

fn ingest(build: &Path, out: &Path, extra: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.arg("ingest").arg(build).arg("--source-root").arg(root().join("examples/c")).arg("-o").arg(out).args(extra);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().expect("spawn butterfly")
}

fn padded(path: &Path, first: &str, mb: usize) {
    let mut s = String::from(first);
    while s.len() < mb * (1 << 20) + 1024 {
        s.push_str("0x0\n");
    }
    std::fs::write(path, s).unwrap();
}

/// README.md:286-287 "Optional `build-info.txt` with `key=value` lines
/// (compiler, flags, git hash), recorded verbatim in provenance" -- a value
/// keeps everything after the first `=`, `=` signs and spaces included.
#[test]
fn build_info_is_recorded_verbatim_in_provenance() {
    let Some(build) = fixture() else { return };
    let d = scratch("binfo");
    let info = d.join("build-info.txt");
    std::fs::write(&info, "compiler=clang version 99 (custom build)\nflags=-O0 -g -DMODE=fast\ngit=0123abcd\n").unwrap();
    let out = d.join("evidence.json");
    let o = ingest(&build, &out, &["--build-info", info.to_str().unwrap()], &[]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let ev: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let bi = &ev["provenance"]["buildInfo"];
    assert_eq!(bi["compiler"], "clang version 99 (custom build)", "{bi}");
    assert_eq!(bi["flags"], "-O0 -g -DMODE=fast", "everything after the first `=`: {bi}");
    assert_eq!(bi["git"], "0123abcd", "{bi}");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1583-1596 "Every artifact `ingest` reads -- ... traces, the
/// auxiliary files -- is sized before it is read ... fails with a sentence
/// naming the file ... Every refusal names the variable that would have let
/// it through."
#[test]
fn an_oversized_trace_stack_file_or_build_info_is_refused_naming_its_variable() {
    let Some(build) = fixture() else { return };
    let d = scratch("caps");
    let trace = d.join("big-trace.log");
    padded(&trace, "", 1);
    let su = d.join("big.su");
    padded(&su, "main.c:1:5:main\t16\tstatic\n", 1);
    let info = d.join("big-build-info.txt");
    padded(&info, "compiler=x\n", 1);
    for (flag, file, var) in [("--trace", &trace, "BUTTERFLY_MAX_TRACE_MB"), ("--stack-usage", &su, "BUTTERFLY_MAX_AUX_MB"), ("--build-info", &info, "BUTTERFLY_MAX_AUX_MB")] {
        let out = d.join("evidence.json");
        let o = ingest(&build, &out, &[flag, file.to_str().unwrap()], &[(var, "1")]);
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success(), "{flag}: a file over the cap must fail the ingest:\n{err}");
        assert!(err.contains(var), "{flag}: names {var}:\n{err}");
        assert!(err.contains(file.file_name().unwrap().to_str().unwrap()), "{flag}: names the file:\n{err}");
        assert!(!out.exists(), "{flag}: and no evidence is written");
    }
    let _ = std::fs::remove_dir_all(d);
}
