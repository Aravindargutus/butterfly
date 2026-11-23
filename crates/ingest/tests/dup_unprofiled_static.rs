// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The negative case of #206, which #237 is about: a call the call graph
//! cannot bind must keep its `helper-call` gap, even when some *other*
//! translation unit exports a covered function of the callee's name.
//!
//! `examples/c-unprofiled-static` holds three `scale`s: a covered extern
//! (`scale`), a covered static (`covered.c:scale`), and a static in
//! `quiet.c` the profile has no record for at all. `beta` calls the third.
//! The DWARF says quiet.c emitted the code at that address and quiet.c has
//! no covered `scale`, so the call leaves the covered source set -- and the
//! extern's name must not override that definitive no.
//!
//! Runs only when the fixture has been built
//! (`examples/c-unprofiled-static/build.sh`); skipped, not failed, otherwise.
use butterfly_model::*;
use std::path::PathBuf;

fn fixture() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-unprofiled-static");
    let build = root.join("build");
    if build.join("coverage.json").exists() && build.join("app").exists() {
        Some(root)
    } else {
        // Built by ci.yml and ci-local, so it is required where the fixtures
        // are (#242): absent, fail rather than pass without asserting.
        butterfly_testsupport::fixture_missing("examples/c-unprofiled-static/build not present (run examples/c-unprofiled-static/build.sh)");
        None
    }
}

fn ingest() -> Option<Evidence> {
    let root = fixture()?;
    let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts.source_root = Some(root);
    Some(butterfly_ingest::ingest(&opts).expect("ingest"))
}

fn f<'a>(ev: &'a Evidence, n: &str) -> &'a Function {
    ev.functions.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("missing {n}"))
}

/// The premise: the fixture really does hold the three-way name clash, and
/// exactly two of the three have a coverage record.
#[test]
fn three_units_define_scale_and_only_two_are_covered() {
    let Some(ev) = ingest() else { return };
    for n in ["scale", "covered.c:scale"] {
        assert!(f(&ev, n).object.is_some(), "{n} is covered and has object code");
    }
    assert!(!ev.functions.iter().any(|f| f.name.contains("quiet.c")), "quiet.c's static has no coverage record: {:?}", ev.functions.iter().map(|f| &f.name).collect::<Vec<_>>());

    let scales: Vec<_> = ev.symbols.iter().filter(|s| s.name == "scale").collect();
    assert_eq!(scales.len(), 3, "the image carries all three copies");
    let attributed = scales.iter().filter(|s| s.function_id.is_some()).count();
    assert_eq!(attributed, 2, "two are attributed, the unprofiled one is not");

    // and the shortfall is explained rather than silently missing
    let im = ev.image.as_ref().unwrap();
    assert!(im.findings.iter().any(|s| s.contains("scale") && s.contains("attributed to none")), "a finding accounts for the third copy: {:?}", im.findings);
}

/// #237. `beta`'s call is unbound in the call graph, so it must also be a
/// `helper-call` gap. Before the fix the name-only fallback matched the
/// unrelated extern `scale` and deleted the gap, leaving the gap check and
/// the call graph disagreeing about one call -- the very thing #206 closed
/// in the positive direction.
#[test]
fn an_unbound_call_keeps_its_gap_though_another_unit_covers_the_name() {
    let Some(ev) = ingest() else { return };
    let beta = f(&ev, "beta");

    let edge = beta.callees.iter().find(|e| e.symbol == "scale").expect("beta calls scale");
    assert_eq!(edge.function_id, None, "the graph leaves it unbound: quiet.c has no covered `scale`");
    let site = edge.sites.first().expect("the edge has a call site").addr;

    let o = beta.object.as_ref().expect("beta has object code");
    let gap = o.gaps.iter().find(|g| g.kind == GapKind::HelperCall && g.addr_start == site).unwrap_or_else(|| {
        panic!("beta's unbound call to `scale` @{site:#x} must be a helper-call gap, but its gaps are {:?}", o.gaps.iter().map(|g| (&g.kind, &g.description)).collect::<Vec<_>>())
    });
    assert!(gap.description.contains("call to `scale`"), "the gap names the callee: {}", gap.description);
    assert!(gap.description.contains("outside the covered source set"), "{}", gap.description);
}

/// The other half of the same rule, so the fix cannot be "report everything".
/// `main` calls all three covered functions by name and the graph binds each,
/// so none of those three call sites may carry a gap; only `printf` does.
#[test]
fn the_calls_the_graph_does_bind_stay_free_of_gaps() {
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

    let main = f(&ev, "main");
    for callee in ["scale", "covered_use", "beta"] {
        assert!(
            main.callees.iter().any(|e| e.symbol == callee && e.function_id.is_some()),
            "main binds its call to {callee}: {:?}",
            main.callees.iter().map(|e| (&e.symbol, e.function_id)).collect::<Vec<_>>()
        );
    }
    // main's extern `scale` and covered.c's static are still two functions
    assert_ne!(f(&ev, "scale").id, f(&ev, "covered.c:scale").id);
    assert_eq!(main.callees.iter().find(|e| e.symbol == "scale").and_then(|e| e.function_id), Some(f(&ev, "scale").id), "main's `scale` is the extern");
    assert_eq!(
        f(&ev, "covered_use").callees.iter().find(|e| e.symbol == "covered.c:scale").and_then(|e| e.function_id),
        Some(f(&ev, "covered.c:scale").id),
        "covered_use's is its own unit's static"
    );
}
