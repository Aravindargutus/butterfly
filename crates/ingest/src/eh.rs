// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! C++ exception landing pads, read from the image's unwind data (#165).
//!
//! A landing pad -- the code a `catch`, or a destructor run during
//! unwinding, compiles to -- is not reached by any branch. The unwinder
//! jumps to it, using the language-specific data area (LSDA) the compiler
//! emitted for the function: a call-site table saying, for each range of
//! call instructions, where to land if something they call throws. Without
//! reading it, every landing pad had no predecessor in the control-flow
//! graph, so the catch handler that had just run was reported unreachable.
//!
//! Two steps, because finding a function's LSDA differs by format while the
//! LSDA itself does not:
//!
//! - Mach-O: the compact-unwind section `__TEXT,__unwind_info` carries an
//!   LSDA index, (function offset, LSDA offset) pairs from the `__TEXT`
//!   segment's address.
//! - ELF: each `.eh_frame` FDE whose CIE augmentation has an `L` names its
//!   LSDA (read with gimli).
//!
//! Then the LSDA (`__gcc_except_tab` / `.gcc_except_table`): a header, and
//! call-site records of (start, length, landing pad, action), each offset
//! from the function's start. A landing pad of 0 means "none": unwind
//! further. Any encoding this reader does not know ends that function's
//! table, and nothing is claimed for it.

/// One call-site range and where the unwinder lands if a call in it throws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSite {
    pub start: u64,
    pub end: u64,
    pub landing_pad: u64,
}

const DW_EH_PE_OMIT: u8 = 0xff;

fn uleb(b: &[u8], i: &mut usize) -> Option<u64> {
    let (mut v, mut shift) = (0u64, 0u32);
    loop {
        let byte = *b.get(*i)?;
        *i += 1;
        if shift >= 64 {
            return None;
        }
        let payload = u64::from(byte & 0x7f);
        if shift > 0 && payload >> (64 - shift) != 0 {
            return None;
        }
        v |= payload << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
}

fn sleb(b: &[u8], i: &mut usize) -> Option<i64> {
    let (mut v, mut shift) = (0i64, 0u32);
    loop {
        let byte = *b.get(*i)?;
        *i += 1;
        if shift >= 64 {
            return None;
        }
        // At shift 63 only the sign bit (bit 63) remains: the continuation bit
        // must be clear and the upper 6 payload bits must be the sign extension
        // of bit 0 (so only 0x00 for non-negative or 0x7f for negative fits).
        if shift == 63 && byte != 0x00 && byte != 0x7f {
            return None;
        }
        v |= i64::from(byte & 0x7f) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 64 && byte & 0x40 != 0 {
                v |= -1i64 << shift;
            }
            return Some(v);
        }
    }
}

fn fixed(b: &[u8], i: &mut usize, n: usize, little_endian: bool) -> Option<u64> {
    let end = i.checked_add(n)?;
    let s = b.get(*i..end)?;
    *i = end;
    let mut a = [0u8; 8];
    if little_endian {
        a[..n].copy_from_slice(s);
        Some(u64::from_le_bytes(a))
    } else {
        a[8 - n..].copy_from_slice(s);
        Some(u64::from_be_bytes(a))
    }
}

/// A call-site field: a plain value in one of the encodings compilers use
/// for these tables. Pointer modifiers (pc-relative and the like) do not
/// occur in call-site records; one that did would be refused.
fn value(b: &[u8], i: &mut usize, enc: u8, little_endian: bool, is_64: bool) -> Option<u64> {
    match enc {
        0x00 => fixed(b, i, if is_64 { 8 } else { 4 }, little_endian),
        0x01 => uleb(b, i),
        0x02 => fixed(b, i, 2, little_endian),
        0x03 => fixed(b, i, 4, little_endian),
        0x04 => fixed(b, i, 8, little_endian),
        0x09 => sleb(b, i).and_then(|v| u64::try_from(v).ok()),
        0x0a => fixed(b, i, 2, little_endian).and_then(|v| u64::try_from(v as u16 as i16).ok()),
        0x0b => fixed(b, i, 4, little_endian).and_then(|v| u64::try_from(v as u32 as i32).ok()),
        0x0c => fixed(b, i, 8, little_endian).and_then(|v| u64::try_from(v as i64).ok()),
        _ => None,
    }
}

/// The call-site table of the LSDA at the start of `lsda`, for the function
/// that starts at `func` (assuming 64-bit little-endian). `None` for anything
/// this reader does not know.
pub fn parse_lsda(func: u64, lsda: &[u8]) -> Option<Vec<CallSite>> {
    parse_lsda_at_fmt(func, lsda, true, true).0
}

/// [`parse_lsda`] with explicit byte order and pointer width.
pub fn parse_lsda_with(func: u64, lsda: &[u8], little_endian: bool, is_64: bool) -> Option<Vec<CallSite>> {
    parse_lsda_at_fmt(func, lsda, little_endian, is_64).0
}

/// [`parse_lsda`], and how many bytes of `lsda` the attempt read -- its
/// header and call-site table if it decoded, or as far as it got before it
/// failed. Either way, the earliest the next function's LSDA can begin.
#[cfg(test)]
fn parse_lsda_at(func: u64, lsda: &[u8]) -> (Option<Vec<CallSite>>, usize) {
    parse_lsda_at_fmt(func, lsda, true, true)
}

fn parse_lsda_at_fmt(func: u64, lsda: &[u8], little_endian: bool, is_64: bool) -> (Option<Vec<CallSite>>, usize) {
    let mut i = 0usize;
    let sites = decode_lsda(func, lsda, &mut i, little_endian, is_64);
    (sites, i)
}

/// The body of [`parse_lsda_at_fmt`], leaving in `i` how far it read, whether
/// or not it succeeds.
///
/// The records must be in address order and must not overlap. The Itanium
/// ABI's personality routine reads them that way -- it stops at the first
/// record that starts past the faulting call -- and compilers write them so.
/// A table that is not is not one this reader can say anything about; read
/// as given, m records that each span the function gave every call every
/// landing pad, m² exception edges (#311).
fn decode_lsda(func: u64, lsda: &[u8], i: &mut usize, little_endian: bool, is_64: bool) -> Option<Vec<CallSite>> {
    let lpstart_enc = *lsda.get(*i)?;
    *i += 1;
    // A landing-pad base other than the function start is legal and rare;
    // compilers do not emit one for C++, so it is not read.
    if lpstart_enc != DW_EH_PE_OMIT {
        return None;
    }
    let ttype_enc = *lsda.get(*i)?;
    *i += 1;
    if ttype_enc != DW_EH_PE_OMIT {
        uleb(lsda, i)?;
    }
    let cs_enc = *lsda.get(*i)?;
    *i += 1;
    let len = usize::try_from(uleb(lsda, i)?).ok()?;
    let end = i.checked_add(len)?;
    if end > lsda.len() {
        return None;
    }
    let mut out = Vec::new();
    // where the previous record's range ended, as an offset from `func`
    let mut covered_to = 0u64;
    while *i < end {
        let start = value(lsda, i, cs_enc, little_endian, is_64)?;
        let length = value(lsda, i, cs_enc, little_endian, is_64)?;
        let pad = value(lsda, i, cs_enc, little_endian, is_64)?;
        uleb(lsda, i)?; // action
        if start < covered_to {
            return None;
        }
        covered_to = start.checked_add(length)?;
        if pad != 0 {
            out.push(CallSite { start: func.checked_add(start)?, end: func.checked_add(covered_to)?, landing_pad: func.checked_add(pad)? });
        }
    }
    (*i == end).then_some(out)
}

/// The landing pad of a call at `addr`, given the sorted, disjoint ranges
/// [`call_sites`] returns: at most one, found by binary search.
pub fn landing_pad_at(sites: &[CallSite], addr: u64) -> Option<u64> {
    let k = sites.partition_point(|c| c.start <= addr);
    let c = sites.get(k.checked_sub(1)?)?;
    (addr < c.end).then_some(c.landing_pad)
}

/// Mach-O `__unwind_info`: (function address, LSDA address) pairs, both
/// offsets from `base`, the `__TEXT` segment's address.
pub fn macho_lsda_index(unwind_info: &[u8], base: u64) -> Option<Vec<(u64, u64)>> {
    let u32at = |o: usize| {
        let end = o.checked_add(4)?;
        unwind_info.get(o..end).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
    };
    if u32at(0)? != 1 {
        return None;
    }
    let (index_off, index_count) = (u32at(20)?, u32at(24)?);
    if index_count == 0 {
        return Some(Vec::new());
    }
    // each first-level entry: function offset, 2nd-level page offset, LSDA index offset;
    // the LSDA array runs from the first entry's LSDA offset to the last's
    let lsda_first = u32at(index_off.checked_add(8)?)?;
    let last_entry = (index_count - 1).checked_mul(12)?.checked_add(index_off)?.checked_add(8)?;
    let lsda_last = u32at(last_entry)?;
    if lsda_last < lsda_first || (lsda_last - lsda_first) % 8 != 0 {
        return None;
    }
    let mut out = Vec::new();
    let mut o = lsda_first;
    while o < lsda_last {
        let o4 = o.checked_add(4)?;
        out.push((base.checked_add(u32at(o)? as u64)?, base.checked_add(u32at(o4)? as u64)?));
        o = o.checked_add(8)?;
    }
    Some(out)
}

/// ELF `.eh_frame`: (function address, LSDA address) for every FDE that has
/// an LSDA, and whether the whole section was decoded. `eh_frame_addr` is the
/// section's address, for pc-relative pointers.
///
/// The bool is `false` if any entry could not be parsed: a skipped FDE means a
/// function whose LSDA is not in the index, so its landing pads would be found
/// unreachable when they are only undecoded. The caller treats an incomplete
/// index conservatively rather than trusting the partial result (#165 review).
pub fn elf_lsda_index(eh_frame: &[u8], eh_frame_addr: u64, little_endian: bool, is_64: bool) -> (Vec<(u64, u64)>, bool) {
    use gimli::{BaseAddresses, CieOrFde, EhFrame, RunTimeEndian, UnwindSection};
    let endian = if little_endian { RunTimeEndian::Little } else { RunTimeEndian::Big };
    let mut section = EhFrame::new(eh_frame, endian);
    section.set_address_size(if is_64 { 8 } else { 4 });
    let bases = BaseAddresses::default().set_eh_frame(eh_frame_addr);
    let mut out = Vec::new();
    let mut complete = true;
    let mut entries = section.entries(&bases);
    loop {
        match entries.next() {
            Ok(Some(CieOrFde::Fde(partial))) => match partial.parse(EhFrame::cie_from_offset) {
                Ok(fde) => match fde.lsda() {
                    Some(gimli::Pointer::Direct(lsda)) => out.push((fde.initial_address(), lsda)),
                    // An FDE that *has* an LSDA in a pointer form this reader
                    // does not resolve (indirect/GOT-relative) is not a fully
                    // decoded entry: its function's call sites are unknown, so
                    // the index is incomplete, not empty here (#165 re-review).
                    Some(_) => complete = false,
                    None => {} // genuinely no LSDA for this function
                },
                Err(_) => complete = false,
            },
            Ok(Some(_)) => {} // a CIE, not an FDE
            Ok(None) => break,
            Err(_) => {
                complete = false;
                break;
            }
        }
    }
    (out, complete)
}

/// Every call-site range with a landing pad in `bin`, from its unwind data,
/// and whether the exception data was decoded in full. The ranges are sorted
/// by address and disjoint, so a call has at most one landing pad
/// ([`landing_pad_at`]).
///
/// The bool is `false` if any part could not be read or parsed -- a missing or
/// unreadable index, an LSDA whose bytes could not be read, or one this reader
/// could not decode. It matters because a call site that was NOT decoded leaves
/// its landing pad without a predecessor, and a pad with no predecessor is
/// otherwise claimed unreachable. So on any decode failure the caller suppresses
/// unreachable claims for the whole image rather than trusting a partial index
/// and mislabelling the undecoded function's pads (#165 review). Empty with
/// `true` means the image genuinely has no exception data.
///
/// The work is linear in the table (#311). Its bytes are read once, and each
/// byte is decoded for one function at most, whether that decode succeeded
/// or failed: an index entry naming an LSDA that starts inside bytes already
/// decoded, or already tried, for another (the same LSDA named twice, say)
/// is not something a compiler writes, and read as given it had every such
/// entry copy and decode the rest of the table again. That entry is not
/// decoded, and the result is incomplete. So is one whose ranges overlap
/// another function's: which pad the unwinder lands on is then not
/// established, and no range is returned.
pub fn call_sites(bin: &crate::objectcode::Binary) -> (Vec<CallSite>, bool) {
    let (sites, complete, _) = call_sites_counted(bin);
    (sites, complete)
}

/// [`call_sites`], and how many table bytes its LSDA decodes read in all,
/// successful or not: at most the table's size.
fn call_sites_counted(bin: &crate::objectcode::Binary) -> (Vec<CallSite>, bool, usize) {
    let section = |names: &[&str]| bin.sections.iter().find(|s| names.contains(&s.name.as_str()));
    let has_split_except = bin.sections.iter().any(|s| s.name.starts_with(".gcc_except_table."));
    let Some(table) = section(&["__gcc_except_tab", ".gcc_except_table"]) else {
        // Unmerged `.gcc_except_table.*` sections exist without a merged table:
        // fail closed rather than claiming the image has no exception data.
        return (Vec::new(), !has_split_except, 0);
    };
    let table_end = table.address.saturating_add(table.size);
    let mut complete = !has_split_except;
    let mut index: Vec<(u64, u64)> = Vec::new();
    let mut saw_index_section = false;
    if let Some(ui) = section(&["__unwind_info"]) {
        saw_index_section = true;
        let base = bin.segments.iter().find(|s| s.name == "__TEXT").map(|s| s.address).unwrap_or(0);
        match usize::try_from(ui.size).ok().and_then(|n| bin.read_at(ui.address, n)).and_then(|b| macho_lsda_index(&b, base)) {
            Some(idx) => index.extend(idx),
            None => complete = false,
        }
    }
    // On ELF this is `.eh_frame`; on Mach-O, `ld64`/`lld` also emit
    // `__eh_frame` alongside `__unwind_info` whenever any function falls back
    // to DWARF unwinding (`UNWIND_MODE_DWARF`), storing its LSDA only in the
    // FDE rather than in `__unwind_info`'s LSDA array.
    if let Some(eh) = section(&[".eh_frame", "__eh_frame"]) {
        saw_index_section = true;
        match usize::try_from(eh.size).ok().and_then(|n| bin.read_at(eh.address, n)) {
            Some(b) => {
                let (eh_idx, ok) = elf_lsda_index(&b, eh.address, bin.little_endian, bin.is_64);
                complete &= ok;
                let seen: std::collections::HashSet<(u64, u64)> = index.iter().copied().collect();
                for pair in eh_idx {
                    if !seen.contains(&pair) {
                        index.push(pair);
                    }
                }
            }
            None => complete = false,
        }
    }
    if !saw_index_section {
        // A table but no index to locate its LSDAs: this reader cannot read it.
        complete = false;
    }
    if index.is_empty() {
        return (Vec::new(), complete, 0);
    }
    // The table's bytes, once, for every LSDA in it.
    let Some(bytes) = usize::try_from(table.size).ok().and_then(|n| bin.read_at(table.address, n)) else {
        return (Vec::new(), false, 0);
    };
    // In table order, so that where one LSDA's call-site table ends bounds
    // where the next can start.
    index.sort_unstable_by_key(|&(func, lsda)| (lsda, func));
    let mut out = Vec::new();
    let mut decoded_to = table.address;
    let mut work = 0usize;
    for (func, lsda) in index {
        if lsda < table.address || lsda >= table_end {
            // The index names an LSDA the selected exception table does not
            // cover (another table section, or a form we did not locate): that
            // function's call sites are unknown, so this is an incomplete
            // decode, not a clean skip (#165 re-review).
            complete = false;
            continue;
        }
        if lsda < decoded_to {
            // bytes already decoded, or tried, for another function (#311)
            complete = false;
            continue;
        }
        let (sites, used) = parse_lsda_at_fmt(func, &bytes[(lsda - table.address) as usize..], bin.little_endian, bin.is_64);
        // A failed decode read its bytes too: they are not read again for
        // the next entry, or a long table that fails late is decoded once
        // per entry naming it (#316 review).
        decoded_to = lsda.saturating_add(used as u64);
        work += used;
        match sites {
            Some(sites) => out.extend(sites),
            None => complete = false, // an LSDA this reader could not decode
        }
    }
    out.sort_unstable_by_key(|c| (c.start, c.end));
    if out.windows(2).any(|w| w[1].start < w[0].end) {
        return (Vec::new(), false, work);
    }
    (out, complete, work)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The LSDA clang wrote for `safe_altitude` in examples/cpp-exceptions
    /// (Apple clang 17, arm64), byte for byte.
    #[test]
    fn reads_the_call_site_table_clang_writes() {
        let lsda = [
            0xff, 0x9b, 0x2d, 0x01, 0x1c, 0x14, 0x08, 0x2c, 0x01, 0x1c, 0x44, 0x00, 0x00, 0x60, 0x14, 0x84, 0x01, 0x00, 0x74, 0x1c, 0x00, 0x00, 0x90, 0x01, 0x04, 0xb4, 0x01, 0x03,
            0x94, 0x01, 0x24, 0x00, 0x00,
        ];
        let cs = parse_lsda(0x5e8, &lsda).expect("a table");
        assert_eq!(
            cs,
            vec![
                CallSite { start: 0x5fc, end: 0x604, landing_pad: 0x614 },
                CallSite { start: 0x648, end: 0x65c, landing_pad: 0x66c },
                CallSite { start: 0x678, end: 0x67c, landing_pad: 0x69c }
            ],
            "the three ranges with a landing pad; the others unwind further"
        );
    }

    #[test]
    fn a_truncated_eh_frame_index_reports_itself_incomplete() {
        // Garbage where an FDE length prefix should be: gimli cannot walk the
        // section, so the index is empty AND flagged incomplete -- the caller
        // must not treat undecoded functions' pads as unreachable (#165).
        let (idx, complete) = elf_lsda_index(&[0xff, 0xff, 0xff, 0x7f, 0x01, 0x02, 0x03], 0, true, true);
        assert!(idx.is_empty());
        assert!(!complete, "an eh_frame it could not walk is not a complete index");
    }

    #[test]
    fn an_empty_eh_frame_is_complete_with_no_lsdas() {
        // No entries at all is a full (if empty) decode: an image with no
        // exception data must not suppress unreachable detection.
        let (idx, complete) = elf_lsda_index(&[], 0, true, true);
        assert!(idx.is_empty() && complete);
    }

    #[test]
    fn an_encoding_it_does_not_know_claims_nothing() {
        // lpstart given explicitly
        assert_eq!(parse_lsda(0, &[0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0x01, 0x00]), None);
        // a call-site encoding with a pointer modifier (pc-relative udata4)
        assert_eq!(parse_lsda(0, &[0xff, 0xff, 0x13, 0x04, 0, 0, 0, 0]), None);
        // a table longer than its bytes
        assert_eq!(parse_lsda(0, &[0xff, 0xff, 0x01, 0x40, 0x01]), None);
    }

    /// examples/cpp-exceptions' `__unwind_info`, byte for byte: two
    /// functions with an LSDA.
    #[test]
    fn reads_the_compact_unwind_lsda_index() {
        let words: [u32; 24] =
            [1, 0x1c, 3, 0x28, 1, 0x2c, 2, 0x0400_0000, 0x0200_1000, 0x5400_0000, 0x4000, 0x588, 0x60, 0x50, 0xad4, 0, 0x60, 0, 0, 0, 0x5e8, 0xb40, 0x740, 0xb70];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        assert_eq!(macho_lsda_index(&bytes, 0x1_0000_0000), Some(vec![(0x1_0000_05e8, 0x1_0000_0b40), (0x1_0000_0740, 0x1_0000_0b70)]));
        // a segment address the offsets would carry past the top of the
        // address space is not an index (#311)
        assert_eq!(macho_lsda_index(&bytes, u64::MAX - 0x100), None);
    }

    /// Call-site records that overlap, or are out of order, are not a table
    /// a compiler writes; read as given, m records that each span the
    /// function gave every call every pad -- m² exception edges (#311).
    #[test]
    fn overlapping_or_unsorted_call_sites_are_not_decoded() {
        // uleb records (start, length, pad, action) in an 8-byte table
        let lsda = |recs: [[u8; 4]; 2]| -> Vec<u8> { [vec![0xff, 0xff, 0x01, 0x08], recs.concat()].concat() };
        assert_eq!(parse_lsda(0x1000, &lsda([[0x00, 0x10, 0x20, 0], [0x00, 0x10, 0x24, 0]])), None, "two records over the same range");
        assert_eq!(parse_lsda(0x1000, &lsda([[0x08, 0x04, 0x20, 0], [0x00, 0x04, 0x24, 0]])), None, "out of address order");
        assert_eq!(parse_lsda(0x1000, &lsda([[0x00, 0x08, 0x20, 0], [0x04, 0x08, 0x24, 0]])), None, "the second starts inside the first");
        let adjacent = parse_lsda(0x1000, &lsda([[0x00, 0x04, 0x20, 0], [0x04, 0x04, 0x24, 0]])).expect("adjacent ranges in order are a table");
        assert_eq!(adjacent, vec![CallSite { start: 0x1000, end: 0x1004, landing_pad: 0x1020 }, CallSite { start: 0x1004, end: 0x1008, landing_pad: 0x1024 }]);
    }

    /// A Mach-O image holding `table` as `__gcc_except_tab` at 0x8000 and an
    /// `__unwind_info` whose LSDA index is `entries` (function, LSDA), both
    /// relative to a `__TEXT` at 0.
    fn image(entries: &[(u32, u32)], table: &[u8]) -> crate::objectcode::Binary {
        use crate::objectcode::{Binary, SectionInfo, SegmentInfo};
        let mut ui: Vec<u32> = vec![1, 0, 0, 0, 0, 28, 2];
        let lsda_first = 28 + 24;
        let lsda_last = lsda_first + 8 * entries.len() as u32;
        ui.extend([0, 0, lsda_first, 0, 0, lsda_last]);
        for &(f, l) in entries {
            ui.extend([f, l]);
        }
        let mut bytes: Vec<u8> = ui.iter().flat_map(|w| w.to_le_bytes()).collect();
        let ui_len = bytes.len() as u64;
        bytes.extend_from_slice(table);
        let sec = |name: &str, address: u64, size: u64, off: u64| SectionInfo {
            name: name.into(),
            address,
            size,
            kind: String::new(),
            flags: String::new(),
            alloc: true,
            file_range: Some((off, size)),
        };
        Binary {
            relocatable: false,
            path: Default::default(),
            format: "MachO".into(),
            arch: "aarch64".into(),
            is_64: true,
            little_endian: true,
            entry: 0,
            sections: vec![sec("__unwind_info", 0x4000, ui_len, 0), sec("__gcc_except_tab", 0x8000, table.len() as u64, ui_len)],
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
            objdump_path: "llvm-objdump".into(),
            // a Mach-O image: no ELF RELATIVE slots (#332)
            relative_words: Default::default(),
        }
    }

    /// One LSDA: a call site over [0, 4) landing at +8.
    const ONE_SITE: [u8; 8] = [0xff, 0xff, 0x01, 0x04, 0x00, 0x04, 0x08, 0x00];

    #[test]
    fn two_functions_with_their_own_lsdas_are_read() {
        let table = [ONE_SITE, ONE_SITE].concat();
        let (sites, complete) = call_sites(&image(&[(0x1000, 0x8000), (0x2000, 0x8008)], &table));
        assert!(complete);
        assert_eq!(sites, vec![CallSite { start: 0x1000, end: 0x1004, landing_pad: 0x1008 }, CallSite { start: 0x2000, end: 0x2004, landing_pad: 0x2008 }]);
        assert_eq!(landing_pad_at(&sites, 0x1000), Some(0x1008));
        assert_eq!(landing_pad_at(&sites, 0x1003), Some(0x1008));
        assert_eq!(landing_pad_at(&sites, 0x1004), None, "the range's end is outside it");
        assert_eq!(landing_pad_at(&sites, 0x0fff), None);
        assert_eq!(landing_pad_at(&sites, 0x2002), Some(0x2008));
    }

    /// An index that names one LSDA for many functions had each entry copy
    /// and decode the rest of the table again: work of entries × table size
    /// (#311). Each byte of the table is decoded for one function at most.
    #[test]
    fn an_lsda_named_by_several_functions_is_decoded_once_and_flagged() {
        let entries: Vec<(u32, u32)> = (0..64).map(|k| (0x1000 + 0x100 * k, 0x8000)).collect();
        let (sites, complete) = call_sites(&image(&entries, &ONE_SITE));
        assert!(!complete, "an LSDA shared by several index entries is not a decode to trust");
        assert!(sites.len() <= 1, "decoded once, not once per entry: {} sites", sites.len());
    }

    /// A failed decode counts against the bound too: a long LSDA whose last
    /// record is bad, named by many index entries, was decoded to its end
    /// once per entry, since only a successful decode advanced past its
    /// bytes (#316 review).
    #[test]
    fn an_lsda_that_fails_late_is_decoded_once_however_often_it_is_named() {
        // 100 in-order uleb records (start k, length 1, pad 0x7f), then one
        // that starts back at 0: out of order, found only at the end.
        let mut records: Vec<u8> = (0..100u8).flat_map(|k| [k, 0x01, 0x7f, 0x00]).collect();
        records.extend([0x00, 0x01, 0x7f, 0x00]);
        assert_eq!(records.len(), 404);
        let table = [vec![0xff, 0xff, 0x01, 0x94, 0x03], records].concat(); // 0x194 = 404, as uleb
        assert_eq!(parse_lsda_at(0x1000, &table), (None, table.len()), "fails at its last record");
        let entries: Vec<(u32, u32)> = (0..64).map(|k| (0x1000 + 0x100 * k, 0x8000)).collect();
        let (sites, complete, work) = call_sites_counted(&image(&entries, &table));
        assert!(!complete);
        assert!(sites.is_empty(), "{sites:?}");
        assert!(work <= table.len(), "{work} bytes decoded from a {}-byte table", table.len());
    }

    /// Two functions' call-site ranges that overlap leave which pad an
    /// unwinder lands on unestablished; nothing is claimed from them.
    #[test]
    fn call_sites_of_different_functions_that_overlap_are_not_trusted() {
        let table = [ONE_SITE, ONE_SITE].concat();
        let (sites, complete) = call_sites(&image(&[(0x1000, 0x8000), (0x1002, 0x8008)], &table));
        assert!(!complete);
        assert!(sites.is_empty(), "{sites:?}");
    }

    #[test]
    fn uleb_and_sleb_reject_tenth_byte_overflow() {
        // 10-byte uleb with bit 64 set (0x02 at shift 63) must fail closed, not wrap to 0
        let mut bad_uleb = vec![0xff, 0xff, 0x01];
        bad_uleb.extend(std::iter::repeat_n(0x80u8, 9));
        bad_uleb.push(0x02);
        assert_eq!(parse_lsda(0x1000, &bad_uleb), None);

        // 10-byte sleb with invalid sign-extension bits (0x02 at shift 63)
        let mut bad_sleb = vec![0xff, 0xff, 0x09, 13];
        bad_sleb.extend(std::iter::repeat_n(0x80u8, 9));
        bad_sleb.push(0x02);
        bad_sleb.extend([0x04, 0x08, 0x00]);
        assert_eq!(parse_lsda(0x1000, &bad_sleb), None);
    }

    #[test]
    fn lsda_respects_endianness_width_and_rejects_negative_signed_at_zero() {
        // Big-endian udata4: start=4, len=8, pad=0x20, action=0
        let be = [vec![0xff, 0xff, 0x03, 13], 4u32.to_be_bytes().to_vec(), 8u32.to_be_bytes().to_vec(), 0x20u32.to_be_bytes().to_vec(), vec![0]].concat();
        assert_eq!(parse_lsda_with(0x1000, &be, false, true), Some(vec![CallSite { start: 0x1004, end: 0x100c, landing_pad: 0x1020 }]));
        // 32-bit absptr (0x00) uses 4-byte fields
        let abs32 = [vec![0xff, 0xff, 0x00, 13], 4u32.to_le_bytes().to_vec(), 8u32.to_le_bytes().to_vec(), 0x20u32.to_le_bytes().to_vec(), vec![0]].concat();
        assert_eq!(parse_lsda_with(0x1000, &abs32, true, false), Some(vec![CallSite { start: 0x1004, end: 0x100c, landing_pad: 0x1020 }]));
        // Negative sdata4 rejected even when func == 0 (cannot wrap to huge u64)
        let neg = [vec![0xff, 0xff, 0x0b, 13], 4i32.to_le_bytes().to_vec(), 8i32.to_le_bytes().to_vec(), (-4i32).to_le_bytes().to_vec(), vec![0]].concat();
        assert_eq!(parse_lsda_with(0, &neg, true, true), None);
    }

    #[test]
    fn macho_hybrid_unwind_info_and_eh_frame_fails_closed_when_eh_frame_corrupt() {
        let mut b = image(&[(0x1000, 0x8000)], &ONE_SITE);
        // Add a corrupt __eh_frame alongside a valid __unwind_info: must not
        // silently ignore __eh_frame and report complete=true (#9.9).
        let eh_off = b.bytes.len() as u64;
        b.bytes.extend_from_slice(&[0xff, 0xff, 0xff, 0x7f, 0x01, 0x02, 0x03]);
        b.sections.push(crate::objectcode::SectionInfo {
            name: "__eh_frame".into(),
            address: 0x9000,
            size: 7,
            kind: String::new(),
            flags: String::new(),
            alloc: true,
            file_range: Some((eh_off, 7)),
        });
        let (_, complete) = call_sites(&b);
        assert!(!complete, "corrupt __eh_frame beside __unwind_info must fail closed");
    }

    #[test]
    fn unmerged_split_gcc_except_table_fails_closed() {
        let mut b = image(&[], &[]);
        b.sections = vec![crate::objectcode::SectionInfo {
            name: ".gcc_except_table.foo".into(),
            address: 0x8000,
            size: 8,
            kind: String::new(),
            flags: String::new(),
            alloc: true,
            file_range: Some((0, 8)),
        }];
        let (_, complete) = call_sites(&b);
        assert!(!complete, "unmerged .gcc_except_table.* must not claim complete empty EH data");
    }
}
