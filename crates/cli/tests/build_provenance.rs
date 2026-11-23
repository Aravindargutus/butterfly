// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly build` and `butterfly run` as one provenance boundary (#287).
//!
//! Every evidence file these commands leave behind must describe inputs from
//! the build that just ran: not a profile, `.su` file, executable or dSYM an
//! earlier run left in the output directory, not a compiler other than the
//! one that compiled, and not the evidence of an earlier run beside a failed
//! one.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_butterfly")
}

fn has_clang() -> bool {
    Command::new("clang").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// A scratch project holding the C example's source. `None` (after saying
/// why) when there is no clang to build it with.
fn project(tag: &str) -> Option<PathBuf> {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/demo.c");
    if !src.exists() {
        butterfly_testsupport::fixture_missing("examples/c/demo.c is not present");
        return None;
    }
    if !has_clang() {
        butterfly_testsupport::fixture_missing("clang is not installed");
        return None;
    }
    let d = butterfly_testsupport::scratch_dir(&format!("prov-{tag}"));
    fs::copy(&src, d.join("demo.c")).unwrap();
    Some(fs::canonicalize(d).unwrap())
}

/// A clang build config: `run` is the coverage run, `variants` are
/// `(name, flags)` pairs written as TOML arrays.
fn clang_config(dir: &Path, cc: &str, run: &str, variants: &[(&str, &str)]) {
    let mut t = format!(
        "[project]\nname = \"demo\"\nout_dir = \"variants\"\n\n[compiler]\nkind = \"clang\"\ncc = \"{cc}\"\nsources = [\"demo.c\"]\ncommon_flags = [\"-std=c11\", \"-g\"]\n\n\
         [coverage]\ncc = \"clang\"\nflags = [\"-O0\", \"-fprofile-instr-generate\", \"-fcoverage-mapping\"]\nrun = {run}\n"
    );
    for (n, f) in variants {
        t.push_str(&format!("\n[[variant]]\nname = \"{n}\"\nflags = {f}\n"));
    }
    fs::write(dir.join("butterfly-build.toml"), t).unwrap();
}

fn build(dir: &Path) -> Output {
    Command::new(bin()).arg("build").arg(dir.join("butterfly-build.toml")).output().expect("spawn butterfly")
}

fn ok(o: &Output) -> bool {
    o.status.success()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn evidence(p: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

/// A run command that exits 0 without writing a profile must fail the build,
/// not have the previous run's `default.profraw` merged in its place.
#[test]
fn a_run_that_writes_no_profile_does_not_reuse_the_last_one() {
    let Some(d) = project("profile") else { return };
    clang_config(&d, "clang", "[\"{exe}\"]", &[("O0", "[\"-O0\"]")]);
    let first = build(&d);
    assert!(ok(&first), "first build: {}", stderr(&first));

    clang_config(&d, "clang", "[\"true\"]", &[("O0", "[\"-O0\"]")]);
    let second = build(&d);
    assert!(!ok(&second), "a run that wrote no profile must fail the build, not merge the previous one");
    assert!(stderr(&second).contains("profile"), "the failure must name the missing profile: {}", stderr(&second));
    assert!(!d.join("variants/coverage.json").exists(), "the previous run's coverage export must not survive a failed build");
    assert!(!d.join("variants/evidence-O0.json").exists(), "the previous run's evidence must not survive a failed build");
    let _ = fs::remove_dir_all(d);
}

/// A `.su` file an earlier compile left in the variant directory -- from a
/// source since removed, or flags since changed -- is not this build's.
#[test]
fn a_stale_stack_usage_file_is_not_ingested() {
    let Some(d) = project("su") else { return };
    clang_config(&d, "clang", "[\"{exe}\"]", &[("O0", "[\"-O0\", \"-fstack-usage\"]")]);
    let first = build(&d);
    assert!(ok(&first), "first build: {}", stderr(&first));
    fs::write(d.join("variants/O0/removed.su"), "removed.c:1:1:ghost\t4096\tstatic\n").unwrap();

    let second = build(&d);
    assert!(ok(&second), "second build: {}", stderr(&second));
    let ev = evidence(&d.join("variants/evidence-O0.json"));
    let su: Vec<String> = ev["provenance"]["inputs"].as_array().unwrap().iter().filter(|i| i["role"] == "stack-usage").map(|i| i["path"].as_str().unwrap().to_string()).collect();
    assert!(!su.is_empty(), "this build's own .su file must still be read");
    assert!(su.iter().all(|p| !p.ends_with("removed.su")), "an earlier compile's .su file was ingested: {su:?}");
    let _ = fs::remove_dir_all(d);
}

/// A `custom` config: every build step is the operator's own command.
fn custom_config(dir: &Path, variant_cmd: &str) {
    let src = dir.join("demo.c");
    let t = format!(
        "[project]\nname = \"demo\"\nout_dir = \"variants\"\n\n[compiler]\nkind = \"custom\"\nsources = [\"demo.c\"]\noutput = \"demo\"\n\n\
         [coverage]\ncommand = [\"clang\", \"-g\", \"-O0\", \"-fprofile-instr-generate\", \"-fcoverage-mapping\", {src:?}, \"-o\", \"{{out}}\"]\nrun = [\"{{exe}}\"]\n\n\
         [[variant]]\nname = \"v\"\ncommand = {variant_cmd}\n"
    );
    fs::write(dir.join("butterfly-build.toml"), t).unwrap();
}

/// A custom command that exits 0 without writing `{out}` leaves the previous
/// run's executable there; build-info would then describe a command that did
/// not produce the image being analysed.
#[test]
fn a_build_step_that_writes_no_output_does_not_reuse_the_last_one() {
    let Some(d) = project("noout") else { return };
    let src = d.join("demo.c");
    custom_config(&d, &format!("[\"clang\", \"-g\", \"-O0\", {src:?}, \"-o\", \"{{out}}\"]"));
    let first = build(&d);
    assert!(ok(&first), "first build: {}", stderr(&first));

    custom_config(&d, "[\"sh\", \"-c\", \"exit 0\", \"-g\", \"{out}\"]");
    let second = build(&d);
    assert!(!ok(&second), "a variant command that wrote nothing must fail the build");
    assert!(stderr(&second).contains("did not write"), "the failure must say the output is missing: {}", stderr(&second));
    assert!(!d.join("variants/evidence-v.json").exists(), "no evidence may be written for an image this run did not build");
    let _ = fs::remove_dir_all(d);
}

/// For a `custom` build, butterfly does not know which compiler the command
/// ran, so it must not record clang's version as if clang had run.
#[test]
fn a_custom_build_does_not_claim_a_compiler_it_did_not_run() {
    let Some(d) = project("customcc") else { return };
    let src = d.join("demo.c");
    // clang behind a shell: the command runs a compiler butterfly never names
    custom_config(&d, &format!("[\"sh\", \"-c\", \"exec clang \\\"$@\\\"\", \"sh\", \"-g\", \"-O0\", {src:?}, \"-o\", \"{{out}}\"]"));
    let o = build(&d);
    assert!(ok(&o), "build: {}", stderr(&o));
    let info = fs::read_to_string(d.join("variants/v/build-info.txt")).unwrap();
    let line = info.lines().find(|l| l.starts_with("compiler=")).expect("a compiler line");
    assert!(!line.to_lowercase().contains("clang version"), "build-info names a compiler butterfly never ran: {line}");
    assert!(line.contains("custom"), "build-info must say the compiler was not identified: {line}");
    let _ = fs::remove_dir_all(d);
}

/// The compiler is identified where it compiled. A toolchain chosen by the
/// directory (rustup's `rust-toolchain.toml`, a directory-scoped wrapper)
/// answers `--version` differently elsewhere, and the probe used to run in
/// butterfly's own working directory.
#[test]
fn the_compiler_is_identified_in_the_directory_it_compiled_in() {
    let Some(d) = project("ccdir") else { return };
    use std::os::unix::fs::PermissionsExt;
    let wrapper = d.join("cc-here");
    fs::write(&wrapper, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo \"cc-here 1.0 in $(basename \"$PWD\")\"; exit 0; fi\nexec clang \"$@\"\n").unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    clang_config(&d, wrapper.to_str().unwrap(), "[\"{exe}\"]", &[("O0", "[\"-O0\"]")]);
    let o = build(&d);
    assert!(ok(&o), "build: {}", stderr(&o));
    let info = fs::read_to_string(d.join("variants/O0/build-info.txt")).unwrap();
    assert!(info.contains("compiler=cc-here 1.0 in O0\n"), "the compiler was identified somewhere other than where it compiled:\n{info}");
    let _ = fs::remove_dir_all(d);
}

/// A variant that fails part-way must not leave the previous run's evidence
/// for the variants after it, beside fresh evidence for the ones before.
#[test]
fn a_failed_build_leaves_no_earlier_runs_evidence_behind() {
    let Some(d) = project("partial") else { return };
    let variants = [("O0", "[\"-O0\"]"), ("O1", "[\"-O1\"]"), ("O2", "[\"-O2\"]")];
    clang_config(&d, "clang", "[\"{exe}\"]", &variants);
    let first = build(&d);
    assert!(ok(&first), "first build: {}", stderr(&first));
    for (n, _) in &variants {
        assert!(d.join(format!("variants/evidence-{n}.json")).exists());
    }

    let broken = [("O0", "[\"-O0\"]"), ("O1", "[\"-O1\", \"-include\", \"no-such-header.h\"]"), ("O2", "[\"-O2\"]")];
    clang_config(&d, "clang", "[\"{exe}\"]", &broken);
    let second = build(&d);
    assert!(!ok(&second), "the broken variant must fail the build");
    for n in ["O1", "O2"] {
        assert!(!d.join(format!("variants/evidence-{n}.json")).exists(), "evidence-{n}.json from the earlier run survived a failed build");
    }
    let _ = fs::remove_dir_all(d);
}

/// Variant names become directories and file names under the output
/// directory, and that directory is cleaned before each build. A name that
/// leaves it, or two variants sharing one, is refused before anything runs.
#[test]
fn variant_names_are_single_distinct_directory_names() {
    let d = butterfly_testsupport::scratch_dir("prov-names");
    fs::write(d.join("demo.c"), "int main(void){return 0;}\n").unwrap();
    for bad in [&[("../escape", "[]")][..], &[("a/b", "[]")], &[("", "[]")], &[("coverage", "[]")], &[("O0", "[]"), ("O0", "[]")]] {
        clang_config(&d, "no-such-compiler", "[\"{exe}\"]", bad);
        let o = build(&d);
        assert!(!ok(&o), "{bad:?} was accepted");
        assert!(stderr(&o).contains("variant name"), "{bad:?}: the refusal must name the variant: {}", stderr(&o));
        assert!(!d.join("variants/coverage").exists(), "{bad:?}: something was built before the names were checked");
    }
    let _ = fs::remove_dir_all(d);
}

/// An executable name that would leave the output directory is refused
/// before anything is cleaned, like a bad variant name: a configuration
/// mistake must not cost the previous run's evidence (#296 review).
#[test]
fn a_bad_output_name_is_refused_before_anything_is_removed() {
    let d = butterfly_testsupport::scratch_dir("prov-outname");
    fs::create_dir_all(d.join("variants")).unwrap();
    fs::write(d.join("demo.c"), "int main(void){return 0;}\n").unwrap();
    for bad in ["../escape", "a/b", "coverage"] {
        fs::write(d.join("variants/coverage.json"), "{}").unwrap();
        fs::write(d.join("variants/evidence-v.json"), "{}").unwrap();
        let t = format!(
            "[project]\nname = \"demo\"\nout_dir = \"variants\"\n\n[compiler]\nkind = \"custom\"\nsources = [\"demo.c\"]\noutput = \"{bad}\"\n\n\
             [coverage]\ncommand = [\"true\"]\nrun = [\"true\"]\n\n[[variant]]\nname = \"v\"\ncommand = [\"true\"]\n"
        );
        fs::write(d.join("butterfly-build.toml"), t).unwrap();
        let o = build(&d);
        assert!(!ok(&o), "{bad:?} was accepted");
        assert!(d.join("variants/coverage.json").exists(), "{bad:?}: the previous coverage export was removed before the name was checked");
        assert!(d.join("variants/evidence-v.json").exists(), "{bad:?}: the previous evidence was removed before the name was checked");
    }
    let _ = fs::remove_dir_all(d);
}

/// `butterfly run` says a failed build makes everything downstream
/// untrustworthy; it must not then leave the previous run's evidence and
/// report in the build directory for `check` to accept.
#[test]
fn a_failed_run_leaves_no_earlier_evidence_in_the_build_directory() {
    let d = butterfly_testsupport::scratch_dir("prov-run");
    fs::create_dir_all(d.join("build")).unwrap();
    fs::write(d.join("build/evidence.json"), "{}").unwrap();
    fs::write(d.join("build/report.html"), "<p>earlier run</p>").unwrap();
    let s = d.join("build.sh");
    fs::write(&s, "#!/bin/sh\necho compiling >&2\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&s, fs::Permissions::from_mode(0o755)).unwrap();

    let o = Command::new(bin()).arg("run").arg(&d).arg("--no-serve").output().unwrap();
    assert!(!ok(&o));
    assert!(!d.join("build/evidence.json").exists(), "the previous run's evidence.json survived a failed run");
    assert!(!d.join("build/report.html").exists(), "the previous run's report.html survived a failed run");
    let _ = fs::remove_dir_all(d);
}

/// A build of the example with two variants, for the ingest-level cases.
fn two_variants(tag: &str) -> Option<PathBuf> {
    let d = project(tag)?;
    clang_config(&d, "clang", "[\"{exe}\"]", &[("O0", "[\"-O0\"]"), ("O2", "[\"-O2\"]")]);
    let o = build(&d);
    assert!(ok(&o), "build: {}", stderr(&o));
    Some(d)
}

/// The object is hashed from the bytes butterfly read; the disassembly must
/// come from those bytes too, not from whatever is at the path by the time
/// `llvm-objdump` opens it. The replacement is simulated by an objdump
/// wrapper that swaps the file before running the real tool.
#[test]
fn the_disassembly_is_of_the_bytes_that_were_hashed() {
    let Some(real) = butterfly_ingest::objectcode::find_llvm_tool("llvm-objdump", None).ok() else {
        butterfly_testsupport::fixture_missing("llvm-objdump is not installed");
        return;
    };
    let Some(d) = two_variants("swap") else { return };
    use std::os::unix::fs::PermissionsExt;
    let tools = d.join("tools");
    fs::create_dir_all(&tools).unwrap();
    let target = d.join("variants/O0/demo");
    let other = d.join("variants/O2/demo");
    let marker = d.join("swapped");
    fs::write(
        tools.join("llvm-objdump"),
        format!(
            "#!/bin/sh\nif [ \"$1\" != --version ] && [ ! -e {m:?} ]; then cp {o:?} {t:?}.new && mv {t:?}.new {t:?}; touch {m:?}; fi\nexec {r:?} \"$@\"\n",
            m = marker.display().to_string(),
            o = other.display().to_string(),
            t = target.display().to_string(),
            r = real.display().to_string()
        ),
    )
    .unwrap();
    fs::set_permissions(tools.join("llvm-objdump"), fs::Permissions::from_mode(0o755)).unwrap();
    let expected = evidence(&d.join("variants/evidence-O0.json"))["summary"]["instructionsTotal"].clone();

    let out = d.join("swapped.json");
    let o = Command::new(bin())
        .args(["ingest", "--coverage"])
        .arg(d.join("variants/coverage.json"))
        .arg("--object")
        .arg(&target)
        .arg("--source-root")
        .arg(&d)
        .arg("--llvm-bin")
        .arg(&tools)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(ok(&o), "ingest: {}", stderr(&o));
    assert!(marker.exists(), "the wrapper never ran, so nothing was tested");
    let got = evidence(&out)["summary"]["instructionsTotal"].clone();
    assert_eq!(got, expected, "the evidence hashes one image and disassembles another");
    let _ = fs::remove_dir_all(d);
}

/// A dSYM beside a Mach-O image is that image's debug information only if
/// their UUIDs agree. Another build's dSYM maps every address to the wrong
/// line and must be refused.
#[cfg(target_os = "macos")]
#[test]
fn a_dsym_from_another_build_is_refused() {
    let Some(d) = two_variants("dsym") else { return };
    let mix = d.join("mix");
    fs::create_dir_all(&mix).unwrap();
    fs::copy(d.join("variants/O2/demo"), mix.join("demo")).unwrap();
    let st = Command::new("cp").arg("-R").arg(d.join("variants/O0/demo.dSYM")).arg(mix.join("demo.dSYM")).status().unwrap();
    assert!(st.success());
    let o = Command::new(bin())
        .args(["ingest", "--coverage"])
        .arg(d.join("variants/coverage.json"))
        .arg("--object")
        .arg(mix.join("demo"))
        .arg("--source-root")
        .arg(&d)
        .arg("-o")
        .arg(mix.join("ev.json"))
        .output()
        .unwrap();
    assert!(!ok(&o), "another build's dSYM was accepted");
    assert!(stderr(&o).contains("UUID"), "the refusal must say why: {}", stderr(&o));
    let _ = fs::remove_dir_all(d);
}

/// The LLVM tool `clang` itself names, by absolute path. `None` (after saying
/// why) when it names none: then there is no profile reader to stand behind
/// a wrapper.
fn clang_tool(name: &str) -> Option<PathBuf> {
    let out = Command::new("clang").arg(format!("-print-prog-name={name}")).output().ok()?;
    let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    if p.is_absolute() && p.is_file() {
        Some(p)
    } else {
        butterfly_testsupport::not_applicable(&format!("clang names no {name} of its own"));
        None
    }
}

/// A stand-in for `real` that answers `--version` itself with `banner` (a
/// shell command, so it can say where it was asked), and runs `real` for
/// everything else.
fn wrapper(path: &Path, banner: &str, real: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then {banner}; exit 0; fi\nexec {:?} \"$@\"\n", real.display().to_string())).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn build_info(p: &Path) -> std::collections::BTreeMap<String, String> {
    fs::read_to_string(p).unwrap().lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The tools that produced `coverage.json` -- the compiler of the
/// instrumented build, the `llvm-profdata` that merged the profile and the
/// `llvm-cov` that exported it -- are recorded by the path that ran and by
/// the version each gives in the directory it ran in (#298). Profile and
/// coverage-mapping formats belong to one LLVM, so an auditor must be able
/// to check the pairing from the evidence, and a baseline must see it change.
#[test]
fn the_coverage_toolchain_is_recorded_where_it_ran() {
    let Some(d) = project("covtools") else { return };
    let (Some(profdata), Some(cov)) = (clang_tool("llvm-profdata"), clang_tool("llvm-cov")) else { return };
    let clang = butterfly_ingest::provenance::resolve_on_path("clang").expect("clang is on PATH");
    let cov_cc = d.join("cov-cc");
    wrapper(&cov_cc, "echo \"cov-cc 2.0 in $(basename \"$PWD\")\"", &clang);
    let tools = d.join("tools");
    for (name, real) in [("llvm-profdata", &profdata), ("llvm-cov", &cov)] {
        wrapper(&tools.join(name), &format!("echo 'LLVM (http://llvm.org/):'; echo \"  {name} LLVM version 99.1 in $(basename \"$PWD\")\""), real);
    }
    // ingest reads BUTTERFLY_LLVM_BIN too, for its disassembler
    let Ok(objdump) = butterfly_ingest::objectcode::find_llvm_tool("llvm-objdump", None) else {
        butterfly_testsupport::fixture_missing("llvm-objdump is not installed");
        return;
    };
    std::os::unix::fs::symlink(objdump, tools.join("llvm-objdump")).unwrap();
    clang_config(&d, "clang", "[\"{exe}\"]", &[("O0", "[\"-O0\"]")]);
    let cfg = d.join("butterfly-build.toml");
    let text = fs::read_to_string(&cfg).unwrap().replace("[coverage]\ncc = \"clang\"", &format!("[coverage]\ncc = {:?}", cov_cc.display().to_string()));
    fs::write(&cfg, text).unwrap();

    let o = Command::new(bin()).arg("build").arg(&cfg).env("BUTTERFLY_LLVM_BIN", &tools).output().unwrap();
    assert!(ok(&o), "build: {}", stderr(&o));
    let info = build_info(&d.join("variants/O0/build-info.txt"));
    let here = d.file_name().unwrap().to_string_lossy().into_owned();
    let expect = [
        // the variant compiler, by path as well, as the adapters record it
        ("compiler_path", clang.display().to_string()),
        // the instrumented build compiles in the source root
        ("compiler_instr", format!("cov-cc 2.0 in {here}")),
        ("compiler_instr_path", cov_cc.display().to_string()),
        // the profile is merged and exported in the coverage directory; the
        // version is the line that carries one, not the URL banner
        ("llvm_profdata", "llvm-profdata LLVM version 99.1 in coverage".to_string()),
        ("llvm_profdata_path", tools.join("llvm-profdata").display().to_string()),
        ("llvm_cov", "llvm-cov LLVM version 99.1 in coverage".to_string()),
        ("llvm_cov_path", tools.join("llvm-cov").display().to_string()),
    ];
    for (k, v) in &expect {
        assert_eq!(info.get(*k), Some(v), "build-info.txt `{k}`: {info:?}");
    }
    // ... and ingest carries them into the evidence's provenance
    let ev = evidence(&d.join("variants/evidence-O0.json"));
    for (k, v) in &expect {
        assert_eq!(ev["provenance"]["buildInfo"][*k].as_str(), Some(v.as_str()), "provenance `{k}`");
    }
    let _ = fs::remove_dir_all(d);
}

/// A `custom` coverage command runs whatever it names, so butterfly cannot
/// say which compiler built the instrumented binary, and must not guess. It
/// ran `llvm-profdata` and `llvm-cov` itself, so those it does name (#298).
#[test]
fn a_custom_build_names_the_profile_tools_but_not_a_coverage_compiler() {
    let Some(d) = project("customcov") else { return };
    let src = d.join("demo.c");
    custom_config(&d, &format!("[\"clang\", \"-g\", \"-O0\", {src:?}, \"-o\", \"{{out}}\"]"));
    let o = build(&d);
    assert!(ok(&o), "build: {}", stderr(&o));
    let info = build_info(&d.join("variants/v/build-info.txt"));
    let cc = info.get("compiler_instr").unwrap_or_else(|| panic!("a compiler_instr line: {info:?}"));
    assert!(cc.contains("not identified") && !cc.to_lowercase().contains("clang version"), "build-info names a coverage compiler butterfly never ran: {cc}");
    for k in ["compiler_path", "compiler_instr_path"] {
        assert!(!info.contains_key(k), "no `{k}` for a compiler that was not identified: {info:?}");
    }
    for k in ["llvm_profdata_path", "llvm_cov_path"] {
        let p = PathBuf::from(info.get(k).unwrap_or_else(|| panic!("build-info records `{k}`: {info:?}")));
        assert!(p.is_absolute() && p.is_file(), "`{k}` names the tool that ran: {}", p.display());
    }
    for k in ["llvm_profdata", "llvm_cov"] {
        assert!(info.get(k).is_some_and(|v| v.chars().any(|c| c.is_ascii_digit())), "`{k}` records the version: {info:?}");
    }
    let _ = fs::remove_dir_all(d);
}

/// A rustup `rustc` on PATH is a proxy: one file for every toolchain, which
/// picks the real compiler from `+toolchain` or the directory. Recording its
/// path names no toolchain. What is recorded is the rustc that ran -- the
/// one in the sysroot the invocation selects -- for the coverage build and
/// for each variant (#305 review).
#[test]
fn a_rustup_rustc_is_recorded_as_the_toolchains_own_binary_not_the_proxy() {
    let nightly = std::env::var("BUTTERFLY_NIGHTLY").unwrap_or_else(|_| "nightly".into());
    let sysroot = |sel: Option<&str>| -> Option<PathBuf> {
        let mut c = Command::new("rustc");
        if let Some(s) = sel {
            c.arg(format!("+{s}"));
        }
        let o = c.arg("--print=sysroot").output().ok().filter(|o| o.status.success())?;
        Some(PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
    };
    let (Some(stable), Some(night)) = (sysroot(None), sysroot(Some(&nightly))) else {
        butterfly_testsupport::fixture_missing(&format!("rustc and the {nightly} toolchain are needed"));
        return;
    };
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/rust/demo.rs");
    let d = butterfly_testsupport::scratch_dir("prov-rustc");
    fs::copy(&src, d.join("demo.rs")).unwrap();
    fs::write(
        d.join("butterfly-build.toml"),
        format!(
            "[project]\nname = \"demo-rust\"\nsource_root = \".\"\nout_dir = \"variants\"\n\n[compiler]\nkind = \"rustc\"\ncc = \"rustc\"\nsources = [\"demo.rs\"]\ncommon_flags = [\"--edition\", \"2021\", \"-g\"]\noutput = \"demo\"\n\n\
             [coverage]\ncc = \"rustc +{nightly}\"\nflags = [\"-Copt-level=0\", \"-Cinstrument-coverage\"]\nrun = [\"{{exe}}\"]\n\n[[variant]]\nname = \"O0\"\nflags = [\"-Copt-level=0\"]\n"
        ),
    )
    .unwrap();
    let o = build(&d);
    assert!(ok(&o), "build: {}", stderr(&o));
    let info = build_info(&d.join("variants/O0/build-info.txt"));
    let proxy = butterfly_ingest::provenance::resolve_on_path("rustc").unwrap();
    for (key, root) in [("compiler_instr_path", &night), ("compiler_path", &stable)] {
        let got = info.get(key).map(PathBuf::from);
        assert_eq!(got.as_deref(), Some(root.join("bin/rustc").as_path()), "`{key}` must be the toolchain's own rustc, not {}: {info:?}", proxy.display());
    }
    let _ = fs::remove_dir_all(d);
}

/// The coverage and variant directories are the build's to clean only when
/// they are its own. As symlinks to another directory, the cleanup deleted
/// the link targets' `.su` files, dSYM bundles, executables, profiles and
/// build-info, and the build then wrote into them (#313).
#[test]
fn cleanup_does_not_follow_a_symlinked_output_directory() {
    let Some(d) = project("subdir-link") else { return };
    clang_config(&d, "clang", "[\"{exe}\"]", &[("O0", "[\"-O0\", \"-fstack-usage\"]")]);
    let first = build(&d);
    assert!(ok(&first), "first build: {}", stderr(&first));

    // two directories that are not the build's, each holding files named as
    // the cleanup's targets are
    let keep = |dir: &Path, rels: &[&str]| {
        for r in rels {
            let p = dir.join(r);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, "not butterfly's").unwrap();
        }
    };
    let victim = d.join("victim");
    let victim_files = ["keep.su", "demo", "build-info.txt", "demo.dSYM/Contents/keep"];
    keep(&victim, &victim_files);
    let cvictim = d.join("cvictim");
    let cvictim_files = ["default.profdata", "default.profraw", "demo-instr", "demo-instr.dSYM/Contents/keep"];
    keep(&cvictim, &cvictim_files);
    for (sub, target) in [("O0", &victim), ("coverage", &cvictim)] {
        let at = d.join("variants").join(sub);
        fs::remove_dir_all(&at).unwrap();
        std::os::unix::fs::symlink(target, &at).unwrap();
    }

    let second = build(&d);
    for (dir, rels) in [(&victim, &victim_files[..]), (&cvictim, &cvictim_files[..])] {
        for r in rels {
            assert_eq!(
                fs::read_to_string(dir.join(r)).ok().as_deref(),
                Some("not butterfly's"),
                "{} was deleted or overwritten through a symlinked output directory; build: {}",
                dir.join(r).display(),
                stderr(&second)
            );
        }
    }
    assert!(ok(&second), "a symlinked output directory is replaced, not a failure: {}", stderr(&second));
    for sub in ["O0", "coverage"] {
        let m = fs::symlink_metadata(d.join("variants").join(sub)).unwrap();
        assert!(m.is_dir() && !m.file_type().is_symlink(), "variants/{sub} is now a directory of the build's own");
    }
    let _ = fs::remove_dir_all(d);
}
