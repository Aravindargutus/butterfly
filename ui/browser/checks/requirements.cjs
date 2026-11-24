// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Requirements-pane regressions in a real browser (#69, BF-QA-02..03).
//
// These cover what a DOM test in a VM cannot: event ordering, focus and
// hover behaviour, and layout overflow. Asserted against examples/c-per-test
// -- the only in-repo example carrying an llr.yaml -- and NOT against the
// fixture the original QA scripts used, which was never committed. See the
// note in README.md: these are equivalent in intent, not the same assertions.

const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const {scratch, ingest, serve, browser} = require('../harness.cjs');

async function run({out}) {
  const dir = scratch('reqs');
  const got = ingest('c-per-test', dir);
  if (got.skip) return [{name: 'fixture', pass: false, skip: got.skip}];

  const results = [];
  let srv, br, page;
  const check = async (name, fn) => {
    try {
      const detail = await fn();
      results.push({name, pass: true, ...(detail ? {detail} : {})});
    } catch (e) {
      const shot = path.join(out, `requirements-${name}.png`);
      await page?.screenshot({path: shot}).catch(() => {});
      results.push({name, pass: false, error: e.message, screenshot: shot});
    }
  };

  try {
    srv = await serve(got.evidence, {readOnly: true, dir, out});
    br = await browser();
    page = await br.newPage({viewport: {width: 1280, height: 900}});
    const pageErrors = [];
    page.on('pageerror', e => pageErrors.push(e.message));
    // A distinct URL every time, so each open is a real page load. The
    // viewer caches requirement text by ID for the session (`REQ` in
    // app.js), so reusing one URL leaves that cache in place and a routed
    // response never reaches the pane -- the mock silently does nothing.
    let visit = 0;
    const open = async hash => {
      await page.goto(`${srv.url}/?qa=${++visit}${hash}`);
      await page.locator('#content').waitFor();
      await page.waitForFunction(() => document.querySelector('#content')?.textContent.length > 0);
    };
    const evidence = JSON.parse(fs.readFileSync(got.evidence, 'utf8'));
    const tagged = evidence.functions.find(f => (f.requirements || []).length);

    await check('overview-renders', async () => {
      await open('#/overview');
      // Presence, not position: the overview opens with a Gates box when
      // thresholds apply, so asserting on the first heading makes this
      // test depend on gate configuration rather than on the pane rendering.
      const headings = await page.locator('#content h2').allTextContents();
      assert.ok(
        headings.some(h => /Annex A table A-7/.test(h)),
        `the A-7 table heading must be present, got ${JSON.stringify(headings)}`,
      );
      return {headings: headings.length};
    });

    // Every function against every tab: the check that actually catches a
    // view that throws, which no unit test covers.
    await check('every-function-tab-renders', async () => {
      let views = 0;
      for (const f of evidence.functions) {
        for (const t of ['flow', 'calls', 'object', 'gaps', 'mcdc', 'stack', 'data', 'variables', 'changes', 'requirements']) {
          await page.evaluate(({id, t}) => { location.hash = `#/fn/${id}/${t}`; }, {id: f.id, t});
          await page.waitForFunction(
            name => document.querySelector('#content h1')?.textContent === name,
            f.name,
          );
          const text = await page.locator('#content').textContent();
          assert.ok(!text.includes('View failed to render'), `${f.name}/${t} failed to render`);
          views++;
        }
      }
      assert.deepEqual(pageErrors, [], 'the viewer must raise no page errors');
      return {views};
    });

    await check('requirement-text-loads-from-the-sidecar', async () => {
      assert.ok(tagged, 'the fixture must tag at least one function with a requirement');
      await open(`#/fn/${tagged.id}/requirements`);
      await page.locator('.requirements-table[aria-busy="false"]').waitFor();
      assert.deepEqual(
        await page.locator('.requirements-table thead th').allTextContents(),
        ['Requirement ID', 'Requirement text'],
      );
      // LLR-LIM-001's statement in examples/c-per-test/llr.yaml
      assert.match(await page.locator('.requirement-text').first().textContent(), /altitude/i);
    });

    // BF-QA-02: Escape dismisses the panel. Keyboard dismissal is exactly
    // the kind of thing that regresses silently.
    await check('escape-closes-the-requirement-panel', async () => {
      await open(`#/fn/${tagged.id}/requirements`);
      await page.locator('.requirements-table[aria-busy="false"]').waitFor();
      await page.locator('.requirements-table .req').first().press('Enter');
      await page.locator('.rp-more').waitFor();
      await page.keyboard.press('Escape');
      await page.waitForFunction(() => !document.querySelector('#req-panel')?.classList.contains('on'), null, {timeout: 4000});
      assert.equal(await page.locator('#req-panel.on').count(), 0, 'Escape must close the panel');
    });

    // BF-QA-03: a clicked requirement stays pinned until dismissed, rather
    // than vanishing when the pointer moves away.
    await check('a-clicked-requirement-stays-pinned', async () => {
      await open(`#/fn/${tagged.id}/requirements`);
      await page.locator('.requirements-table[aria-busy="false"]').waitFor();
      await page.locator('.requirements-table .req').first().click();
      await page.locator('#req-panel.on').waitFor();
      await page.mouse.move(1100, 850);
      await new Promise(r => setTimeout(r, 300));
      assert.equal(await page.locator('#req-panel.on').count(), 1, 'a clicked requirement must stay open');
    });

    // A transient failure must not poison the pane for the rest of the session.
    await check('a-transient-requirements-error-recovers', async () => {
      await page.route('**/api/requirements?*', r => r.fulfill({status: 503, body: 'temporarily unavailable'}));
      await open(`#/fn/${tagged.id}/requirements`);
      await page.locator('.requirements-table[aria-busy="false"]').waitFor();
      assert.match(await page.locator('.requirement-text').first().textContent(), /unavailable/i);
      await page.unroute('**/api/requirements?*');
      await open(`#/fn/${tagged.id}/requirements`);
      await page.locator('.requirements-table[aria-busy="false"]').waitFor();
      assert.match(await page.locator('.requirement-text').first().textContent(), /altitude/i, 'the pane must recover');
    });

    await check('search-narrows-and-navigates', async () => {
      await open(`#/fn/${tagged.id}/flow`);
      const name = evidence.functions[0].name;
      await page.locator('#search').fill(name);
      await page.waitForFunction(n => {
        const rows = [...document.querySelectorAll('.fn-row')];
        return rows.length > 0 && rows.every(r => r.textContent.includes(n));
      }, name);
      await page.locator('#search').fill('no-such-function-anywhere');
      await page.waitForFunction(() => document.querySelectorAll('.fn-row').length === 0);
      await page.locator('#search').fill('');
    });

    // Layout overflow across the widths the viewer claims to support.
    await check('no-overflow-at-supported-widths', async () => {
      const seen = [];
      for (const width of [1024, 1280, 1600, 1920]) {
        await page.setViewportSize({width, height: 900});
        await open(`#/fn/${tagged.id}/requirements`);
        await page.locator('.requirements-table[aria-busy="false"]').waitFor();
        const r = await page.evaluate(() => {
          const pane = document.querySelector('.panes > .pane').getBoundingClientRect();
          const t = document.querySelector('.requirements-table');
          return {
            width: innerWidth,
            overflowTabs: [...document.querySelectorAll('.tabs .tab')].filter(e => e.getBoundingClientRect().right > pane.right + 1).map(e => e.textContent),
            tableOverflow: t.scrollWidth > t.clientWidth + 1,
            documentOverflow: document.documentElement.scrollWidth > innerWidth,
          };
        });
        assert.deepEqual(r.overflowTabs, [], `tabs overflow the pane at ${width}px`);
        assert.equal(r.tableOverflow, false, `the table scrolls sideways at ${width}px`);
        assert.equal(r.documentOverflow, false, `the document scrolls sideways at ${width}px`);
        seen.push(r.width);
      }
      await page.setViewportSize({width: 1280, height: 900});
      return {widths: seen};
    });

    // Requirement text is data, and must never be read as markup.
    await check('requirement-text-is-never-markup', async () => {
      // Echo back the IDs actually asked for. Inventing an ID the function
      // is not tagged with leaves the row with no text at all, so the
      // assertion would pass or fail for the wrong reason.
      await page.route('**/api/requirements?*', r => {
        const ids = new URL(r.request().url()).searchParams.get('ids').split(',');
        return r.fulfill({
          json: {present: true, entries: ids.map(id => ({id, text: '<b>literal requirement, not markup</b>'}))},
        });
      });
      await open(`#/fn/${tagged.id}/requirements`);
      await page.locator('.requirements-table[aria-busy="false"]').waitFor();
      assert.equal(await page.locator('.requirement-text b').count(), 0, 'markup in requirement text must not be rendered');
      assert.match(await page.locator('.requirement-text').first().textContent(), /literal requirement, not markup/);
      await page.unroute('**/api/requirements?*');
    });

    await check('a-read-only-server-refuses-writes', async () => {
      for (const route of ['/api/evidence', '/api/variants', '/api/tags', '/api/report']) {
        assert.equal((await fetch(srv.url + route)).status, 200, route);
      }
      const r = await fetch(`${srv.url}/api/resolutions`, {
        method: 'POST',
        headers: {'content-type': 'application/json'},
        body: '{}',
      });
      assert.equal(r.status, 403, 'a --read-only server must refuse a review write');
    });
  } finally {
    await br?.close();
    srv?.child.kill();
  }
  return results;
}

module.exports = {run};
