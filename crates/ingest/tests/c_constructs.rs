// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The C constructs no other example has (#168), read from a real image:
//! `examples/c-constructs`.
//!
//! Each test states what butterfly does with one construct. The assertions
//! are about the construct, not about one compiler's instruction counts, so
//! they hold on the x86-64 ELF CI runner as they do on the AArch64 Mach-O
//! host they were written on; the one reader that only exists for AArch64
//! (frame-slot attribution of locals) is gated on the image's architecture,
//! not on the host's.
//!
//! Three of these were false claims before #168:
//!   * a VLA or alloca function with no `.su` read as a bounded stack;
//!   * a decision inside a macro was dropped, and its never-run `return`
//!     projected covered from the use line's count;
//!   * in `arm`, the block on the *taken* side of an `if` with no `else` was
//!     inferred uncovered, because the join after it had run.

use butterfly_model::{Evidence, FrameSource, Function, GapKind, Status};
use object::Object;
use std::path::PathBuf;

fn example() -> Option<(PathBuf, PathBuf)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-constructs").canonicalize().ok()?;
    let build = root.join("build");
    if build.join("coverage.json").exists() && build.join("constructs").exists() {
        Some((root, build))
    } else {
        butterfly_testsupport::fixture_missing("run examples/c-constructs/build.sh first");
        None
    }
}

fn ingest_with(keep_su: bool) -> Option<Evidence> {
    let (root, build) = example()?;
    let mut opts = butterfly_ingest::discover(&build).expect("discover");
    assert!(!opts.stack_usage.is_empty(), "build.sh writes a .su for the example");
    if !keep_su {
        // the image as a build without -fstack-usage would hand it over; its
        // build-info.txt says -fstack-usage, which would then be untrue
        opts.stack_usage.clear();
        opts.build_info = None;
    }
    opts.source_root = Some(root);
    Some(butterfly_ingest::ingest(&opts).expect("ingest"))
}

fn func<'a>(ev: &'a Evidence, name: &str) -> &'a Function {
    ev.functions.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("{name} is not in the evidence"))
}

/// The 1-based line of the first source line containing `needle`.
fn line_of(ev: &Evidence, needle: &str) -> u32 {
    let text = &ev.files[0].text;
    let i = text.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("`{needle}` is not in constructs.c"));
    (i + 1) as u32
}

fn stack<'a>(ev: &'a Evidence, name: &str) -> &'a butterfly_model::Symbol {
    ev.symbols.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no symbol {name}"))
}

fn check_dynamic_frames(ev: &Evidence, via: &str) {
    for name in ["vla_sum", "alloca_fill"] {
        let s = stack(ev, name);
        assert!(s.frame.dynamic, "{via}: {name}'s frame is not marked dynamic ({:?})", s.frame);
        assert!(!s.stack.bounded, "{via}: {name} is reported bounded at {:?}", s.stack.worst_case);
        assert_eq!(s.stack.reason.as_deref(), Some("dynamic frame (alloca / VLA)"), "{via}: {name}");
    }
    // a static frame that calls a dynamic one is not bounded either, and main
    // above it neither
    for name in ["calls_vla", "main"] {
        let s = stack(ev, name);
        assert!(!s.frame.dynamic, "{via}: {name}'s own frame is static");
        assert!(!s.stack.bounded, "{via}: {name} calls a dynamic frame but is reported bounded");
    }
    // and a function with neither is bounded: the rule is not "everything"
    for name in ["goto_cleanup", "float_bits", "arm", "checked_scale"] {
        assert!(stack(ev, name).stack.bounded, "{via}: {name} has a fixed frame and no calls");
    }
}

#[test]
fn a_vla_and_an_alloca_are_unbounded_with_the_su() {
    let Some(ev) = ingest_with(true) else { return };
    assert_eq!(stack(&ev, "vla_sum").frame.source, FrameSource::StackUsageFile);
    check_dynamic_frames(&ev, ".su");
}

#[test]
fn a_vla_and_an_alloca_are_unbounded_without_the_su() {
    // Before #168 the prologue estimate stood alone here: the allocation is
    // in the body, and vla_sum read as bounded by its static frame (on a host
    // with no stack probe call to make it unbounded for another reason).
    let Some(ev) = ingest_with(false) else { return };
    assert_eq!(stack(&ev, "vla_sum").frame.source, FrameSource::Prologue);
    check_dynamic_frames(&ev, "prologue");
}

#[test]
fn a_call_through_the_got_is_a_direct_call_to_the_bound_symbol() {
    // #271: on AArch64 Darwin a VLA or alloca function calls the stack probe
    // `___chkstk_darwin` as `adrp x16, GOT; ldr x16, [x16, #off]; blr x16`.
    // The callee is the symbol the GOT slot is bound to, not a function
    // pointer, yet it was reported as an indirect call. The source of these
    // functions has no function pointer at all, so on no target may they
    // carry an indirect call -- whatever probe or helper the compiler calls.
    for keep_su in [true, false] {
        let Some(ev) = ingest_with(keep_su) else { return };
        for name in ["vla_sum", "alloca_fill"] {
            let f = func(&ev, name);
            let o = f.object.as_ref().unwrap();
            let indirect: Vec<&str> = o.gaps.iter().filter(|g| g.kind == GapKind::IndirectBranch).map(|g| g.description.as_str()).collect();
            assert!(indirect.is_empty(), "{name}: {indirect:?}");
            assert!(f.indirect_calls.is_empty(), "{name}: {:?}", f.indirect_calls);
            assert_eq!(stack(&ev, name).indirect_calls, 0, "{name}");
            // the reason is the frame, and only the frame
            assert_eq!(stack(&ev, name).stack.reason.as_deref(), Some("dynamic frame (alloca / VLA)"), "{name}");
        }
    }
    // Where the probe is the GOT call of the issue, it is named, and called
    // what it is: a compiler-inserted runtime routine outside the source set.
    // Gate this compiler-dependent assertion on the original image's imports,
    // not on butterfly's gaps (which would hide a dropped helper). Apple clang
    // emits the Darwin probe, whereas upstream LLVM need not. The assertions
    // above still check both dynamic-frame functions on every compiler, and
    // req_e_got exercises the GOT sequence independently of compiler output.
    let Some(ev) = ingest_with(true) else { return };
    let image = ev.image.as_ref().unwrap();
    if image.arch.starts_with("aarch64") && image.format == "MachO" {
        let (_, build) = example().expect("the ingested fixture exists");
        let bytes = std::fs::read(build.join("constructs")).expect("read original image");
        let object = object::File::parse(bytes.as_slice()).expect("parse original image");
        let imports = object.imports().expect("read original image imports");
        if !imports.iter().any(|import| import.name().ends_with(b"__chkstk_darwin")) {
            return;
        }
        for name in ["vla_sum", "alloca_fill"] {
            let o = func(&ev, name).object.as_ref().unwrap();
            let probe = o
                .gaps
                .iter()
                .find(|g| g.kind == GapKind::HelperCall && g.description.contains("__chkstk_darwin"))
                .unwrap_or_else(|| panic!("{name}: no helper call to the stack probe: {:?}", o.gaps));
            assert_eq!(probe.class.as_deref(), Some("runtime-check"), "{name}");
            let ins = o.blocks.iter().flat_map(|b| &b.instrs).find(|i| i.addr == probe.addr_start).unwrap();
            assert_eq!(ins.mnemonic, "blr", "{name}: the instruction is still shown as written");
        }
    }
}

#[test]
fn goto_paths_are_edges_and_the_untaken_ones_are_uncovered() {
    let Some(ev) = ingest_with(true) else { return };
    for name in ["goto_cleanup", "goto_retry", "goto_into_loop"] {
        let o = func(&ev, name).object.as_ref().expect("object code");
        assert!(o.unreachable_blocks.is_empty(), "{name}: blocks {:?} reported unreachable", o.unreachable_blocks);
    }

    // goto_cleanup: main never fails a step, so the two labels never ran
    let o = func(&ev, "goto_cleanup").object.as_ref().unwrap();
    for label_body in ["resource_b = 0;", "resource_a = 0;"] {
        let l = line_of(&ev, label_body);
        let b = o.blocks.iter().find(|b| b.lines.contains(&l)).unwrap_or_else(|| panic!("no block for line {l}"));
        assert_eq!(b.projected, Status::Uncovered, "label code on line {l} never ran");
    }
    // and each `goto` is a jump into that code from the never-taken arm
    for g in ["goto undo_a;", "goto undo_b;"] {
        let l = line_of(&ev, g);
        assert!(o.blocks.iter().filter(|b| b.lines.contains(&l)).all(|b| b.projected == Status::Uncovered), "`{g}` on line {l} never ran");
    }

    // goto_retry: the backward goto is a back edge -- a jump to a lower address
    let o = func(&ev, "goto_retry").object.as_ref().unwrap();
    let l = line_of(&ev, "goto again;");
    let back = o
        .blocks
        .iter()
        .filter(|b| b.lines.contains(&l))
        .flat_map(|b| b.succs.iter().map(move |e| (b, e)))
        .any(|(b, e)| o.blocks[e.to].address < b.address && e.projected == Status::Covered);
    assert!(back, "the retry's backward goto is not a covered back edge");

    // goto_into_loop: the goto enters the loop body past its test, so the
    // loop has two entries; the one never taken is uncovered, and the body
    // it lands in is covered through the ordinary entry
    let o = func(&ev, "goto_into_loop").object.as_ref().unwrap();
    let goto_line = line_of(&ev, "goto inside;");
    let inside = line_of(&ev, "i++;");
    let goto_blocks: Vec<usize> = o.blocks.iter().filter(|b| b.lines.contains(&goto_line)).map(|b| b.id).collect();
    assert!(!goto_blocks.is_empty());
    let into_body = o.blocks.iter().filter(|b| goto_blocks.contains(&b.id)).flat_map(|b| &b.succs).find(|e| o.blocks[e.to].lines.contains(&inside));
    let e = into_body.expect("the goto's edge lands in the loop body");
    assert_eq!(e.projected, Status::Uncovered);
    assert_eq!(o.blocks[e.to].projected, Status::Covered, "the loop body ran through the loop's own entry");
}

#[test]
fn the_shared_return_of_a_goto_cleanup_is_not_projected_uncovered() {
    // #270: at -O0 both `return`s of goto_cleanup jump to one epilogue, which
    // DWARF gives the closing brace -- a line llvm-cov counts inside the
    // zero-count `undo_a:` label. The epilogue ran on every call, yet was
    // projected uncovered, and so was the covered `return rc;`'s jump into it.
    let Some(ev) = ingest_with(true) else { return };
    let o = func(&ev, "goto_cleanup").object.as_ref().unwrap();
    let ret_line = line_of(&ev, "    rc = 0;") + 1; // the covered `return rc;`
    let returns: Vec<&butterfly_model::Block> = o.blocks.iter().filter(|b| b.instrs.iter().any(|i| i.flags.contains(&butterfly_model::InstrFlag::Return))).collect();
    assert!(!returns.is_empty());
    for r in &returns {
        assert_ne!(r.projected, Status::Uncovered, "B{} returns, and goto_cleanup returned on every call: {:?}", r.id, r.note);
    }
    // what the evidence proves is shown: the covered return's own edge into
    // the epilogue, and the epilogue, as covered -- stated as an inference
    for b in o.blocks.iter().filter(|b| b.lines == [ret_line]) {
        for e in &b.succs {
            assert_eq!(e.projected, Status::Covered, "B{} -> B{}: the covered `return rc;` jumps here", b.id, e.to);
            let t = &o.blocks[e.to];
            assert_eq!(t.projected, Status::Covered);
            assert!(t.note.as_deref().is_some_and(|n| n.contains("inferred")), "the override of the line count is said: {:?}", t.note);
        }
    }
    // the label code only the gotos reach is still uncovered: the rule does
    // not spread coverage backwards, or across an edge nothing proves ran
    for label_body in ["resource_b = 0;", "resource_a = 0;"] {
        let l = line_of(&ev, label_body);
        assert!(o.blocks.iter().filter(|b| b.lines.contains(&l)).all(|b| b.projected == Status::Uncovered), "line {l}");
    }
}

#[test]
fn a_decision_inside_a_macro_is_placed_at_the_macro_use() {
    let Some(ev) = ingest_with(true) else { return };
    let f = func(&ev, "checked_scale");
    let o = f.object.as_ref().unwrap();
    let first = line_of(&ev, "REQUIRE(lo <= hi);");
    for l in first..first + 3 {
        // the branch region llvm-cov locates at the #define is here, at the
        // use, where the object code for it is: the `!(cond)` never held
        let b = f.branches.iter().find(|b| b.line == l).unwrap_or_else(|| panic!("no branch at the REQUIRE on line {l}: {:?}", f.branches));
        assert_eq!((b.true_count, b.false_count), (0, 1), "line {l}");
        // so the object branch on that line has a source decision
        assert!(!o.gaps.iter().any(|g| g.kind == GapKind::ExtraObjectBranch && g.line == Some(l)), "line {l}: the macro's branch is called compiler-generated");
        // and the macro's `return -1` path, whose instructions all borrow the
        // (covered) use line, is not projected covered
        assert!(o.blocks.iter().any(|b| b.lines == [l] && b.projected == Status::Uncovered), "line {l}: nothing of the never-run return is uncovered");
    }
    // nothing that only an uncovered block leads to reads covered
    let mut preds = vec![Vec::new(); o.blocks.len()];
    for b in &o.blocks {
        for e in &b.succs {
            preds[e.to].push(b.id);
        }
    }
    for b in &o.blocks {
        if !preds[b.id].is_empty() && preds[b.id].iter().all(|&p| o.blocks[p].projected == Status::Uncovered) {
            assert_ne!(b.projected, Status::Covered, "B{} is reached only from uncovered blocks but reads covered", b.id);
        }
    }
    assert!(o.instr_projected_uncovered > 0);
}

#[test]
fn an_mcdc_decision_inside_a_macro_is_placed_at_the_macro_use() {
    // #269: llvm-cov records the decision `IN_RANGE(v, 0, 10)` and its two
    // MC/DC conditions at the #define, in the expansion's own file id. The
    // decision was read as one in the body's file at the #define's line --
    // outside the function -- and given a predicated-decision gap saying no
    // object code is attributed to it. The object code is at the use.
    let Some(ev) = ingest_with(true) else { return };
    let f = func(&ev, "in_range");
    let o = f.object.as_ref().unwrap();
    let define = line_of(&ev, "#define IN_RANGE");
    let used = line_of(&ev, "if (IN_RANGE(v, 0, 10))");
    assert!(f.decisions.iter().all(|d| d.line != define), "a decision is placed at the #define: {:?}", f.decisions);
    assert_eq!(f.decisions.len(), 1, "{:?}", f.decisions);
    let d = &f.decisions[0];
    assert_eq!(d.line, used);
    assert!((f.line_start..=f.line_end).contains(&d.line), "the decision lies inside its function");
    // both conditions, from the expansion, in the order the #define writes
    // them; main passes 4, so each was true once and never false
    assert_eq!(d.conditions.len(), 2, "{:?}", d.conditions);
    for c in &d.conditions {
        assert_eq!(c.line, used);
        assert_eq!((c.true_count, c.false_count), (1, 0), "{c:?}");
        assert!(!c.independence_covered, "one run shows no condition independent");
    }
    assert!(!d.mcdc_covered);
    // and no gap claims the decision has no object code
    assert!(!o.gaps.iter().any(|g| g.kind == GapKind::PredicatedDecision), "{:?}", o.gaps.iter().filter(|g| g.kind == GapKind::PredicatedDecision).collect::<Vec<_>>());
    assert!(!o.gaps.iter().any(|g| g.line == Some(define)), "a gap at the #define line");
}

#[test]
fn the_taken_arm_of_an_if_without_else_is_not_inferred_uncovered() {
    // arm(): `if (count < 15) count++;` ran with the condition true. The join
    // after the `if` ran too, and before #168 that was read as evidence that
    // the other successor -- the path into `count++` -- had not.
    let Some(ev) = ingest_with(true) else { return };
    let o = func(&ev, "arm").object.as_ref().unwrap();
    let body = line_of(&ev, "unit_status.count++;");
    let target = o.blocks.iter().find(|b| b.lines.contains(&body)).expect("a block for count++").id;
    assert_eq!(o.blocks[target].projected, Status::Covered);
    for b in &o.blocks {
        if b.projected == Status::Uncovered {
            assert!(!b.succs.iter().any(|e| e.to == target), "B{} leads into count++, which ran, yet is projected uncovered", b.id);
        }
    }
}

#[test]
fn unions_and_bitfields_are_whole_objects() {
    let Some(ev) = ingest_with(true) else { return };
    let id = |n: &str| func(&ev, n).id;
    let g = |n: &str| ev.globals.iter().find(|g| g.name == n).unwrap_or_else(|| panic!("no global {n}"));
    // the union is one global: written as one member and read as another,
    // both are accesses of `shared_word`
    let w = g("shared_word");
    assert_eq!(w.size, 4);
    for f in ["float_bits", "low_byte"] {
        assert!(w.readers.contains(&id(f)) && w.writers.contains(&id(f)), "{f} reads and writes the union: {w:?}");
    }
    // a bitfield write is a read-modify-write of the word that holds it
    let s = g("unit_status");
    assert_eq!(s.size, 4);
    assert!(s.readers.contains(&id("arm")) && s.writers.contains(&id("arm")));
    assert!(s.readers.contains(&id("is_armed")) && !s.writers.contains(&id("is_armed")));
    let r = func(&ev, "arm").data_refs.iter().find(|r| r.global_id == s.id).unwrap();
    let mode = line_of(&ev, "unit_status.mode = mode;");
    assert!(r.lines.contains(&mode));
    assert!(r.reads >= 2 && r.writes >= 2, "each field write reads the word: {r:?}");

    // As locals, frame-slot attribution is read for AArch64 only; elsewhere
    // the variables are listed with the reason instead.
    let pack = func(&ev, "pack_local");
    let var = |n: &str| pack.variables.iter().find(|v| v.name == n).unwrap_or_else(|| panic!("no local {n}"));
    assert_eq!(var("s").type_name.as_deref(), Some("status"));
    assert_eq!(var("w").type_name.as_deref(), Some("word"));
    if ev.image.as_ref().is_some_and(|i| i.arch.starts_with("aarch64")) {
        let field_write = line_of(&ev, "    s.mode = mode;");
        let on = |l: u32, a: &str| var("s").accesses.iter().any(|x| x.line == Some(l) && x.access == a);
        assert!(on(field_write, "read") && on(field_write, "write"), "the bitfield write is a read and a write of `s`: {:?}", var("s").accesses);
        let as_float = line_of(&ev, "w.f = 2.0f;");
        let as_int = line_of(&ev, "(w.u >> 24)");
        let wa = &var("w").accesses;
        assert!(wa.iter().any(|x| x.line == Some(as_float) && x.access == "write") && wa.iter().any(|x| x.line == Some(as_int) && x.access == "read"), "{wa:?}");
    } else {
        assert!(var("s").unattributed.is_some());
    }
}

#[test]
fn a_variadic_function_has_a_bounded_frame_and_an_untracked_va_list() {
    let Some(ev) = ingest_with(true) else { return };
    let s = stack(&ev, "sum_ints");
    assert!(!s.frame.dynamic);
    assert!(s.stack.bounded, "{:?}", s.stack);
    let f = func(&ev, "sum_ints");
    assert!(f.object.as_ref().unwrap().unreachable_blocks.is_empty());
    // va_start takes the va_list's address: what va_arg does through it is
    // not attributed, and the variable says so rather than looking unused
    let ap = f.variables.iter().find(|v| v.name == "ap").expect("ap is listed");
    assert!(ap.unattributed.is_some(), "{ap:?}");
    // the loop's own decision is there, and ran both ways
    let l = line_of(&ev, "for (int i = 0; i < count; i++)");
    let b = f.branches.iter().find(|b| b.line == l).expect("the loop's decision");
    assert!(b.true_count > 0 && b.false_count > 0);
}
