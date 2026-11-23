// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/vars.test.cjs
//
// The order of a variable's accesses is the whole of #53: a set of lines
// cannot carry it, because one line can hold several accesses and the line
// numbers do not even ascend once a loop is involved. These pin the model
// `varAccesses()` builds, with no DOM in sight.
//
// The order is **static instruction order** -- the order the instructions
// appear in the image, by address -- and never an execution order. The
// attribution scans an address-sorted list once, so a site inside a loop is
// recorded once however many iterations run and a site inside a branch is
// recorded whether or not it is reached (review of #53). Every assertion
// here is about that list, never about what a run did.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function setup(ev, localStorage = { getItem: () => null }) {
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    localStorage,
    document: { addEventListener() {}, getElementById: () => null,
      createElement: tag => ({ tag, children: [], attrs: {}, dataset: {}, classList: { toggle() {}, add() {} },
        append(...c) { this.children.push(...c); }, setAttribute() {}, addEventListener() {} }),
      createTextNode: t => ({ tag: '#text', textContent: t }) },
  });
  const src = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  vm.runInContext(src.slice(0, src.lastIndexOf('\ninit().catch')), context);
  vm.runInContext(`S.ev = ${JSON.stringify(ev)}; S.mode = 'projected';`, context);
  return code => vm.runInContext(code, context);
}

const acc = (addr, line, access) => ({ addr, ...(line == null ? {} : { line }), access });

// checksum() from examples/c, as the AArch64 -O0 build really reports it.
// `acc` is read and then written on line 46 -- two sites on one line, which
// is why a set of lines cannot express this -- and `i` is a loop counter, so
// its lines run 45, 45, 46, 45, 45 and never sort into the order its
// instructions are laid out in.
const ev = {
  functions: [{
    id: 0, name: 'checksum', fileId: 0, lineStart: 43, lineEnd: 52, branches: [], decisions: [],
    variables: [
      { name: 'acc', kind: 'local', declLine: 44, typeName: 'uint32_t', accesses: [
        acc(0x6ac, 44, 'write'), acc(0x6cc, 46, 'read'), acc(0x6e0, 46, 'write'),
        acc(0x6e4, 47, 'read'), acc(0x6f0, 48, 'read'), acc(0x700, 48, 'write'), acc(0x71c, 51, 'read')] },
      { name: 'i', kind: 'local', declLine: 45, typeName: 'uint32_t', accesses: [
        acc(0x6b0, 45, 'write'), acc(0x6b8, 45, 'read'), acc(0x6d4, 46, 'read'),
        acc(0x70c, 45, 'read'), acc(0x714, 45, 'write')] },
      // A register-resident local: the reason is the only thing there is to
      // show, and it must never be rendered as "never used".
      { name: 'tmp', kind: 'local', declLine: 47, accesses: [],
        unattributed: 'the optimiser kept it in a register, so it has no frame slot to watch' },
      // An array whose address escapes: real accesses *and* a reason, because
      // what is missing is everything reached through the pointer.
      { name: 'alts', kind: 'local', declLine: 49, accesses: [acc(0x83c, 49, 'write'), acc(0x864, 50, 'address')],
        unattributed: 'its address is taken here, and accesses made through that pointer are not tracked' },
      // An instruction the line table places on no line at all.
      { name: 'len', kind: 'param', declLine: 43, typeName: 'uint32_t', accesses: [
        acc(0x6a8, 43, 'write'), acc(0x6b4, null, 'read'), acc(0x6bc, 45, 'read')] },
      { name: 'nothing', kind: 'local', declLine: 44, accesses: [] },
      // A loop and a branch, which is where reading these numbers as an
      // executed sequence goes wrong: sites 2 and 3 are the loop body and
      // repeat on every iteration, sites 4 and 5 are inside an `if` and may
      // never run at all -- yet each is listed exactly once, because this is
      // a list of instructions and not a trace (review of #53).
      { name: 'flag', kind: 'local', declLine: 60, typeName: 'uint32_t', accesses: [
        acc(0x900, 60, 'write'), acc(0x910, 61, 'read'), acc(0x914, 61, 'write'),
        acc(0x920, 63, 'read'), acc(0x924, 63, 'write'), acc(0x930, 66, 'read')] },
    ],
  }],
  files: [{ display: 'demo.c', text: [], lines: [] }],
  summary: {},
};

const load = () => setup(ev);
// Back across the VM boundary as JSON. Objects built inside the context carry
// that realm's prototypes, and assert/strict compares prototypes -- so a
// correct result fails with actual and expected printing identically. The Map
// comes back as its entries, which is what the assertions want anyway.
const v = (run, i) => JSON.parse(run(
  `JSON.stringify((() => { const a = varAccesses(fn(0), ${i}); return { ...a, byLine: [...a.byLine] }; })())`));

test('accesses keep the static instruction order, not line order', () => {
  const a = v(load(), 0);
  assert.deepEqual(a.all.map(u => [u.order, u.line, u.access]), [
    [1, 44, 'write'], [2, 46, 'read'], [3, 46, 'write'], [4, 47, 'read'],
    [5, 48, 'read'], [6, 48, 'write'], [7, 51, 'read'],
  ]);
  assert.equal(a.declLine, 44);
  // Line 46 holds two sites, and the read's instruction sits at the lower
  // address, so it is listed first. That is a statement about the image and
  // not about a run: both sites are in the loop body, so for zero iterations
  // neither executes at all (review of #53).
  const l46 = a.all.filter(u => u.line === 46);
  assert.ok(l46[0].order < l46[1].order && l46[0].access === 'read' && l46[1].access === 'write',
    'the read on 46 is the lower address, so it must be listed before the write on 46');
  assert.ok(l46[0].addr < l46[1].addr, 'and the address is the only thing the numbering follows');
});

test('the numbering is a static list of sites: a loop site once, a branch site regardless', () => {
  const a = v(load(), 6);
  // Six instructions, six positions. The two on line 61 are the loop body --
  // executed as many times as the loop runs, listed once each -- and the two
  // on 63 are inside an `if`, listed although the branch may never be taken.
  // Reading 4 as "after" 3 would be reading something this list never says.
  assert.deepEqual(a.all.map(u => [u.order, u.line, u.access]), [
    [1, 60, 'write'], [2, 61, 'read'], [3, 61, 'write'],
    [4, 63, 'read'], [5, 63, 'write'], [6, 66, 'read'],
  ]);
  const byLine = new Map(a.byLine);
  assert.deepEqual(byLine.get(61).map(u => u.order), [2, 3], 'the loop body contributes one position per instruction');
  assert.deepEqual(byLine.get(63).map(u => u.order), [4, 5], 'a conditional site is listed whether or not it runs');
  // What the order actually tracks, asserted directly: the addresses ascend,
  // because this is one pass over an address-sorted instruction list.
  assert.deepEqual([...a.all].sort((x, y) => x.addr - y.addr).map(u => u.order), [1, 2, 3, 4, 5, 6]);
});

test('a line touched twice keeps both of its positions', () => {
  const a = v(load(), 0);
  const byLine = new Map(a.byLine);
  assert.deepEqual(byLine.get(46).map(u => [u.order, u.access]), [[2, 'read'], [3, 'write']]);
  assert.deepEqual(byLine.get(48).map(u => [u.order, u.access]), [[5, 'read'], [6, 'write']]);
  // and a line touched once still carries exactly one
  assert.deepEqual(byLine.get(51).map(u => u.order), [7]);
});

test('a loop counter keeps its non-monotonic line sequence', () => {
  const a = v(load(), 1);
  assert.deepEqual(a.all.map(u => u.line), [45, 45, 46, 45, 45],
    'the sequence must not be sorted or deduplicated into a set of lines');
  const byLine = new Map(a.byLine);
  // Line 45 is the 1st, 2nd, 4th and 5th touch -- 3 belongs to line 46 in
  // the middle of them. Sorting the lines would lose exactly that.
  assert.deepEqual(byLine.get(45).map(u => u.order), [1, 2, 4, 5]);
  assert.deepEqual(byLine.get(46).map(u => u.order), [3]);
});

test('an empty access list carries its reason and claims nothing else', () => {
  const a = v(load(), 2);
  assert.deepEqual(a.all, []);
  assert.deepEqual(a.byLine, []);
  assert.match(a.unattributed, /kept it in a register/);
  assert.equal(a.declLine, 47, 'the declaration is still known even when no access is');
  // The distinction the model file insists on: no reason means the variable
  // really is untouched, and the two must not be conflated.
  const none = v(load(), 5);
  assert.deepEqual(none.all, []);
  assert.equal(none.unattributed, null);
});

test('a reason survives alongside accesses that were attributed', () => {
  const a = v(load(), 3);
  assert.deepEqual(a.all.map(u => [u.order, u.line, u.access]), [[1, 49, 'write'], [2, 50, 'address']]);
  assert.match(a.unattributed, /address is taken/,
    'a partially attributed variable must still say what is missing');
});

test('an access with no line holds its position and is counted, not dropped', () => {
  const a = v(load(), 4);
  // Three accesses, and the one the line table could not place keeps
  // position 2 -- so the read on line 45 is the 3rd touch, not the 2nd.
  assert.equal(a.all.length, 3);
  assert.deepEqual(a.all.map(u => [u.order, u.line]), [[1, 43], [2, null], [3, 45]]);
  assert.deepEqual(a.uses.map(u => [u.order, u.line]), [[1, 43], [3, 45]],
    'only the placed accesses can be marked in the source');
  assert.equal(a.unlined, 1, 'the unplaceable access must still be counted and reported');
  assert.equal(new Map(a.byLine).has(null), false);
});

/* --------- which row's marks belong on screen (hover + focus) --------- */
// A row is marked while the pointer is in it *or* the keyboard is on it, and
// the two are independent. Deciding this per event was the bug: an access
// badge's own mouseleave fired when the pointer moved back into its parent
// row -- whose mouseenter does not fire again for a move inside itself -- so
// the marks stayed off, and a focused row lost them as soon as the pointer
// left (review of #53).
test('the pointer wins while it is in a row, and a focused row keeps its marks when it leaves', () => {
  const run = load();
  const target = st => run(`varMarkTarget(${JSON.stringify(st)})`);
  assert.equal(target({ hover: null, focus: null }), null, 'neither: nothing is marked');
  assert.equal(target({ hover: 2, focus: null }), 2);
  assert.equal(target({ hover: null, focus: 1 }), 1,
    'the pointer has left, but the keyboard is still on row 1');
  assert.equal(target({ hover: 2, focus: 1 }), 2,
    'a reader moving the pointer is pointing at something, so it wins');
  // Row 0 is a row. A truthiness test here would mark nothing for the first
  // variable in every function.
  assert.equal(target({ hover: 0, focus: 1 }), 0);
  assert.equal(target({ hover: null, focus: 0 }), 0);
});

test('an index with no variable behind it is null, not an empty variable', () => {
  const run = load();
  assert.equal(run('varAccesses(fn(0), 99)'), null);
  assert.equal(run('varAccesses({}, 0)'), null);
  assert.equal(run('varAccesses(null, 0)'), null);
});

/* ---------------- the switch that makes the marks optional ---------------- */
// The marks are a decoration on somebody else's source, and not everyone
// wants them, so a toggle beside the coverage overlay hides them. What is
// state rather than DOM is pinned here: the default, what an unreadable
// storage does, where the choice is written, and the single rule that
// decides whether a hover paints at all.

// A localStorage holding one value for the marks and nothing else.
const store = v => ({ getItem: k => (k === 'sl-varmarks' ? v : null), setItem() {} });

test('the marks are on unless this browser has been told otherwise', () => {
  assert.equal(setup(ev)('S.varMarks'), true,
    'nothing stored means on: a feature that starts hidden is a feature nobody finds');
  assert.equal(setup(ev, store('0'))('S.varMarks'), false);
  assert.equal(setup(ev, store('1'))('S.varMarks'), true);
  // Only '0' means off. A value some other version of the viewer wrote must
  // not be read as "hide it".
  assert.equal(setup(ev, store('yes'))('S.varMarks'), true);
  assert.equal(setup(ev, store(''))('S.varMarks'), true);
});

test('a storage that can be neither read nor written leaves the marks on, and the click still works', () => {
  const run = load();
  run(`localStorage = { getItem() { throw new Error('denied'); }, setItem() { throw new Error('denied'); } };`);
  assert.equal(run('readVarMarks()'), true,
    'a private window must not silently disable a feature it could not be asked about');
  // The session honours a choice it could not record: a browser refusing
  // site data is not a reason to refuse the button.
  assert.doesNotThrow(() => run('setVarMarks(false)'));
  assert.equal(run('S.varMarks'), false);
});

test('the choice is written under sl-varmarks, beside the theme', () => {
  const written = [];
  // Strings, not tuples: an array built inside the VM carries that realm's
  // prototype, and the strict deepEqual below would reject it.
  const run = setup(ev, { getItem: () => null, setItem: (k, v) => written.push(`${k}=${v}`) });
  run('setVarMarks(false)');
  run('setVarMarks(true)');
  assert.deepEqual(written, ['sl-varmarks=0', 'sl-varmarks=1']);
});

test('whether a hover paints is decided by that state and nothing else', () => {
  const run = load();
  assert.equal(run('varMarksWanted(true)'), true, 'on, and the pointer arrived: paint');
  assert.equal(run('varMarksWanted(false)'), false, 'the pointer leaving never paints');
  run('S.varMarks = false;');
  assert.equal(run('varMarksWanted(true)'), false, 'off: a hover paints nothing at all');
  assert.equal(run('varMarksWanted(false)'), false);
  // ...and the panel's own model does not move with it. The toggle hides
  // marks; it must never change what the list claims about the evidence.
  const a = JSON.parse(run('JSON.stringify((() => { const a = varAccesses(fn(0), 0); return { ...a, byLine: [...a.byLine] }; })())'));
  assert.equal(a.all.length, 7, 'every access is still there with the marks off');
  assert.deepEqual(a.all.map(u => u.order), [1, 2, 3, 4, 5, 6, 7]);
});
