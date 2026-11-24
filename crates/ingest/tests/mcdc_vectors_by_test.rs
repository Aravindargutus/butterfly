// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `pertest::attribute`'s public API, end to end: two per-test exports each
//! reproducing one vector of a merged MC/DC decision, and the same
//! accumulate-then-apply pattern the real ingest loop uses (#48).
//!
//! Built entirely by hand -- no toolchain involved -- because the point is
//! the matching and accumulation logic, not any particular compiler's
//! output. `pertest::tests::vectors_for` (in-crate) covers the matching
//! rules precisely; this exercises the real public functions --
//! `pertest::attribute` and `pertest::apply_vector_matches`, the same one
//! `ingest`'s per-test loop calls -- end to end, including the multi-test
//! accumulation a single call cannot exercise on its own.

use butterfly_ingest::pertest;
use butterfly_model::*;

/// One function `f` with one merged MC/DC decision at line 2, three already-
/// built vectors -- the union three tests would have produced -- and one
/// instrumented, executed line so line-attribution has something to check
/// against without tripping the contradiction guard.
fn evidence_with_decision() -> Evidence {
    let cond = |i| Condition { index: i, line: 2, col: 7, true_count: 1, false_count: 1, independence_covered: true, pair: None };
    let decision = Decision {
        line: 2,
        col: 7,
        line_end: 2,
        col_end: 15,
        true_count: 1,
        false_count: 2,
        conditions: vec![cond(0), cond(1)],
        vectors: vec![
            TestVector { values: vec![Some(false), None], result: false, executed: true, tests: Vec::new() }, // V0
            TestVector { values: vec![Some(true), Some(true)], result: true, executed: true, tests: Vec::new() }, // V1
            TestVector { values: vec![Some(true), Some(false)], result: false, executed: true, tests: Vec::new() }, // V2
        ],
        mcdc_covered: true,
        decision_ordinal: None,
    };
    let mut ev = Evidence { schema: SCHEMA.into(), ..Default::default() };
    ev.files.push(SourceFile {
        id: 0,
        path: "d.c".into(),
        display: "d.c".into(),
        text: vec!["".into(), "int f(int a, int b) {".into(), "  if (a && b) return 1;".into()],
        lines: vec![LineCov { count: None }, LineCov { count: Some(5) }, LineCov { count: Some(5) }],
        function_ids: vec![0],
        ..Default::default()
    });
    ev.functions.push(Function { id: 0, name: "f".into(), file_id: 0, line_start: 1, line_end: 3, count: 5, decisions: vec![decision], ..Default::default() });
    ev
}

/// A minimal coverage export for one test: `f` executed once, one covered
/// line, one MC/DC vector reproducing exactly one of the merged ones.
fn export_for(vector_json: &str) -> String {
    format!(
        r#"{{"version":"2.0.1","type":"llvm.coverage.json.export","data":[{{
          "files":[{{"filename":"d.c","segments":[[2,1,1,true,true,false]],"branches":[],"summary":{{}}}}],
          "functions":[{{"name":"f","count":1,"filenames":["d.c"],
            "regions":[[1,1,3,2,1,0,0,0]],"branches":[],
            "mcdc_records":[[2,7,2,15,1,1,0,0,5,[true,true],[{vector_json}]]]}}]}}]}}"#
    )
}

#[test]
fn two_tests_each_name_the_vector_they_produced() {
    let mut ev = evidence_with_decision();

    let (run0, m0) =
        pertest::attribute(&ev, 0, "t_short_circuit", std::path::Path::new("t0.json"), &export_for(r#"{"conditions":[false,null],"executed":true,"result":false}"#)).unwrap();
    pertest::apply_vector_matches(&mut ev, 0, &m0);
    ev.tests.push(run0);

    let (run1, m1) =
        pertest::attribute(&ev, 1, "t_both_true", std::path::Path::new("t1.json"), &export_for(r#"{"conditions":[true,true],"executed":true,"result":true}"#)).unwrap();
    pertest::apply_vector_matches(&mut ev, 1, &m1);
    ev.tests.push(run1);

    let v = &ev.functions[0].decisions[0].vectors;
    assert_eq!(v[0].tests, vec![0], "V0 (short-circuit) belongs to t_short_circuit alone");
    assert_eq!(v[1].tests, vec![1], "V1 (both true) belongs to t_both_true alone");
    assert!(v[2].tests.is_empty(), "V2 was reproduced by neither test: unattributed, not guessed");
}

/// Two different tests reproducing the *same* vector both get named on it,
/// sorted, and a second attribution of an already-recorded id is not a
/// duplicate -- exactly "either will do", stated as data.
#[test]
fn several_tests_sharing_one_vector_are_all_named_and_deduplicated() {
    let mut ev = evidence_with_decision();
    let same_vector = r#"{"conditions":[true,true],"executed":true,"result":true}"#;

    for (id, name) in [(2, "t_a"), (0, "t_b"), (1, "t_c")] {
        // ids applied out of order on purpose, so the assertion below is
        // proof of the sort, not an accident of insertion order
        let (run, m) = pertest::attribute(&ev, id, name, std::path::Path::new("t.json"), &export_for(same_vector)).unwrap();
        pertest::apply_vector_matches(&mut ev, id, &m);
        ev.tests.push(run);
    }
    // apply the first id again: must not create a duplicate entry
    pertest::apply_vector_matches(&mut ev, 0, &[pertest::VectorMatch { function: 0, decision: 0, vector: 1 }]);

    assert_eq!(ev.functions[0].decisions[0].vectors[1].tests, vec![0, 1, 2]);
}

/// A per-test export that executed a vector the merge does not record as
/// executed is not part of this merge, and `attribute` must refuse it whole
/// -- the same rule it already applies to line and function counts. Anything
/// less puts a test's name on an MC/DC proof it did not produce (#48).
#[test]
fn an_export_contradicting_the_merged_vectors_is_refused_whole() {
    let mut ev = evidence_with_decision();
    // the merge says V1 (both true) never ran
    ev.functions[0].decisions[0].vectors[1].executed = false;

    // ...but this export claims it did, alongside a vector that does match
    let text = export_with_vectors(&[
        r#"{"conditions":[false,null],"executed":true,"result":false}"#, // matches V0
        r#"{"conditions":[true,true],"executed":true,"result":true}"#,   // contradicts V1
    ]);
    let err = pertest::attribute(&ev, 0, "t_bad", std::path::Path::new("t.json"), &text).unwrap_err();
    assert!(err.contains("contradicts the merged coverage"), "{err}");
    assert!(err.contains("does not record as executed"), "the vector contradiction is named: {err}");

    // and nothing from it was staged onto the evidence
    assert!(ev.functions[0].decisions[0].vectors.iter().all(|v| v.tests.is_empty()), "a refused export leaves no attribution behind");
}

/// The honest half: an export whose vectors all agree is still accepted, so
/// the rule above rejects contradiction rather than merely rejecting.
#[test]
fn an_export_whose_vectors_agree_is_still_accepted() {
    let ev = evidence_with_decision();
    let text = export_with_vectors(&[r#"{"conditions":[false,null],"executed":true,"result":false}"#]);
    let (_, matches) = pertest::attribute(&ev, 0, "t_good", std::path::Path::new("t.json"), &text).unwrap();
    assert_eq!(matches.len(), 1, "{matches:?}");
}

/// Build an export carrying the given MC/DC vectors for the one decision.
fn export_with_vectors(vectors: &[&str]) -> String {
    format!(
        r#"{{"version":"2.0.1","type":"llvm.coverage.json.export","data":[{{
          "files":[{{"filename":"d.c","segments":[[2,1,1,true,true,false]],"branches":[],"summary":{{}}}}],
          "functions":[{{"name":"f","count":1,"filenames":["d.c"],
            "regions":[[1,1,3,2,1,0,0,0]],"branches":[],
            "mcdc_records":[[2,7,2,15,1,1,0,0,5,[true,true],[{}]]]}}]}}]}}"#,
        vectors.join(",")
    )
}
