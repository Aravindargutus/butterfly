// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Unit-level integration tests on synthetic inputs: no compiler, no
//! binaries, deterministic.
mod common;
use butterfly_ingest::{baseline, cfg, constructs, datarefs, gaps, stack, trace};
use butterfly_model::*;
use common::*;

/// if (x == 0) { A } else { B }  as aarch64:
///   0x1000 cmp w0,#0 ; 0x1004 b.ne 0x1010 ; 0x1008 mov w0,#1 ; 0x100c ret ; 0x1010 mov w0,#2 ; 0x1014 ret
fn diamond() -> Vec<butterfly_ingest::objectcode::RawInstr> {
    vec![
        ri(0x1000, "cmp", "w0, #0x0"),
        ri(0x1004, "b.ne", "0x1010 <f+0x10>"),
        ri(0x1008, "mov", "w0, #0x1"),
        ri(0x100c, "ret", ""),
        ri(0x1010, "mov", "w0, #0x2"),
        ri(0x1014, "ret", ""),
    ]
}

#[test]
fn cfg_blocks_edges_and_condition_labels() {
    let raw = diamond();
    let lines = [10u32, 10, 11, 11, 13, 13];
    let counts = [Status::Covered, Status::Covered, Status::Covered, Status::Covered, Status::Uncovered, Status::Uncovered];
    let c = cfg::build("aarch64", 0x1000, 0x1018, &raw, &|i| (Some(lines[i]), Some(0), true), &|i| counts[i], &|_| None, &|_| Vec::new(), &|_| Vec::new());
    assert_eq!(c.blocks.len(), 3);
    assert_eq!(c.cond_branches, 1);
    let b0 = &c.blocks[0];
    assert_eq!(b0.succs.len(), 2);
    let taken = b0.succs.iter().find(|e| e.kind == EdgeKind::Taken).unwrap();
    let fall = b0.succs.iter().find(|e| e.kind == EdgeKind::Fallthrough).unwrap();
    assert_eq!(taken.label.as_deref(), Some("ne"));
    assert_eq!(fall.label.as_deref(), Some("eq"));
    assert_eq!(taken.to, 2);
    assert_eq!(fall.to, 1);
    assert_eq!(taken.projected, Status::Uncovered);
    assert_eq!(fall.projected, Status::Covered);
    assert_eq!(c.blocks[2].kind, BlockKind::Exit);
}

#[test]
fn gaps_detect_extra_branch_predicated_and_helper() {
    let raw = vec![
        ri(0x1000, "cmp", "w0, #0x0"),
        ri(0x1004, "b.ne", "0x1014 <f+0x14>"),
        ri(0x1008, "cmp", "w1, #0x0"),
        ri(0x100c, "b.ne", "0x1014 <f+0x14>"),
        ri(0x1010, "bl", "0x2000 <memcpy>"),
        ri(0x1014, "csel", "w0, w1, w2, lt"),
        ri(0x1018, "ret", ""),
    ];
    // lines: 20 has two object branches but one source branch region -> extra-object-branch
    // line 22 has a source branch but only a csel -> predicated-decision
    let lines = [20u32, 20, 20, 20, 21, 22, 23];
    let c = cfg::build("aarch64", 0x1000, 0x101c, &raw, &|i| (Some(lines[i]), Some(0), true), &|_| Status::Covered, &|_| None, &|_| Vec::new(), &|_| Vec::new());
    let mut blocks = c.blocks;
    let branches = vec![
        Branch { line: 20, col: 9, line_end: 20, col_end: 15, true_count: 1, false_count: 1, direction_known: true, decision_ordinal: None },
        Branch { line: 22, col: 9, line_end: 22, col_end: 15, true_count: 1, false_count: 1, direction_known: true, decision_ordinal: None },
    ];
    let found = gaps::detect(gaps::GapInput {
        symbol: "f",
        blocks: &mut blocks,
        branches: &branches,
        decisions: &[],
        call_target: &|addr| (addr == 0x1010).then(|| gaps::CallTarget { symbol: "memcpy".into(), in_source_set: false }),
        demangle: &|s: &str| s.to_string(),
        project_prefixes: &[],
        recorded: true,
        uncounted: false,
    });
    let kinds: Vec<GapKind> = found.iter().map(|g| g.kind).collect();
    assert!(kinds.contains(&GapKind::ExtraObjectBranch), "{kinds:?}");
    assert!(kinds.contains(&GapKind::PredicatedDecision), "{kinds:?}");
    let helper = found.iter().find(|g| g.kind == GapKind::HelperCall).expect("helper call gap");
    assert_eq!(helper.class.as_deref(), Some("memory-helper"));
    assert!(helper.description.contains("memcpy"));
}

/// A function with no coverage record of its own (#273): its lines have
/// counts -- the enclosing function's -- so nothing is projected, and every
/// run of its instructions with a source line says *why* nothing is: no
/// record, not an uninstrumented line. The rest of the analysis is the same.
#[test]
fn gaps_of_a_function_with_no_coverage_record() {
    let raw = vec![
        ri(0x1000, "ldr", "w8, [x0]"),
        ri(0x1004, "adds", "w8, w8, #0x1"),
        ri(0x1008, "b.vs", "0x1018 <c+0x18>"),
        ri(0x100c, "nop", ""),
        ri(0x1010, "str", "w8, [x0]"),
        ri(0x1014, "ret", ""),
        ri(0x1018, "bl", "0x2000 <panic_const_add_overflow>"),
    ];
    let lines = [93u32, 93, 93, 0, 93, 93, 93];
    let c = cfg::build("aarch64", 0x1000, 0x101c, &raw, &|i| (Some(lines[i]), Some(0), true), &|_| Status::Unknown, &|_| None, &|_| Vec::new(), &|_| Vec::new());
    let mut blocks = c.blocks;
    let found = gaps::detect(gaps::GapInput {
        symbol: "c",
        blocks: &mut blocks,
        branches: &[],
        decisions: &[],
        call_target: &|_| Some(gaps::CallTarget { symbol: "core::panicking::panic_const::panic_const_add_overflow".into(), in_source_set: false }),
        demangle: &|s: &str| s.to_string(),
        project_prefixes: &[],
        recorded: false,
        uncounted: false,
    });
    let kinds: Vec<GapKind> = found.iter().map(|g| g.kind).collect();
    assert!(!kinds.contains(&GapKind::NotInstrumented), "{kinds:?}");
    // the line-0 instruction splits the run in two
    assert_eq!(kinds.iter().filter(|k| **k == GapKind::NoCoverageRecord).count(), 2, "{kinds:?}");
    assert!(kinds.contains(&GapKind::LineZero) && kinds.contains(&GapKind::ExtraObjectBranch) && kinds.contains(&GapKind::HelperCall), "{kinds:?}");
    let g = found.iter().find(|g| g.kind == GapKind::NoCoverageRecord).unwrap();
    assert_eq!((g.line, g.key.as_str()), (Some(93), "c@no-coverage-record@1000"));
    assert!(g.description.contains("no coverage record"), "{}", g.description);
    // every instruction with a source line carries one
    for i in blocks.iter().flat_map(|b| &b.instrs).filter(|i| i.line.is_some_and(|l| l > 0)) {
        assert!(i.gap_ids.iter().any(|&id| found[id].kind == GapKind::NoCoverageRecord), "{:#x}", i.addr);
    }
}

/// Which covered function a symbol's name puts it inside (#273). The cut is
/// a path step outside every bracket, and the innermost covered one wins.
#[test]
fn enclosing_function_by_name() {
    use butterfly_ingest::enclosing_function as within;
    let covered = |names: &'static [&'static str]| move |p: &str| names.iter().position(|n| *n == p);
    let rust = covered(&["constructs::closures", "constructs::closures::{closure#0}", "constructs::each_sample::<constructs::closures::{closure#1}>", "k::f"]);
    assert_eq!(within("constructs::closures::{closure#1}", rust), Some(0));
    // nested: the innermost function with a record
    assert_eq!(within("constructs::closures::{closure#0}::{closure#0}", rust), Some(1));
    assert_eq!(within("constructs::closures::{closure#1}::{closure#0}", rust), Some(0));
    // a `::` inside generic arguments is another item's path, not a step
    assert_eq!(within("constructs::other::<constructs::closures::{closure#1}>", rust), None);
    assert_eq!(within("<constructs::closures::{closure#1} as core::ops::function::FnOnce<(i32,)>>::call_once", rust), None);
    assert_eq!(within("k::f::<fn(i32) -> i32>::{closure#0}", rust), None, "k::f::<..> is not k::f");
    assert_eq!(within("k::g::<fn(i32) -> i32>::{closure#0}", covered(&["k::g::<fn(i32) -> i32>"])), Some(0), "`->` is not a closing bracket");
    // the name itself is not code inside a function
    assert_eq!(within("constructs::closures", rust), None);
    assert_eq!(within("core::fmt::write", rust), None);

    let cpp = covered(&["fc::lambdas(int const*, int, int)", "constructs.cpp:fc::lambdas(int const*, int, int)::$_1::operator()(int) const"]);
    assert_eq!(within("fc::lambdas(int const*, int, int)::$_1::__invoke(int)", cpp), Some(0));
    assert_eq!(within("fc::lambdas(int const*, int, int)::$_1::operator int (*)(int)() const", cpp), Some(0));
    assert_eq!(within("fc::Channel::max(int, int)::'lambda'(int, int)::__invoke(int, int)", cpp), None);
    // unbalanced (an `operator<` in the name): no answer rather than a guess
    assert_eq!(within("fc::operator<(fc::Vec2 const&, fc::Vec2 const&)::$_0::__invoke()", covered(&["fc::operator<(fc::Vec2 const&, fc::Vec2 const&)"])), None);
}

/// A C++ deleting destructor (`D0`) as aarch64: destroy through the body,
/// then `operator delete`. It has no coverage counter of its own, so every
/// instruction is projected unknown (#266). That is not "a source line with
/// no coverage count" -- the line has one, the body's -- so no
/// `not-instrumented` gap; the call into the C++ runtime is still a helper
/// call, and compiler-written code on line 0 is still line 0.
#[test]
fn an_uncounted_variant_is_not_misreported_as_uninstrumented_lines() {
    let raw = vec![
        ri(0x1000, "stp", "x29, x30, [sp, #-0x10]!"),
        ri(0x1004, "bl", "0x2000 <__ZN2fc7ChannelD1Ev>"),
        ri(0x1008, "bl", "0x3000 <__ZdlPv>"),
        ri(0x100c, "ldp", "x29, x30, [sp], #0x10"),
        ri(0x1010, "ret", ""),
    ];
    let lines = [55u32, 55, 55, 0, 55];
    let run = |uncounted: bool| {
        let c = cfg::build("aarch64", 0x1000, 0x1014, &raw, &|i| (Some(lines[i]), Some(0), true), &|_| Status::Unknown, &|_| None, &|_| Vec::new(), &|_| Vec::new());
        let mut blocks = c.blocks;
        gaps::detect(gaps::GapInput {
            symbol: "__ZN2fc7ChannelD0Ev",
            blocks: &mut blocks,
            branches: &[],
            decisions: &[],
            call_target: &|addr| match addr {
                0x1004 => Some(gaps::CallTarget { symbol: "_ZN2fc7ChannelD1Ev".into(), in_source_set: true }),
                0x1008 => Some(gaps::CallTarget { symbol: "_ZdlPv".into(), in_source_set: false }),
                _ => None,
            },
            demangle: &|s: &str| butterfly_ingest::demangle(s),
            project_prefixes: &["fc::".to_string()],
            recorded: true,
            uncounted,
        })
    };
    let kinds = |gs: &[Gap]| gs.iter().map(|g| g.kind).collect::<Vec<_>>();
    let variant = run(true);
    assert!(!kinds(&variant).contains(&GapKind::NotInstrumented), "{:?}", kinds(&variant));
    assert!(kinds(&variant).contains(&GapKind::LineZero), "{:?}", kinds(&variant));
    let del = variant.iter().find(|g| g.kind == GapKind::HelperCall).expect("the call to operator delete is a helper call");
    assert!(del.description.contains("operator delete(void*)"), "{}", del.description);
    assert_eq!(del.class.as_deref(), Some("core-lib"));
    // The same instructions in a body, whose status is read from line
    // counts, do say the line has none.
    assert!(kinds(&run(false)).contains(&GapKind::NotInstrumented));
}

#[test]
fn helper_classification() {
    use gaps::classify_helper as c;
    assert_eq!(c("__stack_chk_fail", &[]), "runtime-check");
    // stack probes a VLA or large frame calls (#271)
    assert_eq!(c("__chkstk_darwin", &[]), "runtime-check");
    assert_eq!(c("__rust_probestack", &[]), "runtime-check");
    assert_eq!(c("core::panicking::panic_bounds_check", &[]), "runtime-check");
    assert_eq!(c("__aeabi_uidivmod", &[]), "arith-helper");
    assert_eq!(c("compiler_builtins::mem::memcpy", &[]), "memory-helper");
    assert_eq!(c("<core::option::Option<u32>>::is_some", &[]), "core-lib");
    assert_eq!(c("printf", &[]), "libc");
    assert_eq!(c("kernel::sched::tick", &["kernel".into()]), "project-uncovered");
    assert_eq!(c("mystery_fn", &[]), "unknown");
    // the global allocation functions (#166), including the sized and
    // aligned forms; a class's own operator new is not the library's
    assert_eq!(c("operator new(unsigned long)", &[]), "core-lib");
    assert_eq!(c("operator new[](unsigned long)", &[]), "core-lib");
    assert_eq!(c("operator delete(void*, unsigned long)", &[]), "core-lib");
    assert_eq!(c("operator delete[](void*, std::align_val_t)", &[]), "core-lib");
    assert_eq!(c("fc::Pool::operator new(unsigned long)", &["fc".into()]), "project-uncovered");
    assert_eq!(c("operator==(Vec2 const&, Vec2 const&)", &[]), "unknown");
}

#[test]
fn stack_prologue_and_worst_case() {
    let f = stack::frame_from_prologue("aarch64", &[ri(0, "stp", "x29, x30, [sp, #-0x20]!"), ri(4, "sub", "sp, sp, #0x40"), ri(8, "bl", "0x100")]);
    assert_eq!(f.bytes, 0x60);
    assert_eq!(f.source, FrameSource::Prologue);
    let dynamic = stack::frame_from_prologue("aarch64", &[ri(0, "sub", "sp, sp, x8"), ri(4, "ret", "")]);
    assert!(dynamic.dynamic);
    let nodes = vec![
        stack::StackNode { frame: 16, dynamic: false, callees: vec![1], indirect_calls: 0, unknown: false },
        stack::StackNode { frame: 32, dynamic: false, callees: vec![2], indirect_calls: 1, unknown: false },
        stack::StackNode { frame: 8, dynamic: false, callees: vec![], indirect_calls: 0, unknown: false },
    ];
    let r = stack::worst_case(&nodes);
    assert_eq!(r[0].worst_case, Some(56));
    assert_eq!(r[0].path, vec![0, 1, 2]);
    assert!(!r[0].bounded, "indirect call in the chain must not be reported as bounded");
    assert!(r[2].bounded);
}

#[test]
fn trace_qemu_transitions_and_expand() {
    let log = "IN: a\n0x1000:  d503201f  nop\n0x1004:  14000000  b 0x1010\n\nIN: b\n0x1010:  d65f03c0  ret\n\nTrace 0: 0x1 [0/0000000000001000/0/0] \nTrace 0: 0x2 [0/0000000000001010/0/0] \nTrace 0: 0x1 [0/0000000000001000/0/0] \n";
    let mut t = trace::parse(log);
    assert_eq!(t.format, "qemu -d in_asm,exec");
    assert_eq!(t.block_executions, 3);
    assert_eq!(t.transitions[&(0x1000, 0x1010)], 1);
    assert_eq!(t.transitions[&(0x1010, 0x1000)], 1);
    t.expand(&|_| vec![]);
    assert_eq!(t.counts[&0x1004], 2);
    assert_eq!(t.counts[&0x1010], 1);
    // execlog and plain formats
    assert_eq!(trace::parse("0, 0x2000, 0xd503201f, \"nop\"\n").counts[&0x2000], 1);
    assert_eq!(trace::parse("0x3000 4\n").counts[&0x3000], 4);
}

#[test]
fn datarefs_aarch64_and_x86() {
    let a = vec![ri(0, "adrp", "x8, 0x40000000 <G>"), ri(4, "str", "w9, [x8, #0x10]"), ri(8, "ldr", "w9, [x8]"), ri(12, "add", "x0, x8, #0x20")];
    let lookup = |a: u64| if (0x40000000..0x40000100).contains(&a) { Some(0x40000000) } else { None };
    let r = datarefs::scan("aarch64", &a, &lookup);
    let kinds: Vec<datarefs::Access> = r.iter().map(|x| x.access).collect();
    assert_eq!(kinds, vec![datarefs::Access::Write, datarefs::Access::Read, datarefs::Access::Address]);
    let x = vec![ri(0, "movl", "%eax, 0x2f0e(%rip)  # 0x40000010 <G>"), ri(8, "lea", "0x2f0e(%rip), %rdi  # 0x40000020 <G>")];
    let r = datarefs::scan("x86_64", &x, &lookup);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].access, datarefs::Access::Write);
    assert_eq!(r[1].access, datarefs::Access::Address);
}

#[test]
fn construct_inventory_c_and_rust() {
    let c: Vec<String> = ["int f(int a){ if (a && b || c) { return a / 2; } for(;;){} switch(a){case 1: break;} return x ? 1 : 0; }"].iter().map(|s| s.to_string()).collect();
    let m = constructs::inventory(&c, constructs::Lang::C);
    for k in ["if", "&&", "||", "division", "for", "switch", "case", "ternary", "return"] {
        assert!(m.contains_key(k), "missing {k}: {m:?}");
    }
    let r: Vec<String> =
        ["fn f(a: u32) -> u32 { match a { 0 => 1, _ => 2 } ; let b = a.checked_add(1)?; if a > 1 && b < 3 { unsafe { *p } } else { 0 } }"].iter().map(|s| s.to_string()).collect();
    let m = constructs::inventory(&r, constructs::Lang::Rust);
    for k in ["match", "?-operator", "checked-arith", "if", "&&", "unsafe", "deref"] {
        assert!(m.contains_key(k), "missing {k}: {m:?}");
    }
}

#[test]
fn baseline_compare_detects_flags_and_source_changes() {
    let f = file(&["int f(int a) {", "  return a + 1;", "}"], &[Some(1), Some(1), Some(1)]);
    let mut ev = evidence(vec![f], vec![function(0, "f", 1, 3)]);
    ev.provenance.build_info.insert("compiler".into(), "clang 17".into());
    ev.provenance.build_info.insert("flags_target".into(), "-O0".into());
    let base = baseline::from_evidence(&ev);
    assert_eq!(base.functions.len(), 1);
    let same = baseline::compare(&base, &ev, "b.json");
    assert_eq!(same.status, "matches");
    // flags changed
    let mut ev2 = ev.clone();
    ev2.provenance.build_info.insert("flags_target".into(), "-O2".into());
    let c = baseline::compare(&base, &ev2, "b.json");
    assert_eq!(c.status, "changed");
    assert!(c.differences.iter().any(|d| d.kind == "flags" && d.invalidates));
    // source changed
    let mut ev3 = ev.clone();
    ev3.files[0].text[1] = "  return a + 2;".into();
    let c = baseline::compare(&base, &ev3, "b.json");
    assert_eq!(c.functions_source_changed, vec!["f".to_string()]);
    // new construct
    let mut ev4 = ev.clone();
    ev4.functions[0].constructs.insert("goto".into(), 1);
    let c = baseline::compare(&base, &ev4, "b.json");
    assert_eq!(c.new_constructs, vec!["goto".to_string()]);
}

#[test]
fn branch_conditions_across_archs() {
    assert_eq!(cfg::branch_conditions("aarch64", "tbnz", "w0, #0x3, 0x1000"), Some(("w0[0x3] == 1".into(), "w0[0x3] == 0".into())));
    assert_eq!(cfg::branch_conditions("x86_64", "ja", "0x1000"), Some(("> (unsigned)".into(), "<= (unsigned)".into())));
    assert_eq!(cfg::branch_conditions("arm", "bge", "0x1000"), Some(("ge".into(), "lt".into())));
    assert_eq!(cfg::branch_conditions("riscv64", "blt", "a0, a1, 0x1000"), Some(("a0 < a1".into(), "a0 >= a1".into())));
    assert_eq!(cfg::branch_conditions("aarch64", "b", "0x1000"), None);
}

#[test]
fn demangles_rust_and_cpp_symbols() {
    use butterfly_ingest::demangle;
    // C++ (Itanium), with and without the Mach-O leading underscore
    assert_eq!(demangle("_Z1fii"), "f(int, int)");
    assert_eq!(demangle("__Z1fii"), "f(int, int)");
    assert!(demangle("_ZNK7Limiter5applyEi").contains("Limiter::apply"));
    // Rust v0 still works
    assert!(demangle("_RNvCshz8ouLyN0mp_4demo7average").contains("demo::average"));
    // C and unknown names pass through untouched
    assert_eq!(demangle("clamp_altitude"), "clamp_altitude");
    assert_eq!(demangle("_start"), "_start");
}

#[test]
fn demangles_internal_linkage_names_behind_a_file_prefix() {
    // llvm-cov names a static / anonymous-namespace function `<file>:<symbol>`.
    // The symbol is still mangled, and the file prefix must survive.
    let d = butterfly_ingest::demangle("sched.cpp:_ZNK6flight12_GLOBAL__N_19ClimbTask3runEi");
    assert!(d.starts_with("sched.cpp:"), "file prefix lost: {d}");
    assert!(d.contains("ClimbTask"), "not demangled: {d}");
    assert!(!d.contains("_ZNK"), "still mangled: {d}");
    // a plain C static keeps its name unchanged
    assert_eq!(butterfly_ingest::demangle("demo.c:op_double"), "demo.c:op_double");
}
