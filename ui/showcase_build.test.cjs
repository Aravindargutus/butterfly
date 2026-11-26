// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// Run with: node --test ui/showcase_build.test.cjs
//
// What the showcase page builder must refuse to publish (#154, review of
// #212). The page's argument is that nothing on it is taken on trust, so the
// failures that matter here are the ones that publish something FALSE while
// exiting 0:
//
//   * a screenshot shown under another screenshot's caption, because the two
//     were paired to their capture by array index;
//   * a command's text printed beside another command's output, for the same
//     reason;
//   * an <img> tag for a file that is not on disk, or that is not the file
//     the capture recorded, after a capture run died half way through and
//     left the previous capture.json beside an emptied img/;
//   * a "captured" chip over a section that gained a screenshot nobody ever
//     took.
//
// Lives beside the viewer tests because that is the glob CI and ci-local run.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {render} = require('../scripts/showcase/build.cjs');
const {CAPTURE_SCHEMA} = require('../scripts/showcase/spec.cjs');
const {REPO, HISTORICAL_REPO} = require('../scripts/showcase/markup.cjs');

/// A directory holding `img/` with one PNG per (name, bytes) pair.
function imgDir(files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'showcase-build-'));
  fs.mkdirSync(path.join(dir, 'img'));
  for (const [name, bytes] of Object.entries(files)) fs.writeFileSync(path.join(dir, 'img', name), Buffer.alloc(bytes, 7));
  return dir;
}

const META = {
  commit: 'a'.repeat(40), commitShort: 'aaaaaaaaaaaa', commitDate: '2026-09-19T00:00:00Z',
  branch: 'feature/154-showcase-generator', modified: 0, capturedAt: '2026-09-19T00:00:00Z',
  os: 'Darwin 25.0.0', arch: 'arm64', tools: {butterfly: 'butterfly 0.1.0'},
};

/// One fixture, one section, two screenshots and two commands.
function spec({shots = ['alpha', 'beta'], cmds = ['run-alpha', 'run-beta']} = {}) {
  return {
    fixtures: {demo: {dir: 'examples/c', ci: true, build: ['./examples/c/build.sh'], evidence: 'examples/c/build/evidence.json'}},
    groups: [{id: 'g', title: 'A group'}],
    notClaimed: [],
    features: [{
      id: 'V01', issue: 1, group: 'g', title: 'A section', status: 'captured',
      claim: 'A claim.', limits: ['A limit.'], fixtures: ['demo'],
      shot: shots.map(n => ({file: n, route: '#/overview', caption: `The caption of ${n}, long enough to stand as alt text.`})),
      cmd: cmds.map(n => ({run: `butterfly ${n}`, exit: 0, show: true, caption: `The caption of ${n}.`})),
    }],
  };
}

// The section's numbers are recorded beside the section, not in one object
// shared by the page: see ui/showcase_capture.test.cjs for why. This spec's
// claims quote none, so the snapshot is empty -- but it must be THERE, or the
// builder cannot tell "no numbers" from "we lost which run's numbers".
// META is COPIED, not shared: a test that sets `c.meta.partial` used to leave
// it set for every test that ran after it.
function cap({shots, cmds}) {
  return {schema: CAPTURE_SCHEMA, meta: {...META}, fixtures: {demo: {ok: true}}, factSets: {}, features: {V01: {commands: cmds, shots, facts: {}}}};
}

const CAPTURED = {
  shots: [
    {file: 'alpha.png', source: 'alpha', bytes: 11, width: 10, height: 10, url: 'http://127.0.0.1:8787/#/overview'},
    {file: 'beta.png', source: 'beta', bytes: 22, width: 10, height: 10, url: 'http://127.0.0.1:8787/#/overview'},
  ],
  cmds: [
    {run: 'butterfly run-alpha', exit: 0, expected: 0, output: 'OUTPUT-OF-ALPHA\n'},
    {run: 'butterfly run-beta', exit: 0, expected: 0, output: 'OUTPUT-OF-BETA\n'},
  ],
};
const FILES = {'alpha.png': 11, 'beta.png': 22};
const build = (s, c, dir) => render({spec: s, cap: c, matrix: null, inline: false, dir});

/// The caption an <img> was published under, by image file name.
function captionOf(html, file) {
  const m = new RegExp(`<img src="img/${file}"[^>]*alt="([^"]*)"`).exec(html);
  return m && m[1];
}
/// The command text printed above a given output.
function commandAbove(html, output) {
  const m = new RegExp(`<span class="cmd">([^<]*)</span>\\n${output}`).exec(html);
  return m && m[1];
}

test('a screenshot is published under its own caption after the entries are reordered', () => {
  const dir = imgDir(FILES);
  // The same capture, read by a features.toml whose two shots have been
  // swapped. Paired by index this published beta.png under alpha's caption,
  // with the chip still reading "captured" and the build exiting 0.
  const html = build(spec({shots: ['beta', 'alpha']}), cap(CAPTURED), dir);
  assert.match(captionOf(html, 'alpha.png'), /The caption of alpha/);
  assert.match(captionOf(html, 'beta.png'), /The caption of beta/);
});

test('a command is printed beside the output it actually produced, after a reorder', () => {
  const dir = imgDir(FILES);
  const html = build(spec({cmds: ['run-beta', 'run-alpha']}), cap(CAPTURED), dir);
  assert.equal(commandAbove(html, 'OUTPUT-OF-ALPHA'), 'butterfly run-alpha');
  assert.equal(commandAbove(html, 'OUTPUT-OF-BETA'), 'butterfly run-beta');
});

test('a capture that does not record what each entry is, is refused', () => {
  const dir = imgDir(FILES);
  const noSource = {shots: CAPTURED.shots.map(({source, ...rest}) => rest), cmds: CAPTURED.cmds};
  assert.throws(() => build(spec(), cap(noSource), dir), /predates identity matching/);
  const noRun = {shots: CAPTURED.shots, cmds: CAPTURED.cmds.map(({run, ...rest}) => rest)};
  assert.throws(() => build(spec(), cap(noRun), dir), /predates identity matching/);
});

test('a screenshot nobody took is said to be missing, and the chip stops saying "captured"', () => {
  const dir = imgDir(FILES);
  // features.toml gained a third shot; the capture was not re-run.
  const html = build(spec({shots: ['alpha', 'beta', 'gamma']}), cap(CAPTURED), dir);
  assert.match(html, /Not part of this capture run:/);
  assert.match(html, /captured in part · 1 item not captured/);
  assert.doesNotMatch(html, /<span class="chip captured">captured<\/span>/);
  // ...and it is counted in the feature map, which used to count only sections
  assert.match(html, /1 screenshot or command output was not captured/);
});

test('an image named by the capture and not on disk stops the build', () => {
  const dir = imgDir({'alpha.png': 11});   // beta.png was never written
  assert.throws(() => build(spec(), cap(CAPTURED), dir), /is not on disk: the capture run did not finish/);
});

test('an image that is not the one the capture recorded stops the build', () => {
  // the size capture.json recorded is the check, not the timestamp: a fresh
  // clone gives every file the same mtime
  const dir = imgDir({'alpha.png': 11, 'beta.png': 999});
  assert.throws(() => build(spec(), cap(CAPTURED), dir), /capture\.json recorded 22/);
});

test('a section captured at another commit says so, and the lede stops claiming one commit', () => {
  const dir = imgDir(FILES);
  const c = cap(CAPTURED);
  c.features.V01.commit = 'b'.repeat(40);
  c.meta.partial = ['V02'];
  const html = build(spec(), c, dir);
  assert.match(html, /captured at <a href="[^"]*bbbbbbbbbbbb"/);
  assert.match(html, /not all of it at the same commit/);
  assert.match(html, /<b>Partial capture\.<\/b>/);
  assert.doesNotMatch(html, /Everything below was captured by a script from commit/);
});

test('the reproduce block creates the scratch directory the commands write into', () => {
  const dir = imgDir(FILES);
  const s = spec();
  s.features[0].cmd[0].run = 'butterfly report evidence.json -o target/showcase/r.html';
  const html = build(s, cap({shots: CAPTURED.shots, cmds: [{...CAPTURED.cmds[0], run: s.features[0].cmd[0].run}, CAPTURED.cmds[1]]}), dir);
  const repro = /<div class="repro">[\s\S]*?<\/div>/.exec(html)[0];
  const at = repro.indexOf('mkdir -p target/showcase');
  assert.ok(at > 0, 'the harness step is printed in the reproduce block');
  assert.ok(at < repro.indexOf('butterfly report evidence.json'), 'and before the command that needs it');
  // a section that writes nowhere does not get the line
  assert.doesNotMatch(build(spec(), cap(CAPTURED), dir), /mkdir -p target\/showcase/);
});

test('an evidence string interpolated into a claim is not published with "(s)"', () => {
  // The rule is build.cjs's own (`No "(s)": the page is read by people who
  // audit for a living`), and it used to hold only for the counts the builder
  // works out itself. A `{{…stack.reason}}` is a sentence the ingest crates
  // wrote for a terminal, and V07's claim went out reading "because of 1
  // indirect call(s)" (#219).
  const dir = imgDir(FILES);
  const s = spec();
  s.features[0].claim = 'Not bounded because of {{demo.reason}}.';
  s.features[0].limits = ['And {{demo.many}} besides.'];
  s.features[0].shot[0].caption = 'A shot of {{demo.reason}}, long enough to stand as alt text.';
  const c = cap(CAPTURED);
  c.factSets = {f1: {reason: '1 indirect call(s): callee set unknown', many: '3 unresolved gap(s)'}};
  c.features.V01.facts = {demo: 'f1'};

  const html = build(s, c, dir);
  assert.match(html, /because of 1 indirect call: callee set unknown/, 'a count of one loses the suffix');
  assert.match(html, /And 3 unresolved gaps besides/, 'and a count above one takes it');
  assert.match(captionOf(html, 'alpha.png'), /1 indirect call:/, 'alt text is held to it too');
  assert.doesNotMatch(html, /\(s\)/);

  // Verbatim command output is not prose and is left exactly as the tool
  // printed it -- rewriting what a terminal block shows would be a lie about
  // what the command said.
  const verbatim = build(spec(), cap({shots: CAPTURED.shots, cmds: [{...CAPTURED.cmds[0], output: 'FAIL     3 breach(es)\n'}, CAPTURED.cmds[1]]}), dir);
  assert.match(verbatim, /FAIL     3 breach\(es\)/);
});

test('a "(s)" the builder cannot count stops the build rather than reaching the page', () => {
  const dir = imgDir(FILES);
  const s = spec();
  s.features[0].claim = 'It reads the document key(s).';
  assert.throws(() => build(s, cap(CAPTURED), dir), /cannot resolve into a count/);
});

/// Render `text` as V01's claim and return the page, or throw as the build would.
function claimed(text) {
  const dir = imgDir(FILES);
  const s = spec();
  s.features[0].claim = text;
  return build(s, cap(CAPTURED), dir);
}
/// The claim as it was published, with the surrounding markup stripped.
function claimText(text) {
  const m = /<div class="claim"><p>([\s\S]*?)<\/p>/.exec(claimed(text));
  return m && m[1];
}

// The headline cases above pin one count of one and one count above one, and
// nothing else -- which is how the first cut of `counted()` shipped picking a
// number out of `v1`, `0x1`, `A-7` and `0.1` and nobody noticed (#220
// self-review). These are the boundaries.
test('counted(): the boundaries of the count itself', () => {
  // Zero takes the plural in English, and is not "no suffix".
  assert.equal(claimText('It found 0 gap(s).'), 'It found 0 gaps.');
  assert.equal(claimText('It found 1 gap(s).'), 'It found 1 gap.');
  assert.equal(claimText('It found 2 gap(s).'), 'It found 2 gaps.');
  assert.equal(claimText('It found 12 gap(s).'), 'It found 12 gaps.');
  // A grouped thousand is one number, not a "1" followed by "000". This came
  // out right before only because the regex captured the `000`.
  assert.equal(claimText('It found 1,000 gap(s).'), 'It found 1,000 gaps.');
  assert.equal(claimText('It found 2,001 gap(s).'), 'It found 2,001 gaps.');
  // "(es)" is rewritten as well as "(s)".
  assert.equal(claimText('line 12: 3 conditional branch(es) here.'), 'line 12: 3 conditional branches here.');
  assert.equal(claimText('line 12: 1 conditional branch(es) here.'), 'line 12: 1 conditional branch here.');
});

test('counted(): the three-word window, and the fourth word that stops the build', () => {
  assert.equal(claimText('It found 2 call(s).'), 'It found 2 calls.');
  assert.equal(claimText('It found 2 indirect call(s).'), 'It found 2 indirect calls.');
  assert.equal(claimText('It found 2 very indirect call(s).'), 'It found 2 very indirect calls.');
  // Hyphenated words are one word, so this is still inside the window.
  assert.equal(claimText('It found 2 non-executable source line(s).'), 'It found 2 non-executable source lines.');
  // Four words: the number is too far away to be the count of the noun, so
  // the build stops rather than assuming it is.
  assert.throws(() => claimed('It found 2 very very indirect call(s).'), /cannot resolve into a count/);
  // ...and anything that breaks the run of plain words breaks the window too.
  assert.throws(() => claimed('It found 2 of 3 MC/DC condition(s).'), /cannot resolve into a count/);
  assert.throws(() => claimed('It found 4 file(s)/function(s) changed.'), /cannot resolve into a count/);
});

// Each of these published a FALSE COUNT at exit 0 before the number was
// anchored and its value parsed. The first is the one that mattered: a
// sentence that said "however many there are" went out as "exactly one".
test('counted(): a digit that is not a count stops the build instead of becoming one', () => {
  assert.throws(() => claimed('the ELF v1 program header(s) could not be read'), /cannot resolve into a count/);
  assert.throws(() => claimed('It took 0.1 second(s).'), /cannot resolve into a count/);
  assert.throws(() => claimed('It took 1.5 second(s).'), /cannot resolve into a count/);
  assert.throws(() => claimed('It found 0x1 call(s).'), /cannot resolve into a count/);
  assert.throws(() => claimed('DO-178C table A-7 objective(s).'), /cannot resolve into a count/);
});

// Not a fixed case, and pinned here so it is not mistaken for one. A number
// that is not a count but sits exactly where a count would sit is
// indistinguishable from a count by any rule local to the sentence: "1 has
// gap(s)" has the same shape as "1 indirect call(s)". No string any crate
// writes has it -- gaps.rs:239 writes "line {line}: {} conditional branch(es)",
// and the colon keeps the line number out of the window (asserted above).
test('counted(): the residual it does NOT fix, recorded rather than claimed', () => {
  assert.equal(claimText('On line 1 has gap(s).'), 'On line 1 has gap.');
});

// `counted()` is deliberately narrower than the guard: rewriting an irregular
// plural means guessing at it. Refusing to publish one does not, and before
// this both of these went out verbatim at exit 0 -- `entr(ies)` is a live
// string in crates/cli/src/main.rs (#220 self-review).
test('an irregular or shouted "(s)" the rewriter cannot touch still stops the build', () => {
  assert.throws(() => claimed('skipped 3 entr(ies) of kind llr'), /cannot resolve into a count/);
  assert.throws(() => claimed('2 FILE(S) READ'), /cannot resolve into a count/);
  assert.throws(() => claimed('1 entr(ies) of kind llr'), /cannot resolve into a count/);
});

test('the refusal names the field and the placeholder, and says who can fix it', () => {
  const dir = imgDir(FILES);
  // From an evidence string: nobody can reword this in support/features.toml,
  // so the advice must not tell them to.
  const s = spec();
  s.features[0].limits = ['A limit.', 'Stopped after {{demo.reason}}.'];
  const c = cap(CAPTURED);
  c.factSets = {f1: {reason: 'skipped 3 entr(ies)'}};
  c.features.V01.facts = {demo: 'f1'};
  assert.throws(() => build(s, c, dir), err => {
    assert.match(err.message, /^V01 limit 2:/, 'the field, not just the section');
    assert.match(err.message, /\{\{demo\.reason\}\}/, 'the placeholder that carried it in');
    assert.match(err.message, /crate that formats it/, 'advice a maintainer can act on');
    assert.doesNotMatch(err.message, /Write the sentence with a number/, 'and not the advice for a sentence this page wrote');
    return true;
  });

  // From features.toml itself: the original advice is the right one, and
  // there is no placeholder to blame.
  const s2 = spec();
  s2.features[0].shot[0].caption = 'A shot of the document key(s), long enough to stand as alt text.';
  assert.throws(() => build(s2, cap(CAPTURED), dir), err => {
    assert.match(err.message, /^V01 caption of alpha:/);
    assert.match(err.message, /as written in support\/features\.toml/);
    assert.match(err.message, /Write the sentence with a number in front of the noun/);
    return true;
  });
});

test('a placeholder the builder itself failed to fill never reaches the page', () => {
  const dir = imgDir(FILES);
  const s = spec();
  // a claim resolves through fill(); the TITLE does not, and a stray
  // placeholder there used to be published as literal braces
  s.features[0].title = 'A section with {{c.gapsTotal}} in its title';
  const html = build(s, cap(CAPTURED), dir);
  assert.match(html, /\{\{c\.gapsTotal\}\}/, 'render() itself does not fill a title');
  // ...which is what main()'s last look is for; assert the needle it uses
  assert.ok(html.includes('{{'), 'and the guard in main() is what stops it');
});

/// A spec and capture with enough of V14 for the landing table to render, and
/// `extra` sections beside it so the footer's "mixed commits" line has a count.
function landing({partial = null, demoCommit = null, demoRepo = undefined, metaRepo = undefined, extra = 0, same = true} = {}) {
  const fn = (blocks, gaps) => ({linesCovered: 7, linesTotal: 9, blocks, instructions: 30, gaps, gapKinds: {predicatedDecision: gaps}});
  const s = spec();
  s.fixtures['c-variants'] = {dir: 'examples/c', ci: true, build: ['./examples/c/build.sh'], evidence: 'examples/c/build/evidence.json'};
  const sect = id => ({
    id, issue: 1, group: 'g', title: `Section ${id}`, status: 'captured',
    claim: 'A claim.', limits: ['A limit.'], fixtures: ['c-variants'], shot: [], cmd: [],
  });
  s.features.push(sect('V14'));
  for (let i = 0; i < extra; i++) s.features.push(sect(`V${30 + i}`));

  const c = cap(CAPTURED);
  if (metaRepo !== undefined) c.meta.repo = metaRepo;
  c.fixtures['c-variants'] = {ok: true};
  c.factSets = {fv: {
    variantNames: 'O0, O1, O2, Os', sourceCoverageIdentical: same,
    variant: {O0: {fn: {clamp_altitude: fn(5, 0)}}, O2: {fn: {clamp_altitude: fn(1, 2)}}},
  }};
  for (const f of s.features.filter(x => x.id !== 'V01')) {
    c.features[f.id] = {commands: [], shots: [], facts: {'c-variants': 'fv'}};
    if (demoCommit) c.features[f.id].commit = demoCommit;
    if (demoRepo !== undefined) c.features[f.id].repo = demoRepo;
  }
  if (partial) c.meta.partial = partial;
  return build(s, c, imgDir(FILES));
}

/// The landing table's provenance sentence.
const provenance = html => /at four optimisation levels\. ([\s\S]*?)<\/p>/.exec(html)[1];

// The page uses "Not part of this capture run" everywhere else to mean the
// LAST run, and after `--only V16` mergePartial carries V14's factSet forward
// untouched -- so this sentence claimed a provenance the numbers do not have,
// on a page whose premise is provenance (#220 self-review).
test('the landing table says which run its numbers came from, after a partial capture', () => {
  assert.match(provenance(landing()), /^Read from the evidence of this page's capture run \(<a href="#V14">V14<\/a>\)\.$/,
    'a whole-page capture: the sentence as it was');
  // V14 refreshed by the partial run: still this run's evidence.
  assert.match(provenance(landing({partial: ['V14']})), /this page's capture run/);

  // V14 NOT refreshed, and from an earlier commit: name the commit and the run.
  const stale = provenance(landing({partial: ['V16'], demoCommit: 'c'.repeat(40)}));
  assert.doesNotMatch(stale, /this page's capture run/, 'it must stop claiming the run that did not produce it');
  assert.match(stale, /<code>cccccccccccc<\/code>/, 'the commit the numbers were captured at');
  assert.match(stale, /refreshed only V16/, 'and what the last run actually did');

  // Legacy V14 record (commit without repo) under a post-move page (meta.repo = REPO):
  // commit and repository must be resolved from the same record so the old commit
  // links to HISTORICAL_REPO rather than the new repository (#8 review).
  const postMoveLegacy = provenance(landing({partial: ['V16'], demoCommit: 'c'.repeat(40), metaRepo: REPO}));
  assert.ok(postMoveLegacy.includes(`href="${HISTORICAL_REPO}/commit/${'c'.repeat(40)}"`),
    `legacy V14 commit should link to HISTORICAL_REPO, got: ${postMoveLegacy}`);
  assert.ok(!postMoveLegacy.includes(`href="${REPO}/commit/${'c'.repeat(40)}"`),
    `legacy V14 commit must not pair with post-move meta.repo, got: ${postMoveLegacy}`);

  // Post-move V14 record (explicit repo = REPO) links to REPO.
  const postMoveNew = provenance(landing({partial: ['V16'], demoCommit: 'd'.repeat(40), demoRepo: REPO, metaRepo: REPO}));
  assert.ok(postMoveNew.includes(`href="${REPO}/commit/${'d'.repeat(40)}"`),
    `post-move V14 commit should link to REPO, got: ${postMoveNew}`);

  // Not refreshed but at this page's commit: no commit to name, same retreat.
  const sameCommit = provenance(landing({partial: ['V16']}));
  assert.doesNotMatch(sameCommit, /this page's capture run/);
  assert.doesNotMatch(sameCommit, /, at <a href=/, 'no commit is named when there is no difference');
  assert.match(sameCommit, /refreshed only V16/);
});

test('the qualification section states the public position without linking the internal assessment', () => {
  const html = landing();
  assert.match(html, /not qualified/);
  assert.match(html, /maintained internally and is not published in this repository/);
  assert.doesNotMatch(html, /href="[^"]*DO-330-assessment\.md"/);
});

test('the landing table\'s "coverage differs" warning names the section it sends you to', () => {
  // The name was inside a single-quoted string, so it published as the
  // literal characters "${DEMO_SECTION}" -- in the failure branch of a
  // correctness warning (#220 self-review).
  const html = landing({same: false});
  assert.match(html, /The source coverage differs between the variants/);
  assert.match(html, /read <a href="#V14">V14<\/a> before relying on this table/);
  assert.doesNotMatch(html, /\$\{DEMO_SECTION\}/);
});

// This branch had never rendered on main -- the page was a single-pass
// capture -- so nothing pinned it, and this PR is what would have published
// "23 sections were captured at an earlier commit and says so beside its
// title" (#212 B5, #220 self-review).
test('the footer\'s mixed-commit warning agrees with itself in both numbers', () => {
  const one = landing({partial: ['V16'], demoCommit: 'c'.repeat(40), extra: 0});
  assert.match(one, /1 section was captured at an earlier commit and says so beside its title/);

  const many = landing({partial: ['V16'], demoCommit: 'c'.repeat(40), extra: 2});
  assert.match(many, /3 sections were captured at an earlier commit and say so beside their titles/);
  assert.doesNotMatch(many, /sections were captured at an earlier commit and says so beside its title/);
});

// ──────────────────────────────── #218 ──────────────────────────────────────
//
// The reproduce block deduplicated its `# open …` lines by URL alone, so a
// section that photographs one route in several viewer states kept only the
// LAST shot's line -- and with it, only the last shot's state. The block is
// the page's promise that a reader can get back to what the screenshot
// shows; a line that names the wrong overlay, or that names none where one
// was seeded, breaks exactly that promise while the section still reads
// "captured" and the build still exits 0.

/// A section whose shots all open one route in the given viewer states.
/// Their capture entries carry one URL, which is the condition for the bug.
function sameUrl(seeds, {url = 'http://127.0.0.1:8787/#/overview'} = {}) {
  const names = seeds.map((_, i) => `shot${i}`);
  const s = spec({shots: names, cmds: []});
  s.features[0].shot = names.map((n, i) => ({
    file: n, route: '#/overview', seed: seeds[i],
    caption: `The caption of ${n}, long enough to stand as alt text.`,
  }));
  const shots = names.map((n, i) => ({file: `${n}.png`, source: n, bytes: 11 + i, width: 10, height: 10, url}));
  return {s, c: cap({shots, cmds: []}), files: Object.fromEntries(shots.map(x => [x.file, x.bytes]))};
}

/// The `#   open …` lines of the one section, in order.
const openLines = html =>
  (/<div class="repro">[\s\S]*?<\/div>/.exec(html)[0].match(/#   open [^<\n]*/g) || []).map(l => l.trim());

test('two shots of one route keep one reproduce line each, with the state each was taken in', () => {
  // V01's shape: the Overview twice, once with floors typed into the Gates
  // box and once with none. Keyed by URL the bare one won, so the floors
  // that produced the breach in the screenshot above it were simply gone,
  // and the section printed a line that reproduces the other shot.
  const {s, c, files} = sameUrl([
    {'sl-mode': 'projected', 'sl-gates': '{"min_lines":"80","min_outcomes":"90"}'},
    {},
  ]);
  const lines = openLines(build(s, c, imgDir(files)));
  assert.equal(lines.length, 2, `one line per state, got ${JSON.stringify(lines)}`);
  assert.match(lines[0], /\(select Projected; Gates: lines 80, outcomes 90\)$/);
  assert.equal(lines[1], '#   open http://127.0.0.1:8787/#/overview');
});

test('shots of one route in the same state still collapse to one line', () => {
  // The dedupe still has to dedupe: V09 opens its route three times and two
  // of those are the same state. Key order differs between the two tables on
  // purpose -- a seed is a mapping, not a sequence.
  const {s, c, files} = sameUrl([
    {'sl-mode': 'projected', 'sl-fnhdr': '1'},
    {'sl-fnhdr': '1', 'sl-mode': 'projected'},
    {'sl-mode': 'projected', 'sl-varmarks': '0'},
  ]);
  const lines = openLines(build(s, c, imgDir(files)));
  assert.deepEqual(lines, [
    '#   open http://127.0.0.1:8787/#/overview   (select Projected; function headers on)',
    '#   open http://127.0.0.1:8787/#/overview   (select Projected; Variable marks off)',
  ]);
});

test('every overlay a shot is seeded with is named, not only Projected', () => {
  // V28 photographs one function's Object tab under Projected and under
  // Measured. Only `projected` had a note, so the Measured line came out
  // bare -- and the viewer opens on "No coverage", so a reader following it
  // saw neither of the two overlays the section is about.
  const {s, c, files} = sameUrl([{'sl-mode': 'projected'}, {'sl-mode': 'measured'}, {'sl-mode': 'plain'}]);
  const lines = openLines(build(s, c, imgDir(files)));
  assert.deepEqual(lines.map(l => /\((.*)\)$/.exec(l)[1]),
    ['select Projected', 'select Measured', 'select No coverage']);
});

test('two states the notes cannot tell apart stop the build instead of printing one line twice', () => {
  // The other half of the same defect: deduplicating on the seed rather than
  // on the rendered line lets two different states print the same sentence,
  // which is the bare-duplicate-line failure arriving by another route. The
  // builder must not pick one -- it says which note is missing.
  const {s, c, files} = sameUrl([
    {'sl-mode': 'projected', 'sl-author': 'reviewer'},
    {'sl-mode': 'projected', 'sl-author': 'somebody-else'},
  ]);
  assert.throws(() => build(s, c, imgDir(files)),
    /V01: two screenshots of .* were taken in different viewer states, and the reproduce block renders both as "# +open .*\(select Projected\)"/);
});
