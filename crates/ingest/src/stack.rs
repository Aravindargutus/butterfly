// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Static stack analysis (DO-178C 6.3.4.f "accuracy and consistency": stack
//! usage).
//!
//! Frame sizes come, in order of preference, from a `.stack_sizes` section,
//! from `.su` files, or from the function prologue in the disassembly. The
//! worst case per symbol is own frame + deepest direct-call chain; recursion
//! and indirect calls are reported, never silently bounded.

use crate::objectcode::RawInstr;
use butterfly_model::{Frame, FrameSource, StackResult};
use std::collections::HashMap;

/// Estimate the frame from the prologue: sum of stack-pointer decrements
/// before the first control transfer. Pushes count their register width.
///
/// Fails closed (`FrameSource::Unknown`) if any stack-pointer decrement occurs
/// *after* the prologue cutoff (e.g. a stack probe call such as
/// `___chkstk_darwin` or `__rust_probestack`, an inline `-fstack-clash-protection`
/// probe loop, or a prologue longer than 24 instructions) so a large frame is
/// never understated as a small bounded one (#9.5).
pub fn frame_from_prologue(arch: &str, instrs: &[RawInstr]) -> Frame {
    // `None` once the sum overflows or an unmeasured post-prologue allocation
    // is detected: fail closed as Unknown rather than understating the frame.
    let mut bytes: Option<u64> = Some(0);
    let add = |b: Option<u64>, v: u64| b.and_then(|b| b.checked_add(v));
    let mut dynamic = false;
    let mut seen_any = false;
    let mut stopped_at = instrs.len();
    for (n, i) in instrs.iter().enumerate() {
        if n > 24 {
            stopped_at = n;
            break;
        }
        let m = i.mnemonic.to_ascii_lowercase();
        let ops = i.operands.replace(' ', "").to_ascii_lowercase();
        if arch.starts_with("aarch64") {
            match m.as_str() {
                "b" | "bl" | "br" | "blr" | "blraa" | "blrab" | "blraaz" | "blrabz" | "braa" | "brab" | "braaz" | "brabz" | "ret" | "retaa" | "retab" | "eret" | "cbz" | "cbnz"
                | "tbz" | "tbnz" => {
                    stopped_at = n + 1;
                    break;
                }
                _ if m.starts_with("b.") => {
                    stopped_at = n + 1;
                    break;
                }
                "sub" if ops.starts_with("sp,sp,") => {
                    let rest = &ops[6..];
                    match parse_a64_imm(rest) {
                        Some(v) => {
                            bytes = add(bytes, v);
                            seen_any = true;
                        }
                        None => {
                            dynamic = true;
                        }
                    }
                }
                "stp" | "str" | "stur" => {
                    // pre-indexed push: [sp, #-N]!
                    if let Some(p) = ops.find("[sp,#-") {
                        if ops.ends_with("]!") {
                            if let Some(v) = parse_imm(&ops[p + 6..ops.len() - 2]) {
                                bytes = add(bytes, v);
                                seen_any = true;
                            }
                        }
                    }
                }
                "add" if ops.starts_with("sp,sp,") => {
                    // epilogue reached (leaf with no frame) - stop
                    stopped_at = n + 1;
                    break;
                }
                _ => {}
            }
        } else if arch.starts_with("x86") {
            let (um, uops) = crate::cfg::unwrap_x86_prefix(&m, &i.operands);
            let um = um.to_ascii_lowercase();
            let uops = uops.replace(' ', "").to_ascii_lowercase();
            match um.as_str() {
                "push" | "pushq" => {
                    bytes = add(bytes, 8);
                    seen_any = true;
                }
                "pushl" => {
                    bytes = add(bytes, 4);
                    seen_any = true;
                }
                "sub" | "subq" | "subl" if uops.ends_with(",%rsp") || uops.ends_with(",%esp") => {
                    let imm = uops.trim_start_matches('$');
                    let imm = imm.split(',').next().unwrap_or("");
                    match parse_imm(imm) {
                        Some(v) => {
                            bytes = add(bytes, v);
                            seen_any = true;
                        }
                        None => dynamic = true,
                    }
                }
                "add" | "addq" | "addl" if uops.ends_with(",%rsp") || uops.ends_with(",%esp") => {
                    let first = uops.split(',').next().unwrap_or("");
                    if let Some(neg) = first.strip_prefix("$-") {
                        // `addq $-0x80, %rsp`: shorter encoding for `subq $0x80, %rsp`
                        match parse_imm(neg) {
                            Some(v) => {
                                bytes = add(bytes, v);
                                seen_any = true;
                            }
                            None => dynamic = true,
                        }
                    } else if first.starts_with('$') {
                        // epilogue restore (`addq $N, %rsp`)
                        stopped_at = n + 1;
                        break;
                    } else {
                        dynamic = true;
                    }
                }
                "enter" => {
                    if let Some(v) = uops.trim_start_matches('$').split(',').next().and_then(parse_imm) {
                        bytes = add(add(bytes, v), 8);
                        seen_any = true;
                    }
                }
                "call" | "callq" | "calll" | "jmp" | "jmpq" | "jmpl" | "ret" | "retq" | "retl" => {
                    stopped_at = n + 1;
                    break;
                }
                _ if um.starts_with('j') => {
                    stopped_at = n + 1;
                    break;
                }
                _ => {}
            }
        } else if arch.starts_with("arm") {
            match m.as_str() {
                "push" | "push.w" => {
                    let regs = count_reglist(&ops);
                    bytes = add(bytes, 4 * regs as u64);
                    seen_any = true;
                }
                "stmdb" | "stmfd" if ops.starts_with("sp!,") => {
                    let regs = count_reglist(&ops);
                    bytes = add(bytes, 4 * regs as u64);
                    seen_any = true;
                }
                "vpush" => {
                    bytes = add(bytes, 8 * count_reglist(&ops) as u64);
                    seen_any = true;
                }
                "sub" | "sub.w" | "subs" if ops.starts_with("sp,") => {
                    let imm = ops.rsplit(',').next().unwrap_or("");
                    match parse_imm(imm) {
                        Some(v) => {
                            bytes = add(bytes, v);
                            seen_any = true;
                        }
                        None => dynamic = true,
                    }
                }
                "b" | "bl" | "bx" | "blx" | "pop" => {
                    stopped_at = n + 1;
                    break;
                }
                _ if m.starts_with('b') => {
                    stopped_at = n + 1;
                    break;
                }
                _ => {}
            }
        } else if arch.starts_with("riscv") {
            match m.as_str() {
                "addi" | "c.addi" | "c.addi16sp" if ops.starts_with("sp,sp,-") || ops.starts_with("sp,-") => {
                    let imm = ops.rsplit(',').next().unwrap_or("").trim_start_matches('-');
                    if let Some(v) = parse_imm(imm) {
                        bytes = add(bytes, v);
                        seen_any = true;
                    }
                }
                "sub" if ops.starts_with("sp,sp,") => dynamic = true,
                "jal" | "jalr" | "j" | "ret" | "call" | "tail" => {
                    stopped_at = n + 1;
                    break;
                }
                _ if m.starts_with('b') => {
                    stopped_at = n + 1;
                    break;
                }
                _ => {}
            }
        } else {
            return Frame { bytes: 0, source: FrameSource::Unknown, dynamic: false };
        }
    }
    let _ = seen_any;
    // If any stack-pointer decrement appears *after* the prologue cutoff
    // (post-probe allocation, inline clash-protection loop, or a prologue past
    // 24 instructions), the prologue sum is incomplete: fail closed as Unknown.
    if stopped_at < instrs.len() && has_sp_decrement(arch, &instrs[stopped_at..]) {
        bytes = None;
    }
    match bytes {
        Some(bytes) => Frame { bytes, source: FrameSource::Prologue, dynamic },
        None => Frame { bytes: 0, source: FrameSource::Unknown, dynamic },
    }
}

/// Parse an AArch64 immediate operand, including shifted forms like `#0x1,lsl#12`.
fn parse_a64_imm(rest: &str) -> Option<u64> {
    if let Some((imm_s, shift_s)) = rest.split_once(",lsl") {
        let base = parse_imm(imm_s)?;
        let shift = u32::try_from(parse_imm(shift_s)?).ok()?;
        if shift >= 64 {
            return None;
        }
        return base.checked_mul(1u64 << shift);
    }
    parse_imm(rest)
}

/// Whether `instrs` contains any stack-pointer decrement (constant or
/// otherwise) that a prefix prologue scan missed (#9.5).
fn has_sp_decrement(arch: &str, instrs: &[RawInstr]) -> bool {
    instrs.iter().any(|i| {
        let m = i.mnemonic.to_ascii_lowercase();
        let ops = i.operands.replace(' ', "").to_ascii_lowercase();
        if arch.starts_with("aarch64") {
            (m == "sub" && ops.starts_with("sp,sp,")) || (matches!(m.as_str(), "stp" | "str" | "stur") && ops.contains("[sp,#-") && ops.ends_with("]!"))
        } else if arch.starts_with("x86") {
            let (um, uops) = crate::cfg::unwrap_x86_prefix(&m, &i.operands);
            let um = um.to_ascii_lowercase();
            let uops = uops.replace(' ', "").to_ascii_lowercase();
            let dst_sp = uops.ends_with(",%rsp") || uops.ends_with(",%esp");
            (matches!(um.as_str(), "sub" | "subq" | "subl") && dst_sp)
                || (matches!(um.as_str(), "add" | "addq" | "addl") && dst_sp && uops.split(',').next().is_some_and(|s| s.starts_with("$-")))
                || um == "enter"
        } else if arch.starts_with("arm") {
            matches!(m.as_str(), "sub" | "sub.w" | "subs") && ops.starts_with("sp,")
        } else if arch.starts_with("riscv") {
            (matches!(m.as_str(), "addi" | "c.addi" | "c.addi16sp") && (ops.starts_with("sp,sp,-") || ops.starts_with("sp,-"))) || (m == "sub" && ops.starts_with("sp,sp,"))
        } else {
            false
        }
    })
}

/// Whether anywhere in the function the stack pointer is set to a value only
/// known at run time: what a variable-length array or an `alloca` compiles to
/// (#168). The prologue scan cannot see these -- the allocation happens in the
/// body, after the static frame is set up -- and a `.stack_sizes` section
/// leaves such functions out entirely, so without a `.su` the function would
/// read as bounded by its static frame alone.
///
/// Recognised: the stack pointer written from a register, or offset by one,
/// other than the frame pointer (restoring `sp` from the frame pointer is an
/// epilogue, and constant adjustments and realignments are static). A pattern
/// this does not recognise is not dynamic, so a false positive is the
/// fail-closed direction: a bounded function reported not bounded.
pub fn allocates_dynamically(arch: &str, instrs: &[RawInstr]) -> bool {
    instrs.iter().any(|i| {
        let m = i.mnemonic.to_ascii_lowercase();
        let ops = i.operands.replace(' ', "").to_ascii_lowercase();
        let parts: Vec<&str> = ops.split(',').collect();
        if arch.starts_with("aarch64") || arch.starts_with("arm64") {
            sp_from_register(&m, &parts, "sp", &["sp", "x29", "fp"], "#")
        } else if arch.starts_with("x86") {
            // AT&T: the destination is last (and xchg writes both operands)
            let Some(dst) = parts.last() else { return false };
            if m.starts_with("xchg") {
                return parts.iter().any(|&p| p == "%rsp" || p == "%esp");
            }
            if *dst != "%rsp" && *dst != "%esp" {
                return false;
            }
            if m.starts_with("pop") {
                return true;
            }
            let src = parts[0];
            if m.starts_with("mov") || m.starts_with("sub") || m.starts_with("add") {
                (src.starts_with('%') && !matches!(src, "%rbp" | "%ebp" | "%rsp" | "%esp")) || src.contains('(')
            } else if m.starts_with("lea") {
                !(src.ends_with("(%rbp)") || src.ends_with("(%ebp)"))
            } else {
                false
            }
        } else if arch.starts_with("arm") || arch.starts_with("thumb") {
            sp_from_register(&m, &parts, "sp", &["sp", "r7", "r11", "fp"], "#")
        } else if arch.starts_with("riscv") {
            if m == "mv" || m == "c.mv" {
                parts.first() == Some(&"sp") && parts.get(1).is_some_and(|s| !matches!(*s, "sp" | "s0" | "fp"))
            } else if m == "sub" || m == "add" {
                parts.first() == Some(&"sp")
            } else {
                false
            }
        } else {
            false
        }
    })
}

/// The shared shape for the destination-first ISAs: `mov sp, rN`, or an
/// add/sub into `sp` whose base is not the stack or frame pointer, or whose
/// offset is a register rather than an immediate.
fn sp_from_register(m: &str, parts: &[&str], sp: &str, bases: &[&str], imm: &str) -> bool {
    if parts.first() != Some(&sp) {
        return false;
    }
    let m = m.trim_end_matches(".w");
    match m {
        "mov" | "movs" => parts.get(1).is_some_and(|s| !bases.contains(s)),
        // Thumb's two-operand `sub sp, #8`
        "sub" | "subs" | "add" | "adds" if parts.len() == 2 => !parts[1].starts_with(imm),
        "sub" | "subs" | "add" | "adds" => {
            let base_ok = parts.get(1).is_some_and(|s| bases.contains(s));
            let offset_imm = parts.get(2).is_none_or(|s| s.starts_with(imm));
            !(base_ok && offset_imm)
        }
        _ => false,
    }
}

/// Whether a symbol's frame must be treated as unknown (fail closed). An
/// unrecognised frame source always is; and a prologue *estimate* is too when
/// the authoritative `.stack_sizes` section was malformed -- the estimate
/// replaced an entry lost when parsing stopped at the first bad one, so it is
/// not an established bound and must not read as `bounded` (#247). A `.su`
/// entry is an independent authoritative source, and a valid `.stack_sizes`
/// entry decoded before the corruption is itself trustworthy, so neither is
/// affected.
pub fn frame_unknown(source: FrameSource, stack_sizes_malformed: bool) -> bool {
    source == FrameSource::Unknown || (stack_sizes_malformed && source == FrameSource::Prologue)
}

fn parse_imm(s: &str) -> Option<u64> {
    let s = s.trim().trim_start_matches('#').trim_start_matches('$');
    if let Some(h) = s.strip_prefix("0x") {
        u64::from_str_radix(h, 16).ok()
    } else if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() {
        s.parse().ok()
    } else {
        None
    }
}

fn count_reglist(ops: &str) -> usize {
    let Some(a) = ops.find('{') else { return 0 };
    let Some(rel_b) = ops[a + 1..].find('}') else { return 0 };
    let mut n = 0;
    for part in ops[a + 1..a + 1 + rel_b].split(',') {
        if let Some((lo, hi)) = part.split_once('-') {
            let num = |r: &str| r.trim_start_matches(|c: char| c.is_alphabetic()).parse::<usize>().ok();
            if let (Some(l), Some(h)) = (num(lo), num(hi)) {
                n += h.saturating_sub(l) + 1;
                continue;
            }
        }
        if !part.is_empty() {
            n += 1;
        }
    }
    n
}

/// `.su` line: `file:line:col:function<TAB>bytes<TAB>static|dynamic[,bounded]`
pub fn parse_su(text: &str) -> Vec<(String, u64, bool)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.split('\t');
        // Require all three fields; a truncated line with no qualifier must
        // not silently default to `static` (#9.5).
        let (Some(loc), Some(bytes), Some(qual)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let qual = qual.trim();
        if qual.is_empty() {
            continue;
        }
        let name = su_function(loc).trim();
        let Ok(b) = bytes.trim().parse::<u64>() else { continue };
        // Fail closed: anything other than exact `static` (including `dynamic`,
        // `dynamic,bounded`, or an unknown qualifier) is marked dynamic.
        out.push((name.to_string(), b, !qual.eq_ignore_ascii_case("static")));
    }
    out
}

/// The function field of a `.su` location: everything after `:line:` and,
/// where there is one, `col:` -- read by the fields, not by splitting on the
/// last `:`. GCC writes a C++ function by its printable name (`int
/// ns::C::m(int)`), whose `::` the last-colon split cut down to `m(int)`
/// (#325). Clang writes `file:line:function`, with no column. The first
/// `:<digits>:` ends the file, so a Windows drive letter (`C:\src\a.c`) or
/// a `:` elsewhere in a path stays in the file; a location with none is
/// all name.
fn su_function(loc: &str) -> &str {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    for (i, _) in loc.match_indices(':') {
        let rest = &loc[i + 1..];
        let n = digits(rest);
        if n == 0 || !rest[n..].starts_with(':') {
            continue;
        }
        let after = &rest[n + 1..];
        let c = digits(after);
        return if c > 0 && after[c..].starts_with(':') { &after[c + 1..] } else { after };
    }
    loc
}

pub struct StackNode {
    pub frame: u64,
    pub dynamic: bool,
    pub callees: Vec<usize>,
    pub indirect_calls: u32,
    /// The frame size could not be determined (`FrameSource::Unknown`: an
    /// unrecognised architecture or prologue, or a symbol with no analysable
    /// object code). A node like this contributes a real, unknown amount to
    /// the stack, so it must fail closed -- reported as not bounded -- rather
    /// than be counted as a bounded zero (#247).
    pub unknown: bool,
}

/// The longest call chain, in functions, that `worst_case` will report as
/// bounded. A symbol whose own longest direct-call chain is longer is reported
/// unbounded, with its real chain length as `depth` and only itself as its
/// path. Real call graphs are orders of magnitude shallower; a bounded-stack
/// argument that needed a chain this long could not be made by hand review
/// anyway, so reporting it as unbounded loses nothing real (#247).
///
/// The verdict is a property of the symbol's own call graph, never of the
/// order symbols are visited in, and it caps what is kept per symbol: no
/// path longer than this is copied or stored.
pub const MAX_DEPTH: usize = 4000;

/// Worst-case stack per node: own frame plus the largest-total direct-call
/// chain, with recursion, indirect calls, dynamic and unknown frames, u64
/// overflow and over-deep chains all failing the result closed.
///
/// The traversal is an explicit-stack depth-first search, so an
/// attacker-supplied call graph of any depth runs on the caller's thread
/// without touching the native stack. Every node's result is memoised the
/// first time it completes -- nothing in it depends on how deep the traversal
/// was when it got there -- so each node and call edge is processed once, and
/// stored paths are capped at `MAX_DEPTH`: O(nodes x MAX_DEPTH + edges) at
/// worst (#247).
pub fn worst_case(nodes: &[StackNode]) -> Vec<StackResult> {
    #[derive(Clone)]
    struct Memo {
        total: Option<u64>,
        path: Vec<usize>,
        bounded: bool,
        reason: Option<String>,
        /// Functions on this node's longest direct-call chain, itself
        /// included, recursion edges excluded.
        height: usize,
    }
    /// A node being visited: its callees and what they have reported so far.
    struct Visit {
        u: usize,
        callees: Vec<usize>,
        next: usize,
        best: Option<Memo>,
        recursion: Option<Vec<usize>>,
        unbounded_reason: Option<String>,
        height: usize,
    }
    impl Visit {
        fn new(u: usize, nodes: &[StackNode]) -> Visit {
            let mut callees = nodes[u].callees.clone();
            callees.sort_unstable();
            callees.dedup();
            Visit { u, callees, next: 0, best: None, recursion: None, unbounded_reason: None, height: 0 }
        }
        /// Fold one callee's result into this node's.
        fn absorb(&mut self, r: Memo) {
            if r.reason.as_deref() == Some("recursion") && r.total.is_none() {
                if self.recursion.is_none() {
                    self.recursion = Some(r.path);
                }
                return;
            }
            self.height = self.height.max(r.height);
            // Uncertainty propagates from EVERY reachable callee, not only
            // the largest-total one chosen for the display path: an
            // unknown-size callee must not hide behind a bounded sibling with
            // a larger finite total (#247).
            if !r.bounded && self.unbounded_reason.is_none() {
                self.unbounded_reason = r.reason.clone();
            }
            let better = match &self.best {
                None => true,
                Some(b) => r.total.unwrap_or(0) > b.total.unwrap_or(0),
            };
            if better {
                self.best = Some(r);
            }
        }
        fn finish(self, me: &StackNode) -> Memo {
            let height = self.height.saturating_add(1);
            let mut path = vec![self.u];
            let mut total = me.frame;
            let mut bounded = !me.dynamic && me.indirect_calls == 0 && !me.unknown;
            let mut reason = None;
            if me.dynamic {
                reason = Some("dynamic frame (alloca / VLA)".into());
            } else if me.indirect_calls > 0 {
                reason = Some(format!("{} indirect call(s): callee set unknown", me.indirect_calls));
            } else if me.unknown {
                reason = Some("frame size could not be determined".into());
            }
            if let Some(b) = &self.best {
                // Checked: a hostile `.stack_sizes` can carry frame sizes near
                // u64::MAX, and an unchecked sum panics in debug and wraps in
                // release -- a wrap turns a huge stack into a small "bounded"
                // one (#247). On overflow, saturate for display and fail closed.
                match total.checked_add(b.total.unwrap_or(0)) {
                    Some(t) => total = t,
                    None => {
                        total = u64::MAX;
                        bounded = false;
                        if reason.is_none() {
                            reason = Some("aggregate stack size overflows u64".into());
                        }
                    }
                }
                // A node's best chain is no longer than its longest, so within
                // the bound this path has at most MAX_DEPTH entries.
                if height <= MAX_DEPTH {
                    path.extend_from_slice(&b.path);
                }
            }
            if height > MAX_DEPTH {
                // Past the bound only the symbol itself is kept: a path cut
                // off at an arbitrary point explains nothing, and storing
                // MAX_DEPTH entries for every symbol above the bound is what
                // made a long chain cost gigabytes.
                bounded = false;
                if reason.is_none() {
                    reason = Some(format!("call chain of {height} functions exceeds the analysable depth of {MAX_DEPTH}; reported as unbounded"));
                }
            }
            if self.unbounded_reason.is_some() {
                bounded = false;
            }
            if !bounded && reason.is_none() {
                // the shown branch's own reason when it is the unbounded one,
                // else whichever callee first reported unbounded
                reason = self.best.as_ref().filter(|b| !b.bounded).and_then(|b| b.reason.clone()).or_else(|| self.unbounded_reason.clone());
            }
            match (self.recursion, self.best.is_none()) {
                (Some(cycle), true) => Memo { total: None, path: cycle, bounded: false, reason: Some("recursion".into()), height },
                (Some(cycle), false) => {
                    Memo { total: Some(total), path, bounded: false, reason: Some(format!("recursion via {} symbol(s); bound excludes the cycle", cycle.len())), height }
                }
                (None, _) => Memo { total: Some(total), path, bounded, reason, height },
            }
        }
    }

    let n = nodes.len();
    let mut memo: Vec<Option<Memo>> = vec![None; n];
    // Where each node sits on the current DFS path while it is on it: an edge
    // back to it is recursion, and the cycle is the path from there on.
    let mut on_path: Vec<Option<usize>> = vec![None; n];
    let mut out = Vec::with_capacity(n);
    for root in 0..n {
        if memo[root].is_none() {
            let mut path: Vec<usize> = vec![root];
            let mut visits: Vec<Visit> = vec![Visit::new(root, nodes)];
            on_path[root] = Some(0);
            while let Some(top) = visits.last_mut() {
                if let Some(&c) = top.callees.get(top.next) {
                    top.next += 1;
                    if let Some(m) = &memo[c] {
                        top.absorb(m.clone());
                    } else if let Some(at) = on_path[c] {
                        let cycle: Vec<usize> = path[at..].iter().take(MAX_DEPTH).copied().collect();
                        top.absorb(Memo { total: None, path: cycle, bounded: false, reason: Some("recursion".into()), height: 0 });
                    } else {
                        on_path[c] = Some(path.len());
                        path.push(c);
                        visits.push(Visit::new(c, nodes));
                    }
                    continue;
                }
                let v = visits.pop().expect("the loop holds a visit");
                let u = v.u;
                let m = v.finish(&nodes[u]);
                path.pop();
                on_path[u] = None;
                if let Some(parent) = visits.last_mut() {
                    parent.absorb(m.clone());
                }
                memo[u] = Some(m);
            }
        }
        let m = memo[root].as_ref().expect("a visited root is memoised");
        // Past the bound the path is only the symbol, so report the real
        // chain length rather than the length of what was kept.
        let depth = if m.height > MAX_DEPTH { m.height } else { m.path.len() };
        out.push(StackResult { worst_case: m.total, depth: u32::try_from(depth).unwrap_or(u32::MAX), path: m.path.clone(), bounded: m.bounded, reason: m.reason.clone() });
    }
    out
}

/// Map `.su` entries onto symbols by (demangled) name.
///
/// When multiple translation units emit a `.su` record under the same bare
/// name (e.g. `static helper()` in two files), keep the conservative worst
/// case (`max(bytes)`, `any_dynamic`) rather than letting the last record
/// overwrite a larger or dynamic frame (#9.5).
pub fn su_lookup(entries: &[(String, u64, bool)]) -> HashMap<String, (u64, bool)> {
    let mut out: HashMap<String, (u64, bool)> = HashMap::new();
    for (n, b, d) in entries {
        out.entry(n.clone())
            .and_modify(|(eb, ed)| {
                *eb = (*eb).max(*b);
                *ed = *ed || *d;
            })
            .or_insert((*b, *d));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ri(m: &str, o: &str) -> RawInstr {
        RawInstr { addr: 0, mnemonic: m.into(), operands: o.into() }
    }

    #[test]
    fn aarch64_prologue() {
        let f = frame_from_prologue("aarch64", &[ri("sub", "sp, sp, #0x40"), ri("stp", "x29, x30, [sp, #0x30]"), ri("add", "x29, sp, #0x30"), ri("bl", "0x1000")]);
        assert_eq!(f.bytes, 0x40);
        let g = frame_from_prologue("aarch64", &[ri("stp", "x29, x30, [sp, #-0x10]!"), ri("mov", "x29, sp"), ri("ret", "")]);
        assert_eq!(g.bytes, 16);
    }

    #[test]
    fn x86_prologue() {
        let f = frame_from_prologue("x86_64", &[ri("push", "%rbp"), ri("mov", "%rsp, %rbp"), ri("sub", "$0x20, %rsp"), ri("call", "0x1")]);
        assert_eq!(f.bytes, 8 + 0x20);
    }

    #[test]
    fn malformed_stack_sizes_fails_a_prologue_estimate_closed() {
        use FrameSource::*;
        // Unknown is always unknown, malformed or not.
        assert!(frame_unknown(Unknown, false));
        assert!(frame_unknown(Unknown, true));
        // A prologue estimate is a real bound normally, but fails closed once
        // the authoritative .stack_sizes was corrupt (#247) -- so it reads as
        // unknown, and worst_case then reports the symbol not bounded.
        assert!(!frame_unknown(Prologue, false), "prologue estimation is fine when nothing was corrupt");
        assert!(frame_unknown(Prologue, true), "a prologue estimate after malformed metadata is not an established bound");
        // Independent authoritative sources are unaffected by the corruption.
        assert!(!frame_unknown(StackUsageFile, true), ".su is an independent source");
        assert!(!frame_unknown(StackSizesSection, true), "an entry decoded before the corruption is valid");
        // And the fail-closed frame really does make the symbol unbounded.
        let nodes = vec![StackNode { frame: 16, dynamic: false, callees: vec![], indirect_calls: 0, unknown: frame_unknown(Prologue, true) }];
        assert!(!worst_case(&nodes)[0].bounded, "the machine-readable result fails closed");
    }

    // #168: a VLA or alloca moves the stack pointer by a run-time amount in the
    // body, after the prologue, so the prologue estimate never sees it. With no
    // `.su` -- and a `.stack_sizes` section leaves such functions out
    // altogether -- the function read as bounded by its static frame.

    #[test]
    fn a_vla_in_the_body_is_dynamic_on_aarch64() {
        // examples/c-constructs vla_sum at -O0, Apple clang 17: the static frame,
        // a stack probe call, then the run-time allocation and its release
        let body = [
            ri("stp", "x29, x30, [sp, #-0x10]!"),
            ri("mov", "x29, sp"),
            ri("sub", "sp, sp, #0x50"),
            ri("mov", "x9, sp"),
            ri("blr", "x16"),
            ri("mov", "x9, sp"),
            ri("subs", "x9, x9, x10"),
            ri("mov", "sp, x9"),
            ri("b.ge", "0x100000640"),
            ri("mov", "sp, x8"),
            ri("mov", "sp, x29"),
            ri("ldp", "x29, x30, [sp], #0x10"),
            ri("ret", ""),
        ];
        assert!(allocates_dynamically("aarch64", &body));
        assert!(!frame_from_prologue("aarch64", &body).dynamic, "the prologue alone does not see it: this is the case the body scan exists for");
        assert!(allocates_dynamically("aarch64", &[ri("sub", "sp, sp, x8")]));
    }

    #[test]
    fn a_vla_in_the_body_is_dynamic_on_x86_64() {
        // the same function for x86_64-linux-gnu at -O0, clang 17
        let body = [
            ri("pushq", "%rbp"),
            ri("movq", "%rsp, %rbp"),
            ri("subq", "$0x30, %rsp"),
            ri("movq", "%rsp, %rcx"),
            ri("subq", "%rdx, %rcx"),
            ri("movq", "%rcx, %rsp"),
            ri("jge", "0x93"),
            ri("movq", "%rcx, %rsp"),
            ri("movq", "%rbp, %rsp"),
            ri("popq", "%rbp"),
            ri("retq", ""),
        ];
        assert!(allocates_dynamically("x86_64", &body));
        assert!(allocates_dynamically("x86_64", &[ri("subq", "%rax, %rsp")]));
        assert!(allocates_dynamically("x86_64", &[ri("leaq", "(%rsp,%rax), %rsp")]));
    }

    #[test]
    fn restoring_or_aligning_the_stack_pointer_is_not_dynamic() {
        // epilogues that restore sp from the frame pointer, and a realignment
        for (arch, i) in [
            ("aarch64", ri("mov", "sp, x29")),
            ("aarch64", ri("mov", "sp, fp")),
            ("aarch64", ri("sub", "sp, x29, #0x10")),
            ("aarch64", ri("sub", "sp, sp, #0x1, lsl #12")),
            ("aarch64", ri("add", "sp, sp, #0x40")),
            ("x86_64", ri("movq", "%rbp, %rsp")),
            ("x86_64", ri("leaq", "-0x28(%rbp), %rsp")),
            ("x86_64", ri("andq", "$-0x20, %rsp")),
            ("x86_64", ri("subq", "$0x18620, %rsp")),
            ("x86_64", ri("movq", "%rsp, %rbp")),
            ("arm", ri("mov", "sp, r7")),
            ("arm", ri("sub", "sp, r7, #8")),
            ("arm", ri("sub", "sp, #8")),
            ("riscv64", ri("addi", "sp, s0, -32")),
            ("riscv64", ri("mv", "sp, s0")),
        ] {
            assert!(!allocates_dynamically(arch, std::slice::from_ref(&i)), "{arch}: `{} {}` is not a dynamic allocation", i.mnemonic, i.operands);
        }
        for (arch, i) in [("arm", ri("mov", "sp, r3")), ("arm", ri("sub", "sp, sp, r0")), ("riscv64", ri("sub", "sp, sp, a0")), ("riscv64", ri("mv", "sp, a5"))] {
            assert!(allocates_dynamically(arch, std::slice::from_ref(&i)), "{arch}: `{} {}` is a dynamic allocation", i.mnemonic, i.operands);
        }
    }

    #[test]
    fn worst_case_chain_and_recursion() {
        let nodes = vec![
            StackNode { frame: 16, dynamic: false, callees: vec![1, 2], indirect_calls: 0, unknown: false },
            StackNode { frame: 32, dynamic: false, callees: vec![], indirect_calls: 0, unknown: false },
            StackNode { frame: 8, dynamic: false, callees: vec![3], indirect_calls: 0, unknown: false },
            StackNode { frame: 64, dynamic: false, callees: vec![2], indirect_calls: 0, unknown: false },
        ];
        let r = worst_case(&nodes);
        assert_eq!(r[1].worst_case, Some(32));
        assert!(!r[2].bounded);
        assert!(!r[0].bounded);
        assert_eq!(r[0].path[0], 0);
    }

    #[test]
    fn an_unknown_size_callee_makes_the_caller_unbounded_even_when_not_the_largest() {
        // 0 calls 1 (unknown size, small) and 2 (bounded, larger total). The old
        // code picked 2 as the max-total display branch and reported 0 bounded,
        // silently dropping 1's uncertainty (#247).
        let nodes = vec![
            StackNode { frame: 16, dynamic: false, callees: vec![1, 2], indirect_calls: 0, unknown: false },
            StackNode { frame: 8, dynamic: false, callees: vec![], indirect_calls: 0, unknown: true },
            StackNode { frame: 4096, dynamic: false, callees: vec![], indirect_calls: 0, unknown: false },
        ];
        let r = worst_case(&nodes);
        assert!(!r[1].bounded, "the unknown-size node is itself unbounded");
        assert!(!r[0].bounded, "an unknown-size callee makes the caller unbounded, even off the display path");
        assert_eq!(r[0].path[1], 2, "the display path still follows the largest-total branch");
    }

    #[test]
    fn an_interior_node_within_the_depth_bound_is_not_masked_by_a_deep_root() {
        // A linear chain longer than MAX_DEPTH: from node 0 the tail is truncated
        // and unbounded, but an interior node whose own remaining suffix fits
        // below MAX_DEPTH has a real finite bound. The old code cached the
        // truncated result during the deep visit and returned it when that
        // interior node was later visited as a root, so its bound depended on
        // visit order (#247).
        let n = MAX_DEPTH + 2;
        let nodes: Vec<StackNode> =
            (0..n).map(|i| StackNode { frame: 1, dynamic: false, callees: if i + 1 < n { vec![i + 1] } else { vec![] }, indirect_calls: 0, unknown: false }).collect();
        let r = worst_case(&nodes);
        assert!(!r[0].bounded, "the whole chain exceeds MAX_DEPTH and is unbounded from the root");
        let k = MAX_DEPTH - 10; // visited deep-first from node 0, but a short suffix of its own
        assert!(r[k].bounded, "an interior node within the depth bound must be bounded regardless of visit order");
        assert_eq!(r[k].worst_case, Some((n - k) as u64), "its real bound is its own short suffix");
    }

    #[test]
    fn an_unknown_frame_is_not_reported_as_bounded() {
        // #247: a frame whose size could not be determined must fail closed,
        // not count as a bounded zero.
        let nodes = vec![StackNode { frame: 0, dynamic: false, callees: vec![], indirect_calls: 0, unknown: true }];
        let r = worst_case(&nodes);
        assert!(!r[0].bounded, "an unknown frame must not be bounded");
        assert!(r[0].reason.as_deref().unwrap_or("").contains("could not be determined"), "{:?}", r[0].reason);
    }

    #[test]
    fn a_frame_sum_that_overflows_u64_fails_closed_without_panicking() {
        // #247: a hostile `.stack_sizes` can carry near-u64::MAX frames; the
        // sum must not panic (debug) or wrap (release) into a small "bounded"
        // number.
        let nodes = vec![
            StackNode { frame: u64::MAX, dynamic: false, callees: vec![1], indirect_calls: 0, unknown: false },
            StackNode { frame: u64::MAX, dynamic: false, callees: vec![], indirect_calls: 0, unknown: false },
        ];
        let r = worst_case(&nodes);
        assert_eq!(r[0].worst_case, Some(u64::MAX), "overflow saturates for display rather than wrapping");
        assert!(!r[0].bounded, "an overflowing aggregate must fail closed");
        assert!(r[0].reason.as_deref().unwrap_or("").contains("overflow"), "{:?}", r[0].reason);
    }

    #[test]
    fn a_call_chain_past_the_depth_bound_is_unbounded_and_does_not_overflow_the_stack() {
        // #247: a linear chain deeper than MAX_DEPTH would overflow the native
        // stack under a recursive traversal; it must stop and report unbounded.
        let n = MAX_DEPTH + 50;
        let nodes: Vec<StackNode> =
            (0..n).map(|i| StackNode { frame: 16, dynamic: false, callees: if i + 1 < n { vec![i + 1] } else { vec![] }, indirect_calls: 0, unknown: false }).collect();
        let r = worst_case(&nodes);
        assert!(!r[0].bounded, "a chain past the depth bound must be reported unbounded");
        assert!(r[0].reason.as_deref().unwrap_or("").contains("depth"), "{:?}", r[0].reason);
        // The tail, well within the bound, still gets a real answer.
        assert_eq!(r[n - 1].worst_case, Some(16));
        assert!(r[n - 1].bounded);
    }

    /// Run `worst_case` on a deliberately small thread and give up after a
    /// generous deadline, so a super-linear or natively recursive analysis
    /// fails the test instead of hanging or crashing the suite (#247).
    fn worst_case_within(nodes: Vec<StackNode>, secs: u64) -> Vec<StackResult> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || {
                let _ = tx.send(worst_case(&nodes));
            })
            .unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(secs)).expect("worst_case must finish in bounded time on a small stack")
    }

    fn chain_node(frame: u64, callees: Vec<usize>) -> StackNode {
        StackNode { frame, dynamic: false, callees, indirect_calls: 0, unknown: false }
    }

    #[test]
    fn a_wide_graph_past_the_depth_bound_is_analysed_without_exponential_blowup() {
        // A ladder: each level has two functions, each calling both functions
        // of the next level. Every path is longer than MAX_DEPTH, so a
        // traversal that re-descends depth-limited nodes instead of
        // memoising them walks 2^MAX_DEPTH paths. 8k functions and 16k call
        // edges is an ordinary object.
        let levels = MAX_DEPTH + 10;
        let nodes: Vec<StackNode> = (0..2 * levels)
            .map(|i| {
                let l = i / 2;
                chain_node(16, if l + 1 < levels { vec![2 * (l + 1), 2 * (l + 1) + 1] } else { vec![] })
            })
            .collect();
        let r = worst_case_within(nodes, 30);
        assert!(!r[0].bounded && r[0].reason.as_deref().unwrap_or("").contains("depth"), "{:?}", r[0].reason);
        // a level whose own longest chain is exactly MAX_DEPTH is bounded
        let k = 2 * (levels - MAX_DEPTH);
        assert!(r[k].bounded, "{:?}", r[k].reason);
        assert_eq!(r[k].worst_case, Some(16 * MAX_DEPTH as u64));
        assert!(!r[k - 2].bounded, "one level more exceeds the bound");
    }

    #[test]
    fn a_long_chain_is_analysed_in_linear_time() {
        // Far past the bound: each node's verdict is computed once, not
        // re-walked for every root above the bound.
        let n = MAX_DEPTH * 10;
        let nodes: Vec<StackNode> = (0..n).map(|i| chain_node(16, if i + 1 < n { vec![i + 1] } else { vec![] })).collect();
        let r = worst_case_within(nodes, 30);
        assert!(!r[0].bounded);
        assert_eq!((r[0].depth as usize, r[0].path.clone()), (n, vec![0]), "past the bound: the real chain length, and only the symbol as its path");
        assert!(r[n - MAX_DEPTH].bounded && !r[n - MAX_DEPTH - 1].bounded, "the bound is each node's own longest chain");
    }

    #[test]
    fn the_depth_verdict_does_not_depend_on_symbol_order() {
        // The same chain, numbered bottom-up: node i calls i - 1. Visiting
        // the leaf first used to memoise complete results all the way up, so
        // the top was never cut off and came out bounded, while the same
        // graph numbered top-down came out unbounded.
        let n = MAX_DEPTH + 50;
        let nodes: Vec<StackNode> = (0..n).map(|i| chain_node(16, if i > 0 { vec![i - 1] } else { vec![] })).collect();
        let r = worst_case_within(nodes, 30);
        assert!(!r[n - 1].bounded, "a chain past the bound is unbounded whatever the numbering");
        assert!(r[n - 1].reason.as_deref().unwrap_or("").contains("depth"), "{:?}", r[n - 1].reason);
        assert!(r[MAX_DEPTH - 1].bounded && !r[MAX_DEPTH].bounded);
    }

    #[test]
    fn x86_64_negative_add_to_rsp_is_counted_in_prologue() {
        let f = frame_from_prologue("x86_64", &[ri("pushq", "%rbp"), ri("movq", "%rsp, %rbp"), ri("addq", "$-0x80, %rsp"), ri("callq", "0x100")]);
        assert_eq!((f.bytes, f.source, f.dynamic), (8 + 0x80, FrameSource::Prologue, false));
    }

    #[test]
    fn post_probe_or_post_branch_stack_decrement_fails_closed() {
        // Apple AArch64 large static frame: blr x16 (___chkstk_darwin) then sub sp, sp, #2, lsl #12
        let a64 = frame_from_prologue(
            "aarch64",
            &[ri("stp", "x29, x30, [sp, #-0x10]!"), ri("mov", "x29, sp"), ri("blr", "x16 <___chkstk_darwin@GOT>"), ri("sub", "sp, sp, #0x2, lsl #12"), ri("ret", "")],
        );
        assert_eq!(a64.source, FrameSource::Unknown, "post-probe sub sp must fail closed, not report 16B: {a64:?}");
        // Rust x86-64 __rust_probestack followed by subq $0x2000, %rsp
        let x86 = frame_from_prologue("x86_64", &[ri("pushq", "%rbp"), ri("callq", "0x100 <__rust_probestack>"), ri("subq", "$0x2000, %rsp"), ri("retq", "")]);
        assert_eq!(x86.source, FrameSource::Unknown, "post-probe subq %rsp must fail closed: {x86:?}");
    }

    #[test]
    fn x86_memory_sourced_pop_and_xchg_rsp_are_dynamic() {
        for (m, o) in [("movq", "-0x28(%rbp), %rsp"), ("subq", "-0x10(%rbp), %rsp"), ("popq", "%rsp"), ("xchgq", "%rax, %rsp"), ("xchgq", "%rsp, %rax")] {
            assert!(allocates_dynamically("x86_64", &[ri(m, o)]), "{m} {o} must be dynamic");
        }
    }

    #[test]
    fn duplicate_su_names_keep_worst_case_and_missing_qualifier_is_rejected() {
        let su = "a.c:10:1:helper\t2048\tdynamic\nb.c:10:1:helper\t16\tstatic\nc.c:5:1:truncated\t32\n";
        let entries = parse_su(su);
        assert_eq!(entries.len(), 2, "truncated line with no qualifier must be ignored: {entries:?}");
        let map = su_lookup(&entries);
        assert_eq!(map.get("helper"), Some(&(2048, true)), "larger/dynamic record must not be overwritten by smaller static one");
    }

    #[test]
    fn count_reglist_handles_reversed_braces_without_panicking() {
        assert_eq!(count_reglist("}r0,r1{"), 0);
        assert_eq!(count_reglist("{r4-r7,lr}"), 5);
    }
}
