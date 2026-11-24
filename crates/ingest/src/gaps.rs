// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Gap detection: where source-level structural coverage cannot be trusted to
//! describe the object code (DO-178C 6.4.4.2.b, Annex A Table A-7 objective 9).
//!
//! Every gap is a *finding for a human*, never a verdict.

use butterfly_model::{Block, Branch, Decision, EdgeKind, Gap, GapKind, InstrFlag, Status};
use std::collections::{BTreeMap, BTreeSet};

/// Classify the callee of a helper call.
pub fn classify_helper(pretty: &str, project_prefixes: &[String]) -> &'static str {
    // `p`: no leading underscores (for contains/name checks); `p2`: with the
    // C-runtime double underscore kept (Mach-O adds one more, which we drop).
    let p = pretty.trim_start_matches('_');
    let p2: &str = if pretty.starts_with("___") { &pretty[1..] } else { pretty };
    let lower = p.to_ascii_lowercase();
    let has = |pat: &str| lower.contains(pat);
    // project code first, whatever its name contains: `fcs::abort_takeoff` is
    // no compiler-inserted check and `<uart::Uart>::init` no core-library
    // impl, but the substring rules below said so (#325 B2). On a whole path
    // segment, and never for a library root a monomorphised generic or an
    // instantiated template can put among the covered names.
    let segment = |pre: &str| p.trim_start_matches('<').strip_prefix(pre).is_some_and(|rest| rest.starts_with("::"));
    if project_prefixes.iter().any(|pre| !pre.is_empty() && !matches!(pre.as_str(), "core" | "alloc" | "std" | "compiler_builtins") && segment(pre)) {
        return "project-uncovered";
    }
    // runtime / defensive checks (glossary: compiler-inserted checks, not dead code)
    if has("stack_chk")
        || has("__ubsan")
        || has("__asan")
        || has("panic")
        || has("unwrap_failed")
        || has("expect_failed")
        || has("bounds_check")
        || has("slice_index")
        || has("slice_end_index")
        || has("slice_start_index")
        || has("str_index")
        || has("assert")
        || has("abort")
        || has("unreachable")
        || has("_chk_fail")
        || has("div_by_zero")
        || has("overflow")
        || has("begin_panic")
        || has("rust_begin_unwind")
        || has("__fortify")
        // stack probes: touch each page of a large or run-time-sized frame so
        // the guard page is hit (#271)
        || has("chkstk")
        || has("probestack")
        // an exception escaping a noexcept function: std::terminate
        || has("call_terminate")
    {
        return "runtime-check";
    }
    // compiler-rt / libgcc arithmetic helpers
    const RT: &[&str] = &[
        "__aeabi_",
        "__udiv",
        "__div",
        "__mod",
        "__umod",
        "__mul",
        "__ashl",
        "__ashr",
        "__lshr",
        "__float",
        "__fix",
        "__extend",
        "__trunc",
        "__add",
        "__sub",
        "__popcount",
        "__clz",
        "__ctz",
        "__eq",
        "__ne",
        "__lt",
        "__gt",
        "__ge",
        "__le",
        "__cmp",
        "__unord",
        "__powi",
    ];
    if RT.iter().any(|r| p2.starts_with(r)) || (p.starts_with("compiler_builtins::") && !p.starts_with("compiler_builtins::mem")) {
        return "arith-helper";
    }
    if has("memcpy")
        || has("memset")
        || has("memmove")
        || has("memcmp")
        || has("bcmp")
        || has("bzero")
        || has("bcopy")
        || p.starts_with("compiler_builtins::mem")
        || p.starts_with("core::intrinsics::copy")
        || p.starts_with("core::intrinsics::write_bytes")
        || p.starts_with("core::ptr::copy")
        || p.starts_with("core::ptr::write_bytes")
    {
        return "memory-helper";
    }
    // the C++ language runtime (libc++abi / libsupc++, the unwinder): the
    // throw, catch, unwind and dynamic_cast machinery a C++ build with
    // exceptions and RTTI calls (#165)
    if p.starts_with("cxa_") || p.starts_with("Unwind_") || p == "dynamic_cast" || p.starts_with("gxx_personality") {
        return "core-lib";
    }
    if p.starts_with("core::")
        || p.starts_with("alloc::")
        || p.starts_with("std::")
        || p.starts_with("<core::")
        || p.starts_with("<alloc::")
        || p.starts_with("<std::")
        || p.starts_with("<&core::")
        || p.starts_with("<*")
        || p.starts_with("<&")
        || p.starts_with("<[")
        || p.starts_with("<u")
        || p.starts_with("<i")
        || p.starts_with("<bool")
        || p.starts_with("<str")
        || p.starts_with("<char")
        || p.starts_with("<usize")
        || p.starts_with("<isize")
        // the global allocation functions `new` and `delete` call: the C++
        // standard library's, like `std::` code (#166). A project that
        // replaces them has them in its covered source set, and a class's
        // own `operator new` is qualified by the class, so neither lands here.
        || p.starts_with("operator new(")
        || p.starts_with("operator new[](")
        || p.starts_with("operator delete(")
        || p.starts_with("operator delete[](")
    {
        return "core-lib";
    }
    if project_prefixes.iter().any(|pre| !pre.is_empty() && (p.starts_with(pre.as_str()) || p.starts_with(&format!("<{pre}")))) {
        return "project-uncovered";
    }
    const LIBC: &[&str] = &[
        "printf", "puts", "putchar", "fprintf", "sprintf", "snprintf", "strlen", "strcmp", "strncmp", "strcpy", "strncpy", "strcat", "malloc", "calloc", "realloc", "free", "exit",
        "atoi", "atof", "fopen", "fclose", "fread", "fwrite", "fflush", "write", "read", "open", "close", "sin", "cos", "sqrt", "pow", "exp", "log", "floor", "ceil", "fabs",
        "rand", "srand", "time", "qsort", "bsearch", "memchr", "strchr", "strstr", "isdigit", "isalpha", "toupper", "tolower",
    ];
    // the name itself, glibc's fortified `__name_chk`, or a versioned or PLT
    // reference (`name@GLIBC_2.2.5`, `name@plt`). Not `name_` and anything:
    // `log_event`, `read_altitude` or `free_slot_count` is the project's own
    // function, not the C library (#325 B1).
    if LIBC.iter().any(|l| p == *l || p.strip_prefix(l).is_some_and(|rest| rest == "_chk" || rest.starts_with('@'))) {
        return "libc";
    }
    "unknown"
}

/// What a call instruction targets, as the *call graph's* resolver answers.
///
/// Both fields come from one caller-side resolution, on purpose. This check
/// used to take a symbol name and a separate "is this name in the covered
/// set?" predicate, and answer the name-only way: a call to an
/// internal-linkage function whose coverage record is unit-qualified
/// (`engine.cpp:(anonymous namespace)::scale(int)`) never matched the bare
/// symbol the object code names, so the tool reported a call as leaving the
/// covered source set while its own call graph bound that very call to a
/// covered function (#206). Two implementations of "which function is this"
/// is how that happened; there is now one, and it lives with the call graph.
pub struct CallTarget {
    /// Raw symbol named by the call, for the description.
    pub symbol: String,
    /// Whether the call resolves to a function in the covered source set.
    pub in_source_set: bool,
}

pub struct GapInput<'a> {
    pub symbol: &'a str,
    pub blocks: &'a mut [Block],
    pub branches: &'a [Branch],
    pub decisions: &'a [Decision],
    /// What a call at this address targets, if resolvable.
    pub call_target: &'a dyn Fn(u64) -> Option<CallTarget>,
    /// Demangler used for descriptions.
    pub demangle: &'a dyn Fn(&str) -> String,
    /// Namespaces/crates of the covered source set (for `project-uncovered`).
    pub project_prefixes: &'a [String],
    /// Whether the function has a coverage record of its own. When it has
    /// none (#273), an instruction with a source line and no projection is
    /// not on an uninstrumented line: its line has a count, the enclosing
    /// function's, and it is this code that no count speaks for.
    pub recorded: bool,
    /// The instructions' projected status was not read from line counts:
    /// a C++ constructor/destructor variant, which has no counter of its own
    /// (#266). An unknown status then says nothing about the source line,
    /// and `not-instrumented` would misreport it; the caller reports the
    /// variant itself instead.
    pub uncounted: bool,
}

/// Push one gap and return its index: `(gaps, kind, from, to, line, note)`.
/// Passed in so the per-architecture passes below all record gaps the same
/// way, including the id bookkeeping.
type NewGap<'a> = dyn FnMut(&mut Vec<Gap>, GapKind, u64, u64, Option<u32>, String) -> usize + 'a;

pub fn detect(input: GapInput) -> Vec<Gap> {
    let symbol = input.symbol;
    let mut gaps: Vec<Gap> = Vec::new();
    let mut new_gap = |gaps: &mut Vec<Gap>, kind: GapKind, s: u64, e: u64, line: Option<u32>, d: String| -> usize {
        let id = gaps.len();
        gaps.push(Gap {
            id,
            kind,
            addr_start: s,
            addr_end: e,
            line,
            description: d,
            class: None,
            key: match kind {
                GapKind::PredicatedDecision | GapKind::ExtraObjectBranch => format!("{symbol}@{}@L{}", kind_slug(kind), line.unwrap_or(0)),
                _ => format!("{symbol}@{}@{s:x}", kind_slug(kind)),
            },
        });
        id
    };

    // Flat view of instructions in address order.
    let mut flat: Vec<(usize, usize)> = Vec::new(); // (block, instr index)
    for (bi, b) in input.blocks.iter().enumerate() {
        for ii in 0..b.instrs.len() {
            flat.push((bi, ii));
        }
    }

    // 1-3. Runs of instructions with no line / line 0 / uninstrumented line.
    let mut run_kind: Option<GapKind> = None;
    let mut run_start = 0usize;
    let flush = |gaps: &mut Vec<Gap>, blocks: &mut [Block], kind: GapKind, from: usize, to: usize, flat: &[(usize, usize)], new_gap: &mut NewGap| {
        let (b0, i0) = flat[from];
        let (b1, i1) = flat[to - 1];
        let s = blocks[b0].instrs[i0].addr;
        let e = blocks[b1].instrs[i1].addr;
        let line = blocks[b0].instrs[i0].line;
        let count = to - from;
        let desc = match kind {
            GapKind::NoSourceLine => format!("{count} instruction(s) with no DWARF line-table entry: compiler-generated code not traceable to source"),
            GapKind::LineZero => format!("{count} instruction(s) mapped to line 0: compiler-generated code"),
            GapKind::NotInstrumented => format!("{count} instruction(s) map to a source line without a coverage count; object coverage cannot be projected"),
            GapKind::NoCoverageRecord => format!(
                "{count} instruction(s) of a function with no coverage record of its own: the count on their source line is the enclosing function's, so whether this code ran is not established by source coverage"
            ),
            _ => unreachable!(),
        };
        let id = new_gap(gaps, kind, s, e, line, desc);
        for &(b, i) in &flat[from..to] {
            blocks[b].instrs[i].gap_ids.push(id);
        }
    };
    for (k, &(b, i)) in flat.iter().enumerate() {
        let ins = &input.blocks[b].instrs[i];
        let kind = match ins.line {
            None => Some(GapKind::NoSourceLine),
            Some(0) => Some(GapKind::LineZero),
            Some(_) if ins.projected == Status::Unknown && input.uncounted => None,
            Some(_) if ins.projected == Status::Unknown && !input.recorded => Some(GapKind::NoCoverageRecord),
            Some(_) if ins.projected == Status::Unknown => Some(GapKind::NotInstrumented),
            _ => None,
        };
        if kind != run_kind {
            if let Some(rk) = run_kind {
                flush(&mut gaps, input.blocks, rk, run_start, k, &flat, &mut new_gap);
            }
            run_kind = kind;
            run_start = k;
        }
    }
    if let Some(rk) = run_kind {
        flush(&mut gaps, input.blocks, rk, run_start, flat.len(), &flat, &mut new_gap);
    }

    // 4. Per line: conditional branches in object code vs. source branch regions.
    let mut obj_cond: BTreeMap<u32, Vec<(usize, usize)>> = BTreeMap::new();
    let mut obj_any: BTreeSet<u32> = BTreeSet::new();
    for &(b, i) in &flat {
        let ins = &input.blocks[b].instrs[i];
        if let Some(l) = ins.line.filter(|l| *l != 0) {
            obj_any.insert(l);
            if ins.flags.contains(&InstrFlag::CondBranch) {
                obj_cond.entry(l).or_default().push((b, i));
            }
        }
    }
    let mut src_branches: BTreeMap<u32, usize> = BTreeMap::new();
    for br in input.branches {
        *src_branches.entry(br.line).or_default() += 1;
    }
    for (line, sites) in &obj_cond {
        let src = src_branches.get(line).copied().unwrap_or(0);
        if sites.len() > src {
            let first = input.blocks[sites[0].0].instrs[sites[0].1].addr;
            let last = input.blocks[sites[sites.len() - 1].0].instrs[sites[sites.len() - 1].1].addr;
            let desc = if src == 0 {
                format!(
                    "line {line}: {} conditional branch(es) in object code but no source-level decision; coverage of the branch outcomes is not established by source coverage",
                    sites.len()
                )
            } else {
                format!(
                    "line {line}: {} conditional branch(es) in object code for {src} source-level branch region(s); the extra outcomes are not established by source coverage",
                    sites.len()
                )
            };
            let id = new_gap(&mut gaps, GapKind::ExtraObjectBranch, first, last, Some(*line), desc);
            for &(b, i) in sites {
                input.blocks[b].instrs[i].gap_ids.push(id);
            }
        }
    }

    // 5. Source decisions with no object-level branch at all, or with no
    //    object code attributed to their line at all.
    let mut seen_lines = BTreeSet::new();
    let entry_addr = input.blocks.first().map(|b| b.address).unwrap_or(0);
    for d in input.decisions {
        if seen_lines.contains(&d.line) || obj_cond.contains_key(&d.line) {
            continue;
        }
        seen_lines.insert(d.line);
        if !obj_any.contains(&d.line) {
            new_gap(&mut gaps, GapKind::PredicatedDecision, entry_addr, entry_addr, Some(d.line), format!("line {}: decision with {} condition(s) but no object code is attributed to this line (merged or optimised away); its outcomes cannot be located in the object code", d.line, d.conditions.len()));
            continue;
        }
        let (s, e) = line_span(input.blocks, d.line);
        let id = new_gap(
            &mut gaps,
            GapKind::PredicatedDecision,
            s,
            e,
            Some(d.line),
            format!("line {line}: decision with {n} condition(s) compiled to predicated/conditional-select code with no object-level branch; branch/MC-DC outcomes exist only at source level", line = d.line, n = d.conditions.len()),
        );
        mark_line(input.blocks, d.line, id);
    }
    // A switch dispatched through a recovered jump table (#171) realises
    // each case's branch region as a table edge into the case's block, and
    // the default's as the bounds check's taken branch -- not as a
    // conditional branch on the case's own line. Those regions are located
    // in the object code, so they are not predicated decisions.
    let mut dispatched: BTreeSet<u32> = BTreeSet::new();
    for (bi, b) in input.blocks.iter().enumerate() {
        if !b.succs.iter().any(|e| e.kind == EdgeKind::Indirect) {
            continue;
        }
        for e in &b.succs {
            dispatched.extend(input.blocks[e.to].lines.iter().copied());
        }
        // the bounds check guarding the table: its other way out is the default
        for g in input.blocks.iter() {
            if g.succs.iter().any(|e| e.to == bi && e.kind == EdgeKind::Fallthrough) {
                for e in g.succs.iter().filter(|e| e.kind == EdgeKind::Taken) {
                    dispatched.extend(input.blocks[e.to].lines.iter().copied());
                }
            }
        }
    }
    for br in input.branches {
        if seen_lines.contains(&br.line) || obj_cond.contains_key(&br.line) || dispatched.contains(&br.line) {
            continue;
        }
        seen_lines.insert(br.line);
        if !obj_any.contains(&br.line) {
            new_gap(
                &mut gaps,
                GapKind::PredicatedDecision,
                entry_addr,
                entry_addr,
                Some(br.line),
                format!(
                    "line {}: source branch region but no object code is attributed to this line (merged or optimised away); its outcomes cannot be located in the object code",
                    br.line
                ),
            );
            continue;
        }
        let (s, e) = line_span(input.blocks, br.line);
        let id = new_gap(
            &mut gaps,
            GapKind::PredicatedDecision,
            s,
            e,
            Some(br.line),
            format!("line {}: source branch region compiled without an object-level conditional branch", br.line),
        );
        mark_line(input.blocks, br.line, id);
    }

    // 6. Calls to symbols outside the covered source set; 7. indirect branches.
    for &(b, i) in &flat {
        let ins = &input.blocks[b].instrs[i];
        let addr = ins.addr;
        let line = ins.line;
        if ins.flags.contains(&InstrFlag::Call) {
            match (input.call_target)(addr) {
                Some(t) if !t.in_source_set => {
                    let pretty = (input.demangle)(&t.symbol);
                    let class = classify_helper(&pretty, input.project_prefixes);
                    let id = new_gap(
                        &mut gaps,
                        GapKind::HelperCall,
                        addr,
                        addr,
                        line,
                        format!(
                            "call to `{pretty}`, which is outside the covered source set ({})",
                            match class {
                                "runtime-check" => "compiler/runtime check: defensive structure, not dead code (glossary); justify by analysis",
                                "arith-helper" => "compiler-rt / libgcc arithmetic helper: compiler-generated code sequence (6.4.4.2.b)",
                                "memory-helper" => "memory helper (memcpy/memset family): compiler-generated call",
                                "core-lib" => "language core/std library: needs its own verification credit or a qualified library",
                                "libc" => "C library: outside the airborne software or needs library verification credit",
                                "project-uncovered" => "project code with no coverage evidence in this export",
                                _ => "unclassified",
                            }
                        ),
                    );
                    gaps[id].class = Some(class.to_string());
                    input.blocks[b].instrs[i].gap_ids.push(id);
                }
                None => {
                    let id = new_gap(&mut gaps, GapKind::HelperCall, addr, addr, line, "call to an unresolved target".to_string());
                    input.blocks[b].instrs[i].gap_ids.push(id);
                }
                _ => {}
            }
        } else if ins.flags.contains(&InstrFlag::IndirectCall) {
            let id =
                new_gap(&mut gaps, GapKind::IndirectBranch, addr, addr, line, "indirect call (function pointer / vtable): callee set must be established by analysis".to_string());
            input.blocks[b].instrs[i].gap_ids.push(id);
        } else if ins.flags.contains(&InstrFlag::IndirectBranch) {
            // Recovered from its switch table (#171), the targets are known:
            // what is left for a reviewer is to confirm the table is the
            // switch's, not to establish them.
            let targets = input.blocks[b].succs.iter().filter(|e| e.kind == EdgeKind::Indirect).count();
            let desc = if targets > 0 {
                format!("indirect branch through a switch jump table: {targets} distinct target(s) read from the table and drawn as edges; confirm the table and its bound are the switch's")
            } else {
                "indirect branch (jump table or tail call): every target must be established by analysis".to_string()
            };
            let id = new_gap(&mut gaps, GapKind::IndirectBranch, addr, addr, line, desc);
            input.blocks[b].instrs[i].gap_ids.push(id);
        }
    }

    gaps
}

fn line_span(blocks: &[Block], line: u32) -> (u64, u64) {
    let mut s = u64::MAX;
    let mut e = 0;
    for b in blocks {
        for i in &b.instrs {
            if i.line == Some(line) {
                s = s.min(i.addr);
                e = e.max(i.addr);
            }
        }
    }
    if s == u64::MAX {
        (0, 0)
    } else {
        (s, e)
    }
}

fn mark_line(blocks: &mut [Block], line: u32, id: usize) {
    for b in blocks {
        for i in &mut b.instrs {
            if i.line == Some(line) {
                i.gap_ids.push(id);
            }
        }
    }
}

pub fn kind_slug(k: GapKind) -> &'static str {
    match k {
        GapKind::NoSourceLine => "no-source-line",
        GapKind::LineZero => "line-zero",
        GapKind::ExtraObjectBranch => "extra-object-branch",
        GapKind::PredicatedDecision => "predicated-decision",
        GapKind::HelperCall => "helper-call",
        GapKind::IndirectBranch => "indirect-branch",
        GapKind::NotInstrumented => "not-instrumented",
        GapKind::SharedLine => "shared-line",
        GapKind::FoldedCall => "folded-call",
        GapKind::NoCoverageRecord => "no-coverage-record",
        GapKind::UncountedVariant => "uncounted-variant",
    }
}

#[cfg(test)]
mod classify_tests {
    use super::classify_helper;

    /// The C++ runtime calls a build with exceptions and RTTI makes were
    /// "unclassified" (#165). Mach-O adds a leading underscore; both
    /// spellings arrive here.
    #[test]
    fn the_cpp_exception_and_rtti_runtime_is_the_language_runtime() {
        for n in [
            "__cxa_throw",
            "___cxa_throw",
            "__cxa_allocate_exception",
            "__cxa_begin_catch",
            "__cxa_end_catch",
            "_Unwind_Resume",
            "__Unwind_Resume",
            "__dynamic_cast",
            "___gxx_personality_v0",
        ] {
            assert_eq!(classify_helper(n, &[]), "core-lib", "{n}");
        }
        assert_eq!(classify_helper("__clang_call_terminate", &[]), "runtime-check");
        assert_eq!(classify_helper("printf", &[]), "libc", "unchanged");
    }
}
