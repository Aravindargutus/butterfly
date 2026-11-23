// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Reading low-level requirement text out of Markdown (#56).
//!
//! The format a requirement is *written* in rather than exported to, so the
//! statement is prose on the page and the fields are a `yaml` block under
//! it -- parsed through the same `Keys` as a `.yaml` document, which is why
//! nesting survives and nothing needs flattening.

use butterfly_ingest::reqtext::{read, Keys};
use butterfly_model::limits::Limits;
use std::path::PathBuf;

fn write(tag: &str, files: &[(&str, &str)]) -> (PathBuf, Vec<PathBuf>) {
    let d = butterfly_testsupport::scratch_dir(&format!("reqmd-{tag}"));
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
/// lesser kind (DO-178C 5.1.2.h), and the format it is written in does not
/// change that.
#[test]
fn a_derived_requirement_is_a_low_level_requirement() {
    let (d, ps) = write(
        "derived",
        &[(
            "llr.md",
            "## LLR-A-001\n\nDecomposed from a parent.\n\n\
             ```yaml\ntype: low-level\ntraceability:\n  parent: HLR-A-001\n```\n\n\
             ## LLR-A-002\n\nA design decision nothing higher asked for.\n\n\
             ```yaml\ntype: derived\nstatus: implemented\ntraceability:\n  code: [src/a.rs::f]\n  tests: [tests/a.rs::t]\n```\n",
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 2, "both kinds are low-level requirements");
    assert!(r.skipped.is_empty(), "and nothing was skipped: {:?}", r.skipped);

    let derived = r.get("LLR-A-002").expect("the derived one");
    assert!(derived.is_derived());
    assert_eq!(derived.parent, None, "derived means no parent, and that is correct here");
    assert_eq!(derived.claim.code, ["src/a.rs::f"], "it still carries a code claim");
    assert_eq!(derived.status.as_deref(), Some("implemented"));
    assert_eq!(r.get("LLR-A-001").unwrap().parent.as_deref(), Some("HLR-A-001"));
    let _ = std::fs::remove_dir_all(d);
}

/// Whatever is not taken is counted and named. A reader who expected 304 and
/// got 245 must be able to see where the rest went.
#[test]
fn an_entry_of_an_unaccepted_kind_is_counted_not_dropped() {
    let (d, ps) = write(
        "skip",
        &[("llr.md", "## LLR-B-001\n\nkept\n\n```yaml\ntype: low-level\n```\n\n## HLR-B-001\n\na high-level requirement that wandered in\n\n```yaml\ntype: high-level\n```\n")],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 1);
    assert_eq!(r.skipped.get("high-level"), Some(&1), "said so: {:?}", r.skipped);
    let _ = std::fs::remove_dir_all(d);
}

/// The reason this shape was chosen over bold labels (`**Status:**
/// implemented`): the fields keep their nesting, and the block goes through
/// the same parser and the same keys as a `.yaml` document, so `safety:` --
/// a mapping of mappings -- survives where a line-oriented reader over prose
/// would have nowhere to put it.
#[test]
fn the_statement_is_prose_and_the_fields_keep_their_nesting() {
    let (d, ps) = write(
        "split",
        &[(
            "llr.md",
            "## LLR-C-001\n\nThe statement, which is what goes beside the code.\n\n\
             ```yaml\ntype: low-level\nrationale: the reasoning, longer and not wanted on a source line\nsafety:\n  related: true\n  failure_condition: minor\nverification_criteria: how it is shown\n```\n",
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let e = r.get("LLR-C-001").unwrap();
    assert_eq!(e.text, "The statement, which is what goes beside the code.");
    assert!(e.detail.contains_key("rationale") && e.detail.contains_key("verification_criteria"));
    assert!(e.detail["safety"].contains("failure_condition"), "a nested block is flattened for the pane, not lost");
    let _ = std::fs::remove_dir_all(d);
}

/// Requirement prose contains colons, and a `code:` line in a statement is
/// a sentence rather than a traceability claim. The structured half is the
/// block and only the block, so there is nothing to mistake.
#[test]
fn a_key_looking_line_in_the_statement_is_text_not_structure() {
    let (d, ps) = write(
        "prose",
        &[(
            "llr.md",
            "## LLR-D-001\n\nCompletes the mapping from ModeTransition to a caller-visible return\n\
             code: classify_transition's Rejected variant is reported as something\nelse entirely.\n\n```yaml\ntype: low-level\ntraceability:\n  code: []\n```\n",
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let e = r.get("LLR-D-001").unwrap();
    assert!(e.claim.code.is_empty(), "the real claim is empty, whatever the prose says");
    assert!(e.text.contains("code: classify_transition"), "and the prose is kept whole");
    let _ = std::fs::remove_dir_all(d);
}

/// The statement is kept as written rather than folded onto one line: a
/// long requirement has structure of its own -- a list, a table, a heading
/// deeper than the entry's -- and a reader that rewrote it would be
/// answering for a document it had altered.
#[test]
fn a_statement_may_contain_lines_that_look_like_headings() {
    let (d, ps) = write(
        "headinglike",
        &[("llr.md", "## LLR-E-001\n\nThe mapping shall hold.\n\n### Note\n\nA line beginning #hashtag is a word, not a heading.\n\n```yaml\ntype: low-level\n```\n")],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 1, "a heading deeper than the entry's starts no entry");
    let e = r.get("LLR-E-001").unwrap();
    assert!(e.text.contains("### Note"), "it belongs to the statement, verbatim: {:?}", e.text);
    assert!(e.text.contains("#hashtag"), "and so does a hash that is not a heading at all: {:?}", e.text);
    let _ = std::fs::remove_dir_all(d);
}

/// A fence closes on one at least as long as the one that opened it, so an
/// example containing a fence is an example rather than the end of the
/// block -- and a heading inside a fenced block is somebody's sample text,
/// not the start of a requirement.
#[test]
fn a_fenced_block_may_contain_fences_and_headings_of_its_own() {
    let (d, ps) = write(
        "fences",
        &[(
            "llr.md",
            "## LLR-F-001\n\nShown with an example:\n\n```c\nint f(void) { return 1; }\n## NOT-AN-ENTRY\n```\n\n\
             ````yaml\ntype: low-level\nstatus: implemented\nrationale: |\n  A fence inside the block:\n  ```\n  not the end of it\n  ```\n````\n",
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 1, "the heading inside the C example started nothing");
    let e = r.get("LLR-F-001").unwrap();
    assert!(e.text.contains("int f(void)"), "a non-yaml fence is part of the statement: {:?}", e.text);
    assert_eq!(e.status.as_deref(), Some("implemented"));
    assert!(e.detail["rationale"].contains("not the end of it"), "and the longer fence held: {:?}", e.detail);
    let _ = std::fs::remove_dir_all(d);
}

/// An entry with no block is an entry whose fields are absent, which is a
/// thing a document is allowed to say. The statement is the point; the rest
/// is optional.
#[test]
fn an_entry_with_no_yaml_block_is_read_with_its_fields_absent() {
    let (d, ps) = write("noblock", &[("llr.md", "## LLR-G-001\n\nThe simplest a requirement gets.\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    let e = r.get("LLR-G-001").unwrap();
    assert_eq!(e.text, "The simplest a requirement gets.");
    assert_eq!((e.kind.as_deref(), e.status.as_deref(), e.parent.as_deref()), (None, None, None));
    assert!(e.claim.code.is_empty() && e.detail.is_empty());
    assert!(r.skipped.is_empty(), "an absent kind is taken at its word, as in YAML: {:?}", r.skipped);
    let _ = std::fs::remove_dir_all(d);
}

/// An ID with no statement is traceable, so it is kept -- but a matrix built
/// on requirements that say nothing cannot be reviewed, so it is listed
/// rather than passed over.
#[test]
fn a_requirement_with_no_statement_is_kept_and_listed() {
    let (d, ps) = write("notext", &[("llr.md", "## LLR-H-001\n\nhas one\n\n## LLR-H-002\n\n```yaml\ntype: low-level\nstatus: planned\n```\n\n## LLR-H-003\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 3, "all three are traceable and kept");
    assert_eq!(r.without_text, ["LLR-H-002", "LLR-H-003"], "and the silent two are named");
    assert_eq!(r.get("LLR-H-001").unwrap().text, "has one");
    let _ = std::fs::remove_dir_all(d);
}

/// A document's title and its explanation belong to no requirement, and a
/// `#` heading closes the entry above it so a document can be organised by
/// component. Neither can carry an ID, so there is nothing to attach them to.
#[test]
fn preamble_and_section_headings_belong_to_no_entry() {
    let (d, ps) = write(
        "preamble",
        &[(
            "llr.md",
            "# Limits component\n\nThis paragraph explains the document and is nobody's requirement.\n\n\
             ## LLR-I-001\n\nthe statement\n\n# Notes for the reviewer\n\nProse under a section heading, after the entry closed.\n",
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.entries.len(), 1, "one entry, and the two `#` headings started none");
    assert_eq!(r.get("LLR-I-001").unwrap().text, "the statement", "neither the preamble nor what follows the section heading joined it");
    let _ = std::fs::remove_dir_all(d);
}

/// A heading that is not an ID is refused rather than read as an ID with
/// spaces in it that no tag could match, or as an ID with the rest thrown
/// away. The message says where the heading belongs instead.
#[test]
fn a_level_two_heading_that_is_not_an_id_is_refused_naming_the_line() {
    let (d, ps) = write("notanid", &[("llr.md", "## LLR-J-001\n\nfine\n\n## Notes on the above\n\nand these are the notes\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("line 5"), "naming where: {e}");
    assert!(e.contains("Notes on the above") && e.contains("llr.md"), "and what and which file: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// The flat analogue of "no `requirements` list". A document whose entries
/// are all written `# LLR-1` has none at all under this rule, and a
/// zero-entry sidecar written over it would be the silent failure this whole
/// reader was hardened against (review of #56).
#[test]
fn a_document_with_no_entries_is_refused_naming_the_headings_it_has() {
    let (d, ps) = write("noentries", &[("llr.md", "# Limits component\n\nSome prose.\n\n### A sub-heading\n\nMore prose.\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("no requirements"), "{e}");
    assert!(e.contains("# Limits component") && e.contains("### A sub-heading"), "and says what the document does have: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// Two entries under one ID in one document is the same ambiguity as one ID
/// in two documents, and the line is named because this reader knows it.
#[test]
fn one_id_twice_in_one_document_is_refused_naming_both_lines() {
    let (d, ps) = write("dupone", &[("llr.md", "## LLR-K-001\n\nfirst\n\n## LLR-K-002\n\nbetween\n\n## LLR-K-001\n\nsecond\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("LLR-K-001") && e.contains("line 1") && e.contains("line 9"), "both places, not the file twice: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// Two documents claiming one ID is ambiguity a reader cannot resolve.
#[test]
fn one_id_defined_in_two_files_is_refused_naming_both() {
    let (d, ps) = write("dup", &[("a.md", "## LLR-L-001\n\nfrom one\n"), ("b.md", "## LLR-L-001\n\nfrom two\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("LLR-L-001") && e.contains("a.md") && e.contains("b.md"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

/// The heading is where the ID lives. A block that names a different one is
/// not overruled quietly in either direction.
#[test]
fn a_block_naming_a_different_id_than_its_heading_is_refused() {
    let (d, ps) = write("twoids", &[("llr.md", "## LLR-M-001\n\nthe statement\n\n```yaml\nid: LLR-M-009\ntype: low-level\n```\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("LLR-M-001") && e.contains("LLR-M-009"), "naming both: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// Which of two blocks carries the entry's fields is not a thing to guess.
#[test]
fn a_second_yaml_block_under_one_heading_is_refused() {
    let (d, ps) = write("twoblocks", &[("llr.md", "## LLR-N-001\n\nthe statement\n\n```yaml\ntype: low-level\n```\n\nand more\n\n```yaml\nstatus: implemented\n```\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("second") && e.contains("LLR-N-001"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

/// One block is one YAML document. A `---` on its own line starts a second,
/// and a reader that took the first would answer for a document it had read
/// part of -- so it is refused, naming the fence's line, rather than merged
/// under a policy nobody wrote down (review of #56).
#[test]
fn a_second_yaml_document_inside_one_block_is_refused_naming_the_line() {
    let (d, ps) = write("twodocs", &[("llr.md", "## LLR-S-001\n\nthe statement\n\n```yaml\ntype: low-level\n---\nstatus: implemented\n```\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("line 5"), "naming the fence: {e}");
    assert!(e.contains("LLR-S-001") && e.contains("llr.md"), "and the entry and the file: {e}");
    assert!(e.contains("2 YAML documents") && e.contains("`---`"), "and says how many and what starts one: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// The shape that was imported quietly before: a status, a separator, and
/// then the claims and the safety detail. Those claims are exactly what
/// `butterfly claims` compares later, so losing them loses the comparison --
/// and the sidecar said nothing was missing (review of #56).
#[test]
fn a_status_before_a_separator_is_not_imported_as_the_whole_entry() {
    let (d, ps) = write(
        "lostclaims",
        &[(
            "llr.md",
            "## LLR-S-002\n\nEvery fault code shall map to a stable name.\n\n\
             ```yaml\nstatus: implemented\n---\ntraceability:\n  code: [src/limits.c::fault_name]\n  tests: [src/tests.c::t_faults]\n\
             safety:\n  related: true\n  failure_condition: minor\n```\n",
        )],
    );
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused rather than imported as a bare status");
    assert!(e.contains("traceability") && e.contains("safety"), "naming the fields that would have gone: {e}");
    assert!(e.contains("line 5") && e.contains("LLR-S-002"), "and where: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// Only a `---` that is a document separator is one. Inside a block scalar
/// it is text -- a quoted diff hunk, a horizontal rule in a rationale -- and
/// a rule that refused it would refuse documents that lose nothing.
#[test]
fn a_separator_inside_a_block_scalar_is_text_and_still_reads() {
    let (d, ps) = write(
        "scalardash",
        &[(
            "llr.md",
            "## LLR-S-003\n\nthe statement\n\n\
             ```yaml\ntype: low-level\nstatus: implemented\nrationale: |\n  the reasoning, which quotes a diff:\n  --- a/src/limits.c\n  ---\n  and carries on\n\
             traceability:\n  code: [src/limits.c::fault_name]\n```\n",
        )],
    );
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("one document, whatever it says inside a scalar");
    let e = r.get("LLR-S-003").expect("the entry");
    assert_eq!(e.status.as_deref(), Some("implemented"));
    assert_eq!(e.claim.code, ["src/limits.c::fault_name"], "the fields below the scalar are still read");
    assert!(e.detail["rationale"].contains("---"), "and the dashes survive as text: {:?}", e.detail["rationale"]);
    let _ = std::fs::remove_dir_all(d);
}

/// A fence that is never closed swallows every heading below it, so the
/// entries after it simply would not exist. Refusing beats reporting the
/// ones above it as the whole document.
#[test]
fn a_fence_that_is_never_closed_is_refused() {
    let (d, ps) = write("unclosed", &[("llr.md", "## LLR-O-001\n\nthe statement\n\n```yaml\ntype: low-level\n\n## LLR-O-002\n\nswallowed\n")]);
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    assert!(e.contains("line 5") && e.contains("never closed"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

/// The embedded blocks get the discipline a YAML file gets. An alias bomb is
/// small on disk and expensive to load, and it is refused at the event
/// stream whether it arrives as a document or inside a fence.
#[test]
fn an_alias_inside_an_embedded_block_is_refused_before_it_is_expanded() {
    let mut y = String::from("## LLR-P-001\n\nthe statement\n\n```yaml\na: &a [x,x,x,x,x,x,x,x,x]\n");
    for (c, p) in [('b', 'a'), ('c', 'b'), ('d', 'c'), ('e', 'd'), ('f', 'e'), ('g', 'f')] {
        y.push_str(&format!("{c}: &{c} [*{p},*{p},*{p},*{p},*{p},*{p},*{p},*{p},*{p}]\n"));
    }
    y.push_str("```\n");
    assert!(y.len() < 400, "the bomb is small on disk, which is the point");
    let (d, ps) = write("bomb", &[("llr.md", &y)]);

    let t = std::time::Instant::now();
    let e = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect_err("refused");
    let took = t.elapsed();

    assert!(e.contains("alias"), "and says why: {e}");
    assert!(e.contains("line 5"), "and which block: {e}");
    assert!(took < std::time::Duration::from_millis(500), "without expanding it first (took {took:?})");
    let _ = std::fs::remove_dir_all(d);
}

/// The line a reader is sent to is the heading's, which is where the entry
/// starts on the page.
#[test]
fn the_line_recorded_is_the_heading_the_entry_starts_at() {
    let (d, ps) = write("line", &[("llr.md", "# A component\n\nsome preamble\n\n## LLR-Q-001\n\nfirst\n\n## LLR-Q-002\n\nsecond\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.get("LLR-Q-001").unwrap().line, Some(5));
    assert_eq!(r.get("LLR-Q-002").unwrap().line, Some(9));
    let _ = std::fs::remove_dir_all(d);
}

/// Each file digested on its own: requirements live one file per component,
/// and a single hash over the set could not say which document moved.
#[test]
fn every_source_file_carries_its_own_digest() {
    let (d, ps) = write("prov", &[("a.md", "## LLR-R-001\n\na\n"), ("b.md", "## LLR-R-002\n\nb\n")]);
    let r = read(&ps, &Keys::default(), &lim(), &mut Vec::new()).expect("reads");
    assert_eq!(r.sources.len(), 2);
    assert_ne!(r.sources[0].sha256, r.sources[1].sha256);
    assert!(r.sources.iter().all(|s| s.sha256.len() == 64), "a real SHA-256 each");
    assert_eq!(r.sources[0].count, 1);
    assert_eq!(r.entries.iter().filter(|e| e.source == 1).count(), 1, "and each entry knows its file");
    r.validate().expect("well formed");
    let _ = std::fs::remove_dir_all(d);
}

/// Every project shapes its requirements differently, and the block is read
/// through the *same* key mapping as a `.yaml` document -- which is the
/// point of putting the fields in YAML rather than in bold labels. No new
/// key, and nothing to keep in step (#56).
#[test]
fn a_projects_own_key_names_are_read_from_config() {
    let (d, ps) = write(
        "keys",
        &[(
            "llr.md",
            "## SRS-042\n\nThe widget shall retract within 200 ms of the command.\n\n\
             ```yaml\ncategory: low-level\nstate: implemented\nlinks:\n  derives_from: HLR-9\n  impl: [src/widget.c::retract]\n  verifies: [tests/widget.c::t_retract]\nwhy: Timing is the safety-relevant property here.\n```\n",
        )],
    );
    let keys = Keys {
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
    assert!(e.text.starts_with("The widget shall retract"), "the ID is the heading, whatever `id` is called");
    assert_eq!(e.status.as_deref(), Some("implemented"));
    assert_eq!(e.parent.as_deref(), Some("HLR-9"));
    assert_eq!(e.claim.code, ["src/widget.c::retract"]);
    assert_eq!(e.claim.tests, ["tests/widget.c::t_retract"]);
    assert!(e.detail.contains_key("why"));
    let _ = std::fs::remove_dir_all(d);
}
