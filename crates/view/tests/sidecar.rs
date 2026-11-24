// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The resolutions sidecar is a reviewer's signed-off record. A write to it
//! must never destroy what is already there (#2).

use butterfly_view::sidecar::{write_atomic, Wrote};
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    // a fresh, unpredictable directory, not a name another user could
    // plant a symlink at (#344)
    tempfile::Builder::new().prefix(&format!("butterfly-sidecar-{tag}-")).tempdir().unwrap().keep()
}

/// Anything the write left beside the destination.
fn strays(dir: &Path, dest: &Path) -> Vec<String> {
    fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p != dest).map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
}

#[test]
fn it_writes_the_document_and_leaves_nothing_beside_it() {
    let d = scratch("plain");
    let dest = d.join("resolutions.json");
    assert_eq!(write_atomic(&dest, br#"{"entries":[]}"#).unwrap(), Wrote::Normally);
    assert_eq!(fs::read_to_string(&dest).unwrap(), r#"{"entries":[]}"#);
    assert_eq!(write_atomic(&dest, br#"{"entries":[1]}"#).unwrap(), Wrote::Normally, "and replaces it");
    assert_eq!(fs::read_to_string(&dest).unwrap(), r#"{"entries":[1]}"#);
    assert!(strays(&d, &dest).is_empty(), "no temporary file survives: {:?}", strays(&d, &dest));
    let _ = fs::remove_dir_all(d);
}

#[cfg(unix)]
#[test]
fn a_symlinked_destination_is_replaced_never_written_through() {
    let d = scratch("symlink");
    let victim = d.join("victim.json");
    fs::write(&victim, "PRECIOUS").unwrap();
    let dest = d.join("resolutions.json");
    std::os::unix::fs::symlink(&victim, &dest).unwrap();

    let how = write_atomic(&dest, b"NEW").unwrap();
    assert_eq!(how, Wrote::ReplacedSymlink(victim.clone()), "the caller is told what it displaced");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "PRECIOUS", "the symlink's target is untouched");
    assert_eq!(fs::read_to_string(&dest).unwrap(), "NEW");
    assert!(!fs::symlink_metadata(&dest).unwrap().file_type().is_symlink(), "the destination is a regular file now");
    let _ = fs::remove_dir_all(d);
}

#[cfg(unix)]
#[test]
fn a_symlink_planted_after_the_first_write_still_cannot_reach_its_target() {
    // the time-of-check/time-of-use path in #2: a legitimate sidecar is
    // loaded at startup and swapped for a symlink before a later write
    let d = scratch("toctou");
    let dest = d.join("resolutions.json");
    write_atomic(&dest, br#"{"entries":[]}"#).unwrap();

    let victim = d.join("victim.json");
    fs::write(&victim, "PRECIOUS").unwrap();
    fs::remove_file(&dest).unwrap();
    std::os::unix::fs::symlink(&victim, &dest).unwrap();

    write_atomic(&dest, b"NEW").unwrap();
    assert_eq!(fs::read_to_string(&victim).unwrap(), "PRECIOUS");
    let _ = fs::remove_dir_all(d);
}

#[cfg(unix)]
#[test]
fn a_failed_write_preserves_the_last_complete_document() {
    use std::os::unix::fs::PermissionsExt;
    let d = scratch("failure");
    let dest = d.join("resolutions.json");
    write_atomic(&dest, br#"{"entries":["signed off"]}"#).unwrap();

    // no new file can be created beside it
    fs::set_permissions(&d, fs::Permissions::from_mode(0o500)).unwrap();
    let err = write_atomic(&dest, b"NEW").unwrap_err();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();

    assert!(err.to_string().to_lowercase().contains("permission"), "{err}");
    assert_eq!(fs::read_to_string(&dest).unwrap(), r#"{"entries":["signed off"]}"#, "the old document is still whole");
    assert!(strays(&d, &dest).is_empty(), "and no temporary file is left: {:?}", strays(&d, &dest));
    let _ = fs::remove_dir_all(d);
}

#[cfg(unix)]
#[test]
fn the_destination_keeps_the_permissions_it_had() {
    use std::os::unix::fs::PermissionsExt;
    let d = scratch("perms");
    let dest = d.join("resolutions.json");
    fs::write(&dest, "{}").unwrap();
    fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).unwrap();
    write_atomic(&dest, b"{}").unwrap();
    let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "a reviewer's 0600 sidecar does not become world-readable");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn a_reader_never_sees_a_partial_document() {
    let d = scratch("concurrent");
    let dest = d.join("resolutions.json");
    // big enough that a non-atomic write would be caught mid-flight
    let doc = |writer: usize| format!(r#"{{"writer":{writer},"entries":[{}]}}"#, (0..2000).map(|i| i.to_string()).collect::<Vec<_>>().join(","));
    write_atomic(&dest, doc(0).as_bytes()).unwrap();

    std::thread::scope(|s| {
        for n in 0..4 {
            let dest = dest.clone();
            s.spawn(move || {
                for _ in 0..25 {
                    write_atomic(&dest, doc(n).as_bytes()).unwrap();
                }
            });
        }
        s.spawn(|| {
            for _ in 0..200 {
                let text = fs::read_to_string(&dest).unwrap();
                serde_json::from_str::<serde_json::Value>(&text).unwrap_or_else(|e| panic!("a reader saw a partial document ({} bytes): {e}", text.len()));
            }
        });
    });
    assert!(strays(&d, &dest).is_empty(), "{:?}", strays(&d, &dest));
    let _ = fs::remove_dir_all(d);
}

/// Lay a link to `victim` at every name the old scheme could have chosen for
/// `resolutions.json` in this process. The counter it used was static and
/// shared, so its value here depends on what the rest of the suite has
/// already written -- hence a range rather than a single name.
#[cfg(unix)]
fn plant_the_old_temporary_names(dir: &Path, victim: &Path) {
    for n in 0..4096 {
        let p = dir.join(format!(".resolutions.json.tmp-{}-{n}", std::process::id()));
        std::os::unix::fs::symlink(victim, &p).unwrap();
    }
}

/// The destination was guarded and the temporary file was not. Its name was
/// `.<name>.tmp-<pid>-<counter>`, which anyone can compute, and it was opened
/// with `File::create` -- so preplanting that name as a link to any file
/// butterfly may write got it truncated before the rename (review of #22).
#[cfg(unix)]
#[test]
fn a_preplanted_temporary_name_is_not_written_through() {
    let d = scratch("preplanted-tmp");
    let dest = d.join("resolutions.json");
    let victim = d.join("victim.json");
    fs::write(&victim, "PRECIOUS").unwrap();

    plant_the_old_temporary_names(&d, &victim);

    write_atomic(&dest, b"NEW").unwrap();
    assert_eq!(fs::read_to_string(&victim).unwrap(), "PRECIOUS", "the preplanted link's target is untouched");
    assert_eq!(fs::read_to_string(&dest).unwrap(), "NEW", "and the sidecar was still written");
    let _ = fs::remove_dir_all(d);
}

/// The same trap laid at the destination *and* across the old temporary
/// names at once: neither route may reach the target.
#[cfg(unix)]
#[test]
fn a_symlinked_destination_and_a_preplanted_temporary_both_fail_to_reach_the_target() {
    let d = scratch("preplanted-both");
    let victim = d.join("victim.json");
    fs::write(&victim, "PRECIOUS").unwrap();
    let dest = d.join("resolutions.json");
    std::os::unix::fs::symlink(&victim, &dest).unwrap();
    plant_the_old_temporary_names(&d, &victim);

    assert_eq!(write_atomic(&dest, b"NEW").unwrap(), Wrote::ReplacedSymlink(victim.clone()));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "PRECIOUS");
    let _ = fs::remove_dir_all(d);
}

/// A first sidecar is as readable as `fs::write` would have made it. The
/// exclusive-create fix briefly created it 0600, which hid a team's review
/// record from every other account on the machine (self-review of #22).
#[cfg(unix)]
#[test]
fn a_new_sidecar_gets_the_ordinary_default_mode() {
    use std::os::unix::fs::PermissionsExt;
    let d = scratch("newmode");
    let dest = d.join("resolutions.json");
    write_atomic(&dest, b"{}").unwrap();
    let plain = d.join("plain.json");
    fs::File::create(&plain).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dest), mode(&plain), "the sidecar's mode is what a plain create gives under this umask");
    let _ = fs::remove_dir_all(d);
}
