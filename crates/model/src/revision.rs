// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The project's source revision, in provenance (#299, #300).
//!
//! Which revision of the project's sources the evidence came from, and
//! whether the files the build used are that revision's files. Two rules
//! shape everything here:
//!
//! - **The revision id is a label; the file hashes are the proof.** Evidence
//!   already records a SHA-256 of every source ingest read. The state below
//!   is decided by comparing those same bytes with what the revision holds,
//!   never by asking the version-control tool whether it *thinks* the
//!   working copy is clean.
//! - **Three states, never two.** [`SourceState::Unknown`] carries its reason
//!   and never reads as clean: a missing tool, a refused repository, no
//!   commits yet, a timeout, a file that could not be verified.
//!
//! "Modified" is judged over the files the build used -- the coverage file
//! list plus the files the DWARF line tables name -- so an edited README does
//! not make the build modified. Changes outside that set are counted
//! separately in [`SourceRevision::outside_build`].
//!
//! The shape is the same for every version-control system. Git and SVN
//! (#301) fill the same fields; SVN adds the repository UUID and path in
//! [`RevisionId`], and what only SVN has -- mixed revisions, switched and
//! sparse directories, externals, and which files could be checked by hash
//! -- in [`SvnDetail`].

use crate::ExternalTool;
use serde::{Deserialize, Serialize};

/// How many changed paths are listed. The count beside the list is exact.
pub const LIST_CAP: usize = 200;

/// The version-control system the sources were found in.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Vcs {
    Git,
    Svn,
    /// Not in a git or SVN working copy.
    #[default]
    None,
}

/// When the revision was read. Ingest reads it itself; a build-time capture
/// (#302) is recorded beside it, and a difference between the two is a
/// caveat.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CapturedAt {
    Build,
    #[default]
    Ingest,
}

/// Whether the files the build used are the revision's files.
///
/// Serialised as `"clean"`, `"modified"` or `{"unknown": "<reason>"}`, so an
/// unknown state cannot be written without saying why.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceState {
    /// Every file the build used that is under version control was compared,
    /// byte for byte, with the revision, and every one matched.
    Clean,
    /// At least one file the build used differs from the revision, is not in
    /// it, or is missing; they are listed in [`SourceRevision::modified`].
    Modified,
    /// Whether the files are the revision's could not be established.
    Unknown(String),
}

impl Default for SourceState {
    fn default() -> Self {
        SourceState::Unknown("the source revision was not recorded".into())
    }
}

impl SourceState {
    pub fn is_clean(&self) -> bool {
        matches!(self, SourceState::Clean)
    }

    /// `clean`, `modified` or `unknown (<reason>)`, for a caveat or a line
    /// of a report.
    pub fn describe(&self) -> String {
        match self {
            SourceState::Clean => "clean".into(),
            SourceState::Modified => "modified".into(),
            SourceState::Unknown(r) => format!("unknown ({r})"),
        }
    }
}

/// The revision's identity. For git, `revision` is the commit id (40 hex
/// digits for a SHA-1 repository, 64 for SHA-256). For SVN (#301) it is the
/// working revision `svnversion` reports without its flags -- `1234`, or
/// `1230:1234` for a mixed-revision working copy -- and `repositoryUuid` and
/// `path` (the working copy's `^/`-relative URL) say which repository and
/// which part of it, since a revision number alone names neither.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RevisionId {
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// How a file the build used differs from the revision.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Change {
    /// Its content differs from the revision's.
    Content,
    /// Its content differs from the revision's only in line endings (CRLF
    /// against LF). Still a difference: the compiler read these bytes.
    LineEndings,
    /// The revision does not hold it: untracked, or added but not committed.
    NotInRevision,
    /// The revision holds it, but it is no longer on disk.
    Missing,
    /// A submodule whose checked-out commit is not the one the revision
    /// records.
    SubmoduleCommit,
}

/// One file the build used that is not the revision's.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangedSource {
    /// Relative to the working copy's top directory, with `/` separators.
    pub path: String,
    pub change: Change,
}

/// A submodule holding files the build used, judged against its own HEAD.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubmoduleRevision {
    /// Relative to the working copy's top directory.
    pub path: String,
    /// The commit checked out in the submodule, when it could be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The commit the parent revision records for it.
    pub recorded: String,
}

/// `provenance.sourceRevision`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceRevision {
    pub vcs: Vcs,
    /// Absent when there is no revision to name (no commits yet, or no
    /// working copy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RevisionId>,
    pub state: SourceState,
    /// The working copy's top directory, as found on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Where the repository lives (git: `remote.origin.url`), with any
    /// credentials removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Files the build used that are not the revision's: at most
    /// [`LIST_CAP`], sorted by path.
    #[serde(default)]
    pub modified: Vec<ChangedSource>,
    /// How many there are in all; exact even when the list is capped.
    #[serde(default)]
    pub modified_count: u64,
    /// Changed paths the build does not use. `None` when they could not be
    /// counted; the reason is in `notes`.
    #[serde(default)]
    pub outside_build: Option<u64>,
    /// Files the build used that lie outside the working copy (a vendored
    /// SDK, system headers): not under version control, so not judged. At
    /// most [`LIST_CAP`], sorted.
    #[serde(default)]
    pub unversioned: Vec<String>,
    #[serde(default)]
    pub unversioned_count: u64,
    /// How many files the build used were compared with the revision.
    #[serde(default)]
    pub verified_count: u64,
    #[serde(default)]
    pub submodules: Vec<SubmoduleRevision>,
    pub captured_at: CapturedAt,
    /// The version-control program that was run, with its version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ExternalTool>,
    /// What else a reader should know: a shallow clone, a lock file, a count
    /// that could not be made.
    #[serde(default)]
    pub notes: Vec<String>,
    /// What only an SVN working copy has (#301). Absent for git, and in
    /// evidence written before SVN was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svn: Option<SvnDetail>,
}

/// The shape of an SVN working copy, and how its files were judged (#301).
///
/// SVN keeps a checksum of each file's base copy, but in *normal form*: with
/// `svn:keywords` unexpanded and `svn:eol-style` line endings as LF. A file
/// carrying either property -- or `svn:special` -- legitimately differs from
/// its base copy on disk, so its checksum proves nothing about the working
/// file. Those files are judged by `svn status` alone and counted in
/// `statusOnly`; every other versioned file the build used is compared with
/// the base checksum, by the bytes ingest read, and counted in `hashVerified`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SvnDetail {
    /// `svnversion`'s answer as it gave it: `1234`, `1230:1234MSP`.
    pub svnversion: String,
    /// The lowest and highest working revision in the working copy. They
    /// differ in a mixed-revision working copy, whose files come from more
    /// than one revision -- a caveat even when every file is clean.
    pub low: u64,
    pub high: u64,
    /// svnversion's `M`: something in the working copy -- the build's files
    /// or not, content or properties -- is locally modified.
    #[serde(default)]
    pub locally_modified: bool,
    /// Paths switched to another URL (svnversion's `S`), relative to the
    /// working copy's top, sorted, at most [`LIST_CAP`].
    #[serde(default)]
    pub switched: Vec<String>,
    #[serde(default)]
    pub switched_count: u64,
    /// Directories checked out at less than full depth (svnversion's `P`),
    /// as `path (depth)`, sorted, at most [`LIST_CAP`].
    #[serde(default)]
    pub sparse: Vec<String>,
    #[serde(default)]
    pub sparse_count: u64,
    /// Versioned files the build used that were compared with their base
    /// checksum.
    #[serde(default)]
    pub hash_verified: u64,
    /// Versioned files the build used that `svn status` alone judged,
    /// because their properties make the base checksum inapplicable.
    #[serde(default)]
    pub status_only: u64,
    /// Externals holding files the build used, each judged as its own
    /// working copy.
    #[serde(default)]
    pub externals: Vec<SvnExternal>,
    /// The `svnversion` that ran; [`SourceRevision::tool`] is the `svn`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svnversion_tool: Option<ExternalTool>,
}

/// An SVN external holding files the build used. The parent's revision does
/// not determine an external's content, so its own is recorded.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SvnExternal {
    /// Relative to the top working copy.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RevisionId>,
    /// Its repository root URL, credentials removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Its own `svnversion`, when it could be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svnversion: Option<String>,
}

impl SourceRevision {
    /// An unknown state, with nothing else established.
    pub fn unknown(vcs: Vcs, reason: impl Into<String>) -> SourceRevision {
        SourceRevision { vcs, state: SourceState::Unknown(reason.into()), ..Default::default() }
    }

    /// Reject a record that contradicts itself -- above all one that reads
    /// clean while listing changes, or reads clean without a revision to be
    /// clean against. An evidence file is data from outside the process, and
    /// a hand-edited `"state": "clean"` must not survive loading.
    pub fn check(&self) -> Result<(), String> {
        let err = |m: &str| Err(format!("provenance.sourceRevision: {m}"));
        if (self.modified.len() as u64) > self.modified_count {
            return err("lists more modified files than its modifiedCount");
        }
        if (self.unversioned.len() as u64) > self.unversioned_count {
            return err("lists more unversioned files than its unversionedCount");
        }
        if self.modified.len() > LIST_CAP || self.unversioned.len() > LIST_CAP {
            return err("a list is longer than its cap");
        }
        match &self.state {
            SourceState::Clean => {
                if self.modified_count > 0 {
                    return err("is clean but counts modified files");
                }
                if self.vcs == Vcs::None || self.id.is_none() {
                    return err("is clean without a revision to be clean against");
                }
                if self.verified_count == 0 {
                    return err("is clean but verified no file");
                }
            }
            SourceState::Modified => {
                if self.modified_count == 0 {
                    return err("is modified but counts no modified file");
                }
            }
            SourceState::Unknown(r) => {
                if r.trim().is_empty() {
                    return err("is unknown without a reason");
                }
                if self.modified_count > 0 {
                    return err("counts modified files but is not modified");
                }
            }
        }
        for m in &self.submodules {
            if !is_object_id(&m.recorded) || m.commit.as_deref().is_some_and(|c| !is_object_id(c)) {
                return err("a submodule's commit is not a 40- or 64-hex-digit commit id");
            }
            // the producer calls a submodule at another commit modified, and
            // one whose commit it could not read unknown: neither is clean
            if self.state.is_clean() && m.commit.as_deref() != Some(m.recorded.as_str()) {
                return err("is clean but a submodule is not established at the commit the revision records");
            }
        }
        if let (Vcs::Git, Some(id)) = (self.vcs, &self.id) {
            if !is_object_id(&id.revision) {
                return err("a git revision is not a 40- or 64-hex-digit commit id");
            }
        }
        if let (Vcs::Svn, Some(id)) = (self.vcs, &self.id) {
            if svn_range(&id.revision).is_none() {
                return err("an SVN revision is not a number or a low:high range");
            }
            // a revision number alone is not unique across repositories
            if self.state.is_clean() && id.repository_uuid.as_deref().is_none_or(|u| u.trim().is_empty()) {
                return err("is clean at an SVN revision without the repository's UUID");
            }
        }
        if let Some(svn) = &self.svn {
            if self.vcs != Vcs::Svn {
                return err("carries SVN detail but is not SVN");
            }
            if svn.low > svn.high {
                return err("svn.low is above svn.high");
            }
            if let Some(id) = &self.id {
                if svn_range(&id.revision) != Some((svn.low, svn.high)) {
                    return err("the SVN revision does not match svn.low and svn.high");
                }
            }
            if svn.hash_verified.saturating_add(svn.status_only) > self.verified_count {
                return err("svn.hashVerified and svn.statusOnly add up to more than verifiedCount");
            }
            if (svn.switched.len() as u64) > svn.switched_count || (svn.sparse.len() as u64) > svn.sparse_count {
                return err("lists more switched or sparse paths than it counts");
            }
            if svn.switched.len() > LIST_CAP || svn.sparse.len() > LIST_CAP {
                return err("a list is longer than its cap");
            }
        }
        Ok(())
    }
}

/// An SVN working revision as svnversion gives it without its flags: `N`,
/// or `LOW:HIGH` for a mixed-revision working copy. The pair, low first.
pub fn svn_range(s: &str) -> Option<(u64, u64)> {
    let num = |t: &str| if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) { t.parse::<u64>().ok() } else { None };
    match s.split_once(':') {
        None => num(s).map(|n| (n, n)),
        Some((a, b)) => {
            let (a, b) = (num(a)?, num(b)?);
            (a <= b).then_some((a, b))
        }
    }
}

/// A full git object id: 40 hex digits (SHA-1) or 64 (SHA-256).
pub fn is_object_id(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> SourceRevision {
        SourceRevision {
            vcs: Vcs::Git,
            id: Some(RevisionId { revision: "a".repeat(40), ..Default::default() }),
            state: SourceState::Clean,
            verified_count: 3,
            ..Default::default()
        }
    }

    #[test]
    fn the_states_serialise_as_a_word_or_a_reason() {
        assert_eq!(serde_json::to_string(&SourceState::Clean).unwrap(), "\"clean\"");
        assert_eq!(serde_json::to_string(&SourceState::Modified).unwrap(), "\"modified\"");
        assert_eq!(serde_json::to_string(&SourceState::Unknown("no git".into())).unwrap(), "{\"unknown\":\"no git\"}");
        let back: SourceState = serde_json::from_str("{\"unknown\":\"x\"}").unwrap();
        assert_eq!(back, SourceState::Unknown("x".into()));
    }

    /// The default -- what a record missing its state would read as -- is
    /// unknown, never clean.
    #[test]
    fn the_default_state_is_unknown_not_clean() {
        assert!(!SourceState::default().is_clean());
        assert!(!SourceRevision::default().state.is_clean());
    }

    #[test]
    fn check_rejects_a_record_that_reads_clean_against_its_own_contents() {
        assert!(clean().check().is_ok());
        let mut r = clean();
        r.modified_count = 1;
        assert!(r.check().unwrap_err().contains("clean but counts"));
        let mut r = clean();
        r.id = None;
        assert!(r.check().unwrap_err().contains("without a revision"));
        let mut r = clean();
        r.verified_count = 0;
        assert!(r.check().unwrap_err().contains("verified no file"));
        let mut r = clean();
        r.state = SourceState::Modified;
        assert!(r.check().unwrap_err().contains("counts no modified"));
        let mut r = clean();
        r.state = SourceState::Unknown(" ".into());
        assert!(r.check().unwrap_err().contains("without a reason"));
        let mut r = clean();
        r.id = Some(RevisionId { revision: "HEAD".into(), ..Default::default() });
        assert!(r.check().unwrap_err().contains("commit id"));
        let mut r = clean();
        r.state = SourceState::Modified;
        r.modified_count = 1;
        r.modified = vec![ChangedSource { path: "a".into(), change: Change::Content }, ChangedSource { path: "b".into(), change: Change::Content }];
        assert!(r.check().unwrap_err().contains("more modified"));
    }

    fn svn_clean() -> SourceRevision {
        SourceRevision {
            vcs: Vcs::Svn,
            id: Some(RevisionId { revision: "3:5".into(), repository_uuid: Some("18cc2829-ffa0-4b66-8680-fe46dc2d09bb".into()), path: Some("^/trunk".into()) }),
            state: SourceState::Clean,
            verified_count: 3,
            svn: Some(SvnDetail { svnversion: "3:5S".into(), low: 3, high: 5, hash_verified: 2, status_only: 1, ..Default::default() }),
            ..Default::default()
        }
    }

    #[test]
    fn svn_revisions_are_a_number_or_a_range() {
        assert_eq!(svn_range("1234"), Some((1234, 1234)));
        assert_eq!(svn_range("0"), Some((0, 0)));
        assert_eq!(svn_range("1230:1234"), Some((1230, 1234)));
        for bad in ["", "12M", "1234:1230", ":5", "5:", "r5", "Unversioned directory", "-1", "+5"] {
            assert_eq!(svn_range(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn check_holds_an_svn_record_to_its_own_contents() {
        assert!(svn_clean().check().is_ok());
        let mut r = svn_clean();
        r.id.as_mut().unwrap().revision = "5M".into();
        assert!(r.check().unwrap_err().contains("not a number or a low:high range"));
        let mut r = svn_clean();
        r.id.as_mut().unwrap().repository_uuid = None;
        assert!(r.check().unwrap_err().contains("UUID"), "a bare revision number names no repository");
        let mut r = svn_clean();
        r.id.as_mut().unwrap().repository_uuid = None;
        r.state = SourceState::Unknown("locked".into());
        assert!(r.check().is_ok(), "an unknown record may lack it");
        let mut r = svn_clean();
        r.svn.as_mut().unwrap().high = 6;
        assert!(r.check().unwrap_err().contains("does not match"));
        let mut r = svn_clean();
        r.svn.as_mut().unwrap().status_only = 2;
        assert!(r.check().unwrap_err().contains("more than verifiedCount"));
        let mut r = clean();
        r.svn = svn_clean().svn;
        assert!(r.check().unwrap_err().contains("not SVN"));
        let mut r = svn_clean();
        r.svn.as_mut().unwrap().switched = vec!["sub".into()];
        assert!(r.check().unwrap_err().contains("switched or sparse"));
    }

    /// A record written by #300, with no `svn` field, still loads.
    #[test]
    fn svn_detail_is_optional_on_load() {
        let r: SourceRevision = serde_json::from_str(r#"{"vcs":"git","state":"modified","capturedAt":"ingest","modifiedCount":1}"#).unwrap();
        assert!(r.svn.is_none());
        let json = serde_json::to_string(&svn_clean()).unwrap();
        assert!(json.contains("\"hashVerified\":2") && json.contains("\"statusOnly\":1"), "{json}");
        assert_eq!(serde_json::from_str::<SourceRevision>(&json).unwrap(), svn_clean());
    }

    /// A clean record must agree with its own submodules: the producer
    /// calls a submodule at another commit modified, and one whose commit
    /// could not be read unknown, so neither may load as clean (#306 review).
    #[test]
    fn check_rejects_a_clean_record_whose_submodules_contradict_it() {
        let sub = |commit: Option<&str>, recorded: &str| SubmoduleRevision { path: "lib".into(), commit: commit.map(str::to_string), recorded: recorded.into() };
        let mut r = clean();
        r.submodules = vec![sub(Some(&"c".repeat(40)), &"c".repeat(40))];
        assert!(r.check().is_ok(), "a submodule at the recorded commit is consistent with clean");
        let mut r = clean();
        r.submodules = vec![sub(Some(&"c".repeat(40)), &"d".repeat(40))];
        assert!(r.check().unwrap_err().contains("submodule"), "clean with a submodule at another commit");
        let mut r = clean();
        r.submodules = vec![sub(None, &"d".repeat(40))];
        assert!(r.check().unwrap_err().contains("submodule"), "clean with a submodule whose commit is not established");
        // the ids themselves are commit ids, whatever the state
        let mut r = clean();
        r.state = SourceState::Modified;
        r.modified_count = 1;
        r.submodules = vec![sub(Some("HEAD"), &"d".repeat(40))];
        assert!(r.check().unwrap_err().contains("submodule"));
        let mut r = clean();
        r.state = SourceState::Modified;
        r.modified_count = 1;
        r.submodules = vec![sub(None, "main")];
        assert!(r.check().unwrap_err().contains("submodule"));
    }

    #[test]
    fn sha256_ids_are_accepted() {
        let mut r = clean();
        r.id = Some(RevisionId { revision: "b".repeat(64), ..Default::default() });
        assert!(r.check().is_ok());
    }
}
