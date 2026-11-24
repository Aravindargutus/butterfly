// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Reading low-level requirement text out of YAML (#56).

use butterfly_ingest::reqtext::{read, Keys};
use butterfly_model::limits::Limits;
use std::path::PathBuf;

fn write(tag: &str, files: &[(&str, &str)]) -> (PathBuf, Vec<PathBuf>) {
    let d = butterfly_testsupport::scratch_dir(&format!("reqtext-{tag}"));
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
/// lesser kind. DO-178C 5.1.2.h sends it to the safety assessment *because*
/// nothing higher validates it, so dropping it would remove the entries
/// under the most scrutiny -- and in the corpus this was built against, most
/// of the implemented code traceability is on derived entries.
#[test]
fn a_derived_requirement_is_a_low_level_requirement() {
    let (d, ps) = write(
        "derived",
        &[(
            "llr.yaml",
            r#"
requirements:
  - id: LLR-A-001
    text: decomposed from a parent
    type: low-level
    traceability: { parent: HLR-A-001, code: [], tests: [] }
  - id: LLR-A-002
    text: a design decision nothing higher asked for
    type: derived
    status: implemented
    traceability:
      code: [src/a.rs::f]
      tests: [tests/a.rs::t]
"#,
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

/// Whatever is not taken is counted and named. A reader who expected 304 and
/// got 245 must be able to see where the rest went.
#[test]
fn an_entry_of_an_unaccepted_kind_is_counted_not_dropped() {
    let (d, ps) = write(
        "skip",
        &[(
            "llr.yaml",
            r#"
requirements:
  - id: LLR-B-001
    text: kept
    type: low-level
  - id: HLR-B-001
    text: a high-level requirement that wandered in
    type: high-level
"#,
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 1);
    assert_eq!(r.skipped.get("high-level"), Some(&1), "said so: {:?}", r.skipped);
    let _ = std::fs::remove_dir_all(d);
}

/// Requirement prose contains colons. A line-oriented reader takes a line
/// inside a folded scalar for structure and invents a claim nobody wrote --
/// this exact shape appears in the corpus.
#[test]
fn a_key_looking_line_inside_a_folded_scalar_is_text_not_structure() {
    let (d, ps) = write(
        "folded",
        &[(
            "llr.yaml",
            r#"
requirements:
  - id: LLR-C-001
    text: the mapping shall be complete
    type: low-level
    traceability:
      code: []
      tests: []
    rationale: >-
      Completes the mapping from ModeTransition to a caller-visible return
      code: classify_transition's Rejected variant is reported as
      something else entirely.
"#,
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let e = r.get("LLR-C-001").unwrap();
    assert!(e.claim.code.is_empty(), "the real claim is empty, whatever the prose says");
    assert!(e.detail["rationale"].contains("code: classify_transition"), "and the prose is kept whole");
    let _ = std::fs::remove_dir_all(d);
}

/// Aliases expand multiplicatively, so a file's cost is not visible in its
/// size and a byte cap cannot see it coming. Refused at the event stream,
/// before anything is expanded.
#[test]
fn a_document_using_aliases_is_refused_before_it_is_expanded() {
    let mut y = String::from("a: &a [x,x,x,x,x,x,x,x,x]\n");
    for (c, p) in [('b', 'a'), ('c', 'b'), ('d', 'c'), ('e', 'd'), ('f', 'e'), ('g', 'f')] {
        y.push_str(&format!("{c}: &{c} [*{p},*{p},*{p},*{p},*{p},*{p},*{p},*{p},*{p}]\n"));
    }
    assert!(y.len() < 300, "the bomb is small on disk, which is the point");
    let (d, ps) = write("bomb", &[("llr.yaml", &y)]);

    let t = std::time::Instant::now();
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    let took = t.elapsed();

    assert!(e.contains("alias"), "and says why: {e}");
    assert!(took < std::time::Duration::from_millis(500), "without expanding it first (took {took:?})");
    let _ = std::fs::remove_dir_all(d);
}

/// Two documents claiming one ID is ambiguity a reader cannot resolve.
#[test]
fn one_id_defined_in_two_files_is_refused_naming_both() {
    let one = "requirements:\n  - id: LLR-D-001\n    text: from one\n    type: low-level\n";
    let two = "requirements:\n  - id: LLR-D-001\n    text: from two\n    type: low-level\n";
    let (d, ps) = write("dup", &[("a.yaml", one), ("b.yaml", two)]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("LLR-D-001") && e.contains("a.yaml") && e.contains("b.yaml"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

/// Each file digested on its own: requirements live one file per component,
/// and a single hash over the set could not say which document moved.
#[test]
fn every_source_file_carries_its_own_digest() {
    let (d, ps) = write(
        "prov",
        &[("a.yaml", "requirements:\n  - id: LLR-E-001\n    text: a\n    type: low-level\n"), ("b.yaml", "requirements:\n  - id: LLR-E-002\n    text: b\n    type: low-level\n")],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.sources.len(), 2);
    assert_ne!(r.sources[0].sha256, r.sources[1].sha256);
    assert!(r.sources.iter().all(|s| s.sha256.len() == 64), "a real SHA-256 each");
    assert_eq!(r.sources[0].count, 1);
    assert_eq!(r.entries.iter().filter(|e| e.source == 1).count(), 1, "and each entry knows its file");
    r.validate().expect("well formed");
    let _ = std::fs::remove_dir_all(d);
}

/// The inline half is the point of splitting the record.
#[test]
fn the_statement_is_separated_from_the_analysis_around_it() {
    let (d, ps) = write(
        "split",
        &[(
            "llr.yaml",
            r#"
requirements:
  - id: LLR-F-001
    text: the statement
    type: low-level
    rationale: the reasoning, which is longer and not wanted on a source line
    safety:
      related: true
      failure_condition: minor
    verification_criteria: how it is shown
"#,
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let e = r.get("LLR-F-001").unwrap();
    assert_eq!(e.text, "the statement");
    assert!(e.detail.contains_key("rationale") && e.detail.contains_key("verification_criteria"));
    assert!(e.detail["safety"].contains("failure_condition"), "a nested block is flattened, not lost");
    let _ = std::fs::remove_dir_all(d);
}

/// An entry with no ID cannot be traced to, reported on, or matched against
/// a tag. Reading the rest would mean answering questions about a document
/// while silently ignoring part of it (review of #56).
#[test]
fn an_entry_without_an_id_refuses_the_document_naming_where() {
    let (d, ps) = write(
        "noid",
        &[(
            "llr.yaml",
            r#"
requirements:
  - id: LLR-G-001
    text: fine
    type: low-level
  - text: this one was never given an ID
    type: low-level
"#,
        )],
    );
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("entry 2"), "naming which entry: {e}");
    assert!(e.contains("llr.yaml") && e.contains("id"), "and which file and field: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// An ID with no statement is a different thing: traceable, so kept -- but a
/// matrix built on requirements that say nothing cannot be reviewed, so it
/// is listed rather than passed over.
#[test]
fn a_requirement_with_no_statement_is_kept_and_listed() {
    let (d, ps) = write(
        "notext",
        &[(
            "llr.yaml",
            r#"
requirements:
  - id: LLR-H-001
    text: has one
    type: low-level
  - id: LLR-H-002
    type: low-level
  - id: LLR-H-003
    text: "   "
    type: low-level
"#,
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 3, "all three are traceable and kept");
    assert_eq!(r.without_text, ["LLR-H-002", "LLR-H-003"], "and the silent two are named");
    assert!(r.get("LLR-H-001").unwrap().text == "has one");
    let _ = std::fs::remove_dir_all(d);
}

/// A typo in the list key means the whole file goes unread. Writing a
/// zero-entry sidecar and exiting 0 would be the loudest possible version of
/// "vanished quietly" (review of #56).
#[test]
fn a_document_with_no_requirements_list_is_refused_naming_what_it_has() {
    let (d, ps) = write("nolist", &[("llr.yaml", "requirement:\n  - id: LLR-X-001\n    text: typo in the key above\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("no `requirements` list"), "{e}");
    assert!(e.contains("requirement"), "and says what the document does have: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// ...and a list that is not a list.
#[test]
fn a_wrong_typed_requirements_key_is_refused() {
    let (d, ps) = write("badtype", &[("llr.yaml", "requirements:\n  id: LLR-X-001\n  text: a mapping where a sequence belongs\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("a mapping") && e.contains("not a list"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

/// An explicitly empty list is a document that says there are none, which is
/// a statement rather than a mistake.
#[test]
fn an_explicitly_empty_list_is_allowed() {
    let (d, ps) = write("empty", &[("llr.yaml", "requirements: []\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert!(r.entries.is_empty());
    let _ = std::fs::remove_dir_all(d);
}

/// The ceilings must *stop* the parse, not measure it afterwards.
///
/// A limit checked after `load()` has walked the whole document is a report:
/// the cost is already spent when it fires. The property that distinguishes
/// the two is **scaling** -- a reader that stops at the ceiling costs the
/// same whether the document is a little over it or enormously over it,
/// while one that measures afterwards costs proportionally more.
///
/// Asserted as a ratio rather than a wall-clock bound. An absolute deadline
/// here is a promise about the machine, not about the code, and this test
/// failed once on a loaded host while the ceiling was working perfectly.
#[test]
fn the_event_ceiling_stops_the_parse_rather_than_scaling_with_the_input() {
    // Compact entries, so both documents stay under the 64 MiB auxiliary
    // byte cap: this test is about the *event* ceiling, and a document
    // refused for its size would prove nothing about it.
    let entry = "  - id: L\n    text: t\n";
    // Roughly six events per entry, so 400k is already well past
    // MAX_EVENTS; the second is three times the first, and a reader that
    // walked to the end would take about three times as long.
    let small = format!("requirements:\n{}", entry.repeat(400_000));
    let large = format!("requirements:\n{}", entry.repeat(1_200_000));

    let time_it = |body: &str, tag: &str| {
        let (d, ps) = write(tag, &[("llr.yaml", body)]);
        let t = std::time::Instant::now();
        let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
        let took = t.elapsed();
        assert!(e.contains("parser events"), "refused for the right reason: {e}");
        let _ = std::fs::remove_dir_all(d);
        took
    };
    let a = time_it(&small, "ceil-a");
    let b = time_it(&large, "ceil-b");

    // Three times the input, and the work must not have grown with it. Two
    // is generous room for noise and still clearly below the three a walking
    // reader would show.
    assert!(b.as_secs_f64() < a.as_secs_f64() * 2.0 + 0.5, "3x the document took {b:?} against {a:?} -- the parse scaled with the input instead of stopping at the ceiling");
}

/// Every project shapes its requirements file differently, so the key
/// mapping is configuration rather than an assumption (#56).
#[test]
fn a_projects_own_key_names_are_read_from_config() {
    let (d, ps) = write(
        "keys",
        &[(
            "llr.yaml",
            r#"
llrs:
  - ref: SRS-042
    statement: The widget shall retract within 200 ms of the command.
    category: low-level
    state: implemented
    links:
      derives_from: HLR-9
      impl: [src/widget.c::retract]
      verifies: [tests/widget.c::t_retract]
    why: Timing is the safety-relevant property here.
"#,
        )],
    );

    // the defaults do not fit this document, and say so rather than reading nothing
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("no `requirements` list") && e.contains("llrs"), "{e}");

    let keys = Keys {
        list: "llrs".into(),
        id: "ref".into(),
        text: "statement".into(),
        kind: "category".into(),
        status: "state".into(),
        traceability: "links".into(),
        parent: "derives_from".into(),
        code: "impl".into(),
        tests: "verifies".into(),
        detail: vec!["why".into()],
        ..Keys::default()
    };
    let r = read(&ps, &keys, &lim(), &mut Vec::new()).expect("reads with the project's own names");
    let e = r.get("SRS-042").expect("the entry");
    assert!(e.text.starts_with("The widget shall retract"));
    assert_eq!(e.status.as_deref(), Some("implemented"));
    assert_eq!(e.parent.as_deref(), Some("HLR-9"));
    assert_eq!(e.claim.code, ["src/widget.c::retract"]);
    assert_eq!(e.claim.tests, ["tests/widget.c::t_retract"]);
    assert!(e.detail.contains_key("why"));
    let _ = std::fs::remove_dir_all(d);
}

/// The format is the file's extension and nothing else. Sniffing the
/// contents would make a document's format depend on what it says; guessing
/// YAML would read a `.txt` of anything at all and write a sidecar over it.
/// Refusing by name is the same rule as "no `requirements` list" (#56).
#[test]
fn a_file_in_a_format_this_does_not_know_is_refused_by_name() {
    let (d, ps) = write("ext", &[("llr.txt", "id,text\nLLR-Y-001,neither one thing nor the other\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("llr.txt"), "naming the file: {e}");
    assert!(e.contains(".yaml") && e.contains(".csv") && e.contains(".md"), "and the formats it does understand: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// A component whose requirements exist in two formats is two documents, and
/// one ID in both is the ambiguity any duplicate is. Worth stating on its
/// own, because the directory walk picks up every `llr` document it
/// understands: twin fixtures in one directory are a refusal, not a merge.
#[test]
fn one_id_in_a_yaml_and_a_csv_document_is_refused_like_any_other_duplicate() {
    let (d, ps) = write(
        "mixed",
        &[("llr.yaml", "requirements:\n  - id: LLR-Z-001\n    text: from the yaml\n    type: low-level\n"), ("llr.csv", "id,text,type\nLLR-Z-001,from the csv,low-level\n")],
    );
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("LLR-Z-001") && e.contains("llr.yaml") && e.contains("llr.csv"), "{e}");
    assert!(e.contains("line 2"), "and the half that knows its line says so: {e}");
    let _ = std::fs::remove_dir_all(d);
}
