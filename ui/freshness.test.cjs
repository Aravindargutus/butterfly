// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/freshness.test.cjs
//
// Whether the change history is still current must never quietly disappear.
// The freshness request swallowed every failure, so when the server's job cap
// answered 503 the stale-history warning just did not appear -- the unsafe
// direction for evidence (review of #67). It is now retried after
// Retry-After, and if it still fails the page says "unknown".
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

/// app.js in a VM whose fetch answers from `answers` in order.
function load(answers) {
  const seen = { fetches: 0, top: 0, route: 0 };
  const context = vm.createContext({
    console: { error() {} }, URL, URLSearchParams, setTimeout, clearTimeout,
    localStorage: { getItem: () => null },
    sessionStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    location: { hash: '', pathname: '/', search: '' },
    history: { replaceState() {} },
    fetch: async () => {
      const a = answers[Math.min(seen.fetches, answers.length - 1)];
      seen.fetches++;
      if (a === 'network') throw new TypeError('connection refused');
      return { ok: a.status < 400, status: a.status, headers: { get: n => (n.toLowerCase() === 'retry-after' ? a.retryAfter ?? null : null) }, json: async () => a.body };
    },
    document: { body: { innerHTML: '' }, addEventListener() {}, getElementById: () => null,
      // nodeType, because h() appends a child as a node only if it has one
      createElement: tag => ({ nodeType: 1, tag, children: [], attrs: {}, dataset: {}, classList: { toggle() {}, add() {} },
        append(...c) { this.children.push(...c); }, setAttribute(k, v) { this.attrs[k] = v; }, addEventListener() {} }),
      createTextNode: t => ({ nodeType: 3, tag: '#text', textContent: t }) },
  });
  const src = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  vm.runInContext(src.slice(0, src.lastIndexOf('\ninit().catch')), context);
  context.__seen = seen;
  vm.runInContext('renderTop = () => { __seen.top++; }; route = () => { __seen.route++; };', context);
  return { run: code => vm.runInContext(code, context), seen };
}

const busy = { status: 503, retryAfter: '0' };
const stale = { status: 200, body: { stale: true, reasons: ['the repository has moved on'] } };
const fresh = { status: 200, body: { stale: false, reasons: [] } };

/// Everything a rendered node says, children included.
function text(n) {
  if (n == null) return '';
  if (typeof n === 'string') return n;
  return (n.textContent || '') + (n.children || []).map(text).join('');
}

test('a busy server is retried, and a stale history still reaches the page', async () => {
  const { run, seen } = load([busy, busy, stale]);
  await run('loadFreshness("v", variantGen)');
  assert.equal(seen.fetches, 3, 'two refusals, then the answer');
  assert.equal(run('S.fresh.stale'), true);
  assert.ok(seen.top >= 1, 'and the stale chip is drawn');
});

test('if the answer never comes, freshness is shown as unknown -- never as nothing', async () => {
  const { run, seen } = load([busy]);
  await run('loadFreshness("v", variantGen)');
  assert.equal(seen.fetches, 3, 'bounded: three attempts in all');
  assert.equal(run('S.fresh.unknown'), true);
  assert.equal(run('!!S.fresh.stale'), false);
  assert.ok(seen.top >= 1, 'the top bar is redrawn with the unknown state');

  const box = { items: [], append(...c) { this.items.push(...c); } };
  run('freshnessBanner')(box);
  // wherever h() keeps a node's text, it is in the serialised tree
  assert.match(JSON.stringify(box.items), /freshness is unknown/, 'and the Changes tab says so');
  assert.match(box.items.map(text).join(''), /could not check/, 'with the reason');
});

test('a failure that is not a refusal for load is not retried', async () => {
  const { run, seen } = load([{ status: 500 }]);
  await run('loadFreshness("v", variantGen)');
  assert.equal(seen.fetches, 1);
  assert.equal(run('S.fresh.unknown'), true);
});

test('a dropped connection is retried like a refusal', async () => {
  const { run, seen } = load(['network', stale]);
  await run('loadFreshness("v", variantGen)');
  assert.equal(seen.fetches, 2);
  assert.equal(run('S.fresh.stale'), true);
});

test('a fresh answer is kept and changes nothing on screen', async () => {
  const { run, seen } = load([fresh]);
  await run('loadFreshness("v", variantGen)');
  assert.equal(run('S.fresh.stale'), false);
  assert.equal(run('!!S.fresh.unknown'), false);
  assert.equal(seen.top, 0);
});

test('an answer for a variant no longer on screen is dropped', async () => {
  const { run, seen } = load([busy, stale]);
  await run('(() => { const p = loadFreshness("v", variantGen); variantGen++; return p; })()');
  assert.equal(run('S.fresh == null'), true, 'nothing is shown for the variant that was left');
  assert.equal(seen.top, 0);
});
