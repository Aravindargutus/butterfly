#!/usr/bin/env node
// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The support matrix, rendered into the README and the manual (#153).
//
// support/matrix.toml is the one statement of what butterfly supports, and a
// test keeps it honest (#151). The README and the manual used to carry their
// own hand-written support tables, which drifted from it -- the README still
// said C++ and Rust were built by no CI run after cpp and cpp-lib were. Both
// now carry a block generated from the TOML, between markers:
//
//   README.md            <!-- butterfly:matrix -->  ...  <!-- /butterfly:matrix -->   (Markdown)
//   docs/butterfly.html  <!-- butterfly:matrix -->  ...  <!-- /butterfly:matrix -->   (HTML)
//
//   node scripts/showcase/matrix-docs.cjs            rewrite both blocks
//   node scripts/showcase/matrix-docs.cjs --check    exit 1 if either differs
//
// ui/matrix_docs.test.cjs runs the check, so editing the TOML without
// regenerating fails the build.

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const {ROOT, loadMatrix} = require('./spec.cjs');
const {esc, prose, issue} = require('./markup.cjs');

const OPEN = '<!-- butterfly:matrix -->';
const CLOSE = '<!-- /butterfly:matrix -->';
const NOTE = 'Generated from support/matrix.toml by scripts/showcase/matrix-docs.cjs. Do not edit by hand.';

const LABEL = {supported: 'Supported', caveats: 'Handled, with caveats', 'not-supported': 'Not supported'};
const CHIP = {supported: 'good', caveats: 'major', 'not-supported': 'blocker'};
const uniq = a => [...new Set(a)];
const areaOf = r => r.area || 'other';
const tracking = r => [].concat(r.tracking || []);

function summary(rows) {
  const n = st => rows.filter(r => r.state === st).length;
  return {total: rows.length, supported: n('supported'), caveats: n('caveats'), not: n('not-supported')};
}

/// A table cell: one line, a `|` that cannot end the cell early, and a
/// backslash that reads as itself.
///
/// GitHub's table parser drops one backslash before each `|` and only then
/// parses the cell as Markdown, so every `|` is written `\|`. After that, a
/// backslash outside a code span is a Markdown escape, so it is doubled; inside
/// a code span it is literal, and doubling it there would show two. Checked
/// against GitHub's renderer: `a\|b` as text needs `a\\\|b`, and `` `a\|b` ``
/// needs only `` `a\\|b` `` (CodeQL alert #1). A backtick run opens a code span
/// only if a run of the same length closes it, as in CommonMark.
function cell(s) {
  const text = String(s || '').replace(/\s*\n\s*/g, ' ').trim();
  const out = [];
  let codeEnd = -1;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === '`' && i > codeEnd) {
      let j = i;
      while (text[j] === '`') j++;
      const run = j - i;
      let k = j;
      for (;;) {
        k = text.indexOf('`'.repeat(run), k);
        if (k < 0 || (text[k - 1] !== '`' && text[k + run] !== '`')) break;
        while (text[k] === '`') k++;
      }
      out.push(text.slice(i, j));
      if (k >= 0) codeEnd = k + run - 1;
      i = j - 1;
    } else if (c === '`') {
      out.push(c);
    } else if (c === '|') {
      out.push('\\|');
    } else if (c === '\\' && i > codeEnd) {
      out.push('\\\\');
    } else {
      out.push(c);
    }
  }
  return out.join('');
}

/// The row's id, set apart from the name before it in the same cell.
///
/// GitHub parses a finished cell as one inline string, so a backtick in the
/// name could pair with a backtick that opened the id's code span, across the
/// `<br>`, and the id would lose its code formatting (PR #9 review). An HTML
/// `<code>` element opens with no backtick for the name to pair with. Its text
/// is still parsed as Markdown, so every character Markdown or the table
/// parser could act on is written as a numeric entity, which the renderer
/// shows as the character itself; `|` included, so no backslash is needed.
function idCode(v) {
  const text = String(v || '').replace(/\s*\n\s*/g, ' ').trim();
  return `<code>${text.replace(/[\\`*_[\]<>&|~!]/g, c => `&#${c.charCodeAt(0)};`)}</code>`;
}

function markdown(rows) {
  const s = summary(rows);
  const out = [OPEN, `<!-- ${NOTE} -->`, ''];
  out.push(`${s.total} rows from [\`support/matrix.toml\`](support/matrix.toml), which a test checks textually against the examples, evidence lines and proof tests they name: **${s.supported} supported**, **${s.caveats} handled with caveats**, **${s.not} not supported**. The caveat column is the point of the table: a row says what was shown, on which example, and what was not. The [showcase](docs/showcase/index.html) renders the same rows beside what each feature looks like.`);
  out.push('');
  for (const area of uniq(rows.map(areaOf))) {
    const inArea = rows.filter(r => areaOf(r) === area);
    const s2 = summary(inArea);
    out.push(`<details><summary><b>${esc(area)}</b> — ${s2.total} rows: ${s2.supported} supported, ${s2.caveats} with caveats, ${s2.not} not supported</summary>`);
    out.push('');
    out.push('| what | state | caveat, or why it is not supported | example | proof |');
    out.push('|---|---|---|---|---|');
    for (const r of inArea) {
      // example and proof are each a cell's only code span: wrapped before
      // escaping, so cell() leaves their backslashes as they are. The id
      // shares its cell with the name, so it is not a backtick span.
      const code = v => cell(`\`${v}\``);
      const what = `${cell(r.name || r.id)}<br>${idCode(r.id)}`;
      const why = cell(r.caveat || r.reason) + (tracking(r).length ? ` Tracked in ${tracking(r).map(n => `#${n}`).join(', ')}.` : '');
      out.push(`| ${what} | ${LABEL[r.state] || r.state} | ${why} | ${r.example ? code(r.example) : '—'} | ${r.proof ? code(r.proof) : ''} |`);
    }
    out.push('');
    out.push('</details>');
    out.push('');
  }
  out.push(CLOSE);
  return out.join('\n');
}

function html(rows) {
  const s = summary(rows);
  const row = r => `<tr><td>${prose(r.name || r.id)}<br><span class="mono">${esc(r.id)}</span></td><td><span class="chip ${CHIP[r.state] || 'adv'}">${esc(LABEL[r.state] || r.state)}</span></td><td>${prose(r.caveat || r.reason || '')}${
    tracking(r).length ? ` Tracked in ${tracking(r).map(issue).join(', ')}.` : ''}</td><td>${r.example ? `<span class="mono">${esc(r.example)}</span>` : '&mdash;'}</td><td>${r.proof ? `<span class="mono">${esc(r.proof)}</span>` : ''}</td></tr>`;
  const areas = uniq(rows.map(areaOf)).map(area => {
    const inArea = rows.filter(r => areaOf(r) === area);
    return `  <h3>${esc(area)}</h3>\n  <div class="tw">\n    <table>\n      <tr><th>What</th><th>State</th><th>Caveat, or why it is not supported</th><th>Example</th><th>Proof</th></tr>\n${
      inArea.map(r => `      ${row(r)}`).join('\n')}\n    </table>\n  </div>`;
  });
  return [
    OPEN,
    `  <!-- ${NOTE} -->`,
    `  <p>${s.total} rows from <span class="mono">support/matrix.toml</span>, which a test checks textually against the examples, evidence lines and proof tests they name: <strong>${s.supported} supported</strong>, <strong>${s.caveats} handled with caveats</strong>, <strong>${s.not} not supported</strong>. The caveat column is the point of the table.</p>`,
    ...areas,
    `  ${CLOSE}`,
  ].join('\n');
}

const TARGETS = [
  {file: 'README.md', render: markdown},
  {file: 'docs/butterfly.html', render: html},
];

/// `text` with the block between the markers replaced, or an error naming
/// the file when the markers are missing or out of order.
function splice(text, block, file) {
  const a = text.indexOf(OPEN);
  const b = text.indexOf(CLOSE);
  if (a < 0 || b < 0 || b < a || text.indexOf(OPEN, a + 1) >= 0) {
    throw new Error(`${file}: expected exactly one ${OPEN} ... ${CLOSE} block`);
  }
  // Whatever precedes the opening marker on its line (an indent) belongs to
  // the file, so the block is spliced in from the marker itself.
  return text.slice(0, a) + block.trimStart() + text.slice(b + CLOSE.length);
}

function run(check, rows = loadMatrix()) {
  const stale = [];
  for (const t of TARGETS) {
    const p = path.join(ROOT, t.file);
    const before = fs.readFileSync(p, 'utf8');
    const after = splice(before, t.render(rows), t.file);
    if (after === before) continue;
    if (check) stale.push(t.file);
    else fs.writeFileSync(p, after);
  }
  return stale;
}

if (require.main === module) {
  const check = process.argv.includes('--check');
  const stale = run(check);
  if (check && stale.length) {
    console.error(`matrix-docs: ${stale.join(' and ')} ${stale.length === 1 ? 'does' : 'do'} not match support/matrix.toml. Run: node scripts/showcase/matrix-docs.cjs`);
    process.exit(1);
  }
  if (!check) console.log('matrix-docs: README.md and docs/butterfly.html are in step with support/matrix.toml');
}

module.exports = {markdown, html, splice, run, cell, idCode, OPEN, CLOSE};
