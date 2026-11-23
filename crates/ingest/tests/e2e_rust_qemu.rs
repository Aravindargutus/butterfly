// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! End-to-end test of MEASURED coverage: a bare-metal AArch64 image, run
//! under QEMU, its execution trace imported beside the host's coverage
//! export (#164). Until this fixture existed the trace importer had only
//! ever met hand-written traces (`synthetic.rs`).
//!
//! Runs only when `examples/rust-qemu/build.sh` has been run, which needs
//! `qemu-system-aarch64`, the `aarch64-unknown-none-softfloat` target and a
//! nightly toolchain. Skipped, not failed, otherwise -- and the skip line is
//! the one `scripts/ci-local.sh` and `ci.yml` know to expect on a host with
//! no QEMU, so keep its wording in step with them.
//!
//! The numbers below are the ones `examples/rust-qemu/README.md` prints.
use butterfly_model::*;
use std::path::PathBuf;

fn evidence() -> Option<Evidence> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/rust-qemu");
    let build = root.join("build");
    if !(build.join("coverage.json").exists() && build.join("flight.elf").exists() && build.join("trace.log").exists()) {
        butterfly_testsupport::not_applicable("examples/rust-qemu/build not present (run examples/rust-qemu/build.sh; it needs qemu-system-aarch64)");
        return None;
    }
    // Discovery, not hand-set options: `butterfly ingest build` is what the
    // README tells a reader to run, so the trace must be found by name.
    let mut opts = butterfly_ingest::discover(&build).expect("discover");
    assert!(opts.trace.as_deref().is_some_and(|t| t.ends_with("trace.log")), "discovery must pick up build/trace.log: {:?}", opts.trace);
    assert_eq!(opts.objects.len(), 1, "the instrumented host binary must not be analysed: {:?}", opts.objects);
    opts.source_root = Some(root);
    Some(butterfly_ingest::ingest(&opts).expect("ingest"))
}

fn func<'a>(ev: &'a Evidence, name: &str) -> &'a Function {
    ev.functions.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no function {name}"))
}

fn ran(i: &Instr) -> bool {
    i.measured.is_some_and(|n| n > 0)
}

#[test]
fn a_qemu_trace_yields_measured_instruction_and_outcome_coverage() {
    let Some(ev) = evidence() else { return };
    let m = ev.measured.as_ref().expect("a trace was given, so the evidence must carry measured coverage");
    assert_eq!(m.format, "qemu -d in_asm,exec");
    // 148 blocks run, every time. About one run in three QEMU's log holds
    // 149 records, because it logs a block on entry and re-enters the very
    // first one after abandoning it for a pending exit request; the importer
    // discards that repeat (and says so), since `_start` ends in a call and
    // cannot follow itself. Before it did, this number moved between runs.
    assert_eq!(m.block_executions, 148);
    assert_eq!(m.transitions, 147);
    assert!(m.caveats.iter().all(|c| !c.contains("reconstructed")), "every executed block had an in_asm listing: {:?}", m.caveats);

    // The README's source-level claim about `sum`, which is what makes the
    // object-level picture worth looking at: at source level the function is
    // finished, 9/9 lines and both outcomes of its one decision taken. The
    // page said so and nothing asserted it (self-review of #208).
    let sum = func(&ev, "flight_qemu::monitor::sum");
    assert_eq!((sum.lines_covered, sum.lines_total), (9, 9), "sum is 9/9 lines at source level");
    let [loop_test] = &sum.branches[..] else { panic!("sum has one decision: {:?}", sum.branches) };
    assert!(loop_test.true_count > 0 && loop_test.false_count > 0, "both outcomes of `i < n` are taken: {loop_test:?}");

    let s = &ev.summary;
    assert_eq!(s.instructions_total, 312);
    assert_eq!(s.instructions_measured_covered, Some(257));
    assert_eq!(s.outcome_edges_total, 42);
    assert_eq!(s.outcome_edges_measured, Some(27));
    assert_eq!(m.functions_executed, 7, "every function with object code but cmd_hold");

    // The README's per-function table: instructions, projected covered,
    // measured executed, outcome edges measured / total.
    let table: Vec<(&str, usize, usize, usize, usize, usize)> = ev
        .functions
        .iter()
        .filter_map(|f| {
            f.object
                .as_ref()
                .map(|o| (f.name.as_str(), o.instr_total, o.instr_projected_covered, o.instr_measured_covered.unwrap(), o.outcome_edges_measured.unwrap(), o.outcome_edges_total))
        })
        .collect();
    assert_eq!(
        table,
        [
            ("flight_qemu::campaign", 117, 81, 93, 8, 16),
            ("flight_qemu::monitor::clamp", 27, 20, 27, 4, 4),
            ("flight_qemu::monitor::band", 31, 20, 28, 5, 6),
            ("flight_qemu::monitor::sum", 63, 54, 52, 5, 8),
            ("flight_qemu::monitor::cmd_climb", 18, 15, 15, 1, 2),
            ("flight_qemu::monitor::cmd_descend", 18, 15, 15, 1, 2),
            ("flight_qemu::monitor::cmd_hold", 4, 0, 0, 0, 0),
            ("flight_qemu::monitor::dispatch", 34, 29, 27, 3, 4),
        ]
    );
    assert_eq!((s.instructions_projected_covered, s.instructions_projected_uncovered, s.instructions_unknown), (234, 13, 65));

    // Every conditional branch that ran must have its executions accounted
    // for by its two outcome edges. This is the check that the translation-
    // block transitions really do line up with the CFG built from the
    // disassembly -- the part no synthetic trace could establish.
    for f in &ev.functions {
        let Some(o) = &f.object else { continue };
        for b in &o.blocks {
            let last = b.instrs.last().unwrap();
            if b.succs.len() == 2 {
                let out: u64 = b.succs.iter().map(|e| e.measured.unwrap_or(0)).sum();
                assert_eq!(out, last.measured.unwrap_or(0), "{} B{}: outcome edges must sum to the branch's executions", f.name, b.id);
            }
        }
    }
}

/// The reason the feature exists. Both runs execute the same `campaign()`,
/// so every difference here is one the projection cannot avoid, not a
/// difference in what was tested.
#[test]
fn the_trace_contradicts_the_projection_on_the_panic_paths() {
    let Some(ev) = evidence() else { return };
    let sum = func(&ev, "flight_qemu::monitor::sum");
    let o = sum.object.as_ref().unwrap();

    // `acc = acc + samples[i]` is one source line that ran four times, so
    // every instruction attributed to it is projected covered -- including
    // the call into the bounds-check panic, which never ran.
    let panic_call =
        o.blocks.iter().flat_map(|b| &b.instrs).find(|i| i.mnemonic == "bl" && i.operands.contains("panic_bounds_check")).expect("sum() keeps its bounds check at -Copt-level=0");
    assert_eq!(panic_call.projected, Status::Covered, "the projection credits the panic call with its line's coverage");
    assert!(!ran(panic_call), "the trace never executed it: {:?}", panic_call.measured);
    assert!(o.disagreements.iter().any(|d| d.addr == panic_call.addr && d.kind == DisagreementKind::ProjectedNotExecuted));

    // And the branch guarding it: projected covered both ways, measured one.
    let guard = o.blocks.iter().find(|b| b.succs.iter().any(|e| o.blocks[e.to].instrs.iter().any(|i| i.addr == panic_call.addr))).unwrap();
    let to_panic = guard.succs.iter().find(|e| e.kind == EdgeKind::Taken).unwrap();
    let past_it = guard.succs.iter().find(|e| e.kind == EdgeKind::Fallthrough).unwrap();
    assert_eq!(to_panic.projected, Status::Covered);
    assert_eq!((to_panic.measured, past_it.measured), (Some(0), Some(4)));

    let count = |k: DisagreementKind| ev.functions.iter().filter_map(|f| f.object.as_ref()).flat_map(|o| &o.disagreements).filter(|d| d.kind == k).count();
    assert_eq!(o.disagreements.iter().filter(|d| d.kind == DisagreementKind::ProjectedNotExecuted).count(), 9, "sum: three panic paths, three instructions each");
    assert_eq!(count(DisagreementKind::ProjectedNotExecuted), 21, "all of them panic paths: sum 9, dispatch 6, cmd_climb 3, cmd_descend 3");
    // The other direction: instructions with no source line have no
    // projection at all, and the trace settles them.
    assert_eq!(count(DisagreementKind::ExecutedUnknown), 44);
    assert_eq!(count(DisagreementKind::ExecutedNotProjected), 0, "same campaign on both sides: nothing ran that the source coverage says did not");
    assert_eq!(ev.summary.disagreements, 65);
}

/// Where the two agree, they should: the outcome no run takes, and the
/// function no run calls.
#[test]
fn the_trace_confirms_what_was_never_run() {
    let Some(ev) = evidence() else { return };
    let band = func(&ev, "flight_qemu::monitor::band").object.as_ref().unwrap();
    let first = &band.blocks[0];
    let over = first.succs.iter().find(|e| e.kind == EdgeKind::Taken).unwrap();
    assert_eq!((over.projected, over.measured), (Status::Uncovered, Some(0)), "`alt > 45_000` is never true");
    assert_eq!((band.outcome_edges_total, band.outcome_edges_measured), (6, Some(5)));

    let hold = func(&ev, "flight_qemu::monitor::cmd_hold");
    assert_eq!(hold.count, 0);
    assert_eq!(hold.object.as_ref().unwrap().instr_measured_covered, Some(0));

    // Static analysis can only list who the indirect call MAY reach. The
    // trace says who it did.
    let dispatch = func(&ev, "flight_qemu::monitor::dispatch");
    let [site] = &dispatch.indirect_calls[..] else { panic!("one indirect call: {:?}", dispatch.indirect_calls) };
    let mut may: Vec<&str> = site.candidates.iter().map(|t| t.name.as_str()).collect();
    may.sort_unstable();
    assert_eq!(may, ["flight_qemu::monitor::cmd_climb", "flight_qemu::monitor::cmd_descend", "flight_qemu::monitor::cmd_hold"]);
    let mut did: Vec<(&str, u64)> = site.measured.iter().map(|t| (t.name.as_str(), t.count)).collect();
    did.sort_unstable();
    assert_eq!(did, [("flight_qemu::monitor::cmd_climb", 1), ("flight_qemu::monitor::cmd_descend", 1)]);
    assert_eq!(site.measured_count, Some(2));
}

/// #210: the block that ends in `bl core::panicking::panic_bounds_check` had
/// a fall-through edge, because the CFG builder did not know the callee
/// never returns. The measured count on it was 0, which was right — but an
/// edge no execution can take is not a 0% outcome, it is a claim about
/// control flow that is false. It drew an arrow in the Flow view standing
/// for a transfer that cannot happen.
///
/// The rule is the callee's own `DW_AT_noreturn`, which `rustc` emits for
/// every `fn` returning an uninhabited type, and the block records it. No
/// symbol name is read as a statement about behaviour.
#[test]
fn a_call_that_cannot_return_ends_its_block_and_says_why() {
    let Some(ev) = evidence() else { return };
    let sum = func(&ev, "flight_qemu::monitor::sum").object.as_ref().unwrap();

    let panic_block = sum
        .blocks
        .iter()
        .find(|b| b.instrs.last().is_some_and(|i| i.mnemonic == "bl" && i.operands.contains("panic_bounds_check")))
        .expect("sum() keeps its bounds check at -Copt-level=0");
    assert!(panic_block.succs.is_empty(), "the bounds-check panic does not return, so its block has no successor: {:?}", panic_block.succs);
    let why = panic_block.diverges.as_deref().expect("the block must record why it has no successor");
    assert!(why.contains("DW_AT_noreturn"), "the rule used must be in the evidence, not implied: {why}");
    assert!(why.contains("panic_bounds_check"), "the evidence must name the callee it was read from: {why}");

    // Nothing anywhere in the image now has a fall-through edge out of a
    // call to a function the DWARF marks as diverging. This is the whole
    // claim, checked over every function rather than the one above.
    for f in &ev.functions {
        let Some(o) = &f.object else { continue };
        for b in &o.blocks {
            if b.diverges.is_some() {
                assert!(b.succs.is_empty(), "{} B{}: a block that cannot fall through has a successor: {:?}", f.name, b.id, b.succs);
            }
        }
    }

    // Every diverging block the fixture has, by name, so a rule that stopped
    // firing — or started firing somewhere new — is a test failure rather
    // than a silent change to the control-flow graph. `rustc` emits the
    // attribute on the panic entry points; these six call sites are the
    // bounds check and the arithmetic-overflow checks at -Copt-level=0.
    let mut diverging: Vec<(&str, usize)> = ev
        .functions
        .iter()
        .filter_map(|f| f.object.as_ref().map(|o| (f.name.as_str(), o)))
        .flat_map(|(n, o)| o.blocks.iter().filter(|b| b.diverges.is_some()).map(move |b| (n, b.id)))
        .collect();
    diverging.sort_unstable();
    assert_eq!(
        diverging,
        [
            ("flight_qemu::monitor::cmd_climb", 3),
            ("flight_qemu::monitor::cmd_descend", 3),
            ("flight_qemu::monitor::dispatch", 6),
            ("flight_qemu::monitor::sum", 8),
            ("flight_qemu::monitor::sum", 11),
            ("flight_qemu::monitor::sum", 13),
        ]
    );

    // What did NOT move. Dropping these edges was expected to change the
    // object-branch totals (#210's own wording); it does not, and the
    // reason matters: an outcome edge is counted only out of a block with
    // two successors one of which is `taken` (`crates/ingest/src/lib.rs`),
    // and a call's fall-through was never one of those. What changes is the
    // edge count, and with it the arrow in the Flow view.
    assert_eq!((ev.summary.outcome_edges_total, ev.summary.outcome_edges_measured), (42, Some(27)), "no outcome edge was ever attached to a call's fall-through");
    assert_eq!(ev.summary.instructions_total, 312, "no instruction is added or removed by an edge");
    assert_eq!(func(&ev, "flight_qemu::monitor::sum").complexity.object_edges, 14, "sum loses the two fall-throughs out of its panic calls");
    assert_eq!(func(&ev, "flight_qemu::monitor::dispatch").complexity.object_edges, 8, "dispatch loses the one out of its bounds-check panic");
    // ...and cyclomatic complexity does not move with them: each edge that
    // goes turns its block into an exit, and E - N + 2 counts both.
    assert_eq!(func(&ev, "flight_qemu::monitor::sum").complexity.cyclomatic_object, Some(5));
    assert_eq!(func(&ev, "flight_qemu::monitor::dispatch").complexity.cyclomatic_object, Some(3));
}
