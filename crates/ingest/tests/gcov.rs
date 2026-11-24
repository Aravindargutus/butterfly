// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The gcov front end (#45).
//!
//! The fixtures here are shaped exactly as GCC 16 emits them -- the branch
//! records in particular were taken from real output, because the thing that
//! matters most about them is not obvious from the schema.

use butterfly_ingest::gcov;

/// Two entries sharing a `source_block_id` are the two outcomes of one
/// decision.
const IF_ONLY: &str = r#"{
  "format_version": "2", "gcc_version": "16.2.0", "current_working_directory": "/w",
  "files": [{
    "file": "a.c",
    "functions": [{"name":"f","start_line":1,"start_column":5,"end_line":4,"end_column":1,"execution_count":2}],
    "lines": [
      {"line_number":1,"count":2},
      {"line_number":2,"count":2,"branches":[
        {"count":2,"throw":false,"fallthrough":true,"source_block_id":2,"destination_block_id":3},
        {"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4}]},
      {"line_number":3,"count":0}
    ]
  }]
}"#;

#[test]
fn it_recognises_a_gcov_export_and_not_an_llvm_one() {
    assert!(gcov::looks_like(IF_ONLY));
    assert!(!gcov::looks_like(r#"{"type":"llvm.coverage.json.export","data":[]}"#));
    // both keys are required: either alone is not distinctive enough
    assert!(!gcov::looks_like(r#"{"gcc_version":"16"}"#));
}

/// The heart of it. gcov's `fallthrough` names a CFG edge, and which edge
/// carries the true outcome depends on the construct -- for `if` it is the
/// fall-through, for `while`/`for` it is the loop *exit*. Reading it as
/// polarity reports every loop in a GCC project backwards, and because the
/// direction becomes part of a resolution key, a reviewer's justification
/// would be filed against the outcome they did not justify.
///
/// So no branch from this reader may claim to know its direction.
#[test]
fn no_gcov_branch_claims_to_know_which_outcome_is_true() {
    let e = gcov::parse(IF_ONLY).expect("parses");
    let branches: Vec<_> = e.functions.iter().flat_map(|f| f.branches.iter()).collect();
    assert_eq!(branches.len(), 1, "one decision");
    assert!(!branches[0].direction_known, "gcov cannot say which outcome is true");
    // and both counts survive, because decision coverage needs them
    let (a, b) = (branches[0].true_count, branches[0].false_count);
    assert_eq!(a.max(b), 2);
    assert_eq!(a.min(b), 0);
}

/// Decision coverage is direction-independent, which is why it is still
/// exact: "were both outcomes taken" reads the same either way round.
#[test]
fn both_outcomes_are_kept_so_decision_coverage_is_exact() {
    let e = gcov::parse(IF_ONLY).expect("parses");
    let b = e.functions[0].branches[0];
    let covered = (b.true_count > 0) as usize + (b.false_count > 0) as usize;
    assert_eq!(covered, 1, "one of the two outcomes was taken");
}

/// An exception edge is not an outcome of the source's decision.
#[test]
fn a_throw_edge_is_not_counted_as_a_decision_outcome() {
    let with_throw = IF_ONLY.replace(
        r#"{"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4}"#,
        r#"{"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4},
           {"count":1,"throw":true,"fallthrough":false,"source_block_id":2,"destination_block_id":9}"#,
    );
    let e = gcov::parse(&with_throw).expect("parses");
    assert_eq!(e.functions[0].branches.len(), 1, "still one decision, not one and a half");
}

/// A block with other than two edges is not a two-way decision, and forcing
/// it into one would invent a branch the source does not have.
#[test]
fn a_block_without_exactly_two_edges_is_left_alone() {
    let three = IF_ONLY.replace(
        r#"{"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4}"#,
        r#"{"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4},
           {"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":5}"#,
    );
    let e = gcov::parse(&three).expect("parses");
    assert!(e.functions[0].branches.is_empty(), "a three-way block is not reported as a decision");
}

/// Line counts are taken as stated, and a line the export never mentions is
/// `None` -- no code was generated for it -- rather than zero, which would
/// read as code that never ran.
#[test]
fn line_counts_are_taken_as_stated_and_absence_is_not_zero() {
    let e = gcov::parse(IF_ONLY).expect("parses");
    let counts = butterfly_ingest::coverage::line_counts(&e.files[0], 5);
    assert_eq!(counts[0], Some(2), "line 1 ran twice");
    assert_eq!(counts[2], Some(0), "line 3 is code that never ran");
    assert_eq!(counts[3], None, "line 4 has no code, which is not the same as zero");
    assert_eq!(counts[4], None);
}

/// gcov names files relative to the directory it ran in and states that
/// directory; everything downstream expects a path it can open.
#[test]
fn relative_filenames_are_resolved_against_the_stated_directory() {
    let e = gcov::parse(IF_ONLY).expect("parses");
    assert_eq!(e.files[0].filename, "/w/a.c");
}

/// The limitation travels with the evidence rather than living only in the
/// documentation: GCC records nothing MC/DC can be built from.
#[test]
fn the_absence_of_mcdc_is_stated_in_the_export_caveats() {
    let e = gcov::parse(IF_ONLY).expect("parses");
    assert!(e.functions.iter().all(|f| f.mcdc.is_empty()), "no MC/DC is invented");
    let joined = e.caveats.join(" | ");
    assert!(joined.contains("MC/DC"), "{joined}");
    assert!(joined.contains("objective 5"), "{joined}");
    assert!(joined.to_lowercase().contains("polarity") || joined.contains("does not name"), "the direction limit is stated too: {joined}");
}

/// Without `gcov -b` there are no branch records at all, and evidence that
/// quietly contained no decision coverage would be worse than one that says
/// why.
#[test]
fn an_export_without_branches_says_so() {
    let no_branches = IF_ONLY.replace(
        r#","branches":[
        {"count":2,"throw":false,"fallthrough":true,"source_block_id":2,"destination_block_id":3},
        {"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4}]"#,
        "",
    );
    let e = gcov::parse(&no_branches).expect("parses");
    assert!(e.caveats.iter().any(|c| c.contains("-b")), "{:?}", e.caveats);
}

/// A future format version is announced, not parsed hopefully.
#[test]
fn an_unrecognised_format_version_is_reported() {
    let v9 = IF_ONLY.replace(r#""format_version": "2""#, r#""format_version": "9""#);
    let e = gcov::parse(&v9).expect("still parses what it can");
    assert!(e.caveats.iter().any(|c| c.contains("format_version 9")), "{:?}", e.caveats);
}

/// Every function must resolve to the file it is actually in.
///
/// A `CovFunction`'s `file_id` indexes that function's own `filenames`, not
/// the export's file list. Using the global index worked for the first file
/// and emptied the name of every function after it -- and a function whose
/// filename is empty is dropped from the evidence without a word, so a GCC
/// project of more than one source file silently lost most of itself
/// (self-review of #45).
#[test]
fn a_function_in_the_second_file_still_knows_which_file_it_is_in() {
    const TWO: &str = r#"{
      "format_version":"2","gcc_version":"16.2.0","current_working_directory":"/w",
      "files":[
        {"file":"a.c",
         "functions":[{"name":"in_a","start_line":1,"start_column":1,"end_line":3,"end_column":1,"execution_count":1}],
         "lines":[{"line_number":1,"count":1},
                  {"line_number":2,"count":1,"branches":[
                    {"count":1,"throw":false,"fallthrough":true,"source_block_id":2,"destination_block_id":3},
                    {"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4}]}]},
        {"file":"b.c",
         "functions":[{"name":"in_b","start_line":1,"start_column":1,"end_line":3,"end_column":1,"execution_count":7}],
         "lines":[{"line_number":1,"count":7},
                  {"line_number":2,"count":7,"branches":[
                    {"count":4,"throw":false,"fallthrough":true,"source_block_id":2,"destination_block_id":3},
                    {"count":3,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4}]}]}
      ]}"#;

    let e = gcov::parse(TWO).expect("parses");
    assert_eq!(e.files.len(), 2);
    for (name, want) in [("in_a", "/w/a.c"), ("in_b", "/w/b.c")] {
        let f = e.functions.iter().find(|f| f.name == name).expect(name);
        let body = f.regions.iter().find(|r| r.kind == butterfly_ingest::coverage::KIND_CODE).expect("a body region");
        // exactly how the consumer resolves it
        let resolved = f.filenames.get(body.file_id).cloned().unwrap_or_default();
        assert_eq!(resolved, want, "{name} must resolve to its own file, not an empty string");
        // and its branches must survive the same-file filter the consumer applies
        let kept = f.branches.iter().filter(|b| b.file_id == body.file_id).count();
        assert_eq!(kept, 1, "{name}'s decision must not be filtered away");
    }
    // the second file's counts are its own, not the first's
    let b = e.functions.iter().find(|f| f.name == "in_b").unwrap();
    assert_eq!(b.count, 7);
    assert_eq!([b.branches[0].true_count, b.branches[0].false_count], [4, 3]);
}

/// `gcov --json-format --stdout a.gcda b.gcda` writes one document per data
/// file, newline-separated, not one merged document. Reading only the first
/// would drop every translation unit after it -- and a project is more than
/// one file (self-review of #45).
#[test]
fn a_multi_document_export_is_read_whole() {
    let a = r#"{"format_version":"2","gcc_version":"16","current_working_directory":"/w","files":[{"file":"a.c","functions":[{"name":"in_a","start_line":1,"end_line":2,"execution_count":1}],"lines":[{"line_number":1,"count":1}]}]}"#;
    let b = r#"{"format_version":"2","gcc_version":"16","current_working_directory":"/w","files":[{"file":"b.c","functions":[{"name":"in_b","start_line":1,"end_line":2,"execution_count":5}],"lines":[{"line_number":1,"count":5}]}]}"#;
    let e = gcov::parse(&format!("{a}\n{b}\n")).expect("parses both documents");

    assert_eq!(e.files.len(), 2, "both translation units are present");
    let names: Vec<&str> = e.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"in_a") && names.contains(&"in_b"), "{names:?}");

    // and each still resolves to its own file
    for (name, want) in [("in_a", "/w/a.c"), ("in_b", "/w/b.c")] {
        let f = e.functions.iter().find(|f| f.name == name).unwrap();
        let body = f.regions.iter().find(|r| r.kind == butterfly_ingest::coverage::KIND_CODE).unwrap();
        assert_eq!(f.filenames.get(body.file_id).cloned().unwrap_or_default(), want);
    }
    // the shared caveats are stated once, not once per document
    let mcdc: Vec<_> = e.caveats.iter().filter(|c| c.contains("MC/DC")).collect();
    assert_eq!(mcdc.len(), 1, "{:?}", e.caveats);
}

/// A header with an inline or `static` function is compiled into every unit
/// that includes it, and gcov reports it once per unit -- each copy holding
/// only what *that* unit ran. Left as separate entries the consumer keeps the
/// last one, so a function attributed to an earlier copy reads as uncovered
/// while its own execution count says otherwise. Ordinary C, not malformed
/// input (review of #45).
#[test]
fn a_header_seen_by_two_units_is_one_file_with_the_counts_summed() {
    let a = r#"{"format_version":"2","gcc_version":"16","current_working_directory":"/w",
      "files":[{"file":"shared.h",
        "functions":[{"name":"f","start_line":1,"end_line":3,"execution_count":2}],
        "lines":[{"line_number":1,"count":2},{"line_number":2,"count":2},{"line_number":3,"count":0}]}]}"#;
    let b = r#"{"format_version":"2","gcc_version":"16","current_working_directory":"/w",
      "files":[{"file":"shared.h",
        "functions":[{"name":"g","start_line":5,"end_line":6,"execution_count":0}],
        "lines":[{"line_number":1,"count":0},{"line_number":2,"count":0},{"line_number":5,"count":0}]}]}"#;

    for (order, text) in [("a then b", format!("{a}\n{b}")), ("b then a", format!("{b}\n{a}"))] {
        let e = gcov::parse(&text).unwrap_or_else(|_| panic!("{order}"));
        assert_eq!(e.files.len(), 1, "{order}: one entry per source path");
        let lines = e.files[0].direct_lines.as_ref().expect("counts");
        // summed across units: the line ran if any unit ran it
        assert_eq!(lines.get(&1), Some(&2), "{order}: the covered unit's count survives");
        assert_eq!(lines.get(&2), Some(&2), "{order}");
        assert_eq!(lines.get(&3), Some(&0), "{order}: a line no unit ran is zero, not absent");
        assert_eq!(lines.get(&4), None, "{order}: a line nobody mentioned has no code");
        // both functions are still here, each pointing at the one file
        let names: Vec<&str> = e.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"f") && names.contains(&"g"), "{order}: {names:?}");
    }
}

/// gcov gives no columns, so two decisions on `return a && b;` are both
/// "line 1". Keyed on line and column alone they collapse into one, and
/// justifying one untaken condition silently justifies the other -- a review
/// nobody performed. The ordinal keeps them apart.
#[test]
fn two_decisions_on_one_line_get_distinct_identities() {
    const SHORT_CIRCUIT: &str = r#"{"format_version":"2","gcc_version":"16","current_working_directory":"/w",
      "files":[{"file":"a.c",
        "functions":[{"name":"f","start_line":1,"end_line":1,"execution_count":2}],
        "lines":[{"line_number":1,"count":2,"branches":[
          {"count":1,"throw":false,"fallthrough":true,"source_block_id":2,"destination_block_id":3},
          {"count":1,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":5},
          {"count":1,"throw":false,"fallthrough":true,"source_block_id":3,"destination_block_id":4},
          {"count":0,"throw":false,"fallthrough":false,"source_block_id":3,"destination_block_id":5}]}]}]}"#;

    let e = gcov::parse(SHORT_CIRCUIT).expect("parses");
    let bs = &e.functions[0].branches;
    assert_eq!(bs.len(), 2, "two decisions on the line");

    let ords: Vec<Option<u32>> = bs.iter().map(|b| b.decision_ordinal).collect();
    assert_eq!(ords, vec![Some(0), Some(1)], "each is identified separately");
    // the identity really is distinct once it reaches the model
    let sites: Vec<String> = bs
        .iter()
        .map(|b| {
            butterfly_model::Branch {
                line: b.line_start,
                col: b.col_start,
                line_end: b.line_end,
                col_end: b.col_end,
                true_count: b.true_count,
                false_count: b.false_count,
                direction_known: b.direction_known,
                decision_ordinal: b.decision_ordinal,
            }
            .site()
        })
        .collect();
    assert_eq!(sites, vec!["1:#0", "1:#1"], "a justification on one cannot close the other");
}

/// The same header function compiled into two translation units reaches the
/// instantiation merger, and that merger matched branches on line/column
/// alone. Every gcov decision on a line shares column 0, so the second unit's
/// counts were all added to the *first* decision: one decision came out
/// falsely fully covered and the other lost its counts entirely. Which one
/// depended on the order gcov happened to emit the documents, so the evidence
/// -- and the open obligations derived from it -- were not reproducible.
///
/// Both orders must agree, and each decision must keep its own counts
/// (review of #45).
#[test]
fn a_header_function_in_two_units_keeps_each_decision_separate() {
    // `return a && b;` on line 2: two decisions, both at column 0.
    // unit A reached neither; unit B took decision 0 one way and decision 1
    // the other. Summed per decision that is 0=(1,0) and 1=(0,1) -- and in
    // particular neither is (1,1), which would read as fully covered.
    let doc = |p: [u64; 4]| {
        format!(
            r#"{{"format_version":"2","gcc_version":"16","current_working_directory":"/w",
      "files":[{{"file":"shared.h",
        "functions":[{{"name":"f","start_line":2,"end_line":2,"execution_count":1}}],
        "lines":[{{"line_number":2,"count":1,"branches":[
          {{"count":{},"throw":false,"fallthrough":true,"source_block_id":2,"destination_block_id":3}},
          {{"count":{},"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":5}},
          {{"count":{},"throw":false,"fallthrough":true,"source_block_id":3,"destination_block_id":4}},
          {{"count":{},"throw":false,"fallthrough":false,"source_block_id":3,"destination_block_id":5}}]}}]}}]}}"#,
            p[0], p[1], p[2], p[3]
        )
    };
    let a = doc([0, 0, 0, 0]);
    let b = doc([1, 0, 0, 1]);

    for (order, text) in [("a then b", format!("{a}\n{b}")), ("b then a", format!("{b}\n{a}"))] {
        let e = gcov::parse(&text).unwrap_or_else(|_| panic!("{order}"));
        // Both documents describe the same function, so the reader hands the
        // merger two records for it -- this is the shape that used to break.
        let fs: Vec<_> = e.functions.iter().filter(|f| f.name == "f").collect();
        assert_eq!(fs.len(), 2, "{order}: two records reach the merger");
        for f in fs {
            let ords: Vec<Option<u32>> = f.branches.iter().map(|b| b.decision_ordinal).collect();
            assert_eq!(ords, vec![Some(0), Some(1)], "{order}: each record numbers its own decisions");
        }
    }
}

/// gcov writes one JSON document per .gcda, and what an export *has* is a
/// property of all of them together. The presence flags used to keep the
/// first document's values through the merge, so doctor's advice depended on
/// the order gcov happened to emit the units: a branchless unit first
/// suppressed the missing-`-b` finding the conditions-only unit had earned,
/// and reversing two units flipped a WEAKER into a WRONG (review of #45).
#[test]
fn presence_flags_survive_the_merge_whatever_the_document_order() {
    // one unit compiled branchless: no branches, no conditions
    let branchless = r#"{"format_version":"2","gcc_version":"16","current_working_directory":"/w",
      "files":[{"file":"a.c","functions":[{"name":"a","start_line":1,"end_line":2,"execution_count":1}],
        "lines":[{"line_number":1,"count":1}]}]}"#;
    // one unit whose decisions were recorded (-fcondition-coverage) but whose
    // branch outcomes were not: the shape of a forgotten -b
    let conditions_only = r#"{"format_version":"2","gcc_version":"16","current_working_directory":"/w",
      "files":[{"file":"b.c","functions":[{"name":"b","start_line":1,"end_line":2,"execution_count":1}],
        "lines":[{"line_number":1,"count":1,"conditions":[{"count":2,"covered":1,"not_covered_true":[0],"not_covered_false":[]}]}]}]}"#;
    // one unit with raw branch records that are not two-way decisions: a
    // three-edge multiway block, exactly as -b exports a switch
    let multiway = r#"{"format_version":"2","gcc_version":"16","current_working_directory":"/w",
      "files":[{"file":"c.c","functions":[{"name":"c","start_line":1,"end_line":2,"execution_count":1}],
        "lines":[{"line_number":1,"count":1,"branches":[
          {"count":1,"throw":false,"fallthrough":true,"source_block_id":2,"destination_block_id":3},
          {"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4},
          {"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":5}]}]}]}"#;

    let flags = |a: &str, b: &str| {
        let e = gcov::parse(&format!("{a}\n{b}")).expect("parses");
        (e.raw_branch_records, e.decision_sites)
    };
    // each pair must answer the same both ways round
    assert_eq!(flags(branchless, conditions_only), flags(conditions_only, branchless));
    assert_eq!(flags(branchless, conditions_only), (false, true), "a decision was recorded and no branch outcomes exist: the missing--b shape");
    assert_eq!(flags(conditions_only, multiway), flags(multiway, conditions_only));
    assert_eq!(flags(conditions_only, multiway), (true, true), "raw records exist, so this is not a missing flag");
    assert_eq!(flags(branchless, multiway), flags(multiway, branchless));
    assert_eq!(flags(branchless, multiway), (true, true), "a branchless neighbour does not erase the multiway unit's records");
}

/// The per-document "carries no branch records; re-run with -b" caveat must
/// not outlive a merge that proves it wrong: a caveat above an export that
/// has branch records would be read, believed, and acted on.
#[test]
fn the_no_branch_caveat_speaks_for_the_whole_export() {
    let branchless = r#"{"format_version":"2","gcc_version":"16","current_working_directory":"/w",
      "files":[{"file":"a.c","functions":[{"name":"a","start_line":1,"end_line":2,"execution_count":1}],
        "lines":[{"line_number":1,"count":1}]}]}"#;
    let with_branches = r#"{"format_version":"2","gcc_version":"16","current_working_directory":"/w",
      "files":[{"file":"b.c","functions":[{"name":"b","start_line":1,"end_line":2,"execution_count":1}],
        "lines":[{"line_number":1,"count":1,"branches":[
          {"count":1,"throw":false,"fallthrough":true,"source_block_id":2,"destination_block_id":3},
          {"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4}]}]}]}"#;

    for text in [format!("{branchless}\n{with_branches}"), format!("{with_branches}\n{branchless}")] {
        let e = gcov::parse(&text).expect("parses");
        assert!(!e.caveats.iter().any(|c| c.contains("carries no branch records")), "{:?}", e.caveats);
    }
    // and it stays when it is true of every document
    let e = gcov::parse(&format!("{branchless}\n{branchless}")).expect("parses");
    assert!(e.caveats.iter().any(|c| c.contains("carries no branch records")));
}
