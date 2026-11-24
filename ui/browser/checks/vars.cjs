// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The Variables panel and its source-pane highlighting (#53).
//
// A VM test can pin the model; only a real browser can show that hovering a
// row actually marks the right source lines, that a line touched twice shows
// both of its positions, that the marks survive the pointer crossing onto
// one of the row's own badges -- and, the one that would be a silent
// regression, that the coverage colour underneath is still there afterwards.
// The whole convention in app.css is that a highlight decorates the code
// cell rather than repainting the row, and nothing but a rendered page can
// check it.
//
// # Why the evidence here is written by hand
//
// It used to ingest `examples/c` and assert seven AArch64 frame-slot
// accesses. `vars::attribute` reads frame slots for aarch64 only, and
// deliberately reports nothing on another architecture -- so on CI, which
// runs the browser suite on `ubuntu-24.04` (x86-64) against a demo built for
// that host, every one of those assertions would have failed the moment the
// hosted job could run. Returning a skip is not a fix either: `run.cjs`
// exits 2 for a requested suite that could not run, by decision (#89).
//
// So the interaction assertions are driven from the two evidence documents
// below, written out into this check's scratch directory and served like any
// other. They are the *viewer's* input, which is what this suite is about:
// the panel's contents, the order badges, the marks and the toggle are the
// same code path on every host, and the numbers below are the same on every
// host too. The unsupported-target state -- variables with a reason and no
// accesses, which is what an x86-64 image really produces -- is one of them,
// so it is covered as a state rather than as an accident of the runner
// (review of #53).
//
// The real demo is still exercised where its numbers are real: the AArch64
// attribution itself is `crates/ingest/tests/vars.rs`, host-side.

const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const {scratch, serve, browser} = require('../harness.cjs');

// The fixture's only function, and the only one in its file.
const FN = 0;

/* ----------------------------- the fixture ----------------------------- */

// A source file that is never compiled: what is under test is the evidence,
// and the text exists so the source pane has lines to mark. The shape
// mirrors `checksum` in examples/c -- a line touched twice, a loop counter
// whose lines are not monotonic, an access the line table places on no line,
// a variable with a reason alongside real accesses, and one with a reason
// and nothing else -- and adds the two cases that pin what the numbering
// does *not* mean: a loop body site, which repeats at run time but is listed
// once, and a site inside a branch that is never taken.
const SRC = [
  '// SPDX-License-Identifier: MIT OR Apache-2.0',
  '// A hand-written fixture for ui/browser/checks/vars.cjs.',
  '// Never compiled: the evidence beside it is the input under test.',
  '#include <stdint.h>',
  '',
  '/* Sum a buffer, docking one for every odd entry. */',
  'uint32_t tally(const uint8_t *buf, uint32_t n)',
  '{',
  '    uint32_t sum = 0;',
  '    uint32_t i = 0;',
  '    uint32_t scratch;',
  '',
  '    for (i = 0; i < n; i++) {',
  '        sum += buf[i];',
  '',
  '        if (buf[i] & 1) {',
  '            sum -= 1;',
  '        }',
  '    }',
  '    return sum;',
  '}',
  '',
  '/* nothing below this line is analysed */',
  '',
];

// Line 17 is the body of the `if`, and the run never reached it: count 0.
// That is the whole point of the branch case -- the panel numbers a site on
// a line that demonstrably did not execute.
const COUNTS = {7: 1, 9: 1, 10: 1, 13: 9, 14: 8, 16: 8, 17: 0, 20: 1};

const A = (addr, line, access) => ({addr, ...(line == null ? {} : {line}), access});

// What the AArch64 attribution would report for this function.
const ATTRIBUTED = [
  // A pointer parameter whose address is formed: real accesses *and* a
  // reason, because what is missing is everything reached through it.
  {name: 'buf', kind: 'param', declLine: 7, typeName: 'const uint8_t *',
    accesses: [A(0x1000, 7, 'write'), A(0x1020, 14, 'address')],
    unattributed: 'its address is taken here, and accesses made through that pointer -- by a callee, or later in this function -- are not tracked'},
  // An access the line table places on no line: it keeps its position and is
  // counted, so the read on 13 is the third touch and not the second.
  {name: 'n', kind: 'param', declLine: 7, typeName: 'uint32_t',
    accesses: [A(0x1004, 7, 'write'), A(0x1008, null, 'read'), A(0x1018, 13, 'read')]},
  // Line 14 twice (read then write), and sites 4 and 5 inside the `if`.
  {name: 'sum', kind: 'local', declLine: 9, typeName: 'uint32_t',
    accesses: [A(0x100c, 9, 'write'), A(0x1024, 14, 'read'), A(0x1028, 14, 'write'),
      A(0x1034, 17, 'read'), A(0x1038, 17, 'write'), A(0x1048, 20, 'read')]},
  // The loop counter: 10, 13, 14, 13, 13 -- never in ascending line order.
  {name: 'i', kind: 'local', declLine: 10, typeName: 'uint32_t',
    accesses: [A(0x1010, 10, 'write'), A(0x101c, 13, 'read'), A(0x1030, 14, 'read'),
      A(0x103c, 13, 'read'), A(0x1040, 13, 'write')]},
  // Nothing to attribute, and the reason is the only thing to show.
  {name: 'scratch', kind: 'local', declLine: 11, typeName: 'uint32_t', accesses: [],
    unattributed: 'the optimiser kept it in a register, so it has no frame slot to watch'},
];

// ...and what the same function really produces off AArch64, in the wording
// `vars::attribute` uses: every variable still listed, every access list
// empty, and the reason on each one.
const FOREIGN_REASON = 'frame-slot accesses are read for aarch64; this image is x86_64';
const UNSUPPORTED = ATTRIBUTED.map(v => ({...v, accesses: [], unattributed: FOREIGN_REASON}));

const INSTR = (addr, mnemonic, operands, line, flags = []) =>
  ({addr, mnemonic, operands, line, fileId: 0, isStmt: true, projected: 'covered', flags, gapIds: []});

/// One complete evidence document. Every field `Evidence::validate` and the
/// viewer need, and nothing else -- so a reader can see the whole input this
/// suite asserts against in one place.
function evidence({arch, objectFormat, variables}) {
  const lines = SRC.map((_, i) => ({count: COUNTS[i + 1] == null ? null : COUNTS[i + 1]}));
  const linesTotal = Object.keys(COUNTS).length;
  const linesCovered = Object.values(COUNTS).filter(c => c > 0).length;
  return {
    schema: 'butterfly-evidence/1',
    provenance: {
      tool: 'butterfly', toolVersion: '0.1.0', createdUnix: 1700000000,
      host: 'browser-check-fixture', inputs: [], buildInfo: {},
      coverageExportVersion: 'fixture', objectFormat, arch,
      caveats: ['hand-written fixture for the Variables browser checks (#53); not produced by ingest'],
    },
    summary: {
      files: 1, functions: 1, functionsExecuted: 1,
      linesTotal, linesCovered,
      branchesTotal: 0, branchesCovered: 0, decisionsTotal: 0, decisionsMcdcCovered: 0,
      conditionsTotal: 0, conditionsIndependenceCovered: 0,
      instructionsTotal: 6, instructionsProjectedCovered: 6, instructionsProjectedUncovered: 0,
      instructionsUnknown: 0, gapsTotal: 0, gapsByKind: {},
    },
    files: [{
      id: 0, path: '/fixture/tally.c', display: 'tally.c', sha256: '0'.repeat(64),
      text: SRC, lines, functionIds: [0], linesTotal, linesCovered,
    }],
    functions: [{
      id: 0, name: 'tally', mangled: '_tally', fileId: 0, lineStart: 7, lineEnd: 21,
      count: 1, instantiations: 1, requirements: [], lineRequirements: {},
      linesTotal, linesCovered, branches: [], decisions: [],
      complexity: {
        cyclomaticObject: 2, decisions: 0, conditions: 0, maxConditionsPerDecision: 0,
        objectBlocks: 3, objectEdges: 2, objectInstructions: 6, objectConditionalBranches: 0, callsOut: 0,
      },
      callees: [], callers: [],
      object: {
        symbol: '_tally', address: 0x1000, size: 0x50,
        blocks: [
          {id: 0, address: 0x1000, kind: 'entry', lines: [7, 9, 10], projected: 'covered',
            instrs: [INSTR(0x1000, 'str', 'x0, [sp, #8]', 7), INSTR(0x1004, 'str', 'w1, [sp, #4]', 7)],
            succs: [{to: 1, kind: 'fallthrough', projected: 'covered'}]},
          {id: 1, address: 0x1018, kind: 'normal', lines: [13, 14, 17], projected: 'covered',
            instrs: [INSTR(0x1018, 'ldr', 'w8, [sp, #4]', 13), INSTR(0x1024, 'ldr', 'w9, [sp, #12]', 14)],
            succs: [{to: 2, kind: 'taken', projected: 'covered'}]},
          {id: 2, address: 0x1048, kind: 'exit', lines: [20], projected: 'covered',
            instrs: [INSTR(0x1048, 'ldr', 'w0, [sp, #12]', 20), INSTR(0x104c, 'ret', '', 20, ['return'])],
            succs: []},
        ],
        gaps: [], instrTotal: 6, instrProjectedCovered: 6, instrProjectedUncovered: 0, instrUnknown: 0,
        // Written out although `serde` defaults them: the viewer reads
        // `object.disagreements.length` before any tab renders, so a field
        // left out here is a blank pane rather than a default.
        instrMeasuredCovered: null, disagreements: [],
        outcomeEdgesTotal: 0, outcomeEdgesMeasured: null,
        unreachableBlocks: [], compilerGenerated: {},
      },
      symbolId: null, dataRefs: [], variables, constructs: {}, inlined: [],
      foldedCalls: [], indirectCalls: [], pointerCallers: [], commits: [],
    }],
    symbols: [], globals: [],
  };
}

function writeFixture(dir, name, doc) {
  const p = path.join(dir, `${name}.json`);
  fs.writeFileSync(p, JSON.stringify(doc, null, 2));
  return p;
}

/* ------------------------------ the checks ------------------------------ */

async function run({out}) {
  const dir = scratch('vars');
  const aarch64 = writeFixture(dir, 'evidence-aarch64',
    evidence({arch: 'aarch64', objectFormat: 'macho', variables: ATTRIBUTED}));
  const foreign = writeFixture(dir, 'evidence-x86_64',
    evidence({arch: 'x86_64', objectFormat: 'elf', variables: UNSUPPORTED}));

  const results = [];
  let srv, br, page;
  const check = async (name, fn) => {
    try {
      const detail = await fn();
      results.push({name, pass: true, ...(detail ? {detail} : {})});
    } catch (e) {
      const shot = path.join(out, `vars-${name}.png`);
      await page?.screenshot({path: shot, fullPage: true}).catch(() => {});
      results.push({name, pass: false, error: e.message, screenshot: shot});
    }
  };

  try {
    srv = await serve(aarch64, {readOnly: true, dir, out});
    br = await browser();
    page = await br.newPage({viewport: {width: 1460, height: 900}});
    const pageErrors = [];
    page.on('pageerror', e => pageErrors.push(e.message));

    // One row of the panel, found by its name cell rather than by position,
    // so a reordering of the table does not silently retarget the test.
    const row = name => page.locator('table.vars tr.frow')
      .filter({has: page.locator(`td.mono:text-is("${name}")`)});
    const nameCell = name => row(name).locator('td.mono').first();

    // What the source pane is showing: which line is marked as the
    // declaration, and the badges on each line that is marked as touched.
    const marks = () => page.evaluate(() => ({
      decl: [...document.querySelectorAll('.src tr.vdecl')].map(tr => +tr.dataset.line),
      uses: Object.fromEntries([...document.querySelectorAll('.src tr.vuse')]
        .map(tr => [tr.dataset.line, [...tr.querySelectorAll('.vmk .vb')].map(b => b.textContent)])),
    }));
    // The switch that makes the marking optional, and what it says about
    // itself. A real button, so it is found by role and its pressed state
    // is read the way assistive technology reads it.
    const toggle = () => page.getByRole('button', {name: 'Variable marks', exact: true});
    const pressed = () => toggle().getAttribute('aria-pressed');
    // The panel's own account of what it is showing.
    const note = () => page.locator('.note').filter({hasText: 'Parameters and locals from DWARF'});
    // A hover paints synchronously, so one frame in the page is enough to be
    // sure nothing more is coming. Asserting that something never appears is
    // the one case a locator cannot wait for.
    const settle = () => page.evaluate(() => new Promise(r => requestAnimationFrame(() => r())));
    const cleared = async () => {
      await page.mouse.move(0, 0);
      await page.locator('.src tr.vuse').first().waitFor({state: 'detached'});
    };

    // The row's own background, which is its coverage colour. Read from the
    // row, not the code cell, because that is the thing a highlight must not
    // take over.
    const rowStyle = ln => page.evaluate(l => {
      const tr = document.querySelector(`.src tr[data-line="${l}"]`);
      return {cls: tr.className, bg: getComputedStyle(tr).backgroundColor};
    }, ln);
    // What the coverage export said about a source line, as the pane shows
    // it: the executions column beside the line number.
    const execCount = ln => page.locator(`.src tr[data-line="${ln}"] td.ct`).textContent();

    await page.goto(`${srv.url}/#/fn/${FN}/variables`);
    await page.locator('table.vars').waitFor();
    // The viewer opens in "No coverage" mode -- S.mode defaults to 'plain',
    // and in that mode every source row is deliberately transparent. The
    // projection has to be switched on before any of this can say whether
    // the highlight preserves the colour underneath it; against a blank row
    // that assertion would pass without ever testing anything.
    await page.getByRole('button', {name: 'Projected', exact: true}).click();
    await page.locator('.src tr.cov').first().waitFor();
    await page.locator('table.vars').waitFor();

    // The panel itself: every parameter and local, with what the evidence
    // knows about each. If this is empty the rest proves nothing.
    await check('the-panel-lists-every-parameter-and-local', async () => {
      const names = await page.locator('table.vars tr.frow td.mono:nth-child(1)').allTextContents();
      assert.deepEqual(names, ['buf', 'n', 'sum', 'i', 'scratch'],
        'tally has two parameters and three locals, in declaration order');
      const kinds = await page.locator('table.vars tr.frow td:nth-child(2)').allTextContents();
      assert.deepEqual(kinds, ['param', 'param', 'local', 'local', 'local']);
      // The accesses are shown as an ordered list, not a count.
      const sum = await row('sum').locator('td:nth-child(5) .vb').allTextContents();
      assert.deepEqual(sum, ['1WL9', '2RL14', '3WL14', '4RL17', '5WL17', '6RL20'],
        'each access carries its position, its kind and its line');
      // An address-taken touch is its own kind, and an access the line table
      // could not place keeps its position rather than being renumbered away.
      const buf = await row('buf').locator('td:nth-child(5) .vb').allTextContents();
      assert.deepEqual(buf, ['1WL7', '2&L14']);
      const n = await row('n').locator('td:nth-child(5) .vb').allTextContents();
      assert.deepEqual(n, ['1WL7', '2R—', '3RL13'], 'the unplaceable access holds position 2');
      // A variable with nothing attributed says why, and is never drawn as
      // unused -- the inversion this feature exists to prevent.
      assert.match(await row('scratch').locator('td:nth-child(5)').textContent(),
        /not attributed — the optimiser kept it in a register/);
      // ...and a reason is carried beside real accesses too, plus the note
      // about the access no line could be found for.
      const notes = await page.locator('table.vars tr.vnote').allTextContents();
      assert.equal(notes.length, 2, `one note for buf's escaped address and one for n's unplaced access, got ${JSON.stringify(notes)}`);
      assert.match(notes[0], /incomplete/);
      assert.match(notes[1], /1 of these 3 accesses are on instructions the line table places on no source line/);
      await page.screenshot({path: path.join(out, 'vars-panel.png')});
      return {variables: names.length};
    });

    // What the numbers mean, in the place they are read. The evidence is one
    // pass over an address-sorted instruction list, so the panel may say
    // where the touches are and must not say when they happen.
    await check('the-panel-calls-the-numbering-static-instruction-order', async () => {
      const text = (await note().textContent()).replace(/\s+/g, ' ');
      assert.match(text, /static instruction order/, 'the order is named for what it is');
      assert.match(text, /It is not an execution order/, 'and the limitation is stated where the claim is made');
      assert.match(text, /loop is listed once however many times it repeats/);
      assert.match(text, /branch is listed whether or not it is ever reached/);
      assert.doesNotMatch(text, /the order the object code performs them/,
        'the panel must not claim the object code performs them in this order');
      assert.doesNotMatch(text, /read before it is written/,
        'nor that the list settles a read-before-write question');
      const head = await page.locator('table.vars th').nth(4).textContent();
      assert.equal(head, 'accesses, in static instruction order (by address)');
      // and the badge a reader hovers to find out what the number means
      const title = await row('sum').locator('td:nth-child(5) .vb').nth(3).getAttribute('title');
      assert.match(title, /^static access site 4 of 6, in address order and not an execution order:/, title);
      return {header: head};
    });

    // The feature: hover a variable, see where it lives and where it is used.
    await check('hovering-a-variable-marks-its-declaration-and-every-line-it-touches', async () => {
      const before = await rowStyle(14);
      await row('sum').hover();
      await page.locator('.src tr.vuse').first().waitFor();
      const m = await marks();

      assert.deepEqual(m.decl, [9], 'the declaration line, and only it, is marked as the declaration');
      // Line 9 is both: `sum` is declared there and the initialising store is
      // its first access, so it carries the decl mark and position 1.
      assert.deepEqual(m.uses, {
        9: ['decl', '1W'],
        14: ['2R', '3W'],
        17: ['4R', '5W'],
        20: ['6R'],
      }, 'every touched line is marked, with every position it holds');

      // A line touched twice keeps both positions, in address order.
      assert.deepEqual(m.uses['14'], ['2R', '3W'],
        'on line 14 the read is at the lower address, so it is listed first');

      // ...and the coverage colour underneath is untouched.
      const during = await rowStyle(14);
      assert.equal(during.bg, before.bg, 'the highlight must not replace the row background');
      assert.match(during.cls, /\b(cov|unc|jus|part)\b/, 'the row keeps its coverage status class');
      assert.notEqual(during.bg, 'rgba(0, 0, 0, 0)', 'the coverage colour must still be painted');
      await page.screenshot({path: path.join(out, 'vars-hover-sum.png')});
      return {declLine: m.decl[0], linesMarked: Object.keys(m.uses).length, coverage: during.cls.trim()};
    });

    // A loop counter: the lines it is touched on do not run in order, and a
    // set of lines could not express that at all.
    await check('a-loop-counter-keeps-its-non-monotonic-order', async () => {
      await row('i').hover();
      await page.locator('.src tr[data-line="13"].vuse').waitFor();
      const m = await marks();
      assert.deepEqual(m.decl, [10]);
      assert.deepEqual(m.uses, {10: ['decl', '1W'], 13: ['2R', '4R', '5W'], 14: ['3R']},
        'line 13 holds positions 2, 4 and 5, with 3 on line 14 in between');
      await page.screenshot({path: path.join(out, 'vars-hover-loop.png')});
      return {uses: m.uses};
    });

    // The regression that pins the distinction: this fixture's numbering
    // would be wrong if it were read as the order a run performed the
    // accesses. Line 17 is inside an `if` the run never took -- the pane
    // shows 0 executions for it -- and it is still numbered; line 13 is a
    // loop body the run entered nine times, and it carries one badge per
    // instruction, not one per iteration.
    await check('a-branch-site-is-numbered-on-a-line-that-never-ran', async () => {
      await cleared();
      await row('sum').hover();
      await page.locator('.src tr.vuse').first().waitFor();
      const m = await marks();
      assert.equal(await execCount(17), '0', 'line 17 must be a line the run never executed');
      assert.deepEqual(m.uses['17'], ['4R', '5W'],
        'and it still carries its two positions, because these are static sites');
      const cls = (await rowStyle(17)).cls;
      assert.match(cls, /\bunc\b/, 'the pane shows it as uncovered while the panel numbers it');

      await row('i').hover();
      await page.locator('.src tr[data-line="13"].vuse').waitFor();
      const loop = await marks();
      assert.equal(await execCount(13), '9', 'line 13 was executed nine times');
      assert.equal(loop.uses['13'].length, 3,
        'and carries three badges -- one per instruction in the image, not one per iteration');
      return {branchLineExecutions: 0, loopLineExecutions: 9, loopBadges: 3};
    });

    // Each access entry is a hover target of its own, so a reviewer can go
    // from one instruction to the line it is on.
    await check('hovering-a-single-access-marks-the-same-lines', async () => {
      await cleared();
      await row('sum').locator('td:nth-child(5) .vb').nth(2).hover();
      await page.locator('.src tr.vuse').first().waitFor();
      const m = await marks();
      assert.deepEqual(m.decl, [9], 'an access entry lights its own variable');
      assert.deepEqual(m.uses['14'], ['2R', '3W']);
      return {from: 'access 3 of sum'};
    });

    // Moving *within* the row must change nothing. Each badge used to clear
    // the marks on its own mouseleave, including when the pointer went back
    // to the variable's name -- and the row's mouseenter does not fire again
    // for a move inside itself, so the marks stayed off until the pointer
    // left the row entirely and came back (review of #53).
    await check('the-marks-survive-moving-from-a-badge-back-to-the-variable-name', async () => {
      await cleared();
      await nameCell('sum').hover();
      await page.locator('.src tr.vuse').first().waitFor();
      const onName = await marks();
      assert.equal(Object.keys(onName.uses).length, 4, 'four lines are marked to begin with');

      await row('sum').locator('td:nth-child(5) .vb').first().hover();
      await settle();
      const onBadge = await marks();
      assert.deepEqual(onBadge, onName, 'moving onto one of the row\'s own badges changes nothing');

      await nameCell('sum').hover();
      await settle();
      const back = await marks();
      assert.deepEqual(back, onName,
        'and moving back to the name keeps them: the pointer never left the row');
      assert.ok(await page.locator('.src .vmk').count() > 0, 'the order badges must still be on the source');
      return {linesMarked: Object.keys(back.uses).length};
    });

    // The same question for the keyboard: a row that still holds focus is
    // still the row the reader is on, whatever the pointer does.
    await check('a-focused-row-keeps-its-marks-when-the-pointer-leaves', async () => {
      await cleared();
      await row('sum').focus();
      await page.locator('.src tr.vuse').first().waitFor();
      const focused = await marks();
      assert.deepEqual(focused.decl, [9], 'focus alone marks the row\'s variable');

      await row('sum').hover();
      await settle();
      await page.mouse.move(0, 0);
      await settle();
      assert.deepEqual(await marks(), focused,
        'the pointer left, but the keyboard is still on this row');
      assert.equal(await page.evaluate(() => document.activeElement.classList.contains('frow')), true,
        'and the row really is still document.activeElement');

      // A hover elsewhere wins while it lasts, and hands back to the focus.
      await row('i').hover();
      await page.locator('.src tr[data-line="13"].vuse').waitFor();
      assert.deepEqual((await marks()).decl, [10], 'the pointer wins while it is in another row');
      await page.mouse.move(0, 0);
      await settle();
      assert.deepEqual(await marks(), focused, 'and leaving it returns to the focused row');

      await page.evaluate(() => document.activeElement.blur());
      await settle();
      const after = await marks();
      assert.deepEqual(after.decl, [], 'blurring the row does take the marks off');
      assert.deepEqual(after.uses, {});
    });

    // Moving away takes every mark off again -- a stale badge beside the
    // wrong variable would be a false statement about the object code.
    await check('moving-away-clears-every-mark', async () => {
      await row('sum').hover();
      await page.locator('.src tr.vuse').first().waitFor();
      await cleared();
      const m = await marks();
      assert.deepEqual(m.decl, []);
      assert.deepEqual(m.uses, {});
      assert.equal(await page.locator('.src .vmk').count(), 0, 'no order badge may be left behind');
    });

    /* The marks are optional. Everything above ran with them on, which is
       the default; what follows is the off state, which has to mark exactly
       nothing rather than something fainter. */

    await check('the-marks-are-on-by-default-and-the-button-says-so', async () => {
      assert.equal(await toggle().count(), 1, 'the toggle sits beside the coverage overlay');
      assert.equal(await pressed(), 'true', 'nothing has been stored, so the marks are on');
      assert.match(await note().textContent(), /Hover a variable/,
        'and the panel promises the hover it is about to do');
      return {ariaPressed: 'true'};
    });

    // Switching off has to take effect on what is already painted. Badges
    // left over from a hover that happened before the click would be the
    // viewer stating something it no longer means to state.
    await check('toggling-off-while-marks-are-showing-clears-them-at-once', async () => {
      await row('sum').hover();
      await page.locator('.src tr.vuse').first().waitFor();
      assert.ok(await page.locator('.src .vmk').count() > 0, 'marks must be on screen before the click');
      await toggle().click();
      assert.equal(await pressed(), 'false');
      assert.equal(await page.locator('.src tr.vdecl').count(), 0, 'the declaration mark goes at once');
      assert.equal(await page.locator('.src tr.vuse').count(), 0);
      assert.equal(await page.locator('.src .vmk').count(), 0, 'no order badge may survive the click');
      await page.screenshot({path: path.join(out, 'vars-marks-off.png')});
    });

    // ...and with them off, hovering marks nothing at all.
    await check('with-the-marks-off-hovering-marks-nothing', async () => {
      for (const name of ['sum', 'i']) {
        await row(name).hover();
        await settle();
        const m = await marks();
        assert.deepEqual(m.decl, [], `hovering ${name} must not mark a declaration`);
        assert.deepEqual(m.uses, {}, `hovering ${name} must not mark a line it touches`);
      }
      // an access entry is a hover target of its own, and it is off too
      await row('sum').locator('td:nth-child(5) .vb').nth(2).hover();
      await settle();
      assert.equal(await page.locator('.src .vmk').count(), 0, 'nor may an access badge paint one');
      // nor does the keyboard paint one
      await row('sum').focus();
      await settle();
      assert.equal(await page.locator('.src .vmk').count(), 0, 'nor may focusing a row');
      await page.evaluate(() => document.activeElement.blur());

      // The panel is untouched: the toggle hides marks, never evidence.
      const names = await page.locator('table.vars tr.frow td.mono:nth-child(1)').allTextContents();
      assert.deepEqual(names, ['buf', 'n', 'sum', 'i', 'scratch'], 'the list still holds every parameter and local');
      const sum = await row('sum').locator('td:nth-child(5) .vb').allTextContents();
      assert.deepEqual(sum, ['1WL9', '2RL14', '3WL14', '4RL17', '5WL17', '6RL20'],
        'and every access, with its position, its kind and its line');
      // and it says which state it is in, so a hover doing nothing is
      // explained rather than read as missing data
      assert.match(await note().textContent(), /Marking in the source pane is\s*off/);
    });

    await check('the-setting-survives-a-reload', async () => {
      await page.reload();
      await page.locator('table.vars').waitFor();
      assert.equal(await pressed(), 'false', 'the choice is kept per browser, the way the theme is');
      await row('sum').hover();
      await settle();
      assert.equal(await page.locator('.src .vmk').count(), 0, 'and it still marks nothing');
    });

    // Back on: the next hover paints, and the click itself resurrects
    // nothing -- what was on screen when the marks went off is gone for good.
    await check('turning-the-marks-back-on-paints-the-next-hover-and-nothing-else', async () => {
      await toggle().click();
      assert.equal(await pressed(), 'true');
      assert.equal(await page.locator('.src .vmk').count(), 0, 'switching on must not restore a stale hover');
      await row('sum').hover();
      await page.locator('.src tr.vuse').first().waitFor();
      const m = await marks();
      assert.deepEqual(m.decl, [9], 'and the hover marks what it always did');
      assert.deepEqual(m.uses['14'], ['2R', '3W']);
      await page.reload();
      await page.locator('table.vars').waitFor();
      assert.equal(await pressed(), 'true', 'on is remembered as well as off');
      await page.mouse.move(0, 0);
    });

    // The number keys still select the tabs they used to; the new one is the
    // tenth, on 0 (README).
    await check('the-number-key-shortcuts-still-select-their-tabs', async () => {
      const tabAfter = async key => {
        await page.locator('#content').click({position: {x: 2, y: 2}});
        await page.keyboard.press(key);
        await page.waitForFunction(() => true);
        return page.evaluate(() => location.hash);
      };
      assert.match(await tabAfter('1'), /\/flow$/, '1 is still Flow');
      assert.match(await tabAfter('7'), /\/data$/, '7 is still Data');
      assert.match(await tabAfter('8'), /\/variables$/, '8 is the new Variables tab');
      assert.match(await tabAfter('9'), /\/changes$/, '9 is Changes');
      assert.match(await tabAfter('0'), /\/requirements$/, '0 is the tenth tab');
    });

    await check('the-viewer-raised-no-page-errors', async () => {
      assert.deepEqual(pageErrors, [], 'the viewer must raise no page errors');
    });

    // The state every non-AArch64 build is in, as its own case rather than
    // as whatever the runner happened to compile: the variables are all
    // there, every access list is empty, and each one says why. A panel that
    // showed this as "no accesses" would be reporting an unread image as a
    // variable nothing touches.
    await check('an-unsupported-target-says-why-rather-than-showing-an-empty-list', async () => {
      const fdir = scratch('vars-foreign');
      const fsrv = await serve(foreign, {readOnly: true, dir: fdir, out});
      const fpage = await br.newPage({viewport: {width: 1460, height: 900}});
      const errs = [];
      fpage.on('pageerror', e => errs.push(e.message));
      try {
        await fpage.goto(`${fsrv.url}/#/fn/${FN}/variables`);
        await fpage.locator('table.vars').waitFor();
        await fpage.getByRole('button', {name: 'Projected', exact: true}).click();
        await fpage.locator('.src tr.cov').first().waitFor();

        const names = await fpage.locator('table.vars tr.frow td.mono:nth-child(1)').allTextContents();
        assert.deepEqual(names, ['buf', 'n', 'sum', 'i', 'scratch'],
          'every parameter and local is still listed: they exist, and their declarations are real');
        const cells = await fpage.locator('table.vars tr.frow td:nth-child(5)').allTextContents();
        for (const [i, c] of cells.entries()) {
          assert.match(c, /^not attributed — frame-slot accesses are read for aarch64; this image is x86_64$/,
            `${names[i]} must say why it has no accesses`);
        }
        assert.equal(await fpage.locator('table.vars .vb').count(), 0, 'and there is no access badge to show');

        // Hovering marks no access: there is none to mark. The declaration
        // line is still marked, because DW_AT_decl_line is not part of what
        // the attribution could not do.
        const frow = fpage.locator('table.vars tr.frow')
          .filter({has: fpage.locator('td.mono:text-is("sum")')});
        await frow.hover();
        await fpage.locator('.src tr.vdecl').waitFor();
        const fmarks = await fpage.evaluate(() => ({
          decl: [...document.querySelectorAll('.src tr.vdecl')].map(tr => +tr.dataset.line),
          uses: [...document.querySelectorAll('.src tr.vuse')].map(tr => +tr.dataset.line),
          badges: document.querySelectorAll('.src .vmk .vb:not(.decl)').length,
        }));
        assert.deepEqual(fmarks.decl, [9], 'the declaration is known, and is marked');
        assert.deepEqual(fmarks.uses, [], 'no line may be marked as touched');
        assert.equal(fmarks.badges, 0, 'and no order badge may appear in the source');
        await fpage.screenshot({path: path.join(out, 'vars-unsupported-target.png')});
        assert.deepEqual(errs, [], 'the viewer must raise no page errors here either');
        return {variables: names.length, accessBadges: 0};
      } finally {
        await fpage.close().catch(() => {});
        fsrv.child.kill();
      }
    });
  } finally {
    await page?.close().catch(() => {});
    await br?.close();
    srv?.child.kill();
  }
  return results;
}

// `evidence` and the two variable sets are exported so the fixture can be
// written out and inspected on its own -- `node -e` plus `butterfly summary`
// is how you find out whether a change here still validates.
module.exports = {run, evidence, ATTRIBUTED, UNSUPPORTED};
