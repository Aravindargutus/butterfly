// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for `butterfly requirements` as a command
//! (area A): what it walks, what its output records, and what it refuses.
//! Expected values come from README.md "Requirement text
//! (`requirements.json`)" and "Your own document shape".

use std::path::{Path, PathBuf};
use std::process::Command;

fn run(args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_butterfly")).arg("requirements").args(args).output().expect("run");
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("req-a-cli-{tag}"))
}

fn sidecar(p: &Path) -> butterfly_model::reqtext::Requirements {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).expect("a requirements sidecar")
}

/// SHA-256 of the CSV body below, computed independently with
/// `shasum -a 256`.
const BODY_SHA256: &str = "2cb3e089684a6674a771c7538121d2edfc474390fb690fd088add9ae76ca1dd5";

/// README.md:585-586 "A directory is walked for `llr.yaml`, `llr.yml`,
/// `llr.csv`, `llr.md` and `llr.markdown`" -- the three names the existing
/// walk test does not use, each in a subdirectory; other names are not read.
#[test]
fn the_walk_finds_yml_md_and_markdown_and_nothing_else() {
    let d = scratch("walk");
    for (sub, name, body) in [
        ("a", "llr.yml", "requirements:\n  - id: LLR-W-101\n    text: yml\n"),
        ("b", "llr.md", "## LLR-W-102\n\nmd\n"),
        ("c/deeper", "llr.markdown", "## LLR-W-103\n\nmarkdown\n"),
        ("d", "notes.md", "## LLR-W-199\n\nnot a requirements document by name\n"),
        ("e", "hlr.yaml", "requirements:\n  - id: HLR-W-1\n    text: not by name either\n"),
    ] {
        std::fs::create_dir_all(d.join(sub)).unwrap();
        std::fs::write(d.join(sub).join(name), body).unwrap();
    }
    let out = d.join("out.json");
    let (code, err) = run(&[d.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{err}");
    let r = sidecar(&out);
    let ids: Vec<&str> = r.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["LLR-W-101", "LLR-W-102", "LLR-W-103"], "{err}");
    assert_eq!(r.sources.len(), 3);
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:591-594 "Every source file is recorded with its own SHA-256" --
/// the digest in the sidecar is the SHA-256 of the file's bytes.
#[test]
fn each_source_digest_is_the_sha256_of_the_file() {
    let d = scratch("digest");
    let body = "id,text\nLLR-DG-001,the statement\n";
    let doc = d.join("llr.csv");
    std::fs::write(&doc, body).unwrap();
    let out = d.join("out.json");
    let (code, err) = run(&[doc.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{err}");
    let r = sidecar(&out);
    assert_eq!(r.sources.len(), 1);
    assert_eq!(r.sources[0].sha256, BODY_SHA256);
    assert_eq!(r.sources[0].count, 1);
    r.validate().expect("and the command writes a sidecar its readers accept");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:785-799 "The key mapping is configuration ... say so in
/// `butterfly.toml` and no code changes" -- through `--config`, for CSV too
/// (README.md:645-647 "the names are the **same `[requirements.keys]`**").
#[test]
fn a_projects_own_keys_reach_the_command_through_config() {
    let d = scratch("keys");
    std::fs::write(d.join("butterfly.toml"), "[requirements.keys]\nid = \"ref\"\ntext = \"statement\"\ncode = \"impl\"\ncsv_list_separator = \"|\"\n").unwrap();
    let doc = d.join("llr.csv");
    std::fs::write(&doc, "ref,statement,impl\nLLR-K-001,said,src/a.c::f|src/b.c::g\n").unwrap();
    let out = d.join("out.json");
    let (code, err) = run(&[doc.to_str().unwrap(), "--config", d.join("butterfly.toml").to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{err}");
    let r = sidecar(&out);
    let q = r.get("LLR-K-001").expect("read through the configured keys");
    assert_eq!(q.text, "said");
    assert_eq!(q.claim.code, ["src/a.c::f", "src/b.c::g"]);
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:797-799 "A document that does not match is **refused** ...
/// rather than read as zero requirements" -- the command fails and writes
/// no sidecar.
#[test]
fn a_refused_document_fails_the_command_and_writes_nothing() {
    let d = scratch("refuse");
    let doc = d.join("llr.yaml");
    std::fs::write(&doc, "llrs:\n  - id: LLR-R-001\n    text: under the wrong key\n").unwrap();
    let out = d.join("out.json");
    let (code, err) = run(&[doc.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert_ne!(code, Some(0), "{err}");
    assert!(err.contains("llrs"), "names the keys it does have: {err}");
    assert!(!out.exists(), "and no sidecar of nothing was written");
    let _ = std::fs::remove_dir_all(d);
}
