// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Numbers for the showcase page, read from the evidence at capture time.
//
// The page's rule (#150) is that no number is typed in. A claim in
// `support/features.toml` writes `{{c.gapsTotal}}` and this file is where that
// resolves: from the evidence file the capture run has just produced, on this
// host, from this checkout. A placeholder that does not resolve is an error
// in the builder, so a number cannot quietly go missing either.
//
// When a README figure disagrees with the evidence, the evidence is what gets
// printed (known at the time of writing: examples/cpp-lib, #158).

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');

const pct = (a, b) => (b ? (100 * a / b).toFixed(1) : '0.0');

function fnFacts(f, ev) {
  const kinds = {};
  for (const g of f.object?.gaps || []) kinds[g.kind] = (kinds[g.kind] || 0) + 1;
  const decisions = (f.decisions || []).map(d => ({
    line: d.line,
    conditions: (d.conditions || []).length,
    proven: (d.conditions || []).filter(c => c.independenceCovered).length,
    vectors: (d.vectors || []).length,
  }));
  const sym = f.symbolId == null ? null : ev.symbols[f.symbolId];
  return {
    id: f.id,
    line: f.lineStart,
    blocks: f.object?.blocks.length ?? 0,
    instructions: f.object?.instrTotal ?? 0,
    // Null, not 0, with no trace -- see the note in `evidenceFacts`.
    measuredCovered: f.object?.instrMeasuredCovered ?? null,
    disagreements: f.object?.instrMeasuredCovered == null ? null : (f.object.disagreements || []).length,
    // Basic blocks the trace never entered: no instruction of the block has
    // a non-zero execution count. This is the count V28 quotes, and it is
    // read here rather than recomputed in a claim.
    blocksNeverExecuted: f.object?.instrMeasuredCovered == null ? null
      : f.object.blocks.filter(b => b.instrs.every(i => !(i.measured > 0))).length,
    gaps: (f.object?.gaps || []).length,
    gapKinds: kinds,
    linesCovered: f.linesCovered,
    linesTotal: f.linesTotal,
    branches: (f.branches || []).length,
    decisions,
    requirements: f.requirements || [],
    commits: (f.commits || []).length,
    variables: Object.fromEntries((f.variables || []).map(v => [v.name, {
      accesses: (v.accesses || []).length,
      // `46r 46w`: the line and the kind of each touch, in the order the
      // panel lists them -- static instruction order, not execution order.
      sites: (v.accesses || []).map(a => `${a.line ?? '-'}${a.access[0]}`).join(' '),
      unattributed: v.unattributed || null,
    }])),
    stack: sym?.stack ? {
      frame: sym.frame?.bytes ?? null,
      worstCase: sym.stack.worstCase,
      bounded: sym.stack.bounded,
      reason: sym.stack.reason,
      chain: sym.stack.path.map(i => ev.symbols[i].name).join(' → '),
    } : null,
  };
}

function evidenceFacts(ev) {
  const s = ev.summary;
  // First wins, and how many there were. A duplicated name is real -- two
  // `static` functions of the same name in different translation units, which
  // examples/c-lib exists to exercise (#36) -- so the count travels with the
  // entry and `fill()` refuses a placeholder that would quote one of them at
  // random (#212 S22).
  const fn = {};
  const seen = {};
  for (const f of ev.functions) seen[f.name] = (seen[f.name] || 0) + 1;
  for (const f of ev.functions) if (!(f.name in fn)) fn[f.name] = {...fnFacts(f, ev), duplicates: seen[f.name]};
  const duplicated = Object.keys(seen).filter(n => seen[n] > 1).sort();
  const global = {};
  for (const g of ev.globals || []) {
    global[g.name] = {
      readers: g.readers.length,
      writers: g.writers.length,
      writerFiles: new Set(g.writers.map(id => ev.functions[id]?.fileId)).size,
    };
  }
  const tests = new Set();
  for (const f of ev.functions) for (const t of f.tests || []) tests.add(t.name ?? t);
  // What the run counted at each source line and branch, and nothing about
  // the object code: two variants with the same digest have identical source
  // coverage by construction, which is the comparison V14 makes.
  const source = ev.files.map(f => [f.display, f.sha256, f.lines.map(l => l.count)]);
  const branches = ev.functions.map(f => [f.name, (f.branches || []).map(b => [b.line, b.col, b.trueCount, b.falseCount])]);
  return {
    arch: ev.provenance.arch,
    objectFormat: ev.provenance.objectFormat,
    isAarch64: ev.provenance.arch === 'aarch64',
    compiler: ev.provenance.buildInfo?.compiler || null,
    caveats: ev.provenance.caveats.length,
    inputs: ev.provenance.inputs.length,
    files: s.files,
    functions: s.functions,
    functionsExecuted: s.functionsExecuted,
    linesCovered: s.linesCovered,
    linesTotal: s.linesTotal,
    linesPct: pct(s.linesCovered, s.linesTotal),
    branchesCovered: s.branchesCovered,
    branchesTotal: s.branchesTotal,
    branchesPct: pct(s.branchesCovered, s.branchesTotal),
    conditionsTotal: s.conditionsTotal,
    conditionsProven: s.conditionsIndependenceCovered,
    mcdcVectors: ev.functions.some(f => (f.decisions || []).some(d => (d.vectors || []).length > 0)),
    instructionsTotal: s.instructionsTotal,
    instructionsProjectedCovered: s.instructionsProjectedCovered,
    instructionsCompilerGenerated: s.instructionsCompilerGenerated ?? 0,
    generatedOrigins: Object.keys(s.compilerGeneratedByOrigin || {}).length,
    measured: s.instructionsMeasuredCovered != null,
    // ---- what an imported trace establishes (#164, V28) -----------------
    //
    // `null` wherever there is no trace, never 0. `fill()` refuses a
    // placeholder that resolves to null, so a claim that quotes one of these
    // from an example with no trace STOPS THE BUILD instead of publishing a
    // zero that reads as a measurement. Every other example in this
    // repository is untraced, so that is the case these have to be safe in.
    //
    // `outcomeEdgesTotal` is the exception and is always a number: it counts
    // the conditional edges in the object code, which exist whether or not
    // anything ran.
    outcomeEdgesTotal: s.outcomeEdgesTotal ?? 0,
    instructionsMeasuredCovered: s.instructionsMeasuredCovered ?? null,
    outcomeEdgesMeasured: ev.measured ? (s.outcomeEdgesMeasured ?? 0) : null,
    disagreements: ev.measured ? (s.disagreements ?? 0) : null,
    // The two ways the projection and the trace disagree, counted apart:
    // instructions the projection calls covered that never ran, and
    // instructions that ran which the projection could not account for.
    ...(() => {
      if (!ev.measured) return {projectedNotExecuted: null, executedUnknown: null};
      const k = {projectedNotExecuted: 0, executedUnknown: 0};
      for (const f of ev.functions) for (const d of f.object?.disagreements || []) {
        if (d.kind in k) k[d.kind]++;
      }
      return k;
    })(),
    // The trace itself, as `crates/ingest/src/trace.rs` recorded it.
    traceSource: ev.measured?.source ?? null,
    traceFormat: ev.measured?.format ?? null,
    blockExecutions: ev.measured?.blockExecutions ?? null,
    transitions: ev.measured?.transitions ?? null,
    transitionsMatched: ev.measured?.transitionsMatched ?? null,
    functionsTraced: ev.measured?.functionsExecuted ?? null,
    gapsTotal: s.gapsTotal,
    gapKinds: Object.keys(s.gapsByKind || {}).length,
    stackWorstCase: s.stackWorstCase ?? null,
    stackUnbounded: s.stackUnbounded ?? 0,
    globals: s.globals ?? (ev.globals || []).length,
    dataRefs: s.dataRefs ?? 0,
    commits: ev.changes ? new Set(ev.functions.flatMap(f => (f.commits || []).map(c => c.sha ?? c.hash ?? c.id ?? JSON.stringify(c)))).size : 0,
    baselineStatus: ev.baseline?.status ?? null,
    baselineSourceChanged: (ev.baseline?.functionsSourceChanged || []).join(', '),
    sourceCoverageDigest: crypto.createHash('sha256').update(JSON.stringify([source, branches])).digest('hex'),
    duplicateFunctionNames: duplicated.join(', '),
    duplicateFunctions: duplicated.length,
    fn,
    global,
  };
}

/// Facts for one fixture.
///
/// The top-level numbers are those of the FIRST evidence path the fixture
/// lists -- `{{c-variants.gapsTotal}}` is `-O0`, because that path is written
/// first. That was implicit; it is now recorded in `of`, so a claim can name
/// the file its number came from and a reordering of `evidence` is visible.
/// When a fixture serves several, each is also available by variant name,
/// with whether their source coverage is the same.
function fixtureFacts(paths) {
  const evs = paths.map(p => JSON.parse(fs.readFileSync(p, 'utf8')));
  const out = evidenceFacts(evs[0]);
  out.of = path.basename(paths[0]);
  if (evs.length > 1) {
    out.variant = {};
    paths.forEach((p, i) => {
      const name = /evidence-([^/]+)\.json$/.exec(p)?.[1] ?? String(i);
      out.variant[name] = evidenceFacts(evs[i]);
    });
    const digests = new Set(Object.values(out.variant).map(v => v.sourceCoverageDigest));
    out.variants = evs.length;
    out.variantNames = Object.keys(out.variant).join(', ');
    out.sourceCoverageIdentical = digests.size === 1;
  }
  return out;
}

/// Resolve `{{a.b.c}}` against the facts. Unresolved is an error by design.
///
/// "Unresolved" includes the empty string and a non-finite number. A claim
/// that renders "reported as not bounded because of ." is worse than a build
/// that stops: the reader has no way to know a value went missing (#212 S20).
function fill(text, facts, where) {
  return text.replace(/\{\{([^}]+)\}\}/g, (_, expr) => {
    const keys = expr.trim().split('.');
    let v = facts;
    for (let i = 0; i < keys.length; i++) {
      // The evidence can hold two functions of one name -- a `static` in two
      // translation units, which this repository has fixtures for (#36) --
      // and `fn` keeps the first. Quoting a number from an arbitrary one of
      // them is not something the page may do silently (#212 S22).
      if (keys[i - 1] === 'fn' && v && v[keys[i]] && v[keys[i]].duplicates > 1) {
        throw new Error(`${where}: {{${expr}}} names \`${keys[i]}\`, and this evidence holds ${v[keys[i]].duplicates} functions with that name (internal linkage, #36). The value would be whichever one came first.`);
      }
      v = v == null ? undefined : v[keys[i]];
    }
    if (v === undefined || v === null || typeof v === 'object' || v === '' || (typeof v === 'number' && !Number.isFinite(v))) {
      throw new Error(`${where}: {{${expr}}} does not resolve to a value in the captured facts (got ${JSON.stringify(v)})`);
    }
    return String(v);
  });
}

module.exports = {fixtureFacts, evidenceFacts, fill};
