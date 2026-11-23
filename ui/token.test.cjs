// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/token.test.cjs
//
// The bearer token must never be in a request target (#65). It used to reach
// the page as ?token=, so the first request -- for the page itself -- carried
// it in clear, into the request line and any proxy log, before this script
// could move it. It now arrives in the fragment, which a browser never sends,
// or is typed into the page. These run the real app.js in a VM with a
// recording fetch and history, and hold both paths to that.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const SECRET = 'f00dfeedf00dfeedf00dfeedf00dfeed';

/// `status` is what every fetch answers with.
function load(href, { stored = null, status = 200 } = {}) {
  const u = new URL(href);
  const seen = { fetches: [], replaced: [], reloads: 0 };
  const store = new Map(stored == null ? [] : [['sl-token', stored]]);
  const location = { href, origin: u.origin, pathname: u.pathname, search: u.search, hash: u.hash, reload() { seen.reloads++; } };
  const history = {
    replaceState(_s, _t, url) {
      seen.replaced.push(String(url));
      const n = new URL(String(url), u.origin);
      Object.assign(location, { href: n.href, pathname: n.pathname, search: n.search, hash: n.hash });
    },
  };
  const els = {};
  const body = { innerHTML: '' };
  const context = vm.createContext({
    console: { error() {} }, URL, URLSearchParams, AbortController, setTimeout, clearTimeout, location, history,
    localStorage: { getItem: () => null },
    sessionStorage: { getItem: k => (store.has(k) ? store.get(k) : null), setItem: (k, v) => store.set(k, String(v)), removeItem: k => store.delete(k) },
    fetch: (url, opts = {}) => {
      seen.fetches.push({ url: String(url), headers: opts.headers || {} });
      return Promise.resolve({ ok: status < 400, status, headers: { get: () => null }, json: async () => ({}), text: async () => 'refused' });
    },
    document: { body, addEventListener() {}, getElementById: id => els[id] || null, querySelector: sel => els[sel.replace(/^#/, '')] || null,
      createElement: tag => ({ tag, children: [], attrs: {}, dataset: {}, classList: { toggle() {}, add() {} },
        append(...c) { this.children.push(...c); }, setAttribute() {}, addEventListener() {} }),
      createTextNode: t => ({ tag: '#text', textContent: t }) },
  });
  const src = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  vm.runInContext(src.slice(0, src.lastIndexOf('\ninit().catch')), context);
  return { run: code => vm.runInContext(code, context), seen, store, els, location, body };
}

/// Every request the page made, with nothing of the token in its target.
function assertNoTokenInTargets(seen, token = SECRET) {
  for (const f of seen.fetches) {
    assert.ok(!f.url.includes(token), `the token is in a request target: ${f.url}`);
    assert.ok(!/token/i.test(f.url), `a request target names a token: ${f.url}`);
  }
}

test('a token in the fragment is kept for the tab and sent only as a header', async () => {
  const { run, seen, store, location } = load(`http://127.0.0.1:8787/#token=${SECRET}`);
  assert.equal(store.get('sl-token'), SECRET, 'the tab has the token');
  assert.equal(seen.replaced.length, 1, 'the address bar was rewritten once');
  assert.ok(!seen.replaced[0].includes(SECRET) && !seen.replaced[0].includes('#'), `and the fragment is gone from it: ${seen.replaced[0]}`);
  assert.equal(location.hash, '', 'so the router starts from a clean address');

  await run(`Promise.all([authFetch('/api/variants'), authJson('/api/evidence?v=a'), authFetch('/api/report')])`);
  assert.equal(seen.fetches.length, 3);
  assertNoTokenInTargets(seen);
  for (const f of seen.fetches) assert.equal(f.headers.authorization, `Bearer ${SECRET}`, `${f.url} carries the header`);
});

test('a fragment token keeps its own +, & and =, and percent-escapes are decoded', () => {
  // URLSearchParams read `+` as a space and stopped at `&` (review of #65)
  for (const [fragment, want] of [
    ['abc+def%2Bghi-0123456789', 'abc+def+ghi-0123456789'],
    ['a&b=c-0123456789abcdef', 'a&b=c-0123456789abcdef'],
    ['pct%25sign-0123456789ab', 'pct%sign-0123456789ab'],
    ['not%zzvalid-0123456789a', 'not%zzvalid-0123456789a'],
  ]) {
    const { store } = load(`http://127.0.0.1:8787/#token=${fragment}`);
    assert.equal(store.get('sl-token'), want, `#token=${fragment}`);
  }
});

test('a token in the query string is not taken up', () => {
  // The server refuses such a URL (it has already been sent); the page must
  // not make it work by lifting the token out of it.
  const { seen, store } = load(`http://127.0.0.1:8787/?token=${SECRET}`);
  assert.equal(store.has('sl-token'), false);
  assert.equal(seen.replaced.length, 0);
});

test('an ordinary route in the fragment is left alone', () => {
  const { seen, store } = load('http://127.0.0.1:8787/#/fn/3/mcdc');
  assert.equal(seen.replaced.length, 0, 'the router owns every fragment that is not a token');
  assert.equal(store.has('sl-token'), false);
});

test('the token page takes a token without it ever being in a URL', async () => {
  const { run, seen, store, els } = load('http://127.0.0.1:8787/');
  const handlers = {};
  els.tokenForm = { addEventListener: (type, fn) => { handlers[type] = fn; } };
  els.tokenInput = { value: SECRET };
  const body = { innerHTML: '' };
  run('tokenPage')(body);
  assert.match(body.innerHTML, /type="password"/, 'the field hides what is typed');
  assert.ok(!body.innerHTML.includes('token='), 'and the page no longer tells anyone to build a URL with the token in it');

  let prevented = false;
  handlers.submit({ preventDefault() { prevented = true; } });
  assert.ok(prevented, 'the form is never submitted: that would put the field in a request');
  assert.equal(store.get('sl-token'), SECRET, 'the tab now has the token');
  assert.equal(seen.reloads, 1, 'and the viewer starts again with it');
  assert.equal(seen.replaced.length, 0);

  await run(`authFetch('/api/variants')`);
  assertNoTokenInTargets(seen);
  assert.equal(seen.fetches[0].headers.authorization, `Bearer ${SECRET}`);
});

test('a token the server refused is forgotten, and the page says so', () => {
  const { run, store, els } = load('http://127.0.0.1:8787/', { stored: 'the-wrong-token-0000000000000000' });
  els.tokenForm = { addEventListener() {} };
  const body = { innerHTML: '' };
  run('tokenPage')(body);
  assert.equal(store.has('sl-token'), false, 'a refused token is not sent again');
  assert.match(body.innerHTML, /was not accepted/);
  assert.ok(!body.innerHTML.includes('the-wrong-token'), 'and is not shown');
});

test('a 401 after the viewer has loaded goes to the token page and forgets the token', async () => {
  // e.g. the server restarted and minted a new one (review of #65)
  const { run, store, els, body } = load('http://127.0.0.1:8787/', { stored: SECRET, status: 401 });
  els.tokenForm = { addEventListener() {} };
  await run(`authJson('/api/freshness').catch(() => 'refused')`);
  assert.equal(store.has('sl-token'), false, 'the stale token is forgotten');
  assert.match(body.innerHTML, /needs an access token/, 'and the reader is asked for the current one');
  assert.match(body.innerHTML, /was not accepted/);

  // a write keeps its own error, so an unsaved review is not wiped with the page
  const w = load('http://127.0.0.1:8787/', { stored: SECRET, status: 401 });
  const r = await w.run(`authFetch('/api/resolutions', { method: 'POST', body: '{}' })`);
  assert.equal(r.status, 401);
  assert.equal(w.body.innerHTML, '', 'the page is left as it was');
});

test('the token page puts the viewer\'s own listeners away', () => {
  // They were registered for a body the token page replaces; left in place,
  // '/' swallowed the slash of a token being typed (review of #65).
  const { run, els } = load('http://127.0.0.1:8787/', { stored: SECRET });
  els.tokenForm = { addEventListener() {} };
  const signal = run('viewerListeners = new AbortController(); viewerListeners.signal');
  run('reqPinned = true');
  run('tokenPage')({ innerHTML: '' });
  assert.equal(signal.aborted, true, 'every listener registered with the signal is removed');
  assert.equal(run('reqPinned'), false, 'and a pinned requirement panel lets go');
});

test('a slash typed into a field is text, not the search shortcut', () => {
  const { run, els } = load('http://127.0.0.1:8787/');
  let focused = 0;
  els.search = { tagName: 'INPUT', focus() { focused++; } };
  const key = (key, target) => {
    let prevented = false;
    run('viewerKeydown')({ key, target, preventDefault() { prevented = true; } });
    return prevented;
  };
  // the token field, a review dialog's textarea and anything contenteditable
  for (const target of [{ tagName: 'INPUT', id: 'tokenInput' }, { tagName: 'TEXTAREA' }, { tagName: 'DIV', isContentEditable: true }]) {
    assert.equal(key('/', target), false, `'/' in a ${target.tagName} must reach the field`);
  }
  assert.equal(focused, 0, 'and focus stays where the reader is typing');
  // on the page itself, '/' still jumps to the search box
  assert.equal(key('/', { tagName: 'BODY' }), true);
  assert.equal(focused, 1);
  // with no search box at all -- the token page is up -- nothing throws
  delete els.search;
  assert.equal(key('/', { tagName: 'BODY' }), false);
  assert.doesNotThrow(() => key('Escape', { tagName: 'INPUT' }));
});
