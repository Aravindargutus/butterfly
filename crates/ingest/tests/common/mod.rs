// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Builders for synthetic evidence used across integration tests.
#![allow(dead_code)]
use butterfly_ingest::objectcode::RawInstr;
use butterfly_model::*;
use std::collections::BTreeMap;

pub fn ri(addr: u64, m: &str, o: &str) -> RawInstr {
    RawInstr { addr, mnemonic: m.into(), operands: o.into() }
}

pub fn file(lines: &[&str], counts: &[Option<u64>]) -> SourceFile {
    SourceFile {
        id: 0,
        path: "/src/demo.c".into(),
        display: "demo.c".into(),
        sha256: String::new(),
        text: lines.iter().map(|s| s.to_string()).collect(),
        lines: counts.iter().map(|c| LineCov { count: *c }).collect(),
        function_ids: vec![],
        lines_total: counts.iter().filter(|c| c.is_some()).count(),
        lines_covered: counts.iter().filter(|c| matches!(c, Some(n) if *n > 0)).count(),
    }
}

pub fn function(id: usize, name: &str, ls: u32, le: u32) -> Function {
    Function {
        id,
        name: name.into(),
        mangled: name.into(),
        file_id: 0,
        line_start: ls,
        line_end: le,
        count: 1,
        instantiations: 1,
        tests: Vec::new(),
        requirements: vec![],
        line_requirements: BTreeMap::new(),
        lines_total: 0,
        lines_covered: 0,
        branches: vec![],
        decisions: vec![],
        complexity: Complexity::default(),
        callees: vec![],
        callers: vec![],
        object: None,
        variants: vec![],
        symbol_id: None,
        data_refs: vec![],
        variables: Vec::new(),
        constructs: BTreeMap::new(),
        inlined: vec![],
        own_lines: None,
        folded_calls: vec![],
        indirect_calls: vec![],
        pointer_callers: vec![],
        commits: vec![],
        origin: None,
        unrecorded_within: None,
    }
}

pub fn evidence(files: Vec<SourceFile>, functions: Vec<Function>) -> Evidence {
    Evidence {
        tests: Vec::new(),
        requirements_universe: Vec::new(),
        schema: SCHEMA.into(),
        provenance: Provenance { tool: "test".into(), tool_version: "0".into(), ..Default::default() },
        summary: Summary { files: files.len(), functions: functions.len(), ..Default::default() },
        files,
        functions,
        symbols: vec![],
        globals: vec![],
        image: None,
        measured: None,
        baseline: None,
        changes: None,
    }
}
