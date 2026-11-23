// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! GCC coverage, read from `gcov --json-format` (#45).
//!
//! butterfly's object-code half -- DWARF, symbols, disassembly, the CFG,
//! stack usage -- was already toolchain agnostic; only the coverage front
//! end was tied to llvm-cov. Most certified airborne software is built with
//! GCC or a GCC-derived qualified compiler, so that one dependency held back
//! the widest reach.
//!
//! # Why the JSON, and not `.gcno`/`.gcda`
//!
//! `gcov` ships with the compiler and is version-matched to it, the way
//! `llvm-profdata` must be to the clang that wrote a profile (#61). Its JSON
//! declares its own `format_version`, so a change is announced rather than
//! discovered. The `.gcno`/`.gcda` binaries are internal, undocumented, and
//! change between GCC releases with no compatibility promise -- for a tool
//! whose output is certification evidence, reverse-engineering a format
//! nobody guarantees is the wrong trade at any speed. Third-party readers
//! (gcovr, lcov) would put another program between butterfly and the
//! compiler's own record, which is the opposite of the point.
//!
//! # What GCC states, and what it does not
//!
//! * **Line counts** -- stated outright, per line. Better than llvm-cov's
//!   segments, which have to be resolved into line counts.
//! * **Functions** -- name, line range, and execution count.
//! * **Branch outcomes** -- per basic block, and only with `gcov -b`. They
//!   give decision coverage.
//! * **Conditions** -- with `-fcondition-coverage` (GCC 14+): how many of a
//!   decision's condition outcomes were covered, and which were not.
//! * **MC/DC** -- *not available*. Condition coverage is not the independence
//!   pairs A-7 objective 5 requires, and GCC emits nothing that would let
//!   them be reconstructed. A GCC project gets statement and decision
//!   coverage here, and MC/DC has to come from the object-level path.
//!
//! # The outcome-direction trap
//!
//! gcov's branch entries carry `fallthrough`, which names a **CFG edge**,
//! not the polarity of the condition. Which edge is the true one depends on
//! the construct, measured on GCC 16:
//!
//! | construct | `fallthrough: true` is |
//! |---|---|
//! | `if`, `if`/`else` | the condition being **true** |
//! | `&&`, `||` (short-circuit) | the condition being **true** |
//! | `while`, `for`, `do`/`while` | the loop **exit**, i.e. **false** |
//!
//! So reading `fallthrough` as polarity reports every loop backwards. That
//! would not merely mislabel a column: the direction is part of a
//! resolution key, so a reviewer's justification would be filed against the
//! outcome they did not justify, and the review record would be wrong.
//!
//! Branches from here are therefore marked `direction_known: false`. Both
//! outcomes are reported and decision coverage is exact -- "were both taken"
//! does not depend on which is which -- but neither is named `true`.
//! Establishing the polarity means reading the branch instruction in the
//! object code, which butterfly already disassembles; that is a separate
//! change, not a guess made here.

use crate::coverage::{CovExport, CovFile, CovFunction, RawBranch, Region, KIND_CODE};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::BTreeMap;

/// Does this text look like a gcov JSON export?
///
/// Both keys, because `gcc_version` alone appears in other GCC artifacts and
/// `format_version` alone is not distinctive.
pub fn looks_like(text: &str) -> bool {
    text.contains("\"gcc_version\"") && text.contains("\"format_version\"")
}

fn u64_of(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}
fn u32_of(v: &Value) -> u32 {
    v.as_u64().unwrap_or(0).min(u32::MAX as u64) as u32
}

/// Read a gcov export, which may be several documents.
///
/// `gcov --json-format --stdout a.gcda b.gcda` writes **one JSON document
/// per data file**, newline-separated -- not one merged document. A real
/// project has more than one translation unit, so parsing only the first
/// would silently drop every unit after it, which is the shape of mistake
/// this reader is most concerned with (self-review of #45).
pub fn parse(text: &str) -> Result<CovExport> {
    // Split by parsing, not by looking for lines that begin with a brace:
    // pretty-printed JSON has plenty of those inside one document, and a
    // reader that guessed at the boundaries would tear a single export into
    // fragments. `StreamDeserializer` knows where a value ends.
    let mut docs = Vec::new();
    for v in serde_json::Deserializer::from_str(text).into_iter::<Value>() {
        docs.push(v?);
    }
    let mut merged: Option<CovExport> = None;
    for v in docs {
        let one = from_value(v)?;
        match &mut merged {
            None => merged = Some(one),
            Some(m) => {
                m.files.extend(one.files);
                m.functions.extend(one.functions);
                // Presence is a property of the whole export, not of the
                // document that happened to come first: one unit with real
                // branch records makes the export carry them, whatever a
                // branchless neighbour said. Kept per-document, doctor's
                // advice depended on gcov's emission order (review of #45).
                m.raw_branch_records |= one.raw_branch_records;
                m.decision_sites |= one.decision_sites;
                // The caveats are about the producer, not about one data
                // file, so they are stated once however many documents came.
                for c in one.caveats {
                    if !m.caveats.contains(&c) {
                        m.caveats.push(c);
                    }
                }
            }
        }
    }
    let mut merged = merged.ok_or_else(|| anyhow!("gcov export contained no JSON document"))?;
    // The no-branch caveat is earned per document; the merged export keeps it
    // only when it is true of the whole -- a caveat claiming there are no
    // branch records above an export that has them would be read, believed,
    // and acted on.
    if merged.raw_branch_records {
        merged.caveats.retain(|c| c != NO_BRANCH_CAVEAT);
    }
    coalesce_files(&mut merged);
    Ok(merged)
}

/// Stated per document, revisited at the merge: a document with no branch
/// records earns this, but the merged export only keeps it when *no*
/// document had any. Ingest replaces it with [`NO_BRANCH_NO_JUMP_CAVEAT`]
/// where the object code has no conditional branch to show `-b` was missed
/// (#325).
pub const NO_BRANCH_CAVEAT: &str = "the gcov export carries no branch records, so there is no decision coverage in it; re-run gcov with -b";

/// [`NO_BRANCH_CAVEAT`] where every function the export describes was found
/// in the object code and none has a conditional branch. That makes the cause
/// ambiguous -- a straight-line program exported with `-b` has no records to
/// give -- so the instruction to re-run is dropped. It does not show the
/// source has no decision: one lowered to a conditional select or predication
/// has no branch in the object code either, so what was not supplied is still
/// said (#335 review).
pub const NO_BRANCH_NO_JUMP_CAVEAT: &str = "the gcov export carries no branch records, so decision outcomes were not supplied and decision coverage completeness is unknown; the object code has no conditional branch, so whether gcov ran without -b cannot be told from it -- a source decision lowered to a conditional select or predication has no branch in the object code either";

/// One entry per source path, whatever it took to get there.
///
/// A header with an inline or `static` function is compiled into every unit
/// that includes it, and gcov reports it once per unit -- so `shared.h`
/// arrives two or five times, each copy holding only what *that* unit
/// executed. Left as separate entries the consumer keeps the last one, and
/// every function attributed to an earlier copy reads as uncovered while its
/// own execution count says otherwise. That is ordinary C, not malformed
/// input (review of #45).
///
/// Line counts are summed across the copies, because they are counts of the
/// same source line reached through different units, and a line is covered if
/// any unit ran it. A line no copy mentions stays absent: "no code here" and
/// "code that never ran" remain different answers.
fn coalesce_files(e: &mut CovExport) {
    use std::collections::BTreeMap;
    let mut first: BTreeMap<String, usize> = BTreeMap::new();
    let mut keep: Vec<CovFile> = Vec::new();
    for f in std::mem::take(&mut e.files) {
        match first.get(&f.filename) {
            None => {
                first.insert(f.filename.clone(), keep.len());
                keep.push(f);
            }
            Some(&i) => {
                // Fold this unit's counts into the entry already kept.
                let into = keep[i].direct_lines.get_or_insert_with(BTreeMap::new);
                if let Some(more) = f.direct_lines {
                    for (line, count) in more {
                        into.entry(line).and_modify(|c| *c = c.saturating_add(count)).or_insert(count);
                    }
                }
            }
        }
    }
    e.files = keep;
}

fn from_value(root: Value) -> Result<CovExport> {
    let version = root.get("gcc_version").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let format = root.get("format_version").and_then(|v| v.as_str()).unwrap_or("");
    let mut caveats = Vec::new();

    // A format this reader has not been checked against is said out loud
    // rather than parsed hopefully: the shape may be the same, and the point
    // of a declared version is that a reader can notice when it is not.
    if !format.is_empty() && format != "2" {
        caveats.push(format!(
            "the gcov export declares format_version {format}; this reader was written against version 2, so anything it did not recognise is missing rather than absent"
        ));
    }

    // gcov names files relative to the directory it ran in, and states that
    // directory. llvm-cov gives absolute paths, and everything downstream
    // expects one, so resolve here rather than leaving a relative path to be
    // opened against whatever the reader's working directory happens to be.
    let cwd = root.get("current_working_directory").and_then(|v| v.as_str()).unwrap_or("");

    let files_json = root.get("files").and_then(|v| v.as_array()).ok_or_else(|| anyhow!("gcov export has no `files` array"))?;

    let mut files: Vec<CovFile> = Vec::new();
    let mut functions: Vec<CovFunction> = Vec::new();
    let mut any_branches = false;
    let mut any_conditions = false;

    for f in files_json.iter() {
        let named = f.get("file").and_then(|v| v.as_str()).unwrap_or_default();
        let filename = if named.starts_with('/') || cwd.is_empty() { named.to_string() } else { std::path::Path::new(cwd).join(named).to_string_lossy().into_owned() };

        // ---- line counts, stated rather than derived --------------------
        let mut direct: BTreeMap<u32, u64> = BTreeMap::new();
        let empty = Vec::new();
        let lines = f.get("lines").and_then(|v| v.as_array()).unwrap_or(&empty);
        for l in lines {
            let n = u32_of(l.get("line_number").unwrap_or(&Value::Null));
            if n == 0 {
                continue;
            }
            let c = u64_of(l.get("count").unwrap_or(&Value::Null));
            // A line can appear more than once when several blocks map to
            // it; the line ran if any of them did.
            direct.entry(n).and_modify(|e| *e = (*e).max(c)).or_insert(c);

            // Counted here, not among the branches: gcov lists a condition
            // entry per decision whether or not `-b` was passed, and it is
            // exactly the case where there are *no* branches that needs to
            // know a decision existed. Checking it inside the branch loop
            // meant it was never reached when it mattered.
            if l.get("conditions").and_then(|v| v.as_array()).is_some_and(|c| !c.is_empty()) {
                any_conditions = true;
            }
        }

        // ---- branches, grouped into the decisions they belong to ---------
        //
        // gcov lists every outgoing edge of every block on the line. The two
        // edges of one decision share a `source_block_id`, so grouping by
        // `(line_number, source_block_id)` across all `lines` entries of the
        // file recovers the decisions even when gcov emits multiple `lines`
        // objects for the same line; a block with other than two edges is not
        // a two-way decision and is left alone rather than forced into one.
        let mut by_line: BTreeMap<u32, BTreeMap<u64, Vec<&Value>>> = BTreeMap::new();
        for l in lines {
            let n = u32_of(l.get("line_number").unwrap_or(&Value::Null));
            if n == 0 {
                continue;
            }
            let Some(bs) = l.get("branches").and_then(|v| v.as_array()) else { continue };
            if bs.is_empty() {
                continue;
            }
            any_branches = true;
            let by_block = by_line.entry(n).or_default();
            for b in bs {
                // An exception edge is not a decision outcome; counting it
                // as one would invent a branch the source does not have.
                if b.get("throw").and_then(|v| v.as_bool()).unwrap_or(false) {
                    continue;
                }
                by_block.entry(u64_of(b.get("source_block_id").unwrap_or(&Value::Null))).or_default().push(b);
            }
        }
        let mut branches: Vec<RawBranch> = Vec::new();
        for (n, by_block) in by_line {
            // Numbered per line, in `source_block_id` order across every block
            // on that line, so two decisions on `return a && b;` stay
            // distinguishable even if emitted in separate `lines` entries.
            let mut ordinal: u32 = 0;
            for (_, edges) in by_block {
                if edges.len() != 2 {
                    continue;
                }
                let this = ordinal;
                ordinal += 1;
                // Ordered by destination block, so the pair is stable from
                // run to run; which of the two is "true" is not claimed.
                let mut e = edges;
                e.sort_by_key(|b| u64_of(b.get("destination_block_id").unwrap_or(&Value::Null)));
                branches.push(RawBranch {
                    line_start: n,
                    // gcov reports no columns. Zero says "the whole line"
                    // rather than pointing at a column it never gave.
                    col_start: 0,
                    line_end: n,
                    col_end: 0,
                    true_count: u64_of(e[0].get("count").unwrap_or(&Value::Null)),
                    false_count: u64_of(e[1].get("count").unwrap_or(&Value::Null)),
                    // Same rule as the region above: relative to the
                    // function's own file list, which has one entry.
                    file_id: 0,
                    kind: crate::coverage::KIND_BRANCH,
                    // See the module header: `fallthrough` is a CFG edge,
                    // not a polarity, and the meaning inverts for loops.
                    direction_known: false,
                    // gcov gives no columns, so the ordinal is what keeps two
                    // decisions on one line from sharing a resolution key.
                    decision_ordinal: Some(this),
                });
            }
        }
        branches.sort_by_key(|b| (b.line_start, b.col_start, b.decision_ordinal));

        // ---- functions --------------------------------------------------
        let fns = f.get("functions").and_then(|v| v.as_array()).unwrap_or(&empty);
        for fun in fns {
            let name = fun.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            if name.is_empty() {
                continue;
            }
            let start = u32_of(fun.get("start_line").unwrap_or(&Value::Null));
            let end = u32_of(fun.get("end_line").unwrap_or(&Value::Null)).max(start);
            let count = u64_of(fun.get("execution_count").unwrap_or(&Value::Null));
            functions.push(CovFunction {
                name,
                count,
                regions: vec![Region {
                    line_start: start,
                    col_start: u32_of(fun.get("start_column").unwrap_or(&Value::Null)),
                    line_end: end,
                    col_end: u32_of(fun.get("end_column").unwrap_or(&Value::Null)),
                    count,
                    // Index into this function's own `filenames`, which holds
                    // exactly the file it is in -- not the position of that
                    // file in the export. The consumer resolves a function's
                    // path with `filenames[region.file_id]`, so a global
                    // index here silently emptied the name of every function
                    // outside the first file, and those functions vanished
                    // from the evidence without a word (self-review of #45).
                    file_id: 0,
                    expanded_file_id: 0,
                    kind: KIND_CODE,
                }],
                // Every branch on the function's lines belongs to it. gcov
                // gives each line a `function_name`, but a header line shared
                // between functions has only one, so the range is the surer
                // test of which function a decision sits in.
                branches: branches.iter().filter(|b| b.line_start >= start && b.line_start <= end).copied().collect(),
                mcdc: Vec::new(),
                filenames: vec![filename.clone()],
            });
        }

        files.push(CovFile { filename, segments: Vec::new(), direct_lines: Some(direct) });
    }

    // ---- what this export could not say ---------------------------------
    //
    // Said once, here, so it travels with the evidence rather than depending
    // on someone having read the documentation.
    caveats.push("coverage came from gcov: GCC records no MC/DC, so objective 5 cannot be met from this export and must come from the object-level path".into());
    if !any_branches {
        caveats.push(NO_BRANCH_CAVEAT.into());
    }
    if any_conditions {
        caveats
            .push("the gcov export carries condition coverage (-fcondition-coverage): each condition taken both ways, which is not the independence pairs MC/DC requires".into());
    }
    caveats.push("gcov reports a CFG edge rather than the polarity of a condition, so butterfly does not name either outcome of a gcov decision `true`".into());

    // gcov lists a `conditions` entry per decision whether or not `-b` was
    // passed, so this is what says a decision was there to be measured. It is
    // the difference between "you forgot -b" and "this code branches
    // nowhere", and without it the first is a guess (review of #45).
    Ok(CovExport {
        version: format!("gcov/{version}"),
        files,
        functions,
        caveats,
        // `any_branches` is set from the raw array, before the two-edge
        // filter, so it says the export *had* records -- not that any of them
        // became a decision.
        raw_branch_records: any_branches,
        decision_sites: any_conditions || any_branches,
    })
}
