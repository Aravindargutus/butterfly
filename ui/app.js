// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

/* butterfly viewer. Vanilla JS, no build step, works offline.
 * Reads /api/evidence (never modified) and /api/resolutions (sidecar). */
'use strict';

// The source pane's variable marks (#53) are optional, and which way a
// reader wants them is a property of the reader and not of the evidence --
// so the choice is kept per browser beside the theme (`sl-theme`), under
// the same `sl-` prefix.
//
// It is the one switch here that defaults *on*. The marks are the visible
// half of the Variables tab, and a feature that starts hidden is a feature
// nobody finds: only an explicit '0' turns them off, and a storage that
// cannot be read at all -- a private window, a browser refusing site data --
// leaves them on rather than quietly disabling the thing it could not ask
// about.
const VAR_MARKS_KEY = 'sl-varmarks';
function readVarMarks() {
  try { return localStorage.getItem(VAR_MARKS_KEY) !== '0'; } catch { return true; }
}
function setVarMarks(on) {
  S.varMarks = !!on;
  // The session still honours a choice it could not write down; a browser
  // that refuses storage is not a reason to refuse the click.
  try { localStorage.setItem(VAR_MARKS_KEY, S.varMarks ? '1' : '0'); } catch { /* private window */ }
}
/// Whether a hover should mark anything in the source pane.
///
/// The whole rule, pure and in one place: with the marks off nothing is
/// painted -- no row classes, no order badges -- rather than something
/// fainter. A reviewer who switched them off and forgot has to be able to
/// read "hovering does nothing" off the button's own pressed state, instead
/// of concluding the evidence is empty.
function varMarksWanted(on) { return !!on && S.varMarks; }

/// Which variable's marks belong on screen, given where the pointer is and
/// where the keyboard is.
///
/// The pointer wins while it is inside a row -- a reader moving the mouse is
/// pointing at something -- and a focused row keeps its marks when the
/// pointer leaves it, because that is still where the keyboard is. Pure, so
/// the rule is pinned without a DOM (ui/vars.test.cjs).
///
/// It exists because per-event answers were wrong: each access badge used to
/// clear the marks on its own mouseleave, including when the pointer moved
/// back into its own parent row, and the row's mouseenter does not fire
/// again for a move inside itself -- so the marks stayed off until the
/// pointer left the row and came back. A focused row lost them the same way
/// (review of #53).
function varMarkTarget(st) { return st.hover != null ? st.hover : st.focus; }

/// Paint the marks for whichever variable row the reader is on, and nothing
/// else. One repaint per event, computed from the whole state, rather than
/// each event asserting its own answer about what should be on screen.
function paintVarMarks() {
  const vi = varMarkTarget(S.varMark);
  hl('var', vi, vi != null);
}

const S = {
  ev: null,            // evidence
  res: new Map(),      // key -> resolution
  fnId: null,
  tab: 'flow',
  sort: 'file',
  filter: '',
  showInstr: false,
  selBlock: null,
  focusGap: null,
  zoom: 'fit',
  mode: localStorage.getItem('sl-mode') || 'plain',   // 'plain' (no coverage colouring, default) | 'projected' | 'measured' (needs a trace)
  variants: [],        // [{name, summary}] from /api/variants
  access: { readOnly: false, requiresToken: false },  // what this server permits (#3)
  variant: null,
  evCache: new Map(),  // variant name -> evidence (for Compare)
  fileId: null,        // file view
  fold: localStorage.getItem('sl-fold') || 'all',  // file view: 'all' | 'findings' | 'uncovered'
  density: false,      // file view: instructions-per-line gutter
  lens: null,          // file view: highlighted requirement tag
  defs: new Map(),     // identifier -> [tag] from the optional tags index (#32); empty when none
  hoverFn: localStorage.getItem('sl-hover-fn') === '1',   // hover a function row -> scroll source
  hoverReq: localStorage.getItem('sl-hover-req') === '1', // hover a requirement row -> scroll source
  fnHeaders: localStorage.getItem('sl-fnhdr') === '1',     // file view: sticky function headers (off = plain source)
  varMarks: readVarMarks(),   // source pane: mark a hovered variable's declaration and accesses (#53). On unless switched off.
  varMark: { hover: null, focus: null },  // which Variables row the pointer and the keyboard are on
};

const $ = (sel, el = document) => el.querySelector(sel);
const h = (tag, attrs = {}, ...kids) => {
  const el = tag.includes('svg:') ? document.createElementNS('http://www.w3.org/2000/svg', tag.slice(4)) : document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === 'class') el.setAttribute('class', v);
    else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k === 'html') el.innerHTML = v;
    else el.setAttribute(k, v);
  }
  for (const k of kids.flat(Infinity)) {
    if (k == null || k === false) continue;
    el.append(k.nodeType ? k : document.createTextNode(String(k)));
  }
  return el;
};
const esc = s => String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const hex = n => '0x' + Number(n).toString(16);
const pct = (a, b) => b === 0 ? null : 100 * a / b;
const fmtPct = p => p == null ? '—' : (p >= 99.95 && p < 100 ? '99.9' : p.toFixed(1)) + '%';
const S2O = {
  traceable: 'Directly traceable — every instruction maps to a source statement; no additional code',
  analysed: 'Traceable with analysis — additional/compiler-generated code identified and explained (gaps resolved)',
  'not-traceable': 'Not traceable — compiler behaviour unacceptable; action required',
};
const s2oKey = f => `s2o:${f.name}`;
const s2oOf = f => resFor(s2oKey(f));
const CATS = {
  a: '6.4.4.3.a — Shortcoming in requirements-based test cases/procedures',
  b: '6.4.4.3.b — Inadequacy in software requirements',
  c: '6.4.4.3.c — Extraneous code, including dead code',
  d1: '6.4.4.3.d — Deactivated code, Category One',
  d2: '6.4.4.3.d — Deactivated code, Category Two',
  analysis: '6.4.4.2.b — Compiler-generated code justified by analysis',
};

// The header strip is the first thing a reviewer reads, and a box labelled
// "OUTCOMES —" explains nothing on its own. Each box carries a `?` with the
// definition behind its number, and opens the tab that owns it.
const HELP = {
  calls: 'How many times the instrumented run entered this function. Zero is not by itself dead code: the function may only be reachable on a path the tests never took, or through a pointer.',
  instantiations: 'Separate coverage records with this function\'s name and source range — the same code built more than once — merged into this entry. The counts beside it are their sum. Instantiations at different types have different names and are listed as functions of their own.',
  lines: 'Executable source lines the run reached, over the lines the compiler emitted code for. Blank lines, comments and declarations are not counted. This is line coverage, and it is a proxy for statement coverage (A-7 obj 7), not proof of it: several statements on one line collapse into a single result, so a covered line can still hold a statement that never ran.',
  outcomes: 'Branch outcomes taken, over outcomes present: every decision contributes both its true and its false exit. This is what decision coverage (A-7 obj 6) is measured on, and it must reach 100%.',
  mcdc: 'Conditions shown to independently affect their decision outcome, over the conditions in the function. Level A (A-7 obj 5) needs every one of them, and a condition inside a covered decision can still be missing its pair.',
  vg: 'Cyclomatic complexity of the object-code control-flow graph: the number of linearly independent paths through the compiled function. Measured on the object CFG, not the source, so it counts branches the compiler added.',
  maxcond: 'The largest number of conditions in any single decision here. It sets the size of the MC/DC obligation for that decision — n+1 test cases at best.',
  objproj: 'Object-code instructions projected as covered, over the total. Projected from source coverage through the DWARF line table: inferred, not observed. A-7 obj 9.',
  objmeas: 'Object-code instructions observed executing in the imported trace. Unlike the projection, this is measured evidence.',
  objoutcomes: 'Branch edges in the object control-flow graph observed taken in the trace, over the edges present — the object-level counterpart of the source outcome count.',
  generated: 'Instructions the compiler added that no source statement asked for: stack checks, helper calls, register spills. Not directly traceable to source (6.4.4.2.b), so each needs an explanation before A-7 obj 9 can close.',
  stack: 'Worst-case stack this function needs, taken from the compiler stack-usage files. ∞ means the bound is unknown — usually recursion or a call through a pointer.',
  gaps: 'Object code that source coverage does not account for, unresolved over total. Each gap needs a reviewer justification (6.4.4.2.b) before the source-to-object review can be signed off.',
  reviewed: 'Functions in this file carrying a recorded source-to-object review decision, over those that have object code to review.',
  dead: 'Symbols nothing in the image calls, whose address nothing takes, and that are not the entry point. Candidates only: defensive structures and deactivated code are not dead code.',
};

// Anything that navigates on click has to be reachable without a mouse, so
// it takes a button's role, the tab order, and Enter/Space.
const activatable = handler => ({
  role: 'button',
  tabindex: '0',
  onclick: handler,
  onkeydown: e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); handler(e); } },
});

/* ---------------- requirement text ---------------- */
// A `@satisfies` tag names an ID. The statement behind it lives in the
// requirements sidecar, which is thousands of entries in a real programme --
// so it is fetched for the handful on screen and never loaded whole. What
// arrives is cached by ID for the session; a requirement's text does not
// change while the page is open.
//
// `null` in the cache means "asked, and the sidecar does not have it" --
// distinct from "not asked yet", so a missing requirement is stated rather
// than retried on every hover.
//
// Scoped to the variant, and cleared when one is loaded. The server serves a
// sidecar per evidence file, so two variants may both carry `LLR-1` saying
// different things; a cache keyed on the ID alone would show the first
// variant's statement as the second's. A response that arrives after a
// switch is dropped for the same reason -- the same guard `S.defs` already
// uses for the definition index.
const REQ = new Map();
let reqPending = null;
let reqStale = [];
function resetReq() {
  REQ.clear();
  reqPending = null;
  reqStale = [];
  hideReq();
}

// Batch the IDs raised in one tick into a single request: a function with
// eight tags should cost one round trip, not eight.
function needReq(ids) {
  const want = ids.filter(id => !REQ.has(id));
  if (!want.length) return Promise.resolve();
  if (!reqPending) {
    const forVariant = S.variant;
    const pending = reqPending = { ids: new Set(), p: null };
    pending.p = Promise.resolve().then(async () => {
      const ids = [...pending.ids];
      if (reqPending === pending) reqPending = null;
      // Match the server's MAX_REQ_IDS, including functions with many tags.
      for (let offset = 0; offset < ids.length; offset += 256) {
        if (S.variant !== forVariant) return;
        const batch = ids.slice(offset, offset + 256);
        try {
          const d = await authJson(`/api/requirements${vq()}${vq() ? '&' : '?'}ids=${batch.map(encodeURIComponent).join(',')}`);
          // the variant changed while this was in flight: these statements
          // belong to a document that is no longer on screen
          if (S.variant !== forVariant) return;
          if (Array.isArray(d.stale)) reqStale = d.stale;
          for (const e of d.entries || []) REQ.set(e.id, e);
          for (const id of d.unknown || []) REQ.set(id, null);
          // a sidecar that is not there at all: remember that too, so every
          // tag on the page does not ask again
          if (d.present === false) for (const id of batch) REQ.set(id, null);
        } catch (e) {
          // A transport failure is not a "no such requirement". Caching null
          // here -- the same marker a genuinely-unknown id gets -- would make
          // the miss permanent until a full reload (BF-QA-04). Leave these ids
          // uncached so reopening the view, or switching back to it, retries
          // them once the server is reachable again.
          //
          // A 401 is not a failure to report, though: authFetch has already
          // raised the token page to re-authenticate, so this is an expected,
          // handled re-auth, not an error. Logging it is both noise and a
          // race -- authFetch shows the page before authJson finishes reading
          // the refused body, so the console.error can land after the page is
          // up, which a reviewer (and the token browser check) treats as the
          // "handled" boundary. Leave the ids uncached to retry once
          // re-authenticated, but do not report the re-auth as an error.
          if (!e.needsToken) console.error('butterfly requirements', e);
          if (S.variant !== forVariant) return;
        }
      }
    });
  }
  for (const id of want) reqPending.ids.add(id);
  return reqPending.p;
}

// The panel a requirement chip opens. Kept to one on the page: two open
// statements beside each other is two things to read and no clearer.
//
// Chip and panel are treated as one region. The panel holds the control that
// loads the rest, and hiding on the chip's own `mouseleave` made that control
// unreachable -- the pointer has to cross a gap to get to it, and Tab moves
// focus out of the chip before it can arrive (review of #56).
let reqHideTimer = null;
let reqPinned = false;
let activeReqChip = null;
// Set while closeReq() restores focus to the chip, so the chip's own focus
// handler does not immediately re-open the panel Escape just closed (BF-QA-03).
let reqSuppressOpen = false;
function reqPanel() {
  let el = document.getElementById('req-panel');
  if (!el) {
    el = h('div', { class: 'req-panel', id: 'req-panel', tabindex: '-1' });
    // moving into the panel is staying, not leaving
    el.addEventListener('mouseenter', cancelHideReq);
    el.addEventListener('mouseleave', () => { if (!reqPinned) scheduleHideReq(); });
    el.addEventListener('focusin', cancelHideReq);
    el.addEventListener('focusout', () => { if (!reqPinned) scheduleHideReq(); });
    el.addEventListener('keydown', e => { if (e.key === 'Escape') { e.stopPropagation(); closeReq(); } });
    document.body.append(el);
  }
  return el;
}
function cancelHideReq() {
  if (reqHideTimer) { clearTimeout(reqHideTimer); reqHideTimer = null; }
}
// A grace period, so the pointer can cross the gap between chip and panel.
function scheduleHideReq() {
  cancelHideReq();
  reqHideTimer = setTimeout(hideReq, 180);
}
function hideReq() {
  cancelHideReq();
  if (reqPinned) return;
  if (activeReqChip) { activeReqChip.dataset.open = '0'; activeReqChip = null; }
  const el = document.getElementById('req-panel');
  if (el) el.classList.remove('on');
}
// Pinned by keyboard or click; Escape and clicking away are the ways out.
function closeReq() {
  reqPinned = false;
  cancelHideReq();
  const el = document.getElementById('req-panel');
  if (el) el.classList.remove('on');
  const chip = activeReqChip || document.querySelector('.req.has-text[data-open="1"]');
  activeReqChip = null;
  // Restore focus to the chip, but suppress its focus-to-open so Escape does
  // not bounce the panel straight back open (BF-QA-03). focus() dispatches the
  // focus event synchronously, so the flag is only up for that one event.
  if (chip) { chip.dataset.open = '0'; reqSuppressOpen = true; chip.focus(); reqSuppressOpen = false; }
}
document.addEventListener('click', e => {
  if (!reqPinned) return;
  const el = document.getElementById('req-panel');
  if (el && !el.contains(e.target) && !(e.target.classList && e.target.classList.contains('req'))) closeReq();
});
document.addEventListener('keydown', e => { if (e.key === 'Escape' && reqPinned) closeReq(); });

async function showReq(chip, id) {
  await needReq([id]);
  // the pointer moved on while the fetch was in flight
  if (!chip.isConnected || chip !== activeReqChip || chip.dataset.open !== '1') return;
  const r = REQ.get(id);
  const el = reqPanel();
  el.innerHTML = '';
  if (!r) {
    el.append(h('div', { class: 'rp-id' }, id), h('div', { class: 'rp-none' },
      'No text for this requirement. Either the sidecar does not carry it, or none was produced — `butterfly requirements` writes it.'));
  } else {
    el.append(h('div', { class: 'rp-id' }, r.id,
      r.kind ? h('span', { class: 'rp-tag', title: r.kind === 'derived' ? 'derived: no parent requirement gave rise to it, so the safety assessment must see it (DO-178C 5.1.2.h)' : 'a low-level requirement' }, r.kind) : null,
      r.status ? h('span', { class: 'rp-tag ' + (r.status === 'implemented' ? 'ok' : '') }, r.status) : null));
    el.append(h('div', { class: 'rp-text' }, r.text || '(this requirement carries no statement)'));
    if (reqStale.length) {
      el.append(h('div', { class: 'rp-none', style: 'color:var(--unc)' }, 'Stale requirement text: ' + reqStale.join(' ')));
    }
    // The analysis around the statement is the larger half and is not wanted
    // to put one sentence beside a line, so it is a second request.
    el.append(h('button', { class: 'rp-more', onclick: async e => {
      e.stopPropagation();
      const d = await authJson(`/api/requirements${vq()}${vq() ? '&' : '?'}ids=${encodeURIComponent(id)}&full=1`).catch(() => null);
      const full = d && d.entries && d.entries[0];
      e.target.replaceWith(full ? reqDetail(full) : h('div', { class: 'rp-none' }, 'could not load the rest'));
    } }, 'rationale, safety, claim…'));
  }
  const b = chip.getBoundingClientRect();
  el.classList.add('on');
  const w = el.offsetWidth || 340;
  el.style.top = `${Math.round(Math.min(b.bottom + 6, window.innerHeight - el.offsetHeight - 8))}px`;
  el.style.left = `${Math.round(Math.min(Math.max(8, b.left), window.innerWidth - w - 8))}px`;
}

function reqDetail(r) {
  const wrap = h('div', { class: 'rp-detail' });
  if (r.parent) wrap.append(h('div', { class: 'rp-row' }, h('b', {}, 'decomposes '), r.parent));
  for (const [k, v] of Object.entries(r.detail || {})) {
    wrap.append(h('div', { class: 'rp-row' }, h('b', {}, k.replace(/_/g, ' ') + ' '), v));
  }
  const c = r.claim || {};
  if ((c.code || []).length || (c.tests || []).length) {
    // The document's assertion, not a measurement. `butterfly claims`
    // compares the two; saying so here stops the panel reading as evidence.
    wrap.append(h('div', { class: 'rp-row claim' }, h('b', {}, 'the document claims '),
      [...(c.code || []), ...(c.tests || [])].join(', '),
      h('div', { class: 'rp-none' }, 'what the document asserts — `butterfly claims` checks it against what was measured')));
  }
  if (r.source) wrap.append(h('div', { class: 'rp-row src' }, h('b', {}, 'from '), r.source));
  return wrap;
}

// One requirement tag. Hovering or focusing it shows the statement; the text
// arrives on demand, so a page of tags costs nothing until one is asked
// about.
function reqChip(id, extra) {
  const chip = h('span', {
    class: 'req has-text', tabindex: '0', role: 'button',
    'aria-label': `requirement ${id} — press Enter to read it`,
    ...(extra || {}),
  }, id);
  const open = () => {
    cancelHideReq();
    if (activeReqChip && activeReqChip !== chip) activeReqChip.dataset.open = '0';
    activeReqChip = chip;
    chip.dataset.open = '1';
    showReq(chip, id);
  };
  chip.addEventListener('mouseenter', open);
  chip.addEventListener('mouseleave', () => { if (!reqPinned) scheduleHideReq(); });
  chip.addEventListener('focus', () => { if (reqSuppressOpen) return; open(); });
  chip.addEventListener('blur', () => { if (!reqPinned) scheduleHideReq(); });
  // Click pins the panel (it stays until Escape or a click away), matching the
  // keyboard activation and the "pinned by keyboard or click" contract (BF-QA-05).
  chip.addEventListener('click', e => { e.stopPropagation(); reqPinned = true; open(); });
  // Enter or Space pins it and moves focus inside, so the detail control is
  // reachable without a pointer. Escape closes and comes back here.
  chip.addEventListener('keydown', async e => {
    if (e.key !== 'Enter' && e.key !== ' ') return;
    e.preventDefault();
    reqPinned = true;
    open();
    await needReq([id]);
    const el = document.getElementById('req-panel');
    const first = el && (el.querySelector('.rp-more') || el);
    if (first) first.focus();
  });
  return chip;
}

// Show a `?`'s explanation, positioned in viewport coordinates. The strip is
// right-aligned and wraps, so a box can sit anywhere across the header, and
// #main clips whatever leaves it -- at 1100px three of the tips used to be
// chopped in half. Fixed positioning escapes the clip; the clamp keeps the
// panel on screen whichever box opened it.
function showTip(q) {
  const tip = q.nextElementSibling;
  if (!tip) return;
  const r = q.getBoundingClientRect();
  const w = tip.offsetWidth || 270;
  tip.style.top = `${Math.round(r.bottom + 7)}px`;
  tip.style.left = `${Math.round(Math.min(Math.max(8, r.right - w), window.innerWidth - w - 8))}px`;
  tip.classList.add('on');
}
const hideTip = q => q.nextElementSibling && q.nextElementSibling.classList.remove('on');

// One metric box: the number, its label, a `?` that explains what the number
// means, and -- when a tab of the middle pane owns it -- a click that opens
// that tab. `go` is the click, `help` a key into HELP.
let tipSeq = 0;
const metricBox = (v, label, cls, help, go, goLabel) => {
  const text = help ? HELP[help] : null;
  // the panel is named so the `?` can point a screen reader at it
  const tipId = text ? `tip-${++tipSeq}` : null;
  return h('div', {
      class: 'metric ' + (cls || '') + (go ? ' go' : ''),
      title: go ? `open the ${goLabel} tab` : null,
      ...(go ? activatable(go) : {}),
    },
    h('b', {}, v),
    h('span', {}, label),
    // The `?` sits in a reserved gutter, so it cannot collide with a wide
    // value. Its own empty title stops the browser inheriting the box's
    // "open the X tab" and stacking a second tooltip over this one; focus
    // opens it too, which is what makes it reachable by keyboard and tap.
    text ? h('i', {
      class: 'q', title: '', tabindex: '0', role: 'button',
      'aria-label': `what ${label} means`, 'aria-describedby': tipId,
      onclick: e => e.stopPropagation(),
      // The `?` sits inside a box that is itself a button, so an activation
      // key pressed here would bubble and navigate -- the opposite of what
      // the control does under the mouse. Swallow the keys the parent acts
      // on, and let Escape dismiss.
      onkeydown: e => {
        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); showTip(e.currentTarget); return; }
        if (e.key === 'Escape') { hideTip(e.currentTarget); e.currentTarget.blur(); }
      },
      onkeyup: e => { if (e.key === 'Enter' || e.key === ' ') e.stopPropagation(); },
      onmouseenter: e => showTip(e.currentTarget),
      onmouseleave: e => hideTip(e.currentTarget),
      onfocus: e => showTip(e.currentTarget),
      onblur: e => hideTip(e.currentTarget),
    }, '?') : null,
    text ? h('div', { class: 'tip', role: 'tooltip', id: tipId }, text) : null);
};

/* ---------------- data helpers ---------------- */
const fn = id => S.ev.functions[id];
const file = id => S.ev.files[id];
const lineCount = (fileId, line) => { const l = file(fileId).lines[line - 1]; return l ? l.count : null; };
const resFor = key => S.res.get(key);
const lineKey = (fileId, line) => `line:${fileId}:${line}`;
// How a decision or branch is identified in a resolution key. gcov gives no
// columns, and macro expansions can place multiple decisions/branches on the
// same (line, col) site. The ordinal distinguishes them, written `#n` so it
// can never be mistaken for a column and an old key cannot accidentally match
// a new one. Mirrors Branch::site() and Decision::site() in the model.
const branchSite = b => (b.decisionOrdinal != null ? `${b.line}:#${b.decisionOrdinal}` : `${b.line}:${b.col}`);
const condKey = (fnId, d, i) => `cond:${fnId}:${branchSite(d)}:${i}`;
const branchKey = (fnId, b, o) => `branch:${fnId}:${branchSite(b)}:${o}`;
// The two outcomes, named by what the producer could establish. llvm-cov
// states each branch's polarity; gcov states a CFG edge, and which edge is
// the true one depends on the construct -- fall-through is the true path for
// `if`, and the loop *exit* for `while`/`for`. So a gcov decision keys and
// labels its outcomes A/B, and a justification never claims to be about the
// "true" outcome when nothing identified it as true (#45).
const dirKnown = b => b.directionKnown !== false;
const outcomeKeys = b => (dirKnown(b) ? ['T', 'F'] : ['A', 'B']);
const outcomeLabels = b => (dirKnown(b) ? ['true', 'false'] : ['outcome A', 'outcome B']);

// Every decision on a line, with the function it belongs to. Built once per
// evidence load: the source pane asks this for every visible line.
//
// Keyed to the evidence object itself, not reset at any call site: switching
// variants replaces S.ev, and an index left over from the previous variant
// would paint this one's lines with the other one's decisions.
let _brByLine = null, _brByLineOf = null;
function branchesOnLine(fileId, line) {
  if (!_brByLine || _brByLineOf !== S.ev) {
    _brByLineOf = S.ev;
    _brByLine = new Map();
    for (const f of S.ev.functions) {
      for (const b of f.branches || []) {
        const k = `${f.fileId}:${b.line}`;
        if (!_brByLine.has(k)) _brByLine.set(k, []);
        _brByLine.get(k).push([f, b]);
      }
    }
  }
  return _brByLine.get(`${fileId}:${line}`) || [];
}

// A line that was reached but whose decision has an outcome nobody took.
//
// `if (v > hi) return hi;` is one line holding two things: a condition that
// was evaluated and a statement that was not. The line count says 1 because
// control reached it, so painting on the count alone calls it covered -- and
// a reviewer scanning for what still needs a test scrolls past a statement
// that never ran (#74).
//
// Only an untaken *outcome* makes a line partial. Two statements sharing a
// line with no decision between them (`a(); b();`) cannot be told apart by
// anything in the evidence, and guessing there would be the same error in the
// other direction.
//
// An untaken outcome somebody has justified is closed work: the line reads as
// covered again, the way a justified uncovered line stops reading as a
// finding.
function lineIsPartial(fileId, line) {
  for (const [f, b] of branchesOnLine(fileId, line)) {
    const [k0, k1] = outcomeKeys(b);
    if (!b.trueCount && !resFor(branchKey(f.id, b, k0))) return true;
    if (!b.falseCount && !resFor(branchKey(f.id, b, k1))) return true;
  }
  return false;
}

// Which outcome went untaken, in words, for the row that shows it.
function partialWhy(fileId, line) {
  const parts = [];
  for (const [f, b] of branchesOnLine(fileId, line)) {
    const [k0, k1] = outcomeKeys(b), [w0, w1] = outcomeLabels(b);
    if (!b.trueCount && !resFor(branchKey(f.id, b, k0))) parts.push(w0);
    if (!b.falseCount && !resFor(branchKey(f.id, b, k1))) parts.push(w1);
  }
  if (!parts.length) return '';
  return `This line was reached, but ${parts.join(' and ')} never happened — so whatever this line does on that path has not run.\n`
    + 'Line coverage counts the line as executed because control arrived at it; the decision on it says otherwise.\n'
    + 'Open the MC/DC tab to record a justification, or add a test that takes it.';
}

function lineStatus(fileId, line) {
  if (plainMode()) return 'none';
  const c = lineCount(fileId, line);
  if (c == null) return 'none';
  if (c > 0) return lineIsPartial(fileId, line) ? 'part' : 'cov';
  return resFor(lineKey(fileId, line)) ? 'jus' : 'unc';
}
const sym = id => S.ev.symbols[id];
const hasMeasured = () => !!(S.ev.measured);
const measuredMode = () => S.mode === 'measured' && hasMeasured();
const plainMode = () => S.mode === 'plain';
// The file an instruction's line is in: the one the line table names, when
// the code was inlined from a header or another unit, else the function's
// own. Everything keyed by line number must use it -- line 12 of hdr.h is
// not line 12 of main.c (#317).
const insFileId = (f, ins) => ins.fileId ?? f.fileId;
// The requirement tags on an instruction's own source line. `lineRequirements`
// is per function and keyed by line of that function's OWN file, so a line of
// another file is read from the function that owns that file: the inlined
// callee, when the evidence names it and it lives there. Otherwise there is
// no per-line tag data for that file in the evidence, and the answer is
// "none" -- never the enclosing function's tags by line number alone.
function insLineTags(f, ins) {
  if (!ins.line) return [];
  const fid = insFileId(f, ins);
  if (fid === f.fileId) return (f.lineRequirements || {})[ins.line] || [];
  const inl = ins.inlined != null ? (f.inlined || [])[ins.inlined] : null;
  const callee = inl && inl.functionId != null ? fn(inl.functionId) : null;
  if (callee && callee.fileId === fid) return (callee.lineRequirements || {})[ins.line] || [];
  return [];
}
function instrStatus(f, ins) {
  if (plainMode()) return 'unk';
  if (measuredMode()) return ins.measured > 0 ? 'cov' : (ins.line && resFor(lineKey(insFileId(f, ins), ins.line)) ? 'jus' : 'unc');
  if (ins.projected === 'covered') return 'cov';
  if (ins.projected === 'uncovered') return ins.line && resFor(lineKey(insFileId(f, ins), ins.line)) ? 'jus' : 'unc';
  return 'unk';
}
function blockStatus(f, b) {
  const st = b.instrs.map(i => instrStatus(f, i));
  if (st.includes('cov')) return 'cov';
  if (st.includes('unc')) return 'unc';
  if (st.includes('jus')) return 'jus';
  return 'unk';
}
// The words beside a block's colour in the Object tab. The colour follows the
// overlay, so the words have to: with a trace loaded and Measured chosen, a
// block the trace never entered was drawn red and captioned "projected
// uncovered" -- on a block the projection calls COVERED. That contradiction
// is the one thing a reader switches to Measured to see (#164).
function blockStatusLabel(st) {
  if (st === 'jus') return 'justified';
  if (measuredMode()) return st === 'cov' ? 'measured executed' : st === 'unc' ? 'measured never executed' : 'unknown';
  return st === 'cov' ? 'projected covered' : st === 'unc' ? 'projected uncovered' : 'unknown';
}
function edgeStatus(f, b, e) {
  if (measuredMode() && e.measured != null) return e.measured > 0 ? 'cov' : 'unc';
  // An exception edge is not taken just because both its ends ran: nothing says
  // an exception was thrown along it. The backend projects it Unknown even when
  // both blocks are covered (#165), so respect e.projected rather than painting
  // it green from block coverage.
  if (e.kind === 'exception') return e.projected === 'covered' ? 'cov' : e.projected === 'uncovered' ? 'unc' : 'unk';
  const a = blockStatus(f, b), c = blockStatus(f, f.object.blocks[e.to]);
  if (a === 'cov' && c === 'cov') return 'cov';
  if (a === 'unc' || c === 'unc') return 'unc';
  if (a === 'jus' || c === 'jus') return 'jus';
  return 'unk';
}
// Every object body of a function: its own, then the variants of a C++
// constructor or destructor the compiler emits beside it (C1, D1, the
// deleting D0). Gaps and instruction totals are over all of them, so a
// variant's findings are neither lost nor counted twice (#266).
function fnObjects(f) { return [...(f.object ? [f.object] : []), ...(f.variants || [])]; }
function fnGaps(f) { return fnObjects(f).flatMap(o => o.gaps); }
// Instruction totals for the whole function: the base body plus every
// constructor/destructor variant shown in the Object tab (#266). The tab's
// toolbar and the function tiles must count what the table lists, or a fully
// covered base can read as full coverage above an uncounted variant (#283).
function objTotals(f) {
  const bodies = fnObjects(f);
  const sum = k => bodies.reduce((a, ob) => a + (ob[k] ?? 0), 0);
  const compilerGenerated = {};
  for (const ob of bodies) for (const [k, v] of Object.entries(ob.compilerGenerated || {})) compilerGenerated[k] = (compilerGenerated[k] || 0) + v;
  const compilerGeneratedTotal = Object.values(compilerGenerated).reduce((a, b) => a + b, 0);
  return {
    instrTotal: sum('instrTotal'),
    instrProjectedCovered: sum('instrProjectedCovered'),
    instrProjectedUncovered: sum('instrProjectedUncovered'),
    instrUnknown: sum('instrUnknown'),
    instrMeasuredCovered: sum('instrMeasuredCovered'),
    outcomeEdgesTotal: sum('outcomeEdgesTotal'),
    outcomeEdgesMeasured: sum('outcomeEdgesMeasured'),
    disagreements: bodies.flatMap(ob => ob.disagreements || []),
    compilerGenerated,
    compilerGeneratedTotal,
  };
}
const VARIANT_ROLE = { C1: 'complete-object constructor', C3: 'complete-object allocating constructor', CI1: 'complete-object inheriting constructor', D0: 'deleting destructor', D1: 'complete-object destructor' };
function unresolvedGaps(f) { return fnGaps(f).filter(g => !resFor(g.key)).length; }
/// A function with no coverage record of its own (#273): a closure rustc gave
/// none, a lambda thunk clang gave none. Its `count` of 0 means "not
/// counted", not "never ran", and its 0/0 lines mean the counts on its lines
/// are another function's. Nothing about it reads executed, unexecuted or
/// covered.
const unrecorded = f => f.unrecordedWithin != null;

function fnStats(f) {
  const br = f.branches.length * 2;
  const brc = f.branches.reduce((n, b) => n + (b.trueCount > 0) + (b.falseCount > 0), 0);
  const conds = f.decisions.reduce((n, d) => n + d.conditions.length, 0);
  const condc = f.decisions.reduce((n, d) => n + d.conditions.filter(c => c.independenceCovered).length, 0);
  return { linesPct: pct(f.linesCovered, f.linesTotal), br, brc, conds, condc, gaps: fnGaps(f).length, unresolved: unresolvedGaps(f) };
}

/* ---------------- access ----------------
   The server may require a bearer token on every API route. It reaches the
   page one of two ways, and neither puts it in a request:
     - typed into the token page below, for a token the operator chose;
     - as #token= in the URL butterfly prints for a token it minted.
   The fragment is the part of a URL a browser never sends, so the token is
   not in the request line, a proxy log or a Referer. It used to be ?token=,
   which went to the server in clear on the very first request, before this
   script could move it anywhere (#65). We keep it for the tab and clear the
   fragment before any request is made, so it is not left in history,
   bookmarks or a screenshot. A ?token= is deliberately NOT read: the server
   refuses such a URL, because by then the token has already travelled. */
const TOKEN_KEY = 'sl-token';
(function captureToken() {
  try {
    if (!location.hash.startsWith('#token=')) return;
    // Everything after `#token=` is the token. It is not a query string, so
    // `+`, `&` and `=` are the token's own characters -- URLSearchParams
    // turned `+` into a space and cut at `&`. Percent-escapes are decoded (a
    // `%` in the token is written %25); a value that is not valid
    // percent-encoding is taken as it stands (review of #65).
    const raw = location.hash.slice('#token='.length);
    let t = raw;
    try { t = decodeURIComponent(raw); } catch (e) { /* literal */ }
    history.replaceState(null, '', location.pathname + location.search);
    if (t) sessionStorage.setItem(TOKEN_KEY, t);
  } catch (e) { /* private mode: the header just won't be sent */ }
})();
/* The page shown when the server asks for a token: says what happened and
   takes the token in a password field, so an operator's own token never has
   to be written into a URL at all. */
let tokenPageShown = false;
// The viewer's document- and window-level listeners are registered with this
// signal (init() makes it). They outlive the body they were written for: on a
// 401 after the viewer had loaded, '/' was still the search shortcut, so it
// swallowed the keystroke and then threw on a #search that was no longer
// there -- a token with a slash in it lost its slashes on the way in -- and
// every click threw on the missing #prov (review of #65).
let viewerListeners = null;
function tokenPage(target) {
  // once: a 401 on every request in flight must not redraw it over itself
  if (tokenPageShown) return;
  tokenPageShown = true;
  // Put the viewer away before replacing it. The requirement panel's
  // handlers live at module scope and only act while it is pinned.
  if (viewerListeners) viewerListeners.abort();
  reqPinned = false;
  cancelHideReq();
  let had = false;
  try { had = !!sessionStorage.getItem(TOKEN_KEY); sessionStorage.removeItem(TOKEN_KEY); } catch (e) { }
  target.innerHTML = `<div style="max-width:46rem;margin:12vh auto;padding:0 1.5rem;font:15px/1.6 ui-sans-serif,system-ui,sans-serif;color:#c9d1d9">
      <h1 style="font-size:1.4rem;margin:0 0 .75rem">This viewer needs an access token</h1>
      <p style="margin:0 0 1rem">${had ? 'The token this tab had was not accepted. ' : ''}butterfly was started with a token, so every request must carry it.</p>
      <form id="tokenForm" style="display:flex;gap:.5rem;margin:0 0 1rem">
        <input id="tokenInput" type="password" autocomplete="off" spellcheck="false" placeholder="access token" aria-label="access token"
          style="flex:1;padding:.4rem .6rem;font:inherit;background:#0d1117;color:inherit;border:1px solid #30363d;border-radius:6px">
        <button type="submit" style="padding:.4rem 1rem;font:inherit">Open</button>
      </form>
      <p style="margin:0 0 1rem">If butterfly minted the token, it printed a link to open instead. The token is kept for this tab only.</p>
      <p style="margin:0;color:#8b949e">If you do not have it, ask whoever started the server.</p>
    </div>`;
  const form = document.getElementById('tokenForm');
  if (form) form.addEventListener('submit', ev => {
    ev.preventDefault();
    const t = (document.getElementById('tokenInput') || {}).value || '';
    if (!t) return;
    try { sessionStorage.setItem(TOKEN_KEY, t); } catch (e) { }
    location.reload();
  });
}
function authFetch(url, opts = {}) {
  let t = null;
  try { t = sessionStorage.getItem(TOKEN_KEY); } catch (e) { }
  const headers = Object.assign({}, opts.headers, t ? { authorization: 'Bearer ' + t } : {});
  return fetch(url, Object.assign({}, opts, { headers })).then(r => {
    // A 401 after the viewer has loaded -- the server restarted with a new
    // token, say -- means the one this tab holds is no longer good. A read
    // goes to the token page, which forgets it. A write keeps its own
    // message, so a reviewer's unsaved justification is not thrown away
    // with the page (review of #65).
    if (r.status === 401 && (!opts.method || opts.method.toUpperCase() === 'GET')) tokenPage(document.body);
    return r;
  });
}
/* Every GET of a JSON API goes through here. A refusal is plain text, so
   parsing it as JSON turns "you need a token" into a syntax error somewhere
   else entirely -- which is what a bare fetch of a second variant did
   (review of #24). Check the status, then parse. */
async function authJson(url) {
  const r = await authFetch(url);
  if (!r.ok) {
    const e = new Error('butterfly refused ' + url + ' (' + r.status + '): ' + (await r.text()).trim());
    e.needsToken = r.status === 401;
    throw e;
  }
  return r.json();
}

/* ---------------- init ---------------- */
async function init() {
  // Made before anything is fetched: if a 401 brings up the token page while
  // this is still loading, the signal is already aborted, and a listener
  // added with an aborted signal is never added at all.
  viewerListeners = new AbortController();
  const on = { signal: viewerListeners.signal };
  const theme = localStorage.getItem('sl-theme');
  if (theme) document.documentElement.dataset.theme = theme;

  const boot = await authJson('/api/variants');
  S.variants = boot.variants || [];
  S.access = boot.access || { readOnly: false, requiresToken: false };
  const wanted = localStorage.getItem('sl-variant');
  S.variant = (S.variants.find(v => v.name === wanted) || S.variants[0] || { name: null }).name;
  // At startup there is no prior view to keep, so a load failure is fatal:
  // re-throw into init()'s handler rather than rendering an empty page.
  if (!await loadVariant(S.variant, false)) throw new Error('butterfly could not load the initial evidence');

  $('#brand').onclick = () => navigate('#/overview');
  $('#themeBtn').onclick = () => {
    const t = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light';
    document.documentElement.dataset.theme = t;
    localStorage.setItem('sl-theme', t);
  };
  const rb = $('#reportBtn');
  if (rb) rb.onclick = async () => {
    // Fetched rather than linked so the token travels in a header. The tab
    // is opened inside the click, before any await: a popup blocker permits
    // window.open only while the user's activation is fresh, and a large
    // report can take longer than that to build.
    const tab = window.open('', '_blank');
    const r = await authFetch('/api/report' + vq());
    if (!r.ok) { if (tab) tab.close(); alert('butterfly could not build the report: ' + (await r.text())); return; }
    const url = URL.createObjectURL(await r.blob());
    if (tab) tab.location = url; else window.open(url, '_blank');
  };
  // Go-to-definition. Delegated once, because `highlight()` returns HTML that
  // is set with innerHTML, so the spans it makes carry no handlers of their
  // own -- and this covers both source panes without either knowing about it.
  document.addEventListener('click', e => {
    const d = e.target.closest && e.target.closest('.def');
    if (!d) return;
    // A definition we cannot open -- one in a header this evidence does not
    // carry -- swallows the click rather than letting the row handler send
    // the reader somewhere unrelated. It is marked and explained by its
    // tooltip; going nowhere is the honest outcome.
    e.preventDefault();
    e.stopPropagation();
    if (d.dataset.def) gotoDef(d.dataset.def);
    // Capture phase: a document-level listener otherwise runs LAST in the
    // bubble, after the source row's own click handler has already navigated
    // somewhere else. Catching it on the way down is what makes the
    // identifier the click target rather than the line.
  }, { capture: true, ...on });

  $('#provBtn').onclick = () => { const p = $('#prov'); p.classList.toggle('hidden'); if (!p.classList.contains('hidden')) renderProvenance(p); };
  $('#sort').value = S.sort;
  $('#sort').onchange = e => { S.sort = e.target.value; renderSidebar(); };
  $('#search').oninput = e => { S.filter = e.target.value.trim().toLowerCase(); renderSidebar(); };
  document.addEventListener('keydown', viewerKeydown, on);
  document.addEventListener('click', e => { const p = $('#prov'); if (p && !e.target.closest('#prov') && !e.target.closest('#provBtn')) p.classList.add('hidden'); }, on);
  window.addEventListener('hashchange', route, on);

  renderTop();
  renderSidebar();
  renderCaveat();
  route();
}

// Where typing goes, so a keystroke there is text and not a shortcut.
function isEditable(el) {
  return !!el && (!!el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName || ''));
}
// The viewer's keyboard shortcuts. Escape closes what is open from anywhere;
// the rest are single characters, so they stand down while the reader is
// typing -- in the search box, a review dialog or the token field. '/' used
// to be tested before that, so a slash typed into a justification moved
// focus to the search box instead of going into the text (review of #65).
function viewerKeydown(e) {
  if (e.key === 'Escape') { closeModal(); const p = $('#prov'); if (p) p.classList.add('hidden'); }
  if (isEditable(e.target) || isEditable(document.activeElement)) return;
  const search = $('#search');
  if (e.key === '/' && search) { e.preventDefault(); search.focus(); return; }
  if (e.key === 'f') { if (S.fnId != null) navigate(`#/file/${fn(S.fnId).fileId}/L${fn(S.fnId).lineStart}`); else if (S.fileId != null && S.ev.files[S.fileId].functionIds.length) navigate(`#/fn/${S.ev.files[S.fileId].functionIds[0]}`); }
  if (S.fileId != null && S.fnId == null && (e.key === ']' || e.key === '[')) jumpUncovered(e.key === ']' ? 1 : -1);
  // 1-9 pick the first nine tabs; 0 is the tenth, not a tenth-of-nothing.
  if (/^[0-9]$/.test(e.key) && S.fnId != null) { const t = TABS[(+e.key + 9) % 10]; if (t) setTab(t); }
  if ((e.key === 'j' || e.key === 'k') && S.fnId != null) {
    const ids = visibleFnIds(); const i = ids.indexOf(S.fnId);
    const n = ids[(i + (e.key === 'j' ? 1 : -1) + ids.length) % ids.length];
    if (n != null) navigate(`#/fn/${n}`);
  }
}

// The write target: reviews POST to ?v=<the variant on screen>. It is only
// ever the committed S.variant, never a variant still being loaded.
const vq = () => S.variant ? `?v=${encodeURIComponent(S.variant)}` : '';
// The query for a specific requested name, used while its evidence is loading
// and is not yet the active variant.
const vqOf = name => name ? `?v=${encodeURIComponent(name)}` : '';

// A variant switch is not atomic, so it is made to commit atomically (BF-QA-01).
// The requested name is kept separate from the active S.variant: its evidence
// and resolutions are fetched by name, and only when both arrive for the
// still-current selection are the active variant, evidence and resolutions
// swapped in together. Until then S.variant -- the write target for reviews --
// keeps naming the build actually on screen, so a review can never be saved
// against a variant that is not displayed.
//   * A superseded load (a newer selection started) commits nothing and does
//     not render.
//   * A failed load (evidence/resolutions could not be fetched) commits nothing
//     and leaves the current view and write target untouched.
// Every call takes the next generation number, so an out-of-order or A->B->A
// response is recognised as stale. Returns true only when it committed.
let variantGen = 0;
async function loadVariant(name, rerender = true) {
  const gen = ++variantGen;
  let ev, res;
  try {
    // Fetch by the requested name, not S.variant: the active variant has not
    // changed yet, and must not until these succeed.
    [ev, res] = await Promise.all([authJson('/api/evidence' + vqOf(name)), authJson('/api/resolutions' + vqOf(name))]);
  } catch (e) {
    console.error('butterfly variant', e);
    // Only tell the user when there was a working view to keep; at startup a
    // failure surfaces through init()'s fatal handler instead.
    if (gen === variantGen && S.ev) alert(`butterfly could not load variant "${name}". The view is unchanged.`);
    return false;
  }
  // A newer selection started while these were in flight: drop this one whole.
  if (gen !== variantGen) return false;
  // Commit identity and data together. Close any open resolution modal from
  // the previous variant so a reviewer cannot accidentally save it against
  // the variant that just took over.
  closeModal();
  S.variant = name;
  if (name) localStorage.setItem('sl-variant', name);
  // Requirement text is per variant: drop the last one's now that a new variant
  // is actually in effect -- not before, so a failed load never wipes the cache
  // of the variant still on screen.
  resetReq();
  S.ev = ev; S.evCache.set(name, ev);
  S.res = new Map((res.entries || []).map(e => [e.key, e]));
  if (S.mode === 'measured' && !ev.measured) S.mode = 'plain';
  S.fresh = null;
  loadFreshness(name, gen);
  // The definition index is optional in every direction: no sidecar, no ctags,
  // a stale index -- all mean an empty map and no jump links, never a broken
  // page. Fetched without blocking the render, and dropped if a newer variant
  // has since taken over (its ids would not match the new evidence).
  S.defs = new Map();
  authJson('/api/tags' + vqOf(name)).then(t => {
    if (gen !== variantGen) return;
    const m = new Map();
    for (const e of t.entries || []) {
      if (!m.has(e.name)) m.set(e.name, []);
      m.get(e.name).push(e);
    }
    S.defs = m;
    // Whatever is already on screen was rendered without links, so redraw it.
    if (m.size) route();
  }).catch(() => {});
  if (rerender) { renderTop(); renderSidebar(); renderCaveat(); route(); }
  return true;
}
/* switch variant, keeping the current function by name */
async function switchVariant(name) {
  const cur = S.fnId != null ? fn(S.fnId).name : null;
  // Do not render on a superseded or failed load: the view -- and the variant a
  // review would save against -- must stay consistent with what actually loaded.
  if (!await loadVariant(name, false)) return;
  renderTop(); renderSidebar(); renderCaveat();
  if (cur != null) { const f = S.ev.functions.find(x => x.name === cur); if (f) { S.fnId = f.id; location.hash = `#/fn/${f.id}/${S.tab}`; } else navigate('#/overview'); }
  route();
}
async function allVariants() {
  // Compare loads the variants the user has not opened yet. This was a bare
  // fetch, so with --token every one of them came back 401 and the plaintext
  // refusal was parsed as JSON (review of #24).
  for (const v of S.variants) if (!S.evCache.has(v.name)) S.evCache.set(v.name, await authJson(`/api/evidence?v=${encodeURIComponent(v.name)}`));
  return S.variants.map(v => [v.name, S.evCache.get(v.name)]);
}
function navigate(hash) { if (location.hash === hash) route(); else location.hash = hash; }
// A sidebar badge is a shortcut into the tab that explains its number, not
// decoration on the row. It has to stop the row's own click, or the row would
// navigate second and drop the tab, landing the reader on Flow instead.
const badgeTo = (id, tab) => e => { e.stopPropagation(); navigate(`#/fn/${id}/${tab}`); };
function route() {
  // The token page has replaced the viewer; a late response has nothing to
  // draw into.
  if (!$('#content')) return;
  try { routeInner(); } catch (e) { console.error(e); $('#content').innerHTML = `<div class="empty" style="color:var(--unc);text-align:left"><b>View failed to render</b><pre style="white-space:pre-wrap">${esc(e.stack || e)}</pre></div>`; }
}
// Valid tab segments, plus renames so old links keep working.
// Order matters twice over: it is the order the tabs are drawn in, and the
// number key that selects each one. `variables` goes beside `data` -- the
// two are the same question about different storage, globals there and
// parameters and locals here -- which moves `changes` to 9 and takes
// `requirements` past the nine digits, so `0` selects the tenth (README).
const TABS = ['flow', 'calls', 'object', 'gaps', 'mcdc', 'stack', 'data', 'variables', 'changes', 'requirements'];
const TAB_ALIAS = { butterfly: 'calls' };
const tabOf = t => TABS.includes(t) ? t : (TAB_ALIAS[t] || 'flow');

function routeInner() {
  const m = location.hash.match(/^#\/fn\/(\d+)(?:\/(\w+))?/);
  if (location.hash.startsWith('#/compare')) { S.fnId = null; renderCompare(); renderSidebar(true); return; }
  const fm = location.hash.match(/^#\/file\/(\d+)(?:\/L(\d+))?/);
  if (fm && file(+fm[1])) { S.fnId = null; S.fileId = +fm[1]; renderFile(+fm[1], fm[2] ? +fm[2] : null); renderSidebar(true); return; }
  if (m && fn(+m[1])) {
    if (S.fnId !== +m[1]) { S.selBlock = null; S.focusGap = null; S.zoom = 'fit'; }
    S.fnId = +m[1];
    if (m[2]) S.tab = tabOf(m[2]);
    renderFunction();
  } else {
    S.fnId = null;
    renderOverview();
  }
  renderSidebar(true);
}
function setTab(t) { S.tab = t; location.hash = `#/fn/${S.fnId}/${t}`; }

/* ---------------- top bar ---------------- */
function renderTop() {
  const s = S.ev.summary;
  const roots = new Set(S.ev.files.map(f => f.display.split('/')[0]));
  $('#project').textContent = S.ev.provenance.buildInfo.project || (roots.size === 1 ? [...roots][0] : `${s.files} files`);
  const vbar = $('#variants'); vbar.innerHTML = '';
  if (S.variants.length > 1) {
    const seg = h('div', { class: 'seg', title: 'build variant (exploratory build)' });
    for (const v of S.variants) seg.append(h('button', { class: v.name === S.variant ? 'on' : '', onclick: () => switchVariant(v.name) }, v.name));
    vbar.append(seg, h('button', { class: 'ghost', onclick: () => navigate('#/compare') }, 'Compare'));
  }
  const chips = $('#chips'); chips.innerHTML = '';
  const chip = (label, val, cls) => chips.append(h('span', { class: 'chip', title: label }, h('i', { class: 'dot', style: `background: var(--${cls})` }), h('span', { class: 'lbl' }, label), h('b', {}, val)));
  chip('Lines', fmtPct(pct(s.linesCovered, s.linesTotal)), 'cov');
  chip('Branches', fmtPct(pct(s.branchesCovered, s.branchesTotal)), 'cov');
  chip('MC/DC', s.decisionsTotal ? `${s.decisionsMcdcCovered}/${s.decisionsTotal}` : '—', 'accent-2');
  chip('Object (projected)', fmtPct(pct(s.instructionsProjectedCovered, s.instructionsTotal)), 'jus');
  const unresolved = S.ev.functions.reduce((n, f) => n + unresolvedGaps(f), 0);
  chip('Gaps unresolved', `${unresolved}/${s.gapsTotal}`, unresolved ? 'unc' : 'cov');
  if (hasMeasured()) chip('Object (measured)', fmtPct(pct(s.instructionsMeasuredCovered, s.instructionsTotal)), 'accent');
  if (S.fresh && S.fresh.stale) chip(S.ev.changes ? 'Change history' : 'Requirements', 'stale', 'unc');
  else if (S.fresh && S.fresh.unknown) chip('Change history', 'unknown', 'unc');
}

// The footer is the standing statement of what the colours on the page mean,
// so it has to follow the overlay: it used to print the first provenance
// caveat whatever was selected, and read "Projected, not measured" above
// blocks the Measured overlay had just coloured from a trace (#209).
//
// A pure function of its arguments, so every combination can be tested
// without a page. `measured` is `ev.measured` or nothing.
//
// The branches are the branches of `instrStatus()`, in the same order and on
// the same conditions, because the footer's whole job is to say what that
// function did to the colours:
//
//   'plain'                     -> no overlay
//   'measured' AND a trace      -> measured   (`measuredMode()`)
//   ANYTHING ELSE               -> projected
//
// The last line is not a formality. `S.mode` comes from localStorage, so a
// value left there by an older build -- or a 'measured' that survived into
// evidence with no trace -- is not 'plain' and is not valid-measured, and
// `instrStatus()` colours it PROJECTED. The catch-all used to be the
// no-overlay branch, which put projected colours under a footer denying that
// any overlay was applied: #209 inverted (#212 S17). `loadVariant()` also
// coerces a remembered 'measured' to 'plain' when the evidence carries no
// trace, so the page itself does not reach the fall-through in that case.
//
// What ingest stamps as invalidating is recognised the way the exported
// report recognises it (crates/view/src/report.rs), and is shown ahead of
// the overlay statement in every mode -- a changed baseline matters with no
// overlay at all.
const SERIOUS_CAVEAT = /^(BASELINE CHANGED|EXPLORATORY)/;
const PROJECTED_CAVEAT = "Object-code coverage is PROJECTED from source-level coverage through the DWARF line table; it is not measured. Treat green instructions as 'implied by source coverage', not as executed.";
function caveatParts(mode, measured, caveats) {
  caveats = caveats || [];
  const parts = caveats.filter(c => SERIOUS_CAVEAT.test(c)).map(c => ({
    lead: c.startsWith('BASELINE CHANGED') ? 'Baseline changed' : 'Exploratory build', text: c, serious: true }));
  if (mode === 'measured' && measured) {
    const from = [measured.format, measured.source].filter(Boolean).join(': ');
    // The trace's OWN caveats (crates/model Measured.caveats, written by
    // crates/ingest/src/trace.rs: a block whose extent had to be
    // reconstructed from the disassembly, executed blocks with no in_asm
    // listing). The footer used to say "Whether an instruction executed is
    // exact" over exactly those (#212 S16); where the trace says otherwise,
    // the trace is what the footer reports.
    const own = (measured.caveats || []).filter(Boolean);
    const limits = own.length
      ? `Execution counts from an emulator trace are an upper bound, and the trace states its own limits on what it recorded: ${own.join(' ')}`
      : 'Whether an instruction executed is exact; execution counts from an emulator trace are an upper bound.';
    parts.push({ lead: 'Measured, not projected',
      text: `Object-code colouring is MEASURED from the imported execution trace${from ? ` (${from})` : ''}; it shows the traced run only. ${limits} An emulator is not the target, and measured coverage is not the source-to-object correspondence review (A-7 obj 9), which stays a human review.` });
  } else if (mode === 'plain') {
    parts.push({ lead: 'No coverage overlay',
      text: 'No coverage overlay is applied: the colours on this page say nothing about what executed. Projected colours object code from source coverage through the DWARF line table'
        + (measured ? '; Measured colours it from the imported execution trace.' : '; no execution trace was imported, so there is no measured overlay.') });
  } else {
    // ingest's own sentence when the evidence carries it, so the footer and
    // the provenance panel cannot drift apart
    const own = caveats.find(c => /\bPROJECTED\b/.test(c) && !SERIOUS_CAVEAT.test(c));
    parts.push({ lead: 'Projected, not measured',
      text: (own || PROJECTED_CAVEAT) + (measured ? ' An execution trace was imported: a Measured overlay is available.' : '') });
  }
  return parts;
}
// The one place the overlay changes by hand, so the footer cannot be left
// describing the overlay that was on screen a click ago.
function setMode(mode) { S.mode = mode; localStorage.setItem('sl-mode', mode); renderCaveat(); }
function renderCaveat() {
  const parts = caveatParts(S.mode, S.ev.measured, S.ev.provenance.caveats);
  // `stmt`, not `t`: `.t` is the SYNTAX-TOKEN class (app.css `.t { color:
  // #82aaff }`), picked here as a layout hook without noticing it already
  // carried a colour, so the one sentence on the page whose job is to say
  // what the colours mean was itself rendered in a semantic colour -- in a
  // tool where blue is a token and also reads as an accent (#217). Its own
  // class inherits `#caveat`'s `var(--fg-2)` and cannot collide as the token
  // palette changes.
  $('#caveat').innerHTML = parts.map(p => `<span class="w${p.serious ? ' bad' : ''}">${esc(p.lead)}</span><span class="stmt">${esc(p.text)}</span>`).join('');
}
const stale = f => !!(S.ev.baseline && s2oOf(f) && (S.ev.baseline.functionsSourceChanged.includes(f.name) || S.ev.baseline.functionsObjectChanged.includes(f.name) || S.ev.baseline.status === 'changed'));
const staleWhy = f => { const b = S.ev.baseline; if (!b) return ''; if (b.functionsSourceChanged.includes(f.name)) return 'source changed since the baseline'; if (b.functionsObjectChanged.includes(f.name)) return 'object code changed for unchanged source (compiler/options)'; return 'compiler, options or constructs changed since the baseline'; };

// Which build of butterfly wrote the evidence (#117). The version names a
// release; the commit names the build. The same words as
// `Provenance::tool_build` in crates/model, and `butterfly --version`:
// `commit <id>, clean tree` / `tree modified at build` / `tree state
// unknown`; `commit unknown` when the build could not establish it; and
// `commit not recorded` for evidence written before the field existed,
// which never reads as a clean build.
function toolBuild(pv) {
  const commit = pv.toolCommit || '';
  if (commit === '') return 'commit not recorded';
  if (commit === 'unknown') return 'commit unknown';
  const tree = pv.toolDirty === true ? 'tree modified at build' : pv.toolDirty === false ? 'clean tree' : 'tree state unknown';
  return `commit ${commit}, ${tree}`;
}

function renderProvenance(p) {
  const pv = S.ev.provenance;
  p.innerHTML = '';
  p.append(h('h3', {}, 'Provenance'));
  const dl = h('dl', { class: 'kv' });
  const kv = (k, v) => dl.append(h('dt', {}, k), h('dd', {}, v));
  kv('Tool', `${pv.tool} ${pv.toolVersion}`);
  kv('Tool commit', toolBuild(pv));
  kv('Created', new Date(pv.createdUnix * 1000).toISOString());
  kv('Host', pv.host);
  kv('Coverage export', `llvm-cov json ${pv.coverageExportVersion}`);
  kv('Object format', `${pv.objectFormat} / ${pv.arch}`);
  for (const [k, v] of Object.entries(pv.buildInfo)) kv(k, v);
  for (const t of pv.externalTools || []) kv(`Tool: ${t.name}`, `${t.path} — ${t.version || 'version not recorded'}`);
  p.append(dl, h('h3', { style: 'margin-top:14px' }, 'Inputs (SHA-256)'));
  const t = h('table', { class: 'tbl' }, h('tr', {}, h('th', {}, 'role'), h('th', {}, 'path'), h('th', {}, 'sha256'), h('th', {}, 'bytes')));
  for (const i of pv.inputs) t.append(h('tr', {}, h('td', {}, i.role), h('td', { class: 'mono' }, i.path.split('/').slice(-2).join('/')), h('td', { class: 'mono', title: i.sha256 }, i.sha256.slice(0, 16) + '…'), h('td', {}, i.bytes)));
  p.append(t, h('h3', { style: 'margin-top:14px' }, 'Caveats'), h('ul', { class: 'caveat-list' }, pv.caveats.map(c => h('li', {}, c))),
    h('p', { class: 'muted', style: 'margin-top:14px;font-size:11px;border-top:1px solid var(--line);padding-top:8px' },
      `butterfly ${pv.toolVersion} · free and open source under MIT OR Apache-2.0 · evidence and reports are your project's data.`));
}

/* ---------------- sidebar ---------------- */
function visibleFnIds() {
  let fns = S.ev.functions.filter(f => !S.filter || f.name.toLowerCase().includes(S.filter) || file(f.fileId).display.toLowerCase().includes(S.filter));
  const st = new Map(fns.map(f => [f.id, fnStats(f)]));
  const cmp = {
    file: (a, b) => a.fileId - b.fileId || a.lineStart - b.lineStart,
    name: (a, b) => a.name.localeCompare(b.name),
    cov: (a, b) => (st.get(a.id).linesPct ?? 101) - (st.get(b.id).linesPct ?? 101) || a.name.localeCompare(b.name),
    cyc: (a, b) => (b.complexity.cyclomaticObject ?? 0) - (a.complexity.cyclomaticObject ?? 0) || a.name.localeCompare(b.name),
    gaps: (a, b) => st.get(b.id).unresolved - st.get(a.id).unresolved || st.get(b.id).gaps - st.get(a.id).gaps || a.name.localeCompare(b.name),
  }[S.sort];
  fns.sort(cmp);
  return fns.map(f => f.id);
}
const collapsed = new Set();
function renderSidebar(onlyActive = false) {
  const list = $('#fnlist');
  if (onlyActive && list.childElementCount) {
    list.querySelectorAll('.fn-row').forEach(r => r.classList.toggle('active', +r.dataset.id === S.fnId));
    const a = list.querySelector('.fn-row.active'); if (a) a.scrollIntoView({ block: 'nearest' });
    return;
  }
  list.innerHTML = '';
  const ids = visibleFnIds();
  const byFile = S.sort === 'file';
  let group = null, lastFile = -1;
  for (const id of ids) {
    const f = fn(id); const st = fnStats(f);
    if (byFile && f.fileId !== lastFile) {
      lastFile = f.fileId;
      const fl = file(f.fileId);
      group = h('div', { class: 'file-group' + (collapsed.has(f.fileId) ? ' collapsed' : '') },
        h('div', { class: 'file-head' + (S.fileId === f.fileId && S.fnId == null ? ' active' : '') },
          h('span', { class: 'tri', title: 'collapse', onclick: e => { e.stopPropagation(); collapsed.has(f.fileId) ? collapsed.delete(f.fileId) : collapsed.add(f.fileId); group.classList.toggle('collapsed'); } }, '▾'),
          h('span', { style: 'flex:1;overflow:hidden;text-overflow:ellipsis;cursor:pointer', title: 'open file view (f)', onclick: () => navigate(`#/file/${f.fileId}`) }, fl.display),
          h('span', { class: 'badge' }, fmtPct(pct(fl.linesCovered, fl.linesTotal)))));
      list.append(group);
    }
    const p = st.linesPct;
    const badges = [];
    if (unrecorded(f)) badges.push(h('span', { class: 'badge go amber', title: 'no coverage record of its own: whether it ran is not established — open the Gaps tab', ...activatable(badgeTo(id, 'gaps')) }, 'no record'));
    else if (f.count === 0) badges.push(h('span', { class: 'badge go red', title: 'never executed — open the Calls tab', ...activatable(badgeTo(id, 'calls')) }, '0×'));
    if (st.conds) badges.push(h('span', { class: 'badge go ' + (st.condc === st.conds ? 'green' : 'red'), title: 'MC/DC conditions with independence pair — open the MC/DC tab', ...activatable(badgeTo(id, 'mcdc')) }, `MC/DC ${st.condc}/${st.conds}`));
    if (f.complexity.cyclomaticObject != null) badges.push(h('span', { class: 'badge go', title: 'cyclomatic complexity (object CFG) — open the Flow tab', ...activatable(badgeTo(id, 'flow')) }, `v(G) ${f.complexity.cyclomaticObject}`));
    if (st.gaps) badges.push(h('span', { class: 'badge go ' + (st.unresolved ? 'amber' : ''), title: 'object-code gaps (unresolved/total) — open the Gaps tab', ...activatable(badgeTo(id, 'gaps')) }, `${st.unresolved}/${st.gaps} gaps`));
    const row = h('div', { class: 'fn-row' + (id === S.fnId ? ' active' : ''), 'data-id': id, onclick: () => navigate(`#/fn/${id}`) },
      h('span', { class: 'name', title: f.name }, f.name),
      h('span', { class: 'badges' }, badges),
      h('div', { class: 'bar ' + (p == null ? '' : p < 50 ? 'unc' : p < 100 ? 'mid' : '') }, h('i', { style: `width:${p ?? 0}%` })));
    (byFile ? group : list).append(row);
  }
  if (!ids.length) list.append(h('div', { class: 'empty' }, 'No functions match.'));
}

/* ---------------- overview ---------------- */

/// What to test next, ranked by what one test would close (#44).
///
/// Mirrors crates/ingest/src/worklist.rs, and must keep doing so: the CLI
/// summary and this panel are two views of one worklist, and a reader who
/// sees different orders in them will trust neither. Test obligations only:
/// unresolved gaps are reviewer work (a justification, 6.4.4.3) and live in
/// the Gaps tab, where writing one is a click away.
function worklist() {
  const out = [];
  const condText = (fl, d, i) => {
    const c = d.conditions[i];
    const line = fl.text[c.line - 1] || '';
    const next = d.conditions[i + 1];
    const end = next && next.line === c.line ? next.col - 1 : (c.line === d.lineEnd ? d.colEnd - 1 : line.length);
    let t = line.slice(c.col - 1, end).trim().replace(/(\&\&|\|\|)\s*$/, '').trim();
    const bal = str => [...str].reduce((n, ch) => n + (ch === '(' ? 1 : ch === ')' ? -1 : 0), 0);
    while (t.endsWith(')') && bal(t) < 0) t = t.slice(0, -1).trim();
    while (t.startsWith('(') && bal(t) > 0) t = t.slice(1).trim();
    return t || `condition ${i + 1}`;
  };
  for (const f of S.ev.functions) {
    // no record: its 0 is "not counted", and its gaps are reviewer work, not
    // a test obligation (mirrors worklist.rs, #273)
    if (unrecorded(f)) continue;
    if (f.count === 0) {
      if (resFor(`fn:${f.id}`) || resFor(`dead:${f.name}`)) continue;
      // only *open* items score: a justified line, outcome or condition is
      // closed work whichever function it sits in (mirrors worklist.rs)
      const wfl = file(f.fileId);
      let linesOpen = 0;
      for (let ln = f.lineStart; ln <= f.lineEnd; ln++) {
        if (wfl.lines[ln - 1] && wfl.lines[ln - 1].count != null && !resFor(`line:${f.fileId}:${ln}`)) linesOpen++;
      }
      const outcomesOpen = f.branches.reduce((n, b) => n + outcomeKeys(b).filter(o => !resFor(branchKey(f.id, b, o))).length, 0);
      const condsOpen = f.decisions.reduce((n, d) => n + d.conditions.filter((c, i) => !resFor(condKey(f.id, d, i))).length, 0);
      const closes = 1 + linesOpen + outcomesOpen + condsOpen;
      out.push({ closes, kind: 'function', f, line: f.lineStart, tab: 'calls',
        what: `never executed: ${linesOpen} line(s), ${outcomesOpen} outcome(s) and ${condsOpen} condition(s) are unreachable until something calls it`,
        next: 'add a test that calls it — the Calls tab shows who could' });
      continue; // its inner items are subsumed by the call itself
    }
    const fl = file(f.fileId);
    for (const d of f.decisions) {
      const open = d.conditions.map((c, i) => [c, i]).filter(([c, i]) => !c.independenceCovered && !resFor(condKey(f.id, d, i)));
      if (!open.length) continue;
      const steps = open.map(([c, i]) => {
        const name = condText(fl, d, i);
        if (c.trueCount + c.falseCount === 0) return `reach \`${name}\` at all (every vector short-circuited before it)`;
        if (c.falseCount === 0) return `reach \`${name}\` with it false`;
        if (c.trueCount === 0) return `reach \`${name}\` with it true`;
        return `flip \`${name}\` while the other conditions hold still`;
      });
      out.push({ closes: open.length, kind: 'decision', f, line: d.line, tab: 'mcdc',
        what: `${open.length} of ${d.conditions.length} MC/DC condition(s) unproven at L${d.line}`, next: steps.join('; ') });
    }
    const inDec = new Set(f.decisions.flatMap(d => d.conditions.map(c => `${c.line}:${c.col}`)));
    for (const b of f.branches) {
      if (inDec.has(`${b.line}:${b.col}`)) continue;
      const [k0, k1] = outcomeKeys(b), [w0, w1] = outcomeLabels(b);
      for (const [key, word, other, otherWord, taken] of [[k0, w0, b.falseCount, w1, b.trueCount], [k1, w1, b.trueCount, w0, b.falseCount]]) {
        if (taken > 0 || resFor(branchKey(f.id, b, key))) continue;
        out.push({ closes: 1, kind: 'outcome', f, line: b.line, tab: 'mcdc',
          what: dirKnown(b) ? `the ${word} outcome at L${branchSite(b)} was never taken`
                            : `${word} at L${branchSite(b)} was never taken (gcov does not say which is true)`,
          next: other > 0 ? `${dirKnown(b) ? `the ${otherWord} outcome` : otherWord} ran ${other}x, so this is one input away`
                         : 'neither outcome ran; a test must reach this line first' });
      }
    }
  }
  out.sort((a, b) => b.closes - a.closes || a.f.name.localeCompare(b.f.name) || a.line - b.line);
  return out;
}

// The gate thresholds a reader has chosen. Kept per browser: they are the
// project's policy, not a property of the evidence, and two people reviewing
// the same file may hold different ones until a project fixes them in CI.
const GATE_KEYS = ['min_lines', 'min_outcomes', 'min_mcdc', 'max_unresolved_gaps'];
function gateThresholds() {
  try {
    return JSON.parse(localStorage.getItem('sl-gates') || '{}');
  } catch { return {}; }
}
function setGateThreshold(k, v) {
  const t = gateThresholds();
  if (v === '' || v == null) delete t[k]; else t[k] = v;
  try { localStorage.setItem('sl-gates', JSON.stringify(t)); } catch { /* private window */ }
  renderOverview();
}

/// Ask the server what the gates say, and show it.
function renderGates() {
  const box = h('div', { class: 'gates' });
  const th = gateThresholds();
  const label = { min_lines: 'lines %', min_outcomes: 'outcomes %', min_mcdc: 'MC/DC %', max_unresolved_gaps: 'max gaps' };
  const row = h('div', { class: 'gate-inputs' });
  for (const k of GATE_KEYS) {
    row.append(h('label', {}, label[k], h('input', {
      type: 'number', min: '0', max: k === 'max_unresolved_gaps' ? null : '100', value: th[k] ?? '',
      placeholder: '—', 'aria-label': `${label[k]} floor`,
      onchange: e => setGateThreshold(k, e.target.value),
    })));
  }
  const verdict = h('div', { class: 'gate-verdict muted' }, 'checking…');
  const detail = h('div', {});
  box.append(h('h2', {}, 'Gates'), row, verdict, detail);

  const qs = GATE_KEYS.filter(k => th[k] !== undefined && th[k] !== '').map(k => `${k}=${encodeURIComponent(th[k])}`).join('&');
  const forVariant = S.variant;
  authJson(`/api/gates${vq()}${vq() ? (qs ? '&' : '') : (qs ? '?' : '')}${qs}`).then(g => {
    if (S.variant !== forVariant) return;
    verdict.innerHTML = '';
    detail.innerHTML = '';
    if (!g.thresholdsGiven) {
      verdict.className = 'gate-verdict muted';
      verdict.append('No thresholds set, so nothing is being gated. Enter a floor above to check one.');
    } else if (g.breaches.length) {
      verdict.className = 'gate-verdict bad';
      verdict.append(`${g.breaches.length} breach(es) — this build would fail CI.`);
      for (const b of g.breaches) detail.append(h('div', { class: 'gate-breach' }, h('b', { class: 'mono' }, b.kind), ' ', b.message));
    } else {
      verdict.className = 'gate-verdict ok';
      verdict.append('Every threshold met.');
    }
    // The command that does the same thing, so this can go into CI and a
    // manager can hand it to an engineer -- shown only once a floor is set.
    // With nothing gated there is nothing the command would check, and a
    // verdict about nothing reads as a claim ("exits 0") the panel is not
    // making. The sentence above already says what to do instead.
    if (g.thresholdsGiven && g.command) detail.append(h('div', { class: 'gate-cmd' }, h('code', {}, g.command),
      h('span', { class: 'muted' }, ` · exits ${g.wouldExit}`)));
    if (g.claims && g.claims.disagreements && g.claims.disagreements.length) {
      detail.append(h('div', { class: 'gate-breach' }, h('b', {}, 'claims'), ` ${g.claims.disagreements.length} place(s) where the requirements document and the measurement disagree`));
    }
  }).catch(e => {
    verdict.className = 'gate-verdict bad';
    verdict.textContent = `Could not reach the gates: ${e.message}`;
  });
  return box;
}

function renderOverview() {
  const s = S.ev.summary; const c = $('#content'); c.innerHTML = '';
  const o = h('div', { class: 'overview' });
  const unresolved = S.ev.functions.reduce((n, f) => n + unresolvedGaps(f), 0);
  const tile = (v, t, cls, bar) => h('div', { class: 'tile ' + (cls || '') }, h('div', { class: 'v', html: v }), h('div', { class: 't' }, t), bar != null && h('div', { class: 'bar' }, h('i', { style: `width:${bar}%` })));
  const lp = pct(s.linesCovered, s.linesTotal), bp = pct(s.branchesCovered, s.branchesTotal), op = pct(s.instructionsProjectedCovered, s.instructionsTotal);
  // "Does this pass?" is the first question anyone asks, so it goes first.
  // The thresholds live in the browser, not the evidence: they are project
  // policy, and butterfly does not get to decide them.
  if (!S.ev.changes && S.fresh && S.fresh.stale) freshnessBanner(o);
  o.append(renderGates());
  o.append(h('h2', {}, 'Structural coverage — Annex A table A-7'),
    h('div', { class: 'tiles' },
      // A function with no coverage record is neither executed nor not, so it
      // is out of the denominator, and the tile says how many there are (#273).
      (() => {
        const none = s.functionsUnrecorded || 0, recorded = s.functions - none;
        return tile(`${s.functionsExecuted}<small>/ ${recorded}${none ? ` · +${none} with no record` : ''}</small>`,
          none ? 'functions executed · with no coverage record: not established' : 'functions executed',
          s.functionsExecuted !== recorded ? 'unc' : none ? 'jus' : 'cov');
      })(),
      tile(`${fmtPct(lp)}<small>${s.linesCovered}/${s.linesTotal}</small>`, 'statement (line) coverage · obj 7', lp === 100 ? 'cov' : 'unc', lp),
      tile(`${fmtPct(bp)}<small>${s.branchesCovered}/${s.branchesTotal}</small>`, 'decision outcomes · obj 6', bp === 100 ? 'cov' : 'unc', bp),
      tile(`${s.decisionsMcdcCovered}<small>/ ${s.decisionsTotal} decisions</small>`, `MC/DC · obj 5 · ${s.conditionsIndependenceCovered}/${s.conditionsTotal} conditions`, s.decisionsMcdcCovered === s.decisionsTotal ? 'cov' : 'unc', pct(s.conditionsIndependenceCovered, s.conditionsTotal)),
      tile(`${fmtPct(op)}<small>${s.instructionsProjectedCovered}/${s.instructionsTotal}</small>`, 'object code, projected · obj 9', 'jus', op),
      tile(`${unresolved}<small>/ ${s.gapsTotal}</small>`, 'object-code gaps unresolved', unresolved ? 'unc' : 'cov'),
      s.instructionsTotal ? tile(`${fmtPct(pct(s.instructionsCompilerGenerated, s.instructionsTotal))}<small>${s.instructionsCompilerGenerated}/${s.instructionsTotal}</small>`, 'instructions compiler-generated · obj 9 · ' + Object.entries(s.compilerGeneratedByOrigin || {}).map(([k, v]) => `${k} ${v}`).join(' · '), 'jus', pct(s.instructionsCompilerGenerated, s.instructionsTotal)) : null,
      hasMeasured() ? tile(`${fmtPct(pct(s.instructionsMeasuredCovered, s.instructionsTotal))}<small>${s.instructionsMeasuredCovered}/${s.instructionsTotal}</small>`, `object code, measured · ${S.ev.measured.format}`, 'cov', pct(s.instructionsMeasuredCovered, s.instructionsTotal)) : null,
      hasMeasured() ? tile(`${s.disagreements}`, 'projection ≠ trace (instructions)', s.disagreements ? 'jus' : 'cov') : null,
      hasMeasured() && s.outcomeEdgesTotal ? tile(`${fmtPct(pct(s.outcomeEdgesMeasured, s.outcomeEdgesTotal))}<small>${s.outcomeEdgesMeasured}/${s.outcomeEdgesTotal}</small>`, 'object branch outcomes, measured · CAST-17 route', s.outcomeEdgesMeasured === s.outcomeEdgesTotal ? 'cov' : 'unc', pct(s.outcomeEdgesMeasured, s.outcomeEdgesTotal)) : null,
      s.stackWorstCase != null ? tile(`${fmtB(s.stackWorstCase)}`, `stack worst case · ${sym(s.stackWorstSymbol).name.slice(0, 28)}`, s.stackUnbounded ? 'jus' : 'cov') : null,
      s.stackWorstCase != null ? tile(`${s.stackUnbounded}<small>/ ${S.ev.symbols.length} symbols</small>`, 'stack not bounded (recursion / indirect / dynamic)', s.stackUnbounded ? 'jus' : 'cov') : null,
      S.ev.globals.length ? tile(`${s.globals}<small>globals · ${s.dataRefs} refs</small>`, 'data coupling · obj 8', '') : null));
  {
    const work = worklist();
    if (work.length) {
      const total = work.reduce((n, i) => n + i.closes, 0);
      const list = h('div', { class: 'worklist' });
      for (const item of work.slice(0, 8)) {
        list.append(h('div', { class: 'wl-item', ...activatable(() => navigate(`#/fn/${item.f.id}/${item.tab}`)) },
          h('span', { class: 'wl-score', title: 'how many open obligations one test could close here, at most' }, `closes ${item.closes}`),
          h('div', {},
            h('div', {}, h('b', { class: 'mono' }, item.f.name), h('span', { class: 'muted' }, ` L${item.line} — ${item.what}`)),
            h('div', { class: 'wl-next' }, '→ ', item.next))));
      }
      o.append(h('h2', {}, `What to test next — ${total} open obligation(s), best leverage first`), list);
      if (work.length > 8) list.append(h('div', { class: 'muted', style: 'padding:6px 2px' }, `… and ${work.length - 8} more. Unresolved gaps are reviewer work, not test work, and are not in this list.`));
    }
  }

  if (s.gapsTotal) {
    o.append(h('h2', {}, 'Gaps by kind (6.4.4.2.b — code not directly traceable to source)'));
    const t = h('table', { class: 'tbl' }, h('tr', {}, h('th', {}, 'kind'), h('th', {}, 'meaning'), h('th', {}, 'total'), h('th', {}, 'unresolved')));
    const meaning = {
      'predicated-decision': 'Source decision with no object-level branch (conditional select / merged / table)',
      'extra-object-branch': 'More conditional branches in object code than source branch regions on that line',
      'line-zero': 'Instructions DWARF attributes to line 0 (compiler-generated)',
      'no-source-line': 'Instructions with no line-table entry at all',
      'helper-call': 'Call outside the covered source set (libc, runtime, compiler helper)',
      'indirect-branch': 'Indirect branch or call: target set must be established by analysis',
      'not-instrumented': 'Maps to a source line with no coverage count',
      'shared-line': 'Block whose own source lines do not describe it (merged return / common epilogue): projected uncovered when reached only via a never-taken branch outcome, or covered when a block that ran must enter it — by inference, see the block note',
      'folded-call': 'Call written in the source with no call instruction and no inlined subroutine in the object code — evaluated away by the compiler',
      'no-coverage-record': 'Object code of a function the compiler gave no coverage record (a closure or lambda thunk): the count on its line is the enclosing function\'s, so whether this code ran is not established',
      'uncounted-variant': 'C++ constructor/destructor variant (C1, D1, deleting D0) with no coverage counter of its own: whether it ran is not established',
    };
    const unres = {};
    for (const f of S.ev.functions) for (const g of fnGaps(f)) if (!resFor(g.key)) unres[slug(g.kind)] = (unres[slug(g.kind)] || 0) + 1;
    for (const [k, n] of Object.entries(s.gapsByKind)) t.append(h('tr', {}, h('td', {}, h('span', { class: 'kind ' + k }, k)), h('td', {}, meaning[k] || ''), h('td', {}, n), h('td', { style: unres[k] ? 'color:var(--unc);font-weight:600' : 'color:var(--cov)' }, unres[k] || 0)));
    const cls = {}; const clsUn = {};
    for (const f of S.ev.functions) for (const g of fnGaps(f)) if (slug(g.kind) === 'helper-call') { const c = g.class || 'unknown'; cls[c] = (cls[c] || 0) + 1; if (!resFor(g.key)) clsUn[c] = (clsUn[c] || 0) + 1; }
    const clsMeaning = { 'runtime-check': 'compiler / runtime checks (panics, bounds, stack protector) — defensive structures', 'arith-helper': 'compiler-rt / libgcc arithmetic helpers', 'memory-helper': 'memcpy / memset family', 'core-lib': 'language core / std library code', 'libc': 'C library', 'project-uncovered': 'project code without coverage evidence in this export', unknown: 'unclassified' };
    for (const [k, n] of Object.entries(cls).sort((a, b) => b[1] - a[1])) t.append(h('tr', {}, h('td', { style: 'padding-left:24px' }, h('span', { class: 'kind cls-' + k }, k)), h('td', {}, 'helper-call · ' + (clsMeaning[k] || '')), h('td', {}, n), h('td', { style: clsUn[k] ? 'color:var(--unc);font-weight:600' : 'color:var(--cov)' }, clsUn[k] || 0)));
    o.append(t);
  }

  o.append(h('h2', {}, 'Functions'));
  const t = h('table', { class: 'tbl nowrap' }, h('tr', {}, h('th', {}, 'function'), h('th', {}, 'file'), h('th', {}, 'requirements'), h('th', {}, 'calls'), h('th', {}, 'lines'), h('th', {}, ''), h('th', {}, 'branches'), h('th', {}, 'MC/DC'), h('th', {}, 'v(G)'), h('th', {}, 'object (proj.)'), h('th', {}, 'gaps')));
  for (const f of S.ev.functions) {
    const st = fnStats(f); const o2 = f.object;
    t.append(h('tr', { class: 'row', onclick: () => navigate(`#/fn/${f.id}`) },
      h('td', { class: 'mono' }, f.name), h('td', { class: 'mono muted' }, file(f.fileId).display + ':' + f.lineStart),
      h('td', {}, f.requirements.map(r => reqChip(r, { style: 'margin-right:4px' }))),
      unrecorded(f) ? h('td', { style: 'color:var(--jus)', title: 'no coverage record of its own: whether it ran is not established' }, 'no record') : h('td', { style: f.count ? '' : 'color:var(--unc);font-weight:600' }, f.count),
      h('td', {}, unrecorded(f) ? '—' : `${f.linesCovered}/${f.linesTotal}`), h('td', {}, h('div', { class: 'bar ' + (st.linesPct < 100 ? 'unc' : '') }, h('i', { style: `width:${st.linesPct ?? 0}%` }))),
      h('td', {}, st.br ? `${st.brc}/${st.br}` : '—'), h('td', {}, st.conds ? `${st.condc}/${st.conds}` : '—'),
      h('td', {}, f.complexity.cyclomaticObject ?? '—'),
      h('td', {}, o2 ? `${fnObjects(f).reduce((a, o) => a + o.instrProjectedCovered, 0)}/${fnObjects(f).reduce((a, o) => a + o.instrTotal, 0)}` : '—'),
      h('td', { style: st.unresolved ? 'color:var(--jus);font-weight:600' : '' }, st.gaps ? `${st.unresolved}/${st.gaps}` : '—')));
  }
  o.append(h('div', { class: 'scroll-x' }, t));

  {
    const fns = S.ev.functions.filter(f => f.object);
    const reviewed = fns.filter(f => s2oOf(f));
    const linesOf = f => f.linesTotal || 0;
    const instrOf = f => fnObjects(f).reduce((a, o) => a + o.instrTotal, 0);
    const totLines = fns.reduce((a, f) => a + linesOf(f), 0), revLines = reviewed.reduce((a, f) => a + linesOf(f), 0);
    const totInstr = fns.reduce((a, f) => a + instrOf(f), 0), revInstr = reviewed.reduce((a, f) => a + instrOf(f), 0);
    const byVerdict = {}; for (const f of reviewed) { const c = s2oOf(f).category; byVerdict[c] = (byVerdict[c] || 0) + 1; }
    const staleN = reviewed.filter(stale).length;
    o.append(h('h2', {}, 'Source-to-object traceability analysis — A-7 obj 9 (CAST-12 sample adequacy)'),
      h('div', { class: 'tiles' },
        tile(`${reviewed.length}<small>/ ${fns.length} functions</small>`, 'reviewed source ↔ object', reviewed.length === fns.length ? 'cov' : reviewed.length ? 'jus' : 'unc'),
        tile(`${fmtPct(pct(revLines, totLines))}<small>${revLines}/${totLines} lines</small>`, 'code base covered by the reviewed sample (≥ 10 % expected)', pct(revLines, totLines) >= 10 ? 'cov' : 'unc', pct(revLines, totLines)),
        tile(`${fmtPct(pct(revInstr, totInstr))}<small>${revInstr}/${totInstr} instr</small>`, 'object code covered by the reviewed sample', pct(revInstr, totInstr) >= 10 ? 'cov' : 'unc', pct(revInstr, totInstr)),
        tile(`${byVerdict.traceable || 0}<small> traceable · ${byVerdict.analysed || 0} analysed · ${byVerdict['not-traceable'] || 0} not</small>`, 'verdicts', byVerdict['not-traceable'] ? 'unc' : ''),
        S.ev.baseline ? tile(`${staleN}<small>/ ${reviewed.length} reviews</small>`, 'stale since baseline (4.4.2.c)', staleN ? 'unc' : 'cov') : null));
    if (S.ev.changes) {
      // change impact: ticket -> functions -> requirements -> verification state
      const c = S.ev.changes;
      o.append(h('h2', {}, `Change impact (12.1.1) — ${c.commits.length} commit${c.commits.length === 1 ? '' : 's'} · ${c.tickets.length} ticket${c.tickets.length === 1 ? '' : 's'} · ${c.unticketed.length} unticketed`),
        h('div', { class: 'note', style: 'margin-bottom:8px' }, `History read at ${c.head.slice(0, 12)}${c.since ? ' since ' + c.since : ''}, following each function's line range with git log -L. Ticket patterns: ${c.patterns.join('  |  ')}`));
      freshnessBanner(o);
      const im = c.impact;
      if (c.tickets.length) {
        const t = h('table', { class: 'tbl nowrap' }, h('tr', {}, h('th', {}, 'ticket'), h('th', {}, 'commits'), h('th', {}, 'added'), h('th', {}, 'modified'), h('th', {}, 'deleted'), h('th', {}, 'files'), h('th', {}, 'requirements'), h('th', {}, 'unresolved gaps'), h('th', {}, 'not reviewed'), im ? h('th', {}, 'blockers') : null, im ? h('th', {}, 'obligations') : null));
        c.tickets.forEach((tk, ti) => {
          const fs = tk.functions.map(fn);
          const reqs = [...new Set(fs.flatMap(f => f.requirements))];
          const gaps = fs.reduce((a, f) => a + unresolvedGaps(f), 0);
          const unrev = fs.filter(f => !resFor(`s2o:${f.name}`)).length;
          const bt = im && im.byTicket.find(x => x.ticket === ti);
          const fnLinks = ids => (ids || []).map(fn).map(f => h('a', { class: 'lnk', style: 'margin-right:8px', onclick: () => navigate(`#/fn/${f.id}`) }, f.name));
          const untraced = (tk.files || []).filter(x => !x.functions);
          t.append(h('tr', {}, h('td', {}, h('b', {}, tk.id)),
            h('td', {}, tk.commits.map(i => h('span', { class: 'muted', title: c.commits[i].subject, style: 'margin-right:6px' }, c.commits[i].short.slice(0, 7)))),
            h('td', {}, fnLinks(tk.added)),
            h('td', {}, fnLinks(tk.modified)),
            h('td', { class: (tk.deleted || []).length ? 'bad' : '' }, (tk.deleted || []).map(i => h('code', { style: 'margin-right:6px', title: `${c.deleted[i].file}:${c.deleted[i].lineStart}-${c.deleted[i].lineEnd}` }, c.deleted[i].name))),
            h('td', { title: (tk.files || []).map(x => `${x.status} ${x.path}${x.functions ? '' : '  (no function in the evidence)'}`).join('\n') }, `${(tk.files || []).length}`, untraced.length ? h('span', { class: 'bad' }, ` · ${untraced.length} untraced`) : null),
            h('td', {}, reqs.map(r => h('span', { class: 'req', style: 'margin-right:4px' }, r))),
            h('td', { class: gaps ? 'bad' : 'ok' }, gaps), h('td', { class: unrev ? 'bad' : 'ok' }, unrev),
            im ? h('td', { class: bt && bt.blockers ? 'bad' : 'ok' }, bt ? bt.blockers : 0) : null,
            im ? h('td', {}, bt ? bt.obligations.length : 0) : null));
        });
        o.append(t);
      }
      if (im) {
        // the re-verification obligations: what the changes invalidate and what must be re-run
        const sev = im.bySeverity || {};
        o.append(h('div', { class: 'sub-h', style: 'margin-top:10px' }, `Re-verification obligations (${im.obligations.length}) — ${sev.blocker || 0} blocker · ${sev.major || 0} major · ${sev.advisory || 0} advisory${im.bounded ? '' : ' — whole history, no --since: every function with a commit counts as changed'}`));
        if (im.obligations.length) {
          // two rows per obligation: the four short columns on one line, the
          // sentence of detail beneath them. Reads the same at any pane width.
          const ot = h('table', { class: 'tbl' }, h('tr', {}, h('th', {}, 'severity'), h('th', {}, 'kind'), h('th', {}, 'function'), h('th', {}, 'target / key / requirement')));
          for (const ob of im.obligations.slice(0, 200)) {
            const f = ob.functionId != null ? fn(ob.functionId) : null; const tgt = ob.targetFunction != null ? fn(ob.targetFunction) : null;
            const nw = 'white-space:nowrap;border-bottom:none;padding-bottom:0';
            ot.append(h('tr', {},
              h('td', { style: nw }, h('span', { class: 'k ' + (ob.severity === 'blocker' ? 'bad' : ob.severity === 'major' ? 'jus' : 'muted'), style: 'font-weight:600' }, ob.severity)),
              h('td', { style: nw }, h('code', {}, ob.kind)),
              h('td', { style: nw }, f ? h('a', { class: 'lnk', onclick: () => navigate(`#/fn/${f.id}`) }, f.name) : ob.ticket != null ? h('b', {}, c.tickets[ob.ticket].id) : h('span', { class: 'muted' }, '—')),
              h('td', { style: nw }, tgt ? h('a', { class: 'lnk', onclick: () => navigate(`#/fn/${tgt.id}`) }, tgt.name) : ob.requirement ? h('span', { class: 'req' }, ob.requirement) : h('code', {}, ob.key || ob.file || ''))));
            ot.append(h('tr', {}, h('td', { colspan: 4, class: 'muted', style: 'font-size:11px;padding-top:0;white-space:normal' }, ob.detail)));
          }
          if (im.obligations.length > 200) ot.append(h('tr', {}, h('td', { colspan: 5, class: 'muted' }, `… ${im.obligations.length - 200} more — butterfly impact --json for all`)));
          o.append(ot);
        }
        for (const x of im.caveats || []) o.append(h('div', { class: 'note' }, x));
      } else {
        o.append(h('div', { class: 'note' }, 'No obligations computed yet: run `butterfly impact <evidence> --write` (or `butterfly changes`, which computes them).'));
      }
      if (c.unticketed.length) {
        o.append(h('div', { class: 'sub-h', style: 'margin-top:10px' }, `Commits with no ticket (${c.unticketed.length}) — code changed outside change control`));
        const u = h('table', { class: 'tbl nowrap' }, h('tr', {}, h('th', {}, 'commit'), h('th', {}, 'date'), h('th', {}, 'author'), h('th', {}, 'subject'), h('th', {}, 'functions')));
        for (const i of c.unticketed.slice(0, 50)) { const cm = c.commits[i]; u.append(h('tr', {}, h('td', {}, cm.short.slice(0, 7)), h('td', {}, cm.date.slice(0, 10)), h('td', {}, cm.author), h('td', {}, cm.subject), h('td', {}, cm.functions.length))); }
        if (c.unticketed.length > 50) u.append(h('tr', {}, h('td', { colspan: 5, class: 'muted' }, `… ${c.unticketed.length - 50} more`)));
        o.append(u);
      }
      for (const x of c.caveats.filter(x => /uncommitted|different checkout|could not be walked|not tracked|outside/.test(x))) o.append(h('div', { class: 'note', style: 'color:var(--unc)' }, x));
    }
    if (S.ev.baseline) {
      const b = S.ev.baseline;
      o.append(h('h3', { style: 'margin:14px 0 6px;font-size:12px' }, `Baseline ${b.path.split('/').pop()} · frozen ${new Date(b.createdUnix * 1000).toISOString().slice(0, 10)} · `, h('span', { style: b.status === 'matches' ? 'color:var(--cov)' : b.status === 'changed' ? 'color:var(--unc)' : 'color:var(--jus)' }, b.status)));
      if (b.differences.length) o.append(h('ul', { class: 'caveat-list' }, b.differences.map(d => h('li', {}, h('span', { class: 'kind ' + (d.invalidates ? 'no-source-line' : 'not-instrumented'), style: 'margin-right:6px' }, d.invalidates ? 'invalidates' : 'note'), h('b', {}, d.kind + ': '), d.detail))));
      else o.append(h('p', { class: 'muted' }, 'Compiler, options, target, constructs and every function fingerprint match the baseline: earlier analyses remain valid.'));
      const lists = [['source changed', b.functionsSourceChanged], ['object changed (same source)', b.functionsObjectChanged], ['added', b.functionsAdded], ['removed', b.functionsRemoved]].filter(([, l]) => l.length);
      if (lists.length) o.append(h('table', { class: 'tbl' }, lists.map(([k, l]) => h('tr', {}, h('td', { style: 'white-space:nowrap' }, k), h('td', { class: 'mono', style: 'font-size:11px' }, l.slice(0, 30).join(', ') + (l.length > 30 ? ` … +${l.length - 30}` : ''))))));
    } else o.append(h('p', { class: 'muted' }, 'No baseline: run `butterfly baseline evidence.json` once the analyses are done, keep the file under CM, and pass it (or drop it next to the artifacts as butterfly-baseline.json) on later ingests.'));
    // construct coverage of the sample
    const all = new Map();
    for (const f of fns) for (const k of Object.keys(f.constructs || {})) { if (!all.has(k)) all.set(k, { total: 0, reviewed: 0 }); all.get(k).total++; if (s2oOf(f)) all.get(k).reviewed++; }
    const t = h('table', { class: 'tbl' }, h('tr', {}, h('th', {}, 'construct'), h('th', {}, 'functions using it'), h('th', {}, 'reviewed functions using it'), h('th', {}, 'covered by sample')));
    for (const [k, v] of [...all.entries()].sort((a, b) => a[0].localeCompare(b[0]))) t.append(h('tr', {}, h('td', { class: 'mono' }, k), h('td', {}, v.total), h('td', {}, v.reviewed), h('td', { style: v.reviewed ? 'color:var(--cov)' : 'color:var(--unc);font-weight:600' }, v.reviewed ? '✓' : '✗ not yet')));
    o.append(t);
    // greedy suggestion: functions adding the most uncovered constructs
    const uncovered = new Set([...all.entries()].filter(([, v]) => !v.reviewed).map(([k]) => k));
    if (uncovered.size) {
      const sug = []; const left = new Set(uncovered);
      const cand = fns.filter(f => !s2oOf(f));
      while (left.size && sug.length < 6) {
        let best = null, bestGain = 0;
        for (const f of cand) { const gain = Object.keys(f.constructs || {}).filter(k => left.has(k)).length; if (gain > bestGain) { best = f; bestGain = gain; } }
        if (!best) break;
        sug.push([best, Object.keys(best.constructs).filter(k => left.has(k))]);
        for (const k of Object.keys(best.constructs)) left.delete(k);
      }
      o.append(h('p', { class: 'muted', style: 'margin:8px 0 4px' }, 'Suggested next reviews (greedy: most not-yet-covered constructs first):'),
        h('table', { class: 'tbl' }, sug.map(([f, ks]) => h('tr', { class: 'row', onclick: () => navigate(`#/fn/${f.id}/object`) }, h('td', { class: 'mono' }, f.name), h('td', {}, f.object ? `${f.object.instrTotal} instr` : ''), h('td', { class: 'mono muted' }, ks.join(' · '))))));
    }
  }
  if (S.ev.image) {
    const im = S.ev.image;
    o.append(h('h2', {}, `Image — ${im.format} ${im.arch} · ${im.path.split('/').pop()} (6.3.5 integration outputs)`));
    if (im.findings.length) o.append(h('ul', { class: 'caveat-list' }, im.findings.map(x => h('li', {}, x))));
    const st = h('table', { class: 'tbl' }, h('tr', {}, h('th', {}, 'section'), h('th', {}, 'address'), h('th', {}, 'size'), h('th', {}, 'flags'), h('th', {}, 'symbols'), h('th', {}, 'largest')));
    for (const sec of im.sections) st.append(h('tr', {}, h('td', { class: 'mono' }, sec.name), h('td', { class: 'mono' }, hex(sec.address)), h('td', {}, fmtB(sec.size)), h('td', { class: 'mono' }, sec.flags), h('td', {}, sec.symbols), h('td', { class: 'mono muted', style: 'font-size:11px' }, sec.largest.map(([n, b]) => `${n.slice(0, 36)} (${b})`).join(' · '))));
    const sg = h('table', { class: 'tbl', style: 'margin-top:8px' }, h('tr', {}, h('th', {}, 'segment'), h('th', {}, 'address'), h('th', {}, 'memory size'), h('th', {}, 'file size'), h('th', {}, 'flags')));
    for (const g of im.segments) sg.append(h('tr', {}, h('td', { class: 'mono' }, g.name), h('td', { class: 'mono' }, hex(g.address)), h('td', {}, fmtB(g.size)), h('td', {}, fmtB(g.fileSize)), h('td', { class: 'mono' }, g.flags)));
    o.append(st, sg);
    if (im.textSymbolsUncovered.length) {
      const ut = h('table', { class: 'tbl', style: 'margin-top:8px' }, h('tr', {}, h('th', {}, 'code in image without source-coverage evidence (largest first)'), h('th', {}, 'bytes'), h('th', {}, 'executed in trace')));
      for (const id of im.textSymbolsUncovered.slice(0, 25)) { const y = sym(id); ut.append(h('tr', {}, h('td', { class: 'mono' }, y.name), h('td', {}, y.size), h('td', {}, hasMeasured() ? (y.measured ? `${y.measured}×` : h('span', { style: 'color:var(--unc)' }, 'no')) : '—'))); }
      if (im.textSymbolsUncovered.length > 25) ut.append(h('tr', {}, h('td', { colspan: 3, class: 'muted' }, `… ${im.textSymbolsUncovered.length - 25} more`)));
      o.append(ut);
    }
  }
  if (S.ev.image && S.ev.image.deadCandidates) {
    const dc = S.ev.image.deadCandidates;
    const unreach = S.ev.functions.filter(f => f.object && f.object.unreachableBlocks.length);
    o.append(h('h2', {}, `Dead-code candidates — 6.4.4.3.c (${dc.length} unreferenced symbol${dc.length === 1 ? '' : 's'}, ${unreach.length} function${unreach.length === 1 ? '' : 's'} with unreachable blocks)`),
      h('div', { class: 'note', style: 'margin-bottom:8px' }, 'A symbol is a candidate when nothing in the image calls it directly, nothing takes its address (code or data tables), and it is not the entry point. Executed-in-trace candidates are reached by a path static analysis cannot see (interrupt vectors, indirect jumps, callers outside the image) — they are not dead, but the reaching mechanism must be identified. Remember the glossary exceptions: defensive structures and deactivated code are not dead code.'));
    if (dc.length) {
      const t = h('table', { class: 'tbl' }, h('tr', {}, h('th', {}, 'symbol'), h('th', {}, 'bytes'), h('th', {}, 'source coverage'), h('th', {}, 'trace'), h('th', {}, 'resolution'), h('th', {}, '')));
      for (const d of dc) {
        const r = resFor(d.key);
        t.append(h('tr', {}, h('td', { class: 'mono' }, d.functionId != null ? h('a', { class: 'lnk', onclick: () => navigate(`#/fn/${d.functionId}`) }, d.name) : d.name), h('td', {}, d.size),
          h('td', {}, d.count == null ? h('span', { class: 'muted' }, 'no coverage function') : d.count === 0 ? h('span', { style: 'color:var(--unc)' }, 'never executed') : `${d.count}×`),
          h('td', {}, d.measured == null ? '—' : d.measured === 0 ? h('span', { style: 'color:var(--unc)' }, 'never executed') : h('span', { style: 'color:var(--jus)' }, `executed (${d.measured}) — hidden reaching path`)),
          h('td', {}, r ? [h('b', { style: 'color:var(--jus)' }, r.category.toUpperCase()), ' ', r.justification] : h('span', { class: 'muted' }, 'unresolved')),
          h('td', {}, h('button', { class: 'ghost', style: 'padding:2px 8px', onclick: () => openResolution(d.key, `Dead-code candidate ${d.name}`, d.reasons.join(' · ')) }, r ? 'Edit' : 'Resolve'))));
      }
      o.append(t);
    }
    if (unreach.length) {
      o.append(h('table', { class: 'tbl', style: 'margin-top:8px' }, h('tr', {}, h('th', {}, 'function with unreachable block(s)'), h('th', {}, 'blocks'), h('th', {}, 'note')),
        unreach.map(f => h('tr', { class: 'row', onclick: () => navigate(`#/fn/${f.id}/flow`) }, h('td', { class: 'mono' }, f.name), h('td', { class: 'mono' }, f.object.unreachableBlocks.map(b => 'B' + b).join(', ')), h('td', { class: 'muted' }, 'no incoming edge inside the function: trap after a no-return call, jump-table target, or genuinely unreachable')))));
    }
  }
  if (S.ev.symbols.length) {
    o.append(h('h2', {}, 'Stack — deepest chains (6.3.4.f)'));
    const top = [...S.ev.symbols].filter(y => y.stack.worstCase != null).sort((a, b) => b.stack.worstCase - a.stack.worstCase).slice(0, 12);
    const t = h('table', { class: 'tbl' }, h('tr', {}, h('th', {}, 'symbol'), h('th', {}, 'own frame'), h('th', {}, 'worst case'), h('th', {}, 'depth'), h('th', {}, 'bounded')));
    for (const y of top) t.append(h('tr', { class: y.functionId != null ? 'row' : '', onclick: y.functionId != null ? () => navigate(`#/fn/${y.functionId}/stack`) : null }, h('td', { class: 'mono' }, y.name), h('td', {}, fmtB(y.frame.bytes)), h('td', {}, fmtB(y.stack.worstCase)), h('td', {}, y.stack.depth), h('td', { style: y.stack.bounded ? 'color:var(--cov)' : 'color:var(--jus)' }, y.stack.bounded ? 'yes' : y.stack.reason)));
    o.append(t);
  }
  if (S.ev.globals.length) {
    o.append(h('h2', {}, 'Data coupling — globals and their accessors (A-7 obj 8)'));
    const t = h('table', { class: 'tbl' }, h('tr', {}, h('th', {}, 'global'), h('th', {}, 'section'), h('th', {}, 'size'), h('th', {}, 'readers'), h('th', {}, 'writers'), h('th', {}, 'address taken by')));
    for (const g of S.ev.globals) t.append(h('tr', {}, h('td', { class: 'mono' }, g.name), h('td', { class: 'mono muted' }, g.section), h('td', {}, g.size), h('td', {}, accessorList(g.readerSymbols, false)), h('td', {}, accessorList(g.writerSymbols, false)), h('td', {}, accessorList(g.addressTakerSymbols, false))));
    o.append(t);
  }
  const pv = S.ev.provenance;
  o.append(h('h2', {}, 'Build & provenance'));
  const dl = h('dl', { class: 'kv' });
  const kv = (k, v) => dl.append(h('dt', {}, k), h('dd', {}, v));
  kv('tool', `${pv.tool} ${pv.toolVersion} — ${toolBuild(pv)}`);
  for (const [k, v] of Object.entries(pv.buildInfo)) kv(k, v);
  kv('object', `${pv.objectFormat} ${pv.arch}`);
  kv('inputs', `${pv.inputs.length} artifacts hashed — see Provenance`);
  o.append(dl, h('h2', {}, 'Caveats'), h('ul', { class: 'caveat-list' }, pv.caveats.map(c => h('li', {}, c))),
    h('p', { class: 'muted', style: 'margin-top:20px' }, 'Keys: ', h('kbd', {}, '/'), ' search · ', h('kbd', {}, 'j'), '/', h('kbd', {}, 'k'), ' next/prev function · ', h('kbd', {}, '1'), '–', h('kbd', {}, '9'), ', ', h('kbd', {}, '0'), ' tabs · ', h('kbd', {}, 'f'), ' function ⇄ file view · ', h('kbd', {}, ']'), '/', h('kbd', {}, '['), ' next/prev uncovered region (file view) · ', h('kbd', {}, 'Esc'), ' close'));
  c.append(o);
}
const slug = k => k.replace(/[A-Z]/g, m => '-' + m.toLowerCase());

/* ---------------- file view ---------------- */
function fileFns(fileId) { return file(fileId).functionIds.map(fn); }
function fnAtLine(fileId, ln) { return fileFns(fileId).find(f => ln >= f.lineStart && ln <= f.lineEnd) || null; }
/* per-line facts for a file: branches, decisions, gaps, inferred, density, tags */
function fileIndex(fileId) {
  const fl = file(fileId); const n = fl.text.length;
  const ix = { branches: new Map(), decisions: new Map(), gaps: new Map(), inferred: new Set(), density: new Array(n + 1).fill(0), tags: new Map(), changed: new Set(), gen: new Array(n + 1).fill(0) };
  const b = S.ev.baseline;
  for (const f of fileFns(fileId)) {
    for (const br of f.branches) { if (!ix.branches.has(br.line)) ix.branches.set(br.line, []); ix.branches.get(br.line).push([f, br]); }
    f.decisions.forEach((d, i) => { if (!ix.decisions.has(d.line)) ix.decisions.set(d.line, []); ix.decisions.get(d.line).push([f, d, i]); });
    for (const r of f.requirements) for (let l = f.lineStart; l <= f.lineEnd; l++) { if (!ix.tags.has(l)) ix.tags.set(l, new Set()); ix.tags.get(l).add(r); }
    for (const [l, rs] of Object.entries(f.lineRequirements || {})) { if (!ix.tags.has(+l)) ix.tags.set(+l, new Set()); for (const r of rs) ix.tags.get(+l).add(r); }
    if (b && (b.functionsSourceChanged.includes(f.name) || b.functionsObjectChanged.includes(f.name))) for (let l = f.lineStart; l <= f.lineEnd; l++) ix.changed.add(l);
    for (const g of fnGaps(f)) if (g.line) { if (!ix.gaps.has(g.line)) ix.gaps.set(g.line, []); ix.gaps.get(g.line).push([f, g]); }
    for (const blk of fnObjects(f).flatMap(o => o.blocks)) {
      if (blk.note) for (const l of [...blk.lines, ...(blk.inferredLines || [])]) ix.inferred.add(l);
      for (const i of blk.instrs) if (i.line && insFileId(f, i) === fileId && i.line <= n) { ix.density[i.line]++; if (i.origin && !i.origin.startsWith('inlined:')) ix.gen[i.line]++; }
    }
  }
  return ix;
}
function renderFile(fileId, gotoLine) {
  const fl = file(fileId); const c = $('#content'); c.innerHTML = '';
  const fns = fileFns(fileId); const ix = fileIndex(fileId); S.fileIx = ix;
  const stats = fns.map(fnStats);
  const unresolved = fns.reduce((a, f) => a + unresolvedGaps(f), 0);
  const gapsTotal = fns.reduce((a, f) => a + fnGaps(f).length, 0);
  const reviewed = fns.filter(f => f.object && s2oOf(f)).length, staleN = fns.filter(f => f.object && s2oOf(f) && stale(f)).length;
  const gen = fns.reduce((a, f) => a + (f.object ? objTotals(f).compilerGeneratedTotal : 0), 0);
  const instr = fns.reduce((a, f) => a + (f.object ? objTotals(f).instrTotal : 0), 0);
  const br = stats.reduce((a, s) => a + s.br, 0), brc = stats.reduce((a, s) => a + s.brc, 0);
  const conds = stats.reduce((a, s) => a + s.conds, 0), condc = stats.reduce((a, s) => a + s.condc, 0);
  const dead = (S.ev.image ? S.ev.image.deadCandidates : []).filter(d => d.functionId != null && fn(d.functionId).fileId === fileId);
  // the file view has no tabs to open, so its boxes carry the `?` alone; the
  // same numbers become clickable once you are inside a function
  const metric = (v, label, cls, help) => metricBox(v, label, cls, help, null);
  const head = h('div', { class: 'fn-head' },
    h('h1', {}, fl.display), h('span', { class: 'loc' }, `${fl.text.length} lines · ${fns.length} functions · sha256 ${fl.sha256.slice(0, 12)}…`),
    h('div', { class: 'metrics' },
      metric(fmtPct(pct(fl.linesCovered, fl.linesTotal)), 'lines', fl.linesCovered === fl.linesTotal ? 'cov' : 'unc', 'lines'),
      metric(br ? `${brc}/${br}` : '—', 'outcomes', br ? (brc === br ? 'cov' : 'unc') : '', 'outcomes'),
      metric(conds ? `${condc}/${conds}` : '—', 'MC/DC', conds ? (condc === conds ? 'cov' : 'unc') : '', 'mcdc'),
      metric(gapsTotal ? `${unresolved}/${gapsTotal}` : '0', 'gaps', unresolved ? 'jus' : '', 'gaps'),
      metric(instr ? `${gen}/${instr}` : '—', 'generated', 'jus', 'generated'),
      metric(`${reviewed}/${fns.filter(f => f.object).length}`, 'reviewed', staleN ? 'unc' : reviewed ? 'cov' : '', 'reviewed'),
      dead.length ? metric(dead.length, 'dead cand.', 'unc', 'dead') : null),
    h('div', { class: 'seg', title: 'coverage overlay' },
      h('button', { class: S.mode === 'plain' ? 'on' : '', onclick: () => { setMode('plain'); renderFile(fileId); } }, 'No coverage'),
      h('button', { class: S.mode === 'projected' ? 'on' : '', onclick: () => { setMode('projected'); renderFile(fileId); } }, 'Projected'),
      hasMeasured() ? h('button', { class: S.mode === 'measured' ? 'on' : '', onclick: () => { setMode('measured'); renderFile(fileId); } }, 'Measured') : null));
  const left = h('div', { class: 'pane' }); const right = h('div', { class: 'pane' });
  c.append(head, h('div', { class: 'panes' }, left, right));
  renderFileDashboard(left, fileId, ix, fns, stats, dead);
  renderFileSource(right, fileId, ix, fns, gotoLine);
}

function renderFileDashboard(pane, fileId, ix, fns, stats, dead) {
  const body = h('div', { class: 'pane-body' }); const wrap = h('div', { class: 'stack' });
  // functions by risk
  const risk = f => { const st = fnStats(f); return (f.linesTotal ? (1 - (st.linesPct ?? 0) / 100) : 0) * 10 + unresolvedGaps(f) * 2 + (f.object && !s2oOf(f) ? 3 : 0) + (f.object && stale(f) ? 5 : 0) + (f.count === 0 ? 4 : 0); };
  const sorted = [...fns].sort((a, b) => risk(b) - risk(a));
  const t = h('table', { class: 'mat', style: 'width:100%' }, h('tr', {}, h('th', {}, 'function (by risk)'), h('th', {}, 'lines'), h('th', {}, 'gaps'), h('th', {}, 'review'), h('th', {}, 'v(G)')));
  for (const f of sorted) {
    const st = fnStats(f); const r = s2oOf(f);
    t.append(h('tr', { class: 'frow', style: 'cursor:pointer', title: 'click: open the function view (hover-scroll can be switched on above)',
      onmouseenter: () => { if (S.hoverFn) revealFunction(f); }, onclick: () => navigate(`#/fn/${f.id}`) },
      h('td', { class: 'mono' }, f.name, unrecorded(f) ? h('span', { class: 'badge amber', style: 'margin-left:6px', title: 'no coverage record of its own' }, 'no record') : f.count === 0 ? h('span', { class: 'badge red', style: 'margin-left:6px' }, '0×') : null),
      h('td', { style: st.linesPct != null && st.linesPct < 100 ? 'color:var(--unc)' : 'color:var(--cov)' }, f.linesTotal ? `${f.linesCovered}/${f.linesTotal}` : '—'),
      h('td', { style: st.unresolved ? 'color:var(--jus);font-weight:600' : '' }, st.gaps ? `${st.unresolved}/${st.gaps}` : '—'),
      h('td', {}, !f.object ? h('span', { class: 'muted' }, 'no object') : !r ? h('span', { style: 'color:var(--jus)' }, 'not reviewed') : stale(f) ? h('span', { style: 'color:var(--unc)' }, 'STALE') : h('span', { style: r.category === 'not-traceable' ? 'color:var(--unc)' : 'color:var(--cov)' }, r.category)),
      h('td', {}, f.complexity.cyclomaticObject ?? '—')));
  }
  wrap.append(h('div', { class: 'sub-h' }, 'Functions'), t);
  // requirement lens
  const tagFns = new Map();
  for (const f of fns) { for (const r of f.requirements) { if (!tagFns.has(r)) tagFns.set(r, new Set()); tagFns.get(r).add(f.id); } for (const rs of Object.values(f.lineRequirements || {})) for (const r of rs) { if (!tagFns.has(r)) tagFns.set(r, new Set()); tagFns.get(r).add(f.id); } }
  if (tagFns.size) {
    const rt = h('table', { class: 'mat', style: 'width:100%' }, h('tr', {}, h('th', {}, 'requirement'), h('th', {}, 'functions'), h('th', {}, 'lines'), h('th', {}, 'outcomes'), h('th', {}, 'gaps')));
    for (const [tag, ids] of [...tagFns.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
      const fs = [...ids].map(fn);
      // lines belonging to this tag: whole function if function-level, else the tagged lines
      let lt = 0, lc = 0, bt = 0, bc = 0, g = 0;
      for (const f of fs) {
        const lineLevel = Object.entries(f.lineRequirements || {}).filter(([, rs]) => rs.includes(tag)).map(([l]) => +l);
        const lines = f.requirements.includes(tag) ? Array.from({ length: f.lineEnd - f.lineStart + 1 }, (_, i) => f.lineStart + i) : lineLevel;
        for (const l of lines) { const cnt = lineCount(f.fileId, l); if (cnt != null) { lt++; if (cnt > 0) lc++; } }
        for (const b of f.branches) if (lines.includes(b.line)) { bt += 2; bc += (b.trueCount > 0) + (b.falseCount > 0); }
        for (const gp of fnGaps(f)) if (gp.line && lines.includes(gp.line) && !resFor(gp.key)) g++;
      }
      const firstLine = Math.min(...fs.map(f => f.requirements.includes(tag) ? f.lineStart : Math.min(...Object.entries(f.lineRequirements || {}).filter(([, rs]) => rs.includes(tag)).map(([l]) => +l))));
      rt.append(h('tr', { class: 'frow' + (S.lens === tag ? ' lens-on' : ''), style: 'cursor:pointer', title: 'click: highlight every line implementing this requirement (hover-scroll can be switched on above)',
        onmouseenter: () => { if (S.hoverReq) revealLine(firstLine); },
        onclick: e => { S.lens = S.lens === tag ? null : tag; pane.querySelectorAll('tr.lens-on').forEach(x => x.classList.remove('lens-on')); if (S.lens) e.currentTarget.classList.add('lens-on'); applyLens(fileId, firstLine); } },
        h('td', {}, h('span', { class: 'req' }, tag)), h('td', { class: 'mono', style: 'font-size:11px' }, fs.map(f => f.name).join(', ')),
        h('td', { style: lc === lt ? 'color:var(--cov)' : 'color:var(--unc)' }, `${lc}/${lt}`), h('td', { style: bt && bc < bt ? 'color:var(--unc)' : '' }, bt ? `${bc}/${bt}` : '—'), h('td', { style: g ? 'color:var(--jus);font-weight:600' : '' }, g)));
    }
    wrap.append(h('div', { class: 'sub-h' }, 'Requirement lens — click a requirement to highlight its implementation'), rt);
  }
  // coupling with other files
  const out = new Map(), inn = new Map(), globs = new Map();
  for (const f of fns) {
    for (const e of f.callees) if (e.functionId != null && fn(e.functionId).fileId !== fileId) { const d = file(fn(e.functionId).fileId).display; if (!out.has(d)) out.set(d, new Set()); out.get(d).add(`${f.name} → ${fn(e.functionId).name}`); }
    for (const e of f.callers) if (e.functionId != null && fn(e.functionId).fileId !== fileId) { const d = file(fn(e.functionId).fileId).display; if (!inn.has(d)) inn.set(d, new Set()); inn.get(d).add(`${fn(e.functionId).name} → ${f.name}`); }
    for (const d of (f.dataRefs || [])) { const g = glob(d.globalId); const others = [...g.readers, ...g.writers, ...g.addressTakers].filter(id => fn(id).fileId !== fileId); if (others.length) { if (!globs.has(g.name)) globs.set(g.name, new Set()); for (const o of others) globs.get(g.name).add(file(fn(o).fileId).display); } }
  }
  if (out.size || inn.size || globs.size) {
    const ct = h('table', { class: 'mat', style: 'width:100%' }, h('tr', {}, h('th', {}, 'coupling'), h('th', {}, 'with'), h('th', {}, 'edges')));
    for (const [d, es] of out) ct.append(h('tr', {}, h('td', {}, 'calls into'), h('td', { class: 'mono' }, d), h('td', { class: 'mono muted', style: 'font-size:11px' }, [...es].slice(0, 6).join(' · ') + (es.size > 6 ? ` · +${es.size - 6}` : ''))));
    for (const [d, es] of inn) ct.append(h('tr', {}, h('td', {}, 'called from'), h('td', { class: 'mono' }, d), h('td', { class: 'mono muted', style: 'font-size:11px' }, [...es].slice(0, 6).join(' · ') + (es.size > 6 ? ` · +${es.size - 6}` : ''))));
    for (const [g, ds] of globs) ct.append(h('tr', {}, h('td', {}, 'shares global'), h('td', { class: 'mono' }, g), h('td', { class: 'mono muted', style: 'font-size:11px' }, [...ds].join(' · '))));
    wrap.append(h('div', { class: 'sub-h' }, 'Coupling with other files (A-7 obj 8 / 6.3.4.b)'), ct);
  }
  if (dead.length) wrap.append(h('div', { class: 'sub-h' }, 'Dead-code candidates in this file'), h('div', { class: 'mono', style: 'padding:4px 8px;font-size:12px' }, dead.map(d => h('div', {}, h('a', { class: 'lnk', onclick: () => navigate(`#/fn/${d.functionId}`) }, d.name), ` · ${d.size} B · ${d.count === 0 ? 'never executed' : d.count + '×'}`))));
  if (S.ev.baseline && ix.changed.size) wrap.append(h('div', { class: 'note' }, `${ix.changed.size} line(s) belong to functions changed since the baseline — marked ∆ in the gutter.`));
  body.append(wrap); pane.append(h('div', { class: 'pane-tools' }, h('span', {}, 'File dashboard'),
    h('label', { title: 'hovering a function row scrolls the source to it' }, h('input', { type: 'checkbox', checked: S.hoverFn || null, onchange: e => { S.hoverFn = e.target.checked; localStorage.setItem('sl-hover-fn', S.hoverFn ? '1' : '0'); } }), ' hover-scroll functions'),
    h('label', { title: 'hovering a requirement row scrolls the source to its implementation' }, h('input', { type: 'checkbox', checked: S.hoverReq || null, onchange: e => { S.hoverReq = e.target.checked; localStorage.setItem('sl-hover-req', S.hoverReq ? '1' : '0'); } }), ' hover-scroll requirements'),
    h('span', { class: 'muted hint', style: 'margin-left:auto' }, 'f = back to function · ] / [ = next / previous uncovered region')), body);
}

function renderFileSource(pane, fileId, ix, fns, gotoLine) {
  const fl = file(fileId); const n = fl.text.length;
  const tools = h('div', { class: 'pane-tools' }, h('span', {}, 'Source'),
    h('label', { title: 'sticky headers at each function with coverage, gaps, review state and tags (off = plain source)' }, h('input', { type: 'checkbox', checked: S.fnHeaders || null, onchange: e => { S.fnHeaders = e.target.checked; localStorage.setItem('sl-fnhdr', S.fnHeaders ? '1' : '0'); renderFile(fileId); } }), ' function headers'),
    h('label', { title: 'all lines · findings only (uncovered, partly covered, gaps, decisions, inferred, changed) · uncovered and partly covered only' }, 'show ', h('select', { onchange: e => { S.fold = e.target.value; localStorage.setItem('sl-fold', S.fold); renderFile(fileId); } },
      h('option', { value: 'all', selected: S.fold === 'all' || null }, 'all lines'),
      h('option', { value: 'findings', selected: S.fold === 'findings' || null }, 'findings only'),
      h('option', { value: 'uncovered', selected: S.fold === 'uncovered' || null }, 'uncovered only'))),
    h('label', {}, h('input', { type: 'checkbox', checked: S.density || null, onchange: e => { S.density = e.target.checked; renderFile(fileId); } }), ' instructions / line'),
    h('span', { class: 'muted hint', style: 'margin-left:auto' }, 'strip on the right = whole file · click to jump'));
  const body = h('div', { class: 'src-scroll' });
  const shell = h('div', { class: 'pane-body file-body' }, body);
  const t = h('table', { class: 'src' + (plainMode() ? ' plain' : '') });
  const fnStart = new Map(fns.map(f => [f.lineStart, f]));
  // folding looks at the coverage *data*, not the overlay: an uncovered line is
  // a finding even when the view is in "No coverage" mode
  // A line that ran but holds a statement that did not is work still to do,
  // so "uncovered only" shows it too -- somebody filtering for what is left
  // is not helped by hiding a return that never executed (#74).
  const uncovered = ln => fnAtLine(fileId, ln) && (lineCount(fileId, ln) === 0 || lineIsPartial(fileId, ln));
  const finding = S.fold === 'uncovered'
    ? ln => uncovered(ln) || (S.lens && ix.tags.get(ln)?.has(S.lens))
    : ln => uncovered(ln) || ix.gaps.has(ln) || ix.inferred.has(ln) || ix.branches.has(ln) || ix.decisions.has(ln) || (S.fnHeaders && fnStart.has(ln)) || ix.changed.has(ln) || (S.lens && ix.tags.get(ln)?.has(S.lens));
  const interesting = ln => finding(ln) || finding(ln - 1) || finding(ln + 1); // one line of context each side
  const maxD = Math.max(1, ...ix.density);
  let ln = 1;
  while (ln <= n) {
    if (S.fold !== 'all') {
      // collapse a run of >= 4 uninteresting lines
      let e = ln; while (e <= n && !interesting(e)) e++;
      if (e - ln >= 4) {
        const from = ln, to = e - 1;
        const row = h('tr', { class: 'fold' }, h('td', { colspan: 5, onclick: () => { row.replaceWith(...Array.from({ length: to - from + 1 }, (_, i) => lineRow(from + i))); } }, `… ${to - from + 1} lines ${S.fold === 'uncovered' ? 'covered or not instrumented' : 'without findings'} — click to expand …`));
        t.append(row); ln = e; continue;
      }
    }
    t.append(lineRow(ln)); ln++;
  }
  function lineRow(ln) {
    const f = fnAtLine(fileId, ln);
    const frag = document.createDocumentFragment();
    if (S.fnHeaders && fnStart.has(ln)) {
      // optional sticky header: name, range, tags, and the per-function facts
      const ff = fnStart.get(ln); const st = fnStats(ff); const r = ff.object ? s2oOf(ff) : null;
      frag.append(h('tr', { class: 'fnhdr' }, h('td', { colspan: 5 },
        h('a', { class: 'lnk mono', style: 'font-weight:600', onclick: () => navigate(`#/fn/${ff.id}`) }, ff.name),
        h('span', { class: 'muted' }, ` · L${ff.lineStart}–${ff.lineEnd}`),
        plainMode() ? null : [h('span', { class: 'muted' }, ' · '), h('span', { style: st.linesPct === 100 ? 'color:var(--cov)' : 'color:var(--unc)' }, fmtPct(st.linesPct)), st.conds ? h('span', { class: 'muted' }, ` · MC/DC ${st.condc}/${st.conds}`) : null],
        st.gaps ? h('span', { style: st.unresolved ? 'color:var(--jus)' : 'color:var(--fg-2)' }, ` · gaps ${st.unresolved}/${st.gaps}`) : null,
        ff.object ? h('span', { class: 'muted' }, ` · v(G) ${ff.complexity.cyclomaticObject ?? '—'} · `) : h('span', { class: 'muted' }, ' · no object code'),
        ff.object ? h('span', { style: !r ? 'color:var(--jus)' : stale(ff) ? 'color:var(--unc)' : 'color:var(--cov)' }, !r ? 'not reviewed' : stale(ff) ? 'review STALE' : `reviewed: ${r.category}`) : null,
        ff.requirements.map(q => reqChip(q, { style: 'margin-left:6px' })))));
    }
    const stt = f ? lineStatus(fileId, ln) : 'none'; const cnt = lineCount(fileId, ln);
    const tr = h('tr', { class: 'l ' + stt + (S.lens && ix.tags.get(ln)?.has(S.lens) ? ' lens' : ''), 'data-line': ln, onclick: () => { if (f) navigate(`#/fn/${f.id}`); } });
    const lnCell = h('td', { class: 'ln' }, ln, ix.changed.has(ln) ? h('span', { class: 'delta', title: 'function changed since baseline' }, '∆') : null);
    if (f && (stt === 'unc' || stt === 'jus')) { lnCell.title = 'Resolve this uncovered line'; lnCell.onclick = e => { e.stopPropagation(); openResolution(lineKey(fileId, ln), `Uncovered line ${ln}`, `${fl.display}:${ln}  ${fl.text[ln - 1].trim()}`); }; }
    tr.append(lnCell, h('td', { class: 'ct' }, cnt == null ? '' : cnt));
    if (S.density) { const d = ix.density[ln] || 0; tr.append(h('td', { class: 'dens', title: `${d} instruction(s) attributed to this line${ix.gen[ln] ? `, ${ix.gen[ln]} compiler-generated` : ''}` }, d ? h('span', { class: 'dbar' }, h('i', { style: `width:${Math.max(4, 100 * d / maxD)}%` }), h('b', {}, d)) : h('span', { class: 'muted', style: 'font-size:10px' }, f && cnt != null ? '0' : ''))); }
    else tr.append(h('td', { class: 'dens', style: 'width:0;padding:0' }));
    const codeTd = h('td', { class: 'code', html: highlight(fl.text[ln - 1] || '') });
    for (const [ff, g] of (ix.gaps.get(ln) || [])) codeTd.append(h('span', { class: 'gapmark' + (resFor(g.key) ? ' res' : ''), style: 'margin-left:8px', title: `${slug(g.kind)}: ${g.description}`, onclick: e => { e.stopPropagation(); S.focusGap = g.key; navigate(`#/fn/${ff.id}/gaps`); } }, '▲'));
    if (ix.inferred.has(ln)) codeTd.append(h('span', { class: 'badge amber', style: 'margin-left:8px;font-size:9px' }, 'inferred'));
    for (const r of ((f && f.lineRequirements || {})[ln] || [])) codeTd.append(reqChip(r, { style: 'margin-left:8px;font-size:10px' }));
    tr.append(codeTd);
    frag.append(tr);
    const brs = ix.branches.get(ln), decs = ix.decisions.get(ln);
    if ((brs || decs) && !plainMode()) {
      const ann = h('div', { class: 'ann-row' });
      for (const [ff, d, i] of decs || []) ann.append(h('span', { class: 'bchip dec', onclick: () => navigate(`#/fn/${ff.id}/mcdc`) }, 'MC/DC', h('span', { class: 'o ' + (d.mcdcCovered ? 'cov' : 'unc') }, `${d.conditions.filter(c => c.independenceCovered).length}/${d.conditions.length}`), `T ${d.trueCount} · F ${d.falseCount}`));
      for (const [ff, b] of brs || []) {
        const [bk0, bk1] = outcomeKeys(b), [bl0, bl1] = outcomeLabels(b);
        const short = l => (l === 'true' ? 'T' : l === 'false' ? 'F' : l === 'outcome A' ? 'A' : 'B');
        ann.append(h('span', { class: 'bchip', title: dirKnown(b) ? null : 'gcov reports a CFG edge, not the polarity of the condition, so these are the two outcomes rather than true and false' }, `${branchSite(b)}`,
          h('span', { class: 'o ' + (b.trueCount ? 'cov' : resFor(branchKey(ff.id, b, bk0)) ? 'jus' : 'unc'), title: bl0 }, `${short(bl0)} ${b.trueCount}`),
          h('span', { class: 'o ' + (b.falseCount ? 'cov' : resFor(branchKey(ff.id, b, bk1)) ? 'jus' : 'unc'), title: bl1 }, `${short(bl1)} ${b.falseCount}`)));
      }
      frag.append(h('tr', { class: 'ann' }, h('td', { colspan: 5 }, ann)));
    }
    return frag;
  }
  body.append(t);
  // minimap: a sibling column of the scroll area, never scrolls itself
  const mm = h('canvas', { class: 'minimap', title: 'whole file: green covered · striped partly covered · red uncovered · amber gap · purple inferred · click to jump' });
  shell.append(mm);
  pane.append(tools, shell);
  const paint = () => {
    const H = shell.clientHeight; if (H <= 0) return; mm.width = 14; mm.height = H; mm.style.height = H + 'px';
    const g = mm.getContext('2d'); g.clearRect(0, 0, 14, H);
    const css = k => getComputedStyle(document.documentElement).getPropertyValue(k).trim();
    const col = { cov: css('--cov'), unc: css('--unc'), jus: css('--jus'), unk: css('--line-2'), part: css('--jus') };
    const scale = H / n;
    for (let l = 1; l <= n; l++) {
      const st = fnAtLine(fileId, l) ? lineStatus(fileId, l) : 'none';
      const y = (l - 1) * scale, hh = Math.max(1, scale);
      if (st === 'cov' || st === 'unc' || st === 'jus') { g.fillStyle = col[st]; g.fillRect(0, y, 8, hh); }
      // Partly covered: half the width in the same amber the row uses, so the
      // strip does not show a gap where the source pane shows a finding.
      if (st === 'part') { g.fillStyle = col.cov; g.fillRect(0, y, 8, hh); g.fillStyle = col.part; g.fillRect(0, y, 4, hh); }
      if (ix.gaps.has(l)) { g.fillStyle = col.jus; g.fillRect(9, y, 5, Math.max(1.5, hh)); }
      if (ix.inferred.has(l)) { g.fillStyle = css('--accent-2'); g.fillRect(9, y, 5, Math.max(1.5, hh)); }
      if (S.lens && ix.tags.get(l)?.has(S.lens)) { g.fillStyle = css('--accent'); g.fillRect(0, y, 2, Math.max(1, hh)); }
    }
    // viewport indicator
    const rows = body.querySelectorAll('tr.l'); if (!rows.length) return;
    const first = [...rows].find(r => r.getBoundingClientRect().bottom > body.getBoundingClientRect().top);
    const last = [...rows].reverse().find(r => r.getBoundingClientRect().top < body.getBoundingClientRect().bottom);
    if (first && last) { g.strokeStyle = css('--fg-2'); g.lineWidth = 1; g.strokeRect(0.5, (+first.dataset.line - 1) * scale + 0.5, 13, Math.max(3, (+last.dataset.line - +first.dataset.line + 1) * scale)); }
  };
  mm.onclick = e => { const l = Math.max(1, Math.min(n, Math.round(e.offsetY / (mm.height / n)))); const row = body.querySelector(`tr[data-line="${l}"]`) || [...body.querySelectorAll('tr.l')].find(r => +r.dataset.line >= l); if (row) row.scrollIntoView({ block: 'center' }); };
  shell._paint = paint;
  body.addEventListener('scroll', () => requestAnimationFrame(paint));
  setTimeout(() => { paint(); if (gotoLine) { const row = body.querySelector(`tr[data-line="${gotoLine}"]`); if (row) { row.scrollIntoView({ block: 'center' }); row.classList.add('sel'); } } }, 0);
  new ResizeObserver(() => paint()).observe(shell);
}
function applyLens(fileId, firstLine) {
  const body = $('#content .src-scroll'); const ix = S.fileIx; if (!body || !ix) { renderFile(fileId); return; }
  for (const tr of body.querySelectorAll('tr.l')) { const ln = +tr.dataset.line; tr.classList.toggle('lens', !!(S.lens && ix.tags.get(ln)?.has(S.lens))); }
  // a fold may hide the first line: expand it
  if (S.lens && firstLine && !body.querySelector(`tr[data-line="${firstLine}"]`)) for (const fold of body.querySelectorAll('tr.fold')) { fold.querySelector('td').click(); if (body.querySelector(`tr[data-line="${firstLine}"]`)) break; }
  const shell = body.parentElement; if (shell && shell._paint) shell._paint();
  if (S.lens && firstLine) { const f = fnAtLine(fileId, firstLine); if (f && f.lineStart === firstLine) revealFunction(f); else revealLine(firstLine); }
}
/* scroll the file source so line `ln` sits just below the sticky function header */
function revealLine(ln) {
  const body = $('#content .src-scroll'); if (!body) return;
  let row = body.querySelector(`tr[data-line="${ln}"]`);
  if (!row) { for (const fold of body.querySelectorAll('tr.fold')) { fold.querySelector('td').click(); row = body.querySelector(`tr[data-line="${ln}"]`); if (row) break; } }
  if (!row) return;
  const hdr = body.querySelector('tr.fnhdr td'); const hdrH = hdr ? hdr.getBoundingClientRect().height : 0;
  body.scrollTop = Math.max(0, row.getBoundingClientRect().top - body.getBoundingClientRect().top + body.scrollTop - hdrH - 2);
  body.querySelectorAll('tr.l.flash').forEach(e => e.classList.remove('flash'));
  row.classList.add('flash');
  clearTimeout(revealLine.timer); revealLine.timer = setTimeout(() => row.classList.remove('flash'), 1400);
}
/* scroll the file source so `f` starts at the top (its sticky header lands in place) and flash its lines */
function revealFunction(f) {
  const body = $('#content .src-scroll'); if (!body) return;
  // expand a fold that hides the function start
  let row = body.querySelector(`tr[data-line="${f.lineStart}"]`);
  if (!row) { for (const fold of body.querySelectorAll('tr.fold')) { fold.querySelector('td').click(); row = body.querySelector(`tr[data-line="${f.lineStart}"]`); if (row) break; } }
  if (!row) return;
  const hdr = row.previousElementSibling && row.previousElementSibling.classList.contains('fnhdr') ? row.previousElementSibling : row;
  const top = hdr.getBoundingClientRect().top - body.getBoundingClientRect().top + body.scrollTop;
  body.scrollTop = Math.max(0, top - 2);
  body.querySelectorAll('tr.l.flash').forEach(e => e.classList.remove('flash'));
  for (let l = f.lineStart; l <= f.lineEnd; l++) { const r = body.querySelector(`tr[data-line="${l}"]`); if (r) r.classList.add('flash'); }
  clearTimeout(revealFunction.timer);
  revealFunction.timer = setTimeout(() => body.querySelectorAll('tr.l.flash').forEach(e => e.classList.remove('flash')), 1400);
}
function jumpUncovered(dir) {
  const body = $('#content .src-scroll'); if (!body) return;
  const rows = [...body.querySelectorAll('tr.l.unc, tr.l.jus, tr.l.lens')]; if (!rows.length) return;
  const top = body.getBoundingClientRect().top + 40;
  let target;
  if (dir > 0) target = rows.find(r => r.getBoundingClientRect().top > top + 20) || rows[0];
  else target = [...rows].reverse().find(r => r.getBoundingClientRect().top < top - 20) || rows[rows.length - 1];
  target.scrollIntoView({ block: 'center' }); body.querySelectorAll('tr.sel').forEach(e => e.classList.remove('sel')); target.classList.add('sel');
}

/* ---------------- compare (variants) ---------------- */
async function renderCompare() {
  const c = $('#content'); c.innerHTML = '';
  const o = h('div', { class: 'overview' });
  o.append(h('h2', {}, 'Compare build variants — what the optimiser did to each function'), h('div', { class: 'note', style: 'margin-bottom:12px' }, 'Exploratory builds compiled by `butterfly build`. Use them to understand merged returns, predication, inlining and stack growth; the certification evidence is the configuration-controlled build.'));
  c.append(o);
  const list = await allVariants();
  const names = list.map(([n]) => n);
  // summary tiles per variant
  const st = h('table', { class: 'tbl' }, h('tr', {}, h('th', {}, 'variant'), h('th', {}, 'flags'), h('th', {}, 'functions with object code'), h('th', {}, 'instructions'), h('th', {}, 'obj projected'), h('th', {}, 'gaps'), h('th', {}, 'inferred blocks'), h('th', {}, 'stack worst case')));
  for (const [n, ev] of list) {
    const s = ev.summary; const inferred = ev.functions.reduce((a, f) => a + fnObjects(f).reduce((x, ob) => x + ob.blocks.filter(b => b.note).length, 0), 0);
    st.append(h('tr', { class: 'row', onclick: () => switchVariant(n) }, h('td', { class: 'mono' }, h('b', {}, n)), h('td', { class: 'mono muted', style: 'font-size:11px;white-space:nowrap' }, (ev.provenance.buildInfo.flags_target || '').split(' ').filter(f => !['-std=c11','-Wall','-Wextra','-g'].includes(f)).join(' ')), h('td', {}, ev.functions.filter(f => f.object).length + ' / ' + ev.functions.length), h('td', {}, s.instructionsTotal), h('td', {}, `${fmtPct(pct(s.instructionsProjectedCovered, s.instructionsTotal))} (${s.instructionsProjectedUncovered} uncovered)`), h('td', {}, s.gapsTotal), h('td', {}, inferred), h('td', {}, s.stackWorstCase != null ? fmtB(s.stackWorstCase) : '—')));
  }
  o.append(st, h('h2', {}, 'Per function'));
  const metrics = [
    ['instr', f => f.object ? objTotals(f).instrTotal : null],
    ['blocks', f => f.object ? f.complexity.objectBlocks : null],
    ['v(G)', f => f.complexity.cyclomaticObject],
    ['uncov', f => f.object ? objTotals(f).instrProjectedUncovered : null],
    ['gaps', f => f.object ? fnGaps(f).length : null],
    ['inferred', f => f.object ? fnObjects(f).reduce((a, ob) => a + ob.blocks.filter(b => b.note).length, 0) : null],
    ['stack', f => f.symbolId != null && S.evCache ? null : null],
  ].filter(m => m[0] !== 'stack');
  const stackOf = (ev, f) => f.symbolId != null ? ev.symbols[f.symbolId].stack.worstCase : null;
  metrics.push(['stack B', null]);
  const t = h('table', { class: 'tbl nowrap' });
  const hdr = h('tr', {}, h('th', {}, 'function'));
  for (const n of names) hdr.append(h('th', { colspan: metrics.length, style: 'text-align:center;border-left:1px solid var(--line)' }, n));
  t.append(hdr);
  const hdr2 = h('tr', {}, h('th', {}));
  for (const _ of names) metrics.forEach((m, i) => hdr2.append(h('th', { style: i === 0 ? 'border-left:1px solid var(--line)' : '' }, m[0])));
  t.append(hdr2);
  const fnames = [...new Set(list.flatMap(([, ev]) => ev.functions.map(f => f.name)))].sort();
  for (const name of fnames) {
    const row = h('tr', { class: 'row' });
    row.append(h('td', { class: 'mono' }, name));
    const base = list[0][1].functions.find(f => f.name === name);
    let firstVariantWith = null;
    list.forEach(([n, ev], vi) => {
      const f = ev.functions.find(x => x.name === name);
      if (f && f.object && firstVariantWith == null) firstVariantWith = [n, f.id];
      metrics.forEach((m, i) => {
        const val = !f ? null : m[1] ? m[1](f) : stackOf(ev, f);
        const bval = !base ? null : m[1] ? m[1](base) : stackOf(list[0][1], base);
        const diff = vi > 0 && val !== bval && (val != null || bval != null);
        const worse = diff && (m[0] === 'uncov' || m[0] === 'gaps' || m[0] === 'inferred') && (val ?? 0) > (bval ?? 0);
        row.append(h('td', { style: (i === 0 ? 'border-left:1px solid var(--line);' : '') + (diff ? (worse ? 'color:var(--unc);font-weight:600' : 'color:var(--accent);font-weight:600') : ''), title: !f ? 'not in this variant' : val == null ? 'no object code (inlined / removed)' : '' }, !f ? '·' : val == null ? '—' : val));
      });
    });
    row.onclick = () => { if (firstVariantWith) { const [n, id] = firstVariantWith; if (n === S.variant) navigate(`#/fn/${id}`); else switchVariant(n).then(() => { const f = S.ev.functions.find(x => x.name === name); if (f) navigate(`#/fn/${f.id}`); }); } };
    t.append(row);
  }
  o.append(h('div', { class: 'scroll-x' }, t), h('p', { class: 'muted', style: 'margin-top:10px' }, 'Blue = differs from the first variant · red = more uncovered instructions, gaps or inferred blocks than the first variant · — = no object code for the function in that variant (inlined or removed).'));
}

/* ---------------- function view ---------------- */
function renderFunction() {
  const f = fn(S.fnId); const c = $('#content'); c.innerHTML = '';
  const st = fnStats(f); const fl = file(f.fileId);
  // every box here explains itself through its `?` and opens its own tab
  const go = t => () => navigate(`#/fn/${f.id}/${t}`);
  const metric = (v, label, cls, help, tab, tabLabel) => metricBox(v, label, cls, help, tab ? go(tab) : null, tabLabel);
  const head = h('div', { class: 'fn-head' },
    h('h1', {}, f.name),
    h('span', { class: 'loc' }, `${fl.display}:${f.lineStart}–${f.lineEnd}`, f.object ? ` · ${f.object.symbol} @ ${hex(f.object.address)} (${f.object.size} B)` : ' · no object code',
      (f.variants || []).length ? h('a', { class: 'lnk', style: 'margin-left:6px', title: 'constructor/destructor variants the compiler emits beside the body: analysed in the Object tab, with no coverage counter of their own', onclick: () => setTab('object') }, `+ ${f.variants.map(v => v.variant).join(' ')} (${f.variants.reduce((a, v) => a + v.size, 0)} B)`) : null),
    // A function defined in a header is compiled into every translation unit
    // that includes it, and the evidence carries each one separately. Name the
    // others: otherwise the reader cannot tell which copy this is, or that the
    // rest exist, and a go-to-definition that picked one looks arbitrary
    // rather than considered.
    ...(() => {
      const sibs = tuSiblings(f);
      if (!sibs.length) return [];
      // "copies", not "instantiations": the header already has an
      // INSTANTIATIONS metric, and there it means merged coverage records
      return [h('span', { class: 'loc sibs', title: 'the same function, compiled separately into each translation unit that includes its header' },
        `compiled into ${sibs.length + 1} units · also `,
        ...sibs.map((g, i) => [i ? ', ' : null, h('a', { class: 'lnk', title: g.name, onclick: () => navigate(`#/fn/${g.id}`) }, tuOf(g))]).flat())];
    })(),
    f.requirements.map(r => reqChip(r, { title: 'requirement tag from source' })),
    // Say it where the function is opened: the line it is written on reads
    // covered, and nothing of that is this code's (#273).
    unrecorded(f) ? h('div', { class: 'note unrecorded' },
      h('b', {}, 'No coverage record of its own. '), 'The compiler gave this code no coverage record; it is written inside ',
      h('a', { class: 'lnk', onclick: () => navigate(`#/fn/${f.unrecordedWithin}`) }, fn(f.unrecordedWithin).name),
      ', and the counts on its lines are that function\'s. Whether this code ran is not established by source coverage: its object code is analysed, nothing of it is projected covered or uncovered, and every instruction with a source line carries a no-coverage-record gap. A trace (measured coverage) is what can show it ran.') : null,
    h('div', { class: 'metrics' },
      unrecorded(f) ? metric('—', 'calls', 'jus', 'calls', 'calls', 'Calls') : metric(f.count, 'calls', f.count ? '' : 'unc', 'calls', 'calls', 'Calls'),
      // no tab owns an instantiation count; the `?` is the whole answer
      f.instantiations > 1 ? metric(f.instantiations, 'instantiations', '', 'instantiations') : null,
      unrecorded(f) ? metric('—', 'lines', 'jus', 'lines') : metric(`${f.linesCovered}/${f.linesTotal}`, 'lines', f.linesCovered === f.linesTotal ? 'cov' : 'unc', 'lines'),
      metric(st.br ? `${st.brc}/${st.br}` : '—', 'outcomes', st.br ? (st.brc === st.br ? 'cov' : 'unc') : '', 'outcomes', 'mcdc', 'MC/DC'),
      metric(st.conds ? `${st.condc}/${st.conds}` : '—', 'MC/DC', st.conds ? (st.condc === st.conds ? 'cov' : 'unc') : '', 'mcdc', 'mcdc', 'MC/DC'),
      metric(f.complexity.cyclomaticObject ?? '—', 'v(G)', '', 'vg', 'flow', 'Flow'),
      metric(f.complexity.maxConditionsPerDecision, 'max cond', '', 'maxcond', 'mcdc', 'MC/DC'),
      ...(() => {
        const tot = f.object ? objTotals(f) : null;
        return [
          metric(tot ? `${tot.instrProjectedCovered}/${tot.instrTotal}` : '—', 'obj proj.', 'jus', 'objproj', 'object', 'Object'),
          hasMeasured() && tot ? metric(`${tot.instrMeasuredCovered}/${tot.instrTotal}`, 'obj meas.', tot.instrMeasuredCovered === tot.instrTotal ? 'cov' : '', 'objmeas', 'object', 'Object') : null,
          hasMeasured() && tot && tot.outcomeEdgesTotal ? metric(`${tot.outcomeEdgesMeasured}/${tot.outcomeEdgesTotal}`, 'obj outcomes', tot.outcomeEdgesMeasured === tot.outcomeEdgesTotal ? 'cov' : 'unc', 'objoutcomes', 'object', 'Object') : null,
          tot ? metric(`${tot.compilerGeneratedTotal}`, 'generated', 'jus', 'generated', 'object', 'Object') : null,
        ];
      })(),
      f.symbolId != null ? metric(sym(f.symbolId).stack.worstCase != null ? `${sym(f.symbolId).stack.worstCase} B` : '∞', 'stack', sym(f.symbolId).stack.bounded ? '' : 'jus', 'stack', 'stack', 'Stack') : null,
      metric(st.gaps ? `${st.unresolved}/${st.gaps}` : '0', 'gaps', st.unresolved ? 'jus' : '', 'gaps', 'gaps', 'Gaps')),
    // Beside the coverage overlay, because the two answer the same question
    // about the source pane: what is painted on it. A real button with a
    // pressed state, in the same segmented control the overlay uses, so it
    // is reachable and readable the same way.
    h('div', { class: 'seg', title: 'Variables tab: mark a hovered variable’s declaration and every line the object code touches it on. Off, hovering marks nothing — the panel still lists every access.' },
      h('button', { class: S.varMarks ? 'on' : '', 'aria-pressed': S.varMarks ? 'true' : 'false',
        onclick: () => { setVarMarks(!S.varMarks); renderFunction(); } }, 'Variable marks')),
    h('div', { class: 'seg', title: 'coverage overlay: none, source-projected, or from the imported execution trace' },
      h('button', { class: S.mode === 'plain' ? 'on' : '', onclick: () => { setMode('plain'); renderFunction(); } }, 'No coverage'),
      h('button', { class: S.mode === 'projected' ? 'on' : '', onclick: () => { setMode('projected'); renderFunction(); } }, 'Projected'),
      hasMeasured() ? h('button', { class: S.mode === 'measured' ? 'on' : '', onclick: () => { setMode('measured'); renderFunction(); } }, 'Measured') : null));
  {
    const r = s2oOf(f);
    const isStale = r && stale(f);
    const cls = !r ? 'amber' : isStale ? 'red' : r.category === 'traceable' ? 'green' : r.category === 'analysed' ? 'blue' : 'red';
    const label = !r ? 'source→object: not reviewed' : `source→object: ${r.category}${r.author ? ' · ' + r.author : ''}${r.date ? ' · ' + r.date : ''}${isStale ? ' · STALE — ' + staleWhy(f) : ''}`;
    const cons = Object.entries(f.constructs || {}).map(([k, v]) => v > 1 ? `${k}×${v}` : k).join(' · ');
    head.append(h('div', { class: 'vrow' },
      h('button', { class: 'badge ' + cls, style: 'cursor:pointer;font-size:11px;padding:3px 8px;background:transparent', title: r ? r.justification : 'Record the CAST-12 source-to-object traceability review for this function', onclick: () => openReview(f) }, label),
      S.ev.changes && (f.commits || []).length ? (() => { const c = S.ev.changes; const ts = [...new Set(f.commits.flatMap(i => c.commits[i].tickets))].map(t => c.tickets[t].id);
        return h('span', { class: 'badge blue', style: 'cursor:help', title: f.commits.map(i => { const cm = c.commits[i]; return `${cm.short.slice(0, 7)} ${cm.date.slice(0, 10)} ${cm.subject}${cm.tickets.length ? '  [' + cm.tickets.map(t => c.tickets[t].id).join(' ') + ']' : ''}`; }).join('\n') }, `${f.commits.length} commit${f.commits.length > 1 ? 's' : ''}${ts.length ? ' · ' + ts.slice(0, 3).join(' ') + (ts.length > 3 ? ' …' : '') : ''}`); })() : null,
      (f.inlined || []).length ? h('span', { class: 'badge blue', title: (f.inlined || []).map(i => `${i.name} at L${i.callLine ?? '?'} (${i.instrCount || 'folded'})`).join('\n'), style: 'cursor:help', onclick: () => setTab('calls') }, `${f.inlined.length} inlined callee${f.inlined.length > 1 ? 's' : ''}`) : null,
      h('span', { class: 'muted', style: 'font-size:11px', title: 'source constructs in this function (for sample adequacy)' }, cons ? 'constructs: ' + cons : 'constructs: none detected')));
  }
  if (S.variants.length > 1) {
    const row = h('div', { class: 'vrow' }, h('span', { class: 'muted' }, 'variants:'));
    head.append(row);
    allVariants().then(list => {
      for (const [name, ev] of list) {
        const g = ev.functions.find(x => x.name === f.name);
        const o = g && g.object;
        row.append(h('button', { class: 'vchip' + (name === S.variant ? ' on' : ''), onclick: () => switchVariant(name), title: o ? `${o.instrTotal} instr · ${g.complexity.objectBlocks} blocks · ${o.instrProjectedUncovered} uncovered · ${o.gaps.length} gaps` : 'no object code (inlined / removed)' },
          h('b', {}, name), o ? ` ${o.instrTotal}i · ${g.complexity.objectBlocks}b · ${o.gaps.length}g` : ' — inlined'));
      }
    });
  }
  const left = h('div', { class: 'pane' });
  const right = h('div', { class: 'pane' });
  c.append(head, h('div', { class: 'panes' }, left, right));
  renderLeft(left, f);
  renderSource(right, f);
}

function renderLeft(pane, f) {
  const disN = f.object ? objTotals(f).disagreements.length : 0;
  const tabs = [['flow', 'Flow'], ['calls', 'Calls'], ['object', 'Object'], ['gaps', 'Gaps', fnGaps(f).length + disN, unresolvedGaps(f)], ['mcdc', 'MC/DC', f.decisions.length || f.branches.length], ['stack', 'Stack'], ['data', 'Data', f.data_refs ? f.data_refs.length : (f.dataRefs || []).length], ['variables', 'Variables', (f.variables || []).length], ['changes', 'Changes', (f.commits || []).length, S.ev.changes && S.ev.changes.impact && S.ev.changes.impact.obligations.some(o => o.functionId === f.id && o.severity === 'blocker')]];
  const tb = h('div', { class: 'tabs' });
  tabs.push(['requirements', 'Requirements', functionReqIds(f).length]);
  for (const [id, label, n, red] of tabs) tb.append(h('div', { class: 'tab' + (S.tab === id ? ' active' : ''), ...activatable(() => setTab(id)) }, label, n ? h('span', { class: 'n' + (red ? ' red' : '') }, n) : null));
  const body = h('div', { class: 'pane-body' });
  pane.append(tb, body);
  // Requirements belong to source, even when a function has no object code.
  if (S.tab === 'requirements') { renderRequirements(body, f); return; }
  // the change history does not need object code
  if (S.tab === 'changes') { renderChanges(body, f); return; }
  if (!f.object && S.tab !== 'mcdc') {
    // say *why* there is none: inlined, folded away, or simply not linked
    const inlinedInto = S.ev.functions.filter(g => (g.inlined || []).some(i => i.functionId === f.id));
    const foldedIn = S.ev.functions.filter(g => (g.foldedCalls || []).some(c => c.functionId === f.id));
    const link = g => h('a', { class: 'lnk', onclick: () => navigate(`#/fn/${g.id}`) }, g.name);
    const join = gs => gs.slice(0, 6).map((g, i) => [i ? ', ' : null, link(g)]).concat(gs.length > 6 ? [` … +${gs.length - 6}`] : []);
    let msg;
    if (inlinedInto.length) {
      msg = h('div', {}, h('b', {}, 'No object code of its own — the compiler inlined it.'), h('br'),
        'Its instructions live inside ', join(inlinedInto), '. Open a caller and look for the ',
        h('code', {}, '⤷ inlined ' + f.name.split('::').pop()), ' markers in the Object view.');
    } else if (foldedIn.length) {
      msg = h('div', {}, h('b', {}, 'No object code — every call was evaluated away.'), h('br'),
        'Called in the source from ', join(foldedIn), ', but the compiler folded the call (constant arguments or interprocedural optimisation), so nothing of this function survives in the image.');
    } else {
      msg = h('div', {}, h('b', {}, 'Not present in the analysed image.'), h('br'),
        'The coverage run exercised this function ', h('b', {}, `${f.count}×`), ', but no symbol of this name is in the binary being analysed. Usually one of: it is not linked into this image (host-only or test-only code — the coverage run and the target build are different builds), it was inlined everywhere without a DWARF inline record, or it is a generic instantiated only in the coverage build.',
        f.instantiations > 1 ? h('div', { class: 'note', style: 'margin-top:8px' }, `${f.instantiations} coverage instantiations were merged into this entry.`) : null);
    }
    body.append(h('div', { class: 'empty', style: 'text-align:left;max-width:52em' }, msg));
    return;
  }
  ({ flow: renderFlow, calls: renderCalls, object: renderObject, gaps: renderGaps, mcdc: renderMcdc, stack: renderStack, data: renderData, variables: renderVariables, changes: renderChanges })[S.tab](body, f);
}

function functionReqIds(f) {
  return [...new Set([...(f.requirements || []), ...Object.values(f.lineRequirements || {}).flat()])];
}

async function renderRequirements(body, f) {
  const ids = functionReqIds(f);
  const forVariant = S.variant;
  body.append(h('div', { class: 'pane-tools' }, h('b', {}, 'Requirements'),
    h('span', { class: 'hint' }, 'Function and line tags · select an ID for details')));
  if (!ids.length) {
    body.append(h('div', { class: 'empty' }, 'No requirement tags are linked to this function.'));
    return;
  }
  const rows = h('tbody');
  const table = h('table', { class: 'requirements-table', 'aria-busy': 'true' },
    h('caption', {}, `Requirements linked to ${f.name}`),
    h('thead', {}, h('tr', {}, h('th', { scope: 'col' }, 'Requirement ID'), h('th', { scope: 'col' }, 'Requirement text'))), rows);
  const cells = ids.map(id => {
    const cell = h('td', { class: 'requirement-text muted' }, 'Loading requirement text…');
    rows.append(h('tr', {}, h('th', { scope: 'row' }, reqChip(id)), cell));
    return cell;
  });
  body.append(table);
  await needReq(ids);
  // Navigation may have replaced the pane while the sidecar was loading.
  if (!body.isConnected || S.variant !== forVariant) return;
  if (reqStale.length) {
    table.before(h('div', { class: 'note', style: 'color:var(--unc);border-left:3px solid var(--unc);padding-left:10px;margin:6px 10px' },
      h('b', {}, 'Requirement statements may be stale. '), reqStale.join(' ')));
  }
  ids.forEach((id, i) => {
    const r = REQ.get(id);
    cells[i].textContent = r ? (r.text || '(this requirement carries no statement)') :
      'Requirement text unavailable. Check the requirements sidecar and server connection.';
    cells[i].classList.toggle('muted', !r);
  });
  table.setAttribute('aria-busy', 'false');
}

/* ---------------- source pane ---------------- */
function renderSource(pane, f) {
  const fl = file(f.fileId);
  const from = Math.max(1, f.lineStart - Math.min(6, f.lineStart - 1)), to = Math.min(fl.text.length, f.lineEnd + 1);
  // start at the requirement comment block if it is right above
  let start = f.lineStart;
  while (start > from && /^\s*(\/\/|\/\*|\*|\*\/|#\[)/.test(fl.text[start - 2] || '')) start--;
  const branchesByLine = new Map();
  for (const b of f.branches) { if (!branchesByLine.has(b.line)) branchesByLine.set(b.line, []); branchesByLine.get(b.line).push(b); }
  const decByLine = new Map(); f.decisions.forEach((d, i) => { if (!decByLine.has(d.line)) decByLine.set(d.line, []); decByLine.get(d.line).push([d, i]); });
  const execByLine = new Map();
  if (hasMeasured() && f.object) for (const ob of fnObjects(f)) for (const b of ob.blocks) for (const i of b.instrs) if (i.line && insFileId(f, i) === f.fileId && i.measured != null) execByLine.set(i.line, Math.max(execByLine.get(i.line) || 0, i.measured));
  const tools = h('div', { class: 'pane-tools' }, h('span', {}, 'Source'), h('span', { class: 'muted hint', style: 'margin-left:auto' }, hasMeasured() ? 'count = instrumented run · exec = trace (max over the line\'s instructions)' : 'count = executions in the instrumented run · hover a line to locate it in the object code'));
  const body = h('div', { class: 'pane-body' });
  const t = h('table', { class: 'src' + (plainMode() ? ' plain' : '') });
  for (let ln = start; ln <= to; ln++) {
    const stt = lineStatus(f.fileId, ln); const cnt = lineCount(f.fileId, ln);
    const tr = h('tr', { class: 'l ' + stt, 'data-line': ln,
      onmouseenter: () => hl('line', ln, true), onmouseleave: () => hl('line', ln, false),
      onclick: () => selectLine(ln) });
    const lnCell = h('td', { class: 'ln' }, ln);
    if (stt === 'unc' || stt === 'jus') { lnCell.title = 'Resolve this uncovered line (6.4.4.3)'; lnCell.onclick = e => { e.stopPropagation(); openResolution(lineKey(f.fileId, ln), `Uncovered line ${ln}`, `${fl.display}:${ln}  ${fl.text[ln - 1].trim()}`); }; }
    // Say why it is striped rather than green, in the place it is noticed.
    if (stt === 'part') tr.title = partialWhy(f.fileId, ln);
    tr.append(lnCell, h('td', { class: 'ct' }, cnt == null ? '' : cnt));
    if (hasMeasured()) tr.append(h('td', { class: 'ex', title: 'executions in the trace' }, execByLine.has(ln) ? execByLine.get(ln) : ''));
    const codeTd = h('td', { class: 'code', html: highlight(fl.text[ln - 1] || '') });
    for (const r of ((f.lineRequirements || {})[ln] || [])) codeTd.append(reqChip(r, { style: 'margin-left:10px;font-size:10px', title: 'requirement tagged on this line' }));
    tr.append(codeTd);
    t.append(tr);
    const brs = branchesByLine.get(ln), decs = decByLine.get(ln);
    if ((brs || decs) && !plainMode()) {
      const ann = h('div', { class: 'ann-row' });
      for (const [d, i] of decs || []) {
        const nOk = d.conditions.filter(c => c.independenceCovered).length, nAll = d.conditions.length;
        // spell the counts out: "T 2 · F 1" is unreadable until someone tells
        // you it is the decision's own outcomes, not a pass/fail
        const why = `This decision: ${nOk} of its ${nAll} conditions proven to affect the outcome on their own.\n`
          + `The decision itself came out true ${d.trueCount}× and false ${d.falseCount}× across the run.\n`
          + `Click for the truth table.`;
        ann.append(h('span', { class: 'bchip dec', title: why, onclick: () => { S.tab = 'mcdc'; location.hash = `#/fn/${f.id}/mcdc`; } },
          'MC/DC', h('span', { class: 'o ' + (d.mcdcCovered ? 'cov' : 'unc') }, `${nOk}/${nAll}`),
          h('span', { class: 'muted' }, `decision T ${d.trueCount} · F ${d.falseCount}`)));
      }
      for (const b of brs || []) {
        // which condition this column belongs to, so the chip can name it
        const cond = f.decisions.flatMap(d => d.conditions.map((c, i) => [d, c, i])).find(([, c]) => c.line === ln && c.col === b.col);
        const cname = cond ? (fl.text[ln - 1] || '').slice(b.col - 1, b.colEnd ? b.colEnd - 1 : undefined).replace(/[\s()]*(&&|\|\|)\s*$/, '').trim() : '';
        const label = cname ? `\`${cname}\`` : `the condition at column ${b.col}`;
        // Keyed and labelled by what the producer established: T/F where the
        // polarity is known, A/B where it is not, so a justification recorded
        // here is the same one the MC/DC pane and the worklist see.
        const [ok0, ok1] = outcomeKeys(b), [ow0, ow1] = outcomeLabels(b);
        const oc = (o, word, n) => { const key = branchKey(f.id, b, o); const cls = n > 0 ? 'cov' : resFor(key) ? 'jus' : 'unc';
          return h('span', { class: 'o ' + cls, title: n > 0 ? `${label} was evaluated ${word} ${n}× in the run` : `${label} was never ${word} — click to record a justification`,
            onclick: n > 0 ? null : e => { e.stopPropagation(); openResolution(key, `Untaken ${word} at ${branchSite(b)}`, `${fl.display}:${ln}  ${fl.text[ln - 1].trim()}`); } }, `${o} ${n}`); };
        ann.append(h('span', { class: 'bchip', title: dirKnown(b) ? `${label} — how often it came out each way when the run reached it` : `${label} — gcov reports a CFG edge rather than the polarity, so these are the two outcomes and neither is named true` }, `${branchSite(b)}`, oc(ok0, ow0, b.trueCount), oc(ok1, ow1, b.falseCount)));
      }
      t.append(h('tr', { class: 'ann' }, h('td', { colspan: hasMeasured() ? 4 : 3 }, ann)));
    }
  }
  body.append(t);
  pane.append(tools, body);
}

function highlight(src) {
  // Group 6 is a bare identifier. It is matched last, so a keyword, a type, a
  // number, a comment or a string is consumed first and never becomes a link
  // -- `int` in a comment is prose, not a definition.
  const re = /(\/\/.*$|\/\*[\s\S]*?\*\/|\/\*.*$|^\s*\*.*$)|("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')|\b(if|else|for|while|do|switch|case|default|return|break|continue|goto|sizeof|struct|union|enum|typedef|static|const|volatile|extern|inline|fn|let|mut|pub|impl|match|loop|use|mod|as|in|unsafe|where|trait|type|Self|self|true|false)\b|\b(u?int(?:8|16|32|64)?_t|void|char|short|int|long|float|double|unsigned|signed|bool|u8|u16|u32|u64|i8|i16|i32|i64|usize|isize|f32|f64|str|String)\b|\b(0x[0-9a-fA-F]+u?|\d+(?:\.\d+)?[uUlLfF]*)\b|\b([A-Za-z_]\w*)\b/g;
  let out = '', last = 0;
  for (const m of src.matchAll(re)) {
    out += esc(src.slice(last, m.index));
    if (m[6]) {
      // an identifier: a link only when the index actually knows it
      out += defLink(m[6]);
    } else {
      const cls = m[1] ? 'c' : m[2] ? 's' : m[3] ? 'k' : m[4] ? 't' : 'n';
      out += `<span class="${cls}">${esc(m[0])}</span>`;
    }
    last = m.index + m[0].length;
  }
  return out + esc(src.slice(last));
}

/// The translation unit a function was compiled into, as a bare file name.
/// The evidence prefixes an internal-linkage function with its unit
/// (`main.c:geom_clamp`); anything else belongs to the file it was found in.
/// A C++ name is never split -- the colons in `flight::scale_by<int>` are
/// namespacing, not a unit prefix.
function tuOf(f) {
  if (!f.name.includes('::')) {
    const i = f.name.lastIndexOf(':');
    if (i > 0) return f.name.slice(0, i);
  }
  return file(f.fileId).display.split('/').pop();
}

/// The translation unit the reader is currently in. Taken from the function
/// itself, not from its file: a header function's *file* is the header, so
/// reading `geom.c:geom_clamp` in inc/geom.h the unit is still `geom.c`.
function currentTu() {
  if (S.fnId != null) return tuOf(fn(S.fnId));
  return S.fileId != null ? file(S.fileId).display.split('/').pop() : null;
}

/// The same function compiled into other translation units: same source
/// range, and a name that differs only in its unit prefix. The name test is
/// what keeps C++ template instantiations out -- `scale_by<int>` and
/// `scale_by<short>` share a header range but are different functions in one
/// unit, not one function in several.
function tuSiblings(f) {
  const bare = n => { const i = n.includes('::') ? -1 : n.lastIndexOf(':'); return i > 0 ? n.slice(i + 1) : n; };
  const mine = bare(f.name);
  return file(f.fileId).functionIds.map(fn)
    .filter(g => g.id !== f.id && g.lineStart === f.lineStart && g.lineEnd === f.lineEnd && bare(g.name) === mine);
}

/// The analysed functions living at a tag's definition site. A function
/// defined in a header is compiled once per translation unit that includes
/// it, so the evidence holds one entry per TU at the same header lines and
/// the tags index cannot name a single one -- it leaves functionId null.
/// That is not a reason to drop the reader in the file view.
function defFns(t) {
  if (t.fileId == null) return [];
  return file(t.fileId).functionIds.map(fn).filter(f => t.line >= f.lineStart && t.line <= f.lineEnd);
}

/// Which instantiation to open. Prefer one that has object code, so the
/// reader lands on a control-flow graph rather than on the "no object code"
/// note, and among those prefer the one belonging to the file being read.
/// The function header names the siblings, so this choice hides nothing.
function defFn(t) {
  if (t.functionId != null) return t.functionId;
  // highlight() calls defLink for every identifier on every line, so this
  // runs thousands of times per render. Cache it on the tag, keyed by the
  // unit being read, which is the only input that varies between renders.
  const tu = currentTu();
  if (t._defFnTu === tu) return t._defFn;
  const c = defFns(t);
  const mine = f => tu && f.name.startsWith(tu + ':');
  const withObj = c.filter(f => f.object);
  const pick = c.length ? (withObj.find(mine) || withObj[0] || c.find(mine) || c[0]).id : null;
  t._defFnTu = tu;
  t._defFn = pick;
  return pick;
}

/// One identifier, as a jump link when the definition index knows it.
///
/// Three outcomes, which is the whole point of the index carrying both ids:
///   * an analysed function -> the function view, where the CFG is;
///   * a definition in a file the evidence carries -> that file at its line;
///   * a definition in a header the evidence never compiled -> named, but
///     NOT a link: there is no file view to open, and pretending otherwise
///     would send the reader nowhere.
function defLink(name) {
  const tags = S.defs.get(name);
  if (!tags || !tags.length) return esc(name);
  const t = pickDef(tags);
  const where = `${t.file}:${t.line}`;
  // one resolution behind both the title and the click, so they cannot disagree
  if (defFn(t) != null) return `<span class="def" data-def="${esc(name)}" title="${esc(t.kind)} · ${esc(where)} — go to function">${esc(name)}</span>`;
  if (t.fileId != null) return `<span class="def" data-def="${esc(name)}" title="${esc(t.kind)} · ${esc(where)} — go to definition">${esc(name)}</span>`;
  return `<span class="def off" title="${esc(t.kind)} · ${esc(where)} — defined in a header this evidence does not carry">${esc(name)}</span>`;
}

/// Which definition a name means here. A name can be defined more than once
/// -- a `static helper` per translation unit -- so prefer one in the file
/// being read before falling back to the first.
function pickDef(tags) {
  if (tags.length === 1) return tags[0];
  const here = S.fileId != null ? tags.find(t => t.fileId === S.fileId) : null;
  if (here) return here;
  return tags.find(t => defFn(t) != null) || tags[0];
}

/// Where a click on an identifier goes.
function gotoDef(name) {
  const tags = S.defs.get(name);
  if (!tags || !tags.length) return;
  const t = pickDef(tags);
  const id = defFn(t);
  if (id != null) navigate(`#/fn/${id}`);
  else if (t.fileId != null) navigate(`#/file/${t.fileId}/L${t.line}`);
}

/* cross-highlighting: source line <-> CFG blocks <-> object rows */
function hl(kind, id, on) {
  const c = $('#content');
  if (kind === 'line') {
    c.querySelectorAll(`.src tr[data-line="${id}"]`).forEach(e => e.classList.toggle('hl', on));
    c.querySelectorAll(`.obj tr.i[data-line="${id}"]`).forEach(e => e.classList.toggle('hl', on));
    // Both graphs, so the highlight is bidirectional wherever a graph is
    // showing: `.node` is a control-flow block, `.bf-node` a call-graph box.
    // The Calls tab already lit the source from a box; this is the way back.
    c.querySelectorAll(`.node, .bf-node`).forEach(n => { if ((n.dataset.lines || '').split(',').includes(String(id))) n.classList.toggle('hl', on); });
  } else if (kind === 'block') {
    const f = fn(S.fnId); const b = f.object.blocks[id];
    for (const l of b.lines) c.querySelectorAll(`.src tr[data-line="${l}"]`).forEach(e => e.classList.toggle('hl', on));
    c.querySelectorAll(`.obj tr.i[data-block="${id}"]`).forEach(e => e.classList.toggle('hl', on));
    // and the call-graph boxes standing for the block's lines, so a hover
    // anywhere lights the same code in every view that is showing
    const lines = new Set(b.lines.map(String));
    c.querySelectorAll(`.node, .bf-node`).forEach(nd => {
      if ((nd.dataset.lines || '').split(',').some(l => lines.has(l))) nd.classList.toggle('hl', on);
    });
  } else if (kind === 'var') {
    // A variable's declaration and its accesses (#53). Deliberately not
    // `.hl`: that class is a run of consecutive lines owned by markHlRuns(),
    // and these lines are neither consecutive nor interchangeable -- each
    // carries its own position in static instruction order, so each is
    // marked on its own.
    const v = varAccesses(fn(S.fnId), id);
    // Clearing first, unconditionally, is also what makes switching the
    // marks off take effect on whatever is already on screen: the next
    // paint decides nothing, so nothing survives it.
    clearVarMarks(c);
    if (v && varMarksWanted(on)) {
      for (const tr of c.querySelectorAll(`.src tr[data-line="${v.declLine}"]`)) {
        tr.classList.add('vdecl');
        markCode(tr, [h('span', { class: 'vb decl', title: `${v.name} is declared here (DW_AT_decl_line)` }, 'decl')]);
      }
      for (const [line, us] of v.byLine) {
        for (const tr of c.querySelectorAll(`.src tr[data-line="${line}"]`)) {
          tr.classList.add('vuse');
          // Every position on this line, not just one: L46 in the fixture is
          // both `acc`'s 2nd touch and its 3rd, and showing one of them
          // would misreport the sequence the reader is here to check.
          markCode(tr, us.map(u => h('span', { class: 'vb ' + ACCESS_CLS[u.access],
            title: `static access site ${u.order} of ${v.all.length}, in address order and not an execution order: `
              + `${v.name} is ${ACCESS_WORD[u.access] || u.access} here, at ${hex(u.addr)}` },
          h('i', {}, u.order), ACCESS_MARK[u.access] || '?')));
        }
      }
      // The object rows are deliberately not marked here. They and this
      // panel are the same left pane -- only one tab's renderer runs -- so
      // when a variable is being hovered there is no instruction listing on
      // screen to mark, and code that could never run would be a comment
      // claiming a behaviour the viewer does not have.
    }
  }
  markHlRuns(c);
}

/// Append a variable's order badges to a source row's code cell.
function markCode(tr, badges) {
  const code = tr.querySelector('.code');
  if (!code) return;
  let mk = code.querySelector('.vmk');
  if (!mk) { mk = h('span', { class: 'vmk' }); code.append(mk); }
  mk.append(...badges);
}

/// Take every variable mark off, wherever the pointer went.
///
/// Painting always clears first, so moving between two rows without an
/// intervening mouseleave -- which happens whenever the pointer crosses
/// directly from a panel row to one of its own access badges -- cannot
/// leave the previous variable's badges behind next to the new one's.
function clearVarMarks(c) {
  c.querySelectorAll('.src tr.vdecl, .src tr.vuse').forEach(e => e.classList.remove('vdecl', 'vuse'));
  c.querySelectorAll('.src .vmk').forEach(e => e.remove());
}

/// Mark where each highlighted run starts and ends.
///
/// A block usually covers several consecutive source lines, and outlining
/// every row drew a stack of separate boxes with rules between them -- five
/// lines, five rectangles. The run is one region of code, so it should read
/// as one: the sides go on every row, the top only on the first and the
/// bottom only on the last. The CSS does the drawing; this only says which
/// row is which.
///
/// A run is broken by any *source* line that is not highlighted. An
/// annotation row (a branch chip, an MC/DC pill) carries no line of its own,
/// so one sitting between two highlighted lines is inside the run and joins
/// it -- otherwise the box would have a hole where the annotation is.
function markHlRuns(c) {
  for (const tbl of c.querySelectorAll('table.src')) {
    const rows = [...tbl.rows];
    rows.forEach(r => r.classList.remove('hl-top', 'hl-bot', 'hl-in'));
    const lit = rows.map(r => r.classList.contains('hl'));
    const inRun = lit.slice();
    for (let i = 0; i < rows.length; i++) {
      if (inRun[i] || rows[i].hasAttribute('data-line')) continue;
      // a row with no line of its own: part of the run when the run continues
      // past it. Scanning left to right means inRun[i-1] is already settled.
      let j = i;
      while (j < rows.length && !lit[j] && !rows[j].hasAttribute('data-line')) j++;
      if (i > 0 && inRun[i - 1] && j < rows.length && lit[j]) inRun[i] = true;
    }
    for (let i = 0; i < rows.length; i++) {
      if (!inRun[i]) continue;
      if (!lit[i]) rows[i].classList.add('hl-in');
      if (!inRun[i - 1]) rows[i].classList.add('hl-top');
      if (!inRun[i + 1]) rows[i].classList.add('hl-bot');
    }
  }
}
function selectLine(ln) {
  const c = $('#content');
  c.querySelectorAll('.src tr.sel').forEach(e => e.classList.remove('sel'));
  c.querySelectorAll(`.src tr[data-line="${ln}"]`).forEach(e => e.classList.add('sel'));
  const row = c.querySelector(`.obj tr.i[data-line="${ln}"]`); if (row) row.scrollIntoView({ block: 'center' });
  const f = fn(S.fnId);
  if (f.object) { const b = f.object.blocks.find(b => b.lines.includes(ln)); if (b) selectBlock(b.id, false); }
}
function selectBlock(id, scrollSource = true) {
  S.selBlock = id;
  const c = $('#content');
  c.querySelectorAll('.node.sel').forEach(n => n.classList.remove('sel'));
  const n = c.querySelector(`.node[data-id="${id}"]`); if (n) n.classList.add('sel');
  if (scrollSource) {
    const f = fn(S.fnId); const b = f.object.blocks[id];
    c.querySelectorAll('.src tr.sel').forEach(e => e.classList.remove('sel'));
    for (const l of b.lines) c.querySelectorAll(`.src tr[data-line="${l}"]`).forEach(e => e.classList.add('sel'));
    const first = c.querySelector(`.src tr[data-line="${Math.min(...b.lines)}"]`); if (first) first.scrollIntoView({ block: 'center' });
    const row = c.querySelector(`.obj tr.blk[data-block="${id}"]`); if (row) row.scrollIntoView({ block: 'start' });
  }
}

/* ---------------- flow (CFG) ---------------- */
/* Layered (Sugiyama-style) layout: longest-path layering, dummy nodes for
 * edges spanning several layers, barycenter ordering, iterative median
 * x-placement, orthogonal edge routing with rounded corners, back edges in
 * lanes on the right. Deterministic: same input -> same picture. */
function layoutCfg(blocks, dims, opts) {
  const n = blocks.length;
  const { gapX, gapY, dummyW } = opts;
  const succ = blocks.map(b => b.succs.map(e => e.to));
  // back edges via DFS
  const back = new Set(); const state = new Array(n).fill(0);
  const dfs = u => { state[u] = 1; for (const v of succ[u]) { if (state[v] === 1) back.add(`${u}-${v}`); else if (state[v] === 0) dfs(v); } state[u] = 2; };
  dfs(0); for (let i = 0; i < n; i++) if (state[i] === 0) dfs(i);
  const fwd = blocks.map((b, u) => succ[u].filter(v => !back.has(`${u}-${v}`) && v !== u));
  // longest-path layering
  const layer = new Array(n).fill(0);
  const indeg = new Array(n).fill(0); fwd.forEach(l => l.forEach(v => indeg[v]++));
  const q = []; for (let i = 0; i < n; i++) if (indeg[i] === 0) q.push(i);
  const seen = new Set();
  while (q.length) { const u = q.shift(); seen.add(u); for (const v of fwd[u]) { layer[v] = Math.max(layer[v], layer[u] + 1); if (--indeg[v] === 0) q.push(v); } }
  // nodes: real 0..n-1, dummies n..; each has layer, w, h
  const N = []; blocks.forEach((b, i) => N.push({ id: i, real: true, layer: layer[i], w: dims[i].w, h: dims[i].h, up: [], down: [] }));
  const edges = []; // {u, v, kind, label, back, chain:[node ids from u to v]}
  blocks.forEach((b, u) => b.succs.forEach(e => {
    const v = e.to; const isBack = back.has(`${u}-${v}`) || v === u;
    const rec = { u, v, kind: e.kind, label: e.label, back: isBack, self: v === u, chain: [u], e };
    if (!isBack) {
      let prev = u;
      for (let L = layer[u] + 1; L < layer[v]; L++) { const d = { id: N.length, real: false, layer: L, w: dummyW, h: 0, up: [], down: [] }; N.push(d); N[prev].down.push(d.id); d.up.push(prev); rec.chain.push(d.id); prev = d.id; }
      N[prev].down.push(v); N[v].up.push(prev);
    }
    rec.chain.push(v); edges.push(rec);
  }));
  const L = Math.max(...N.map(x => x.layer)) + 1;
  const layers = Array.from({ length: L }, () => []);
  N.forEach(x => layers[x.layer].push(x.id));
  // barycenter ordering
  const pos = new Array(N.length).fill(0);
  const setPos = () => layers.forEach(l => l.forEach((v, i) => pos[v] = i));
  setPos();
  const bary = (ids, fallback) => ids.length ? ids.reduce((a, v) => a + pos[v], 0) / ids.length : fallback;
  for (let it = 0; it < 8; it++) {
    for (let li = 1; li < L; li++) { layers[li].sort((a, b) => bary(N[a].up, pos[a]) - bary(N[b].up, pos[b]) || a - b); setPos(); }
    for (let li = L - 2; li >= 0; li--) { layers[li].sort((a, b) => bary(N[a].down, pos[a]) - bary(N[b].down, pos[b]) || a - b); setPos(); }
  }
  // initial x: pack left-to-right
  const x = new Array(N.length).fill(0);
  layers.forEach(l => { let cx = 0; l.forEach(v => { x[v] = cx; cx += N[v].w + gapX; }); });
  const center = v => x[v] + N[v].w / 2;
  // iterative median placement with overlap resolution
  const place = (li, useUp, useDown) => {
    const l = layers[li];
    const want = l.map(v => { const nb = [...(useUp ? N[v].up : []), ...(useDown ? N[v].down : [])]; if (!nb.length) return center(v); const cs = nb.map(center).sort((a, b) => a - b); return cs[Math.floor((cs.length - 1) / 2)] * 0.5 + cs[Math.ceil((cs.length - 1) / 2)] * 0.5; });
    // left-to-right: honour min spacing
    let prevRight = -Infinity;
    l.forEach((v, i) => { x[v] = Math.max(want[i] - N[v].w / 2, prevRight + gapX); prevRight = x[v] + N[v].w; });
    // right-to-left: pull back nodes pushed past their wish when there is room
    let nextLeft = Infinity;
    for (let i = l.length - 1; i >= 0; i--) { const v = l[i]; const wish = want[i] - N[v].w / 2; if (x[v] > wish) x[v] = Math.max(wish, Math.min(x[v], nextLeft - gapX - N[v].w)); nextLeft = x[v]; }
  };
  for (let it = 0; it < 6; it++) {
    for (let li = 1; li < L; li++) place(li, true, false);
    for (let li = L - 2; li >= 0; li--) place(li, false, true);
    for (let li = 0; li < L; li++) place(li, true, true);
  }
  // normalise: shift so min x = 0
  const minX = Math.min(...x); N.forEach(v => x[v.id] -= minX);
  const maxRight = Math.max(...N.map(v => x[v.id] + v.w));
  // y per layer
  const layerH = layers.map(l => Math.max(...l.map(v => N[v].h)));
  const y = []; let cy = 0;
  for (let li = 0; li < L; li++) { y[li] = cy; cy += layerH[li] + gapY; }
  const totalH = cy - gapY;
  N.forEach(v => { v.x = x[v.id]; v.y = y[v.layer] + (v.real ? 0 : layerH[v.layer] / 2); });
  return { N, edges, layers, layerH, layerY: y, totalW: maxRight, totalH, back };
}

/* axis-aligned polyline -> path with rounded corners */
function roundedPath(pts, r = 8) {
  if (pts.length < 2) return '';
  let d = `M${pts[0][0]} ${pts[0][1]}`;
  for (let i = 1; i < pts.length - 1; i++) {
    const [px, py] = pts[i - 1], [cx, cy] = pts[i], [nx, ny] = pts[i + 1];
    const l1 = Math.hypot(cx - px, cy - py), l2 = Math.hypot(nx - cx, ny - cy);
    const rr = Math.min(r, l1 / 2, l2 / 2);
    if (rr < 1) { d += ` L${cx} ${cy}`; continue; }
    const ax = cx - (cx - px) / l1 * rr, ay = cy - (cy - py) / l1 * rr;
    const bx = cx + (nx - cx) / l2 * rr, by = cy + (ny - cy) / l2 * rr;
    d += ` L${ax} ${ay} Q${cx} ${cy} ${bx} ${by}`;
  }
  const [ex, ey] = pts[pts.length - 1];
  return d + ` L${ex} ${ey}`;
}

function renderFlow(body, f) {
  const blocks = f.object.blocks;
  const fl = file(f.fileId);
  const W = S.showInstr ? 250 : 230, PAD = 28;
  // A block's source lines in the order its instructions execute, not sorted:
  // the line of the block's terminating branch (usually the `if` or the `}`
  // the jump belongs to) would otherwise sort to the front and make every
  // block look like the condition. That trailing line is dimmed and marked.
  const blockLines = b => {
    const n = b.instrs.length;
    const last = b.instrs[n - 1];
    const termIsBranch = last && last.flags.some(f => ['branch', 'condBranch', 'indirectBranch', 'return'].includes(f));
    const real = new Set(), order = [];
    b.instrs.forEach((i, k) => {
      if (!i.line) return;
      if (!order.includes(i.line)) order.push(i.line);
      if (!(termIsBranch && k === n - 1)) real.add(i.line);
    });
    return order.map(line => ({ line, term: !real.has(line) }));
  };
  // Which source condition is each conditional block testing? A decision like
  //   if ((gear_down && speed_ok) || wow)
  // compiles to one conditional block per condition, and llvm-cov emits one
  // branch region (with columns) per condition. When the two counts agree the
  // blocks can be named after the condition they test, instead of every block
  // repeating the whole `if` line -- which is what made the graph unreadable.
  // Only an exact count match is attributed; anything else falls back to the
  // plain line text rather than guessing.
  const condOf = (() => {
    const m = new Map(), byLine = new Map();
    blocks.forEach((b, i) => {
      const last = b.instrs[b.instrs.length - 1];
      if (!last || !last.flags.includes('condBranch') || !last.line) return;
      if (!byLine.has(last.line)) byLine.set(last.line, []);
      byLine.get(last.line).push(i);
    });
    for (const [line, idxs] of byLine) {
      const brs = (f.branches || []).filter(x => x.line === line).sort((a, b) => a.col - b.col);
      if (brs.length !== idxs.length) continue;
      idxs.forEach((bi, k) => m.set(bi, { br: brs[k], k, n: brs.length, line }));
    }
    return m;
  })();
  const srcSpan = br => (br.line === br.lineEnd
    ? (fl.text[br.line - 1] || '').slice(br.col - 1, br.colEnd - 1)
    : (fl.text[br.line - 1] || '').slice(br.col - 1) + ' …').trim();
  // A block that is nothing but an unconditional jump: control-flow plumbing
  // the compiler emits for short-circuit paths. Real, so it stays in the
  // graph and keeps its number, but drawn as a connector so the eye skips it.
  const isHop = b => b.instrs.length === 1 && b.instrs[0].flags.includes('branch') && !b.instrs[0].flags.includes('condBranch');
  const branchText = b => {
    const last = b.instrs[b.instrs.length - 1];
    if (!last || !last.flags.includes('condBranch')) return null;
    const prev = b.instrs[b.instrs.length - 2];
    const cmp = prev && /^(cmp|cmn|tst|ccmp|test|subs|adds|ands|fcmp)/.test(prev.mnemonic) ? `${prev.mnemonic} ${prev.operands} · ` : '';
    return `${cmp}${last.mnemonic} ${last.operands.replace(/0x[0-9a-f]+\s*<[^>]*>|0x[0-9a-f]+/g, '').replace(/,\s*$/, '').trim()}`.trim();
  };
  // Lines to print in a node. When the block ends in a conditional branch and
  // a ⑂ row is drawn for it, that row already speaks for the branch's line --
  // and the line number is in the header -- so printing the line text again is
  // pure repetition. It was why a switch showed `switch (phase) {` in ten
  // consecutive blocks, and a short-circuit `if` in six.
  const shownLines = (b, i) => {
    const ol = blockLines(b), last = b.instrs[b.instrs.length - 1];
    const drop = last && last.flags.includes('condBranch') && branchText(b) ? last.line : null;
    return drop ? ol.filter(x => x.line !== drop) : ol;
  };
  const dims = blocks.map((b, i) => {
    if (!S.showInstr && isHop(b)) return { w: W, h: 26 };
    const rows = S.showInstr ? Math.min(b.instrs.length, 10) : Math.min(shownLines(b, i).length, 5) + Math.min((b.inferredLines || []).length, 3);
    return { w: W, h: 30 + Math.max(1, rows) * 15 + 8 + (!S.showInstr && branchText(b) ? 15 : 0) };
  });
  const lay = layoutCfg(blocks, dims, { gapX: 36, gapY: 64, dummyW: 14 });
  const { N, edges, layerY, layerH } = lay;
  const backEdges = edges.filter(e => e.back && !e.self);
  const laneW = 16;
  const totalW = lay.totalW + PAD * 2 + (backEdges.length ? 24 + backEdges.length * laneW : 24);
  const totalH = lay.totalH + PAD * 2;
  if (S.zoom === 'fit') S.zoom = Math.max(0.5, Math.min(1, (body.clientWidth - 8) / totalW));
  const svg = h('svg:svg', { width: totalW * S.zoom, height: totalH * S.zoom, viewBox: `0 0 ${totalW * S.zoom} ${totalH * S.zoom}` });
  const defs = h('svg:defs');
  for (const c of ['cov', 'unc', 'unk', 'jus', 'tr', 'fl']) defs.append(h('svg:marker', { id: 'arr-' + c, viewBox: '0 0 10 10', refX: 9, refY: 5, markerWidth: 7, markerHeight: 7, orient: 'auto' }, h('svg:path', { d: 'M0 0L10 5L0 10z', fill: `var(--${c === 'tr' ? 'accent' : c === 'fl' ? 'jus' : c})` })));
  svg.append(defs);
  const g = h('svg:g', { transform: `scale(${S.zoom})` });
  svg.append(g);
  const nx = v => N[v].x + PAD, ny = v => N[v].y + PAD;

  // ports: outgoing at 30% / 70% of the width for conditional branches, centre otherwise;
  // incoming spread across the target's top edge in order of source x.
  const inc = new Map();
  for (const e of edges) { if (!inc.has(e.v)) inc.set(e.v, []); inc.get(e.v).push(e); }
  for (const [v, list] of inc) {
    list.sort((a, b) => (a.back ? 1e9 : nx(a.chain[a.chain.length - 2] ?? a.u)) - (b.back ? 1e9 : nx(b.chain[b.chain.length - 2] ?? b.u)));
    const k = list.length; list.forEach((e, i) => { e.inX = nx(v) + N[v].w * (i + 1) / (k + 1); });
  }
  // exit ports: for a conditional block the edge heading further left leaves
  // from the left port, so the two exits do not cross under the block.
  const nextX = e => e.self ? nx(e.u) + N[e.u].w : e.back ? Infinity : (e.chain.length > 2 ? N[e.chain[1]].x + PAD + N[e.chain[1]].w / 2 : e.inX);
  const portOf = new Map();
  for (const u of blocks.keys()) {
    const outs = edges.filter(e => e.u === u && !e.self);
    if (outs.length === 2 && blocks[u].succs.some(s => s.kind === 'taken')) {
      const [a, b] = outs; const aLeft = nextX(a) <= nextX(b);
      portOf.set(a, aLeft ? 0.3 : 0.7); portOf.set(b, aLeft ? 0.7 : 0.3);
    } else outs.forEach(e => portOf.set(e, 0.5));
  }
  const outX = (u, e) => nx(u) + N[u].w * (portOf.get(e) ?? 0.5);
  const edgeCls = e => { const st = edgeStatus(f, blocks[e.u], e.e); return plainMode() ? 'plain' : (st === 'jus' ? 'jus' : st); };
  const dirOf = e => { const cond = blocks[e.u].succs.length === 2 && blocks[e.u].succs.some(s => s.kind === 'taken'); return cond ? (e.kind === 'taken' ? 'tr' : 'fl') : ''; };
  const labelPill = (txt, x, y, cls, dir, anchor) => {
    const w = txt.length * 6.3 + 10;
    const lx = anchor === 'end' ? x - w : anchor === 'middle' ? x - w / 2 : x;
    const grp = h('svg:g', { class: 'elabel' });
    grp.append(h('svg:rect', { x: lx, y: y - 10, width: w, height: 15, rx: 4, class: 'elabel-bg' }));
    grp.append(h('svg:text', { class: `edge-label ${cls} ${dir}`, x: lx + 5, y: y + 1 }, txt));
    return grp;
  };

  // edges
  // horizontal routing rows in the gap below each layer: one row per edge so
  // parallel segments never overlap
  const rowsUsed = new Array(layerY.length).fill(0);
  const gapMid = li => { const k = rowsUsed[li]++; return layerY[li] + layerH[li] + PAD + 18 + (k % 6) * 7; };
  let lane = 0;
  for (const e of edges) {
    const cls = edgeCls(e), dir = dirOf(e);
    const marker = cls === 'plain' ? (dir || 'unk') : cls;
    let pts;
    if (e.self) {
      const u = e.u; const x1 = nx(u) + N[u].w, y0 = ny(u) + 14, y1 = ny(u) + N[u].h - 12;
      pts = [[x1, y1], [x1 + 22, y1], [x1 + 22, y0], [x1 + 1, y0]];
    } else if (e.back) {
      const u = e.u, v = e.v; const lx = PAD + lay.totalW + 24 + lane * laneW; lane++;
      const sx = outX(u, e), sy = ny(u) + N[u].h;
      const ty = ny(v) - 14 - (lane % 3) * 4;
      pts = [[sx, sy], [sx, sy + 14], [lx, sy + 14], [lx, ty], [e.inX, ty], [e.inX, ny(v)]];
    } else {
      pts = [];
      const u = e.u; let cx = outX(u, e); pts.push([cx, ny(u) + N[u].h]);
      // walk the chain: after each hop, route horizontally in the gap below the current layer
      for (let i = 1; i < e.chain.length; i++) {
        const prev = e.chain[i - 1], cur = e.chain[i];
        const targetX = i === e.chain.length - 1 ? e.inX : N[cur].x + PAD + N[cur].w / 2;
        const rowY = gapMid(N[prev].layer);
        if (Math.abs(targetX - cx) > 0.5) { pts.push([cx, rowY]); pts.push([targetX, rowY]); cx = targetX; }
        if (i === e.chain.length - 1) pts.push([cx, ny(cur)]);
      }
    }
    g.append(h('svg:path', { class: `edge ${cls} ${dir}${e.back ? ' back' : ''}`, d: roundedPath(pts), 'marker-end': `url(#arr-${marker})` }));
    const cond = dir !== '';
    if (cond || e.back) {
      const cnt = measuredMode() && e.e.measured != null ? ` ×${e.e.measured}` : '';
      const txt = (cond ? (e.label || (e.kind === 'taken' ? 'taken' : 'not taken')) + (e.back ? ' ↺' : '') : '↺') + cnt;
      if (e.self) g.append(labelPill(txt, pts[1][0] + 4, (pts[0][1] + pts[2][1]) / 2, cls, dir, 'start'));
      else if (e.back) g.append(labelPill(txt, pts[2][0] - 4, pts[1][1] + 1, cls, dir, 'end'));
      else { const left = (portOf.get(e) ?? 0.5) < 0.5; g.append(labelPill(txt, pts[0][0] + (left ? -4 : 4), pts[0][1] + 16, cls, dir, left ? 'end' : 'start')); }
    }
  }
  // nodes
  blocks.forEach((b, i) => {
    const p = { x: nx(i), y: ny(i), w: N[i].w, h: N[i].h }; const st = plainMode() ? 'plain' : blockStatus(f, b);
    const node = h('svg:g', { class: `node ${st}${S.selBlock === i ? ' sel' : ''}`, 'data-id': i, 'data-lines': b.lines.join(','), transform: `translate(${p.x},${p.y})`,
      onclick: () => selectBlock(i), onmouseenter: () => hl('block', i, true), onmouseleave: () => hl('block', i, false) });
    node.append(h('svg:rect', { class: 'box', width: p.w, height: p.h }));
    if (b.note || b.diverges) node.append(h('svg:title', {}, [b.note, b.diverges].filter(Boolean).join('\n')));
    node.append(h('svg:rect', { class: 'hdr', x: 1, y: 1, width: p.w - 2, height: 21, rx: 6 }));
    node.append(h('svg:text', { class: 'hd', x: 8, y: 15 }, `B${i} · ${hex(b.address)}`));
    if (b.note) node.append(h('svg:text', { class: 'kind', x: p.w - (b.kind !== 'normal' ? 44 : 8), y: 15, 'text-anchor': 'end', fill: 'var(--jus)' }, 'INFERRED'));
    if ((f.object.unreachableBlocks || []).includes(i)) node.append(h('svg:text', { class: 'kind', x: p.w - (b.kind !== 'normal' ? 44 : 8), y: 15, 'text-anchor': 'end', fill: 'var(--unc)' }, 'UNREACHABLE'));
    if (b.kind !== 'normal') node.append(h('svg:text', { class: 'kind', x: p.w - 8, y: 15, 'text-anchor': 'end' }, b.kind.toUpperCase()));
    else if (b.lines.length) { const lo = Math.min(...b.lines), hi = Math.max(...b.lines); node.append(h('svg:text', { class: 'hd', x: p.w - 8, y: 15, 'text-anchor': 'end' }, lo === hi ? `L${lo}` : `L${lo}–${hi}`)); }
    let yy = 36;
    if (!S.showInstr && isHop(b)) {
      node.classList.add('hop');
      node.append(h('svg:title', {}, `jump only: ${b.instrs[0].mnemonic} ${b.instrs[0].operands}\nThe compiler emits these to join short-circuit paths. It executes, but decides nothing.`));
    } else if (S.showInstr) {
      for (const ins of b.instrs.slice(0, 10)) { node.append(h('svg:text', { x: 8, y: yy, fill: instrStatus(f, ins) === 'unc' ? 'var(--unc)' : null }, trunc(`${ins.mnemonic} ${ins.operands}`, 34))); yy += 15; }
      if (b.instrs.length > 10) node.append(h('svg:text', { class: 'dim', x: 8, y: yy }, `… +${b.instrs.length - 10} more`));
    } else if (b.lines.length || (b.inferredLines || []).length) {
      for (const l of (b.inferredLines || []).slice(0, 3)) { node.append(h('svg:text', { class: 'dim', x: 8, y: yy, fill: lineStatus(f.fileId, l) === 'unc' ? 'var(--unc)' : null }, trunc(`≈${l}  ${(fl.text[l - 1] || '').trim()}`, 34))); yy += 15; }
      const ol = shownLines(b, i);
      for (const { line: l, term } of ol.slice(0, 5)) { node.append(h('svg:text', { class: term ? 'dim' : '', x: 8, y: yy, fill: !term && lineStatus(f.fileId, l) === 'unc' ? 'var(--unc)' : null }, trunc(`${term ? '↳' : ' '}${l}  ${(fl.text[l - 1] || '').trim()}`, 34))); yy += 15; }
      if (ol.length > 5) { node.append(h('svg:text', { class: 'dim', x: 8, y: yy }, `… +${ol.length - 5} lines`)); yy += 15; }
      const bt = branchText(b), cnd = condOf.get(i);
      if (cnd) {
        // what this block decides, in source terms, then how it does it
        const nth = cnd.n > 1 ? `  ${cnd.k + 1}/${cnd.n}` : '';
        const t = h('svg:text', { class: 'br', x: 8, y: yy }, '⑂ ');
        t.append(h('svg:tspan', { class: 'cnd' }, trunc(srcSpan(cnd.br), 22)));
        if (nth) t.append(h('svg:tspan', { class: 'dim' }, nth));
        if (bt) t.append(h('svg:tspan', { class: 'dim' }, '  · ' + trunc(bt, 18)));
        t.append(h('svg:title', {}, `condition ${cnd.k + 1} of ${cnd.n} on line ${cnd.line}: ${srcSpan(cnd.br)}\nmachine test: ${bt || '—'}\ntrue ${cnd.br.trueCount}× · false ${cnd.br.falseCount}× in the instrumented run`));
        node.append(t);
      } else if (bt) node.append(h('svg:text', { class: 'br', x: 8, y: yy }, '⑂ ' + trunc(bt, 34)));
      const breq = [...new Set(b.lines.flatMap(l => (f.lineRequirements || {})[l] || []))];
      if (breq.length) node.append(h('svg:text', { class: 'kind', x: p.w - 8, y: p.h - 6, 'text-anchor': 'end', fill: 'var(--accent)' }, trunc(breq.join(' '), 28)));
    } else node.append(h('svg:text', { class: 'dim', x: 8, y: yy }, `${b.instrs.length} instr · no source line`));
    g.append(node);
  });
  const wrap = h('div', { class: 'graph-wrap' }, svg);
  enablePan(wrap);
  setTimeout(() => { wrap.scrollLeft = Math.max(0, nx(0) * S.zoom - wrap.clientWidth / 2 + (N[0].w * S.zoom) / 2); }, 0);
  body.append(h('div', { class: 'graph-bar' },
    h('div', { class: 'gtools' },
      h('button', { class: 'ghost', title: 'zoom in', onclick: () => { S.zoom = Math.min(2, S.zoom * 1.2); rerenderLeft(); } }, '+'),
      h('button', { class: 'ghost', title: 'zoom out', onclick: () => { S.zoom = Math.max(0.4, S.zoom / 1.2); rerenderLeft(); } }, '−'),
      h('button', { class: 'ghost', title: 'actual size', onclick: () => { S.zoom = 1; rerenderLeft(); } }, '1:1'),
      h('button', { class: 'ghost', title: 'fit width', onclick: () => { S.zoom = 'fit'; rerenderLeft(); } }, '⤢'),
      h('span', { class: 'sep' }),
      h('button', { class: 'ghost', style: 'width:auto;padding:0 8px', title: 'show instructions instead of source lines', onclick: () => { S.showInstr = !S.showInstr; rerenderLeft(); } }, S.showInstr ? 'source' : 'asm')),
    h('div', { class: 'legend' }, h('span', {}, h('i', { style: 'background:var(--accent)' }), 'condition holds'), h('span', {}, h('i', { style: 'background:var(--jus)' }), 'condition fails (fall-through)'), plainMode() ? h('span', { class: 'muted', title: '⑂ names the source condition a block tests, then the machine test it uses. Dashed blocks contain only a jump. Edge labels are the machine condition, which is often the inverse of the source `if`.' }, '· ⑂ = condition tested · dashed = jump only · edge labels are the machine condition') : [h('span', {}, h('i', { style: 'background:var(--cov)' }), measuredMode() ? 'executed' : 'projected covered'), h('span', {}, h('i', { style: 'background:var(--unc)' }), measuredMode() ? 'not executed' : 'uncovered'), h('span', {}, h('i', { style: 'background:var(--jus)' }), 'justified'), h('span', {}, h('i', { style: 'background:var(--unk)' }), 'unknown')])), wrap);
}
const trunc = (s, n) => s.length > n ? s.slice(0, n - 1) + '…' : s;
function rerenderLeft() { const pane = $('#content .pane'); pane.innerHTML = ''; renderLeft(pane, fn(S.fnId)); }
// One pair of window listeners for the app's whole lifetime, not one per
// graph render (#72). Every zoom, fit-width and tab switch used to call
// enablePan again on a fresh wrapper without ever removing the previous
// pair, so each redraw left its mousemove/mouseup handlers on window
// forever -- still cheap to run individually, but retaining the detached
// graph and everything under it, and accumulating without bound over a long
// review session. A single mutable "who is being dragged right now" makes
// a second registration unnecessary: enablePan only ever adds one
// mousedown listener to its own wrapper, which is freed exactly when the
// wrapper itself is, the same as any other DOM listener.
let panState = null;
// Guarded, not assumed: this file also loads inside the node --test vm
// harness, which has no `window` at module scope (only inside a function
// the tests never call). A single top-level ReferenceError there fails
// every test in the file before any of them runs.
if (typeof window !== 'undefined') {
  window.addEventListener('mousemove', e => {
    if (!panState) return;
    const { wrap, x, y, sl, st } = panState;
    wrap.scrollLeft = sl - (e.clientX - x);
    wrap.scrollTop = st - (e.clientY - y);
  });
  window.addEventListener('mouseup', () => {
    // Setting properties on an already-detached wrapper (the graph was
    // replaced mid-drag) is a harmless no-op, not an error -- this is what
    // "terminate a drag safely if its graph is removed" comes down to:
    // there is nothing to clean up beyond dropping the reference.
    if (panState) panState.wrap.classList.remove('dragging');
    panState = null;
  });
}
function enablePan(wrap) {
  wrap.addEventListener('mousedown', e => {
    if (e.target.closest('.node, .bf-node')) return;
    panState = { wrap, x: e.clientX, y: e.clientY, sl: wrap.scrollLeft, st: wrap.scrollTop };
    wrap.classList.add('dragging');
  });
}

/* ---------------- changes: the commits and tickets that touched this function ---------------- */
// A banner when the history in the evidence no longer matches the repository
// or the files on disk, with the exact command that brings it up to date.
/* Whether the change history is still current. It used to be fetched once
   with every failure swallowed, so a 503 from a busy server left S.fresh
   empty and the stale-history warning simply did not appear -- the unsafe
   direction for evidence (review of #67). A refusal for load, or a failed
   connection, is retried after the Retry-After it names, up to `tries`
   attempts in all; if there is still no answer, the page says freshness is
   unknown rather than saying nothing. */
async function loadFreshness(name, gen, tries = 3) {
  for (let attempt = 1; ; attempt++) {
    let r = null;
    try { r = await authFetch('/api/freshness' + vqOf(name)); } catch (e) { r = null; }
    if (gen !== variantGen) return;
    if (r && r.ok) {
      let f = null;
      try { f = await r.json(); } catch (e) { f = null; }
      if (gen !== variantGen) return;
      if (f) {
        S.fresh = f;
        if (f.stale) { renderTop(); if (S.fnId == null || S.tab === 'changes') route(); }
        return;
      }
    }
    if (r && r.status === 401) return; // the token page has taken over
    const retryable = !r || r.status === 503;
    if (!retryable || attempt >= tries) break;
    const secs = r ? parseInt(r.headers.get('retry-after'), 10) : NaN;
    await new Promise(res => setTimeout(res, Math.min(30, Number.isFinite(secs) && secs >= 0 ? secs : 2) * 1000));
    if (gen !== variantGen) return;
  }
  S.fresh = { unknown: true, reasons: ['butterfly could not check whether the change history is still current: the server was busy or the check failed. Reload to try again.'] };
  renderTop();
  if (S.fnId == null || S.tab === 'changes') route();
}

function freshnessBanner(container) {
  const f = S.fresh;
  if (f && f.unknown) {
    container.append(h('div', { class: 'note', style: 'color:var(--unc);border-left:3px solid var(--unc);padding-left:10px;margin:6px 0 10px' },
      h('b', {}, 'Change history freshness is unknown. '), f.reasons.join(' ')));
    return;
  }
  if (!f || !f.stale) return;
  const heading = !S.ev.changes && f.requirementsStale && f.requirementsStale.length ? 'Requirements are stale. ' : 'Change history is stale. ';
  container.append(h('div', { class: 'note', style: 'color:var(--unc);border-left:3px solid var(--unc);padding-left:10px;margin:6px 0 10px' },
    h('b', {}, heading), f.reasons.join(' '), ' ',
    f.rerun ? h('span', {}, 'Re-run: ', h('code', {}, f.rerun), ' then reload.') : null));
}

function renderChanges(body, f) {
  const c = S.ev.changes;
  const wrap = h('div', { class: 'mcdc' });
  if (!c) {
    wrap.append(h('div', { class: 'empty', style: 'text-align:left;max-width:56em' }, 'No change history in this evidence. Run ', h('code', {}, `butterfly changes <evidence.json> --since <certified baseline>`), ' and reload: every function then lists the commits and tickets that touched it, and what must be re-verified.'));
    body.append(wrap); return;
  }
  freshnessBanner(wrap);
  const ids = f.commits || [];
  const origin = f.origin ? c.commits.find(x => x.sha === f.origin) : null;
  wrap.append(h('div', { class: 'sub-h' }, `${ids.length} commit${ids.length === 1 ? '' : 's'} touched this function${c.since ? ' since ' + c.since : ' (whole history)'}` +
    (f.origin ? ` · introduced in ${f.origin.slice(0, 7)}${origin ? ' (' + origin.date.slice(0, 10) + ')' : c.since ? ' (before ' + c.since + ')' : ''}` : '')));
  if (!ids.length) {
    wrap.append(h('div', { class: 'muted', style: 'padding:8px 0 14px' }, c.since ? `Not touched between ${c.since} and HEAD.` : 'No commit touched these lines.'));
  } else {
    const t = h('table', { class: 'tt' }, h('tr', {}, h('th', {}, 'date'), h('th', {}, 'ticket'), h('th', {}, 'commit'), h('th', {}, 'author'), h('th', {}, 'subject'), h('th', {}, 'this function')));
    for (const i of ids) {
      const cm = c.commits[i];
      const role = f.origin === cm.sha ? 'added' : 'modified';
      t.append(h('tr', {},
        h('td', { class: 'muted', style: 'white-space:nowrap' }, cm.date.slice(0, 10)),
        h('td', {}, cm.tickets.length ? cm.tickets.map(ti => h('b', { style: 'margin-right:6px' }, c.tickets[ti].id)) : h('span', { class: 'bad', title: 'change outside change control' }, 'no ticket')),
        h('td', { class: 'muted', style: 'white-space:nowrap', title: cm.sha }, cm.short.slice(0, 7)),
        h('td', { class: 'muted' }, cm.author),
        h('td', {}, cm.subject),
        h('td', { class: role === 'added' ? 'ok' : '' }, role)));
    }
    wrap.append(t);
  }
  // what the changes oblige for this function
  const im = c.impact;
  if (!im) {
    wrap.append(h('div', { class: 'note', style: 'margin-top:10px' }, 'Obligations not computed: run ', h('code', {}, 'butterfly impact <evidence.json> --write'), ' and reload.'));
  } else {
    const mine = im.obligations.filter(o => o.functionId === f.id);
    wrap.append(h('div', { class: 'sub-h', style: 'margin-top:12px' }, `Re-verification obligations for this function (${mine.length})`));
    if (!mine.length) wrap.append(h('div', { class: 'muted', style: 'padding:6px 0' }, ids.length ? 'None: covered, tagged, ticketed, nothing stale.' : 'None: not changed in the reported range.'));
    else {
      const ot = h('table', { class: 'tt' }, h('tr', {}, h('th', {}, 'severity'), h('th', {}, 'kind'), h('th', {}, 'target / key / requirement'), h('th', {}, 'detail')));
      for (const ob of mine) {
        const tgt = ob.targetFunction != null ? fn(ob.targetFunction) : null;
        ot.append(h('tr', {},
          h('td', {}, h('span', { class: 'k ' + (ob.severity === 'blocker' ? 'bad' : ob.severity === 'major' ? 'jus' : 'muted'), style: 'font-weight:600' }, ob.severity)),
          h('td', {}, h('code', {}, ob.kind)),
          h('td', {}, tgt ? h('a', { class: 'lnk', onclick: () => navigate(`#/fn/${tgt.id}`) }, tgt.name) : ob.requirement ? h('span', { class: 'req' }, ob.requirement) : h('code', {}, ob.key || ob.file || '')),
          h('td', { class: 'muted', style: 'font-size:11px' }, ob.detail)));
      }
      wrap.append(ot);
    }
  }
  body.append(wrap);
}

/* ---------------- calls (callers & callees) ---------------- */
// ---- the expandable call graph (#100) ----

// How certain a path is. Weakest wins: a chain through two call
// instructions and one address-formed candidate establishes a hypothesis,
// not a reachability fact, and the reader has to be able to see which hop
// made it one (#100).
const CONF_RANK = { fact: 3, run: 2, hypothesis: 1 };
const weaker = (a, b) => (CONF_RANK[a] <= CONF_RANK[b] ? a : b);

/// The neighbours of one function in one direction, already classified by
/// what the evidence actually establishes about each edge.
///
/// Downstream edges come from the disassembly: `callees` is one entry per
/// call instruction target. Inlined and folded entries are facts about the
/// *absence* of a call instruction, so neither is expandable — an inlined
/// body's own calls already appear as this function's calls, and expanding
/// it would show one instruction as two independent facts.
function callNeighbours(f, dir) {
  const out = [];
  if (dir > 0) {
    for (const e of f.callees || []) {
      out.push({ name: e.symbol, fnId: e.functionId, kind: 'direct', conf: 'fact', sites: e.sites || [] });
    }
    for (const i of f.inlined || []) {
      out.push({ name: i.name, fnId: i.functionId, kind: 'inlined', conf: 'fact', openable: false,
        note: 'inlined here — its calls appear as this function\'s calls' });
    }
    for (const c of f.foldedCalls || []) {
      out.push({ name: c.name, fnId: c.functionId, kind: 'folded', conf: 'fact', openable: false,
        note: 'written in the source, no call instruction in the object code' });
    }
    (f.indirectCalls || []).forEach((ic, n) => {
      // A site whose targets are all unknown is missing evidence, not a
      // leaf: it must stay on the graph as the site itself, the way the
      // one-hop view kept it, or a dispatcher with unresolved calls reads
      // as calling nothing at all (review of #101).
      // Its identity is the owning function and the site, not the label:
      // every function numbers its sites from one, and two dispatchers'
      // unknown calls are not one shared target (review of #101).
      if (!(ic.measured || []).length && !(ic.candidates || []).length) {
        out.push({ name: `⇢ indirect call #${n + 1}`, fnId: null, key: `site:${f.id}:${n}`, kind: 'indirect-site', conf: 'hypothesis',
          openable: false, note: `at L${ic.line ?? '?'} · targets unknown — needs analysis` });
      }
      for (const m of ic.measured || []) {
        out.push({ name: m.name, fnId: m.functionId, kind: 'indirect-measured', conf: 'run', via: n + 1,
          note: 'resolved by the trace in this evidence' });
      }
      for (const c of ic.candidates || []) {
        if ((ic.measured || []).some(m => m.symbolId === c.symbolId)) continue;
        out.push({ name: c.name, fnId: c.functionId, kind: 'indirect-candidate', conf: 'hypothesis', via: n + 1,
          note: 'a candidate target, not an observed call' });
      }
    });
  } else {
    for (const e of f.callers || []) {
      out.push({ name: e.symbol, fnId: e.functionId, kind: 'direct', conf: 'fact', sites: e.sites || [] });
    }
    for (const p of f.pointerCallers || []) {
      out.push({ name: p.name, fnId: p.functionId, kind: 'pointer-caller',
        conf: p.kind === 'trace' ? 'run' : 'hypothesis',
        note: p.kind === 'trace' ? 'called through a pointer, seen in the trace' : 'takes this function\'s address — a possible caller' });
    }
  }
  return out;
}

/// The expandable call graph around one function.
///
/// Pure: evidence in, nodes and edges out. No DOM and no module state, so
/// the parts that are easy to get wrong — cycles, confidence along a path,
/// the node budget — are testable without a browser.
///
/// `expanded` is the set of node ids the reader has opened. Direction is
/// part of a node's identity: the same function can sit both upstream and
/// downstream of the focus, and those are two different statements about it.
///
/// Expansion only ever grows away from the focus. A caller's own callers are
/// upstream; its callees are not drawn, because they lead back across the
/// graph and say nothing about what reaches the focus.
function callGraphModel(ev, focusId, expanded, opts) {
  expanded = expanded || new Set();
  opts = opts || {};
  const maxNodes = opts.maxNodes || 140;
  const perNode = opts.perNode || 12;
  // Per-node reveals: extra neighbours the reader asked for past the cap.
  // The cap bounds one expansion; it must not permanently remove call
  // relationships from the view (review of #101).
  const more = opts.more || new Map();
  const focus = (ev.functions || [])[focusId];
  if (!focus) return { nodes: [], edges: [], truncated: false, budget: maxNodes };

  // How definite a call kind is, independent of the path that found it. A
  // node reached both as an inlined occurrence and as a direct call is a
  // real function with a real call instruction somewhere -- the inlined
  // status belongs to that one occurrence, never to the function.
  const KIND_RANK = { self: 9, direct: 6, 'indirect-measured': 5, 'indirect-candidate': 4, 'pointer-caller': 3, inlined: 2, folded: 1, 'indirect-site': 0 };

  const nodes = new Map();
  // Edges dedupe by identity, keeping the strongest confidence seen: a
  // reconciliation pass may walk the same edge again with a better path.
  const edgeMap = new Map();
  let truncated = false;

  // External symbols dedupe by name -- one memcpy is one memcpy -- but a
  // neighbour that carries its own key (an unresolved site) is only itself.
  const nodeId = (dir, nb) => (nb.key != null ? `${dir}:${nb.key}` : nb.fnId == null ? `${dir}:ext:${nb.name}` : `${dir}:${nb.fnId}`);
  const addEdge = (from, to, kind, conf, cycle, dir, via) => {
    const key = `${from}|${to}|${kind}|${via ?? ''}`;
    const prev = edgeMap.get(key);
    if (!prev || CONF_RANK[conf] > CONF_RANK[prev.conf]) edgeMap.set(key, { from, to, kind, conf, cycle, dir });
  };

  nodes.set('focus', {
    id: 'focus', fnId: focusId, name: focus.name, dir: 0, depth: 0,
    kind: 'self', conf: 'fact', openable: false, expandable: false, hidden: 0, cycle: false,
  });

  // Breadth first, so a node reached by two paths keeps the shorter one and
  // the budget is spent near the focus rather than down one deep branch.
  // A node is re-enqueued when a later path improves it -- a stronger
  // confidence, or openability it did not have -- so the state everything
  // downstream inherited is recomputed rather than frozen by whichever
  // occurrence enumeration happened to produce first (review of #101).
  // Termination: both upgrades are monotone and bounded, so each node is
  // revisited at most a handful of times.
  const enqueued = new Map(); // id -> best conf it was queued with
  const queue = [{ id: 'focus', fnId: focusId, dir: 0, depth: 0, conf: 'fact', chain: [focusId] }];
  enqueued.set('focus', 'fact');

  while (queue.length) {
    const cur = queue.shift();
    const dirs = cur.depth === 0 ? [-1, 1] : [cur.dir];
    for (const dir of dirs) {
      if (cur.depth > 0 && !expanded.has(cur.id)) continue;
      if (cur.fnId == null) continue;
      const f = ev.functions[cur.fnId];
      if (!f) continue;

      const all = callNeighbours(f, dir);
      const limit = perNode + (more.get(cur.id + ':' + dir) || 0);
      const shown = all.slice(0, limit);
      const parent = nodes.get(cur.id);
      // Recomputed per visit, per direction: assignment let one direction
      // overwrite the other on the focus, and accumulation across revisits
      // would double-count on every reconciliation pass.
      if (parent) parent['hidden' + (dir > 0 ? 'Down' : 'Up')] = all.length - shown.length;

      for (const nb of shown) {
        const conf = weaker(cur.conf, nb.conf);
        const id = nodeId(dir, nb);
        const isCycle = nb.fnId != null && cur.chain.includes(nb.fnId);
        const nbOpen = nb.openable !== false && nb.fnId != null;

        let node = nodes.get(id);
        if (!node) {
          if (nodes.size >= maxNodes) { truncated = true; continue; }
          const external = nb.fnId == null;
          const target = external ? null : ev.functions[nb.fnId];
          const outward = external || !target ? [] : callNeighbours(target, dir);
          node = {
            id, fnId: nb.fnId, name: nb.name, dir, depth: cur.depth + 1,
            kind: nb.kind, conf, note: nb.note || null, via: nb.via,
            external,
            cycle: isCycle,
            openable: nbOpen,
            expandable: nbOpen && !isCycle && outward.length > 0,
            outward: outward.length,
            hiddenUp: 0, hiddenDown: 0,
          };
          nodes.set(id, node);
          if (!isCycle && nbOpen) {
            queue.push({ id, fnId: nb.fnId, dir, depth: cur.depth + 1, conf, chain: cur.chain.concat([nb.fnId]) });
            enqueued.set(id, conf);
          }
        } else {
          // Reconcile: this occurrence may establish more than the first
          // one did. An inlined first occurrence must not make a function
          // later reached by a real call permanently non-expandable, and a
          // candidate-first path must not leave a directly-called function
          // marked hypothetical (review of #101).
          //
          // Once a node is openable its confidence is that of the best call
          // path to it, and only a call path can change it. A folded or
          // inlined occurrence is a fact about the absence of a call; it
          // establishes no route to the out-of-line body, so its certainty
          // must not be lent to a candidate path that does reach it. Its own
          // edge still says what it is (review of #101).
          let improved = false;
          if ((KIND_RANK[nb.kind] ?? 0) > (KIND_RANK[node.kind] ?? 0)) { node.kind = nb.kind; node.note = nb.note || null; node.via = nb.via; }
          if (nbOpen && !node.openable) {
            node.openable = true;
            node.conf = conf;
            const target = ev.functions[nb.fnId];
            const outward = target ? callNeighbours(target, dir) : [];
            node.outward = outward.length;
            node.expandable = !node.cycle && outward.length > 0;
            improved = true;
          } else if ((nbOpen || !node.openable) && CONF_RANK[conf] > CONF_RANK[node.conf]) {
            node.conf = conf;
            improved = nbOpen;
          }
          if (improved && !isCycle && node.openable) {
            const best = enqueued.get(id);
            if (best == null || CONF_RANK[node.conf] > CONF_RANK[best]) {
              queue.push({ id, fnId: nb.fnId, dir, depth: node.depth, conf: node.conf, chain: cur.chain.concat([nb.fnId]) });
              enqueued.set(id, node.conf);
            }
          }
        }
        addEdge(dir > 0 ? cur.id : id, dir > 0 ? id : cur.id, nb.kind, conf, isCycle, dir, nb.via);
      }
    }
  }

  const out = [...nodes.values()].map(n => ({ ...n, hidden: (n.hiddenUp || 0) + (n.hiddenDown || 0) }));
  return { nodes: out, edges: [...edgeMap.values()], truncated, budget: maxNodes };
}

/// Place the call graph's nodes: depth on x, packed on y.
///
/// Pure, like the model: nodes and edges in, coordinates out. Hand-rolled
/// rather than delegated to a layout library because the viewer serves
/// `script-src 'self'` with no `unsafe-eval` (crates/view/src/lib.rs), so
/// nothing can be fetched from a CDN — and after the model has broken cycles
/// the structure is layered enough that a full Sugiyama pass would be more
/// machinery than the picture needs.
///
/// Callers sit at negative depth and callees at positive, so the focus stays
/// in the middle and the reader's left-to-right intuition — what reaches
/// this, then this, then what it reaches — survives expansion.
function callGraphLayout(graph, opts) {
  opts = opts || {};
  const NW = opts.nodeWidth || 180;
  const NH = opts.nodeHeight || 34;
  const XGAP = opts.xGap || 90;
  const YGAP = opts.yGap || 12;
  const PAD = opts.pad || 20;

  const nodes = graph.nodes.map(n => ({ ...n }));
  if (!nodes.length) return { nodes: [], edges: [], width: 0, height: 0 };

  // Signed depth: a caller three hops upstream is at -3.
  for (const n of nodes) n.x0 = n.dir === 0 ? 0 : n.dir * n.depth;

  // Group by column, keeping the order the model produced. That order is
  // breadth-first from the focus, so siblings stay together and a node's
  // children sit near it without any crossing-minimisation pass.
  const byCol = new Map();
  for (const n of nodes) {
    if (!byCol.has(n.x0)) byCol.set(n.x0, []);
    byCol.get(n.x0).push(n);
  }

  // Order children under their parent: walk outward from the focus so each
  // column is laid out after the one that feeds it.
  const parentOf = new Map();
  for (const e of graph.edges) {
    // the edge's far end, relative to the focus, is the child
    const a = nodes.find(n => n.id === e.from);
    const b = nodes.find(n => n.id === e.to);
    if (!a || !b) continue;
    const child = Math.abs(a.x0) > Math.abs(b.x0) ? a : b;
    const parent = child === a ? b : a;
    if (child !== parent && !parentOf.has(child.id)) parentOf.set(child.id, parent.id);
  }

  const cols = [...byCol.keys()].sort((p, q) => Math.abs(p) - Math.abs(q));
  const centreOf = new Map();
  for (const c of cols) {
    const col = byCol.get(c);
    if (c !== 0) {
      // stable sort by the parent's placed centre, so edges run mostly
      // straight instead of crossing the column
      col.sort((a, b) => (centreOf.get(parentOf.get(a.id)) ?? 0) - (centreOf.get(parentOf.get(b.id)) ?? 0));
    }
    const total = col.length * NH + Math.max(0, col.length - 1) * YGAP;
    col.forEach((n, i) => {
      n.w = NW;
      n.h = NH;
      n.y = -total / 2 + i * (NH + YGAP);
      n.x = c * (NW + XGAP);
      centreOf.set(n.id, n.y + NH / 2);
    });
  }

  // Shift everything positive and pad, so the caller can use the numbers
  // directly as an SVG viewBox without a transform of its own.
  const minX = Math.min(...nodes.map(n => n.x));
  const minY = Math.min(...nodes.map(n => n.y));
  for (const n of nodes) {
    n.x += PAD - minX;
    n.y += PAD - minY;
  }
  const width = Math.max(...nodes.map(n => n.x + n.w)) + PAD;
  const height = Math.max(...nodes.map(n => n.y + n.h)) + PAD;

  const at = new Map(nodes.map(n => [n.id, n]));
  const edges = graph.edges.map(e => {
    const a = at.get(e.from);
    const b = at.get(e.to);
    if (!a || !b) return null;
    // A cycle edge back to a node in the same column would be a zero-length
    // line; give it a visible loop instead.
    const selfish = a.id === b.id;
    return {
      ...e,
      x1: a.x + a.w, y1: a.y + a.h / 2,
      x2: b.x, y2: b.y + b.h / 2,
      selfish,
    };
  }).filter(Boolean);

  return { nodes, edges, width, height };
}

/// Which functions the reader has opened, and for which focus.
///
/// Kept beside the graph rather than in the URL: expansion is a way of
/// reading, not a place, and a link someone shares should open the function
/// rather than a particular shape of somebody else's exploration.
let callsOpen = { fnId: null, set: new Set(), more: new Map() };

function callsState(fnId) {
  if (callsOpen.fnId !== fnId) callsOpen = { fnId, set: new Set(), more: new Map() };
  return callsOpen;
}

/// The class that carries an edge's standing into the stylesheet. Kind first,
/// because what an edge *is* matters more to a reviewer than whether the
/// test run happened to take it.
const callNodeClass = n => {
  const byKind = {
    'direct': '',
    'inlined': 'inl',
    'folded': 'fold',
    'indirect-measured': 'ptr-meas',
    'indirect-candidate': 'ptr-cand ptr',
    'pointer-caller': 'ptr',
    'indirect-site': 'ptr',
    'self': 'self',
  }[n.kind] ?? '';
  return [byKind, n.external ? 'ext' : '', n.cycle ? 'cyc' : '', n.conf === 'hypothesis' ? 'guess' : ''].filter(Boolean).join(' ');
};

/// One line under the name, saying what this node is and how it is known.
function callNodeSub(n) {
  if (n.kind === 'self') {
    const f = fn(n.fnId);
    return `${unrecorded(f) ? 'no coverage record' : `${f.count} calls`} · v(G) ${f.complexity?.cyclomaticObject ?? '—'}`;
  }
  if (n.cycle) return 'already shown — recursion';
  if (n.kind === 'inlined') return 'inlined — its calls are this function\'s';
  if (n.kind === 'folded') return 'folded away — no call instruction';
  if (n.kind === 'indirect-site') return n.note || 'targets unknown — needs analysis';
  // The STATEMENT first, the reference to it second. These labels are longer
  // than the node is wide, so one of the two halves is dropped when it is
  // drawn (`fitSub` below); with the pointer first, what survived on the node
  // was "via pointer #1 · candidate, no…" -- the reference kept and the one
  // sentence that stops a hypothesis reading as a fact cut off mid-word
  // (#214). Ordered this way the sentence is what survives, and the node's
  // <title> still carries the whole of it.
  if (n.kind === 'indirect-measured') return `seen in the trace · via pointer #${n.via}`;
  if (n.kind === 'indirect-candidate') return `candidate, not observed · via pointer #${n.via}`;
  if (n.kind === 'pointer-caller') return n.conf === 'run' ? 'called through a pointer, in the trace' : 'takes this address — possible caller';
  if (n.external) return 'outside the covered source set';
  const f = n.fnId != null ? fn(n.fnId) : null;
  return f ? (unrecorded(f) ? 'no coverage record' : `called ${f.count}×`) : '';
}

// How much of a node's two text rows actually fits inside it.
//
// The budgets were `n.w / 8` and `n.w / 6`, which were wrong twice over: they
// counted the whole node, though the text starts 10px in and the box ends
// 10px before its own border, and they ignored the controls that sit on the
// SAME baseline -- `→` and `+N more…` on the sub row, `+N`/`−` on the name
// row. So the labels both overran the node's border and were cut off
// mid-word: "via pointer #1 · candidate, no…" drew to x=196.6 on a 190-wide
// box, and the external node's "outside the covered source set" to x=190.6
// (measured in ui/browser/checks/calls.cjs, which pins both).
//
// The advances are of the monospace the two classes set, 11.5px for the name
// and 10px for the sub. They are measured, not assumed, and the browser
// check re-measures them on the host it runs on, so a font that renders
// wider here fails there rather than silently clipping.
const NAME_ADVANCE = 6.92, SUB_ADVANCE = 6.02, NODE_PAD = 10;
/// Width in pixels the control on a row takes from the text beside it.
const CTL = { more: 17, go: 14, capped: 56 };
const nameBudget = n => Math.max(6, Math.floor(
  (n.w - 2 * NODE_PAD - (n.expandable ? CTL.more : 0) - (n.hidden > 0 && n.openable ? CTL.go : 0)) / NAME_ADVANCE));
const subBudget = n => Math.max(6, Math.floor(
  (n.w - 2 * NODE_PAD - (n.hidden > 0 ? CTL.capped : (n.openable && n.fnId != null && n.kind !== 'self') ? CTL.go : 0)) / SUB_ADVANCE));
/// Fit a `·`-separated sub-label by dropping whole trailing segments, so what
/// is drawn on the node is a complete statement rather than a cut-off one.
/// Only when even the first segment does not fit is it cut by characters.
const fitSub = (s, budget) => {
  if (s.length <= budget) return s;
  const parts = s.split(' · ');
  for (let k = parts.length - 1; k >= 1; k--) {
    const cut = `${parts.slice(0, k).join(' · ')} …`;
    if (cut.length <= budget) return cut;
  }
  return trunc(s, budget);
};

function renderCalls(body, f) {
  const state = callsState(f.id);
  const expanded = state.set;
  const graph = callGraphModel(S.ev, f.id, expanded, { more: state.more });
  // 210, not 190: at 190 the correct budget above leaves the sub row 25
  // characters, which "candidate, not observed …" fills exactly and the
  // external node's "outside the covered source set" does not fit at all --
  // so the node has to carry a little more width for these labels to be
  // readable as the statements they are (#214).
  const laid = callGraphLayout(graph, { nodeWidth: 210, nodeHeight: 38 });

  const svg = h('svg:svg', { width: laid.width, height: laid.height, viewBox: `0 0 ${laid.width} ${laid.height}` });
  const defs = h('svg:defs');
  for (const c of ['cov', 'unc', 'unk']) {
    defs.append(h('svg:marker', { id: 'bfarr-' + c, viewBox: '0 0 10 10', refX: 9, refY: 5, markerWidth: 7, markerHeight: 7, orient: 'auto' },
      h('svg:path', { d: 'M0 0L10 5L0 10z', fill: `var(--${c})` })));
  }
  svg.append(defs);

  const edgeCls = e => (e.conf === 'hypothesis' ? 'ptr' : e.conf === 'run' ? 'ptr-meas' : plainMode() ? 'unk' : 'cov');
  for (const e of laid.edges) {
    if (e.selfish) {
      // a loop above the node, so recursion is visible rather than a
      // zero-length line hidden under the box
      const n = laid.nodes.find(x => x.id === e.from);
      svg.append(h('svg:path', { class: 'edge ' + edgeCls(e),
        d: `M${n.x + n.w * 0.35} ${n.y} C ${n.x + n.w * 0.2} ${n.y - 26}, ${n.x + n.w * 0.8} ${n.y - 26}, ${n.x + n.w * 0.65} ${n.y}` }));
      continue;
    }
    svg.append(h('svg:path', { class: 'edge ' + edgeCls(e),
      d: `M${e.x1} ${e.y1} C ${(e.x1 + e.x2) / 2} ${e.y1}, ${(e.x1 + e.x2) / 2} ${e.y2}, ${e.x2} ${e.y2}`,
      'marker-end': `url(#bfarr-${edgeCls(e) === 'ptr-meas' ? 'cov' : edgeCls(e) === 'ptr' ? 'unk' : edgeCls(e)})` }));
  }

  const redraw = () => { body.replaceChildren(); renderCalls(body, f); };

  for (const n of laid.nodes) {
    const cls = ['bf-node', callNodeClass(n), n.expandable ? 'can-open' : '', expanded.has(n.id) ? 'is-open' : ''].filter(Boolean).join(' ');
    const g = h('svg:g', { class: cls, transform: `translate(${n.x},${n.y})` });
    g.append(h('svg:rect', { width: n.w, height: n.h }));
    g.append(h('svg:title', {}, `${n.name}\n${callNodeSub(n)}`));
    g.append(h('svg:text', { x: NODE_PAD, y: 16 }, trunc(n.name, nameBudget(n))));
    g.append(h('svg:text', { class: 'sub', x: NODE_PAD, y: 29 }, fitSub(callNodeSub(n), subBudget(n))));

    // Expanding is the node's own gesture: one click, no waiting to find out
    // whether a second is coming.
    if (n.expandable) {
      const open = expanded.has(n.id);
      g.setAttribute('tabindex', '0');
      g.setAttribute('role', 'button');
      g.setAttribute('aria-expanded', open ? 'true' : 'false');
      g.setAttribute('aria-label', `${n.name}, ${open ? 'collapse' : `expand ${n.outward} more`}`);
      const toggle = () => { open ? expanded.delete(n.id) : expanded.add(n.id); redraw(); };
      g.addEventListener('click', toggle);
      g.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggle(); } });
      g.append(h('svg:text', { class: 'more', x: n.w - 10, y: 16, 'text-anchor': 'end' }, open ? '−' : `+${n.outward}`));
    }

    // Neighbours this node has but the graph did not draw. Without saying
    // so, a capped node is indistinguishable from a fully drawn one, and the
    // reader would take an incomplete picture for the whole of it.
    if (n.hidden > 0) {
      // A control, not a caption: the cap bounds one expansion, it must not
      // permanently remove call relationships from the view (review of
      // #101). Clicking raises this node's own limit, in the direction(s)
      // actually capped, within the overall budget.
      const reveal = () => {
        if (n.hiddenUp > 0) state.more.set(n.id + ':-1', (state.more.get(n.id + ':-1') || 0) + 12);
        if (n.hiddenDown > 0) state.more.set(n.id + ':1', (state.more.get(n.id + ':1') || 0) + 12);
        redraw();
      };
      const more = h('svg:text', { class: 'capped', x: n.w - 10, y: n.h - 8, 'text-anchor': 'end',
        role: 'button', tabindex: '0', onclick: ev => { ev.stopPropagation(); reveal(); } }, `+${n.hidden} more…`);
      more.addEventListener('keydown', ev => { if (ev.key === 'Enter' || ev.key === ' ') { ev.preventDefault(); ev.stopPropagation(); reveal(); } });
      more.append(h('svg:title', {}, `show more of the ${n.hidden} neighbour(s) not drawn`));
      g.append(more);
    }

    // Opening in the source pane is a separate control, so the graph is
    // never lost by a click meant to look deeper.
    if (n.openable && n.fnId != null && n.kind !== 'self') {
      const go = h('svg:text', { class: 'go', x: n.w - 10, y: n.hidden > 0 ? 16 : n.h - 8, 'text-anchor': 'end',
        onclick: ev => { ev.stopPropagation(); navigate(`#/fn/${n.fnId}`); } }, '→');
      go.append(h('svg:title', {}, `open ${n.name}`));
      g.append(go);
    }
    svg.append(g);
  }

  const wrap = h('div', { class: 'graph-wrap' }, svg);
  enablePan(wrap);

  const up = graph.nodes.filter(n => n.dir < 0).length;
  const down = graph.nodes.filter(n => n.dir > 0).length;
  const guesses = graph.nodes.filter(n => n.conf === 'hypothesis').length;
  body.append(h('div', { class: 'graph-bar' },
    h('span', { class: 'muted' }, `${up} upstream · ${down} downstream${expanded.size ? ` · ${expanded.size} expanded` : ''}`),
    graph.truncated ? h('span', { class: 'warn-tag' }, `stopped at ${graph.budget} nodes`) : null,
    guesses ? h('span', {}, h('i', { style: 'display:inline-block;width:10px;height:2px;border-top:2px dashed var(--jus);vertical-align:middle;margin-right:4px' }),
      h('span', { class: 'muted' }, `${guesses} reached only through a candidate — a hypothesis, not a measured path`)) : null,
    h('span', { class: 'muted' }, 'click a node to expand · → opens it')), wrap);
}

/* ---------------- object code ---------------- */
function renderObject(body, f) {
  const o = f.object; const fl = file(f.fileId);
  const t = h('table', { class: 'obj' + (plainMode() ? ' plain' : '') });
  const cols0 = hasMeasured() ? 5 : 4;
  const appendBody = ob => {
  const gapById = new Map(ob.gaps.map(g => [g.id, g]));
  // block ids are per body: only the function's own body's rows answer to
  // the Flow graph's block selection
  const blockId = b => ob === o ? b.id : null;
  for (const b of ob.blocks) {
    const st = blockStatus(f, b);
    t.append(h('tr', { class: 'blk', 'data-block': blockId(b) }, h('td', { colspan: hasMeasured() ? 5 : 4 },
      h('b', {}, `B${b.id}`), ` · ${hex(b.address)} · ${b.kind} · ${b.instrs.length} instr · `,
      h('span', { style: `color:var(--${st === 'jus' ? 'jus' : st})` }, blockStatusLabel(st)),
      b.succs.length ? ` · → ${b.succs.map(e => `B${e.to}${e.kind === 'taken' ? '(T)' : e.kind === 'fallthrough' ? '(F)' : ''}`).join(', ')}` : ' · exit',
      b.note ? h('span', { class: 'infer', title: b.note }, ' · inferred ⓘ') : null,
      b.diverges ? h('span', { class: 'infer', title: b.diverges }, ' · does not return ⓘ') : null)));
    for (const l of (b.inferredLines || [])) {
      t.append(h('tr', { class: 'srcl inf ' + lineStatus(f.fileId, l), 'data-line': l, onclick: () => selectLine(l), onmouseenter: () => hl('line', l, true), onmouseleave: () => hl('line', l, false), title: 'no instruction is attributed to this line; assigned to this block by branch-outcome inference' },
        h('td', { colspan: cols0 }, h('span', { class: 'sln' }, `≈ L${l}`), plainMode() ? null : h('span', { class: 'scnt' }, `${lineCount(f.fileId, l) ?? 0}×`), h('span', { class: 'scode', html: highlight((fl.text[l - 1] || '').trim()) }), h('span', { class: 'muted', style: 'margin-left:8px;font-size:11px' }, 'inferred — no instruction attributed'))));
    }
    let curLine; let curInl = null;
    for (const ins of b.instrs) {
      const st = instrStatus(f, ins); const ctl = ins.flags.length > 0;
      const gaps = ins.gapIds.map(id => gapById.get(id)).filter(Boolean);
      // inlined-callee boundary marker
      if ((ins.inlined ?? null) !== (curInl ?? null)) {
        curInl = ins.inlined ?? null;
        if (curInl != null) { const inl = f.inlined[curInl]; t.append(h('tr', { class: 'inlmark' }, h('td', { colspan: cols0 }, '⤷ inlined ', h('code', {}, inl.name), inl.callLine ? ` (called at L${inl.callLine})` : '', inl.functionId != null ? h('a', { class: 'lnk', style: 'margin-left:8px', onclick: () => navigate(`#/fn/${inl.functionId}`) }, 'open') : null))); }
        else t.append(h('tr', { class: 'inlmark end' }, h('td', { colspan: cols0 }, '⤶ back in ', h('code', {}, f.name))));
      }
      // interleave the source line the following instructions implement.
      // The line is a line OF A FILE: code inlined from a header carries its
      // own fileId, and its status, count, text and tags are that file's,
      // not the function's own file's at the same number (#317). Two files'
      // line 12 in a row are two source rows, not one.
      const fid = insFileId(f, ins);
      const ownFile = fid === f.fileId;
      const srcFile = ownFile ? fl : file(fid);
      const lineKeyOf = ins.line == null ? 'none' : `${fid}:${ins.line}`;
      if (lineKeyOf !== curLine) {
        curLine = lineKeyOf;
        const cols = hasMeasured() ? 5 : 4;
        if (ins.line) {
          const ls = lineStatus(fid, ins.line); const cnt = lineCount(fid, ins.line);
          const ltags = insLineTags(f, ins);
          t.append(h('tr', { class: 'srcl ' + ls, 'data-line': ownFile ? ins.line : '', onclick: ownFile ? () => selectLine(ins.line) : null, onmouseenter: ownFile ? () => hl('line', ins.line, true) : null, onmouseleave: ownFile ? () => hl('line', ins.line, false) : null },
            h('td', { colspan: cols }, h('span', { class: 'sln' }, `${srcFile !== fl ? srcFile.display + ':' : ''}L${ins.line}`), plainMode() || cnt == null ? null : h('span', { class: 'scnt' }, `${cnt}×`), h('span', { class: 'scode', html: highlight((srcFile.text[ins.line - 1] || '').trim()) }), ltags.map(r => h('span', { class: 'req', style: 'margin-left:8px', title: 'requirement tagged on this line: the instructions below implement it' }, r)))));
        } else {
          t.append(h('tr', { class: 'srcl none' }, h('td', { colspan: cols }, h('span', { class: 'sln' }, ins.line === 0 ? 'L0' : '—'), h('span', { class: 'scode muted' }, ins.line === 0 ? 'compiler-generated (line 0)' : 'no source line'))));
        }
      }
      const tr = h('tr', { class: 'i ' + st, 'data-line': ownFile && ins.line ? ins.line : '', 'data-block': blockId(b), title: ins.line ? `${srcFile.display}:${ins.line}  ${(srcFile.text[ins.line - 1] || '').trim()}` : 'no source line',
        onmouseenter: ownFile && ins.line ? () => hl('line', ins.line, true) : null, onmouseleave: ownFile && ins.line ? () => hl('line', ins.line, false) : null, onclick: ownFile && ins.line ? () => selectLine(ins.line) : null });
      // requirement(s) this instruction implements: its own source line's tag
      // (of the file the line is in), else the inlined callee's, else the
      // function's; compiler-generated code gets none
      const generated = ins.origin && !ins.origin.startsWith('inlined:');
      const lineTags = insLineTags(f, ins);
      let ireqs = [];
      if (!generated) {
        if (lineTags.length) ireqs = lineTags;
        else if (ins.inlined != null && f.inlined[ins.inlined] && f.inlined[ins.inlined].functionId != null) ireqs = fn(f.inlined[ins.inlined].functionId).requirements;
        else ireqs = f.requirements;
      }
      tr.append(h('td', { class: 'a' }, ins.addr.toString(16)), h('td', { class: 'm' + (ctl ? ' ctl' : '') }, ins.mnemonic), h('td', { class: 'op' }, ins.operands,
        ins.origin ? h('span', { class: 'org ' + (generated ? 'gen' : 'inl'), title: generated ? 'compiler-generated: not directly traceable to a source statement of this function (6.4.4.2.b)' : 'attributed to an inlined callee' }, generated ? '⚙ ' + ins.origin : '⤷ ' + ins.origin.slice(8)) : null,
        ireqs.map(r => h('span', { class: 'org rq', title: 'implements requirement ' + r + (lineTags.includes(r) ? ' (tagged on this line)' : ' (function-level tag)') }, r))));
      if (hasMeasured()) tr.append(h('td', { class: 'x', title: 'executions in the trace' }, ins.measured == null ? '·' : ins.measured));
      tr.append(
        h('td', { class: 'g' }, gaps.map(g => h('span', { class: 'gapmark' + (resFor(g.key) ? ' res' : ''), title: `${slug(g.kind)}: ${g.description}`, onclick: e => { e.stopPropagation(); S.focusGap = g.key; setTab('gaps'); } }, '▲'))));
      t.append(tr);
    }
  }
  };
  appendBody(o);
  // The constructor/destructor variants (#266): the same rows, under a header
  // that says what each is and why its status is not read from line counts.
  for (const v of (f.variants || [])) {
    const why = f.count === 0 ? 'the body never ran, so neither did this variant: projected uncovered'
      : 'no coverage counter of its own: whether it ran is not established, so its instructions are projected unknown, not covered';
    t.append(h('tr', { class: 'blk' }, h('td', { colspan: cols0 },
      h('b', {}, `${v.variant} · ${VARIANT_ROLE[v.variant] || 'constructor/destructor variant'}`), ` · ${v.symbol} @ ${hex(v.address)} (${v.size} B) · ${v.instrTotal} instr · `,
      h('span', { style: `color:var(--${f.count === 0 ? 'unc' : 'jus'})` }, why),
      hasMeasured() ? h('span', {}, ` · ${v.instrMeasuredCovered ?? 0} executed in trace`) : null)));
    appendBody(v);
  }
  // source lines of the function that own no instruction at all
  const owned = new Set(); for (const bb of o.blocks) { for (const i of bb.instrs) if (i.line && insFileId(f, i) === f.fileId) owned.add(i.line); for (const l of (bb.inferredLines || [])) owned.add(l); }
  const orphan = []; for (let l = f.lineStart; l <= f.lineEnd; l++) { if (owned.has(l)) continue; const c = lineCount(f.fileId, l); if (c == null) continue; const txt = (fl.text[l - 1] || '').trim(); if (!txt || txt === '}' || txt === '{') continue; orphan.push([l, c]); }
  if (orphan.length) {
    t.append(h('tr', { class: 'blk' }, h('td', { colspan: cols0 }, h('b', {}, 'Source lines with no object code'), ' · merged, predicated or optimised away — their coverage exists only at source level')));
    for (const [l, c] of orphan) t.append(h('tr', { class: 'srcl orphan ' + lineStatus(f.fileId, l), 'data-line': l, onclick: () => selectLine(l), onmouseenter: () => hl('line', l, true), onmouseleave: () => hl('line', l, false) },
      h('td', { colspan: cols0 }, h('span', { class: 'sln' }, `L${l}`), plainMode() ? null : h('span', { class: 'scnt' }, `${c}×`), h('span', { class: 'scode', html: highlight((fl.text[l - 1] || '').trim()) }))));
  }
  const reqBar = f.requirements.length ? h('div', { class: 'pane-tools', style: 'gap:6px' }, h('span', { class: 'muted' }, 'implements'), f.requirements.map(r => h('span', { class: 'req' }, r)), h('span', { class: 'muted', style: 'margin-left:6px' }, '— every instruction below is object code for these requirements; line tags narrow it further')) : null;
  if (reqBar) body.append(reqBar);
  const tot = objTotals(f);
  body.append(h('div', { class: 'pane-tools' }, h('span', {}, `${tot.instrTotal} instructions · `), h('span', { style: 'color:var(--cov)' }, `${tot.instrProjectedCovered} projected covered`), h('span', {}, '·'), h('span', { style: 'color:var(--unc)' }, `${tot.instrProjectedUncovered} uncovered`), h('span', {}, '·'), h('span', {}, `${tot.instrUnknown} unknown`), hasMeasured() ? h('span', { style: 'color:var(--accent-2)' }, `· ${tot.instrMeasuredCovered} executed in trace`) : null, h('span', { style: 'color:var(--jus)', title: Object.entries(tot.compilerGenerated).map(([k, v]) => `${k}: ${v}`).join('\n') }, `· ⚙ ${Object.values(tot.compilerGenerated).reduce((a, b) => a + b, 0)} compiler-generated`), h('span', { style: 'margin-left:auto' }, '▲ = gap finding')), t);
}

/* ---------------- gaps ---------------- */
function renderGaps(body, f) {
  const gaps = fnGaps(f); const fl = file(f.fileId);
  const wrap = h('div', { class: 'gaps' });
  const ds = f.object ? objTotals(f).disagreements : [];
  if (hasMeasured() && ds.length) {
    const byKind = {}; for (const d of ds) byKind[d.kind] = (byKind[d.kind] || 0) + 1;
    const lbl = { projectedNotExecuted: ['pne', 'projected covered, not in trace'], executedNotProjected: ['enp', 'in trace, projected uncovered'], executedUnknown: ['eu', 'in trace, no projection'] };
    const box = h('div', { class: 'dec' }, h('div', { class: 'dec-head' }, h('span', {}, h('b', {}, 'Projection vs trace')), Object.entries(byKind).map(([k, n]) => h('span', { class: 'chip' }, h('span', { class: 'k ' + lbl[k][0], style: 'font-size:10px' }, lbl[k][1]), h('b', {}, n)))),
      h('div', { class: 'note', style: 'margin:8px 10px' }, S.ev.measured.caveats[S.ev.measured.caveats.length - 1]));
    // one row per (kind, source line): instruction count, address range, max executions
    const groups = new Map();
    for (const d of ds) { const k = `${d.kind}:${d.line ?? 'none'}`; if (!groups.has(k)) groups.set(k, { kind: d.kind, line: d.line, n: 0, lo: d.addr, hi: d.addr, max: 0 }); const g = groups.get(k); g.n++; g.lo = Math.min(g.lo, d.addr); g.hi = Math.max(g.hi, d.addr); g.max = Math.max(g.max, d.measured); }
    const rows = [...groups.values()].sort((a, b) => a.kind.localeCompare(b.kind) || (a.line ?? 0) - (b.line ?? 0));
    const list = h('div');
    for (const g of rows.slice(0, 80)) list.append(h('div', { class: 'dis' }, h('span', { class: 'k ' + lbl[g.kind][0] }, lbl[g.kind][1]), h('span', {}, h('a', { class: 'lnk', onclick: () => { S.tab = 'object'; location.hash = `#/fn/${f.id}/object`; } }, g.lo === g.hi ? hex(g.lo) : `${hex(g.lo)}–${hex(g.hi)}`), ` · ${g.n} instr`, g.line ? [' · ', h('a', { class: 'lnk', onclick: () => selectLine(g.line) }, `L${g.line}`), ' ', h('span', { class: 'muted' }, (fl.text[g.line - 1] || '').trim().slice(0, 50))] : [' · ', h('span', { class: 'muted' }, 'no source line')]), h('span', { class: 'muted' }, g.max ? `${g.max}×` : '0×')));
    if (rows.length > 80) list.append(h('div', { class: 'muted', style: 'padding:6px 10px' }, `… ${rows.length - 80} more lines`));
    box.append(list); wrap.append(box);
  }
  if (!gaps.length) { wrap.append(h('div', { class: 'empty' }, 'No object-code gaps: every instruction maps to an instrumented source line and every source decision has a matching object-level branch.')); body.append(wrap); return; }
  const byKind = {}; for (const g of gaps) byKind[slug(g.kind)] = (byKind[slug(g.kind)] || 0) + 1;
  const unresolved = gaps.filter(g => !resFor(g.key)).length;
  const byClass = {}; for (const g of gaps) if (slug(g.kind) === 'helper-call') byClass[g.class || 'unknown'] = (byClass[g.class || 'unknown'] || 0) + 1;
  wrap.append(h('div', { class: 'gap-sum' }, h('span', { class: 'chip' }, h('b', {}, unresolved), ' unresolved of ', h('b', {}, gaps.length)), Object.entries(byKind).map(([k, n]) => h('span', { class: 'chip' }, h('span', { class: 'kind ' + k }, k), h('b', {}, n))), Object.entries(byClass).map(([k, n]) => h('span', { class: 'chip', title: 'helper-call class' }, h('span', { class: 'kind cls-' + k }, k), h('b', {}, n)))));
  const order = ['no-coverage-record', 'uncounted-variant', 'folded-call', 'shared-line', 'predicated-decision', 'extra-object-branch', 'no-source-line', 'line-zero', 'indirect-branch', 'helper-call', 'not-instrumented'];
  const sorted = [...gaps].sort((a, b) => order.indexOf(slug(a.kind)) - order.indexOf(slug(b.kind)) || a.addrStart - b.addrStart);
  // Group helper calls by callee so 70 memcpy call sites read as one finding.
  const groups = new Map();
  const items = [];
  const classOrder = ['runtime-check', 'arith-helper', 'memory-helper', 'project-uncovered', 'core-lib', 'libc', 'unknown'];
  for (const g of sorted) {
    if (slug(g.kind) === 'helper-call') {
      const callee = (g.description.match(/`([^`]+)`/) || [])[1] || g.description;
      if (!groups.has(callee)) { groups.set(callee, []); items.push({ group: callee, gaps: groups.get(callee), cls: g.class || 'unknown' }); }
      groups.get(callee).push(g);
    } else items.push({ gap: g });
  }
  items.sort((a, b) => (a.group ? 1 : 0) - (b.group ? 1 : 0) || (a.group && b.group ? classOrder.indexOf(a.cls) - classOrder.indexOf(b.cls) || a.group.localeCompare(b.group) : 0));
  for (const it of items) {
    if (it.group) {
      const gs = it.gaps; const unres = gs.filter(g => !resFor(g.key)).length;
      const CLS = {
        'runtime-check': 'compiler/runtime check — defensive structure, not dead code (glossary); justify by analysis',
        'arith-helper': 'compiler-rt / libgcc arithmetic helper — compiler-generated code sequence (6.4.4.2.b)',
        'memory-helper': 'memory helper (memcpy/memset family) — compiler-generated call',
        'core-lib': 'language core/std library — needs its own verification credit or a qualified library',
        'libc': 'C library — outside the airborne software, or needs library verification credit',
        'project-uncovered': 'project code with no coverage evidence in this export — the coverage run did not instantiate it',
        'unknown': 'unclassified callee',
      };
      const el = h('div', { class: 'gap' + (unres ? '' : ' resolved') },
        h('span', {}, h('span', { class: 'kind helper-call' }, 'helper-call'), h('br'), h('span', { class: 'kind cls-' + it.cls, style: 'margin-top:4px;display:inline-block' }, it.cls)),
        h('div', { class: 'desc' }, `${gs.length} call site${gs.length > 1 ? 's' : ''} to `, h('code', {}, it.group), ' — ', CLS[it.cls] || CLS.unknown, '.'),
        h('div', { class: 'where' }, gs.slice(0, 12).map((g, i) => [i ? ' · ' : null, h('a', { onclick: () => g.line && selectLine(g.line) }, g.line ? `L${g.line}` : hex(g.addrStart))]), gs.length > 12 ? ` · +${gs.length - 12} more` : null),
        unres < gs.length ? h('div', { class: 'res' }, `${gs.length - unres} of ${gs.length} sites resolved`) : null,
        h('div', { class: 'act' }, h('button', { class: 'ghost', title: 'apply one resolution to every call site of this callee', onclick: () => openResolution(gs.map(g => g.key), `helper-call ×${gs.length} to ${it.group} in ${f.name}`, gs[0].description) }, unres ? 'Resolve all' : 'Edit all')));
      wrap.append(el);
      continue;
    }
    const g = it.gap;
    const r = resFor(g.key); const k = slug(g.kind);
    const el = h('div', { class: 'gap' + (r ? ' resolved' : '') + (S.focusGap === g.key ? ' focus' : ''), 'data-gap': g.key },
      h('span', { class: 'kind ' + k }, k),
      h('div', { class: 'desc' }, g.description),
      h('div', { class: 'where' },
        g.addrStart ? h('a', { onclick: () => { S.tab = 'object'; location.hash = `#/fn/${f.id}/object`; setTimeout(() => { const row = $(`#content .obj tr.i[data-line]`); const t = [...document.querySelectorAll('#content .obj tr.i')].find(tr => parseInt(tr.firstChild.textContent, 16) === g.addrStart); if (t) { t.scrollIntoView({ block: 'center' }); t.classList.add('hl'); } }, 50); } }, g.addrStart === g.addrEnd ? hex(g.addrStart) : `${hex(g.addrStart)}–${hex(g.addrEnd)}`) : null,
        g.line ? [' · ', h('a', { onclick: () => selectLine(g.line) }, `L${g.line}`), ' ', h('span', { class: 'muted' }, (fl.text[g.line - 1] || '').trim().slice(0, 60))] : null),
      r ? h('div', { class: 'res' }, h('b', {}, r.category.toUpperCase()), ' ', CATS[r.category] ? CATS[r.category].split(' — ')[1] : '', ' — ', r.justification, h('span', { class: 'muted' }, ` (${r.author}, ${r.date})`)) : null,
      h('div', { class: 'act' }, h('button', { class: 'ghost', onclick: () => openResolution(g.key, `${k} in ${f.name}`, g.description) }, r ? 'Edit' : 'Resolve')));
    wrap.append(el);
  }
  body.append(wrap);
  if (S.focusGap != null) setTimeout(() => { const e = [...wrap.querySelectorAll('[data-gap]')].find(x => x.dataset.gap === S.focusGap); if (e) e.scrollIntoView({ block: 'center' }); }, 30);
}

/* ---------------- MC/DC ---------------- */
/// MC/DC in a sentence, per condition.
///
/// The truth table is the evidence, but it is not the explanation: a reader
/// looking at "C2 · true 1 · false 0 · ✗ no pair" still has to work out what
/// an independence pair is, which two records proved the other conditions,
/// and what test would close the one that failed. Say it instead.
///
/// Phrased in **condition vectors** unless the evidence can do better.
/// `Decision::vectors` holds LLVM's condition vectors: one test can produce
/// several (a decision inside a loop), several tests can collapse into one,
/// records are deduplicated across merged instantiations, and some may carry
/// `executed: false`. So the evidence cannot say how many tests ran, and this
/// text must not imply that it can (review of #32).
///
/// What it *can* say, when per-test exports were ingested, is which tests
/// produced a given vector -- `TestVector.tests` (#48). Where both vectors of
/// a pair carry that, the sentence names the tests a reviewer would re-run
/// instead of two numbers they would then have to look up. Where either does
/// not, the vector wording above stands unchanged: an empty `tests` means
/// "could not be determined", never "no test did this", and inventing a name
/// for it would be the confident sentence the evidence does not support.
// The tests that produced one vector, named. `undefined` when the evidence
// cannot say -- no per-test exports were ingested, or none of them reproduced
// this vector -- which is not the same as "no test did", and the caller must
// fall back to the vector number rather than guess (#48).
function vectorTestNames(v) {
  const ids = (v && v.tests) || [];
  if (!ids.length || !S.ev.tests || !S.ev.tests.length) return undefined;
  const names = ids.map(id => S.ev.tests[id] && S.ev.tests[id].name).filter(Boolean);
  return names.length ? names : undefined;
}
// Several tests producing the same vector is ordinary, and either one proves
// the pair: say so rather than picking one and implying it is the only.
//
// Parenthesised, because the alternatives sit inside a sentence that already
// joins the two halves of the pair with "and": a flat "t_a or t_b and t_c"
// does not say whether the "or" or the "and" binds tighter, and this sentence
// is the evidence a reviewer acts on.
const namesPhrase = ns => (ns.length === 1 ? ns[0] : `${ns[0]} (or ${ns.slice(1).join(', ')})`);

function mcdcStory(d, i, condName) {
  const c = d.conditions[i];
  const name = condName || `C${i + 1}`;
  const val = (v, k) => v.values[k];
  if (c.independenceCovered && c.pair) {
    const [a, b] = c.pair;
    const va = d.vectors[a - 1], vb = d.vectors[b - 1];
    if (va && vb) {
      // read the pair in the direction the condition actually moved
      const [lo, hi] = val(va, i) === false ? [[a, va], [b, vb]] : [[b, vb], [a, va]];
      const moved = `they differ only in ${name}, which goes false → true, and the decision follows (${lo[1].result ? 'true' : 'false'} → ${hi[1].result ? 'true' : 'false'}).`;
      // Both halves must be attributable, or the sentence would name one
      // test and a bare number for the other -- worse than two numbers.
      const [loT, hiT] = [vectorTestNames(lo[1]), vectorTestNames(hi[1])];
      if (loT && hiT) {
        return { ok: true, text: `Proven by ${namesPhrase(loT)} and ${namesPhrase(hiT)} (condition vectors ${lo[0]} and ${hi[0]}): ${moved}` };
      }
      return { ok: true, text: `Proven by condition vectors ${lo[0]} and ${hi[0]}: ${moved}` };
    }
    return { ok: true, text: `Proven by condition vectors ${a} and ${b}.` };
  }
  if (c.independenceCovered) return { ok: true, text: 'Proven.' };

  // why not, from the counts and the vectors alone
  const seen = c.trueCount + c.falseCount;
  if (seen === 0) {
    return { ok: false, text: `Never evaluated: every recorded vector short-circuited before reaching ${name}. Nothing can be concluded about it until a test reaches it.` };
  }
  if (c.falseCount === 0) {
    return { ok: false, text: `Not proven. ${name} was evaluated ${c.trueCount}× and was true every time, so no test shows what a false ${name} does. Add a test that reaches ${name} with it false.` };
  }
  if (c.trueCount === 0) {
    return { ok: false, text: `Not proven. ${name} was evaluated ${c.falseCount}× and was false every time, so no test shows what a true ${name} does. Add a test that reaches ${name} with it true.` };
  }
  return { ok: false, text: `Not proven. ${name} was seen both true and false, but in every pair of recorded vectors another condition changed as well, so the outcome can never be attributed to ${name} alone. Add a test that flips ${name} while the others stay as they are.` };
}

function renderMcdc(body, f) {
  const fl = file(f.fileId); const wrap = h('div', { class: 'mcdc' });
  if (!f.decisions.length && !f.branches.length) { wrap.append(h('div', { class: 'empty' }, 'No decisions in this function.')); body.append(wrap); return; }
  const srcText = (l1, c1, l2, c2) => { if (l1 === l2) return (fl.text[l1 - 1] || '').slice(c1 - 1, c2 - 1); return (fl.text[l1 - 1] || '').slice(c1 - 1) + ' … ' + (fl.text[l2 - 1] || '').slice(0, c2 - 1); };
  f.decisions.forEach((d, di) => {
    const ok = d.mcdcCovered; const n = d.conditions.length;
    const covered = d.conditions.filter(c => c.independenceCovered).length;
    const allJustified = d.conditions.every(c => c.independenceCovered || resFor(condKey(f.id, d, c.index)));
    const box = h('div', { class: 'dec' });
    box.append(h('div', { class: 'dec-head' }, h('span', {}, `Decision `, h('a', { onclick: () => selectLine(d.line) }, `L${d.line}:${d.col}`)), h('code', {}, srcText(d.line, d.col, d.lineEnd, d.colEnd)),
      h('span', { class: 'muted' }, `true ${d.trueCount}× · false ${d.falseCount}×`),
      h('span', { class: 'st ' + (ok ? 'ok' : allJustified ? 'jus' : 'bad') }, ok ? `MC/DC achieved (${n}/${n})` : allJustified ? `${covered}/${n} + justified` : `MC/DC not achieved (${covered}/${n})`)));
    // conditions
    const ct = h('table', { class: 'tt' }, h('tr', {}, h('th', {}, '#'), h('th', {}, 'condition'), h('th', {}, 'true'), h('th', {}, 'false'), h('th', { title: 'the two condition vectors that prove this condition changes the outcome on its own' }, 'pair'), h('th', {}, '')));
    const condSrc = (c, i) => {
      const next = d.conditions[i + 1];
      const end = next && next.line === c.line ? next.col : (c.line === d.lineEnd ? d.colEnd : (fl.text[c.line - 1] || '').length + 1);
      let t = (fl.text[c.line - 1] || '').slice(c.col - 1, end - 1).replace(/[\s(]*(&&|\|\|)\s*$/, '').replace(/^\s*(&&|\|\|)\s*/, '').trim();
      // the slice runs to the next condition, so an operand that closes a
      // group keeps the bracket -- `speed_ok)`. Drop only unbalanced ones.
      const bal = str => [...str].reduce((n, ch) => n + (ch === '(' ? 1 : ch === ')' ? -1 : 0), 0);
      while (t.endsWith(')') && bal(t) < 0) t = t.slice(0, -1).trim();
      while (t.startsWith('(') && bal(t) > 0) t = t.slice(1).trim();
      return t;
    };
    d.conditions.forEach((c, i) => {
      const key = condKey(f.id, d, i); const r = resFor(key);
      ct.append(h('tr', { onmouseenter: () => c.pair && hlPair(box, c.pair, true), onmouseleave: () => c.pair && hlPair(box, c.pair, false) },
        h('td', {}, `C${i + 1}`), h('td', {}, h('code', {}, condSrc(c, i) || `(${c.line}:${c.col})`)),
        h('td', { class: c.trueCount ? 'ok' : 'bad' }, c.trueCount), h('td', { class: c.falseCount ? 'ok' : 'bad' }, c.falseCount),
        // kept short on purpose: the sentence below carries the reason, and
        // this column used to wrap one word per line and run off the panel
        h('td', { class: c.independenceCovered ? 'ok' : r ? '' : 'bad', title: c.independenceCovered ? 'the two condition vectors that prove this condition' : r ? `${r.category.toUpperCase()} — ${r.justification}` : 'no pair of recorded vectors isolates this condition' },
          c.independenceCovered ? (c.pair ? h('span', { class: 'pairbtn' }, `✓ ${c.pair[0]} & ${c.pair[1]}`) : '✓ yes') : r ? h('span', { style: 'color:var(--jus)' }, 'justified') : '✗ none'),
        h('td', {}, c.independenceCovered ? null : h('button', { class: 'ghost', style: 'padding:2px 8px', onclick: () => openResolution(key, `Condition C${i + 1} of decision at L${d.line}`, `${fl.display}:${c.line}:${c.col}  ${condSrc(c, i)}`) }, r ? 'Edit' : 'Resolve'))));
    });
    // what the tests actually prove, in words
    const story = h('div', { class: 'mcdc-story' });
    story.append(h('div', { class: 'sub-h' }, 'What the evidence proves'));
    // Only executed records are evidence of anything, and they are condition
    // vectors, not test cases: one test can produce several and several can
    // collapse into one, so nothing here may be stated as a count of tests.
    const execN = d.vectors.filter(v => v.executed !== false).length;
    const need = n + 1;
    const shortfall = execN < need
      ? `MC/DC needs at least ${need} distinct condition vectors for ${n} conditions, and ${execN} ${execN === 1 ? 'was' : 'were'} recorded as executed, so it cannot be met by this evidence however the inputs were chosen.`
      : `${execN} executed condition vectors are recorded, enough in principle for ${n} conditions; the gap is which combinations they cover.`;
    story.append(h('p', { class: 'lede' },
      ok ? `All ${n} conditions are proven: every one has a pair of condition vectors that changes it alone and changes the decision with it.`
         : `${covered} of ${n} conditions proven. ${shortfall}`,
      d.vectors.length !== execN
        ? h('span', { class: 'muted' }, ` (${d.vectors.length - execN} further vector${d.vectors.length - execN === 1 ? '' : 's'} recorded but not executed, and not counted here.)`)
        : null));
    story.append(h('p', { class: 'caveat' },
      'These are LLVM condition vectors, not test cases: one test can produce several, several can collapse into one, and they are deduplicated across merged instantiations. The evidence carries no test identity, so it cannot say how many tests ran.'));
    const ul = h('ul', {});
    d.conditions.forEach((c, i) => {
      const st = mcdcStory(d, i, condSrc(c, i) || `C${i + 1}`);
      const res = resFor(condKey(f.id, d, i));
      ul.append(h('li', { class: st.ok ? 'ok' : res ? 'jus' : 'bad' },
        h('code', {}, condSrc(c, i) || `C${i + 1}`), ' — ',
        st.ok || !res ? st.text : `Not proven, but justified as ${res.category.toUpperCase()}: ${res.justification}`));
    });
    story.append(ul);
    const leftCol = h('div', {}, h('div', { class: 'sub-h' }, 'Conditions'), ct);
    // vectors
    const vt = h('table', { class: 'tt' }, h('tr', {}, h('th', {}, 'vector'), d.conditions.map((_, i) => h('th', {}, `C${i + 1}`)), h('th', {}, 'result')));
    d.vectors.forEach((v, vi) => vt.append(h('tr', { 'data-vec': vi + 1 }, h('td', {}, vi + 1), v.values.map(x => h('td', { class: x == null ? 'D' : x ? 'T' : 'F' }, x == null ? '—' : x ? 'T' : 'F')), h('td', { class: v.result ? 'T' : 'F' }, v.result ? 'T' : 'F'))));
    if (!d.vectors.length) vt.append(h('tr', {}, h('td', { colspan: n + 2, class: 'D' }, 'no test vectors executed')));
    box.append(h('div', { class: 'side' }, leftCol, h('div', {}, h('div', { class: 'sub-h' }, `Condition vectors (${d.vectors.length}) — “—” = not evaluated (short-circuited)`), vt)));
    box.append(story);
    wrap.append(box);
  });
  // plain branches not part of a decision record
  const inDec = new Set(f.decisions.flatMap(d => d.conditions.map(c => `${c.line}:${c.col}`)));
  const plain = f.branches.filter(b => !inDec.has(`${b.line}:${b.col}`));
  if (plain.length) {
    const box = h('div', { class: 'dec' }, h('div', { class: 'dec-head' }, h('span', {}, 'Branch regions without MC/DC records'), h('span', { class: 'muted' }, 'loops, switch cases, single-condition decisions')));
    // Column names follow the first branch: a file is one producer's output,
    // so they do not mix within a function.
    const [w0, w1] = outcomeLabels(plain[0] || {});
    const t = h('table', { class: 'tt' }, h('tr', {}, h('th', {}, 'where'), h('th', {}, 'source'), h('th', {}, w0), h('th', {}, w1),
      h('th', {}, dirKnown(plain[0] || {}) ? '' : h('span', { class: 'muted', title: 'gcov reports a CFG edge, not the polarity of the condition, so butterfly does not say which outcome is the true one' }, 'direction not stated'))));
    for (const b of plain) {
      const [o0, o1] = outcomeKeys(b), [l0, l1] = outcomeLabels(b);
      const kT = branchKey(f.id, b, o0), kF = branchKey(f.id, b, o1);
      t.append(h('tr', {}, h('td', {}, h('a', { onclick: () => selectLine(b.line) }, `L${branchSite(b)}`)), h('td', {}, h('code', {}, srcText(b.line, b.col, b.lineEnd, b.colEnd).trim() || (fl.text[b.line - 1] || '').trim())),
        h('td', { class: b.trueCount ? 'ok' : resFor(kT) ? '' : 'bad' }, b.trueCount, !b.trueCount && resFor(kT) ? ' (justified)' : ''), h('td', { class: b.falseCount ? 'ok' : resFor(kF) ? '' : 'bad' }, b.falseCount, !b.falseCount && resFor(kF) ? ' (justified)' : ''),
        h('td', {}, [!b.trueCount && h('button', { class: 'ghost', style: 'padding:2px 8px', onclick: () => openResolution(kT, `Untaken ${l0} at L${b.line}:${b.col}`, (fl.text[b.line - 1] || '').trim()) }, `${l0}…`), !b.falseCount && h('button', { class: 'ghost', style: 'padding:2px 8px;margin-left:4px', onclick: () => openResolution(kF, `Untaken ${l1} at L${b.line}:${b.col}`, (fl.text[b.line - 1] || '').trim()) }, `${l1}…`)])));
    }
    box.append(t); wrap.append(box);
  }
  body.append(wrap);
}
function hlPair(box, pair, on) { for (const p of pair) { const r = box.querySelector(`tr[data-vec="${p}"]`); if (r) r.classList.toggle('pair', on); } }

/* ---------------- stack ---------------- */
const fmtB = n => n == null ? '∞' : n >= 1024 ? `${(n / 1024).toFixed(1)} KiB` : `${n} B`;
const srcLabel = { stackSizesSection: '.stack_sizes section (compiler-reported)', stackUsageFile: '.su file (compiler-reported)', prologue: 'estimated from the function prologue', unknown: 'unknown' };
function renderStack(body, f) {
  const wrap = h('div', { class: 'stack' });
  if (f.symbolId == null) { wrap.append(h('div', { class: 'empty' }, 'No symbol for this function in the image.')); body.append(wrap); return; }
  const s = sym(f.symbolId); const st = s.stack;
  wrap.append(h('div', { class: 'stat-line' },
    h('span', {}, 'own frame ', h('b', {}, fmtB(s.frame.bytes)), s.frame.dynamic ? h('span', { class: 'bad' }, ' dynamic') : null),
    h('span', {}, 'worst case ', h('b', { class: st.bounded ? 'ok' : 'warn' }, fmtB(st.worstCase)), ' via ', h('b', {}, st.depth), ' frames'),
    h('span', { class: st.bounded ? 'ok' : 'warn' }, st.bounded ? '✓ bounded (direct calls only)' : `⚠ ${st.reason || 'not bounded'}`)),
    h('div', { class: 'note' }, `Frame size ${srcLabel[s.frame.source] || s.frame.source}. Worst case = own frame + deepest chain of direct calls resolved from the object code; indirect calls, recursion, dynamic frames (alloca / VLA), frames of undetermined size, call chains deeper than the analysable depth and totals that overflow are reported, not bounded. Interrupts and alternate stacks are outside this figure entirely. Compiler-reported frames (-fstack-usage / -fstack-size-section / -Zemit-stack-sizes) replace the estimate when present.`));
  // path
  const list = h('div');
  list.append(h('div', { class: 'row hdr' }, h('span', {}, '#'), h('span', {}, 'deepest chain'), h('span', { class: 'b' }, 'frame'), h('span', { class: 'cum' }, 'cumulative'), h('span', {})));
  let cum = 0; const total = st.worstCase || 1;
  st.path.forEach((id, i) => {
    const p = sym(id); cum += p.frame.bytes;
    list.append(h('div', { class: 'row' }, h('span', { class: 'd' }, i), h('span', { class: 'n', title: p.name }, p.functionId != null ? h('a', { onclick: () => navigate(`#/fn/${p.functionId}/stack`) }, p.name) : h('span', { class: 'muted' }, p.name), p.frame.dynamic ? h('span', { class: 'badge red', style: 'margin-left:6px' }, 'dyn') : null, p.indirectCalls ? h('span', { class: 'badge amber', style: 'margin-left:6px' }, `${p.indirectCalls} indirect`) : null),
      h('span', { class: 'b' }, fmtB(p.frame.bytes)), h('span', { class: 'cum' }, fmtB(cum)), h('div', { class: 'bar' }, h('i', { style: `width:${Math.min(100, 100 * cum / total)}%` }))));
  });
  wrap.append(h('div', { class: 'sub-h' }, 'Deepest call chain'), list);
  // direct callees ranked
  if (s.callees.length) {
    const cl = h('div'); cl.append(h('div', { class: 'row hdr' }, h('span', {}), h('span', {}, 'direct callee'), h('span', { class: 'b' }, 'frame'), h('span', { class: 'cum' }, 'its worst'), h('span', {})));
    const cs = s.callees.map(sym).sort((a, b) => (b.stack.worstCase ?? 1e12) - (a.stack.worstCase ?? 1e12));
    for (const c of cs) cl.append(h('div', { class: 'row' }, h('span', {}), h('span', { class: 'n', title: c.name }, c.functionId != null ? h('a', { onclick: () => navigate(`#/fn/${c.functionId}/stack`) }, c.name) : h('span', { class: 'muted' }, c.name)), h('span', { class: 'b' }, fmtB(c.frame.bytes)), h('span', { class: 'cum' }, fmtB(c.stack.worstCase)), h('span', { class: c.stack.bounded ? 'muted' : 'warn', style: 'font-size:11px' }, c.stack.bounded ? '' : c.stack.reason)));
    wrap.append(h('div', { class: 'sub-h' }, `Direct callees (${s.callees.length})${s.indirectCalls ? ` · ${s.indirectCalls} indirect call(s) not included` : ''}`), cl);
  }
  body.append(wrap);
}

/* ---------------- data coupling ---------------- */
const glob = id => S.ev.globals[id];
function accessorList(ids, isFn) { return ids.map((id, i) => [i ? ', ' : null, isFn ? h('a', { class: 'lnk', onclick: () => navigate(`#/fn/${id}/data`) }, fn(id).name) : (sym(id).functionId != null ? h('a', { class: 'lnk', onclick: () => navigate(`#/fn/${sym(id).functionId}/data`) }, sym(id).name) : h('span', { class: 'muted' }, sym(id).name))]); }
function renderData(body, f) {
  const refs = f.dataRefs || []; const fl = file(f.fileId);
  const wrap = h('div', { class: 'stack' });
  if (!refs.length) { wrap.append(h('div', { class: 'empty' }, 'No global data referenced from this function\'s object code (or references go through pointers passed in registers, which the scan cannot attribute).')); body.append(wrap); return; }
  wrap.append(h('div', { class: 'note' }, 'Data coupling (6.4.4.2.c): global data this function reads (R), writes (W) or takes the address of (&), found from address-forming instruction sequences in the object code. Other accessors are listed so the coupling between components is visible.'));
  const t = h('table', { class: 'mat' }, h('tr', {}, h('th', {}, 'global'), h('th', {}, 'section'), h('th', {}, 'size'), h('th', {}, 'this function'), h('th', {}, 'lines'), h('th', {}, 'other readers'), h('th', {}, 'other writers')));
  for (const r of refs) {
    const g = glob(r.globalId);
    const others = ids => ids.filter(id => id !== f.symbolId);
    t.append(h('tr', {}, h('td', { class: 'mono' }, g.name), h('td', { class: 'mono muted' }, g.section), h('td', {}, fmtB(g.size)),
      h('td', {}, r.reads ? h('span', { class: 'rw r' }, `R${r.reads}`) : null, r.writes ? h('span', { class: 'rw w' }, `W${r.writes}`) : null, r.addrTaken ? h('span', { class: 'rw a' }, `&${r.addrTaken}`) : null),
      h('td', {}, r.lines.map((l, i) => [i ? ' ' : null, h('a', { class: 'lnk', onclick: () => selectLine(l) }, `L${l}`)])),
      h('td', {}, accessorList(others(g.readerSymbols), false)), h('td', {}, accessorList(others(g.writerSymbols), false))));
  }
  wrap.append(t); body.append(wrap);
}

/* ---------------- variables (#53) ---------------- */
/// The declaration line and every access, numbered in **static instruction
/// order**: the order the instructions that touch the variable appear in the
/// image, which is address order.
///
/// It is *not* an execution order and must never be described as one.
/// `objectcode.rs` sorts a function's instructions by address and
/// `vars::attribute` scans that list once; nothing here walks a control-flow
/// path and nothing records a trace. So a site inside a loop is listed once
/// however many times it runs, a site inside a branch is listed whether or
/// not it ever executes, and two positions on the same line say which
/// instruction comes first in the image -- not which ran first, nor that
/// either ran at all (review of #53).
///
/// Pure on purpose: a function record and a variable index in, plain data
/// out, so the ordering can be tested without a DOM (ui/vars.test.cjs) and
/// the rendering below stays thin.
///
/// The numbering is still the feature, because a set of lines cannot carry
/// it: in the C fixture `acc` is written *and* read on L46, and `i` runs 45,
/// 45, 46, 45, 45 -- a loop, so the lines are not even monotonic. What it
/// answers is "where in the object code are the touches", which is a
/// question a set of highlighted lines cannot answer at all.
function varAccesses(f, vi) {
  const v = ((f && f.variables) || [])[vi];
  if (!v) return null;
  // 1-based: the reader is being told "this is the 3rd touch", not an index.
  const all = (v.accesses || []).map((a, i) => ({ order: i + 1, line: a.line == null ? null : a.line, access: a.access, addr: a.addr }));
  // An access the line table could not place keeps its position in the
  // ordering and is counted separately. Renumbering around it would claim
  // the variable is touched fewer times than the object code touches it.
  const uses = all.filter(u => u.line != null);
  const byLine = new Map();
  for (const u of uses) { if (!byLine.has(u.line)) byLine.set(u.line, []); byLine.get(u.line).push(u); }
  return {
    name: v.name, kind: v.kind, typeName: v.typeName || null, declLine: v.declLine,
    all, uses, byLine, unlined: all.length - uses.length, unattributed: v.unattributed || null,
  };
}

const ACCESS_WORD = { read: 'read', write: 'written', address: 'address taken' };
const ACCESS_MARK = { read: 'R', write: 'W', address: '&' };
const ACCESS_CLS = { read: 'r', write: 'w', address: 'a' };

function renderVariables(body, f) {
  const vars = f.variables || [];
  // A fresh render replaces every row, so whatever the old ones were under
  // is no longer true -- including after the Variable marks button is
  // clicked, which re-renders from inside a row that still has focus.
  S.varMark = { hover: null, focus: null };
  const wrap = h('div', { class: 'stack' });
  if (!vars.length) {
    // Never "this function has no variables": the attribution only runs on
    // AArch64 frame slots, so on any other target or above -O0 every
    // function lands here. Saying what was not attempted is the honest
    // reading; saying "none" would invert it.
    wrap.append(h('div', { class: 'empty', style: 'text-align:left;max-width:52em' },
      h('div', {}, h('b', {}, 'No parameters or locals were attributed for this function.'), h('br'),
        'This is not a claim that it has none. Variables are read from the compiler\'s own ',
        h('code', {}, 'DW_AT_location'), ' and matched against the disassembly, which currently resolves ',
        'frame slots on AArch64. On another architecture, or above ', h('code', {}, '-O0'),
        ' where the optimiser keeps variables in registers or moves them across PC ranges, nothing is attributed and nothing is guessed.')));
    body.append(wrap);
    return;
  }
  wrap.append(h('div', { class: 'note' },
    'Parameters and locals from DWARF, and every instruction that touches them, in ', h('b', {}, 'static instruction order'),
    ' — the order those instructions appear in the image, by address — never from reading the source text. ',
    // Said here because this is where the numbers are read. The evidence is
    // a single pass over an address-sorted instruction list: it records
    // where the touches are, not when they happen, and a reader who takes
    // "2 R" before "3 W" as proof of a read-before-write has been misled by
    // this panel rather than by the data (review of #53).
    h('b', {}, 'It is not an execution order.'),
    ' Nothing here traces a run: a site inside a loop is listed once however many times it repeats, a site inside a branch is listed whether or not it is ever reached, and two positions on one line say which instruction comes first in the image — not which ran first, nor that either ran at all. ',
    // Which state the marks are in is said here, not left to be discovered.
    // Someone who switched them off a week ago and is now hovering to no
    // effect reads the reason on the line that promised the hover, instead
    // of concluding the accesses below are not real.
    S.varMarks
      ? 'Hover a variable, or one of its accesses, to mark its declaration and each line it touches in the source pane, numbered in that same static order. '
      : h('span', {}, 'Marking in the source pane is ', h('b', {}, 'off'), ' — the ', h('b', {}, 'Variable marks'),
        ' button above the source turns it back on. Nothing below depends on it: every access is listed either way. '),
    'Globals are in the Data tab: their references carry no order, so they cannot be shown this way.'));
  const t = h('table', { class: 'mat vars' },
    h('tr', {}, h('th', {}, 'variable'), h('th', {}, 'kind'), h('th', {}, 'type'), h('th', {}, 'declared'), h('th', {}, 'accesses, in static instruction order (by address)')));
  vars.forEach((v, vi) => {
    const a = varAccesses(f, vi);
    const accCell = h('td', {});
    if (!a.all.length) {
      // The distinction the model file is explicit about: a reason means
      // "we could not tell", and its absence means the variable really is
      // never touched. Those must not read the same.
      accCell.append(a.unattributed
        ? h('span', { class: 'muted' }, 'not attributed — ', a.unattributed)
        : h('span', { class: 'muted' }, 'no instruction in this function touches it'));
    } else {
      for (const u of a.all) {
        accCell.append(h('span', {
          class: 'vb ' + ACCESS_CLS[u.access] + (u.line == null ? ' noline' : ''),
          // "site", and the order named as what it is: the badge is the one
          // place a reader hovers to find out what the number means.
          title: `static access site ${u.order} of ${a.all.length}, in address order and not an execution order: `
            + `${v.name} is ${ACCESS_WORD[u.access] || u.access} at ${hex(u.addr)}`
            + (u.line == null ? ', on an instruction the line table places on no source line' : `, on line ${u.line}`),
          // No hover handlers of their own: a badge is inside the row, so
          // the row's own mouseenter has already lit this variable, and a
          // mouseleave here fired for a move *within* the row (review of #53).
        }, h('i', {}, u.order), ACCESS_MARK[u.access] || '?', u.line == null ? '—' : 'L' + u.line));
      }
    }
    const tr = h('tr', {
      class: 'frow', tabindex: '0',
      // Hover and focus are two independent inputs to one question, so each
      // event records only what it knows and the answer is recomputed from
      // both. The marks then survive everything that happens inside the row
      // -- crossing onto one of its own badges, or the pointer leaving a row
      // the keyboard is still focused in.
      onmouseenter: () => { S.varMark.hover = vi; paintVarMarks(); },
      onmouseleave: () => { if (S.varMark.hover === vi) S.varMark.hover = null; paintVarMarks(); },
      onfocus: () => { S.varMark.focus = vi; paintVarMarks(); },
      onblur: () => { if (S.varMark.focus === vi) S.varMark.focus = null; paintVarMarks(); },
    },
      h('td', { class: 'mono' }, v.name),
      h('td', { class: 'muted' }, v.kind),
      // A type the debug info did not name is said to be unnamed, not blank.
      h('td', { class: 'mono muted' }, a.typeName || h('span', { title: 'the debug info records no type name for this variable' }, '—')),
      h('td', {}, h('a', { class: 'lnk', onclick: () => selectLine(a.declLine) }, 'L' + a.declLine)),
      accCell);
    t.append(tr);
    // Carried whenever it is present, not only when the list is empty:
    // `main`'s arrays in the fixture have accesses *and* a reason, because
    // what is missing is everything reached through the pointer.
    if (a.unattributed && a.all.length) {
      t.append(h('tr', { class: 'vnote' }, h('td', {}), h('td', { colspan: 4 },
        h('span', { class: 'badge amber' }, 'incomplete'), ' ', a.unattributed)));
    }
    if (a.unlined) {
      t.append(h('tr', { class: 'vnote' }, h('td', {}), h('td', { colspan: 4 },
        h('span', { class: 'muted' }, `${a.unlined} of these ${a.all.length} accesses are on instructions the line table places on no source line, so they are counted in the order above but cannot be marked in the source.`))));
    }
  });
  wrap.append(t);
  body.append(wrap);
}

/* ---------------- resolutions ---------------- */
// This server may refuse writes (butterfly serve --read-only, or any
// non-loopback listener without --token). Say so in the dialog rather than
// offering a Save that comes back 403.
function readOnly() { return !!(S.access && S.access.readOnly); }
function readOnlyNotice() {
  return readOnly()
    ? h('div', { class: 'hint', style: 'color:var(--jus)' }, 'This server is serving read-only, so review decisions cannot be recorded here. Open the evidence on a loopback viewer, or ask the operator to restart butterfly with a token (--token-file).')
    : null;
}
function openResolution(key, title, ctx) {
  const target = { variant: S.variant, gen: variantGen };
  const keys = Array.isArray(key) ? key : [key];
  key = keys[0];
  const existing = resFor(key) || {};
  const m = $('#modal'); m.innerHTML = ''; m.classList.remove('hidden');
  const sel = h('select', {}, Object.entries(CATS).map(([k, v]) => h('option', { value: k, selected: existing.category === k }, `${k.toUpperCase()} · ${v}`)));
  const ta = h('textarea', { placeholder: 'Why is this acceptable? Reference the analysis, test case, PSAC section or change request.' }, existing.justification || '');
  const author = h('input', { value: existing.author || localStorage.getItem('sl-author') || '', placeholder: 'reviewer' });
  const date = h('input', { value: existing.date || new Date().toISOString().slice(0, 10) });
  const dlg = h('div', { class: 'dialog' },
    h('h3', {}, readOnly() ? 'Resolution' : existing.key ? 'Edit resolution' : 'Resolve coverage gap'), h('div', { class: 'ctx' }, title, h('br'), ctx, h('br'), h('span', { class: 'muted' }, keys.length > 1 ? `${keys.length} keys` : 'key: ' + key)),
    h('label', {}, 'DO-178C 6.4.4.3 category'), sel,
    h('label', {}, 'Justification'), ta,
    h('div', { class: 'row' }, h('div', {}, h('label', {}, 'Reviewer'), author), h('div', {}, h('label', {}, 'Date'), date)),
    h('div', { class: 'hint' }, 'Stored in the resolutions sidecar, never in the evidence file. Keep the sidecar under configuration management with the evidence it annotates.'),
    readOnlyNotice(),
    h('div', { class: 'actions' },
      existing.key && !readOnly() ? h('button', { class: 'ghost danger', onclick: async () => {
        let ok = true;
        for (const k of keys) {
          if (variantGen !== target.gen || S.variant !== target.variant) { closeModal(); return; }
          if (!await api('/api/resolutions/delete', { key: k }, k === keys[keys.length - 1], target)) { ok = false; break; }
        }
        if (ok) closeModal();
      } }, 'Remove') : null,
      h('button', { class: 'ghost', onclick: closeModal }, readOnly() ? 'Close' : 'Cancel'),
      readOnly() ? null : h('button', { class: 'primary', onclick: async () => {
        if (!ta.value.trim()) { ta.focus(); return; }
        localStorage.setItem('sl-author', author.value);
        // close only when every key persisted; a partial failure keeps the draft
        let ok = true;
        for (const k of keys) {
          if (variantGen !== target.gen || S.variant !== target.variant) { closeModal(); return; }
          if (!await api('/api/resolutions', { key: k, category: sel.value, justification: ta.value.trim(), author: author.value, date: date.value }, k === keys[keys.length - 1], target)) { ok = false; break; }
        }
        if (ok) closeModal();
      } }, 'Save')));
  m.append(dlg); m.onclick = e => { if (e.target === m) closeModal(); };
  ta.focus();
}
function openReview(f) {
  const target = { variant: S.variant, gen: variantGen };
  const key = s2oKey(f); const existing = resFor(key) || {};
  const m = $('#modal'); m.innerHTML = ''; m.classList.remove('hidden');
  const sel = h('select', {}, Object.entries(S2O).map(([k, v]) => h('option', { value: k, selected: existing.category === k }, `${k} · ${v}`)));
  const ta = h('textarea', { placeholder: 'What was compared, which constructs/gaps were examined, and the conclusion. Reference the object listing addresses where useful.' }, existing.justification || '');
  const author = h('input', { value: existing.author || localStorage.getItem('sl-author') || '', placeholder: 'reviewer' });
  const date = h('input', { value: existing.date || new Date().toISOString().slice(0, 10) });
  const o = f.object; const unresolved = unresolvedGaps(f);
  const cons = Object.entries(f.constructs || {}).map(([k, v]) => `${k}×${v}`).join(', ');
  const dlg = h('div', { class: 'dialog' },
    h('h3', {}, readOnly() ? 'Source-to-object review' : existing.key ? 'Edit source-to-object review' : 'Source-to-object traceability review'),
    h('div', { class: 'ctx' }, f.name, h('br'), o ? `${o.instrTotal} instructions · ${fnGaps(f).length} gap(s), ${unresolved} unresolved · constructs: ${cons || 'none'}` : 'no object code', h('br'), h('span', { class: 'muted' }, 'key: ' + key)),
    unresolved ? h('div', { class: 'hint', style: 'color:var(--jus)' }, `${unresolved} gap(s) are still unresolved — "analysed" normally requires every gap to carry a resolution.`) : null,
    h('label', {}, 'Verdict (CAST-12 / DO-178C 6.4.4.2.b)'), sel,
    h('label', {}, 'Analysis notes'), ta,
    h('div', { class: 'row' }, h('div', {}, h('label', {}, 'Reviewer'), author), h('div', {}, h('label', {}, 'Date'), date)),
    h('div', { class: 'hint' }, 'Stored in the resolutions sidecar keyed by function name, so it survives re-ingest and applies across variants of the same sources only if the compiler and options are unchanged (4.4.2.c).'),
    readOnlyNotice(),
    h('div', { class: 'actions' },
      existing.key && !readOnly() ? h('button', { class: 'ghost danger', onclick: async () => {
        if (variantGen !== target.gen || S.variant !== target.variant) { closeModal(); return; }
        if (await api('/api/resolutions/delete', { key }, true, target)) closeModal();
      } }, 'Remove') : null,
      h('button', { class: 'ghost', onclick: closeModal }, readOnly() ? 'Close' : 'Cancel'),
      readOnly() ? null : h('button', { class: 'primary', onclick: async () => {
        if (!ta.value.trim()) { ta.focus(); return; }
        if (variantGen !== target.gen || S.variant !== target.variant) { closeModal(); return; }
        localStorage.setItem('sl-author', author.value);
        // close only if it actually persisted, or a failed save loses the draft
        if (await api('/api/resolutions', { key, category: sel.value, justification: ta.value.trim(), author: author.value, date: date.value }, true, target)) closeModal();
      } }, 'Save')));
  m.append(dlg); m.onclick = e => { if (e.target === m) closeModal(); };
  ta.focus();
}
function closeModal() { const m = $('#modal'); if (m) m.classList.add('hidden'); }
// Returns true only when the write was persisted. Callers must close the
// dialog on true alone, so a failed save keeps the draft open to retry
// (BF-QA-02). A dropped connection is a failure too, not a silent success.
async function api(url, body, rerender = true, target = null) {
  const targetVariant = target ? target.variant : S.variant;
  const targetGen = target ? target.gen : variantGen;
  let r;
  try {
    r = await authFetch(url + vqOf(targetVariant), { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  } catch (e) {
    console.error('butterfly api', e);
    alert('butterfly could not reach the server to save that. Your draft is kept — check the connection and try again.');
    return false;
  }
  if (!r.ok) {
    const text = await r.text().catch(() => '');
    console.error('butterfly api', text);
    // a refused or failed write must not look like a saved one
    alert(r.status === 401 || r.status === 403 ? text : 'butterfly could not save that resolution:\n\n' + text);
    return false;
  }
  const res = await r.json();
  // If a variant switch committed while this write was in flight, do not
  // overwrite the newly active variant's resolutions or redraw its view.
  if (variantGen !== targetGen || S.variant !== targetVariant) return true;
  S.res = new Map((res.entries || []).map(e => [e.key, e]));
  if (rerender) {
    renderTop();
    renderSidebar();
    route();
    if (S.ev && S.ev.changes) loadFreshness(targetVariant, targetGen);
  }
  return true;
}

init().catch(e => {
  // a missing token is an ordinary situation, not a crash: ask for it
  if (e.needsToken) tokenPage(document.body);
  // a 401 inside loadVariant already put the token page up; keep it
  else if (!tokenPageShown) document.body.innerHTML = `<pre style="padding:20px;white-space:pre-wrap;color:#f85149">${esc(e.stack || e)}</pre>`;
});
