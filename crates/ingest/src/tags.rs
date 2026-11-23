// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Produce a source definition index from Universal Ctags.
//!
//! Optional and self-contained: it runs an external `ctags`, and if there is
//! no usable one, `produce` says so and nothing else is affected. It reads the
//! source set from the evidence, so the index describes exactly the files the
//! evidence covers, plus the headers those files pull in.

use anyhow::{bail, Context, Result};
use butterfly_model::tags::{Tag, Tags, TAGS_SCHEMA};
use butterfly_model::Evidence;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Find a Universal Ctags and confirm it is one -- BSD/Exuberant ctags do not
/// speak the JSON we parse. Order: an explicit path, then `BUTTERFLY_CTAGS`,
/// then `ctags` on `PATH`.
pub fn find_ctags(explicit: Option<&Path>) -> Option<PathBuf> {
    find_ctags_in(explicit, std::env::var_os("BUTTERFLY_CTAGS").as_deref(), std::env::var_os("PATH").as_deref())
}

/// [`find_ctags`] with its inputs explicit, so it can be tested.
///
/// An explicit choice -- the flag, or the variable when there is no flag --
/// is honoured or refused, never quietly swapped for a different binary on
/// PATH: a wrong choice should surface, not hide. The variable used to be one
/// candidate ahead of `ctags` on PATH, so a `BUTTERFLY_CTAGS` that was not
/// Universal Ctags fell through to whatever PATH held (#246). Every answer
/// is an absolute path, because that is what runs and what is recorded.
fn find_ctags_in(explicit: Option<&Path>, env: Option<&std::ffi::OsStr>, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let chosen = explicit.map(Path::to_path_buf).or_else(|| env.filter(|e| !e.is_empty()).map(PathBuf::from));
    let bin = match chosen {
        Some(p) => std::path::absolute(&p).unwrap_or(p),
        None => crate::provenance::resolve_in(path_var?, "ctags")?,
    };
    is_universal(&bin).then_some(bin)
}

/// The first line of a candidate's `--version`, or `None`.
///
/// The candidate is whatever the operator or PATH named, so the probe is
/// untrusted too: it runs as a bounded helper probe, which answers at once
/// or not at all (review of #32, #246).
fn version_line(bin: &Path) -> Option<String> {
    let out = crate::helper::run(bin, &[std::ffi::OsStr::new("--version")], None, crate::helper::Bounds::probe()).ok()?;
    Some(String::from_utf8_lossy(&out).lines().next().unwrap_or_default().to_string())
}

/// Is this a Universal Ctags? BSD and Exuberant ctags do not speak the JSON we
/// parse, and both would otherwise pass a bare existence check.
fn is_universal(bin: &Path) -> bool {
    version_line(bin).map(|v| v.contains("Universal Ctags")).unwrap_or(false)
}

/// One raw ctags JSON record, only the fields we use.
#[derive(serde::Deserialize)]
struct Raw {
    name: String,
    path: String,
    line: u32,
    kind: Option<String>,
    #[serde(default)]
    end: Option<u32>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(rename = "_type")]
    ty: Option<String>,
}

/// Build the definition index for `ev`, whose file was read from `evidence_path`.
///
/// `source_root` bounds where sources are looked for, exactly as ingest does;
/// `ctags` is the binary from [`find_ctags`].
/// How long `ctags` may run, and how much it may say.
///
/// `produce` points a source parser at a tree butterfly does not control, so
/// the parser is untrusted too (review of #32). It runs as a helper (#246)
/// with its own, tighter ceiling: its whole output is optional.
const CTAGS_MAX_SECS: u64 = 300;
const CTAGS_MAX_STDOUT: u64 = 256 * 1024 * 1024;

pub fn produce(ev: &Evidence, evidence_path: &Path, source_root: &Path, ctags: &Path) -> Result<Outcome> {
    // The filenames in an evidence document are untrusted -- the same input
    // that made arbitrary local files readable in #1. ctags would parse
    // whatever they name and put its symbol names in the index, so every
    // candidate is resolved and required to be inside the source root before
    // it is handed to ctags. One that is not is skipped and counted, never
    // read.
    let root = source_root.canonicalize().with_context(|| format!("resolving the source root {}", source_root.display()))?;

    let mut scan: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut outside: Vec<String> = Vec::new();
    {
        let mut consider = |p: PathBuf, display: Option<&str>| {
            use std::os::unix::ffi::OsStrExt;
            // Open once and ask the kernel where the descriptor really points,
            // refusing anything outside `root` or hard-linked (#248).
            let Ok(placed) = crate::fsauth::open_placed(&root, &p) else { return };
            if placed.standing != crate::fsauth::Standing::Inside {
                outside.push(display.map(str::to_string).unwrap_or_else(|| placed.real.display().to_string()));
                return;
            }
            if !placed.meta.is_file() {
                return;
            }
            // `ctags -L -` splits paths on newlines; a filename containing
            // `\n`, `\r`, or `\0` would inject arbitrary paths into `-L -`.
            if placed.real.as_os_str().as_bytes().iter().any(|&b| matches!(b, b'\n' | b'\r' | 0)) {
                outside.push(display.map(str::to_string).unwrap_or_else(|| placed.real.display().to_string()));
                return;
            }
            if seen.insert(placed.real.clone()) {
                scan.push(placed.real);
            }
        };
        for f in &ev.files {
            consider(PathBuf::from(&f.path), Some(&f.display));
        }
        collect_headers(&root, 0, &mut |p| consider(p, None));
    }
    outside.sort();
    outside.dedup();
    if scan.is_empty() {
        bail!(
            "no source files inside {} to index{}",
            root.display(),
            if outside.is_empty() { String::new() } else { format!(" ({} named by the evidence resolve outside it)", outside.len()) }
        );
    }

    // ctags, JSON, with the fields we map. `--options=NONE` prevents loading
    // `.ctags.d/*.ctags` from an untrusted working directory or home.
    // `--fields=+neKSZ`: end line, kind (long name), signature, scope. Feed
    // the file list on stdin so a huge argv is not a problem.
    let mut list = Vec::new();
    for p in &scan {
        use std::os::unix::ffi::OsStrExt;
        list.extend_from_slice(p.as_os_str().as_bytes());
        list.push(b'\n');
    }
    let args: Vec<&std::ffi::OsStr> = ["--options=NONE", "--output-format=json", "--fields=+neKSZ", "--extras=+q", "-L", "-", "-f", "-"].iter().map(std::ffi::OsStr::new).collect();
    let bounds = crate::helper::Bounds::standard().with_deadline(std::time::Duration::from_secs(CTAGS_MAX_SECS)).with_stdout(CTAGS_MAX_STDOUT);
    let stdout = match crate::helper::run(ctags, &args, Some(list), bounds) {
        Ok(out) => out,
        Err(crate::helper::HelperError::Failed { stderr, .. }) => bail!("ctags failed: {}", stderr.trim()),
        Err(e) => bail!("{e} The definition index is optional: re-run `butterfly tags` over a smaller --source-root, or leave it out."),
    };

    // evidence path (absolute) -> file_id, and function name -> id for the
    // analysed-function link. A name maps to a function only when it is
    // unambiguous; a name defined in two files is left unlinked rather than
    // linked to the wrong one.
    let file_id: HashMap<PathBuf, usize> = ev.files.iter().enumerate().filter_map(|(i, f)| PathBuf::from(&f.path).canonicalize().ok().map(|c| (c, i))).collect();
    // Keyed by (file, name), not name alone. Two translation units can each
    // have a `static helper`, and linking by name would attach both tags to
    // whichever one the evidence analysed -- sending the viewer to the wrong
    // function's control-flow graph.
    //
    // The bare name is not enough on its own, though. An inline function
    // defined in a header is compiled into *every* translation unit that
    // includes it, so the evidence carries one analysed function per unit --
    // `geom.c:geom_clamp` and `main.c:geom_clamp`, both pointing at the same
    // header line. ctags sees one definition. Matching the qualified form as
    // well finds them, and finding more than one is the answer: there is no
    // single control-flow graph for that definition, so it is left unlinked
    // and the viewer sends the reader to the source line instead. Ambiguity
    // within one file (a C++ overload set) resolves the same way.
    let mut fn_by_place: HashMap<(usize, &str), Option<usize>> = HashMap::new();
    for f in &ev.functions {
        // `sched.c:task_climb` is butterfly's own disambiguation; index the
        // bare name too so a ctags tag can find it
        let bare = f.name.rsplit(':').next().unwrap_or(f.name.as_str());
        // Deduplicated: an unprefixed name is its own bare form, and
        // inserting it twice would mark it ambiguous against itself.
        let mut keys = vec![f.name.as_str()];
        if bare != f.name {
            keys.push(bare);
        }
        for key in keys {
            fn_by_place.entry((f.file_id, key)).and_modify(|e| *e = None).or_insert(Some(f.id));
        }
    }
    let display_of: HashMap<PathBuf, String> = ev.files.iter().filter_map(|f| PathBuf::from(&f.path).canonicalize().ok().map(|c| (c, f.display.clone()))).collect();

    let mut entries = Vec::new();
    for line in String::from_utf8_lossy(&stdout).lines() {
        let r: Raw = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(_) => continue, // ctags prints a leading non-tag line or two
        };
        if r.ty.as_deref() != Some("tag") {
            continue;
        }
        let abs = PathBuf::from(&r.path).canonicalize().unwrap_or_else(|_| PathBuf::from(&r.path));
        let fid = file_id.get(&abs).copied();
        let kind = r.kind.unwrap_or_default();
        // a function tag links to the analysed function only when the evidence
        // has exactly one of that name -- so the viewer can route it to the
        // control-flow view; a header-only inline or a bare type stays a
        // source-line jump.
        let function_id = match (kind.as_str(), fid) {
            ("function", Some(f)) => fn_by_place.get(&(f, r.name.as_str())).copied().flatten(),
            // a function in a file the evidence does not carry cannot be one
            // of its analysed functions
            _ => None,
        };
        let file = fid.and_then(|i| ev.files.get(i)).map(|f| f.display.clone()).or_else(|| display_of.get(&abs).cloned()).unwrap_or_else(|| display_name(&abs, Some(&root)));
        entries.push(Tag { name: r.name, kind, language: r.language.unwrap_or_default(), file, line: r.line, end: r.end, file_id: fid, function_id, scope: r.scope });
    }
    entries.sort_by(|a, b| (a.file.as_str(), a.line, a.name.as_str()).cmp(&(b.file.as_str(), b.line, b.name.as_str())));

    let generator = version_line(ctags).unwrap_or_else(|| "Universal Ctags".to_string());
    // Bounded like every other evidence read: `produce` is a library entry
    // point, so it cannot lean on the CLI having sized the file first, and
    // an evidence whose hash cannot be taken under the cap is one whose hash
    // is simply not recorded -- as it already was for any unreadable file
    // (#231).
    let lim = butterfly_model::limits::Limits::from_env();
    let evidence_sha256 = butterfly_model::limits::read_bounded(evidence_path, butterfly_model::limits::Kind::Evidence, &lim).ok().map(|b| sha_hex(&b)).unwrap_or_default();
    Ok(Outcome { tags: Tags { schema: TAGS_SCHEMA.into(), generator, evidence_sha256, entries }, outside })
}

/// The index, and the evidence-named files that were refused for being
/// outside the source root -- reported so a surprising result is visible
/// rather than silent.
pub struct Outcome {
    pub tags: Tags,
    pub outside: Vec<String>,
}

/// Every `.h`/`.hpp`/`.hh`/`.hxx` under the source root, recursively.
///
/// Directory *symlinks* are not followed: `is_dir()` follows them, so a link
/// pointing at an ancestor is an infinite descent, and one pointing outside
/// the root would walk out of the very boundary the caller enforces on each
/// candidate.
/// Depth is bounded too, so an unusually deep tree cannot exhaust the stack.
fn collect_headers(dir: &Path, depth: u32, push: &mut impl FnMut(PathBuf)) {
    const MAX_DEPTH: u32 = 64;
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
        if md.file_type().is_symlink() {
            continue;
        }
        if md.is_dir() {
            collect_headers(&p, depth + 1, push);
        } else if md.is_file() && matches!(p.extension().and_then(|x| x.to_str()), Some("h" | "hpp" | "hh" | "hxx" | "H")) {
            push(p);
        }
    }
}

fn display_name(abs: &Path, root: Option<&Path>) -> String {
    root.and_then(|r| abs.strip_prefix(r).ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| abs.display().to_string()))
}

fn sha_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in that answers `--version` the way Universal Ctags does.
    fn fake_ctags(tag: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let d = butterfly_testsupport::scratch_dir(&format!("ctags-{tag}"));
        let p = d.join("ctags");
        std::fs::write(&p, "#!/bin/sh\necho 'Universal Ctags 6.1.0'\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// `BUTTERFLY_CTAGS` is an explicit choice, honoured or refused. It used
    /// to fall through to `ctags` on PATH when it was not Universal Ctags, so
    /// the index could be built by a binary nobody chose (#246).
    #[test]
    fn a_configured_ctags_that_is_not_universal_is_refused_not_replaced() {
        let good = fake_ctags("path");
        let path = std::env::join_paths([good.parent().unwrap()]).unwrap();
        assert_eq!(find_ctags_in(None, None, Some(&path)).as_deref(), Some(good.as_path()), "the control: PATH finds it");
        assert_eq!(find_ctags_in(None, Some(std::ffi::OsStr::new("/usr/bin/true")), Some(&path)), None, "the variable is not swapped for PATH's");
        assert_eq!(find_ctags_in(Some(Path::new("/usr/bin/true")), None, Some(&path)), None, "nor is the flag");
    }

    #[test]
    fn the_ctags_that_is_chosen_is_recorded_absolute() {
        let good = fake_ctags("abs");
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        let rel: PathBuf = cwd.components().skip(1).map(|_| "..").collect::<PathBuf>().join(good.strip_prefix("/").unwrap());
        let got = find_ctags_in(Some(&rel), None, None).expect("it is Universal Ctags");
        assert!(got.is_absolute(), "{got:?}");
    }
}
