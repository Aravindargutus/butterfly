// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Is the change history in the evidence still the repository's history?
//!
//! The evidence records the HEAD its history was read at and the SHA-256 of
//! every analysed source. The viewer compares both with the machine it runs
//! on — the repository (when it is here) and the files on disk — and the
//! resolutions it has written since it loaded, so the page can say "this
//! attribution is N commits old, re-run `butterfly changes`" instead of
//! presenting stale attribution as current. Nothing here touches the
//! network.
//!
//! Every path it uses comes from the evidence, which is untrusted, so each
//! is bound to what the operator approved when the viewer started (#248): a
//! source is read only if it is inside the approved source root, placed by
//! the kernel as ingest does, and git is only ever run in the approved
//! repository -- a repository's own config can make git run a program.
//! Anything else is reported as not checked, never probed.

use butterfly_model::limits::{self, Limits};
use butterfly_model::Evidence;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// What the check needs from the evidence, kept small so a variant can hold
/// it without holding the whole `Evidence`.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// (display, recorded path, sha256) per analysed file.
    pub files: Vec<(String, String, String)>,
    pub repo: Option<String>,
    pub head: Option<String>,
    pub since: Option<String>,
    pub has_changes: bool,
    pub has_impact: bool,
    /// Why the persisted impact no longer binds to the evidence/resolutions it
    /// was computed against, captured when the variant loaded. The in-process
    /// `resolution_edits` counter only sees writes this run makes, so after a
    /// restart it is zero even when the loaded resolutions already differ from
    /// the impact's recorded digest -- this catches that (#249).
    pub impact_stale: Option<String>,
    /// Reasons requirement source documents no longer match the loaded
    /// requirements sidecar (or could not be verified), captured at load.
    pub requirements_stale: Vec<String>,
}

impl Input {
    pub fn of(ev: &Evidence) -> Input {
        let c = ev.changes.as_ref();
        Input {
            files: ev.files.iter().map(|f| (f.display.clone(), f.path.clone(), f.sha256.clone())).collect(),
            repo: c.map(|c| c.repo.clone()),
            head: c.map(|c| c.head.clone()),
            since: c.and_then(|c| c.since.clone()),
            has_changes: c.is_some(),
            has_impact: c.map(|c| c.impact.is_some()).unwrap_or(false),
            // filled by the caller, which has the loaded resolutions the impact
            // must be bound against (impact::staleness lives in the ingest crate).
            impact_stale: None,
            requirements_stale: Vec::new(),
        }
    }
}

/// What the operator approved for this viewer: the tree whose sources may
/// be read, and the repository git may be run in. Both canonical.
#[derive(Debug, Clone)]
pub struct Authority {
    pub source_root: PathBuf,
    pub repo: Option<PathBuf>,
}

impl Authority {
    /// `--source-root` (default: the working directory, as `ingest` and
    /// `tags` use) and `--repo` (default: the repository that contains the
    /// source root, found by looking for `.git` -- git itself is not run to
    /// find it).
    pub fn resolve(source_root: Option<&Path>, repo: Option<&Path>) -> anyhow::Result<Authority> {
        use anyhow::Context;
        let root = match source_root {
            Some(r) => r.to_path_buf(),
            None => std::env::current_dir().context("determining the working directory that bounds the freshness check")?,
        };
        let source_root = root.canonicalize().with_context(|| format!("resolving the source root {}", root.display()))?;
        let repo = match repo {
            Some(r) => Some(r.canonicalize().with_context(|| format!("resolving the repository {}", r.display()))?),
            None => source_root.ancestors().find(|d| d.join(".git").symlink_metadata().is_ok()).map(Path::to_path_buf),
        };
        Ok(Authority { source_root, repo })
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Freshness {
    /// Something the evidence says about the history or the sources is no
    /// longer true on this machine.
    pub stale: bool,
    pub reasons: Vec<String>,
    pub recorded_head: Option<String>,
    pub current_head: Option<String>,
    /// Commits on the repository since the recorded HEAD, when both are known.
    pub commits_behind: Option<u64>,
    /// Analysed files whose text on disk differs from the analysed text.
    pub sources_modified: Vec<String>,
    /// Analysed files that are no longer on disk.
    pub sources_missing: Vec<String>,
    /// Analysed files that could not be read, each with why. Whether they
    /// still match is unknown, which is not the same as knowing they match.
    pub sources_unreadable: Vec<String>,
    /// Analysed files outside what the operator approved, each with why.
    /// They were not opened, so whether they match is unknown.
    pub sources_unapproved: Vec<String>,
    /// Analysed files with uncommitted changes in the repository.
    pub uncommitted: Vec<String>,
    /// Resolutions written by this viewer since the obligations were computed.
    pub resolution_edits: usize,
    /// Requirement source documents that changed or could not be verified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirements_stale: Vec<String>,
    /// The command that brings the evidence up to date.
    pub rerun: Option<String>,
    /// When there is no history yet: how to get one.
    pub hint: Option<String>,
    pub checked_unix: u64,
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    // the same resolved git that `changes` runs and records, under the same
    // helper policy: a cleaned environment (no token, no GIT_DIR), a
    // deadline and an output ceiling (#246)
    let mut argv: Vec<&std::ffi::OsStr> = vec!["-C".as_ref(), repo.as_os_str()];
    argv.extend(args.iter().map(|a| std::ffi::OsStr::new(*a)));
    let out = butterfly_ingest::helper::run(butterfly_ingest::change::git_exe(), &argv, None, butterfly_ingest::helper::Bounds::standard()).ok()?;
    Some(String::from_utf8_lossy(&out).trim_end().to_string())
}

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Why a read failed, in words, without quoting an OS message back into the
/// page.
fn why(e: &std::io::Error) -> &'static str {
    use std::io::ErrorKind::*;
    match e.kind() {
        PermissionDenied => "permission denied",
        IsADirectory => "it is a directory",
        InvalidData => "not valid text",
        _ => "unreadable",
    }
}

/// How reading one analysed source ended.
pub(crate) enum Analysed {
    Text(Vec<u8>),
    Missing,
    /// Why it could not be read, in the words that go in the reason.
    Unreadable(String),
    /// Outside what the operator approved: not read, and why.
    Unapproved(&'static str),
}

/// Where a recorded path stands against the approved root, before anything
/// is opened.
pub(crate) enum Approval {
    /// Inside the root, at this canonical path.
    Inside(PathBuf),
    Missing,
    Unreadable(String),
    Outside,
}

/// Judge a path the evidence recorded against the approved root.
///
/// It must be absolute with no `..` -- a path that climbs is refused, not
/// resolved. It is then canonicalised, because the evidence records the
/// coverage export's own spelling and that need not be the canonical one
/// (macOS's /tmp is /private/tmp). Canonicalising looks the name up; it
/// does not open or read it. A path outside the root gets the same answer
/// whether or not it exists, so the check says nothing about files there.
/// The kernel still has the last word on the file actually opened.
pub(crate) fn approve(p: &Path, root: &Path) -> Approval {
    if !p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Approval::Outside;
    }
    match p.canonicalize() {
        Ok(c) if c.starts_with(root) => Approval::Inside(c),
        Ok(_) => Approval::Outside,
        // Gone, or not reachable: where would it have been? The nearest
        // ancestor that still resolves says, with the rest re-attached --
        // there is no `..` left in it to climb with.
        Err(e) => {
            let placed = p.ancestors().skip(1).find_map(|a| a.canonicalize().ok().map(|c| c.join(p.strip_prefix(a).unwrap_or(p))));
            match placed {
                Some(c) if c.starts_with(root) && e.kind() == std::io::ErrorKind::NotFound => Approval::Missing,
                Some(c) if c.starts_with(root) => Approval::Unreadable(why(&e).to_string()),
                _ => Approval::Outside,
            }
        }
    }
}

/// A limits refusal without the canonical path it starts with: the reason is
/// shown after the file's display name, and the absolute path is the
/// operator's directory layout, not the client's business (#340).
fn unpathed(refusal: String, p: &Path) -> String {
    match refusal.strip_prefix(&format!("{}: ", p.display())) {
        Some(rest) => rest.to_string(),
        None => refusal,
    }
}

/// Read one analysed source to hash it, under the same per-kind cap `ingest`
/// applied when it embedded the text.
///
/// The paths come from the evidence, and the evidence is untrusted. So a
/// path is first judged by its text against the approved root and not
/// touched at all if it is outside; then it is opened once, and the kernel
/// is asked where that file is and how many names it has, exactly as ingest
/// does (#248). Only then is it read, from the same descriptor, under the
/// source cap: without that, `/api/freshness` read a file of any size, on
/// every request, on a live route (#231).
///
/// The outcomes are kept apart because they are different answers (#10).
/// The open is `O_NONBLOCK` and the file must be regular, so a named pipe
/// among the analysed files is refused rather than blocking the route.
fn read_analysed(p: &Path, root: &Path, limits: &Limits) -> Analysed {
    read_approved(p, root, limits::Kind::Source, limits)
}

/// Read a single untrusted path (from evidence or a sidecar) only if it lives
/// inside `root` and passes `fsauth::open_placed`, bounded under `kind`.
pub(crate) fn read_approved(p: &Path, root: &Path, kind: limits::Kind, limits: &Limits) -> Analysed {
    use butterfly_ingest::fsauth::{self, Standing};
    use std::io::Read;
    let p = match approve(p, root) {
        Approval::Inside(c) => c,
        Approval::Missing => return Analysed::Missing,
        Approval::Unreadable(w) => return Analysed::Unreadable(w),
        Approval::Outside => return Analysed::Unapproved("outside the approved source root"),
    };
    let placed = match fsauth::open_placed(root, &p) {
        Ok(placed) => placed,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Analysed::Missing,
        Err(e) => return Analysed::Unreadable(why(&e).to_string()),
    };
    match placed.standing {
        Standing::Inside => {}
        Standing::Outside => return Analysed::Unapproved("resolves outside the approved source root"),
        Standing::Linked(_) => return Analysed::Unapproved("hard-linked, so its place cannot be established"),
    }
    if !placed.meta.is_file() {
        return Analysed::Unreadable("not a regular file".into());
    }
    if let Err(refusal) = limits.admit(placed.meta.len(), kind, &p) {
        return Analysed::Unreadable(unpathed(refusal, &p));
    }
    let cap = limits.cap(kind);
    let mut bytes = Vec::with_capacity(placed.meta.len() as usize);
    if let Err(e) = (&placed.file).take(cap.saturating_add(1)).read_to_end(&mut bytes) {
        return Analysed::Unreadable(why(&e).to_string());
    }
    if bytes.len() as u64 > cap {
        return Analysed::Unreadable(format!("grew past the {} {} cap while being read", limits::human(cap), kind.what()));
    }
    // `admit` charged the descriptor's metadata length against the total
    // budget. If the file grew while we read it -- still under the per-file
    // cap, so admissible on its own -- those extra bytes must be charged too,
    // or a later source shares an undercounted BUTTERFLY_MAX_TOTAL_MB and the
    // aggregate budget stops meaning anything (#248). `read_bounded` does the
    // same; this route holds its own descriptor and cannot reopen the path, so
    // it calls the shared charge directly and refuses if the growth does not
    // fit the run's remaining budget.
    if let Err(refusal) = limits.charge_growth(kind, &p, placed.meta.len(), bytes.len() as u64) {
        return Analysed::Unreadable(unpathed(refusal, &p));
    }
    Analysed::Text(bytes)
}

/// A path the operator approved, as the client is shown it: relative to the
/// working directory where it is under it, the way `/api/gates` prints paths
/// (#214), and the working directory itself by name -- never the operator's
/// directory layout (#340).
fn shown(p: &Path) -> String {
    match std::env::current_dir().and_then(|c| c.canonicalize()) {
        Ok(cwd) if cwd == p => "the working directory".into(),
        _ => crate::for_display(p),
    }
}

/// The first few of a list, for a sentence a person reads.
fn few(v: &[String]) -> String {
    format!("{}{}", v.iter().take(4).cloned().collect::<Vec<_>>().join(", "), if v.len() > 4 { ", …" } else { "" })
}

pub fn check(inp: &Input, resolution_edits: usize, evidence_path: &Path, auth: &Authority) -> Freshness {
    check_with(inp, resolution_edits, evidence_path, &Limits::from_env(), auth)
}

/// [`check`] against explicit caps.
///
/// The caps are a parameter rather than read from the environment here so a
/// test can state them, and so one `/api/freshness` request is one budget
/// (the same shape as "one budget per command" on the CLI side).
pub fn check_with(inp: &Input, resolution_edits: usize, evidence_path: &Path, limits: &Limits, auth: &Authority) -> Freshness {
    let mut f = Freshness {
        recorded_head: inp.head.clone(),
        resolution_edits,
        checked_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        ..Default::default()
    };
    // printed in a command the reader runs from the same directory (#340)
    let ev_arg = crate::sh_quote(&crate::for_display(evidence_path));
    if !inp.has_changes {
        f.hint = Some(format!("butterfly changes {ev_arg} --since <certified baseline>"));
        if !inp.requirements_stale.is_empty() {
            f.requirements_stale = inp.requirements_stale.clone();
            f.reasons.extend(inp.requirements_stale.iter().cloned());
            f.stale = true;
        }
        return f;
    }
    // ---- the analysed text vs the files on disk (no git needed) ----------
    // A read that fails is not a match. Every error used to be discarded, so
    // a source that had been deleted, or that could not be opened, left the
    // evidence looking fresh -- the one thing this check exists to rule out
    // (#10). Missing and unreadable are kept apart from modified: they mean
    // different things and need different words.
    for (display, path, sha) in &inp.files {
        match read_analysed(Path::new(path), &auth.source_root, limits) {
            Analysed::Text(bytes) => {
                if &sha_hex(&bytes) != sha {
                    f.sources_modified.push(display.clone());
                }
            }
            Analysed::Missing => f.sources_missing.push(display.clone()),
            Analysed::Unreadable(w) => f.sources_unreadable.push(format!("{display} ({w})")),
            Analysed::Unapproved(w) => f.sources_unapproved.push(format!("{display} ({w})")),
        }
    }
    if !f.sources_modified.is_empty() {
        f.reasons.push(format!(
            "{} analysed source file(s) differ on disk from the analysed text ({}): rebuild, re-ingest, then re-run butterfly changes.",
            f.sources_modified.len(),
            few(&f.sources_modified)
        ));
    }
    if !f.sources_missing.is_empty() {
        f.reasons.push(format!(
            "{} analysed source file(s) are not on this machine ({}): the analysed text cannot be checked against anything, so the attribution is unverified rather than current. Expected when viewing evidence away from the tree it was built from.",
            f.sources_missing.len(),
            few(&f.sources_missing)
        ));
    }
    if !f.sources_unreadable.is_empty() {
        f.reasons.push(format!(
            "{} analysed source file(s) could not be read ({}): whether they still match the analysed text is unknown, which is not the same as knowing they match.",
            f.sources_unreadable.len(),
            few(&f.sources_unreadable)
        ));
    }
    if !f.sources_unapproved.is_empty() {
        f.reasons.push(format!(
            "{} analysed source file(s) are outside the approved source root, {}, and were not read ({}): whether they still match the analysed text is unknown. Serve with --source-root at the tree the evidence was built from.",
            f.sources_unapproved.len(),
            shown(&auth.source_root),
            few(&f.sources_unapproved)
        ));
    }
    // ---- the repository, only the approved one ----------------------------
    // Compared as text, both canonical: the recorded path is not looked up.
    let mut repo_unverified = false;
    let repo = match (&inp.repo, &auth.repo) {
        (Some(recorded), Some(approved)) if Path::new(recorded) == approved.as_path() => Some(approved.clone()),
        (Some(recorded), approved) => {
            repo_unverified = true;
            f.reasons.push(format!(
                "the history was read in {recorded}, which is not the approved repository ({}), so git was not consulted: whether the history is current is unverified. Serve with --repo to approve it.",
                approved.as_ref().map(|a| shown(a)).unwrap_or_else(|| "none found above the source root".into())
            ));
            None
        }
        (None, _) => None,
    };
    if let (Some(repo), Some(recorded)) = (repo, &inp.head) {
        if let Some(current) = git(&repo, &["rev-parse", "HEAD"]) {
            if &current != recorded {
                f.commits_behind = if recorded.starts_with('-') {
                    None
                } else {
                    git(&repo, &["rev-list", "--count", &format!("{recorded}..{current}"), "--"]).and_then(|s| s.trim().parse().ok())
                };
                f.reasons.push(match f.commits_behind {
                    Some(n) => format!("the repository has moved on: {n} commit(s) since the history was read at {}.", recorded.chars().take(12).collect::<String>()),
                    None => format!(
                        "the repository is at {} but the history was read at {} (not an ancestor: rebased or a different branch).",
                        current.chars().take(12).collect::<String>(),
                        recorded.chars().take(12).collect::<String>()
                    ),
                });
            }
            f.current_head = Some(current);
        } else {
            // A failed git rev-parse must fail closed rather than look fresh.
            repo_unverified = true;
            f.reasons.push("git could not read HEAD in the approved repository: whether the change history is current is unverified.".into());
        }
        let mut args: Vec<&str> = vec!["status", "--porcelain", "--"];
        // only paths inside BOTH the approved root and the approved repository
        // reach `git status`, so an out-of-repo file cannot make git exit 128
        // and silently drop the uncommitted-changes check for every other file
        let mut paths: Vec<String> = Vec::new();
        let mut outside_repo: Vec<String> = Vec::new();
        for (display, p, _) in &inp.files {
            if let Approval::Inside(c) = approve(Path::new(p), &auth.source_root) {
                if c.starts_with(&repo) {
                    paths.push(c.display().to_string());
                } else {
                    outside_repo.push(display.clone());
                }
            }
        }
        if !outside_repo.is_empty() {
            repo_unverified = true;
            f.reasons.push(format!(
                "{} analysed source file(s) lie outside the approved repository ({}): whether they have uncommitted changes is unverified.",
                outside_repo.len(),
                few(&outside_repo)
            ));
        }
        args.extend(paths.iter().map(String::as_str));
        if !paths.is_empty() {
            if let Some(st) = git(&repo, &args) {
                for line in st.lines() {
                    let p = line.get(3..).unwrap_or("").trim();
                    if !p.is_empty() {
                        f.uncommitted.push(p.to_string());
                    }
                }
            } else {
                // A failed git status must fail closed rather than report zero
                // uncommitted changes.
                repo_unverified = true;
                f.reasons.push("git status failed in the approved repository: whether analysed files have uncommitted changes is unverified.".into());
            }
        }
        if !f.uncommitted.is_empty() {
            f.reasons.push(format!("{} analysed file(s) have uncommitted changes: the attribution describes the committed text.", f.uncommitted.len()));
        }
    }
    // ---- obligations vs. resolutions written since --------------------------
    if inp.has_impact && resolution_edits > 0 {
        f.reasons.push(format!("{resolution_edits} resolution(s) written since the obligations were computed: stale-review / stale-resolution findings may have been answered."));
    }
    // The impact recorded a digest of the evidence and resolutions it was
    // computed against. When the loaded ones already differ -- an evidence
    // rebuilt or a sidecar edited between runs -- the obligations are stale
    // even though this process has written nothing, so the counter above is
    // zero (#249). This is captured at load, when the resolutions are in hand.
    if let Some(reason) = &inp.impact_stale {
        f.reasons.push(reason.clone());
    }
    if !inp.requirements_stale.is_empty() {
        f.requirements_stale = inp.requirements_stale.clone();
        f.reasons.extend(inp.requirements_stale.iter().cloned());
    }
    f.stale = !f.reasons.is_empty();
    if f.stale {
        let history_stale = repo_unverified
            || f.commits_behind.is_some()
            || !f.sources_modified.is_empty()
            || !f.sources_missing.is_empty()
            || !f.sources_unreadable.is_empty()
            || !f.sources_unapproved.is_empty()
            || !f.uncommitted.is_empty()
            || (f.current_head.is_some() && f.current_head != f.recorded_head);
        let impact_stale = (inp.has_impact && resolution_edits > 0) || inp.impact_stale.is_some();
        f.rerun = if history_stale {
            Some(format!("butterfly changes {ev_arg}{}", inp.since.as_ref().map(|s| format!(" --since {}", crate::sh_quote(s))).unwrap_or_default()))
        } else if impact_stale {
            Some(format!("butterfly impact {ev_arg} --write"))
        } else {
            None
        };
    }
    f
}

#[cfg(test)]
mod tests {

    /// The hash comparison only ran when the read succeeded, and every error
    /// was discarded. So a source that had been deleted -- or whose
    /// permissions had changed, or that sat on a filesystem that had gone
    /// away -- left the evidence looking fresh, which is the one conclusion
    /// this check exists to prevent (#10).
    #[test]
    fn a_source_that_cannot_be_read_is_never_fresh() {
        let d = tempfile::Builder::new().prefix("butterfly-fresh-unread-").tempdir().unwrap().keep();

        let src = d.join("a.c");
        fs::write(&src, "int f(void) { return 1; }\n").unwrap();
        let sha = sha_hex(&fs::read(&src).unwrap());
        let input = |files: Vec<(String, String, String)>| Input {
            files,
            repo: None,
            head: Some("0".repeat(40)),
            since: None,
            has_changes: true,
            has_impact: false,
            impact_stale: None,
            requirements_stale: vec![],
        };
        let one = vec![("a.c".into(), src.display().to_string(), sha.clone())];

        // the baseline: it is there and it matches
        let f = check(&input(one.clone()), 0, Path::new("e.json"), &approve(&d));
        assert!(!f.stale, "an unchanged source on disk is fresh: {:?}", f.reasons);
        assert!(f.sources_missing.is_empty() && f.sources_unreadable.is_empty());

        // modified: unchanged behaviour, and still its own category
        fs::write(&src, "int f(void) { return 2; }\n").unwrap();
        let f = check(&input(one.clone()), 0, Path::new("e.json"), &approve(&d));
        assert!(f.stale);
        assert_eq!(f.sources_modified, ["a.c"], "{:?}", f.reasons);
        assert!(f.sources_missing.is_empty() && f.sources_unreadable.is_empty());

        // deleted
        fs::remove_file(&src).unwrap();
        let f = check(&input(one.clone()), 0, Path::new("e.json"), &approve(&d));
        assert!(f.stale, "a source that is gone cannot be verified: {:?}", f.reasons);
        assert_eq!(f.sources_missing, ["a.c"]);
        assert!(f.sources_modified.is_empty(), "gone is not the same as changed: {:?}", f.sources_modified);
        assert!(f.reasons.iter().any(|r| r.contains("not on this machine")), "{:?}", f.reasons);
        assert!(f.rerun.is_some(), "and it says what to run: {f:?}");

        let _ = fs::remove_dir_all(&d);
    }

    /// A persisted impact bound to the resolutions/evidence it was computed
    /// against must read as stale when the loaded ones already differ, even on
    /// a fresh process where nothing has been written yet (#249).
    #[test]
    fn a_persisted_impact_that_no_longer_binds_is_stale_with_no_in_process_edits() {
        let inp = Input {
            files: vec![],
            repo: None,
            head: Some("0".repeat(40)),
            since: None,
            has_changes: true,
            has_impact: true,
            impact_stale: Some("the resolutions have changed since this impact was recorded; recompute it with `butterfly impact`".into()),
            requirements_stale: vec![],
        };
        // no files, so the approved root is never consulted here
        let f = check(&inp, 0, Path::new("e.json"), &approve(&std::env::temp_dir()));
        assert!(f.stale, "a load-time impact drift is stale even with resolution_edits == 0: {:?}", f.reasons);
        assert!(f.reasons.iter().any(|r| r.contains("since this impact was recorded")), "{:?}", f.reasons);
        assert!(f.rerun.as_deref().unwrap().starts_with("butterfly impact"), "impact drift reruns impact, not changes: {:?}", f.rerun);
    }

    /// An analysed source too large for the cap is "we could not tell", not
    /// "it matches" and not "it differs" -- and it costs the cap, not the
    /// file's size, to find that out (#231).
    ///
    /// The paths in an evidence document are untrusted and this runs on
    /// `/api/freshness`, on every request, so an evidence naming a huge file
    /// was a way to make a running server read it.
    #[test]
    fn an_analysed_source_past_the_cap_is_refused_rather_than_read() {
        let d = tempfile::Builder::new().prefix("butterfly-fresh-cap-").tempdir().unwrap().keep();
        let src = d.join("big.c");
        fs::write(&src, vec![b'x'; 4096]).unwrap();
        let inp = Input {
            files: vec![("big.c".into(), src.display().to_string(), sha_hex(&fs::read(&src).unwrap()))],
            repo: None,
            head: Some("0".repeat(40)),
            since: None,
            has_changes: true,
            has_impact: false,
            impact_stale: None,
            requirements_stale: vec![],
        };

        // 1 KiB per kind, against a 4 KiB file
        let tiny = Limits::new(1024, 1024, 1024, 1024, 1024, 1024, 1 << 20);
        let f = check_with(&inp, 0, Path::new("e.json"), &tiny, &approve(&d));
        assert_eq!(f.sources_unreadable.len(), 1, "{:?}", f);
        assert!(f.sources_unreadable[0].starts_with("big.c ("), "it names the file: {:?}", f.sources_unreadable);
        assert!(f.sources_unreadable[0].contains("BUTTERFLY_MAX_SOURCE_MB"), "and the cap to raise: {:?}", f.sources_unreadable);
        assert!(!f.sources_unreadable[0].contains(d.to_str().unwrap()), "the display name is enough; no absolute path goes to the client (#340): {:?}", f.sources_unreadable);
        assert!(f.sources_modified.is_empty() && f.sources_missing.is_empty(), "a file that was not read neither matches nor differs: {f:?}");
        assert!(f.stale, "and unverified is not fresh: {:?}", f.reasons);

        // the control: with room for it, the same file is read and matches
        let roomy = Limits::new(1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20);
        let f = check_with(&inp, 0, Path::new("e.json"), &roomy, &approve(&d));
        assert!(!f.stale, "under the cap it is read and it matches: {:?}", f.reasons);
        assert!(f.sources_unreadable.is_empty(), "{:?}", f.sources_unreadable);

        let _ = fs::remove_dir_all(&d);
    }

    /// Every analysed source is read under one total budget, so a read charges
    /// it and a later source is refused when it is spent -- the aggregate
    /// invariant `read_analysed` keeps by charging `admit()` and any growth
    /// beyond the admitted length (#248). Without that charge the total budget
    /// is undercounted and the cap becomes per-file only.
    #[test]
    fn analysed_sources_share_one_total_budget() {
        let d = scratch("total-budget");
        let mk = |name: &str, fill: u8| {
            let p = d.join(name);
            fs::write(&p, vec![fill; 2048]).unwrap();
            (name.to_string(), p.display().to_string(), sha_hex(&fs::read(&p).unwrap()))
        };
        let inp = Input {
            files: vec![mk("a.c", b'a'), mk("b.c", b'b')],
            repo: None,
            head: Some("0".repeat(40)),
            since: None,
            has_changes: true,
            has_impact: false,
            impact_stale: None,
            requirements_stale: vec![],
        };
        // a generous per-file cap, but a total budget that fits only one 2 KiB file
        let tight = Limits::new(1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 3072);
        let f = check_with(&inp, 0, Path::new("e.json"), &tight, &approve(&d));
        assert_eq!(f.sources_unreadable.len(), 1, "the second source is past the shared total budget: {f:?}");
        assert!(f.sources_unreadable[0].contains("BUTTERFLY_MAX_TOTAL_MB"), "refused by the total budget, not the per-file cap: {:?}", f.sources_unreadable);
        assert!(f.stale, "a source left unread is not fresh: {:?}", f.reasons);

        // the control: a budget that fits both leaves neither refused
        let roomy = Limits::new(1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20);
        let f = check_with(&inp, 0, Path::new("e.json"), &roomy, &approve(&d));
        assert!(!f.stale && f.sources_unreadable.is_empty(), "under a budget that fits both, neither is refused: {f:?}");

        let _ = fs::remove_dir_all(&d);
    }

    /// Unreadable is a third thing: not "matches", not "differs", but "we
    /// could not tell" -- and the reason has to say which file and why.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_source_is_reported_as_unknown_not_as_matching() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { geteuid() } == 0 {
            butterfly_testsupport::not_applicable("running as root, permissions do not apply");
            return;
        }
        let d = tempfile::Builder::new().prefix("butterfly-fresh-perm-").tempdir().unwrap().keep();
        let src = d.join("a.c");
        fs::write(&src, "int f(void) { return 1; }\n").unwrap();
        let sha = sha_hex(&fs::read(&src).unwrap());
        fs::set_permissions(&src, fs::Permissions::from_mode(0o000)).unwrap();

        let inp = Input {
            files: vec![("a.c".into(), src.display().to_string(), sha)],
            repo: None,
            head: Some("0".repeat(40)),
            since: None,
            has_changes: true,
            has_impact: false,
            impact_stale: None,
            requirements_stale: vec![],
        };
        let f = check(&inp, 0, Path::new("e.json"), &approve(&d));

        assert!(f.stale, "an unverifiable source is not a fresh one: {:?}", f.reasons);
        assert_eq!(f.sources_unreadable.len(), 1, "{:?}", f.sources_unreadable);
        assert!(f.sources_unreadable[0].starts_with("a.c ("), "it names the file: {:?}", f.sources_unreadable);
        assert!(f.sources_unreadable[0].contains("permission denied"), "and why: {:?}", f.sources_unreadable);
        assert!(f.sources_modified.is_empty() && f.sources_missing.is_empty(), "and keeps the three apart");
        assert!(f.reasons.iter().any(|r| r.contains("is unknown")), "{:?}", f.reasons);

        let _ = fs::set_permissions(&src, fs::Permissions::from_mode(0o600));
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    unsafe fn geteuid() -> u32 {
        extern "C" {
            fn geteuid() -> u32;
        }
        geteuid()
    }

    use super::*;
    use std::fs;
    use std::process::Command;

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
    fn commit(dir: &Path, msg: &str) -> String {
        git_ok(dir, &["add", "-A"]);
        git_ok(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", msg]);
        git_ok(dir, &["rev-parse", "HEAD"])
    }

    #[test]
    fn stale_when_the_repository_moves_sources_change_or_resolutions_are_written() {
        if Command::new("git").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
            butterfly_testsupport::fixture_missing("git not installed");
            return;
        }
        let d = tempfile::Builder::new().prefix("butterfly-fresh-").tempdir().unwrap().keep();
        let d = d.canonicalize().unwrap();
        git_ok(&d, &["init", "-q", "-b", "main"]);
        let src = d.join("a.c");
        fs::write(&src, "int a(void) { return 1; }\n").unwrap();
        let head = commit(&d, "ABC-1: a");
        let inp = Input {
            files: vec![("a.c".into(), src.display().to_string(), sha_hex(b"int a(void) { return 1; }\n"))],
            repo: Some(d.display().to_string()),
            head: Some(head.clone()),
            since: Some("v1".into()),
            has_changes: true,
            has_impact: true,
            impact_stale: None,
            requirements_stale: vec![],
        };
        let ev_path = d.join("evidence.json");
        let auth = Authority::resolve(Some(&d), None).unwrap();
        assert_eq!(auth.repo.as_deref(), Some(d.as_path()), "the repository is found from the source root");
        // fresh
        let f = check(&inp, 0, &ev_path, &auth);
        assert!(!f.stale, "{:?}", f.reasons);
        assert_eq!(f.current_head.as_deref(), Some(head.as_str()));
        assert!(f.rerun.is_none());
        // resolutions written since -> impact stale, history fine
        let f = check(&inp, 2, &ev_path, &auth);
        assert!(f.stale && f.reasons.len() == 1);
        assert!(f.rerun.as_deref().unwrap().starts_with("butterfly impact"), "{:?}", f.rerun);
        // uncommitted edit on disk -> sources differ + uncommitted
        fs::write(&src, "int a(void) { return 2; }\n").unwrap();
        let f = check(&inp, 0, &ev_path, &auth);
        assert_eq!(f.sources_modified, vec!["a.c"]);
        assert_eq!(f.uncommitted.len(), 1, "{:?}", f);
        assert!(f.rerun.as_deref().unwrap().contains("butterfly changes") && f.rerun.as_deref().unwrap().contains("--since v1"));
        // committed -> the repository moved on by one
        commit(&d, "ABC-2: bump");
        let f = check(&inp, 0, &ev_path, &auth);
        assert_eq!(f.commits_behind, Some(1));
        assert!(f.reasons.iter().any(|r| r.contains("moved on")), "{:?}", f.reasons);
        // no history at all -> not stale, but a hint
        let none = Input { has_changes: false, ..inp.clone() };
        let f = check(&none, 5, &ev_path, &auth);
        assert!(!f.stale && f.hint.is_some());
        let _ = fs::remove_dir_all(&d);
    }

    /// Approve `d` as the source root, with no repository.
    fn approve(d: &Path) -> Authority {
        Authority { source_root: d.canonicalize().unwrap(), repo: None }
    }

    fn scratch(tag: &str) -> PathBuf {
        // a fresh, unpredictable directory, not a name another user could
        // plant a symlink at (#344)
        let d = tempfile::Builder::new().prefix(&format!("butterfly-fresh-{tag}-")).tempdir().unwrap().keep();
        d.canonicalize().unwrap()
    }

    fn one_file(path: &Path, sha: String) -> Input {
        Input {
            files: vec![("a.c".into(), path.display().to_string(), sha)],
            repo: None,
            head: Some("0".repeat(40)),
            since: None,
            has_changes: true,
            has_impact: false,
            impact_stale: None,
            requirements_stale: vec![],
        }
    }

    /// The paths in an evidence document are untrusted, and the viewer used
    /// to hash whatever they named. An evidence naming a file outside the
    /// tree it was served for turned `/api/freshness` into a way to ask a
    /// running server about any file it could open -- does it exist, is it
    /// readable, does it hash to this (#248). A path outside the approved
    /// root is now not opened at all, and says so; an unverified source is
    /// not a fresh one.
    #[test]
    fn a_source_outside_the_approved_root_is_never_read() {
        let d = scratch("outside");
        let root = d.join("root");
        fs::create_dir_all(&root).unwrap();
        let secret = d.join("secret.c");
        fs::write(&secret, "SENSITIVE\n").unwrap();

        // the evidence even carries the right hash: were it read, it would match
        let f = check(&one_file(&secret, sha_hex(b"SENSITIVE\n")), 0, Path::new("e.json"), &approve(&root));
        assert_eq!(f.sources_unapproved.len(), 1, "{f:?}");
        assert!(f.sources_unapproved[0].starts_with("a.c"), "{:?}", f.sources_unapproved);
        assert!(f.sources_modified.is_empty() && f.sources_missing.is_empty() && f.sources_unreadable.is_empty(), "it was neither read nor probed: {f:?}");
        assert!(f.stale, "and unverified is not fresh: {:?}", f.reasons);
        assert!(f.reasons.iter().any(|r| r.contains("--source-root")), "and it says how to approve the tree: {:?}", f.reasons);

        // a path that climbs out lexically is refused the same way
        let climbing = root.join("..").join("secret.c");
        let f = check(&one_file(&climbing, sha_hex(b"SENSITIVE\n")), 0, Path::new("e.json"), &approve(&root));
        assert_eq!(f.sources_unapproved.len(), 1, "{f:?}");

        // and a relative one has no place to be judged from
        let f = check(&one_file(Path::new("secret.c"), String::new()), 0, Path::new("e.json"), &approve(&root));
        assert_eq!(f.sources_unapproved.len(), 1, "{f:?}");
        let _ = fs::remove_dir_all(&d);
    }

    /// A name inside the root is not enough: the kernel is asked where the
    /// opened file really is, as ingest does.
    #[cfg(unix)]
    #[test]
    fn a_link_in_the_root_to_a_file_outside_it_is_not_read() {
        let d = scratch("link");
        let root = d.join("root");
        fs::create_dir_all(&root).unwrap();
        let secret = d.join("secret.c");
        fs::write(&secret, "SENSITIVE\n").unwrap();
        let sym = root.join("sym.c");
        std::os::unix::fs::symlink(&secret, &sym).unwrap();
        let hard = root.join("hard.c");
        fs::hard_link(&secret, &hard).unwrap();

        for alias in [&sym, &hard] {
            let f = check(&one_file(alias, sha_hex(b"SENSITIVE\n")), 0, Path::new("e.json"), &approve(&root));
            assert_eq!(f.sources_unapproved.len(), 1, "{}: {f:?}", alias.display());
            assert!(f.sources_modified.is_empty() && f.sources_unreadable.is_empty(), "{f:?}");
            assert!(f.stale, "{:?}", f.reasons);
        }
        let _ = fs::remove_dir_all(&d);
    }

    /// The repository comes from the evidence too, and git is not a safe
    /// thing to point at an arbitrary directory: a repository's own config
    /// runs programs (`core.fsmonitor` on every `git status`). Only the
    /// repository the operator approved is ever handed to git (#248).
    #[cfg(unix)]
    #[test]
    fn a_repository_other_than_the_approved_one_is_never_handed_to_git() {
        use std::os::unix::fs::PermissionsExt;
        if Command::new("git").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
            butterfly_testsupport::fixture_missing("git not installed");
            return;
        }
        let d = scratch("repo");
        let approved = d.join("approved");
        let hostile = d.join("hostile");
        fs::create_dir_all(&approved).unwrap();
        fs::create_dir_all(&hostile).unwrap();
        git_ok(&hostile, &["init", "-q", "-b", "main"]);
        let src = hostile.join("a.c");
        fs::write(&src, "int a(void) { return 1; }\n").unwrap();
        let head = commit(&hostile, "a");
        let marker = d.join("git-ran-a-program");
        let hook = d.join("hook.sh");
        fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        git_ok(&hostile, &["config", "core.fsmonitor", hook.to_str().unwrap()]);

        let inp = Input {
            files: vec![("a.c".into(), src.display().to_string(), sha_hex(b"int a(void) { return 1; }\n"))],
            repo: Some(hostile.display().to_string()),
            head: Some(head),
            since: None,
            has_changes: true,
            has_impact: false,
            impact_stale: None,
            requirements_stale: vec![],
        };
        let auth = Authority { source_root: approved.clone(), repo: Some(approved.clone()) };
        let f = check(&inp, 0, Path::new("e.json"), &auth);
        assert!(!marker.exists(), "git ran a program from a repository the operator never approved");
        assert!(f.current_head.is_none(), "git was not asked about it at all: {f:?}");
        assert!(f.reasons.iter().any(|r| r.contains("not the approved repository")), "{:?}", f.reasons);
        assert!(f.stale, "an unconsulted history is unverified: {:?}", f.reasons);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn failed_git_checks_and_unapproved_repo_fail_closed_with_quoted_rerun() {
        let d = scratch("git-fail-closed");
        let src = d.join("a.c");
        fs::write(&src, "int a(void) { return 1; }\n").unwrap();
        let sha = sha_hex(b"int a(void) { return 1; }\n");

        // Approved repo is a plain directory (no .git): git rev-parse HEAD and git status both fail.
        let inp = Input {
            files: vec![("a.c".into(), src.display().to_string(), sha.clone())],
            repo: Some(d.display().to_string()),
            head: Some("0".repeat(40)),
            since: Some("v1; touch /tmp/pwn".into()),
            has_changes: true,
            has_impact: false,
            impact_stale: None,
            requirements_stale: vec![],
        };
        let auth = Authority { source_root: d.clone(), repo: Some(d.clone()) };
        let f = check(&inp, 0, Path::new("my evidence.json"), &auth);
        assert!(f.stale, "failed git rev-parse/status must never look fresh: {f:?}");
        assert!(f.reasons.iter().any(|r| r.contains("git could not read HEAD")), "{:?}", f.reasons);
        assert!(f.reasons.iter().any(|r| r.contains("git status failed")), "{:?}", f.reasons);
        let rerun = f.rerun.as_deref().expect("rerun command");
        assert!(rerun.starts_with("butterfly changes 'my evidence.json' --since 'v1; touch /tmp/pwn'"), "{rerun}");

        // Unapproved repo with matching sources: rerun must suggest `butterfly changes`, not `butterfly impact`.
        let other = d.join("other-repo");
        fs::create_dir_all(&other).unwrap();
        let inp2 = Input { repo: Some(other.display().to_string()), since: Some("v1".into()), ..inp };
        let f2 = check(&inp2, 0, Path::new("e.json"), &auth);
        assert!(f2.stale);
        assert!(f2.rerun.as_deref().unwrap().starts_with("butterfly changes "), "{:?}", f2.rerun);

        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn out_of_repo_approved_source_does_not_suppress_in_repo_git_status() {
        if Command::new("git").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
            butterfly_testsupport::fixture_missing("git not installed");
            return;
        }
        let d = scratch("out-of-repo-status");
        let repo = d.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q", "-b", "main"]);
        let in_repo = repo.join("in.c");
        fs::write(&in_repo, "int a(void) { return 1; }\n").unwrap();
        let head = commit(&repo, "init");
        // Modify in_repo on disk and record its modified hash so hash check passes while git status sees it dirty.
        fs::write(&in_repo, "int a(void) { return 2; }\n").unwrap();
        let in_sha = sha_hex(b"int a(void) { return 2; }\n");
        let outside = d.join("ext.c");
        fs::write(&outside, "int b(void) { return 3; }\n").unwrap();
        let out_sha = sha_hex(b"int b(void) { return 3; }\n");

        let inp = Input {
            files: vec![("in.c".into(), in_repo.display().to_string(), in_sha), ("ext.c".into(), outside.display().to_string(), out_sha)],
            repo: Some(repo.display().to_string()),
            head: Some(head),
            since: None,
            has_changes: true,
            has_impact: false,
            impact_stale: None,
            requirements_stale: vec![],
        };
        let auth = Authority { source_root: d.clone(), repo: Some(repo.clone()) };
        let f = check(&inp, 0, Path::new("e.json"), &auth);
        assert!(f.stale);
        assert_eq!(f.uncommitted, vec!["in.c"], "in-repo uncommitted change is still detected: {f:?}");
        assert!(f.reasons.iter().any(|r| r.contains("outside the approved repository")), "{:?}", f.reasons);
        let _ = fs::remove_dir_all(&d);
    }
}
