// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for `butterfly report` (area C).
//!
//! Expected results come from README.md "Report" (1350-1370), "Requirements
//! coverage" (537-551), "Change impact" (1237-1321), the handbook's "The
//! report" (docs/butterfly.html), and `report --help`.
//!
//! Runs only when examples/c (and, for one case, examples/c-per-test) has
//! been built.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_butterfly")
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tmp(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("reqc-report-{tag}"))
}

fn bf(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin()).current_dir(cwd).args(args).output().expect("run butterfly");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// examples/c, ingested into a fresh directory of its own.
fn evidence(tag: &str, extra: &[&str]) -> Option<(PathBuf, PathBuf)> {
    let ex = repo().join("examples/c");
    if !ex.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c/build.sh first");
        return None;
    }
    let d = tmp(tag);
    let ev = d.join("evidence.json");
    let mut args: Vec<String> =
        vec!["ingest".into(), ex.join("build").to_str().unwrap().to_string(), "--source-root".into(), ex.to_str().unwrap().into(), "-o".into(), ev.to_str().unwrap().into()];
    args.extend(extra.iter().map(|s| s.to_string()));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, _, err) = bf(&d, &args);
    assert_eq!(code, 0, "ingest examples/c: {err}");
    Some((d, ev))
}

/// `butterfly changes` on evidence ingested from examples/c into `d`. The
/// evidence records examples/c/butterfly.toml, which is outside both its
/// directory and the working directory, so it is named here: a config path
/// taken from an evidence file is honoured only from inside (#342).
fn changes(d: &Path, ev: &Path) -> (i32, String, String) {
    let cfg = repo().join("examples/c/butterfly.toml");
    bf(d, &["changes", ev.to_str().unwrap(), "--config", cfg.to_str().unwrap()])
}

fn report(d: &Path, ev: &Path, extra: &[&str]) -> (i32, String, String) {
    let out = d.join("report.html");
    let _ = std::fs::remove_file(&out);
    let mut args = vec!["report", ev.to_str().unwrap(), "-o", out.to_str().unwrap()];
    args.extend(extra);
    let (code, _, err) = bf(d, &args);
    (code, std::fs::read_to_string(&out).unwrap_or_default(), err)
}

/// The `<h2>` headings in document order, as (number, title).
fn headings(html: &str) -> Vec<(usize, String)> {
    html.split("<h2>")
        .skip(1)
        .map(|h| {
            let t = h.split("</h2>").next().unwrap();
            let (n, title) = t.split_once(". ").unwrap_or_else(|| panic!("an unnumbered heading: {t}"));
            (n.parse().unwrap_or_else(|_| panic!("not a number: {t}")), title.to_string())
        })
        .collect()
}

/// The body of the section whose title contains `key`.
fn section<'a>(html: &'a str, key: &str) -> &'a str {
    let i = html.find(&format!(". {key}")).unwrap_or_else(|| panic!("a section `{key}`"));
    let rest = &html[i..];
    let end = rest[1..].find("<h2>").map(|e| e + 1).unwrap_or(rest.len());
    &rest[..end]
}

fn sidecar(entries: Value) -> String {
    serde_json::json!({"schema": "butterfly-resolutions/1", "entries": entries}).to_string()
}

fn first_gap_key(ev: &Path) -> String {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(ev).unwrap()).unwrap();
    v["functions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["object"]["gaps"].as_array().and_then(|g| g.first()))
        .map(|g| g["key"].as_str().unwrap().to_string())
        .next()
        .expect("a gap")
}

// README.md:1356-1357 "a self-contained, script-free HTML document"; handbook "The report": "one
// self-contained HTML file — no external assets, safe to email".
#[test]
fn the_report_is_self_contained_and_script_free() {
    let Some((d, ev)) = evidence("selfcontained", &[]) else { return };
    let (code, html, err) = report(&d, &ev, &[]);
    assert_eq!(code, 0, "{err}");
    let lower = html.to_ascii_lowercase();
    assert!(!lower.contains("<script"), "script-free");
    assert!(!regex_like_handler(&lower), "no inline event handlers");
    for attr in ["src=\"http", "src='http", "href=\"http", "href='http", "url(http", "@import", "<link", "<iframe", "<img src=\"//", "src=\"//"] {
        assert!(!lower.contains(attr), "no external asset ({attr})");
    }
}

/// `onclick=`, `onload=` and friends, without a regex dependency.
fn regex_like_handler(s: &str) -> bool {
    ["onclick=", "onload=", "onerror=", "onmouseover=", "onfocus="].iter().any(|h| s.contains(h))
}

// report --help: "-o, --out <OUT> [default: butterfly-report.html]".
#[test]
fn the_report_defaults_to_butterfly_report_html_in_the_working_directory() {
    let Some((d, ev)) = evidence("default-out", &[]) else { return };
    let cwd = d.join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let (code, _, err) = bf(&cwd, &["report", ev.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(cwd.join("butterfly-report.html").is_file(), "{err}");
}

// Handbook "The report": "Thirteen sections, numbered in the order they appear: summary against table
// A-7, requirements coverage, the analysis baseline, build and provenance with input hashes, functions,
// compiler-generated code, gaps and their resolutions, coverage justifications ..., source-to-object
// traceability with CAST-12 sample adequacy, dead-code candidates and the image, stack usage, change
// impact, and caveats."
// README.md:541 "butterfly report ... # §2 of the report" is the requirements matrix.
#[test]
fn with_every_conditional_section_present_there_are_thirteen_in_the_documented_order() {
    let Some((d, ev)) = evidence("thirteen", &[]) else { return };
    let (code, _, err) = changes(&d, &ev);
    assert_eq!(code, 0, "changes: {err}");
    let (code, html, err) = report(&d, &ev, &["--resolutions", &write_empty_sidecar(&d)]);
    assert_eq!(code, 0, "{err}");
    let h = headings(&html);
    let want = [
        "Summary",
        "Requirements coverage",
        "baseline",
        "provenance",
        "Functions",
        "Compiler-generated",
        "gaps",
        "Coverage justifications",
        "Source-to-object",
        "Dead-code",
        "Stack usage",
        "Change impact",
        "Caveats",
    ];
    assert_eq!(h.len(), 13, "{h:?}");
    for (i, ((n, title), key)) in h.iter().zip(want).enumerate() {
        assert_eq!(*n, i + 1, "numbered in order: {h:?}");
        assert!(title.to_lowercase().contains(&key.to_lowercase()), "section {} is {key}: {title}", i + 1);
    }
    // README.md:1357-1358 "provenance and input hashes"
    let prov = section(&html, "Build and provenance");
    assert!(prov.contains("sha256") || prov.chars().filter(|c| c.is_ascii_hexdigit()).count() > 64, "input hashes are listed");
}

fn write_empty_sidecar(d: &Path) -> String {
    let p = d.join("empty-resolutions.json");
    std::fs::write(&p, sidecar(serde_json::json!([]))).unwrap();
    p.display().to_string()
}

// Handbook "The report": "Dead-code candidates, stack usage and change impact appear only when the
// evidence carries them; the numbering closes up around one that is absent".
#[test]
fn without_change_history_the_section_is_absent_and_the_numbering_closes_up() {
    let Some((d, ev)) = evidence("closeup", &[]) else { return };
    let (code, html, err) = report(&d, &ev, &[]);
    assert_eq!(code, 0, "{err}");
    let h = headings(&html);
    assert!(h.iter().all(|(_, t)| !t.contains("Change impact")), "{h:?}");
    assert_eq!(h.iter().map(|(n, _)| *n).collect::<Vec<_>>(), (1..=h.len()).collect::<Vec<_>>(), "{h:?}");
    assert_eq!(h.last().unwrap().1, "Caveats");
}

// README.md:1362-1367 "A path given with --resolutions must load: missing, unreadable, malformed or a
// schema this build does not know are all errors naming the file, and no report is written."
#[test]
fn an_explicit_resolutions_path_must_load_or_no_report_is_written() {
    let Some((d, ev)) = evidence("res-explicit", &[]) else { return };
    std::fs::write(d.join("malformed.json"), "{ not json").unwrap();
    std::fs::write(d.join("foreign.json"), serde_json::json!({"schema": "butterfly-resolutions/99", "entries": []}).to_string()).unwrap();
    std::fs::write(
        d.join("invalid.json"),
        sidecar(serde_json::json!([{"key": first_gap_key(&ev), "category": "not-a-category", "justification": "x", "author": "a", "date": "2026-01-15"}])),
    )
    .unwrap();
    for name in ["absent.json", "malformed.json", "foreign.json", "invalid.json"] {
        let p = d.join(name);
        let (code, html, err) = report(&d, &ev, &["--resolutions", p.to_str().unwrap()]);
        assert_eq!(code, 1, "{name}: an error: {err}");
        assert!(err.contains(name), "{name}: the error names the file: {err}");
        assert!(html.is_empty(), "{name}: no report is written");
    }
}

// README.md:1367-1369 "Only the discovered sidecar may be absent — a project that has recorded nothing
// yet has no file — and the run says so rather than staying silent."
#[test]
fn an_absent_discovered_sidecar_is_allowed_and_said() {
    let Some((d, ev)) = evidence("res-absent", &[]) else { return };
    let (code, html, err) = report(&d, &ev, &[]);
    assert_eq!(code, 0, "{err}");
    assert!(!html.is_empty());
    assert!(err.contains("no resolutions sidecar"), "said, not silent: {err}");
}

// README.md:1367 "Only the discovered sidecar may be absent": one that is there must load, like an
// explicit one -- a malformed discovered sidecar is not "no reviews".
#[test]
fn a_discovered_sidecar_that_is_malformed_is_an_error_not_an_absence() {
    let Some((d, ev)) = evidence("res-discovered-bad", &[]) else { return };
    std::fs::write(d.join("resolutions.json"), "[ truncated").unwrap();
    let (code, html, err) = report(&d, &ev, &[]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("resolutions.json"), "{err}");
    assert!(html.is_empty());
}

// README.md:1357-1358 the report carries "gaps with their resolutions".
#[test]
fn a_recorded_resolution_appears_with_its_gap() {
    let Some((d, ev)) = evidence("res-shown", &[]) else { return };
    let key = first_gap_key(&ev);
    let just = "Reviewed against the object code: prologue spill, no decision (req-c)";
    std::fs::write(d.join("resolutions.json"), sidecar(serde_json::json!([{"key": key, "category": "analysis", "justification": just, "author": "req-c", "date": "2026-01-15"}])))
        .unwrap();
    let (code, html, err) = report(&d, &ev, &[]);
    assert_eq!(code, 0, "{err}");
    let gaps = section(&html, "Object-code gaps");
    assert!(gaps.contains("Reviewed against the object code"), "the justification is in the gaps section");
}

// README.md:1511-1517 "Besides the gap, s2o: and dead: resolutions, the Coverage justifications section
// lists each fn:, line:, branch: and cond: entry with what it justifies — the function, the source
// line, the branch outcome or the MC/DC condition — and its category, reasoning, reviewer and date ...
// With none recorded the section says so."
#[test]
fn every_kind_of_justification_is_in_the_report_with_what_it_justifies() {
    let Some((d, ev)) = evidence("justifications", &[]) else { return };
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&ev).unwrap()).unwrap();
    // one obligation of each kind, from the evidence itself
    let f = v["functions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| !f["branches"].as_array().unwrap().is_empty() && !f["decisions"].as_array().unwrap().is_empty())
        .expect("a function with a decision");
    let (fid, name) = (f["id"].as_u64().unwrap(), f["name"].as_str().unwrap());
    let b = f["branches"].as_array().unwrap().iter().find(|b| b["decisionOrdinal"].is_null() && b["directionKnown"] == true).expect("a T/F branch");
    let dc = &f["decisions"][0];
    let file = &v["files"][f["fileId"].as_u64().unwrap() as usize];
    let line = file["lines"].as_array().unwrap().iter().position(|l| !l["count"].is_null()).expect("an instrumented line") + 1;
    let entries = serde_json::json!([
        {"key": format!("fn:{fid}"), "category": "d2", "justification": "JUST-FN never called in this configuration", "author": "req-c", "date": "2026-01-15"},
        {"key": format!("line:{}:{line}", file["id"]), "category": "analysis", "justification": "JUST-LINE defensive check", "author": "req-c", "date": "2026-01-16"},
        {"key": format!("branch:{fid}:{}:{}:F", b["line"], b["col"]), "category": "analysis", "justification": "JUST-BRANCH false edge infeasible", "author": "req-c", "date": "2026-01-17"},
        {"key": format!("cond:{fid}:{}:{}:1", dc["line"], dc["col"]), "category": "analysis", "justification": "JUST-COND masked by the first operand", "author": "req-c", "date": "2026-01-18"},
    ]);
    std::fs::write(d.join("resolutions.json"), sidecar(entries)).unwrap();
    let (code, html, err) = report(&d, &ev, &[]);
    assert_eq!(code, 0, "{err}");
    let s = section(&html, "Coverage justifications");
    let row = |marker: &str| -> String {
        let i = s.find(marker).unwrap_or_else(|| panic!("{marker} is in the section: {s}"));
        let start = s[..i].rfind("<tr>").unwrap();
        let end = i + s[i..].find("</tr>").unwrap();
        s[start..end].to_string()
    };
    let r = row("JUST-FN");
    assert!(r.contains("function") && r.contains(name) && r.contains("D2") && r.contains("req-c") && r.contains("2026-01-15"), "{r}");
    let r = row("JUST-LINE");
    assert!(r.contains(&format!("{}:{line}", file["display"].as_str().unwrap())), "the source line it justifies: {r}");
    let r = row("JUST-BRANCH");
    assert!(r.contains(name) && r.contains(&format!("L{}:{}", b["line"], b["col"])) && r.contains("outcome F"), "the branch outcome: {r}");
    let r = row("JUST-COND");
    assert!(r.contains(name) && r.contains(&format!("L{}:{}", dc["line"], dc["col"])) && r.contains("condition 1"), "the MC/DC condition: {r}");
    // README.md:1504 "script-free", with the same CSP as before
    assert!(!html.to_ascii_lowercase().contains("<script"));
    assert!(html.contains("content=\"default-src 'none'; style-src 'unsafe-inline'\""), "the CSP is unchanged");

    // none recorded: the section is there and says so
    let (code, html, err) = report(&d, &ev, &["--resolutions", &write_empty_sidecar(&d)]);
    assert_eq!(code, 0, "{err}");
    assert!(section(&html, "Coverage justifications").contains("No coverage justifications recorded"), "{html}");
}

// README.md:1358 "baseline status"; README.md:124 an ingest given --baseline "compares against it and
// flags stale analyses". The same build matches; a different build's baseline lists its differences.
#[test]
fn the_baseline_section_states_the_status_and_lists_differences() {
    let Some((d, ev)) = evidence("baseline-same", &[]) else { return };
    let b = d.join("baseline.json");
    let (code, _, err) = bf(&d, &["baseline", ev.to_str().unwrap(), "-o", b.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let Some((d2, ev2)) = evidence("baseline-same-2", &["--baseline", b.to_str().unwrap()]) else { return };
    let (code, html, err) = report(&d2, &ev2, &[]);
    assert_eq!(code, 0, "{err}");
    let s = section(&html, "Analysis baseline");
    assert!(s.contains("matches"), "the build it was frozen from matches: {s}");
    assert!(!s.contains("No baseline was supplied"), "{s}");

    // a baseline frozen from a different program: examples/c-per-test
    let other = repo().join("examples/c-per-test");
    if !other.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c-per-test/build.sh first");
        return;
    }
    let oev = d.join("other.json");
    let (code, _, err) = bf(&other, &["ingest", "build", "--source-root", ".", "-o", oev.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let ob = d.join("other-baseline.json");
    let (code, _, err) = bf(&d, &["baseline", oev.to_str().unwrap(), "-o", ob.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let Some((d3, ev3)) = evidence("baseline-diff", &["--baseline", ob.to_str().unwrap()]) else { return };
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&ev3).unwrap()).unwrap();
    let status = v["baseline"]["status"].as_str().unwrap().to_string();
    assert_ne!(status, "matches", "a different program's baseline does not match");
    let (code, html, err) = report(&d3, &ev3, &[]);
    assert_eq!(code, 0, "{err}");
    let s = section(&html, "Analysis baseline");
    assert!(s.contains(&status), "the status is stated: {s}");
    let n = v["baseline"]["differences"].as_array().map(|a| a.len()).unwrap_or(0);
    assert_eq!(s.matches("<li>").count(), n, "every difference is listed: {s}");
}

// README.md:1270-1272 "Obligations are grouped per ticket ... on the Overview, in the report's Change
// impact section, and in butterfly impact --json" -- the same obligations in both.
#[test]
fn the_change_impact_section_carries_the_obligations_impact_reports() {
    let Some((d, ev)) = evidence("impact", &[]) else { return };
    let (code, _, err) = changes(&d, &ev);
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = bf(&d, &["impact", ev.to_str().unwrap(), "--json"]);
    assert_eq!(code, 0, "{err}");
    let imp: Value = serde_json::from_str(&out).unwrap();
    let n = imp["obligations"].as_array().unwrap().len();
    let (code, html, err) = report(&d, &ev, &[]);
    assert_eq!(code, 0, "{err}");
    let s = section(&html, "Change impact");
    assert!(s.contains(&format!("<b>{n}</b> obligation(s)")), "{n} obligations stated: {}", &s[..s.len().min(800)]);
    // one row per obligation in the obligations table (header row excluded)
    let rows = s.matches("<tr><td class=\"bad\">").count() + s.matches("<tr><td class=\"warn\">").count() + s.matches("<tr><td class=\"muted\">").count();
    assert_eq!(rows, n, "every obligation is listed");
    assert!(!s.contains("STALE"), "computed against these resolutions, so not stale");
}

// README.md:1314-1321 "counts the resolutions written since the obligations were computed. If any of
// that has moved ... say so ... (`butterfly impact … --write` when only resolutions changed). Stale
// attribution is never presented as current."
#[test]
fn obligations_computed_before_a_new_review_are_marked_stale() {
    let Some((d, ev)) = evidence("impact-stale", &[]) else { return };
    let (code, _, err) = changes(&d, &ev);
    assert_eq!(code, 0, "{err}");
    std::fs::write(
        d.join("resolutions.json"),
        sidecar(serde_json::json!([{"key": first_gap_key(&ev), "category": "analysis", "justification": "reviewed after impact", "author": "req-c", "date": "2026-01-15"}])),
    )
    .unwrap();
    let (code, html, err) = report(&d, &ev, &[]);
    assert_eq!(code, 0, "{err}");
    assert!(section(&html, "Change impact").contains("STALE"), "not presented as current");
    // and impact --write brings it up to date
    let (code, _, err) = bf(&d, &["impact", ev.to_str().unwrap(), "--write"]);
    assert_eq!(code, 0, "{err}");
    let (_, html, _) = report(&d, &ev, &[]);
    assert!(!section(&html, "Change impact").contains("STALE"));
}
