// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Parameters and locals, and where the object code touches them (#52).
//!
//! Two sources, both the compiler's own, and neither one the source text:
//!
//! * **DWARF** says what exists and where it lives.
//!   `DW_TAG_formal_parameter` and `DW_TAG_variable` carry the name, the
//!   declaration line, the type, and `DW_AT_location` -- the storage the
//!   compiler actually chose.
//! * **the disassembly** says which instructions reach that storage.
//!
//! Reading the source instead would be guessing at both: which declarations
//! the compiler kept, and which machine instructions correspond to a use.
//!
//! # What this reads, and what it does not
//!
//! A variable in a frame slot -- `DW_OP_fbreg <offset>` against a frame base
//! that is the stack pointer, which is what every unoptimised build produces
//! -- is attributed exactly: the offset resolves to a `[sp, #imm]` operand
//! and every load and store of it is a read or a write.
//!
//! Everything else is **reported, not guessed**:
//!
//! * a variable the optimiser put in a register has no frame slot to watch;
//! * a location list gives it different storage over different PC ranges;
//! * a frame base that is not the stack pointer needs the frame register's
//!   value, which this does not track.
//!
//! In each case the variable is still listed -- it exists, and its
//! declaration is real -- with `unattributed` saying why its access list is
//! short. An empty list that means "we could not tell" must never read as
//! "it is never used": that is the inversion this tool exists to prevent,
//! and it would be a quiet one.

use crate::objectcode::RawInstr;
use butterfly_model::{VarAccess, Variable};
use std::collections::HashMap;
use std::sync::Arc;

/// Which register a frame slot is measured from.
///
/// Not assumed: a function compiled with a frame pointer measures from
/// `x29`, one without from `sp`, and the same offset means a different slot
/// in each. Matching an `[x29, #8]` operand against a variable that lives at
/// `[sp, #8]` would attribute an access to the wrong variable, which is
/// worse than attributing none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    Sp,
    Fp,
}

impl Base {
    fn matches(self, reg: &str) -> bool {
        match self {
            Base::Sp => reg == "sp" || reg == "wsp",
            Base::Fp => reg == "x29" || reg == "w29" || reg == "fp",
        }
    }
}

/// Where a variable lives, as far as we could tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A frame slot at this offset from that base register.
    Frame(Base, i64),
    /// Somewhere this reader does not follow, with the reason.
    Elsewhere(&'static str),
}

/// One variable as DWARF describes it, before the disassembly is consulted.
#[derive(Debug, Clone)]
pub struct Declared {
    pub name: Arc<str>,
    pub kind: &'static str,
    pub decl_line: u32,
    pub type_name: Option<Arc<str>>,
    pub place: Place,
    /// Bytes the variable occupies, from its type. `None` when the debug
    /// info does not say, and then only accesses that reach its first byte
    /// can be recognised -- which is exact for a scalar and incomplete for
    /// anything larger, so the variable says so rather than looking unused.
    pub size: Option<u64>,
}

fn is_load(m: &str) -> bool {
    m.starts_with("ld")
}
fn is_store(m: &str) -> bool {
    m.starts_with("st")
}
/// `ldp`/`stp` move two registers, so they touch twice one register's width.
fn is_pair(m: &str) -> bool {
    m == "ldp" || m == "stp" || m == "ldnp" || m == "stnp"
}

/// How many bytes one load or store moves.
///
/// Needed because a variable is a *range*, not an address: `values[1]` of an
/// `int values[2]` is written at the variable's offset plus four, and an
/// `stp` writes sixteen bytes across two slots. Matching the start offset
/// alone reports both as untouched (review of #52).
fn access_bytes(m: &str, ops: &[String]) -> u64 {
    // The suffix names the width outright where there is one.
    let base = if m.ends_with('b') {
        1
    } else if m.ends_with('h') {
        2
    } else {
        // Otherwise the destination register's width says it: w0 is four
        // bytes, x0 eight. `ldrsw` is a 4-byte load sign-extended into an
        // x register, so the suffix is consulted before the register.
        if m.contains("sw") {
            4
        } else {
            match ops.first().map(|o| o.trim_start_matches('\'')) {
                Some(r) if r.starts_with('w') || r.starts_with('s') => 4,
                Some(r) if r.starts_with('d') => 8,
                Some(r) if r.starts_with('q') || r.starts_with('v') => 16,
                _ => 8,
            }
        }
    };
    if is_pair(m) {
        base * 2
    } else {
        base
    }
}

/// The `#imm` of a `[<base>, #imm]` operand, or `Some(0)` for a bare
/// `[<base>]`, when the operand's base register is the one asked for.
///
/// Deliberately narrow: only a plain immediate off the expected base. A
/// register-indexed or pre/post-indexed form addresses something that
/// depends on a register value this does not track, and reading it as a
/// frame slot would attribute an access to the wrong variable.
fn frame_offset(operand: &str, base: Base) -> Option<i64> {
    let inner = operand.strip_prefix('[')?.strip_suffix(']')?;
    if inner.ends_with('!') {
        return None; // pre-indexed: it also writes the base
    }
    let mut parts = inner.split(',').map(str::trim);
    if !base.matches(parts.next()?) {
        return None;
    }
    match parts.next() {
        None => Some(0),
        Some(off) => {
            if parts.next().is_some() {
                return None; // a third component is an addressing mode we do not read
            }
            let t = off.trim_start_matches('#');
            let (neg, t) = match t.strip_prefix('-') {
                Some(r) => (true, r),
                None => (false, t),
            };
            let v = if let Some(h) = t.strip_prefix("0x") { i64::from_str_radix(h, 16).ok()? } else { t.parse::<i64>().ok()? };
            Some(if neg { -v } else { v })
        }
    }
}

/// Attribute each declared variable's accesses from one function's
/// instructions, in the order they appear -- which is address order, since
/// `objectcode` hands them over sorted by address.
///
/// That makes the resulting sequence a **static** one: this is a single
/// pass over a list of instructions, not a walk over the control-flow graph
/// and not a trace of a run. It records where a variable is touched, never
/// when, and consumers must say so (review of #53).
///
/// `line_of` gives the source line the line table attributes to an address,
/// where it attributes one.
pub fn attribute(arch: &str, declared: &[Declared], instrs: &[RawInstr], line_of: &dyn Fn(u64) -> Option<u32>) -> Vec<Variable> {
    // Only AArch64's operand shape is read here. Saying so is the point: an
    // empty list on another architecture would otherwise look like a
    // variable nothing touches.
    let foreign = (!arch.starts_with("aarch64")).then(|| format!("frame-slot accesses are read for aarch64; this image is {arch}"));

    // Where a frame base stops meaning what DWARF said it means. A pre- or
    // post-indexed memory operand writes its base register back, so every
    // offset measured from *that* register afterwards names a different
    // slot. Recorded per register, because a variable based on x29 is
    // unaffected by a writeback through sp -- and neither is touched by one
    // through x1 (review of #52).
    let writebacks: Vec<(u64, String)> = instrs
        .iter()
        .filter_map(|i| {
            let m = i.mnemonic.to_ascii_lowercase();
            (is_load(&m) || is_store(&m)).then(|| writeback_register(&i.operands).map(|r| (i.addr, r)))?
        })
        .collect();

    let mut out = Vec::new();
    for d in declared {
        let mut v = Variable {
            name: d.name.to_string(),
            kind: d.kind.into(),
            decl_line: d.decl_line,
            type_name: d.type_name.as_deref().map(str::to_string),
            accesses: Vec::new(),
            unattributed: None,
        };
        // The architecture is checked before the place. `declared` reads
        // frame bases as AArch64 register numbers, so on another image the
        // place is usually `Elsewhere` for a reason that is a symptom -- an
        // x86-64 `DW_OP_reg6` base reads as "an expression this reader does
        // not evaluate" -- and naming the symptom would hide the cause:
        // nothing on this image is read at all (#172).
        if let Some(why) = &foreign {
            v.unattributed = Some(why.clone());
            out.push(v);
            continue;
        }
        let (base, off) = match &d.place {
            Place::Elsewhere(why) => {
                v.unattributed = Some((*why).into());
                out.push(v);
                continue;
            }
            Place::Frame(b, o) => (*b, *o),
        };

        // The variable occupies [off, off + size). Where the debug info did
        // not give a size, only an access reaching its first byte can be
        // recognised -- exact for a scalar, incomplete for anything larger,
        // and said so below rather than left to look unused. Compared in
        // i128 so an offset or size at the edge of i64 (hostile DWARF)
        // cannot overflow: the comparison stays exact.
        let span = i128::from(d.size.unwrap_or(1));
        let overlaps = |at: i64, width: i64| i128::from(at) < i128::from(off) + span && i128::from(off) < i128::from(at) + i128::from(width);

        // only a writeback through *this* variable's own base ends its
        // attribution
        let stops_at = writebacks.iter().filter(|(_, r)| base.matches(r)).map(|(a, _)| *a).min();

        let mut escaped = false;
        let mut truncated = false;
        for i in instrs {
            if stops_at.is_some_and(|w| i.addr >= w) {
                truncated = true;
                break;
            }
            let m = i.mnemonic.to_ascii_lowercase();
            let ops = crate::datarefs::split_ops(&i.operands);

            // A load or store: attribute it when the bytes it moves overlap
            // the variable's own.
            if is_load(&m) || is_store(&m) {
                if let Some(mem) = ops.iter().find(|o| o.starts_with('[')) {
                    if let Some(at) = frame_offset(mem, base) {
                        if overlaps(at, access_bytes(&m, &ops) as i64) {
                            v.accesses.push(VarAccess { addr: i.addr, line: line_of(i.addr), access: if is_store(&m) { "write".into() } else { "read".into() } });
                        }
                    }
                }
                continue;
            }

            // The variable's address being formed -- `add x0, sp, #16`, or
            // `sub x0, x29, #4` -- is a third kind of touch, and the model
            // already has a word for it. What happens through that pointer
            // afterwards is not visible here, so the variable is also marked
            // incomplete: a list showing a read and no write, because the
            // callee did the writing, would read as a use-before-set bug
            // that is not there.
            if let Some(at) = formed_address(&m, &ops, base) {
                if overlaps(at, 1) {
                    v.accesses.push(VarAccess { addr: i.addr, line: line_of(i.addr), access: "address".into() });
                    escaped = true;
                }
            }
        }

        // Why the list may be shorter than the source implies. Ordered by
        // how much it costs the reader to be misled by it.
        v.unattributed = if escaped {
            Some("its address is taken here, and accesses made through that pointer -- by a callee, or later in this function -- are not tracked".into())
        } else if truncated {
            Some("an instruction writes back the frame base register, so offsets after it name different slots and were not read".into())
        } else if d.size.is_none() && !matches!(d.place, Place::Elsewhere(_)) {
            Some("the debug info gives no size for it, so only accesses reaching its first byte were recognised".into())
        } else {
            None
        };
        out.push(v);
    }
    out
}

/// The register a memory operand writes back, if it writes one back.
/// `[sp, #16]!` is pre-indexed and `[sp], #16` post-indexed; both leave that
/// register changed.
///
/// The post-indexed form is why the whole operand list has to be inspected
/// rather than the bracketed part alone: split_ops yields `[sp]` and `#16`
/// separately, and `[sp]` on its own looks like an ordinary offset of zero.
///
/// *Which* register matters as much as whether: `ldr x0, [x1], #8` is
/// ordinary pointer walking and says nothing about the frame. Treating it as
/// a frame writeback erased every stack variable's evidence in any function
/// that walks a pointer (review of #52).
fn writeback_register(operands: &str) -> Option<String> {
    let ops = crate::datarefs::split_ops(operands);
    let k = ops.iter().position(|o| o.starts_with('['))?;
    let pre = ops[k].ends_with("]!");
    // post-indexed: a plain `[reg]` followed by an immediate
    let post = ops[k].ends_with(']') && ops.get(k + 1).is_some_and(|n| n.starts_with('#'));
    if !pre && !post {
        return None;
    }
    // the base is the first component inside the brackets
    let inner = ops[k].trim_start_matches('[').trim_end_matches('!').trim_end_matches(']');
    Some(inner.split(',').next()?.trim().to_string())
}

/// The frame offset an `add`/`sub` forms into a register, when it takes the
/// address of a frame slot rather than loading from one.
fn formed_address(m: &str, ops: &[String], base: Base) -> Option<i64> {
    let sign = match m {
        "add" => 1,
        "sub" => -1,
        _ => return None,
    };
    // add <dst>, <base>, #imm -- and the destination must not be the base
    // itself, which is a frame adjustment rather than an address being taken
    let (dst, src, imm) = (ops.first()?, ops.get(1)?, ops.get(2)?);
    if !base.matches(src) || base.matches(dst) {
        return None;
    }
    let t = imm.trim_start_matches('#');
    let n = if let Some(h) = t.strip_prefix("0x") { i64::from_str_radix(h, 16).ok()? } else { t.parse::<i64>().ok()? };
    Some(sign * n)
}

/// Read `DW_TAG_formal_parameter` and `DW_TAG_variable` for every
/// subprogram, keyed by the subprogram's low PC.
///
/// Nested `DW_TAG_lexical_block` scopes are walked: a variable declared
/// inside an `if` is still a variable of that function, and stopping at the
/// first block would drop it.
pub fn declared(file: &object::File) -> anyhow::Result<std::collections::HashMap<u64, Vec<Declared>>> {
    let (sections, endian, _) = crate::objectcode::dwarf_sections_of(file)?;
    let dwarf = sections.borrow(|section| gimli::EndianSlice::new(section, endian));
    declared_from_dwarf(&dwarf)
}

fn declared_from_dwarf<'a>(dwarf: &gimli::Dwarf<gimli::EndianSlice<'a, gimli::RunTimeEndian>>) -> anyhow::Result<std::collections::HashMap<u64, Vec<Declared>>> {
    use gimli::{AttributeValue, DwTag};
    let mut out: std::collections::HashMap<u64, Vec<Declared>> = std::collections::HashMap::new();
    // Intern DWARF strings so many DIEs referencing one large `.debug_str`
    // entry share a single `Arc<str>` allocation instead of multiplying heap
    // usage (#9.3).
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
        let mut entries = unit.entries();
        let mut depth: isize = 0;
        // the subprogram we are inside, and whether its frame base is the
        // stack pointer -- the only base whose slots we can find in the
        // operands without tracking a register's value
        let mut current: Option<(isize, u64, Option<Base>)> = None;

        while let Some((delta, entry)) = entries.next_dfs()? {
            depth += delta;
            if current.map(|(d, _, _)| depth <= d).unwrap_or(false) {
                current = None;
            }
            if entry.tag() == gimli::DW_TAG_subprogram {
                let low = match entry.attr_value(gimli::DW_AT_low_pc)? {
                    Some(AttributeValue::Addr(a)) => Some(a),
                    Some(AttributeValue::DebugAddrIndex(i)) => dwarf.address(&unit, i).ok(),
                    _ => None,
                };
                // Which register `DW_OP_fbreg` offsets are measured from.
                // `DW_OP_reg31` is WSP, `DW_OP_reg29` the frame pointer;
                // a build with a frame pointer uses the latter, and reading
                // its offsets against `sp` would name the wrong slot. Require
                // an exact single-op expression so multi-op expressions fail
                // closed.
                let base = match entry.attr_value(gimli::DW_AT_frame_base)? {
                    Some(AttributeValue::Exprloc(e)) => decode_frame_base(e.0.slice()),
                    _ => None,
                };
                if let Some(low) = low {
                    current = Some((depth, low, base));
                }
                continue;
            }
            let kind: &'static str = match entry.tag() {
                DwTag(t) if t == gimli::DW_TAG_formal_parameter.0 => "param",
                DwTag(t) if t == gimli::DW_TAG_variable.0 => "local",
                _ => continue,
            };
            let Some((_, low, fb)) = current else { continue };

            let name = match entry.attr_value(gimli::DW_AT_name)? {
                Some(v) => intern_str(dwarf.attr_string(&unit, v)?),
                None => continue, // an unnamed slot is nothing a reader can ask about
            };
            let decl_line = match entry.attr_value(gimli::DW_AT_decl_line)? {
                Some(AttributeValue::Udata(v)) => v as u32,
                Some(AttributeValue::Data1(v)) => v as u32,
                Some(AttributeValue::Data2(v)) => v as u32,
                Some(AttributeValue::Data4(v)) => v,
                _ => 0,
            };
            // How many bytes the variable occupies. A scalar states it
            // outright; an array usually does not, and its extent is the
            // element size times the count in its subrange. Without this a
            // variable is treated as a single address and every access past
            // its first element is invisible.
            let size = match entry.attr_value(gimli::DW_AT_type)? {
                Some(AttributeValue::UnitRef(off)) => type_size(&unit, off, 16).unwrap_or(None),
                _ => None,
            };
            let type_name = match entry.attr_value(gimli::DW_AT_type)? {
                Some(AttributeValue::UnitRef(off)) => {
                    let t = unit.entry(off)?;
                    match t.attr_value(gimli::DW_AT_name)? {
                        Some(v) => dwarf.attr_string(&unit, v).ok().map(&mut intern_str),
                        None => None,
                    }
                }
                _ => None,
            };

            let place = match entry.attr_value(gimli::DW_AT_location)? {
                // A single expression valid for the whole scope. `DW_OP_fbreg`
                // (0x91) plus a SLEB128 offset is the unoptimised shape.
                Some(AttributeValue::Exprloc(e)) => decode_location(e.0.slice(), fb),
                // A location list: different storage over different PC
                // ranges, which is what optimisation produces.
                Some(AttributeValue::LocationListsRef(_)) | Some(AttributeValue::DebugLocListsIndex(_)) => {
                    Place::Elsewhere("its storage changes across the function (a location list), which this reader does not follow")
                }
                _ => Place::Elsewhere("the debug info records no location for it"),
            };

            out.entry(low).or_default().push(Declared { name, kind, decl_line, type_name, place, size });
        }
    }
    Ok(out)
}

fn decode_frame_base(b: &[u8]) -> Option<Base> {
    match b {
        [0x6f] => Some(Base::Sp), // DW_OP_reg31 (WSP)
        [0x6d] => Some(Base::Fp), // DW_OP_reg29 (frame pointer)
        _ => None,
    }
}

fn decode_location(b: &[u8], fb: Option<Base>) -> Place {
    match b.first() {
        // DW_OP_fbreg <sleb>: an offset from the subprogram's own frame base,
        // with no trailing opcodes (e.g. DW_OP_deref / DW_OP_piece /
        // DW_OP_plus_uconst).
        Some(0x91) => match sleb128_exact(&b[1..]) {
            Sleb::Exact(o) => match fb {
                Some(base) => Place::Frame(base, o),
                None => Place::Elsewhere("its frame base is an expression this reader does not evaluate"),
            },
            Sleb::Trailing => Place::Elsewhere("its location is an expression this reader does not evaluate"),
            Sleb::Invalid => Place::Elsewhere("its frame offset could not be decoded"),
        },
        // DW_OP_breg31 <sleb>: `sp + offset` stated outright, and DW_OP_breg29
        // the same off the frame pointer.
        Some(0x8f) => match sleb128_exact(&b[1..]) {
            Sleb::Exact(o) => Place::Frame(Base::Sp, o),
            Sleb::Trailing => Place::Elsewhere("its location is an expression this reader does not evaluate"),
            Sleb::Invalid => Place::Elsewhere("its stack offset could not be decoded"),
        },
        Some(0x8d) => match sleb128_exact(&b[1..]) {
            Sleb::Exact(o) => Place::Frame(Base::Fp, o),
            Sleb::Trailing => Place::Elsewhere("its location is an expression this reader does not evaluate"),
            Sleb::Invalid => Place::Elsewhere("its frame offset could not be decoded"),
        },
        // DW_OP_reg0..reg31: it lives in a register, so there is no memory
        // operand to watch for.
        Some(0x50..=0x6f) if b.len() == 1 => Place::Elsewhere("the compiler kept it in a register, which has no frame slot to watch"),
        _ => Place::Elsewhere("its location is an expression this reader does not evaluate"),
    }
}

/// An unsigned constant however the producer chose to encode it. clang
/// writes a small array count as `Data1`, so matching `Udata` alone reads
/// every short array as sizeless.
fn udata<R: gimli::Reader>(v: Option<gimli::AttributeValue<R>>) -> Option<u64> {
    use gimli::AttributeValue::*;
    match v? {
        Udata(n) => Some(n),
        Data1(n) => Some(n as u64),
        Data2(n) => Some(n as u64),
        Data4(n) => Some(n as u64),
        Data8(n) => Some(n),
        Sdata(n) if n >= 0 => Some(n as u64),
        _ => None,
    }
}

/// One array dimension's element count, from its subrange's `DW_AT_count`
/// when stated, else one more than its `DW_AT_upper_bound`.
///
/// Checked, like the product it feeds: a bound of `u64::MAX` -- GCC's `-1`
/// for an array of unknown extent, or a corrupt file's -- has no count one
/// more than it. `n + 1` panicked in a debug build and wrapped to a count of
/// 0 in a release one; `None` makes the size unknown instead (#325).
pub fn subrange_count(count: Option<u64>, upper_bound: Option<u64>) -> Option<u64> {
    match count {
        Some(n) => Some(n),
        None => upper_bound?.checked_add(1),
    }
}

/// The size in bytes of the type at `off`, following the qualifier and
/// typedef chain, and computing an array's extent from its element type and
/// subrange count when the array itself does not state one.
///
/// Bounded: `depth` limits BOTH the iterative qualifier chain and recursive
/// `DW_TAG_array_type` element-type lookups so a malformed or cyclic debug
/// section cannot spin or overflow the stack (#9.1).
fn type_size<R: gimli::Reader>(unit: &gimli::Unit<R>, off: gimli::UnitOffset<R::Offset>, mut depth: u8) -> gimli::Result<Option<u64>> {
    use gimli::AttributeValue;
    let mut at = off;
    while depth > 0 {
        depth -= 1;
        let die = unit.entry(at)?;
        if let Some(n) = udata(die.attr_value(gimli::DW_AT_byte_size)?) {
            return Ok(Some(n));
        }
        if die.tag() == gimli::DW_TAG_array_type {
            // An array DIE with no children has no subrange bounds: do not let
            // `next_dfs` advance into its siblings and steal another array's
            // subrange (#9.6).
            if !die.has_children() {
                return Ok(None);
            }
            // element size x count. The count is either stated, or one more
            // than the highest index the subrange allows.
            let Some(AttributeValue::UnitRef(elem)) = die.attr_value(gimli::DW_AT_type)? else { return Ok(None) };
            let Some(esz) = type_size(unit, elem, depth)? else { return Ok(None) };
            // Every dimension, not the first. `int m[2][3]` is one array
            // type with two sibling subranges, and stopping at the first
            // gives 2 x 4 = 8 rather than 24 -- so an access to m[1][2]
            // lands outside the range and the variable reads as unused,
            // which is the false-unused result this whole feature exists to
            // avoid (review of #52).
            //
            // A dimension whose bound the debug info does not state makes
            // the whole extent unknown: a partial product would be a size
            // that is confidently wrong, and `None` at least says so.
            let mut kids = unit.entries_at_offset(at)?;
            kids.next_dfs()?;
            let mut rel_depth: isize = 0;
            let mut total: Option<u64> = None;
            while let Some((delta, k)) = kids.next_dfs()? {
                rel_depth += delta;
                if rel_depth <= 0 {
                    break;
                }
                if rel_depth != 1 || k.tag() != gimli::DW_TAG_subrange_type {
                    continue;
                }
                let n = subrange_count(udata(k.attr_value(gimli::DW_AT_count)?), udata(k.attr_value(gimli::DW_AT_upper_bound)?));
                match n {
                    // checked: a corrupt or hostile bound must not wrap into
                    // a small size that silently matches nothing
                    Some(n) => match total.unwrap_or(1).checked_mul(n) {
                        Some(t) => total = Some(t),
                        None => return Ok(None), // an absurd bound: no size, not a wrapped one
                    },
                    None => return Ok(None),
                }
            }
            return Ok(total.and_then(|c| esz.checked_mul(c)));
        }
        // a qualifier or alias: the size is whatever it wraps
        match die.attr_value(gimli::DW_AT_type)? {
            Some(AttributeValue::UnitRef(next)) => at = next,
            _ => return Ok(None),
        }
    }
    Ok(None)
}

#[derive(Debug, PartialEq, Eq)]
enum Sleb {
    Exact(i64),
    Trailing,
    Invalid,
}

/// A signed LEB128 that must consume all of `b`, as a single-op `DW_OP_fbreg`
/// / `DW_OP_breg*` expression does (#9.6).
fn sleb128_exact(b: &[u8]) -> Sleb {
    let (mut result, mut shift) = (0i64, 0u32);
    for (n, &byte) in b.iter().enumerate() {
        if n > 9 {
            return Sleb::Invalid;
        }
        // At n == 9 (shift == 63) only bit 63 remains: continuation bit must be
        // clear and upper 6 bits must match sign bit (0x00 or 0x7f).
        if n == 9 && byte != 0x00 && byte != 0x7f {
            return Sleb::Invalid;
        }
        result |= ((byte & 0x7f) as i64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 64 && byte & 0x40 != 0 {
                result |= -1i64 << shift;
            }
            return if n + 1 == b.len() { Sleb::Exact(result) } else { Sleb::Trailing };
        }
    }
    Sleb::Invalid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_op_locations_and_frame_bases_fail_closed() {
        // Plain single-op DW_OP_fbreg -16
        assert_eq!(decode_frame_base(&[0x6d]), Some(Base::Fp));
        assert_eq!(decode_frame_base(&[0x6f]), Some(Base::Sp));
        assert_eq!(decode_frame_base(&[0x6d, 0x06]), None, "trailing opcode in frame_base");
        assert_eq!(decode_location(&[0x91, 0x70], Some(Base::Fp)), Place::Frame(Base::Fp, -16));
        // DW_OP_fbreg -16, DW_OP_deref (by-ref param) must NOT be misread as Place::Frame
        assert!(matches!(decode_location(&[0x91, 0x70, 0x06], Some(Base::Fp)), Place::Elsewhere(_)));
        // DW_OP_fbreg -16, DW_OP_plus_uconst 8
        assert!(matches!(decode_location(&[0x91, 0x70, 0x23, 0x08], Some(Base::Fp)), Place::Elsewhere(_)));
        // DW_OP_breg31 8, DW_OP_piece 4
        assert!(matches!(decode_location(&[0x8f, 0x08, 0x93, 0x04], Some(Base::Sp)), Place::Elsewhere(_)));
        // 10th-byte SLEB128 overflow
        let mut bad = vec![0x91];
        bad.extend(std::iter::repeat_n(0x80u8, 9));
        bad.push(0x02);
        assert!(matches!(decode_location(&bad, Some(Base::Fp)), Place::Elsewhere(s) if s.contains("decoded")));
    }

    /// Build a minimal DWARF v4 unit with the given `.debug_abbrev` and
    /// `.debug_info` body (everything after the 11-byte CU header).
    fn test_dwarf(abbrev: Vec<u8>, body: Vec<u8>) -> (gimli::DwarfSections<Vec<u8>>, gimli::RunTimeEndian) {
        let mut info = Vec::new();
        let unit_len = (7 + body.len()) as u32;
        info.extend_from_slice(&unit_len.to_le_bytes());
        info.extend_from_slice(&4u16.to_le_bytes()); // version 4
        info.extend_from_slice(&0u32.to_le_bytes()); // debug_abbrev_offset
        info.push(8); // address_size
        info.extend_from_slice(&body);
        let sections = gimli::DwarfSections::load::<_, std::convert::Infallible>(|id| {
            Ok(match id {
                gimli::SectionId::DebugAbbrev => abbrev.clone(),
                gimli::SectionId::DebugInfo => info.clone(),
                _ => Vec::new(),
            })
        })
        .unwrap();
        (sections, gimli::RunTimeEndian::Little)
    }

    /// A cyclic `DW_TAG_array_type` whose `DW_AT_type` points to itself must
    /// terminate and yield `None`, never overflowing the stack (#9.1).
    #[test]
    fn cyclic_array_type_does_not_overflow_stack() {
        // Abbrev 1: DW_TAG_compile_unit (0x11), has children (1), no attrs
        // Abbrev 2: DW_TAG_array_type (0x01), has children (1), DW_AT_type (0x49) DW_FORM_ref4 (0x13)
        // Abbrev 3: DW_TAG_subrange_type (0x21), no children (0), DW_AT_count (0x37) DW_FORM_data1 (0x0b)
        let abbrev = vec![
            1, 0x11, 1, 0, 0, // CU
            2, 0x01, 1, 0x49, 0x13, 0, 0, // array_type
            3, 0x21, 0, 0x37, 0x0b, 0, 0, // subrange_type
            0,
        ];
        // CU header is 11 bytes. Byte 11 is CU DIE (abbrev 1).
        // Byte 12 is self-referential array_type DIE (offset 12 = 0x0c).
        let mut body = vec![1]; // CU
        body.push(2); // array_type at unit offset 12
        body.extend_from_slice(&12u32.to_le_bytes()); // DW_AT_type -> itself (12)
        body.extend_from_slice(&[3, 4]); // subrange count = 4
        body.push(0); // end array_type children
        body.push(0); // end CU children

        let (sections, endian) = test_dwarf(abbrev, body);
        let dwarf = sections.borrow(|s| gimli::EndianSlice::new(s.as_slice(), endian));
        let header = dwarf.units().next().unwrap().unwrap();
        let unit = dwarf.unit(header).unwrap();
        assert_eq!(type_size(&unit, gimli::UnitOffset(12), 16).unwrap(), None);
    }

    /// An unsized `DW_TAG_array_type` with no children (`DW_CHILDREN_no`) must
    /// return `None`, never walking into a sibling array's subrange (#9.6).
    #[test]
    fn childless_array_type_does_not_steal_sibling_subrange() {
        // Abbrev 1: CU (children=1)
        // Abbrev 2: DW_TAG_base_type (0x24), no children, DW_AT_byte_size (0x0b) DW_FORM_data1 (0x0b)
        // Abbrev 3: DW_TAG_array_type (0x01), NO children (0), DW_AT_type (0x49) DW_FORM_ref4 (0x13)
        // Abbrev 4: DW_TAG_array_type (0x01), HAS children (1), DW_AT_type (0x49) DW_FORM_ref4 (0x13)
        // Abbrev 5: DW_TAG_subrange_type (0x21), no children, DW_AT_count (0x37) DW_FORM_data1 (0x0b)
        let abbrev = vec![
            1, 0x11, 1, 0, 0, // CU
            2, 0x24, 0, 0x0b, 0x0b, 0, 0, // base_type
            3, 0x01, 0, 0x49, 0x13, 0, 0, // childless array_type
            4, 0x01, 1, 0x49, 0x13, 0, 0, // sized array_type
            5, 0x21, 0, 0x37, 0x0b, 0, 0, // subrange_type
            0,
        ];
        // Offsets:
        // 11: CU (1 byte)
        // 12: base_type byte_size=4 (2 bytes)
        // 14: childless array_type -> base_type 12 (5 bytes)
        // 19: sibling sized array_type -> base_type 12 (5 bytes) + subrange count=10 (2 bytes) + 0
        let mut body = vec![1];
        body.extend_from_slice(&[2, 4]); // offset 12: 4-byte scalar
        body.push(3); // offset 14: childless array
        body.extend_from_slice(&12u32.to_le_bytes());
        body.push(4); // offset 19: sibling array with subrange
        body.extend_from_slice(&12u32.to_le_bytes());
        body.extend_from_slice(&[5, 10]); // count = 10
        body.push(0); // end sibling array
        body.push(0); // end CU

        let (sections, endian) = test_dwarf(abbrev, body);
        let dwarf = sections.borrow(|s| gimli::EndianSlice::new(s.as_slice(), endian));
        let header = dwarf.units().next().unwrap().unwrap();
        let unit = dwarf.unit(header).unwrap();
        assert_eq!(type_size(&unit, gimli::UnitOffset(14), 16).unwrap(), None, "childless array has unknown size");
        assert_eq!(type_size(&unit, gimli::UnitOffset(19), 16).unwrap(), Some(40), "sibling array still resolves to 4 * 10");
    }
}
