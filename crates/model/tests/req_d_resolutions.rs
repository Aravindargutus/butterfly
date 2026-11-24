// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests, area D: resolutions and the definition index.
//!
//! Expected values come from docs/butterfly.html "Resolving a gap"
//! (docs/butterfly.html:504-516), support/features.toml:391-392 and README.md
//! "Go to definition", not from the implementation.

use butterfly_model::tags::{Tag, Tags, TAGS_SCHEMA};
use butterfly_model::*;

fn entry(key: &str, category: &str) -> Resolution {
    Resolution { key: key.into(), category: category.into(), justification: "requirements-based test TC-12 covers it".into(), author: "reviewer".into(), date: "2026-03-14".into() }
}

fn sidecar(entries: Vec<Resolution>) -> Resolutions {
    Resolutions { schema: RESOLUTIONS_SCHEMA.into(), entries }
}

/// docs/butterfly.html:506-512 -- a resolution "records a category, a
/// justification, an author and a date"; the categories are a, b, c, d1, d2
/// and analysis (§6.4.4.3, §6.4.4.2.b). Any other category is refused
/// (support/features.toml:391: "validated (known key, category, non-empty
/// justification)").
#[test]
fn r01_the_six_gap_categories_are_accepted_and_nothing_else() {
    for c in ["a", "b", "c", "d1", "d2", "analysis"] {
        assert!(entry("f@line-zero@1000", c).validate_fields().is_ok(), "category {c}");
    }
    for c in ["", "d", "A", "d3", "waived", "traceable"] {
        assert!(entry("f@line-zero@1000", c).validate_fields().is_err(), "category {c:?} is not a DO-178C one");
    }
}

/// docs/butterfly.html:516 -- "each function carries a source-to-object
/// review record — traceable, analysed or not-traceable". All three verdicts
/// are records a reviewer can make; the viewer offers all three
/// (ui/app.js `S2O`).
#[test]
fn r02_every_documented_source_to_object_verdict_is_accepted() {
    for c in ["traceable", "analysed", "not-traceable"] {
        assert!(entry("s2o:checksum", c).validate_fields().is_ok(), "s2o verdict {c}: {:?}", entry("s2o:checksum", c).validate_fields());
    }
}

/// docs/butterfly.html:516 -- review verdicts are accepted on a review key; and a gap
/// category is not a review verdict.
#[test]
fn r03_review_verdicts_and_gap_categories_are_different_vocabularies() {
    assert!(entry("s2o:checksum", "traceable").validate_fields().is_ok());
    assert!(entry("s2o:checksum", "analysed").validate_fields().is_ok());
    assert!(entry("s2o:checksum", "a").validate_fields().is_err());
    // a gap in a C++ namespace called s2o is a gap, not a review
    assert!(entry("s2o::compute@line-zero@200", "analysis").validate_fields().is_ok());
}

/// support/features.toml:391 -- "non-empty justification"; a resolution
/// records an author and a date (docs/butterfly.html:506). Blank human fields
/// and an empty key are refused.
#[test]
fn r04_blank_human_fields_are_refused() {
    let mut e = entry("f@line-zero@1000", "a");
    e.justification = "   \n\t".into();
    assert!(e.validate_fields().unwrap_err().contains("justification"));
    let mut e = entry("f@line-zero@1000", "a");
    e.author = " ".into();
    assert!(e.validate_fields().unwrap_err().contains("author"));
    let e = entry("", "a");
    assert!(e.validate_fields().unwrap_err().contains("key"));
}

/// docs/butterfly.html:506 -- the date is a date: a real calendar day in
/// YYYY-MM-DD, leap years included.
#[test]
fn r05_the_date_is_a_real_calendar_date() {
    let ok = ["2026-01-31", "2024-02-29", "2000-02-29", "2026-12-31", "2026-04-30"];
    let bad = [
        "2026-02-30",
        "2025-02-29",
        "1900-02-29",
        "2026-13-01",
        "2026-00-10",
        "2026-01-00",
        "2026-04-31",
        "2026-1-01",
        "26-01-01",
        "2026/01/01",
        "2026-0a-15",
        "yesterday",
        "",
        "2026-01-01T00:00",
    ];
    for d in ok {
        let mut e = entry("f@line-zero@1000", "a");
        e.date = d.into();
        assert!(e.validate_fields().is_ok(), "{d} is a real date");
    }
    for d in bad {
        let mut e = entry("f@line-zero@1000", "a");
        e.date = d.into();
        assert!(e.validate_fields().is_err(), "{d:?} is not a real YYYY-MM-DD date");
    }
}

/// A sidecar is one decision per finding: two entries resolving the same key
/// are refused, as is a sidecar of another schema.
#[test]
fn r06_duplicate_keys_and_a_foreign_schema_are_refused() {
    let ok = sidecar(vec![entry("f@line-zero@1000", "a"), entry("s2o:f", "analysed")]);
    assert!(ok.validate().is_ok());
    let dup = sidecar(vec![entry("f@line-zero@1000", "a"), entry("f@line-zero@1000", "b")]);
    assert!(dup.validate().unwrap_err().contains("duplicate"));
    let foreign = Resolutions { schema: "butterfly-resolutions/0".into(), entries: vec![] };
    assert!(foreign.validate().is_err());
    // an invalid entry is named by its position and key
    let bad = sidecar(vec![entry("f@line-zero@1000", "a"), entry("g@line-zero@2000", "zzz")]);
    let err = bad.validate().unwrap_err();
    assert!(err.contains("entries[1]") && err.contains("g@line-zero@2000"), "{err}");
}

/// Bounded human input: a field past its limit is refused rather than
/// carried (a sidecar is read by people and diffs).
#[test]
fn r07_oversized_fields_are_refused() {
    let mut e = entry("f@line-zero@1000", "a");
    e.justification = "x".repeat(100_000);
    assert!(e.validate_fields().unwrap_err().contains("justification"));
    let mut e = entry("f@line-zero@1000", "a");
    e.author = "x".repeat(10_000);
    assert!(e.validate_fields().unwrap_err().contains("author"));
    let e = entry(&"k".repeat(10_000), "a");
    assert!(e.validate_fields().unwrap_err().contains("key"));
}

/// support/features.toml:391 -- "validated (known key, ...)": a new record
/// must name an obligation the evidence has.
#[test]
fn r08_a_written_record_must_name_a_known_obligation() {
    let mut ev = Evidence { schema: SCHEMA.into(), ..Default::default() };
    ev.files.push(SourceFile {
        id: 0,
        path: "a.c".into(),
        display: "a.c".into(),
        text: vec!["x".into(); 3],
        lines: vec![LineCov { count: Some(1) }, LineCov { count: None }, LineCov { count: Some(0) }],
        function_ids: vec![0],
        ..Default::default()
    });
    ev.functions.push(Function { id: 0, name: "f".into(), line_start: 1, line_end: 3, ..Default::default() });
    assert!(sidecar(vec![entry("s2o:f", "traceable")]).validate_for_write(&ev).is_ok());
    assert!(sidecar(vec![entry("line:0:3", "a")]).validate_for_write(&ev).is_ok(), "an instrumented line");
    assert!(sidecar(vec![entry("fn:0", "a")]).validate_for_write(&ev).is_ok());
    for k in ["s2o:nosuch", "line:0:2", "line:0:0", "line:5:1", "fn:7", "dead:nosuch", "nokind"] {
        let err = sidecar(vec![entry(k, if k.starts_with("s2o") { "traceable" } else { "a" })]).validate_for_write(&ev).unwrap_err();
        assert!(err.contains(k), "{k}: {err}");
    }
}

/// README.md:150-152 -- "A definition in a header the evidence never compiled
/// is marked as such": a tag with no file id is valid; an id past the
/// evidence's arrays, or a line 0, is not. A tags file of another schema is
/// refused with the command that rebuilds it.
#[test]
fn r09_a_definition_index_is_range_checked_against_its_evidence() {
    let t = |file_id, function_id, line| Tag { name: "x".into(), kind: "function".into(), file: "a.h".into(), line, file_id, function_id, ..Default::default() };
    let mut tags = Tags { schema: TAGS_SCHEMA.into(), entries: vec![t(None, None, 4), t(Some(0), Some(1), 2)], ..Default::default() };
    assert!(tags.check_schema().is_ok());
    assert!(tags.validate(1, 2).is_ok());
    tags.entries.push(t(Some(1), None, 1));
    assert!(tags.validate(1, 2).is_err(), "file id past the evidence");
    tags.entries.pop();
    tags.entries.push(t(Some(0), Some(2), 1));
    assert!(tags.validate(1, 2).is_err(), "function id past the evidence");
    tags.entries.pop();
    tags.entries.push(t(None, None, 0));
    assert!(tags.validate(1, 2).is_err(), "lines are 1-based");
    let old = Tags { schema: "butterfly-tags/0".into(), ..Default::default() };
    assert!(old.check_schema().unwrap_err().contains("butterfly tags"));
}
