// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/tool_build.test.cjs
//
// The viewer names the build of butterfly that wrote the evidence in the
// same words as the report and `butterfly --version` (#117), and evidence
// written before the field existed never reads as a clean build.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function setup() {
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    localStorage: { getItem: () => null, setItem() {} },
    document: { addEventListener() {}, getElementById: () => null, querySelector: () => null,
      createElement: tag => ({ tag, children: [], attrs: {}, dataset: {}, classList: { toggle() {}, add() {} },
        append(...c) { this.children.push(...c); }, setAttribute() {}, addEventListener() {} }),
      createTextNode: t => ({ tag: '#text', textContent: t }) },
  });
  const src = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  const boot = src.lastIndexOf('\ninit().catch');
  assert.ok(boot > 0, 'ui/app.js still ends with the init() call this test cuts away');
  vm.runInContext(src.slice(0, boot), context);
  return code => vm.runInContext(code, context);
}

const describe = pv => setup()(`toolBuild(${JSON.stringify(pv)})`);
const ID = '0123456789abcdef0123456789abcdef01234567';

test('a recorded commit is named with the tree state, in the report\'s words', () => {
  assert.equal(describe({ toolCommit: ID, toolDirty: false }), `commit ${ID}, clean tree`);
  assert.equal(describe({ toolCommit: ID, toolDirty: true }), `commit ${ID}, tree modified at build`);
  assert.equal(describe({ toolCommit: ID, toolDirty: null }), `commit ${ID}, tree state unknown`);
  assert.equal(describe({ toolCommit: ID }), `commit ${ID}, tree state unknown`);
});

test('an unknown commit says so, and older evidence never reads as clean', () => {
  assert.equal(describe({ toolCommit: 'unknown', toolDirty: null }), 'commit unknown');
  assert.equal(describe({}), 'commit not recorded');
  assert.equal(describe({ toolCommit: '', toolDirty: false }), 'commit not recorded');
});
