// SPDX-License-Identifier: MIT OR Apache-2.0
// Run with: node --test ui/requirements.test.cjs
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// Minimal DOM for testing the actual viewer functions without a browser or
// network dependency. Full layout/interaction checks are done in the viewer.
class Element {
  constructor(tag, text = '') {
    this.tag = tag; this.nodeType = 1; this.children = []; this.attrs = {};
    this.text = text; this.isConnected = true; this.dataset = {};
    this.classList = { toggle() {} };
  }
  append(...children) { this.children.push(...children); }
  setAttribute(k, v) { this.attrs[k] = v; }
  addEventListener() {}
  set textContent(text) { this.text = text; this.children = []; }
  get textContent() { return this.text + this.children.map(c => c.textContent).join(''); }
}
function setup(response) {
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    localStorage: { getItem: () => null },
    document: {
      addEventListener() {}, getElementById: () => null,
      createElement: tag => new Element(tag),
      createTextNode: text => new Element('#text', text),
    },
  });
  const source = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  // Omit only bootstrap; all helpers and renderers are the production code.
  vm.runInContext(source.slice(0, source.lastIndexOf('\ninit().catch')), context);
  const requests = [];
  context.mockRequest = async url => {
    const ids = new URL(url, 'http://localhost').searchParams.get('ids').split(',');
    requests.push(ids);
    return response(ids);
  };
  vm.runInContext('authJson = mockRequest; S.variant = "a";', context);
  return { context, requests, run: code => vm.runInContext(code, context) };
}
function descendants(el, tag) {
  return el.children.flatMap(c => [...(c.tag === tag ? [c] : []), ...descendants(c, tag)]);
}

test('one two-column row per unique function or line requirement; literal safe text', async () => {
  const { context, requests, run } = setup(ids => ({ entries: ids.map(id => ({ id, text: '<script>literal</script>\nSecond line' })) }));
  context.body = new Element('div');
  await run('renderRequirements(body, {name: "example", requirements: ["R1", "R1"], lineRequirements: {12: ["R1", "R2"]}})');
  const table = descendants(context.body, 'table')[0];
  const rows = descendants(descendants(table, 'tbody')[0], 'tr');
  assert.equal(rows.length, 2);
  assert.ok(rows.every(r => r.children.length === 2));
  assert.deepEqual(rows.map(r => r.children[0].textContent), ['R1', 'R2']);
  assert.equal(rows[0].children[1].textContent, '<script>literal</script>\nSecond line');
  assert.equal(descendants(table, 'script').length, 0);
  assert.equal(table.attrs['aria-busy'], 'false');
  assert.deepEqual(requests, [['R1', 'R2']]);
  assert.equal(run('tabOf("requirements")'), 'requirements');
});

test('no tags does not fetch; missing statements and missing sidecar are explicit', async () => {
  const { context, requests, run } = setup(() => ({ present: false }));
  context.body = new Element('div');
  await run('renderRequirements(body, {name:"empty", requirements: []})');
  assert.match(context.body.textContent, /No requirement tags/);
  assert.equal(requests.length, 0);
  context.body = new Element('div');
  await run('renderRequirements(body, {name:"missing", requirements: ["R1"]})');
  assert.match(context.body.textContent, /Requirement text unavailable/);
});

test('batches large tables at the server limit and reuses cached statements', async () => {
  const { requests, run } = setup(ids => ({ entries: ids.map(id => ({ id, text: id })) }));
  await run('needReq(Array.from({length: 600}, (_, i) => "R" + i))');
  assert.deepEqual(requests.map(r => r.length), [256, 256, 88]);
  await run('needReq(["R1", "R599"])');
  assert.equal(requests.length, 3);
});

test('a detached pane is not populated after navigation', async () => {
  let finish;
  const { context, run } = setup(() => new Promise(resolve => { finish = resolve; }));
  context.body = new Element('div');
  const pending = run('renderRequirements(body, {name:"old", requirements:["R1"]})');
  await Promise.resolve();
  context.body.isConnected = false;
  finish({ entries: [{ id: 'R1', text: 'Old text' }] });
  await pending;
  assert.doesNotMatch(context.body.textContent, /Old text/);
});

test('variant switch drops in-flight statements', async () => {
  let finish;
  const { run } = setup(() => new Promise(resolve => { finish = resolve; }));
  const pending = run('needReq(["R1"])');
  await Promise.resolve();
  run('S.variant = "b"; resetReq()');
  finish({ entries: [{ id: 'R1', text: 'Wrong variant' }] });
  await pending;
  assert.equal(run('REQ.size'), 0);
});

test('variant reset before queued fetch does not access a cleared pending batch', async () => {
  const { run, requests } = setup(ids => ({ entries: ids.map(id => ({ id, text: id })) }));
  await run('const old = needReq(["OLD"]); S.variant = "b"; resetReq(); Promise.all([old, needReq(["NEW"])])');
  assert.deepEqual(requests, [['NEW']]);
  assert.equal(run('REQ.has("OLD")'), false);
  assert.equal(run('REQ.has("NEW")'), true);
});
