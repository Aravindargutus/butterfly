#!/usr/bin/env node
// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Writes the one showcase fixture that no compiler on the host can produce:
// the evidence an x86-64 image gives the Variables panel (#154, V09).
//
//   node scripts/showcase/synthetic.cjs vars-x86_64 <out/evidence.json>
//
// It is not a new fixture. It is the document `ui/browser/checks/vars.cjs`
// already serves in CI to assert the unsupported-target state, written to a
// file so the capture run and a reader following the "Reproduce it" block
// serve the same bytes. The page labels it as hand-written wherever it shows.

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const vars = require('../../ui/browser/checks/vars.cjs');

const [kind, out] = process.argv.slice(2);
const kinds = {
  'vars-x86_64': () => vars.evidence({arch: 'x86_64', objectFormat: 'elf', variables: vars.UNSUPPORTED}),
  'vars-aarch64': () => vars.evidence({arch: 'aarch64', objectFormat: 'macho', variables: vars.ATTRIBUTED}),
};
if (!kinds[kind] || !out) {
  console.error(`usage: synthetic.cjs <${Object.keys(kinds).join('|')}> <out.json>`);
  process.exit(2);
}
fs.mkdirSync(path.dirname(out), {recursive: true});
fs.writeFileSync(out, `${JSON.stringify(kinds[kind](), null, 2)}\n`);
console.log(`wrote ${out} (hand-written ${kind} evidence from ui/browser/checks/vars.cjs)`);
