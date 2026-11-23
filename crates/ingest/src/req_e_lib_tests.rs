// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests (area E) for the projection steps in lib.rs that
//! tag compiler-generated instructions and flag unreachable blocks.
//!
//! - README.md:1146-1152 "Every instruction carries an *origin*: directly
//!   traceable to a source statement of its function, or compiler-generated
//!   -- `prologue`, `epilogue`, `stack-protector`, `runtime-check`
//!   (bounds/overflow/panic helpers and the compare+branch guarding them),
//!   `line-0`, `no-line`, `padding`, or `inlined:<callee>`."
//! - README.md:1329 "Blocks with no incoming edge inside a function are
//!   flagged `UNREACHABLE`."

use super::*;

fn ins(addr: u64, m: &str, o: &str, line: Option<u32>) -> Instr {
    let flags = match cfg::classify("x86_64", m, o) {
        cfg::Ctl::Call => vec![InstrFlag::Call],
        cfg::Ctl::Return => vec![InstrFlag::Return],
        cfg::Ctl::CondBranch => vec![InstrFlag::CondBranch],
        _ => vec![],
    };
    Instr { addr, mnemonic: m.into(), operands: o.into(), line, flags, ..Default::default() }
}

fn a64(addr: u64, m: &str, o: &str, line: Option<u32>) -> Instr {
    let flags = match cfg::classify("aarch64", m, o) {
        cfg::Ctl::Call => vec![InstrFlag::Call],
        cfg::Ctl::Return => vec![InstrFlag::Return],
        cfg::Ctl::CondBranch => vec![InstrFlag::CondBranch],
        _ => vec![],
    };
    Instr { addr, mnemonic: m.into(), operands: o.into(), line, flags, ..Default::default() }
}

fn block(id: usize, instrs: Vec<Instr>, succs: &[(usize, EdgeKind)]) -> Block {
    Block {
        id,
        address: instrs.first().map_or(0, |i| i.addr),
        instrs,
        succs: succs.iter().map(|&(to, kind)| Edge { to, kind, ..Default::default() }).collect(),
        ..Default::default()
    }
}

fn origins(b: &[Block]) -> Vec<(u64, Option<String>)> {
    b.iter().flat_map(|b| &b.instrs).map(|i| (i.addr, i.origin.clone())).collect()
}

fn origin_at(b: &[Block], addr: u64) -> Option<String> {
    b.iter().flat_map(|b| &b.instrs).find(|i| i.addr == addr).and_then(|i| i.origin.clone())
}

/// README.md:1146-1152 on x86-64: frame set-up and tear-down, a
/// stack-protector check, line 0, no line, and statements of the function.
#[test]
fn x86_64_origins() {
    let mut b = vec![
        block(
            0,
            vec![
                ins(0x00, "pushq", "%rbp", Some(3)),
                ins(0x01, "movq", "%rsp, %rbp", Some(3)),
                ins(0x04, "subq", "$0x20, %rsp", Some(3)),
                ins(0x08, "movl", "%edi, -0x4(%rbp)", Some(4)),
                ins(0x0b, "movl", "$0x0, %eax", Some(0)),
                ins(0x10, "movq", "%rax, %rcx", None),
                ins(0x13, "callq", "0x900 <__stack_chk_fail>", Some(9)),
            ],
            &[(1, EdgeKind::Fallthrough)],
        ),
        block(1, vec![ins(0x18, "addq", "$0x20, %rsp", Some(9)), ins(0x1c, "popq", "%rbp", Some(9)), ins(0x1d, "retq", "", Some(9))], &[]),
    ];
    classify_origins("x86_64", &mut b, &[], &[]);
    assert_eq!(
        origins(&b),
        vec![
            (0x00, Some("prologue".into())),
            (0x01, Some("prologue".into())),
            (0x04, Some("prologue".into())),
            (0x08, None),
            (0x0b, Some("line-0".into())),
            (0x10, Some("no-line".into())),
            (0x13, Some("stack-protector".into())),
            (0x18, Some("epilogue".into())),
            (0x1c, Some("epilogue".into())),
            (0x1d, None),
        ]
    );
}

/// README.md:1149 `padding`: alignment no-ops with no source line. x86-64
/// pads with the multi-byte `nop` forms objdump prints as `nopl`/`nopw`
/// (`nopw %cs:0x0(%rax,%rax)`), and with `int3`.
#[test]
fn x86_64_alignment_padding_is_padding() {
    let mut b = vec![block(
        0,
        vec![
            ins(0x00, "retq", "", Some(3)),
            ins(0x01, "nopw", "%cs:0x0(%rax,%rax)", None),
            ins(0x0b, "nopl", "(%rax)", None),
            ins(0x0e, "int3", "", None),
            ins(0x0f, "nop", "", None),
        ],
        &[],
    )];
    classify_origins("x86_64", &mut b, &[], &[]);
    for a in [0x01, 0x0b, 0x0e, 0x0f] {
        assert_eq!(origin_at(&b, a).as_deref(), Some("padding"), "{a:#x}");
    }
}

/// The bare forms on both architectures are padding.
#[test]
fn bare_nops_and_traps_without_a_line_are_padding() {
    let mut b = vec![block(0, vec![a64(0x0, "ret", "", Some(3)), a64(0x4, "nop", "", None), a64(0x8, "udf", "#0x0", Some(0)), a64(0xc, "brk", "#0x1", None)], &[])];
    classify_origins("aarch64", &mut b, &[], &[]);
    for a in [0x4, 0x8, 0xc] {
        assert_eq!(origin_at(&b, a).as_deref(), Some("padding"), "{a:#x}");
    }
}

/// README.md:1146-1152 on AArch64: the frame record pushed and popped, the
/// frame pointer set; an inlined callee's code is `inlined:<callee>`.
#[test]
fn aarch64_origins_and_inlined_code() {
    let inl = Inlined { name: "clamp".into(), function_id: None, call_line: Some(5), ranges: vec![(0x0c, 0x10)], instr_count: 1, depth: 1 };
    let mut b = vec![block(
        0,
        vec![
            a64(0x00, "stp", "x29, x30, [sp, #-0x10]!", Some(3)),
            a64(0x04, "mov", "x29, sp", Some(3)),
            a64(0x08, "add", "w0, w0, #0x1", Some(4)),
            Instr { inlined: Some(0), ..a64(0x0c, "csel", "w0, w0, w1, lt", Some(20)) },
            a64(0x10, "ldp", "x29, x30, [sp], #0x10", Some(6)),
            a64(0x14, "ret", "", Some(6)),
        ],
        &[],
    )];
    classify_origins("aarch64", &mut b, &[inl], &[]);
    assert_eq!(origin_at(&b, 0x00).as_deref(), Some("prologue"));
    assert_eq!(origin_at(&b, 0x04).as_deref(), Some("prologue"));
    assert_eq!(origin_at(&b, 0x08), None);
    assert_eq!(origin_at(&b, 0x0c).as_deref(), Some("inlined:clamp"));
    assert_eq!(origin_at(&b, 0x10).as_deref(), Some("epilogue"));
}

/// README.md:1148 "`runtime-check` (bounds/overflow/panic helpers and the
/// compare+branch guarding them)".
#[test]
fn a_runtime_check_and_its_guard() {
    let mut b = vec![
        block(
            0,
            vec![a64(0x00, "add", "w8, w0, #0x1", Some(4)), a64(0x04, "cmp", "x8, #0x4", Some(4)), a64(0x08, "b.hs", "0x14 <f+0x14>", Some(4))],
            &[(2, EdgeKind::Taken), (1, EdgeKind::Fallthrough)],
        ),
        block(1, vec![a64(0x0c, "ldr", "w0, [x1, x8, lsl #2]", Some(4)), a64(0x10, "ret", "", Some(5))], &[]),
        block(2, vec![a64(0x14, "bl", "0x900 <core::panicking::panic_bounds_check>", Some(4))], &[]),
    ];
    let gap = Gap { kind: GapKind::HelperCall, addr_start: 0x14, addr_end: 0x18, class: Some("runtime-check".into()), ..Default::default() };
    classify_origins("aarch64", &mut b, &[], &[gap]);
    assert_eq!(origin_at(&b, 0x14).as_deref(), Some("runtime-check"), "the helper call");
    assert_eq!(origin_at(&b, 0x04).as_deref(), Some("runtime-check"), "the compare guarding it");
    assert_eq!(origin_at(&b, 0x08).as_deref(), Some("runtime-check"), "the branch guarding it");
    assert_eq!(origin_at(&b, 0x0c), None, "the guarded access is the statement");
}

/// README.md:1148: only "the compare+branch guarding" a check is
/// runtime-check. The statement's own arithmetic just before the compare
/// (`i + 1` here) is traceable to the source, not compiler-generated.
#[test]
fn only_the_compare_and_branch_guard_a_check() {
    let mut b = vec![
        block(
            0,
            vec![
                a64(0x00, "mov", "x9, x1", Some(3)),
                a64(0x04, "add", "w8, w0, #0x1", Some(4)),
                a64(0x08, "cmp", "x8, #0x4", Some(4)),
                a64(0x0c, "b.hs", "0x18 <f+0x18>", Some(4)),
            ],
            &[(2, EdgeKind::Taken), (1, EdgeKind::Fallthrough)],
        ),
        block(1, vec![a64(0x10, "ldr", "w0, [x9, x8, lsl #2]", Some(4)), a64(0x14, "ret", "", Some(5))], &[]),
        block(2, vec![a64(0x18, "bl", "0x900 <core::panicking::panic_bounds_check>", Some(4))], &[]),
    ];
    let gap = Gap { kind: GapKind::HelperCall, addr_start: 0x18, addr_end: 0x1c, class: Some("runtime-check".into()), ..Default::default() };
    classify_origins("aarch64", &mut b, &[], &[gap]);
    assert_eq!(origin_at(&b, 0x08).as_deref(), Some("runtime-check"));
    assert_eq!(origin_at(&b, 0x0c).as_deref(), Some("runtime-check"));
    assert_eq!(origin_at(&b, 0x04), None, "`i + 1` is the statement's");
}

/// README.md:1329: a block nothing enters (other than the entry) is
/// unreachable; a jump, a fall-through, a table edge or an exception edge
/// each enter one.
#[test]
fn blocks_with_no_incoming_edge_are_unreachable() {
    let r = |m: &str| vec![a64(0, m, "", Some(1))];
    let b = vec![
        block(0, r("cbz"), &[(1, EdgeKind::Taken), (2, EdgeKind::Fallthrough), (4, EdgeKind::Exception)]),
        block(1, r("ret"), &[]),
        block(2, r("br"), &[(5, EdgeKind::Indirect)]),
        block(3, r("ret"), &[]),
        block(4, r("ret"), &[]),
        block(5, r("ret"), &[]),
    ];
    assert_eq!(unreachable(&b), vec![3]);
    // the entry block is never unreachable, even with no edge into it
    assert!(unreachable(&[block(0, r("ret"), &[])]).is_empty());
}
