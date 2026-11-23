// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests, area D: the analysis baseline (DO-178C 4.4.2.c).
//!
//! Expected values come from README.md "Analysis baseline (DO-178C §4.4.2.c)"
//! (README.md:1103-1131) and docs/butterfly.html "Analysis baseline", not
//! from the implementation. Pure: hand-built evidence, no fixture, no git.

use butterfly_ingest::baseline::{compare, from_evidence};
use butterfly_model::*;
use std::collections::BTreeMap;

// ---------------------------------------------------------------- helpers

/// One instruction as `(mnemonic, operands)`.
type I<'a> = (&'a str, &'a str);

/// A function at `base`, one block per slice, each block falling through to
/// the next.
fn object(name: &str, base: u64, blocks: &[&[I]]) -> ObjectFn {
    let mut addr = base;
    let n = blocks.len();
    let mut out = Vec::new();
    for (bi, b) in blocks.iter().enumerate() {
        let start = addr;
        let instrs: Vec<Instr> = b
            .iter()
            .map(|(m, o)| {
                let i = Instr { addr, mnemonic: (*m).into(), operands: (*o).into(), ..Default::default() };
                addr += 4;
                i
            })
            .collect();
        let succs = if bi + 1 < n { vec![Edge { to: bi + 1, kind: EdgeKind::Fallthrough, ..Default::default() }] } else { vec![] };
        out.push(Block { id: bi, address: start, instrs, succs, ..Default::default() });
    }
    let total: usize = blocks.iter().map(|b| b.len()).sum();
    ObjectFn { symbol: name.into(), address: base, size: addr - base, instr_total: total, blocks: out, ..Default::default() }
}

/// Evidence with one source file holding the given functions as
/// `(name, line_start, line_end, object)`.
fn ev(text: &[&str], fns: Vec<(&str, u32, u32, Option<ObjectFn>)>) -> Evidence {
    let file = SourceFile {
        id: 0,
        path: "/src/a.c".into(),
        display: "a.c".into(),
        text: text.iter().map(|s| s.to_string()).collect(),
        lines: vec![LineCov { count: Some(1) }; text.len()],
        function_ids: (0..fns.len()).collect(),
        ..Default::default()
    };
    let functions = fns
        .into_iter()
        .enumerate()
        .map(|(i, (n, ls, le, o))| Function { id: i, name: n.into(), file_id: 0, line_start: ls, line_end: le, count: 1, object: o, ..Default::default() })
        .collect();
    let mut bi = BTreeMap::new();
    bi.insert("compiler".to_string(), "clang 18.1.0".to_string());
    bi.insert("flags_target".to_string(), "-O2".to_string());
    bi.insert("flags_instr".to_string(), "-O0 -fprofile-instr-generate -fcoverage-mapping".to_string());
    bi.insert("linker".to_string(), "ld64 1053".to_string());
    Evidence {
        schema: SCHEMA.into(),
        provenance: Provenance { arch: "aarch64".into(), object_format: "mach-o".into(), build_info: bi, ..Default::default() },
        files: vec![file],
        functions,
        ..Default::default()
    }
}

const SRC: &[&str] = &["int f(int x) {", "  return x + 1;", "}"];

fn f_at(base: u64, blocks: &[&[I]]) -> Evidence {
    ev(SRC, vec![("f", 1, 3, Some(object("f", base, blocks)))])
}

const BODY: &[&[I]] = &[&[("cmp", "w0, #0x0"), ("b.eq", "0x1010")], &[("add", "w0, w0, #0x1"), ("bl", "0x2000 <helper>")], &[("ret", "")]];

// ===================================================================== tests

/// README.md:1110-1113 -- "The baseline records compiler version, target and
/// instrumented flags, target arch/format, every `build-info.txt` key, the
/// set of source constructs in use, and a per-function fingerprint: source
/// SHA-256, constructs, instruction and block counts, and a SHA-256 of the
/// function's normalised object code"; docs/butterfly.html:600 adds "and
/// where the function was".
#[test]
fn b01_the_baseline_records_environment_and_per_function_fingerprint() {
    let mut e = f_at(0x1000, BODY);
    e.functions[0].constructs.insert("switch".into(), 1);
    let b = from_evidence(&e);
    assert_eq!(b.compiler, "clang 18.1.0");
    assert_eq!(b.flags_target, "-O2");
    assert!(b.flags_instr.contains("-fprofile-instr-generate"));
    assert_eq!((b.arch.as_str(), b.object_format.as_str()), ("aarch64", "mach-o"));
    assert_eq!(b.build_info.get("linker").map(String::as_str), Some("ld64 1053"), "every build-info key");
    assert_eq!(b.constructs, vec!["switch".to_string()]);
    assert_eq!(b.functions.len(), 1);
    let fp = b.functions.values().next().unwrap();
    assert_eq!(fp.name, "f");
    assert_eq!(fp.source_sha256.len(), 64, "a SHA-256");
    assert_eq!(fp.object_sha256.len(), 64, "a SHA-256 of the object code");
    assert_eq!((fp.instr_total, fp.blocks), (5, 3));
    assert_eq!(fp.constructs.get("switch"), Some(&1));
    assert_eq!((fp.file.as_str(), fp.line_start, fp.line_end), ("/src/a.c", 1, 3), "where the function was");
}

/// README.md:1121-1122 -- an ingest given the baseline "reports `matches` /
/// `changed`". The same evidence against its own baseline matches.
#[test]
fn b02_unchanged_evidence_matches() {
    let e = f_at(0x1000, BODY);
    let c = compare(&from_evidence(&e), &e, "b.json");
    assert_eq!(c.status, "matches", "{:?}", c.differences);
    assert!(c.differences.is_empty() && c.functions_source_changed.is_empty() && c.functions_object_changed.is_empty());
    assert_eq!(c.path, "b.json");
}

/// README.md:1116-1118 -- "Immediates are semantic and are fingerprinted; load
/// addresses are normalised, so a reproducible rebuild at a different address
/// compares equal." The whole function moves by 0x10000: its own branch
/// target moves with it and the named callee moved too.
#[test]
fn b03_a_rebuild_at_a_different_address_compares_equal() {
    let base = from_evidence(&f_at(0x1000, BODY));
    let moved: &[&[I]] = &[&[("cmp", "w0, #0x0"), ("b.eq", "0x11010")], &[("add", "w0, w0, #0x1"), ("bl", "0x12000 <helper>")], &[("ret", "")]];
    let c = compare(&base, &f_at(0x11000, moved), "b.json");
    assert_eq!(c.status, "matches", "relocated, otherwise identical code: {:?} {:?}", c.differences, c.functions_object_changed);
}

/// README.md:1116-1118 again, on x86-64. llvm-objdump prints a RIP-relative
/// access with the absolute address it resolves to as a trailing comment:
/// `leaq 0x2f3e(%rip), %rdi  # 0x404010 <table>`. The displacement is the
/// same in a rebuild that moved the whole image; only the load address in
/// the comment differs, and load addresses are normalised.
#[test]
fn b04_an_x86_rip_relative_reference_relocated_compares_equal() {
    let at = |base: u64, target: u64| {
        let ops = format!("0x2f3e(%rip), %rdi  # 0x{target:x} <table>");
        let o = object("f", base, &[&[("leaq", ops.as_str()), ("retq", "")]]);
        ev(SRC, vec![("f", 1, 3, Some(o))])
    };
    let base = from_evidence(&at(0x401000, 0x404010));
    let c = compare(&base, &at(0x501000, 0x504010), "b.json");
    assert_eq!(c.status, "matches", "same code, whole image relocated: {:?}", c.functions_object_changed);
}

/// README.md:1113-1116 -- "Counts alone are not a fingerprint": the same
/// instruction and block counts with different machine code is an object
/// change for unchanged source, which README.md:1121-1122 lists ("functions
/// whose object code changed for *unchanged* source").
#[test]
fn b05_same_counts_different_code_is_an_object_change_for_unchanged_source() {
    let base = from_evidence(&f_at(0x1000, BODY));
    let other: &[&[I]] = &[&[("cmp", "w0, #0x0"), ("b.eq", "0x1010")], &[("sub", "w0, w0, #0x1"), ("bl", "0x2000 <helper>")], &[("ret", "")]];
    let c = compare(&base, &f_at(0x1000, other), "b.json");
    assert_eq!(c.status, "changed");
    assert_eq!(c.functions_object_changed, vec!["f".to_string()]);
    assert!(c.functions_source_changed.is_empty());
    // an immediate alone is semantic too
    let imm: &[&[I]] = &[&[("cmp", "w0, #0x1"), ("b.eq", "0x1010")], &[("add", "w0, w0, #0x1"), ("bl", "0x2000 <helper>")], &[("ret", "")]];
    assert_eq!(compare(&base, &f_at(0x1000, imm), "b.json").functions_object_changed, vec!["f".to_string()]);
    // so is the callee's identity
    let callee: &[&[I]] = &[&[("cmp", "w0, #0x0"), ("b.eq", "0x1010")], &[("add", "w0, w0, #0x1"), ("bl", "0x2000 <other>")], &[("ret", "")]];
    assert_eq!(compare(&base, &f_at(0x1000, callee), "b.json").functions_object_changed, vec!["f".to_string()]);
}

/// README.md:1121-1122 -- a function whose source changed is reported, and the
/// status is `changed`.
#[test]
fn b06_a_source_change_is_reported_and_invalidates() {
    let base = from_evidence(&f_at(0x1000, BODY));
    let e = ev(&["int f(int x) {", "  return x + 2;", "}"], vec![("f", 1, 3, Some(object("f", 0x1000, BODY)))]);
    let c = compare(&base, &e, "b.json");
    assert_eq!(c.status, "changed");
    assert_eq!(c.functions_source_changed, vec!["f".to_string()]);
    assert!(c.functions_object_changed.is_empty(), "source changed: not reported as a pure compiler effect");
}

/// README.md:1120-1122 and docs/butterfly.html:600-601 -- "A new compiler, new
/// options or new source constructs invalidate earlier coverage and
/// traceability analyses": compiler, flags, target and new constructs are
/// each reported, and each makes the status `changed`.
#[test]
fn b07_compiler_flags_target_and_new_constructs_each_invalidate() {
    let e0 = f_at(0x1000, BODY);
    let base = from_evidence(&e0);
    let check = |mutate: &dyn Fn(&mut Evidence), kind: &str| {
        let mut e = e0.clone();
        mutate(&mut e);
        let c = compare(&base, &e, "b.json");
        assert_eq!(c.status, "changed", "{kind}: {:?}", c.differences);
        assert!(c.differences.iter().any(|d| d.kind == kind && d.invalidates), "{kind}: {:?}", c.differences);
    };
    check(
        &|e| {
            e.provenance.build_info.insert("compiler".into(), "clang 19.1.0".into());
        },
        "compiler",
    );
    check(
        &|e| {
            e.provenance.build_info.insert("flags_target".into(), "-Os".into());
        },
        "flags",
    );
    check(&|e| e.provenance.arch = "x86_64".into(), "arch");
    check(&|e| e.provenance.object_format = "elf".into(), "arch");
    check(
        &|e| {
            e.functions[0].constructs.insert("goto".into(), 1);
        },
        "constructs",
    );
    let mut e = e0.clone();
    e.functions[0].constructs.insert("goto".into(), 1);
    assert_eq!(compare(&base, &e, "b.json").new_constructs, vec!["goto".to_string()]);
}

/// README.md:1110-1111 ("instrumented flags", "every build-info.txt key") and
/// README.md:1120-1122: a change in the instrumented build's flags, a changed
/// build-info key and a new one are all recorded, so all are differences --
/// the comparison never says `matches` over them.
#[test]
fn b08_instrumented_flags_and_build_info_keys_are_compared() {
    let e0 = f_at(0x1000, BODY);
    let base = from_evidence(&e0);
    let mut e = e0.clone();
    e.provenance.build_info.insert("flags_instr".into(), "-O1 -fprofile-instr-generate -fcoverage-mapping".into());
    let c = compare(&base, &e, "b.json");
    assert_ne!(c.status, "matches", "instrumented flags changed: {:?}", c.differences);
    assert!(c.differences.iter().any(|d| d.kind == "flags"), "{:?}", c.differences);

    let mut e = e0.clone();
    e.provenance.build_info.insert("linker".into(), "ld64 1100".into());
    let c = compare(&base, &e, "b.json");
    assert_eq!(c.status, "changed", "a changed toolchain key: {:?}", c.differences);
    assert!(c.differences.iter().any(|d| d.detail.contains("linker")), "{:?}", c.differences);

    let mut e = e0.clone();
    e.provenance.build_info.insert("assembler".into(), "as 2.42".into());
    let c = compare(&base, &e, "b.json");
    assert_ne!(c.status, "matches", "a build-info key the baseline did not have: {:?}", c.differences);
    assert!(c.differences.iter().any(|d| d.detail.contains("assembler")), "{:?}", c.differences);
}

/// README.md:1121-1122 -- "functions whose source changed": a function that
/// moved down the file (a line inserted above it) *and* changed its body is
/// still a source change, not an add plus a remove.
#[test]
fn b09_a_moved_and_edited_function_is_a_source_change() {
    let base = from_evidence(&f_at(0x1000, BODY));
    let e = ev(&["// new header comment", "int f(int x) {", "  return x + 7;", "}"], vec![("f", 2, 4, Some(object("f", 0x1000, BODY)))]);
    let c = compare(&base, &e, "b.json");
    assert_eq!(c.status, "changed");
    assert_eq!(c.functions_source_changed, vec!["f".to_string()]);
    assert!(c.functions_added.is_empty() && c.functions_removed.is_empty(), "{:?} {:?}", c.functions_added, c.functions_removed);
}

/// README.md:1121-1122 -- the object-code comparison holds for a moved
/// function too: same source, different code is a compiler/option effect.
#[test]
fn b10_a_moved_function_with_different_object_code_is_an_object_change() {
    let base = from_evidence(&f_at(0x1000, BODY));
    let other: &[&[I]] = &[&[("cmp", "w0, #0x0"), ("b.ne", "0x1010")], &[("add", "w0, w0, #0x1"), ("bl", "0x2000 <helper>")], &[("ret", "")]];
    let e = ev(&["", "int f(int x) {", "  return x + 1;", "}"], vec![("f", 2, 4, Some(object("f", 0x1000, other)))]);
    let c = compare(&base, &e, "b.json");
    assert_eq!(c.status, "changed");
    assert_eq!(c.functions_object_changed, vec!["f".to_string()]);
    assert!(c.functions_source_changed.is_empty());
}

/// docs/butterfly.html:600 ("where the function was") and README.md:1304-1307:
/// a function added since the baseline and one removed since are both named.
#[test]
fn b11_added_and_removed_functions_are_named() {
    let two = ev(&["int f(int x) {", "  return x + 1;", "}", "int g(void) {", "  return 0;", "}"], vec![("f", 1, 3, Some(object("f", 0x1000, BODY))), ("g", 4, 6, None)]);
    let base = from_evidence(&two);
    let only_f = ev(&["int f(int x) {", "  return x + 1;", "}", "int h(void) {", "  return 0;", "}"], vec![("f", 1, 3, Some(object("f", 0x1000, BODY))), ("h", 4, 6, None)]);
    let c = compare(&base, &only_f, "b.json");
    assert_eq!(c.functions_removed, vec!["g".to_string()]);
    assert_eq!(c.functions_added, vec!["h".to_string()]);
    assert_ne!(c.status, "matches");
}

/// A baseline is kept "under CM" as a file (README.md:1106): it round-trips
/// through its JSON form and compares the same afterwards; a document that
/// is not a baseline does not read as one.
#[test]
fn b12_a_baseline_survives_its_file_form() {
    let e = f_at(0x1000, BODY);
    let b = from_evidence(&e);
    let text = serde_json::to_string(&b).unwrap();
    let back: Baseline = serde_json::from_str(&text).unwrap();
    assert_eq!(back.schema, BASELINE_SCHEMA);
    assert_eq!(compare(&back, &e, "b.json").status, "matches");
    assert!(serde_json::from_str::<Baseline>("{\"schema\": 3}").is_err());
}
