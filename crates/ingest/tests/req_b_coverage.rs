// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for source coverage and MC/DC (area B).
//!
//! Every input is a hand-written `llvm-cov export` (or gcov JSON) and its
//! source, ingested with no object file, so the expected numbers can be
//! worked out by hand from the documentation:
//!
//! * README.md "Inputs" (~L276-281), "Rust" (~L460-490), "MC/DC does not come
//!   from GCC" (~L929-946), "A line can be partly covered" (~L1078-1102);
//! * docs/butterfly.html "What it will not claim" (~L807-820);
//! * llvm-cov's own line-count rule, which README.md:471-473 says butterfly
//!   matches ("summing counts exactly as llvm-cov already does for the file's
//!   line totals"): a line's count is the wrapped segment's count, raised by
//!   any region entry that starts on the line; it is mapped only if one of
//!   those has a count.

use butterfly_ingest::IngestOptions;
use butterfly_model::*;
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let d = butterfly_testsupport::scratch_dir(&format!("reqb-{tag}"));
    d.canonicalize().unwrap()
}

const W_C: &str = "int f(int v) {
  if (v > 3) {
    v = 1;
  }
  return v;
}

int g(int a, int b) {
  if (a || b) return 1;
  return 0;
}
";

/// The merged file segments for W_C. Worked by hand with llvm-cov's rule:
/// L1 4, L2 4 (wrapped 4, the then-region entry 0 does not lower it), L3 0,
/// L4 0 (only a gap segment starts there), L5 4, L6 4, L7 unmapped,
/// L8 2, L9 2, L10 1, L11 1. So 10 lines, 8 covered.
const W_SEGMENTS: &str = r#"[[1,14,4,true,true,false],[2,14,0,true,true,false],[4,4,4,true,false,true],[5,3,4,true,true,false],[6,2,0,false,false,false],
  [8,21,2,true,true,false],[9,15,1,true,true,false],[9,24,1,true,false,true],[10,3,1,true,true,false],[11,2,0,false,false,false]]"#;

fn f_record(path: &str) -> String {
    format!(r#"{{"name":"f","count":4,"regions":[[1,14,6,2,4,0,0,0],[2,14,4,4,0,0,0,0]],"branches":[[2,7,2,12,0,4,0,0,4]],"mcdc_records":[],"filenames":[{path:?}]}}"#)
}

/// One record of `g`: count, the two condition branch regions (`a`, `b`)
/// as (true, false) counts, and the MC/DC record's tail. With an MC/DC record
/// the conditions are MC/DC branch regions (kind 6), as clang exports them;
/// without, plain branch regions (kind 4).
fn g_record(path: &str, name: &str, count: u64, a: (u64, u64), b: (u64, u64), mcdc: &str) -> String {
    let kind = if mcdc.is_empty() { 4 } else { 6 };
    format!(
        r#"{{"name":{name:?},"count":{count},"regions":[[8,21,11,2,{count},0,0,0]],"branches":[[9,7,9,8,{},{},0,0,{kind}],[9,12,9,13,{},{},0,0,{kind}]],"mcdc_records":[{mcdc}],"filenames":[{path:?}]}}"#,
        a.0, a.1, b.0, b.1
    )
}

/// LLVM 19+ MC/DC layout for `a || b` on line 9.
fn mcdc19(t: u64, f: u64, covered: [bool; 2], vectors: &[(Option<bool>, Option<bool>, bool, bool)]) -> String {
    let v: Vec<String> = vectors
        .iter()
        .map(|(a, b, executed, result)| {
            let c = |x: &Option<bool>| x.map(|b| b.to_string()).unwrap_or_else(|| "null".into());
            format!(r#"{{"conditions":[{},{}],"executed":{executed},"result":{result}}}"#, c(a), c(b))
        })
        .collect();
    format!("[9,7,9,13,{t},{f},0,0,5,[{},{}],[{}]]", covered[0], covered[1], v.join(","))
}

fn export(path: &str, segments: &str, functions: &[String]) -> String {
    format!(
        r#"{{"version":"2.0.1","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{path:?},"segments":{segments},"branches":[],"summary":{{}}}}],"functions":[{}],"totals":{{}}}}]}}"#,
        functions.join(",")
    )
}

fn ingest(dir: &Path, cov: &str, tests: &[(&str, String)]) -> anyhow::Result<Evidence> {
    fs::write(dir.join("coverage.json"), cov).unwrap();
    let mut o = IngestOptions { coverage_json: dir.join("coverage.json"), source_root: Some(dir.to_path_buf()), ..Default::default() };
    for (name, text) in tests {
        let p = dir.join(format!("{name}.json"));
        fs::write(&p, text).unwrap();
        o.tests.push(p);
    }
    butterfly_ingest::ingest(&o)
}

/// W_C with one record of `g`, whose vectors are given.
fn one_g(tag: &str, g: impl Fn(&str) -> String) -> (PathBuf, Evidence) {
    let d = scratch(tag);
    fs::write(d.join("w.c"), W_C).unwrap();
    let p = d.join("w.c").display().to_string();
    let ev = ingest(&d, &export(&p, W_SEGMENTS, &[f_record(&p), g(&p)]), &[]).expect("ingest");
    (d, ev)
}

fn func<'a>(ev: &'a Evidence, name: &str) -> &'a Function {
    ev.functions.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no function {name}"))
}

fn caveats(ev: &Evidence) -> String {
    ev.provenance.caveats.join(" | ")
}

/// The pair, if any, as the two vectors it names (numbered from 1, as the
/// truth table numbers its rows).
fn pair_vectors(d: &Decision, i: usize) -> Option<(&TestVector, &TestVector)> {
    d.conditions[i].pair.map(|(a, b)| (&d.vectors[a - 1], &d.vectors[b - 1]))
}

/// README.md:938-941: "an independence pair ... two runs differing in that
/// condition alone and in the decision's outcome". Checks one reported pair
/// against that definition.
fn assert_is_independence_pair(d: &Decision, i: usize) {
    let (x, y) = pair_vectors(d, i).expect("a pair");
    assert!(x.executed && y.executed, "both are runs");
    assert_ne!(x.result, y.result, "the outcome differs");
    assert!(x.values[i].is_some() && y.values[i].is_some() && x.values[i] != y.values[i], "condition {i} differs");
    for j in 0..x.values.len() {
        if j != i {
            assert!(x.values[j] == y.values[j] || x.values[j].is_none() || y.values[j].is_none(), "condition {j} also differs");
        }
    }
}

// ---------------------------------------------------------------------------
// Line coverage
// ---------------------------------------------------------------------------

/// README.md:471-473 (llvm-cov's line totals), README.md:213 (`not-instrumented`
/// is a line with no count) and the worked counts above W_SEGMENTS.
#[test]
fn line_counts_follow_llvm_covs_rule_and_an_unmapped_line_has_none() {
    let d = scratch("lines");
    fs::write(d.join("w.c"), W_C).unwrap();
    let p = d.join("w.c").display().to_string();
    let g = g_record(&p, "g", 2, (0, 2), (1, 1), "");
    let ev = ingest(&d, &export(&p, W_SEGMENTS, &[f_record(&p), g]), &[]).expect("ingest");
    let counts: Vec<Option<u64>> = ev.files[0].lines.iter().map(|l| l.count).collect();
    assert_eq!(&counts[..11], &[Some(4), Some(4), Some(0), Some(0), Some(4), Some(4), None, Some(2), Some(2), Some(1), Some(1)]);
    assert_eq!((ev.files[0].lines_total, ev.files[0].lines_covered), (10, 8));
    assert_eq!((ev.summary.lines_total, ev.summary.lines_covered), (10, 8));
    let f = func(&ev, "f");
    assert_eq!((f.lines_total, f.lines_covered), (6, 4));
    let _ = fs::remove_dir_all(d);
}

/// README.md:1083-1100 "A line can be partly covered ... Line coverage counts
/// it as executed ... **The numbers do not change.** `linesCovered` is what
/// was measured". Line 2 of `f` holds a decision whose true outcome never ran:
/// it still counts as a covered line, and the untaken outcome is reported as
/// an untaken outcome.
#[test]
fn a_line_with_an_untaken_outcome_still_counts_as_covered() {
    let (d, ev) = one_g("partial", |p| g_record(p, "g", 0, (0, 0), (0, 0), ""));
    let f = func(&ev, "f");
    assert!(ev.files[0].lines[1].count.unwrap() > 0, "line 2 was reached");
    assert_eq!(f.lines_covered, 4, "and it is counted covered: the figure is what was measured");
    let b = f.branches.iter().find(|b| b.line == 2).expect("the decision on line 2");
    assert_eq!((b.true_count, b.false_count), (0, 4), "its true outcome never ran");
    // branch outcome totals: f's two outcomes (one taken) and g's four (none)
    assert_eq!((ev.summary.branches_total, ev.summary.branches_covered), (6, 1));
    let _ = fs::remove_dir_all(d);
}

/// README.md:277 "`llvm-cov export -format=text` JSON" and README.md:279-281
/// "butterfly reads whichever the export says it is": something that is
/// neither is refused, not read as an empty export.
#[test]
fn an_export_that_is_neither_llvm_cov_nor_gcov_is_refused() {
    let d = scratch("notcov");
    for text in [r#"{"type":"something.else","data":[]}"#, "not json at all", r#"{"type":"llvm.coverage.json.export","data":[]}"#] {
        assert!(ingest(&d, text, &[]).is_err(), "accepted: {text}");
    }
    let _ = fs::remove_dir_all(d);
}

// ---------------------------------------------------------------------------
// MC/DC
// ---------------------------------------------------------------------------

/// README.md:938-941 independence pairs, on one record (LLVM 19 layout).
/// `a || b` run as (F,T)->T, (F,F)->F and (T,-)->T: `b` is shown by the first
/// two; `a` by (T,-)->T against (F,F)->F, where `b` was not evaluated.
#[test]
fn every_condition_with_a_pair_is_covered_and_its_pair_satisfies_the_definition() {
    let m = mcdc19(2, 1, [true, true], &[(Some(false), Some(true), true, true), (Some(false), Some(false), true, false), (Some(true), None, true, true)]);
    let (d, ev) = one_g("pair", |p| g_record(p, "g", 3, (1, 2), (1, 1), &m));
    let g = func(&ev, "g");
    assert_eq!(g.decisions.len(), 1);
    let dec = &g.decisions[0];
    assert_eq!(dec.conditions.len(), 2);
    assert_eq!(dec.vectors.len(), 3, "the truth table carries the executed vectors");
    for i in 0..2 {
        assert!(dec.conditions[i].independence_covered, "condition {i}");
        assert_is_independence_pair(dec, i);
    }
    assert!(dec.mcdc_covered);
    assert_eq!((ev.summary.conditions_total, ev.summary.conditions_independence_covered), (2, 2));
    let _ = fs::remove_dir_all(d);
}

/// README.md:938-941, the negative: two runs that differ in the condition but
/// not in the outcome are not a pair, and no pair is invented.
#[test]
fn no_pair_is_reported_where_the_outcome_did_not_change() {
    let m = mcdc19(2, 0, [false, false], &[(Some(false), Some(true), true, true), (Some(true), None, true, true)]);
    let (d, ev) = one_g("nopair", |p| g_record(p, "g", 2, (1, 1), (1, 0), &m));
    let dec = &func(&ev, "g").decisions[0];
    for c in &dec.conditions {
        assert!(!c.independence_covered, "{c:?}");
        assert!(c.pair.is_none(), "{c:?}");
    }
    assert!(!dec.mcdc_covered);
    let _ = fs::remove_dir_all(d);
}

/// docs/butterfly.html:816 "Older LLVM. clang 18 exports MC/DC records without
/// outcome counts or test vectors. Both layouts are read; the older one yields
/// an empty truth table and a caveat", and support/features.toml:443
/// "Independence per condition is still exact."
#[test]
fn the_llvm_18_layout_yields_an_empty_truth_table_a_caveat_and_exact_independence() {
    let legacy = "[9,7,9,13,0,5,[false,true]]";
    let (d, ev) = one_g("llvm18", |p| g_record(p, "g", 2, (0, 2), (1, 1), legacy));
    let dec = &func(&ev, "g").decisions[0];
    assert!(dec.vectors.is_empty(), "an empty truth table");
    assert_eq!((dec.true_count, dec.false_count), (0, 0), "no outcome counts were exported");
    assert!(!dec.conditions[0].independence_covered);
    assert!(dec.conditions[1].independence_covered, "independence per condition is exact");
    assert!(caveats(&ev).contains("LLVM 18"), "{}", caveats(&ev));
    let _ = fs::remove_dir_all(d);
}

/// docs/butterfly.html:811 "Rust has no MC/DC ... So nothing invents pairs":
/// a Rust export with branch regions and no MC/DC records yields no
/// decisions and no conditions.
#[test]
fn a_rust_export_with_branch_regions_has_no_invented_mcdc() {
    let d = scratch("rust");
    let src = "fn f(a: bool, b: bool) -> u32 {\n    if a || b { 1 } else { 0 }\n}\n";
    fs::write(d.join("lib.rs"), src).unwrap();
    let p = d.join("lib.rs").display().to_string();
    let rec = format!(r#"{{"name":"_RNvCs1234_3lib1f","count":2,"regions":[[1,32,3,2,2,0,0,0]],"branches":[[2,8,2,9,1,1,0,0,4],[2,13,2,14,0,1,0,0,4]],"filenames":[{p:?}]}}"#);
    let ev = ingest(&d, &export(&p, "[[1,32,2,true,true,false],[3,2,0,false,false,false]]", &[rec]), &[]).expect("ingest");
    let f = &ev.functions[0];
    assert_eq!(f.branches.len(), 2, "the branch regions are kept");
    assert!(f.decisions.is_empty(), "no decision is synthesised");
    assert_eq!((ev.summary.decisions_total, ev.summary.conditions_total), (0, 0));
    assert_eq!(f.name, "lib::f", "matched by demangled name (README.md:488-489)");
    let _ = fs::remove_dir_all(d);
}

// ---------------------------------------------------------------------------
// Instantiations
// ---------------------------------------------------------------------------

/// README.md:468-472 "butterfly merges records that share a name, file and
/// line range, summing counts exactly as llvm-cov already does ... and
/// reports how many were merged". Two records of `g`: one ran (F,T)->T, the
/// other (F,F)->F. Summed: count 2, `a` 0 true / 2 false, `b` 1/1, decision
/// 1/1. Together the two runs are the independence pair for `b`
/// (README.md:938-941), though neither record alone had one.
#[test]
fn instantiations_are_merged_with_summed_counts_and_their_runs_pair_up() {
    let d = scratch("merge");
    fs::write(d.join("w.c"), W_C).unwrap();
    let p = d.join("w.c").display().to_string();
    let g1 = g_record(&p, "g", 1, (0, 1), (1, 0), &mcdc19(1, 0, [false, false], &[(Some(false), Some(true), true, true)]));
    let g2 = g_record(&p, "g", 1, (0, 1), (0, 1), &mcdc19(0, 1, [false, false], &[(Some(false), Some(false), true, false)]));
    let ev = ingest(&d, &export(&p, W_SEGMENTS, &[f_record(&p), g1, g2]), &[]).expect("ingest");

    let gs: Vec<_> = ev.functions.iter().filter(|f| f.name == "g").collect();
    assert_eq!(gs.len(), 1, "one source function");
    let g = gs[0];
    assert_eq!(g.instantiations, 2);
    assert_eq!(g.count, 2);
    let a = g.branches.iter().find(|b| b.col == 7).unwrap();
    let b = g.branches.iter().find(|b| b.col == 12).unwrap();
    assert_eq!((a.true_count, a.false_count), (0, 2));
    assert_eq!((b.true_count, b.false_count), (1, 1));
    let dec = &g.decisions[0];
    assert_eq!((dec.true_count, dec.false_count), (1, 1));
    assert!(!dec.conditions[0].independence_covered, "`a` was never true");
    assert!(dec.conditions[1].independence_covered, "(F,T)->T and (F,F)->F show `b`");
    assert_is_independence_pair(dec, 1);
    assert_eq!(ev.summary.functions, 2);
    assert!(caveats(&ev).contains("merged"), "the merge is reported: {}", caveats(&ev));
    let _ = fs::remove_dir_all(d);
}

/// The same requirement where one record also lists the other's vector as a
/// slot it never executed (the MC/DC layout keeps a placeholder for each
/// combination). Whether a run happened is a fact about the campaign: the
/// second record executed (F,F)->F, so the merged decision has that run and
/// `b` its pair, whichever record happened to be read first.
#[test]
fn an_executed_run_is_not_lost_behind_another_records_unexecuted_slot() {
    let d = scratch("merge-slot");
    fs::write(d.join("w.c"), W_C).unwrap();
    let p = d.join("w.c").display().to_string();
    let g1 = g_record(&p, "g", 1, (0, 1), (1, 0), &mcdc19(1, 0, [false, false], &[(Some(false), Some(true), true, true), (Some(false), Some(false), false, false)]));
    let g2 = g_record(&p, "g", 1, (0, 1), (0, 1), &mcdc19(0, 1, [false, false], &[(Some(false), Some(false), true, false)]));
    let ev = ingest(&d, &export(&p, W_SEGMENTS, &[f_record(&p), g1, g2]), &[]).expect("ingest");
    let dec = &func(&ev, "g").decisions[0];
    assert!(dec.vectors.iter().any(|v| v.executed && v.values == vec![Some(false), Some(false)] && !v.result), "the executed (F,F)->F run is kept: {:?}", dec.vectors);
    assert!(dec.conditions[1].independence_covered, "and `b` has its pair");
    let _ = fs::remove_dir_all(d);
}

/// README.md:470 merges records that share a name, file AND line range: two
/// records with one name at different ranges are two functions.
#[test]
fn records_with_one_name_at_different_ranges_are_not_merged() {
    let d = scratch("nomerge");
    fs::write(d.join("w.c"), W_C).unwrap();
    let p = d.join("w.c").display().to_string();
    let other = format!(r#"{{"name":"f","count":2,"regions":[[8,21,11,2,2,0,0,0]],"branches":[],"filenames":[{p:?}]}}"#);
    let ev = ingest(&d, &export(&p, W_SEGMENTS, &[f_record(&p), other]), &[]).expect("ingest");
    let fs_: Vec<_> = ev.functions.iter().filter(|f| f.name == "f").collect();
    assert_eq!(fs_.len(), 2, "{:?}", ev.functions.iter().map(|f| (&f.name, f.line_start)).collect::<Vec<_>>());
    assert!(fs_.iter().all(|f| f.instantiations == 1));
    assert!(!caveats(&ev).contains("merged into"), "{}", caveats(&ev));
    let _ = fs::remove_dir_all(d);
}

// ---------------------------------------------------------------------------
// GCC
// ---------------------------------------------------------------------------

fn gcov_doc(cwd: &Path, file: &str, fn_count: u64, lines: &str) -> String {
    format!(
        r#"{{"format_version":"2","gcc_version":"16.2.0","current_working_directory":{:?},"files":[{{"file":{file:?},"functions":[{{"name":"hf","start_line":1,"start_column":1,"end_line":4,"end_column":1,"execution_count":{fn_count}}}],"lines":[{lines}]}}]}}"#,
        cwd.display().to_string()
    )
}

const HF: &str = "static inline int hf(int v) {\n  if (v) return 1;\n  return 0;\n}\n";

/// README.md:279-281 "butterfly reads whichever the export says it is";
/// README.md:941-946 the absence of MC/DC "says this itself, in
/// `provenance.caveats`"; README.md:959-962 a gcov decision's outcomes are
/// keyed, not named true/false.
#[test]
fn a_gcov_export_is_read_without_a_flag_and_says_what_it_cannot_carry() {
    let d = scratch("gcov");
    fs::write(d.join("h.h"), HF).unwrap();
    let lines = r#"{"line_number":1,"count":2},{"line_number":2,"count":2,"branches":[{"count":2,"throw":false,"fallthrough":true,"source_block_id":2,"destination_block_id":3},{"count":0,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4}]},{"line_number":3,"count":0}"#;
    let ev = ingest(&d, &gcov_doc(&d, "h.h", 2, lines), &[]).expect("ingest");
    let f = func(&ev, "hf");
    assert_eq!(f.branches.len(), 1);
    assert!(!f.branches[0].direction_known);
    assert_eq!(f.branches[0].outcome_keys(), ["A", "B"]);
    assert!(f.decisions.is_empty(), "MC/DC is not synthesised");
    assert!(caveats(&ev).contains("MC/DC"), "{}", caveats(&ev));
    assert_eq!((ev.summary.lines_total, ev.summary.lines_covered), (3, 2));
    let _ = fs::remove_dir_all(d);
}

/// README.md:925-927 "without `-b` the export contains no branch records at
/// all and the evidence would carry no decision coverage. butterfly says so
/// in the provenance caveats rather than letting it pass silently."
#[test]
fn a_gcov_export_without_branch_records_says_so() {
    let d = scratch("gcov-nob");
    fs::write(d.join("h.h"), HF).unwrap();
    let lines = r#"{"line_number":1,"count":2},{"line_number":2,"count":2},{"line_number":3,"count":0}"#;
    let ev = ingest(&d, &gcov_doc(&d, "h.h", 2, lines), &[]).expect("ingest");
    assert!(caveats(&ev).contains("-b"), "{}", caveats(&ev));
    let _ = fs::remove_dir_all(d);
}

/// Compile `c` with the host's C compiler, debug info and no optimisation,
/// into `dir/<name>.o`, or `None` where there is no compiler.
fn compiled(dir: &Path, name: &str, c: &str) -> Option<PathBuf> {
    compiled_at(dir, name, c, "-O0")
}

/// [`compiled`] at optimisation level `opt`.
fn compiled_at(dir: &Path, name: &str, c: &str, opt: &str) -> Option<PathBuf> {
    let src = dir.join(format!("{name}.c"));
    let obj = dir.join(format!("{name}.o"));
    fs::write(&src, c).unwrap();
    let ok = std::process::Command::new("cc").args(["-c", "-g", opt]).arg(&src).arg("-o").arg(&obj).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: no working `cc` on this host");
        return None;
    }
    Some(obj)
}

fn ingest_with_object(dir: &Path, cov: &str, obj: &Path) -> Evidence {
    fs::write(dir.join("coverage.json"), cov).unwrap();
    let o = IngestOptions { coverage_json: dir.join("coverage.json"), objects: vec![obj.to_path_buf()], source_root: Some(dir.to_path_buf()), ..Default::default() };
    butterfly_ingest::ingest(&o).expect("ingest")
}

/// gcov writes `"branches": []` on every line whether or not `-b` was
/// passed, so the export alone cannot tell the two apart.
const NO_RECORDS: &str = r#"{"line_number":1,"count":2,"branches":[]},{"line_number":2,"count":2,"branches":[]},{"line_number":3,"count":2,"branches":[]}"#;

/// README "`gcov -b` matters": without `-b` the export contains no branch
/// records at all, and butterfly says so in the provenance caveats. Decided in
/// #325 (ambiguity 7) and narrowed by its review: object code with no
/// conditional branch makes the *cause* ambiguous -- a straight-line program
/// exported with `-b` has no records to give -- so the instruction to re-run
/// with `-b` is dropped. It does not prove the source has no decision, so the
/// caveat that no branch records and no decision outcomes were supplied stays.
#[test]
fn a_straight_line_gcov_program_is_not_told_to_re_run_with_b() {
    let d = scratch("gcov-straight");
    let Some(obj) = compiled(&d, "s", "int sl(int v) {\n  int w = v * 3;\n  return w + 1;\n}\n") else { return };
    let ev = ingest_with_object(&d, &gcov_doc(&d, "s.c", 2, NO_RECORDS).replace(r#""name":"hf""#, r#""name":"sl""#), &obj);
    let f = func(&ev, "sl");
    assert!(f.object.is_some(), "the function's object code was found");
    assert_eq!(f.complexity.object_conditional_branches, 0, "straight-line code");
    assert!(!caveats(&ev).contains("re-run gcov with -b"), "{}", caveats(&ev));
    // the records are still absent, and that is still said
    assert!(caveats(&ev).contains("no branch records"), "{}", caveats(&ev));
    assert!(caveats(&ev).contains("decision outcomes were not supplied"), "{}", caveats(&ev));
    // the export's other limits are still said
    assert!(caveats(&ev).contains("MC/DC"), "{}", caveats(&ev));
    let _ = fs::remove_dir_all(d);
}

/// #335 review (P2): no conditional jump is not no source decision. A source
/// `?:` lowered to a conditional select (`csel` on AArch64, `cmov` on x86)
/// is a decision whose code has no conditional branch, and `cfg::classify`
/// counts none. A gcov export with no branch records over that body has
/// supplied no outcome for the decision, and the caveat must say so.
#[test]
fn a_conditional_select_body_with_no_branch_records_keeps_the_caveat() {
    let d = scratch("gcov-csel");
    let src = "int mn(int a, int b) {\n  return a < b ? a : b;\n}\n";
    let Some(obj) = compiled_at(&d, "m", src, "-O2") else { return };
    let doc = gcov_doc(&d, "m.c", 2, NO_RECORDS).replace(r#""name":"hf""#, r#""name":"mn""#).replace(r#""end_line":4"#, r#""end_line":3"#);
    let ev = ingest_with_object(&d, &doc, &obj);
    let f = func(&ev, "mn");
    assert!(f.object.is_some(), "the function's object code was found");
    if f.complexity.object_conditional_branches != 0 {
        // A pass must mean the case was asserted: under
        // BUTTERFLY_REQUIRE_FIXTURES (CI) this fails rather than skipping, and
        // otherwise prints the `skipping:` line CI's guard looks for.
        let _ = fs::remove_dir_all(&d);
        butterfly_testsupport::fixture_missing("a compiler that lowers `a < b ? a : b` at -O2 to a conditional select, not a branch");
        return;
    }
    let c = caveats(&ev);
    assert!(c.contains("no branch records"), "{c}");
    assert!(c.contains("decision outcomes were not supplied"), "{c}");
    assert!(c.contains("unknown"), "completeness is not claimed: {c}");
    let _ = fs::remove_dir_all(d);
}

/// The other half of the same decision: where the object code *does* branch
/// and the export has no record of it, the records are missing and the
/// advice stands.
#[test]
fn a_branching_gcov_program_with_no_branch_records_is_still_told() {
    let d = scratch("gcov-branchy");
    let Some(obj) = compiled(&d, "b", "int hf(int v) {\n  if (v) return 1;\n  return 0;\n}\n") else { return };
    let ev = ingest_with_object(&d, &gcov_doc(&d, "b.c", 2, NO_RECORDS), &obj);
    let f = func(&ev, "hf");
    assert!(f.object.is_some(), "the function's object code was found");
    assert!(f.complexity.object_conditional_branches > 0, "the object code branches");
    assert!(caveats(&ev).contains("re-run gcov with -b"), "{}", caveats(&ev));
    let _ = fs::remove_dir_all(d);
}

/// README.md:933-941 "`-fcondition-coverage` records that each condition was
/// taken both ways. MC/DC requires an independence pair ... GCC emits nothing
/// those could be built from, so butterfly does not synthesise them".
#[test]
fn gcc_condition_coverage_is_not_turned_into_mcdc() {
    let d = scratch("gcov-cond");
    fs::write(d.join("h.h"), HF).unwrap();
    let lines = r#"{"line_number":1,"count":2},{"line_number":2,"count":2,"conditions":[{"count":2,"covered":2,"not_covered_true":[],"not_covered_false":[]}],"branches":[{"count":1,"throw":false,"fallthrough":true,"source_block_id":2,"destination_block_id":3},{"count":1,"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":4}]},{"line_number":3,"count":1}"#;
    let ev = ingest(&d, &gcov_doc(&d, "h.h", 2, lines), &[]).expect("ingest");
    assert!(func(&ev, "hf").decisions.is_empty());
    assert_eq!((ev.summary.conditions_total, ev.summary.conditions_independence_covered), (0, 0));
    assert!(caveats(&ev).contains("condition coverage"), "{}", caveats(&ev));
    let _ = fs::remove_dir_all(d);
}

/// README.md:470-472 for GCC: a header's inline function compiled into two
/// units arrives as two documents (README.md:913-918 runs gcov per .gcda). It
/// is one source function and one file, and its counts are the sum: line 2
/// ran 2 + 0 times, and the function 2 + 0.
#[test]
fn one_header_in_two_gcov_units_is_one_file_and_one_function() {
    let d = scratch("gcov-two");
    fs::write(d.join("h.h"), HF).unwrap();
    let ran = r#"{"line_number":1,"count":2},{"line_number":2,"count":2},{"line_number":3,"count":0}"#;
    let idle = r#"{"line_number":1,"count":0},{"line_number":2,"count":0},{"line_number":3,"count":0},{"line_number":0,"count":9}"#;
    let text = format!("{}\n{}\n", gcov_doc(&d, "h.h", 2, ran), gcov_doc(&d, "h.h", 0, idle));
    let ev = ingest(&d, &text, &[]).expect("ingest");
    assert_eq!(ev.files.len(), 1, "one file");
    let counts: Vec<Option<u64>> = ev.files[0].lines.iter().map(|l| l.count).collect();
    assert_eq!(&counts[..4], &[Some(2), Some(2), Some(0), None], "summed per line; line 0 is not a line");
    let hf: Vec<_> = ev.functions.iter().filter(|f| f.name == "hf").collect();
    assert_eq!(hf.len(), 1, "one function");
    assert_eq!(hf[0].count, 2);
    let _ = fs::remove_dir_all(d);
}

// ---------------------------------------------------------------------------
// Per-test attribution
// ---------------------------------------------------------------------------

/// docs/butterfly.html:782 / README.md:419 "Per-test attribution: which test
/// entered which function and line", and README.md:546-549 the matrix counts
/// what "those named tests reached".
#[test]
fn each_test_is_credited_with_the_functions_and_lines_it_entered() {
    let d = scratch("pertest");
    fs::write(d.join("w.c"), W_C).unwrap();
    let p = d.join("w.c").display().to_string();
    let g = g_record(&p, "g", 2, (0, 2), (1, 1), "");
    let merged = export(&p, W_SEGMENTS, &[f_record(&p), g]);
    // t_f entered only f (lines 1,2,5,6); t_g only g (lines 8-11)
    let t_f = export(
        &p,
        "[[1,14,4,true,true,false],[2,14,0,true,true,false],[4,4,4,true,false,true],[5,3,4,true,true,false],[6,2,0,false,false,false]]",
        &[f_record(&p), g_record(&p, "g", 0, (0, 0), (0, 0), "")],
    );
    let t_g = export(
        &p,
        "[[8,21,2,true,true,false],[9,15,1,true,true,false],[9,24,1,true,false,true],[10,3,1,true,true,false],[11,2,0,false,false,false]]",
        &[f_record(&p).replace("\"count\":4", "\"count\":0"), g_record(&p, "g", 2, (0, 2), (1, 1), "")],
    );
    let ev = ingest(&d, &merged, &[("t_f", t_f), ("t_g", t_g)]).expect("ingest");
    assert_eq!(ev.tests.len(), 2, "{}", caveats(&ev));
    let id = |n: &str| ev.tests.iter().find(|t| t.name == n).unwrap().id;
    assert_eq!(func(&ev, "f").tests, vec![id("t_f")]);
    assert_eq!(func(&ev, "g").tests, vec![id("t_g")]);
    let tf = ev.tests.iter().find(|t| t.name == "t_f").unwrap();
    assert_eq!(tf.lines.get(&0), Some(&vec![(1, 2), (5, 6)]), "the lines it reached");
    let _ = fs::remove_dir_all(d);
}

/// The same requirement for a C++ (or Rust) project: llvm-cov names every
/// function by its mangled symbol in both the merged and the per-test
/// exports, and the test must still be credited with the function it entered.
#[test]
fn a_cpp_test_is_credited_with_the_mangled_function_it_entered() {
    let d = scratch("pertest-cpp");
    let src = "int f(int v) {\n  return v + 1;\n}\n";
    fs::write(d.join("w.cpp"), src).unwrap();
    let p = d.join("w.cpp").display().to_string();
    let rec = format!(r#"{{"name":"_Z1fi","count":1,"regions":[[1,14,3,2,1,0,0,0]],"branches":[],"filenames":[{p:?}]}}"#);
    let segs = "[[1,14,1,true,true,false],[3,2,0,false,false,false]]";
    let merged = export(&p, segs, std::slice::from_ref(&rec));
    let ev = ingest(&d, &merged, &[("t_one", export(&p, segs, &[rec]))]).expect("ingest");
    assert_eq!(ev.functions[0].name, "f(int)");
    assert_eq!(ev.tests.len(), 1, "the test export is attributed: {}", caveats(&ev));
    assert_eq!(ev.functions[0].tests, vec![ev.tests[0].id]);
    let _ = fs::remove_dir_all(d);
}

/// The merged export is the measurement; a per-test export claiming a line
/// the merged one says never ran describes a different run. README.md:546-549
/// says the matrix reports what the named tests reached: crediting a test
/// with an impossible line would report coverage nothing measured. It is
/// refused, and the refusal names the test.
#[test]
fn a_test_export_that_contradicts_the_merged_coverage_is_not_attributed() {
    let d = scratch("pertest-bad");
    fs::write(d.join("w.c"), W_C).unwrap();
    let p = d.join("w.c").display().to_string();
    let g = g_record(&p, "g", 2, (0, 2), (1, 1), "");
    let merged = export(&p, W_SEGMENTS, &[f_record(&p), g.clone()]);
    // claims line 3 ran, and f ran 9 times where the merge says 4
    let bad = export(&p, "[[1,14,9,true,true,false],[6,2,0,false,false,false]]", &[f_record(&p).replace("\"count\":4", "\"count\":9"), g]);
    let ev = ingest(&d, &merged, &[("t_bad", bad)]).expect("ingest");
    assert!(ev.tests.is_empty(), "nothing is attributed from it");
    assert!(func(&ev, "f").tests.is_empty());
    assert!(caveats(&ev).contains("t_bad"), "{}", caveats(&ev));
    let _ = fs::remove_dir_all(d);
}
