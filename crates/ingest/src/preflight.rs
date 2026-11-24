// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! What a build directory has to contain, checked before it is trusted.
//!
//! butterfly does not care which build system produced a directory. It cares
//! that a short list of things is in it, and the list is the same for a
//! hand-written `build.sh`, make, CMake or Bazel, and for clang or GCC.
//!
//! The reason this module exists is that **none of these mistakes produces an
//! error**. Each produces evidence that looks complete and is wrong:
//!
//! * analysing the *instrumented* binary gives coverage that looks perfect,
//!   because the counters are what was analysed rather than the delivered
//!   code;
//! * no `.su` files means every stack frame is estimated from the prologue,
//!   which is weaker evidence (§6.3.4.f) reported the same way as measured;
//! * `gcov` without `-b` yields no decision coverage at all;
//! * no `dsymutil` on macOS leaves every instruction with no source line.
//!
//! A tool whose output is certification evidence should not let those pass in
//! silence, so each check names the problem, says which build system usually
//! causes it, and gives the line to change.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// The evidence would be wrong, not merely thinner.
    Wrong,
    /// The evidence is weaker than it looks, and says so.
    Weaker,
    /// Worth knowing; nothing is misreported.
    Note,
}

impl Level {
    pub fn word(self) -> &'static str {
        match self {
            Level::Wrong => "WRONG",
            Level::Weaker => "WEAKER",
            Level::Note => "note",
        }
    }
}

/// One thing that is missing or wrong, and what to do about it.
#[derive(Debug, Clone)]
pub struct Finding {
    pub level: Level,
    /// A stable identifier, so a gate can select on it.
    pub kind: &'static str,
    /// What was found.
    pub what: String,
    /// Why it matters, in terms of the evidence.
    pub why: &'static str,
    /// What to change, per build system.
    pub fixes: Vec<(&'static str, String)>,
}

/// What the coverage export turned out to be, decided by the caller so this
/// module neither reads nor parses files.
#[derive(Debug, Clone, Copy)]
pub struct CoverageState {
    pub is_gcov: bool,
    /// Whether any decision in it carries outcome counts.
    pub has_branches: bool,
    /// The export contained branch records, whether or not any became a
    /// two-way decision. Separates "you omitted -b" from "these are switch or
    /// exception edges that are not two-way decisions".
    pub has_raw_branches: bool,
    /// Whether it carries MC/DC records. Only clang can, and only when the
    /// build asked for them.
    pub has_mcdc: bool,
    /// Whether the export contains any decision at all.
    ///
    /// The difference between "no branch records" and "no decisions to
    /// record" is not visible in the branch list alone: a straight-line
    /// program exported *with* `gcov -b` has none either. Without something
    /// that says a decision was expected, "you forgot -b" is a guess, and
    /// telling somebody to re-run the command they already ran is worse than
    /// saying nothing (review of #45).
    pub has_decisions: bool,
}

/// What happened when the coverage export was read.
#[derive(Debug, Clone)]
pub enum Coverage {
    /// Read and understood.
    Read(CoverageState),
    /// Present and unusable, with the reason.
    ///
    /// Distinct from absent: a malformed or over-limit export is a build that
    /// produced something butterfly cannot ingest, and reporting it as
    /// "no coverage features" would let `doctor` pass a directory `ingest`
    /// is about to refuse.
    Unreadable(String),
}

/// Which toolchain produced the coverage, as far as the directory shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolchain {
    Clang,
    Gcc,
    Unknown,
}

/// Is this object built with coverage instrumentation?
///
/// Both toolchains are asked about, because they leave different traces and a
/// check that knew only one would wave the other's instrumented binary
/// through — which is the single worst outcome on this list.
///
/// * **clang** puts the coverage map in its own section (`__llvm_covmap`,
///   `__llvm_prf_*`).
/// * **GCC** adds no such section. It emits a counter symbol per function
///   (`__gcov0.<name>`, `___gcov0.<name>` on Mach-O) and pulls in
///   `__gcov_init` / `__gcov_merge_add`. Looking only for sections misses it
///   completely.
pub fn instrumented_with(file: &object::File) -> Option<Toolchain> {
    use object::{Object, ObjectSection, ObjectSymbol};
    let clang = file.sections().filter_map(|s| s.name().ok().map(str::to_string)).any(|n| n.contains("llvm_covmap") || n.contains("llvm_prf"));
    if clang {
        return Some(Toolchain::Clang);
    }
    let gcc = file.symbols().chain(file.dynamic_symbols()).filter_map(|s| s.name().ok()).any(|n| {
        let n = n.trim_start_matches('_');
        n.starts_with("gcov0.") || n.starts_with("gcov_init") || n.starts_with("gcov_merge") || n.starts_with("gcov_exit")
    });
    if gcc {
        return Some(Toolchain::Gcc);
    }
    None
}

fn fixes_for_instrumented() -> Vec<(&'static str, String)> {
    vec![
        ("build.sh", "build twice from one source list: the second build with no --coverage / -fprofile-* / -fcoverage-* flag, and ingest *that* binary".into()),
        ("make", "a second target sharing $(SRCS), e.g. `app: CFLAGS += -g -fstack-usage` with no coverage flags".into()),
        ("cmake", "a second `add_executable` over the same source list; put the coverage flags only on the instrumented target with `target_compile_options`".into()),
        ("bazel", "a second `cc_binary` on the same `srcs`; keep `--collect_code_coverage` off that target".into()),
    ]
}

/// Check a directory that is about to be ingested.
///
/// `objects` are the candidate binaries discovery found, `skipped` the ones it
/// rejected as instrumented, and `cov` the coverage export if there is one.
pub fn check(dir: &Path, objects: &[PathBuf], skipped: &[(PathBuf, Toolchain)], cov: Option<&Path>, cov_state: Option<Coverage>, su: &[PathBuf]) -> Vec<Finding> {
    let mut out = Vec::new();

    // ---- an object at all -------------------------------------------------
    if objects.is_empty() {
        let what = if skipped.is_empty() {
            format!("no object file in {}", dir.display())
        } else {
            let names: Vec<String> = skipped.iter().map(|(p, t)| format!("{} ({:?} instrumentation)", p.file_name().unwrap_or_default().to_string_lossy(), t)).collect();
            format!("every object here is instrumented: {}", names.join(", "))
        };
        out.push(Finding {
            level: Level::Wrong,
            kind: "no-analysis-object",
            what,
            why: "the object code shown, disassembled and measured for stack is the *analysis* build. With only an instrumented binary there is nothing to project coverage onto, and analysing the instrumented one would measure the counters instead of the delivered code",
            fixes: fixes_for_instrumented(),
        });
    } else if !skipped.is_empty() {
        out.push(Finding {
            level: Level::Note,
            kind: "instrumented-skipped",
            what: format!(
                "{} instrumented object(s) ignored: {}",
                skipped.len(),
                skipped.iter().map(|(p, _)| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ")
            ),
            why: "this is what should happen — said out loud so a directory holding only instrumented binaries is not mistaken for one holding none",
            fixes: Vec::new(),
        });
    }

    // ---- the coverage export ---------------------------------------------
    match (cov, cov_state.as_ref()) {
        (None, _) => out.push(Finding {
            level: Level::Wrong,
            kind: "no-coverage",
            what: format!("no coverage export in {}", dir.display()),
            why: "without one there is no source coverage to project, and the run produces object-code structure alone",
            fixes: vec![
                ("clang", "llvm-profdata merge -sparse *.profraw -o x.profdata && llvm-cov export -format=text ... > coverage.json".into()),
                ("gcc", "gcov -b --json-format --stdout *.gcda > coverage.json".into()),
                ("bazel", "declare the export as a rule output; a sandboxed build leaves it where you will not find it".into()),
            ],
        }),
        // An export that is there and cannot be read. `ingest` will refuse
        // it, so `doctor` passing it would be advice that expires the moment
        // it is acted on.
        (Some(p), Some(Coverage::Unreadable(why))) => out.push(Finding {
            level: Level::Wrong,
            kind: "coverage-unreadable",
            what: format!("{} cannot be read: {why}", p.display()),
            why: "the export exists, so the build looks complete, but ingest cannot parse it -- nothing downstream of it can run",
            fixes: vec![
                ("any", "re-run the export step; a truncated file usually means the command failed or was interrupted".into()),
                ("limits", "if the file is simply large, raise BUTTERFLY_MAX_COVERAGE_MB".into()),
            ],
        }),

        // gcov without -b is the quiet one: a complete-looking export with no
        // decision coverage in it at all.
        //
        // Decided by parsing rather than by looking for the word "branches":
        // gcov emits `"branches": []` on every line when it was not asked for
        // them, so the key is present either way and a substring test says
        // the opposite of the truth.
        // ...but only where the export shows a decision that has no outcome
        // counts. A straight-line program exported *with* `-b` also has no
        // branch records, and telling its author to add the flag they already
        // passed is advice that cannot be acted on. Where the producer
        // recorded conditions but no branches, `-b` really is missing.
        // Records are there, but none of them is a two-way decision: a
        // `switch`, or blocks with exception edges. `-b` was passed and
        // telling the operator to add it would send them to fix a flag that
        // is already correct. Say what is actually unrepresented instead.
        (Some(_), Some(Coverage::Read(state))) if state.is_gcov && !state.has_branches && state.has_raw_branches => {
            out.push(Finding {
                level: Level::Weaker,
                kind: "gcov-no-two-way-decisions",
                what: "the gcov export has branch records, but none of them is a two-way decision".into(),
                why: "butterfly represents a decision only where a block has exactly two non-exception edges. A `switch`, or a block whose other edges are exception edges, is not forced into a shape it does not have -- so those constructs carry no decision coverage here, and objective 6 cannot be met for them from this export",
                fixes: vec![
                    ("gcc", "nothing to change in the build: the flag is already right. Cover these constructs at the object level, or record why they carry no decision coverage".into()),
                ],
            });
        }
        (Some(_), Some(Coverage::Read(state))) if state.is_gcov && !state.has_branches && !state.has_raw_branches && state.has_decisions => {
            out.push(Finding {
                level: Level::Wrong,
                kind: "gcov-no-branches",
                what: "the gcov export records decisions but carries no branch outcomes".into(),
                why: "gcov emits branch records only when asked, so the evidence would contain statement coverage alone while looking entirely healthy — objective 6 could not be met from it",
                fixes: vec![("gcc", "add -b: gcov -b --json-format --stdout *.gcda".into())],
            });
        }
        // The clang counterpart of the missing `-b`, and the one that costs
        // most: an export built without `-fcoverage-mcdc` carries branches
        // but no MC/DC records, so MC/DC reads 0/0 -- which is exactly what
        // code with no decisions reads. For Level A that difference *is* the
        // objective, and nothing on the page distinguishes "there was nothing
        // to measure" from "nobody asked for it".
        (Some(_), Some(Coverage::Read(state))) if !state.is_gcov && state.has_branches && !state.has_mcdc => {
            out.push(Finding {
                level: Level::Weaker,
                kind: "no-mcdc-instrumentation",
                what: "the clang export carries branches but no MC/DC records".into(),
                why: "MC/DC then reads 0/0, which is indistinguishable from code that has no decisions at all. clang can do MC/DC -- unlike GCC -- so for Level A this is a flag away rather than a limitation",
                fixes: vec![
                    ("clang", "add -fcoverage-mcdc alongside -fprofile-instr-generate -fcoverage-mapping".into()),
                    ("cmake", "add it to the instrumented target's target_compile_options, not the analysis one".into()),
                    ("bazel", "add it to the instrumented cc_binary's copts".into()),
                    ("not needed", "if this project is Level B or below, MC/DC is not required and this is only a note".into()),
                ],
            });
        }
        _ => {}
    }

    // ---- a profile that was never turned into an export -------------------
    //
    // Stopping one step early looks like having no coverage at all, and the
    // fix is completely different from actually not having instrumented.
    if cov.is_none() {
        let raws: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".profraw") || n.ends_with(".profdata") || n.ends_with(".gcda"))
            .collect();
        if !raws.is_empty() {
            let gcc = raws.iter().any(|n| n.ends_with(".gcda"));
            out.push(Finding {
                level: Level::Wrong,
                kind: "profile-not-exported",
                what: format!("raw profile data is here but no export: {}", raws.join(", ")),
                why: "the run happened and the counters exist; the step that turns them into the JSON butterfly reads was not run, so this looks exactly like a build that was never instrumented",
                fixes: if gcc {
                    vec![("gcc", "gcov -b --json-format --stdout *.gcda > coverage.json".into())]
                } else {
                    vec![("clang", "llvm-profdata merge -sparse *.profraw -o x.profdata && llvm-cov export -format=text -instr-profile=x.profdata <binary> > coverage.json".into())]
                },
            });
        }
    }

    // ---- stack usage ------------------------------------------------------
    if su.is_empty() {
        out.push(Finding {
            level: Level::Weaker,
            kind: "no-stack-usage",
            what: format!("no .su files in {}", dir.display()),
            why: "every stack frame is then estimated from the prologue rather than stated by the compiler, which is weaker evidence for §6.3.4.f",
            fixes: vec![
                ("build.sh", "add -fstack-usage to the analysis build, and move the .su files into the directory you ingest".into()),
                ("make", "add -fstack-usage to CFLAGS for the analysis target; .su lands beside each .o".into()),
                (
                    "cmake",
                    "add -fstack-usage, then collect from CMakeFiles/<target>.dir/ — they do not land in the build root (see examples/build-systems/cmake/collect_su.cmake)".into(),
                ),
                ("bazel", "declare the .su files as outputs, or the sandbox discards them".into()),
            ],
        });
    }

    // ---- provenance -------------------------------------------------------
    if !dir.join("build-info.txt").exists() {
        out.push(Finding {
            level: Level::Note,
            kind: "no-build-info",
            what: "no build-info.txt".into(),
            why: "the compiler and both flag sets are then absent from the evidence provenance, and the analysis baseline has less to compare against",
            fixes: vec![("any", "write key=value lines: compiler=..., flags_instr=..., flags_target=...".into())],
        });
    }

    // ---- macOS debug info -------------------------------------------------
    if cfg!(target_os = "macos") {
        let missing: Vec<String> = objects
            .iter()
            .filter(|o| {
                let dsym = PathBuf::from(format!("{}.dSYM", o.display()));
                !dsym.exists()
            })
            .map(|o| o.file_name().unwrap_or_default().to_string_lossy().into_owned())
            .collect();
        if !missing.is_empty() && !objects.is_empty() {
            out.push(Finding {
                level: Level::Weaker,
                kind: "no-dsym",
                what: format!("no .dSYM beside {}", missing.join(", ")),
                why: "on macOS the line table lives in the .dSYM after a link of separate objects; without it every instruction has no source line and the projection is worthless",
                fixes: vec![("any", "run `dsymutil <binary>` after linking".into())],
            });
        }
    }

    out.sort_by_key(|f| f.level);
    out
}

/// Print findings the way the CLI does, so `doctor` and `ingest` agree.
pub fn render(findings: &[Finding], verbose: bool) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    if findings.is_empty() {
        return "preflight : everything butterfly needs is here\n".to_string();
    }
    for f in findings {
        let _ = writeln!(s, "{:<9} {}", f.level.word(), f.what);
        if f.level == Level::Note && !verbose {
            continue;
        }
        let _ = writeln!(s, "          why: {}", f.why);
        for (system, fix) in &f.fixes {
            let _ = writeln!(s, "          {system:<9} {fix}");
        }
    }
    s
}

/// The distinct build systems named by a set of findings, for a summary line.
pub fn systems_named(findings: &[Finding]) -> BTreeSet<&'static str> {
    findings.iter().flat_map(|f| f.fixes.iter().map(|(s, _)| *s)).collect()
}
