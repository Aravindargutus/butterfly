// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Reading low-level requirement text out of CSV (#56).
//!
//! The same principles the YAML reader holds to, over a format that hides
//! its structure in punctuation: a statement may contain a comma, a quote or
//! a newline, and a reader that splits on commas is right until the first
//! requirement anybody writes.

use butterfly_ingest::reqtext::{read, Keys};
use butterfly_model::limits::Limits;
use std::path::PathBuf;

fn write(tag: &str, files: &[(&str, &str)]) -> (PathBuf, Vec<PathBuf>) {
    let d = butterfly_testsupport::scratch_dir(&format!("reqcsv-{tag}"));
    let mut ps = Vec::new();
    for (name, body) in files {
        let p = d.join(name);
        std::fs::write(&p, body).unwrap();
        ps.push(p);
    }
    (d, ps)
}

fn lim() -> Limits {
    Limits::from_env()
}

/// A derived requirement is a low-level requirement with no parent, not a
/// lesser kind (DO-178C 5.1.2.h). The format it arrives in does not change
/// that, and CSV is the format a requirements tool exports.
#[test]
fn a_derived_requirement_is_a_low_level_requirement() {
    let (d, ps) = write(
        "derived",
        &[(
            "llr.csv",
            "id,text,type,status,parent,code,tests\n\
             LLR-A-001,decomposed from a parent,low-level,implemented,HLR-A-001,,\n\
             LLR-A-002,a design decision nothing higher asked for,derived,implemented,,src/a.rs::f,tests/a.rs::t\n",
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 2, "both kinds are low-level requirements");
    assert!(r.skipped.is_empty(), "and nothing was skipped: {:?}", r.skipped);

    let derived = r.get("LLR-A-002").expect("the derived one");
    assert!(derived.is_derived());
    assert_eq!(derived.parent, None, "derived means no parent, and that is correct here");
    assert_eq!(derived.claim.code, ["src/a.rs::f"], "it still carries a code claim");
    assert_eq!(r.get("LLR-A-001").unwrap().parent.as_deref(), Some("HLR-A-001"));
    let _ = std::fs::remove_dir_all(d);
}

/// Whatever is not taken is counted and named, whatever the format. A reader
/// who expected 304 and got 245 must be able to see where the rest went.
#[test]
fn an_entry_of_an_unaccepted_kind_is_counted_not_dropped() {
    let (d, ps) = write("skip", &[("llr.csv", "id,text,type\nLLR-B-001,kept,low-level\nHLR-B-001,a high-level requirement that wandered in,high-level\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 1);
    assert_eq!(r.skipped.get("high-level"), Some(&1), "said so: {:?}", r.skipped);
    let _ = std::fs::remove_dir_all(d);
}

/// The whole reason this parser is hand-written rather than a `split(',')`.
/// A requirement statement contains commas; a quoted one may contain
/// newlines and quotes of its own, and every one of those is content.
#[test]
fn a_statement_may_hold_commas_quotes_and_newlines_of_its_own() {
    let (d, ps) = write(
        "quoting",
        &[(
            "llr.csv",
            "id,text,type\n\
             LLR-C-001,\"An altitude within the envelope shall be returned unchanged, and one outside it as the nearer bound.\",low-level\n\
             LLR-C-002,\"The value is \"\"limited\"\", never rejected.\",low-level\n\
             LLR-C-003,\"Two sentences.\nOn two lines.\",low-level\n",
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 3, "three records, not five: the newline and the commas are content");
    assert!(r.get("LLR-C-001").unwrap().text.ends_with("the nearer bound."), "the comma did not end the field");
    assert_eq!(r.get("LLR-C-002").unwrap().text, "The value is \"limited\", never rejected.", "a doubled quote is one quote");
    assert_eq!(r.get("LLR-C-003").unwrap().text, "Two sentences.\nOn two lines.", "and the newline is kept, not made into a record");
    let _ = std::fs::remove_dir_all(d);
}

/// A line number that counted records would be a line number that is wrong
/// the moment a statement wraps -- which is the moment anybody writes a real
/// one. It counts lines, including the ones inside quoted cells.
#[test]
fn the_line_recorded_is_the_line_the_row_begins_on() {
    let (d, ps) = write("line", &[("llr.csv", "id,text\nLLR-D-001,\"one\ntwo\nthree\"\nLLR-D-002,after it\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.get("LLR-D-001").unwrap().line, Some(2), "the first record begins on the line after the header");
    assert_eq!(r.get("LLR-D-002").unwrap().line, Some(5), "and the next one after the three lines the first spans");
    let _ = std::fs::remove_dir_all(d);
}

/// Files arrive off Windows and out of Excel. A reader that only reads the
/// ones written on the host it runs on is not a reader of CSV.
#[test]
fn crlf_endings_and_a_byte_order_mark_are_read_like_any_other_file() {
    let (d, ps) = write("dialect", &[("llr.csv", "\u{feff}id,text,type\r\nLLR-E-001,written on Windows,low-level\r\nLLR-E-002,and exported by Excel,low-level\r\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 2);
    // The mark left in place becomes part of the first column's name, and
    // the `id` column is then not found at all.
    assert_eq!(r.get("LLR-E-001").unwrap().text, "written on Windows", "no stray carriage return on the end");
    let _ = std::fs::remove_dir_all(d);
}

/// CSV has no lists, so a cell holding several claims needs a separator of
/// its own. It cannot be the comma, and it cannot be the colon a claim is
/// already split on.
#[test]
fn several_claims_in_one_cell_are_split_on_the_configured_separator() {
    let (d, ps) = write("claims", &[("llr.csv", "id,text,type,code,tests\nLLR-F-001,several,low-level,src/a.c::f;src/b.c::g,tests/t.c::t_one; tests/t.c::t_two\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let e = r.get("LLR-F-001").unwrap();
    assert_eq!(e.claim.code, ["src/a.c::f", "src/b.c::g"], "two claims, and the `::` in each survives");
    assert_eq!(e.claim.tests, ["tests/t.c::t_one", "tests/t.c::t_two"], "and the space after the separator is not part of the claim");

    let keys = Keys { csv_list_separator: "|".into(), ..Keys::default() };
    let (d2, ps2) = write("claims-sep", &[("llr.csv", "id,text,type,code\nLLR-F-002,another project,low-level,src/a.c::f|src/b.c::g\n")]);
    let r = read(&ps2, &keys, &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.get("LLR-F-002").unwrap().claim.code, ["src/a.c::f", "src/b.c::g"], "a project that needs another separator says so");
    let _ = std::fs::remove_dir_all(d);
    let _ = std::fs::remove_dir_all(d2);
}

/// The flat analogue of "no `requirements` list". A misspelled ID column
/// means every row is anonymous, and writing a zero-entry sidecar over that
/// would be the silent failure this reader was hardened against.
#[test]
fn a_document_with_no_id_column_is_refused_naming_what_it_has() {
    let (d, ps) = write("noidcol", &[("llr.csv", "ref,statement,type\nLLR-G-001,the column above is not called id,low-level\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("no `id` column"), "{e}");
    assert!(e.contains("ref") && e.contains("statement"), "and says what the header does name: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// An entry with no ID cannot be traced to, reported on, or matched against
/// a tag. For a line-oriented format the position is a line number, which is
/// what the operator's editor wants (review of #56).
#[test]
fn a_row_without_an_id_refuses_the_document_naming_the_line() {
    let (d, ps) = write("noid", &[("llr.csv", "id,text,type\nLLR-H-001,fine,low-level\n,this one was never given an ID,low-level\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("line 3"), "naming where: {e}");
    assert!(e.contains("llr.csv") && e.contains("`id`"), "and which file and field: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// An ID with no statement is a different thing: traceable, so kept -- but a
/// matrix built on requirements that say nothing cannot be reviewed, so it
/// is listed rather than passed over.
#[test]
fn a_requirement_with_no_statement_is_kept_and_listed() {
    let (d, ps) = write("notext", &[("llr.csv", "id,text,type\nLLR-I-001,has one,low-level\nLLR-I-002,,low-level\nLLR-I-003,\"   \",low-level\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 3, "all three are traceable and kept");
    assert_eq!(r.without_text, ["LLR-I-002", "LLR-I-003"], "and the silent two are named");
    assert_eq!(r.get("LLR-I-001").unwrap().text, "has one");
    let _ = std::fs::remove_dir_all(d);
}

/// The inline half is the point of splitting the record: the statement goes
/// beside the code, the analysis around it does not.
#[test]
fn the_statement_is_separated_from_the_analysis_around_it() {
    let (d, ps) =
        write("split", &[("llr.csv", "id,text,type,rationale,verification_criteria\nLLR-J-001,the statement,low-level,\"the reasoning, which is longer\",how it is shown\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let e = r.get("LLR-J-001").unwrap();
    assert_eq!(e.text, "the statement");
    assert_eq!(e.detail["rationale"], "the reasoning, which is longer");
    assert!(e.detail.contains_key("verification_criteria"));
    let _ = std::fs::remove_dir_all(d);
}

/// A header is written once and read for every row, so a column name nobody
/// recognises costs the whole column rather than one entry. Dropped like an
/// unknown YAML key, but said out loud.
#[test]
fn a_column_the_reader_cannot_place_is_dropped_and_reported() {
    let (d, ps) = write("unknown", &[("llr.csv", "id,text,type,owner,Rationale\nLLR-K-001,kept,low-level,someone,capitalised by the export\n")]);
    let mut notes = Vec::new();
    let r = read(&ps, &Keys::default(), &lim(), &mut notes).expect("reads: an unknown column is not a refusal");
    assert_eq!(r.entries.len(), 1);
    assert!(r.get("LLR-K-001").unwrap().detail.is_empty(), "`Rationale` is not `rationale`, and is not guessed at");
    assert_eq!(notes.len(), 1, "one note: {notes:?}");
    assert!(notes[0].contains("owner") && notes[0].contains("Rationale"), "naming both: {}", notes[0]);
    assert!(notes[0].contains("llr.csv"), "and the file: {}", notes[0]);
    let _ = std::fs::remove_dir_all(d);
}

/// A row wider than its header is data with nowhere to go, and the usual
/// cause is a quote opened and never closed above -- which means the rows
/// around it are wrong too. A row narrower than its header is just a
/// document that did not say everything, which it is allowed not to.
#[test]
fn a_row_wider_than_the_header_is_refused_and_a_narrower_one_is_not() {
    let (d, ps) = write("ragged", &[("llr.csv", "id,text,type\nLLR-L-001,fine,low-level\nLLR-L-002,too many,low-level,and one more\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("line 3"), "naming where: {e}");
    assert!(e.contains("4 cells") && e.contains("3 column"), "and both counts: {e}");

    let (d2, ps2) = write("short", &[("llr.csv", "id,text,type,status\nLLR-L-003,short but complete,low-level\n")]);
    let r = read(&ps2, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.get("LLR-L-003").unwrap().status, None, "the field the row stopped short of is simply absent");
    let _ = std::fs::remove_dir_all(d);
    let _ = std::fs::remove_dir_all(d2);
}

/// An unclosed quote swallows the rest of the file into one value. Refusing
/// beats reporting the one requirement that survived it.
#[test]
fn an_unclosed_quote_is_refused_rather_than_swallowing_the_file() {
    let (d, ps) = write("unclosed", &[("llr.csv", "id,text\nLLR-M-001,\"it starts here\nLLR-M-002,and everything below is inside it\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("line 2") && e.contains("never closed"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

/// `"he said "no""` is a quote that was not doubled. Read literally it joins
/// two fields into one and every column after it shifts -- silently.
#[test]
fn a_quote_inside_a_quoted_field_that_was_not_doubled_is_refused() {
    let (d, ps) = write("badquote", &[("llr.csv", "id,text,type\nLLR-N-001,\"the value is \"limited\"\",low-level\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("line 2") && e.contains("closing quote"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

/// Two documents claiming one ID is ambiguity a reader cannot resolve.
#[test]
fn one_id_defined_in_two_files_is_refused_naming_both() {
    let (d, ps) = write("dup", &[("a.csv", "id,text,type\nLLR-O-001,from one,low-level\n"), ("b.csv", "id,text,type\nLLR-O-001,from two,low-level\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("LLR-O-001") && e.contains("a.csv") && e.contains("b.csv"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

/// Each file digested on its own: requirements live one file per component,
/// and a single hash over the set could not say which document moved.
#[test]
fn every_source_file_carries_its_own_digest() {
    let (d, ps) = write("prov", &[("a.csv", "id,text,type\nLLR-P-001,a,low-level\n"), ("b.csv", "id,text,type\nLLR-P-002,b,low-level\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.sources.len(), 2);
    assert_ne!(r.sources[0].sha256, r.sources[1].sha256);
    assert!(r.sources.iter().all(|s| s.sha256.len() == 64), "a real SHA-256 each");
    assert_eq!(r.sources[0].count, 1);
    assert_eq!(r.entries.iter().filter(|e| e.source == 1).count(), 1, "and each entry knows its file");
    r.validate().expect("well formed");
    let _ = std::fs::remove_dir_all(d);
}

/// A header row with nothing under it is a document that says there are
/// none -- a statement, not a mistake, and the analogue of `requirements: []`.
#[test]
fn a_header_with_no_rows_under_it_is_allowed() {
    let (d, ps) = write("empty", &[("llr.csv", "id,text,type\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert!(r.entries.is_empty());
    let _ = std::fs::remove_dir_all(d);
}

/// Every project shapes its requirements file differently, so the key
/// mapping is configuration rather than an assumption -- and it is the
/// *same* configuration the YAML reader uses, not a second one to keep in
/// step (#56).
#[test]
fn a_projects_own_key_names_are_read_from_config() {
    let (d, ps) = write(
        "keys",
        &[(
            "llr.csv",
            "ref,statement,category,state,derives_from,impl,verifies,why\n\
             SRS-042,The widget shall retract within 200 ms of the command.,low-level,implemented,HLR-9,src/widget.c::retract,tests/widget.c::t_retract,Timing is the safety-relevant property here.\n",
        )],
    );

    // the defaults do not fit this document, and say so rather than reading nothing
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("no `id` column") && e.contains("ref"), "{e}");

    let keys = Keys {
        id: "ref".into(),
        text: "statement".into(),
        kind: "category".into(),
        status: "state".into(),
        parent: "derives_from".into(),
        code: "impl".into(),
        tests: "verifies".into(),
        detail: vec!["why".into()],
        ..Keys::default()
    };
    let mut notes = Vec::new();
    let r = read(&ps, &keys, &lim(), &mut notes).expect("reads with the project's own names");
    assert!(notes.is_empty(), "and every column is placed: {notes:?}");
    let e = r.get("SRS-042").expect("the entry");
    assert!(e.text.starts_with("The widget shall retract"));
    assert_eq!(e.status.as_deref(), Some("implemented"));
    assert_eq!(e.parent.as_deref(), Some("HLR-9"));
    assert_eq!(e.claim.code, ["src/widget.c::retract"]);
    assert_eq!(e.claim.tests, ["tests/widget.c::t_retract"]);
    assert!(e.detail.contains_key("why"));
    let _ = std::fs::remove_dir_all(d);
}

/// `keys.traceability` names a nesting level, and CSV has none. A column of
/// that name is therefore an unknown column, not a silently special one.
#[test]
fn the_keys_that_describe_nesting_have_no_meaning_here_and_say_so() {
    let (d, ps) = write("flat", &[("llr.csv", "id,text,type,traceability,requirements\nLLR-Q-001,flat,low-level,something,something else\n")]);
    let mut notes = Vec::new();
    let r = read(&ps, &Keys::default(), &lim(), &mut notes).expect("reads");
    assert_eq!(r.entries.len(), 1);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("traceability") && notes[0].contains("requirements"), "both are reported, not quietly honoured: {}", notes[0]);
    let _ = std::fs::remove_dir_all(d);
}
