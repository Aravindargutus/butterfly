// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Data-coupling extraction (DO-178C 6.4.4.2.c, Annex A table A-7 objective
//! 8): which global data each function reads, writes or takes the address of,
//! derived from address-forming instruction sequences in the disassembly.

use crate::cfg;
use crate::objectcode::RawInstr;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Address,
}

#[derive(Debug, Clone)]
pub struct DataAccess {
    pub addr: u64,
    pub target: u64,
    pub access: Access,
}

/// Scan one function. `lookup(addr)` must say whether `addr` falls inside a
/// known data symbol (returns the symbol's start address).
pub fn scan(arch: &str, instrs: &[RawInstr], lookup: &dyn Fn(u64) -> Option<u64>) -> Vec<DataAccess> {
    let mut out = Vec::new();
    if arch.starts_with("aarch64") {
        scan_aarch64(instrs, lookup, &mut out);
    } else if arch.starts_with("x86") {
        scan_x86(instrs, lookup, &mut out);
    } else if arch.starts_with("riscv") {
        scan_riscv(instrs, lookup, &mut out);
    }
    out
}

fn is_store(m: &str) -> bool {
    m.starts_with("st") && !m.starts_with("stack")
}
fn is_load(m: &str) -> bool {
    m.starts_with("ld")
}

pub(crate) fn split_ops(ops: &str) -> Vec<String> {
    // "x8, [x9, #0x10]" -> ["x8", "[x9, #0x10]"]
    // "%eax, 0x10(%rbx,%rcx,4)" -> ["%eax", "0x10(%rbx,%rcx,4)"]: AT&T
    // groups with parentheses, and splitting inside one put the memory
    // operand somewhere other than last, so a store read as a load
    // (self-review of #14)
    let mut v = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for c in ops.chars() {
        match c {
            '[' | '{' | '(' => {
                depth += 1;
                cur.push(c);
            }
            ']' | '}' | ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                v.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        v.push(cur.trim().to_string());
    }
    v
}

fn imm(s: &str) -> Option<i64> {
    let s = s.trim().trim_start_matches('#');
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let v = if let Some(h) = s.strip_prefix("0x") { i64::from_str_radix(h, 16).ok()? } else { s.parse::<i64>().ok()? };
    if neg {
        v.checked_neg()
    } else {
        Some(v)
    }
}

/// The addresses some direct branch in the function targets: control-flow
/// joins. The other path into one may arrive with any value in a register,
/// so an address tracked by falling through is not known there (README
/// "tracked until ... control flow joins"). Losing a real access in a loop
/// costs a missed claim; keeping it would risk a wrong one.
fn joins(arch: &str, instrs: &[RawInstr]) -> HashSet<u64> {
    instrs.iter().filter(|i| matches!(cfg::classify(arch, &i.mnemonic, &i.operands), cfg::Ctl::Jump | cfg::Ctl::CondBranch)).filter_map(|i| cfg::target(&i.operands)).collect()
}

/// AArch64 `wN` is the low half of `xN`: writing it replaces the register,
/// so both names key one page.
fn a64reg(r: &str) -> String {
    match r.strip_prefix('w') {
        Some(n) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => format!("x{n}"),
        _ => r.to_string(),
    }
}

/// Does the AAPCS64 ABI guarantee this register survives a call?
/// `x19`..=`x28` are callee-saved on both Linux and Darwin; `x0`..=`x18` and
/// `x30` are caller-saved.
fn is_a64_callee_saved(reg: &str) -> bool {
    matches!(reg, "x19" | "x20" | "x21" | "x22" | "x23" | "x24" | "x25" | "x26" | "x27" | "x28")
}

/// Whether a tracked AArch64 register holds a 4 KiB page base from `adrp` or a
/// concrete address formed by `adrp`+`add` / `adr`. Register-indexed loads and
/// stores (`[x8, x9, lsl #3]`, e.g. `TASKS[i]`) are only valid accesses when
/// the base already holds a concrete global address (`Addr`), never when it
/// only holds a page base (`Page`).
#[derive(Clone, Copy)]
enum A64Val {
    Page(u64),
    Addr(u64),
}

impl A64Val {
    fn base(self) -> u64 {
        match self {
            A64Val::Page(a) | A64Val::Addr(a) => a,
        }
    }
}

fn scan_aarch64(instrs: &[RawInstr], lookup: &dyn Fn(u64) -> Option<u64>, out: &mut Vec<DataAccess>) {
    // register -> page base from `adrp`, or concrete address from `adrp+add`/`adr`
    let mut pages: HashMap<String, A64Val> = HashMap::new();
    let joins = joins("aarch64", instrs);
    for i in instrs {
        let m = i.mnemonic.to_ascii_lowercase();
        let ops: Vec<String> = split_ops(&i.operands).iter().map(|o| a64reg(o)).collect();
        let ctl = cfg::classify("aarch64", &i.mnemonic, &i.operands);
        if joins.contains(&i.addr) {
            pages.clear();
        }
        if ctl == cfg::Ctl::Call {
            pages.retain(|r, _| is_a64_callee_saved(r));
            continue;
        }
        if ctl != cfg::Ctl::None {
            pages.clear();
            continue;
        }
        match m.as_str() {
            "adrp" => {
                if let (Some(r), Some(t)) = (ops.first(), cfg::target(&i.operands)) {
                    pages.insert(r.clone(), A64Val::Page(t));
                }
                continue;
            }
            "adr" => {
                if let Some(t) = cfg::target(&i.operands) {
                    if let Some(g) = lookup(t) {
                        out.push(DataAccess { addr: i.addr, target: g, access: Access::Address });
                    }
                    if let Some(r) = ops.first() {
                        pages.insert(r.clone(), A64Val::Addr(t));
                    }
                } else if let Some(r) = ops.first() {
                    pages.remove(r);
                }
                continue;
            }
            "add" if ops.len() == 3 => {
                if let Some(val) = pages.get(&ops[1]).copied() {
                    // a sum past the address space is no global (fail
                    // closed): the result register is forgotten below
                    if let Some(t) = imm(&ops[2]).and_then(|off| val.base().checked_add_signed(off)) {
                        if let Some(g) = lookup(t) {
                            out.push(DataAccess { addr: i.addr, target: g, access: Access::Address });
                        }
                        // the result register now holds the concrete address
                        pages.insert(ops[0].clone(), A64Val::Addr(t));
                        continue;
                    }
                }
            }
            _ => {}
        }
        if is_load(&m) || is_store(&m) {
            // memory operand: [xN, #imm], [xN], or register-indexed [xN, xM(, lsl #s)]
            if let Some(mem_idx) = ops.iter().position(|o| o.starts_with('[')) {
                let mem = &ops[mem_idx];
                let pre = mem.ends_with("]!");
                let post = mem.ends_with(']') && ops.get(mem_idx + 1).is_some_and(|n| n.starts_with('#'));
                // Strip `!` before `]` so `[x8, #0x10]!` does not leave `]` on
                // the offset (#9.8).
                let inner = mem.trim_start_matches('[').trim_end_matches('!').trim_end_matches(']');
                let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
                if let Some(base_reg) = parts.first().map(|r| a64reg(r)) {
                    if let Some(val) = pages.get(&base_reg).copied() {
                        let resolved = match parts.as_slice() {
                            [_] => lookup(val.base()),
                            [_, o] => match imm(o) {
                                Some(off) => val.base().checked_add_signed(off).and_then(lookup),
                                // Register-indexed `[x8, x9]`: only valid when `x8` already
                                // holds a concrete global address (array indexing), never a
                                // bare `adrp` page base.
                                None => match val {
                                    A64Val::Addr(a) => lookup(a),
                                    A64Val::Page(_) => None,
                                },
                            },
                            // Register-indexed with shift/extend `[x8, x9, lsl #3]`
                            _ => match val {
                                A64Val::Addr(a) => lookup(a),
                                A64Val::Page(_) => None,
                            },
                        };
                        if let Some(g) = resolved {
                            out.push(DataAccess { addr: i.addr, target: g, access: if is_store(&m) { Access::Write } else { Access::Read } });
                        }
                    }
                    if pre || post {
                        pages.remove(&base_reg);
                    }
                }
            }
        }
        // destination register(s) overwritten -> forget their pages
        if !is_store(&m) && m != "cmp" && m != "cmn" && m != "tst" {
            let dests = if (m.starts_with("ld") && m.contains('p')) || m.starts_with("casp") { 2 } else { 1 };
            for d in ops.iter().take(dests) {
                if !d.starts_with('[') && m != "adrp" {
                    pages.remove(d);
                }
            }
            // Atomics like `ldadd x0, x1, [x2]` / `swp x0, x1, [x2]` write ops[1].
            if (m.starts_with("ldadd")
                || m.starts_with("ldclr")
                || m.starts_with("ldeor")
                || m.starts_with("ldset")
                || m.starts_with("ldsmax")
                || m.starts_with("ldsmin")
                || m.starts_with("ldumax")
                || m.starts_with("ldumin")
                || m.starts_with("swp"))
                && ops.len() >= 2
                && !ops[1].starts_with('[')
            {
                pages.remove(&ops[1]);
            }
        }
    }
}

/// x86-64 register names alias by width: `%rax`, `%eax`, `%ax`, `%al` and
/// `%ah` are one register, and writing any of them clobbers the pointer a
/// wider name was holding. Normalise to the 64-bit name.
fn reg64(name: &str) -> Option<String> {
    let r = name.trim().trim_start_matches('%').to_ascii_lowercase();
    if r.is_empty() {
        return None;
    }
    // r8..r15 with a width suffix
    if let Some(rest) = r.strip_prefix('r') {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            let tail = &rest[digits.len()..];
            if matches!(tail, "" | "d" | "w" | "b") {
                return Some(format!("r{digits}"));
            }
            return None;
        }
        // rax, rbx, rsp …
        if rest.len() == 2 {
            return Some(r);
        }
    }
    // eax/ax/al/ah -> rax, and the same shape for bx/cx/dx/si/di/bp/sp
    let core = match r.len() {
        // sil/dil/bpl/spl: the REX-only low byte of a register whose 16-bit
        // name has no x. Writing one clobbers the whole register just as
        // %al clobbers %rax, so it must normalise the same way.
        3 if matches!(r.as_str(), "sil" | "dil" | "bpl" | "spl") => r[..2].to_string(),
        3 if r.starts_with('e') => r[1..].to_string(),
        2 if r.ends_with('l') || r.ends_with('h') => {
            let c = &r[..1];
            if matches!(c, "a" | "b" | "c" | "d") {
                format!("{c}x")
            } else {
                r.clone()
            }
        }
        2 => r.clone(),
        _ => return None,
    };
    matches!(core.as_str(), "ax" | "bx" | "cx" | "dx" | "si" | "di" | "bp" | "sp").then(|| format!("r{core}"))
}

/// Does the ABI guarantee this register survives a call?
///
/// The System V AMD64 set (`rbx`, `rbp`, `r12`-`r15`). Windows x64 preserves
/// a superset of it, so keeping only these is correct under either: a
/// register that is really preserved but not listed costs a missed access,
/// never a wrong one.
fn is_callee_saved(reg: &str) -> bool {
    matches!(reg, "rbx" | "rbp" | "r12" | "r13" | "r14" | "r15")
}

/// The base register of an AT&T memory operand: `0x10(%rax)`, `(%rax)`,
/// `(%rax,%rcx,4)`. An operand with an index register is not tracked --
/// the effective address is not the global itself.
/// The registers an AT&T memory operand adds up: `disp(base,index,scale)`.
/// The index counts only at scale 1 -- `global + 4*i` is inside the global
/// when the global's address is the base, not when it is the index.
fn mem_regs(op: &str) -> Vec<String> {
    let Some(open) = op.find('(') else { return Vec::new() };
    let Some(close) = op[open..].find(')').map(|c| c + open) else { return Vec::new() };
    let mut parts = op[open + 1..close].split(',').map(str::trim);
    let base = parts.next().and_then(reg64);
    let index = parts.next().and_then(reg64);
    let scale = parts.next().map_or(1, |s| s.parse::<u32>().unwrap_or(0));
    base.into_iter().chain(index.filter(|_| scale == 1)).collect()
}

/// Does this mnemonic read its destination before writing it?
fn is_rmw(m: &str) -> bool {
    // bts/btr/btc: the bit test-and-modify forms read the word they set
    const RMW: [&str; 22] =
        ["add", "adc", "sub", "sbb", "and", "or", "xor", "inc", "dec", "neg", "not", "shl", "shr", "sar", "rol", "ror", "xadd", "cmpxchg", "xchg", "bts", "btr", "btc"];
    RMW.iter().any(|p| m.starts_with(p))
}

/// A single memory operand that is only written: `pop (%rax)`, `setne (%rax)`.
/// Everything else with one operand reads it (self-review of #14).
fn is_write_only(m: &str) -> bool {
    m.starts_with("pop") || m.starts_with("set")
}

/// The integer form of a mnemonic that also names an SSE instruction: `mul`,
/// `mulq` and `mulb` clobber `rdx:rax`; `mulsd` and `mulps` do not.
fn int_form(m: &str, base: &str) -> bool {
    m.strip_prefix(base).is_some_and(|suffix| suffix.is_empty() || (suffix.len() == 1 && "bwlq".contains(suffix)))
}

/// A string instruction, and what it does to each of its two memory
/// operands: the `%rsi` side is the source, the `%rdi` side the destination.
///
/// Both are memory, and the scan took only the first operand that looked
/// like memory. For `rep movsb %ds:(%rsi), %es:(%rdi)` that is the source,
/// so a global whose address was in `%rdi` was never recorded as written --
/// and `%rdi` was then invalidated, losing the write for good (review of
/// #14).
///
/// Recognised by the segment overrides objdump prints, not by the mnemonic:
/// `movs` also begins `movsbl` and the SSE `movsd`, which are not string
/// instructions and touch neither register.
fn string_memory(m: &str, ops: &str) -> Option<(Option<Access>, Option<Access>)> {
    // The segment override is the whole discrimination: `movsbl (%rsi), %eax`
    // is an ordinary load through a register that happens to be %rsi, and
    // `%ds:(%rsi)` is a string instruction's implicit source.
    if !ops.contains("%ds:(%rsi)") && !ops.contains("%es:(%rdi)") {
        return None;
    }
    match strip_rep(m) {
        f if f.starts_with("movs") => Some((Some(Access::Read), Some(Access::Write))),
        f if f.starts_with("stos") => Some((None, Some(Access::Write))),
        f if f.starts_with("lods") => Some((Some(Access::Read), None)),
        f if f.starts_with("scas") => Some((None, Some(Access::Read))),
        f if f.starts_with("cmps") => Some((Some(Access::Read), Some(Access::Read))),
        _ => None,
    }
}

/// The mnemonic without a `rep` family prefix, however objdump joined it on.
fn strip_rep(m: &str) -> &str {
    let t = m.trim();
    for p in ["repnz", "repne", "repz", "repe", "rep"] {
        if let Some(rest) = t.strip_prefix(p) {
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return rest.trim_start();
            }
        }
    }
    t
}

/// Registers an instruction writes without naming them in its operands. A
/// global's address left in one of these is gone afterwards, and a later
/// access through it belongs to something else (self-review of #14).
fn implicit_writes(m: &str, ops: &str, nparts: usize) -> &'static [&'static str] {
    if m.starts_with("cpuid") {
        &["rax", "rbx", "rcx", "rdx"]
    } else if m.starts_with("rdtscp") {
        &["rax", "rdx", "rcx"]
    } else if m.starts_with("rdtsc") || m.starts_with("rdpmc") {
        &["rax", "rdx"]
    } else if m.starts_with("syscall") {
        &["rax", "rcx", "r11"]
    } else if int_form(m, "mul") || int_form(m, "div") || int_form(m, "idiv") || (nparts == 1 && int_form(m, "imul")) {
        &["rax", "rdx"]
    } else if m.starts_with("cqt") || m.starts_with("clt") || m.starts_with("cwt") || m.starts_with("cbt") {
        // cqto/cltd/cwtd write rdx; cltq/cwtl/cbtw write rax
        &["rax", "rdx"]
    } else if m.starts_with("cmpxchg") {
        &["rax"]
    } else if string_memory(m, ops).is_some() {
        // the string instructions advance their pointers, and a `rep` form
        // counts down %rcx; `lods` and `scas` also write the accumulator
        match strip_rep(m) {
            f if f.starts_with("lods") => &["rax", "rsi", "rcx"],
            f if f.starts_with("scas") => &["rdi", "rcx"],
            f if f.starts_with("stos") => &["rdi", "rcx"],
            f if f.starts_with("cmps") => &["rsi", "rdi", "rcx"],
            _ => &["rsi", "rdi", "rcx"],
        }
    } else {
        &[]
    }
}

/// Does it only read its operands?
fn is_read_only(m: &str) -> bool {
    m.starts_with("cmp") && !m.starts_with("cmpxchg")
        || m.starts_with("test")
        || m.starts_with("push")
        || m.starts_with("bt") && !m.starts_with("btr") && !m.starts_with("bts") && !m.starts_with("btc")
}

/// x86-64, AT&T syntax (`mov src, dst`).
///
/// Two ways a global is touched. Either the instruction names it directly
/// with a rip-relative operand, which llvm-objdump annotates
/// (`movl 0x1234(%rip), %eax  # 0x404040 <g>`), or -- far more common in
/// clang output -- its address is materialised into a register with `lea`
/// and the access happens later through that register. Tracking only the
/// first reports an address-taken and misses every read and write (#14).
fn scan_x86(instrs: &[RawInstr], lookup: &dyn Fn(u64) -> Option<u64>, out: &mut Vec<DataAccess>) {
    // register -> the global whose address it currently holds
    let mut held: HashMap<String, u64> = HashMap::new();
    let joins = joins("x86_64", instrs);
    for i in instrs {
        if joins.contains(&i.addr) {
            held.clear();
        }
        let m = i.mnemonic.to_ascii_lowercase();
        let ctl = cfg::classify("x86_64", &i.mnemonic, &i.operands);
        // A call clobbers the caller-saved registers, and we cannot see the
        // callee. The callee-saved ones the ABI guarantees to preserve, so a
        // global address materialised into one is still there afterwards and
        // dropping it loses a real access (review of #14).
        if ctl == cfg::Ctl::Call {
            held.retain(|r, _| is_callee_saved(r));
            continue;
        }
        if ctl != cfg::Ctl::None {
            held.clear();
            continue;
        }
        // llvm-objdump prints a `rep` prefix as the mnemonic, with the string
        // instruction it prefixes at the head of the operands
        let (m, operands) = match i.operands.trim_start().split_once(char::is_whitespace) {
            Some((inner, rest)) if !m.is_empty() && strip_rep(&m).is_empty() => (format!("{m} {}", inner.to_ascii_lowercase()), rest),
            _ => (m, i.operands.as_str()),
        };
        // the annotation llvm-objdump appends for a rip-relative operand
        let annotated = operands.find('#').and_then(|h| cfg::target(&operands[h..])).and_then(&lookup);
        let ops = match operands.find('#') {
            Some(h) => &operands[..h],
            None => operands,
        };
        let parts = split_ops(ops);

        // `lea`: an address is formed, not a memory access
        if m.starts_with("lea") {
            let dst = parts.last().and_then(|d| reg64(d));
            match (annotated, dst) {
                (Some(g), Some(d)) => {
                    out.push(DataAccess { addr: i.addr, target: g, access: Access::Address });
                    held.insert(d, g);
                }
                (Some(g), None) => out.push(DataAccess { addr: i.addr, target: g, access: Access::Address }),
                (None, Some(d)) => {
                    held.remove(&d);
                }
                (None, None) => {}
            }
            continue;
        }

        // `nopw 0x0(%rax,%rax,1)`: alignment padding shaped like an access
        if m.starts_with("nop") {
            continue;
        }

        // A string instruction has two memory operands, and the general
        // path below takes only the first. Record both sides, then let the
        // implicit-write step invalidate the pointers.
        if let Some((from_rsi, to_rdi)) = string_memory(&m, ops) {
            for part in &parts {
                let side = if part.contains("(%rsi)") {
                    from_rsi
                } else if part.contains("(%rdi)") {
                    to_rdi
                } else {
                    None
                };
                if let (Some(access), Some(g)) = (side, mem_regs(part).iter().find_map(|r| held.get(r).copied())) {
                    out.push(DataAccess { addr: i.addr, target: g, access });
                }
            }
            for r in implicit_writes(&m, ops, parts.len()) {
                held.remove(*r);
            }
            continue;
        }

        // which operand is memory, and which global does it reach?
        let mem_idx = parts.iter().position(|o| o.contains('('));
        let target = match mem_idx {
            Some(k) => annotated.or_else(|| mem_regs(&parts[k]).iter().find_map(|r| held.get(r).copied())),
            None => None,
        };
        if let (Some(k), Some(g)) = (mem_idx, target) {
            let last = k + 1 == parts.len();
            // AT&T puts the destination last; a single operand is both,
            // unless the instruction only stores through it
            let writes = (last || parts.len() == 1) && !is_read_only(&m);
            let reads = !last || (parts.len() == 1 && !is_write_only(&m)) || is_rmw(&m) || is_read_only(&m);
            if reads {
                out.push(DataAccess { addr: i.addr, target: g, access: Access::Read });
            }
            if writes && (is_rmw(&m) || !reads || parts.len() > 1) {
                out.push(DataAccess { addr: i.addr, target: g, access: Access::Write });
            }
        }

        // a write to a register invalidates whatever address it held
        if let Some(d) = parts.last().filter(|d| !d.contains('(')).and_then(|d| reg64(d)) {
            if !is_read_only(&m) {
                held.remove(&d);
            }
        }
        // `xchg` and `xadd` write their first operand as well
        if m.starts_with("xchg") || m.starts_with("xadd") {
            if let Some(d) = parts.first().filter(|d| !d.contains('(')).and_then(|d| reg64(d)) {
                held.remove(&d);
            }
        }
        for r in implicit_writes(&m, ops, parts.len()) {
            held.remove(*r);
        }
    }
}

fn is_riscv_callee_saved(reg: &str) -> bool {
    matches!(
        reg,
        "fp" | "s0"
            | "s1"
            | "s2"
            | "s3"
            | "s4"
            | "s5"
            | "s6"
            | "s7"
            | "s8"
            | "s9"
            | "s10"
            | "s11"
            | "x8"
            | "x9"
            | "x18"
            | "x19"
            | "x20"
            | "x21"
            | "x22"
            | "x23"
            | "x24"
            | "x25"
            | "x26"
            | "x27"
    )
}

fn is_riscv_store(m: &str) -> bool {
    matches!(m, "sb" | "sh" | "sw" | "sd" | "c.sw" | "c.sd" | "c.swsp" | "c.sdsp" | "fsw" | "fsd" | "c.fsw" | "c.fsd") || m.starts_with("sc.")
}

fn scan_riscv(instrs: &[RawInstr], lookup: &dyn Fn(u64) -> Option<u64>, out: &mut Vec<DataAccess>) {
    let mut pages: HashMap<String, u64> = HashMap::new();
    let joins = joins("riscv64", instrs);
    for i in instrs {
        if joins.contains(&i.addr) {
            pages.clear();
        }
        let m = i.mnemonic.to_ascii_lowercase();
        let ctl = cfg::classify("riscv64", &i.mnemonic, &i.operands);
        if ctl == cfg::Ctl::Call {
            pages.retain(|r, _| is_riscv_callee_saved(r));
            continue;
        }
        if ctl != cfg::Ctl::None {
            pages.clear();
            continue;
        }
        let ops = split_ops(&i.operands);
        if m == "auipc" {
            if let (Some(r), Some(v)) = (ops.first(), ops.get(1).and_then(|s| imm(s))) {
                pages.insert(r.clone(), i.addr.wrapping_add((v << 12) as u64));
            }
            continue;
        }
        let mem = ops.iter().find(|o| o.contains('('));
        if let Some(mem) = mem {
            let (off, reg) = mem.split_once('(').unwrap_or(("0", mem));
            let reg = reg.trim_end_matches(')');
            if let Some(page) = pages.get(reg).copied() {
                let off_val = if off.trim().is_empty() { Some(0) } else { imm(off) };
                if let Some(g) = off_val.and_then(|o| page.checked_add_signed(o)).and_then(lookup) {
                    let access = if is_riscv_store(&m) { Access::Write } else { Access::Read };
                    out.push(DataAccess { addr: i.addr, target: g, access });
                }
            }
        } else if m == "addi" && ops.len() == 3 {
            if let Some(t) = pages.get(&ops[1]).and_then(|page| imm(&ops[2]).and_then(|o| page.checked_add_signed(o))) {
                if let Some(g) = lookup(t) {
                    out.push(DataAccess { addr: i.addr, target: g, access: Access::Address });
                }
                pages.insert(ops[0].clone(), t);
                continue;
            }
        }
        if let Some(d) = ops.first() {
            if !is_riscv_store(&m) {
                pages.remove(d);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ri(a: u64, m: &str, o: &str) -> RawInstr {
        RawInstr { addr: a, mnemonic: m.into(), operands: o.into() }
    }

    /// Clang materialises a global's address with `lea` and then works
    /// through the register: the reads and writes are on the *later*
    /// instructions, and a scanner that only looks at rip-relative operands
    /// sees an address-taken and nothing else (issue #14).
    #[test]
    fn x86_tracks_access_through_a_materialised_address() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        // void increment_counter(void) { global_counter++; }
        let rmw = vec![ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <global_counter>"), ri(7, "incl", "(%rax)")];
        let r = scan("x86_64", &rmw, &lookup);
        let kinds: Vec<Access> = r.iter().map(|x| x.access).collect();
        assert!(kinds.contains(&Access::Address), "the lea is still an address-taken: {r:?}");
        assert!(kinds.contains(&Access::Read) && kinds.contains(&Access::Write), "increment through the register is a read AND a write: {r:?}");
        assert!(r.iter().all(|x| x.target == 0x404040));

        // int read_counter(void) { return global_counter; }
        let read = vec![ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <global_counter>"), ri(7, "movl", "(%rax), %eax")];
        let r = scan("x86_64", &read, &lookup);
        assert!(r.iter().any(|x| x.access == Access::Read), "load through the register: {r:?}");
        assert!(!r.iter().any(|x| x.access == Access::Write), "a load must not be a write: {r:?}");

        // void set(int v) { global_counter = v; }
        let write = vec![ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <global_counter>"), ri(7, "movl", "%edi, (%rax)")];
        let r = scan("x86_64", &write, &lookup);
        assert!(r.iter().any(|x| x.access == Access::Write), "store through the register: {r:?}");
        assert!(!r.iter().any(|x| x.access == Access::Read), "a plain store is not a read: {r:?}");
    }

    /// A write to any architectural alias of a register clobbers the whole
    /// 64-bit register. `reg64` handled `%al`/`%ah` but not the REX-only
    /// low-byte names `%sil`, `%dil`, `%bpl`, `%spl`, so a write through one
    /// of those left the old association standing and the next dereference
    /// was attributed to a global it no longer points at -- a coupling claim
    /// that is not merely missing but wrong (review of #14).
    #[test]
    fn x86_low_byte_aliases_clobber_the_register_they_alias() {
        for (full, low) in [("%rsi", "%sil"), ("%rdi", "%dil"), ("%rbp", "%bpl"), ("%rsp", "%spl"), ("%rax", "%al"), ("%rbx", "%bh"), ("%r12", "%r12b")] {
            assert_eq!(reg64(low), reg64(full), "{low} is {full}");
        }
        // every width of every register normalises to one name
        for (name, want) in [
            ("%rsi", "rsi"),
            ("%esi", "rsi"),
            ("%si", "rsi"),
            ("%sil", "rsi"),
            ("%rdi", "rdi"),
            ("%edi", "rdi"),
            ("%di", "rdi"),
            ("%dil", "rdi"),
            ("%rbp", "rbp"),
            ("%ebp", "rbp"),
            ("%bp", "rbp"),
            ("%bpl", "rbp"),
            ("%rsp", "rsp"),
            ("%esp", "rsp"),
            ("%sp", "rsp"),
            ("%spl", "rsp"),
            ("%rax", "rax"),
            ("%eax", "rax"),
            ("%ax", "rax"),
            ("%al", "rax"),
            ("%ah", "rax"),
            ("%r12", "r12"),
            ("%r12d", "r12"),
            ("%r12w", "r12"),
            ("%r12b", "r12"),
        ] {
            assert_eq!(reg64(name).as_deref(), Some(want), "{name}");
        }

        // the reviewer's sequence: %sil is written, so %rsi no longer holds
        // the global and the load through it is not ours
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rsi  # 0x404040 <global_counter>"), ri(7, "movb", "$0, %sil"), ri(11, "movl", "(%rsi), %eax")], &lookup);
        assert!(r.iter().any(|x| x.access == Access::Address), "the lea is still an address-taken: {r:?}");
        assert!(!r.iter().any(|x| x.access == Access::Read), "the load is through a clobbered register and must not be reported: {r:?}");
    }

    /// A call clobbers the caller-saved registers only. Forgetting the
    /// callee-saved ones as well loses a pointer the ABI guarantees
    /// survives, and the read through it goes unreported (review of #14).
    #[test]
    fn x86_a_call_keeps_the_callee_saved_registers() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        // the reviewer's sequence, for each callee-saved register
        for reg in ["%rbx", "%rbp", "%r12", "%r13", "%r14", "%r15"] {
            let r = scan(
                "x86_64",
                &[ri(0, "leaq", &format!("0x1234(%rip), {reg}  # 0x404040 <global_counter>")), ri(7, "callq", "0x1000 <other>"), ri(12, "movl", &format!("({reg}), %eax"))],
                &lookup,
            );
            assert!(r.iter().any(|x| x.access == Access::Read && x.target == 0x404040), "{reg} is callee-saved: the read after the call must still be attributed: {r:?}");
        }
        // and the caller-saved ones are still forgotten
        for reg in ["%rax", "%rcx", "%rdx", "%rsi", "%rdi", "%r8", "%r9", "%r10", "%r11"] {
            let r = scan(
                "x86_64",
                &[ri(0, "leaq", &format!("0x1234(%rip), {reg}  # 0x404040 <global_counter>")), ri(7, "callq", "0x1000 <other>"), ri(12, "movl", &format!("({reg}), %eax"))],
                &lookup,
            );
            assert!(!r.iter().any(|x| x.access == Access::Read), "{reg} is caller-saved: what it holds after a call is unknown, so nothing may be attributed: {r:?}");
        }
    }

    #[test]
    fn x86_register_tracking_stops_when_the_register_is_clobbered() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        // the address is overwritten before the load: that load is not ours
        let clobbered = vec![ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <global_counter>"), ri(7, "movq", "%rdi, %rax"), ri(10, "movl", "(%rax), %eax")];
        let r = scan("x86_64", &clobbered, &lookup);
        assert_eq!(r.iter().filter(|x| x.access != Access::Address).count(), 0, "clobbered register must not attribute the load: {r:?}");
        // a call may clobber caller-saved registers
        let across_call = vec![ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <global_counter>"), ri(7, "callq", "0x1000 <other>"), ri(12, "movl", "(%rax), %eax")];
        let r = scan("x86_64", &across_call, &lookup);
        assert_eq!(r.iter().filter(|x| x.access != Access::Address).count(), 0, "a call clobbers %rax: {r:?}");
    }

    #[test]
    fn x86_sub_register_writes_clobber_the_whole_register() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        // %eax aliases %rax: writing the 32-bit name invalidates the pointer
        let v = vec![ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <global_counter>"), ri(7, "movl", "$0x0, %eax"), ri(12, "movl", "(%rax), %ecx")];
        let r = scan("x86_64", &v, &lookup);
        assert_eq!(r.iter().filter(|x| x.access != Access::Address).count(), 0, "%eax write must clobber %rax: {r:?}");
    }

    #[test]
    fn x86_direct_rip_relative_read_modify_write_is_both() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        // `incl sym(%rip)` reads and writes; it was reported write-only
        let v = vec![ri(0, "incl", "0x1234(%rip)  # 0x404040 <global_counter>")];
        let r = scan("x86_64", &v, &lookup);
        let kinds: Vec<Access> = r.iter().map(|x| x.access).collect();
        assert!(kinds.contains(&Access::Read) && kinds.contains(&Access::Write), "read-modify-write is both: {r:?}");
        // a plain store stays a single write
        let st = vec![ri(0, "movl", "%eax, 0x1234(%rip)  # 0x404040 <global_counter>")];
        let r = scan("x86_64", &st, &lookup);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].access, Access::Write);
        // and a plain load stays a single read
        let ld = vec![ri(0, "movl", "0x1234(%rip), %eax  # 0x404040 <global_counter>")];
        let r = scan("x86_64", &ld, &lookup);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].access, Access::Read);
    }

    #[test]
    fn x86_an_indexed_store_is_a_store() {
        // `split_ops` did not group parentheses, so the memory operand of an
        // indexed store was split in three and no longer last: a write to
        // the global read as a load (self-review of #14)
        assert_eq!(split_ops("%eax, 0x10(%rbx,%rcx,4)"), vec!["%eax", "0x10(%rbx,%rcx,4)"]);
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rbx  # 0x404040 <table>"), ri(7, "movl", "%eax, (%rbx,%rcx,4)")], &lookup);
        let kinds: Vec<Access> = r.iter().map(|x| x.access).collect();
        assert!(kinds.contains(&Access::Write) && !kinds.contains(&Access::Read), "table[i] = v is a store: {r:?}");
    }

    /// A string instruction has two memory operands. The scan took the first
    /// one that looked like memory -- the source -- so a global whose
    /// address was in %rdi was never recorded as written, and %rdi was then
    /// invalidated, losing the write for good (review of #14).
    #[test]
    fn x86_string_instructions_record_both_sides() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        let lea = |reg: &str| ri(0, "leaq", &format!("0x1234(%rip), {reg}  # 0x404040 <g>"));

        // the destination of a copy is written
        let r = scan("x86_64", &[lea("%rdi"), ri(7, "rep movsb", "%ds:(%rsi), %es:(%rdi)")], &lookup);
        assert!(r.iter().any(|x| x.access == Access::Write && x.target == 0x404040), "memcpy into the global is a write: {r:?}");
        assert!(!r.iter().any(|x| x.access == Access::Read), "nothing was read from it: {r:?}");

        // and the source of the same copy is read
        let r = scan("x86_64", &[lea("%rsi"), ri(7, "rep movsb", "%ds:(%rsi), %es:(%rdi)")], &lookup);
        assert!(r.iter().any(|x| x.access == Access::Read && x.target == 0x404040), "memcpy out of the global is a read: {r:?}");
        assert!(!r.iter().any(|x| x.access == Access::Write), "nothing was written to it: {r:?}");

        // memset writes; the accumulator side is not memory
        let r = scan("x86_64", &[lea("%rdi"), ri(7, "rep stosb", "%al, %es:(%rdi)")], &lookup);
        assert!(r.iter().any(|x| x.access == Access::Write), "memset into the global is a write: {r:?}");
        assert!(!r.iter().any(|x| x.access == Access::Read), "{r:?}");

        // lods reads its source; scas reads its destination
        let r = scan("x86_64", &[lea("%rsi"), ri(7, "lodsb", "%ds:(%rsi), %al")], &lookup);
        assert!(r.iter().any(|x| x.access == Access::Read) && !r.iter().any(|x| x.access == Access::Write), "{r:?}");
        let r = scan("x86_64", &[lea("%rdi"), ri(7, "repnz scasb", "%es:(%rdi), %al")], &lookup);
        assert!(r.iter().any(|x| x.access == Access::Read) && !r.iter().any(|x| x.access == Access::Write), "strlen reads: {r:?}");

        // memcmp reads both sides
        for reg in ["%rsi", "%rdi"] {
            let r = scan("x86_64", &[lea(reg), ri(7, "repz cmpsb", "%es:(%rdi), %ds:(%rsi)")], &lookup);
            assert!(r.iter().any(|x| x.access == Access::Read), "memcmp reads {reg}: {r:?}");
            assert!(!r.iter().any(|x| x.access == Access::Write), "and writes neither: {r:?}");
        }

        // both sides at once: one global copied onto itself is read and written
        let r = scan("x86_64", &[lea("%rsi"), ri(7, "leaq", "0x1234(%rip), %rdi  # 0x404040 <g>"), ri(14, "rep movsb", "%ds:(%rsi), %es:(%rdi)")], &lookup);
        let kinds: Vec<Access> = r.iter().map(|x| x.access).collect();
        assert!(kinds.contains(&Access::Read) && kinds.contains(&Access::Write), "{r:?}");
    }

    /// The mnemonics the string instructions share a prefix with must not be
    /// mistaken for them: they touch neither %rsi nor %rdi.
    #[test]
    fn x86_lookalikes_are_not_string_instructions() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        // sign-extending load: a plain read through the register
        let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rsi  # 0x404040 <g>"), ri(7, "movsbl", "(%rsi), %eax")], &lookup);
        assert!(r.iter().any(|x| x.access == Access::Read), "movsbl is an ordinary load: {r:?}");
        // SSE: not memory through either pointer, and %rsi survives it
        let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rsi  # 0x404040 <g>"), ri(7, "movsd", "%xmm1, %xmm0"), ri(11, "movl", "(%rsi), %eax")], &lookup);
        assert!(r.iter().any(|x| x.access == Access::Read), "movsd between registers leaves %rsi alone: {r:?}");
    }

    #[test]
    fn x86_pop_and_setcc_to_memory_are_writes_not_reads() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        for (m, ops) in [("popq", "(%rax)"), ("setne", "(%rax)"), ("sete", "(%rax)")] {
            let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <g>"), ri(7, m, ops)], &lookup);
            let kinds: Vec<Access> = r.iter().map(|x| x.access).collect();
            assert!(kinds.contains(&Access::Write), "{m} {ops} stores: {r:?}");
            assert!(!kinds.contains(&Access::Read), "{m} {ops} does not load: {r:?}");
        }
        // and the one-operand instructions that do read stay reads
        for (m, ops) in [("mull", "(%rax)"), ("pushq", "(%rax)"), ("idivl", "(%rax)")] {
            let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <g>"), ri(7, m, ops)], &lookup);
            let kinds: Vec<Access> = r.iter().map(|x| x.access).collect();
            assert!(kinds.contains(&Access::Read) && !kinds.contains(&Access::Write), "{m} {ops}: {r:?}");
        }
    }

    #[test]
    fn x86_an_indexed_access_through_a_held_base_is_attributed() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        // int get(int i) { return table[i]; }
        let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <table>"), ri(7, "movslq", "%edi, %rcx"), ri(10, "movl", "(%rax,%rcx,4), %eax")], &lookup);
        assert!(r.iter().any(|x| x.access == Access::Read && x.target == 0x404040), "an array element is an access to the array: {r:?}");
        // the global's address as the index, at scale 1, is the same sum
        let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <table>"), ri(7, "movb", "(%rcx,%rax,1), %dl")], &lookup);
        assert!(r.iter().any(|x| x.access == Access::Read), "{r:?}");
        // but scaled, it is not inside the global
        let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <table>"), ri(7, "movl", "(%rcx,%rax,4), %edx")], &lookup);
        assert!(!r.iter().any(|x| x.access == Access::Read), "{r:?}");
    }

    #[test]
    fn x86_alignment_padding_shaped_like_an_access_is_ignored() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        let r = scan(
            "x86_64",
            &[ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <g>"), ri(7, "nopw", "0x0(%rax,%rax,1)"), ri(13, "nopl", "(%rax)"), ri(15, "movl", "(%rax), %ecx")],
            &lookup,
        );
        assert_eq!(r.iter().filter(|x| x.access == Access::Read).count(), 1, "the nops are padding, the mov is the access: {r:?}");
    }

    #[test]
    fn x86_implicit_register_writes_end_tracking() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        // each of these writes %rax without naming it
        for (m, ops) in [
            ("mulq", "%rcx"),
            ("divl", "%ecx"),
            ("imull", "%ecx"),
            ("cpuid", ""),
            ("rdtsc", ""),
            ("syscall", ""),
            ("cltq", ""),
            ("cmpxchg", "%rcx, (%rdx)"),
            ("lods", "%ds:(%rsi), %al"),
        ] {
            let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <g>"), ri(7, m, ops), ri(12, "movl", "(%rax), %ecx")], &lookup);
            assert!(!r.iter().any(|x| x.access == Access::Read), "{m} {ops} clobbers %rax, so the load is not through the global: {r:?}");
        }
        // the SSE spellings do not touch %rax
        for (m, ops) in [("mulsd", "%xmm1, %xmm0"), ("divss", "%xmm1, %xmm0")] {
            let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <g>"), ri(7, m, ops), ri(12, "movl", "(%rax), %ecx")], &lookup);
            assert!(r.iter().any(|x| x.access == Access::Read), "{m} is a float op and leaves %rax alone: {r:?}");
        }
        // a string move clobbers rsi, rdi and rcx; a sign-extending load does not
        let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rdi  # 0x404040 <g>"), ri(7, "rep movsb", "%ds:(%rsi), %es:(%rdi)"), ri(9, "movl", "(%rdi), %ecx")], &lookup);
        assert!(!r.iter().any(|x| x.access == Access::Read), "{r:?}");
        let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rdi  # 0x404040 <g>"), ri(7, "movsbl", "(%rsi), %eax"), ri(10, "movl", "(%rdi), %ecx")], &lookup);
        assert!(r.iter().any(|x| x.access == Access::Read), "{r:?}");
        // xchg writes both operands
        let r = scan("x86_64", &[ri(0, "leaq", "0x1234(%rip), %rax  # 0x404040 <g>"), ri(7, "xchgq", "%rax, %rcx"), ri(10, "movl", "(%rax), %edx")], &lookup);
        assert!(!r.iter().any(|x| x.access == Access::Read), "{r:?}");
    }

    #[test]
    fn aarch64_adrp_ldr_str() {
        let instrs = vec![ri(0, "adrp", "x8, 0x40090000 <STATE>"), ri(4, "ldr", "w9, [x8, #0x10]"), ri(8, "str", "w9, [x8, #0x14]"), ri(12, "add", "x0, x8, #0x20")];
        let lookup = |a: u64| if (0x40090000..0x40090100).contains(&a) { Some(0x40090000) } else { None };
        let r = scan("aarch64", &instrs, &lookup);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].access, Access::Read);
        assert_eq!(r[1].access, Access::Write);
        assert_eq!(r[2].access, Access::Address);
    }

    #[test]
    fn aarch64_call_clobbers_caller_saved_but_keeps_callee_saved() {
        let lookup = |a: u64| (a == 0x40090040).then_some(0x40090040);
        let instrs = vec![
            ri(0, "adrp", "x0, 0x40090000 <g>"),
            ri(4, "adrp", "x19, 0x40090000 <g>"),
            ri(8, "bl", "0x1000 <get_ptr>"),
            ri(12, "ldr", "w1, [x0, #0x40]"),
            ri(16, "ldr", "w2, [x19, #0x40]"),
        ];
        let r = scan("aarch64", &instrs, &lookup);
        assert_eq!(r.len(), 1, "x0 was clobbered by bl, only x19 survives: {r:?}");
        assert_eq!(r[0].addr, 16);
    }

    #[test]
    fn aarch64_pre_indexed_register_indexed_and_ldp_are_handled_soundly() {
        // Two globals in the same page: PAGE_ZERO at +0, TARGET at +0x40
        let lookup = |a: u64| {
            if a == 0x40090000 {
                Some(0x40090000)
            } else if a == 0x40090040 {
                Some(0x40090040)
            } else {
                None
            }
        };
        // Pre-indexed [x8, #0x40]! accesses +0x40 (not +0) and invalidates x8 afterwards
        let pre = vec![ri(0, "adrp", "x8, 0x40090000"), ri(4, "ldr", "w1, [x8, #0x40]!"), ri(8, "ldr", "w2, [x8, #0x0]")];
        let r = scan("aarch64", &pre, &lookup);
        assert_eq!(r.len(), 1, "writeback invalidates x8 for addr 8: {r:?}");
        assert_eq!((r[0].addr, r[0].target), (4, 0x40090040), "offset #0x40 must not become 0");

        // Register-indexed [x8, x9] from a bare adrp page base must NOT fall back to offset 0 (PAGE_ZERO),
        // whereas register-indexed access through a concrete global address (adrp + add) IS an array read.
        let reg_idx = vec![ri(0, "adrp", "x8, 0x40090000"), ri(4, "ldr", "w1, [x8, x9]"), ri(8, "ldr", "w2, [x8, x9, lsl #2]")];
        assert!(scan("aarch64", &reg_idx, &lookup).is_empty(), "bare adrp page base + register index must not attribute to offset 0");
        let arr_idx = vec![ri(0, "adrp", "x8, 0x40090000"), ri(4, "add", "x8, x8, #0x40"), ri(8, "ldr", "x0, [x8, x9, lsl #3]")];
        let r = scan("aarch64", &arr_idx, &lookup);
        assert!(r.iter().any(|x| x.addr == 8 && x.target == 0x40090040 && x.access == Access::Read), "concrete global + register index is an array read: {r:?}");

        // ldp writes BOTH ops[0] and ops[1], so x8 is overwritten
        let pair = vec![ri(0, "adrp", "x8, 0x40090000"), ri(4, "ldp", "x0, x8, [sp]"), ri(8, "ldr", "w1, [x8, #0x40]")];
        assert!(scan("aarch64", &pair, &lookup).is_empty(), "ldp second destination x8 must invalidate page");
    }

    #[test]
    fn riscv_call_and_shift_invalidate_tracked_registers() {
        let lookup = |a: u64| (a == 0x404040).then_some(0x404040);
        // `slli a0, a0, 2` starts with 's' but is NOT a store; it overwrites a0
        let shifted = vec![ri(0, "auipc", "a0, 0x404"), ri(4, "slli", "a0, a0, 2"), ri(8, "lw", "a1, 0x40(a0)")];
        assert!(scan("riscv64", &shifted, &lookup).is_empty(), "slli overwrites a0");
        // `call` clobbers caller-saved a0
        let called = vec![ri(0, "auipc", "a0, 0x404"), ri(4, "call", "0x1000 <foo>"), ri(8, "lw", "a1, 0x40(a0)")];
        assert!(scan("riscv64", &called, &lookup).is_empty(), "call clobbers a0");
    }
}
