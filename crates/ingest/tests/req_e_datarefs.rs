// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests (area E): data coupling.
//!
//! README.md:246-252: "Data coupling is extracted from address-forming
//! sequences: `adrp`+`add`/`ldr`/`str` on AArch64 and on x86-64 both a
//! direct RIP-relative operand and the far more common clang pattern of
//! materialising the address with `lea` and then loading or storing through
//! that register -- tracked until the register is overwritten, a call
//! clobbers it, or control flow joins. A read-modify-write (`incl (%rax)`,
//! `addl $1, g(%rip)`) counts as both a read and a write."
//!
//! docs/butterfly.html, The other analyses: "Address-forming instruction
//! sequences are decoded per architecture to find which globals each
//! function reads, writes or takes the address of".
//!
//! Instruction text is exactly what `llvm-objdump -d --no-show-raw-insn
//! --print-imm-hex` prints (checked against LLVM 21's output for each
//! prefix form below): in particular a `rep`/`lock` prefix is its own
//! mnemonic, with the instruction it prefixes in the operand text.

mod common;
use butterfly_ingest::datarefs::{scan, Access};
use butterfly_ingest::objectcode::RawInstr;
use common::ri;

const G: u64 = 0x404040;
const H: u64 = 0x404080;

/// Two globals, 64 bytes each.
fn lookup(a: u64) -> Option<u64> {
    [G, H].into_iter().find(|&g| a >= g && a < g + 0x40)
}

fn acc(arch: &str, v: &[RawInstr]) -> Vec<(u64, u64, Access)> {
    scan(arch, v, &lookup).into_iter().map(|d| (d.addr, d.target, d.access)).collect()
}

fn lea(addr: u64, reg: &str) -> RawInstr {
    ri(addr, "leaq", &format!("0x1234(%rip), {reg}  # 0x404040 <g>"))
}

// ----------------------------------------------------------------- x86-64

/// "a direct RIP-relative operand": a load reads, a store writes, a compare
/// reads, a read-modify-write does both.
#[test]
fn x86_64_rip_relative_accesses() {
    let v = [
        ri(0, "movl", "0x1234(%rip), %eax  # 0x404040 <g>"),
        ri(6, "movl", "$0x1, 0x1234(%rip)  # 0x404084 <h+0x4>"),
        ri(16, "cmpl", "$0x0, 0x1234(%rip)  # 0x404040 <g>"),
        ri(23, "addl", "$0x1, 0x1234(%rip)  # 0x404040 <g>"),
        ri(30, "pushq", "0x1234(%rip)  # 0x404040 <g>"),
        ri(36, "popq", "0x1234(%rip)  # 0x404080 <h>"),
    ];
    assert_eq!(
        acc("x86_64", &v),
        vec![(0, G, Access::Read), (6, H, Access::Write), (16, G, Access::Read), (23, G, Access::Read), (23, G, Access::Write), (30, G, Access::Read), (36, H, Access::Write),]
    );
}

/// "materialising the address with lea and then loading or storing through
/// that register", including through a narrower name of the same register
/// and with a displacement inside the global.
#[test]
fn x86_64_lea_then_access_through_the_register() {
    let v = [lea(0, "%rax"), ri(7, "movl", "0x8(%rax), %ecx"), ri(10, "movb", "$0x1, (%rax)"), ri(13, "incl", "0x4(%rax)")];
    assert_eq!(acc("x86_64", &v), vec![(0, G, Access::Address), (7, G, Access::Read), (10, G, Access::Write), (13, G, Access::Read), (13, G, Access::Write)]);
}

/// "tracked until the register is overwritten": a write to any width of the
/// register ends it.
#[test]
fn x86_64_tracking_ends_when_the_register_is_overwritten() {
    for w in ["movl $0x0, %eax", "xorl %eax, %eax", "movb $0x1, %al", "popq %rax", "cltq", "cqto"] {
        let (m, o) = w.split_once(' ').unwrap_or((w, ""));
        let v = [lea(0, "%rax"), ri(7, m, o), ri(10, "movl", "(%rax), %ecx")];
        let r = acc("x86_64", &v);
        assert!(!r.iter().any(|x| x.0 == 10), "{w}: {r:?}");
    }
}

/// "a call clobbers it" -- a caller-saved register. A callee-saved one is
/// preserved by the ABI and keeps the address (datarefs.rs:204-210).
#[test]
fn x86_64_a_call_clobbers_caller_saved_registers_only() {
    let v = [lea(0, "%rax"), lea(7, "%rbx"), ri(14, "callq", "0x401000 <f>"), ri(19, "movl", "(%rax), %ecx"), ri(21, "movl", "%ecx, (%rbx)")];
    let r = acc("x86_64", &v);
    assert!(!r.iter().any(|x| x.0 == 19), "%rax does not survive a call: {r:?}");
    assert!(r.contains(&(21, G, Access::Write)), "%rbx does: {r:?}");
}

/// "or control flow joins": a branch ends tracking.
#[test]
fn x86_64_tracking_ends_at_a_branch() {
    let v = [lea(0, "%rax"), ri(7, "jne", "0x20 <f+0x20>"), ri(9, "movl", "(%rax), %ecx")];
    assert!(!acc("x86_64", &v).iter().any(|x| x.0 == 9));
}

/// "or control flow joins": an instruction some branch targets is a join --
/// the register may hold anything on the other path -- so an address formed
/// before it is not tracked into it.
#[test]
fn x86_64_tracking_ends_where_control_flow_joins() {
    // 0x00: jne 0x10      (other path arrives at 0x10 with %rax unknown)
    // 0x02: leaq g, %rax
    // 0x10: movl (%rax), %ecx   <- join
    let v = [ri(0x00, "jne", "0x10 <f+0x10>"), lea(0x02, "%rax"), ri(0x10, "movl", "(%rax), %ecx")];
    let r = acc("x86_64", &v);
    assert!(!r.iter().any(|x| x.0 == 0x10), "an access at a join is not attributed: {r:?}");
}

/// "A read-modify-write ... counts as both a read and a write" -- the bit
/// test-and-modify instructions (`bts`/`btr`/`btc`) on memory are one; clang
/// emits `lock btsl` for `atomic_fetch_or(&g, 1 << n) & (1 << n)`.
#[test]
fn x86_64_bit_test_and_modify_is_a_read_and_a_write() {
    for m in ["btsl", "btrl", "btcl"] {
        let v = [ri(0, "lock", ""), ri(1, m, "$0x3, 0x1234(%rip)  # 0x404040 <g>")];
        let r = acc("x86_64", &v);
        assert!(r.contains(&(1, G, Access::Read)) && r.contains(&(1, G, Access::Write)), "{m}: {r:?}");
    }
    // the plain bit test only reads
    assert_eq!(acc("x86_64", &[ri(0, "btl", "$0x3, 0x1234(%rip)  # 0x404040 <g>")]), vec![(0, G, Access::Read)]);
}

/// `lock` is printed as its own line (llvm-objdump: `lock` / `incl (%rip)`),
/// and the prefixed instruction is still a read-modify-write.
#[test]
fn x86_64_lock_prefixed_rmw_reads_and_writes() {
    let v = [ri(0, "lock", ""), ri(1, "incl", "0x1234(%rip)  # 0x404040 <g>"), ri(7, "lock", ""), ri(8, "xaddl", "%eax, 0x1234(%rip)  # 0x404080 <h>")];
    let r = acc("x86_64", &v);
    for want in [(1, G, Access::Read), (1, G, Access::Write), (8, H, Access::Read), (8, H, Access::Write)] {
        assert!(r.contains(&want), "{want:?}: {r:?}");
    }
}

/// A `rep movsb` copies into the global whose address is in %rdi: a write
/// "through that register". llvm-objdump prints the prefix as the mnemonic
/// and the string instruction in the operands:
/// `rep\t\tmovsb\t(%rsi), %es:(%rdi)` (verified with the LLVM 21 objdump).
#[test]
fn x86_64_rep_movs_as_objdump_prints_it_writes_the_destination() {
    let v = [lea(0, "%rdi"), ri(7, "rep", "movsb\t(%rsi), %es:(%rdi)")];
    let r = acc("x86_64", &v);
    assert!(r.contains(&(7, G, Access::Write)), "{r:?}");
}

/// `rep stosb` (memset into the global through %rdi) as objdump prints it.
#[test]
fn x86_64_rep_stos_as_objdump_prints_it_writes_the_destination() {
    let v = [lea(0, "%rdi"), ri(7, "rep", "stosb\t%al, %es:(%rdi)")];
    let r = acc("x86_64", &v);
    assert!(r.contains(&(7, G, Access::Write)), "{r:?}");
}

/// Without a prefix objdump prints the string instruction normally
/// (`movsb (%rsi), %es:(%rdi)`, `lodsb (%rsi), %al`): source read,
/// destination written.
#[test]
fn x86_64_unprefixed_string_instructions() {
    let v = [lea(0, "%rdi"), ri(7, "movsb", "(%rsi), %es:(%rdi)")];
    assert!(acc("x86_64", &v).contains(&(7, G, Access::Write)));
    let v = [lea(0, "%rsi"), ri(7, "lodsb", "(%rsi), %al")];
    assert!(acc("x86_64", &v).contains(&(7, G, Access::Read)));
}

/// An indexed access with scale > 1 into a global whose address is the base
/// is inside the global (`table[i]`).
#[test]
fn x86_64_indexed_access_through_the_base() {
    let v = [lea(0, "%rax"), ri(7, "movl", "(%rax,%rcx,4), %edx")];
    assert!(acc("x86_64", &v).contains(&(7, G, Access::Read)));
    // the global's address as the *scaled index* is not an access to it
    let v = [lea(0, "%rax"), ri(7, "movl", "(%rcx,%rax,4), %edx")];
    assert!(!acc("x86_64", &v).iter().any(|x| x.0 == 7));
}

/// RISC-V is not supported (README.md:253-256): nothing in it is a claim this
/// test can make, beyond not panicking on its shapes.
#[test]
fn riscv_is_scanned_without_panicking() {
    let _ = scan("riscv64", &[ri(0, "auipc", "a0, 0x404"), ri(4, "lw", "a1, 0x40(a0)"), ri(8, "addi", "a2, a0, 0x40")], &lookup);
    assert!(scan("mips", &[ri(0, "lw", "a0, 0(a1)")], &lookup).is_empty(), "no reader, no claim");
}

// ---------------------------------------------------------------- AArch64

/// "`adrp`+`add`/`ldr`/`str` on AArch64".
#[test]
fn aarch64_adrp_add_ldr_str() {
    let v = [
        ri(0, "adrp", "x8, 0x404000 <x>"),
        ri(4, "ldr", "w9, [x8, #0x40]"),
        ri(8, "str", "w9, [x8, #0x84]"),
        ri(12, "add", "x10, x8, #0x40"),
        ri(16, "ldrb", "w11, [x10, #0x3]"),
        ri(20, "stp", "x1, x2, [x10]"),
        ri(24, "adr", "x12, 0x404080 <h>"),
    ];
    assert_eq!(
        acc("aarch64", &v),
        vec![(4, G, Access::Read), (8, H, Access::Write), (12, G, Access::Address), (16, G, Access::Read), (20, G, Access::Write), (24, H, Access::Address)]
    );
}

/// An AArch64 register written through its 32-bit name is overwritten too:
/// `mov w8, #5` replaces x8, so the page it held is gone and a later
/// `[x8, #off]` is not the global.
#[test]
fn aarch64_writing_the_w_register_ends_tracking() {
    let v = [ri(0, "adrp", "x8, 0x404000 <x>"), ri(4, "mov", "w8, #0x5"), ri(8, "str", "wzr, [x8, #0x40]")];
    let r = acc("aarch64", &v);
    assert!(r.is_empty(), "{r:?}");
    // the same through a load into w8
    let v = [ri(0, "adrp", "x8, 0x404000 <x>"), ri(4, "ldr", "w8, [x8, #0x40]"), ri(8, "str", "wzr, [x8, #0x84]")];
    let r = acc("aarch64", &v);
    assert_eq!(r, vec![(4, G, Access::Read)], "the store is through the loaded value, not the page");
}

/// A branch ends AArch64 tracking too.
#[test]
fn aarch64_tracking_ends_at_a_branch() {
    let v = [ri(0, "adrp", "x8, 0x404000 <x>"), ri(4, "cbz", "w0, 0x20 <f+0x20>"), ri(8, "ldr", "w9, [x8, #0x40]")];
    assert!(acc("aarch64", &v).is_empty());
}

/// Fail closed at the edge of the address space: a page plus an offset that
/// does not fit is not a global, and never a panic.
#[test]
fn aarch64_address_arithmetic_at_the_edge_does_not_panic() {
    let v = [ri(0, "adrp", "x8, 0x7ffffffffffff000 <x>"), ri(4, "ldr", "x9, [x8, #0x1000]"), ri(8, "add", "x10, x8, #0x1000")];
    assert!(acc("aarch64", &v).is_empty());
}
