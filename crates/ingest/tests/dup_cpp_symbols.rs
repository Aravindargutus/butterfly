// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The C++ shape of #36: same-named internal-linkage functions in two
//! translation units.
//!
//! llvm-cov qualifies an internal-linkage C++ function the same way it does
//! in C -- `geom.cpp:_ZN12_GLOBAL__N_15scaleEi` -- and butterfly demangles
//! the tail while keeping the prefix, giving
//! `geom.cpp:(anonymous namespace)::scale(int)`. That name contains `::`,
//! and a splitter that gave up on `::` treated the whole string as one bare
//! name, so duplicate C++ locals never reached the unit resolver at all
//! (review of #36).
//!
//! Runs only when the fixture has been built; skipped, not failed, otherwise.
use butterfly_model::*;
use std::path::PathBuf;

fn ingest() -> Option<Evidence> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cpp-header-inline");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/cpp-header-inline/build.sh first");
        return None;
    }
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root);
    Some(butterfly_ingest::ingest(&o).expect("ingest"))
}

#[test]
fn anonymous_namespace_copies_keep_their_own_unit() {
    let Some(ev) = ingest() else { return };
    let scales: Vec<_> = ev.symbols.iter().filter(|s| s.name.contains("scale")).collect();
    assert_eq!(scales.len(), 2, "one copy per translation unit: {:?}", scales.iter().map(|s| &s.name).collect::<Vec<_>>());

    let owner = |s: &Symbol| ev.functions[s.function_id.unwrap_or_else(|| panic!("{} is attributed", s.name))].name.clone();
    let mut owners: Vec<String> = scales.iter().map(|s| owner(s)).collect();
    owners.sort();
    assert_eq!(
        owners,
        vec!["engine.cpp:(anonymous namespace)::scale(int)".to_string(), "geom.cpp:(anonymous namespace)::scale(int)".to_string()],
        "each symbol belongs to the unit that compiled it"
    );

    // both have their own object code, at their own address
    let a = ev.functions.iter().find(|f| f.name.starts_with("geom.cpp:")).unwrap();
    let b = ev.functions.iter().find(|f| f.name.starts_with("engine.cpp:")).unwrap();
    let (ao, bo) = (a.object.as_ref().expect("geom's copy"), b.object.as_ref().expect("engine's copy"));
    assert_ne!(ao.address, bo.address);

    // Nothing the coverage export knows about is left unattributed.
    //
    // Not "every text symbol in the image": a Mach-O executable carries none
    // beyond the program's own, but an ELF one carries the C runtime's
    // startup stubs -- _start, _init, _fini, frame_dummy, the tm_clones pair
    // -- which have no coverage record and are legitimately unattributed.
    // Requiring the whole image failed on Ubuntu for that reason alone. The
    // property that must hold on every platform is narrower and stronger:
    // no symbol whose name the evidence analysed may be left behind.
    let im = ev.image.as_ref().unwrap();
    let analysed: std::collections::HashSet<&str> = ev.functions.iter().map(|f| bare(&f.name)).collect();
    for &i in &im.text_symbols_uncovered {
        let s = &ev.symbols[i];
        assert!(!analysed.contains(bare(&s.name)), "`{}` @{:#x} is analysed code and must not be left unattributed", s.name, s.address);
    }
}

/// A function name without its translation-unit qualifier, which the symbol
/// table never carries.
fn bare(name: &str) -> &str {
    if name.contains("::") {
        // C++: the first single colon separates the unit, `::` does not
        let b = name.as_bytes();
        for i in 0..b.len() {
            if b[i] == b':' && b.get(i + 1) != Some(&b':') && (i == 0 || b[i - 1] != b':') {
                return &name[i + 1..];
            }
        }
        return name;
    }
    match name.rfind(':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// The header `inline` is compiled into every unit that includes it, and the
/// linker keeps one copy; it must still be attributed.
#[test]
fn the_header_inline_is_attributed() {
    let Some(ev) = ingest() else { return };
    let f = ev.functions.iter().find(|f| f.name.contains("clamp_to")).expect("clamp_to analysed");
    assert!(f.object.is_some(), "the surviving copy carries object code");
}

/// #206: the gap classifier and the call graph must answer "which function
/// is this call?" the same way.
///
/// `flight::drive` and `flight::span` each call *their own unit's*
/// `(anonymous namespace)::scale(int)`. The call graph binds both, through
/// the compilation unit that holds the target address (#36/#37). A
/// helper-call gap at the same address, saying the call leaves the covered
/// source set, is the two paths contradicting each other -- a false finding
/// a reviewer has to chase.
///
/// Stated as an invariant over the whole fixture rather than a gap count,
/// which is host-dependent: no call the graph resolved to a covered
/// function may carry a helper-call gap.
#[test]
fn a_call_the_graph_resolves_is_not_a_gap_outside_the_covered_source_set() {
    let Some(ev) = ingest() else { return };

    // every call site the graph bound to a function in the covered set
    let mut bound: Vec<(u64, String, String)> = Vec::new();
    for f in &ev.functions {
        for e in f.callees.iter().filter(|e| e.function_id.is_some()) {
            bound.extend(e.sites.iter().map(|s| (s.addr, f.name.clone(), e.symbol.clone())));
        }
    }
    assert!(!bound.is_empty(), "the fixture has a call graph to compare against");

    for f in &ev.functions {
        let Some(o) = f.object.as_ref() else { continue };
        for g in o.gaps.iter().filter(|g| g.kind == GapKind::HelperCall) {
            if let Some((_, caller, callee)) = bound.iter().find(|(a, _, _)| *a == g.addr_start) {
                panic!("{caller} @{:#x} calls {callee}, which the call graph resolved, yet the gap says: {}", g.addr_start, g.description);
            }
        }
    }

    // and the two calls this was found on, named: each unit's own `scale`
    // is reached, and neither call is described as a helper call
    for (caller, unit) in [("flight::drive(int)", "engine.cpp:"), ("flight::span(int, int)", "geom.cpp:")] {
        let f = ev.functions.iter().find(|f| f.name == caller).unwrap_or_else(|| panic!("missing {caller}"));
        let e = f
            .callees
            .iter()
            .find(|e| e.symbol.starts_with(unit) && e.symbol.contains("scale"))
            .unwrap_or_else(|| panic!("{caller} calls {unit}'s scale: {:?}", f.callees.iter().map(|e| &e.symbol).collect::<Vec<_>>()));
        assert!(e.function_id.is_some(), "{caller}'s call to {unit}scale is bound to a function");
        let o = f.object.as_ref().unwrap_or_else(|| panic!("{caller} has object code"));
        assert!(
            !o.gaps.iter().any(|g| g.kind == GapKind::HelperCall && g.description.contains("scale")),
            "{caller} must not report its own unit's scale as outside the covered source set: {:?}",
            o.gaps.iter().map(|g| &g.description).collect::<Vec<_>>()
        );
    }
}
