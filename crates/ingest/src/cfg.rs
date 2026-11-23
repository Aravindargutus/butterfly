// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Object-level control-flow graph: basic blocks and edges derived purely
//! from the instruction stream of one function.

use crate::objectcode::RawInstr;
use butterfly_model::{Block, BlockKind, Edge, EdgeKind, Instr, InstrFlag, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ctl {
    None,
    Jump,
    CondBranch,
    Call,
    IndirectCall,
    IndirectBranch,
    Return,
    /// A branch through a bound GOT slot: a transfer to the named symbol that
    /// does not return to this function (a tail call, #271). It is a *call* to
    /// that symbol for the call graph, gaps and stack -- so it carries
    /// `InstrFlag::Call` -- but it *terminates* the block like a return, with
    /// no fall-through, because control does not come back here.
    TailCall,
}

/// Strip leading x86 instruction prefixes that `llvm-objdump` separates from
/// the mnemonic with whitespace (`notrack callq *%rax`, `notrack jmpq *%rax`,
/// `rep retq`, `bnd jmpq`, `data16 callq`, `rex64 jmpq`), returning the real
/// mnemonic and its operands.
pub(crate) fn unwrap_x86_prefix<'a>(mnemonic: &'a str, operands: &'a str) -> (&'a str, &'a str) {
    let mut m = mnemonic;
    let mut ops = operands.trim_start();
    for _ in 0..4 {
        let is_prefix = m.eq_ignore_ascii_case("notrack")
            || m.eq_ignore_ascii_case("bnd")
            || m.eq_ignore_ascii_case("rep")
            || m.eq_ignore_ascii_case("repz")
            || m.eq_ignore_ascii_case("repe")
            || m.eq_ignore_ascii_case("repne")
            || m.eq_ignore_ascii_case("repnz")
            || m.eq_ignore_ascii_case("data16")
            || m.eq_ignore_ascii_case("rex64");
        if !is_prefix || ops.is_empty() {
            break;
        }
        match ops.split_once(char::is_whitespace) {
            Some((next_m, rest)) => {
                m = next_m;
                ops = rest.trim_start();
            }
            None => {
                m = ops;
                ops = "";
            }
        }
    }
    (m, ops)
}

/// Classify one instruction for the given architecture.
///
/// A register- or memory-indirect call or branch whose operands carry a
/// `<symbol@GOT>` mark is one through a GOT slot the image binds to that
/// symbol ([`crate::got`], #271): a call, or a jump, to it -- the same
/// transfer a `<symbol@plt>` call makes, and treated the same way.
pub fn classify(arch: &str, mnemonic: &str, operands: &str) -> Ctl {
    let m = mnemonic.to_ascii_lowercase();
    let via_got = operands.contains(crate::got::MARK);
    match arch {
        a if a.starts_with("aarch64") => match m.as_str() {
            "b" => Ctl::Jump,
            "bl" => Ctl::Call,
            "blr" | "blraa" | "blrab" | "blraaz" | "blrabz" if via_got => Ctl::Call,
            "br" | "braa" | "brab" | "braaz" | "brabz" if via_got => Ctl::TailCall,
            "blr" | "blraa" | "blrab" | "blraaz" | "blrabz" => Ctl::IndirectCall,
            "br" | "braa" | "brab" | "braaz" | "brabz" => Ctl::IndirectBranch,
            "ret" | "retaa" | "retab" | "eret" => Ctl::Return,
            "cbz" | "cbnz" | "tbz" | "tbnz" => Ctl::CondBranch,
            _ if m.starts_with("b.") => Ctl::CondBranch,
            _ => Ctl::None,
        },
        a if a.starts_with("x86") || a.starts_with("i386") => {
            let (um, uops) = unwrap_x86_prefix(&m, operands);
            let um = um.to_ascii_lowercase();
            let uops = uops.trim_start();
            match um.as_str() {
                "jmp" | "jmpq" | "jmpl" => {
                    if via_got {
                        Ctl::TailCall
                    } else if uops.starts_with('*') {
                        Ctl::IndirectBranch
                    } else {
                        Ctl::Jump
                    }
                }
                "call" | "callq" | "calll" => {
                    if uops.starts_with('*') && !via_got {
                        Ctl::IndirectCall
                    } else {
                        Ctl::Call
                    }
                }
                "ret" | "retq" | "retl" | "iret" | "iretq" | "iretl" => Ctl::Return,
                _ if um.starts_with('j') => Ctl::CondBranch,
                _ if um == "loop" || um.starts_with("loop") => Ctl::CondBranch,
                _ => Ctl::None,
            }
        }
        a if a.starts_with("arm") => {
            // ARM/Thumb: mnemonic may carry a condition suffix and ".w".
            let base = m.trim_end_matches(".w").trim_end_matches(".n");
            let (root, cond) = split_arm_cond(base);
            match root {
                "b" => {
                    if cond.is_empty() || cond == "al" {
                        Ctl::Jump
                    } else {
                        Ctl::CondBranch
                    }
                }
                "bl" => Ctl::Call,
                "blx" => {
                    if operands.starts_with("0x") || operands.starts_with('#') {
                        Ctl::Call
                    } else {
                        Ctl::IndirectCall
                    }
                }
                "bx" => {
                    if operands.trim() == "lr" {
                        Ctl::Return
                    } else {
                        Ctl::IndirectBranch
                    }
                }
                "cbz" | "cbnz" => Ctl::CondBranch,
                "pop" | "ldm" | "ldmia" if operands.contains("pc") => Ctl::Return,
                _ => Ctl::None,
            }
        }
        a if a.starts_with("riscv") => match m.as_str() {
            "j" | "c.j" => Ctl::Jump,
            "jal" => match operands.split_once(',') {
                Some((dst, _)) if matches!(dst.trim(), "zero" | "x0") => Ctl::Jump,
                _ => Ctl::Call,
            },
            "call" | "c.jal" => Ctl::Call,
            "tail" => Ctl::TailCall,
            "jalr" => match operands.split_once(',') {
                Some((dst, _)) if matches!(dst.trim(), "zero" | "x0") => Ctl::IndirectBranch,
                _ => Ctl::IndirectCall,
            },
            "c.jalr" => Ctl::IndirectCall,
            "jr" | "c.jr" => {
                if matches!(operands.trim(), "ra" | "x1") {
                    Ctl::Return
                } else {
                    Ctl::IndirectBranch
                }
            }
            "ret" | "c.ret" | "mret" | "sret" => Ctl::Return,
            "beq" | "bne" | "blt" | "bge" | "bltu" | "bgeu" | "beqz" | "bnez" | "blez" | "bgez" | "bltz" | "bgtz" | "bgt" | "ble" | "bgtu" | "bleu" | "c.beqz" | "c.bnez" => {
                Ctl::CondBranch
            }
            _ => Ctl::None,
        },
        _ => Ctl::None,
    }
}

const ARM_CONDS: &[&str] = &["eq", "ne", "cs", "hs", "cc", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le", "al"];

/// Whether this instruction runs only when a condition holds.
///
/// 32-bit ARM predicates the transfer itself rather than branching around
/// it: `bleq <f>` calls `f` when Z is set and does nothing at all when it is
/// not. `classify` already folds the suffix away and returns `Ctl::Call`,
/// which is right for edge-building -- the call's own return lands on the
/// next instruction either way -- but it is *not* right for divergence. A
/// predicated call has a second way of reaching the next instruction that
/// has nothing to do with the callee: the condition being false, in which
/// case the callee never runs. So what the DWARF says about that callee
/// cannot remove the fall-through (#244 review).
///
/// `al` is the always-condition and is therefore not a predicate; `b.cc` on
/// AArch64 is a conditional *branch*, classified as one, and never reaches
/// here as a call.
pub fn is_predicated(arch: &str, mnemonic: &str) -> bool {
    if !arch.starts_with("arm") {
        return false;
    }
    let m = mnemonic.to_ascii_lowercase();
    let base = m.trim_end_matches(".w").trim_end_matches(".n");
    let (_, cond) = split_arm_cond(base);
    !cond.is_empty() && cond != "al"
}

fn split_arm_cond(m: &str) -> (&str, &str) {
    for root in ["blx", "bl", "bx", "b"] {
        if let Some(rest) = m.strip_prefix(root) {
            if rest.is_empty() {
                return (root, "");
            }
            if ARM_CONDS.contains(&rest) {
                return (root, rest);
            }
        }
    }
    (m, "")
}

/// Branch/call target address: the last `0x...` literal outside `<...>`.
/// An unconditional direct branch whose target is outside `start..end` is a
/// tail call: the compiler replaced `call f; ret` with `b f`, so the callee
/// runs and returns to this function's caller. It has no successor here, as
/// a return has none, and it is a call to everything that asks which
/// functions this one calls (#264).
pub fn is_tail_call(ctl: Ctl, operands: &str, start: u64, end: u64) -> bool {
    ctl == Ctl::Jump && target(operands).is_some_and(|t| t < start || t >= end)
}

pub fn target(operands: &str) -> Option<u64> {
    let mut s = operands.to_string();
    while let Some(i) = s.find('<') {
        match s[i..].find('>') {
            Some(j) => s.replace_range(i..i + j + 1, ""),
            None => break,
        }
    }
    let mut last = None;
    for tok in s.split(|c: char| c == ',' || c.is_whitespace() || c == '#') {
        let t = tok.trim_start_matches('*');
        if let Some(h) = t.strip_prefix("0x") {
            if let Ok(v) = u64::from_str_radix(h, 16) {
                last = Some(v);
            }
        }
    }
    last
}

/// Symbol named in `<sym>` or `<sym+0x..>`, if any.
pub fn target_symbol(operands: &str) -> Option<String> {
    let i = operands.find('<')?;
    let j = operands[i..].find('>')?;
    let inner = &operands[i + 1..i + j];
    let name = inner.split('+').next().unwrap_or(inner);
    let name = name.split('-').next().unwrap_or(name);
    let name = name.trim_end_matches("@plt").trim_end_matches("@PLT").trim_end_matches("@GOT");
    (!name.is_empty()).then(|| name.to_string())
}

pub struct FnCfg {
    pub blocks: Vec<Block>,
    pub cond_branches: u32,
}

/// (taken, not-taken) condition labels of a conditional branch.
pub fn branch_conditions(arch: &str, mnemonic: &str, operands: &str) -> Option<(String, String)> {
    let m = mnemonic.to_ascii_lowercase();
    let ops: Vec<&str> = operands.split(',').map(str::trim).collect();
    let neg = |cc: &str| -> String {
        match cc {
            "eq" => "ne",
            "ne" => "eq",
            "lt" => "ge",
            "ge" => "lt",
            "gt" => "le",
            "le" => "gt",
            "lo" | "cc" => "hs",
            "hs" | "cs" => "lo",
            "hi" => "ls",
            "ls" => "hi",
            "mi" => "pl",
            "pl" => "mi",
            "vs" => "vc",
            "vc" => "vs",
            other => return format!("!{other}"),
        }
        .to_string()
    };
    if arch.starts_with("aarch64") || arch.starts_with("arm") {
        if let Some(cc) = m.strip_prefix("b.") {
            return Some((cc.to_string(), neg(cc)));
        }
        if arch.starts_with("arm") {
            if let Some(cc) = m.strip_prefix('b').filter(|c| ARM_CONDS.contains(c)) {
                return Some((cc.to_string(), neg(cc)));
            }
        }
        let r = ops.first().copied().unwrap_or("?");
        return match m.as_str() {
            "cbz" => Some((format!("{r} == 0"), format!("{r} != 0"))),
            "cbnz" => Some((format!("{r} != 0"), format!("{r} == 0"))),
            "tbz" => Some((
                format!("{r}[{}] == 0", ops.get(1).map(|b| b.trim_start_matches('#')).unwrap_or("?")),
                format!("{r}[{}] == 1", ops.get(1).map(|b| b.trim_start_matches('#')).unwrap_or("?")),
            )),
            "tbnz" => Some((
                format!("{r}[{}] == 1", ops.get(1).map(|b| b.trim_start_matches('#')).unwrap_or("?")),
                format!("{r}[{}] == 0", ops.get(1).map(|b| b.trim_start_matches('#')).unwrap_or("?")),
            )),
            _ => None,
        };
    }
    if arch.starts_with("x86") || arch.starts_with("i386") {
        let (um, _) = unwrap_x86_prefix(&m, operands);
        let um = um.to_ascii_lowercase();
        let cc = um.trim_start_matches('j');
        let (t, f) = match cc {
            "e" | "z" => ("==", "!="),
            "ne" | "nz" => ("!=", "=="),
            "l" => ("< (signed)", ">= (signed)"),
            "ge" => (">= (signed)", "< (signed)"),
            "g" => ("> (signed)", "<= (signed)"),
            "le" => ("<= (signed)", "> (signed)"),
            "b" | "c" | "nae" => ("< (unsigned)", ">= (unsigned)"),
            "ae" | "nb" | "nc" => (">= (unsigned)", "< (unsigned)"),
            "a" | "nbe" => ("> (unsigned)", "<= (unsigned)"),
            "be" | "na" => ("<= (unsigned)", "> (unsigned)"),
            "s" => ("negative", "not negative"),
            "ns" => ("not negative", "negative"),
            "o" => ("overflow", "no overflow"),
            "no" => ("no overflow", "overflow"),
            "p" | "pe" => ("parity even", "parity odd"),
            "np" | "po" => ("parity odd", "parity even"),
            _ => return None,
        };
        return Some((t.to_string(), f.to_string()));
    }
    if arch.starts_with("riscv") {
        let a = ops.first().copied().unwrap_or("?");
        let b = ops.get(1).copied().unwrap_or("?");
        let base = m.trim_start_matches("c.");
        return match base {
            "beq" => Some((format!("{a} == {b}"), format!("{a} != {b}"))),
            "bne" => Some((format!("{a} != {b}"), format!("{a} == {b}"))),
            "blt" => Some((format!("{a} < {b}"), format!("{a} >= {b}"))),
            "bge" => Some((format!("{a} >= {b}"), format!("{a} < {b}"))),
            "bltu" => Some((format!("{a} <u {b}"), format!("{a} >=u {b}"))),
            "bgeu" => Some((format!("{a} >=u {b}"), format!("{a} <u {b}"))),
            "beqz" => Some((format!("{a} == 0"), format!("{a} != 0"))),
            "bnez" => Some((format!("{a} != 0"), format!("{a} == 0"))),
            "blez" => Some((format!("{a} <= 0"), format!("{a} > 0"))),
            "bgez" => Some((format!("{a} >= 0"), format!("{a} < 0"))),
            "bltz" => Some((format!("{a} < 0"), format!("{a} >= 0"))),
            "bgtz" => Some((format!("{a} > 0"), format!("{a} <= 0"))),
            _ => None,
        };
    }
    None
}

/// Why a block ends where it does with no fall-through successor, and on
/// whose authority.
///
/// A missing edge is a claim: it says no execution can continue past this
/// call. The instruction stream cannot establish that — a `bl` looks the
/// same whether or not its callee returns — so the claim needs a source, and
/// the source is named in the block's evidence.
///
/// There is one rule, and it is not a guess: the callee's own
/// `DW_AT_noreturn`, as the producer emitted it. C and C++ `_Noreturn` /
/// `[[noreturn]]` and any Rust `fn` returning an uninhabited type carry it.
/// A list of well-known diverging names (`abort`, `panic_bounds_check`, ...)
/// would reach further, into images whose debug info does not describe the
/// callee — but it is a heuristic: it reads a symbol's spelling as a
/// statement about its behaviour, and a program is free to define a function
/// called `abort` that returns. Nothing in this repository needs that reach,
/// so nothing here guesses; if a name list is ever added it belongs beside
/// this enum, labelled a heuristic, and its reason string must say so (#210).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DivergeRule {
    /// The callee's `DW_TAG_subprogram` in this image's DWARF has
    /// `DW_AT_noreturn`.
    DwarfNoreturn,
}

impl DivergeRule {
    /// The sentence recorded in the block, naming the rule and the callee.
    pub fn explain(self, callee: &str, addr: u64) -> String {
        match self {
            DivergeRule::DwarfNoreturn => {
                format!("no fall-through edge: the call at {addr:#x} targets {callee}, which this image's DWARF marks DW_AT_noreturn, so control does not continue past it")
            }
        }
    }
}

/// Build blocks for a function whose instructions occupy `[start, end)`.
///
/// `line_of` gives (line, file_id, is_stmt) per instruction index and
/// `status_of` gives the projected coverage per instruction index.
/// `diverging` is asked, for the target address of a direct call, whether
/// the callee is known not to return and by which rule.
// Each argument is a separate fact about the function the caller already
// holds; bundling them into a struct would only rename the list.
#[allow(clippy::too_many_arguments)]
pub fn build(
    arch: &str,
    start: u64,
    end: u64,
    raw: &[RawInstr],
    line_of: &dyn Fn(usize) -> (Option<u32>, Option<usize>, bool),
    status_of: &dyn Fn(usize) -> Status,
    diverging: &dyn Fn(u64) -> Option<(DivergeRule, String)>,
    table_targets: &dyn Fn(u64) -> Vec<u64>,
    landing_pads: &dyn Fn(u64) -> Vec<u64>,
) -> FnCfg {
    let n = raw.len();
    if n == 0 {
        return FnCfg { blocks: Vec::new(), cond_branches: 0 };
    }
    let ctl: Vec<Ctl> = raw.iter().map(|i| classify(arch, &i.mnemonic, &i.operands)).collect();
    let tgt: Vec<Option<u64>> = raw
        .iter()
        .zip(&ctl)
        .map(|(i, c)| match c {
            Ctl::Jump | Ctl::CondBranch => target(&i.operands).filter(|t| *t >= start && *t < end),
            _ => None,
        })
        .collect();

    // Divergence, per instruction rather than per block: an unconditional
    // direct call whose callee the producer marked as never returning.
    //
    // It is settled here, before leaders, because it *decides* a leader.
    // Read only at a block's last instruction it would miss any such call
    // that nothing happens to branch past -- one followed by padding, or by
    // dead instructions the compiler left in place -- and that block would
    // go on taking its successors from a later terminator, modelling flow
    // that continues through a call which cannot return (#244 review).
    let diverge_at: Vec<Option<String>> = raw
        .iter()
        .zip(&ctl)
        .map(|(i, c)| {
            if *c != Ctl::Call || is_predicated(arch, &i.mnemonic) {
                return None;
            }
            target(&i.operands).and_then(&diverging).map(|(rule, callee)| rule.explain(&callee, i.addr))
        })
        .collect();

    // An indirect branch through a switch table the caller recovered
    // (jumptable.rs) has those targets; one that was not has none, and its
    // successors stay unknown rather than guessed (#171).
    let indirect: Vec<Vec<usize>> = raw
        .iter()
        .zip(&ctl)
        .map(|(i, c)| {
            if *c != Ctl::IndirectBranch {
                return Vec::new();
            }
            let mut seen = std::collections::HashSet::new();
            table_targets(i.addr).into_iter().filter_map(|t| raw.binary_search_by_key(&t, |x| x.addr).ok()).filter(|k| seen.insert(*k)).collect()
        })
        .collect();

    // A call the function's exception table covers may land, if what it
    // calls throws, on a landing pad the unwinder jumps to (#165).
    let unwind: Vec<Vec<usize>> = raw
        .iter()
        .zip(&ctl)
        .map(|(i, c)| {
            if !matches!(c, Ctl::Call | Ctl::IndirectCall) {
                return Vec::new();
            }
            let mut seen = std::collections::HashSet::new();
            landing_pads(i.addr).into_iter().filter_map(|t| raw.binary_search_by_key(&t, |x| x.addr).ok()).filter(|k| seen.insert(*k)).collect()
        })
        .collect();

    // Leaders.
    let mut is_leader = vec![false; n];
    is_leader[0] = true;
    for i in 0..n {
        match ctl[i] {
            Ctl::Jump | Ctl::CondBranch | Ctl::Return | Ctl::IndirectBranch | Ctl::TailCall => {
                if i + 1 < n {
                    is_leader[i + 1] = true;
                }
                if let Some(t) = tgt[i] {
                    if let Ok(k) = raw.binary_search_by_key(&t, |x| x.addr) {
                        is_leader[k] = true;
                    }
                }
                for &k in &indirect[i] {
                    is_leader[k] = true;
                }
            }
            // A call that does not return ends its block here, so whatever
            // follows starts a new one -- reached by a branch or by nothing
            // at all, but never by falling out of this call.
            Ctl::Call if diverge_at[i].is_some() && i + 1 < n => is_leader[i + 1] = true,
            _ => {}
        }
    }
    for ks in &unwind {
        for &k in ks {
            is_leader[k] = true;
        }
    }
    let mut block_of = vec![0usize; n];
    let mut starts = Vec::new();
    for i in 0..n {
        if is_leader[i] {
            starts.push(i);
        }
        block_of[i] = starts.len() - 1;
    }

    let mut blocks = Vec::with_capacity(starts.len());
    let mut cond_branches = 0u32;
    for (b, &s) in starts.iter().enumerate() {
        let e = starts.get(b + 1).copied().unwrap_or(n);
        let mut instrs = Vec::with_capacity(e - s);
        let mut lines = Vec::new();
        for i in s..e {
            let (line, file_id, is_stmt) = line_of(i);
            let mut flags = Vec::new();
            match ctl[i] {
                // A tail call through the GOT is a call to the bound symbol for
                // every consumer that reads the call graph, gaps and stack, so
                // it carries the Call flag; the block termination below is what
                // keeps it from falling through (#271 review).
                Ctl::Call | Ctl::TailCall => flags.push(InstrFlag::Call),
                Ctl::IndirectCall => flags.push(InstrFlag::IndirectCall),
                Ctl::Jump => {
                    flags.push(InstrFlag::Branch);
                    if is_tail_call(ctl[i], &raw[i].operands, start, end) {
                        flags.push(InstrFlag::Call);
                    }
                }
                Ctl::CondBranch => {
                    flags.push(InstrFlag::CondBranch);
                    cond_branches += 1;
                }
                Ctl::IndirectBranch => flags.push(InstrFlag::IndirectBranch),
                Ctl::Return => flags.push(InstrFlag::Return),
                Ctl::None => {}
            }
            if let Some(l) = line {
                if l != 0 && !lines.contains(&l) {
                    lines.push(l);
                }
            }
            instrs.push(Instr {
                addr: raw[i].addr,
                mnemonic: raw[i].mnemonic.clone(),
                operands: raw[i].operands.clone(),
                line,
                file_id,
                is_stmt,
                projected: status_of(i),
                flags,
                gap_ids: Vec::new(),
                measured: None,
                inlined: None,
                origin: None,
            });
        }
        lines.sort_unstable();
        let projected = summarise(instrs.iter().map(|i| i.projected));

        let last = e - 1;
        // A direct call whose callee is known not to return ends the block:
        // the instruction after it is reached by nothing, so there is no
        // fall-through edge to draw. Without this the panic call `rustc`
        // emits for a bounds check gets a successor no execution can take,
        // which shows up as a never-taken outcome and an arrow in the Flow
        // view that stands for nothing (#210).
        //
        // Decided in `diverge_at` above, where it also made this the last
        // instruction of the block; here it is only read back.
        let diverges = diverge_at[last].clone();
        let mut succs = Vec::new();
        let conds = if ctl[last] == Ctl::CondBranch { branch_conditions(arch, &raw[last].mnemonic, &raw[last].operands) } else { None };
        let mut push = |to: usize, kind: EdgeKind| {
            let label = match (&conds, kind) {
                (Some((t, _)), EdgeKind::Taken) => Some(t.clone()),
                (Some((_, f)), EdgeKind::Fallthrough) => Some(f.clone()),
                _ => None,
            };
            succs.push(Edge { to, kind, projected: Status::Unknown, label, measured: None })
        };
        match ctl[last] {
            Ctl::Jump => {
                if let Some(t) = tgt[last] {
                    if let Ok(k) = raw.binary_search_by_key(&t, |x| x.addr) {
                        push(block_of[k], EdgeKind::Jump);
                    }
                }
            }
            Ctl::CondBranch => {
                if let Some(t) = tgt[last] {
                    if let Ok(k) = raw.binary_search_by_key(&t, |x| x.addr) {
                        push(block_of[k], EdgeKind::Taken);
                    }
                }
                if e < n {
                    push(block_of[e], EdgeKind::Fallthrough);
                }
            }
            Ctl::IndirectBranch => {
                // A jump table's indirect branch gets an edge to each case the
                // table names (#171); an unresolved one has an empty list.
                for &k in &indirect[last] {
                    push(block_of[k], EdgeKind::Indirect);
                }
                if e < n && is_predicated(arch, &raw[last].mnemonic) {
                    push(block_of[e], EdgeKind::Fallthrough);
                }
            }
            // A tail call does not return here, so -- like a return -- its block
            // has no in-function successor (#271). A predicated ARM return
            // (`bxeq lr`) still falls through when its condition is false.
            Ctl::Return => {
                if e < n && is_predicated(arch, &raw[last].mnemonic) {
                    push(block_of[e], EdgeKind::Fallthrough);
                }
            }
            Ctl::TailCall => {}
            _ => {
                if e < n && diverges.is_none() {
                    push(block_of[e], EdgeKind::Fallthrough);
                }
            }
        }
        // In first-seen order, de-duplicated by a set: a block of k calls
        // each with its own pad has k of them (#311).
        let mut seen = std::collections::HashSet::new();
        let pads: Vec<usize> = unwind[s..e].iter().flatten().map(|&k| block_of[k]).filter(|b| seen.insert(*b)).collect();
        for p in pads {
            push(p, EdgeKind::Exception);
        }
        let kind = if b == 0 {
            BlockKind::Entry
        } else if succs.is_empty() {
            BlockKind::Exit
        } else {
            BlockKind::Normal
        };
        blocks.push(Block { id: b, address: raw[s].addr, kind, instrs, succs, lines, projected, measured: None, note: None, inferred_lines: Vec::new(), diverges });
    }

    project_edges(&mut blocks);
    FnCfg { blocks, cond_branches }
}

/// Edge projection: both ends executed -> covered; either end uncovered ->
/// uncovered; otherwise unknown.
pub fn project_edges(blocks: &mut [Block]) {
    let statuses: Vec<Status> = blocks.iter().map(|b| b.projected).collect();
    for b in blocks.iter_mut() {
        let from = b.projected;
        for e in &mut b.succs {
            let to = statuses[e.to];
            e.projected = match (from, to) {
                // A landing pad's code borrows lines that also run on the
                // normal path (a scope's closing brace, a destructor call),
                // so both ends having run says nothing about whether an
                // exception ever took this edge (#165). Only the negative
                // is sound: a pad that never ran was never landed on.
                (Status::Covered, Status::Covered) if e.kind == EdgeKind::Exception => Status::Unknown,
                (Status::Covered, Status::Covered) => Status::Covered,
                (Status::Uncovered, _) | (_, Status::Uncovered) => Status::Uncovered,
                _ => Status::Unknown,
            };
        }
    }
}

pub fn summarise(it: impl Iterator<Item = Status>) -> Status {
    let mut any_cov = false;
    let mut any_unc = false;
    for s in it {
        match s {
            Status::Covered => any_cov = true,
            Status::Uncovered => any_unc = true,
            Status::Unknown => {}
        }
    }
    if any_cov {
        Status::Covered
    } else if any_unc {
        Status::Uncovered
    } else {
        Status::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions() {
        assert_eq!(branch_conditions("aarch64", "cbz", "w8, 0x1000"), Some(("w8 == 0".into(), "w8 != 0".into())));
        assert_eq!(branch_conditions("aarch64", "b.ne", "0x1000"), Some(("ne".into(), "eq".into())));
        assert_eq!(branch_conditions("x86_64", "jle", "0x1000"), Some(("<= (signed)".into(), "> (signed)".into())));
        assert_eq!(branch_conditions("riscv64", "bnez", "a0, 0x1000"), Some(("a0 != 0".into(), "a0 == 0".into())));
    }

    #[test]
    fn targets() {
        assert_eq!(target("0x100000514 <_mode_permits_engage>"), Some(0x100000514));
        assert_eq!(target("w0, #0x0, 0x100000520 <_f+0x20>"), Some(0x100000520));
        assert_eq!(target("*%rax"), None);
        assert_eq!(target_symbol("0x10000070c <_printf+0x10000070c>").as_deref(), Some("_printf"));
    }

    /// `bl <diverging>; <next>` — the instruction after the call is reached
    /// by nothing, so the block must end there with no successor, and the
    /// block must say on whose authority (#210).
    ///
    /// Fixture-free on purpose: the rule is a property of the builder, and a
    /// suite that needs a built example to run is a suite that reports
    /// success having run nothing when the example is absent (#227).
    #[test]
    fn a_call_to_a_diverging_callee_gets_no_fall_through_edge() {
        let ri = |addr: u64, m: &str, ops: &str| RawInstr { addr, mnemonic: m.into(), operands: ops.into() };
        // b.hs +0x8 ; bl 0x9000 <abort> ; nop ; ret
        let raw = [ri(0x1000, "b.hs", "0x1008"), ri(0x1004, "bl", "0x9000 <abort>"), ri(0x1008, "nop", ""), ri(0x100c, "ret", "")];
        let line = |_: usize| (Some(7u32), Some(0usize), true);
        let cov = |_: usize| Status::Covered;

        // With nothing known about the callee, the call falls through: this
        // is what the builder did for every call before #210.
        let unknown = build("aarch64", 0x1000, 0x1010, &raw, &line, &cov, &|_| None, &|_| Vec::new(), &|_| Vec::new());
        let panic_block = unknown.blocks.iter().find(|b| b.instrs.iter().any(|i| i.mnemonic == "bl")).expect("the call is in some block");
        assert_eq!(panic_block.succs.len(), 1, "with no divergence rule the call still falls through");
        assert_eq!(panic_block.succs[0].kind, EdgeKind::Fallthrough);
        assert!(panic_block.diverges.is_none());

        // Told the callee does not return, the same block ends there.
        let known =
            build("aarch64", 0x1000, 0x1010, &raw, &line, &cov, &|t| (t == 0x9000).then(|| (DivergeRule::DwarfNoreturn, "abort".to_string())), &|_| Vec::new(), &|_| Vec::new());
        let ended = known.blocks.iter().find(|b| b.instrs.iter().any(|i| i.mnemonic == "bl")).expect("the call is in some block");
        assert!(ended.succs.is_empty(), "a call that does not return has no fall-through successor: {:?}", ended.succs);
        assert_eq!(ended.kind, BlockKind::Exit, "with no successor the block leaves the function");
        let why = ended.diverges.as_deref().expect("the block must record why it has no successor");
        assert!(why.contains("abort") && why.contains("DW_AT_noreturn") && why.contains("0x1004"), "the evidence must name the rule, the callee and the call site: {why}");

        // The rule is asked about the call's target, not about the block's
        // own address or the next instruction: a different callee is not
        // covered by what we know about this one.
        let other =
            build("aarch64", 0x1000, 0x1010, &raw, &line, &cov, &|t| (t == 0x1008).then(|| (DivergeRule::DwarfNoreturn, "not-the-callee".to_string())), &|_| Vec::new(), &|_| {
                Vec::new()
            });
        assert_eq!(other.blocks.iter().filter(|b| b.diverges.is_some()).count(), 0, "divergence is keyed on the callee's address");
    }

    /// Only a *direct* call diverges on this evidence. An indirect call's
    /// callee set is unknown by definition, and a conditional branch keeps
    /// both of its outcomes whatever lies at the target.
    #[test]
    fn divergence_is_not_read_off_anything_but_a_direct_call() {
        let ri = |addr: u64, m: &str, ops: &str| RawInstr { addr, mnemonic: m.into(), operands: ops.into() };
        let line = |_: usize| (Some(7u32), Some(0usize), true);
        let cov = |_: usize| Status::Covered;
        let always = |_: u64| Some((DivergeRule::DwarfNoreturn, "abort".to_string()));

        // blr x8 (indirect): the callee is not named, so nothing is known.
        let indirect = [ri(0x1000, "b.hs", "0x1008"), ri(0x1004, "blr", "x8"), ri(0x1008, "nop", ""), ri(0x100c, "ret", "")];
        let c = build("aarch64", 0x1000, 0x1010, &indirect, &line, &cov, &always, &|_| Vec::new(), &|_| Vec::new());
        let b = c.blocks.iter().find(|b| b.instrs.iter().any(|i| i.mnemonic == "blr")).unwrap();
        assert_eq!(b.succs.len(), 1, "an indirect call keeps its fall-through: its callee set is unknown");
        assert!(b.diverges.is_none());

        // A conditional branch keeps both outcomes.
        let cond = [ri(0x1000, "cbz", "w0, 0x1008"), ri(0x1004, "nop", ""), ri(0x1008, "ret", "")];
        let c = build("aarch64", 0x1000, 0x100c, &cond, &line, &cov, &always, &|_| Vec::new(), &|_| Vec::new());
        assert_eq!(c.blocks[0].succs.len(), 2, "both outcomes of a branch survive: {:?}", c.blocks[0].succs);
        assert!(c.blocks[0].diverges.is_none());
    }

    /// A predicated call is not an unconditional transfer. On 32-bit ARM
    /// `bleq` calls only when Z is set; when the condition is false the call
    /// is simply not executed and the next instruction runs. That path is
    /// reachable whatever the callee does with the flow it is given, so a
    /// `DW_AT_noreturn` callee must not remove it.
    ///
    /// Fixture-free: `classify` maps a 32-bit ARM object's `arch` string
    /// (`Architecture::Arm`, lowercased in `objectcode.rs`), so this is the
    /// same path any ARM image takes.
    #[test]
    fn a_predicated_arm_call_keeps_its_fall_through() {
        let ri = |addr: u64, m: &str, ops: &str| RawInstr { addr, mnemonic: m.into(), operands: ops.into() };
        let line = |_: usize| (Some(7u32), Some(0usize), true);
        let cov = |_: usize| Status::Covered;
        let always = |_: u64| Some((DivergeRule::DwarfNoreturn, "abort".to_string()));
        // The `bhs` makes 0x1008 a leader, so the call is the last
        // instruction of its block -- which is where divergence is read.
        let call_at = |m: &str, ops: &str| [ri(0x1000, "bhs", "0x1008"), ri(0x1004, m, ops), ri(0x1008, "nop", ""), ri(0x100c, "bx", "lr")];
        let block_with = |c: &FnCfg, m: &str| {
            c.blocks.iter().find(|b| b.instrs.iter().any(|i| i.mnemonic == m)).map(|b| (b.succs.len(), b.diverges.is_some())).expect("the call is in some block")
        };

        for (m, ops) in [("bleq", "0x9000 <abort>"), ("blne", "0x9000 <abort>"), ("blxeq", "0x9000 <abort>")] {
            let raw = call_at(m, ops);
            let c = build("arm", 0x1000, 0x1010, &raw, &line, &cov, &always, &|_| Vec::new(), &|_| Vec::new());
            assert_eq!(classify("arm", m, ops), Ctl::Call, "{m} is classified as a direct call, which is how it reaches the divergence rule");
            assert_eq!(
                block_with(&c, m),
                (1, false),
                "`{m}` runs only when its condition holds; the path where it does not is reachable however the callee behaves, so the fall-through must survive and no divergence may be recorded"
            );
        }

        // The unpredicated forms still diverge: the fix is about the
        // predicate, not about switching the rule off for ARM.
        for (m, ops) in [("bl", "0x9000 <abort>"), ("blx", "0x9000 <abort>"), ("blal", "0x9000 <abort>")] {
            let raw = call_at(m, ops);
            let c = build("arm", 0x1000, 0x1010, &raw, &line, &cov, &always, &|_| Vec::new(), &|_| Vec::new());
            assert_eq!(block_with(&c, m), (0, true), "`{m}` is executed unconditionally, so a callee that cannot return ends the block");
        }
    }

    /// Divergence is a property of the call, not of where a block happened
    /// to end. If nothing branches to the instruction after a non-returning
    /// call, that instruction used to stay inside the same block and the
    /// rule never saw the call at all -- the block went on to take its
    /// successors from a later terminator, modelling flow that continues
    /// past a call which cannot return.
    ///
    /// Deliberately without an incoming branch to the instruction after the
    /// call, so this cannot pass for the reason
    /// `a_call_to_a_diverging_callee_gets_no_fall_through_edge` does (#227).
    #[test]
    fn a_diverging_call_ends_its_block_with_nothing_branching_after_it() {
        let ri = |addr: u64, m: &str, ops: &str| RawInstr { addr, mnemonic: m.into(), operands: ops.into() };
        let line = |_: usize| (Some(7u32), Some(0usize), true);
        let cov = |_: usize| Status::Covered;
        // bl <abort> ; nop ; b 0x1000. Nothing targets the `nop`: the only
        // branch in the function goes back to the entry.
        let raw = [ri(0x1000, "bl", "0x9000 <abort>"), ri(0x1004, "nop", ""), ri(0x1008, "b", "0x1000")];
        let c =
            build("aarch64", 0x1000, 0x100c, &raw, &line, &cov, &|t| (t == 0x9000).then(|| (DivergeRule::DwarfNoreturn, "abort".to_string())), &|_| Vec::new(), &|_| Vec::new());

        let call_block = c.blocks.iter().find(|b| b.instrs.iter().any(|i| i.mnemonic == "bl")).expect("the call is in some block");
        let addrs: Vec<String> = call_block.instrs.iter().map(|i| format!("{:#x}", i.addr)).collect();
        assert_eq!(addrs, ["0x1000"], "the call must end its block even though nothing branches to the instruction after it");
        assert!(call_block.succs.is_empty(), "a call that does not return has no successor, whatever follows it in the listing: {:?}", call_block.succs);
        assert_eq!(call_block.kind, BlockKind::Entry, "it is still the function's entry block");
        assert!(call_block.diverges.is_some(), "the block must record why it stops here");

        // The instructions after it are their own block -- unreachable, but
        // modelled honestly rather than folded into the call's block.
        let after = c.blocks.iter().find(|b| b.address == 0x1004).expect("the instructions after the call form their own block");
        assert_eq!(after.instrs.len(), 2);
    }

    #[test]
    fn a_got_tail_call_is_a_terminating_call() {
        let ri = |addr: u64, m: &str, ops: &str| RawInstr { addr, mnemonic: m.into(), operands: ops.into() };
        let line = |_: usize| (Some(7u32), Some(0usize), true);
        let cov = |_: usize| Status::Covered;
        // setup, then `br x1 <ext@GOT>` -- a tail call to ext -- then padding
        // nothing branches to.
        let raw = [ri(0x1000, "stp", "x29, x30, [sp, #-16]!"), ri(0x1004, "br", "x1 <ext@GOT>"), ri(0x1008, "nop", "")];
        let c = build("aarch64", 0x1000, 0x100c, &raw, &line, &cov, &|_: u64| None, &|_| Vec::new(), &|_| Vec::new());

        // It carries the Call flag, so the call graph, gaps and stack all see it.
        let tail = c.blocks.iter().flat_map(|b| &b.instrs).find(|i| i.mnemonic == "br").expect("the tail call is present");
        assert!(tail.flags.contains(&InstrFlag::Call), "a GOT tail call is a call for every consumer: {:?}", tail.flags);
        assert!(!tail.flags.contains(&InstrFlag::Branch) && !tail.flags.contains(&InstrFlag::IndirectBranch), "it is not left as a plain/indirect branch: {:?}", tail.flags);

        // But it terminates the block: control does not return here, so there
        // is no in-function successor and no fall-through to the padding.
        let blk = c.blocks.iter().find(|b| b.instrs.iter().any(|i| i.mnemonic == "br")).unwrap();
        assert!(blk.succs.is_empty(), "a tail call has no in-function successor: {:?}", blk.succs);
        assert!(c.blocks.iter().any(|b| b.address == 0x1008), "the padding after it is its own (unreachable) block, not folded in");
    }

    #[test]
    fn aarch64_classes() {
        assert_eq!(classify("aarch64", "b.ne", ""), Ctl::CondBranch);
        assert_eq!(classify("aarch64", "bl", ""), Ctl::Call);
        assert_eq!(classify("aarch64", "ret", ""), Ctl::Return);
        assert_eq!(classify("aarch64", "csel", ""), Ctl::None);
        assert_eq!(classify("x86_64", "jne", ""), Ctl::CondBranch);
        assert_eq!(classify("x86_64", "jmp", "*%rax"), Ctl::IndirectBranch);
        assert_eq!(classify("arm", "bne", ""), Ctl::CondBranch);
        assert_eq!(classify("arm", "bx", "lr"), Ctl::Return);
    }

    /// A switch table's targets, when the caller recovered them, are the
    /// indirect branch's successors: each starts a block and gets an
    /// `Indirect` edge, once however many cases share it (#171).
    #[test]
    fn a_recovered_switch_table_gives_the_indirect_branch_its_successors() {
        let ri = |addr: u64, m: &str, ops: &str| RawInstr { addr, mnemonic: m.into(), operands: ops.into() };
        let raw = vec![ri(0x1000, "br", "x8"), ri(0x1004, "mov", "w0, #1"), ri(0x1008, "ret", ""), ri(0x100c, "mov", "w0, #2"), ri(0x1010, "ret", "")];
        let line = |_: usize| (Some(1), Some(0), true);
        let cov = |_: usize| Status::Covered;
        let c = build("aarch64", 0x1000, 0x1014, &raw, &line, &cov, &|_| None, &|a| if a == 0x1000 { vec![0x1004, 0x100c, 0x1004] } else { Vec::new() }, &|_| Vec::new());
        let succ: Vec<(usize, EdgeKind)> = c.blocks[0].succs.iter().map(|e| (e.to, e.kind)).collect();
        assert_eq!(succ, vec![(1, EdgeKind::Indirect), (2, EdgeKind::Indirect)], "{:?}", c.blocks);

        // unrecovered, the branch keeps no successors at all
        let c = build("aarch64", 0x1000, 0x1014, &raw, &line, &cov, &|_| None, &|_| Vec::new(), &|_| Vec::new());
        assert!(c.blocks[0].succs.is_empty());
    }

    /// A call the exception table covers gets an `Exception` edge from its
    /// block to the landing pad, which starts a block of its own (#165).
    #[test]
    fn a_covered_call_site_gets_an_edge_to_its_landing_pad() {
        let ri = |addr: u64, m: &str, ops: &str| RawInstr { addr, mnemonic: m.into(), operands: ops.into() };
        let raw = vec![ri(0x1000, "bl", "0x9000 <_may_throw>"), ri(0x1004, "ret", ""), ri(0x1008, "bl", "0x9100 <___cxa_begin_catch>"), ri(0x100c, "ret", "")];
        let line = |_: usize| (Some(1), Some(0), true);
        let cov = |_: usize| Status::Covered;
        let c = build("aarch64", 0x1000, 0x1010, &raw, &line, &cov, &|_| None, &|_| Vec::new(), &|a| if a == 0x1000 { vec![0x1008] } else { Vec::new() });
        let pad = c.blocks.iter().position(|b| b.address == 0x1008).expect("the landing pad starts a block");
        assert!(c.blocks[0].succs.iter().any(|e| e.to == pad && e.kind == EdgeKind::Exception), "{:?}", c.blocks[0].succs);
        assert_ne!(c.blocks[0].succs.iter().find(|e| e.kind == EdgeKind::Exception).unwrap().projected, Status::Covered, "both ends ran, and that still says nothing");
    }

    /// A branch out of the function is a tail call: the call graph and the
    /// helper-call check must see its callee, and the block ends there (#264).
    #[test]
    fn a_branch_out_of_the_function_is_a_tail_call() {
        let ri = |addr: u64, m: &str, ops: &str| RawInstr { addr, mnemonic: m.into(), operands: ops.into() };
        let raw = vec![ri(0x1000, "cbz", "w0, 0x100c"), ri(0x1004, "mov", "x0, x1"), ri(0x1008, "b", "0x9000 <_puts>"), ri(0x100c, "b", "0x1004"), ri(0x1010, "ret", "")];
        let line = |_: usize| (Some(1), Some(0), true);
        let cov = |_: usize| Status::Covered;
        let c = build("aarch64", 0x1000, 0x1014, &raw, &line, &cov, &|_| None, &|_| Vec::new(), &|_| Vec::new());
        let tail = c.blocks.iter().flat_map(|b| &b.instrs).find(|i| i.addr == 0x1008).unwrap();
        assert!(tail.flags.contains(&InstrFlag::Call), "the branch to puts is a call: {:?}", tail.flags);
        let blk = c.blocks.iter().find(|b| b.instrs.iter().any(|i| i.addr == 0x1008)).unwrap();
        assert!(blk.succs.is_empty(), "and nothing in this function follows it");
        // a branch inside the function is not
        let inner = c.blocks.iter().flat_map(|b| &b.instrs).find(|i| i.addr == 0x100c).unwrap();
        assert!(!inner.flags.contains(&InstrFlag::Call));
    }

    /// Prefixed x86-64 (Intel CET `notrack`, `rep ret`, `bnd`), 32-bit `i386`
    /// (`calll`/`jmpl`/`retl`), RISC-V (`tail`, `jal zero`, Zbs `b*`), and
    /// predicated ARM `bx<cc>` must be classified accurately (#9.4).
    #[test]
    fn prefixed_x86_i386_riscv_and_predicated_arm_classification() {
        // Intel CET / GCC x86-64 prefixes
        assert_eq!(classify("x86_64", "notrack", "callq\t*%rax"), Ctl::IndirectCall);
        assert_eq!(classify("x86_64", "notrack", "jmpq\t*%rax"), Ctl::IndirectBranch);
        assert_eq!(classify("x86_64", "notrack", "callq\t*0x10(%rip) <ext@GOT>"), Ctl::Call);
        assert_eq!(classify("x86_64", "notrack", "jmpq\t*0x10(%rip) <ext@GOT>"), Ctl::TailCall);
        assert_eq!(classify("x86_64", "rep", "retq"), Ctl::Return);
        assert_eq!(classify("x86_64", "repz", "retq"), Ctl::Return);
        assert_eq!(classify("x86_64", "bnd", "jmpq\t0x1000 <f>"), Ctl::Jump);
        assert_eq!(classify("x86_64", "bnd", "callq\t*%rcx"), Ctl::IndirectCall);
        assert_eq!(classify("x86_64", "bnd", "retq"), Ctl::Return);
        // 32-bit i386
        assert_eq!(classify("i386", "calll", "0x1000 <f>"), Ctl::Call);
        assert_eq!(classify("i386", "calll", "*%eax"), Ctl::IndirectCall);
        assert_eq!(classify("i386", "jmpl", "0x1000 <f>"), Ctl::Jump);
        assert_eq!(classify("i386", "jmpl", "*%eax"), Ctl::IndirectBranch);
        assert_eq!(classify("i386", "retl", ""), Ctl::Return);
        // RISC-V: tail is a direct tail call, jal zero is a jump, Zbs b* are not branches
        assert_eq!(classify("riscv64", "tail", "0x1000 <callee>"), Ctl::TailCall);
        assert_eq!(classify("riscv64", "jal", "zero, 0x1000"), Ctl::Jump);
        assert_eq!(classify("riscv64", "jal", "x0, 0x1000"), Ctl::Jump);
        assert_eq!(classify("riscv64", "jal", "ra, 0x1000"), Ctl::Call);
        assert_eq!(classify("riscv64", "jalr", "zero, 0(a0)"), Ctl::IndirectBranch);
        assert_eq!(classify("riscv64", "jalr", "ra, 0(a0)"), Ctl::IndirectCall);
        for zbs in ["bset", "bseti", "bclr", "bclri", "bext", "bexti", "binv", "binvi"] {
            assert_eq!(classify("riscv64", zbs, "a0, a1, a2"), Ctl::None, "{zbs} is arithmetic, not a branch");
        }
        assert_eq!(classify("riscv64", "bne", "a0, a1, 0x1000"), Ctl::CondBranch);
        assert_eq!(classify("riscv64", "c.bnez", "a0, 0x1000"), Ctl::CondBranch);
        // ARM predicated return keeps its fall-through edge
        let ri = |addr: u64, m: &str, ops: &str| RawInstr { addr, mnemonic: m.into(), operands: ops.into() };
        let raw = vec![ri(0x1000, "bxeq", "lr"), ri(0x1004, "mov", "r0, #1"), ri(0x1008, "bx", "lr")];
        let c = build("arm", 0x1000, 0x100c, &raw, &|_| (Some(1), Some(0), true), &|_| Status::Covered, &|_| None, &|_| Vec::new(), &|_| Vec::new());
        assert_eq!(c.blocks.len(), 2);
        assert_eq!(c.blocks[0].succs.len(), 1, "predicated bxeq lr must fall through when eq is false");
        assert_eq!(c.blocks[0].succs[0].kind, EdgeKind::Fallthrough);
        assert_eq!(c.blocks[0].succs[0].to, 1);
        assert!(c.blocks[1].succs.is_empty(), "unpredicated bx lr has no fall-through");
    }
}
