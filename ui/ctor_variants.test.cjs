// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/ctor_variants.test.cjs
//
// A C++ constructor or destructor is several object bodies: the one its
// coverage record names (`object`) and the variants the compiler emits beside
// it (`variants`: C1, D1, the deleting D0). Each variant is analysed, and has
// no coverage counter of its own (#266). The viewer has to show them, count
// their gaps with the function's, and say why their instructions are not
// credited -- through renderObject() and fnGaps(), the code a reader sees.
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
    set textContent(t) { this._t = String(t); },
    get textContent() { return (this._t || '') + this.children.map(c => c.textContent || '').join(''); },
  };
}

function setup() {
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    localStorage: { getItem: k => (k === 'sl-mode' ? 'projected' : null) },
    document: {
      addEventListener() {}, getElementById: () => null,
      createElement: element,
      createTextNode: text => ({ nodeType: 3, children: [], textContent: String(text) }),
    },
  });
  const source = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  vm.runInContext(source.slice(0, source.lastIndexOf('\ninit().catch')), context);
  vm.runInContext(`S.res = new Map(); S.ev = { measured: null, functions: [],
    files: [{ display: 'constructs.cpp', text: ['Channel::~Channel() {', '  delete[] samples_;', '}'], lines: [{ count: 2 }, { count: 2 }, { count: 2 }] }] };`, context);
  return code => vm.runInContext(code, context);
}

const instr = (addr, projected, gapIds = '[]') => `{ addr: ${addr}, line: 1, mnemonic: 'bl', operands: 'x', flags: [], gapIds: ${gapIds}, projected: '${projected}' }`;
const gap = (id, kind, key) => `{ id: ${id}, kind: '${kind}', key: '${key}', addrStart: 1, addrEnd: 1, line: 1, description: '${kind}' }`;
const body = (addr, n, projected, extra) => `{ gaps: [], disagreements: [], instrTotal: ${n}, instrProjectedCovered: ${projected === 'covered' ? n : 0},
  instrProjectedUncovered: 0, instrUnknown: ${projected === 'unknown' ? n : 0}, compilerGenerated: {}, address: ${addr}, size: 8,
  blocks: [{ id: 0, address: ${addr}, kind: 'entry', succs: [], instrs: [${instr(addr, projected)}] }], ${extra || ''} }`;

function dtor(count) {
  return `({ id: 0, name: 'fc::Channel::~Channel()', fileId: 0, count: ${count}, requirements: [], inlined: [], lineRequirements: {},
    branches: [], decisions: [], lineStart: 1, lineEnd: 3, linesTotal: 3, linesCovered: 3,
    object: Object.assign(${body(4096, 1, 'covered')}, { symbol: '_ZN2fc7ChannelD2Ev', gaps: [${gap(0, 'helperCall', '_ZN2fc7ChannelD2Ev@helper-call@1000')}] }),
    variants: [Object.assign(${body(8192, 1, count ? 'unknown' : 'uncovered')}, { symbol: '_ZN2fc7ChannelD0Ev', variant: 'D0',
      gaps: [${gap(0, 'helperCall', '_ZN2fc7ChannelD0Ev@helper-call@2004')}, ${gap(1, 'uncountedVariant', '_ZN2fc7ChannelD0Ev@uncounted-variant@2000')}] })] })`;
}

function render(run, count) {
  run(`F = ${dtor(count)}; S.ev.functions = [F]; BODY = h('div', {}); renderObject(BODY, F);`);
  return JSON.parse(run(`JSON.stringify(BODY.children[BODY.children.length - 1].children.map(r => ({ cls: r.attrs.class, block: r.attrs['data-block'], text: r.textContent })))`));
}

test('the Object tab shows each variant under the body, saying why it is not credited', () => {
  const rows = render(setup(), 5);
  const head = rows.find(r => r.cls === 'blk' && /D0/.test(r.text));
  assert.ok(head, JSON.stringify(rows));
  assert.match(head.text, /deleting destructor/);
  assert.match(head.text, /_ZN2fc7ChannelD0Ev/);
  assert.match(head.text, /not established/);
  // its instruction is drawn, with no status the counters never gave it
  const ins = rows.filter(r => /^i /.test(r.cls || ''));
  assert.equal(ins.length, 2, 'the body and the variant each have one instruction');
  assert.match(ins[1].cls, /unk/, ins[1].cls);
  // block ids are per body: only the body's rows answer to the Flow graph
  assert.ok(rows.filter(r => r.block != null).every(r => rows.indexOf(r) < rows.indexOf(head)), 'a variant row carries the body\'s block id');
});

test('a variant of a body that never ran says it never ran', () => {
  const rows = render(setup(), 0);
  const head = rows.find(r => r.cls === 'blk' && /D0/.test(r.text));
  assert.match(head.text, /never ran/);
});

test('objTotals counts the body and every variant, so nothing is dropped (#283)', () => {
  const run = setup();
  run(`F = ${dtor(5)};`);
  // base body: 1 instr, projected covered; D0 variant: 1 instr, unknown
  const tot = JSON.parse(run(`JSON.stringify(objTotals(F))`));
  assert.equal(tot.instrTotal, 2, 'base body + D0 variant, not the body alone');
  assert.equal(tot.instrProjectedCovered, 1);
  assert.equal(tot.instrUnknown, 1, 'the uncredited variant is counted, not hidden behind a full-coverage body');
});

test('the Object-tab toolbar totals include the variants, not only the body (#283)', () => {
  const run = setup();
  run(`F = ${dtor(5)}; S.ev.functions = [F]; BODY = h('div', {}); renderObject(BODY, F);`);
  // the pane-tools toolbar is appended just before the table
  const bar = run(`BODY.children[BODY.children.length - 2].textContent`);
  assert.match(bar, /2 instructions/, `the toolbar must count both bodies: ${bar}`);
  assert.match(bar, /1 unknown/, `and show the uncredited variant's instruction: ${bar}`);
});

test('the function\'s gaps are its body\'s and its variants\', keyed apart', () => {
  const run = setup();
  run(`F = ${dtor(5)};`);
  assert.deepEqual(JSON.parse(run(`JSON.stringify(fnGaps(F).map(g => g.key))`)), [
    '_ZN2fc7ChannelD2Ev@helper-call@1000', '_ZN2fc7ChannelD0Ev@helper-call@2004', '_ZN2fc7ChannelD0Ev@uncounted-variant@2000',
  ]);
  assert.equal(run(`fnStats(F).gaps`), 3);
  assert.equal(run(`unresolvedGaps(F)`), 3);
  // two bodies' gap ids overlap (each starts at 0): the key is what tells them apart
  assert.equal(run(`new Set(fnGaps(F).map(g => g.id)).size`), 2);
});
