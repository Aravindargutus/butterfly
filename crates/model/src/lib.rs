// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Evidence database schema.
//!
//! One `Evidence` value is the complete, self-describing output of
//! `butterfly ingest`. It is serialised to JSON and is the *only* thing the
//! viewer reads. Every number the viewer displays must be derivable from this
//! file, and every value here must be traceable to an input artifact listed in
//! `provenance.inputs` (by SHA-256).
//!
//! Field naming is `camelCase` in JSON.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod claimcheck;
pub mod limits;
pub mod reqmatrix;
pub mod reqtext;
pub mod resolutions;
pub mod revision;
pub mod tags;

pub const SCHEMA: &str = "butterfly-evidence/1";

fn one() -> u32 {
    1
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub schema: String,
    pub provenance: Provenance,
    pub summary: Summary,
    pub files: Vec<SourceFile>,
    pub functions: Vec<Function>,
    /// Per-test attribution, when the ingest was given per-test exports.
    /// Empty for a single merged export, which is the ordinary case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<TestRun>,
    /// The requirements the project says exist (`ingest --requirements`),
    /// whether or not any code is tagged with them. This is what makes a
    /// "requirement with no code" knowable: absence cannot be inferred from
    /// the tags alone (#42).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirements_universe: Vec<String>,
    /// Every text symbol in the analysed image (covered or not).
    #[serde(default)]
    pub symbols: Vec<Symbol>,
    /// Data symbols referenced from code (globals, statics).
    #[serde(default)]
    pub globals: Vec<Global>,
    #[serde(default)]
    pub image: Option<Image>,
    #[serde(default)]
    pub measured: Option<Measured>,
    /// Result of comparing this evidence with a frozen analysis baseline
    /// (DO-178C 4.4.2.c: new compiler / options / constructs invalidate earlier
    /// coverage and traceability analyses).
    #[serde(default)]
    pub baseline: Option<BaselineCheck>,
    /// Change impact: which commits and tickets touched each function, from
    /// git history (`butterfly changes`). Absent until that command has run.
    #[serde(default)]
    pub changes: Option<Changes>,
}

pub const CHANGES_SCHEMA: &str = "butterfly-changes/1";

/// Function -> commit -> ticket, recovered from the repository history with
/// `git log -L` over each function's line range (which follows the range
/// through edits above it and through file renames). The trace a reviewer
/// needs is ticket -> functions -> requirements -> verification state; the
/// first hop lives here and the rest is already on `Function`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    /// Repository top level.
    pub repo: String,
    /// Commit the history was read at.
    pub head: String,
    /// Revision the walk started after (`<since>..head`); `None` = full history.
    pub since: Option<String>,
    pub since_sha: Option<String>,
    pub created_unix: u64,
    /// Ticket patterns in force (capture group 1, or the whole match).
    pub patterns: Vec<String>,
    pub commits: Vec<Commit>,
    pub tickets: Vec<Ticket>,
    /// Commits that touched analysed code but carry no ticket: an audit
    /// finding in its own right.
    pub unticketed: Vec<usize>,
    /// How each analysed source file relates to the repository.
    pub files: Vec<ChangedFile>,
    /// Functions in the baseline that no longer exist (needs `--since` and
    /// the frozen baseline).
    #[serde(default)]
    pub deleted: Vec<DeletedFunction>,
    pub caveats: Vec<String>,
    /// The re-verification obligations the changes create (phase 3 of the
    /// change impact analysis). Computed from the evidence and the
    /// resolutions sidecar by `butterfly changes` / `butterfly impact`.
    #[serde(default)]
    pub impact: Option<Impact>,
}

/// What must be re-verified because of the changes since the certified
/// baseline (DO-178C 12.1.1). Each obligation names a changed function and
/// the artefact it invalidates or the work it requires.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Impact {
    /// `true` when the history was read with `--since`, so "changed" means
    /// "changed since the certified baseline". Without it every function
    /// that has any history counts as changed, which is rarely what is wanted.
    pub bounded: bool,
    pub functions_changed: Vec<usize>,
    /// Sorted: blockers first, then major, then advisory; within a severity
    /// by function id, then kind, then target.
    pub obligations: Vec<Obligation>,
    pub by_ticket: Vec<TicketImpact>,
    pub by_kind: BTreeMap<String, usize>,
    pub by_severity: BTreeMap<String, usize>,
    pub caveats: Vec<String>,
    /// SHA-256 of the resolution set this impact was derived from, and of the
    /// evidence revision it was derived against. A persisted impact is a cache;
    /// bound to nothing, a report could show it as current after the reviews
    /// or the build moved on (#249). A consumer recomputes both from the
    /// evidence and resolutions in hand and, if either differs, treats the
    /// impact as stale. Absent (empty) in impacts written before this field.
    #[serde(default)]
    pub resolutions_digest: String,
    #[serde(default)]
    pub evidence_digest: String,
}

/// One thing to do or to redo. `kind` values and their severities:
///
/// | kind | severity | meaning |
/// |---|---|---|
/// | `not-exercised` | blocker | the changed function never ran in the coverage run |
/// | `unticketed` | blocker | a commit touching it carries no ticket (change outside change control) |
/// | `no-requirement` | blocker | the changed function traces to no requirement |
/// | `stale-review` | blocker | its source-to-object review predates the change |
/// | `stale-resolution` | blocker | a gap / condition / branch / line / dead-code resolution on it predates the change |
/// | `dead-changed` | blocker | it changed and nothing in the image calls it |
/// | `uncovered-lines` | major | lines in it are not covered |
/// | `mcdc-incomplete` | major | a decision in it is not MC/DC covered |
/// | `retest-requirement` | advisory | the tests for one of its requirements must be re-run |
/// | `retest-caller` | advisory | a caller (control coupling) must be re-tested |
/// | `data-coupling` | advisory | a reader of a global it writes must be re-tested |
/// | `deleted` | blocker | a function the baseline certified no longer exists: its tests are obsolete and its callers changed |
/// | `untraced-file` | major | the ticket changed a file holding no function in the evidence (header, linker script, build file): every includer's object code is suspect |
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Obligation {
    pub kind: String,
    /// `blocker` | `major` | `advisory`
    pub severity: String,
    /// The changed function; `None` for ticket-level findings (a deleted
    /// function, a changed file with no function in the evidence).
    pub function_id: Option<usize>,
    /// Repository-relative file (`deleted`, `untraced-file`).
    #[serde(default)]
    pub file: Option<String>,
    /// Index into `Changes.tickets` for ticket-level findings.
    #[serde(default)]
    pub ticket: Option<usize>,
    /// The coupled function (`retest-caller`, `data-coupling`).
    pub target_function: Option<usize>,
    /// Resolution key or global name.
    pub key: Option<String>,
    pub requirement: Option<String>,
    /// Index into `Changes.commits` (`unticketed`).
    pub commit: Option<usize>,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketImpact {
    /// Index into `Changes.tickets`.
    pub ticket: usize,
    /// Indices into `Impact.obligations`.
    pub obligations: Vec<usize>,
    pub blockers: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub sha: String,
    pub short: String,
    pub author: String,
    /// ISO 8601 author date, for display.
    pub date: String,
    /// Author date as a unix timestamp: the sort key. ISO strings only sort
    /// chronologically within a single UTC offset.
    #[serde(default)]
    pub date_unix: u64,
    pub subject: String,
    /// Indices into `Changes.tickets`.
    pub tickets: Vec<usize>,
    /// Functions whose lines this commit touched (function ids).
    pub functions: Vec<usize>,
    /// Every file the commit changed (`git show --name-status`), including
    /// files that hold no function in the evidence.
    #[serde(default)]
    pub files: Vec<FileChange>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// Repository-relative path (the new path for a rename).
    pub path: String,
    /// `added` | `modified` | `deleted` | `renamed`
    pub status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub id: String,
    /// Indices into `Changes.commits`.
    pub commits: Vec<usize>,
    /// Union of the live functions those commits touched (function ids).
    pub functions: Vec<usize>,
    /// Functions whose origin commit belongs to this ticket.
    #[serde(default)]
    pub added: Vec<usize>,
    /// Functions that existed before this ticket and were touched by it.
    #[serde(default)]
    pub modified: Vec<usize>,
    /// Indices into `Changes.deleted`.
    #[serde(default)]
    pub deleted: Vec<usize>,
    /// Every file the ticket's commits changed, with the net status over the
    /// ticket and whether the evidence holds any function in it.
    #[serde(default)]
    pub files: Vec<TicketFile>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketFile {
    pub path: String,
    /// `added` | `modified` | `deleted` | `renamed`
    pub status: String,
    /// The evidence has this file (so its functions are attributed).
    pub in_evidence: bool,
    /// Functions the evidence holds in it.
    pub functions: usize,
}

/// A function the baseline certified that no longer exists.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedFunction {
    pub name: String,
    /// Repository-relative path it was in.
    pub file: String,
    pub line_start: u32,
    pub line_end: u32,
    /// The commit that removed it (index into `Changes.commits`), when it
    /// could be located.
    pub commit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub file_id: usize,
    /// Path relative to the repository top level.
    pub repo_path: String,
    /// `matches-head` — the analysed text is exactly the committed text;
    /// `matches-worktree` — it matches uncommitted edits, so line ranges
    /// may not line up with HEAD; `mismatch` — it matches neither, the
    /// evidence came from a different checkout; `untracked` / `outside-repo`
    /// / `excluded` — not walked.
    pub status: String,
    /// Functions walked in this file.
    pub functions_walked: usize,
}

impl Baseline {
    /// A baseline frozen by an older butterfly cannot answer the questions
    /// this one asks -- `butterfly-baseline/1` carries no object fingerprint,
    /// so comparing against it would silently fall back to instruction and
    /// block counts. Say so and name the fix.
    pub fn check_schema(&self) -> Result<(), String> {
        if self.schema == BASELINE_SCHEMA {
            return Ok(());
        }
        Err(format!(
            "this baseline is {:?}, but this butterfly writes {BASELINE_SCHEMA}. \
             Older baselines carry no object-code fingerprint, no function \
             locations or no digest of a C++ constructor's or destructor's \
             variants, so comparing against one would under-report changes. Re-freeze it from the current evidence: `butterfly baseline \
             <evidence.json> -o <baseline.json>`, and keep the new file under \
             configuration management.",
            self.schema
        ))
    }
}

/// A frozen description of the environment and code an analysis was done
/// against. Written by `butterfly baseline`, compared on every ingest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    pub schema: String,
    pub created_unix: u64,
    pub compiler: String,
    pub flags_target: String,
    pub flags_instr: String,
    pub arch: String,
    pub object_format: String,
    pub coverage_export_version: String,
    /// All `build-info.txt` keys (toolchain, linker, target ...).
    pub build_info: BTreeMap<String, String>,
    /// Union of source constructs over all functions.
    pub constructs: Vec<String>,
    pub functions: BTreeMap<String, FnFingerprint>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FnFingerprint {
    /// The function's display name. Carried so the fingerprint map can be
    /// keyed by a unique identity (name plus file and line) rather than the
    /// name alone -- two internal-linkage functions can share a display name,
    /// and a name-keyed map silently overwrote one with the other before they
    /// could be compared (#249). Absent in baselines frozen before this field.
    #[serde(default)]
    pub name: String,
    /// SHA-256 of the function's source lines (trimmed).
    pub source_sha256: String,
    pub constructs: BTreeMap<String, u32>,
    pub instr_total: usize,
    pub blocks: usize,
    /// SHA-256 of the function's normalised object code: mnemonics, operands
    /// with load addresses made function-relative, and the CFG edges. Counts
    /// alone are not a fingerprint -- an opcode, an immediate or a branch
    /// target can change while both stay equal.
    #[serde(default)]
    pub object_sha256: String,
    /// The same digest for each compiler-emitted variant of a C++
    /// constructor or destructor (`C1`, `D1`, the deleting `D0`), keyed by
    /// its variant code. A change confined to a variant -- `D0`'s call to
    /// `operator delete` under `-fsized-deallocation` -- changes the function's
    /// object code as surely as a change to its body (#280). Empty for a
    /// function with no variants.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variants: BTreeMap<String, String>,
    /// Where the function was when the baseline was frozen (path as recorded
    /// in the evidence, 1-based inclusive lines). Needed to locate a
    /// deletion in the history; absent in baselines frozen before this field.
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line_start: u32,
    #[serde(default)]
    pub line_end: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineCheck {
    pub path: String,
    pub created_unix: u64,
    /// `matches` | `changed`
    pub status: String,
    pub differences: Vec<BaselineDifference>,
    /// Functions whose source text changed.
    pub functions_source_changed: Vec<String>,
    /// Functions whose object shape changed (instruction/block count) while
    /// the source did not — compiler or option effect.
    pub functions_object_changed: Vec<String>,
    pub functions_added: Vec<String>,
    pub functions_removed: Vec<String>,
    pub new_constructs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineDifference {
    /// `compiler` | `flags` | `arch` | `coverage-toolchain` | `toolchain` |
    /// `constructs` | `functions`
    pub kind: String,
    pub detail: String,
    /// True when this difference invalidates earlier analyses (4.4.2.c).
    pub invalidates: bool,
}

/// `/4`: fingerprints cover every variant of a C++ constructor or destructor
/// (#280). A `/3` baseline has no variant digests and is refused, to be
/// re-frozen, rather than compared as if its variants were absent.
pub const BASELINE_SCHEMA: &str = "butterfly-baseline/4";

/// One text symbol of the image, whether or not it has source coverage.
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Symbol {
    pub id: usize,
    pub name: String,
    pub mangled: String,
    pub address: u64,
    pub size: u64,
    pub section: String,
    pub function_id: Option<usize>,
    pub frame: Frame,
    /// Direct callees (symbol ids), from `bl`/`call` targets.
    pub callees: Vec<usize>,
    pub indirect_calls: u32,
    pub stack: StackResult,
    /// Executions of the first instruction, when a trace was imported.
    #[serde(default)]
    pub measured: Option<u64>,
    /// Global data this symbol reads / writes / takes the address of.
    #[serde(default)]
    pub data_refs: Vec<DataRef>,
    /// Symbols that take this symbol's address (function pointers, tables).
    #[serde(default)]
    pub address_taken_by: Vec<usize>,
    /// Symbols that call this one directly.
    #[serde(default)]
    pub callers: Vec<usize>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub bytes: u64,
    pub source: FrameSource,
    /// Frame grows at run time (alloca / VLA / `sub sp, sp, xN`).
    pub dynamic: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FrameSource {
    /// `.stack_sizes` section (`-fstack-size-section` / `-Zemit-stack-sizes`).
    StackSizesSection,
    /// `.su` file (`-fstack-usage`).
    StackUsageFile,
    /// Estimated from the function prologue in the disassembly.
    Prologue,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackResult {
    /// Own frame plus the deepest direct-call chain, in bytes.
    pub worst_case: Option<u64>,
    /// Symbol ids along the deepest chain, starting with this symbol.
    pub path: Vec<usize>,
    /// False when recursion or indirect calls make the bound unreliable.
    pub bounded: bool,
    pub reason: Option<String>,
    pub depth: u32,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Global {
    pub id: usize,
    pub name: String,
    pub mangled: String,
    pub address: u64,
    pub size: u64,
    pub section: String,
    /// Function ids (covered code only).
    pub readers: Vec<usize>,
    pub writers: Vec<usize>,
    pub address_takers: Vec<usize>,
    /// Symbol ids (all code in the image).
    #[serde(default)]
    pub reader_symbols: Vec<usize>,
    #[serde(default)]
    pub writer_symbols: Vec<usize>,
    #[serde(default)]
    pub address_taker_symbols: Vec<usize>,
}

/// One parameter or local, and where the object code touches it.
///
/// Built from the compiler's own record rather than from reading the source:
/// `DW_TAG_formal_parameter` and `DW_TAG_variable` say what exists and where
/// it lives, and the disassembly says which instructions reach that place.
/// A source-level scan would be guessing at both.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    /// `param` or `local`, as the DWARF tag said.
    pub kind: String,
    /// 1-based line the compiler recorded as its declaration.
    pub decl_line: u32,
    /// The type's name as written in the debug info, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    /// Every touch, in **static instruction order**: the order the
    /// instructions that perform them appear in the image, which is address
    /// order.
    ///
    /// Order is the point -- a set of lines could not carry it, and a line
    /// touched twice would collapse -- but it is a *static* order and must
    /// never be presented as an executed one. The attribution scans an
    /// address-sorted instruction list once: no control-flow path is walked
    /// and no run is traced, so a site inside a loop appears once however
    /// many iterations run, a site inside a branch appears whether or not it
    /// is reached, and two positions on one line say which instruction comes
    /// first in the image rather than which ran first (review of #53).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accesses: Vec<VarAccess>,
    /// Why some or all of this variable could not be attributed, when that
    /// is so.
    ///
    /// Present rather than silent: a variable the optimiser kept in a
    /// register, or dropped for part of its scope, has fewer accesses than
    /// the source implies, and an empty list that means "we could not tell"
    /// must not read as "it is never used".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unattributed: Option<String>,
}

/// One instruction touching a variable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VarAccess {
    /// Instruction address, so the object view can line up with this.
    pub addr: u64,
    /// Source line the line table attributes that instruction to, when it
    /// attributes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// `read`, `write`, or `address` -- the same three the global data
    /// coupling scan reports, for the same reason.
    pub access: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataRef {
    pub global_id: usize,
    pub reads: u32,
    pub writes: u32,
    pub addr_taken: u32,
    pub lines: Vec<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    pub path: String,
    pub format: String,
    pub arch: String,
    pub entry: Option<u64>,
    pub segments: Vec<Segment>,
    pub sections: Vec<Section>,
    /// Human-readable observations (overlaps, unverified code, ...).
    pub findings: Vec<String>,
    pub text_symbols: usize,
    /// Symbol ids of text symbols with no coverage function behind them.
    pub text_symbols_uncovered: Vec<usize>,
    pub text_bytes: u64,
    pub text_bytes_covered: u64,
    /// Object-level dead-code candidates (DO-178C 6.4.4.3.c).
    #[serde(default)]
    pub dead_candidates: Vec<DeadCandidate>,
}

/// A text symbol nothing in the image references: no direct call, no
/// address taken, not the entry point, and (with a trace) never executed.
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeadCandidate {
    pub symbol_id: usize,
    pub name: String,
    pub size: u64,
    pub function_id: Option<usize>,
    /// Source-level execution count when the symbol has coverage.
    pub count: Option<u64>,
    /// Executions in the trace, when one was imported.
    pub measured: Option<u64>,
    pub reasons: Vec<String>,
    /// Stable key for resolutions: `dead:<name>`.
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub file_size: u64,
    pub flags: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub kind: String,
    pub flags: String,
    pub symbols: usize,
    /// Largest symbols in the section: (name, bytes).
    pub largest: Vec<(String, u64)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measured {
    pub source: String,
    pub format: String,
    pub block_executions: u64,
    pub addresses_in_trace: usize,
    pub addresses_matched: usize,
    pub addresses_outside_symbols: usize,
    pub functions_executed: usize,
    pub instructions_measured_covered: usize,
    pub disagreements: usize,
    pub caveats: Vec<String>,
    /// Block-to-block transitions seen in the trace.
    #[serde(default)]
    pub transitions: u64,
    /// ... of which matched an edge of a function's CFG.
    #[serde(default)]
    pub transitions_matched: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Disagreement {
    pub addr: u64,
    pub line: Option<u32>,
    pub kind: DisagreementKind,
    pub projected: Status,
    pub measured: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DisagreementKind {
    /// Source coverage implied the instruction ran; the trace never saw it.
    ProjectedNotExecuted,
    /// Source coverage said the line never ran; the trace executed it.
    ExecutedNotProjected,
    /// No projection was possible; the trace executed it.
    ExecutedUnknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    pub tool: String,
    pub tool_version: String,
    /// The commit of butterfly's own sources the tool was built from
    /// (#117): the full id, 40 hex digits (64 in a SHA-256 repository).
    /// `unknown` when the build could not establish it -- no git, or not a
    /// checkout of butterfly -- and nothing was given through
    /// `BUTTERFLY_BUILD_COMMIT`; empty in evidence written before it was
    /// recorded. The tool version alone names a release, not a build: every
    /// build between two releases carries the same `tool_version`, so this
    /// is what traces evidence to the tool's source baseline (DO-330 §7.2.1,
    /// §7.2.2.g). `#[serde(default)]`: older evidence still loads.
    #[serde(default)]
    pub tool_commit: String,
    /// Whether butterfly's own work tree had uncommitted changes when it was
    /// built -- `Some(true)` means the binary is not the sources
    /// `tool_commit` names. `None` when that could not be established, and
    /// in evidence written before it was recorded; it never reads as clean.
    #[serde(default)]
    pub tool_dirty: Option<bool>,
    pub created_unix: u64,
    pub host: String,
    /// Every file read during ingest, with its hash.
    pub inputs: Vec<InputArtifact>,
    /// Free-form key/values from a `build-info.txt` next to the artifacts
    /// (compiler version, flags, git hash ...).
    pub build_info: BTreeMap<String, String>,
    pub coverage_export_version: String,
    pub object_format: String,
    pub arch: String,
    /// The external tools butterfly invoked to produce this evidence, with the
    /// path each was resolved to and its self-reported version. The object-code
    /// evidence *is* `llvm-objdump`'s output; a different objdump can print
    /// different operand syntax, so the disassembler that produced it must be
    /// identified (DO-330 §5.2.2.2.g, §6.1.3.3.e). `git` appears here when
    /// change data was embedded. `#[serde(default)]`: evidence written before
    /// this field existed still loads, with no external tools recorded.
    #[serde(default)]
    pub external_tools: Vec<ExternalTool>,
    /// The project's source revision and whether the files the build used
    /// are that revision's (#300): see [`revision`]. `None` in evidence
    /// written before it was recorded, which says nothing about the sources
    /// -- it never reads as clean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<revision::SourceRevision>,
    /// Human-readable caveats the viewer must show (e.g. "object coverage is
    /// projected from source coverage, not measured").
    pub caveats: Vec<String>,
}

impl Provenance {
    /// The tool's build, for a report line or a banner: `commit <id>, clean
    /// tree` / `tree modified at build` / `tree state unknown`, `commit
    /// unknown` when the build could not establish it, and `commit not
    /// recorded` for evidence that predates the field. Shared with
    /// `butterfly --version` through [`describe_tool_build`] so the binary
    /// and the evidence it writes describe one build the same way.
    pub fn tool_build(&self) -> String {
        describe_tool_build(&self.tool_commit, self.tool_dirty)
    }
}

/// See [`Provenance::tool_build`]. `commit` is the value `tool_commit`
/// holds; `dirty` the value `tool_dirty` holds.
pub fn describe_tool_build(commit: &str, dirty: Option<bool>) -> String {
    match commit {
        "" => "commit not recorded".into(),
        "unknown" => "commit unknown".into(),
        id => {
            let tree = match dirty {
                Some(false) => "clean tree",
                Some(true) => "tree modified at build",
                None => "tree state unknown",
            };
            format!("commit {id}, {tree}")
        }
    }
}

/// One external component butterfly ran, identified for configuration
/// management. `version` is the first non-empty line of the tool's
/// `--version`; it is empty when that line could not be read, in which case a
/// provenance caveat records that the tool ran but its version is unknown.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalTool {
    pub name: String,
    pub path: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputArtifact {
    pub role: String,
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub files: usize,
    pub functions: usize,
    pub functions_executed: usize,
    /// Of `functions`, those with no coverage record of their own
    /// (`Function::unrecorded_within`, #273). They are neither executed nor
    /// unexecuted, so a reader of `functions_executed / functions` must take
    /// them out of the denominator, and say so.
    #[serde(default)]
    pub functions_unrecorded: usize,
    pub lines_total: usize,
    pub lines_covered: usize,
    pub branches_total: usize,
    pub branches_covered: usize,
    pub decisions_total: usize,
    pub decisions_mcdc_covered: usize,
    /// MC/DC decision records whose file could not be established, so they
    /// could not be placed and are absent from every function's `decisions`
    /// (LLVM 18 names no file and the condition regions did not settle it,
    /// #269). They remain unproven obligations: an MC/DC gate must not pass
    /// while any exist, so the count is carried structurally rather than only
    /// in a caveat (#284).
    #[serde(default)]
    pub decisions_unplaced: usize,
    pub conditions_total: usize,
    pub conditions_independence_covered: usize,
    pub instructions_total: usize,
    pub instructions_projected_covered: usize,
    pub instructions_projected_uncovered: usize,
    pub instructions_unknown: usize,
    pub gaps_total: usize,
    pub gaps_by_kind: BTreeMap<String, usize>,
    #[serde(default)]
    pub stack_worst_case: Option<u64>,
    #[serde(default)]
    pub stack_worst_symbol: Option<usize>,
    #[serde(default)]
    pub stack_unbounded: usize,
    #[serde(default)]
    pub globals: usize,
    #[serde(default)]
    pub data_refs: usize,
    #[serde(default)]
    pub instructions_measured_covered: Option<usize>,
    #[serde(default)]
    pub disagreements: usize,
    #[serde(default)]
    pub outcome_edges_total: usize,
    #[serde(default)]
    pub outcome_edges_measured: Option<usize>,
    /// Instructions the compiler generated without a directly traceable
    /// source statement (A-7 objective 9 headline), by origin.
    #[serde(default)]
    pub instructions_compiler_generated: usize,
    #[serde(default)]
    pub compiler_generated_by_origin: BTreeMap<String, usize>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFile {
    pub id: usize,
    /// Path as recorded in the coverage export (usually absolute).
    pub path: String,
    /// Shortened path for display.
    pub display: String,
    pub sha256: String,
    pub text: Vec<String>,
    /// One entry per source line (index = line - 1).
    pub lines: Vec<LineCov>,
    pub function_ids: Vec<usize>,
    pub lines_total: usize,
    pub lines_covered: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineCov {
    /// `None` = line is not instrumented (comment, blank, declaration...).
    pub count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Function {
    pub id: usize,
    pub name: String,
    pub mangled: String,
    pub file_id: usize,
    pub line_start: u32,
    pub line_end: u32,
    /// Entry count from the instrumented run, summed over instantiations.
    pub count: u64,
    /// How many coverage records (instantiations of this source function, e.g.
    /// the same crate compiled under several `-Cmetadata` hashes, or a generic
    /// instantiated for several types) were merged into this entry.
    #[serde(default = "one")]
    pub instantiations: u32,
    /// Requirement tags found in the comment block above the function
    /// (`@satisfies X`, `@verifies X`).
    pub requirements: Vec<String>,
    /// Ids of the tests that entered this function. Empty when no per-test
    /// exports were given, which is not the same as "no test covers it".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<usize>,
    /// Requirement tags found on specific lines inside the function body
    /// (line -> tags): the object code attributed to those lines implements
    /// them.
    #[serde(default)]
    pub line_requirements: BTreeMap<u32, Vec<String>>,
    pub lines_total: usize,
    pub lines_covered: usize,
    pub branches: Vec<Branch>,
    pub decisions: Vec<Decision>,
    pub complexity: Complexity,
    pub callees: Vec<CallEdge>,
    pub callers: Vec<CallEdge>,
    /// The object code the coverage record describes: the symbol it names,
    /// or, failing an exact match, the one its demangled name resolves to.
    pub object: Option<ObjectFn>,
    /// The function's other object bodies: the variants of a C++ constructor
    /// or destructor the compiler emits beside the one the coverage record
    /// names (the complete-object `C1`/`D1` and the deleting `D0` beside the
    /// base-object `C2`/`D2`). Each is analysed like `object` -- blocks,
    /// gaps, a projected status -- and carries `ObjectFn::variant`. An ELF
    /// alias of the body (same address) is the body, not a variant, and is
    /// not here (#266).
    ///
    /// `object` and these together are the function's object code: every
    /// total over the evidence counts both, each address once, and
    /// `Function::objects` is the one way to walk them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<ObjectFn>,
    #[serde(default)]
    pub symbol_id: Option<usize>,
    #[serde(default)]
    pub data_refs: Vec<DataRef>,
    /// Parameters and locals, with where the object code touches each.
    /// Empty when the binary carries no debug info for them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<Variable>,
    /// Source-language constructs found in the function body (name -> count),
    /// for CAST-12 sample-adequacy tracking.
    #[serde(default)]
    pub constructs: BTreeMap<String, u32>,
    /// Functions the compiler inlined into this one (from DWARF).
    #[serde(default)]
    pub inlined: Vec<Inlined>,
    /// This function's own line counts over `line_start..=line_end` (index
    /// 0 is `line_start`), when it shares its source range with another
    /// function -- a generic's monomorphised copies, a template's
    /// instantiations -- and its figures come from its own coverage records
    /// rather than the file's lines, which are the sum over all of them
    /// (#167). `lines_total`/`lines_covered` are derived from these, and
    /// the derivation check binds them to the file's (#276). Absent for
    /// every other function.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own_lines: Option<Vec<Option<u64>>>,
    /// Covered functions called in the source but absent from the object
    /// code (neither a call nor an inline): (name, function id, line).
    #[serde(default)]
    pub folded_calls: Vec<FoldedCall>,
    /// Indirect (function-pointer) call sites in this function.
    #[serde(default)]
    pub indirect_calls: Vec<IndirectCall>,
    /// Who may call this function through a pointer: symbols that take its
    /// address, data tables holding it, and trace-resolved pointer callers.
    #[serde(default)]
    pub pointer_callers: Vec<PointerCaller>,
    /// Commits that touched this function's lines (indices into
    /// `Evidence.changes.commits`). Empty when `changes` is absent.
    #[serde(default)]
    pub commits: Vec<usize>,
    /// SHA of the commit that introduced this function's lines (whole
    /// history, so it may predate `--since`). Tells "added" from "modified".
    #[serde(default)]
    pub origin: Option<String>,
    /// Set when this function has **no coverage record of its own**: the id
    /// of the covered function its code is written inside. rustc drops the
    /// record of a closure whose body maps no region (`|_| count += 1`), and
    /// clang gives none to the `__invoke` thunk of a lambda converted to a
    /// function pointer, yet their code is in the image, on source lines the
    /// enclosing function's counts cover (#273). Such a function is found by
    /// its symbol's name (`<enclosing>::{closure#N}`, `<enclosing>::$_N::…`)
    /// and placed by the DWARF line table.
    ///
    /// Its `count` is then 0 because nothing counted it, not because it never
    /// ran, and its `lines_total`/`lines_covered` are 0/0 because the counts
    /// on its lines are the enclosing function's. Its object code is analysed,
    /// every instruction is projected `Unknown`, and each run of them carries a
    /// `no-coverage-record` gap. It is never executed, never unexecuted, and
    /// never covered: whether it ran is not established.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrecorded_within: Option<usize>,
}

impl Function {
    /// Every object body of this function: `object` first, then its
    /// `variants`. Anything that counts or lists a function's object code --
    /// instructions, gaps, resolutions -- walks this, so a constructor's
    /// thunks and deleting destructor are neither left out nor counted twice
    /// (#266).
    pub fn objects(&self) -> impl Iterator<Item = &ObjectFn> {
        self.object.iter().chain(self.variants.iter())
    }
}

/// Written out rather than derived: `instantiations` counts the coverage
/// records merged into this entry, so its zero value is 1, not 0. Serde
/// already defaults a missing field to 1, and `Default::default()` giving 0
/// contradicted it -- code that computes `instantiations - 1` underflowed on
/// a default-built function (review of #12).
impl Default for Function {
    fn default() -> Function {
        Function {
            id: 0,
            name: String::new(),
            mangled: String::new(),
            file_id: 0,
            line_start: 0,
            line_end: 0,
            count: 0,
            instantiations: 1,
            requirements: Vec::new(),
            tests: Vec::new(),
            line_requirements: BTreeMap::new(),
            lines_total: 0,
            lines_covered: 0,
            branches: Vec::new(),
            decisions: Vec::new(),
            complexity: Complexity::default(),
            callees: Vec::new(),
            callers: Vec::new(),
            object: None,
            variants: Vec::new(),
            symbol_id: None,
            data_refs: Vec::new(),
            variables: Vec::new(),
            constructs: BTreeMap::new(),
            inlined: Vec::new(),
            own_lines: None,
            folded_calls: Vec::new(),
            indirect_calls: Vec::new(),
            pointer_callers: Vec::new(),
            commits: Vec::new(),
            origin: None,
            unrecorded_within: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndirectCall {
    pub addr: u64,
    pub line: Option<u32>,
    /// Text symbols whose address this function forms (static candidates).
    pub candidates: Vec<PointerTarget>,
    /// Targets observed in the trace for this call site.
    pub measured: Vec<PointerTarget>,
    /// Executions of the call instruction in the trace.
    pub measured_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerTarget {
    pub symbol_id: usize,
    pub name: String,
    pub function_id: Option<usize>,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerCaller {
    /// `code` (a symbol takes the address), `data` (a data table holds it),
    /// or `trace` (an indirect call was observed reaching this function).
    pub kind: String,
    pub symbol_id: Option<usize>,
    pub name: String,
    pub function_id: Option<usize>,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoldedCall {
    pub name: String,
    /// `None` for library / external callees.
    pub function_id: Option<usize>,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inlined {
    pub name: String,
    pub function_id: Option<usize>,
    /// Line in this function where the inlined call was written.
    pub call_line: Option<u32>,
    pub ranges: Vec<(u64, u64)>,
    pub instr_count: usize,
    /// Nesting depth (0 = inlined directly into this function).
    pub depth: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub line: u32,
    pub col: u32,
    pub line_end: u32,
    pub col_end: u32,
    pub true_count: u64,
    pub false_count: u64,
    /// Whether `true_count` and `false_count` really are the true and false
    /// outcomes, or merely the decision's two outcomes in an arbitrary order.
    ///
    /// llvm-cov states the polarity of each branch region, so it is known.
    /// gcov does not: its `fallthrough` flag names a **CFG edge**, and which
    /// edge carries the true outcome depends on the construct — for `if` and
    /// short-circuit operators the fall-through is the true path, for
    /// `while`, `for` and `do`/`while` it is the loop *exit*, so the meaning
    /// inverts. Reading it as polarity would report every loop in a GCC
    /// project backwards (#45).
    ///
    /// Decision coverage does not care: "both outcomes taken" is the same
    /// question either way, and the summary counts it identically. What must
    /// not happen is naming an outcome we cannot identify — the name reaches
    /// a reviewer's justification key and is then part of the record.
    ///
    /// Defaults to `true` so every existing evidence file keeps its meaning.
    #[serde(default = "yes")]
    pub direction_known: bool,
    /// Which decision this is, among several starting on the same line.
    ///
    /// llvm-cov gives every decision a column, so `(line, col)` identifies it.
    /// gcov gives no columns at all: `return a && b;` yields two decisions
    /// that are both "line 2", and keyed on `(line, col)` alone they collapse
    /// into one -- so justifying one untaken condition silently justified the
    /// other, without anybody reviewing it (review of #45).
    ///
    /// This is an **ordinal, not a column**: 0, 1, 2 in the order the
    /// producer listed them. It is stable for a given export and exists only
    /// to keep resolution keys distinct; nothing may slice source with it.
    /// `None` where the column already identifies the decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_ordinal: Option<u32>,
}

fn yes() -> bool {
    true
}

impl Branch {
    /// How this decision is identified inside its function, for a resolution
    /// key: the column where that identifies it, and the ordinal where the
    /// producer gave no columns.
    ///
    /// Rendered as `<line>:<col>` or `<line>:#<ordinal>`, so the two cannot
    /// be confused for one another and an old key cannot accidentally match
    /// a new one.
    pub fn site(&self) -> String {
        match self.decision_ordinal {
            Some(n) => format!("{}:#{n}", self.line),
            None => format!("{}:{}", self.line, self.col),
        }
    }

    /// The letters this branch's two outcomes are keyed by in the
    /// resolutions sidecar.
    ///
    /// `T`/`F` when the polarity is known, and `A`/`B` when it is not. They
    /// are deliberately different letters rather than a flag beside the key:
    /// a justification recorded against `…:T` asserts something about the
    /// *true* outcome, and a key that said `T` where the producer could not
    /// tell true from false would put a claim in the review record that
    /// nothing supports. `A`/`B` says what is actually known -- the decision
    /// has two outcomes and this is one of them (#45).
    pub fn outcome_keys(&self) -> [&'static str; 2] {
        if self.direction_known {
            ["T", "F"]
        } else {
            ["A", "B"]
        }
    }

    /// How to name the two outcomes to a reader.
    pub fn outcome_words(&self) -> [&'static str; 2] {
        if self.direction_known {
            ["true", "false"]
        } else {
            ["one outcome", "the other outcome"]
        }
    }

    /// Counts in the same order as `outcome_keys` / `outcome_words`.
    pub fn outcome_counts(&self) -> [u64; 2] {
        [self.true_count, self.false_count]
    }
}

/// One test run, and what it executed.
///
/// Coverage is otherwise aggregate: one merged profile, one count per line,
/// and no way to say *what* ran a line. DO-178C §6.4.4 defines structural
/// coverage analysis as determining what the **requirements-based test
/// procedures** did not exercise, so evidence that cannot name the tests
/// behind its coverage is asking to be taken on trust (#41).
///
/// Optional in every direction: evidence ingested from a single merged
/// export has no tests, and everything else behaves exactly as before.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRun {
    pub id: usize,
    /// What to call it: the export's file stem unless the caller said so.
    pub name: String,
    /// The coverage export this attribution came from.
    pub path: String,
    /// Functions this test entered, by function id.
    pub functions: Vec<usize>,
    /// Lines this test executed, per file id, as inclusive ranges.
    ///
    /// Ranges rather than one entry per line: a test walks contiguous
    /// regions, so a project with many tests stays a readable file rather
    /// than N x M integers.
    pub lines: BTreeMap<usize, Vec<(u32, u32)>>,
}

/// One MC/DC decision (a boolean expression with >= 1 condition).
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub line: u32,
    pub col: u32,
    pub line_end: u32,
    pub col_end: u32,
    pub true_count: u64,
    pub false_count: u64,
    pub conditions: Vec<Condition>,
    pub vectors: Vec<TestVector>,
    /// True when every condition has an independence pair.
    pub mcdc_covered: bool,
    /// Disambiguating ordinal when multiple decisions share the same
    /// `(line, col)` site (for example, macro expansions). `None` when
    /// `(line, col)` already identifies the decision uniquely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_ordinal: Option<u32>,
}

impl Decision {
    /// How this decision is identified inside its function, for a resolution
    /// key: `<line>:<col>` when unique, or `<line>:#<ordinal>` when multiple
    /// decisions share a site. Mirrors [`Branch::site`].
    pub fn site(&self) -> String {
        match self.decision_ordinal {
            Some(n) => format!("{}:#{n}", self.line),
            None => format!("{}:{}", self.line, self.col),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub index: usize,
    pub line: u32,
    pub col: u32,
    pub true_count: u64,
    pub false_count: u64,
    pub independence_covered: bool,
    /// 1-based indices into `Decision::vectors` forming an independence pair,
    /// when one could be identified from the executed vectors.
    pub pair: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestVector {
    /// One entry per condition; `None` = not evaluated (short-circuited).
    pub values: Vec<Option<bool>>,
    pub result: bool,
    pub executed: bool,
    /// Ids of the tests that produced this exact vector (matched on `values`
    /// and `result`, across every per-test export ingested), sorted and
    /// deduplicated. Empty means unattributed -- no per-test export was
    /// ingested, or none of them produced this vector -- and a consumer must
    /// fall back to naming the vector by number rather than guess (#48).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Complexity {
    /// E - N + 2 over the object-code CFG. `None` when no object code.
    pub cyclomatic_object: Option<u32>,
    pub decisions: u32,
    pub conditions: u32,
    pub max_conditions_per_decision: u32,
    pub object_blocks: u32,
    pub object_edges: u32,
    pub object_instructions: u32,
    pub object_conditional_branches: u32,
    pub calls_out: u32,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Exercised {
    Yes,
    No,
    #[default]
    Unknown,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallEdge {
    /// `None` when the target is not a function we have source coverage for
    /// (libc, compiler helper, ...).
    pub function_id: Option<usize>,
    pub symbol: String,
    pub sites: Vec<CallSite>,
    /// Projected: caller line executed AND callee entry count > 0.
    pub exercised: Exercised,
    /// Number of times the callee is named with `(` in this function's
    /// source; more object sites than this means the call was duplicated
    /// (loop unrolling, tail duplication), fewer means some were folded.
    #[serde(default)]
    pub source_calls: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSite {
    pub addr: u64,
    pub line: Option<u32>,
    #[serde(default)]
    pub measured: Option<u64>,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Covered,
    Uncovered,
    /// The safe default: a record built without an explicit status claims
    /// nothing about coverage.
    #[default]
    Unknown,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectFn {
    pub symbol: String,
    /// Set on one of `Function::variants`: the Itanium C++ ABI code of the
    /// constructor or destructor variant this body is (`C1`, `C3`, `CI1`,
    /// `D0`, `D1`, ...). `None` on `Function::object`. A variant has no
    /// coverage counter of its own -- clang instruments only the base
    /// variant -- so its projected status is not read from line counts
    /// (#266).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub address: u64,
    pub size: u64,
    pub blocks: Vec<Block>,
    pub gaps: Vec<Gap>,
    pub instr_total: usize,
    pub instr_projected_covered: usize,
    pub instr_projected_uncovered: usize,
    pub instr_unknown: usize,
    #[serde(default)]
    pub instr_measured_covered: Option<usize>,
    #[serde(default)]
    pub disagreements: Vec<Disagreement>,
    /// Outcome edges of conditional branches (taken + fall-through).
    #[serde(default)]
    pub outcome_edges_total: usize,
    /// ... of which the trace followed at least once.
    #[serde(default)]
    pub outcome_edges_measured: Option<usize>,
    /// Blocks with no incoming edge (other than the entry block): unreachable
    /// by direct control flow inside the function.
    #[serde(default)]
    pub unreachable_blocks: Vec<usize>,
    /// Instructions not directly traceable to a source statement of this
    /// function (any `origin` other than inlined), by origin.
    #[serde(default)]
    pub compiler_generated: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BlockKind {
    Entry,
    #[default]
    Normal,
    Exit,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub id: usize,
    pub address: u64,
    pub kind: BlockKind,
    pub instrs: Vec<Instr>,
    pub succs: Vec<Edge>,
    pub lines: Vec<u32>,
    pub projected: Status,
    #[serde(default)]
    pub measured: Option<u64>,
    /// Set when the projected status was refined by branch-outcome inference
    /// rather than taken from line counts alone; explains the reasoning.
    #[serde(default)]
    pub note: Option<String>,
    /// Source lines this block is believed to implement although no
    /// instruction is attributed to them (set by branch-outcome inference).
    #[serde(default)]
    pub inferred_lines: Vec<u32>,
    /// Set when the block ends in a call the CFG builder knows does not
    /// return, so it was given no fall-through successor. The string is the
    /// rule that established it, in the words a reviewer needs to judge it
    /// — which attribute, on which callee, from which input — because
    /// "there is no edge here" is a claim about control flow that the
    /// instruction stream alone does not support (#210).
    #[serde(default)]
    pub diverges: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EdgeKind {
    #[default]
    Fallthrough,
    Taken,
    Jump,
    /// Through a switch jump table the object code was read for (#171).
    Indirect,
    /// From a block with a call that may throw to the landing pad the
    /// unwinder goes to, as the function's exception table says (#165).
    Exception,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub to: usize,
    pub kind: EdgeKind,
    pub projected: Status,
    /// Machine condition under which this edge is followed (e.g. `w8 == 0`,
    /// `ne`, `lt`), for edges leaving a conditional branch.
    #[serde(default)]
    pub label: Option<String>,
    /// Times this edge was followed in the imported trace (block-to-block
    /// transitions). `None` when no trace was imported.
    #[serde(default)]
    pub measured: Option<u64>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instr {
    pub addr: u64,
    pub mnemonic: String,
    pub operands: String,
    pub line: Option<u32>,
    pub file_id: Option<usize>,
    pub is_stmt: bool,
    pub projected: Status,
    pub flags: Vec<InstrFlag>,
    pub gap_ids: Vec<usize>,
    /// Executions in the imported trace: `Some(0)` for an instruction the
    /// trace never executed, `None` only when no trace was imported (#325).
    #[serde(default)]
    pub measured: Option<u64>,
    /// Index into `Function::inlined` when this instruction belongs to an
    /// inlined callee (innermost).
    #[serde(default)]
    pub inlined: Option<usize>,
    /// `None` = directly traceable to a source statement of this function.
    /// Otherwise why the compiler emitted it: `prologue`, `epilogue`,
    /// `stack-protector`, `runtime-check`, `line-0`, `no-line`,
    /// `padding`, `inlined:<callee>`.
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstrFlag {
    Call,
    IndirectCall,
    Branch,
    CondBranch,
    IndirectBranch,
    Return,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GapKind {
    /// Instruction has no DWARF line-table entry.
    #[default]
    NoSourceLine,
    /// DWARF maps the instruction to line 0 (compiler-generated).
    LineZero,
    /// A source line has more conditional branches in the object code than
    /// decisions/conditions in the coverage mapping.
    ExtraObjectBranch,
    /// A source decision compiled to predicated/conditional-select code with
    /// no object-level branch: branch coverage has nothing to map onto.
    PredicatedDecision,
    /// Call to a symbol that is not part of the covered source set
    /// (compiler helper, runtime, libc).
    HelperCall,
    /// Indirect branch (jump table or function pointer).
    IndirectBranch,
    /// Instruction maps to a source line that has no coverage count.
    NotInstrumented,
    /// A block whose instructions borrow source lines that do not describe
    /// it (merged returns, common epilogues), so its status is inferred from
    /// control flow: reachable only through a never-taken branch outcome, it
    /// is projected uncovered where line counts alone would mark it covered;
    /// entered unconditionally from a block that ran, it is projected
    /// covered where its borrowed line reads 0 (#270).
    SharedLine,
    /// A call written in the source to a covered function that appears in
    /// the object code neither as a call nor as an inlined subroutine: the
    /// compiler evaluated it away (constant folding / interprocedural
    /// optimisation). Nothing of the callee executes on this path.
    FoldedCall,
    /// Object code of a function with no coverage record of its own (a
    /// closure or lambda thunk the compiler gave none, #273): its source
    /// lines' counts belong to the enclosing function, so nothing says
    /// whether this code ran.
    NoCoverageRecord,
    /// A compiler-emitted variant of a C++ constructor or destructor (`C1`,
    /// `D1`, the deleting `D0`, ...) that has no coverage counter of its own
    /// while the body it belongs to ran: source coverage cannot say whether
    /// this variant ran, so its instructions are projected unknown and it
    /// is object code a reviewer has to account for (#266).
    UncountedVariant,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    pub id: usize,
    pub kind: GapKind,
    pub addr_start: u64,
    pub addr_end: u64,
    pub line: Option<u32>,
    pub description: String,
    /// Stable key for resolution annotations: `<symbol>@<kind>@<addr_start:x>`.
    pub key: String,
    /// For `helper-call`: what kind of code the callee is
    /// (`runtime-check`, `arith-helper`, `memory-helper`, `core-lib`, `libc`,
    /// `project-uncovered`, `unknown`).
    #[serde(default)]
    pub class: Option<String>,
}

/// Sidecar file written by the viewer, kept under configuration management
/// separately from the evidence (which is never modified after ingest).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolutions {
    pub schema: String,
    pub entries: Vec<Resolution>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    /// Gap key, or `line:<file_id>:<line>`, `branch:<fn_id>:<line>:<col>`,
    /// `cond:<fn_id>:<line>:<col>:<index>`, `fn:<fn_id>`.
    pub key: String,
    /// DO-178C 6.4.4.3 category: `a` test shortcoming, `b` requirement
    /// inadequacy, `c` extraneous/dead code, `d1`/`d2` deactivated code,
    /// `analysis` for compiler-generated code justified by analysis
    /// (6.4.4.2.b).
    pub category: String,
    pub justification: String,
    pub author: String,
    pub date: String,
}

pub const RESOLUTIONS_SCHEMA: &str = "butterfly-resolutions/1";

impl Resolutions {
    /// A sidecar written by a different version carries entries this build
    /// may not understand. Reading it as an empty set would drop reviewer
    /// decisions from a verification report, so say what was found instead.
    pub fn check_schema(&self) -> Result<(), String> {
        if self.schema == RESOLUTIONS_SCHEMA {
            return Ok(());
        }
        Err(format!("unsupported resolutions schema {:?} (expected {RESOLUTIONS_SCHEMA})", self.schema))
    }
}

/// The longest construct name `Evidence::validate` accepts. Ingest's longest
/// is 13 bytes (`shift/bitwise`, `checked-arith`); this leaves room for more.
pub const MAX_CONSTRUCT_NAME: usize = 64;

/// Whether `k` has the shape of a construct name ingest writes: non-empty,
/// bounded, printable ASCII without whitespace or the characters that open
/// markup or quote an attribute (#337). `&` stays allowed: `&&` is one.
pub fn construct_name_ok(k: &str) -> bool {
    !k.is_empty() && k.len() <= MAX_CONSTRUCT_NAME && k.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'<' | b'>' | b'"' | b'\'' | b'`'))
}

impl Evidence {
    /// Check every index in the database before anything renders it.
    ///
    /// An evidence file is data from outside the process (it may be handed
    /// over by a supplier, or simply be truncated). Rendering indexes into
    /// `files`, `functions`, `symbols`, `globals` and per-function `blocks`;
    /// an out-of-range index would panic mid-render. Validate once, on load,
    /// and fail with a message that names the offending field.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(r) = &self.provenance.source_revision {
            r.check()?;
        }
        let (nf, nfn, nsym, ng) = (self.files.len(), self.functions.len(), self.symbols.len(), self.globals.len());
        // Macros, not closures, so the message is built only when the check
        // fails. As closures taking `&str`, every call site had to `format!`
        // its label first -- and these run per instruction and per gap id, so
        // a document with millions of instructions allocated millions of
        // Strings describing errors that were not happening. Profiling a 50k
        // function evidence file put `validate` at 15.9% of samples with
        // another 6.7% in formatting and 20.9% in the allocator; the labels
        // were most of it.
        macro_rules! chk {
            ($what:expr, $i:expr, $n:expr) => {{
                let (i, n) = ($i, $n);
                if i >= n {
                    return Err(format!("{}: index {i} out of range (len {n})", $what));
                }
            }};
        }
        if self.schema != SCHEMA {
            return Err(format!("unsupported schema {:?} (expected {SCHEMA})", self.schema));
        }
        // Counter consistency: covered can never exceed total. The viewer and
        // the worklist subtract these pairs, so a document where they are
        // inverted turns into an underflow or a negative percentage the
        // moment it is consumed -- it must stop here, at the door, with the
        // field named (review of #44).
        macro_rules! pair {
            ($what:expr, $covered:expr, $total:expr) => {{
                let (covered, total) = ($covered, $total);
                if covered > total {
                    return Err(format!("{}: {covered} covered of {total} total — covered exceeds total", $what));
                }
            }};
        }
        pair!("summary.lines", self.summary.lines_covered, self.summary.lines_total);
        pair!("summary.branches", self.summary.branches_covered, self.summary.branches_total);
        pair!("summary.decisions", self.summary.decisions_mcdc_covered, self.summary.decisions_total);
        pair!("summary.conditions", self.summary.conditions_independence_covered, self.summary.conditions_total);
        pair!("summary.functionsExecuted", self.summary.functions_executed, self.summary.functions);
        for (i, fl) in self.files.iter().enumerate() {
            pair!(format!("files[{i}] `{}` lines", fl.display), fl.lines_covered, fl.lines_total);
        }
        for (i, f) in self.functions.iter().enumerate() {
            pair!(format!("functions[{i}] `{}` lines", f.name), f.lines_covered, f.lines_total);
        }
        for (i, f) in self.functions.iter().enumerate() {
            let at = |w: &str| format!("functions[{i}] `{}` {w}", f.name);
            chk!(at("fileId"), f.file_id, nf);
            // A source range is iterated by consumers -- the requirements
            // matrix walks it line by line -- so line zero underflows the
            // 1-based conversion and an oversized end is an attacker-chosen
            // loop count in `summary` and `report` alike (review of #42).
            let n_lines = self.files[f.file_id].text.len();
            if f.line_start == 0 || f.line_start > f.line_end || f.line_end as usize > n_lines {
                return Err(format!("{}: source range {}..={} is not within the file's {n_lines} line(s)", at("lines"), f.line_start, f.line_end));
            }
            if let Some(s) = f.symbol_id {
                chk!(at("symbolId"), s, nsym);
            }
            // Construct names are a small vocabulary ingest chooses (`if`,
            // `&&`, `shift/bitwise`, ...), printed as table cells in the
            // report and the viewer. A name outside that shape is not one
            // ingest wrote, so it is refused here rather than trusted to every
            // consumer's escaping (#337).
            if let Some(k) = f.constructs.keys().find(|k| !construct_name_ok(k)) {
                return Err(format!("{}: {k:?} is not a construct name (1-{MAX_CONSTRUCT_NAME} printable ASCII characters, none of < > \" ' `)", at("constructs")));
            }
            // the viewer links to it (#273)
            if let Some(w) = f.unrecorded_within {
                chk!(at("unrecordedWithin"), w, nfn);
            }
            for d in &f.data_refs {
                chk!(at("dataRefs.globalId"), d.global_id, ng);
            }
            for e in f.callees.iter().chain(f.callers.iter()) {
                if let Some(t) = e.function_id {
                    chk!(at("callEdge.functionId"), t, nfn);
                }
            }
            for x in &f.inlined {
                if let Some(t) = x.function_id {
                    chk!(at("inlined.functionId"), t, nfn);
                }
            }
            for c in &f.folded_calls {
                if let Some(t) = c.function_id {
                    chk!(at("foldedCalls.functionId"), t, nfn);
                }
            }
            for ic in &f.indirect_calls {
                for t in ic.candidates.iter().chain(ic.measured.iter()) {
                    chk!(at("indirectCalls.symbolId"), t.symbol_id, nsym);
                    if let Some(x) = t.function_id {
                        chk!(at("indirectCalls.functionId"), x, nfn);
                    }
                }
            }
            for pc in &f.pointer_callers {
                if let Some(x) = pc.symbol_id {
                    chk!(at("pointerCallers.symbolId"), x, nsym);
                }
                if let Some(x) = pc.function_id {
                    chk!(at("pointerCallers.functionId"), x, nfn);
                }
            }
            for o in f.objects() {
                let nb = o.blocks.len();
                for b in &o.blocks {
                    for e in &b.succs {
                        chk!(at("block.succs.to"), e.to, nb);
                    }
                    for ins in &b.instrs {
                        if let Some(x) = ins.file_id {
                            chk!(at("instr.fileId"), x, nf);
                        }
                        if let Some(x) = ins.inlined {
                            chk!(at("instr.inlined"), x, f.inlined.len());
                        }
                        for g in &ins.gap_ids {
                            chk!(at("instr.gapIds"), *g, o.gaps.len());
                        }
                    }
                }
                for u in &o.unreachable_blocks {
                    chk!(at("unreachableBlocks"), *u, nb);
                }
            }
        }
        for (i, s) in self.symbols.iter().enumerate() {
            let at = |w: &str| format!("symbols[{i}] `{}` {w}", s.name);
            if let Some(f) = s.function_id {
                chk!(at("functionId"), f, nfn);
            }
            for c in &s.callees {
                chk!(at("callees"), *c, nsym);
            }
            for c in &s.callers {
                chk!(at("callers"), *c, nsym);
            }
            // `usize::MAX` is the "held in a data table" sentinel
            for t in s.address_taken_by.iter().filter(|t| **t != usize::MAX) {
                chk!(at("addressTakenBy"), *t, nsym);
            }
            for p in &s.stack.path {
                chk!(at("stack.path"), *p, nsym);
            }
            for d in &s.data_refs {
                chk!(at("dataRefs.globalId"), d.global_id, ng);
            }
        }
        for (i, g) in self.globals.iter().enumerate() {
            let at = |w: &str| format!("globals[{i}] `{}` {w}", g.name);
            for f in g.readers.iter().chain(g.writers.iter()).chain(g.address_takers.iter()) {
                chk!(at("accessor functionId"), *f, nfn);
            }
            for x in g.reader_symbols.iter().chain(g.writer_symbols.iter()).chain(g.address_taker_symbols.iter()) {
                chk!(at("accessor symbolId"), *x, nsym);
            }
        }
        for f in &self.files {
            if f.lines.len() > f.text.len() {
                return Err(format!("files `{}`: {} coverage lines for {} source lines", f.display, f.lines.len(), f.text.len()));
            }
        }

        // `files[].functionIds` is the reverse of `functions[].fileId`, so it
        // can be derived rather than spot-checked -- and deriving catches
        // every way the two can disagree at once. Checking only the entries
        // that happened to be present left two of them accepted: a function
        // whose file never lists it, which the viewer then silently hides,
        // and one listed twice, which it renders twice (review of #11).
        //
        // `fileId` is range-checked in the loop above, so indexing is safe
        // here. Order is part of it: the viewer renders a file's functions in
        // this order, and ingest emits them ascending.
        let mut owned: Vec<Vec<usize>> = vec![Vec::new(); nf];
        for (k, f) in self.functions.iter().enumerate() {
            owned[f.file_id].push(k);
        }
        for (i, f) in self.files.iter().enumerate() {
            if f.function_ids == owned[i] {
                continue;
            }
            // Both sides as sets first. Asking `Vec::contains` inside a loop
            // over the other vector is quadratic, and this is the path a
            // malformed file takes -- a complete list in reverse order is the
            // worst case and reaches it on every entry, which turns an
            // untrusted document into CPU to burn (review of #11).
            let listed: std::collections::BTreeSet<usize> = f.function_ids.iter().copied().collect();
            let owns: std::collections::BTreeSet<usize> = owned[i].iter().copied().collect();
            let name = |k: &usize| self.functions.get(*k).map_or("?".into(), |x| format!("`{}`", x.name));
            let missing: Vec<String> = owned[i].iter().filter(|k| !listed.contains(k)).map(|k| format!("{k} {}", name(k))).collect();
            let extra: Vec<String> = f
                .function_ids
                .iter()
                .filter(|k| !owns.contains(k))
                .map(|k| match self.functions.get(*k) {
                    Some(x) => format!("{k} `{}`, which is in files[{}]", x.name, x.file_id),
                    None => format!("{k}, which is not a function at all (there are {nfn})"),
                })
                .collect();
            let mut seen = std::collections::BTreeSet::new();
            let dup: Vec<String> = f.function_ids.iter().filter(|k| !seen.insert(**k)).map(|k| format!("{k} {}", name(k))).collect();
            let mut why = Vec::new();
            if !missing.is_empty() {
                why.push(format!("does not list {} (which name this file, so the viewer would hide them)", missing.join(", ")));
            }
            if !extra.is_empty() {
                why.push(format!("lists {}", extra.join("; ")));
            }
            if !dup.is_empty() {
                why.push(format!("lists {} more than once (the viewer would show them twice)", dup.join(", ")));
            }
            if why.is_empty() {
                // complete and unique, so the only thing left is the order;
                // name the first place they part company rather than printing
                // two lists that may be enormous
                let at = f.function_ids.iter().zip(&owned[i]).position(|(a, b)| a != b).unwrap_or(0);
                why.push(format!(
                    "lists them in a different order: position {at} is {} but this file's functions in order are {}",
                    f.function_ids.get(at).map_or("nothing".into(), |k| format!("{k} {}", name(k))),
                    owned[i].get(at).map_or("nothing".into(), |k| format!("{k} {}", name(k)))
                ));
            }
            return Err(format!("files[{i}] `{}` functionIds {}", f.display, why.join("; ")));
        }

        // Positional ids. Every consumer indexes these arrays with the id --
        // `S.ev.functions[id]` in the viewer, `chk` against the length here --
        // so an id that disagrees with its position silently addresses the
        // wrong record. Serialising the id at all is then a redundancy that
        // has to be checked rather than trusted (#11).
        let positional = |what: &str, ids: &mut dyn Iterator<Item = usize>| -> Result<(), String> {
            for (i, id) in ids.enumerate() {
                if id != i {
                    return Err(format!("{what}[{i}]: id is {id}, but these ids are positions and every consumer indexes with them"));
                }
            }
            Ok(())
        };
        positional("files", &mut self.files.iter().map(|x| x.id))?;
        positional("functions", &mut self.functions.iter().map(|x| x.id))?;
        positional("symbols", &mut self.symbols.iter().map(|x| x.id))?;
        positional("globals", &mut self.globals.iter().map(|x| x.id))?;
        for (i, f) in self.functions.iter().enumerate() {
            for o in f.objects() {
                positional(&format!("functions[{i}] `{}` {} blocks", f.name, o.symbol), &mut o.blocks.iter().map(|x| x.id))?;
                positional(&format!("functions[{i}] `{}` {} gaps", f.name, o.symbol), &mut o.gaps.iter().map(|x| x.id))?;
            }
        }
        if let Some(im) = &self.image {
            for d in &im.dead_candidates {
                chk!("image.deadCandidates.symbolId", d.symbol_id, nsym);
                if let Some(f) = d.function_id {
                    chk!("image.deadCandidates.functionId", f, nfn);
                }
            }
            for u in &im.text_symbols_uncovered {
                chk!("image.textSymbolsUncovered", *u, nsym);
            }
        }
        if let Some(c) = &self.changes {
            if c.head.bytes().any(|b| b.is_ascii_control()) {
                return Err("changes.head contains a control character".into());
            }
            if let Some(s) = &c.since {
                if s.is_empty() || s.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace()) {
                    return Err("changes.since is empty or contains whitespace or a control character".into());
                }
            }
            let (nc, nt) = (c.commits.len(), c.tickets.len());
            for (i, f) in self.functions.iter().enumerate() {
                for x in &f.commits {
                    chk!(format!("functions[{i}] `{}` commits", f.name), *x, nc);
                }
            }
            for (i, cm) in c.commits.iter().enumerate() {
                for t in &cm.tickets {
                    chk!(format!("changes.commits[{i}].tickets"), *t, nt);
                }
                for f in &cm.functions {
                    chk!(format!("changes.commits[{i}].functions"), *f, nfn);
                }
            }
            for (i, t) in c.tickets.iter().enumerate() {
                for x in &t.commits {
                    chk!(format!("changes.tickets[{i}].commits"), *x, nc);
                }
                for f in &t.functions {
                    chk!(format!("changes.tickets[{i}].functions"), *f, nfn);
                }
            }
            for u in &c.unticketed {
                chk!("changes.unticketed", *u, nc);
            }
            for cf in &c.files {
                chk!("changes.files.fileId", cf.file_id, nf);
            }
            let nd = c.deleted.len();
            for (i, d) in c.deleted.iter().enumerate() {
                if let Some(x) = d.commit {
                    chk!(format!("changes.deleted[{i}].commit"), x, nc);
                }
            }
            for (i, t) in c.tickets.iter().enumerate() {
                for f in t.added.iter().chain(t.modified.iter()) {
                    chk!(format!("changes.tickets[{i}].added/modified"), *f, nfn);
                }
                for d in &t.deleted {
                    chk!(format!("changes.tickets[{i}].deleted"), *d, nd);
                }
            }
            if let Some(im) = &c.impact {
                let no = im.obligations.len();
                for f in &im.functions_changed {
                    chk!("changes.impact.functionsChanged", *f, nfn);
                }
                for (i, o) in im.obligations.iter().enumerate() {
                    if let Some(f) = o.function_id {
                        chk!(format!("changes.impact.obligations[{i}].functionId"), f, nfn);
                    }
                    if let Some(t) = o.ticket {
                        chk!(format!("changes.impact.obligations[{i}].ticket"), t, nt);
                    }
                    if let Some(t) = o.target_function {
                        chk!(format!("changes.impact.obligations[{i}].targetFunction"), t, nfn);
                    }
                    if let Some(x) = o.commit {
                        chk!(format!("changes.impact.obligations[{i}].commit"), x, nc);
                    }
                }
                for (i, t) in im.by_ticket.iter().enumerate() {
                    chk!(format!("changes.impact.byTicket[{i}].ticket"), t.ticket, nt);
                    for x in &t.obligations {
                        chk!(format!("changes.impact.byTicket[{i}].obligations"), *x, no);
                    }
                }
            }
        } else if let Some((i, f)) = self.functions.iter().enumerate().find(|(_, f)| !f.commits.is_empty()) {
            return Err(format!("functions[{i}] `{}` has commits but evidence has no `changes`", f.name));
        }
        if let Some(w) = self.summary.stack_worst_symbol {
            chk!("summary.stackWorstSymbol", w, nsym);
        }
        // --- per-test attribution (#41) ----------------------------------
        // Every index here reaches into another array the viewer will index
        // without checking, and the reverse index is the part a hand-edited
        // or partially-written document gets wrong: ids that skip a number,
        // a function claiming a test that does not claim it back.
        let nt = self.tests.len();
        for (i, t) in self.tests.iter().enumerate() {
            let at = |w: &str| format!("tests[{i}] `{}` {w}", t.name);
            if t.id != i {
                return Err(format!("{}: id is {} but its position is {i}", at("id"), t.id));
            }
            for &f in &t.functions {
                chk!(at("functions"), f, nfn);
            }
            if t.functions.windows(2).any(|w| w[0] >= w[1]) {
                return Err(at("functions: not sorted and unique"));
            }
            for (&fid, ranges) in &t.lines {
                chk!(at("lines.fileId"), fid, nf);
                let n = self.files[fid].text.len() as u32;
                let mut prev_end = 0u32;
                for &(a, b) in ranges {
                    if a == 0 || a > b {
                        return Err(format!("{}: range {a}..={b} is not a line range", at("lines")));
                    }
                    if b > n {
                        return Err(format!("{}: range {a}..={b} runs past the file's {n} line(s)", at("lines")));
                    }
                    if a <= prev_end {
                        return Err(format!("{}: range {a}..={b} overlaps or precedes the one before it", at("lines")));
                    }
                    prev_end = b;
                }
            }
        }
        for (i, f) in self.functions.iter().enumerate() {
            if f.tests.windows(2).any(|w| w[0] >= w[1]) {
                return Err(format!("functions[{i}] `{}` tests: not sorted and unique", f.name));
            }
            for &t in &f.tests {
                chk!(format!("functions[{i}] `{}` tests", f.name), t, nt);
                if self.tests[t].functions.binary_search(&i).is_err() {
                    return Err(format!("functions[{i}] `{}` claims test `{}`, but that test does not list it", f.name, self.tests[t].name));
                }
            }
            for (di, d) in f.decisions.iter().enumerate() {
                let nv = d.vectors.len();
                for (ci, c) in d.conditions.iter().enumerate() {
                    if let Some((a, b)) = c.pair {
                        if a == 0 || a > nv || b == 0 || b > nv {
                            return Err(format!("functions[{i}] `{}` decisions[{di}].conditions[{ci}].pair: ({a}, {b}) out of range for {nv} vector(s) (1-based)", f.name));
                        }
                    }
                }
                for (vi, v) in d.vectors.iter().enumerate() {
                    if v.tests.windows(2).any(|w| w[0] >= w[1]) {
                        return Err(format!("functions[{i}] `{}` decisions[{di}].vectors[{vi}].tests: not sorted and unique", f.name));
                    }
                    for &t in &v.tests {
                        chk!(format!("functions[{i}] `{}` decisions[{di}].vectors[{vi}].tests", f.name), t, nt);
                    }
                }
            }
        }
        for t in &self.tests {
            for &f in &t.functions {
                if self.functions[f].tests.binary_search(&t.id).is_err() {
                    return Err(format!("test `{}` lists `{}`, but that function does not claim it", t.name, self.functions[f].name));
                }
            }
        }

        Ok(())
    }

    /// Recompute every decision-authoritative summary field from the primitive
    /// records and reject the first that disagrees with the stored summary
    /// (#249).
    ///
    /// The gates (`check --min-lines/--min-outcomes/--min-mcdc/--max-unresolved-gaps`)
    /// and the report read `summary`, but it is a *derived* cache: a
    /// hand-edited or supplier-provided evidence whose summary understates
    /// gaps or overstates coverage would drive a passing gate while the
    /// functions it was derived from say otherwise. Recomputing the fields
    /// that back a decision -- exactly as ingest builds them -- and refusing
    /// the first mismatch binds the number a gate reads to the records it is
    /// supposed to summarise. Fields that are not pure functions of the
    /// files/functions here (stack, globals, measured/trace, compiler-generated
    /// origins, data references) are left to their own validation.
    pub fn check_summary_derivation(&self) -> Result<(), String> {
        let s = &self.summary;
        // (label, stored, recomputed) — checked in order, first mismatch wins.
        let want = |label: &str, stored: usize, got: usize| -> Result<(), String> {
            if stored != got {
                return Err(format!("summary.{label} is {stored}, but the records derive {got}: the summary does not match the evidence it is drawn from"));
            }
            Ok(())
        };
        let add = |acc: &mut usize, delta: usize, label: &str| -> Result<(), String> {
            *acc = acc.checked_add(delta).ok_or_else(|| format!("summary.{label} overflows usize while summing records"))?;
            Ok(())
        };
        want("files", s.files, self.files.len())?;
        want("functions", s.functions, self.functions.len())?;
        let (mut lines_total, mut lines_covered) = (0usize, 0usize);
        for f in &self.files {
            // `f.lines_total`/`f.lines_covered` are themselves caches ingest
            // derives from the line records, so summing them only checks two
            // cache layers agree. Recompute from the primitives -- the line
            // records -- and bind the file's own cache to them first, so an
            // internally inconsistent file cannot reach the line gate (#249).
            let ft = f.lines.iter().filter(|l| l.count.is_some()).count();
            let fc = f.lines.iter().filter(|l| l.count.is_some_and(|c| c > 0)).count();
            if f.lines_total != ft || f.lines_covered != fc {
                return Err(format!(
                    "file {} reports {}/{} lines covered/total, but its {} line record(s) derive {}/{}: a cached file total disagrees with the coverage it is drawn from",
                    f.display,
                    f.lines_covered,
                    f.lines_total,
                    f.lines.len(),
                    fc,
                    ft
                ));
            }
            add(&mut lines_total, ft, "linesTotal")?;
            add(&mut lines_covered, fc, "linesCovered")?;
        }
        want("linesTotal", s.lines_total, lines_total)?;
        want("linesCovered", s.lines_covered, lines_covered)?;
        let (mut fx, mut bt, mut bc, mut dt, mut dm, mut ct, mut cc) = (0usize, 0, 0, 0, 0, 0, 0);
        let (mut it, mut ipc, mut ipu, mut iu, mut gaps) = (0usize, 0, 0, 0, 0);
        let mut unrecorded = 0usize;
        for f in &self.functions {
            // Function.lines_total/lines_covered are caches gate::regressions()
            // reads per function (regression-function). Derive them from the
            // file's line records over the function's range -- ingest's own
            // rule -- and reject a mismatch, so a per-function counter cannot
            // drive a wrong regression verdict while the file/global totals
            // look right (#249).
            //
            // A function with no coverage record has no line figures of its
            // own -- the counts on its lines are the enclosing function's --
            // and nothing may be projected from them (#273). Hold it to
            // exactly that: an evidence file claiming such a function
            // executed, covered a line, or projected an instruction either
            // way is claiming what no record showed.
            if let Some(within) = f.unrecorded_within {
                if self.functions.get(within).is_none_or(|w| w.unrecorded_within.is_some()) {
                    return Err(format!(
                        "function `{}` has no coverage record and names function {within} as the one it is written inside, which is not a function with a record",
                        f.name
                    ));
                }
                let has_projected = f.objects().any(|o| o.instr_projected_covered != 0 || o.instr_projected_uncovered != 0);
                let projected: usize = f.objects().fold(0usize, |acc, o| acc.saturating_add(o.instr_projected_covered).saturating_add(o.instr_projected_uncovered));
                if f.count != 0 || f.lines_total != 0 || f.lines_covered != 0 || !f.branches.is_empty() || !f.decisions.is_empty() || has_projected {
                    return Err(format!(
                        "function `{}` has no coverage record, yet reports a count of {}, {}/{} lines, {} branch(es), {} decision(s) and {projected} projected instruction(s): nothing can have established any of them",
                        f.name,
                        f.count,
                        f.lines_covered,
                        f.lines_total,
                        f.branches.len(),
                        f.decisions.len()
                    ));
                }
                unrecorded += 1;
            } else if let (Some(file), Some(own)) = (self.files.get(f.file_id), &f.own_lines) {
                // A copy sharing its range with another: its figures come
                // from its own counts, and those are bound to the file's --
                // a line a copy ran ran in the file, and a line a copy can
                // execute is executable there. The file's lines are the sum
                // over the copies, so neither can be exceeded (#276).
                let span = (f.line_end - f.line_start) as usize + 1;
                if own.len() != span {
                    return Err(format!("function `{}` carries {} own line counts for lines {}-{}, which is {span} line(s)", f.name, own.len(), f.line_start, f.line_end));
                }
                for (k, c) in own.iter().enumerate() {
                    let line = f.line_start as usize + k;
                    let in_file = file.lines.get(line - 1).and_then(|l| l.count);
                    if (c.is_some() && in_file.is_none()) || (matches!(c, Some(n) if *n > 0) && !matches!(in_file, Some(n) if n > 0)) {
                        return Err(format!(
                            "function `{}` says line {line} {} but its file's line records say {}: a function's own counts cannot exceed the file's",
                            f.name,
                            if matches!(c, Some(n) if *n > 0) { "ran" } else { "is executable" },
                            match in_file {
                                None => "it is not executable".to_string(),
                                Some(n) => format!("it ran {n} time(s)"),
                            }
                        ));
                    }
                }
                let ft = own.iter().filter(|c| c.is_some()).count();
                let fc = own.iter().filter(|c| matches!(c, Some(n) if *n > 0)).count();
                if f.lines_total != ft || f.lines_covered != fc {
                    return Err(format!(
                        "function `{}` reports {}/{} lines covered/total, but its own line counts derive {fc}/{ft}: a cached per-function total disagrees with the coverage it is drawn from",
                        f.name, f.lines_covered, f.lines_total
                    ));
                }
            } else if let Some(file) = self.files.get(f.file_id) {
                let (mut ft, mut fc) = (0usize, 0usize);
                for l in f.line_start..=f.line_end {
                    let Some(idx) = (l as usize).checked_sub(1) else { continue };
                    if let Some(c) = file.lines.get(idx) {
                        if c.count.is_some() {
                            ft += 1;
                        }
                        if matches!(c.count, Some(n) if n > 0) {
                            fc += 1;
                        }
                    }
                }
                if f.lines_total != ft || f.lines_covered != fc {
                    return Err(format!(
                        "function `{}` reports {}/{} lines covered/total, but its file's line records over lines {}-{} derive {}/{}: a cached per-function total disagrees with the coverage it is drawn from",
                        f.name, f.lines_covered, f.lines_total, f.line_start, f.line_end, fc, ft
                    ));
                }
            }
            if f.count > 0 {
                fx += 1;
            }
            let b_delta = f.branches.len().checked_mul(2).ok_or_else(|| "summary.branchesTotal overflows usize while summing records".to_string())?;
            add(&mut bt, b_delta, "branchesTotal")?;
            let bc_delta: usize = f.branches.iter().map(|b| (b.true_count > 0) as usize + (b.false_count > 0) as usize).sum();
            add(&mut bc, bc_delta, "branchesCovered")?;
            for (di, d) in f.decisions.iter().enumerate() {
                let derived_mcdc = !d.conditions.is_empty() && d.conditions.iter().all(|c| c.independence_covered);
                if d.mcdc_covered != derived_mcdc {
                    return Err(format!(
                        "function `{}` decisions[{di}] reports mcdcCovered={}, but its {} condition(s) derive {derived_mcdc}: a cached decision flag disagrees with its conditions",
                        f.name,
                        d.mcdc_covered,
                        d.conditions.len()
                    ));
                }
            }
            add(&mut dt, f.decisions.len(), "decisionsTotal")?;
            add(&mut dm, f.decisions.iter().filter(|d| d.mcdc_covered).count(), "decisionsMcdcCovered")?;
            for d in &f.decisions {
                add(&mut ct, d.conditions.len(), "conditionsTotal")?;
                add(&mut cc, d.conditions.iter().filter(|c| c.independence_covered).count(), "conditionsIndependenceCovered")?;
            }
            for o in f.objects() {
                add(&mut it, o.instr_total, "instructionsTotal")?;
                add(&mut ipc, o.instr_projected_covered, "instructionsProjectedCovered")?;
                add(&mut ipu, o.instr_projected_uncovered, "instructionsProjectedUncovered")?;
                add(&mut iu, o.instr_unknown, "instructionsUnknown")?;
                add(&mut gaps, o.gaps.len(), "gapsTotal")?;
            }
        }
        want("functionsExecuted", s.functions_executed, fx)?;
        want("functionsUnrecorded", s.functions_unrecorded, unrecorded)?;
        want("branchesTotal", s.branches_total, bt)?;
        want("branchesCovered", s.branches_covered, bc)?;
        want("decisionsTotal", s.decisions_total, dt)?;
        want("decisionsMcdcCovered", s.decisions_mcdc_covered, dm)?;
        want("conditionsTotal", s.conditions_total, ct)?;
        want("conditionsIndependenceCovered", s.conditions_independence_covered, cc)?;
        want("instructionsTotal", s.instructions_total, it)?;
        want("instructionsProjectedCovered", s.instructions_projected_covered, ipc)?;
        want("instructionsProjectedUncovered", s.instructions_projected_uncovered, ipu)?;
        want("instructionsUnknown", s.instructions_unknown, iu)?;
        want("gapsTotal", s.gaps_total, gaps)?;
        Ok(())
    }
}

/// Reduce a name from a data file to something safe to put in a file path:
/// no separators, no traversal, no control characters, never empty.
pub fn safe_path_component(name: &str) -> String {
    let out: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect();
    let out = out.trim_matches('.').to_string();
    if out.is_empty() {
        "unnamed".into()
    } else {
        out.chars().take(64).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Default` and Serde must agree about a missing `instantiations`.
    /// The derived `Default` gave 0 while Serde gives 1, and code that
    /// computes `instantiations - 1` underflowed on a default-built
    /// function (review of #12).
    #[test]
    fn a_default_function_matches_what_serde_fills_in() {
        let d = Function::default();
        assert_eq!(d.instantiations, 1, "one record is one instantiation, not zero");
        // the count of merged duplicates must not underflow
        assert_eq!(d.instantiations - 1, 0, "a lone function contributes no duplicates");

        // and a document with the field missing deserialises the same way
        let mut v = serde_json::to_value(&d).unwrap();
        v.as_object_mut().unwrap().remove("instantiations");
        let from_json: Function = serde_json::from_value(v).unwrap();
        assert_eq!(from_json.instantiations, d.instantiations, "Default and a missing field must agree");
    }

    #[test]
    fn path_components_from_data_are_neutralised() {
        assert_eq!(safe_path_component("O2"), "O2");
        assert_eq!(safe_path_component("my-variant_1.0"), "my-variant_1.0");
        // traversal, separators, NUL and control characters must not survive
        // separators are what make traversal possible; they never survive
        for hostile in ["../../../../tmp/pwned", "/etc/passwd", "..\\..\\windows", "a/../../b"] {
            let out = safe_path_component(hostile);
            assert!(!out.contains('/') && !out.contains('\\'), "{hostile} -> {out}");
        }
        assert_eq!(safe_path_component("a\u{0}b\nc"), "a_b_c");
        assert_eq!(safe_path_component(".."), "unnamed");
        assert_eq!(safe_path_component(""), "unnamed");
        assert!(!safe_path_component(&"x".repeat(500)).contains('/'));
        assert!(safe_path_component(&"x".repeat(500)).len() <= 64);
    }

    /// Evidence written before `provenance.sourceRevision` existed still
    /// loads, and says nothing about the sources rather than calling them
    /// clean. A record that contradicts itself is refused on load (#300).
    #[test]
    fn source_revision_is_optional_on_load_and_checked_when_present() {
        let mut v = serde_json::to_value(tiny()).unwrap();
        v["provenance"].as_object_mut().unwrap().remove("sourceRevision");
        let ev: Evidence = serde_json::from_value(v.clone()).unwrap();
        assert!(ev.provenance.source_revision.is_none());
        assert!(ev.validate().is_ok());

        v["provenance"]["sourceRevision"] = serde_json::json!({"vcs": "git", "state": "clean", "capturedAt": "ingest", "modifiedCount": 2});
        let forged: Evidence = serde_json::from_value(v).unwrap();
        assert!(forged.validate().unwrap_err().contains("sourceRevision"), "{:?}", forged.validate());
    }

    /// Evidence written before `provenance.toolCommit` and `toolDirty`
    /// existed (#117) still loads, and says the commit was not recorded
    /// rather than inventing one or calling the tree clean. The description
    /// is one function for the report, the viewer's wording and
    /// `butterfly --version`, so each state is pinned here.
    #[test]
    fn tool_commit_is_optional_on_load_and_never_reads_as_clean_when_absent() {
        let mut v = serde_json::to_value(tiny()).unwrap();
        let prov = v["provenance"].as_object_mut().unwrap();
        prov.remove("toolCommit");
        prov.remove("toolDirty");
        let ev: Evidence = serde_json::from_value(v).unwrap();
        assert_eq!(ev.provenance.tool_commit, "");
        assert_eq!(ev.provenance.tool_dirty, None);
        assert!(ev.validate().is_ok());
        assert_eq!(ev.provenance.tool_build(), "commit not recorded");

        let id = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(describe_tool_build(id, Some(false)), format!("commit {id}, clean tree"));
        assert_eq!(describe_tool_build(id, Some(true)), format!("commit {id}, tree modified at build"));
        assert_eq!(describe_tool_build(id, None), format!("commit {id}, tree state unknown"));
        assert_eq!(describe_tool_build("unknown", None), "commit unknown");

        // and what a build writes round-trips in camelCase
        let mut ev = tiny();
        ev.provenance.tool_commit = id.into();
        ev.provenance.tool_dirty = Some(true);
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["provenance"]["toolCommit"], id);
        assert_eq!(v["provenance"]["toolDirty"], true);
        let back: Evidence = serde_json::from_value(v).unwrap();
        assert_eq!(back.provenance.tool_commit, id);
        assert_eq!(back.provenance.tool_dirty, Some(true));
    }

    fn tiny() -> Evidence {
        Evidence {
            schema: SCHEMA.into(),
            provenance: Provenance::default(),
            summary: Summary::default(),
            files: vec![SourceFile {
                id: 0,
                path: "a.c".into(),
                display: "a.c".into(),
                sha256: String::new(),
                // two lines, so a fixture may add a second function on
                // line 2 without claiming a range past the file's end
                text: vec!["int f(){}".into(), "int g(){}".into()],
                lines: vec![LineCov { count: Some(1) }, LineCov { count: Some(1) }],
                function_ids: vec![0],
                lines_total: 2,
                lines_covered: 2,
            }],
            // the file says it holds function 0, so function 0 has to exist
            // and has to agree that it lives in this file (#11)
            tests: Vec::new(),
            requirements_universe: Vec::new(),
            // line 1 is executable and covered, so the function's own line
            // counters must say so (the per-function derivation checks them)
            functions: vec![Function { id: 0, name: "f".into(), file_id: 0, line_start: 1, line_end: 1, lines_total: 1, lines_covered: 1, ..Default::default() }],
            symbols: vec![],
            globals: vec![],
            image: None,
            measured: None,
            baseline: None,
            changes: None,
        }
    }

    #[test]
    fn validate_rejects_out_of_range_indices() {
        let mut ev = tiny();
        assert!(ev.validate().is_ok());
        let mut f = Function {
            id: 0,
            name: "f".into(),
            mangled: "f".into(),
            file_id: 0,
            line_start: 1,
            line_end: 1,
            count: 1,
            instantiations: 1,
            tests: Vec::new(),
            requirements: vec![],
            line_requirements: BTreeMap::new(),
            lines_total: 1,
            lines_covered: 1,
            branches: vec![],
            decisions: vec![],
            complexity: Complexity::default(),
            callees: vec![],
            callers: vec![],
            object: None,
            variants: Vec::new(),
            symbol_id: None,
            data_refs: vec![],
            variables: Vec::new(),
            constructs: BTreeMap::new(),
            inlined: vec![],
            own_lines: None,
            folded_calls: vec![],
            indirect_calls: vec![],
            pointer_callers: vec![],
            commits: vec![],
            origin: None,
            unrecorded_within: None,
        };
        f.file_id = 42;
        ev.functions = vec![f.clone()];
        let e = ev.validate().unwrap_err();
        assert!(e.contains("fileId"), "{e}");
        f.file_id = 0;
        f.symbol_id = Some(7);
        ev.functions = vec![f.clone()];
        assert!(ev.validate().unwrap_err().contains("symbolId"));
        f.symbol_id = None;
        f.unrecorded_within = Some(5);
        ev.functions = vec![f.clone()];
        assert!(ev.validate().unwrap_err().contains("unrecordedWithin"));
        f.unrecorded_within = None;
        f.data_refs = vec![DataRef { global_id: 3, reads: 1, writes: 0, addr_taken: 0, lines: vec![] }];
        ev.functions = vec![f];
        assert!(ev.validate().unwrap_err().contains("globalId"));
    }

    /// A construct name is part of a small vocabulary ingest writes; one
    /// carrying markup, whitespace or a control character is refused at the
    /// door, and every name ingest emits is accepted (#337).
    #[test]
    fn validate_refuses_a_construct_name_ingest_would_not_write() {
        let mut ev = tiny();
        let f = ev.functions[0].clone();
        for good in ["if", "&&", "||", "?-operator", "shift/bitwise", "panic/unwrap", "try/catch", "scope-res", "as-cast", "int64"] {
            let mut g = f.clone();
            g.constructs = BTreeMap::from([(good.to_string(), 1)]);
            ev.functions = vec![g];
            assert_eq!(ev.validate(), Ok(()), "{good} is a construct ingest writes");
        }
        for bad in ["", "<tr><td>memcpy</td><!--", "a b", "a\nb", "quote\"", "it's", "back`tick", "é", &"x".repeat(MAX_CONSTRUCT_NAME + 1)] {
            let mut g = f.clone();
            g.constructs = BTreeMap::from([(bad.to_string(), 1)]);
            ev.functions = vec![g];
            let e = ev.validate().expect_err(bad);
            assert!(e.contains("constructs"), "{e}");
        }
    }

    /// `files[].functionIds` is the reverse of `functions[].fileId`, and the
    /// viewer builds a file's function list straight from it. Checking only
    /// the entries that happened to be present left a function its file
    /// never lists (silently hidden) and one listed twice (shown twice) both
    /// accepted (#11, review of #11).
    #[test]
    fn validate_holds_a_files_function_list_to_exactly_what_names_it() {
        let mut ev = tiny();
        assert!(ev.validate().is_ok(), "the fixture is consistent to begin with");

        // out of range
        ev.files[0].function_ids = vec![999_999];
        let e = ev.validate().unwrap_err();
        assert!(e.contains("999999") && e.contains("not a function at all"), "{e}");

        // omitted: the function names this file, the file does not list it
        ev.files[0].function_ids = vec![];
        let e = ev.validate().unwrap_err();
        assert!(e.contains("does not list") && e.contains("hide"), "an omitted function is hidden by the viewer: {e}");

        // listed twice
        ev.files[0].function_ids = vec![0, 0];
        let e = ev.validate().unwrap_err();
        assert!(e.contains("more than once"), "{e}");

        // in range, but the function says it lives elsewhere
        ev.files.push(SourceFile { id: 1, path: "b.c".into(), display: "b.c".into(), ..Default::default() });
        ev.files[0].function_ids = vec![0];
        ev.files[1].function_ids = vec![0];
        let e = ev.validate().unwrap_err();
        assert!(e.contains("files[1]") && e.contains("which is in files[0]"), "the back-reference has to agree: {e}");

        // and exactly-once membership is accepted
        ev.files[1].function_ids = vec![];
        assert!(ev.validate().is_ok(), "{:?}", ev.validate());
    }

    /// Complete and unique but reordered is still a disagreement: the viewer
    /// renders a file's functions in this order.
    #[test]
    fn validate_rejects_a_reordered_function_list() {
        let mut ev = tiny();
        ev.functions.push(Function { id: 1, name: "g".into(), file_id: 0, line_start: 2, line_end: 2, ..Default::default() });
        ev.files[0].function_ids = vec![0, 1];
        assert!(ev.validate().is_ok(), "{:?}", ev.validate());

        ev.files[0].function_ids = vec![1, 0];
        let e = ev.validate().unwrap_err();
        assert!(e.contains("different order"), "{e}");
    }

    /// The diagnostic path is what a malformed file reaches, so it has to
    /// stay cheap. `Vec::contains` inside a loop over the other vector is
    /// quadratic, and a complete list in reverse order hits the worst case on
    /// every entry: ~1.8s at 20,000 ids, growing as the square (review of
    /// #11). At 100,000 that shape would take minutes; this takes
    /// milliseconds, so the test needs no wall-clock assertion -- if the
    /// quadratic scan came back, the suite would stop finishing.
    #[test]
    fn a_large_malformed_function_list_is_diagnosed_without_quadratic_work() {
        const N: usize = 100_000;
        let mut ev = tiny();
        ev.functions = (0..N).map(|k| Function { id: k, name: format!("f{k}"), file_id: 0, line_start: 1, line_end: 1, ..Default::default() }).collect();

        // complete, unique, and entirely in the wrong order
        ev.files[0].function_ids = (0..N).rev().collect();
        let e = ev.validate().unwrap_err();
        assert!(e.contains("different order"), "{e}");
        assert!(e.len() < 400, "the message names the first divergence, not both lists ({} bytes)", e.len());

        // and the same size, correct, is accepted
        ev.files[0].function_ids = (0..N).collect();
        assert!(ev.validate().is_ok(), "{:?}", ev.validate().unwrap_err());

        // one missing from a large list is still found, and still cheap
        ev.files[0].function_ids = (0..N - 1).collect();
        let e = ev.validate().unwrap_err();
        assert!(e.contains("does not list") && e.contains(&format!("f{}", N - 1)), "{e}");
    }

    /// Every consumer indexes these arrays with the id -- `S.ev.functions[id]`
    /// in the viewer -- so an id that disagrees with its position silently
    /// addresses a different record (#11).
    #[test]
    fn validate_rejects_an_id_that_is_not_its_position() {
        let case = |mutate: fn(&mut Evidence), want: &str| {
            let mut ev = tiny();
            ev.symbols.push(Symbol { id: 0, name: "s".into(), ..Default::default() });
            ev.globals.push(Global { id: 0, name: "g".into(), ..Default::default() });
            ev.functions[0].object = Some(ObjectFn {
                blocks: vec![Block { id: 0, ..Default::default() }, Block { id: 1, ..Default::default() }],
                gaps: vec![Gap { id: 0, ..Default::default() }],
                ..Default::default()
            });
            assert!(ev.validate().is_ok(), "the fixture is consistent to begin with: {:?}", ev.validate());
            mutate(&mut ev);
            let e = ev.validate().unwrap_err();
            assert!(e.contains(want) && e.contains("positions"), "expected {want} in: {e}");
        };
        case(|ev| ev.files[0].id = 42, "files[0]");
        case(|ev| ev.functions[0].id = 42, "functions[0]");
        case(|ev| ev.symbols[0].id = 42, "symbols[0]");
        case(|ev| ev.globals[0].id = 42, "globals[0]");
        case(|ev| ev.functions[0].object.as_mut().unwrap().blocks[1].id = 5, "blocks[1]");
        case(|ev| ev.functions[0].object.as_mut().unwrap().gaps[0].id = 5, "gaps[0]");
    }

    #[test]
    fn validate_rejects_a_foreign_schema() {
        let mut ev = tiny();
        ev.schema = "something-else/9".into();
        assert!(ev.validate().unwrap_err().contains("unsupported schema"));
    }

    /// #249: the summary is a derived cache that gates and reports read. A
    /// consistent one passes; one whose fields disagree with the records they
    /// are drawn from is rejected, naming the first field.
    /// Two copies of one generic share a source range, and each carries its
    /// own line counts, recovered from its own coverage records: the file's
    /// are the sum over both (#167). The per-function check derives a copy's
    /// figures from its own counts -- the file's would reject every copy
    /// that did not run every line (#276) -- and binds those counts to the
    /// file's: a line a copy says it ran must have run in the file, and a
    /// line a copy calls executable must be executable there.
    #[test]
    fn a_function_with_its_own_line_counts_is_derived_from_them_and_bound_to_the_file() {
        let mut ev = tiny();
        ev.files[0].text = vec!["fn limit<T>() {".into(), "  a".into(), "  b".into()];
        ev.files[0].lines = vec![LineCov { count: Some(2) }, LineCov { count: Some(2) }, LineCov { count: Some(1) }];
        ev.files[0].lines_total = 3;
        ev.files[0].lines_covered = 3;
        let copy = |id: usize, name: &str, own: Vec<Option<u64>>, covered: usize| Function {
            id,
            name: name.into(),
            file_id: 0,
            line_start: 1,
            line_end: 3,
            lines_total: 3,
            lines_covered: covered,
            own_lines: Some(own),
            ..Default::default()
        };
        // the <i32> copy never reached line 3; the <u64> copy did
        ev.functions = vec![copy(0, "limit::<i32>", vec![Some(1), Some(1), Some(0)], 2), copy(1, "limit::<u64>", vec![Some(1), Some(1), Some(1)], 3)];
        ev.files[0].function_ids = vec![0, 1];
        ev.summary = Summary { files: 1, functions: 2, lines_total: 3, lines_covered: 3, ..Default::default() };
        assert!(ev.check_summary_derivation().is_ok(), "{:?}", ev.check_summary_derivation());

        // a cached total that disagrees with the copy's own counts is refused
        let mut e = ev.clone();
        e.functions[0].lines_covered = 3;
        assert!(e.check_summary_derivation().unwrap_err().contains("its own line counts"));

        // a copy claiming a line ran that did not run in the file is refused
        let mut e = ev.clone();
        e.files[0].lines[2].count = Some(0);
        e.files[0].lines_covered = 2;
        e.summary.lines_covered = 2;
        assert!(e.check_summary_derivation().unwrap_err().contains("line 3"));

        // own counts of the wrong length cannot be read against the range
        let mut e = ev.clone();
        e.functions[1].own_lines = Some(vec![Some(1)]);
        assert!(e.check_summary_derivation().unwrap_err().contains("own line counts"));
    }

    #[test]
    fn check_summary_derivation_binds_the_summary_to_the_records() {
        let mut ev = tiny();
        // tiny() carries a default (zeroed) summary; make it agree with its one
        // file (2 covered lines) and one function.
        ev.summary = Summary { files: 1, functions: 1, lines_total: 2, lines_covered: 2, ..Default::default() };
        assert!(ev.check_summary_derivation().is_ok(), "{:?}", ev.check_summary_derivation());

        // A summary that understates coverage/gaps or miscounts is refused, and
        // the message names the field that disagreed.
        let bump = |mutate: fn(&mut Summary)| {
            let mut e = ev.clone();
            mutate(&mut e.summary);
            e.check_summary_derivation().unwrap_err()
        };
        assert!(bump(|s| s.lines_covered = 1).contains("linesCovered"));
        assert!(bump(|s| s.gaps_total = 5).contains("gapsTotal"));
        assert!(bump(|s| s.functions = 9).contains("functions"));
        assert!(bump(|s| s.branches_total = 4).contains("branchesTotal"));

        // A forged summary claiming zero gaps over a function that actually has
        // one is the security case: the recompute catches it. The object's
        // instruction total is reflected in the summary so `gapsTotal` is the
        // only field left disagreeing.
        let mut forged = ev.clone();
        forged.functions[0].object =
            Some(ObjectFn { symbol: "_f".into(), gaps: vec![Gap { id: 0, key: "_f@line-zero@100".into(), ..Default::default() }], instr_total: 1, ..Default::default() });
        forged.summary.instructions_total = 1; // matches the object; gaps_total stays 0
        assert!(forged.check_summary_derivation().unwrap_err().contains("gapsTotal"));

        // A file's own lines_total/lines_covered are caches ingest derives from
        // the line records. A document where that file cache disagrees with its
        // records is refused even when the summary agrees with the bad cache --
        // the summary is bound to the primitives, not to another cache (#249).
        let mut bad_file = ev.clone();
        bad_file.files[0].lines_total = 3; // the two line records still derive 2
        bad_file.summary.lines_total = 3; // summary made to agree with the bad cache
        let err = bad_file.check_summary_derivation().unwrap_err();
        assert!(err.contains("disagrees with the coverage it is drawn from"), "{err}");

        // A per-function counter that disagrees with the records is refused too,
        // even with the file and global totals correct: gate::regressions()
        // reads it per function, so it cannot be left unbound (#249).
        let mut bad_fn = ev.clone();
        bad_fn.functions[0].lines_total = 2; // its range (line 1) has one executable line
        let err = bad_fn.check_summary_derivation().unwrap_err();
        assert!(err.contains("per-function total disagrees with the coverage it is drawn from"), "{err}");
    }

    /// A function with no coverage record (#273) sits on lines that carry
    /// another function's counts. Its own figures are 0/0 and nothing of it
    /// is projected, and the summary counts it apart. Evidence claiming more
    /// for it is claiming what no record showed, and is refused.
    #[test]
    fn a_function_with_no_coverage_record_can_claim_nothing() {
        let mut ev = tiny();
        // `f` covers line 1; the closure written on it has no record
        ev.functions.push(Function {
            id: 1,
            name: "f::{closure#0}".into(),
            file_id: 0,
            line_start: 1,
            line_end: 1,
            instantiations: 0,
            unrecorded_within: Some(0),
            object: Some(ObjectFn { symbol: "_c".into(), instr_total: 4, instr_unknown: 4, ..Default::default() }),
            ..Default::default()
        });
        ev.files[0].function_ids = vec![0, 1];
        ev.summary =
            Summary { files: 1, functions: 2, functions_unrecorded: 1, lines_total: 2, lines_covered: 2, instructions_total: 4, instructions_unknown: 4, ..Default::default() };
        assert!(ev.validate().is_ok(), "{:?}", ev.validate());
        assert!(ev.check_summary_derivation().is_ok(), "{:?}", ev.check_summary_derivation());

        let claim = |mutate: fn(&mut Evidence)| {
            let mut e = ev.clone();
            mutate(&mut e);
            e.check_summary_derivation().unwrap_err()
        };
        // Line 1 is covered, but not by this function's counts.
        assert!(claim(|e| e.functions[1].lines_total = 1).contains("no coverage record"));
        assert!(claim(|e| e.functions[1].lines_covered = 1).contains("no coverage record"));
        assert!(claim(|e| e.functions[1].count = 3).contains("no coverage record"));
        assert!(claim(|e| e.functions[1].object.as_mut().unwrap().instr_projected_covered = 4).contains("no coverage record"));
        // It must name a function that has a record.
        assert!(claim(|e| e.functions[1].unrecorded_within = Some(1)).contains("not a function with a record"));
        assert!(claim(|e| e.functions[1].unrecorded_within = Some(7)).contains("not a function with a record"));
        // And the summary must count it.
        assert!(claim(|e| e.summary.functions_unrecorded = 0).contains("functionsUnrecorded"));
        // Projected instructions in a variant of an unrecorded function must
        // also be refused (not only `f.object`), including overflow-shaped counts.
        assert!(claim(|e| {
            e.functions[1].variants.push(ObjectFn { symbol: "_cD1".into(), variant: Some("D1".into()), instr_total: 2, instr_projected_covered: 2, ..Default::default() });
        })
        .contains("no coverage record"));
        assert!(claim(|e| {
            let o = e.functions[1].object.as_mut().unwrap();
            o.instr_projected_covered = usize::MAX;
            o.instr_projected_uncovered = 1;
        })
        .contains("no coverage record"));
    }

    #[test]
    fn check_summary_derivation_rejects_overflow_and_mcdc_mismatch() {
        let mut ev = tiny();
        ev.functions[0].object = Some(ObjectFn { symbol: "_f".into(), instr_total: usize::MAX, ..Default::default() });
        ev.functions[0].variants.push(ObjectFn { symbol: "_fD1".into(), variant: Some("D1".into()), instr_total: 1, ..Default::default() });
        ev.summary = Summary { files: 1, functions: 1, lines_total: 2, lines_covered: 2, instructions_total: 0, ..Default::default() };
        assert!(ev.check_summary_derivation().unwrap_err().contains("overflows"), "wrapped sum must not match forged 0");

        let mut ev = tiny();
        ev.functions[0].decisions.push(Decision {
            line: 1,
            col: 1,
            line_end: 1,
            col_end: 5,
            true_count: 1,
            false_count: 0,
            conditions: vec![Condition { index: 0, line: 1, col: 1, true_count: 1, false_count: 0, independence_covered: false, pair: None }],
            vectors: vec![],
            mcdc_covered: true,
            decision_ordinal: None,
        });
        ev.summary = Summary { files: 1, functions: 1, lines_total: 2, lines_covered: 2, decisions_total: 1, decisions_mcdc_covered: 1, conditions_total: 1, ..Default::default() };
        assert!(ev.check_summary_derivation().unwrap_err().contains("mcdcCovered"));
    }

    #[test]
    fn validate_checks_mcdc_pair_vector_tests_and_changes_refs() {
        let mut ev = tiny();
        ev.tests.push(TestRun { id: 0, name: "t0".into(), path: "t0.json".into(), functions: vec![0], lines: BTreeMap::new() });
        ev.functions[0].tests = vec![0];
        ev.functions[0].decisions.push(Decision {
            line: 1,
            col: 1,
            line_end: 1,
            col_end: 5,
            true_count: 1,
            false_count: 1,
            conditions: vec![Condition { index: 0, line: 1, col: 1, true_count: 1, false_count: 1, independence_covered: true, pair: Some((1, 2)) }],
            vectors: vec![
                TestVector { values: vec![Some(false)], result: false, executed: true, tests: vec![0] },
                TestVector { values: vec![Some(true)], result: true, executed: true, tests: vec![0] },
            ],
            mcdc_covered: true,
            decision_ordinal: Some(0),
        });
        assert_eq!(ev.functions[0].decisions[0].site(), "1:#0");
        assert!(ev.validate().is_ok(), "{:?}", ev.validate());

        // out-of-range 1-based pair (0 or past len)
        let mut bad = ev.clone();
        bad.functions[0].decisions[0].conditions[0].pair = Some((0, 1));
        assert!(bad.validate().unwrap_err().contains(".pair"));
        let mut bad = ev.clone();
        bad.functions[0].decisions[0].conditions[0].pair = Some((1, 3));
        assert!(bad.validate().unwrap_err().contains(".pair"));

        // out-of-range or duplicate/unsorted vector.tests
        let mut bad = ev.clone();
        bad.functions[0].decisions[0].vectors[0].tests = vec![9];
        assert!(bad.validate().unwrap_err().contains("vectors[0].tests"));
        let mut bad = ev.clone();
        bad.functions[0].decisions[0].vectors[0].tests = vec![0, 0];
        assert!(bad.validate().unwrap_err().contains("not sorted and unique"));

        // changes.since whitespace/control chars and changes.head control chars
        let mut bad = ev.clone();
        bad.changes = Some(Changes { head: "a".repeat(40), since: Some("main; rm -rf".into()), ..Default::default() });
        assert!(bad.validate().unwrap_err().contains("changes.since"));
        let mut bad = ev.clone();
        bad.changes = Some(Changes { head: "abc\ndef".into(), since: Some("v1.0".into()), ..Default::default() });
        assert!(bad.validate().unwrap_err().contains("changes.head"));
    }
}
