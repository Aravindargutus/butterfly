// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/review_fixes.test.cjs
//
// Regression tests for the view/UI/showcase fixes:
//   - condKey decisionOrdinal support (#6.7)
//   - variant-switch / resolution-save race & File View route preservation
//   - freshness refresh after resolution edits & stale requirements banner
//   - insFileId cross-file guards in renderSource / renderObject / fileIndex
//   - C++ constructor/destructor f.variants aggregation in objTotals & renderGaps
//   - reqChip activeReqChip cleanup
//   - showcase toml.cjs prototype safety & build.cjs shot.file validation
'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function makeElement(tag) {
  const el = {
    tag,
    nodeType: 1,
    children: [],
    attrs: {},
    dataset: {},
    style: {},
    isConnected: true,
    _listeners: {},
    classList: {
      _set: new Set(),
      add(...cs) { for (const c of cs) this._set.add(c); },
      remove(...cs) { for (const c of cs) this._set.delete(c); },
      toggle(c, on) { if (on === undefined) { if (this._set.has(c)) this._set.delete(c); else this._set.add(c); } else if (on) this._set.add(c); else this._set.delete(c); },
      contains(c) { return this._set.has(c); },
    },
    append(...c) { this.children.push(...c.filter(x => x != null)); },
    before(...c) { this._before = (this._before || []).concat(c); },
    setAttribute(k, v) { this.attrs[k] = v; if (k === 'class') this.classList._set = new Set(String(v).split(/\s+/).filter(Boolean)); },
    addEventListener(ev, fn) { (this._listeners[ev] ||= []).push(fn); },
    focus() { this._focused = true; },
    getBoundingClientRect() { return { left: 10, top: 10, bottom: 30, right: 80 }; },
    set innerHTML(t) { this._html = String(t); this.children = []; },
    set textContent(t) { this._html = String(t); this.children = []; },
    get textContent() { return (this._html || '') + this.children.map(c => c.textContent || '').join(''); },
  };
  return el;
}

function loadApp(extra = {}) {
  const byId = new Map();
  const context = vm.createContext({
    console: { error() {} },
    URL,
    URLSearchParams,
    setTimeout,
    clearTimeout,
    alert() {},
    window: { innerWidth: 1200, innerHeight: 800, addEventListener() {} },
    localStorage: { getItem: k => (k === 'sl-mode' ? (extra.mode || 'projected') : null), setItem() {} },
    sessionStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    location: { hash: '#/overview', pathname: '/', search: '' },
    history: { replaceState() {} },
    fetch: extra.fetch || (async () => ({ ok: true, status: 200, headers: { get: () => null }, json: async () => ({ entries: [] }), text: async () => '' })),
    document: {
      body: { innerHTML: '', append(el) { if (el.attrs && el.attrs.id) byId.set(el.attrs.id, el); } },
      addEventListener() {},
      getElementById: id => byId.get(id) || null,
      querySelector: sel => {
        if (sel === '#modal') {
          if (!byId.has('modal')) byId.set('modal', makeElement('div'));
          return byId.get('modal');
        }
        return null;
      },
      createElement: makeElement,
      createTextNode: text => ({ nodeType: 3, children: [], textContent: String(text) }),
    },
  });
  const source = fs.readFileSync(path.join(__dirname, 'app.js'), 'utf8');
  vm.runInContext(source.slice(0, source.lastIndexOf('\ninit().catch')), context);
  return { context, run: code => vm.runInContext(code, context), byId };
}

test('condKey uses branchSite(d) so macro decisionOrdinal keys match backend', () => {
  const { run } = loadApp();
  assert.equal(run(`condKey(2, { line: 10, col: 4 }, 1)`), 'cond:2:10:4:1');
  assert.equal(run(`condKey(2, { line: 10, col: 4, decisionOrdinal: null }, 1)`), 'cond:2:10:4:1');
  assert.equal(run(`condKey(2, { line: 10, col: 4, decisionOrdinal: 0 }, 1)`), 'cond:2:10:#0:1');
  assert.equal(run(`condKey(2, { line: 10, col: 4, decisionOrdinal: 2 }, 0)`), 'cond:2:10:#2:0');
});

test('api() preserves File View via route() and refreshes freshness after resolution edits', async () => {
  const calls = [];
  const { run } = loadApp({
    fetch: async url => {
      calls.push(url);
      if (url.startsWith('/api/resolutions')) return { ok: true, status: 200, headers: { get: () => null }, json: async () => ({ entries: [{ key: 'line:0:2', category: 'a', justification: 'ok', author: 'r', date: '2026-01-01' }] }) };
      if (url.startsWith('/api/freshness')) return { ok: true, status: 200, headers: { get: () => null }, json: async () => ({ stale: true, reasons: ['1 resolution(s) written'] }) };
      throw new Error('unexpected ' + url);
    },
  });
  let routed = 0, overviewed = 0;
  run(`
    S.variant = 'O0';
    S.fnId = null;
    S.fileId = 0;
    S.ev = { changes: { head: 'abc', commits: [], tickets: [], unticketed: [], caveats: [] }, summary: {}, files: [], functions: [] };
    renderTop = () => {};
    renderSidebar = () => {};
    renderOverview = () => { __overviewed++; };
    route = () => { __routed++; };
  `);
  const ctx = run('this');
  ctx.__routed = 0;
  ctx.__overviewed = 0;
  const ok = await run(`api('/api/resolutions', { key: 'line:0:2' })`);
  assert.equal(ok, true);
  assert.equal(ctx.__routed, 1, 'File View rerenders via route() instead of renderOverview()');
  assert.equal(ctx.__overviewed, 0);
  // Let microtask for loadFreshness settle
  await new Promise(r => setImmediate(r));
  assert.ok(calls.some(u => u.startsWith('/api/freshness?v=O0')), `freshness refreshed after edit: ${calls}`);
  assert.equal(run(`S.fresh && S.fresh.stale`), true);
});

test('api() response arriving after a variant switch does not overwrite the new variant S.res', async () => {
  let resolvePost;
  const postedUrls = [];
  const { run } = loadApp({
    fetch: async url => {
      postedUrls.push(url);
      await new Promise(r => { resolvePost = r; });
      return { ok: true, status: 200, headers: { get: () => null }, json: async () => ({ entries: [{ key: 'fn:0', category: 'a', justification: 'from-O0', author: 'r', date: '2026-01-01' }] }) };
    },
  });
  run(`
    S.variant = 'O0';
    S.res = new Map();
    renderTop = () => {};
    renderSidebar = () => {};
    route = () => {};
  `);
  const p = run(`api('/api/resolutions', { key: 'fn:0' }, true, { variant: 'O0', gen: variantGen })`);
  // Simulate a variant switch committing while the POST is in flight
  run(`
    variantGen++;
    S.variant = 'O2';
    S.res = new Map([['fn:9', { key: 'fn:9', justification: 'from-O2' }]]);
  `);
  resolvePost();
  await p;
  assert.deepEqual(postedUrls, ['/api/resolutions?v=O0']);
  assert.equal(run(`S.res.has('fn:9')`), true, 'O2 resolutions are preserved');
  assert.equal(run(`S.res.has('fn:0')`), false, 'late O0 response must not overwrite O2 S.res');
});

test('cross-file inlined instructions do not collide on line number in renderSource, renderObject, or fileIndex', () => {
  const { run } = loadApp({ mode: 'measured' });
  run(`
    S.res = new Map();
    S.ev = {
      measured: { format: 'qemu', caveats: ['trace caveat'] },
      baseline: null,
      files: [
        { id: 0, display: 'main.c', functionIds: [0],
          text: ['int main(void) {', '  int x = 0;', '  int folded = 1;', '  return x;', '}'],
          lines: [{ count: 1 }, { count: 1 }, { count: 1 }, { count: 1 }, { count: 1 }] },
        { id: 1, display: 'hdr.h', functionIds: [],
          text: ['static inline int h(void) {', '  return 42;', '  return 99;', '}'],
          lines: [{ count: 500 }, { count: 500 }, { count: 500 }, { count: 500 }] },
      ],
      functions: [{
        id: 0, name: 'main', fileId: 0, count: 1, requirements: [], lineRequirements: {},
        inlined: [{ name: 'h', functionId: null, callLine: 2, ranges: [], instrCount: 2, depth: 0 }],
        branches: [], decisions: [], lineStart: 1, lineEnd: 5, linesTotal: 5, linesCovered: 5,
        object: {
          symbol: 'main', address: 0x1000, size: 12, gaps: [], disagreements: [],
          instrTotal: 3, instrProjectedCovered: 3, instrProjectedUncovered: 0, instrUnknown: 0,
          instrMeasuredCovered: 3, compilerGenerated: {},
          blocks: [{
            id: 0, address: 0x1000, kind: 'entry', succs: [],
            instrs: [
              { addr: 0x1000, line: 2, mnemonic: 'mov', operands: 'w0, #0', flags: [], gapIds: [], projected: 'covered', measured: 1 },
              { addr: 0x1004, line: 2, fileId: 1, inlined: 0, origin: 'inlined:h', mnemonic: 'mov', operands: 'w1, #42', flags: [], gapIds: [], projected: 'covered', measured: 500 },
              { addr: 0x1008, line: 3, fileId: 1, inlined: 0, origin: 'inlined:h', mnemonic: 'mov', operands: 'w2, #99', flags: [], gapIds: [], projected: 'covered', measured: 500 },
            ],
          }],
        },
      }],
    };
  `);
  // 1. fileIndex density must not count hdr.h instructions on main.c lines 2 and 3
  assert.equal(run(`fileIndex(0).density[2]`), 1, 'only main.c instruction counted on L2');
  assert.equal(run(`fileIndex(0).density[3]`), 0, 'hdr.h L3 must not count as main.c L3');

  // 2. renderSource exec column must show 1 (not 500) on main.c L2, and empty on main.c L3
  run(`SP = h('div', {}); renderSource(SP, S.ev.functions[0]);`);
  const execL2 = run(`SP.children[1].children[0].children.find(r => r.attrs['data-line'] === 2).children[2].textContent`);
  const execL3 = run(`SP.children[1].children[0].children.find(r => r.attrs['data-line'] === 3).children[2].textContent`);
  assert.equal(execL2, '1', 'inlined hdr.h:2 measured count 500 must not overwrite main.c:2');
  assert.equal(execL3, '', 'inlined hdr.h:3 measured count 500 must not appear on main.c:3');

  // 3. renderObject orphan lines must still include main.c:3 (even though hdr.h:3 has an instruction)
  //    and hdr.h instruction rows must NOT set data-line
  run(`OP = h('div', {}); renderObject(OP, S.ev.functions[0]);`);
  const orphanTexts = run(`OP.children[OP.children.length - 1].children.filter(r => /\\borphan\\b/.test(r.attrs.class || '')).map(r => r.textContent)`);
  assert.ok(orphanTexts.some(t => t.includes('L3') && t.includes('folded')), `main.c:3 should be listed as orphan: ${JSON.stringify(orphanTexts)}`);
  const hdrInsDataLines = run(`OP.children[OP.children.length - 1].children.filter(r => /^i /.test(r.attrs.class || '') && (r.attrs.title || '').startsWith('hdr.h:')).map(r => r.attrs['data-line'])`);
  assert.deepEqual(hdrInsDataLines, ['', ''], 'inlined hdr.h instructions must not carry main.c data-line');
});

test('objTotals and renderGaps aggregate outcomeEdges, compilerGenerated, and disagreements across f.variants', () => {
  const { run } = loadApp({ mode: 'measured' });
  run(`
    S.res = new Map();
    S.ev = {
      measured: { format: 'qemu', caveats: ['trace note'] },
      files: [{ id: 0, display: 'a.cpp', text: ['A::~A() {}'], lines: [{ count: 1 }] }],
      functions: [{
        id: 0, name: 'A::~A()', fileId: 0, count: 1, requirements: [], lineRequirements: {}, branches: [], decisions: [],
        lineStart: 1, lineEnd: 1, linesTotal: 1, linesCovered: 1,
        object: {
          symbol: '_ZN1AD2Ev', address: 0x1000, size: 4, gaps: [], disagreements: [],
          instrTotal: 2, instrProjectedCovered: 2, instrProjectedUncovered: 0, instrUnknown: 0, instrMeasuredCovered: 2,
          outcomeEdgesTotal: 2, outcomeEdgesMeasured: 2, compilerGenerated: { prologue: 1 }, blocks: [],
        },
        variants: [{
          symbol: '_ZN1AD0Ev', variant: 'D0', address: 0x2000, size: 4, gaps: [],
          disagreements: [{ kind: 'executedUnknown', addr: 0x2000, line: 1, measured: 3 }],
          instrTotal: 3, instrProjectedCovered: 0, instrProjectedUncovered: 0, instrUnknown: 3, instrMeasuredCovered: 1,
          outcomeEdgesTotal: 4, outcomeEdgesMeasured: 1, compilerGenerated: { epilogue: 2 }, blocks: [],
        }],
      }],
    };
  `);
  const tot = JSON.parse(run(`JSON.stringify(objTotals(S.ev.functions[0]))`));
  assert.equal(tot.outcomeEdgesTotal, 6);
  assert.equal(tot.outcomeEdgesMeasured, 3);
  assert.equal(tot.compilerGeneratedTotal, 3);
  assert.equal(tot.disagreements.length, 1);

  run(`GP = h('div', {}); renderGaps(GP, S.ev.functions[0]);`);
  assert.match(run(`GP.textContent`), /Projection vs trace/, 'variant disagreements are shown in Gaps tab');
});

test('reqChip clears data-open when switching chips or hiding, preventing stale overwrites', () => {
  const { run } = loadApp();
  run(`
    C1 = reqChip('REQ-1');
    C2 = reqChip('REQ-2');
    C1._listeners.mouseenter[0]();
  `);
  assert.equal(run(`C1.dataset.open`), '1');
  run(`C2._listeners.mouseenter[0]();`);
  assert.equal(run(`C1.dataset.open`), '0', 'switching to C2 clears C1 data-open');
  assert.equal(run(`C2.dataset.open`), '1');
  run(`hideReq();`);
  assert.equal(run(`C2.dataset.open`), '0', 'hideReq clears active chip data-open');
});

test('showcase toml.cjs handles Object.prototype keys safely without prototype pollution', () => {
  const { parse } = require('../scripts/showcase/toml.cjs');
  const doc = parse('toString = "ok"\n[__proto__]\npolluted = true\n');
  assert.equal(doc.toString, 'ok');
  assert.equal(({}).polluted, undefined, 'Object.prototype must not be polluted');
  assert.equal(Object.hasOwn(doc, '__proto__'), true);
  assert.equal(doc['__proto__'].polluted, true);
});

test('showcase build.cjs rejects path-traversal or HTML-breaking shot.file names', () => {
  const { render } = require('../scripts/showcase/build.cjs');
  const { CAPTURE_SCHEMA } = require('../scripts/showcase/spec.cjs');
  const spec = {
    fixtures: { c: { dir: 'examples/c', build: ['echo ok'], evidence: 'e.json', ci: true } },
    groups: [{ id: 'g', title: 'G', intro: 'intro' }],
    notClaimed: [],
    features: [{
      id: 'V01', title: 'T', claim: 'Claim.', group: 'g', issue: 1, status: 'captured',
      limits: [], fixtures: ['c'],
      shot: [{ file: 's', caption: 'A long enough caption for the screenshot.', route: '#/overview', expect: ['x'] }],
    }],
  };
  const cap = {
    schema: CAPTURE_SCHEMA,
    meta: { commit: 'abc123456789', commitShort: 'abc123456789', commitDate: '2026-01-01', branch: 'main', modified: 0, capturedAt: '2026-01-01', os: 'x', arch: 'x', tools: {} },
    fixtures: { c: { ok: true, commands: [] } },
    factSets: {},
    features: { V01: { commit: 'abc123456789', commands: [], facts: {}, shots: [{ source: 's', file: '../../etc/passwd', width: 10, height: 10, bytes: 1 }] } },
  };
  assert.throws(() => render({ spec, cap, matrix: null, inline: false, dir: __dirname }), /invalid image file/);
});
