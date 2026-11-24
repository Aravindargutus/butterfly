// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/callgraph.test.cjs
//
// The expandable call graph (#100). These cover the parts that decide
// whether the graph can be trusted rather than the parts that decide whether
// it looks right: what an edge establishes, what a chain through a weak edge
// establishes, and whether recursion terminates.
//
// Deliberately no browser. The model is a pure function precisely so that
// cycles, confidence and the node budget can be proven without one.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function setup() {
  // The same minimal host the other viewer tests build: app.js touches
  // localStorage and document as it loads, long before anything calls the
  // function under test.
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    localStorage: { getItem: () => null, setItem() {} },
    document: {
      addEventListener() {}, getElementById: () => null,
      createElement: tag => ({ tag, children: [], attrs: {}, dataset: {},
        classList: { toggle() {}, add() {}, contains: () => false },
        append(...c) { this.children.push(...c); }, setAttribute(k, v) { this.attrs[k] = v; },
        addEventListener() {},
        set textContent(t) { this._t = t; }, get textContent() { return this._t || ''; } }),
      createTextNode: text => ({ tag: '#text', children: [], textContent: text }),
    },
  });
  const source = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  vm.runInContext(source.slice(0, source.lastIndexOf('\ninit().catch')), context);
  return (ev, focus, expanded = [], opts = {}) => {
    context.__ev = ev;
    context.__exp = expanded;
    context.__opts = opts;
    return vm.runInContext('callGraphModel(__ev, ' + focus + ', new Set(__exp), __opts)', context);
  };
}

const fnn = (id, name, extra = {}) => ({ id, name, callees: [], callers: [], ...extra });
const call = (name, functionId, line = 1) => ({ symbol: name, functionId, sites: [{ line }] });

test('a function with neighbours of its own is offered for expansion, a leaf is not', () => {
  const graph = setup();
  const ev = { functions: [
    fnn(0, 'main', { callees: [call('helper', 1)] }),
    fnn(1, 'helper', { callees: [call('leaf', 2)], callers: [call('main', 0)] }),
    fnn(2, 'leaf', { callers: [call('helper', 1)] }),
  ] };
  const g = graph(ev, 0);
  const helper = g.nodes.find(n => n.name === 'helper');
  assert.equal(helper.expandable, true, 'helper still has a callee to show');

  const opened = graph(ev, 0, ['1:1']);
  const leaf = opened.nodes.find(n => n.name === 'leaf');
  assert.equal(leaf.expandable, false, 'a leaf must not offer an expansion that opens nothing');
});

test('recursion terminates and is drawn as a cycle rather than a second node', () => {
  const graph = setup();
  const ev = { functions: [
    fnn(0, 'main', { callees: [call('rec', 1)] }),
    fnn(1, 'rec', { callees: [call('rec', 1, 9)], callers: [call('main', 0)] }),
  ] };
  const g = graph(ev, 0, ['1:1']);
  assert.equal(g.nodes.filter(n => n.name === 'rec').length, 1, 'the recursive function is one node, not two');
  const selfEdge = g.edges.find(e => e.from === '1:1' && e.to === '1:1');
  assert.ok(selfEdge, 'the recursion is still visible as an edge');
  assert.equal(selfEdge.cycle, true, 'and it is marked as a cycle');
});

test('mutual recursion terminates', () => {
  const graph = setup();
  const ev = { functions: [
    fnn(0, 'main', { callees: [call('ping', 1)] }),
    fnn(1, 'ping', { callees: [call('pong', 2)], callers: [call('main', 0)] }),
    fnn(2, 'pong', { callees: [call('ping', 1)], callers: [call('ping', 1)] }),
  ] };
  const g = graph(ev, 0, ['1:1', '1:2']);
  assert.equal(g.nodes.filter(n => n.name === 'ping').length, 1);
  assert.equal(g.nodes.filter(n => n.name === 'pong').length, 1);
});

test('a chain through a candidate edge is a hypothesis, and stays one', () => {
  const graph = setup();
  const ev = { functions: [
    fnn(0, 'a', { callees: [call('b', 1)] }),
    fnn(1, 'b', { callers: [call('a', 0)],
      indirectCalls: [{ line: 5, measured: [], candidates: [{ name: 'c', functionId: 2, symbolId: 9 }] }] }),
    fnn(2, 'c', { callees: [call('d', 3)] }),
    fnn(3, 'd', {}),
  ] };
  const g = graph(ev, 0, ['1:1', '1:2']);
  const by = n => g.nodes.find(x => x.name === n);
  assert.equal(by('b').conf, 'fact', 'a call instruction is a fact');
  assert.equal(by('c').conf, 'hypothesis', 'a candidate target is not an observed call');
  assert.equal(by('d').conf, 'hypothesis', 'and nothing downstream of it recovers certainty');
});

test('a trace-resolved indirect call is a fact about the run, not about the program', () => {
  const graph = setup();
  const ev = { functions: [
    fnn(0, 'a', { indirectCalls: [{ line: 3, measured: [{ name: 'b', functionId: 1, symbolId: 4, count: 2 }], candidates: [] }] }),
    fnn(1, 'b', {}),
  ] };
  const g = graph(ev, 0);
  assert.equal(g.nodes.find(n => n.name === 'b').conf, 'run');
});

test('an inlined callee is not expandable, and its calls are not drawn twice', () => {
  const graph = setup();
  // main inlines helper; helper's call to log therefore appears as main's
  // own call instruction. One instruction must be one box.
  const ev = { functions: [
    fnn(0, 'main', { callees: [call('log', 2, 4)], inlined: [{ name: 'helper', functionId: 1, callLine: 3 }] }),
    fnn(1, 'helper', { callees: [call('log', 2, 9)] }),
    fnn(2, 'log', {}),
  ] };
  const g = graph(ev, 0);
  const helper = g.nodes.find(n => n.name === 'helper');
  assert.equal(helper.expandable, false, 'expanding it would show one call instruction as two facts');
  assert.equal(g.nodes.filter(n => n.name === 'log').length, 1);
});

test('a folded call and an external symbol offer no expansion', () => {
  const graph = setup();
  const ev = { functions: [
    fnn(0, 'main', { callees: [call('memcpy', null)], foldedCalls: [{ name: 'gone', functionId: 1, line: 7 }] }),
    fnn(1, 'gone', {}),
  ] };
  const g = graph(ev, 0);
  const ext = g.nodes.find(n => n.name === 'memcpy');
  assert.equal(ext.external, true);
  assert.equal(ext.expandable, false, 'its object code is not in this evidence');
  assert.equal(g.nodes.find(n => n.name === 'gone').expandable, false);
});

test('callers and callees are separate statements about the same function', () => {
  const graph = setup();
  // util both calls main and is called by main
  const ev = { functions: [
    fnn(0, 'main', { callees: [call('util', 1)], callers: [call('util', 1)] }),
    fnn(1, 'util', { callees: [call('main', 0)], callers: [call('main', 0)] }),
  ] };
  const g = graph(ev, 0);
  assert.equal(g.nodes.filter(n => n.name === 'util').length, 2, 'upstream and downstream are different claims');
  // Compared as a string, not with deepEqual: arrays built inside the vm
  // context have that realm's Array prototype, so a structural comparison
  // fails on identity alone.
  assert.equal(g.nodes.filter(n => n.name === 'util').map(n => n.dir).sort().join(','), '-1,1');
});

test('the node budget stops the graph and says so, rather than hanging', () => {
  const graph = setup();
  const ev = { functions: [fnn(0, 'hub')] };
  for (let i = 1; i <= 60; i++) {
    ev.functions[0].callees.push(call('f' + i, i, i));
    ev.functions.push(fnn(i, 'f' + i, { callers: [call('hub', 0)] }));
  }
  const g = graph(ev, 0, [], { maxNodes: 12, perNode: 8 });
  assert.ok(g.nodes.length <= 12, `budget honoured, got ${g.nodes.length}`);
  assert.equal(g.nodes.find(n => n.id === 'focus').hidden, 52, 'and it says how many it did not draw');
});

test('a capped caller side survives an uncapped callee side', () => {
  const graph = setup();
  // 30 callers against a per-node cap of 12, and two callees that fit. The
  // focus is visited once per direction, and `hidden` used to be assigned
  // rather than accumulated -- so the callee pass overwrote the caller
  // pass and 18 hidden callers read as "+0 not shown".
  const ev = { functions: [fnn(0, 'hub')] };
  for (let i = 1; i <= 30; i++) {
    ev.functions.push(fnn(i, 'caller' + i, { callees: [call('hub', 0, i)] }));
    ev.functions[0].callers.push(call('caller' + i, i, i));
  }
  for (let i = 31; i <= 32; i++) {
    ev.functions.push(fnn(i, 'callee' + i));
    ev.functions[0].callees.push(call('callee' + i, i, i));
  }
  const g = graph(ev, 0);
  assert.equal(g.nodes.find(n => n.id === 'focus').hidden, 18, 'the hidden callers must not be erased by the callee pass');
});

test('an indirect site with no resolved targets stays on the graph', () => {
  const graph = setup();
  // Missing evidence is not a leaf: a dispatcher whose only call is an
  // unresolved pointer must not read as calling nothing (review of #101).
  const ev = { functions: [
    fnn(0, 'dispatch', { indirectCalls: [{ line: 85, measured: [], candidates: [] }] }),
  ] };
  const g = graph(ev, 0);
  const site = g.nodes.find(n => n.kind === 'indirect-site');
  assert.ok(site, 'the unresolved site must appear');
  assert.match(site.name, /indirect call #1/);
  assert.equal(site.expandable, false);
  assert.equal(site.conf, 'hypothesis');
  assert.match(site.note, /L85/);
});

test('the per-node cap can be raised through opts.more', () => {
  const graph = setup();
  const ev = { functions: [fnn(0, 'hub')] };
  for (let i = 1; i <= 13; i++) {
    ev.functions.push(fnn(i, 'f' + i));
    ev.functions[0].callees.push(call('f' + i, i, i));
  }
  const capped = graph(ev, 0);
  assert.equal(capped.nodes.length, 13, '12 shown plus the focus');
  assert.equal(capped.nodes.find(n => n.id === 'focus').hidden, 1);

  const revealed = graph(ev, 0, [], { more: new Map([['focus:1', 12]]) });
  assert.equal(revealed.nodes.length, 14, 'the reveal must actually add f13');
  assert.equal(revealed.nodes.find(n => n.id === 'focus').hidden, 0);
});

test('an inlined first occurrence must not suppress a later real call', () => {
  const graph = setup();
  // root -> A and B; A inlines C, B calls C directly; C -> D. Whichever of
  // A or B is enumerated first, C is expandable and D reachable: the
  // inlined status belongs to A's occurrence, not to C (review of #101).
  const build = order => {
    const ev = { functions: [
      fnn(0, 'root'),
      fnn(1, 'A', { inlined: [{ name: 'C', functionId: 3, callLine: 5 }], callers: [call('root', 0)] }),
      fnn(2, 'B', { callees: [call('C', 3)], callers: [call('root', 0)] }),
      fnn(3, 'C', { callees: [call('D', 4)] }),
      fnn(4, 'D', {}),
    ] };
    ev.functions[0].callees = order.map(i => call(ev.functions[i].name, i));
    return ev;
  };
  for (const order of [[1, 2], [2, 1]]) {
    const g = graph(build(order), 0, ['1:1', '1:2', '1:3']);
    const c = g.nodes.find(n => n.name === 'C');
    assert.equal(c.expandable, true, `order ${order}: C is really called and really calls D`);
    assert.equal(c.kind, 'direct', `order ${order}: the direct occurrence outranks the inlined one`);
    assert.ok(g.nodes.find(n => n.name === 'D'), `order ${order}: D must be reachable`);
  }
});

test('a direct path upgrades a candidate-first confidence, in either order', () => {
  const graph = setup();
  // A reaches C only as a candidate; B calls C directly; C -> D. In both
  // enumeration orders C and D are facts -- the label "reached only through
  // a candidate" must never depend on enumeration order (review of #101).
  const build = order => {
    const ev = { functions: [
      fnn(0, 'root'),
      fnn(1, 'A', { indirectCalls: [{ line: 9, measured: [], candidates: [{ name: 'C', functionId: 3, symbolId: 7 }] }], callers: [call('root', 0)] }),
      fnn(2, 'B', { callees: [call('C', 3)], callers: [call('root', 0)] }),
      fnn(3, 'C', { callees: [call('D', 4)] }),
      fnn(4, 'D', {}),
    ] };
    ev.functions[0].callees = order.map(i => call(ev.functions[i].name, i));
    return ev;
  };
  for (const order of [[1, 2], [2, 1]]) {
    const g = graph(build(order), 0, ['1:1', '1:2', '1:3']);
    assert.equal(g.nodes.find(n => n.name === 'C').conf, 'fact', `order ${order}`);
    assert.equal(g.nodes.find(n => n.name === 'D').conf, 'fact', `order ${order}: the upgrade must propagate`);
  }
});

test('a folded or inlined occurrence lends no certainty to a candidate path, in either order', () => {
  const graph = setup();
  // root -> A and B; A folds (or inlines) its call to C; B reaches C only as
  // a candidate; C -> D. The non-call occurrence proves the absence of a
  // call, not a route to C's out-of-line body, so everything reached at run
  // time through C stays a hypothesis whichever of A or B comes first
  // (review of #101).
  const build = (order, occurrence) => {
    const ev = { functions: [
      fnn(0, 'root'),
      fnn(1, 'A', { ...occurrence, callers: [call('root', 0)] }),
      fnn(2, 'B', { indirectCalls: [{ line: 9, measured: [], candidates: [{ name: 'C', functionId: 3, symbolId: 7 }] }], callers: [call('root', 0)] }),
      fnn(3, 'C', { callees: [call('D', 4)] }),
      fnn(4, 'D', {}),
    ] };
    ev.functions[0].callees = order.map(i => call(ev.functions[i].name, i));
    return ev;
  };
  const occurrences = {
    folded: { foldedCalls: [{ name: 'C', functionId: 3, line: 5 }] },
    inlined: { inlined: [{ name: 'C', functionId: 3, callLine: 5 }] },
  };
  for (const [kind, occurrence] of Object.entries(occurrences)) {
    for (const order of [[1, 2], [2, 1]]) {
      const at = `${kind}, order ${order}`;
      const g = graph(build(order, occurrence), 0, ['1:1', '1:2', '1:3']);
      assert.equal(g.nodes.find(n => n.name === 'C').conf, 'hypothesis', `${at}: C is reached only through a candidate`);
      assert.equal(g.nodes.find(n => n.name === 'D').conf, 'hypothesis', `${at}: and so is D`);
      const cToD = g.edges.filter(e => e.from === '1:3' && e.to === '1:4');
      assert.equal(cToD.length, 1, at);
      assert.equal(cToD[0].conf, 'hypothesis', `${at}: C->D is a hypothesis`);
      // The occurrence itself is still shown for what it is.
      const aToC = g.edges.find(e => e.from === '1:1' && e.to === '1:3');
      assert.equal(aToC.kind, kind, at);
      assert.equal(aToC.conf, 'fact', `${at}: the ${kind} occurrence is still a fact`);
    }
  }
});

test('unresolved sites in two functions are two nodes, each with its own line', () => {
  const graph = setup();
  // A and B each number their one unresolved site #1. Keyed by label, they
  // merged into one node with two incoming edges and A's line -- unrelated
  // unknown calls presented as a shared target (review of #101).
  const ev = { functions: [
    fnn(0, 'root', { callees: [call('A', 1), call('B', 2)] }),
    fnn(1, 'A', { indirectCalls: [{ line: 10, measured: [], candidates: [] }], callers: [call('root', 0)] }),
    fnn(2, 'B', { indirectCalls: [{ line: 40, measured: [], candidates: [] }], callers: [call('root', 0)] }),
  ] };
  const g = graph(ev, 0, ['1:1', '1:2']);
  const sites = g.nodes.filter(n => n.kind === 'indirect-site');
  assert.equal(sites.length, 2, 'one node per site, not one per label');
  for (const [owner, line] of [['1:1', 'L10'], ['1:2', 'L40']]) {
    const into = g.edges.filter(e => e.from === owner && sites.some(s => s.id === e.to));
    assert.equal(into.length, 1, `${owner} has its own site`);
    const site = sites.find(s => s.id === into[0].to);
    assert.match(site.note, new RegExp(line), `${owner}'s site keeps its own line`);
    assert.equal(g.edges.filter(e => e.to === site.id).length, 1, `${owner}'s site is not shared`);
  }
});

test('a genuine external symbol is still one node however many functions call it', () => {
  const graph = setup();
  const ev = { functions: [
    fnn(0, 'root', { callees: [call('A', 1), call('B', 2)] }),
    fnn(1, 'A', { callees: [call('memcpy', null, 3)], callers: [call('root', 0)] }),
    fnn(2, 'B', { callees: [call('memcpy', null, 8)], callers: [call('root', 0)] }),
  ] };
  const g = graph(ev, 0, ['1:1', '1:2']);
  const ext = g.nodes.filter(n => n.name === 'memcpy');
  assert.equal(ext.length, 1, 'one symbol, one node');
  assert.equal(g.edges.filter(e => e.to === ext[0].id).length, 2, 'reached from both callers');
});

test('reconciliation does not duplicate edges', () => {
  const graph = setup();
  const ev = { functions: [
    fnn(0, 'root'),
    fnn(1, 'A', { indirectCalls: [{ line: 9, measured: [], candidates: [{ name: 'C', functionId: 3, symbolId: 7 }] }], callers: [call('root', 0)] }),
    fnn(2, 'B', { callees: [call('C', 3)], callers: [call('root', 0)] }),
    fnn(3, 'C', { callees: [call('D', 4)] }),
    fnn(4, 'D', {}),
  ] };
  ev.functions[0].callees = [call('A', 1), call('B', 2)];
  const g = graph(ev, 0, ['1:1', '1:2', '1:3']);
  const cToD = g.edges.filter(e => e.to === '1:4');
  assert.equal(cToD.length, 1, 'the C->D edge is walked twice by reconciliation but drawn once');
});

test('an unknown focus produces an empty graph rather than throwing', () => {
  const graph = setup();
  const g = graph({ functions: [] }, 7);
  assert.equal(g.nodes.length, 0);
  assert.equal(g.edges.length, 0);
});
