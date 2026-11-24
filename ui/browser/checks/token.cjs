// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The token page, when it comes back after the viewer has loaded (#65).
//
// A 401 on a read replaces the viewer with the token page. The viewer's own
// document-level handlers used to survive that: '/' was still the search
// shortcut, so a slash typed into the token field was swallowed and the
// handler then threw on the #search that was gone, and every click threw on
// the missing #prov. A token may contain '/', so re-entering a good one
// silently changed it. No VM test sees this -- it takes real focus, real key
// events and a page that has really been replaced.
//
// The server here is a real one with a real token, restarted on the same
// port with a different token: what an open tab sees when the operator
// restarts butterfly. The port comes from the server's own URL line.

const path = require('node:path');
const fs = require('node:fs');
const assert = require('node:assert/strict');
const {scratch, ingest, serveWithToken, stop, browser, waitFor} = require('../harness.cjs');

// Both valid under the token rule (16+ visible ASCII). The second carries the
// viewer's shortcut keys -- '/', '[', ']', 'f', 'j', 'k' and digits -- so
// any shortcut still listening would show up as a missing character.
const FIRST = 'first-login-token-0123456789';
const SECOND = 'abc/def/jk[f]19/restarted-0123';

async function run({out}) {
  const dir = scratch('token');
  const got = ingest('c-per-test', dir);
  if (got.skip) return [{name: 'fixture', pass: false, skip: got.skip}];
  const evidence = JSON.parse(fs.readFileSync(got.evidence, 'utf8'));
  const tagged = evidence.functions.find(f => (f.requirements || []).length);
  if (!tagged) throw new Error('examples/c-per-test carries no tagged function');

  const results = [];
  let br, page, srv;
  const check = async (name, fn) => {
    try {
      const detail = await fn();
      results.push({name, pass: true, ...(detail ? {detail} : {})});
    } catch (e) {
      const shot = path.join(out, `token-${name}.png`);
      await page?.screenshot({path: shot}).catch(() => {});
      results.push({name, pass: false, error: e.message, screenshot: shot});
    }
  };

  try {
    br = await browser();
    page = await br.newPage({viewport: {width: 1280, height: 900}});
    const pageErrors = [];
    page.on('pageerror', e => pageErrors.push(e.message));
    // Console errors are collected from the moment the recovery page is up.
    // Before that, the refused read itself is logged (the browser's own
    // "401" line, and the viewer's note of the refusal); that is expected.
    let consoleErrors = null;
    page.on('console', m => { if (consoleErrors && m.type() === 'error') consoleErrors.push(m.text()); });
    const sent = [];
    page.on('request', r => { if (r.url().includes('/api/variants')) sent.push(r.headers().authorization || null); });

    const tokenPageUp = () => waitFor(page, () => !!document.getElementById('tokenInput'), null, {what: 'the token page'});
    const viewerUp = name => waitFor(page, n => document.querySelector('#content h1')?.textContent === n, name, {what: `the viewer showing ${name}`});
    // By keyboard, one key at a time, as a person types it -- `fill` would
    // set the value directly and never meet a keydown handler.
    const typeToken = async token => {
      await page.locator('#tokenInput').click();
      await page.keyboard.type(token, {delay: 10});
      return page.locator('#tokenInput').inputValue();
    };

    await check('a-later-401-takes-a-slash-token-by-keyboard', async () => {
      srv = await serveWithToken(got.evidence, {token: FIRST, dir, out, tag: 'first'});

      // A successful first login through the token page.
      await page.goto(`${srv.url}/#/fn/${tagged.id}/flow`);
      await tokenPageUp();
      assert.equal(await typeToken(FIRST), FIRST, 'the first token must arrive as typed');
      // The form stores the token and reloads; wait for that load, or a
      // poll can land in the page being torn down.
      let reloaded = page.waitForEvent('load');
      await page.keyboard.press('Enter');
      await reloaded;
      await viewerUp(tagged.name);
      // Everything the load starts (freshness, tags) has to have come back,
      // or its listener registration would not be the one being tested.
      await page.waitForLoadState('networkidle');
      assert.equal(pageErrors.length, 0, `page errors on the first login: ${pageErrors.join(' | ')}`);

      // The operator restarts with a new token, on the same port.
      const {port} = srv;
      await stop(srv.child);
      srv = await serveWithToken(got.evidence, {token: SECOND, port, dir, out, tag: 'second'});
      assert.equal(srv.port, port);

      // An ordinary read from the loaded viewer: opening a requirement.
      await page.locator('.req.has-text').first().click();
      await tokenPageUp();
      consoleErrors = [];
      const before = sent.length;

      const typed = await typeToken(SECOND);
      assert.equal(typed, SECOND,
        `the recovery field must hold exactly what was typed, got ${JSON.stringify(typed)}; page errors: ${pageErrors.join(' | ') || 'none'}`);
      reloaded = page.waitForEvent('load');
      await page.locator('#tokenForm button[type=submit]').click();
      await reloaded;
      await viewerUp(tagged.name);
      await page.waitForLoadState('networkidle');

      const stored = await page.evaluate(() => sessionStorage.getItem('sl-token'));
      assert.equal(stored, SECOND, 'the tab must keep the token as typed');
      const after = sent.slice(before);
      assert.ok(after.length > 0, 'the viewer must have asked the server again');
      assert.ok(after.every(a => a === `Bearer ${SECOND}`), `every request must carry the typed token: ${JSON.stringify(after)}`);
      assert.equal(pageErrors.length, 0, `page errors: ${pageErrors.join(' | ')}`);
      assert.deepEqual(consoleErrors, [], 'no console errors from the recovery page on');
      return {port, requestsAfterRecovery: after.length};
    });
  } finally {
    await page?.close().catch(() => {});
    await br?.close();
    if (srv) await stop(srv.child);
  }
  return results;
}

module.exports = {run};
