// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for execution traces and measured coverage
//! (area B).
//!
//! Expectations come from README.md "Projected, measured, and the gap
//! between them" (~L221-232), "Inputs" (~L288-289), the security posture
//! (~L1622, counters saturate), the support matrix row
//! `feature.measured-coverage` (README.md:436) and docs/butterfly.html
//! "Projected versus measured" (~L361-372).
//!
//! The parser tests are hand-written logs. The measured-ingest tests run on
//! examples/c with an address-list trace written from that build's own
//! addresses, so they hold on any architecture the example builds for; they
//! skip (or fail under BUTTERFLY_REQUIRE_FIXTURES=1) when it is not built.

use butterfly_ingest::trace;
use butterfly_model::*;
use std::fs;
use std::path::PathBuf;

/// README.md:288-289 "a plain list of executed addresses", optionally with a
/// count after each address.
#[test]
fn a_plain_address_list_counts_each_address() {
    let t = trace::parse("0x1000\n0x1004 3\n1008\n0x1000\n");
    assert_eq!(t.format, "address list");
    assert_eq!(t.counts.get(&0x1000), Some(&2));
    assert_eq!(t.counts.get(&0x1004), Some(&3));
    assert_eq!(t.counts.get(&0x1008), Some(&1));
    assert_eq!(t.counts.len(), 3);
}

/// README.md:288 "the QEMU `execlog` plugin output": one line per executed
/// instruction.
#[test]
fn an_execlog_counts_one_execution_per_line() {
    let log = "0, 0x40080000, 0xd53800a0, \"mrs x0, mpidr_el1\"\n0, 0x40080004, 0x92400400, \"and x0, x0, #3\"\n0, 0x40080000, 0xd53800a0, \"mrs x0, mpidr_el1\"\n";
    let t = trace::parse(log);
    assert_eq!(t.format, "qemu execlog plugin");
    assert_eq!(t.counts.get(&0x40080000), Some(&2));
    assert_eq!(t.counts.get(&0x40080004), Some(&1));
}

/// README.md:1622 "Counters saturate. A corrupt trace cannot overflow an
/// execution count."
#[test]
fn a_count_that_would_overflow_saturates() {
    let t = trace::parse("0x1000 18446744073709551615\n0x1000 18446744073709551615\n0x1000\n");
    assert_eq!(t.counts.get(&0x1000), Some(&u64::MAX));
    assert_eq!(t.block_executions, u64::MAX, "the total saturates too");
}

/// A QEMU `-d in_asm,exec` log (README.md:288): an `IN:` block lists a
/// translation block's instructions and each `Trace` record is one entry of
/// it; spread over instructions, each instruction of the block ran as often
/// as the block was entered.
#[test]
fn a_qemu_log_counts_blocks_and_spreads_them_over_their_instructions() {
    let log = "IN:\n0x00001000:  d2800020  mov x0, #1\n0x00001004:  d65f03c0  ret\n\nTrace 0: 0x7f00 [00000000/0000000000001000/00000000/00000000]\nTrace 0: 0x7f00 [00000000/0000000000001000/00000000/00000000]\n";
    let mut t = trace::parse(log);
    assert_eq!(t.format, "qemu -d in_asm,exec");
    assert_eq!(t.tb_counts.get(&0x1000), Some(&2));
    t.expand(&|_| Vec::new());
    assert_eq!(t.counts.get(&0x1000), Some(&2));
    assert_eq!(t.counts.get(&0x1004), Some(&2));
}

/// support/features.toml:169-176 / README.md:436 "multi-CPU logs are read per
/// CPU": what ran after what is a fact about one CPU, so two neighbouring
/// records of different CPUs are not a transition.
#[test]
fn transitions_are_drawn_within_one_cpus_records_only() {
    let rec = |cpu: u32, pc: u64| format!("Trace {cpu}: 0x7f00 [00000000/{pc:016x}/00000000/00000000]\n");
    let log = [rec(0, 0x1000), rec(1, 0x2000), rec(0, 0x1004), rec(1, 0x2004)].concat();
    let t = trace::parse(&log);
    assert_eq!(t.transitions.get(&(0x1000, 0x1004)), Some(&1));
    assert_eq!(t.transitions.get(&(0x2000, 0x2004)), Some(&1));
    assert_eq!(t.transitions.len(), 2, "no transition crosses CPUs: {:?}", t.transitions);
    assert!(t.caveats.iter().any(|c| c.contains("2 CPUs")), "the interleaving is stated: {:?}", t.caveats);
}

/// README.md:436 "execution counts are an upper bound: QEMU logs a block on
/// entry, and an abandoned entry that cannot be proven impossible is still
/// counted" -- and, by the same sentence, one that CAN be proven impossible
/// (a block whose last instruction cannot lead back to its own start, logged
/// twice in a row by one CPU) is not.
#[test]
fn a_repeat_is_discarded_only_when_it_is_provably_impossible() {
    let log = "IN:\n0x00001000:  d2800020  mov x0, #1\n0x00001004:  14000010  b 0x1044\n\nTrace 0: 0x7f00 [00000000/0000000000001000/00000000/00000000]\nTrace 0: 0x7f00 [00000000/0000000000001000/00000000/00000000]\n";
    let mut kept = trace::parse(log);
    assert_eq!(kept.discard_impossible_repeats(&|_, _| true), 0, "cannot be proven impossible: counted");
    assert_eq!(kept.tb_counts.get(&0x1000), Some(&2));

    let mut proven = trace::parse(log);
    assert_eq!(proven.discard_impossible_repeats(&|_, _| false), 1);
    assert_eq!(proven.tb_counts.get(&0x1000), Some(&1));
    assert!(!proven.caveats.is_empty(), "and the discard is said, not silent");
}

/// docs/butterfly.html:808 "Every one of these is a caveat on the evidence
/// rather than a silent guess": an executed block the log never listed has
/// its extent reconstructed from the disassembly, and the evidence says so.
#[test]
fn a_reconstructed_block_extent_is_stated() {
    let log = "IN:\n0x00001000:  d65f03c0  ret\n\nTrace 0: 0x7f00 [00000000/0000000000001000/00000000/00000000]\nTrace 0: 0x7f00 [00000000/0000000000002000/00000000/00000000]\n";
    let mut t = trace::parse(log);
    t.expand(&|pc| if pc == 0x2000 { vec![0x2000, 0x2004] } else { Vec::new() });
    assert_eq!(t.counts.get(&0x2004), Some(&1));
    assert!(t.caveats.iter().any(|c| c.contains("reconstructed")), "{:?}", t.caveats);
}

/// Hostile input is a caveat or a refusal, never a panic: brackets the wrong
/// way round, a missing PC and a truncated `IN:` line.
#[test]
fn malformed_qemu_records_do_not_panic() {
    let log = "IN:\n0x0000100\nTrace 0] x [0/\nTrace\nTrace 0: [zz/yy]\n";
    let t = trace::parse(log);
    assert_eq!(t.format, "qemu -d in_asm,exec");
    assert!(t.refusal().is_some(), "a log with nothing readable is refused, not reported as measured");
}

// ---------------------------------------------------------------------------
// Measured coverage on examples/c
// ---------------------------------------------------------------------------

fn example() -> Option<(PathBuf, butterfly_ingest::IngestOptions)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c/build.sh first");
        return None;
    }
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root.clone());
    o.trace = None;
    Some((root, o))
}

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("reqb-trace-{tag}"))
}

/// README.md:223-227 "With an execution trace (`--trace`), coverage becomes
/// **measured**: every instruction carries its execution count ... the Gaps
/// tab lists where projection and trace disagree (projected-covered but never
/// executed; executed but projected-uncovered ...)", and README.md:227-228
/// "The two views are only comparable when the trace and the instrumented run
/// execute the same test campaign".
///
/// The trace is every instruction of one function the projection calls
/// covered (`f_cov`), once, and every instruction of the function the
/// coverage run never called (`legacy_scale`), once. So: `f_cov` agrees with
/// its projection wherever it is projected covered; everything else projected
/// covered is "projected-covered but never executed"; every instruction of
/// `legacy_scale` projected uncovered is "executed but projected-uncovered".
#[test]
fn a_trace_makes_coverage_measured_and_names_every_disagreement() {
    let Some((_, mut o)) = example() else { return };
    let projected = butterfly_ingest::ingest(&o).expect("ingest");
    let legacy = projected.functions.iter().find(|f| f.name == "legacy_scale").expect("legacy_scale");
    assert_eq!(legacy.count, 0, "the example never calls it");
    let covered = projected
        .functions
        .iter()
        .find(|f| f.count > 0 && f.object.as_ref().is_some_and(|ob| ob.blocks.iter().flat_map(|b| &b.instrs).all(|i| i.projected == Status::Covered)))
        .expect("a function projected covered throughout");
    let addrs = |f: &Function| -> Vec<u64> { f.object.as_ref().unwrap().blocks.iter().flat_map(|b| b.instrs.iter().map(|i| i.addr)).collect() };
    let traced: Vec<u64> = addrs(covered).into_iter().chain(addrs(legacy)).collect();

    let d = scratch("measured");
    let tp = d.join("trace.log");
    fs::write(&tp, traced.iter().map(|a| format!("{a:#x}\n")).collect::<String>()).unwrap();
    o.trace = Some(tp);
    let ev = butterfly_ingest::ingest(&o).expect("ingest with trace");
    let m = ev.measured.as_ref().expect("coverage is measured");
    assert_eq!(m.format, "address list");
    assert!(m.caveats.iter().any(|c| c.contains("same test campaign")), "{:?}", m.caveats);

    let mut expect_pne = 0usize;
    let mut expect_enp = 0usize;
    let mut ran = 0usize;
    for f in &ev.functions {
        let Some(ob) = &f.object else { continue };
        for i in ob.blocks.iter().flat_map(|b| &b.instrs) {
            let executed = traced.contains(&i.addr);
            assert_eq!(i.measured.unwrap_or(0) > 0, executed, "{} @ {:#x}: measured {:?}", f.name, i.addr, i.measured);
            if executed {
                ran += 1;
                assert_eq!(i.measured, Some(1), "it ran once");
            }
            match (i.projected, executed) {
                (Status::Covered, false) => expect_pne += 1,
                (Status::Uncovered, true) => expect_enp += 1,
                _ => {}
            }
        }
    }
    let all: Vec<&Disagreement> = ev.functions.iter().filter_map(|f| f.object.as_ref()).flat_map(|o| &o.disagreements).collect();
    let pne = all.iter().filter(|d| d.kind == DisagreementKind::ProjectedNotExecuted).count();
    let enp = all.iter().filter(|d| d.kind == DisagreementKind::ExecutedNotProjected).count();
    assert_eq!(pne, expect_pne, "projected-covered but never executed");
    assert_eq!(enp, expect_enp, "executed but projected-uncovered");
    assert!(enp > 0 && pne > 0, "both kinds occur in this arrangement");
    assert!(all.iter().all(|d| d.addr != 0 && !(d.kind == DisagreementKind::ProjectedNotExecuted && traced.contains(&d.addr))));
    assert_eq!(ev.summary.instructions_measured_covered, Some(ran));
    assert_eq!(m.instructions_measured_covered, ran);
    let _ = fs::remove_dir_all(d);
}

/// README.md:223-224 "With an execution trace (`--trace`), coverage becomes
/// **measured**: every instruction carries its execution count", decided in
/// #325 (ambiguity 11): an instruction the trace never executed carries
/// `measured: 0`, and `null` is kept for "no trace was imported". The two
/// answers are different -- "not executed" against "not measured" -- and the
/// count stated is the one the trace gives. Blocks and symbols, whose count
/// is their first instruction's, say the same.
#[test]
fn with_a_trace_an_instruction_never_executed_measures_zero_not_null() {
    let Some((_, mut o)) = example() else { return };
    let projected = butterfly_ingest::ingest(&o).expect("ingest");
    for f in &projected.functions {
        for ob in f.objects() {
            for b in &ob.blocks {
                assert_eq!(b.measured, None, "no trace: {} B{} is not measured", f.name, b.id);
                assert!(b.instrs.iter().all(|i| i.measured.is_none()), "no trace: {} is not measured", f.name);
            }
        }
    }
    assert!(projected.symbols.iter().all(|s| s.measured.is_none()), "no trace: no symbol is measured");

    // one instruction of `main` ran, three times; nothing else did
    let main = projected.functions.iter().find(|f| f.name == "main").expect("main");
    let first = main.object.as_ref().expect("main's object code").blocks[0].instrs[0].addr;
    let d = scratch("zero");
    let tp = d.join("trace.log");
    fs::write(&tp, format!("{first:#x} 3\n")).unwrap();
    o.trace = Some(tp);
    let ev = butterfly_ingest::ingest(&o).expect("ingest with trace");
    let (mut zeros, mut ran) = (0usize, 0usize);
    for f in &ev.functions {
        for ob in f.objects() {
            for b in &ob.blocks {
                let expect = if b.instrs.first().is_some_and(|i| i.addr == first) { 3 } else { 0 };
                assert_eq!(b.measured, Some(expect), "{} B{}: a block's count is its first instruction's", f.name, b.id);
                for i in &b.instrs {
                    match i.measured {
                        Some(3) if i.addr == first => ran += 1,
                        Some(0) if i.addr != first => zeros += 1,
                        other => panic!("{} @ {:#x}: measured {other:?}", f.name, i.addr),
                    }
                }
            }
        }
    }
    assert_eq!(ran, 1);
    assert!(zeros > 0, "the instructions that did not run are 0");
    for s in &ev.symbols {
        if s.address == first {
            assert_eq!(s.measured, Some(3), "{}", s.name);
        } else if s.function_id.is_some() {
            assert_eq!(s.measured, Some(0), "{}: a traced symbol that never ran is 0", s.name);
        }
    }
    // the totals still count what ran, not what was measured
    assert_eq!(ev.summary.instructions_measured_covered, Some(1));
    assert_eq!(ev.measured.as_ref().unwrap().functions_executed, 1);
    let _ = fs::remove_dir_all(d);
}

/// README.md:288-289 names the trace formats butterfly reads; a file that is
/// none of them must not be read as an empty trace -- that would report every
/// instruction as never executed, a measurement nobody made
/// (docs/butterfly.html:808, a caveat or a refusal, never a silent guess).
#[test]
fn an_unrecognised_trace_is_refused_not_read_as_empty() {
    let Some((_, mut o)) = example() else { return };
    let d = scratch("unrecognised");
    let tp = d.join("trace.log");
    fs::write(&tp, "this is not a trace\nnor is this\n").unwrap();
    o.trace = Some(tp);
    let err = butterfly_ingest::ingest(&o).expect_err("refused").to_string();
    assert!(err.contains("trace.log"), "the refusal names the file: {err}");
    let _ = fs::remove_dir_all(d);
}
