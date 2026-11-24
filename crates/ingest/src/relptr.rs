// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Pointers in the data of a position-independent ELF image (#325 G1).
//!
//! A PIE does not know its load address, so the linker cannot write a
//! pointer to one of its own functions into data. It writes an
//! `R_*_RELATIVE` dynamic relocation instead -- "add the load base to this
//! addend and store it here" -- and, with RELA relocations, leaves the slot
//! itself zero. A fn-pointer table (`TASKS`), a C++ or Rust vtable, an
//! initialiser array: every word of them reads as 0 from the file. The data
//! scans that find which functions are held in data, and so are candidates
//! of an indirect call rather than dead code, read the file's bytes and saw
//! nothing on x86-64 or AArch64 Linux.
//!
//! [`relative_words`] collects, for every RELATIVE relocation of a RELA
//! section, the slot address and the value the slot holds once loaded at
//! the link-time base: its addend. [`crate::objectcode::Binary::pointer_at`]
//! reads a data word through that map.
//!
//! What is read is bounded and fails closed:
//! * only `SHT_RELA` sections the image loads (`SHF_ALLOC`), so the entries
//!   are bounded by the bytes the run's budget already admitted;
//! * only a relocation with no symbol, a non-negative addend, and a slot
//!   that lies whole in a loaded section with bytes in the file -- the word
//!   a data scan would read;
//! * a slot two relocations disagree about is dropped, not guessed;
//! * a malformed relocation section is skipped whole.
//!
//! Nothing else changes meaning: a relocation names no pointer that the
//! scans then trust blindly, it only replaces the zero the file holds, and
//! the scans still accept a value only when it is the start of a function.
//! `SHT_REL` (32-bit x86 and Arm) keeps the addend in the slot, and RELR
//! (`-z pack-relative-relocs`) only lists slots whose bytes already hold
//! the link-time value, so for both the file's bytes are already right.
//! Mach-O chained fixups are decoded elsewhere ([`crate`]'s
//! `pointer_candidates`) and are untouched.

use std::collections::{BTreeMap, BTreeSet};

/// The `R_*_RELATIVE` type for an ELF machine, where RELA is the norm.
pub fn relative_type(arch: object::Architecture) -> Option<u32> {
    use object::elf::*;
    Some(match arch {
        object::Architecture::X86_64 => R_X86_64_RELATIVE,
        object::Architecture::Aarch64 => R_AARCH64_RELATIVE,
        object::Architecture::Riscv64 | object::Architecture::Riscv32 => R_RISCV_RELATIVE,
        object::Architecture::PowerPc64 => R_PPC64_RELATIVE,
        _ => return None,
    })
}

/// One RELA entry: slot offset, type, symbol index, addend.
pub type Rela = (u64, u32, u32, i64);

/// Slot address -> link-time value, from `relas` of an image whose
/// RELATIVE type is `relative`. `slot_ok(addr)` says whether a word at
/// `addr` lies in a loaded section with bytes in the file.
pub fn relative_words(relative: u32, relas: impl IntoIterator<Item = Rela>, slot_ok: impl Fn(u64) -> bool) -> BTreeMap<u64, u64> {
    let mut out = BTreeMap::new();
    let mut disputed = BTreeSet::new();
    for (offset, r_type, sym, addend) in relas {
        if r_type != relative || sym != 0 || !slot_ok(offset) {
            continue;
        }
        let Ok(v) = u64::try_from(addend) else { continue };
        match out.insert(offset, v) {
            Some(old) if old != v => {
                disputed.insert(offset);
            }
            _ => {}
        }
    }
    for d in disputed {
        out.remove(&d);
    }
    out
}

/// The RELATIVE words of a linked ELF image; empty for anything else.
pub fn of_file(file: &object::File, data: &[u8]) -> BTreeMap<u64, u64> {
    use object::Object;
    let Some(relative) = relative_type(file.architecture()) else { return BTreeMap::new() };
    match file {
        object::File::Elf64(f) => elf(f, data, relative),
        object::File::Elf32(f) => elf(f, data, relative),
        _ => BTreeMap::new(),
    }
}

fn elf<'data, E: object::read::elf::FileHeader>(f: &object::read::elf::ElfFile<'data, E, &'data [u8]>, data: &'data [u8], relative: u32) -> BTreeMap<u64, u64> {
    use object::read::elf::{Rela as _, SectionHeader as _};
    let endian = f.endian();
    let mips64el = f.elf_header().is_mips64el(endian);
    let width: u64 = if f.elf_header().is_type_64() { 8 } else { 4 };
    let headers = f.elf_section_table().iter();
    // loaded sections with bytes in the file: merged, disjoint (start, end)
    // ranges so `slot_ok` is O(log sections) rather than O(sections) per
    // relocation.
    let mut loaded: Vec<(u64, u64)> = headers
        .clone()
        .filter(|s| s.sh_flags(endian).into() & u64::from(object::elf::SHF_ALLOC) != 0 && s.sh_type(endian) != object::elf::SHT_NOBITS)
        .map(|s| {
            let start: u64 = s.sh_addr(endian).into();
            let size: u64 = s.sh_size(endian).into();
            (start, start.saturating_add(size))
        })
        .filter(|(s, e)| s < e)
        .collect();
    loaded.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(loaded.len());
    for (s, e) in loaded {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let slot_ok = |a: u64| {
        a.checked_add(width).is_some_and(|end| {
            let k = merged.partition_point(|&(s, _)| s <= a);
            k > 0 && end <= merged[k - 1].1
        })
    };
    // Collect zero-copy slices for SHF_ALLOC SHT_RELA sections, deduplicating
    // overlapping file byte ranges so crafted headers pointing repeatedly at
    // the same file bytes cannot amplify memory or CPU past the file's size.
    let mut sections: Vec<(u64, u64, &'data [E::Rela])> = Vec::new();
    for sh in headers {
        if sh.sh_flags(endian).into() & u64::from(object::elf::SHF_ALLOC) == 0 {
            continue;
        }
        // a malformed section is skipped whole
        let Ok(Some((entries, _))) = sh.rela(endian, data) else { continue };
        if entries.is_empty() {
            continue;
        }
        let off: u64 = sh.sh_offset(endian).into();
        let sz: u64 = sh.sh_size(endian).into();
        sections.push((off, off.saturating_add(sz), entries));
    }
    sections.sort_unstable_by_key(|&(off, end, _)| (off, std::cmp::Reverse(end)));
    let mut covered_to = 0u64;
    let mut non_overlapping: Vec<&'data [E::Rela]> = Vec::with_capacity(sections.len());
    for (off, end, entries) in sections {
        if off < covered_to {
            continue;
        }
        covered_to = end;
        non_overlapping.push(entries);
    }
    let relas = non_overlapping
        .into_iter()
        .flat_map(|entries| entries.iter().map(move |r| (r.r_offset(endian).into(), r.r_type(endian, mips64el), r.r_sym(endian, mips64el), r.r_addend(endian).into())));
    relative_words(relative, relas, slot_ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::elf::*;

    fn in_data(a: u64) -> bool {
        (0x3000..0x3100).contains(&a) && a + 8 <= 0x3100
    }

    #[test]
    fn a_relative_slot_holds_its_addend() {
        // TASKS: three fn pointers in .data.rel.ro, as rust-lld writes them
        let relas = [(0x3000, R_X86_64_RELATIVE, 0, 0x1570), (0x3008, R_X86_64_RELATIVE, 0, 0x15a0), (0x3010, R_X86_64_RELATIVE, 0, 0x15d0)];
        let w = relative_words(R_X86_64_RELATIVE, relas, in_data);
        assert_eq!(w, BTreeMap::from([(0x3000, 0x1570), (0x3008, 0x15a0), (0x3010, 0x15d0)]));
    }

    #[test]
    fn what_is_not_a_plain_relative_slot_is_left_alone() {
        let relas = [
            (0x3000, R_X86_64_GLOB_DAT, 3, 0),       // bound to a symbol: not ours to read
            (0x3008, R_X86_64_64, 4, 0x10),          // symbol + addend
            (0x3010, R_X86_64_RELATIVE, 2, 0x1570),  // RELATIVE never names a symbol
            (0x3018, R_X86_64_RELATIVE, 0, -8),      // a negative link-time address
            (0x2ff8, R_X86_64_RELATIVE, 0, 0x1570),  // outside every loaded section
            (0x30fc, R_X86_64_RELATIVE, 0, 0x1570),  // runs off the end of one
            (0x3020, R_AARCH64_RELATIVE, 0, 0x1570), // another machine's type
        ];
        assert!(relative_words(R_X86_64_RELATIVE, relas, in_data).is_empty());
    }

    #[test]
    fn a_slot_two_relocations_disagree_about_is_dropped() {
        let relas = [
            (0x3000, R_X86_64_RELATIVE, 0, 0x1570),
            (0x3000, R_X86_64_RELATIVE, 0, 0x15a0),
            (0x3000, R_X86_64_RELATIVE, 0, 0x1570),
            (0x3008, R_X86_64_RELATIVE, 0, 0x15d0),
            (0x3008, R_X86_64_RELATIVE, 0, 0x15d0),
        ];
        assert_eq!(relative_words(R_X86_64_RELATIVE, relas, in_data), BTreeMap::from([(0x3008, 0x15d0)]), "a repeated, agreeing entry is kept");
    }

    #[test]
    fn the_relative_type_is_per_machine() {
        assert_eq!(relative_type(object::Architecture::X86_64), Some(R_X86_64_RELATIVE));
        assert_eq!(relative_type(object::Architecture::Aarch64), Some(R_AARCH64_RELATIVE));
        assert_eq!(relative_type(object::Architecture::Arm), None, "SHT_REL: the addend is already in the slot");
        assert_eq!(relative_type(object::Architecture::I386), None);
    }

    /// Overlapping SHF_ALLOC SHT_RELA section headers pointing at the same file
    /// bytes must not amplify memory or CPU beyond the file's size (#9.2).
    #[test]
    fn overlapping_rela_section_headers_are_deduplicated() {
        // Minimal ELF64 with one SHF_ALLOC .data section and 256 SHF_ALLOC
        // SHT_RELA section headers all pointing at the same 100-entry table.
        let mut elf = vec![0u8; 64];
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2; // ELFCLASS64
        elf[5] = 1; // ELFDATA2LSB
        elf[6] = 1; // EV_CURRENT
        elf[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
        elf[18..20].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
        elf[20..24].copy_from_slice(&1u32.to_le_bytes());
        elf[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
        elf[58..60].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize

        let shstr_off = elf.len() as u64;
        elf.push(0); // empty string table
        let rela_off = elf.len() as u64;
        let n_entries = 100u64;
        for i in 0..n_entries {
            elf.extend_from_slice(&(0x3000 + i * 8).to_le_bytes()); // r_offset
            elf.extend_from_slice(&(R_X86_64_RELATIVE as u64).to_le_bytes()); // r_info (sym=0)
            elf.extend_from_slice(&(0x1000 + i * 16).to_le_bytes()); // r_addend
        }
        let rela_size = n_entries * 24;

        let shoff = elf.len() as u64;
        let dup_headers = 256u16;
        let shnum = 3 + dup_headers; // null + .shstrtab + .data + dup SHT_RELA headers
        elf[40..48].copy_from_slice(&shoff.to_le_bytes());
        elf[60..62].copy_from_slice(&shnum.to_le_bytes());
        elf[62..64].copy_from_slice(&1u16.to_le_bytes()); // e_shstrndx = 1

        let push_shdr = |buf: &mut Vec<u8>, sh_type: u32, flags: u64, addr: u64, off: u64, size: u64, entsize: u64| {
            buf.extend_from_slice(&0u32.to_le_bytes()); // sh_name
            buf.extend_from_slice(&sh_type.to_le_bytes());
            buf.extend_from_slice(&flags.to_le_bytes());
            buf.extend_from_slice(&addr.to_le_bytes());
            buf.extend_from_slice(&off.to_le_bytes());
            buf.extend_from_slice(&size.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
            buf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
            buf.extend_from_slice(&8u64.to_le_bytes()); // sh_addralign
            buf.extend_from_slice(&entsize.to_le_bytes());
        };
        push_shdr(&mut elf, SHT_NULL, 0, 0, 0, 0, 0);
        push_shdr(&mut elf, SHT_STRTAB, 0, 0, shstr_off, 1, 0);
        push_shdr(&mut elf, SHT_PROGBITS, u64::from(SHF_ALLOC), 0x3000, rela_off, n_entries * 8, 0);
        for _ in 0..dup_headers {
            push_shdr(&mut elf, SHT_RELA, u64::from(SHF_ALLOC), 0x1000, rela_off, rela_size, 24);
        }

        let file = object::File::parse(&*elf).expect("synthetic ELF64");
        let words = of_file(&file, &elf);
        assert_eq!(words.len(), n_entries as usize);
        assert_eq!(words.get(&0x3000), Some(&0x1000));
        assert_eq!(words.get(&(0x3000 + 99 * 8)), Some(&(0x1000 + 99 * 16)));
    }
}
