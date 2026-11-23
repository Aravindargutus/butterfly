// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Which test exercised what (#41).
//!
//! Coverage is otherwise aggregate: one merged profile, one count per line,
//! no way to say what ran a line. DO-178C §6.4.4 defines structural coverage
//! analysis as determining what the *requirements-based test procedures* did
//! not exercise, so evidence that cannot name the tests behind its coverage
//! is asking a reviewer to take that on trust.
//!
//! The merged export stays the source of truth for every count. These
//! per-test exports add attribution on top and never change a number -- and
//! an export that disagrees with the merged coverage is refused whole, with
//! the contradiction named in a caveat. It describes a different run or a
//! different build, and attaching the parts that happen to look consistent
//! would manufacture provenance for coverage that does not exist.
//!
//! Vector attribution (the remaining half of #41, split out as #48) is a
//! different kind of join from function and line attribution. Those match by
//! name; a decision's vectors are matched by their *content* -- the values
//! each condition took and the result -- because a per-test export's vectors
//! are its own list, produced independently of the merged export's numbering.
//! One test can produce several vectors for one decision (a loop body taking
//! different paths on different iterations); several tests can produce the
//! identical vector. So a merged `TestVector` accumulates a *set* of test
//! ids, and a vector no per-test export happened to reproduce keeps an empty
//! one -- which a consumer must read as "could not be determined", never as
//! "no test produced this", since only an *offered* export can attribute at
//! all.

use crate::coverage;
use butterfly_model::{Evidence, TestRun};
use std::collections::BTreeMap;
use std::path::Path;
/// Commit staged matches from one accepted export: each names a test that
/// reproduced one merged vector, and gets appended once. Applying the same
/// match twice must not duplicate the id -- a test can genuinely reproduce a
/// vector this call has already recorded it against (a loop calling the
/// same decision more than once), and that is not a second reviewer.
pub fn apply_vector_matches(ev: &mut Evidence, id: usize, matches: &[VectorMatch]) {
    for m in matches {
        let v = &mut ev.functions[m.function].decisions[m.decision].vectors[m.vector];
        if !v.tests.contains(&id) {
            v.tests.push(id);
            v.tests.sort_unstable();
        }
    }
}

/// One test's vector, staged: which function, which of its decisions (by
/// position in `Function::decisions`), and which of that decision's already-
/// known vectors (by position in `Decision::vectors`) this test reproduced.
///
/// Staged rather than written straight into `ev`: `attribute` may still
/// reject the whole export after this is computed (a later contradiction in
/// the same export), and a rejected export must leave nothing behind --
/// including a test id quietly left on a vector it does not actually vouch
/// for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VectorMatch {
    pub function: usize,
    pub decision: usize,
    pub vector: usize,
}

/// Turn the covered lines of one file into inclusive ranges.
///
/// A test walks contiguous regions, so ranges keep a project with many tests
/// to a readable evidence file rather than one entry per line per test.
fn ranges(lines: impl Iterator<Item = u32>) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for l in lines {
        match out.last_mut() {
            Some(r) if r.1 + 1 == l => r.1 = l,
            _ => out.push((l, l)),
        }
    }
    out
}

/// Attribute one per-test export onto evidence whose ids are already final.
///
/// Matching is by name, not by position: the per-test export is a separate
/// llvm-cov run and its arrays need not be in the same order.
///
/// The aggregate is authoritative, and this checks that rather than assuming
/// it. An export can name the right files and the right functions and still
/// describe a different run -- a function the aggregate never executed, a
/// line the aggregate reports uncovered -- and attaching that to the evidence
/// would manufacture provenance for coverage that does not exist. Any such
/// contradiction rejects the **whole export**: one impossible claim is enough
/// to say this export is not of this run, and keeping the rest of it would be
/// keeping the parts that happened to be consistent (review of #41).
pub fn attribute(ev: &Evidence, id: usize, name: &str, path: &Path, text: &str) -> Result<(TestRun, Vec<VectorMatch>), String> {
    let cov = coverage::parse(text).map_err(|e| format!("{e}"))?;

    let file_of: BTreeMap<&str, usize> = ev.files.iter().enumerate().map(|(i, f)| (f.path.as_str(), i)).collect();
    // Key functions by (file_path, demangled_name, line_start) so same-named
    // static twins in different directories (`alpha/dup.c:tally` vs
    // `beta/dup.c:tally`, #249) stay distinct. Keep a unique-name fallback for
    // functions whose per-test record lacks a body region.
    let mut fn_by_place: BTreeMap<(&str, &str, u32), usize> = BTreeMap::new();
    let mut fn_by_name: BTreeMap<&str, Option<usize>> = BTreeMap::new();
    for (i, f) in ev.functions.iter().enumerate() {
        let fpath = ev.files.get(f.file_id).map(|fl| fl.path.as_str()).unwrap_or("");
        fn_by_place.insert((fpath, f.name.as_str(), f.line_start), i);
        fn_by_name.entry(f.name.as_str()).and_modify(|slot| *slot = None).or_insert(Some(i));
    }

    let mut lines: BTreeMap<usize, Vec<(u32, u32)>> = BTreeMap::new();
    let mut unknown_files = 0usize;
    let mut contradictions: Vec<String> = Vec::new();

    for cf in &cov.files {
        let Some(&fid) = file_of.get(cf.filename.as_str()) else {
            unknown_files += 1;
            continue;
        };
        let n = ev.files[fid].text.len();
        let counts = coverage::line_counts(cf, n);
        let mut hit: Vec<u32> = Vec::new();
        for (i, c) in counts.iter().enumerate() {
            if !matches!(c, Some(x) if *x > 0) {
                continue;
            }
            let line = i as u32 + 1;
            // the aggregate is the merge of every run, so a line one run
            // executed cannot be uncovered in it
            let agg = ev.files[fid].lines.get(i).and_then(|l| l.count).unwrap_or(0);
            let run = match c {
                Some(x) => *x,
                None => 0,
            };
            if agg == 0 && contradictions.len() < 4 {
                contradictions.push(format!("{}:{line} is covered here but uncovered in the merged export", ev.files[fid].display));
            } else if run > agg && contradictions.len() < 4 {
                // the aggregate is a sum over every run, so one run cannot
                // have executed a line more often than all of them together
                contradictions.push(format!("{}:{line} runs {run}x here but {agg}x in the merged export", ev.files[fid].display));
            }
            hit.push(line);
        }
        let r = ranges(hit.into_iter());
        if !r.is_empty() {
            lines.insert(fid, r);
        }
    }

    // A function is "entered by this test" when the export says its count is
    // non-zero. Names are matched against the merged evidence, which already
    // carries llvm-cov's `file.c:name` qualifier for internal linkage, so two
    // same-named statics stay apart here as they do everywhere else. The
    // export names a C++ or Rust function by its mangled symbol and the
    // evidence by the demangled one, so the export's name is demangled the
    // way ingest demangled it; looking up `_Z1fi` among `f(int)` rejected
    // every C++ and Rust test export (#325 B4).
    let mut functions: Vec<usize> = Vec::new();
    let mut unknown_fns = 0usize;
    let mut vector_matches: Vec<VectorMatch> = Vec::new();
    for cfn in &cov.functions {
        if cfn.count == 0 {
            continue;
        }
        let dem = crate::demangle(&cfn.name);
        let found = cfn
            .regions
            .iter()
            .find(|r| r.kind == coverage::KIND_CODE)
            .and_then(|body| cfn.filenames.get(body.file_id).and_then(|p| fn_by_place.get(&(p.as_str(), dem.as_str(), body.line_start)).copied()))
            .or_else(|| fn_by_name.get(dem.as_str()).and_then(|o| *o));
        match found {
            Some(i) => {
                let agg = ev.functions[i].count;
                if agg == 0 && contradictions.len() < 4 {
                    contradictions.push(format!("`{}` is executed here but never executed in the merged export", ev.functions[i].name));
                } else if cfn.count > agg && contradictions.len() < 4 {
                    contradictions.push(format!("`{}` runs {}x here but only {agg}x in the merged export", ev.functions[i].name, cfn.count));
                }
                functions.push(i);
                vector_matches.extend(vectors_for(cfn, i, &ev.functions[i].decisions, &mut contradictions));
            }
            None => unknown_fns += 1,
        }
    }
    functions.sort_unstable();
    functions.dedup();

    if !contradictions.is_empty() {
        return Err(format!("this export contradicts the merged coverage and describes a different run, so none of it is attributed: {}", contradictions.join("; ")));
    }
    if unknown_files > 0 || unknown_fns > 0 {
        return Err(format!(
            "{unknown_files} file(s) and {unknown_fns} executed function(s) in this export are not in the merged coverage. \
             A per-test export from a different build cannot be attributed, and its coverage is not counted here."
        ));
    }
    Ok((TestRun { id, name: name.to_string(), path: path.display().to_string(), functions, lines }, vector_matches))
}

/// This function's MC/DC vectors, matched onto the merged decisions already
/// built for it (`fid`). Matching is by content -- the values each condition
/// took and the result -- not by position: a per-test export's vectors are
/// its own list, unrelated to how the merged export happened to number them.
///
/// Only an *executed* vector on both sides is a real outcome; an unexecuted
/// slot is a placeholder MC/DC keeps for a combination nothing produced, and
/// two placeholders being equal proves nothing about this test.
fn vectors_for(cfn: &coverage::CovFunction, fid: usize, decisions: &[butterfly_model::Decision], contradictions: &mut Vec<String>) -> Vec<VectorMatch> {
    let Some(body) = cfn.regions.iter().find(|r| r.kind == coverage::KIND_CODE) else { return Vec::new() };
    let mut out = Vec::new();
    // placed as the merged decisions were, so one inside a macro is found at
    // the macro's use (#269), and multiple decisions sharing a placed site get
    // the same deterministic per-line ordinal as ingest assigned them.
    let placed = coverage::place_decisions(cfn, &coverage::decision_files(cfn), body.file_id);
    let mut items: Vec<(&coverage::RawMcdc, coverage::PlacedDecision, Option<u32>)> = cfn.mcdc.iter().zip(placed).filter_map(|(m, p)| Some((m, p?, None))).collect();
    items.sort_by_key(|(m, p, _)| (p.line_start, p.col_start, m.line_start, m.col_start, m.line_end, m.col_end));
    let mut site_counts: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for (_, p, _) in &items {
        *site_counts.entry((p.line_start, p.col_start)).or_default() += 1;
    }
    let mut next_ord: BTreeMap<u32, u32> = BTreeMap::new();
    for (_, p, ord) in &mut items {
        if site_counts.get(&(p.line_start, p.col_start)).copied().unwrap_or(0) > 1 {
            let n = next_ord.entry(p.line_start).or_insert(0);
            *ord = Some(*n);
            *n += 1;
        }
    }
    for (m, p, ord) in items {
        let Some((di, d)) = decisions.iter().enumerate().find(|(_, d)| {
            (d.line, d.col, d.line_end, d.col_end) == (p.line_start, p.col_start, p.line_end, p.col_end)
                && d.decision_ordinal == ord
                && d.conditions.len() == p.conditions.len().max(m.condition_covered.len())
        }) else {
            continue;
        };
        // A merge that carries no vectors for this decision cannot be asked
        // about one -- the LLVM 18 layout exports none at all -- so there is
        // nothing to contradict and nothing to attribute. Staying silent here
        // is the conservative answer, not a missed check.
        if d.vectors.is_empty() {
            continue;
        }
        for v in m.vectors.iter().filter(|v| v.executed) {
            let matched: Vec<usize> = d.vectors.iter().enumerate().filter(|(_, mv)| mv.executed && mv.values == v.conditions && mv.result == v.result).map(|(vi, _)| vi).collect();
            if matched.is_empty() {
                // The aggregate is the merge of every run, so a vector one run
                // executed must appear executed in it. Absent -- or present and
                // marked unexecuted -- this export describes a different merge,
                // and saying so is the same rule `attribute` already applies to
                // line and function counts. Left unchecked it would put a test's
                // name on an MC/DC proof it did not produce (review of #48).
                if contradictions.len() < 4 {
                    let shape: Vec<String> = v
                        .conditions
                        .iter()
                        .map(|c| match c {
                            Some(true) => "T".into(),
                            Some(false) => "F".into(),
                            None => "-".to_string(),
                        })
                        .collect();
                    contradictions.push(format!(
                        "`{}` L{}: this export executed the condition vector [{}] -> {}, which the merged export does not record as executed",
                        cfn.name,
                        m.line_start,
                        shape.join(""),
                        if v.result { "true" } else { "false" }
                    ));
                }
                continue;
            }
            out.extend(matched.into_iter().map(|vi| VectorMatch { function: fid, decision: di, vector: vi }));
        }
    }
    out
}

/// The name a test is known by: the export's file stem, minus the noise a
/// profile-per-test workflow tends to leave on it.
pub fn name_for(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    stem.trim_end_matches(".coverage").trim_end_matches("-coverage").trim_end_matches(".cov").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covered_lines_become_ranges() {
        assert_eq!(ranges([1u32, 2, 3, 7, 8, 20].into_iter()), vec![(1, 3), (7, 8), (20, 20)]);
        assert_eq!(ranges(std::iter::empty()), Vec::new());
        assert_eq!(ranges([5u32].into_iter()), vec![(5, 5)]);
    }

    #[test]
    fn a_name_loses_the_workflow_noise() {
        assert_eq!(name_for(Path::new("/p/nav_smoke.coverage.json")), "nav_smoke");
        assert_eq!(name_for(Path::new("/p/alt-limits-coverage.json")), "alt-limits");
        assert_eq!(name_for(Path::new("/p/plain.json")), "plain");
    }

    // ---------------------------------------------------------------------
    // vectors_for (#48). Apple clang (this machine's toolchain) still emits
    // the LLVM-18 MC/DC layout, which carries no test vectors at all -- not
    // even in the merged export -- so real compiler output cannot exercise
    // this matching logic here. Built by hand instead, in the exact LLVM
    // 19+ array layout `coverage::parse` already reads (proven against real
    // output by `coverage::tests::llvm18_mcdc_record_layout_is_read`), so
    // this tests the matching itself without depending on which toolchain
    // happens to be installed.

    use butterfly_model::{Condition, Decision};

    /// One function `f`, one KIND_CODE region (so `vectors_for` can resolve
    /// `body.file_id`), one MC/DC decision at line 2 with the given vectors.
    fn cov_fn_with_vectors(vectors_json: &str) -> coverage::CovFunction {
        let text = format!(
            r#"{{"version":"2.0.1","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":"d.c","segments":[],"branches":[],"summary":{{}}}}],
            "functions":[{{"name":"f","count":5,"filenames":["d.c"],
              "regions":[[1,1,3,2,5,0,0,0]],
              "branches":[],
              "mcdc_records":[[2,7,2,15,1,2,0,0,5,[true,true],{vectors_json}]]}}]}}]}}"#
        );
        coverage::parse(&text).unwrap().functions.into_iter().next().unwrap()
    }

    /// The merged decision `vectors_for` matches against: same span as the
    /// mcdc_record above (line 2, col 7..15), with two conditions and three
    /// already-built vectors -- the union a real merge would have produced
    /// from three tests.
    fn merged_decisions() -> Vec<Decision> {
        let cond = |i| Condition { index: i, line: 2, col: 7, true_count: 1, false_count: 1, independence_covered: true, pair: None };
        vec![Decision {
            line: 2,
            col: 7,
            line_end: 2,
            col_end: 15,
            true_count: 1,
            false_count: 2,
            conditions: vec![cond(0), cond(1)],
            vectors: vec![
                // V0: a false, short-circuit -- b masked
                butterfly_model::TestVector { values: vec![Some(false), None], result: false, executed: true, tests: Vec::new() },
                // V1: a true, b true
                butterfly_model::TestVector { values: vec![Some(true), Some(true)], result: true, executed: true, tests: Vec::new() },
                // V2: a true, b false
                butterfly_model::TestVector { values: vec![Some(true), Some(false)], result: false, executed: true, tests: Vec::new() },
            ],
            mcdc_covered: true,
            decision_ordinal: None,
        }]
    }

    /// One test's export reproducing exactly one of the three merged
    /// vectors is matched to that vector, and only that one.
    #[test]
    fn a_single_reproduced_vector_matches_by_content() {
        let decisions = merged_decisions();
        let cfn = cov_fn_with_vectors(r#"[{"conditions":[false,null],"executed":true,"result":false}]"#);
        let m = vectors_for(&cfn, 0, &decisions, &mut Vec::new());
        assert_eq!(m, vec![VectorMatch { function: 0, decision: 0, vector: 0 }], "matches V0 by content, not by being first");
    }

    /// Content match, not position: a per-test export whose only vector is
    /// V2 must land on index 2, not on whatever position it appears in the
    /// per-test export's own (irrelevant) ordering.
    #[test]
    fn matching_is_by_content_not_by_position() {
        let decisions = merged_decisions();
        let cfn = cov_fn_with_vectors(r#"[{"conditions":[true,false],"executed":true,"result":false}]"#);
        let m = vectors_for(&cfn, 7, &decisions, &mut Vec::new());
        assert_eq!(m, vec![VectorMatch { function: 7, decision: 0, vector: 2 }]);
    }

    /// One test producing several vectors for one decision -- a loop body
    /// taking different paths on different iterations -- attributes to
    /// every merged vector it reproduced, not just the first.
    #[test]
    fn one_test_can_match_more_than_one_vector() {
        let decisions = merged_decisions();
        let cfn = cov_fn_with_vectors(r#"[{"conditions":[false,null],"executed":true,"result":false},{"conditions":[true,true],"executed":true,"result":true}]"#);
        let mut m = vectors_for(&cfn, 0, &decisions, &mut Vec::new());
        m.sort_by_key(|v| v.vector);
        assert_eq!(m, vec![VectorMatch { function: 0, decision: 0, vector: 0 }, VectorMatch { function: 0, decision: 0, vector: 1 }]);
    }

    /// A vector that matches nothing in the merged decision -- content this
    /// particular merge never produced -- is silently unmatched, not an
    /// error: the caller applies whatever came back, and an empty result
    /// leaves that vector's `tests` exactly as unattributed as it already
    /// was, which is the correct "could not be determined" state.
    #[test]
    fn an_unreproduced_vector_matches_nothing() {
        let decisions = merged_decisions();
        let cfn = cov_fn_with_vectors(r#"[{"conditions":[false,true],"executed":true,"result":true}]"#);
        assert!(vectors_for(&cfn, 0, &decisions, &mut Vec::new()).is_empty());
    }

    /// An unexecuted vector on either side is not a real outcome and must
    /// not be treated as one: MC/DC keeps a placeholder for a combination
    /// nothing produced, and two placeholders agreeing proves nothing about
    /// this test.
    #[test]
    fn an_unexecuted_vector_never_matches() {
        let mut decisions = merged_decisions();
        decisions[0].vectors[0].executed = false; // V0 becomes a placeholder
        let cfn = cov_fn_with_vectors(r#"[{"conditions":[false,null],"executed":true,"result":false}]"#);
        assert!(vectors_for(&cfn, 0, &decisions, &mut Vec::new()).is_empty(), "an unexecuted merged slot must not match a real per-test vector");

        // and the reverse: the per-test side unexecuted, merged side real
        let decisions2 = merged_decisions();
        let cfn2 = cov_fn_with_vectors(r#"[{"conditions":[false,null],"executed":false,"result":false}]"#);
        assert!(vectors_for(&cfn2, 0, &decisions2, &mut Vec::new()).is_empty());
    }

    /// The merged export is the merge of every run, so a vector one run
    /// executed must appear executed in it. An export claiming otherwise
    /// describes a different merge -- and `attribute` already rejects a whole
    /// export for exactly this on line and function counts. Left unreported,
    /// a test's name would land on an MC/DC proof it did not produce (review
    /// of #48).
    #[test]
    fn a_vector_the_merge_never_executed_is_a_contradiction() {
        let mut decisions = merged_decisions();
        decisions[0].vectors[1].executed = false; // the merge says V1 never ran
                                                  // ...but this export says it did
        let cfn = cov_fn_with_vectors(r#"[{"conditions":[true,true],"executed":true,"result":true}]"#);
        let mut c = Vec::new();
        let m = vectors_for(&cfn, 0, &decisions, &mut c);
        assert!(m.is_empty(), "nothing may be attributed from a contradicting export: {m:?}");
        assert_eq!(c.len(), 1, "and the contradiction is named: {c:?}");
        assert!(c[0].contains("does not record as executed"), "{}", c[0]);
    }

    /// Content the merge has no record of at all, executed here: same rule.
    #[test]
    fn a_vector_absent_from_the_merge_is_a_contradiction() {
        let decisions = merged_decisions();
        let cfn = cov_fn_with_vectors(r#"[{"conditions":[false,true],"executed":true,"result":true}]"#);
        let mut c = Vec::new();
        assert!(vectors_for(&cfn, 0, &decisions, &mut c).is_empty());
        assert_eq!(c.len(), 1, "{c:?}");
    }

    /// The LLVM 18 layout exports no vectors at all, so a merge built from it
    /// carries none. There is then nothing to contradict and nothing to
    /// attribute -- silence, not a false accusation against every export.
    #[test]
    fn a_merge_with_no_vector_data_accuses_nothing() {
        let mut decisions = merged_decisions();
        decisions[0].vectors.clear();
        let cfn = cov_fn_with_vectors(r#"[{"conditions":[true,true],"executed":true,"result":true}]"#);
        let mut c = Vec::new();
        assert!(vectors_for(&cfn, 0, &decisions, &mut c).is_empty());
        assert!(c.is_empty(), "no vectors on the merged side is not a contradiction: {c:?}");
    }

    /// An unexecuted per-test vector is a placeholder, not a claim, so it is
    /// neither attributed nor treated as contradicting anything.
    #[test]
    fn an_unexecuted_per_test_vector_accuses_nothing() {
        let decisions = merged_decisions();
        let cfn = cov_fn_with_vectors(r#"[{"conditions":[false,true],"executed":false,"result":true}]"#);
        let mut c = Vec::new();
        assert!(vectors_for(&cfn, 0, &decisions, &mut c).is_empty());
        assert!(c.is_empty(), "{c:?}");
    }

    /// A decision at a different line/column is a different decision, even
    /// with identical condition content -- two `if (a && b)` in one
    /// function must not blur into each other.
    #[test]
    fn a_decision_at_a_different_span_does_not_match() {
        let mut decisions = merged_decisions();
        decisions[0].line = 99; // now nothing in `decisions` has span (2,7,2,15)
        let cfn = cov_fn_with_vectors(r#"[{"conditions":[false,null],"executed":true,"result":false}]"#);
        assert!(vectors_for(&cfn, 0, &decisions, &mut Vec::new()).is_empty());
    }
}
