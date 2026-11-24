// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/pan.test.cjs
//
// Every graph redraw called enablePan again on a fresh wrapper without ever
// removing the previous pair of window listeners, so a long review session
// accumulated mousemove/mouseup handlers without bound, each retaining the
// graph it was drawn over (#72). This tests the fix's actual invariant --
// the window listener count is fixed, however many times a graph is
// redrawn -- not just that panning still works.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

class Element {
  constructor(tag) {
    this.tag = tag;
    this.children = [];
    this.attrs = {};
    this.classList = { list: new Set(), add(c) { this.list.add(c); }, remove(c) { this.list.delete(c); }, contains(c) { return this.list.has(c); } };
    this.listeners = {};
    this.scrollLeft = 0;
    this.scrollTop = 0;
  }
  append(...c) { this.children.push(...c); }
  setAttribute(k, v) { this.attrs[k] = v; }
  addEventListener(type, fn) { (this.listeners[type] ??= []).push(fn); }
  removeEventListener() {}
  // mousedown delegates through closest() the same way a real node would
  closest(sel) { return sel.includes(this.tag) ? this : null; }
  fire(type, ev) { for (const fn of this.listeners[type] || []) fn(ev); }
}

function setup() {
  const windowListeners = { mousemove: [], mouseup: [] };
  const mockWindow = {
    addEventListener(type, fn) { (windowListeners[type] ??= []).push(fn); },
    removeEventListener() {},
  };
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    window: mockWindow,
    localStorage: { getItem: () => null },
    document: { addEventListener() {}, getElementById: () => null, createElement: tag => new Element(tag), createTextNode: t => new Element('#text', t) },
  });
  const source = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  vm.runInContext(source.slice(0, source.lastIndexOf('\ninit().catch')), context);
  return { context, windowListeners, run: code => vm.runInContext(code, context) };
}

/// Fires whichever of `windowListeners.mousemove`/`mouseup` were registered
/// at *setup time* -- the fixed, one-time pair -- standing in for a real
/// `window.dispatchEvent`.
function dispatchWindow(windowListeners, type, ev) {
  for (const fn of windowListeners[type]) fn(ev);
}

test('enablePan adds no window listeners of its own -- panning uses the ones registered once at load', () => {
  const { context, windowListeners, run } = setup();
  const before = { mousemove: windowListeners.mousemove.length, mouseup: windowListeners.mouseup.length };
  assert.ok(before.mousemove >= 1 && before.mouseup >= 1, 'the fixed pair must exist from module load');

  context.wrap = new Element('div');
  run('enablePan(wrap)');
  run('enablePan(wrap)'); // a second graph render, same as a redraw would do

  assert.deepEqual({ mousemove: windowListeners.mousemove.length, mouseup: windowListeners.mouseup.length }, before, 'enablePan must never grow the window listener count');
});

test('200 redraws leave the window listener count exactly where it started', () => {
  const { context, windowListeners, run } = setup();
  const before = { mousemove: windowListeners.mousemove.length, mouseup: windowListeners.mouseup.length };
  for (let i = 0; i < 200; i++) {
    context.wrap = new Element('div');
    run('enablePan(wrap)');
  }
  assert.deepEqual({ mousemove: windowListeners.mousemove.length, mouseup: windowListeners.mouseup.length }, before, 'no growth across 200 redraws, matching the issue\'s own reproduction');
});

test('a drag still pans the wrapper that started it', () => {
  const { context, windowListeners, run } = setup();
  context.wrap = new Element('div');
  context.wrap.scrollLeft = 50;
  context.wrap.scrollTop = 20;
  run('enablePan(wrap)');

  run('wrap.fire("mousedown", { clientX: 100, clientY: 100, target: { closest: () => null } })');
  assert.ok(context.wrap.classList.contains('dragging'), 'a drag starts visibly');

  dispatchWindow(windowListeners, 'mousemove', { clientX: 130, clientY: 90 });
  assert.equal(context.wrap.scrollLeft, 20, '50 - (130-100)');
  assert.equal(context.wrap.scrollTop, 30, '20 - (90-100)');

  dispatchWindow(windowListeners, 'mouseup', {});
  assert.ok(!context.wrap.classList.contains('dragging'), 'the drag ends');
});

test('mousedown on a node does not start a drag', () => {
  const { context, windowListeners, run } = setup();
  context.wrap = new Element('div');
  context.wrap.scrollLeft = 0;
  run('enablePan(wrap)');

  // e.target.closest(".node, .bf-node") returning truthy is the only thing
  // this guard checks -- a plain object with that method is a faithful
  // stand-in for a real DOM node inside one.
  run('wrap.fire("mousedown", { clientX: 0, clientY: 0, target: { closest: () => true } })');
  dispatchWindow(windowListeners, 'mousemove', { clientX: 40, clientY: 0 });

  assert.equal(context.wrap.scrollLeft, 0, 'no drag started, so the move must not pan it');
  assert.ok(!context.wrap.classList.contains('dragging'));
});

test('two different graphs pan independently, and only the one being dragged moves', () => {
  const { context, windowListeners, run } = setup();
  context.a = new Element('div'); context.a.scrollLeft = 0;
  context.b = new Element('div'); context.b.scrollLeft = 0;
  run('enablePan(a); enablePan(b)');

  run('a.fire("mousedown", { clientX: 0, clientY: 0, target: { closest: () => null } })');
  dispatchWindow(windowListeners, 'mousemove', { clientX: 40, clientY: 0 });

  assert.equal(context.a.scrollLeft, -40, 'a is being dragged');
  assert.equal(context.b.scrollLeft, 0, 'b, drawn earlier, must not move');
});

test('a graph removed mid-drag terminates safely: no throw, and nothing left to leak', () => {
  const { context, windowListeners, run } = setup();
  context.wrap = new Element('div');
  run('enablePan(wrap)');
  run('wrap.fire("mousedown", { clientX: 0, clientY: 0, target: { closest: () => null } })');

  // the pane was replaced: nothing in the real app calls wrap.remove(), the
  // reference is simply dropped by innerHTML = ''. The detached element is
  // still the one panState points at, and must not throw when touched.
  assert.doesNotThrow(() => dispatchWindow(windowListeners, 'mousemove', { clientX: 10, clientY: 0 }));
  assert.doesNotThrow(() => dispatchWindow(windowListeners, 'mouseup', {}));

  // and the very next drag, on a fresh graph, is unaffected by the stale one
  context.fresh = new Element('div'); context.fresh.scrollLeft = 0;
  run('enablePan(fresh)');
  run('fresh.fire("mousedown", { clientX: 0, clientY: 0, target: { closest: () => null } })');
  dispatchWindow(windowListeners, 'mousemove', { clientX: 15, clientY: 0 });
  assert.equal(context.fresh.scrollLeft, -15);
});
