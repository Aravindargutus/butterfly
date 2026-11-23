// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Variant switching under concurrency (#69, BF-QA-01).
//
// Two cases, both about the same hazard: the label on screen and the
// evidence behind it must never disagree, because the active variant is the
// target a review POST is saved against. Attributing a reviewer's
// justification to a build they were not looking at is the worst outcome
// this viewer can produce, and it can only be provoked with real request
// ordering -- which is why this is a browser check and not a unit test.
//
// Needs the exploratory multi-variant fixture,
// `butterfly build examples/c/butterfly-build.toml`. Reported as a skip
// when absent rather than passing quietly.

const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const {ROOT, scratch, serve, browser, waitFor} = require('../harness.cjs');

const VARIANTS = path.join(ROOT, 'examples/c/variants');

async function run({out}) {
  if (!fs.existsSync(VARIANTS)) {
    return [{
      name: 'fixture',
      pass: false,
      skip: 'examples/c/variants is not built (target/debug/butterfly build examples/c/butterfly-build.toml)',
    }];
  }
  const names = fs.readdirSync(VARIANTS)
    .filter(f => /^evidence-.*\.json$/.test(f))
    .map(f => f.replace(/^evidence-|\.json$/g, ''))
    .sort();
  if (names.length < 2) {
    return [{name: 'fixture', pass: false, skip: `examples/c/variants holds ${names.length} variant(s); this needs two`}];
  }
  const [first, second] = names;
  const instructions = n => JSON.parse(fs.readFileSync(path.join(VARIANTS, `evidence-${n}.json`), 'utf8')).summary.instructionsTotal;

  const dir = scratch('variants');
  const results = [];
  let srv, br, page;
  const check = async (name, fn) => {
    try {
      const detail = await fn();
      results.push({name, pass: true, ...(detail ? {detail} : {})});
    } catch (e) {
      const shot = path.join(out, `variants-${name}.png`);
      await page?.screenshot({path: shot}).catch(() => {});
      results.push({name, pass: false, error: e.message, screenshot: shot});
    }
  };

  try {
    const all = names.map(n => path.join(VARIANTS, `evidence-${n}.json`));
    srv = await serve(all, {readOnly: true, dir, out});

    br = await browser();
    page = await br.newPage({viewport: {width: 1280, height: 900}});

    const select = async name => {
      await page.locator('#variants').getByRole('button', {name, exact: true}).click();
    };
    // `S` is a top-level `const` in app.js, so it lives in the global
    // lexical environment: reachable by name here, but not as `window.S`.
    const shown = () => page.evaluate(() => ({
      label: document.querySelector('#variants button.on')?.textContent,
      instructions: S.ev?.summary?.instructionsTotal,
      variant: S.variant,
    }));
    const settle = async name => {
      await waitFor(page, t => (typeof S === 'undefined' ? false : S.ev?.summary?.instructionsTotal === t),
        instructions(name), {what: `variant ${name} to load`});
    };

    // BF-QA-01 case 1: an *older* load completing after a newer selection
    // must be dropped whole.
    //
    // The previous version of this held one request and sampled it
    // mid-flight, which never creates the situation the guard exists for:
    // with a single outstanding load there is nothing newer for it to lose
    // to, and removing app.js's `if (gen !== variantGen) return false;` left
    // the test green (review of #69). Two requests have to be in flight, and
    // the older one has to finish last.
    const outOfOrder = async (from, held, winner) => {
      await page.goto(`${srv.url}/?fresh=1#/fn/0/flow`);
      await page.locator('#variants button').first().waitFor();
      await select(from);
      await settle(from);

      // `held` is answered slowly; `winner` is selected afterwards and
      // answers immediately, so `held` lands last and out of order.
      let release;
      const holding = new Promise(r => { release = r; });
      await page.route(`**/api/evidence?v=${held}`, async r => { await holding; await r.continue(); });
      await page.route(`**/api/resolutions?v=${held}`, async r => { await holding; await r.continue(); });

      await select(held);
      await new Promise(r => setTimeout(r, 150));
      await select(winner);
      await settle(winner);

      release();
      // Give the superseded response time to arrive and, if unguarded, commit.
      await new Promise(r => setTimeout(r, 800));
      await page.unroute(`**/api/evidence?v=${held}`);
      await page.unroute(`**/api/resolutions?v=${held}`);

      const st = await shown();
      assert.equal(st.variant, winner, `the superseded ${held} load committed over ${winner}`);
      assert.equal(st.label, winner, `the label moved to ${st.label} after ${winner} was selected`);
      assert.equal(st.instructions, instructions(winner), `${st.label} is showing ${st.instructions} instructions`);
      return st;
    };

    await check('a-superseded-load-never-commits-over-a-newer-one', async () => outOfOrder(first, second, first));

    // ...and the same with three distinct selections, so the winner is not
    // simply the variant already loaded.
    await check('the-newest-selection-wins-among-three', async () => {
      if (names.length < 3) {
        return {note: `only ${names.length} variants; A->B->A covered above`};
      }
      return outOfOrder(first, second, names[2]);
    });

    // BF-QA-01 case 2: a switch whose evidence fails to load must leave the
    // active variant -- the target a review POST is saved against -- on the
    // build still displayed.
    //
    // Checking S.variant is not enough: #69 asks for a real review save
    // afterwards, because the failure that matters is a POST routed to a
    // stale variant while the displayed state looks right. That needs a
    // writable server with a sidecar per variant, so the two can be told
    // apart on disk.
    await check('a-failed-switch-saves-the-review-against-the-shown-build', async () => {
      const wdir = scratch('variants-write');
      const wsrv = await serve(all, {dir: wdir, out, resolutions: path.join(wdir, 'resolutions.json')});
      const wpage = await br.newPage({viewport: {width: 1280, height: 900}});
      try {
        await wpage.goto(`${wsrv.url}/?fresh=1#/fn/0/flow`);
        await wpage.locator('#variants button').first().waitFor();
        await wpage.locator('#variants').getByRole('button', {name: first, exact: true}).click();
        await waitFor(wpage, t => (typeof S === 'undefined' ? false : S.ev?.summary?.instructionsTotal === t),
          instructions(first), {what: `variant ${first} to load`});

        await wpage.route(`**/api/evidence?v=${second}`, r => r.fulfill({status: 503, body: 'QA simulated unavailable'}));
        let alert = '';
        wpage.once('dialog', async d => { alert = d.message(); await d.accept(); });
        await wpage.locator('#variants').getByRole('button', {name: second, exact: true}).click();
        await new Promise(r => setTimeout(r, 600));
        await wpage.unroute(`**/api/evidence?v=${second}`);

        const st = await wpage.evaluate(() => ({variant: S.variant, label: document.querySelector('#variants button.on')?.textContent}));
        assert.equal(st.variant, first, 'the write target moved off the displayed build after a failed switch');
        assert.equal(st.label, first, 'the selected label moved off the displayed build');

        // Now actually save one, through the UI, and see where it lands.
        await wpage.getByRole('button', {name: 'source→object: not reviewed', exact: true}).click();
        await wpage.locator('#modal textarea').fill('QA TEST ONLY: proving which variant a review is attributed to.');
        await wpage.getByPlaceholder('reviewer', {exact: true}).fill('QA variant fixture');
        await wpage.getByRole('button', {name: 'Save', exact: true}).click();
        await wpage.locator('#modal.hidden').waitFor({state: 'attached'});

        const shownEntries = await (await fetch(`${wsrv.url}/api/resolutions?v=${encodeURIComponent(first)}`)).json();
        const failedEntries = await (await fetch(`${wsrv.url}/api/resolutions?v=${encodeURIComponent(second)}`)).json();
        assert.equal((shownEntries.entries || []).length, 1, 'the displayed variant must hold exactly the one review');
        assert.equal((shownEntries.entries || [])[0].author, 'QA variant fixture');
        assert.equal((failedEntries.entries || []).length, 0, 'the variant that failed to load must hold no review');

        // and on disk, which is what a later run would read back
        const files = fs.readdirSync(wdir).filter(f => f.startsWith('resolutions'));
        const withReview = files.filter(f => fs.readFileSync(path.join(wdir, f), 'utf8').includes('QA variant fixture'));
        assert.equal(withReview.length, 1, `exactly one sidecar must carry the review, got ${JSON.stringify(files)}`);
        return {alert, sidecar: withReview[0]};
      } finally {
        await wpage.close().catch(() => {});
        wsrv.child.kill();
      }
    });

  } finally {
    await br?.close();
    srv?.child.kill();
  }
  return results;
}

module.exports = {run};
