// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The directory form of `butterfly requirements` walks a tree it was handed,
//! and a walk over someone else's filesystem has to be bounded (review of #56).

use std::path::PathBuf;
use std::process::Command;

fn run(args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_butterfly")).arg("requirements").args(args).output().expect("run");
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("walk-{tag}"))
}

/// Recursion has to stop before the stack does. The comment claimed bounded;
/// now it is.
#[test]
fn a_tree_deeper_than_the_ceiling_is_refused_not_recursed_into() {
    let d = scratch("deep");
    let mut p = d.clone();
    for i in 0..80 {
        p = p.join(format!("d{i}"));
    }
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("llr.yaml"), "requirements: []\n").unwrap();

    let (code, err) = run(&[d.to_str().unwrap()]);
    assert_eq!(code, Some(1), "refused rather than overflowing: {err}");
    assert!(err.contains("deeper than"), "and says why: {err}");
    let _ = std::fs::remove_dir_all(&d);
}

/// The walk is for requirement documents, not for one format of them: a
/// component that keeps its LLRs as CSV is found beside one that keeps them
/// as YAML. By base name rather than by extension, because a requirements
/// tree holds other CSVs -- a sweep of every `*.csv` under it would refuse
/// the whole run over a spreadsheet nobody offered it.
#[test]
fn the_walk_finds_every_format_the_reader_understands() {
    let d = scratch("formats");
    let (a, b) = (d.join("alpha"), d.join("beta"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("llr.yaml"), "requirements:\n  - id: LLR-W-010\n    text: from the yaml\n    type: low-level\n").unwrap();
    std::fs::write(b.join("llr.csv"), "id,text,type\nLLR-W-011,from the csv,low-level\n").unwrap();
    // Not a requirements document, and not named like one. Read as one it
    // would refuse the run for having no `id` column.
    std::fs::write(d.join("effort-estimates.csv"), "task,days\nwrite the reader,3\n").unwrap();

    let out = d.join("out.json");
    let (code, err) = run(&[d.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{err}");
    assert!(err.contains("2 file(s)"), "two documents, not three: {err}");
    let doc = std::fs::read_to_string(&out).unwrap();
    assert!(doc.contains("LLR-W-010") && doc.contains("LLR-W-011"), "both formats were read: {err}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A link planted inside the tree must not redirect the walk. The tree is
/// less trusted than the path the operator typed.
#[cfg(unix)]
#[test]
fn a_symlinked_directory_inside_the_tree_is_not_descended() {
    let d = scratch("link");
    let real = d.join("real");
    let elsewhere = d.join("elsewhere");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(real.join("llr.yaml"), "requirements:\n  - id: LLR-W-001\n    text: found\n    type: low-level\n").unwrap();
    // the one behind the link must not be read
    std::fs::write(elsewhere.join("llr.yaml"), "requirements:\n  - id: LLR-W-002\n    text: hidden\n    type: low-level\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, real.join("link")).unwrap();

    let out = d.join("out.json");
    let (code, err) = run(&[real.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{err}");
    let doc = std::fs::read_to_string(&out).unwrap();
    assert!(doc.contains("LLR-W-001"), "the real document is read");
    assert!(!doc.contains("LLR-W-002"), "the one behind the link is not: {err}");
    let _ = std::fs::remove_dir_all(&d);
}
