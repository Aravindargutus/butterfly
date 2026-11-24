// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/showcase_capture.test.cjs
//
// What a PARTIAL capture (`scripts/showcase.sh --only V02`) may not do
// (#154, review of #212).
//
// A section on the showcase page is one statement: this is what the tool
// showed, and these are the numbers it showed. The two halves have to come
// from one moment. `--only` keeps the unrefreshed sections' records and
// screenshots from the previous run and rebuilds only the named sections'
// fixtures -- and the numbers used to be ONE object shared by the whole page,
// which the rebuilt fixture overwrote. Two sections back onto `examples/c`;
// refreshing one of them rewrote the other's caption from "2 of 3 conditions
// proven" to "3 of 3" over the screenshot taken when it was 2 of 3, with both
// still badged as captured. The partial-run notice does not make that true.
//
// The capture run therefore writes each section's numbers down BESIDE that
// section, and the builder fills a section from those and from nothing else.
// The test below is the reviewer's: two sections share a fixture, one is
// refreshed against changed evidence, and the other must not move.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {render} = require('../scripts/showcase/build.cjs');
const {CAPTURE_SCHEMA} = require('../scripts/showcase/spec.cjs');
const {view, record, mergePartial, prune} = require('../scripts/showcase/capture.cjs');

const META = () => ({
  commit: 'a'.repeat(40), commitShort: 'aaaaaaaaaaaa', commitDate: '2026-09-19T00:00:00Z',
  branch: 'feature/154-showcase-generator', modified: 0, capturedAt: '2026-09-19T00:00:00Z',
  os: 'Darwin 25.0.0', arch: 'arm64', tools: {butterfly: 'butterfly 0.1.0'},
});

/// Two sections, V01 and V02, both backed by the one fixture `demo`, both
/// quoting its MC/DC numbers in the caption of their own screenshot. This is
/// the shape of V02 and V06 in support/features.toml.
const SPEC = {
  fixtures: {demo: {dir: 'examples/c', ci: true, build: ['./examples/c/build.sh'], evidence: 'examples/c/build/evidence.json'}},
  groups: [{id: 'g', title: 'A group'}],
  notClaimed: [],
  features: ['V01', 'V02'].map(id => ({
    id, issue: 1, group: 'g', title: `Section ${id}`, status: 'captured',
    claim: `${id} claims something about examples/c.`, limits: [], fixtures: ['demo'],
    shot: [{
      file: `${id.toLowerCase()}-mcdc`, route: '#/overview',
      caption: `mode_permits_engage in examples/c: {{demo.conditionsProven}} of {{demo.conditionsTotal}} conditions proven, ${id}.`,
    }],
  })),
};

const dirWith = files => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'showcase-capture-'));
  fs.mkdirSync(path.join(dir, 'img'));
  for (const [name, bytes] of Object.entries(files)) fs.writeFileSync(path.join(dir, 'img', name), Buffer.alloc(bytes, 7));
  return dir;
};
const IMAGES = {'v01-mcdc.png': 11, 'v02-mcdc.png': 22};

/// One capture run over `ids`, as capture.cjs assembles it: a record per
/// section, and each section's own view of the facts recorded beside it.
function run({ids, facts, commit = 'a'.repeat(40), at = '2026-09-19T00:00:00Z'}) {
  const manifest = {
    schema: CAPTURE_SCHEMA,
    meta: {...META(), commit, commitShort: commit.slice(0, 12), capturedAt: at},
    fixtures: {demo: {ok: true, commands: []}},
    features: {},
    factSets: {},
  };
  for (const id of ids) {
    const f = SPEC.features.find(x => x.id === id);
    manifest.features[id] = {
      commands: [], commit, capturedAt: at,
      shots: [{file: `${id.toLowerCase()}-mcdc.png`, source: f.shot[0].file, bytes: IMAGES[`${id.toLowerCase()}-mcdc.png`], width: 10, height: 10, url: 'http://127.0.0.1:8787/#/overview'}],
    };
    record(manifest, f, view(facts, f));
  }
  return prune(manifest);
}

const PROVEN_2 = {demo: {conditionsProven: 2, conditionsTotal: 3}};
const PROVEN_3 = {demo: {conditionsProven: 3, conditionsTotal: 3}};

const build = (cap, dir) => render({spec: SPEC, cap, matrix: null, inline: false, dir});
/// The caption an <img> was published under, by image file name.
const captionOf = (html, file) => new RegExp(`<img src="img/${file}"[^>]*alt="([^"]*)"`).exec(html)?.[1];

test('a partial refresh does not rewrite a section it did not recapture', () => {
  const dir = dirWith(IMAGES);
  // The full run: both sections, two of three conditions proven.
  const full = run({ids: ['V01', 'V02'], facts: PROVEN_2});
  assert.match(captionOf(build(full, dir), 'v02-mcdc.png'), /2 of 3 conditions proven/);

  // `--only V01` on a checkout whose examples/c has changed, so the fixture
  // now reports all three proven. V01 is rebuilt, recaptured and rephotographed;
  // V02 is not touched, and keeps the image taken when it was 2 of 3.
  const partial = run({ids: ['V01'], facts: PROVEN_3, commit: 'b'.repeat(40), at: '2026-09-20T00:00:00Z'});
  const merged = prune(mergePartial(full, partial, new Set(['V01'])));
  const html = build(merged, dir);

  assert.match(captionOf(html, 'v01-mcdc.png'), /3 of 3 conditions proven/, 'the refreshed section shows what the refreshed evidence says');
  assert.match(captionOf(html, 'v02-mcdc.png'), /2 of 3 conditions proven/, 'the section that was NOT recaptured keeps the numbers its screenshot was taken with');
  assert.doesNotMatch(captionOf(html, 'v02-mcdc.png'), /3 of 3/);
  // ...and the page still says it is a partial capture, which is a separate
  // statement and never a licence to print a number the picture contradicts.
  assert.match(html, /<b>Partial capture\.<\/b>/);
  assert.match(html, /V01/);
});

test('two partial refreshes in a row each keep their own numbers', () => {
  const dir = dirWith(IMAGES);
  const full = run({ids: ['V01', 'V02'], facts: PROVEN_2});
  const a = prune(mergePartial(full, run({ids: ['V01'], facts: PROVEN_3}), new Set(['V01'])));
  // a third state of the evidence, refreshing V02 this time
  const later = {demo: {conditionsProven: 1, conditionsTotal: 3}};
  const b = prune(mergePartial(a, run({ids: ['V02'], facts: later}), new Set(['V02'])));
  const html = build(b, dir);
  assert.match(captionOf(html, 'v01-mcdc.png'), /3 of 3 conditions proven/);
  assert.match(captionOf(html, 'v02-mcdc.png'), /1 of 3 conditions proven/);
  assert.deepEqual(b.meta.partial, ['V01', 'V02'], 'both runs are named, not just the last');
});

test('a snapshot no section references any more is not carried for ever', () => {
  const full = run({ids: ['V01', 'V02'], facts: PROVEN_2});
  assert.equal(Object.keys(full.factSets).length, 1, 'one fixture, one snapshot, however many sections quote it');
  const a = prune(mergePartial(full, run({ids: ['V01'], facts: PROVEN_3}), new Set(['V01'])));
  assert.equal(Object.keys(a.factSets).length, 2, 'V02 still needs the old numbers');
  // refresh V02 too, and the old snapshot has no reader left
  const b = prune(mergePartial(a, run({ids: ['V02'], facts: PROVEN_3}), new Set(['V02'])));
  assert.equal(Object.keys(b.factSets).length, 1);
});

test('a section is only handed the evidence it names', () => {
  const f = SPEC.features[0];
  const got = view({demo: {conditionsProven: 2}, other: {conditionsProven: 9}}, f);
  assert.deepEqual(Object.keys(got), ['demo']);
});

test('a capture that cannot say which numbers a section used is refused', () => {
  const dir = dirWith(IMAGES);
  const full = run({ids: ['V01', 'V02'], facts: PROVEN_2});

  // the shape before this fix: one `facts` object for the whole page
  const shared = JSON.parse(JSON.stringify(full));
  delete shared.factSets;
  for (const r of Object.values(shared.features)) delete r.facts;
  shared.facts = PROVEN_2;
  assert.throws(() => build(shared, dir), /records no facts for the section/);

  // ...and an older schema is named as such rather than half-read
  const old = {...JSON.parse(JSON.stringify(full)), schema: 'butterfly-showcase-capture/1'};
  assert.throws(() => build(old, dir), /and this builder reads butterfly-showcase-capture\/2/);

  // a snapshot the manifest lost is an error, not an empty object
  const lost = JSON.parse(JSON.stringify(full));
  lost.factSets = {};
  assert.throws(() => build(lost, dir), /has no fact snapshot/);
});
