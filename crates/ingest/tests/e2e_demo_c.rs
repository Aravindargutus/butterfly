// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! End-to-end test on the C fixture. Runs only when the fixture has been
//! built (`examples/c/build.sh` needs clang + rustup llvm-tools); it is
//! skipped, not failed, otherwise, so `cargo test` stays green on machines
//! without a compiler.
use butterfly_model::*;
use std::path::PathBuf;

fn fixture() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    let build = root.join("build");
    if build.join("coverage.json").exists() && build.join("demo").exists() {
        Some(root)
    } else {
        butterfly_testsupport::fixture_missing("examples/c/build not present (run examples/c/build.sh)");
        None
    }
}

#[test]
fn demo_c_end_to_end() {
    let Some(root) = fixture() else { return };
    let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts.source_root = Some(root.clone());
    let ev = butterfly_ingest::ingest(&opts).expect("ingest");
    let names: Vec<&str> = ev.functions.iter().map(|f| f.name.as_str()).collect();
    for n in ["clamp_altitude", "mode_permits_engage", "phase_name", "checksum", "average", "legacy_scale", "main"] {
        assert!(names.contains(&n), "missing {n}");
    }
    let f = |n: &str| ev.functions.iter().find(|f| f.name == n).unwrap();
    // coverage facts from the fixture's run
    assert_eq!(f("legacy_scale").count, 0);
    assert_eq!(f("mode_permits_engage").decisions.len(), 1);
    let d = &f("mode_permits_engage").decisions[0];
    assert_eq!(d.conditions.len(), 3);
    assert!(!d.mcdc_covered, "C2 has no independence pair in the fixture run");
    assert_eq!(d.conditions.iter().filter(|c| c.independence_covered).count(), 2);
    // requirement tags
    assert_eq!(f("checksum").requirements, vec!["LLR-CRC-004".to_string()]);
    // object code present and matched
    assert!(f("main").object.is_some());
    let callees: Vec<&str> = f("main").callees.iter().map(|e| e.symbol.as_str()).collect();
    for n in ["clamp_altitude", "mode_permits_engage", "checksum", "average", "printf"] {
        assert!(callees.contains(&n), "main should call {n}: {callees:?}");
    }
    // the never-taken `return 0` path in average must not be projected covered
    let avg = f("average").object.as_ref().unwrap();
    assert!(avg.instr_projected_uncovered > 0, "average has an uncovered return path");
    // dead code: legacy_scale is never referenced
    let im = ev.image.as_ref().unwrap();
    assert!(im.dead_candidates.iter().any(|d| d.name == "legacy_scale"), "{:?}", im.dead_candidates.iter().map(|d| &d.name).collect::<Vec<_>>());
    // ... but op_double and op_negate are: OPS holds them, so they are the
    // candidates of apply_op's call, not dead code. In an x86-64 Linux PIE
    // OPS's words are R_X86_64_RELATIVE relocations the file holds as zero;
    // until #325 G1 both read as dead there, and the call had no candidates.
    for n in ["demo.c:op_double", "demo.c:op_negate"] {
        assert!(!im.dead_candidates.iter().any(|d| d.name == n), "{n} is held by OPS, so it is not dead code");
        assert!(f(n).pointer_callers.iter().any(|p| p.kind == "data" && p.name == "OPS"), "{n} is held by the OPS table");
    }
    let may: Vec<&str> = f("apply_op").indirect_calls.iter().flat_map(|s| s.candidates.iter().map(|c| c.name.as_str())).collect();
    assert!(may.contains(&"op_double") && may.contains(&"op_negate"), "the call through OPS may reach either entry: {may:?}");
    // every gap key is unique
    let mut keys = std::collections::HashSet::new();
    for g in ev.functions.iter().flat_map(|f| f.object.iter().flat_map(|o| o.gaps.iter())) {
        assert!(keys.insert(g.key.clone()), "duplicate gap key {}", g.key);
    }
    // provenance hashes every input
    assert!(ev.provenance.inputs.iter().any(|i| i.role == "coverage-json"));
    assert!(ev.provenance.inputs.iter().any(|i| i.role == "object"));
    assert!(ev.provenance.inputs.iter().all(|i| i.sha256.len() == 64));
    // round-trips through JSON
    let json = serde_json::to_string(&ev).unwrap();
    let back: Evidence = serde_json::from_str(&json).unwrap();
    assert_eq!(back.functions.len(), ev.functions.len());
}

/// The support matrix row `feature.object` claims "every instruction with its
/// source line and projected coverage", and `demo_c_end_to_end` asserted only
/// that object code is there and that `average` has some uncovered
/// instruction (#200). This reads `average` instruction by instruction: every
/// instruction that carries a line of `demo.c` carries one inside the
/// function, the never-taken `return 0;` has instructions of its own and
/// every one of them is projected uncovered, and the loop body the run did
/// execute is projected covered.
#[test]
fn every_instruction_of_average_carries_its_source_line_and_projected_status() {
    let Some(root) = fixture() else { return };
    let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts.source_root = Some(root.clone());
    let ev = butterfly_ingest::ingest(&opts).expect("ingest");
    let f = ev.functions.iter().find(|f| f.name == "average").expect("average");
    let src = std::fs::read_to_string(root.join("demo.c")).unwrap();
    let lines: Vec<&str> = src.lines().collect();
    let line_of = |from: u32, text: &str| -> u32 {
        let i = (from as usize - 1..lines.len()).find(|&i| lines[i].trim() == text).unwrap_or_else(|| panic!("demo.c has no line {text:?} after L{from}"));
        u32::try_from(i + 1).unwrap()
    };
    let ret0 = line_of(f.line_start, "return 0;");
    let body = line_of(f.line_start, "sum += v[i];");
    let obj = f.object.as_ref().expect("average has object code");
    let ins: Vec<&Instr> = obj.blocks.iter().flat_map(|b| b.instrs.iter()).collect();
    assert_eq!(ins.len(), obj.instr_total, "the blocks hold every instruction the totals count");
    for i in &ins {
        if let Some(l) = i.line.filter(|&l| l != 0) {
            if i.file_id.is_none_or(|id| id == f.file_id) {
                assert!((f.line_start..=f.line_end).contains(&l), "{:#x} {} carries L{l}, outside average (L{}-L{})", i.addr, i.mnemonic, f.line_start, f.line_end);
            }
        }
    }
    let on = |l: u32| ins.iter().filter(move |i| i.line == Some(l)).collect::<Vec<_>>();
    let r = on(ret0);
    assert!(!r.is_empty(), "the `return 0;` at L{ret0} has instructions of its own");
    assert!(
        r.iter().all(|i| i.projected == Status::Uncovered),
        "the never-taken `return 0;` at L{ret0} is projected uncovered: {:?}",
        r.iter().map(|i| (i.addr, i.projected)).collect::<Vec<_>>()
    );
    let b = on(body);
    assert!(
        !b.is_empty() && b.iter().all(|i| i.projected == Status::Covered),
        "the executed loop body at L{body} is projected covered: {:?}",
        b.iter().map(|i| (i.addr, i.projected)).collect::<Vec<_>>()
    );
}

/// The support matrix row `feature.provenance` claims "every input named and
/// hashed, with the caveats that apply", and `demo_c_end_to_end` checked only
/// that each LISTED input has a hash and that two roles are there (#200).
/// This checks the list against what the build directory holds: the coverage
/// export, the one source, the config beside it, the build record, every
/// stack-usage file, the object -- and that the projection caveat, the one
/// that applies to every evidence file without a trace, is carried.
#[test]
fn provenance_names_every_input_of_the_example_and_carries_the_projection_caveat() {
    let Some(root) = fixture() else { return };
    let build = root.join("build");
    let mut opts = butterfly_ingest::discover(&build).expect("discover");
    opts.source_root = Some(root.clone());
    let ev = butterfly_ingest::ingest(&opts).expect("ingest");
    let p = &ev.provenance;
    let has = |role: &str, tail: &str| p.inputs.iter().any(|i| i.role == role && i.path.ends_with(tail));
    for (role, tail) in [("coverage-json", "coverage.json"), ("source", "demo.c"), ("config", "butterfly.toml"), ("build-info", "build-info.txt"), ("object", "build/demo")] {
        assert!(has(role, tail), "provenance names no {role} input ending in {tail}: {:?}", p.inputs.iter().map(|i| (&i.role, &i.path)).collect::<Vec<_>>());
    }
    let su: Vec<String> = std::fs::read_dir(&build).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".su")).collect();
    assert!(!su.is_empty(), "examples/c/build.sh collects its .su files into build/");
    for n in &su {
        assert!(has("stack-usage", n), "provenance does not name the stack-usage file {n}");
    }
    assert!(p.inputs.iter().all(|i| i.sha256.len() == 64 && i.sha256.bytes().all(|c| c.is_ascii_hexdigit())), "every input is hashed");
    assert!(p.caveats.iter().any(|c| c.contains("Object-code coverage is PROJECTED")), "the projection caveat applies to evidence with no trace: {:?}", p.caveats);
    assert!(ev.measured.is_none(), "examples/c imports no trace");
}

/// A file butterfly cannot read is not an empty trace.
///
/// `trace::parse` labels it `unrecognised` and returns no counts, and the
/// measured path over no counts is not "nothing measured": it is every
/// instruction reported as executed zero times, and a ProjectedNotExecuted
/// disagreement for every instruction the projection covers. On this fixture
/// that reads as a full measured verdict -- hundreds of instructions
/// "projected covered and never run" -- derived from a file the tool did not
/// understand. Measured coverage is evidence; a false one is the worst thing
/// this tool can produce, so `--trace` is refused instead (self-review of #208).
#[test]
fn an_unrecognised_trace_is_refused_rather_than_measured_as_zeros() {
    let Some(root) = fixture() else { return };
    let dir = butterfly_testsupport::scratch_dir("bad-trace");
    let p = dir.join("trace.log");

    let ingest_with = |text: &str| {
        std::fs::write(&p, text).unwrap();
        let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
        opts.source_root = Some(root.clone());
        opts.trace = Some(p.clone());
        butterfly_ingest::ingest(&opts)
    };

    for junk in ["<html><body>404 Not Found</body></html>\n", "", "no addresses here at all\n"] {
        let e = ingest_with(junk).expect_err("an unrecognised trace must be refused");
        let msg = e.to_string();
        assert!(msg.contains("not an execution trace butterfly recognises"), "{msg}");
        assert!(msg.contains("No measured coverage was produced"), "{msg}");
    }

    // ...and a format it does understand is still read: the refusal is about
    // not knowing, not about the trace being unhelpful
    let ev = ingest_with("0x1000 3\n").expect("an address list is a trace");
    let m = ev.measured.as_ref().expect("measured coverage from a recognised trace");
    assert_eq!(m.format, "address list");

    let _ = std::fs::remove_dir_all(&dir);
}
