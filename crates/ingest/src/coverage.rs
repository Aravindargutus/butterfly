// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Parser for `llvm-cov export -format=text` JSON (export format 2.x / 3.x).
//!
//! The arrays in that format are positional; this module turns them into
//! named structs and nothing else. No interpretation happens here.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct CovExport {
    pub version: String,
    pub files: Vec<CovFile>,
    pub functions: Vec<CovFunction>,
    /// Things the export could not say (older llvm-cov layouts).
    pub caveats: Vec<String>,
    /// Whether the export contained branch records at all, in any shape.
    ///
    /// Distinct from "the reader produced branches": a gcov block with other
    /// than two non-throw edges is a multiway or exception construct, not a
    /// two-way decision, and is deliberately left alone. Such an export has
    /// raw records but yields no branches -- which is not the same situation
    /// as `-b` having been omitted, and must not be diagnosed as if it were
    /// (review of #45).
    pub raw_branch_records: bool,
    /// Whether the producer recorded any decision, whether or not it recorded
    /// the outcomes.
    ///
    /// The distinction matters for exactly one question: gcov emits branch
    /// records only with `-b`, but a straight-line program has none either
    /// way. gcov *does* list a `conditions` entry per decision regardless
    /// when the build used `-fcondition-coverage`, so this separates "the
    /// flag is missing" from "this code branches nowhere" -- and without it,
    /// advising `-b` is a guess (review of #45). Without condition coverage
    /// ingest asks the object code instead (#325).
    pub decision_sites: bool,
}

#[derive(Debug, Clone)]
pub struct CovFile {
    pub filename: String,
    pub segments: Vec<Segment>,
    /// Per-line counts stated outright by the producer, when it states them.
    ///
    /// llvm-cov gives segments and the line count has to be derived from
    /// them; gcov gives the count for each line directly. Deriving it back
    /// out of synthesised segments would be inventing a harder question than
    /// the one the tool already answered, so a reader that knows the counts
    /// puts them here and `line_counts` uses them (#45).
    #[allow(clippy::type_complexity)]
    pub direct_lines: Option<std::collections::BTreeMap<u32, u64>>,
}

#[derive(Debug, Clone, Copy)]
pub struct Segment {
    pub line: u32,
    pub col: u32,
    pub count: u64,
    pub has_count: bool,
    pub is_region_entry: bool,
    pub is_gap: bool,
}

/// Region kinds from `llvm/ProfileData/Coverage/CoverageMapping.h`.
pub const KIND_CODE: u64 = 0;
pub const KIND_EXPANSION: u64 = 1;
pub const KIND_SKIPPED: u64 = 2;
pub const KIND_GAP: u64 = 3;
pub const KIND_BRANCH: u64 = 4;
pub const KIND_MCDC_DECISION: u64 = 5;
pub const KIND_MCDC_BRANCH: u64 = 6;

#[derive(Debug, Clone, Copy)]
pub struct Region {
    pub line_start: u32,
    pub col_start: u32,
    pub line_end: u32,
    pub col_end: u32,
    pub count: u64,
    pub file_id: usize,
    pub expanded_file_id: usize,
    pub kind: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct RawBranch {
    pub line_start: u32,
    pub col_start: u32,
    pub line_end: u32,
    pub col_end: u32,
    pub true_count: u64,
    pub false_count: u64,
    pub file_id: usize,
    pub kind: u64,
    /// Whether the two counts really are true and false, or just the two
    /// outcomes. llvm-cov states the polarity; gcov does not (#45).
    pub direction_known: bool,
    /// Which decision this is among several on one line, where the producer
    /// gave no columns to tell them apart. An ordinal, never a column.
    pub decision_ordinal: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct RawMcdc {
    pub line_start: u32,
    pub col_start: u32,
    pub line_end: u32,
    pub col_end: u32,
    pub true_count: u64,
    pub false_count: u64,
    /// The file id the decision is written in. `None` for the LLVM 18
    /// layout, which exports the region's *expanded* file id in that place
    /// (0 for a decision region) and no file id at all: see
    /// [`decision_files`] (#269).
    pub file_id: Option<usize>,
    pub condition_covered: Vec<bool>,
    pub vectors: Vec<RawVector>,
}

#[derive(Debug, Clone)]
pub struct RawVector {
    pub conditions: Vec<Option<bool>>,
    pub executed: bool,
    pub result: bool,
}

#[derive(Debug, Clone)]
pub struct CovFunction {
    pub name: String,
    pub count: u64,
    pub regions: Vec<Region>,
    pub branches: Vec<RawBranch>,
    pub mcdc: Vec<RawMcdc>,
    pub filenames: Vec<String>,
}

pub fn parse(text: &str) -> Result<CovExport> {
    let root: Value = serde_json::from_str(text).context("coverage export is not valid JSON")?;
    let ty = root.get("type").and_then(Value::as_str).unwrap_or("");
    if ty != "llvm.coverage.json.export" {
        bail!("not an llvm-cov export (type = {ty:?})");
    }
    let version = root.get("version").and_then(Value::as_str).unwrap_or("unknown").to_string();
    let data = root.get("data").and_then(Value::as_array).and_then(|d| d.first()).ok_or_else(|| anyhow!("export has no data[0]"))?;

    let mut files = Vec::new();
    for f in data.get("files").and_then(Value::as_array).into_iter().flatten() {
        let filename = str_field(f, "filename")?.to_string();
        let mut segments = Vec::new();
        for s in arr(f, "segments") {
            let a = s.as_array().ok_or_else(|| anyhow!("segment is not an array"))?;
            segments.push(Segment {
                line: u32_at(a, 0)?,
                col: u32_at(a, 1)?,
                count: u64_at(a, 2)?,
                has_count: bool_at(a, 3)?,
                is_region_entry: bool_at(a, 4)?,
                is_gap: a.get(5).and_then(Value::as_bool).unwrap_or(false),
            });
        }
        segments.sort_by_key(|s| (s.line, s.col));
        files.push(CovFile { filename, segments, direct_lines: None });
    }

    let mut functions = Vec::new();

    let mut caveats: Vec<String> = Vec::new();

    let mut legacy_noted = false;
    for f in data.get("functions").and_then(Value::as_array).into_iter().flatten() {
        let name = str_field(f, "name")?.to_string();
        let count = f.get("count").and_then(Value::as_u64).unwrap_or(0);
        let filenames = arr(f, "filenames").iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let mut regions = Vec::new();
        for r in arr(f, "regions") {
            let a = r.as_array().ok_or_else(|| anyhow!("region is not an array"))?;
            regions.push(Region {
                line_start: u32_at(a, 0)?,
                col_start: u32_at(a, 1)?,
                line_end: u32_at(a, 2)?,
                col_end: u32_at(a, 3)?,
                count: u64_at(a, 4)?,
                file_id: u64_at(a, 5)? as usize,
                expanded_file_id: u64_at(a, 6)? as usize,
                kind: u64_at(a, 7)?,
            });
        }
        let mut branches = Vec::new();
        for b in arr(f, "branches") {
            let a = b.as_array().ok_or_else(|| anyhow!("branch is not an array"))?;
            branches.push(RawBranch {
                line_start: u32_at(a, 0)?,
                col_start: u32_at(a, 1)?,
                line_end: u32_at(a, 2)?,
                col_end: u32_at(a, 3)?,
                true_count: u64_at(a, 4)?,
                false_count: u64_at(a, 5)?,
                file_id: u64_at(a, 6)? as usize,
                kind: u64_at(a, 8)?,
                // llvm-cov's branch regions carry the polarity of each side,
                // and a column that already identifies each decision.
                direction_known: true,
                decision_ordinal: None,
            });
        }
        branches.sort_by_key(|b| (b.file_id, b.line_start, b.col_start));
        let mut mcdc = Vec::new();
        for m in arr(f, "mcdc_records") {
            let a = m.as_array().ok_or_else(|| anyhow!("mcdc record is not an array"))?;
            // Two layouts. LLVM 19+:
            //   [ls, cs, le, ce, trueCount, falseCount, fileID, expandedFileID,
            //    kind, [conditionIndependencePairCovered...], [testVectors...]]
            // LLVM 18 (the first release with MC/DC, still Ubuntu 24.04's clang):
            //   [ls, cs, le, ce, fileID, kind, [conditionIndependencePairCovered...]]
            // -- no decision outcome counts and no test vectors at all.
            let legacy = a.get(6).map(Value::is_array).unwrap_or(false);
            if legacy && !legacy_noted {
                legacy_noted = true;
                caveats.push("The coverage export uses the LLVM 18 MC/DC record layout, which carries no decision outcome counts and no test vectors: decisions show 0 true / 0 false and the MC/DC truth tables are empty. Independence-pair coverage per condition is still exact. A newer llvm-cov (19+) exports both.".to_string());
            }
            let covered_idx = if legacy { 6 } else { 9 };
            let condition_covered = a.get(covered_idx).and_then(Value::as_array).map(|c| c.iter().map(|v| v.as_bool().unwrap_or(false)).collect()).unwrap_or_default();
            let mut vectors = Vec::new();
            for tv in if legacy { None } else { a.get(10).and_then(Value::as_array) }.into_iter().flatten() {
                let conditions = tv.get("conditions").and_then(Value::as_array).map(|c| c.iter().map(Value::as_bool).collect()).unwrap_or_default();
                vectors.push(RawVector {
                    conditions,
                    executed: tv.get("executed").and_then(Value::as_bool).unwrap_or(false),
                    result: tv.get("result").and_then(Value::as_bool).unwrap_or(false),
                });
            }
            mcdc.push(RawMcdc {
                line_start: u32_at(a, 0)?,
                col_start: u32_at(a, 1)?,
                line_end: u32_at(a, 2)?,
                col_end: u32_at(a, 3)?,
                true_count: if legacy { 0 } else { u64_at(a, 4)? },
                false_count: if legacy { 0 } else { u64_at(a, 5)? },
                // LLVM 18 index 4 is `ExpandedFileID` (renderMCDCRecord), not
                // the record's file: reading it as one put a decision inside a
                // macro at the #define line, as if in the body (#269)
                file_id: if legacy { None } else { Some(u64_at(a, 6)? as usize) },
                condition_covered,
                vectors,
            });
        }
        functions.push(CovFunction { name, count, regions, branches, mcdc, filenames });
    }
    // llvm-cov emits a branch region per decision unconditionally, so for it
    // "a decision exists" and "outcomes were recorded" are the same fact.
    let decision_sites = functions.iter().any(|f| !f.branches.is_empty() || !f.mcdc.is_empty());
    // llvm-cov exports branch regions directly; there is no shape it records
    // but declines to represent, so raw presence and parsed presence agree.
    let raw_branch_records = functions.iter().any(|f| !f.branches.is_empty());
    Ok(CovExport { version, files, functions, caveats, raw_branch_records, decision_sites })
}

/// Where, in `body_file`, the code llvm-cov numbers as file `file_id` was
/// written: the macro use its expansion came from (#168).
///
/// llvm-cov puts a macro's code in a file id of its own, one per expansion,
/// located at the `#define` lines; an expansion region in the enclosing file
/// id is the use site. A macro used inside another macro's body is a chain,
/// followed to the body's file. `None` when there is no such chain -- the file
/// id is not an expansion reached from the body.
///
/// One lookup; a caller that asks about many files builds [`Expansions`]
/// once instead (#312).
pub fn expansion_site(regions: &[Region], file_id: usize, body_file: usize) -> Option<Region> {
    Expansions::new(regions).site(file_id, body_file)
}

/// A function's macro expansions, indexed once: for each expanded file id,
/// the first expansion region naming it -- its use site, one level out.
///
/// Following a chain used to scan every region at every level, and the
/// callers asked once per branch, per view and per decision: O(R²) a lookup,
/// under three nested loops, which a crafted export of a few hundred KB
/// turned into minutes (#312). Through this index a level is one lookup, and
/// [`Expansions::sites_in`] answers for every file at once in one pass.
pub struct Expansions<'a> {
    parent: HashMap<usize, &'a Region>,
}

impl<'a> Expansions<'a> {
    pub fn new(regions: &'a [Region]) -> Self {
        let mut parent = HashMap::new();
        for r in regions.iter().filter(|r| r.kind == KIND_EXPANSION) {
            parent.entry(r.expanded_file_id).or_insert(r);
        }
        Expansions { parent }
    }

    /// [`expansion_site`], through the index.
    pub fn site(&self, file_id: usize, body_file: usize) -> Option<Region> {
        let mut id = file_id;
        // one step per nesting level; a chain longer than the index is a cycle
        // in a malformed export
        for _ in 0..=self.parent.len() {
            let site = self.parent.get(&id)?;
            if site.file_id == body_file {
                return Some(**site);
            }
            id = site.file_id;
        }
        None
    }

    /// [`Expansions::site`] in `view` for every file id that has one, in one
    /// pass: each file's answer is its parent's, unless the parent is the
    /// use in `view`, so every chain is walked once for all of them.
    ///
    /// Linear in the expansions: a file id joins a walk's path at most once
    /// in the whole pass, and every check on it is one lookup. A file id on
    /// the path is in `memo` as `None` until the walk's answer replaces it,
    /// so meeting it again is a cycle, found without scanning the path.
    pub fn sites_in(&self, view: usize) -> HashMap<usize, Region> {
        let mut memo: HashMap<usize, Option<Region>> = HashMap::new();
        for &start in self.parent.keys() {
            let mut path = Vec::new();
            let mut id = start;
            let answer = loop {
                // answered by an earlier walk, or on this one's path: a cycle
                if let Some(known) = memo.get(&id) {
                    break *known;
                }
                let Some(site) = self.parent.get(&id) else { break None };
                memo.insert(id, None);
                path.push(id);
                if site.file_id == view {
                    break Some(**site);
                }
                id = site.file_id;
            };
            for id in path {
                memo.insert(id, answer);
            }
        }
        memo.into_iter().filter_map(|(id, r)| Some((id, r?))).collect()
    }
}

/// Where a point of file `file` appears in file `view`: itself when it is in
/// `view`, else the use in `view` of the expansion it comes from. `sites` is
/// [`Expansions::sites_in`]`(view)`.
fn position_in(sites: &HashMap<usize, Region>, file: usize, line: u32, col: u32, view: usize) -> Option<(u32, u32)> {
    if file == view {
        return Some((line, col));
    }
    sites.get(&file).map(|s| (s.line_start, s.col_start))
}

fn span_contains(m: &RawMcdc, p: (u32, u32)) -> bool {
    p >= (m.line_start, m.col_start) && p <= (m.line_end, m.col_end)
}

/// The file id each MC/DC decision record of `f` is written in, parallel to
/// `f.mcdc` (#269). `None`: not established.
///
/// LLVM 19+ states it. The LLVM 18 layout does not, so it is recovered from
/// the decision's conditions: the file `v` in whose view some MC/DC
/// condition region (kind 6) lies inside the decision's span -- the
/// condition's own position when it is in `v`, or the use in `v` of the
/// macro it comes from. A decision inside a macro has its conditions at the
/// #define, in the expansion's file; a body decision whose conditions are
/// macro uses has them at the uses, in the body.
///
/// A macro used twice in one function gives two records with the same span
/// and two files that answer. The layout cannot say which record is which
/// file; when the records carry the same evidence (the same per-condition
/// independence) the pairing changes nothing, and they are paired in order.
/// When they differ, neither is established -- never a guess.
pub fn decision_files(f: &CovFunction) -> Vec<Option<usize>> {
    // Every point an MC/DC condition shows at, by the file it shows in: its
    // own position, and each use its expansion chain passes through. Each
    // chain is walked once for all the conditions under it (#312); asking,
    // for every decision, every view and every condition, re-walked it.
    let expansions = Expansions::new(&f.regions);
    let mut points: std::collections::BTreeMap<usize, Vec<(u32, u32)>> = std::collections::BTreeMap::new();
    let mut walked: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for b in f.branches.iter().filter(|b| b.kind == KIND_MCDC_BRANCH) {
        points.entry(b.file_id).or_default().push((b.line_start, b.col_start));
        let mut id = b.file_id;
        while walked.insert(id) {
            let Some(site) = expansions.parent.get(&id) else { break };
            points.entry(site.file_id).or_default().push((site.line_start, site.col_start));
            id = site.file_id;
        }
    }
    let candidates: Vec<Vec<usize>> = f
        .mcdc
        .iter()
        .map(|m| match m.file_id {
            Some(id) => vec![id],
            None => points.iter().filter(|(_, ps)| ps.iter().any(|&p| span_contains(m, p))).map(|(&v, _)| v).collect(),
        })
        .collect();
    let span = |m: &RawMcdc| (m.line_start, m.col_start, m.line_end, m.col_end);
    f.mcdc
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let c = &candidates[i];
            match c.len() {
                0 => None,
                1 => Some(c[0]),
                _ => {
                    let twins: Vec<usize> = (0..f.mcdc.len()).filter(|&j| f.mcdc[j].file_id.is_none() && span(&f.mcdc[j]) == span(m)).collect();
                    let alike = twins.iter().all(|&j| f.mcdc[j].condition_covered == m.condition_covered && candidates[j] == *c);
                    if alike && twins.len() == c.len() {
                        twins.iter().position(|&j| j == i).map(|k| c[k])
                    } else {
                        None
                    }
                }
            }
        })
        .collect()
}

/// An MC/DC decision as it appears in the function body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedDecision {
    /// The span in the body: the record's own, or the macro use's.
    pub line_start: u32,
    pub col_start: u32,
    pub line_end: u32,
    pub col_end: u32,
    /// Indices into the function's `branches`: the regions inside the
    /// decision, in the order they are written where the decision is written
    /// -- for a decision inside a macro, their order in the #define.
    pub conditions: Vec<usize>,
}

/// Place decision record `i` of `f`, written in file `file`, in the body
/// file (#269). A decision inside a macro goes to the macro's use, where
/// DWARF puts its object code, as its branches do (`expansion_site`, #168).
/// `None` when the file has no expansion chain to the body.
///
/// One decision; a caller placing all of a function's uses
/// [`place_decisions`], which builds the indexes once (#312).
pub fn place_decision(f: &CovFunction, i: usize, file: usize, body_file: usize) -> Option<PlacedDecision> {
    let expansions = Expansions::new(&f.regions);
    place_in(f, &f.mcdc[i], file, body_file, &expansions.sites_in(file), &expansions.sites_in(body_file))
}

/// [`place_decision`] for every decision of `f`, each in the file `files`
/// gives it ([`decision_files`]); `None` where that is `None` or the
/// decision cannot be placed. The expansion index is built once for the
/// function, and the expansion uses in each file once for that file -- not
/// once per decision, which repeated every chain walk for each (#312).
pub fn place_decisions(f: &CovFunction, files: &[Option<usize>], body_file: usize) -> Vec<Option<PlacedDecision>> {
    let expansions = Expansions::new(&f.regions);
    let in_body = expansions.sites_in(body_file);
    let mut in_view: HashMap<usize, HashMap<usize, Region>> = HashMap::new();
    f.mcdc
        .iter()
        .zip(files)
        .map(|(m, &file)| {
            let file = file?;
            let in_file = in_view.entry(file).or_insert_with(|| expansions.sites_in(file));
            place_in(f, m, file, body_file, in_file, &in_body)
        })
        .collect()
}

/// Decision `m`, written in `file`, placed in the body; `in_file` and
/// `in_body` are [`Expansions::sites_in`] those files.
fn place_in(f: &CovFunction, m: &RawMcdc, file: usize, body_file: usize, in_file: &HashMap<usize, Region>, in_body: &HashMap<usize, Region>) -> Option<PlacedDecision> {
    let (line_start, col_start, line_end, col_end) = if file == body_file {
        (m.line_start, m.col_start, m.line_end, m.col_end)
    } else {
        let s = in_body.get(&file)?;
        (s.line_start, s.col_start, s.line_end, s.col_end)
    };
    let mut inside: Vec<((u32, u32), usize)> = f
        .branches
        .iter()
        .enumerate()
        .filter_map(|(k, b)| {
            let p = position_in(in_file, b.file_id, b.line_start, b.col_start, file)?;
            // reached from the body too, or it has nowhere to be shown
            position_in(in_body, b.file_id, b.line_start, b.col_start, body_file)?;
            span_contains(m, p).then_some((p, k))
        })
        .collect();
    inside.sort();
    Some(PlacedDecision { line_start, col_start, line_end, col_end, conditions: inside.into_iter().map(|(_, k)| k).collect() })
}

/// Per-line execution counts for one file, index = line - 1.
///
/// Mirrors `LineCoverageStats` in LLVM's `CoverageMapping.cpp`: a line's count
/// is the maximum over the non-gap region entries that start on the line; if
/// none start there, it is the count of the segment wrapping the line. A line
/// with no segment carrying a count is "not instrumented" (`None`).
pub fn line_counts(file: &CovFile, num_lines: usize) -> Vec<Option<u64>> {
    let mut out = vec![None; num_lines];
    // A producer that states its line counts is believed. A line it does not
    // mention is one it generated no code for, which is `None` -- not zero:
    // "no code here" and "code that never ran" are different findings and
    // the distinction is the whole point of the uncovered/unreachable split.
    if let Some(direct) = &file.direct_lines {
        for (&line, &count) in direct {
            if line >= 1 && (line as usize) <= num_lines {
                out[(line - 1) as usize] = Some(count);
            }
        }
        return out;
    }
    let segs = &file.segments;
    let mut idx = 0usize;
    let mut wrapped: Option<Segment> = None;
    for line in 1..=(num_lines as u32) {
        // Skip any malformed line-0 or out-of-order segments below `line` so
        // they cannot stall `idx` and drop the rest of the file's coverage.
        while idx < segs.len() && segs[idx].line < line {
            idx += 1;
        }
        // segments starting on this line
        let start = idx;
        while idx < segs.len() && segs[idx].line == line {
            idx += 1;
        }
        let on_line = &segs[start..idx];
        let is_start_of_region = |s: &Segment| !s.is_gap && s.has_count && s.is_region_entry;
        let min_region_count = on_line.iter().filter(|s| is_start_of_region(s)).count();
        let start_of_skipped = on_line.first().map(|s| !s.has_count && s.is_region_entry).unwrap_or(false);
        let wrapped_has_count = wrapped.map(|w| w.has_count).unwrap_or(false);
        let mapped = !start_of_skipped && (wrapped_has_count || min_region_count > 0);
        let count = if !mapped {
            None
        } else {
            let mut c = wrapped.map(|w| w.count).unwrap_or(0);
            if min_region_count > 0 {
                for s in on_line.iter().filter(|s| is_start_of_region(s)) {
                    c = c.max(s.count);
                }
            }
            Some(c)
        };
        out[(line - 1) as usize] = count;
        if let Some(last) = on_line.last() {
            wrapped = Some(*last);
        }
    }
    out
}

/// The segments llvm-cov would derive from `regions` alone: a port of
/// `SegmentBuilder` in LLVM's `CoverageMapping.cpp` (sort nested regions,
/// combine duplicates, then walk them with a stack of active regions).
///
/// The export states segments per *file*, built from every function record
/// that maps into it -- and a generic or a template instantiated at two
/// types is two records over the same lines, so the file's counts are their
/// sum. One instantiation's own line counts exist only in its own regions;
/// this is how they are recovered, by the same rules the file's were made
/// with (#167). `regions` must all belong to one file.
pub fn segments_from_regions(regions: &[Region]) -> Vec<Segment> {
    type Loc = (u32, u32);
    let start = |r: &Region| -> Loc { (r.line_start, r.col_start) };
    let end = |r: &Region| -> Loc { (r.line_end, r.col_end) };

    // sortNestedRegions: by start; an enclosing region before what it
    // contains; for one area, code before expansion before skipped.
    // Drop malformed line-0 or inverted regions (end < start), which would
    // otherwise emit out-of-order completion segments.
    let mut rs: Vec<Region> =
        regions.iter().filter(|r| matches!(r.kind, KIND_CODE | KIND_EXPANSION | KIND_SKIPPED | KIND_GAP) && r.line_start > 0 && end(r) >= start(r)).copied().collect();
    rs.sort_by(|a, b| start(a).cmp(&start(b)).then(end(b).cmp(&end(a))).then(a.kind.cmp(&b.kind)));
    // combineRegions: one region per area, counts summed over the regions of
    // the kind that came first.
    let mut regions: Vec<Region> = Vec::with_capacity(rs.len());
    for r in rs {
        match regions.last_mut() {
            Some(a) if start(a) == start(&r) && end(a) == end(&r) => {
                if a.kind == r.kind {
                    a.count = a.count.saturating_add(r.count);
                }
            }
            _ => regions.push(r),
        }
    }

    fn start_segment(segs: &mut Vec<Segment>, r: &Region, at: Loc, entry: bool, skipped: bool) {
        let has_count = !skipped && r.kind != KIND_SKIPPED;
        let count = if has_count { r.count } else { 0 };
        // a segment that would not change what is rendered is not emitted
        if let Some(last) = segs.last() {
            if !entry && !skipped && last.has_count == has_count && last.count == count && !last.is_region_entry {
                return;
            }
        }
        segs.push(Segment { line: at.0, col: at.1, count, has_count, is_region_entry: entry, is_gap: has_count && r.kind == KIND_GAP });
    }

    // completeRegionsUntil: close active[first..], which have all ended by
    // `at`, resuming whatever encloses them.
    fn complete_until(segs: &mut Vec<Segment>, active: &mut Vec<Region>, at: Option<Loc>, first: usize) {
        let end = |r: &Region| -> Loc { (r.line_end, r.col_end) };
        active[first..].sort_by_key(end);
        for i in first + 1..active.len() {
            let seg_at = end(&active[i - 1]);
            if at == Some(seg_at) {
                break;
            }
            if seg_at == end(&active[i]) {
                continue;
            }
            let mut completed = i;
            for j in i + 1..active.len() {
                if end(&active[i]) == end(&active[j]) {
                    completed = j;
                }
            }
            let r = active[completed];
            start_segment(segs, &r, seg_at, false, false);
        }
        let last = *active.last().expect("called with active regions");
        if first > 0 && at != Some(end(&last)) {
            let r = active[first - 1];
            start_segment(segs, &r, end(&last), false, false);
        } else if first == 0 && at != Some(end(&last)) {
            start_segment(segs, &last, end(&last), false, true);
        }
        active.truncate(first);
    }

    let mut segs: Vec<Segment> = Vec::new();
    let mut active: Vec<Region> = Vec::new();
    for (i, cr) in regions.iter().enumerate() {
        let cur = start(cr);
        // stable_partition: still open first, those ended by `cur` after
        let (open, done): (Vec<Region>, Vec<Region>) = active.iter().partition(|r| end(r) > cur);
        let first = open.len();
        let any_done = !done.is_empty();
        active = open.into_iter().chain(done).collect();
        if any_done {
            complete_until(&mut segs, &mut active, Some(cur), first);
        }
        let gap = cr.kind == KIND_GAP;
        if cur == end(cr) {
            // A zero-length region is never made active.
            let skipped = i + 1 == regions.len() || cr.kind == KIND_SKIPPED;
            let r = active.last().copied().unwrap_or(*cr);
            start_segment(&mut segs, &r, cur, !gap, skipped);
            if skipped {
                if let Some(r) = active.last().copied() {
                    start_segment(&mut segs, &r, cur, false, false);
                }
            }
            continue;
        }
        if i + 1 == regions.len() || cur != start(&regions[i + 1]) {
            start_segment(&mut segs, cr, cur, !gap, false);
        }
        active.push(*cr);
    }
    if !active.is_empty() {
        complete_until(&mut segs, &mut active, None, 0);
    }
    segs.sort_by_key(|s| (s.line, s.col));
    segs
}

/// Function records' own per-line counts over one file, index = line - 1:
/// what `line_counts` would give if these records were the only ones
/// mapping into `filename`. A record can name one file under several ids
/// (a macro expanded from the same file); llvm-cov takes every one of them,
/// and so does this.
pub fn records_line_counts<'a>(records: impl IntoIterator<Item = &'a CovFunction>, filename: &str, num_lines: usize) -> Vec<Option<u64>> {
    let regions: Vec<Region> = records.into_iter().flat_map(|f| f.regions.iter().filter(move |r| f.filenames.get(r.file_id).is_some_and(|n| n == filename))).copied().collect();
    line_counts(&CovFile { filename: String::new(), segments: segments_from_regions(&regions), direct_lines: None }, num_lines)
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key).and_then(Value::as_str).ok_or_else(|| anyhow!("missing string field {key:?}"))
}

fn u64_at(a: &[Value], i: usize) -> Result<u64> {
    a.get(i).and_then(Value::as_u64).ok_or_else(|| anyhow!("expected integer at index {i} in {a:?}"))
}

fn u32_at(a: &[Value], i: usize) -> Result<u32> {
    let v = u64_at(a, i)?;
    u32::try_from(v).map_err(|_| anyhow!("integer {v} at index {i} exceeds u32::MAX"))
}

fn bool_at(a: &[Value], i: usize) -> Result<bool> {
    a.get(i).and_then(Value::as_bool).ok_or_else(|| anyhow!("expected bool at index {i} in {a:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seg(line: u32, col: u32, count: u64, has: bool, entry: bool, gap: bool) -> Segment {
        Segment { line, col, count, has_count: has, is_region_entry: entry, is_gap: gap }
    }

    fn region(l: u32, c: u32, le: u32, ce: u32, file_id: usize, expanded_file_id: usize, kind: u64) -> Region {
        Region { line_start: l, col_start: c, line_end: le, col_end: ce, count: 1, file_id, expanded_file_id, kind }
    }

    #[test]
    fn a_macro_expansion_is_traced_to_its_use_site() {
        // examples/c-constructs checked_scale: REQUIRE used on lines 157-159,
        // each use its own expansion file id 1..3 located at the #define
        let regions = [
            region(156, 42, 161, 2, 0, 0, KIND_CODE),
            region(157, 5, 157, 12, 0, 1, KIND_EXPANSION),
            region(158, 5, 158, 12, 0, 2, KIND_EXPANSION),
            region(148, 5, 152, 16, 1, 0, KIND_CODE),
            // a macro used inside REQUIRE's body: file 4 expands in file 2
            region(149, 13, 149, 20, 2, 4, KIND_EXPANSION),
        ];
        assert_eq!(expansion_site(&regions, 1, 0).map(|r| (r.line_start, r.col_start)), Some((157, 5)));
        assert_eq!(expansion_site(&regions, 2, 0).map(|r| r.line_start), Some(158));
        assert_eq!(expansion_site(&regions, 4, 0).map(|r| r.line_start), Some(158), "nested: the outermost use in the body");
        assert!(expansion_site(&regions, 7, 0).is_none(), "not an expansion");
        // a malformed export whose expansions point at each other ends
        let cycle = [region(1, 1, 1, 2, 5, 6, KIND_EXPANSION), region(1, 1, 1, 2, 6, 5, KIND_EXPANSION)];
        assert!(expansion_site(&cycle, 5, 0).is_none());
    }
    /// A chain of expansions -- each macro used inside the next -- is followed
    /// one lookup per level, and each branch's chain once: re-scanning every
    /// region at every level, for every branch, view and decision, took
    /// minutes on one crafted function of a few hundred KB (#312). Nor is it
    /// walked again for each decision, nor its path re-scanned at each level
    /// for a cycle (#319 review).
    #[test]
    fn a_deep_expansion_chain_is_placed_in_linear_time() {
        let depth = 20_000usize;
        let mut regions = vec![region(1, 1, 90_000, 1, 0, 0, KIND_CODE)];
        // file k is used at line 10+k of file k-1; file `depth` is the innermost
        regions.extend((1..=depth).map(|k| region(10 + k as u32, 5, 10 + k as u32, 12, k - 1, k, KIND_EXPANSION)));
        let branch = |file_id: usize| RawBranch {
            line_start: 5,
            col_start: 2,
            line_end: 5,
            col_end: 3,
            true_count: 1,
            false_count: 0,
            file_id,
            kind: KIND_MCDC_BRANCH,
            direction_known: true,
            decision_ordinal: None,
        };
        let branches: Vec<RawBranch> = (0..80).map(|_| branch(depth)).collect();
        // a decision written in the body, spanning the first use
        let mcdc =
            vec![RawMcdc { line_start: 11, col_start: 1, line_end: 11, col_end: 20, true_count: 1, false_count: 1, file_id: None, condition_covered: vec![true], vectors: vec![] }];
        let f = CovFunction { name: "f".into(), count: 1, regions: regions.clone(), branches: branches.clone(), mcdc, filenames: vec!["a.c".into()] };
        // and many written in the innermost macro, each placed at the outermost use
        let many = CovFunction { mcdc: (0..500).map(|_| record(5, 1, 5, 20, Some(depth), &[true])).collect(), ..f.clone() };
        let t = std::time::Instant::now();
        let files = decision_files(&f);
        let placed = place_decision(&f, 0, 0, 0);
        let site = expansion_site(&regions, depth, 0);
        let all = place_decisions(&many, &decision_files(&many), 0);
        assert!(t.elapsed().as_secs() < 5, "took {:?}", t.elapsed());
        // and the answers are the ones the chain gives: the innermost branches
        // show in the body at the outermost use, line 11
        assert_eq!(files, vec![Some(0)]);
        assert_eq!(placed.map(|p| (p.line_start, p.conditions.len())), Some((11, 80)));
        assert_eq!(site.map(|r| r.line_start), Some(11));
        assert_eq!(all.len(), 500);
        assert!(all.iter().all(|p| p.as_ref().map(|p| (p.line_start, p.conditions.len())) == Some((11, 80))));
    }

    /// [`Expansions::sites_in`] still ends a walk at a cycle -- now by the
    /// path's entries in its memo, not a scan of the path -- and answers for
    /// the chain that leads into it no differently.
    #[test]
    fn sites_in_ends_at_a_cycle() {
        // 1 and 2 expand each other; 3 is used in 1; 4 is used in the body
        let regions = [
            region(1, 1, 1, 2, 2, 1, KIND_EXPANSION),
            region(2, 1, 2, 2, 1, 2, KIND_EXPANSION),
            region(3, 1, 3, 2, 1, 3, KIND_EXPANSION),
            region(4, 1, 4, 2, 0, 4, KIND_EXPANSION),
        ];
        let sites = Expansions::new(&regions).sites_in(0);
        assert_eq!(sites.keys().copied().collect::<Vec<_>>(), vec![4]);
        assert_eq!(Expansions::new(&regions).sites_in(1).get(&3).map(|r| r.line_start), Some(3));
        assert_eq!(Expansions::new(&regions).sites_in(1).get(&2).map(|r| r.line_start), Some(2));
    }

    #[test]
    fn llvm18_mcdc_record_layout_is_read() {
        // Ubuntu 24.04's clang 18 -- seen in CI: seven elements, conditions at index 6
        let text = r#"{"version":"2.0.1","type":"llvm.coverage.json.export","data":[{"files":[{"filename":"demo.c","segments":[],"branches":[],"summary":{}}],
            "functions":[{"name":"mode_permits_engage","count":3,"regions":[],"branches":[],"filenames":["demo.c"],
              "mcdc_records":[[23,9,23,39,0,5,[true,false,true]]]}]}]}"#;
        let ex = parse(text).unwrap();
        let m = &ex.functions[0].mcdc[0];
        assert_eq!((m.line_start, m.col_start, m.line_end, m.col_end), (23, 9, 23, 39));
        assert_eq!(m.condition_covered, vec![true, false, true]);
        assert!(m.vectors.is_empty() && m.true_count == 0 && m.false_count == 0);
        assert!(ex.caveats.iter().any(|c| c.contains("LLVM 18")), "{:?}", ex.caveats);
        // and the newer layout is untouched
        let text2 = text
            .replace("[[23,9,23,39,0,5,[true,false,true]]]", "[[23,9,23,39,2,1,0,0,5,[true,false,true],[{\"conditions\":[false,null,false],\"executed\":true,\"result\":false}]]]");
        let ex2 = parse(&text2).unwrap();
        let m2 = &ex2.functions[0].mcdc[0];
        assert_eq!((m2.true_count, m2.false_count, m2.vectors.len()), (2, 1, 1));
        assert!(ex2.caveats.is_empty());
        // the LLVM 18 record's index 4 is its expanded file id, not its file
        // (#269): no file is read from it; the newer layout states one
        assert_eq!(m.file_id, None);
        assert_eq!(m2.file_id, Some(0));
    }

    fn mbranch(l: u32, c: u32, file_id: usize, kind: u64, t: u64, f: u64) -> RawBranch {
        RawBranch { line_start: l, col_start: c, line_end: l, col_end: c + 5, true_count: t, false_count: f, file_id, kind, direction_known: true, decision_ordinal: None }
    }

    fn record(l: u32, c: u32, le: u32, ce: u32, file_id: Option<usize>, covered: &[bool]) -> RawMcdc {
        RawMcdc { line_start: l, col_start: c, line_end: le, col_end: ce, true_count: 0, false_count: 0, file_id, condition_covered: covered.to_vec(), vectors: Vec::new() }
    }

    fn function(regions: Vec<Region>, branches: Vec<RawBranch>, mcdc: Vec<RawMcdc>) -> CovFunction {
        CovFunction { name: "in_range".into(), count: 1, regions, branches, mcdc, filenames: vec!["constructs.c".into()] }
    }

    #[test]
    fn an_mcdc_decision_inside_a_macro_is_placed_at_its_use_in_either_layout() {
        // examples/c-constructs in_range, Apple clang 17: IN_RANGE #defined on
        // line 193 (file 1 is its expansion), used on line 197 col 9
        let regions = vec![region(195, 21, 201, 2, 0, 0, KIND_CODE), region(197, 9, 197, 17, 0, 1, KIND_EXPANSION), region(193, 29, 193, 57, 1, 0, KIND_CODE)];
        let branches = vec![mbranch(193, 30, 1, KIND_MCDC_BRANCH, 1, 0), mbranch(193, 45, 1, KIND_MCDC_BRANCH, 1, 0)];
        for file_id in [None, Some(1)] {
            let f = function(regions.clone(), branches.clone(), vec![record(193, 30, 193, 56, file_id, &[false, false])]);
            assert_eq!(decision_files(&f), vec![Some(1)], "{file_id:?}");
            let p = place_decision(&f, 0, 1, 0).unwrap();
            assert_eq!((p.line_start, p.col_start, p.line_end, p.col_end), (197, 9, 197, 17), "at the use, not the #define");
            assert_eq!(p.conditions, vec![0, 1], "the #define's order");
        }
        // a file with no way to the body is not placed
        let f = function(regions.clone(), branches.clone(), vec![record(193, 30, 193, 56, Some(7), &[false, false])]);
        assert!(place_decision(&f, 0, 7, 0).is_none());
    }

    #[test]
    fn an_llvm18_body_decision_keeps_its_place() {
        // an ordinary decision in the body, and one whose conditions are both
        // macro uses: neither has a condition region inside its span in any
        // file but the body's view
        let regions = vec![region(10, 1, 30, 2, 0, 0, KIND_CODE), region(20, 9, 20, 13, 0, 1, KIND_EXPANSION), region(20, 17, 20, 21, 0, 2, KIND_EXPANSION)];
        let branches = vec![
            mbranch(12, 9, 0, KIND_MCDC_BRANCH, 1, 1),
            mbranch(12, 18, 0, KIND_MCDC_BRANCH, 1, 0),
            mbranch(2, 20, 1, KIND_MCDC_BRANCH, 2, 0),
            mbranch(3, 20, 2, KIND_MCDC_BRANCH, 0, 2),
        ];
        let f = function(regions, branches, vec![record(12, 9, 12, 25, None, &[true, false]), record(20, 9, 20, 21, None, &[false, false])]);
        assert_eq!(decision_files(&f), vec![Some(0), Some(0)]);
        let p = place_decision(&f, 0, 0, 0).unwrap();
        assert_eq!((p.line_start, p.col_start, p.conditions.clone()), (12, 9, vec![0, 1]));
        let p = place_decision(&f, 1, 0, 0).unwrap();
        assert_eq!((p.line_start, p.col_start, p.conditions.clone()), (20, 9, vec![2, 3]), "the macro conditions at their uses, in use order");
    }

    #[test]
    fn two_uses_of_one_macro_are_paired_only_when_it_cannot_matter() {
        // IN_RANGE used twice: files 1 and 2, identical spans at the #define.
        // The LLVM 18 layout does not say which record is which use.
        let regions = vec![
            region(10, 1, 30, 2, 0, 0, KIND_CODE),
            region(12, 9, 12, 17, 0, 1, KIND_EXPANSION),
            region(14, 9, 14, 17, 0, 2, KIND_EXPANSION),
            region(2, 29, 2, 57, 1, 0, KIND_CODE),
            region(2, 29, 2, 57, 2, 0, KIND_CODE),
        ];
        let branches = vec![
            mbranch(2, 30, 1, KIND_MCDC_BRANCH, 1, 0),
            mbranch(2, 45, 1, KIND_MCDC_BRANCH, 1, 0),
            mbranch(2, 30, 2, KIND_MCDC_BRANCH, 1, 1),
            mbranch(2, 45, 2, KIND_MCDC_BRANCH, 1, 0),
        ];
        // the same evidence in both: the pairing changes nothing
        let same = function(regions.clone(), branches.clone(), vec![record(2, 30, 2, 56, None, &[false, false]), record(2, 30, 2, 56, None, &[false, false])]);
        assert_eq!(decision_files(&same), vec![Some(1), Some(2)]);
        // different evidence: which use proved independence is not known
        let differ = function(regions.clone(), branches.clone(), vec![record(2, 30, 2, 56, None, &[true, false]), record(2, 30, 2, 56, None, &[false, false])]);
        assert_eq!(decision_files(&differ), vec![None, None]);
        // the newer layout says, and is taken at its word
        let stated = function(regions, branches, vec![record(2, 30, 2, 56, Some(2), &[true, false]), record(2, 30, 2, 56, Some(1), &[false, false])]);
        assert_eq!(decision_files(&stated), vec![Some(2), Some(1)]);
        assert_eq!(place_decision(&stated, 0, 2, 0).map(|p| (p.line_start, p.conditions)), Some((14, vec![2, 3])));
    }

    #[test]
    fn line_counts_follow_llvm_rules() {
        // line 1: region entry count 3; line 2: wrapped by it; line 3: starts a
        // skipped region (comment) -> not mapped; line 4: an entry with count 0
        // but wrapped by a count-3 segment -> max = 3 (LLVM rule); line 5: nothing
        let f = CovFile {
            filename: "x".into(),
            direct_lines: None,
            segments: vec![
                seg(1, 1, 3, true, true, false),
                seg(3, 1, 0, false, true, false),
                seg(3, 20, 3, true, false, false),
                seg(4, 1, 0, true, true, false),
                seg(4, 10, 0, false, false, false),
            ],
        };
        let c = line_counts(&f, 5);
        assert_eq!(c, vec![Some(3), Some(3), None, Some(3), None]);
    }

    /// Two records over one function's lines, as a generic monomorphised at
    /// two types gives: `if v < lo { return lo; } v` with the early return
    /// taken by one copy only. Each record's own counts say which copy ran
    /// which line; the two together give the file's sum (#167).
    #[test]
    fn a_records_own_line_counts_are_recovered_from_its_regions() {
        let reg = |l0, c0, l1, c1, count| Region { line_start: l0, col_start: c0, line_end: l1, col_end: c1, count, file_id: 0, expanded_file_id: 0, kind: KIND_CODE };
        let rec = |taken: u64| CovFunction {
            name: "f".into(),
            count: 1,
            // body, the return nested in it, the tail after it
            regions: vec![reg(1, 1, 2, 12, 1), reg(2, 12, 3, 6, taken), reg(4, 1, 4, 6, 1 - taken), reg(5, 1, 5, 2, 1 - taken)],
            branches: vec![],
            mcdc: vec![],
            filenames: vec!["x.rs".into()],
        };
        let (a, b) = (rec(1), rec(0));
        assert_eq!(records_line_counts([&a], "x.rs", 5), vec![Some(1), Some(1), Some(1), Some(0), Some(0)]);
        assert_eq!(records_line_counts([&b], "x.rs", 5), vec![Some(1), Some(1), Some(0), Some(1), Some(1)]);
        assert_eq!(records_line_counts([&a, &b], "x.rs", 5), vec![Some(2), Some(2), Some(1), Some(1), Some(1)], "together they are the file's view");
        assert_eq!(records_line_counts([&a], "y.rs", 5), vec![None; 5], "a record's regions in another file are not this file's");
    }
}
