// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Shared plumbing for the browser checks (#69).
//
// Everything the old QA scripts hardcoded is derived here instead: the
// repository root from this file's location, the browser from Playwright's
// own download, the port from the kernel, and every writable path from a
// private temp directory. That is what lets these run on a CI runner and on
// a developer's machine without editing them.

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const {spawn, spawnSync} = require('node:child_process');

const ROOT = path.resolve(__dirname, '../..');
const BIN = process.env.BUTTERFLY_BIN || path.join(ROOT, 'target/debug/butterfly');

/// A port the kernel says is free, asked for at the moment it is needed.
/// The old scripts hardcoded 8792/8793/8794, so two suites could not run at
/// once and a developer's own server on that port broke them.
function freePort() {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.on('error', reject);
    s.listen(0, '127.0.0.1', () => {
      const {port} = s.address();
      s.close(() => resolve(port));
    });
  });
}

/// A private directory for this run's fixtures and artifacts. Never inside
/// the repository: a writable test that puts a resolutions sidecar in the
/// work tree can overwrite a real one.
function scratch(tag) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `butterfly-browser-${tag}-`));
}

/// Ingest one example into a fresh evidence file, with its requirements
/// sidecar when the example has an `llr.yaml`.
function ingest(example, dir) {
  const src = path.join(ROOT, 'examples', example);
  if (!fs.existsSync(path.join(src, 'build/coverage.json'))) {
    return {skip: `examples/${example} is not built (run examples/${example}/build.sh)`};
  }
  const evidence = path.join(dir, 'evidence.json');
  const args = ['ingest', path.join(src, 'build'), '--source-root', src, '-o', evidence];
  const tests = path.join(src, 'build/tests');
  if (fs.existsSync(tests)) args.push('--tests', tests);
  let r = spawnSync(BIN, args, {encoding: 'utf8'});
  if (r.status !== 0) throw new Error(`ingest failed: ${r.stderr || r.stdout}`);

  const llr = path.join(src, 'llr.yaml');
  if (fs.existsSync(llr)) {
    r = spawnSync(BIN, ['requirements', llr, '-o', path.join(dir, 'requirements.json')], {encoding: 'utf8'});
    if (r.status !== 0) throw new Error(`requirements failed: ${r.stderr || r.stdout}`);
  }
  return {evidence};
}

/// Start `butterfly serve` and wait for it to answer a request.
///
/// Readiness is an answered request, not a TCP connect: `connect` succeeds
/// as soon as the listener is bound and says nothing about the server being
/// able to answer. The server's stderr is kept, so a startup failure is
/// reported rather than surfacing later as a puzzling assertion.
/// `cwd` is the directory the server process runs in. It matters because the
/// viewer prints paths in the Gates box's "equivalent command" relative to
/// the working directory (crates/view `for_display`, #214), so a check of
/// that behaviour has to be able to say what the working directory is. Left
/// out, the server inherits this process's, which is the repository root.
async function serve(evidence, {readOnly = false, resolutions, dir, out, cwd} = {}) {
  // One path or several: a multi-variant server is the same command with
  // more evidence files, and the variant checks need exactly that.
  const paths = Array.isArray(evidence) ? evidence : [evidence];
  const port = await freePort();
  // The log goes in the artifact directory, not the scratch directory. CI
  // uploads only the former, so a server log written beside the fixtures was
  // lost with the runner -- exactly when a red job needed it most (review of
  // #69). `out` is optional so a caller can still keep one elsewhere.
  const logPath = path.join(out || dir, `serve-${port}.log`);
  const log = fs.openSync(logPath, 'a');
  const args = ['serve', ...paths, '--listen', `127.0.0.1:${port}`];
  if (readOnly) args.push('--read-only');
  // Always an explicit sidecar for a writable server. The default is the
  // relative name `resolutions.json`, resolved against the process's cwd --
  // so a review saved by a test could land in the developer's work tree.
  args.push('--resolutions', resolutions || path.join(dir, 'resolutions.json'));

  const child = spawn(BIN, args, {stdio: ['ignore', log, log], ...(cwd ? {cwd} : {})});
  const url = `http://127.0.0.1:${port}`;
  const deadline = Date.now() + 30_000;
  let last = 'no attempt completed';
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`serve exited during startup: ${child.exitCode}\n${fs.readFileSync(logPath, 'utf8')}`);
    }
    try {
      const r = await fetch(`${url}/api/variants`);
      if (r.ok) return {url, port, child, logPath};
    } catch (e) {
      last = e.message;
    }
    await new Promise(r => setTimeout(r, 100));
  }
  child.kill();
  throw new Error(`serve never answered: ${last}\n${fs.readFileSync(logPath, 'utf8')}`);
}

/// Start `butterfly serve` with a bearer token, on the port the kernel
/// assigns to the server itself (`--listen 127.0.0.1:0`), read back from
/// its `butterfly: http://HOST:PORT/` line. Nothing binds a port and lets
/// it go first -- that is the race #91 removed from the Rust suites. `port`
/// is only for restarting on a port this server already had, which is what
/// a reader's open tab sees when the operator restarts with a new token.
///
/// The token goes in a mode-600 file, never in argv.
async function serveWithToken(evidence, {token, port = 0, dir, out, tag = 'token'} = {}) {
  const tokenFile = path.join(dir, `${tag}-${Date.now()}.token`);
  fs.writeFileSync(tokenFile, `${token}\n`, {mode: 0o600});
  const logPath = path.join(out || dir, `serve-${tag}-${Date.now()}.log`);
  const args = ['serve', evidence, '--listen', `127.0.0.1:${port}`, '--token-file', tokenFile,
    '--resolutions', path.join(dir, `resolutions-${tag}.json`)];
  const child = spawn(BIN, args, {stdio: ['ignore', 'ignore', 'pipe']});
  let log = '';
  child.stderr.on('data', d => { log += d; fs.appendFileSync(logPath, d); });

  const deadline = Date.now() + 30_000;
  let url = null;
  while (!url && Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`serve exited during startup: ${child.exitCode}\n${log}`);
    const m = /^butterfly: http:\/\/([^/\s]+)\/$/m.exec(log);
    if (m) url = `http://${m[1]}`;
    else await new Promise(r => setTimeout(r, 50));
  }
  if (!url) { child.kill(); throw new Error(`serve never printed its URL line\n${log}`); }
  // The URL line is printed after the bind; readiness is an answered request.
  let last = 'no attempt completed';
  while (Date.now() < deadline) {
    try {
      const r = await fetch(`${url}/api/variants`, {headers: {authorization: `Bearer ${token}`}});
      if (r.ok) return {url, port: +new URL(url).port, child, logPath};
      last = `status ${r.status}`;
    } catch (e) {
      last = e.message;
    }
    await new Promise(r => setTimeout(r, 100));
  }
  child.kill();
  throw new Error(`serve never answered: ${last}\n${log}`);
}

/// Stop a server and wait until it has gone, so its port can be bound again.
function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise(resolve => { child.once('exit', resolve); child.kill(); });
}

/// Playwright's own Chromium. The old scripts pointed at
/// `/Applications/Google Chrome.app`, which is why they only ran on one
/// machine; the pinned download is the same build everywhere.
async function browser() {
  const {chromium} = require('playwright');
  return chromium.launch({headless: true});
}

/// Poll a page-side expression from Node until it holds.
///
/// Not `page.waitForFunction`: the viewer serves a strict
/// `script-src 'self'` with no `unsafe-eval` (crates/view/src/lib.rs), and
/// Playwright's waiter evaluates its polling wrapper as a string, which that
/// policy correctly refuses. Polling with `page.evaluate` goes through the
/// CDP call path instead, so the checks do not require the product to weaken
/// its own CSP in order to be testable.
async function waitFor(page, fn, arg, {timeout = 10_000, what = 'condition'} = {}) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    last = await page.evaluate(fn, arg);
    if (last) return last;
    await new Promise(r => setTimeout(r, 50));
  }
  throw new Error(`timed out waiting for ${what} (last saw ${JSON.stringify(last)})`);
}

module.exports = {ROOT, BIN, freePort, scratch, ingest, serve, serveWithToken, stop, browser, waitFor};
