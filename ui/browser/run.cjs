// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The documented entry point for the viewer's real-browser checks (#69).
//
//   cd ui/browser && npm ci && npx playwright install chromium && npm test
//
// Exit codes: 0 all checks passed, 1 a check failed, 2 the harness could not
// run (Playwright absent, a fixture missing, the binary not built). A skip is
// never silent and never exit 0 pretending otherwise -- the caller decides
// whether an unrunnable suite is acceptable, which is the mistake #89 was
// about.

const fs = require('node:fs');
const path = require('node:path');
const {ROOT, BIN, scratch} = require('./harness.cjs');

const SUITES = {
  requirements: require('./checks/requirements.cjs'),
  review: require('./checks/review.cjs'),
  variants: require('./checks/variants.cjs'),
  vars: require('./checks/vars.cjs'),
  token: require('./checks/token.cjs'),
  footer: require('./checks/footer.cjs'),
  // The call graph's node labels and the Gates command's paths (#214): both
  // are properties of the rendered page, so neither is visible to
  // ui/callgraph.test.cjs, which drives the model with no layout.
  calls: require('./checks/calls.cjs'),
  // The showcase capture's screenshot assertion (#154). It needs a real
  // layout engine and nothing else -- no fixture, no binary.
  showcase: require('./checks/showcase.cjs'),
};

function preflight() {
  if (!fs.existsSync(BIN)) return `${path.relative(ROOT, BIN)} is not built (cargo build --locked)`;
  try {
    require.resolve('playwright');
  } catch {
    return 'playwright is not installed (cd ui/browser && npm ci && npx playwright install chromium)';
  }
  return null;
}

(async () => {
  const reason = preflight();
  if (reason) {
    console.error(`cannot run: ${reason}`);
    process.exitCode = 2;
    return;
  }

  const only = process.argv.slice(2).filter(a => !a.startsWith('-'));
  const names = only.length ? only : Object.keys(SUITES);
  // An externally supplied directory need not exist yet -- ci.yml names one
  // under $RUNNER_TEMP and ci-local.sh one inside its own work directory.
  // `scratch` creates what it returns; this has to create what it is given,
  // or every check can pass and the run still dies writing its results.
  const out = process.env.BROWSER_CHECK_OUT || scratch('artifacts');
  fs.mkdirSync(out, {recursive: true});
  console.log(`artifacts: ${out}`);

  const results = [];
  for (const name of names) {
    const suite = SUITES[name];
    if (!suite) {
      console.error(`no such suite: ${name}`);
      process.exitCode = 2;
      return;
    }
    const r = await suite.run({out});
    results.push(...r.map(x => ({suite: name, ...x})));
    for (const x of r) {
      const mark = x.pass === true ? 'pass' : x.skip ? 'SKIP' : 'FAIL';
      console.log(`${mark}  ${name}/${x.name}${x.skip ? ` -- ${x.skip}` : ''}${x.error ? ` -- ${x.error}` : ''}`);
    }
  }
  fs.writeFileSync(path.join(out, 'results.json'), JSON.stringify(results, null, 2));

  const failed = results.filter(r => r.pass === false && !r.skip);
  const skipped = results.filter(r => r.skip);
  console.log(`\n${results.length - failed.length - skipped.length} passed, ${failed.length} failed, ${skipped.length} skipped`);
  if (failed.length) {
    console.log(`\nfailure artifacts (screenshots, server logs) are under ${out}`);
    process.exitCode = 1;
  }
  if (skipped.length) {
    // A suite that was asked for and could not run is exit 2, not success.
    // Reporting it and exiting 0 made a skipped suite indistinguishable from
    // a passing one to anything reading the status -- the hosted step uses
    // this exit code directly, and `step` in ci-local.sh marks it PASS. An
    // opt-out belongs at the caller, which knows whether the fixture is
    // meant to exist on this host; the runner only knows it was asked for a
    // suite it could not run (review of #69).
    for (const s of skipped) console.log(`skipped: ${s.suite}/${s.name} -- ${s.skip}`);
    console.log(`\n${skipped.length} requested suite(s) could not run; this is not a pass`);
    if (!failed.length) process.exitCode = 2;
  }
})().catch(e => {
  console.error(e);
  process.exitCode = 1;
});
