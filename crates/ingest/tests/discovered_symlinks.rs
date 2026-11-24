// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! A file discovery picks by its name is read only as the file at that name,
//! never through a symlink (#341).
//!
//! `build-info.txt` is copied line by line into `provenance.build_info`, which
//! lands in `evidence.json` and `report.html` -- often published as a CI
//! artifact. Anyone who could write to the build directory could plant
//! `build-info.txt -> ~/.aws/credentials` and have the operator's own secrets
//! published with the evidence. A path the operator names on the command line
//! is theirs, and keeps following links.

#![cfg(unix)]

use butterfly_ingest::IngestOptions;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

const SECRET: &str = "aws_secret_access_key=SECRET-341-DO-NOT-PUBLISH";

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("disc-link-{tag}")).canonicalize().unwrap()
}

/// A minimal llvm-cov export naming one source.
fn export(src: &Path) -> String {
    format!(
        r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[[1,1,1,true,true,false]],"branches":[],"summary":{{}}}}],
           "functions":[{{"name":"f","count":1,"regions":[[1,1,3,2,1,0,0,0]],"branches":[],"filenames":[{p:?}]}}]}}]}}"#,
        p = src.to_str().unwrap()
    )
}

/// A build directory ingest accepts, and beside it -- outside it -- a file of
/// the operator's that must never reach the evidence.
fn build_dir(tag: &str) -> (PathBuf, PathBuf) {
    let d = scratch(tag);
    let src = d.join("a.c");
    fs::write(&src, "int f(void) {\n  return 1;\n}\n").unwrap();
    fs::write(d.join("coverage.json"), export(&src)).unwrap();
    let outside = scratch(&format!("{tag}-home"));
    let secret = outside.join("credentials");
    fs::write(&secret, format!("{SECRET}\n")).unwrap();
    (d, secret)
}

fn ingest(o: &mut IngestOptions, root: &Path) -> anyhow::Result<butterfly_model::Evidence> {
    o.source_root = Some(root.to_path_buf());
    butterfly_ingest::ingest(o)
}

/// The issue's scenario, as written: a `build-info.txt` symlink to a secret
/// is refused, by name, and the secret is in neither the error nor any
/// evidence.
#[test]
fn a_build_info_symlink_to_a_secret_is_refused_and_the_secret_never_appears() {
    let (d, secret) = build_dir("bi");
    symlink(&secret, d.join("build-info.txt")).unwrap();

    let err = match butterfly_ingest::discover(&d) {
        Ok(mut o) => match ingest(&mut o, &d) {
            Ok(ev) => {
                let json = serde_json::to_string(&ev).unwrap();
                assert!(!json.contains("SECRET-341"), "the secret was copied into the evidence: {:?}", ev.provenance.build_info);
                panic!("a symlinked build-info.txt was accepted: {:?}", ev.provenance.build_info);
            }
            Err(e) => format!("{e:#}"),
        },
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("build-info.txt"), "the error names the file: {err}");
    assert!(err.contains("symlink"), "and says why: {err}");
    assert!(err.contains("--build-info"), "and how to follow it on purpose: {err}");
    assert!(!err.contains("SECRET-341"), "the secret is not in the error either: {err}");
}

/// Every file discovery takes by its name is held to the same rule, not just
/// the one the issue happened to name.
#[test]
fn every_input_discovery_picks_by_name_refuses_a_symlink() {
    for name in ["demo.su", "butterfly.toml", "butterfly-baseline.json", "butterfly-changes.json", "trace.log", "run.trace"] {
        let (d, secret) = build_dir(&format!("each-{}", name.replace('.', "-")));
        symlink(&secret, d.join(name)).unwrap();
        let err = butterfly_ingest::discover_artifacts(&d).map(|_| ()).expect_err(&format!("a symlinked {name} was discovered"));
        let err = format!("{err:#}");
        assert!(err.contains(name) && err.contains("symlink"), "{name}: {err}");
        assert!(!err.contains("SECRET-341"), "{name}: {err}");
    }
}

/// A symlink discovery would have taken by its contents -- a coverage export
/// or an object -- is refused the same way.
#[test]
fn a_symlinked_coverage_export_or_object_is_refused_too() {
    let (d, _) = build_dir("bycontent");
    let elsewhere = scratch("bycontent-elsewhere");
    fs::rename(d.join("coverage.json"), elsewhere.join("real.json")).unwrap();
    symlink(elsewhere.join("real.json"), d.join("coverage.json")).unwrap();
    let err = format!("{:#}", butterfly_ingest::discover_artifacts(&d).map(|_| ()).unwrap_err());
    assert!(err.contains("coverage.json") && err.contains("symlink") && err.contains("--coverage"), "{err}");

    let (d, _) = build_dir("object");
    let obj = elsewhere.join("image");
    fs::write(&obj, b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0").unwrap();
    symlink(&obj, d.join("image")).unwrap();
    let err = format!("{:#}", butterfly_ingest::discover_artifacts(&d).map(|_| ()).unwrap_err());
    assert!(err.contains("image") && err.contains("symlink") && err.contains("--object"), "{err}");
}

/// What discovery never reads is not refused: a link to a directory, or one
/// with a name discovery ignores, is passed over as before.
#[test]
fn a_symlink_discovery_ignores_is_still_ignored() {
    let (d, secret) = build_dir("ignored");
    symlink(secret.parent().unwrap(), d.join("home")).unwrap();
    symlink(&secret, d.join("notes.log")).unwrap();
    symlink(&secret, d.join("default.profraw")).unwrap();
    let (o, cov) = butterfly_ingest::discover_artifacts(&d).expect("nothing here is an input");
    assert!(cov.is_some());
    assert!(o.objects.is_empty() && o.build_info.is_none() && o.trace.is_none(), "{o:?}");
}

/// The check is not only at discovery. A regular `build-info.txt` swapped for
/// a symlink after discovery and before the read is still refused: the read
/// itself does not follow the link (`O_NOFOLLOW`).
#[test]
fn a_build_info_swapped_for_a_symlink_after_discovery_is_refused_at_the_read() {
    for name in ["build-info.txt", "demo.su"] {
        let (d, secret) = build_dir(&format!("swap-{}", name.replace('.', "-")));
        let body = if name.ends_with(".su") { "a.c:1:5:f\t16\tstatic\n" } else { "compiler=clang\n" };
        fs::write(d.join(name), body).unwrap();
        let mut o = butterfly_ingest::discover(&d).expect("a regular file is discovered");
        fs::remove_file(d.join(name)).unwrap();
        symlink(&secret, d.join(name)).unwrap();

        let err = format!("{:#}", ingest(&mut o, &d).map(|_| ()).expect_err(&format!("{name}: read through the swapped-in symlink")));
        assert!(err.contains(name) && err.contains("symlink"), "{name}: {err}");
        assert!(!err.contains("SECRET-341"), "{name}: {err}");
    }
}

/// A path the operator names is theirs: `--build-info` through a symlink is
/// followed, as it always was.
#[test]
fn a_build_info_the_operator_names_may_be_a_symlink() {
    let (d, secret) = build_dir("named");
    let link = d.join("my-build-info");
    symlink(&secret, &link).unwrap();
    let mut o = IngestOptions { coverage_json: d.join("coverage.json"), build_info: Some(link), ..Default::default() };
    let ev = ingest(&mut o, &d).expect("an operator-named build-info is read through its link");
    assert_eq!(ev.provenance.build_info.get("aws_secret_access_key").map(String::as_str), Some("SECRET-341-DO-NOT-PUBLISH"));
}
