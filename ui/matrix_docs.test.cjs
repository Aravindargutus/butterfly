// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The support tables in README.md and docs/butterfly.html are generated from
// support/matrix.toml, and may not drift from it (#153).

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const {loadMatrix} = require('../scripts/showcase/spec.cjs');
const {markdown, html, splice, run, idCode, OPEN, CLOSE} = require('../scripts/showcase/matrix-docs.cjs');

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
    assert.ok(md.includes(`<br>${idCode(r.id)} |`), `README block is missing ${r.id}`);
    assert.ok(h.includes(`>${r.id}<`), `manual block is missing ${r.id}`);
  }
  const n = st => rows.filter(r => r.state === st).length;
  assert.ok(md.includes(`**${n('not-supported')} not supported**`));
});

test('a pipe in the text cannot end a Markdown cell early', () => {
  const md = markdown([{id: 'x.y', name: 'a | b', area: 'A', state: 'caveats', caveat: 'one | two'}]);
  assert.ok(md.includes('a \\| b') && md.includes('one \\| two'), md);
});

// What GitHub's renderer needs for each cell to read as written, checked
// against it (CodeQL alert #1): a `|` is always `\|`; a backslash is doubled
// in prose, where it would be a Markdown escape, and left alone in a code
// span, where doubling it would show two.
test('a backslash reads as itself in prose and in code spans', () => {
  const md = markdown([{id: 'x.back\\slash|id', name: 'name a\\b|c', area: 'A', state: 'caveats',
    caveat: 'prose a\\|b and `code a\\|b` and C:\\dir', example: 'ex\\ample', proof: 'p.rs::a\\b'}]);
  const row = md.split('\n').find(l => l.startsWith('| name'));
  assert.equal(row, '| name a\\\\b\\|c<br><code>x.back&#92;slash&#124;id</code> | Handled, with caveats | prose a\\\\\\|b and `code a\\\\|b` and C:\\\\dir | `ex\\ample` | `p.rs::a\\b` |');
});

// GitHub parses the finished `what` cell as one inline string. When the id
// was a backtick span, a lone backtick in the name paired with its opening
// one across the <br>: the id lost its code formatting and the break showed
// as text (PR #9 review). The id opens with no backtick to pair with, and
// writes every character Markdown could act on as an entity.
test('nothing in the name can reach into the id in the what cell', () => {
  for (const name of ['a lone ` here', 'two `` ticks', 'closed `span` then `open', 'a *star and _under']) {
    const md = markdown([{id: 'x.y', name, area: 'A', state: 'caveats', caveat: 'c'}]);
    const what = md.split('\n').find(l => l.startsWith('| ' + name.slice(0, 3))).split(' | ')[0];
    const [, id] = what.split('<br>');
    assert.equal(id, '<code>x.y</code>', what);
  }
  assert.equal(idCode('a`b*c_d[e]f<g>h&i|j~k!l\\m'), '<code>a&#96;b&#42;c&#95;d&#91;e&#93;f&#60;g&#62;h&#38;i&#124;j&#126;k&#33;l&#92;m</code>');
  assert.equal(idCode('BF-x.y-2'), '<code>BF-x.y-2</code>', 'an id of plain characters is written as itself');
});

test('a backtick run opens a code span only when one of the same length closes it', () => {
  const md = markdown([{id: 'x.y', area: 'A', state: 'caveats', caveat: '``a`b\\c`` then a lone ` and d\\e'}]);
  assert.ok(md.includes('``a`b\\c`` then a lone ` and d\\\\e'), md);
});

test('a file without exactly one marked block is refused, not appended to', () => {
  assert.throws(() => splice('no markers here', 'X', 'f.md'), /exactly one/);
  assert.throws(() => splice(`${OPEN}\n${CLOSE}\n${OPEN}\n${CLOSE}`, 'X', 'f.md'), /exactly one/);
  assert.throws(() => splice(`${CLOSE}\n${OPEN}`, 'X', 'f.md'), /exactly one/);
  assert.equal(splice(`a\n  ${OPEN}\nold\n  ${CLOSE}\nb`, `${OPEN}\nnew\n${CLOSE}`, 'f.md'), `a\n  ${OPEN}\nnew\n${CLOSE}\nb`);
});
