// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! A switch that compiles to a jump table, read from a real image (#171).
//!
//! `examples/c-switch` has one sixteen-case switch that clang lowers to an
//! indirect branch through a table at every optimisation level: word
//! offsets at `-O0`, byte offsets scaled by four at `-O2` (AArch64), word
//! offsets from the table itself on x86-64. main runs cases 0 to 5.
//!
//! Before the table was read, the indirect branch had no successors, so all
//! sixteen case blocks were reported unreachable; a branch region per case
//! was reported as a predicated decision; and at `-O0` on AArch64 the table,
//! which clang places inside the function, was analysed as sixteen `udf`
//! instructions. The assertions below are about the switch, not about one
//! architecture's instruction counts, so they hold on the x86-64 CI runner
//! as they do here.

use butterfly_model::{EdgeKind, Evidence, GapKind, Status};
use std::path::PathBuf;

fn example(dir: &str) -> Option<(PathBuf, PathBuf)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-switch").canonicalize().ok()?;
    let build = root.join(dir);
    if build.join("coverage.json").exists() && build.join("switch").exists() {
        Some((root, build))
    } else {
        butterfly_testsupport::fixture_missing("run examples/c-switch/build.sh first");
        None
    }
}

fn ingest(dir: &str) -> Option<Evidence> {
    let (root, build) = example(dir)?;
    let mut opts = butterfly_ingest::discover(&build).expect("discover");
    opts.source_root = Some(root);
    Some(butterfly_ingest::ingest(&opts).expect("ingest"))
}

fn check_dispatch(ev: &Evidence, level: &str) {
    let f = ev.functions.iter().find(|f| f.name == "dispatch").expect("dispatch is analysed");
    let o = f.object.as_ref().expect("dispatch has object code");

    // the indirect branch has the cases as successors
    let br = o.blocks.iter().find(|b| b.succs.iter().any(|e| e.kind == EdgeKind::Indirect)).unwrap_or_else(|| panic!("{level}: no indirect branch has table edges"));
    let targets = br.succs.iter().filter(|e| e.kind == EdgeKind::Indirect).count();
    assert!(targets >= 8, "{level}: only {targets} case targets were recovered");
    // every case line is the first line of some case block, reached through the table
    let reached: Vec<u32> = br.succs.iter().flat_map(|e| o.blocks[e.to].lines.clone()).collect();
    for line in 21..=36u32 {
        assert!(reached.contains(&line), "{level}: case on line {line} is not reached through the table ({reached:?})");
    }

    // nothing behind the table is called unreachable
    assert!(o.unreachable_blocks.is_empty(), "{level}: {:?} reported unreachable", o.unreachable_blocks);

    // a case's branch region is realised by its table edge, so none is a predicated decision
    let predicated: Vec<u32> = o.gaps.iter().filter(|g| g.kind == GapKind::PredicatedDecision).filter_map(|g| g.line).collect();
    assert!(predicated.is_empty(), "{level}: case regions reported as predicated decisions on lines {predicated:?}");

    // the table's bytes are not instructions
    let udf = o.blocks.iter().flat_map(|b| &b.instrs).filter(|i| i.mnemonic == "udf").count();
    assert_eq!(udf, 0, "{level}: the table was disassembled as code");

    // the indirect branch stays a review item, and says its targets are known
    let g = o.gaps.iter().find(|g| g.kind == GapKind::IndirectBranch).unwrap_or_else(|| panic!("{level}: no indirect-branch gap"));
    assert!(g.description.contains("jump table") && g.description.contains("read from the table"), "{level}: {}", g.description);
}

#[test]
fn the_o0_switch_is_read_through_its_table() {
    let Some(ev) = ingest("build") else { return };
    check_dispatch(&ev, "-O0");
    // cases 0..5 ran, 6..15 did not: six covered table edges, ten uncovered
    let o = ev.functions.iter().find(|f| f.name == "dispatch").and_then(|f| f.object.as_ref()).unwrap();
    let br = o.blocks.iter().find(|b| b.succs.iter().any(|e| e.kind == EdgeKind::Indirect)).unwrap();
    let covered = br.succs.iter().filter(|e| e.projected == Status::Covered).count();
    let uncovered = br.succs.iter().filter(|e| e.projected == Status::Uncovered).count();
    assert_eq!((covered, uncovered), (6, 10), "{:?}", br.succs);
}

#[test]
fn the_o2_switch_is_read_through_its_table() {
    let Some(ev) = ingest("build-o2") else { return };
    check_dispatch(&ev, "-O2");
}

/// At -O2 clang turns each `puts(...); break;` into a tail call, `b _puts`.
/// It was classified as a plain jump, so the call graph had no `puts` and
/// the folded-call check said the source's `puts` calls had no object
/// counterpart (#264).
#[test]
fn tail_calls_are_calls() {
    let Some(ev) = ingest("build-o2") else { return };
    let f = ev.functions.iter().find(|f| f.name == "dispatch").expect("dispatch");
    assert!(f.callees.iter().any(|c| c.symbol.trim_start_matches('_') == "puts"), "puts is a callee: {:?}", f.callees.iter().map(|c| &c.symbol).collect::<Vec<_>>());
    assert!(f.folded_calls.is_empty(), "no call is reported folded: {:?}", f.folded_calls.iter().map(|c| (&c.name, c.line)).collect::<Vec<_>>());
    let folded_gaps = f.object.as_ref().unwrap().gaps.iter().filter(|g| g.kind == GapKind::FoldedCall).count();
    assert_eq!(folded_gaps, 0);
}
