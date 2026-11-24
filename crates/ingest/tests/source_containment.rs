// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Source filenames in a coverage export are untrusted input.
//!
//! butterfly reads each one from the local filesystem and embeds its path,
//! hash and complete contents in the evidence -- which is then served,
//! archived or mailed. A crafted export naming `/etc/hosts`, or escaping the
//! project with `../`, or pointing a symlink out of it, must not turn into
//! file disclosure (#1).

use butterfly_ingest::IngestOptions;
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let d = butterfly_testsupport::scratch_dir(&format!("sc-{tag}"));
    d.canonicalize().unwrap()
}

/// A minimal llvm-cov export naming the given source files.
fn export(files: &[&str]) -> String {
    let f: Vec<String> = files.iter().map(|p| format!(r#"{{"filename":{p:?},"segments":[[1,1,1,true,true,false]],"branches":[],"summary":{{}}}}"#)).collect();
    let fnames: Vec<String> = files.iter().map(|p| format!("{p:?}")).collect();
    format!(
        r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{}],
           "functions":[{{"name":"f","count":1,"regions":[[1,1,3,2,1,0,0,0]],"branches":[],"filenames":[{}]}}]}}]}}"#,
        f.join(","),
        fnames.join(",")
    )
}

fn opts(dir: &Path, root: Option<&Path>, allow_external: bool) -> IngestOptions {
    IngestOptions { coverage_json: dir.join("coverage.json"), source_root: root.map(Path::to_path_buf), allow_external_source: allow_external, ..Default::default() }
}

fn write_export(dir: &Path, files: &[&str]) {
    fs::write(dir.join("coverage.json"), export(files)).unwrap();
}

#[test]
fn an_absolute_path_outside_the_source_root_is_refused() {
    let d = scratch("abs");
    let proj = d.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("ok.c"), "int f(void) {\n  return 1;\n}\n").unwrap();
    let secret = d.join("secret.txt");
    fs::write(&secret, "SENSITIVE\n").unwrap();

    write_export(&d, &[proj.join("ok.c").to_str().unwrap(), secret.to_str().unwrap()]);
    let err = butterfly_ingest::ingest(&opts(&d, Some(&proj), false)).unwrap_err().to_string();
    assert!(err.contains("secret.txt"), "the error names the rejected path: {err}");
    assert!(!err.contains("SENSITIVE"), "and never quotes its contents: {err}");
    assert!(err.contains("--allow-external-source") || err.contains("--source-root"), "and says how to proceed: {err}");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn a_traversal_out_of_the_root_is_refused() {
    let d = scratch("traverse");
    let proj = d.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(d.join("outside.c"), "int g(void) {\n  return 2;\n}\n").unwrap();
    let escaping = format!("{}/../outside.c", proj.display());
    write_export(&d, &[&escaping]);
    let err = butterfly_ingest::ingest(&opts(&d, Some(&proj), false)).unwrap_err().to_string();
    assert!(err.contains("outside.c"), "{err}");
    let _ = fs::remove_dir_all(d);
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_the_root_pointing_out_of_it_is_refused() {
    let d = scratch("symlink");
    let proj = d.join("proj");
    fs::create_dir_all(&proj).unwrap();
    let secret = d.join("secret.txt");
    fs::write(&secret, "SENSITIVE\n").unwrap();
    let link = proj.join("looks_local.c");
    std::os::unix::fs::symlink(&secret, &link).unwrap();

    write_export(&d, &[link.to_str().unwrap()]);
    let err = butterfly_ingest::ingest(&opts(&d, Some(&proj), false)).unwrap_err().to_string();
    assert!(err.contains("looks_local.c") || err.contains("secret.txt"), "a symlink escape is caught by canonicalising: {err}");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn files_inside_the_root_are_read_normally() {
    let d = scratch("inside");
    let proj = d.join("proj");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::write(proj.join("src/a.c"), "int f(void) {\n  return 1;\n}\n").unwrap();
    // a path that needs normalising but stays inside
    let noisy = format!("{}/src/./a.c", proj.display());
    write_export(&d, &[&noisy]);
    let ev = butterfly_ingest::ingest(&opts(&d, Some(&proj), false)).expect("a contained source must be accepted");
    assert_eq!(ev.files.len(), 1);
    assert!(!ev.files[0].text.is_empty(), "its text was read");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn the_override_permits_external_sources_and_says_so() {
    let d = scratch("override");
    let proj = d.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("ok.c"), "int f(void) {\n  return 1;\n}\n").unwrap();
    let ext = d.join("vendored.c");
    fs::write(&ext, "int g(void) {\n  return 2;\n}\n").unwrap();
    write_export(&d, &[proj.join("ok.c").to_str().unwrap(), ext.to_str().unwrap()]);

    let ev = butterfly_ingest::ingest(&opts(&d, Some(&proj), true)).expect("the override must allow it");
    assert_eq!(ev.files.len(), 2);
    assert!(
        ev.provenance.caveats.iter().any(|c| c.contains("outside") && c.contains("vendored.c")),
        "the evidence records which files came from outside the project: {:?}",
        ev.provenance.caveats
    );
    let _ = fs::remove_dir_all(d);
}

#[test]
fn every_rejected_path_is_listed_not_just_the_first() {
    let d = scratch("many");
    let proj = d.join("proj");
    fs::create_dir_all(&proj).unwrap();
    for n in ["one.txt", "two.txt", "three.txt"] {
        fs::write(d.join(n), "x\n").unwrap();
    }
    let paths: Vec<String> = ["one.txt", "two.txt", "three.txt"].iter().map(|n| d.join(n).display().to_string()).collect();
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    write_export(&d, &refs);
    let err = butterfly_ingest::ingest(&opts(&d, Some(&proj), false)).unwrap_err().to_string();
    for n in ["one.txt", "two.txt", "three.txt"] {
        assert!(err.contains(n), "{n} missing from: {err}");
    }
    let _ = fs::remove_dir_all(d);
}

#[test]
fn without_a_source_root_the_working_directory_bounds_the_read() {
    let d = scratch("nortoot");
    fs::write(d.join("secret.txt"), "SENSITIVE\n").unwrap();
    write_export(&d, &[d.join("secret.txt").to_str().unwrap()]);
    // cwd is the repository, so a file in a temp directory is out of bounds
    let err = butterfly_ingest::ingest(&opts(&d, None, false)).unwrap_err().to_string();
    assert!(err.contains("secret.txt"), "with no --source-root the working directory is the bound: {err}");
    let _ = fs::remove_dir_all(d);
}

#[cfg(unix)]
#[test]
fn a_symlink_that_stays_inside_the_root_is_read_through_its_resolved_path() {
    let d = scratch("inner-link");
    let proj = d.join("proj");
    fs::create_dir_all(proj.join("gen")).unwrap();
    fs::write(proj.join("gen/real.c"), "int f(void) {\n  return 1;\n}\n").unwrap();
    let link = proj.join("a.c");
    std::os::unix::fs::symlink(proj.join("gen/real.c"), &link).unwrap();

    write_export(&d, &[link.to_str().unwrap()]);
    let ev = butterfly_ingest::ingest(&opts(&d, Some(&proj), false)).expect("a link that stays inside is fine");
    assert_eq!(ev.files.len(), 1);
    assert!(ev.files[0].path.ends_with("a.c"), "the evidence keeps the export's own name: {}", ev.files[0].path);
    assert_eq!(ev.files[0].text.len(), 3, "and the target's text was read");
    let _ = fs::remove_dir_all(d);
}

/// The check and the read used to be two separate lookups of the same name:
/// `canonicalize` said the file was inside the root, and `fs::read` resolved
/// the name again afterwards. Anyone able to write in the source tree could
/// swap the file -- or a directory above it -- for a link out of the project
/// in between, and the contents of the target were embedded in the evidence
/// even though `--allow-external-source` was never given (review of #21).
///
/// The file is now opened once, the kernel is asked where that descriptor
/// points, and the bytes come off the same descriptor. There is no second
/// lookup to win.
#[cfg(unix)]
#[test]
fn a_source_swapped_after_the_check_still_cannot_leak_its_target() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let d = scratch("race");
    let proj = d.join("proj");
    fs::create_dir_all(&proj).unwrap();
    let secret = d.join("secret.txt");
    fs::write(&secret, "SENSITIVE\n").unwrap();

    const GOOD: &str = "int f(void) {\n  return 1; /* INSIDE */\n}\n";
    let real = proj.join("a.c");
    fs::write(&real, GOOD).unwrap();
    write_export(&d, &[real.to_str().unwrap()]);

    let stop = AtomicBool::new(false);
    let leaked = AtomicBool::new(false);
    std::thread::scope(|s| {
        // the attacker: alternate a.c between the project's own file and a
        // link out of the project, each swap atomic so a reader sees one or
        // the other and never a half-written file
        s.spawn(|| {
            let link = proj.join(".swap-link");
            let copy = proj.join(".swap-copy");
            while !stop.load(Ordering::Relaxed) {
                let _ = fs::remove_file(&link);
                if std::os::unix::fs::symlink(&secret, &link).is_ok() {
                    let _ = fs::rename(&link, &real);
                }
                let _ = fs::remove_file(&copy);
                if fs::write(&copy, GOOD).is_ok() {
                    let _ = fs::rename(&copy, &real);
                }
            }
        });

        for _ in 0..400 {
            // refusing the ingest is a fine outcome; reading the secret is not
            if let Ok(ev) = butterfly_ingest::ingest(&opts(&d, Some(&proj), false)) {
                let text: String = ev.files.iter().flat_map(|f| f.text.iter().cloned()).collect::<Vec<_>>().join("\n");
                if text.contains("SENSITIVE") {
                    leaked.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
        stop.store(true, Ordering::Relaxed);
    });

    assert!(!leaked.load(Ordering::Relaxed), "a file outside the source root was embedded in the evidence");
    let _ = fs::remove_dir_all(d);
}

/// A name that cannot be opened must not get a second chance later. It used
/// to be skipped by the containment check and then read again by its raw
/// name, so a target appearing between the two was read unchecked.
#[cfg(unix)]
#[test]
fn a_dangling_symlink_is_an_error_and_is_never_read_again_by_name() {
    let d = scratch("dangling");
    let proj = d.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("ok.c"), "int f(void) {\n  return 1;\n}\n").unwrap();

    let secret = d.join("appears-later.txt");
    let dangling = proj.join("a.c");
    std::os::unix::fs::symlink(&secret, &dangling).unwrap();
    write_export(&d, &[proj.join("ok.c").to_str().unwrap(), dangling.to_str().unwrap()]);

    let err = butterfly_ingest::ingest(&opts(&d, Some(&proj), false)).unwrap_err().to_string();
    assert!(err.contains("a.c"), "the error names the file it could not open: {err}");

    // and now the target exists: the same run must still refuse it
    fs::write(&secret, "SENSITIVE\n").unwrap();
    match butterfly_ingest::ingest(&opts(&d, Some(&proj), false)) {
        Ok(ev) => {
            let text: String = ev.files.iter().flat_map(|f| f.text.iter().cloned()).collect::<Vec<_>>().join("\n");
            panic!("a link out of the project was followed once its target appeared: {text}");
        }
        Err(e) => assert!(!e.to_string().contains("SENSITIVE"), "{e}"),
    }
    let _ = fs::remove_dir_all(d);
}

/// A hard link has no "real" name to check. The kernel reports one of the
/// file's names -- on Linux the one it was opened by, on macOS whichever
/// `F_GETPATH` settles on -- and both can be inside the root while another
/// name for the same inode is not. So a link planted in the source tree to a
/// file outside it passed the containment check and its contents were
/// embedded (#248). macOS lets anyone hard-link a file they cannot even read.
#[cfg(unix)]
#[test]
fn a_hard_link_inside_the_root_to_a_file_outside_it_is_refused() {
    let d = scratch("hardlink-out");
    let proj = d.join("proj");
    fs::create_dir_all(&proj).unwrap();
    let secret = d.join("secret.txt");
    fs::write(&secret, "SENSITIVE\n").unwrap();
    let alias = proj.join("looks_local.c");
    fs::hard_link(&secret, &alias).unwrap();

    write_export(&d, &[alias.to_str().unwrap()]);
    let err = butterfly_ingest::ingest(&opts(&d, Some(&proj), false)).unwrap_err().to_string();
    assert!(err.contains("looks_local.c"), "the error names the file: {err}");
    assert!(err.contains("hard link"), "and says why an in-root name was not enough: {err}");
    assert!(!err.contains("SENSITIVE"), "and never quotes its contents: {err}");
    let _ = fs::remove_dir_all(d);
}

/// Whether the other name is inside the root cannot be found out from the
/// open file: the kernel keeps no list of an inode's names. Both names being
/// in the tree is therefore indistinguishable from one of them being
/// outside it, and the check fails closed rather than guessing.
#[cfg(unix)]
#[test]
fn a_source_with_a_second_name_is_refused_even_when_both_are_inside() {
    let d = scratch("hardlink-in");
    let proj = d.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("a.c"), "int f(void) {\n  return 1;\n}\n").unwrap();
    fs::hard_link(proj.join("a.c"), proj.join("b.c")).unwrap();

    write_export(&d, &[proj.join("a.c").to_str().unwrap()]);
    let err = butterfly_ingest::ingest(&opts(&d, Some(&proj), false)).unwrap_err().to_string();
    assert!(err.contains("a.c") && err.contains("hard link"), "{err}");
    let _ = fs::remove_dir_all(d);
}

/// The operator's override still works, and the evidence records what it
/// let through, as it does for a file outside the root.
#[cfg(unix)]
#[test]
fn the_override_reads_a_hard_linked_source_and_says_so() {
    let d = scratch("hardlink-override");
    let proj = d.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("a.c"), "int f(void) {\n  return 1;\n}\n").unwrap();
    fs::hard_link(proj.join("a.c"), d.join("elsewhere.c")).unwrap();

    write_export(&d, &[proj.join("a.c").to_str().unwrap()]);
    let ev = butterfly_ingest::ingest(&opts(&d, Some(&proj), true)).expect("the override must allow it");
    assert!(!ev.files[0].text.is_empty(), "its text was read");
    assert!(
        ev.provenance.caveats.iter().any(|c| c.contains("hard link") && c.contains("a.c")),
        "the evidence records the file whose authority could not be established: {:?}",
        ev.provenance.caveats
    );
    let _ = fs::remove_dir_all(d);
}
