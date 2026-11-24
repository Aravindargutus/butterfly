// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests, area D: staleness of the change history.
//!
//! Expected values come from README.md "Staleness" (README.md:1323-1330) and
//! docs/butterfly.html "Staleness" (docs/butterfly.html:595-597), not from the
//! implementation. Each case builds its own scratch tree (and, where git is
//! needed, its own throwaway repository).

use butterfly_view::freshness::{check, Authority, Input};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn have_git() -> bool {
    Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

macro_rules! need_git {
    () => {
        if !have_git() {
            butterfly_testsupport::fixture_missing("git not installed");
            return;
        }
    };
}

fn scratch(tag: &str) -> PathBuf {
    // a fresh, unpredictable directory, not a name another user could
    // plant a symlink at (#344)
    let d = tempfile::Builder::new().prefix(&format!("butterfly-reqd-fresh-{tag}-")).tempdir().unwrap().keep();
    d.canonicalize().unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, msg: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

const TEXT: &str = "int a(void) {\n    return 1;\n}\n";

/// A repository with one committed source, and the freshness input of
/// evidence whose history was read at that commit.
fn repo_with_history(tag: &str) -> (PathBuf, Input, Authority) {
    let d = scratch(tag);
    git(&d, &["init", "-q", "-b", "main"]);
    fs::write(d.join("a.c"), TEXT).unwrap();
    let head = commit(&d, "ABC-1: a");
    let inp = Input {
        files: vec![("a.c".into(), d.join("a.c").display().to_string(), sha(TEXT.as_bytes()))],
        repo: Some(d.display().to_string()),
        head: Some(head),
        since: Some("v1.2.0".into()),
        has_changes: true,
        has_impact: true,
        impact_stale: None,
        requirements_stale: vec![],
    };
    let auth = Authority::resolve(Some(&d), None).unwrap();
    (d, inp, auth)
}

/// README.md:1323-1324 -- "The evidence records the HEAD its history was read
/// at and the hash of every analysed source": that is what the check is fed.
#[test]
fn s01_the_check_is_fed_the_recorded_head_and_every_source_hash() {
    let mut ev = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
    ev.files.push(butterfly_model::SourceFile { id: 0, path: "/s/a.c".into(), display: "a.c".into(), sha256: "ab".repeat(32), ..Default::default() });
    ev.files.push(butterfly_model::SourceFile { id: 1, path: "/s/b.c".into(), display: "b.c".into(), sha256: "cd".repeat(32), ..Default::default() });
    let none = Input::of(&ev);
    assert!(!none.has_changes && none.head.is_none() && none.repo.is_none());
    assert_eq!(none.files.len(), 2, "every analysed source");
    ev.changes = Some(butterfly_model::Changes { repo: "/s".into(), head: "f".repeat(40), since: Some("v1".into()), ..Default::default() });
    let inp = Input::of(&ev);
    assert!(inp.has_changes && !inp.has_impact);
    assert_eq!(inp.head.as_deref(), Some("f".repeat(40).as_str()));
    assert_eq!(inp.repo.as_deref(), Some("/s"));
    assert_eq!(inp.since.as_deref(), Some("v1"));
    assert_eq!(inp.files[1], ("b.c".to_string(), "/s/b.c".to_string(), "cd".repeat(32)));
}

/// README.md:1325-1330 -- nothing has moved: not stale, nothing to re-run.
#[test]
fn s02_nothing_moved_is_fresh() {
    need_git!();
    let (d, inp, auth) = repo_with_history("fresh");
    let f = check(&inp, 0, &d.join("evidence.json"), &auth);
    assert!(!f.stale, "{:?}", f.reasons);
    assert!(f.rerun.is_none() && f.reasons.is_empty());
    assert_eq!(f.current_head, inp.head);
    let _ = fs::remove_dir_all(&d);
}

/// README.md:1325-1329 -- the repository moved on: stale, and the exact
/// command is `butterfly changes …` (with the same --since) "when the history
/// is behind".
#[test]
fn s03_new_commits_make_the_history_stale_and_say_to_rerun_changes() {
    need_git!();
    let (d, inp, auth) = repo_with_history("moved");
    fs::write(d.join("b.c"), "int b;\n").unwrap();
    commit(&d, "ABC-2: b");
    commit(&d, "ABC-3: nothing");
    let ev = d.join("evidence.json");
    let f = check(&inp, 0, &ev, &auth);
    assert!(f.stale);
    assert_eq!(f.commits_behind, Some(2));
    assert_eq!(f.rerun.as_deref(), Some(format!("butterfly changes {} --since v1.2.0", ev.display()).as_str()));
    let _ = fs::remove_dir_all(&d);
}

/// README.md:1325-1330 -- the history was read at a commit the repository no
/// longer has (rebased away): that has "moved" too, and is never presented as
/// current.
#[test]
fn s04_a_recorded_head_the_repository_does_not_have_is_stale() {
    need_git!();
    let (d, mut inp, auth) = repo_with_history("rebased");
    inp.head = Some("0123456789abcdef0123456789abcdef01234567".into());
    let f = check(&inp, 0, &d.join("evidence.json"), &auth);
    assert!(f.stale, "{:?}", f);
    assert!(f.reasons.iter().any(|r| r.contains("0123456789ab")), "names the recorded HEAD: {:?}", f.reasons);
    assert!(f.rerun.as_deref().unwrap_or("").starts_with("butterfly changes"), "{:?}", f.rerun);
    let _ = fs::remove_dir_all(&d);
}

/// README.md:1326-1328 -- "counts the resolutions written since the
/// obligations were computed" and gives "`butterfly impact … --write` when only
/// resolutions changed".
#[test]
fn s05_only_resolutions_written_says_to_rerun_impact() {
    need_git!();
    let (d, inp, auth) = repo_with_history("resol");
    let ev = d.join("evidence.json");
    let f = check(&inp, 3, &ev, &auth);
    assert!(f.stale);
    assert_eq!(f.resolution_edits, 3);
    assert!(f.reasons.iter().any(|r| r.contains('3')), "{:?}", f.reasons);
    assert_eq!(f.rerun.as_deref(), Some(format!("butterfly impact {} --write", ev.display()).as_str()));
    let _ = fs::remove_dir_all(&d);
}

/// README.md:1279-1281 and docs/butterfly.html:596 -- "a file has uncommitted
/// edits" is a reason. Here the evidence was built from the uncommitted text,
/// so the hash on disk matches; the uncommitted edit alone makes it stale.
#[test]
fn s06_uncommitted_edits_alone_make_the_history_stale() {
    need_git!();
    let (d, mut inp, auth) = repo_with_history("uncommitted");
    let edited = "int a(void) {\n    return 2;\n}\n";
    fs::write(d.join("a.c"), edited).unwrap();
    inp.files[0].2 = sha(edited.as_bytes());
    let f = check(&inp, 0, &d.join("evidence.json"), &auth);
    assert!(f.sources_modified.is_empty(), "the analysed text is what is on disk");
    assert_eq!(f.uncommitted.len(), 1, "{:?}", f);
    assert!(f.stale);
    assert!(f.rerun.as_deref().unwrap_or("").starts_with("butterfly changes"));
    let _ = fs::remove_dir_all(&d);
}

/// docs/butterfly.html:597 -- "A source that cannot be read counts too, and
/// is reported apart from one that changed: missing when it is no longer on
/// this machine, unreadable when the read failed for another reason, each
/// naming the file and why. Neither is a match."
#[test]
fn s07_missing_and_unreadable_sources_are_kept_apart_and_never_match() {
    let d = scratch("unread");
    let gone = d.join("gone.c");
    let dir = d.join("dir.c");
    fs::create_dir_all(&dir).unwrap();
    let inp = Input {
        files: vec![("gone.c".into(), gone.display().to_string(), sha(b"x")), ("dir.c".into(), dir.display().to_string(), sha(b"y"))],
        repo: None,
        head: Some("0".repeat(40)),
        since: None,
        has_changes: true,
        has_impact: false,
        impact_stale: None,
        requirements_stale: vec![],
    };
    let auth = Authority { source_root: d.clone(), repo: None };
    let f = check(&inp, 0, Path::new("e.json"), &auth);
    assert!(f.stale);
    assert_eq!(f.sources_missing, vec!["gone.c".to_string()]);
    assert_eq!(f.sources_unreadable.len(), 1, "{:?}", f);
    assert!(f.sources_unreadable[0].starts_with("dir.c (") && f.sources_unreadable[0].len() > "dir.c ()".len(), "names the file and why: {:?}", f.sources_unreadable);
    assert!(f.sources_modified.is_empty(), "neither is reported as changed");
    assert!(f.reasons.iter().any(|r| r.contains("gone.c")) && f.reasons.iter().any(|r| r.contains("dir.c")), "{:?}", f.reasons);
    let _ = fs::remove_dir_all(&d);
}

/// docs/butterfly.html:597 -- evidence viewed away from the tree it was built
/// from is never presented as current, and each source is named.
#[test]
fn s08_evidence_viewed_away_from_its_tree_is_not_current() {
    let d = scratch("away");
    let inp = Input {
        files: vec![("a.c".into(), "/nonexistent-ci-host/build/src/a.c".into(), sha(b"x")), ("b.c".into(), "/nonexistent-ci-host/build/src/b.c".into(), sha(b"y"))],
        repo: None,
        head: Some("0".repeat(40)),
        since: None,
        has_changes: true,
        has_impact: false,
        impact_stale: None,
        requirements_stale: vec![],
    };
    let auth = Authority { source_root: d.clone(), repo: None };
    let f = check(&inp, 0, Path::new("e.json"), &auth);
    assert!(f.stale);
    let unknown = f.sources_missing.len() + f.sources_unapproved.len() + f.sources_unreadable.len();
    assert_eq!(unknown, 2, "both sources are unverified: {:?}", f);
    assert!(f.sources_modified.is_empty());
    let _ = fs::remove_dir_all(&d);
}

/// docs/butterfly.html "Staleness" and README.md "Staleness": evidence viewed
/// away from the tree it was built from puts every source in *not checked*
/// -- outside the approved `--source-root`, never opened, and named with the
/// flag that approves the tree.
///
/// This test first expected *missing*, from a sentence in the manual that
/// predates #248. Reporting such a source as missing would need the viewer
/// to find out whether a path outside the approved root exists, which is
/// exactly the probe #248 closed: a path outside the root gets the same
/// answer whether or not it exists (README "The freshness check reads only
/// what you approved"). The docs were corrected to the post-#248 behaviour
/// (#325 D6); this checks that behaviour, including that it is not a match.
#[test]
fn s09_evidence_viewed_away_from_its_tree_puts_every_source_in_not_checked() {
    let d = scratch("away-missing");
    let inp = Input {
        files: vec![("a.c".into(), "/nonexistent-ci-host/build/src/a.c".into(), sha(b"x"))],
        repo: None,
        head: Some("0".repeat(40)),
        since: None,
        has_changes: true,
        has_impact: false,
        impact_stale: None,
        requirements_stale: vec![],
    };
    let auth = Authority { source_root: d.clone(), repo: None };
    let f = check(&inp, 0, Path::new("e.json"), &auth);
    assert!(f.stale, "{:?}", f);
    assert_eq!(f.sources_unapproved, vec!["a.c (outside the approved source root)".to_string()], "{:?}", f);
    assert!(f.sources_missing.is_empty() && f.sources_unreadable.is_empty() && f.sources_modified.is_empty(), "{:?}", f);
    assert!(f.reasons.iter().any(|r| r.contains("were not read") && r.contains("--source-root")), "{:?}", f.reasons);
    let _ = fs::remove_dir_all(&d);
}

/// README.md:1325 -- "the repository, when it is there": found from the
/// source root without being named; a named one that does not exist is an
/// error, not a silent "no repository".
#[test]
fn s10_the_repository_is_found_from_the_source_root_or_refused_by_name() {
    let d = scratch("auth");
    fs::create_dir_all(d.join(".git")).unwrap();
    fs::create_dir_all(d.join("src")).unwrap();
    let a = Authority::resolve(Some(&d.join("src")), None).unwrap();
    assert_eq!(a.repo.as_deref(), Some(d.as_path()));
    assert_eq!(a.source_root, d.join("src"));
    let err = Authority::resolve(Some(&d), Some(Path::new("/nonexistent-repo-xyz"))).unwrap_err();
    assert!(format!("{err:#}").contains("/nonexistent-repo-xyz"), "{err:#}");
    let _ = fs::remove_dir_all(&d);
}
