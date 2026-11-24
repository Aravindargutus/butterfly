// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for `butterfly build` (area F): the
//! butterfly-build.toml driver, its variants, where it writes, and how it
//! fails.
//!
//! Expected values come from README.md ("Commands", "Exploratory builds and
//! variant comparison", "Security posture"), docs/butterfly.html, the
//! example configs under examples/*/butterfly-build.toml and
//! support/features.toml -- each test names its source.
//!
//! The failure paths use `kind = "custom"` and need no compiler. The
//! end-to-end ones need clang and its LLVM tools, like
//! build_stack_usage.rs, and skip (or, where fixtures are required, fail)
//! without them.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn scratch(tag: &str) -> PathBuf {
    let d = butterfly_testsupport::scratch_dir(&format!("req-f-build-{tag}"));
    d.canonicalize().unwrap()
}

fn butterfly() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.env_remove("BUTTERFLY_TOKEN").stdin(Stdio::null());
    c
}

fn build(cfg: &Path, extra: &[&str], env: &[(&str, &str)]) -> Output {
    let mut c = butterfly();
    c.arg("build").arg(cfg).args(extra);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().expect("spawn butterfly build")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A `custom` config whose coverage step is `coverage` (a TOML array).
fn custom_cfg(dir: &Path, coverage: &str) -> PathBuf {
    let p = dir.join("butterfly-build.toml");
    std::fs::write(
        &p,
        format!(
            "[project]\nname = \"t\"\nsource_root = \".\"\nout_dir = \"out\"\n\n[compiler]\nkind = \"custom\"\n\n[coverage]\nrun = [\"true\"]\ncommand = {coverage}\n\n[[variant]]\nname = \"V\"\ncommand = [\"true\"]\n"
        ),
    )
    .unwrap();
    p
}

fn evidence_files(dir: &Path) -> Vec<String> {
    let mut v = Vec::new();
    let mut walk = vec![dir.to_path_buf()];
    while let Some(d) = walk.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk.push(p);
            } else if p.file_name().unwrap().to_string_lossy().starts_with("evidence") {
                v.push(p.display().to_string());
            }
        }
    }
    v
}

// ============================================================ failures

/// REQ-F-BUILD-CONFIG-READ: the config `butterfly build` reads is sized and
/// read like every other input, and "fails with a sentence naming the file"
/// (README.md:1582-1583, 1592). A config that is not there is such a case.
#[test]
fn req_f_a_missing_config_is_an_error_naming_it() {
    let d = scratch("missing");
    let o = build(&d.join("no-such-build.toml"), &[], &[]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("no-such-build.toml"), "{}", stderr(&o));
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-BUILD-CONFIG-SHAPE: `build <config.toml>` (README.md:125) reads the
/// shape the examples write (examples/c/butterfly-build.toml). Something
/// that is not that shape is refused before anything is built or written.
#[test]
fn req_f_a_malformed_or_incomplete_config_is_refused_before_anything_runs() {
    let d = scratch("malformed");
    let marker = d.join("ran");
    for (name, text) in [
        ("syntax.toml", "[project\nname = 1".to_string()),
        // no [coverage] table
        ("incomplete.toml", "[project]\nname = \"t\"\nout_dir = \"out\"\n[compiler]\nkind = \"custom\"\n[[variant]]\nname = \"V\"\n".to_string()),
        // a [[variant]] without a name
        (
            "nameless.toml",
            format!(
                "[project]\nname = \"t\"\nout_dir = \"out\"\n[compiler]\nkind = \"custom\"\n[coverage]\nrun = [\"true\"]\ncommand = [\"touch\", \"{}\"]\n[[variant]]\nflags = []\n",
                marker.display()
            ),
        ),
    ] {
        let p = d.join(name);
        std::fs::write(&p, text).unwrap();
        let o = build(&p, &[], &[]);
        assert!(!o.status.success(), "{name}: accepted:\n{}", stderr(&o));
        assert!(!marker.exists(), "{name}: a command ran from a config that does not parse");
        assert!(!d.join("out").exists(), "{name}: an output directory was created from a config that does not parse");
    }
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-BUILD-VARIANT-UNIQUE: README "Exploratory builds and variant
/// comparison": each `[[variant]]` gets a directory and an
/// `evidence-<name>.json` of its own, so two with one name would write the
/// second over the first. Decided in #325 (ambiguity 8): such a config is
/// refused before anything is built, by an error naming the duplicate and
/// the config file.
#[test]
fn req_f_duplicate_variant_names_are_refused_before_anything_runs() {
    let d = scratch("dup-variant");
    let marker = d.join("ran");
    let p = d.join("dup-build.toml");
    std::fs::write(
        &p,
        format!(
            "[project]\nname = \"t\"\nsource_root = \".\"\nout_dir = \"out\"\n[compiler]\nkind = \"custom\"\n[coverage]\nrun = [\"true\"]\ncommand = [\"touch\", \"{}\"]\n[[variant]]\nname = \"O1\"\ncommand = [\"true\"]\n[[variant]]\nname = \"O2\"\ncommand = [\"true\"]\n[[variant]]\nname = \"O1\"\ncommand = [\"true\"]\n",
            marker.display()
        ),
    )
    .unwrap();
    let o = build(&p, &[], &[]);
    let err = stderr(&o);
    assert!(!o.status.success(), "accepted:\n{err}");
    assert!(err.contains("\"O1\""), "the duplicate is named: {err}");
    assert!(err.contains("dup-build.toml"), "the config file is named: {err}");
    assert!(!marker.exists(), "a command ran from a config with a duplicate variant");
    assert!(!d.join("out").exists(), "an output directory was created from a config with a duplicate variant");
    assert!(evidence_files(&d).is_empty());
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-BUILD-KIND: the compiler is one the driver knows how to drive --
/// clang, gcc, rustc (examples/*/butterfly-build.toml), or a `custom`
/// command. An unknown kind is refused, naming it, and builds nothing.
#[test]
fn req_f_an_unknown_compiler_kind_is_refused_by_name() {
    let d = scratch("kind");
    let p = d.join("b.toml");
    std::fs::write(&p, "[project]\nname = \"t\"\nout_dir = \"out\"\n[compiler]\nkind = \"fortran77\"\n[coverage]\nrun = [\"true\"]\n[[variant]]\nname = \"V\"\n").unwrap();
    let o = build(&p, &[], &[]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("fortran77"), "{}", stderr(&o));
    assert!(evidence_files(&d).is_empty());
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-BUILD-STEP-FAILS: `butterfly build` "runs the commands in
/// `butterfly-build.toml`" (README.md:1624-1626). A command that fails --
/// or is empty -- fails the build, and no evidence is written for it.
#[test]
fn req_f_a_failing_or_empty_build_command_fails_the_build_and_writes_no_evidence() {
    for (tag, cmd, says) in
        [("false", r#"["false"]"#, "coverage build"), ("empty", "[]", "empty"), ("absent", r#"["/nonexistent/req-f-no-such-compiler"]"#, "req-f-no-such-compiler")]
    {
        let d = scratch(&format!("step-{tag}"));
        let cfg = custom_cfg(&d, cmd);
        let o = build(&cfg, &[], &[]);
        let e = stderr(&o);
        assert!(!o.status.success(), "{tag}: a failed step reported success:\n{e}");
        assert!(e.contains(says), "{tag}: the error does not say which step failed:\n{e}");
        assert!(evidence_files(&d).is_empty(), "{tag}: evidence written by a failed build: {:?}", evidence_files(&d));
        let _ = std::fs::remove_dir_all(d);
    }
}

/// REQ-F-BUILD-OWN-ENV: "your own build (`build.sh` under `run`, the compile
/// and test commands of `build`) ... runs with your environment"
/// (README.md:1612-1615), unlike butterfly's helper programs.
#[test]
fn req_f_the_configured_commands_run_with_the_callers_environment() {
    let d = scratch("env");
    let seen = d.join("seen");
    let cfg = custom_cfg(&d, &format!(r#"["sh", "-c", "printf %s \"$REQ_F_PROJECT_VAR\" > '{}'"]"#, seen.display()));
    let _ = build(&cfg, &[], &[("REQ_F_PROJECT_VAR", "kept-for-the-build")]);
    assert_eq!(std::fs::read_to_string(&seen).expect("the coverage command did not run"), "kept-for-the-build");
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-BUILD-SERVE-REFUSES-FIRST: "`--allow-remote` on its own refuses to
/// start and says so"; "Off loopback butterfly refuses to start without a
/// token" (README.md:1500, 1506-1507). For `build --serve` that refusal has
/// to come before the build runs anything, not minutes after it
/// (`ServeFlags`: "One definition shared by `serve`, `build --serve` and
/// `run`").
#[test]
fn req_f_build_serve_refuses_a_listener_it_would_not_start_before_building() {
    let d = scratch("serve-first");
    let marker = d.join("built");
    let cfg = custom_cfg(&d, &format!(r#"["touch", "{}"]"#, marker.display()));
    for extra in [vec!["--allow-remote"], vec!["--allow-remote", "--insecure-http"], vec!["--insecure-http"]] {
        let mut args = vec!["--serve", "--listen", "0.0.0.0:0"];
        args.extend(&extra);
        let o = build(&cfg, &args, &[]);
        assert!(!o.status.success(), "{extra:?}: {}", stderr(&o));
        assert!(!marker.exists(), "{extra:?}: the build ran before the listener was refused");
    }
    let _ = std::fs::remove_dir_all(d);
}

// ======================================================== end to end (clang)

fn clang_ready() -> bool {
    let ok = Command::new("clang").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        butterfly_testsupport::fixture_missing("clang is not installed");
    }
    ok
}

/// A C project with a header in `include/` and another in `extra/`, built at
/// two optimisation levels into `out_dir = "variants"`: two translation
/// units, or (`single`) one that holds both functions.
fn c_project(dir: &Path, single: bool, fast_flags: &str, small_flags: &str) -> PathBuf {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("include")).unwrap();
    std::fs::create_dir_all(dir.join("extra")).unwrap();
    std::fs::write(dir.join("include/util.h"), "int twice(int x);\n").unwrap();
    std::fs::write(dir.join("extra/limits_extra.h"), "#define REQ_F_LIMIT 3\n").unwrap();
    std::fs::write(dir.join("src/util.c"), "#include \"util.h\"\nint twice(int x) {\n  if (x > 100) {\n    return 200;\n  }\n  return x * 2;\n}\n").unwrap();
    std::fs::write(
        dir.join("src/main.c"),
        "#include \"util.h\"\n#include \"limits_extra.h\"\nint main(int argc, char **argv) {\n  (void)argv;\n  int s = 0;\n  for (int i = 0; i < REQ_F_LIMIT; i++) {\n    s += twice(i + argc);\n  }\n  return s > 1000;\n}\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/single.c"), "#include \"limits_extra.h\"\n#include \"../src/util.c\"\n#include \"../src/main.c\"\n").unwrap();
    let sources = if single { r#"["src/single.c"]"# } else { r#"["src/util.c", "src/main.c"]"# };
    let p = dir.join("butterfly-build.toml");
    std::fs::write(
        &p,
        format!(
            "[project]\nname = \"reqf\"\nsource_root = \".\"\nout_dir = \"variants\"\n\n[compiler]\nkind = \"clang\"\ncc = \"clang\"\nsources = {sources}\ncommon_flags = [\"-std=c11\", \"-g\", \"-Iinclude\", \"-I\", \"extra\"]\noutput = \"reqf\"\n\n[coverage]\nflags = [\"-O0\", \"-fprofile-instr-generate\", \"-fcoverage-mapping\"]\nrun = [\"{{exe}}\"]\n\n[[variant]]\nname = \"fast\"\nflags = [{fast_flags}]\n\n[[variant]]\nname = \"small\"\nflags = [{small_flags}]\n"
        ),
    )
    .unwrap();
    p
}

fn load(p: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

/// REQ-F-BUILD-VARIANTS: "one instrumented build + run for coverage, then
/// every `[[variant]]` in the TOML ... compiled, ingested" (README.md:1431-
/// 1433), each to `<out_dir>/evidence-<name>.json` beside the config
/// (support/features.toml:148-153: `butterfly build examples/c/
/// butterfly-build.toml` from the repository root gives
/// `examples/c/variants/evidence-O0.json`), with include directories written
/// relative to the config as in examples/c-lib/butterfly-build.toml:17.
/// REQ-F-EXPLORATORY: "Everything produced this way is stamped **EXPLORATORY
/// BUILD** in provenance and in the caveat bar." (README.md:1440-1441;
/// docs/butterfly.html:450; examples/README.md:212-213)
#[test]
fn req_f_every_variant_is_built_ingested_and_stamped_exploratory() {
    if !clang_ready() {
        return;
    }
    let d = scratch("e2e");
    let proj = d.join("proj");
    // No -fstack-usage in the flags: the driver adds it (F2), and every
    // unit keeps its own .su (F3).
    let cfg = c_project(&proj, false, r#""-O1""#, r#""-Os""#);
    // run from somewhere else: out_dir is the config's, not the cwd's
    let elsewhere = d.join("cwd");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let relative = Path::new("..").join(cfg.strip_prefix(&d).unwrap());
    let o = butterfly().current_dir(&elsewhere).arg("build").arg(&relative).output().unwrap();
    assert!(o.status.success(), "build failed:\n{}", stderr(&o));
    assert!(evidence_files(&elsewhere).is_empty(), "evidence written relative to the working directory");

    for name in ["fast", "small"] {
        let p = proj.join("variants").join(format!("evidence-{name}.json"));
        let ev = load(&p);
        assert_eq!(ev["provenance"]["buildInfo"]["variant"], name, "{name}: the evidence names its variant");
        let caveats = ev["provenance"]["caveats"].as_array().unwrap();
        assert!(caveats.iter().any(|c| c.as_str().unwrap_or_default().contains("EXPLORATORY BUILD")), "{name}: not stamped EXPLORATORY BUILD: {caveats:?}");
        let fns: Vec<&str> = ev["functions"].as_array().unwrap().iter().filter_map(|f| f["name"].as_str()).collect();
        assert!(fns.iter().any(|f| f.contains("twice")) && fns.iter().any(|f| f.contains("main")), "{name}: both units were compiled and ingested: {fns:?}");
        // the coverage of the instrumented run was projected onto it
        assert!(ev["summary"]["linesCovered"].as_u64().unwrap_or(0) > 0, "{name}: no coverage reached the variant");
    }
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-BUILD-STACK: "Each variant is built with `-fstack-usage` where the
/// toolchain supports it, and that data is ingested: the compiler's own
/// figures are authoritative" (README.md:1438). clang supports it, so a
/// variant whose flags do not mention it still gets the compiler's figures.
#[test]
fn req_f_every_clang_variant_gets_the_compilers_stack_figures() {
    if !clang_ready() {
        return;
    }
    let d = scratch("su");
    let cfg = c_project(&d, true, r#""-O1""#, r#""-Os""#);
    let o = build(&cfg, &[], &[]);
    assert!(o.status.success(), "build failed:\n{}", stderr(&o));
    for name in ["fast", "small"] {
        let ev = load(&d.join("variants").join(format!("evidence-{name}.json")));
        let from_su = ev["symbols"].as_array().unwrap().iter().filter(|s| s["frame"]["source"] == "stackUsageFile").count();
        assert!(from_su > 0, "{name}: no symbol carries the compiler's stack figure");
    }
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-BUILD-SERVE: `butterfly build … --serve` serves the variants
/// "together. The UI gains a variant switcher" (README.md:1428-1434), "one
/// resolutions sidecar per variant" (README.md:1448), and its freshness
/// check reads sources under "the config's source root for `build`"
/// (README.md:1559-1560).
#[test]
fn req_f_build_serve_serves_every_variant_with_its_own_sidecar() {
    if !clang_ready() {
        return;
    }
    let d = scratch("serve");
    let cfg = c_project(&d, false, r#""-O1""#, r#""-Os""#);
    let mut srv = common::serve(butterfly().arg("build").arg(&cfg).arg("--serve"));
    let (code, body) = common::get(&mut srv, "/api/variants");
    assert_eq!(code, 200, "{body}");
    let boot: serde_json::Value = serde_json::from_str(&body).unwrap();
    let names: Vec<&str> = boot["variants"].as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["fast", "small"], "{body}");

    let err = srv.stderr();
    let line = err.lines().find(|l| l.contains("freshness reads sources only under")).unwrap_or_else(|| panic!("no freshness authority line:\n{err}"));
    assert!(line.contains(&*d.display().to_string()), "freshness is not bounded by the config's source root {}: {line}", d.display());

    // a review for one variant lands in a sidecar of its own, among the outputs
    let ev = load(&d.join("variants/evidence-small.json"));
    let key = ev["functions"].as_array().unwrap().iter().find_map(|f| f["object"]["gaps"].as_array().and_then(|g| g.first()).and_then(|g| g["key"].as_str()).map(str::to_string));
    let key = key.unwrap_or_else(|| "fn:0".into());
    let review = format!(r#"{{"key":"{key}","category":"analysis","justification":"req-f small only","author":"a","date":"2026-01-01"}}"#);
    let (code, body) = common::request(&mut srv, "POST", "/api/resolutions?v=small", &[("Content-Type", "application/json")], &review);
    assert_eq!(code, 200, "{body}");
    let (_, fast) = common::get(&mut srv, "/api/resolutions?v=fast");
    assert!(!fast.contains("req-f small only"), "{fast}");
    drop(srv);
    let sidecars: Vec<String> =
        std::fs::read_dir(d.join("variants")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains("resolutions")).collect();
    assert_eq!(sidecars.len(), 1, "one sidecar, for the one variant reviewed: {sidecars:?}");
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-BUILD-MULTI-SOURCE-STACK: examples/c-lib/butterfly-build.toml is
/// documented to be run as `butterfly build examples/c-lib/butterfly-build.toml
/// --serve` (its line 4) and asks for `-fstack-usage` on six sources that "the
/// driver compiles and links ... in one step per variant" (its lines 6-7);
/// "that data is ingested" (README.md:1438). Every unit's `.su` has to
/// survive for that: one compile+link step wrote one, named for the output,
/// so every unit but the last lost its figures and the ingest failed closed
/// on the variant (#325 F3).
#[test]
fn req_f_the_c_lib_example_config_builds() {
    if !clang_ready() {
        return;
    }
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-lib");
    let d = scratch("c-lib");
    let st = Command::new("cp").arg("-R").arg(&src).arg(&d).status().unwrap();
    assert!(st.success());
    let cfg = d.join("c-lib/butterfly-build.toml");
    let _ = std::fs::remove_dir_all(d.join("c-lib/variants"));
    let o = build(&cfg, &[], &[]);
    assert!(o.status.success(), "the documented example does not build:\n{}", stderr(&o));
    for name in ["O0", "O1", "O2", "Os"] {
        let ev = load(&d.join(format!("c-lib/variants/evidence-{name}.json")));
        let from_su = ev["symbols"].as_array().unwrap().iter().filter(|s| s["frame"]["source"] == "stackUsageFile").count();
        assert!(from_su > 0, "{name}: no compiler stack figures");
    }
    let _ = std::fs::remove_dir_all(d);
}

/// A two-variant clang project over the given sources (relative path, text),
/// with `common` as its `common_flags` (TOML array items).
fn sources_project(dir: &Path, files: &[(&str, &str)], common: &str) -> PathBuf {
    for (rel, text) in files {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let sources: Vec<String> = files.iter().map(|(rel, _)| format!("\"{rel}\"")).collect();
    let p = dir.join("butterfly-build.toml");
    std::fs::write(
        &p,
        format!(
            "[project]\nname = \"reqf\"\nsource_root = \".\"\nout_dir = \"variants\"\n\n[compiler]\nkind = \"clang\"\ncc = \"clang\"\nsources = [{}]\ncommon_flags = [{common}]\noutput = \"reqf\"\n\n[coverage]\nflags = [\"-O0\", \"-fprofile-instr-generate\", \"-fcoverage-mapping\"]\nrun = [\"{{exe}}\"]\n\n[[variant]]\nname = \"O0\"\nflags = [\"-O0\"]\n\n[[variant]]\nname = \"O1\"\nflags = [\"-O1\"]\n",
            sources.join(", ")
        ),
    )
    .unwrap();
    p
}

/// The names of the analysed functions, and of those whose frame came from a
/// `.su` file.
fn functions_and_su(ev: &serde_json::Value) -> (Vec<String>, Vec<String>) {
    let names = ev["functions"].as_array().unwrap().iter().filter_map(|f| f["name"].as_str().map(str::to_string)).collect();
    let su = ev["symbols"].as_array().unwrap().iter().filter(|s| s["frame"]["source"] == "stackUsageFile").filter_map(|s| s["name"].as_str().map(str::to_string)).collect();
    (names, su)
}

/// REQ-F-BUILD-MULTI-SOURCE-STACK, object names: every source is compiled
/// "to an object of its own, so each keeps its own `.su`" (README.md:1438).
/// Two sources sharing a stem, and a third whose stem is what a suffix for
/// the repeat would be (`a/foo.c`, `b/foo.c`, `foo-0.c`), once wrote two
/// compiles to one object: one unit's code and frames were lost and the link
/// failed (#328 review).
#[test]
fn req_f_sources_whose_stems_collide_each_get_an_object_and_su_of_their_own() {
    if !clang_ready() {
        return;
    }
    let d = scratch("stems");
    let cfg = sources_project(
        &d,
        &[
            ("a/foo.c", "int from_a(int x) { return x + 1; }\n"),
            ("b/foo.c", "int from_b(int x) { return x * 3; }\n"),
            ("foo-0.c", "int from_a(int);\nint from_b(int);\nint main(int argc, char **argv) {\n  (void)argv;\n  return from_a(argc) + from_b(argc) > 1000;\n}\n"),
        ],
        r#""-std=c11", "-g""#,
    );
    let o = build(&cfg, &[], &[]);
    assert!(o.status.success(), "build failed:\n{}", stderr(&o));
    for name in ["O0", "O1"] {
        let ev = load(&d.join(format!("variants/evidence-{name}.json")));
        let (fns, su) = functions_and_su(&ev);
        for f in ["from_a", "from_b", "main"] {
            assert!(fns.iter().any(|n| n.contains(f)), "{name}: {f} was not compiled and analysed: {fns:?}");
            assert!(su.iter().any(|n| n.contains(f)), "{name}: {f} has no compiler stack figure: {su:?}");
        }
        let objs: Vec<String> = std::fs::read_dir(d.join(format!("variants/{name}/obj"))).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(objs.iter().filter(|n| n.ends_with(".o")).count(), 3, "{name}: one object per source: {objs:?}");
        assert_eq!(objs.iter().filter(|n| n.ends_with(".su")).count(), 3, "{name}: one .su per source: {objs:?}");
    }
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-BUILD-FLAGS: `common_flags` and a variant's `flags` are the
/// options of the build (examples/*/butterfly-build.toml), and a linker
/// option among them is the link's. Compiling each source with `-c` must not
/// hand it to the compiles too, where it is an unused argument that
/// `-Werror` turns into a failed build (#328 review). A control build of the
/// same sources without the linker options shows it is the linker options
/// that are under test.
#[test]
fn req_f_linker_options_reach_only_the_link_even_under_werror() {
    if !clang_ready() {
        return;
    }
    let linker = if cfg!(target_os = "macos") { "-Wl,-dead_strip" } else { "-Wl,--as-needed" };
    let files = [
        ("src/util.c", "int twice(int x) { return x * 2; }\n"),
        ("src/main.c", "int twice(int);\nint main(int argc, char **argv) {\n  (void)argv;\n  return twice(argc) > 1000;\n}\n"),
    ];
    for (tag, common) in [("control", r#""-std=c11", "-g", "-Werror""#.to_string()), ("linker", format!(r#""-std=c11", "-g", "-Werror", "{linker}", "-lm""#))] {
        let d = scratch(&format!("werror-{tag}"));
        let cfg = sources_project(&d, &files, &common);
        let o = build(&cfg, &[], &[]);
        assert!(o.status.success(), "{tag}: build failed:\n{}", stderr(&o));
        let err = stderr(&o);
        let compiles: Vec<&str> = err.lines().filter(|l| l.contains(" -c ")).collect();
        assert!(!compiles.is_empty(), "{tag}: no separate compile seen:\n{err}");
        assert!(compiles.iter().all(|l| !l.contains(linker) && !l.contains(" -lm")), "{tag}: a linker option reached a compile:\n{err}");
        if tag == "linker" {
            assert!(err.lines().any(|l| !l.contains(" -c ") && l.contains(linker) && l.contains(" -lm")), "the linker options never reached the link:\n{err}");
        }
        for name in ["O0", "O1"] {
            let (fns, su) = functions_and_su(&load(&d.join(format!("variants/evidence-{name}.json"))));
            assert!(fns.iter().any(|n| n.contains("twice")) && su.iter().any(|n| n.contains("twice")), "{tag} {name}: {fns:?} {su:?}");
        }
        let _ = std::fs::remove_dir_all(d);
    }
}

/// REQ-F-BUILD-STACK, provenance: a variant's stack figures are "the
/// compiler's own" for that build (README.md:1438). Rebuilding the same
/// out_dir after switching `compiler.kind` from clang to `custom` left the
/// clang run's `<variant>/obj/*.su` in place, and they were ingested into the
/// custom build's evidence, frames for an executable they never described
/// (#328 review). Only the `.su` files this run's own compiles wrote count.
#[test]
fn req_f_a_rebuild_of_another_kind_takes_no_stack_files_from_the_last() {
    if !clang_ready() {
        return;
    }
    let d = scratch("rekind");
    let files = [
        ("src/util.c", "int twice(int x) { return x * 2; }\n"),
        ("src/main.c", "int twice(int);\nint main(int argc, char **argv) {\n  (void)argv;\n  return twice(argc) > 1000;\n}\n"),
    ];
    let cfg = sources_project(&d, &files, r#""-std=c11", "-g""#);
    let o = build(&cfg, &[], &[]);
    assert!(o.status.success(), "clang build failed:\n{}", stderr(&o));
    let stale =
        |v: &str| std::fs::read_dir(d.join(format!("variants/{v}/obj"))).map(|r| r.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "su")).count()).unwrap_or(0);
    assert_eq!(stale("O0"), 2, "the clang run left one .su per source");

    // the same project and out_dir, now built by commands with no -fstack-usage
    let srcs: Vec<String> = files.iter().map(|(rel, _)| format!("\"{}\"", d.join(rel).display())).collect();
    let srcs = srcs.join(", ");
    std::fs::write(
        &cfg,
        format!(
            "[project]\nname = \"reqf\"\nsource_root = \".\"\nout_dir = \"variants\"\n\n[compiler]\nkind = \"custom\"\noutput = \"reqf\"\n\n[coverage]\ncommand = [\"clang\", \"-std=c11\", \"-g\", \"-O0\", \"-fprofile-instr-generate\", \"-fcoverage-mapping\", {srcs}, \"-o\", \"{{out}}\"]\nrun = [\"{{exe}}\"]\n\n[[variant]]\nname = \"O0\"\ncommand = [\"clang\", \"-std=c11\", \"-g\", \"-O0\", {srcs}, \"-o\", \"{{out}}\"]\n"
        ),
    )
    .unwrap();
    let o = build(&cfg, &[], &[]);
    assert!(o.status.success(), "custom rebuild failed:\n{}", stderr(&o));
    assert_eq!(stale("O0"), 2, "the earlier run's .su files are still there to be (wrongly) read");
    let (fns, su) = functions_and_su(&load(&d.join("variants/evidence-O0.json")));
    assert!(fns.iter().any(|n| n.contains("twice")), "the custom build was analysed: {fns:?}");
    assert!(su.is_empty(), "the custom build took stack figures from the clang run's .su files: {su:?}");
    let _ = std::fs::remove_dir_all(d);
}
