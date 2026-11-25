// SPDX-License-Identifier: MIT OR Apache-2.0

use serde_json::{json, Value};
use std::{fs, process::Command};

const REPORT: &str = include_str!("fixtures/ozone-report.txt");

fn butterfly() -> Command {
    Command::new(env!("CARGO_BIN_EXE_butterfly"))
}

#[test]
fn standalone_and_ingest_preserve_the_scoped_report() {
    let dir = butterfly_testsupport::scratch_dir("ozone-evidence");
    let report = dir.join("report.txt");
    fs::write(&report, format!("\u{feff}{}", REPORT.replace('\n', "\r\n"))).unwrap();
    let output = butterfly().arg("ozone").arg(&report).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let profile: Value = serde_json::from_slice(&output.stdout).unwrap();
    let modules = &profile["modules"];
    assert_eq!(modules[0]["name"], "filter.c");
    assert_eq!(modules[0]["functions"][0]["name"], "update");
    assert_eq!(modules[1]["functions"][0]["coverage"]["instructions"], json!({"covered":0,"total":4}));
    assert_eq!(modules[1]["functions"][1]["name"], "operator|");
    assert_eq!(profile["total"]["sourceLines"], json!({"covered":3,"total":5}));
    assert_eq!(profile["total"]["instructions"], json!({"covered":10,"total":17}));

    let coverage = dir.join("coverage.json");
    fs::write(&coverage, r#"{"type":"llvm.coverage.json.export","data":[{"files":[],"functions":[]}]}"#).unwrap();
    let evidence = dir.join("evidence.json");
    let ingest = butterfly().args(["ingest", "--coverage"]).arg(&coverage).arg("--ozone-report").arg(&report).arg("-o").arg(&evidence).output().unwrap();
    assert!(ingest.status.success(), "{}", String::from_utf8_lossy(&ingest.stderr));
    let ev: Value = serde_json::from_slice(&fs::read(&evidence).unwrap()).unwrap();
    assert_eq!(ev["ozoneReport"], profile);
    assert!(ev["measured"].is_null());
    assert_eq!(ev["summary"]["linesCovered"], 0);
    let input = ev["provenance"]["inputs"].as_array().unwrap().iter().find(|p| p["role"] == "ozone-code-profile").unwrap();
    assert_eq!(input["sha256"].as_str().unwrap().len(), 64);
    let summary = butterfly().arg("summary").arg(&evidence).output().unwrap();
    assert!(summary.status.success());
    assert!(String::from_utf8_lossy(&summary.stderr).contains("Ozone export: 3/5 source lines, 10/17 instructions"));
}

#[test]
fn invalid_or_incomplete_reports_fail_without_emitting_coverage() {
    let report = butterfly_testsupport::scratch_dir("ozone-invalid").join("bad.txt");
    for text in [
        REPORT.replace("2 / 3", "9 / 3"),
        REPORT.replace("filter.c           |                 |", ""),
        REPORT.replace("  Total ", "  Missing "),
        "Address,Encoding,Opcode\n1000,abcd,mov".into(),
    ] {
        fs::write(&report, text).unwrap();
        let result = butterfly().arg("ozone").arg(&report).output().unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
}
