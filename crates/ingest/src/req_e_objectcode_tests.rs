// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests (area E) for objectcode.rs's private readers of
//! `llvm-objdump` output, fed the exact lines LLVM 21's objdump prints for
//! ELF images (GNU mode), which no test on this macOS host otherwise reaches.
//!
//! - README.md:1338-1349 (Function pointers) and the call graph: a call's
//!   target must carry its symbol, also in an unlinked object, where the
//!   callee lives in a relocation (matrix.toml:177, c.unlinked-object).
//! - README.md:1132-1157 (Compiler-generated instructions): every
//!   instruction is accounted for -- so a line that is not an instruction
//!   (a symbol header, a `...` gap, a relocation) must never become one.

use super::*;

/// The x86-64 ELF lines of tests/data/req_e/x86_64.s, as objdump prints them.
#[test]
fn gnu_mode_x86_64_lines_are_read() {
    let i = parse_instr_line("    13f9:      \tleaq\t-0x1088(%rip), %rcx     # 0x378 <may_throw+0x378>").unwrap();
    assert_eq!((i.addr, i.mnemonic.as_str()), (0x13f9, "leaq"));
    assert!(i.operands.starts_with("-0x1088(%rip), %rcx") && i.operands.contains("# 0x378"), "the rip target comment is kept: {:?}", i.operands);
    let i = parse_instr_line("    13d5:      \tcallq\t0x1450 <may_throw@plt>").unwrap();
    assert_eq!((i.addr, i.operands.as_str()), (0x13d5, "0x1450 <may_throw@plt>"));
    let i = parse_instr_line("    1407:      \tjmpq\t*%rax").unwrap();
    assert_eq!((i.mnemonic.as_str(), i.operands.as_str()), ("jmpq", "*%rax"));
    let i = parse_instr_line("    13e0:      \tretq").unwrap();
    assert_eq!((i.mnemonic.as_str(), i.operands.as_str()), ("retq", ""));
    // a prefix is its own mnemonic, the prefixed instruction its operands
    let i = parse_instr_line("      20:      \trep\t\tmovsb\t(%rsi), %es:(%rdi)").unwrap();
    assert_eq!((i.mnemonic.as_str(), i.operands.as_str()), ("rep", "movsb\t(%rsi), %es:(%rdi)"));
    let i = parse_instr_line("       0:      \tlock").unwrap();
    assert_eq!((i.mnemonic.as_str(), i.operands.as_str()), ("lock", ""));
}

/// AArch64 ELF lines: the `// =10` immediate comment is not an operand.
#[test]
fn gnu_mode_aarch64_lines_are_read() {
    let i = parse_instr_line("   10428:      \tmov\tw0, #0xa                // =10").unwrap();
    assert_eq!((i.addr, i.mnemonic.as_str(), i.operands.as_str()), (0x10428, "mov", "w0, #0xa"));
    let i = parse_instr_line("   10414:      \tadd\tx9, x9, #0x378").unwrap();
    assert_eq!(i.operands, "x9, x9, #0x378");
}

/// Lines that are not instructions are never instructions.
#[test]
fn non_instruction_lines_are_skipped() {
    for l in [
        "",
        "x86_64.so:\tfile format elf64-x86-64",
        "Disassembly of section .text:",
        "00000000000013d4 <thrower>:",
        "\t\t...",
        "     ...",
        "    1440:      \t<unknown>",
        "    1440:      \t",
        "    1440:      \t.byte\t0xff",
    ] {
        assert!(parse_instr_line(l).is_none(), "{l:?}");
    }
}

/// matrix.toml:177 (c.unlinked-object): in a `.o` a call's callee is the
/// relocation that follows it; data relocations are consumed but name no
/// callee. These are x86-64 ELF relocation lines exactly as objdump prints them.
#[test]
fn x86_64_elf_relocation_lines() {
    assert_eq!(parse_reloc_line("\t\t0000000000000001:  R_X86_64_PLT32\tmay_throw-0x4"), Some(Some((1, "may_throw".to_string()))));
    assert_eq!(parse_reloc_line("\t\t0000000000000010:  R_X86_64_PC32\t.data-0x4"), Some(None));
    assert_eq!(parse_reloc_line("\t\t000000000000001a:  R_X86_64_GOTPCRELX\tg-0x4"), Some(None));
    assert_eq!(parse_reloc_line("\t\t0000000000000004:  R_AARCH64_JUMP26\ttail_callee"), Some(Some((4, "tail_callee".to_string()))));
    // a relocation line naming no symbol, and one whose address is not hex
    assert_eq!(parse_reloc_line("\t\t0000000000000004:  R_AARCH64_CALL26"), Some(None));
    assert_eq!(parse_reloc_line("\t\tzz:  R_AARCH64_CALL26\tf"), None);
    // a tab-indented line that is not a relocation
    assert_eq!(parse_reloc_line("\t\t10: \tnop"), None);
}
