// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Merging two translation units' copies of one header function, end to end
//! through `ingest` -- the path where the bug actually showed (#45).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn tmp(tag: &str) -> PathBuf {
    let d = butterfly_testsupport::scratch_dir(&format!("gcovmerge-{tag}"));
    fs::create_dir_all(d.join("w")).unwrap();
    d
}

/// `return a && b;` -- two decisions on one line, both at gcov's column 0.
fn doc(cwd: &str, p: [u64; 4]) -> String {
    format!(
        r#"{{"format_version":"2","gcc_version":"16","current_working_directory":"{cwd}",
  "files":[{{"file":"shared.h",
    "functions":[{{"name":"f","start_line":2,"end_line":2,"execution_count":1}}],
    "lines":[{{"line_number":2,"count":1,"branches":[
      {{"count":{},"throw":false,"fallthrough":true,"source_block_id":2,"destination_block_id":3}},
      {{"count":{},"throw":false,"fallthrough":false,"source_block_id":2,"destination_block_id":5}},
      {{"count":{},"throw":false,"fallthrough":true,"source_block_id":3,"destination_block_id":4}},
      {{"count":{},"throw":false,"fallthrough":false,"source_block_id":3,"destination_block_id":5}}]}}]}}]}}"#,
        p[0], p[1], p[2], p[3]
    )
}

/// Unit A reached neither decision; unit B took decision 0 one way and
/// decision 1 the other. Summed per decision that is 0=(1,0), 1=(0,1).
///
/// Matching branches on line and column alone put every one of unit B's
/// counts onto decision 0, giving it (1,1) -- fully covered, and a closed
/// obligation nobody earned -- while decision 1 kept (0,0). Reversing the
/// documents moved the damage to the other decision, so the evidence was not
/// even reproducible from the same build.
#[test]
fn each_decision_keeps_its_own_counts_whatever_the_document_order() {
    let d = tmp("order");
    let cwd = d.join("w");
    fs::write(cwd.join("shared.h"), "int x;\nint f(int a,int b){return a&&b;}\n").unwrap();
    let cwds = cwd.to_str().unwrap();
    let a = doc(cwds, [0, 0, 0, 0]);
    let b = doc(cwds, [1, 0, 0, 1]);

    let mut seen = Vec::new();
    for (order, text) in [("a-then-b", format!("{a}\n{b}")), ("b-then-a", format!("{b}\n{a}"))] {
        let cov = d.join(format!("{order}.json"));
        let out = d.join(format!("{order}-ev.json"));
        fs::write(&cov, &text).unwrap();
        let o = Command::new(env!("CARGO_BIN_EXE_butterfly"))
            .args(["ingest", "--coverage", cov.to_str().unwrap(), "--source-root", cwds, "-o", out.to_str().unwrap()])
            .output()
            .expect("spawn butterfly ingest");
        assert!(o.status.success(), "{order}: {}", String::from_utf8_lossy(&o.stderr));

        let ev: serde_json::Value = serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
        let fns = ev["functions"].as_array().expect("functions");
        assert_eq!(fns.len(), 1, "{order}: the two copies merge into one function");
        let bs = fns[0]["branches"].as_array().expect("branches");
        assert_eq!(bs.len(), 2, "{order}: and keep both decisions");

        let got: Vec<(u64, u64, u64)> =
            bs.iter().map(|b| (b["decisionOrdinal"].as_u64().expect("ordinal"), b["trueCount"].as_u64().unwrap(), b["falseCount"].as_u64().unwrap())).collect();
        assert_eq!(got, vec![(0, 1, 0), (1, 0, 1)], "{order}: counts belong to the decision that earned them");
        // and neither decision may read as fully covered
        for (o, t, f) in &got {
            assert!(*t == 0 || *f == 0, "{order}: decision {o} is falsely complete at ({t},{f})");
        }
        seen.push(got);
    }
    assert_eq!(seen[0], seen[1], "the evidence must not depend on gcov's document order");
}
