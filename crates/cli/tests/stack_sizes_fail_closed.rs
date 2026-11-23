// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! A malformed `.stack_sizes` section fails the stack result closed, end to
//! end (#247): from a real linked image, through `ingest`, into the
//! serialized evidence and the rendered report.
//!
//! The parser is unit-tested byte by byte; this is the result-level check
//! that its `malformed` outcome reaches the reviewer. Each case is a real ELF
//! -- built with `clang -fstack-size-section` for the target, linked by the
//! toolchain's `rust-lld` -- so the address width and byte order come from
//! the object's own class and data encoding, as they do in production, across
//! ELF32/ELF64 and little/big endian. The corruption is spliced in with
//! `llvm-objcopy --update-section`: the first entry is kept and the second is
//! cut off partway through its address.
//!
//! Needs clang with the ELF targets and the rustup `llvm-tools`. On a bare
//! developer worktree a case whose tools are missing prints `skipping:` and
//! does not count as a pass. But where the fixtures are required -- CI, or
//! `BUTTERFLY_REQUIRE_FIXTURES` set -- a skip is a lie: this suite's whole
//! claim is that the bound fails closed across *every* address width and byte
//! order, and a silently skipped target proves nothing about that width. So in
//! required mode a missing toolchain fails the case instead of skipping it,
//! the same rule #242 gives the fixture-backed suites. clang is a
//! cross-compiler for every triple here and `rust-lld` is target-agnostic, so
//! all four cases do build wherever the C fixtures do.

use butterfly_ingest::objectcode::find_llvm_tool;
use butterfly_ingest::IngestOptions;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Are fixtures required in this environment? An explicit
/// `BUTTERFLY_REQUIRE_FIXTURES` always wins; otherwise `CI` being set to a
/// truthy value requires them. Mirrors `butterfly_testsupport`'s rule (#242),
/// inlined because that helper post-dates this branch.
fn fixtures_required() -> bool {
    fn truthy(v: &str) -> bool {
        !matches!(v.trim().to_ascii_lowercase().as_str(), "" | "0" | "false" | "no" | "off")
    }
    match std::env::var("BUTTERFLY_REQUIRE_FIXTURES") {
        Ok(v) => truthy(&v),
        Err(_) => std::env::var("CI").ok().as_deref().map(truthy).unwrap_or(false),
    }
}

const SRC: &str = "int leaf(int x) { volatile int a[8]; a[0] = x; return a[0]; }\n\
int mid(int x) { volatile int b[4]; b[1] = x; return leaf(b[1]) + 1; }\n\
void _start(void) { mid(3); for (;;) {} }\n";

/// A minimal llvm-cov export naming the source.
fn export(src: &Path) -> String {
    format!(
        r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[[1,1,1,true,true,false]],"branches":[],"summary":{{}}}}],
           "functions":[{{"name":"leaf","count":1,"regions":[[1,1,1,60,1,0,0,0]],"branches":[],"filenames":[{p:?}]}}]}}]}}"#,
        p = src.to_str().unwrap()
    )
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{cmd:?}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Build the image for `target` in a fresh scratch directory, or say why not.
fn build(target: &str) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let objcopy = find_llvm_tool("llvm-objcopy", None).map_err(|e| e.to_string())?;
    let lld = find_llvm_tool("rust-lld", None).map_err(|e| e.to_string())?;
    let d = butterfly_testsupport::scratch_dir(&format!("stack-sizes-{target}"));
    let src = d.join("t.c");
    std::fs::write(&src, SRC).unwrap();
    let obj = d.join("t.o");
    let elf = d.join("t.elf");
    run(Command::new("clang")
        .arg(format!("--target={target}"))
        .args(["-O1", "-g", "-ffreestanding", "-fno-stack-protector", "-fstack-size-section", "-c"])
        .arg(&src)
        .arg("-o")
        .arg(&obj))?;
    run(Command::new(&lld).args(["-flavor", "gnu", "-static", "-e", "_start", "-o"]).arg(&elf).arg(&obj))?;
    Ok((d, elf, objcopy))
}

fn ingest(dir: &Path, elf: &Path) -> butterfly_model::Evidence {
    let cov = dir.join("coverage.json");
    std::fs::write(&cov, export(&dir.join("t.c"))).unwrap();
    let opts = IngestOptions { coverage_json: cov, objects: vec![elf.to_path_buf()], source_root: Some(dir.to_path_buf()), ..Default::default() };
    butterfly_ingest::ingest(&opts).expect("ingest")
}

/// name -> (frame source, bounded), read back from the serialized evidence.
fn stack_of(json: &serde_json::Value) -> std::collections::BTreeMap<String, (String, bool)> {
    json["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap().to_string(), (s["frame"]["source"].as_str().unwrap().to_string(), s["stack"]["bounded"].as_bool().unwrap())))
        .collect()
}

const CAVEAT: &str = "the .stack_sizes section is malformed";

fn check(target: &str, addr_len: usize) {
    let (dir, elf, objcopy) = match build(target) {
        Ok(b) => b,
        Err(why) if fixtures_required() => panic!(
            "{target}: this fail-closed case must run where the fixtures are required (#242), \
             but its toolchain is missing so it would otherwise skip and count as a pass -- and a \
             skipped target proves nothing about failing closed at this width/byte order. Install \
             the ELF cross target and rustup llvm-tools, or set BUTTERFLY_REQUIRE_FIXTURES=0. {why}"
        ),
        Err(why) => {
            eprintln!("skipping: {target}: {why}");
            return;
        }
    };

    // Clean section: every function's frame comes from it, which is only
    // possible if its addresses decoded at this width and byte order.
    let ev = ingest(&dir, &elf);
    let json = serde_json::to_value(&ev).unwrap();
    let clean = stack_of(&json);
    for f in ["leaf", "mid", "_start"] {
        assert_eq!(clean.get(f).map(|x| x.0.as_str()), Some("stackSizesSection"), "{target}: {f} should take its frame from .stack_sizes: {clean:?}");
        assert!(clean[f].1, "{target}: {f} is bounded on clean metadata: {clean:?}");
    }
    assert!(!ev.provenance.caveats.iter().any(|c| c.contains(CAVEAT)), "{target}: no malformed caveat on a clean section");

    // Keep the first entry, then cut the second off one byte short of a
    // whole address.
    let orig = dir.join("orig.bin");
    run(Command::new(&objcopy).arg(format!("--dump-section=.stack_sizes={}", orig.display())).arg(&elf).arg(dir.join("scratch.elf"))).unwrap();
    let orig = std::fs::read(&orig).unwrap();
    let first = addr_len + orig[addr_len..].iter().position(|b| b & 0x80 == 0).unwrap() + 1;
    let mut bad = orig[..first].to_vec();
    bad.extend_from_slice(&orig[first..first + addr_len - 1]);
    let bad_path = dir.join("bad.bin");
    std::fs::write(&bad_path, &bad).unwrap();
    let bad_elf = dir.join("bad.elf");
    run(Command::new(&objcopy).arg(format!("--update-section=.stack_sizes={}", bad_path.display())).arg(&elf).arg(&bad_elf)).unwrap();

    let ev = ingest(&dir, &bad_elf);
    let json = serde_json::to_value(&ev).unwrap();
    let got = stack_of(&json);

    // The invalid-metadata outcome is in the serialized provenance ...
    let caveats: Vec<&str> = json["provenance"]["caveats"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
    assert!(caveats.iter().any(|c| c.contains(CAVEAT)), "{target}: malformed caveat missing: {caveats:?}");
    // ... and it agrees with the result: the entry decoded before the
    // corruption still stands, but everything that lost its entry -- and
    // every caller above it -- is not bounded, whatever the prologue says.
    assert_eq!(got["leaf"], ("stackSizesSection".to_string(), true), "{target}: the entry before the corruption is still valid: {got:?}");
    for f in ["mid", "_start"] {
        assert_ne!(got[f].0, "stackSizesSection", "{target}: {f}'s entry was cut off: {got:?}");
        assert!(!got[f].1, "{target}: {f} must fail closed after malformed .stack_sizes: {got:?}");
    }
    assert!(got.values().all(|(src, bounded)| src != "prologue" || !bounded), "{target}: no prologue estimate may read as bounded: {got:?}");

    // The report carries the same verdict to the reader.
    let html = butterfly_view::report::render(&ev, &Default::default(), None, None);
    assert!(html.contains(CAVEAT), "{target}: the report must show the malformed caveat");
    let stack = html.split("Stack usage (§6.3.4.f)").nth(1).and_then(|s| s.split("</table>").next()).unwrap_or_else(|| panic!("{target}: no stack table"));
    let row = |f: &str| stack.split("<tr>").find(|r| r.starts_with(&format!("<td class=mono>{f}</td>"))).unwrap_or_else(|| panic!("{target}: no stack row for {f}")).to_string();
    assert!(row("leaf").contains("class=\"ok\">yes<"), "{target}: leaf row: {}", row("leaf"));
    for f in ["mid", "_start"] {
        assert!(row(f).contains("class=\"warn\">"), "{target}: {f} must render not bounded: {}", row(f));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn elf64_little_endian() {
    check("aarch64-linux-gnu", 8);
}

#[test]
fn elf64_big_endian() {
    check("aarch64_be-linux-gnu", 8);
}

#[test]
fn elf32_little_endian() {
    check("armv7-linux-gnueabi", 4);
}

#[test]
fn elf32_big_endian() {
    check("armebv7-linux-gnueabi", 4);
}
