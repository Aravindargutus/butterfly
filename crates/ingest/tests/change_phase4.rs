// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Change impact, phase 4: the ticket-centric view. Per ticket: which
//! functions it added, modified or deleted, and which files it touched --
//! including files that hold no function in the evidence (headers, build
//! files), which a function-centric walk cannot see. Deletions are located
//! through the frozen baseline (`butterfly baseline`), which must therefore
//! record where each function was.
//!
//! These encode the *intended* behaviour. Skipped when git is not installed.

mod common;

use butterfly_ingest::change::{self, Options};
use butterfly_ingest::impact;
use butterfly_model::*;
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
        let d = butterfly_testsupport::scratch_dir("p4");
        let r = Repo { dir: d.canonicalize().unwrap(), n: 0 };
        r.git(&["init", "-q", "-b", "main"]);
        r
    }
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(&self.dir).args(args).output().expect("spawn git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
    fn write(&self, rel: &str, content: &str) {
        let p = self.dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
    fn commit(&mut self, msg: &str) -> String {
        self.n += 1;
        let date = format!("2026-01-01T00:{:02}:{:02}+00:00", self.n / 60, self.n % 60);
        self.commit_dated(msg, &date)
    }
    /// Commit with an explicit author date, for the clock-skew cases.
    fn commit_dated(&self, msg: &str, date: &str) -> String {
        self.git(&["add", "-A"]);
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", msg])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .unwrap();
        assert!(out.status.success(), "commit: {}", String::from_utf8_lossy(&out.stderr));
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

/// Evidence over files as they are on disk now: `(rel path, [(name, start, end)])`.
fn ev_disk(repo: &Repo, specs: &[FileSpec]) -> Evidence {
    let mut files = Vec::new();
    let mut fns: Vec<Function> = Vec::new();
    for (fi, (rel, fl)) in specs.iter().enumerate() {
        let abs = repo.dir.join(rel);
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
            fun.requirements = vec![format!("LLR-{name}")];
            f.function_ids.push(id);
            fns.push(fun);
        }
        files.push(f);
    }
    common::evidence(files, fns)
}

fn opts(since: Option<&str>) -> Options {
    Options { since: since.map(str::to_string), repo: None, patterns: vec![r"\b([A-Z]{2,10}-\d+)\b".into()], exclude: vec![], jobs: 2, baseline: None }
}

fn ticket<'a>(ev: &'a Evidence, id: &str) -> &'a Ticket {
    ev.changes.as_ref().unwrap().tickets.iter().find(|t| t.id == id).unwrap_or_else(|| panic!("no ticket {id}"))
}

fn subjects(ev: &Evidence, fid: usize) -> Vec<String> {
    let c = ev.changes.as_ref().unwrap();
    ev.functions[fid].commits.iter().map(|&i| c.commits[i].subject.clone()).collect()
}

fn names(ev: &Evidence, ids: &[usize]) -> Vec<String> {
    let mut v: Vec<String> = ids.iter().map(|&i| ev.functions[i].name.clone()).collect();
    v.sort();
    v
}

/// Freeze a baseline from evidence into `path` (what `butterfly baseline` does).
fn freeze(ev: &Evidence, path: &Path) {
    let b = butterfly_ingest::baseline::from_evidence(ev);
    fs::write(path, serde_json::to_string(&b).unwrap()).unwrap();
}

const F1: &str = "int f1(int x) {\n    return x + 1;\n}\n";
const F1F2: &str = "int f1(int x) {\n    return x + 1;\n}\n\nint f2(int x) {\n    return x + 2;\n}\n";

// ===================================================================== normal

#[test]
fn p01_added_versus_modified_per_ticket() {
    need_git!();
    let mut r = Repo::new();
    r.write("README", "base\n");
    let base = r.commit("BASE");
    r.write("a.c", F1);
    r.commit("ABC-1: add f1");
    r.write("a.c", &F1F2.replace("x + 1", "x + 10"));
    r.commit("ABC-2: edit f1, add f2");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3), ("f2", 5, 7)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t1 = ticket(&ev, "ABC-1");
    assert_eq!(names(&ev, &t1.added), vec!["f1"]);
    assert!(t1.modified.is_empty());
    let t2 = ticket(&ev, "ABC-2");
    assert_eq!(names(&ev, &t2.added), vec!["f2"]);
    assert_eq!(names(&ev, &t2.modified), vec!["f1"]);
    assert_eq!(names(&ev, &t2.functions), vec!["f1", "f2"], "functions stays the union");
    assert!(ev.functions[0].origin.is_some() && ev.functions[1].origin.is_some());
}

#[test]
fn p02_a_function_older_than_since_is_modified_never_added() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.commit("OLD-9: long ago");
    let base = r.commit("BASE");
    r.write("a.c", &F1.replace("x + 1", "x + 2"));
    r.commit("ABC-1: tweak");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-1");
    assert_eq!(names(&ev, &t.modified), vec!["f1"]);
    assert!(t.added.is_empty());
    let c = ev.changes.as_ref().unwrap();
    assert!(c.tickets.iter().all(|t| t.id != "OLD-9"), "out-of-range tickets do not appear: {:?}", c.tickets.iter().map(|t| &t.id).collect::<Vec<_>>());
    assert_eq!(ev.functions[0].origin.as_deref().map(str::len), Some(40), "origin is known even though it predates --since");
}

#[test]
fn p03_deleted_function_is_found_through_the_baseline_and_attributed() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1F2);
    let base = r.commit("BASE");
    let ev0 = ev_disk(&r, &[("a.c", &[("f1", 1, 3), ("f2", 5, 7)])]);
    let bpath = r.dir.join("butterfly-baseline.json");
    freeze(&ev0, &bpath);
    r.write("a.c", F1);
    r.commit("ABC-3: delete f2");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let o = Options { baseline: Some(bpath.clone()), ..opts(Some(&base)) };
    change::analyse(&mut ev, &o, None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.deleted.len(), 1, "{:?}", c.deleted);
    let d = &c.deleted[0];
    assert_eq!((d.name.as_str(), d.file.as_str(), d.line_start, d.line_end), ("f2", "a.c", 5, 7));
    let cm = &c.commits[d.commit.expect("deleting commit attributed")];
    assert_eq!(cm.subject, "ABC-3: delete f2");
    let t = ticket(&ev, "ABC-3");
    assert_eq!(t.deleted, vec![0]);
    assert!(t.functions.is_empty(), "f2 no longer exists, so it is not among the ticket's live functions");
    // and the ruleset turns it into a blocker with no function id
    let imp = impact::compute(&ev, &Resolutions::default()).unwrap();
    let del: Vec<&Obligation> = imp.obligations.iter().filter(|o| o.kind == "deleted").collect();
    assert_eq!(del.len(), 1);
    assert_eq!(del[0].severity, "blocker");
    assert!(del[0].function_id.is_none());
    assert!(del[0].detail.contains("f2"), "{}", del[0].detail);
    assert_eq!(del[0].ticket, Some(c.tickets.iter().position(|t| t.id == "ABC-3").unwrap()));
    ev.changes.as_mut().unwrap().impact = Some(imp);
    ev.validate().unwrap();
}

#[test]
fn p04_ticket_file_diff_with_statuses_and_untraced_files() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.write("state.h", "#define ALT_MAX 60000\n");
    r.write("old.mk", "all:\n");
    let base = r.commit("BASE");
    r.write("a.c", &F1.replace("x + 1", "x + 3"));
    r.write("state.h", "#define ALT_MAX 65000\n");
    r.write("build.mk", "all: a.o\n");
    fs::remove_file(r.dir.join("old.mk")).unwrap();
    r.commit("ABC-1: raise the limit");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-1");
    let mut files: Vec<(&str, &str, bool, usize)> = t.files.iter().map(|f| (f.path.as_str(), f.status.as_str(), f.in_evidence, f.functions)).collect();
    files.sort();
    assert_eq!(files, vec![("a.c", "modified", true, 1), ("build.mk", "added", false, 0), ("old.mk", "deleted", false, 0), ("state.h", "modified", false, 0)]);
    let imp = impact::compute(&ev, &Resolutions::default()).unwrap();
    let mut untraced: Vec<&str> = imp.obligations.iter().filter(|o| o.kind == "untraced-file").map(|o| o.file.as_deref().unwrap()).collect();
    untraced.sort();
    assert_eq!(untraced, vec!["build.mk", "old.mk", "state.h"]);
    assert!(imp.obligations.iter().filter(|o| o.kind == "untraced-file").all(|o| o.severity == "major" && o.function_id.is_none() && o.ticket == Some(0)));
    assert!(impact::fails(&imp, impact::Level::Major));
    // ticket-level obligations are counted under the ticket
    let bt = imp.by_ticket.iter().find(|b| b.ticket == 0).unwrap();
    assert!(bt.obligations.iter().any(|&i| imp.obligations[i].kind == "untraced-file"));
}

#[test]
fn p05_rename_plus_edit_in_one_commit() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1F2);
    let base = r.commit("BASE");
    r.git(&["mv", "a.c", "b.c"]);
    r.write("b.c", &F1F2.replace("x + 2", "x + 20"));
    r.commit("ABC-1: move and edit f2");
    let mut ev = ev_disk(&r, &[("b.c", &[("f1", 1, 3), ("f2", 5, 7)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-1");
    assert_eq!(names(&ev, &t.modified), vec!["f2"], "only f2's lines changed; f1 moved with the file");
    assert!(t.added.is_empty());
    let f = t.files.iter().find(|f| f.path == "b.c").expect("renamed file listed under its new path");
    assert_eq!(f.status, "renamed");
    assert!(f.in_evidence && f.functions == 2);
}

#[test]
fn p06_ticket_spanning_two_files() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.write("b.c", "int g1(int x) {\n    return x - 1;\n}\n");
    let base = r.commit("BASE");
    r.write("a.c", &F1.replace("x + 1", "x + 5"));
    r.write("b.c", "int g1(int x) {\n    return x - 5;\n}\n");
    r.commit("ABC-1: both");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)]), ("b.c", &[("g1", 1, 3)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-1");
    assert_eq!(names(&ev, &t.modified), vec!["f1", "g1"]);
    assert_eq!(t.files.len(), 2);
    assert!(t.files.iter().all(|f| f.in_evidence && f.status == "modified"));
}

// =================================================================== abnormal

#[test]
fn a07_whole_file_deleted_still_locates_the_function() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.write("b.c", "int g1(int x) {\n    return x - 1;\n}\n");
    let base = r.commit("BASE");
    let ev0 = ev_disk(&r, &[("a.c", &[("f1", 1, 3)]), ("b.c", &[("g1", 1, 3)])]);
    let bpath = r.dir.join("butterfly-baseline.json");
    freeze(&ev0, &bpath);
    fs::remove_file(r.dir.join("b.c")).unwrap();
    r.commit("ABC-9: drop b.c");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let o = Options { baseline: Some(bpath), ..opts(Some(&base)) };
    change::analyse(&mut ev, &o, None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.deleted.len(), 1);
    assert_eq!(c.deleted[0].name, "g1");
    assert_eq!(c.commits[c.deleted[0].commit.unwrap()].subject, "ABC-9: drop b.c");
    let t = ticket(&ev, "ABC-9");
    assert_eq!(t.deleted, vec![0]);
    assert!(t.files.iter().any(|f| f.path == "b.c" && f.status == "deleted"));
}

#[test]
fn a08_unticketed_deletion_is_still_a_blocker() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1F2);
    let base = r.commit("BASE");
    let ev0 = ev_disk(&r, &[("a.c", &[("f1", 1, 3), ("f2", 5, 7)])]);
    let bpath = r.dir.join("butterfly-baseline.json");
    freeze(&ev0, &bpath);
    r.write("a.c", F1);
    r.commit("removed f2, no ticket");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let o = Options { baseline: Some(bpath), ..opts(Some(&base)) };
    change::analyse(&mut ev, &o, None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.deleted.len(), 1);
    let ci = c.deleted[0].commit.unwrap();
    assert!(c.commits[ci].tickets.is_empty());
    assert!(c.unticketed.contains(&ci), "the deleting commit is listed as unticketed even though no live function references it");
    let imp = impact::compute(&ev, &Resolutions::default()).unwrap();
    let del: Vec<&Obligation> = imp.obligations.iter().filter(|o| o.kind == "deleted").collect();
    assert_eq!(del.len(), 1);
    assert!(del[0].ticket.is_none());
    assert!(impact::fails(&imp, impact::Level::Blocker));
}

#[test]
fn a09_deletions_need_since_and_say_so() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1F2);
    r.commit("BASE");
    let ev0 = ev_disk(&r, &[("a.c", &[("f1", 1, 3), ("f2", 5, 7)])]);
    let bpath = r.dir.join("butterfly-baseline.json");
    freeze(&ev0, &bpath);
    r.write("a.c", F1);
    r.commit("ABC-3: delete f2");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let o = Options { baseline: Some(bpath), ..opts(None) };
    change::analyse(&mut ev, &o, None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert!(c.deleted.is_empty());
    assert!(c.caveats.iter().any(|x| x.contains("--since") && x.to_lowercase().contains("delet")), "{:?}", c.caveats);
}

#[test]
fn a10_old_baseline_without_locations_is_a_caveat_not_a_crash() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1F2);
    let base = r.commit("BASE");
    let ev0 = ev_disk(&r, &[("a.c", &[("f1", 1, 3), ("f2", 5, 7)])]);
    let mut b = butterfly_ingest::baseline::from_evidence(&ev0);
    for fp in b.functions.values_mut() {
        fp.file.clear();
        fp.line_start = 0;
        fp.line_end = 0;
    }
    let bpath = r.dir.join("old-baseline.json");
    fs::write(&bpath, serde_json::to_string(&b).unwrap()).unwrap();
    r.write("a.c", F1);
    r.commit("ABC-3: delete f2");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let o = Options { baseline: Some(bpath), ..opts(Some(&base)) };
    change::analyse(&mut ev, &o, None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert!(c.deleted.is_empty());
    assert!(c.caveats.iter().any(|x| x.contains("f2") && x.to_lowercase().contains("baseline")), "{:?}", c.caveats);
}

#[test]
fn a11_revert_within_a_ticket_still_counts_as_touched() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    let base = r.commit("BASE");
    r.write("a.c", &F1.replace("x + 1", "x + 9"));
    r.commit("ABC-1: try");
    r.write("a.c", F1);
    r.commit("ABC-1: revert");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-1");
    assert_eq!(names(&ev, &t.modified), vec!["f1"], "touched-then-reverted is still reported: the conservative superset");
    assert_eq!(t.commits.len(), 2);
}

#[test]
fn a12_missing_baseline_file_is_an_error_naming_it() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    let base = r.commit("BASE");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let o = Options { baseline: Some(r.dir.join("nope.json")), ..opts(Some(&base)) };
    let err = change::analyse(&mut ev, &o, None).unwrap_err().to_string();
    assert!(err.contains("nope.json"), "{err}");
}

// ================================================================= robustness

#[test]
fn r13_a_different_since_reuses_the_cache() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    let base = r.commit("BASE");
    r.write("a.c", &F1.replace("x + 1", "x + 2"));
    let mid = r.commit("ABC-1: one");
    r.write("a.c", &F1.replace("x + 1", "x + 3"));
    r.commit("ABC-2: two");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let (cache, _) = change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    assert_eq!(ev.changes.as_ref().unwrap().tickets.len(), 2);
    let mut ev2 = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let (_, out) = change::analyse(&mut ev2, &opts(Some(&mid)), Some(&cache)).unwrap();
    assert_eq!((out.walked, out.reused), (0, 1), "the history is cached in full; --since only filters it");
    let ids: Vec<&str> = ev2.changes.as_ref().unwrap().tickets.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, vec!["ABC-2"]);
    let mut ev3 = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let (_, out3) = change::analyse(&mut ev3, &opts(None), Some(&cache)).unwrap();
    assert_eq!((out3.walked, out3.reused), (0, 1));
    assert_eq!(ev3.changes.as_ref().unwrap().tickets.len(), 2);
}

#[test]
fn r14_ticket_level_obligations_print_round_trip_and_validate() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.write("cfg.h", "#define X 1\n");
    let base = r.commit("BASE");
    r.write("cfg.h", "#define X 2\n");
    r.commit("ABC-1: config only");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let imp = impact::compute(&ev, &Resolutions::default()).unwrap();
    assert_eq!(imp.obligations.len(), 1);
    assert_eq!(imp.obligations[0].kind, "untraced-file");
    assert!(imp.functions_changed.is_empty(), "no function changed, yet the ticket is not harmless");
    ev.changes.as_mut().unwrap().impact = Some(imp);
    ev.validate().unwrap();
    let s = serde_json::to_string(&ev).unwrap();
    let back: Evidence = serde_json::from_str(&s).unwrap();
    back.validate().unwrap();
    let mut bad = ev.clone();
    bad.changes.as_mut().unwrap().impact.as_mut().unwrap().obligations[0].ticket = Some(9);
    assert!(bad.validate().is_err());
    let mut bad = ev.clone();
    bad.changes.as_mut().unwrap().tickets[0].deleted = vec![9];
    assert!(bad.validate().is_err());
}

#[test]
fn r15_exclude_globs_scope_the_ticket_file_list_too() {
    need_git!();
    let mut r = Repo::new();
    r.write("src/a.c", F1);
    r.write("vendor/lib.h", "#define V 1\n");
    r.write("docs/notes.md", "x\n");
    let base = r.commit("BASE");
    r.write("src/a.c", &F1.replace("x + 1", "x + 4"));
    r.write("vendor/lib.h", "#define V 2\n");
    r.write("docs/notes.md", "y\n");
    r.commit("ABC-1: everything");
    let mut ev = ev_disk(&r, &[("src/a.c", &[("f1", 1, 3)])]);
    let o = Options { exclude: vec!["vendor/**".into(), "*.md".into()], ..opts(Some(&base)) };
    change::analyse(&mut ev, &o, None).unwrap();
    let t = ticket(&ev, "ABC-1");
    let paths: Vec<&str> = t.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["src/a.c"], "excluded files are not the ticket's problem");
    let imp = impact::compute(&ev, &Resolutions::default()).unwrap();
    assert!(imp.obligations.iter().all(|o| o.kind != "untraced-file"));
}

#[test]
fn r16_identical_signature_elsewhere_in_the_file_does_not_steal_the_origin() {
    need_git!();
    let mut r = Repo::new();
    // two impls with a byte-identical `fn new` line; the second is added later
    r.write("a.c", "int a_new(int raw) {\n    return raw;\n}\n");
    let base = r.commit("BASE");
    r.write("a.c", "int a_new(int raw) {\n    return raw;\n}\n\nint a_new(int raw) {\n    return raw + 1;\n}\n");
    r.commit("ABC-7: add the second one");
    let mut ev = ev_disk(&r, &[("a.c", &[("first", 1, 3), ("second", 5, 7)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-7");
    assert_eq!(names(&ev, &t.added), vec!["second"], "the second function's origin is the commit that added ITS lines, not the first's");
    assert!(t.modified.is_empty(), "the first function was not touched: {:?}", names(&ev, &t.modified));
    let c = ev.changes.as_ref().unwrap();
    assert_ne!(ev.functions[0].origin, ev.functions[1].origin);
    assert_eq!(ev.functions[1].origin.as_deref(), Some(c.commits[t.commits[0]].sha.as_str()));
}

// ============================================ issue #13: ancestry, not clocks

/// Git author dates are user-controlled and need not increase along the
/// commit graph. Filtering a function's commits by timestamp therefore drops
/// real changes; the walk order that `git log -L` returns is the ancestry.
#[test]
fn i13a_child_commit_with_an_older_author_date_is_still_attributed() {
    need_git!();
    let r = Repo::new();
    r.write("a.c", "int keep(void) {\n    return 0;\n}\n");
    let base = r.commit_dated("BASE", "2026-01-01T00:00:00Z");
    // the function is introduced with a date far in the future ...
    r.write("a.c", "int keep(void) {\n    return 0;\n}\n\nint f1(int x) {\n    return x + 1;\n}\n");
    r.commit_dated("ABC-1: add f1", "2030-06-01T00:00:00Z");
    // ... and modified by a child commit dated years earlier
    r.write("a.c", "int keep(void) {\n    return 0;\n}\n\nint f1(int x) {\n    return x + 99;\n}\n");
    r.commit_dated("ABC-2: change the body", "2020-03-01T00:00:00Z");
    let mut ev = ev_disk(&r, &[("a.c", &[("keep", 1, 3), ("f1", 5, 7)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let subs: Vec<String> = subjects(&ev, 1);
    assert!(subs.iter().any(|s| s.starts_with("ABC-2")), "the child commit must not be dropped for having an older clock: {subs:?}");
    assert!(subs.iter().any(|s| s.starts_with("ABC-1")), "the origin commit is still listed: {subs:?}");
    let t = ticket(&ev, "ABC-2");
    assert_eq!(names(&ev, &t.modified), vec!["f1"], "and it is reported as a modification");
    let imp = impact::compute(&ev, &Resolutions::default()).unwrap();
    assert!(imp.functions_changed.contains(&1), "a changed function with a skewed clock still yields obligations");
}

/// The mirror image: a parent dated later than its child must not swallow
/// the child's commits either.
#[test]
fn i13b_parent_with_a_newer_author_date_does_not_invert_history() {
    need_git!();
    let r = Repo::new();
    r.write("a.c", "int f1(int x) {\n    return x + 1;\n}\n");
    let base = r.commit_dated("BASE", "2030-01-01T00:00:00Z");
    r.write("a.c", "int f1(int x) {\n    return x + 2;\n}\n");
    r.commit_dated("ABC-1: later change, earlier clock", "2021-01-01T00:00:00Z");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-1");
    assert_eq!(names(&ev, &t.modified), vec!["f1"]);
    assert!(t.added.is_empty(), "the function predates --since, so it was modified, not added");
}

/// A one-line function's whole line changes when its body changes, so an
/// exact signature match called the modifying commit the origin and the
/// ticket reported `added`.
#[test]
fn i13c_editing_a_one_line_function_body_is_modified_not_added() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int one_liner(int x) { return x + 1; }\n");
    let base = r.commit("BASE");
    r.write("a.c", "int one_liner(int x) { return x + 2; }\n");
    r.commit("ABC-1: tweak the constant");
    let mut ev = ev_disk(&r, &[("a.c", &[("one_liner", 1, 1)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-1");
    assert_eq!(names(&ev, &t.modified), vec!["one_liner"], "a body edit is a modification");
    assert!(t.added.is_empty(), "it existed before --since, so nothing was added: {:?}", names(&ev, &t.added));
}

/// ... while a genuinely new one-line function still reports `added`.
#[test]
fn i13d_a_genuinely_new_one_line_function_is_added() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int existing(void) { return 0; }\n");
    let base = r.commit("BASE");
    r.write("a.c", "int existing(void) { return 0; }\nint fresh(int x) { return x + 1; }\n");
    r.commit("ABC-2: add fresh");
    let mut ev = ev_disk(&r, &[("a.c", &[("existing", 1, 1), ("fresh", 2, 2)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-2");
    assert_eq!(names(&ev, &t.added), vec!["fresh"]);
    assert!(t.modified.is_empty(), "existing was untouched: {:?}", names(&ev, &t.modified));
}

/// When the origin cannot be established, changes are kept rather than
/// filtered away: an unknown origin must never lose a commit.
#[test]
fn i13e_unknown_origin_keeps_every_commit() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int f(void) {\n    return 1;\n}\n");
    let base = r.commit("BASE");
    r.write("a.c", "int f(void) {\n    return 2;\n}\n");
    r.commit("ABC-1: change");
    // a signature too short to search for -> origin unknown
    let mut ev = ev_disk(&r, &[("a.c", &[("f", 2, 2)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    assert!(!ev.functions[0].commits.is_empty(), "an unknown origin must not drop the function's commits");
}

/// An ordinary signature edit -- renaming a parameter, changing its type --
/// must not reset the function's identity. `signature_identity` kept the
/// whole parameter list, so the edit added an identity that had never been
/// removed and the editing commit was chosen as the origin: an existing
/// function was reported `added` rather than `modified` (review of #13).
#[test]
fn p13_a_signature_edit_is_a_modification_not_an_introduction() {
    need_git!();
    // parameter renamed, type changed, a parameter added, and the return
    // type changed -- every ordinary edit that moves the signature
    for (before, after, what) in [
        ("int f(int old_name) {\n    return old_name + 1;\n}\n", "int f(int new_name) {\n    return new_name + 1;\n}\n", "parameter renamed"),
        ("int f(int x) {\n    return x + 1;\n}\n", "int f(long x) {\n    return x + 1;\n}\n", "parameter type changed"),
        ("int f(int x) {\n    return x + 1;\n}\n", "int f(int x, int y) {\n    return x + y;\n}\n", "parameter added"),
        ("int f(int x) {\n    return x + 1;\n}\n", "long f(int x) {\n    return x + 1;\n}\n", "return type changed"),
    ] {
        let mut r = Repo::new();
        r.write("a.c", before);
        let born = r.commit("OLD-1: introduce f");
        let base = r.commit("BASE");
        r.write("a.c", after);
        r.commit("ABC-1: edit the signature");
        let mut ev = ev_disk(&r, &[("a.c", &[("f", 1, 3)])]);
        change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
        let t = ticket(&ev, "ABC-1");
        assert_eq!(names(&ev, &t.modified), vec!["f"], "{what}: an existing function was edited");
        assert!(t.added.is_empty(), "{what}: it was not introduced by this ticket");
        assert_eq!(ev.functions[0].origin.as_deref(), Some(born.as_str()), "{what}: the origin is still the commit that introduced f");
    }
}

/// Rust is an advertised supported language, and a generic signature took a
/// different path through identity extraction: the `>` of `f<T>` sat where
/// the identifier scan expected the name, so it fell back to the full
/// signature and an ordinary parameter rename reported the function as
/// `added` with the editing commit as its origin (second review of #13).
#[test]
fn p15_a_generic_rust_signature_edit_is_a_modification_too() {
    need_git!();
    for (before, after, what) in [
        ("pub fn f<T>(old_name: T) -> T {\n    old_name\n}\n", "pub fn f<T>(new_name: T) -> T {\n    new_name\n}\n", "generic parameter renamed"),
        ("pub fn f<T: Clone>(x: T) -> T {\n    x.clone()\n}\n", "pub fn f<T: Clone + Send>(x: T) -> T {\n    x.clone()\n}\n", "generic bound changed"),
        (
            "pub fn f<T: Into<Vec<u8>>>(x: T) -> usize {\n    x.into().len()\n}\n",
            "pub fn f<T: Into<Vec<u8>>>(y: T) -> usize {\n    y.into().len()\n}\n",
            "nested generics, parameter renamed",
        ),
        // restricted visibility puts a `(` before the name; taking it for
        // the parameter list made the identity `pub` (self-review of #13)
        ("pub(crate) fn f(old_name: i32) -> i32 {\n    old_name + 1\n}\n", "pub(crate) fn f(new_name: i32) -> i32 {\n    new_name + 1\n}\n", "pub(crate), parameter renamed"),
        ("pub fn f(x: i32) -> i32 {\n    x + 1\n}\n", "pub(crate) fn f(x: i32) -> i32 {\n    x + 1\n}\n", "visibility narrowed"),
    ] {
        let mut r = Repo::new();
        r.write("a.rs", before);
        let born = r.commit("OLD-1: introduce f");
        let base = r.commit("BASE");
        r.write("a.rs", after);
        r.commit("ABC-1: edit the signature");
        let mut ev = ev_disk(&r, &[("a.rs", &[("f", 1, 3)])]);
        change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
        let t = ticket(&ev, "ABC-1");
        assert_eq!(names(&ev, &t.modified), vec!["f"], "{what}: an existing function was edited");
        assert!(t.added.is_empty(), "{what}: it was not introduced by this ticket");
        assert_eq!(ev.functions[0].origin.as_deref(), Some(born.as_str()), "{what}: the origin is still the commit that introduced f");
    }
}

/// C++ is an advertised supported language and its operators take a
/// different path through identity extraction: the name ends in symbols, so
/// the identifier scan found nothing and the full signature was used again.
/// Renaming a parameter then reported the operator as `added` and lost its
/// origin (second review of #13).
#[test]
fn p16_a_cpp_operator_signature_edit_is_a_modification_too() {
    need_git!();
    for (before, after, name, what) in [
        (
            "bool C::operator==(const C& old_name) const {\n    return v == old_name.v;\n}\n",
            "bool C::operator==(const C& new_name) const {\n    return v == new_name.v;\n}\n",
            "C::operator==",
            "equality, parameter renamed",
        ),
        (
            "int& C::operator[](size_t old_name) {\n    return v[old_name];\n}\n",
            "int& C::operator[](size_t new_name) {\n    return v[new_name];\n}\n",
            "C::operator[]",
            "subscript, parameter renamed",
        ),
        (
            "Vec operator+(const Vec &a, const Vec &b) {\n    return add(a, b);\n}\n",
            "Vec operator+(const Vec &a, const Vec &c) {\n    return add(a, c);\n}\n",
            "operator+",
            "free operator, parameter renamed",
        ),
    ] {
        let mut r = Repo::new();
        r.write("a.cpp", before);
        let born = r.commit("OLD-1: introduce the operator");
        let base = r.commit("BASE");
        r.write("a.cpp", after);
        r.commit("ABC-1: edit the signature");
        let mut ev = ev_disk(&r, &[("a.cpp", &[(name, 1, 3)])]);
        change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
        let t = ticket(&ev, "ABC-1");
        assert_eq!(names(&ev, &t.modified), vec![name], "{what}: an existing function was edited");
        assert!(t.added.is_empty(), "{what}: it was not introduced by this ticket");
        assert_eq!(ev.functions[0].origin.as_deref(), Some(born.as_str()), "{what}: the origin is still the commit that introduced it");
    }
}

/// The other half of the same rule: a function that really is new must
/// still be reported as added, so the fix above cannot be "call everything
/// a modification".
#[test]
fn p14_a_genuinely_new_function_is_still_added() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", "int f(int x) {\n    return x + 1;\n}\n");
    r.commit("OLD-1: introduce f");
    let base = r.commit("BASE");
    r.write("a.c", "int f(int x) {\n    return x + 1;\n}\n\nint g(int y) {\n    return y + 2;\n}\n");
    r.commit("ABC-1: add g");
    let mut ev = ev_disk(&r, &[("a.c", &[("f", 1, 3), ("g", 5, 7)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-1");
    assert_eq!(names(&ev, &t.added), vec!["g"], "g is new in this ticket");
    assert!(!names(&ev, &t.added).contains(&"f".to_string()), "f is not");
}
