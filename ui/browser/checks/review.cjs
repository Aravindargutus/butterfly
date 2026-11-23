// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The review workflow against a writable server (#69, BF-QA-04..05).
//
// The server gets its own evidence copy and its own explicit --resolutions
// path inside a private temp directory, so a review saved here can never
// land in the developer's work tree -- which the original scripts risked by
// relying on the default relative sidecar name.

const path = require('node:path');
const assert = require('node:assert/strict');
const {scratch, ingest, serve, browser} = require('../harness.cjs');

async function run({out}) {
  const dir = scratch('review');
  const got = ingest('c-per-test', dir);
  if (got.skip) return [{name: 'fixture', pass: false, skip: got.skip}];

  const results = [];
  let br;
  // Each check gets its own server, its own resolutions sidecar and its own
  // page. These tests write reviews, so sharing one server makes each check
  // depend on the previous one having cleaned up after itself -- and when an
  // early check fails, every later one fails too for reasons that have
  // nothing to do with what it was testing.
  const check = async (name, fn) => {
    let srv, page;
    try {
      srv = await serve(got.evidence, {dir, out, resolutions: path.join(dir, `resolutions-${name}.json`)});
      page = await br.newPage({viewport: {width: 1280, height: 900}});
      await page.goto(`${srv.url}/#/fn/0/flow`);
      await page.locator('#content').waitFor();
      const detail = await fn(page, srv);
      results.push({name, pass: true, ...(detail ? {detail} : {})});
    } catch (e) {
      const shot = path.join(out, `review-${name}.png`);
      await page?.screenshot({path: shot}).catch(() => {});
      results.push({name, pass: false, error: e.message, screenshot: shot});
    } finally {
      await page?.close().catch(() => {});
      srv?.child.kill();
    }
  };

  try {
    br = await browser();

    // BF-QA-04: a review saves, survives a reload, and can be removed.
    await check('a-review-saves-reloads-and-can-be-removed', async (page, srv) => {
      await page.getByRole('button', {name: 'source→object: not reviewed', exact: true}).click();
      await page.locator('#modal textarea').fill('QA TEST ONLY: exercising the save path, not a verification conclusion.');
      await page.getByPlaceholder('reviewer', {exact: true}).fill('QA fixture');
      await page.getByRole('button', {name: 'Save', exact: true}).click();
      await page.locator('#modal.hidden').waitFor({state: 'attached'});

      await page.reload();
      await page.getByRole('button', {name: /source→object: traceable · QA fixture/}).waitFor();
      const saved = await (await fetch(`${srv.url}/api/resolutions`)).json();
      assert.equal(saved.entries.length, 1, 'exactly one record must be written');
      assert.equal(saved.entries[0].author, 'QA fixture');

      await page.getByRole('button', {name: /source→object: traceable · QA fixture/}).click();
      await page.getByRole('button', {name: 'Remove', exact: true}).click();
      await page.getByRole('button', {name: 'source→object: not reviewed', exact: true}).waitFor();
      const after = await (await fetch(`${srv.url}/api/resolutions`)).json();
      assert.equal(after.entries.length, 0, 'removal must clear the record');
    });

    // BF-QA-05: a failed save must keep the reviewer's words. Losing a
    // justification somebody just wrote is the most expensive small bug in
    // the viewer, and only a real browser shows it.
    await check('a-failed-save-keeps-the-draft', async (page, srv) => {
      await page.getByRole('button', {name: 'source→object: not reviewed', exact: true}).click();
      await page.locator('#modal textarea').fill('QA unsaved draft that must survive a temporary server failure.');
      await page.route('**/api/resolutions*', r => (r.request().method() === 'POST'
        ? r.fulfill({status: 500, body: 'QA simulated storage failure'})
        : r.continue()));
      let alert = '';
      page.once('dialog', async d => { alert = d.message(); await d.accept(); });
      await page.getByRole('button', {name: 'Save', exact: true}).click();
      await page.waitForFunction(() => true);
      await new Promise(r => setTimeout(r, 300));
      await page.unroute('**/api/resolutions*');

      assert.match(alert, /could not save/i, 'the reviewer must be told the save failed');
      assert.equal(await page.locator('#modal').isVisible(), true, 'a failed save must leave the dialog open');
      assert.match(
        await page.locator('#modal textarea').inputValue(),
        /QA unsaved draft/,
        'the draft must still be there after a failed save',
      );
      return {alert};
    });
  } finally {
    await br?.close();
  }
  return results;
}

module.exports = {run};
