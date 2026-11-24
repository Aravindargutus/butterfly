// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Exploratory build driver: compile the same sources several ways (e.g.
//! -O0 / -O1 / -O2), run the instrumented variant once for coverage, and
//! ingest every variant so they can be compared function by function.
//!
//! Everything produced here is stamped "exploratory" in provenance. It is a
//! way to *understand* what the optimiser does, never a substitute for the
//! configuration-controlled build whose binary is the certification evidence.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Deserialize)]
pub struct BuildConfig {
    pub project: Project,
    pub compiler: Compiler,
    pub coverage: Coverage,
    #[serde(rename = "variant")]
    pub variants: Vec<Variant>,
}

#[derive(Debug, Deserialize)]
pub struct Project {
    pub name: String,
    #[serde(default = "dot")]
    pub source_root: String,
    #[serde(default = "default_out")]
    pub out_dir: String,
    /// Read source files that resolve outside `source_root`. Off by default;
    /// set it only when the build legitimately compiles generated or vendored
    /// sources kept elsewhere.
    #[serde(default)]
    pub allow_external_source: bool,
}
fn dot() -> String {
    ".".into()
}
fn default_out() -> String {
    "butterfly-out".into()
}

#[derive(Debug, Deserialize)]
pub struct Compiler {
    /// `clang` | `gcc` | `rustc` | `custom`
    pub kind: String,
    /// Compiler command, may include a toolchain selector (`rustc +nightly`).
    #[serde(default)]
    pub cc: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub common_flags: Vec<String>,
    /// Name of the produced executable (default: project name).
    #[serde(default)]
    pub output: String,
}

#[derive(Debug, Deserialize)]
pub struct Coverage {
    /// Compiler override for the instrumented build (e.g. `rustc +nightly`).
    #[serde(default)]
    pub cc: String,
    /// Directory holding llvm-profdata / llvm-cov. Leave empty and, for a
    /// rustc project, they are taken from the coverage compiler's own sysroot
    /// — a profile written by nightly cannot be merged by stable's tools.
    #[serde(default)]
    pub llvm_bin: String,
    #[serde(default)]
    pub flags: Vec<String>,
    /// Command run in the coverage build directory to exercise the program;
    /// `{exe}` is replaced by the instrumented executable.
    pub run: Vec<String>,
    /// `custom` kind: full build command; `{out}` = output path.
    #[serde(default)]
    pub command: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Variant {
    pub name: String,
    #[serde(default)]
    pub flags: Vec<String>,
    /// `custom` kind: full build command; `{out}` = output path.
    #[serde(default)]
    pub command: Vec<String>,
}

fn split_cmd(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

fn run(desc: &str, cwd: &Path, argv: &[String], env: &[(&str, String)]) -> Result<()> {
    if argv.is_empty() {
        bail!("{desc}: empty command");
    }
    eprintln!("  $ {}", argv.join(" "));
    let mut c = Command::new(&argv[0]);
    // `build --serve` may hold a token in its environment; no compiler or
    // test binary it runs needs it (review of #65)
    c.env_remove("BUTTERFLY_TOKEN").args(&argv[1..]).current_dir(cwd);
    for (k, v) in env {
        c.env(k, v);
    }
    let st = c.status().with_context(|| format!("{desc}: cannot run {}", argv[0]))?;
    if !st.success() {
        bail!("{desc}: `{}` failed with {st}", argv.join(" "));
    }
    Ok(())
}

fn llvm_bin() -> Option<PathBuf> {
    butterfly_ingest::objectcode::find_llvm_tool("llvm-profdata", None).ok().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// A directory named by the environment, if it holds the tools.
///
/// `BUTTERFLY_LLVM_BIN` is butterfly's own; `LLVM_BIN` is the one the README
/// tells people to set for `examples/c/build.sh`, and which CI exports. Both
/// were ignored by `butterfly build`, which went straight to whatever rustup
/// had (review of #20).
fn env_llvm_bin() -> Option<PathBuf> {
    ["BUTTERFLY_LLVM_BIN", "LLVM_BIN"].iter().filter_map(std::env::var_os).map(PathBuf::from).find(|d| d.join("llvm-profdata").is_file())
}

/// Ask the compiler a configuration question, as a bounded helper probe
/// (#246): resolved to an absolute path first -- a bare name on PATH, once
/// -- then run with a cleaned environment, a short deadline and a small
/// ceiling. `None` for any failure: these probes only choose a default.
///
/// Asked in `dir`, the directory the compile it describes runs in, and a
/// relative path is resolved there as that compile resolves it. A rustup
/// proxy chooses its toolchain from the `rust-toolchain.toml` above its
/// working directory; asked from wherever butterfly was started, it named a
/// toolchain other than the one that compiled (#287).
fn probe(prog: &str, args: &[String], extra: &[&str], dir: &Path) -> Option<String> {
    let exe = resolve(prog, dir)?;
    let argv: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).chain(extra.iter().map(std::ffi::OsStr::new)).collect();
    let out = butterfly_ingest::helper::run_in(&exe, &argv, dir, butterfly_ingest::helper::Bounds::probe()).ok()?;
    Some(String::from_utf8_lossy(&out).trim().to_string())
}

/// The absolute path `prog` names when run in `dir`: a relative path is
/// resolved there, a bare name on PATH, once.
fn resolve(prog: &str, dir: &Path) -> Option<PathBuf> {
    if prog.contains('/') {
        std::path::absolute(dir.join(prog)).ok()
    } else {
        butterfly_ingest::provenance::resolve_on_path(prog)
    }
}

/// The compiler of `cmd` (which may carry a toolchain selector, as
/// `rustc +nightly` does), identified in `dir`, where it compiled: the first
/// line of its `--version`, and the path that ran. The version is empty when
/// it could not be read, as the variant compiler's is.
///
/// For `rustc` the path is the toolchain's own binary, `<sysroot>/bin/rustc`,
/// for the sysroot this invocation selects. On PATH, `rustc` is usually
/// rustup's proxy: one file for every toolchain, which chooses the real
/// compiler from `+toolchain` or the directory, so its path names none of
/// them (#305 review). The proxy's path is recorded only when the sysroot
/// cannot be read.
fn identify_compiler(cmd: &str, kind: &str, dir: &Path) -> (String, Option<PathBuf>) {
    let argv = split_cmd(cmd);
    let Some((prog, rest)) = argv.split_first() else { return (String::new(), None) };
    let version = probe(prog, rest, &["--version"], dir).and_then(|v| v.lines().next().map(str::to_string)).unwrap_or_default();
    let own = (kind == "rustc")
        .then(|| probe(prog, rest, &["--print=sysroot"], dir))
        .flatten()
        .map(|sysroot| PathBuf::from(sysroot).join("bin").join(Path::new(prog).file_name().unwrap_or(prog.as_ref())))
        .filter(|p| p.is_file());
    (version, own.or_else(|| resolve(prog, dir)))
}

/// The build-info lines naming what produced `coverage.json` (#298): the
/// compiler of the instrumented build, and the `llvm-profdata` and
/// `llvm-cov` that merged and exported its profile, each by the path that
/// ran and the version it gave where it ran. Profile and coverage-mapping
/// formats belong to one LLVM, so the pairing must be checkable from the
/// evidence, and a baseline must see it change. The keys are the ones the
/// shipped adapters write (`examples/adapter.sh`).
fn coverage_toolchain_info(compiler: &(String, Option<PathBuf>), profdata: &Path, cov: &Path, dir: &Path) -> String {
    let mut o = format!("compiler_instr={}\n", compiler.0);
    if let Some(p) = &compiler.1 {
        o.push_str(&format!("compiler_instr_path={}\n", p.display()));
    }
    for (key, tool) in [("llvm_profdata", profdata), ("llvm_cov", cov)] {
        let version = butterfly_ingest::provenance::version_line_in(tool, dir).unwrap_or_default();
        o.push_str(&format!("{key}={version}\n{key}_path={}\n", tool.display()));
    }
    o
}

/// The LLVM tools that belong to a clang, found through clang itself.
///
/// A raw profile carries a format version that `llvm-profdata` must match
/// exactly. Ubuntu's clang 18 writes version 9 and the rustup toolchain's
/// tools expect 10, so merging with whatever rustup happened to install
/// fails outright: "raw profile version mismatch ... no profile can be
/// merged". The tools that match are the ones shipped beside the compiler
/// that wrote the profile (review of #20).
///
/// `--print-resource-dir` gives `<prefix>/lib/clang/<version>`, so three
/// components up is the install prefix and `<prefix>/bin` holds the tools.
/// `-print-prog-name` is tried first because a compiler that answers it
/// answers precisely.
fn clang_llvm_bin(cc: &str, dir: &Path) -> Option<PathBuf> {
    let argv = split_cmd(cc);
    let (prog, args) = argv.split_first()?;
    let run = |extra: &str| probe(prog, args, &[extra], dir);
    let named = run("-print-prog-name=llvm-profdata").map(PathBuf::from).filter(|p| p.is_absolute() && p.is_file()).and_then(|p| p.parent().map(Path::to_path_buf));
    named.or_else(|| {
        let res = PathBuf::from(run("--print-resource-dir")?);
        let prefix = res.ancestors().nth(3)?;
        let dir = prefix.join("bin");
        dir.join("llvm-profdata").is_file().then_some(dir)
    })
}

/// `<sysroot>/lib/rustlib/<host>/bin` for the given rustc invocation, which is
/// where that toolchain's llvm-profdata and llvm-cov live.
fn rustc_llvm_bin(cc: &str, dir: &Path) -> Option<PathBuf> {
    let argv = split_cmd(cc);
    let (prog, args) = argv.split_first()?;
    let sysroot = probe(prog, args, &["--print=sysroot"], dir)?;
    let host = probe(prog, args, &["-vV"], dir)?.lines().find_map(|l| l.strip_prefix("host: ").map(str::to_string))?;
    let dir = PathBuf::from(sysroot).join("lib/rustlib").join(host).join("bin");
    dir.join("llvm-profdata").is_file().then_some(dir)
}

/// Which directory of LLVM tools merges and reads the profile.
///
/// The profile's format version is the compiler's, so the tools that can
/// read it are the compiler's too: a rustup toolchain that happens to be
/// newer cannot merge clang 18's output at all (review of #20). In order:
///
///   1. `[coverage] llvm_bin` -- the most specific statement there is.
///   2. For `rustc`, the coverage compiler's own sysroot. It is definitional:
///      the tools that built the profile live beside the compiler that wrote
///      it, and nothing in the environment knows better. `LLVM_BIN` in
///      particular is what the C examples and CI export for a *clang*, and
///      letting it override a rustc sysroot merged nightly's profile with
///      clang 18's `llvm-profdata` (self-review of #20).
///   3. `BUTTERFLY_LLVM_BIN`, then `LLVM_BIN`.
///   4. For `clang`, what the compiler reports. Inferred rather than
///      definitional -- `-print-prog-name` falls back to `PATH` -- which is
///      why the environment gets to speak first.
///   5. Whatever is installed.
///
/// Every source is a closure so nothing is probed that a higher rung has
/// already settled.
fn choose_llvm(
    kind: &str,
    configured: Option<PathBuf>,
    env: impl FnOnce() -> Option<PathBuf>,
    compiler: impl FnOnce() -> Option<PathBuf>,
    installed: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    if configured.is_some() {
        return configured;
    }
    if kind == "rustc" {
        compiler().or_else(env).or_else(installed)
    } else {
        env().or_else(compiler).or_else(installed)
    }
}

/// Run the whole pipeline; returns (variant name, evidence path) pairs, and
/// the source root they were ingested against -- which is what a viewer
/// started on them should approve for its freshness check (#248).
pub fn build(config_path: &Path) -> Result<(Vec<(String, PathBuf)>, PathBuf)> {
    // The build config is read like every other auxiliary artifact: sized
    // first, against the same cap `ingest` applies to `butterfly.toml`
    // (`config.rs`), so a crafted file fails with a sentence rather than by
    // allocating its full size (#231).
    let lim = butterfly_model::limits::Limits::from_env();
    // The message already names the file, so no `reading {path}` context.
    let text = butterfly_model::limits::read_bounded_string(config_path, butterfly_model::limits::Kind::Auxiliary, &lim).map_err(|e| anyhow::anyhow!("{e}"))?;
    // the message and position only, never toml's echo of the line (#342)
    let cfg: BuildConfig =
        toml::from_str(&text).map_err(|e| anyhow::anyhow!("parsing build config {}: {}", config_path.display(), butterfly_ingest::config::toml_error(&text, &e)))?;
    check_variant_names(&cfg.variants, config_path)?;
    // The executable name, like the variant names, is checked before
    // anything under the output directory is created or removed: a bad
    // configuration must not cost the previous run its evidence.
    let exe = if cfg.compiler.output.is_empty() { cfg.compiler.name_default() } else { cfg.compiler.output.clone() };
    check_output_name(&exe)?;
    let base = std::fs::canonicalize(config_path.parent().unwrap_or(Path::new(".")))?;
    let src_root = std::fs::canonicalize(base.join(&cfg.project.source_root))?;
    let out = base.join(&cfg.project.out_dir);
    std::fs::create_dir_all(&out)?;
    let out = std::fs::canonicalize(&out)?;
    // One build is one transaction: every output it writes is either this
    // run's or absent. The evidence and coverage export of an earlier run
    // go first, so a step that fails below cannot leave them beside this
    // run's as if the two were one build (#287).
    crate::remove_output(&out.join("coverage.json"))?;
    for v in &cfg.variants {
        crate::remove_output(&out.join(format!("evidence-{}.json", v.name)))?;
    }
    let kind = cfg.compiler.kind.as_str();
    // Prefer the LLVM tools of the toolchain that produced the profile.
    let cov_cc = if cfg.coverage.cc.is_empty() { cfg.compiler.cc.clone() } else { cfg.coverage.cc.clone() };
    let llvm = choose_llvm(
        kind,
        (!cfg.coverage.llvm_bin.is_empty()).then(|| base.join(&cfg.coverage.llvm_bin)),
        env_llvm_bin,
        // asked where the coverage build compiles, which is src_root
        || {
            if kind == "rustc" {
                rustc_llvm_bin(if cov_cc.is_empty() { "rustc" } else { &cov_cc }, &src_root)
            } else {
                clang_llvm_bin(if cov_cc.is_empty() { "clang" } else { &cov_cc }, &src_root)
            }
        },
        llvm_bin,
    );
    if let Some(d) = &llvm {
        eprintln!("llvm     : {}", d.display());
    }
    // Each tool by the absolute path that runs, so the one recorded is the
    // one that ran: from the chosen directory, or else PATH's, once.
    let tool = |n: &str| -> Result<PathBuf> {
        let p = match &llvm {
            Some(d) => d.join(n),
            None => butterfly_ingest::provenance::resolve_on_path(n).with_context(|| format!("no {n}: none was found beside the compiler or installed, and none is on PATH. Set `[coverage] llvm_bin` to the directory of the LLVM tools that match the coverage compiler."))?,
        };
        Ok(std::path::absolute(p)?)
    };
    let sources: Vec<String> = cfg.compiler.sources.iter().map(|s| src_root.join(s).display().to_string()).collect();
    let abs_paths = |flags: &[String]| -> Vec<String> { resolve_flag_paths(kind, &src_root, flags) };
    let common_flags = abs_paths(&cfg.compiler.common_flags);

    // ---- 1. instrumented build + run -> coverage.json ----------------------
    let cov_held = crate::output_subdir(&out, "coverage")?;
    let cov_dir = cov_held.path().to_path_buf();
    let instr_exe = cov_dir.join(format!("{exe}-instr"));
    let profraw = cov_dir.join("default.profraw");
    let profdata = cov_dir.join("default.profdata");
    // What an earlier run left here would otherwise stand in for anything
    // this run fails to produce: a run command that writes no profile had
    // the last run's merged, and a build command that ignores `{out}` had
    // the last run's executable analysed (#287). Removed through the
    // directory held open, never by a path resolved again (#343).
    for p in [&instr_exe, &profraw, &profdata] {
        crate::remove_output_in(&cov_held, file_name(p))?;
    }
    crate::remove_output_dir_in(&cov_held, file_name(&dsym_of(&instr_exe)))?;
    eprintln!("== coverage build");
    // The compiler of the instrumented build, identified where it compiled.
    // A `custom` command runs whatever it names, which butterfly cannot
    // identify, so it says so rather than name one that may not have run
    // (#287, #298).
    let compiler_instr = match kind {
        "custom" => ("not identified (custom coverage build command)".to_string(), None),
        _ => identify_compiler(if cov_cc.is_empty() { default_cc(kind) } else { cov_cc.clone() }.as_str(), kind, &src_root),
    };
    match kind {
        "clang" | "gcc" | "rustc" => {
            let cc = if cov_cc.is_empty() { default_cc(kind) } else { cov_cc.clone() };
            let mut argv = split_cmd(&cc);
            argv.extend(common_flags.iter().cloned());
            argv.extend(abs_paths(&cfg.coverage.flags));
            argv.extend(sources.iter().cloned());
            argv.push("-o".into());
            argv.push(instr_exe.display().to_string());
            run("coverage build", &src_root, &argv, &[])?;
        }
        "custom" => {
            let argv: Vec<String> = cfg.coverage.command.iter().map(|a| a.replace("{out}", &instr_exe.display().to_string())).collect();
            run("coverage build", &src_root, &argv, &[])?;
        }
        other => bail!("unknown compiler kind {other:?} (clang | gcc | rustc | custom)"),
    }
    produced("coverage build", &instr_exe)?;
    let run_argv: Vec<String> = cfg.coverage.run.iter().map(|a| a.replace("{exe}", &instr_exe.display().to_string())).collect();
    run("coverage run", &cov_dir, &run_argv, &[("LLVM_PROFILE_FILE", profraw.display().to_string())])?;
    if !profraw.is_file() {
        bail!(
            "coverage run: `{}` exited 0 but wrote no profile at {}. The instrumented program must run and exit normally with LLVM_PROFILE_FILE as given; `{{exe}}` in [coverage] run is replaced by {}.",
            run_argv.join(" "),
            profraw.display(),
            instr_exe.display()
        );
    }
    let llvm_profdata = tool("llvm-profdata")?;
    run(
        "llvm-profdata",
        &cov_dir,
        &[llvm_profdata.display().to_string(), "merge".into(), "-sparse".into(), profraw.display().to_string(), "-o".into(), profdata.display().to_string()],
        &[],
    )?;
    let cov_json = out.join("coverage.json");
    // A helper like any other (#246), and its output is the coverage export
    // `ingest` reads next, so it is held to the same cap here as there.
    let lim = butterfly_model::limits::Limits::from_env();
    let profile = format!("-instr-profile={}", profdata.display());
    let args: [&std::ffi::OsStr; 4] = ["export".as_ref(), "-format=text".as_ref(), profile.as_ref(), instr_exe.as_os_str()];
    let llvm_cov = tool("llvm-cov")?;
    let bounds = butterfly_ingest::helper::Bounds::standard().with_stdout(lim.cap(butterfly_model::limits::Kind::Coverage));
    // in the coverage directory, as the merge is, so each is identified
    // where it ran
    let export = match butterfly_ingest::helper::run_in(&llvm_cov, &args, &cov_dir, bounds) {
        Ok(out) => out,
        Err(butterfly_ingest::helper::HelperError::Failed { stderr, .. }) => bail!("llvm-cov export failed: {stderr}"),
        Err(e) => bail!("llvm-cov export: {e}"),
    };
    crate::write_output(&cov_json, &export)?;
    eprintln!("  wrote {}", cov_json.display());
    let coverage_toolchain = coverage_toolchain_info(&compiler_instr, &llvm_profdata, &llvm_cov, &cov_dir);

    // ---- 2. variants ----------------------------------------------------
    let mut results = Vec::new();
    for v in &cfg.variants {
        eprintln!("== variant {}", v.name);
        let vheld = crate::output_subdir(&out, &v.name)?;
        let vdir = vheld.path().to_path_buf();
        let vexe = vdir.join(&exe);
        // The executable, its dSYM, build-info and every `.su` file here
        // are this variant's outputs; an earlier compile's must not be read
        // as this one's -- a `.su` from a source since removed was (#287).
        // All of it through the directory held open: `out/<variant>` swapped
        // for a link after the check must not take the clean with it (#343).
        crate::remove_output_in(&vheld, file_name(&vexe))?;
        crate::remove_output_dir_in(&vheld, file_name(&dsym_of(&vexe)))?;
        crate::remove_output_in(&vheld, std::ffi::OsStr::new("build-info.txt"))?;
        for n in vheld.names().with_context(|| format!("reading {}", vdir.display()))? {
            if Path::new(&n).extension().is_some_and(|x| x == "su") {
                crate::remove_output_in(&vheld, &n)?;
            }
        }
        let flags_text;
        // the `.su` files this variant's own compiles wrote; nothing else in
        // `vdir/obj` is read (#328 review)
        let mut unit_su = Vec::new();
        match kind {
            "clang" | "gcc" | "rustc" => {
                let cc = if cfg.compiler.cc.is_empty() { default_cc(kind) } else { cfg.compiler.cc.clone() };
                let mut flags = common_flags.clone();
                flags.extend(abs_paths(&v.flags));
                // README: "Each variant is built with `-fstack-usage` where
                // the toolchain supports it" -- clang and gcc do; rustc does
                // not. Added unless the config already says either way, and
                // recorded with the flags it was built with (#325 F2).
                let added = stack_usage_flag(kind, &cfg.compiler.common_flags, &v.flags);
                flags.extend(added.iter().map(|f| f.to_string()));
                flags_text = format!("{} {} {}", cfg.compiler.common_flags.join(" "), v.flags.join(" "), added.unwrap_or_default());
                if kind == "rustc" {
                    let mut argv = split_cmd(&cc);
                    argv.extend(flags);
                    argv.extend(sources.iter().cloned());
                    argv.push("-o".into());
                    argv.push(vexe.display().to_string());
                    run("variant build", &vdir, &argv, &[])?;
                } else {
                    unit_su = compile_and_link(&cc, &flags, &sources, &vheld, &vexe)?;
                }
            }
            _ => {
                let argv: Vec<String> = v.command.iter().map(|a| a.replace("{out}", &vexe.display().to_string())).collect();
                flags_text = argv.join(" ");
                run("variant build", &vdir, &argv, &[])?;
            }
        }
        produced("variant build", &vexe)?;
        // The provenance version probe goes through the same bounded helper as
        // the configuration probes (#246): resolved path, cleared environment,
        // deadline and output ceiling. A bare `Command::output()` here had
        // none of those, so a slow or faulty compiler wrapper could stall or
        // buffer without bound. `split_first` keeps a toolchain selector such
        // as `rustc +nightly` on the command line rather than dropping it.
        //
        // Asked in the directory it compiled in (#287). A `custom` command
        // runs whatever it names, which butterfly cannot identify: `[compiler]
        // cc` is not what ran, and recording its version -- or clang's, the
        // default -- named a compiler that never ran.
        //
        // By path as well as banner, under the adapters' key (#298), as the
        // coverage compiler is.
        let (compiler_version, compiler_path) = if kind == "custom" {
            ("not identified (custom build command; see flags_target)".to_string(), None)
        } else {
            identify_compiler(&if cfg.compiler.cc.is_empty() { default_cc(kind) } else { cfg.compiler.cc.clone() }, kind, &vdir)
        };
        let compiler_path = compiler_path.map(|p| format!("compiler_path={}\n", p.display())).unwrap_or_default();
        crate::write_output(
            &vdir.join("build-info.txt"),
            format!(
                "project={}\nvariant={}\ncompiler={}\n{compiler_path}flags_target={}\nflags_instr={}\n{coverage_toolchain}butterfly_build=exploratory\n",
                cfg.project.name,
                v.name,
                compiler_version,
                flags_text.trim(),
                cfg.coverage.flags.join(" ")
            )
            .as_bytes(),
        )?;
        // The variant directory holds the executable, build-info and the
        // compiler's -fstack-usage output; the coverage export lives one level
        // up and is supplied below. Discovery failures are reported, never
        // swallowed -- silently defaulting here is how the .su files went
        // missing from every variant (#7).
        let (mut opts, _) = butterfly_ingest::discover_artifacts(&vdir).with_context(|| format!("discovering build artifacts in {}", vdir.display()))?;
        // Each unit's `.su` lies beside its object, one level down, and only
        // this run's are taken: a `vdir/obj` left by an earlier clang or gcc
        // run must not lend its frames to a later rustc or custom build of
        // the same variant (#328 review).
        opts.stack_usage.extend(unit_su);
        opts.stack_usage.sort();
        opts.coverage_json = cov_json.clone();
        opts.objects = vec![vexe.clone()];
        opts.build_info = Some(vdir.join("build-info.txt"));
        opts.source_root = Some(src_root.clone());
        opts.allow_external_source = cfg.project.allow_external_source;
        // The directory chosen for the profile tools is handed to ingest as
        // its disassembler's directory only when the operator named it in
        // the config -- then it is authoritative, and an error if it has no
        // llvm-objdump (#246) -- or when it really holds one. A directory
        // found from the compiler or from LLVM_BIN was chosen for
        // llvm-profdata, and must not turn "no llvm-objdump here" into a
        // failure the operator never asked for.
        opts.llvm_bin = llvm.clone().filter(|d| !cfg.coverage.llvm_bin.is_empty() || d.join("llvm-objdump").is_file());
        let mut ev = butterfly_ingest::ingest(&opts)?;
        ev.provenance.caveats.insert(0, format!("EXPLORATORY BUILD: variant `{}` was compiled by `butterfly build`, not by the configuration-controlled build. Use it to understand the optimiser; it is not certification evidence.", v.name));
        let ev_path = out.join(format!("evidence-{}.json", v.name));
        crate::write_output(&ev_path, serde_json::to_string(&ev)?.as_bytes())?;
        eprintln!("  wrote {}  ({} instr, {} gaps)", ev_path.display(), ev.summary.instructions_total, ev.summary.gaps_total);
        results.push((v.name.clone(), ev_path));
    }
    Ok((results, src_root))
}

/// Resolve relative paths in `-I`, `-isystem`, `-L`, and `-include` flags
/// against `src_root`, because variants compile with cwd set to their output
/// directory.
///
/// Only flags that legitimately take a joined path (`-I`, `-isystem`, `-L`)
/// are prefix-matched, and a suffix starting with `-` is left untouched so
/// distinct options such as `-include-pch`, `-isystem-after`, or `-I-` are
/// never corrupted. For `rustc`, `-L [KIND=]PATH` splits an optional search
/// kind before resolving the path portion.
fn resolve_flag_paths(kind: &str, src_root: &Path, flags: &[String]) -> Vec<String> {
    const SEPARATE: [&str; 4] = ["-I", "-isystem", "-L", "-include"];
    const JOINED: [&str; 3] = ["-I", "-isystem", "-L"];
    let resolve_val = |flag: &str, val: &str| -> String {
        if kind == "rustc" && flag == "-L" {
            if let Some((k, rest)) = val.split_once('=') {
                if matches!(k, "dependency" | "crate" | "native" | "framework" | "all") {
                    if Path::new(rest).is_absolute() {
                        return val.to_string();
                    }
                    return format!("{k}={}", src_root.join(rest).display());
                }
            }
        }
        if Path::new(val).is_absolute() {
            val.to_string()
        } else {
            src_root.join(val).display().to_string()
        }
    };
    let mut out: Vec<String> = Vec::with_capacity(flags.len());
    let mut pending: Option<&str> = None;
    for fl in flags {
        if let Some(prev) = pending.take() {
            out.push(resolve_val(prev, fl));
            continue;
        }
        if let Some(&p) = SEPARATE.iter().find(|&&p| fl == p) {
            pending = Some(p);
            out.push(fl.clone());
            continue;
        }
        match JOINED.iter().find(|&&p| fl.starts_with(p) && fl.len() > p.len() && !fl[p.len()..].starts_with('-')) {
            Some(&p) => out.push(format!("{p}{}", resolve_val(p, &fl[p.len()..]))),
            None => out.push(fl.clone()),
        }
    }
    out
}

/// `-fstack-usage`, when a clang or gcc variant's flags do not already
/// mention it or its `-fno-` form; `None` otherwise (#325 F2).
fn stack_usage_flag(kind: &str, common: &[String], variant: &[String]) -> Option<&'static str> {
    let said = common.iter().chain(variant).any(|f| f == "-fstack-usage" || f == "-fno-stack-usage");
    (matches!(kind, "clang" | "gcc") && !said).then_some("-fstack-usage")
}

/// Where a variant's objects go, and their `-fstack-usage` output with them.
const OBJ_DIR: &str = "obj";

/// One object per source in `vdir/obj`, then one link: the executable is made
/// of the same objects, compiled with the same flags, as before.
///
/// Compiled and linked in one step, clang names every unit's `-fstack-usage`
/// output after the executable, so each overwrites the last and only one
/// unit's frames survive; ingest then refuses the variant (#325 F3). A
/// compile of its own writes each unit's `.su` beside its object, under a
/// name of its own. On macOS the debug information of a link of separate
/// objects stays in those objects, and the one-step build ran `dsymutil` to
/// collect it; so does this, as the compiler driver did.
///
/// Every object is named `<index>-<stem>.o`. The index alone makes the name
/// unique; a suffix added only to stems that repeat could equal another
/// source's own stem (`a/foo.c`, `b/foo.c` and `foo-0.c` gave two `foo-0.o`),
/// and one object -- and its `.su` -- overwrote the other (#328 review). The
/// directory is emptied first, so no object or `.su` of an earlier run, from
/// a source since removed, is linked or read as this one's.
///
/// The configured flags are split by [`phase_flags`]: a linker-only option
/// given to `-c` is an unused argument, which `-Werror` makes an error.
///
/// Returns the `.su` files these compiles wrote, in source order. Discovery
/// reads the variant directory itself, not below it, so they are supplied to
/// ingest explicitly -- and only they, never whatever else `vdir/obj` holds.
fn compile_and_link(cc: &str, flags: &[String], sources: &[String], vheld: &butterfly_ingest::fsauth::HeldDir, vexe: &Path) -> Result<Vec<PathBuf>> {
    let vdir = vheld.path();
    let obj_dir = fresh_obj_dir(vheld)?;
    let (compile, link) = phase_flags(flags);
    let mut objects = Vec::with_capacity(sources.len());
    let mut su = Vec::new();
    for (i, src) in sources.iter().enumerate() {
        let obj = obj_dir.join(object_name(i, src));
        let mut argv = split_cmd(cc);
        argv.extend(compile.iter().cloned());
        argv.extend(["-c".into(), src.clone(), "-o".into(), obj.display().to_string()]);
        run("variant build", vdir, &argv, &[])?;
        let unit = obj.with_extension("su");
        if std::fs::symlink_metadata(&unit).is_ok_and(|m| m.is_file()) {
            su.push(unit);
        }
        objects.push(obj.display().to_string());
    }
    let mut argv = split_cmd(cc);
    argv.extend(link.iter().cloned());
    argv.extend(objects);
    argv.push("-o".into());
    argv.push(vexe.display().to_string());
    run("variant link", vdir, &argv, &[])?;
    if is_mach_o(vexe) && flags.iter().any(|f| f.starts_with("-g") && f != "-g0") {
        let (prog, args) = split_cmd(cc).split_first().map(|(p, a)| (p.clone(), a.to_vec())).unwrap_or_default();
        let dsymutil = probe(&prog, &args, &["-print-prog-name=dsymutil"], vdir).filter(|p| Path::new(p).is_absolute()).unwrap_or_else(|| "dsymutil".into());
        run("dsymutil", vdir, &[dsymutil, vexe.display().to_string()], &[])?;
    }
    Ok(su)
}

/// `<index>-<stem>.o`: unique for every index, whatever the stems are.
fn object_name(i: usize, src: &str) -> String {
    let stem = Path::new(src).file_stem().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "src".into());
    format!("{i}-{stem}.o")
}

/// An empty `obj` in the variant directory, whatever was there: a symlink is
/// removed, never followed, and an earlier run's tree is removed through
/// descriptors from the variant directory held open -- so `out/<variant>`
/// swapped for a link after it was checked cannot redirect the removal into
/// the link's target (#343). Returns the path the compiles are given.
fn fresh_obj_dir(vheld: &butterfly_ingest::fsauth::HeldDir) -> Result<PathBuf> {
    let obj = vheld.fresh_subdir(std::ffi::OsStr::new(OBJ_DIR)).with_context(|| format!("emptying {}", vheld.path().join(OBJ_DIR).display()))?;
    Ok(obj.path().to_path_buf())
}

/// The configured flags split into those for each `-c` compile and those for
/// the link, in their order.
///
/// `common_flags` and a variant's `flags` were one list given to one
/// compile-and-link, where every option found its phase. Given to `clang -c`
/// as well, a linker option (`-Wl,--as-needed`, `-lm`) is an unused argument
/// and `-Werror` fails the compile (#328 review). So:
///
///   * linker-only options go to the link alone: `-Wl,*`, `-Xlinker <arg>`,
///     `-l`/`-L` (joined or not), `-framework`/`-weak_framework <arg>`, `-T`, `-u <sym>`,
///     `-e <sym>`, `-z <kw>`, `-undefined <arg>`, `-fuse-ld=`, `--ld-path=`,
///     `-rtlib=`, `-unwindlib=`, `-static*`, `-shared*`, `-pie`, `-no-pie`,
///     `-rdynamic`, `-nostdlib`, `-nodefaultlibs`, `-nostartfiles`, `-nolibc`,
///     `-s`, `-B{static,dynamic,symbolic}`, `-dead_strip`, `-dynamiclib`,
///     `-bundle`, and libraries or objects named as inputs;
///   * preprocessor and front-end options go to the compiles alone: `-I`,
///     `-D`, `-U`, `-include`, `-imacros`, `-isystem`, `-iquote`,
///     `-idirafter`, `-std=`, `-M*`, `-Wp,*`, `-Wa,*`, `-x <lang>` (which at
///     the link would make the objects source), `-Xclang <arg>`,
///     `-Xpreprocessor <arg>`, `-Xassembler <arg>`, `-nostdinc*`, and
///     `-mllvm <arg>` unless the build uses LTO, whose link needs it too;
///   * everything else -- optimisation, `-g`, `-f*` code generation, coverage,
///     sanitizers, `-m*`, `-arch`, `--target`, `--sysroot`, `-isysroot`,
///     `-stdlib=`, `-pthread`, warnings -- goes to both, as before.
fn phase_flags(flags: &[String]) -> (Vec<String>, Vec<String>) {
    #[derive(Clone, Copy, PartialEq)]
    enum Phase {
        Compile,
        Link,
        Both,
    }
    const LINK_ARG: [&str; 10] = ["-Xlinker", "-l", "-L", "-framework", "-weak_framework", "-T", "-u", "-e", "-z", "-undefined"];
    const LINK_EXACT: [&str; 22] = [
        "-static",
        "-static-libgcc",
        "-static-libstdc++",
        "-static-pie",
        "-shared",
        "-shared-libgcc",
        "-pie",
        "-no-pie",
        "-nopie",
        "-rdynamic",
        "-nostdlib",
        "-nodefaultlibs",
        "-nostartfiles",
        "-nolibc",
        "-s",
        "-Bstatic",
        "-Bdynamic",
        "-Bsymbolic",
        "-dead_strip",
        "-dynamiclib",
        "-bundle",
        "-headerpad_max_install_names",
    ];
    const LINK_PREFIX: [&str; 10] = ["-Wl,", "-l", "-L", "-T", "-fuse-ld=", "--ld-path=", "-rtlib=", "--rtlib=", "-unwindlib=", "--unwindlib="];
    const COMPILE_ARG: [&str; 15] =
        ["-I", "-D", "-U", "-include", "-imacros", "-isystem", "-iquote", "-idirafter", "-x", "-MF", "-MT", "-MQ", "-Xclang", "-Xpreprocessor", "-Xassembler"];
    // taken with their argument, so that argument is never read as an input
    const BOTH_ARG: [&str; 6] = ["-arch", "-target", "--target", "--sysroot", "-isysroot", "-B"];
    const COMPILE_PREFIX: [&str; 13] = ["-I", "-D", "-U", "-include", "-imacros", "-isystem", "-iquote", "-idirafter", "-std=", "-M", "-Wp,", "-Wa,", "-nostdinc"];
    const LINK_INPUT: [&str; 7] = [".a", ".o", ".so", ".dylib", ".tbd", ".lib", ".obj"];
    let lto = flags.iter().any(|f| f.starts_with("-flto"));
    let (mut compile, mut link) = (Vec::new(), Vec::new());
    let mut it = flags.iter();
    while let Some(f) = it.next() {
        let s = f.as_str();
        let (phase, takes_arg) = if LINK_ARG.contains(&s) {
            (Phase::Link, true)
        } else if COMPILE_ARG.contains(&s) {
            (Phase::Compile, true)
        } else if BOTH_ARG.contains(&s) {
            (Phase::Both, true)
        } else if s == "-mllvm" {
            (if lto { Phase::Both } else { Phase::Compile }, true)
        } else if LINK_EXACT.contains(&s) || LINK_PREFIX.iter().any(|p| s.starts_with(p)) {
            (Phase::Link, false)
        } else if COMPILE_PREFIX.iter().any(|p| s.starts_with(p)) {
            (Phase::Compile, false)
        } else if !s.starts_with('-') && (LINK_INPUT.iter().any(|x| s.ends_with(x)) || s.contains(".so.")) {
            (Phase::Link, false)
        } else {
            (Phase::Both, false)
        };
        let mut group = vec![f.clone()];
        if takes_arg {
            group.extend(it.next().cloned());
        }
        if phase != Phase::Link {
            compile.extend(group.iter().cloned());
        }
        if phase != Phase::Compile {
            link.extend(group);
        }
    }
    (compile, link)
}

/// A Mach-O image, thin or universal, by its magic number.
fn is_mach_o(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    let read = std::fs::File::open(path).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut magic));
    read.is_ok() && matches!(magic, [0xcf, 0xfa, 0xed, 0xfe] | [0xce, 0xfa, 0xed, 0xfe] | [0xfe, 0xed, 0xfa, 0xcf] | [0xfe, 0xed, 0xfa, 0xce] | [0xca, 0xfe, 0xba, 0xbe])
}

/// Variant names become a directory under the output directory and part of
/// an evidence file name, and that directory is cleaned before each build.
/// So each must be one plain path component, not the coverage build's
/// directory, and distinct: two variants sharing a directory mixed their
/// outputs, and one that named `..` was cleaned outside the output (#287).
/// The executable output name is joined onto the build directory, and its
/// `.dSYM` sibling is removed before each build (#287 review). Hold it to a
/// single path component -- no separator, no `..` -- so a configured
/// `compiler.output` cannot direct those writes or removals outside the build
/// tree. `coverage` is refused as well: that is the coverage build's own
/// directory.
fn check_output_name(name: &str) -> Result<()> {
    let mut parts = Path::new(name).components();
    let plain = matches!((parts.next(), parts.next()), (Some(std::path::Component::Normal(_)), None)) && !name.contains(['/', '\\']);
    if !plain || name == "coverage" {
        bail!("compiler.output {name:?} must be a single file name, without a path separator or `..`, and not `coverage` (the coverage build's directory)");
    }
    Ok(())
}

fn check_variant_names(variants: &[Variant], config: &Path) -> Result<()> {
    // Name the config as well as the variant: the build is refused before
    // anything runs, so the file is the only place there is to look (#325).
    let mut seen = std::collections::BTreeMap::new();
    for (i, v) in variants.iter().enumerate() {
        let mut parts = Path::new(&v.name).components();
        let plain = matches!((parts.next(), parts.next()), (Some(std::path::Component::Normal(_)), None)) && !v.name.contains(['/', '\\']);
        if !plain || v.name == "coverage" {
            bail!("{}: variant name {:?} must be a single directory name, and not `coverage` (the coverage build's directory)", config.display(), v.name);
        }
        if let Some(first) = seen.insert(v.name.as_str(), i + 1) {
            bail!(
                "{}: variant name {:?} is used twice ([[variant]] {first} and {}); each variant needs its own directory and evidence file, so nothing was built",
                config.display(),
                v.name,
                i + 1
            );
        }
    }
    Ok(())
}

/// The dSYM bundle a Mach-O link step writes beside `exe`.
/// The final component of a path the build made by joining a name onto one
/// of its directories.
fn file_name(p: &Path) -> &std::ffi::OsStr {
    p.file_name().unwrap_or_default()
}

fn dsym_of(exe: &Path) -> PathBuf {
    PathBuf::from(format!("{}.dSYM", exe.display()))
}

/// A build step's output exists: the step ran and wrote it. A step that
/// exits 0 without writing `{out}` is a failure, not permission to read
/// whatever an earlier run left there (#287).
fn produced(step: &str, path: &Path) -> Result<()> {
    if !path.is_file() {
        bail!("{step}: the command exited 0 but did not write {}. A custom command must write the file named by {{out}}.", path.display());
    }
    Ok(())
}

fn default_cc(kind: &str) -> String {
    match kind {
        "gcc" => "gcc".into(),
        "rustc" => "rustc".into(),
        _ => "clang".into(),
    }
}

impl Compiler {
    fn name_default(&self) -> String {
        self.sources.first().and_then(|s| Path::new(s).file_stem().map(|f| f.to_string_lossy().into_owned())).unwrap_or_else(|| "a.out".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `out/<variant>` is checked once and then used. Swapped for a symlink
    /// after that check, the variant's `obj` resolved through the link and
    /// the attacker's target lost a directory named `obj` (#343). The swap is
    /// made deterministically here, between the check and the clean.
    #[cfg(unix)]
    #[test]
    fn a_variant_directory_swapped_for_a_symlink_after_the_check_does_not_redirect_the_clean() {
        let root = butterfly_testsupport::scratch_dir("vdir-swap");
        let out = root.join("out");
        std::fs::create_dir(&out).unwrap();
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("obj")).unwrap();
        std::fs::write(elsewhere.join("obj/precious"), "the attacker's target, never ours").unwrap();

        let vdir = crate::output_subdir(&out, "O0").unwrap();
        std::fs::create_dir(out.join("O0/obj")).unwrap();
        std::fs::write(out.join("O0/obj/stale.o"), "an earlier run's object").unwrap();
        // the swap: the checked directory moves away, a link takes its name
        std::fs::rename(out.join("O0"), root.join("moved")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, out.join("O0")).unwrap();

        let obj = fresh_obj_dir(&vdir).unwrap();
        assert_eq!(std::fs::read_to_string(elsewhere.join("obj/precious")).unwrap(), "the attacker's target, never ours", "the clean went through the swapped-in link");
        assert!(!root.join("moved/obj/stale.o").exists(), "the directory that was checked is the one cleaned");
        assert!(root.join("moved/obj").is_dir(), "and its obj made afresh");
        let _ = obj;
        let _ = std::fs::remove_dir_all(root);
    }

    fn p(s: &str) -> Option<PathBuf> {
        Some(PathBuf::from(s))
    }

    /// `compiler.output` is joined onto the build tree and its `.dSYM` removed,
    /// so it must be a single file name -- never a path that could reach
    /// outside, and never the coverage build's own directory (#287 review).
    #[test]
    fn the_output_name_must_be_a_single_contained_file_name() {
        assert!(check_output_name("demo").is_ok());
        assert!(check_output_name("flight").is_ok());
        // a separator, a parent ref, an absolute path, or the reserved name
        for bad in ["../demo", "a/b", "/etc/demo", "sub/demo", "..", "coverage", "a\\b"] {
            assert!(check_output_name(bad).is_err(), "{bad:?} must be refused");
        }
    }

    /// The ordering, rung by rung, for both compiler kinds.
    #[test]
    fn llvm_tools_are_chosen_in_the_documented_order() {
        let cfg = p("/cfg");
        // the config outranks everything, for both kinds
        assert_eq!(choose_llvm("clang", cfg.clone(), || p("/env"), || p("/cc"), || p("/inst")), cfg);
        assert_eq!(choose_llvm("rustc", cfg.clone(), || p("/env"), || p("/cc"), || p("/inst")), cfg);

        // clang: the environment speaks before an inferred compiler answer
        assert_eq!(choose_llvm("clang", None, || p("/env"), || p("/cc"), || p("/inst")), p("/env"));
        assert_eq!(choose_llvm("clang", None, || None, || p("/cc"), || p("/inst")), p("/cc"));
        assert_eq!(choose_llvm("clang", None, || None, || None, || p("/inst")), p("/inst"));

        // rustc: the sysroot is definitional and is not overridden by an
        // LLVM_BIN that was exported for a clang
        assert_eq!(choose_llvm("rustc", None, || p("/env"), || p("/sysroot"), || p("/inst")), p("/sysroot"));
        assert_eq!(choose_llvm("rustc", None, || p("/env"), || None, || p("/inst")), p("/env"));
        assert_eq!(choose_llvm("rustc", None, || None, || None, || p("/inst")), p("/inst"));
        assert_eq!(choose_llvm("rustc", None, || None, || None, || None), None);
    }

    /// A settled rung must not probe the ones below it: they run compilers.
    #[test]
    fn lower_rungs_are_not_probed_once_one_answers() {
        let boom = || -> Option<PathBuf> { panic!("a lower rung was probed") };
        assert_eq!(choose_llvm("clang", p("/cfg"), boom, boom, boom), p("/cfg"));
        assert_eq!(choose_llvm("clang", None, || p("/env"), boom, boom), p("/env"));
        assert_eq!(choose_llvm("rustc", None, boom, || p("/sysroot"), boom), p("/sysroot"));
    }

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// Linker-only options go to the link alone, front-end ones to the
    /// compiles alone, the rest to both, each with its argument (#328).
    #[test]
    fn flags_are_routed_to_the_phase_that_uses_them() {
        let (c, l) = phase_flags(&v(&[
            "-g",
            "-Werror",
            "-Wl,--as-needed",
            "-O2",
            "-Iinclude",
            "-D",
            "X=1",
            "-lm",
            "-L",
            "/opt/lib",
            "-Xlinker",
            "-dead_strip",
            "-fuse-ld=lld",
            "-static",
            "-pie",
            "-no-pie",
            "-rdynamic",
            "-shared",
            "-std=c11",
            "-x",
            "c",
            "-fstack-usage",
            "-fsanitize=address",
            "-march=native",
            "-arch",
            "arm64",
            "--target=aarch64-linux-gnu",
            "-isysroot",
            "/sdk",
            "--sysroot",
            "/sys",
            "libfoo.a",
            "-framework",
            "CoreFoundation",
            "-mllvm",
            "-inline-threshold=5",
            "-MD",
            "-include",
            "pre.h",
        ]));
        assert_eq!(
            c,
            v(&[
                "-g",
                "-Werror",
                "-O2",
                "-Iinclude",
                "-D",
                "X=1",
                "-std=c11",
                "-x",
                "c",
                "-fstack-usage",
                "-fsanitize=address",
                "-march=native",
                "-arch",
                "arm64",
                "--target=aarch64-linux-gnu",
                "-isysroot",
                "/sdk",
                "--sysroot",
                "/sys",
                "-mllvm",
                "-inline-threshold=5",
                "-MD",
                "-include",
                "pre.h",
            ])
        );
        assert_eq!(
            l,
            v(&[
                "-g",
                "-Werror",
                "-Wl,--as-needed",
                "-O2",
                "-lm",
                "-L",
                "/opt/lib",
                "-Xlinker",
                "-dead_strip",
                "-fuse-ld=lld",
                "-static",
                "-pie",
                "-no-pie",
                "-rdynamic",
                "-shared",
                "-fstack-usage",
                "-fsanitize=address",
                "-march=native",
                "-arch",
                "arm64",
                "--target=aarch64-linux-gnu",
                "-isysroot",
                "/sdk",
                "--sysroot",
                "/sys",
                "libfoo.a",
                "-framework",
                "CoreFoundation",
            ])
        );
        // with LTO the link runs the optimiser, and takes -mllvm too
        let (_, l) = phase_flags(&v(&["-flto", "-mllvm", "-inline-threshold=5"]));
        assert_eq!(l, v(&["-flto", "-mllvm", "-inline-threshold=5"]));
    }

    /// The reviewer's case: a suffix only for repeated stems made `foo-0.o`
    /// twice. The index makes every name distinct (#328 review).
    #[test]
    fn object_names_are_unique_whatever_the_stems() {
        let srcs = ["/p/a/foo.c", "/p/b/foo.c", "/p/foo-0.c", "/p/1-foo.c", "/p/x.c"];
        let names: Vec<String> = srcs.iter().enumerate().map(|(i, s)| object_name(i, s)).collect();
        let set: std::collections::BTreeSet<&String> = names.iter().collect();
        assert_eq!(set.len(), names.len(), "{names:?}");
        assert_eq!(names[0], "0-foo.o");
        assert_eq!(names[2], "2-foo-0.o");
    }

    #[test]
    fn resolve_flag_paths_preserves_prefix_flags_and_rustc_search_kinds() {
        let root = Path::new("/proj");
        let clang = resolve_flag_paths(
            "clang",
            root,
            &v(&["-Iinclude", "-I", "inc2", "-I-", "-include", "pre.h", "-include-pch", "foo.pch", "-isystem-after", "sys", "-isystem", "sys2", "-Llib", "-L/abs/lib"]),
        );
        assert_eq!(
            clang,
            v(&[
                "-I/proj/include",
                "-I",
                "/proj/inc2",
                "-I-",
                "-include",
                "/proj/pre.h",
                "-include-pch",
                "foo.pch",
                "-isystem-after",
                "sys",
                "-isystem",
                "/proj/sys2",
                "-L/proj/lib",
                "-L/abs/lib",
            ])
        );

        let rustc = resolve_flag_paths("rustc", root, &v(&["-L", "dependency=target/deps", "-Ldependency=target/deps", "-L", "native=/abs/lib", "-L", "plain_dir"]));
        assert_eq!(rustc, v(&["-L", "dependency=/proj/target/deps", "-Ldependency=/proj/target/deps", "-L", "native=/abs/lib", "-L", "/proj/plain_dir"]));
    }
}
