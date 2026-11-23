// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The whole pipeline in one command: build, check, ingest, report, serve.
//!
//! Five commands in sequence is five chances to skip one. The failure that
//! motivated this was exactly that: a build script died leaving a zero-byte
//! coverage file, `ingest` was run anyway and said "no coverage found", and
//! the real cause was three steps back and invisible. So this narrates every
//! phase as it happens and ends with a ledger of what went well and what did
//! not.
//!
//! It runs your build script because *you* invoked it, from your terminal, as
//! you. That is not the same as the viewer running it: the viewer answers
//! HTTP, and any page in your browser can reach a loopback port. This is why
//! `butterfly serve` has no build endpoint and never will. See SECURITY.md.

use anyhow::{bail, Context, Result};
use butterfly_ingest::preflight::{self, Level};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

/// What one phase did, kept so the summary can be printed after everything.
pub struct Phase {
    pub name: &'static str,
    pub ok: bool,
    /// The one line worth remembering.
    pub note: String,
    /// Things that are true but unwelcome.
    pub warnings: Vec<String>,
}

pub struct Report {
    pub phases: Vec<Phase>,
}

impl Report {
    pub fn add_phase(&mut self, name: &'static str, ok: bool, note: impl Into<String>) -> &mut Phase {
        self.phases.push(Phase { name, ok, note: note.into(), warnings: Vec::new() });
        self.phases.last_mut().expect("just pushed")
    }

    /// Every phase that ran, what it produced, and what is still wrong. The
    /// point is that a reader who has scrolled past the build output can still
    /// answer "is this evidence usable?" from the last twenty lines.
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push('\n');
        s.push_str("─────────────────────────────── summary ───────────────────────────────\n");
        for p in &self.phases {
            s.push_str(&format!("{}  {:<9} {}\n", if p.ok { "  ok  " } else { " FAIL " }, p.name, p.note));
            for w in &p.warnings {
                s.push_str(&format!("        {:<9} {w}\n", ""));
            }
        }
        let failed = self.phases.iter().filter(|p| !p.ok).count();
        let warned: usize = self.phases.iter().map(|p| p.warnings.len()).sum();
        s.push_str("───────────────────────────────────────────────────────────────────────\n");
        // "weaker than it looks" is doctor's phrase for a specific class of
        // finding. Borrowing it for everything indented here -- a first run
        // with no reviews yet, say -- would overstate what was found.
        s.push_str(&match (failed, warned) {
            (0, 0) => "Everything went well.\n".to_string(),
            (0, w) => format!("Nothing failed. {w} thing(s) worth knowing, listed above.\n"),
            (f, 0) => format!("{f} phase(s) failed.\n"),
            (f, w) => format!("{f} phase(s) failed, and {w} thing(s) are worth knowing.\n"),
        });
        s
    }
}

pub fn heading(n: usize, total: usize, what: &str) {
    eprintln!();
    eprintln!("── [{n}/{total}] {what} {}", "─".repeat(58usize.saturating_sub(what.len())));
}

/// Run the project's build script, streaming its output through rather than
/// capturing it. A build is the slowest phase and the one most likely to go
/// wrong; watching it is the whole point.
pub fn build(project_dir: &Path, script: &Path, rep: &mut Report) -> Result<()> {
    if !script.exists() {
        bail!(
            "no build script at {}\n\
             Pass --build-script to name yours, or --no-build to use what is\n\
             already in the output directory. templates/build.sh is a starting point.",
            script.display()
        );
    }
    let script = std::fs::canonicalize(script).with_context(|| format!("resolving {}", script.display()))?;
    let dir = std::fs::canonicalize(project_dir).with_context(|| format!("resolving {}", project_dir.display()))?;
    eprintln!("running {} (in {})", script.display(), dir.display());
    eprintln!();

    // The build script is the operator's code, not the viewer's: it never
    // gets the viewer's token (review of #65).
    let mut child = Command::new(&script)
        .env_remove("BUTTERFLY_TOKEN")
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {} (is it executable? chmod +x)", script.display()))?;

    // Both streams are echoed. The build script's diagnostics are on stderr
    // and are the thing most worth seeing, so they are never swallowed.
    let out = child.stdout.take().expect("piped");
    let err = child.stderr.take().expect("piped");
    let pump = std::thread::spawn(move || {
        let mut kept = Vec::new();
        for line in BufReader::new(err).lines().map_while(std::result::Result::ok) {
            eprintln!("{line}");
            kept.push(line);
        }
        kept
    });
    let mut stdout_lines = 0usize;
    for line in BufReader::new(out).lines().map_while(std::result::Result::ok) {
        eprintln!("{line}");
        stdout_lines += 1;
    }
    let stderr_lines = pump.join().unwrap_or_default();
    let status = child.wait().context("waiting for the build script")?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        // A script that dies silently is the case that started all of this,
        // so when there was no output at all, say so explicitly rather than
        // leaving the reader to wonder what they missed. Either stream
        // counts: telling someone "it printed nothing" directly below a
        // screen of its own stdout would be a diagnostic that lies.
        if stderr_lines.is_empty() && stdout_lines == 0 {
            eprintln!();
            eprintln!("The build script exited {code} without printing anything.");
            eprintln!("That usually means a diagnostic was redirected to /dev/null.");
        }
        rep.add_phase("build", false, format!("{} exited {code}", script.display()));
        bail!("the build failed; nothing downstream would be trustworthy");
    }
    rep.add_phase("build", true, format!("{} exited 0", script.display()));
    Ok(())
}

/// Remove what an earlier `run` wrote into the build directory: the
/// evidence and the report. Removed, not left to be overwritten, so that a
/// run that stops at any phase leaves none of an earlier run's (#287). A
/// symlinked build directory is refused rather than followed, and removals go
/// through a `HeldDir` opened with `O_NOFOLLOW`.
pub fn clear_outputs(build_dir: &Path) -> Result<()> {
    match std::fs::symlink_metadata(build_dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("inspecting {}", build_dir.display())),
        Ok(m) if m.file_type().is_symlink() => {
            bail!("{} is a symlink; refusing to clear outputs through a symlinked --out-dir", build_dir.display());
        }
        Ok(m) if !m.is_dir() => return Ok(()),
        Ok(_) => {}
    }
    let held =
        butterfly_ingest::fsauth::HeldDir::open(build_dir).with_context(|| format!("opening {} as a directory of its own (a symlink there is refused)", build_dir.display()))?;
    for name in ["evidence.json", "report.html"] {
        crate::remove_output_in(&held, std::ffi::OsStr::new(name))?;
    }
    Ok(())
}

/// Everything `doctor` would say, folded into the run.
pub fn doctor(dir: &Path, verbose: bool, rep: &mut Report) -> Result<Vec<preflight::Finding>> {
    let (opts, cov) = butterfly_ingest::discover_artifacts(dir)?;
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
                has_raw_branches: ev.raw_branch_records,
                has_mcdc: ev.functions.iter().any(|f| !f.mcdc.is_empty()),
                has_decisions: ev.decision_sites,
            }),
        }
    });
    let findings = preflight::check(dir, &opts.objects, &opts.instrumented_skipped, cov.as_deref(), cov_state, &opts.stack_usage);

    eprintln!("directory : {}", dir.display());
    if let Some(c) = &cov {
        eprintln!("coverage  : {}", c.display());
    }
    for o in &opts.objects {
        eprintln!("object    : {}", o.display());
    }
    eprintln!("stack     : {} .su file(s)", opts.stack_usage.len());
    eprintln!();
    eprint!("{}", preflight::render(&findings, verbose));

    let wrong: Vec<&preflight::Finding> = findings.iter().filter(|f| f.level == Level::Wrong).collect();
    let weaker: Vec<&preflight::Finding> = findings.iter().filter(|f| f.level == Level::Weaker).collect();
    let note = if wrong.is_empty() && weaker.is_empty() { "nothing to fix".to_string() } else { format!("{} would make the evidence wrong, {} weaker", wrong.len(), weaker.len()) };
    let ok = wrong.is_empty();
    let p = rep.add_phase("doctor", ok, note);
    for f in weaker {
        p.warnings.push(format!("weaker: {}", f.what));
    }
    for f in findings.iter().filter(|f| f.level == Level::Wrong) {
        p.warnings.push(format!("WRONG:  {}", f.what));
    }
    Ok(findings)
}
