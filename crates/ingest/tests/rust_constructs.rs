// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! What butterfly does with the Rust constructs of `examples/rust-constructs`
//! (#167): a generic function monomorphised at two types, a trait called
//! through `dyn`, closures that capture and are passed to generic functions,
//! and a `macro_rules!` macro that expands to a decision. Each test names
//! the matrix row it is the `proof` of.
//!
//! What is asserted is what holds of the example's source and of rustc's
//! documented behaviour, not what one host's code generator did: no
//! instruction counts, no frame sizes, no gap totals. The one place the
//! image's data has to be read -- which functions a vtable holds -- is
//! asserted on macOS/AArch64 and x86-64 Linux only, where it has been
//! verified, as for the fn-pointer table in `e2e_rust_constructs.rs`. On
//! Linux the vtable's words are `R_X86_64_RELATIVE` relocations the file
//! holds as zero; until #325 G1 they were not read, and every impl reached
//! only through `dyn` was a dead-code candidate there.
use butterfly_model::*;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture() -> Option<PathBuf> {
    let root = repo().join("examples/rust-constructs");
    if root.join("build/coverage.json").exists() {
        Some(root)
    } else {
        butterfly_testsupport::fixture_missing("examples/rust-constructs/build not present (run examples/rust-constructs/build.sh; it needs a nightly with llvm-tools)");
        None
    }
}

fn ingest(root: &Path) -> Evidence {
    let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts.source_root = Some(root.to_path_buf());
    butterfly_ingest::ingest(&opts).expect("ingest")
}

fn func<'a>(ev: &'a Evidence, name: &str) -> &'a Function {
    ev.functions.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no function {name}; have {:?}", ev.functions.iter().map(|f| &f.name).collect::<Vec<_>>()))
}

/// The projected status of every instruction `f`'s object code maps to `line`.
fn projected_on(f: &Function, line: u32) -> Vec<Status> {
    f.object.as_ref().expect("object code").blocks.iter().flat_map(|b| &b.instrs).filter(|i| i.line == Some(line)).map(|i| i.projected).collect()
}

/// The line a source text first appears on, so a test names the construct
/// and not a number that moves when a comment is added above it.
fn line_of(root: &Path, text: &str) -> u32 {
    let src = std::fs::read_to_string(root.join("constructs.rs")).unwrap();
    src.lines().position(|l| l.contains(text)).unwrap_or_else(|| panic!("{text:?} is not in constructs.rs")) as u32 + 1
}

const VERIFIED_HERE: bool = cfg!(any(all(target_os = "macos", target_arch = "aarch64"), all(target_os = "linux", target_arch = "x86_64")));

/// The rebuilt segments are what the per-instantiation counts below stand
/// on, so they are checked against the one thing that can contradict them:
/// llvm-cov's own file segments. Every function record that maps into a
/// file, put through `records_line_counts` together, must give the file's
/// line counts exactly -- on every example built here, C, C++ and Rust.
#[test]
fn line_counts_rebuilt_from_function_regions_match_llvm_covs_own() {
    let Some(_) = fixture() else { return };
    let mut checked = 0;
    for dir in std::fs::read_dir(repo().join("examples")).unwrap().flatten() {
        let cov = dir.path().join("build/coverage.json");
        let Ok(text) = std::fs::read_to_string(&cov) else { continue };
        let Ok(export) = butterfly_ingest::coverage::parse(&text) else { continue };
        for file in &export.files {
            let n = file.segments.iter().map(|s| s.line as usize).max().unwrap_or(0) + 1;
            let want = butterfly_ingest::coverage::line_counts(file, n);
            let got = butterfly_ingest::coverage::records_line_counts(&export.functions, &file.filename, n);
            let diff: Vec<(usize, Option<u64>, Option<u64>)> = want.iter().zip(&got).enumerate().filter(|(_, (w, g))| w != g).map(|(i, (w, g))| (i + 1, *w, *g)).collect();
            assert!(diff.is_empty(), "{}: {}: (line, llvm-cov, rebuilt) {diff:?}", cov.display(), file.filename);
            checked += 1;
        }
    }
    assert!(checked > 0);
    eprintln!("rust_constructs: {checked} file(s) rebuilt from their function regions");
}

/// `rust.generics`. `limit` is one source function and two object symbols,
/// `limit::<i32>` and `limit::<u64>`, each with a coverage record of its
/// own. main() drives them down different paths: the `i32` copy takes the
/// low clamp and never evaluates `v > hi`; the `u64` copy takes the high
/// clamp and never returns `lo`.
///
/// The file's line counts are the sum over both records, as llvm-cov makes
/// them, so every line of `limit` but the final `v` ran *in some copy*. Each
/// copy's lines and object code used to be projected from those summed
/// counts, and so `limit::<i32>`'s `return hi` read as covered object code
/// though that copy never reached it -- only its own branch record said
/// otherwise. Each copy is now projected from its own record.
#[test]
fn each_monomorphised_copy_is_projected_from_its_own_record() {
    let Some(root) = fixture() else { return };
    let ev = ingest(&root);
    let (i32_, u64_) = (func(&ev, "constructs::limit::<i32>"), func(&ev, "constructs::limit::<u64>"));
    let (lo, hi) = (line_of(&root, "if v < lo"), line_of(&root, "if v > hi"));
    let (ret_lo, ret_hi) = (lo + 1, hi + 1);

    // Two functions, one source range, two symbols -- not merged into one.
    assert_eq!((i32_.line_start, i32_.line_end), (u64_.line_start, u64_.line_end));
    assert_eq!((i32_.instantiations, u64_.instantiations), (1, 1));
    assert_ne!(i32_.symbol_id, u64_.symbol_id);
    assert!(i32_.object.is_some() && u64_.object.is_some(), "both copies are analysed as object code");
    assert!(i32_.mangled.starts_with("_R") && u64_.mangled.starts_with("_R"));

    // Each copy's branch record is its own.
    let outcome = |f: &Function, line: u32| f.branches.iter().find(|b| b.line == line).map(|b| (b.true_count, b.false_count)).unwrap();
    assert_eq!(outcome(i32_, lo), (1, 0));
    assert_eq!(outcome(i32_, hi), (0, 0), "limit::<i32> returned before `v > hi`");
    assert_eq!(outcome(u64_, lo), (0, 1));
    assert_eq!(outcome(u64_, hi), (1, 0));

    // The file view is the sum: both returns ran, in one copy or the other.
    let file = &ev.files[i32_.file_id];
    let count = |l: u32| file.lines[(l - 1) as usize].count;
    assert!(count(ret_lo).is_some_and(|c| c > 0) && count(ret_hi).is_some_and(|c| c > 0), "the file's lines are summed over both copies");

    // Each copy's own view is not.
    for (f, never) in [(i32_, [hi, ret_hi]), (u64_, [ret_lo, ret_lo])] {
        for l in never {
            let p = projected_on(f, l);
            assert!(!p.is_empty(), "{}: object code on line {l}", f.name);
            assert!(!p.contains(&Status::Covered), "{}: line {l} never ran in this copy, but its object code is projected {p:?}", f.name);
        }
    }
    assert!(projected_on(i32_, ret_lo).contains(&Status::Covered), "limit::<i32> did return lo");
    assert!(projected_on(u64_, ret_hi).contains(&Status::Covered), "limit::<u64> did return hi");
    // And so are its line figures: 7 of 9 each when both were read off the
    // summed file.
    assert_eq!((i32_.lines_covered, i32_.lines_total), (4, 9), "limit::<i32>: its signature, `v < lo`, `return lo` and the closing brace");
    assert_eq!((u64_.lines_covered, u64_.lines_total), (6, 9), "limit::<u64>: all but `return lo`, the brace after it and `v`");
    let (fl_cov, fl_tot) = (i32_.line_start..=i32_.line_end).fold((0, 0), |(c, t), l| match count(l) {
        Some(n) => (c + usize::from(n > 0), t + 1),
        None => (c, t),
    });
    assert_eq!((fl_cov, fl_tot), (7, 9), "the file's view of the same lines is the sum over both copies");
}

/// `rust.dyn-dispatch`. `run_law` calls `apply` through a `&dyn Law`: an
/// indirect call through the vtable, which is an indirect-branch gap. The
/// impls it reaches are called by nothing directly, and are not dead: the
/// vtables hold their addresses. The impl no value is ever built for has no
/// vtable, and rustc emits no code for it at all.
#[test]
fn a_dyn_call_is_an_indirect_branch_and_the_impls_behind_it_are_not_dead() {
    let Some(root) = fixture() else { return };
    let ev = ingest(&root);
    let run = func(&ev, "constructs::run_law");
    let [site] = &run.indirect_calls[..] else { panic!("one indirect call in run_law: {}", run.indirect_calls.len()) };
    assert_eq!(site.line, Some(line_of(&root, "law.apply(x)")));
    assert!(run.object.as_ref().unwrap().gaps.iter().any(|g| g.kind == GapKind::IndirectBranch && g.line == site.line), "the call through the vtable is an indirect-branch gap");

    let impls = ["<constructs::Gain as constructs::Law>::apply", "<constructs::Deadband as constructs::Law>::apply"];
    for name in impls {
        let f = func(&ev, name);
        assert!(f.count > 0 && f.object.is_some(), "{name} ran and has object code");
        assert!(f.callers.is_empty(), "{name} is reached only through the vtable: {:?}", f.callers);
    }
    let bypass = func(&ev, "<constructs::Bypass as constructs::Law>::apply");
    assert_eq!(bypass.count, 0);
    assert!(bypass.object.is_none() && bypass.symbol_id.is_none(), "no Bypass is ever built, so there is no vtable for it and no code");

    if !VERIFIED_HERE {
        eprintln!("rust_constructs: the vtable-to-impl links are verified on macOS/AArch64 and x86-64 Linux only; not asserted on this host");
        return;
    }
    let dead: Vec<&str> = ev.image.as_ref().unwrap().dead_candidates.iter().map(|d| d.name.as_str()).collect();
    for name in impls {
        assert!(!dead.contains(&name), "{name} is held by a vtable, so it is not a dead-code candidate");
        assert!(func(&ev, name).pointer_callers.iter().any(|p| p.kind == "data"), "{name}'s address is held in data (its vtable)");
    }
    // The row's caveat: the vtable arrives as an argument, so nothing in
    // run_law names a table or takes an address, and the call has no static
    // candidates at all. When it has some, the caveat is out of date.
    assert!(
        site.candidates.is_empty(),
        "run_law's dyn call now has candidates ({:?}); rust.dyn-dispatch's caveat says it has none",
        site.candidates.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
}

/// `rust.closure`. Three closures in `closures()`: one capturing by
/// reference with a decision in it and passed as `FnMut` to a generic, one
/// capturing by mutable reference whose body rustc does not instrument, and
/// one capturing by move and consumed as `FnOnce`. Each generic that takes a
/// closure is a copy per closure type, and calls its closure directly. The
/// uninstrumented one has no coverage record, and is a function with none
/// (#273).
#[test]
fn closures_are_functions_of_their_own_called_by_their_own_generic_copy() {
    let Some(root) = fixture() else { return };
    let ev = ingest(&root);

    let c0 = func(&ev, "constructs::closures::{closure#0}");
    assert_eq!(c0.line_start, line_of(&root, "each_sample(samples, |s| {"));
    // samples [5, 50, 500] against threshold 10
    assert_eq!(c0.branches.iter().map(|b| (b.line, b.true_count, b.false_count)).collect::<Vec<_>>(), [(line_of(&root, "if s > threshold"), 2, 1)]);
    assert!(c0.object.is_some());
    let callers: Vec<&str> = c0.callers.iter().map(|c| c.symbol.as_str()).collect();
    assert_eq!(callers, ["constructs::each_sample::<constructs::closures::{closure#0}>"], "static dispatch: the generic's copy for this closure calls it");

    let c2 = func(&ev, "constructs::closures::{closure#2}");
    assert_eq!(c2.line_start, line_of(&root, "once(move ||"));
    assert!(c2.count == 1 && c2.object.is_some(), "the FnOnce closure ran once");
    assert_eq!(c2.callers.iter().map(|c| c.symbol.as_str()).collect::<Vec<_>>(), ["constructs::once::<constructs::closures::{closure#2}>"]);

    // One generic, one copy per closure type, each a function with its own
    // symbol.
    for n in ["constructs::each_sample::<constructs::closures::{closure#0}>", "constructs::each_sample::<constructs::closures::{closure#1}>"] {
        let f = func(&ev, n);
        assert!(f.count == 1 && f.object.is_some(), "{n}");
    }

    // `|_| count += 1` has no coverage record: rustc maps nothing in its
    // body. Its code is in the image and runs, on a line the enclosing
    // function's counts cover. It used to be in no function, analysed by
    // nothing, findable only in the image's count of unattributed symbols
    // (#273). It is now a function of its own, marked as having no record
    // and placed inside `closures` -- analysed, and never projected covered.
    // If rustc starts giving it a record, the first assertion says so, and
    // rust.closure's caveat is out of date.
    let c1 = func(&ev, "constructs::closures::{closure#1}");
    let closures = func(&ev, "constructs::closures");
    assert_eq!(c1.unrecorded_within, Some(closures.id), "closure#1 now has a coverage record; rust.closure's caveat says it has none");
    // tied to the line it is written on, by the DWARF
    let written_on = line_of(&root, "each_sample(samples, |_| count += 1)");
    assert_eq!((c1.file_id, c1.line_start, c1.line_end), (closures.file_id, written_on, written_on));
    let file = &ev.files[c1.file_id];
    assert!(file.lines[(written_on - 1) as usize].count.is_some_and(|n| n > 0), "the line reads covered, through closures()'s counts");
    assert!(file.function_ids.contains(&c1.id));
    // ...and none of that is credited to it: no count, no line figures, no
    // projection
    assert_eq!((c1.count, c1.instantiations, c1.lines_covered, c1.lines_total), (0, 0, 0, 0));
    let obj = c1.object.as_ref().expect("closure#1's object code is analysed");
    assert!(obj.instr_total > 0 && obj.instr_projected_covered == 0 && obj.instr_projected_uncovered == 0, "no record, so nothing is projected");
    let on_line = projected_on(c1, written_on);
    assert!(!on_line.is_empty() && on_line.iter().all(|p| *p == Status::Unknown), "{on_line:?}");
    assert!(
        obj.gaps.iter().any(|g| g.kind == GapKind::NoCoverageRecord && g.line == Some(written_on)),
        "the reviewer is told why: {:?}",
        obj.gaps.iter().map(|g| g.kind).collect::<Vec<_>>()
    );
    assert!(!obj.gaps.iter().any(|g| g.kind == GapKind::NotInstrumented), "its line has a count; it is this code that has none");
    // it is called by its own generic copy, like the others
    assert_eq!(c1.callers.iter().map(|c| c.symbol.as_str()).collect::<Vec<_>>(), ["constructs::each_sample::<constructs::closures::{closure#1}>"]);
    // its symbol is attributed, so it is no longer among the image's
    // unattributed symbols
    let sym = ev.symbols.iter().find(|s| s.name == "constructs::closures::{closure#1}").expect("closure#1's code is in the image");
    assert_eq!(sym.function_id, Some(c1.id));
    assert_eq!(c1.symbol_id, Some(sym.id));
    // counted apart: not executed, not unexecuted
    assert_eq!(ev.summary.functions_unrecorded, ev.functions.iter().filter(|f| f.unrecorded_within.is_some()).count());
    assert!(ev.summary.functions_unrecorded >= 1);
    let none = Resolutions { schema: RESOLUTIONS_SCHEMA.into(), entries: Vec::new() };
    assert!(
        !butterfly_ingest::worklist::compute(&ev, &none).iter().any(|i| i.function == c1.name),
        "its count of 0 is \"not counted\": it is not a never-executed test obligation"
    );
    // The closures that do have records are not touched.
    assert!(c0.unrecorded_within.is_none() && c2.unrecorded_within.is_none());
    ev.validate().expect("the evidence is well-formed");

    // The inventory counts the three closures, and no `||` operator: the
    // `move ||` is a closure.
    let inv = &func(&ev, "constructs::closures").constructs;
    assert_eq!(inv.get("closure"), Some(&3), "{inv:?}");
    assert_eq!(inv.get("||"), None, "{inv:?}");
}

/// `rust.macro-rules`. `within!` expands to `if $v >= $lo && $v <= $hi`,
/// used twice in `in_envelope`. rustc gives the expanded condition no
/// branch region, so its outcomes are not in the source coverage at all;
/// and the image's line table puts the expanded code on the macro's
/// definition, not on the two lines that use it. butterfly shows what that
/// leaves: covered call-site lines, and object code on a line outside the
/// function with no count, projected neither covered nor uncovered.
#[test]
fn a_macro_expanded_decision_lands_on_the_definition_with_no_count() {
    let Some(root) = fixture() else { return };
    let ev = ingest(&root);
    let f = func(&ev, "constructs::in_envelope");
    assert!(f.branches.is_empty(), "rustc now emits branch regions inside a macro expansion ({:?}); rust.macro-rules can say more", f.branches);
    let (use_a, use_s) = (line_of(&root, "within!(alt,"), line_of(&root, "within!(speed,"));
    let file = &ev.files[f.file_id];
    for l in [use_a, use_s] {
        assert!(file.lines[(l - 1) as usize].count.is_some_and(|c| c > 0), "line {l} is covered");
    }

    let def = line_of(&root, "if $v >= $lo && $v <= $hi");
    assert!(def < f.line_start, "the definition is outside in_envelope");
    let on_def = projected_on(f, def);
    assert!(!on_def.is_empty(), "the expanded comparisons are attributed to the macro's definition");
    assert!(on_def.iter().all(|p| *p == Status::Unknown), "a line with no count projects nothing: {on_def:?}");
    let gaps = &f.object.as_ref().unwrap().gaps;
    assert!(gaps.iter().any(|g| g.kind == GapKind::NotInstrumented && g.line == Some(def)), "the definition line has no coverage count");
    assert!(
        gaps.iter().any(|g| g.kind == GapKind::ExtraObjectBranch && g.line == Some(def)),
        "both expansions' conditional branches are on the definition line, with no source decision to account for them"
    );
    // Nothing on the call-site lines carries the decision.
    assert!(!gaps.iter().any(|g| g.kind == GapKind::ExtraObjectBranch && (g.line == Some(use_a) || g.line == Some(use_s))));
}

/// The evidence ingest writes for the generic's two copies is evidence the
/// rest of butterfly accepts. Each copy's line figures come from its own
/// records, and the derivation check used to derive them from the file's
/// summed lines instead, so `check`, `report` and the other commands refused
/// this example's evidence outright (#276).
#[test]
fn the_evidence_with_per_copy_figures_passes_the_derivation_check() {
    let Some(root) = fixture() else { return };
    let ev = ingest(&root);
    ev.validate().expect("valid");
    ev.check_summary_derivation().expect("the per-copy figures are bound to their own counts and to the file");
    let copies: Vec<&Function> = ev.functions.iter().filter(|f| f.name.starts_with("constructs::limit::<")).collect();
    assert!(copies.len() >= 2, "the generic's copies: {:?}", copies.iter().map(|f| &f.name).collect::<Vec<_>>());
    assert!(copies.iter().all(|f| f.own_lines.is_some()), "each copy carries its own line counts");
}
