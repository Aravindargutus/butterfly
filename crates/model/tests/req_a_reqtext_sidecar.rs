// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the requirement-text sidecar as input
//! (area A). The sidecar rides beside the evidence and is read by `serve`,
//! `report` and `claims`, so it is input like any other (README.md:1580-1590,
//! "Evidence files are validated on load ... a foreign `schema` is rejected:
//! a truncated or hostile evidence file produces a named error").

use butterfly_model::reqtext::{Requirement, Requirements, Source, REQTEXT_SCHEMA};

fn doc(ids: &[&str]) -> Requirements {
    Requirements {
        schema: REQTEXT_SCHEMA.into(),
        sources: vec![Source { path: "llr.yaml".into(), sha256: "0".repeat(64), count: ids.len() }],
        entries: ids.iter().map(|id| Requirement { id: id.to_string(), text: format!("{id} says"), ..Default::default() }).collect(),
        ..Default::default()
    }
}

/// README.md:1612-1615 "a foreign `schema` is rejected" -- the sidecar has a
/// schema of its own, and a foreign one is refused by name.
#[test]
fn a_foreign_schema_is_refused() {
    let mut r = doc(&["LLR-1"]);
    r.schema = "butterfly-requirements/999".into();
    let e = r.validate().expect_err("foreign schema");
    assert!(e.contains("butterfly-requirements/999"), "{e}");
    assert!(doc(&["LLR-1"]).validate().is_ok(), "and the real one is accepted");
}

/// README.md:1612-1614 "Every index ... is range-checked before anything
/// renders" -- an entry's `source` is an index into `sources`.
#[test]
fn an_entry_pointing_past_the_sources_is_refused() {
    let mut r = doc(&["LLR-1"]);
    r.entries[0].source = 1;
    let e = r.validate().expect_err("out of range");
    assert!(e.contains("LLR-1"), "names the entry: {e}");
}

/// A hostile sidecar must not make the viewer show the wrong statement: an
/// entry with no ID, or two for one ID, or entries out of order (the
/// lookup is by ID) are refused rather than answered from.
#[test]
fn a_sidecar_whose_lookups_would_be_ambiguous_is_refused() {
    assert!(doc(&[""]).validate().is_err(), "no ID");
    assert!(doc(&["LLR-1", "LLR-1"]).validate().is_err(), "one ID twice");
    assert!(doc(&["LLR-2", "LLR-1"]).validate().is_err(), "out of order");
    let ok = doc(&["LLR-1", "LLR-2", "LLR-3"]);
    ok.validate().unwrap();
    for id in ["LLR-1", "LLR-2", "LLR-3"] {
        assert_eq!(ok.get(id).map(|q| q.text.as_str()), Some(format!("{id} says").as_str()));
    }
    assert!(ok.get("LLR-4").is_none());
}

/// README.md:1612-1615 "a truncated or hostile ... file produces a named
/// error, never a panic" -- a truncated sidecar fails to parse.
#[test]
fn a_truncated_sidecar_does_not_parse() {
    let full = serde_json::to_string(&doc(&["LLR-1"])).unwrap();
    assert!(serde_json::from_str::<Requirements>(&full).is_ok());
    assert!(serde_json::from_str::<Requirements>(&full[..full.len() / 2]).is_err());
}

/// A source digest is a SHA-256: 64 lowercase hex characters. Anything else
/// is refused by name, so no consumer has to slice a string that is not one
/// (#338).
#[test]
fn a_source_digest_that_is_not_sha256_hex_is_refused() {
    let short = format!("{}é", "a".repeat(11));
    for bad in [String::new(), "0".repeat(63), "0".repeat(65), "A".repeat(64), "g".repeat(64), format!("{short}{}", "0".repeat(51))] {
        let mut r = doc(&["LLR-1"]);
        r.sources[0].sha256 = bad.clone();
        let e = r.validate().expect_err(&bad);
        assert!(e.contains("sources[0]") && e.contains("sha256"), "names the field: {e}");
    }
    let mut r = doc(&["LLR-1"]);
    r.sources[0].sha256 = "0123456789abcdef".repeat(4);
    assert_eq!(r.validate(), Ok(()), "a real digest is accepted");
}
