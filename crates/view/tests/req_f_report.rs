// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the standalone report the viewer serves and
//! `butterfly report` writes (area F). Expected values come from README.md
//! "Report" and "Security posture" and docs/butterfly.html "Report and CI".

use butterfly_model::{BaselineCheck, BaselineDifference, Evidence, Resolution, Resolutions, RESOLUTIONS_SCHEMA};
use std::path::PathBuf;

fn c_evidence() -> Option<Evidence> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build/evidence.json");
    if !p.exists() {
        butterfly_testsupport::fixture_missing("build examples/c and ingest it to examples/c/build/evidence.json first");
        return None;
    }
    Some(serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap())
}

const HOSTILE: &str = "\"'><script>alert(1)</script><img src=x onerror=alert(2)>";

/// REQ-F-REPORT-INERT: "The standalone report carries the policy in a
/// `<meta>` tag, so injected markup stays inert even in a file mailed to a
/// reviewer." (README.md:1577-1578) and it is "script-free" (README.md:1356).
/// Text that comes from the evidence or a reviewer -- a project name, a
/// justification, an author, a baseline difference -- must arrive as text,
/// never as markup, in element or attribute context.
#[test]
fn req_f_hostile_text_in_the_inputs_is_escaped_in_the_report() {
    let Some(mut ev) = c_evidence() else { return };
    ev.provenance.build_info.insert("project".into(), HOSTILE.into());
    ev.provenance.caveats.push(HOSTILE.into());
    let key = ev.functions.iter().find_map(|f| f.object.as_ref().and_then(|o| o.gaps.first()).map(|g| g.key.clone())).expect("examples/c has gaps");
    ev.baseline = Some(BaselineCheck {
        path: HOSTILE.into(),
        created_unix: 0,
        status: "changed".into(),
        differences: vec![BaselineDifference { kind: "flags".into(), detail: HOSTILE.into(), invalidates: true }],
        ..Default::default()
    });
    let res = Resolutions {
        schema: RESOLUTIONS_SCHEMA.into(),
        entries: vec![Resolution { key, category: "analysis".into(), justification: HOSTILE.into(), author: HOSTILE.into(), date: "2026-01-01".into() }],
    };
    let html = butterfly_view::report::render(&ev, &res, None, Some(HOSTILE));
    let lower = html.to_ascii_lowercase();
    assert!(!lower.contains("<script"), "a <script> element reached the report");
    assert!(!lower.contains("<img"), "an <img> element reached the report");
    assert!(!html.contains("\"'>"), "a quote-breaking sequence reached the report unescaped");
    // and the text is still there, as text, for the reader
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "the hostile text was dropped rather than escaped");
}

/// REQ-F-REPORT-BASELINE: the report carries "baseline status"
/// (README.md:1357), the ingest's comparison against a baseline reporting
/// "`matches` / `changed`" with its differences (README.md:1120-1124); with
/// no baseline it does not imply one was checked (README.md:1124-1125).
#[test]
fn req_f_the_report_states_the_baseline_status_and_its_differences() {
    let Some(mut ev) = c_evidence() else { return };
    let none = Resolutions { schema: RESOLUTIONS_SCHEMA.into(), entries: vec![] };
    ev.baseline = None;
    let without = butterfly_view::report::render(&ev, &none, None, None);
    assert!(without.contains("Analysis baseline"), "the section is always there");
    assert!(!without.contains("status <b"), "no baseline, yet a status was shown");

    ev.baseline = Some(BaselineCheck {
        path: "butterfly-baseline.json".into(),
        created_unix: 1_700_000_000,
        status: "changed".into(),
        differences: vec![BaselineDifference { kind: "compiler".into(), detail: "clang 17 -> clang 18".into(), invalidates: true }],
        ..Default::default()
    });
    let with = butterfly_view::report::render(&ev, &none, None, None);
    assert!(with.contains("butterfly-baseline.json") && with.contains("changed"), "the status is not stated");
    assert!(with.contains("clang 17 -&gt; clang 18"), "the difference is not listed");
    assert!(with.contains("invalidates"), "a difference that invalidates earlier analyses must say so (§4.4.2.c)");
}
