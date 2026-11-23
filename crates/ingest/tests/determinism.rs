// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Evidence must be a function of its inputs.
//!
//! butterfly's output is meant to be read as review evidence and compared
//! against itself -- `butterfly changes`, `--no-regression`, a reviewer
//! diffing last week's file against this week's. All of that assumes that
//! ingesting one unchanged build twice gives one answer. It did not: the
//! order of an indirect call's candidates came from `HashMap` iteration
//! order, which Rust randomises per process, so two runs over an untouched
//! `examples/c/build` produced different files (#215).
//!
//! This is the broad net for that class of bug, not the fix's own test.
//! Note what it can and cannot do. Each `ingest` builds fresh hash maps
//! with fresh `RandomState` seeds, so repeats within one process really do
//! resample the iteration order -- but a randomised order can still come
//! out the same way twice, so this catches a given instance with some
//! probability per repeat rather than with certainty. Twelve repeats put
//! the chance of missing a two-way flip like #215 under 0.05%; a bug that
//! only shows on one seed in a hundred would still slip through. The
//! deterministic guard on #215 itself is
//! `candidate_order_does_not_depend_on_map_iteration_order` in
//! `crates/ingest/src/lib.rs`, which forces both orders instead of
//! sampling them. Treat a pass here as evidence, not as proof -- and note
//! that the failure direction is safe: this can only fail when the output
//! really did differ.
use std::path::{Path, PathBuf};

const REPEATS: usize = 12;

fn fixture(name: &str) -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name);
    if root.join("build/coverage.json").exists() {
        Some(root)
    } else {
        butterfly_testsupport::fixture_missing(&format!("examples/{name}/build not present (run examples/{name}/build.sh)"));
        None
    }
}

/// One ingest, serialised, with the wall-clock stamp flattened.
///
/// `provenance.createdUnix` is the time of the run and is *supposed* to
/// differ between runs; it is the one field that legitimately does. Zeroing
/// it is what makes a byte comparison of the rest meaningful -- leaving it
/// in would make every pair differ for a reason that is not a defect, and
/// the check would be asserting nothing.
fn ingest_once(root: &Path) -> String {
    let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts.source_root = Some(root.to_path_buf());
    let mut ev = butterfly_ingest::ingest(&opts).expect("ingest");
    ev.provenance.created_unix = 0;
    serde_json::to_string(&ev).expect("serialise")
}

/// Where the two strings first differ, with a little context, so a failure
/// says which field moved rather than just "they differ".
fn first_difference(a: &str, b: &str) -> String {
    let at = a.bytes().zip(b.bytes()).position(|(x, y)| x != y).unwrap_or_else(|| a.len().min(b.len()));
    let from = at.saturating_sub(120);
    let end = |s: &str| (at + 120).min(s.len());
    format!("first difference at byte {at}\n  run 1: ...{}...\n  run n: ...{}...", &a[from..end(a)], &b[from..end(b)])
}

fn repeated_ingest_is_identical(name: &str) {
    let Some(root) = fixture(name) else { return };
    let first = ingest_once(&root);
    for i in 2..=REPEATS {
        let again = ingest_once(&root);
        assert!(again == first, "ingest {i} of {REPEATS} over an unchanged examples/{name}/build differs from ingest 1\n{}", first_difference(&first, &again));
    }
}

/// `examples/c` is the fixture #215 was reported against: `OPS[]` is a
/// table of function pointers and `apply_op` calls through it, which is the
/// exact shape that made the candidate order a function of the hash seed.
#[test]
fn ingesting_the_c_fixture_twelve_times_gives_one_answer() {
    repeated_ingest_is_identical("c");
}

/// `examples/c-callgraph` reaches the same code by another route --
/// `dispatch`'s indirect call, drawn with its candidates in the showcase's
/// call graph -- and carries recursion and mutual recursion besides, so it
/// exercises more of the graph building than the flat fixtures do.
#[test]
fn ingesting_the_callgraph_fixture_twelve_times_gives_one_answer() {
    repeated_ingest_is_identical("c-callgraph");
}
