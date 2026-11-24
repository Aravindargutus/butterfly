// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Which commit of butterfly this build is (#117).
//!
//! `provenance.toolVersion` names a release, not a build: every commit
//! between two tags carries the same version string, so evidence written by
//! two materially different builds reads the same (DO-330 §7.2.1, §7.2.2.g;
//! `docs/qualification/DO-330-assessment.md`, T-8 objective 2). This script
//! captures, at build time, the commit the sources are at and whether the
//! work tree had uncommitted changes, and hands both to the crate as
//! `BUTTERFLY_TOOL_COMMIT` and `BUTTERFLY_TOOL_DIRTY`, which `ingest` writes
//! into every evidence file and `butterfly --version` prints.
//!
//! Three answers, never a guess:
//!
//! 1. **A checkout of butterfly.** `git` is on `PATH`, the directory this
//!    crate sits in belongs to a work tree, and that work tree's top level is
//!    the workspace root two levels up. The last condition matters: a source
//!    tarball unpacked inside some *other* repository -- the operator's own
//!    project, say -- would otherwise record that project's commit as
//!    butterfly's. The commit is `git rev-parse HEAD`; the tree is dirty when
//!    `git status --porcelain` lists anything, untracked files included,
//!    because an untracked source file compiles like a tracked one.
//! 2. **No checkout, but `BUTTERFLY_BUILD_COMMIT` set** -- a build from an
//!    exported archive whose packager knows which commit it was cut from.
//!    Taken only when it is a full commit id (40 or 64 hex digits); the tree
//!    state is then unknown, since there is nothing to compare against.
//! 3. **Otherwise `unknown`.** Said as such, with a build warning, so a
//!    binary that cannot be traced to a source baseline says so in every
//!    evidence file it writes rather than carrying a plausible-looking id.
//!
//! The dirty flag is as current as the last run of this script, so the
//! script is re-run whenever the answer could change: when `HEAD`, the
//! branch's ref file (or, while that ref is packed, the directory its loose
//! file will be created in), `packed-refs` or the index move -- a commit,
//! checkout, add or `git status` -- when the fallback variable changes, and when
//! anything under `crates/` or `ui/`, or `Cargo.toml` or `Cargo.lock`,
//! changes. The last is what keeps an edit honest: without it, cargo would
//! recompile the edited crate and leave this script's `clean tree` standing,
//! which is the one answer that must never be stale. The price is that an
//! edit anywhere in the sources re-runs one `git status` and rebuilds this
//! crate. A file outside those paths (a README, a script) is not watched;
//! it does not go into the binary. The release workflow builds from a fresh
//! checkout and checks the binary's own answer against the tag
//! (`release.yml`).
//!
//! Plain `std::process::Command`, not the crate's bounded helper policy
//! (`helper.rs`): that policy exists for helpers reading untrusted input at
//! run time, and a build script cannot use the crate it is building. The
//! only input here is the developer's own checkout. `GIT_DIR`,
//! `GIT_WORK_TREE` and `GIT_INDEX_FILE` are still cleared, so a stray
//! variable in the shell cannot point git at another repository.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=BUTTERFLY_BUILD_COMMIT");
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));

    let (commit, dirty) = match from_git(&manifest_dir) {
        Some(c) => c,
        None => match std::env::var("BUTTERFLY_BUILD_COMMIT") {
            Ok(v) if is_commit_id(v.trim()) => (v.trim().to_string(), None),
            Ok(v) => {
                println!("cargo:warning=BUTTERFLY_BUILD_COMMIT={v:?} is not a full commit id (40 or 64 hex digits); the tool commit is recorded as unknown");
                ("unknown".to_string(), None)
            }
            Err(_) => {
                println!("cargo:warning=not building from a git checkout of butterfly and BUTTERFLY_BUILD_COMMIT is not set; the tool commit is recorded as unknown");
                ("unknown".to_string(), None)
            }
        },
    };
    println!("cargo:rustc-env=BUTTERFLY_TOOL_COMMIT={commit}");
    println!(
        "cargo:rustc-env=BUTTERFLY_TOOL_DIRTY={}",
        match dirty {
            Some(true) => "true",
            Some(false) => "false",
            None => "unknown",
        }
    );
}

/// 40 or 64 lowercase hex digits: a SHA-1 or SHA-256 object id and nothing
/// else -- not a short id, not a ref name, not a description.
fn is_commit_id(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// `Some((commit, Some(dirty)))` from the checkout this crate is part of;
/// `None` when there is no git, no work tree, or the work tree is not
/// butterfly's. A failure inside a real checkout (no commits yet, a
/// repository git refuses) is reported as a warning and also yields `None`.
fn from_git(manifest_dir: &Path) -> Option<(String, Option<bool>)> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git").args(args).current_dir(manifest_dir).env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE").output().ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim_end_matches(['\n', '\r']).to_string())
    };

    // Inside a work tree at all? `false` or a failure says no: a repository
    // that is not there, or that git refuses (dubious ownership), reads the
    // same way -- there is no commit to record either way.
    if git(&["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        return None;
    }
    // ... and it is butterfly's: the work tree's top level is the workspace
    // root, two levels above crates/ingest. Compared canonicalised, so a
    // symlinked checkout agrees with itself.
    let top = git(&["rev-parse", "--show-toplevel"]).map(PathBuf::from)?;
    let workspace = manifest_dir.join("../..");
    match (top.canonicalize(), workspace.canonicalize()) {
        (Ok(a), Ok(b)) if a == b => {}
        _ => {
            println!(
                "cargo:warning={} is inside the git work tree {} but is not its checkout of butterfly; the tool commit is recorded as unknown",
                manifest_dir.display(),
                top.display()
            );
            return None;
        }
    }

    // Re-run when the commit or the index moves. Only paths that exist are
    // named: cargo treats a named file that does not exist as always changed.
    let git_path = |w: &str| git(&["rev-parse", "--git-path", w]).map(|p| manifest_dir.join(p));
    for w in ["HEAD", "packed-refs", "index"] {
        if let Some(p) = git_path(w).filter(|p| p.exists()) {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
    // The branch's ref: its loose file when there is one. A branch whose ref
    // is packed has none, and the next commit on it creates one without
    // touching HEAD, packed-refs or the index -- so the directory that file
    // will appear in is watched instead (cargo scans a watched directory for
    // any new or modified file). Watching only what existed left that commit
    // unseen and the previous one recorded (#349 review).
    if let (Some(r), Some(refs)) = (git(&["symbolic-ref", "-q", "HEAD"]), git_path("refs")) {
        if let Some(p) = git_path(&r) {
            match p.ancestors().find(|a| a.exists()) {
                Some(a) if a.starts_with(&refs) => println!("cargo:rerun-if-changed={}", a.display()),
                _ => println!("cargo:rerun-if-changed={}", refs.display()),
            }
        }
    }

    // ... and when the sources that go into the binary change, so an edit
    // after a clean build cannot leave `clean tree` standing. A directory
    // entry makes cargo scan it for any modified file.
    for src in ["crates", "ui", "Cargo.toml", "Cargo.lock"] {
        let p = workspace.join(src);
        if p.exists() {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }

    let Some(commit) = git(&["rev-parse", "HEAD"]).filter(|c| is_commit_id(c)) else {
        println!("cargo:warning=git could not name HEAD in {} (no commits yet, or a repository git refuses); the tool commit is recorded as unknown", manifest_dir.display());
        return None;
    };
    // `--no-optional-locks`: a status probe must not write the index of the
    // tree it is asked about. The tree state is unknown, not clean, when the
    // probe fails: a commit without a verdict on the tree is still a commit.
    let dirty = git(&["--no-optional-locks", "status", "--porcelain", "--untracked-files=normal"]).map(|s| !s.trim().is_empty());
    if dirty.is_none() {
        println!("cargo:warning=git status failed in {}; whether the tool's work tree was modified is recorded as unknown", manifest_dir.display());
    }
    Some((commit, dirty))
}
