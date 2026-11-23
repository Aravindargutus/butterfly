// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests (area E): the object-level control-flow graph.
//!
//! - README.md:1338-1340 "Indirect call sites (`blr`, `call *`)" are
//!   indirect calls; docs/butterfly.html Gap kinds, indirect-branch: "A jump
//!   table, tail call or function pointer: the target set must be
//!   established by analysis".
//! - README.md:1329 "Blocks with no incoming edge inside a function are
//!   flagged UNREACHABLE" -- so an edge must exist wherever control can go.
//! - matrix.toml:374 (cpp.exceptions) "an exception edge is never projected
//!   covered, because a pad's lines also run on the normal path".
//! - cfg.rs:301-321 (DivergeRule): a missing fall-through is a claim whose
//!   only authority is the callee's DW_AT_noreturn, named in the block.
//! - cfg.rs:164-168: an unconditional branch out of the function is a tail
//!   call: no successor here, and a call for the call graph.

mod common;
use butterfly_ingest::cfg::{self, build, classify, Ctl, DivergeRule};
use butterfly_ingest::objectcode::RawInstr;
use butterfly_model::{BlockKind, EdgeKind, InstrFlag, Status};
use common::ri;

fn g(arch: &str, raw: &[RawInstr], end: u64, status: &dyn Fn(usize) -> Status, pads: &dyn Fn(u64) -> Vec<u64>) -> cfg::FnCfg {
    build(arch, raw[0].addr, end, raw, &|_| (Some(1), Some(0), true), status, &|_| None, &|_| Vec::new(), pads)
}

/// README.md:1338 and the Gap kinds table: `blr` and `call *` are indirect
/// calls, `br` and `jmp *` indirect branches, on each architecture and in
/// each spelling objdump uses.
#[test]
fn indirect_transfers_are_classified_per_architecture() {
    for (arch, m, o, want) in [
        ("aarch64", "blr", "x8", Ctl::IndirectCall),
        ("aarch64", "blraa", "x8, x9", Ctl::IndirectCall),
        ("aarch64", "blraaz", "x8", Ctl::IndirectCall),
        ("aarch64", "br", "x16", Ctl::IndirectBranch),
        ("aarch64", "braaz", "x16", Ctl::IndirectBranch),
        ("aarch64", "retab", "", Ctl::Return),
        ("aarch64", "bl", "0x1000 <f>", Ctl::Call),
        ("aarch64", "b", "0x1000 <f>", Ctl::Jump),
        ("aarch64", "tbnz", "w0, #3, 0x1000 <f>", Ctl::CondBranch),
        ("aarch64", "B.NE", "0x1000 <f>", Ctl::CondBranch),
        ("x86_64", "callq", "*%rax", Ctl::IndirectCall),
        ("x86_64", "call", "*0x10(%rax)", Ctl::IndirectCall),
        ("x86_64", "callq", "*0x2fe2(%rip)           # 0x4000 <slot>", Ctl::IndirectCall),
        ("x86_64", "jmpq", "*%rax", Ctl::IndirectBranch),
        ("x86_64", "jmp", "*(%rcx,%rax,8)", Ctl::IndirectBranch),
        ("x86_64", "callq", "0x401000 <f>", Ctl::Call),
        ("x86_64", "jmp", "0x401000 <f>", Ctl::Jump),
        ("x86_64", "jne", "0x401000 <f>", Ctl::CondBranch),
        ("x86_64", "loopne", "0x401000 <f>", Ctl::CondBranch),
        ("x86_64", "retq", "", Ctl::Return),
        ("x86_64", "movl", "%eax, %ebx", Ctl::None),
    ] {
        assert_eq!(classify(arch, m, o), want, "{arch} {m} {o}");
    }
}

/// got.rs:17-21: a marked GOT transfer is a call (or, for a branch, a tail
/// call) on both architectures; the same instruction unmarked stays
/// indirect.
#[test]
fn got_marked_transfers_are_calls() {
    assert_eq!(classify("aarch64", "blr", "x16 <___chkstk_darwin@GOT>"), Ctl::Call);
    assert_eq!(classify("aarch64", "br", "x16 <ext@GOT>"), Ctl::TailCall);
    assert_eq!(classify("x86_64", "callq", "*0x2fe2(%rip) <ext@GOT>"), Ctl::Call);
    assert_eq!(classify("x86_64", "jmpq", "*0x2fe2(%rip) <ext@GOT>"), Ctl::TailCall);
    assert_eq!(cfg::target_symbol("*0x2fe2(%rip) <ext@GOT>").as_deref(), Some("ext"));
    assert_eq!(cfg::target_symbol("0x1000 <memcpy@plt>").as_deref(), Some("memcpy"));
    assert_eq!(cfg::target_symbol("0x1000 <f+0x1c>").as_deref(), Some("f"));
}

/// cfg.rs:164-168: `b f` out of the function is a tail call -- the block ends
/// with no successor here, and the branch carries the Call flag the call
/// graph reads. A `b` inside the function is an ordinary jump.
#[test]
fn a_branch_out_of_the_function_is_a_tail_call() {
    let raw = vec![ri(0x100, "cbz", "w0, 0x10c <f+0xc>"), ri(0x104, "b", "0x900 <g>"), ri(0x108, "nop", ""), ri(0x10c, "ret", "")];
    let f = g("aarch64", &raw, 0x110, &|_| Status::Covered, &|_| Vec::new());
    let tail = f.blocks.iter().find(|b| b.instrs.iter().any(|i| i.addr == 0x104)).unwrap();
    let br = tail.instrs.iter().find(|i| i.addr == 0x104).unwrap();
    assert!(br.flags.contains(&InstrFlag::Call) && br.flags.contains(&InstrFlag::Branch), "{:?}", br.flags);
    assert!(tail.succs.is_empty(), "a tail call has no in-function successor: {:?}", tail.succs);
    assert_eq!(tail.kind, BlockKind::Exit);
}

/// Gap kinds, indirect-branch: the target set of an unresolved indirect
/// branch "must be established by analysis" -- so the graph does not guess
/// one: no successors at all.
#[test]
fn an_unresolved_indirect_branch_has_no_guessed_successors() {
    let raw = vec![ri(0x100, "br", "x8"), ri(0x104, "mov", "w0, #1"), ri(0x108, "ret", "")];
    let f = g("aarch64", &raw, 0x10c, &|_| Status::Covered, &|_| Vec::new());
    assert!(f.blocks[0].succs.is_empty(), "{:?}", f.blocks[0].succs);
    assert_eq!(f.blocks[0].instrs[0].flags, vec![InstrFlag::IndirectBranch]);
    // and an indirect *call* returns: it falls through
    let raw = vec![ri(0x100, "blr", "x8"), ri(0x104, "ret", "")];
    let f = g("aarch64", &raw, 0x108, &|_| Status::Covered, &|_| Vec::new());
    assert_eq!(f.blocks.len(), 1, "a call does not end a block");
    assert_eq!(f.blocks[0].instrs[0].flags, vec![InstrFlag::IndirectCall]);
}

/// cfg.rs:301-321 "There is one rule ... the callee's own DW_AT_noreturn":
/// a call to such a callee ends its block with no fall-through, and the
/// block names the rule; a call the image says nothing about falls through,
/// whatever its name.
#[test]
fn only_dw_at_noreturn_removes_a_fall_through() {
    let raw = vec![ri(0x100, "bl", "0x900 <abort>"), ri(0x104, "bl", "0x800 <panic>"), ri(0x108, "nop", ""), ri(0x10c, "ret", "")];
    let f = build(
        "aarch64",
        0x100,
        0x110,
        &raw,
        &|_| (Some(1), Some(0), true),
        &|_| Status::Covered,
        &|t| (t == 0x800).then(|| (DivergeRule::DwarfNoreturn, "panic".to_string())),
        &|_| Vec::new(),
        &|_| Vec::new(),
    );
    // `abort` at 0x900 is not marked: it falls through into the `panic` call
    assert_eq!(f.blocks[0].instrs.len(), 2, "the unmarked call does not end the block");
    assert!(f.blocks[0].succs.is_empty(), "the marked call ends it with no fall-through");
    let why = f.blocks[0].diverges.as_deref().unwrap();
    assert!(why.contains("DW_AT_noreturn") && why.contains("panic") && why.contains("0x104"), "{why}");
    // the nop after it starts a block nothing enters
    assert_eq!(f.blocks[1].address, 0x108);
}

/// A predicated ARM call (`bleq`) may not run at all, so the instruction after
/// it is reached either way: no divergence (cfg.rs:126-138).
#[test]
fn a_predicated_call_never_diverges() {
    assert!(cfg::is_predicated("arm", "bleq"));
    assert!(!cfg::is_predicated("arm", "blal"));
    assert!(!cfg::is_predicated("aarch64", "b.eq"));
    let raw = vec![ri(0x100, "bleq", "0x800 <panic>"), ri(0x104, "bx", "lr")];
    let f =
        build("arm", 0x100, 0x108, &raw, &|_| (Some(1), Some(0), true), &|_| Status::Covered, &|_| Some((DivergeRule::DwarfNoreturn, "panic".into())), &|_| Vec::new(), &|_| {
            Vec::new()
        });
    assert!(f.blocks[0].diverges.is_none());
    assert_eq!(classify("arm", "bx", "lr"), Ctl::Return);
    assert_eq!(classify("arm", "bx", "r3"), Ctl::IndirectBranch);
    assert_eq!(classify("arm", "blx", "r3"), Ctl::IndirectCall);
}

/// matrix.toml:374 "Landing pads are read ... and drawn as exception edges,
/// so no pad is called unreachable; an exception edge is never projected
/// covered". Only the negative is sound: an uncovered pad gives an uncovered
/// edge.
#[test]
fn exception_edges_enter_pads_and_are_never_projected_covered() {
    let raw = vec![ri(0x100, "bl", "0x900 <may_throw>"), ri(0x104, "ret", ""), ri(0x108, "bl", "0x980 <cleanup>"), ri(0x10c, "ret", "")];
    let pads = |a: u64| if a == 0x100 { vec![0x108] } else { Vec::new() };
    for (pad_status, want) in [(Status::Covered, Status::Unknown), (Status::Uncovered, Status::Uncovered), (Status::Unknown, Status::Unknown)] {
        let f = g("aarch64", &raw, 0x110, &|i| if i >= 2 { pad_status } else { Status::Covered }, &pads);
        let pad = f.blocks.iter().position(|b| b.address == 0x108).expect("the pad is its own block");
        let e: Vec<_> = f.blocks[0].succs.iter().filter(|e| e.to == pad).collect();
        assert_eq!(e.len(), 1, "{:?}", f.blocks[0].succs);
        assert_eq!(e[0].kind, EdgeKind::Exception);
        assert_eq!(e[0].projected, want, "pad {pad_status:?}");
    }
    // a non-call instruction in a covered range has no exception edge: only calls throw
    let raw = vec![ri(0x100, "mov", "w0, #1"), ri(0x104, "ret", ""), ri(0x108, "ret", "")];
    let f = g("aarch64", &raw, 0x10c, &|_| Status::Covered, &|_| vec![0x108]);
    assert!(f.blocks.iter().all(|b| b.succs.iter().all(|e| e.kind != EdgeKind::Exception)));
}

/// An ordinary edge between two covered blocks is projected covered; one
/// with an uncovered end is uncovered (cfg.rs:607-609).
#[test]
fn ordinary_edges_project_from_their_ends() {
    let raw = vec![ri(0x100, "cbz", "w0, 0x108 <f+0x8>"), ri(0x104, "mov", "w0, #1"), ri(0x108, "ret", "")];
    let f = g("aarch64", &raw, 0x10c, &|i| if i == 1 { Status::Uncovered } else { Status::Covered }, &|_| Vec::new());
    let by_to = |a: u64| f.blocks[0].succs.iter().find(|e| f.blocks[e.to].address == a).unwrap().clone();
    let taken = by_to(0x108);
    assert_eq!((taken.kind, taken.projected, taken.label.as_deref()), (EdgeKind::Taken, Status::Covered, Some("w0 == 0")));
    let fall = by_to(0x104);
    assert_eq!((fall.kind, fall.projected, fall.label.as_deref()), (EdgeKind::Fallthrough, Status::Uncovered, Some("w0 != 0")));
    assert_eq!(f.cond_branches, 1);
}

/// The labels a conditional branch's outcomes carry are each other's
/// negation (the Flow view's taken / not-taken text).
#[test]
fn branch_conditions_are_complementary() {
    for (arch, m, o, t, n) in [
        ("x86_64", "jne", "0x10", "!=", "=="),
        ("x86_64", "jb", "0x10", "< (unsigned)", ">= (unsigned)"),
        ("x86_64", "jle", "0x10", "<= (signed)", "> (signed)"),
        ("aarch64", "b.hi", "0x10", "hi", "ls"),
        ("aarch64", "b.ge", "0x10", "ge", "lt"),
        ("aarch64", "tbz", "w8, #0x3, 0x10", "w8[0x3] == 0", "w8[0x3] == 1"),
        ("aarch64", "cbnz", "x1, 0x10", "x1 != 0", "x1 == 0"),
        ("arm", "bne", "0x10", "ne", "eq"),
    ] {
        assert_eq!(cfg::branch_conditions(arch, m, o), Some((t.to_string(), n.to_string())), "{arch} {m}");
    }
    assert_eq!(cfg::branch_conditions("x86_64", "jmp", "0x10"), None);
}

/// An empty function has an empty graph, not a panic.
#[test]
fn an_empty_function_has_no_blocks() {
    let f = build("aarch64", 0x100, 0x100, &[], &|_| (None, None, false), &|_| Status::Unknown, &|_| None, &|_| Vec::new(), &|_| Vec::new());
    assert!(f.blocks.is_empty() && f.cond_branches == 0);
}

/// cfg.rs:172 "the last `0x...` literal outside `<...>`" is the branch target.
#[test]
fn the_target_ignores_the_symbol_annotation() {
    assert_eq!(cfg::target("0x1000 <f+0x20>"), Some(0x1000));
    assert_eq!(cfg::target("w0, #0x3, 0x1000 <f>"), Some(0x1000));
    assert_eq!(cfg::target("*%rax"), None);
    assert_eq!(cfg::target("0xffffffffffffffffff <too_wide>"), None, "more than 64 bits is not an address");
}
