// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for where butterfly reads and writes (area A).
//!
//! README.md:295-308 (source reads bounded by `--source-root`) and
//! README.md:1572-1577 / docs/butterfly.html:837 ("Outputs are replaced,
//! never written through").
#![cfg(unix)]

use butterfly_ingest::fsauth::{open_placed, write_atomic, Standing, Wrote};
use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let d = butterfly_testsupport::scratch_dir(&format!("req-a-fsauth-{tag}"));
    d.canonicalize().unwrap()
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// docs/butterfly.html:837 "Each write goes to a temporary file beside it,
/// is flushed to disk, then renamed over the top" -- a new file and a
/// replaced file both hold exactly the new bytes, and no temporary is left.
#[test]
fn an_output_is_written_whole_and_leaves_no_temporary() {
    let d = scratch("whole");
    let p = d.join("evidence.json");
    assert_eq!(write_atomic(&p, b"first").unwrap(), Wrote::Normally);
    assert_eq!(write_atomic(&p, b"second, longer").unwrap(), Wrote::Normally);
    assert_eq!(std::fs::read(&p).unwrap(), b"second, longer");
    assert_eq!(names(&d), ["evidence.json"], "nothing else beside it");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1575-1577 "A symlink at the destination is displaced rather than
/// followed, and butterfly says so" -- the link's target is untouched and the
/// caller is told which link it was.
#[test]
fn a_symlink_at_the_destination_is_displaced_and_its_target_untouched() {
    let d = scratch("link");
    let victim = d.join("victim.txt");
    std::fs::write(&victim, "precious").unwrap();
    let dest = d.join("report.html");
    std::os::unix::fs::symlink(&victim, &dest).unwrap();
    let w = write_atomic(&dest, b"<html>").unwrap();
    assert!(matches!(w, Wrote::ReplacedSymlink(_)), "the caller is told: {w:?}");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "precious", "the target is untouched");
    assert!(!std::fs::symlink_metadata(&dest).unwrap().file_type().is_symlink(), "the link itself was replaced");
    assert_eq!(std::fs::read(&dest).unwrap(), b"<html>");
    let _ = std::fs::remove_dir_all(d);
}

/// docs/butterfly.html:837 "an interrupted or failed write leaves the last
/// complete document intact" -- a destination the rename cannot replace (a
/// non-empty directory) is left as it was, with no temporary beside it.
#[test]
fn a_failed_write_leaves_the_destination_and_no_temporary() {
    let d = scratch("fail");
    let dest = d.join("out");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join("keep.txt"), "kept").unwrap();
    assert!(write_atomic(&dest, b"new").is_err(), "a directory cannot be replaced by a file");
    assert_eq!(std::fs::read_to_string(dest.join("keep.txt")).unwrap(), "kept");
    assert_eq!(names(&d), ["out"], "no temporary left behind");

    let e = write_atomic(&d.join("no-such-dir").join("x.json"), b"x");
    assert!(e.is_err(), "a missing directory is an error, not created");
    assert!(!d.join("no-such-dir").exists());
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:295-303 "each name is resolved, symlinks and `..` included ...
/// A source with more than one name (a hard link) is refused the same way".
#[test]
fn a_file_is_placed_by_where_the_kernel_says_it_is() {
    let d = scratch("place");
    let root = d.join("root");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.c"), "a").unwrap();
    std::fs::write(d.join("secret.c"), "s").unwrap();
    std::os::unix::fs::symlink(d.join("secret.c"), root.join("src/link.c")).unwrap();
    std::fs::write(root.join("src/h1.c"), "h").unwrap();
    std::fs::hard_link(root.join("src/h1.c"), root.join("src/h2.c")).unwrap();

    assert_eq!(open_placed(&root, &root.join("src/a.c")).unwrap().standing, Standing::Inside);
    assert_eq!(open_placed(&root, &root.join("src/../../secret.c")).unwrap().standing, Standing::Outside, "`..` out of the root");
    assert_eq!(open_placed(&root, &root.join("src/link.c")).unwrap().standing, Standing::Outside, "a symlink out of the root");
    assert_eq!(open_placed(&root, &root.join("src/h1.c")).unwrap().standing, Standing::Linked(2), "a second name");
    assert!(open_placed(&root, &root.join("src/missing.c")).is_err(), "a missing file is an error");
    // A sibling whose name merely starts with the root's is not inside it.
    std::fs::create_dir_all(d.join("root-evil")).unwrap();
    std::fs::write(d.join("root-evil/x.c"), "x").unwrap();
    assert_eq!(open_placed(&root, &d.join("root-evil/x.c")).unwrap().standing, Standing::Outside, "a prefix of the name is not containment");
    let _ = std::fs::remove_dir_all(d);
}
