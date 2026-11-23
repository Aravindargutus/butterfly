// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! A library ingested as unlinked `.o` files, beside the same library linked
//! with its test harness (#169).
//!
//! `examples/c-lib/build-lib.sh` builds the library as delivered -- five
//! translation units and no `main` -- drives it from a harness for
//! coverage, and links archive and harness into the image to analyse. The
//! objects it compiled on the way are what a project without a link step
//! would hand butterfly instead. The manual compares the two (#160); this
//! test is where those differences are asserted rather than quoted.
//!
//! What is asserted is the shape of the difference the caveats promise, not
//! one host's instruction counts: those are the compiler's, and the manual
//! says which compiler produced the figures it prints.

use butterfly_model::Evidence;
use std::path::{Path, PathBuf};

/// The library's objects: every object `build-lib.sh` compiled except the
/// harness's. Found rather than named, because the file names are the build
/// script's to choose.
fn library_objects(root: &Path) -> Vec<PathBuf> {
    let mut objs: Vec<PathBuf> = std::fs::read_dir(root.join("build-lib/obj"))
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "o") && !p.to_string_lossy().contains("test_flight")).collect())
        .unwrap_or_default();
    objs.sort();
    objs
}

fn example() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-lib").canonicalize().ok()?;
    let lib = root.join("build-lib");
    if lib.join("harness").exists() && lib.join("coverage.json").exists() && library_objects(&root).len() == 5 {
        Some(root)
    } else {
        butterfly_testsupport::fixture_missing("run examples/c-lib/build-lib.sh first");
        None
    }
}

fn linked(root: &Path) -> Evidence {
    let mut opts = butterfly_ingest::discover(&root.join("build-lib")).expect("discover build-lib");
    opts.source_root = Some(root.to_path_buf());
    butterfly_ingest::ingest(&opts).expect("the linked harness ingests")
}

/// Exactly what the example's README tells you to run: the harness's
/// coverage export, and the library's objects by repeated `--object`.
fn unlinked(root: &Path) -> Evidence {
    let lib = root.join("build-lib");
    let opts =
        butterfly_ingest::IngestOptions { coverage_json: lib.join("coverage.json"), objects: library_objects(root), source_root: Some(root.to_path_buf()), ..Default::default() };
    butterfly_ingest::ingest(&opts).expect("unlinked objects ingest, with caveats")
}

#[test]
fn every_unlinked_object_is_named_as_one_and_only_the_first_is_analysed_whole() {
    let Some(root) = example() else { return };
    let ev = unlinked(&root);
    let caveats = &ev.provenance.caveats;
    for o in library_objects(&root) {
        let name = o.file_name().unwrap().to_string_lossy().into_owned();
        assert!(caveats.iter().any(|c| c.contains(&name) && c.contains("UNLINKED")), "{name} is not called unlinked: {caveats:?}");
    }
    assert!(caveats.iter().any(|c| c.contains("only the first object file is used")), "that the symbol table, stack and coupling come from one unit is said: {caveats:?}");
}

/// The differences the caveat promises, each against the linked build of
/// the same sources and the same coverage.
#[test]
fn unlinked_objects_under_report_coupling_stack_and_the_entry_point() {
    let Some(root) = example() else { return };
    let (l, u) = (linked(&root), unlinked(&root));
    let (ls, us) = (&l.summary, &u.summary);

    // coupling: a global-data reference in an object is a relocation, not an address
    assert!(ls.globals > 0 && ls.data_refs > 0, "the control: the linked image shows coupling ({} globals, {} refs)", ls.globals, ls.data_refs);
    assert_eq!((us.globals, us.data_refs), (0, 0), "the objects show none of it");

    // stack: chains stop at the unit boundary, so nothing reaches the harness's unbounded chain
    assert!(us.stack_worst_case < ls.stack_worst_case, "unlinked worst case {:?} B vs linked {:?} B", us.stack_worst_case, ls.stack_worst_case);
    assert!(ls.stack_unbounded > 0 && us.stack_unbounded == 0, "the dispatch table's unbounded chains are only visible linked");

    // the entry point
    assert!(l.image.as_ref().is_some_and(|i| i.entry.is_some()), "the linked image has an entry point");
    assert!(u.image.as_ref().is_some_and(|i| i.entry.is_none()), "an object has none");

    // object code: less of it is analysed, and what was uncovered stays
    // uncovered -- as far as the object format lets it be seen. On Mach-O the
    // library's uncovered instructions are the same code either way; on ELF the
    // relocatable objects are addressed section-relative and project fewer of
    // them, so this is a bound -- the unlinked build never reports *more*
    // uncovered than the linked one -- not a cross-format equality (#169, review).
    assert!(us.instructions_total < ls.instructions_total);
    assert!(us.instructions_projected_covered < ls.instructions_projected_covered);
    assert!(
        us.instructions_projected_uncovered <= ls.instructions_projected_uncovered,
        "unlinked uncovered {} must not exceed linked {}",
        us.instructions_projected_uncovered,
        ls.instructions_projected_uncovered
    );
}

/// With no entry point, reachability cannot be established, and the caveat
/// says so. The evidence used to list dead-code candidates anyway -- among
/// them `saturating_add` and `abs_i32`, which other units call -- because the
/// only callers it could see were in the one object it analysed whole.
#[test]
fn no_dead_code_is_claimed_without_an_entry_point() {
    let Some(root) = example() else { return };
    let u = unlinked(&root);
    let dead: Vec<&str> = u.image.as_ref().map(|i| i.dead_candidates.iter().map(|d| d.name.as_str()).collect()).unwrap_or_default();
    assert!(dead.is_empty(), "dead code was claimed for an unlinked object: {dead:?}");

    // the control: linked, the one genuinely unused function is found. The
    // whole-image candidate list also carries the C runtime's own unused
    // symbols on ELF (_fini, frame_dummy, register_tm_clones,
    // __do_global_dtors_aux), which are not the fixture's functions and not
    // what this asserts. Restrict to the library's own functions -- the ones
    // that have source in the evidence -- so the claim is about the fixture,
    // not the platform's crt (#169, review).
    let l = linked(&root);
    let fixture: std::collections::HashSet<&str> = l.functions.iter().map(|f| f.name.as_str()).collect();
    let dead: Vec<&str> = l.image.as_ref().map(|i| i.dead_candidates.iter().map(|d| d.name.as_str()).filter(|n| fixture.contains(n)).collect()).unwrap_or_default();
    assert_eq!(dead, ["unused_scale"], "among the fixture's own functions, only unused_scale is dead: {dead:?}");
}

/// The deliverable archive's `ar` is named by the path that ran, as the
/// compiler and the LLVM tools are (#289): it was a bare `ar`, whichever PATH
/// found first, and nothing in build-info.txt said which.
#[test]
fn the_archiver_that_made_the_deliverable_is_recorded() {
    let Some(root) = example() else { return };
    let info = std::fs::read_to_string(root.join("build-lib/build-info.txt")).unwrap();
    let ar = info.lines().find_map(|l| l.strip_prefix("ar_path=")).unwrap_or("");
    assert!(Path::new(ar).is_absolute() && Path::new(ar).is_file(), "build-info.txt records the ar that built libflight.a: {info}");
}
