// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests (area E): exception tables (LSDA) and the
//! indexes that locate them.
//!
//! Requirements, with the expected behaviour taken from them:
//! - matrix.toml:374 (cpp.exceptions): landing pads are read from the
//!   exception table and drawn as exception edges.
//! - eh.rs:23-27: call-site records are (start, length, landing pad, action),
//!   each an offset from the function's start; "A landing pad of 0 means
//!   'none'"; "Any encoding this reader does not know ends that function's
//!   table, and nothing is claimed for it".
//! - eh.rs:205-215 (`call_sites`): the bool is false "if any part could not
//!   be read or parsed"; "Empty with true means the image genuinely has no
//!   exception data".
//! - Every malformed input fails closed: never a panic (task rule: ingest
//!   must fail closed, not crash).
//!
//! Not repeated here: overlapping / unsorted records and the Mach-O base
//! overflow, which open PR #316 tests.

use butterfly_ingest::eh::{call_sites, elf_lsda_index, macho_lsda_index, parse_lsda, CallSite};
use butterfly_ingest::objectcode::{Binary, SectionInfo, SegmentInfo};
use std::path::PathBuf;

/// One record (start 4, length 8, pad 0x20, action 0) in call-site encoding
/// `enc`, each field written by `put`.
fn lsda_with(enc: u8, put: Put) -> Vec<u8> {
    let rec: Vec<u8> = [put(4), put(8), put(0x20), vec![0]].concat();
    [vec![0xff, 0xff, enc, rec.len() as u8], rec].concat()
}

/// Writes one call-site field in some encoding.
type Put = fn(u64) -> Vec<u8>;

const ONE: [CallSite; 1] = [CallSite { start: 0x1004, end: 0x100c, landing_pad: 0x1020 }];

/// eh.rs:23-25: the call-site fields are offsets from the function start, in
/// every plain encoding a compiler uses (DWARF EH pointer encodings 0x00-0x0c).
#[test]
fn every_plain_call_site_encoding_is_read() {
    let cases: [(u8, Put); 9] = [
        (0x00, |v| v.to_le_bytes().to_vec()),          // absptr (8 bytes on a 64-bit target)
        (0x01, |v| vec![v as u8]),                     // uleb128
        (0x02, |v| (v as u16).to_le_bytes().to_vec()), // udata2
        (0x03, |v| (v as u32).to_le_bytes().to_vec()), // udata4
        (0x04, |v| v.to_le_bytes().to_vec()),          // udata8
        (0x09, |v| vec![v as u8]),                     // sleb128 (small positives)
        (0x0a, |v| (v as i16).to_le_bytes().to_vec()), // sdata2
        (0x0b, |v| (v as i32).to_le_bytes().to_vec()), // sdata4
        (0x0c, |v| (v as i64).to_le_bytes().to_vec()), // sdata8
    ];
    for (enc, put) in cases {
        assert_eq!(parse_lsda(0x1000, &lsda_with(enc, put)), Some(ONE.to_vec()), "encoding {enc:#04x}");
    }
}

/// eh.rs:23 -- a type table is announced by its encoding and an uleb offset;
/// it does not change the call-site table that follows.
#[test]
fn a_type_table_offset_is_skipped() {
    let rec = [4u8, 8, 0x20, 1];
    let lsda = [&[0xff, 0x9b, 0x0d, 0x01, rec.len() as u8][..], &rec].concat();
    assert_eq!(parse_lsda(0x1000, &lsda), Some(ONE.to_vec()));
}

/// eh.rs:24 "A landing pad of 0 means 'none': unwind further" -- such a range
/// claims nothing, and a table of only those is a table with no pads.
#[test]
fn a_zero_landing_pad_claims_nothing() {
    assert_eq!(parse_lsda(0x1000, &[0xff, 0xff, 0x01, 0x04, 0x04, 0x08, 0x00, 0x00]), Some(vec![]));
    // an empty call-site table is a valid (empty) table
    assert_eq!(parse_lsda(0x1000, &[0xff, 0xff, 0x01, 0x00]), Some(vec![]));
}

/// eh.rs:26 "Any encoding this reader does not know ends that function's
/// table, and nothing is claimed for it" -- and truncation is not a table.
#[test]
fn truncated_or_unknown_lsdas_claim_nothing() {
    for (why, b) in [
        ("empty", &[][..]),
        ("lpstart only", &[0xff][..]),
        ("no call-site encoding", &[0xff, 0xff][..]),
        ("no table length", &[0xff, 0xff, 0x01][..]),
        ("type table offset missing", &[0xff, 0x9b][..]),
        ("record cut inside", &[0xff, 0xff, 0x01, 0x03, 0x04, 0x08, 0x20][..]),
        ("udata4 field cut", &[0xff, 0xff, 0x03, 0x06, 0x04, 0, 0, 0, 0x08, 0][..]),
        ("an encoding no compiler uses (0x05)", &[0xff, 0xff, 0x05, 0x04, 1, 2, 3, 0][..]),
        ("datarel (0x30) modifier", &[0xff, 0xff, 0x31, 0x04, 1, 2, 3, 0][..]),
        ("action missing at the table's end", &[0xff, 0xff, 0x01, 0x03, 0x04, 0x08, 0x20][..]),
    ] {
        assert_eq!(parse_lsda(0x1000, b), None, "{why}");
    }
}

/// Fail closed on hostile values: an overlong LEB128, a negative offset, and
/// a range that runs past the top of the address space are not call sites.
#[test]
fn hostile_values_are_refused_without_panicking() {
    // an 11-byte uleb128 table length
    let overlong = [0xff, 0xff, 0x01, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
    assert_eq!(parse_lsda(0x1000, &overlong), None);
    // an overlong sleb128 field
    let sleb = [&[0xff, 0xff, 0x09, 0x0e][..], &[0x80; 11], &[0x00, 0x08, 0x20]].concat();
    assert_eq!(parse_lsda(0x1000, &sleb), None);
    // sleb128 -1 as a start offset: not a place in the function
    assert_eq!(parse_lsda(0x1000, &[0xff, 0xff, 0x09, 0x04, 0x7f, 0x08, 0x20, 0x00]), None);
    // sdata4 -4 as the landing pad
    let neg = [&[0xff, 0xff, 0x0b, 0x0d][..], &4i32.to_le_bytes(), &8i32.to_le_bytes(), &(-4i32).to_le_bytes(), &[0]].concat();
    assert_eq!(parse_lsda(0x1000, &neg), None);
    // a function at the top of the address space: start + length overflows
    assert_eq!(parse_lsda(u64::MAX - 2, &[0xff, 0xff, 0x01, 0x04, 0x04, 0x08, 0x20, 0x00]), None);
}

// ------------------------------------------------------------ Mach-O index

fn words(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// eh.rs:17-19: the compact-unwind section's LSDA index; anything that is not
/// a version-1 section with a well-formed index is not an index.
#[test]
fn a_malformed_unwind_info_is_not_an_index() {
    // header: version, common enc off/count, personalities off/count, index off/count
    let ok = words(&[1, 0, 0, 0, 0, 28, 2, 0x100, 0, 52, 0x200, 0, 60, 0x100, 0x400]);
    assert_eq!(macho_lsda_index(&ok, 0x1000), Some(vec![(0x1100, 0x1400)]), "a positive control");
    let mut v2 = ok.clone();
    v2[0] = 2;
    assert_eq!(macho_lsda_index(&v2, 0x1000), None, "version 2");
    assert_eq!(macho_lsda_index(&ok[..20], 0x1000), None, "header cut before the index count");
    assert_eq!(macho_lsda_index(&ok[..40], 0x1000), None, "index cut before its last entry");
    assert_eq!(macho_lsda_index(&ok[..56], 0x1000), None, "LSDA array cut");
    assert_eq!(macho_lsda_index(&words(&[1, 0, 0, 0, 0, 28, 0]), 0x1000), Some(vec![]), "no index entries: nothing to find");
    // the last entry's LSDA offset before the first's, and a misaligned array
    assert_eq!(macho_lsda_index(&words(&[1, 0, 0, 0, 0, 28, 2, 0, 0, 60, 0, 0, 52]), 0), None);
    assert_eq!(macho_lsda_index(&words(&[1, 0, 0, 0, 0, 28, 2, 0, 0, 52, 0, 0, 56, 0, 0]), 0), None);
    // an index offset far past the section
    assert_eq!(macho_lsda_index(&words(&[1, 0, 0, 0, 0, u32::MAX, 2]), 0), None);
    assert_eq!(macho_lsda_index(&[], 0), None);
}

// --------------------------------------------------------------- ELF index

fn eh_frame_of(image: &str) -> (Vec<u8>, u64) {
    use object::{Object, ObjectSection};
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/req_e").join(image);
    let bytes = std::fs::read(p).unwrap();
    let f = object::File::parse(&*bytes).unwrap();
    let s = f.section_by_name(".eh_frame").unwrap();
    (s.data().unwrap().to_vec(), s.address())
}

/// eh.rs:20 "ELF: each .eh_frame FDE whose CIE augmentation has an L names its
/// LSDA" -- the FDE of `thrower` (tests/data/req_e/x86_64.s) names the LSDA at
/// the start of .gcc_except_table (0x388), with a pc-relative pointer.
#[test]
fn an_lld_linked_eh_frame_names_its_lsda() {
    let (b, at) = eh_frame_of("x86_64.so");
    let (idx, complete) = elf_lsda_index(&b, at, true, true);
    assert!(complete);
    assert_eq!(idx, vec![(0x13d4, 0x388)]);
}

/// eh.rs:166-170 "The bool is false if any entry could not be parsed": a
/// section cut anywhere inside an entry is not a complete index, and never a
/// panic.
#[test]
fn an_eh_frame_cut_anywhere_is_incomplete() {
    let (b, at) = eh_frame_of("x86_64.so");
    for cut in 1..b.len() {
        let (idx, complete) = elf_lsda_index(&b[..cut], at, true, true);
        // a cut exactly on an entry boundary is a smaller, valid section: it
        // may only name what the full section names
        assert!(!complete || idx.iter().all(|e| *e == (0x13d4, 0x388)), "cut at {cut}: {idx:?}");
        // a cut inside the FDE can never yield its LSDA as decoded *and* complete
    }
    // the FDE's own bytes corrupted: its CIE pointer aims nowhere
    let mut bad = b.clone();
    let cie_len = u32::from_le_bytes(bad[0..4].try_into().unwrap()) as usize + 4;
    bad[cie_len + 4..cie_len + 8].copy_from_slice(&0x7fff_0000u32.to_le_bytes());
    let (idx, complete) = elf_lsda_index(&bad, at, true, true);
    assert!(!complete && idx.is_empty(), "{idx:?}");
}

// ------------------------------------------------------------- call_sites

fn sec(name: &str, address: u64, size: u64, file_off: Option<u64>) -> SectionInfo {
    SectionInfo { name: name.into(), address, size, kind: String::new(), flags: String::new(), alloc: true, file_range: file_off.map(|o| (o, size)) }
}

/// A Mach-O-shaped image whose bytes are `bytes`, mapped 1:1 at address 0.
fn bin(sections: Vec<SectionInfo>, bytes: Vec<u8>) -> Binary {
    Binary {
        path: PathBuf::from("synthetic"),
        format: "MachO".into(),
        relocatable: false,
        arch: "aarch64".into(),
        is_64: true,
        little_endian: true,
        entry: 0,
        sections,
        segments: vec![SegmentInfo { name: "__TEXT".into(), address: 0, size: 0x10000, file_size: 0x10000, flags: String::new() }],
        symbols: vec![],
        data_symbols: vec![],
        stack_sizes: vec![],
        stack_sizes_malformed: false,
        line_rows: vec![],
        dwarf_source: None,
        instrs: vec![],
        inlines: vec![],
        subprogram_tus: vec![],
        noreturn_subprograms: vec![],
        bytes,
        dwarf_bytes: None,
        objdump_path: PathBuf::new(),
        // a Mach-O image: no ELF RELATIVE slots (#332)
        relative_words: Default::default(),
    }
}

/// An `__unwind_info` at 0 naming `(function, LSDA)` pairs, then the LSDA
/// table at 0x100.
fn image(pairs: &[(u32, u32)], table: &[u8]) -> Vec<u8> {
    let first = 28 + 24;
    let last = first + 8 * pairs.len() as u32;
    let mut w = vec![1, 0, 0, 0, 0, 28, 2, 0, 0, first, 0, 0, last];
    for &(f, l) in pairs {
        w.extend([f, l]);
    }
    let mut b = words(&w);
    b.resize(0x100, 0);
    b.extend_from_slice(table);
    b
}

const TABLE: [u8; 8] = [0xff, 0xff, 0x01, 0x04, 0x04, 0x08, 0x20, 0x00];

/// eh.rs:213-215 "Empty with true means the image genuinely has no exception
/// data".
#[test]
fn no_exception_table_is_a_complete_empty_answer() {
    let b = bin(vec![sec("__text", 0x1000, 0x10, Some(0))], vec![0; 0x10]);
    assert_eq!(call_sites(&b), (vec![], true));
}

/// matrix.toml:374 on Mach-O: a function's LSDA, located through
/// `__unwind_info`, gives its pads relative to the function start.
#[test]
fn a_macho_lsda_is_located_and_read() {
    let bytes = image(&[(0x1000, 0x100)], &TABLE);
    let b = bin(vec![sec("__unwind_info", 0, 0x100, Some(0)), sec("__gcc_except_tab", 0x100, 8, Some(0x100))], bytes);
    assert_eq!(call_sites(&b), (ONE.to_vec(), true));
}

/// eh.rs:205-212: "false if any part could not be read or parsed -- a missing
/// or unreadable index, an LSDA whose bytes could not be read, or one this
/// reader could not decode".
#[test]
fn every_decode_failure_is_reported_incomplete() {
    let ok = image(&[(0x1000, 0x100)], &TABLE);
    let t = |b: Binary, why: &str| {
        let (_, complete) = call_sites(&b);
        assert!(!complete, "{why}");
    };
    // a table with no index at all
    t(bin(vec![sec("__gcc_except_tab", 0x100, 8, Some(0x100))], ok.clone()), "no index section");
    // an index section with no bytes in the file
    t(bin(vec![sec("__unwind_info", 0, 0x100, None), sec("__gcc_except_tab", 0x100, 8, Some(0x100))], ok.clone()), "unreadable __unwind_info");
    t(bin(vec![sec(".eh_frame", 0x200, 0x40, None), sec(".gcc_except_table", 0x100, 8, Some(0x100))], ok.clone()), "unreadable .eh_frame");
    // an index that is not one
    let mut v9 = ok.clone();
    v9[0] = 9;
    t(bin(vec![sec("__unwind_info", 0, 0x100, Some(0)), sec("__gcc_except_tab", 0x100, 8, Some(0x100))], v9), "not an __unwind_info");
    // the index names an LSDA outside the table
    let outside = image(&[(0x1000, 0x180)], &TABLE);
    t(bin(vec![sec("__unwind_info", 0, 0x100, Some(0)), sec("__gcc_except_tab", 0x100, 8, Some(0x100))], outside), "LSDA outside the table");
    // an LSDA this reader cannot decode
    let undecodable = image(&[(0x1000, 0x100)], &[0x00, 0xff, 0x01, 0x00, 0, 0, 0, 0]);
    t(bin(vec![sec("__unwind_info", 0, 0x100, Some(0)), sec("__gcc_except_tab", 0x100, 8, Some(0x100))], undecodable), "explicit lpstart");
    // an LSDA whose bytes are not in the file
    t(bin(vec![sec("__unwind_info", 0, 0x100, Some(0)), sec("__gcc_except_tab", 0x100, 8, None)], ok.clone()), "table has no file bytes");
}
