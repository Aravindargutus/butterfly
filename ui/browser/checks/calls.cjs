// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Two things a screenshot makes obvious and no assertion caught (#214).
//
//   1. The label on a call-graph node reached only through a candidate --
//      "candidate, not observed" -- was cut off inside the node at the
//      default width, so the one statement that stops a hypothesis reading
//      as a fact was not on the node that carries the hypothesis. The text
//      also ran PAST the node's own border, which is the same measurement
//      being wrong in the other direction.
//
//   2. The Gates box prints "the equivalent command", and it printed the
//      evidence path exactly as the server was given it. Started with an
//      absolute path -- what a script or an IDE produces -- the operator's
//      own directory layout appeared in the UI and in every screenshot of
//      it.
//
// # Why both live in a browser check, and why the evidence is hand-written
//
// Both are properties of the RENDERED page: a truncation is a measurement
// against a font the layout engine chose, and the Gates command is assembled
// by the server and read back by the viewer. Neither is visible to
// ui/callgraph.test.cjs, which drives the graph MODEL with no layout at all,
// and that is exactly why they were found in a screenshot.
//
// The evidence is written here rather than ingested from an example so that
// the check is independent of the host's compiler and architecture: the
// nodes under test are an indirect call's candidate target, an indirect call
// the trace resolved, and a callee outside the covered source set, none of
// which an arbitrary host is guaranteed to build (the lesson of #89).

const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const {scratch, serve, browser, waitFor} = require('../harness.cjs');

const SRC = [
  '#include <stdio.h>',
  '',
  'typedef int (*limiter_t)(int);',
  '',
  'int apply_limit_to_channel(int v) { return v > 100 ? 100 : v; }',
  '',
  'void dispatch(limiter_t f, int v) {',
  '    printf("%d\\n", f(v));',
  '}',
];

const INSTR = (addr, mnemonic, operands, line, flags = []) =>
  ({addr, mnemonic, operands, line, fileId: 0, isStmt: true, projected: 'covered', flags, gapIds: []});

/// The object half of a function: three blocks, all projected covered. The
/// graph does not read it, but the viewer does before any tab renders.
const object = (symbol, address) => ({
  symbol, address, size: 0x40,
  blocks: [
    {id: 0, address, kind: 'entry', lines: [5], projected: 'covered',
      instrs: [INSTR(address, 'stp', 'x29, x30, [sp, #-16]!', 5)],
      succs: [{to: 1, kind: 'fallthrough', projected: 'covered'}]},
    {id: 1, address: address + 8, kind: 'normal', lines: [5], projected: 'covered',
      instrs: [INSTR(address + 8, 'cmp', 'w0, #100', 5)],
      succs: [{to: 2, kind: 'taken', projected: 'covered'}]},
    {id: 2, address: address + 16, kind: 'exit', lines: [5], projected: 'covered',
      instrs: [INSTR(address + 16, 'ret', '', 5, ['return'])], succs: []},
  ],
  gaps: [], instrTotal: 3, instrProjectedCovered: 3, instrProjectedUncovered: 0, instrUnknown: 0,
  instrMeasuredCovered: null, disagreements: [],
  outcomeEdgesTotal: 0, outcomeEdgesMeasured: null,
  unreachableBlocks: [], compilerGenerated: {},
});

const fn = (id, name, lineStart, extra) => ({
  id, name, mangled: `_${name}`, fileId: 0, lineStart, lineEnd: lineStart,
  count: 1, instantiations: 1, requirements: [], lineRequirements: {},
  linesTotal: 1, linesCovered: 1, branches: [], decisions: [],
  complexity: {
    cyclomaticObject: 2, decisions: 0, conditions: 0, maxConditionsPerDecision: 0,
    objectBlocks: 3, objectEdges: 2, objectInstructions: 3, objectConditionalBranches: 0, callsOut: 0,
  },
  callees: [], callers: [],
  object: object(`_${name}`, 0x1000 + id * 0x100),
  symbolId: null, dataRefs: [], variables: [], constructs: {}, inlined: [],
  foldedCalls: [], indirectCalls: [], pointerCallers: [], commits: [],
  ...extra,
});

/// The symbol behind a function. The call graph does not read these, but
/// `Evidence::validate` resolves every `symbolId` an indirect call names.
const symbol = (id, name) => ({
  id, name, mangled: `_${name}`, address: 0x1000 + id * 0x100, size: 0x40,
  section: '__text', functionId: id,
  frame: {bytes: 16, source: 'prologue', dynamic: false},
  callees: [], indirectCalls: id === 0 ? 2 : 0,
  stack: {worstCase: 16, path: [id], bounded: true, reason: null, depth: 0},
  measured: null, dataRefs: [], addressTakenBy: [], callers: [],
});

/// One evidence document holding each node kind whose label is longer than
/// the node is wide:
///
///   * `dispatch` calls `printf`, which has no object code here, so it is
///     drawn as external -- "outside the covered source set";
///   * it calls through a function pointer whose candidate target is
///     `apply_limit_to_channel` -- "candidate, not observed";
///   * and through a second pointer the trace resolved to `clamp_channel`
///     -- "seen in the trace".
///
/// The names are long on purpose: the node's NAME row has a budget of its
/// own, and it shares a baseline with the expand control.
function evidence() {
  const lines = SRC.map((_, i) => ({count: i + 1 === 5 || i + 1 === 8 ? 1 : null}));
  return {
    schema: 'butterfly-evidence/1',
    provenance: {
      tool: 'butterfly', toolVersion: '0.1.0', createdUnix: 1700000000,
      host: 'browser-check-fixture', inputs: [], buildInfo: {},
      coverageExportVersion: 'fixture', objectFormat: 'macho', arch: 'aarch64',
      caveats: ['hand-written fixture for the call-graph browser checks (#214); not produced by ingest'],
    },
    summary: {
      files: 1, functions: 3, functionsExecuted: 3,
      linesTotal: 2, linesCovered: 2,
      branchesTotal: 0, branchesCovered: 0, decisionsTotal: 0, decisionsMcdcCovered: 0,
      conditionsTotal: 0, conditionsIndependenceCovered: 0,
      instructionsTotal: 9, instructionsProjectedCovered: 9, instructionsProjectedUncovered: 0,
      instructionsUnknown: 0, gapsTotal: 0, gapsByKind: {},
    },
    files: [{
      id: 0, path: '/fixture/dispatch.c', display: 'dispatch.c', sha256: '0'.repeat(64),
      text: SRC, lines, functionIds: [0, 1, 2], linesTotal: 2, linesCovered: 2,
    }],
    functions: [
      // dispatch spans lines 7-9 (signature, the printf body on line 8, the
      // closing brace); line 8 is its one executable, covered line. The `fn`
      // helper defaults lineEnd to lineStart, which would leave the range at
      // 7-7 -- a range that holds no executable line, so the per-function
      // derivation gate (#255) rightly rejects the 1/1 counters as unbacked.
      // Pin lineEnd to the real function end so the range covers line 8 and
      // the declared 1/1 is what its primitive line records derive.
      fn(0, 'dispatch', 7, {
        lineEnd: 9,
        callees: [{
          symbol: 'printf', functionId: null, exercised: 'yes', sourceCalls: 1,
          sites: [{addr: 0x100c, line: 8, measured: null}],
        }],
        indirectCalls: [
          {addr: 0x1004, line: 8, measuredCount: null, measured: [],
            candidates: [{name: 'apply_limit_to_channel', functionId: 1, symbolId: 1, count: 0}]},
          {addr: 0x1008, line: 8, measuredCount: 2, candidates: [],
            measured: [{name: 'clamp_channel', functionId: 2, symbolId: 2, count: 2}]},
        ],
      }),
      fn(1, 'apply_limit_to_channel', 5, {}),
      fn(2, 'clamp_channel', 5, {}),
    ],
    // `Evidence::validate` resolves every symbolId, so the two pointer
    // targets above need symbols to point at.
    symbols: [0, 1, 2].map(id => symbol(id, ['dispatch', 'apply_limit_to_channel', 'clamp_channel'][id])),
    globals: [],
  };
}

/// What each node kind must be readable as. The sentence, not a prefix of it.
const MUST_READ = {
  'ptr-cand': 'candidate, not observed',
  'ptr-meas': 'seen in the trace',
  ext: 'outside the covered source set',
};

async function run({out}) {
  // `realpathSync`, because the server relativises against `getcwd()`, which
  // is resolved: on macOS `scratch()` returns a path under /var, a symlink
  // to /private/var, and a path and a working directory that only look
  // alike do not share a prefix. `for_display` deliberately does not
  // canonicalize -- resolving symlinks would print a path the operator never
  // typed -- so it is this check that has to hand it a real one, which is
  // also the situation the defect was reported in.
  const dir = fs.realpathSync(scratch('calls'));
  const evPath = path.join(dir, 'evidence-calls.json');
  fs.writeFileSync(evPath, JSON.stringify(evidence(), null, 2));

  const results = [];
  let page;
  const check = async (name, fn_) => {
    try {
      const detail = await fn_();
      results.push({name, pass: true, ...(detail ? {detail} : {})});
    } catch (e) {
      const shot = path.join(out, `calls-${name}.png`);
      await page?.screenshot({path: shot, fullPage: true}).catch(() => {});
      results.push({name, pass: false, error: e.message, screenshot: shot});
    }
  };

  /// Every node as the page actually draws it: the text that reached the
  /// screen, its measured width, and the leftmost edge of the controls that
  /// share its baseline.
  const nodes = () => page.evaluate(() => [...document.querySelectorAll('.bf-node')].map(g => {
    const rect = g.querySelector('rect');
    const texts = [...g.querySelectorAll('text')];
    const sub = g.querySelector('text.sub');
    const name = texts[0];
    const at = e => Number(e.getAttribute('x'));
    const rowOf = e => Number(e.getAttribute('y'));
    // A control is right-anchored, so it occupies [x - length, x].
    const leftEdgeOn = row => Math.min(
      ...texts.filter(t => t !== name && t !== sub && Math.abs(rowOf(t) - row) < 8)
        .map(t => at(t) - t.getComputedTextLength()),
      Number(rect.getAttribute('width')) - 10);
    return {
      cls: g.getAttribute('class'),
      name: name.textContent, nameRight: at(name) + name.getComputedTextLength(),
      nameLimit: leftEdgeOn(rowOf(name)),
      sub: sub.textContent, subRight: at(sub) + sub.getComputedTextLength(),
      subLimit: leftEdgeOn(rowOf(sub)),
      title: g.querySelector('title').textContent,
      w: Number(rect.getAttribute('width')),
    };
  }));

  let srv, br;
  try {
    br = await browser();

    /* ---------------------- 1. the call-graph labels --------------------- */
    // Served from the repository root, as every other suite is: this half
    // says nothing about paths.
    srv = await serve(evPath, {readOnly: true, dir, out});
    page = await br.newPage({viewport: {width: 1460, height: 900}});
    const pageErrors = [];
    page.on('pageerror', e => pageErrors.push(e.message));
    await page.goto(`${srv.url}/#/fn/0/calls`);
    await page.locator('.bf-node').first().waitFor();

    await check('a-candidate-node-says-it-is-a-candidate-on-the-node', async () => {
      const seen = await nodes();
      const found = {};
      for (const [cls, sentence] of Object.entries(MUST_READ)) {
        const n = seen.find(x => x.cls.split(' ').includes(cls));
        assert.ok(n, `the fixture draws a ${cls} node; got ${JSON.stringify(seen.map(x => x.cls))}`);
        // The whole sentence, not a prefix of it. This is the defect as
        // reported: the node read "via pointer #1 · candidate, no…".
        assert.ok(n.sub.includes(sentence),
          `a ${cls} node must read "${sentence}" on the node itself, and reads "${n.sub}"`);
        found[cls] = n.sub;
      }
      return found;
    });

    await check('no-label-is-drawn-past-the-node-it-belongs-to', async () => {
      // The other half of the same wrong measurement. `n.w / 6` counted the
      // whole node although the text starts 10px in, so the candidate label
      // was drawn to x=196.6 on a 190-wide box and the external node's to
      // x=190.6 -- over the border, and over the controls on their row.
      const over = (await nodes()).flatMap(n => [
        n.nameRight > n.nameLimit ? `${n.name}: the name reaches ${n.nameRight.toFixed(1)}, past ${n.nameLimit.toFixed(1)}` : null,
        n.subRight > n.subLimit ? `${n.name}: "${n.sub}" reaches ${n.subRight.toFixed(1)}, past ${n.subLimit.toFixed(1)}` : null,
      ].filter(Boolean));
      assert.deepEqual(over, [], 'every label stops before the node border and the controls on its row');
    });

    await check('a-label-that-does-not-fit-is-cut-between-statements-not-inside-one', async () => {
      // What makes the sentence survive: the label is `·`-separated and
      // whole segments are dropped, so what is on the node is always a
      // complete statement and the <title> carries the rest.
      const cand = (await nodes()).find(n => n.cls.split(' ').includes('ptr-cand'));
      assert.match(cand.sub, /^candidate, not observed(| …)$/,
        `the node carries the statement whole or the statement and an ellipsis, got "${cand.sub}"`);
      assert.ok(cand.title.includes('via pointer #'),
        `and the reference it dropped is still on the node's tooltip, got "${cand.title}"`);
      return cand.sub;
    });

    await check('no-page-errors-in-the-call-graph', async () => assert.deepEqual(pageErrors, []));
    await page.close();
    srv.child.kill();

    /* ------------------- 2. the path in the Gates command ---------------- */
    const gateCommand = async () => {
      await page.goto(`${srv.url}/#/overview`);
      await waitFor(page, () => document.querySelector('.gate-cmd code')?.textContent,
        undefined, {what: 'the Gates box to print its equivalent command'});
      return page.evaluate(() => document.querySelector('.gate-cmd code').textContent);
    };
    // The viewer only prints the command when a floor is set, so one is.
    // `srv` is read by `gateCommand` at call time, which is the server this
    // has just started.
    const withGates = async () => {
      page = await br.newPage({viewport: {width: 1460, height: 900}});
      await page.addInitScript(() => localStorage.setItem('sl-gates', '{"min_lines":"50"}'));
      return gateCommand();
    };

    // Started from the directory the evidence is in, with the absolute path
    // a script or an IDE would produce.
    srv = await serve(evPath, {readOnly: true, dir, out, cwd: dir});
    await check('an-absolute-path-under-the-working-directory-is-printed-relative-to-it', async () => {
      const cmd = await withGates();
      assert.ok(cmd.includes('butterfly check evidence-calls.json'),
        `the command must name the evidence as a reader would, got: ${cmd}`);
      assert.ok(!cmd.includes(dir),
        `and must not carry the operator's directory into the UI (${dir}), got: ${cmd}`);
      return cmd;
    });
    await page.close();
    srv.child.kill();

    // ...and from somewhere the evidence is NOT under. There is no relative
    // form a reader could run, so the path that was given is what is printed:
    // the fix must not invent one.
    const elsewhere = fs.realpathSync(scratch('calls-elsewhere'));
    srv = await serve(evPath, {readOnly: true, dir, out, cwd: elsewhere});
    await check('a-path-outside-the-working-directory-is-not-rewritten-into-one-that-does-not-exist', async () => {
      const cmd = await withGates();
      assert.ok(cmd.includes(evPath),
        `the path the server was given is the only one it may print, got: ${cmd}`);
      return cmd;
    });
  } finally {
    await page?.close().catch(() => {});
    await br?.close();
    srv?.child.kill();
  }
  return results;
}

module.exports = {run, evidence};
