// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly-ingest`: turn compiler and coverage artifacts into one evidence
//! database. This crate is the qualification-path component: it must be
//! deterministic, read-only with respect to its inputs, and every value it
//! emits must be traceable to an input listed in the provenance block.

pub mod baseline;
pub mod cfg;
pub mod change;
pub mod config;
pub mod constructs;
pub mod coverage;
pub mod datarefs;
pub mod eh;
pub mod fsauth;
pub mod gaps;
pub mod gate;
pub mod gcov;
pub mod got;
pub mod helper;
pub mod impact;
pub mod jumptable;
pub mod objectcode;
pub mod pertest;
pub mod preflight;
pub mod provenance;
pub mod relptr;
pub mod reqcsv;
pub mod reqmd;
pub mod reqtext;
pub mod revision;
pub mod stack;
pub mod tags;
pub mod trace;
pub mod variants;
pub mod vars;
pub mod worklist;

use anyhow::{bail, Context, Result};
use butterfly_model::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const TOOL: &str = "butterfly";
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The commit of butterfly's own sources this binary was built from, or
/// `unknown` (#117). Captured by `build.rs`; see there for what is and is not
/// established, and [`Provenance::tool_commit`] for what the value means.
pub const TOOL_COMMIT: &str = env!("BUTTERFLY_TOOL_COMMIT");
/// Whether the tool's own work tree had uncommitted changes when it was
/// built; `None` when that could not be established (#117, `build.rs`).
pub fn tool_dirty() -> Option<bool> {
    match env!("BUTTERFLY_TOOL_DIRTY") {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}
/// `0.2.0 (commit <id>, clean tree)`: the version and the build, as
/// `butterfly --version` prints it and as a report's header names the tool.
/// One sentence from one source, so the binary and the evidence it writes
/// cannot describe the same build two ways.
pub fn tool_version_and_build() -> String {
    format!("{TOOL_VERSION} ({})", describe_tool_build(TOOL_COMMIT, tool_dirty()))
}

#[derive(Debug, Clone, Default)]
pub struct IngestOptions {
    pub coverage_json: PathBuf,
    /// Linked executables or relocatable objects to analyse (may be empty).
    pub objects: Vec<PathBuf>,
    /// Objects discovery rejected because they carry coverage
    /// instrumentation, with the toolchain that put it there.
    ///
    /// Kept rather than dropped: a directory holding *only* instrumented
    /// binaries otherwise looks exactly like one holding no objects at all,
    /// and those two need very different advice.
    pub instrumented_skipped: Vec<(PathBuf, preflight::Toolchain)>,
    pub build_info: Option<PathBuf>,
    /// Root used to shorten displayed source paths.
    pub source_root: Option<PathBuf>,
    pub llvm_bin: Option<PathBuf>,
    /// Execution trace (QEMU `-d in_asm,exec,nochain` log, execlog CSV, or
    /// a plain address list) for measured object coverage.
    pub trace: Option<PathBuf>,
    /// A requirements universe: one requirement ID per line (`#` comments
    /// allowed). Makes "a requirement with no code" knowable in the matrix;
    /// tags alone cannot prove absence (#42).
    pub requirements_list: Option<PathBuf>,
    /// Per-test coverage exports: one `llvm-cov export` per test run, so the
    /// evidence can say which test exercised what (#41). The merged export in
    /// `coverage_json` stays the source of truth for every count; these only
    /// add attribution on top of it.
    pub tests: Vec<PathBuf>,
    /// `.su` files from `-fstack-usage`.
    pub stack_usage: Vec<PathBuf>,
    /// Frozen analysis baseline to compare against (`butterfly baseline`).
    pub baseline: Option<PathBuf>,
    /// Project configuration (`butterfly.toml`): requirement-tag patterns.
    pub config: Option<PathBuf>,
    /// Change-history cache written by `butterfly changes`; re-attached when
    /// every analysed file still matches its cached hash.
    pub changes: Option<PathBuf>,
    /// Read source files that resolve outside the source root. Off by
    /// default: filenames in a coverage export are untrusted input, and
    /// their contents are embedded in the evidence.
    pub allow_external_source: bool,
    /// The inputs discovery found by their name (or contents) in a
    /// directory, rather than being handed their path. Each is read only as
    /// the regular file at that name, never through a symlink, however its
    /// field was filled (#341). A path the operator names is not in here and
    /// follows links as it always did.
    pub found_by_name: Vec<PathBuf>,
}

/// Discover artifacts in a build directory: `coverage.json` (or any llvm
/// export), `build-info.txt`, and executables/objects.
pub fn discover(dir: &Path) -> Result<IngestOptions> {
    let (mut opts, coverage) = discover_artifacts(dir)?;
    let Some(c) = coverage else { bail!("no llvm-cov export JSON found in {}", dir.display()) };
    opts.coverage_json = c;
    Ok(opts)
}

/// Everything a build directory holds, with the coverage export returned
/// separately because it is often *not* there.
///
/// `butterfly build` writes each variant's executable, `build-info.txt` and
/// `-fstack-usage` output into the variant directory and the coverage export
/// one level up. A caller in that position still wants the stack-usage files;
/// making discovery all-or-nothing forced it to discard them (#7).
pub fn discover_artifacts(dir: &Path) -> Result<(IngestOptions, Option<PathBuf>)> {
    let lim = &limits::Limits::from_env();
    let mut opts = IngestOptions::default();
    let mut coverage = None;
    let mut skipped: Vec<(PathBuf, preflight::Toolchain)> = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let p = entry.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if p.is_dir() {
            continue;
        }
        // A symlink is never taken as an input (#341). Anyone who can write
        // here could otherwise plant `build-info.txt -> ~/.aws/credentials`
        // and have its lines published in the evidence. What the name (or,
        // for an export or an object, the first bytes) says it would have
        // been decides only how the refusal is worded; a link discovery would
        // have passed over is passed over still.
        if entry.file_type().is_ok_and(|t| t.is_symlink()) {
            if let Some(flag) = symlink_would_be(&p, &name) {
                bail!(
                    "{}: is a symlink (to {}). Discovery takes a file by its name only as the regular file there, never through a link, so that a link planted in a build directory cannot put another file into the evidence (#341). Replace the link with the file itself{}.",
                    p.display(),
                    std::fs::read_link(&p).map(|t| t.display().to_string()).unwrap_or_else(|_| "an unreadable target".into()),
                    follow_remedy(flag, true)
                );
            }
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(md) = entry.metadata() {
                if md.is_file() && md.nlink() > 1 {
                    if let Some(flag) = symlink_would_be(&p, &name) {
                        bail!(
                            "{}: has {} hard links ({}). Discovery takes a file by its name only when it has a single name, never through a hard link (#248, #341). Replace the link with a copy{}.",
                            p.display(),
                            md.nlink(),
                            fsauth::LINKED_REASON,
                            follow_remedy(flag, true)
                        );
                    }
                    continue;
                }
            }
        }
        // butterfly's own JSON sidecars are matched by name FIRST. The
        // generic `.json` branch below recognises a coverage export and
        // `continue`s unconditionally, so anything tested after it is
        // unreachable for a file ending in `.json` -- which is how a
        // `butterfly-baseline.json` dropped into a build directory was seen,
        // skipped and reported as `baseline: null`, with no warning and no
        // stale-review invalidation (#238). Any future sidecar belongs here
        // too, above that `continue`, never below it.
        if name == "butterfly-changes.json" {
            opts.changes = Some(p.clone());
            continue;
        }
        if name == "butterfly-baseline.json" {
            opts.baseline = Some(p.clone());
            continue;
        }
        if name.ends_with(".json") {
            // llvm-cov puts `"type"` after the `data` array, so the marker
            // is at the end of a file that is almost all data -- both ends,
            // bounded, rather than the whole file. Reading every .json in a
            // build directory whole is the cost this avoids (#5).
            if let Ok(edges) = limits::read_edges(&p, 4096) {
                let head = String::from_utf8_lossy(&edges);
                // Either producer's export is a coverage export. gcov puts
                // `gcc_version` and `format_version` at the top; llvm-cov
                // puts its marker after the data array, which is why both
                // ends are read rather than the whole file (#5).
                if head.contains("llvm.coverage.json.export") || gcov::looks_like(&head) {
                    coverage = Some(p.clone());
                }
            }
            continue;
        }
        if name == "build-info.txt" {
            opts.build_info = Some(p.clone());
            continue;
        }
        if name.ends_with(".su") {
            opts.stack_usage.push(p.clone());
            continue;
        }
        if name == "butterfly.toml" {
            opts.config = Some(p.clone());
            continue;
        }
        if (name.contains("trace") && (name.ends_with(".log") || name.ends_with(".txt"))) || name.ends_with(".trace") {
            opts.trace = Some(p.clone());
            continue;
        }
        if name.ends_with(".profraw") || name.ends_with(".profdata") || name.ends_with(".txt") || name.ends_with(".log") {
            continue;
        }
        // Instrumented binaries carry a __llvm_prf / __llvm_covmap section; skip
        // them - the object view must show the non-instrumented code.
        //
        // Check the magic number first. Every unknown file used to be read
        // whole and handed to the parser just to find out whether it was an
        // object at all, so one large unrelated file in a build directory
        // cost its own size in memory (#5).
        let Ok(head) = limits::read_header(&p, 64) else { continue };
        if !limits::looks_like_object(&head) {
            continue;
        }
        let Ok(bytes) = limits::read_bounded(&p, limits::Kind::Object, lim) else { continue };
        if let Ok(f) = object::File::parse(&*bytes) {
            // Both toolchains, not just clang. A GCC `--coverage` binary has
            // no `__llvm_covmap` section at all -- it carries `__gcov0.<fn>`
            // counter symbols instead -- so a section-only test waved it
            // through and butterfly analysed the counters as if they were the
            // delivered code. That is the worst thing on this list because it
            // produces coverage that looks perfect (#45).
            match preflight::instrumented_with(&f) {
                None => opts.objects.push(p.clone()),
                Some(t) => skipped.push((p.clone(), t)),
            }
        }
    }
    opts.objects.sort();
    opts.stack_usage.sort();
    opts.instrumented_skipped = skipped;
    opts.found_by_name = [opts.changes.clone(), opts.baseline.clone(), coverage.clone(), opts.build_info.clone(), opts.config.clone(), opts.trace.clone()]
        .into_iter()
        .flatten()
        .chain(opts.stack_usage.iter().cloned())
        .chain(opts.objects.iter().cloned())
        .collect();
    Ok((opts, coverage))
}

/// The flag that names explicitly what discovery would have taken a symlink
/// at `p` for, or `None` when discovery would have passed it over anyway.
///
/// The by-name inputs are told by name alone. A `.json` file or an unknown
/// one is an input only by its contents, and a few bytes are read through the
/// link to tell -- bounded, and never kept, so nothing of the target can
/// reach the evidence or the error.
fn symlink_would_be(p: &Path, name: &str) -> Option<&'static str> {
    if name == "butterfly-changes.json" {
        // ingest has no flag for the cache: it is only ever discovered
        return Some("");
    }
    if name == "butterfly-baseline.json" {
        return Some("--baseline");
    }
    if name.ends_with(".json") {
        let edges = limits::read_edges(p, 4096).ok()?;
        let head = String::from_utf8_lossy(&edges);
        return (head.contains("llvm.coverage.json.export") || gcov::looks_like(&head)).then_some("--coverage");
    }
    if name == "build-info.txt" {
        return Some("--build-info");
    }
    if name.ends_with(".su") {
        return Some("--stack-usage");
    }
    if name == "butterfly.toml" {
        return Some("--config");
    }
    if (name.contains("trace") && (name.ends_with(".log") || name.ends_with(".txt"))) || name.ends_with(".trace") {
        return Some("--trace");
    }
    if name.ends_with(".profraw") || name.ends_with(".profdata") || name.ends_with(".txt") || name.ends_with(".log") {
        return None;
    }
    limits::read_header(p, 64).ok().filter(|h| limits::looks_like_object(h)).map(|_| "--object")
}

/// How to have a refused link followed on purpose, if there is a way: name
/// it with `flag`. After discovery, that means giving the inputs explicitly
/// rather than the directory, which is discovered as a whole.
fn follow_remedy(flag: &str, discovering: bool) -> String {
    match (flag, discovering) {
        ("", _) => String::new(),
        (f, true) => format!(", or name it with {f} -- giving the inputs explicitly rather than the directory -- to have it read through the link"),
        (f, false) => format!(", or name it with {f} to have it read through the link"),
    }
}

/// Read one input as text, under the budget.
///
/// `by_name` is the flag that would name it, when butterfly found the file by
/// its name rather than being handed its path: then only the regular file at
/// that name is read, never what a symlink there points at, and the refusal
/// is part of the open (#341). A path the operator named follows links as it
/// always did.
fn read_input(path: &Path, kind: limits::Kind, lim: &limits::Limits, by_name: Option<&str>) -> Result<String> {
    let Some(flag) = by_name else {
        return limits::read_bounded_string(path, kind, lim).map_err(|e| anyhow::anyhow!("{e}"));
    };
    let f = fsauth::open_discovered(path).map_err(|e| {
        anyhow::anyhow!(
            "{}: {e}. butterfly found this file by its name, and reads only a single-name regular file there, never through a symlink or hard link (#341, #248). Replace the link with a copy{}.",
            path.display(),
            follow_remedy(flag, false)
        )
    })?;
    limits::read_bounded_string_from(f, path, kind, lim).map_err(|e| anyhow::anyhow!("{e}"))
}

/// The coverage export's source files, opened, checked and read as one step.
struct Bound {
    /// The root the reads are confined to.
    root: PathBuf,
    /// Names that resolve outside `root`, in export order.
    outside: Vec<String>,
    /// Names whose file has other names too, with how many: whether those
    /// are inside `root` cannot be established (#248).
    linked: Vec<(String, u64)>,
    /// Export name -> (where it really was, what it held). Filled while the
    /// file was still open, so nothing is looked up by name a second time.
    read: HashMap<String, (PathBuf, Vec<u8>)>,
    /// Export name -> why it could not be opened or read at all. Kept as text
    /// rather than retried later: a name that failed here must not become a
    /// second chance to touch the filesystem.
    failed: HashMap<String, String>,
}

/// Open, bound and read the coverage export's source files.
///
/// The root is `--source-root` when given, else the working directory. Each
/// name is opened once; the kernel is then asked where that descriptor
/// actually points, and the bytes are read from the same descriptor. There is
/// no window in which a name could be re-aimed, because the name is never
/// resolved twice.
///
/// A file outside the root -- or one with other names, which cannot be
/// placed at all (#248) -- is closed unread unless the operator passed
/// `--allow-external-source`; a name that will not open, or that turns out
/// not to be a regular file, is recorded and reported by the caller.
fn contained_sources(names: &[&str], source_root: Option<&Path>, allow_external: bool, limits: &limits::Limits) -> Result<Bound> {
    use std::io::Read;

    let root = match source_root {
        Some(r) => r.to_path_buf(),
        None => std::env::current_dir().context("determining the working directory that bounds source reads")?,
    };
    let root = root.canonicalize().with_context(|| format!("resolving the source root {}", root.display()))?;
    let mut b = Bound { root, outside: Vec::new(), linked: Vec::new(), read: HashMap::new(), failed: HashMap::new() };
    for name in names {
        let key = (*name).to_string();
        let placed = match fsauth::open_placed(&b.root, Path::new(name)) {
            Ok(p) => p,
            Err(e) => {
                b.failed.insert(key, e.to_string());
                continue;
            }
        };
        match placed.standing {
            fsauth::Standing::Inside => {}
            fsauth::Standing::Outside => b.outside.push(key.clone()),
            fsauth::Standing::Linked(n) => b.linked.push((key.clone(), n)),
        }
        // the operator has not asked for this one: close it unread
        if placed.standing != fsauth::Standing::Inside && !allow_external {
            continue;
        }
        let fsauth::Placed { file: f, real, meta: md, .. } = placed;
        if !md.is_file() {
            b.failed.insert(key, "not a regular file".into());
            continue;
        }
        // Asked of the descriptor already open, so the size checked is the
        // size of the file about to be read -- and checked before the
        // allocation below rather than after it (#5).
        if let Err(e) = limits.admit(md.len(), limits::Kind::Source, Path::new(name)) {
            b.failed.insert(key, e);
            continue;
        }
        // The read is bounded independently of the metadata: a file can grow
        // between the stat and the read, and `read_to_end` would follow it.
        // Read at most one byte past the cap, reject if it got there, and
        // charge the total budget for any growth beyond what was reserved
        // (review of #5).
        let cap = limits.cap(limits::Kind::Source);
        let mut bytes = Vec::with_capacity(md.len() as usize);
        if let Err(e) = (&f).take(cap.saturating_add(1)).read_to_end(&mut bytes) {
            b.failed.insert(key, e.to_string());
            continue;
        }
        let actual = bytes.len() as u64;
        if actual > cap {
            b.failed.insert(key, format!("grew past the {} source-file cap while being read", limits::human(cap)));
            continue;
        }
        if actual > md.len() {
            if let Err(e) = limits.admit(actual - md.len(), limits::Kind::Source, Path::new(name)) {
                b.failed.insert(key, e);
                continue;
            }
        }
        b.read.insert(key, (real, bytes));
    }
    Ok(b)
}

/// Split an llvm-cov function name into its translation-unit qualifier and
/// the bare name the symbol table uses.
///
/// llvm-cov prefixes an internal-linkage function with its unit, and
/// `demangle` keeps that prefix while demangling the rest, so a C++ static
/// arrives as `a.cpp:ns::helper`. The qualifier is therefore the text before
/// the first *single* colon: a `::` is C++ namespacing and never a unit
/// separator. Refusing to split anything containing `::` -- as this did --
/// left every duplicate C++ local out of the resolver entirely (review).
///
/// The prefix must also look like a source file, so an unqualified name that
/// merely contains a colon is not mistaken for a qualified one.
fn split_tu(name: &str) -> (Option<&str>, &str) {
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b':' {
            let doubled = b.get(i + 1) == Some(&b':') || (i > 0 && b[i - 1] == b':');
            if !doubled {
                let (head, tail) = (&name[..i], &name[i + 1..]);
                let file_like = head.contains('.') && !head.contains([' ', '<', '(', ':']);
                return if file_like && !tail.is_empty() { (Some(head), tail) } else { (None, name) };
            }
        }
        i += 1;
    }
    (None, name)
}

/// The last path component.
fn base_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// `.su` records by function name: each record's (file, bytes, dynamic).
type SuByName = HashMap<String, Vec<(String, u64, bool)>>;

fn su_by_name(records: Vec<stack::SuRecord>) -> SuByName {
    let mut out = SuByName::new();
    for r in records {
        out.entry(r.name).or_default().push((r.file, r.bytes, r.dynamic));
    }
    out
}

/// The `.su` record for a symbol tied to no coverage function: any record
/// of its name, worst case first. Nothing here is held to the missing-.su
/// guard, so a guess cannot hide a missing record.
fn su_unplaced(su: &SuByName, dem: &str, cov_name: &str) -> Option<(u64, bool)> {
    let keys: &[&str] = if dem == cov_name { &[dem] } else { &[dem, cov_name] };
    keys.iter().filter_map(|k| su.get(*k)).flatten().map(|(_, b, d)| (*b, *d)).reduce(worst)
}

fn worst((b1, d1): (u64, bool), (b2, d2): (u64, bool)) -> (u64, bool) {
    (b1.max(b2), d1 || d2)
}

/// An analysed symbol the `.su` records must account for: its address, its
/// two names, and the file that defines the coverage function it was tied
/// to -- or, for a copy tied to none (llvm-cov keeps one of two same-named
/// statics), the unit the DWARF says emitted it. That copy is not held to
/// the missing-.su guard, but its unit is still a file a record could name,
/// so a record that also matches it is ambiguous.
struct SuSlot<'a> {
    addr: u64,
    dem: &'a str,
    cov_name: &'a str,
    tu: &'a str,
}

/// The `-fstack-usage` frame at each analysed address whose records are all
/// present (#10). An address with none here has no stated frame of its own.
///
/// A record names the file that *defines* the function, not the unit that
/// emitted it, so two checks stand in for the emitting unit:
///
/// - A record is assigned to a defining file only when its path picks out
///   exactly one of the files that define a function of its name. A bare
///   `dup.c` matches both `src/alpha/dup.c` and `src/beta/dup.c`, and an
///   ambiguous record satisfies neither, as `resolve_fn` never guesses a unit.
/// - Every emitting unit writes one record per copy it emits, so a function
///   needs at least as many assigned records as it has distinct addresses. A
///   header's `static inline` compiled into geom.c and main.c is two copies
///   naming one header: with main.c's `.su` gone -- even a `.su` holding only
///   header functions -- geom.c's record no longer covers both.
///
/// The count cannot tell which copy's record is the missing one, so every
/// copy of a short function is refused. Several records for one copy (a
/// C++ inline the linker folded) keep the worst case. A C++ ctor/dtor alias
/// with no record of its own shares its address with the variant that has
/// one, so the address carries the frame for both (#266 review).
fn place_su(su: &SuByName, slots: &[SuSlot]) -> HashMap<u64, (u64, bool)> {
    // (record name, defining file) -> the distinct addresses that need it
    let mut needs: BTreeMap<(&str, &str), BTreeSet<u64>> = BTreeMap::new();
    for s in slots {
        let key = if su.contains_key(s.dem) { s.dem } else { s.cov_name };
        if su.contains_key(key) {
            needs.entry((key, s.tu)).or_default().insert(s.addr);
        }
    }
    let mut files_of: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, tu) in needs.keys() {
        files_of.entry(name).or_default().push(tu);
    }
    let mut assigned: HashMap<(&str, &str), Vec<(u64, bool)>> = HashMap::new();
    for (name, tus) in &files_of {
        for (file, b, d) in &su[*name] {
            let mut hits = tus.iter().filter(|t| file.is_empty() || same_tu(file, t));
            if let (Some(tu), None) = (hits.next(), hits.next()) {
                assigned.entry((name, tu)).or_default().push((*b, *d));
            }
        }
    }
    let mut out = HashMap::new();
    for (key, addrs) in &needs {
        let Some(recs) = assigned.get(key).filter(|r| r.len() >= addrs.len()) else { continue };
        let frame = recs.iter().copied().reduce(worst).expect("at least one address, so one record");
        // Two differently-named functions folded onto one address (ICF) are
        // one body: keep the worst of their frames rather than whichever
        // name came last.
        for a in addrs {
            out.entry(*a).and_modify(|cur| *cur = worst(*cur, frame)).or_insert(frame);
        }
    }
    out
}

/// Do two paths name the same translation unit?
///
/// llvm-cov's qualifier and the DWARF unit's name are both source paths but
/// not necessarily in the same form -- one may be relative to the build
/// directory and the other absolute. Compare component-wise from the right
/// and accept when one is a suffix of the other, so `src/foo.c` matches
/// `/work/src/foo.c` and does *not* match `tests/foo.c`. Comparing basenames
/// alone collapsed those two into one unit (review).
///
/// `.` and `..` are resolved first. make (and any build that compiles from a
/// subdirectory) invokes the compiler with a relative path like
/// `../../c-lib/src/util.c`, so the DWARF unit name carries the `..` verbatim
/// while the coverage path is absolute. Compared literally, the `..`
/// components line up against real directory names (`examples`, `IDE`) and
/// never match -- which silently dropped object attribution for *every*
/// internal-linkage function under such a build (#170). Normalising resolves
/// each `..` against the component before it; a leading `..` that cannot be
/// resolved marks where the comparable suffix begins, since nothing above it
/// is known.
fn same_tu(a: &str, b: &str) -> bool {
    // Normalise, then keep only the components after any unresolved leading
    // `..` -- that trailing run is the part of the path we can actually
    // compare.
    fn comparable(p: &str) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for c in p.split(['/', '\\']) {
            match c {
                "" | "." => {}
                ".." if matches!(out.last(), Some(x) if *x != "..") => {
                    out.pop();
                }
                _ => out.push(c),
            }
        }
        let start = out.iter().rposition(|c| *c == "..").map_or(0, |i| i + 1);
        out.split_off(start)
    }
    let ac = comparable(a);
    let bc = comparable(b);
    let n = ac.len().min(bc.len());
    n > 0 && ac[ac.len() - n..] == bc[bc.len() - n..]
}

/// Invert `table_of` (function symbol -> the data tables that hold it) into
/// table name -> the function symbols it holds.
///
/// Takes the entries as an iterator rather than the map itself so a caller
/// -- a test -- can fix the order they arrive in. Over a `HashMap` that
/// order is the process's hash seed's, which Rust randomises per process,
/// and it decides the order of each returned `Vec` (#215).
fn tables_by_name<'a>(entries: impl Iterator<Item = (usize, &'a [String])>) -> HashMap<String, Vec<usize>> {
    let mut tables: HashMap<String, Vec<usize>> = HashMap::new();
    for (t, names) in entries {
        for n in names {
            tables.entry(n.clone()).or_default().push(t);
        }
    }
    tables
}

/// The static candidates for one function's indirect calls: the function
/// symbols it takes the address of, plus every function held in a data
/// table it reads.
///
/// Ordered by symbol address, the symbol id breaking a tie between aliases,
/// so the list is a function of the binary and nothing else.
///
/// The sort is the point. `tables`' values arrive in `HashMap` iteration
/// order and this list is serialised into `evidence.json`, so without it two
/// ingests of one unchanged build produced different evidence -- 21 runs one
/// way and 19 the other, over 40 ingests of `examples/c/build` (#215). A
/// qualification tool's output has to be a function of its inputs alone.
/// Sorting here rather than making `table_of` a `BTreeMap` fixes the order
/// whatever route an id arrives by, including the `takes` list and anything
/// added later; the map's type only fixes one of those routes.
fn candidate_ids<'a>(takes: &[usize], reads: impl Iterator<Item = &'a str>, tables: &HashMap<String, Vec<usize>>, addr_of: impl Fn(usize) -> u64) -> Vec<usize> {
    let mut ids: Vec<usize> = takes.to_vec();
    for name in reads {
        for &t in tables.get(name).into_iter().flatten() {
            if !ids.contains(&t) {
                ids.push(t);
            }
        }
    }
    ids.sort_by_key(|&t| (addr_of(t), t));
    ids
}

/// Trace transitions resolved to measured indirect calls: a transition
/// whose source translation block ends with an indirect call and whose
/// target is a symbol's entry point is one call through that site, made
/// `n` times. Returns `(caller function, site index, target symbol, n)`.
///
/// Ordered by call site and then by target address, so a site's *measured*
/// targets read in the same order as its *candidates* and a reader can put
/// the two lists side by side.
///
/// The ordering is not cosmetic. The transitions live in a `HashMap`, and
/// these entries are pushed straight into `indirect_calls[].measured` and
/// into the trace `pointer_callers`, both of which are serialised: without
/// the sort the two lists came out in the process's hash-seed order.
/// Measured on `examples/rust-qemu` (which is not on main -- see #164),
/// `dispatch`'s two measured targets came out one way in 13 of 20 ingests
/// of one unchanged build and the other way in 7. Same defect as the
/// static candidate list above, on the measured path (#215).
///
/// Like `tables_by_name`, the entries arrive as an iterator rather than as
/// the map, so a test can fix the order they come in.
fn resolved_calls<'a>(
    transitions: impl Iterator<Item = (&'a (u64, u64), &'a u64)>,
    tb_last: &HashMap<u64, u64>,
    site_of: &HashMap<u64, (usize, usize)>,
    sym_by_entry: &HashMap<u64, usize>,
    addr_of: impl Fn(usize) -> u64,
) -> Vec<(usize, usize, usize, u64)> {
    let mut resolved: Vec<(usize, usize, usize, u64)> = Vec::new();
    for ((from_tb, to_tb), n) in transitions {
        let Some(&last) = tb_last.get(from_tb) else { continue };
        let Some(&(fid, si)) = site_of.get(&last) else { continue };
        let Some(&t) = sym_by_entry.get(to_tb) else { continue };
        resolved.push((fid, si, t, *n));
    }
    resolved.sort_by_key(|&(fid, si, t, _)| (fid, si, addr_of(t), t));
    resolved
}

pub fn ingest(opts: &IngestOptions) -> Result<Evidence> {
    // one budget for the whole run: see crates/model/src/limits.rs (#5)
    let lim = limits::Limits::from_env();
    let mut inputs: Vec<InputArtifact> = Vec::new();
    let mut external_tools: Vec<ExternalTool> = Vec::new();
    let mut caveats = vec![
        "Object-code coverage is PROJECTED from source-level coverage through the DWARF line table; it is not measured. Treat green instructions as 'implied by source coverage', not as executed.".to_string(),
        "Source coverage comes from an instrumented build; the object code shown is the non-instrumented build of the same sources.".to_string(),
    ];

    // --- project configuration -----------------------------------------------
    // A config found beside the sources or the export is found by its name,
    // and read as such (#341).
    let by_name = |p: &Path, flag: &'static str| opts.found_by_name.iter().any(|f| f == p).then_some(flag);
    let (cfg_path, cfg_by_name) = match opts.config.clone() {
        Some(p) => {
            let n = by_name(&p, "--config");
            (Some(p), n)
        }
        None => {
            let candidates = [opts.source_root.clone().map(|r| r.join("butterfly.toml")), opts.coverage_json.parent().map(|d| d.join("butterfly.toml"))];
            (candidates.into_iter().flatten().find(|p| p.symlink_metadata().is_ok()), Some("--config"))
        }
    };
    // read once, under the budget: the bytes serve the parse and, further
    // down, the provenance hash
    let cfg_text = match &cfg_path {
        Some(p) => Some(read_input(p, limits::Kind::Auxiliary, &lim, cfg_by_name)?),
        None => None,
    };
    let config = match (&cfg_path, &cfg_text) {
        (Some(p), Some(t)) if cfg_by_name.is_some() => config::Config::parse_untrusted(t, p)?,
        (Some(p), Some(t)) => config::Config::parse(t, p)?,
        _ => config::Config::default(),
    };
    let tags = config.finder()?;

    // --- coverage --------------------------------------------------------
    let cov_text = read_input(&opts.coverage_json, limits::Kind::Coverage, &lim, by_name(&opts.coverage_json, "--coverage"))?;
    inputs.push(artifact("coverage-json", &opts.coverage_json, cov_text.as_bytes()));
    // Which front end: the export says which tool produced it, so this is
    // not a guess and not a flag the operator has to remember (#45).
    let cov = if gcov::looks_like(&cov_text) { gcov::parse(&cov_text)? } else { coverage::parse(&cov_text)? };
    // what the export itself could not say (older llvm-cov record layouts)
    caveats.extend(cov.caveats.iter().cloned());

    // --- source files ----------------------------------------------------
    // The filenames come from the coverage export, and their contents are
    // embedded whole in the evidence. Open, bound and read in one step, so
    // the file that was checked is the file that was read (#1).
    let names: Vec<&str> = cov.files.iter().map(|f| f.filename.as_str()).collect();
    let bound = contained_sources(&names, opts.source_root.as_deref(), opts.allow_external_source, &lim)?;
    if !bound.outside.is_empty() {
        if opts.allow_external_source {
            caveats.push(format!(
                "{} source file(s) were read from outside the source root {} under --allow-external-source, and their contents are embedded in this evidence: {}",
                bound.outside.len(),
                bound.root.display(),
                bound.outside.join(", ")
            ));
        } else {
            bail!(
                "{n} source file(s) named by the coverage export resolve outside the source root and were NOT read:\n  {list}\n\nsource root: {root}\n\nCoverage filenames are untrusted input and their contents are embedded whole in the evidence, so butterfly reads only what is inside the source root. Point --source-root at the directory that contains these files, or pass --allow-external-source to read them anyway.",
                n = bound.outside.len(),
                list = bound.outside.join("\n  "),
                root = bound.root.display()
            );
        }
    }
    if !bound.linked.is_empty() {
        let list: Vec<String> = bound.linked.iter().map(|(name, n)| format!("{name} ({n} hard links)")).collect();
        if opts.allow_external_source {
            caveats.push(format!(
                "{} source file(s) have more than one name (hard links), so whether each is inside the source root {} could not be established; they were read under --allow-external-source and their contents are embedded in this evidence: {}",
                list.len(),
                bound.root.display(),
                list.join(", ")
            ));
        } else {
            bail!(
                "{n} source file(s) named by the coverage export have more than one name (hard links) and were NOT read:\n  {list}\n\nsource root: {root}\n\nFor each, {why}: an in-root name for a file that also lives outside the root would otherwise pass. Replace the link with a copy, or pass --allow-external-source to read them anyway.",
                n = list.len(),
                list = list.join("\n  "),
                root = bound.root.display(),
                why = fsauth::LINKED_REASON
            );
        }
    }
    let mut files: Vec<SourceFile> = Vec::new();
    let mut file_index: HashMap<String, usize> = HashMap::new();
    let mut basename_index: HashMap<String, Vec<usize>> = HashMap::new();
    let root = opts.source_root.clone().or_else(|| common_root(cov.files.iter().map(|f| f.filename.as_str())));
    for cf in &cov.files {
        let path = Path::new(&cf.filename);
        // the bytes the containment check itself read, off the descriptor it
        // checked -- never a second lookup of the export's string
        let Some((_, bytes)) = bound.read.get(&cf.filename) else {
            let why = bound.failed.get(&cf.filename).map_or("it could not be opened", String::as_str);
            bail!("reading source {}: {why}", cf.filename);
        };
        inputs.push(artifact("source", path, bytes));
        let text: Vec<String> = String::from_utf8_lossy(bytes).lines().map(str::to_string).collect();
        let counts = coverage::line_counts(cf, text.len());
        let lines: Vec<LineCov> = counts.iter().map(|c| LineCov { count: *c }).collect();
        let lines_total = counts.iter().filter(|c| c.is_some()).count();
        let lines_covered = counts.iter().filter(|c| matches!(c, Some(n) if *n > 0)).count();
        let id = files.len();
        let norm = objectcode::normalize(path);
        file_index.insert(norm.clone(), id);
        if let Some(b) = path.file_name().and_then(|b| b.to_str()) {
            basename_index.entry(b.to_string()).or_default().push(id);
        }
        files.push(SourceFile {
            id,
            path: cf.filename.clone(),
            display: display_path(&cf.filename, root.as_deref()),
            sha256: sha_hex(bytes),
            text,
            lines,
            function_ids: Vec::new(),
            lines_total,
            lines_covered,
        });
    }
    let resolve_file = |p: &str| -> Option<usize> {
        let norm = objectcode::normalize(Path::new(p));
        if let Some(id) = file_index.get(&norm) {
            return Some(*id);
        }
        let b = Path::new(p).file_name()?.to_str()?;
        match basename_index.get(b) {
            Some(v) if v.len() == 1 => Some(v[0]),
            _ => None,
        }
    };

    // --- functions from coverage --------------------------------------------
    let mut functions: Vec<Function> = Vec::new();
    let mut fn_by_name: HashMap<String, usize> = HashMap::new();
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    // The coverage records behind each function, under the key the merge
    // below uses, so a function's own line counts can be recovered from its
    // own regions when the file's are shared with another function.
    let mut records_of: HashMap<(String, usize, u32, u32), Vec<&coverage::CovFunction>> = HashMap::new();
    let mut unplaced_decisions: Vec<String> = Vec::new();
    for cfn in &cov.functions {
        let Some(body) = cfn.regions.iter().find(|r| r.kind == coverage::KIND_CODE) else { continue };
        let fname = cfn.filenames.get(body.file_id).cloned().unwrap_or_default();
        let Some(file_id) = resolve_file(&fname) else {
            *skipped.entry(display_path(&fname, root.as_deref())).or_default() += 1;
            continue;
        };
        let line_start = body.line_start;
        let line_end = cfn.regions.iter().filter(|r| r.file_id == body.file_id && r.kind == coverage::KIND_CODE).map(|r| r.line_end).max().unwrap_or(body.line_end);
        let n_lines = files[file_id].text.len();
        if line_start == 0 || line_start > line_end || line_end as usize > n_lines {
            bail!("coverage export claims function `{}` spans lines {line_start}..={line_end} of {}, which has {n_lines} line(s)", demangle(&cfn.name), files[file_id].display);
        }

        // A branch inside a macro expansion is located by llvm-cov at the
        // `#define`, in a file id of its own. It is placed at the macro's use
        // in the body instead: that is where DWARF puts its object code, and
        // where a reader looks for it. Dropping it, as was done, left the
        // expansion's object branches with no source decision, and its
        // never-run path projected covered from the use line's count (#168).
        // Every expansion's use in the body, found in one pass (#312).
        let body_sites = coverage::Expansions::new(&cfn.regions).sites_in(body.file_id);
        let placed = |b: &coverage::RawBranch| -> Option<Branch> {
            let (line, col, line_end, col_end) = if b.file_id == body.file_id {
                (b.line_start, b.col_start, b.line_end, b.col_end)
            } else {
                let s = body_sites.get(&b.file_id)?;
                (s.line_start, s.col_start, s.line_end, s.col_end)
            };
            Some(Branch {
                line,
                col,
                line_end,
                col_end,
                true_count: b.true_count,
                false_count: b.false_count,
                direction_known: b.direction_known,
                decision_ordinal: b.decision_ordinal,
            })
        };
        let mut placed_branches: Vec<_> = cfn.branches.iter().filter_map(|b| Some((placed(b)?, (b.file_id, b.line_start, b.col_start, b.line_end, b.col_end)))).collect();
        placed_branches.sort_by_key(|(b, orig)| (b.line, b.col, b.decision_ordinal, *orig));
        {
            let mut site_counts: BTreeMap<(u32, u32), usize> = BTreeMap::new();
            let mut next_ord: BTreeMap<u32, u32> = BTreeMap::new();
            for (b, _) in &placed_branches {
                if let Some(ord) = b.decision_ordinal {
                    let e = next_ord.entry(b.line).or_insert(0);
                    *e = (*e).max(ord.saturating_add(1));
                } else {
                    *site_counts.entry((b.line, b.col)).or_default() += 1;
                }
            }
            for (b, _) in &mut placed_branches {
                if b.decision_ordinal.is_none() && site_counts.get(&(b.line, b.col)).copied().unwrap_or(0) > 1 {
                    let n = next_ord.entry(b.line).or_insert(0);
                    b.decision_ordinal = Some(*n);
                    *n += 1;
                }
            }
        }
        let branches: Vec<Branch> = placed_branches.into_iter().map(|(b, _)| b).collect();

        // An MC/DC decision is placed like its branches: one inside a macro
        // at the macro's use, its conditions in the #define's order (#269).
        // One whose file cannot be established is not shown, and the
        // caveat below says so; placing it by its raw coordinates put it at
        // the #define line, with a false predicated-decision gap.
        let placed_decisions = coverage::place_decisions(cfn, &coverage::decision_files(cfn), body.file_id);
        let mut placed_decs: Vec<(Decision, (u32, u32, u32, u32))> = Vec::new();
        for (m, p) in cfn.mcdc.iter().zip(placed_decisions) {
            let Some(p) = p else {
                unplaced_decisions.push(format!("{} (record at L{}:{})", demangle(&cfn.name), m.line_start, m.col_start));
                continue;
            };
            let conditions: Vec<Branch> = p.conditions.iter().filter_map(|&k| placed(&cfn.branches[k])).collect();
            let at = coverage::RawMcdc { line_start: p.line_start, col_start: p.col_start, line_end: p.line_end, col_end: p.col_end, ..m.clone() };
            placed_decs.push((build_decision(&at, &conditions), (m.line_start, m.col_start, m.line_end, m.col_end)));
        }
        placed_decs.sort_by_key(|(d, orig)| (d.line, d.col, *orig));
        {
            let mut site_counts: BTreeMap<(u32, u32), usize> = BTreeMap::new();
            for (d, _) in &placed_decs {
                *site_counts.entry((d.line, d.col)).or_default() += 1;
            }
            let mut next_ord: BTreeMap<u32, u32> = BTreeMap::new();
            for (d, _) in &mut placed_decs {
                if site_counts.get(&(d.line, d.col)).copied().unwrap_or(0) > 1 {
                    let n = next_ord.entry(d.line).or_insert(0);
                    d.decision_ordinal = Some(*n);
                    *n += 1;
                }
            }
        }
        let decisions: Vec<Decision> = placed_decs.into_iter().map(|(d, _)| d).collect();

        let file = &files[file_id];
        let lines_total = (line_start..=line_end).filter(|l| file.lines.get((*l - 1) as usize).map(|c| c.count.is_some()).unwrap_or(false)).count();
        let lines_covered = (line_start..=line_end).filter(|l| file.lines.get((*l - 1) as usize).map(|c| matches!(c.count, Some(n) if n > 0)).unwrap_or(false)).count();

        let requirements = tags.find(&file.text, line_start, line_end);
        let line_requirements = tags.find_lines(&file.text, line_start, line_end);
        let name = demangle(&cfn.name);
        records_of.entry((name.clone(), file_id, line_start, line_end)).or_default().push(cfn);
        let id = functions.len();
        fn_by_name.insert(cfn.name.clone(), id);
        fn_by_name.insert(name.clone(), id);
        // The construct inventory reads the body plus any immediately-preceding
        // `template<...>` declaration line. The coverage line range for a
        // template function starts at the signature, one line below the
        // `template` prefix, so the C++ vocabulary would otherwise never see
        // that a function is a template. Only a contiguous run of `template`
        // lines directly above the signature is pulled in; nothing else.
        let construct_start = {
            // `template` as a keyword: the word must end there (next is `<`,
            // whitespace, or nothing), so a line beginning `templated…` is not
            // mistaken for a template prefix.
            let is_template_prefix = |line: &str| line.trim_start().strip_prefix("template").is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'));
            let mut s = (line_start as usize - 1).min(file.text.len());
            while s > 0 && is_template_prefix(&file.text[s - 1]) {
                s -= 1;
            }
            s
        };
        let complexity = Complexity {
            cyclomatic_object: None,
            decisions: decisions.len().max(branches.len()) as u32,
            conditions: if decisions.is_empty() { branches.len() as u32 } else { decisions.iter().map(|d| d.conditions.len() as u32).sum() },
            max_conditions_per_decision: decisions.iter().map(|d| d.conditions.len() as u32).max().unwrap_or(if branches.is_empty() { 0 } else { 1 }),
            ..Default::default()
        };
        functions.push(Function {
            id,
            name,
            mangled: cfn.name.clone(),
            file_id,
            line_start,
            line_end,
            count: cfn.count,
            requirements,
            tests: Vec::new(),
            line_requirements,
            lines_total,
            lines_covered,
            branches,
            decisions,
            complexity,
            callees: Vec::new(),
            callers: Vec::new(),
            object: None,
            variants: Vec::new(),
            instantiations: 1,
            symbol_id: None,
            data_refs: Vec::new(),
            variables: Vec::new(),
            inlined: Vec::new(),
            own_lines: None,
            folded_calls: Vec::new(),
            indirect_calls: Vec::new(),
            pointer_callers: Vec::new(),
            commits: vec![],
            origin: None,
            unrecorded_within: None,
            constructs: constructs::inventory(&file.text[construct_start..(line_end as usize).min(file.text.len())], constructs::Lang::from_path(&file.path)),
        });
    }
    if !unplaced_decisions.is_empty() {
        caveats.push(format!(
            "{} MC/DC decision record(s) are not shown: the file each is written in could not be established, so neither its line nor its conditions are known. The LLVM 18 export layout names no file for a decision; it is recovered from the decision's condition regions, and here they did not settle it (e.g. two uses of one macro in a function, with different results). Their MC/DC evidence is not reported (#269): {}",
            unplaced_decisions.len(),
            unplaced_decisions.join(", ")
        ));
    }
    if !skipped.is_empty() {
        let total: usize = skipped.values().sum();
        let list: Vec<String> = skipped.iter().map(|(f, n)| format!("{f} ({n})")).collect();
        caveats.push(format!(
            "{total} function(s) in the coverage export were skipped because their file is not in the export's file set (typically test code excluded by the coverage tool): {}",
            list.join(", ")
        ));
    }
    // ---- merge instantiations of the same source function -------------
    //
    // llvm-cov emits one record per instantiation: a generic instantiated for
    // several types, or — more often here — the same crate compiled twice in
    // one build graph under different `-Cmetadata` hashes. They are the same
    // source function (same file and line range), and llvm-cov has already
    // *summed* them in the file's line counts, so sum them here too; otherwise
    // one entry claims the object code and its twin looks like it has none.
    {
        let mut merged: Vec<Function> = Vec::with_capacity(functions.len());
        let mut index: HashMap<(String, usize, u32, u32), usize> = HashMap::new();
        for f in functions.drain(..) {
            let key = (f.name.clone(), f.file_id, f.line_start, f.line_end);
            match index.get(&key) {
                None => {
                    index.insert(key, merged.len());
                    merged.push(f);
                }
                Some(&at) => {
                    let t = &mut merged[at];
                    t.instantiations += 1;
                    t.count = t.count.saturating_add(f.count);
                    for b in f.branches {
                        // decision_ordinal is part of the identity, not a
                        // decoration. gcov gives every decision on a line the
                        // same (line, col=0), so without it two decisions in
                        // `return a && b;` match each other and the second
                        // instantiation's counts are added to the first --
                        // marking one decision covered and losing the other.
                        // Which one wins then depends on document order
                        // (review of #45).
                        match t
                            .branches
                            .iter_mut()
                            .find(|x| x.line == b.line && x.col == b.col && x.line_end == b.line_end && x.col_end == b.col_end && x.decision_ordinal == b.decision_ordinal)
                        {
                            Some(x) => {
                                x.true_count = x.true_count.saturating_add(b.true_count);
                                x.false_count = x.false_count.saturating_add(b.false_count);
                            }
                            None => t.branches.push(b),
                        }
                    }
                    for d in f.decisions {
                        match t
                            .decisions
                            .iter_mut()
                            .find(|x| x.line == d.line && x.col == d.col && x.decision_ordinal == d.decision_ordinal && x.conditions.len() == d.conditions.len())
                        {
                            Some(x) => {
                                x.true_count = x.true_count.saturating_add(d.true_count);
                                x.false_count = x.false_count.saturating_add(d.false_count);
                                // one vector per (values, result), executed if
                                // any record executed it: an unexecuted slot
                                // read first must not hide another record's run
                                // of the same values, or the pair it made is
                                // lost depending on record order (#325 B3)
                                for v in d.vectors {
                                    match x.vectors.iter_mut().find(|y| y.values == v.values && y.result == v.result) {
                                        Some(y) => y.executed |= v.executed,
                                        None => x.vectors.push(v),
                                    }
                                }
                                for (i, c) in d.conditions.into_iter().enumerate() {
                                    if let Some(y) = x.conditions.get_mut(i) {
                                        y.true_count = y.true_count.saturating_add(c.true_count);
                                        y.false_count = y.false_count.saturating_add(c.false_count);
                                        y.independence_covered |= c.independence_covered;
                                    }
                                }
                                // the extra vectors may now form an independence pair
                                for i in 0..x.conditions.len() {
                                    if let Some(p) = find_pair(&x.vectors, i) {
                                        x.conditions[i].pair = Some(p);
                                        x.conditions[i].independence_covered = true;
                                    }
                                }
                                x.mcdc_covered = !x.conditions.is_empty() && x.conditions.iter().all(|c| c.independence_covered);
                            }
                            None => t.decisions.push(d),
                        }
                    }
                    // ...and the ordinal orders them too. Sorting on
                    // (line, col) alone leaves gcov's decisions, which all
                    // share col 0, in whatever order the merge happened to
                    // append them.
                    t.branches.sort_by_key(|b| (b.line, b.col, b.decision_ordinal));
                    t.decisions.sort_by_key(|d| (d.line, d.col, d.decision_ordinal));
                }
            }
        }
        let dupes: u32 = merged.iter().map(|f| f.instantiations - 1).sum();
        if dupes > 0 {
            caveats.push(format!(
                "{dupes} additional coverage record(s) were merged into {} source function(s): llvm-cov reports one record per instantiation (a generic instantiated for several types, or the same crate compiled more than once in the build graph). Counts are summed, matching llvm-cov's own per-line totals.",
                merged.iter().filter(|f| f.instantiations > 1).count()
            ));
        }
        functions = merged;
    }
    functions.sort_by_key(|f| (f.file_id, f.line_start));
    for (i, f) in functions.iter_mut().enumerate() {
        f.id = i;
    }
    // ---- functions that share their source lines with another ----------
    //
    // A generic monomorphised at two types, or a C++ template instantiated
    // at two, is one source range and two functions with records of their
    // own (`limit::<i32>`, `limit::<u64>`). The file's line counts are the
    // sum over both, as llvm-cov makes them; a line one copy never reached
    // is counted there by the other. Projected from those, each copy's lines
    // and object code read as covered wherever *either* ran (#167). Each is
    // given its own counts instead, recovered from its own records' regions
    // by the rules llvm-cov builds the file's with. The file view keeps the
    // sum: it is a statement about the source, and the source line did run.
    let own_lines: HashMap<usize, Vec<Option<u64>>> = {
        let mut sharing: HashMap<(usize, u32, u32), Vec<usize>> = HashMap::new();
        for f in &functions {
            sharing.entry((f.file_id, f.line_start, f.line_end)).or_default().push(f.id);
        }
        let mut own = HashMap::new();
        for ids in sharing.values().filter(|ids| ids.len() > 1) {
            for &id in ids {
                let f = &functions[id];
                let file = &files[f.file_id];
                let Some(recs) = records_of.get(&(f.name.clone(), f.file_id, f.line_start, f.line_end)) else { continue };
                let counts = coverage::records_line_counts(recs.iter().copied(), &file.path, file.text.len());
                own.insert(id, counts);
            }
        }
        for (&id, counts) in &own {
            let f = &mut functions[id];
            // exactly the function's range, clamped to the file's length so an
            // invalid range cannot allocate unbounded memory
            let last = files[f.file_id].text.len().min(u32::MAX as usize) as u32;
            let span: Vec<Option<u64>> = (f.line_start.max(1)..=f.line_end.min(last)).map(|l| counts.get(l as usize - 1).copied().flatten()).collect();
            f.lines_total = span.iter().filter(|c| c.is_some()).count();
            f.lines_covered = span.iter().filter(|c| matches!(c, Some(n) if *n > 0)).count();
            // carried in the evidence, so the figures above can be checked
            // against what they were drawn from (#276)
            f.own_lines = Some(span);
        }
        own
    };
    // The count a function's object code is projected from, for a line of
    // `file_id`: its own where it has its own and the line is in its range,
    // the file's otherwise.
    let line_count = |files: &[SourceFile], f: &Function, file_id: usize, line: u32| -> Option<u64> {
        if file_id == f.file_id && (f.line_start..=f.line_end).contains(&line) {
            if let Some(own) = own_lines.get(&f.id) {
                return own.get((line.max(1) - 1) as usize).copied().flatten();
            }
        }
        files[file_id].lines.get((line.max(1) - 1) as usize).and_then(|c| c.count)
    };
    fn_by_name.clear();
    // llvm-cov names an internal-linkage C function `file.c:name`, while the
    // symbol table only ever knows the bare `name`. So a bare name may stand
    // for a function only while exactly one function answers to it. Several
    // can: two units that both include a header's `static inline` define one
    // each (`geom.c:geom_clamp`, `main.c:geom_clamp`), and a `static helper`
    // in one unit sits happily beside an extern `helper` in another -- there
    // the claimants do not even share a name shape. Resolving a contested
    // bare name to any of them credits one unit's object code to another
    // unit's function (#36), so contested names are kept out of the map
    // entirely and resolved per symbol through the DWARF, which knows which
    // unit emitted the code at a given address.
    let mut bare_names: HashMap<String, Vec<usize>> = HashMap::new();
    // bare name -> every (translation unit, function) answering to it. The
    // unit is kept whole, not reduced to a basename: src/foo.c and
    // tests/foo.c are different units.
    let mut fn_by_place: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    for f in &functions {
        fn_by_name.insert(f.mangled.clone(), f.id);
        fn_by_name.insert(f.name.clone(), f.id);
        let (qual, bare) = split_tu(&f.name);
        // A qualified name carries its unit; an unqualified one belongs to
        // the file it was found in. A header function is always qualified,
        // which matters because its file is the *header*, not the unit.
        // llvm-cov's qualifier is only a basename -- two statics named
        // `tally`, one in src/alpha/dup.c and one in src/beta/dup.c, are
        // *both* `dup.c:tally`. When the qualifier names this function's own
        // file, take the file's fuller path instead, which does tell them
        // apart. A header function keeps the qualifier: `geom.c:geom_clamp`
        // lives in inc/geom.h, and geom.c is the unit that compiled it.
        // `path` and not `display`: display shortens to a basename, so two
        // files really called src/alpha/dup.c and src/beta/dup.c both show as
        // `dup.c` and would compare equal here.
        let path = &files[f.file_id].path;
        let tu = match qual {
            Some(q) if base_name(q) == base_name(path) => path.clone(),
            Some(q) => q.to_string(),
            None => path.clone(),
        };
        bare_names.entry(bare.to_string()).or_default().push(f.id);
        fn_by_place.entry(bare.to_string()).or_default().push((tu, f.id));
    }
    // A bare name never resolves on its own here. A contested one is removed
    // outright -- an unqualified claimant put it in the map under its own
    // full name, ahead of this gate -- and an uncontested one is left for
    // resolve_fn to reach only after the DWARF has had its say, because the
    // one function of that name may belong to a different unit than the
    // symbol in hand (llvm-cov names two same-named statics identically and
    // keeps a record for only one of them).
    for (bare, ids) in &bare_names {
        if ids.len() > 1 {
            fn_by_name.remove(bare.as_str());
        }
    }
    for f in &functions {
        files[f.file_id].function_ids.push(f.id);
    }

    // --- object code -----------------------------------------------------
    let mut object_format = String::new();
    let mut arch = String::new();
    let mut build_info = BTreeMap::new();
    build_info.insert("requirement_patterns".into(), tags.patterns.join("  |  "));
    build_info.insert("requirement_scope".into(), format!("{} (lookback {})", tags.scope, tags.lookback));
    if let (Some(p), Some(t)) = (&cfg_path, &cfg_text) {
        inputs.push(artifact("config", p, t.as_bytes()));
    }
    if let Some(bi) = &opts.build_info {
        let t = read_input(bi, limits::Kind::Auxiliary, &lim, by_name(bi, "--build-info"))?;
        inputs.push(artifact("build-info", bi, t.as_bytes()));
        for line in t.lines() {
            if let Some((k, v)) = line.split_once('=') {
                build_info.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }

    // What the analysis build says it did. A build that declares `-g` or
    // `-fstack-usage` has promised the evidence a line table and a stated
    // frame for every function; below, what it delivered is held to that,
    // whatever adapter produced the directory (#245).
    let declared = build_info.get("flags_target").map(|f| DeclaredMode::of(f));
    if let (Some(d), false) = (&declared, opts.objects.is_empty()) {
        if !d.debug {
            bail!(
                "build-info.txt records the analysis build's flags as `{}`, which leave it without debug information (no -g, or a later -g0). Object code cannot be mapped to source without it: build the analysed image with -g, and record the flags it was really built with.",
                build_info["flags_target"]
            );
        }
    }
    let mut stated_frames_missing: Vec<String> = Vec::new();

    let mut symbols: Vec<Symbol> = Vec::new();
    let mut globals: Vec<Global> = Vec::new();
    let mut image: Option<Image> = None;
    let mut measured: Option<Measured> = None;
    let mut measured_counts: Option<HashMap<u64, u64>> = None;
    let mut su_entries: Vec<stack::SuRecord> = Vec::new();
    for su in &opts.stack_usage {
        let t = read_input(su, limits::Kind::Auxiliary, &lim, by_name(su, "--stack-usage"))?;
        inputs.push(artifact("stack-usage", su, t.as_bytes()));
        su_entries.extend(stack::parse_su_records(&t));
    }
    let su_map = su_by_name(su_entries);
    if declared.as_ref().is_some_and(|d| !d.stack_usage) && !opts.stack_usage.is_empty() {
        caveats.push(format!(
            "{} .su file(s) were read, but build-info.txt records the analysis build's flags without -fstack-usage: the provenance does not describe the build that produced this stack evidence. Record the flags the build really used.",
            opts.stack_usage.len()
        ));
    }
    let known_symbols: BTreeSet<String> = functions.iter().flat_map(|f| [f.mangled.clone(), f.name.clone()]).collect();
    // Name-only membership of the covered source set. It cannot see past a
    // unit qualifier -- `engine.cpp:(anonymous namespace)::scale(int)` is not
    // the bare symbol the object code names -- so it is the *fallback* for a
    // call whose target address the resolver cannot be given, never the first
    // answer (#206).
    let is_known = |s: &str| {
        known_symbols.contains(s)
            || known_symbols.contains(s.strip_prefix('_').unwrap_or(s))
            || known_symbols.contains(&demangle(s))
            || known_symbols.contains(&demangle(s.strip_prefix('_').unwrap_or(s)))
    };
    let demangle_fn = |s: &str| demangle(s);
    // crate / namespace prefixes of the covered code, for classifying calls
    // into project code that has no coverage evidence
    let project_prefixes: Vec<String> = {
        let mut v: BTreeSet<String> = BTreeSet::new();
        for f in &functions {
            let n = f.name.trim_start_matches('<');
            if let Some(i) = n.find("::") {
                v.insert(n[..i].to_string());
            }
        }
        v.into_iter().collect()
    };
    /// A direct call found in the object code, held until every function is
    /// attached and the graph edges can be made.
    struct PendingCall {
        caller: usize,
        callee_sym: String,
        /// Resolved at the call site, where the target address can still
        /// tell a duplicated static apart (#36).
        callee_id: Option<usize>,
        addr: u64,
        line: Option<u32>,
        /// Made from a constructor/destructor variant, which has no counter
        /// of its own: the caller's line count does not say whether this
        /// call ran (#266).
        from_variant: bool,
    }
    let mut pending_calls: Vec<PendingCall> = Vec::new();
    // Every file the DWARF line tables name: with the coverage file list,
    // the files the build used, which the source revision is judged over
    // (#300). A header can reach the object code without appearing in the
    // coverage export.
    let mut dwarf_files: BTreeSet<String> = BTreeSet::new();

    for obj_path in &opts.objects {
        // Load first, then hash what it admitted. Reading the object here to
        // hash it, ahead of the bounded load, let an oversized or growing
        // image allocate in full before the cap could refuse it -- and
        // charged the run budget once for what were several whole-file reads
        // (review of #5).
        let mut bin = if by_name(obj_path, "--object").is_some() {
            objectcode::load_discovered(obj_path, opts.llvm_bin.as_deref(), &lim)?
        } else {
            objectcode::load(obj_path, opts.llvm_bin.as_deref(), &lim)?
        };
        // Switch jump tables (#171). Their targets are the indirect branch's
        // successors, and their bytes -- which the compiler may place inside
        // the function's own symbol -- are data, not instructions: left in,
        // they were analysed as code (`udf` on AArch64, a spurious
        // `callq *%rsp` on x86-64) with a gap each. Removed once, here, so
        // every consumer of the instruction list sees the same code.
        let jump_tables: Vec<jumptable::Table> = bin
            .symbols
            .iter()
            .flat_map(|s| {
                let end = s.address.saturating_add(s.size);
                jumptable::recover(&bin.arch, bin.instrs_in(s.address, end), s.address, end, &|a, l| bin.read_at(a, l))
            })
            .collect();
        jumptable::strip(&mut bin.instrs, &jump_tables);
        let table_targets: HashMap<u64, Vec<u64>> = jump_tables.iter().map(|t| (t.branch, t.targets.clone())).collect();
        // C++ landing pads, from the image's unwind data (#165). An image
        // with an exception table this reader could not read has pads with
        // no known way in, so nothing in it is claimed unreachable.
        // An exception table this reader could not read *in full* leaves some
        // landing pads with no known way in, so nothing in the image is claimed
        // unreachable: a partial index would otherwise mislabel the pads of
        // whichever function failed to decode, even while others decoded fine
        // (#165 review).
        let (call_sites, eh_complete) = eh::call_sites(&bin);
        let eh_unread = !eh_complete;
        // At most one pad per call: the ranges are sorted and disjoint (#311).
        let pads_of = |addr: u64| -> Vec<u64> { eh::landing_pad_at(&call_sites, addr).into_iter().collect() };
        // Record the disassembler that produced this object-code evidence.
        let (objdump, objdump_caveat) = provenance::identify("llvm-objdump", &bin.objdump_path);
        provenance::record(&mut external_tools, &mut caveats, objdump, objdump_caveat);
        inputs.push(artifact("object", obj_path, &bin.bytes));
        if let (Some(d), Some(db)) = (&bin.dwarf_source, &bin.dwarf_bytes) {
            inputs.push(artifact("dwarf", d, db));
        }
        {
            let mut last: Option<&str> = None;
            for r in &bin.line_rows {
                let f: &str = r.file.as_ref();
                if last != Some(f) && !f.is_empty() && !dwarf_files.contains(f) {
                    dwarf_files.insert(r.file.to_string());
                }
                last = Some(f);
            }
        }
        if bin.line_rows.is_empty() {
            if declared.as_ref().is_some_and(|d| d.debug) {
                bail!(
                    "{}: build-info.txt declares a debug build (-g), but the image has no DWARF line table{}. It was stripped, or linked without its debug information: every instruction would have no source line. Rebuild without stripping{}.",
                    obj_path.display(),
                    if cfg!(target_os = "macos") { " and no .dSYM beside it" } else { "" },
                    if cfg!(target_os = "macos") { ", and run dsymutil on the linked image" } else { "" }
                );
            }
            caveats.push(format!("{}: no DWARF line table found; object code cannot be mapped to source (build with -g).", obj_path.display()));
        }
        if bin.stack_sizes_malformed {
            // Explicit invalid-metadata outcome (#247): the decoder stopped at
            // the first bad entry, so the reviewer must be told the stack
            // evidence is degraded rather than read a truncated list as whole.
            caveats.push(format!(
                "{}: the .stack_sizes section is malformed (an overlong or overflowing ULEB size, a truncated entry, or unreadable data); decoding stopped at the first bad entry, and a function whose frame is then only a prologue estimate is reported as not bounded, as are its callers.",
                obj_path.display()
            ));
        }
        if bin.relocatable {
            caveats.push(format!(
                "{}: this is an UNLINKED object. Its addresses are section-relative placeholders, not the addresses of the delivered image, and references between compilation units are still relocations. Call targets are recovered from branch relocations, but global-data references are not, so data coupling (A-7 objective 8) is under-reported, there is no entry point so dead-code reachability cannot be established, and stack chains stop at the unit boundary. For object-code analysis that stands up as evidence, link the library with its test harness or into the integration image and analyse that binary.",
                obj_path.display()
            ));
        }
        object_format = bin.format.clone();
        arch = bin.arch.clone();
        let is_macho = bin.format == "MachO";

        // ---- code inside a covered function, with no record of its own ----
        //
        // rustc gives a closure a coverage record only if its body maps a
        // region: `|_| count += 1` maps none, so it has no record, while its
        // code is in the image and runs (#273). Clang gives the `__invoke`
        // thunk of a lambda converted to a function pointer none either.
        // Without a record there was no function, so the code was analysed by
        // nothing -- and the line it is written on reads covered, through the
        // enclosing function's counts. A reviewer had nothing to open for it.
        //
        // Each such symbol becomes a function of its own, marked as having no
        // record and naming the covered function its name puts it inside
        // (`<fn>::{closure#N}`, `<fn>::$_N::__invoke`). Its lines are where the
        // DWARF puts its instructions, in that function's file; a symbol the
        // line table places nowhere in it is not claimed. Its object code is
        // analysed like any other, but projected from nothing: the counts on
        // its lines are the enclosing function's, and projecting them would
        // call this code covered on the strength of other code having run.
        // Every instruction is Unknown, and the gaps say why.
        //
        // Enclosers are looked up among functions that *have* records, before
        // any of these are added, so a closure inside an unrecorded closure is
        // placed in the recorded function around both.
        let mut unrecorded: Vec<Function> = Vec::new();
        for sym in bin.symbols.iter().filter(|s| s.size > 0) {
            let cov = if is_macho { sym.name.strip_prefix('_').unwrap_or(&sym.name) } else { sym.name.as_str() };
            if fn_by_name.contains_key(cov) || fn_by_name.contains_key(sym.name.as_str()) || unrecorded.iter().any(|f| f.mangled == cov) {
                continue;
            }
            let dem = demangle(cov);
            // A name some covered function answers to is resolved, or
            // reported as unresolvable, by the DWARF unit below. Not here.
            if fn_by_name.contains_key(&dem) || bare_names.contains_key(cov) || bare_names.contains_key(&dem) {
                continue;
            }
            let covered = |p: &str| {
                fn_by_name.get(p).copied().or_else(|| match bare_names.get(p).map(|ids| &ids[..]) {
                    Some([only]) => Some(*only),
                    _ => None,
                })
            };
            let Some(within) = enclosing_function(&dem, covered) else { continue };
            let file_id = functions[within].file_id;
            let lines: Vec<u32> = bin
                .instrs_in(sym.address, sym.address.saturating_add(sym.size))
                .iter()
                .filter_map(|i| bin.line_at(i.addr))
                .filter(|r| r.line > 0 && resolve_file(&r.file) == Some(file_id))
                .map(|r| r.line)
                .collect();
            let (Some(&line_start), Some(&line_end)) = (lines.iter().min(), lines.iter().max()) else { continue };
            unrecorded.push(Function {
                name: dem,
                mangled: cov.to_string(),
                file_id,
                line_start,
                line_end,
                // nothing was counted: not "never ran", and no records merged
                count: 0,
                instantiations: 0,
                unrecorded_within: Some(within),
                ..Default::default()
            });
        }
        for mut f in unrecorded {
            f.id = functions.len();
            fn_by_name.insert(f.mangled.clone(), f.id);
            fn_by_name.entry(f.name.clone()).or_insert(f.id);
            // In id order, as `Evidence::validate` requires. Being last, it is
            // also after every recorded function, so a lookup by line that
            // takes the first function around it finds the one whose counts
            // that line carries.
            files[f.file_id].function_ids.push(f.id);
            functions.push(f);
        }

        let sym_addr: HashMap<String, u64> = bin.symbols.iter().map(|s| (s.name.clone(), s.address)).collect();
        let addr_sym: BTreeMap<u64, String> = bin.symbols.iter().map(|s| (s.address, s.name.clone())).collect();

        /// What the resolver could establish about a symbol.
        ///
        /// `Option` could not say why it had no function, and one caller --
        /// the helper-call gap check -- needs the reason. A callee the DWARF
        /// places in a unit holding no covered function of that name is
        /// *definitively* outside the covered source set, and a same-named
        /// function in some other unit must not overturn that (#237). Only a
        /// missing answer may fall back to a bare name.
        enum Resolved {
            /// This symbol is that coverage function.
            Fn(usize),
            /// The DWARF named the unit that emitted this code, and no
            /// covered function of that unit answers to this name. A no.
            NotInUnit,
            /// No evidence either way: no target address, no debug info for
            /// it, or a name several units answer to indistinguishably.
            Unknown,
        }
        impl Resolved {
            /// The attribution, for the callers that only want a function.
            fn id(self) -> Option<usize> {
                match self {
                    Resolved::Fn(id) => Some(id),
                    _ => None,
                }
            }
            /// Try a second spelling of the same name. A definitive no is
            /// replaced by whatever the second spelling establishes; a
            /// *don't know* can only be upgraded to a hit, never hardened
            /// into a no it has not earned.
            fn or_spelling(self, f: impl FnOnce() -> Resolved) -> Resolved {
                match self {
                    Resolved::Fn(id) => Resolved::Fn(id),
                    Resolved::NotInUnit => f(),
                    Resolved::Unknown => match f() {
                        Resolved::Fn(id) => Resolved::Fn(id),
                        _ => Resolved::Unknown,
                    },
                }
            }
        }

        // Resolve a symbol name to its coverage function. The plain lookups
        // cover every unique name; a bare name that several translation
        // units define is resolved through the unit the DWARF says emitted
        // the code at `addr`, matched against llvm-cov's `file.c:name`
        // qualifier. No answer means no attribution -- never a guess.
        let resolve_fn = |raw: &str, addr: Option<u64>| -> Resolved {
            let cov = if is_macho { raw.strip_prefix('_').unwrap_or(raw) } else { raw };
            if let Some(&fid) = fn_by_name.get(cov).or_else(|| fn_by_name.get(raw)).or_else(|| fn_by_name.get(&demangle(cov))) {
                return Resolved::Fn(fid);
            }
            // Ask the DWARF which unit emitted the code at this address and
            // take the one function that unit compiled under this name. When
            // the DWARF can answer, its answer is final: falling back to a
            // name match here is how a symbol from a unit with no coverage
            // record gets credited to another unit's function. Two units
            // whose paths cannot be told apart leave it ambiguous, and
            // ambiguous means no attribution, never a guess.
            let one = |bare: &str, unit: &str| -> Resolved {
                let Some(claimants) = fn_by_place.get(bare) else {
                    // Nothing anywhere answers to this name, so the unit the
                    // DWARF named certainly does not.
                    return Resolved::NotInUnit;
                };
                let mut hit = None;
                for (tu, id) in claimants {
                    if same_tu(tu, unit) {
                        if hit.is_some() {
                            // Two units this comparison cannot tell apart:
                            // ambiguous, not a no.
                            return Resolved::Unknown;
                        }
                        hit = Some(*id);
                    }
                }
                match hit {
                    Some(id) => Resolved::Fn(id),
                    // The unit that emitted this code has no covered
                    // function of this name. Other units' copies are not it.
                    None => Resolved::NotInUnit,
                }
            };
            if let Some(unit) = addr.and_then(|a| bin.tu_of(a)) {
                return one(cov, unit).or_spelling(|| one(&demangle(cov), unit));
            }
            // No debug info for this address. A bare name that exactly one
            // function answers to is the best available evidence; a contested
            // one stays unattributed, and the image findings say so.
            let unique = |bare: &str| match bare_names.get(bare).map(|ids| &ids[..]) {
                Some([only]) => Resolved::Fn(*only),
                _ => Resolved::Unknown,
            };
            unique(cov).or_spelling(|| unique(&demangle(cov)))
        };

        // Parameters and locals (#52): what the DWARF declares, keyed by each
        // subprogram's low PC. Parsed from the bytes this load already
        // admitted -- the dSYM's when one was found -- not a fresh read of
        // the file, which would walk past the budget the load applied.
        let var_decls = {
            let dbytes: &[u8] = bin.dwarf_bytes.as_deref().unwrap_or(&bin.bytes);
            object::File::parse(dbytes).ok().and_then(|f| vars::declared(&f).ok()).unwrap_or_default()
        };

        // Several symbols can answer to one function's demangled name: a C++
        // constructor is emitted as C1 and C2, a destructor as D0, D1 and D2,
        // and llvm-cov records only the variant holding the body (C2, D2).
        // Where the complete-object variant is a separate thunk (Mach-O), the
        // first by address was analysed instead -- a call to the body, drawn
        // as the function calling itself, with the body's own calls lost
        // (#166). The symbol the coverage record names is the one analysed;
        // the demangled match stays the fallback for everything else.
        let exact_symbol: HashMap<usize, usize> = {
            let mut m = HashMap::new();
            for (i, sym) in bin.symbols.iter().enumerate().filter(|(_, s)| s.size > 0) {
                let bare = if is_macho { sym.name.strip_prefix('_').unwrap_or(&sym.name) } else { &sym.name };
                if let Some(fid) = resolve_fn(&sym.name, Some(sym.address)).id() {
                    let cov = &functions[fid].mangled;
                    if cov.rsplit_once(':').map_or(cov.as_str(), |(_, m)| m) == bare {
                        m.entry(fid).or_insert(i);
                    }
                }
            }
            m
        };
        // Two passes. The first analyses each function's body, the symbol
        // chosen above. The second analyses the other variants of a
        // constructor or destructor (#266): attributed to the function, and
        // before this never looked at -- no blocks, no gaps, no status, while
        // `textBytesCovered` counted them. A variant is analysed like the
        // body but has no coverage counter of its own, so its status comes
        // from `variants::projected`, not from line counts. An address is
        // analysed once: an ELF alias of the body (C1 = C2, D1 = D2) is the
        // body.
        for pass in 0..2 {
            let jobs: Vec<(usize, Option<String>)> = if pass == 0 {
                (0..bin.symbols.len()).map(|si| (si, None)).collect()
            } else {
                let mut analysed: HashSet<u64> = functions.iter().filter_map(|f| f.object.as_ref().map(|o| o.address)).collect();
                let bare = |n: &str| -> String {
                    if is_macho {
                        n.strip_prefix('_').unwrap_or(n).to_string()
                    } else {
                        n.to_string()
                    }
                };
                let mut v = Vec::new();
                for (si, sym) in bin.symbols.iter().enumerate().filter(|(_, s)| s.size > 0) {
                    let Some(fid) = resolve_fn(&sym.name, Some(sym.address)).id() else { continue };
                    let Some(body) = &functions[fid].object else { continue };
                    let Some(code) = variants::of(&bare(&body.symbol), &bare(&sym.name)) else { continue };
                    if analysed.insert(sym.address) {
                        v.push((si, Some(code)));
                    }
                }
                v
            };
            for (si, variant) in jobs {
                let sym = &bin.symbols[si];
                let Some(fid) = resolve_fn(&sym.name, Some(sym.address)).id() else { continue };
                if variant.is_none() && (functions[fid].object.is_some() || exact_symbol.get(&fid).is_some_and(|&e| e != si)) {
                    continue;
                }
                let end = sym.address.saturating_add(sym.size);
                let raw = bin.instrs_in(sym.address, end);
                if raw.is_empty() {
                    continue;
                }
                // Per-instruction line info and projected status.
                let info: Vec<(Option<u32>, Option<usize>, bool)> = raw
                    .iter()
                    .map(|i| match bin.line_at(i.addr) {
                        Some(r) => (Some(r.line), resolve_file(&r.file), r.is_stmt),
                        None => (None, None, false),
                    })
                    .collect();
                let body_count = functions[fid].count;
                // No record, no projection (#273): see where these functions are made.
                let recorded = functions[fid].unrecorded_within.is_none();
                let status: Vec<Status> = info
                    .iter()
                    .map(|(line, file_id, _)| match (line, file_id) {
                        _ if variant.is_some() => variants::projected(body_count),
                        (Some(l), Some(file)) if *l > 0 && recorded => match line_count(&files, &functions[fid], *file, *l) {
                            Some(0) => Status::Uncovered,
                            Some(_) => Status::Covered,
                            None => Status::Unknown,
                        },
                        _ => Status::Unknown,
                    })
                    .collect();
                // Where the object code touches each declared parameter and
                // local, in execution order. Off the DWARF and the disassembly,
                // never the source text (#52).
                // A variant's own subprogram declares the same `this` again; the
                // function's variables are the body's.
                if let Some(ds) = var_decls.get(&sym.address).filter(|_| variant.is_none()) {
                    functions[fid].variables = vars::attribute(&bin.arch, ds, raw, &|a| bin.line_at(a).map(|r| r.line));
                }
                // A call whose callee this image's DWARF marks DW_AT_noreturn
                // ends its block with no fall-through successor (#210). The
                // attribute is the producer's, read at the callee's own
                // `DW_AT_low_pc`; nothing here infers divergence from a symbol's
                // name.
                let cfg = cfg::build(
                    &bin.arch,
                    sym.address,
                    end,
                    raw,
                    &|i| info[i],
                    &|i| status[i],
                    &|t| bin.noreturn_at(t).map(|n| (cfg::DivergeRule::DwarfNoreturn, demangle(n.strip_prefix('_').filter(|_| is_macho).unwrap_or(n)))),
                    &|a| table_targets.get(&a).cloned().unwrap_or_default(),
                    &pads_of,
                );
                let mut blocks = cfg.blocks;
                // Branch-outcome inference refines a status read from line
                // counts, which a variant does not have.
                let inferred = if variant.is_none() {
                    infer_outcomes(&mut blocks, &functions[fid], &|l| if recorded { line_count(&files, &functions[fid], functions[fid].file_id, l) } else { None })
                } else {
                    Vec::new()
                };

                // Calls.
                for b in &blocks {
                    for ins in &b.instrs {
                        if ins.flags.contains(&InstrFlag::Call) {
                            let target = cfg::target(&ins.operands);
                            let callee = target.and_then(|t| addr_sym.get(&t).cloned()).or_else(|| cfg::target_symbol(&ins.operands));
                            if let Some(c) = callee {
                                // resolved here, where the target address can
                                // still disambiguate a duplicated static (#36)
                                let cid = resolve_fn(&c, target).id();
                                // A thunk calling the body it is a variant of is
                                // not the function calling itself (#166, #266).
                                if variant.is_some() && cid == Some(fid) {
                                    continue;
                                }
                                pending_calls.push(PendingCall { caller: fid, callee_sym: c, callee_id: cid, addr: ins.addr, line: ins.line, from_variant: variant.is_some() });
                            }
                        }
                    }
                }

                let f = &functions[fid];
                // A variant implements none of the function's source decisions:
                // a conditional branch in one is compiler-written, an extra
                // object branch, and a decision in the body has no counterpart
                // in it to be missing.
                let (branches, decisions): (&[Branch], &[Decision]) = if variant.is_some() { (&[], &[]) } else { (&f.branches, &f.decisions) };
                let gaps_found = {
                    // gaps::detect needs &mut blocks and a closure reading blocks; do
                    // the call-target lookup on a snapshot to satisfy the borrow checker.
                    let snapshot: Vec<(u64, String)> =
                        blocks.iter().flat_map(|b| b.instrs.iter()).filter(|i| i.flags.contains(&InstrFlag::Call)).map(|i| (i.addr, i.operands.clone())).collect();
                    let lookup = |addr: u64| -> Option<gaps::CallTarget> {
                        let (_, ops) = snapshot.iter().find(|(a, _)| *a == addr)?;
                        let target = cfg::target(ops);
                        let raw = target.and_then(|t| addr_sym.get(&t).cloned()).or_else(|| cfg::target_symbol(ops))?;
                        // The call graph's own resolver, given the same target
                        // address it uses there, so the two cannot disagree about
                        // which function this call names (#206).
                        let in_source_set = match resolve_fn(&raw, target) {
                            Resolved::Fn(_) => true,
                            // The DWARF named the unit that emitted the callee
                            // and that unit has no covered function of this
                            // name: the call really does leave the covered
                            // source set, and an unrelated same-named function
                            // in another unit does not bring it back (#237).
                            Resolved::NotInUnit => false,
                            // Nothing to resolve through -- a symbolic operand,
                            // or a build with no DWARF -- so the name is all the
                            // evidence there is.
                            Resolved::Unknown => is_known(&raw),
                        };
                        let symbol = if is_macho { raw.strip_prefix('_').unwrap_or(&raw).to_string() } else { raw };
                        Some(gaps::CallTarget { symbol, in_source_set })
                    };
                    gaps::detect(gaps::GapInput {
                        symbol: &sym.name,
                        blocks: &mut blocks,
                        branches,
                        decisions,
                        call_target: &lookup,
                        demangle: &demangle_fn,
                        project_prefixes: &project_prefixes,
                        recorded,
                        uncounted: variant.is_some(),
                    })
                };

                let mut gaps_found = gaps_found;
                for (bid, why) in inferred {
                    let b = &blocks[bid];
                    let id = gaps_found.len();
                    let key = format!("{}@shared-line@{:x}", sym.name, b.address);
                    gaps_found.push(Gap {
                        id,
                        kind: GapKind::SharedLine,
                        addr_start: b.address,
                        addr_end: b.instrs.last().map(|i| i.addr).unwrap_or(b.address),
                        line: b.lines.first().copied(),
                        description: why,
                        key,
                        class: None,
                    });
                    for ins in &mut blocks[bid].instrs {
                        ins.gap_ids.push(id);
                    }
                }
                // The one thing source coverage cannot say about a variant whose
                // body ran is whether the variant did. Say so where a reviewer
                // resolves findings (#266). A variant of a body that never ran
                // is projected uncovered instead, which needs no gap to be seen.
                if let Some(code) = variant.as_deref().filter(|_| body_count > 0) {
                    let id = gaps_found.len();
                    let line = blocks.first().and_then(|b| b.instrs.first()).and_then(|i| i.line);
                    let last = blocks.iter().flat_map(|b| b.instrs.last()).map(|i| i.addr).max().unwrap_or(sym.address);
                    let n: usize = blocks.iter().map(|b| b.instrs.len()).sum();
                    let f = &functions[fid];
                    let body = f.object.as_ref().map(|o| o.symbol.as_str()).unwrap_or("");
                    let description = format!(
                        "`{}` is the {} ({code}) of `{}`, which the compiler emits beside the body the coverage record names (`{body}`). It has no coverage counter of its own -- the instrumentation counts the base variant only -- so whether it ran is not established: its {n} instructions are projected unknown, not covered, although the body ran {} time(s). A trace, or an analysis of what reaches it, has to account for it (6.4.4.2.b).",
                        sym.name,
                        variants::role(code),
                        f.name,
                        f.count
                    );
                    let key = format!("{}@uncounted-variant@{:x}", sym.name, sym.address);
                    gaps_found.push(Gap { id, kind: GapKind::UncountedVariant, addr_start: sym.address, addr_end: last, line, description, key, class: None });
                    if let Some(i) = blocks.first_mut().and_then(|b| b.instrs.first_mut()) {
                        i.gap_ids.push(id);
                    }
                }
                let outcome_edges_total: usize = blocks.iter().filter(|b| b.succs.len() == 2 && b.succs.iter().any(|e| e.kind == EdgeKind::Taken)).map(|b| b.succs.len()).sum();
                let unreachable_blocks = if eh_unread { Vec::new() } else { unreachable(&blocks) };
                let instr_total = blocks.iter().map(|b| b.instrs.len()).sum();
                let instr_projected_covered = blocks.iter().flat_map(|b| &b.instrs).filter(|i| i.projected == Status::Covered).count();
                let instr_projected_uncovered = blocks.iter().flat_map(|b| &b.instrs).filter(|i| i.projected == Status::Uncovered).count();
                let instr_unknown = instr_total - instr_projected_covered - instr_projected_uncovered;
                let edges: u32 = blocks.iter().map(|b| b.succs.len() as u32).sum();
                let nodes = blocks.len() as u32;
                let exits = blocks.iter().filter(|b| b.succs.is_empty()).count() as u32;
                // Standard E - N + 2 treats a single exit; with several return
                // blocks, connect them to one virtual exit node.
                let cyclomatic = (edges as i64 + exits as i64 + 2 - nodes as i64 - 1).max(1) as u32;

                // inlined subroutines whose ranges fall inside this symbol
                // (the body's: `Instr::inlined` indexes `Function::inlined`, which
                // is one list per function, so a variant's instructions carry none)
                let mut inlined: Vec<Inlined> = Vec::new();
                for rec in bin.inlines.iter().filter(|_| variant.is_none()).filter(|r| {
                    if r.ranges.is_empty() {
                        r.subprogram_low == Some(sym.address)
                    } else {
                        r.ranges.iter().all(|(lo, _)| *lo >= sym.address && *lo < end)
                    }
                }) {
                    let cov_name = if is_macho { rec.origin.strip_prefix('_').unwrap_or(rec.origin.as_ref()) } else { rec.origin.as_ref() };
                    let dem = demangle(cov_name);
                    // a duplicated static can only have been inlined from its
                    // own translation unit, which is the enclosing symbol's
                    let function_id = resolve_fn(&rec.origin, Some(sym.address)).id();
                    let instr_count = blocks.iter().flat_map(|b| b.instrs.iter()).filter(|i| rec.ranges.iter().any(|(lo, hi)| i.addr >= *lo && i.addr < *hi)).count();
                    inlined.push(Inlined { name: dem, function_id, call_line: rec.call_line, ranges: rec.ranges.clone(), instr_count, depth: rec.depth });
                }
                inlined.sort_by_key(|i| (i.ranges.first().map(|r| r.0).unwrap_or(0), i.depth));
                // innermost inline per instruction
                for b in &mut blocks {
                    for ins in &mut b.instrs {
                        let mut best: Option<(usize, u32)> = None;
                        for (k, inl) in inlined.iter().enumerate() {
                            if inl.ranges.iter().any(|(lo, hi)| ins.addr >= *lo && ins.addr < *hi) && best.map(|(_, d)| inl.depth >= d).unwrap_or(true) {
                                best = Some((k, inl.depth));
                            }
                        }
                        ins.inlined = best.map(|(k, _)| k);
                    }
                }
                classify_origins(&bin.arch, &mut blocks, &inlined, &gaps_found);
                let mut compiler_generated: BTreeMap<String, usize> = BTreeMap::new();
                for b in &blocks {
                    for i in &b.instrs {
                        if let Some(o) = &i.origin {
                            if !o.starts_with("inlined:") {
                                *compiler_generated.entry(o.clone()).or_default() += 1;
                            }
                        }
                    }
                }
                let f = &mut functions[fid];
                // The function's complexity and inlining are its body's: the
                // source function is what they measure.
                if variant.is_none() {
                    f.inlined = inlined;
                    f.complexity.cyclomatic_object = Some(cyclomatic);
                    f.complexity.object_blocks = nodes;
                    f.complexity.object_edges = edges;
                    f.complexity.object_instructions = instr_total as u32;
                    f.complexity.object_conditional_branches = cfg.cond_branches;
                }
                let o = ObjectFn {
                    symbol: sym.name.clone(),
                    variant: variant.clone(),
                    address: sym.address,
                    size: sym.size,
                    blocks,
                    gaps: gaps_found,
                    instr_total,
                    instr_projected_covered,
                    instr_projected_uncovered,
                    instr_unknown,
                    instr_measured_covered: None,
                    disagreements: Vec::new(),
                    outcome_edges_total,
                    outcome_edges_measured: None,
                    unreachable_blocks,
                    compiler_generated,
                };
                if variant.is_some() {
                    f.variants.push(o);
                } else {
                    f.object = Some(o);
                }
                let _ = &sym_addr;
            }
        }
        for f in &mut functions {
            f.variants.sort_by_key(|o| o.address);
        }

        // ---- whole-image analysis: symbol table, stack, data coupling, trace ----
        if symbols.is_empty() {
            let arch = bin.arch.clone();
            let sym_of_addr = |a: u64| -> Option<usize> {
                let idx = bin.symbols.partition_point(|s| s.address <= a);
                if idx == 0 {
                    return None;
                }
                let s = &bin.symbols[idx - 1];
                (a >= s.address && a < s.address.saturating_add(s.size.max(1))).then_some(idx - 1)
            };
            let stack_sizes: HashMap<u64, u64> = bin.stack_sizes.iter().cloned().collect();
            // A call may go through a stub (weak/linkonce symbols such as C++
            // template instantiations, or anything imported), in which case the
            // branch target is the stub, not the function. objdump still names
            // the real callee in `<...>`, so fall back to that name.
            let mut sym_by_name: HashMap<String, usize> = HashMap::new();
            for (i, s) in bin.symbols.iter().enumerate() {
                sym_by_name.entry(s.name.clone()).or_insert(i);
                sym_by_name.entry(s.name.strip_prefix('_').unwrap_or(&s.name).to_string()).or_insert(i);
            }
            let call_target_sym = |ins: &objectcode::RawInstr| -> Option<usize> {
                if let Some(t) = cfg::target(&ins.operands).and_then(sym_of_addr) {
                    return Some(t);
                }
                let n = cfg::target_symbol(&ins.operands)?;
                sym_by_name.get(&n).or_else(|| sym_by_name.get(n.strip_prefix('_').unwrap_or(&n))).copied()
            };
            // A C++ constructor or destructor is emitted under several mangled
            // names -- the Itanium complete/base/deleting variants (C1/C2,
            // D0/D1/D2) -- which the linker may fold to one address. On ELF
            // clang writes a .su record for only the base variant, so the other
            // alias at the same address finds no record of its own and would be
            // reported as a stated frame that is missing. They are the same
            // code, so a .su frame resolved for any symbol at an address serves
            // every symbol at that address (#266 review). This index is built
            // over the whole symbol table first, so the order symbols are
            // visited in below does not matter. Records are placed against
            // every analysed symbol at once, so another unit's record for a
            // same-named function cannot fill an address either (#10).
            let names: Vec<(String, String)> = bin
                .symbols
                .iter()
                .map(|sym| {
                    let cov_name = if is_macho { sym.name.strip_prefix('_').unwrap_or(&sym.name) } else { sym.name.as_str() };
                    (demangle(cov_name), cov_name.to_string())
                })
                .collect();
            let slots: Vec<SuSlot> = bin
                .symbols
                .iter()
                .zip(&names)
                .filter(|(sym, _)| !bin.instrs_in(sym.address, sym.address.saturating_add(sym.size)).is_empty())
                .filter_map(|(sym, (dem, cov_name))| {
                    // a copy tied to no coverage function still has a unit
                    // that emitted it, which a record's path may also match
                    let tu = match resolve_fn(&sym.name, Some(sym.address)).id() {
                        Some(fid) => files[functions[fid].file_id].path.as_str(),
                        None => bin.tu_of(sym.address)?,
                    };
                    Some(SuSlot { addr: sym.address, dem, cov_name, tu })
                })
                .collect();
            let su_by_addr = place_su(&su_map, &slots);
            let mut nodes = Vec::new();
            let mut attribution_findings: Vec<String> = Vec::new();
            for (i, sym) in bin.symbols.iter().enumerate() {
                let cov_name = if is_macho { sym.name.strip_prefix('_').unwrap_or(&sym.name) } else { sym.name.as_str() };
                let dem = demangle(cov_name);
                let function_id = resolve_fn(&sym.name, Some(sym.address)).id();
                if function_id.is_none() {
                    // A function of this name exists but this symbol could
                    // not be tied to it. Say so, or the code drops out of
                    // every total with nothing to explain the shortfall.
                    if let Some(ids) = bare_names.get(cov_name).or_else(|| bare_names.get(&dem)).filter(|ids| !ids.is_empty()) {
                        let claimants = ids.iter().map(|&id| functions[id].name.as_str()).collect::<Vec<_>>().join(", ");
                        let why = if ids.len() > 1 {
                            format!("{} coverage functions share this name ({claimants}) and the DWARF does not say which translation unit emitted this copy", ids.len())
                        } else {
                            format!(
                                "the only coverage function of this name ({claimants}) belongs to another translation unit, so the coverage export carries no record for this copy -- llvm-cov names two same-named statics identically and keeps one"
                            )
                        };
                        attribution_findings.push(format!(
                            "symbol {} @{:#x} ({} B): {why}, so its object code is attributed to none of them and is missing from every object-code total",
                            sym.name, sym.address, sym.size,
                        ));
                    }
                }
                let raw = bin.instrs_in(sym.address, sym.address.saturating_add(sym.size));
                let mut callees = Vec::new();
                let mut indirect = 0u32;
                let sym_end = sym.address.saturating_add(sym.size);
                for ins in raw {
                    let ctl = cfg::classify(&arch, &ins.mnemonic, &ins.operands);
                    // A GOT tail call is a call to the bound symbol: it must
                    // enter the call graph (and, through it, the stack graph)
                    // exactly like a direct or indirect-through-GOT call, or
                    // it would lose its callee entirely (#271 review). An
                    // external target resolves to no in-image symbol and
                    // becomes a helper-call gap instead (via InstrFlag::Call).
                    // A direct branch out of the function is a tail call too:
                    // counting the callee's frame on top of this one
                    // over-states the depth it really reaches, which is the
                    // safe direction (#264).
                    let is_call = matches!(ctl, cfg::Ctl::Call | cfg::Ctl::TailCall) || cfg::is_tail_call(ctl, &ins.operands, sym.address, sym_end);
                    match ctl {
                        _ if is_call => {
                            if let Some(t) = call_target_sym(ins) {
                                if t != i {
                                    callees.push(t);
                                }
                            }
                        }
                        cfg::Ctl::IndirectCall => indirect += 1,
                        _ => {}
                    }
                }
                callees.sort_unstable();
                callees.dedup();
                let mut frame = if let Some(b) = stack_sizes.get(&sym.address) {
                    Frame { bytes: *b, source: FrameSource::StackSizesSection, dynamic: false }
                } else if let Some((b, d)) = su_by_addr.get(&sym.address).copied().or_else(|| function_id.is_none().then(|| su_unplaced(&su_map, &dem, cov_name)).flatten()) {
                    // The records placed at this address (`place_su`), which a
                    // ctor/dtor alias shares with its recorded variant (#266);
                    // or, for a symbol tied to no function, any of its name.
                    Frame { bytes: b, source: FrameSource::StackUsageFile, dynamic: d }
                } else if raw.is_empty() {
                    Frame::default()
                } else {
                    stack::frame_from_prologue(&arch, raw)
                };
                // A VLA or alloca allocates in the body, where no frame source
                // but a `.su` looks: `.stack_sizes` omits the function and the
                // prologue precedes the allocation. Without this, a build with
                // no `.su` reported it bounded by its static frame (#168).
                if !frame.dynamic && stack::allocates_dynamically(&arch, raw) {
                    frame.dynamic = true;
                }
                if declared.as_ref().is_some_and(|d| d.stack_usage)
                    && function_id.is_some()
                    && !raw.is_empty()
                    && !matches!(frame.source, FrameSource::StackUsageFile | FrameSource::StackSizesSection)
                {
                    stated_frames_missing.push(dem.clone());
                }
                let section = bin.sections.iter().find(|s| sym.address >= s.address && sym.address < s.address.saturating_add(s.size)).map(|s| s.name.clone()).unwrap_or_default();
                nodes.push(stack::StackNode {
                    frame: frame.bytes,
                    dynamic: frame.dynamic,
                    callees: callees.clone(),
                    indirect_calls: indirect,
                    // Fail closed when the authoritative compiler metadata was
                    // corrupt: a prologue estimate that replaced an entry lost
                    // to a malformed .stack_sizes is not an established bound
                    // (#247). Its callers inherit the uncertainty.
                    unknown: stack::frame_unknown(frame.source, bin.stack_sizes_malformed),
                });
                symbols.push(Symbol {
                    id: i,
                    name: dem,
                    mangled: sym.name.clone(),
                    address: sym.address,
                    size: sym.size,
                    section,
                    function_id,
                    frame,
                    callees,
                    indirect_calls: indirect,
                    stack: StackResult::default(),
                    measured: None,
                    data_refs: Vec::new(),
                    address_taken_by: Vec::new(),
                    callers: Vec::new(),
                });
            }
            if !stated_frames_missing.is_empty() {
                stated_frames_missing.sort();
                stated_frames_missing.dedup();
                bail!(
                    "build-info.txt declares -fstack-usage, but {} analysed function(s) in {} have no stack-usage record: {}.\n\nA .su file the build promised is missing, or two objects' .su files were flattened onto one name and one overwrote the other. Their frames would otherwise be estimated from the prologue and read as compiler-stated. Collect every .su the analysis build wrote, keeping names distinct.",
                    stated_frames_missing.len(),
                    obj_path.display(),
                    stated_frames_missing.join(", ")
                );
            }
            for (s, r) in symbols.iter_mut().zip(stack::worst_case(&nodes)) {
                s.stack = r;
            }
            for s in &symbols {
                if let Some(fid) = s.function_id {
                    if functions[fid].symbol_id.is_none() {
                        functions[fid].symbol_id = Some(s.id);
                    }
                }
            }

            // Data coupling.
            let data_of_addr = |a: u64| -> Option<u64> {
                let idx = bin.data_symbols.partition_point(|s| s.address <= a);
                if idx == 0 {
                    return None;
                }
                let s = &bin.data_symbols[idx - 1];
                (a < s.address.saturating_add(s.size.max(1))).then_some(s.address)
            };
            let mut global_by_addr: HashMap<u64, usize> = HashMap::new();
            let mut sym_refs: Vec<Vec<DataRef>> = vec![Vec::new(); symbols.len()];
            for s in &symbols {
                let fid = s.function_id.filter(|&f| functions[f].symbol_id == Some(s.id));
                let raw = bin.instrs_in(s.address, s.address.saturating_add(s.size));
                let accesses = datarefs::scan(&arch, raw, &data_of_addr);
                if accesses.is_empty() {
                    continue;
                }
                let mut per: BTreeMap<u64, DataRef> = BTreeMap::new();
                for a in accesses {
                    let gid = match global_by_addr.get(&a.target) {
                        Some(g) => *g,
                        None => {
                            let Some(ds) = bin.data_symbols.iter().find(|d| d.address == a.target) else { continue };
                            let id = globals.len();
                            let cov_name = if is_macho { ds.name.strip_prefix('_').unwrap_or(&ds.name) } else { ds.name.as_str() };
                            let section =
                                bin.sections.iter().find(|s| ds.address >= s.address && ds.address.saturating_sub(s.address) < s.size).map(|s| s.name.clone()).unwrap_or_default();
                            globals.push(Global {
                                id,
                                name: demangle(cov_name),
                                mangled: ds.name.clone(),
                                address: ds.address,
                                size: ds.size,
                                section,
                                readers: Vec::new(),
                                writers: Vec::new(),
                                address_takers: Vec::new(),
                                reader_symbols: Vec::new(),
                                writer_symbols: Vec::new(),
                                address_taker_symbols: Vec::new(),
                            });
                            global_by_addr.insert(a.target, id);
                            id
                        }
                    };
                    let e = per.entry(a.target).or_insert(DataRef { global_id: gid, reads: 0, writes: 0, addr_taken: 0, lines: Vec::new() });
                    match a.access {
                        datarefs::Access::Read => e.reads += 1,
                        datarefs::Access::Write => e.writes += 1,
                        datarefs::Access::Address => e.addr_taken += 1,
                    }
                    if let Some(r) = bin.line_at(a.addr) {
                        if r.line > 0 && !e.lines.contains(&r.line) {
                            e.lines.push(r.line);
                        }
                    }
                }
                for (_, d) in per {
                    let g = &mut globals[d.global_id];
                    if d.reads > 0 {
                        g.reader_symbols.push(s.id);
                    }
                    if d.writes > 0 {
                        g.writer_symbols.push(s.id);
                    }
                    if d.addr_taken > 0 {
                        g.address_taker_symbols.push(s.id);
                    }
                    if let Some(fid) = fid {
                        if d.reads > 0 && !g.readers.contains(&fid) {
                            g.readers.push(fid);
                        }
                        if d.writes > 0 && !g.writers.contains(&fid) {
                            g.writers.push(fid);
                        }
                        if d.addr_taken > 0 && !g.address_takers.contains(&fid) {
                            g.address_takers.push(fid);
                        }
                        functions[fid].data_refs.push(d.clone());
                    }
                    sym_refs[s.id].push(d);
                }
            }
            for (s, r) in symbols.iter_mut().zip(sym_refs) {
                s.data_refs = r;
            }

            // Address-taken text symbols (function pointers, vector tables) and
            // direct callers, for dead-code analysis.
            let text_of_addr = |a: u64| -> Option<u64> {
                let idx = bin.symbols.partition_point(|s| s.address <= a);
                if idx == 0 {
                    return None;
                }
                let s = &bin.symbols[idx - 1];
                (a < s.address.saturating_add(s.size.max(1))).then_some(s.address)
            };
            let sym_id_of_addr: HashMap<u64, usize> = symbols.iter().map(|s| (s.address, s.id)).collect();
            let image_base: u64 = bin.segments.iter().filter(|g| g.size > 0 && g.address > 0).map(|g| g.address).min().unwrap_or(0);
            let mut taken: Vec<Vec<usize>> = vec![Vec::new(); symbols.len()];
            let mut callers: Vec<Vec<usize>> = vec![Vec::new(); symbols.len()];
            for s in &symbols {
                let raw = bin.instrs_in(s.address, s.address.saturating_add(s.size));
                for a in datarefs::scan(&arch, raw, &text_of_addr) {
                    if let Some(&t) = sym_id_of_addr.get(&a.target) {
                        if t != s.id && !taken[t].contains(&s.id) {
                            taken[t].push(s.id);
                        }
                    }
                }
                for &c in &s.callees {
                    if !callers[c].contains(&s.id) {
                        callers[c].push(s.id);
                    }
                }
            }
            // pointers stored in data (vector tables, dispatch tables): scan
            // data sections for text addresses
            {
                use object::{Object, ObjectSection};
                // the bytes the load already admitted; re-reading the file
                // here bypassed the cap entirely (review of #5)
                {
                    if let Ok(file) = object::File::parse(&*bin.bytes) {
                        let width = if bin.is_64 { 8 } else { 4 };
                        for sec in file.sections() {
                            if !objectcode::is_data_section(&sec) {
                                continue;
                            }
                            let Ok(data) = sec.data() else { continue };
                            let base = sec.address();
                            let mut i = 0;
                            while i + width <= data.len() {
                                let v =
                                    if width == 8 { u64::from_le_bytes(data[i..i + 8].try_into().unwrap()) } else { u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as u64 };
                                // an ELF PIE's RELATIVE slot reads 0 from the file (#325 G1)
                                let v = bin.pointer_at(base.saturating_add(i as u64), v);
                                for cand in pointer_candidates(v, is_macho, image_base) {
                                    if let Some(&t) = sym_id_of_addr.get(&cand) {
                                        if !taken[t].contains(&usize::MAX) {
                                            taken[t].push(usize::MAX);
                                        }
                                        break;
                                    }
                                }
                                i += width;
                            }
                        }
                    }
                }
            }
            // which text symbols each symbol takes the address of (static
            // candidates for its indirect calls)
            let mut takes: Vec<Vec<usize>> = vec![Vec::new(); symbols.len()];
            for (t, takers) in taken.iter().enumerate() {
                for &k in takers {
                    if k != usize::MAX && !takes[k].contains(&t) {
                        takes[k].push(t);
                    }
                }
            }
            // data tables holding function addresses: record which table
            let mut table_of: HashMap<usize, Vec<String>> = HashMap::new();
            {
                use object::{Object, ObjectSection};
                // the bytes the load already admitted; re-reading the file
                // here bypassed the cap entirely (review of #5)
                {
                    if let Ok(file) = object::File::parse(&*bin.bytes) {
                        let width = if bin.is_64 { 8 } else { 4 };
                        for sec in file.sections() {
                            if !objectcode::is_data_section(&sec) {
                                continue;
                            }
                            let Ok(data) = sec.data() else { continue };
                            let base = sec.address();
                            let mut i = 0;
                            while i + width <= data.len() {
                                let v =
                                    if width == 8 { u64::from_le_bytes(data[i..i + 8].try_into().unwrap()) } else { u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as u64 };
                                let v = bin.pointer_at(base.saturating_add(i as u64), v);
                                if let Some(&t) = pointer_candidates(v, is_macho, image_base).iter().find_map(|c| sym_id_of_addr.get(c)) {
                                    let here = base.saturating_add(i as u64);
                                    let owner = bin
                                        .data_symbols
                                        .iter()
                                        .rev()
                                        .find(|d| here >= d.address && here.saturating_sub(d.address) < d.size.max(1))
                                        .map(|d| demangle(d.name.trim_start_matches('_')))
                                        .unwrap_or_else(|| format!("{} @ {:#x}", sec.name().unwrap_or("data"), here));
                                    let e = table_of.entry(t).or_default();
                                    if !e.contains(&owner) {
                                        e.push(owner);
                                    }
                                }
                                i += width;
                            }
                        }
                    }
                }
            }
            for (s, (t, c)) in symbols.iter_mut().zip(taken.into_iter().zip(callers)) {
                s.address_taken_by = t;
                s.callers = c;
            }
            // per-function indirect call sites (static candidates) and pointer callers
            // table name -> function symbols it holds
            let tables = tables_by_name(table_of.iter().map(|(t, names)| (*t, names.as_slice())));
            for s in &symbols {
                let Some(fid) = s.function_id else { continue };
                if functions[fid].symbol_id != Some(s.id) {
                    continue;
                }
                let raw = bin.instrs_in(s.address, s.address.saturating_add(s.size));
                let cand_ids = candidate_ids(&takes[s.id], s.data_refs.iter().map(|d| globals[d.global_id].name.as_str()), &tables, |t| symbols[t].address);
                let candidates: Vec<PointerTarget> =
                    cand_ids.iter().map(|&t| PointerTarget { symbol_id: t, name: symbols[t].name.clone(), function_id: symbols[t].function_id, count: 0 }).collect();
                let mut sites = Vec::new();
                for ins in raw {
                    if cfg::classify(&arch, &ins.mnemonic, &ins.operands) == cfg::Ctl::IndirectCall {
                        sites.push(IndirectCall {
                            addr: ins.addr,
                            line: bin.line_at(ins.addr).map(|r| r.line).filter(|l| *l > 0),
                            candidates: candidates.clone(),
                            measured: Vec::new(),
                            measured_count: None,
                        });
                    }
                }
                functions[fid].indirect_calls = sites;
                let mut pc: Vec<PointerCaller> = s
                    .address_taken_by
                    .iter()
                    .filter(|&&k| k != usize::MAX)
                    .map(|&k| PointerCaller { kind: "code".into(), symbol_id: Some(k), name: symbols[k].name.clone(), function_id: symbols[k].function_id, count: 0 })
                    .collect();
                for tbl in table_of.get(&s.id).cloned().unwrap_or_default() {
                    pc.push(PointerCaller { kind: "data".into(), symbol_id: None, name: tbl, function_id: None, count: 0 });
                }
                functions[fid].pointer_callers = pc;
            }

            // Image / link map.
            // Each address once: an alias (ELF emits a constructor's C1 as
            // one of C2) is the same bytes under a second name. Covered bytes
            // are those a coverage record's counters stand behind: attributed
            // to a function, and not one of its constructor/destructor
            // variants, which have a functionId but no counter (#266).
            let variant_addrs: HashSet<u64> = functions.iter().flat_map(|f| f.variants.iter().map(|o| o.address)).collect();
            let mut by_addr: BTreeMap<u64, (u64, bool)> = BTreeMap::new();
            for s in &symbols {
                let e = by_addr.entry(s.address).or_default();
                e.0 = e.0.max(s.size);
                e.1 |= s.function_id.is_some() && !variant_addrs.contains(&s.address);
            }
            let text_bytes: u64 = by_addr.values().map(|(n, _)| n).sum();
            let text_bytes_covered: u64 = by_addr.values().filter(|(_, c)| *c).map(|(n, _)| n).sum();
            let mut uncovered: Vec<usize> = symbols.iter().filter(|s| s.function_id.is_none() && s.size > 0).map(|s| s.id).collect();
            uncovered.sort_by_key(|&i| std::cmp::Reverse(symbols[i].size));
            let mut findings = attribution_findings;
            for (i, a) in bin.segments.iter().enumerate() {
                for b in &bin.segments[i + 1..] {
                    if a.address < b.address.saturating_add(b.size) && b.address < a.address.saturating_add(a.size) {
                        findings.push(format!("segments {} @{:#x} ({} B) and {} @{:#x} ({} B) overlap", a.name, a.address, a.size, b.name, b.address, b.size));
                    }
                }
            }
            for (i, a) in bin.sections.iter().enumerate() {
                for b in &bin.sections[i + 1..] {
                    if a.address != 0
                        && b.address != 0
                        && a.address < b.address.saturating_add(b.size)
                        && b.address < a.address.saturating_add(a.size)
                        && a.kind != "Debug"
                        && b.kind != "Debug"
                    {
                        findings.push(format!("sections {} @{:#x} and {} @{:#x} overlap", a.name, a.address, b.name, b.address));
                    }
                }
            }
            if let Some(e) = symbols.iter().find(|s| s.address == bin.entry) {
                findings.push(format!("entry point {:#x} = {}", bin.entry, e.name));
            } else if bin.entry != 0 {
                findings.push(format!("entry point {:#x} is not at a symbol boundary", bin.entry));
            }
            {
                let vs: Vec<&ObjectFn> = functions.iter().flat_map(|f| f.variants.iter()).collect();
                if !vs.is_empty() {
                    let bytes: u64 = vs.iter().map(|o| o.size).sum();
                    let list: Vec<String> = vs.iter().take(8).map(|o| format!("{} ({}, {} B)", o.symbol, o.variant.as_deref().unwrap_or("?"), o.size)).collect();
                    findings.push(format!(
                        "{} C++ constructor/destructor variant(s) ({bytes} B) are attributed to their source function but have no coverage counter of their own (the instrumentation counts the base variant only): each is analysed under its function with its instructions projected unknown -- uncovered where the body never ran -- and none is in the covered bytes; {}{}",
                        vs.len(),
                        list.join(", "),
                        if vs.len() > 8 { format!(", and {} more", vs.len() - 8) } else { String::new() }
                    ));
                }
            }
            if !uncovered.is_empty() {
                let bytes: u64 = uncovered.iter().map(|&i| symbols[i].size).sum();
                let top: Vec<String> = uncovered.iter().take(8).map(|&i| format!("{} ({} B)", symbols[i].name, symbols[i].size)).collect();
                findings.push(format!(
                    "{} of {} text symbols ({} of {} bytes, {:.1}%) have no source-coverage evidence behind them (runtime, core/libc, or code outside the coverage build); largest: {}",
                    uncovered.len(), symbols.len(), bytes, text_bytes, if text_bytes == 0 { 0.0 } else { 100.0 * bytes as f64 / text_bytes as f64 }, top.join(", ")
                ));
            }
            let sections = bin
                .sections
                .iter()
                .filter(|sec| sec.address != 0 && sec.kind != "Debug" && sec.kind != "Metadata" && sec.kind != "Note" && sec.kind != "Linker")
                .map(|sec| {
                    let end = sec.address.saturating_add(sec.size);
                    let mut members: Vec<(String, u64)> = symbols
                        .iter()
                        .filter(|s| s.address >= sec.address && s.address < end)
                        .map(|s| (s.name.clone(), s.size))
                        .chain(bin.data_symbols.iter().filter(|s| s.address >= sec.address && s.address < end).map(|s| (demangle(s.name.trim_start_matches('_')), s.size)))
                        .collect();
                    let count = members.len();
                    members.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                    members.truncate(6);
                    Section { name: sec.name.clone(), address: sec.address, size: sec.size, kind: sec.kind.clone(), flags: sec.flags.clone(), symbols: count, largest: members }
                })
                .collect();
            let mut dead_candidates = Vec::new();
            // Reachability is per *address*, not per symbol name. A C++
            // ctor/dtor is emitted under several mangled names (Itanium C1/C2,
            // D0/D1/D2) the linker may fold to one address, and a call lands on
            // whichever name it resolved -- so the other alias at that address
            // has no caller of its own yet is the same, reachable code. An
            // address any symbol reaches (a caller, its address taken, or the
            // entry) is reachable for every symbol there (#266 review).
            let reachable_addrs: BTreeSet<u64> =
                symbols.iter().filter(|s| s.address == bin.entry || !s.callers.is_empty() || !s.address_taken_by.is_empty()).map(|s| s.address).collect();
            // An unlinked object has no entry point, and its callers may be
            // in units this analysis never saw whole, so reachability cannot
            // be established -- the caveat above says so. Listing candidates
            // anyway named functions other units call as dead (#169).
            for s in symbols.iter().filter(|_| !bin.relocatable) {
                if s.size == 0 || reachable_addrs.contains(&s.address) {
                    continue;
                }
                let lname = s.name.to_ascii_lowercase();
                if lname.contains("start")
                    || lname.contains("reset")
                    || lname.contains("vector")
                    || lname.contains("handler")
                    || lname.contains("exception")
                    || lname.contains("main")
                    || lname.starts_with("_") && lname.contains("init")
                {
                    continue;
                }
                // A function with no record has no count to report (#273).
                let unrecorded = s.function_id.is_some_and(|f| functions[f].unrecorded_within.is_some());
                let count = s.function_id.filter(|_| !unrecorded).map(|f| functions[f].count);
                let mut reasons = vec!["no direct call from any symbol in the image".to_string(), "address never taken (code or data tables)".to_string()];
                if unrecorded {
                    reasons.push("no coverage record of its own: whether it ran is not established".into());
                }
                if let Some(c) = count {
                    reasons.push(if c == 0 {
                        "never executed in the instrumented run".into()
                    } else {
                        format!("executed {c}× in the instrumented run (called from code outside this image?)")
                    });
                }
                dead_candidates.push(DeadCandidate {
                    symbol_id: s.id,
                    name: s.name.clone(),
                    size: s.size,
                    function_id: s.function_id,
                    count,
                    measured: None,
                    reasons,
                    key: format!("dead:{}", s.name),
                });
            }
            dead_candidates.sort_by_key(|d| std::cmp::Reverse(d.size));
            image = Some(Image {
                dead_candidates,
                path: obj_path.display().to_string(),
                format: bin.format.clone(),
                arch: arch.clone(),
                entry: (bin.entry != 0).then_some(bin.entry),
                segments: bin
                    .segments
                    .iter()
                    .map(|s| Segment {
                        name: if s.name.is_empty() { "LOAD".into() } else { s.name.clone() },
                        address: s.address,
                        size: s.size,
                        file_size: s.file_size,
                        flags: s.flags.clone(),
                    })
                    .collect(),
                sections,
                findings,
                text_symbols: symbols.len(),
                text_symbols_uncovered: uncovered,
                text_bytes,
                text_bytes_covered,
            });

            // Measured coverage from an execution trace.
            if let Some(tp) = &opts.trace {
                let text = read_input(tp, limits::Kind::Trace, &lim, by_name(tp, "--trace"))?;
                inputs.push(artifact("trace", tp, text.as_bytes()));
                let mut tr = trace::parse(&text);
                // A file whose format was not recognised yields no counts, and
                // the measured path over no counts is not "nothing measured":
                // it is every instruction reported as executed zero times, and
                // a ProjectedNotExecuted disagreement for every instruction the
                // projection covers -- "0 of 312 executed, 234 projected
                // covered and never run", from a file butterfly could not read.
                // Refuse instead (review of #208).
                if tr.format == "unrecognised" {
                    anyhow::bail!(
                        "{}: not an execution trace butterfly recognises. Expected a QEMU `-d in_asm,exec` log (`IN:` blocks and `Trace N: ... [cs_base/pc/...]` records), a QEMU execlog plugin log (`N, 0xPC, 0xINSN, \"mnemonic\"`), or one address per line. No measured coverage was produced: reading an unrecognised file as an empty trace would report every instruction as never executed.",
                        tp.display()
                    );
                }
                // The same judgement one step further in: a log most of whose
                // exec records cannot be read is not a trace with a gap in
                // it. Every readable record in it is still used and every
                // loss is still in the caveats -- this refuses only the case
                // where what is left has an unknown relationship to the run,
                // and would nonetheless be published as *measured* coverage
                // (review of #208).
                if let Some(why) = tr.refusal() {
                    anyhow::bail!("{}: {why}", tp.display());
                }
                let extent = |pc: u64| -> Vec<u64> {
                    let idx = bin.instrs.partition_point(|i| i.addr < pc);
                    let mut v = Vec::new();
                    for ins in bin.instrs[idx..].iter().take(512) {
                        v.push(ins.addr);
                        if cfg::classify(&arch, &ins.mnemonic, &ins.operands) != cfg::Ctl::None {
                            break;
                        }
                    }
                    v
                };
                // Can control go from the instruction at `from_last` to a block
                // starting at `to`? Used before counts are spread over
                // instructions, to drop a block logged twice in a row that
                // cannot have run twice in a row, and to count the transitions
                // the disassembly forbids. Only a direct transfer with a
                // readable target answers `false`; an indirect one, a block
                // that ended for some other reason, or an address we cannot
                // disassemble all say `true`, which keeps the count.
                //
                // A conditional branch and a call each have two real
                // successors -- the target and the instruction after, which is
                // the fall-through and the return address respectively -- so
                // both count. (For the self case the fall-through is always
                // past the block's start, so this is the same answer the
                // narrower oracle gave.)
                let may_follow = |from_last: u64, to: u64| -> bool {
                    let idx = bin.instrs.partition_point(|i| i.addr < from_last);
                    let Some(ins) = bin.instrs.get(idx).filter(|i| i.addr == from_last) else { return true };
                    let next = bin.instrs.get(idx + 1).map(|i| i.addr);
                    match cfg::classify(&arch, &ins.mnemonic, &ins.operands) {
                        cfg::Ctl::Jump => cfg::target(&ins.operands).is_none_or(|t| t == to),
                        cfg::Ctl::CondBranch | cfg::Ctl::Call => cfg::target(&ins.operands).is_none_or(|t| t == to || next == Some(to)),
                        _ => true,
                    }
                };
                tr.discard_impossible_repeats(&may_follow);
                tr.expand(&extent);
                // last instruction address of each translation block
                let tb_last: HashMap<u64, u64> = tr.tb_instrs.iter().filter_map(|(s, v)| v.last().map(|l| (*s, *l))).collect();
                let tr_transitions = std::mem::take(&mut tr.transitions);
                let mut transitions_matched: u64 = 0;
                let counts = tr.counts;
                let (mut matched, mut outside) = (0usize, 0usize);
                for a in counts.keys() {
                    if sym_of_addr(*a).is_some() {
                        matched += 1
                    } else {
                        outside += 1
                    }
                }
                // With a trace, what it does not mention it did not see run:
                // 0, not "not measured". `None` is kept for evidence with no
                // trace at all, so the two answers stay apart (#325).
                let seen = |a: &u64| Some(counts.get(a).copied().unwrap_or(0));
                for s in &mut symbols {
                    s.measured = seen(&s.address);
                }
                if let Some(im) = image.as_mut() {
                    for d in &mut im.dead_candidates {
                        let addr = symbols[d.symbol_id].address;
                        let size = symbols[d.symbol_id].size;
                        let n: u64 = counts.iter().filter(|(a, _)| **a >= addr && **a < addr.saturating_add(size)).map(|(_, n)| *n).fold(0u64, u64::saturating_add);
                        d.measured = Some(n);
                        d.reasons.push(if n == 0 {
                            "never executed in the trace".into()
                        } else {
                            format!(
                                "EXECUTED in the trace ({n} instruction executions): reached by a path the static analysis cannot see (interrupt, indirect jump, external caller)"
                            )
                        });
                    }
                }
                // indirect-call resolution: a transition whose source TB ends
                // with an indirect call and whose target is a symbol entry
                {
                    let sym_by_entry: HashMap<u64, usize> = symbols.iter().map(|s| (s.address, s.id)).collect();
                    let mut site_of: HashMap<u64, (usize, usize)> = HashMap::new(); // call addr -> (fn id, site idx)
                    for f in &functions {
                        for (i, ic) in f.indirect_calls.iter().enumerate() {
                            site_of.insert(ic.addr, (f.id, i));
                        }
                    }
                    let resolved = resolved_calls(tr_transitions.iter(), &tb_last, &site_of, &sym_by_entry, |t| symbols[t].address);
                    for (fid, si, t, n) in resolved {
                        let (name, target_fn) = (symbols[t].name.clone(), symbols[t].function_id);
                        let caller_name = functions[fid].name.clone();
                        let caller_sym = functions[fid].symbol_id;
                        let ic = &mut functions[fid].indirect_calls[si];
                        match ic.measured.iter_mut().find(|m| m.symbol_id == t) {
                            Some(m) => m.count = m.count.saturating_add(n),
                            None => ic.measured.push(PointerTarget { symbol_id: t, name: name.clone(), function_id: target_fn, count: n }),
                        }
                        if let Some(tf) = target_fn {
                            let pcs = &mut functions[tf].pointer_callers;
                            match pcs.iter_mut().find(|p| p.kind == "trace" && p.function_id == Some(fid)) {
                                Some(p) => p.count = p.count.saturating_add(n),
                                None => pcs.push(PointerCaller { kind: "trace".into(), symbol_id: caller_sym, name: caller_name, function_id: Some(fid), count: n }),
                            }
                        }
                    }
                    for f in &mut functions {
                        for ic in &mut f.indirect_calls {
                            ic.measured_count = seen(&ic.addr);
                        }
                    }
                }
                let (mut fn_exec, mut total_meas, mut disagreements) = (0usize, 0usize, 0usize);
                for f in &mut functions {
                    // every body of the function, its variants included: a
                    // trace is the evidence that says whether a variant ran
                    // (#266)
                    let mut any = false;
                    for o in f.object.iter_mut().chain(f.variants.iter_mut()) {
                        let mut mc = 0usize;
                        for b in &mut o.blocks {
                            b.measured = seen(&b.address);
                            for ins in &mut b.instrs {
                                ins.measured = seen(&ins.addr);
                                let ran = ins.measured.map(|c| c > 0).unwrap_or(false);
                                if ran {
                                    any = true;
                                    mc += 1;
                                }
                                let kind = match (ins.projected, ran) {
                                    (Status::Covered, false) => Some(DisagreementKind::ProjectedNotExecuted),
                                    (Status::Uncovered, true) => Some(DisagreementKind::ExecutedNotProjected),
                                    (Status::Unknown, true) => Some(DisagreementKind::ExecutedUnknown),
                                    _ => None,
                                };
                                if let Some(k) = kind {
                                    o.disagreements.push(Disagreement { addr: ins.addr, line: ins.line, kind: k, projected: ins.projected, measured: ins.measured.unwrap_or(0) });
                                }
                            }
                        }
                        // block-to-block transitions -> edge counts. A QEMU
                        // translation block ends at every branch, so a transition
                        // whose source TB ends at this block's last instruction and
                        // whose target is a block start of this function is an
                        // execution of that CFG edge.
                        let starts: HashMap<u64, usize> = o.blocks.iter().map(|b| (b.address, b.id)).collect();
                        let last_addr: HashMap<u64, usize> = o.blocks.iter().filter_map(|b| b.instrs.last().map(|i| (i.addr, b.id))).collect();
                        let mut edge_counts: HashMap<(usize, usize), u64> = HashMap::new();
                        for ((from_tb, to_tb), n) in &tr_transitions {
                            let Some(&to_b) = starts.get(to_tb) else { continue };
                            let from_last = tb_last.get(from_tb).copied().unwrap_or(*from_tb);
                            let Some(&from_b) = last_addr.get(&from_last) else { continue };
                            let ec = edge_counts.entry((from_b, to_b)).or_default();
                            *ec = ec.saturating_add(*n);
                            transitions_matched = transitions_matched.saturating_add(*n);
                        }
                        let mut oe_total = 0usize;
                        let mut oe_meas = 0usize;
                        for b in &mut o.blocks {
                            let cond = b.succs.len() == 2 && b.succs.iter().any(|e| e.kind == EdgeKind::Taken);
                            for e in &mut b.succs {
                                let n = edge_counts.get(&(b.id, e.to)).copied().unwrap_or(0);
                                e.measured = Some(n);
                                if cond {
                                    oe_total += 1;
                                    if n > 0 {
                                        oe_meas += 1;
                                    }
                                }
                            }
                        }
                        o.outcome_edges_total = oe_total;
                        o.outcome_edges_measured = Some(oe_meas);
                        o.instr_measured_covered = Some(mc);
                        total_meas += mc;
                        disagreements += o.disagreements.len();
                    }
                    if any {
                        fn_exec += 1;
                    }
                }
                let mut tcaveats = tr.caveats.clone();
                tcaveats.push("Measured coverage reflects the traced run only. Compare it with projected coverage only when the trace and the instrumented run execute the same test campaign on the same sources.".into());
                measured = Some(Measured {
                    source: tp.display().to_string(),
                    format: tr.format.clone(),
                    block_executions: tr.block_executions,
                    addresses_in_trace: counts.len(),
                    addresses_matched: matched,
                    addresses_outside_symbols: outside,
                    functions_executed: fn_exec,
                    instructions_measured_covered: total_meas,
                    disagreements,
                    caveats: tcaveats,
                    transitions: tr_transitions.values().copied().fold(0u64, u64::saturating_add),
                    transitions_matched,
                });
                measured_counts = Some(counts);
            }
        } else {
            caveats.push(format!("{}: only the first object file is used for the symbol table, stack, data-coupling and image analysis.", obj_path.display()));
        }
    }
    if opts.objects.is_empty() {
        caveats.push("No object file was given; object-code views, CFGs, call graphs and gap analysis are unavailable.".to_string());
    }

    // --- call graph ----------------------------------------------------------
    let is_macho = object_format == "MachO";
    for PendingCall { caller, callee_sym, callee_id, addr, line, from_variant } in pending_calls {
        let cov_name = if is_macho { callee_sym.strip_prefix('_').unwrap_or(&callee_sym).to_string() } else { callee_sym.clone() };
        let display = callee_id.map(|i| functions[i].name.clone()).unwrap_or_else(|| demangle(&cov_name));
        let site = CallSite { addr, line, measured: measured_counts.as_ref().map(|c| c.get(&addr).copied().unwrap_or(0)) };
        // A call in a constructor/destructor variant ran only if the variant
        // did, which no count says -- unless the body never ran, in which
        // case no variant did either (#266). Its line's count is the body's.
        // A function with no coverage record says nothing either way (#273):
        // not about its call sites, whose lines carry another function's
        // counts, and not as a callee, whose 0 is "not counted". A call to one
        // is then judged by its call site alone, as a call out of the covered
        // set is.
        let recorded = |i: usize| functions[i].unrecorded_within.is_none();
        let caller_line_status = if from_variant {
            (functions[caller].count == 0).then_some(0)
        } else {
            line.filter(|_| recorded(caller)).and_then(|l| line_count(&files, &functions[caller], functions[caller].file_id, l))
        };
        let exercised = match (callee_id.filter(|&i| recorded(i)).map(|i| functions[i].count), caller_line_status) {
            (Some(0), _) => Exercised::No,
            (_, Some(0)) => Exercised::No,
            (Some(n), Some(m)) if n > 0 && m > 0 => Exercised::Yes,
            (None, Some(m)) if m > 0 => Exercised::Yes, // external callee, call site executed
            _ => Exercised::Unknown,
        };
        {
            let f = &mut functions[caller];
            match f.callees.iter_mut().find(|e| e.symbol == display) {
                Some(e) => {
                    e.sites.push(site);
                    e.exercised = merge_exercised(e.exercised, exercised);
                }
                None => f.callees.push(CallEdge { function_id: callee_id, symbol: display.clone(), sites: vec![site], exercised, source_calls: 0 }),
            }
        }
        if let Some(cid) = callee_id {
            let caller_name = functions[caller].name.clone();
            let f = &mut functions[cid];
            match f.callers.iter_mut().find(|e| e.function_id == Some(caller)) {
                Some(e) => {
                    e.sites.push(site);
                    e.exercised = merge_exercised(e.exercised, exercised);
                }
                None => f.callers.push(CallEdge { function_id: Some(caller), symbol: caller_name, sites: vec![site], exercised, source_calls: 0 }),
            }
        }
    }
    // Calls written in the source with no object counterpart (call or inline),
    // and source-call counts per callee (to spot duplicated calls).
    {
        // candidate callee names: covered functions, plus every external
        // symbol called anywhere in the image, plus common libc builtins
        // only short names that identify exactly one covered function, and
        // that are not trivially common method names
        let mut short_count: HashMap<String, usize> = HashMap::new();
        for f in &functions {
            *short_count.entry(f.name.rsplit("::").next().unwrap_or(&f.name).to_string()).or_default() += 1;
        }
        let mut names: Vec<(String, Option<usize>)> = functions
            .iter()
            .map(|f| (f.name.rsplit("::").next().unwrap_or(&f.name).to_string(), Some(f.id)))
            .filter(|(n, _)| short_count.get(n) == Some(&1) && n.len() >= 3 && !n.contains('{') && !n.starts_with('<'))
            .collect();
        let mut externals: BTreeSet<String> = functions.iter().flat_map(|f| f.callees.iter().filter(|e| e.function_id.is_none()).map(|e| e.symbol.clone())).collect();
        for l in [
            "strlen", "strcmp", "strncmp", "strcpy", "strncpy", "memcpy", "memset", "memmove", "memcmp", "abs", "labs", "fabs", "sqrt", "floor", "ceil", "sin", "cos", "pow",
            "exp", "log", "printf", "puts", "putchar", "malloc", "free",
        ] {
            externals.insert(l.into());
        }
        for e in externals {
            if !names.iter().any(|(n, _)| *n == e) {
                names.push((e, None));
            }
        }
        // count occurrences of `name(` in a function body
        let count_calls = |file: &SourceFile, f: &Function, short: &str| -> u32 {
            let mut n = 0;
            let mut first_line = 0;
            let last = file.text.len().min(u32::MAX as usize) as u32;
            for l in f.line_start.max(1)..=f.line_end.min(last) {
                let Some(text) = file.text.get((l - 1) as usize) else { continue };
                let code = text.split("//").next().unwrap_or("");
                let mut from = 0;
                while let Some(pos) = code[from..].find(short) {
                    let a = from + pos;
                    let e = a + short.len();
                    let prev = if a == 0 { b' ' } else { code.as_bytes()[a - 1] };
                    let before = !(prev.is_ascii_alphanumeric() || prev == b'_');
                    // `.name(` is a method call on some receiver: not a call to a free function of that name
                    let not_method = prev != b'.';
                    let after = code[e..].trim_start().starts_with('(');
                    // `fn name(` is the definition, not a call
                    let not_def = !code[..a].trim_end().ends_with("fn");
                    if before && after && not_method && not_def {
                        n += 1;
                        if first_line == 0 {
                            first_line = l;
                        }
                    }
                    from = e;
                }
            }
            n | (first_line << 16)
        };
        for fid in 0..functions.len() {
            if functions[fid].object.is_none() {
                continue;
            }
            let file = &files[functions[fid].file_id];
            // source-call counts for existing callees
            let shorts: Vec<(usize, String)> = functions[fid].callees.iter().enumerate().map(|(i, e)| (i, e.symbol.rsplit("::").next().unwrap_or(&e.symbol).to_string())).collect();
            for (i, short) in shorts {
                let c = count_calls(file, &functions[fid], &short) & 0xffff;
                functions[fid].callees[i].source_calls = c;
            }
            let called: BTreeSet<String> = functions[fid]
                .callees
                .iter()
                .map(|e| e.symbol.rsplit("::").next().unwrap_or(&e.symbol).to_string())
                .chain(functions[fid].inlined.iter().map(|i| i.name.rsplit("::").next().unwrap_or(&i.name).to_string()))
                .collect();
            let mut folded = Vec::new();
            for (short, id) in &names {
                if *id == Some(fid) || called.contains(short) || short.is_empty() {
                    continue;
                }
                let r = count_calls(file, &functions[fid], short);
                let (n, line) = (r & 0xffff, r >> 16);
                if n > 0 {
                    folded.push(FoldedCall { name: id.map(|i| functions[i].name.clone()).unwrap_or_else(|| short.clone()), function_id: *id, line });
                }
            }
            if folded.is_empty() {
                continue;
            }
            let sym_name = functions[fid].object.as_ref().map(|o| o.symbol.clone()).unwrap_or_default();
            let entry = functions[fid].object.as_ref().map(|o| o.address).unwrap_or(0);
            for fc in &folded {
                let o = functions[fid].object.as_mut().unwrap();
                let id = o.gaps.len();
                o.gaps.push(Gap {
                    id,
                    kind: GapKind::FoldedCall,
                    addr_start: entry,
                    addr_end: entry,
                    line: Some(fc.line),
                    description: format!("line {}: call to `{}` written in the source has no object counterpart — neither a call instruction nor an inlined subroutine; the compiler evaluated it away ({}). Its structural coverage on this path exists only at source level.", fc.line, fc.name, if fc.function_id.is_some() { "constant folding / interprocedural optimisation" } else { "library builtin evaluated at compile time" }),
                    key: format!("{sym_name}@folded-call@{}", fc.name),
                    class: None,
                });
            }
            functions[fid].folded_calls = folded;
        }
    }
    for f in &mut functions {
        f.complexity.calls_out = f.callees.len() as u32;
        f.callees.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        f.callers.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    }

    // --- summary -----------------------------------------------------------------
    let mut summary = Summary { files: files.len(), functions: functions.len(), ..Default::default() };
    for f in &files {
        summary.lines_total += f.lines_total;
        summary.lines_covered += f.lines_covered;
    }
    for f in &functions {
        if f.count > 0 {
            summary.functions_executed += 1;
        }
        if f.unrecorded_within.is_some() {
            summary.functions_unrecorded += 1;
        }
        summary.branches_total += f.branches.len() * 2;
        summary.branches_covered += f.branches.iter().map(|b| (b.true_count > 0) as usize + (b.false_count > 0) as usize).sum::<usize>();
        summary.decisions_total += f.decisions.len();
        summary.decisions_mcdc_covered += f.decisions.iter().filter(|d| d.mcdc_covered).count();
        // (decisions_unplaced is image-wide, set once after this loop)
        summary.conditions_total += f.decisions.iter().map(|d| d.conditions.len()).sum::<usize>();
        summary.conditions_independence_covered += f.decisions.iter().flat_map(|d| &d.conditions).filter(|c| c.independence_covered).count();
        // every body, the variants included (#266): their instructions are
        // object code in the image, projected unknown, and their gaps are
        // findings
        for o in f.objects() {
            summary.instructions_total += o.instr_total;
            summary.instructions_projected_covered += o.instr_projected_covered;
            summary.instructions_projected_uncovered += o.instr_projected_uncovered;
            summary.instructions_unknown += o.instr_unknown;
            summary.gaps_total += o.gaps.len();
            for g in &o.gaps {
                *summary.gaps_by_kind.entry(gaps::kind_slug(g.kind).to_string()).or_default() += 1;
            }
        }
    }

    // MC/DC decisions that could not be placed are absent from every function's
    // `decisions`, so they are counted here rather than lost to a caveat: the
    // gate treats them as unproven obligations (#284, #269).
    summary.decisions_unplaced = unplaced_decisions.len();

    let mut best: Option<(u64, usize)> = None;
    for s in &symbols {
        if let Some(w) = s.stack.worst_case {
            if best.map(|(b, _)| w > b).unwrap_or(true) {
                best = Some((w, s.id));
            }
        }
    }
    summary.stack_worst_case = best.map(|b| b.0);
    summary.stack_worst_symbol = best.map(|b| b.1);
    summary.stack_unbounded = symbols.iter().filter(|s| !s.stack.bounded).count();
    summary.globals = globals.len();
    summary.data_refs = functions.iter().map(|f| f.data_refs.len()).sum();
    summary.instructions_measured_covered = measured.as_ref().map(|m| m.instructions_measured_covered);
    for f in &functions {
        for o in f.objects() {
            for (k, n) in &o.compiler_generated {
                summary.instructions_compiler_generated += n;
                *summary.compiler_generated_by_origin.entry(k.clone()).or_default() += n;
            }
        }
    }
    summary.outcome_edges_total = functions.iter().flat_map(|f| f.objects()).map(|o| o.outcome_edges_total).sum();
    summary.outcome_edges_measured = measured.as_ref().map(|_| functions.iter().flat_map(|f| f.objects()).map(|o| o.outcome_edges_measured.unwrap_or(0)).sum());
    summary.disagreements = measured.as_ref().map(|m| m.disagreements).unwrap_or(0);
    if let Some(m) = &measured {
        caveats.extend(m.caveats.iter().cloned());
    }
    // gcov writes an empty `branches` array on every line with or without
    // `-b`, so a straight-line program run through `gcov -b` looks exactly
    // like a forgotten flag. Where every function the export describes was
    // found and none has a conditional branch, telling that project to re-run
    // gcov with `-b` may send it after a flag it already passed (#325) -- so
    // that instruction is dropped. The caveat itself is not: no conditional
    // jump is not no source decision, since a decision lowered to a `csel` or
    // predication has none either (#335 review), and no outcome was supplied
    // for it. Where any function's code was not found, or branches, or the
    // export itself recorded a decision, the records are missing and the
    // advice stands.
    if !cov.decision_sites && !functions.is_empty() && functions.iter().all(|f| f.object.is_some() && f.complexity.object_conditional_branches == 0) {
        for c in caveats.iter_mut().filter(|c| c.as_str() == gcov::NO_BRANCH_CAVEAT) {
            *c = gcov::NO_BRANCH_NO_JUMP_CAVEAT.into();
        }
    }

    // the requirements universe, when one was supplied: IDs the project says
    // exist, whether or not anything is tagged with them
    let requirements_universe = match &opts.requirements_list {
        Some(p) => {
            let t = limits::read_bounded_string(p, limits::Kind::Auxiliary, &lim).map_err(|e| anyhow::anyhow!("{e}"))?;
            inputs.push(artifact("requirements-list", p, t.as_bytes()));
            let mut v: Vec<String> = t.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_string).collect();
            v.sort();
            v.dedup();
            v
        }
        None => Vec::new(),
    };
    // --- the project's source revision (#300) ------------------------------
    // Judged over the files the build used, with the bytes read above for
    // the coverage files; never a failure, always a caveat when not clean.
    let source_revision = {
        let mut files: Vec<revision::BuildFile> =
            cov.files.iter().filter_map(|cf| bound.read.get(&cf.filename)).map(|(real, bytes)| revision::BuildFile { path: real.clone(), bytes: Some(bytes.as_slice()) }).collect();
        files.extend(dwarf_files.iter().map(|f| revision::BuildFile { path: PathBuf::from(f), bytes: None }));
        let mut req = revision::Request::new(bound.root.clone(), files);
        req.max_file_bytes = lim.cap(limits::Kind::Source);
        revision::inspect(&req)
    };
    // the git or svn that ran, and for SVN the svnversion beside it (#301)
    for t in source_revision.tool.iter().chain(source_revision.svn.as_ref().and_then(|s| s.svnversion_tool.as_ref())) {
        provenance::record(&mut external_tools, &mut caveats, t.clone(), None);
    }
    if let Some(c) = revision::caveat(&source_revision) {
        caveats.push(c);
    }
    let provenance = Provenance {
        tool: TOOL.into(),
        tool_version: TOOL_VERSION.into(),
        tool_commit: TOOL_COMMIT.into(),
        tool_dirty: tool_dirty(),
        created_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        host: hostname(),
        inputs,
        build_info,
        coverage_export_version: cov.version.clone(),
        object_format,
        arch,
        external_tools,
        source_revision: Some(source_revision),
        caveats,
    };
    let mut ev = Evidence {
        schema: SCHEMA.into(),
        provenance,
        summary,
        files,
        functions,
        tests: Vec::new(),
        requirements_universe,
        symbols,
        globals,
        image,
        measured,
        baseline: None,
        changes: None,
    };

    // --- per-test attribution (#41) --------------------------------------
    // After the ids are final, and on the finished evidence: attribution is
    // a join onto what the merged export already established, never a second
    // opinion about it. No count changes here.
    let mut test_caveats: Vec<String> = Vec::new();
    // Two exports can share a file stem -- suites/a/t1.json and
    // suites/b/t1.json -- and two attributions under one name would be
    // indistinguishable everywhere the name is shown. Ids stay the identity;
    // the name is what a person reads, so collisions are made visible.
    let mut names_taken: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for tp in &opts.tests {
        // Bounded like every other untrusted input (#5): a per-test export is
        // a coverage export, so it takes the coverage cap and the run budget.
        // And every failure here is this export's alone -- a missing or
        // unreadable one costs its own attribution, never the ingest.
        let outcome = limits::read_bounded_string(tp, limits::Kind::Coverage, &lim).map_err(|e| e.to_string()).and_then(|text| {
            // the id is the position it will occupy, assigned on success:
            // taking it from the input list leaves holes when an export
            // is rejected, and a hole is an index into nothing
            let base = pertest::name_for(tp);
            let n = names_taken.entry(base.clone()).or_insert(0);
            *n += 1;
            let name = if *n == 1 { base } else { format!("{base} ({n})") };
            pertest::attribute(&ev, ev.tests.len(), &name, tp, &text).map(|(run, matches)| (run, matches, text))
        });
        match outcome {
            Ok((run, matches, text)) => {
                ev.provenance.inputs.push(artifact("coverage-per-test", tp, text.as_bytes()));
                let id = run.id;
                ev.tests.push(run);
                // Applied only now, on an export already accepted whole: a
                // contradiction found later in `attribute` must leave no
                // trace, and staging the matches until the export passes is
                // what makes that true (#48).
                pertest::apply_vector_matches(&mut ev, id, &matches);
            }
            Err(e) => test_caveats.push(format!("test `{}` ({}): {e}", pertest::name_for(tp), tp.display())),
        }
    }
    // the reverse index the viewer and the requirements matrix both want
    for t in &ev.tests {
        for &f in &t.functions {
            ev.functions[f].tests.push(t.id);
        }
    }
    if !opts.tests.is_empty() {
        // say what happened to every export that was offered, not only to the
        // ones that worked
        test_caveats.push(format!("per-test attribution: {} of {} export(s) attributed.", ev.tests.len(), opts.tests.len()));
        let untested = ev.functions.iter().filter(|f| f.tests.is_empty() && f.count > 0).count();
        if untested > 0 {
            test_caveats.push(format!(
                "{untested} function(s) were executed in the merged coverage but by none of the {} attributed per-test export(s). \
                 Coverage that no named test accounts for cannot be shown to come from requirements-based testing (DO-178C 6.4.4).",
                ev.tests.len()
            ));
        }
    }
    ev.provenance.caveats.extend(test_caveats);
    if let Some(bp) = &opts.baseline {
        let t = read_input(bp, limits::Kind::Auxiliary, &lim, by_name(bp, "--baseline"))?;
        let base: Baseline = serde_json::from_str(&t).with_context(|| format!("{} is not a butterfly baseline", bp.display()))?;
        base.check_schema().map_err(|e| anyhow::anyhow!("{}: {e}", bp.display()))?;
        ev.provenance.inputs.push(artifact("baseline", bp, t.as_bytes()));
        let check = baseline::compare(&base, &ev, &bp.display().to_string());
        if check.status == "changed" {
            ev.provenance.caveats.insert(
                0,
                format!(
                    "BASELINE CHANGED (4.4.2.c): {}. Coverage and source-to-object analyses done against the baseline may no longer be valid.",
                    check.differences.iter().filter(|d| d.invalidates).map(|d| d.detail.clone()).collect::<Vec<_>>().join("; ")
                ),
            );
        }
        ev.baseline = Some(check);
    }
    if let Some(cp) = &opts.changes {
        // git is deliberately not consulted here: ingest runs on build
        // artefacts, often where there is no repository. The cache carries
        // enough to prove it still describes these sources, or to say it
        // does not.
        let cache_text = read_input(cp, limits::Kind::Auxiliary, &lim, by_name(cp, ""));
        match cache_text.as_ref().map_err(|e| anyhow::anyhow!("{e}")).and_then(|t| change::parse(t, cp)) {
            Ok(cache) => {
                let pats = Some(config.change.patterns.clone());
                match change::embed(&mut ev, &cache, pats.as_deref()) {
                    Ok(()) => ev.provenance.inputs.push(artifact("changes", cp, cache_text.as_deref().unwrap_or_default().as_bytes())),
                    Err(e) => ev.provenance.caveats.push(format!("{}: change history NOT attached — {e}", cp.display())),
                }
            }
            Err(e) => ev.provenance.caveats.push(format!("{}: change history NOT attached — {e}", cp.display())),
        }
    }
    Ok(ev)
}

/// Blocks nothing reaches, or none when that cannot be established: an
/// indirect branch whose targets were not recovered may reach any of them,
/// and a switch's case blocks used to be listed as unreachable for exactly
/// that reason (#171).
fn unreachable(blocks: &[Block]) -> Vec<usize> {
    let unresolved = blocks.iter().any(|b| b.succs.is_empty() && b.instrs.last().is_some_and(|i| i.flags.contains(&InstrFlag::IndirectBranch)));
    if unresolved {
        return Vec::new();
    }
    let mut has_pred = vec![false; blocks.len()];
    for b in blocks {
        for e in &b.succs {
            has_pred[e.to] = true;
        }
    }
    (1..blocks.len()).filter(|&i| !has_pred[i]).collect()
}

/// Branch-outcome inference (refines line-level projection).
///
/// When llvm-cov reports that exactly one outcome of the branch at line L was
/// never taken, and of the two successors of the block ending with that
/// branch one has *exclusive* covered source evidence (a line no other block
/// owns, with count > 0) while the other has none, the latter is the
/// never-taken outcome: it is projected uncovered even though the lines its
/// instructions borrow (a merged `ret`, a common epilogue) executed on the
/// other path. Applied only when every predecessor of the target block agrees.
/// Whether control can flow from block `from` to block `to` along CFG edges.
fn reaches(blocks: &[Block], from: usize, to: usize) -> bool {
    let mut seen = vec![false; blocks.len()];
    let mut stack = vec![from];
    while let Some(b) = stack.pop() {
        if b == to {
            return true;
        }
        if std::mem::replace(&mut seen[b], true) {
            continue;
        }
        stack.extend(blocks[b].succs.iter().map(|e| e.to));
    }
    false
}

fn infer_outcomes(blocks: &mut [Block], f: &Function, count: &dyn Fn(u32) -> Option<u64>) -> Vec<(usize, String)> {
    let mut owners: HashMap<u32, usize> = HashMap::new();
    for b in blocks.iter() {
        for l in &b.lines {
            *owners.entry(*l).or_default() += 1;
        }
    }
    let exclusive_cov = |b: &Block| b.lines.iter().any(|l| owners.get(l) == Some(&1) && count(*l).map(|c| c > 0).unwrap_or(false));
    // preds
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); blocks.len()];
    for b in blocks.iter() {
        for e in &b.succs {
            preds[e.to].push(b.id);
        }
    }
    // a cond-branch block "votes" a successor uncovered when its line has one
    // never-taken outcome and the sibling successor has exclusive evidence
    let zero_outcome_line = |b: &Block| -> Option<(u32, &'static str)> {
        let last = b.instrs.last()?;
        if !last.flags.contains(&InstrFlag::CondBranch) {
            return None;
        }
        let l = last.line.filter(|l| *l > 0)?;
        let brs: Vec<&Branch> = f.branches.iter().filter(|br| br.line == l).collect();
        if brs.len() != 1 {
            return None;
        }
        match (brs[0].true_count == 0, brs[0].false_count == 0) {
            (true, false) => Some((l, if brs[0].direction_known { "the true outcome" } else { "one outcome" })),
            (false, true) => Some((l, if brs[0].direction_known { "the false outcome" } else { "one outcome" })),
            _ => None,
        }
    };
    let mut out = Vec::new();
    for z in 0..blocks.len() {
        if blocks[z].projected != Status::Covered || exclusive_cov(&blocks[z]) || preds[z].is_empty() {
            continue;
        }
        let mut reasons = Vec::new();
        let mut ok = true;
        for &p in &preds[z] {
            let pb = &blocks[p];
            let Some((l, which)) = zero_outcome_line(pb) else {
                ok = false;
                break;
            };
            let sibling = pb.succs.iter().map(|e| e.to).find(|&t| t != z);
            match sibling {
                // The sibling's coverage says which outcome ran only if z
                // cannot flow into it. When it can, the sibling is the join
                // after an `if` with no `else` -- run whichever way the branch
                // went -- and z may well be the outcome that was taken
                // (examples/c-constructs `arm`, #168).
                Some(sib) if reaches(blocks, z, sib) => {
                    ok = false;
                    break;
                }
                Some(sib) if exclusive_cov(&blocks[sib]) || (sib != z && blocks[sib].projected == Status::Covered && sib != p) => {
                    reasons.push(format!("{which} of the branch at L{l} (B{p}) was never taken while its other successor B{sib} has exclusive covered evidence"));
                }
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok || reasons.is_empty() {
            continue;
        }
        let shared: Vec<String> = blocks[z].lines.iter().map(|l| format!("L{l}")).collect();
        let why = format!(
            "B{z} is reachable only through a never-taken branch outcome: {}. Its instructions borrow source lines shared with other paths ({}), so line counts alone would show it covered; projected UNCOVERED by branch-outcome inference. Verify against a trace.",
            reasons.join("; "),
            if shared.is_empty() { "line 0 only".to_string() } else { shared.join(", ") }
        );
        for ins in &mut blocks[z].instrs {
            ins.projected = Status::Uncovered;
        }
        // Source lines the never-taken outcome implements: the run of
        // zero-count lines without object code that follows the branch line.
        let branch_line = preds[z].iter().filter_map(|&p| blocks[p].instrs.last().and_then(|i| i.line)).min().unwrap_or(0);
        let mut inferred_lines = Vec::new();
        if let Some(start) = (branch_line > 0).then(|| branch_line.checked_add(1)).flatten() {
            for l in start..=f.line_end {
                if owners.contains_key(&l) {
                    break;
                }
                match count(l) {
                    Some(0) => inferred_lines.push(l),
                    Some(_) => break,
                    None => {}
                }
            }
        }
        blocks[z].projected = Status::Uncovered;
        blocks[z].note = Some(why.clone());
        blocks[z].inferred_lines = inferred_lines;
        out.push((z, why));
    }
    // What only an inferred-uncovered block leads to did not run either (#168):
    // a macro's `if (!(c)) { r = -1; return; }` compiles to the never-taken
    // arm and a block after it, and every instruction borrows the use line,
    // so the second block read covered behind an uncovered edge. Followed only
    // from blocks this inference marked, and only when the CFG's predecessor
    // sets are complete: an unresolved indirect branch could reach any block.
    let unresolved_indirect = blocks.iter().any(|b| b.instrs.last().is_some_and(|i| i.flags.contains(&InstrFlag::IndirectBranch)) && b.succs.is_empty());
    let mut inferred: BTreeSet<usize> = out.iter().map(|(z, _)| *z).collect();
    let mut grew = !inferred.is_empty() && !unresolved_indirect;
    while grew {
        grew = false;
        for z in 0..blocks.len() {
            if inferred.contains(&z) || blocks[z].projected != Status::Covered || exclusive_cov(&blocks[z]) || preds[z].is_empty() {
                continue;
            }
            if !preds[z].iter().all(|p| inferred.contains(p)) {
                continue;
            }
            let from: Vec<String> = preds[z].iter().map(|p| format!("B{p}")).collect();
            let why = format!(
                "B{z} is reached only from {}, which branch-outcome inference projected UNCOVERED; its instructions borrow source lines shared with other paths, so line counts alone would show it covered. Verify against a trace.",
                from.join(", ")
            );
            for ins in &mut blocks[z].instrs {
                ins.projected = Status::Uncovered;
            }
            blocks[z].projected = Status::Uncovered;
            blocks[z].note = Some(why.clone());
            out.push((z, why));
            inferred.insert(z);
            grew = true;
        }
    }
    out.extend(infer_entered(blocks, &inferred, &exclusive_cov, &owners, &count));
    if !out.is_empty() {
        cfg::project_edges(blocks);
    }
    out
}

/// The opposite direction to the inference above (#270): a block whose own
/// lines have count 0 although control must have entered it. At -O0 every
/// `return` of a goto-cleanup function jumps to one epilogue, which DWARF
/// gives the closing brace; llvm-cov counts that line inside the last label's
/// region, 0 when no goto was taken, so the epilogue that ran on every call
/// was projected uncovered.
///
/// Inferring *covered* is the direction that can hide a real gap, so the rule
/// takes nothing but what the evidence forces. A block `p` was entered when
///   (a) it holds the only object code of a line whose count is non-zero --
///       the evidence `exclusive_cov` already stands on -- or it was itself
///       entered by this rule; and branch-outcome inference did not mark it.
/// Control then certainly left `p` for its successor `z` when
///   (b) `p` contains no call, since a callee need not return; and
///   (c) `p` has exactly one successor, reached by an unconditional jump or
///       by falling through -- no conditional, indirect or return terminator.
/// A basic block is entered only at its first instruction and runs straight
/// through, so under (b) and (c) every entry into `p` is followed by an entry
/// into `z`. `z` is then projected covered, with a note saying so.
///
/// Why this cannot hide a real gap: `z` is marked covered only when `p` ran.
/// If `z` did not run, `p` did not run either, so `p`'s own covered
/// projection is already false -- the one premise every projection rests on,
/// that a counted line's exclusive object code executed. The rule adds no
/// claim beyond that premise, and never runs backwards, across a branch, a
/// call, or into a block branch-outcome inference marked uncovered (on that
/// conflict the uncovered answer stands). Nothing that is not proven entered
/// changes: it keeps the projection of its own line.
fn infer_entered(
    blocks: &mut [Block],
    uncovered: &BTreeSet<usize>,
    exclusive_cov: &dyn Fn(&Block) -> bool,
    owners: &HashMap<u32, usize>,
    count: &dyn Fn(u32) -> Option<u64>,
) -> Vec<(usize, String)> {
    let has_call = |b: &Block| b.instrs.iter().any(|i| i.flags.contains(&InstrFlag::Call) || i.flags.contains(&InstrFlag::IndirectCall));
    let unconditional = |b: &Block| {
        b.succs.len() == 1
            && matches!(b.succs[0].kind, EdgeKind::Jump | EdgeKind::Fallthrough)
            && b.instrs.last().is_some_and(|i| !i.flags.iter().any(|f| matches!(f, InstrFlag::CondBranch | InstrFlag::IndirectBranch | InstrFlag::Return)))
    };
    // why each block is known to have been entered
    let mut entered: Vec<Option<String>> = blocks
        .iter()
        .map(|b| {
            if uncovered.contains(&b.id) || !exclusive_cov(b) {
                return None;
            }
            let l = b.lines.iter().find(|l| owners.get(*l) == Some(&1) && count(**l).is_some_and(|c| c > 0))?;
            Some(format!("B{} holds the only object code of L{l}, which ran ({}x)", b.id, count(*l).unwrap_or(0)))
        })
        .collect();
    let mut out = Vec::new();
    let mut grew = true;
    while grew {
        grew = false;
        for p in 0..blocks.len() {
            let Some(because) = entered[p].clone() else { continue };
            if has_call(&blocks[p]) || !unconditional(&blocks[p]) {
                continue;
            }
            let z = blocks[p].succs[0].to;
            // A call inside z is not necessarily a return: an un-annotated
            // throw/longjmp/exit does not split the block (cfg::build only
            // splits at a DW_AT_noreturn call), so entering z establishes
            // execution up to that call, never the instructions after it.
            // Promoting the whole block would flip their real zero counts to
            // Covered. Require a call-free successor -- only a block every
            // instruction of which is reached once z is entered is promoted
            // (#270 review). The shared goto epilogue this rule exists for is
            // call-free, so nothing legitimate is lost.
            if entered[z].is_some() || uncovered.contains(&z) || has_call(&blocks[z]) {
                continue;
            }
            entered[z] = Some(format!("B{z} was entered from B{p}, and {because}"));
            grew = true;
            // Only a block its line counts call uncovered is changed: that is
            // the false answer this corrects. One with no projection (line 0)
            // keeps it -- the trace, not an inference, settles those.
            if !blocks[z].instrs.iter().any(|i| i.projected == Status::Uncovered) {
                continue;
            }
            let lines: Vec<String> = blocks[z].lines.iter().map(|l| format!("L{l} ({}x)", count(*l).map_or("no count".into(), |c| c.to_string()))).collect();
            let why = format!(
                "B{z} is inferred COVERED from control flow: {because}; B{p} contains no call and its only successor is B{z}, so each time B{p} ran, B{z} was entered. Its own line(s) {} say otherwise because the block borrows them (a shared epilogue is given the closing brace, which llvm-cov counts in whatever region encloses it, such as a never-taken label's), so line counts alone would show it uncovered. Verify against a trace.",
                if lines.is_empty() { "line 0 only".to_string() } else { lines.join(", ") }
            );
            for ins in blocks[z].instrs.iter_mut().filter(|i| i.projected == Status::Uncovered) {
                ins.projected = Status::Covered;
            }
            blocks[z].projected = Status::Covered;
            blocks[z].note = Some(why.clone());
            out.push((z, why));
        }
    }
    out
}

/// Why each instruction exists (A-7 objective 9): directly traceable to a
/// source statement, or generated by the compiler for a recognisable reason.
fn classify_origins(arch: &str, blocks: &mut [Block], inlined: &[Inlined], gaps: &[Gap]) {
    let aarch64 = arch.starts_with("aarch64");
    let x86 = arch.starts_with("x86");
    // AArch64 callee-saved registers: x19-x28, x29 (fp), x30 (lr), d8-d15
    let callee_saved_regs = |ops: &str| -> bool {
        ops.split([',', '[', ']', ' ', '!']).filter(|t| !t.is_empty() && !t.starts_with('#')).take_while(|t| !t.starts_with("sp")).any(|t| {
            let t = t.trim();
            matches!(t, "x29" | "x30" | "fp" | "lr")
                || t.strip_prefix('x').and_then(|n| n.parse::<u32>().ok()).map(|n| (19..=28).contains(&n)).unwrap_or(false)
                || t.strip_prefix('d').and_then(|n| n.parse::<u32>().ok()).map(|n| (8..=15).contains(&n)).unwrap_or(false)
        })
    };
    let is_callee_saved_store = |m: &str, ops: &str| {
        if aarch64 {
            (m == "stp" || m == "str") && ops.contains("[sp") && callee_saved_regs(ops)
        } else if x86 {
            m.starts_with("push")
        } else {
            false
        }
    };
    let is_callee_saved_load = |m: &str, ops: &str| {
        if aarch64 {
            (m == "ldp" || m == "ldr") && ops.contains("[sp") && callee_saved_regs(ops)
        } else if x86 {
            m.starts_with("pop") || m == "leave"
        } else {
            false
        }
    };
    let sp_adjust = |m: &str, ops: &str| {
        let o = ops.replace(' ', "");
        if aarch64 {
            (m == "sub" || m == "add") && o.starts_with("sp,sp,") || (m == "mov" && (o == "x29,sp" || o.starts_with("x29,sp"))) || (m == "add" && o.starts_with("x29,sp,"))
        } else if x86 {
            (m.starts_with("sub") || m.starts_with("add")) && o.ends_with(",%rsp") || (m.starts_with("mov") && (o == "%rsp,%rbp" || o == "%rbp,%rsp"))
        } else {
            false
        }
    };
    // helper-call gaps by address -> class
    let helper_class: HashMap<u64, String> = gaps.iter().filter(|g| g.kind == GapKind::HelperCall).map(|g| (g.addr_start, g.class.clone().unwrap_or_default())).collect();
    // blocks whose only purpose is to call a runtime check (panic / bounds)
    let mut check_blocks: Vec<bool> = vec![false; blocks.len()];
    for (bi, b) in blocks.iter().enumerate() {
        let calls_check = b.instrs.iter().any(|i| i.flags.contains(&InstrFlag::Call) && helper_class.get(&i.addr).map(|c| c == "runtime-check").unwrap_or(false));
        if calls_check && b.instrs.len() <= 8 {
            check_blocks[bi] = true;
        }
    }
    let n = blocks.len();
    for bi in 0..n {
        let is_entry = bi == 0;
        let last_idx = blocks[bi].instrs.len().saturating_sub(1);
        let ends_with_ret = blocks[bi].instrs.last().map(|i| i.flags.contains(&InstrFlag::Return)).unwrap_or(false);
        let cond_to_check = blocks[bi].succs.iter().any(|e| e.kind == EdgeKind::Taken && check_blocks[e.to]);
        // the compare+branch that guards the check: the branch and the
        // compares just before it, not the statement's own arithmetic
        let guard_from = if cond_to_check {
            let is_compare = |m: &str| m.starts_with("cmp") && !m.starts_with("cmpxchg") || m.starts_with("test") || matches!(m, "cmn" | "tst" | "ccmp" | "ccmn");
            let ins = &blocks[bi].instrs;
            (0..last_idx).rev().take_while(|&k| is_compare(&ins[k].mnemonic.to_ascii_lowercase())).last().unwrap_or(last_idx)
        } else {
            usize::MAX
        };
        for ii in 0..blocks[bi].instrs.len() {
            let (m, ops, line, is_call, addr) = {
                let i = &blocks[bi].instrs[ii];
                (i.mnemonic.to_ascii_lowercase(), i.operands.clone(), i.line, i.flags.contains(&InstrFlag::Call), i.addr)
            };
            let mut origin: Option<String> = None;
            if let Some(k) = blocks[bi].instrs[ii].inlined {
                origin = inlined.get(k).map(|inl| format!("inlined:{}", inl.name));
            }
            if origin.is_none() {
                // the second half of the old stack_chk test repeated the
                // first (`A || (B && C && A)` is just `A`), so it never
                // decided anything
                if ops.contains("stack_chk") {
                    origin = Some("stack-protector".into());
                } else if (is_call && helper_class.get(&addr).map(|c| c == "runtime-check").unwrap_or(false))
                    || check_blocks[bi]
                    // the compare + branch that guards the check
                    || ii >= guard_from
                {
                    origin = Some("runtime-check".into());
                } else if is_entry && ii < 8 && (is_callee_saved_store(&m, &ops) || sp_adjust(&m, &ops)) {
                    origin = Some("prologue".into());
                } else if ends_with_ret && ii + 6 >= last_idx && (is_callee_saved_load(&m, &ops) || sp_adjust(&m, &ops) || m.starts_with("autibsp") || m.starts_with("retab")) {
                    origin = Some("epilogue".into());
                } else if (m.starts_with("nop") || m == "udf" || m == "brk" || m == "int3" || m == "ud2" || (x86 && m.starts_with("xchg") && ops.replace(' ', "") == "%ax,%ax"))
                    && (line.is_none() || line == Some(0))
                {
                    // x86-64 pads with the multi-byte forms too: `nopw
                    // %cs:0x0(%rax,%rax)`, `nopl (%rax)`, `xchg %ax, %ax`
                    origin = Some("padding".into());
                } else if line == Some(0) {
                    origin = Some("line-0".into());
                } else if line.is_none() {
                    origin = Some("no-line".into());
                }
            }
            blocks[bi].instrs[ii].origin = origin;
        }
    }
}

/// Possible pointer values encoded in a data slot. Mach-O executables use
/// chained fixups: a rebase slot (bit 63 clear) carries the target in its low
/// 36 bits, relative to the image base; bind slots (bit 63 set) are imports.
fn pointer_candidates(v: u64, macho: bool, image_base: u64) -> Vec<u64> {
    let mut c = vec![v];
    if macho {
        if v >> 63 == 0 {
            c.push(image_base.wrapping_add(v & 0xF_FFFF_FFFF));
        } else {
            c.clear();
        }
    }
    c
}

fn merge_exercised(a: Exercised, b: Exercised) -> Exercised {
    match (a, b) {
        (Exercised::Yes, _) | (_, Exercised::Yes) => Exercised::Yes,
        (Exercised::No, _) | (_, Exercised::No) => Exercised::No,
        _ => Exercised::Unknown,
    }
}

fn build_decision(m: &coverage::RawMcdc, branches: &[Branch]) -> Decision {
    // Conditions are the MC/DC branch regions inside the decision span, in
    // source order.
    let inside: Vec<&Branch> = branches.iter().filter(|b| (b.line, b.col) >= (m.line_start, m.col_start) && (b.line, b.col) <= (m.line_end, m.col_end)).collect();
    let n = m.condition_covered.len().max(inside.len());
    let vectors: Vec<TestVector> = m.vectors.iter().map(|v| TestVector { values: v.conditions.clone(), result: v.result, executed: v.executed, tests: Vec::new() }).collect();
    let mut conditions = Vec::with_capacity(n);
    for i in 0..n {
        let b = inside.get(i);
        conditions.push(Condition {
            index: i,
            line: b.map(|b| b.line).unwrap_or(m.line_start),
            col: b.map(|b| b.col).unwrap_or(m.col_start),
            true_count: b.map(|b| b.true_count).unwrap_or(0),
            false_count: b.map(|b| b.false_count).unwrap_or(0),
            independence_covered: m.condition_covered.get(i).copied().unwrap_or(false),
            pair: find_pair(&vectors, i),
        });
    }
    let mcdc_covered = !conditions.is_empty() && conditions.iter().all(|c| c.independence_covered);
    Decision {
        line: m.line_start,
        col: m.col_start,
        line_end: m.line_end,
        col_end: m.col_end,
        true_count: m.true_count,
        false_count: m.false_count,
        conditions,
        vectors,
        mcdc_covered,
        decision_ordinal: None,
    }
}

/// Independence pair for condition `i`: two executed vectors whose result
/// differs, whose value for `i` differs, and whose other conditions are equal
/// or masked (`None`) in at least one of them.
fn find_pair(vectors: &[TestVector], i: usize) -> Option<(usize, usize)> {
    for a in 0..vectors.len() {
        for b in a + 1..vectors.len() {
            let (va, vb) = (&vectors[a], &vectors[b]);
            if !va.executed || !vb.executed || va.result == vb.result {
                continue;
            }
            let (Some(x), Some(y)) = (va.values.get(i).copied().flatten(), vb.values.get(i).copied().flatten()) else { continue };
            if x == y {
                continue;
            }
            let others_ok = va.values.iter().zip(&vb.values).enumerate().all(|(j, (p, q))| j == i || p == q || p.is_none() || q.is_none());
            if others_ok {
                return Some((a + 1, b + 1));
            }
        }
    }
    None
}

/// The covered function a demangled symbol name is written inside, if any:
/// the longest prefix of `name`, cut at a `::` outside every bracket, that
/// `covered` answers to. `constructs::closures::{closure#1}` is inside
/// `constructs::closures`; `fc::lambdas(int const*, int, int)::$_1::__invoke(int)`
/// is inside `fc::lambdas(int const*, int, int)` (#273).
///
/// Only a cut at nesting depth 0 is a path step. A `::` inside generic
/// arguments or a parameter list names some *other* item, and
/// `each_sample::<constructs::closures::{closure#1}>` is a copy of
/// `each_sample`, not code inside `closures`. The name itself must not be an
/// answer: that is a function with a record, not code inside one. A name
/// whose brackets this cannot balance (an `operator<` in it, say) yields
/// nothing, and the symbol stays unattributed, as it was.
pub fn enclosing_function(name: &str, covered: impl Fn(&str) -> Option<usize>) -> Option<usize> {
    let bytes = name.as_bytes();
    let mut depth = 0i32;
    let mut cuts = Vec::new();
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            b'<' | b'(' | b'[' | b'{' => depth += 1,
            // `->` in a Rust `fn(i32) -> i32` type is not a closing bracket
            b'>' if i > 0 && bytes[i - 1] == b'-' => {}
            b'>' | b')' | b']' | b'}' => depth -= 1,
            // `f::<T>` is one item, turbofish and all: no step before `<`
            b':' if depth == 0 && i > 0 && bytes[i - 1] == b':' && bytes.get(i + 1) != Some(&b'<') => cuts.push(i - 1),
            _ => {}
        }
        if depth < 0 {
            return None;
        }
    }
    if depth != 0 {
        return None;
    }
    cuts.iter().rev().find_map(|&at| covered(&name[..at]))
}

/// Demangle a Rust (legacy or v0) or C++ (Itanium) symbol; anything else is
/// returned unchanged, so C and assembly names pass straight through.
pub fn demangle(name: &str) -> String {
    // llvm-cov names an internal-linkage function `<file>:<symbol>` so that
    // two files can each have their own `static` of the same name. A C++
    // `static` function or one in an anonymous namespace is still mangled
    // underneath, so demangle the tail and put the file prefix back.
    if let Some((file, sym)) = name.split_once(':') {
        // `_Z…`/`_R…`, or the Mach-O `__Z…`/`__R…` spelling of the same
        if ["_Z", "_R", "__Z", "__R"].iter().any(|p| sym.starts_with(p)) {
            let d = demangle(sym);
            if d != sym {
                return format!("{file}:{d}");
            }
        }
    }
    // Mach-O prefixes every symbol with an underscore, so a Rust `_R…` or a
    // C++ `_Z…` arrives as `__R…` / `__Z…`. Strip exactly that one.
    let n = match name.strip_prefix('_') {
        Some(r) if r.starts_with("_R") || r.starts_with("_Z") => r,
        _ => name,
    };
    if let Ok(d) = rustc_demangle::try_demangle(n) {
        return format!("{d:#}");
    }
    if n.starts_with("_Z") {
        if let Ok(d) = cpp_demangle::Symbol::new(n.as_bytes()) {
            return d.to_string();
        }
    }
    name.to_string()
}

fn artifact(role: &str, path: &Path, bytes: &[u8]) -> InputArtifact {
    InputArtifact { role: role.into(), path: path.display().to_string(), sha256: sha_hex(bytes), bytes: bytes.len() as u64 }
}

fn sha_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// The machine's name, asked of the kernel. It was `hostname` found on
/// PATH, which recorded whatever program of that name came first as fact in
/// the evidence's provenance (#246).
fn hostname() -> String {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

fn common_root<'a>(paths: impl Iterator<Item = &'a str>) -> Option<PathBuf> {
    let mut root: Option<PathBuf> = None;
    for p in paths {
        let dir = Path::new(p).parent()?.to_path_buf();
        root = Some(match root {
            None => dir,
            Some(r) => {
                let mut common = PathBuf::new();
                for (a, b) in r.components().zip(dir.components()) {
                    if a == b {
                        common.push(a);
                    } else {
                        break;
                    }
                }
                common
            }
        });
    }
    root
}

fn display_path(p: &str, root: Option<&Path>) -> String {
    if let Some(r) = root {
        if let Ok(rel) = Path::new(p).strip_prefix(r) {
            let s = rel.display().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    Path::new(p).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string())
}

/// What `flags_target` in build-info.txt says the analysis build did.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeclaredMode {
    /// It carries debug information: the last `-g…` flag is not `-g0`, or
    /// rustc's `-Cdebuginfo` is above 0.
    debug: bool,
    /// `-fstack-usage`, not later undone by `-fno-stack-usage`.
    stack_usage: bool,
}

impl DeclaredMode {
    fn of(flags: &str) -> DeclaredMode {
        let mut debug = false;
        let mut stack_usage = false;
        let toks: Vec<&str> = flags.split_whitespace().collect();
        for (i, t) in toks.iter().enumerate() {
            match *t {
                "-g0" => debug = false,
                "-fstack-usage" => stack_usage = true,
                "-fno-stack-usage" => stack_usage = false,
                t if t.starts_with("-g") && !t.starts_with("-gno-") => debug = true,
                t if t.starts_with("-Cdebuginfo=") || t.starts_with("-Cdebug-info=") => debug = !t.ends_with("=0") && !t.ends_with("=none"),
                "-C" => {
                    if let Some(v) = toks.get(i + 1).and_then(|v| v.strip_prefix("debuginfo=")) {
                        debug = v != "0" && v != "none";
                    }
                }
                _ => {}
            }
        }
        DeclaredMode { debug, stack_usage }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn declared_modes_follow_the_compilers_last_word() {
        use super::DeclaredMode;
        let m = |f| DeclaredMode::of(f);
        assert_eq!(m("-g -O0 -fstack-usage"), DeclaredMode { debug: true, stack_usage: true });
        assert_eq!(m("-g -O0 -g0"), DeclaredMode { debug: false, stack_usage: false }, "a later -g0 wins");
        assert_eq!(m("-gdwarf-4 -fstack-usage -fno-stack-usage"), DeclaredMode { debug: true, stack_usage: false });
        assert_eq!(m("-gline-tables-only"), DeclaredMode { debug: true, stack_usage: false });
        assert_eq!(m("-O2 -gno-column-info"), DeclaredMode { debug: false, stack_usage: false }, "-gno-* adjusts, it does not enable");
        assert_eq!(m("-Copt-level=0 -Cdebuginfo=2"), DeclaredMode { debug: true, stack_usage: false });
        assert_eq!(m("-g -C debuginfo=0"), DeclaredMode { debug: false, stack_usage: false });
    }

    use super::*;

    /// A block of one instruction per line in `lines`, the last carrying
    /// `last`, projected from `counts` (line -> count).
    fn blk(id: usize, lines: &[u32], last: Option<InstrFlag>, succs: &[(usize, EdgeKind)], counts: &HashMap<u32, u64>) -> Block {
        let st = |l: u32| match counts.get(&l) {
            Some(0) => Status::Uncovered,
            Some(_) => Status::Covered,
            None => Status::Unknown,
        };
        let mut instrs: Vec<Instr> = lines.iter().map(|&l| Instr { line: Some(l), projected: st(l), ..Default::default() }).collect();
        if let (Some(f), Some(i)) = (last, instrs.last_mut()) {
            i.flags.push(f);
        }
        let projected = cfg::summarise(instrs.iter().map(|i| i.projected));
        Block {
            id,
            address: 0x100 + 0x10 * id as u64,
            instrs,
            succs: succs.iter().map(|&(to, kind)| Edge { to, kind, ..Default::default() }).collect(),
            lines: lines.to_vec(),
            projected,
            ..Default::default()
        }
    }

    fn entered(blocks: &mut [Block], counts: &HashMap<u32, u64>, uncovered: &[usize]) -> Vec<usize> {
        let mut owners: HashMap<u32, usize> = HashMap::new();
        for b in blocks.iter() {
            for l in &b.lines {
                *owners.entry(*l).or_default() += 1;
            }
        }
        let count = |l: u32| counts.get(&l).copied();
        let exclusive = |b: &Block| b.lines.iter().any(|l| owners.get(l) == Some(&1) && count(*l).is_some_and(|c| c > 0));
        let marked: BTreeSet<usize> = uncovered.iter().copied().collect();
        infer_entered(blocks, &marked, &exclusive, &owners, &count).into_iter().map(|(z, _)| z).collect()
    }

    #[test]
    fn a_shared_return_entered_from_a_covered_block_is_covered() {
        // examples/c-constructs goto_cleanup (#270): B0 `return rc;` (L66,
        // ran once) jumps to the epilogue B2 (L72, the closing brace, counted
        // 0 in the `undo_a:` label's region); B1 is the label code, never run
        let counts = HashMap::from([(66, 1), (70, 0), (72, 0)]);
        let mut b = vec![
            blk(0, &[66], Some(InstrFlag::Branch), &[(2, EdgeKind::Jump)], &counts),
            blk(1, &[70], Some(InstrFlag::Branch), &[(2, EdgeKind::Jump)], &counts),
            blk(2, &[72, 72], Some(InstrFlag::Return), &[], &counts),
        ];
        assert_eq!(entered(&mut b, &counts, &[]), vec![2]);
        assert_eq!(b[2].projected, Status::Covered);
        assert!(b[2].instrs.iter().all(|i| i.projected == Status::Covered));
        assert!(b[2].note.as_deref().unwrap().contains("B0 holds the only object code of L66"), "{:?}", b[2].note);
        assert_eq!(b[1].projected, Status::Uncovered, "nothing runs backwards");
        // and by falling through, and along a chain of such blocks
        let mut b = vec![
            blk(0, &[66], None, &[(1, EdgeKind::Fallthrough)], &counts),
            blk(1, &[72], Some(InstrFlag::Branch), &[(2, EdgeKind::Jump)], &counts),
            blk(2, &[70], Some(InstrFlag::Return), &[], &counts),
        ];
        assert_eq!(entered(&mut b, &counts, &[]), vec![1, 2]);
    }

    #[test]
    fn nothing_unproven_is_inferred_covered() {
        let counts = HashMap::from([(10, 3), (11, 0), (12, 0), (13, 5)]);
        let none = |mut b: Vec<Block>, uncovered: &[usize], why: &str| {
            let before: Vec<Status> = b.iter().map(|b| b.projected).collect();
            assert!(entered(&mut b, &counts, uncovered).is_empty(), "{why}");
            assert_eq!(before, b.iter().map(|b| b.projected).collect::<Vec<_>>(), "{why}");
        };
        // a conditional branch: either successor may be the one not taken
        none(
            vec![
                blk(0, &[10], Some(InstrFlag::CondBranch), &[(1, EdgeKind::Taken), (2, EdgeKind::Fallthrough)], &counts),
                blk(1, &[11], None, &[], &counts),
                blk(2, &[12], None, &[], &counts),
            ],
            &[],
            "cond branch",
        );
        // a call first: the callee need not return
        let mut call = blk(0, &[10, 10], Some(InstrFlag::Branch), &[(1, EdgeKind::Jump)], &counts);
        call.instrs[0].flags.push(InstrFlag::Call);
        none(vec![call, blk(1, &[11], None, &[], &counts)], &[], "a call in the predecessor");
        // a call inside the *entered successor*: p is covered, call-free and
        // flows only to z, but z's own call may not return, so the
        // instructions after it (L12 here) are not established by entering z.
        // The whole block must stay unpromoted (#270 review).
        let mut z_with_call = blk(1, &[11, 12], None, &[], &counts);
        z_with_call.instrs[0].flags.push(InstrFlag::Call);
        none(vec![blk(0, &[10], None, &[(1, EdgeKind::Fallthrough)], &counts), z_with_call], &[], "a call in the entered successor");
        // the predecessor's line is shared with another block: its count may
        // be the other block's
        none(
            vec![blk(0, &[10], Some(InstrFlag::Branch), &[(2, EdgeKind::Jump)], &counts), blk(1, &[10], Some(InstrFlag::Return), &[], &counts), blk(2, &[11], None, &[], &counts)],
            &[],
            "shared evidence",
        );
        // the predecessor itself only counted 0
        none(vec![blk(0, &[12], Some(InstrFlag::Branch), &[(1, EdgeKind::Jump)], &counts), blk(1, &[11], None, &[], &counts)], &[], "an uncovered predecessor");
        // branch-outcome inference marked the target uncovered: that stands
        none(vec![blk(0, &[10], Some(InstrFlag::Branch), &[(1, EdgeKind::Jump)], &counts), blk(1, &[11], None, &[], &counts)], &[1], "a conflict keeps uncovered");
        // ... or the predecessor: it is not evidence
        none(vec![blk(0, &[10], Some(InstrFlag::Branch), &[(1, EdgeKind::Jump)], &counts), blk(1, &[11], None, &[], &counts)], &[0], "a marked predecessor");
        // an indirect branch or a return ends the flow
        none(vec![blk(0, &[10], Some(InstrFlag::IndirectBranch), &[], &counts), blk(1, &[11], None, &[], &counts)], &[], "indirect");
        // a successor with no count of its own to overturn is left alone
        none(vec![blk(0, &[10], Some(InstrFlag::Branch), &[(1, EdgeKind::Jump)], &counts), blk(1, &[99], None, &[], &counts)], &[], "unknown");
    }

    fn slot<'a>(addr: u64, name: &'a str, tu: &'a str) -> SuSlot<'a> {
        SuSlot { addr, dem: name, cov_name: name, tu }
    }

    #[test]
    fn a_ctor_dtor_alias_inherits_the_same_address_body_su_record() {
        // The ELF #266 case: clang writes a .su record only for the base
        // variant, and the complete variant C1 is folded to the same address
        // with no record of its own. Both demangle alike, so C1 finds no
        // record by name; the address carries the base variant's frame.
        let su = su_by_name(stack::parse_su_records("chan.cc:12:3:_ZN2fc7ChannelC2Ei\t48\tstatic\n"));
        let dem = "fc::Channel::Channel(int)";
        let slots = [
            SuSlot { addr: 0x9e8, dem, cov_name: "_ZN2fc7ChannelC2Ei", tu: "/w/chan.cc" },
            SuSlot { addr: 0x9e8, dem, cov_name: "_ZN2fc7ChannelC1Ei", tu: "/w/chan.cc" },
            SuSlot { addr: 0x2000, dem: "fc::Other::gone()", cov_name: "_ZN2fc5Other4goneEv", tu: "/w/chan.cc" },
        ];
        let placed = place_su(&su, &slots);
        assert_eq!(placed.get(&0x9e8), Some(&(48, false)), "one address, one record: both variants have it");
        // a genuinely unrecorded function at its own address is still missing
        assert_eq!(placed.get(&0x2000), None);
    }

    /// Two differently-named functions the linker folded onto one address
    /// (ICF) are one body: the address keeps the worst of their frames,
    /// whichever name is placed last.
    #[test]
    fn functions_folded_onto_one_address_keep_the_worst_frame() {
        let slots = [slot(0x70, "a", "/w/a.c"), slot(0x70, "b", "/w/b.c")];
        for (text, want) in [
            ("a.c:1:a\t16\tstatic\nb.c:1:b\t32\tdynamic\n", (32, true)),
            ("a.c:1:a\t32\tdynamic\nb.c:1:b\t16\tstatic\n", (32, true)),
            ("a.c:1:a\t48\tstatic\nb.c:1:b\t16\tdynamic\n", (48, true)),
        ] {
            assert_eq!(place_su(&su_by_name(stack::parse_su_records(text)), &slots).get(&0x70), Some(&want), "{text:?}");
        }
    }

    /// #10: aux.c's static `scale` and scale.c's external `scale` share a
    /// name. With scale.c's record gone, aux.c's must not answer for it.
    #[test]
    fn a_same_named_function_in_another_unit_does_not_supply_a_missing_su_record() {
        let slots = [slot(0x10, "scale", "/w/src/aux.c"), slot(0x20, "scale", "/w/src/scale.c")];
        let both = su_by_name(stack::parse_su_records("src/aux.c:9:scale\t16\tstatic\nsrc/scale.c:7:scale\t24\tdynamic\n"));
        let placed = place_su(&both, &slots);
        assert_eq!((placed.get(&0x10), placed.get(&0x20)), (Some(&(16, false)), Some(&(24, true))), "each unit gets its own record");

        let aux_only = su_by_name(stack::parse_su_records("src/aux.c:9:scale\t16\tstatic\n"));
        let placed = place_su(&aux_only, &slots);
        assert_eq!(placed.get(&0x10), Some(&(16, false)));
        assert_eq!(placed.get(&0x20), None, "aux.c's record is not scale.c's");

        // a symbol tied to no function keeps the name-only worst case
        assert_eq!(su_unplaced(&both, "scale", "scale"), Some((24, true)));
        // a record with no file still serves the one function of its name
        let unplaced = su_by_name(stack::parse_su_records("scale\t8\tstatic\n"));
        assert_eq!(place_su(&unplaced, &slots[1..]).get(&0x20), Some(&(8, false)));
        assert_eq!(place_su(&unplaced, &slots).get(&0x20), None, "but not one of two");
    }

    /// #11 review: a record names only the file that defines the function.
    /// GCC writes the path it was given, so a unit compiled from its own
    /// directory names a bare `dup.c`, which matches two units.
    #[test]
    fn an_ambiguous_su_record_satisfies_neither_unit() {
        let slots = [slot(0x30, "tally", "/w/src/alpha/dup.c"), slot(0x40, "tally", "/w/src/beta/dup.c")];
        let bare = su_by_name(stack::parse_su_records("dup.c:8:1:tally\t16\tstatic\n"));
        assert!(place_su(&bare, &slots).is_empty(), "one bare record cannot cover either unit");
        let bare_both = su_by_name(stack::parse_su_records("dup.c:8:1:tally\t16\tstatic\ndup.c:6:1:tally\t16\tstatic\n"));
        assert!(place_su(&bare_both, &slots).is_empty(), "nor two that cannot be told apart");
        // a qualified record still picks out its own unit
        let alpha = su_by_name(stack::parse_su_records("src/alpha/dup.c:8:tally\t16\tstatic\n"));
        let placed = place_su(&alpha, &slots);
        assert_eq!((placed.get(&0x30), placed.get(&0x40)), (Some(&(16, false)), None));
        // and with one function of the name, a bare record is unambiguous
        assert_eq!(place_su(&bare, &slots[..1]).get(&0x30), Some(&(16, false)));
        // a copy tied to no coverage function stands for its DWARF unit, so
        // the bare record is ambiguous against it too
        let with_unplaced = [slot(0x30, "tally", "/w/src/alpha/dup.c"), slot(0x40, "tally", "src/beta/dup.c")];
        assert!(place_su(&bare_both, &with_unplaced).is_empty());
    }

    /// #11 review: a header's static inline is one defining file for every
    /// unit that emits a copy. Each copy needs its own unit's record, so a
    /// unit whose .su holds only header functions is still missed.
    #[test]
    fn each_copy_of_a_header_function_needs_its_own_record() {
        let slots = [slot(0x50, "geom_clamp", "/w/inc/geom.h"), slot(0x60, "geom_clamp", "/w/inc/geom.h")];
        let two = su_by_name(stack::parse_su_records("inc/geom.h:15:geom_clamp\t16\tstatic\ninc/geom.h:15:geom_clamp\t32\tstatic\n"));
        let placed = place_su(&two, &slots);
        assert_eq!((placed.get(&0x50), placed.get(&0x60)), (Some(&(32, false)), Some(&(32, false))), "both present: the worst of them");
        let one = su_by_name(stack::parse_su_records("inc/geom.h:15:geom_clamp\t16\tstatic\n"));
        assert!(place_su(&one, &slots).is_empty(), "two copies, one record: a unit's record is missing");
        // a C++ inline the linker folded to one copy keeps every unit's record
        assert_eq!(place_su(&two, &slots[..1]).get(&0x50), Some(&(32, false)));
    }

    #[test]
    fn same_tu_matches_a_relative_dotdot_path_against_an_absolute_one() {
        // The #170 case: make compiles `../../c-lib/src/util.c`, so the DWARF
        // unit name keeps the `..` while the coverage path is absolute.
        assert!(same_tu("/Users/x/proj/examples/c-lib/src/util.c", "../../c-lib/src/util.c"));
        assert!(same_tu("../../c-lib/src/sched.c", "/Users/x/proj/examples/c-lib/src/sched.c"));
        // `.` and doubled slashes are also normalised.
        assert!(same_tu("./src/util.c", "/work/src/util.c"));
    }

    #[test]
    fn same_tu_still_tells_distinct_units_apart() {
        // The property the suffix comparison exists to keep (review): a
        // relative path matches a longer absolute one only on a true suffix.
        assert!(same_tu("src/foo.c", "/work/src/foo.c"));
        assert!(!same_tu("tests/foo.c", "/work/src/foo.c"));
        // Two same-named statics in sibling directories stay distinct even
        // when one side is written with `..`.
        assert!(!same_tu("../alpha/dup.c", "/work/src/beta/dup.c"));
        assert!(same_tu("../alpha/dup.c", "/work/src/alpha/dup.c"));
    }

    // Symbol ids in these tests: 7 = op_negate @0x1000, 8 = op_double
    // @0x1100, both held in the table `OPS`. `apply_op` reads `OPS` and takes
    // neither address directly, which is the shape in examples/c.
    fn ops_table() -> Vec<String> {
        vec!["OPS".to_string()]
    }

    /// The regression test for #215.
    ///
    /// The bug was that `tables_by_name`'s input arrives in `HashMap`
    /// iteration order, so `tables["OPS"]` came out `[7, 8]` in some
    /// processes and `[8, 7]` in others, and the candidate list was stored
    /// in whichever order it got. Running the whole ingest twice does not
    /// reliably show that -- the two orders are roughly equally likely and
    /// the reporter saw 5 of 6 runs agree -- so this forces both orders
    /// instead of hoping for them: it feeds the entries in ascending and
    /// descending key order and requires the same answer from each.
    ///
    /// Without the sort in `candidate_ids` this fails every time, on the
    /// descending case, whatever the hash seed.
    #[test]
    fn candidate_order_does_not_depend_on_map_iteration_order() {
        let names = ops_table();
        let entries = [(7usize, names.as_slice()), (8usize, names.as_slice())];
        let addr = |t: usize| if t == 7 { 0x1000 } else { 0x1100 };

        let ascending = tables_by_name(entries.iter().copied());
        let descending = tables_by_name(entries.iter().rev().copied());
        // the two orders really are different at the point that matters,
        // or this test would be asserting nothing
        assert_eq!(ascending["OPS"], vec![7, 8]);
        assert_eq!(descending["OPS"], vec![8, 7]);

        let a = candidate_ids(&[], std::iter::once("OPS"), &ascending, addr);
        let d = candidate_ids(&[], std::iter::once("OPS"), &descending, addr);
        assert_eq!(a, d, "candidate order must not depend on map iteration order");
        assert_eq!(a, vec![7, 8], "and must be by address");
    }

    /// The same property stated the way the report did, over two real
    /// `HashMap`s filled in opposite insertion orders. This is the weaker
    /// of the two: `HashMap` iteration order is the hash seed's, not the
    /// insertion order's, so both maps may well iterate the same way and
    /// the test can pass without exercising anything. It is kept because it
    /// is the shape of the original reproduction; the test above is the one
    /// that actually holds the fix in place.
    #[test]
    fn candidate_order_is_the_same_from_two_independently_built_maps() {
        let names = ops_table();
        let addr = |t: usize| if t == 7 { 0x1000 } else { 0x1100 };
        let mut forward: HashMap<usize, Vec<String>> = HashMap::new();
        forward.insert(7, names.clone());
        forward.insert(8, names.clone());
        let mut backward: HashMap<usize, Vec<String>> = HashMap::new();
        backward.insert(8, names.clone());
        backward.insert(7, names.clone());

        let one = tables_by_name(forward.iter().map(|(t, n)| (*t, n.as_slice())));
        let two = tables_by_name(backward.iter().map(|(t, n)| (*t, n.as_slice())));
        assert_eq!(candidate_ids(&[], std::iter::once("OPS"), &one, addr), candidate_ids(&[], std::iter::once("OPS"), &two, addr));
    }

    /// Candidates reached through the `takes` list -- the symbols a function
    /// names directly -- are ordered by address too, and merge with the
    /// table's without duplicating.
    #[test]
    fn takes_and_table_candidates_merge_in_address_order() {
        let names = ops_table();
        let tables = tables_by_name([(8usize, names.as_slice())].into_iter());
        let addr = |t: usize| match t {
            7 => 0x1000,
            8 => 0x1100,
            _ => 0x900,
        };
        // 9 sits below both and is named second, 8 is named directly *and*
        // held in the table: one entry each, in address order rather than
        // the order they were reached in
        let got = candidate_ids(&[8, 9], std::iter::once("OPS"), &tables, addr);
        assert_eq!(got, vec![9, 8]);
    }

    /// A table nothing reads contributes nothing, and a read of a global
    /// that is not a table is not an error.
    #[test]
    fn a_global_that_is_not_a_table_contributes_no_candidates() {
        let tables = tables_by_name([(8usize, ops_table().as_slice())].into_iter());
        assert!(candidate_ids(&[], std::iter::once("LIMITS"), &tables, |_| 0).is_empty());
    }

    /// The same property on the measured path (#215's second instance).
    ///
    /// One call site in function 3, two targets reached from the trace.
    /// The transitions live in a `HashMap`, so they arrive in hash-seed
    /// order; feeding both orders in turn and requiring the same answer is
    /// what running an ingest twice cannot promise. Without the sort in
    /// `resolved_calls` this fails every run, on the reversed case.
    #[test]
    fn measured_call_order_does_not_depend_on_map_iteration_order() {
        // TB 0x100 ends with the indirect call at 0x108, which is site 0 of
        // function 3; symbol 5 @0x400 and symbol 6 @0x300 are its targets.
        let tb_last: HashMap<u64, u64> = [(0x100, 0x108)].into();
        let site_of: HashMap<u64, (usize, usize)> = [(0x108, (3usize, 0usize))].into();
        let sym_by_entry: HashMap<u64, usize> = [(0x400, 5usize), (0x300, 6usize)].into();
        let addr = |t: usize| if t == 5 { 0x400 } else { 0x300 };
        let entries: [((u64, u64), u64); 2] = [((0x100, 0x400), 2), ((0x100, 0x300), 7)];

        let one = resolved_calls(entries.iter().map(|(k, n)| (k, n)), &tb_last, &site_of, &sym_by_entry, addr);
        let other = resolved_calls(entries.iter().rev().map(|(k, n)| (k, n)), &tb_last, &site_of, &sym_by_entry, addr);
        assert_eq!(one, other, "measured target order must not depend on map iteration order");
        // symbol 6 is the lower address, so it comes first whichever order
        // the transitions arrived in; the counts ride along untouched
        assert_eq!(one, vec![(3, 0, 6, 7), (3, 0, 5, 2)]);
    }

    /// A transition whose source block does not end in an indirect call,
    /// or whose target is not a symbol entry, is not a measured call.
    #[test]
    fn a_transition_that_is_not_a_call_through_a_site_resolves_to_nothing() {
        let tb_last: HashMap<u64, u64> = [(0x100, 0x108)].into();
        let site_of: HashMap<u64, (usize, usize)> = [(0x108, (3usize, 0usize))].into();
        let sym_by_entry: HashMap<u64, usize> = [(0x400, 5usize)].into();
        // target 0x404 is mid-symbol, not an entry; source 0x200 is a block
        // with no recorded last instruction
        let entries: [((u64, u64), u64); 2] = [((0x100, 0x404), 1), ((0x200, 0x400), 1)];
        assert!(resolved_calls(entries.iter().map(|(k, n)| (k, n)), &tb_last, &site_of, &sym_by_entry, |_| 0).is_empty());
    }
}

#[cfg(test)]
#[path = "req_e_lib_tests.rs"]
mod req_e_tests;
