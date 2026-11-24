// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! End-to-end checks of the Rust rows of `support/matrix.toml`, on the real
//! `examples/rust` and `examples/rust-lib` builds (#152).
//!
//! Until these two examples were built by `.github/workflows/ci.yml` and
//! `scripts/ci-local.sh`, every Rust row that leaned on them said "not built
//! in CI" and "no test reads it". Building them is half of the fix; the other
//! half is a test that says what butterfly actually does with each construct,
//! so a row's claim is something a run can contradict. Each test below names
//! the matrix row it is the `proof` of.
//!
//! What is asserted is what holds of the example's *source* and of the two
//! toolchains' documented behaviour, not what one host's code generator
//! happened to do: no instruction counts, no frame sizes, no gap totals.
//!
//! Three assertions are narrower than the rest, and say so where they are
//! made: how a function-pointer table and an atomic static are linked to the
//! functions that use them is read out of the linked image's data and
//! instruction operands, and that has been verified on macOS/AArch64 and
//! x86-64 Linux only -- the latter since a PIE's data pointers, which the
//! file holds as zero beside an `R_X86_64_RELATIVE` relocation, are read
//! through it (#325 G1). On any other host those assertions are not made
//! (the rest of each test still is), and the matrix rows they back say the
//! same in their caveats.
//!
//! Both examples are required where the fixtures are (#227, #242): absent,
//! a suite fails rather than returning without asserting anything.
use butterfly_model::*;
use std::path::{Path, PathBuf};

/// `dir` is the example's path from the repository root, spelt out at each
/// call so the support-matrix check can see which example a test reads.
fn fixture(dir: &str) -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(dir);
    if root.join("build/coverage.json").exists() {
        Some(root)
    } else {
        butterfly_testsupport::fixture_missing(&format!("{dir}/build not present (run {dir}/build.sh; it needs a nightly with llvm-tools)"));
        None
    }
}

fn ingest(root: &Path) -> Evidence {
    // Discovery, as the README tells a reader to run it: the instrumented
    // binary must not be the image under analysis.
    let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts.source_root = Some(root.to_path_buf());
    butterfly_ingest::ingest(&opts).expect("ingest")
}

fn func<'a>(ev: &'a Evidence, name: &str) -> &'a Function {
    ev.functions.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no function {name}; have {:?}", ev.functions.iter().map(|f| &f.name).collect::<Vec<_>>()))
}

fn symbol_of<'a>(ev: &'a Evidence, f: &Function) -> &'a Symbol {
    let id = f.symbol_id.unwrap_or_else(|| panic!("{} has no symbol in the analysed image", f.name));
    &ev.symbols[id]
}

/// On the hosts these links were verified on. Elsewhere they are not
/// asserted, and the rows say so; see the module comment.
const LINKS_VERIFIED_HERE: bool = cfg!(any(all(target_os = "macos", target_arch = "aarch64"), all(target_os = "linux", target_arch = "x86_64")));

// ───────────────────────────────────────────────────────────── examples/rust

/// `rust.demangling`. The coverage export names functions as the nightly
/// compiler mangled them; the image was built by the stable one, whose crate
/// hash differs. The two meet only through the demangled path -- so a
/// demangler that got the path wrong would not show a wrong name, it would
/// lose the function's object code altogether.
#[test]
fn rust_symbols_are_demangled_and_join_across_the_two_compilers() {
    let Some(root) = fixture("examples/rust") else { return };
    let ev = ingest(&root);
    for name in ["demo::clamp_altitude", "demo::mode_permits_engage", "demo::phase_name", "demo::checksum", "demo::average", "demo::main"] {
        let f = func(&ev, name);
        assert!(f.mangled.starts_with("_R") || f.mangled.starts_with("_ZN"), "{name}: the coverage export's name should be a Rust mangling, got {:?}", f.mangled);
        let s = symbol_of(&ev, f);
        assert_eq!(s.name, f.name, "{name}: the image's symbol demangles to a different path");
        let (from_cov, from_image) = (f.mangled.trim_start_matches('_'), s.mangled.trim_start_matches('_'));
        assert_ne!(
            from_cov, from_image,
            "{name}: the nightly-built coverage and the stable-built image carry the same mangled name. They are two compilers with two crate hashes; if the names now agree, the join below is no longer exercising demangling"
        );
        assert!(f.object.is_some(), "{name}: executed, so it has object code in the image");
    }
    // No display name leaks a mangling or a legacy hash suffix.
    for f in &ev.functions {
        assert!(!f.name.starts_with("_R") && !f.name.starts_with("_ZN") && !f.name.contains("::h0") && !f.name.contains('$'), "{:?} is not fully demangled", f.name);
    }
}

/// `rust.branch-coverage`. An `if` gives branch regions, with the direction
/// of each outcome; a `match` gives none -- rustc maps its arms as regions,
/// so a `match` is covered arm by arm as lines, and nothing more is claimed
/// for it. `rust.mcdc`: no decision records at all.
#[test]
fn if_conditions_are_branch_regions_and_match_arms_are_lines() {
    let Some(root) = fixture("examples/rust") else { return };
    let ev = ingest(&root);

    let clamp = func(&ev, "demo::clamp_altitude");
    let lines: Vec<u32> = clamp.branches.iter().map(|b| b.line).collect();
    assert_eq!(lines, [10, 13], "one branch region per `if` in clamp_altitude");
    // main() calls it with -100, 500 and 90_000: `alt < lo` is true once.
    assert_eq!((clamp.branches[0].true_count, clamp.branches[0].false_count), (1, 2));
    assert!(clamp.branches.iter().all(|b| b.direction_known), "a Rust branch region says which outcome is the true one");

    // `(gear_down && speed_ok) || wow`: one region per condition
    // (-Zcoverage-options=condition), and no MC/DC to go with them.
    let engage = func(&ev, "demo::mode_permits_engage");
    assert_eq!(engage.branches.len(), 3, "one branch region per condition");

    let phase = func(&ev, "demo::phase_name");
    assert!(phase.branches.is_empty(), "rustc emits no branch region for a match arm; if it now does, rust.branch-coverage can claim match outcomes: {:?}", phase.branches);
    assert!(phase.lines_covered > 0 && phase.lines_covered < phase.lines_total, "the arms main() never selects are uncovered lines: {}/{}", phase.lines_covered, phase.lines_total);

    assert!(ev.functions.iter().all(|f| f.decisions.is_empty()), "rustc supplies no MC/DC, and none may be inferred");
    assert_eq!((ev.summary.branches_covered, ev.summary.branches_total), (11, 14));
}

/// `toolchain.rustc-nightly` and `toolchain.rustc-stable`: nightly writes
/// the coverage, stable builds the image that is analysed, and the build
/// record says which is which.
#[test]
fn nightly_supplies_the_coverage_and_stable_builds_the_analysed_image() {
    let Some(root) = fixture("examples/rust") else { return };
    let ev = ingest(&root);
    let info = &ev.provenance.build_info;
    let compiler = info.get("compiler").expect("build-info.txt records the image's compiler");
    let instr = info.get("compiler_instr").expect("build-info.txt records the coverage compiler");
    assert!(compiler.starts_with("rustc ") && !compiler.contains("nightly"), "the analysed image is the pinned stable toolchain's: {compiler}");
    assert!(instr.contains("nightly"), "branch regions need a nightly: {instr}");
    assert!(info.get("flags_instr").is_some_and(|f| f.contains("-Zcoverage-options=condition")));
    assert!(info.get("flags_target").is_some_and(|f| !f.contains("instrument-coverage")), "the analysed image is not instrumented");

    let objects: Vec<&str> = ev.provenance.inputs.iter().filter(|i| i.role == "object").map(|i| i.path.as_str()).collect();
    assert_eq!(objects.len(), 1, "one image under analysis: {objects:?}");
    assert!(objects[0].ends_with("build/demo") || objects[0].ends_with("build/demo.exe"), "the stable image, not demo-instr: {objects:?}");

    // Every function the run executed has object code in the stable image,
    // and the one nobody calls has none: rustc dropped it (see the README).
    for f in ev.functions.iter().filter(|f| f.count > 0) {
        assert!(f.object.is_some(), "{} ran but has no object code in the analysed image", f.name);
    }
    let legacy = func(&ev, "demo::legacy_scale");
    assert_eq!(legacy.count, 0);
    assert!(legacy.object.is_none(), "legacy_scale is never called and rustc emits no code for it");
}

/// The profile is read by the coverage compiler's own LLVM tools, and the
/// build record says which (#289). The builds used to put nightly's tool
/// directory first on PATH and call `llvm-profdata` by name, so a nightly
/// without llvm-tools quietly handed the profile to the pinned stable's
/// LLVM -- or to any other on PATH.
#[test]
fn the_coverage_is_read_by_the_nightlys_own_llvm_tools() {
    for name in ["examples/rust", "examples/rust-lib", "examples/rust-constructs"] {
        let Some(root) = fixture(name) else { continue };
        let info = std::fs::read_to_string(root.join("build/build-info.txt")).unwrap();
        let field = |k: &str| info.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
        let llvm_bin = PathBuf::from(field("llvm_bin"));
        assert!(llvm_bin.is_absolute(), "{name}: build-info.txt records the LLVM that read the profile: {info}");
        // `rustc 1.99.0-nightly (...)` -> its llvm-tools say `-rust-1.99.0-nightly`
        let release = field("compiler_instr").split_whitespace().nth(1).unwrap_or("").to_string();
        let v = std::process::Command::new(llvm_bin.join("llvm-profdata")).arg("--version").output().expect("run the recorded llvm-profdata");
        let v = String::from_utf8_lossy(&v.stdout);
        assert!(!release.is_empty() && v.contains(&format!("rust-{release}")), "{name}: the reader must be {release}'s own, not: {v}");
        // and it names each tool that produced coverage.json, by path and
        // version, under the keys `butterfly build` writes (#298)
        assert!(field("compiler_instr_path").starts_with('/'), "{name}: records the compiler that wrote the profile: {info}");
        // the nightly's own rustc, in the toolchain whose llvm-tools read the
        // profile -- not rustup's proxy, which is the same file for every
        // toolchain and so names none of them (#305)
        let rustc = PathBuf::from(field("compiler_instr_path"));
        let toolchain = llvm_bin.ancestors().nth(4).expect("<sysroot>/lib/rustlib/<host>/bin");
        assert_eq!(rustc, toolchain.join("bin/rustc"), "{name}: records the nightly's own rustc, not a proxy: {info}");
        let v = std::process::Command::new(&rustc).arg("-V").output().expect("run the recorded rustc");
        assert_eq!(String::from_utf8_lossy(&v.stdout).trim(), field("compiler_instr"), "{name}: the recorded path is the compiler the version names");
        for tool in ["llvm-profdata", "llvm-cov"] {
            let key = tool.replace('-', "_");
            assert_eq!(PathBuf::from(field(&format!("{key}_path"))), llvm_bin.join(tool), "{name}: records the {tool} that ran: {info}");
            assert!(field(&key).contains(&format!("rust-{release}")), "{name}: records {tool}'s version: {info}");
        }
    }
}

/// `rust.runtime-check`. The division in `average` carries rustc's
/// divide-by-zero and overflow checks. They are classified as runtime
/// checks, not as dead code and not as project code -- and, from the
/// projection alone, they read as covered because their line ran. That last
/// part is the row's caveat, pinned here so it cannot be quietly lost.
#[test]
fn the_division_checks_in_average_are_classified_as_runtime_checks() {
    let Some(root) = fixture("examples/rust") else { return };
    let ev = ingest(&root);
    let avg = func(&ev, "demo::average");
    let obj = avg.object.as_ref().expect("average has object code");
    assert!(obj.compiler_generated.get("runtime-check").copied().unwrap_or(0) > 0, "no runtime-check instructions in average: {:?}", obj.compiler_generated);

    let checks: Vec<&Gap> = obj.gaps.iter().filter(|g| g.kind == GapKind::HelperCall && g.class.as_deref() == Some("runtime-check")).collect();
    assert!(
        checks.iter().any(|g| g.description.contains("div_by_zero")),
        "the divide-by-zero panic call is a runtime-check helper call: {:?}",
        checks.iter().map(|g| &g.description).collect::<Vec<_>>()
    );
    assert!(checks.iter().all(|g| g.line == Some(63)), "the checks belong to `sum / v.len()` on line 63");

    // The projection's view: every runtime-check instruction sits on a line
    // that ran, so it is projected covered -- though no input divides by
    // zero. Only a trace says otherwise (rust.panic-path-measured).
    let rc: Vec<&Instr> = obj.blocks.iter().flat_map(|b| b.instrs.iter()).filter(|i| i.origin.as_deref() == Some("runtime-check")).collect();
    assert!(!rc.is_empty());
    assert!(
        rc.iter().all(|i| i.projected == Status::Covered),
        "a runtime-check instruction is no longer projected covered by its line; rust.runtime-check's caveat says it is: {:?}",
        rc.iter().map(|i| (i.line, i.projected)).collect::<Vec<_>>()
    );
}

/// The one closure in this example, `|&x| x as i64` in `average`, is a
/// function of its own in both the coverage and the image, under the name
/// rustc gives it, and the enclosing function's inventory counts it. The
/// `rust.closure` row is proved on `examples/rust-constructs` (#167), which
/// has closures that capture; this one stays as the simple case.
#[test]
fn the_closure_in_average_is_a_function_of_its_own() {
    let Some(root) = fixture("examples/rust") else { return };
    let ev = ingest(&root);
    let closure = func(&ev, "demo::average::{closure#0}");
    assert_eq!(closure.line_start, 62, "the closure is on average's line 62");
    assert!(closure.count > 0, "average maps it over a non-empty slice");
    assert_eq!((closure.lines_covered, closure.lines_total), (1, 1));
    assert!(closure.object.is_some(), "an executed closure has object code of its own");
    assert_eq!(symbol_of(&ev, closure).name, closure.name);
    assert_eq!(func(&ev, "demo::average").constructs.get("closure"), Some(&1), "average's inventory counts the one closure");
}

// ───────────────────────────────────────────────────────── examples/rust-lib

/// `rust.prologue-frame`. rustc writes no `.su` file, so every Rust frame
/// is read from the prologue -- and says so in its `source`, so a reader
/// can tell an estimate from a compiler-reported size.
#[test]
fn every_rust_frame_is_a_prologue_estimate_labelled_as_one() {
    let Some(root) = fixture("examples/rust-lib") else { return };
    let ev = ingest(&root);
    let has_su = std::fs::read_dir(root.join("build")).unwrap().flatten().any(|e| e.path().extension().is_some_and(|x| x == "su"));
    assert!(!has_su, "rustc writes no .su file; if this build now has one, the row is out of date");
    let mut seen = 0;
    for f in ev.functions.iter().filter(|f| f.symbol_id.is_some()) {
        let s = symbol_of(&ev, f);
        assert_eq!(s.frame.source, FrameSource::Prologue, "{}: frame source {:?}", f.name, s.frame.source);
        seen += 1;
    }
    assert!(seen >= 10, "most of the crate's functions are in the image, found {seen}");
    assert!(symbol_of(&ev, func(&ev, "main::main")).frame.bytes > 0, "main has locals, so its prologue reserves a frame");
}

/// `rust.atomic-global`. The atomics in `state.rs` are globals of the
/// image. At -Copt-level=0 every access is a call into `core::sync::atomic`
/// with the static's address, so what butterfly records is which functions
/// take that address -- not a read or a write.
#[test]
fn the_atomic_statics_are_globals_of_the_data_coupling() {
    let Some(root) = fixture("examples/rust-lib") else { return };
    let ev = ingest(&root);
    let names: Vec<&str> = ev.globals.iter().map(|g| g.name.as_str()).collect();
    for want in ["main::state::ALTITUDE", "main::state::PHASE", "main::state::FAULTS", "main::sched::DISPATCHES"] {
        assert!(names.contains(&want), "{want} is not a global of the image: {names:?}");
    }
    if !LINKS_VERIFIED_HERE {
        eprintln!("e2e_rust_constructs: the atomic-to-function links are verified on macOS/AArch64 and x86-64 Linux only; not asserted on this host");
        return;
    }
    let alt = ev.globals.iter().find(|g| g.name == "main::state::ALTITUDE").unwrap();
    let mut takers: Vec<&str> = alt.address_taker_symbols.iter().map(|&s| ev.symbols[s].name.as_str()).collect();
    takers.sort_unstable();
    assert_eq!(takers, ["main::state::altitude", "main::state::reset", "main::state::set_altitude"]);
    assert!(
        alt.reader_symbols.is_empty() && alt.writer_symbols.is_empty(),
        "at -Copt-level=0 the access is inside core::sync::atomic, so no project function reads or writes it directly"
    );
}

/// `rust.fn-pointer-table`. `sched::run` calls through `TASKS`; the call is
/// an indirect branch, and the tasks it may reach are never called directly.
#[test]
fn the_task_table_dispatch_is_an_indirect_call() {
    let Some(root) = fixture("examples/rust-lib") else { return };
    let ev = ingest(&root);
    let run = func(&ev, "main::sched::run");
    let [site] = &run.indirect_calls[..] else { panic!("one indirect call in run: {:?}", run.indirect_calls.len()) };
    assert_eq!(site.line, Some(40), "the call through TASKS[idx]");
    assert!(run.object.as_ref().unwrap().gaps.iter().any(|g| g.kind == GapKind::IndirectBranch), "the call is an indirect-branch gap");
    for t in ["main::sched::task_climb", "main::sched::task_hold", "main::sched::task_trim"] {
        assert!(func(&ev, t).callers.is_empty(), "{t} is reached only through the table");
    }
    if !LINKS_VERIFIED_HERE {
        eprintln!("e2e_rust_constructs: the table-to-target links are verified on macOS/AArch64 and x86-64 Linux only; not asserted on this host");
        return;
    }
    let may: Vec<&str> = site.candidates.iter().map(|c| c.name.as_str()).collect();
    for t in ["main::sched::task_climb", "main::sched::task_hold", "main::sched::task_trim"] {
        assert!(may.contains(&t), "{t} is in TASKS, so it is a candidate of the call");
        assert!(func(&ev, t).pointer_callers.iter().any(|p| p.kind == "data" && p.name == "main::sched::TASKS"), "{t} is held by the TASKS table");
    }
    // The row's caveat: a table's extent is its symbol's size. An ELF symbol
    // has one, so the candidates are exactly the three in TASKS. A Mach-O
    // symbol has none, so TASKS is read up to the next data symbol and takes
    // in the std vtables after it: over-approximate. When either changes,
    // the caveat is out of date.
    let format = &ev.image.as_ref().expect("an analysed image").format;
    if format.contains("MachO") {
        assert!(may.len() > 3, "the candidate set is no longer over-approximate on Mach-O ({may:?}); rust.fn-pointer-table's caveat says it is");
    } else {
        assert_eq!(may.len(), 3, "{format}: the candidates are TASKS' three entries, read to its symbol's size ({may:?}); rust.fn-pointer-table's caveat says so");
    }
}
