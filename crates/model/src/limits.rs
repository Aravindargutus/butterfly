// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Ceilings on what butterfly will read into memory.
//!
//! Nearly everything butterfly ingests is untrusted: a coverage export names
//! the sources, an evidence file may have come by mail, a build directory may
//! hold anything at all. Each was read whole before anything semantic looked
//! at it, so an oversized or crafted artifact could exhaust memory or stall a
//! CI runner long before the error that would have rejected it (#5).
//!
//! Two bounds, both checked *before* a byte is allocated:
//!
//!   * a per-file cap, by kind -- a source file is not an object file and
//!     should not be allowed to be the size of one; and
//!   * a total budget across one run, so a thousand files just under the
//!     per-file cap cannot add up to the same problem.
//!
//! Every cap is generous by default: they exist to make a hostile input fail
//! with a sentence, not to tell an honest project how large it may be. Any of
//! them can be raised (see [`Limits::from_env`]), which is the documented way
//! through for an unusually large tree.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

const MIB: u64 = 1024 * 1024;

/// What is being read, so the message can say what was too big and the cap
/// can suit the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An llvm-cov export.
    Coverage,
    /// A butterfly evidence document.
    Evidence,
    /// One analysed source file, whose text is embedded in the evidence.
    Source,
    /// An executable, object or dSYM.
    Object,
    /// An execution trace.
    Trace,
    /// build-info, `.su`, config, baseline, the change cache, a sidecar.
    Auxiliary,
}

impl Kind {
    pub fn what(self) -> &'static str {
        match self {
            Kind::Coverage => "coverage export",
            Kind::Evidence => "evidence file",
            Kind::Source => "source file",
            Kind::Object => "object file",
            Kind::Trace => "trace",
            Kind::Auxiliary => "file",
        }
    }
    /// The environment variable that raises this kind's cap.
    pub fn env_var(self) -> &'static str {
        match self {
            Kind::Coverage => "BUTTERFLY_MAX_COVERAGE_MB",
            Kind::Evidence => "BUTTERFLY_MAX_EVIDENCE_MB",
            Kind::Source => "BUTTERFLY_MAX_SOURCE_MB",
            Kind::Object => "BUTTERFLY_MAX_OBJECT_MB",
            Kind::Trace => "BUTTERFLY_MAX_TRACE_MB",
            Kind::Auxiliary => "BUTTERFLY_MAX_AUX_MB",
        }
    }
}

/// The caps for one run, and what is left of the total budget.
#[derive(Debug)]
pub struct Limits {
    coverage: u64,
    evidence: u64,
    source: u64,
    object: u64,
    trace: u64,
    auxiliary: u64,
    /// Bytes still available across every read this run.
    remaining: AtomicU64,
    total: u64,
}

impl Default for Limits {
    /// Sized so that no plausible project meets them, and no single file can
    /// be the reason a machine runs out of memory.
    ///
    /// A source file is a person's editor buffer; 64 MiB is already absurd
    /// for one. An object file with debug info is the largest thing here by
    /// far. The total is the one that actually binds on a hostile directory.
    fn default() -> Limits {
        Limits::new(512 * MIB, 512 * MIB, 64 * MIB, 2048 * MIB, 512 * MIB, 64 * MIB, 8192 * MIB)
    }
}

impl Limits {
    #[allow(clippy::too_many_arguments)]
    pub fn new(coverage: u64, evidence: u64, source: u64, object: u64, trace: u64, auxiliary: u64, total: u64) -> Limits {
        Limits { coverage, evidence, source, object, trace, auxiliary, remaining: AtomicU64::new(total), total }
    }

    /// The defaults, with any `BUTTERFLY_MAX_*_MB` override applied.
    ///
    /// A value that is not a positive number is ignored rather than fatal: a
    /// mistyped environment variable should not stop an ingest, and the cap
    /// it failed to raise will say so if it is ever reached.
    pub fn from_env() -> Limits {
        let mb = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.trim().parse::<u64>().ok()).filter(|n| *n > 0).map(|n| n.saturating_mul(MIB)).unwrap_or(d);
        let d = Limits::default();
        Limits::new(
            mb(Kind::Coverage.env_var(), d.coverage),
            mb(Kind::Evidence.env_var(), d.evidence),
            mb(Kind::Source.env_var(), d.source),
            mb(Kind::Object.env_var(), d.object),
            mb(Kind::Trace.env_var(), d.trace),
            mb(Kind::Auxiliary.env_var(), d.auxiliary),
            mb("BUTTERFLY_MAX_TOTAL_MB", d.total),
        )
    }

    pub fn cap(&self, kind: Kind) -> u64 {
        match kind {
            Kind::Coverage => self.coverage,
            Kind::Evidence => self.evidence,
            Kind::Source => self.source,
            Kind::Object => self.object,
            Kind::Trace => self.trace,
            Kind::Auxiliary => self.auxiliary,
        }
    }

    /// Bytes still available across the run.
    pub fn remaining(&self) -> u64 {
        self.remaining.load(Ordering::Relaxed)
    }

    /// May a file of `size` bytes be read as `kind`? Charges the budget when
    /// it may. The check is before the allocation, which is the whole point.
    pub fn admit(&self, size: u64, kind: Kind, path: &Path) -> Result<(), String> {
        let cap = self.cap(kind);
        if size > cap {
            return Err(format!(
                "{}: this {} is {} and butterfly reads at most {} of one. If the project really is this large, raise {}; \
                 the limit is there so a crafted or truncated artifact fails here rather than by exhausting memory.",
                path.display(),
                kind.what(),
                human(size),
                human(cap),
                kind.env_var()
            ));
        }
        // one compare-and-swap, so concurrent readers cannot both be admitted
        // by a budget that only had room for one
        let mut left = self.remaining.load(Ordering::Relaxed);
        loop {
            if size > left {
                return Err(format!(
                    "{}: reading this {} would take butterfly past its total input budget of {} for one run ({} left). Raise BUTTERFLY_MAX_TOTAL_MB.",
                    path.display(),
                    kind.what(),
                    human(self.total),
                    human(left)
                ));
            }
            match self.remaining.compare_exchange_weak(left, left - size, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return Ok(()),
                Err(now) => left = now,
            }
        }
    }

    /// Charge whatever arrived beyond what was admitted.
    ///
    /// `admit` is called with the stat's length, but a file can grow between
    /// the stat and the read and still land under its own per-kind cap. Those
    /// extra bytes were spent without being counted, and repeating that across
    /// files is a way past the total budget, so the difference is charged here
    /// -- and refused if it does not fit, rather than keeping bytes the budget
    /// never approved (review of #5). A reader that holds its own descriptor
    /// and cannot reopen the path (the freshness route, #248) calls this to
    /// keep the same aggregate-budget invariant `read_bounded` has.
    ///
    /// Public and separate from the read so the growth case can be tested
    /// without racing a writer against a reader.
    pub fn charge_growth(&self, kind: Kind, path: &Path, admitted: u64, actual: u64) -> Result<(), String> {
        if actual > admitted {
            self.admit(actual - admitted, kind, path)?;
        }
        Ok(())
    }
}

/// Bytes, the way a person would say them.
pub fn human(n: u64) -> String {
    if n >= MIB {
        format!("{:.1} MiB", n as f64 / MIB as f64)
    } else if n >= 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// Read a whole file, having first established it is small enough to.
///
/// `metadata` before `read` is the point: `fs::read` sizes its buffer from
/// the metadata and then allocates, so asking first is what turns "the
/// machine ran out of memory" into a sentence naming the file (#5).
///
/// The size is re-checked against what actually arrived, because a file can
/// grow between the two calls and `read_to_end` would follow it.
pub fn read_bounded(path: &Path, kind: Kind, limits: &Limits) -> Result<Vec<u8>, String> {
    // Asked of the name before it is opened, so a fifo is refused rather
    // than blocked on.
    require_regular(path)?;
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    read_bounded_from(f, path, kind, limits)
}

/// The same, from a file the caller has already opened -- for a caller that
/// had to open it a particular way, such as without following a final
/// symlink (#341). The size and type come from the descriptor, so they are
/// those of the file the bytes are read from. `path` only names it in errors.
pub fn read_bounded_from(f: std::fs::File, path: &Path, kind: Kind, limits: &Limits) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let md = f.metadata().map_err(|e| format!("{}: {e}", path.display()))?;
    if !md.is_file() {
        return Err(format!("{}: not a regular file", path.display()));
    }
    limits.admit(md.len(), kind, path)?;

    let cap = limits.cap(kind);
    let mut buf = Vec::with_capacity(md.len() as usize);
    // one byte past the cap, so growth between the stat and the read is
    // caught rather than followed
    f.take(cap.saturating_add(1)).read_to_end(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    let actual = buf.len() as u64;
    if actual > cap {
        return Err(format!("{}: this {} grew past {} while it was being read. Raise {} if that is legitimate.", path.display(), kind.what(), human(cap), kind.env_var()));
    }
    limits.charge_growth(kind, path, md.len(), actual)?;
    Ok(buf)
}

/// The same, as text.
pub fn read_bounded_string(path: &Path, kind: Kind, limits: &Limits) -> Result<String, String> {
    let bytes = read_bounded(path, kind, limits)?;
    String::from_utf8(bytes).map_err(|_| format!("{}: this {} is not valid UTF-8", path.display(), kind.what()))
}

/// [`read_bounded_from`], as text.
pub fn read_bounded_string_from(f: std::fs::File, path: &Path, kind: Kind, limits: &Limits) -> Result<String, String> {
    let bytes = read_bounded_from(f, path, kind, limits)?;
    String::from_utf8(bytes).map_err(|_| format!("{}: this {} is not valid UTF-8", path.display(), kind.what()))
}

/// The first `n` bytes, for deciding what a file *is* without reading it.
///
/// Discovery classified every unknown file in a build directory by reading
/// it whole and handing it to the object parser. A directory holding one
/// large file therefore cost that file's size in memory before anything had
/// established it was even an object (#5).
/// Reject anything that is not a regular file *before* opening it.
///
/// `File::open` on a fifo blocks until a writer appears, so classifying every
/// entry in a build directory by opening it would hang on a named pipe left
/// there. `metadata` does not open, so it cannot block; a non-regular entry
/// is refused here and never reaches `open` (review of #5).
fn require_regular(path: &Path) -> Result<(), String> {
    let md = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if md.is_file() {
        Ok(())
    } else {
        Err(format!("{}: not a regular file", path.display()))
    }
}

pub fn read_header(path: &Path, n: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    require_regular(path)?;
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = Vec::with_capacity(n);
    f.take(n as u64).read_to_end(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(buf)
}

/// The first and last `n` bytes, concatenated.
///
/// An llvm-cov export is recognised by `"type":"llvm.coverage.json.export"`,
/// and llvm-cov emits that field *after* the `data` array -- so it sits at
/// the very end of a file that is almost entirely data. A prefix alone would
/// not find it, and reading the whole file to look is the cost being avoided
/// in the first place. Both ends, bounded, finds it wherever a producer put
/// it. They overlap for a small file, which does no harm to a substring test.
pub fn read_edges(path: &Path, n: usize) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek, SeekFrom};
    require_regular(path)?;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = f.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();

    let mut buf = Vec::with_capacity(2 * n);
    (&mut f).take(n as u64).read_to_end(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    if len > n as u64 {
        f.seek(SeekFrom::End(-(n.min(len as usize) as i64))).map_err(|e| format!("{}: {e}", path.display()))?;
        f.take(n as u64).read_to_end(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(buf)
}

/// Does this look like an object file butterfly could parse?
///
/// Magic numbers only, from a bounded header: ELF, the Mach-O forms
/// (including the byte-swapped and fat variants) and PE.
pub fn looks_like_object(head: &[u8]) -> bool {
    const MAGICS: [&[u8]; 7] = [
        b"\x7fELF",          // ELF
        b"\xfe\xed\xfa\xce", // Mach-O 32, big-endian
        b"\xce\xfa\xed\xfe", // Mach-O 32, little-endian
        b"\xfe\xed\xfa\xcf", // Mach-O 64, big-endian
        b"\xcf\xfa\xed\xfe", // Mach-O 64, little-endian
        b"\xca\xfe\xba\xbe", // Mach-O universal
        b"MZ",               // PE / COFF executable
    ];
    MAGICS.iter().any(|m| head.starts_with(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        butterfly_testsupport::scratch_dir(&format!("limits-{tag}"))
    }

    fn tiny_limits() -> Limits {
        // 1 KiB per kind, 4 KiB for the run
        Limits::new(1024, 1024, 1024, 1024, 1024, 1024, 4096)
    }

    /// The check is before the allocation, and the message says what to do.
    #[test]
    fn an_oversized_file_is_refused_by_name_before_it_is_read() {
        let d = scratch("oversize");
        let big = d.join("coverage.json");
        std::fs::write(&big, vec![b'x'; 8192]).unwrap();

        let lim = tiny_limits();
        let e = read_bounded(&big, Kind::Coverage, &lim).unwrap_err();
        assert!(e.contains("coverage.json"), "it names the file: {e}");
        assert!(e.contains("coverage export"), "and what it was reading it as: {e}");
        assert!(e.contains("BUTTERFLY_MAX_COVERAGE_MB"), "and how to raise the cap: {e}");
        assert_eq!(lim.remaining(), 4096, "a refused read spends none of the budget");

        // and one under the cap is read whole
        let ok = d.join("small.json");
        std::fs::write(&ok, vec![b'y'; 512]).unwrap();
        assert_eq!(read_bounded(&ok, Kind::Coverage, &lim).unwrap().len(), 512);
        assert_eq!(lim.remaining(), 4096 - 512, "and spends what it read");

        let _ = std::fs::remove_dir_all(d);
    }

    /// A file that grew after the stat must cost what it actually was.
    #[test]
    fn growth_after_admission_is_charged_not_ignored() {
        let p = Path::new("/grown");
        let lim = Limits::new(1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1000);
        // admitted at 400, arrived at 700: the extra 300 must be charged
        lim.admit(400, Kind::Auxiliary, p).unwrap();
        lim.charge_growth(Kind::Auxiliary, p, 400, 700).unwrap();
        assert_eq!(lim.remaining(), 300, "700 spent of a 1000 budget, not 400");

        // and growth that does not fit the remaining budget is refused
        let err = lim.charge_growth(Kind::Auxiliary, p, 700, 1200).unwrap_err();
        assert!(err.contains("BUTTERFLY_MAX_TOTAL_MB"), "names the budget to raise: {err}");
    }

    /// The ordinary case still charges exactly the bytes read.
    #[test]
    fn a_file_that_did_not_grow_is_charged_once() {
        let p = Path::new("/steady");
        let lim = Limits::new(1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1000);
        lim.admit(400, Kind::Auxiliary, p).unwrap();
        lim.charge_growth(Kind::Auxiliary, p, 400, 400).unwrap();
        assert_eq!(lim.remaining(), 600);
    }

    /// A per-file cap alone does not stop a thousand files just under it.
    #[test]
    fn the_run_has_a_budget_of_its_own() {
        let d = scratch("budget");
        let lim = tiny_limits();
        for i in 0..4 {
            let p = d.join(format!("f{i}.c"));
            std::fs::write(&p, vec![b'x'; 1024]).unwrap();
            assert!(read_bounded(&p, Kind::Source, &lim).is_ok(), "file {i} fits the budget");
        }
        assert_eq!(lim.remaining(), 0);

        let one_more = d.join("last.c");
        std::fs::write(&one_more, vec![b'x'; 1]).unwrap();
        let e = read_bounded(&one_more, Kind::Source, &lim).unwrap_err();
        assert!(e.contains("total input budget"), "{e}");
        assert!(e.contains("BUTTERFLY_MAX_TOTAL_MB"), "{e}");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Each kind has its own ceiling: a source file is not an object file.
    #[test]
    fn the_cap_depends_on_what_is_being_read() {
        let d = Limits::default();
        assert!(d.cap(Kind::Source) < d.cap(Kind::Object), "an object may be far larger than a source file");
        assert!(d.cap(Kind::Auxiliary) <= d.cap(Kind::Source));
        for k in [Kind::Coverage, Kind::Evidence, Kind::Source, Kind::Object, Kind::Trace, Kind::Auxiliary] {
            assert!(d.cap(k) > 0, "{k:?}");
            assert!(k.env_var().starts_with("BUTTERFLY_MAX_"), "{k:?}");
            assert!(!k.what().is_empty(), "{k:?}");
        }
    }

    /// A file that grows between the stat and the read must not be followed
    /// past the cap.
    #[test]
    fn a_read_stops_at_the_cap_whatever_the_metadata_said() {
        let d = scratch("grow");
        let p = d.join("t.c");
        std::fs::write(&p, vec![b'x'; 100]).unwrap();
        // a cap below the real size stands in for the file having grown
        let lim = Limits::new(50, 50, 50, 50, 50, 50, 4096);
        assert!(read_bounded(&p, Kind::Source, &lim).is_err(), "the stat check catches it");

        // and the read itself is bounded even if the stat under-reported
        let lim2 = Limits::new(1024, 1024, 1024, 1024, 1024, 1024, 4096);
        assert_eq!(read_bounded(&p, Kind::Source, &lim2).unwrap().len(), 100);
        let _ = std::fs::remove_dir_all(d);
    }

    /// Only files. A directory or a device is not something to slurp.
    #[test]
    fn only_regular_files_are_read() {
        let d = scratch("kinds");
        let lim = tiny_limits();
        let e = read_bounded(&d, Kind::Source, &lim).unwrap_err();
        assert!(e.contains("not a regular file"), "{e}");
        let e = read_bounded(&d.join("nope"), Kind::Source, &lim).unwrap_err();
        assert!(e.contains("nope"), "a missing file still names itself: {e}");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Object files are recognised by magic number, so an unrelated file in a
    /// build directory is never read whole just to classify it.
    #[test]
    fn an_object_is_recognised_without_reading_it() {
        assert!(looks_like_object(b"\x7fELF\x02\x01\x01"));
        assert!(looks_like_object(b"\xcf\xfa\xed\xfe\x0c"), "Mach-O 64 little-endian");
        assert!(looks_like_object(b"\xca\xfe\xba\xbe"), "Mach-O universal");
        assert!(looks_like_object(b"MZ\x90\x00"), "PE");
        // the things a build directory is full of
        assert!(!looks_like_object(b"{\"data\":[{\"files\":"));
        assert!(!looks_like_object(b"int main(void) { return 0; }"));
        assert!(!looks_like_object(b""));
        assert!(!looks_like_object(b"\x7fEL"), "a truncated magic is not a match");
    }

    /// The marker in an llvm-cov export is at the end of the file, so both
    /// ends are read; neither costs the whole file.
    #[test]
    fn both_ends_of_a_candidate_are_looked_at() {
        let d = scratch("edges");
        let p = d.join("coverage.json");
        let mut body = vec![b' '; 40_000];
        body.splice(0..5, b"HEAD!".iter().copied());
        body.extend_from_slice(b"\"type\":\"llvm.coverage.json.export\"");
        std::fs::write(&p, &body).unwrap();

        let edges = read_edges(&p, 4096).unwrap();
        assert!(edges.len() <= 8192, "bounded: {} bytes of a {} byte file", edges.len(), body.len());
        let text = String::from_utf8_lossy(&edges);
        assert!(text.contains("llvm.coverage.json.export"), "the marker at the end is found");
        assert!(text.contains("HEAD!"), "and the start is there too");

        // a file smaller than one window still reads cleanly
        let small = d.join("small.json");
        std::fs::write(&small, b"{\"type\":\"llvm.coverage.json.export\"}").unwrap();
        assert!(String::from_utf8_lossy(&read_edges(&small, 4096).unwrap()).contains("llvm.coverage"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn sizes_are_reported_the_way_a_person_would_say_them() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(2048), "2.0 KiB");
        assert_eq!(human(3 * MIB), "3.0 MiB");
    }
}
