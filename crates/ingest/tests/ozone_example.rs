// SPDX-License-Identifier: MIT OR Apache-2.0

use butterfly_model::OzoneReport;
use std::{fs, path::Path};

#[test]
fn worked_report_preserves_function_counts_and_scope() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ozone/build/profile.json");
    if !path.exists() {
        butterfly_testsupport::fixture_missing("run examples/ozone/build.sh first");
        return;
    }
    let report: OzoneReport = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let functions: Vec<_> = report
        .modules
        .iter()
        .flat_map(|module| {
            module.functions.iter().map(|f| {
                (
                    module.name.as_str(),
                    f.name.as_str(),
                    f.coverage.source_lines.covered,
                    f.coverage.source_lines.total,
                    f.coverage.instructions.covered,
                    f.coverage.instructions.total,
                )
            })
        })
        .collect();
    assert_eq!(functions, [("filter.c", "update", 2, 3, 7, 10), ("gate.cpp", "update", 0, 1, 0, 4), ("gate.cpp", "operator|", 1, 1, 3, 3)]);
    assert_eq!((report.total.source_lines.covered, report.total.source_lines.total), (3, 5));
    assert_eq!((report.total.instructions.covered, report.total.instructions.total), (10, 17));
}
