// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! An evidence file is untrusted input on every command that loads one, not
//! only `serve`. `summary`, `report`, `baseline`, `changes` and `impact` read
//! it before validating, so an oversized file could allocate its full size
//! first -- and those are the ordinary entry points (#5).

use std::path::PathBuf;
use std::process::{Command, Output};

fn evidence() -> Option<PathBuf> {
    // `examples/c/build/evidence.json` is produced by CI's documented-workflow
    // step, which runs after the tests, and build.sh deletes it -- so it is
    // not reliably present. Ingest it from the coverage export build.sh does
    // leave, once, rather than skip (which the CI `skipping:` gate would also
    // catch, but a self-sufficient test is better than a skipped one).
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let shared = root.join("examples/c/build/evidence.json");
    if shared.exists() {
        return Some(shared.canonicalize().unwrap());
    }
    if !root.join("examples/c/build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run ./examples/c/build.sh first");
        return None;
    }
    let out = butterfly_testsupport::scratch_dir("evlim-src").join("evidence.json");
    let o = Command::new(env!("CARGO_BIN_EXE_butterfly"))
        .args(["ingest", root.join("examples/c/build").to_str().unwrap(), "--source-root", root.join("examples/c").to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .expect("spawn ingest");
    assert!(o.status.success(), "ingest for the fixture: {}", String::from_utf8_lossy(&o.stderr));
    Some(out)
}

fn run(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().expect("spawn butterfly")
}

/// A valid evidence file padded past a lowered cap. The pad is trailing
/// whitespace inside the top-level object, so the file stays valid JSON and
/// would parse fine -- the point is that it is refused before it is read
/// whole, by size, not by content.
fn padded_evidence(dir: &std::path::Path, mb: usize) -> PathBuf {
    let ev = std::fs::read_to_string(evidence().unwrap()).unwrap();
    // insert whitespace after the opening brace
    let padded = format!("{{{}{}", " ".repeat(mb * 1024 * 1024), &ev[1..]);
    let p = dir.join("big-evidence.json");
    std::fs::write(&p, padded).unwrap();
    p
}

#[test]
fn every_evidence_command_refuses_an_oversized_file() {
    let Some(_) = evidence() else {
        butterfly_testsupport::fixture_missing("build examples/c first");
        return;
    };
    let d = butterfly_testsupport::scratch_dir("evlim");
    let big = padded_evidence(&d, 2); // 2 MiB against a 1 MiB cap
    let bigs = big.to_str().unwrap();
    let out = d.join("out");

    // each command that loads evidence, with the cap lowered to 1 MiB
    let cases: &[(&str, Vec<&str>)] = &[
        ("summary", vec!["summary", bigs]),
        ("report", vec!["report", bigs, "-o", out.to_str().unwrap()]),
        ("baseline", vec!["baseline", bigs, "-o", out.to_str().unwrap()]),
        ("impact", vec!["impact", bigs, "--json"]),
    ];
    for (name, args) in cases {
        let o = run(args, &[("BUTTERFLY_MAX_EVIDENCE_MB", "1")]);
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success(), "{name}: an oversized evidence file must be refused");
        assert!(err.contains("evidence file") && err.contains("BUTTERFLY_MAX_EVIDENCE_MB"), "{name}: the refusal names the kind and the override: {err}");
    }

    // and with the default cap the same file loads (2 MiB is far within 512)
    let o = run(&["summary", bigs], &[]);
    assert!(o.status.success(), "under the default cap the file is fine: {}", String::from_utf8_lossy(&o.stderr));
    let _ = std::fs::remove_dir_all(d);
}
