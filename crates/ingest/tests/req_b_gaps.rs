// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the gap kinds (area B).
//!
//! Expectations come from README.md "Projected, measured, and the gap between
//! them" (~L200-219) and docs/butterfly.html "Gap kinds" (~L484-500), not from
//! `gaps.rs`. Instruction lists are hand-written aarch64.
mod common;
use butterfly_ingest::{cfg, gaps};
use butterfly_model::*;
use common::ri;

/// Run gap detection over `raw`, with per-instruction line numbers (`None` =
/// no line-table entry), the given source branch regions and decisions, and
/// a call resolver.
fn detect(
    raw: &[butterfly_ingest::objectcode::RawInstr],
    lines: &[Option<u32>],
    branches: &[Branch],
    decisions: &[Decision],
    call: &dyn Fn(u64) -> Option<gaps::CallTarget>,
    prefixes: &[String],
) -> Vec<Gap> {
    let end = raw.last().unwrap().addr + 4;
    let c = cfg::build("aarch64", raw[0].addr, end, raw, &|i| (lines[i], Some(0), true), &|_| Status::Covered, &|_| None, &|_| Vec::new(), &|_| Vec::new());
    let mut blocks = c.blocks;
    gaps::detect(gaps::GapInput {
        symbol: "f",
        blocks: &mut blocks,
        branches,
        decisions,
        call_target: call,
        demangle: &|s: &str| s.to_string(),
        project_prefixes: prefixes,
        recorded: true,
        uncounted: false,
    })
}

fn no_call(_: u64) -> Option<gaps::CallTarget> {
    None
}

fn kinds(g: &[Gap]) -> Vec<GapKind> {
    g.iter().map(|g| g.kind).collect()
}

fn decision(line: u32, n: usize) -> Decision {
    Decision {
        line,
        col: 5,
        line_end: line,
        col_end: 30,
        conditions: (0..n).map(|i| Condition { index: i, line, col: 5 + 10 * i as u32, true_count: 1, false_count: 1, independence_covered: false, pair: None }).collect(),
        ..Default::default()
    }
}

/// README.md:205-219 and docs/butterfly.html:486-500: the documented names of
/// every gap kind, exactly as a reviewer will meet them in the Gaps tab, the
/// Overview's gaps-by-kind table and the resolution keys.
#[test]
fn every_gap_kind_has_its_documented_name() {
    let want = [
        (GapKind::PredicatedDecision, "predicated-decision"),
        (GapKind::ExtraObjectBranch, "extra-object-branch"),
        (GapKind::LineZero, "line-zero"),
        (GapKind::NoSourceLine, "no-source-line"),
        (GapKind::HelperCall, "helper-call"),
        (GapKind::IndirectBranch, "indirect-branch"),
        (GapKind::NotInstrumented, "not-instrumented"),
        (GapKind::FoldedCall, "folded-call"),
        (GapKind::SharedLine, "shared-line"),
        (GapKind::UncountedVariant, "uncounted-variant"),
    ];
    for (k, name) in want {
        assert_eq!(gaps::kind_slug(k), name);
    }
    // and the serialised kind in the evidence is the same name
    for (k, name) in want {
        let camel = serde_json::to_value(k).unwrap();
        let back: GapKind = serde_json::from_value(camel).unwrap();
        assert_eq!(gaps::kind_slug(back), name);
    }
}

/// docs/butterfly.html:490 "no-source-line: The instruction has no
/// line-table entry at all", and html:489 "line-zero: DWARF maps the
/// instruction to line 0". The two are different findings, and one is never
/// reported as the other.
#[test]
fn no_line_entry_and_line_zero_are_different_kinds() {
    let raw = vec![ri(0x1000, "mov", "w0, #0x1"), ri(0x1004, "mov", "w1, #0x2"), ri(0x1008, "mov", "w2, #0x3"), ri(0x100c, "ret", "")];
    let g = detect(&raw, &[Some(3), None, Some(0), Some(3)], &[], &[], &no_call, &[]);
    let nsl: Vec<_> = g.iter().filter(|g| g.kind == GapKind::NoSourceLine).collect();
    let lz: Vec<_> = g.iter().filter(|g| g.kind == GapKind::LineZero).collect();
    assert_eq!(nsl.len(), 1, "{g:#?}");
    assert_eq!(lz.len(), 1, "{g:#?}");
    assert_eq!((nsl[0].addr_start, nsl[0].addr_end), (0x1004, 0x1004), "the instruction with no entry");
    assert_eq!((lz[0].addr_start, lz[0].addr_end), (0x1008, 0x1008), "the instruction on line 0");
}

/// README.md:207 "predicated-decision: a source decision compiled to ...
/// merged code with no object-level branch (or no object code at all on that
/// line)". A decision whose line has no instruction at all is that finding.
#[test]
fn a_decision_on_a_line_with_no_object_code_is_predicated() {
    let raw = vec![ri(0x1000, "mov", "w0, #0x1"), ri(0x1004, "ret", "")];
    let g = detect(&raw, &[Some(3), Some(4)], &[], &[decision(7, 2)], &no_call, &[]);
    let p: Vec<_> = g.iter().filter(|g| g.kind == GapKind::PredicatedDecision).collect();
    assert_eq!(p.len(), 1, "{g:#?}");
    assert_eq!(p[0].line, Some(7));
}

/// The same for a plain source branch region with no MC/DC record (Rust, gcov,
/// clang without -fcoverage-mcdc): README.md:207, "a source decision".
#[test]
fn a_branch_region_on_a_line_with_no_object_code_is_predicated() {
    let raw = vec![ri(0x1000, "mov", "w0, #0x1"), ri(0x1004, "ret", "")];
    let b = Branch { line: 9, col: 5, line_end: 9, col_end: 9, true_count: 1, false_count: 0, direction_known: true, decision_ordinal: None };
    let g = detect(&raw, &[Some(3), Some(4)], &[b], &[], &no_call, &[]);
    assert_eq!(g.iter().filter(|g| g.kind == GapKind::PredicatedDecision && g.line == Some(9)).count(), 1, "{g:#?}");
}

/// README.md:207, read the other way: a decision that did compile to a
/// conditional branch on its line is not predicated.
#[test]
fn a_decision_with_an_object_branch_on_its_line_is_not_predicated() {
    let raw = vec![ri(0x1000, "cmp", "w0, #0x0"), ri(0x1004, "b.ne", "0x100c <f+0xc>"), ri(0x1008, "mov", "w0, #0x1"), ri(0x100c, "ret", "")];
    let b = Branch { line: 7, col: 5, line_end: 7, col_end: 9, true_count: 1, false_count: 1, direction_known: true, decision_ordinal: None };
    let g = detect(&raw, &[Some(7), Some(7), Some(8), Some(9)], &[b], &[decision(7, 1)], &no_call, &[]);
    assert!(!kinds(&g).contains(&GapKind::PredicatedDecision), "{g:#?}");
    assert!(!kinds(&g).contains(&GapKind::ExtraObjectBranch), "one object branch for one source region: {g:#?}");
}

/// README.md:208 "extra-object-branch: more conditional branches in the
/// object code than branch regions in the source on that line" -- and only
/// then: two object branches for two regions is not extra.
#[test]
fn extra_object_branch_is_counted_per_line() {
    let raw = vec![
        ri(0x1000, "cmp", "w0, #0x0"),
        ri(0x1004, "b.ne", "0x1014 <f+0x14>"),
        ri(0x1008, "cmp", "w1, #0x0"),
        ri(0x100c, "b.ne", "0x1014 <f+0x14>"),
        ri(0x1010, "mov", "w0, #0x1"),
        ri(0x1014, "ret", ""),
    ];
    let lines = [Some(5), Some(5), Some(5), Some(5), Some(6), Some(7)];
    let br = |col| Branch { line: 5, col, line_end: 5, col_end: col + 3, true_count: 1, false_count: 1, direction_known: true, decision_ordinal: None };
    let one = detect(&raw, &lines, &[br(5)], &[], &no_call, &[]);
    assert_eq!(one.iter().filter(|g| g.kind == GapKind::ExtraObjectBranch).count(), 1, "two object branches, one region: {one:#?}");
    let two = detect(&raw, &lines, &[br(5), br(15)], &[], &no_call, &[]);
    assert!(!kinds(&two).contains(&GapKind::ExtraObjectBranch), "two for two: {two:#?}");
}

/// README.md:209 "helper-call: call to a symbol outside the covered source
/// set"; a call into the covered set is not one, and a call whose target
/// cannot be resolved at all is not silently passed over.
#[test]
fn helper_call_is_reported_only_for_calls_leaving_the_covered_set() {
    let raw = vec![ri(0x1000, "bl", "0x2000 <g>"), ri(0x1004, "bl", "0x3000 <memset>"), ri(0x1008, "bl", "0x4000"), ri(0x100c, "ret", "")];
    let call = |a: u64| match a {
        0x1000 => Some(gaps::CallTarget { symbol: "g".into(), in_source_set: true }),
        0x1004 => Some(gaps::CallTarget { symbol: "memset".into(), in_source_set: false }),
        _ => None,
    };
    let g = detect(&raw, &[Some(2), Some(3), Some(4), Some(5)], &[], &[], &call, &[]);
    let h: Vec<_> = g.iter().filter(|g| g.kind == GapKind::HelperCall).collect();
    assert!(!h.iter().any(|g| g.addr_start == 0x1000), "a covered callee is not a helper: {h:#?}");
    let ms = h.iter().find(|g| g.addr_start == 0x1004).expect("memset is a helper call");
    assert_eq!(ms.class.as_deref(), Some("memory-helper"));
    assert!(h.iter().any(|g| g.addr_start == 0x1008), "an unresolved call is still a finding: {g:#?}");
}

/// README.md:210 "indirect-branch: jump table, tail call or function pointer".
#[test]
fn calls_and_branches_through_a_register_are_indirect_branches() {
    let raw = vec![ri(0x1000, "blr", "x8"), ri(0x1004, "br", "x9")];
    let g = detect(&raw, &[Some(2), Some(3)], &[], &[], &no_call, &[]);
    let ib: Vec<u64> = g.iter().filter(|g| g.kind == GapKind::IndirectBranch).map(|g| g.addr_start).collect();
    assert_eq!(ib, vec![0x1000, 0x1004], "{g:#?}");
}

/// README.md:209 helper classes: "runtime-check (panics, bounds/stack checks
/// -- defensive, not dead code), arith-helper (compiler-rt/libgcc),
/// memory-helper (memcpy family), core-lib, libc, or project-uncovered
/// (project code the coverage run never instantiated)".
#[test]
fn helper_classes_follow_the_documented_categories() {
    use gaps::classify_helper as c;
    for n in ["core::panicking::panic", "core::panicking::panic_fmt", "__stack_chk_fail", "core::panicking::panic_const::panic_const_div_by_zero"] {
        assert_eq!(c(n, &[]), "runtime-check", "{n}");
    }
    for n in ["__udivti3", "__divdi3", "__muldi3", "__aeabi_ldivmod", "__ashldi3", "__floatsidf", "compiler_builtins::int::udiv::__udivti3"] {
        assert_eq!(c(n, &[]), "arith-helper", "{n}");
    }
    for n in ["memcpy", "memset", "memmove", "_memcpy", "compiler_builtins::mem::memset"] {
        assert_eq!(c(n, &[]), "memory-helper", "{n}");
    }
    for n in ["core::fmt::write", "<core::fmt::Arguments>::new_const", "alloc::alloc::exchange_malloc", "std::io::stdio::_print"] {
        assert_eq!(c(n, &[]), "core-lib", "{n}");
    }
    for n in ["printf", "puts", "strlen", "malloc", "_printf"] {
        assert_eq!(c(n, &[]), "libc", "{n}");
    }
    assert_eq!(c("fcs::limits::clamp", &["fcs".into()]), "project-uncovered");
    assert_eq!(c("<fcs::Mode>::engage", &["fcs".into()]), "project-uncovered");
}

/// README.md:209 "libc" is the C library. A project function that happens to
/// start with a libc name and an underscore is not the C library: telling a
/// reviewer `log_event` or `read_altitude` "needs library verification
/// credit" sends them to the wrong place.
#[test]
fn a_project_function_with_a_libc_prefix_is_not_libc() {
    use gaps::classify_helper as c;
    for n in ["log_event", "read_altitude", "write_config", "open_valve", "close_valve", "time_since_boot", "exit_degraded_mode", "free_slot_count"] {
        assert_ne!(c(n, &[]), "libc", "`{n}` is not a C library function");
    }
}

/// README.md:209 "project-uncovered (project code the coverage run never
/// instantiated)" against "runtime-check (panics, bounds/stack checks)" and
/// "core-lib". A function in a named project namespace is project code,
/// whatever words its name contains: an `abort_takeoff` or `assert_gear_down`
/// in the project is not a compiler-inserted check, and a project type whose
/// name starts with `u` or `i` is not the core library.
#[test]
fn project_code_is_project_uncovered_whatever_its_name_contains() {
    use gaps::classify_helper as c;
    let pre = ["fcs".to_string(), "uart".to_string(), "imu".to_string()];
    for n in ["fcs::abort_takeoff", "fcs::assert_gear_down", "<uart::Uart>::init", "<imu::Sensor as fcs::Read>::read"] {
        assert_eq!(c(n, &pre), "project-uncovered", "`{n}` is project code");
    }
}
