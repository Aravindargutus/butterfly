// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests (area E): switch jump tables.
//!
//! - matrix.toml:74 (c.switch): "the table is read from the image, each case
//!   is an edge from the indirect branch, no case block is called
//!   unreachable ... Only clang's table shapes are read (AArch64 word,
//!   halfword and byte entries; x86-64 32-bit offsets) ... Any other shape
//!   leaves the branch unresolved."
//! - docs/butterfly.html, Gap kinds, indirect-branch: "A switch's jump table
//!   is read from the image and its targets drawn as edges".
//! - jumptable.rs:15-19: "only when every piece is found ... Anything less is
//!   not a table this module claims to have read".
//! - Malformed input fails closed, never a panic.

mod common;
use butterfly_ingest::cfg;
use butterfly_ingest::jumptable::recover;
use butterfly_ingest::objectcode::RawInstr;
use butterfly_model::{EdgeKind, Status};
use common::ri;

fn reader(at: u64, bytes: Vec<u8>) -> impl Fn(u64, usize) -> Option<Vec<u8>> {
    move |a, n| {
        let off = usize::try_from(a.checked_sub(at)?).ok()?;
        bytes.get(off..off.checked_add(n)?).map(<[u8]>::to_vec)
    }
}

/// clang -O1+ on AArch64, `count` cases, entries of `width` bytes loaded by
/// `load`; the bound is `cmp w0, #bound`.
fn a64(bound: &str, load: &str, shift: &str) -> Vec<RawInstr> {
    vec![
        ri(0x1000, "cmp", &format!("w0, #{bound}")),
        ri(0x1004, "b.hi", "0x1040 <f+0x40>"),
        ri(0x1008, "mov", "w8, w0"),
        ri(0x100c, "adrp", "x9, 0x2000 <tbl>"),
        ri(0x1010, "add", "x9, x9, #0x10"),
        ri(0x1014, "adr", "x10, 0x1024 <f+0x24>"),
        ri(0x1018, load, "w11, [x9, x8, lsl #1]"),
        ri(0x101c, "add", &format!("x10, x10, x11{shift}")),
        ri(0x1020, "br", "x10"),
        ri(0x1024, "mov", "w0, #1"),
        ri(0x1028, "ret", ""),
        ri(0x102c, "mov", "w0, #2"),
        ri(0x1030, "ret", ""),
        ri(0x1034, "mov", "w0, #3"),
        ri(0x1038, "ret", ""),
        ri(0x103c, "nop", ""),
        ri(0x1040, "mov", "w0, wzr"),
        ri(0x1044, "ret", ""),
    ]
}

/// matrix.toml:74 "AArch64 ... halfword ... entries": halfword offsets,
/// scaled by four, from the `adr` base.
#[test]
fn aarch64_halfword_entries_are_read() {
    let raw = a64("0x2", "ldrh", ", lsl #2");
    let t = recover("aarch64", &raw, 0x1000, 0x1048, &reader(0x2010, vec![0, 0, 2, 0, 4, 0]));
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!((t[0].branch, t[0].addr, t[0].len), (0x1020, 0x2010, 6));
    assert_eq!(t[0].targets, vec![0x1024, 0x102c, 0x1034]);
}

/// "each case is an edge from the indirect branch, no case block is called
/// unreachable" (matrix.toml:74): the table's targets become `Indirect`
/// edges, one per distinct case block, even when cases share a target.
#[test]
fn every_case_is_an_indirect_edge() {
    let raw = a64("0x3", "ldrh", ", lsl #2");
    // four entries, two of them the same case
    let bytes = vec![0, 0, 2, 0, 2, 0, 4, 0];
    let t = recover("aarch64", &raw, 0x1000, 0x1048, &reader(0x2010, bytes));
    assert_eq!(t.len(), 1);
    let targets = t[0].targets.clone();
    let g = cfg::build(
        "aarch64",
        0x1000,
        0x1048,
        &raw,
        &|_| (Some(1), Some(0), true),
        &|_| Status::Covered,
        &|_| None,
        &|a| if a == 0x1020 { targets.clone() } else { Vec::new() },
        &|_| Vec::new(),
    );
    let br = g.blocks.iter().find(|b| b.instrs.last().is_some_and(|i| i.addr == 0x1020)).unwrap();
    let to: Vec<u64> = br
        .succs
        .iter()
        .map(|e| {
            assert_eq!(e.kind, EdgeKind::Indirect);
            g.blocks[e.to].address
        })
        .collect();
    assert_eq!(to, vec![0x1024, 0x102c, 0x1034], "one edge per distinct case");
    // and every case block has a predecessor
    for a in [0x1024, 0x102c, 0x1034] {
        let id = g.blocks.iter().position(|b| b.address == a).unwrap();
        assert!(g.blocks.iter().any(|b| b.succs.iter().any(|e| e.to == id)), "{a:#x} has no predecessor");
    }
}

/// jumptable.rs:15-19 and matrix.toml:74 "Any other shape leaves the branch
/// unresolved".
#[test]
fn any_other_shape_leaves_the_branch_unresolved() {
    let bytes = || reader(0x2010, vec![0, 0, 2, 0, 4, 0]);
    let none = |raw: &[RawInstr], why: &str| assert!(recover("aarch64", raw, 0x1000, 0x1048, &bytes()).is_empty(), "{why}");
    // no bounds check
    let mut r = a64("0x2", "ldrh", ", lsl #2");
    r.remove(1);
    none(&r, "no b.hi");
    // a bound compared with something other than cmp/subs
    let mut r = a64("0x2", "ldrh", ", lsl #2");
    r[0] = ri(0x1000, "tst", "w0, #0x2");
    none(&r, "tst is not a bound");
    // a signed-byte entry load is not a shape clang emits
    none(&a64("0x2", "ldrsb", ", lsl #2"), "ldrsb");
    // a base that is not formed by adr
    let mut r = a64("0x2", "ldrh", ", lsl #2");
    r[5] = ri(0x1014, "mov", "x10, x9");
    none(&r, "base from mov");
    // the table address from adr alone (what lld's relaxation leaves: nop; adr)
    let mut r = a64("0x2", "ldrh", ", lsl #2");
    r[3] = ri(0x100c, "nop", "");
    r[4] = ri(0x1010, "adr", "x9, 0x2010 <tbl>");
    none(&r, "relaxed adrp+add");
    // a shift that is not `lsl #n`
    none(&a64("0x2", "ldrh", ", asr #2"), "asr");
    // the table cannot be read
    assert!(recover("aarch64", &a64("0x2", "ldrh", ", lsl #2"), 0x1000, 0x1048, &|_, _| None).is_empty(), "unreadable table");
    // a target outside the function
    assert!(recover("aarch64", &a64("0x2", "ldrh", ", lsl #2"), 0x1000, 0x1030, &bytes()).is_empty(), "a case past the function's end");
    // an architecture whose shapes are not read
    assert!(recover("riscv64", &a64("0x2", "ldrh", ", lsl #2"), 0x1000, 0x1048, &bytes()).is_empty(), "riscv");
    assert!(recover("arm", &a64("0x2", "ldrh", ", lsl #2"), 0x1000, 0x1048, &bytes()).is_empty(), "arm");
}

/// jumptable.rs:15-19, and fail closed: an implausible entry count is not a
/// table -- neither the largest a compare can encode nor one past it.
#[test]
fn an_implausible_count_is_not_a_table() {
    let all = || reader(0x2010, vec![0; 0x10000]);
    assert!(recover("aarch64", &a64("0x1000", "ldrh", ", lsl #2"), 0x1000, 0x1048, &all()).is_empty(), "4097 entries");
    assert!(recover("aarch64", &a64("0xfffffffe", "ldrh", ", lsl #2"), 0x1000, 0x1048, &all()).is_empty());
}

/// Fail closed on the largest immediate: the entry count is the bound plus
/// one, which must not overflow.
#[test]
fn a_bound_of_u64_max_is_not_a_table_and_does_not_panic() {
    let raw = a64("0xffffffffffffffff", "ldrh", ", lsl #2");
    assert!(recover("aarch64", &raw, 0x1000, 0x1048, &reader(0x2010, vec![0; 64])).is_empty());
}

/// Fail closed on a shift no instruction encodes: `lsl #64` (or more) is not
/// a scaling clang emits, so the branch stays unresolved -- not a panic.
#[test]
fn an_oversized_shift_is_not_a_table_and_does_not_panic() {
    let raw = a64("0x2", "ldrh", ", lsl #64");
    assert!(recover("aarch64", &raw, 0x1000, 0x1048, &reader(0x2010, vec![0, 0, 2, 0, 4, 0])).is_empty());
}

/// matrix.toml:74 "x86-64 32-bit offsets" -- with the bound given by a
/// `subq` (jumptable.rs:44 "or subq $0xf, %rax"), as objdump prints a
/// Mach-O table: `## 0x...` comment.
#[test]
fn x86_64_bound_by_sub_and_a_macho_comment() {
    let raw = vec![
        ri(0x100, "movl", "%edi, %eax"),
        ri(0x102, "subq", "$0x2, %rax"),
        ri(0x106, "ja", "0x140 <f+0x40>"),
        ri(0x108, "leaq", "0x31(%rip), %rcx         ## 0x140"),
        ri(0x10f, "movslq", "(%rcx,%rax,4), %rax"),
        ri(0x113, "addq", "%rcx, %rax"),
        ri(0x116, "jmpq", "*%rax"),
        ri(0x118, "retq", ""),
        ri(0x120, "retq", ""),
        ri(0x128, "retq", ""),
    ];
    let offs: Vec<u8> = [-0x28i32, -0x20, -0x18].iter().flat_map(|o| o.to_le_bytes()).collect();
    let t = recover("x86_64", &raw, 0x100, 0x150, &reader(0x140, offs));
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!((t[0].branch, t[0].addr, t[0].len), (0x116, 0x140, 12));
    assert_eq!(t[0].targets, vec![0x118, 0x120, 0x128]);
}

/// matrix.toml:74 "Only clang's table shapes are read": a table of 64-bit
/// absolute addresses (`jmpq *0x...(,%rax,8)`, GCC's non-PIC shape) or a
/// scale other than 4 leaves the branch unresolved.
#[test]
fn x86_64_other_shapes_are_unresolved() {
    let base = |load: RawInstr| {
        vec![
            ri(0x100, "cmpl", "$0x2, %edi"),
            ri(0x103, "ja", "0x140 <f+0x40>"),
            ri(0x105, "leaq", "0x31(%rip), %rcx         # 0x140"),
            load,
            ri(0x110, "addq", "%rcx, %rax"),
            ri(0x113, "jmpq", "*%rax"),
            ri(0x118, "retq", ""),
        ]
    };
    let rd = || reader(0x140, vec![0; 64]);
    assert!(recover("x86_64", &base(ri(0x10c, "movslq", "(%rcx,%rax,8), %rax")), 0x100, 0x150, &rd()).is_empty(), "scale 8");
    assert!(recover("x86_64", &base(ri(0x10c, "movl", "(%rcx,%rax,4), %eax")), 0x100, 0x150, &rd()).is_empty(), "movl, not movslq");
    let gcc = vec![ri(0x100, "cmpl", "$0x2, %edi"), ri(0x103, "ja", "0x140"), ri(0x105, "jmpq", "*0x402000(,%rdi,8)")];
    assert!(recover("x86_64", &gcc, 0x100, 0x150, &rd()).is_empty(), "absolute table");
    // a jmp through a register nothing loaded
    assert!(recover("x86_64", &[ri(0x100, "jmpq", "*%rax")], 0x100, 0x150, &rd()).is_empty(), "bare indirect jump");
}

/// matrix.toml:74 "x86-64 32-bit offsets" on ELF: GNU-mode llvm-objdump ends
/// the `lea` with a single-`#` comment (`leaq -0x1088(%rip), %rcx  # 0x378
/// <sym+0x378>`), where Mach-O mode writes `##`. The table is the same table.
#[test]
fn x86_64_elf_comment_form_is_read() {
    let raw = vec![
        ri(0x13f2, "cmpl", "$0x3, %edi"),
        ri(0x13f5, "ja", "0x1421 <dispatch+0x2f>"),
        ri(0x13f7, "movl", "%edi, %eax"),
        ri(0x13f9, "leaq", "-0x1088(%rip), %rcx     # 0x378 <may_throw+0x378>"),
        ri(0x1400, "movslq", "(%rcx,%rax,4), %rax"),
        ri(0x1404, "addq", "%rcx, %rax"),
        ri(0x1407, "jmpq", "*%rax"),
        ri(0x1409, "retq", ""),
        ri(0x140f, "retq", ""),
        ri(0x1415, "retq", ""),
        ri(0x141b, "retq", ""),
        ri(0x1421, "retq", ""),
    ];
    let offs: Vec<u8> = [0x1091i32, 0x1097, 0x109d, 0x10a3].iter().flat_map(|o| o.to_le_bytes()).collect();
    let t = recover("x86_64", &raw, 0x13f2, 0x1424, &reader(0x378, offs));
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!(t[0].targets, vec![0x1409, 0x140f, 0x1415, 0x141b]);
}
