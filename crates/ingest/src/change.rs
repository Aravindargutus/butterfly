// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Change impact: which commits, and through them which tickets, touched
//! each function (DO-178C 12.1.1 change impact analysis, 7.2.4 problem
//! reporting and change control) — and, per ticket, what it added, modified
//! and deleted, across every file it touched.
//!
//! Attribution is function-centric: for every function in the evidence,
//! `git log -L <start>,<end>:<path>` over the *whole* history lists the
//! commits that touched those lines, following the range through later
//! edits above it and through file renames — which a diff-hunk mapping onto
//! today's line numbers cannot do. The oldest of those commits is the
//! function's origin, which is how "added by this ticket" is told apart from
//! "modified by it". `--since` does not change what is walked; it selects the
//! commits reported, so a different baseline never costs a re-walk.
//!
//! Two things a walk over today's functions cannot see are found otherwise:
//! * **Deleted functions.** The frozen baseline (`butterfly baseline`) records
//!   where every certified function was. For one that no longer exists,
//!   `git blame --reverse` says the last commit its lines existed in, and the
//!   first commit after that to touch the file is the one that deleted it.
//! * **Files without functions.** Each commit's `--name-status` is kept, so a
//!   ticket that changed a header, a linker script or a build file — code
//!   that alters every includer's object code without touching a function's
//!   lines — is reported as touching a file nothing in the evidence traces to.
//!
//! Results are written into the evidence and into a cache next to it, keyed
//! by (file, function, line range) with the file's SHA-256 and the HEAD it was
//! read at. A re-run walks only functions in files changed since that HEAD;
//! `ingest` re-embeds the cache without touching git as long as every file
//! hash still matches, and says so when one does not.

use anyhow::{anyhow, bail, Context, Result};
use butterfly_model::{Baseline, ChangedFile, Changes, Commit, DeletedFunction, Evidence, FileChange, Ticket, TicketFile, CHANGES_SCHEMA};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

pub struct Options {
    /// Report `<since>..HEAD` only. `None` = the whole history.
    pub since: Option<String>,
    /// Repository to read; found from the analysed source paths when `None`.
    pub repo: Option<PathBuf>,
    /// Ticket regexes: capture group 1, or the whole match.
    pub patterns: Vec<String>,
    /// Glob patterns on repository-relative paths that are not walked.
    pub exclude: Vec<String>,
    /// Concurrent `git` processes; `0` = one per CPU.
    pub jobs: usize,
    /// Frozen baseline (`butterfly baseline`) from the certified build, used
    /// to find functions that no longer exist.
    pub baseline: Option<PathBuf>,
}

/// The on-disk cache (`butterfly-changes.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cache {
    pub schema: String,
    pub repo: String,
    pub head: String,
    pub since: Option<String>,
    pub since_sha: Option<String>,
    /// Commits in `since..head` at the time of the run; empty with no `since`.
    #[serde(default)]
    pub range: Vec<String>,
    pub created_unix: u64,
    pub patterns: Vec<String>,
    /// Every commit referenced by some function or deletion, over the whole
    /// history, with the full message so tickets can be re-extracted.
    pub commits: Vec<CachedCommit>,
    pub files: Vec<CachedFile>,
    #[serde(default)]
    pub deleted: Vec<CachedDeleted>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedCommit {
    pub sha: String,
    pub author: String,
    pub date: String,
    /// Unix timestamp of the author date: the sort key.
    pub date_unix: u64,
    pub subject: String,
    pub message: String,
    /// `--name-status` of the commit; `None` when not yet read.
    #[serde(default)]
    pub files: Option<Vec<FileChange>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedFile {
    /// Path as recorded in the evidence.
    pub path: String,
    pub repo_path: String,
    pub sha256: String,
    pub status: String,
    pub functions: Vec<CachedFn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedFn {
    pub name: String,
    pub line_start: u32,
    pub line_end: u32,
    /// Every commit that touched the range, whole history, newest first.
    pub commits: Vec<String>,
    /// The oldest of them: the commit that introduced the lines.
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedDeleted {
    pub name: String,
    pub repo_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub commit: Option<String>,
}

/// What a run did, for the progress line.
#[derive(Debug, Default)]
pub struct Outcome {
    pub walked: usize,
    pub reused: usize,
    pub skipped: usize,
    pub deleted: usize,
}

/// `items.iter().map(f)` on up to `jobs` threads, results in input order.
/// Used for the many short git calls: they are independent and git is happy
/// with concurrent readers. `jobs == 0` means one worker per CPU.
fn parallel_map<T: Sync, R: Send>(jobs: usize, items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = effective_jobs(jobs).min(items.len().max(1));
    if workers <= 1 || items.len() <= 1 {
        return items.iter().map(&f).collect();
    }
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(it) = items.get(i) else { break };
                let r = f(it);
                results.lock().unwrap().push((i, r));
            });
        }
    });
    let mut v = results.into_inner().unwrap();
    v.sort_by_key(|(i, _)| *i);
    v.into_iter().map(|(_, r)| r).collect()
}

/// `0` = one worker per CPU (capped at 16: past that git is I/O bound).
pub fn effective_jobs(jobs: usize) -> usize {
    if jobs == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16)
    } else {
        jobs
    }
}

fn walkable(status: &str) -> bool {
    matches!(status, "matches-head" | "matches-worktree" | "mismatch")
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The git binary, resolved once by the same executable-matching lookup that
/// records it in provenance, and cached. Running the resolved path (rather than
/// the bare name again) guarantees the git that ran is the git that was
/// recorded, even when an earlier PATH entry holds a non-executable `git`.
pub fn git_exe() -> &'static Path {
    static GIT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    GIT.get_or_init(|| crate::provenance::resolve_on_path("git").unwrap_or_else(|| PathBuf::from("git")))
}

fn git_raw(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    // bounded, with a cleaned environment, like every helper (#246): a
    // GIT_DIR in the operator's shell cannot redirect it, and a history that
    // will not end stops at the deadline or the output ceiling
    let mut argv: Vec<&std::ffi::OsStr> = vec!["-C".as_ref(), repo.as_os_str()];
    argv.extend(args.iter().map(|a| std::ffi::OsStr::new(*a)));
    match crate::helper::run(git_exe(), &argv, None, crate::helper::Bounds::standard()) {
        Ok(out) => Ok(out),
        Err(crate::helper::HelperError::Unresolved(_)) => bail!("git was not found on PATH (is it installed?)"),
        Err(crate::helper::HelperError::Failed { stderr, .. }) => bail!("git {}: {}", args.join(" "), stderr.trim()),
        Err(e) => bail!("git {}: {e}", args.join(" ")),
    }
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git_raw(repo, args)?).trim_end().to_string())
}

/// Repository top level, canonicalised so it can be a prefix of the
/// canonicalised source paths.
fn find_repo(ev: &Evidence, opts: &Options) -> Result<PathBuf> {
    let start = match &opts.repo {
        Some(r) => r.clone(),
        None => ev
            .files
            .iter()
            .map(|f| PathBuf::from(&f.path))
            .filter(|p| p.is_absolute())
            .find_map(|p| p.ancestors().skip(1).find(|a| a.is_dir()).map(Path::to_path_buf))
            .ok_or_else(|| anyhow!("none of the analysed source files exists on this machine; pass --repo"))?,
    };
    let top = git(&start, &["rev-parse", "--show-toplevel"]).map_err(|e| anyhow!("{} is not inside a git repository ({e})", start.display()))?;
    PathBuf::from(&top).canonicalize().with_context(|| format!("canonicalising {top}"))
}

/// Repository-relative path of an analysed file, or `None` when it lies
/// outside the repository.
fn rel(repo: &Path, path: &str) -> Option<String> {
    let p = Path::new(path);
    let abs = if p.is_absolute() { p.to_path_buf() } else { repo.join(p) };
    let abs = abs.canonicalize().unwrap_or(abs);
    abs.strip_prefix(repo).ok().map(|r| r.to_string_lossy().replace('\\', "/"))
}

/// gitignore-flavoured glob: `*` within a path component, `**` across
/// components, `?` one character; a pattern with no `/` matches at any
/// depth; a match on a directory covers everything below it.
fn glob_re(g: &str) -> Result<Regex> {
    let anchored = g.contains('/');
    let mut r = String::from(if anchored { "^" } else { "(?:^|/)" });
    let cs: Vec<char> = g.trim_start_matches('/').chars().collect();
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '*' if i + 1 < cs.len() && cs[i + 1] == '*' => {
                r.push_str(".*");
                i += 2;
                if i < cs.len() && cs[i] == '/' {
                    i += 1;
                }
                continue;
            }
            '*' => r.push_str("[^/]*"),
            '?' => r.push_str("[^/]"),
            c => r.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    r.push_str("(?:$|/)");
    Regex::new(&r).with_context(|| format!("exclude pattern {g:?}"))
}

fn compile(patterns: &[String]) -> Result<Vec<Regex>> {
    patterns.iter().map(|p| Regex::new(p).with_context(|| format!("ticket pattern {p:?}"))).collect()
}

/// Tickets named in a commit message, in order of first appearance.
pub fn tickets_in(res: &[Regex], message: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for re in res {
        for c in re.captures_iter(message) {
            let t = c.get(1).map(|m| m.as_str()).unwrap_or_else(|| c.get(0).unwrap().as_str());
            if !t.is_empty() && !out.iter().any(|x| x == t) {
                out.push(t.to_string());
            }
        }
    }
    out
}

/// Each record starts with the separator so that, for `log -L`, the diff git
/// prints after the body stays attached to its commit.
const FMT: &str = "--format=%x1e%H%x1f%an%x1f%aI%x1f%at%x1f%s%x1f%B";

/// Records, plus whatever git printed after each body (the diff, for -L).
fn parse_records(text: &str) -> Vec<(CachedCommit, String)> {
    let mut out = Vec::new();
    for rec in text.split('\x1e') {
        let rec = rec.trim_start_matches('\n');
        if rec.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.splitn(6, '\x1f').collect();
        if f.len() < 6 {
            continue;
        }
        // the body ends where git's diff output begins
        let (message, diff) = match f[5].find("\ndiff --git ") {
            Some(i) => (&f[5][..i], &f[5][i + 1..]),
            None => (f[5], ""),
        };
        out.push((
            CachedCommit {
                sha: f[0].trim().to_string(),
                author: f[1].to_string(),
                date: f[2].to_string(),
                date_unix: f[3].trim().parse().unwrap_or(0),
                subject: f[4].to_string(),
                message: message.trim_end().to_string(),
                files: None,
            },
            diff.to_string(),
        ));
    }
    out
}

fn parse_commits(text: &str) -> Vec<CachedCommit> {
    parse_records(text).into_iter().map(|(c, _)| c).collect()
}

/// The part of a signature line that identifies the function rather than its
/// body: everything up to and including the parameter list's closing paren.
/// A one-line function's whole line changes when its body changes, so the
/// full line is not an identity -- matching on it made an ordinary edit look
/// like the function's introduction (#13).
fn signature_identity(line: &str) -> &str {
    let t = line.trim();
    match params_open(t).and_then(|o| t[o..].find(')').map(|c| o + c + 1)) {
        Some(end) => t[..end].trim_end(),
        None => t,
    }
}

/// A C++ operator's declared name: `bool C::operator==` -> `C::operator==`.
///
/// The identifier scan walks back over letters, digits, `_`, `:` and `~`,
/// and an operator's name ends in none of those. `bool C::operator==(const
/// C& old_name)` therefore yielded no name at all, the caller fell back to
/// the full signature, and renaming a parameter reported the operator as a
/// newly added function -- on C++, which butterfly advertises (second review
/// of #13).
///
/// Handled before the generic-list step, because `bool operator>(…)` ends in
/// a `>` that is an operator, not the close of a parameter list.
///
/// The keyword has to be a word of its own: `operator_new` is an ordinary
/// identifier, and `operator new`, `operator bool` and `operator""_km` are
/// not. Whatever follows the keyword up to the parameter list is part of the
/// name, so `operator==`, `operator[]`, `operator new[]` and `operator bool`
/// each identify as themselves.
fn operator_name(before: &str) -> Option<&str> {
    let t = before.trim_end();
    let k = t.rfind("operator")?;
    // preceded by nothing identifier-ish: not the tail of `my_operator`
    if t[..k].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let after = &t[k + "operator".len()..];
    // `operatorfoo` is one identifier; `operator new` and `operator==` are not
    if after.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        return None;
    }
    // keep any `Class::` qualification in front of the keyword
    let start = k - t[..k].chars().rev().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '~')).map(char::len_utf8).sum::<usize>();
    let name = t[start..].trim_start_matches(':');
    (!name.is_empty()).then_some(name)
}

/// The `(` that opens the parameter list.
///
/// Usually the first one on the line, but Rust's restricted visibility puts
/// a parenthesised group before the name: `pub(crate) fn f(x: T)`. Taking
/// the first `(` there made every such function's identity `pub`, and its
/// pickaxe search `-Spub(crate)` -- so changing `pub fn f` to `pub(crate) fn
/// f` read as an introduction, and the origin found by pickaxe was the first
/// commit to use restricted visibility anywhere in the file (self-review of
/// #13). Skip a group that directly follows `pub`.
fn params_open(t: &str) -> Option<usize> {
    let mut from = 0;
    loop {
        let open = from + t[from..].find('(')?;
        if !t[..open].trim_end().ends_with("pub") {
            return Some(open);
        }
        from = open + t[open..].find(')').map_or(1, |c| c + 1);
    }
}

/// The declared name in a signature line: the identifier immediately before
/// the parameter list, keeping any `::` qualification and a leading `~`.
///
/// `signature_identity` keeps the parameter list, so renaming a parameter or
/// changing its type produced an identity that had never been removed, and
/// the editing commit was taken for the function's introduction -- reporting
/// an existing function as `added` (review of #13). The name survives every
/// ordinary signature edit, which is exactly what an identity has to do.
///
/// `int f(int x)` -> `f`, `void Klass::method(int) const` -> `Klass::method`,
/// `pub fn compute(&self) -> i32` -> `compute`, `pub fn f<T>(x: T)` -> `f`.
/// Returns `None` for a line with no identifier before the paren (a function
/// pointer declarator, a macro), and the caller keeps the full-signature
/// behaviour there.
fn name_identity(line: &str) -> Option<&str> {
    let t = line.trim();
    let open = params_open(t)?;
    if let Some(op) = operator_name(&t[..open]) {
        return Some(op);
    }
    let head = skip_generics(&t[..open])?;
    let start = head.len() - head.chars().rev().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '~')).map(char::len_utf8).sum::<usize>();
    let name = head[start..].trim_start_matches(':');
    (!name.is_empty() && name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '~')).then_some(name)
}

/// Everything before a generic parameter list that belongs to the name.
///
/// `pub fn f<T>(…)` put a `>` where the identifier scan expected the last
/// letter of the name, so it found nothing and the caller fell back to the
/// full signature -- and renaming a parameter of a generic function reset
/// its identity again, on a language butterfly advertises (review of #13).
///
/// The list is only the name's when it abuts the parameter list. A space, as
/// in `std::map<int,int> (*fp)(void)`, means the `<…>` closed a type and
/// there is no name to take; an unbalanced `>`, as in `bool operator>`, is
/// not a list at all. Both keep the previous answer of `None`.
fn skip_generics(before: &str) -> Option<&str> {
    if !before.ends_with('>') {
        return Some(before.trim_end());
    }
    let mut depth = 0usize;
    for (i, c) in before.char_indices().rev() {
        match c {
            '>' => depth += 1,
            '<' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(before[..i].trim_end());
                }
            }
            _ => {}
        }
    }
    None
}

/// What one function's history walk produced: its commits, and the origin
/// commit when the `-L` diffs settled it.
type WalkResult = Result<(Vec<CachedCommit>, Option<String>)>;

/// `git log -L` over one function, whole history, newest first, with each
/// commit's diff of the range. The origin is read from those diffs: the
/// oldest commit whose diff *adds* the function's signature line. `-L` alone
/// cannot say -- when a commit edits the function above and adds this one in
/// the same hunk, the line-log traces the new lines back through that hunk
/// to whatever created the file -- but the diff text can, and it costs no
/// second git call. Returns `(commits, origin)`; origin is `None` when no
/// diff adds the signature (too short to trust, or changed since), and the
/// caller falls back to a pickaxe search.
fn log_full(repo: &Path, head: &str, repo_path: &str, ls: u32, le: u32, signature: Option<&str>) -> Result<(Vec<CachedCommit>, Option<String>)> {
    let spec = format!("{ls},{le}:{repo_path}");
    let recs = parse_records(&git(repo, &["log", "-L", &spec, FMT, head])?);
    // The declared name where the line has one, the full signature otherwise.
    // Matching on the name means a signature edit shows the identity both
    // added and removed, so it reads as a modification rather than an
    // introduction (review of #13).
    fn key(line: &str) -> &str {
        name_identity(line).unwrap_or_else(|| signature_identity(line))
    }
    // A name is an identity however short; a fallback full signature still
    // has to be long enough not to match by accident.
    let sig = signature.filter(|s| name_identity(s).is_some() || signature_identity(s).len() >= 8).map(key);
    // The origin introduced the function: its diff adds the identity and does
    // not also remove it. A commit that does both edited an existing function
    // -- for a one-liner that is every body change, and for any function that
    // is every signature edit.
    let origin = sig.and_then(|sig| {
        recs.iter()
            .rev()
            .find(|(_, diff)| {
                let (mut added, mut removed) = (false, false);
                for l in diff.lines() {
                    if let Some(x) = l.strip_prefix('+') {
                        added |= key(x) == sig;
                    } else if let Some(x) = l.strip_prefix('-') {
                        removed |= key(x) == sig;
                    }
                }
                added && !removed
            })
            .map(|(c, _)| c.sha.clone())
    });
    Ok((recs.into_iter().map(|(c, _)| c).collect(), origin))
}

/// Fallback origin: the oldest commit in which the signature line appeared
/// in the file (pickaxe), following renames. A whole-history scan, so it is
/// only run when the -L diffs did not settle it.
fn origin_of(repo: &Path, head: &str, repo_path: &str, signature: Option<&str>, oldest_l: Option<&str>) -> Option<String> {
    let sig = signature.map(signature_identity).filter(|s| s.len() >= 8)?;
    let arg = format!("-S{sig}");
    let text = git(repo, &["log", "--follow", "--format=%H", &arg, head, "--", repo_path]).ok()?;
    text.lines().last().map(|l| l.trim().to_string()).filter(|l| butterfly_model::revision::is_object_id(l)).or_else(|| oldest_l.map(str::to_string))
}

fn show_commit(repo: &Path, sha: &str) -> Result<CachedCommit> {
    if !butterfly_model::revision::is_object_id(sha) {
        bail!("invalid commit id {sha:?}");
    }
    parse_commits(&git(repo, &["show", "-s", FMT, "--end-of-options", sha])?).into_iter().next().ok_or_else(|| anyhow!("git show {sha}: no record"))
}

/// `--name-status` of one commit, renames detected. Statuses are spelled
/// out: `added` | `modified` | `deleted` | `renamed`.
fn commit_files(repo: &Path, sha: &str) -> Result<Vec<FileChange>> {
    if !butterfly_model::revision::is_object_id(sha) {
        bail!("invalid commit id {sha:?}");
    }
    let text = git(repo, &["show", "--name-status", "--format=", "-M", "--end-of-options", sha])?;
    let mut out = Vec::new();
    for line in text.lines() {
        let mut it = line.split('\t');
        let Some(st) = it.next() else { continue };
        let paths: Vec<&str> = it.collect();
        let (status, path) = match (st.chars().next(), paths.as_slice()) {
            (Some('A'), [p]) | (Some('C'), [_, p]) => ("added", *p),
            (Some('M'), [p]) | (Some('T'), [p]) => ("modified", *p),
            (Some('D'), [p]) => ("deleted", *p),
            (Some('R'), [_, p]) => ("renamed", *p),
            (_, [p, ..]) => ("modified", *p),
            _ => continue,
        };
        out.push(FileChange { path: path.to_string(), status: status.to_string() });
    }
    Ok(out)
}

/// Where a deleted function's lines last existed, and the commit that
/// removed them. `-L` needs the range as it was at `since`.
fn find_deletion(repo: &Path, since_sha: &str, head: &str, repo_path: &str, ls: u32, le: u32) -> Result<Option<String>> {
    let text = git(repo, &["blame", "--reverse", "--porcelain", "-L", &format!("{ls},{le}"), &format!("{since_sha}..{head}"), "--", repo_path])?;
    // porcelain: a header line "<sha> <orig> <final> [<n>]" per group
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for line in text.lines() {
        let sha = line.split(' ').next().unwrap_or("");
        if butterfly_model::revision::is_object_id(sha) {
            *counts.entry(sha).or_default() += 1;
        }
    }
    // Lines still present at HEAD say nothing about the deletion: a function
    // renamed in place keeps its body, so most of its lines survive and only
    // the signature's last existed before the rename (#325 D2). The removal
    // is where the lines that did go last existed.
    // If they all still exist at HEAD it was not deleted after all (moved, or
    // the baseline is stale for this function).
    let Some((last, _)) = counts.into_iter().filter(|(s, _)| *s != head).max_by_key(|(s, n)| (*n, s.to_string())) else { return Ok(None) };
    let next = git(repo, &["log", "--ancestry-path", "--reverse", "--format=%H", &format!("{last}..{head}"), "--", repo_path])?;
    Ok(next.lines().next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

struct Meta {
    repo: String,
    head: String,
    since: Option<String>,
    since_sha: Option<String>,
    range: Option<HashSet<String>>,
    created_unix: u64,
    patterns: Vec<String>,
}

/// Turn per-function commit lists into the evidence's cross-indexed form.
/// Only commits inside the range are reported; origins are kept as SHAs.
#[allow(clippy::too_many_arguments)]
fn assemble(
    ev: &mut Evidence,
    meta: Meta,
    res: &[Regex],
    all: &HashMap<String, CachedCommit>,
    per_fn: &HashMap<usize, (Vec<String>, Option<String>)>,
    deleted: &[CachedDeleted],
    files: Vec<ChangedFile>,
    caveats: Vec<String>,
) {
    let in_range = |sha: &str| meta.range.as_ref().map(|r| r.contains(sha)).unwrap_or(true);
    // commits to report: every commit in the range when there is one (a
    // commit that touched no function still touched files); otherwise those
    // some function references, plus deletions
    let mut wanted: BTreeSet<&str> = BTreeSet::new();
    if let Some(r) = &meta.range {
        for s in r {
            if all.contains_key(s) {
                wanted.insert(s.as_str());
            }
        }
    }
    for (shas, _) in per_fn.values() {
        for s in shas {
            if in_range(s) && all.contains_key(s) {
                wanted.insert(s.as_str());
            }
        }
    }
    for d in deleted {
        if let Some(c) = &d.commit {
            if all.contains_key(c) {
                wanted.insert(c.as_str());
            }
        }
    }
    let mut commits: Vec<&CachedCommit> = wanted.iter().map(|s| &all[*s]).collect();
    commits.sort_by(|a, b| b.date_unix.cmp(&a.date_unix).then_with(|| a.sha.cmp(&b.sha)));
    let idx: HashMap<&str, usize> = commits.iter().enumerate().map(|(i, c)| (c.sha.as_str(), i)).collect();
    let mut out: Vec<Commit> = commits
        .iter()
        .map(|c| Commit {
            sha: c.sha.clone(),
            short: c.sha.chars().take(12).collect(),
            author: c.author.clone(),
            date: c.date.clone(),
            date_unix: c.date_unix,
            subject: c.subject.clone(),
            tickets: Vec::new(),
            functions: Vec::new(),
            files: c.files.clone().unwrap_or_default(),
        })
        .collect();
    let mut by_ticket: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, c) in commits.iter().enumerate() {
        for t in tickets_in(res, &c.message) {
            by_ticket.entry(t).or_default().push(i);
        }
    }
    let mut tickets: Vec<Ticket> = by_ticket.into_iter().map(|(id, cs)| Ticket { id, commits: cs, ..Default::default() }).collect();
    for (ti, t) in tickets.iter().enumerate() {
        for &ci in &t.commits {
            out[ci].tickets.push(ti);
        }
    }
    let mut origin_of: HashMap<usize, String> = HashMap::new();
    for (fi, f) in ev.functions.iter_mut().enumerate() {
        f.commits.clear();
        f.origin = None;
        if let Some((shas, origin)) = per_fn.get(&fi) {
            // -L over-attributes across a merged hunk (editing the function
            // above and adding this one in one commit traces the new lines
            // back through the older function's history), so commits older
            // than the origin are dropped. `shas` is the order `git log -L`
            // walked the graph, which IS the ancestry -- author dates are
            // user-controlled and need not increase along it, so cutting by
            // timestamp silently discarded real changes (#13). An origin we
            // could not establish keeps everything.
            let cut = origin.as_deref().and_then(|o| shas.iter().position(|s| s == o));
            let visible = match cut {
                Some(k) => &shas[..=k],
                None => &shas[..],
            };
            let mut v: Vec<usize> = visible.iter().filter_map(|s| idx.get(s.as_str()).copied()).collect();
            v.sort_unstable();
            v.dedup();
            for &ci in &v {
                out[ci].functions.push(fi);
            }
            f.commits = v;
            f.origin = origin.clone();
            if let Some(o) = origin {
                origin_of.insert(fi, o.clone());
            }
        }
    }
    let del: Vec<DeletedFunction> = deleted
        .iter()
        .map(|d| DeletedFunction {
            name: d.name.clone(),
            file: d.repo_path.clone(),
            line_start: d.line_start,
            line_end: d.line_end,
            commit: d.commit.as_deref().and_then(|s| idx.get(s).copied()),
        })
        .collect();
    // evidence file -> repo path, for "does this changed file hold functions"
    let fn_count: HashMap<&str, usize> = files.iter().map(|cf| (cf.repo_path.as_str(), ev.files.get(cf.file_id).map(|f| f.function_ids.len()).unwrap_or(0))).collect();
    for t in tickets.iter_mut() {
        let shas: HashSet<&str> = t.commits.iter().map(|&ci| out[ci].sha.as_str()).collect();
        let mut fs: Vec<usize> = t.commits.iter().flat_map(|&ci| out[ci].functions.iter().copied()).collect();
        fs.sort_unstable();
        fs.dedup();
        t.added = fs.iter().copied().filter(|f| origin_of.get(f).map(|o| shas.contains(o.as_str())).unwrap_or(false)).collect();
        t.modified = fs.iter().copied().filter(|f| !t.added.contains(f)).collect();
        t.functions = fs;
        t.deleted = del.iter().enumerate().filter(|(_, d)| d.commit.map(|ci| t.commits.contains(&ci)).unwrap_or(false)).map(|(i, _)| i).collect();
        // net effect over the ticket, from whether the file existed before its
        // first commit and after its last: deleted then re-added is still
        // there, so modified (#325 D1). `t.commits` is newest first.
        // path -> (first status, last status, renamed on the way)
        let mut fmap: BTreeMap<String, (String, String, bool)> = BTreeMap::new();
        for &ci in t.commits.iter().rev() {
            for fc in &out[ci].files {
                let e = fmap.entry(fc.path.clone()).or_insert_with(|| (fc.status.clone(), fc.status.clone(), false));
                e.1 = fc.status.clone();
                e.2 |= fc.status == "renamed";
            }
        }
        t.files = fmap
            .into_iter()
            .map(|(path, (first, last, renamed))| {
                let status = match (first != "added", last != "deleted") {
                    (true, true) if renamed => "renamed",
                    (true, true) => "modified",
                    (false, true) => "added",
                    // gone at the end, whether or not it was there before
                    (_, false) => "deleted",
                }
                .to_string();
                let n = fn_count.get(path.as_str()).copied();
                TicketFile { in_evidence: n.is_some(), functions: n.unwrap_or(0), path, status }
            })
            .collect();
    }
    let unticketed = out.iter().enumerate().filter(|(_, c)| c.tickets.is_empty()).map(|(i, _)| i).collect();
    ev.changes = Some(Changes {
        repo: meta.repo,
        head: meta.head,
        since: meta.since,
        since_sha: meta.since_sha,
        created_unix: meta.created_unix,
        patterns: meta.patterns,
        commits: out,
        tickets,
        unticketed,
        files,
        deleted: del,
        caveats,
        impact: None,
    });
}

/// Check that every git object ID recorded in a loaded cache is a valid hex
/// SHA-1 or SHA-256 commit ID before trusting it in git invocations.
fn cache_ids_valid(c: &Cache) -> bool {
    use butterfly_model::revision::is_object_id;
    is_object_id(&c.head)
        && c.since_sha.as_deref().is_none_or(is_object_id)
        && c.range.iter().all(|s| is_object_id(s))
        && c.commits.iter().all(|cm| is_object_id(&cm.sha))
        && c.files.iter().all(|f| f.functions.iter().all(|fn_| fn_.commits.iter().all(|s| is_object_id(s)) && fn_.origin.as_deref().is_none_or(is_object_id)))
        && c.deleted.iter().all(|d| d.commit.as_deref().is_none_or(is_object_id))
}

/// Read the history for every function in `ev`, reusing `prior` where the
/// file and the function are unchanged since it was built. Writes the result
/// into `ev` and returns the new cache.
pub fn analyse(ev: &mut Evidence, opts: &Options, prior: Option<&Cache>) -> Result<(Cache, Outcome)> {
    let loaded: Option<Baseline> = match &opts.baseline {
        Some(p) => {
            // the baseline is a supplied sidecar like the change cache
            let lim = butterfly_model::limits::Limits::from_env();
            let t = butterfly_model::limits::read_bounded_string(p, butterfly_model::limits::Kind::Auxiliary, &lim).map_err(|e| anyhow!("reading baseline {e}"))?;
            let b: Baseline = serde_json::from_str(&t).with_context(|| format!("{} is not a butterfly baseline", p.display()))?;
            b.check_schema().map_err(|e| anyhow!("{}: {e}", p.display()))?;
            Some(b)
        }
        None => None,
    };
    analyse_with_baseline(ev, opts, prior, loaded.as_ref())
}

/// [`analyse`] with a caller-supplied, already-loaded baseline (so the caller
/// can enforce placement/containment rules before reading an evidence-named
/// baseline file). Ignores `opts.baseline`.
pub fn analyse_with_baseline(ev: &mut Evidence, opts: &Options, prior: Option<&Cache>, baseline: Option<&Baseline>) -> Result<(Cache, Outcome)> {
    let repo = find_repo(ev, opts)?;
    let repo_s = repo.display().to_string();
    let head = git(&repo, &["rev-parse", "HEAD"]).context("reading HEAD (empty repository?)")?;
    // `git` reached the repository above, so change data is being embedded:
    // record the git executable that produced it in provenance (#118).
    {
        // Identify the same binary git_raw actually ran, not an independent
        // PATH scan that could resolve a different (or non-executable) file.
        let (tool, caveat) = crate::provenance::identify("git", git_exe());
        crate::provenance::record(&mut ev.provenance.external_tools, &mut ev.provenance.caveats, tool, caveat);
    }
    let since_sha = match &opts.since {
        Some(s) => Some(git(&repo, &["rev-parse", "--verify", "--end-of-options", &format!("{s}^{{commit}}")]).map_err(|e| anyhow!("--since {s}: {e}"))?),
        None => None,
    };
    let res = compile(&opts.patterns)?;
    let excl = opts.exclude.iter().map(|g| glob_re(g)).collect::<Result<Vec<_>>>()?;
    let shallow = git(&repo, &["rev-parse", "--is-shallow-repository"]).map(|s| s == "true").unwrap_or(false);
    let range: Option<HashSet<String>> = match &since_sha {
        Some(s) if s == &head => Some(HashSet::new()),
        Some(s) => Some(git(&repo, &["rev-list", "--end-of-options", &format!("{s}..{head}")])?.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()),
        None => None,
    };

    // A prior cache is usable when it describes the same repository and every
    // object ID in it is a valid hex commit ID (refusing option-like values
    // such as `--output=...` from an untrusted sidecar). Files changed between
    // its HEAD and ours must be re-walked; `since` does not matter, the walk
    // is over the whole history either way.
    let prior = prior.filter(|c| c.schema == CHANGES_SCHEMA && c.repo == repo_s && cache_ids_valid(c));
    let changed_since_prior: Option<HashSet<String>> = prior.and_then(|c| {
        if c.head == head {
            Some(HashSet::new())
        } else {
            git(&repo, &["diff", "--name-only", "--end-of-options", &format!("{}..{}", c.head, head)]).ok().map(|s| s.lines().map(str::to_string).collect())
        }
    });
    let prior = if changed_since_prior.is_some() { prior } else { None };
    let prior_files: HashMap<&str, &CachedFile> = prior.map(|c| c.files.iter().map(|f| (f.path.as_str(), f)).collect()).unwrap_or_default();
    let prior_commits: HashMap<&str, &CachedCommit> = prior.map(|c| c.commits.iter().map(|x| (x.sha.as_str(), x)).collect()).unwrap_or_default();

    // ---- classify files, decide per function: reuse / walk / skip --------
    struct Job {
        fn_id: usize,
        repo_path: String,
        ls: u32,
        le: u32,
        /// the function's first line, for locating its origin
        sig: Option<String>,
    }
    let mut jobs: Vec<Job> = Vec::new();
    let mut per_fn: HashMap<usize, (Vec<String>, Option<String>)> = HashMap::new();
    let mut all: HashMap<String, CachedCommit> = HashMap::new();
    let mut file_status: Vec<(String, String)> = Vec::with_capacity(ev.files.len());
    let mut outcome = Outcome::default();
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();

    // classify every file first, in parallel: two or three git calls each
    let classify = |f: &butterfly_model::SourceFile| -> (String, &'static str) {
        match rel(&repo, &f.path) {
            None => (f.path.clone(), "outside-repo"),
            Some(rp) => {
                if excl.iter().any(|re| re.is_match(&rp)) {
                    (rp, "excluded")
                } else if git_raw(&repo, &["cat-file", "-e", &format!("HEAD:{rp}")]).is_err() {
                    let had_history = git(&repo, &["log", "-1", "--format=%H", "--", &rp]).map(|s| !s.is_empty()).unwrap_or(false);
                    (rp, if had_history { "not-at-head" } else { "untracked" })
                } else {
                    let blob = git_raw(&repo, &["show", &format!("HEAD:{rp}")]).unwrap_or_default();
                    if sha_hex(&blob) == f.sha256 {
                        (rp, "matches-head")
                    } else {
                        // a worktree source file, read only to hash it
                        let lim = butterfly_model::limits::Limits::from_env();
                        let wt = butterfly_model::limits::read_bounded(std::path::Path::new(&f.path), butterfly_model::limits::Kind::Source, &lim)
                            .map(|b| sha_hex(&b))
                            .unwrap_or_default();
                        if wt == f.sha256 {
                            (rp, "matches-worktree")
                        } else {
                            (rp, "mismatch")
                        }
                    }
                }
            }
        }
    };
    let classified: Vec<(String, &'static str)> = parallel_map(opts.jobs, &ev.files, |f| classify(f));
    for (f, (repo_path, status)) in ev.files.iter().zip(classified) {
        *counts.entry(status).or_default() += 1;
        let pf = prior_files.get(f.path.as_str()).filter(|pf| pf.sha256 == f.sha256 && changed_since_prior.as_ref().map(|s| !s.contains(&repo_path)).unwrap_or(false));
        for &fid in &f.function_ids {
            if !walkable(status) {
                outcome.skipped += 1;
                continue;
            }
            let fun = &ev.functions[fid];
            let cached = pf.and_then(|pf| pf.functions.iter().find(|c| c.name == fun.name && c.line_start == fun.line_start && c.line_end == fun.line_end && c.origin.is_some()));
            match cached {
                Some(c) => {
                    for sha in &c.commits {
                        if let Some(pc) = prior_commits.get(sha.as_str()) {
                            all.entry(sha.clone()).or_insert_with(|| (*pc).clone());
                        }
                    }
                    per_fn.insert(fid, (c.commits.clone(), c.origin.clone()));
                    outcome.reused += 1;
                }
                None => jobs.push(Job {
                    fn_id: fid,
                    repo_path: repo_path.clone(),
                    ls: fun.line_start,
                    le: fun.line_end.max(fun.line_start),
                    sig: f.text.get(fun.line_start.saturating_sub(1) as usize).cloned(),
                }),
            }
        }
        file_status.push((repo_path, status.to_string()));
    }

    // ---- walk, in parallel: git is happy with concurrent readers ---------
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, WalkResult)>> = Mutex::new(Vec::with_capacity(jobs.len()));
    let workers = effective_jobs(opts.jobs).min(jobs.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(j) = jobs.get(i) else { break };
                let r = log_full(&repo, &head, &j.repo_path, j.ls, j.le, j.sig.as_deref()).map(|(list, origin)| {
                    let origin = origin.or_else(|| {
                        let oldest = list.last().map(|c| c.sha.clone());
                        origin_of(&repo, &head, &j.repo_path, j.sig.as_deref(), oldest.as_deref())
                    });
                    (list, origin)
                });
                results.lock().unwrap().push((i, r));
            });
        }
    });
    let mut failures: Vec<String> = Vec::new();
    for (i, r) in results.into_inner().unwrap() {
        let j = &jobs[i];
        match r {
            Ok((list, origin)) => {
                per_fn.insert(j.fn_id, (list.iter().map(|c| c.sha.clone()).collect(), origin));
                for c in list {
                    all.entry(c.sha.clone()).or_insert(c);
                }
                outcome.walked += 1;
            }
            Err(e) => {
                failures.push(format!("{} ({}:{}-{}): {}", ev.functions[j.fn_id].name, j.repo_path, j.ls, j.le, e));
                outcome.skipped += 1;
            }
        }
    }

    // ---- deletions: baseline functions that no longer exist ---------------
    let mut caveats = Vec::new();
    let mut deleted: Vec<CachedDeleted> = Vec::new();
    if let Some(b) = baseline {
        // The baseline is keyed by a composite (name, file, line) identity, not
        // the display name, so two same-named functions no longer mask each
        // other (#249). A function that only moved (its start line shifted) is
        // reconciled the same way `baseline::compare` reconciles it, so a move
        // is not reported as a deletion here.
        let gone: Vec<&butterfly_model::FnFingerprint> = crate::baseline::gone(b, ev);
        match &since_sha {
            None if !gone.is_empty() => caveats.push(format!(
                "{} function(s) in the baseline no longer exist ({}{}), but deletions can only be located from a start revision: pass --since <the baseline's commit>.",
                gone.len(),
                gone.iter().take(5).map(|fp| fp.name.as_str()).collect::<Vec<_>>().join(", "),
                if gone.len() > 5 { ", …" } else { "" }
            )),
            None => {}
            Some(since) => {
                let mut unlocated: Vec<&str> = Vec::new();
                for fp in gone {
                    let name = fp.name.as_str();
                    if fp.file.is_empty() || fp.line_start == 0 {
                        unlocated.push(name);
                        continue;
                    }
                    let Some(rp) = rel(&repo, &fp.file) else {
                        unlocated.push(name);
                        continue;
                    };
                    match find_deletion(&repo, since, &head, &rp, fp.line_start, fp.line_end.max(fp.line_start)) {
                        Ok(Some(sha)) => {
                            if !all.contains_key(&sha) {
                                match show_commit(&repo, &sha) {
                                    Ok(c) => {
                                        all.insert(sha.clone(), c);
                                    }
                                    Err(e) => caveats.push(format!("deletion of {name}: {e}")),
                                }
                            }
                            deleted.push(CachedDeleted { name: name.to_string(), repo_path: rp, line_start: fp.line_start, line_end: fp.line_end, commit: Some(sha) });
                            outcome.deleted += 1;
                        }
                        Ok(None) => {
                            deleted.push(CachedDeleted { name: name.to_string(), repo_path: rp, line_start: fp.line_start, line_end: fp.line_end, commit: None });
                            outcome.deleted += 1;
                        }
                        Err(e) => caveats.push(format!("deletion of {name} ({rp}:{}-{}) could not be located: {e}", fp.line_start, fp.line_end)),
                    }
                }
                if !unlocated.is_empty() {
                    caveats.push(format!("{} function(s) missing from the evidence could not be located because the baseline records no file/line for them ({}{}): re-freeze the baseline with this version of butterfly.", unlocated.len(), unlocated.iter().take(5).cloned().collect::<Vec<_>>().join(", "), if unlocated.len() > 5 { ", …" } else { "" }));
                }
            }
        }
    }

    // ---- every commit in the range is reported, whether or not a function
    // references it: a commit that only touched a header is the point -------
    if let Some(r) = &range {
        if r.iter().any(|sha| !all.contains_key(sha)) {
            if let Some(since) = &since_sha {
                for c in parse_commits(&git(&repo, &["log", FMT, "--end-of-options", &format!("{since}..{head}")])?) {
                    all.entry(c.sha.clone()).or_insert(c);
                }
            }
        }
    }

    // ---- files per reported commit (name-status), reused from the cache ---
    let in_range = |sha: &str| range.as_ref().map(|r| r.contains(sha)).unwrap_or(true);
    let mut need: Vec<String> = all.values().filter(|c| c.files.is_none() && in_range(&c.sha)).map(|c| c.sha.clone()).collect();
    // `all` is a HashMap, so this list arrives in the process's hash-seed
    // order; where two commits fail below, their caveat lines were emitted
    // in that order and the output was not reproducible (#215's class, on
    // an error path). Sorting also fixes the order the git calls are made
    // in, which is one less thing that varies between runs.
    need.sort();
    let listed = parallel_map(opts.jobs, &need, |sha| commit_files(&repo, sha));
    for (sha, r) in need.into_iter().zip(listed) {
        match r {
            Ok(fs) => {
                // the same excludes that keep vendored or generated files out
                // of the walk keep them out of the ticket's file list
                let fs: Vec<FileChange> = fs.into_iter().filter(|f| !excl.iter().any(|re| re.is_match(&f.path))).collect();
                if let Some(c) = all.get_mut(&sha) {
                    c.files = Some(fs);
                }
            }
            Err(e) => caveats.push(format!("files of commit {}: {e}", &sha[..12.min(sha.len())])),
        }
    }

    // ---- caveats -----------------------------------------------------------
    if shallow {
        caveats.push("This is a SHALLOW clone: the history is truncated, so every function's earliest visible commit is the clone boundary, not its real origin, and tickets before that boundary are lost. Run against a full clone (git fetch --unshallow) for attribution that can be relied on.".to_string());
    }
    if let Some(n) = counts.get("matches-worktree") {
        caveats.push(format!("{n} analysed file(s) match the working tree but not HEAD: the evidence was built from uncommitted edits, so the line ranges used to attribute commits may be shifted by those edits. Commit the sources and rebuild before relying on this attribution."));
    }
    if let Some(n) = counts.get("mismatch") {
        caveats.push(format!("{n} analysed file(s) match neither HEAD nor the working tree: the evidence was built from a different checkout. Their functions were walked at HEAD's line numbers, which is unreliable."));
    }
    for (k, label) in [
        ("not-at-head", "have history but do not exist at HEAD (deleted or renamed away)"),
        ("untracked", "not tracked by git"),
        ("outside-repo", "outside the repository"),
        ("excluded", "excluded by [change].exclude"),
    ] {
        if let Some(n) = counts.get(k) {
            caveats.push(format!("{n} analysed file(s) {label}: their functions have no change history."));
        }
    }
    if !failures.is_empty() {
        let shown: Vec<&String> = failures.iter().take(5).collect();
        caveats.push(format!(
            "{} function(s) could not be walked: {}{}",
            failures.len(),
            shown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; "),
            if failures.len() > 5 { "; …" } else { "" }
        ));
    }
    if opts.since.is_some() {
        caveats.push(format!("Commits are reported from {} to HEAD only; each function's origin is known from the whole history.", opts.since.as_deref().unwrap_or("")));
    } else {
        caveats.push("No --since: every function with any history counts as changed, only commits that touched some function are listed (a commit that changed only a header or a build file is not), and deleted functions cannot be found. For a re-certification answer pass --since <certified baseline commit> and the frozen butterfly-baseline.json.".to_string());
    }
    caveats.push("Attribution follows each function's line range through the history (git log -L). A function touched and reverted within a ticket is still reported (the conservative superset). Squashed or rebased history collapses tickets into whichever commit survived; a renamed function appears as deleted plus added.".to_string());

    // ---- cache + evidence --------------------------------------------------
    let created_unix = now_unix();
    let cache = Cache {
        schema: CHANGES_SCHEMA.into(),
        repo: repo_s.clone(),
        head: head.clone(),
        since: opts.since.clone(),
        since_sha: since_sha.clone(),
        range: range
            .as_ref()
            .map(|r| {
                let mut v: Vec<String> = r.iter().cloned().collect();
                v.sort();
                v
            })
            .unwrap_or_default(),
        created_unix,
        patterns: opts.patterns.clone(),
        commits: {
            let mut v: Vec<CachedCommit> = all.values().cloned().collect();
            v.sort_by(|a, b| b.date_unix.cmp(&a.date_unix).then_with(|| a.sha.cmp(&b.sha)));
            v
        },
        files: ev
            .files
            .iter()
            .zip(file_status.iter())
            .map(|(f, (rp, st))| CachedFile {
                path: f.path.clone(),
                repo_path: rp.clone(),
                sha256: f.sha256.clone(),
                status: st.clone(),
                functions: f
                    .function_ids
                    .iter()
                    .filter_map(|&fid| {
                        per_fn.get(&fid).map(|(c, o)| CachedFn {
                            name: ev.functions[fid].name.clone(),
                            line_start: ev.functions[fid].line_start,
                            line_end: ev.functions[fid].line_end,
                            commits: c.clone(),
                            origin: o.clone(),
                        })
                    })
                    .collect(),
            })
            .collect(),
        deleted: deleted.clone(),
    };
    let files: Vec<ChangedFile> = cache
        .files
        .iter()
        .enumerate()
        .map(|(i, cf)| ChangedFile { file_id: i, repo_path: cf.repo_path.clone(), status: cf.status.clone(), functions_walked: cf.functions.len() })
        .collect();
    let meta = Meta { repo: repo_s, head, since: opts.since.clone(), since_sha, range, created_unix, patterns: opts.patterns.clone() };
    assemble(ev, meta, &res, &all, &per_fn, &deleted, files, caveats);
    Ok((cache, outcome))
}

/// Re-attach a cache to freshly ingested evidence without touching git,
/// reporting the same `since` the cache was built with. Refuses (with the
/// reason) when any analysed file or function no longer matches what the
/// cache was built from — a stale attribution must not look like a current one.
pub fn embed(ev: &mut Evidence, cache: &Cache, patterns: Option<&[String]>) -> Result<(), String> {
    if cache.schema != CHANGES_SCHEMA {
        return Err(format!("unsupported schema {:?}", cache.schema));
    }
    let by_path: HashMap<&str, &CachedFile> = cache.files.iter().map(|f| (f.path.as_str(), f)).collect();
    let mut stale: Vec<String> = Vec::new();
    let mut files = Vec::new();
    let mut per_fn: HashMap<usize, (Vec<String>, Option<String>)> = HashMap::new();
    for (fi, f) in ev.files.iter().enumerate() {
        let Some(cf) = by_path.get(f.path.as_str()) else {
            stale.push(f.display.clone());
            continue;
        };
        if cf.sha256 != f.sha256 {
            stale.push(f.display.clone());
            continue;
        }
        files.push(ChangedFile { file_id: fi, repo_path: cf.repo_path.clone(), status: cf.status.clone(), functions_walked: cf.functions.len() });
        for &fid in &f.function_ids {
            let fun = &ev.functions[fid];
            match cf.functions.iter().find(|c| c.name == fun.name && c.line_start == fun.line_start && c.line_end == fun.line_end) {
                Some(c) => {
                    per_fn.insert(fid, (c.commits.clone(), c.origin.clone()));
                }
                None if walkable(&cf.status) => stale.push(format!("{}:{}", f.display, fun.name)),
                None => {}
            }
        }
    }
    if !stale.is_empty() {
        let shown: Vec<&str> = stale.iter().take(5).map(String::as_str).collect();
        return Err(format!(
            "{} file(s)/function(s) differ from the cached history ({}{}): rerun `butterfly changes`",
            stale.len(),
            shown.join(", "),
            if stale.len() > 5 { ", …" } else { "" }
        ));
    }
    let pats: Vec<String> = patterns.map(|p| p.to_vec()).unwrap_or_else(|| cache.patterns.clone());
    let res = compile(&pats).map_err(|e| e.to_string())?;
    let all: HashMap<String, CachedCommit> = cache.commits.iter().map(|c| (c.sha.clone(), c.clone())).collect();
    let mut caveats =
        vec![format!("Change history re-attached from a cache built at {} (every analysed file still matches its cached hash).", cache.head.chars().take(12).collect::<String>())];
    if cache.since.is_some() {
        caveats.push(format!("Commits are reported from {} to that HEAD only.", cache.since.as_deref().unwrap_or("")));
    }
    let range = cache.since_sha.as_ref().map(|_| cache.range.iter().cloned().collect::<HashSet<String>>());
    let meta = Meta {
        repo: cache.repo.clone(),
        head: cache.head.clone(),
        since: cache.since.clone(),
        since_sha: cache.since_sha.clone(),
        range,
        created_unix: cache.created_unix,
        patterns: pats,
    };
    assemble(ev, meta, &res, &all, &per_fn, &cache.deleted, files, caveats);
    Ok(())
}

/// Parse a change cache that has already been read.
///
/// As with the config, the caller reads it once under the run's budget and
/// the same bytes serve the parse and the provenance hash (review of #5).
pub fn parse(text: &str, path: &Path) -> Result<Cache> {
    serde_json::from_str(text).with_context(|| format!("{} is not a butterfly change cache", path.display()))
}

pub fn load(path: &Path) -> Result<Cache> {
    let lim = butterfly_model::limits::Limits::from_env();
    let t = butterfly_model::limits::read_bounded_string(path, butterfly_model::limits::Kind::Auxiliary, &lim).map_err(|e| anyhow::anyhow!("{e}"))?;
    parse(&t, path)
}

/// Write the cache the way every output is written: atomically, and
/// replacing a symlink at `path` rather than writing through it. The cache
/// sits next to the evidence, usually in a shared build directory, and a
/// link planted there used to have the next run overwrite whatever it
/// pointed at (#248). The caller reports a replaced link.
pub fn save(cache: &Cache, path: &Path) -> Result<crate::fsauth::Wrote> {
    crate::fsauth::write_atomic(path, serde_json::to_string_pretty(cache)?.as_bytes()).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {

    /// A generic parameter list belongs to the name, not to the arguments.
    /// `pub fn f<T>(…)` put a `>` where the identifier scan expected the last
    /// letter, so `name_identity` gave up, the caller fell back to the full
    /// signature, and renaming a parameter of a generic function reset its
    /// identity all over again (review of #13).
    #[test]
    fn a_generic_parameter_list_is_not_part_of_the_identity() {
        for (line, want) in [
            ("pub fn f<T>(old_name: T) -> T { old_name }", Some("f")),
            ("pub fn f<T>(new_name: T) -> T { new_name }", Some("f")),
            ("fn compute<T: Clone, U>(a: T, b: U) -> T {", Some("compute")),
            ("fn nested<T: Into<Vec<u8>>>(x: T) {", Some("nested")),
            ("fn konst<const N: usize>(x: [u8; N]) {", Some("konst")),
            // restricted visibility is a parenthesised group before the name
            ("pub(crate) fn f(x: i32) -> i32 {", Some("f")),
            ("pub(super) fn f<T>(x: T) -> T {", Some("f")),
            ("pub(in crate::a) fn f(x: i32) {", Some("f")),
            ("    pub(crate) async fn f(&self) {", Some("f")),
            ("template <> void Klass::method<int>(int x) {", Some("Klass::method")),
            // no generics: unchanged
            ("int f(int x) {", Some("f")),
            ("void Klass::method(int) const {", Some("Klass::method")),
            ("pub fn compute(&self) -> i32 {", Some("compute")),
            // C++ operators: the name ends in symbols, not letters
            ("bool C::operator==(const C& other) const {", Some("C::operator==")),
            ("bool C::operator!=(const C& other) const {", Some("C::operator!=")),
            ("int& C::operator[](size_t i) {", Some("C::operator[]")),
            ("Vec operator+(const Vec &a, const Vec &b) {", Some("operator+")),
            ("bool operator>(const A &a, const A &b) {", Some("operator>")),
            ("bool operator<(const A &a, const A &b) {", Some("operator<")),
            ("C& C::operator=(const C& other) {", Some("C::operator=")),
            ("std::ostream& operator<<(std::ostream& o, const C& c) {", Some("operator<<")),
            ("C::operator bool() const {", Some("C::operator bool")),
            ("void* operator new[](size_t n) {", Some("operator new[]")),
            // `operator` has to be a word of its own
            ("int operator_new(int x) {", Some("operator_new")),
            ("int my_operator(int x) {", Some("my_operator")),
            // not a name, and must stay that way
            ("std::map<int,int> (*fp)(void);", None),
            ("(void)x;", None),
        ] {
            assert_eq!(name_identity(line), want, "{line}");
        }
    }

    /// Renaming a parameter has to leave the identity alone -- that is the
    /// whole point of matching on the name -- and it must do so for a generic
    /// signature too.
    /// The fallback identity and the pickaxe argument both come from
    /// `signature_identity`; with the first `(` it was `pub(crate)` for every
    /// restricted-visibility function, which is not a signature at all.
    #[test]
    fn a_visibility_group_is_not_the_parameter_list() {
        assert_eq!(signature_identity("pub(crate) fn f(x: i32) -> i32 {"), "pub(crate) fn f(x: i32)");
        assert_eq!(signature_identity("pub fn f(x: i32) -> i32 {"), "pub fn f(x: i32)");
        assert_eq!(signature_identity("int f(int x) {"), "int f(int x)");
        // a line with `pub` and no parameter list at all
        assert_eq!(signature_identity("pub(crate) struct S;"), "pub(crate) struct S;");
    }

    /// The property the whole change exists for, on an operator.
    #[test]
    fn renaming_a_parameter_does_not_change_an_operators_identity() {
        for (before, after) in [
            ("bool C::operator==(const C& old_name) const {", "bool C::operator==(const C& new_name) const {"),
            ("int& C::operator[](size_t old_name) {", "int& C::operator[](size_t new_name) {"),
            ("Vec operator+(const Vec &old_name, const Vec &b) {", "Vec operator+(const Vec &new_name, const Vec &b) {"),
        ] {
            assert_eq!(name_identity(before), name_identity(after), "{before}");
            assert!(name_identity(before).is_some(), "{before}");
            assert_ne!(signature_identity(before), signature_identity(after), "the signature moves, which is why the name is used");
        }
        // and two different operators on the same class stay apart
        assert_ne!(name_identity("bool C::operator==(const C& a) const {"), name_identity("bool C::operator!=(const C& a) const {"),);
    }

    #[test]
    fn renaming_a_parameter_does_not_change_a_generic_functions_identity() {
        let before = "pub fn f<T>(old_name: T) -> T { old_name }";
        let after = "pub fn f<T>(new_name: T) -> T { new_name }";
        assert_eq!(name_identity(before), name_identity(after), "the identity survives the edit");
        assert_ne!(signature_identity(before), signature_identity(after), "while the signature does not, which is why the name is used");
    }

    use super::*;

    #[test]
    fn tickets_use_group_one_or_whole_match_and_dedupe() {
        let res = compile(&[r"\b([A-Z]{2,10}-\d+)\b".to_string(), r"refs #(\d+)".to_string()]).unwrap();
        let t = tickets_in(&res, "ABC-12: fix\n\nAlso touches ABC-12 and XYZ-7. refs #42");
        assert_eq!(t, vec!["ABC-12", "XYZ-7", "42"]);
        assert!(tickets_in(&res, "no ticket here").is_empty());
    }

    #[test]
    fn globs_match_like_gitignore() {
        let v = glob_re("vendor/**").unwrap();
        assert!(v.is_match("vendor/a/b.c"));
        assert!(!v.is_match("src/vendor.c"));
        let g = glob_re("*.gen.c").unwrap();
        assert!(g.is_match("src/x.gen.c"));
        assert!(!g.is_match("src/x.c"));
        let d = glob_re("generated").unwrap();
        assert!(d.is_match("a/generated/x.c"));
        assert!(!d.is_match("a/generated_x.c"));
    }

    #[test]
    fn origin_is_the_commit_whose_diff_adds_the_signature() {
        // two -L records newest first; the older one created the file with f1,
        // the newer edited f1 and added f2 in the same hunk (git traces f2 back)
        let newer = "\x1e{n}\x1fme\x1f2026-02-01T00:00:00+00:00\x1f200\x1fABC-2: edit f1, add f2\x1fABC-2: edit f1, add f2\ndiff --git a/a.c b/a.c\n--- a/a.c\n+++ b/a.c\n@@ -2,2 +2,6 @@\n-    return x + 1;\n+    return x + 10;\n }\n+\n+int f2(int x) {\n+    return x + 2;\n+}\n";
        let older = "\x1e{o}\x1fme\x1f2026-01-01T00:00:00+00:00\x1f100\x1fABC-1: add f1\x1fABC-1: add f1\ndiff --git a/a.c b/a.c\n--- /dev/null\n+++ b/a.c\n@@ -0,0 +1,3 @@\n+int f1(int x) {\n+    return x + 1;\n+}\n";
        let text = format!("{}{}", newer.replace("{n}", &"b".repeat(40)), older.replace("{o}", &"a".repeat(40)));
        let recs = parse_records(&text);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].0.message, "ABC-2: edit f1, add f2", "the diff is not part of the message");
        assert!(recs[0].1.starts_with("diff --git"));
        let origin = |sig: &str| recs.iter().rev().find(|(_, d)| d.lines().any(|l| l.strip_prefix('+').map(|x| x.trim() == sig).unwrap_or(false))).map(|(c, _)| c.sha.clone());
        assert_eq!(origin("int f2(int x) {").as_deref(), Some("b".repeat(40).as_str()), "f2 was added by the newer commit even though -L lists both");
        assert_eq!(origin("int f1(int x) {").as_deref(), Some("a".repeat(40).as_str()));
        assert_eq!(origin("int f9(int x) {"), None, "unknown signature -> pickaxe fallback");
    }

    #[test]
    fn commit_record_parsing_survives_separators_in_the_message() {
        let text = format!("\x1e{sha}\x1fme\x1f2026-01-01T00:00:00+00:00\x1f1700000000\x1fsubj\x1fbody with \x1f inside", sha = "a".repeat(40));
        let v = parse_commits(&text);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].subject, "subj");
        assert!(v[0].message.starts_with("body with"));
    }
}
