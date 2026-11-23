// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The project's source revision, read at ingest and verified against the
//! bytes ingest read (#300).
//!
//! Every case builds a real git repository in a temporary directory. The
//! state is judged by comparing the files the build used with HEAD's tree,
//! by content -- so the controls here also show plain `git status` being
//! fooled (assume-unchanged) or running a program (core.fsmonitor), where
//! butterfly is not.

use butterfly_ingest::helper::Bounds;
use butterfly_ingest::revision::{inspect, BuildFile, Request};
use butterfly_model::revision::{Change, SourceRevision, SourceState, Vcs, LIST_CAP};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

// ---- scaffolding --------------------------------------------------------

fn scratch(tag: &str) -> PathBuf {
    let d = butterfly_testsupport::scratch_dir(&format!("rev-{tag}"));
    d.canonicalize().unwrap()
}

fn git_exe() -> PathBuf {
    butterfly_ingest::provenance::resolve_on_path("git").expect("git is needed to build the test repositories")
}

/// Run git for the test's own setup, isolated from the operator's global
/// and system configuration so a `commit.gpgsign` there cannot break it.
fn g(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(git_exe())
        .current_dir(dir)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .args(["-c", "init.defaultBranch=main", "-c", "advice.addEmbeddedRepo=false", "-c", "protocol.file.allow=always"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn write(dir: &Path, rel: &str, text: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

/// A repository holding `files`, committed.
fn repo_with(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let d = scratch(tag);
    g(&d, &["init", "-q"]);
    for (p, t) in files {
        write(&d, p, t);
    }
    g(&d, &["add", "-A"]);
    g(&d, &["commit", "-q", "-m", "one"]);
    d
}

fn standard() -> PathBuf {
    repo_with("std", &[("src/a.c", "int a(void) {\n  return 1;\n}\n"), ("src/inc.h", "#define X 1\n"), ("README.md", "readme\n")])
}

/// Inspect as ingest does: `read` files are passed with the bytes ingest
/// read (the coverage file list); `dwarf` files are named only, and the
/// layer reads them itself (headers the DWARF line tables name).
fn inspect_with(start: &Path, read: &[PathBuf], dwarf: &[PathBuf], git: Option<PathBuf>, bounds: Bounds) -> SourceRevision {
    let bytes: Vec<Vec<u8>> = read.iter().map(|p| fs::read(p).unwrap()).collect();
    let mut files: Vec<BuildFile> = read.iter().zip(&bytes).map(|(p, b)| BuildFile { path: p.clone(), bytes: Some(b.as_slice()) }).collect();
    files.extend(dwarf.iter().map(|p| BuildFile { path: p.clone(), bytes: None }));
    let mut req = Request::new(start.to_path_buf(), files);
    req.git = git;
    req.bounds = bounds;
    let r = inspect(&req);
    // every record the layer produces must pass the model's own check
    r.check().unwrap_or_else(|e| panic!("{e}: {r:#?}"));
    r
}

fn run(start: &Path, read: &[PathBuf], dwarf: &[PathBuf]) -> SourceRevision {
    inspect_with(start, read, dwarf, Some(git_exe()), Bounds::standard())
}

fn changes(r: &SourceRevision) -> Vec<(String, Change)> {
    r.modified.iter().map(|c| (c.path.clone(), c.change)).collect()
}

fn head(d: &Path) -> String {
    g(d, &["rev-parse", "HEAD"])
}

fn unknown_reason(r: &SourceRevision) -> String {
    match &r.state {
        SourceState::Unknown(why) => why.clone(),
        other => panic!("expected unknown, got {other:?}: {r:#?}"),
    }
}

// ---- normal ---------------------------------------------------------------

#[test]
fn a_clean_checkout_is_clean_and_names_its_commit() {
    let d = standard();
    let r = run(&d, &[d.join("src/a.c")], &[d.join("src/inc.h")]);
    assert_eq!(r.vcs, Vcs::Git);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.id.as_ref().unwrap().revision, head(&d));
    assert_eq!(r.verified_count, 2);
    assert_eq!(r.modified_count, 0);
    assert_eq!(r.outside_build, Some(0));
    assert_eq!(r.root.as_deref(), Some(d.to_str().unwrap()));
    let tool = r.tool.as_ref().expect("the git that ran is recorded");
    assert!(tool.version.contains("git version"), "{tool:?}");
}

#[test]
fn one_edited_source_makes_it_modified() {
    let d = standard();
    write(&d, "src/a.c", "int a(void) {\n  return 2;\n}\n");
    let r = run(&d, &[d.join("src/a.c")], &[d.join("src/inc.h")]);
    assert_eq!(r.state, SourceState::Modified);
    assert_eq!(changes(&r), vec![("src/a.c".to_string(), Change::Content)]);
    assert_eq!(r.modified_count, 1);
    assert_eq!(r.id.as_ref().unwrap().revision, head(&d), "the revision is still named: it is the label");
}

// ---- equivalence classes ----------------------------------------------------

#[test]
fn a_change_staged_but_not_committed_is_modified() {
    let d = standard();
    write(&d, "src/a.c", "int a(void) {\n  return 3;\n}\n");
    g(&d, &["add", "src/a.c"]);
    let r = run(&d, &[d.join("src/a.c")], &[]);
    assert_eq!(changes(&r), vec![("src/a.c".to_string(), Change::Content)]);
}

#[test]
fn a_deleted_header_the_build_used_is_missing_and_a_deleted_unused_file_is_counted_outside() {
    let d = standard();
    fs::remove_file(d.join("src/inc.h")).unwrap();
    fs::remove_file(d.join("README.md")).unwrap();
    let r = run(&d, &[d.join("src/a.c")], &[d.join("src/inc.h")]);
    assert_eq!(r.state, SourceState::Modified);
    assert_eq!(changes(&r), vec![("src/inc.h".to_string(), Change::Missing)]);
    assert_eq!(r.outside_build, Some(1), "README.md, which the build does not use");
}

#[test]
fn a_renamed_file_the_build_uses_is_not_in_the_revision() {
    let d = standard();
    g(&d, &["mv", "src/inc.h", "src/renamed.h"]);
    let r = run(&d, &[d.join("src/a.c")], &[d.join("src/renamed.h")]);
    assert_eq!(changes(&r), vec![("src/renamed.h".to_string(), Change::NotInRevision)]);
    assert_eq!(r.outside_build, Some(1), "the old name is gone, and the build does not use it");
}

#[test]
fn an_untracked_file_counts_only_if_the_build_used_it() {
    let d = standard();
    // generated and even ignored: compiled in, so it counts
    write(&d, ".gitignore", "gen/\n");
    g(&d, &["add", ".gitignore"]);
    g(&d, &["commit", "-q", "-m", "ignore"]);
    write(&d, "gen/config.h", "#define GENERATED 1\n");
    write(&d, "notes.txt", "scratch\n");
    let r = run(&d, &[d.join("src/a.c")], &[d.join("gen/config.h")]);
    assert_eq!(r.state, SourceState::Modified);
    assert_eq!(changes(&r), vec![("gen/config.h".to_string(), Change::NotInRevision)]);
    assert_eq!(r.outside_build, Some(1), "notes.txt is untracked but unused");

    let r = run(&d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "an untracked file the build did not use leaves it clean");
    assert_eq!(r.outside_build, Some(1), "notes.txt; the ignored generated header is a build product, not a change");
}

#[test]
fn a_change_only_outside_the_build_is_clean_and_counted_separately() {
    let d = standard();
    write(&d, "README.md", "edited\n");
    let r = run(&d, &[d.join("src/a.c")], &[d.join("src/inc.h")]);
    assert_eq!(r.state, SourceState::Clean);
    assert_eq!(r.outside_build, Some(1));
    assert!(r.modified.is_empty());
}

#[test]
fn assume_unchanged_does_not_hide_an_edit() {
    let d = standard();
    g(&d, &["update-index", "--assume-unchanged", "src/a.c"]);
    write(&d, "src/a.c", "int a(void) {\n  return 4;\n}\n");
    // the control: git itself now reports the tree clean
    assert_eq!(g(&d, &["status", "--porcelain"]), "", "git status is fooled");
    let r = run(&d, &[d.join("src/a.c")], &[]);
    assert_eq!(changes(&r), vec![("src/a.c".to_string(), Change::Content)], "butterfly compares content: {r:#?}");
}

#[test]
fn a_submodule_at_another_commit_is_modified_and_its_files_are_judged_against_its_own_head() {
    let d = standard();
    let sub = d.join("lib");
    fs::create_dir_all(&sub).unwrap();
    g(&sub, &["init", "-q"]);
    write(&sub, "lib.c", "int lib(void) { return 1; }\n");
    g(&sub, &["add", "-A"]);
    g(&sub, &["commit", "-q", "-m", "lib one"]);
    let recorded = head(&sub);
    g(&d, &["add", "lib"]);
    g(&d, &["commit", "-q", "-m", "add lib"]);

    // at the recorded commit: clean, and the submodule's commit is recorded
    let r = run(&d, &[d.join("src/a.c"), sub.join("lib.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.submodules.len(), 1);
    assert_eq!(r.submodules[0].path, "lib");
    assert_eq!(r.submodules[0].commit.as_deref(), Some(recorded.as_str()));
    assert_eq!(r.submodules[0].recorded, recorded);
    assert_eq!(r.verified_count, 2);

    // move the submodule on: its file is clean against *its* HEAD, but the
    // submodule is not at the commit the parent records
    write(&sub, "lib.c", "int lib(void) { return 2; }\n");
    g(&sub, &["commit", "-q", "-am", "lib two"]);
    let moved = head(&sub);
    let r = run(&d, &[d.join("src/a.c"), sub.join("lib.c")], &[]);
    assert_eq!(r.state, SourceState::Modified);
    assert_eq!(changes(&r), vec![("lib".to_string(), Change::SubmoduleCommit)]);
    assert_eq!(r.submodules[0].commit.as_deref(), Some(moved.as_str()));
    assert_eq!(r.submodules[0].recorded, recorded);

    // and an edit inside the submodule is judged against the submodule's HEAD
    write(&sub, "lib.c", "int lib(void) { return 3; }\n");
    let r = run(&d, &[d.join("src/a.c"), sub.join("lib.c")], &[]);
    assert!(changes(&r).contains(&("lib/lib.c".to_string(), Change::Content)), "{r:#?}");
}

// ---- boundary -------------------------------------------------------------

#[test]
fn a_repository_with_no_commits_is_unknown_with_no_id() {
    let d = scratch("empty");
    g(&d, &["init", "-q"]);
    write(&d, "a.c", "int a;\n");
    let r = run(&d, &[d.join("a.c")], &[]);
    assert!(r.id.is_none(), "{r:#?}");
    assert!(unknown_reason(&r).contains("no commits"), "{r:#?}");
}

#[test]
fn a_shallow_clone_is_judged_like_any_other() {
    let origin = standard();
    write(&origin, "src/a.c", "int a(void) {\n  return 9;\n}\n");
    g(&origin, &["commit", "-q", "-am", "two"]);
    let parent = scratch("shallow");
    g(&parent, &["clone", "-q", "--depth", "1", &format!("file://{}", origin.display()), "clone"]);
    let d = parent.join("clone");
    assert_eq!(g(&d, &["rev-parse", "--is-shallow-repository"]), "true");
    let r = run(&d, &[d.join("src/a.c")], &[d.join("src/inc.h")]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.id.as_ref().unwrap().revision, head(&origin));
    assert!(r.notes.iter().any(|n| n.contains("shallow")), "{:?}", r.notes);
}

#[test]
fn a_detached_head_names_the_commit_checked_out() {
    let d = standard();
    let first = head(&d);
    write(&d, "src/a.c", "int a(void) {\n  return 5;\n}\n");
    g(&d, &["commit", "-q", "-am", "two"]);
    g(&d, &["checkout", "-q", "--detach", &first]);
    let r = run(&d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.id.as_ref().unwrap().revision, first);
}

#[test]
fn a_sha256_repository_is_verified_with_sha256_blob_ids() {
    let d = scratch("sha256");
    g(&d, &["init", "-q", "--object-format=sha256"]);
    write(&d, "a.c", "int a;\n");
    write(&d, "b.c", "int b;\n");
    g(&d, &["add", "-A"]);
    g(&d, &["commit", "-q", "-m", "one"]);
    let r = run(&d, &[d.join("a.c"), d.join("b.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.id.as_ref().unwrap().revision.len(), 64);
    write(&d, "b.c", "int b = 2;\n");
    let r = run(&d, &[d.join("a.c"), d.join("b.c")], &[]);
    assert_eq!(changes(&r), vec![("b.c".to_string(), Change::Content)]);
}

/// The build reads bytes, not permission bits: a file whose mode alone
/// changed compiled to the same thing.
#[test]
fn a_mode_only_change_is_not_a_change_to_what_the_build_used() {
    use std::os::unix::fs::PermissionsExt;
    let d = standard();
    fs::set_permissions(d.join("src/a.c"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_ne!(g(&d, &["status", "--porcelain"]), "", "git sees the mode change");
    let r = run(&d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
}

#[test]
fn line_endings_that_differ_are_a_change_and_say_so() {
    let d = standard();
    write(&d, "src/a.c", "int a(void) {\r\n  return 1;\r\n}\r\n");
    let r = run(&d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Modified);
    assert_eq!(changes(&r), vec![("src/a.c".to_string(), Change::LineEndings)]);
}

#[test]
fn paths_with_spaces_newlines_and_non_ascii_are_matched_exactly() {
    let names = ["has space.c", "new\nline.c", "ünïcødé.c", "tab\there.c"];
    let files: Vec<(&str, &str)> = names.iter().map(|n| (*n, "int x;\n")).collect();
    let d = repo_with("paths", &files);
    let paths: Vec<PathBuf> = names.iter().map(|n| d.join(n)).collect();
    let r = run(&d, &paths, &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.verified_count, names.len() as u64);
    write(&d, "new\nline.c", "int y;\n");
    let r = run(&d, &paths, &[]);
    assert_eq!(changes(&r), vec![("new\nline.c".to_string(), Change::Content)]);
}

#[test]
fn thousands_of_modified_files_are_capped_in_the_list_and_counted_exactly() {
    let d = scratch("many");
    g(&d, &["init", "-q"]);
    let n = 3000;
    for i in 0..n {
        write(&d, &format!("src/f{i:04}.c"), format!("int f{i};\n"));
    }
    g(&d, &["add", "-A"]);
    g(&d, &["commit", "-q", "-m", "many"]);
    for i in 0..n {
        write(&d, &format!("src/f{i:04}.c"), format!("int f{i} = 1;\n"));
    }
    let paths: Vec<PathBuf> = (0..n).map(|i| d.join(format!("src/f{i:04}.c"))).collect();
    let r = run(&d, &paths, &[]);
    assert_eq!(r.state, SourceState::Modified);
    assert_eq!(r.modified_count, n as u64, "exact");
    assert_eq!(r.modified.len(), LIST_CAP, "capped");
    assert_eq!(r.modified[0].path, "src/f0000.c", "sorted");
}

#[test]
fn a_source_root_in_a_subdirectory_finds_the_repository_above_it() {
    let d = repo_with("subdir", &[("fw/src/a.c", "int a;\n"), ("docs/x.md", "x\n")]);
    let r = run(&d.join("fw"), &[d.join("fw/src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.root.as_deref(), Some(d.to_str().unwrap()));
    write(&d, "fw/src/a.c", "int a = 1;\n");
    let r = run(&d.join("fw"), &[d.join("fw/src/a.c")], &[]);
    assert_eq!(changes(&r), vec![("fw/src/a.c".to_string(), Change::Content)], "paths are relative to the repository's top");
}

#[test]
fn sources_outside_the_repository_are_listed_as_unversioned_not_modified() {
    let d = standard();
    let sdk = scratch("sdk");
    write(&sdk, "include/sdk.h", "#define SDK 1\n");
    let r = run(&d, &[d.join("src/a.c")], &[sdk.join("include/sdk.h"), PathBuf::from("/usr/include/definitely-not-here.h")]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.unversioned_count, 2);
    assert!(r.unversioned.contains(&sdk.join("include/sdk.h").display().to_string()), "{:?}", r.unversioned);
    assert!(r.modified.is_empty());
}

#[test]
fn sources_not_in_any_working_copy_are_unknown_with_no_vcs() {
    let d = scratch("novcs");
    write(&d, "a.c", "int a;\n");
    let r = run(&d, &[d.join("a.c")], &[]);
    assert_eq!(r.vcs, Vcs::None);
    assert!(unknown_reason(&r).contains("not in a git or SVN working copy"), "{r:#?}");
}

#[test]
fn credentials_are_stripped_from_the_recorded_remote() {
    let d = standard();
    g(&d, &["remote", "add", "origin", "https://someone:s3cret-token@example.invalid/team/fw.git"]);
    let r = run(&d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.repository.as_deref(), Some("https://example.invalid/team/fw.git"));
    let json = serde_json::to_string(&r).unwrap();
    assert!(!json.contains("s3cret") && !json.contains("someone"), "{json}");
}

// ---- robustness -----------------------------------------------------------

#[test]
fn no_git_is_unknown() {
    let d = standard();
    let r = inspect_with(&d, &[d.join("src/a.c")], &[], None, Bounds::standard());
    assert_eq!(r.vcs, Vcs::Git);
    assert!(unknown_reason(&r).contains("git was not found"), "{r:#?}");
    let r = inspect_with(&d, &[d.join("src/a.c")], &[], Some(d.join("no-such-git")), Bounds::standard());
    assert!(!r.state.is_clean());
    unknown_reason(&r);
}

fn stub(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("git-stub");
    fs::write(&p, format!("#!/bin/sh\ncase \"$*\" in *--version*) echo 'git version 9.9.9'; exit 0;; esac\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// Git refuses a repository owned by someone else unless it is listed in
/// `safe.directory`. Not reproducible without a second user, so a stub
/// answers as git does.
#[test]
fn a_dubious_ownership_refusal_is_unknown_and_says_why() {
    let d = standard();
    let tools = scratch("dubious");
    let git = stub(&tools, &format!("echo \"fatal: detected dubious ownership in repository at '{}'\" >&2\necho \"To add an exception for this directory, call: git config --global --add safe.directory {}\" >&2\nexit 128", d.display(), d.display()));
    let r = inspect_with(&d, &[d.join("src/a.c")], &[], Some(git), Bounds::standard());
    let why = unknown_reason(&r);
    assert!(why.contains("dubious ownership") && why.contains("safe.directory"), "{why}");
}

#[test]
fn a_hanging_git_is_stopped_and_unknown() {
    let d = standard();
    let tools = scratch("hang");
    let git = stub(&tools, "sleep 30");
    let t = Instant::now();
    let r = inspect_with(&d, &[d.join("src/a.c")], &[], Some(git), Bounds::standard().with_deadline(Duration::from_millis(500)));
    assert!(t.elapsed() < Duration::from_secs(10), "bounded: {:?}", t.elapsed());
    assert!(unknown_reason(&r).contains("did not finish"), "{r:#?}");
}

/// An `index.lock` is a note, not a doubt. Files are compared by their
/// bytes with the tree of the commit recorded, by its id, and neither the
/// index nor a lock can change that tree. So a leftover lock from an
/// interrupted git does not turn a proven result into unknown. A checkout
/// caught mid-way still shows: its files would not all match the recorded
/// commit.
#[test]
fn an_index_lock_is_noted_but_does_not_unsettle_a_proven_result() {
    let d = standard();
    fs::write(d.join(".git/index.lock"), "").unwrap();
    let r = run(&d, &[d.join("src/a.c")], &[]);
    assert!(r.state.is_clean(), "every file matched the recorded commit: {r:#?}");
    assert_eq!(r.id.as_ref().map(|i| i.revision.clone()), Some(head(&d)));
    assert!(r.notes.iter().any(|n| n.contains("index.lock")), "the lock is still reported: {:?}", r.notes);
    write(&d, "src/a.c", "int a(void) {\n  return 7;\n}\n");
    let r = run(&d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Modified);
    assert!(r.notes.iter().any(|n| n.contains("index.lock")), "{:?}", r.notes);
}

#[test]
fn a_corrupt_repository_is_unknown() {
    let d = standard();
    // remove HEAD's tree object: the commit is still named, but its files
    // cannot be read
    let tree = g(&d, &["rev-parse", "HEAD^{tree}"]);
    fs::remove_file(d.join(".git/objects").join(&tree[..2]).join(&tree[2..])).unwrap();
    let r = run(&d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.vcs, Vcs::Git);
    assert_eq!(r.id.as_ref().map(|i| i.revision.clone()), Some(head(&d)), "the commit is still named");
    assert!(unknown_reason(&r).contains("tree"), "{r:#?}");
}

/// A repository's own config can name a program for git to run. Plain git
/// runs it; butterfly's git must not.
#[test]
fn a_core_fsmonitor_command_in_the_repository_config_does_not_run() {
    let d = standard();
    let marker = d.parent().unwrap().join(format!("{}-fsmonitor-ran", d.file_name().unwrap().to_string_lossy()));
    let _ = fs::remove_file(&marker);
    g(&d, &["config", "core.fsmonitor", &format!("touch {}; false", marker.display())]);
    write(&d, "src/a.c", "int a(void) {\n  return 8;\n}\n");
    write(&d, "untracked.txt", "u\n");

    // the control: plain git runs it
    let _ = Command::new(git_exe()).current_dir(&d).env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").args(["status", "--porcelain"]).output().unwrap();
    assert!(marker.exists(), "the control: plain `git status` runs core.fsmonitor");
    fs::remove_file(&marker).unwrap();

    let r = run(&d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Modified, "{r:#?}");
    assert!(!marker.exists(), "butterfly ran the repository's core.fsmonitor command");
    let _ = fs::remove_file(&marker);
}

// ---- through ingest --------------------------------------------------------

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

#[test]
fn ingest_records_the_source_revision_and_a_caveat_when_it_is_not_clean() {
    let d = standard();
    let out = scratch("ingest-out");
    let a = d.join("src/a.c");
    fs::write(out.join("coverage.json"), export(&[a.to_str().unwrap()])).unwrap();
    let opts = butterfly_ingest::IngestOptions { coverage_json: out.join("coverage.json"), source_root: Some(d.clone()), ..Default::default() };

    let ev = butterfly_ingest::ingest(&opts).unwrap();
    let r = ev.provenance.source_revision.as_ref().expect("recorded");
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.id.as_ref().unwrap().revision, head(&d));
    assert!(!ev.provenance.caveats.iter().any(|c| c.contains("source revision")), "{:?}", ev.provenance.caveats);
    assert!(ev.validate().is_ok());
    assert!(ev.provenance.external_tools.iter().any(|t| t.name == "git"), "git ran, so it is identified: {:?}", ev.provenance.external_tools);

    write(&d, "src/a.c", "int a(void) {\n  return 6;\n}\n");
    let ev = butterfly_ingest::ingest(&opts).unwrap();
    let r = ev.provenance.source_revision.as_ref().unwrap();
    assert_eq!(r.state, SourceState::Modified);
    assert!(ev.provenance.caveats.iter().any(|c| c.contains("source revision") && c.contains("modified") && c.contains("src/a.c")), "{:?}", ev.provenance.caveats);
    // not a failure: ingest succeeded and the evidence loads
    assert!(ev.validate().is_ok());
}

/// The tree judged is the tree of the commit recorded. A commit that lands
/// between reading HEAD and reading its tree must not let the record name
/// the old commit while matching the files against the new one's tree --
/// which here would call an edit that the old commit does not contain
/// clean (#306 review). The stub is real git that commits the pending edit
/// right after answering `rev-parse HEAD`.
#[test]
fn the_tree_judged_is_the_tree_of_the_commit_recorded() {
    let d = standard();
    let before = head(&d);
    write(&d, "src/a.c", "int a(void) {\n  return 2;\n}\n");
    let tools = scratch("race");
    let real = git_exe();
    let git = stub(
        &tools,
        &format!(
            "{real:?} \"$@\"; rc=$?\ncase \"$*\" in *\"rev-parse --verify --quiet HEAD\"*) {real:?} -C {d:?} -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false commit -q -a -m race >/dev/null 2>&1;; esac\nexit $rc",
        ),
    );
    let r = inspect_with(&d, &[d.join("src/a.c")], &[], Some(git), Bounds::standard());
    assert_ne!(head(&d), before, "the stub must have committed in between, or this test proves nothing");
    assert_eq!(r.id.as_ref().map(|i| i.revision.as_str()), Some(before.as_str()), "the record names the commit read first: {r:#?}");
    assert!(matches!(r.state, SourceState::Modified), "the edit is not in the recorded commit, so the record must not read clean: {r:#?}");
    assert_eq!(changes(&r), vec![("src/a.c".to_string(), Change::Content)]);
}

// ---- what a clean record covers (#314) ----------------------------------

/// A header the build uses only for its types or macros is named by neither
/// the coverage export nor the line tables. Changed, it changes what was
/// compiled, so a record that reads clean must say that other files differ
/// rather than keep the count to itself (#314).
#[test]
fn a_clean_record_says_that_files_the_evidence_does_not_name_differ() {
    let d = repo_with("unnamed-header", &[("main.c", "#include \"config.h\"\nint main(void) { return LIMIT; }\n"), ("config.h", "#define LIMIT 10\n")]);
    // the control: nothing changed, nothing to say
    let r = run(&d, &[d.join("main.c")], &[]);
    assert_eq!((r.state.clone(), r.outside_build), (SourceState::Clean, Some(0)), "{r:#?}");
    assert_eq!(butterfly_ingest::revision::caveat(&r), None);

    write(&d, "config.h", "#define LIMIT 99\n");
    let r = run(&d, &[d.join("main.c")], &[]);
    assert_eq!(r.outside_build, Some(1), "{r:#?}");
    let c = butterfly_ingest::revision::caveat(&r).unwrap_or_default();
    assert!(c.contains("1 other file"), "a clean record over a changed file the evidence does not name must carry a caveat that says so: {c:?} {r:#?}");
}

/// HEAD holds `a.c` as a symlink whose target text is exactly the bytes of
/// the regular file `a.c` on disk: one blob id, but not the same file --
/// `git status` reports a type change (#314).
#[test]
fn a_symlink_in_the_revision_is_not_the_regular_file_with_its_bytes() {
    let d = scratch("typechange");
    g(&d, &["init", "-q"]);
    std::os::unix::fs::symlink("int limit = 99;", d.join("a.c")).unwrap();
    g(&d, &["add", "-A"]);
    g(&d, &["commit", "-q", "-m", "link"]);
    fs::remove_file(d.join("a.c")).unwrap();
    write(&d, "a.c", "int limit = 99;");
    assert!(g(&d, &["status", "--porcelain"]).contains("T a.c"), "the control: git sees a type change");
    let r = run(&d, &[d.join("a.c")], &[]);
    assert_eq!(r.state, SourceState::Modified, "{r:#?}");
    assert_eq!(changes(&r), vec![("a.c".to_string(), Change::Content)]);
}
