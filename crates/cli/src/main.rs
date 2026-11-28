// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

mod builder;
mod run;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};

/// `0.2.0 (commit <id>, clean tree)`: the version and the build, from the
/// same description `ingest` writes into `provenance` (#117), so the binary
/// and its evidence name one build the same way. clap wants a `&'static
/// str`; the string is built once and kept for the process.
fn version_and_build() -> &'static str {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(butterfly_ingest::tool_version_and_build)
}

/// DO-178C structural-coverage and object-code evidence browser.
#[derive(Parser)]
#[command(name = "butterfly", version = version_and_build(), about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Parse compiler/coverage artifacts into an evidence database (JSON).
    Ingest {
        /// Build directory to auto-discover artifacts in (coverage export
        /// JSON, non-instrumented binaries, build-info.txt).
        dir: Option<PathBuf>,
        /// llvm-cov export JSON (overrides discovery).
        #[arg(long)]
        coverage: Option<PathBuf>,
        /// Executable / object file to analyse (repeatable).
        #[arg(long = "object")]
        objects: Vec<PathBuf>,
        /// key=value file recorded in provenance (compiler, flags, git hash).
        #[arg(long)]
        build_info: Option<PathBuf>,
        /// Root used to shorten displayed source paths, and the bound on
        /// which source files may be read (default: the working directory).
        #[arg(long)]
        source_root: Option<PathBuf>,
        /// Read source files that resolve outside --source-root. Coverage
        /// filenames are untrusted input and their contents are embedded in
        /// the evidence; use this only for generated or toolchain sources you
        /// intend to include.
        #[arg(long)]
        allow_external_source: bool,
        /// Directory containing llvm-objdump (default: rustup llvm-tools).
        #[arg(long)]
        llvm_bin: Option<PathBuf>,
        /// Execution trace for measured object coverage (QEMU `-d
        /// in_asm,exec,nochain` log, execlog CSV, or one address per line).
        #[arg(long)]
        trace: Option<PathBuf>,
        /// Attach aggregate coverage from a SEGGER Ozone Code Profile Report (text).
        #[arg(long)]
        ozone_report: Option<PathBuf>,
        /// `.su` file from `-fstack-usage` (repeatable).
        #[arg(long = "stack-usage")]
        stack_usage: Vec<PathBuf>,
        /// A requirements universe: one ID per line, `#` comments allowed.
        /// Lets the report list requirements that no code is tagged with
        #[arg(long = "requirements")]
        requirements: Option<PathBuf>,
        /// One coverage export per test, so the evidence can say which test
        /// exercised what. Repeatable, or point it at a directory and every
        /// *.json inside is taken as one test, named by its file stem. The
        /// merged --coverage export stays the source of every count; these
        /// only add attribution.
        #[arg(long = "tests")]
        tests: Vec<PathBuf>,
        /// Frozen analysis baseline to compare against (see `baseline`).
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Project configuration (butterfly.toml): requirement-tag patterns.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Output file.
        #[arg(short, long, default_value = "evidence.json")]
        out: PathBuf,
    },
    /// Exploratory build: compile the sources in several variants (e.g.
    /// -O0/-O1/-O2), run the instrumented build for coverage, ingest each
    /// variant. Output is stamped "exploratory" - not certification evidence.
    Build {
        /// butterfly-build.toml
        config: PathBuf,
        /// Serve the results afterwards.
        #[arg(long)]
        serve: bool,
        #[command(flatten)]
        net: ServeFlags,
    },
    /// Serve the browser UI over one or more evidence databases (several
    /// files = variants of the same sources, comparable in the UI).
    Serve {
        /// evidence.json produced by `ingest` (repeat for variants).
        #[arg(required = true)]
        evidence: Vec<PathBuf>,
        /// Sidecar file for coverage-resolution annotations.
        #[arg(long, default_value = "resolutions.json")]
        resolutions: PathBuf,
        #[command(flatten)]
        net: ServeFlags,
    },
    /// Read a SEGGER Ozone Code Profile Report and print its coverage summary as JSON.
    Ozone { report: PathBuf },
    /// Print a short textual summary of an evidence database.
    Summary { evidence: PathBuf },
    /// Optional: write a source definition index (<evidence-stem>.tags.json)
    /// beside the evidence, for "go to definition" in the viewer. Needs Universal Ctags;
    /// entirely separate from the evidence, and its absence changes nothing.
    Tags {
        evidence: PathBuf,
        /// Bound the index to this root: files the evidence names outside it are
        /// not read. Default: the working directory, as `ingest` uses.
        #[arg(long)]
        source_root: Option<PathBuf>,
        /// Where to write the index. Default: <evidence-stem>.tags.json next to
        /// the evidence (evidence.tags.json for evidence.json), where serve
        /// looks for it.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Path to a Universal Ctags binary (default: $BUTTERFLY_CTAGS, then `ctags` on PATH).
        #[arg(long)]
        ctags_bin: Option<PathBuf>,
    },
    /// Optional: read low-level requirement text from YAML, CSV or Markdown
    /// into a sidecar (requirements.json), so the viewer can show what a
    /// `@satisfies` tag refers to. Entirely separate from the evidence, whose
    /// absence changes nothing except that a tag shows its ID alone.
    Requirements {
        /// The requirement documents. The format is the file's extension:
        /// .yaml/.yml, .csv, or .md/.markdown. A directory is walked for
        /// `llr.yaml`, `llr.yml`, `llr.csv`, `llr.md` and `llr.markdown`;
        /// anything else is taken as a file, and may be repeated.
        #[arg(required = true)]
        input: Vec<PathBuf>,
        /// Where to write it. Default: requirements.json in the working directory.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// butterfly.toml, whose `[requirements.keys]` says where each field
        /// lives in your documents. Without one the defaults are assumed
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Build, check, ingest, report and serve in one go, saying what went
    /// well and what did not. Runs YOUR build script, because you invoked it
    /// from your own terminal -- the viewer never does (see SECURITY.md).
    Run {
        /// Project directory (default: the working directory).
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// The build script to run first.
        #[arg(long, default_value = "build.sh")]
        build_script: PathBuf,
        /// Skip the build and use what is already in the output directory.
        #[arg(long)]
        no_build: bool,
        /// Where the build script puts its output, relative to `dir`.
        #[arg(long, default_value = "build")]
        out_dir: PathBuf,
        /// Stop after writing the report instead of serving it.
        #[arg(long)]
        no_serve: bool,
        /// Carry on even when doctor says the evidence would be wrong.
        #[arg(long)]
        force: bool,
        /// Report doctor's notes as well as its problems.
        #[arg(short, long)]
        verbose: bool,
        /// A requirements document to fold into the report: YAML, CSV or
        /// Markdown (or a directory of llr documents), read as `butterfly
        /// requirements` reads it, or the requirements.json it wrote.
        #[arg(long)]
        requirements: Option<PathBuf>,
        /// Project configuration (butterfly.toml).
        #[arg(long)]
        config: Option<PathBuf>,
        #[command(flatten)]
        net: ServeFlags,
    },
    /// Check a build directory before ingesting it: is everything butterfly
    /// needs here, and if not, what should the build produce instead. Exit
    /// status 3 when something would make the evidence wrong rather than
    /// merely thinner.
    Doctor {
        /// The build directory that would be ingested.
        dir: PathBuf,
        /// Report the notes as well as the problems.
        #[arg(short, long)]
        verbose: bool,
    },
    /// Compare the requirements document's own traceability claims with what
    /// the build measured: code it says implements a requirement, tests it
    /// says verify one, and source tags citing a requirement it does not
    /// hold. Exit status 3 when the two disagree, for CI.
    Claims {
        evidence: PathBuf,
        /// The requirements sidecar, from `butterfly requirements`.
        #[arg(short, long)]
        requirements: PathBuf,
        /// Machine-readable disagreements on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Write a self-contained HTML report (Software Verification Results).
    Report {
        evidence: PathBuf,
        /// Resolutions sidecar, and it must load. Default: resolutions.json next to the evidence, else in the current directory.
        #[arg(long)]
        resolutions: Option<PathBuf>,
        /// Requirement text, so the report quotes each requirement rather
        /// than naming it. Default: requirements.json beside the evidence.
        /// The report is read away from this tool, so it must carry what it
        /// shows -- it cannot fetch a statement the way the viewer does
        #[arg(long)]
        requirements: Option<PathBuf>,
        #[arg(short, long, default_value = "butterfly-report.html")]
        out: PathBuf,
    },
    /// Freeze an analysis baseline (compiler, options, constructs, per-function
    /// fingerprints) from an evidence file. Later ingests compare against it
    /// (DO-178C 4.4.2.c) and flag stale analyses.
    Baseline {
        evidence: PathBuf,
        #[arg(short, long, default_value = "butterfly-baseline.json")]
        out: PathBuf,
    },
    /// Change impact (DO-178C 12.1.1): for every function, the commits and
    /// tickets that touched it, read from git history with `git log -L` over
    /// its line range. Writes the result into the evidence and a cache next
    /// to it so a re-run only walks what changed.
    Changes {
        evidence: PathBuf,
        /// Read `<since>..HEAD` only (a tag, sha or branch). Default: the whole history.
        #[arg(long)]
        since: Option<String>,
        /// Repository to read. Default: found from the analysed source paths.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// butterfly.toml for `[change]` ticket patterns and excludes. Default: the one recorded in the evidence when it lies inside the evidence's directory, --repo or the working directory (one recorded anywhere else is refused), else next to the evidence, else built-in.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Cache file. Default: butterfly-changes.json next to the evidence.
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Where to write the updated evidence. Default: overwrite the input.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Concurrent git processes. Default 0 = one per CPU.
        #[arg(long, default_value_t = 0)]
        jobs: usize,
        /// Resolutions sidecar, so reviews and justifications on changed functions are flagged stale. Default: resolutions.json next to the evidence, else in the current directory.
        #[arg(long)]
        resolutions: Option<PathBuf>,
        /// Exit with status 3 when obligations at or above this level exist: none | blocker | major | any.
        #[arg(long, default_value = "none", value_parser = parse_fail_on)]
        fail_on: String,
        /// Frozen baseline from the certified build, to find deleted functions. Default: the one the evidence was checked against, else butterfly-baseline.json next to it.
        #[arg(long)]
        baseline: Option<PathBuf>,
    },
    /// Gate the evidence: exit 3 when a threshold is breached or anything is
    /// worse than a prior evidence file. A passing run writes nothing to
    /// stdout and prints at most one line: a note on stderr when no
    /// resolutions sidecar was discovered, because --max-unresolved-gaps and
    /// --min-mcdc are judged against that sidecar and a gate that passed
    /// having read no reviews must not look like one that read some.
    /// --no-resolutions silences it, and a passing run is then silent
    Check {
        evidence: PathBuf,
        /// Sidecar of review decisions; justified gaps and conditions count
        /// as resolved. Default: discovered next to the evidence
        #[arg(long)]
        resolutions: Option<PathBuf>,
        /// Judge with no reviews at all, and do not go looking for a sidecar.
        /// Distinct from omitting --resolutions, which *discovers* one: a
        /// viewer serving no reviews needs to name a command that also has
        /// none, or the two disagree about whether the build passes (#75).
        /// Applies to the --no-regression prior as well, so both sides of the
        /// comparison are judged under the same (empty) set of reviews, and
        /// nothing about sidecars is printed for either (#213).
        #[arg(long, conflicts_with = "resolutions")]
        no_resolutions: bool,
        /// Ceiling on gaps with no recorded resolution
        #[arg(long)]
        max_unresolved_gaps: Option<usize>,
        /// Floor on line coverage, percent. Line coverage is a proxy for
        /// statement coverage, not proof of it -- hence the name
        #[arg(long, value_parser = parse_pct)]
        min_lines: Option<f64>,
        /// Floor on branch-outcome coverage, percent (what decision coverage
        /// is measured on)
        #[arg(long, value_parser = parse_pct)]
        min_outcomes: Option<f64>,
        /// Floor on MC/DC conditions proven or justified, percent
        #[arg(long, value_parser = parse_pct)]
        min_mcdc: Option<f64>,
        /// A prior evidence file nothing may be worse than. This is prior
        /// *evidence*, not butterfly-baseline.json: the baseline fingerprints
        /// the build and deliberately carries no coverage to regress against
        #[arg(long, value_name = "PRIOR_EVIDENCE")]
        no_regression: Option<PathBuf>,
        /// Machine-readable breaches on stdout
        #[arg(long)]
        json: bool,
    },
    /// Re-verification obligations from the change history already in the
    /// evidence (no git needed): what each ticket invalidates and what must
    /// be re-run. Exit status 3 when --fail-on is exceeded, for CI.
    Impact {
        evidence: PathBuf,
        /// Resolutions sidecar. Default: resolutions.json next to the evidence, else in the current directory.
        #[arg(long)]
        resolutions: Option<PathBuf>,
        /// Exit with status 3 when obligations at or above this level exist: none | blocker | major | any.
        #[arg(long, default_value = "none", value_parser = parse_fail_on)]
        fail_on: String,
        /// Print the impact record as JSON on stdout instead of the table.
        #[arg(long)]
        json: bool,
        /// Store the recomputed impact back into the evidence file.
        #[arg(long)]
        write: bool,
    },
}

/// Resolutions for an evidence file: explicit path, else next to the
/// evidence, else the working directory, else none.
/// Read the resolutions sidecar, failing closed.
///
/// A resolution is a reviewer's recorded decision about a coverage gap. A
/// command that treats an unreadable or malformed sidecar as "no reviews"
/// produces a verification report that silently omits them, exits zero, and
/// looks complete -- which is worse than producing nothing at all.
///
/// So: a path given explicitly must load. Every failure -- missing, no
/// permission, malformed, wrong schema -- is an error naming the file. Only
/// the *discovered* sidecar may be absent, because a project that has not
/// recorded any resolution yet has no file, and that is a normal first run.
/// Even then, a file that exists but cannot be read is an error.
fn load_resolutions(evidence: &std::path::Path, ev: &butterfly_model::Evidence, explicit: Option<PathBuf>) -> Result<(butterfly_model::Resolutions, Option<PathBuf>)> {
    let strict = |p: &std::path::Path| -> Result<butterfly_model::Resolutions> {
        // a sidecar is an auxiliary input like any other, and this is the
        // last unbounded whole-file read on the command path (review of #5)
        let lim = butterfly_model::limits::Limits::from_env();
        let t = butterfly_model::limits::read_bounded_string(p, butterfly_model::limits::Kind::Auxiliary, &lim).map_err(|e| anyhow::anyhow!("reading resolutions {e}"))?;
        let r: butterfly_model::Resolutions = serde_json::from_str(&t).map_err(|e| anyhow::anyhow!("{}: not a resolutions file — {e}", p.display()))?;
        // Schema, and every field of every entry. A hand-edited or scripted
        // sidecar with a bogus category or a blank justification must fail
        // here, at the door -- not read as "resolved" by every consumer
        // downstream that only checks key presence (review of #66).
        r.validate().map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
        // A key naming no current obligation is reported, not fatal. It
        // closes nothing (every consumer looks up the obligation's own key),
        // and it is the ordinary result of a rebuild moving the addresses
        // gap keys are built from -- so failing here would make a routine
        // code change brick `check`, `report` and `impact` alike.
        let stale = r.stale_keys(ev);
        if !stale.is_empty() {
            eprintln!(
                "warning: {}: {} review(s) name nothing in this evidence and close nothing — most likely stale after a rebuild: {}{}",
                p.display(),
                stale.len(),
                stale.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
                if stale.len() > 3 { ", …" } else { "" }
            );
        }
        Ok(r)
    };
    if let Some(p) = explicit {
        let r = strict(&p)?;
        return Ok((r, Some(p)));
    }
    for p in [evidence.parent().map(|d| d.join("resolutions.json")).unwrap_or_default(), PathBuf::from("resolutions.json")] {
        if p.as_os_str().is_empty() {
            continue;
        }
        // Ask about the directory entry, not what it leads to. `exists()`
        // follows symlinks and so answers "no" for a dangling one -- a
        // sidecar whose target was removed became "no reviews recorded"
        // and a complete-looking report (review of #19). Only `NotFound`
        // means absent; anything else that is there, or that cannot be
        // asked about, goes to the strict reader.
        match std::fs::symlink_metadata(&p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            _ => {
                let r = strict(&p)?;
                return Ok((r, Some(p)));
            }
        }
    }
    // Once per run, not once per load. `check --no-regression` reads a
    // sidecar twice -- for this evidence and for the prior -- and printed
    // the note for each, so a passing run emitted the same line twice while
    // the README promised one. The absence is a property of the run, not of
    // the file being read, so saying it twice tells the reader nothing new.
    static NOTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !NOTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!("note     : no resolutions sidecar found; the report records no reviews or justifications");
    }
    Ok((butterfly_model::Resolutions { schema: butterfly_model::RESOLUTIONS_SCHEMA.into(), entries: Vec::new() }, None))
}

/// The obligation table, for a human, on stdout.
fn print_impact(ev: &butterfly_model::Evidence, imp: &butterfly_model::Impact) {
    // a closed pipe (`| head`) is not an error worth a panic
    use std::io::Write;
    let mut so = std::io::stdout().lock();
    let c = ev.changes.as_ref().unwrap();
    let _ = writeln!(
        so,
        "change impact (12.1.1): {} function(s) changed{} · {} obligation(s): {}",
        imp.functions_changed.len(),
        if imp.bounded { format!(" since {}", c.since.as_deref().unwrap_or("")) } else { " (whole history: no --since)".into() },
        imp.obligations.len(),
        imp.by_severity.iter().map(|(k, v)| format!("{v} {k}")).collect::<Vec<_>>().join(", ")
    );
    if !imp.by_ticket.is_empty() {
        let _ = writeln!(so);
        let _ =
            writeln!(so, "{:<16} {:>8} {:>9} {:>11} {:>6} {:>9} {:>8} {:>6}  functions", "ticket", "commits", "blockers", "obligations", "added", "modified", "deleted", "files");
        for t in &imp.by_ticket {
            let tk = &c.tickets[t.ticket];
            let names: Vec<&str> = tk.functions.iter().filter_map(|&f| ev.functions.get(f)).map(|f| f.name.as_str()).collect();
            let _ = writeln!(
                so,
                "{:<16} {:>8} {:>9} {:>11} {:>6} {:>9} {:>8} {:>6}  {}",
                tk.id,
                tk.commits.len(),
                t.blockers,
                t.obligations.len(),
                tk.added.len(),
                tk.modified.len(),
                tk.deleted.len(),
                tk.files.len(),
                names.join(", ")
            );
        }
    }
    if !imp.obligations.is_empty() {
        let _ = writeln!(so);
        let _ = writeln!(so, "{:<9} {:<19} {:<30} {:<28} detail", "severity", "kind", "function", "target / key / requirement");
        for o in imp.obligations.iter().take(300) {
            let what = o
                .target_function
                .and_then(|t| ev.functions.get(t))
                .map(|f| f.name.clone())
                .or_else(|| o.key.clone())
                .or_else(|| o.requirement.clone())
                .or_else(|| o.file.clone())
                .unwrap_or_default();
            let name =
                o.function_id.and_then(|f| ev.functions.get(f)).map(|f| f.name.as_str()).or_else(|| o.ticket.and_then(|t| c.tickets.get(t)).map(|t| t.id.as_str())).unwrap_or("—");
            let _ =
                writeln!(so, "{:<9} {:<19} {:<30} {:<28} {}", o.severity, o.kind, name.chars().take(30).collect::<String>(), what.chars().take(28).collect::<String>(), o.detail);
        }
        if imp.obligations.len() > 300 {
            let _ = writeln!(so, "… {} more (use --json for all)", imp.obligations.len() - 300);
        }
    }
    for cv in &imp.caveats {
        let _ = writeln!(so, "note: {cv}");
    }
}

/// Who can reach the viewer, and with what. One definition shared by
/// `serve`, `build --serve` and `run`, so the three cannot drift apart on
/// something this important.
// No `Debug`: `token` holds a literal `--token` secret as typed (review of #65).
#[derive(Args)]
struct ServeFlags {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: String,
    /// Allow binding a non-loopback address. Anyone who can reach the port
    /// can read the source code the evidence embeds. Also needs
    /// --insecure-http; serves read-only unless a token is given.
    #[arg(long)]
    allow_remote: bool,
    /// Acknowledge that a non-loopback listener is plain HTTP: the evidence
    /// and any bearer token cross the network unencrypted. Required with
    /// --allow-remote. Prefer loopback plus `ssh -L`; behind a
    /// TLS-terminating reverse proxy on another host, this acknowledges the
    /// plain hop between the proxy and butterfly.
    #[arg(long)]
    insecure_http: bool,
    /// Refuse every resolution write, whatever the address or token.
    #[arg(long)]
    read_only: bool,
    /// Require `Authorization: Bearer <token>` on every API route, reading
    /// the token from this file (one line; keep it mode 600). This is how
    /// review decisions are recorded over the network. The token may come
    /// from the BUTTERFLY_TOKEN environment variable instead; giving both is
    /// an error.
    #[arg(long, value_name = "PATH")]
    token_file: Option<PathBuf>,
    /// `generate` mints a 256-bit token and prints the link to open. A
    /// literal secret still works but is DEPRECATED: it is visible in the
    /// process list and kept in shell history. Use --token-file or
    /// BUTTERFLY_TOKEN.
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,
    /// Serve a non-loopback listener with no token: reads open to anyone who
    /// can reach the port, writes refused. Without this a network listener
    /// needs a token, read-only included.
    #[arg(long)]
    allow_unauthenticated_remote: bool,
    /// The tree the viewer's freshness check may read sources from. The
    /// evidence names its sources, and a path outside this root is reported
    /// as not checked rather than read. Default: the working directory for
    /// `serve`, the project for `run`, the config's source root for `build`.
    #[arg(long, value_name = "DIR")]
    source_root: Option<PathBuf>,
    /// The repository git may be run in to check the change history. Any
    /// other repository the evidence names is not handed to git, whose own
    /// config can run programs. Default: the repository that contains the
    /// source root.
    #[arg(long, value_name = "DIR")]
    repo: Option<PathBuf>,
}

/// The viewer's exposure, from the flags and the environment, checked.
///
/// Which source a token came from travels with it: butterfly has to print a
/// token it minted, because nobody else knows it, and must not print one the
/// operator already has (review of #24). Deprecation and permission warnings
/// go to stderr here; none of them contains the token (#65).
///
/// Called before anything else a command does, so that `run` and `build
/// --serve` refuse a listener they would not start before the build, not
/// minutes after it.
fn serve_options(f: ServeFlags) -> Result<butterfly_view::ServeOptions> {
    use butterfly_view::access;
    let env = std::env::var_os(access::TOKEN_ENV);
    let r = access::resolve_token(access::TokenSources { literal: f.token.as_deref(), file: f.token_file.as_deref(), env: env.as_deref() }).map_err(anyhow::Error::msg)?;
    for w in &r.warnings {
        eprintln!("{w}");
    }
    let opts = butterfly_view::ServeOptions {
        listen: f.listen,
        allow_remote: f.allow_remote,
        insecure_http: f.insecure_http,
        read_only: f.read_only,
        token: r.token,
        allow_unauthenticated_remote: f.allow_unauthenticated_remote,
        source_root: f.source_root,
        repo: f.repo,
    };
    opts.check()?;
    Ok(opts)
}

/// A `--fail-on` level, checked where the arguments are parsed: a level
/// that does not exist is a usage mistake (exit 2, README "Gating a
/// pipeline"), not an error found while running (exit 1) (#325 C2).
fn parse_fail_on(v: &str) -> Result<String, String> {
    butterfly_ingest::impact::Level::parse(v).map(|_| v.to_string())
}

/// A coverage floor must be a percentage.
///
/// An unconstrained f64 accepted -1 (which passes anything), 101 (which
/// `meets` treats as 100), and NaN (which breaches every nonempty
/// measurement) -- three ways for a CI configuration typo to masquerade as
/// coverage policy (review of #43).
fn parse_pct(v: &str) -> Result<f64, String> {
    let x: f64 = v.parse().map_err(|_| format!("{v:?} is not a number"))?;
    if !x.is_finite() || !(0.0..=100.0).contains(&x) {
        return Err(format!("{v} is not a percentage between 0 and 100"));
    }
    Ok(x)
}

/// Expand `--tests` into one export per test.
///
/// A directory is the ordinary case -- a profile-per-test workflow leaves a
/// directory of exports -- so every `*.json` inside becomes a test, sorted so
/// the ids are stable between runs. Anything else is taken as a file.
fn expand_test_exports(given: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in given {
        if p.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(p)
                .with_context(|| format!("reading {}", p.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect();
            if found.is_empty() {
                anyhow::bail!("--tests {}: no *.json coverage exports in that directory", p.display());
            }
            found.sort();
            out.extend(found);
        } else {
            out.push(p.clone());
        }
    }
    Ok(out)
}

/// Read an evidence file under the resource budget, so the most common CLI
/// entry points cannot be made to allocate a hostile file's full size before
/// it is parsed or validated. One budget per command, which for these
/// single-evidence commands is one evidence load (review of #5).
fn read_evidence_text(path: &std::path::Path) -> Result<String> {
    let lim = butterfly_model::limits::Limits::from_env();
    butterfly_model::limits::read_bounded_string(path, butterfly_model::limits::Kind::Evidence, &lim).map_err(|e| anyhow::anyhow!("{e}"))
}

/// The config an evidence file says it was built with, for `changes`
/// without `--config` -- read only if it is where the operator's own files
/// are (#342).
///
/// The path is the evidence's word, and the evidence is input like any other:
/// honoured blindly, it made `changes` parse any file the operator can read,
/// and a parse error printed that file's first line. So it is read only from
/// inside the evidence's directory or the project -- `--repo` when given, and
/// the working directory -- and placed the way `ingest` places a source: the
/// name is opened once and the kernel asked where that descriptor is, so a
/// link inside leading out is outside, and a file with other names cannot be
/// placed at all. A path that is outside by its spelling is refused before
/// anything is opened, so whether it exists is not given away either.
///
/// `Ok(None)` when the named file is inside but not there (an evidence moved
/// from the machine that built it): the caller falls back as it always did.
///
/// The same holds for the `butterfly.toml` found beside the evidence when the
/// evidence names no config, or names one that is not there (`discovered`):
/// it sits in the evidence's directory, which is no more the operator's than
/// the evidence is, so a link there leading out is refused as well rather than
/// followed (#346 review). Either way, a parse error carries the file and the
/// position but nothing the file says.
/// Open a path named by an evidence file (or discovered beside it) only if it
/// sits inside the evidence's directory or the project (`--repo`, or the
/// working directory) as a plain, single-link file (#342, #346).
///
/// The spelling is checked before anything is opened (so existence of outside
/// paths is not leaked). After opening, `placed.real` is checked against the
/// canonical roots and hardlinks (`nlink > 1`) are refused unconditionally on
/// the descriptor's metadata -- never via `Standing::Linked` from a single
/// root, which was skipped whenever `placed.real` landed under a canonical
/// alias (such as `/private/tmp` vs `/tmp`) or another allowed root. A
/// dangling symlink inside the roots fails closed rather than reading as
/// absent.
fn open_evidence_placed(
    recorded: &std::path::Path,
    ev_dir: &std::path::Path,
    repo: Option<&std::path::Path>,
    refuse: impl Fn(&str) -> anyhow::Error,
) -> Result<Option<std::fs::File>> {
    use butterfly_ingest::fsauth::open_placed;
    use std::os::unix::fs::MetadataExt;
    let cwd = std::env::current_dir().context("determining the working directory")?;
    let absolute = |p: &std::path::Path| if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
    // Spelled roots gate the path before anything is opened (the recorded path
    // may carry either form, e.g. macOS's /tmp vs /private/tmp); canonical
    // roots gate the kernel's `placed.real`.
    let mut spelled_roots = Vec::new();
    let mut canonical_roots = Vec::new();
    for r in [Some(ev_dir), Some(cwd.as_path()), repo].into_iter().flatten() {
        let r = lexical(&absolute(r));
        if let Ok(c) = r.canonicalize() {
            canonical_roots.push(c.clone());
            spelled_roots.push(c);
        }
        spelled_roots.push(r);
    }
    let spelled = lexical(&absolute(recorded));
    if !spelled_roots.iter().any(|r| spelled.starts_with(r)) {
        return Err(refuse("is outside both"));
    }
    let probe_root = canonical_roots.first().cloned().unwrap_or_else(|| spelled_roots[0].clone());
    let placed = match open_placed(&probe_root, &spelled) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if std::fs::symlink_metadata(&spelled).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(refuse("is a broken symlink"));
            }
            return Ok(None);
        }
        Err(e) => anyhow::bail!("{}: {e}", recorded.display()),
    };
    let linked = placed.meta.is_file() && placed.meta.nlink() > 1;
    let inside = !linked && canonical_roots.iter().any(|r| placed.real.starts_with(r));
    if !inside {
        return Err(refuse("is a link, resolves outside both, or has other names that cannot be placed"));
    }
    Ok(Some(placed.file))
}

fn evidence_named_config(
    evidence: &std::path::Path,
    recorded: &std::path::Path,
    ev_dir: &std::path::Path,
    repo: Option<&std::path::Path>,
    discovered: bool,
) -> Result<Option<(PathBuf, butterfly_ingest::config::Config)>> {
    let path = recorded.to_path_buf();
    let refuse = |why: &str| {
        if discovered {
            anyhow::anyhow!(
                "{} is beside {} as its config, but it {why}. butterfly reads a config it finds beside an evidence file only as a plain file inside the evidence's directory or the project (--repo, or the working directory), since that directory is no more yours than the evidence is (#342). Pass --config to use it, or another config.",
                path.display(),
                evidence.display()
            )
        } else {
            anyhow::anyhow!(
                "{} names {} as the config it was built with, which {why}. butterfly reads a config an evidence file names only from the evidence's directory or the project (--repo, or the working directory), since the path is the evidence's word, not yours (#342). Pass --config to use it, or another config.",
                evidence.display(),
                path.display()
            )
        }
    };
    let Some(file) = open_evidence_placed(&path, ev_dir, repo, refuse)? else { return Ok(None) };
    let lim = butterfly_model::limits::Limits::from_env();
    let text = butterfly_model::limits::read_bounded_string_from(file, &path, butterfly_model::limits::Kind::Auxiliary, &lim).map_err(|e| anyhow::anyhow!("{e}"))?;
    let cfg = butterfly_ingest::config::Config::parse_untrusted(&text, &path)?;
    Ok(Some((path, cfg)))
}

/// The frozen baseline an evidence file names (`ev.baseline.path`), or
/// `butterfly-baseline.json` discovered beside the evidence, for `changes`
/// without `--baseline`: held to the same placement and symlink/hardlink rules
/// as `evidence_named_config` so an untrusted evidence file cannot read
/// arbitrary host files or follow an escaping symlink beside it.
fn evidence_named_baseline(
    evidence: &std::path::Path,
    recorded: &std::path::Path,
    ev_dir: &std::path::Path,
    repo: Option<&std::path::Path>,
    discovered: bool,
) -> Result<Option<(PathBuf, butterfly_model::Baseline)>> {
    let path = recorded.to_path_buf();
    let refuse = |why: &str| {
        if discovered {
            anyhow::anyhow!(
                "{} is beside {} as its baseline, but it {why}. butterfly reads a baseline it finds beside an evidence file only as a plain file inside the evidence's directory or the project (--repo, or the working directory), since that directory is no more yours than the evidence is. Pass --baseline to use it, or another baseline.",
                path.display(),
                evidence.display()
            )
        } else {
            anyhow::anyhow!(
                "{} names {} as the baseline it was checked against, which {why}. butterfly reads a baseline an evidence file names only from the evidence's directory or the project (--repo, or the working directory), since the path is the evidence's word, not yours. Pass --baseline to use it, or another baseline.",
                evidence.display(),
                path.display()
            )
        }
    };
    let Some(file) = open_evidence_placed(&path, ev_dir, repo, refuse)? else { return Ok(None) };
    let lim = butterfly_model::limits::Limits::from_env();
    let text =
        butterfly_model::limits::read_bounded_string_from(file, &path, butterfly_model::limits::Kind::Auxiliary, &lim).map_err(|e| anyhow::anyhow!("reading baseline {e}"))?;
    let b: butterfly_model::Baseline =
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("{} is not a butterfly baseline (line {}, column {})", path.display(), e.line(), e.column()))?;
    b.check_schema().map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(Some((path, b)))
}

/// `p` with `.` and `..` folded away by spelling alone, touching nothing.
fn lexical(p: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// The same for a sidecar an operator names on the command line -- a
/// requirements document, a baseline, a resolutions file. `Kind::Auxiliary`
/// is what every other sidecar read uses (`ingest`'s `reqtext`, the viewer's
/// `load_requirements`), so a file refused here is refused at the same size
/// wherever it is read (#231).
fn read_sidecar_text(path: &std::path::Path) -> Result<String> {
    let lim = butterfly_model::limits::Limits::from_env();
    butterfly_model::limits::read_bounded_string(path, butterfly_model::limits::Kind::Auxiliary, &lim).map_err(|e| anyhow::anyhow!("{e}"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Ingest {
            dir,
            coverage,
            objects,
            build_info,
            source_root,
            allow_external_source,
            llvm_bin,
            trace,
            ozone_report,
            stack_usage,
            requirements,
            tests,
            baseline,
            config,
            out,
        } => {
            let mut opts = match dir {
                Some(d) => butterfly_ingest::discover(&d)?,
                None => butterfly_ingest::IngestOptions::default(),
            };
            if let Some(c) = coverage {
                opts.coverage_json = c;
            }
            if !objects.is_empty() {
                opts.objects = objects;
            }
            if build_info.is_some() {
                opts.build_info = build_info;
            }
            if source_root.is_some() {
                opts.source_root = source_root;
            }
            opts.allow_external_source = allow_external_source;
            opts.llvm_bin = llvm_bin;
            opts.ozone_report = ozone_report;
            if trace.is_some() {
                opts.trace = trace;
            }
            if !stack_usage.is_empty() {
                opts.stack_usage = stack_usage;
            }
            if !tests.is_empty() {
                opts.tests = expand_test_exports(&tests)?;
            }
            if requirements.is_some() {
                opts.requirements_list = requirements;
            }
            if baseline.is_some() {
                opts.baseline = baseline;
            }
            if config.is_some() {
                opts.config = config;
            }
            if opts.coverage_json.as_os_str().is_empty() {
                anyhow::bail!("no coverage export given: pass a build directory or --coverage");
            }
            eprintln!("coverage : {}", opts.coverage_json.display());
            for o in &opts.objects {
                eprintln!("object   : {}", o.display());
            }
            if let Some(t) = &opts.trace {
                eprintln!("trace    : {}", t.display());
            }
            for su in &opts.stack_usage {
                eprintln!("stack    : {}", su.display());
            }
            if let Some(o) = &opts.ozone_report {
                eprintln!("ozone    : {}", o.display());
            }
            let ev = butterfly_ingest::ingest(&opts)?;
            let json = serde_json::to_string(&ev)?;
            write_output(&out, json.as_bytes())?;
            print_summary(&ev);
            eprintln!("wrote {}", out.display());
            Ok(())
        }
        Cmd::Build { config, serve, net } => {
            let opts = if serve { Some(serve_options(net)?) } else { None };
            let (results, source_root) = builder::build(&config)?;
            eprintln!("built {} variant(s)", results.len());
            if let Some(mut opts) = opts {
                opts.source_root.get_or_insert(source_root);
                let paths: Vec<PathBuf> = results.iter().map(|(_, p)| p.clone()).collect();
                let Some(first) = paths.first() else { anyhow::bail!("no variants were built") };
                let res = first.parent().unwrap_or(std::path::Path::new(".")).join("resolutions.json");
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(butterfly_view::serve(paths, res, opts))
            } else {
                for (n, p) in &results {
                    eprintln!("  {n}: {}", p.display());
                }
                eprintln!("serve with: butterfly serve {}", results.iter().map(|(_, p)| p.display().to_string()).collect::<Vec<_>>().join(" "));
                Ok(())
            }
        }
        Cmd::Serve { evidence, resolutions, net } => {
            let opts = serve_options(net)?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(butterfly_view::serve(evidence, resolutions, opts))
        }
        Cmd::Report { evidence, resolutions, requirements, out } => {
            let text = read_evidence_text(&evidence)?;
            let ev: butterfly_model::Evidence = serde_json::from_str(&text)?;
            ev.validate().map_err(|e| anyhow::anyhow!("{}: inconsistent evidence — {e}", evidence.display()))?;
            ev.check_summary_derivation().map_err(|e| anyhow::anyhow!("{}: {e}", evidence.display()))?;
            let (res, from) = load_resolutions(&evidence, &ev, resolutions)?;
            let reqs = load_report_requirements(&evidence, requirements)?;
            let html = butterfly_view::report::render(&ev, &res, reqs.as_ref(), None);
            if let Some(q) = &reqs {
                let missing = ev
                    .functions
                    .iter()
                    .flat_map(|f| f.requirements.iter().chain(f.line_requirements.values().flatten()))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .filter(|id| q.get(id).is_none())
                    .count();
                eprintln!("requirements: {} statement(s) from {} document(s)", q.entries.len(), q.sources.len());
                if missing > 0 {
                    // Said here rather than left as a blank row in the report
                    eprintln!("              {missing} tag(s) in the source have no statement; the report names them without one");
                }
            }
            if let Some(p) = &from {
                eprintln!("resolutions: {} ({} entr{})", p.display(), res.entries.len(), if res.entries.len() == 1 { "y" } else { "ies" });
            }
            write_output(&out, html.as_bytes())?;
            eprintln!("wrote {}", out.display());
            Ok(())
        }
        Cmd::Baseline { evidence, out } => {
            let text = read_evidence_text(&evidence)?;
            let ev: butterfly_model::Evidence = serde_json::from_str(&text)?;
            ev.validate().map_err(|e| anyhow::anyhow!("{}: inconsistent evidence — {e}", evidence.display()))?;
            ev.check_summary_derivation().map_err(|e| anyhow::anyhow!("{}: {e}", evidence.display()))?;
            let b = butterfly_ingest::baseline::from_evidence(&ev);
            write_output(&out, serde_json::to_string_pretty(&b)?.as_bytes())?;
            eprintln!("baseline: {} functions, {} constructs, compiler `{}`, flags `{}`", b.functions.len(), b.constructs.len(), b.compiler, b.flags_target);
            eprintln!("wrote {} — keep it under configuration management next to the evidence", out.display());
            Ok(())
        }
        Cmd::Changes { evidence, since, repo, config, cache, out, jobs, resolutions, fail_on, baseline } => {
            let level = butterfly_ingest::impact::Level::parse(&fail_on).map_err(|e| anyhow::anyhow!("--fail-on: {e}"))?;
            let text = read_evidence_text(&evidence)?;
            let mut ev: butterfly_model::Evidence = serde_json::from_str(&text)?;
            ev.validate().map_err(|e| anyhow::anyhow!("{}: inconsistent evidence — {e}", evidence.display()))?;
            ev.check_summary_derivation().map_err(|e| anyhow::anyhow!("{}: {e}", evidence.display()))?;
            let dir = evidence.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
            // config: explicit, else the one the evidence was built with --
            // if it is where the operator's own files are (#342) -- else next
            // to the evidence
            let named = match config {
                Some(p) => {
                    let c = butterfly_ingest::config::Config::load(&p)?;
                    Some((p, c))
                }
                None => match ev.provenance.inputs.iter().find(|i| i.role == "config") {
                    Some(i) => evidence_named_config(&evidence, std::path::Path::new(&i.path), &dir, repo.as_deref(), false)?,
                    None => None,
                },
            };
            // else the one beside the evidence, placed the same way: a link
            // there leading out is refused, not followed (#346 review)
            let named = match named {
                Some(n) => Some(n),
                None => evidence_named_config(&evidence, &dir.join("butterfly.toml"), &dir, repo.as_deref(), true)?,
            };
            let (cfg_path, cfg) = match named {
                Some((p, c)) => (Some(p), c),
                None => (None, butterfly_ingest::config::Config::default()),
            };
            let cache_path = cache.unwrap_or_else(|| dir.join("butterfly-changes.json"));
            let prior = if cache_path.exists() {
                match butterfly_ingest::change::load(&cache_path) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        eprintln!("warning  : cache {} ignored and will be rebuilt — {e}", cache_path.display());
                        None
                    }
                }
            } else {
                None
            };
            let loaded_baseline: Option<(PathBuf, butterfly_model::Baseline)> = match baseline {
                Some(p) => {
                    let t = read_sidecar_text(&p)?;
                    let b: butterfly_model::Baseline = serde_json::from_str(&t).with_context(|| format!("{} is not a butterfly baseline", p.display()))?;
                    b.check_schema().map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
                    Some((p, b))
                }
                None => {
                    let named_base = match &ev.baseline {
                        Some(b) if !b.path.is_empty() => evidence_named_baseline(&evidence, std::path::Path::new(&b.path), &dir, repo.as_deref(), false)?,
                        _ => None,
                    };
                    match named_base {
                        Some(n) => Some(n),
                        None => evidence_named_baseline(&evidence, &dir.join("butterfly-baseline.json"), &dir, repo.as_deref(), true)?,
                    }
                }
            };
            let baseline_path = loaded_baseline.as_ref().map(|(p, _)| p.clone());
            let opts = butterfly_ingest::change::Options { since, repo, patterns: cfg.change.patterns.clone(), exclude: cfg.change.exclude.clone(), jobs, baseline: None };
            let (new_cache, outcome) = butterfly_ingest::change::analyse_with_baseline(&mut ev, &opts, prior.as_ref(), loaded_baseline.as_ref().map(|(_, b)| b))?;
            if let butterfly_ingest::fsauth::Wrote::ReplacedSymlink(target) = butterfly_ingest::change::save(&new_cache, &cache_path)? {
                replaced_warning(&cache_path, &target);
            }
            let (res, res_path) = load_resolutions(&evidence, &ev, resolutions)?;
            let imp = butterfly_ingest::impact::compute(&ev, &res).map_err(|e| anyhow::anyhow!(e))?;
            ev.changes.as_mut().unwrap().impact = Some(imp.clone());
            ev.validate().map_err(|e| anyhow::anyhow!("internal: change analysis produced inconsistent evidence — {e}"))?;
            let out_path = out.unwrap_or(evidence);
            write_output(&out_path, serde_json::to_string_pretty(&ev)?.as_bytes())?;
            let c = ev.changes.as_ref().unwrap();
            eprintln!("repo     : {} @ {}{}", c.repo, &c.head[..12.min(c.head.len())], c.since.as_deref().map(|s| format!("  (since {s})")).unwrap_or_default());
            eprintln!("config   : {}", cfg_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "built-in patterns".into()));
            eprintln!("walked   : {} function(s) · {} reused from cache · {} skipped", outcome.walked, outcome.reused, outcome.skipped);
            eprintln!(
                "baseline : {}",
                baseline_path
                    .as_ref()
                    .map(|p| format!("{} · {} deleted function(s) located", p.display(), outcome.deleted))
                    .unwrap_or_else(|| "none (deleted functions cannot be found without one)".into())
            );
            let touched = ev.functions.iter().filter(|f| !f.commits.is_empty()).count();
            eprintln!("history  : {} commit(s) touched {} function(s) · {} ticket(s) · {} unticketed commit(s)", c.commits.len(), touched, c.tickets.len(), c.unticketed.len());
            for t in c.tickets.iter().take(12) {
                eprintln!(
                    "ticket   : {:<14} {} commit(s) · {} added · {} modified · {} deleted · {} file(s){}",
                    t.id,
                    t.commits.len(),
                    t.added.len(),
                    t.modified.len(),
                    t.deleted.len(),
                    t.files.len(),
                    {
                        let u = t.files.iter().filter(|f| f.functions == 0).count();
                        if u > 0 {
                            format!(" ({u} with no function in the evidence)")
                        } else {
                            String::new()
                        }
                    }
                );
            }
            if c.tickets.len() > 12 {
                eprintln!("ticket   : … {} more", c.tickets.len() - 12);
            }
            for cv in c.caveats.iter().filter(|c| c.contains("uncommitted") || c.contains("different checkout") || c.contains("could not be walked")) {
                eprintln!("caveat   : {cv}");
            }
            eprintln!(
                "impact   : {} obligation(s) — {}{}",
                imp.obligations.len(),
                imp.by_severity.iter().map(|(k, v)| format!("{v} {k}")).collect::<Vec<_>>().join(", "),
                res_path.map(|p| format!(" · resolutions {}", p.display())).unwrap_or_else(|| " · no resolutions file".into())
            );
            eprintln!("wrote {} and cache {}", out_path.display(), cache_path.display());
            if butterfly_ingest::impact::fails(&imp, level) {
                eprintln!("FAIL     : obligations at or above `{fail_on}` exist (run `butterfly impact {}` for the list)", out_path.display());
                std::process::exit(3);
            }
            Ok(())
        }
        Cmd::Check { evidence, resolutions, no_resolutions, max_unresolved_gaps, min_lines, min_outcomes, min_mcdc, no_regression, json } => {
            let text = read_evidence_text(&evidence)?;
            let ev: butterfly_model::Evidence = serde_json::from_str(&text).with_context(|| format!("{} is not a butterfly evidence file", evidence.display()))?;
            ev.validate().map_err(|e| anyhow::anyhow!("{}: inconsistent evidence — {e}", evidence.display()))?;
            ev.check_summary_derivation().map_err(|e| anyhow::anyhow!("{}: {e}", evidence.display()))?;
            let (res, _) = if no_resolutions { (butterfly_model::Resolutions::default(), None) } else { load_resolutions(&evidence, &ev, resolutions)? };

            let th = butterfly_ingest::gate::Thresholds { max_unresolved_gaps, min_lines, min_outcomes, min_mcdc };
            // Nothing asked, nothing judged: a quiet exit 0 would read as a
            // gate that held. One line says so, in the viewer's words; the
            // exit status and stdout are what a pass gives (#325).
            if max_unresolved_gaps.is_none() && min_lines.is_none() && min_outcomes.is_none() && min_mcdc.is_none() && no_regression.is_none() {
                eprintln!("note     : no thresholds given; nothing is gated");
            }
            let mut breaches = butterfly_ingest::gate::evaluate(&ev, &res, &th);
            if let Some(pp) = &no_regression {
                let ptext = read_evidence_text(pp)?;
                let prior: butterfly_model::Evidence = serde_json::from_str(&ptext).with_context(|| format!("{} is not a butterfly evidence file", pp.display()))?;
                prior.validate().map_err(|e| anyhow::anyhow!("{}: inconsistent evidence — {e}", pp.display()))?;
                // gate::regressions() compares against prior.summary, so the
                // prior's summary must be bound to its own records too, or an
                // inconsistent baseline could drive the regression verdict (#249).
                prior.check_summary_derivation().map_err(|e| anyhow::anyhow!("{}: {e}", pp.display()))?;
                // The prior's own decisions, discovered next to it, so a gap
                // justified back then still counts as resolved back then --
                // unless --no-resolutions, which means judge with no reviews
                // at all. Loading the prior's sidecar under that flag made
                // the comparison asymmetric: the unresolved-gap and MC/DC
                // counts were taken with no reviews on this side and with
                // every review on the other, which reports a regression that
                // is an artefact of the flag rather than of the build, and
                // went looking for a sidecar the operator said not to look
                // for (#213).
                let (prior_res, _) = if no_resolutions { (butterfly_model::Resolutions::default(), None) } else { load_resolutions(pp, &prior, None)? };
                breaches.extend(butterfly_ingest::gate::regressions(&ev, &res, &prior, &prior_res));
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "passed": breaches.is_empty(), "breaches": breaches }))?);
            } else {
                for b in &breaches {
                    eprintln!("BREACH  {:<18} {}", b.kind, b.message);
                }
            }
            if !breaches.is_empty() {
                if !json {
                    eprintln!("FAIL     {} breach(es) in {}", breaches.len(), evidence.display());
                }
                std::process::exit(3);
            }
            Ok(())
        }
        Cmd::Impact { evidence, resolutions, fail_on, json, write } => {
            let level = butterfly_ingest::impact::Level::parse(&fail_on).map_err(|e| anyhow::anyhow!("--fail-on: {e}"))?;
            let text = read_evidence_text(&evidence)?;
            let mut ev: butterfly_model::Evidence = serde_json::from_str(&text)?;
            ev.validate().map_err(|e| anyhow::anyhow!("{}: inconsistent evidence — {e}", evidence.display()))?;
            ev.check_summary_derivation().map_err(|e| anyhow::anyhow!("{}: {e}", evidence.display()))?;
            let (res, _) = load_resolutions(&evidence, &ev, resolutions)?;
            let imp = butterfly_ingest::impact::compute(&ev, &res).map_err(|e| anyhow::anyhow!("{}: {e}", evidence.display()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&imp)?);
            } else {
                print_impact(&ev, &imp);
            }
            if write {
                ev.changes.as_mut().unwrap().impact = Some(imp.clone());
                ev.validate().map_err(|e| anyhow::anyhow!("internal: impact produced inconsistent evidence — {e}"))?;
                write_output(&evidence, serde_json::to_string_pretty(&ev)?.as_bytes())?;
                eprintln!("wrote {}", evidence.display());
            }
            if butterfly_ingest::impact::fails(&imp, level) {
                eprintln!("FAIL: obligations at or above `{fail_on}` exist");
                std::process::exit(3);
            }
            Ok(())
        }
        Cmd::Ozone { report } => {
            let (report, _) = butterfly_ingest::ozone::read(&report, &butterfly_model::limits::Limits::from_env())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
            println!();
            Ok(())
        }
        Cmd::Summary { evidence } => {
            let text = read_evidence_text(&evidence)?;
            let ev: butterfly_model::Evidence = serde_json::from_str(&text)?;
            ev.validate().map_err(|e| anyhow::anyhow!("{}: inconsistent evidence — {e}", evidence.display()))?;
            ev.check_summary_derivation().map_err(|e| anyhow::anyhow!("{}: {e}", evidence.display()))?;
            // the sidecar, when there is one: a justified item is off the
            // worklist, and summary should agree with the viewer about that
            let (res, _) = load_resolutions(&evidence, &ev, None)?;
            print_summary_with(&ev, &res);
            Ok(())
        }
        Cmd::Run { dir, build_script, no_build, out_dir, no_serve, force, verbose, requirements, config, net } => {
            // before the build: a listener or token that would be refused
            // should be refused now, not after the build has run
            let serve_opts = if no_serve { None } else { Some(serve_options(net)?) };
            let total = if no_serve { 4 } else { 5 };
            let build_dir = dir.join(&out_dir);
            let mut rep = run::Report { phases: Vec::new() };
            // This run's outputs are this run's or absent. A build that fails
            // -- the case this command stops for as "nothing downstream would
            // be trustworthy" -- otherwise left the previous run's evidence
            // beside the new binaries, for `check` to accept (#287).
            run::clear_outputs(&build_dir)?;

            // 1. build
            if no_build {
                eprintln!("── [1/{total}] build ── skipped (--no-build)");
                rep.phases.push(run::Phase {
                    name: "build",
                    ok: true,
                    note: "skipped; using what was already there".into(),
                    warnings: vec!["the evidence describes an earlier build, not this run".into()],
                });
            } else {
                run::heading(1, total, "build");
                let script = dir.join(&build_script);
                if let Err(e) = run::build(&dir, &script, &mut rep) {
                    eprint!("{}", rep.render());
                    return Err(e);
                }
            }

            // 2. doctor. Its whole purpose is to stop evidence that would be
            // wrong from being produced at all, so a Wrong finding halts the
            // run unless the operator overrides it deliberately.
            run::heading(2, total, "doctor");
            match std::fs::symlink_metadata(&build_dir) {
                Ok(m) if m.file_type().is_symlink() => {
                    anyhow::bail!("{} is a symlink; refusing to ingest or write outputs through a symlinked --out-dir", build_dir.display());
                }
                Ok(m) if m.is_dir() => {}
                _ => {
                    // Reached mainly via --no-build. A bare ENOENT here would be
                    // the same unhelpful failure this command exists to replace.
                    rep.add_phase("doctor", false, format!("{} does not exist", build_dir.display()));
                    eprint!("{}", rep.render());
                    eprintln!();
                    eprintln!("There is no build directory at {}.", build_dir.display());
                    if no_build {
                        eprintln!("--no-build says to use an existing build, but there is none.");
                        eprintln!("Drop --no-build to run {}, or pass --out-dir if it is elsewhere.", build_script.display());
                    } else {
                        eprintln!("The build script ran but produced no {}. Pass --out-dir if it writes elsewhere.", out_dir.display());
                    }
                    std::process::exit(2);
                }
            }
            let findings = run::doctor(&build_dir, verbose, &mut rep)?;
            let wrong = findings.iter().filter(|f| f.level == butterfly_ingest::preflight::Level::Wrong).count();
            if wrong > 0 && !force {
                eprint!("{}", rep.render());
                eprintln!();
                eprintln!("Stopping: {wrong} finding(s) would make the evidence wrong.");
                eprintln!("Fix them, or pass --force to build evidence you already know is wrong.");
                std::process::exit(3);
            }

            // 3. ingest
            run::heading(3, total, "ingest");
            let mut opts = butterfly_ingest::discover(&build_dir)?;
            opts.source_root = Some(dir.clone());
            if config.is_some() {
                opts.config = config;
            } else if dir.join("butterfly.toml").exists() {
                opts.config = Some(dir.join("butterfly.toml"));
            }
            let ev = butterfly_ingest::ingest(&opts)?;
            let evidence_path = build_dir.join("evidence.json");
            write_output(&evidence_path, serde_json::to_string(&ev)?.as_bytes())?;
            eprintln!("wrote {}", evidence_path.display());
            let partial = ev.functions.iter().filter(|f| f.lines_covered < f.lines_total).count();
            // a function with no coverage record was not counted, which is
            // not the same as not entered (#273)
            let unreached = ev.functions.iter().filter(|f| f.count == 0 && f.unrecorded_within.is_none()).count();
            let p = rep.add_phase("ingest", true, format!("{} function(s); {partial} not fully covered", ev.functions.len()));
            if unreached > 0 {
                p.warnings.push(format!("{unreached} function(s) were never entered by any test"));
            }

            // 4. report
            run::heading(4, total, "report");
            // One project-bound sidecar for the whole run: `<dir>/resolutions.json`,
            // the same file the serve phase opens. Discovery is deliberately
            // not consulted -- it also searches the *caller's* working
            // directory, and `butterfly run /path/to/project` from somewhere
            // else must neither drop the project's recorded reviews nor pick
            // up an unrelated file that happens to sit where the command was
            // typed (review of #75). Absent means a first run and no reviews;
            // present -- a dangling symlink included -- must load or the run
            // fails closed.
            let sidecar = dir.join("resolutions.json");
            let (res, from) = match std::fs::symlink_metadata(&sidecar) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (butterfly_model::Resolutions::default(), None),
                _ => load_resolutions(&evidence_path, &ev, Some(sidecar))?,
            };
            let reqs = load_run_requirements(&evidence_path, requirements, opts.config.as_deref())?;
            let html = butterfly_view::report::render(&ev, &res, reqs.as_ref(), None);
            let report_path = build_dir.join("report.html");
            write_output(&report_path, html.as_bytes())?;
            eprintln!("wrote {}", report_path.display());
            let p = rep.add_phase("report", true, format!("{}", report_path.display()));
            match &from {
                Some(r) => eprintln!("resolutions: {}", r.display()),
                None => p.warnings.push("no resolutions sidecar yet, so no reviews are recorded".into()),
            }

            // 5. serve
            let Some(mut serve_opts) = serve_opts else {
                eprint!("{}", rep.render());
                eprintln!();
                eprintln!("serve it with: butterfly serve {}", evidence_path.display());
                return Ok(());
            };
            run::heading(5, total, "serve");
            rep.add_phase("serve", true, format!("http://{}/", serve_opts.listen));
            eprint!("{}", rep.render());
            let rt = tokio::runtime::Runtime::new()?;
            // Beside the project, not inside the output directory: build.sh
            // does `rm -rf build`, and a reviewer's justifications must not be
            // deleted by the next build (this trap is called out in STEPS.md).
            let res_path = dir.join("resolutions.json");
            // the project is the tree this run ingested, so it is the one the
            // viewer's freshness check may read (#248)
            serve_opts.source_root.get_or_insert(dir.clone());
            rt.block_on(butterfly_view::serve(vec![evidence_path], res_path, serve_opts))
        }
        Cmd::Doctor { dir, verbose } => {
            let (opts, cov) = butterfly_ingest::discover_artifacts(&dir)?;
            // Read only the head of the export: enough to tell which producer
            // wrote it and whether it carries branch records, without pulling
            // a large file into memory to answer a yes/no question.
            // Parse it, rather than pattern-match its text: gcov writes
            // `"branches": []` when it was not asked for branches, so the key
            // is there either way and only the contents distinguish them.
            // A read or parse failure is *reported*, never folded into "no
            // coverage features": an export that ingest will refuse must not
            // let doctor pass, or the advice expires the moment it is acted
            // on (review of #45).
            let cov_state = cov.as_ref().map(|p| {
                use butterfly_ingest::preflight::{Coverage, CoverageState};
                let lim = butterfly_model::limits::Limits::from_env();
                let text = match butterfly_model::limits::read_bounded_string(p, butterfly_model::limits::Kind::Coverage, &lim) {
                    Ok(t) => t,
                    Err(e) => return Coverage::Unreadable(e),
                };
                let is_gcov = butterfly_ingest::gcov::looks_like(&text);
                let parsed = if is_gcov { butterfly_ingest::gcov::parse(&text) } else { butterfly_ingest::coverage::parse(&text) };
                match parsed {
                    Err(e) => Coverage::Unreadable(e.to_string()),
                    Ok(ev) => Coverage::Read(CoverageState {
                        is_gcov,
                        has_branches: ev.functions.iter().any(|f| !f.branches.is_empty()),
                        // From the reader, not from the parsed branches: an
                        // export can hold records that describe no two-way
                        // decision, and that is a different diagnosis.
                        has_raw_branches: ev.raw_branch_records,
                        has_mcdc: ev.functions.iter().any(|f| !f.mcdc.is_empty()),
                        // Did the producer record a decision at all? gcov
                        // writes a `conditions` entry per decision even when
                        // `-b` was not passed, so this distinguishes "the
                        // flag is missing" from "this code branches nowhere".
                        has_decisions: ev.decision_sites,
                    }),
                }
            });
            let findings = butterfly_ingest::preflight::check(&dir, &opts.objects, &opts.instrumented_skipped, cov.as_deref(), cov_state, &opts.stack_usage);

            println!("directory : {}", dir.display());
            if let Some(c) = &cov {
                println!("coverage  : {}", c.display());
            }
            for o in &opts.objects {
                println!("object    : {}", o.display());
            }
            println!("stack     : {} .su file(s)", opts.stack_usage.len());
            println!();
            print!("{}", butterfly_ingest::preflight::render(&findings, verbose));

            let wrong = findings.iter().filter(|f| f.level == butterfly_ingest::preflight::Level::Wrong).count();
            let weaker = findings.iter().filter(|f| f.level == butterfly_ingest::preflight::Level::Weaker).count();
            if wrong + weaker > 0 {
                println!();
                println!("{wrong} would make the evidence wrong, {weaker} would make it weaker than it looks.");
            }
            if wrong > 0 {
                std::process::exit(3);
            }
            Ok(())
        }
        Cmd::Claims { evidence, requirements, json } => {
            let text = read_evidence_text(&evidence)?;
            let ev: butterfly_model::Evidence = serde_json::from_str(&text)?;
            ev.validate().map_err(|e| anyhow::anyhow!("{}: inconsistent evidence — {e}", evidence.display()))?;
            ev.check_summary_derivation().map_err(|e| anyhow::anyhow!("{}: {e}", evidence.display()))?;
            let rtext = read_sidecar_text(&requirements)?;
            let reqs: butterfly_model::reqtext::Requirements = serde_json::from_str(&rtext)?;
            reqs.validate().map_err(|e| anyhow::anyhow!("{}: inconsistent requirements — {e}", requirements.display()))?;

            let rep = butterfly_model::claimcheck::compare(&ev, &reqs);
            if json {
                println!("{}", serde_json::to_string_pretty(&rep)?);
            }
            if ev.tests.is_empty() {
                // Without per-test data a test claim cannot be checked at
                // all, and silence would read as agreement.
                eprintln!("note       no per-test coverage in this evidence, so test claims were not checked (see --tests on ingest)");
            }
            // What could not be checked, claim by claim. The note above says
            // it of the *evidence*; this says it of the *claims*, which is
            // the part that decides whether a requirement may be called
            // agreed -- and it covers the unchecked claims the note does not
            // predict (#233).
            let unchecked_reqs = rep.not_checked_requirements();
            if !rep.not_checked.is_empty() {
                eprintln!("unchecked  {} claim(s) in {} requirement(s) the measurement cannot answer — neither agreed nor disagreed:", rep.not_checked.len(), unchecked_reqs.len());
                for n in &rep.not_checked {
                    eprintln!("           [{}] {}", n.kind, n.requirement);
                    eprintln!("                       document: {}", n.claimed);
                    eprintln!("                       evidence: {}", n.reason);
                }
            }
            // Agreement is stated as plainly as disagreement: an empty list
            // of problems and a check that never looked read the same
            // otherwise.
            eprintln!(
                "checked    {} requirement(s): {} agreed, {} not checked, {} with nothing claimed",
                reqs.entries.len(),
                rep.agreed.len(),
                unchecked_reqs.len(),
                rep.unclaimed.len()
            );
            if rep.is_empty() {
                if rep.not_checked.is_empty() {
                    eprintln!("claims     the document and the measurement agree everywhere they both speak");
                } else {
                    // The sentence this replaces was the defect on the
                    // terminal: no disagreement was being reported as
                    // agreement everywhere, over claims nobody had looked
                    // at. Say where the agreement stops (#233).
                    eprintln!(
                        "claims     no disagreement where the measurement could answer — but {} claim(s) went unchecked, so this is not agreement everywhere the document speaks",
                        rep.not_checked.len()
                    );
                }
                return Ok(());
            }
            let by = rep.by_kind();
            eprintln!("claims     {} disagreement(s): {}", rep.disagreements.len(), by.iter().map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join(", "));
            for d in &rep.disagreements {
                eprintln!("           [{}] {}", d.kind, d.requirement);
                eprintln!("                       document: {}", d.claimed);
                eprintln!("                       evidence: {}", d.measured);
            }
            std::process::exit(3);
        }
        Cmd::Requirements { input, out, config } => {
            // A directory means "the llr documents under here", which is how
            // requirements are actually kept: one document per component.
            let mut files = Vec::new();
            for p in &input {
                // The operator named this path, so a symlink here is their
                // choice and is resolved; links found *inside* the walk are
                // not, because the tree is less trusted than the argument.
                // `is_dir()` would say false for a broken link and send it to
                // the reader as a file, so the error comes from here instead.
                let md = std::fs::metadata(p).with_context(|| format!("reading {}", p.display()))?;
                if md.is_dir() {
                    collect_llr(p, 0, &mut files)?;
                } else {
                    files.push(p.clone());
                }
            }
            files.sort();
            files.dedup();
            if files.is_empty() {
                anyhow::bail!("no requirement documents found in {}", input.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
            }
            let limits = butterfly_model::limits::Limits::from_env();
            // A project's own shape, if it has said so; the measured default
            // otherwise.
            let cfg = match &config {
                Some(p) => butterfly_ingest::config::Config::load(p)?,
                None => butterfly_ingest::config::Config::default(),
            };
            let keys = cfg.requirements.keys.clone();
            // The reader prints nothing itself, so anything it wants said
            // comes back here. A note is not an error -- the document was
            // read -- but a column nobody could place is the kind of thing
            // that costs a reviewer an afternoon if it goes unsaid.
            let mut notes = Vec::new();
            let reqs = butterfly_ingest::reqtext::read(&files, &keys, &limits, &mut notes).map_err(|e| anyhow::anyhow!("{e}"))?;
            for n in &notes {
                eprintln!("note       {n}");
            }
            reqs.validate().map_err(|e| anyhow::anyhow!("produced an inconsistent sidecar — {e}"))?;

            let out = out.unwrap_or_else(|| PathBuf::from("requirements.json"));
            // Atomic, and a symlink at the destination is replaced rather
            // than written through (#2/#22), same as every other sidecar.
            let wrote =
                butterfly_view::sidecar::write_atomic(&out, serde_json::to_string_pretty(&reqs)?.as_bytes()).map_err(|e| anyhow::anyhow!("writing {}: {e}", out.display()))?;
            if let butterfly_view::sidecar::Wrote::ReplacedSymlink(t) = &wrote {
                eprintln!("note       {} was a symlink to {}; it was replaced, not written through", out.display(), t.display());
            }

            let derived = reqs.entries.iter().filter(|r| r.is_derived()).count();
            eprintln!("read       {} low-level requirement(s) from {} file(s)", reqs.entries.len(), reqs.sources.len());
            if derived > 0 {
                // Named rather than folded in: a derived requirement has no
                // parent to validate it against, so DO-178C 5.1.2.h puts it
                // in front of the safety assessment.
                eprintln!("derived    {derived} of them, which the safety assessment must see (DO-178C 5.1.2.h)");
            }
            if !reqs.without_text.is_empty() {
                // Traceable but unreviewable: the matrix can point at these,
                // and a reader arriving at one learns nothing.
                let shown: Vec<&str> = reqs.without_text.iter().take(5).map(String::as_str).collect();
                let more = reqs.without_text.len().saturating_sub(shown.len());
                eprintln!(
                    "no text    {} requirement(s) carry no statement: {}{}",
                    reqs.without_text.len(),
                    shown.join(", "),
                    if more > 0 { format!(", and {more} more") } else { String::new() }
                );
            }
            for (kind, n) in &reqs.skipped {
                eprintln!("skipped    {n} entr(ies) of kind `{kind}`: not a low-level requirement");
            }
            let planned = reqs.entries.iter().filter(|r| r.status.as_deref() == Some("planned")).count();
            if planned > 0 {
                eprintln!("planned    {planned} not yet implemented, so code is not expected to reference them");
            }
            println!("wrote {}", out.display());
            Ok(())
        }
        Cmd::Tags { evidence, source_root, out, ctags_bin } => {
            let text = read_evidence_text(&evidence)?;
            let ev: butterfly_model::Evidence = serde_json::from_str(&text)?;
            ev.validate().map_err(|e| anyhow::anyhow!("{}: inconsistent evidence — {e}", evidence.display()))?;
            ev.check_summary_derivation().map_err(|e| anyhow::anyhow!("{}: {e}", evidence.display()))?;

            let Some(ctags) = butterfly_ingest::tags::find_ctags(ctags_bin.as_deref()) else {
                anyhow::bail!(
                    "no Universal Ctags found. This feature is optional and needs it:\n                       brew install universal-ctags   (or your platform's package)\n                     then re-run, or pass --ctags-bin <path>. The system `ctags` on macOS is BSD ctags and will not do."
                );
            };
            // The source root is the boundary, not a hint: an evidence file's
            // filenames are untrusted, and ctags would parse whatever they
            // name. Default to the working directory, as `ingest` does, so
            // the default comes from the operator's shell rather than from
            // the document's own contents.
            let root = match source_root {
                Some(r) => r,
                None => std::env::current_dir().context("determining the working directory that bounds the index")?,
            };
            let produced = butterfly_ingest::tags::produce(&ev, &evidence, &root, &ctags)?;
            let tags = produced.tags;
            tags.validate(ev.files.len(), ev.functions.len()).map_err(|e| anyhow::anyhow!("produced an inconsistent index — {e}"))?;

            // Named from the evidence, not a bare `tags.json`: two evidence
            // files in one directory would otherwise share an index and
            // overwrite each other's (#6).
            let out = out.unwrap_or_else(|| {
                let stem = evidence.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "evidence".into());
                evidence.parent().unwrap_or(std::path::Path::new(".")).join(format!("{stem}.tags.json"))
            });
            // Replaced, never written through: a symlink at the destination
            // would otherwise redirect the write onto its target (#2).
            let bytes = serde_json::to_vec_pretty(&tags)?;
            match butterfly_view::sidecar::write_atomic(&out, &bytes).with_context(|| format!("writing {}", out.display()))? {
                butterfly_view::sidecar::Wrote::ReplacedSymlink(target) => {
                    eprintln!("tags     : {} was a symlink to {} — it was REPLACED, not written through; the target is untouched.", out.display(), target.display())
                }
                butterfly_view::sidecar::Wrote::Normally => {}
            }
            let in_ev = tags.entries.iter().filter(|t| t.file_id.is_some()).count();
            let linked = tags.entries.iter().filter(|t| t.function_id.is_some()).count();
            eprintln!(
                "tags     : {} definition(s) — {in_ev} in evidence files, {} in headers/elsewhere, {linked} linked to analysed functions",
                tags.entries.len(),
                tags.entries.len() - in_ev
            );
            if !produced.outside.is_empty() {
                eprintln!(
                    "tags     : {} file(s) named by the evidence resolve outside {} and were NOT read: {}",
                    produced.outside.len(),
                    root.display(),
                    produced.outside.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
                );
            }
            eprintln!("wrote {} ({})", out.display(), tags.generator);
            Ok(())
        }
    }
}

/// Write one of butterfly's outputs: atomically, and replacing a symlink at
/// the destination rather than writing through it (#2, #248). A replaced
/// link is said out loud -- the operator may have put it there on purpose,
/// and needs to know it is now a plain file.
fn write_output(path: &Path, bytes: &[u8]) -> Result<()> {
    if let butterfly_ingest::fsauth::Wrote::ReplacedSymlink(target) = butterfly_ingest::fsauth::write_atomic(path, bytes).with_context(|| format!("writing {}", path.display()))? {
        replaced_warning(path, &target);
    }
    Ok(())
}

/// The directory `parent/name` the build owns and cleans (`coverage`, one
/// per variant), made sure to be a real directory of its own.
///
/// The cleanup below it only checks the last component of each path: with
/// `parent/name` a symlink to somewhere else, every `.su` file, the dSYM
/// bundle, the executable and `build-info.txt` of the link's *target* were
/// deleted, and the build then wrote into it (#313). A symlink here is
/// removed -- the link, never its target -- and said out loud, as
/// [`write_output`] does for a file; anything else that is not a directory
/// is refused.
///
/// The directory is returned held open, by a name whose final component is
/// not a symlink, and the build cleans it only through that descriptor: a
/// swap of `parent/name` for a link after this check cannot redirect any of
/// it (#343).
pub(crate) fn output_subdir(parent: &Path, name: &str) -> Result<butterfly_ingest::fsauth::HeldDir> {
    let dir = parent.join(name);
    match std::fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("inspecting {}", dir.display())),
        Ok(m) if m.file_type().is_symlink() => {
            let target = std::fs::read_link(&dir).unwrap_or_default();
            std::fs::remove_file(&dir).with_context(|| format!("removing the symlink {}", dir.display()))?;
            eprintln!("warning  : {} was a symlink to {} — it was REPLACED by a directory of its own, not followed; the target is untouched.", dir.display(), target.display());
        }
        Ok(m) if m.is_dir() => return hold_dir(&dir),
        Ok(_) => anyhow::bail!("{} is not a directory; the build keeps its outputs there. Remove it if it is not wanted.", dir.display()),
    }
    std::fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
    hold_dir(&dir)
}

/// Open the directory just checked or made, refusing it if it has been
/// swapped for a link in the meantime.
fn hold_dir(dir: &Path) -> Result<butterfly_ingest::fsauth::HeldDir> {
    butterfly_ingest::fsauth::HeldDir::open(dir)
        .with_context(|| format!("opening {}, which the build keeps its outputs in, as a directory of its own (a symlink there is refused)", dir.display()))
}

/// Remove an earlier run's ordinary output file at `path` before a step that
/// would write it (#287): a file or a symlink -- the link itself, never its
/// target, and said out loud as [`write_output`] says it, since the operator
/// may have put it there on purpose. A directory here is NOT an output this
/// function owns (only a dSYM bundle is, and [`remove_output_dir_in`] removes
/// that): refuse it rather than recursively erase whatever it holds -- a
/// pre-existing directory colliding with an expected file name is the
/// operator's, not ours (#287 review). Absent is fine; the point is only that
/// no earlier *file* is there afterwards.
pub(crate) fn remove_output(path: &Path) -> Result<()> {
    let r = match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => Err(e),
        Ok(m) if m.file_type().is_symlink() => {
            let target = std::fs::read_link(path).unwrap_or_default();
            let r = std::fs::remove_file(path);
            if r.is_ok() {
                eprintln!("warning  : {} was a symlink to {} — it was REMOVED, not followed; the target is untouched.", path.display(), target.display());
            }
            r
        }
        Ok(m) if m.is_dir() => {
            anyhow::bail!(
                "{} is a directory where an ordinary output file was expected. An earlier run, or something else, put it there; refusing to remove it and everything inside. Remove it yourself if it is not wanted.",
                path.display()
            )
        }
        Ok(_) => std::fs::remove_file(path),
    };
    r.with_context(|| format!("removing {} left by an earlier run", path.display()))
}

/// [`remove_output`] for `name` in a directory the build holds open, through
/// its descriptor (#343).
pub(crate) fn remove_output_in(dir: &butterfly_ingest::fsauth::HeldDir, name: &std::ffi::OsStr) -> Result<()> {
    use butterfly_ingest::fsauth::Entry;
    let path = dir.path().join(name);
    let r = match dir.entry(name) {
        Ok(Entry::Absent) => return Ok(()),
        Err(e) => Err(e),
        Ok(Entry::Symlink(target)) => {
            let r = dir.unlink(name);
            if r.is_ok() {
                eprintln!("warning  : {} was a symlink to {} — it was REMOVED, not followed; the target is untouched.", path.display(), target.display());
            }
            r
        }
        Ok(Entry::Dir) => {
            anyhow::bail!(
                "{} is a directory where an ordinary output file was expected. An earlier run, or something else, put it there; refusing to remove it and everything inside. Remove it yourself if it is not wanted.",
                path.display()
            )
        }
        Ok(Entry::Other) => dir.unlink(name),
    };
    r.with_context(|| format!("removing {} left by an earlier run", path.display()))
}

/// Remove an earlier run's dSYM bundle `name` in a directory the build holds
/// open -- the one output that is a directory (#287). Used only for names
/// the build itself owns as a bundle (`{exe}.dSYM`), never for a name meant
/// to hold a file. A symlink is removed as the link, never followed; a plain
/// file where a bundle was expected is removed too. Absent is fine. The
/// bundle is removed through descriptors all the way down, never by a path
/// resolved again (#343).
pub(crate) fn remove_output_dir_in(dir: &butterfly_ingest::fsauth::HeldDir, name: &std::ffi::OsStr) -> Result<()> {
    use butterfly_ingest::fsauth::Entry;
    let path = dir.path().join(name);
    let r = match dir.entry(name) {
        Ok(Entry::Absent) => return Ok(()),
        Err(e) => Err(e),
        Ok(Entry::Symlink(target)) => {
            let r = dir.unlink(name);
            if r.is_ok() {
                eprintln!("warning  : {} was a symlink to {} — it was REMOVED, not followed; the target is untouched.", path.display(), target.display());
            }
            r
        }
        Ok(Entry::Dir) => dir.remove_tree(name),
        Ok(Entry::Other) => dir.unlink(name),
    };
    r.with_context(|| format!("removing {} left by an earlier run", path.display()))
}

fn replaced_warning(path: &Path, target: &Path) {
    eprintln!("warning  : {} was a symlink to {} — it was REPLACED, not written through; the target is untouched.", path.display(), target.display());
}

fn print_summary(ev: &butterfly_model::Evidence) {
    print_summary_with(ev, &butterfly_model::Resolutions { schema: butterfly_model::RESOLUTIONS_SCHEMA.into(), entries: Vec::new() });
}

fn print_summary_with(ev: &butterfly_model::Evidence, res: &butterfly_model::Resolutions) {
    let s = &ev.summary;
    let pct = |a: usize, b: usize| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
    if s.functions_unrecorded > 0 {
        // Not executed, not unexecuted: out of the denominator, and said so (#273).
        eprintln!(
            "files {}  functions {} ({} executed of {} with a coverage record; {} with none, whether they ran is not established)",
            s.files,
            s.functions,
            s.functions_executed,
            s.functions - s.functions_unrecorded,
            s.functions_unrecorded
        );
    } else {
        eprintln!("files {}  functions {} ({} executed)", s.files, s.functions, s.functions_executed);
    }
    eprintln!("lines      {:>5}/{:<5} {:5.1}%", s.lines_covered, s.lines_total, pct(s.lines_covered, s.lines_total));
    eprintln!("branches   {:>5}/{:<5} {:5.1}%", s.branches_covered, s.branches_total, pct(s.branches_covered, s.branches_total));
    eprintln!("MC/DC dec. {:>5}/{:<5} {:5.1}%", s.decisions_mcdc_covered, s.decisions_total, pct(s.decisions_mcdc_covered, s.decisions_total));
    eprintln!(
        "object     {} instr: {} projected covered, {} projected uncovered, {} unknown",
        s.instructions_total, s.instructions_projected_covered, s.instructions_projected_uncovered, s.instructions_unknown
    );
    eprintln!("gaps       {} {:?}", s.gaps_total, s.gaps_by_kind);
    eprintln!("generated  {} instr compiler-generated {:?}", s.instructions_compiler_generated, s.compiler_generated_by_origin);
    if let Some(w) = s.stack_worst_case {
        // The symbol's NAME, not its index in the evidence file. An index is
        // internal to the document and means nothing to a reader of the
        // summary (review of #212).
        let via = s.stack_worst_symbol.and_then(|i| ev.symbols.get(i)).map(|y| y.name.as_str()).unwrap_or("an unnamed symbol");
        eprintln!("stack      worst case {w} B via {via} · {} symbol(s) unbounded", s.stack_unbounded);
    }
    eprintln!("data       {} globals referenced, {} function->global refs", s.globals, s.data_refs);
    if let Some(report) = &ev.ozone_report {
        // Ozone's own counts for the scope it exported, beside ours, not merged into them.
        let total = &report.total;
        eprintln!(
            "ozone      {}/{} source lines, {}/{} instructions over {} function rows (Ozone's scope)",
            total.source_lines.covered,
            total.source_lines.total,
            total.instructions.covered,
            total.instructions.total,
            report.modules.iter().map(|m| m.functions.len()).sum::<usize>()
        );
    }
    if !ev.tests.is_empty() {
        // Coverage nobody can name a test for is coverage a reviewer has to
        // take on trust; DO-178C 6.4.4 is about what the requirements-based
        // tests did not exercise, so say how much of it is accounted for.
        let executed = ev.functions.iter().filter(|f| f.count > 0).count();
        let attributed = ev.functions.iter().filter(|f| !f.tests.is_empty()).count();
        eprintln!("tests      {} run · {attributed}/{executed} executed function(s) attributed to a named test", ev.tests.len());
        let mut by_test: Vec<(&str, usize)> = ev.tests.iter().map(|t| (t.name.as_str(), t.functions.len())).collect();
        by_test.sort_by_key(|(n, _)| *n);
        for (name, n) in by_test {
            // a test that enters nothing is either misattributed or dead
            eprintln!("           {name:<24} {n:>4} function(s)");
        }
    }
    {
        let m = butterfly_model::reqmatrix::compute(ev);
        if !m.rows.is_empty() || !m.requirements_without_code.is_empty() {
            let src = if m.per_test_data { "covered by the named tests" } else { "aggregate — no per-test data, so a proxy, not obj 3/4 evidence" };
            // With a universe supplied, the headline is its size: the rows
            // are only the *implemented* ones, and a count that omitted the
            // no-code entries printed below would understate the traceability
            // scope while listing more IDs than it claimed (review of #42).
            if m.requirements_without_code.is_empty() {
                eprintln!("reqs       {} requirement(s); lines {src}", m.rows.len());
            } else {
                eprintln!("reqs       {} requirement(s) in the supplied universe, {} implemented; lines {src}", m.rows.len() + m.requirements_without_code.len(), m.rows.len());
            }
            for r in m.rows.iter().take(8) {
                let tests = if !m.per_test_data {
                    "—".to_string()
                } else if r.tests.is_empty() {
                    "NO TEST".to_string()
                } else {
                    r.tests.iter().map(|&t| ev.tests[t].name.as_str()).collect::<Vec<_>>().join(",")
                };
                eprintln!("           {:<16} {:>3} fn · {:>3}/{:<3} lines · tests: {tests}", r.requirement, r.functions.len(), r.lines_covered, r.lines_total);
            }
            if m.rows.len() > 8 {
                eprintln!("           … and {} more requirement(s)", m.rows.len() - 8);
            }
            for r in &m.requirements_without_code {
                eprintln!("           {r:<16} NO CODE — in the supplied universe, tagged nowhere");
            }
            if !m.functions_without_requirement.is_empty() {
                eprintln!("           {} function(s) carry no requirement tag (extraneous, or untagged)", m.functions_without_requirement.len());
            }
        }
    }
    let work = butterfly_ingest::worklist::compute(ev, res);
    if !work.is_empty() {
        // ranked by leverage: what one new test would close, most first
        let total: usize = work.iter().map(|i| i.closes).sum();
        eprintln!("next       {} open test obligation(s); the top of the worklist:", total);
        for item in work.iter().take(5) {
            eprintln!("           [closes {:>2}] {} L{} — {}", item.closes, item.function, item.line, item.what);
            eprintln!("                       → {}", item.next);
        }
        if work.len() > 5 {
            eprintln!("           … and {} more (unresolved gaps are reviewer work, and are not in this list)", work.len() - 5);
        }
    }
    if let Some(b) = &ev.baseline {
        eprintln!(
            "baseline   {} ({} difference(s); {} source-changed, {} object-changed functions)",
            b.status,
            b.differences.len(),
            b.functions_source_changed.len(),
            b.functions_object_changed.len()
        );
    }
    if let Some(m) = &ev.measured {
        eprintln!(
            "measured   {} · {} block exec · {} instr executed · {} functions executed · {} disagreements with projection",
            m.format, m.block_executions, m.instructions_measured_covered, m.functions_executed, m.disagreements
        );
    }
}

/// How deep the walk may go, and how many documents it may find. The same
/// depth ceiling `collect_headers` uses: deep enough that no real tree
/// reaches it, shallow enough that recursion cannot exhaust the stack.
const LLR_MAX_DEPTH: u32 = 64;
const LLR_MAX_FILES: usize = 10_000;

/// Every `llr` document under `dir`, depth-first, bounded in both depth and
/// count, for the `requirements` command's directory form.
///
/// One base name in the formats the reader understands, rather than "every
/// .csv under here": a requirements tree holds other CSVs, and a walk that
/// swept them in would refuse the whole run over a file nobody offered it.
///
/// Bounded in both because either alone is a half-measure: a depth cap does
/// nothing against a wide shallow tree, and a count cap does nothing against
/// a deep narrow one. Symlinked directories are not descended, so a link
/// planted in the tree cannot redirect the walk somewhere nobody asked for
/// (the same rule `collect_headers` follows).
fn collect_llr(dir: &std::path::Path, depth: u32, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if depth > LLR_MAX_DEPTH {
        anyhow::bail!("{}: deeper than {LLR_MAX_DEPTH} directories; name the documents instead", dir.display());
    }
    for e in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let p = e?.path();
        // Not `metadata()`: that follows links, and a link into a tree of a
        // million files is a walk nobody asked for.
        let md = std::fs::symlink_metadata(&p)?;
        if md.file_type().is_symlink() {
            continue;
        }
        if md.is_dir() {
            collect_llr(&p, depth + 1, out)?;
        } else if md.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| matches!(n, "llr.yaml" | "llr.yml" | "llr.csv" | "llr.md" | "llr.markdown")) {
            if out.len() >= LLR_MAX_FILES {
                anyhow::bail!("more than {LLR_MAX_FILES} requirement documents under the given path; name them instead");
            }
            out.push(p);
        }
    }
    Ok(())
}

/// The requirement text for `run`'s report: a requirements document --
/// YAML, CSV or Markdown, or a directory of `llr` documents, read exactly as
/// `butterfly requirements` reads them, under the project's
/// `[requirements.keys]` -- or a `requirements.json` sidecar that command
/// already wrote. `run --help` calls the flag "a requirements document"; it
/// accepted only the sidecar and refused the document itself (#325 C3).
fn load_run_requirements(evidence: &std::path::Path, given: Option<PathBuf>, config: Option<&std::path::Path>) -> anyhow::Result<Option<butterfly_model::reqtext::Requirements>> {
    let Some(p) = given else { return load_report_requirements(evidence, None) };
    let md = std::fs::metadata(&p).with_context(|| format!("reading {}", p.display()))?;
    if md.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
        return load_report_requirements(evidence, Some(p));
    }
    let mut files = Vec::new();
    if md.is_dir() {
        collect_llr(&p, 0, &mut files)?;
        files.sort();
        if files.is_empty() {
            anyhow::bail!("no requirement documents found in {}", p.display());
        }
    } else {
        files.push(p);
    }
    let cfg = match config {
        Some(c) => butterfly_ingest::config::Config::load(c)?,
        None => butterfly_ingest::config::Config::default(),
    };
    let limits = butterfly_model::limits::Limits::from_env();
    let mut notes = Vec::new();
    let reqs = butterfly_ingest::reqtext::read(&files, &cfg.requirements.keys, &limits, &mut notes).map_err(|e| anyhow::anyhow!("{e}"))?;
    for n in &notes {
        eprintln!("note       {n}");
    }
    reqs.validate().map_err(|e| anyhow::anyhow!("inconsistent requirements — {e}"))?;
    eprintln!("requirements: {} statement(s) from {} document(s)", reqs.entries.len(), reqs.sources.len());
    Ok(Some(reqs))
}

/// The requirement text for a report: the path given, or `requirements.json`
/// beside the evidence.
///
/// An explicitly named file must load -- asking for it and silently getting a
/// report without it would be the quiet failure this whole feature is built
/// to avoid. A discovered one is optional, like every other sidecar.
///
/// Read through the auxiliary-input cap, as the resolutions sidecar and the
/// server already are: a report sidecar is another file of someone else's
/// bytes, and there is no reason for this path to be the one that trusts
/// them (review of #56).
fn load_report_requirements(evidence: &std::path::Path, given: Option<PathBuf>) -> anyhow::Result<Option<butterfly_model::reqtext::Requirements>> {
    use butterfly_model::limits::{read_bounded_string, Kind, Limits};
    let limits = Limits::from_env();
    let read = |p: &std::path::Path| -> anyhow::Result<butterfly_model::reqtext::Requirements> {
        let text = read_bounded_string(p, Kind::Auxiliary, &limits).map_err(|e| anyhow::anyhow!("{e}"))?;
        let q: butterfly_model::reqtext::Requirements = serde_json::from_str(&text).with_context(|| format!("parsing {}", p.display()))?;
        q.validate().map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
        Ok(q)
    };
    if let Some(p) = given {
        return Ok(Some(read(&p)?));
    }
    let stem = evidence.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = evidence.parent().unwrap_or(std::path::Path::new("."));
    for p in [dir.join(format!("{stem}.requirements.json")), dir.join("requirements.json")] {
        // `exists()` follows the link and answers false for a dangling one,
        // which would quietly reclassify "the baseline is broken" as "there
        // is no baseline" and produce a complete-looking report without it.
        // `symlink_metadata` asks about the entry itself, and only NotFound
        // means absent -- anything else is a sidecar that is there and
        // cannot be read, which has to be said.
        match std::fs::symlink_metadata(&p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(anyhow::anyhow!("{}: {e}", p.display())),
            Ok(_) => return Ok(Some(read(&p)?)),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod cleanup_tests {
    use super::{remove_output, remove_output_dir_in, remove_output_in};
    use butterfly_ingest::fsauth::HeldDir;
    use std::ffi::OsStr;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        butterfly_testsupport::scratch_dir(&format!("cleanup-{tag}"))
    }

    /// A directory sitting where an ordinary output file is expected is the
    /// operator's, not ours: remove_output refuses it and leaves it and its
    /// contents intact, rather than recursively erasing them (#287 review).
    #[test]
    fn remove_output_refuses_a_directory_and_preserves_its_contents() {
        let d = scratch("refuse-dir");
        let collide = d.join("coverage.json"); // a file name, but a dir is here
        fs::create_dir(&collide).unwrap();
        let inside = collide.join("precious.txt");
        fs::write(&inside, b"keep me").unwrap();

        let err = remove_output(&collide).unwrap_err();
        assert!(format!("{err:#}").contains("directory"), "the refusal names the cause: {err:#}");
        assert!(collide.is_dir(), "the directory is left in place");
        assert_eq!(fs::read(&inside).unwrap(), b"keep me", "its contents are untouched");
        let _ = fs::remove_dir_all(&d);
    }

    /// The ordinary case still works: a file (and a symlink) at the path is
    /// removed so the next write is not confused with an earlier run's.
    #[test]
    fn remove_output_removes_a_file() {
        let d = scratch("remove-file");
        let f = d.join("evidence.json");
        fs::write(&f, b"old").unwrap();
        remove_output(&f).unwrap();
        assert!(!f.exists(), "the file is gone");
        // absent is fine
        remove_output(&f).unwrap();
        let _ = fs::remove_dir_all(&d);
    }

    /// remove_output_dir_in is the one place a directory is an owned output
    /// (a dSYM bundle): it removes it recursively.
    #[test]
    fn remove_output_dir_removes_a_bundle() {
        let d = scratch("remove-dir");
        let bundle = d.join("demo.dSYM");
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        fs::write(bundle.join("Contents/info"), b"x").unwrap();
        let held = HeldDir::open(&d).unwrap();
        remove_output_dir_in(&held, OsStr::new("demo.dSYM")).unwrap();
        assert!(!bundle.exists(), "the bundle is removed");
        // absent is fine
        remove_output_dir_in(&held, OsStr::new("demo.dSYM")).unwrap();
        let _ = fs::remove_dir_all(&d);
    }

    /// The held-directory form of remove_output keeps its rules: a file goes,
    /// a symlink goes as itself, a directory is refused and kept.
    #[test]
    fn remove_output_in_keeps_the_rules_of_remove_output() {
        let d = scratch("remove-in");
        let outside = scratch("remove-in-outside");
        fs::write(outside.join("target"), b"keep").unwrap();
        fs::write(d.join("demo"), b"old").unwrap();
        std::os::unix::fs::symlink(outside.join("target"), d.join("build-info.txt")).unwrap();
        fs::create_dir(d.join("collide")).unwrap();
        let held = HeldDir::open(&d).unwrap();
        remove_output_in(&held, OsStr::new("demo")).unwrap();
        remove_output_in(&held, OsStr::new("build-info.txt")).unwrap();
        assert!(!d.join("demo").exists() && d.join("build-info.txt").symlink_metadata().is_err());
        assert_eq!(fs::read(outside.join("target")).unwrap(), b"keep", "the link's target is untouched");
        assert!(remove_output_in(&held, OsStr::new("collide")).is_err() && d.join("collide").is_dir());
        let _ = fs::remove_dir_all(&d);
        let _ = fs::remove_dir_all(&outside);
    }
}
