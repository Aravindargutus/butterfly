// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/unrecorded.test.cjs
//
// A function with no coverage record of its own (#273): a closure rustc gave
// none, a lambda thunk clang gave none. Its count of 0 means "not counted",
// so the viewer must never read it as "never executed" -- nor as covered.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function setup() {
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
  vm.runInContext('S.res = new Map();', context);
  return code => vm.runInContext(code, context);
}

// `closures` has a record and ran; `closures::{closure#1}` is written inside
// it on line 2 and has none. `never` has a record and never ran.
const ev = `{
  functions: [
    { id: 0, name: 'closures', fileId: 0, count: 1, lineStart: 1, lineEnd: 3, linesTotal: 3, linesCovered: 3, branches: [], decisions: [], callees: [], callers: [], object: null },
    { id: 1, name: 'never', fileId: 0, count: 0, lineStart: 3, lineEnd: 3, linesTotal: 1, linesCovered: 0, branches: [], decisions: [], callees: [], callers: [], object: null },
    { id: 2, name: 'closures::{closure#1}', fileId: 0, count: 0, instantiations: 0, lineStart: 2, lineEnd: 2, linesTotal: 0, linesCovered: 0, unrecordedWithin: 0,
      branches: [], decisions: [], callees: [], callers: [], object: null },
  ],
  files: [{ display: 'a.rs', text: ['a', 'b', 'c'], lines: [{ count: 1 }, { count: 1 }, { count: 0 }] }],
  summary: {},
}`;

test('the worklist does not call a function with no record "never executed"', () => {
  const run = setup();
  run(`S.ev = ${ev};`);
  const fns = JSON.parse(run(`JSON.stringify(worklist().filter(i => i.kind === 'function').map(i => i.f.name))`));
  assert.deepEqual(fns, ['never'], 'only the function with a record and a count of 0 is a "never executed" test obligation');
});

test('the call graph names it as having no record, not as called 0 times', () => {
  const run = setup();
  run(`S.ev = ${ev};`);
  assert.equal(run(`callNodeSub({ kind: 'callee', fnId: 2 })`), 'no coverage record');
  assert.match(run(`callNodeSub({ kind: 'self', fnId: 2 })`), /^no coverage record · /);
  assert.equal(run(`callNodeSub({ kind: 'callee', fnId: 1 })`), 'called 0×');
});
