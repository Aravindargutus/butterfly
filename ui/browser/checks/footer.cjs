// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The footer and the overlay never disagree (#209).
//
// The footer is the page's standing statement of what its colours mean. It
// used to print the first provenance caveat whatever overlay was on, so with
// a trace imported and Measured selected it read "Projected, not measured"
// under blocks coloured from the trace. `ui/footer.test.cjs` pins the text
// selection as a pure function; what only a rendered page can show is that a
// click on an overlay button -- in the function view and in the file view,
// which have separate handlers -- actually reaches the footer, and that what
// the footer then says matches the words beside the blocks above it.
//
// # Why the evidence here is written by hand
//
// A measured overlay needs an execution trace, and a trace needs an emulator
// the hosted runner does not have. The viewer's input is the evidence
// document, so this writes one: the `vars` fixture, plus a `measured` block
// and per-instruction counts in which the trace never entered B1 although
// the projection calls it covered -- the disagreement the overlay exists to
// show.

const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const {scratch, serve, browser, waitFor} = require('../harness.cjs');
const {evidence, ATTRIBUTED} = require('./vars.cjs');

const PROJECTED = "Object-code coverage is PROJECTED from source-level coverage through the DWARF line table; it is not measured. Treat green instructions as 'implied by source coverage', not as executed.";
// One of the sentences crates/ingest/src/trace.rs writes into
// `Measured.caveats` when the trace cannot account for a block itself.
const TRACE_CAVEAT = '2 executed block(s) had no in_asm listing; their extent was reconstructed from the disassembly';

/// The `vars` evidence, with or without a trace imported into it.
function fixture({traced}) {
  const ev = evidence({arch: 'aarch64', objectFormat: 'elf', variables: ATTRIBUTED});
  // what ingest writes first, so Projected is checked against the real sentence
  ev.provenance.caveats = [PROJECTED, 'hand-written fixture for the footer browser checks (#209); not produced by ingest'];
  if (!traced) return ev;
  const o = ev.functions[0].object;
  // B0 and B2 ran once; the trace never entered B1.
  const ran = {0: 1, 1: 0, 2: 1};
  for (const b of o.blocks) for (const i of b.instrs) i.measured = ran[b.id];
  o.instrMeasuredCovered = 4;
  o.disagreements = o.blocks[1].instrs.map(i =>
    ({addr: i.addr, line: i.line, kind: 'projectedNotExecuted', projected: 'covered', measured: 0}));
  ev.summary.instructionsMeasuredCovered = 4;
  ev.summary.disagreements = 2;
  ev.measured = {
    source: 'fixture-trace.log', format: 'qemu-exec', blockExecutions: 2,
    addressesInTrace: 4, addressesMatched: 4, addressesOutsideSymbols: 0,
    functionsExecuted: 1, instructionsMeasuredCovered: 4, disagreements: 2,
    // A caveat of the shape crates/ingest/src/trace.rs really writes, so the
    // check can hold the footer to reporting it: the trace could not account
    // for a block's extent on its own terms, which is exactly the case the
    // footer must not describe as exact (#212 S16).
    caveats: [TRACE_CAVEAT],
    transitions: 1, transitionsMatched: 1,
  };
  return ev;
}

// What each overlay's footer must say, and what it must not.
const LEAD = {plain: 'No coverage overlay', projected: 'Projected, not measured', measured: 'Measured, not projected'};
const BUTTON = {plain: 'No coverage', projected: 'Projected', measured: 'Measured'};

async function run({out}) {
  const dir = scratch('footer');
  const write = (name, doc) => {
    const p = path.join(dir, `${name}.json`);
    fs.writeFileSync(p, JSON.stringify(doc, null, 2));
    return p;
  };
  const traced = write('evidence-traced', fixture({traced: true}));
  const untraced = write('evidence-untraced', fixture({traced: false}));

  const results = [];
  let page;
  const check = async (name, fn) => {
    try {
      const detail = await fn();
      results.push({name, pass: true, ...(detail ? {detail} : {})});
    } catch (e) {
      const shot = path.join(out, `footer-${name}.png`);
      await page?.screenshot({path: shot, fullPage: true}).catch(() => {});
      results.push({name, pass: false, error: e.message, screenshot: shot});
    }
  };

  // The footer as the page shows it, the overlay button that is lit, and the
  // words beside each block.
  const state = () => page.evaluate(names => ({
    leads: [...document.querySelectorAll('#caveat .w')].map(e => e.textContent),
    text: document.querySelector('#caveat').textContent,
    // other segmented controls light buttons too (Variable marks); only the
    // overlay's three are of interest
    on: [...document.querySelectorAll('.seg button.on')].map(b => b.textContent).filter(t => names.includes(t)),
    captions: [...document.querySelectorAll('table.obj tr.blk td > span:not(.infer)')].map(e => e.textContent),
  }), Object.values(BUTTON));
  const choose = async mode => {
    await page.getByRole('button', {name: BUTTON[mode], exact: true}).click();
    // the footer is rewritten by the same click; wait for it rather than
    // assume it, since that is the behaviour under test
    await waitFor(page, want => [...document.querySelectorAll('#caveat .w')].some(e => e.textContent === want),
      LEAD[mode], {what: `the footer to read "${LEAD[mode]}"`});
    return state();
  };
  // The contradiction itself, in both directions. The second direction needs
  // the block captions to follow the overlay, which is #164's change to
  // `renderObject` (`blockStatusLabel`); until that is on the same branch the
  // captions say "projected ..." in every mode, and only the first direction
  // can be held against them.
  const captionsFollow = () => page.evaluate(() => typeof blockStatusLabel === 'function');
  const agree = async (mode, s) => {
    assert.deepEqual(s.leads, [LEAD[mode]], `${mode}: the footer names this overlay and no other`);
    assert.deepEqual(s.on, [BUTTON[mode]], `${mode}: and it is the overlay whose button is lit`);
    const saysMeasured = s.captions.some(c => /^measured/.test(c));
    const saysProjected = s.captions.some(c => /^projected/.test(c));
    if (saysMeasured) assert.equal(mode, 'measured', `a block is captioned as measured under a footer reading "${s.leads}"`);
    if (mode !== 'measured') assert.doesNotMatch(s.text, /is MEASURED/);
    if (mode !== 'projected') assert.doesNotMatch(s.text, /is PROJECTED/);
    // (the file view has no blocks, and so no captions to hold it against)
    if (mode === 'measured' && s.captions.length && await captionsFollow()) {
      assert.ok(saysMeasured && !saysProjected, `Measured: captions must be the trace's, got ${JSON.stringify(s.captions)}`);
    }
  };

  let srv, br;
  try {
    br = await browser();

    srv = await serve(traced, {readOnly: true, dir, out});
    page = await br.newPage({viewport: {width: 1460, height: 900}});
    const pageErrors = [];
    page.on('pageerror', e => pageErrors.push(e.message));
    await page.goto(`${srv.url}/#/fn/0/object`);
    await page.locator('table.obj tr.blk').first().waitFor();

    await check('footer-follows-the-overlay-in-the-function-view', async () => {
      const seen = {};
      // Measured first: it is the mode the defect was reported in, and coming
      // to it from the default proves the click, not the initial render.
      for (const mode of ['measured', 'projected', 'plain', 'measured']) {
        const s = await choose(mode);
        await agree(mode, s);
        assert.equal(s.captions.length, 3, 'the Object tab is showing its three blocks');
        seen[mode] = s;
        await page.screenshot({path: path.join(out, `footer-${mode}.png`)});
      }
      assert.match(seen.measured.text, /MEASURED from the imported execution trace \(qemu-exec: fixture-trace\.log\)/);
      assert.match(seen.measured.text, /upper bound/);
      assert.match(seen.measured.text, /An emulator is not the target/);
      assert.match(seen.measured.text, /A-7 obj 9/);
      // the trace's own caveat reaches the footer, and the claim it
      // contradicts is not made over it (#212 S16). This fixture has carried
      // a caveat since it was written and nothing asserted it arrived.
      assert.ok(seen.measured.text.includes(TRACE_CAVEAT), `the trace's own caveat must be in the footer, got: ${seen.measured.text}`);
      assert.doesNotMatch(seen.measured.text, /Whether an instruction executed is exact/);
      assert.ok(seen.projected.text.includes(PROJECTED), 'Projected keeps the sentence it always had');
      assert.match(seen.projected.text, /a Measured overlay is available/);
      assert.match(seen.plain.text, /No coverage overlay is applied/);
      // ...and none of it is clipped: the footer grows to hold its statement
      const clipped = await page.evaluate(() => { const f = document.querySelector('#caveat'); return f.scrollHeight > f.clientHeight + 1 || f.scrollWidth > f.clientWidth + 1; });
      assert.equal(clipped, false, 'the Measured statement must be readable in full');
      return Object.fromEntries(Object.entries(seen).map(([m, s]) => [m, s.leads]));
    });

    await check('footer-follows-the-overlay-in-the-file-view', async () => {
      await page.goto(`${srv.url}/#/file/0`);
      await page.getByRole('button', {name: 'Measured', exact: true}).waitFor();
      for (const mode of ['projected', 'measured', 'plain']) await agree(mode, await choose(mode));
    });

    await check('footer-survives-a-reload-in-the-remembered-overlay', async () => {
      await choose('measured');
      await page.reload();
      await waitFor(page, () => document.querySelector('#caveat .w')?.textContent, undefined, {what: 'the footer after a reload'});
      const s = await state();
      assert.deepEqual(s.leads, [LEAD.measured], 'the overlay is remembered, and the footer opens on it');
    });

    // #217: the statement was wrapped in `<span class="t">`, the syntax-token
    // class, so the sentence that says what the page's colours mean was
    // itself printed in the token blue -- in both themes, in 12 committed
    // showcase screenshots, with nothing red. A colour is only checkable on a
    // rendered page, which is why this lives here and not in
    // ui/footer.test.cjs.
    //
    // The assertion is an EQUALITY against the footer's own colour, not an
    // inequality against today's token blue: it holds whatever the palette
    // becomes, and it is what "the statement belongs to the footer" means.
    //
    // The statement is found STRUCTURALLY -- the span in #caveat that is not
    // a lead -- and not by the class this fix gave it. Keyed on `.stmt`, the
    // check would have failed the old markup by not finding an element,
    // timing out with a message about a missing selector; the defect is a
    // colour, so the check has to reach the element that has the wrong one
    // and say so. The token class is probed beside it so the message can name
    // the collision.
    await check('the-footer-statement-is-the-footers-colour-not-a-token-colour', async () => {
      const seen = {};
      for (const theme of ['dark', 'light']) {
        await page.evaluate(t => localStorage.setItem('sl-theme', t), theme);
        await page.reload();
        // Both halves, in one condition: `data-theme` alone is already set by
        // index.html before init() runs, so waiting on it returned before the
        // footer had rendered and the check failed for the wrong reason.
        await waitFor(page, t => document.documentElement.getAttribute('data-theme') === t
          && [...document.querySelectorAll('#caveat > span')].some(e => !e.classList.contains('w')),
        theme, {what: `the footer rendered in the ${theme} theme`});
        const c = await page.evaluate(() => {
          const foot = document.querySelector('#caveat');
          const stmt = [...foot.querySelectorAll(':scope > span')].find(e => !e.classList.contains('w'));
          if (!stmt) return {none: true};
          // What `class="t"` resolves to at this spot, measured rather than
          // assumed, so the message can say whether this IS the token colour.
          const probe = document.createElement('span');
          probe.className = 't';
          foot.append(probe);
          const token = getComputedStyle(probe).color;
          probe.remove();
          return {
            footer: getComputedStyle(foot).color,
            stmt: getComputedStyle(stmt).color,
            cls: stmt.className,
            token,
            theme: document.documentElement.getAttribute('data-theme'),
          };
        });
        assert.equal(c.none, undefined, `${theme}: the footer renders a statement span beside its lead`);
        assert.equal(c.theme, theme, `the ${theme} theme is the one being measured`);
        assert.equal(c.stmt, c.footer,
          `${theme}: the footer statement (span.${c.cls}) must be the footer's own colour (${c.footer}), got ${c.stmt}` +
          (c.stmt === c.token ? ` -- the syntax-token colour, which is the #217 collision` : ''));
        assert.ok(!/\bt\b/.test(c.cls),
          `${theme}: the footer statement must not carry the syntax-token class \`.t\` (#217); it is span.${c.cls}`);
        seen[theme] = c.stmt;
      }
      // A footer statement that came out the same colour in both themes would
      // mean it stopped following `#caveat`, which is the defect from the
      // other side.
      assert.notEqual(seen.dark, seen.light, 'the statement follows the theme, as the footer does');
      await page.evaluate(() => localStorage.removeItem('sl-theme'));
      await page.reload();
      await waitFor(page, () => !!document.querySelector('#caveat .stmt'), undefined,
        {what: 'the footer back in the default theme'});
      return seen;
    });

    await check('no-page-errors-with-a-trace', async () => assert.deepEqual(pageErrors, []));
    await page.close();
    srv.child.kill();

    // No trace: there is no Measured button, the footer never offers one, and
    // a remembered 'measured' from another evidence file is not claimed.
    srv = await serve(untraced, {readOnly: true, dir, out});
    page = await br.newPage({viewport: {width: 1460, height: 900}});
    await check('without-a-trace-nothing-is-called-measured', async () => {
      await page.addInitScript(() => localStorage.setItem('sl-mode', 'measured'));
      await page.goto(`${srv.url}/#/fn/0/object`);
      await page.locator('table.obj tr.blk').first().waitFor();
      let s = await state();
      assert.deepEqual(s.leads, [LEAD.plain], 'a remembered Measured with no trace falls back, as the colouring does');
      assert.match(s.text, /no execution trace was imported/);
      assert.equal(await page.getByRole('button', {name: 'Measured', exact: true}).count(), 0);
      s = await choose('projected');
      assert.deepEqual(s.leads, [LEAD.projected]);
      assert.doesNotMatch(s.text, /Measured overlay is available/);
    });
  } finally {
    await page?.close().catch(() => {});
    await br?.close();
    srv?.child.kill();
  }
  return results;
}

module.exports = {run, fixture};
