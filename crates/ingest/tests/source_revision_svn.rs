// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The project's source revision for an SVN working copy, read at ingest
//! (#301).
//!
//! Every case builds a real repository with `svnadmin create` and checks it
//! out over `file://` into a temporary directory, so no server is needed.
//! Where Subversion is not installed the suite is a named skip -- a failure
//! wherever the fixtures are required (#227), since CI installs it.
//!
//! SVN's base checksums are of each file's normal form (keywords unexpanded,
//! LF line endings), so a file with `svn:keywords`, `svn:eol-style` or
//! `svn:special` is judged by `svn status`; every other file the build used
//! is compared with its base checksum by content. The controls show plain
//! `svn status` missing an edit that the hash comparison finds.

use butterfly_ingest::helper::Bounds;
use butterfly_ingest::revision::{inspect, BuildFile, Request};
use butterfly_model::revision::{Change, SourceRevision, SourceState, Vcs, LIST_CAP};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

// ---- scaffolding --------------------------------------------------------

const NEEDS: &str = "Subversion (svn, svnadmin, svnversion) is not installed: apt-get install subversion, or brew install subversion";

fn scratch(tag: &str) -> PathBuf {
    let d = butterfly_testsupport::scratch_dir(&format!("svnrev-{tag}"));
    d.canonicalize().unwrap()
}

fn tool(name: &str) -> Option<PathBuf> {
    butterfly_ingest::provenance::resolve_on_path(name)
}

/// Subversion is here, or the suite says so and -- where fixtures are
/// required -- fails.
fn have_svn() -> bool {
    if ["svn", "svnadmin", "svnversion"].iter().all(|t| tool(t).is_some()) {
        return true;
    }
    butterfly_testsupport::fixture_missing(NEEDS);
    false
}

/// A repository and a working copy of its trunk.
struct Fx {
    /// The repository's root URL.
    url: String,
    /// The working copy.
    wc: PathBuf,
    /// Where the fixture's own svn keeps its configuration.
    home: PathBuf,
}

/// Run a Subversion program for the test's own setup, isolated from the
/// operator's configuration.
fn prog(name: &str, home: &Path, dir: &Path, args: &[&str]) -> String {
    let mut c = Command::new(tool(name).unwrap());
    c.current_dir(dir).env_clear().env("PATH", std::env::var_os("PATH").unwrap_or_default()).env("HOME", home);
    for k in ["LANG", "LC_ALL", "LC_CTYPE"] {
        if let Some(v) = std::env::var_os(k) {
            c.env(k, v);
        }
    }
    if name == "svn" {
        c.args(["--non-interactive", "--config-dir"]).arg(home.join(".subversion"));
    }
    let out = c.args(args).output().unwrap();
    assert!(out.status.success(), "{name} {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

impl Fx {
    fn svn(&self, args: &[&str]) -> String {
        prog("svn", &self.home, &self.wc, args)
    }

    /// `svnversion` of the working copy, as the control.
    fn svnversion(&self) -> String {
        prog("svnversion", &self.home, &self.wc, &["."])
    }

    fn uuid(&self) -> String {
        self.svn(&["info", "--show-item", "repos-uuid", "."])
    }
}

fn write(dir: &Path, rel: &str, text: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

/// A repository with `trunk`, `branches` and `lib`, `files` committed to
/// trunk, and a working copy of trunk at its latest revision. `None` when
/// Subversion is not installed.
fn repo_in(base: &Path, files: &[(&str, &str)]) -> Option<Fx> {
    if !have_svn() {
        return None;
    }
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    prog("svnadmin", &home, base, &["create", "repo"]);
    let url = format!("file://{}", base.join("repo").display());
    prog("svn", &home, base, &["mkdir", "-q", "-m", "layout", &format!("{url}/trunk"), &format!("{url}/branches"), &format!("{url}/lib")]);
    let wc = base.join("wc");
    prog("svn", &home, base, &["checkout", "-q", &format!("{url}/trunk"), wc.to_str().unwrap()]);
    let fx = Fx { url, wc: wc.canonicalize().unwrap(), home };
    for (p, t) in files {
        write(&fx.wc, p, t);
    }
    if !files.is_empty() {
        fx.svn(&["add", "-q", "--force", "."]);
        fx.svn(&["commit", "-q", "-m", "one"]);
        fx.svn(&["update", "-q"]);
    }
    Some(fx)
}

fn repo_with(tag: &str, files: &[(&str, &str)]) -> Option<Fx> {
    repo_in(&scratch(tag), files)
}

fn standard() -> Option<Fx> {
    repo_with("std", &[("src/a.c", "int a(void) {\n  return 1;\n}\n"), ("src/inc.h", "#define X 1\n"), ("README", "readme\n")])
}

/// Inspect as ingest does: `read` files carry the bytes ingest read (the
/// coverage file list); `dwarf` files are named only, and the layer reads
/// them itself.
fn inspect_req(start: &Path, read: &[PathBuf], dwarf: &[PathBuf], tweak: impl FnOnce(&mut Request)) -> SourceRevision {
    let bytes: Vec<Vec<u8>> = read.iter().map(|p| fs::read(p).unwrap()).collect();
    let mut files: Vec<BuildFile> = read.iter().zip(&bytes).map(|(p, b)| BuildFile { path: p.clone(), bytes: Some(b.as_slice()) }).collect();
    files.extend(dwarf.iter().map(|p| BuildFile { path: p.clone(), bytes: None }));
    let mut req = Request::new(start.to_path_buf(), files);
    tweak(&mut req);
    let r = inspect(&req);
    // every record the layer produces must pass the model's own check
    r.check().unwrap_or_else(|e| panic!("{e}: {r:#?}"));
    r
}

fn run(start: &Path, read: &[PathBuf], dwarf: &[PathBuf]) -> SourceRevision {
    inspect_req(start, read, dwarf, |_| {})
}

fn changes(r: &SourceRevision) -> Vec<(String, Change)> {
    r.modified.iter().map(|c| (c.path.clone(), c.change)).collect()
}

fn unknown_reason(r: &SourceRevision) -> String {
    match &r.state {
        SourceState::Unknown(why) => why.clone(),
        other => panic!("expected unknown, got {other:?}: {r:#?}"),
    }
}

/// svnversion's revision without its flags: `5`, `3:5`.
fn range_of(v: &str) -> String {
    v.trim_end_matches(|c: char| c.is_ascii_alphabetic()).to_string()
}

fn caveat(r: &SourceRevision) -> String {
    butterfly_ingest::revision::caveat(r).unwrap_or_default()
}

// ---- normal ---------------------------------------------------------------

#[test]
fn a_clean_checkout_is_clean_and_names_its_revision_repository_and_path() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    let r = run(d, &[d.join("src/a.c")], &[d.join("src/inc.h")]);
    assert_eq!(r.vcs, Vcs::Svn);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    let id = r.id.as_ref().expect("a revision");
    assert_eq!(id.revision, fx.svnversion(), "svnversion's revision");
    assert_eq!(id.repository_uuid.as_deref(), Some(fx.uuid().as_str()));
    assert_eq!(id.path.as_deref(), Some("^/trunk"));
    assert_eq!(r.repository.as_deref(), Some(fx.url.as_str()));
    assert_eq!(r.root.as_deref(), Some(d.to_str().unwrap()));
    assert_eq!(r.verified_count, 2);
    assert_eq!(r.modified_count, 0);
    assert_eq!(r.outside_build, Some(0));
    let svn = r.svn.as_ref().expect("the SVN detail");
    assert_eq!(svn.hash_verified, 2, "neither file has a property that stops the hash comparison");
    assert_eq!(svn.status_only, 0);
    assert!(!svn.locally_modified && svn.switched.is_empty() && svn.sparse.is_empty() && svn.externals.is_empty(), "{svn:#?}");
    let tool = r.tool.as_ref().expect("the svn that ran is recorded");
    assert!(tool.name == "svn" && tool.version.contains("version"), "{tool:?}");
    assert!(svn.svnversion_tool.as_ref().is_some_and(|t| t.name == "svnversion"), "{svn:?}");
    // a clean single-revision working copy has nothing to say but what its
    // checksums are (#315)
    assert!(caveat(&r).contains("working-copy metadata") && !caveat(&r).contains("mixed"), "{}", caveat(&r));
}

#[test]
fn one_edited_source_makes_it_modified() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    write(d, "src/a.c", "int a(void) {\n  return 2;\n}\n");
    let r = run(d, &[d.join("src/a.c")], &[d.join("src/inc.h")]);
    assert_eq!(r.state, SourceState::Modified);
    assert_eq!(changes(&r), vec![("src/a.c".to_string(), Change::Content)]);
    assert_eq!(r.id.as_ref().unwrap().revision, range_of(&fx.svnversion()), "the revision is still named: it is the label");
    assert!(r.svn.as_ref().unwrap().locally_modified, "svnversion's M");
    assert!(caveat(&r).contains("src/a.c"), "{}", caveat(&r));
}

// ---- equivalence classes ----------------------------------------------------

#[test]
fn a_file_added_but_not_committed_is_not_in_the_revision() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    write(d, "src/new.c", "int n;\n");
    fx.svn(&["add", "-q", "src/new.c"]);
    let r = run(d, &[d.join("src/a.c"), d.join("src/new.c")], &[]);
    assert_eq!(r.state, SourceState::Modified);
    assert_eq!(changes(&r), vec![("src/new.c".to_string(), Change::NotInRevision)]);
}

#[test]
fn a_deleted_header_the_build_used_is_missing_and_a_deleted_unused_file_is_counted_outside() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    fx.svn(&["rm", "-q", "src/inc.h"]);
    fs::remove_file(d.join("README")).unwrap();
    let r = run(d, &[d.join("src/a.c")], &[d.join("src/inc.h")]);
    assert_eq!(r.state, SourceState::Modified);
    assert_eq!(changes(&r), vec![("src/inc.h".to_string(), Change::Missing)]);
    assert_eq!(r.outside_build, Some(1), "README, missing, which the build does not use");
}

#[test]
fn an_unversioned_file_counts_only_if_the_build_used_it() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    // generated and even ignored: compiled in, so it counts
    fx.svn(&["propset", "-q", "svn:ignore", "gen", "."]);
    fx.svn(&["commit", "-q", "-m", "ignore gen"]);
    fx.svn(&["update", "-q"]);
    write(d, "gen/config.h", "#define GENERATED 1\n");
    write(d, "notes.txt", "scratch\n");
    let r = run(d, &[d.join("src/a.c")], &[d.join("gen/config.h")]);
    assert_eq!(r.state, SourceState::Modified, "{r:#?}");
    assert_eq!(changes(&r), vec![("gen/config.h".to_string(), Change::NotInRevision)]);
    assert_eq!(r.outside_build, Some(1), "notes.txt is unversioned but unused");

    let r = run(d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "an unversioned file the build did not use leaves it clean");
    assert_eq!(r.outside_build, Some(1), "notes.txt; the ignored generated header is a build product, not a change");
}

#[test]
fn a_mixed_revision_working_copy_is_recorded_as_its_range_with_a_caveat() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    write(d, "src/c.c", "int c;\n");
    fx.svn(&["add", "-q", "src/c.c"]);
    fx.svn(&["commit", "-q", "-m", "c"]);
    // no update: src/c.c is at the new revision, the rest at the old one
    let v = fx.svnversion();
    assert!(v.contains(':'), "the control: svnversion reports a range, {v}");
    let r = run(d, &[d.join("src/a.c"), d.join("src/c.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "each file is its own revision's: {r:#?}");
    assert_eq!(r.id.as_ref().unwrap().revision, range_of(&v), "recorded as the range");
    let svn = r.svn.as_ref().unwrap();
    assert!(svn.low < svn.high, "{svn:?}");
    let c = caveat(&r);
    assert!(c.contains("mixed") && c.contains(&v), "a mixed revision is a caveat even when clean: {c}");
}

#[test]
fn a_switched_directory_is_recorded_and_named() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    fx.svn(&["copy", "-q", "-m", "branch", &format!("{}/trunk", fx.url), &format!("{}/branches/b1", fx.url)]);
    fx.svn(&["switch", "-q", "--ignore-ancestry", &format!("{}/branches/b1/src", fx.url), "src"]);
    let v = fx.svnversion();
    assert!(v.contains('S'), "the control: svnversion flags it, {v}");
    let r = run(d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "its files are its branch's: {r:#?}");
    let svn = r.svn.as_ref().unwrap();
    assert_eq!(svn.switched, vec!["src".to_string()]);
    assert_eq!(svn.switched_count, 1);
    assert_eq!(svn.svnversion, v);
    let c = caveat(&r);
    assert!(c.contains("switched") && c.contains("src"), "{c}");
}

#[test]
fn a_sparse_checkout_is_recorded_and_named() {
    let Some(fx) = repo_with("sparse", &[("src/a.c", "int a;\n"), ("docs/deep/x.md", "x\n")]) else { return };
    let d = &fx.wc;
    fx.svn(&["update", "-q", "--set-depth", "empty", "docs"]);
    let v = fx.svnversion();
    assert!(v.contains('P'), "the control: svnversion flags it, {v}");
    let r = run(d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    let svn = r.svn.as_ref().unwrap();
    assert_eq!(svn.sparse, vec!["docs (empty)".to_string()]);
    assert!(caveat(&r).contains("sparse") && caveat(&r).contains("docs"), "{}", caveat(&r));
}

#[test]
fn an_external_is_judged_as_its_own_working_copy() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    // the external's own content, in the same repository's ^/lib
    let lib = d.parent().unwrap().join("libwc");
    prog("svn", &fx.home, d, &["checkout", "-q", &format!("{}/lib", fx.url), lib.to_str().unwrap()]);
    write(&lib, "lib.c", "int lib(void) { return 1; }\n");
    prog("svn", &fx.home, &lib, &["add", "-q", "lib.c"]);
    prog("svn", &fx.home, &lib, &["commit", "-q", "-m", "lib"]);
    fx.svn(&["propset", "-q", "svn:externals", "^/lib ext", "."]);
    fx.svn(&["commit", "-q", "-m", "external"]);
    fx.svn(&["update", "-q"]);
    assert!(d.join("ext/.svn").exists(), "the control: the external is its own working copy");

    let r = run(d, &[d.join("src/a.c"), d.join("ext/lib.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.verified_count, 2);
    let svn = r.svn.as_ref().unwrap();
    assert_eq!(svn.externals.len(), 1, "{svn:#?}");
    let ext = &svn.externals[0];
    assert_eq!(ext.path, "ext");
    let id = ext.id.as_ref().expect("the external's own revision");
    assert_eq!(id.path.as_deref(), Some("^/lib"));
    assert_eq!(id.repository_uuid.as_deref(), Some(fx.uuid().as_str()));
    assert_eq!(ext.svnversion.as_deref(), Some(prog("svnversion", &fx.home, &d.join("ext"), &["."]).as_str()));

    // an edit inside the external is judged against the external's base
    write(d, "ext/lib.c", "int lib(void) { return 2; }\n");
    let r = run(d, &[d.join("src/a.c"), d.join("ext/lib.c")], &[]);
    assert_eq!(r.state, SourceState::Modified);
    assert_eq!(changes(&r), vec![("ext/lib.c".to_string(), Change::Content)]);
    assert!(!r.svn.as_ref().unwrap().locally_modified, "the parent's svnversion does not look into externals");
}

#[test]
fn a_change_only_outside_the_build_is_clean_and_counted_separately() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    write(d, "README", "edited\n");
    let r = run(d, &[d.join("src/a.c")], &[d.join("src/inc.h")]);
    assert_eq!(r.state, SourceState::Clean);
    assert_eq!(r.outside_build, Some(1));
    assert!(r.modified.is_empty());
    assert!(r.svn.as_ref().unwrap().locally_modified, "svnversion's M is the whole working copy's, not the build's");
}

/// `svn status` trusts a file whose size and timestamp match what it
/// recorded, and does not read it. An edit of the same length with the
/// timestamp put back is invisible to it; the hash comparison is not fooled.
#[test]
fn an_edit_svn_status_cannot_see_is_found_by_the_hash() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    let p = d.join("src/a.c");
    let when = fs::metadata(&p).unwrap().modified().unwrap();
    write(d, "src/a.c", "int a(void) {\n  return 7;\n}\n");
    fs::File::options().write(true).open(&p).unwrap().set_modified(when).unwrap();
    assert_eq!(fx.svn(&["status", "src/a.c"]), "", "the control: svn status is fooled");
    let r = run(d, std::slice::from_ref(&p), &[]);
    assert_eq!(changes(&r), vec![("src/a.c".to_string(), Change::Content)], "butterfly compares content: {r:#?}");
}

// ---- boundary -------------------------------------------------------------

#[test]
fn revision_zero_is_unknown_but_named() {
    let base = scratch("r0");
    let Some(fx) = repo_in(&base, &[]) else { return };
    // a checkout of the repository before anything was committed
    let wc0 = base.join("wc0");
    prog("svn", &fx.home, &base, &["checkout", "-q", &format!("{}@0", fx.url), wc0.to_str().unwrap()]);
    let wc0 = wc0.canonicalize().unwrap();
    assert_eq!(prog("svnversion", &fx.home, &wc0, &["."]), "0", "the control");
    write(&wc0, "a.c", "int a;\n");
    prog("svn", &fx.home, &wc0, &["add", "-q", "a.c"]);
    let r = run(&wc0, &[wc0.join("a.c")], &[]);
    assert!(unknown_reason(&r).contains("r0"), "{r:#?}");
    let id = r.id.as_ref().expect("r0 is still named");
    assert_eq!(id.revision, "0");
    assert!(id.repository_uuid.is_some());
}

#[test]
fn keyword_and_eol_style_properties_on_an_unmodified_file_leave_it_clean() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    write(d, "src/k.c", "/* $Id$ */\nint k;\n");
    write(d, "src/e.c", "int e;\nint f;\n");
    fx.svn(&["add", "-q", "src/k.c", "src/e.c"]);
    fx.svn(&["propset", "-q", "svn:keywords", "Id", "src/k.c"]);
    fx.svn(&["propset", "-q", "svn:eol-style", "CRLF", "src/e.c"]);
    fx.svn(&["commit", "-q", "-m", "props"]);
    fx.svn(&["update", "-q"]);
    // the control: the working files differ from their base copies' form
    assert!(fs::read_to_string(d.join("src/k.c")).unwrap().contains("$Id: k.c "), "the keyword is expanded");
    assert!(fs::read(d.join("src/e.c")).unwrap().windows(2).any(|w| w == b"\r\n"), "the line endings are CRLF");

    let files = [d.join("src/a.c"), d.join("src/k.c"), d.join("src/e.c")];
    let r = run(d, &files, &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    let svn = r.svn.as_ref().unwrap();
    // e.c's only translating property is svn:eol-style: it is compared by
    // its normal form, LF line endings (#315); k.c's keyword is svn status's
    assert_eq!((svn.hash_verified, svn.status_only), (2, 1), "{svn:?}");
    assert!(r.notes.iter().any(|n| n.contains("svn status") && n.contains("svn:keywords")), "the record says how they were judged: {:?}", r.notes);

    // and an edit to one is still seen, by svn status
    write(d, "src/k.c", "/* $Id$ */\nint k = 2;\n");
    let r = run(d, &files, &[]);
    assert_eq!(changes(&r), vec![("src/k.c".to_string(), Change::Content)]);
}

#[test]
fn awkward_paths_are_matched_exactly() {
    // svn refuses control characters in names, so no newline here; the rest
    // are the ones that trip a parser: spaces, non-ASCII, XML's specials, a
    // peg-revision @, a leading dash -- and a working copy whose own path
    // has a space and an @
    let names = ["has space.c", "ünïcødé.c", "a&b<c>'\".c", "at@sign.c", "-dash.c", "dir with space/x.c"];
    let base = scratch("paths").join("w c@1");
    fs::create_dir_all(&base).unwrap();
    let Some(fx) = repo_in(&base, &[]) else { return };
    let d = &fx.wc;
    for n in names {
        write(d, n, "int x;\n");
    }
    fx.svn(&["add", "-q", "--force", "."]);
    fx.svn(&["commit", "-q", "-m", "paths"]);
    fx.svn(&["update", "-q"]);
    let paths: Vec<PathBuf> = names.iter().map(|n| d.join(n)).collect();
    let r = run(d, &paths, &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.verified_count, names.len() as u64);
    assert_eq!(r.svn.as_ref().unwrap().hash_verified, names.len() as u64);
    for n in ["a&b<c>'\".c", "ünïcødé.c", "at@sign.c"] {
        write(d, n, "int y;\n");
    }
    let r = run(d, &paths, &[]);
    // sorted by path, byte-wise
    let want: Vec<(String, Change)> = ["a&b<c>'\".c", "at@sign.c", "ünïcødé.c"].iter().map(|n| (n.to_string(), Change::Content)).collect();
    assert_eq!(changes(&r), want);
}

#[test]
fn thousands_of_modified_files_are_capped_in_the_list_and_counted_exactly() {
    let n = 3000;
    let files: Vec<(String, String)> = (0..n).map(|i| (format!("src/f{i:04}.c"), format!("int f{i};\n"))).collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    let Some(fx) = repo_with("many", &refs) else { return };
    let d = &fx.wc;
    for i in 0..n {
        write(d, &format!("src/f{i:04}.c"), format!("int f{i} = 1;\n"));
    }
    let paths: Vec<PathBuf> = (0..n).map(|i| d.join(format!("src/f{i:04}.c"))).collect();
    let r = run(d, &paths, &[]);
    assert_eq!(r.state, SourceState::Modified);
    assert_eq!(r.modified_count, n as u64, "exact");
    assert_eq!(r.modified.len(), LIST_CAP, "capped");
    assert_eq!(r.modified[0].path, "src/f0000.c", "sorted");
}

#[test]
fn a_source_root_in_a_subdirectory_finds_the_working_copy_above_it() {
    let Some(fx) = repo_with("subdir", &[("fw/src/a.c", "int a;\n"), ("docs/x.md", "x\n")]) else { return };
    let d = &fx.wc;
    let r = run(&d.join("fw"), &[d.join("fw/src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert_eq!(r.root.as_deref(), Some(d.to_str().unwrap()));
    write(d, "fw/src/a.c", "int a = 1;\n");
    let r = run(&d.join("fw"), &[d.join("fw/src/a.c")], &[]);
    assert_eq!(changes(&r), vec![("fw/src/a.c".to_string(), Change::Content)], "paths are relative to the working copy's top");
}

// ---- robustness -----------------------------------------------------------

#[test]
fn no_svn_on_path_is_unknown() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    let empty = scratch("nopath");
    let r = inspect_req(d, &[d.join("src/a.c")], &[], |req| req.resolve_tools_in(empty.as_os_str()));
    assert_eq!(r.vcs, Vcs::Svn);
    assert!(unknown_reason(&r).contains("svn was not found"), "{r:#?}");
}

/// The working copy an interrupted svn operation leaves behind: a real
/// `svn commit` is killed while it waits for its log message, and its
/// working-copy lock stays until `svn cleanup`.
#[test]
fn a_locked_working_copy_is_unknown_and_says_to_run_cleanup() {
    use std::os::unix::fs::PermissionsExt;
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    // something to commit that is not a file the build used
    fx.svn(&["propset", "-q", "note", "x", "README"]);
    let tools = scratch("lock-tools");
    let marker = tools.join("editing");
    let editor = tools.join("editor");
    fs::write(&editor, format!("#!/bin/sh\necho $$ > '{}'\nexec sleep 60\n", marker.display())).unwrap();
    fs::set_permissions(&editor, fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = Command::new(tool("svn").unwrap())
        .current_dir(d)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &fx.home)
        .env("SVN_EDITOR", &editor)
        .args(["commit", "--force-interactive", "--config-dir"])
        .arg(fx.home.join(".subversion"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let t = Instant::now();
    while !marker.exists() || fs::read_to_string(&marker).unwrap().trim().is_empty() {
        assert!(t.elapsed() < Duration::from_secs(30), "the commit never asked for its log message");
        std::thread::sleep(Duration::from_millis(50));
    }
    child.kill().unwrap();
    let _ = child.wait();
    let _ = Command::new("kill").arg("-9").arg(fs::read_to_string(&marker).unwrap().trim()).status();
    assert!(fx.svn(&["status"]).lines().any(|l| l.as_bytes().get(2) == Some(&b'L')), "the control: svn status shows the lock");

    let r = run(d, &[d.join("src/a.c")], &[]);
    let why = unknown_reason(&r);
    assert!(why.contains("locked") && why.contains("svn cleanup"), "{why}");
    assert!(r.id.is_some(), "the revision is still named");
    // and a real change still reads modified, with the lock noted
    write(d, "src/a.c", "int a(void) {\n  return 3;\n}\n");
    let r = run(d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Modified);
    assert!(r.notes.iter().any(|n| n.contains("svn cleanup")), "{:?}", r.notes);
}

/// A working copy written by a newer client. Simulated: the working-copy
/// format is SQLite's `user_version` in `.svn/wc.db` -- four big-endian
/// bytes at offset 60 of the database header -- and it is set past any
/// format this client knows, so the real client gives its real refusal.
#[test]
fn a_working_copy_from_a_newer_client_is_unknown() {
    use std::io::{Seek, SeekFrom, Write};
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    let mut db = fs::File::options().write(true).open(d.join(".svn/wc.db")).unwrap();
    db.seek(SeekFrom::Start(60)).unwrap();
    db.write_all(&999u32.to_be_bytes()).unwrap();
    drop(db);
    let out = Command::new(tool("svn").unwrap()).args(["info", "--non-interactive"]).arg(d).env("HOME", &fx.home).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("E155021"), "the control: svn refuses it, {}", String::from_utf8_lossy(&out.stderr));

    let r = run(d, &[d.join("src/a.c")], &[]);
    assert!(unknown_reason(&r).contains("newer"), "{r:#?}");
}

#[test]
fn an_unversioned_source_root_is_unknown() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    write(d, "unadded/a.c", "int a;\n");
    let r = run(&d.join("unadded"), &[d.join("unadded/a.c")], &[]);
    assert_eq!(r.vcs, Vcs::Svn);
    assert!(unknown_reason(&r).contains("not under version control"), "{r:#?}");

    // and a .svn that is not a working copy at all
    let fake = scratch("fake");
    fs::create_dir_all(fake.join(".svn")).unwrap();
    write(&fake, "a.c", "int a;\n");
    let r = run(&fake, &[fake.join("a.c")], &[]);
    assert!(unknown_reason(&r).contains("not a working copy"), "{r:#?}");
}

fn stub(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    fs::write(&p, format!("#!/bin/sh\ncase \"$*\" in *--version*) echo '{name}, version 9.9.9'; exit 0;; esac\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[test]
fn a_hanging_svn_is_stopped_and_unknown() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    let tools = scratch("hang");
    let (svn, svnversion) = (stub(&tools, "svn", "sleep 30"), stub(&tools, "svnversion", "sleep 30"));
    let t = Instant::now();
    let r = inspect_req(d, &[d.join("src/a.c")], &[], |req| {
        req.svn = Some(svn);
        req.svnversion = Some(svnversion);
        req.bounds = Bounds::standard().with_deadline(Duration::from_millis(500));
    });
    assert!(t.elapsed() < Duration::from_secs(10), "bounded: {:?}", t.elapsed());
    assert!(unknown_reason(&r).contains("did not finish"), "{r:#?}");
}

/// Every svn call is local and non-interactive, with the environment
/// cleared: a stub in front of the real programs logs what it was given.
#[test]
fn svn_runs_non_interactive_offline_and_with_a_cleared_environment() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    let tools = scratch("log");
    let log = tools.join("log");
    let wrap = |name: &str| {
        use std::os::unix::fs::PermissionsExt;
        let p = tools.join(name);
        fs::write(&p, format!("#!/bin/sh\n{{ printf '%s' '{name}'; for a in \"$@\"; do printf ' [%s]' \"$a\"; done; echo; env | sed 's/=.*//' | sort | tr '\\n' ' ' | sed 's/^/  env: /'; echo; }} >> '{}'\nexec '{}' \"$@\"\n", log.display(), tool(name).unwrap().display())).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    };
    let (svn, svnversion) = (wrap("svn"), wrap("svnversion"));
    let r = inspect_req(d, &[d.join("src/a.c")], &[], |req| {
        req.svn = Some(svn);
        req.svnversion = Some(svnversion);
    });
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    let text = fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let calls: Vec<&str> = lines.iter().copied().filter(|l| l.starts_with("svn")).collect();
    assert!(calls.iter().any(|l| l.starts_with("svn [status]") || l.contains("[status]")), "{text}");
    for c in calls.iter().filter(|l| l.starts_with("svn ") && !l.contains("[--version]")) {
        assert!(c.contains("[--non-interactive]"), "{c}");
        assert!(!c.contains("[-u]") && !c.contains("[--show-updates]"), "never the network: {c}");
    }
    let allowed = [
        "PATH",
        "HOME",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "LC_MESSAGES",
        "TZ",
        "USER",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "CARGO_HOME",
        "SDKROOT",
        "DEVELOPER_DIR",
        "PWD",
        "SHLVL",
        "_",
        "OLDPWD",
    ];
    for l in lines.iter().filter(|l| l.starts_with("  env:")) {
        for k in l.trim_start_matches("  env:").split_whitespace() {
            assert!(allowed.contains(&k), "{k} reached svn: {l}");
        }
    }
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
fn ingest_records_an_svn_revision_and_a_caveat_when_it_is_not_clean() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    let out = scratch("ingest-out");
    let a = d.join("src/a.c");
    fs::write(out.join("coverage.json"), export(&[a.to_str().unwrap()])).unwrap();
    let opts = butterfly_ingest::IngestOptions { coverage_json: out.join("coverage.json"), source_root: Some(d.clone()), ..Default::default() };

    let ev = butterfly_ingest::ingest(&opts).unwrap();
    let r = ev.provenance.source_revision.as_ref().expect("recorded");
    assert_eq!(r.vcs, Vcs::Svn);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    assert!(!ev.provenance.caveats.iter().any(|c| c.contains("source revision")), "{:?}", ev.provenance.caveats);
    assert!(ev.validate().is_ok());
    for t in ["svn", "svnversion"] {
        assert!(ev.provenance.external_tools.iter().any(|x| x.name == t), "{t} ran, so it is identified: {:?}", ev.provenance.external_tools);
    }

    write(d, "src/a.c", "int a(void) {\n  return 6;\n}\n");
    let ev = butterfly_ingest::ingest(&opts).unwrap();
    let r = ev.provenance.source_revision.as_ref().unwrap();
    assert_eq!(r.state, SourceState::Modified);
    assert!(ev.provenance.caveats.iter().any(|c| c.contains("source revision") && c.contains("modified") && c.contains("src/a.c")), "{:?}", ev.provenance.caveats);
    // not a failure: ingest succeeded and the evidence loads
    assert!(ev.validate().is_ok());
}

// ---- what a clean SVN record rests on (#315) -----------------------------

/// `svn:eol-style` is on every file in many repositories (auto-props). Judged
/// by `svn status` alone, which trusts size and timestamp, a same-size edit
/// with its timestamp put back read clean. Its normal form is known -- LF
/// line endings -- so it is compared by hash like any other file (#315).
#[test]
fn an_eol_style_file_is_compared_by_its_normal_form_not_by_svn_status() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    write(d, "src/e.c", "int limit = 10;\n");
    fx.svn(&["add", "-q", "src/e.c"]);
    fx.svn(&["propset", "-q", "svn:eol-style", "native", "src/e.c"]);
    fx.svn(&["commit", "-q", "-m", "eol"]);
    fx.svn(&["update", "-q"]);
    let r = run(d, &[d.join("src/e.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "unmodified, it verifies: {r:#?}");
    assert_eq!(r.svn.as_ref().map(|s| (s.hash_verified, s.status_only)), Some((1, 0)));

    let when = fs::metadata(d.join("src/e.c")).unwrap().modified().unwrap();
    write(d, "src/e.c", "int limit = 99;\n");
    fs::File::options().write(true).open(d.join("src/e.c")).unwrap().set_modified(when).unwrap();
    assert!(!fx.svnversion().ends_with('M'), "the control: svn does not see a same-size edit with its timestamp put back");
    let r = run(d, &[d.join("src/e.c")], &[]);
    assert_eq!(changes(&r), vec![("src/e.c".to_string(), Change::Content)], "{r:#?}");
}

/// The base checksums are the working copy's own record (`.svn/wc.db`):
/// whoever can edit a file can edit that too, and nothing checks it against
/// the repository. A clean SVN record says so, and says when some files were
/// judged by `svn status` alone (#315).
#[test]
fn a_clean_svn_record_says_what_its_checksums_and_status_rest_on() {
    let Some(fx) = standard() else { return };
    let d = &fx.wc;
    let r = run(d, &[d.join("src/a.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    let c = caveat(&r);
    assert!(c.contains("working-copy metadata"), "{c:?}");
    assert!(!c.contains("svn status alone"), "every file here was compared by hash: {c:?}");

    write(d, "src/k.c", "/* $Id$ */\nint k;\n");
    fx.svn(&["add", "-q", "src/k.c"]);
    fx.svn(&["propset", "-q", "svn:keywords", "Id", "src/k.c"]);
    fx.svn(&["commit", "-q", "-m", "kw"]);
    fx.svn(&["update", "-q"]);
    let r = run(d, &[d.join("src/a.c"), d.join("src/k.c")], &[]);
    assert_eq!(r.state, SourceState::Clean, "{r:#?}");
    let c = caveat(&r);
    assert!(c.contains("svn status alone") && c.contains("timestamp"), "{c:?}");
    // the checksum caveat counts only the files compared by hash
    assert!(c.contains("1 file(s) the build used were compared with the base checksums"), "{c:?}");
}
