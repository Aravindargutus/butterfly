// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly changes` reads the config an evidence file names only from
//! where the operator's own files are, and a config it cannot parse is
//! reported without its contents (#342).
//!
//! Without `--config`, `changes` fell back to `provenance.inputs[role ==
//! "config"].path` -- a path taken from the evidence, which is input like any
//! other. toml prints a parse error with the offending source line, so an
//! evidence file naming any readable file had that file's first line printed
//! to the CI log.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SECRET: &str = "SECRET-342-first-line-of-a-private-file";

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("chcfg-{tag}")).canonicalize().unwrap()
}

/// The smallest evidence `changes` loads, naming `config` as the config it
/// was built with.
fn evidence_naming(dir: &Path, config: &Path) -> PathBuf {
    let ev = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
    let mut v = serde_json::to_value(&ev).unwrap();
    v["provenance"]["inputs"] = serde_json::json!([{ "role": "config", "path": config.to_str().unwrap(), "sha256": "0".repeat(64), "bytes": 1 }]);
    let p = dir.join("evidence.json");
    fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();
    p
}

/// A file that is not TOML, whose first line is the thing that must not leak.
fn private_file(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, format!("{SECRET} and no equals sign\nsecond line\n")).unwrap();
    p
}

/// `changes` run from `cwd`, the project it is working in.
fn changes(cwd: &Path, ev: &Path, extra: &[&str]) -> Output {
    let out = ev.with_file_name("out.json");
    let cache = ev.with_file_name("cache.json");
    Command::new(env!("CARGO_BIN_EXE_butterfly"))
        .current_dir(cwd)
        .args(["changes", ev.to_str().unwrap(), "-o", out.to_str().unwrap(), "--cache", cache.to_str().unwrap()])
        .args(extra)
        .output()
        .expect("spawn butterfly")
}

/// The issue's scenario: an evidence naming a file outside both the
/// evidence's directory and the project is refused, the refusal says to pass
/// `--config`, and nothing of the file reaches stderr.
#[test]
fn an_evidence_naming_a_config_outside_is_refused_and_its_contents_never_printed() {
    let (project, home) = (scratch("project"), scratch("home"));
    let secret = private_file(&home, "credentials");
    let ev = evidence_naming(&project, &secret);

    let o = changes(&project, &ev, &[]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "an outside config was accepted: {err}");
    assert!(err.contains("--config"), "the refusal says how to use it on purpose: {err}");
    assert!(err.contains(secret.to_str().unwrap()), "and names the path the evidence gave: {err}");
    assert!(!err.contains("SECRET-342"), "the file's contents never reach stderr: {err}");
}

/// A link inside the evidence's directory leading out is outside too: the
/// place is the kernel's answer for the opened file, not the name.
#[test]
fn a_symlink_inside_leading_outside_is_outside() {
    let (project, home) = (scratch("link-project"), scratch("link-home"));
    let secret = private_file(&home, "credentials");
    let link = project.join("butterfly.toml");
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    let ev = evidence_naming(&project, &link);

    let o = changes(&project, &ev, &[]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "a link out of the evidence's directory was followed: {err}");
    assert!(err.contains("--config"), "{err}");
    assert!(!err.contains("SECRET-342"), "{err}");
}

/// A config inside the evidence's directory is honoured as before -- here
/// shown by its parse error, which names the file and the position but not
/// the line.
#[test]
fn a_config_beside_the_evidence_is_read_and_its_parse_error_carries_no_source() {
    let project = scratch("inside");
    let cfg = private_file(&project, "butterfly.toml");
    let ev = evidence_naming(&project, &cfg);

    let o = changes(&project, &ev, &[]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "{err}");
    assert!(err.contains("parsing") && err.contains(cfg.to_str().unwrap()), "the file was read and the error names it: {err}");
    assert!(err.contains("line 1"), "and where the problem is: {err}");
    assert!(!err.contains("SECRET-342"), "but not what the line says: {err}");
}

/// `--config` is the operator's own choice and is read wherever it is; its
/// parse error is held to the same rule.
#[test]
fn an_explicit_config_is_read_wherever_it_is_without_echoing_it() {
    let (project, home) = (scratch("explicit-project"), scratch("explicit-home"));
    let cfg = private_file(&home, "butterfly.toml");
    let ev = evidence_naming(&project, &project.join("absent.toml"));

    let o = changes(&project, &ev, &["--config", cfg.to_str().unwrap()]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "{err}");
    assert!(err.contains("parsing") && err.contains(cfg.to_str().unwrap()), "read from where the operator said: {err}");
    assert!(!err.contains("SECRET-342"), "{err}");
}

/// The smallest evidence `changes` loads, naming no config at all.
fn evidence_without_config(dir: &Path) -> PathBuf {
    let ev = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
    let p = dir.join("evidence.json");
    fs::write(&p, serde_json::to_string(&ev).unwrap()).unwrap();
    p
}

/// `changes` refused a `butterfly.toml` beside the evidence that links out,
/// without having parsed it.
fn refused_before_parsing(o: &Output) {
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "a butterfly.toml linking out was followed: {err}");
    assert!(err.contains("--config"), "the refusal says how to use it on purpose: {err}");
    assert!(!err.contains("parsing"), "refused before the file was parsed: {err}");
    assert!(!err.contains("SECRET-342"), "{err}");
}

/// With no config named, `changes` falls back to `butterfly.toml` beside the
/// evidence. That fallback is placed like a named config, so a link there
/// leading out is refused instead of read (#346 review).
#[test]
fn a_discovered_butterfly_toml_linking_out_is_refused_when_no_config_is_named() {
    let (project, home) = (scratch("disc-none-project"), scratch("disc-none-home"));
    let secret = private_file(&home, "credentials");
    std::os::unix::fs::symlink(&secret, project.join("butterfly.toml")).unwrap();
    let ev = evidence_without_config(&project);
    refused_before_parsing(&changes(&project, &ev, &[]));
}

/// The same when the evidence names a config inside that is not there: the
/// fallback it then takes is placed too.
#[test]
fn a_discovered_butterfly_toml_linking_out_is_refused_when_the_named_config_is_missing() {
    let (project, home) = (scratch("disc-missing-project"), scratch("disc-missing-home"));
    let secret = private_file(&home, "credentials");
    std::os::unix::fs::symlink(&secret, project.join("butterfly.toml")).unwrap();
    let ev = evidence_naming(&project, &project.join("absent.toml"));
    refused_before_parsing(&changes(&project, &ev, &[]));
}

/// serde's error for a mistyped value quotes it; an evidence-named config is
/// reported by position alone, so the value never reaches stderr.
#[test]
fn a_type_error_in_an_evidence_named_config_does_not_echo_its_value() {
    let project = scratch("typed");
    let cfg = project.join("butterfly.toml");
    fs::write(&cfg, "requirements = \"DUMMY_PRIVATE_VALUE\"\n").unwrap();
    let ev = evidence_naming(&project, &cfg);

    let o = changes(&project, &ev, &[]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "{err}");
    assert!(err.contains("parsing") && err.contains("line 1"), "the file was read and the error placed: {err}");
    assert!(!err.contains("DUMMY_PRIVATE_VALUE"), "the mistyped value is not echoed: {err}");
}

/// A hardlink inside a non-canonical root (a symlinked project directory, or
/// macOS `/tmp` -> `/private/tmp`) must still be refused: `open_placed`
/// against a non-canonical root returned `Standing::Outside` without checking
/// `nlink()`, which let a subsequent multi-root check accept the hardlink.
#[cfg(unix)]
#[test]
fn a_hardlinked_config_inside_a_non_canonical_root_is_refused() {
    let base = scratch("hl-noncanon");
    let real_proj = base.join("real_proj");
    let home = base.join("home");
    fs::create_dir_all(&real_proj).unwrap();
    fs::create_dir_all(&home).unwrap();
    let alias_proj = base.join("alias_proj");
    std::os::unix::fs::symlink(&real_proj, &alias_proj).unwrap();

    let secret = private_file(&home, "credentials");
    let hl = real_proj.join("butterfly.toml");
    fs::hard_link(&secret, &hl).unwrap();
    // Evidence names the config through the non-canonical alias path
    let ev = evidence_naming(&real_proj, &alias_proj.join("butterfly.toml"));

    let o = changes(&alias_proj, &ev, &[]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "a hardlink under a non-canonical root was accepted: {err}");
    assert!(err.contains("--config"), "{err}");
    assert!(!err.contains("SECRET-342"), "{err}");
}

/// A symlink in the evidence's directory pointing to a hardlink in `cwd` must
/// also be refused, even when all roots are canonical.
#[cfg(unix)]
#[test]
fn a_cross_root_symlink_to_a_hardlink_is_refused() {
    let (ev_dir, cwd_dir, home) = (scratch("xroot-ev"), scratch("xroot-cwd"), scratch("xroot-home"));
    let secret = private_file(&home, "credentials");
    let hl = cwd_dir.join("shared.toml");
    fs::hard_link(&secret, &hl).unwrap();
    let link = ev_dir.join("butterfly.toml");
    std::os::unix::fs::symlink(&hl, &link).unwrap();
    let ev = evidence_naming(&ev_dir, &link);

    let o = changes(&cwd_dir, &ev, &[]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "a cross-root symlink to a hardlink was accepted: {err}");
    assert!(err.contains("--config"), "{err}");
    assert!(!err.contains("SECRET-342"), "{err}");
}

/// The smallest evidence `changes` loads, naming `baseline` in `ev.baseline.path`.
fn evidence_naming_baseline(dir: &Path, baseline: &Path) -> PathBuf {
    let ev = butterfly_model::Evidence {
        schema: butterfly_model::SCHEMA.into(),
        baseline: Some(butterfly_model::BaselineCheck { path: baseline.to_str().unwrap().into(), status: "matches".into(), ..Default::default() }),
        ..Default::default()
    };
    let p = dir.join("evidence.json");
    fs::write(&p, serde_json::to_string(&ev).unwrap()).unwrap();
    p
}

/// An evidence naming a baseline outside the evidence's directory and project
/// must be refused before opening or echoing its contents.
#[test]
fn an_evidence_naming_a_baseline_outside_is_refused_and_its_contents_never_printed() {
    let (project, home) = (scratch("base-project"), scratch("base-home"));
    let secret = home.join("secret.json");
    fs::write(&secret, format!(r#"{{"schema":"{SECRET}"}}"#)).unwrap();
    let ev = evidence_naming_baseline(&project, &secret);

    let o = changes(&project, &ev, &[]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "an outside baseline was accepted: {err}");
    assert!(err.contains("--baseline"), "the refusal names --baseline: {err}");
    assert!(!err.contains("SECRET-342"), "the file's contents never reach stderr: {err}");
}

/// A discovered `butterfly-baseline.json` beside the evidence that links out
/// or is a dangling symlink must fail closed rather than follow or ignore it.
#[cfg(unix)]
#[test]
fn a_discovered_baseline_symlink_escaping_or_dangling_is_refused() {
    let (project, home) = (scratch("disc-base-project"), scratch("disc-base-home"));
    let secret = home.join("secret.json");
    fs::write(&secret, format!(r#"{{"schema":"{SECRET}"}}"#)).unwrap();
    std::os::unix::fs::symlink(&secret, project.join("butterfly-baseline.json")).unwrap();
    let ev = evidence_without_config(&project);

    let o = changes(&project, &ev, &[]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "an escaping baseline symlink was followed: {err}");
    assert!(err.contains("--baseline"), "{err}");
    assert!(!err.contains("SECRET-342"), "{err}");

    // Dangling symlink must fail closed too (not silently fall back to no baseline)
    fs::remove_file(project.join("butterfly-baseline.json")).unwrap();
    std::os::unix::fs::symlink(home.join("does-not-exist.json"), project.join("butterfly-baseline.json")).unwrap();
    let o = changes(&project, &ev, &[]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "a dangling baseline symlink was ignored: {err}");
    assert!(err.contains("--baseline"), "{err}");
}
