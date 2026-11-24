// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The showcase capture's screenshot assertion, against a real browser
// (#154, review of #212).
//
// Every screenshot on the showcase page carries a caption, and the caption is
// a claim about what the picture shows. `support/features.toml` writes that
// claim down as `expect`, and capture.cjs refuses to publish a shot whose
// `expect` is not in it. The whole guard is `shows()` -- so `shows()` has to
// mean "in the pixels that were published", not "somewhere in the document".
//
// It did not. It intersected the element's `getBoundingClientRect()` with the
// outer capture box alone, and a rectangle can lie inside the viewport while
// the element is painted nowhere, because an ancestor clips it. The viewer is
// built of independently scrolling panes -- source, object code, call graph --
// so this is not a hypothetical: asserted text can be scrolled out of its pane
// and the guard still passes. The first check below is the reviewer's: a 20px
// pane with `overflow: hidden` and the wanted text 60px beneath its top. The
// screenshot is blank and the old `shows()` said yes.
//
// These need no fixture and no butterfly binary: the subject is the assertion,
// so the pages are the smallest ones that put an element where each rule has
// to decide. They live with the browser checks rather than in `ui/*.test.cjs`
// because they need a real layout engine, and because that glob runs in CI
// before Playwright is installed.

const assert = require('node:assert/strict');
const path = require('node:path');
const {browser} = require('../harness.cjs');
const {shows, clipOf} = require('../../../scripts/showcase/capture.cjs');

const VIEW = {width: 400, height: 300};
const WANT = '28 gaps';

// A pane 20px high that clips, 100px down the page, with the wanted text 60px
// below the pane's own top -- inside the VIEWPORT, outside the pane.
const CLIPPED = `<style>body{margin:0;background:#fff;color:#000}
 #pane{position:absolute;top:100px;left:0;width:400px;height:20px;overflow:hidden}
 #tall{height:400px;position:relative}
 #target{position:absolute;top:OFFSETpx;left:0;font:16px monospace}</style>
 <div id="pane"><div id="tall"><span id="target">${WANT}</span></div></div>`;

/// The colours in a screenshot of `clip` (or of the viewport). A page whose
/// only content is clipped away is one colour: the background.
async function colours(page, clip) {
  const buf = await page.screenshot({type: 'png', ...(clip ? {clip} : {})});
  const w = Math.round(clip ? clip.width : VIEW.width);
  const h = Math.round(clip ? clip.height : VIEW.height);
  return page.evaluate(async ({data, w: cw, h: ch}) => {
    const img = new Image();
    await new Promise(r => { img.onload = r; img.src = data; });
    const c = document.createElement('canvas');
    c.width = cw; c.height = ch;
    const g = c.getContext('2d');
    g.drawImage(img, 0, 0);
    const px = g.getImageData(0, 0, cw, ch).data;
    const seen = new Set();
    for (let i = 0; i < px.length; i += 4) seen.add(`${px[i]},${px[i + 1]},${px[i + 2]}`);
    return seen.size;
  }, {data: `data:image/png;base64,${buf.toString('base64')}`, w, h});
}

async function run({out}) {
  const results = [];
  let br, page;
  const check = async (name, fn) => {
    try {
      const detail = await fn();
      results.push({name, pass: true, ...(detail ? {detail} : {})});
    } catch (e) {
      const shot = path.join(out, `showcase-${name}.png`);
      await page?.screenshot({path: shot}).catch(() => {});
      results.push({name, pass: false, error: e.message, screenshot: shot});
    }
  };

  try {
    br = await browser();
    const ctx = await br.newContext({viewport: VIEW, deviceScaleFactor: 1, reducedMotion: 'reduce'});
    page = await ctx.newPage();

    // THE REGRESSION. The rect of #target is at y=160, well inside the 400x300
    // viewport this shot would publish; the pane that contains it stops at
    // y=120. Nothing of the text is drawn.
    await check('text-clipped-out-of-a-scrolling-pane-is-not-shown', async () => {
      await page.setContent(CLIPPED.replace('OFFSETpx', '60px'));
      const rect = await page.locator('#target').boundingBox();
      const pane = await page.locator('#pane').boundingBox();
      assert.ok(rect.y > pane.y + pane.height, 'the fixture must put the text past the pane it is in');
      assert.ok(rect.y + rect.height < VIEW.height, 'and inside the viewport, or this tests the old below-the-fold rule');
      assert.equal(await colours(page, null), 1, 'the published pixels must really be blank, or the fixture is wrong');
      assert.equal(await shows(page, null, WANT), false, 'shows() must not accept text no pixel carries');
      return {rect: `${rect.x},${rect.y}`, pane: `${pane.y}..${pane.y + pane.height}`};
    });

    // ...and the same page with the text scrolled to where the pane paints it.
    // A guard that answers "no" to everything is no guard at all.
    await check('text-visible-in-the-same-pane-is-shown', async () => {
      await page.setContent(CLIPPED.replace('OFFSETpx', '2px'));
      assert.ok(await colours(page, null) > 1, 'the text must be in the pixels');
      assert.equal(await shows(page, null, WANT), true);
    });

    // The rule that was already there, kept: below the fold of the window the
    // shot publishes.
    await check('text-below-the-fold-is-not-shown', async () => {
      await page.setContent(`<style>body{margin:0;background:#fff}#target{position:absolute;top:900px;font:16px monospace}</style><span id="target">${WANT}</span>`);
      assert.equal(await shows(page, null, WANT), false);
    });

    // A cropped shot: `clip` is computed first and the assertion is made
    // inside it, so text outside the crop is not shown even when it is on
    // screen -- and text clipped out of a pane INSIDE the crop is not either.
    await check('a-crop-is-what-the-assertion-is-made-over', async () => {
      await page.setContent(`<style>body{margin:0;background:#fff}
        #box{position:absolute;top:0;left:0;width:200px;height:80px;border:1px solid #000}
        #inside{position:absolute;top:20px;left:10px;font:16px monospace}
        #outside{position:absolute;top:200px;left:10px;font:16px monospace}</style>
        <div id="box"><span id="inside">${WANT}</span></div><span id="outside">99 gaps</span>`);
      const clip = await clipOf(page, {clip: '#box'});
      assert.equal(await shows(page, clip, WANT), true, 'text inside the crop');
      assert.equal(await shows(page, clip, '99 gaps'), false, 'text outside the crop');
    });

    await check('a-clipped-pane-inside-a-crop-is-still-clipped', async () => {
      await page.setContent(`<style>body{margin:0;background:#fff}
        #crop{position:absolute;top:0;left:0;width:300px;height:250px}
        #pane{position:absolute;top:20px;left:0;width:300px;height:20px;overflow:hidden}
        #tall{height:400px;position:relative}
        #target{position:absolute;top:60px;left:0;font:16px monospace}</style>
        <div id="crop"><div id="pane"><div id="tall"><span id="target">${WANT}</span></div></div></div>`);
      const clip = await clipOf(page, {clip: '#crop'});
      // the element's rect is inside the crop; the pane paints none of it
      const rect = await page.locator('#target').boundingBox();
      assert.ok(rect.y > clip.y && rect.y < clip.y + clip.height, 'the fixture must put the rect inside the crop');
      assert.equal(await shows(page, clip, WANT), false);
    });

    // Rendered visibility, not just geometry: `visibility: hidden` leaves a
    // rectangle behind and paints nothing.
    await check('text-hidden-by-visibility-or-display-is-not-shown', async () => {
      await page.setContent(`<style>body{margin:0;background:#fff}
        #a{visibility:hidden;font:16px monospace}#b{display:none}</style>
        <div id="a">${WANT}</div><div id="b">99 gaps</div>`);
      assert.equal(await colours(page, null), 1, 'nothing of either is painted');
      assert.equal(await shows(page, null, WANT), false, 'visibility: hidden');
      assert.equal(await shows(page, null, '99 gaps'), false, 'display: none');
    });

    // The walk must not over-reject: a `position: fixed` element is NOT
    // clipped by an `overflow: hidden` ancestor that is not its containing
    // block, and the browser really does paint it.
    await check('a-fixed-element-escaping-a-clipping-ancestor-is-shown', async () => {
      await page.setContent(`<style>body{margin:0;background:#fff}
        #pane{position:absolute;top:100px;left:0;width:400px;height:20px;overflow:hidden}
        #target{position:fixed;top:200px;left:0;font:16px monospace}</style>
        <div id="pane"><span id="target">${WANT}</span></div>`);
      assert.ok(await colours(page, null) > 1, 'the browser paints it outside the pane');
      assert.equal(await shows(page, null, WANT), true);
    });

    // A scrolling pane that has been scrolled: what is off the top of it is
    // gone, what is in it is not. This is the viewer's source pane.
    await check('a-scrolled-pane-shows-only-what-it-scrolled-to', async () => {
      await page.setContent(`<style>body{margin:0;background:#fff}
        #pane{position:absolute;top:0;left:0;width:400px;height:40px;overflow:auto}
        .row{height:40px;font:16px monospace}</style>
        <div id="pane"><div class="row">${WANT}</div><div class="row">99 gaps</div></div>`);
      await page.locator('#pane').evaluate(el => { el.scrollTop = 40; });
      assert.equal(await shows(page, null, WANT), false, 'scrolled off the top of its pane');
      assert.equal(await shows(page, null, '99 gaps'), true, 'scrolled to');
    });
  } finally {
    await page?.close().catch(() => {});
    await br?.close();
  }
  return results;
}

module.exports = {run};
