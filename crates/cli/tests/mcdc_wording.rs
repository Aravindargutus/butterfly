// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The MC/DC panel must not describe condition vectors as test cases.
//!
//! `Decision::vectors` holds LLVM condition vectors. One test can produce
//! several (a decision inside a loop), several tests can collapse into one,
//! records are deduplicated across merged instantiations, and some carry
//! `executed: false`. So the viewer cannot say how many tests ran, and must
//! not imply that it can: in an MC/DC view that is assurance text a reviewer
//! may rely on (review of #32).
//!
//! There is no JavaScript test runner in this workspace, so this guards the
//! wording and the executed-only filter statically. The behaviour itself was
//! checked against evidence carrying unexecuted vectors.

use std::path::PathBuf;

fn app_js() -> String {
    std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/app.js")).expect("ui/app.js")
}

/// The summary counts executed vectors, not every record present.
#[test]
fn the_mcdc_summary_counts_only_executed_vectors() {
    let js = app_js();
    assert!(js.contains("d.vectors.filter(v => v.executed !== false)"), "the MC/DC lede must count executed vectors only; an unexecuted record is not evidence that anything ran");
}

/// No sentence in the MC/DC panel may assert a number of test cases.
#[test]
fn the_mcdc_panel_never_counts_test_cases() {
    let js = app_js();
    // phrases that would state, rather than disclaim, a test count
    for bad in ["tests ran, enough", "Proven by tests ", "pair of tests isolates", "the two tests that prove", "${n} tests for"] {
        assert!(!js.contains(bad), "MC/DC text must not describe condition vectors as test cases: found {bad:?}");
    }
    // and the distinction is stated where a reader will see it
    assert!(js.contains("not test cases"), "the panel must say what a condition vector is");
}
