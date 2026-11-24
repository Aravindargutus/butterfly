// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `Binary::entry` is a virtual address, whatever the format called it.
//!
//! `object::File::entry()` returns what the header holds, and a Mach-O header
//! holds `LC_MAIN.entryoff` -- a file offset. butterfly compared it against
//! virtual symbol addresses, so it reported the real entry as "not at a
//! symbol boundary" and could not exempt it from dead-code candidacy (#8).
//!
//! The unit tests beside `entry_vaddr_of` cover the arithmetic on every
//! format. This covers the thing they cannot: a real Mach-O, linked by a real
//! linker, read through the real `object` crate.
//!
//! This whole suite is macOS-only: only a macOS build of the examples emits a
//! Mach-O. On any other target it used to print `skipping:` at run time --
//! twice, once per test -- and `.github/workflows/ci.yml`'s "no suite may
//! skip" guard, which excludes only the variants and QEMU suites by name,
//! failed the Ubuntu job on those two unexcluded lines even though the tests
//! themselves were passing. Compile-gating the module to macOS means it emits
//! nothing on Linux -- no test to skip, no line for the guard to catch -- and
//! loses no coverage, since these tests could never run off a Mach-O host
//! anyway. On macOS it is unchanged.
#![cfg(target_os = "macos")]

use std::path::PathBuf;

/// A linked Mach-O executable from the built examples, if there is one.
fn macho() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for c in ["examples/c/variants/O0/demo", "examples/c/build/demo"] {
        let p = root.join(c);
        if p.exists() {
            return Some(p);
        }
    }
    butterfly_testsupport::fixture_missing("build examples/c first");
    None
}

#[test]
fn a_macho_entry_lands_on_the_entry_symbol() {
    let Some(path) = macho() else { return };
    let bin = butterfly_ingest::objectcode::load(&path, None, &butterfly_model::limits::Limits::from_env()).expect("read the image");

    assert_eq!(bin.format, "MachO", "{path:?}");
    assert!(!bin.relocatable, "a linked image");
    assert!(bin.entry != 0, "a linked executable has an entry point");

    // it is an address in the loaded image, not a file offset
    let text = bin.segments.iter().find(|s| s.flags.contains('X')).expect("an executable segment");
    assert!(
        bin.entry >= text.address && bin.entry < text.address + text.size,
        "entry {:#x} is outside the executable segment at {:#x}..{:#x} — it is still a file offset",
        bin.entry,
        text.address,
        text.address + text.size
    );

    // and it is exactly where the entry symbol is
    let at = bin.symbols.iter().find(|s| s.address == bin.entry);
    let at = at.unwrap_or_else(|| panic!("entry {:#x} is not at a symbol boundary; symbols start at {:#x}", bin.entry, bin.symbols.first().map(|s| s.address).unwrap_or(0)));
    assert!(at.name.trim_start_matches('_') == "main", "the entry is `main`, not {:?}", at.name);
}

/// The consequence the issue names: with the entry unplaceable, the symbol it
/// belongs to was not exempt from dead-code candidacy.
#[test]
fn the_entry_symbol_is_never_a_dead_code_candidate() {
    let Some(_) = macho() else { return };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ev_path = root.join("examples/c/variants/evidence-O0.json");
    if !ev_path.exists() {
        butterfly_testsupport::not_applicable("run `butterfly build examples/c/butterfly-build.toml` first");
        return;
    }
    let ev: butterfly_model::Evidence = serde_json::from_str(&std::fs::read_to_string(&ev_path).unwrap()).unwrap();
    let image = ev.image.as_ref().expect("a linked image");
    let entry = image.entry.expect("an entry point");

    let at_entry = ev.symbols.iter().find(|s| s.address == entry).expect("the entry is at a symbol");
    assert!(!image.dead_candidates.iter().any(|d| d.symbol_id == at_entry.id), "`{}` is the entry point and cannot be dead code", at_entry.name);
    assert!(
        image.findings.iter().any(|f| f.contains("entry point") && f.contains(&at_entry.name)),
        "the findings name the entry symbol: {:?}",
        image.findings.iter().filter(|f| f.contains("entry")).collect::<Vec<_>>()
    );
    assert!(!image.findings.iter().any(|f| f.contains("not at a symbol boundary")), "{:?}", image.findings);
}
