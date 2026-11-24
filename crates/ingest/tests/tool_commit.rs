// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The tool's own commit in provenance (#117): every evidence file names the
//! commit of butterfly that wrote it, or says `unknown`, and never carries a
//! commit that is not the one the binary was built from.
//!
//! Fixture-free: a one-function coverage export in a scratch directory, no
//! objects. What is under test is what `build.rs` captured and `ingest`
//! copied, not the analysis.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("butterfly-tool-commit-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

fn export(file: &Path) -> String {
    let p = file.to_str().unwrap();
    format!(
        r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[[1,1,1,true,true,false]],"branches":[],"summary":{{}}}}],
           "functions":[{{"name":"f","count":1,"regions":[[1,1,3,2,1,0,0,0]],"branches":[],"filenames":[{p:?}]}}]}}]}}"#
    )
}

fn is_commit_id(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// git in `dir`, with the variables that could point it at another
/// repository cleared -- as the build script runs it.
fn git_in(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(dir).env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE").output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end_matches(['\n', '\r']).to_string())
}

/// The commit of the butterfly checkout `manifest_dir` is in, by the build
/// script's own rule: inside a work tree whose top level is the workspace
/// root two levels up. A source archive unpacked inside some other
/// repository is not a checkout of butterfly; the build says `unknown`
/// there, and so must this, or the test expects the other project's HEAD
/// (#349 review).
fn head_of_butterfly_checkout(manifest_dir: &Path) -> Option<String> {
    if git_in(manifest_dir, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        return None;
    }
    let top = PathBuf::from(git_in(manifest_dir, &["rev-parse", "--show-toplevel"])?);
    if top.canonicalize().ok()? != manifest_dir.join("../..").canonicalize().ok()? {
        return None;
    }
    git_in(manifest_dir, &["rev-parse", "HEAD"]).filter(|id| is_commit_id(id))
}

#[test]
fn fresh_evidence_names_the_commit_butterfly_was_built_from_or_says_unknown() {
    let d = scratch("ingest");
    let src = d.join("a.c");
    fs::write(&src, "int f(void) {\n  return 1;\n}\n").unwrap();
    fs::write(d.join("coverage.json"), export(&src)).unwrap();
    let opts = butterfly_ingest::IngestOptions { coverage_json: d.join("coverage.json"), source_root: Some(d.clone()), ..Default::default() };
    let ev = butterfly_ingest::ingest(&opts).unwrap();
    assert!(ev.validate().is_ok());

    let pv = &ev.provenance;
    assert_eq!(pv.tool_version, butterfly_ingest::TOOL_VERSION);
    assert_eq!(pv.tool_commit, butterfly_ingest::TOOL_COMMIT, "the evidence carries what the build captured");
    assert_eq!(pv.tool_dirty, butterfly_ingest::tool_dirty());
    assert!(!pv.tool_commit.is_empty(), "a fresh ingest always records the field");
    assert!(is_commit_id(&pv.tool_commit) || pv.tool_commit == "unknown", "a full commit id or the word unknown, never a guess: {:?}", pv.tool_commit);
    if pv.tool_commit == "unknown" {
        assert_eq!(pv.tool_dirty, None, "no commit, no verdict on the tree");
    }
    if let Some(head) = head_of_butterfly_checkout(Path::new(env!("CARGO_MANIFEST_DIR"))) {
        assert_eq!(pv.tool_commit, head, "built from this checkout, so the evidence names its HEAD");
    }

    // and it is written in camelCase beside toolVersion, where a reader of
    // the file looks for it
    let v = serde_json::to_value(&ev).unwrap();
    assert_eq!(v["provenance"]["toolCommit"], pv.tool_commit);
    assert!(v["provenance"].get("toolDirty").is_some(), "the tree state is always written, null when unknown");

    // the banner the binary prints is the same description the report uses
    let banner = butterfly_ingest::tool_version_and_build();
    assert!(banner.starts_with(butterfly_ingest::TOOL_VERSION), "{banner}");
    assert!(banner.contains(&pv.tool_build()), "{banner} vs {}", pv.tool_build());
    let _ = fs::remove_dir_all(d);
}

// -- the build script itself, in a workspace of its own --------------------
//
// The answers above are whatever this test binary was built with. What
// follows builds a one-crate workspace laid out like butterfly's, with this
// crate's own build.rs, and asks its binary what that script captured --
// so the cases a normal checkout never hits are exercised for real.

/// git for the fixtures: an identity and nothing from the developer's own
/// configuration, so a commit is made the same way everywhere.
fn git_fixture(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=butterfly-test", "-c", "user.email=test@butterfly.invalid", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// git and cargo both there, or the fixture tests say what is missing.
fn tools_present() -> bool {
    let ok = |c: &str| Command::new(c).arg("--version").output().is_ok_and(|o| o.status.success());
    if !ok("git") {
        butterfly_testsupport::fixture_missing("git, to build a checkout fixture for the tool-commit build script");
        return false;
    }
    true
}

/// A workspace at `root` with `crates/ingest` holding this crate's build.rs
/// and a binary that prints `<commit> <dirty>`; its target directory beside
/// it, outside the tree the script watches and git reports on.
fn workspace(root: &Path) -> PathBuf {
    let krate = root.join("crates/ingest");
    fs::create_dir_all(krate.join("src")).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/ingest\"]\nresolver = \"2\"\n").unwrap();
    fs::write(krate.join("Cargo.toml"), "[package]\nname = \"tool-commit-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n").unwrap();
    fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("build.rs"), krate.join("build.rs")).unwrap();
    fs::write(krate.join("src/main.rs"), "fn main() {\n    println!(\"{} {}\", env!(\"BUTTERFLY_TOOL_COMMIT\"), env!(\"BUTTERFLY_TOOL_DIRTY\"));\n}\n").unwrap();
    // cargo writes the lock file on the first build; it is not a source
    fs::write(root.join(".gitignore"), "Cargo.lock\n").unwrap();
    krate
}

/// Build the fixture incrementally and run it: what its build script
/// captured, as `(commit, dirty)`.
fn build_and_ask(root: &Path, target: &Path, build_commit: Option<&str>) -> (String, String) {
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let mut c = Command::new(cargo);
    c.args(["build", "--offline", "--quiet"]).current_dir(root).env("CARGO_TARGET_DIR", target).env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE");
    match build_commit {
        Some(v) => c.env("BUTTERFLY_BUILD_COMMIT", v),
        None => c.env_remove("BUTTERFLY_BUILD_COMMIT"),
    };
    let out = c.output().expect("spawn cargo");
    assert!(out.status.success(), "fixture build failed: {}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(target.join("debug/tool-commit-fixture")).output().expect("run the fixture");
    let said = String::from_utf8_lossy(&run.stdout).trim().to_string();
    let (commit, dirty) = said.split_once(' ').unwrap_or_else(|| panic!("fixture said {said:?}"));
    (commit.to_string(), dirty.to_string())
}

/// A packed branch has no loose ref file, and the next commit on it creates
/// one without touching HEAD, packed-refs or the index. The next build must
/// still see that commit, not keep the one before it (#349 review).
#[test]
fn a_commit_on_a_packed_branch_is_seen_by_the_next_build() {
    if !tools_present() {
        return;
    }
    let base = scratch("packed");
    let (root, target) = (base.join("ws"), base.join("target"));
    fs::create_dir_all(&root).unwrap();
    workspace(&root);
    git_fixture(&root, &["init", "-q"]);
    git_fixture(&root, &["add", "-A"]);
    git_fixture(&root, &["commit", "-q", "-m", "fixture"]);
    git_fixture(&root, &["pack-refs", "--all", "--prune"]);
    let branch = git_fixture(&root, &["symbolic-ref", "HEAD"]);
    assert!(!root.join(".git").join(&branch).exists(), "the branch ref is packed, with no loose file");

    let first = git_fixture(&root, &["rev-parse", "HEAD"]);
    assert_eq!(build_and_ask(&root, &target, None), (first.clone(), "false".into()));

    // A new commit on the branch, made so that only the loose ref appears:
    // `git commit` also rewrites the index on some git versions, and the
    // index is watched, which would rerun the script for the wrong reason
    // and hide the case. commit-tree + update-ref touch neither HEAD (a
    // symbolic ref), packed-refs nor the index.
    let index = root.join(".git/index");
    let index_before = fs::metadata(&index).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let second = git_fixture(&root, &["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "nothing but a new commit"]);
    git_fixture(&root, &["update-ref", &branch, &second]);
    assert_eq!(git_fixture(&root, &["rev-parse", "HEAD"]), second);
    assert!(root.join(".git").join(&branch).exists(), "the commit created the loose ref");
    assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), index_before, "and touched nothing else the script watches");
    assert_ne!(first, second);
    assert_eq!(build_and_ask(&root, &target, None), (second, "false".into()), "the rebuild names the new HEAD, not the one before it");
    let _ = fs::remove_dir_all(base);
}

/// A source archive unpacked inside an unrelated repository is not a
/// checkout of butterfly: the build records `unknown`, or the commit the
/// packager supplied -- never the enclosing project's HEAD. The test's own
/// checkout detection holds to the same rule (#349 review).
#[test]
fn an_archive_inside_another_repository_records_unknown_or_the_supplied_commit() {
    if !tools_present() {
        return;
    }
    let base = scratch("archive");
    let (parent, target) = (base.join("project"), base.join("target"));
    fs::create_dir_all(&parent).unwrap();
    fs::write(parent.join("README"), "someone else's project\n").unwrap();
    git_fixture(&parent, &["init", "-q"]);
    git_fixture(&parent, &["add", "-A"]);
    git_fixture(&parent, &["commit", "-q", "-m", "the enclosing project"]);
    let root = parent.join("vendor/butterfly");
    fs::create_dir_all(&root).unwrap();
    let krate = workspace(&root);

    // git alone would answer with the enclosing project's commit
    let theirs = git_fixture(&parent, &["rev-parse", "HEAD"]);
    assert_eq!(git_in(&krate, &["rev-parse", "HEAD"]).as_deref(), Some(theirs.as_str()), "the trap is real");
    assert_eq!(head_of_butterfly_checkout(&krate), None, "but it is not butterfly's checkout");

    assert_eq!(build_and_ask(&root, &target, None), ("unknown".into(), "unknown".into()), "no override: unknown");
    let supplied = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(build_and_ask(&root, &target, Some(supplied)), (supplied.into(), "unknown".into()), "a full id from the packager is taken");
    assert_eq!(build_and_ask(&root, &target, Some("v1.2")), ("unknown".into(), "unknown".into()), "anything else is not");
    let _ = fs::remove_dir_all(base);
}
