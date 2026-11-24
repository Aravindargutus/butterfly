// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! C++ exceptions and RTTI, on one program built both ways (#165).
//!
//! `examples/cpp-exceptions` builds `flight.cpp` with exceptions and RTTI
//! (build/) and with `-fno-exceptions -fno-rtti` (build-noexc/), the way
//! every other C++ example is built. The first throws once, catches it, and
//! uses `dynamic_cast`.
//!
//! Before the image's exception tables were read, every landing pad had no
//! way in, so the catch handler that had just run was reported unreachable,
//! and the C++ runtime calls (`__cxa_throw`, `_Unwind_Resume`,
//! `__dynamic_cast`) were unclassified. What is asserted is what holds on any
//! architecture; the counts are this toolchain's and the README says so.

use butterfly_model::{EdgeKind, Evidence, Status};
use std::path::PathBuf;

fn ingest(dir: &str) -> Option<Evidence> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cpp-exceptions").canonicalize().ok()?;
    let build = root.join(dir);
    if !(build.join("coverage.json").exists() && build.join("flight").exists()) {
        butterfly_testsupport::fixture_missing("run examples/cpp-exceptions/build.sh first");
        return None;
    }
    let mut opts = butterfly_ingest::discover(&build).expect("discover");
    opts.source_root = Some(root);
    Some(butterfly_ingest::ingest(&opts).expect("ingest"))
}

fn helper_classes(ev: &Evidence) -> Vec<(String, String)> {
    ev.functions
        .iter()
        .filter_map(|f| f.object.as_ref())
        .flat_map(|o| &o.gaps)
        .filter(|g| g.kind == butterfly_model::GapKind::HelperCall)
        .map(|g| (g.description.split('`').nth(1).unwrap_or("").to_string(), g.class.clone().unwrap_or_default()))
        .collect()
}

#[test]
fn landing_pads_are_reached_through_the_exception_table() {
    let Some(ev) = ingest("build") else { return };
    let mut pads = 0;
    for f in &ev.functions {
        let Some(o) = &f.object else { continue };
        assert!(o.unreachable_blocks.is_empty(), "{}: {:?} reported unreachable", f.name, o.unreachable_blocks);
        for b in &o.blocks {
            for e in b.succs.iter().filter(|e| e.kind == EdgeKind::Exception) {
                pads += 1;
                // a pad's lines also run on the normal path, so line coverage
                // cannot say an exception took this edge
                assert_ne!(e.projected, Status::Covered, "{}: an exception edge B{}->B{} is projected covered", f.name, b.id, e.to);
            }
        }
    }
    assert!(pads > 0, "no exception edge was drawn");
    // the catch in safe_altitude is one of them
    let safe = ev.functions.iter().find(|f| f.name.starts_with("safe_altitude")).and_then(|f| f.object.as_ref()).expect("safe_altitude");
    assert!(safe.blocks.iter().any(|b| b.succs.iter().any(|e| e.kind == EdgeKind::Exception)), "the try block has no edge to its catch");
}

#[test]
fn the_exception_and_rtti_runtime_is_classified() {
    let Some(ev) = ingest("build") else { return };
    let classes = helper_classes(&ev);
    for (callee, want) in [("__cxa_throw", "core-lib"), ("__cxa_begin_catch", "core-lib"), ("__dynamic_cast", "core-lib")] {
        let got: Vec<&str> = classes.iter().filter(|(c, _)| c.trim_start_matches('_') == callee.trim_start_matches('_')).map(|(_, k)| k.as_str()).collect();
        assert!(!got.is_empty() && got.iter().all(|k| *k == want), "{callee}: {got:?}");
    }
    let unclassified: Vec<&String> = classes.iter().filter(|(_, k)| k == "unknown").map(|(c, _)| c).collect();
    assert!(unclassified.is_empty(), "runtime calls left unclassified: {unclassified:?}");
}

/// The same program without exceptions and RTTI: none of that machinery,
/// and the virtual calls it makes instead are indirect branches.
#[test]
fn without_exceptions_there_is_no_landing_pad_and_no_exception_runtime() {
    let (Some(with), Some(without)) = (ingest("build"), ingest("build-noexc")) else { return };
    let exception_edges =
        |ev: &Evidence| ev.functions.iter().filter_map(|f| f.object.as_ref()).flat_map(|o| &o.blocks).flat_map(|b| &b.succs).filter(|e| e.kind == EdgeKind::Exception).count();
    assert_eq!(exception_edges(&without), 0);
    assert!(!helper_classes(&without).iter().any(|(c, _)| c.contains("cxa_") || c.contains("Unwind_") || c.contains("dynamic_cast")));
    assert!(without.summary.instructions_total < with.summary.instructions_total, "exceptions and RTTI add object code");
    assert!(without.summary.gaps_by_kind.get("indirect-branch").copied().unwrap_or(0) > 0, "the virtual calls that replace dynamic_cast are indirect");
}
