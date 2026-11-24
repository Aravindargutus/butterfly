// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests, area D: tickets in commit messages, the
//! `[change]` configuration, the ticket view and the change-history cache.
//!
//! Every expected value is taken from the documentation (README.md
//! "Project configuration", "Change impact", "The ticket view";
//! docs/butterfly.html "How attribution works", "The ticket view"), never
//! from what the implementation currently does. Each test names the line it
//! checks. Every git case builds its own throwaway repository in a temporary
//! directory; skipped when git is not installed.

mod common;

use butterfly_ingest::change::{self, Options};
use butterfly_ingest::config::Config;
use butterfly_ingest::impact;
use butterfly_model::*;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        let d = butterfly_testsupport::scratch_dir("reqd");
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
    fn remove(&self, rel: &str) {
        fs::remove_file(self.dir.join(rel)).unwrap();
    }
    /// Commit everything, with a distinct increasing date so the order is
    /// deterministic.
    fn commit(&mut self, msg: &str) -> String {
        self.n += 1;
        let date = format!("2026-01-01T00:{:02}:{:02}+00:00", self.n / 60, self.n % 60);
        self.git(&["add", "-A"]);
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", msg])
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
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

/// Evidence over the files as they are on disk now. Every function is
/// executed, covered and traced to `LLR-<name>`.
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
    ev.changes.as_ref().unwrap().tickets.iter().find(|t| t.id == id).unwrap_or_else(|| panic!("no ticket {id}: {:?}", ev.changes.as_ref().unwrap().tickets))
}

fn names(ev: &Evidence, ids: &[usize]) -> Vec<String> {
    let mut v: Vec<String> = ids.iter().map(|&i| ev.functions[i].name.clone()).collect();
    v.sort();
    v
}

fn file_status<'a>(t: &'a Ticket, path: &str) -> Option<&'a str> {
    t.files.iter().find(|f| f.path == path).map(|f| f.status.as_str())
}

fn res(patterns: &[&str]) -> Vec<Regex> {
    patterns.iter().map(|p| Regex::new(p).unwrap()).collect()
}

const F1: &str = "int f1(int x) {\n    return x + 1;\n}\n";

// ======================================================= ticket extraction

/// README.md:1181-1182 -- "The built-in pattern is Jira-style `ABC-123`. It
/// is deliberately narrow and still matches things like `DO-178` or `UTF-8`
/// in a message".
#[test]
fn d01_the_built_in_pattern_is_jira_style_and_also_matches_do_178_and_utf_8() {
    let cfg = Config::parse("", Path::new("butterfly.toml")).unwrap();
    let pats: Vec<&str> = cfg.change.patterns.iter().map(String::as_str).collect();
    let r = res(&pats);
    assert_eq!(change::tickets_in(&r, "FCS-104: clamp the altitude"), vec!["FCS-104"]);
    let found = change::tickets_in(&r, "decode UTF-8 names as DO-178 asks");
    assert!(found.contains(&"UTF-8".to_string()) && found.contains(&"DO-178".to_string()), "the doc says both are matched: {found:?}");
    assert!(change::tickets_in(&r, "tidy whitespace").is_empty(), "no ticket in a message without one");
    assert!(cfg.change.exclude.is_empty(), "no exclude globs unless configured");
}

/// README.md:1176-1177, docs/butterfly.html "Configuration" -- "capture group
/// 1 is the identifier, or the whole match when the pattern has no group";
/// several patterns are all applied; a ticket named twice is one ticket.
#[test]
fn d02_capture_group_one_or_the_whole_match_several_patterns_and_no_duplicates() {
    let r = res(&[r"\b([A-Z]{2,10}-\d+)\b", r"refs #(\d+)", r"CR\d+"]);
    let t = change::tickets_in(&r, "ABC-1 and ABC-2: fix\n\nrefs #42, see CR7; ABC-1 again");
    assert_eq!(t, vec!["ABC-1", "ABC-2", "42", "CR7"]);
}

/// README.md:1174-1178 -- the `[change]` table carries `patterns` and
/// `exclude`; a table without one of them keeps that one's default.
#[test]
fn d03_change_table_is_read_from_butterfly_toml() {
    let text = "[change]\npatterns = ['refs #(\\d+)']\nexclude = [\"vendor/**\", \"generated/**\"]\n";
    let cfg = Config::parse(text, Path::new("butterfly.toml")).unwrap();
    assert_eq!(cfg.change.patterns, vec![r"refs #(\d+)".to_string()]);
    assert_eq!(cfg.change.exclude, vec!["vendor/**".to_string(), "generated/**".to_string()]);
    let only_ex = Config::parse("[change]\nexclude = [\"x/**\"]\n", Path::new("b.toml")).unwrap();
    assert_eq!(only_ex.change.patterns, Config::parse("", Path::new("b.toml")).unwrap().change.patterns, "patterns default to the built-in one");
    // a malformed table is an error naming the file, never a silent default
    let err = Config::parse("[change]\npatterns = \"not a list\"\n", Path::new("/p/butterfly.toml")).unwrap_err();
    assert!(format!("{err:#}").contains("/p/butterfly.toml"), "{err:#}");
}

/// README.md:1212-1213 -- "Tickets are extracted from each commit message
/// (subject *and* body)".
#[test]
fn d04_a_ticket_only_in_the_body_is_extracted() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.commit("Refactor the adder\n\nPart of ABC-77.\nrefs #9");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let o = Options { patterns: vec![r"\b([A-Z]{2,10}-\d+)\b".into(), r"refs #(\d+)".into()], ..opts(None) };
    change::analyse(&mut ev, &o, None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    let ids: Vec<&str> = c.tickets.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert!(ids.contains(&"ABC-77") && ids.contains(&"9"), "{ids:?}");
    assert!(c.unticketed.is_empty(), "a ticket in the body is a ticket");
    assert_eq!(ticket(&ev, "ABC-77").functions, vec![0]);
}

/// README.md:1222-1223 ("a table of commits with no ticket") and README.md:1260
/// (`unticketed` | blocker | a commit touching it carries no ticket).
#[test]
fn d05_a_commit_with_no_ticket_is_listed_and_is_a_blocker_on_what_it_touched() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    let base = r.commit("BASE");
    r.write("a.c", &F1.replace("x + 1", "x + 2"));
    r.commit("tidy up the adder\n\nno reference here");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.unticketed.len(), 1);
    assert_eq!(c.commits[c.unticketed[0]].subject, "tidy up the adder");
    assert!(c.tickets.is_empty());
    let imp = impact::compute(&ev, &Resolutions::default()).unwrap();
    let u: Vec<&Obligation> = imp.obligations.iter().filter(|o| o.kind == "unticketed").collect();
    assert_eq!(u.len(), 1, "{:?}", imp.obligations);
    assert_eq!(u[0].severity, "blocker");
    assert_eq!(u[0].function_id, Some(0));
    assert!(impact::fails(&imp, impact::Level::Blocker));
}

// ====================================================== [change].exclude

/// README.md:1178 ("gitignore-style, repo-relative") and README.md:1313-1315
/// (the excludes scope the ticket's file list too). gitignore: `?` is one
/// character other than `/`, and a pattern with no slash matches at any
/// depth.
#[test]
fn d06_exclude_globs_are_gitignore_style() {
    need_git!();
    let mut r = Repo::new();
    let g = |n: &str| format!("int {n}(void) {{\n    return 1;\n}}\n");
    r.write("src/a.c", &g("a"));
    r.write("gen1.c", &g("g1"));
    r.write("src/deep/gen2.c", &g("g2"));
    r.write("src/genXY.c", &g("gxy"));
    r.write("notes.tmp", "x\n");
    let base = r.commit("BASE");
    for (p, n) in [("src/a.c", "a"), ("gen1.c", "g1"), ("src/deep/gen2.c", "g2"), ("src/genXY.c", "gxy")] {
        r.write(p, &g(n).replace("return 1", "return 2"));
    }
    r.write("notes.tmp", "y\n");
    r.commit("ABC-1: everything");
    let spec: &[FileSpec] = &[("src/a.c", &[("a", 1, 3)]), ("gen1.c", &[("g1", 1, 3)]), ("src/deep/gen2.c", &[("g2", 1, 3)]), ("src/genXY.c", &[("gxy", 1, 3)])];
    let mut ev = ev_disk(&r, spec);
    let o = Options { exclude: vec!["gen?.c".into(), "*.tmp".into()], ..opts(Some(&base)) };
    let (_, out) = change::analyse(&mut ev, &o, None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    let st: Vec<&str> = c.files.iter().map(|f| f.status.as_str()).collect();
    assert_eq!(st[1], "excluded", "gen?.c matches gen1.c at the top");
    assert_eq!(st[2], "excluded", "a pattern with no slash matches at any depth");
    assert_ne!(st[3], "excluded", "? is exactly one character: genXY.c is not gen?.c");
    assert_ne!(st[0], "excluded");
    assert_eq!((out.walked, out.skipped), (2, 2));
    let t = ticket(&ev, "ABC-1");
    let mut paths: Vec<&str> = t.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["src/a.c", "src/genXY.c"], "excluded files are out of the ticket's file list too");
}

// ======================================================== the ticket view

/// README.md:1316-1318 -- "every file its commits changed with the net status
/// over the ticket (`added` / `modified` / `deleted` / `renamed`)". A file
/// added and then edited within one ticket is, net, added; one edited and
/// then deleted is, net, deleted.
#[test]
fn d07_net_status_added_then_modified_is_added_and_modified_then_deleted_is_deleted() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.write("old.h", "#define A 1\n");
    let base = r.commit("BASE");
    r.write("new.h", "#define B 1\n");
    r.write("old.h", "#define A 2\n");
    r.commit("ABC-1: part one");
    r.write("new.h", "#define B 2\n");
    r.remove("old.h");
    r.commit("ABC-1: part two");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-1");
    assert_eq!(file_status(t, "new.h"), Some("added"));
    assert_eq!(file_status(t, "old.h"), Some("deleted"));
    assert!(t.files.iter().all(|f| !f.in_evidence && f.functions == 0), "{:?}", t.files);
}

/// README.md:1316-1318 -- the *net* status over the ticket. A file the
/// ticket deleted and then put back exists both before and after the ticket:
/// net, it was modified, not deleted.
#[test]
fn d08_net_status_deleted_then_re_added_is_modified() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.write("cfg.h", "#define LIMIT 10\n");
    let base = r.commit("BASE");
    r.remove("cfg.h");
    r.commit("ABC-2: drop the old config header");
    r.write("cfg.h", "#define LIMIT 20\n");
    r.commit("ABC-2: bring the header back with the new limit");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-2");
    assert_eq!(file_status(t, "cfg.h"), Some("modified"), "cfg.h exists before and after ABC-2: {:?}", t.files);
}

/// README.md:1316-1318 -- a file whose type changed (a regular file replaced
/// by a symlink) is one of the four statuses: it still exists, so `modified`.
#[test]
fn d09_a_type_change_is_reported_as_modified() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.write("cfg.h", "#define A 1\n");
    r.write("real.h", "#define A 1\n");
    let base = r.commit("BASE");
    r.remove("cfg.h");
    std::os::unix::fs::symlink("real.h", r.dir.join("cfg.h")).unwrap();
    r.commit("ABC-3: point cfg.h at real.h");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    change::analyse(&mut ev, &opts(Some(&base)), None).unwrap();
    let t = ticket(&ev, "ABC-3");
    let st = file_status(t, "cfg.h");
    assert!(matches!(st, Some("modified" | "added" | "deleted" | "renamed")), "one of the four documented statuses: {st:?}");
    assert_eq!(st, Some("modified"));
}

/// README.md:1350-1351 -- "A renamed function appears as deleted plus
/// added, which is what its tests and callers see"; README.md:1304-1307 --
/// the deletion is attributed to "the first commit after [the last one its
/// lines existed in] to touch the file". Here only the signature line
/// changes; the body survives the rename.
#[test]
fn d10_a_function_renamed_in_place_is_deleted_plus_added_in_the_renaming_ticket() {
    need_git!();
    let mut r = Repo::new();
    let old = "int old_name(int x) {\n    int y = x;\n    y = y * 2;\n    y = y + 1;\n    return y;\n}\n";
    r.write("a.c", old);
    let base = r.commit("BASE");
    let ev0 = ev_disk(&r, &[("a.c", &[("old_name", 1, 6)])]);
    let bpath = r.dir.join("butterfly-baseline.json");
    fs::write(&bpath, serde_json::to_string(&butterfly_ingest::baseline::from_evidence(&ev0)).unwrap()).unwrap();
    r.write("a.c", &old.replace("old_name", "new_name"));
    r.commit("ABC-4: rename old_name to new_name");
    let mut ev = ev_disk(&r, &[("a.c", &[("new_name", 1, 6)])]);
    let o = Options { baseline: Some(bpath), ..opts(Some(&base)) };
    change::analyse(&mut ev, &o, None).unwrap();
    let c = ev.changes.as_ref().unwrap();
    assert_eq!(c.deleted.len(), 1, "old_name is gone: {:?}", c.deleted);
    assert_eq!(c.deleted[0].name, "old_name");
    let t = ticket(&ev, "ABC-4");
    assert_eq!(names(&ev, &t.added), vec!["new_name"], "the new name is added");
    let cm = c.deleted[0].commit.map(|i| c.commits[i].subject.clone());
    assert_eq!(cm.as_deref(), Some("ABC-4: rename old_name to new_name"), "the rename commit is the one that removed old_name");
    assert_eq!(t.deleted, vec![0], "the renaming ticket lists the deletion");
    // and the `deleted` blocker is grouped under that ticket
    let imp = impact::compute(&ev, &Resolutions::default()).unwrap();
    let del: Vec<&Obligation> = imp.obligations.iter().filter(|o| o.kind == "deleted").collect();
    assert_eq!(del.len(), 1);
    assert_eq!(del[0].ticket, Some(c.tickets.iter().position(|t| t.id == "ABC-4").unwrap()));
}

/// README.md:1304-1307 + "None of this is recoverable from git, so none of it
/// is pretended" (README.md:1288): a certified function the baseline places
/// outside the repository cannot be located in its history. It must not be
/// silently dropped: the run says which one it could not locate.
#[test]
fn d11_a_baseline_function_outside_the_repository_is_named_not_dropped() {
    need_git!();
    let mut r = Repo::new();
    let two = "int f1(int x) {\n    return x + 1;\n}\n\nint f2(int x) {\n    return x + 2;\n}\n";
    r.write("a.c", two);
    let base = r.commit("BASE");
    let mut ev0 = ev_disk(&r, &[("a.c", &[("f1", 1, 3), ("f2", 5, 7)])]);
    let mut b = butterfly_ingest::baseline::from_evidence(&ev0);
    // f2 as the baseline recorded it, in a tree that is not this repository
    let key = b.functions.keys().find(|k| k.starts_with("f2")).unwrap().clone();
    let mut fp = b.functions.remove(&key).unwrap();
    fp.file = "/nonexistent-elsewhere/a.c".into();
    b.functions.insert(format!("f2\u{1f}{}\u{1f}5", fp.file), fp);
    let bpath = r.dir.join("b.json");
    fs::write(&bpath, serde_json::to_string(&b).unwrap()).unwrap();
    r.write("a.c", F1);
    r.commit("ABC-5: remove f2");
    ev0 = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let o = Options { baseline: Some(bpath), ..opts(Some(&base)) };
    change::analyse(&mut ev0, &o, None).unwrap();
    let c = ev0.changes.as_ref().unwrap();
    let said = c.deleted.iter().any(|d| d.name == "f2") || c.caveats.iter().any(|x| x.contains("f2"));
    assert!(said, "f2 left the code; it is either a deletion or named in a caveat: {:?} / {:?}", c.deleted, c.caveats);
}

// ================================================================ the cache

/// README.md:1262-1265 -- "`ingest` re-attaches the cache without touching git
/// as long as every file's hash still matches, and says exactly which file
/// differs when one does not."
#[test]
fn d12_reattaching_names_the_file_whose_hash_differs() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.write("b.c", "int g(int x) {\n    return x;\n}\n");
    r.commit("ABC-1: a and b");
    let spec: &[FileSpec] = &[("a.c", &[("f1", 1, 3)]), ("b.c", &[("g", 1, 3)])];
    let mut ev = ev_disk(&r, spec);
    let (cache, _) = change::analyse(&mut ev, &opts(None), None).unwrap();
    // the same tree re-attaches, without git
    let mut same = ev_disk(&r, spec);
    change::embed(&mut same, &cache, None).unwrap();
    assert!(same.changes.is_some());
    // b.c differs
    r.write("b.c", "int g(int x) {\n    return -x;\n}\n");
    let mut moved = ev_disk(&r, spec);
    let err = change::embed(&mut moved, &cache, None).unwrap_err();
    assert!(err.contains("b.c"), "names the file that differs: {err}");
    assert!(!err.contains("a.c"), "and only that one: {err}");
    assert!(moved.changes.is_none(), "a mismatch attaches nothing");
}

/// README.md:1262-1265 -- a file the cache never saw, or a function the cache
/// has no history for, is a difference too, and is named.
#[test]
fn d13_reattaching_names_a_file_or_function_the_cache_does_not_have() {
    need_git!();
    let mut r = Repo::new();
    let two = "int f1(int x) {\n    return x + 1;\n}\n\nint f2(int x) {\n    return x + 2;\n}\n";
    r.write("a.c", two);
    r.write("c.c", "int h(void) {\n    return 0;\n}\n");
    r.commit("ABC-1: files");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let (cache, _) = change::analyse(&mut ev, &opts(None), None).unwrap();
    // same text of a.c, but now the evidence holds f2 as well
    let mut more = ev_disk(&r, &[("a.c", &[("f1", 1, 3), ("f2", 5, 7)])]);
    let err = change::embed(&mut more, &cache, None).unwrap_err();
    assert!(err.contains("f2"), "names the function with no cached history: {err}");
    // a file the cache never saw
    let mut other = ev_disk(&r, &[("a.c", &[("f1", 1, 3)]), ("c.c", &[("h", 1, 3)])]);
    let err = change::embed(&mut other, &cache, None).unwrap_err();
    assert!(err.contains("c.c"), "names the file the cache never saw: {err}");
}

/// README.md:1289 -- "A corrupt or pre-upgrade cache is ignored ... and
/// rebuilt": a prior cache of another schema is not reused.
#[test]
fn d14_a_pre_upgrade_cache_is_not_reused() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.commit("ABC-1: a");
    let mut ev = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let (mut cache, _) = change::analyse(&mut ev, &opts(None), None).unwrap();
    cache.schema = "butterfly-changes/0".into();
    let mut ev2 = ev_disk(&r, &[("a.c", &[("f1", 1, 3)])]);
    let (_, out) = change::analyse(&mut ev2, &opts(None), Some(&cache)).unwrap();
    assert_eq!((out.walked, out.reused), (1, 0), "an old-schema cache is rebuilt, not trusted");
    assert_eq!(ticket(&ev2, "ABC-1").functions, vec![0]);
    // and garbage does not parse as a cache at all
    assert!(change::parse("{ not json", Path::new("butterfly-changes.json")).is_err());
}

/// README.md:1262-1266 -- "A re-run walks only functions in files that
/// changed since the cached HEAD"; the commits a reused function refers to
/// are still reported.
#[test]
fn d15_a_rerun_reuses_unchanged_files_and_still_reports_their_commits() {
    need_git!();
    let mut r = Repo::new();
    r.write("a.c", F1);
    r.write("b.c", "int g(int x) {\n    return x;\n}\n");
    r.commit("ABC-1: a and b");
    let spec: &[FileSpec] = &[("a.c", &[("f1", 1, 3)]), ("b.c", &[("g", 1, 3)])];
    let mut ev = ev_disk(&r, spec);
    let (cache, _) = change::analyse(&mut ev, &opts(None), None).unwrap();
    r.write("b.c", "int g(int x) {\n    return -x;\n}\n");
    r.commit("ABC-2: negate g");
    let mut ev2 = ev_disk(&r, spec);
    let (_, out) = change::analyse(&mut ev2, &opts(None), Some(&cache)).unwrap();
    assert_eq!((out.walked, out.reused), (1, 1), "only b.c is walked again");
    assert_eq!(ticket(&ev2, "ABC-1").functions, vec![0, 1], "the reused f1 keeps its commit");
    assert_eq!(ticket(&ev2, "ABC-2").functions, vec![1]);
}
