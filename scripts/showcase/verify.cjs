#!/usr/bin/env node
// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Opens the generated showcase page the way a reader will -- from file://,
// with the network OFF -- and checks what a reader would trip over (#154):
//
//   * no console errors, no page errors, no request that is not file:/data:
//   * every image loads
//   * every internal #link lands on an element
//   * the page never scrolls sideways, at desktop, tablet and phone widths
//   * in light and in dark
//
//   node scripts/showcase/verify.cjs [page.html ...] [--shots <dir>]
//
// Exit 1 on any finding. `--shots` keeps a screenshot per width and theme.

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const {ROOT} = require('./spec.cjs');
const H = require(path.join(ROOT, 'ui/browser/harness.cjs'));

const WIDTHS = [1460, 1024, 400];

async function main() {
  const argv = process.argv.slice(2);
  let shots = null;
  const pages = [];
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--shots') shots = path.resolve(argv[++i]);
    else pages.push(path.resolve(argv[i]));
  }
  if (!pages.length) pages.push(path.join(ROOT, 'docs/showcase/index.html'));
  if (shots) fs.mkdirSync(shots, {recursive: true});

  const findings = [];
  const br = await H.browser();
  try {
    for (const file of pages) {
      for (const scheme of ['light', 'dark']) {
        for (const width of WIDTHS) {
          const tag = `${path.basename(file)} ${width}px ${scheme}`;
          const ctx = await br.newContext({viewport: {width, height: 900}, colorScheme: scheme, offline: true});
          const page = await ctx.newPage();
          page.on('console', m => { if (m.type() === 'error') findings.push(`${tag}: console error: ${m.text()}`); });
          page.on('pageerror', e => findings.push(`${tag}: page error: ${e.message}`));
          page.on('request', r => { if (!/^(file|data):/.test(r.url())) findings.push(`${tag}: network request to ${r.url()}`); });
          await page.goto(`file://${file}`);
          // lazy images load as they scroll into view: walk the page once
          await page.evaluate(async () => {
            for (const img of document.images) { img.loading = 'eager'; }
            await Promise.all([...document.images].map(i => (i.complete ? null : new Promise(r => { i.onload = i.onerror = r; }))));
          });
          const r = await page.evaluate(() => ({
            overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
            wide: [...document.querySelectorAll('body *')].filter(e => e.getBoundingClientRect().right > document.documentElement.clientWidth + 1 && !e.closest('pre, .scroll')).slice(0, 5).map(e => `${e.tagName.toLowerCase()}.${e.className}`),
            broken: [...document.images].filter(i => !i.naturalWidth).map(i => i.getAttribute('src').slice(0, 60)),
            noAlt: [...document.images].filter(i => !(i.getAttribute('alt') || '').trim()).length,
            images: document.images.length,
            dead: [...document.querySelectorAll('a[href^="#"]')].map(a => a.getAttribute('href')).filter(h => h.length > 1 && !document.getElementById(decodeURIComponent(h.slice(1)))),
            relative: [...document.querySelectorAll('a[href]')].map(a => a.getAttribute('href')).filter(h => !/^(#|https?:|data:)/.test(h)),
            bg: getComputedStyle(document.body).backgroundColor,
          }));
          if (r.overflow > 0) findings.push(`${tag}: the page overflows sideways by ${r.overflow}px (${r.wide.join(', ')})`);
          for (const b of r.broken) findings.push(`${tag}: image did not load: ${b}`);
          if (r.noAlt) findings.push(`${tag}: ${r.noAlt} image(s) without alt text`);
          for (const d of r.dead) findings.push(`${tag}: internal link ${d} has no target`);
          for (const rel of new Set(r.relative)) {
            if (!fs.existsSync(path.resolve(path.dirname(file), rel.split('#')[0]))) findings.push(`${tag}: relative link ${rel} does not exist`);
          }
          console.error(`verify: ${tag}: ${r.images} images, background ${r.bg}, overflow ${r.overflow}px`);
          if (shots) await page.screenshot({path: path.join(shots, `${path.basename(file, '.html')}-${width}-${scheme}.png`), fullPage: false});
          await ctx.close();
        }
      }
    }
  } finally {
    await br.close();
  }
  for (const f of findings) console.error(`verify: FAIL ${f}`);
  console.error(findings.length ? `verify: ${findings.length} finding(s)` : 'verify: clean');
  process.exit(findings.length ? 1 : 0);
}

main().catch(e => { console.error(e); process.exit(1); });
