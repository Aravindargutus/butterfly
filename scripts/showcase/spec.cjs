// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Reads and validates `support/features.toml` (#154).
//
// Both halves of the generator -- the capture script and the page builder --
// go through here, so a malformed entry is refused once, by name, before a
// browser is launched or a page is written. The rules are the epic's (#150):
// a section that shows something must say what backs it, how to reproduce it
// and where it stops; a section that shows nothing must say which issue it is
// waiting for.

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const {parse} = require('./toml.cjs');

const ROOT = path.resolve(__dirname, '../..');
const FEATURES = path.join(ROOT, 'support/features.toml');
// SHOWCASE_MATRIX lets the renderer be tried against a matrix that has not
// merged yet; the page footer-side text still says where it came from.
const MATRIX = process.env.SHOWCASE_MATRIX || path.join(ROOT, 'support/matrix.toml');

const STEP_VERBS = ['click', 'hover', 'fill', 'select', 'press', 'wait', 'wait_gone', 'pause', 'scroll', 'move', 'wheel', 'drag'];
// The version of `capture.json` this generator reads and writes. Bumped when
// the shape changes in a way that would make an older file render something
// untrue: /2 records the facts each section consumed WITH that section, so a
// `--only` run cannot fill an unrefreshed section from a refreshed fixture
// (#212 review, finding 1).
const CAPTURE_SCHEMA = 'butterfly-showcase-capture/2';

/// Every string of a section that is resolved through `fill()`.
///
/// Titles are deliberately not here: a placeholder in a title is caught by
/// build.cjs's last look over the rendered page, which is what keeps the
/// builder's own un-filled strings out of the page.
function filledTexts(f) {
  return [
    f.claim,
    ...(f.limits || []),
    ...(f.shot || []).flatMap(s => [s.caption, s.requires_reason, ...(s.expect || [])]),
    ...(f.cmd || []).map(c => c.caption),
  ].filter(x => typeof x === 'string');
}

/// The evidence a section may quote numbers from: the fixtures it names, the
/// fixtures its shots name, and the keys of its own `facts` block.
///
/// A section's numbers come from its own evidence and from nowhere else. That
/// is what lets a capture run write down, beside each section, exactly the
/// facts that section consumed -- which is what stops a partial refresh from
/// rewriting another section's caption (#212 review, finding 1).
function factSources(f) {
  return new Set([
    ...(f.fixtures || []),
    ...(f.shot || []).map(s => s.fixture).filter(Boolean),
    ...Object.keys(f.facts || {}),
  ]);
}

/// The top-level fact keys a section actually quotes, `requires` included.
function factKeys(f) {
  const keys = new Set();
  for (const text of filledTexts(f)) {
    for (const m of text.matchAll(/\{\{([^}]+)\}\}/g)) keys.add(m[1].trim().split('.')[0]);
  }
  for (const s of f.shot || []) if (typeof s.requires === 'string') keys.add(s.requires.split('.')[0]);
  return [...keys].sort();
}

function load(file = FEATURES) {
  const doc = parse(fs.readFileSync(file, 'utf8'), path.relative(ROOT, file));
  const problems = [];
  const bad = (where, msg) => problems.push(`${where}: ${msg}`);

  const fixtures = doc.fixture || {};
  for (const [name, fx] of Object.entries(fixtures)) {
    const w = `fixture.${name}`;
    if (!Array.isArray(fx.build) || !fx.build.length) bad(w, 'needs `build`, the commands that produce it');
    if (typeof fx.evidence !== 'string' && !Array.isArray(fx.evidence)) bad(w, 'needs `evidence`, the file(s) `butterfly serve` is given');
    if (typeof fx.ci !== 'boolean') bad(w, 'needs `ci = true|false`: is this example built by CI? (#152)');
    if (typeof fx.dir !== 'string') bad(w, 'needs `dir`, the example it is built from');
  }

  const features = doc.feature || [];
  const seen = new Set();
  const files = new Set();
  for (const f of features) {
    const w = f.id || '(feature without id)';
    if (!/^V\d\d$/.test(f.id || '')) bad(w, '`id` must be V01..V99');
    if (seen.has(f.id)) bad(w, 'duplicate id');
    seen.add(f.id);
    for (const k of ['title', 'claim', 'group']) if (typeof f[k] !== 'string' || !f[k].trim()) bad(w, `needs \`${k}\``);
    if (!Number.isInteger(f.issue)) bad(w, 'needs `issue`, the section issue number');
    if (!['captured', 'pending'].includes(f.status)) bad(w, '`status` must be "captured" or "pending"');
    if (f.status === 'pending') {
      if (!Array.isArray(f.blocked_by) || !f.blocked_by.length) bad(w, 'a pending section names the issue(s) it waits for in `blocked_by`');
      if (typeof f.pending_reason !== 'string') bad(w, 'a pending section says why in `pending_reason`');
      // A pending section may already carry the shots and commands it will
      // use: they are checked for shape here and run by nobody, so the day
      // its blocker merges it flips to `captured` by changing one word.
    }
    const live = f.status === 'captured';
    // A captured section is a claim, and a claim carries its limits.
    if (live && !Array.isArray(f.limits)) bad(w, 'needs `limits` (an empty list is a statement too, but it must be made)');
    if (live && (!Array.isArray(f.fixtures) || !f.fixtures.length)) bad(w, 'needs `fixtures`, the example(s) that back it');
    for (const fx of f.fixtures || []) if (!fixtures[fx]) bad(w, `unknown fixture \`${fx}\``);
    if (live && !(f.shot || []).length && !(f.cmd || []).some(c => c.show)) bad(w, 'a captured section shows at least one screenshot or one command output');
    // Printed as the first comment of the reproduce block: what the reader
    // must already have, which the block cannot install for them.
    if (f.prereq !== undefined && (typeof f.prereq !== 'string' || !f.prereq.trim())) bad(w, '`prereq` is a sentence naming what the block needs and cannot install');
    for (const s of f.shot || []) {
      const sw = `${w} shot ${s.file || '?'}`;
      if (!/^[a-z0-9-]+$/.test(s.file || '')) bad(sw, '`file` is a bare lower-case name; the extension is chosen at capture time');
      if (files.has(s.file)) bad(sw, 'duplicate file name');
      files.add(s.file);
      if (typeof s.caption !== 'string' || s.caption.length < 20) bad(sw, 'needs a `caption` that can stand as the alt text');
      if (s.kind !== 'file' && typeof s.route !== 'string') bad(sw, 'needs `route`');
      // A screenshot with no `expect` is a picture nobody checks: the page
      // would publish whatever the viewer happened to draw. Six shots had
      // none (#212 S12).
      if (!Array.isArray(s.expect) || !s.expect.length) bad(sw, 'needs `expect`: the text the screenshot must show, checked inside the published crop before the shot is taken');
      // `requires_reason` is rendered in place of the screenshot on a host
      // where `requires` is falsy, and it goes through fill(). Nothing used
      // to demand it, so a host that took that branch got a crash instead of
      // a reason (#212 S23).
      if (s.requires !== undefined && (typeof s.requires_reason !== 'string' || !s.requires_reason.trim())) {
        bad(sw, '`requires` needs `requires_reason`: what this host cannot show, and why');
      }
      if (s.requires === undefined && s.requires_reason !== undefined) bad(sw, '`requires_reason` without `requires` is never rendered');
      const fx = s.fixture || (f.fixtures || [])[0];
      if (!fixtures[fx]) bad(sw, `unknown fixture \`${fx}\``);
      for (const st of s.steps || []) {
        const verbs = Object.keys(st).filter(k => STEP_VERBS.includes(k));
        if (verbs.length !== 1) bad(sw, `a step has exactly one verb of ${STEP_VERBS.join(', ')}: ${JSON.stringify(st)}`);
      }
    }
    // A number a section quotes must come from evidence that section names.
    // Otherwise the capture run cannot record, beside the section, the facts
    // its text was filled from, and a `--only` refresh of one section rewrote
    // another section's caption over that section's untouched screenshot
    // (#212 review, finding 1).
    const sources = factSources(f);
    for (const k of factKeys(f)) {
      if (!sources.has(k)) {
        bad(w, `quotes \`{{${k}....}}\`, and \`${k}\` is not one of this section's fixtures or \`facts\` keys (${[...sources].join(', ') || 'none'}). A section's numbers come from its own evidence, so that a partial capture can keep them with it.`);
      }
    }
    for (const c of f.cmd || []) {
      const cw = `${w} cmd \`${(c.run || '?').slice(0, 40)}\``;
      if (typeof c.run !== 'string') bad(cw, 'needs `run`');
      if (!Number.isInteger(c.exit)) bad(cw, 'needs `exit`, the status the command is expected to end with');
      if (c.show && (typeof c.caption !== 'string' || !c.caption)) bad(cw, 'a shown command needs a `caption`');
    }
  }
  for (const n of doc.not_claimed || []) {
    if (typeof n.text !== 'string' || typeof n.source !== 'string') bad('not_claimed', 'each entry needs `text` and `source`');
  }
  if (problems.length) {
    throw new Error(`support/features.toml:\n  ${problems.join('\n  ')}`);
  }
  return {fixtures, features, notClaimed: doc.not_claimed || [], groups: doc.group || []};
}

/// Does the captured text show `want`?
///
/// Case-insensitive, because the viewer upper-cases headings in CSS and the
/// page reports them as drawn; whitespace-insensitive, because a line break
/// is a layout decision. NOT a bare substring test: `"28 gaps"` used to be
/// satisfied by `"128 gaps"` (#212 S12), so a word boundary is required
/// wherever the expectation itself begins or ends on a word character.
function wantPattern(want) {
  const quoted = want.replace(/[.*+?^${}()|[\]\\]/g, '\\$&').replace(/\s+/g, '\\s+');
  return (/^\w/.test(want) ? '\\b' : '') + quoted + (/\w$/.test(want) ? '\\b' : '');
}
function matches(haystack, want) {
  return new RegExp(wantPattern(want), 'i').test(haystack);
}

/// `support/matrix.toml`, when #151 has merged. Its absence is a state the
/// page renders, not an error.
function loadMatrix(file = MATRIX) {
  if (!fs.existsSync(file)) return null;
  const doc = parse(fs.readFileSync(file, 'utf8'), path.relative(ROOT, file));
  // `doc.row` by name, not "whichever top-level array of tables all carry a
  // `state`". That guess picked the FIRST such array, so giving `[[local_only]]`
  // a `state` key would have truncated the rendered matrix to the one-row
  // local-only list, silently (#213). The schema is #151's `[[row]]`.
  const rows = doc.row;
  if (!Array.isArray(rows) || !rows.length) throw new Error('support/matrix.toml: no `[[row]]` entries were found');
  return rows;
}

module.exports = {ROOT, FEATURES, MATRIX, STEP_VERBS, CAPTURE_SCHEMA, load, loadMatrix, matches, wantPattern, factKeys, factSources, filledTexts};
