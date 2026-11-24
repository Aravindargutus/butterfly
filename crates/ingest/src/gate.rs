// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly check`: the gate over the evidence (#43).
//!
//! butterfly produces evidence and then waits for a person to look at it. A
//! number nobody is accountable for drifts, and unresolved gaps in
//! particular are a reviewer's queue -- a queue with no ceiling grows. This
//! turns the numbers the viewer shows into something CI can refuse: absolute
//! floors, and "no worse than a prior evidence file".
//!
//! Wording is deliberate, and follows the review of #35: the line metric is
//! **line coverage**, a proxy for statement coverage and never presented as
//! it; branch outcomes are what decision coverage is measured on. The flags
//! are named for what is actually measured (`--min-lines`, `--min-outcomes`),
//! not for the objectives a reader might wish they satisfied.
//!
//! Fail closed: a threshold over records the evidence does not carry is a
//! breach, not a vacuous pass. A gate that passes because instrumentation
//! was silently absent is the worst outcome a gate can have.

use butterfly_model::{Evidence, Resolutions};
use serde::Serialize;
use std::collections::HashSet;

/// The absolute floors and ceilings. All optional: only what the caller
/// asked for is checked.
#[derive(Debug, Clone, Default)]
pub struct Thresholds {
    /// Ceiling on gaps with no recorded resolution.
    pub max_unresolved_gaps: Option<usize>,
    /// Floor on line coverage, percent.
    pub min_lines: Option<f64>,
    /// Floor on branch-outcome coverage, percent.
    pub min_outcomes: Option<f64>,
    /// Floor on MC/DC conditions with an independence pair or a recorded
    /// justification, percent.
    pub min_mcdc: Option<f64>,
}

/// One reason the gate fails.
#[derive(Debug, Clone, Serialize)]
pub struct Breach {
    /// Short machine-readable kind: `lines`, `outcomes`, `mcdc`,
    /// `unresolved-gaps`, `regression-*`, `no-records`.
    pub kind: String,
    pub message: String,
    /// The function it names, when it names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
}

fn breach(kind: &str, message: String) -> Breach {
    Breach { kind: kind.into(), message, function: None }
}

fn pct(covered: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * covered as f64 / total as f64
    }
}

/// Does `covered/total` meet a percentage floor?
///
/// A floor of 100 means exactly "everything": it is compared as a count, not
/// as a float, so rounding can neither grant nor deny the last item.
fn meets(covered: usize, total: usize, min: f64) -> bool {
    if min >= 100.0 {
        covered == total
    } else {
        pct(covered, total) >= min
    }
}

/// The resolution keys, for "is this gap/condition justified" lookups.
fn keys(res: &Resolutions) -> HashSet<&str> {
    res.entries.iter().map(|e| e.key.as_str()).collect()
}

/// Gaps with no recorded resolution, per function, worst first.
fn unresolved_gaps(ev: &Evidence, res: &Resolutions) -> Vec<(String, usize)> {
    let done = keys(res);
    let mut per: Vec<(String, usize)> = ev
        .functions
        .iter()
        .filter_map(|f| {
            let n = f.objects().flat_map(|o| o.gaps.iter()).filter(|g| !done.contains(g.key.as_str())).count();
            (n > 0).then(|| (f.name.clone(), n))
        })
        .collect();
    per.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    per
}

/// MC/DC conditions satisfied (independence pair, or justified) over total.
///
/// The key format is the one the viewer writes:
/// `cond:<fn_id>:<site>:<index>`.
fn mcdc_counts(ev: &Evidence, res: &Resolutions) -> (usize, usize) {
    let done = keys(res);
    let mut total = 0usize;
    let mut ok = 0usize;
    for f in &ev.functions {
        for d in &f.decisions {
            let site = d.site();
            for c in &d.conditions {
                total += 1;
                if c.independence_covered || done.contains(format!("cond:{}:{site}:{}", f.id, c.index).as_str()) {
                    ok += 1;
                }
            }
        }
    }
    (ok, total)
}

/// Check the absolute thresholds.
pub fn evaluate(ev: &Evidence, res: &Resolutions, th: &Thresholds) -> Vec<Breach> {
    let mut out = Vec::new();
    let s = &ev.summary;

    if let Some(min) = th.min_lines {
        if s.lines_total == 0 {
            out.push(breach("no-records", "a line-coverage floor was required, but the evidence carries no executable lines; the gate cannot attest what does not exist".into()));
        } else if !meets(s.lines_covered, s.lines_total, min) {
            out.push(breach(
                "lines",
                format!(
                    "line coverage is {:.1}% ({}/{} lines) against a floor of {min}%. This is line coverage, a proxy for statement coverage, not proof of it.",
                    pct(s.lines_covered, s.lines_total),
                    s.lines_covered,
                    s.lines_total
                ),
            ));
        }
    }
    if let Some(min) = th.min_outcomes {
        if s.branches_total == 0 {
            out.push(breach("no-records", "a branch-outcome floor was required, but the evidence carries no branch records; the gate cannot attest what does not exist".into()));
        } else if !meets(s.branches_covered, s.branches_total, min) {
            out.push(breach(
                "outcomes",
                format!(
                    "branch outcomes are {:.1}% ({}/{}) against a floor of {min}%: {} outcome(s) were never taken",
                    pct(s.branches_covered, s.branches_total),
                    s.branches_covered,
                    s.branches_total,
                    s.branches_total - s.branches_covered
                ),
            ));
        }
    }
    if let Some(min) = th.min_mcdc {
        let (ok, total) = mcdc_counts(ev, res);
        // MC/DC decisions that could not be placed are absent from `decisions`
        // and so from `mcdc_counts`' denominator. They are unproven
        // obligations, not proof of coverage: while any exists, the floor
        // cannot be attested at all, or a fully-covered remainder would let
        // `--min-mcdc 100` pass over records that were never accounted for
        // (#284). This is checked before the ratio so it holds at any floor.
        let unplaced = ev.summary.decisions_unplaced;
        if unplaced > 0 {
            out.push(breach(
                "mcdc-incomplete",
                format!(
                    "an MC/DC floor was required, but {unplaced} MC/DC decision record(s) could not be placed and are unproven (their file could not be established, #269); the floor cannot be attested while any obligation is unaccounted for — see the evidence caveats"
                ),
            ));
        }
        if total == 0 && unplaced == 0 {
            out.push(breach(
                "no-records",
                "an MC/DC floor was required, but the evidence carries no MC/DC records (built without -fcoverage-mcdc, or a toolchain that cannot emit them); the gate cannot attest what does not exist".into(),
            ));
        } else if total > 0 && !meets(ok, total, min) {
            out.push(breach(
                "mcdc",
                format!(
                    "MC/DC is {:.1}% ({ok}/{total} conditions proven or justified) against a floor of {min}%; the MC/DC tab of each failing decision says which test would close it",
                    pct(ok, total)
                ),
            ));
        }
    }
    if let Some(max) = th.max_unresolved_gaps {
        let per = unresolved_gaps(ev, res);
        let n: usize = per.iter().map(|(_, c)| c).sum();
        if n > max {
            let worst = per.iter().take(4).map(|(f, c)| format!("{f} ({c})")).collect::<Vec<_>>().join(", ");
            out.push(breach(
                "unresolved-gaps",
                format!("{n} unresolved gap(s) against a ceiling of {max} — worst: {worst}. Each needs a reviewer resolution (6.4.4.3) or a justification (6.4.4.2.b)."),
            ));
        }
    }
    out
}

/// Did `covered/total` fall below the prior `covered/total`?
///
/// Cross-multiplied counts, so float rounding can neither invent nor hide a
/// one-line regression. The zero-denominator case is defined explicitly and
/// fails closed: a prior that measured something against a current that
/// measures nothing is not "no regression", it is the instrumentation-loss
/// case this module exists to refuse -- both cross-products are zero there,
/// which is how the naive comparison passed it (review of #43).
fn ratio_fell(c: usize, t: usize, pc: usize, pt: usize) -> bool {
    if t == 0 {
        return pt > 0;
    }
    (c as u128) * (pt as u128) < (pc as u128) * (t as u128)
}

/// Nothing may be worse than a prior evidence file.
///
/// The comparison is against prior *evidence*, not the analysis baseline:
/// `butterfly-baseline.json` fingerprints compiler, flags and object code,
/// and deliberately carries no coverage numbers, so there is nothing in it
/// to regress against. Freeze an evidence file instead and hand it here.
pub fn regressions(ev: &Evidence, res: &Resolutions, prior: &Evidence, prior_res: &Resolutions) -> Vec<Breach> {
    let mut out = Vec::new();
    let (s, p) = (&ev.summary, &prior.summary);

    let worse = ratio_fell;
    if worse(s.lines_covered, s.lines_total, p.lines_covered, p.lines_total) {
        out.push(breach(
            "regression-lines",
            format!(
                "line coverage fell: {:.1}% ({}/{}) now, {:.1}% ({}/{}) in the prior evidence",
                pct(s.lines_covered, s.lines_total),
                s.lines_covered,
                s.lines_total,
                pct(p.lines_covered, p.lines_total),
                p.lines_covered,
                p.lines_total
            ),
        ));
    }
    if worse(s.branches_covered, s.branches_total, p.branches_covered, p.branches_total) {
        out.push(breach(
            "regression-outcomes",
            format!(
                "branch outcomes fell: {:.1}% ({}/{}) now, {:.1}% ({}/{}) in the prior evidence",
                pct(s.branches_covered, s.branches_total),
                s.branches_covered,
                s.branches_total,
                pct(p.branches_covered, p.branches_total),
                p.branches_covered,
                p.branches_total
            ),
        ));
    }
    let (ok, total) = mcdc_counts(ev, res);
    let (pok, ptotal) = mcdc_counts(prior, prior_res);
    let unplaced = ev.summary.decisions_unplaced;
    if unplaced > 0 && (ptotal > 0 || unplaced > prior.summary.decisions_unplaced) {
        out.push(breach(
            "mcdc-incomplete",
            format!(
                "{unplaced} MC/DC decision record(s) could not be placed and are unproven (their file could not be established, #269); non-regression cannot be attested while obligations are unaccounted for"
            ),
        ));
    }
    if worse(ok, total, pok, ptotal) {
        out.push(breach("regression-mcdc", format!("MC/DC fell: {ok}/{total} conditions proven or justified now, {pok}/{ptotal} in the prior evidence")));
    }
    let n: usize = unresolved_gaps(ev, res).iter().map(|(_, c)| c).sum();
    let pn: usize = unresolved_gaps(prior, prior_res).iter().map(|(_, c)| c).sum();
    if n > pn {
        out.push(breach("regression-gaps", format!("unresolved gaps rose from {pn} to {n}")));
    }

    // Per function, matched by (file, name) so same-named statics in different
    // directories do not overwrite each other (#249), falling back to unique
    // display path or unique bare name across relocated checkouts, and only
    // while the function's shape is comparable.
    fn file_of<'a>(e: &'a Evidence, f: &butterfly_model::Function) -> (&'a str, &'a str) {
        e.files.get(f.file_id).map(|fl| (fl.path.as_str(), fl.display.as_str())).unwrap_or(("", ""))
    }
    let mut prior_by_path: std::collections::HashMap<(&str, &str), Option<&butterfly_model::Function>> = std::collections::HashMap::new();
    let mut prior_by_disp: std::collections::HashMap<(&str, &str), Option<&butterfly_model::Function>> = std::collections::HashMap::new();
    let mut prior_by_name: std::collections::HashMap<&str, Option<&butterfly_model::Function>> = std::collections::HashMap::new();
    for pf in &prior.functions {
        let (path, disp) = file_of(prior, pf);
        prior_by_path.entry((path, pf.name.as_str())).and_modify(|s| *s = None).or_insert(Some(pf));
        prior_by_disp.entry((disp, pf.name.as_str())).and_modify(|s| *s = None).or_insert(Some(pf));
        prior_by_name.entry(pf.name.as_str()).and_modify(|s| *s = None).or_insert(Some(pf));
    }
    let mut cur_name_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for f in &ev.functions {
        *cur_name_counts.entry(f.name.as_str()).or_default() += 1;
    }
    for f in &ev.functions {
        let (path, disp) = file_of(ev, f);
        let pf = prior_by_path
            .get(&(path, f.name.as_str()))
            .and_then(|o| *o)
            .or_else(|| prior_by_disp.get(&(disp, f.name.as_str())).and_then(|o| *o))
            .or_else(|| (cur_name_counts.get(f.name.as_str()) == Some(&1)).then(|| prior_by_name.get(f.name.as_str()).and_then(|o| *o)).flatten());
        let Some(pf) = pf else { continue };
        if f.lines_total == pf.lines_total && f.lines_covered < pf.lines_covered {
            out.push(Breach {
                kind: "regression-function".into(),
                message: format!("`{}` fell from {}/{} to {}/{} covered lines", f.name, pf.lines_covered, pf.lines_total, f.lines_covered, f.lines_total),
                function: Some(f.name.clone()),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_floor_of_100_is_a_count_not_a_float() {
        assert!(meets(3, 3, 100.0));
        assert!(!meets(2, 3, 100.0), "one short of everything is not everything");
        assert!(meets(999_999, 1_000_000, 99.9), "fractional floors compare as percentages");
    }

    #[test]
    fn losing_every_measurement_is_a_regression_not_a_pass() {
        // prior 1/1, current 0/0: both cross-products are zero, which is
        // exactly how the naive comparison called this "no regression"
        assert!(ratio_fell(0, 0, 1, 1), "measurements vanished");
        assert!(!ratio_fell(0, 0, 0, 0), "nothing before, nothing now: no claim either way");
        assert!(!ratio_fell(5, 10, 0, 0), "measurements appearing is not a regression");
        assert!(!ratio_fell(5, 10, 5, 10), "unchanged is unchanged");
        assert!(ratio_fell(4, 10, 5, 10), "one item worse is worse");
        assert!(!ratio_fell(6, 10, 5, 10), "improvement is not a regression");
    }

    #[test]
    fn an_empty_denominator_is_zero_percent_not_a_pass() {
        // callers breach on total == 0 before ever comparing, but the
        // fallback must still be the failing direction
        assert_eq!(pct(0, 0), 0.0);
        assert!(!meets(0, 0, 50.0), "nothing measured cannot meet a fractional floor");
    }
}
