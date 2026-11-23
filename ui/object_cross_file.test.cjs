// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/object_cross_file.test.cjs
//
// In the Object tab an instruction can come from a file other than the
// function's own: code inlined from a header carries `ins.fileId`. The source
// row it sits under is that file's line, so its colour, count, text and
// requirement chips, and the instruction's own tooltip and chips, must be
// looked up in that file -- not by line number in the function's own file,
// which is what the tab did until #317. These drive renderObject() on
// hand-written evidence: no compiler on any host is asked to inline across
// files for the check to run.
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
  // main.c line 2 ran 7 times and tags REQ-MAIN-2; hdr.h line 2 never ran and
  // tags REQ-HDR-2. Same line number, different files: the one case the tab
  // used to conflate.
  vm.runInContext(`S.res = new Map(); S.ev = { measured: null, functions: [], files: [
    { id: 0, display: 'main.c', text: ['int main(void) {', '  int v = clamp(9); // @satisfies REQ-MAIN-2', '  return v;', '}'],
      lines: [{ count: 7 }, { count: 7 }, { count: 7 }, { count: 7 }] },
    { id: 1, display: 'hdr.h', text: ['static inline int clamp(int x) {', '  if (x > 5) return 5; // @satisfies REQ-HDR-2', '  return x;', '}'],
      lines: [{ count: 3 }, { count: 0 }, { count: 3 }, { count: 3 }] } ] };`, context);
  return code => vm.runInContext(code, context);
}

// Two instructions on "line 2": the function's own, then one inlined from
// hdr.h. `calleeId` is what the evidence says about the inlined callee: its
// function id when it is a function of its own, null when it is not.
function evidence(calleeId) {
  const body = `{ gaps: [], disagreements: [], instrTotal: 2, instrProjectedCovered: 1, instrProjectedUncovered: 1, instrUnknown: 0,
    compilerGenerated: {}, address: 4096, size: 8, blocks: [{ id: 0, address: 4096, kind: 'entry', succs: [], instrs: [
      { addr: 4096, line: 2, mnemonic: 'mov', operands: 'w0, #9', flags: [], gapIds: [], projected: 'covered' },
      { addr: 4100, line: 2, fileId: 1, inlined: 0, origin: 'inlined:clamp', mnemonic: 'cmp', operands: 'w0, #5', flags: [], gapIds: [], projected: 'uncovered' } ] }] }`;
  return `S.ev.functions = [
    { id: 0, name: 'main', fileId: 0, count: 7, requirements: ['REQ-MAIN'], lineRequirements: { 2: ['REQ-MAIN-2'] },
      inlined: [{ name: 'clamp', functionId: ${calleeId}, callLine: 2, ranges: [], instrCount: 1, depth: 0 }],
      branches: [], decisions: [], lineStart: 1, lineEnd: 4, linesTotal: 4, linesCovered: 4, object: ${body} },
    { id: 1, name: 'clamp', fileId: 1, count: 3, requirements: ['REQ-CLAMP'], lineRequirements: { 2: ['REQ-HDR-2'] }, inlined: [],
      branches: [], decisions: [], lineStart: 1, lineEnd: 4, linesTotal: 4, linesCovered: 3,
      object: { gaps: [], disagreements: [], instrTotal: 0, instrProjectedCovered: 0, instrProjectedUncovered: 0, instrUnknown: 0, compilerGenerated: {}, address: 8192, size: 0, blocks: [] } } ];`;
}

// Every row of the Object table: its class, title and text, and the class
// and text of each requirement chip in it.
const srcRows = rows => rows.filter(r => /^srcl /.test(r.cls || '') && !/\borphan\b/.test(r.cls));
const insRows = rows => rows.filter(r => /^i /.test(r.cls || ''));
function render(run, calleeId) {
  run(`${evidence(calleeId)} F = S.ev.functions[0]; BODY = h('div', {}); renderObject(BODY, F);`);
  return JSON.parse(run(`JSON.stringify(BODY.children[BODY.children.length - 1].children.map(r => {
    const chips = []; const walk = e => { for (const c of (e.children || [])) { if (c.attrs && /\\b(req|rq)\\b/.test(c.attrs.class || '')) chips.push(c.textContent); walk(c); } };
    walk(r); return { cls: r.attrs.class, title: r.attrs.title, text: r.textContent, chips };
  }))`));
}

test('a source row from another file shows that file\'s line: status, count, text and tags (#317)', () => {
  const rows = render(setup(), 1);
  const src = srcRows(rows);
  assert.equal(src.length, 2, `main.c:2 and hdr.h:2 are two source rows, not one: ${JSON.stringify(src)}`);
  // the function's own line is untouched
  assert.match(src[0].cls, /\bcov\b/);
  assert.match(src[0].text, /^L2.*7×/);
  assert.deepEqual(src[0].chips, ['REQ-MAIN-2']);
  // the header's line: never ran, no count of main.c's, its own text and tag
  assert.match(src[1].text, /^hdr\.h:L2/, src[1].text);
  assert.match(src[1].cls, /\bunc\b/, `hdr.h:2 never ran, whatever main.c:2 did: ${src[1].cls}`);
  assert.match(src[1].text, /0×/, `hdr.h:2's count, not main.c:2's: ${src[1].text}`);
  assert.match(src[1].text, /REQ-HDR-2/, src[1].text);
  assert.doesNotMatch(src[1].text, /clamp\(9\)|REQ-MAIN-2/, `main.c:2's text or tag on hdr.h's row: ${src[1].text}`);
  assert.deepEqual(src[1].chips, ['REQ-HDR-2']);
});

test('the inlined instruction\'s tooltip and chips are its own line\'s, not main.c\'s line of the same number (#317)', () => {
  const rows = render(setup(), 1);
  const ins = insRows(rows);
  assert.equal(ins.length, 2);
  assert.equal(ins[0].title, 'main.c:2  int v = clamp(9); // @satisfies REQ-MAIN-2');
  assert.deepEqual(ins[0].chips, ['REQ-MAIN-2']);
  assert.equal(ins[1].title, 'hdr.h:2  if (x > 5) return 5; // @satisfies REQ-HDR-2');
  assert.deepEqual(ins[1].chips, ['REQ-HDR-2'], 'the tag on hdr.h:2, read from the callee that owns that file');
});

test('with no per-line tag data for the other file, the row shows no line chip rather than a wrong one (#317)', () => {
  // The inlined callee is not a function of its own in the evidence, so no
  // function owns hdr.h's lines: there is nothing correct to show on the line,
  // and the instruction falls through to the enclosing function's tag.
  const rows = render(setup(), null);
  const src = srcRows(rows);
  assert.equal(src.length, 2);
  assert.deepEqual(src[1].chips, [], `main.c:2's tag must not be shown on hdr.h:2: ${JSON.stringify(src[1])}`);
  const ins = insRows(rows);
  assert.deepEqual(ins[1].chips, ['REQ-MAIN'], 'function-level fallthrough, as before');
});

test('a justification on main.c:2 does not justify the instruction from hdr.h:2, and one on hdr.h:2 does (#317)', () => {
  const run = setup();
  run(`S.res.set(lineKey(0, 2), { kind: 'analysis' });`);
  let ins = insRows(render(run, 1));
  assert.match(ins[1].cls, /\bunc\b/, `justified by another file's line: ${ins[1].cls}`);
  run(`S.res.clear(); S.res.set(lineKey(1, 2), { kind: 'analysis' });`);
  ins = insRows(render(run, 1));
  assert.match(ins[1].cls, /\bjus\b/, ins[1].cls);
  // and so is the source row above it
  const src = srcRows(render(run, 1));
  assert.match(src[1].cls, /\bjus\b/, src[1].cls);
});
