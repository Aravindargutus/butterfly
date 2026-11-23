#!/usr/bin/env node
// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
//
// The showcase page builder (#154, design #155).
//
//   node scripts/showcase/build.cjs [--dir docs/showcase] [--single-file [out.html]]
//
// Reads `support/features.toml`, `<dir>/capture.json` and `<dir>/img/` (what
// capture.cjs wrote), and `support/matrix.toml` when it exists; writes
// `<dir>/index.html`. With --single-file it also writes one self-contained
// HTML with every image inlined as base64, so the page can be mailed.
//
// No framework, no CDN, no network: the page has to open from file:// on an
// auditor's laptop. The only script in it is the theme switch.
//
// The builder adds no facts of its own. Every number comes from
// capture.json through a {{placeholder}}, and a placeholder that does not
// resolve stops the build. A section whose capture is missing is rendered as
// missing, with the reason -- never left out, never filled in.
//
// And a section is filled from the numbers recorded BESIDE it, never from one
// object shared by the page: after `scripts/showcase.sh --only V02`, V06 is
// still the run that photographed V06 (#212 review, finding 1).

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const {ROOT, load, loadMatrix, CAPTURE_SCHEMA} = require('./spec.cjs');
const {fill} = require('./facts.cjs');

const {REPO, esc, prose} = require('./markup.cjs');
// The section whose evidence the landing table is read from. It is the section
// the table links to, and its numbers must be that section's, not whatever the
// last partial run happened to rebuild.
const DEMO_SECTION = 'V14';
// The scratch directory capture.cjs creates and works in (its `WORK`).
const WORK_DIR = 'target/showcase';

function args() {
  const a = process.argv.slice(2);
  const opt = {dir: path.join(ROOT, 'docs/showcase'), single: null};
  for (let i = 0; i < a.length; i++) {
    if (a[i] === '--dir') opt.dir = path.resolve(a[++i]);
    else if (a[i] === '--single-file') opt.single = a[i + 1] && !a[i + 1].startsWith('--') ? path.resolve(a[++i]) : true;
    else { console.error(`build: unknown argument ${a[i]}`); process.exit(2); }
  }
  if (opt.single === true) opt.single = path.join(opt.dir, 'butterfly-showcase.html');
  return opt;
}



const kib = n => `${(n / 1024).toFixed(0)} KiB`;
/// Sizes above a mebibyte are unreadable in kibibytes (the footer used to say
/// "6242 KiB"), so say MiB there.
const size = n => (n >= 1024 * 1024 ? `${(n / (1024 * 1024)).toFixed(1)} MiB` : kib(n));
/// No "(s)": the page is read by people who audit for a living.
const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

/// The same rule, applied to the strings the page interpolates rather than
/// counts itself.
///
/// `plural()` only ever saw the numbers the builder works out. A
/// `{{…stack.reason}}` carries a sentence the ingest crates wrote for a
/// terminal, where `1 indirect call(s)` is the house idiom, and it went
/// straight into V07's claim past the rule above (#219). Fix it here rather
/// than in the crates: the reason string is written into every evidence file
/// on disk, and examples/rust-qemu pins numbers read from them.
///
/// Only the counted form is rewritten, because only that form can be resolved
/// without guessing: a number, then up to three plain words, then `(s)` or
/// `(es)` -- `1 indirect call(s)` reads its count from the `1` three words
/// back. Anything else is left for the assertion below to reject, which is
/// the honest outcome: a builder that cannot tell how many there are must not
/// pick a number.
///
/// The number is ANCHORED and its VALUE is read, because the first cut did
/// neither and so picked a number out of anything with a digit in it (#220
/// self-review). `\d+` unanchored matched the `1` of `v1`, of `0x1` and of
/// `0.1`, and the `7` of `A-7`: "the ELF v1 program header(s) could not be
/// read" was republished as "the ELF v1 program header", singular, on an
/// auditor-facing page, at exit 0 -- the safety net below bypassed precisely
/// *by* the stray digit. The lookbehind rejects a digit glued to a word,
/// a decimal point, a comma or a hyphen; the lookahead rejects one that is
/// only part of a longer number. `1,000 gap(s)` and `A-7 objective(s)` came
/// out right before this change by luck (the regex captured the `000` and the
/// `7`); now the first is right because 1000 is parsed and is not 1, and the
/// second stops the build.
///
/// Two things this deliberately does NOT do:
///
///   * a decimal (`0.1 second(s)`) falls through to the throw rather than
///     being pluralised. "0.1 seconds" would be right, but no string any
///     crate writes has one (every `(s)` in crates/ is `{integer} noun(s)`),
///     so the rule stays as narrow as the evidence it has to handle;
///   * a number that is not a count but sits exactly where a count sits is
///     still mis-resolved. "line 1 has gap(s)" -> "line 1 has gap" is
///     structurally identical to "1 indirect call(s)" -> "1 indirect call",
///     and nothing local separates them. No crate string has this shape --
///     `gaps.rs:239` writes "line {line}: {} conditional branch(es)", whose
///     colon keeps the line number out of the window -- so this is a known
///     residual, pinned by a test, not a fixed case.
const counted = text => text.replace(
  /(?<![\w.,-])(\d{1,3}(?:,\d{3})*|\d+)(?![.,]?\d)((?: [A-Za-z][A-Za-z-]*){1,3})\((e?s)\)/g,
  (_, n, words, suffix) => `${n}${words}${Number(n.replace(/,/g, '')) === 1 ? '' : suffix}`);

/// What the guard searches for, which is WIDER than what `counted()` rewrites.
/// Rewriting an irregular plural means guessing at it, and guessing is how
/// this whole class of defect starts; refusing to publish one does not. So
/// `(ies)` and `(S)` -- `crates/cli/src/main.rs:1134` writes "skipped {n}
/// entr(ies)" today -- stop the build instead of going out verbatim at exit
/// 0, which is what they did before (#220 self-review).
const UNCOUNTED = /\((?:e?s|ies)\)/i;

/// Prose the page writes in its own voice: claims, limits and captions.
/// Verbatim command output is NOT prose and never passes through here -- the
/// terminal blocks print what the tool printed, "(s)" and all.
///
/// `raw` is the sentence before `fill()` substituted the capture's values, and
/// `facts` is what filled them. They are only used to write the error: the
/// message used to name the section and nothing else, and its advice ("write
/// the sentence with a number in front of the noun") was un-actionable in the
/// case this will actually fire on, where the text is an evidence string a
/// crate wrote and features.toml only quoted (#220 self-review).
function sentence(text, where, raw = text, facts = null) {
  const out = counted(text);
  const at = out.search(UNCOUNTED);
  if (at < 0) return out;
  const bad = UNCOUNTED.exec(out.slice(at))[0];
  // Which placeholder carried it in, if any: re-fill each one on its own and
  // see which value still holds the form. Nobody can reword an evidence
  // string from support/features.toml, so that case gets different advice.
  const guilty = facts
    ? [...new Set(raw.match(/\{\{[^}]+\}\}/g) || [])].filter(k => {
      try { return UNCOUNTED.test(fill(k, facts, where)); } catch { return false; }
    })
    : [];
  const source = guilty.length
    ? `interpolated by ${guilty.join(' and ')}`
    : 'as written in support/features.toml';
  const advice = guilty.length
    ? `That text is written by a crate and quoted here, not composed by this page: give the number a plain noun in the crate that formats it and re-run scripts/showcase.sh, or quote a field that does not carry a "(s)".`
    : `Write the sentence with a number in front of the noun, or say it without the parenthesis.`;
  throw new Error(`${where}: "…${out.slice(Math.max(0, at - 40), at + bad.length)}" reaches the page ${source}, carrying a "${bad}" the builder cannot resolve into a count. ${advice}`);
}

function render({spec, cap, matrix, inline, dir}) {
  if (cap.schema !== CAPTURE_SCHEMA) {
    throw new Error(`capture.json is ${JSON.stringify(cap.schema)}, and this builder reads ${CAPTURE_SCHEMA}. Re-run scripts/showcase.sh.`);
  }
  const meta = cap.meta;
  const issue = n => `<a href="${REPO}/issues/${n}">#${n}</a>`;

  /* ------------------ the facts a section was captured with ---------------- */
  // Never one object shared by the whole page. `--only` refreshes some
  // sections and leaves the others' records and images alone; filling every
  // section from the refreshed fixture's numbers is how V06 came to say "3 of
  // 3 conditions proven" over the screenshot taken when it was 2 of 3 (#212
  // review, finding 1). Each section is filled from the snapshot the capture
  // run recorded beside it, and from nothing else.
  const factsFor = f => {
    const rec = cap.features[f.id] || {};
    if (!rec.facts) {
      throw new Error(`${f.id}: this capture.json records no facts for the section — it cannot be filled without guessing which run's numbers are its own. Re-run scripts/showcase.sh.`);
    }
    const out = {};
    for (const [key, id] of Object.entries(rec.facts)) {
      if (!(id in (cap.factSets || {}))) throw new Error(`${f.id}: capture.json has no fact snapshot ${id} for \`${key}\`. Re-run scripts/showcase.sh.`);
      out[key] = cap.factSets[id];
    }
    return out;
  };

  /* ------------------- pairing a section to its capture ------------------- */
  // A shot and a terminal block are matched to their capture entry by
  // IDENTITY -- the shot's `file`, the command's `run` -- and never by
  // position. Pairing by array index meant that inserting, removing or
  // reordering an entry in features.toml published one capture's image under
  // another's caption, and printed one command's text beside another's
  // output, with the chip still reading "captured" and the build exiting 0
  // (#212 review). A capture.json written before the identity was recorded is
  // refused rather than guessed at.
  const pairs = new Map();
  const paired = f => {
    if (pairs.has(f.id)) return pairs.get(f.id);
    const rec = cap.features[f.id] || {};
    // Equal entries (the same command run twice in one section) are consumed
    // in order, so they still pair one-to-one.
    const take = (list, key, what) => {
      const used = new Set();
      return want => {
        const i = list.findIndex((x, n) => !used.has(n) && x[key] === want);
        if (i < 0) {
          if (list.some(x => x[key] === undefined)) {
            throw new Error(`${f.id}: this capture.json predates identity matching — a ${what} entry carries no \`${key}\`. Re-run scripts/showcase.sh.`);
          }
          return null;   // declared in features.toml, not in this capture run
        }
        used.add(i);
        return list[i];
      };
    };
    const shotFor = take(rec.shots || [], 'source', 'shot');
    const cmdFor = take(rec.commands || [], 'run', 'command');
    const out = {
      shots: (f.shot || []).map(s => ({s, got: shotFor(s.file)})),
      cmds: (f.cmd || []).map(c => ({c, r: cmdFor(c.run)})),
    };
    pairs.set(f.id, out);
    return out;
  };

  /* --------------------------- per-feature state -------------------------- */
  const state = f => {
    if (f.status === 'pending') return {key: 'pending', label: `not yet demonstrated · ${f.blocked_by.map(n => `#${n}`).join(', ')}`};
    const rec = cap.features[f.id];
    if (!rec) return {key: 'missing', label: 'not captured in this run'};
    if (rec.notCaptured) return {key: 'missing', label: 'not captured on this host'};
    const p = paired(f);
    // A shot or command that features.toml declares and this capture never ran
    // counts as not captured too: a section that gained one without a
    // recapture used to keep its "captured" chip over a red figure (#212 S14).
    const bad = p.shots.filter(x => !x.got || !x.got.file).length + p.cmds.filter(x => !x.r || x.r.failed).length;
    if (bad) return {key: 'partial', label: `captured in part · ${plural(bad, 'item', 'items')} not captured`};
    return {key: 'captured', label: 'captured'};
  };

  const img = (shot, caption) => {
    if (typeof shot.file !== 'string' || !/^[a-z0-9-]+\.(png|jpg)$/.test(shot.file)) {
      throw new Error(`capture.json names an invalid image file ${JSON.stringify(shot.file)}: expected a bare lower-case .png or .jpg filename.`);
    }
    const file = path.join(dir, 'img', shot.file);
    // The page may not claim an image that is not on disk, or one that is not
    // the image the capture recorded. capture.cjs empties img/ before it
    // starts; a run that died in between left the PREVIOUS capture.json beside
    // an empty img/, and the page went out with 42 broken <img> tags and 46
    // "captured" chips (#212 review). The recorded byte count is the check:
    // it does not depend on file timestamps, which a fresh clone does not keep.
    const st = fs.existsSync(file) ? fs.statSync(file) : null;
    if (!st) {
      throw new Error(`${path.relative(ROOT, file)} is named by capture.json and is not on disk: the capture run did not finish. Re-run scripts/showcase.sh.`);
    }
    if (shot.bytes != null && st.size !== shot.bytes) {
      throw new Error(`${path.relative(ROOT, file)} is ${st.size} bytes; capture.json recorded ${shot.bytes}. The images and the capture are from different runs — re-run scripts/showcase.sh.`);
    }
    const src = inline
      ? `data:image/${shot.file.endsWith('.jpg') ? 'jpeg' : 'png'};base64,${fs.readFileSync(file).toString('base64')}`
      : `img/${esc(shot.file)}`;
    const tag = `<img src="${src}" alt="${esc(caption)}" width="${esc(shot.width)}" height="${esc(shot.height)}" loading="lazy" decoding="async">`;
    return inline ? tag : `<a href="${src}">${tag}</a>`;
  };

  /* ------------------------------- a section ------------------------------ */
  const section = f => {
    const st = state(f);
    const rc = cap.features[f.id];
    // `--only` refreshes part of a capture, so a section can come from an
    // earlier commit than the one the lede and the footer name. Say which,
    // in the section itself (#212 review).
    const ownCommit = rc && rc.commit && rc.commit !== meta.commit
      ? `<span class="chip partial">captured at <a href="${REPO}/commit/${esc(rc.commit)}"><code>${esc(rc.commit.slice(0, 12))}</code></a>, not at this page's commit</span>`
      : '';
    const head = `<header class="sec-head"><span class="vid">${f.id}</span><h3 id="${f.id}">${esc(f.title)}</h3>
      <span class="chip ${st.key}">${esc(st.label)}</span>${ownCommit}<a class="iss" href="${REPO}/issues/${f.issue}">section issue #${f.issue}</a></header>`;

    if (f.status === 'pending') {
      return `<section class="feature pending" aria-labelledby="${f.id}">${head}
        <div class="pending-box"><p class="pending-title">Not yet demonstrated — tracked in ${f.blocked_by.map(issue).join(' and ')}</p>
        <p>${prose(f.pending_reason)}</p>
        <p class="muted">What the section will have to prove: ${prose(f.claim)}</p></div></section>`;
    }

    if (!rc || rc.notCaptured) {
      const why = rc ? ['Not captured on this host:', rc.notCaptured] : ['Not part of this capture run:', 'this section was not refreshed by the run that produced the page'];
      return `<section class="feature" aria-labelledby="${f.id}">${head}
        <div class="missing"><b>${esc(why[0])}</b> ${esc(why[1])}</div>
        <p class="muted">What it would show: ${prose(f.claim.replace(/\{\{[^}]+\}\}/g, '…'))}</p></section>`;
    }

    const where = `${f.id}`;
    const facts = factsFor(f);
    // `at` names the FIELD, not just the section: "V13 limit 2" and "V13
    // caption of v13-file" are where a maintainer has to go, and the message
    // used to say only "V13" (#220 self-review).
    const t = (text, at) => prose(sentence(fill(text, facts, at), at, text, facts));
    const local = f.fixtures.filter(n => !spec.fixtures[n].ci);
    const backed = f.fixtures.map(n => `<code>${esc(spec.fixtures[n].dir)}</code>${spec.fixtures[n].synthetic ? ' (hand-written viewer fixture)' : ''}`).join(', ');
    const limits = f.limits.length
      ? `<ul>${f.limits.map((l, i) => `<li>${t(l, `${where} limit ${i + 1}`)}</li>`).join('')}</ul>`
      : '<p>None stated for this section.</p>';

    // Three different things, kept apart the way capture.cjs keeps them apart
    // (capture.cjs:25-35): the HOST could not show it, the TOOL did not do what
    // features.toml says, or this capture run never ran it at all (#212 S13).
    const p = paired(f);
    const figures = p.shots.map(({s, got}) => {
      const capWhere = `${where} caption of ${s.file}`;
      const caption = t(s.caption, capWhere);
      if (got && got.file) {
        // The alt text is the same sentence as the caption, so it gets the
        // same treatment: a screen reader should not be read "(s)" either.
        return `<figure>${img(got, sentence(fill(s.caption, facts, capWhere), capWhere, s.caption, facts))}<figcaption>${caption} <span class="muted">(${esc(got.file)}, ${kib(got.bytes)})</span></figcaption></figure>`;
      }
      const why = !got ? ['Not part of this capture run:', 'this screenshot is written down in support/features.toml and was not taken by the run that produced this page. Re-run scripts/showcase.sh.']
        : got.notCaptured ? ['Not captured on this host:', got.notCaptured]
          : got.failed ? ['Capture failed:', `the capture run failed at this shot — ${got.failed}`]
            : ['Not captured:', 'the capture recorded no image and no reason, which should not happen.'];
      return `<figure class="missing"><div><b>${esc(why[0])}</b> ${prose(why[1])}</div><figcaption>It would show: ${caption}</figcaption></figure>`;
    }).join('\n');

    const terms = p.cmds.filter(x => x.c.show).map(({c, r}) => {
      if (!r) return `<div class="missing"><b>Not part of this capture run:</b> <code>${esc(c.run)}</code> is written down in support/features.toml and was not run by the capture that produced this page.</div>`;
      if (r.output === undefined) return `<div class="missing"><b>Not captured on this host:</b> <code>${esc(c.run)}</code> was not run.</div>`;
      const ok = r.exit === c.exit;
      // r.run, not c.run: what is printed beside the output is the string that
      // produced it. They are equal by construction now that the two are
      // paired by identity, and printing the capture's own copy keeps it so.
      return `<figure class="term"><figcaption>${t(c.caption, `${where} caption of \`${c.run}\``)}</figcaption>
        <pre tabindex="0"><span class="ps">$ </span><span class="cmd">${esc(r.run)}</span>\n${esc(r.output.replace(/\s+$/, ''))}${r.output.trim() ? '\n' : ''}<span class="exit ${ok ? '' : 'bad'}">exit ${esc(r.exit)}${ok ? '' : ` — expected ${c.exit}; ${esc(r.failed || '')}`}</span></pre></figure>`;
    }).join('\n');

    // The reproduce block: the fixtures' commands, then the section's, then
    // how the screenshots' server was started. The first two groups are the
    // strings the capture run executed; nothing here is paraphrased.
    const lines = [];
    // A prerequisite the block cannot install for the reader. V21's Limits
    // named Universal Ctags and its block did not, so the block simply failed
    // on a host with BSD ctags (review of #212).
    if (f.prereq) lines.push(`# ${f.prereq}`);
    for (const n of f.fixtures) {
      lines.push(`# ${spec.fixtures[n].dir}`);
      lines.push(...spec.fixtures[n].build);
    }
    const own = [];
    if ((f.cmd || []).length) own.push('# this section', ...f.cmd.map(c => c.run + (c.exit ? `    # exits ${c.exit}` : '')));
    const served = new Map();
    p.shots.forEach(({s, got: g}) => {
      const got = g || {};
      if (!got.file || s.page) return;
      const ev = [].concat(s.evidence || spec.fixtures[s.fixture || f.fixtures[0]].evidence).join(' ');
      const cmd = `butterfly serve ${ev}${s.token ? ' --token-file target/showcase/token.txt' : ''}${s.resolutions ? ` --resolutions ${s.resolutions}` : ''}`;
      if (!served.has(cmd)) served.set(cmd, []);
      served.get(cmd).push({url: got.url, seed: s.seed || {}});
    });
    for (const [cmd, urls] of served) {
      own.push('# the viewer (Ctrl-C to stop)', cmd);
      // Keyed on the URL *and the seed*, not on the URL alone. Several
      // sections photograph one route in more than one viewer state, and
      // keyed by URL the last shot won and the rest were dropped -- along
      // with the state they were taken in. V09 opens `#/fn/N/variables`
      // three times and the winner was the one seeded `sl-varmarks = "0"`,
      // so the block told the reader to switch OFF the marking the section
      // exists to demonstrate; V01 lost the Gates floors of `v01-overview`
      // and printed a bare line under each of its two servers (#218).
      //
      // The seed is serialised with its keys SORTED, so two shots whose
      // tables were written in a different order are still one line.
      const seedKey = seed => JSON.stringify(Object.entries(seed).sort((a, b) => (a[0] < b[0] ? -1 : 1)));
      const seen = new Map();
      for (const u of urls) {
        const key = `${u.url} ${seedKey(u.seed)}`;
        if (!seen.has(key)) seen.set(key, u);
      }
      // Deduplicating on the seed rather than on the rendered line leaves the
      // other half of the same defect open: two states that the notes below
      // do not distinguish would print the same sentence twice, which is the
      // bare-duplicate-line half of #218 arriving by another route. The
      // builder does not guess which state the reader wanted -- it stops, and
      // says which note is missing.
      const lines = new Set();
      for (const u of seen.values()) {
        const notes = [];
        // Every overlay, not just Projected: the viewer opens on "No
        // coverage", so a shot seeded `measured` -- V28's second Object-tab
        // shot, at the same route as its first -- reproduced as a bare line
        // that silently showed the reader the wrong overlay. The names are
        // the viewer's own button labels (ui/app.js, and the BUTTON map in
        // ui/browser/checks/footer.cjs).
        const MODE = {projected: 'Projected', measured: 'Measured', plain: 'No coverage'};
        if (MODE[u.seed['sl-mode']]) notes.push(`select ${MODE[u.seed['sl-mode']]}`);
        if (u.seed['sl-theme'] === 'light') notes.push('light theme');
        if (u.seed['sl-gates']) notes.push(`Gates: ${Object.entries(JSON.parse(u.seed['sl-gates'])).map(([k, v]) => `${k.replace(/^m(in|ax)_/, '').replace(/_/g, ' ')} ${v}`).join(', ')}`);
        if (u.seed['sl-variant']) notes.push(`variant ${u.seed['sl-variant']}`);
        if (u.seed['sl-fold']) notes.push(`show: ${u.seed['sl-fold']} only`);
        if (u.seed['sl-fnhdr']) notes.push('function headers on');
        if (u.seed['sl-varmarks'] === '0') notes.push('Variable marks off');
        const line = `#   open ${u.url}${notes.length ? `   (${notes.join('; ')})` : ''}`;
        if (lines.has(line)) {
          throw new Error(`${f.id}: two screenshots of ${u.url} were taken in different viewer states, and the reproduce block renders both as "${line.trim()}". Give the state that differs a note in build.cjs, so the block says which shot each line reproduces.`);
        }
        lines.add(line);
        own.push(line);
      }
    }
    // `pages` was not deduplicated the way `served` is, so a section with
    // three shots of one report printed the same "# open ..." line three times.
    for (const line of new Set(p.shots.filter(x => x.s.page).map(x => `#   open ${x.s.page} in a browser`))) own.push(line);

    // Several sections write into target/showcase, and `butterfly requirements`
    // and `butterfly report` do not create the parent directory. The capture
    // harness makes it before it runs anything (capture.cjs `sh()`), and never
    // printed the step, so three blocks failed in a fresh clone while the page
    // showed exit 0 (#212 review). Emit the harness's real step.
    if (own.join('\n').includes(WORK_DIR)) {
      lines.push(`# the capture harness creates its scratch directory before it runs anything`, `mkdir -p ${WORK_DIR}`);
    }
    lines.push(...own);

    return `<section class="feature" aria-labelledby="${f.id}">${head}
      <div class="claim-row">
        <div class="claim"><p>${t(f.claim, `${where} claim`)}</p>
          <p class="backed">Backed by ${backed}.${local.length ? ` <span class="local">Verified locally; not built in hosted CI: ${local.map(n => esc(spec.fixtures[n].dir)).join(', ')}.</span>` : ''}</p></div>
        <aside class="limits" aria-label="Limits of ${esc(f.title)}"><h4>Limits</h4>${limits}</aside>
      </div>
      ${figures}
      ${terms}
      <div class="repro"><h4>Reproduce it</h4>
        <p class="muted">From the repository root, with <code>butterfly</code> on your <code>PATH</code> (<code>cargo install --path crates/cli</code>, or <code>export PATH="$PWD/target/debug:$PATH"</code> after <code>cargo build</code>). The build and section commands are the ones the capture run executed. The viewer was started by the browser-check harness on a free <code>--listen</code> port, with a sidecar of its own where the <code>butterfly serve</code> line names none and a token file of its own where the line names one. Function and file ids in a URL are assigned at ingest; if yours differ, pick the function in the sidebar.</p>
        <pre tabindex="0">${lines.map(l => (l.startsWith('#') ? `<span class="cm">${esc(l)}</span>` : esc(l))).join('\n')}</pre></div>
    </section>`;
  };

  /* ------------------------------- landing -------------------------------- */
  // The table is V14's evidence, and it says so. Read from V14's own snapshot:
  // after a partial run that rebuilt `c-variants` for another section, the
  // page would otherwise print numbers V14 never showed.
  const demoFeature = spec.features.find(f => f.id === DEMO_SECTION);
  const demoLive = demoFeature && cap.features[demoFeature.id] && !cap.features[demoFeature.id].notCaptured;
  const v = demoLive ? factsFor(demoFeature)['c-variants'] : null;
  let demo;
  if (v && v.variant && v.variant.O0 && v.variant.O2 && v.variant.O0.fn.clamp_altitude) {
    const kindName = k => k.replace(/([A-Z])/g, '-$1').toLowerCase();
    const row = n => {
      const fn = v.variant[n].fn.clamp_altitude;
      const kinds = Object.entries(fn.gapKinds).map(([k, c]) => `${c} ${kindName(k)}`).join(', ');
      return `<tr><th scope="row"><code>-${n}</code></th><td>${fn.linesCovered}/${fn.linesTotal}</td><td class="n">${fn.blocks}</td><td class="n">${fn.instructions}</td><td><span class="n">${fn.gaps}</span>${kinds ? ` <span class="muted">(${esc(kinds)})</span>` : ''}</td></tr>`;
    };
    const same = v.sourceCoverageIdentical;
    const o0 = v.variant.O0.fn.clamp_altitude, o2 = v.variant.O2.fn.clamp_altitude;
    // The provenance sentence has to survive `--only`, which the rest of the
    // partial machinery does and this line did not (#220 self-review). After
    // `--only V16`, mergePartial carries V14's factSet forward untouched, so
    // these numbers are the earlier run's -- and "this page's capture run"
    // means the LAST run everywhere else on the page ("Not part of this
    // capture run"). Unlike a section, the hero paragraph carries no
    // "captured at ..." chip to correct it, and it sits two paragraphs under
    // a lede saying the page was not captured in one pass.
    const demoLink = `<a href="#${DEMO_SECTION}">${DEMO_SECTION}</a>`;
    const demoCommit = (cap.features[demoFeature.id] || {}).commit || meta.commit;
    const refreshed = !meta.partial || meta.partial.includes(DEMO_SECTION);
    const provenance = refreshed
      ? `Read from the evidence of this page's capture run (${demoLink}).`
      : `Read from the evidence ${demoLink} was captured with${demoCommit !== meta.commit ? `, at <a href="${REPO}/commit/${esc(demoCommit)}"><code>${esc(demoCommit.slice(0, 12))}</code></a>` : ''}: this page's last capture run refreshed only ${meta.partial.map(esc).join(', ')}, so these numbers come from the run before it.`;
    demo = `<div class="demo"><h3>One function, four builds</h3>
      <p><code>clamp_altitude</code> in <code>examples/c</code>, two <code>if</code> statements, at four optimisation levels. ${provenance}</p>
      <div class="scroll"><table><thead><tr><th>build</th><th>source lines covered</th><th>object blocks</th><th>instructions</th><th>gaps</th></tr></thead>
      <tbody>${Object.keys(v.variant).map(row).join('')}</tbody></table></div>
      <p>${same
        ? 'The source coverage is the same in every row: it is one coverage export, and the per-line and per-branch counts of the variants compare equal.'
        // Backticks, not quotes: this was a single-quoted string, so the
        // section name was dead literal text -- in the failure branch of a
        // correctness warning, where it is the one thing the reader needs
        // (#220 self-review).
        : `<b>The source coverage differs between the variants in this capture</b>, which the generator did not expect; read ${demoLink} before relying on this table.`}
      ${o2.blocks < o0.blocks && o2.gaps > o0.gaps
        ? `The object code is not: the ${o0.blocks} blocks of <code>-O0</code> are ${o2.blocks} at <code>-O2</code>, where the source decisions no longer have a conditional branch that can be located in the object code. butterfly reports each as a gap for a reviewer; it does not decide whether it matters.`
        : 'On this host the optimiser left the shape of the object code as it was, so the difference this table usually shows is not visible here.'}</p>
      <p class="muted">Exploratory builds, made to understand the optimiser: never certification evidence.</p></div>`;
  } else {
    demo = `<div class="demo"><div class="missing"><b>Not captured on this host:</b> the optimisation-level comparison (${esc(cap.fixtures['c-variants']?.reason || 'fixture c-variants was not part of this run')}).</div></div>`;
  }

  const groups = spec.groups.map(g => ({...g, features: spec.features.filter(f => f.group === g.id)})).filter(g => g.features.length);
  const counts = {captured: 0, partial: 0, missing: 0, pending: 0};
  for (const f of spec.features) counts[state(f).key]++;
  // Sections count sections; the map used to say "0 not captured on this host"
  // over a section that renders exactly that for one of its screenshots, so
  // the items are counted too (#212 S15).
  let itemsMissing = 0;
  for (const f of spec.features.filter(x => x.status === 'captured' && cap.features[x.id] && !cap.features[x.id].notCaptured)) {
    const pp = paired(f);
    itemsMissing += pp.shots.filter(x => !x.got || !x.got.file).length + pp.cmds.filter(x => !x.r || x.r.failed).length;
  }

  /* ------------------ was the whole page captured at once? ---------------- */
  // `--only` refreshes some sections and leaves the rest from an earlier run,
  // at an earlier commit. The lede and the footer used to claim the fresh
  // commit for all of it (#212 B5).
  const liveFeatures = spec.features.filter(f => cap.features[f.id] && !cap.features[f.id].notCaptured);
  const older = liveFeatures.filter(f => (cap.features[f.id].commit || meta.commit) !== meta.commit);
  const mixed = older.length > 0;
  const partialNote = (meta.partial || mixed)
    ? `<p class="missing partial-note"><b>Partial capture.</b> The last run refreshed ${meta.partial ? `only ${meta.partial.map(esc).join(', ')}` : 'part of this page'}; the rest comes from an earlier run${
      mixed ? `, and ${plural(older.length, 'section carries', 'sections carry')} the commit it was captured at beside its title` : ''}. Run <code>scripts/showcase.sh</code> with no arguments for a page captured in one pass, at one commit.</p>`
    : '';
  const ledeCapture = mixed
    ? `Everything below was captured by a script from this repository, but not all of it at the same commit: ${plural(liveFeatures.length - older.length, 'section is', 'sections are')} from <code>${esc(meta.commitShort)}</code> and ${plural(older.length, 'section is', 'sections are')} from an earlier one, named beside the section's own title${meta.modified ? `, with ${plural(meta.modified, 'uncommitted change', 'uncommitted changes')} in the work tree` : ''}.`
    : `Everything below was captured by a script from commit <code>${esc(meta.commitShort)}</code>${meta.modified ? `, with ${plural(meta.modified, 'uncommitted change', 'uncommitted changes')} in the work tree` : ''}.`;

  const map = groups.map(g => `<div class="map-group"><h3>${esc(g.title)}</h3><ul>${g.features.map(f => {
    const st = state(f);
    return `<li><a href="#${f.id}"><span class="vid">${f.id}</span> ${esc(f.title)}</a> <span class="chip ${st.key}">${esc(st.key === 'pending' ? f.blocked_by.map(n => `#${n}`).join(' ') : st.key === 'captured' ? 'captured' : st.key === 'partial' ? 'in part' : 'not captured')}</span></li>`;
  }).join('')}</ul></div>`).join('');

  /* -------------------------------- matrix -------------------------------- */
  let matrixHtml;
  let notSupported = [];
  if (!matrix) {
    matrixHtml = `<div class="missing"><b>Matrix not yet merged (${issue(151)}).</b> The language, architecture, object-format and toolchain matrix is a data file with a checker behind it, <code>support/matrix.toml</code>. It was not in the checkout this page was built from, so this page claims nothing about language support yet. It is rendered here as soon as it exists.</div>`;
  } else {
    notSupported = matrix.filter(r => r.state === 'not-supported');
    const label = {supported: 'Supported', caveats: 'Handled, with caveats', 'not-supported': 'Not supported'};
    const chip = {supported: 'captured', caveats: 'partial', 'not-supported': 'pending'};
    const count = st => matrix.filter(r => r.state === st).length;
    const uniq = a => [...new Set(a)];
    const rowHtml = r => `<tr><th scope="row">${prose(r.name || r.id || '')}<br><span class="vid">${esc(r.id || '')}</span></th>
      <td><span class="chip ${chip[r.state] || 'missing'}">${esc(label[r.state] || r.state)}</span></td>
      <td>${prose(r.caveat || r.reason || '')}${[].concat(r.tracking || []).length ? ` <span class="muted">Tracked in ${[].concat(r.tracking).map(issue).join(', ')}.</span>` : ''}</td>
      <td>${r.example ? `<code>${esc(r.example)}</code>` : '<span class="muted">none</span>'}${r.evidence ? `<br><span class="muted">${esc(r.evidence)}</span>` : ''}</td>
      <td>${r.proof ? `<code>${esc(r.proof)}</code>` : ''}</td></tr>`;
    matrixHtml = `<p>${matrix.length} rows from <code>support/matrix.toml</code>, which a test checks textually against the examples, evidence lines and proof tests they name (${issue(151)}): ${count('supported')} supported, ${count('caveats')} handled with caveats, ${count('not-supported')} not supported. The third column is the point of the table.</p>`
      + uniq(matrix.map(r => r.area || 'other')).map(a => {
        const inArea = matrix.filter(r => (r.area || 'other') === a);
        return `<h3 class="area">${esc(a)}</h3>` + uniq(inArea.map(r => r.group || '')).map(g => `${g ? `<h4 class="mgroup">${esc(g)}</h4>` : ''}<div class="scroll"><table class="matrix"><thead><tr><th>what</th><th>state</th><th>caveat, or the reason it is not supported</th><th>example</th><th>proof</th></tr></thead><tbody>${
          inArea.filter(r => (r.group || '') === g).map(rowHtml).join('')}</tbody></table></div>`).join('');
      }).join('');
  }

  const notClaimed = `<ul class="not-claimed">${spec.notClaimed.map(n => `<li><p>${prose(n.text)}</p><p class="muted">Where the project says so: ${prose(n.source)}</p></li>`).join('')}${
    notSupported.map(r => `<li><p><b>${prose(r.name || r.id)}</b> is not supported. ${prose(r.reason || '')}</p><p class="muted">support/matrix.toml, row ${esc(r.id || '')}</p></li>`).join('')
  }</ul>${matrix ? '' : `<p class="muted">The matrix's not-supported rows will be listed here as well once ${issue(151)} has merged.</p>`}`;

  /* -------------------------------- footer -------------------------------- */
  const shots = liveFeatures.flatMap(f => paired(f).shots.map(x => x.got)).filter(s => s && s.file);
  const totalBytes = shots.reduce((n, s) => n + s.bytes, 0);
  const biggest = shots.reduce((m, s) => (s.bytes > (m?.bytes || 0) ? s : m), null);
  const reused = Object.entries(cap.fixtures).filter(([, x]) => x.reused).map(([n]) => n);
  const qualDoc = inline ? `${REPO}/blob/${meta.commit}/docs/qualification/DO-330-assessment.md` : '../qualification/DO-330-assessment.md';
  const tools = Object.entries(meta.tools).map(([k, val]) => `<div><dt>${esc(k)}</dt><dd>${val ? esc(val) : '<span class="muted">not installed</span>'}</dd></div>`).join('');

  const css = fs.readFileSync(path.join(__dirname, 'page.css'), 'utf8');
  return `<!doctype html>
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- GENERATED by scripts/showcase/build.cjs from support/features.toml and docs/showcase/capture.json. Do not edit: run scripts/showcase.sh. -->
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>butterfly — what it does, shown and reproducible</title>
<style>${css}</style>
</head>
<body>
<a class="skip" href="#main">Skip to content</a>
<header class="top"><div class="wrap"><span class="brand">butterfly</span>
  <nav aria-label="Page"><a href="#map">Features</a><a href="#not-claimed">What we do not claim</a><a href="#qualification">Qualification</a><a href="#made">How this page is made</a></nav>
  <button id="theme" type="button" aria-label="Switch between light and dark">◐</button></div></header>
<main id="main" class="wrap">

<section class="landing" aria-labelledby="h-landing">
  <p class="kicker">DO-178C structural coverage and object-code evidence, in a browser</p>
  <h1 id="h-landing">Coverage tools measure the source. At Level A you answer for the object code.</h1>
  <div class="landing-grid">
    <div>
      <p>DO-178C Annex A Table A-7 asks a Level A project for statement, decision and MC/DC coverage, for data and control coupling, and — where the compiler generates object code that is not directly traceable to the source — for additional verification of that code (§6.4.4.2.b). Coverage tools report on the source; relating that to the object code is separate work.</p>
      <p><b>butterfly reads the coverage export of a coverage-instrumented build</b> together with <b>the non-instrumented binary built from the same sources</b> and the compiler's stack-usage files for that binary, and writes one evidence file. <b>It shows each function's object code beside its source</b>, with coverage carried across through the line table, and inside an analysed function records each place its checks find that the two do not match as a gap for a reviewer. <b>It turns the same evidence into gates for CI</b>, a requirements-to-test trace and a change-impact list.</p>
      <p class="muted">${ledeCapture} Every section names its example, prints the commands that reproduce it, and states its limits beside the claim. ${counts.captured + counts.partial} of ${spec.features.length} sections are demonstrated today${counts.pending + counts.missing ? `; the other ${counts.pending + counts.missing} say what they are waiting for` : ''}.</p>
      <p class="position"><b>butterfly is not qualified</b> (DO-330), and nothing here is a claim of certification or compliance. <a href="#qualification">The qualification position</a> · <a href="#not-claimed">What we do not claim</a></p>
    </div>
    ${demo}
  </div>
  ${partialNote}
</section>

<section id="map" aria-labelledby="h-map"><h2 id="h-map">Feature map</h2>
  <p class="muted">${counts.captured} captured · ${counts.partial} captured in part · ${counts.missing} not captured on this host · ${counts.pending} not yet demonstrated, each with the issue that blocks it. Counted by section; inside them, ${plural(itemsMissing, 'screenshot or command output was', 'screenshots or command outputs were')} not captured.</p>
  <div class="map">${map}</div></section>

${groups.map(g => `<section class="group" aria-labelledby="g-${g.id}"><h2 id="g-${g.id}">${esc(g.title)}</h2>${g.features.map(section).join('\n')}</section>`).join('\n')}

<section id="matrix" aria-labelledby="h-matrix"><h2 id="h-matrix">Language and toolchain matrix</h2>${matrixHtml}</section>

<section id="not-claimed" aria-labelledby="h-nc"><h2 id="h-nc">What we do not claim</h2>
  <p>A tool for certification work is only as useful as it is exact about where it stops. These are the project's own statements of where butterfly stops.</p>
  ${notClaimed}</section>

<section id="qualification" aria-labelledby="h-q"><h2 id="h-q">Qualification position</h2>
  <p>The criteria and the level are the <b>applicant's</b> determination for their own intended use, recorded in the PSAC; they are not properties of the tool. A project that claims credit only for the objectives butterfly automates — Table A-7 objectives 5, 6 and 7, and objective 8 — would classify it as a DO-178C §12.2.2 <b>Criteria 3</b> tool, which <b>DO-178C Table 12-1</b> puts at <b>TQL-5</b> for Levels A to D. Claiming that some other activity is reduced because butterfly exists — dropping the human source-to-object review on the strength of the projection, or treating <code>butterfly check</code>'s exit status as the verification of the coverage objectives — is a Criteria 2 use, and Criteria 2 is TQL-4 at Levels A and B.</p>
  <p>butterfly is <b>not qualified</b>: no DO-330 life-cycle data exists for it today. It does not perform the source-to-object correspondence review (objective 9) — it presents instruction-level evidence for a human reviewer and records the verdict — and the exploratory-build function (<code>butterfly build</code>) is excluded from any set of qualified functions, because its output is object code. The project's own assessment of what a TQL-5 package would take, and of the uses that would move it to Criteria 2, is <a href="${qualDoc}">docs/qualification/DO-330-assessment.md</a>.</p></section>

<section id="made" aria-labelledby="h-made"><h2 id="h-made">How this page is made</h2>
  <p><code>scripts/showcase.sh</code> builds butterfly from the checkout, runs each example's build commands and each section's terminal commands verbatim from the repository root, keeps their real output and exit status, serves each example with the browser-check harness, which starts <code>butterfly serve</code> on a free port with its own scratch files rather than running the printed <code>butterfly serve</code> line, drives the viewer with Playwright and takes the screenshots. A command that ends with a status other than the one written in <code>support/features.toml</code>, or a screenshot that does not show the strings its section lists there as the point of it (its <code>expect</code>), fails the run. Nothing checks that a caption's every assertion is among those strings — that is a reading, and it is what the claims audit (${issue(200)}) is for. What the host cannot show is rendered as not captured, with the reason.</p>
  <p>Three things are altered, and only these: in terminal output the repository root, the temp directory and the home directory are replaced by <code>&lt;repo&gt;</code>, <code>&lt;tmp&gt;</code> and <code>~</code>, and any ANSI colour a tool emitted despite <code>NO_COLOR</code> is stripped; the viewer is served on a free port by the harness, and URLs are printed with the default <code>127.0.0.1:8787</code> and without the harness's own <code>?shot=</code> parameter; screenshots may be cropped to a pane, and one larger than the page's image budget is taken again as a JPEG. Text typed into dialogs by the script says that it is example text. Screenshots are never edited.</p>
  ${reused.length ? `<p class="missing"><b>This page was built with --reuse-fixtures</b> for ${reused.map(esc).join(', ')}: those examples were not rebuilt for this run.</p>` : ''}
  ${meta.partial ? `<p class="missing"><b>Partial capture:</b> the last run refreshed only ${meta.partial.map(esc).join(', ')}; the other sections come from an earlier run.</p>` : ''}
</section>
</main>

<footer class="foot"><div class="wrap">
  <dl class="meta">
    <div><dt>commit</dt><dd><a href="${REPO}/commit/${esc(meta.commit)}"><code>${esc(meta.commit)}</code></a>${meta.modified ? ` <b class="warn">plus ${plural(meta.modified, 'uncommitted change', 'uncommitted changes')} in the work tree</b>` : ''} <span class="muted">(${esc(meta.commitDate)}, branch ${esc(meta.branch)})</span>${
      // `plural()` covered the subject and left the second clause singular,
      // so this rendered "23 sections were captured at an earlier commit and
      // says so beside its title" -- a subject-verb disagreement in the
      // page's own provenance warning, in a PR about "(s)". The `mixed`
      // branch had never rendered before this branch, so nothing pinned it
      // (#212 B5, #220 self-review); there is a test now.
      mixed ? ` <b class="warn">${older.length === 1
        ? '1 section was captured at an earlier commit and says so beside its title'
        : `${older.length} sections were captured at an earlier commit and say so beside their titles`}</b>` : ''}</dd></div>
    <div><dt>captured</dt><dd>${esc(meta.capturedAt)}${meta.partial ? ` · a partial run: ${meta.partial.map(esc).join(', ')}` : ''} · locally, not by CI</dd></div>
    <div><dt>host</dt><dd>${esc(meta.os)} · ${esc(meta.arch)}</dd></div>
    ${tools}
    <div><dt>images</dt><dd>${plural(shots.length, 'screenshot', 'screenshots')}, ${size(totalBytes)} in total${biggest ? `; the largest is ${esc(biggest.file)} at ${size(biggest.bytes)}` : ''}</dd></div>
  </dl>
  <p class="muted">butterfly is dual-licensed MIT OR Apache-2.0. This page is generated; to change it, change <code>support/features.toml</code>, <code>support/matrix.toml</code> or <code>scripts/showcase/build.cjs</code>, and run <code>scripts/showcase.sh</code>.</p>
</div></footer>
<script>
(function () {
  var root = document.documentElement, key = 'bf-showcase-theme', saved = null;
  try { saved = localStorage.getItem(key); } catch (e) {}
  if (saved === 'light' || saved === 'dark') root.setAttribute('data-theme', saved);
  document.getElementById('theme').addEventListener('click', function () {
    var dark = root.getAttribute('data-theme')
      ? root.getAttribute('data-theme') === 'dark'
      : window.matchMedia('(prefers-color-scheme: dark)').matches;
    var next = dark ? 'light' : 'dark';
    root.setAttribute('data-theme', next);
    try { localStorage.setItem(key, next); } catch (e) {}
  });
})();
</script>
</body>
</html>
`;
}

function main() {
  const opt = args();
  const spec = load();
  const capPath = path.join(opt.dir, 'capture.json');
  if (!fs.existsSync(capPath)) {
    console.error(`build: ${path.relative(ROOT, capPath)} does not exist. Run scripts/showcase/capture.cjs first: the page is built from a capture, never without one.`);
    process.exit(2);
  }
  const cap = JSON.parse(fs.readFileSync(capPath, 'utf8'));
  const matrix = loadMatrix();
  const out = path.join(opt.dir, 'index.html');
  // Both pages are rendered BEFORE either is written. index.html used to be on
  // disk already when the single-file pass threw, so a build that failed left
  // a broken page committed and verify.cjs never ran over it (#212 B2).
  const html = render({spec, cap, matrix, inline: false, dir: opt.dir});
  const single = opt.single ? render({spec, cap, matrix, inline: true, dir: opt.dir}) : null;
  // An image no section names. `--only` prunes nothing, so a renamed shot --
  // or one that switched from PNG to JPEG because it grew past the budget --
  // leaves its predecessor in img/ to be committed for ever.
  const named = new Set(Object.values(cap.features).flatMap(r => (r.shots || []).map(s => s.file)).filter(Boolean));
  const orphans = fs.readdirSync(path.join(opt.dir, 'img')).filter(n => !named.has(n));
  if (orphans.length) {
    throw new Error(`${plural(orphans.length, 'file', 'files')} in ${path.relative(ROOT, path.join(opt.dir, 'img'))} named by no section: ${orphans.join(', ')}. A full \`scripts/showcase.sh\` (no --only) prunes them; delete them if you are iterating.`);
  }
  // A placeholder the builder itself wrote past `fill()` -- build.cjs has
  // several strings that never go through it -- would reach the page as
  // literal braces. Cheap last look over what is actually rendered.
  for (const [name, text] of [[out, html], [opt.single, single]]) {
    const at = text ? text.indexOf('{{') : -1;
    if (at >= 0) throw new Error(`${path.relative(ROOT, name)} still contains an unfilled placeholder: ${JSON.stringify(text.slice(at, at + 60))}`);
  }
  fs.writeFileSync(out, html);
  console.error(`build: wrote ${path.relative(ROOT, out)} (${size(fs.statSync(out).size)})${matrix ? `, matrix: ${matrix.length} rows` : ', matrix not yet merged (#151)'}`);
  if (opt.single) {
    fs.writeFileSync(opt.single, single);
    console.error(`build: wrote ${path.relative(ROOT, opt.single)} (${size(fs.statSync(opt.single).size)}, self-contained)`);
  }
}

if (require.main === module) main();
module.exports = {render, prose};
