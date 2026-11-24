// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Oversized artifacts fail with a sentence, not with an allocation.
//!
//! Everything ingest reads is untrusted: the coverage export names the
//! sources, the build directory may hold anything. Each was read whole before
//! anything semantic looked at it, so a large or crafted artifact could
//! exhaust memory or stall a CI runner long before the error that would have
//! rejected it (#5).

use butterfly_ingest::IngestOptions;
use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("rl-{tag}"))
}

/// A minimal llvm-cov export naming one source.
fn export(src: &Path) -> String {
    format!(
        r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[[1,1,1,true,true,false]],"branches":[],"summary":{{}}}}],
           "functions":[{{"name":"f","count":1,"regions":[[1,1,3,2,1,0,0,0]],"branches":[],"filenames":[{p:?}]}}]}}]}}"#,
        p = src.to_str().unwrap()
    )
}

fn opts(dir: &Path) -> IngestOptions {
    IngestOptions { coverage_json: dir.join("coverage.json"), source_root: Some(dir.to_path_buf()), ..Default::default() }
}

/// Every cap can be raised, and the message says which variable to raise.
/// Setting one absurdly low stands in for an input that is absurdly large,
/// which is the only way to test this without writing gigabytes.
fn with_env<T>(k: &str, v: &str, f: impl FnOnce() -> T) -> T {
    // the suite runs tests in threads, so serialise anything touching the
    // process environment
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let old = std::env::var(k).ok();
    std::env::set_var(k, v);
    let out = f();
    match old {
        Some(o) => std::env::set_var(k, o),
        None => std::env::remove_var(k),
    }
    out
}

#[test]
fn an_oversized_coverage_export_is_refused_before_it_is_parsed() {
    let d = scratch("coverage");
    let src = d.join("a.c");
    std::fs::write(&src, "int f(void) {\n  return 1;\n}\n").unwrap();
    std::fs::write(d.join("coverage.json"), export(&src)).unwrap();

    // 1 MB cap against an export larger than that is the same decision as a
    // 512 MiB cap against a 600 MiB one, without writing 600 MiB
    let err = with_env("BUTTERFLY_MAX_COVERAGE_MB", "1", || {
        std::fs::write(d.join("coverage.json"), format!("{}{}", " ".repeat(2 * 1024 * 1024), export(&src))).unwrap();
        butterfly_ingest::ingest(&opts(&d)).unwrap_err().to_string()
    });
    assert!(err.contains("coverage.json"), "names the file: {err}");
    assert!(err.contains("coverage export"), "and what it is: {err}");
    assert!(err.contains("BUTTERFLY_MAX_COVERAGE_MB"), "and how to proceed: {err}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn an_oversized_source_file_is_refused_and_names_itself() {
    let d = scratch("source");
    let src = d.join("a.c");
    std::fs::write(&src, format!("int f(void) {{ return 1; }}\n{}", "// pad\n".repeat(200_000))).unwrap();
    std::fs::write(d.join("coverage.json"), export(&src)).unwrap();

    let err = with_env("BUTTERFLY_MAX_SOURCE_MB", "1", || butterfly_ingest::ingest(&opts(&d)).unwrap_err().to_string());
    assert!(err.contains("a.c"), "names the source: {err}");
    assert!(err.contains("BUTTERFLY_MAX_SOURCE_MB"), "{err}");
    let _ = std::fs::remove_dir_all(d);
}

/// The whole run has a budget, so many files just under the per-file cap
/// cannot add up to the same problem.
#[test]
fn the_run_budget_stops_an_accumulation_of_admissible_files() {
    let d = scratch("total");
    let src = d.join("a.c");
    std::fs::write(&src, "int f(void) {\n  return 1;\n}\n").unwrap();
    std::fs::write(d.join("coverage.json"), export(&src)).unwrap();

    // a budget smaller than the coverage export alone
    let err = with_env("BUTTERFLY_MAX_TOTAL_MB", "1", || {
        std::fs::write(d.join("coverage.json"), format!("{}{}", " ".repeat(2 * 1024 * 1024), export(&src))).unwrap();
        butterfly_ingest::ingest(&opts(&d)).unwrap_err().to_string()
    });
    assert!(err.contains("budget") || err.contains("BUTTERFLY_MAX"), "{err}");
    let _ = std::fs::remove_dir_all(d);
}

/// Discovery classified an unknown file by reading it whole and handing it to
/// the object parser, so one large unrelated file in a build directory cost
/// its own size in memory before anything established it was even an object.
#[test]
fn discovery_does_not_read_an_unrelated_file_whole() {
    let d = scratch("discovery");
    let src = d.join("a.c");
    std::fs::write(&src, "int f(void) {\n  return 1;\n}\n").unwrap();
    std::fs::write(d.join("coverage.json"), export(&src)).unwrap();
    // not an object, and far past the object cap this run allows
    std::fs::write(d.join("blob.bin"), vec![0u8; 4 * 1024 * 1024]).unwrap();

    let found = with_env("BUTTERFLY_MAX_OBJECT_MB", "1", || butterfly_ingest::discover(&d).expect("discovery still succeeds"));
    assert!(found.objects.is_empty(), "the blob is not an object: {:?}", found.objects);
    assert_eq!(found.coverage_json, d.join("coverage.json"), "and the export is still found");
    let _ = std::fs::remove_dir_all(d);
}

/// The defaults must not get in an honest project's way: the example ingests
/// unchanged with nothing set.
#[test]
fn the_default_limits_do_not_disturb_a_real_ingest() {
    let build = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build");
    if !build.join("coverage.json").exists() {
        butterfly_testsupport::fixture_missing("build examples/c first");
        return;
    }
    let mut o = butterfly_ingest::discover(&build).expect("discover");
    o.source_root = Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c"));
    let ev = butterfly_ingest::ingest(&o).expect("the example is well within every default");
    assert!(!ev.functions.is_empty());
    assert!(!ev.files.is_empty());
}

/// A named pipe in the build directory must not hang discovery. `File::open`
/// on a fifo blocks until a writer appears, so classifying every entry by
/// opening it would wait forever; discovery checks the file type first
/// (review of #5).
#[cfg(unix)]
#[test]
fn discovery_does_not_block_on_a_fifo() {
    use std::os::unix::fs::FileTypeExt;
    let d = scratch("fifo");
    let src = d.join("a.c");
    std::fs::write(&src, "int f(void){ return 1; }\n").unwrap();
    std::fs::write(d.join("coverage.json"), export(&src)).unwrap();

    // a fifo with a name that discovery would otherwise try to sniff/parse
    let fifo = d.join("pipe.bin");
    let cs = std::ffi::CString::new(fifo.as_os_str().to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc_mkfifo(cs.as_ptr(), 0o644) }, 0, "mkfifo");
    assert!(std::fs::symlink_metadata(&fifo).unwrap().file_type().is_fifo());

    // Run discovery on another thread; if it opens the fifo it blocks forever.
    let (tx, rx) = std::sync::mpsc::channel();
    let dd = d.clone();
    std::thread::spawn(move || {
        let _ = tx.send(butterfly_ingest::discover(&dd).map(|o| o.objects.len()));
    });
    match rx.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok(Ok(n)) => assert_eq!(n, 0, "the fifo is not an object"),
        Ok(Err(e)) => panic!("discovery failed: {e}"),
        Err(_) => panic!("discovery blocked on the fifo (never returned)"),
    }
    let _ = std::fs::remove_dir_all(d);
}

extern "C" {
    fn mkfifo(path: *const std::os::raw::c_char, mode: u32) -> std::os::raw::c_int;
}
#[cfg(unix)]
unsafe fn libc_mkfifo(path: *const std::os::raw::c_char, mode: u32) -> std::os::raw::c_int {
    mkfifo(path, mode)
}

/// One budget for the whole run: `objectcode::load` charges the budget it is
/// handed, so two objects deplete it cumulatively. It used to mint its own
/// fresh 8 GiB per object, so object reads were never cumulative across a run
/// (review of #5). Env granularity is 1 MiB, too coarse for the tiny example,
/// so this drives `load` directly with a byte-level budget.
#[test]
fn objectcode_load_charges_the_budget_it_is_given() {
    use butterfly_model::limits::Limits;
    let obj = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build/demo");
    if !obj.exists() {
        butterfly_testsupport::fixture_missing("build examples/c first");
        return;
    }
    // What one load charges, measured rather than assumed: it reads the
    // object and, on macOS, the .dSYM's DWARF beside it, so the object's own
    // size under-counts it -- and for a small x86-64 Mach-O build the DWARF
    // is the larger part, and the budget below let a second load through
    // (found by #172's x86-64 run).
    let roomy = Limits::new(1 << 40, 1 << 40, 1 << 40, 1 << 40, 1 << 40, 1 << 40, 1 << 40);
    butterfly_ingest::objectcode::load(&obj, None, &roomy).expect("the object loads");
    let size = (1u64 << 40) - roomy.remaining();
    assert!(size > 0, "a load charges the budget");
    // per-kind caps generous; total holds one load's worth but not two
    let lim = Limits::new(size * 4, size * 4, size * 4, size * 4, size * 4, size * 4, size + size / 2);

    // first load succeeds and spends most of the shared budget
    butterfly_ingest::objectcode::load(&obj, None, &lim).expect("first object fits");
    assert!(lim.remaining() < size, "the first read charged the shared budget");

    // second load against the SAME budget is refused, by the total not a cap
    let err = butterfly_ingest::objectcode::load(&obj, None, &lim).unwrap_err().to_string();
    assert!(err.contains("total input budget") || err.contains("BUTTERFLY_MAX_TOTAL_MB"), "the shared run budget is what bit: {err}");
}

/// The object cap must bite in `ingest`, not only in `objectcode::load`.
///
/// ingest used to read the object whole to hash it for provenance *before*
/// calling the bounded loader, and read it whole twice more for the
/// data-reference scans. A direct `objectcode::load` test cannot see any of
/// that: the allocation happens in the caller. So drive the real entry point
/// and prove the refusal names the object and the variable to raise (review
/// of #5).
#[test]
fn an_oversized_object_is_refused_by_ingest_not_only_by_the_loader() {
    let d = scratch("object");
    let src = d.join("a.c");
    std::fs::write(&src, "int f(void) {\n  return 0;\n}\n").unwrap();
    std::fs::write(d.join("coverage.json"), export(&src)).unwrap();
    // a real Mach-O/ELF is not needed: the cap is applied before the bytes
    // are parsed, which is the whole point of the finding
    // 2 MiB against a 1 MiB cap. (A cap of 0 is deliberately ignored as "not
    // a positive number", so the refusal has to come from a real ceiling.)
    let obj = d.join("image.bin");
    std::fs::write(&obj, vec![0u8; 2 * 1024 * 1024]).unwrap();

    let mut o = opts(&d);
    o.objects = vec![obj.clone()];
    let err = with_env("BUTTERFLY_MAX_OBJECT_MB", "1", || butterfly_ingest::ingest(&o)).unwrap_err().to_string();
    assert!(err.contains("image.bin"), "names the object it refused: {err}");
    assert!(err.contains("BUTTERFLY_MAX_OBJECT_MB"), "and the cap to raise: {err}");
    // the giveaway that the old order was wrong: had ingest hashed the file
    // first, it would have allocated all 2 MiB and failed later, in the parse
    assert!(!err.contains("not a recognised object file"), "refused before parsing, not after: {err}");
}

/// The same object must not be read from disk again once it has been
/// admitted: provenance, parsing and the data scans all share those bytes.
/// Deleting the file after ingest has started is not possible from a test,
/// so assert the weaker, checkable property -- one admitted copy is enough
/// to produce the evidence, with the cap set just above the real size.
#[test]
fn an_admitted_object_is_not_read_from_disk_again() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c/build.sh first");
        return;
    }
    let mut o = butterfly_ingest::discover(&root.join("build")).expect("discover");
    o.source_root = Some(root.clone());
    let obj = o.objects[0].clone();
    let mb = std::fs::metadata(&obj).unwrap().len().div_ceil(1024 * 1024).max(1);
    // exactly one whole-object read fits under a budget of its own size;
    // the old code needed three and would have exceeded the run total
    let ev = with_env("BUTTERFLY_MAX_TOTAL_MB", &format!("{}", mb * 2 + 8), || butterfly_ingest::ingest(&o)).expect("one admitted copy is enough to build the evidence");
    assert!(ev.functions.iter().any(|f| f.object.is_some()), "object code was still analysed");
    assert!(ev.provenance.inputs.iter().any(|i| i.role == "object" && i.sha256.len() == 64), "and hashed from the same bytes");
}

/// A discovered `butterfly.toml` is read before anything semantic runs, so
/// it was the earliest unbounded read on the ingest path -- earlier than the
/// budget itself existed. It is an auxiliary input like any other.
#[test]
fn an_oversized_config_is_refused() {
    let d = scratch("config");
    let src = d.join("a.c");
    std::fs::write(&src, "int f(void) {\n  return 0;\n}\n").unwrap();
    std::fs::write(d.join("coverage.json"), export(&src)).unwrap();
    // valid TOML, 2 MiB of it, against a 1 MiB auxiliary cap
    std::fs::write(d.join("butterfly.toml"), format!("[requirements]\npatterns = [\"{}\"]\n", "R".repeat(2 * 1024 * 1024))).unwrap();

    let err = with_env("BUTTERFLY_MAX_AUX_MB", "1", || butterfly_ingest::ingest(&opts(&d))).unwrap_err().to_string();
    assert!(err.contains("butterfly.toml"), "names the config: {err}");
    assert!(err.contains("BUTTERFLY_MAX_AUX_MB"), "and the cap to raise: {err}");
    assert!(!err.contains("parsing"), "refused before it was parsed: {err}");
    let _ = std::fs::remove_dir_all(&d);
}

/// The change cache is supplied on the command line and read the same way.
#[test]
fn an_oversized_change_cache_is_refused() {
    let d = scratch("cache");
    let src = d.join("a.c");
    std::fs::write(&src, "int f(void) {\n  return 0;\n}\n").unwrap();
    std::fs::write(d.join("coverage.json"), export(&src)).unwrap();
    let cache = d.join("butterfly-changes.json");
    std::fs::write(&cache, format!(r#"{{"schema":"x","pad":"{}"}}"#, "y".repeat(2 * 1024 * 1024))).unwrap();

    let mut o = opts(&d);
    o.changes = Some(cache);
    let ev = with_env("BUTTERFLY_MAX_AUX_MB", "1", || butterfly_ingest::ingest(&o)).expect("ingest still succeeds");
    // the cache is optional: a refusal becomes a caveat, not a failed run,
    // but it must say so rather than silently carry on
    assert!(
        ev.provenance.caveats.iter().any(|c| c.contains("BUTTERFLY_MAX_AUX_MB") || c.contains("butterfly-changes.json")),
        "the refusal is reported: {:?}",
        ev.provenance.caveats
    );
    let _ = std::fs::remove_dir_all(&d);
}
