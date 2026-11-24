// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/footer.test.cjs
//
// The footer states what the colours on the page mean, so it has to follow
// the overlay. It used to print the first provenance caveat unconditionally,
// and read "Projected, not measured" while the Measured overlay was on (#209).
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function setup() {
  const footer = { innerHTML: '' };
  const context = vm.createContext({
    console: { error() {} }, URL, setTimeout, clearTimeout,
    localStorage: { getItem: () => null, setItem() {} },
    document: { addEventListener() {}, getElementById: () => null,
      querySelector: sel => (sel === '#caveat' ? footer : null),
      createElement: tag => ({ tag, children: [], attrs: {}, dataset: {}, classList: { toggle() {}, add() {} },
        append(...c) { this.children.push(...c); }, setAttribute() {}, addEventListener() {} }),
      createTextNode: t => ({ tag: '#text', textContent: t }) },
  });
  const src = fs.readFileSync(`${__dirname}/app.js`, 'utf8');
  // The viewer's last line boots it; everything above it is the module under
  // test. If that line is ever renamed, slice(0, -1) would silently evaluate
  // the WHOLE file and the tests would pass against a page that booted.
  const boot = src.lastIndexOf('\ninit().catch');
  assert.ok(boot > 0, 'ui/app.js still ends with the init() call this test cuts away');
  vm.runInContext(src.slice(0, boot), context);
  const run = code => vm.runInContext(code, context);
  return { run, footer };
}

// The sentences ingest writes (crates/ingest/src/lib.rs, crates/cli/src/builder.rs).
const PROJECTED = "Object-code coverage is PROJECTED from source-level coverage through the DWARF line table; it is not measured. Treat green instructions as 'implied by source coverage', not as executed.";
const INSTRUMENTED = 'Source coverage comes from an instrumented build; the object code shown is the non-instrumented build of the same sources.';
const BASELINE = 'BASELINE CHANGED (4.4.2.c): compiler changed from clang 17 to clang 18. Coverage and source-to-object analyses done against the baseline may no longer be valid.';
const EXPLORATORY = 'EXPLORATORY BUILD: variant `O2` was compiled by `butterfly build`, not by the configuration-controlled build. Use it to understand the optimiser; it is not certification evidence.';
const TRACE = { format: 'qemu-exec', source: 'build/trace.log', caveats: [] };

// The pure function under test, called with plain JSON so nothing from one
// case can leak into the next.
function parts(mode, measured, caveats) {
  const { run } = setup();
  return JSON.parse(run(`JSON.stringify(caveatParts(${JSON.stringify(mode)}, ${JSON.stringify(measured)}, ${JSON.stringify(caveats)}))`));
}
const overlay = ps => ps.filter(p => !p.serious);
const serious = ps => ps.filter(p => p.serious);

test('Measured says measured, names the trace, and keeps the limits that still apply', () => {
  const ps = parts('measured', TRACE, [PROJECTED, INSTRUMENTED]);
  assert.equal(ps.length, 1);
  const [p] = ps;
  assert.equal(p.lead, 'Measured, not projected');
  assert.doesNotMatch(p.lead, /^Projected/);
  assert.match(p.text, /MEASURED from the imported execution trace \(qemu-exec: build\/trace\.log\)/);
  assert.doesNotMatch(p.text, /is PROJECTED/, 'the projection caveat is not what this overlay shows');
  assert.match(p.text, /execution counts from an emulator trace are an upper bound/);
  assert.match(p.text, /Whether an instruction executed is exact/);
  assert.match(p.text, /An emulator is not the target/);
  assert.match(p.text, /not the source-to-object correspondence review \(A-7 obj 9\), which stays a human review/);
});

test('a trace that states its own limits has them in the footer, not "exact"', () => {
  // crates/ingest/src/trace.rs writes these when it cannot account for a
  // block on the trace's own terms. `Measured.caveats` carries them into the
  // evidence, and the footer ignored them while claiming "Whether an
  // instruction executed is exact" (#212 S16).
  const RECONSTRUCTED = 'trace has exec records but no in_asm blocks: block extents were reconstructed from the disassembly';
  const NO_LISTING = '3 executed block(s) had no in_asm listing; their extent was reconstructed from the disassembly';
  const [p] = parts('measured', {...TRACE, caveats: [RECONSTRUCTED, NO_LISTING]}, [PROJECTED]);
  assert.equal(p.lead, 'Measured, not projected');
  assert.ok(p.text.includes(RECONSTRUCTED), 'the trace\'s own caveat is in the footer');
  assert.ok(p.text.includes(NO_LISTING));
  assert.doesNotMatch(p.text, /Whether an instruction executed is exact/,
    'a trace that reconstructed a block extent has not shown that exactly those instructions executed');
  // the limits that hold either way are still stated
  assert.match(p.text, /upper bound/);
  assert.match(p.text, /An emulator is not the target/);
  assert.match(p.text, /A-7 obj 9/);
  // ...and a trace with nothing to add keeps the plain wording
  assert.match(parts('measured', {...TRACE, caveats: []}, [PROJECTED])[0].text, /Whether an instruction executed is exact/);
  assert.match(parts('measured', {format: 'qemu-exec', source: 'x.log'}, [PROJECTED])[0].text, /Whether an instruction executed is exact/);
});

test('Projected keeps today\'s text, and says so when a measured overlay is available', () => {
  const without = parts('projected', null, [PROJECTED, INSTRUMENTED]);
  assert.deepEqual(without, [{ lead: 'Projected, not measured', text: PROJECTED }]);

  const [p] = parts('projected', TRACE, [PROJECTED, INSTRUMENTED]);
  assert.equal(p.lead, 'Projected, not measured');
  assert.ok(p.text.startsWith(PROJECTED));
  assert.match(p.text, /a Measured overlay is available\.$/);
});

test('Projected states the projection even when the evidence does not carry the sentence', () => {
  // hand-written or older evidence: the first caveat is about something else,
  // and must not be presented as the account of the overlay
  const [p] = parts('projected', null, ['hand-written fixture; not produced by ingest']);
  assert.equal(p.text, PROJECTED);
});

test('No coverage says no overlay is applied, and whether a trace exists', () => {
  const [a] = parts('plain', null, [PROJECTED]);
  assert.equal(a.lead, 'No coverage overlay');
  assert.match(a.text, /^No coverage overlay is applied/);
  assert.match(a.text, /no execution trace was imported/);
  assert.doesNotMatch(a.lead + a.text, /is PROJECTED|is MEASURED/);

  const [b] = parts('plain', TRACE, [PROJECTED]);
  assert.match(b.text, /^No coverage overlay is applied/);
  assert.match(b.text, /Measured colours it from the imported execution trace/);
});

test('the footer describes the overlay instrStatus() actually applied', () => {
  // instrStatus(): 'plain' is the only mode with no overlay; 'measured'
  // colours from the trace only when one was imported; ANYTHING ELSE falls
  // through to the projected branch. `S.mode` comes from localStorage, so
  // "anything else" is reachable -- a value left there by an older build.
  // The catch-all used to be the no-overlay branch, which printed "No
  // coverage overlay is applied" under projected colours: #209 inverted
  // (#212 S17).
  assert.equal(parts('projcted', null, [PROJECTED])[0].lead, 'Projected, not measured');
  assert.equal(parts('', TRACE, [PROJECTED])[0].lead, 'Projected, not measured');
  assert.equal(parts(undefined, null, [PROJECTED])[0].lead, 'Projected, not measured');
  // 'measured' with no trace is not measured either, and it is not "no
  // overlay": measuredMode() is false, so the colours are the projection.
  // (loadVariant() coerces S.mode to 'plain' in that case, so the page does
  // not reach this; the footer must not contradict the colouring if it ever
  // does.)
  assert.equal(parts('measured', null, [PROJECTED])[0].lead, 'Projected, not measured');
  // and 'plain' is still the one mode that says nothing is applied
  assert.equal(parts('plain', TRACE, [PROJECTED])[0].lead, 'No coverage overlay');
});

test('the sentence ingest writes is the sentence the footer falls back to', () => {
  // PROJECTED_CAVEAT is written out in four places and nothing held them
  // equal: ui/app.js, this file, ui/browser/checks/footer.cjs and the one
  // that matters, crates/ingest/src/lib.rs, which is what a real evidence
  // file carries. A drift there would leave the footer quietly printing a
  // different sentence from the provenance panel beside it.
  const read = f => fs.readFileSync(`${__dirname}/${f}`, 'utf8');
  const ingest = read('../crates/ingest/src/lib.rs');
  assert.ok(ingest.includes(PROJECTED), 'crates/ingest/src/lib.rs writes this exact sentence');
  assert.ok(read('app.js').includes(PROJECTED), 'ui/app.js falls back to this exact sentence');
  assert.ok(read('browser/checks/footer.cjs').includes(PROJECTED), 'the browser check asserts this exact sentence');
});

test('a changed baseline or an exploratory build leads the footer in every mode', () => {
  for (const mode of ['plain', 'projected', 'measured']) {
    for (const trace of [null, TRACE]) {
      for (const [caveat, lead] of [[BASELINE, 'Baseline changed'], [EXPLORATORY, 'Exploratory build']]) {
        const ps = parts(mode, trace, [caveat, PROJECTED, INSTRUMENTED]);
        assert.deepEqual(serious(ps), [{ lead, text: caveat, serious: true }], `${mode}: ${lead}`);
        assert.equal(ps[0].serious, true, 'the serious caveat comes first');
        // ...and the overlay statement is the same one as without it
        assert.deepEqual(overlay(ps), parts(mode, trace, [PROJECTED, INSTRUMENTED]), `${mode}: overlay statement unchanged`);
      }
    }
  }
  // both at once: both are shown
  assert.deepEqual(serious(parts('projected', null, [BASELINE, EXPLORATORY, PROJECTED])).map(p => p.lead), ['Baseline changed', 'Exploratory build']);
  // nothing else is promoted
  assert.deepEqual(serious(parts('projected', null, [PROJECTED, INSTRUMENTED, 'gcov: condition data absent'])), []);
});

test('the rendered footer follows the overlay buttons, and escapes what it prints', () => {
  const { run, footer } = setup();
  run(`S.ev = ${JSON.stringify({ measured: TRACE, provenance: { caveats: ['BASELINE CHANGED (4.4.2.c): <b>x</b>', PROJECTED] } })};`);
  for (const [mode, lead] of [['measured', 'Measured, not projected'], ['projected', 'Projected, not measured'], ['plain', 'No coverage overlay']]) {
    run(`setMode('${mode}')`);   // what each overlay button calls
    assert.match(footer.innerHTML, new RegExp(`<span class="w">${lead}</span>`), mode);
    assert.match(footer.innerHTML, /<span class="w bad">Baseline changed<\/span>/, mode);
    assert.doesNotMatch(footer.innerHTML, /<b>x<\/b>/);
    assert.equal(run('S.mode'), mode);
  }
  assert.doesNotMatch(footer.innerHTML, /Projected, not measured/, 'the last mode set was No coverage');
});

// The defect as it was reported, through nothing but what the page already
// had: `renderCaveat()` and `S.mode`. Against the old footer this fails on
// the text itself, not on a missing function.
test('renderCaveat() does not say "Projected, not measured" while Measured is selected', () => {
  const { run, footer } = setup();
  run(`S.ev = ${JSON.stringify({ measured: TRACE, provenance: { caveats: [PROJECTED, INSTRUMENTED] } })};`);
  run(`S.mode = 'measured'; renderCaveat();`);
  assert.doesNotMatch(footer.innerHTML, /Projected, not measured|is PROJECTED/);
  assert.match(footer.innerHTML, /Measured, not projected/);
  run(`S.mode = 'plain'; renderCaveat();`);
  assert.doesNotMatch(footer.innerHTML, /Projected, not measured|is PROJECTED/);
  run(`S.mode = 'projected'; renderCaveat();`);
  assert.match(footer.innerHTML, /Projected, not measured/);
});
