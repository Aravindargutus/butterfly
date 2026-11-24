// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/mcdc_tests.test.cjs
//
// The MC/DC panel names the tests that proved an independence pair when the
// evidence supports it, and keeps the old vector wording when it does not
// (#48). The failure this whole area keeps producing is a confident sentence
// the evidence cannot support, so the unmapped and ambiguous cases matter as
// much as the clean one.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function setup() {
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    localStorage: { getItem: () => null },
    document: { addEventListener() {}, getElementById: () => null, createElement: t => ({ tag: t, children: [], append() {}, setAttribute() {}, addEventListener() {}, classList: { toggle() {}, add() {} } }), createTextNode: t => ({ tag: '#text', textContent: t }) },
  });
  const src = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  vm.runInContext(src.slice(0, src.lastIndexOf('\ninit().catch')), context);
  return code => vm.runInContext(code, context);
}

/// A decision whose condition 0 is proven by vectors 1 and 2, with whatever
/// test attribution the case under test wants on each.
const decision = (t1, t2) => JSON.stringify({
  line: 2, col: 7,
  conditions: [{ index: 0, line: 2, col: 7, trueCount: 1, falseCount: 1, independenceCovered: true, pair: [1, 2] }],
  vectors: [
    { values: [false, null], result: false, executed: true, tests: t1 },
    { values: [true, true], result: true, executed: true, tests: t2 },
  ],
});
const withTests = names => `S.ev = { tests: ${JSON.stringify(names.map((n, i) => ({ id: i, name: n })))}, functions: [], files: [] };`;

test('both vectors attributed: the sentence names the tests a reviewer would re-run', () => {
  const run = setup();
  run(withTests(['t_bank_shallow', 't_bank_steep']));
  const out = JSON.parse(run(`JSON.stringify(mcdcStory(${decision([0], [1])}, 0, 'bank_deg > 60'))`));
  assert.ok(out.ok);
  assert.match(out.text, /Proven by t_bank_shallow and t_bank_steep/, out.text);
  // the vector numbers stay, so the truth table above is still navigable
  assert.match(out.text, /condition vectors 1 and 2/, out.text);
  assert.match(out.text, /differ only in bank_deg > 60/, out.text);
});

test('several tests producing one vector: either proves it, and it says so', () => {
  const run = setup();
  run(withTests(['t_a', 't_b', 't_c']));
  const out = JSON.parse(run(`JSON.stringify(mcdcStory(${decision([0, 1], [2])}, 0, 'C1'))`));
  // parenthesised so the "or" cannot be read as binding across the "and"
  assert.match(out.text, /Proven by t_a \(or t_b\) and t_c/, out.text);
  assert.ok(!/t_a or t_b and/.test(out.text), 'a flat or/and reads ambiguously: ' + out.text);
});

test('one half unattributed: today\'s vector wording stands, no half-named sentence', () => {
  const run = setup();
  run(withTests(['t_only']));
  const out = JSON.parse(run(`JSON.stringify(mcdcStory(${decision([0], [])}, 0, 'C1'))`));
  assert.ok(out.ok);
  assert.match(out.text, /^Proven by condition vectors 1 and 2:/, out.text);
  assert.ok(!out.text.includes('t_only'), 'naming one half and numbering the other is worse than numbering both: ' + out.text);
});

test('no per-test data at all: unchanged from before the feature', () => {
  const run = setup();
  run('S.ev = { tests: [], functions: [], files: [] };');
  const out = JSON.parse(run(`JSON.stringify(mcdcStory(${decision([], [])}, 0, 'C1'))`));
  assert.match(out.text, /^Proven by condition vectors 1 and 2:/, out.text);
});

test('a test id the evidence does not carry is not invented', () => {
  const run = setup();
  run(withTests(['t_only'])); // ids 0 only
  const out = JSON.parse(run(`JSON.stringify(mcdcStory(${decision([0], [99])}, 0, 'C1'))`));
  assert.match(out.text, /^Proven by condition vectors 1 and 2:/, 'an out-of-range id must fall back, not print undefined: ' + out.text);
  assert.ok(!out.text.includes('undefined'), out.text);
});
