// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The showcase generator's data layer (#154): the TOML reader, the rules
// support/features.toml is held to, and the placeholder that keeps typed-in
// numbers off the page. Lives beside the viewer tests because that is the
// glob CI and ci-local run (`ui/*.test.cjs`).

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {parse} = require('../scripts/showcase/toml.cjs');
const {load, loadMatrix, matches, factKeys, factSources, FEATURES} = require('../scripts/showcase/spec.cjs');
const {fill} = require('../scripts/showcase/facts.cjs');

test('reads the TOML the two support files use', () => {
  const doc = parse(`
schema = 1
"quoted key" = 'literal \\n'
[fixture.c]
build = [
  "a",  # comment
  "b",
]
seed = {sl-mode = "projected", n = 2}
[[row]]
id = "one"
text = """
first \\
  second"""
[[row]]
id = "two"
[[row.shot]]
file = "x"
`);
  assert.equal(doc.schema, 1);
  assert.equal(doc['quoted key'], 'literal \\n');
  assert.deepEqual(doc.fixture.c.build, ['a', 'b']);
  assert.deepEqual(doc.fixture.c.seed, {'sl-mode': 'projected', n: 2});
  assert.equal(doc.row[0].text, 'first second');
  assert.equal(doc.row[1].shot[0].file, 'x');
});

test('refuses what it cannot read, naming the line', () => {
  assert.throws(() => parse('a = 1\nd = 1979-05-27'), /toml:2: unsupported value/);
  assert.throws(() => parse('a = 1\na = 2'), /toml:2: duplicate key a/);
  assert.throws(() => parse('a = "x" y'), /unexpected/);
  assert.throws(() => parse('a = "unterminated'), /unterminated|newline/);
  assert.throws(() => parse('[t]\n[t]'), /defined twice/);
});

test('a single-bracket header over an array of tables is refused, not merged', () => {
  // `[[r]]` pushed its element without recording it as a defined table, so
  // the refusal below never fired and `[r]` merged into the last element: a
  // `[feature.shot]` typed with single brackets silently became part of the
  // shot above it, and one screenshot disappeared from the page (#212 S19).
  assert.throws(() => parse('[[r]]\nx = 1\n[r]\ny = 2'), /array of tables/);
  assert.throws(() => parse('[[feature]]\nid = "V01"\n[[feature.shot]]\nfile = "a"\n[feature.shot]\nfile = "b"'), /array of tables/);
  // ...and the shapes that are legal still parse
  const doc = parse('[[feature]]\nid = "V01"\n[[feature.shot]]\nfile = "a"\n[[feature.shot]]\nfile = "b"\n[feature.facts]\nk = "v"');
  assert.deepEqual(doc.feature[0].shot.map(s => s.file), ['a', 'b']);
  assert.deepEqual(doc.feature[0].facts, {k: 'v'});
});

test('an expectation is matched on word boundaries, not as a bare substring', () => {
  // "28 gaps" used to be satisfied by "128 gaps" (#212 S12)
  assert.equal(matches('the function has 128 gaps', '28 gaps'), false);
  assert.equal(matches('the function has 28 gaps', '28 gaps'), true);
  assert.equal(matches('reviewers here', 'reviewer'), false);
  assert.equal(matches('recorded by reviewer.', 'reviewer'), true);
  // ...while a line break and the viewer's CSS upper-casing still match
  assert.equal(matches('MC/DC NOT ACHIEVED\n  (1/3)', 'MC/DC not achieved (1/3)'), true);
  // an expectation that does not begin or end on a word character needs no
  // boundary of its own
  assert.equal(matches('breach(es) — this build would fail CI', '— this build would fail CI'), true);
});

test('support/features.toml obeys the showcase rules', () => {
  const spec = load();
  assert.ok(spec.features.length >= 27, 'one entry per section of the epic');
  for (const f of spec.features) {
    if (f.status === 'pending') assert.ok(f.blocked_by.length, `${f.id} names its blocker`);
    else assert.ok(Array.isArray(f.limits), `${f.id} states its limits`);
  }
});

test('a section quotes numbers from its own evidence and from nowhere else', () => {
  // This is what lets the capture run write each section's numbers down
  // BESIDE that section, which is what stops a `--only` refresh from
  // rewriting another section's caption (#212 review, finding 1). Every
  // section of support/features.toml already obeys it:
  for (const f of load().features) {
    const sources = factSources(f);
    for (const k of factKeys(f)) assert.ok(sources.has(k), `${f.id} quotes {{${k}...}}, which it does not name`);
  }
  const base = fs.readFileSync(FEATURES, 'utf8');
  // ...and one that does not is refused by name, before a browser is launched.
  const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'showcase-spec-')), 'features.toml');
  fs.writeFileSync(file, base.replace(
    'caption = "clamp_altitude in examples/c: the object-code control-flow graph',
    'caption = "{{cpp-lib.gapsTotal}} — clamp_altitude in examples/c: the object-code control-flow graph'));
  assert.throws(() => load(file), /V02: quotes `\{\{cpp-lib\.\.\.\.\}\}`.*not one of this section's fixtures/s);
});

test('the support matrix is read by name, not by guessing which array is the rows', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'showcase-matrix-'));
  const file = path.join(dir, 'matrix.toml');
  // `[[local_only]]` comes first and, the day it gains a `state` key, the old
  // "first top-level array of tables whose elements all carry a `state`"
  // rule would have rendered the matrix as that one row (#213).
  fs.writeFileSync(file, `
[[local_only]]
example = "examples/cpp"
reason = "hosted CI does not build it"
state = "caveats"

[[row]]
id = "c"
state = "supported"

[[row]]
id = "cpp"
state = "caveats"
`);
  assert.deepEqual(loadMatrix(file).map(r => r.id), ['c', 'cpp']);
  assert.equal(loadMatrix(path.join(dir, 'absent.toml')), null, 'no matrix yet is a state the page renders');
  fs.writeFileSync(file, '[[other]]\nid = "x"\n');
  assert.throws(() => loadMatrix(file), /no `\[\[row\]\]` entries/);
});

/* ----------------------- no number is typed in ---------------------------- */
//
// The page's rule (#150) is that a count, a size or a measurement comes from
// the evidence through a {{placeholder}}. What MAY be typed is a reference:
// a standard's section, an objective, an issue, a flag, a version, a file
// name -- things that do not change when the code does.
//
// Three holes were found in the first version of this guard (#212 B4):
//
//   * it exempted every backticked span, so ``clamp_altitude is `8 blocks` ``
//     passed -- and a backtick renders as <code>, which is exactly where a
//     reader looks for a literal value;
//   * `\d` is ASCII-only, so "eight blocks", "VIII of IX" and a full-width
//     "８" all passed;
//   * it scanned `claim` and `limits` only. A caption is a claim
//     (docs/showcase/README.md), and so are a title, a pending reason and an
//     entry in "what we do not claim".
//
// A code span is still exempt when it holds no digit at all: `checksum` is a
// name, not a number. One that holds a digit has to earn it through the
// reference list below, like any other text.
const REFERENCE = [
  /\{\{[^}]+\}\}/g,                                            // the placeholder itself
  /§\s?[\d.]+[a-z.]*/g,                                        // §6.4.4.3
  /\bDO-\d+[A-Z]?\b/g, /\bED-\d+\b/g, /\bTQL-\d\b/g, /\bCAST-\d+\b/g, /\bRFC \d+\b/g,
  /\bTables? A-\d\b/g, /\bA-7\b/g, /\bTable 12-1\b/g,
  /\b(?:objectives?|obj) \d(?:(?:,| and) \d)*\b/g, /\bCriteria \d\b/g, /\bLevel [A-D]\b/g,
  /(^|[\s(])#\d{1,4}\b/g,                                      // an issue
  /\bV\d\d\b/g,                                                // a section of this page
  /\bLLR-[A-Z]+-\d+\b/g,                                       // a requirement id
  /\bSHA-256\b/g, /\bx86[-_]64\b/g, /\bAArch64\b/gi, /\bRISC-V\b/g,
  // a tool and its version: "clang ≥ 18", "LLVM 18", "llvm-cov 19"
  /\b(?:upstream )?(?:Apple |LLVM\/)?(?:LLVM|clang|llvm-cov|llvm-profdata|gcov)\b(?:\/clang)?(?: ≥)? \d+(?:\.\d+)*\b/g,
  /\b\d+(?:\.\d+){2,}\b/g,                                     // 1.97.1, 192.0.2.10
  /-O[0-3sz]\b/g, /-f[a-zA-Z][\w-]*\b/g, /-Z[a-z][\w-]*\b/g, /--[a-z][a-z-]*\b/g,
  /\b[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+(?:=\d+)?/g,                 // BUTTERFLY_MAX_COVERAGE_MB=1
  /\bexits? \d\b/g, /\bexit status \d\b/g,
  /\b[\w./-]+\.(?:c|h|cpp|rs|js|cjs|json|toml|yaml|yml|md|csv|sh|html|txt|log|su)\b(?::\d+(?:-\d+)?)?/g,
  /\bL\d+\b/g,                                                 // L23, a line marker the viewer draws
  /\bline-0\b/g,                                               // an Instr.origin, not a count
  /\b\d+ MiB\b/g,                                              // a cap the command itself sets
  /\bc11\b/g, /\bC\+\+\d+\b/g,
];
// "one" is deliberately absent: in English it is the indefinite article's
// cousin ("one page answers", "one command", "one line per breach") and
// banning it would force worse prose than it prevents.
const SPELLED = /\b(?:two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred|thousand|million|billion|dozen|twice|thrice|score)\b/i;
const ROMAN = /\b(?:XVIII|XVII|XVI|XIV|XIII|XII|XIX|XV|XI|XX|IX|VIII|VII|VI|IV|III|II)\b/;

/// The reason this text may not go on the page, or null.
function typedNumber(text) {
  let rest = String(text);
  // a code span with no digit in it is a name; one with a digit is not exempt
  rest = rest.replace(/`([^`]*)`/g, (m, inner) => (/[0-9]|\p{Nd}/u.test(inner) ? m : ' '));
  for (const rx of REFERENCE) rest = rest.replace(rx, ' ');
  const arabic = /.{0,30}[0-9].{0,30}/.exec(rest);
  if (arabic) return `a number is typed in: ${JSON.stringify(arabic[0].trim())}`;
  const other = /\p{Nd}/u.exec(rest);
  if (other) return `a non-ASCII digit is typed in: ${JSON.stringify(other[0])}`;
  const word = SPELLED.exec(rest);
  if (word) return `a number is spelled out: ${JSON.stringify(word[0])}`;
  const roman = ROMAN.exec(rest);
  if (roman) return `a roman numeral is typed in: ${JSON.stringify(roman[0])}`;
  return null;
}

/// Every string on the page that a reader reads as a claim.
function customerVisible(spec) {
  const out = [];
  for (const g of spec.groups) out.push([`group ${g.id} title`, g.title]);
  for (const f of spec.features) {
    out.push([`${f.id} title`, f.title]);
    out.push([`${f.id} claim`, f.claim]);
    for (const l of f.limits || []) out.push([`${f.id} limit`, l]);
    if (f.pending_reason) out.push([`${f.id} pending_reason`, f.pending_reason]);
    for (const s of f.shot || []) {
      out.push([`${f.id} shot ${s.file} caption`, s.caption]);
      if (s.requires_reason) out.push([`${f.id} shot ${s.file} requires_reason`, s.requires_reason]);
    }
    for (const c of f.cmd || []) if (c.caption) out.push([`${f.id} cmd caption`, c.caption]);
  }
  for (const n of spec.notClaimed) out.push(['not_claimed text', n.text]);
  return out;
}

test('no number is typed into anything a reader reads as a claim', () => {
  const spec = load();
  const fields = customerVisible(spec);
  assert.ok(fields.length > 100, 'the guard scans every customer-visible field, not just claims and limits');
  for (const [where, text] of fields) {
    assert.equal(typedNumber(text), null, `${where}: ${typedNumber(text)}\n    in ${JSON.stringify(String(text).slice(0, 180))}`);
  }
});

test('the number guard catches each way round it', () => {
  // the bypasses that were live (#212 B4)
  assert.match(typedNumber('clamp_altitude is `8 blocks` at -O0'), /a number is typed in/);
  assert.match(typedNumber('`28` gaps'), /a number is typed in/);
  assert.match(typedNumber('the function is eight blocks'), /spelled out/);
  assert.match(typedNumber('VIII of IX conditions are proven'), /roman numeral/);
  assert.match(typedNumber('coverage is ≥ 99 percent'), /a number is typed in/);
  assert.match(typedNumber('８ blocks'), /non-ASCII digit/);
  assert.match(typedNumber('twice as many gaps'), /spelled out/);
  assert.match(typedNumber('28 gaps across 4 kinds'), /a number is typed in/);
  // ...and lets a reference through
  for (const ok of [
    'DO-178C §6.4.4.3 and Table A-7 objectives 5, 6 and 7',
    'tracked in #164; see V28',
    'Needs upstream LLVM/clang ≥ 18 with `-fcoverage-mcdc`',
    'rust-toolchain.toml pins 1.97.1',
    'the decision at `demo.c:23`, marked L23',
    'refused with BUTTERFLY_MAX_COVERAGE_MB=1 against a 2 MiB input',
    'bound to 192.0.2.10 (RFC 5737)',
    '`clamp_altitude` is {{c.fn.clamp_altitude.blocks}} blocks at `-O0`',
    'compiler-generated instructions, among them `line-0`',
    'TQL-5 at Level A, Criteria 3',
    'one page answers the question',
  ]) assert.equal(typedNumber(ok), null, ok);
});

test('an unresolved placeholder stops the build', () => {
  assert.equal(fill('{{a.b}} gaps', {a: {b: 28}}, 'x'), '28 gaps');
  assert.throws(() => fill('{{a.c}}', {a: {b: 28}}, 'V05'), /V05: \{\{a.c\}\} does not resolve/);
  assert.throws(() => fill('{{a}}', {a: {b: 28}}, 'V05'), /does not resolve/);
  // An empty string and a non-finite number used to resolve, and rendered as
  // nothing at all: "reported as not bounded because of ." (#212 S20).
  assert.throws(() => fill('because of {{a.reason}}.', {a: {reason: ''}}, 'V07'), /does not resolve/);
  assert.throws(() => fill('{{a.pct}}%', {a: {pct: NaN}}, 'V07'), /does not resolve/);
  assert.throws(() => fill('{{a.n}}', {a: {n: Infinity}}, 'V07'), /does not resolve/);
  // ...while a legitimate zero still resolves: "0 gaps" is a claim
  assert.equal(fill('{{a.n}} gaps', {a: {n: 0}}, 'x'), '0 gaps');
  assert.equal(fill('{{a.b}}', {a: {b: false}}, 'x'), 'false');
});

test('a placeholder that names a duplicated function is refused, not answered at random', () => {
  // Two `static` functions of one name in different translation units is a
  // real shape this repository has fixtures for (#36); `fn` keeps the first,
  // and a number quoted from it would be whichever came first (#212 S22).
  const facts = {c: {fn: {helper: {blocks: 3, duplicates: 2}, checksum: {blocks: 9, duplicates: 1}}}};
  assert.throws(() => fill('{{c.fn.helper.blocks}}', facts, 'V02'), /2 functions with that name/);
  assert.equal(fill('{{c.fn.checksum.blocks}}', facts, 'V02'), '9');
});
