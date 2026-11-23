// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Switch jump tables, recovered from the object code (#171).
//!
//! A dense `switch` compiles to an indirect branch through a table of
//! offsets: `br x8` on AArch64, `jmpq *%rax` on x86-64. Without reading the
//! table, the indirect branch has no known successors, so every case block
//! looked unreachable, and where the compiler places the table inside the
//! function's own symbol (clang at `-O0` on AArch64, and on x86-64 Mach-O)
//! its bytes were disassembled and analysed as instructions -- `udf`, or
//! whatever garbage the offsets decode to, with a line-0 gap each.
//!
//! The table is recovered from the instructions that use it, the way the
//! compiler emits it, and only when every piece is found: the bounds check
//! that gives the entry count, the table's address, the entry width and
//! signedness, the base the entries are added to, and a target for every
//! entry inside the function. Anything less is not a table this module
//! claims to have read, and the branch stays unresolved.
//!
//! The shapes read, as `llvm-objdump --print-imm-hex` prints them:
//!
//! AArch64, clang `-O0`: entries are 32-bit signed offsets from an `adr` base
//! ```text
//! subs x8, x8, #0xf ; b.hi DEFAULT
//! adrp x10, PAGE ; add x10, x10, #OFF        table
//! adr  x8, BASE
//! ldrsw x9, [x10, x11, lsl #2]
//! add  x8, x8, x9
//! br   x8
//! ```
//! AArch64, clang `-O1` and up: byte (or halfword) entries, scaled by 4
//! ```text
//! cmp w0, #0xf ; b.hi DEFAULT
//! adrp x9, PAGE ; add x9, x9, #OFF
//! adr  x10, BASE
//! ldrb w11, [x9, x8]
//! add  x10, x10, x11, lsl #2
//! br   x10
//! ```
//! x86-64, clang: 32-bit signed offsets from the table itself
//! ```text
//! cmpl $0xf, %edi ; ja DEFAULT               (or subq $0xf, %rax)
//! leaq OFF(%rip), %rcx       ## TABLE
//! movslq (%rcx,%rax,4), %rax
//! addq %rcx, %rax
//! jmpq *%rax
//! ```

use crate::objectcode::RawInstr;
use std::collections::BTreeSet;

/// A table this module read, and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// The indirect branch that goes through it.
    pub branch: u64,
    /// Where the table is, and how many bytes it occupies.
    pub addr: u64,
    pub len: u64,
    /// One target per entry, in entry order (cases may share a target).
    pub targets: Vec<u64>,
}

/// How far back from the branch the pieces may be, in instructions. The
/// compiler emits them together; a search that wandered further would be
/// matching unrelated code.
const WINDOW: usize = 16;

/// Every jump table `raw` (one function, `start..end`) branches through.
/// `read(addr, len)` returns the image's bytes at a virtual address.
pub fn recover(arch: &str, raw: &[RawInstr], start: u64, end: u64, read: &dyn Fn(u64, usize) -> Option<Vec<u8>>) -> Vec<Table> {
    let targets: BTreeSet<u64> = raw
        .iter()
        .filter(|i| matches!(crate::cfg::classify(arch, &i.mnemonic, &i.operands), crate::cfg::Ctl::Jump | crate::cfg::Ctl::CondBranch | crate::cfg::Ctl::Call))
        .filter_map(|i| crate::cfg::target(&i.operands))
        .collect();
    let mut out = Vec::new();
    for (k, ins) in raw.iter().enumerate() {
        let t = match arch {
            "aarch64" => aarch64(raw, k, ins, &targets, read),
            "x86_64" => x86_64(raw, k, ins, &targets, read),
            _ => None,
        };
        if let Some(t) = t.filter(|t| !t.targets.is_empty() && t.targets.iter().all(|&a| a >= start && a < end)) {
            out.push(t);
        }
    }
    out
}

/// Remove from `instrs` (sorted by address) every instruction that lies in
/// a table's bytes: data the disassembler decoded as code. The tables'
/// ranges are merged and each instruction is looked up by binary search,
/// so this is O((instructions + tables) log tables); asking every table
/// about every instruction was their product, which a large image with
/// many switches reaches without any crafting (#311).
pub fn strip(instrs: &mut Vec<RawInstr>, tables: &[Table]) {
    let mut spans: Vec<(u64, u64)> = tables.iter().map(|t| (t.addr, t.addr.saturating_add(t.len))).filter(|(a, e)| a < e).collect();
    spans.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(spans.len());
    for (a, e) in spans {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((a, e)),
        }
    }
    instrs.retain(|i| {
        let k = merged.partition_point(|&(a, _)| a <= i.addr);
        !(k > 0 && i.addr < merged[k - 1].1)
    });
}

fn hex_or_dec(s: &str) -> Option<u64> {
    let s = s.trim().trim_start_matches('#').trim_start_matches('$');
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// The first `0x…` address in an operand string (`adr x8, 0x10000052c <…>`).
fn address_in(s: &str) -> Option<u64> {
    let i = s.find("0x")?;
    let h: String = s[i + 2..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    u64::from_str_radix(&h, 16).ok()
}

/// The operands without objdump's trailing comment: `// …` on AArch64,
/// `## …` on x86-64 Mach-O and `# …` on x86-64 ELF (#325). An AArch64
/// immediate is a `#` too, but never one followed by a space.
fn strip_comment(s: &str) -> &str {
    let s = s.split("//").next().unwrap_or(s);
    let end = s.char_indices().find(|&(i, c)| c == '#' && s[i + 1..].starts_with(['#', ' ', '\t'])).map_or(s.len(), |(i, _)| i);
    &s[..end]
}

fn ops(s: &str) -> Vec<String> {
    // split on commas outside brackets/parens
    let mut out = Vec::new();
    let (mut depth, mut cur) = (0i32, String::new());
    for c in strip_comment(s).chars() {
        match c {
            '[' | '(' => {
                depth += 1;
                cur.push(c)
            }
            ']' | ')' => {
                depth -= 1;
                cur.push(c)
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// `w11` and `x11` are one register.
fn a64reg(r: &str) -> String {
    let r = r.trim();
    match r.strip_prefix('w').or_else(|| r.strip_prefix('x')) {
        Some(n) if n.chars().all(|c| c.is_ascii_digit()) => format!("r{n}"),
        _ => r.to_string(),
    }
}

/// The instruction before `k` (within the window and the same basic block)
/// that writes `reg`. Stops at any control transfer or incoming branch target
/// so definitions from another block cannot be mismatched (#9.7).
fn last_def<'a>(
    arch: &str,
    raw: &'a [RawInstr],
    k: usize,
    targets: &BTreeSet<u64>,
    reg: &str,
    norm: fn(&str) -> String,
    dst: fn(&[String]) -> Option<String>,
) -> Option<(usize, &'a RawInstr)> {
    let lo = k.saturating_sub(WINDOW);
    for i in (lo..k).rev() {
        // Nothing may branch into raw[i + 1] between the def and k.
        if targets.contains(&raw[i + 1].addr) {
            return None;
        }
        if crate::cfg::classify(arch, &raw[i].mnemonic, &raw[i].operands) != crate::cfg::Ctl::None {
            return None;
        }
        if dst(&ops(&raw[i].operands)).is_some_and(|d| norm(&d) == reg) {
            return Some((i, &raw[i]));
        }
    }
    None
}

/// The entry count from the bounds check immediately preceding `k`'s basic
/// block: a compare or subtract of `#N` followed by an unsigned "higher"
/// branch to the default case. Must not cross another control transfer or
/// incoming branch target, and the compared register must match (or feed)
/// `index_reg` (#9.7).
#[allow(clippy::too_many_arguments)]
fn bound(
    arch: &str,
    raw: &[RawInstr],
    k: usize,
    targets: &BTreeSet<u64>,
    hi: &[&str],
    cmp: &[&str],
    imm_at: fn(&[String]) -> Option<&str>,
    cmp_reg_at: fn(&[String]) -> Option<String>,
    index_matches: impl Fn(&[RawInstr], usize, usize, &str) -> bool,
) -> Option<u64> {
    let lo = k.saturating_sub(WINDOW + 4);
    for i in (lo..k).rev() {
        // No other path may enter the jump-table dispatch block between the
        // bounds branch and the indirect branch.
        if targets.contains(&raw[i + 1].addr) {
            return None;
        }
        let ctl = crate::cfg::classify(arch, &raw[i].mnemonic, &raw[i].operands);
        if hi.contains(&raw[i].mnemonic.as_str()) {
            let ci = i.checked_sub(1)?;
            let c = raw.get(ci)?;
            if targets.contains(&raw[i].addr) || !cmp.contains(&c.mnemonic.as_str()) {
                return None;
            }
            let c_ops = ops(&c.operands);
            let cmp_reg = cmp_reg_at(&c_ops)?;
            if !index_matches(raw, ci, i, &cmp_reg) {
                return None;
            }
            // `N + 1` entries; a bound of u64::MAX has no count, so no table
            return hex_or_dec(imm_at(&c_ops)?).and_then(|n| n.checked_add(1));
        }
        if ctl != crate::cfg::Ctl::None {
            return None;
        }
    }
    None
}

fn entries(read: &dyn Fn(u64, usize) -> Option<Vec<u8>>, table: u64, count: u64, width: usize, signed: bool) -> Option<Vec<i64>> {
    if count == 0 || count > 4096 {
        return None;
    }
    let bytes = read(table, count as usize * width)?;
    Some(
        bytes
            .chunks_exact(width)
            .map(|c| match (width, signed) {
                (1, _) => c[0] as i64,
                (2, _) => u16::from_le_bytes([c[0], c[1]]) as i64,
                (4, true) => i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as i64,
                (4, false) => u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as i64,
                _ => 0,
            })
            .collect(),
    )
}

fn aarch64(raw: &[RawInstr], k: usize, br: &RawInstr, targets: &BTreeSet<u64>, read: &dyn Fn(u64, usize) -> Option<Vec<u8>>) -> Option<Table> {
    if br.mnemonic != "br" {
        return None;
    }
    let dst = |o: &[String]| o.first().cloned();
    let target_reg = a64reg(&br.operands);
    // add Xt, Xbase, Xentry[, lsl #s]
    let (ai, add) = last_def("aarch64", raw, k, targets, &target_reg, a64reg, dst)?;
    if add.mnemonic != "add" {
        return None;
    }
    let o = ops(&add.operands);
    let (base_reg, entry_reg) = (a64reg(o.get(1)?), a64reg(o.get(2)?));
    let shift = match o.get(3).map(|s| s.as_str()) {
        None => 0,
        Some(s) => s.strip_prefix("lsl #").and_then(hex_or_dec).and_then(|n| u32::try_from(n).ok())?,
    };
    // adr Xbase, BASE
    let (_, adr) = last_def("aarch64", raw, ai, targets, &base_reg, a64reg, dst)?;
    if adr.mnemonic != "adr" {
        return None;
    }
    let base = address_in(&adr.operands)?;
    // ldrsw/ldrb/ldrh Xentry, [Xtable, Xindex(, lsl #n)]
    let (li, ld) = last_def("aarch64", raw, ai, targets, &entry_reg, a64reg, dst)?;
    let (width, signed) = match ld.mnemonic.as_str() {
        "ldrsw" => (4, true),
        "ldrb" => (1, false),
        "ldrh" => (2, false),
        _ => return None,
    };
    let mem = ops(&ld.operands).get(1)?.trim_matches(|c| c == '[' || c == ']').to_string();
    let mut mem_parts = mem.split(',').map(str::trim);
    let table_reg = a64reg(mem_parts.next()?);
    let index_reg = a64reg(mem_parts.next()?);
    // adrp Xtable, PAGE ; add Xtable, Xtable, #OFF
    let (pi, off) = last_def("aarch64", raw, li, targets, &table_reg, a64reg, dst)?;
    if off.mnemonic != "add" {
        return None;
    }
    let off_ops = ops(&off.operands);
    if off_ops.get(1).map(|s| a64reg(s)).as_deref() != Some(table_reg.as_str()) {
        return None;
    }
    let off_imm = hex_or_dec(off_ops.get(2)?)?;
    let (_, adrp) = last_def("aarch64", raw, pi, targets, &table_reg, a64reg, dst)?;
    if adrp.mnemonic != "adrp" {
        return None;
    }
    let table = address_in(&adrp.operands)?.checked_add(off_imm)?;
    let a64_cmp_reg = |o: &[String]| {
        // `cmp w0, #N` has 2 operands (reg at 0); `subs x8, x8, #N` has 3 (src reg at 1)
        if o.len() >= 3 {
            o.get(1).map(|s| a64reg(s))
        } else {
            o.first().map(|s| a64reg(s))
        }
    };
    let index_matches = |raw: &[RawInstr], ci: usize, hi_idx: usize, cmp_reg: &str| -> bool {
        if index_reg == cmp_reg {
            return true;
        }
        // Check how `index_reg` was defined in the dispatch block (after b.hi, before ld):
        // either copied/extended from `cmp_reg` (e.g. `mov w8, w0`), or reloaded from a
        // stack slot that `cmp_reg` was spilled to before `cmp`/`subs` (-O0).
        if let Some((_, def)) = last_def("aarch64", raw, li, targets, &index_reg, a64reg, dst) {
            let d_ops = ops(&def.operands);
            if matches!(def.mnemonic.as_str(), "mov" | "uxtw" | "sxtw" | "uxtb" | "uxth" | "ubfx" | "and" | "orr") && d_ops.get(1).is_some_and(|s| a64reg(s) == cmp_reg) {
                return true;
            }
            if matches!(def.mnemonic.as_str(), "ldr" | "ldur" | "ldrsw") {
                if let Some(slot) = d_ops.get(1) {
                    // Look back a few instructions before `ci` (same block) for `str cmp_reg, slot`
                    let spill_lo = ci.saturating_sub(6);
                    for j in (spill_lo..ci).rev() {
                        if targets.contains(&raw[j + 1].addr) || crate::cfg::classify("aarch64", &raw[j].mnemonic, &raw[j].operands) != crate::cfg::Ctl::None {
                            break;
                        }
                        if matches!(raw[j].mnemonic.as_str(), "str" | "stur") {
                            let s_ops = ops(&raw[j].operands);
                            if s_ops.first().is_some_and(|s| a64reg(s) == cmp_reg) && s_ops.get(1) == Some(slot) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        let _ = hi_idx;
        false
    };
    let count = bound("aarch64", raw, k, targets, &["b.hi"], &["cmp", "subs"], |o| o.last().map(|s| s.as_str()), a64_cmp_reg, index_matches)?;
    // A shift or target that does not fit is not a scaling clang emits:
    // the branch stays unresolved rather than given wrong targets.
    let scale = 1i64.checked_shl(shift).filter(|s| *s > 0)?;
    let targets = entries(read, table, count, width, signed)?.into_iter().map(|e| e.checked_mul(scale).and_then(|d| base.checked_add_signed(d))).collect::<Option<_>>()?;
    Some(Table { branch: br.addr, addr: table, len: count * width as u64, targets })
}

/// `%eax`, `%rax` and `%ax` are one register; so are `%r8d` and `%r8`.
fn x86reg(r: &str) -> String {
    let r = r.trim().trim_start_matches('*').trim_start_matches('%');
    if let Some(n) = r.strip_prefix('r').filter(|n| n.chars().next().is_some_and(|c| c.is_ascii_digit())) {
        return n.trim_end_matches(['d', 'w', 'b']).to_string();
    }
    r.trim_start_matches(['r', 'e']).trim_end_matches(['x', 'l', 'h']).to_string()
}

fn x86_64(raw: &[RawInstr], k: usize, jmp: &RawInstr, targets: &BTreeSet<u64>, read: &dyn Fn(u64, usize) -> Option<Vec<u8>>) -> Option<Table> {
    let (jm, jops) = crate::cfg::unwrap_x86_prefix(&jmp.mnemonic, &jmp.operands);
    if !(jm == "jmpq" || jm == "jmp") || !jops.trim().starts_with("*%") {
        return None;
    }
    // AT&T: the destination is the last operand
    let dst = |o: &[String]| o.last().cloned();
    let target_reg = x86reg(jops);
    // addq %table, %target
    let (ai, add) = last_def("x86_64", raw, k, targets, &target_reg, x86reg, dst)?;
    if !add.mnemonic.starts_with("add") {
        return None;
    }
    let table_reg = x86reg(ops(&add.operands).first()?);
    // movslq (%table,%index,4), %target
    let (li, ld) = last_def("x86_64", raw, ai, targets, &target_reg, x86reg, dst)?;
    if ld.mnemonic != "movslq" {
        return None;
    }
    let mem = ops(&ld.operands).first()?.trim_matches(|c| c == '(' || c == ')').to_string();
    let parts: Vec<&str> = mem.split(',').map(str::trim).collect();
    if parts.len() != 3 || x86reg(parts[0]) != table_reg || parts[2] != "4" {
        return None;
    }
    let index_reg = x86reg(parts[1]);
    // leaq OFF(%rip), %table   ## TABLE  (Mach-O; `# TABLE` on ELF)
    let (_, lea) = last_def("x86_64", raw, ai, targets, &table_reg, x86reg, dst)?;
    if !lea.mnemonic.starts_with("lea") || !lea.operands.contains("(%rip)") {
        return None;
    }
    let table = lea.operands.get(strip_comment(&lea.operands).len()..).and_then(address_in)?;
    let x86_cmp_reg = |o: &[String]| o.get(1).map(|s| x86reg(s));
    let index_matches = |raw: &[RawInstr], ci: usize, _hi_idx: usize, cmp_reg: &str| -> bool {
        if index_reg == cmp_reg {
            return true;
        }
        if let Some((_, def)) = last_def("x86_64", raw, li, targets, &index_reg, x86reg, dst) {
            let d_ops = ops(&def.operands);
            if def.mnemonic.starts_with("mov") {
                if let Some(src) = d_ops.first() {
                    if src.starts_with('%') && x86reg(src) == cmp_reg {
                        return true;
                    }
                    // -O0 reload from stack slot where cmp_reg was spilled before cmp/sub
                    if src.contains('(') {
                        let spill_lo = ci.saturating_sub(6);
                        for j in (spill_lo..ci).rev() {
                            if targets.contains(&raw[j + 1].addr) || crate::cfg::classify("x86_64", &raw[j].mnemonic, &raw[j].operands) != crate::cfg::Ctl::None {
                                break;
                            }
                            if raw[j].mnemonic.starts_with("mov") {
                                let s_ops = ops(&raw[j].operands);
                                if s_ops.first().is_some_and(|s| s.starts_with('%') && x86reg(s) == cmp_reg) && s_ops.last() == Some(src) {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }
        false
    };
    let count = bound("x86_64", raw, k, targets, &["ja"], &["cmpl", "cmpq", "subq", "subl"], |o| o.first().map(|s| s.as_str()), x86_cmp_reg, index_matches)?;
    let targets = entries(read, table, count, 4, true)?.into_iter().map(|e| table.checked_add_signed(e)).collect::<Option<_>>()?;
    Some(Table { branch: jmp.addr, addr: table, len: count * 4, targets })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ri(addr: u64, m: &str, o: &str) -> RawInstr {
        RawInstr { addr, mnemonic: m.into(), operands: o.into() }
    }

    /// A reader over one span of little-endian bytes.
    fn reader(at: u64, bytes: Vec<u8>) -> impl Fn(u64, usize) -> Option<Vec<u8>> {
        move |a, n| {
            let off = a.checked_sub(at)? as usize;
            bytes.get(off..off + n).map(<[u8]>::to_vec)
        }
    }

    /// The exact `-O0` sequence clang emits for examples/c-switch on arm64.
    #[test]
    fn aarch64_o0_word_offsets_from_an_adr_base() {
        let raw = vec![
            ri(0x508, "ldur", "w8, [x29, #-0x4]"),
            ri(0x50c, "subs", "w8, w8, #0x0"),
            ri(0x510, "mov", "w8, w8"),
            ri(0x514, "str", "x8, [sp]"),
            ri(0x518, "subs", "x8, x8, #0xf"),
            ri(0x51c, "b.hi", "0x720 <_dispatch+0x228>"),
            ri(0x520, "ldr", "x11, [sp]"),
            ri(0x524, "adrp", "x10, 0x0 <_puts+0x0>"),
            ri(0x528, "add", "x10, x10, #0x740"),
            ri(0x52c, "adr", "x8, 0x52c <_dispatch+0x34>"),
            ri(0x530, "ldrsw", "x9, [x10, x11, lsl #2]"),
            ri(0x534, "add", "x8, x8, x9"),
            ri(0x538, "br", "x8"),
        ];
        let offsets: [i32; 16] = [0x10, 0x28, 0x4c, 0x64, 0x8c, 0xa8, 0xc0, 0xe4, 0x100, 0x11c, 0x140, 0x158, 0x170, 0x198, 0x1b4, 0x1d0];
        let bytes: Vec<u8> = offsets.iter().flat_map(|o| o.to_le_bytes()).collect();
        let t = recover("aarch64", &raw, 0x4f8, 0x780, &reader(0x740, bytes));
        assert_eq!(t.len(), 1, "{t:?}");
        assert_eq!((t[0].branch, t[0].addr, t[0].len), (0x538, 0x740, 64));
        assert_eq!(t[0].targets[0], 0x52c + 0x10);
        assert_eq!(t[0].targets[15], 0x52c + 0x1d0);
    }

    /// `-O2`: byte entries, scaled by four, from a table outside the function.
    #[test]
    fn aarch64_o2_byte_entries_scaled_by_four() {
        let raw = vec![
            ri(0x548, "cmp", "w0, #0xf"),
            ri(0x54c, "b.hi", "0x728 <_dispatch+0x1e0>"),
            ri(0x550, "mov", "w8, w0"),
            ri(0x554, "adrp", "x9, 0x0 <_puts+0x0>"),
            ri(0x558, "add", "x9, x9, #0x78c"),
            ri(0x55c, "adr", "x10, 0x56c <_dispatch+0x24>"),
            ri(0x560, "ldrb", "w11, [x9, x8]"),
            ri(0x564, "add", "x10, x10, x11, lsl #2"),
            ri(0x568, "br", "x10"),
        ];
        let bytes = vec![0x00, 0x47, 0x29, 0x37, 0x11, 0x4f, 0x5a, 0x40, 0x68, 0x21, 0x62, 0x0b, 0x18, 0x55, 0x06, 0x2f];
        let t = recover("aarch64", &raw, 0x548, 0x780, &reader(0x78c, bytes));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].targets[0], 0x56c);
        assert_eq!(t[0].targets[1], 0x56c + 0x47 * 4);
        assert_eq!(t[0].len, 16);
    }

    #[test]
    fn x86_64_offsets_from_the_table_itself() {
        let raw = vec![
            ri(0x514, "cmpl", "$0xf, %edi"),
            ri(0x517, "ja", "0x630 <_dispatch+0x120>"),
            ri(0x51d, "movl", "%edi, %eax"),
            ri(0x51f, "leaq", "0x11a(%rip), %rcx       ## 0x640 <_dispatch+0x130>"),
            ri(0x526, "movslq", "(%rcx,%rax,4), %rax"),
            ri(0x52a, "addq", "%rcx, %rax"),
            ri(0x52d, "jmpq", "*%rax"),
        ];
        let offs: Vec<i32> = (0..16).map(|i| -0x111 + i * 0x10).collect();
        let bytes: Vec<u8> = offs.iter().flat_map(|o| o.to_le_bytes()).collect();
        let t = recover("x86_64", &raw, 0x510, 0x680, &reader(0x640, bytes));
        assert_eq!(t.len(), 1, "{t:?}");
        assert_eq!(t[0].targets[0], 0x640 - 0x111);
        assert_eq!((t[0].addr, t[0].len), (0x640, 64));
    }

    /// Exactly the tables' bytes leave the instruction list -- overlapping,
    /// adjacent and duplicated tables included -- and nothing else (#311).
    #[test]
    fn strip_removes_exactly_the_bytes_of_every_table() {
        let mut instrs: Vec<RawInstr> = (0..32u64).map(|k| ri(0x1000 + 4 * k, "nop", "")).collect();
        let t = |addr: u64, len: u64| Table { branch: 0, addr, len, targets: vec![] };
        let tables = [t(0x1010, 8), t(0x1014, 8), t(0x1018, 4), t(0x1040, 4), t(0x1040, 4), t(0x1070, 0), t(0x2000, 16)];
        strip(&mut instrs, &tables);
        let left: Vec<u64> = instrs.iter().map(|i| i.addr).collect();
        let gone = [0x1010, 0x1014, 0x1018, 0x1040];
        let want: Vec<u64> = (0..32u64).map(|k| 0x1000 + 4 * k).filter(|a| !gone.contains(a)).collect();
        assert_eq!(left, want);
    }

    /// The same table in an ELF image: GNU-mode objdump comments with one
    /// `#`, not Mach-O's `##` (#325). The lea's destination was read as
    /// `%rcx # 0x640 <…>`, so the table was never found on Linux.
    #[test]
    fn x86_64_elf_comment_with_a_single_hash() {
        let raw = vec![
            ri(0x1134, "movl", "-0x4(%rbp), %eax"),
            ri(0x1137, "movq", "%rax, -0x10(%rbp)"),
            ri(0x113b, "subq", "$0xf, %rax"),
            ri(0x113f, "ja", "0x1250 <dispatch+0x130>"),
            ri(0x1145, "movq", "-0x10(%rbp), %rax"),
            ri(0x1149, "leaq", "-0x1088(%rip), %rcx       # 0x640 <dispatch+0x520>"),
            ri(0x1150, "movslq", "(%rcx,%rax,4), %rax"),
            ri(0x1154, "addq", "%rcx, %rax"),
            ri(0x1157, "jmpq", "*%rax"),
        ];
        let offs: Vec<i32> = (0..16).map(|i| 0xb20 + i * 0x10).collect();
        let bytes: Vec<u8> = offs.iter().flat_map(|o| o.to_le_bytes()).collect();
        let t = recover("x86_64", &raw, 0x1120, 0x1260, &reader(0x640, bytes));
        assert_eq!(t.len(), 1, "{t:?}");
        assert_eq!((t[0].branch, t[0].addr, t[0].len), (0x1157, 0x640, 64));
        assert_eq!(t[0].targets[0], 0x640 + 0xb20);
        assert_eq!(t[0].targets[15], 0x640 + 0xb20 + 15 * 0x10);
        // an AArch64 immediate is not a comment
        assert_eq!(ops("x10, x10, #0x740"), ["x10", "x10", "#0x740"]);
        assert_eq!(ops("%rcx # 0x640 <dispatch+0x520>"), ["%rcx"]);
    }

    /// Anything missing, and nothing is claimed: no bounds check, a target
    /// outside the function, or a table that cannot be read.
    #[test]
    fn an_incomplete_pattern_is_not_a_table() {
        let tail = |cmp: bool| {
            let mut v = vec![];
            if cmp {
                v.push(ri(0x548, "cmp", "w0, #0xf"));
                v.push(ri(0x54c, "b.hi", "0x728"));
            }
            v.extend([
                ri(0x554, "adrp", "x9, 0x0"),
                ri(0x558, "add", "x9, x9, #0x78c"),
                ri(0x55c, "adr", "x10, 0x56c"),
                ri(0x560, "ldrb", "w11, [x9, x8]"),
                ri(0x564, "add", "x10, x10, x11, lsl #2"),
                ri(0x568, "br", "x10"),
            ]);
            v
        };
        let bytes = vec![1u8; 16];
        assert!(recover("aarch64", &tail(false), 0x548, 0x780, &reader(0x78c, bytes.clone())).is_empty(), "no bounds check, no count");
        assert!(recover("aarch64", &tail(true), 0x548, 0x570, &reader(0x78c, bytes.clone())).is_empty(), "a target past the function's end");
        assert!(recover("aarch64", &tail(true), 0x548, 0x780, &reader(0x900, bytes)).is_empty(), "the table's bytes are not readable");
        assert!(recover("aarch64", &[ri(0x10, "br", "x16")], 0, 0x100, &|_, _| None).is_empty(), "a plain tail call");
    }

    /// An exhaustive match/switch with no bounds check of its own must not
    /// attach an unrelated earlier `cmp; b.hi` (across a branch or on another
    /// register), nor match `add x9, x0, #off` from another base (#9.7).
    #[test]
    fn unrelated_bounds_check_or_cross_block_def_is_rejected() {
        let bytes = vec![1u8; 64];
        // Unrelated register compared (w1 vs index w0 -> w8)
        let wrong_reg = vec![
            ri(0x548, "cmp", "w1, #0xf"),
            ri(0x54c, "b.hi", "0x728"),
            ri(0x550, "mov", "w8, w0"),
            ri(0x554, "adrp", "x9, 0x0"),
            ri(0x558, "add", "x9, x9, #0x78c"),
            ri(0x55c, "adr", "x10, 0x56c"),
            ri(0x560, "ldrb", "w11, [x9, x8]"),
            ri(0x564, "add", "x10, x10, x11, lsl #2"),
            ri(0x568, "br", "x10"),
        ];
        assert!(recover("aarch64", &wrong_reg, 0x548, 0x900, &reader(0x78c, bytes.clone())).is_empty(), "cmp on unrelated w1 must not bind switch on w0");

        // Another branch sits between the earlier cmp+b.hi and the jump table
        let cross_block = vec![
            ri(0x540, "cmp", "w0, #0xf"),
            ri(0x544, "b.hi", "0x728"),
            ri(0x548, "cbz", "w2, 0x554"),
            ri(0x550, "mov", "w8, w0"),
            ri(0x554, "adrp", "x9, 0x0"),
            ri(0x558, "add", "x9, x9, #0x78c"),
            ri(0x55c, "adr", "x10, 0x56c"),
            ri(0x560, "ldrb", "w11, [x9, x8]"),
            ri(0x564, "add", "x10, x10, x11, lsl #2"),
            ri(0x568, "br", "x10"),
        ];
        assert!(recover("aarch64", &cross_block, 0x540, 0x900, &reader(0x78c, bytes.clone())).is_empty(), "must not cross cbz/join to reach an earlier b.hi");

        // `add x9, x0, #0x78c` does not add to `x9` from `adrp`
        let wrong_add_src = vec![
            ri(0x548, "cmp", "w0, #0xf"),
            ri(0x54c, "b.hi", "0x728"),
            ri(0x550, "mov", "w8, w0"),
            ri(0x554, "adrp", "x9, 0x0"),
            ri(0x558, "add", "x9, x0, #0x78c"),
            ri(0x55c, "adr", "x10, 0x56c"),
            ri(0x560, "ldrb", "w11, [x9, x8]"),
            ri(0x564, "add", "x10, x10, x11, lsl #2"),
            ri(0x568, "br", "x10"),
        ];
        assert!(recover("aarch64", &wrong_add_src, 0x548, 0x900, &reader(0x78c, bytes)).is_empty(), "add with unrelated source register must not match adrp+add");
    }

    /// Intel CET `notrack jmpq *%rax` recovers its jump table (#9.4 / #9.7).
    #[test]
    fn x86_64_notrack_jmpq_recovers_jump_table() {
        let raw = vec![
            ri(0x514, "cmpl", "$0x3, %edi"),
            ri(0x517, "ja", "0x630 <_dispatch+0x120>"),
            ri(0x51d, "movl", "%edi, %eax"),
            ri(0x51f, "leaq", "0x11a(%rip), %rcx       # 0x640 <_dispatch+0x130>"),
            ri(0x526, "movslq", "(%rcx,%rax,4), %rax"),
            ri(0x52a, "addq", "%rcx, %rax"),
            ri(0x52d, "notrack", "jmpq\t*%rax"),
        ];
        let offs: Vec<u8> = [-0x110i32, -0x100, -0xf0, -0xe0].iter().flat_map(|o| o.to_le_bytes()).collect();
        let t = recover("x86_64", &raw, 0x510, 0x680, &reader(0x640, offs));
        assert_eq!(t.len(), 1, "{t:?}");
        assert_eq!(t[0].targets, vec![0x530, 0x540, 0x550, 0x560]);
    }
}
