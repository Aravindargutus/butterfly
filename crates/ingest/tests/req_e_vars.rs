// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests (area E): variables (parameters and locals).
//!
//! - matrix.toml:836-846 (feature.variables) "Parameters and locals with every
//!   access, in static order ... AArch64 only; on any other architecture the
//!   panel says why it is empty."
//! - matrix.toml:213 (c.union) "writing it as `float` and reading it as
//!   `uint32_t` shows as a write and a read of the same variable".
//! - matrix.toml:225 (c.bitfield) "a write to one field is the
//!   read-modify-write of the word that holds it: it shows as a read and a
//!   write of the struct (... as a local's accesses on AArch64)".
//! - matrix.toml:237 (c.varargs) "`va_start` takes the `va_list`'s address,
//!   so what `va_arg` reads through it is not attributed, and the variable
//!   says so."
//! - vars.rs:22-33 "Everything else is reported, not guessed ... An empty list
//!   that means 'we could not tell' must never read as 'it is never used'".
//! - vars.rs:41-46: an `[x29, #8]` access must not be matched against a
//!   variable at `[sp, #8]`.

mod common;
use butterfly_ingest::objectcode::RawInstr;
use butterfly_ingest::vars::{attribute, Base, Declared, Place};
use butterfly_model::Variable;
use common::ri;

fn var(name: &str, place: Place, size: Option<u64>) -> Declared {
    Declared { name: name.into(), kind: "local", decl_line: 3, type_name: Some("int".into()), place, size }
}

fn run(arch: &str, d: &[Declared], v: &[RawInstr]) -> Vec<Variable> {
    attribute(arch, d, v, &|a| Some(10 + (a / 4) as u32))
}

fn kinds(v: &Variable) -> Vec<(u64, &str)> {
    v.accesses.iter().map(|a| (a.addr, a.access.as_str())).collect()
}

/// feature.variables: "on any other architecture the panel says why it is
/// empty" -- every variable is listed, each with that reason.
#[test]
fn other_architectures_say_why_nothing_is_attributed() {
    let d = [var("a", Place::Frame(Base::Sp, 8), Some(4)), var("b", Place::Elsewhere("in a register"), None)];
    let r = run("x86_64", &d, &[ri(0, "movl", "%eax, 0x8(%rsp)")]);
    assert_eq!(r.len(), 2);
    for v in &r {
        assert!(v.accesses.is_empty());
        let why = v.unattributed.as_deref().unwrap_or_default();
        assert!(why.contains("x86_64") && why.contains("aarch64"), "{}: {why}", v.name);
    }
}

/// vars.rs:22-31: a variable not in a frame slot is listed with its reason.
#[test]
fn a_variable_elsewhere_is_listed_with_its_reason() {
    let r = run("aarch64", &[var("r", Place::Elsewhere("the optimiser keeps it in a register"), Some(4))], &[ri(0, "str", "w0, [sp, #0x8]")]);
    assert!(r[0].accesses.is_empty());
    assert_eq!(r[0].unattributed.as_deref(), Some("the optimiser keeps it in a register"));
}

/// vars.rs:18-21: a frame slot is attributed exactly, in static order, with
/// the line of each access.
#[test]
fn frame_slot_reads_and_writes_in_static_order() {
    let d = [var("x", Place::Frame(Base::Sp, 0xc), Some(4))];
    let v = [ri(0, "str", "w0, [sp, #0xc]"), ri(4, "ldr", "w8, [sp, #0x8]"), ri(8, "ldr", "w9, [sp, #0xc]"), ri(12, "add", "w9, w9, #1"), ri(16, "stur", "w9, [sp, #0xc]")];
    let r = run("aarch64", &d, &v);
    assert_eq!(kinds(&r[0]), vec![(0, "write"), (8, "read"), (16, "write")]);
    assert_eq!(r[0].accesses[1].line, Some(12));
    assert!(r[0].unattributed.is_none());
}

/// matrix.toml:213 (c.union): one object -- a float write and an integer read
/// of it are a write and a read of the same variable.
#[test]
fn a_union_written_as_float_and_read_as_int_is_one_variable() {
    let d = [var("u", Place::Frame(Base::Sp, 0x8), Some(4))];
    let r = run("aarch64", &d, &[ri(0, "str", "s0, [sp, #0x8]"), ri(4, "ldr", "w0, [sp, #0x8]")]);
    assert_eq!(kinds(&r[0]), vec![(0, "write"), (4, "read")]);
}

/// matrix.toml:225 (c.bitfield): a field write is a read-modify-write of the
/// word holding it -- a read and a write of the struct, wherever in the
/// struct the word is.
#[test]
fn a_bitfield_write_is_a_read_and_a_write_of_the_struct() {
    let d = [var("flags", Place::Frame(Base::Sp, 0x10), Some(8))];
    let v = [ri(0, "ldr", "w8, [sp, #0x14]"), ri(4, "bfi", "w8, w0, #3, #2"), ri(8, "str", "w8, [sp, #0x14]")];
    assert_eq!(kinds(&run("aarch64", &d, &v)[0]), vec![(0, "read"), (8, "write")]);
}

/// "a variable is a range, not an address" (vars.rs:99-102): an element
/// access at an offset inside it, and a pair store spanning two slots, are
/// accesses to every variable they overlap; an access just past the end is
/// not.
#[test]
fn accesses_are_matched_by_overlap_with_the_variable() {
    let arr = var("values", Place::Frame(Base::Sp, 0x10), Some(8));
    let next = var("n", Place::Frame(Base::Sp, 0x18), Some(4));
    let v = [ri(0, "str", "w1, [sp, #0x14]"), ri(4, "stp", "x0, x1, [sp, #0x10]"), ri(8, "ldrb", "w2, [sp, #0x17]"), ri(12, "ldr", "w3, [sp, #0x18]")];
    let r = run("aarch64", &[arr, next], &v);
    assert_eq!(kinds(&r[0]), vec![(0, "write"), (4, "write"), (8, "read")]);
    assert_eq!(kinds(&r[1]), vec![(4, "write"), (12, "read")], "the stp's second register lands on n");
}

/// vars.rs:41-46: the base register is part of the slot. A frame-pointer
/// access at the same offset is a different slot.
#[test]
fn the_base_register_is_part_of_the_slot() {
    let sp_var = var("s", Place::Frame(Base::Sp, 8), Some(4));
    let fp_var = var("f", Place::Frame(Base::Fp, -4), Some(4));
    let v = [ri(0, "str", "w0, [x29, #0x8]"), ri(4, "stur", "w1, [x29, #-0x4]"), ri(8, "ldr", "w2, [sp, #0x8]"), ri(12, "ldr", "w3, [sp, #-0x4]")];
    let r = run("aarch64", &[sp_var, fp_var], &v);
    assert_eq!(kinds(&r[0]), vec![(8, "read")]);
    assert_eq!(kinds(&r[1]), vec![(4, "write")]);
}

/// matrix.toml:237 (c.varargs) "the variable says so": its address taken
/// (`add x0, sp, #N`), it is marked incomplete rather than looking
/// written-never-read.
#[test]
fn an_address_taken_variable_says_its_list_is_incomplete() {
    let d = [var("ap", Place::Frame(Base::Sp, 0x20), Some(8))];
    let v = [ri(0, "add", "x8, sp, #0x20"), ri(4, "str", "x8, [sp]"), ri(8, "ldr", "x9, [sp, #0x20]")];
    let r = run("aarch64", &d, &v);
    assert_eq!(kinds(&r[0]), vec![(0, "address"), (8, "read")]);
    assert!(r[0].unattributed.as_deref().is_some_and(|s| s.contains("address is taken")), "{:?}", r[0].unattributed);
    // `sub x0, x29, #4` takes the address of a frame-pointer slot
    let d = [var("p", Place::Frame(Base::Fp, -4), Some(4))];
    let r = run("aarch64", &d, &[ri(0, "sub", "x0, x29, #0x4")]);
    assert_eq!(kinds(&r[0]), vec![(0, "address")]);
    // and moving the frame (`add sp, sp, #N`) is not taking an address
    let d = [var("q", Place::Frame(Base::Sp, 0x10), Some(4))];
    let r = run("aarch64", &d, &[ri(0, "add", "sp, sp, #0x10")]);
    assert!(r[0].accesses.is_empty());
}

/// vars.rs:192-200: a writeback through the variable's own base register
/// ends attribution after it, and the variable says why; one through another
/// register does not.
#[test]
fn a_writeback_of_the_frame_base_stops_attribution() {
    let d = [var("x", Place::Frame(Base::Sp, 0x8), Some(4))];
    let v = [ri(0, "ldr", "w0, [sp, #0x8]"), ri(4, "str", "x30, [sp, #-0x10]!"), ri(8, "ldr", "w1, [sp, #0x8]")];
    let r = run("aarch64", &d, &v);
    assert_eq!(kinds(&r[0]), vec![(0, "read")]);
    assert!(r[0].unattributed.as_deref().is_some_and(|s| s.contains("writes back")), "{:?}", r[0].unattributed);
    // post-indexed through sp, too
    let v = [ri(0, "ldr", "x30, [sp], #0x10"), ri(4, "ldr", "w1, [sp, #0x8]")];
    assert!(run("aarch64", &d, &v)[0].accesses.is_empty());
    // a pointer walk through x1 says nothing about the frame
    let v = [ri(0, "ldr", "x2, [x1], #0x8"), ri(4, "ldr", "w1, [sp, #0x8]")];
    let r = run("aarch64", &d, &v);
    assert_eq!(kinds(&r[0]), vec![(4, "read")]);
    assert!(r[0].unattributed.is_none());
}

/// vars.rs:76-79: with no size from the debug info only the first byte is
/// recognised, and the variable says so.
#[test]
fn a_sizeless_variable_says_so() {
    let d = [var("blob", Place::Frame(Base::Sp, 0x10), None)];
    let r = run("aarch64", &d, &[ri(0, "ldr", "w0, [sp, #0x10]"), ri(4, "ldr", "w0, [sp, #0x14]")]);
    assert_eq!(kinds(&r[0]), vec![(0, "read")]);
    assert!(r[0].unattributed.as_deref().is_some_and(|s| s.contains("no size")), "{:?}", r[0].unattributed);
}

/// Register-indexed and pre-indexed operands are not frame slots
/// (vars.rs:133-137).
#[test]
fn addressing_modes_that_depend_on_a_register_are_not_slots() {
    let d = [var("x", Place::Frame(Base::Sp, 0x0), Some(8))];
    let r = run("aarch64", &d, &[ri(0, "ldr", "x0, [sp, x1]"), ri(4, "ldr", "x0, [sp, x1, lsl #3]")]);
    assert!(r[0].accesses.is_empty(), "{:?}", r[0].accesses);
}

/// Fail closed on hostile DWARF: a frame offset at the edge of i64, or an
/// immediate there, is not a slot anything reaches -- and not a panic.
#[test]
fn offsets_at_the_edge_do_not_panic() {
    let d = [var("x", Place::Frame(Base::Sp, i64::MAX - 2), Some(16))];
    let r = run("aarch64", &d, &[ri(0, "ldr", "x0, [sp, #0x8]")]);
    assert!(r[0].accesses.is_empty());
}

/// The other edge: an operand immediate at the top of i64 is far from any
/// slot and is matched without overflow.
#[test]
fn an_immediate_at_the_edge_is_not_a_slot() {
    let d = [var("y", Place::Frame(Base::Sp, 8), Some(4))];
    let r = run("aarch64", &d, &[ri(0, "ldr", "x0, [sp, #0x7fffffffffffffff]"), ri(4, "ldr", "x0, [sp, #0xffffffffffffffff]")]);
    assert!(r[0].accesses.is_empty());
}

/// An array dimension's element count, from a DWARF subrange: `DW_AT_count`
/// where stated, else one more than `DW_AT_upper_bound`. Decided in #325
/// (ambiguity 13): a bound of `u64::MAX` -- which GCC writes as `-1` for an
/// array of unknown extent, and which a corrupt file can say of anything --
/// has no count one more than it, and the size fails closed (unknown, which
/// the variable reports as "no size", above) rather than overflowing: a
/// debug build panicked, and a release build wrapped the count to 0, a size
/// that is confidently wrong. Checked through the helper `type_size` uses,
/// since a compiler will not emit such a bound for a test to read.
#[test]
fn a_subrange_bound_at_the_top_of_u64_has_no_count() {
    use butterfly_ingest::vars::subrange_count;
    assert_eq!(subrange_count(None, Some(u64::MAX)), None, "u64::MAX + 1 does not exist");
    assert_eq!(subrange_count(None, Some(u64::MAX - 1)), Some(u64::MAX), "the largest bound that has a count");
    assert_eq!(subrange_count(None, Some(9)), Some(10), "`int a[10]` states upper bound 9");
    assert_eq!(subrange_count(Some(3), None), Some(3), "a stated count is the count");
    assert_eq!(subrange_count(Some(3), Some(u64::MAX)), Some(3), "and wins over the bound");
    assert_eq!(subrange_count(None, None), None, "no bound: unknown extent");
}
