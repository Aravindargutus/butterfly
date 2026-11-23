// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the requirement-text readers (area A).
//!
//! Every expected value here is taken from README.md, "Requirement text
//! (`requirements.json`)" through "YAML, carefully", and from the bounded-read
//! clause of "Security posture" -- not from what the readers happen to do. Each
//! test names the sentence it checks.

use butterfly_ingest::reqtext::{read, Keys};
use butterfly_model::limits::Limits;
use std::path::PathBuf;

fn write(tag: &str, files: &[(&str, &str)]) -> (PathBuf, Vec<PathBuf>) {
    let d = butterfly_testsupport::scratch_dir(&format!("req-a-text-{tag}"));
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

const GIB: u64 = 1 << 30;

// ---- YAML, carefully ----------------------------------------------------

/// README.md:817 "**Anchors and aliases are refused.**" -- an anchor alone,
/// with no alias using it, is still an anchor.
#[test]
fn an_anchor_with_no_alias_is_refused() {
    let y = "requirements:\n  - id: LLR-AA-001\n    text: &stmt the statement\n    type: low-level\n";
    let (d, ps) = write("anchor", &[("llr.yaml", y)]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("an anchor is refused");
    assert!(e.contains("llr.yaml"), "names the file: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:830-834 "Each fenced `yaml` block is walked as an event stream
/// before it is loaded, by the same prescan" -- so the anchor refusal of
/// README.md:817 holds inside a Markdown block too.
#[test]
fn an_anchor_inside_a_markdown_block_is_refused() {
    let md = "## LLR-AA-002\n\nThe statement.\n\n```yaml\ntype: &k low-level\nstatus: implemented\n```\n";
    let (d, ps) = write("mdanchor", &[("llr.md", md)]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("an anchor in a block is refused");
    assert!(e.contains("llr.md"), "names the file: {e}");
    let _ = std::fs::remove_dir_all(d);
}

// ---- what counts as a low-level requirement ---------------------------

/// README.md:600-603 "an entry with no `type` at all is taken at its word and
/// read as a low-level requirement, whatever its ID looks like" -- in each of
/// the three formats.
#[test]
fn an_untyped_entry_is_low_level_whatever_its_id_looks_like() {
    let (d, ps) = write(
        "untyped",
        &[("a.yaml", "requirements:\n  - id: HLR-UT-001\n    text: from yaml\n"), ("b.csv", "id,text\nHLR-UT-002,from csv\n"), ("c.md", "## HLR-UT-003\n\nfrom markdown\n")],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    for id in ["HLR-UT-001", "HLR-UT-002", "HLR-UT-003"] {
        assert!(r.get(id).is_some(), "{id} is kept as a low-level requirement");
    }
    assert!(r.skipped.is_empty(), "nothing was skipped: {:?}", r.skipped);
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:793 "`kinds` lists which values count as low-level" -- the
/// filter follows the configured list, and what it does not take is counted
/// (README.md:598-600 "counted and reported rather than dropped in silence").
#[test]
fn the_configured_kinds_decide_what_is_low_level() {
    let keys = Keys { kinds: vec!["llr".into()], ..Keys::default() };
    let y = "requirements:\n  - id: LLR-KD-001\n    text: one\n    type: llr\n  - id: LLR-KD-002\n    text: two\n    type: low-level\n";
    let (d, ps) = write("kinds", &[("llr.yaml", y)]);
    let r = read(&ps, &keys, &lim(), &mut Vec::new()).expect("reads");
    assert!(r.get("LLR-KD-001").is_some(), "the configured kind is taken");
    assert!(r.get("LLR-KD-002").is_none(), "a kind not in the list is not");
    assert_eq!(r.skipped.get("low-level"), Some(&1), "and it is counted: {:?}", r.skipped);
    let _ = std::fs::remove_dir_all(d);
}

// ---- the expected shape -------------------------------------------------

/// README.md:613-636: `text` is the statement; `status`, `type` and
/// `traceability.parent` are recorded; `traceability.code` and `.tests` are
/// claims; README.md:626-628 `rationale`, `safety` and
/// `verification_criteria` "are kept whole" -- `safety` is a mapping
/// (README.md:690-692), so its nested content must survive.
#[test]
fn every_field_of_the_documented_shape_lands_where_the_readme_says() {
    let y = "requirements:\n  - id: LLR-SH-001\n    text: >-\n      An altitude shall be\n      returned unchanged.\n    type: low-level\n    status: planned\n    traceability:\n      parent: HLR-SH-001\n      code:  [src/limits.c::clamp_alt]\n      tests: [src/tests.c::t_clamp, src/tests.c::t_ceiling]\n    rationale: >-\n      Kept out of the inline path.\n    safety:\n      related: true\n      failure_condition: minor\n    verification_criteria: by test\n";
    let (d, ps) = write("shape", &[("llr.yaml", y)]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let q = r.get("LLR-SH-001").expect("the entry");
    assert_eq!(q.text, "An altitude shall be returned unchanged.");
    assert_eq!(q.kind.as_deref(), Some("low-level"));
    assert_eq!(q.status.as_deref(), Some("planned"));
    assert_eq!(q.parent.as_deref(), Some("HLR-SH-001"));
    assert_eq!(q.claim.code, ["src/limits.c::clamp_alt"]);
    assert_eq!(q.claim.tests, ["src/tests.c::t_clamp", "src/tests.c::t_ceiling"]);
    assert!(q.detail.get("rationale").is_some_and(|v| v.contains("Kept out of the inline path")), "{:?}", q.detail);
    assert!(q.detail.get("verification_criteria").is_some_and(|v| v.contains("by test")), "{:?}", q.detail);
    let safety = q.detail.get("safety").unwrap_or_else(|| panic!("safety kept: {:?}", q.detail));
    assert!(safety.contains("failure_condition") && safety.contains("minor"), "kept whole, nesting included: {safety}");
    assert!(!q.text.contains("Kept out"), "and the statement carries none of it");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:586-587 "a file is taken as one document, read in the format
/// its extension names" -- the walk's own names include `.yml` and
/// `.markdown` (README.md:585-586), so those extensions name formats too.
#[test]
fn the_yml_and_markdown_extensions_name_their_formats() {
    let (d, ps) =
        write("ext", &[("component.yml", "requirements:\n  - id: LLR-EX-001\n    text: yml\n    type: low-level\n"), ("component.markdown", "## LLR-EX-002\n\nmarkdown\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("both read");
    assert_eq!(r.get("LLR-EX-001").map(|q| q.text.as_str()), Some("yml"));
    assert_eq!(r.get("LLR-EX-002").map(|q| q.text.as_str()), Some("markdown"));
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:651-653 "Each entry records the line of its `id`, as the CSV
/// and Markdown readers record theirs ... and an ID written twice in one
/// file is refused naming both lines" -- in a `.yaml` and a `.yml` file,
/// past comments and blank lines, and wherever in the entry `id` sits.
#[test]
fn a_yaml_entry_records_the_line_of_its_id() {
    let y = "# the limits component\n\nrequirements:\n  - id: LLR-YL-001\n    text: first\n\n  # an entry whose id is not its first key\n  - text: >-\n      second,\n      folded\n    type: low-level\n    id: LLR-YL-002\n  - {id: LLR-YL-003, text: flow}\n";
    let (d, ps) = write("yamlline", &[("llr.yaml", y), ("more.yml", "requirements:\n- id: LLR-YL-004\n  text: yml\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.get("LLR-YL-001").unwrap().line, Some(4));
    assert_eq!(r.get("LLR-YL-002").unwrap().line, Some(12), "the line of its `id`, not of the entry's first key");
    assert_eq!(r.get("LLR-YL-003").unwrap().line, Some(13), "a flow mapping is an entry like any other");
    assert_eq!(r.get("LLR-YL-004").unwrap().line, Some(2), "`.yml` is the same reader");
    let _ = std::fs::remove_dir_all(d);

    let (d, ps) = write("yamldup", &[("llr.yaml", "requirements:\n  - id: LLR-YL-005\n    text: one\n  - id: LLR-YL-005\n    text: two\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("an ID defined twice is refused");
    assert!(e.contains("line 2") && e.contains("line 4"), "naming both lines: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:651 the line is of the configured `id` key, in a document whose
/// list is not called `requirements` -- and in every document of a stream.
#[test]
fn the_line_follows_the_configured_keys_and_every_document() {
    let keys = Keys { list: "llrs".into(), id: "ref".into(), ..Keys::default() };
    let y = "llrs:\n  - ref: LLR-YK-001\n    text: one\n---\nother: [1, 2]\nllrs:\n  - text: two\n    ref: LLR-YK-002\n";
    let (d, ps) = write("yamlkeys", &[("llr.yaml", y)]);
    let r = read(&ps, &keys, &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.get("LLR-YK-001").unwrap().line, Some(2));
    assert_eq!(r.get("LLR-YK-002").unwrap().line, Some(8), "the second document counts lines from the top of the file");
    let _ = std::fs::remove_dir_all(d);
}

// ---- CSV ----------------------------------------------------------------

/// README.md:665-667 "RFC 4180 throughout: a quoted field may hold commas,
/// newlines ... and `\r\n` endings ... are read as written" and
/// README.md:673 "Each entry records the line it began on." A quoted CRLF
/// inside a cell is content and moves the next row's line on by one; a last
/// row without a final line ending is still a row (RFC 4180 2.2).
#[test]
fn a_quoted_crlf_is_content_and_the_last_row_needs_no_line_ending() {
    let csv = "id,text,type\r\nLLR-CR-001,\"first line\r\nsecond line\",low-level\r\nLLR-CR-002,last,low-level";
    let (d, ps) = write("crlf", &[("llr.csv", csv)]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let one = r.get("LLR-CR-001").expect("first");
    assert!(one.text.contains("first line") && one.text.contains("second line"), "both lines are the statement: {:?}", one.text);
    assert_eq!(one.line, Some(2));
    let two = r.get("LLR-CR-002").expect("the unterminated last row is read");
    assert_eq!(two.text, "last");
    assert_eq!(two.line, Some(4), "it began on line 4, after the two-line cell");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:682-685 "an empty `id` cell (named by line) ... A blank line is
/// no row at all and is skipped wherever it sits, so the empty-`id` rule does
/// not apply to it; a line of bare commas is a row of empty cells, and does."
/// The rows around a blank line keep the lines they began on (README.md:688).
#[test]
fn a_blank_csv_line_is_skipped_and_a_line_of_bare_commas_is_refused() {
    let csv = "\nid,text,type\n\nLLR-BL-001,one,low-level\n   \nLLR-BL-002,two,low-level\n\n";
    let (d, ps) = write("blank", &[("llr.csv", csv)]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("blank lines are skipped, not refused");
    assert_eq!(r.entries.len(), 2, "and are no entries: {:?}", r.entries.iter().map(|q| &q.id).collect::<Vec<_>>());
    assert_eq!(r.get("LLR-BL-001").unwrap().line, Some(4));
    assert_eq!(r.get("LLR-BL-002").unwrap().line, Some(6), "a whitespace-only line is blank too");
    let _ = std::fs::remove_dir_all(d);

    let (d, ps) = write("commas", &[("llr.csv", "id,text,type\nLLR-BL-003,one,low-level\n,,\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("a row of empty cells has an empty id");
    assert!(e.contains("line 3"), "named by line: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:651-655 "`code` and `tests` are lists ... split on
/// `csv_list_separator` ... not a colon, because a claim is `path::symbol`
/// and is split at its first `::`" -- a claim keeps every `::` it has.
#[test]
fn a_claim_keeps_its_double_colons_and_a_custom_separator_is_honoured() {
    let csv = "id,text,code,tests\nLLR-SP-001,s,src/a.rs::outer::f;src/b.rs::g,tests/t.rs::m::t\n";
    let (d, ps) = write("sep", &[("llr.csv", csv)]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let q = r.get("LLR-SP-001").unwrap();
    assert_eq!(q.claim.code, ["src/a.rs::outer::f", "src/b.rs::g"]);
    assert_eq!(q.claim.tests, ["tests/t.rs::m::t"]);
    let _ = std::fs::remove_dir_all(d);

    let keys = Keys { csv_list_separator: "|".into(), ..Keys::default() };
    let csv = "id,text,code\nLLR-SP-002,s,src/a.rs::f|src/b.rs::g;h\n";
    let (d, ps) = write("sep2", &[("llr.csv", csv)]);
    let r = read(&ps, &keys, &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.get("LLR-SP-002").unwrap().claim.code, ["src/a.rs::f", "src/b.rs::g;h"], "split on `|` only");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:668-670 "What is refused is what could not be read honestly --
/// ... a row with more cells than the header" -- a wider row anywhere in the
/// file, not only the first, refuses the document and says where.
#[test]
fn a_wide_row_after_good_ones_is_refused_naming_its_line() {
    let csv = "id,text,type\nLLR-WD-001,ok,low-level\nLLR-WD-002,ok,low-level\nLLR-WD-003,too,low-level,extra\n";
    let (d, ps) = write("wide", &[("llr.csv", csv)]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains('4'), "names line 4: {e}");
    let _ = std::fs::remove_dir_all(d);
}

// ---- Markdown -----------------------------------------------------------

/// README.md:708-709 "**One `yaml` block per entry is structural**, wherever
/// it sits" -- above the prose as well as below it -- and README.md:704-707
/// "a ` ```c ` example ... The statement is kept as written, not folded onto
/// one line."
#[test]
fn the_yaml_block_is_structural_wherever_it_sits_and_the_prose_is_verbatim() {
    let md = "## LLR-MD-001\n\n```yaml\ntype: derived\nstatus: implemented\n```\n\nFirst paragraph.\n\n```c\nint x = 1;\n```\n\n### A sub-heading\n\nLast paragraph.\n";
    let (d, ps) = write("mdblock", &[("llr.md", md)]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let q = r.get("LLR-MD-001").unwrap();
    assert!(q.is_derived(), "the block above the prose was read as the fields");
    assert_eq!(q.status.as_deref(), Some("implemented"));
    assert!(!q.text.contains("status: implemented"), "and is not part of the statement: {:?}", q.text);
    assert!(q.text.contains("```c\nint x = 1;\n```"), "a c block is statement, verbatim: {:?}", q.text);
    assert!(q.text.contains("### A sub-heading"), "a deeper heading is statement: {:?}", q.text);
    assert!(q.text.contains("First paragraph.\n"), "not folded onto one line: {:?}", q.text);
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:701-703 "**`#` is a section heading.** It starts no entry and
/// closes the one above it" -- what follows the `#` heading, up to the next
/// `##`, belongs to no entry.
#[test]
fn a_section_heading_between_entries_closes_the_first() {
    let md = "## LLR-SC-001\n\nfirst statement\n\n# Component two\n\nintroduction to component two\n\n## LLR-SC-002\n\nsecond statement\n";
    let (d, ps) = write("section", &[("llr.md", md)]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.get("LLR-SC-001").unwrap().text, "first statement");
    assert_eq!(r.get("LLR-SC-002").unwrap().text, "second statement");
    assert_eq!(r.get("LLR-SC-002").unwrap().line, Some(9), "README.md:729 each entry records the line of its heading");
    let _ = std::fs::remove_dir_all(d);
}

// ---- bounded reads ------------------------------------------------------

/// README.md:1583-1592 "Every artifact `ingest` reads ... the auxiliary
/// files -- is sized before it is read ... A crafted or truncated input fails
/// with a sentence naming the file ... Every refusal names the variable that
/// would have let it through" -- applied to every requirement format.
#[test]
fn an_oversized_requirement_document_is_refused_naming_the_aux_variable() {
    let pad = "x".repeat(4096);
    let (d, ps) = write(
        "big",
        &[
            ("big.yaml", &format!("requirements:\n  - id: LLR-BG-001\n    text: {pad}\n")),
            ("big.csv", &format!("id,text\nLLR-BG-002,{pad}\n")),
            ("big.md", &format!("## LLR-BG-003\n\n{pad}\n")),
        ],
    );
    for p in &ps {
        let small = Limits::new(GIB, GIB, GIB, GIB, GIB, 1024, 8 * GIB);
        let e = read(std::slice::from_ref(p), &Keys::default(), &small, &mut Vec::new()).expect_err("over the auxiliary cap");
        let name = p.file_name().unwrap().to_str().unwrap();
        assert!(e.contains("BUTTERFLY_MAX_AUX_MB"), "{name}: names the variable: {e}");
        assert!(e.contains(name), "{name}: names the file: {e}");
    }
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1583-1586 "against a per-kind cap and a budget for the whole
/// run" -- two documents each under the per-file cap that together exceed
/// the run budget are refused, naming the budget's variable.
#[test]
fn requirement_documents_share_the_run_budget() {
    let pad = "y".repeat(3000);
    let (d, ps) = write("budget", &[("a.csv", &format!("id,text\nLLR-BU-001,{pad}\n")), ("b.csv", &format!("id,text\nLLR-BU-002,{pad}\n"))]);
    let tight = Limits::new(GIB, GIB, GIB, GIB, GIB, 4096, 5000);
    let e = read(&ps, &Keys::default(), &tight, &mut Vec::new()).expect_err("the pair is over the run budget");
    assert!(e.contains("BUTTERFLY_MAX_TOTAL_MB"), "names the budget's variable: {e}");
    let _ = std::fs::remove_dir_all(d);
}

// ---- malformed and abnormal documents ----------------------------------

/// README.md:797-799 "A document that does not match is **refused** ...
/// rather than read as zero requirements" -- whatever the wrong value under
/// the list key is.
#[test]
fn a_requirements_key_holding_a_scalar_is_refused() {
    for (tag, v) in [("num", "42"), ("bool", "true"), ("str", "just some text")] {
        let (d, ps) = write(&format!("scalar-{tag}"), &[("llr.yaml", &format!("requirements: {v}\n"))]);
        let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
        assert!(e.contains("llr.yaml"), "{tag}: names the file: {e}");
        let _ = std::fs::remove_dir_all(d);
    }
}

/// README.md:1589-1590 "A crafted or truncated input fails with a sentence
/// naming the file instead of exhausting memory" -- a deeply nested YAML
/// document is refused, not a stack overflow.
#[test]
fn a_deeply_nested_document_is_refused_not_a_crash() {
    let depth = 20_000;
    let y = format!("requirements:\n  - id: LLR-DP-001\n    text: t\n    rationale: {}{}\n", "[".repeat(depth), "]".repeat(depth));
    let (d, ps) = write("deep", &[("llr.yaml", &y)]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("llr.yaml"), "names the file: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:626-628 the analysis fields "are kept whole" -- a rationale
/// written as a list keeps every item.
#[test]
fn a_detail_field_written_as_a_list_keeps_every_item() {
    let y = "requirements:\n  - id: LLR-DL-001\n    text: t\n    rationale:\n      - first reason\n      - second reason\n";
    let (d, ps) = write("detaillist", &[("llr.yaml", y)]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let why = r.get("LLR-DL-001").unwrap().detail.get("rationale").cloned().unwrap_or_default();
    assert!(why.contains("first reason") && why.contains("second reason"), "{why:?}");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:668-672 "What is refused is what could not be read honestly"
/// -- an empty file has no header at all, so no `id` column.
#[test]
fn an_empty_csv_file_is_refused() {
    let (d, ps) = write("emptycsv", &[("llr.csv", "")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("llr.csv"), "names the file: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:668 "What is refused is what could not be read honestly" -- a
/// header naming one column twice leaves which cell holds the value a guess.
#[test]
fn a_header_naming_a_column_twice_is_refused() {
    let (d, ps) = write("dupcol", &[("llr.csv", "id,text,text\nLLR-DC-001,one,two\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("text"), "names the column: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:716-718 / 680-695: the block holds the entry's fields "through
/// the same parser" as a `.yaml` document's entry -- an empty block adds
/// none, and a block that is a list rather than fields is refused.
#[test]
fn an_empty_block_adds_no_fields_and_a_list_block_is_refused() {
    let (d, ps) = write("mdempty", &[("llr.md", "## LLR-EB-001\n\nthe statement\n\n```yaml\n```\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("an empty block reads");
    let q = r.get("LLR-EB-001").unwrap();
    assert_eq!(q.text, "the statement");
    assert!(q.status.is_none() && q.kind.is_none());
    let _ = std::fs::remove_dir_all(d);

    let (d, ps) = write("mdlist", &[("llr.md", "## LLR-EB-002\n\nthe statement\n\n```yaml\n- type\n- derived\n```\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("a list is not fields");
    assert!(e.contains("llr.md"), "names the file: {e}");
    let _ = std::fs::remove_dir_all(d);
}
