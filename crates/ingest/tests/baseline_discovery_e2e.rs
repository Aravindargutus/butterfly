// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! End-to-end proof of the #238 fix: a `butterfly-baseline.json` dropped into
//! a build directory is not only *discovered* (asserted fixture-free in
//! `discovery.rs`) but actually *drives* the source-to-object review-staleness
//! check (DO-178C 4.4.2.c) all the way through `discover` + `ingest`.
//!
//! The bug the fix closed was silent: the baseline was read, discarded and
//! reported as `baseline: null`, so a function whose source had changed since
//! the certified freeze produced no comparison, no invalidation and no
//! warning. A unit test on discovery alone cannot catch that regression coming
//! back through a different code path, so this drives the real binary path: the
//! same `discover` an operator gets with no `--baseline` flag, then `ingest`,
//! and asserts the discovered baseline reaches `ev.baseline` and raises the
//! "BASELINE CHANGED" caveat.
//!
//! Runs on the built `examples/c` fixture; skipped, not failed, otherwise, so
//! `cargo test` stays green on a machine without a compiler -- the same
//! convention as the sibling `e2e_demo_c` suite on this fixture.
use std::path::{Path, PathBuf};

fn fixture() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    let build = root.join("build");
    if build.join("coverage.json").exists() && build.join("demo").exists() {
        Some(root)
    } else {
        eprintln!("skipping: examples/c/build not present (run examples/c/build.sh)");
        None
    }
}

/// Copy a build directory recursively into scratch, so a baseline can be
/// dropped beside the artefacts without touching the shared fixture (which
/// other suites ingest in parallel).
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let (from, to) = (e.path(), dst.join(e.file_name()));
        if e.file_type().unwrap().is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

#[test]
fn a_baseline_dropped_in_the_build_dir_is_discovered_and_drives_staleness() {
    let Some(root) = fixture() else { return };

    // Ingest the pristine build once: no baseline is present, so the
    // review-staleness comparison has nothing to run against.
    let mut opts0 = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts0.source_root = Some(root.clone());
    let ev0 = butterfly_ingest::ingest(&opts0).expect("ingest");
    assert!(ev0.baseline.is_none(), "the pristine build directory carries no baseline");
    assert!(!ev0.provenance.caveats.iter().any(|c| c.contains("BASELINE CHANGED")), "no baseline, no staleness caveat: {:?}", ev0.provenance.caveats);

    // Freeze a baseline from that evidence, then make it look like one
    // function's source drifted since the freeze -- exactly the case 4.4.2.c
    // exists to catch. `clamp_altitude` is a real function in this fixture.
    let mut base = butterfly_ingest::baseline::from_evidence(&ev0);
    // The baseline's functions are keyed by a name+file+line fingerprint key
    // (#249), not the bare name, so find the entry by the name it records.
    let fp = base.functions.values_mut().find(|fp| fp.name == "clamp_altitude").expect("clamp_altitude is in the fixture");
    fp.source_sha256 = "0".repeat(64); // a source text this build could not have produced

    // Drop it into a *copy* of the build directory, named exactly as the
    // documentation says and as no --baseline flag is given.
    let scratch = butterfly_testsupport::scratch_dir("baseline-e2e");
    let build = scratch.join("build");
    copy_tree(&root.join("build"), &build);
    std::fs::write(build.join("butterfly-baseline.json"), serde_json::to_string(&base).unwrap()).unwrap();

    // The operator's path: discover with no flag, then ingest.
    let mut opts = butterfly_ingest::discover(&build).expect("discover");
    opts.source_root = Some(root.clone());
    assert_eq!(
        opts.baseline.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).as_deref(),
        Some("butterfly-baseline.json"),
        "the baseline in the build dir is discovered with no --baseline flag (#238)"
    );

    let ev = butterfly_ingest::ingest(&opts).expect("ingest");

    // The discovered baseline reached the comparison: the drifted function is
    // named source-changed, the check is `changed`, and the invalidation
    // caveat that was silently missing before #238 is now raised.
    let check = ev.baseline.as_ref().expect("a discovered baseline populates ev.baseline end-to-end (#238)");
    assert_eq!(check.status, "changed", "a source-changed function makes the baseline check `changed`: {:?}", check.differences);
    assert!(check.functions_source_changed.iter().any(|n| n == "clamp_altitude"), "the drifted function is reported source-changed: {:?}", check.functions_source_changed);
    assert!(
        ev.provenance.caveats.iter().any(|c| c.contains("BASELINE CHANGED") && c.contains("4.4.2.c")),
        "the review-staleness caveat is raised (the safeguard that #238's bug silently skipped): {:?}",
        ev.provenance.caveats
    );

    let _ = std::fs::remove_dir_all(&scratch);
}
