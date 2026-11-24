// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Reading an SVN working copy's revision at ingest (#301).
//!
//! The revision is `svnversion`'s -- a number, or a `low:high` range in a
//! mixed-revision working copy -- with the repository's UUID and the working
//! copy's `^/` path from `svn info`, since a revision number alone names
//! neither the repository nor the branch.
//!
//! **The revision is a label; the file hashes are the proof, where SVN
//! allows it.** SVN keeps a SHA-1 of each file's base copy, but of its
//! *normal form*: `svn:keywords` unexpanded, `svn:eol-style` line endings as
//! LF, an `svn:special` link as its `link` text. For a file with none of
//! those properties the working file must equal the base copy byte for byte,
//! so it is compared with that checksum by the bytes ingest read -- which
//! `svn status` cannot be talked out of the way it can be by a timestamp put
//! back after an edit. A file whose only such property is `svn:eol-style`
//! is compared the same way, by its normal form: its line endings as LF
//! (#315). A file with `svn:keywords` or `svn:special` legitimately differs
//! from its base copy in a way this does not reproduce, so it is judged by
//! `svn status` alone -- which trusts size and timestamp -- and the record
//! counts which files were judged which way, and says so in its caveat.
//!
//! The checksums themselves are the working copy's own record
//! (`.svn/wc.db`), not the repository's: nothing here asks the repository,
//! and whoever can edit a file can edit its base checksum too. Unlike a git
//! commit id, then, a clean SVN record rests on the working copy's account
//! of its revision, and its caveat says that (#315).
//!
//! Every call is local and non-interactive: `svnversion`, and `svn info`,
//! `proplist` and `status` with `--non-interactive` and never `-u`, so the
//! repository is never contacted. Each runs through the bounded helper
//! policy (#246) with the environment cleared, inside the working copy with
//! `.` as its target, so no path of ours is ever parsed by svn as a peg
//! revision (`@`) or an option (`-`), and paths come back relative, in XML.
//!
//! Externals, and any working copy checked out inside this one, are judged
//! as their own working copies: the parent revision does not determine
//! their content, so each records its own.

use super::{crlf_to_lf, lf_to_crlf, read_file, strip_credentials, Placed, Request};
use crate::helper::{self, Bounds, Extra, HelperError};
use butterfly_model::revision::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// Nested externals deeper than this are not followed.
const MAX_DEPTH: usize = 8;

/// The working-copy states that are a change, for counting changes outside
/// the build. A change to properties alone is not: the build read bytes.
const CHANGED: &[&str] = &["modified", "added", "deleted", "missing", "replaced", "conflicted", "obstructed", "unversioned", "incomplete"];

/// The properties that make a working file differ from its base copy's
/// normal form, so the base checksum proves nothing about it.
const TRANSLATING: &[&str] = &["svn:keywords", "svn:eol-style", "svn:special"];

struct Svn<'r> {
    svn: &'r Path,
    svnversion: &'r Path,
    bounds: Bounds,
    env: Vec<(&'static str, OsString)>,
}

type Output = (ExitStatus, Vec<u8>, Vec<u8>);

impl Svn<'_> {
    fn raw(&self, exe: &Path, dir: &Path, args: &[&str]) -> Result<Output, HelperError> {
        let env: Vec<(&str, &OsStr)> = self.env.iter().map(|(k, v)| (*k, v.as_os_str())).collect();
        let argv: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        helper::run_any_with(exe, &argv, None, self.bounds, &Extra { cwd: Some(dir), env: &env })
    }

    /// `svn <subcommand> --non-interactive <args> .`, run in `dir`.
    fn svn(&self, dir: &Path, subcommand: &str, args: &[&str]) -> Result<Output, HelperError> {
        let mut argv = vec![subcommand, "--non-interactive"];
        argv.extend_from_slice(args);
        argv.push(".");
        self.raw(self.svn, dir, &argv)
    }

    /// [`Svn::svn`], where only a zero exit is an answer.
    fn ok(&self, dir: &Path, subcommand: &str, args: &[&str]) -> Result<Vec<u8>, String> {
        answer(self.svn(dir, subcommand, args), &format!("svn {subcommand}"))
    }

    fn svnversion(&self, dir: &Path) -> Result<String, String> {
        answer(self.raw(self.svnversion, dir, &["."]), "svnversion").map(|o| String::from_utf8_lossy(&o).trim().to_string())
    }
}

/// svn's errors are located by their code (`E155021`), which does not
/// change with the locale, and quoted for the rest.
fn answer(r: Result<Output, HelperError>, what: &str) -> Result<Vec<u8>, String> {
    match r {
        Ok((st, out, _)) if st.success() => Ok(out),
        Ok((_, _, stderr)) => Err(failure(&String::from_utf8_lossy(&stderr), what)),
        Err(e) => Err(why(&e, what)),
    }
}

/// Why a program gave no answer, in a sentence.
fn why(e: &HelperError, what: &str) -> String {
    match e {
        HelperError::Unresolved(p) | HelperError::Spawn { tool: p, .. } if what.starts_with("svnversion") => format!("svnversion could not be run at {}: {e}", p.display()),
        HelperError::Unresolved(_) => "svn was not found on PATH".into(),
        HelperError::Spawn { tool, error } => format!("svn could not be run at {}: {error}", tool.display()),
        other => format!("{what}: {other}"),
    }
}

/// The error codes svn wrote, in order.
fn codes(stderr: &str) -> Vec<&str> {
    stderr
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() == 7 && (w.starts_with('E') || w.starts_with('W')) && w[1..].bytes().all(|b| b.is_ascii_digit()))
        .collect()
}

fn failure(stderr: &str, what: &str) -> String {
    let text: String = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).collect::<Vec<_>>().join(" ").chars().take(400).collect();
    let c = codes(stderr);
    let has = |code: &str| c.contains(&code);
    if has("E155021") {
        format!("the working copy's format is newer than this svn client reads: it was written by a newer Subversion client, which is needed to read it ({text})")
    } else if has("E155036") {
        format!("the working copy is in an older format this svn client does not read until `svn upgrade` converts it ({text})")
    } else if has("E155037") || has("E155004") || has("E155032") {
        format!("the working copy is locked by an svn operation that is under way or was interrupted, and needs `svn cleanup` ({text})")
    } else if has("W155010") {
        format!("not under version control ({text})")
    } else if has("E155007") {
        format!("not a working copy ({text})")
    } else {
        format!("{what} failed: {text}")
    }
}

/// A UTF-8 locale for svn when the operator's is not one: svn converts
/// paths through the locale's character set, and under `C` a non-ASCII name
/// on disk cannot be converted. Also makes its messages English, though
/// only the error codes are relied on.
fn locale() -> Vec<(&'static str, OsString)> {
    let effective = ["LC_ALL", "LC_CTYPE", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()));
    let utf8 = effective.is_some_and(|v| {
        let v = v.to_ascii_lowercase();
        v.contains("utf-8") || v.contains("utf8")
    });
    if utf8 {
        Vec::new()
    } else {
        vec![("LC_ALL", OsString::from("C.UTF-8"))]
    }
}

// ---- the working copy -----------------------------------------------------

struct Node {
    file: bool,
    /// SHA-1 of the base copy's normal form.
    checksum: Option<String>,
    depth: Option<String>,
}

struct Status {
    item: String,
    switched: bool,
    locked: bool,
}

/// One working copy, read.
struct Wc {
    root: PathBuf,
    svnversion: String,
    low: u64,
    high: u64,
    flags: String,
    id: RevisionId,
    repository: Option<String>,
    /// Every versioned node, by its path relative to the root (`""` is the
    /// root itself).
    nodes: HashMap<Vec<u8>, Node>,
    /// Files whose properties make their base checksum inapplicable.
    translated: BTreeSet<Vec<u8>>,
    /// Of those, the files whose only translating property is
    /// `svn:eol-style`: their normal form is their text with LF line
    /// endings, so they are compared by hash all the same (#315).
    eol_only: BTreeSet<Vec<u8>>,
    /// What `svn status` reported, by path.
    status: BTreeMap<Vec<u8>, Status>,
}

impl Wc {
    fn switched(&self) -> Vec<String> {
        self.status.iter().filter(|(_, s)| s.switched).map(|(p, _)| show_rel(p)).collect()
    }

    fn sparse(&self) -> Vec<String> {
        let mut v: Vec<String> =
            self.nodes.iter().filter_map(|(p, n)| n.depth.as_deref().filter(|d| !n.file && *d != "infinity").map(|d| format!("{} ({d})", show_rel(p)))).collect();
        v.sort();
        v
    }

    fn locked(&self) -> Vec<String> {
        self.status.iter().filter(|(_, s)| s.locked).map(|(p, _)| show_rel(p)).collect()
    }
}

fn show_rel(p: &[u8]) -> String {
    if p.is_empty() {
        ".".into()
    } else {
        String::from_utf8_lossy(p).into_owned()
    }
}

/// A path as svn printed it, relative to the working copy's root.
fn rel_of(p: &str) -> Vec<u8> {
    let p = p.strip_prefix("./").unwrap_or(p);
    if p == "." {
        Vec::new()
    } else {
        p.as_bytes().to_vec()
    }
}

/// svnversion's answer: the range and the flags, or why it is not one.
fn parse_svnversion(s: &str) -> Result<(u64, u64, String), String> {
    let at = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let (num, flags) = s.split_at(at);
    if let (Some((low, high)), true) = (svn_range(num), flags.chars().all(|c| matches!(c, 'M' | 'S' | 'P'))) {
        return Ok((low, high, flags.to_string()));
    }
    Err(match s {
        "Unversioned directory" | "Unversioned file" => "svnversion reports the working copy's top as unversioned".into(),
        "Uncommitted local addition, copy or move" => "the working copy's top is added, copied or moved and not committed, so it has no revision".into(),
        other => format!("svnversion answered {other:?}, which is not a revision"),
    })
}

/// Read a working copy: its revision, every versioned node, the properties
/// that stop a hash comparison, and what `svn status` says. `Err` carries
/// the reason and, when it was read, the revision.
fn open_wc(svn: &Svn, root: &Path) -> Result<Wc, (String, Option<RevisionId>)> {
    let info = svn.ok(root, "info", &["--xml", "-R"]).map_err(|e| (e, None))?;
    let info = xml::parse(&String::from_utf8_lossy(&info)).map_err(|e| (format!("svn info wrote XML this could not read: {e}"), None))?;
    let mut nodes = HashMap::new();
    let mut top: Option<(String, String, String, String)> = None; // revision, uuid, relative-url, repository root
    for e in info.all("entry") {
        let Some(path) = e.attr("path") else { continue };
        let rel = rel_of(path);
        let wc_info = e.child("wc-info");
        if rel.is_empty() {
            let text = |p: &[&str]| e.find(p).map(|x| x.text.trim().to_string()).unwrap_or_default();
            top = Some((e.attr("revision").unwrap_or_default().to_string(), text(&["repository", "uuid"]), text(&["relative-url"]), text(&["repository", "root"])));
        }
        nodes.insert(
            rel,
            Node {
                file: e.attr("kind") == Some("file"),
                checksum: wc_info.and_then(|w| w.child("checksum")).map(|c| c.text.trim().to_ascii_lowercase()).filter(|c| c.len() == 40),
                depth: wc_info.and_then(|w| w.child("depth")).map(|d| d.text.trim().to_string()),
            },
        );
    }
    let Some((_, uuid, relative, repo_root)) = top else {
        return Err(("svn info did not describe the working copy's top".into(), None));
    };
    let version = svn.svnversion(root).map_err(|e| (e, None))?;
    let (low, high, flags) = match parse_svnversion(&version) {
        Ok(v) => v,
        Err(e) => return Err((e, None)),
    };
    let range = if low == high { low.to_string() } else { format!("{low}:{high}") };
    let id = RevisionId { revision: range, repository_uuid: (!uuid.is_empty()).then_some(uuid), path: (!relative.is_empty()).then(|| strip_credentials(&relative)) };
    let repository = (!repo_root.is_empty()).then(|| strip_credentials(&repo_root));

    let props = svn.ok(root, "proplist", &["--xml", "-R"]).map_err(|e| (e, Some(id.clone())))?;
    let props = xml::parse(&String::from_utf8_lossy(&props)).map_err(|e| (format!("svn proplist wrote XML this could not read: {e}"), Some(id.clone())))?;
    let mut translated = BTreeSet::new();
    let mut eol_only = BTreeSet::new();
    for t in props.all("target") {
        let names: Vec<&str> = t.all("property").filter_map(|p| p.attr("name")).filter(|n| TRANSLATING.contains(n)).collect();
        if names.is_empty() {
            continue;
        }
        if let Some(p) = t.attr("path") {
            if names.iter().all(|n| *n == "svn:eol-style") {
                eol_only.insert(rel_of(p));
            }
            translated.insert(rel_of(p));
        }
    }

    // --ignore-externals: each external is judged as its own working copy
    let st = svn.ok(root, "status", &["--xml", "--ignore-externals"]).map_err(|e| (e, Some(id.clone())))?;
    let st = xml::parse(&String::from_utf8_lossy(&st)).map_err(|e| (format!("svn status wrote XML this could not read: {e}"), Some(id.clone())))?;
    let mut status = BTreeMap::new();
    for target in st.all("target") {
        for e in target.all("entry") {
            let (Some(path), Some(w)) = (e.attr("path"), e.child("wc-status")) else { continue };
            status.insert(
                rel_of(path),
                Status { item: w.attr("item").unwrap_or("none").to_string(), switched: w.attr("switched") == Some("true"), locked: w.attr("wc-locked") == Some("true") },
            );
        }
    }
    Ok(Wc { root: root.to_path_buf(), svnversion: version, low, high, flags, id, repository, nodes, translated, eol_only, status })
}

// ---- judging ------------------------------------------------------------

#[derive(Default)]
struct Judged {
    modified: Vec<ChangedSource>,
    verified: u64,
    hash_verified: u64,
    status_only: u64,
    outside: u64,
    externals: Vec<SvnExternal>,
    notes: Vec<String>,
    doubts: Vec<String>,
}

pub(super) fn inspect(req: &Request, files: &[Placed], found: &Path) -> SourceRevision {
    let mut rev = SourceRevision { root: Some(found.display().to_string()), ..SourceRevision::unknown(Vcs::Svn, "") };
    let Some(exe) = req.svn.as_deref() else {
        rev.state = SourceState::Unknown("svn was not found on PATH".into());
        return rev;
    };
    let Some(ver_exe) = req.svnversion.as_deref() else {
        rev.state = SourceState::Unknown("svnversion was not found beside svn or on PATH".into());
        return rev;
    };
    let svn = Svn { svn: exe, svnversion: ver_exe, bounds: req.bounds, env: locale() };
    let start = req.start.canonicalize().unwrap_or_else(|_| req.start.clone());

    // The first call says whether svn runs at all, and where the working
    // copy's top is.
    let first = svn.svn(&start, "info", &["--xml"]);
    if !matches!(first, Err(HelperError::Unresolved(_) | HelperError::Spawn { .. })) {
        rev.tool = Some(crate::provenance::identify("svn", exe).0);
    }
    let top = match first {
        Ok((st, out, _)) if st.success() => xml::parse(&String::from_utf8_lossy(&out))
            .ok()
            .and_then(|x| x.all("entry").next().and_then(|e| e.find(&["wc-info", "wcroot-abspath"])).map(|w| PathBuf::from(w.text.trim())))
            .filter(|p| p.is_absolute()),
        Ok((_, _, stderr)) => {
            let stderr = String::from_utf8_lossy(&stderr);
            let c = codes(&stderr);
            rev.state = SourceState::Unknown(if c.contains(&"W155010") {
                format!("the source root {} is not under version control: it is an unversioned directory in the SVN working copy at {}", start.display(), found.display())
            } else if c.contains(&"E155007") {
                format!("{} holds a .svn directory, but svn says it is not a working copy", found.display())
            } else {
                failure(&stderr, "svn info")
            });
            return rev;
        }
        Err(e) => {
            rev.state = SourceState::Unknown(why(&e, "svn info"));
            return rev;
        }
    };
    let Some(top) = top else {
        rev.state = SourceState::Unknown("svn info did not name the working copy's top directory".into());
        return rev;
    };
    let top = top.canonicalize().unwrap_or(top);
    rev.root = Some(top.display().to_string());

    let wc = open_wc(&svn, &top);
    let version_tool = crate::provenance::identify("svnversion", ver_exe).0;
    let wc = match wc {
        Ok(w) => w,
        Err((reason, id)) => {
            rev.id = id;
            rev.state = SourceState::Unknown(reason);
            return rev;
        }
    };
    rev.id = Some(wc.id.clone());
    rev.repository = wc.repository.clone();
    let cap = |mut v: Vec<String>| {
        let n = v.len() as u64;
        v.truncate(LIST_CAP);
        (v, n)
    };
    let (switched, switched_count) = cap(wc.switched());
    let (sparse, sparse_count) = cap(wc.sparse());
    let mut detail = SvnDetail {
        svnversion: wc.svnversion.clone(),
        low: wc.low,
        high: wc.high,
        locally_modified: wc.flags.contains('M'),
        switched,
        switched_count,
        sparse,
        sparse_count,
        svnversion_tool: Some(version_tool),
        ..Default::default()
    };
    if wc.high == 0 {
        rev.svn = Some(detail);
        rev.state =
            SourceState::Unknown("the working copy is at r0, the repository's empty first revision: nothing is committed yet, so there is no revision to compare with".into());
        return rev;
    }

    // outside the working copy: not under version control, not judged
    let (inside, outside): (Vec<&Placed>, Vec<&Placed>) = files.iter().partition(|f| f.path.starts_with(&top));
    let mut unversioned: Vec<String> = outside.iter().map(|f| f.path.display().to_string()).collect();
    unversioned.sort();
    rev.unversioned_count = unversioned.len() as u64;
    unversioned.truncate(LIST_CAP);
    rev.unversioned = unversioned;

    let mut judged = Judged::default();
    judge(&svn, &wc, &inside, "", req, 0, &mut judged);

    let mut modified = judged.modified;
    modified.sort_by(|a, b| a.path.cmp(&b.path));
    modified.dedup_by(|a, b| a.path == b.path);
    rev.modified_count = modified.len() as u64;
    modified.truncate(LIST_CAP);
    rev.modified = modified;
    rev.verified_count = judged.verified;
    rev.outside_build = Some(judged.outside);
    detail.hash_verified = judged.hash_verified;
    detail.status_only = judged.status_only;
    detail.externals = judged.externals;
    if detail.status_only > 0 {
        rev.notes.push(format!(
            "{} file(s) the build used carry svn:keywords, svn:eol-style or svn:special, so their base checksum -- taken over the file's normal form -- cannot be compared with the working file: their state is svn status's, not a hash comparison",
            detail.status_only
        ));
    }
    rev.notes.extend(judged.notes);
    for d in &judged.doubts {
        if !rev.notes.contains(d) {
            rev.notes.push(d.clone());
        }
    }
    rev.svn = Some(detail);
    rev.state = if rev.modified_count > 0 {
        SourceState::Modified
    } else if !judged.doubts.is_empty() {
        SourceState::Unknown(judged.doubts.join("; "))
    } else if rev.verified_count == 0 {
        SourceState::Unknown(if files.is_empty() {
            "the evidence names no source file to verify".into()
        } else {
            "none of the files the build used is inside the working copy".into()
        })
    } else {
        SourceState::Clean
    };
    rev
}

/// The nearest working copy nested inside `root` that holds `file` -- an
/// external, or a checkout inside this one -- found by its `.svn`.
fn nested_root(root: &Path, file: &Path, cache: &mut HashMap<PathBuf, Option<PathBuf>>) -> Option<PathBuf> {
    let dir = file.parent()?;
    if let Some(hit) = cache.get(dir) {
        return hit.clone();
    }
    let mut found = None;
    for d in dir.ancestors() {
        if d == root || !d.starts_with(root) {
            break;
        }
        if d.join(".svn").symlink_metadata().is_ok() {
            found = Some(d.to_path_buf());
            break;
        }
    }
    cache.insert(dir.to_path_buf(), found.clone());
    found
}

/// Judge `files` (all inside `wc.root`) against the working copy, recursing
/// into the nested working copies that hold any of them. `prefix` places
/// this working copy's paths under the top one.
fn judge(svn: &Svn, wc: &Wc, files: &[&Placed], prefix: &str, req: &Request, depth: usize, out: &mut Judged) {
    let show = |rel: &[u8]| format!("{prefix}{}", String::from_utf8_lossy(rel));
    let locked = wc.locked();
    if !locked.is_empty() {
        out.doubts.push(format!(
            "the working copy at {} is locked ({}): an svn operation is under way in it, or one was interrupted and it needs `svn cleanup`, so its files may be mid-change",
            wc.root.display(),
            locked.join(", ")
        ));
    }
    let mut cache = HashMap::new();
    let mut here: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut nested: BTreeMap<PathBuf, Vec<&Placed>> = BTreeMap::new();
    for f in files {
        if let Some(n) = nested_root(&wc.root, &f.path, &mut cache) {
            nested.entry(n).or_default().push(f);
            continue;
        }
        let rel = f.path.strip_prefix(&wc.root).unwrap_or(&f.path).as_os_str().as_bytes().to_vec();
        here.insert(rel.clone());
        judge_file(wc, f, &rel, &show(&rel), req, out);
    }

    // changes the build does not use, from svn status: an unversioned
    // directory holding a build file, or a nested working copy, is not one
    let mut shelter: BTreeSet<&[u8]> = BTreeSet::new();
    let nested_rels: Vec<Vec<u8>> = nested.keys().map(|n| n.strip_prefix(&wc.root).unwrap_or(n).as_os_str().as_bytes().to_vec()).collect();
    for p in here.iter().chain(nested_rels.iter()) {
        for (i, b) in p.iter().enumerate() {
            if *b == b'/' {
                shelter.insert(&p[..i]);
            }
        }
        shelter.insert(p);
    }
    out.outside += wc.status.iter().filter(|(p, s)| !shelter.contains(p.as_slice()) && CHANGED.contains(&s.item.as_str())).count() as u64;

    for (root, sub_files) in nested {
        let path = show(root.strip_prefix(&wc.root).unwrap_or(&root).as_os_str().as_bytes());
        let mut record = SvnExternal { path: path.clone(), ..Default::default() };
        if depth >= MAX_DEPTH {
            out.doubts.push(format!("{path}: working copies nested more than {MAX_DEPTH} deep are not followed"));
            out.externals.push(record);
            continue;
        }
        match open_wc(svn, &root) {
            Ok(sub) => {
                record.id = Some(sub.id.clone());
                record.repository = sub.repository.clone();
                record.svnversion = Some(sub.svnversion.clone());
                out.notes.push(format!(
                    "{path} is its own SVN working copy (an external, or a checkout inside this one) at {}: the parent revision does not determine its content, so its files were judged against its own",
                    sub.svnversion
                ));
                out.externals.push(record);
                if sub.high == 0 {
                    out.doubts.push(format!("{path}: the working copy is at r0, so there is no revision to compare with"));
                    continue;
                }
                judge(svn, &sub, &sub_files, &format!("{path}/"), req, depth + 1, out);
            }
            Err((reason, id)) => {
                record.id = id;
                out.doubts.push(format!("{path}: {reason}"));
                out.externals.push(record);
            }
        }
    }
}

fn judge_file(wc: &Wc, f: &Placed, rel: &[u8], shown: &str, req: &Request, out: &mut Judged) {
    let item = wc.status.get(rel).map(|s| s.item.as_str()).unwrap_or("normal");
    let change = |out: &mut Judged, c: Change| {
        out.verified += 1;
        out.modified.push(ChangedSource { path: shown.to_string(), change: c });
    };
    match item {
        "unversioned" | "ignored" | "added" | "replaced" => return change(out, Change::NotInRevision),
        "missing" => return change(out, Change::Missing),
        "deleted" if f.bytes.is_none() && f.path.symlink_metadata().is_err() => return change(out, Change::Missing),
        "modified" | "conflicted" | "obstructed" => return change(out, Change::Content),
        "incomplete" => {
            out.doubts.push(format!("{shown}: svn reports it incomplete, as an interrupted update leaves it"));
            return;
        }
        // normal, a file external, or scheduled for deletion but still here
        _ => {}
    }
    // not versioned, and not reported: ignored
    let Some(node) = wc.nodes.get(rel) else { return change(out, Change::NotInRevision) };
    if !node.file {
        return;
    }
    let eol = wc.eol_only.contains(rel);
    let sum = match (&node.checksum, wc.translated.contains(rel) && !eol) {
        (Some(sum), false) => sum,
        _ if item == "deleted" => {
            out.doubts.push(format!("{shown} is scheduled for deletion, and its properties stop a comparison with its base checksum"));
            return;
        }
        _ => {
            out.verified += 1;
            out.status_only += 1;
            return;
        }
    };
    let read;
    let bytes = match f.bytes {
        Some(b) => b,
        None => match read_file(&f.path, req.max_file_bytes) {
            Ok(Some(b)) => {
                read = b;
                read.as_slice()
            }
            Ok(None) => return change(out, Change::Missing),
            Err(e) => {
                out.doubts.push(format!("{shown} could not be read to compare with the revision: {e}"));
                return;
            }
        },
    };
    match if eol { compare_normal_form(bytes, sum) } else { compare(bytes, sum) } {
        Verdict::Same => {
            out.verified += 1;
            out.hash_verified += 1;
        }
        Verdict::Collision => out.doubts.push(format!("{shown}: its SHA-1 shows the signature of a collision attack, so a matching checksum proves nothing")),
        Verdict::LineEndings => {
            out.hash_verified += 1;
            change(out, Change::LineEndings)
        }
        Verdict::Differs => {
            out.hash_verified += 1;
            change(out, Change::Content)
        }
    }
}

enum Verdict {
    Same,
    LineEndings,
    Differs,
    Collision,
}

/// Compare `bytes` with a base copy's SHA-1.
fn compare(bytes: &[u8], sum: &str) -> Verdict {
    let (id, collided) = sha1(bytes);
    if collided {
        return Verdict::Collision;
    }
    if id == sum {
        return Verdict::Same;
    }
    let lf = crlf_to_lf(bytes);
    let crlf = lf_to_crlf(&lf);
    for alt in [lf, crlf] {
        if alt != bytes {
            let (id, collided) = sha1(&alt);
            if !collided && id == sum {
                return Verdict::LineEndings;
            }
        }
    }
    Verdict::Differs
}

/// Compare an `svn:eol-style` file with its base copy's SHA-1. The base
/// copy's line endings are the property's to choose -- LF for `native`, the
/// named style for `CRLF`, `CR` or `LF` -- so the file is compared with its
/// text under each style: equal under one, it is the same text; a change to
/// anything but line endings still differs under all of them (#315).
fn compare_normal_form(bytes: &[u8], sum: &str) -> Verdict {
    let lf = crlf_to_lf(bytes);
    let lf: Vec<u8> = lf.iter().map(|&b| if b == b'\r' { b'\n' } else { b }).collect();
    let cr: Vec<u8> = lf.iter().map(|&b| if b == b'\n' { b'\r' } else { b }).collect();
    let crlf = lf_to_crlf(&lf);
    for form in [bytes, lf.as_slice(), crlf.as_slice(), cr.as_slice()] {
        match sha1(form) {
            (_, true) => return Verdict::Collision,
            (id, false) if id == sum => return Verdict::Same,
            _ => {}
        }
    }
    Verdict::Differs
}

/// SHA-1 of `bytes`, and whether collision detection fired.
fn sha1(bytes: &[u8]) -> (String, bool) {
    use sha1_checked::Digest;
    let mut h = sha1_checked::Sha1::new();
    h.update(bytes);
    let r = h.try_finalize();
    (r.hash().iter().map(|b| format!("{b:02x}")).collect(), r.has_collision())
}

/// The caveat's account of an SVN working copy's shape: a mixed revision,
/// switched paths, a sparse checkout. Each is worth saying even when every
/// file is clean.
pub(super) fn shape(rev: &SourceRevision) -> Vec<String> {
    let Some(s) = &rev.svn else { return Vec::new() };
    let mut v = Vec::new();
    if s.low != s.high {
        v.push(format!("The SVN working copy is at mixed revisions {}:{}: its files come from more than one revision, so no single revision number names them.", s.low, s.high));
    }
    let list = |l: &[String], n: u64| {
        let more = n.saturating_sub(l.len() as u64);
        format!("{}{}", l.join(", "), if more > 0 { format!(", and {more} more") } else { String::new() })
    };
    if s.switched_count > 0 {
        v.push(format!(
            "{} path(s) in the SVN working copy are switched to another URL than {} ({}): their files are that URL's.",
            s.switched_count,
            rev.id.as_ref().and_then(|i| i.path.as_deref()).unwrap_or("the working copy's"),
            list(&s.switched, s.switched_count)
        ));
    }
    if s.sparse_count > 0 {
        v.push(format!("The SVN working copy is a sparse checkout: {} ({}) are not at full depth.", s.sparse_count, list(&s.sparse, s.sparse_count)));
    }
    // What a clean record rests on (#315).
    if rev.state.is_clean() {
        if s.status_only > 0 {
            v.push(format!(
                "{} file(s) the build used carry svn:keywords or svn:special and were judged by svn status alone, which trusts each file's size and timestamp: an edit that keeps both is not seen.",
                s.status_only
            ));
        }
        if s.hash_verified > 0 {
            v.push(format!(
                "{} file(s) the build used were compared with the base checksums the SVN working copy keeps, which are working-copy metadata (.svn/wc.db), not verified against the repository: whoever can edit the working copy can edit them too.",
                s.hash_verified
            ));
        }
    }
    v
}

// ---- XML ------------------------------------------------------------------

/// Just enough XML for what `svn --xml` writes: elements, attributes, text
/// and the predefined and numeric entities. No DTDs, no namespaces.
mod xml {
    #[derive(Debug, Default)]
    pub struct El {
        pub name: String,
        pub attrs: Vec<(String, String)>,
        pub kids: Vec<El>,
        pub text: String,
    }

    impl El {
        pub fn attr(&self, k: &str) -> Option<&str> {
            self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
        }

        pub fn child(&self, name: &str) -> Option<&El> {
            self.kids.iter().find(|k| k.name == name)
        }

        pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
            self.kids.iter().filter(move |k| k.name == name)
        }

        pub fn find(&self, path: &[&str]) -> Option<&El> {
            path.iter().try_fold(self, |e, n| e.child(n))
        }
    }

    /// The document's root element.
    pub fn parse(s: &str) -> Result<El, String> {
        let mut stack: Vec<El> = vec![El::default()];
        let mut i = 0;
        while i < s.len() {
            let rest = &s[i..];
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                stack.last_mut().unwrap().text.push_str(&decode(&rest[..end])?);
                i += end;
                continue;
            }
            let skip = |open: &str, close: &str| rest.starts_with(open).then(|| rest.find(close).map(|e| e + close.len()));
            if let Some(end) = skip("<?", "?>").or_else(|| skip("<!--", "-->")).or_else(|| skip("<!DOCTYPE", ">")) {
                i += end.ok_or("an unterminated declaration")?;
                continue;
            }
            if let Some(body) = rest.strip_prefix("<![CDATA[") {
                let end = body.find("]]>").ok_or("an unterminated CDATA section")?;
                stack.last_mut().unwrap().text.push_str(&body[..end]);
                i += 9 + end + 3;
                continue;
            }
            // a tag ends at the first '>' outside a quoted value
            let mut quote = None;
            let mut end = None;
            for (j, c) in rest.char_indices().skip(1) {
                match (quote, c) {
                    (None, '"' | '\'') => quote = Some(c),
                    (Some(q), c) if c == q => quote = None,
                    (None, '>') => {
                        end = Some(j);
                        break;
                    }
                    _ => {}
                }
            }
            let end = end.ok_or("an unterminated tag")?;
            let tag = &rest[1..end];
            i += end + 1;
            if let Some(name) = tag.strip_prefix('/') {
                let el = stack.pop().filter(|_| !stack.is_empty()).ok_or("a closing tag with nothing open")?;
                if el.name != name.trim() {
                    return Err(format!("</{}> closes <{}>", name.trim(), el.name));
                }
                stack.last_mut().ok_or("a closing tag with nothing open")?.kids.push(el);
                continue;
            }
            let (tag, closed) = match tag.strip_suffix('/') {
                Some(t) => (t, true),
                None => (tag, false),
            };
            let name_end = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
            let mut el = El { name: tag[..name_end].to_string(), ..Default::default() };
            let mut a = &tag[name_end..];
            loop {
                a = a.trim_start();
                if a.is_empty() {
                    break;
                }
                let eq = a.find('=').ok_or_else(|| format!("an attribute without a value in <{}>", el.name))?;
                let key = a[..eq].trim().to_string();
                let v = a[eq + 1..].trim_start();
                let q = v.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or("an unquoted attribute value")?;
                let close = v[1..].find(q).ok_or("an unterminated attribute value")?;
                el.attrs.push((key, decode(&v[1..1 + close])?));
                a = &v[1 + close + 1..];
            }
            if closed {
                stack.last_mut().unwrap().kids.push(el);
            } else {
                stack.push(el);
            }
        }
        if stack.len() != 1 {
            return Err(format!("<{}> is not closed", stack.last().map(|e| e.name.as_str()).unwrap_or("")));
        }
        stack.pop().unwrap().kids.pop().ok_or_else(|| "no root element".into())
    }

    fn decode(s: &str) -> Result<String, String> {
        if !s.contains('&') {
            return Ok(s.to_string());
        }
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(i) = rest.find('&') {
            out.push_str(&rest[..i]);
            let tail = &rest[i + 1..];
            let semi = tail.find(';').filter(|n| *n <= 10).ok_or("a bare '&'")?;
            let ent = &tail[..semi];
            let c = match ent {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                _ => {
                    let n = if let Some(h) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                        u32::from_str_radix(h, 16).ok()
                    } else if let Some(d) = ent.strip_prefix('#') {
                        d.parse().ok()
                    } else {
                        None
                    };
                    n.and_then(char::from_u32).ok_or_else(|| format!("an unknown entity &{ent};"))?
                }
            };
            out.push(c);
            rest = &tail[semi + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_what_svn_writes() {
            let x = parse(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<status>\n<target\n   path=\".\">\n<entry\n   path=\"a&amp;b&lt;c&gt;&apos;&quot;.c\">\n<wc-status\n   props=\"none\"\n   item=\"modified\"\n   revision=\"3\"/>\n</entry>\n<entry path='x &#252; &#xFC;.c'><wc-status item=\"normal\"></wc-status></entry>\n</target>\n</status>\n",
            )
            .unwrap();
            assert_eq!(x.name, "status");
            let entries: Vec<&El> = x.child("target").unwrap().all("entry").collect();
            assert_eq!(entries[0].attr("path"), Some("a&b<c>'\".c"));
            assert_eq!(entries[0].find(&["wc-status"]).unwrap().attr("item"), Some("modified"));
            assert_eq!(entries[1].attr("path"), Some("x ü ü.c"));
        }

        #[test]
        fn text_and_nesting() {
            let x = parse("<info><entry path=\"p\"><wc-info><checksum>ab&amp;c</checksum></wc-info></entry></info>").unwrap();
            assert_eq!(x.find(&["entry", "wc-info", "checksum"]).unwrap().text, "ab&c");
        }

        #[test]
        fn malformed_input_is_an_error_not_a_panic() {
            for bad in ["<a>", "<a></b>", "</a>", "<a b=c/>", "<a b=\"c/>", "<a>&bogus;</a>", "<a>& </a>", "<a", "<!-- x", "<a><![CDATA[x</a>", "", "text only"] {
                assert!(parse(bad).is_err(), "{bad:?}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svnversion_answers() {
        assert_eq!(parse_svnversion("1234"), Ok((1234, 1234, String::new())));
        assert_eq!(parse_svnversion("1230:1234MSP"), Ok((1230, 1234, "MSP".into())));
        assert_eq!(parse_svnversion("4M"), Ok((4, 4, "M".into())));
        assert!(parse_svnversion("Unversioned directory").unwrap_err().contains("unversioned"));
        assert!(parse_svnversion("Uncommitted local addition, copy or move").unwrap_err().contains("no revision"));
        assert!(parse_svnversion("4X").is_err());
        assert!(parse_svnversion("5:4").is_err());
        assert!(parse_svnversion("").is_err());
    }

    #[test]
    fn errors_are_recognised_by_their_code() {
        assert!(failure("svn: E155021: This client is too old to work with the working copy at\n'/x' (format 99).", "svn info").contains("newer"));
        assert!(failure("svn: E155037: Previous operation has not finished; run 'cleanup' if it was interrupted", "svn status").contains("svn cleanup"));
        assert!(failure("svn: warning: W155010: The node '/x/gen' was not found.\n\nsvn: E200009: Could not display info", "svn info").starts_with("not under version control"));
        assert!(failure("svn: E155007: '/x' is not a working copy", "svn info").starts_with("not a working copy"));
        // localised text, the same code
        assert!(failure("svn: E155021: Dieser Client ist zu alt", "svn info").contains("newer"));
        assert!(failure("svn: E999999: something else", "svn info").starts_with("svn info failed"));
    }

    #[test]
    fn credentials_are_removed_from_svn_urls() {
        assert_eq!(strip_credentials("svn+ssh://someone@svn.example.invalid/repo"), "svn+ssh://svn.example.invalid/repo");
        assert_eq!(strip_credentials("https://someone:s3cret@svn.example.invalid/repo"), "https://svn.example.invalid/repo");
        assert_eq!(strip_credentials("file:///srv/svn/repo"), "file:///srv/svn/repo");
        assert_eq!(strip_credentials("^/trunk"), "^/trunk");
    }
}
