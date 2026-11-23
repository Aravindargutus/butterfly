// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests (area E): calls through GOT slots.
//!
//! - matrix.toml:189 (c.dynamic-frame): the stack probe's GOT slot "is
//!   resolved to the symbol the image binds it to, so the probe is a direct
//!   ... call and not a function-pointer call ... The same resolution covers
//!   ELF R_*_GLOB_DAT slots (AArch64 adrp/ldr/blr, x86-64
//!   call *foo@GOTPCREL(%rip))".
//! - got.rs:33-39 "What is not recognised stays indirect (fail closed): a
//!   slot neither table names ..., a register loaded from anywhere but a GOT
//!   slot ..., a sequence that crosses a branch target, and every other
//!   architecture."
//! - README.md:1338 "Indirect call sites (`blr`, `call *`)" are what stays.

mod common;
use butterfly_ingest::cfg::{classify, Ctl};
use butterfly_ingest::got::{annotate, bindings, Bound};
use butterfly_ingest::objectcode::RawInstr;
use common::ri;
use std::collections::HashMap;

fn import(symbol: &str) -> Bound {
    Bound { symbol: symbol.into(), addr: None }
}

fn slots() -> HashMap<u64, Bound> {
    HashMap::from([(0x4010, import("ext")), (0x4018, import("other"))])
}

fn ctl(arch: &str, i: &RawInstr) -> Ctl {
    classify(arch, &i.mnemonic, &i.operands)
}

/// got.rs:237-239: "`ldr xN, [xB]` after `add xB, xB, #off`" is the other
/// form of the slot load.
#[test]
fn aarch64_slot_formed_by_add_then_plain_ldr_is_named() {
    let mut v = vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "add", "x16, x16, #0x10"), ri(0x108, "ldr", "x16, [x16]"), ri(0x10c, "blr", "x16")];
    annotate("aarch64", &mut v, &slots());
    assert_eq!(ctl("aarch64", &v[3]), Ctl::Call, "{:?}", v[3]);
    assert!(v[3].operands.contains("<ext@GOT>"), "{:?}", v[3]);
}

/// The pointer-authenticating call forms are the same call (got.rs:237).
#[test]
fn aarch64_authenticated_call_through_a_slot_is_named() {
    let mut v = vec![ri(0x100, "adrp", "x17, 0x4000 <x>"), ri(0x104, "ldr", "x17, [x17, #0x18]"), ri(0x108, "blraaz", "x17")];
    annotate("aarch64", &mut v, &slots());
    assert_eq!(ctl("aarch64", &v[2]), Ctl::Call, "{:?}", v[2]);
    assert!(v[2].operands.contains("<other@GOT>"));
}

/// got.rs:33-39: every shape that is not "load the bound slot, branch to it"
/// stays an indirect call or branch.
#[test]
fn what_is_not_a_bound_slot_stays_indirect() {
    let cases: Vec<(&str, Vec<RawInstr>)> = vec![
        ("a slot no table names", vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "ldr", "x16, [x16, #0x20]"), ri(0x108, "blr", "x16")]),
        (
            "a pointer read through the GOT (two loads)",
            vec![ri(0x100, "adrp", "x8, 0x4000 <x>"), ri(0x104, "ldr", "x8, [x8, #0x10]"), ri(0x108, "ldr", "x1, [x8]"), ri(0x10c, "blr", "x1")],
        ),
        (
            "the register rewritten after the load",
            vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "ldr", "x16, [x16, #0x10]"), ri(0x108, "mov", "x16, x0"), ri(0x10c, "blr", "x16")],
        ),
        ("its 32-bit half rewritten", vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "ldr", "x16, [x16, #0x10]"), ri(0x108, "mov", "w16, #0x1"), ri(0x10c, "blr", "x16")]),
        (
            "the base rewritten between adrp and ldr",
            vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "mov", "x16, x2"), ri(0x108, "ldr", "x16, [x16, #0x10]"), ri(0x10c, "blr", "x16")],
        ),
        ("a control transfer between", vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "ldr", "x16, [x16, #0x10]"), ri(0x108, "bl", "0x900 <f>"), ri(0x10c, "blr", "x16")]),
        ("a register-offset load", vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "ldr", "x16, [x16, x1]"), ri(0x108, "blr", "x16")]),
        ("a pre-indexed load", vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "ldr", "x16, [x16, #0x10]!"), ri(0x108, "blr", "x16")]),
        ("a load pair", vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "ldp", "x16, x17, [x16, #0x10]"), ri(0x108, "blr", "x16")]),
        ("no adrp at all", vec![ri(0x104, "ldr", "x16, [x16, #0x10]"), ri(0x108, "blr", "x16")]),
        ("the slot loaded too far back", {
            let mut v = vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "ldr", "x16, [x16, #0x10]")];
            v.extend((0..9).map(|k| ri(0x108 + 4 * k, "add", "x0, x0, #0x1")));
            v.push(ri(0x12c, "blr", "x16"));
            v
        }),
    ];
    for (why, mut v) in cases {
        annotate("aarch64", &mut v, &slots());
        let last = v.last().unwrap();
        assert_eq!(ctl("aarch64", last), Ctl::IndirectCall, "{why}: {last:?}");
        assert!(!last.operands.contains("@GOT"), "{why}: {last:?}");
    }
}

/// got.rs:192-194 "A sequence is matched only when nothing can enter it
/// part-way": a branch anywhere in the image to the `ldr` or the `blr` leaves
/// the call indirect.
#[test]
fn a_sequence_entered_part_way_stays_indirect() {
    for target in ["0x104", "0x108"] {
        let mut v = vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "ldr", "x16, [x16, #0x10]"), ri(0x108, "blr", "x16"), ri(0x200, "b", &format!("{target} <f>"))];
        annotate("aarch64", &mut v, &slots());
        assert_eq!(ctl("aarch64", &v[2]), Ctl::IndirectCall, "entered at {target}");
    }
    // a branch to the adrp itself enters the whole sequence: still a GOT call
    let mut v = vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "ldr", "x16, [x16, #0x10]"), ri(0x108, "blr", "x16"), ri(0x200, "b", "0x100 <f>")];
    annotate("aarch64", &mut v, &slots());
    assert_eq!(ctl("aarch64", &v[2]), Ctl::Call);
}

/// matrix.toml:189 "x86-64 `call *foo@GOTPCREL(%rip)`": the slot objdump
/// computes is in the `# 0x...` comment; a call and a jump through it are a
/// call and a tail call. With no comment, or not rip-relative, no match.
#[test]
fn x86_64_slots_are_read_from_the_rip_comment() {
    let mut v = vec![
        ri(0x100, "callq", "*0x3f0a(%rip)           # 0x4010 <foo+0x10>"),
        ri(0x106, "jmpq", "*0x3f0c(%rip)           # 0x4018"),
        ri(0x10c, "callq", "*0x3f0a(%rip)"),
        ri(0x112, "callq", "*0x10(%rax)             # 0x4010"),
        ri(0x118, "callq", "*0x3f0a(%rip)           # 0x4020 <x>"),
    ];
    annotate("x86_64", &mut v, &slots());
    assert_eq!(ctl("x86_64", &v[0]), Ctl::Call, "{:?}", v[0]);
    assert!(v[0].operands.ends_with("<ext@GOT>") && !v[0].operands.contains("foo"), "objdump's guessed symbol is replaced: {:?}", v[0]);
    assert_eq!(ctl("x86_64", &v[1]), Ctl::TailCall);
    for i in &v[2..] {
        assert_eq!(ctl("x86_64", i), Ctl::IndirectCall, "{i:?}");
    }
}

/// got.rs:38-39 "and every other architecture".
#[test]
fn other_architectures_are_left_alone() {
    let mut v = vec![ri(0x100, "adrp", "x16, 0x4000 <x>"), ri(0x104, "ldr", "x16, [x16, #0x10]"), ri(0x108, "blr", "x16")];
    let before = v.clone();
    annotate("riscv64", &mut v, &slots());
    assert_eq!(v.iter().map(|i| &i.operands).collect::<Vec<_>>(), before.iter().map(|i| &i.operands).collect::<Vec<_>>());
    // no bindings: nothing to do
    annotate("aarch64", &mut v, &HashMap::new());
    assert_eq!(v[2].operands, "x16");
}

/// got.rs:61-68: an image format with no GOT bindings this reader knows (a
/// COFF object) and an ELF object with no dynamic relocations bind nothing.
#[test]
fn formats_without_bindings_bind_nothing() {
    // the smallest COFF object: an x86-64 header, no sections, no symbols
    let mut coff = vec![0x64, 0x86];
    coff.extend([0u8; 18]);
    let f = object::File::parse(&*coff).expect("a COFF header");
    assert!(bindings(&f).is_empty());
    // an unlinked ELF object (relocatable): no dynamic relocations
    let mut elf = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    elf.extend(1u16.to_le_bytes()); // ET_REL
    elf.extend(62u16.to_le_bytes()); // EM_X86_64
    elf.extend(1u32.to_le_bytes());
    elf.extend([0u8; 24]); // entry, phoff, shoff
    elf.extend(0u32.to_le_bytes()); // flags
    elf.extend(64u16.to_le_bytes()); // ehsize
    elf.extend([0u8; 10]); // phentsize, phnum, shentsize, shnum, shstrndx
    let f = object::File::parse(&*elf).expect("an ELF header");
    assert!(bindings(&f).is_empty());
}

/// got.rs:24-31: "A `.got` slot whose addend is the start of a function
/// symbol is bound to that function exactly as a `GLOB_DAT` slot is to its
/// symbol, and its mark also carries the address" -- rustc's x86-64 Linux
/// `call *slot(%rip)` to a function linked into the same PIE. The call is a
/// direct call to that address, not a function-pointer call.
#[test]
fn x86_64_relative_slot_call_names_the_function_and_its_address() {
    let got = HashMap::from([(0x559c0, Bound { symbol: "callee".into(), addr: Some(0x50c10) })]);
    let mut v = vec![ri(0x151c8, "callq", "*0x407f2(%rip)          # 0x559c0 <writev+0x559c0>"), ri(0x151ce, "jmpq", "*0x407ec(%rip)          # 0x559c0")];
    annotate("x86_64", &mut v, &got);
    assert_eq!(ctl("x86_64", &v[0]), Ctl::Call, "{:?}", v[0]);
    assert_eq!(butterfly_ingest::cfg::target(&v[0].operands), Some(0x50c10), "{:?}", v[0]);
    assert_eq!(butterfly_ingest::cfg::target_symbol(&v[0].operands).as_deref(), Some("callee"));
    assert!(!v[0].operands.contains("writev"), "objdump's guessed symbol is replaced: {:?}", v[0]);
    assert_eq!(ctl("x86_64", &v[1]), Ctl::TailCall);
    assert_eq!(butterfly_ingest::cfg::target(&v[1].operands), Some(0x50c10));
}
