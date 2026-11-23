// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the gates in the viewer (area C).
//!
//! README.md:1046-1048 "Set a floor in the Gates panel and it reports the same
//! verdict `butterfly check` would, because it calls the same function", and
//! README.md:1056-1058 "It always shows the command that does the same
//! thing". Exercised here at the boundaries -- exactly at a floor, a hair
//! over it, a ceiling equal to the count and one under it -- where two
//! front doors are most likely to round differently.
//!
//! Runs only when examples/c has been built and ingested.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn evidence() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build/evidence.json");
    if p.exists() {
        Some(p)
    } else {
        butterfly_testsupport::fixture_missing("build examples/c and ingest it first");
        None
    }
}

fn check(cwd: &Path, args: &[String]) -> i32 {
    Command::new(env!("CARGO_BIN_EXE_butterfly")).current_dir(cwd).args(args).output().expect("check").status.code().unwrap_or(-1)
}

/// Split the shown command the way a shell would for the words it can
/// contain here: plain words and single-quoted paths.
fn words(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in cmd.chars() {
        match c {
            '\'' => {
                quoted = !quoted;
                any = true;
            }
            ' ' if !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            _ => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

#[test]
fn at_every_boundary_the_viewer_the_shown_command_and_check_agree() {
    let Some(ev) = evidence() else { return };
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&ev).unwrap()).unwrap();
    let s = &v["summary"];
    let n = |k: &str| s[k].as_u64().unwrap();
    let lines = 100.0 * n("linesCovered") as f64 / n("linesTotal") as f64;
    let outcomes = 100.0 * n("branchesCovered") as f64 / n("branchesTotal") as f64;
    let gaps = n("gapsTotal");
    let mut cases = vec![
        format!("min_lines={lines}"),
        format!("min_lines={}", lines + 0.01),
        format!("min_outcomes={outcomes}"),
        format!("min_outcomes={}", outcomes + 0.01),
        format!("max_unresolved_gaps={gaps}"),
        format!("max_unresolved_gaps={}", gaps.saturating_sub(1)),
        "min_lines=100".into(),
        "min_mcdc=100".into(),
        "min_mcdc=0".into(),
    ];
    if n("conditionsTotal") > 0 {
        cases.push(format!("min_mcdc={}", 100.0 * n("conditionsIndependenceCovered") as f64 / n("conditionsTotal") as f64));
    }
    let cwd = butterfly_testsupport::scratch_dir("reqc-gates");
    let mut srv = common::serve(Command::new(env!("CARGO_BIN_EXE_butterfly")).arg("serve").arg(&ev).arg("--read-only"));
    for q in cases {
        let (code, body) = common::get(&mut srv, &format!("/api/gates?{q}"));
        assert_eq!(code, 200, "{q}: {body}");
        let g: serde_json::Value = serde_json::from_str(&body).unwrap();
        let would = g["wouldExit"].as_i64().unwrap() as i32;
        let cmd = g["command"].as_str().unwrap();
        let mut w = words(cmd);
        assert_eq!(&w[..2], ["butterfly", "check"], "{cmd}");
        w.drain(..1);
        // the command names the evidence as the server was given it, so it
        // is run from where the server was started
        let shown = check(Path::new(env!("CARGO_MANIFEST_DIR")), &w);
        assert_eq!(shown, would, "{q}: the shown command `{cmd}` exits {shown}, the panel says {would}");
        // and check over the same evidence and the same (absent) reviews, typed by hand
        let flag = q.split('=').next().unwrap().replace('_', "-");
        let val = q.split('=').nth(1).unwrap();
        let typed = check(&cwd, &["check".into(), ev.display().to_string(), "--no-resolutions".into(), format!("--{flag}"), val.into()]);
        assert_eq!(typed, would, "{q}: check exits {typed}, the panel says {would}");
    }
}
