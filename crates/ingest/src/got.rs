// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Calls made through a global offset table slot (#271).
//!
//! A compiler that calls an imported function without a PLT or stub loads
//! the callee's address from its GOT slot and branches to the register:
//! `adrp x16, GOT; ldr x16, [x16, #off]; blr x16` on AArch64 (Darwin's
//! `___chkstk_darwin` stack probe is always called this way), or
//! `call *foo@GOTPCREL(%rip)` on x86-64 with `-fno-plt`. The instruction is a
//! register- or memory-indirect branch, but the callee is not a function
//! pointer: the slot is bound by the dynamic linker to one named symbol, and
//! the image says which -- the Mach-O indirect symbol table, or an ELF
//! `R_*_GLOB_DAT` dynamic relocation. Reporting it as an indirect call asked
//! a reviewer to establish a callee set that is a single, named routine.
//!
//! [`bindings`] reads which symbol each GOT slot is bound to; [`annotate`]
//! finds the branches whose target register (or memory operand) is that slot
//! and marks their operands `<symbol@GOT>`. [`crate::cfg::classify`] reads a
//! marked indirect call as a call and a marked indirect branch as a jump, so
//! every consumer of a direct call -- call graph, helper-call gaps, the stack
//! analysis -- sees one, as it already does for `<symbol@plt>`.
//!
//! A position-independent ELF executable also has GOT slots for functions
//! *inside* the image, filled by an `R_*_RELATIVE` relocation: the load base
//! plus a link-time constant. rustc calls every function of another crate
//! this way on x86-64 Linux (`-Zplt=no`, an `R_X86_64_GOTPCREL` the linker
//! does not relax), so the panics behind a division or a bounds check are
//! `call *slot(%rip)`. A `.got` slot whose addend is the start of a function
//! symbol is bound to that function exactly as a `GLOB_DAT` slot is to its
//! symbol, and its mark also carries the address (#325).
//!
//! What is not recognised stays indirect (fail closed): a slot neither table
//! names (a Mach-O local symbol's slot, whose value is a rebase; an ELF
//! relative slot outside `.got`, or not at a function's start), a register
//! loaded from anywhere but a GOT slot -- a function pointer read *through*
//! the GOT, `ldr x8, [x8, #got]; ldr x1, [x8]; br x1`, loads twice and is not
//! matched -- a sequence that crosses a branch target, and every other
//! architecture.

use crate::objectcode::RawInstr;
use std::collections::{BTreeSet, HashMap};

/// The marker [`annotate`] appends, and that `cfg` reads back.
pub const MARK: &str = "@GOT>";

/// What a GOT slot is bound to: a symbol, and its address when it is a
/// function of the image itself (an ELF relative slot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub symbol: String,
    pub addr: Option<u64>,
}

impl Bound {
    fn import(symbol: impl Into<String>) -> Self {
        Bound { symbol: symbol.into(), addr: None }
    }
}

/// GOT slot address -> the symbol the dynamic linker binds it to.
pub fn bindings(file: &object::File) -> HashMap<u64, Bound> {
    match file {
        object::File::MachO64(f) => macho_bindings(f),
        object::File::Elf64(_) | object::File::Elf32(_) => elf_bindings(file),
        _ => HashMap::new(),
    }
}

/// Mach-O: a non-lazy (`__got`, `__auth_got`) or lazy (`__la_symbol_ptr`)
/// pointer section's `reserved1` is its first entry in the indirect symbol
/// table, one entry per pointer. This table is written for chained fixups
/// and for the older bind opcodes alike, so it needs neither decoded. An
/// entry marked LOCAL or ABS names no import: that slot holds a rebased
/// address, and is left unresolved.
fn macho_bindings(f: &object::read::macho::MachOFile64<object::Endianness>) -> HashMap<u64, Bound> {
    use object::macho::{INDIRECT_SYMBOL_ABS, INDIRECT_SYMBOL_LOCAL, SECTION_TYPE, S_LAZY_SYMBOL_POINTERS, S_NON_LAZY_SYMBOL_POINTERS};
    use object::read::macho::{Nlist as _, Section as _};
    use object::{Object, ReadRef};
    let mut out = HashMap::new();
    let e = f.endian();
    let data = f.data();
    let Ok(mut cmds) = f.macho_load_commands() else { return out };
    let mut dysymtab = None;
    while let Ok(Some(c)) = cmds.next() {
        if let Ok(Some(d)) = c.dysymtab() {
            dysymtab = Some(d);
        }
    }
    let Some(d) = dysymtab else { return out };
    let Ok(indirect) = data.read_slice_at::<object::U32<object::Endianness>>(d.indirectsymoff.get(e).into(), d.nindirectsyms.get(e) as usize) else { return out };
    let symtab = f.macho_symbol_table();
    for sec in f.sections() {
        let s = sec.macho_section();
        let ty = s.flags(e) & SECTION_TYPE;
        if ty != S_NON_LAZY_SYMBOL_POINTERS && ty != S_LAZY_SYMBOL_POINTERS {
            continue;
        }
        let first = s.reserved1.get(e) as usize;
        let max_k = usize::try_from(s.size(e) / 8).unwrap_or(usize::MAX).min(indirect.len().saturating_sub(first));
        for k in 0..max_k {
            let Some(idx) = first.checked_add(k).and_then(|pos| indirect.get(pos)).map(|v| v.get(e)) else { break };
            if idx & (INDIRECT_SYMBOL_LOCAL | INDIRECT_SYMBOL_ABS) != 0 {
                continue;
            }
            let Ok(sym) = symtab.symbol(object::SymbolIndex(idx as usize)) else { continue };
            let Ok(name) = sym.name(e, symtab.strings()) else { continue };
            if let (Ok(name), Some(slot_addr)) = (std::str::from_utf8(name), (k as u64).checked_mul(8).and_then(|off| s.addr(e).checked_add(off))) {
                out.insert(slot_addr, Bound::import(name));
            }
        }
    }
    out
}

/// ELF: the dynamic relocations that bind a GOT slot to a symbol.
fn elf_bindings(file: &object::File) -> HashMap<u64, Bound> {
    use object::{Object, ObjectSection, ObjectSymbol, ObjectSymbolTable, SymbolKind};
    let machine = file.architecture();
    let mut relocs = Vec::new();
    let Some(it) = file.dynamic_relocations() else { return HashMap::new() };
    let dynsyms = file.dynamic_symbol_table();
    for (offset, r) in it {
        let object::RelocationFlags::Elf { r_type } = r.flags() else { continue };
        let name = match r.target() {
            object::RelocationTarget::Symbol(i) => {
                let Some(name) = dynsyms.as_ref().and_then(|t| t.symbol_by_index(i).ok()).and_then(|s| s.name().ok().map(str::to_string)) else { continue };
                name
            }
            _ => String::new(),
        };
        relocs.push(Reloc { offset, r_type, name, addend: r.addend() });
    }
    let got: Vec<(u64, u64)> = file.sections().filter(|s| s.name() == Ok(".got")).map(|s| (s.address(), s.address().saturating_add(s.size()))).collect();
    // function start -> its name; of several aliases, a global one, then the
    // first by name, so the choice does not depend on symbol-table order
    let mut funcs: HashMap<u64, (bool, String)> = HashMap::new();
    for s in file.symbols() {
        let Ok(name) = s.name() else { continue };
        if s.kind() != SymbolKind::Text || s.is_undefined() || name.is_empty() {
            continue;
        }
        let key = (!s.is_global(), name.to_string());
        let e = funcs.entry(s.address()).or_insert_with(|| key.clone());
        if key < *e {
            *e = key;
        }
    }
    got_slots(machine, relocs, |a| got.iter().any(|&(lo, hi)| (lo..hi).contains(&a)), |a| funcs.get(&a).map(|(_, n)| n.clone()))
}

/// One ELF dynamic relocation: where, which type, the symbol it names
/// (empty for none) and its addend.
struct Reloc {
    offset: u64,
    r_type: u32,
    name: String,
    addend: i64,
}

/// The relocations among `relocs` that bind a GOT slot. `R_*_GLOB_DAT`
/// binds one to the named symbol. `R_*_RELATIVE` does when its slot is in
/// `.got` (`in_got`) and its addend is a function's start (`func_at`): the
/// slot holds that function's address once the image is loaded, whatever
/// the load base. `R_*_JUMP_SLOT` belongs to a PLT, whose calls the
/// disassembler already names `<sym@plt>`; a relative slot elsewhere (a
/// table of pointers) or into a function's middle binds nothing.
fn got_slots(machine: object::Architecture, relocs: impl IntoIterator<Item = Reloc>, in_got: impl Fn(u64) -> bool, func_at: impl Fn(u64) -> Option<String>) -> HashMap<u64, Bound> {
    use object::elf::*;
    let (glob_dat, relative) = match machine {
        object::Architecture::X86_64 => (R_X86_64_GLOB_DAT, R_X86_64_RELATIVE),
        object::Architecture::Aarch64 => (R_AARCH64_GLOB_DAT, R_AARCH64_RELATIVE),
        object::Architecture::I386 => (R_386_GLOB_DAT, R_386_RELATIVE),
        object::Architecture::Arm => (R_ARM_GLOB_DAT, R_ARM_RELATIVE),
        _ => return HashMap::new(),
    };
    let mut out = HashMap::new();
    let mut disputed = BTreeSet::new();
    for r in relocs {
        let bound = if r.r_type == glob_dat && !r.name.is_empty() {
            Some(Bound::import(r.name))
        } else if r.r_type == relative && r.name.is_empty() && in_got(r.offset) {
            u64::try_from(r.addend).ok().and_then(|a| func_at(a).map(|symbol| Bound { symbol, addr: Some(a) }))
        } else {
            None
        };
        if let Some(b) = bound {
            match out.insert(r.offset, b.clone()) {
                Some(old) if old != b => {
                    disputed.insert(r.offset);
                }
                _ => {}
            }
        }
    }
    for d in disputed {
        out.remove(&d);
    }
    out
}

/// Mark each call or branch through a bound GOT slot with `<symbol@GOT>`.
///
/// `instrs` is the whole image, sorted by address. A sequence is matched
/// only when nothing can enter it part-way: no direct branch or call in the
/// image targets an instruction after its first. (A jump table could, in
/// principle; AArch64 compilers load the probe into x16, the intra-procedure
/// scratch register a linker veneer may clobber, which no compiler keeps live
/// into a block.)
pub fn annotate(arch: &str, instrs: &mut [RawInstr], got: &HashMap<u64, Bound>) {
    if got.is_empty() {
        return;
    }
    let targets: BTreeSet<u64> = instrs
        .iter()
        .filter(|i| matches!(crate::cfg::classify(arch, &i.mnemonic, &i.operands), crate::cfg::Ctl::Jump | crate::cfg::Ctl::CondBranch | crate::cfg::Ctl::Call))
        .filter_map(|i| crate::cfg::target(&i.operands))
        .collect();
    // `from == to` for a one-instruction sequence (x86-64): nothing lies
    // after its first, and `range(to + 1..=to)` would panic
    let entered = |from: u64, to: u64| from < to && targets.range(from + 1..=to).next().is_some();
    for i in 0..instrs.len() {
        let slot = if arch.starts_with("aarch64") {
            aarch64_slot(instrs, i)
        } else if arch.starts_with("x86_64") {
            x86_64_slot(&instrs[i])
        } else {
            None
        };
        let Some((start, slot)) = slot else { continue };
        if entered(start, instrs[i].addr) {
            continue;
        }
        let Some(b) = got.get(&slot) else { continue };
        let ins = &mut instrs[i];
        let mut ops = ins.operands.clone();
        // x86: drop objdump's `# 0x2430 <nearest+0x..>` comment -- the slot
        // address and a guessed symbol -- for the bound name
        if let Some(c) = ops.find(['#', '<']) {
            ops.truncate(c);
        }
        // a function of the image is named with its address, as a direct
        // call is (`0x50c10 <sym>`), so it resolves to that function
        let at = b.addr.map(|a| format!(" {a:#x}")).unwrap_or_default();
        ins.operands = format!("{}{at} <{}{MARK}", ops.trim(), b.symbol);
    }
}

/// `blr xN` / `br xN` (and the pointer-authenticating forms) whose `xN` was
/// loaded from `page + off` by `ldr xN, [xB, #off]`, `xB` set by
/// `adrp xB, page` -- or `ldr xN, [xB]` after `add xB, xB, #off`. Any other
/// write to either register in between, or any control transfer, and it is
/// not a GOT call. Returns (address of the `adrp`, slot address).
fn aarch64_slot(instrs: &[RawInstr], i: usize) -> Option<(u64, u64)> {
    let m = instrs[i].mnemonic.to_ascii_lowercase();
    if !matches!(m.as_str(), "blr" | "br" | "blraa" | "blrab" | "blraaz" | "blrabz" | "braa" | "brab" | "braaz" | "brabz") {
        return None;
    }
    let ops = |k: usize| -> Vec<String> { instrs[k].operands.to_ascii_lowercase().split(',').map(|s| s.trim().to_string()).collect() };
    let reg = ops(i).first()?.clone();
    // the load of `reg`, and what it was loaded from
    let (ld, base, mut off) = {
        let mut k = i;
        loop {
            k = k.checked_sub(1)?;
            if i - k > 8 || crate::cfg::classify("aarch64", &instrs[k].mnemonic, &instrs[k].operands) != crate::cfg::Ctl::None {
                return None;
            }
            let o = ops(k);
            if !writes(&instrs[k].mnemonic, &o, &reg) {
                continue;
            }
            if !instrs[k].mnemonic.eq_ignore_ascii_case("ldr") || o[0] != reg {
                return None;
            }
            // `[xB, #off]` or `[xB]`, never pre/post-indexed or register-offset
            let mem = o.get(1..)?.join(",");
            let inner = mem.strip_prefix('[')?.strip_suffix(']')?;
            let mut parts = inner.split(',');
            let base = parts.next()?.trim().to_string();
            let off = match parts.next() {
                None => 0,
                Some(imm) => parse_imm(imm.trim())?,
            };
            if parts.next().is_some() {
                return None;
            }
            break (k, base, off);
        }
    };
    // where `base` was set: `adrp base, page`, perhaps through one
    // `add base, base, #lo12`
    let mut k = ld;
    loop {
        k = k.checked_sub(1)?;
        if ld - k > 8 || crate::cfg::classify("aarch64", &instrs[k].mnemonic, &instrs[k].operands) != crate::cfg::Ctl::None {
            return None;
        }
        let o = ops(k);
        if !writes(&instrs[k].mnemonic, &o, &base) {
            continue;
        }
        match instrs[k].mnemonic.to_ascii_lowercase().as_str() {
            "adrp" if o[0] == base => {
                let page = crate::cfg::target(&instrs[k].operands)?;
                return Some((instrs[k].addr, page.checked_add(off)?));
            }
            "add" if o.len() == 3 && o[0] == base && o[1] == base && off == 0 => {
                off = parse_imm(&o[2])?;
            }
            _ => return None,
        }
    }
}

/// Whether an AArch64 instruction may write `reg`, erring towards yes: a
/// yes only ever leaves a call indirect. The destination is the first
/// operand (so a store of `reg` counts, harmlessly), or either of a pair
/// load's two; and any writeback addressing mode that names `reg` at all.
fn writes(mnemonic: &str, ops: &[String], reg: &str) -> bool {
    let w = reg.strip_prefix('x').map(|n| format!("w{n}"));
    let is = |d: &String| d == reg || w.as_deref() == Some(d.as_str());
    let m = mnemonic.to_ascii_lowercase();
    let dests = if (m.starts_with("ld") && m.contains('p')) || m.starts_with("casp") { 2 } else { 1 };
    let text = ops.join(",");
    ops.iter().take(dests).any(is) || ((text.contains('!') || text.contains("],")) && text.contains(reg))
}

fn parse_imm(s: &str) -> Option<u64> {
    let s = s.trim().trim_start_matches('#');
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// `call *disp(%rip)` / `jmp *disp(%rip)`: objdump prints the slot address
/// it computed as a `# 0x…` (ELF) or `## 0x…` (Mach-O) comment. Without that
/// comment, no match.
fn x86_64_slot(ins: &RawInstr) -> Option<(u64, u64)> {
    let m_owned = ins.mnemonic.to_ascii_lowercase();
    let (m, ops) = crate::cfg::unwrap_x86_prefix(&m_owned, &ins.operands);
    let m = m.to_ascii_lowercase();
    if !matches!(m.as_str(), "call" | "callq" | "jmp" | "jmpq") {
        return None;
    }
    let ops = ops.trim();
    let (operand, comment) = ops.split_once('#')?;
    let operand = operand.trim();
    if !(operand.starts_with('*') && operand.ends_with("(%rip)")) {
        return None;
    }
    let slot = comment.trim_start_matches('#').split_whitespace().next()?.strip_prefix("0x")?;
    Some((ins.addr, u64::from_str_radix(slot, 16).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ri(addr: u64, m: &str, o: &str) -> RawInstr {
        RawInstr { addr, mnemonic: m.into(), operands: o.into() }
    }

    fn got() -> HashMap<u64, Bound> {
        HashMap::from([(0x100004008, Bound::import("___chkstk_darwin")), (0x20420, Bound::import("ext")), (0x2430, Bound::import("ext"))])
    }

    #[test]
    fn the_darwin_stack_probe_call_is_named() {
        // examples/c-constructs vla_sum, Apple clang 17
        let mut v = vec![
            ri(0x1000005c8, "stur", "x9, [x29, #-0x40]"),
            ri(0x1000005cc, "adrp", "x16, 0x100004000 <_printf+0x100004000>"),
            ri(0x1000005d0, "ldr", "x16, [x16, #0x8]"),
            ri(0x1000005d4, "blr", "x16"),
            ri(0x1000005d8, "ldur", "x9, [x29, #-0x40]"),
        ];
        annotate("aarch64", &mut v, &got());
        assert_eq!(v[3].operands, "x16 <___chkstk_darwin@GOT>");
        assert_eq!(v[3].mnemonic, "blr", "the instruction is shown as written");
        assert_eq!(crate::cfg::classify("aarch64", &v[3].mnemonic, &v[3].operands), crate::cfg::Ctl::Call);
        assert_eq!(crate::cfg::target_symbol(&v[3].operands).as_deref(), Some("___chkstk_darwin"));
        assert_eq!(crate::cfg::target(&v[3].operands), None, "no address: the callee is outside the image");
    }

    #[test]
    fn an_elf_tail_call_through_the_got_with_a_scheduled_move_is_named() {
        // lld-linked aarch64-linux-gnu -fno-plt: `ext(1)` as a tail call
        let mut v = vec![ri(0x10370, "adrp", "x1, 0x20000 <_start+0xfc90>"), ri(0x10374, "mov", "w0, #0x1"), ri(0x10378, "ldr", "x1, [x1, #0x420]"), ri(0x1037c, "br", "x1")];
        annotate("aarch64", &mut v, &got());
        assert_eq!(v[3].operands, "x1 <ext@GOT>");
        // A GOT branch is a tail call to the bound symbol, not a plain jump: it
        // must reach the call graph, gaps and stack as a call (#271 review).
        assert_eq!(crate::cfg::classify("aarch64", "br", &v[3].operands), crate::cfg::Ctl::TailCall);
        // and through `add` then `ldr [xB]`, the arm64e __auth_got shape
        let mut v = vec![ri(0x10, "adrp", "x17, 0x100004000"), ri(0x14, "add", "x17, x17, #0x8"), ri(0x18, "ldr", "x16, [x17]"), ri(0x1c, "blraa", "x16, x17")];
        annotate("aarch64", &mut v, &got());
        assert_eq!(v[3].operands, "x16, x17 <___chkstk_darwin@GOT>");
        assert_eq!(crate::cfg::classify("aarch64", "blraa", &v[3].operands), crate::cfg::Ctl::Call);
    }

    #[test]
    fn x86_64_call_through_gotpcrel_is_named() {
        // lld-linked x86_64-linux-gnu -fno-plt, as llvm-objdump prints it
        let mut v = vec![ri(0x1360, "jmpq", "*0x10ca(%rip)           # 0x2430 <fp+0x2430>"), ri(0x1385, "callq", "*0x10a5(%rip)           # 0x2430 <fp+0x2430>")];
        annotate("x86_64", &mut v, &got());
        assert_eq!(v[0].operands, "*0x10ca(%rip) <ext@GOT>");
        assert_eq!(crate::cfg::classify("x86_64", "jmpq", &v[0].operands), crate::cfg::Ctl::TailCall);
        assert_eq!(crate::cfg::classify("x86_64", "callq", &v[1].operands), crate::cfg::Ctl::Call);
        assert_eq!(crate::cfg::target(&v[0].operands), None);
    }

    #[test]
    fn x86_64_call_through_gotpcrel_is_named_in_an_image_with_branch_targets() {
        // The x86-64 sequence is the call alone. With any direct branch in the
        // image, checking that nothing enters it part-way once asked for the
        // empty range `addr + 1..=addr`, which panics (hosted CI, demo.c -O0).
        let mut v = vec![
            ri(0x1350, "jmp", "0x1385 <main+0x35>"),
            ri(0x1360, "jmpq", "*0x10ca(%rip)           # 0x2430 <fp+0x2430>"),
            ri(0x1385, "callq", "*0x10a5(%rip)           # 0x2430 <fp+0x2430>"),
        ];
        annotate("x86_64", &mut v, &got());
        assert_eq!(v[1].operands, "*0x10ca(%rip) <ext@GOT>");
        assert_eq!(v[2].operands, "*0x10a5(%rip) <ext@GOT>", "a branch target at the call itself does not enter it part-way");
    }

    #[test]
    fn what_is_not_a_bound_slot_stays_indirect() {
        let g = got();
        let unchanged = |arch: &str, mut v: Vec<RawInstr>| {
            let before: Vec<String> = v.iter().map(|i| i.operands.clone()).collect();
            annotate(arch, &mut v, &g);
            let after: Vec<String> = v.iter().map(|i| i.operands.clone()).collect();
            assert_eq!(before, after);
            assert!(v.iter().all(|i| !i.operands.contains(MARK)));
        };
        // a function pointer read through the GOT: `fp(n)` loads twice
        unchanged("aarch64", vec![ri(0x0, "adrp", "x8, 0x20000"), ri(0x4, "ldr", "x8, [x8, #0x420]"), ri(0x8, "ldr", "x1, [x8]"), ri(0xc, "br", "x1")]);
        // ... and on x86-64, through a register
        unchanged("x86_64", vec![ri(0x0, "movq", "0x10c1(%rip), %rax      # 0x2430 <fp+0x2430>"), ri(0x7, "jmpq", "*(%rax)")]);
        // the register is overwritten after the load
        unchanged("aarch64", vec![ri(0x0, "adrp", "x16, 0x100004000"), ri(0x4, "ldr", "x16, [x16, #0x8]"), ri(0x8, "mov", "x16, x0"), ri(0xc, "blr", "x16")]);
        // the base is overwritten between the adrp and the load
        unchanged("aarch64", vec![ri(0x0, "adrp", "x16, 0x100004000"), ri(0x4, "ldr", "x16, [x16, #0x8]!"), ri(0x8, "blr", "x16")]);
        unchanged("aarch64", vec![ri(0x0, "adrp", "x16, 0x100004000"), ri(0x4, "add", "x16, x16, x2"), ri(0x8, "ldr", "x16, [x16, #0x8]"), ri(0xc, "blr", "x16")]);
        // a slot the tables do not bind (a local symbol's rebase)
        unchanged("aarch64", vec![ri(0x0, "adrp", "x16, 0x100004000"), ri(0x4, "ldr", "x16, [x16, #0x10]"), ri(0x8, "blr", "x16")]);
        // a branch lands between the load and the call: another path could
        // bring another value
        unchanged("aarch64", vec![ri(0x0, "adrp", "x16, 0x100004000"), ri(0x4, "ldr", "x16, [x16, #0x8]"), ri(0x8, "blr", "x16"), ri(0xc, "b", "0x4"), ri(0x10, "b", "0x8")]);
        // a control transfer inside the sequence
        unchanged("aarch64", vec![ri(0x0, "adrp", "x16, 0x100004000"), ri(0x4, "ldr", "x16, [x16, #0x8]"), ri(0x8, "bl", "0x40"), ri(0xc, "blr", "x16")]);
        // an x86 memory-indirect call with no computed address, or not
        // rip-relative
        unchanged("x86_64", vec![ri(0x0, "callq", "*0x10(%rax)           # 0x2430"), ri(0x8, "callq", "*0x10a5(%rip)")]);
        // and an architecture this does not read
        unchanged("riscv64", vec![ri(0x0, "auipc", "t1, 0x2"), ri(0x4, "ld", "t1, 0x420(t1)"), ri(0x8, "jalr", "t1")]);
    }

    #[test]
    fn only_glob_dat_binds_a_got_slot_to_an_import() {
        use object::elf::*;
        let r = |offset: u64, r_type: u32, n: &str| Reloc { offset, r_type, name: n.to_string(), addend: 0 };
        let none = |_| None;
        let x = got_slots(
            object::Architecture::X86_64,
            [r(0x2430, R_X86_64_GLOB_DAT, "ext"), r(0x2438, R_X86_64_JUMP_SLOT, "puts"), r(0x2440, R_X86_64_RELATIVE, ""), r(0x2448, R_X86_64_64, "table")],
            |_| true,
            none,
        );
        assert_eq!(x, HashMap::from([(0x2430, Bound::import("ext"))]));
        let a = got_slots(object::Architecture::Aarch64, [r(0x20420, R_AARCH64_GLOB_DAT, "ext"), r(0x20428, R_AARCH64_JUMP_SLOT, "puts")], |_| true, none);
        assert_eq!(a, HashMap::from([(0x20420, Bound::import("ext"))]));
        assert!(got_slots(object::Architecture::Riscv64, [r(0x10, 6, "ext")], |_| true, none).is_empty());
    }

    /// rustc on x86-64 Linux calls another crate's functions through the GOT
    /// (`-Zplt=no`), and in a PIE the slot of a function linked into the
    /// image is an `R_X86_64_RELATIVE` relocation: `demo::average`'s
    /// divide-by-zero panic, rustc 1.97.1 linked by rust-lld (#325).
    #[test]
    fn a_relative_got_slot_binds_the_function_at_its_addend() {
        use object::elf::*;
        let rel = |offset: u64, addend: i64| Reloc { offset, r_type: R_X86_64_RELATIVE, name: String::new(), addend };
        let in_got = |a: u64| (0x55900..0x55a00).contains(&a);
        let func_at = |a: u64| (a == 0x50c10).then(|| "_ZN4core9panicking11panic_const23panic_const_div_by_zero17h0123456789abcdefE".to_string());
        let x = got_slots(
            object::Architecture::X86_64,
            [
                rel(0x559c0, 0x50c10),
                // into a function's middle, or not a function at all
                rel(0x559c8, 0x50c14),
                // a table of pointers outside .got: TASKS, a vtable
                rel(0x56000, 0x50c10),
                rel(0x559d0, -8),
            ],
            in_got,
            func_at,
        );
        assert_eq!(x.len(), 1, "{x:?}");
        let b = &x[&0x559c0];
        assert_eq!(b.addr, Some(0x50c10));

        let mut v = vec![
            ri(0x151bd, "jne", "0x151e2 <demo::average+0xb2>"),
            ri(0x151c8, "callq", "*0x407f2(%rip)          # 0x559c0 <writev+0x559c0>"),
            ri(0x151e9, "callq", "*0x407d9(%rip)          # 0x559c8 <writev+0x559c8>"),
        ];
        annotate("x86_64", &mut v, &x);
        assert!(v[1].operands.starts_with("*0x407f2(%rip) 0x50c10 <_ZN4core9panicking11panic_const23panic_const_div_by_zero"), "{}", v[1].operands);
        assert_eq!(crate::cfg::classify("x86_64", "callq", &v[1].operands), crate::cfg::Ctl::Call);
        assert_eq!(crate::cfg::target(&v[1].operands), Some(0x50c10), "the callee is in the image, at the slot's addend");
        assert!(crate::cfg::target_symbol(&v[1].operands).unwrap().contains("panic_const_div_by_zero"));
        assert_eq!(crate::cfg::classify("x86_64", "callq", &v[2].operands), crate::cfg::Ctl::IndirectCall, "an unbound slot stays indirect");
    }

    #[test]
    fn x86_64_macho_double_hash_got_comment_is_named() {
        let mut v = vec![
            ri(0x1360, "jmpq", "*0x10ca(%rip)           ## 0x2430 <fp+0x2430>"),
            ri(0x1385, "callq", "*0x10a5(%rip)           ## 0x2430 <fp+0x2430>"),
            ri(0x1390, "notrack", "callq *0x109a(%rip)    # 0x2430"),
        ];
        annotate("x86_64", &mut v, &got());
        assert_eq!(v[0].operands, "*0x10ca(%rip) <ext@GOT>");
        assert_eq!(v[1].operands, "*0x10a5(%rip) <ext@GOT>");
        assert_eq!(crate::cfg::classify("x86_64", "jmpq", &v[0].operands), crate::cfg::Ctl::TailCall);
        assert_eq!(crate::cfg::classify("x86_64", "callq", &v[1].operands), crate::cfg::Ctl::Call);
        assert!(v[2].operands.ends_with("<ext@GOT>"), "{:?}", v[2]);
        assert_eq!(crate::cfg::classify("x86_64", &v[2].mnemonic, &v[2].operands), crate::cfg::Ctl::Call);
    }

    #[test]
    fn disputed_got_slots_are_dropped() {
        use object::elf::*;
        let r = |offset: u64, n: &str| Reloc { offset, r_type: R_X86_64_GLOB_DAT, name: n.to_string(), addend: 0 };
        let x = got_slots(object::Architecture::X86_64, [r(0x2430, "a"), r(0x2430, "b"), r(0x2438, "c"), r(0x2438, "c")], |_| true, |_| None);
        assert_eq!(x, HashMap::from([(0x2438, Bound::import("c"))]));
    }
}
