// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests (area E): the object-code analyses on linked
//! **ELF** images, x86-64 and AArch64.
//!
//! The support matrix says several ELF paths have never run on a real image
//! here: the `.eh_frame` LSDA index (cpp.exceptions), the x86-64 jump table
//! on ELF (c.switch, said until #325 ran it on Linux images), `R_*_GLOB_DAT` GOT calls on an ingested ELF image
//! (c.dynamic-frame), and x86-64 data coupling on an image (arch.x86-64).
//! These images reach every one of them on any host, without a linker:
//! `tests/data/req_e/{x86_64,aarch64}.s`, linked by `build.sh` into the
//! committed `.so` files. Every expected address below is read from the
//! assembly source and the linker's layout (`llvm-objdump -d` of the image),
//! never from what butterfly reports.

use butterfly_ingest::cfg::{self, Ctl};
use butterfly_ingest::datarefs::{self, Access};
use butterfly_ingest::eh::{self, CallSite};
use butterfly_ingest::objectcode::{self, Binary};
use butterfly_ingest::{got, jumptable};
use std::path::PathBuf;

fn image(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/req_e").join(name)
}

fn load(name: &str) -> Binary {
    objectcode::load(&image(name), None, &butterfly_model::limits::Limits::from_env()).expect("load the image")
}

fn sym(bin: &Binary, name: &str) -> (u64, u64) {
    let s = bin.symbols.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no symbol {name}"));
    (s.address, s.address + s.size)
}

fn instr(bin: &Binary, addr: u64) -> &objectcode::RawInstr {
    bin.instrs.iter().find(|i| i.addr == addr).unwrap_or_else(|| panic!("no instruction at {addr:#x}"))
}

fn coupling(bin: &Binary, func: &str) -> Vec<(u64, u64, Access)> {
    let (s, e) = sym(bin, func);
    let lookup = |a: u64| bin.data_symbols.iter().find(|d| a >= d.address && a < d.address + d.size.max(1)).map(|d| d.address);
    datarefs::scan(&bin.arch, bin.instrs_in(s, e), &lookup).into_iter().map(|d| (d.addr, d.target, d.access)).collect()
}

fn data(bin: &Binary, name: &str) -> u64 {
    bin.data_symbols.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("no data symbol {name}")).address
}

// ---------------------------------------------------------------- x86-64 ELF

/// matrix.toml:374 (cpp.exceptions) "Landing pads are read from the exception
/// table (... the ELF .eh_frame path has not been run on a real image)";
/// eh.rs:20 "ELF: each .eh_frame FDE whose CIE augmentation has an L names its
/// LSDA". The source has two call sites, one with a pad at `.Lpad0` and one
/// that unwinds further (pad 0), which claims nothing.
#[test]
fn x86_64_elf_landing_pads_come_from_eh_frame() {
    let bin = load("x86_64.so");
    assert_eq!((bin.format.as_str(), bin.arch.as_str()), ("Elf", "x86_64"));
    let (sites, complete) = eh::call_sites(&bin);
    assert!(complete, "every FDE with an LSDA was decoded");
    // thrower @0x13d4: pushq (1) | callq @0x13d5 (5) | callq @0x13da | popq | retq | .Lpad0 @0x13e1
    assert_eq!(sites, vec![CallSite { start: 0x13d5, end: 0x13da, landing_pad: 0x13e1 }]);
}

/// matrix.toml:374 "drawn as exception edges, so no pad is called
/// unreachable; an exception edge is never projected covered".
#[test]
fn x86_64_elf_landing_pad_is_entered_by_an_exception_edge() {
    let bin = load("x86_64.so");
    let (sites, _) = eh::call_sites(&bin);
    let (s, e) = sym(&bin, "thrower");
    let raw = bin.instrs_in(s, e);
    let pads = |a: u64| sites.iter().filter(|c| a >= c.start && a < c.end).map(|c| c.landing_pad).collect::<Vec<_>>();
    let g = cfg::build(&bin.arch, s, e, raw, &|_| (Some(1), Some(0), true), &|_| butterfly_model::Status::Covered, &|_| None, &|_| Vec::new(), &pads);
    let pad = g.blocks.iter().position(|b| b.address == 0x13e1).expect("the pad starts a block");
    let into: Vec<_> = g.blocks.iter().flat_map(|b| &b.succs).filter(|x| x.to == pad).collect();
    assert_eq!(into.len(), 1, "{into:?}");
    assert_eq!(into[0].kind, butterfly_model::EdgeKind::Exception);
    assert_eq!(into[0].projected, butterfly_model::Status::Unknown, "both ends ran, yet whether an exception took the edge is unknown");
}

/// matrix.toml:189 (c.dynamic-frame) "The same resolution covers ELF
/// R_*_GLOB_DAT slots (... x86-64 `call *foo@GOTPCREL(%rip)`) ... not on an
/// ingested ELF image"; got.rs:10-14: the slot is bound to one named symbol,
/// so the call is a call, and a `jmp` through it a tail call.
#[test]
fn x86_64_elf_glob_dat_calls_name_their_symbol() {
    let bytes = std::fs::read(image("x86_64.so")).unwrap();
    let file = object::File::parse(&*bytes).unwrap();
    let b = got::bindings(&file);
    // an import: bound to its name, with no address in this image
    assert_eq!(b.get(&0x2530), Some(&got::Bound { symbol: "ext_fn".into(), addr: None }));
    assert_eq!(b.get(&0x2538), Some(&got::Bound { symbol: "ext_tail".into(), addr: None }));
    assert!(!b.values().any(|n| n.symbol == "may_throw"), "a JUMP_SLOT belongs to the PLT, not a GOT call: {b:?}");

    // load() annotates what it disassembled
    let bin = load("x86_64.so");
    let call = instr(&bin, 0x13e5);
    assert_eq!(cfg::classify(&bin.arch, &call.mnemonic, &call.operands), Ctl::Call, "{call:?}");
    assert_eq!(cfg::target_symbol(&call.operands).as_deref(), Some("ext_fn"));
    let tail = instr(&bin, 0x13ec);
    assert_eq!(cfg::classify(&bin.arch, &tail.mnemonic, &tail.operands), Ctl::TailCall, "{tail:?}");
    assert_eq!(cfg::target_symbol(&tail.operands).as_deref(), Some("ext_tail"));
}

/// matrix.toml:74 (c.switch) "x86-64 32-bit offsets ... the x86-64 path has
/// been run here on Mach-O and on rust-lld-linked x86-64 Linux ELF images";
/// "until #325 the ELF disassembly's single-`#` comment hid the table's
/// address" (BUG-E9). The table is `.Ltable` in .rodata (0x378), four
/// entries relative to itself.
#[test]
fn x86_64_elf_jump_table_is_read_from_rodata() {
    let bin = load("x86_64.so");
    let (s, e) = sym(&bin, "dispatch");
    let t = jumptable::recover(&bin.arch, bin.instrs_in(s, e), s, e, &|a, l| bin.read_at(a, l));
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!((t[0].branch, t[0].addr, t[0].len), (0x1407, 0x378, 16));
    assert_eq!(t[0].targets, vec![0x1409, 0x140f, 0x1415, 0x141b]);
}

/// README.md:243-249: on x86-64, "a direct RIP-relative operand and ...
/// materialising the address with lea and then loading or storing through
/// that register"; "A read-modify-write (`incl (%rax)`, ...) counts as both
/// a read and a write". `writer` takes `counter`'s address, increments it
/// through %rax, stores to the file-static `flag` and reads it back.
#[test]
fn x86_64_elf_data_coupling_on_a_linked_image() {
    let bin = load("x86_64.so");
    let (counter, flag) = (data(&bin, "counter"), data(&bin, "flag"));
    assert_eq!(
        coupling(&bin, "writer"),
        vec![(0x1424, counter, Access::Address), (0x142b, counter, Access::Read), (0x142b, counter, Access::Write), (0x142d, flag, Access::Write), (0x1437, flag, Access::Read),]
    );
}

/// README.md:238 "Frame sizes come from a `.stack_sizes` section". The
/// source lists thrower at 16 bytes and writer at 0.
#[test]
fn x86_64_elf_stack_sizes_section_is_read() {
    let bin = load("x86_64.so");
    assert!(!bin.stack_sizes_malformed);
    let (thrower, _) = sym(&bin, "thrower");
    let (writer, _) = sym(&bin, "writer");
    let mut got = bin.stack_sizes.clone();
    got.sort_unstable();
    assert_eq!(got, vec![(thrower, 16), (writer, 0)]);
}

// --------------------------------------------------------------- AArch64 ELF

/// matrix.toml:374, eh.rs:20 -- the same table on AArch64 ELF.
#[test]
fn aarch64_elf_landing_pads_come_from_eh_frame() {
    let bin = load("aarch64.so");
    assert_eq!((bin.format.as_str(), bin.arch.as_str()), ("Elf", "aarch64"));
    let (sites, complete) = eh::call_sites(&bin);
    assert!(complete);
    // thrower @0x103c4: stp | bl @0x103c8 | bl @0x103cc | ldp | ret | .Lpad0 @0x103d8
    assert_eq!(sites, vec![CallSite { start: 0x103c8, end: 0x103cc, landing_pad: 0x103d8 }]);
}

/// matrix.toml:189 "ELF R_*_GLOB_DAT slots (AArch64 adrp/ldr/blr ...)":
/// a call, and a `br` through a bound slot is a tail call to that symbol.
#[test]
fn aarch64_elf_glob_dat_calls_name_their_symbol() {
    let bytes = std::fs::read(image("aarch64.so")).unwrap();
    let file = object::File::parse(&*bytes).unwrap();
    let b = got::bindings(&file);
    assert_eq!(b.get(&0x20570), Some(&got::Bound { symbol: "ext_fn".into(), addr: None }));
    assert_eq!(b.get(&0x20578), Some(&got::Bound { symbol: "ext_tail".into(), addr: None }));
    let bin = load("aarch64.so");
    let call = instr(&bin, 0x103f0);
    assert_eq!(cfg::classify(&bin.arch, &call.mnemonic, &call.operands), Ctl::Call, "{call:?}");
    assert_eq!(cfg::target_symbol(&call.operands).as_deref(), Some("ext_fn"));
    let tail = instr(&bin, 0x10400);
    assert_eq!(cfg::classify(&bin.arch, &tail.mnemonic, &tail.operands), Ctl::TailCall, "{tail:?}");
    assert_eq!(cfg::target_symbol(&tail.operands).as_deref(), Some("ext_tail"));
}

/// matrix.toml:74 "AArch64 word, halfword and byte entries": clang -O2's
/// byte table, scaled by four from the `adr` base.
#[test]
fn aarch64_elf_jump_table_is_read_from_rodata() {
    let bin = load("aarch64.so");
    let (s, e) = sym(&bin, "dispatch");
    let t = jumptable::recover(&bin.arch, bin.instrs_in(s, e), s, e, &|a, l| bin.read_at(a, l));
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!((t[0].branch, t[0].addr, t[0].len), (0x10424, 0x378, 4));
    assert_eq!(t[0].targets, vec![0x10428, 0x10430, 0x10438, 0x10440]);
}

/// README.md:243 "`adrp`+`add`/`ldr`/`str` on AArch64".
#[test]
fn aarch64_elf_data_coupling_on_a_linked_image() {
    let bin = load("aarch64.so");
    let (counter, flag) = (data(&bin, "counter"), data(&bin, "flag"));
    assert_eq!(coupling(&bin, "writer"), vec![(0x10454, counter, Access::Address), (0x1045c, flag, Access::Read), (0x10464, flag, Access::Write)]);
}

/// README.md:238 ".stack_sizes" on AArch64 ELF.
#[test]
fn aarch64_elf_stack_sizes_section_is_read() {
    let bin = load("aarch64.so");
    assert!(!bin.stack_sizes_malformed);
    let (thrower, _) = sym(&bin, "thrower");
    let (writer, _) = sym(&bin, "writer");
    let mut got = bin.stack_sizes.clone();
    got.sort_unstable();
    assert_eq!(got, vec![(thrower, 16), (writer, 0)]);
}
