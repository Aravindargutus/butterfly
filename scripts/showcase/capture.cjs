#!/usr/bin/env node
// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The showcase capture run (#154): every screenshot and every terminal block
// on the showcase page comes from here, and from nowhere else.
//
//   node scripts/showcase/capture.cjs [--only V02,V14] [--out docs/showcase]
//                                     [--reuse-fixtures]
//
// What it does, in order:
//
//   1. records the commit, the host and the tool versions;
//   2. for every fixture a captured section names, runs that fixture's
//      `build` commands VERBATIM from the repository root -- the same strings
//      the page prints in its "Reproduce it" block -- and reads the numbers
//      the page will quote out of the evidence they produced (facts.cjs);
//   3. for every section, runs its `cmd` entries verbatim and keeps their
//      real output and exit status;
//   4. serves each fixture with the browser-check harness
//      (ui/browser/harness.cjs), drives the viewer through each shot's steps
//      and takes the screenshot;
//   5. writes `capture.json` (everything above) and `img/` beside it, with
//      the numbers each section consumed recorded BESIDE that section -- so
//      that `--only`, which leaves the other sections' records and images
//      from an earlier run, cannot rewrite their text.
//
// Two kinds of "did not work", and they are kept apart on purpose:
//
//   not captured   the HOST cannot show it -- a fixture that does not build
//                  here, an AArch64-only panel on x86-64. Recorded with the
//                  reason, rendered on the page as "not captured on this
//                  host", and the run still succeeds.
//   failed         the TOOL did not do what features.toml says it does -- a
//                  command ended with another status, a selector never
//                  appeared, an expected text is not on the page. Recorded,
//                  rendered the same way so nothing is faked, and the run
//                  exits 1: a claim drifted and somebody has to look.
//
// `butterfly` in a command is the binary built from THIS checkout:
// target/debug is put first on PATH, and the run refuses to start without it.

'use strict';

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const {spawnSync} = require('node:child_process');
const {ROOT, load, matches, wantPattern, factKeys, CAPTURE_SCHEMA} = require('./spec.cjs');
const {fixtureFacts, fill} = require('./facts.cjs');
const {REPO} = require('./markup.cjs');
const H = require(path.join(ROOT, 'ui/browser/harness.cjs'));

const VIEWPORT = {width: 1460, height: 900};
const IMAGE_BUDGET = 250 * 1024;
const WORK = path.join(ROOT, 'target/showcase');

function args() {
  const a = process.argv.slice(2);
  const opt = {only: null, reuse: false, out: path.join(ROOT, 'docs/showcase')};
  for (let i = 0; i < a.length; i++) {
    if (a[i] === '--only') {
      const v = a[++i];
      if (!v || v.startsWith('--')) { console.error('capture: --only needs a comma-separated list of section ids, as in --only V02,V14'); process.exit(2); }
      opt.only = new Set(v.split(',').map(x => x.trim()).filter(Boolean));
    } else if (a[i] === '--out') opt.out = path.resolve(a[++i]);
    else if (a[i] === '--reuse-fixtures') opt.reuse = true;
    else { console.error(`capture: unknown argument ${a[i]}`); process.exit(2); }
  }
  return opt;
}

const say = msg => process.stderr.write(`capture: ${msg}\n`);

/// One line of a tool's `--version`, or null when it is not installed.
function version(cmd, argv = ['--version']) {
  const r = spawnSync(cmd, argv, {encoding: 'utf8'});
  if (r.status !== 0) return null;
  return (r.stdout || r.stderr).split('\n')[0].trim();
}

function git(...argv) {
  const r = spawnSync('git', ['-C', ROOT, ...argv], {encoding: 'utf8'});
  return r.status === 0 ? r.stdout.trim() : null;
}

function meta() {
  // The page's own output directory does not make the tree "modified": it is
  // what this run writes. Anything else uncommitted is said in the footer,
  // because a page that names a commit is claiming to show that commit.
  const dirty = (git('status', '--porcelain', '--', '.', ':!docs/showcase') || '')
    .split('\n').filter(l => l && !/ui\/browser\/node_modules$/.test(l));
  return {
    // which repository that commit is in: the page links it there (#7)
    repo: REPO,
    commit: git('rev-parse', 'HEAD'),
    commitShort: git('rev-parse', '--short=12', 'HEAD'),
    commitDate: git('show', '-s', '--format=%cI', 'HEAD'),
    branch: git('rev-parse', '--abbrev-ref', 'HEAD'),
    modified: dirty.length,
    capturedAt: new Date().toISOString(),
    os: `${os.type()} ${os.release()}`,
    platform: process.platform,
    arch: process.arch,
    tools: {
      butterfly: version(H.BIN),
      rustc: version('rustc'),
      clang: version(process.env.CC || 'clang'),
      'clang++': version(process.env.CXX || 'clang++'),
      node: process.version,
      playwright: require(path.join(ROOT, 'ui/browser/node_modules/playwright/package.json')).version,
      ctags: version('ctags'),
    },
  };
}

/// Paths that differ between two checkouts of the same commit are replaced
/// in TERMINAL output, so a second run compares equal and the page does not
/// publish a home directory. Screenshots are never edited.
function scrub(text) {
  const swaps = [[fs.realpathSync(ROOT), '<repo>'], [ROOT, '<repo>'], [fs.realpathSync(os.tmpdir()), '<tmp>'], [os.homedir(), '~']];
  for (const [from, to] of swaps) text = text.split(from).join(to);
  // ANSI colour, should a tool emit it despite NO_COLOR
  // eslint-disable-next-line no-control-regex
  return text.replace(/\x1b\[[0-9;]*m/g, '');
}

/// Run one command string exactly as written, from the repository root, with
/// stdout and stderr interleaved the way a terminal shows them.
function sh(run) {
  fs.mkdirSync(WORK, {recursive: true});
  const log = path.join(WORK, `cmd-${process.pid}.log`);
  const fd = fs.openSync(log, 'w');
  const started = Date.now();
  const r = spawnSync('sh', ['-c', run], {
    cwd: ROOT,
    stdio: ['ignore', fd, fd],
    env: {...process.env, PATH: `${path.dirname(H.BIN)}${path.delimiter}${process.env.PATH}`, NO_COLOR: '1'},
    timeout: 15 * 60_000,
  });
  fs.closeSync(fd);
  const output = scrub(fs.readFileSync(log, 'utf8'));
  fs.rmSync(log, {force: true});
  return {run, exit: r.status ?? `signal ${r.signal}`, output, ms: Date.now() - started};
}

const lastLine = text => text.trim().split('\n').filter(Boolean).pop() || '(no output)';

/* -------------------- the facts a section was filled from -------------------
 *
 * A section's text and its screenshot are one statement: "this is what the
 * tool showed, and these are the numbers it showed". They have to come from
 * one moment, or the page states a number that contradicts the picture
 * printed beneath it.
 *
 * `--only` is where that came apart. The run keeps the other sections'
 * records and images from the previous capture and rebuilds only the named
 * sections' fixtures -- but the numbers used to be ONE object shared by the
 * whole page, and the rebuilt fixture overwrote it. A partial refresh of V02
 * against a changed `examples/c` rewrote V06's caption from "2 of 3
 * conditions proven" to "3 of 3" while V06 kept the screenshot, and the
 * commit badge, of the run where it was 2 of 3 (#212 review, finding 1).
 *
 * So each section's record carries the facts THAT section consumed. The
 * snapshots are stored once each, by content, in `factSets`, and referenced
 * by hash: a full run stores exactly what the old shared object held, and a
 * partial run adds only the fixtures it actually rebuilt. Sections left
 * behind keep pointing at the snapshot they were captured with.
 *
 * The alternative -- invalidating every section that consumes a refreshed
 * fixture and forcing its recapture -- was rejected: `c` alone backs six
 * sections, so `--only V02` would have to re-shoot most of the page, which
 * is the thing `--only` exists to avoid; and it would still not make the
 * page honest, because a section can quote numbers from a fixture whose
 * evidence it does not itself photograph.
 */

/// Just the fact keys section `f` is allowed to quote (spec.cjs::factKeys).
function view(facts, f) {
  const out = {};
  for (const k of factKeys(f)) if (k in facts) out[k] = facts[k];
  return out;
}

/// Store `snapshot` in `manifest.factSets` by content and point `f`'s record
/// at it. Identical facts are stored once, so a full run's capture.json is
/// the size it always was.
function record(manifest, f, snapshot) {
  const rec = manifest.features[f.id];
  if (!rec) return;
  rec.facts = {};
  for (const [key, value] of Object.entries(snapshot)) {
    const id = crypto.createHash('sha256').update(JSON.stringify(value)).digest('hex').slice(0, 16);
    manifest.factSets[id] = value;
    rec.facts[key] = id;
  }
}

/// What `--only` leaves on disk: this run's sections over the previous run's,
/// each section still carrying its own numbers.
function mergePartial(prior, run, only) {
  return {
    ...run,
    fixtures: {...prior.fixtures, ...run.fixtures},
    features: {...prior.features, ...run.features},
    // Hash-keyed, so this is a union and never an overwrite: the snapshot a
    // section left behind still resolves to the numbers it was captured with.
    factSets: {...prior.factSets, ...run.factSets},
    meta: {
      ...run.meta,
      // ACCUMULATED, not overwritten: two `--only` runs in a row both left the
      // page saying the earlier one had not happened (#212 B5).
      partial: [...new Set([...(prior.meta?.partial || []), ...only])].sort(),
    },
  };
}

/// Drop snapshots no section references any more, so a long series of `--only`
/// runs does not grow capture.json for ever.
function prune(manifest) {
  const live = new Set(Object.values(manifest.features).flatMap(r => Object.values(r.facts || {})));
  for (const id of Object.keys(manifest.factSets)) if (!live.has(id)) delete manifest.factSets[id];
  return manifest;
}

async function main() {
  const opt = args();
  process.chdir(ROOT);
  if (!fs.existsSync(H.BIN)) {
    say(`${path.relative(ROOT, H.BIN)} does not exist. Run \`cargo build\` first: the page must show the binary built from this checkout.`);
    process.exit(2);
  }
  const spec = load();
  if (opt.only) {
    // `--only V99` used to prune img/, capture nothing and exit 0.
    const known = new Set(spec.features.map(f => f.id));
    const unknown = [...opt.only].filter(id => !known.has(id));
    if (unknown.length) { say(`--only names no section of support/features.toml: ${unknown.join(', ')}`); process.exit(2); }
    const notCaptured = [...opt.only].filter(id => spec.features.find(f => f.id === id).status !== 'captured');
    if (notCaptured.length) { say(`--only names ${notCaptured.join(', ')}, which ${notCaptured.length === 1 ? 'is' : 'are'} not \`status = "captured"\`: there is nothing to capture`); process.exit(2); }
  }
  const features = spec.features.filter(f => f.status === 'captured' && (!opt.only || opt.only.has(f.id)));
  const imgDir = path.join(opt.out, 'img');
  fs.mkdirSync(imgDir, {recursive: true});
  if (!opt.only) for (const f of fs.readdirSync(imgDir)) fs.rmSync(path.join(imgDir, f));

  const manifest = {schema: CAPTURE_SCHEMA, meta: meta(), fixtures: {}, features: {}, factSets: {}};
  // The numbers this RUN read out of the evidence. Never written to
  // capture.json as one shared object: each section keeps its own view of it
  // (`snapshot` below), because `--only` leaves other sections behind (#212
  // review, finding 1).
  const facts = {};
  let failed = 0;

  /* ------------------------------ fixtures ------------------------------ */
  const needed = new Set(features.flatMap(f => [...f.fixtures, ...(f.shot || []).map(s => s.fixture).filter(Boolean)]));
  for (const name of needed) {
    const fx = spec.fixtures[name];
    const rec = {commands: [], ok: true};
    manifest.fixtures[name] = rec;
    for (const dep of fx.needs || []) {
      if (!manifest.fixtures[dep]?.ok) { rec.ok = false; rec.reason = `it is built from fixture \`${dep}\`, which was not built`; }
    }
    // --reuse-fixtures is for iterating on a shot. The footer says when it was
    // used: such a page does not show a fresh build.
    const reusable = opt.reuse && [].concat(fx.evidence).every(p => fs.existsSync(path.join(ROOT, p)));
    if (reusable) rec.reused = true;
    for (const run of rec.ok && !reusable ? fx.build : []) {
      say(`[${name}] $ ${run}`);
      const r = sh(run);
      rec.commands.push({run, exit: r.exit, ms: r.ms});
      if (r.exit !== 0) {
        rec.ok = false;
        rec.reason = `\`${run}\` exited ${r.exit}: ${lastLine(r.output)}`;
        say(`[${name}] NOT BUILT: ${rec.reason}`);
        break;
      }
    }
    if (rec.ok) {
      const paths = [].concat(fx.evidence).map(p => path.join(ROOT, p));
      const missing = paths.find(p => !fs.existsSync(p));
      if (missing) { rec.ok = false; rec.reason = `${path.relative(ROOT, missing)} was not produced`; } else facts[name] = fixtureFacts(paths);
    }
  }

  /* -------------------------- terminal commands ------------------------- */
  for (const f of features) {
    // The commit this section was captured at. With `--only`, the rest of the
    // page comes from an earlier run at an earlier commit, and the lede and
    // the footer must not claim this one for all of it (#212 B5).
    const rec = {commands: [], shots: [], repo: manifest.meta.repo, commit: manifest.meta.commit, capturedAt: manifest.meta.capturedAt};
    manifest.features[f.id] = rec;
    const broken = f.fixtures.find(n => !manifest.fixtures[n].ok);
    if (broken) { rec.notCaptured = `fixture \`${broken}\` could not be built: ${manifest.fixtures[broken].reason}`; continue; }
    for (const c of f.cmd || []) {
      say(`[${f.id}] $ ${c.run}`);
      const r = sh(c.run);
      const entry = {run: c.run, exit: r.exit, expected: c.exit, ms: r.ms};
      if (c.show) entry.output = r.output;
      if (r.exit !== c.exit) {
        entry.failed = `expected exit ${c.exit}, got ${r.exit}: ${lastLine(r.output)}`;
        say(`[${f.id}] FAILED: ${entry.failed}`);
        failed++;
      }
      for (const want of c.expect || []) {
        if (!matches(r.output, want)) {
          entry.failed = `the output does not contain ${JSON.stringify(want)}`;
          say(`[${f.id}] FAILED: ${entry.failed}`);
          failed++;
        }
      }
      rec.commands.push(entry);
    }
    // Facts a section's own commands produced (the baseline pair, the
    // change history): read after they have run.
    for (const [key, p] of Object.entries(f.facts || {})) {
      const paths = [].concat(p).map(x => path.join(ROOT, x));
      if (paths.every(x => fs.existsSync(x))) facts[key] = fixtureFacts(paths);
    }
  }

  /* ------------------------------ screenshots --------------------------- */
  const scratch = H.scratch('showcase');
  const servers = new Map();
  const serverFor = async (f, s) => {
    const name = s.fixture || f.fixtures[0];
    // Relative to the repository root, which is this process's directory:
    // the viewer prints the evidence path in the Gates box's command, and it
    // should be the path a reader following the reproduce block will see.
    const paths = [].concat(s.evidence || spec.fixtures[name].evidence);
    const key = s.isolate ? `${f.id}:${s.file}` : `${paths.join('|')}:${s.token ? 'token' : 'open'}:${s.resolutions || ''}`;
    if (!servers.has(key)) {
      const dir = fs.mkdtempSync(path.join(scratch, 'srv-'));
      const srv = s.token
        ? await H.serveWithToken(paths[0], {token: s.token, dir, out: scratch})
        : await H.serve(paths, {dir, out: scratch, ...(s.resolutions ? {resolutions: path.join(ROOT, s.resolutions)} : {})});
      const headers = s.token ? {authorization: `Bearer ${s.token}`} : {};
      srv.evidence = await (await fetch(`${srv.url}/api/evidence`, {headers})).json();
      servers.set(key, srv);
    }
    return servers.get(key);
  };

  let br;
  let visit = 0;
  try {
    br = await H.browser();
    manifest.meta.tools.chromium = br.version();
    for (const f of features) {
      const rec = manifest.features[f.id];
      // The facts THIS section may quote, and the only ones its text is ever
      // filled from -- here and in build.cjs. spec.cjs has already refused a
      // section that quotes evidence it does not name, so this is complete.
      const own = view(facts, f);
      for (const s of f.shot || []) {
        const out = {file: null, source: s.file};
        rec.shots.push(out);
        if (rec.notCaptured) { out.notCaptured = rec.notCaptured; continue; }
        const fxName = s.fixture || f.fixtures[0];
        if (!manifest.fixtures[fxName]?.ok) { out.notCaptured = `fixture \`${fxName}\` could not be built: ${manifest.fixtures[fxName]?.reason}`; continue; }
        let ctx;
        try {
          // Inside the try: `requires_reason` goes through fill(), and an
          // unresolved placeholder in it used to throw out of the whole run --
          // on a host where the gate is falsy, with img/ already pruned and no
          // capture.json written (#212 S23). It is a shot failure like any
          // other. `continue` still runs the `finally`.
          if (s.requires) {
            let v = own;
            for (const k of s.requires.split('.')) v = v?.[k];
            if (!v) { out.notCaptured = fill(s.requires_reason, own, `${f.id} ${s.file}`); say(`[${f.id}] ${s.file}: not captured on this host: ${out.notCaptured}`); continue; }
          }
          // A `page` shot opens a file one of the section's commands wrote
          // (the static report); everything else is the served viewer.
          const srv = s.page ? {url: `file://${path.join(ROOT, s.page)}`, evidence: {functions: [], files: []}} : await serverFor(f, s);
          const lookup = text => text
            .replace(/\{fn:([^}]+)\}/g, (_, n) => {
              const hit = srv.evidence.functions.find(x => x.name === n);
              if (!hit) throw new Error(`no function named ${n} in the served evidence`);
              return hit.id;
            })
            .replace(/\{file:([^}]+)\}/g, (_, n) => {
              const hit = srv.evidence.files.find(x => x.display === n || x.display.endsWith(`/${n}`));
              if (!hit) throw new Error(`no file named ${n} in the served evidence`);
              return hit.id;
            });
          ctx = await br.newContext({viewport: s.viewport ? {width: s.viewport[0], height: s.viewport[1]} : VIEWPORT, deviceScaleFactor: 1, reducedMotion: 'reduce'});
          // Seeds go in before the viewer's own script reads them, which is
          // the only moment that matters: `sl-mode` is read once at startup.
          await ctx.addInitScript(seed => {
            try { for (const [k, v] of Object.entries(seed)) if (localStorage.getItem(k) === null) localStorage.setItem(k, v); } catch { /* private window */ }
          }, s.seed || {});
          const page = await ctx.newPage();
          const pageErrors = [];
          page.on('pageerror', e => pageErrors.push(e.message));
          const route = lookup(s.route);
          out.route = route;
          // A distinct URL per visit, as checks/requirements.cjs does: the
          // viewer caches requirement text per session.
          await page.goto(s.page ? srv.url : `${srv.url}/?shot=${++visit}${route}`);
          if (!s.token_prompt && !s.page) {
            await H.waitFor(page, () => (document.querySelector('#content')?.textContent.length || 0) > 0, null, {what: 'the viewer to render'});
          }
          for (const st of s.steps || []) await step(page, st, lookup);
          await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r()))));
          // A caption is a claim, so `expect` must hold over WHAT IS
          // PUBLISHED, not over the document. It used to be asserted against
          // the whole page's innerText while 20 of 43 shots are cropped to a
          // `clip` and the rest show a 1460x900 window of a taller page: the
          // asserted text could be absent from every pixel that went out
          // (#212 S12). The clip is computed first and the assertion is made
          // inside it, at the moment the screenshot is taken.
          const clip = await clipOf(page, s);
          for (const want of s.expect || []) {
            const w = fill(want, own, `${f.id} ${s.file}`);
            if (!await shows(page, clip, w)) throw new Error(`${clip ? 'the cropped screenshot' : 'the captured viewport'} does not show ${JSON.stringify(w)}`);
          }
          if (pageErrors.length) throw new Error(`the viewer raised: ${pageErrors.join('; ')}`);
          Object.assign(out, await shoot(page, s, imgDir, clip));
          out.bytes = fs.statSync(path.join(imgDir, out.file)).size;
          out.url = s.page ? s.page : page.url().replace(srv.url, 'http://127.0.0.1:8787').replace(/\?shot=\d+/, '');
          say(`[${f.id}] ${out.file}  ${(out.bytes / 1024).toFixed(0)} KiB`);
        } catch (e) {
          // What the page looked like when it went wrong, kept out of img/ so
          // it can never be mistaken for a capture.
          const debugDir = path.join(WORK, 'failed');
          fs.mkdirSync(debugDir, {recursive: true});
          await ctx?.pages()[0]?.screenshot({path: path.join(debugDir, `${s.file}.png`)}).catch(() => {});
          out.failed = scrub(e.message.split('\n')[0]);
          say(`[${f.id}] ${s.file} FAILED: ${out.failed}`);
          failed++;
        } finally {
          await ctx?.close();
        }
      }
    }
  } finally {
    await br?.close();
    for (const srv of servers.values()) await H.stop(srv.child);
    fs.rmSync(scratch, {recursive: true, force: true});
  }

  // The facts each section consumed, written down BESIDE that section. This is
  // the run's last act over the numbers, so every fixture and every
  // section-owned `facts` file has been read by now.
  for (const f of features) record(manifest, f, view(facts, f));

  // `--only` refreshes part of an existing capture rather than replacing it.
  const manifestPath = path.join(opt.out, 'capture.json');
  if (opt.only && fs.existsSync(manifestPath)) {
    const prior = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
    Object.assign(manifest, mergePartial(prior, manifest, opt.only));
  }
  prune(manifest);
  // Written to a temporary file and renamed. capture.json is the page's only
  // account of what exists in img/, and img/ is emptied at the start of a run:
  // a half-written or stale manifest beside a pruned img/ is what let a page
  // go out claiming 46 captures with every image gone (#212 B2).
  const tmp = `${manifestPath}.tmp-${process.pid}`;
  fs.writeFileSync(tmp, `${JSON.stringify(manifest, null, 1)}\n`);
  fs.renameSync(tmp, manifestPath);
  const shots = Object.values(manifest.features).flatMap(r => r.shots);
  const n = (c, one, many) => `${c} ${c === 1 ? one : many}`;
  say(`${n(shots.filter(s => s.file).length, 'screenshot', 'screenshots')}, ${shots.filter(s => s.notCaptured).length} not captured on this host, ${n(failed, 'failure', 'failures')}`);
  say(`wrote ${path.relative(ROOT, manifestPath)}`);
  process.exit(failed ? 1 : 0);
}

/// One interaction. Selectors are Playwright selectors, so `text=` and
/// `:has-text()` work; `{fn:NAME}` resolves to that function's id.
async function step(page, st, lookup) {
  const sel = v => page.locator(lookup(v)).nth(st.nth || 0);
  const timeout = 10_000;
  if (st.click !== undefined) await sel(st.click).click({timeout});
  else if (st.hover !== undefined) await sel(st.hover).hover({timeout, ...(st.at ? {position: {x: st.at[0], y: st.at[1]}} : {})});
  else if (st.fill !== undefined) await sel(st.fill).fill(st.value, {timeout});
  else if (st.select !== undefined) await sel(st.select).selectOption(st.value, {timeout});
  else if (st.press !== undefined) await page.keyboard.press(st.press);
  else if (st.wait !== undefined) await sel(st.wait).waitFor({state: 'visible', timeout});
  else if (st.wait_gone !== undefined) await sel(st.wait_gone).waitFor({state: 'detached', timeout});
  else if (st.pause !== undefined) await new Promise(r => setTimeout(r, st.pause));
  else if (st.move !== undefined) await page.mouse.move(st.move[0], st.move[1]);
  else if (st.drag !== undefined) {
    const [x0, y0, x1, y1] = st.drag;
    await page.mouse.move(x0, y0);
    await page.mouse.down();
    await page.mouse.move(x1, y1, {steps: 8});
    await page.mouse.up();
    // a drag that starts over text selects it; the selection is not part of the view
    await page.evaluate(() => window.getSelection()?.removeAllRanges());
  }
  else if (st.wheel !== undefined) await page.mouse.wheel(0, st.wheel);
  else if (st.scroll !== undefined) {
    await sel(st.scroll).evaluate((el, block) => el.scrollIntoView({block, behavior: 'instant'}), st.block || 'start');
  }
}

/// Where the screenshot will be taken: the union of the `clip` selectors, or
/// null for the whole viewport. Computed before the assertion, because the
/// assertion is made over this rectangle.
async function clipOf(page, s) {
  if (!s.clip) return null;
  const boxes = [];
  for (const c of [].concat(s.clip)) {
    const b = await page.locator(c).first().boundingBox();
    if (!b) throw new Error(`clip selector ${c} is not on the page`);
    boxes.push(b);
  }
  const vp = page.viewportSize();
  const pad = s.pad ?? 8;
  const x0 = Math.max(0, Math.min(...boxes.map(b => b.x)) - pad);
  const y0 = Math.max(0, Math.min(...boxes.map(b => b.y)) - pad);
  const x1 = Math.min(vp.width, Math.max(...boxes.map(b => b.x + b.width)) + pad);
  const y1 = Math.min(vp.height, Math.max(...boxes.map(b => b.y + b.height)) + pad);
  return {x: x0, y: y0, width: x1 - x0, height: y1 - y0};
}

/// Is `want` drawn inside `box` -- the crop that will be published, or the
/// viewport when there is no crop?
///
/// The INNERMOST elements whose own rendered text matches are found first
/// (innerText, so `<span><i>2</i>R</span>` reads "2R" as a reader sees it and
/// not as two text nodes), and the answer is whether any of them is PAINTED
/// inside the box.
///
/// "Painted" is not `getBoundingClientRect()` alone. A rectangle can sit
/// inside the viewport and be drawn nowhere, because an ancestor clips it:
/// the viewer is built of independently scrolling panes -- source, object,
/// call graph -- and text scrolled out of a pane keeps a rect at its
/// scrolled-to position while the pane paints nothing there. Asserting
/// against the outer box alone accepted text absent from every published
/// pixel: in a 20px-high `overflow: hidden` pane with the wanted text 60px
/// below its top, the assertion passed over a blank screenshot (#212 review,
/// finding 2). So the element's rectangle is cut down by every ancestor that
/// clips it, and by `visibility`/`display`, before it is compared.
///
/// Where a phrase is split across sibling elements the innermost match is
/// their common parent, and the check is then over that parent's box: weaker
/// than a per-character test, and still bounded by what was captured.
function shows(page, box, want) {
  return page.evaluate(({b, src}) => {
    const r = b || {x: 0, y: 0, width: window.innerWidth, height: window.innerHeight};
    // `overflow: visible` is the only value that does not clip. `clip` and
    // `overlay` are the two the shorthand list is usually missing.
    const CLIPS = v => v !== 'visible';
    // An element is its descendants' containing block when it is positioned,
    // and for `position: fixed` also when it is transformed, filtered or has
    // a perspective. Only a containing block clips an out-of-flow descendant.
    const transformish = cs => cs.transform !== 'none' || cs.filter !== 'none' || cs.perspective !== 'none' || cs.willChange.split(/,\s*/).includes('transform');

    /// The part of `el` that is actually painted, or null when none of it is.
    const painted = el => {
      const cs0 = getComputedStyle(el);
      if (cs0.visibility !== 'visible') return null;
      const q = el.getBoundingClientRect();
      const box2 = {left: q.left, top: q.top, right: q.right, bottom: q.bottom};
      if (box2.right <= box2.left || box2.bottom <= box2.top) return null;
      // `node` is the element whose `position` decides who may clip next: it
      // moves up the chain each time an ancestor is entitled to clip it.
      let pos = cs0.position;
      for (let p = el.parentElement; p; p = p.parentElement) {
        const cs = getComputedStyle(p);
        if (cs.display === 'none') return null;
        const isCB = cs.position !== 'static' || transformish(cs);
        const clips = pos === 'fixed' ? transformish(cs) : pos === 'absolute' ? isCB : true;
        if (!clips) continue;
        if (CLIPS(cs.overflowX) || CLIPS(cs.overflowY)) {
          const c = p.getBoundingClientRect();
          if (CLIPS(cs.overflowX)) { box2.left = Math.max(box2.left, c.left); box2.right = Math.min(box2.right, c.right); }
          if (CLIPS(cs.overflowY)) { box2.top = Math.max(box2.top, c.top); box2.bottom = Math.min(box2.bottom, c.bottom); }
          if (box2.right <= box2.left || box2.bottom <= box2.top) return null;
        }
        // From here up it is `p`'s own position that decides who may clip.
        pos = cs.position;
      }
      return box2;
    };

    const rx = new RegExp(src, 'i');
    const textOf = el => (typeof el.innerText === 'string' ? el.innerText : el.textContent);
    const norm = s => (s || '').replace(/\s+/g, ' ');
    const all = [...document.querySelectorAll('body *')].filter(el => rx.test(norm(textOf(el))));
    const set = new Set(all);
    const innermost = all.filter(el => ![...el.children].some(c => set.has(c)));
    return innermost.some(el => {
      const q = painted(el);
      return !!q && q.right > r.x && q.left < r.x + r.width && q.bottom > r.y && q.top < r.y + r.height;
    });
  }, {b: box, src: wantPattern(want)});
}

/// The screenshot: the whole viewport, or the clip computed above.
/// PNG first; a shot over the budget is retaken as JPEG, which is what keeps
/// a dense graph under ~250 KB without shrinking it.
async function shoot(page, s, imgDir, clip) {
  const base = {animations: 'disabled', caret: 'hide', ...(clip ? {clip} : {})};
  const png = path.join(imgDir, `${s.file}.png`);
  await page.screenshot({...base, path: png, type: 'png'});
  const size = clip ? {width: Math.round(clip.width), height: Math.round(clip.height)} : page.viewportSize();
  if (fs.statSync(png).size <= IMAGE_BUDGET) return {file: `${s.file}.png`, ...size};
  fs.rmSync(png);
  for (const quality of [88, 78, 68]) {
    const jpg = path.join(imgDir, `${s.file}.jpg`);
    await page.screenshot({...base, path: jpg, type: 'jpeg', quality});
    if (fs.statSync(jpg).size <= IMAGE_BUDGET || quality === 68) break;
  }
  return {file: `${s.file}.jpg`, ...size};
}

// Exit status, and showcase.sh depends on the distinction:
//   0  the run finished, nothing drifted
//   1  the run FINISHED and capture.json was written; a claim drifted
//   2  the run did NOT finish -- a missing prerequisite, or it died part-way.
//      img/ has been pruned and no capture.json was written, so the page must
//      not be rebuilt from what is on disk (#212 B2).
if (require.main === module) {
  main().catch(e => {
    console.error(e);
    say('the capture run did not finish; capture.json was not written and docs/showcase/img/ is incomplete');
    process.exit(2);
  });
}

// The pieces the tests drive directly: the screenshot assertion, which needs a
// real browser and a real clipping ancestor to mean anything, and the `--only`
// merge, which is where a section's numbers and its picture came apart.
module.exports = {shows, clipOf, view, record, mergePartial, prune};
