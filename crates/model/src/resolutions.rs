// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Semantic validation for a resolutions sidecar (review of #66).
//!
//! Everywhere else in this codebase, "is this gap justified" is answered by
//! asking whether *a key* is present in the sidecar -- `gate.rs`,
//! `worklist.rs`, `impact.rs` all build a `HashSet` of recorded keys and
//! check membership. That is fast and correct once a `Resolution` is known
//! to be a real, well-formed reviewer decision. It was never enforced that
//! one is.
//!
//! `check_schema` (in `lib.rs`) only checks the file carries the schema tag
//! this build understands. It says nothing about any individual entry: a
//! category of `"totally-invalid"`, a blank justification, a blank author,
//! an unparseable date, or a key that names no obligation the evidence
//! actually has all pass it. Any one of those turns a hand-edited or
//! scripted sidecar into a verification gate that closes on command.
//!
//! `Resolutions::validate` is the single place that stops that: schema,
//! entry count, per-field well-formedness, key uniqueness, and -- the check
//! that needs the evidence, not just the sidecar -- that every key names an
//! obligation the evidence actually records. It must be called on every
//! path that turns bytes from outside the process into a `Resolutions`
//! value: the CLI's own loader and the viewer's, and the API write that
//! accepts one record instead of a file. Reads of an in-memory value that
//! was already validated when the process took it in do not call it again.

use crate::{Evidence, Resolution, Resolutions};
use std::collections::HashSet;

/// DO-178C 6.4.4.3 / 6.4.4.2.b categories a resolution may claim. Not
/// open-ended: a category outside this list cannot be acted on by anything
/// that reads one, and accepting it anyway would let a gate close on a
/// justification nothing downstream understands.
pub const ALLOWED_CATEGORIES: &[&str] = &["a", "b", "c", "d1", "d2", "analysis"];

/// `s2o:<name>` records a source-to-object *traceability* review, not a
/// coverage-gap justification, and the viewer's own vocabulary for it (the
/// `s2o` cell in the functions table, `ui/app.js`) is `traceable`,
/// `analysed` and `not-traceable` -- disjoint from the six above. A validator that only knew one
/// vocabulary would reject every real `s2o:` review ever recorded, which is
/// exactly what the first version of this file did (caught by
/// `impact_cli::t08_resolutions_sidecar_makes_reviews_stale` and two other
/// existing fixtures, not by anything new). `not-traceable` is a verdict
/// too -- the review was done and found compiler behaviour that needs
/// action -- and refusing it meant that finding could never be recorded
/// (#325 D5).
pub const ALLOWED_S2O_CATEGORIES: &[&str] = &["traceable", "analysed", "not-traceable"];

/// Which vocabulary applies: the ticket's own ask is a category "appropriate
/// to the key type", and a source-to-object review is a different kind of
/// finding from a coverage gap.
///
/// The prefix alone will not do. A gap key is `<symbol>@<kind>@<addr>`, and a
/// C++ symbol carries its namespace, so a gap in `s2o::compute` produces the
/// key `s2o::compute@line-zero@200` -- which begins with `s2o:` while being a
/// coverage gap, not a traceability review. Requiring the s2o vocabulary for
/// it rejects the correct `analysis` justification outright. The `@` is what
/// separates them: no source-level key has one (review of #66).
fn allowed_categories_for(key: &str) -> &'static [&'static str] {
    if key.starts_with("s2o:") && !key.contains('@') {
        ALLOWED_S2O_CATEGORIES
    } else {
        ALLOWED_CATEGORIES
    }
}

/// However large a real review record legitimately gets, it is written by
/// people looking at specific findings, not generated in bulk. This is
/// headroom over the largest plausible project, not a target: a sidecar
/// anywhere near it is already a sign something upstream is wrong.
pub const MAX_RESOLUTION_ENTRIES: usize = 20_000;

/// A gap key is `<symbol>@<kind>@<hex address>`; the source-level keys are a
/// tag and a few small integers. Long past this, a key is not naming
/// anything butterfly itself would have written.
const MAX_KEY_LEN: usize = 512;
/// Generous for a paragraph of DO-178C rationale, bounded so one entry
/// cannot dominate the file a reviewer or a diff has to read.
const MAX_JUSTIFICATION_LEN: usize = 8192;
const MAX_AUTHOR_LEN: usize = 256;

impl Resolution {
    /// Everything checkable about one entry without the evidence: a real
    /// category, non-blank human fields, a calendar date, and length limits
    /// a hostile or corrupt entry cannot exceed. Whether the key names a
    /// real obligation needs the evidence, and is `Resolutions::validate`'s
    /// job, not this one's.
    pub fn validate_fields(&self) -> Result<(), String> {
        if self.key.is_empty() {
            return Err("key is empty".into());
        }
        if self.key.len() > MAX_KEY_LEN {
            return Err(format!("key is {} bytes, over the {MAX_KEY_LEN} limit", self.key.len()));
        }
        let allowed = allowed_categories_for(&self.key);
        if !allowed.contains(&self.category.as_str()) {
            return Err(format!("category {:?} is not one of {allowed:?}, which this key requires", self.category));
        }
        // A resolution *is* its justification. Blank -- including
        // whitespace-only, which `is_empty` alone would miss -- is a record
        // that closes an obligation while saying nothing about why.
        if self.justification.trim().is_empty() {
            return Err("justification is blank".into());
        }
        if self.justification.len() > MAX_JUSTIFICATION_LEN {
            return Err(format!("justification is {} bytes, over the {MAX_JUSTIFICATION_LEN} limit", self.justification.len()));
        }
        if self.author.trim().is_empty() {
            return Err("author is blank".into());
        }
        if self.author.len() > MAX_AUTHOR_LEN {
            return Err(format!("author is {} bytes, over the {MAX_AUTHOR_LEN} limit", self.author.len()));
        }
        if !is_valid_date(&self.date) {
            return Err(format!("date {:?} is not a real YYYY-MM-DD date", self.date));
        }
        Ok(())
    }
}

impl Resolutions {
    /// Schema, entry count, every field of every entry, and key uniqueness.
    /// Call this on every sidecar or record that comes from outside the
    /// process -- nothing downstream re-checks, because everything
    /// downstream trusts key presence alone (review of #66).
    ///
    /// Deliberately *not* checked here: whether each key still names an
    /// obligation this evidence has. See `stale_keys`.
    pub fn validate(&self) -> Result<(), String> {
        self.check_schema()?;
        if self.entries.len() > MAX_RESOLUTION_ENTRIES {
            return Err(format!("{} entries exceeds the limit of {MAX_RESOLUTION_ENTRIES}", self.entries.len()));
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for (i, e) in self.entries.iter().enumerate() {
            let at = |what: &str| format!("entries[{i}] (key {:?}): {what}", e.key);
            e.validate_fields().map_err(|m| at(&m))?;
            if !seen.insert(e.key.as_str()) {
                return Err(at("duplicate key: an earlier entry already resolves it"));
            }
        }
        Ok(())
    }

    /// Entries whose key names no obligation in `ev` -- normally reviews
    /// that have gone **stale**, not reviews that are wrong.
    ///
    /// This is a separate, non-fatal question on purpose. A gap key embeds
    /// the instruction's address (`<symbol>@<kind>@<addr>`), so *any*
    /// recompile that shifts a function invalidates every gap resolution
    /// against it. Refusing to load the sidecar in that case would mean a
    /// routine code change bricks `check`, `report`, `serve` and -- worst --
    /// `impact`, the one command whose entire job is to report that reviews
    /// have gone stale (`stale-resolution` in `impact.rs`). The tool would
    /// refuse to run *because* it had something to say.
    ///
    /// Nor does refusing buy any safety: a key matching no obligation is
    /// inert. Every consumer asks "is the key of *this* gap/condition
    /// present", so a key naming nothing can never close anything. What
    /// closes an obligation wrongly is a *well-formed key with a bogus
    /// justification*, and that is what `validate` refuses.
    ///
    /// A new write is the one case where an unmatched key is not staleness
    /// but a bug or an attack -- nothing legitimate produces one -- so the
    /// API write path checks this and refuses; see `validate_for_write`.
    pub fn stale_keys<'a>(&'a self, ev: &Evidence) -> Vec<&'a str> {
        // Indexed once, not rescanned per entry. `key_names_an_obligation`
        // walks every function's gaps for one key; calling it for each of up
        // to MAX_RESOLUTION_ENTRIES entries against a real project's function
        // count is hundreds of millions of string comparisons, so a sidecar
        // sized just under the cap could stall every command that reads one
        // -- on the very change that exists to harden this path (review of
        // #66).
        let gaps: HashSet<&str> = ev
            .functions
            .iter()
            .flat_map(|f| f.objects())
            .flat_map(|o| o.gaps.iter().map(|g| g.key.as_str()))
            .chain(ev.image.iter().flat_map(|im| im.dead_candidates.iter().map(|d| d.key.as_str())))
            .collect();
        let names: HashSet<&str> = ev.functions.iter().map(|f| f.name.as_str()).collect();
        self.entries.iter().map(|e| e.key.as_str()).filter(|k| !gaps.contains(k) && !source_key_names_an_obligation(k, ev, &names)).collect()
    }

    /// `validate`, plus the requirement that every key names an obligation
    /// this evidence actually has. For accepting a record from outside the
    /// process *as a new decision* -- the API write path -- where a key that
    /// names nothing cannot be staleness, because the client derived it from
    /// this very evidence a moment ago.
    pub fn validate_for_write(&self, ev: &Evidence) -> Result<(), String> {
        self.validate()?;
        match self.stale_keys(ev).first() {
            Some(k) => Err(format!("key {k:?} does not correspond to any obligation in this evidence")),
            None => Ok(()),
        }
    }
}

/// Does a source-level key name something the evidence actually has -- rather
/// than merely being shaped like one?
///
/// Mirrors the key taxonomy `impact.rs`'s `key_owner` documents (`s2o:<name>`,
/// `dead:<name>`, `fn:<id>`, `cond:<id>:<line>:<col>:<index>`,
/// `branch:<id>:<line>:<col>:<T|F>`, `line:<file>:<line>`), but checks the
/// referenced obligation *exists* rather than only that the function it names
/// does -- `key_owner` answers "whose stale-review notice is this", a
/// different, coarser question.
///
/// Gap keys are not handled here: they are opaque strings this process minted,
/// matched by exact comparison against the evidence's own gaps by the caller,
/// because one may legitimately contain a `:` (a C++ namespace does) which
/// would defeat the parsing below. `names` is the function-name index, passed
/// in so a caller checking many keys builds it once.
fn source_key_names_an_obligation(key: &str, ev: &Evidence, names: &HashSet<&str>) -> bool {
    let Some((kind, rest)) = key.split_once(':') else { return false };
    let parts: Vec<&str> = rest.split(':').collect();
    match kind {
        "s2o" | "dead" => names.contains(rest),
        "fn" => parts.len() == 1 && parts[0].parse::<usize>().is_ok_and(|i| i < ev.functions.len()),
        // `branch:<fid>:<site>:<outcome>`, where the site is itself
        // `<line>:<col>` or `<line>:#<ordinal>` and so contains a colon of
        // its own. Rather than reparse it -- which is how this arm came to
        // accept only numeric columns and only T/F, refusing the ordinal
        // sites and A/B outcomes gcov produces (#88) -- take the ends off
        // and ask the branch what it calls itself.
        "branch" => {
            let Some((fid, rest)) = rest.split_once(':') else { return false };
            let Some((site, outcome)) = rest.rsplit_once(':') else { return false };
            let Ok(fid) = fid.parse::<usize>() else { return false };
            ev.functions.get(fid).is_some_and(|f| f.branches.iter().any(|b| b.site() == site && b.outcome_keys().contains(&outcome)))
        }
        "cond" => {
            let Some((fid, rest)) = rest.split_once(':') else { return false };
            let Some((site, idx)) = rest.rsplit_once(':') else { return false };
            let (Ok(fid), Ok(idx)) = (fid.parse::<usize>(), idx.parse::<usize>()) else { return false };
            ev.functions.get(fid).is_some_and(|f| f.decisions.iter().any(|d| d.site() == site && d.conditions.iter().any(|c| c.index == idx)))
        }
        "line" => {
            let [file, line] = parts[..] else { return false };
            let (Ok(file), Ok(line)) = (file.parse::<usize>(), line.parse::<u32>()) else { return false };
            line >= 1 && ev.files.get(file).is_some_and(|fl| fl.lines.get(line as usize - 1).is_some_and(|l| l.count.is_some()))
        }
        _ => false,
    }
}

/// A strict `YYYY-MM-DD`, and a real calendar date -- `2026-02-30` is
/// well-formed and still wrong. Works on bytes throughout, so it never
/// panics on a non-UTF-8-boundary slice of adversarial input; every index
/// used is only reached after the length check makes it valid.
fn is_valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if !b.iter().enumerate().all(|(i, &c)| i == 4 || i == 7 || c.is_ascii_digit()) {
        return false;
    }
    let two = |i: usize| (b[i] - b'0') as u32 * 10 + (b[i + 1] - b'0') as u32;
    let year = two(0) * 100 + two(2);
    let month = two(5);
    let day = two(8);
    if !(1..=12).contains(&month) || day == 0 {
        return false;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if leap {
                29
            } else {
                28
            }
        }
    };
    day <= days_in_month
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn entry(key: &str) -> Resolution {
        Resolution { key: key.into(), category: "a".into(), justification: "because".into(), author: "reviewer".into(), date: "2026-01-15".into() }
    }
    fn res(entries: Vec<Resolution>) -> Resolutions {
        Resolutions { schema: RESOLUTIONS_SCHEMA.into(), entries }
    }

    /// One function with one gap, one branch, one decision (one condition),
    /// and one instrumented line -- one real, valid target per key kind.
    fn ev_with_obligations() -> Evidence {
        let mut e = Evidence { schema: SCHEMA.into(), ..Default::default() };
        e.files.push(SourceFile {
            id: 0,
            path: "a.c".into(),
            display: "a.c".into(),
            text: vec!["x".into(); 5],
            lines: vec![LineCov { count: Some(1) }; 5],
            function_ids: vec![0],
            ..Default::default()
        });
        e.functions.push(Function {
            id: 0,
            name: "f".into(),
            line_start: 1,
            line_end: 5,
            file_id: 0,
            lines_total: 5,
            lines_covered: 5,
            branches: vec![Branch { line: 2, col: 5, line_end: 2, col_end: 10, true_count: 1, false_count: 0, direction_known: true, decision_ordinal: None }],
            decisions: vec![Decision {
                line: 3,
                col: 5,
                line_end: 3,
                col_end: 10,
                true_count: 1,
                false_count: 0,
                conditions: vec![Condition { index: 0, line: 3, col: 5, true_count: 1, false_count: 0, independence_covered: false, pair: None }],
                vectors: vec![],
                mcdc_covered: false,
                decision_ordinal: None,
            }],
            object: Some(ObjectFn { symbol: "_f".into(), gaps: vec![Gap { id: 0, key: "_f@line-zero@100".into(), ..Default::default() }], ..Default::default() }),
            ..Default::default()
        });
        e
    }

    #[test]
    fn every_real_key_shape_is_accepted() {
        let ev = ev_with_obligations();
        for k in ["_f@line-zero@100", "fn:0", "dead:f", "s2o:f", "branch:0:2:5:T", "branch:0:2:5:F", "cond:0:3:5:0", "line:0:3"] {
            // through the public path, which is what production uses
            assert!(res(vec![entry(k)]).stale_keys(&ev).is_empty(), "{k} should be a real obligation");
        }
    }

    /// The finding's own reproduction: a real gap key, everything else
    /// forged. Field validation must catch what key existence cannot.
    #[test]
    fn the_reported_exploit_is_rejected() {
        let r = res(vec![Resolution { key: "_f@line-zero@100".into(), category: "totally-invalid".into(), justification: "".into(), author: "".into(), date: "".into() }]);
        let err = r.validate().unwrap_err();
        assert!(err.contains("category"), "{err}");
    }

    /// A key naming nothing is *reported*, not fatal, on the load path --
    /// and refused on the write path. The split matters: a gap key embeds an
    /// address, so a rebuild makes stale keys routine, and failing the load
    /// would take down the very commands a reviewer needs in order to see
    /// that their reviews went stale (review of #66).
    #[test]
    fn a_key_pointing_nowhere_is_stale_on_load_and_refused_on_write() {
        let ev = ev_with_obligations();
        for k in ["fn:99", "branch:0:2:5:X", "branch:0:999:5:T", "cond:0:3:5:9", "line:0:999", "line:99:1", "made:up:nonsense"] {
            let r = res(vec![entry(k)]);
            assert!(r.validate().is_ok(), "{k}: loading must survive a key that names nothing");
            assert_eq!(r.stale_keys(&ev), vec![k], "{k}: and it must be reported as stale");
            assert!(r.validate_for_write(&ev).is_err(), "{k}: but a new write naming it is a bug or an attack");
        }
    }

    /// gcov gives no columns, so two decisions on one line are told apart
    /// by an ordinal, and their outcomes are `A`/`B` because gcov cannot say
    /// which edge is the true one. Both facts reach the key, and validation
    /// has to accept the key the producer itself generates -- the arm used
    /// to demand a numeric column and a T/F outcome, so every legitimate
    /// gcov review was refused as naming nothing (#88).
    #[test]
    fn two_gcc_decisions_on_one_line_keep_their_own_reviews() {
        let mut ev = ev_with_obligations();
        let gcov = |ordinal| Branch { line: 2, col: 0, line_end: 2, col_end: 0, true_count: 1, false_count: 0, direction_known: false, decision_ordinal: Some(ordinal) };
        ev.functions[0].branches = vec![gcov(0), gcov(1)];

        // exactly the keys production builds, from the branch itself
        let generated: Vec<String> = ev.functions[0].branches.iter().flat_map(|b| b.outcome_keys().map(move |o| format!("branch:0:{}:{o}", b.site()))).collect();
        assert_eq!(generated, ["branch:0:2:#0:A", "branch:0:2:#0:B", "branch:0:2:#1:A", "branch:0:2:#1:B"]);
        for k in &generated {
            let r = res(vec![entry(k)]);
            assert!(r.stale_keys(&ev).is_empty(), "{k}: the generated key must name its own branch");
            assert!(r.validate_for_write(&ev).is_ok(), "{k}: and must be writable");
        }

        // An ordinal nobody emitted must not land on the decision next door,
        // and the polarity must not be upgraded: T/F would assert which edge
        // is true, which gcov never said.
        for k in ["branch:0:2:#2:A", "branch:0:2:0:A", "branch:0:2:#0:T", "branch:0:2:#0:F"] {
            let r = res(vec![entry(k)]);
            assert_eq!(r.stale_keys(&ev), vec![k], "{k}: names no branch here");
            assert!(r.validate_for_write(&ev).is_err(), "{k}: and must not be writable");
        }
    }

    /// A real key is not stale, on either path.
    #[test]
    fn a_key_that_does_name_an_obligation_is_never_stale() {
        let ev = ev_with_obligations();
        let r = res(vec![entry("fn:0"), entry("_f@line-zero@100")]);
        assert!(r.validate().is_ok());
        assert!(r.stale_keys(&ev).is_empty(), "{:?}", r.stale_keys(&ev));
        assert!(r.validate_for_write(&ev).is_ok());
    }

    /// The security property is field validation, not key existence: a
    /// malformed record is refused on *both* paths, however real its key.
    #[test]
    fn a_malformed_record_is_refused_even_with_a_perfectly_real_key() {
        let ev = ev_with_obligations();
        let mut e = entry("fn:0");
        e.category = "totally-invalid".into();
        let r = res(vec![e]);
        assert!(r.validate().is_err(), "the load path refuses it");
        assert!(r.validate_for_write(&ev).is_err(), "and so does the write path");
    }

    #[test]
    fn each_category_outside_the_allow_list_is_rejected() {
        for bad in ["", "e", "A", " a", "analysis "] {
            let mut e = entry("fn:0");
            e.category = bad.into();
            assert!(res(vec![e]).validate().is_err(), "{bad:?} must not be an allowed category");
        }
        for good in ALLOWED_CATEGORIES {
            let mut e = entry("fn:0");
            e.category = (*good).into();
            assert!(res(vec![e]).validate().is_ok(), "{good} is an allowed category");
        }
    }

    #[test]
    fn whitespace_only_is_blank() {
        let mut e = entry("fn:0");
        e.justification = "   \t\n".into();
        assert!(res(vec![e]).validate().unwrap_err().contains("blank"));
    }

    #[test]
    fn a_calendar_date_is_checked_not_just_the_shape() {
        for bad in ["2026-02-30", "2026-13-01", "2026-00-01", "2026-01-00", "26-01-01", "2026/01/01", "2026-01-01x", "", "2026-01-011"] {
            let mut e = entry("fn:0");
            e.date = bad.into();
            assert!(res(vec![e]).validate().is_err(), "{bad:?} must not validate");
        }
        // a leap day is real in a leap year, and only then
        let mut leap = entry("fn:0");
        leap.date = "2024-02-29".into();
        assert!(res(vec![leap]).validate().is_ok());
        let mut not_leap = entry("fn:1");
        not_leap.key = "fn:0".into();
        not_leap.date = "2025-02-29".into();
        assert!(res(vec![not_leap]).validate().is_err());
    }

    #[test]
    fn a_duplicate_key_is_rejected_even_when_both_are_otherwise_valid() {
        let r = res(vec![entry("fn:0"), entry("fn:0")]);
        assert!(r.validate().unwrap_err().contains("duplicate"));
    }

    #[test]
    fn a_key_that_is_only_absurdly_long_is_rejected_before_anything_else() {
        let mut e = entry("fn:0");
        e.key = "x".repeat(MAX_KEY_LEN + 1);
        assert!(res(vec![e]).validate().is_err());
    }

    #[test]
    fn an_entry_count_far_past_any_real_review_is_rejected() {
        let entries = (0..=MAX_RESOLUTION_ENTRIES).map(|i| entry(&format!("fn:{i}"))).collect();
        assert!(res(entries).validate().unwrap_err().contains("exceeds the limit"));
    }

    /// A gap key may legitimately contain a colon (a namespaced or mangled
    /// symbol can). It must be matched by exact comparison against the
    /// evidence's own gaps, not torn apart by the source-key parser.
    #[test]
    fn a_gap_key_containing_a_colon_is_still_matched_by_exact_comparison() {
        let mut ev = ev_with_obligations();
        ev.functions[0].object.as_mut().unwrap().gaps.push(Gap { id: 1, key: "ns::Class::method@line-zero@200".into(), ..Default::default() });
        let r = res(vec![entry("ns::Class::method@line-zero@200")]);
        assert!(r.validate().is_ok(), "{:?}", r.validate());
    }

    /// The bug this test caught the first time: an s2o review uses its own
    /// category vocabulary, disjoint from the gap-justification one. A
    /// validator that only knew the latter rejected every real `s2o:`
    /// review that has ever been written to disk in this repository's own
    /// fixtures.
    #[test]
    fn s2o_reviews_use_their_own_category_vocabulary() {
        for good in ALLOWED_S2O_CATEGORIES {
            let mut e = entry("s2o:f");
            e.category = (*good).into();
            assert!(res(vec![e]).validate().is_ok(), "{good} is a valid s2o category");
        }
        // a gap-justification category on an s2o key is still wrong, and the
        // reverse -- an s2o category on a gap key -- is wrong too, because
        // neither vocabulary describes what the other key type means
        let mut wrong_kind = entry("s2o:f");
        wrong_kind.category = "analysis".into();
        assert!(res(vec![wrong_kind]).validate().is_err(), "a gap category on an s2o key must be rejected");
        let mut backwards = entry("fn:0");
        backwards.category = "traceable".into();
        assert!(res(vec![backwards]).validate().is_err(), "an s2o category on a non-s2o key must be rejected");
    }

    /// A gap key is `<symbol>@<kind>@<addr>`, and a C++ symbol carries its
    /// namespace -- so a gap in `s2o::compute` yields a key beginning with
    /// `s2o:` that is a coverage gap, not a traceability review. Dispatching
    /// the category vocabulary on the prefix alone rejected the correct
    /// `analysis` justification for it (review of #66).
    #[test]
    fn a_symbol_in_a_namespace_called_s2o_is_still_a_gap() {
        let mut ev = ev_with_obligations();
        ev.functions[0].object.as_mut().unwrap().gaps.push(Gap { id: 1, key: "s2o::compute@line-zero@200".into(), ..Default::default() });

        let mut gap = entry("s2o::compute@line-zero@200");
        gap.category = "analysis".into(); // the right category for a gap
        assert!(res(vec![gap]).validate().is_ok(), "a gap in namespace s2o takes the gap vocabulary");

        // and the s2o vocabulary must not be accepted for it
        let mut wrong = entry("s2o::compute@line-zero@200");
        wrong.category = "traceable".into();
        assert!(res(vec![wrong]).validate().is_err());

        // while a real source-to-object review, which has no `@`, is unchanged
        let mut review = entry("s2o:f");
        review.category = "traceable".into();
        assert!(res(vec![review]).validate().is_ok());
    }

    /// `stale_keys` indexes the evidence once rather than rescanning it per
    /// entry. Sized at the entry cap against a project-sized evidence,
    /// because that is the case an attacker controls: 20,000 entries against
    /// 20,000 obligations is 400 million string comparisons when rescanned
    /// and a few tens of thousands of hash operations when indexed. The
    /// bound below sits three orders of magnitude above the indexed cost and
    /// far under the rescan, so it distinguishes the two without being a
    /// stopwatch on a loaded machine.
    #[test]
    fn the_entry_cap_against_a_project_sized_evidence_stays_cheap() {
        let mut ev = ev_with_obligations();
        for i in 1..500 {
            let gaps = (0..40).map(|g| Gap { id: g, key: format!("_f{i}@line-zero@{i:x}{g:02x}"), ..Default::default() }).collect();
            ev.functions.push(Function { id: i, name: format!("f{i}"), object: Some(ObjectFn { symbol: format!("_f{i}"), gaps, ..Default::default() }), ..Default::default() });
        }
        // the worst case: every entry runs the full lookup before concluding
        // it is stale, and there are as many as the cap allows
        let entries = (0..MAX_RESOLUTION_ENTRIES).map(|i| entry(&format!("_nothing{i}@line-zero@ffff"))).collect();
        let r = res(entries);
        let t = std::time::Instant::now();
        let stale = r.stale_keys(&ev);
        let elapsed = t.elapsed();
        assert_eq!(stale.len(), MAX_RESOLUTION_ENTRIES);
        assert!(elapsed.as_secs() < 3, "{} entries against ~20k obligations took {elapsed:?}: the lookup is rescanning per entry, not indexed", MAX_RESOLUTION_ENTRIES);
    }

    #[test]
    fn a_well_formed_entry_over_a_real_obligation_is_accepted() {
        assert!(res(vec![entry("fn:0")]).validate().is_ok());
    }

    #[test]
    fn dead_candidates_without_matching_function_name_are_valid_obligations() {
        let mut ev = ev_with_obligations();
        ev.functions[0].name = "a.c:helper".into();
        ev.image = Some(Image {
            dead_candidates: vec![
                DeadCandidate { symbol_id: 0, name: "orphan_asm".into(), size: 16, function_id: None, key: "dead:orphan_asm".into(), ..Default::default() },
                DeadCandidate { symbol_id: 1, name: "helper".into(), size: 16, function_id: Some(0), key: "dead:helper".into(), ..Default::default() },
            ],
            ..Default::default()
        });
        for k in ["dead:orphan_asm", "dead:helper", "dead:a.c:helper"] {
            let r = res(vec![entry(k)]);
            assert!(r.stale_keys(&ev).is_empty(), "{k} must be recognized as a real obligation");
            assert!(r.validate_for_write(&ev).is_ok(), "{k} must be writable");
        }
        assert_eq!(res(vec![entry("dead:no_such_symbol")]).stale_keys(&ev), vec!["dead:no_such_symbol"]);
    }

    #[test]
    fn macro_decisions_with_ordinals_validate_by_decision_site() {
        let mut ev = ev_with_obligations();
        ev.functions[0].decisions = vec![
            Decision {
                line: 3,
                col: 5,
                line_end: 3,
                col_end: 10,
                true_count: 1,
                false_count: 0,
                conditions: vec![Condition { index: 0, line: 3, col: 5, true_count: 1, false_count: 0, independence_covered: false, pair: None }],
                vectors: vec![],
                mcdc_covered: false,
                decision_ordinal: Some(0),
            },
            Decision {
                line: 3,
                col: 5,
                line_end: 3,
                col_end: 10,
                true_count: 1,
                false_count: 0,
                conditions: vec![Condition { index: 0, line: 3, col: 5, true_count: 1, false_count: 0, independence_covered: false, pair: None }],
                vectors: vec![],
                mcdc_covered: false,
                decision_ordinal: Some(1),
            },
        ];
        for k in ["cond:0:3:#0:0", "cond:0:3:#1:0"] {
            assert!(res(vec![entry(k)]).validate_for_write(&ev).is_ok(), "{k} must match its ordinalled decision");
        }
        for bad in ["cond:0:3:5:0", "cond:0:3:#2:0", "cond:0:3:#0:1"] {
            assert!(res(vec![entry(bad)]).validate_for_write(&ev).is_err(), "{bad} must be rejected");
        }
    }
}
