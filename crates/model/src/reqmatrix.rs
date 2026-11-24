// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The requirements × tests matrix: A-7 objectives 3 and 4 (#42).
//!
//! butterfly already holds requirement → code (the `@satisfies` tags) and,
//! since #41, test → code. Joining them answers the question a
//! certification authority asks first: for every requirement, what
//! implements it, what exercises that implementation, and how much of it did
//! those tests actually reach.
//!
//! The orphans matter more than the filled rows, and in three directions:
//! a requirement with no code (not implemented, or tagged where the scraper
//! cannot see), a requirement with code but no test (implemented,
//! unverified), and code with no requirement — the A-7 obj 9 question in a
//! different guise, and the one that finds extraneous functionality.
//!
//! Three honesty rules. A requirement's coverage is derived from the **tests
//! that touch its own code** — never from aggregate coverage of those lines,
//! because §6.4.4 asks what the requirements-based tests exercised, and
//! aggregate counts cannot say.
//!
//! **A line-level tag scopes to its lines.** `@satisfies` on one line inside
//! a ten-line function claims those lines, not the function. Collapsing that
//! to the function would list every test that entered the function anywhere
//! and count every line of it — turning an implemented-but-unverified
//! requirement into an apparently verified one, which is the single worst
//! error this table can make (review of #42). A function-level tag scopes to
//! the whole function, as it should. When the evidence carries no
//! per-test data at all the matrix falls back to aggregate numbers and
//! `per_test_data` says so, so a reader cannot mistake the proxy for the
//! real thing. And a requirement nobody tagged is only knowable from a
//! supplied universe (`ingest --requirements`); without one, the no-code
//! orphan list is empty because absence cannot be inferred from tags alone.

use crate::Evidence;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// One requirement's row.
#[derive(Debug, Clone, Serialize)]
pub struct ReqRow {
    pub requirement: String,
    /// Implementing functions, by id.
    pub functions: Vec<usize>,
    /// Tests that covered a line this requirement claims, by id -- not
    /// merely tests that entered the function. Empty means unverified when
    /// `Matrix::per_test_data`, and means nothing otherwise.
    pub tests: Vec<usize>,
    /// Instrumented lines this requirement claims: the whole function for a
    /// function-level tag, only the tagged lines for a line-level one.
    pub lines_total: usize,
    /// Of those, lines the attributed tests covered (or, with no per-test
    /// data, the aggregate's covered lines).
    pub lines_covered: usize,
}

/// The whole table, with its orphans made explicit.
#[derive(Debug, Clone, Serialize)]
pub struct Matrix {
    pub rows: Vec<ReqRow>,
    /// In the supplied universe, tagged nowhere. Empty when no universe was
    /// supplied -- absence cannot be inferred from tags alone.
    pub requirements_without_code: Vec<String>,
    /// Tagged, but no attributed test enters any implementing function.
    /// Meaningful only when `per_test_data`.
    pub requirements_without_tests: Vec<String>,
    /// Functions carrying no requirement tag at all, by id.
    pub functions_without_requirement: Vec<usize>,
    /// Whether coverage columns come from named tests or from the aggregate.
    pub per_test_data: bool,
}

/// What a requirement claims inside one function.
#[derive(Debug, Clone)]
enum Scope {
    /// A function-level tag: the whole function.
    Whole,
    /// A line-level tag: only these lines.
    Lines(BTreeSet<u32>),
}

impl Scope {
    /// Widen to the whole function -- a function-level tag subsumes any
    /// line-level one for the same requirement.
    fn widen(&mut self) {
        *self = Scope::Whole;
    }
}

/// The instrumented lines a scope actually claims, inside the function's own
/// range. A line the compiler emitted no code for is not coverable and must
/// not sit in a denominator. When a function carries `own_lines` (a
/// monomorphised instantiation sharing its source range with another), its own
/// instrumentation is consulted rather than the file's aggregate (#167).
fn scoped_lines(ev: &Evidence, f: usize, scope: &Scope) -> Vec<u32> {
    let func = &ev.functions[f];
    let file = &ev.files[func.file_id];
    let instrumented = |ln: u32| {
        if ln < 1 {
            return false;
        }
        if let Some(own) = &func.own_lines {
            return ln >= func.line_start && own.get((ln - func.line_start) as usize).is_some_and(|c| c.is_some());
        }
        file.lines.get(ln as usize - 1).map(|l| l.count.is_some()).unwrap_or(false)
    };
    match scope {
        // clamped to the file even though validate() already refuses a range
        // outside it: a consumer that trusts its input is one bad document
        // away from a multi-billion-iteration loop
        Scope::Whole => (func.line_start.max(1)..=func.line_end.min(file.text.len() as u32)).filter(|&ln| instrumented(ln)).collect(),
        // clipped to the function: a tag outside it is not this function's
        Scope::Lines(s) => s.iter().copied().filter(|&ln| ln >= func.line_start && ln <= func.line_end && instrumented(ln)).collect(),
    }
}

/// Whether `ln` is covered in `func`'s aggregate counts: from `own_lines` when
/// the function carries per-instantiation counts, else from the file's lines.
fn aggregate_line_covered(ev: &Evidence, f: usize, ln: u32) -> bool {
    let func = &ev.functions[f];
    if let Some(own) = &func.own_lines {
        return ln >= func.line_start && own.get((ln - func.line_start) as usize).and_then(|c| *c).unwrap_or(0) > 0;
    }
    ev.files[func.file_id].lines.get(ln as usize - 1).and_then(|l| l.count).unwrap_or(0) > 0
}

/// Build the matrix.
pub fn compute(ev: &Evidence) -> Matrix {
    let per_test_data = !ev.tests.is_empty();

    // requirement -> function -> what it claims there. A function-level tag
    // takes the whole function; a line-level tag takes only its lines.
    let mut impls: BTreeMap<&str, BTreeMap<usize, Scope>> = BTreeMap::new();
    for f in &ev.functions {
        for r in &f.requirements {
            impls.entry(r).or_default().entry(f.id).and_modify(Scope::widen).or_insert(Scope::Whole);
        }
        for (&line, rs) in &f.line_requirements {
            for r in rs {
                match impls.entry(r).or_default().entry(f.id).or_insert_with(|| Scope::Lines(BTreeSet::new())) {
                    Scope::Whole => {}
                    Scope::Lines(set) => {
                        set.insert(line);
                    }
                }
            }
        }
    }

    let mut rows = Vec::new();
    let mut without_tests = Vec::new();
    for (req, per_fn) in &impls {
        // A test verifies this requirement only if it covered a line the
        // requirement actually claims. Entering the function elsewhere is
        // not verification of it.
        //
        // Deduplicate by (file_id, line) across functions claiming this
        // requirement so monomorphised instantiations sharing one source range
        // do not double-count the same source lines. In aggregate mode a
        // shared line is covered only when every claiming instantiation
        // covered it (fail closed when one copy missed a branch).
        let mut line_covered: BTreeMap<(usize, u32), bool> = BTreeMap::new();
        for (&f, scope) in per_fn {
            let file_id = ev.functions[f].file_id;
            for ln in scoped_lines(ev, f, scope) {
                let cov = aggregate_line_covered(ev, f, ln);
                line_covered.entry((file_id, ln)).and_modify(|all| *all &= cov).or_insert(cov);
            }
        }
        let mut tests: BTreeSet<usize> = BTreeSet::new();
        let (mut lines_total, mut lines_covered) = (0usize, 0usize);
        for (&(file_id, ln), &agg_cov) in &line_covered {
            lines_total += 1;
            if per_test_data {
                let hitters: Vec<usize> =
                    ev.tests.iter().filter(|t| t.lines.get(&file_id).map(|rs| rs.iter().any(|&(a, b)| a <= ln && ln <= b)).unwrap_or(false)).map(|t| t.id).collect();
                if !hitters.is_empty() {
                    lines_covered += 1;
                    tests.extend(hitters);
                }
            } else if agg_cov {
                // no test identity: fall back to the aggregate, which the
                // caller is told is a proxy
                lines_covered += 1;
            }
        }
        if per_test_data && tests.is_empty() {
            without_tests.push((*req).to_string());
        }
        rows.push(ReqRow { requirement: (*req).to_string(), functions: per_fn.keys().copied().collect(), tests: tests.into_iter().collect(), lines_total, lines_covered });
    }

    let requirements_without_code = ev.requirements_universe.iter().filter(|r| !impls.contains_key(r.as_str())).cloned().collect();
    let functions_without_requirement = ev.functions.iter().filter(|f| f.requirements.is_empty() && f.line_requirements.is_empty()).map(|f| f.id).collect();

    Matrix { rows, requirements_without_code, requirements_without_tests: without_tests, functions_without_requirement, per_test_data }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Function, LineCov, SourceFile, SCHEMA};

    #[test]
    fn monomorphised_copies_do_not_double_count_and_respect_own_lines() {
        let ev = Evidence {
            schema: SCHEMA.into(),
            files: vec![SourceFile {
                id: 0,
                path: "g.rs".into(),
                display: "g.rs".into(),
                text: vec!["fn g<T>() {".into(), "  a();".into(), "  b();".into()],
                lines: vec![LineCov { count: Some(2) }, LineCov { count: Some(2) }, LineCov { count: Some(1) }],
                function_ids: vec![0, 1],
                lines_total: 3,
                lines_covered: 3,
                ..Default::default()
            }],
            functions: vec![
                Function {
                    id: 0,
                    name: "g::<i32>".into(),
                    file_id: 0,
                    line_start: 1,
                    line_end: 3,
                    requirements: vec!["REQ-G".into()],
                    lines_total: 3,
                    lines_covered: 2,
                    own_lines: Some(vec![Some(1), Some(1), Some(0)]),
                    ..Default::default()
                },
                Function {
                    id: 1,
                    name: "g::<u64>".into(),
                    file_id: 0,
                    line_start: 1,
                    line_end: 3,
                    requirements: vec!["REQ-G".into()],
                    lines_total: 3,
                    lines_covered: 3,
                    own_lines: Some(vec![Some(1), Some(1), Some(1)]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let m = compute(&ev);
        assert_eq!(m.rows.len(), 1);
        let row = &m.rows[0];
        assert_eq!(row.requirement, "REQ-G");
        assert_eq!(row.functions, [0, 1]);
        assert_eq!(row.lines_total, 3, "shared source lines must not be double-counted across monomorphisations");
        assert_eq!(row.lines_covered, 2, "line 3 missed in g::<i32> must not read as covered");
    }
}
