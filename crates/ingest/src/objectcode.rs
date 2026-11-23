// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Object-code readers: symbols (via `object`), DWARF line table (via `gimli`)
//! and disassembly text (via `llvm-objdump`, part of the same LLVM toolchain
//! that produced the binary).

use anyhow::{anyhow, bail, Context, Result};
use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct TextSymbol {
    pub name: String,
    pub address: u64,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct LineRow {
    pub address: u64,
    pub file: Arc<str>,
    pub line: u32,
    pub is_stmt: bool,
    pub end_sequence: bool,
}

#[derive(Debug, Clone)]
pub struct RawInstr {
    pub addr: u64,
    pub mnemonic: String,
    pub operands: String,
}

#[derive(Debug, Clone)]
pub struct SectionInfo {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub kind: String,
    pub flags: String,
    /// Loaded into the process image at `address` (ELF SHF_ALLOC; a Mach-O
    /// section with a real virtual address). Only these carry a meaningful
    /// virtual address: `.symtab`, `.strtab`, `.debug_*` and the like sit at a
    /// nominal address of 0 with a large size, so a virtual-address lookup that
    /// did not exclude them could read symbol-table bytes for a `.rodata`
    /// address (#171 review).
    pub alloc: bool,
    /// (offset, size) of the section's bytes in the file, when it has any
    /// (a `.bss` has none).
    pub file_range: Option<(u64, u64)>,
}

#[derive(Debug, Clone)]
pub struct SegmentInfo {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub file_size: u64,
    pub flags: String,
}

#[derive(Debug, Clone)]
pub struct Binary {
    pub path: PathBuf,
    pub format: String,
    /// An unlinked object (`.o`): addresses are section-relative placeholders
    /// and cross-unit references are still relocations, not addresses.
    pub relocatable: bool,
    pub arch: String,
    pub is_64: bool,
    pub little_endian: bool,
    /// Where execution starts, as a **virtual address**, or 0 when the image
    /// has no entry point or it could not be resolved. Normalised on the way
    /// in, so consumers can compare it against symbol addresses directly.
    pub entry: u64,
    pub sections: Vec<SectionInfo>,
    pub segments: Vec<SegmentInfo>,
    pub symbols: Vec<TextSymbol>,
    /// Data symbols (globals/statics), sorted by address.
    pub data_symbols: Vec<TextSymbol>,
    /// (function address, stack frame bytes) from a `.stack_sizes` section.
    pub stack_sizes: Vec<(u64, u64)>,
    /// The `.stack_sizes` section was present but malformed (an overlong or
    /// overflowing ULEB, a truncated final entry, or unreadable data), so the
    /// pairs above stop at the first bad entry. The caller raises a caveat
    /// (#247): a reviewer must see the stack evidence is degraded, not read a
    /// truncated list as complete.
    pub stack_sizes_malformed: bool,
    /// Sorted by address.
    pub line_rows: Vec<LineRow>,
    pub dwarf_source: Option<PathBuf>,
    /// Sorted by address.
    pub instrs: Vec<RawInstr>,
    /// Inlined subroutines from DWARF.
    pub inlines: Vec<InlineRec>,
    /// `DW_AT_low_pc` of every `DW_TAG_subprogram`, paired with the source
    /// path of the compilation unit that emitted it (the unit's
    /// `DW_AT_name`), sorted by address. Two translation units may each emit
    /// a local symbol under one name -- a `static inline` in a shared header
    /// does exactly that -- and the symbol table cannot tell the copies
    /// apart, but the subprogram's own unit can (#36).
    pub subprogram_tus: Vec<(u64, Arc<str>)>,
    /// `(DW_AT_low_pc, name)` of every `DW_TAG_subprogram` the producer
    /// marked `DW_AT_noreturn`, sorted by address. This is the compiler's
    /// own statement that the function does not return — C and C++
    /// `_Noreturn` / `[[noreturn]]`, and any Rust `fn` whose return type is
    /// uninhabited — not a guess from the symbol's spelling. The CFG builder
    /// uses it to end a block at such a call instead of drawing a
    /// fall-through edge no execution can take (#210).
    pub noreturn_subprograms: Vec<(u64, Arc<str>)>,
    /// ELF `R_*_RELATIVE` slots: address -> the pointer the slot holds once
    /// loaded at the link-time base. A PIE's file holds 0 there ([`crate::relptr`],
    /// #325 G1). Empty for Mach-O, unlinked objects and non-PIE images.
    pub relative_words: std::collections::BTreeMap<u64, u64>,
    /// The image's bytes, as admitted by the run's budget. Kept so a caller
    /// reuses them for provenance hashing and data scans instead of reading
    /// the file again -- an unbounded re-read would walk straight past the
    /// cap this load already applied, and would charge the budget once for
    /// several full reads (review of #5).
    pub bytes: Vec<u8>,
    /// The dSYM's bytes, on the same terms, when one was used.
    pub dwarf_bytes: Option<Vec<u8>>,
    /// The `llvm-objdump` this load resolved and ran to disassemble the image,
    /// so the caller can record it in provenance (the object-code evidence is
    /// this tool's output).
    pub objdump_path: PathBuf,
}

/// What one walk of `.debug_info` yields: the inlined subroutines, the
/// `DW_AT_low_pc` of every subprogram paired with the source path of the
/// compilation unit that emitted it, and the subprograms the producer marked
/// `DW_AT_noreturn`.
type DebugInfo = (Vec<InlineRec>, Vec<(u64, Arc<str>)>, Vec<(u64, Arc<str>)>);

/// One `DW_TAG_inlined_subroutine`: `origin` was inlined into the function
/// containing `ranges`, at `call_line` of the caller.
#[derive(Debug, Clone)]
pub struct InlineRec {
    pub origin: Arc<str>,
    pub call_line: Option<u32>,
    /// Empty when the inlined body was folded away entirely.
    pub ranges: Vec<(u64, u64)>,
    pub depth: u32,
    /// `DW_AT_low_pc` of the enclosing subprogram.
    pub subprogram_low: Option<u64>,
}

pub fn load(path: &Path, llvm_bin: Option<&Path>, lim: &butterfly_model::limits::Limits) -> Result<Binary> {
    let bytes = butterfly_model::limits::read_bounded(path, butterfly_model::limits::Kind::Object, lim).map_err(|e| anyhow::anyhow!("{e}"))?;
    load_from_bytes(bytes, path, llvm_bin, lim, false)
}

/// [`load`] for an object file discovered by name inside a build directory:
/// opened with [`crate::fsauth::open_discovered`] (`O_NOFOLLOW` + single-link
/// regular file check) before reading bounded bytes (#6.4).
pub fn load_discovered(path: &Path, llvm_bin: Option<&Path>, lim: &butterfly_model::limits::Limits) -> Result<Binary> {
    let file = crate::fsauth::open_discovered(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let bytes = butterfly_model::limits::read_bounded_from(file, path, butterfly_model::limits::Kind::Object, lim).map_err(|e| anyhow::anyhow!("{e}"))?;
    load_from_bytes(bytes, path, llvm_bin, lim, true)
}

fn load_from_bytes(bytes: Vec<u8>, path: &Path, llvm_bin: Option<&Path>, lim: &butterfly_model::limits::Limits, discovered: bool) -> Result<Binary> {
    if bytes.starts_with(b"!<arch>\n") || bytes.starts_with(b"!<thin>\n") {
        bail!(
            "{} is a static library archive, which cannot be analysed directly.\n\
             Object-code analysis needs a linked image: link the library with its test \n\
             harness (or into the integration image) and pass that binary instead, or \n\
             extract the archive and pass its members with repeated --object.",
            path.display()
        );
    }
    let file = object::File::parse(&*bytes).with_context(|| format!("{} is not a recognised object file", path.display()))?;
    let relocatable = matches!(file.kind(), object::ObjectKind::Relocatable);
    let format = format!("{:?}", file.format());
    let arch = format!("{:?}", file.architecture()).to_lowercase();
    let is_macho = matches!(file.format(), object::BinaryFormat::MachO);

    let mut symbols = text_symbols(&file);
    fill_missing_sizes(&mut symbols, &file, object::SectionKind::Text);
    let mut data_symbols = data_symbols_of(&file);
    fill_missing_sizes(&mut data_symbols, &file, object::SectionKind::Data);
    let sections = sections_of(&file);
    let segments = segments_of(&file);
    let (stack_sizes, stack_sizes_malformed) = stack_sizes_of(&file);

    // DWARF: in the file itself, or in a dSYM bundle next to a Mach-O binary.
    // One pass over the debug info, and -- when it lives in a dSYM beside a
    // Mach-O binary -- one read of that bundle, whose bytes are then kept. A
    // dSYM is the largest input in a debug build and runs to hundreds of
    // megabytes on a real project, so reading it per consumer cost both the
    // time and, once there was a budget, several charges for one file.
    let mut dwarf_source = None;
    let mut dwarf_bytes: Option<Vec<u8>> = None;
    let mut line_rows = load_line_rows(&file).unwrap_or_default();
    let (mut inlines, mut subprogram_tus, mut noreturn_subprograms) = load_inlines(&file).unwrap_or_default();
    if is_macho && (line_rows.is_empty() || subprogram_tus.is_empty()) {
        if let Some(dsym) = find_dsym(path) {
            let dbytes = if discovered {
                let df = crate::fsauth::open_discovered(&dsym).map_err(|e| anyhow::anyhow!("{}: {e}", dsym.display()))?;
                butterfly_model::limits::read_bounded_from(df, &dsym, butterfly_model::limits::Kind::Object, lim).map_err(|e| anyhow::anyhow!("{e}"))?
            } else {
                butterfly_model::limits::read_bounded(&dsym, butterfly_model::limits::Kind::Object, lim).map_err(|e| anyhow::anyhow!("{e}"))?
            };
            let dfile = object::File::parse(&*dbytes).with_context(|| format!("parsing dSYM {}", dsym.display()))?;
            // A dSYM is found by name alone, so one left by another build of
            // the same program was read as this image's debug information,
            // mapping every address to another build's lines (#287). The
            // link step stamps both with one UUID; unequal or missing, it is
            // not this image's.
            let (img, dbg) = (file.mach_uuid().ok().flatten(), dfile.mach_uuid().ok().flatten());
            if img.is_none() || img != dbg {
                let show = |u: Option<[u8; 16]>| u.map(|b| b.iter().map(|x| format!("{x:02X}")).collect::<String>()).unwrap_or_else(|| "none".into());
                bail!(
                    "{} is not the debug information of {}: its UUID is {}, the image's is {}. It was left by another build; rerun dsymutil on this image, or remove the stale dSYM.",
                    dsym.display(),
                    path.display(),
                    show(dbg),
                    show(img)
                );
            }
            line_rows = load_line_rows(&dfile)?;
            (inlines, subprogram_tus, noreturn_subprograms) = load_inlines(&dfile).unwrap_or_default();
            dwarf_source = Some(dsym);
            dwarf_bytes = Some(dbytes);
        }
    }
    // Sort by address, but put a sequence terminator *before* a real row at
    // the same address. A DWARF `end_sequence` row marks the end of the
    // previous sequence and covers nothing; when the next sequence starts at
    // exactly that address — which is what happens whenever one function abuts
    // the next — the real row must win, or every instruction at the start of
    // that function looks as if it had no source line.
    line_rows.sort_by_key(|r| (r.address, !r.end_sequence));

    // Resolve the disassembler once and disassemble with it, so the exact
    // executable that produced the object-code evidence is what we record.
    let objdump_path = find_llvm_tool("llvm-objdump", llvm_bin)?;
    let mut instrs = disassemble(&objdump_path, &bytes, path, is_macho)?;
    // A call through a GOT slot names the symbol the slot is bound to (#271).
    // An unlinked object has no bound slots: its GOT references are still
    // relocations.
    if !relocatable {
        crate::got::annotate(&arch, &mut instrs, &crate::got::bindings(&file));
    }

    let relative_words = if relocatable { Default::default() } else { crate::relptr::of_file(&file, &bytes) };

    Ok(Binary {
        relative_words,
        path: path.to_path_buf(),
        format,
        relocatable,
        arch,
        is_64: file.is_64(),
        little_endian: file.is_little_endian(),
        entry: entry_vaddr(&file, &bytes, is_macho, relocatable),
        sections,
        segments,
        symbols,
        data_symbols,
        stack_sizes,
        stack_sizes_malformed,
        line_rows,
        dwarf_source,
        instrs,
        inlines,
        subprogram_tus,
        noreturn_subprograms,
        bytes,
        dwarf_bytes,
        objdump_path,
    })
}

fn text_symbols(file: &object::File) -> Vec<TextSymbol> {
    let mut v: Vec<TextSymbol> = file
        .symbols()
        .filter(|s| s.kind() == object::SymbolKind::Text && s.is_definition())
        .filter_map(|s| {
            let name = s.name().ok()?;
            if name.is_empty() || name.starts_with("$x") || name.starts_with("$d") || name.starts_with("ltmp") || name.starts_with("__mh_") {
                return None;
            }
            Some(TextSymbol { name: name.to_string(), address: s.address(), size: s.size() })
        })
        .collect();
    v.sort_by_key(|s| (s.address, std::cmp::Reverse(s.size)));
    v.dedup_by(|a, b| a.address == b.address && a.name == b.name);
    v
}

/// Sections that hold data (globals, tables, strings). `object` leaves some
/// Mach-O sections such as `__DATA_CONST,__const` as `Unknown`, so fall back
/// to the segment name for those.
pub fn is_data_section(s: &object::Section) -> bool {
    match s.kind() {
        object::SectionKind::Data
        | object::SectionKind::ReadOnlyData
        | object::SectionKind::UninitializedData
        | object::SectionKind::ReadOnlyString
        | object::SectionKind::Tls
        | object::SectionKind::UninitializedTls
        | object::SectionKind::Common => true,
        object::SectionKind::Unknown => {
            let seg = s.segment_name().ok().flatten().unwrap_or("");
            (seg.starts_with("__DATA") || seg == "__RODATA") && s.address() != 0 && s.size() > 0
        }
        _ => false,
    }
}

fn data_symbols_of(file: &object::File) -> Vec<TextSymbol> {
    let data_sections: Vec<(u64, u64)> = file.sections().filter(is_data_section).map(|s| (s.address(), s.address().saturating_add(s.size()))).collect();
    let mut v: Vec<TextSymbol> = file
        .symbols()
        .filter(|s| {
            s.is_definition()
                && (s.kind() == object::SymbolKind::Data || (s.kind() == object::SymbolKind::Unknown && data_sections.iter().any(|(a, e)| s.address() >= *a && s.address() < *e)))
        })
        .filter_map(|s| {
            let name = s.name().ok()?;
            if name.is_empty() || name.starts_with("$d") || name.starts_with("$x") || name.starts_with("ltmp") || name.starts_with("L_") || name.starts_with(".L") {
                return None;
            }
            Some(TextSymbol { name: name.to_string(), address: s.address(), size: s.size() })
        })
        .collect();
    v.sort_by_key(|s| (s.address, std::cmp::Reverse(s.size)));
    v.dedup_by(|a, b| a.address == b.address && a.name == b.name);
    v
}

fn sections_of(file: &object::File) -> Vec<SectionInfo> {
    let mut v: Vec<SectionInfo> = file
        .sections()
        .filter(|s| s.size() > 0)
        .map(|s| {
            let kind = format!("{:?}", s.kind());
            let flags = match s.flags() {
                object::SectionFlags::Elf { sh_flags } => {
                    let mut f = String::new();
                    if sh_flags & 0x2 != 0 {
                        f.push('A');
                    }
                    if sh_flags & 0x1 != 0 {
                        f.push('W');
                    }
                    if sh_flags & 0x4 != 0 {
                        f.push('X');
                    }
                    f
                }
                object::SectionFlags::MachO { flags } => format!("{flags:#x}"),
                _ => String::new(),
            };
            // Loaded at a real virtual address: ELF SHF_ALLOC, or a Mach-O
            // section the linker placed (address != 0). Non-allocated ELF
            // sections report address 0, so they must not answer a VA lookup.
            let alloc = match s.flags() {
                object::SectionFlags::Elf { sh_flags } => sh_flags & 0x2 != 0,
                _ => s.address() != 0,
            };
            SectionInfo { name: s.name().unwrap_or("?").to_string(), address: s.address(), size: s.size(), kind, flags, alloc, file_range: s.file_range() }
        })
        .collect();
    v.sort_by_key(|s| s.address);
    v
}

/// Does this segment hold executable code?
fn segment_is_executable(s: &object::Segment) -> bool {
    flags_are_executable(s.flags())
}

/// The execute bit, per format. Split out because it is what keeps
/// `__PAGEZERO` -- which spans the whole low 4GiB of the address space and
/// would swallow any file offset -- out of the entry-point search (#8).
fn flags_are_executable(f: object::SegmentFlags) -> bool {
    match f {
        object::SegmentFlags::Elf { p_flags } => p_flags & 0x1 != 0,
        object::SegmentFlags::MachO { initprot, .. } => initprot & 0x4 != 0,
        _ => false,
    }
}

/// The entry point as a virtual address.
///
/// `object::File::entry()` hands back whatever the format's header holds.
/// For ELF and PE that is already a virtual address. For Mach-O it is
/// `LC_MAIN.entryoff`, which is a **file offset** -- so butterfly compared a
/// file offset against virtual symbol addresses, reported the real entry as
/// "not at a symbol boundary", and could not exempt it from dead-code
/// candidacy. On `examples/c` that is `entryoff 2064` against `_main` at
/// `0x100000810` (#8).
///
/// Which of the two a Mach-O carries depends on its load command -- `LC_MAIN`
/// gives an offset, the older `LC_UNIXTHREAD` gives an address -- so rather
/// than guessing from the command, ask where the value actually lands. Only
/// executable segments are considered: `__PAGEZERO` spans the whole low 4GiB
/// of the address space, and would otherwise swallow any offset.
///
/// Returns 0 when there is no entry point (an unlinked `.o`) or when it
/// cannot be placed, so the caller's existing `!= 0` test still means "there
/// is an entry point and we know where it is".
fn entry_vaddr(file: &object::File, bytes: &[u8], is_macho: bool, relocatable: bool) -> u64 {
    let exec: Vec<ExecSeg> = file
        .segments()
        .filter(segment_is_executable)
        .map(|s| {
            let (file_off, file_len) = s.file_range();
            ExecSeg { addr: s.address(), file_off, file_len }
        })
        .collect();
    // For Mach-O the raw value comes from the load command itself, not from
    // `file.entry()`: `object` 0.36 only reads `LC_MAIN`, and hands back zero
    // for a thread-state entry, so an LC_UNIXTHREAD image would be reported as
    // having no entry at all (review of #8).
    let (kind, raw) = if is_macho { macho_entry(bytes) } else { (MachoEntry::Address, file.entry()) };
    entry_vaddr_of(raw, kind, relocatable, &exec)
}

/// What a Mach-O's entry value *is*, read from the load command that set it
/// rather than guessed from the number.
///
/// `object::File::entry()` returns `LC_MAIN.entryoff` -- a file offset -- for
/// one form and the `LC_UNIXTHREAD` program counter -- a virtual address --
/// for the other, and the two are not distinguishable from the value and the
/// segment ranges alone: a low-linked image can have a file offset that also
/// lands inside an executable segment's virtual-address range, so range
/// inference returned it unmapped (review of #8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MachoEntry {
    /// LC_MAIN: a file offset, mapped through the segment that contains it.
    FileOffset,
    /// LC_UNIXTHREAD: already a virtual address.
    Address,
    /// Neither command is present (a dylib, a bundle): no entry point.
    None,
}

/// Where in an `LC_UNIXTHREAD` register block the program counter sits.
///
/// The block is a sequence of `flavor, count, state[count]` groups, and the
/// PC's position inside `state` is architecture-specific. Flavor numbers are
/// *not* unique across architectures -- flavor 1 is `x86_THREAD_STATE32` on
/// Intel and `ARM_THREAD_STATE` on ARM -- so the CPU type decides, and the
/// flavor is checked against it rather than trusted on its own.
///
/// Offsets are into `state`, in bytes:
///   * x86-64  `x86_THREAD_STATE64` (4): rip is register 16 of 21 u64 -> 128
///   * arm64   `ARM_THREAD_STATE64` (6): x[29], fp, lr, sp, pc -> 32 u64 -> 256
///   * i386    `x86_THREAD_STATE32` (1): eip is word 10 -> 40
///   * arm     `ARM_THREAD_STATE`   (1): r[13], sp, lr, pc -> word 15 -> 60
fn thread_pc_layout(cputype: u32) -> Option<(u32, usize, bool)> {
    use object::macho::*;
    match cputype {
        CPU_TYPE_X86_64 => Some((4, 128, true)),
        CPU_TYPE_ARM64 => Some((6, 256, true)),
        CPU_TYPE_X86 => Some((1, 40, false)),
        CPU_TYPE_ARM => Some((1, 60, false)),
        _ => None,
    }
}

/// The program counter out of an `LC_UNIXTHREAD` register block.
fn unixthread_pc<E: object::Endian>(state: &[u8], endian: E, cputype: u32) -> Option<u64> {
    let (want_flavor, off, is64) = thread_pc_layout(cputype)?;
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = state.get(at..at + 4)?.try_into().ok()?;
        Some(endian.read_u32_bytes(b))
    };
    let mut at = 0usize;
    // walk the flavor/count groups until the one this architecture uses
    while at + 8 <= state.len() {
        let flavor = u32_at(at)?;
        let count = u32_at(at + 4)? as usize;
        let body = at + 8;
        let len = count.checked_mul(4)?;
        let end = body.checked_add(len)?;
        if end > state.len() {
            return None;
        }
        if flavor == want_flavor {
            let s = &state[body..end];
            return if is64 {
                let b: [u8; 8] = s.get(off..off + 8)?.try_into().ok()?;
                Some(endian.read_u64_bytes(b))
            } else {
                let b: [u8; 4] = s.get(off..off + 4)?.try_into().ok()?;
                Some(endian.read_u32_bytes(b) as u64)
            };
        }
        at = end;
    }
    None
}

/// Read the entry-point load command from a thin Mach-O: what kind it is,
/// and the raw value it carries.
///
/// `object::File::parse` does not accept a fat/universal binary, so entry
/// resolution only ever sees a thin image at offset 0 -- 32- or 64-bit,
/// either endianness, all handled by `object::Endianness` at runtime.
///
/// The value is read here rather than from `object::File::entry()` because
/// that only understands `LC_MAIN`; for `LC_UNIXTHREAD` it returns zero, and
/// a zero raw value is indistinguishable from "no entry point".
fn macho_entry(data: &[u8]) -> (MachoEntry, u64) {
    use object::read::macho::MachHeader;
    use object::FileKind;

    fn scan<M: MachHeader>(data: &[u8]) -> (MachoEntry, u64) {
        let none = (MachoEntry::None, 0);
        let Ok(header) = M::parse(data, 0) else { return none };
        let Ok(endian) = header.endian() else { return none };
        let cputype = header.cputype(endian);
        let Ok(mut cmds) = header.load_commands(endian, data, 0) else { return none };
        // An image may carry both commands. dyld keeps the first of each kind
        // and then prefers LC_MAIN, so the whole list is scanned and the
        // choice made afterwards -- returning on the first match reported the
        // thread-state PC for a UNIXTHREAD-first image, which is not the
        // entry dyld would use (review of #8).
        let mut main_off: Option<u64> = None;
        let mut thread_pc: Option<u64> = None;
        let mut saw_thread = false;
        while let Ok(Some(cmd)) = cmds.next() {
            match cmd.cmd() {
                object::macho::LC_MAIN if main_off.is_none() => {
                    main_off = Some(cmd.entry_point().ok().flatten().map(|e| e.entryoff.get(endian)).unwrap_or(0));
                }
                object::macho::LC_UNIXTHREAD if !saw_thread => {
                    saw_thread = true;
                    if let Ok(object::read::macho::LoadCommandVariant::Thread(_, state)) = cmd.variant() {
                        thread_pc = unixthread_pc(state, endian, cputype);
                    }
                }
                _ => {}
            }
        }
        match (main_off, thread_pc) {
            (Some(off), _) => (MachoEntry::FileOffset, off),
            // the command was there but its register block could not be read:
            // refuse rather than report a wrong entry
            (None, Some(pc)) => (MachoEntry::Address, pc),
            (None, None) => none,
        }
    }

    match FileKind::parse(data) {
        Ok(FileKind::MachO32) => scan::<object::macho::MachHeader32<object::Endianness>>(data),
        Ok(FileKind::MachO64) => scan::<object::macho::MachHeader64<object::Endianness>>(data),
        _ => (MachoEntry::None, 0),
    }
}

/// One executable segment: where it lives in memory, and where its bytes are
/// in the file.
#[derive(Debug, Clone, Copy)]
struct ExecSeg {
    addr: u64,
    file_off: u64,
    file_len: u64,
}

/// The decision on its own, so it can be tested without a binary to hand.
fn entry_vaddr_of(raw: u64, kind: MachoEntry, relocatable: bool, exec: &[ExecSeg]) -> u64 {
    if relocatable || raw == 0 || kind == MachoEntry::None {
        return 0;
    }
    match kind {
        // ELF, PE and LC_UNIXTHREAD already carry a virtual address
        MachoEntry::Address => raw,
        // LC_MAIN is a file offset: map it through the executable segment that
        // contains it. This is the only case that maps, so a file offset that
        // also happens to fall in a virtual-address range is no longer
        // mistaken for an address.
        MachoEntry::FileOffset => {
            exec.iter().find(|s| raw >= s.file_off && raw < s.file_off.saturating_add(s.file_len)).map(|s| s.addr.saturating_add(raw - s.file_off)).unwrap_or(0)
        }
        MachoEntry::None => 0,
    }
}

fn segments_of(file: &object::File) -> Vec<SegmentInfo> {
    let mut v: Vec<SegmentInfo> = file
        .segments()
        .map(|s| {
            let (_, fsize) = s.file_range();
            let flags = match s.flags() {
                object::SegmentFlags::Elf { p_flags } => {
                    let mut f = String::new();
                    if p_flags & 0x4 != 0 {
                        f.push('R');
                    }
                    if p_flags & 0x2 != 0 {
                        f.push('W');
                    }
                    if p_flags & 0x1 != 0 {
                        f.push('X');
                    }
                    f
                }
                object::SegmentFlags::MachO { initprot, .. } => {
                    let mut f = String::new();
                    if initprot & 0x1 != 0 {
                        f.push('R');
                    }
                    if initprot & 0x2 != 0 {
                        f.push('W');
                    }
                    if initprot & 0x4 != 0 {
                        f.push('X');
                    }
                    f
                }
                _ => String::new(),
            };
            SegmentInfo { name: s.name().ok().flatten().unwrap_or("").to_string(), address: s.address(), size: s.size(), file_size: fsize, flags }
        })
        .filter(|s| s.size > 0)
        .collect();
    v.sort_by_key(|s| s.address);
    v
}

/// `.stack_sizes`: repeated (address, ULEB128 size) pairs.
fn stack_sizes_of(file: &object::File) -> (Vec<(u64, u64)>, bool) {
    let Some(sec) = file.section_by_name(".stack_sizes").or_else(|| file.section_by_name("__stack_sizes")) else { return (Vec::new(), false) };
    let Ok(data) = sec.uncompressed_data() else {
        // The section is present but unreadable (bad compression): that is
        // itself malformed metadata, not an absent section.
        return (Vec::new(), true);
    };
    parse_stack_sizes(&data, if file.is_64() { 8 } else { 4 }, file.is_little_endian())
}

/// Decode a `.stack_sizes` payload into (address, size) pairs. Split out from
/// the section lookup so the byte-level parser -- the part that faces
/// attacker-controlled input -- can be tested directly.
///
/// Returns the decoded pairs and a flag that is `true` when the section was
/// **malformed** and parsing stopped early. A malformed section is an explicit
/// invalid-metadata outcome, not a silent partial success: the caller raises a
/// provenance caveat so a reviewer sees the stack evidence is degraded rather
/// than reading a truncated list as complete (#247).
///
/// Fails closed on a malformed ULEB128: a `u64` size needs at most ten 7-bit
/// groups, so a continuation sequence that would push the shift to or past 64
/// is overlong, and even at a valid shift the final group's payload can carry
/// bits that do not fit. Left unchecked, `<< shift` panics in debug and wraps
/// in release, and a truncated value silently understates a frame -- making an
/// unbounded stack look bounded, the exact fail-open #247 forbids. On any of
/// these the section has lost framing, so parsing stops: the in-progress pair
/// is dropped, the pairs decoded before it are returned, `malformed` is set,
/// and functions past the corruption fall back to prologue estimation.
fn parse_stack_sizes(data: &[u8], addr_len: usize, le: bool) -> (Vec<(u64, u64)>, bool) {
    let mut out = Vec::new();
    let mut i = 0;
    while i + addr_len <= data.len() {
        let mut addr: u64 = 0;
        for k in 0..addr_len {
            let b = data[i + k] as u64;
            addr |= if le { b << (8 * k) } else { b << (8 * (addr_len - 1 - k)) };
        }
        i += addr_len;
        let mut size: u64 = 0;
        let mut shift = 0u32;
        loop {
            if i >= data.len() {
                // The section ended in the middle of a size: the final entry
                // is truncated, which is malformed metadata.
                return (out, true);
            }
            // A 64-bit value is at most ten groups (shifts 0,7,…,63). Any byte
            // that would shift by 64 or more is overlong: stop rather than
            // panic or wrap. The in-progress (addr, size) is dropped, not
            // pushed, because its size cannot be trusted.
            if shift >= 64 {
                return (out, true);
            }
            let b = data[i];
            i += 1;
            let payload = (b & 0x7f) as u64;
            // A valid shift is not a valid value: the last admissible group
            // (shift 63) has one value bit left, so a payload wider than that
            // overflows u64 and Rust would silently drop the high bits --
            // accepting an oversized frame as a smaller one, the exact
            // fail-open #247 forbids. Reject any group whose payload would not
            // fit above `shift`. `shift > 0` guards the `>> 64` at shift 0,
            // where all 64 bits are available and a 7-bit payload always fits.
            if shift > 0 && payload >> (64 - shift) != 0 {
                return (out, true);
            }
            size |= payload << shift;
            shift += 7;
            if b & 0x80 == 0 {
                break;
            }
        }
        out.push((addr, size));
    }
    // Trailing bytes too few for another address are a partial final entry:
    // the section did not end on an entry boundary, so it is malformed. An
    // empty section (nothing to consume) and an exactly-consumed one both
    // leave `i == data.len()` and are clean.
    (out, i != data.len())
}

/// Mach-O symbols carry no size; derive it from the next symbol or the end of
/// the containing section. `symbols` is sorted by address; walking backward
/// tracks the next strictly greater address in O(N) and binary-searches
/// `sections` in O(log S), avoiding O(N²) CPU DoS when many zero-sized symbols
/// share an address (#9.11).
fn fill_missing_sizes(symbols: &mut [TextSymbol], file: &object::File, kind: object::SectionKind) {
    let mut sections: Vec<(u64, u64)> = file
        .sections()
        .filter(|s| s.kind() == kind || (kind == object::SectionKind::Data && is_data_section(s)))
        .map(|s| (s.address(), s.address().saturating_add(s.size())))
        .filter(|(a, e)| a < e)
        .collect();
    sections.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(sections.len());
    for (a, e) in sections {
        // Only merge genuinely overlapping sections (`a < last.1`): adjacent
        // sections (`a == last.1`) must keep their distinct end boundary so
        // the last symbol of one section does not extend into the next.
        match merged.last_mut() {
            Some(last) if a < last.1 => last.1 = last.1.max(e),
            _ => merged.push((a, e)),
        }
    }
    fill_missing_sizes_in(symbols, &merged);
}

fn fill_missing_sizes_in(symbols: &mut [TextSymbol], merged_sections: &[(u64, u64)]) {
    let n = symbols.len();
    let mut next_addr: Option<u64> = None;
    let mut cur_group_addr: Option<u64> = None;
    for i in (0..n).rev() {
        let addr = symbols[i].address;
        if let Some(g) = cur_group_addr {
            if addr < g {
                next_addr = Some(g);
                cur_group_addr = Some(addr);
            }
        } else {
            cur_group_addr = Some(addr);
        }
        if symbols[i].size != 0 {
            continue;
        }
        let sec_end = {
            let k = merged_sections.partition_point(|&(a, _)| a <= addr);
            (k > 0 && addr < merged_sections[k - 1].1).then(|| merged_sections[k - 1].1)
        };
        let end = match (next_addr, sec_end) {
            (Some(nx), Some(e)) => nx.min(e),
            (Some(nx), None) => nx,
            (None, Some(e)) => e,
            (None, None) => addr,
        };
        symbols[i].size = end.saturating_sub(addr);
    }
}

fn find_dsym(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let dsym = path.with_extension("dSYM");
    let candidate = dsym.join("Contents/Resources/DWARF").join(name);
    if candidate.exists() {
        return Some(candidate);
    }
    let alt = PathBuf::from(format!("{}.dSYM", path.display())).join("Contents/Resources/DWARF").join(name);
    alt.exists().then_some(alt)
}

pub(crate) struct DwarfSizes {
    pub debug_line: usize,
    pub debug_info: usize,
    pub debug_ranges: usize,
}

pub(crate) type LoadedDwarf<'a> = (gimli::DwarfSections<Cow<'a, [u8]>>, gimli::RunTimeEndian, DwarfSizes);

/// Load all DWARF sections using `uncompressed_data()`, so compressed
/// (`SHF_COMPRESSED`) ELF sections fail cleanly rather than feeding raw
/// compressed bytes to `gimli` (#9.11).
pub(crate) fn dwarf_sections_of<'a>(file: &'a object::File<'a>) -> Result<LoadedDwarf<'a>> {
    let endian = if file.is_little_endian() { gimli::RunTimeEndian::Little } else { gimli::RunTimeEndian::Big };
    let mut sizes = DwarfSizes { debug_line: 0, debug_info: 0, debug_ranges: 0 };
    let load_section = |id: gimli::SectionId| -> Result<Cow<'a, [u8]>> {
        // Mach-O DWARF sections are named "__debug_info" inside "__DWARF".
        let name = id.name();
        let sect = file.section_by_name(name).or_else(|| file.section_by_name(&format!("__{}", name.trim_start_matches('.'))));
        let bytes = match sect {
            Some(s) => s.uncompressed_data()?,
            None => Cow::Borrowed(&[][..]),
        };
        match id {
            gimli::SectionId::DebugLine => sizes.debug_line = bytes.len(),
            gimli::SectionId::DebugInfo => sizes.debug_info = bytes.len(),
            gimli::SectionId::DebugRanges | gimli::SectionId::DebugRngLists => sizes.debug_ranges = sizes.debug_ranges.saturating_add(bytes.len()),
            _ => {}
        }
        Ok(bytes)
    };
    let sections = gimli::DwarfSections::load(load_section)?;
    Ok((sections, endian, sizes))
}

fn load_line_rows(file: &object::File) -> Result<Vec<LineRow>> {
    let (sections, endian, sizes) = dwarf_sections_of(file)?;
    let dwarf = sections.borrow(|section| gimli::EndianSlice::new(section, endian));
    load_line_rows_from_dwarf(&dwarf, sizes.debug_line)
}

fn load_line_rows_from_dwarf(dwarf: &gimli::Dwarf<gimli::EndianSlice<'_, gimli::RunTimeEndian>>, debug_line_len: usize) -> Result<Vec<LineRow>> {
    // At most 1 opcode per byte in `.debug_line` can emit a row; capping at
    // 2x section bytes (with a generous floor for tiny tables) guards against
    // pathological line-program amplification (#9.3).
    let max_rows = debug_line_len.saturating_mul(2).clamp(65_536, 16_000_000);
    let mut rows = Vec::new();
    let mut intern: HashMap<String, Arc<str>> = HashMap::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        let Some(program) = unit.line_program.clone() else { continue };
        let comp_dir = unit.comp_dir.map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        let header = program.header().clone();
        let mut file_cache: HashMap<u64, Arc<str>> = HashMap::new();
        let mut iter = program.rows();
        while let Some((_, row)) = iter.next_row()? {
            if rows.len() >= max_rows {
                bail!("DWARF line table exceeds budget ({max_rows} rows)");
            }
            let file = match file_cache.get(&row.file_index()) {
                Some(f) => f.clone(),
                None => {
                    let s = row
                        .file(&header)
                        .map(|fe| {
                            let mut p = PathBuf::new();
                            let dir = fe.directory(&header).and_then(|d| dwarf.attr_string(&unit, d).ok()).map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                            let name = dwarf.attr_string(&unit, fe.path_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            if !Path::new(&dir).is_absolute() {
                                p.push(&comp_dir);
                            }
                            p.push(dir);
                            p.push(name);
                            normalize(&p)
                        })
                        .unwrap_or_default();
                    let arc = match intern.get(&s) {
                        Some(hit) => hit.clone(),
                        None => {
                            let arc: Arc<str> = Arc::from(s.as_str());
                            intern.insert(s, arc.clone());
                            arc
                        }
                    };
                    file_cache.insert(row.file_index(), arc.clone());
                    arc
                }
            };
            rows.push(LineRow { address: row.address(), file, line: row.line().map(|l| l.get() as u32).unwrap_or(0), is_stmt: row.is_stmt(), end_sequence: row.end_sequence() });
        }
    }
    Ok(rows)
}

/// Walk every compilation unit for `DW_TAG_inlined_subroutine` entries.
fn load_inlines(file: &object::File) -> Result<DebugInfo> {
    let (sections, endian, sizes) = dwarf_sections_of(file)?;
    let budget_bytes = sizes.debug_info.saturating_add(sizes.debug_ranges);
    let dwarf = sections.borrow(|section| gimli::EndianSlice::new(section, endian));
    load_inlines_from_dwarf(&dwarf, budget_bytes)
}

fn load_inlines_from_dwarf<'a>(dwarf: &gimli::Dwarf<gimli::EndianSlice<'a, gimli::RunTimeEndian>>, budget_bytes: usize) -> Result<DebugInfo> {
    // Cap total expanded inline ranges so many tiny DIEs referencing the same
    // large `.debug_ranges` list cannot amplify into gigabytes (#9.3).
    let max_ranges = budget_bytes.clamp(65_536, 8_000_000);
    let mut total_ranges = 0usize;
    let mut out = Vec::new();
    let mut tus: Vec<(u64, Arc<str>)> = Vec::new();
    let mut noreturn: Vec<(u64, Arc<str>)> = Vec::new();
    let mut intern: HashMap<&'a [u8], Arc<str>> = HashMap::new();
    let mut intern_str = |s: gimli::EndianSlice<'a, gimli::RunTimeEndian>| -> Arc<str> {
        let bytes = s.slice();
        if let Some(hit) = intern.get(bytes) {
            return hit.clone();
        }
        let arc: Arc<str> = Arc::from(String::from_utf8_lossy(bytes));
        intern.insert(bytes, arc.clone());
        arc
    };
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        // the unit's own source path: the translation unit that emitted
        // every subprogram below, and the disambiguator for a name two
        // units both define (#36)
        let tu = unit.name.map(&mut intern_str).filter(|n| !n.is_empty());
        let mut entries = unit.entries();
        let mut depth: isize = 0;
        let mut inline_depth: Vec<isize> = Vec::new();
        let mut subprogram: Option<(isize, u64)> = None;
        while let Some((delta, entry)) = entries.next_dfs()? {
            depth += delta;
            while inline_depth.last().map(|d| *d >= depth).unwrap_or(false) {
                inline_depth.pop();
            }
            if subprogram.map(|(d, _)| depth <= d).unwrap_or(false) {
                subprogram = None;
            }
            if entry.tag() == gimli::DW_TAG_subprogram {
                let low = match entry.attr_value(gimli::DW_AT_low_pc)? {
                    Some(gimli::AttributeValue::Addr(a)) => Some(a),
                    Some(gimli::AttributeValue::DebugAddrIndex(i)) => dwarf.address(&unit, i).ok(),
                    _ => None,
                };
                if let Some(a) = low {
                    subprogram = Some((depth, a));
                    if let Some(tu) = &tu {
                        tus.push((a, tu.clone()));
                    }
                    // The producer's own `DW_AT_noreturn`. A declaration
                    // with no code has `low_pc` 0 and is skipped above: a
                    // call can only be resolved to a definition that is in
                    // this image, and `0` is every such declaration's
                    // address, so admitting it would match every call
                    // whose target could not be read.
                    if a != 0 && matches!(entry.attr_value(gimli::DW_AT_noreturn)?, Some(v) if truthy(&v)) {
                        let name = entry
                            .attr_value(gimli::DW_AT_linkage_name)?
                            .or(entry.attr_value(gimli::DW_AT_name)?)
                            .and_then(|n| dwarf.attr_string(&unit, n).ok())
                            .map(&mut intern_str)
                            .unwrap_or_else(|| Arc::from(""));
                        noreturn.push((a, name));
                    }
                }
                continue;
            }
            if entry.tag() != gimli::DW_TAG_inlined_subroutine {
                continue;
            }
            // name via abstract origin (possibly chained through specification)
            let mut origin: Option<Arc<str>> = None;
            if let Some(gimli::AttributeValue::UnitRef(off)) = entry.attr_value(gimli::DW_AT_abstract_origin)? {
                let mut cur = Some(off);
                let mut hops = 0;
                while let Some(o) = cur.take() {
                    hops += 1;
                    if hops > 4 {
                        break;
                    }
                    let e = unit.entry(o)?;
                    if let Some(n) = e.attr_value(gimli::DW_AT_linkage_name)?.or(e.attr_value(gimli::DW_AT_name)?) {
                        if let Ok(sname) = dwarf.attr_string(&unit, n) {
                            origin = Some(intern_str(sname));
                            break;
                        }
                    }
                    cur = match e.attr_value(gimli::DW_AT_specification)?.or(e.attr_value(gimli::DW_AT_abstract_origin)?) {
                        Some(gimli::AttributeValue::UnitRef(o2)) => Some(o2),
                        _ => None,
                    };
                }
            }
            let Some(origin) = origin.filter(|s| !s.is_empty()) else { continue };
            let call_line = match entry.attr_value(gimli::DW_AT_call_line)? {
                Some(gimli::AttributeValue::Udata(v)) => Some(v as u32),
                Some(gimli::AttributeValue::Data1(v)) => Some(v as u32),
                Some(gimli::AttributeValue::Data2(v)) => Some(v as u32),
                Some(gimli::AttributeValue::Data4(v)) => Some(v),
                _ => None,
            };
            let mut ranges = Vec::new();
            let mut it = dwarf.die_ranges(&unit, entry)?;
            while let Some(r) = it.next()? {
                if r.end > r.begin {
                    total_ranges += 1;
                    if total_ranges > max_ranges {
                        bail!("DWARF inline ranges exceed budget ({max_ranges} entries)");
                    }
                    ranges.push((r.begin, r.end));
                }
            }
            out.push(InlineRec { origin, call_line, ranges, depth: inline_depth.len() as u32, subprogram_low: subprogram.map(|(_, a)| a) });
            inline_depth.push(depth);
        }
    }
    tus.sort();
    tus.dedup();
    // An address two units both claim -- identical code folding can merge
    // two bodies into one -- is evidence of nothing, so drop it rather than
    // pick a side. After the dedup, a repeated address names distinct units.
    let mut unique = Vec::with_capacity(tus.len());
    let mut i = 0;
    while i < tus.len() {
        let same = tus[i..].iter().take_while(|(a, _)| *a == tus[i].0).count();
        if same == 1 {
            unique.push(tus[i].clone());
        }
        i += same;
    }
    noreturn.sort();
    noreturn.dedup_by_key(|(a, _)| *a);
    Ok((out, unique, noreturn))
}

/// Is a `DW_AT_noreturn` attribute set? DWARF 4 lets a flag be `DW_FORM_flag`
/// with a byte value, DWARF 5 adds `DW_FORM_flag_present` with no value at
/// all; gimli surfaces those as `Flag` and as `Udata`/`Data1` depending on
/// the producer. Anything that is not recognisably false is treated as false,
/// so an attribute read wrongly leaves the fall-through edge in place rather
/// than deleting control flow on a misread byte.
fn truthy<R: gimli::Reader>(v: &gimli::AttributeValue<R>) -> bool {
    match v {
        gimli::AttributeValue::Flag(b) => *b,
        gimli::AttributeValue::Udata(n) => *n != 0,
        gimli::AttributeValue::Data1(n) => *n != 0,
        _ => false,
    }
}

/// Lexically normalise `a/./b/../c` without touching the filesystem.
pub fn normalize(p: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::RootDir => parts.push(String::new()),
            other => parts.push(other.as_os_str().to_string_lossy().into_owned()),
        }
    }
    let s = parts.join("/");
    if s.is_empty() {
        "/".into()
    } else {
        s
    }
}

impl Binary {
    /// The source file of the translation unit whose subprogram starts at
    /// exactly `addr`, when the DWARF records one.
    pub fn tu_of(&self, addr: u64) -> Option<&str> {
        self.subprogram_tus.binary_search_by_key(&addr, |(a, _)| *a).ok().map(|i| self.subprogram_tus[i].1.as_ref())
    }

    /// The name of the subprogram starting at exactly `addr`, when this
    /// image's DWARF marks it `DW_AT_noreturn`. Exact match only: a call
    /// lands on a function's entry, and matching anything else would be a
    /// guess (#210).
    pub fn noreturn_at(&self, addr: u64) -> Option<&str> {
        self.noreturn_subprograms.binary_search_by_key(&addr, |(a, _)| *a).ok().map(|i| self.noreturn_subprograms[i].1.as_ref())
    }

    /// The line-table row in effect at `addr`, if any.
    /// `len` bytes of the image at virtual address `addr`, when they lie
    /// inside one section that has bytes in the file. Used to read switch
    /// jump tables (#171).
    pub fn read_at(&self, addr: u64, len: usize) -> Option<Vec<u8>> {
        // Only loaded sections carry a meaningful virtual address; a
        // non-allocated section (e.g. ELF `.symtab` at nominal address 0 with a
        // large size) would otherwise spuriously contain the address and hand
        // back symbol-table bytes for a `.rodata` table (#171 review). Loaded
        // sections have disjoint address ranges, so the match is unambiguous.
        let s = self.sections.iter().find(|s| s.alloc && addr >= s.address && addr.checked_add(len as u64).is_some_and(|e| e <= s.address.saturating_add(s.size)))?;
        let (off, size) = s.file_range?;
        let rel = addr - s.address;
        if rel.checked_add(len as u64)? > size {
            return None;
        }
        let start = usize::try_from(off.checked_add(rel)?).ok()?;
        self.bytes.get(start..start.checked_add(len)?).map(<[u8]>::to_vec)
    }

    /// The pointer-sized data word at `addr`, whose bytes in the file read
    /// `raw`: the RELATIVE relocation's value when the slot has one, since
    /// a PIE's file holds 0 there (#325 G1), else `raw`.
    pub fn pointer_at(&self, addr: u64, raw: u64) -> u64 {
        self.relative_words.get(&addr).copied().unwrap_or(raw)
    }

    pub fn line_at(&self, addr: u64) -> Option<&LineRow> {
        let idx = self.line_rows.partition_point(|r| r.address <= addr);
        if idx == 0 {
            return None;
        }
        let r = &self.line_rows[idx - 1];
        // Landing on a terminator means `addr` is past the end of a sequence
        // and no new one has started: the address has no source line.
        (!r.end_sequence).then_some(r)
    }

    pub fn instrs_in(&self, start: u64, end: u64) -> &[RawInstr] {
        let lo = self.instrs.partition_point(|i| i.addr < start);
        let hi = self.instrs.partition_point(|i| i.addr < end);
        &self.instrs[lo..hi]
    }
}

/// Locate an LLVM tool: explicit dir, `BUTTERFLY_LLVM_BIN`, the rustup
/// toolchain's `llvm-tools`, then `PATH`.
///
/// A directory the operator configured -- `--llvm-bin`, or
/// `BUTTERFLY_LLVM_BIN` when there is no flag -- is the answer or the error.
/// It used to be one candidate among many, so a pinned directory that did
/// not hold the tool quietly fell through to a rustup toolchain or `PATH`,
/// and the evidence recorded a disassembler nobody chose (#246).
pub fn find_llvm_tool(name: &str, llvm_bin: Option<&Path>) -> Result<PathBuf> {
    let env = std::env::var_os("BUTTERFLY_LLVM_BIN");
    select_llvm_tool(name, llvm_bin, env.as_deref(), || discovered_llvm_candidates(name), std::env::var_os("PATH").as_deref())
}

/// [`find_llvm_tool`] with its inputs explicit, so it can be tested.
fn select_llvm_tool(
    name: &str,
    explicit: Option<&Path>,
    env: Option<&std::ffi::OsStr>,
    discovered: impl FnOnce() -> Vec<PathBuf>,
    path_var: Option<&std::ffi::OsStr>,
) -> Result<PathBuf> {
    let configured = explicit.map(|d| (d.to_path_buf(), "--llvm-bin")).or_else(|| env.filter(|e| !e.is_empty()).map(|e| (PathBuf::from(e), "BUTTERFLY_LLVM_BIN")));
    if let Some((dir, how)) = configured {
        let cand = dir.join(name);
        if cand.is_file() {
            return Ok(std::path::absolute(&cand).unwrap_or(cand));
        }
        bail!("{how} names {}, which has no {name}. butterfly uses the tool you configured or none: it does not fall back to another copy.", dir.display());
    }
    match pick_llvm_tool(&discovered(), path_var, name) {
        Some(p) => Ok(p),
        None => bail!("cannot find {name}; set BUTTERFLY_LLVM_BIN or `rustup component add llvm-tools`"),
    }
}

/// Where rustup keeps LLVM tools: the active toolchain's sysroot first, then
/// every installed toolchain.
fn discovered_llvm_candidates(name: &str) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    // rustc resolved on PATH and run as a bounded probe, like every helper
    // (#246)
    if let Some(rustc) = crate::provenance::resolve_on_path("rustc") {
        let args = [std::ffi::OsStr::new("--print"), std::ffi::OsStr::new("sysroot")];
        if let Ok(out) = crate::helper::run(&rustc, &args, None, crate::helper::Bounds::probe()) {
            let sysroot = String::from_utf8_lossy(&out).trim().to_string();
            if let Ok(dir) = std::fs::read_dir(Path::new(&sysroot).join("lib/rustlib")) {
                for e in dir.flatten() {
                    candidates.push(e.path().join("bin").join(name));
                }
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        if let Ok(dir) = std::fs::read_dir(Path::new(&home).join(".rustup/toolchains")) {
            for tc in dir.flatten() {
                if let Ok(rl) = std::fs::read_dir(tc.path().join("lib/rustlib")) {
                    for e in rl.flatten() {
                        candidates.push(e.path().join("bin").join(name));
                    }
                }
            }
        }
    }
    candidates
}

/// The lookup behind [`find_llvm_tool`], with its inputs explicit so it can be
/// tested. The returned path is what runs *and* what provenance records
/// (#118), so it must be the tool the OS would execute, as an unambiguous
/// absolute path:
///
/// - a discovered candidate (a rustup toolchain) is taken as soon as it
///   exists (a configured directory never reaches here: see
///   [`select_llvm_tool`]);
/// - `PATH` is searched the way execution searches it, by the same resolver
///   that finds `git` (`provenance::resolve_in`): a non-executable file in an
///   earlier entry is skipped rather than chosen and then failing with
///   "Permission denied", and a relative entry is made absolute.
fn pick_llvm_tool(candidates: &[PathBuf], path_var: Option<&std::ffi::OsStr>, name: &str) -> Option<PathBuf> {
    if let Some(c) = candidates.iter().find(|c| c.is_file()) {
        return Some(std::path::absolute(c).unwrap_or_else(|_| c.clone()));
    }
    crate::provenance::resolve_in(path_var?, name)
}

/// Mach-O: `llvm-objdump --macho` names stub targets ("symbol stub for:
/// _printf") where the generic mode only guesses the nearest symbol. Harvest
/// those per call address and patch them into the generic listing.
fn macho_stub_targets(tool: &Path, bytes: &[u8]) -> HashMap<u64, String> {
    let mut map = HashMap::new();
    let args: [&std::ffi::OsStr; 5] = ["--macho".as_ref(), "-d".as_ref(), "--no-show-raw-insn".as_ref(), "--print-imm-hex".as_ref(), "-".as_ref()];
    let Ok(out) = crate::helper::run(tool, &args, Some(bytes.to_vec()), crate::helper::Bounds::standard()) else { return map };
    for line in String::from_utf8_lossy(&out).lines() {
        let Some(i) = line.find("symbol stub for:") else { continue };
        let sym = line[i + 16..].trim().trim_end_matches(';').trim().to_string();
        let t = line.trim_start();
        let addr_s = t.split([':', '\t']).next().unwrap_or("");
        if let Ok(a) = u64::from_str_radix(addr_s, 16) {
            map.insert(a, sym);
        }
    }
    map
}

/// Disassemble `bytes` -- the image as it was read, hashed and parsed --
/// handed to `llvm-objdump` on stdin (`-`). Naming the path instead had the
/// tool open it again, so a file replaced in between was disassembled under
/// the other one's hash and symbols (#287). `path` only names it in errors.
/// The copy for the pipe is bounded by the object cap already applied.
fn disassemble(tool: &Path, bytes: &[u8], path: &Path, macho: bool) -> Result<Vec<RawInstr>> {
    let stubs = if macho { macho_stub_targets(tool, bytes) } else { HashMap::new() };
    // In an unlinked object a call's target field is a placeholder and the
    // real callee lives in a relocation, so `--reloc` asks for them; for a
    // linked image it prints nothing extra. Bounded like every helper
    // (#246): a disassembly that will not end stops at the ceiling.
    let args: [&std::ffi::OsStr; 5] = ["-d".as_ref(), "--no-show-raw-insn".as_ref(), "--print-imm-hex".as_ref(), "--reloc".as_ref(), "-".as_ref()];
    let out = match crate::helper::run(tool, &args, Some(bytes.to_vec()), crate::helper::Bounds::standard()) {
        Ok(out) => out,
        Err(crate::helper::HelperError::Failed { stderr, .. }) => bail!("llvm-objdump failed: {stderr}"),
        Err(e) => bail!("{e}"),
    };
    let text = String::from_utf8_lossy(&out);
    let instrs = parse_disassembly(&text, &stubs);
    if instrs.is_empty() {
        return Err(anyhow!("llvm-objdump produced no instructions for {}", path.display()));
    }
    Ok(instrs)
}

fn parse_disassembly(text: &str, stubs: &HashMap<u64, String>) -> Vec<RawInstr> {
    let mut instrs: Vec<RawInstr> = Vec::new();
    for line in text.lines() {
        // A relocation line follows the instruction it patches. Fold the
        // symbol into that instruction's operands as `<name>`, the same shape
        // objdump uses for a resolved target, so everything downstream (call
        // graph, stack chains, dead code) sees a normal call.
        if let Some(rel) = parse_reloc_line(line) {
            // Every relocation line is consumed here: it must never reach
            // parse_instr_line, which would read `0:  ARM64_RELOC_…` as an
            // instruction at that address.
            if let Some((addr, sym)) = rel {
                // On RISC (AArch64/ARM/RISC-V) `r_offset == instr.addr`; on
                // x86-64 (`R_X86_64_PLT32`, `X86_64_RELOC_BRANCH`) `r_offset`
                // points to the disp32 operand inside the instruction
                // (`instr.addr + 1` or `+ 2`), so match the immediately
                // preceding instruction spanning `addr` (#9.11).
                if let Some(i) = instrs.iter_mut().rev().take(4).find(|i| i.addr <= addr && addr.saturating_sub(i.addr) < 15) {
                    // objdump prints a placeholder target like
                    // `<_state_set_altitude+0x1c>` for the unrelocated field;
                    // replace it, do not append to it.
                    let mut ops = i.operands.clone();
                    if let Some(c) = ops.find('<') {
                        ops.truncate(c);
                    }
                    i.operands = format!("{} <{}>", ops.trim(), sym);
                }
            }
            continue;
        }
        let Some(mut instr) = parse_instr_line(line) else { continue };
        if let Some(sym) = stubs.get(&instr.addr) {
            let mut ops = instr.operands.clone();
            if let Some(i) = ops.find('<') {
                ops.truncate(i);
            }
            instr.operands = format!("{} <{}>", ops.trim(), sym);
        }
        instrs.push(instr);
    }
    instrs.sort_by_key(|i| i.addr);
    instrs.dedup_by_key(|i| i.addr);
    instrs
}

/// A relocation line printed by `llvm-objdump --reloc`, which follows the
/// instruction it applies to:
/// `\t\t0000000000000034:  ARM64_RELOC_BRANCH26\t_clamp_i32` (Mach-O) or
/// `\t\t34: R_AARCH64_CALL26\tclamp_i32` (ELF).
/// Only branch/call relocations are used: those are the ones whose target the
/// disassembly could not encode. Returns (address, symbol).
/// `Some(None)` = a relocation line that names no callee (a data relocation);
/// it is still consumed so it cannot be mistaken for an instruction.
fn parse_reloc_line(line: &str) -> Option<Option<(u64, String)>> {
    if !line.starts_with('\t') {
        return None;
    }
    let t = line.trim();
    let (addr_s, rest) = t.split_once(':')?;
    if addr_s.is_empty() || !addr_s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut it = rest.split_whitespace();
    let kind = it.next()?;
    let k = kind.to_ascii_uppercase();
    // relocation type names: ELF `R_AARCH64_CALL26`, Mach-O `ARM64_RELOC_BRANCH26`
    if !(k.starts_with("R_") || k.contains("_RELOC_")) {
        return None;
    }
    let addr = match u64::from_str_radix(addr_s, 16) {
        Ok(a) => a,
        Err(_) => return Some(None),
    };
    // Only branch-style relocations name a call target; a data relocation is
    // consumed but not used (see the unlinked-object caveat in ingest).
    if !(k.contains("BRANCH") || k.contains("CALL") || k.contains("JUMP") || k.contains("PLT") || k.contains("PC26") || k.contains("BR26")) {
        return Some(None);
    }
    let Some(sym) = it.next() else { return Some(None) };
    let sym = sym.split(['+', '-']).next().unwrap_or(sym);
    if sym.is_empty() {
        return Some(None);
    }
    Some(Some((addr, sym.to_string())))
}

/// `  100000504:     \tcsel\tw8, w0, w2, lt` (ELF / generic mode) or
/// `0000000100000504\tcsel\tw8, w0, w2, lt` (`--macho` mode) -> (addr, mnemonic, operands).
/// A `; symbol stub for: _printf` comment becomes a `<_printf>` annotation so
/// the call target carries the real symbol.
fn parse_instr_line(line: &str) -> Option<RawInstr> {
    let t = line.trim_start();
    let (addr_s, rest) = match t.split_once(':') {
        Some((a, r)) if !a.is_empty() && a.chars().all(|c| c.is_ascii_hexdigit()) => (a, r),
        _ => {
            // --macho: "<16 hex>\t<mnemonic>\t<operands>"
            let (a, r) = t.split_once('\t')?;
            if a.len() < 8 || !a.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            (a, r)
        }
    };
    let rest = rest.trim();
    if rest.is_empty() || rest.starts_with('<') {
        return None;
    }
    let addr = u64::from_str_radix(addr_s, 16).ok()?;
    let mut it = rest.splitn(2, |c: char| c.is_whitespace());
    let mnemonic = it.next()?.to_string();
    let mut operands = it.next().unwrap_or("").trim().to_string();
    let stub = operands.find("symbol stub for:").map(|i| operands[i + 16..].trim().trim_end_matches(';').trim().to_string());
    // Strip trailing comments ("; =0", "# imm = 0x..", "// ...").
    for marker in [" ;", "\t;", "# imm", " //", ";"] {
        if let Some(i) = operands.find(marker) {
            operands.truncate(i);
        }
    }
    if let Some(stub) = stub {
        // drop any nearest-symbol guess and annotate with the real target
        if let Some(i) = operands.find('<') {
            operands.truncate(i);
        }
        operands = format!("{} <{}>", operands.trim(), stub);
    }
    if mnemonic == "..." || mnemonic.starts_with('.') && mnemonic != ".word" {
        return None;
    }
    Some(RawInstr { addr, mnemonic, operands: operands.trim().to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stack_sizes_decodes_multibyte_uleb() {
        // addr=0x1000 (LE u64), size=300 (ULEB 0xAC 0x02); then addr=0x2000, size=1.
        let data = [0x00, 0x10, 0, 0, 0, 0, 0, 0, 0xac, 0x02, 0x00, 0x20, 0, 0, 0, 0, 0, 0, 0x01];
        assert_eq!(parse_stack_sizes(&data, 8, true), (vec![(0x1000, 300), (0x2000, 1)], false), "a clean section is not malformed");
    }

    #[test]
    fn parse_stack_sizes_rejects_an_overlong_uleb_without_panicking() {
        // #247: a continuation sequence longer than a u64 can hold. Before the
        // fix `<< shift` panicked in debug and wrapped in release; now it stops,
        // returns the pairs decoded before it (here: none), and reports the
        // section malformed so the caller can raise a caveat.
        let mut data = vec![0x00, 0x10, 0, 0, 0, 0, 0, 0]; // addr 0x1000
        data.extend(std::iter::repeat_n(0x80u8, 64)); // 64 continuation bytes, never terminating
        data.push(0x01);
        let (pairs, malformed) = parse_stack_sizes(&data, 8, true);
        assert!(pairs.is_empty(), "overlong ULEB must not yield a (wrapped) size: {pairs:?}");
        assert!(malformed, "an overlong ULEB is an explicit invalid-metadata outcome");
    }

    #[test]
    fn parse_stack_sizes_accepts_the_largest_u64_frame() {
        // u64::MAX encodes as nine 0xff groups then a final 0x01 (its bit 63):
        // a valid value in the last admissible group, which must be kept.
        let mut data = vec![0x00, 0x10, 0, 0, 0, 0, 0, 0]; // addr 0x1000
        data.extend(std::iter::repeat_n(0xffu8, 9));
        data.push(0x01);
        assert_eq!(parse_stack_sizes(&data, 8, true), (vec![(0x1000, u64::MAX)], false));
    }

    #[test]
    fn parse_stack_sizes_rejects_value_overflow_in_the_final_group() {
        // #247: the shift (63) is valid but the payload is not -- 0x02 sets a
        // bit that would land at position 64. The old guard only rejected an
        // overlong *shift*, so `<< 63` silently dropped the high bit and
        // accepted a smaller frame. It must now be dropped and reported
        // malformed, not truncated.
        let mut data = vec![0x00, 0x10, 0, 0, 0, 0, 0, 0]; // addr 0x1000
        data.extend(std::iter::repeat_n(0xffu8, 9));
        data.push(0x02); // one bit too wide for the final group
        let (pairs, malformed) = parse_stack_sizes(&data, 8, true);
        assert!(pairs.is_empty(), "value overflow must be dropped, not truncated: {pairs:?}");
        assert!(malformed);
    }

    #[test]
    fn parse_stack_sizes_reports_a_truncated_final_entry_as_malformed() {
        // The section ends mid-size: the last entry is incomplete.
        let data = vec![0x00, 0x10, 0, 0, 0, 0, 0, 0, 0x80]; // addr, then a continuation byte with no successor
        let (pairs, malformed) = parse_stack_sizes(&data, 8, true);
        assert!(pairs.is_empty());
        assert!(malformed, "a truncated final entry is malformed metadata");
    }

    #[test]
    fn parse_stack_sizes_flags_a_partial_trailing_address() {
        // A complete entry, then trailing bytes too few for another address:
        // the section did not end on an entry boundary.
        let mut data = vec![0x00, 0x10, 0, 0, 0, 0, 0, 0, 0x08]; // addr 0x1000, size 8
        data.extend_from_slice(&[0x00, 0x20, 0, 0]); // only 4 of the next 8 address bytes
        let (pairs, malformed) = parse_stack_sizes(&data, 8, true);
        assert_eq!(pairs, vec![(0x1000, 8)], "the complete entry survives");
        assert!(malformed, "a partial trailing address is malformed metadata");
    }

    #[test]
    fn parse_stack_sizes_flags_a_partial_address_as_the_only_entry() {
        let data = vec![0x00, 0x20, 0, 0]; // 4 of 8 address bytes, nothing decodable
        let (pairs, malformed) = parse_stack_sizes(&data, 8, true);
        assert!(pairs.is_empty());
        assert!(malformed, "a section that is only a partial address is malformed");
    }

    #[test]
    fn parse_stack_sizes_accepts_empty_and_exactly_consumed_sections() {
        assert_eq!(parse_stack_sizes(&[], 8, true), (vec![], false), "an empty section is clean, not malformed");
        // one entry, nothing trailing: exactly consumed
        let data = vec![0x00, 0x10, 0, 0, 0, 0, 0, 0, 0x08];
        assert_eq!(parse_stack_sizes(&data, 8, true), (vec![(0x1000, 8)], false));
    }

    /// Every address width and byte order `stack_sizes_of` can select (from
    /// the object's class and data encoding): ELF32/ELF64, little/big endian.
    const LAYOUTS: [(usize, bool); 4] = [(4, true), (4, false), (8, true), (8, false)];

    /// Encode `addr` as an `addr_len`-byte address in the given byte order.
    fn enc_addr(addr: u64, addr_len: usize, le: bool) -> Vec<u8> {
        let b = addr.to_le_bytes()[..addr_len].to_vec();
        if le {
            b
        } else {
            b.into_iter().rev().collect()
        }
    }

    #[test]
    fn parse_stack_sizes_decodes_known_address_bytes_for_each_width_and_byte_order() {
        // Byte-for-byte what the linker wrote for `leaf` (0x200f4 on 32-bit
        // ARM, 0x210120 on AArch64) followed by its ULEB size, so the byte
        // order is pinned by an independent source, not by `enc_addr`.
        assert_eq!(parse_stack_sizes(&[0xf4, 0x00, 0x02, 0x00, 0x04], 4, true), (vec![(0x200f4, 4)], false));
        assert_eq!(parse_stack_sizes(&[0x00, 0x02, 0x00, 0xf4, 0x04], 4, false), (vec![(0x200f4, 4)], false));
        assert_eq!(parse_stack_sizes(&[0x20, 0x01, 0x21, 0, 0, 0, 0, 0, 0x10], 8, true), (vec![(0x210120, 16)], false));
        assert_eq!(parse_stack_sizes(&[0, 0, 0, 0, 0, 0x21, 0x01, 0x20, 0x10], 8, false), (vec![(0x210120, 16)], false));
    }

    #[test]
    fn parse_stack_sizes_boundaries_hold_for_every_width_and_byte_order() {
        for (w, le) in LAYOUTS {
            let at = |a: u64| enc_addr(a, w, le);
            let tag = format!("{w}-byte {}", if le { "LE" } else { "BE" });
            // A valid two-entry section (multi-byte ULEB), exactly consumed.
            let mut good = at(0x1000);
            good.extend_from_slice(&[0xac, 0x02]); // 300
            good.extend(at(0x2000));
            good.push(0x01);
            assert_eq!(parse_stack_sizes(&good, w, le), (vec![(0x1000, 300), (0x2000, 1)], false), "{tag}: valid");
            assert_eq!(parse_stack_sizes(&[], w, le), (vec![], false), "{tag}: empty is clean");

            // A partial address, as the only entry and after a valid one,
            // for every length short of a whole address.
            for k in 1..w {
                let (p, m) = parse_stack_sizes(&at(0x3000)[..k], w, le);
                assert!(p.is_empty() && m, "{tag}: lone {k}-byte partial address must be malformed");
                let mut d = good.clone();
                d.extend_from_slice(&at(0x3000)[..k]);
                assert_eq!(parse_stack_sizes(&d, w, le), (vec![(0x1000, 300), (0x2000, 1)], true), "{tag}: trailing {k}-byte partial address");
            }

            // A whole address with no size at all, and a size that ends on a
            // continuation byte: both are truncated final entries.
            let mut d = good.clone();
            d.extend(at(0x3000));
            assert_eq!(parse_stack_sizes(&d, w, le), (vec![(0x1000, 300), (0x2000, 1)], true), "{tag}: address with no size");
            d.push(0x80);
            assert_eq!(parse_stack_sizes(&d, w, le), (vec![(0x1000, 300), (0x2000, 1)], true), "{tag}: size ends mid-ULEB");

            // The size is a u64 whatever the address width: the largest value
            // is kept, one bit more is value overflow, and a longer
            // continuation is overlong.
            let mut max = at(0x1000);
            max.extend(std::iter::repeat_n(0xffu8, 9));
            let mut over = max.clone();
            let mut long = max.clone();
            max.push(0x01);
            over.push(0x02);
            long.extend_from_slice(&[0x80, 0x00]);
            assert_eq!(parse_stack_sizes(&max, w, le), (vec![(0x1000, u64::MAX)], false), "{tag}: u64::MAX");
            assert_eq!(parse_stack_sizes(&over, w, le), (vec![], true), "{tag}: value overflow");
            assert_eq!(parse_stack_sizes(&long, w, le), (vec![], true), "{tag}: overlong ULEB");
        }
    }

    #[test]
    fn parse_stack_sizes_keeps_valid_entries_before_a_corrupt_one_and_flags_it() {
        // One good pair, then an overlong size: the good pair survives, the
        // corrupt one is dropped, and the section is reported malformed.
        let mut data = vec![0x00, 0x10, 0, 0, 0, 0, 0, 0, 0x10]; // addr 0x1000, size 16
        data.extend_from_slice(&[0x00, 0x20, 0, 0, 0, 0, 0, 0]); // addr 0x2000
        data.extend(std::iter::repeat_n(0x80u8, 20)); // overlong size, no terminator
        assert_eq!(parse_stack_sizes(&data, 8, true), (vec![(0x1000, 16)], true));
    }

    /// Build a thin Mach-O carrying one `LC_UNIXTHREAD`, so the entry point
    /// has to come out of the register block. `object` 0.36 does not parse
    /// thread state at all, so before this such an image reported no entry.
    /// One `LC_UNIXTHREAD` command carrying `pc` in its register block.
    fn thread_cmd(is64: bool, flavor: u32, count: u32, pc_off: usize, pc: u64) -> Vec<u8> {
        let mut state = vec![0u8; (count as usize) * 4];
        if is64 {
            state[pc_off..pc_off + 8].copy_from_slice(&pc.to_le_bytes());
        } else {
            state[pc_off..pc_off + 4].copy_from_slice(&(pc as u32).to_le_bytes());
        }
        let mut cmd = Vec::new();
        cmd.extend_from_slice(&object::macho::LC_UNIXTHREAD.to_le_bytes());
        cmd.extend_from_slice(&((16 + state.len()) as u32).to_le_bytes());
        cmd.extend_from_slice(&flavor.to_le_bytes());
        cmd.extend_from_slice(&count.to_le_bytes());
        cmd.extend_from_slice(&state);
        cmd
    }

    /// One `LC_MAIN` carrying `entryoff`.
    fn main_cmd(entryoff: u64) -> Vec<u8> {
        let mut cmd = Vec::new();
        cmd.extend_from_slice(&object::macho::LC_MAIN.to_le_bytes());
        cmd.extend_from_slice(&24u32.to_le_bytes());
        cmd.extend_from_slice(&entryoff.to_le_bytes());
        cmd.extend_from_slice(&0u64.to_le_bytes()); // stacksize
        cmd
    }

    /// A thin Mach-O carrying `cmds` in the order given.
    fn image(is64: bool, cputype: u32, cmds: &[Vec<u8>]) -> Vec<u8> {
        let cmd: Vec<u8> = cmds.concat();
        let ncmds = cmds.len() as u32;
        let mut out = Vec::new();
        out.extend_from_slice(&(if is64 { object::macho::MH_MAGIC_64 } else { object::macho::MH_MAGIC }).to_le_bytes());
        out.extend_from_slice(&cputype.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&object::macho::MH_EXECUTE.to_le_bytes());
        out.extend_from_slice(&ncmds.to_le_bytes());
        out.extend_from_slice(&(cmd.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        if is64 {
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out.extend_from_slice(&cmd);
        out
    }

    /// The old single-command shorthand, in terms of the pieces above.
    fn unixthread_image(is64: bool, cputype: u32, flavor: u32, count: u32, pc_off: usize, pc: u64) -> Vec<u8> {
        image(is64, cputype, &[thread_cmd(is64, flavor, count, pc_off, pc)])
    }

    #[test]
    fn a_64_bit_unixthread_entry_is_read_from_the_register_block() {
        // arm64: ARM_THREAD_STATE64, pc is the 33rd u64 of the block
        let img = unixthread_image(true, object::macho::CPU_TYPE_ARM64, 6, 68, 256, 0x1_0000_9abc);
        assert_eq!(macho_entry(&img), (MachoEntry::Address, 0x1_0000_9abc));
        // x86-64: x86_THREAD_STATE64, rip is the 17th u64
        let img = unixthread_image(true, object::macho::CPU_TYPE_X86_64, 4, 42, 128, 0x1_0000_1234);
        assert_eq!(macho_entry(&img), (MachoEntry::Address, 0x1_0000_1234));
    }

    #[test]
    fn a_32_bit_unixthread_entry_is_read_from_the_register_block() {
        // i386: x86_THREAD_STATE32, eip is word 10
        let img = unixthread_image(false, object::macho::CPU_TYPE_X86, 1, 16, 40, 0x0000_2048);
        assert_eq!(macho_entry(&img), (MachoEntry::Address, 0x0000_2048));
        // arm: ARM_THREAD_STATE, pc is word 15
        let img = unixthread_image(false, object::macho::CPU_TYPE_ARM, 1, 17, 60, 0x0000_8000);
        assert_eq!(macho_entry(&img), (MachoEntry::Address, 0x0000_8000));
    }

    /// An image carrying both commands, `LC_UNIXTHREAD` first.
    ///
    /// dyld keeps the first of each kind and then prefers `LC_MAIN`, so the
    /// order they appear in must not decide the answer. Returning on the
    /// first match reported the thread-state PC here (review of #8).
    #[test]
    fn lc_main_wins_over_a_thread_state_that_comes_first() {
        let thread = thread_cmd(true, 6, 68, 256, 0x1_0000_9abc);
        let main = main_cmd(2064);
        let arm64 = object::macho::CPU_TYPE_ARM64;

        // UNIXTHREAD first: LC_MAIN must still win
        let img = image(true, arm64, &[thread.clone(), main.clone()]);
        assert_eq!(macho_entry(&img), (MachoEntry::FileOffset, 2064));
        // and the other order agrees
        let img = image(true, arm64, &[main, thread.clone()]);
        assert_eq!(macho_entry(&img), (MachoEntry::FileOffset, 2064));
        // with no LC_MAIN, the thread state is still what is used
        let img = image(true, arm64, &[thread]);
        assert_eq!(macho_entry(&img), (MachoEntry::Address, 0x1_0000_9abc));
    }

    /// A thread-state entry is already a virtual address, so it passes
    /// through unmapped even when the same number also lands inside a
    /// file-offset range -- the ambiguity that made range inference wrong.
    #[test]
    fn a_thread_state_entry_is_never_mapped_through_a_segment() {
        let exec = [ExecSeg { addr: 0x1000, file_off: 0, file_len: 0x4000 }];
        assert_eq!(entry_vaddr_of(0x1000, MachoEntry::Address, false, &exec), 0x1000);
        assert_eq!(entry_vaddr_of(0x1000, MachoEntry::FileOffset, false, &exec), 0x2000);
    }

    /// Flavor numbers are not unique across architectures, and a block can be
    /// truncated. Either way the answer is "no entry", never a number read
    /// from the wrong offset.
    #[test]
    fn an_unreadable_register_block_reports_no_entry() {
        let img = unixthread_image(true, object::macho::CPU_TYPE_ARM64, 4, 42, 128, 0xdead_beef);
        assert_eq!(macho_entry(&img), (MachoEntry::None, 0));
        let mut img = unixthread_image(true, object::macho::CPU_TYPE_ARM64, 6, 68, 256, 0x1234);
        img.truncate(img.len() - 40);
        assert_eq!(macho_entry(&img).0, MachoEntry::None);
    }

    /// `object::File::entry()` gives what the header holds, and for Mach-O
    /// that is `LC_MAIN.entryoff` -- a file offset -- or the `LC_UNIXTHREAD`
    /// program counter -- a virtual address. Which one is read from the load
    /// command, not inferred from the number, because the two can overlap
    /// (review of #8).
    #[test]
    fn a_macho_entry_offset_becomes_a_virtual_address() {
        // examples/c: __TEXT at 0x1_0000_0000 from file offset 0, entryoff
        // 2064, and `_main` at 0x1_0000_0810
        let text = ExecSeg { addr: 0x1_0000_0000, file_off: 0, file_len: 0x4000 };
        assert_eq!(entry_vaddr_of(2064, MachoEntry::FileOffset, false, &[text]), 0x1_0000_0810);

        // a segment whose file offset is not 0
        let later = ExecSeg { addr: 0x1_0000_4000, file_off: 0x4000, file_len: 0x1000 };
        assert_eq!(entry_vaddr_of(0x4010, MachoEntry::FileOffset, false, &[text, later]), 0x1_0000_4010);
    }

    /// The case range-inference got wrong: a low-linked image whose file
    /// offset also lands inside an executable segment's *virtual*-address
    /// range. `__TEXT` maps file 0..0x4000 to virtual 0x1000..0x5000, and
    /// `LC_MAIN.entryoff = 0x1000` is in both ranges. Reading the command
    /// kind resolves it to 0x2000; the old inference returned 0x1000
    /// (review of #8).
    #[test]
    fn a_file_offset_that_also_looks_like_an_address_is_still_mapped() {
        let text = ExecSeg { addr: 0x1000, file_off: 0, file_len: 0x4000 };
        assert_eq!(entry_vaddr_of(0x1000, MachoEntry::FileOffset, false, &[text]), 0x2000, "LC_MAIN is a file offset even here");
        // and read as an LC_UNIXTHREAD address, the same number is left alone
        assert_eq!(entry_vaddr_of(0x1000, MachoEntry::Address, false, &[text]), 0x1000);
    }

    /// `flags_are_executable` is what keeps `__PAGEZERO` -- which spans the
    /// whole low 4GiB -- and data segments out of the offset search, so the
    /// entry maps through code, not through whatever else shares its file
    /// range (#8).
    #[test]
    fn only_executable_segments_are_searched_for_the_entry() {
        use object::SegmentFlags;
        // __PAGEZERO: initprot 0. __TEXT: r-x. __DATA: rw-.
        assert!(!flags_are_executable(SegmentFlags::MachO { flags: 0, maxprot: 0, initprot: 0 }));
        assert!(flags_are_executable(SegmentFlags::MachO { flags: 0, maxprot: 5, initprot: 5 }));
        assert!(!flags_are_executable(SegmentFlags::MachO { flags: 0, maxprot: 3, initprot: 3 }));
        // ELF PT_LOAD: R=4, W=2, X=1
        assert!(flags_are_executable(SegmentFlags::Elf { p_flags: 0x5 }));
        assert!(!flags_are_executable(SegmentFlags::Elf { p_flags: 0x6 }));
        assert!(!flags_are_executable(SegmentFlags::None));

        // the exec list `entry_vaddr` builds excludes the non-exec segments,
        // so only __TEXT is offered to the offset search
        let text = ExecSeg { addr: 0x1_0000_0000, file_off: 0, file_len: 0x4000 };
        assert_eq!(entry_vaddr_of(2064, MachoEntry::FileOffset, false, &[text]), 0x1_0000_0810);
    }

    /// The older Mach-O load command gives an address, not an offset, and is
    /// left untouched.
    #[test]
    fn a_macho_entry_that_is_already_an_address_is_left_alone() {
        let text = ExecSeg { addr: 0x1_0000_0000, file_off: 0, file_len: 0x4000 };
        assert_eq!(entry_vaddr_of(0x1_0000_0810, MachoEntry::Address, false, &[text]), 0x1_0000_0810);
    }

    /// ELF and PE already carry a virtual address; nothing about them changes.
    #[test]
    fn elf_and_pe_entry_points_pass_through_untouched() {
        let text = ExecSeg { addr: 0x40_1000, file_off: 0x1000, file_len: 0x1000 };
        assert_eq!(entry_vaddr_of(0x40_1234, MachoEntry::Address, false, &[text]), 0x40_1234);
        // even one that would look like a file offset if it were Mach-O
        assert_eq!(entry_vaddr_of(0x1234, MachoEntry::Address, false, &[text]), 0x1234);
    }

    /// An unlinked object has no entry point, and neither has an image whose
    /// entry cannot be placed or whose file has no entry command. All report
    /// 0, which is what the caller's `!= 0` test already means.
    #[test]
    fn no_entry_point_is_reported_as_none() {
        let text = ExecSeg { addr: 0x1_0000_0000, file_off: 0, file_len: 0x4000 };
        assert_eq!(entry_vaddr_of(2064, MachoEntry::FileOffset, true, &[text]), 0, "a relocatable .o has no entry");
        assert_eq!(entry_vaddr_of(0, MachoEntry::FileOffset, false, &[text]), 0, "nor has an image whose header says 0");
        assert_eq!(entry_vaddr_of(2064, MachoEntry::None, false, &[text]), 0, "nor a file with neither entry command");
        assert_eq!(entry_vaddr_of(0x9999_9999, MachoEntry::FileOffset, false, &[text]), 0, "an offset in no executable segment cannot be placed");
        assert_eq!(entry_vaddr_of(2064, MachoEntry::FileOffset, false, &[]), 0, "nor can one with no executable segment at all");
    }

    fn row(address: u64, line: u32, end_sequence: bool) -> LineRow {
        LineRow { address, file: "a.rs".into(), line, is_stmt: !end_sequence, end_sequence }
    }

    /// A sequence terminator sharing an address with the first row of the next
    /// sequence must not hide it (regression: whole function prologues were
    /// reported as having no source line, and then counted as
    /// compiler-generated).
    #[test]
    fn line_lookup_survives_abutting_sequences() {
        let mut b = Binary {
            relocatable: false,
            path: PathBuf::new(),
            format: String::new(),
            arch: String::new(),
            is_64: true,
            little_endian: true,
            entry: 0,
            sections: vec![],
            segments: vec![],
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
            bytes: vec![],
            dwarf_bytes: None,
            objdump_path: std::path::PathBuf::from("llvm-objdump"),
            relative_words: Default::default(),
        };
        b.line_rows = vec![
            row(0x1000, 583, false),
            row(0x1100, 106, false), // next sequence starts here ...
            row(0x1100, 583, true),  // ... exactly where the previous ends
            row(0x1120, 107, false),
            row(0x1140, 107, true),
        ];
        b.line_rows.sort_by_key(|r| (r.address, !r.end_sequence));
        assert_eq!(b.line_at(0x1000).map(|r| r.line), Some(583));
        assert_eq!(b.line_at(0x1100).map(|r| r.line), Some(106), "row shadowed by the terminator");
        assert_eq!(b.line_at(0x1110).map(|r| r.line), Some(106));
        assert_eq!(b.line_at(0x1120).map(|r| r.line), Some(107));
        // past the final terminator there is genuinely no line
        assert!(b.line_at(0x1140).is_none());
        assert!(b.line_at(0x2000).is_none());
    }

    /// `read_at` must read the loaded section a virtual address belongs to, not
    /// a non-allocated one (ELF `.symtab` at nominal address 0 with a large
    /// size) that happens to span it (#171 review).
    #[test]
    fn read_at_ignores_non_allocated_sections_that_span_the_address() {
        // file: 0x80 bytes of 0xEE ("symtab" region), then the table's bytes.
        let mut bytes = vec![0xEEu8; 0x80];
        bytes.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        bytes.extend(std::iter::repeat_n(0xBB, 0x3c));
        let sec = |name: &str, address: u64, size: u64, alloc: bool, file_range: Option<(u64, u64)>| SectionInfo {
            name: name.into(),
            address,
            size,
            kind: String::new(),
            flags: if alloc { "A".into() } else { String::new() },
            alloc,
            file_range,
        };
        let mut b = Binary {
            relocatable: false,
            path: PathBuf::new(),
            format: String::new(),
            arch: String::new(),
            is_64: true,
            little_endian: true,
            entry: 0,
            sections: vec![],
            segments: vec![],
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
            objdump_path: std::path::PathBuf::from("llvm-objdump"),
            relative_words: Default::default(),
        };
        // .symtab (not loaded) sits at address 0 and spans VA 0x40 through its
        // 0x100 size, mapping it to the 0xEE region; .rodata (loaded) holds the
        // real table at VA 0x40, mapped to the DEADBEEF bytes. Sorted by
        // address, the non-allocated section comes first.
        b.sections = vec![sec(".symtab", 0, 0x100, false, Some((0, 0x80))), sec(".rodata", 0x40, 0x40, true, Some((0x80, 0x40)))];
        b.sections.sort_by_key(|s| s.address);
        assert_eq!(b.read_at(0x40, 4), Some(vec![0xDE, 0xAD, 0xBE, 0xEF]), "the table is read from the loaded section, not the symbol table that spans the address");
    }

    #[test]
    fn reloc_lines_are_consumed_and_branches_name_their_callee() {
        // Mach-O branch relocation: names the callee
        assert_eq!(parse_reloc_line("\t\t0000000000000034:  ARM64_RELOC_BRANCH26\t_clamp_i32"), Some(Some((0x34, "_clamp_i32".to_string()))));
        // ELF spelling
        assert_eq!(parse_reloc_line("\t\t34: R_AARCH64_CALL26\tclamp_i32"), Some(Some((0x34, "clamp_i32".to_string()))));
        // a data relocation is consumed but names no callee -- crucially it
        // must not fall through to parse_instr_line, which would read
        // "0: ARM64_RELOC_GOT_LOAD_PAGE21" as an instruction at address 0
        assert_eq!(parse_reloc_line("\t\t0000000000000000:  ARM64_RELOC_GOT_LOAD_PAGE21\t_g_state"), Some(None));
        // an ordinary instruction line is not a relocation
        assert_eq!(parse_reloc_line("       0:      \tadrp\tx8, 0x0 <ltmp0>"), None);
    }

    #[test]
    fn parses_objdump_line() {
        let m = parse_instr_line("0000000100000804\tbl\t0x100000988 ; symbol stub for: _printf").unwrap();
        assert_eq!(m.addr, 0x100000804);
        assert_eq!(m.operands, "0x100000988 <_printf>");
        let i = parse_instr_line("100000634:     \tbl\t0x100000514 <_mode_permits_engage>").unwrap();
        assert_eq!(i.addr, 0x100000634);
        assert_eq!(i.mnemonic, "bl");
        assert_eq!(i.operands, "0x100000514 <_mode_permits_engage>");
        let j = parse_instr_line("10000064c:     \tmov\tw0, #0x0                ; =0").unwrap();
        assert_eq!(j.operands, "w0, #0x0");
        assert!(parse_instr_line("0000000100000500 <_clamp_altitude>:").is_none());
        assert!(parse_instr_line("Disassembly of section __TEXT,__text:").is_none());
    }

    // ---- LLVM tool lookup (#118 review: the recorded path must be the tool
    // that runs, as an unambiguous absolute path) ----

    fn lookup_tempdir(tag: &str) -> PathBuf {
        let d = butterfly_testsupport::scratch_dir(&format!("llvmlookup-{tag}"));
        std::fs::canonicalize(&d).unwrap()
    }

    fn write_tool(dir: &Path, name: &str, mode: u32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\necho 'LLVM version 99.0.0'\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    /// `dir` spelled relative to the current directory (`../../..` up to the
    /// root, then down), so a lookup through it exercises relative handling.
    fn relative_to_cwd(dir: &Path) -> PathBuf {
        let cwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        let mut rel = PathBuf::new();
        for _ in cwd.components().skip(1) {
            rel.push("..");
        }
        rel.join(dir.strip_prefix("/").unwrap())
    }

    #[test]
    fn llvm_path_lookup_skips_a_non_executable_earlier_entry() {
        let early = lookup_tempdir("ne-early");
        let late = lookup_tempdir("ne-late");
        write_tool(&early, "llvm-objdump", 0o644); // execution would skip this one
        let exe = write_tool(&late, "llvm-objdump", 0o755);
        let path = std::env::join_paths([&early, &late]).unwrap();
        let got = pick_llvm_tool(&[], Some(&path), "llvm-objdump").expect("the executable one resolves");
        assert_eq!(got, exe, "the non-executable earlier file is not the tool that runs");
    }

    #[test]
    fn llvm_path_lookup_through_a_relative_entry_records_an_absolute_path() {
        let dir = lookup_tempdir("rel-path");
        let exe = write_tool(&dir, "llvm-objdump", 0o755);
        let path = std::env::join_paths([relative_to_cwd(&dir)]).unwrap();
        let got = pick_llvm_tool(&[], Some(&path), "llvm-objdump").expect("resolves through the relative entry");
        assert!(got.is_absolute(), "provenance needs an unambiguous path, got {got:?}");
        assert_eq!(std::fs::canonicalize(&got).unwrap(), exe);
    }

    #[test]
    fn an_explicit_relative_llvm_dir_is_recorded_absolute() {
        let dir = lookup_tempdir("rel-explicit");
        let exe = write_tool(&dir, "llvm-objdump", 0o755);
        let cand = relative_to_cwd(&dir).join("llvm-objdump");
        let got = pick_llvm_tool(&[cand], None, "llvm-objdump").expect("the explicit candidate is used");
        assert!(got.is_absolute(), "got {got:?}");
        assert_eq!(std::fs::canonicalize(&got).unwrap(), exe);
    }

    /// A configured directory is the answer or the error. It used to be one
    /// candidate among several, so a pinned directory without the tool fell
    /// through to whatever rustup or PATH offered (#246).
    #[test]
    fn a_configured_llvm_dir_without_the_tool_is_an_error_not_a_fallback() {
        let empty = lookup_tempdir("cfg-empty");
        let on_path = lookup_tempdir("cfg-path");
        let elsewhere = write_tool(&on_path, "llvm-objdump", 0o755);
        let path = std::env::join_paths([&on_path]).unwrap();
        let discovered = || vec![elsewhere.clone()];

        let e = select_llvm_tool("llvm-objdump", Some(&empty), None, discovered, Some(&path)).unwrap_err().to_string();
        assert!(e.contains("--llvm-bin") && e.contains("does not fall back"), "{e}");
        let e = select_llvm_tool("llvm-objdump", None, Some(empty.as_os_str()), discovered, Some(&path)).unwrap_err().to_string();
        assert!(e.contains("BUTTERFLY_LLVM_BIN"), "{e}");

        // and with nothing configured, discovery and PATH still work
        assert_eq!(select_llvm_tool("llvm-objdump", None, None, discovered, Some(&path)).unwrap(), elsewhere);
    }

    /// The flag outranks the variable: both configured, the flag's directory
    /// is the one used, and the variable's is not consulted.
    #[test]
    fn the_llvm_bin_flag_outranks_the_variable() {
        let flag = lookup_tempdir("cfg-flag");
        let var = lookup_tempdir("cfg-var");
        let want = write_tool(&flag, "llvm-objdump", 0o755);
        write_tool(&var, "llvm-objdump", 0o755);
        assert_eq!(select_llvm_tool("llvm-objdump", Some(&flag), Some(var.as_os_str()), Vec::new, None).unwrap(), want);
    }

    #[test]
    fn an_explicit_llvm_candidate_wins_over_path() {
        let explicit = lookup_tempdir("explicit-wins");
        let on_path = lookup_tempdir("explicit-path");
        let want = write_tool(&explicit, "llvm-objdump", 0o755);
        write_tool(&on_path, "llvm-objdump", 0o755);
        let path = std::env::join_paths([&on_path]).unwrap();
        let got = pick_llvm_tool(std::slice::from_ref(&want), Some(&path), "llvm-objdump").unwrap();
        assert_eq!(got, want);
    }

    /// x86-64 `.o` call/branch relocations sit at `instr.addr + 1` (after the
    /// 1-byte `0xe8`/`0xe9` opcode), not at `instr.addr` (#9.11).
    #[test]
    fn parse_disassembly_attaches_x86_64_and_aarch64_relocs() {
        let listing = "\
0000000000000000 <caller>:\n\
       0:\tcallq\t0x5 <caller+0x5>\n\
\t\t0000000000000001:  R_X86_64_PLT32\tmay_throw-0x4\n\
       5:\tjmp\t0xa <caller+0xa>\n\
\t\t0000000000000006:  X86_64_RELOC_BRANCH\t_tail_callee\n\
      10:\tbl\t0x10 <caller+0x10>\n\
\t\t0000000000000010:  R_AARCH64_CALL26\tarm_callee\n";
        let instrs = parse_disassembly(listing, &HashMap::new());
        assert_eq!(instrs.len(), 3);
        assert_eq!(instrs[0].operands, "0x5 <may_throw>");
        assert_eq!(instrs[1].operands, "0xa <_tail_callee>");
        assert_eq!(instrs[2].operands, "0x10 <arm_callee>");
    }

    /// Many zero-sized alias symbols at one address must finish linearly (#9.11).
    #[test]
    fn fill_missing_sizes_handles_many_aliases_at_one_address() {
        let mut syms: Vec<TextSymbol> = (0..10_000).map(|k| TextSymbol { name: format!("alias_{k}"), address: 0x1000, size: 0 }).collect();
        syms.push(TextSymbol { name: "next".into(), address: 0x1200, size: 0 });
        fill_missing_sizes_in(&mut syms, &[(0x1000, 0x1218), (0x1218, 0x2000)]);
        assert_eq!(syms[0].size, 0x200);
        assert_eq!(syms[9_999].size, 0x200);
        assert_eq!(syms[10_000].size, 0x18, "adjacent section boundary at 0x1218 must be preserved");
    }

    /// `load_discovered` refuses symlinks and multi-link hardlinks (#6.4).
    #[test]
    fn load_discovered_refuses_symlinks_and_hardlinks() {
        let dir = lookup_tempdir("load-discovered");
        let real = dir.join("real.bin");
        std::fs::write(&real, b"not an elf").unwrap();
        let sym = dir.join("sym.bin");
        std::os::unix::fs::symlink(&real, &sym).unwrap();
        let hard = dir.join("hard.bin");
        std::fs::hard_link(&real, &hard).unwrap();
        let lim = butterfly_model::limits::Limits::default();
        assert!(load_discovered(&sym, None, &lim).is_err(), "symlink must be refused");
        assert!(load_discovered(&hard, None, &lim).is_err(), "hardlink (nlink > 1) must be refused");
    }

    /// Compressed (`SHF_COMPRESSED`) ELF `.debug_info` must fail cleanly via
    /// `uncompressed_data()`, never feeding raw compressed bytes to gimli (#9.11).
    #[test]
    fn dwarf_sections_of_rejects_compressed_debug_info() {
        use object::elf::*;
        let mut elf = vec![0u8; 64];
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2; // ELFCLASS64
        elf[5] = 1; // ELFDATA2LSB
        elf[6] = 1;
        elf[16..18].copy_from_slice(&1u16.to_le_bytes()); // ET_REL
        elf[18..20].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
        elf[20..24].copy_from_slice(&1u32.to_le_bytes());
        elf[52..54].copy_from_slice(&64u16.to_le_bytes());
        elf[58..60].copy_from_slice(&64u16.to_le_bytes());
        elf[60..62].copy_from_slice(&3u16.to_le_bytes()); // null + .shstrtab + .debug_info
        elf[62..64].copy_from_slice(&1u16.to_le_bytes()); // e_shstrndx = 1

        let shstr_off = elf.len() as u64;
        let shstr = b"\0.shstrtab\0.debug_info\0";
        elf.extend_from_slice(shstr);
        let dbg_off = elf.len() as u64;
        // Elf64_Chdr (24 bytes) + dummy compressed payload
        elf.extend_from_slice(&1u32.to_le_bytes()); // ELFCOMPRESS_ZLIB
        elf.extend_from_slice(&0u32.to_le_bytes());
        elf.extend_from_slice(&64u64.to_le_bytes()); // ch_size
        elf.extend_from_slice(&1u64.to_le_bytes()); // ch_addralign
        elf.extend_from_slice(&[0x78, 0x9c, 0x03, 0x00]);
        let dbg_size = elf.len() as u64 - dbg_off;

        let shoff = elf.len() as u64;
        elf[40..48].copy_from_slice(&shoff.to_le_bytes());
        let push_shdr = |buf: &mut Vec<u8>, name: u32, ty: u32, flags: u64, off: u64, sz: u64| {
            buf.extend_from_slice(&name.to_le_bytes());
            buf.extend_from_slice(&ty.to_le_bytes());
            buf.extend_from_slice(&flags.to_le_bytes());
            buf.extend_from_slice(&0u64.to_le_bytes());
            buf.extend_from_slice(&off.to_le_bytes());
            buf.extend_from_slice(&sz.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes());
            buf.extend_from_slice(&1u64.to_le_bytes());
            buf.extend_from_slice(&0u64.to_le_bytes());
        };
        push_shdr(&mut elf, 0, SHT_NULL, 0, 0, 0);
        push_shdr(&mut elf, 1, SHT_STRTAB, 0, shstr_off, shstr.len() as u64);
        push_shdr(&mut elf, 11, SHT_PROGBITS, u64::from(SHF_COMPRESSED), dbg_off, dbg_size);

        let file = object::File::parse(&*elf).expect("synthetic ELF64");
        assert!(dwarf_sections_of(&file).is_err(), "SHF_COMPRESSED debug section must fail cleanly");
    }

    /// `load_line_rows_from_dwarf` interns file paths as `Arc<str>` so repeated
    /// rows share a single allocation instead of cloning the full string (#9.3).
    #[test]
    fn line_rows_share_interned_arc_str_paths() {
        // Minimal DWARF v4 .debug_line program emitting two rows for file 1.
        let mut line = Vec::new();
        let mut hdr = vec![
            1,            // minimum_instruction_length
            1,            // maximum_operations_per_instruction
            1,            // default_is_stmt
            (-5i8) as u8, // line_base
            14,           // line_range
            13,           // opcode_base
        ];
        hdr.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]); // std opcode lengths
        hdr.push(0); // include_directories terminator
        hdr.extend_from_slice(b"src/flight.c\0");
        hdr.extend_from_slice(&[0, 0, 0]); // dir, mtime, size
        hdr.push(0); // file_names terminator
        let hdr_len = hdr.len() as u32;
        let prog = vec![
            0, 9, 2, 0x00, 0x10, 0, 0, 0, 0, 0, 0, // DW_LNE_set_address 0x1000
            1, // DW_LNS_copy
            2, 4, // DW_LNS_advance_pc +4
            1, // DW_LNS_copy
            0, 1, 1, // DW_LNE_end_sequence
        ];
        let unit_len = (2 + 4 + hdr.len() + prog.len()) as u32;
        line.extend_from_slice(&unit_len.to_le_bytes());
        line.extend_from_slice(&4u16.to_le_bytes()); // DWARF v4
        line.extend_from_slice(&hdr_len.to_le_bytes());
        line.extend_from_slice(&hdr);
        line.extend_from_slice(&prog);

        // Minimal DWARF v4 .debug_info CU pointing at .debug_line offset 0
        let abbrev = vec![1, 0x11, 0, 0x10, 0x17, 0, 0, 0]; // CU: DW_AT_stmt_list (0x10) DW_FORM_sec_offset (0x17)
        let mut info = Vec::new();
        let body = vec![1, 0, 0, 0, 0]; // abbrev 1, stmt_list = 0
        info.extend_from_slice(&((7 + body.len()) as u32).to_le_bytes());
        info.extend_from_slice(&4u16.to_le_bytes());
        info.extend_from_slice(&0u32.to_le_bytes());
        info.push(8);
        info.extend_from_slice(&body);

        let sections = gimli::DwarfSections::load::<_, std::convert::Infallible>(|id| {
            Ok(match id {
                gimli::SectionId::DebugAbbrev => abbrev.clone(),
                gimli::SectionId::DebugInfo => info.clone(),
                gimli::SectionId::DebugLine => line.clone(),
                _ => Vec::new(),
            })
        })
        .unwrap();
        let dwarf = sections.borrow(|s| gimli::EndianSlice::new(s.as_slice(), gimli::RunTimeEndian::Little));
        let rows = load_line_rows_from_dwarf(&dwarf, line.len()).unwrap();
        assert!(rows.len() >= 2);
        assert!(Arc::ptr_eq(&rows[0].file, &rows[1].file), "rows for the same file must share one Arc<str>");
    }

    /// `load_inlines_from_dwarf` enforces a total range-entry budget so many
    /// inline DIEs referencing one large `.debug_ranges` list fail closed
    /// instead of exhausting memory (#9.3).
    #[test]
    fn inlines_range_budget_fails_closed_on_amplification() {
        // .debug_ranges: 512 range entries (8 KiB) + (0, 0) terminator
        let mut ranges = Vec::new();
        for k in 0..512u64 {
            ranges.extend_from_slice(&(0x1000 + k * 4).to_le_bytes());
            ranges.extend_from_slice(&(0x1004 + k * 4).to_le_bytes());
        }
        ranges.extend_from_slice(&0u64.to_le_bytes());
        ranges.extend_from_slice(&0u64.to_le_bytes());

        // Abbrev 1: CU (children=1), no attrs
        // Abbrev 2: DW_TAG_subprogram (0x2e), no children, DW_AT_name (0x03) DW_FORM_string (0x08)
        // Abbrev 3: DW_TAG_inlined_subroutine (0x1d), no children,
        //           DW_AT_abstract_origin (0x31) DW_FORM_ref4 (0x13),
        //           DW_AT_ranges (0x55) DW_FORM_sec_offset (0x17)
        let abbrev = vec![
            1, 0x11, 1, 0, 0, // CU
            2, 0x2e, 0, 0x03, 0x08, 0, 0, // subprogram (abstract origin)
            3, 0x1d, 0, 0x31, 0x13, 0x55, 0x17, 0, 0, // inlined_subroutine
            0,
        ];
        // Unit offset 11 = CU; offset 12 = subprogram "f\0" (3 bytes); offset 15+ = 200 inlines -> 200*512 = 102,400 > 65,536
        let mut body = vec![1, 2, b'f', 0];
        for _ in 0..200 {
            body.push(3);
            body.extend_from_slice(&12u32.to_le_bytes()); // abstract_origin = 12
            body.extend_from_slice(&0u32.to_le_bytes()); // ranges offset = 0
        }
        body.push(0); // end CU

        let mut info = Vec::new();
        info.extend_from_slice(&((7 + body.len()) as u32).to_le_bytes());
        info.extend_from_slice(&4u16.to_le_bytes());
        info.extend_from_slice(&0u32.to_le_bytes());
        info.push(8);
        info.extend_from_slice(&body);

        let sections = gimli::DwarfSections::load::<_, std::convert::Infallible>(|id| {
            Ok(match id {
                gimli::SectionId::DebugAbbrev => abbrev.clone(),
                gimli::SectionId::DebugInfo => info.clone(),
                gimli::SectionId::DebugRanges => ranges.clone(),
                _ => Vec::new(),
            })
        })
        .unwrap();
        let dwarf = sections.borrow(|s| gimli::EndianSlice::new(s.as_slice(), gimli::RunTimeEndian::Little));
        let budget = info.len() + ranges.len();
        let err = load_inlines_from_dwarf(&dwarf, budget).unwrap_err();
        assert!(err.to_string().contains("exceed budget"), "{err}");
    }
}

#[cfg(test)]
#[path = "req_e_objectcode_tests.rs"]
mod req_e_tests;
