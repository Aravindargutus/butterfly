// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/lines.test.cjs
//
// A line that was reached but whose statement never ran must not read as
// covered (#74).
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function setup(ev, resolutions = []) {
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    localStorage: { getItem: () => null },
    document: { addEventListener() {}, getElementById: () => null,
      createElement: tag => ({ tag, children: [], attrs: {}, dataset: {}, classList: { toggle() {}, add() {} },
        append(...c) { this.children.push(...c); }, setAttribute() {}, addEventListener() {} }),
      createTextNode: t => ({ tag: '#text', textContent: t }) },
  });
  const src = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  vm.runInContext(src.slice(0, src.lastIndexOf('\ninit().catch')), context);
  // No manual cache reset here, on purpose: the harness must rely on the
  // same invalidation production relies on, or a missing production reset
  // hides behind the test setup (review of #74).
  vm.runInContext(`S.ev = ${JSON.stringify(ev)}; S.res = new Map(${JSON.stringify(resolutions)}.map(k => [k, { key: k }])); S.mode = 'projected';`, context);
  return code => vm.runInContext(code, context);
}

// `if (v > hi) return hi;` on line 3, reached once, true outcome never taken.
// This is the shape both clang and GCC produce for the same source.
const ev = (branch) => ({
  functions: [{ id: 0, name: 'clamp', fileId: 0, lineStart: 1, lineEnd: 5,
    branches: [branch], decisions: [] }],
  files: [{ display: 's.c', text: ['int clamp(int v,int lo,int hi){', 'if (v < lo) return lo;', 'if (v > hi) return hi;', 'return v;', '}'],
    lines: [{ count: 2 }, { count: 2 }, { count: 1 }, { count: 1 }, { count: 1 }] }],
  summary: {},
});
const clangBranch = { line: 3, col: 9, trueCount: 0, falseCount: 1, directionKnown: true };
const gcovBranch = { line: 3, col: 0, trueCount: 0, falseCount: 1, directionKnown: false, decisionOrdinal: 0 };

test('a reached line whose statement never ran is not painted covered', () => {
  for (const [name, b] of [['clang', clangBranch], ['gcov', gcovBranch]]) {
    const run = setup(ev(b));
    assert.equal(run('lineStatus(0, 3)'), 'part', `${name}: line 3 holds a statement that never ran`);
    // the lines around it are unaffected
    assert.equal(run('lineStatus(0, 2)'), 'cov', `${name}: line 2 took both outcomes`);
    assert.equal(run('lineStatus(0, 4)'), 'cov', `${name}: line 4 has no decision at all`);
  }
});

test('a line whose decision took both outcomes stays covered', () => {
  const run = setup(ev({ line: 3, col: 9, trueCount: 1, falseCount: 1, directionKnown: true }));
  assert.equal(run('lineStatus(0, 3)'), 'cov');
});

test('a line that never ran at all is still uncovered, not partial', () => {
  const e = ev(clangBranch);
  e.files[0].lines[2] = { count: 0 };
  assert.equal(setup(e)('lineStatus(0, 3)'), 'unc', 'no count means it never ran; partial is about lines that did');
});

test('a justified untaken outcome closes the finding', () => {
  // the key the MC/DC pane writes for the untaken true outcome
  const run0 = setup(ev(clangBranch));
  const key = run0(`branchKey(0, ${JSON.stringify(clangBranch)}, outcomeKeys(${JSON.stringify(clangBranch)})[0])`);
  const run = setup(ev(clangBranch), [key]);
  assert.equal(run('lineStatus(0, 3)'), 'cov', `a reviewer accepted it (${key}), so it is no longer an open finding`);
});

test('the gcov A/B keys are the ones consulted, not T/F', () => {
  const run0 = setup(ev(gcovBranch));
  const key = run0(`branchKey(0, ${JSON.stringify(gcovBranch)}, outcomeKeys(${JSON.stringify(gcovBranch)})[0])`);
  assert.match(key, /:A$/, 'gcov outcomes key A/B');
  assert.equal(setup(ev(gcovBranch), [key])('lineStatus(0, 3)'), 'cov');
  // a T-keyed resolution must NOT close a gcov finding, or one pane's review
  // would silently satisfy another's question
  assert.equal(setup(ev(gcovBranch), ['branch:0:3:#0:T'])('lineStatus(0, 3)'), 'part');
});

test('it says which outcome went untaken, and never invents a polarity', () => {
  const clang = setup(ev(clangBranch))('partialWhy(0, 3)');
  assert.match(clang, /true never happened/);

  const gcov = setup(ev(gcovBranch))('partialWhy(0, 3)');
  assert.doesNotMatch(gcov, /\btrue never happened\b/, 'gcov does not establish which outcome is true');
  assert.match(gcov, /outcome A|outcome B/);
});

test('plain mode still turns the overlay off entirely', () => {
  const run = setup(ev(clangBranch));
  run("S.mode = 'plain';");
  assert.equal(run('lineStatus(0, 3)'), 'none');
});

// Switching variants replaces S.ev wholesale (loadVariant commits `S.ev = ev`),
// and the per-line branch index used to survive that: the new variant's lines
// were painted with the old variant's decisions. The index is keyed to the
// evidence object itself, so this test swaps S.ev exactly the way a variant
// switch does -- with no manual reset -- and the answer must follow.
test('the branch index follows a variant switch instead of outliving it', () => {
  const run = setup(ev(clangBranch));
  assert.equal(run('lineStatus(0, 3)'), 'part', 'first variant: line 3 is partial');

  // second variant: same file shape, but its decision took both outcomes
  const both = { ...clangBranch, trueCount: 1 };
  run(`S.ev = ${JSON.stringify(ev(both))};`);
  assert.equal(run('lineStatus(0, 3)'), 'cov',
    'after the switch, line 3 must be judged by the new variant, not the cached one');

  // and back again, in case the first build is the one that sticks
  run(`S.ev = ${JSON.stringify(ev(clangBranch))};`);
  assert.equal(run('lineStatus(0, 3)'), 'part', 'switching back re-judges again');
});

// A failed or stale variant load aborts before `S.ev = ev` commits, so the
// evidence on screen is unchanged -- and the index for it must survive, not
// be dropped or rebuilt against anything else. Keying the cache to the
// evidence object gives exactly that: same object, same index.
test('a load that never commits leaves the current view alone', () => {
  const run = setup(ev(clangBranch));
  assert.equal(run('lineStatus(0, 3)'), 'part');
  const before = run('_brByLine');

  // what loadVariant does on a stale generation: gives up before S.ev moves
  run('(function(){ const stale = true; if (stale) return; S.ev = null; })()');
  assert.equal(run('lineStatus(0, 3)'), 'part', 'the view on screen still answers');
  assert.equal(run('_brByLine') === before, true, 'and its index was not rebuilt');
});
