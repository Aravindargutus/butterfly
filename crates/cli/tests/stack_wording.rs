// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The Stack tab's standing note must name the categories the stack model
//! can actually report, and no others (#234).
//!
//! The note is assurance text: a reviewer reading a Level A stack figure is
//! entitled to read "X is reported, not bounded" as "the tool looked for X".
//! It used to say `interrupt frames`, a category nothing in
//! `crates/ingest/src/stack.rs` detects — the walk is over
//! `StackNode.callees`, which is direct calls only — while omitting dynamic
//! frames, which it does detect and report.
//!
//! There is no JavaScript test runner in this workspace, so this guards the
//! wording statically, the way `mcdc_wording.rs` does. What makes it more
//! than a spell-check is the second half: the categories the note names are
//! compared against the `reason` strings `stack.rs` can attach to an
//! unbounded figure, so the note cannot drift away from the code in either
//! direction — a category added to the note without an implementation fails
//! here, and so does an implementation the note does not mention. The
//! second direction is checked by reading every `reason = Some(...)` the
//! stack walk can assign and requiring each to fall under a named category;
//! until #318 this comment claimed that check and the test did not make it,
//! and the note was three reasons short.

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The one clause under test: what the note says is "reported, not bounded".
fn reported_not_bounded(js: &str) -> String {
    let note = js.lines().find(|l| l.contains("Worst case = own frame + deepest chain of direct calls")).expect("the Stack tab's standing note is in ui/app.js");
    let (_, rest) = note.split_once("object code; ").expect("the note lists what it reports after the semicolon");
    let (clause, _) = rest.split_once(" are reported, not bounded").expect("the note says which categories are reported rather than bounded");
    clause.to_string()
}

/// Each category the note names, and the marker that identifies its reason
/// strings: every reason the stack walk assigns in that category contains the
/// marker. Markers are matched against the assigned reason literals, never
/// against the file's raw text, where a word like "recursion" also appears in
/// identifiers and comments. The note is written for a reader, the reason is written for the
/// panel, so they are matched by hand rather than by string equality — but
/// every pair must hold, and the list must be complete in both directions.
const CATEGORIES: &[(&str, &str)] = &[
    ("indirect calls", "indirect call(s): callee set unknown"),
    ("recursion", "recursion"),
    ("dynamic frames (alloca / VLA)", "dynamic frame (alloca / VLA)"),
    ("frames of undetermined size", "frame size could not be determined"),
    ("call chains deeper than the analysable depth", "exceeds the analysable depth"),
    ("totals that overflow", "overflows u64"),
];

/// The production half of `stack.rs`: everything before its unit tests, whose
/// fixtures may build a `Memo` with any reason they like.
fn production(stack_rs: &str) -> &str {
    stack_rs.split_once("#[cfg(test)]").map_or(stack_rs, |(p, _)| p)
}

/// The string literal at the start of `s` (after an optional `format!(`),
/// with `\"` escapes honoured, or `None` if `s` does not start with one.
fn leading_literal(s: &str) -> Option<String> {
    let mut chars = s.trim_start_matches("format!(").strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

/// Every reason string the stack walk can assign: the literal in each
/// `reason = Some(...)` or `reason: Some(...)` in the production code of
/// `stack.rs`. Comparisons (`== Some(...)`) are not assignments and are left
/// out. A reason built by `format!` is read as its format string, which is
/// what the category marker is matched against.
///
/// The scan is textual, so it refuses what it cannot read rather than
/// skipping it. Every `reason = ` / `reason: ` value must be `None`, a
/// `Some(` with a literal on the same line, or a copy of a reason already
/// assigned (its value mentions `reason`: a callee's figure carried up). Any
/// other value, and the `get_or_insert` / `replace` / `insert` methods, fail
/// here with the line, so a new reason cannot reach the panel without
/// passing the note.
fn reasons_assigned(stack_rs: &str) -> Vec<String> {
    let mut out = Vec::new();
    for l in production(stack_rs).lines() {
        let code = l.split_once("//").map_or(l, |(c, _)| c);
        let unreadable = |why: &str| -> ! {
            panic!("stack.rs sets a reason in a form this test cannot read ({why}); write it as `reason = Some(\"...\")` so the Stack note can be checked against it: {l:?}")
        };
        for method in ["reason.get_or_insert", "reason.replace(", "reason.insert("] {
            if code.contains(method) {
                unreadable(method);
            }
        }
        let Some(value) = ["reason = ", "reason: "].iter().find_map(|m| code.split_once(m)).map(|(_, a)| a.trim_start()) else { continue };
        if value.starts_with("Option<") || value.starts_with("None") {
            continue; // a declaration, or no reason
        }
        if let Some(after) = value.strip_prefix("Some(") {
            out.push(leading_literal(after).unwrap_or_else(|| unreadable("not a string literal on the same line")));
        } else if !value.contains("reason") {
            unreadable("neither a literal nor a copy of an existing reason");
        }
    }
    out
}

#[test]
fn every_category_the_stack_note_names_is_one_the_model_can_report() {
    let js = read("ui/app.js");
    let reasons = reasons_assigned(&read("crates/ingest/src/stack.rs"));
    let clause = reported_not_bounded(&js);

    for (named, marker) in CATEGORIES {
        assert!(clause.contains(named), "the Stack note must name {named:?} among what it reports but cannot bound; it says {clause:?}");
        assert!(
            reasons.iter().any(|r| r.contains(marker)),
            "the Stack note names {named:?}, so crates/ingest/src/stack.rs must assign a reason containing {marker:?}; the reasons it assigns are {reasons:?}"
        );
    }

    // Nothing else may be in the list. Split on the separators the sentence
    // uses and check every item is one of CATEGORIES: an extra category
    // would be a claim with no implementation behind it, which is exactly
    // what "interrupt frames" was.
    for item in clause.replace(" and ", ", ").split(", ") {
        let item = item.trim();
        assert!(CATEGORIES.iter().any(|(n, _)| *n == item), "the Stack note reports {item:?}, which the stack model does not detect; add the detection or drop the word (#234)");
    }
}

/// The other direction (#318): every reason the stack walk can attach to a
/// figure is one the note names. A reason added to `stack.rs` without a word
/// in the note fails here, which is what the module comment promised before
/// this test existed.
#[test]
fn every_reason_the_model_can_report_is_one_the_stack_note_names() {
    let stack_rs = read("crates/ingest/src/stack.rs");
    let reasons = reasons_assigned(&stack_rs);
    // A guard against the scan quietly missing a site: every category must be
    // found among the assigned reasons, not just "enough" assignments in all.
    for (named, marker) in CATEGORIES {
        assert!(reasons.iter().any(|r| r.contains(marker)), "no reason assignment for {named:?} ({marker:?}) was found in stack.rs, so the scan is missing sites: {reasons:?}");
    }
    for reason in &reasons {
        assert!(
            CATEGORIES.iter().any(|(_, marker)| reason.contains(marker)),
            "crates/ingest/src/stack.rs can report {reason:?}, which the Stack note does not name; add the category to the note and to CATEGORIES (#318)"
        );
    }
}

/// The scanner itself: escaped quotes, comments, the test module and the
/// forms it must refuse.
#[test]
fn the_reason_scanner_reads_what_it_should_and_refuses_what_it_cannot() {
    let src = "reason = Some(\"a \\\"q\\\" b\".into());\n// reason = Some(\"commented\")\nx; // reason = Some(\"trailing\")\nMemo { reason: Some(format!(\"f {n}\")) }\n#[cfg(test)]\nreason: Some(\"fixture only\".into())\n";
    assert_eq!(reasons_assigned(src), vec!["a \"q\" b".to_string(), "f {n}".to_string()]);
    assert!(
        reasons_assigned("reason: Option<String>,\nlet mut reason = None;\nself.unbounded_reason = r.reason.clone();\n").is_empty(),
        "declarations, None and copies carry no new reason"
    );
    for bad in ["reason.get_or_insert_with(|| \"x\".into());", "reason = Some(\n    \"split\".into());", "reason = why;"] {
        assert!(std::panic::catch_unwind(|| reasons_assigned(bad)).is_err(), "{bad:?} must be refused, not skipped");
    }
}

/// The specific false claim, and the reason it was false. If interrupt
/// handling is ever modelled, this test is the place that says so.
#[test]
fn the_stack_note_does_not_claim_interrupt_frames_are_accounted_for() {
    let js = read("ui/app.js");
    let stack_rs = read("crates/ingest/src/stack.rs");
    assert!(
        !stack_rs.to_lowercase().contains("interrupt"),
        "crates/ingest/src/stack.rs now mentions interrupts: if the model really has grown an interrupt-frame concept, say so in the Stack note and in the V07 limit, then update this test"
    );
    let clause = reported_not_bounded(&js);
    assert!(!clause.contains("interrupt"), "the stack model has no interrupt-frame concept, so the note must not list one: {clause:?}");
    // ...and the note must say what it does NOT cover, rather than leaving
    // the reader to infer it from an absence.
    assert!(
        js.contains("Interrupts and alternate stacks are outside this figure entirely"),
        "the Stack note must say plainly that interrupts and alternate stacks are outside the figure"
    );
}

/// The showcase's V07 limit and the note are two statements about the same
/// figure, published side by side. Before #234 they contradicted each other
/// and the limit had to warn the reader which one to believe; now that the
/// note is right, the limit must not still be hedging against it.
#[test]
fn the_v07_limit_no_longer_contradicts_the_screenshot_it_sits_under() {
    let toml = read("support/features.toml");
    for hedge in ["The note inside the screenshot above says otherwise", "This limit is the true one", "the note is being corrected under #234"] {
        assert!(!toml.contains(hedge), "V07's limit still hedges against its own screenshot ({hedge:?}); the note was corrected in #234");
    }
    assert!(toml.contains("it says nothing about interrupts or other stacks"), "V07 must still state the limit itself");
}
