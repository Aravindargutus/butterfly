// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for bounded reads (area A).
//!
//! Expected values come from README.md "Security posture", the "Bounded
//! reads" bullet (README.md:1583-1599), and the matching bullet of
//! docs/butterfly.html "Security posture".

use butterfly_model::limits::{looks_like_object, read_bounded, read_bounded_string, read_header, Kind, Limits};
use std::path::{Path, PathBuf};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("req-a-limits-{tag}"))
}

const KINDS: [(Kind, &str); 6] = [
    (Kind::Coverage, "BUTTERFLY_MAX_COVERAGE_MB"),
    (Kind::Evidence, "BUTTERFLY_MAX_EVIDENCE_MB"),
    (Kind::Source, "BUTTERFLY_MAX_SOURCE_MB"),
    (Kind::Object, "BUTTERFLY_MAX_OBJECT_MB"),
    (Kind::Trace, "BUTTERFLY_MAX_TRACE_MB"),
    (Kind::Auxiliary, "BUTTERFLY_MAX_AUX_MB"),
];

/// README.md:1592-1595 "raise any with `BUTTERFLY_MAX_SOURCE_MB`, ... or
/// `BUTTERFLY_MAX_TOTAL_MB`. Every refusal names the variable that would
/// have let it through" -- and README.md:1589 names the file.
#[test]
fn every_kind_refuses_past_its_cap_naming_its_variable_and_the_file() {
    for (kind, var) in KINDS {
        let l = Limits::new(10, 10, 10, 10, 10, 10, GIB);
        let p = Path::new("/somewhere/the-input.bin");
        assert!(l.admit(10, kind, p).is_ok(), "{var}: a file exactly at the cap is admitted");
        let e = l.admit(11, kind, p).expect_err("one byte over");
        assert!(e.contains(var), "names {var}: {e}");
        assert!(e.contains("the-input.bin"), "names the file: {e}");
        assert_eq!(kind.env_var(), var);
    }
}

/// README.md:1584-1586 "against a per-kind cap and a budget for the whole
/// run" -- files each under their cap are refused once their sum passes the
/// budget, naming `BUTTERFLY_MAX_TOTAL_MB`.
#[test]
fn the_run_budget_binds_across_files() {
    let l = Limits::new(GIB, GIB, GIB, GIB, GIB, 100, 150);
    let p = Path::new("x.yaml");
    l.admit(100, Kind::Auxiliary, p).expect("first fits");
    assert_eq!(l.remaining(), 50, "and is charged to the budget");
    let e = l.admit(60, Kind::Auxiliary, p).expect_err("the second does not");
    assert!(e.contains("BUTTERFLY_MAX_TOTAL_MB"), "{e}");
    assert_eq!(l.remaining(), 50, "a refused file is not charged");
    l.admit(50, Kind::Auxiliary, p).expect("exactly the rest fits");
    assert_eq!(l.remaining(), 0);
}

/// README.md:1583-1586 "is sized before it is read, against a per-kind cap
/// and a budget for the whole run" -- a file that grows after it was sized
/// is charged again, and growth past the budget is refused with its variable.
#[test]
fn growth_after_sizing_is_charged_and_capped() {
    let l = Limits::new(GIB, GIB, 100, GIB, GIB, GIB, GIB);
    let p = Path::new("src/a.c");
    l.admit(80, Kind::Source, p).unwrap();
    l.charge_growth(Kind::Source, p, 80, 80).expect("no growth, no charge");
    let before = l.remaining();
    l.charge_growth(Kind::Source, p, 80, 90).expect("10 bytes of growth fit");
    assert_eq!(before - l.remaining(), 10);

    let tight = Limits::new(GIB, GIB, 100, GIB, GIB, GIB, 90);
    tight.admit(80, Kind::Source, p).unwrap();
    let e = tight.charge_growth(Kind::Source, p, 80, 95).expect_err("growth past the run budget");
    assert!(e.contains("BUTTERFLY_MAX_TOTAL_MB") && e.contains("a.c"), "{e}");
}

/// README.md:1583-1590 sized before it is read; README.md:1589-1590 a refusal
/// names the file. `read_bounded` returns exactly the file's bytes when it is
/// admitted, and a sentence naming it when it is not.
#[test]
fn read_bounded_reads_what_fits_and_names_what_does_not() {
    let d = scratch("rb");
    let p = d.join("doc.txt");
    std::fs::write(&p, "hello, world").unwrap();
    let ok = Limits::new(GIB, GIB, GIB, GIB, GIB, 64, GIB);
    assert_eq!(read_bounded(&p, Kind::Auxiliary, &ok).unwrap(), b"hello, world");
    assert_eq!(read_bounded_string(&p, Kind::Auxiliary, &ok).unwrap(), "hello, world");
    let tight = Limits::new(GIB, GIB, GIB, GIB, GIB, 5, GIB);
    let e = read_bounded(&p, Kind::Auxiliary, &tight).expect_err("over the cap");
    assert!(e.contains("doc.txt") && e.contains("BUTTERFLY_MAX_AUX_MB"), "{e}");
    let e = read_bounded(&d.join("missing.txt"), Kind::Auxiliary, &ok).expect_err("missing");
    assert!(e.contains("missing.txt"), "names the missing file: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1583-1590 "A crafted ... input fails with a sentence naming the
/// file" -- text that is not UTF-8 read as text is refused, not mangled
/// silently.
#[test]
fn bytes_that_are_not_text_are_refused_as_text() {
    let d = scratch("utf8");
    let p = d.join("bad.yaml");
    std::fs::write(&p, [b'a', 0xff, 0xfe, b'b']).unwrap();
    let e = read_bounded_string(&p, Kind::Auxiliary, &Limits::from_env()).expect_err("not UTF-8");
    assert!(e.contains("bad.yaml"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1590-1591 "discovery classifies an unknown file by its magic
/// number rather than by reading it whole" -- ELF and Mach-O (README.md:284
/// "ELF or Mach-O") are recognised from a header; text is not.
#[test]
fn objects_are_recognised_by_their_magic_number() {
    assert!(looks_like_object(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0"), "ELF");
    assert!(looks_like_object(&[0xcf, 0xfa, 0xed, 0xfe, 7, 0, 0, 1]), "Mach-O 64-bit");
    assert!(looks_like_object(&[0xce, 0xfa, 0xed, 0xfe, 7, 0, 0, 0]), "Mach-O 32-bit");
    assert!(!looks_like_object(b"{\"data\":[]}"), "JSON is not an object");
    assert!(!looks_like_object(b""), "nor is nothing");
    assert!(!looks_like_object(b"\x7fEL"), "nor is a truncated magic");

    let d = scratch("head");
    let p = d.join("big.bin");
    std::fs::write(&p, vec![b'z'; 4096]).unwrap();
    assert_eq!(read_header(&p, 16).unwrap().len(), 16, "only the header is read");
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1592-1594 "The defaults are generous (64 MiB a source, 2 GiB an
/// object, 8 GiB a run)".
#[test]
fn the_documented_defaults() {
    // The environment may raise any of them; only an unset variable shows the default.
    let unset = |v: &str| std::env::var_os(v).is_none();
    let l = Limits::from_env();
    if unset("BUTTERFLY_MAX_SOURCE_MB") {
        assert_eq!(l.cap(Kind::Source), 64 * MIB);
    }
    if unset("BUTTERFLY_MAX_OBJECT_MB") {
        assert_eq!(l.cap(Kind::Object), 2 * GIB);
    }
    if unset("BUTTERFLY_MAX_TOTAL_MB") {
        assert_eq!(l.remaining(), 8 * GIB);
    }
}
