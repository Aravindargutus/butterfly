// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/outcomes.test.cjs
//
// Branch-outcome identity and labelling, across every pane that touches it.
// A review recorded in one pane must be the same review every other pane
// sees, and a gcov decision must never be labelled with a polarity the
// producer did not establish (review of #45).
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function setup(resolutions = []) {
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    localStorage: { getItem: () => null },
    document: {
      addEventListener() {}, getElementById: () => null,
      createElement: tag => ({ tag, children: [], attrs: {}, dataset: {}, classList: { toggle() {}, add() {}, contains: () => false },
        append(...c) { this.children.push(...c); }, setAttribute(k, v) { this.attrs[k] = v; }, addEventListener() {},
        set textContent(t) { this._t = t; }, get textContent() { return (this._t || '') + this.children.map(c => c.textContent || '').join(''); } }),
      createTextNode: text => ({ tag: '#text', children: [], textContent: text }),
    },
  });
  const source = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  vm.runInContext(source.slice(0, source.lastIndexOf('\ninit().catch')), context);
  vm.runInContext(`S.res = new Map(${JSON.stringify(resolutions)}.map(k => [k, { key: k }]));`, context);
  return code => vm.runInContext(code, context);
}

// A decision the producer gave a column for, and one it did not.
const withCol = '{ line: 11, col: 9, trueCount: 1, falseCount: 0, directionKnown: true }';
const gcov0 = '{ line: 2, col: 0, trueCount: 1, falseCount: 0, directionKnown: false, decisionOrdinal: 0 }';
const gcov1 = '{ line: 2, col: 0, trueCount: 0, falseCount: 1, directionKnown: false, decisionOrdinal: 1 }';

test('two gcov decisions on one line do not share a resolution key', () => {
  const run = setup();
  const a = run(`branchKey(0, ${gcov0}, 'A')`);
  const b = run(`branchKey(0, ${gcov1}, 'A')`);
  assert.notEqual(a, b, 'same key for two different decisions: justifying one would justify the other');
  assert.equal(a, 'branch:0:2:#0:A');
  assert.equal(b, 'branch:0:2:#1:A');
});

test('an ordinal key can never collide with a column key', () => {
  const run = setup();
  // column 0 and ordinal 0 must not produce the same string
  assert.notEqual(run(`branchKey(0, ${gcov0}, 'A')`), run(`branchKey(0, { line: 2, col: 0, directionKnown: true }, 'A')`));
});

test('a decision with a column keeps its old key, so existing reviews still match', () => {
  const run = setup();
  assert.equal(run(`branchKey(0, ${withCol}, 'T')`), 'branch:0:11:9:T');
});

test('the worklist keys and labels gcov outcomes the same way the panes do', () => {
  // an existing justification on outcome B of the *second* decision
  const run = setup(['branch:0:2:#1:B']);
  const f = `{ id: 0, name: 'f', fileId: 0, count: 1, lineStart: 1, lineEnd: 4, linesTotal: 4, linesCovered: 4,
               branches: [${gcov0}, ${gcov1}], decisions: [], object: null }`;
  run(`S.ev = { functions: [${f}], files: [{ display: 'a.c', text: ['a','b','c','d'], lines: [] }], summary: {} };`);
  const outcomes = JSON.parse(run(`JSON.stringify(worklist().filter(i => i.kind === 'outcome').map(i => ({ what: i.what, next: i.next })))`));

  // the justified one is gone; the other two remain
  assert.equal(outcomes.length, 2, JSON.stringify(outcomes.map(o => o.what)));
  for (const o of outcomes) {
    assert.ok(!/\b(true|false) outcome\b/.test(o.what), `polarity the producer did not establish: ${o.what}`);
    assert.match(o.what, /gcov does not say which is true/);
  }
  // and it names the decision by its ordinal, not by a column it never had
  assert.ok(outcomes.some(o => /L2:#0/.test(o.what)), JSON.stringify(outcomes.map(o => o.what)));
});

test('a resolution saved in one pane is seen by the worklist', () => {
  const f = `{ id: 0, name: 'f', fileId: 0, count: 1, lineStart: 1, lineEnd: 4, linesTotal: 4, linesCovered: 4,
               branches: [${gcov1}], decisions: [], object: null }`;
  const ev = `{ functions: [${f}], files: [{ display: 'a.c', text: ['a','b','c','d'], lines: [] }], summary: {} }`;

  const r0 = setup();
  r0(`S.ev = ${ev};`);
  assert.equal(r0(`worklist().filter(i => i.kind === 'outcome').length`), 1, 'one untaken outcome to start with');

  // the key the MC/DC pane would write for that same outcome
  const key = setup()(`branchKey(0, ${gcov1}, outcomeKeys(${gcov1})[0])`);
  const r1 = setup([key]);
  r1(`S.ev = ${ev};`);
  assert.equal(r1(`worklist().filter(i => i.kind === 'outcome').length`), 0, `the worklist did not see the review saved as ${key}`);
});

test('a clang decision is still labelled true and false', () => {
  // Compared as JSON: arrays cross the VM realm boundary, so deepEqual sees
  // two different Array constructors and fails on values that are equal.
  const run = setup();
  assert.equal(run(`JSON.stringify(outcomeKeys(${withCol}))`), '["T","F"]');
  assert.equal(run(`JSON.stringify(outcomeLabels(${withCol}))`), '["true","false"]');
  assert.equal(run(`JSON.stringify(outcomeKeys(${gcov0}))`), '["A","B"]');
});

// The worklist scores a *function* item by how much closing it would close,
// and that count was built from a raw `line:col` key while every other
// consumer had moved to branchKey. For gcov evidence, where two decisions
// share column 0, the two spellings are different strings: a justification
// saved by any pane was invisible to this counter, so a function kept
// advertising work that was already reviewed -- and the ranking that decides
// what a reviewer sees first was computed from it (review of #45).
test('the function score counts the same outcome keys the panes write', () => {
  // count: 0 -- the function score exists only for a function nothing called,
  // which is exactly where a reviewer's justifications should reduce it
  const f = `{ id: 0, name: 'f', fileId: 0, count: 0, lineStart: 1, lineEnd: 2, linesTotal: 2, linesCovered: 0,
               branches: [${gcov0}, ${gcov1}], decisions: [], object: null }`;
  const ev = `{ functions: [${f}], files: [{ display: 'a.c', text: ['a','b'], lines: [{ count: 0 }, { count: 0 }] }], summary: {} }`;
  const fnScore = run => JSON.parse(run(`JSON.stringify(worklist().filter(i => i.kind === 'function').map(i => i.closes))`))[0];

  const before = setup();
  before(`S.ev = ${ev};`);
  const open = fnScore(before);

  // justify one outcome, spelled exactly as the panes spell it
  const key = setup()(`branchKey(0, ${gcov0}, outcomeKeys(${gcov0})[0])`);
  const after = setup([key]);
  after(`S.ev = ${ev};`);

  assert.equal(fnScore(after), open - 1, `the function score did not see the review saved as ${key}`);
});
