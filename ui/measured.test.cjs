// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/measured.test.cjs
//
// The Measured overlay (#164). Until examples/rust-qemu existed nothing in the
// repository loaded a trace, so nobody had looked at this mode with real
// evidence in it. The first look found a block drawn red -- the trace never
// entered it -- under the caption "projected uncovered", on a block the
// projection calls covered.
//
// The defect was one argument at one call site in renderObject(), so the
// assertions go through renderObject() and read the captions back out of the
// table it builds. Pinning blockStatusLabel() alone left the whole visible
// bug revertable with every test still green (self-review of #208).
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// Enough of a DOM for renderObject() to build its table and for the test to
// read the text back out of it. `nodeType` matters: h() wraps anything
// without one in a text node, which would flatten every nested element to
// "[object Object]".
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

function setup({ mode, measured }) {
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    localStorage: { getItem: k => (k === 'sl-mode' ? mode : null) },
    document: {
      addEventListener() {}, getElementById: () => null,
      createElement: element,
      createTextNode: text => ({ nodeType: 3, children: [], textContent: String(text) }),
    },
  });
  const source = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  vm.runInContext(source.slice(0, source.lastIndexOf('\ninit().catch')), context);
  vm.runInContext(`S.res = new Map(); S.ev = { measured: ${measured ? '{ format: "qemu" }' : 'null'} };`, context);
  return code => vm.runInContext(code, context);
}

// sum()'s bounds-check panic block in examples/rust-qemu: its line ran four
// times, so both blocks are projected covered; the trace entered only one.
const f = '{ fileId: 0 }';
// With a trace imported, an instruction the trace never ran is measured 0;
// null is "no trace", and never appears beside one (#325).
const panicBlock = `{ instrs: [{ line: 59, projected: 'covered', measured: 0 }, { line: 59, projected: 'covered', measured: 0 }] }`;
const loopBlock = `{ instrs: [{ line: 59, projected: 'covered', measured: 4 }] }`;

const instr = (addr, measured) => `{ addr: ${addr}, line: 2, mnemonic: 'ldr', operands: 'x0', flags: [], gapIds: [], projected: 'covered', measured: ${measured} }`;

/// Render the two blocks as a function's Object tab and return the caption
/// row of each block, in order. Those rows are what the reader sees beside
/// the colour.
function blockCaptions(run) {
  run(`S.ev.files = [{ display: 'monitor.rs', text: ['', 'acc = acc + samples[i];'], lines: [{ count: 4 }] }];
       S.ev.functions = [];
       F = { id: 0, name: 'sum', fileId: 0, requirements: [], inlined: [], lineRequirements: {},
             lineStart: 1, lineEnd: 2,
             object: { gaps: [], disagreements: [], instrTotal: 2, instrProjectedCovered: 2, instrProjectedUncovered: 0,
                       instrUnknown: 0, instrMeasuredCovered: 1, compilerGenerated: {},
                       blocks: [{ id: 0, address: 4096, kind: 'entry', succs: [], instrs: [${instr(4096, 4)}] },
                                { id: 1, address: 4104, kind: 'panic', succs: [], instrs: [${instr(4104, 0)}] }] } };
       BODY = h('div', {});
       renderObject(BODY, F);`);
  return JSON.parse(run(`(function () {
    var table = BODY.children[BODY.children.length - 1];
    return JSON.stringify(table.children.filter(r => r.attrs.class === 'blk').map(r => r.textContent));
  })()`));
}

test('the Measured overlay is offered only when the evidence carries a trace', () => {
  assert.equal(setup({ mode: 'measured', measured: true })('hasMeasured() && measuredMode()'), true);
  assert.equal(setup({ mode: 'measured', measured: false })('hasMeasured() || measuredMode()'), false);
});

test('the Object tab captions a block the trace never entered as measured, not as projected', () => {
  const [ran, never] = blockCaptions(setup({ mode: 'measured', measured: true }));
  assert.match(ran, /measured executed/, ran);
  assert.match(never, /measured never executed/, never);
  for (const c of [ran, never]) assert.doesNotMatch(c, /projected/, `the projection's words under the Measured overlay: ${c}`);
});

test('the same blocks under Projected keep the projection\'s own words', () => {
  const [ran, never] = blockCaptions(setup({ mode: 'projected', measured: true }));
  // both are projected covered: that contradiction is what Measured reveals
  for (const c of [ran, never]) {
    assert.match(c, /projected covered/, c);
    assert.doesNotMatch(c, /measured/, c);
  }
});

test('a Measured preference left over from other evidence does not relabel evidence with no trace', () => {
  const [ran, never] = blockCaptions(setup({ mode: 'measured', measured: false }));
  for (const c of [ran, never]) {
    assert.match(c, /projected covered/, c);
    assert.doesNotMatch(c, /measured/, c);
  }
});

// The helper's own truth table, beside the rendering tests rather than
// instead of them: every status has to name the overlay its colour came from.
test('blockStatusLabel names the overlay the colour came from', () => {
  const run = setup({ mode: 'measured', measured: true });
  assert.equal(run(`blockStatus(${f}, ${panicBlock})`), 'unc');
  assert.equal(run(`blockStatusLabel(blockStatus(${f}, ${panicBlock}))`), 'measured never executed');
  assert.equal(run(`blockStatusLabel(blockStatus(${f}, ${loopBlock}))`), 'measured executed');
  assert.equal(run(`blockStatusLabel('jus')`), 'justified');
  assert.equal(run(`blockStatusLabel('unk')`), 'unknown');
  const proj = setup({ mode: 'projected', measured: true });
  assert.equal(proj(`blockStatusLabel(blockStatus(${f}, ${panicBlock}))`), 'projected covered');
  assert.equal(proj(`blockStatusLabel('jus')`), 'justified');
});

// README "Projected, measured, and the gap between them": with a trace,
// "every instruction carries its execution count" -- so an instruction the
// trace never ran shows its count, 0, in the Object tab's execution column,
// not the mark for "not measured" (#325).
test('the Object tab shows a never-executed instruction\'s count as 0', () => {
  const run = setup({ mode: 'measured', measured: true });
  blockCaptions(run);
  const cells = JSON.parse(run(`(function () {
    var table = BODY.children[BODY.children.length - 1];
    var out = [];
    (function walk(n) { if (!n || !n.children) return; if (n.tag === 'td' && n.attrs.class === 'x') out.push(n.textContent); n.children.forEach(walk); })(table);
    return JSON.stringify(out);
  })()`));
  assert.deepEqual(cells, ['4', '0']);
});
