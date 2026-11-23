// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The support tables in README.md and docs/butterfly.html are generated from
// support/matrix.toml, and may not drift from it (#153).

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const {loadMatrix} = require('../scripts/showcase/spec.cjs');
const {markdown, html, splice, run, OPEN, CLOSE} = require('../scripts/showcase/matrix-docs.cjs');

test('the committed README and manual blocks are what the matrix generates', () => {
  assert.deepEqual(run(true), [], 'run: node scripts/showcase/matrix-docs.cjs');
});

test('editing the matrix without regenerating fails the check', () => {
  const rows = loadMatrix().map(r => ({...r}));
  rows[0].caveat = `${rows[0].caveat || rows[0].reason || ''} (edited)`;
  // `run(true, …)` compares, never writes
  assert.deepEqual(run(true, rows).sort(), ['README.md', 'docs/butterfly.html']);
});

test('every row appears in both renderings, with its state', () => {
  const rows = loadMatrix();
  const md = markdown(rows);
  const h = html(rows);
  for (const r of rows) {
    assert.ok(md.includes(`\`${r.id}\``), `README block is missing ${r.id}`);
    assert.ok(h.includes(`>${r.id}<`), `manual block is missing ${r.id}`);
  }
  const n = st => rows.filter(r => r.state === st).length;
  assert.ok(md.includes(`**${n('not-supported')} not supported**`));
});

test('a pipe in the text cannot end a Markdown cell early', () => {
  const md = markdown([{id: 'x.y', name: 'a | b', area: 'A', state: 'caveats', caveat: 'one | two'}]);
  assert.ok(md.includes('a \\| b') && md.includes('one \\| two'), md);
});

test('a file without exactly one marked block is refused, not appended to', () => {
  assert.throws(() => splice('no markers here', 'X', 'f.md'), /exactly one/);
  assert.throws(() => splice(`${OPEN}\n${CLOSE}\n${OPEN}\n${CLOSE}`, 'X', 'f.md'), /exactly one/);
  assert.throws(() => splice(`${CLOSE}\n${OPEN}`, 'X', 'f.md'), /exactly one/);
  assert.equal(splice(`a\n  ${OPEN}\nold\n  ${CLOSE}\nb`, `${OPEN}\nnew\n${CLOSE}`, 'f.md'), `a\n  ${OPEN}\nnew\n${CLOSE}\nb`);
});
