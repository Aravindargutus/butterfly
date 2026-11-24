// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Self-contained HTML report: the evidence rendered as a document a reviewer
//! can file as Software Verification Results (DO-178C §11.14). No scripts, no
//! external assets; prints cleanly.

use butterfly_model::*;
use std::collections::BTreeMap;
use std::fmt::Write;

/// HTML-escape. Covers `'` and `/` as well as the usual four so the result is
/// safe in attribute context too, not only in text.
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}
fn pct(a: usize, b: usize) -> String {
    if b == 0 {
        "—".into()
    } else {
        format!("{:.1}%", 100.0 * a as f64 / b as f64)
    }
}
fn slug(k: GapKind) -> &'static str {
    match k {
        GapKind::NoSourceLine => "no-source-line",
        GapKind::LineZero => "line-zero",
        GapKind::ExtraObjectBranch => "extra-object-branch",
        GapKind::PredicatedDecision => "predicated-decision",
        GapKind::HelperCall => "helper-call",
        GapKind::IndirectBranch => "indirect-branch",
        GapKind::NotInstrumented => "not-instrumented",
        GapKind::SharedLine => "shared-line",
        GapKind::FoldedCall => "folded-call",
        GapKind::NoCoverageRecord => "no-coverage-record",
        GapKind::UncountedVariant => "uncounted-variant",
    }
}

const CSS: &str = r#"
body{font-family:-apple-system,Segoe UI,Helvetica,Arial,sans-serif;font-size:12px;color:#1f2933;margin:32px auto;max-width:1100px;padding:0 20px}
h1{font-size:20px;margin:0 0 4px}h2{font-size:14px;margin:26px 0 8px;border-bottom:1px solid #ccd;padding-bottom:4px}h3{font-size:12px;margin:14px 0 6px}
table{border-collapse:collapse;width:100%;margin:6px 0}th,td{border:1px solid #d7dce2;padding:4px 6px;text-align:left;vertical-align:top}th{background:#eef1f5;font-weight:600}
code,.mono{font-family:ui-monospace,Menlo,Consolas,monospace;font-size:11px}.muted{color:#6b7280}.ok{color:#1a7f37}.bad{color:#cf222e}.warn{color:#9a6700}
.tiles{display:flex;flex-wrap:wrap;gap:8px}.tile{border:1px solid #d7dce2;border-radius:6px;padding:8px 12px;min-width:150px}.tile b{font-size:18px;display:block}
.banner{border-left:4px solid #9a6700;background:#fff8e1;padding:8px 12px;margin:10px 0}.banner.bad{border-color:#cf222e;background:#fdecec}
tr.reqtext td{border-top:none;padding-top:0;color:#333;font-size:12px;line-height:1.5}
.warn-tag{display:inline-block;margin-left:6px;padding:1px 6px;border:1px solid #9a6700;border-radius:3px;color:#9a6700;font-size:11px}
.src-tag{display:inline-block;margin-left:6px;font-family:ui-monospace,Menlo,monospace;font-size:11px;color:#555}
@media print{body{margin:0}h2{page-break-after:avoid}tr{page-break-inside:avoid}}
"#;

/// The Software Verification Results report.
///
/// `reqs` is the requirement text sidecar, when one was produced. The report
/// is read away from the tool -- it is the artifact a Compliance
/// Verification Engineer signs against -- so unlike the viewer it cannot
/// fetch a statement on demand and must carry what it shows.
///
/// It carries only the requirements this evidence actually names, which is
/// bounded by the code under analysis rather than by the programme's
/// universe: a component implementing forty requirements embeds forty, not
/// the three thousand its project has.
/// Which document a statement was quoted from, when more than one was read.
///
/// Silent for a single document: the section already said it once, and
/// repeating it on every row is noise. With several it is the only way to
/// know which baseline backs a given sentence.
fn quoted_from(reqs: Option<&butterfly_model::reqtext::Requirements>, t: &butterfly_model::reqtext::Requirement) -> String {
    let Some(q) = reqs else { return String::new() };
    if q.sources.len() < 2 {
        return String::new();
    }
    match q.sources.get(t.source) {
        Some(src) => format!(" <span class=src-tag>{} <span class=muted>{}</span></span>", esc(&src.path), esc(&src.sha256.chars().take(12).collect::<String>())),
        // An out-of-range index is refused by `validate()`, so this is
        // unreachable through the CLI; saying so beats printing nothing.
        None => " <span class=warn-tag>source document unknown</span>".to_string(),
    }
}

pub fn render(ev: &Evidence, res: &Resolutions, reqs: Option<&butterfly_model::reqtext::Requirements>, variant: Option<&str>) -> String {
    render_with_warnings(ev, res, reqs, variant, &[])
}

pub fn render_with_warnings(ev: &Evidence, res: &Resolutions, reqs: Option<&butterfly_model::reqtext::Requirements>, variant: Option<&str>, req_stale: &[String]) -> String {
    let r: BTreeMap<&str, &Resolution> = res.entries.iter().map(|e| (e.key.as_str(), e)).collect();
    let s = &ev.summary;
    let pv = &ev.provenance;
    let mut o = String::new();
    let _ = write!(o, "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\"><meta name=\"referrer\" content=\"no-referrer\"><title>butterfly report — {}</title><style>{CSS}</style></head><body>", esc(pv.build_info.get("project").map(String::as_str).unwrap_or("evidence")));
    let _ = write!(o, "<h1>Structural coverage and object-code analysis — Software Verification Results</h1>");
    let _ = write!(
        o,
        "<div class=muted>Generated by {} {} ({}) · evidence created {} · {}{}</div>",
        esc(&pv.tool),
        esc(&pv.tool_version),
        esc(&pv.tool_build()),
        unix(pv.created_unix),
        esc(&pv.object_format),
        variant.map(|v| format!(" · variant <b>{}</b>", esc(v))).unwrap_or_default()
    );
    for c in &pv.caveats {
        let bad = c.starts_with("BASELINE CHANGED") || c.starts_with("EXPLORATORY");
        let _ = write!(o, "<div class=\"banner{}\">{}</div>", if bad { " bad" } else { "" }, esc(c));
    }

    // A section's number is its position among the sections this report
    // actually carries, never a literal. Three of them are conditional (the
    // image, stack usage, change impact), and literals had already left a
    // hole once: a report must not read as if a section had been removed.
    let mut sections = 0usize;
    let mut section = |o: &mut String, title: &str| {
        sections += 1;
        let _ = write!(o, "<h2>{sections}. {title}</h2>");
    };

    // ---- summary ----
    section(&mut o, "Summary (Annex A table A-7)");
    let _ = write!(o, "<div class=tiles>");
    let tile = |o: &mut String, v: String, t: &str| {
        let _ = write!(o, "<div class=tile><b>{v}</b>{t}</div>");
    };
    // A function with no coverage record was neither executed nor not: it
    // is out of the denominator, and the tile says how many (#273).
    if s.functions_unrecorded > 0 {
        tile(
            &mut o,
            format!("{}/{} <small>+{} no record</small>", s.functions_executed, s.functions - s.functions_unrecorded, s.functions_unrecorded),
            "functions executed · those with no coverage record of their own: not established",
        );
    } else {
        tile(&mut o, format!("{}/{}", s.functions_executed, s.functions), "functions executed");
    }
    tile(&mut o, format!("{} <small>{}/{}</small>", pct(s.lines_covered, s.lines_total), s.lines_covered, s.lines_total), "statement coverage · obj 7");
    tile(&mut o, format!("{} <small>{}/{}</small>", pct(s.branches_covered, s.branches_total), s.branches_covered, s.branches_total), "decision outcomes · obj 6");
    tile(&mut o, format!("{}/{}", s.decisions_mcdc_covered, s.decisions_total), "MC/DC decisions · obj 5");
    tile(
        &mut o,
        format!("{} <small>{}/{}</small>", pct(s.instructions_projected_covered, s.instructions_total), s.instructions_projected_covered, s.instructions_total),
        "object code, projected · obj 9",
    );
    tile(
        &mut o,
        format!("{} <small>{}/{}</small>", pct(s.instructions_compiler_generated, s.instructions_total), s.instructions_compiler_generated, s.instructions_total),
        "instructions compiler-generated · obj 9",
    );
    if let Some(m) = s.instructions_measured_covered {
        tile(&mut o, format!("{} <small>{}/{}</small>", pct(m, s.instructions_total), m, s.instructions_total), "object code, measured (trace)");
    }
    if let Some(m) = s.outcome_edges_measured {
        tile(&mut o, format!("{}/{}", m, s.outcome_edges_total), "object branch outcomes, measured");
    }
    let unresolved: usize = ev.functions.iter().flat_map(|f| f.objects().flat_map(|ob| ob.gaps.iter())).filter(|g| !r.contains_key(g.key.as_str())).count();
    tile(&mut o, format!("{}/{}", unresolved, s.gaps_total), "gaps unresolved");
    if let Some(w) = s.stack_worst_case {
        tile(&mut o, format!("{w} B"), "stack worst case");
    }
    let _ = write!(o, "</div>");

    // ---- requirements x tests (A-7 obj 3/4, #42) ------------------------
    {
        let m = butterfly_model::reqmatrix::compute(ev);
        section(&mut o, "Requirements coverage (A-7 objectives 3 and 4)");
        for reason in req_stale {
            let _ = write!(o, "<div class=\"banner bad\">Requirement statements below may be STALE: {}.</div>", esc(reason));
        }
        if m.rows.is_empty() && m.requirements_without_code.is_empty() {
            let _ = write!(o, "<p class=muted>No requirement tags were found in the sources, and no requirements universe was supplied to ingest.</p>");
        } else {
            if m.per_test_data {
                let _ = write!(o, "<p>Coverage below is what the <b>named tests</b> reached in each requirement's implementation — the §6.4.4 question — not aggregate coverage of those lines.</p>");
            } else {
                let _ = write!(
                    o,
                    "<p class=warn>No per-test data in this evidence: coverage below is the <b>aggregate</b>, a proxy that cannot show the requirements-based tests did the covering. Re-ingest with <code>--tests</code> for objective 3/4 evidence.</p>"
                );
            }
            if !m.requirements_without_code.is_empty() {
                let _ = write!(o, "<p>{} requirement(s) in the supplied universe; {} implemented.</p>", m.rows.len() + m.requirements_without_code.len(), m.rows.len());
            }
            // Where the statements came from, by digest. Text shown without
            // a baseline is text a reviewer cannot check against the document
            // that was signed, which is worse than showing none.
            //
            // With one document a single line says it. With several, each
            // statement names its own below, because "one of these five
            // digests" does not tell a reviewer which baseline backs the
            // sentence in front of them (review of #56).
            if let Some(q) = reqs {
                match q.sources.len() {
                    0 => {}
                    1 => {
                        let src = &q.sources[0];
                        let _ = write!(
                            o,
                            "<p class=muted>Requirement statements below are quoted from <code>{}</code> <span class=muted>{}</span>.</p>",
                            esc(&src.path),
                            esc(&src.sha256.chars().take(12).collect::<String>())
                        );
                    }
                    n => {
                        let _ = write!(o, "<p class=muted>Requirement statements below are quoted from {n} documents; each names its own.</p>");
                    }
                }
            }
            let _ = write!(o, "<table><tr><th>requirement</th><th>functions</th><th>tests</th><th>lines</th></tr>");
            for r in &m.rows {
                let fns = r.functions.iter().map(|&f| esc(&ev.functions[f].name)).collect::<Vec<_>>().join(", ");
                let tests = if !m.per_test_data {
                    "<span class=muted>no per-test data</span>".to_string()
                } else if r.tests.is_empty() {
                    "<b class=bad>none — implemented, unverified</b>".to_string()
                } else {
                    r.tests.iter().map(|&t| esc(&ev.tests[t].name)).collect::<Vec<_>>().join(", ")
                };
                let cls = if r.lines_covered == r.lines_total { "ok" } else { "bad" };
                let _ = write!(o, "<tr><td><code>{}</code></td><td>{fns}</td><td>{tests}</td><td class={cls}>{}/{}</td></tr>", esc(&r.requirement), r.lines_covered, r.lines_total);
                // The statement itself, on its own row. A reviewer checking a
                // traceability argument should not have to hold an ID in mind
                // and go looking for what it says.
                if let Some(t) = reqs.and_then(|q| q.get(&r.requirement)) {
                    let mut notes = String::new();
                    if t.is_derived() {
                        // A derived requirement has no parent to validate it
                        // against, which is why 5.1.2.h routes it to the
                        // safety assessment. Saying so here is the point of
                        // carrying `kind` at all.
                        notes.push_str(" <span class=warn-tag>derived — no parent requirement; DO-178C 5.1.2.h sends it to the safety assessment</span>");
                    }
                    if t.text.is_empty() {
                        notes.push_str(" <span class=warn-tag>no statement recorded</span>");
                    }
                    let _ = write!(o, "<tr class=reqtext><td></td><td colspan=3>{}{notes}{}</td></tr>", esc(&t.text), quoted_from(reqs, t));
                }
            }
            for r in &m.requirements_without_code {
                // `planned` is the expected state of something not yet built,
                // and most of a real programme is planned; reporting it the
                // same as an unimplemented `implemented` would bury the ones
                // that matter.
                let (label, cls) = match reqs.and_then(|q| q.get(r)).and_then(|t| t.status.clone()) {
                    Some(st) if st == "planned" => ("no code — planned, so none is expected yet".to_string(), "muted"),
                    Some(st) => (format!("no code — but the document says `{}`", esc(&st)), "bad"),
                    None => ("no code — in the supplied universe, tagged nowhere".to_string(), "bad"),
                };
                let _ = write!(o, "<tr><td><code>{}</code></td><td colspan=3><b class={cls}>{label}</b></td></tr>", esc(r));
                if let Some(t) = reqs.and_then(|q| q.get(r)) {
                    if !t.text.is_empty() {
                        let _ = write!(o, "<tr class=reqtext><td></td><td colspan=3>{}{}</td></tr>", esc(&t.text), quoted_from(reqs, t));
                    }
                }
            }
            let _ = write!(o, "</table>");
            // The other direction of "unmatched": statements the document
            // carries that nothing in this evidence claims. Reported for the
            // same reason as the first -- a reader must be able to see where
            // the two sets differ, in both directions, without diffing them
            // by hand.
            if let Some(q) = reqs {
                let named: std::collections::BTreeSet<&str> = m.rows.iter().map(|r| r.requirement.as_str()).chain(m.requirements_without_code.iter().map(String::as_str)).collect();
                let orphans: Vec<&str> = q.entries.iter().map(|e| e.id.as_str()).filter(|id| !named.contains(id)).collect();
                if !orphans.is_empty() {
                    let shown = orphans.iter().take(20).map(|id| format!("<code>{}</code>", esc(id))).collect::<Vec<_>>().join(", ");
                    let more = orphans.len().saturating_sub(20);
                    let _ = write!(
                        o,
                        "<p class=muted>{} requirement(s) in the document are not referenced by this evidence{}: {shown}{}. \
                         That is expected where this component implements part of a larger programme, and a finding where it does not.</p>",
                        orphans.len(),
                        if q.sources.len() == 1 { "" } else { " (across all documents read)" },
                        if more > 0 { format!(", and {more} more") } else { String::new() }
                    );
                }
            }
            if !m.functions_without_requirement.is_empty() {
                let names = m.functions_without_requirement.iter().map(|&f| esc(&ev.functions[f].name)).collect::<Vec<_>>().join(", ");
                let _ = write!(
                    o,
                    "<p><b>{} function(s) carry no requirement tag:</b> {names}. Untagged, or functionality no requirement asks for (§6.4.4.3.c is the question to ask).</p>",
                    m.functions_without_requirement.len()
                );
            }
        }
    }

    // ---- baseline ----
    section(&mut o, "Analysis baseline (§4.4.2.c)");
    if let Some(b) = &ev.baseline {
        let _ = write!(
            o,
            "<p>Baseline <code>{}</code> frozen {} — status <b class=\"{}\">{}</b></p>",
            esc(&b.path),
            unix(b.created_unix),
            if b.status == "matches" { "ok" } else { "bad" },
            esc(&b.status)
        );
        if !b.differences.is_empty() {
            let _ = write!(o, "<ul>");
            for d in &b.differences {
                let _ = write!(o, "<li><b>{}</b>{}: {}</li>", esc(&d.kind), if d.invalidates { " (invalidates earlier analyses)" } else { "" }, esc(&d.detail));
            }
            let _ = write!(o, "</ul>");
        }
    } else {
        let _ = write!(o, "<p class=muted>No baseline was supplied for this ingest.</p>");
    }

    // ---- provenance ----
    section(&mut o, "Build and provenance");
    let _ = write!(o, "<table><tr><th>key</th><th>value</th></tr>");
    // The tool first: which build of butterfly wrote this. The version names
    // a release; the commit names the build (#117).
    let _ = write!(
        o,
        "<tr><td>tool</td><td class=mono>{} {}</td></tr><tr><td>tool commit</td><td class=mono>{}</td></tr>",
        esc(&pv.tool),
        esc(&pv.tool_version),
        esc(&pv.tool_build())
    );
    for (k, v) in &pv.build_info {
        let _ = write!(o, "<tr><td>{}</td><td class=mono>{}</td></tr>", esc(k), esc(v));
    }
    let _ = write!(
        o,
        "<tr><td>coverage export</td><td class=mono>llvm-cov json {}</td></tr><tr><td>object</td><td class=mono>{} {}</td></tr></table>",
        esc(&pv.coverage_export_version),
        esc(&pv.object_format),
        esc(&pv.arch)
    );
    if !pv.external_tools.is_empty() {
        let _ = write!(o, "<h3>External tools</h3><table><tr><th>tool</th><th>path</th><th>version</th></tr>");
        for t in &pv.external_tools {
            let _ = write!(
                o,
                "<tr><td>{}</td><td class=mono>{}</td><td class=mono>{}</td></tr>",
                esc(&t.name),
                esc(&t.path),
                if t.version.is_empty() { "version not recorded".to_string() } else { esc(&t.version) }
            );
        }
        let _ = write!(o, "</table>");
    }
    let _ = write!(o, "<h3>Inputs</h3><table><tr><th>role</th><th>path</th><th>sha256</th><th>bytes</th></tr>");
    for i in &pv.inputs {
        let _ = write!(o, "<tr><td>{}</td><td class=mono>{}</td><td class=mono>{}</td><td>{}</td></tr>", esc(&i.role), esc(&i.path), esc(&i.sha256), i.bytes);
    }
    let _ = write!(o, "</table>");

    // ---- per function ----
    section(&mut o, "Functions");
    let _ = write!(o, "<table><tr><th>function</th><th>file:line</th><th>requirements</th><th>calls</th><th>lines</th><th>outcomes</th><th>MC/DC</th><th>v(G)</th><th>object proj.</th>{}<th>gaps (unres/total)</th><th>source→object review</th></tr>", if ev.measured.is_some() { "<th>object meas.</th><th>outcomes meas.</th>" } else { "" });
    for f in &ev.functions {
        let file = &ev.files[f.file_id];
        let br = f.branches.len() * 2;
        let brc: usize = f.branches.iter().map(|b| (b.true_count > 0) as usize + (b.false_count > 0) as usize).sum();
        let conds: usize = f.decisions.iter().map(|d| d.conditions.len()).sum();
        let condc: usize = f.decisions.iter().flat_map(|d| d.conditions.iter()).filter(|c| c.independence_covered).count();
        // every object body of the function, its constructor/destructor
        // variants included, so the row adds up to the summary (#266)
        let gaps: usize = f.objects().map(|ob| ob.gaps.len()).sum();
        let unres: usize = f.objects().flat_map(|ob| ob.gaps.iter()).filter(|g| !r.contains_key(g.key.as_str())).count();
        let sum = |k: fn(&ObjectFn) -> usize| -> usize { f.objects().map(k).sum() };
        let variants = if f.variants.is_empty() {
            String::new()
        } else {
            format!(" <small class=muted>incl. {}</small>", esc(&f.variants.iter().filter_map(|v| v.variant.as_deref()).collect::<Vec<_>>().join(" ")))
        };
        let rev = r
            .get(format!("s2o:{}", f.name).as_str())
            .map(|x| format!("{} — {} ({}, {})", esc(&x.category), esc(&x.justification), esc(&x.author), esc(&x.date)))
            .unwrap_or_else(|| "<span class=muted>not reviewed</span>".into());
        // no record: no count and no line figures of its own (#273)
        let (calls, lines) = match f.unrecorded_within {
            Some(w) => (format!("<td class=warn>no coverage record (written in {})</td>", esc(ev.functions.get(w).map(|w| w.name.as_str()).unwrap_or("?"))), "—".to_string()),
            None => (format!("<td{}>{}</td>", if f.count == 0 { " class=bad" } else { "" }, f.count), format!("{}/{}", f.lines_covered, f.lines_total)),
        };
        let _ = write!(
            o,
            "<tr><td class=mono>{}</td><td class=mono>{}:{}</td><td class=mono>{}</td>{}<td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>{}<td{}>{}/{}</td><td>{}</td></tr>",
            esc(&f.name),
            esc(&file.display),
            f.line_start,
            esc(&f.requirements.join(" ")),
            calls,
            lines,
            if br > 0 { format!("{brc}/{br}") } else { "—".into() },
            if conds > 0 { format!("{condc}/{conds}") } else { "—".into() },
            f.complexity.cyclomatic_object.map(|c| c.to_string()).unwrap_or("—".into()),
            if f.object.is_some() { format!("{}/{}{variants}", sum(|ob| ob.instr_projected_covered), sum(|ob| ob.instr_total)) } else { "—".into() },
            if ev.measured.is_some() {
                if f.object.is_some() {
                    format!(
                        "<td>{}/{}</td><td>{}/{}</td>",
                        sum(|ob| ob.instr_measured_covered.unwrap_or(0)),
                        sum(|ob| ob.instr_total),
                        sum(|ob| ob.outcome_edges_measured.unwrap_or(0)),
                        sum(|ob| ob.outcome_edges_total)
                    )
                } else {
                    "<td>—</td><td>—</td>".into()
                }
            } else {
                String::new()
            },
            if unres > 0 { " class=warn" } else { "" },
            unres,
            gaps,
            rev
        );
    }
    let _ = write!(o, "</table>");

    // ---- compiler-generated code ----
    section(&mut o, "Compiler-generated code (§6.4.4.2.b, A-7 objective 9)");
    let _ = write!(
        o,
        "<p>{} of {} instructions ({}) are not directly traceable to a source statement of their function.</p><table><tr><th>origin</th><th>instructions</th><th>meaning</th></tr>",
        s.instructions_compiler_generated,
        s.instructions_total,
        pct(s.instructions_compiler_generated, s.instructions_total)
    );
    for (k, n) in &s.compiler_generated_by_origin {
        let meaning = match k.as_str() {
            "prologue" => "frame set-up and callee-saved register spills",
            "epilogue" => "frame teardown and register restores",
            "stack-protector" => "stack canary load/check",
            "runtime-check" => "bounds / overflow / panic checks and the compare+branch guarding them",
            "line-0" => "DWARF attributes to line 0",
            "no-line" => "no DWARF line-table entry",
            "padding" => "alignment padding or trap after a return",
            _ => "",
        };
        let _ = write!(o, "<tr><td class=mono>{}</td><td>{}</td><td>{}</td></tr>", esc(k), n, meaning);
    }
    let _ = write!(o, "</table>");

    // ---- gaps ----
    section(&mut o, "Object-code gaps and resolutions (§6.4.4.2.b, §6.4.4.3)");
    let mut by_kind: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for f in &ev.functions {
        for g in f.objects().flat_map(|ob| ob.gaps.iter()) {
            let e = by_kind.entry(slug(g.kind)).or_default();
            e.0 += 1;
            if !r.contains_key(g.key.as_str()) {
                e.1 += 1;
            }
        }
    }
    let _ = write!(o, "<table><tr><th>kind</th><th>total</th><th>unresolved</th></tr>");
    for (k, (t, u)) in &by_kind {
        let _ = write!(o, "<tr><td>{k}</td><td>{t}</td><td{}>{u}</td></tr>", if *u > 0 { " class=bad" } else { " class=ok" });
    }
    let _ = write!(o, "</table><table><tr><th>function</th><th>kind / class</th><th>where</th><th>finding</th><th>resolution</th></tr>");
    for f in &ev.functions {
        // collapse helper calls per (callee, class) without a string delimiter
        // so C++ operator| / operator|| names never split into the class column
        let mut seen_callee: BTreeMap<(String, String), (usize, usize)> = BTreeMap::new();
        for (ob, g) in f.objects().flat_map(|ob| ob.gaps.iter().map(move |g| (ob, g))) {
            if g.kind == GapKind::HelperCall {
                let callee = g.description.split('`').nth(1).unwrap_or("?").to_string();
                let e = seen_callee.entry((callee, g.class.clone().unwrap_or_default())).or_default();
                e.0 += 1;
                if !r.contains_key(g.key.as_str()) {
                    e.1 += 1;
                }
                continue;
            }
            let res = r
                .get(g.key.as_str())
                .map(|x| format!("<b>{}</b> {} <span class=muted>({}, {})</span>", esc(&x.category.to_uppercase()), esc(&x.justification), esc(&x.author), esc(&x.date)))
                .unwrap_or_else(|| "<span class=bad>unresolved</span>".into());
            let _ = write!(
                o,
                "<tr><td class=mono>{}{}</td><td>{}</td><td class=mono>{:#x}{}</td><td>{}</td><td>{}</td></tr>",
                esc(&f.name),
                ob.variant.as_deref().map(|v| format!(" [{}]", esc(v))).unwrap_or_default(),
                slug(g.kind),
                g.addr_start,
                g.line.map(|l| format!(" L{l}")).unwrap_or_default(),
                esc(&g.description),
                res
            );
        }
        for ((callee, class), (n, u)) in seen_callee {
            let _ = write!(
                o,
                "<tr><td class=mono>{}</td><td>helper-call / {}</td><td>{} site(s)</td><td>call to <code>{}</code></td><td>{}</td></tr>",
                esc(&f.name),
                esc(&class),
                n,
                esc(&callee),
                if u == 0 { "<span class=ok>resolved</span>".to_string() } else { format!("<span class=bad>{u} unresolved</span>") }
            );
        }
    }
    let _ = write!(o, "</table>");

    // ---- coverage justifications ----
    // `fn:`, `line:`, `branch:` and `cond:` entries close obligations in
    // `check` and the worklist exactly as a gap resolution does, so a report
    // carrying only the gap, `s2o:` and `dead:` reviews looked complete while
    // omitting reviews the gate had counted (#325). Each is shown with what
    // it justifies, in the evidence's own terms.
    section(&mut o, "Coverage justifications (§6.4.4.3)");
    let fname = |id: &str| id.parse::<usize>().ok().and_then(|i| ev.functions.get(i)).map(|f| format!("<code>{}</code>", esc(&f.name)));
    // A retained record whose obligation is gone -- the function is there,
    // the branch, decision or instrumented line is not -- closes nothing in
    // `check` or the worklist. `load_resolutions` warns of it on stderr, but
    // this document is read without that, so it says so itself rather than
    // presenting the record as a current target (#335 review). Computed once.
    let stale: std::collections::HashSet<&str> = res.stale_keys(ev).into_iter().collect();
    let mut rows = Vec::new();
    for x in &res.entries {
        let Some((kind, rest)) = x.key.split_once(':') else { continue };
        if !matches!(kind, "fn" | "line" | "branch" | "cond") {
            continue;
        }
        if stale.contains(x.key.as_str()) {
            rows.push(("<span class=bad>stale</span>", format!("<span class=bad>stale: not in the current evidence; closes no obligation</span> <code>{}</code>", esc(&x.key)), x));
            continue;
        }
        let what = match kind {
            "fn" => fname(rest).map(|f| ("function", format!("function {f}"))),
            "line" => rest.split_once(':').and_then(|(file, line)| {
                let fl = file.parse::<usize>().ok().and_then(|i| ev.files.get(i))?;
                Some(("line", format!("<code>{}:{}</code>", esc(&fl.display), esc(line))))
            }),
            // `branch:<fid>:<site>:<outcome>`, the site holding a colon of
            // its own -- so the ends come off, as the sidecar check does
            "branch" => rest.split_once(':').and_then(|(fid, rest)| {
                let (site, outcome) = rest.rsplit_once(':')?;
                Some(("branch outcome", format!("{} branch at L{}, outcome {}", fname(fid)?, esc(site), esc(outcome))))
            }),
            // `cond:<fid>:<site>:<idx>`, where `<site>` is `<line>:<col>` or
            // `<line>:#<ordinal>` (mirroring `branch:` above)
            "cond" => rest.split_once(':').and_then(|(fid, rest)| {
                let (site, idx) = rest.rsplit_once(':')?;
                Some(("MC/DC condition", format!("{} decision at L{}, condition {}", fname(fid)?, esc(site), esc(idx))))
            }),
            _ => continue,
        };
        // A key the evidence does not hold is shown as written, not dropped:
        // the review exists, whatever it now points at.
        let (kind, what) = what.unwrap_or(("not in this evidence", format!("<code>{}</code>", esc(&x.key))));
        rows.push((kind, what, x));
    }
    if rows.is_empty() {
        let _ = write!(o, "<p class=muted>No coverage justifications recorded (<code>fn:</code>, <code>line:</code>, <code>branch:</code>, <code>cond:</code>).</p>");
    } else {
        let _ = write!(o, "<table><tr><th>kind</th><th>justifies</th><th>resolution</th><th>reviewer</th><th>date</th></tr>");
        for (kind, what, x) in rows {
            let _ = write!(
                o,
                "<tr><td>{kind}</td><td class=mono>{what}</td><td><b>{}</b> {}</td><td>{}</td><td>{}</td></tr>",
                esc(&x.category.to_uppercase()),
                esc(&x.justification),
                esc(&x.author),
                esc(&x.date)
            );
        }
        let _ = write!(o, "</table>");
    }

    // ---- source-to-object review ----
    let fns: Vec<&Function> = ev.functions.iter().filter(|f| f.object.is_some()).collect();
    let reviewed: Vec<&Function> = fns.iter().copied().filter(|f| r.contains_key(format!("s2o:{}", f.name).as_str())).collect();
    let tot_l: usize = fns.iter().map(|f| f.lines_total).sum();
    let rev_l: usize = reviewed.iter().map(|f| f.lines_total).sum();
    section(&mut o, "Source-to-object traceability analysis (A-7 obj 9, CAST-12)");
    let _ = write!(o, "<p>{} of {} functions reviewed · {} of code lines in the reviewed sample (≥ 10% expected).</p>", reviewed.len(), fns.len(), pct(rev_l, tot_l));
    let mut cons: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for f in &fns {
        for k in f.constructs.keys() {
            let e = cons.entry(k.as_str()).or_default();
            e.0 += 1;
            if r.contains_key(format!("s2o:{}", f.name).as_str()) {
                e.1 += 1;
            }
        }
    }
    let _ = write!(o, "<table><tr><th>construct</th><th>functions using it</th><th>reviewed functions using it</th><th>covered by sample</th></tr>");
    for (k, (t, rv)) in &cons {
        // escaped like every other string from the evidence (#337)
        let _ = write!(
            o,
            "<tr><td class=mono>{}</td><td>{t}</td><td>{rv}</td><td class=\"{}\">{}</td></tr>",
            esc(k),
            if *rv > 0 { "ok" } else { "bad" },
            if *rv > 0 { "yes" } else { "no" }
        );
    }
    let _ = write!(o, "</table>");
    let stale = |f: &Function| {
        ev.baseline.as_ref().map(|b| b.functions_source_changed.contains(&f.name) || b.functions_object_changed.contains(&f.name) || b.status == "changed").unwrap_or(false)
    };
    if !reviewed.is_empty() {
        let _ = write!(o, "<table><tr><th>function</th><th>verdict</th><th>notes</th><th>reviewer</th><th>date</th><th>status</th></tr>");
        for f in &reviewed {
            let x = r[format!("s2o:{}", f.name).as_str()];
            let _ = write!(
                o,
                "<tr><td class=mono>{}</td><td{}>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"{}\">{}</td></tr>",
                esc(&f.name),
                // a review that found the object code not traceable is a
                // finding needing action, not a pass
                if x.category == "not-traceable" { " class=bad" } else { "" },
                esc(&x.category),
                esc(&x.justification),
                esc(&x.author),
                esc(&x.date),
                if stale(f) { "bad" } else { "ok" },
                if stale(f) { "STALE (baseline changed)" } else { "valid" }
            );
        }
        let _ = write!(o, "</table>");
    }

    // ---- dead code ----
    if let Some(im) = &ev.image {
        section(&mut o, "Dead-code candidates (§6.4.4.3.c) and image");
        if im.dead_candidates.is_empty() {
            let _ = write!(o, "<p class=ok>No unreferenced text symbols.</p>");
        } else {
            let _ = write!(o, "<table><tr><th>symbol</th><th>bytes</th><th>source coverage</th><th>trace</th><th>resolution</th></tr>");
            for d in &im.dead_candidates {
                let res = r
                    .get(d.key.as_str())
                    .map(|x| format!("<b>{}</b> {}", esc(&x.category.to_uppercase()), esc(&x.justification)))
                    .unwrap_or_else(|| "<span class=bad>unresolved</span>".into());
                let _ = write!(
                    o,
                    "<tr><td class=mono>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    esc(&d.name),
                    d.size,
                    d.count.map(|c| c.to_string()).unwrap_or("no coverage function".into()),
                    d.measured.map(|m| if m == 0 { "never executed".to_string() } else { format!("executed ({m}) — hidden reaching path") }).unwrap_or("—".into()),
                    res
                );
            }
            let _ = write!(o, "</table>");
        }
        for f in &im.findings {
            let _ = write!(o, "<p>{}</p>", esc(f));
        }
        let _ = write!(o, "<table><tr><th>section</th><th>address</th><th>size</th><th>flags</th><th>symbols</th></tr>");
        for sec in &im.sections {
            let _ = write!(
                o,
                "<tr><td class=mono>{}</td><td class=mono>{:#x}</td><td>{}</td><td class=mono>{}</td><td>{}</td></tr>",
                esc(&sec.name),
                sec.address,
                sec.size,
                esc(&sec.flags),
                sec.symbols
            );
        }
        let _ = write!(o, "</table>");
    }

    // ---- stack ----
    if !ev.symbols.is_empty() {
        section(&mut o, "Stack usage (§6.3.4.f)");
        let _ = write!(o, "<table><tr><th>symbol</th><th>own frame</th><th>worst case</th><th>depth</th><th>bounded</th></tr>");
        let mut top: Vec<&Symbol> = ev.symbols.iter().filter(|y| y.stack.worst_case.is_some()).collect();
        top.sort_by_key(|y| std::cmp::Reverse(y.stack.worst_case.unwrap_or(0)));
        for y in top.iter().take(20) {
            let _ = write!(
                o,
                "<tr><td class=mono>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"{}\">{}</td></tr>",
                esc(&y.name),
                y.frame.bytes,
                y.stack.worst_case.unwrap_or(0),
                y.stack.depth,
                if y.stack.bounded { "ok" } else { "warn" },
                if y.stack.bounded { "yes".into() } else { esc(y.stack.reason.as_deref().unwrap_or("no")) }
            );
        }
        let _ = write!(o, "</table><p class=muted>{} of {} symbols not bounded (recursion / indirect calls / dynamic frames).</p>", s.stack_unbounded, ev.symbols.len());
    }

    // ---- change impact ----
    if let Some(c) = &ev.changes {
        section(&mut o, "Change impact (§12.1.1)");
        let _ = write!(
            o,
            "<p class=muted>History read at {}{} · {} commit(s) · {} ticket(s) · {} commit(s) with no ticket.</p>",
            esc(&c.head.chars().take(12).collect::<String>()),
            c.since.as_deref().map(|x| format!(" since {}", esc(x))).unwrap_or_default(),
            c.commits.len(),
            c.tickets.len(),
            c.unticketed.len()
        );
        match &c.impact {
            Some(im) => {
                // A persisted impact is bound to the resolutions and evidence
                // revision it was derived from; if either has moved, say so
                // rather than present a stale count as current (#249).
                if let Some(reason) = butterfly_ingest::impact::staleness(im, ev, res) {
                    let _ = write!(o, "<div class=\"banner bad\">Change impact below is STALE: {}.</div>", esc(&reason));
                }
                let sev = |k: &str| im.by_severity.get(k).copied().unwrap_or(0);
                let _ = write!(
                    o,
                    "<p><b>{}</b> function(s) changed{} · <b>{}</b> obligation(s): <span class=bad>{} blocker</span> · <span class=warn>{} major</span> · {} advisory</p>",
                    im.functions_changed.len(),
                    if im.bounded { "" } else { " (whole history — no --since given)" },
                    im.obligations.len(),
                    sev("blocker"),
                    sev("major"),
                    sev("advisory")
                );
                if !c.tickets.is_empty() {
                    let _ = write!(o, "<table><tr><th>ticket</th><th>commits</th><th>added</th><th>modified</th><th>deleted</th><th>files (untraced)</th><th>blockers</th><th>obligations</th></tr>");
                    for t in &im.by_ticket {
                        let tk = &c.tickets[t.ticket];
                        let nm = |ids: &[usize]| ids.iter().filter_map(|&f| ev.functions.get(f)).map(|f| esc(&f.name)).collect::<Vec<_>>().join(", ");
                        let del: Vec<String> = tk.deleted.iter().filter_map(|&d| c.deleted.get(d)).map(|d| esc(&d.name)).collect();
                        let untraced = tk.files.iter().filter(|f| f.functions == 0).count();
                        let _ = write!(o, "<tr><td class=mono>{}</td><td>{}</td><td class=mono>{}</td><td class=mono>{}</td><td class=\"mono {}\">{}</td><td>{}{}</td><td class=\"{}\">{}</td><td>{}</td></tr>", esc(&tk.id), tk.commits.len(), nm(&tk.added), nm(&tk.modified), if del.is_empty() { "" } else { "bad" }, del.join(", "), tk.files.len(), if untraced > 0 { format!(" <span class=bad>({untraced} untraced)</span>") } else { String::new() }, if t.blockers > 0 { "bad" } else { "ok" }, t.blockers, t.obligations.len());
                    }
                    let _ = write!(o, "</table>");
                }
                if !im.obligations.is_empty() {
                    let _ = write!(o, "<table><tr><th>severity</th><th>kind</th><th>function</th><th>target / key / requirement</th><th>detail</th></tr>");
                    for ob in &im.obligations {
                        let what = ob
                            .target_function
                            .and_then(|t| ev.functions.get(t))
                            .map(|f| f.name.clone())
                            .or_else(|| ob.key.clone())
                            .or_else(|| ob.requirement.clone())
                            .or_else(|| ob.file.clone())
                            .unwrap_or_default();
                        let name = ob
                            .function_id
                            .and_then(|f| ev.functions.get(f))
                            .map(|f| f.name.as_str())
                            .or_else(|| ob.ticket.and_then(|t| c.tickets.get(t)).map(|t| t.id.as_str()))
                            .unwrap_or("—");
                        let _ = write!(
                            o,
                            "<tr><td class=\"{}\">{}</td><td class=mono>{}</td><td class=mono>{}</td><td class=mono>{}</td><td>{}</td></tr>",
                            match ob.severity.as_str() {
                                "blocker" => "bad",
                                "major" => "warn",
                                _ => "muted",
                            },
                            esc(&ob.severity),
                            esc(&ob.kind),
                            esc(name),
                            esc(&what),
                            esc(&ob.detail)
                        );
                    }
                    let _ = write!(o, "</table>");
                }
                for cv in &im.caveats {
                    let _ = write!(o, "<p class=muted>{}</p>", esc(cv));
                }
            }
            None => {
                let _ = write!(o, "<p class=muted>No obligations computed: run <code>butterfly impact &lt;evidence&gt; --write</code>.</p>");
            }
        }
        if !c.unticketed.is_empty() {
            let _ = write!(o, "<table><tr><th>commit with no ticket</th><th>date</th><th>author</th><th>subject</th><th>functions</th></tr>");
            for &i in c.unticketed.iter().take(100) {
                let cm = &c.commits[i];
                let _ = write!(
                    o,
                    "<tr><td class=mono>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    esc(&cm.short.chars().take(7).collect::<String>()),
                    esc(&cm.date.chars().take(10).collect::<String>()),
                    esc(&cm.author),
                    esc(&cm.subject),
                    cm.functions.len()
                );
            }
            let _ = write!(o, "</table>");
        }
        for cv in c.caveats.iter().filter(|x| x.contains("uncommitted") || x.contains("different checkout") || x.contains("SHALLOW") || x.contains("could not be walked")) {
            let _ = write!(o, "<p class=bad>{}</p>", esc(cv));
        }
    }

    section(&mut o, "Caveats");
    let _ = write!(o, "<ul>");
    for c in &pv.caveats {
        let _ = write!(o, "<li>{}</li>", esc(c));
    }
    let _ = write!(o, "</ul>");
    // Attribution for the generating tool. The report's *content* is the
    // analysed project's data and carries no licence from the tool's author.
    let _ = write!(
        o,
        "<hr style=\"margin-top:28px;border:none;border-top:1px solid #d7dce2\">\
         <p class=muted style=\"font-size:11px\">Generated by butterfly {} — \
         free and open source under MIT OR Apache-2.0. The analysis results above are the property of the analysed project.</p>",
        esc(&pv.tool_version)
    );
    let _ = write!(o, "</body></html>");
    o
}

/// `yyyy-mm-dd` (UTC, proleptic Gregorian) from a unix timestamp, without
/// pulling in a date crate.
///
/// Arithmetic, not a walk: the timestamp comes from the evidence and is not
/// range-checked, and stepping a year at a time from 1970 took ~5.8e11
/// iterations for `u64::MAX`, minutes of a job slot (#339). This is Howard
/// Hinnant's `civil_from_days`, shifted so a year starts on 1 March and the
/// leap day is the last day of it. Every u64 fits: `t / 86400` is below
/// 2^48, so nothing below can overflow an `i64`.
fn unix(t: u64) -> String {
    let z = (t / 86400) as i64 + 719_468; // days since 0000-03-01
    let era = z / 146_097; // 400-year eras; z >= 0, so no floor adjustment
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March = 0
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::unix;

    /// Known dates, each checked against an independent calendar (#339).
    #[test]
    fn unix_formats_known_dates() {
        for (t, want) in [
            (0, "1970-01-01"),
            (86_399, "1970-01-01"),
            (86_400, "1970-01-02"),
            (68_169_600, "1972-02-29"),
            (946_598_400, "1999-12-31"),
            (951_782_400, "2000-02-29"),
            (951_868_800, "2000-03-01"),
            (4_107_456_000, "2100-02-28"),
            // 2100 is not a leap year: the day after 28 February is 1 March
            (4_107_542_400, "2100-03-01"),
            (1_791_417_600, "2026-10-08"),
            (1_791_417_600 + 86_399, "2026-10-08"),
            (253_402_214_400, "9999-12-31"),
            (253_402_300_799, "9999-12-31"),
            (253_402_300_800, "10000-01-01"),
        ] {
            assert_eq!(unix(t), want, "unix({t})");
        }
    }

    /// Day by day from 1970 to past 2400, against a calendar that just counts
    /// forward, so every month length and leap rule in the range is checked.
    #[test]
    fn unix_agrees_with_counting_days_forward() {
        let (mut y, mut m, mut d) = (1970u64, 1u64, 1u64);
        for day in 0..160_000u64 {
            assert_eq!(unix(day * 86_400 + 43_200), format!("{y:04}-{m:02}-{d:02}"), "day {day}");
            let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
            let len = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][(m - 1) as usize];
            d += 1;
            if d > len {
                (d, m) = (1, m + 1);
                if m > 12 {
                    (m, y) = (1, y + 1);
                }
            }
        }
        assert!(y > 2400, "the range reaches a 400-year leap ({y})");
    }

    /// Every u64 is a date, computed at once: the old loop walked a year at a
    /// time, so u64::MAX took minutes (#339).
    #[test]
    fn unix_of_the_largest_timestamp_is_immediate() {
        let t = std::time::Instant::now();
        assert_eq!(unix(u64::MAX), "584554051223-11-09");
        assert!(t.elapsed() < std::time::Duration::from_millis(500), "{:?}", t.elapsed());
    }
}
