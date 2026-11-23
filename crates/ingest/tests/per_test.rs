// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Which test exercised what (#41).
//!
//! Runs only when the fixture has been built (`examples/c-per-test/build.sh`);
//! skipped, not failed, otherwise.
use butterfly_model::*;
use std::path::PathBuf;

fn root() -> Option<PathBuf> {
    let r = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-per-test");
    if r.join("build/coverage.json").exists() {
        Some(r)
    } else {
        butterfly_testsupport::fixture_missing("run examples/c-per-test/build.sh first");
        None
    }
}

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("pt-{tag}"))
}

fn ingest_with(root: &std::path::Path, tests: &[PathBuf]) -> Evidence {
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root.to_path_buf());
    o.tests = tests.to_vec();
    butterfly_ingest::ingest(&o).expect("ingest still succeeds")
}

fn ingest(with_tests: bool) -> Option<Evidence> {
    let r = root()?;
    let mut o = butterfly_ingest::discover(&r.join("build")).expect("discover");
    o.source_root = Some(r.clone());
    if with_tests {
        let d = r.join("build/tests");
        let mut v: Vec<PathBuf> = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
        v.sort();
        o.tests = v;
    }
    Some(butterfly_ingest::ingest(&o).expect("ingest"))
}

#[test]
fn every_test_is_recorded_and_named_for_its_export() {
    let Some(ev) = ingest(true) else { return };
    let mut names: Vec<&str> = ev.tests.iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["t_band_high", "t_band_low", "t_bank_shallow", "t_bank_steep_and_turning", "t_bank_steep_no_turn", "t_ceiling", "t_clamp", "t_faults"]);
    // ids are positional, and each names the export it came from
    for (i, t) in ev.tests.iter().enumerate() {
        assert_eq!(t.id, i);
        assert!(t.path.ends_with(".json"), "{}", t.path);
    }
    // and every export is hashed into the provenance like any other input
    assert_eq!(ev.provenance.inputs.iter().filter(|i| i.role == "coverage-per-test").count(), 8);
}

/// The point of the feature: a function knows which tests entered it, and
/// two tests that both enter one function are both recorded.
#[test]
fn a_function_lists_the_tests_that_entered_it() {
    let Some(ev) = ingest(true) else { return };
    let name_of = |ids: &[usize]| {
        let mut v: Vec<&str> = ids.iter().map(|&i| ev.tests[i].name.as_str()).collect();
        v.sort_unstable();
        v
    };
    let f = |n: &str| ev.functions.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("missing {n}"));

    // clamp_alt is reached by two different tests
    assert_eq!(name_of(&f("clamp_alt").tests), ["t_ceiling", "t_clamp"]);
    // fault_name by exactly one
    assert_eq!(name_of(&f("fault_name").tests), ["t_faults"]);
    // rate_ok is reached by three tests, split across its two independence
    // pairs (#48): t_bank_shallow short-circuits the first condition, the
    // other two differ only in the second.
    assert_eq!(name_of(&f("bank_angle_ok").tests), ["t_bank_shallow", "t_bank_steep_and_turning", "t_bank_steep_no_turn"]);
    // and a function no test reaches has none -- which is not the same as
    // "there were no per-test exports". rate_ok stays deliberately untested:
    // its LLR-LIM-002 traceability claim is wrong on purpose (#57), and
    // that scaffold needs rate_ok to genuinely have no test.
    let untested = f("rate_ok");
    assert_eq!(untested.count, 0, "the fixture leaves rate_ok unexercised on purpose");
    assert!(untested.tests.is_empty());
}

/// Two tests entering the same function still cover different lines of it.
/// Attribution that could not tell them apart would be attribution in name
/// only.
#[test]
fn tests_that_share_a_function_are_distinguished_by_line() {
    let Some(ev) = ingest(true) else { return };
    let t = |n: &str| ev.tests.iter().find(|t| t.name == n).unwrap();
    let lim = ev.files.iter().position(|f| f.display.ends_with("limits.c")).expect("limits.c");
    let lines = |name: &str| -> std::collections::BTreeSet<u32> { t(name).lines.get(&lim).into_iter().flatten().flat_map(|&(a, b)| a..=b).collect() };
    let (low, high) = (lines("t_clamp"), lines("t_ceiling"));
    assert!(!low.is_empty() && !high.is_empty());
    assert_ne!(low, high, "the low-edge and high-edge tests must not look identical");
}

/// The merged export stays the source of truth: adding per-test attribution
/// changes no count anywhere.
#[test]
fn attribution_changes_no_number() {
    let (Some(plain), Some(attributed)) = (ingest(false), ingest(true)) else { return };
    assert!(plain.tests.is_empty(), "no per-test exports, no tests");
    assert_eq!(plain.summary.lines_covered, attributed.summary.lines_covered);
    assert_eq!(plain.summary.lines_total, attributed.summary.lines_total);
    assert_eq!(plain.functions.len(), attributed.functions.len());
    for (a, b) in plain.functions.iter().zip(&attributed.functions) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.count, b.count, "{}", a.name);
        assert_eq!(a.lines_covered, b.lines_covered, "{}", a.name);
    }
}

/// An export describing a different build is refused outright, and the
/// refusal is on the record. Recording it with an empty attribution would
/// put a test in the evidence that verified nothing.
#[test]
fn an_export_from_another_build_is_refused_not_recorded_empty() {
    let Some(r) = root() else { return };
    let d = scratch("foreign");
    let foreign = d.join("t_foreign.json");
    std::fs::write(
        &foreign,
        r#"{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{"files":[{"filename":"/nowhere/ghost.c","segments":[],"branches":[],"summary":{}}],
           "functions":[{"name":"ghost","count":7,"regions":[],"branches":[],"filenames":["/nowhere/ghost.c"]}]}]}"#,
    )
    .unwrap();
    let ev = ingest_with(&r, &[foreign]);
    assert!(ev.tests.is_empty(), "nothing was attributed, so nothing is recorded");
    assert!(ev.provenance.caveats.iter().any(|c| c.contains("t_foreign") && c.contains("not in the merged coverage")), "and the refusal is reported: {:?}", ev.provenance.caveats);
    let _ = std::fs::remove_dir_all(&d);
}

/// The aggregate is authoritative. An export can name real files and real
/// functions and still describe a different run -- and attaching that would
/// manufacture provenance for coverage the merged export says never
/// happened.
#[test]
fn an_export_that_contradicts_the_aggregate_is_refused_whole() {
    let Some(r) = root() else { return };
    let d = scratch("contra");
    // rate_ok is real, is in the merged export, and its aggregate count is 0
    let src = r.join("src/limits.c").canonicalize().unwrap();
    let bad = d.join("t_impossible.json");
    std::fs::write(
        &bad,
        format!(
            r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[],"branches":[],"summary":{{}}}}],
               "functions":[{{"name":"rate_ok","count":7,"regions":[],"branches":[],"filenames":[{p:?}]}}]}}]}}"#,
            p = src.to_str().unwrap()
        ),
    )
    .unwrap();
    let ev = ingest_with(&r, &[bad]);
    assert!(ev.tests.is_empty(), "the whole export is refused, not the impossible part of it");
    assert!(ev.functions.iter().all(|f| f.tests.is_empty()), "and nothing carries its provenance");
    assert!(
        ev.provenance.caveats.iter().any(|c| c.contains("contradicts the merged coverage") && c.contains("rate_ok")),
        "the contradiction is named: {:?}",
        ev.provenance.caveats
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Ids are positions, so a refused export in the middle must not leave a
/// hole. valid, malformed, valid used to produce ids [0, 2] with len 2 --
/// an index into nothing, which `report` rendered without complaint.
#[test]
fn a_refused_export_leaves_no_hole_in_the_ids() {
    let Some(r) = root() else { return };
    let d = scratch("hole");
    let good = r.join("build/tests");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&good).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    v.sort();
    let broken = d.join("t_broken.json");
    std::fs::write(&broken, "{ this is not json").unwrap();

    let ev = ingest_with(&r, &[v[0].clone(), broken, v[1].clone()]);
    assert_eq!(ev.tests.len(), 2, "the two good exports are attributed");
    for (i, t) in ev.tests.iter().enumerate() {
        assert_eq!(t.id, i, "ids are positions with no gaps");
    }
    ev.validate().expect("and the document validates");
    assert!(ev.provenance.caveats.iter().any(|c| c.contains("t_broken")), "the broken one is accounted for: {:?}", ev.provenance.caveats);
    let _ = std::fs::remove_dir_all(&d);
}

/// A single run cannot have executed a line more often than every run
/// together: the aggregate is their sum. A per-test export claiming so is
/// the same impossible claim as an unexecuted function, at line grain.
#[test]
fn a_line_count_beyond_the_aggregate_is_refused() {
    let Some(r) = root() else { return };
    let d = scratch("linecount");
    let src = r.join("src/limits.c").canonicalize().unwrap();
    // limits.c line 7 (`if (v < lo) return lo;`) runs a handful of times in
    // the real suite; one million from a single test is not of this run.
    // Segment quads are [line, col, count, has_count]-shaped quintets in the
    // export: give the line an entry region with an absurd count.
    let bad = d.join("t_hot.json");
    std::fs::write(
        &bad,
        format!(
            r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[[7,1,1000000,true,true,false],[7,80,0,false,false,false]],"branches":[],"summary":{{}}}}],
               "functions":[]}}]}}"#,
            p = src.to_str().unwrap()
        ),
    )
    .unwrap();
    let ev = ingest_with(&r, &[bad]);
    assert!(ev.tests.is_empty(), "refused whole");
    assert!(
        ev.provenance.caveats.iter().any(|c| c.contains("contradicts the merged coverage") && c.contains("1000000x here")),
        "the impossible count is named: {:?}",
        ev.provenance.caveats
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Two exports named t_clamp must not become two indistinguishable tests.
#[test]
fn a_duplicate_test_name_is_disambiguated() {
    let Some(r) = root() else { return };
    let d = scratch("dupname");
    let orig = r.join("build/tests/t_clamp.json");
    let copy = d.join("t_clamp.json");
    std::fs::copy(&orig, &copy).unwrap();
    let ev = ingest_with(&r, &[orig, copy]);
    let names: Vec<&str> = ev.tests.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["t_clamp", "t_clamp (2)"], "same stem, two identities");
    ev.validate().expect("and the document validates");
    let _ = std::fs::remove_dir_all(&d);
}

/// validate() has to catch what a hand-edited or partially-written document
/// gets wrong, because the viewer indexes these arrays without checking.
#[test]
fn validate_rejects_a_broken_test_index() {
    let Some(ev) = ingest(true) else { return };
    ev.validate().expect("the real thing is valid");

    let mut bad = ev.clone();
    bad.tests[1].id = 7;
    assert!(bad.validate().unwrap_err().contains("position"), "an id that is not its position");

    let mut bad = ev.clone();
    bad.tests[0].functions.push(9999);
    assert!(bad.validate().unwrap_err().contains("out of range"), "a function id past the end");

    let mut bad = ev.clone();
    let fid = *bad.tests[0].lines.keys().next().unwrap();
    bad.tests[0].lines.insert(fid, vec![(5, 3)]);
    assert!(bad.validate().unwrap_err().contains("not a line range"), "a backwards range");

    let mut bad = ev.clone();
    bad.tests[0].lines.insert(fid, vec![(1, 1_000_000)]);
    assert!(bad.validate().unwrap_err().contains("past the file"), "a range past the end of the file");

    // the reverse index has to agree in both directions
    let mut bad = ev.clone();
    let f = bad.tests[0].functions[0];
    bad.functions[f].tests.clear();
    assert!(bad.validate().unwrap_err().contains("does not claim it"), "a test whose function disowns it");

    let mut bad = ev.clone();
    let f = bad.tests[0].functions[0];
    bad.tests[0].functions.retain(|&x| x != f);
    assert!(bad.validate().unwrap_err().contains("does not list it"), "a function claiming a test that disowns it");

    // a duplicated claim passes every containment check and is still wrong
    let mut bad = ev.clone();
    let f = bad.tests[0].functions[0];
    let t = bad.functions[f].tests[0];
    bad.functions[f].tests = vec![t, t];
    assert!(bad.validate().unwrap_err().contains("not sorted and unique"), "a function listing one test twice");
}
