// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Same-named local symbols, in the two shapes that occur (#36):
//!
//!   * a header `static inline` compiled into two units -- both claimants
//!     are unit-qualified (`geom.c:geom_clamp`, `main.c:geom_clamp`);
//!   * a `static scale` in one unit beside an extern `scale` in another --
//!     the claimants do not even share a name shape (`aux.c:scale`,
//!     `scale`), which is the case a qualifier-only rule misses.
//!
//! Runs only when the fixture has been built
//! (`examples/c-header-inline/build.sh`); skipped, not failed, otherwise.
use butterfly_model::*;
use std::path::PathBuf;

fn fixture() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-header-inline");
    let build = root.join("build");
    if build.join("coverage.json").exists() && build.join("app").exists() {
        Some(root)
    } else {
        butterfly_testsupport::fixture_missing("examples/c-header-inline/build not present (run examples/c-header-inline/build.sh)");
        None
    }
}

fn ingest() -> Option<Evidence> {
    let root = fixture()?;
    let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts.source_root = Some(root);
    Some(butterfly_ingest::ingest(&opts).expect("ingest"))
}

/// The heart of #36: each unit's copy of the header function gets its own
/// object code, at its own address, and nothing is silently dropped.
#[test]
fn each_translation_units_copy_keeps_its_own_object_code() {
    let Some(ev) = ingest() else { return };
    let f = |n: &str| ev.functions.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("missing {n}"));

    let g = f("geom.c:geom_clamp");
    let m = f("main.c:geom_clamp");
    let go = g.object.as_ref().expect("geom.c copy has object code");
    let mo = m.object.as_ref().expect("main.c copy has object code");
    assert_ne!(go.address, mo.address, "the copies are distinct code");
    assert!(go.instr_total > 0 && mo.instr_total > 0);

    // both are cross-linked to their own symbol
    for c in [g, m] {
        let sid = c.symbol_id.unwrap_or_else(|| panic!("{} has a symbol", c.name));
        assert_eq!(ev.symbols[sid].function_id, Some(c.id), "{} round-trips through its symbol", c.name);
        assert_eq!(ev.symbols[sid].address, c.object.as_ref().unwrap().address);
    }

    // Both geom_clamp symbols are attributed, and neither collides. (The
    // image also carries the same-basename `tally` pair, one copy of which
    // llvm-cov has no record for at all -- that is
    // `units_with_the_same_basename_are_not_one_unit`, and it is the reason
    // this assertion is about these symbols rather than the whole image.)
    let im = ev.image.as_ref().unwrap();
    assert_eq!(im.text_symbols, ev.symbols.len());
    let clamps: Vec<_> = ev.symbols.iter().filter(|s| s.name == "geom_clamp").collect();
    assert_eq!(clamps.len(), 2, "both copies are in the image");
    assert!(clamps.iter().all(|s| s.function_id.is_some()), "and both are attributed");
    assert!(!im.findings.iter().any(|s| s.contains("geom_clamp")), "no unresolved collision for them: {:?}", im.findings);
}

/// Attribution, proved through the call graph: a static can only be called
/// from its own translation unit, so `main` must call main.c's copy and
/// `geom_span` geom.c's. Before the fix both edges pointed at one of them.
#[test]
fn calls_bind_to_the_callers_own_copy() {
    let Some(ev) = ingest() else { return };
    let f = |n: &str| ev.functions.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("missing {n}"));
    let calls = |from: &str, to: &str| {
        let from = f(from);
        assert!(
            from.callees.iter().any(|e| e.function_id == Some(f(to).id)),
            "{} should call {to}: {:?}",
            from.name,
            from.callees.iter().map(|e| (&e.symbol, e.function_id)).collect::<Vec<_>>()
        );
    };
    calls("main", "main.c:geom_clamp");
    calls("geom_span", "geom.c:geom_clamp");
    // the mixed-linkage pair: aux_halve calls its own unit's static, main
    // the extern one, and the two are different functions
    calls("aux_halve", "aux.c:scale");
    calls("main", "scale");
    // and the reverse edges agree
    assert!(f("main.c:geom_clamp").callers.iter().any(|e| e.function_id == Some(f("main").id)));
    assert!(f("geom.c:geom_clamp").callers.iter().any(|e| e.function_id == Some(f("geom_span").id)));
    assert!(f("aux.c:scale").callers.iter().any(|e| e.function_id == Some(f("aux_halve").id)));
}

/// The shape a qualifier-only rule misses: one claimant is `aux.c:scale`,
/// the other is a plain `scale` with no qualifier at all, so the contested
/// name has to be recognised across both shapes and resolved by the unit
/// each function was compiled in -- for the unqualified one, the file it
/// was found in.
#[test]
fn a_static_and_an_extern_of_one_name_stay_apart() {
    let Some(ev) = ingest() else { return };
    let f = |n: &str| ev.functions.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("missing {n}"));

    let stat = f("aux.c:scale");
    let ext = f("scale");
    let so = stat.object.as_ref().expect("the static has object code");
    let eo = ext.object.as_ref().expect("the extern has object code");
    assert_ne!(so.address, eo.address, "two distinct bodies, not one shown twice");

    // each is behind its own symbol, and each symbol points back
    for c in [stat, ext] {
        let sid = c.symbol_id.unwrap_or_else(|| panic!("{} has a symbol", c.name));
        assert_eq!(ev.symbols[sid].function_id, Some(c.id), "{} round-trips", c.name);
        assert_eq!(ev.symbols[sid].address, c.object.as_ref().unwrap().address);
    }
    // the regression this guards: before the fix both `scale` symbols
    // resolved to the extern, which was then shown the static's body
    let scale_syms: Vec<_> = ev.symbols.iter().filter(|s| s.name == "scale").collect();
    assert_eq!(scale_syms.len(), 2, "the image really does carry two");
    let mut owners: Vec<_> = scale_syms.iter().filter_map(|s| s.function_id).collect();
    owners.sort_unstable();
    owners.dedup();
    assert_eq!(owners.len(), 2, "the two symbols must not share an owner");
}

/// The two copies were exercised on opposite branches, and the projections
/// must reflect that: a swapped attribution would give each copy the other
/// unit's counts.
#[test]
fn the_copies_keep_their_own_coverage() {
    let Some(ev) = ingest() else { return };
    let f = |n: &str| ev.functions.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("missing {n}"));
    // source-level: geom.c's run never reached `return v` or `return lo`;
    // main.c's never reached `return hi` or `return v` -- both partial
    for n in ["geom.c:geom_clamp", "main.c:geom_clamp"] {
        let c = f(n);
        assert!(c.count > 0, "{n} was executed");
        assert!(c.lines_covered < c.lines_total, "{n} is partially covered by design");
        let o = c.object.as_ref().unwrap();
        assert!(o.instr_projected_covered > 0, "{n} has covered instructions");
        assert!(o.instr_projected_uncovered > 0, "{n} has uncovered instructions");
    }
}

/// Two units of the *same file name* in different directories.
///
/// `src/alpha/dup.c` and `src/beta/dup.c` each hold a `static tally`.
/// llvm-cov qualifies both as `dup.c:tally` -- only the basename -- so the
/// coverage export cannot tell them apart and in fact keeps a record for
/// only one. The DWARF unit paths can, and must: comparing basenames alone
/// collapsed the two units into one (review of #36).
#[test]
fn units_with_the_same_basename_are_not_one_unit() {
    let Some(ev) = ingest() else { return };
    let sym_at = |a: u64| ev.symbols.iter().find(|s| s.address == a);
    let tally: Vec<_> = ev.symbols.iter().filter(|s| s.name == "tally").collect();
    assert_eq!(tally.len(), 2, "the image carries both statics");

    // exactly one has a coverage record behind it; the other must be left
    // unattributed rather than credited to the first unit's function
    let owned: Vec<_> = tally.iter().filter_map(|s| s.function_id).collect();
    assert_eq!(owned.len(), 1, "only one copy has a coverage record: {owned:?}");
    assert_eq!(ev.functions[owned[0]].name, "dup.c:tally");
    let orphan = tally.iter().find(|s| s.function_id.is_none()).expect("the other copy is unattributed");
    let _ = sym_at(orphan.address);

    // and the shortfall is explained rather than silently missing
    let im = ev.image.as_ref().unwrap();
    assert!(im.text_bytes_covered < im.text_bytes, "the unattributed copy is out of the totals");
    assert!(im.findings.iter().any(|f| f.contains("tally") && f.contains("attributed to none")), "a finding accounts for it: {:?}", im.findings);
}

/// #206, the C shape: the helper-call gap check and the call graph must
/// resolve one call site the same way.
///
/// `geom_span` and `main` each call *their own unit's* `geom_clamp`, and
/// `aux_halve` its own unit's `scale`; the graph binds all three through the
/// unit that holds the target address (#36). A gap saying those calls leave
/// the covered source set contradicts the graph about the same call.
///
/// The converse is asserted too: the `tally` copy llvm-cov keeps no record
/// for really is outside the covered source set, and must still be reported.
#[test]
fn a_call_the_graph_resolves_is_not_a_gap_outside_the_covered_source_set() {
    let Some(ev) = ingest() else { return };

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

    // named, for the two duplicated-static shapes
    for (caller, callee) in [("geom_span", "geom.c:geom_clamp"), ("main", "main.c:geom_clamp"), ("aux_halve", "aux.c:scale")] {
        let f = ev.functions.iter().find(|f| f.name == caller).unwrap_or_else(|| panic!("missing {caller}"));
        let short = callee.rsplit(':').next().unwrap();
        assert!(f.callees.iter().any(|e| e.symbol == callee && e.function_id.is_some()), "{caller} binds its call to {callee}");
        let o = f.object.as_ref().unwrap_or_else(|| panic!("{caller} has object code"));
        assert!(
            !o.gaps.iter().any(|g| g.kind == GapKind::HelperCall && g.description.contains(&format!("call to `{short}`"))),
            "{caller} must not report its own unit's {short} as outside the covered source set: {:?}",
            o.gaps.iter().map(|g| &g.description).collect::<Vec<_>>()
        );
    }

    // The copy with no coverage record is genuinely outside the covered
    // source set: the graph leaves that edge unbound, and the gap stands.
    let orphan = ev
        .functions
        .iter()
        .find(|f| f.callees.iter().any(|e| e.symbol == "tally" && e.function_id.is_none()))
        .expect("one `tally` caller reaches the copy llvm-cov has no record for");
    let o = orphan.object.as_ref().expect("it has object code");
    assert!(
        o.gaps.iter().any(|g| g.kind == GapKind::HelperCall && g.description.contains("call to `tally`")),
        "{} keeps its genuine helper-call gap: {:?}",
        orphan.name,
        o.gaps.iter().map(|g| &g.description).collect::<Vec<_>>()
    );
}
