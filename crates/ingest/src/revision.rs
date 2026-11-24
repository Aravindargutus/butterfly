// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Reading the project's source revision at ingest (#300).
//!
//! The revision id is a label; the file hashes are the proof. So this does
//! not ask git whether the working tree is clean -- `git status` can be told
//! not to look (`assume-unchanged`, `skip-worktree`) or be answered by a
//! program the repository names (`core.fsmonitor`). It takes the bytes ingest
//! already read and hashed, computes each file's git blob id from them, and
//! compares that with the entry in HEAD's tree. Git is asked only for facts
//! it cannot be talked out of: which commit HEAD is, and what that commit's
//! tree holds.
//!
//! The files judged are the files the build used: the coverage file list,
//! with the bytes ingest read, plus the files the DWARF line tables name,
//! which this reads itself when they are inside the working copy. A file
//! outside the working copy (a vendored SDK, a system header) is listed as
//! not under version control. Changed files the build does not use are
//! counted separately and do not make the build modified.
//!
//! Three states, never two: anything that stops the comparison -- no git, a
//! refused repository, no commits, a timeout, a corrupt object store, a file
//! that could not be read -- is `unknown` with its reason, never clean.
//!
//! The layer is shaped for two systems. [`detect`] finds the working copy by
//! looking for `.git` or `.svn` without running either; each system then
//! fills the same [`SourceRevision`]. SVN (#301) is in [`svn`]: there the
//! base checksums are of each file's normal form, so a file whose
//! properties translate it (keywords, eol-style) is judged by `svn status`,
//! and every other file by its hash.

mod svn;

use crate::helper::{self, Bounds, HelperError};
use butterfly_model::revision::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};

/// One file the build used.
#[derive(Debug, Clone)]
pub struct BuildFile<'a> {
    /// Where it is on this machine. A relative path is taken against
    /// [`Request::start`].
    pub path: PathBuf,
    /// The bytes ingest read and hashed, when it read them (the coverage
    /// file list). `None` for a file only named -- a header from the DWARF
    /// line tables -- which is read here, if it is inside the working copy.
    pub bytes: Option<&'a [u8]>,
}

/// What to inspect, and with what.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    /// Where to look for the working copy: the source root.
    pub start: PathBuf,
    pub files: Vec<BuildFile<'a>>,
    /// The git executable, as an absolute path; `None` when it was not
    /// found.
    pub git: Option<PathBuf>,
    /// `svn` and `svnversion`, likewise (#301).
    pub svn: Option<PathBuf>,
    pub svnversion: Option<PathBuf>,
    /// For every git and svn call.
    pub bounds: Bounds,
    /// The largest file this reads itself.
    pub max_file_bytes: u64,
    /// How much of the working copy outside the build is read to count its
    /// changes; past it, the count is not made.
    pub max_outside_bytes: u64,
}

impl<'a> Request<'a> {
    /// With git and svn resolved on `PATH` and the standard helper bounds.
    pub fn new(start: PathBuf, files: Vec<BuildFile<'a>>) -> Request<'a> {
        let mut req = Request { start, files, git: None, svn: None, svnversion: None, bounds: Bounds::standard(), max_file_bytes: 256 << 20, max_outside_bytes: 1 << 30 };
        if let Some(path) = std::env::var_os("PATH") {
            req.resolve_tools_in(&path);
        }
        req
    }

    /// Resolve git, svn and svnversion against `path_var`, a `PATH`-style
    /// list, as the OS would. `svnversion` is taken from beside the `svn`
    /// found when it is there, so the two come from one installation.
    pub fn resolve_tools_in(&mut self, path_var: &OsStr) {
        use crate::provenance::resolve_in;
        self.git = resolve_in(path_var, "git");
        self.svn = resolve_in(path_var, "svn");
        self.svnversion = self.svn.as_deref().and_then(Path::parent).and_then(|d| resolve_in(d.as_os_str(), "svnversion")).or_else(|| resolve_in(path_var, "svnversion"));
    }
}

/// The working copy `start` is in, found by looking for `.git` or `.svn` in
/// it and its parents -- the nearest wins -- without running either tool.
pub fn detect(start: &Path) -> Option<(Vcs, PathBuf)> {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    for d in start.ancestors() {
        if d.join(".git").symlink_metadata().is_ok() {
            return Some((Vcs::Git, d.to_path_buf()));
        }
        if d.join(".svn").symlink_metadata().is_ok() {
            return Some((Vcs::Svn, d.to_path_buf()));
        }
    }
    None
}

/// Read the revision and judge the files the build used against it.
pub fn inspect(req: &Request) -> SourceRevision {
    let files = prepare(req);
    let mut rev = match detect(&req.start) {
        None => SourceRevision::unknown(Vcs::None, "the sources are not in a git or SVN working copy"),
        Some((Vcs::Svn, root)) => svn::inspect(req, &files, &root),
        Some((_, _)) => git_inspect(req, &files),
    };
    rev.captured_at = CapturedAt::Ingest;
    rev
}

/// A build file, placed: its real path, and its bytes when ingest read them.
struct Placed<'a> {
    path: PathBuf,
    bytes: Option<&'a [u8]>,
}

/// Absolute, symlinks resolved, one entry per file. A file named twice keeps
/// the bytes ingest read, if either name had them.
fn prepare<'a>(req: &Request<'a>) -> Vec<Placed<'a>> {
    let start = req.start.canonicalize().unwrap_or_else(|_| req.start.clone());
    let mut by_path: BTreeMap<PathBuf, Option<&'a [u8]>> = BTreeMap::new();
    for f in &req.files {
        if f.path.as_os_str().is_empty() {
            continue;
        }
        let abs = if f.path.is_absolute() { f.path.clone() } else { start.join(&f.path) };
        let e = by_path.entry(real_path(&abs)).or_insert(None);
        if e.is_none() {
            *e = f.bytes;
        }
    }
    by_path.into_iter().map(|(path, bytes)| Placed { path, bytes }).collect()
}

/// `canonicalize`, or -- for a file that no longer exists -- its nearest
/// existing ancestor canonicalised with the rest appended lexically.
fn real_path(p: &Path) -> PathBuf {
    if let Ok(c) = p.canonicalize() {
        return c;
    }
    let mut lexical = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                lexical.pop();
            }
            Component::CurDir => {}
            other => lexical.push(other),
        }
    }
    let mut rest: Vec<OsString> = Vec::new();
    let mut cur = lexical.as_path();
    loop {
        if let Ok(c) = cur.canonicalize() {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return out;
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                cur = parent;
            }
            _ => return lexical,
        }
    }
}

// ---- git ------------------------------------------------------------------

struct Git<'r> {
    exe: &'r Path,
    bounds: Bounds,
}

/// Why a git call gave no answer, in a sentence.
fn why(e: &HelperError, what: &str) -> String {
    match e {
        HelperError::Unresolved(_) => "git was not found on PATH".into(),
        HelperError::Spawn { tool, error } => format!("git could not be run at {}: {error}", tool.display()),
        HelperError::Failed { stderr, .. } => {
            let first: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).take(2).collect();
            let text: String = first.join(" ").chars().take(300).collect();
            if stderr.contains("dubious ownership") {
                format!("git refused the repository for dubious ownership: it belongs to another user and is not listed in safe.directory ({text})")
            } else {
                format!("git {what} failed: {text}")
            }
        }
        other => format!("git {what}: {other}"),
    }
}

impl Git<'_> {
    /// `git --no-optional-locks -c core.fsmonitor=false -C <dir> <args>`,
    /// through the bounded helper policy (#246), whose cleared environment
    /// already removes `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and
    /// `GIT_CONFIG_*`. Every exit status and both streams come back.
    fn call(&self, dir: &Path, args: &[&str]) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), HelperError> {
        let mut argv: Vec<&OsStr> = vec!["--no-optional-locks".as_ref(), "-c".as_ref(), "core.fsmonitor=false".as_ref(), "-C".as_ref(), dir.as_os_str()];
        argv.extend(args.iter().map(OsStr::new));
        helper::run_any(self.exe, &argv, None, self.bounds)
    }

    /// Only a zero exit is an answer.
    fn ok(&self, dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
        let what = args.first().copied().unwrap_or("");
        match self.call(dir, args) {
            Ok((st, out, _)) if st.success() => Ok(out),
            Ok((status, _, stderr)) => Err(why(&HelperError::Failed { tool: self.exe.to_path_buf(), status, stderr: String::from_utf8_lossy(&stderr).into_owned() }, what)),
            Err(e) => Err(why(&e, what)),
        }
    }
}

/// One line of output, without its line ending.
fn line(out: &[u8]) -> Vec<u8> {
    let mut v = out.to_vec();
    if v.last() == Some(&b'\n') {
        v.pop();
    }
    v
}

fn git_inspect(req: &Request, files: &[Placed]) -> SourceRevision {
    let Some(exe) = req.git.as_deref() else {
        return SourceRevision::unknown(Vcs::Git, "git was not found on PATH");
    };
    let git = Git { exe, bounds: req.bounds };
    let start = req.start.canonicalize().unwrap_or_else(|_| req.start.clone());

    // The first call says whether git runs at all; the version is recorded
    // whenever it did.
    let first = git.call(&start, &["rev-parse", "--show-toplevel"]);
    let tool = match &first {
        Err(HelperError::Unresolved(_) | HelperError::Spawn { .. }) => None,
        _ => Some(crate::provenance::identify("git", exe).0),
    };
    let mut rev = SourceRevision { tool: tool.clone(), ..SourceRevision::unknown(Vcs::Git, "") };
    let top = match first {
        Ok((st, out, _)) if st.success() => PathBuf::from(OsString::from_vec(line(&out))),
        Ok((status, _, stderr)) => {
            rev.state = SourceState::Unknown(why(&HelperError::Failed { tool: exe.to_path_buf(), status, stderr: String::from_utf8_lossy(&stderr).into_owned() }, "rev-parse"));
            return rev;
        }
        Err(e) => {
            rev.state = SourceState::Unknown(why(&e, "rev-parse"));
            return rev;
        }
    };
    let top = top.canonicalize().unwrap_or(top);
    rev.root = Some(top.display().to_string());
    rev.repository = remote(&git, &top);

    let repo = match open_repo(&git, &top) {
        Ok(r) => r,
        Err((reason, head)) => {
            rev.id = head.map(|revision| RevisionId { revision, ..Default::default() });
            rev.state = SourceState::Unknown(reason);
            return rev;
        }
    };
    rev.id = Some(RevisionId { revision: repo.head.clone(), ..Default::default() });
    rev.notes.extend(repo.notes.iter().cloned());

    // outside the working copy: not under version control, not judged
    let (inside, outside): (Vec<&Placed>, Vec<&Placed>) = files.iter().partition(|f| f.path.starts_with(&top));
    let mut unversioned: Vec<String> = outside.iter().map(|f| f.path.display().to_string()).collect();
    unversioned.sort();
    rev.unversioned_count = unversioned.len() as u64;
    unversioned.truncate(LIST_CAP);
    rev.unversioned = unversioned;

    let mut judged = Judged::default();
    judge(&git, &top, &repo, &inside, "", req, 0, &mut judged);

    let mut modified = judged.modified;
    modified.sort_by(|a, b| a.path.cmp(&b.path));
    modified.dedup_by(|a, b| a.path == b.path);
    rev.modified_count = modified.len() as u64;
    modified.truncate(LIST_CAP);
    rev.modified = modified;
    rev.verified_count = judged.verified;
    rev.outside_build = judged.outside;
    rev.submodules = judged.submodules;
    rev.notes.extend(judged.notes);
    for d in &judged.doubts {
        if !rev.notes.contains(d) {
            rev.notes.push(d.clone());
        }
    }
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

/// `remote.origin.url`, with any credentials in it removed.
fn remote(git: &Git, top: &Path) -> Option<String> {
    let out = git.ok(top, &["config", "--get", "remote.origin.url"]).ok()?;
    let url = String::from_utf8_lossy(&line(&out)).trim().to_string();
    (!url.is_empty()).then(|| strip_credentials(&url))
}

/// A URL without its user information, query or fragment -- where tokens
/// and passwords are put. An scp-style `user@host:path` keeps its user,
/// which is a login name, unless it carries a password too.
pub fn strip_credentials(url: &str) -> String {
    let url = url.split(['?', '#']).next().unwrap_or(url);
    if let Some(i) = url.find("://") {
        let (scheme, rest) = url.split_at(i + 3);
        let end = rest.find('/').unwrap_or(rest.len());
        let (authority, path) = rest.split_at(end);
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        return format!("{scheme}{host}{path}");
    }
    match url.split_once('@') {
        Some((user, host)) if user.contains(':') => host.to_string(),
        _ => url.to_string(),
    }
}

/// A repository whose HEAD and tree have been read.
struct Repo {
    head: String,
    tree: HashMap<Vec<u8>, Entry>,
    /// Remarks that do not stop the comparison (a shallow clone, an
    /// `index.lock`).
    notes: Vec<String>,
}

struct Entry {
    mode: Vec<u8>,
    oid: String,
}

const GITLINK: &[u8] = b"160000";

/// HEAD's commit and tree. `Err` carries the reason and, when HEAD could be
/// read, its commit.
fn open_repo(git: &Git, top: &Path) -> Result<Repo, (String, Option<String>)> {
    // HEAD itself, not peeled: peeling reads the object, and a missing
    // commit object is a corrupt repository, not an empty one -- the tree
    // read below reports that
    let head = match git.call(top, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
        Ok((st, out, _)) if st.success() => String::from_utf8_lossy(&line(&out)).trim().to_string(),
        // --quiet: a HEAD that names no commit yet exits 1 and says nothing
        Ok((st, _, err)) if st.code() == Some(1) && err.iter().all(u8::is_ascii_whitespace) => {
            return Err(("the repository has no commits yet, so there is no revision to compare with".into(), None))
        }
        Ok((status, _, stderr)) => {
            return Err((why(&HelperError::Failed { tool: git.exe.to_path_buf(), status, stderr: String::from_utf8_lossy(&stderr).into_owned() }, "rev-parse HEAD"), None))
        }
        Err(e) => return Err((why(&e, "rev-parse HEAD"), None)),
    };
    if !is_object_id(&head) {
        return Err((format!("git named HEAD as {head:?}, which is not a commit id"), None));
    }
    // The tree of the commit just recorded, by its id -- not `HEAD` again:
    // a commit landing in between would otherwise have the record name one
    // commit while the files are judged against another's tree (#306 review).
    let listing = git.ok(top, &["ls-tree", "-r", "-z", "--full-tree", &head]).map_err(|e| (format!("reading the tree of {head}: {e}"), Some(head.clone())))?;
    let mut tree = HashMap::new();
    for rec in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        // <mode> SP <type> SP <oid> TAB <path>
        let Some(tab) = rec.iter().position(|b| *b == b'\t') else { continue };
        let meta: Vec<&[u8]> = rec[..tab].split(|b| *b == b' ').collect();
        if meta.len() != 3 {
            continue;
        }
        tree.insert(rec[tab + 1..].to_vec(), Entry { mode: meta[0].to_vec(), oid: String::from_utf8_lossy(meta[2]).to_ascii_lowercase() });
    }
    let mut notes = Vec::new();
    if git.ok(top, &["rev-parse", "--is-shallow-repository"]).map(|o| line(&o) == b"true").unwrap_or(false) {
        notes.push("the repository is a shallow clone: its history is truncated, but the commit id and the file comparison are exact".into());
    }
    if let Ok(p) = git.ok(top, &["rev-parse", "--git-path", "index.lock"]) {
        let p = PathBuf::from(OsString::from_vec(line(&p)));
        let p = if p.is_absolute() { p } else { top.join(p) };
        if p.exists() {
            // A note, not a doubt: the files are compared with the tree of
            // the commit recorded, by its id, which no index or lock can
            // change, and a checkout caught mid-way shows as files that do
            // not match it.
            notes.push(format!(
                "{} exists: a git operation is under way in this working copy, or one was interrupted; the files were still compared with the recorded commit's own tree",
                p.display()
            ));
        }
    }
    Ok(Repo { head, tree, notes })
}

#[derive(Default)]
struct Judged {
    modified: Vec<ChangedSource>,
    verified: u64,
    /// `None` once any repository's outside count could not be made.
    outside: Option<u64>,
    submodules: Vec<SubmoduleRevision>,
    notes: Vec<String>,
    doubts: Vec<String>,
}

/// Nested submodules deeper than this are not followed.
const MAX_DEPTH: usize = 8;

/// Judge `files` (all inside `top`) against `repo`, recursing into the
/// submodules that hold any of them. `prefix` places this repository's
/// paths under the top-level working copy.
#[allow(clippy::too_many_arguments)]
fn judge(git: &Git, top: &Path, repo: &Repo, files: &[&Placed], prefix: &str, req: &Request, depth: usize, out: &mut Judged) {
    let gitlinks: BTreeSet<&[u8]> = repo.tree.iter().filter(|(_, e)| e.mode == GITLINK).map(|(p, _)| p.as_slice()).collect();
    let show = |rel: &[u8]| format!("{prefix}{}", String::from_utf8_lossy(rel));
    let mut here: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut in_sub: BTreeMap<&[u8], Vec<&Placed>> = BTreeMap::new();
    for f in files {
        let rel = f.path.strip_prefix(top).unwrap_or(&f.path).as_os_str().as_bytes().to_vec();
        // the longest submodule path this file is inside
        let sub = gitlinks.iter().filter(|g| rel.len() > g.len() && rel.starts_with(g) && rel[g.len()] == b'/').max_by_key(|g| g.len());
        if let Some(g) = sub {
            in_sub.entry(g).or_default().push(f);
            continue;
        }
        here.insert(rel.clone());
        out.verified += 1;
        let Some(entry) = repo.tree.get(&rel) else {
            out.modified.push(ChangedSource { path: show(&rel), change: Change::NotInRevision });
            continue;
        };
        if entry.mode != b"100644" && entry.mode != b"100755" {
            // A symlink, or a submodule, in the revision where the build read
            // a regular file (every path here is canonical): not this file,
            // even when the link's text is its bytes and the blob ids agree
            // (#314).
            out.modified.push(ChangedSource { path: show(&rel), change: Change::Content });
            continue;
        }
        let read;
        let bytes = match f.bytes {
            Some(b) => b,
            None => match read_file(&f.path, req.max_file_bytes) {
                Ok(Some(b)) => {
                    read = b;
                    read.as_slice()
                }
                Ok(None) => {
                    out.modified.push(ChangedSource { path: show(&rel), change: Change::Missing });
                    continue;
                }
                Err(e) => {
                    out.verified -= 1;
                    out.doubts.push(format!("{} could not be read to compare with the revision: {e}", show(&rel)));
                    continue;
                }
            },
        };
        match compare(bytes, &entry.oid) {
            Verdict::Same => {}
            Verdict::Collision => {
                out.verified -= 1;
                out.doubts.push(format!("{}: its SHA-1 shows the signature of a collision attack, so a matching blob id proves nothing", show(&rel)));
            }
            Verdict::LineEndings => out.modified.push(ChangedSource { path: show(&rel), change: Change::LineEndings }),
            Verdict::Differs => out.modified.push(ChangedSource { path: show(&rel), change: Change::Content }),
        }
    }

    // changes the build does not use
    let outside = if depth == 0 || out.outside.is_some() { count_outside(git, top, repo, &here, &gitlinks, req) } else { Err(String::new()) };
    match outside {
        Ok(n) => out.outside = Some(out.outside.unwrap_or(0) + n),
        Err(e) if !e.is_empty() => {
            out.outside = None;
            out.notes.push(format!(
                "changes outside the build were not counted{}: {e}",
                if prefix.is_empty() { String::new() } else { format!(" in {}", prefix.trim_end_matches('/')) }
            ));
        }
        Err(_) => {}
    }

    for (g, sub_files) in in_sub {
        let path = show(g);
        let recorded = repo.tree.get(g).map(|e| e.oid.clone()).unwrap_or_default();
        let sub_top = top.join(OsStr::from_bytes(g));
        let mut record = SubmoduleRevision { path: path.clone(), commit: None, recorded: recorded.clone() };
        if depth >= MAX_DEPTH {
            out.doubts.push(format!("{path}: submodules nested more than {MAX_DEPTH} deep are not followed"));
            out.submodules.push(record);
            continue;
        }
        if sub_top.join(".git").symlink_metadata().is_err() {
            out.doubts.push(format!("{path}: the submodule is not checked out, so its files cannot be compared"));
            out.submodules.push(record);
            continue;
        }
        match open_repo(git, &sub_top) {
            Ok(sub) => {
                record.commit = Some(sub.head.clone());
                if sub.head != recorded {
                    out.modified.push(ChangedSource { path: path.clone(), change: Change::SubmoduleCommit });
                }
                out.notes.extend(sub.notes.iter().map(|n| format!("{path}: {n}")));
                out.submodules.push(record);
                judge(git, &sub_top, &sub, &sub_files, &format!("{path}/"), req, depth + 1, out);
            }
            Err((reason, head)) => {
                record.commit = head;
                out.doubts.push(format!("{path}: {reason}"));
                out.submodules.push(record);
            }
        }
    }
}

enum Verdict {
    Same,
    LineEndings,
    Differs,
    Collision,
}

/// Compare `bytes` with the blob `oid` names, in the repository's object
/// format -- which the id's length says: 40 hex digits for SHA-1, 64 for
/// SHA-256.
fn compare(bytes: &[u8], oid: &str) -> Verdict {
    let (id, collided) = blob_id(bytes, oid.len() == 64);
    if collided {
        return Verdict::Collision;
    }
    if id == oid {
        return Verdict::Same;
    }
    let lf = crlf_to_lf(bytes);
    let crlf = lf_to_crlf(&lf);
    for alt in [lf, crlf] {
        if alt != bytes {
            let (id, collided) = blob_id(&alt, oid.len() == 64);
            if !collided && id == oid {
                return Verdict::LineEndings;
            }
        }
    }
    Verdict::Differs
}

/// Git's blob id for `bytes`: the hash of `blob <len>\0` then the bytes.
/// The second value is true when SHA-1's collision detection fired.
pub fn blob_id(bytes: &[u8], sha256: bool) -> (String, bool) {
    let header = format!("blob {}\0", bytes.len());
    let hex = |d: &[u8]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
    if sha256 {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(header.as_bytes());
        h.update(bytes);
        (hex(&h.finalize()), false)
    } else {
        use sha1_checked::Digest;
        let mut h = sha1_checked::Sha1::new();
        h.update(header.as_bytes());
        h.update(bytes);
        let r = h.try_finalize();
        (hex(r.hash()), r.has_collision())
    }
}

fn crlf_to_lf(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn lf_to_crlf(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len() + b.len() / 16);
    for &c in b {
        if c == b'\n' {
            out.push(b'\r');
        }
        out.push(c);
    }
    out
}

/// The provenance caveat for a revision that is not clean: modified and
/// unknown are always visible, and never a failure by themselves. An SVN
/// working copy's shape -- mixed revisions, switched paths, a sparse
/// checkout -- is part of the caveat, clean or not.
pub fn caveat(rev: &SourceRevision) -> Option<String> {
    let at = match (&rev.id, rev.vcs) {
        (Some(id), Vcs::Svn) => format!("SVN r{}{}", id.revision, id.path.as_deref().map(|p| format!(" of {p}")).unwrap_or_default()),
        (Some(id), _) => format!("{} {}", vcs_name(rev.vcs), id.revision),
        (None, _) => format!("the {} working copy", vcs_name(rev.vcs)),
    };
    let state = match &rev.state {
        // Clean covers the files the evidence names. A header included only
        // for its types or macros is named by neither the coverage export nor
        // the line tables, so any other difference is said, not kept in a
        // count nobody reads (#314).
        SourceState::Clean => match rev.outside_build {
            Some(0) => None,
            Some(n) => Some(format!(
                "The source revision is clean for the {} file(s) the evidence names, but {n} other file(s) in the working copy differ from {at} or are not in it. The evidence cannot show the build did not use them: a header included only for its types or macros is named by neither the coverage export nor the line tables.",
                rev.verified_count
            )),
            None => Some(format!(
                "The source revision is clean for the {} file(s) the evidence names, but whether other files in the working copy differ from {at} was not established. The evidence cannot show the build did not use them: a header included only for its types or macros is named by neither the coverage export nor the line tables.",
                rev.verified_count
            )),
        },
        SourceState::Modified => {
            let shown: Vec<&str> = rev.modified.iter().take(5).map(|c| c.path.as_str()).collect();
            let more = rev.modified_count.saturating_sub(shown.len() as u64);
            Some(format!(
                "The source revision is modified: {} file(s) the build used are not as {at} holds them ({}{}). The evidence describes the files as they were read, which no commit records.",
                rev.modified_count,
                shown.join(", "),
                if more > 0 { format!(", and {more} more") } else { String::new() }
            ))
        }
        SourceState::Unknown(why) => Some(format!("The source revision is unknown: {why}. Whether the files the build used are those of a recorded revision was not established.")),
    };
    let parts: Vec<String> = state.into_iter().chain(svn::shape(rev)).collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn vcs_name(v: Vcs) -> &'static str {
    match v {
        Vcs::Git => "git",
        Vcs::Svn => "SVN",
        Vcs::None => "no",
    }
}

/// A file's bytes, bounded; `Ok(None)` when it does not exist. Opened
/// non-blocking and checked to be a regular file on the descriptor, so a
/// FIFO named by the DWARF cannot stall the read.
fn read_file(path: &Path, max: u64) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let f = match std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let md = f.metadata().map_err(|e| e.to_string())?;
    if !md.is_file() {
        return Err("not a regular file".into());
    }
    if md.len() > max {
        return Err(format!("larger than the {} this reads", butterfly_model::limits::human(max)));
    }
    let mut buf = Vec::with_capacity(md.len() as usize);
    f.take(max.saturating_add(1)).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > max {
        return Err(format!("grew past the {} this reads", butterfly_model::limits::human(max)));
    }
    Ok(Some(buf))
}

/// How many paths outside the build differ from HEAD: tracked files whose
/// content differs or that are gone, and untracked files git does not
/// ignore. Tracked files are compared by content, as the build's are --
/// never through git's index, whose stat shortcuts can be told not to look
/// and whose content filters can run programs the repository names.
/// `Err` when the working copy is too large to read within the budget.
fn count_outside(git: &Git, top: &Path, repo: &Repo, build: &BTreeSet<Vec<u8>>, gitlinks: &BTreeSet<&[u8]>, req: &Request) -> Result<u64, String> {
    let mut n = 0u64;
    let mut spent = 0u64;
    for (rel, e) in &repo.tree {
        if build.contains(rel) || e.mode == GITLINK {
            continue;
        }
        let p = top.join(OsStr::from_bytes(rel));
        let md = match p.symlink_metadata() {
            Ok(m) => m,
            Err(_) => {
                n += 1;
                continue;
            }
        };
        let bytes = if md.file_type().is_symlink() {
            std::fs::read_link(&p).map(|t| t.into_os_string().into_vec()).ok()
        } else if md.is_file() {
            spent = spent.saturating_add(md.len());
            if spent > req.max_outside_bytes {
                return Err(format!("the tracked files outside the build hold more than {}", butterfly_model::limits::human(req.max_outside_bytes)));
            }
            read_file(&p, req.max_outside_bytes).ok().flatten()
        } else {
            None
        };
        match bytes {
            Some(b) if matches!(compare(&b, &e.oid), Verdict::Same) => {}
            _ => n += 1,
        }
    }
    let others = git.ok(top, &["ls-files", "-z", "--others", "--exclude-standard"])?;
    for rel in others.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let rel = rel.strip_suffix(b"/").unwrap_or(rel);
        if !build.contains(rel) && !gitlinks.contains(rel) {
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_ids_match_git_in_both_object_formats() {
        // `printf 'hello\n' | git hash-object --stdin` and the same with
        // `--object-format=sha256`
        assert_eq!(blob_id(b"hello\n", false).0, "ce013625030ba8dba906f756967f9e9ca394464a");
        assert_eq!(blob_id(b"hello\n", true).0, "2cf8d83d9ee29543b34a87727421fdecb7e3f3a183d337639025de576db9ebb4");
        assert_eq!(blob_id(b"", false).0, "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }

    #[test]
    fn credentials_are_removed_from_urls() {
        assert_eq!(strip_credentials("https://user:pw@example.com/a.git"), "https://example.com/a.git");
        assert_eq!(strip_credentials("https://ghp_token@example.com/a.git?x=1"), "https://example.com/a.git");
        assert_eq!(strip_credentials("ssh://git@example.com:22/a.git"), "ssh://example.com:22/a.git");
        assert_eq!(strip_credentials("git@example.com:team/a.git"), "git@example.com:team/a.git");
        assert_eq!(strip_credentials("user:pw@example.com:team/a.git"), "example.com:team/a.git");
        assert_eq!(strip_credentials("/srv/repos/a.git"), "/srv/repos/a.git");
    }

    /// The helper's cleared environment is what keeps `GIT_DIR`,
    /// `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_CONFIG_*` from redirecting
    /// the git this runs.
    #[test]
    fn no_git_variable_reaches_the_git_this_runs() {
        assert!(helper::PASSED_ENV.iter().all(|k| !k.starts_with("GIT_")));
    }

    #[test]
    fn line_ending_conversions() {
        assert_eq!(crlf_to_lf(b"a\r\nb\r\n\r"), b"a\nb\n\r");
        assert_eq!(lf_to_crlf(b"a\nb"), b"a\r\nb");
    }
}
