// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Artifact discovery must not be all-or-nothing.
//!
//! `butterfly build` puts each variant's executable, build-info and `.su`
//! stack-usage file in the variant directory, and the coverage export one
//! level up. A discovery routine that refuses to return anything without
//! coverage forces its caller to throw away the rest -- which is how
//! compiler-supplied stack data went missing from every exploratory build
//! (#7).

use std::fs;
use std::path::PathBuf;

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("disc-{tag}"))
}

#[test]
fn artifacts_are_discovered_without_a_coverage_export() {
    let d = scratch("nocov");
    fs::write(d.join("demo.su"), "demo.c:1:5:main\t48\tstatic\n").unwrap();
    fs::write(d.join("other.su"), "demo.c:9:5:f\t16\tstatic\n").unwrap();
    fs::write(d.join("build-info.txt"), "compiler=clang\n").unwrap();
    fs::write(d.join("butterfly.toml"), "[requirements]\npatterns = ['@satisfies (\\S+)']\n").unwrap();

    let (opts, coverage) = butterfly_ingest::discover_artifacts(&d).expect("discovery must succeed without coverage");
    assert!(coverage.is_none(), "there is no coverage export here");
    let mut su: Vec<String> = opts.stack_usage.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    su.sort();
    assert_eq!(su, vec!["demo.su", "other.su"], "every .su file is returned");
    assert!(opts.build_info.is_some(), "build-info.txt is returned");
    assert!(opts.config.is_some(), "butterfly.toml is returned");
}

#[test]
fn discover_still_requires_coverage_and_names_the_directory() {
    let d = scratch("requires");
    fs::write(d.join("demo.su"), "demo.c:1:5:main\t48\tstatic\n").unwrap();
    let err = butterfly_ingest::discover(&d).unwrap_err().to_string();
    assert!(err.contains("llvm-cov") || err.contains("coverage"), "{err}");
    assert!(err.contains(&d.display().to_string()), "the error names the directory: {err}");
    let _ = fs::remove_dir_all(d);
}

/// The two sidecars butterfly writes into a build directory must be found by
/// name, with no flag, and must behave alike.
///
/// `butterfly-baseline.json` was not: its branch sat below a generic `.json`
/// branch whose `continue` consumed every JSON file in the directory, so the
/// documented convention -- drop the baseline beside the build and re-ingest
/// -- silently produced `baseline: null` and no stale-review invalidation
/// (#238). `butterfly-changes.json` sat above that branch and worked, and the
/// asymmetry was the whole defect; both are asserted here so a future edit
/// cannot reintroduce it on either side.
#[test]
fn the_sidecars_in_a_build_directory_are_discovered_by_name() {
    let d = scratch("sidecars");
    // A real coverage export, so the generic .json branch has something to
    // recognise and the two sidecars are not the only JSON in the directory.
    fs::write(d.join("coverage.json"), r#"{"data":[],"type":"llvm.coverage.json.export","version":"2.0.1"}"#).unwrap();
    fs::write(d.join("butterfly-baseline.json"), r#"{"schema":"butterfly-baseline/2","functions":[]}"#).unwrap();
    fs::write(d.join("butterfly-changes.json"), r#"{"schema":"butterfly-changes/1","functions":[]}"#).unwrap();

    let (opts, coverage) = butterfly_ingest::discover_artifacts(&d).expect("discovery must succeed");
    assert_eq!(
        coverage.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).as_deref(),
        Some("coverage.json"),
        "the coverage export is still recognised"
    );
    assert_eq!(
        opts.baseline.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).as_deref(),
        Some("butterfly-baseline.json"),
        "a baseline placed in the build directory is discovered with no --baseline flag (#238)"
    );
    assert_eq!(
        opts.changes.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).as_deref(),
        Some("butterfly-changes.json"),
        "the change-history cache is still discovered, as it always was"
    );
    let _ = fs::remove_dir_all(d);
}

#[test]
fn a_directory_that_cannot_be_read_is_an_error_not_an_empty_result() {
    let missing = scratch("missing").join("does-not-exist");
    assert!(butterfly_ingest::discover_artifacts(&missing).is_err(), "a missing directory must be reported, never silently defaulted");
}
