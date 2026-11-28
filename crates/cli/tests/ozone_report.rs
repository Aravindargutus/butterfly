// SPDX-License-Identifier: MIT OR Apache-2.0

use serde_json::{json, Value};
use std::{fs, process::Command};

const REPORT: &str = include_str!("../../../examples/ozone/report.txt");

fn butterfly() -> Command {
    Command::new(env!("CARGO_BIN_EXE_butterfly"))
}

#[test]
fn standalone_and_ingest_preserve_the_scoped_report() {
    let dir = butterfly_testsupport::scratch_dir("ozone-evidence");
    let report = dir.join("report.txt");
    // A BOM and CRLF line endings, built from LF: on a checkout that already
    // has CRLF, include_str! keeps it, and replacing '\n' alone would give
    // "\r\r\n" and a different digest from the one asserted below.
    fs::write(&report, format!("\u{feff}{}", REPORT.replace("\r\n", "\n").replace('\n', "\r\n"))).unwrap();
    let output = butterfly().arg("ozone").arg(&report).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let profile: Value = serde_json::from_slice(&output.stdout).unwrap();
    let modules = &profile["modules"];
    assert_eq!(modules.as_array().unwrap().len(), 2);
    assert_eq!(modules[0]["name"], "filter.c");
    assert_eq!(modules[1]["name"], "gate.cpp");
    assert_eq!(modules[1]["functions"][0]["name"], "update");
    assert_eq!(modules[0]["functions"][0]["coverage"], json!({"sourceLines":{"covered":2,"total":3},"instructions":{"covered":7,"total":10}}));
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
    // Digest of the fixture's original BOM-prefixed CRLF bytes.
    assert_eq!(input["sha256"], "4742c901f5abc71daea7f43cc26ebea1c075c60c3b0219bcd84821d55c6a40a1");
    assert_eq!(input["bytes"], fs::metadata(&report).unwrap().len());
    let baseline = dir.join("without-ozone.json");
    let result = butterfly().args(["ingest", "--coverage"]).arg(&coverage).arg("-o").arg(&baseline).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let without: Value = serde_json::from_slice(&fs::read(baseline).unwrap()).unwrap();
    assert!(without.get("ozoneReport").is_none());
    assert_eq!(ev["summary"], without["summary"]);
    assert_eq!(ev["measured"], without["measured"]);
    let summary = butterfly().arg("summary").arg(&evidence).output().unwrap();
    assert!(summary.status.success());
    assert!(String::from_utf8_lossy(&summary.stderr).contains("ozone      3/5 source lines, 10/17 instructions over 3 function rows"));
}

#[test]
fn invalid_or_incomplete_reports_fail_without_emitting_coverage() {
    let dir = butterfly_testsupport::scratch_dir("ozone-invalid");
    let report = dir.join("bad.txt");
    let output = dir.join("evidence.json");
    let coverage = dir.join("coverage.json");
    fs::write(&coverage, r#"{"type":"llvm.coverage.json.export","data":[{"files":[],"functions":[]}]}"#).unwrap();
    for text in [
        REPORT.replace("2 / 3", "9 / 3"),
        REPORT.replace("66.7%", "100.0%"),
        REPORT.replace("2 / 3  66.7%", "0 / 0  50.0%"),
        REPORT.replace("2 / 3  66.7%", "10 / 17  58.82352941176470588234%"),
        REPORT.replace("filter.c           |                 |", ""),
        REPORT.replace("  Total ", "  Missing "),
        // a Total straight under the header: a report over no function rows
        "Ozone Code Profile Report\n\nCode Coverage Summary\n\nModule/Function | Source Lines | Instructions\n----------------+--------------+-------------\n  Total         | 3 / 5 60.0%  | 10 / 17 58.8%\n".into(),
        REPORT.replace("2 / 3  66.7%", "+2 / +3  66.7%"),
        REPORT.replace("2 / 3  66.7%", "2 / 3  67.%"),
        "Address,Encoding,Opcode\n1000,abcd,mov".into(),
    ] {
        fs::write(&report, text).unwrap();
        let result = butterfly().arg("ozone").arg(&report).output().unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        let ingest = butterfly().args(["ingest", "--coverage"]).arg(&coverage).arg("--ozone-report").arg(&report).arg("-o").arg(&output).output().unwrap();
        assert!(!ingest.status.success());
        assert!(String::from_utf8_lossy(&ingest.stderr).contains("Ozone Code Profile Report"));
        assert!(!output.exists());
    }
}

#[test]
fn preserves_counts_at_the_displayed_precision() {
    let path = butterfly_testsupport::scratch_dir("ozone-decimals").join("capture.txt");
    for (cell, covered, total) in [
        ("0 / 0 0.0%", 0, 0),
        ("10 / 17 58.824%", 10, 17),
        ("10 / 17 58.82352941176470588235%", 10, 17),
        ("18446744073709551614 / 18446744073709551615 100.0%", u64::MAX - 1, u64::MAX),
        // truncated rather than rounded, so partial coverage never reads 100.0%
        ("9999 / 10000 99.9%", 9999, 10000),
        ("9999 / 10000 100.0%", 9999, 10000),
        ("2 / 3 66.6%", 2, 3),
    ] {
        fs::write(&path, REPORT.replace("2 / 3  66.7%", cell)).unwrap();
        let output = butterfly().arg("ozone").arg(&path).output().unwrap();
        assert!(output.status.success(), "{cell}: {}", String::from_utf8_lossy(&output.stderr));
        let profile: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(profile["modules"][0]["functions"][0]["coverage"]["sourceLines"], json!({"covered":covered,"total":total}));
    }
}

#[cfg(unix)]
#[test]
fn ozone_reports_are_explicit_inputs_even_when_linked() {
    let dir = butterfly_testsupport::scratch_dir("ozone-explicit");
    let build = dir.join("build");
    fs::create_dir(&build).unwrap();
    let report = dir.join("original.txt");
    fs::write(&report, REPORT).unwrap();
    let coverage = build.join("coverage.json");
    fs::write(&coverage, r#"{"type":"llvm.coverage.json.export","data":[{"files":[],"functions":[]}]}"#).unwrap();
    let link = build.join("ozone-report.txt");
    std::os::unix::fs::symlink(&report, &link).unwrap();
    let hardlink = build.join("ozone-profile.txt");
    fs::hard_link(&report, &hardlink).unwrap();
    let opts = butterfly_ingest::discover(&build).unwrap();
    assert!(opts.ozone_report.is_none());
    let out = dir.join("evidence.json");
    for named in [link, hardlink] {
        let cmd = butterfly().arg("ozone").arg(&named).output().unwrap();
        assert!(cmd.status.success(), "{}", String::from_utf8_lossy(&cmd.stderr));
        let cmd = butterfly().arg("ingest").arg(&build).arg("--ozone-report").arg(named).arg("-o").arg(&out).output().unwrap();
        assert!(cmd.status.success(), "{}", String::from_utf8_lossy(&cmd.stderr));
    }
}

/// Evidence is validated when it is loaded, the Ozone report included: an
/// edited report with impossible counts or empty names must not reach the
/// summary or the HTML report (review of 70dfa93, finding 1).
#[test]
fn malformed_ozone_evidence_is_refused_when_loaded() {
    let dir = butterfly_testsupport::scratch_dir("ozone-tampered");
    let report = dir.join("report.txt");
    fs::write(&report, REPORT).unwrap();
    let coverage = dir.join("coverage.json");
    fs::write(&coverage, r#"{"type":"llvm.coverage.json.export","data":[{"files":[],"functions":[]}]}"#).unwrap();
    let evidence = dir.join("evidence.json");
    let ingest = butterfly().args(["ingest", "--coverage"]).arg(&coverage).arg("--ozone-report").arg(&report).arg("-o").arg(&evidence).output().unwrap();
    assert!(ingest.status.success(), "{}", String::from_utf8_lossy(&ingest.stderr));
    let good: Value = serde_json::from_slice(&fs::read(&evidence).unwrap()).unwrap();
    let ok = butterfly().arg("summary").arg(&evidence).output().unwrap();
    assert!(ok.status.success(), "the untouched evidence loads: {}", String::from_utf8_lossy(&ok.stderr));

    type Edit = fn(&mut Value);
    let tampered: [(&str, Edit); 5] = [
        ("total source lines", |v| v["ozoneReport"]["total"]["sourceLines"] = json!({"covered": 999, "total": 5})),
        ("function instructions", |v| v["ozoneReport"]["modules"][0]["functions"][0]["coverage"]["instructions"] = json!({"covered": 999, "total": 10})),
        ("empty module name", |v| v["ozoneReport"]["modules"][0]["name"] = json!("")),
        ("empty function name", |v| v["ozoneReport"]["modules"][1]["functions"][0]["name"] = json!(" ")),
        ("no function rows", |v| v["ozoneReport"]["modules"] = json!([])),
    ];
    for (what, edit) in tampered {
        let mut v = good.clone();
        edit(&mut v);
        let bad = dir.join("tampered.json");
        fs::write(&bad, serde_json::to_vec(&v).unwrap()).unwrap();
        let summary = butterfly().arg("summary").arg(&bad).output().unwrap();
        assert!(!summary.status.success(), "{what}: summary accepted it");
        assert!(String::from_utf8_lossy(&summary.stderr).contains("ozoneReport"), "{what}: {}", String::from_utf8_lossy(&summary.stderr));
        let html = dir.join("report.html");
        let _ = fs::remove_file(&html);
        let rendered = butterfly().arg("report").arg(&bad).arg("-o").arg(&html).output().unwrap();
        assert!(!rendered.status.success(), "{what}: report accepted it");
        assert!(!html.exists(), "{what}: a report was written");
    }
}
