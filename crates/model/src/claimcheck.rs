// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! What the requirements document *claims*, against what the build
//! *measured* (DO-178C 6.4.4.2, and the traceability half of A-7 objectives
//! 3 and 4).
//!
//! A requirements file carries its own traceability: which code implements a
//! requirement, which tests verify it. butterfly derives both mappings
//! independently -- requirement to code from the `@satisfies` tags in the
//! source, test to code from per-test coverage exports. So there are two
//! accounts of the same facts, made by different means:
//!
//! * the document says what somebody **intended**, and wrote down;
//! * the evidence says what the compiler and the test run **did**.
//!
//! Neither is worth much alone. A hand-maintained traceability argument
//! always looks complete, because the hand that maintains it is the hand
//! that decides when it is done. Comparing it with the measurement is what
//! makes it checkable, and that is this module.
//!
//! Nothing here rewrites the requirements file. Editing somebody's
//! requirements from a coverage tool is not this tool's business; saying
//! precisely where the two accounts differ is.
//!
//! # Matching is explicit, never fuzzy
//!
//! A claim is written `path::symbol` (`crates/kernel/src/collections.rs::FixedVec`).
//! Resolving it to a function has to be exact, because a near-miss here
//! produces a *false agreement* -- the one outcome worse than a false
//! disagreement, since it reports a requirement as traced when nothing
//! checked it. So:
//!
//! * **paths** match on whole components from the right, against the path
//!   the coverage export *recorded*, never the shortened display path. The
//!   document's path is usually the shorter (`src/limits.c` against
//!   `/work/src/limits.c`), so the shorter must be a component-wise suffix of
//!   the longer. `limits.c` matches `src/limits.c`; `mylimits.c` does not.
//!   Matching the display path, which may be a bare `limits.c`, let
//!   `other/limits.c` resolve to `src/limits.c` (#325 C1).
//! * **symbols** match exactly, or against the `file.c:name` form the
//!   evidence uses to keep two same-named statics apart -- that encoding
//!   specifically, and nothing looser. Stripping to the last colon would let
//!   the under-qualified `demo.cpp::foo` match an evidence `ns::foo`, which
//!   is a false agreement rather than a near miss.
//! * a claim's **path ends at the first `::`**, so a namespaced symbol keeps
//!   its namespace: `demo.cpp::ns::foo` is `ns::foo` in `demo.cpp`, not
//!   `foo` in a file called `demo.cpp::ns`.
//! * a claim resolving to **nothing** is reported, not ignored. A renamed or
//!   deleted symbol is the most common way a traceability argument rots.
//!
//! # A test verifies the requirement's lines, not its function
//!
//! `@satisfies` on one line claims that line; above a function it claims the
//! whole function. So "did this test verify the requirement" is answered
//! against the lines the requirement actually claims, intersected with the
//! lines the test executed -- not against whether the test entered the
//! function.
//!
//! Entering elsewhere in the function is not verification. Accepting it
//! would report an unverified requirement as verified, which is the same
//! inversion #42 had to fix in the matrix and the one this module exists to
//! prevent rather than commit (review of #57).

use crate::reqtext::Requirements;
use crate::Evidence;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One way the document and the measurement disagree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Disagreement {
    /// A stable kind, for a gate to select on.
    pub kind: String,
    pub requirement: String,
    /// What the document asserts, in its own words.
    pub claimed: String,
    /// What the evidence shows.
    pub measured: String,
}

/// One claim the measurement cannot answer, and why.
///
/// The third outcome, and the reason it has to exist. A claim that was
/// checked and held, and a claim nobody could look at, are not the same
/// finding; folding the second into `agreed` made a verification gate report
/// agreement it never established, and `--json` carried nothing that could
/// tell them apart (#233).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotChecked {
    /// A stable kind, for a gate to select on.
    pub kind: String,
    pub requirement: String,
    /// What the document asserts, in its own words.
    pub claimed: String,
    /// Why the measurement could not answer it, and where possible what
    /// would let it.
    pub reason: String,
}

/// The comparison.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub disagreements: Vec<Disagreement>,
    /// Requirements whose every claim agreed with the measurement.
    ///
    /// Counted, because a reviewer needs to see that the check ran and found
    /// nothing -- an empty list of problems and a check that never looked
    /// are the same page otherwise.
    ///
    /// Every claim, and *checked*: a requirement with any claim in
    /// `not_checked` is not here, however well its other claims did (#233).
    pub agreed: Vec<String>,
    /// Requirements the document makes no claim about, so there was nothing
    /// to check. Not a fault; not agreement either.
    pub unclaimed: Vec<String>,
    /// Claims the measurement could not answer.
    ///
    /// Neither agreement nor disagreement. A requirement listed here is in
    /// neither `agreed` nor `unclaimed`: the document did speak, and this
    /// tool did not establish that the measurement agreed.
    ///
    /// `#[serde(default)]` so a report written before this field existed
    /// still loads -- as empty, which for those reports is a claim they
    /// cannot support. They predate the distinction; they do not disprove it.
    #[serde(default)]
    pub not_checked: Vec<NotChecked>,
}

/// Split `path::symbol` at the *first* `::`.
///
/// A file path has no `::` in it; a C++ or Rust symbol may have several. So
/// the first separator is the boundary and everything after it is the
/// symbol, namespaces included. Splitting at the last one instead read
/// `demo.cpp::ns::foo` as `foo` inside a file named `demo.cpp::ns`, and then
/// failed to resolve a claim that was perfectly correct (review of #57).
fn split_claim(c: &str) -> Option<(&str, &str)> {
    let i = c.find("::")?;
    let (p, s) = (c[..i].trim(), c[i + 2..].trim());
    (!p.is_empty() && !s.is_empty()).then_some((p, s))
}

/// Is `short` a component-wise suffix of `long`?
fn path_suffix(short: &str, long: &str) -> bool {
    let (short, long) = (short.replace('\\', "/"), long.replace('\\', "/"));
    let a: Vec<&str> = short.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    let b: Vec<&str> = long.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    !a.is_empty() && a.len() <= b.len() && a.iter().rev().zip(b.iter().rev()).all(|(x, y)| x == y)
}

fn paths_match(a: &str, b: &str) -> bool {
    path_suffix(a, b) || path_suffix(b, a)
}

/// Exact, or the evidence's own `file.c:name` encoding for a file-scoped
/// static -- that one form and no other.
///
/// The looser "everything after the last colon" let `demo.cpp::foo` match an
/// evidence function named `ns::foo`: an under-qualified claim reported as
/// agreement, which is the failure this whole module exists to catch rather
/// than commit (review of #57).
fn symbol_matches(fn_name: &str, symbol: &str) -> bool {
    if fn_name == symbol {
        return true;
    }
    // `file.c:name`: exactly one colon, and what precedes it is a file name,
    // never a namespace (those are written `::`).
    match fn_name.split_once(':') {
        Some((file, rest)) => !file.is_empty() && !file.contains(':') && !rest.starts_with(':') && rest == symbol,
        None => false,
    }
}

/// Every function a claim resolves to.
fn resolve(ev: &Evidence, claim: &str) -> Vec<usize> {
    let Some((path, symbol)) = split_claim(claim) else { return Vec::new() };
    ev.functions.iter().filter(|f| symbol_matches(&f.name, symbol) && paths_match(path, &ev.files[f.file_id].path)).map(|f| f.id).collect()
}

/// Compare the document with the measurement.
pub fn compare(ev: &Evidence, reqs: &Requirements) -> Report {
    // requirement -> the functions the *evidence* says implement it
    let mut tagged: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    for f in &ev.functions {
        for r in &f.requirements {
            tagged.entry(r.as_str()).or_default().insert(f.id);
        }
        for rs in f.line_requirements.values() {
            for r in rs {
                tagged.entry(r.as_str()).or_default().insert(f.id);
            }
        }
    }
    // requirement -> file -> the lines it actually claims there. A tag above
    // a function claims its whole body; a tag on a line claims that line.
    let mut scope: BTreeMap<&str, BTreeMap<usize, BTreeSet<u32>>> = BTreeMap::new();
    for f in &ev.functions {
        let file = &ev.files[f.file_id];
        // clamped to the file: a range the evidence never checked must not
        // become a four-billion-step loop here
        let last = file.lines.len().min(u32::MAX as usize) as u32;
        let body: Vec<u32> = (f.line_start.max(1)..=f.line_end.min(last)).collect();
        for r in &f.requirements {
            scope.entry(r.as_str()).or_default().entry(f.file_id).or_default().extend(body.iter().copied());
        }
        for (&line, rs) in &f.line_requirements {
            for r in rs {
                scope.entry(r.as_str()).or_default().entry(f.file_id).or_default().insert(line);
            }
        }
    }

    let test_by_name: BTreeMap<&str, usize> = ev.tests.iter().map(|t| (t.name.as_str(), t.id)).collect();
    let per_test = !ev.tests.is_empty();

    /// Did `t` execute any line this requirement claims?
    fn reaches(t: &crate::TestRun, scoped: &BTreeMap<usize, BTreeSet<u32>>) -> bool {
        scoped.iter().any(|(file_id, lines)| t.lines.get(file_id).is_some_and(|ranges| ranges.iter().any(|&(a, b)| lines.range(a..=b).next().is_some())))
    }

    let mut out = Report::default();
    for req in &reqs.entries {
        let measured: &BTreeSet<usize> = tagged.get(req.id.as_str()).unwrap_or(&EMPTY);
        let before = out.disagreements.len();
        let before_unchecked = out.not_checked.len();

        // 1. code the document claims, that the evidence does not show
        let mut claimed_fns: BTreeSet<usize> = BTreeSet::new();
        for c in &req.claim.code {
            let hits = resolve(ev, c);
            if hits.is_empty() {
                out.disagreements.push(Disagreement {
                    kind: "code-not-found".into(),
                    requirement: req.id.clone(),
                    claimed: c.clone(),
                    measured: "no function of that name in that file was analysed — renamed, deleted, or never built".into(),
                });
                continue;
            }
            for h in &hits {
                claimed_fns.insert(*h);
                if !measured.contains(h) {
                    out.disagreements.push(Disagreement {
                        kind: "claimed-not-tagged".into(),
                        requirement: req.id.clone(),
                        claimed: c.clone(),
                        measured: format!("`{}` carries no tag for this requirement", ev.functions[*h].name),
                    });
                }
            }
        }

        // 2. code the evidence shows, that the document does not claim.
        // Only when the document claims *something*: a requirement with no
        // traceability block has not made a wrong statement, it has made
        // none, and that is `unclaimed` below.
        if !req.claim.code.is_empty() {
            for f in measured.difference(&claimed_fns) {
                out.disagreements.push(Disagreement {
                    kind: "tagged-not-claimed".into(),
                    requirement: req.id.clone(),
                    claimed: "not in the document's traceability".into(),
                    measured: format!("`{}` is tagged with it in the source", ev.functions[*f].name),
                });
            }
        }

        // 3. a test the document says verifies it, that never reached the code
        for t in &req.claim.tests {
            let name = split_claim(t).map(|(_, s)| s).unwrap_or(t.as_str());
            let Some(&id) = test_by_name.get(name) else {
                if per_test {
                    out.disagreements.push(Disagreement {
                        kind: "test-not-found".into(),
                        requirement: req.id.clone(),
                        claimed: t.clone(),
                        measured: "no test of that name was run".into(),
                    });
                } else {
                    // Without per-test data every lookup misses, so this is
                    // not "the test is absent" -- it is "nobody looked".
                    // Inventing a disagreement here would be as wrong as
                    // inventing an agreement; saying nothing at all was the
                    // bug, because the requirement then fell through to
                    // `agreed` (#233).
                    out.not_checked.push(NotChecked {
                        kind: "no-per-test-data".into(),
                        requirement: req.id.clone(),
                        claimed: t.clone(),
                        reason: "this evidence carries no per-test coverage, so no test claim can be checked — re-run ingest with --tests".into(),
                    });
                }
                continue;
            };
            // Against the lines the requirement claims, not the functions it
            // sits in. The scope comes from the evidence's own tags; where
            // there are none, fall back to the bodies of the functions the
            // *document* claims, so a wrong code claim cannot excuse a wrong
            // test claim by leaving the scope empty.
            let scoped = scope.get(req.id.as_str()).cloned().unwrap_or_else(|| {
                let mut m: BTreeMap<usize, BTreeSet<u32>> = BTreeMap::new();
                for f in &claimed_fns {
                    let func = &ev.functions[*f];
                    let last = ev.files[func.file_id].lines.len().min(u32::MAX as usize) as u32;
                    m.entry(func.file_id).or_default().extend(func.line_start.max(1)..=func.line_end.min(last));
                }
                m
            });
            if scoped.is_empty() {
                // Per-test data exists and the test ran, but nothing locates
                // this requirement in the code: no tag in the evidence, and
                // no code claim in the document that resolved. There are no
                // lines to ask the test about, so the claim is unchecked --
                // the same silence as the branch above, reached a different
                // way, and equally not agreement. The issue does not mention
                // this path; it is the same defect (#233).
                out.not_checked.push(NotChecked {
                    kind: "claim-has-no-scope".into(),
                    requirement: req.id.clone(),
                    claimed: t.clone(),
                    reason: format!(
                        "`{name}` ran, but nothing locates this requirement in the code — it carries no tag in the evidence and the document names no code for it, so there are no lines to ask the test about"
                    ),
                });
                continue;
            }
            if !reaches(&ev.tests[id], &scoped) {
                out.disagreements.push(Disagreement {
                    kind: "test-does-not-reach".into(),
                    requirement: req.id.clone(),
                    claimed: t.clone(),
                    measured: format!("`{name}` ran, but executed none of the lines this requirement claims"),
                });
            }
        }

        // 4. said to be built, with nothing behind it. `planned` is expected
        // to have none, which is why status is carried at all.
        if req.status.as_deref() == Some("implemented") && measured.is_empty() && req.claim.code.is_empty() {
            out.disagreements.push(Disagreement {
                kind: "implemented-no-code".into(),
                requirement: req.id.clone(),
                claimed: "status: implemented".into(),
                measured: "no code is tagged with it and the document names none".into(),
            });
        }

        if out.disagreements.len() > before {
            continue;
        }
        // A claim nobody could look at is not agreement, so the requirement
        // is represented by its `not_checked` entries and by nothing else.
        // This is the line the issue asks for: `agreed` now means what its
        // doc comment has always said (#233).
        if out.not_checked.len() > before_unchecked {
            continue;
        }
        if req.claim.code.is_empty() && req.claim.tests.is_empty() && req.status.as_deref() != Some("implemented") {
            out.unclaimed.push(req.id.clone());
        } else {
            out.agreed.push(req.id.clone());
        }
    }

    // 5. the other direction: a tag citing an ID the document does not hold.
    // Every check above starts from the document, so a tag nobody wrote a
    // requirement for -- a typo, a retired ID -- was compared with nothing and
    // passed. Code claiming a requirement that does not exist is as much a
    // hole in the traceability as a requirement nothing implements (#325).
    // One per tagged function, as `tagged-not-claimed` is, and after the
    // document's own findings so theirs keep their order.
    for (id, fns) in &tagged {
        if reqs.get(id).is_some() {
            continue;
        }
        for f in fns {
            out.disagreements.push(Disagreement {
                kind: "tag-not-in-document".into(),
                requirement: (*id).to_string(),
                claimed: "not in the requirements document".into(),
                measured: format!("`{}` is tagged with it in the source", ev.functions[*f].name),
            });
        }
    }
    out
}

static EMPTY: BTreeSet<usize> = BTreeSet::new();

impl Report {
    /// No *disagreement*. Deliberately not "nothing to report": a report
    /// with unchecked claims is empty of disagreements and is still not a
    /// statement that the document and the measurement agree, which is why
    /// callers must consult `not_checked` before they say so (#233).
    pub fn is_empty(&self) -> bool {
        self.disagreements.is_empty()
    }
    /// The requirements with at least one claim that could not be checked,
    /// deduplicated, in the order they were first seen.
    pub fn not_checked_requirements(&self) -> Vec<&str> {
        let mut seen = BTreeSet::new();
        let mut ids = Vec::new();
        for n in &self.not_checked {
            if seen.insert(n.requirement.as_str()) {
                ids.push(n.requirement.as_str());
            }
        }
        ids
    }
    /// Disagreements by kind, for a one-line summary.
    pub fn by_kind(&self) -> BTreeMap<&str, usize> {
        let mut m = BTreeMap::new();
        for d in &self.disagreements {
            *m.entry(d.kind.as_str()).or_insert(0) += 1;
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reqtext::{Claim, Requirement, Requirements, Source, REQTEXT_SCHEMA};
    use crate::{Function, LineCov, SourceFile, SCHEMA};

    fn make_ev() -> Evidence {
        Evidence {
            schema: SCHEMA.into(),
            files: vec![SourceFile {
                id: 0,
                path: "a.c".into(),
                display: "a.c".into(),
                text: vec!["x".into()],
                lines: vec![LineCov { count: Some(1) }],
                function_ids: vec![0],
                ..Default::default()
            }],
            functions: vec![Function { id: 0, name: "f".into(), file_id: 0, line_start: 1, line_end: 1, requirements: vec!["REQ-1".into()], ..Default::default() }],
            ..Default::default()
        }
    }

    fn make_reqs(status: Option<&str>) -> Requirements {
        Requirements {
            schema: REQTEXT_SCHEMA.into(),
            generator: "test".into(),
            sources: vec![Source { path: "llr.yaml".into(), sha256: "0".repeat(64), count: 1 }],
            entries: vec![Requirement {
                id: "REQ-1".into(),
                text: "stmt".into(),
                kind: Some("low-level".into()),
                status: status.map(str::to_string),
                parent: None,
                claim: Claim::default(),
                detail: BTreeMap::new(),
                source: 0,
                line: None,
            }],
            without_text: vec![],
            skipped: BTreeMap::new(),
        }
    }

    #[test]
    fn requirement_with_no_claims_and_non_implemented_status_is_unclaimed_even_when_tagged() {
        let ev = make_ev();
        for status in [None, Some("planned")] {
            let r = compare(&ev, &make_reqs(status));
            assert!(r.disagreements.is_empty(), "{status:?}: {:?}", r.disagreements);
            assert!(r.agreed.is_empty(), "{status:?}: must not be reported as agreed without any claim: {:?}", r.agreed);
            assert_eq!(r.unclaimed, ["REQ-1"], "{status:?}: should be unclaimed");
        }
        // status: implemented IS a checked claim (disagrees via implemented-no-code when untagged)
        let r = compare(&ev, &make_reqs(Some("implemented")));
        assert!(r.disagreements.is_empty());
        assert_eq!(r.agreed, ["REQ-1"]);
        assert!(r.unclaimed.is_empty());
    }
}
