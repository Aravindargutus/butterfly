// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Change impact against a synthetic git repository: per-function commit
//! attribution that survives edits above the function and a rename, ticket
//! extraction from subject and body, `--since`, cache reuse, incremental
//! re-walks, re-embedding without git, and the uncommitted-edit caveat.
//! Skipped when git is not installed.

mod common;

use butterfly_ingest::change::{self, Options};
use butterfly_model::{Evidence, Function};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One file's fixture: its path, its text, and the functions in it as
/// `(name, first line, last line)`.
type FnSpec<'a> = (&'a str, u32, u32);
type FileSpec<'a> = (&'a str, &'a [FnSpec<'a>]);

fn have_git() -> bool {
    Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "-m", msg]);
}

fn tmp_repo() -> PathBuf {
    let d = butterfly_testsupport::scratch_dir("change");
    let d = d.canonicalize().unwrap();
    git(&d, &["init", "-q"]);
    d
}

fn sha_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Evidence over real files in the repo: `(relative path, [(name, start, end)])`.
fn ev_for(repo: &Path, specs: &[FileSpec]) -> Evidence {
    let mut files = Vec::new();
    let mut fns: Vec<Function> = Vec::new();
    for (fi, (rel, fl)) in specs.iter().enumerate() {
        let abs = repo.join(rel);
        let text = fs::read_to_string(&abs).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let counts: Vec<Option<u64>> = lines.iter().map(|_| Some(1)).collect();
        let mut f = common::file(&lines, &counts);
        f.id = fi;
        f.path = abs.display().to_string();
        f.display = rel.to_string();
        f.sha256 = sha_hex(text.as_bytes());
        for (name, ls, le) in fl.iter() {
            let id = fns.len();
            let mut fun = common::function(id, name, *ls, *le);
            fun.file_id = fi;
            f.function_ids.push(id);
            fns.push(fun);
        }
        files.push(f);
    }
    common::evidence(files, fns)
}

fn subjects(ev: &Evidence, fid: usize) -> Vec<String> {
    let c = ev.changes.as_ref().unwrap();
    ev.functions[fid].commits.iter().map(|&i| c.commits[i].subject.clone()).collect()
}

fn opts(since: Option<&str>) -> Options {
    Options { since: since.map(str::to_string), repo: None, patterns: vec![r"\b([A-Z]{2,10}-\d+)\b".into()], exclude: vec![], jobs: 2, baseline: None }
}

#[test]
fn walks_history_per_function_and_extracts_tickets() {
    if !have_git() {
        butterfly_testsupport::fixture_missing("git not installed");
        return;
    }
    let repo = tmp_repo();
    let v1 = "int one(int x) {\n    return x + 1;\n}\n\nint two(int x) {\n    return x + 2;\n}\n";
    fs::write(repo.join("lib.c"), v1).unwrap();
    commit(&repo, "ABC-1: add lib");
    let v2 = v1.replace("x + 2", "x + 20");
    fs::write(repo.join("lib.c"), &v2).unwrap();
    commit(&repo, "ABC-2: change two\n\nRefs ABC-9 for the rationale.");
    let v3 = v2.replace("x + 1;", "x + 10;");
    fs::write(repo.join("lib.c"), &v3).unwrap();
    commit(&repo, "no ticket at all");
    let mid = git(&repo, &["rev-parse", "HEAD"]);
    fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["mv", "lib.c", "src/lib.c"]);
    commit(&repo, "ABC-3: move into src");
    let lib: (&str, &[(&str, u32, u32)]) = ("src/lib.c", &[("one", 1, 3), ("two", 5, 7)]);

    // ---- full history, through the rename ---------------------------------
    let mut ev = ev_for(&repo, &[lib]);
    let (cache, out) = change::analyse(&mut ev, &opts(None), None).unwrap();
    assert_eq!((out.walked, out.reused, out.skipped), (2, 0, 0));
    ev.validate().unwrap();
    let one = subjects(&ev, 0);
    let two = subjects(&ev, 1);
    assert!(one.iter().any(|s| s.starts_with("ABC-1")) && one.iter().any(|s| s == "no ticket at all"), "one: {one:?}");
    assert!(!one.iter().any(|s| s.starts_with("ABC-2")), "one must not carry two's commit: {one:?}");
    assert!(two.iter().any(|s| s.starts_with("ABC-1")) && two.iter().any(|s| s.starts_with("ABC-2")), "two: {two:?}");
    assert!(!two.iter().any(|s| s == "no ticket at all"), "two must not carry one's commit: {two:?}");
    let c = ev.changes.as_ref().unwrap();
    let ids: Vec<&str> = c.tickets.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"ABC-1") && ids.contains(&"ABC-2") && ids.contains(&"ABC-9"), "tickets from subject and body: {ids:?}");
    let t9 = c.tickets.iter().find(|t| t.id == "ABC-9").unwrap();
    assert_eq!(t9.functions, vec![1], "a body-only ticket traces to the function its commit touched");
    let unticketed: Vec<&str> = c.unticketed.iter().map(|&i| c.commits[i].subject.as_str()).collect();
    assert_eq!(unticketed, vec!["no ticket at all"]);
    assert_eq!(c.files[0].status, "matches-head");
    assert_eq!(c.files[0].repo_path, "src/lib.c");

    // ---- --since narrows the range ------------------------------------------
    let mut ev2 = ev_for(&repo, &[lib]);
    change::analyse(&mut ev2, &opts(Some(&mid)), None).unwrap();
    let two2 = subjects(&ev2, 1);
    assert!(two2.iter().all(|s| !s.starts_with("ABC-1") && !s.starts_with("ABC-2")), "since {mid}: {two2:?}");
    assert!(ev2.changes.as_ref().unwrap().since_sha.as_deref() == Some(mid.as_str()));

    // ---- cache: identical inputs -> nothing walked, same attribution ---------
    let mut ev3 = ev_for(&repo, &[lib]);
    let (_, out3) = change::analyse(&mut ev3, &opts(None), Some(&cache)).unwrap();
    assert_eq!((out3.walked, out3.reused), (0, 2));
    assert_eq!(subjects(&ev3, 0), one);
    assert_eq!(subjects(&ev3, 1), two);

    // ---- incremental: a new file in a new commit -> only it is walked --------
    fs::write(repo.join("src/other.c"), "int three(int x) {\n    return x + 3;\n}\n").unwrap();
    commit(&repo, "ABC-4: other");
    let both: [FileSpec; 2] = [lib, ("src/other.c", &[("three", 1, 3)])];
    let mut ev4 = ev_for(&repo, &both);
    let (cache4, out4) = change::analyse(&mut ev4, &opts(None), Some(&cache)).unwrap();
    assert_eq!((out4.walked, out4.reused), (1, 2), "only the new file's function is walked");
    assert_eq!(subjects(&ev4, 2), vec!["ABC-4: other"]);
    assert_eq!(subjects(&ev4, 0), one, "reused attribution is unchanged");

    // ---- embed: same sources -> attached without git; changed hash -> refused
    let mut ev5 = ev_for(&repo, &both);
    change::embed(&mut ev5, &cache4, None).unwrap();
    ev5.validate().unwrap();
    assert_eq!(subjects(&ev5, 2), vec!["ABC-4: other"]);
    let mut ev6 = ev_for(&repo, &both);
    ev6.files[0].sha256 = "0".repeat(64);
    let err = change::embed(&mut ev6, &cache4, None).unwrap_err();
    assert!(err.contains("rerun"), "{err}");
    assert!(ev6.changes.is_none() && ev6.functions.iter().all(|f| f.commits.is_empty()));

    // ---- cache round-trips through disk ---------------------------------------
    let cp = repo.join("butterfly-changes.json");
    change::save(&cache4, &cp).unwrap();
    assert_eq!(change::load(&cp).unwrap().head, cache4.head);

    // ---- uncommitted edit: status + caveat, still walked -----------------------
    fs::write(repo.join("src/lib.c"), v3.replace("x + 10;", "x + 11;")).unwrap();
    let mut ev7 = ev_for(&repo, &[lib]);
    change::analyse(&mut ev7, &opts(None), None).unwrap();
    let c7 = ev7.changes.as_ref().unwrap();
    assert_eq!(c7.files[0].status, "matches-worktree");
    assert!(c7.caveats.iter().any(|x| x.contains("uncommitted")), "{:?}", c7.caveats);
    assert!(!ev7.functions[0].commits.is_empty());

    let _ = fs::remove_dir_all(&repo);
}
