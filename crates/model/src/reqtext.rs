// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirement *text*, beside the evidence rather than inside it.
//!
//! The evidence records what was measured: which lines ran, which outcomes
//! were taken, which object code answers for which source. A requirement's
//! prose is not a measurement. It belongs to the requirements tool, is
//! baselined on its own schedule, and changes for reasons that have nothing
//! to do with a build -- so it rides beside the evidence as
//! `requirements.json`, the way `tags.json` and the resolutions sidecar do,
//! and its absence changes nothing except that tags show an ID alone.
//!
//! Only **low-level** requirements are carried. DO-178C traces source code
//! to LLRs; high-level requirements trace through them, not to the code, so
//! an HLR in this file would be a category error rather than extra detail.
//!
//! "Low-level" includes **derived** requirements. A derived requirement is
//! one no higher-level requirement gave rise to -- it comes out of a design
//! decision -- and DO-178C 5.1.2.h has it fed to the safety assessment for
//! exactly that reason: there is no parent to validate it against. It is
//! still a low-level requirement, still implemented by code, and still
//! traced. Dropping derived entries would quietly remove the requirements
//! under the *most* scrutiny, so `kind` records what the document said and
//! nothing is filtered on it beyond HLRs.
//!
//! # Why the text is split from the rest
//!
//! Measured over the ARINC 653 corpus (304 LLRs, 25 files, 628 KB): the
//! statement itself averages 277 bytes, the whole record 1,988 -- so `text`
//! is about a seventh of what an entry weighs. The source pane needs the
//! statement for the handful of requirements on one function; it does not
//! need every rationale and safety analysis in the programme. Keeping
//! `detail` separate is what lets a viewer ask for the small half.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const REQTEXT_SCHEMA: &str = "butterfly-requirements/1";

/// The requirement text beside one evidence file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Requirements {
    pub schema: String,
    /// The tool and version that produced this.
    pub generator: String,
    /// Every file this was read from, each with its own digest.
    ///
    /// A list, not one hash: requirements live one file per component (25 of
    /// them in the corpus this was built against), and a single digest over
    /// the set could not say *which* document moved.
    pub sources: Vec<Source>,
    /// Requirements by ID, sorted, so a reader can binary-search and a diff
    /// of two sidecars is readable.
    pub entries: Vec<Requirement>,
    /// Requirements carrying no statement. They are kept -- an entry with an
    /// ID is traceable whether or not anyone wrote its text -- but a
    /// traceability matrix built on requirements that say nothing is a
    /// matrix that cannot be reviewed, so the reader is told.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub without_text: Vec<String>,
    /// Entries that were read and *not* kept, by the kind that excluded them.
    ///
    /// Nothing may vanish quietly. A reader who expected 304 requirements and
    /// got 245 has to be able to see where the other 59 went without
    /// diffing the source documents by hand.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skipped: BTreeMap<String, usize>,
}

impl Requirement {
    /// A derived requirement has no parent by definition (DO-178C 5.1.2.h),
    /// so an absent one is expected here and a gap anywhere else.
    pub fn is_derived(&self) -> bool {
        self.kind.as_deref() == Some("derived")
    }
}

/// One file the text came from.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// Path as given, for a human to recognise.
    pub path: String,
    /// SHA-256 of the file's bytes. A Compliance Verification Engineer signs
    /// against a baseline; text shown without one is text that cannot be
    /// checked, which is worse than no text.
    pub sha256: String,
    /// How many requirements came out of this file.
    pub count: usize,
}

/// What a low-level requirement says, and what it claims about itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Requirement {
    pub id: String,
    /// The requirement statement: the half the source pane shows inline.
    pub text: String,
    /// `low-level`, `derived`, ... as written. Kept rather than collapsed:
    /// a derived requirement has no parent by definition, so "no parent" is
    /// correct for it and a finding for any other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `planned`, `implemented`, ... as written in the source document.
    ///
    /// This is not decoration. 212 of the corpus's 304 entries are
    /// `planned`, and a planned requirement with no code is *expected*.
    /// Without this, every one of them reads as an orphan and buries the
    /// handful that are genuinely missing their implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The high-level requirement this decomposes, where the document says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// What the document *claims* implements and verifies it, as written.
    ///
    /// Never treated as fact: butterfly derives the same two mappings from
    /// tags and from per-test coverage, and the point of keeping the claim is
    /// that the two accounts can be compared (#57).
    #[serde(default, skip_serializing_if = "Claim::is_empty")]
    pub claim: Claim,
    /// Everything else the document carried -- rationale, safety analysis,
    /// verification criteria -- kept whole but out of the inline path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub detail: BTreeMap<String, String>,
    /// Which source file this came from, as an index into `sources`.
    pub source: usize,
    /// 1-based line of this entry's `id`, so a reader can be sent to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// The document's own traceability claim.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    /// Code said to implement it, as written (`path::symbol` in the corpus).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub code: Vec<String>,
    /// Tests said to verify it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<String>,
}

impl Claim {
    pub fn is_empty(&self) -> bool {
        self.code.is_empty() && self.tests.is_empty()
    }
}

impl Requirements {
    pub fn check_schema(&self) -> Result<(), String> {
        if self.schema == REQTEXT_SCHEMA {
            Ok(())
        } else {
            Err(format!("unsupported requirements schema {:?} (expected {REQTEXT_SCHEMA})", self.schema))
        }
    }

    /// Refuse a document a consumer would have to second-guess.
    pub fn validate(&self) -> Result<(), String> {
        self.check_schema()?;
        // A digest is what a reviewer signs against, and consumers shorten it
        // for display. Anything but SHA-256 hex is not one ingest wrote
        // (#338).
        for (i, s) in self.sources.iter().enumerate() {
            if s.sha256.len() != 64 || !s.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                return Err(format!("sources[{i}] `{}`: sha256 {:?} is not 64 lowercase hex characters", s.path, s.sha256.chars().take(80).collect::<String>()));
            }
        }
        let n = self.sources.len();
        let mut last: Option<&str> = None;
        for (i, r) in self.entries.iter().enumerate() {
            if r.id.is_empty() {
                return Err(format!("entries[{i}]: a requirement with no ID"));
            }
            if r.source >= n {
                return Err(format!("entries[{i}] `{}`: source {} out of range (len {n})", r.id, r.source));
            }
            // Sorted and unique, because the viewer looks entries up by ID and
            // two entries for one ID would make which-one-you-get depend on
            // search order.
            match last {
                Some(p) if p == r.id.as_str() => return Err(format!("entries[{i}]: `{}` appears twice", r.id)),
                Some(p) if p > r.id.as_str() => return Err(format!("entries[{i}]: `{}` follows `{p}`, but entries must be sorted by ID", r.id)),
                _ => {}
            }
            last = Some(&r.id);
        }
        Ok(())
    }

    /// The text for one ID, if it is here.
    pub fn get(&self, id: &str) -> Option<&Requirement> {
        self.entries.binary_search_by(|r| r.id.as_str().cmp(id)).ok().map(|i| &self.entries[i])
    }
}
