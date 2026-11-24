// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! What butterfly does with everyday C++, on the real `examples/cpp-constructs`
//! build (#166): lambdas, overloaded operators, user-written constructors and
//! destructors, `new`/`delete`.
//!
//! Each of these puts object code in the image that the source spells
//! differently or not at all: a lambda's `operator()` has a compiler-chosen
//! name (`$_0`, or `{lambda(int, int)#1}` inside an inline function); one
//! constructor is several symbols (the complete-object `C1` and base-object
//! `C2`, and for a destructor `D1`, `D2` and the deleting `D0`), of which the
//! coverage export records only one; `new` and `delete` are calls into the
//! C++ runtime, and `delete` through a base pointer is a virtual call.
//!
//! The assertions are about those constructs, not this host's instruction
//! counts, so they hold on the x86-64 ELF runner as on arm64 Mach-O. Where
//! the two compilers differ -- ELF aliases `C1` to `C2`, Mach-O emits `C1` as
//! a separate thunk -- the assertion is the one true of both.

use butterfly_model::{Evidence, Function, GapKind};
use std::path::PathBuf;

fn ingest() -> Option<Evidence> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cpp-constructs");
    let build = root.join("build");
    if !(build.join("coverage.json").exists() && build.join("constructs").exists()) {
        butterfly_testsupport::fixture_missing("examples/cpp-constructs/build not present (run examples/cpp-constructs/build.sh)");
        return None;
    }
    let mut opts = butterfly_ingest::discover(&build).expect("discover");
    opts.source_root = Some(root);
    Some(butterfly_ingest::ingest(&opts).expect("ingest"))
}

fn func<'a>(ev: &'a Evidence, name: &str) -> &'a Function {
    ev.functions.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("{name} is not in the evidence: {:?}", ev.functions.iter().map(|f| &f.name).collect::<Vec<_>>()))
}

/// The one function whose name contains `part`.
fn func_like<'a>(ev: &'a Evidence, part: &str) -> &'a Function {
    let hits: Vec<&Function> = ev.functions.iter().filter(|f| f.name.contains(part)).collect();
    assert_eq!(hits.len(), 1, "{part}: {:?}", hits.iter().map(|f| &f.name).collect::<Vec<_>>());
    hits[0]
}

fn calls(caller: &Function, callee: &Function) -> Vec<Option<u32>> {
    caller.callees.iter().filter(|e| e.function_id == Some(callee.id)).flat_map(|e| e.sites.iter().map(|s| s.line)).collect()
}

/// The coverage record's symbol, without llvm-cov's `file.cpp:` qualifier.
fn bare(mangled: &str) -> &str {
    mangled.rsplit_once(':').map_or(mangled, |(_, m)| m)
}

/// The object symbol, without the leading underscore Mach-O adds to every
/// C-level name.
fn sym<'a>(ev: &Evidence, name: &'a str) -> &'a str {
    let macho = ev.image.as_ref().is_some_and(|im| im.format == "MachO");
    if macho {
        name.strip_prefix('_').unwrap_or(name)
    } else {
        name
    }
}

fn no_project_function_is_dead(ev: &Evidence) {
    let dead: Vec<&str> = ev.image.iter().flat_map(|im| &im.dead_candidates).filter(|d| d.function_id.is_some()).map(|d| d.name.as_str()).collect();
    assert!(dead.is_empty(), "project functions reported as dead code: {dead:?}");
}

#[test]
fn every_lambda_is_matched_to_its_object_code_and_its_callers() {
    let Some(ev) = ingest() else { return };

    // Four lambdas: three named $_N in the non-inline `lambdas`, one named
    // by its signature inside the inline `Channel::peak`. Each coverage
    // record found its operator() in the image -- none is "no object code".
    let lambdas: Vec<&Function> = ev.functions.iter().filter(|f| f.name.contains("::operator()") && (f.name.contains("$_") || f.name.contains("{lambda"))).collect();
    assert_eq!(lambdas.len(), 4, "{:?}", lambdas.iter().map(|f| &f.name).collect::<Vec<_>>());
    for f in &lambdas {
        let o = f.object.as_ref().unwrap_or_else(|| panic!("{} has no object code", f.name));
        assert_eq!(sym(&ev, &o.symbol), bare(&f.mangled), "{} is analysed from the wrong symbol", f.name);
        assert!(f.count > 0, "{} ran, but its count is {}", f.name, f.count);
        assert!(o.unreachable_blocks.is_empty(), "{}: {:?}", f.name, o.unreachable_blocks);
    }
    func_like(&ev, "peak() const::{lambda(int, int)#1}::operator()");

    // Call edges into each closure's operator(): through a template, directly,
    // and from the inline member.
    let lam = func(&ev, "fc::lambdas(int const*, int, int)");
    let by_value = func_like(&ev, "::$_0::operator()");
    let by_ref = func_like(&ev, "::$_2::operator()");
    let count_if = func_like(&ev, "fc::count_if<");
    assert!(!calls(count_if, by_value).is_empty(), "count_if does not call the capturing lambda");
    assert!(!calls(lam, by_ref).is_empty(), "lambdas does not call the by-reference lambda");
    assert!(!calls(func(&ev, "fc::Channel::peak() const"), func_like(&ev, "{lambda(int, int)#1}::operator()")).is_empty());

    // The lambda converted to a function pointer: the call through the
    // pointer is an indirect branch. The conversion operator and __invoke the
    // compiler wrote have no coverage record, so each is a function of its
    // own inside `lambdas`, projected from nothing -- unknown, never covered
    // (#273) -- and calling the conversion is an ordinary call edge to it.
    let o = lam.object.as_ref().unwrap();
    assert!(o.gaps.iter().any(|g| g.kind == GapKind::IndirectBranch), "the call through the function pointer is not an indirect branch");
    for part in ["$_1::operator int (*", "$_1::__invoke("] {
        let f = func_like(&ev, part);
        assert_eq!(f.unrecorded_within, Some(lam.id), "{} is not a no-record function inside lambdas", f.name);
        let fo = f.object.as_ref().unwrap_or_else(|| panic!("{} has no object code", f.name));
        assert_eq!(fo.instr_projected_covered, 0, "{} is projected covered from its enclosing function's lines", f.name);
    }
    assert!(!calls(lam, func_like(&ev, "$_1::operator int (*")).is_empty(), "lambdas does not call the conversion operator");

    // Its operator() is reached only through the compiler's __invoke, whose
    // address the conversion operator returns: not dead code.
    no_project_function_is_dead(&ev);
}

#[test]
fn overloaded_operators_are_functions_with_callers() {
    let Some(ev) = ingest() else { return };
    let main = func(&ev, "main");
    let plus = func(&ev, "fc::operator+(fc::Vec2, fc::Vec2 const&)");
    let plus_eq = func(&ev, "fc::Vec2::operator+=(fc::Vec2 const&)");
    let eq = func(&ev, "fc::operator==(fc::Vec2 const&, fc::Vec2 const&)");
    let index = func(&ev, "fc::Channel::operator[](int)");
    for f in [plus, plus_eq, eq, index] {
        assert!(f.object.is_some(), "{} has no object code", f.name);
        assert!(f.constructs.get("operator").copied().unwrap_or(0) > 0, "{} is not inventoried as an operator: {:?}", f.name, f.constructs);
    }
    // `a + b` and `c == ...` are calls, though the source spells no call.
    assert!(!calls(main, plus).is_empty() && !calls(main, eq).is_empty());
    assert!(!calls(plus, plus_eq).is_empty());
    assert!(calls(func(&ev, "fc::lifetimes(int)"), index).len() >= 4, "every `[]` in lifetimes is a call");
    // operator== is a decision with two conditions, and MC/DC is measured on it.
    assert_eq!(eq.decisions.len(), 1);
    assert_eq!(eq.decisions[0].conditions.len(), 2);
}

#[test]
fn a_constructor_is_analysed_from_the_variant_that_has_its_body() {
    let Some(ev) = ingest() else { return };

    // llvm-cov records a constructor or destructor once, under the variant
    // that holds its body (C2, D2). The image can hold more: on Mach-O, C1
    // and D1 are separate thunks that call C2 and D2, and D0 (the deleting
    // destructor) calls D1 and operator delete. All of them demangle to the
    // same name. The object code analysed must be the body's.
    for f in &ev.functions {
        let o = f.object.as_ref().unwrap_or_else(|| panic!("{} has no object code", f.name));
        assert_eq!(sym(&ev, &o.symbol), bare(&f.mangled), "{} is analysed from {}, not from the symbol its coverage record names", f.name, o.symbol);
    }

    let boosted = func(&ev, "fc::Boosted::Boosted(int, int)");
    let channel = func(&ev, "fc::Channel::Channel(int)");
    let boosted_dtor = func(&ev, "fc::Boosted::~Boosted()");
    let channel_dtor = func(&ev, "fc::Channel::~Channel()");
    // So a derived constructor calls its base's, and a thunk calling its body
    // is not drawn as a function calling itself.
    assert!(!calls(boosted, channel).is_empty(), "Boosted's constructor does not call Channel's: {:?}", boosted.callees);
    assert!(!calls(boosted_dtor, channel_dtor).is_empty(), "~Boosted does not call ~Channel: {:?}", boosted_dtor.callees);
    for f in &ev.functions {
        assert!(calls(f, f).is_empty(), "{} is drawn calling itself; nothing in the example recurses", f.name);
    }

    // Every variant in the image is attributed to its one source function.
    for s in &ev.symbols {
        let m = sym(&ev, &s.mangled);
        for (stem, f) in [("_ZN2fc7ChannelC", channel), ("_ZN2fc7ChannelD", channel_dtor), ("_ZN2fc7BoostedC", boosted), ("_ZN2fc7BoostedD", boosted_dtor)] {
            if m.starts_with(stem) {
                assert_eq!(s.function_id, Some(f.id), "{} is not attributed to {}", s.mangled, f.name);
            }
        }
    }
    // The deleting destructors are reached through the vtable only: not dead.
    no_project_function_is_dead(&ev);

    // The destructor of a local runs at the closing brace of its scope: object
    // code on a line with no statement.
    let lifetimes = func(&ev, "fc::lifetimes(int)");
    let src = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cpp-constructs/constructs.cpp")).unwrap();
    let dtor_lines = calls(lifetimes, channel_dtor);
    assert!(!dtor_lines.is_empty(), "the local's destructor call is not in the call graph");
    for l in dtor_lines {
        let text = src.lines().nth(l.expect("the destructor call has a line") as usize - 1).unwrap().trim();
        assert_eq!(text, "}", "the implicit destructor call is attributed to `{text}`");
    }
}

#[test]
fn new_and_delete_are_calls_into_the_cpp_runtime() {
    let Some(ev) = ingest() else { return };
    let helper = |f: &Function, what: &str| -> String {
        let g = f
            .object
            .as_ref()
            .unwrap()
            .gaps
            .iter()
            .find(|g| g.kind == GapKind::HelperCall && g.description.contains(what))
            .unwrap_or_else(|| panic!("{}: no helper call to {what}", f.name));
        g.class.clone().unwrap_or_default()
    };
    // The allocation functions are the C++ standard library's, like `std::`
    // code: they need their own verification credit.
    assert_eq!(helper(func(&ev, "fc::Channel::Channel(int)"), "operator new[]"), "core-lib");
    assert_eq!(helper(func(&ev, "fc::Channel::~Channel()"), "operator delete[]"), "core-lib");
    let lifetimes = func(&ev, "fc::lifetimes(int)");
    assert_eq!(helper(lifetimes, "operator new("), "core-lib");

    // `delete c` through a base pointer: a null check the source does not
    // write, and a virtual call to the deleting destructor.
    let o = lifetimes.object.as_ref().unwrap();
    let src = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cpp-constructs/constructs.cpp")).unwrap();
    let delete_line = src.lines().position(|l| l.trim() == "delete c;").unwrap() as u32 + 1;
    let on = |k: GapKind| o.gaps.iter().any(|g| g.kind == k && g.line == Some(delete_line));
    assert!(on(GapKind::ExtraObjectBranch), "`delete c`'s null check is not an extra object branch");
    assert!(on(GapKind::IndirectBranch), "`delete c`'s virtual destructor call is not an indirect branch");

    // ...whose callee, the deleting destructor, is where operator delete is
    // called: a helper call in that variant, not an unreported one (#266).
    for dtor in ["fc::Channel::~Channel()", "fc::Boosted::~Boosted()"] {
        let d0 = func(&ev, dtor).variants.iter().find(|v| v.variant.as_deref() == Some("D0")).unwrap_or_else(|| panic!("{dtor} has no D0"));
        let g = d0.gaps.iter().find(|g| g.kind == GapKind::HelperCall && g.description.contains("operator delete(")).unwrap_or_else(|| panic!("{dtor} D0: {:?}", d0.gaps));
        assert_eq!(g.class.as_deref(), Some("core-lib"));
    }
}

#[test]
fn every_constructor_and_destructor_variant_is_analysed_once() {
    let Some(ev) = ingest() else { return };
    let macho = ev.image.as_ref().is_some_and(|im| im.format == "MachO");

    // Every symbol the image attributes to a function is one of that
    // function's analysed bodies (#266). Before, only the body the coverage
    // record names was: the C1/D1 thunks (Mach-O) and every D0 were
    // attributed and then never looked at -- no blocks, no gaps, no status.
    for s in ev.symbols.iter().filter(|s| s.size > 0) {
        let Some(fid) = s.function_id else { continue };
        let f = &ev.functions[fid];
        assert!(f.objects().any(|o| o.address == s.address), "{} ({} B) is attributed to {} but not analysed", s.mangled, s.size, f.name);
    }
    // ...and no address is analysed twice: on ELF C1 and D1 are aliases of
    // C2 and D2, the same bytes under a second name.
    let mut seen = std::collections::HashSet::new();
    for f in &ev.functions {
        for o in f.objects() {
            assert!(seen.insert(o.address), "{:#x} ({}) is analysed twice", o.address, o.symbol);
        }
    }

    let channel = func(&ev, "fc::Channel::Channel(int)");
    let channel_dtor = func(&ev, "fc::Channel::~Channel()");
    let code = |f: &Function| -> Vec<String> {
        let mut v: Vec<String> = f.variants.iter().map(|o| o.variant.clone().unwrap_or_default()).collect();
        v.sort();
        v
    };
    // A virtual destructor has a deleting variant on both formats; the
    // complete-object variants are separate thunks only on Mach-O.
    assert_eq!(code(channel_dtor), if macho { vec!["D0", "D1"] } else { vec!["D0"] }, "{}", channel_dtor.name);
    assert_eq!(code(channel), if macho { vec!["C1"] } else { Vec::<&str>::new() }, "{}", channel.name);
    for f in &ev.functions {
        assert!(f.object.as_ref().unwrap().variant.is_none(), "{}'s body is marked as a variant", f.name);
        if !f.variants.is_empty() {
            assert!(f.name.contains("::Channel(") || f.name.contains("::Boosted(") || f.name.contains("::~"), "{} has variants but is no constructor or destructor", f.name);
        }
    }

    // No variant has a coverage counter of its own (clang instruments the
    // base variant only), and each of these bodies ran, so nothing says
    // whether a variant did: not one of its instructions is credited, and
    // each is a gap a reviewer has to account for. Channel's D0 is the one
    // that never runs in this example -- nothing deletes a Channel through a
    // pointer -- and it must not read as covered.
    for f in &ev.functions {
        for v in &f.variants {
            assert!(f.count > 0);
            assert_eq!(v.instr_projected_covered, 0, "{} is credited with coverage no counter recorded", v.symbol);
            assert_eq!(v.instr_unknown, v.instr_total, "{}", v.symbol);
            assert!(v.instr_total > 0, "{} has no instructions", v.symbol);
            let g: Vec<_> = v.gaps.iter().filter(|g| g.kind == GapKind::UncountedVariant).collect();
            assert_eq!(g.len(), 1, "{}: {:?}", v.symbol, v.gaps);
            assert!(g[0].key.starts_with(&format!("{}@uncounted-variant@", v.symbol)), "{}", g[0].key);
        }
    }

    // D0 is where `delete` through a base pointer reaches the C++ runtime:
    // its call to operator delete is a reported helper call.
    let d0 = channel_dtor.variants.iter().find(|o| o.variant.as_deref() == Some("D0")).unwrap();
    assert_eq!(sym(&ev, &d0.symbol), "_ZN2fc7ChannelD0Ev");
    let del = d0
        .gaps
        .iter()
        .find(|g| g.kind == GapKind::HelperCall && g.description.contains("operator delete("))
        .unwrap_or_else(|| panic!("D0 reports no call to operator delete: {:?}", d0.gaps));
    assert_eq!(del.class.as_deref(), Some("core-lib"));

    // A variant calling its own body is not the function calling itself
    // (asserted for every function above), and the variant's call to
    // operator delete is in the destructor's call graph.
    assert!(channel_dtor.callees.iter().any(|e| e.symbol.starts_with("operator delete(")), "{:?}", channel_dtor.callees);
}

#[test]
fn object_totals_count_each_body_once_and_credit_only_counted_code() {
    let Some(ev) = ingest() else { return };
    let im = ev.image.as_ref().unwrap();

    // `textBytesCovered` is the object code with source-coverage evidence
    // behind it: the bodies the coverage records name, each address once.
    // The variants carry a functionId but no counter, so they are not in it
    // (#266); before, their 296 bytes (Mach-O) were.
    let mut bodies = std::collections::BTreeMap::new();
    for f in &ev.functions {
        let o = f.object.as_ref().unwrap();
        bodies.insert(o.address, o.size);
    }
    assert_eq!(im.text_bytes_covered, bodies.values().sum::<u64>());
    let variant_bytes: u64 = ev.functions.iter().flat_map(|f| &f.variants).map(|v| v.size).sum();
    assert!(variant_bytes > 0);
    assert!(im.text_bytes_covered + variant_bytes <= im.text_bytes);
    // ...and the image findings name them, so the bytes are accounted for
    // rather than silently dropped from the covered figure.
    assert!(im.findings.iter().any(|x| x.contains("ChannelD0Ev")), "{:#?}", im.findings);

    // The summary counts every analysed body: the variants' instructions are
    // in the total, as unknown, and their gaps in the gap count.
    let s = &ev.summary;
    let objs = || ev.functions.iter().flat_map(|f| f.objects());
    assert_eq!(s.instructions_total, objs().map(|o| o.instr_total).sum::<usize>());
    assert_eq!(s.instructions_unknown, objs().map(|o| o.instr_unknown).sum::<usize>());
    assert_eq!(s.gaps_total, objs().map(|o| o.gaps.len()).sum::<usize>());
    assert_eq!(s.gaps_by_kind.get("uncounted-variant").copied(), Some(ev.functions.iter().map(|f| f.variants.len()).sum::<usize>()));
    // The evidence's own integrity check derives the same totals.
    ev.validate().expect("evidence validates");
    ev.check_summary_derivation().expect("summary derives from the functions");
}

#[test]
fn a_trace_shows_which_deleting_destructor_ran() {
    let Some(ev) = ingest() else { return };

    // What source coverage cannot say about a variant, a trace can, and the
    // variants take part in it like any body (#266). The trace here is
    // written from the run's own story: every analysed instruction executed
    // once except those of Channel's deleting destructor, which nothing in
    // the example reaches (the one `delete` is of a Boosted).
    let d0 = |ev: &Evidence, dtor: &str| -> butterfly_model::ObjectFn {
        func(ev, dtor).variants.iter().find(|v| v.variant.as_deref() == Some("D0")).cloned().unwrap_or_else(|| panic!("{dtor} has no D0"))
    };
    let skip = d0(&ev, "fc::Channel::~Channel()").address;
    let mut text = String::new();
    for o in ev.functions.iter().flat_map(|f| f.objects()).filter(|o| o.address != skip) {
        for i in o.blocks.iter().flat_map(|b| &b.instrs) {
            text.push_str(&format!("{:#x}\n", i.addr));
        }
    }
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cpp-constructs-266.trace");
    std::fs::write(&path, text).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cpp-constructs");
    let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts.source_root = Some(root);
    opts.trace = Some(path);
    let ev = butterfly_ingest::ingest(&opts).expect("ingest with the trace");

    let never = d0(&ev, "fc::Channel::~Channel()");
    assert_eq!(never.instr_measured_covered, Some(0), "Channel's D0 is not shown as never run");
    assert!(never.blocks.iter().flat_map(|b| &b.instrs).all(|i| i.measured.is_none_or(|n| n == 0)));
    let ran = d0(&ev, "fc::Boosted::~Boosted()");
    assert_eq!(ran.instr_measured_covered, Some(ran.instr_total), "Boosted's D0 ran in the trace");
    // Measured totals count every body once, variants included.
    let objs = || ev.functions.iter().flat_map(|f| f.objects());
    assert_eq!(ev.summary.instructions_measured_covered, Some(objs().map(|o| o.instr_measured_covered.unwrap_or(0)).sum::<usize>()));
    assert_eq!(ev.summary.instructions_measured_covered, Some(ev.summary.instructions_total - never.instr_total));
}

/// The baseline fingerprints every variant (#280). On the real image, a
/// change confined to `Channel`'s deleting destructor `D0` -- what
/// `-fsized-deallocation` does to its call to `operator delete` -- marks the
/// destructor object-changed, and nothing else.
#[test]
fn a_change_confined_to_a_destructor_variant_is_a_baseline_change() {
    let Some(ev) = ingest() else { return };
    let base = butterfly_ingest::baseline::from_evidence(&ev);
    assert_eq!(butterfly_ingest::baseline::compare(&base, &ev, "b.json").status, "matches");

    let mut changed = ev.clone();
    let dtor = changed.functions.iter_mut().find(|f| f.name == "fc::Channel::~Channel()").expect("Channel's destructor");
    let d0 = dtor.variants.iter_mut().find(|v| v.variant.as_deref() == Some("D0")).expect("Channel's deleting destructor is a variant");
    let call = d0.blocks.iter_mut().flat_map(|b| b.instrs.iter_mut()).find(|i| i.operands.contains("Zdl")).expect("D0 calls operator delete");
    call.operands = call.operands.replace("ZdlPv", "ZdlPvm");
    let chk = butterfly_ingest::baseline::compare(&base, &changed, "b.json");
    assert_eq!(chk.functions_object_changed, vec!["fc::Channel::~Channel()".to_string()], "{chk:?}");
    assert!(chk.functions_source_changed.is_empty());
}
