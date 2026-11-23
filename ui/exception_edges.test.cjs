// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/exception_edges.test.cjs
//
// An exception edge -- a call's edge to the landing pad the function's
// exception table names (#165) -- is not taken just because both its ends ran:
// nothing establishes that an exception was thrown along it. The backend
// projects it Unknown even when both blocks are covered, and the Flow viewer's
// edgeStatus() must not repaint it green from block coverage.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function element(tag) {
  return {
    tag, nodeType: 1, children: [], attrs: {}, dataset: {},
    classList: { toggle() {}, add() {}, remove() {}, contains: () => false },
    append(...c) { this.children.push(...c.filter(x => x != null)); },
    setAttribute(k, v) { this.attrs[k] = v; },
    addEventListener() {},
    set innerHTML(t) { this._t = String(t); },
    get textContent() { return (this._t || '') + this.children.map(c => c.textContent || '').join(''); },
  };
}

function setup() {
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    localStorage: { getItem: k => (k === 'sl-mode' ? 'projected' : null) },
    document: { addEventListener() {}, getElementById: () => null, createElement: element, createTextNode: t => ({ nodeType: 3, children: [], textContent: String(t) }) },
  });
  const source = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  vm.runInContext(source.slice(0, source.lastIndexOf('\ninit().catch')), context);
  vm.runInContext(`S.res = new Map(); S.ev = { measured: null, functions: [] };
    F = { fileId: 0, object: { blocks: [
      { id: 0, instrs: [{ projected: 'covered', line: 1, gapIds: [], flags: [] }], succs: [] },
      { id: 1, instrs: [{ projected: 'covered', line: 2, gapIds: [], flags: [] }], succs: [] } ] } };`, context);
  return code => vm.runInContext(code, context);
}

test('a covered-both-ends exception edge stays unknown, not green (#165)', () => {
  const run = setup();
  const st = run(`edgeStatus(F, F.object.blocks[0], { kind: 'exception', to: 1, projected: 'unknown', measured: null })`);
  assert.equal(st, 'unk', 'both ends ran, but nothing says an exception was thrown along it');
});

test('an ordinary edge with both ends covered is still green', () => {
  const run = setup();
  const st = run(`edgeStatus(F, F.object.blocks[0], { kind: 'fallthrough', to: 1, projected: 'covered', measured: null })`);
  assert.equal(st, 'cov', 'normal control flow is unaffected by the exception rule');
});

test('a measured exception edge still reflects the trace', () => {
  const run = setup();
  run(`S.ev.measured = { }; S.mode = 'measured';`);
  assert.equal(run(`edgeStatus(F, F.object.blocks[0], { kind: 'exception', to: 1, projected: 'unknown', measured: 3 })`), 'cov', 'the trace saw it taken');
  assert.equal(run(`edgeStatus(F, F.object.blocks[0], { kind: 'exception', to: 1, projected: 'unknown', measured: 0 })`), 'unc', 'the trace saw it never taken');
});
