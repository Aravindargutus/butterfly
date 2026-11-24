// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! What a build says it did is held against what it delivered (#245).
//!
//! `build-info.txt` records the analysis build's flags, and a build that
//! declares `-fstack-usage` or `-g` has promised the evidence something: a
//! compiler-stated frame for every function, a line table for every
//! instruction. An adapter that lost a `.su` file -- Bazel's sandbox
//! discarding it, two objects' `.su` flattened onto one name -- or that
//! stripped the image, used to produce a bundle that looked complete: the
//! missing frames were quietly estimated from prologues, the missing lines
//! became a caveat. Declared and delivered now have to agree, or ingest
//! fails naming what is missing. The checks live in ingest rather than in
//! each adapter, so every adapter -- shell, Make, CMake, Bazel -- is held to
//! them at once.
//!
//! Runs on a copy of the `examples/c-lib` build, which CI provisions.

use std::fs;
use std::path::{Path, PathBuf};

fn example() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-lib").canonicalize().ok()?;
    if root.join("build/coverage.json").exists() && root.join("build/flight").exists() {
        Some(root)
    } else {
        butterfly_testsupport::fixture_missing("run examples/c-lib/build.sh first");
        None
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type().unwrap().is_dir() {
            copy_tree(&src, &dst);
        } else {
            fs::copy(&src, &dst).unwrap();
        }
    }
}

/// A private copy of the example's build directory.
fn staged(tag: &str) -> Option<(PathBuf, PathBuf)> {
    let root = example()?;
    let d = butterfly_testsupport::scratch_dir(&format!("modes-{tag}"));
    copy_tree(&root.join("build"), &d);
    Some((root, d))
}

fn ingest(root: &Path, build: &Path) -> anyhow::Result<butterfly_model::Evidence> {
    let mut opts = butterfly_ingest::discover(build)?;
    opts.source_root = Some(root.to_path_buf());
    butterfly_ingest::ingest(&opts)
}

/// The `.su` in `build` that states `function`'s frame -- found by what it
/// says, since the file names are the adapter's choice.
fn su_for(build: &Path, function: &str) -> PathBuf {
    fs::read_dir(build)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension().is_some_and(|x| x == "su")
                && fs::read_to_string(p).unwrap().lines().any(|l| l.split('\t').next().is_some_and(|loc| loc.ends_with(&format!(":{function}"))))
        })
        .unwrap_or_else(|| panic!("no .su in {} states {function}", build.display()))
}

fn set_flags_target(build: &Path, flags: &str) {
    let p = build.join("build-info.txt");
    let t: String = fs::read_to_string(&p).unwrap().lines().map(|l| if l.starts_with("flags_target=") { format!("flags_target={flags}\n") } else { format!("{l}\n") }).collect();
    fs::write(&p, t).unwrap();
}

#[test]
fn the_documented_build_is_accepted() {
    let Some((root, d)) = staged("control") else { return };
    let ev = ingest(&root, &d).expect("the example as built is consistent");
    assert!(!ev.provenance.caveats.iter().any(|c| c.contains("-fstack-usage")), "{:?}", ev.provenance.caveats);
    let _ = fs::remove_dir_all(d);
}

/// A `.su` the build promised is gone: the functions it described would
/// have been silently estimated from their prologues instead.
#[test]
fn a_missing_stack_usage_file_fails_when_the_build_declares_stack_usage() {
    let Some((root, d)) = staged("missing-su") else { return };
    fs::remove_file(su_for(&d, "crc_block")).unwrap();
    let e = ingest(&root, &d).unwrap_err().to_string();
    assert!(e.contains("crc_block"), "it names the function left without a stated frame: {e}");
    assert!(e.contains("-fstack-usage"), "and the promise it breaks: {e}");
    let _ = fs::remove_dir_all(d);
}

/// Two objects' `.su` flattened onto one name: the second overwrote the
/// first. Whatever the adapter, the loser's functions have no record.
#[test]
fn a_stack_usage_file_overwritten_by_a_namesake_fails() {
    let Some((root, d)) = staged("collision") else { return };
    fs::copy(su_for(&d, "clamp_i32"), su_for(&d, "mode_advance")).unwrap();
    let e = ingest(&root, &d).unwrap_err().to_string();
    assert!(e.contains("mode_advance"), "{e}");
    let _ = fs::remove_dir_all(d);
}

/// The analysis build must carry debug information; a build-info that says
/// it did not is refused before anything is projected.
#[test]
fn an_analysis_build_without_debug_information_fails() {
    let Some((root, d)) = staged("no-g") else { return };
    set_flags_target(&d, "-std=c11 -Iinclude -O0 -fstack-usage");
    let e = ingest(&root, &d).unwrap_err().to_string();
    assert!(e.contains("-g"), "{e}");

    // the last -g flag is the one the compiler obeys
    set_flags_target(&d, "-std=c11 -g -Iinclude -O0 -fstack-usage -g0");
    let e = ingest(&root, &d).unwrap_err().to_string();
    assert!(e.contains("-g0") || e.contains("-g"), "{e}");
    let _ = fs::remove_dir_all(d);
}

/// Declared `-g`, delivered no line table: stripped, or on macOS the
/// `.dSYM` never made. Every instruction would have had no source line.
#[test]
fn a_declared_debug_build_whose_image_has_no_line_table_fails() {
    let Some((root, d)) = staged("stripped") else { return };
    if cfg!(target_os = "macos") {
        fs::remove_dir_all(d.join("flight.dSYM")).unwrap();
    } else {
        let objcopy = butterfly_ingest::objectcode::find_llvm_tool("llvm-objcopy", None).expect("llvm-objcopy");
        let st = std::process::Command::new(objcopy).arg("--strip-debug").arg(d.join("flight")).status().unwrap();
        assert!(st.success());
    }
    let e = ingest(&root, &d).unwrap_err().to_string();
    assert!(e.contains("line table") && e.contains("-g"), "{e}");
    let _ = fs::remove_dir_all(d);
}

/// `.su` files the recorded flags do not mention: the evidence is still
/// right, but the provenance does not describe the build that made it.
#[test]
fn stack_usage_the_build_info_does_not_declare_is_a_caveat() {
    let Some((root, d)) = staged("undeclared") else { return };
    set_flags_target(&d, "-std=c11 -Wall -Wextra -g -Iinclude -O0");
    let ev = ingest(&root, &d).expect("the evidence itself is complete");
    assert!(
        ev.provenance.caveats.iter().any(|c| c.contains("-fstack-usage") && c.contains("build-info")),
        "the mismatch between the files and the recorded flags is said: {:?}",
        ev.provenance.caveats
    );
    let _ = fs::remove_dir_all(d);
}

/// Every shell adapter CI builds -- single file, header-inline, per-test,
/// C++, library, and the library as delivered with its harness -- records the
/// compiler it really ran and is held to what it declares: take one of its
/// `.su` files away and ingest refuses the directory. (CMake, Make and Bazel
/// get the same altered-mode check in `scripts/verify-build-systems.sh`,
/// which builds them.)
#[test]
fn every_shell_adapter_records_its_compiler_and_is_held_to_its_stack_usage() {
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").canonicalize().unwrap();
    let adapters = ["c", "c-callgraph", "c-header-inline", "c-per-test", "c-lib", "cpp", "cpp-header-inline", "cpp-lib"].map(|n| (n, "build", "build.sh"));
    // the library deliverable (#286): archive + harness, its own directory
    for (example, out, script) in adapters.into_iter().chain([("c-lib", "build-lib", "build-lib.sh")]) {
        let name = format!("{example}/{out}");
        let root = examples.join(example);
        let build = root.join(out);
        if !build.join("build-info.txt").exists() {
            butterfly_testsupport::fixture_missing(&format!("run examples/{example}/{script} first"));
            continue;
        }
        let info = fs::read_to_string(build.join("build-info.txt")).unwrap();
        let field = |k: &str| info.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
        assert!(field("compiler_path").starts_with('/'), "{name}: the compiler that ran is recorded by absolute path: {info}");
        assert!(!field("target").is_empty(), "{name}: and the target it compiled for: {info}");
        // and the other tools that shaped the evidence, by the path that ran
        // (#289): the LLVM that read the profile, and on macOS the dsymutil
        // that made the line table. PATH chose them, so PATH is not an answer.
        let llvm_bin = PathBuf::from(field("llvm_bin"));
        assert!(llvm_bin.is_absolute() && llvm_bin.join("llvm-profdata").is_file(), "{name}: records the LLVM that read its profile: {info}");
        if cfg!(target_os = "macos") {
            let dsymutil = PathBuf::from(field("dsymutil_path"));
            assert!(dsymutil.is_absolute() && dsymutil.is_file(), "{name}: records the dsymutil that made its line table: {info}");
        }
        // and the tools that produced coverage.json, by path and version
        // (#298): the compiler that wrote the profile, and the llvm-profdata
        // and llvm-cov that read it -- the ones in llvm_bin
        assert!(field("compiler_instr_path").starts_with('/') && !field("compiler_instr").is_empty(), "{name}: records the compiler that wrote its profile: {info}");
        for tool in ["llvm-profdata", "llvm-cov"] {
            let key = tool.replace('-', "_");
            assert_eq!(PathBuf::from(field(&format!("{key}_path"))), llvm_bin.join(tool), "{name}: records the {tool} that ran: {info}");
            assert!(field(&key).chars().any(|c| c.is_ascii_digit()), "{name}: records the version of {tool}: {info}");
        }

        let d = butterfly_testsupport::scratch_dir(&format!("modes-each-{example}-{out}"));
        copy_tree(&build, &d);
        ingest(&root, &d).unwrap_or_else(|e| panic!("{name}: the example as built must be accepted: {e}"));
        let sus: Vec<PathBuf> = fs::read_dir(&d).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "su")).collect();
        if field("flags_target").split_whitespace().any(|f| f == "-fstack-usage") {
            assert!(!sus.is_empty(), "{name}: declares -fstack-usage and delivered no .su");
            fs::remove_file(&sus[0]).unwrap();
            let e = ingest(&root, &d).expect_err(&format!("{name}: accepted with {} removed", sus[0].display())).to_string();
            assert!(e.contains("-fstack-usage"), "{name}: refused for the missing .su: {e}");
        } else {
            assert!(sus.is_empty(), "{name}: writes .su files its build-info does not declare");
        }
        let _ = fs::remove_dir_all(d);
    }
}
