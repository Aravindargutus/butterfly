// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Change impact: normal, abnormal and robustness cases against synthetic
//! git repositories. Every test builds its own repository, so they run in
//! parallel. Skipped when git is not installed.
//!
//! These encode the *intended* behaviour. A failing test here is a finding
//! about the implementation, not about the test.

mod common;

use butterfly_ingest::change::{self, Cache, Options};
use butterfly_model::{Evidence, Function};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One file's fixture: its path, its text, and the functions in it as
/// `(name, first line, last line)`.
type FnSpec<'a> = (&'a str, u32, u32);
type FileSpec<'a> = (&'a str, &'a [FnSpec<'a>]);

// ---------------------------------------------------------------- helpers

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

struct Repo {
    dir: PathBuf,
    n: u32,
}

impl Repo {
    fn new() -> Repo {
        let d = butterfly_testsupport::scratch_dir("cc");
        let dir = d.canonicalize().unwrap();
        let r = Repo { dir, n: 0 };
        r.git(&["init", "-q", "-b", "main"]);
        r
    }
    fn git(&self, args: &[&str]) -> String {
        self.git_in(&self.dir, args)
    }
    fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().expect("spawn git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
    fn write(&self, rel: &str, content: &str) {
        let p = self.dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
    /// Commit everything with a distinct, increasing author date so ordering
    /// is deterministic even when commits land within the same second.
    fn commit(&mut self, msg: &str) -> String {
        self.n += 1;
        let date = format!("2026-01-01T00:{:02}:{:02}+00:00", self.n / 60, self.n % 60);
        self.commit_dated(msg, &date)
    }
    fn commit_dated(&self, msg: &str, date: &str) -> String {
        self.git(&["add", "-A"]);
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "--allow-empty-message", "-m", msg])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .unwrap();
        assert!(out.status.success(), "commit: {}", String::from_utf8_lossy(&out.stderr));
        self.git(&["rev-parse", "HEAD"])
    }
    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn sha_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// `(absolute path, text the evidence was built from, [(name, start, end)])`.
fn ev_from(specs: &[(&Path, &str, &[FnSpec])]) -> Evidence {
    let mut files = Vec::new();
    let mut fns: Vec<Function> = Vec::new();
    for (fi, (abs, text, fl)) in specs.iter().enumerate() {
        let lines: Vec<&str> = text.lines().collect();
        let counts: Vec<Option<u64>> = lines.iter().map(|_| Some(1)).collect();
        let mut f = common::file(&lines, &counts);
        f.id = fi;
        f.path = abs.display().to_string();
        f.display = abs.file_name().unwrap().to_string_lossy().to_string();
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

/// Evidence over files as they are on disk right now.
fn ev_disk(repo: &Repo, specs: &[FileSpec]) -> Evidence {
    let owned: Vec<(PathBuf, String)> = specs
        .iter()
        .map(|(rel, _)| {
            let p = repo.dir.join(rel);
            let t = fs::read_to_string(&p).unwrap();
            (p, t)
        })
        .collect();
    let v: Vec<(&Path, &str, &[FnSpec])> = owned.iter().zip(specs.iter()).map(|((p, t), (_, fl))| (p.as_path(), t.as_str(), *fl)).collect();
    ev_from(&v)
}

fn opts() -> Options {
    Options { since: None, repo: None, patterns: vec![r"\b([A-Z]{2,10}-\d+)\b".into()], exclude: vec![], jobs: 2, baseline: None }
}

fn subjects(ev: &Evidence, fid: usize) -> Vec<String> {
    let c = ev.changes.as_ref().expect("changes");
    ev.functions[fid].commits.iter().map(|&i| c.commits[i].subject.clone()).collect()
}

fn ticket_ids(ev: &Evidence) -> Vec<String> {
    ev.changes.as_ref().unwrap().tickets.iter().map(|t| t.id.clone()).collect()
}

const TWO_FNS: &str = "int one(int x) {\n    return x + 1;\n}\n\nint two(int x) {\n    return x + 2;\n}\n";
const ONE: (&str, u32, u32) = ("one", 1, 3);
const TWO: (&str, u32, u32) = ("two", 5, 7);

// ===================================================================== normal

#[test]
fn n01_single_function_single_commit() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int f(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: add f");
    let mut ev = ev_disk(&r, &[("a.c", &[("f", 1, 3)])]);
    let (_, out) = change::analyse(&mut ev, &opts(), None).unwrap();
    assert_eq!(out.walked, 1);
    assert_eq!(subjects(&ev, 0), vec!["ABC-1: add f"]);
    assert_eq!(ticket_ids(&ev), vec!["ABC-1"]);
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.tickets[0].functions, vec![0]);
    assert!(c.unticketed.is_empty());
    assert_eq!(c.head, r.head());
}

#[test]
fn n02_several_tickets_per_commit_and_one_ticket_across_commits() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", TWO_FNS);
    r.commit("ABC-1 ABC-2: add both");
    r.write("a.c", &TWO_FNS.replace("x + 1", "x + 10"));
    r.commit("ABC-1: touch one again");
    let mut ev = ev_disk(&r, &[("a.c", &[ONE, TWO])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(ticket_ids(&ev), vec!["ABC-1", "ABC-2"]);
    let t1 = &c.tickets[0];
    assert_eq!(t1.commits.len(), 2, "ABC-1 appears in two commits");
    assert_eq!(t1.functions, vec![0, 1], "union of functions, deduped, sorted");
    let first = c.commits.iter().find(|x| x.subject.starts_with("ABC-1 ABC-2")).unwrap();
    assert_eq!(first.tickets.len(), 2);
}

#[test]
fn n03_one_commit_touching_two_files() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.write("b.c", "int b(void) {\n    return 2;\n}\n");
    r.commit("XY-7: both files");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)]), ("b.c", &[("b", 1, 3)])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.commits.len(), 1);
    assert_eq!(c.commits[0].functions, vec![0, 1]);
    assert_eq!(c.tickets[0].functions, vec![0, 1]);
}

#[test]
fn n04_function_added_later_gets_only_its_own_history() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int one(int x) {\n    return x + 1;\n}\n");
    r.commit("ABC-1: one");
    r.write("a.c", TWO_FNS);
    r.commit("ABC-2: add two");
    let mut ev = ev_disk(&r, &[("a.c", &[ONE, TWO])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    assert_eq!(subjects(&ev, 0), vec!["ABC-1: one"]);
    assert_eq!(subjects(&ev, 1), vec!["ABC-2: add two"]);
}

#[test]
fn n05_exclude_glob_skips_the_file() {
    need_git!();
    let mut r = Repo::new();
    r.write("src/a.c", "int a(void) {\n    return 1;\n}\n");
    r.write("vendor/v.c", "int v(void) {\n    return 2;\n}\n");
    r.commit("ABC-1: all");
    let mut ev = ev_disk(&r, &[("src/a.c", &[("a", 1, 3)]), ("vendor/v.c", &[("v", 1, 3)])]);
    let o = Options { exclude: vec!["vendor/**".into()], ..opts() };
    let (_, out) = change::analyse(&mut ev, &o, None).unwrap();
    assert_eq!((out.walked, out.skipped), (1, 1));
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.files[1].status, "excluded");
    assert!(ev.functions[1].commits.is_empty());
    assert!(c.caveats.iter().any(|x| x.contains("excluded")));
}

#[test]
fn n06_pattern_without_capture_group_uses_whole_match() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("fix #123 and #45");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let o = Options { patterns: vec![r"#\d+".into()], ..opts() };
    change::analyse(&mut ev, &o, None).unwrap();
    assert_eq!(ticket_ids(&ev), vec!["#123", "#45"]);
}

#[test]
fn n07_commits_newest_first_and_function_indices_sorted_unique() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int one(int x) {\n    return x + 1;\n}\n");
    r.commit("ABC-1: first");
    r.write("a.c", "int one(int x) {\n    return x + 2;\n}\n");
    r.commit("ABC-2: second");
    r.write("a.c", "int one(int x) {\n    return x + 3;\n}\n");
    r.commit("ABC-3: third");
    let mut ev = ev_disk(&r, &[("a.c", &[("one", 1, 3)])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    let subs: Vec<&str> = c.commits.iter().map(|x| x.subject.as_str()).collect();
    assert_eq!(subs, vec!["ABC-3: third", "ABC-2: second", "ABC-1: first"]);
    let f = &ev.functions[0].commits;
    let mut sorted = f.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(*f, sorted);
    assert_eq!(f.len(), 3);
}

#[test]
fn n08_since_accepts_a_tag() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", TWO_FNS);
    r.commit("ABC-1: base");
    r.git(&["tag", "v1.0"]);
    r.write("a.c", &TWO_FNS.replace("x + 2", "x + 20"));
    r.commit("ABC-2: after tag");
    let mut ev = ev_disk(&r, &[("a.c", &[ONE, TWO])]);
    let o = Options { since: Some("v1.0".into()), ..opts() };
    change::analyse(&mut ev, &o, None).unwrap();
    assert!(subjects(&ev, 0).is_empty(), "one untouched since v1.0");
    assert_eq!(subjects(&ev, 1), vec!["ABC-2: after tag"]);
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.since.as_deref(), Some("v1.0"));
    assert_eq!(c.since_sha.as_deref().map(str::len), Some(40));
}

#[test]
fn n09_untracked_and_outside_repo_files_are_skipped_with_status() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: a");
    r.write("new.c", "int n(void) {\n    return 3;\n}\n"); // never added
    let outside_dir = butterfly_testsupport::scratch_dir("outside");
    let outside = outside_dir.canonicalize().unwrap().join("o.c");
    fs::write(&outside, "int o(void) {\n    return 4;\n}\n").unwrap();
    let a_text = fs::read_to_string(r.dir.join("a.c")).unwrap();
    let new_text = fs::read_to_string(r.dir.join("new.c")).unwrap();
    let o_text = fs::read_to_string(&outside).unwrap();
    let mut ev = ev_from(&[(&r.dir.join("a.c"), &a_text, &[("a", 1, 3)]), (&r.dir.join("new.c"), &new_text, &[("n", 1, 3)]), (&outside, &o_text, &[("o", 1, 3)])]);
    let (_, out) = change::analyse(&mut ev, &opts(), None).unwrap();
    assert_eq!((out.walked, out.skipped), (1, 2));
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.files[0].status, "matches-head");
    assert_eq!(c.files[1].status, "untracked");
    assert_eq!(c.files[2].status, "outside-repo");
    assert!(ev.functions[1].commits.is_empty() && ev.functions[2].commits.is_empty());
    let _ = fs::remove_dir_all(outside_dir);
}

#[test]
fn n10_same_function_name_in_two_files_is_kept_apart() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "static int helper(void) {\n    return 1;\n}\n");
    r.write("b.c", "static int helper(void) {\n    return 2;\n}\n");
    r.commit("ABC-1: both");
    r.write("b.c", "static int helper(void) {\n    return 22;\n}\n");
    r.commit("ABC-2: only b");
    let mut ev = ev_disk(&r, &[("a.c", &[("a.c:helper", 1, 3)]), ("b.c", &[("b.c:helper", 1, 3)])]);
    let (cache, _) = change::analyse(&mut ev, &opts(), None).unwrap();
    assert_eq!(subjects(&ev, 0), vec!["ABC-1: both"]);
    assert_eq!(subjects(&ev, 1), vec!["ABC-2: only b", "ABC-1: both"]);
    // and the cache keeps them apart too
    let mut ev2 = ev_disk(&r, &[("a.c", &[("a.c:helper", 1, 3)]), ("b.c", &[("b.c:helper", 1, 3)])]);
    change::embed(&mut ev2, &cache, None).unwrap();
    assert_eq!(subjects(&ev2, 0), vec!["ABC-1: both"]);
    assert_eq!(subjects(&ev2, 1).len(), 2);
}

#[test]
fn n11_header_with_no_functions_is_harmless() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.h", "#define X 1\n");
    r.write("a.c", "int a(void) {\n    return X;\n}\n");
    r.commit("ABC-1: all");
    let mut ev = ev_disk(&r, &[("a.h", &[]), ("a.c", &[("a", 1, 3)])]);
    let (cache, out) = change::analyse(&mut ev, &opts(), None).unwrap();
    assert_eq!(out.walked, 1);
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.files[0].status, "matches-head");
    assert_eq!(c.files[0].functions_walked, 0);
    let mut ev2 = ev_disk(&r, &[("a.h", &[]), ("a.c", &[("a", 1, 3)])]);
    change::embed(&mut ev2, &cache, None).unwrap();
}

#[test]
fn n12_branch_commit_reaches_the_function_after_a_merge() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", TWO_FNS);
    r.commit("ABC-1: base");
    r.git(&["checkout", "-q", "-b", "feat"]);
    r.write("a.c", &TWO_FNS.replace("x + 2", "x + 20"));
    r.commit("FEAT-2: on branch");
    r.git(&["checkout", "-q", "main"]);
    r.git(&["-c", "user.name=t", "-c", "user.email=t@t", "merge", "-q", "--no-ff", "-m", "merge feat", "feat"]);
    let mut ev = ev_disk(&r, &[("a.c", &[ONE, TWO])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    assert!(ticket_ids(&ev).contains(&"FEAT-2".to_string()));
    assert!(subjects(&ev, 1).iter().any(|s| s == "FEAT-2: on branch"));
    assert!(!subjects(&ev, 0).iter().any(|s| s == "FEAT-2: on branch"));
}

#[test]
fn n13_relative_paths_work_with_an_explicit_repo() {
    need_git!();
    let mut r = Repo::new();
    r.write("src/a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: a");
    let text = fs::read_to_string(r.dir.join("src/a.c")).unwrap();
    let mut ev = ev_from(&[(Path::new("src/a.c"), &text, &[("a", 1, 3)])]);
    let o = Options { repo: Some(r.dir.clone()), ..opts() };
    change::analyse(&mut ev, &o, None).unwrap();
    assert_eq!(subjects(&ev, 0), vec!["ABC-1: a"]);
    assert_eq!(ev.changes.as_ref().unwrap().files[0].repo_path, "src/a.c");
}

#[test]
fn n14_repo_given_as_a_subdirectory() {
    need_git!();
    let mut r = Repo::new();
    r.write("src/deep/a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: a");
    let mut ev = ev_disk(&r, &[("src/deep/a.c", &[("a", 1, 3)])]);
    let o = Options { repo: Some(r.dir.join("src/deep")), ..opts() };
    change::analyse(&mut ev, &o, None).unwrap();
    assert_eq!(subjects(&ev, 0), vec!["ABC-1: a"]);
    assert_eq!(ev.changes.as_ref().unwrap().repo, r.dir.display().to_string());
}

// =================================================================== abnormal

#[test]
fn a15_not_a_git_repository() {
    need_git!();
    let d = butterfly_testsupport::scratch_dir("norepo");
    let p = d.canonicalize().unwrap().join("a.c");
    fs::write(&p, "int a(void) {\n    return 1;\n}\n").unwrap();
    let mut ev = ev_from(&[(&p, "int a(void) {\n    return 1;\n}\n", &[("a", 1, 3)])]);
    let err = change::analyse(&mut ev, &opts(), None).unwrap_err().to_string();
    assert!(err.contains("git repository"), "{err}");
    assert!(ev.changes.is_none());
    let _ = fs::remove_dir_all(d);
}

#[test]
fn a16_empty_repository_without_commits() {
    need_git!();
    let r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let err = change::analyse(&mut ev, &opts(), None).unwrap_err().to_string();
    assert!(err.to_lowercase().contains("head") || err.to_lowercase().contains("empty"), "{err}");
}

#[test]
fn a17_since_unknown_revision_is_an_error_naming_it() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: a");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let o = Options { since: Some("no-such-tag".into()), ..opts() };
    let err = change::analyse(&mut ev, &o, None).unwrap_err().to_string();
    assert!(err.contains("no-such-tag"), "{err}");
}

#[test]
fn a18_since_equal_to_head_yields_no_commits_and_no_error() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: a");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let o = Options { since: Some(r.head()), ..opts() };
    let (_, out) = change::analyse(&mut ev, &o, None).unwrap();
    assert_eq!(out.walked, 1);
    let c = ev.changes.as_ref().unwrap();
    assert!(c.commits.is_empty() && c.tickets.is_empty() && ev.functions[0].commits.is_empty());
}

#[test]
fn a19_invalid_ticket_regex_is_reported_with_the_pattern() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: a");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let o = Options { patterns: vec!["([".into()], ..opts() };
    let err = change::analyse(&mut ev, &o, None).unwrap_err().to_string();
    assert!(err.contains("([") && err.to_lowercase().contains("pattern"), "{err}");
}

#[test]
fn a20_range_beyond_file_at_head_is_a_caveat_and_others_still_attributed() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", TWO_FNS);
    r.commit("ABC-1: both");
    // evidence built from a longer, uncommitted-elsewhere version of the file
    let longer = format!("{TWO_FNS}\nint three(int x) {{\n    return x + 3;\n}}\n");
    let mut ev = ev_from(&[(&r.dir.join("a.c"), &longer, &[ONE, TWO, ("three", 9, 11)])]);
    let (_, out) = change::analyse(&mut ev, &opts(), None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.files[0].status, "mismatch");
    assert_eq!(subjects(&ev, 0), vec!["ABC-1: both"]);
    assert!(ev.functions[2].commits.is_empty());
    assert_eq!(out.skipped, 1);
    assert!(c.caveats.iter().any(|x| x.contains("could not be walked") && x.contains("three")), "{:?}", c.caveats);
    assert!(c.caveats.iter().any(|x| x.contains("different checkout")));
}

#[test]
fn a21_inverted_or_zero_line_range_does_not_panic() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", TWO_FNS);
    r.commit("ABC-1: both");
    let text = fs::read_to_string(r.dir.join("a.c")).unwrap();
    let mut ev = ev_from(&[(&r.dir.join("a.c"), &text, &[("inverted", 3, 1), ("zero", 0, 3)])]);
    let res = change::analyse(&mut ev, &opts(), None);
    assert!(res.is_ok(), "{:?}", res.err());
    let c = ev.changes.as_ref().unwrap();
    // each is either attributed or explicitly caveated -- never silently empty
    for (i, f) in ev.functions.iter().enumerate() {
        assert!(!f.commits.is_empty() || c.caveats.iter().any(|x| x.contains(&f.name)), "function {i} `{}` silently got nothing", f.name);
    }
    // The analyser copes, and the document is still refused at the door: a
    // range like 3..=1 is not a range, and a consumer that walks it line by
    // line -- the requirements matrix does -- must never be handed one
    // (review of #42). Robustness here and refusal there are both wanted.
    let err = ev.validate().unwrap_err();
    assert!(err.contains("source range"), "{err}");
}

#[test]
fn a22_cache_from_another_repo_or_another_range_is_ignored() {
    need_git!();
    let mut a = Repo::new();
    a.write("a.c", "int a(void) {\n    return 1;\n}\n");
    a.commit("ABC-1: a");
    let mut b = Repo::new();
    b.write("a.c", "int a(void) {\n    return 1;\n}\n");
    b.commit("XYZ-9: a in b");
    let mut ev_a = ev_disk(&a, &[("a.c", &[("a", 1, 3)])]);
    let (cache_a, _) = change::analyse(&mut ev_a, &opts(), None).unwrap();
    let mut ev_b = ev_disk(&b, &[("a.c", &[("a", 1, 3)])]);
    let (_, out) = change::analyse(&mut ev_b, &opts(), Some(&cache_a)).unwrap();
    assert_eq!(
        (out.walked, out.reused),
        (1, 0),
        "a cache from another repository must not be reused; files={:?} caveats={:?}",
        ev_b.changes.as_ref().map(|c| c.files.iter().map(|f| (f.repo_path.clone(), f.status.clone())).collect::<Vec<_>>()),
        ev_b.changes.as_ref().map(|c| c.caveats.clone())
    );
    assert_eq!(subjects(&ev_b, 0), vec!["XYZ-9: a in b"]);
    // same repo, different --since: the full history is cached, so nothing is re-walked
    let mut ev_a2 = ev_disk(&a, &[("a.c", &[("a", 1, 3)])]);
    let o = Options { since: Some(a.head()), ..opts() };
    let (_, out2) = change::analyse(&mut ev_a2, &o, Some(&cache_a)).unwrap();
    assert_eq!((out2.walked, out2.reused), (0, 1));
    assert!(ev_a2.functions[0].commits.is_empty(), "and the range filter still applies");
}

#[test]
fn a23_cache_head_that_no_longer_exists_falls_back_to_a_full_walk() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: a");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let (mut cache, _) = change::analyse(&mut ev, &opts(), None).unwrap();
    cache.head = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".into();
    let mut ev2 = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let (_, out) = change::analyse(&mut ev2, &opts(), Some(&cache)).unwrap();
    assert_eq!((out.walked, out.reused), (1, 0));
    assert_eq!(subjects(&ev2, 0), vec!["ABC-1: a"]);
}

#[test]
fn a24_embed_rejects_a_wrong_schema() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: a");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let (mut cache, _) = change::analyse(&mut ev, &opts(), None).unwrap();
    cache.schema = "butterfly-changes/99".into();
    let mut ev2 = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let err = change::embed(&mut ev2, &cache, None).unwrap_err();
    assert!(err.contains("schema"), "{err}");
    assert!(ev2.changes.is_none());
}

#[test]
fn a25_file_deleted_at_head_is_reported_as_such_not_as_untracked() {
    need_git!();
    let mut r = Repo::new();
    let text = "int gone(void) {\n    return 1;\n}\n";
    r.write("gone.c", text);
    r.commit("ABC-1: add gone");
    fs::remove_file(r.dir.join("gone.c")).unwrap();
    r.commit("ABC-2: delete gone");
    let mut ev = ev_from(&[(&r.dir.join("gone.c"), text, &[("gone", 1, 3)])]);
    let (_, out) = change::analyse(&mut ev, &opts(), None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(out.skipped, 1);
    assert_eq!(c.files[0].status, "not-at-head", "a file with history that is gone at HEAD is not 'untracked'");
    assert!(c.caveats.iter().any(|x| x.contains("HEAD")), "{:?}", c.caveats);
}

#[test]
fn a26_shallow_clone_is_flagged_as_truncated_history() {
    need_git!();
    let mut src = Repo::new();
    src.write("a.c", "int a(void) {\n    return 1;\n}\n");
    src.commit("ABC-1: first");
    src.write("a.c", "int a(void) {\n    return 2;\n}\n");
    src.commit("ABC-2: second");
    let dst = butterfly_testsupport::scratch_dir("shallow");
    let out = Command::new("git").args(["clone", "-q", "--depth", "1", &format!("file://{}", src.dir.display()), &dst.display().to_string()]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let dst = dst.canonicalize().unwrap();
    let text = fs::read_to_string(dst.join("a.c")).unwrap();
    let mut ev = ev_from(&[(&dst.join("a.c"), &text, &[("a", 1, 3)])]);
    let o = Options { repo: Some(dst.clone()), ..opts() };
    change::analyse(&mut ev, &o, None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert!(c.caveats.iter().any(|x| x.to_lowercase().contains("shallow")), "truncated history must be flagged: {:?}", c.caveats);
    assert!(!subjects(&ev, 0).iter().any(|s| s.starts_with("ABC-1")), "ABC-1 is not visible in a depth-1 clone");
    let _ = fs::remove_dir_all(dst);
}

#[test]
fn a27_no_files_is_an_error_without_repo_and_empty_with_it() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: a");
    let mut ev = ev_from(&[]);
    let err = change::analyse(&mut ev, &opts(), None).unwrap_err().to_string();
    assert!(err.contains("--repo"), "{err}");
    let mut ev2 = ev_from(&[]);
    let o = Options { repo: Some(r.dir.clone()), ..opts() };
    let (_, out) = change::analyse(&mut ev2, &o, None).unwrap();
    assert_eq!((out.walked, out.reused, out.skipped), (0, 0, 0));
    let c = ev2.changes.as_ref().unwrap();
    assert!(c.commits.is_empty() && c.files.is_empty());
    ev2.validate().unwrap();
}

// ================================================================= robustness

fn canon(ev: &Evidence) -> String {
    let mut e = ev.clone();
    if let Some(c) = e.changes.as_mut() {
        c.created_unix = 0;
    }
    serde_json::to_string(&e.changes).unwrap() + &serde_json::to_string(&e.functions.iter().map(|f| f.commits.clone()).collect::<Vec<_>>()).unwrap()
}

#[test]
fn r28_deterministic_across_job_counts() {
    need_git!();
    let mut r = Repo::new();
    let mut src = String::new();
    for i in 0..12 {
        src.push_str(&format!("int f{i}(int x) {{\n    return x + {i};\n}}\n\n"));
    }
    r.write("a.c", &src);
    r.commit("ABC-1: all");
    for i in [3usize, 7, 11] {
        src = src.replace(&format!("x + {i};"), &format!("x + {};", i * 100));
        r.write("a.c", &src);
        r.commit(&format!("ABC-{}: f{i}", i + 10));
    }
    let fl: Vec<(String, u32, u32)> = (0..12).map(|i| (format!("f{i}"), (i * 4 + 1) as u32, (i * 4 + 3) as u32)).collect();
    let fl_ref: Vec<(&str, u32, u32)> = fl.iter().map(|(n, a, b)| (n.as_str(), *a, *b)).collect();
    let mut ev1 = ev_disk(&r, &[("a.c", &fl_ref)]);
    let mut ev8 = ev_disk(&r, &[("a.c", &fl_ref)]);
    change::analyse(&mut ev1, &Options { jobs: 1, ..opts() }, None).unwrap();
    change::analyse(&mut ev8, &Options { jobs: 8, ..opts() }, None).unwrap();
    assert_eq!(canon(&ev1), canon(&ev8));
    assert_eq!(subjects(&ev1, 3), vec!["ABC-13: f3", "ABC-1: all"]);
    assert_eq!(subjects(&ev1, 0), vec!["ABC-1: all"]);
}

#[test]
fn r29_record_separators_inside_a_message_do_not_corrupt_attribution() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    let sha = r.commit("ABC-1: weird\n\nbody with \x1f unit and \x1e record separators ABC-2");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.commits.len(), 1);
    assert_eq!(c.commits[0].sha, sha);
    assert_eq!(ev.functions[0].commits, vec![0]);
    assert!(ticket_ids(&ev).contains(&"ABC-1".to_string()));
}

#[test]
fn r30_non_utf8_commit_message_does_not_panic() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    let msg = r.dir.join("msg.txt");
    fs::write(&msg, b"ABC-1: caf\xe9 latin-1\n").unwrap();
    r.git(&["add", "a.c"]);
    let out = Command::new("git")
        .arg("-C")
        .arg(&r.dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "-F", msg.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    fs::remove_file(&msg).unwrap();
    r.n += 1;
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    assert_eq!(ev.functions[0].commits.len(), 1);
    assert_eq!(ticket_ids(&ev), vec!["ABC-1"]);
}

#[test]
fn r31_paths_with_spaces_and_unicode() {
    need_git!();
    let mut r = Repo::new();
    r.write("src dir/héllo wörld.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: unicode path");
    let mut ev = ev_disk(&r, &[("src dir/héllo wörld.c", &[("a", 1, 3)])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    assert_eq!(subjects(&ev, 0), vec!["ABC-1: unicode path"]);
    assert_eq!(ev.changes.as_ref().unwrap().files[0].repo_path, "src dir/héllo wörld.c");
}

#[test]
fn r32_path_containing_a_colon_is_attributed_or_caveated_never_silently_empty() {
    need_git!();
    let mut r = Repo::new();
    r.write("src/we:ird.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: colon path");
    let mut ev = ev_disk(&r, &[("src/we:ird.c", &[("a", 1, 3)])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    let attributed = subjects(&ev, 0) == vec!["ABC-1: colon path"];
    let caveated = c.caveats.iter().any(|x| x.contains("could not be walked") && x.contains("we:ird"));
    assert!(attributed || caveated, "{:?} / {:?}", subjects(&ev, 0), c.caveats);
}

#[test]
fn r33_mixed_timezone_offsets_still_sort_chronologically() {
    need_git!();
    let r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    // 10:00+05:00 is 05:00Z -- the OLDER commit, despite the larger clock face
    r.commit_dated("ABC-1: older", "2026-01-01T10:00:00+05:00");
    r.write("a.c", "int a(void) {\n    return 2;\n}\n");
    r.commit_dated("ABC-2: newer", "2026-01-01T06:00:00+00:00");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    let subs: Vec<&str> = c.commits.iter().map(|x| x.subject.as_str()).collect();
    assert_eq!(subs, vec!["ABC-2: newer", "ABC-1: older"], "newest first means by instant, not by the wall-clock string");
}

#[test]
fn r34_empty_commit_message() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.commits.len(), 1);
    assert_eq!(ev.functions[0].commits, vec![0]);
    assert_eq!(c.unticketed, vec![0]);
}

#[test]
fn r35_two_hundred_functions_in_one_file() {
    need_git!();
    let mut r = Repo::new();
    let mut src = String::new();
    for i in 0..200 {
        src.push_str(&format!("int f{i}(int x) {{\n    return x + {i};\n}}\n\n"));
    }
    r.write("big.c", &src);
    r.commit("ABC-1: big");
    let fl: Vec<(String, u32, u32)> = (0..200).map(|i| (format!("f{i}"), (i * 4 + 1) as u32, (i * 4 + 3) as u32)).collect();
    let fl_ref: Vec<(&str, u32, u32)> = fl.iter().map(|(n, a, b)| (n.as_str(), *a, *b)).collect();
    let mut ev = ev_disk(&r, &[("big.c", &fl_ref)]);
    let t = std::time::Instant::now();
    let (_, out) = change::analyse(&mut ev, &Options { jobs: 4, ..opts() }, None).unwrap();
    assert_eq!(out.walked, 200);
    assert!(ev.functions.iter().all(|f| f.commits.len() == 1));
    assert!(t.elapsed().as_secs() < 60, "took {:?}", t.elapsed());
    ev.validate().unwrap();
}

#[test]
fn r36_crlf_file_hashes_match_head() {
    need_git!();
    let mut r = Repo::new();
    let text = "int a(void) {\r\n    return 1;\r\n}\r\n";
    r.write("a.c", text);
    r.git(&["config", "core.autocrlf", "false"]);
    r.commit("ABC-1: crlf");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.files[0].status, "matches-head");
    assert_eq!(subjects(&ev, 0), vec!["ABC-1: crlf"]);
}

#[test]
fn r37_validate_rejects_out_of_range_change_indices() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: a");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    ev.validate().unwrap();
    let mut bad = ev.clone();
    bad.changes.as_mut().unwrap().commits[0].functions = vec![99];
    assert!(bad.validate().is_err());
    let mut bad = ev.clone();
    bad.changes.as_mut().unwrap().tickets[0].commits = vec![7];
    assert!(bad.validate().is_err());
    let mut bad = ev.clone();
    bad.changes.as_mut().unwrap().unticketed = vec![3];
    assert!(bad.validate().is_err());
    let mut bad = ev.clone();
    bad.functions[0].commits = vec![5];
    assert!(bad.validate().is_err());
    let mut bad = ev.clone();
    bad.changes.as_mut().unwrap().files[0].file_id = 4;
    assert!(bad.validate().is_err());
}

#[test]
fn r38_validate_rejects_function_commits_without_changes() {
    let mut ev = ev_from(&[]);
    let mut f = common::function(0, "a", 1, 3);
    f.commits = vec![0];
    ev.functions.push(f);
    // three lines, because the function claims 1..=3 and a range past the
    // end of its file is now refused before this check is reached
    let mut file = common::file(&["x", "y", "z"], &[Some(1), Some(1), Some(1)]);
    file.function_ids = vec![0];
    ev.files.push(file);
    let err = ev.validate().unwrap_err();
    assert!(err.contains("changes"), "{err}");
}

#[test]
fn r39_evidence_with_changes_round_trips_through_json() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", TWO_FNS);
    r.commit("ABC-1: both");
    let mut ev = ev_disk(&r, &[("a.c", &[ONE, TWO])]);
    change::analyse(&mut ev, &opts(), None).unwrap();
    let s = serde_json::to_string(&ev).unwrap();
    let back: Evidence = serde_json::from_str(&s).unwrap();
    back.validate().unwrap();
    assert_eq!(canon(&ev), canon(&back));
}

#[test]
fn r40_jobs_zero_is_treated_as_one() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", TWO_FNS);
    r.commit("ABC-1: both");
    let mut ev = ev_disk(&r, &[("a.c", &[ONE, TWO])]);
    let (_, out) = change::analyse(&mut ev, &Options { jobs: 0, ..opts() }, None).unwrap();
    assert_eq!(out.walked, 2);
}

#[test]
fn r41_changing_patterns_re_extracts_tickets_without_a_rewalk() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int a(void) {\n    return 1;\n}\n");
    r.commit("ABC-1: fix #42");
    let mut ev = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let (cache, _) = change::analyse(&mut ev, &opts(), None).unwrap();
    assert_eq!(ticket_ids(&ev), vec!["ABC-1"]);
    let mut ev2 = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let o = Options { patterns: vec![r"#(\d+)".into()], ..opts() };
    let (_, out) = change::analyse(&mut ev2, &o, Some(&cache)).unwrap();
    assert_eq!((out.walked, out.reused), (0, 1), "patterns are applied to cached messages; no git needed");
    assert_eq!(ticket_ids(&ev2), vec!["42"]);
    // and embed honours the patterns it is given
    let mut ev3 = ev_disk(&r, &[("a.c", &[("a", 1, 3)])]);
    let pats = vec![r"#(\d+)".to_string()];
    change::embed(&mut ev3, &cache, Some(&pats)).unwrap();
    assert_eq!(ticket_ids(&ev3), vec!["42"]);
}

#[test]
fn r42_cache_round_trip_preserves_reuse() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", TWO_FNS);
    r.commit("ABC-1: both");
    let mut ev = ev_disk(&r, &[("a.c", &[ONE, TWO])]);
    let (cache, _) = change::analyse(&mut ev, &opts(), None).unwrap();
    let p = r.dir.join("cache.json");
    change::save(&cache, &p).unwrap();
    let back: Cache = change::load(&p).unwrap();
    let mut ev2 = ev_disk(&r, &[("a.c", &[ONE, TWO])]);
    let (_, out) = change::analyse(&mut ev2, &opts(), Some(&back)).unwrap();
    assert_eq!((out.walked, out.reused), (0, 2));
    assert_eq!(canon(&ev), canon(&ev2));
}
