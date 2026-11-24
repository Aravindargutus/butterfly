// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! A malformed supplier evidence file must be refused with a sentence, never
//! answered with a panic (review of #44).
//!
//! Runs only when examples/c has been built; skipped, not failed, otherwise.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn covered_beyond_total_is_refused_at_the_door_not_by_an_underflow() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c/build.sh first");
        return;
    }
    let d = butterfly_testsupport::scratch_dir("malformed");
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root);
    let ev = butterfly_ingest::ingest(&o).expect("ingest");
    let mut doc: serde_json::Value = serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();

    // schema-valid, arithmetically impossible: an uncalled function claiming
    // one covered line of zero. This used to pass validate() and then panic
    // summary with `attempt to subtract with overflow`.
    let f = doc["functions"].as_array_mut().unwrap().iter_mut().find(|f| f["count"] == 0).expect("an uncalled function");
    let name = f["name"].as_str().unwrap().to_string();
    f["linesTotal"] = serde_json::json!(0);
    f["linesCovered"] = serde_json::json!(1);
    std::fs::write(d.join("evidence.json"), serde_json::to_string(&doc).unwrap()).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_butterfly")).arg("summary").arg(d.join("evidence.json")).output().expect("run summary");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "refused");
    assert!(!err.contains("panicked"), "with a sentence, not a panic: {err}");
    assert!(err.contains("inconsistent evidence") && err.contains("covered exceeds total"), "{err}");
    assert!(err.contains(&name), "naming the function: {err}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Ingest the C example once, hand the document to `doctor`, write it, and
/// run `summary` over the result. Returns (exit code, stderr).
fn summary_over(tag: &str, doctor: impl FnOnce(&mut serde_json::Value)) -> Option<(Option<i32>, String)> {
    summary_over_timed(tag, doctor).map(|(c, e, _)| (c, e))
}

/// As `summary_over`, and also how long `summary` took: some of these
/// documents fail by being slow rather than by being refused.
fn summary_over_timed(tag: &str, doctor: impl FnOnce(&mut serde_json::Value)) -> Option<(Option<i32>, String, std::time::Duration)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c/build.sh first");
        return None;
    }
    let d = butterfly_testsupport::scratch_dir(&format!("malformed-{tag}"));
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root);
    let ev = butterfly_ingest::ingest(&o).expect("ingest");
    let mut doc: serde_json::Value = serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
    doctor(&mut doc);
    std::fs::write(d.join("evidence.json"), serde_json::to_string(&doc).unwrap()).unwrap();
    let t = std::time::Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_butterfly")).arg("summary").arg(d.join("evidence.json")).output().expect("run summary");
    let took = t.elapsed();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&d);
    Some((out.status.code(), err, took))
}

/// A line number is 1-based everywhere in the evidence, so 0 is malformed --
/// but the worklist reached `lines[ln - 1]` and `text[line - 1]` before
/// anything checked, and an unsigned 0 - 1 is a panic, not a None. Two
/// distinct sites, so two cases (self-review of #44: the first fix for the
/// P1 underflow removed one subtraction and introduced another).
#[test]
fn a_function_starting_at_line_zero_does_not_underflow_the_worklist() {
    let Some((code, err)) = summary_over("line0", |doc| {
        let f = doc["functions"].as_array_mut().unwrap().iter_mut().find(|f| f["count"] == 0).expect("an uncalled function");
        f["lineStart"] = serde_json::json!(0);
    }) else {
        return;
    };
    assert!(!err.contains("attempt to subtract with overflow"), "{err}");
    assert!(!err.contains("panicked"), "a sentence or a shrug, never a panic: {err}");
    let _ = code;
}

#[test]
fn a_condition_on_line_zero_does_not_underflow_its_label() {
    let Some((code, err)) = summary_over("cond0", |doc| {
        for f in doc["functions"].as_array_mut().unwrap() {
            if f["count"].as_u64().unwrap_or(0) == 0 {
                continue;
            }
            for dec in f["decisions"].as_array_mut().unwrap() {
                for c in dec["conditions"].as_array_mut().unwrap() {
                    if c["independenceCovered"] == serde_json::json!(false) {
                        c["line"] = serde_json::json!(0);
                    }
                }
            }
        }
    }) else {
        return;
    };
    assert!(!err.contains("attempt to subtract with overflow"), "{err}");
    assert!(!err.contains("panicked"), "a sentence or a shrug, never a panic: {err}");
    let _ = code;
}

/// An uncalled function whose range runs to the end of the address space.
///
/// `validate()` on this branch checks counter pairs but not source ranges, so
/// such a document is *accepted* -- and the worklist then walked the range it
/// was given. Four billion iterations is not a crash, which is what makes it
/// worse: `summary` simply stops answering. The consumer clamps to the file,
/// so the cost is the file's length whatever the document claims (review of
/// #44).
#[test]
fn an_uncalled_function_ending_at_the_end_of_u32_does_not_hang_the_summary() {
    let Some((code, err, took)) = summary_over_timed("huge", |doc| {
        let f = doc["functions"].as_array_mut().unwrap().iter_mut().find(|f| f["count"] == 0).expect("an uncalled function");
        f["lineStart"] = serde_json::json!(1);
        f["lineEnd"] = serde_json::json!(u32::MAX);
    }) else {
        return;
    };
    assert!(!err.contains("panicked"), "{err}");
    // The point of the test: it answers at all. Four billion iterations does
    // not fit in ten seconds; the file's own length does, easily.
    assert!(took < std::time::Duration::from_secs(10), "summary took {took:?} — the range was walked, not clamped");
    let _ = code;
}
