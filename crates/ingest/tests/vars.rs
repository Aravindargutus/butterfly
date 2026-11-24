// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Parameters and locals, attributed from DWARF and the disassembly (#52).
//!
//! The rule under test everywhere here: an access is attributed only when
//! the instruction's operand provably names the variable's frame slot, and
//! everything else is *reported*, never guessed. An empty access list that
//! means "we could not tell" must carry the reason -- silence is reserved
//! for a variable the code genuinely never touches.

use butterfly_ingest::objectcode::RawInstr;
use butterfly_ingest::vars::{attribute, Base, Declared, Place};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn ins(addr: u64, mnemonic: &str, operands: &str) -> RawInstr {
    RawInstr { addr, mnemonic: mnemonic.into(), operands: operands.into() }
}
/// A four-byte scalar unless a test says otherwise -- the common case, and
/// the one where a size makes attribution exact rather than approximate.
fn decl(name: &str, place: Place) -> Declared {
    sized(name, place, Some(4))
}
fn sized(name: &str, place: Place, size: Option<u64>) -> Declared {
    Declared { name: name.into(), kind: "local", decl_line: 7, type_name: Some("int32_t".into()), place, size }
}
fn no_lines(_: u64) -> Option<u32> {
    None
}

/// A fresh scratch directory under a random name, so a previous run cannot
/// leak into this one -- the same idiom as the other suites here (#344).
fn tmp(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("vars-{tag}"))
}

/// Order is the point of the whole feature, and it is a *static* one: the
/// accesses must come out in the order the instructions performing them
/// appear in the image, which is address order. This scans that list once,
/// so what it records is where a variable is touched and never when -- a
/// site in a loop is emitted once, whatever the loop does at run time.
#[test]
fn accesses_come_out_in_instruction_order() {
    let d = [decl("acc", Place::Frame(Base::Sp, 12))];
    let i = [
        ins(0x100, "str", "w0, [sp, #12]"),
        ins(0x104, "ldr", "w8, [sp, #12]"),
        ins(0x108, "add", "w8, w8, #1"), // not a memory access at all
        ins(0x10c, "str", "w8, [sp, #12]"),
    ];
    let v = &attribute("aarch64", &d, &i, &|a| if a == 0x104 { Some(9) } else { None })[0];
    let seq: Vec<(&str, u64)> = v.accesses.iter().map(|a| (a.access.as_str(), a.addr)).collect();
    assert_eq!(seq, [("write", 0x100), ("read", 0x104), ("write", 0x10c)]);
    // and the line table's attribution rides along where it exists
    assert_eq!(v.accesses[1].line, Some(9));
    assert_eq!(v.accesses[0].line, None);
    assert!(v.unattributed.is_none());
}

/// `[sp, #8]` and `[x29, #8]` are different slots. A variable based on one
/// register must never collect the other's accesses -- crediting them would
/// attribute a touch to the wrong variable, which is worse than none.
#[test]
fn the_base_register_is_part_of_the_slot_identity() {
    let d = [decl("a", Place::Frame(Base::Sp, 8)), decl("b", Place::Frame(Base::Fp, 8))];
    let i = [ins(0x100, "str", "w0, [sp, #8]"), ins(0x104, "stur", "w1, [x29, #8]")];
    let vs = attribute("aarch64", &d, &i, &no_lines);
    assert_eq!(vs[0].accesses.iter().map(|a| a.addr).collect::<Vec<_>>(), [0x100], "sp-based var takes only the sp access");
    assert_eq!(vs[1].accesses.iter().map(|a| a.addr).collect::<Vec<_>>(), [0x104], "fp-based var takes only the x29 access");
}

/// Negative offsets are the common case off a frame pointer -- locals sit
/// below x29 -- and hex immediates are how llvm-objdump prints larger ones.
#[test]
fn negative_and_hex_offsets_resolve() {
    let d = [decl("lo", Place::Frame(Base::Fp, -4)), decl("hi", Place::Frame(Base::Sp, 0x10))];
    let i = [ins(0x100, "stur", "w0, [x29, #-4]"), ins(0x104, "ldr", "w8, [sp, #0x10]")];
    let vs = attribute("aarch64", &d, &i, &no_lines);
    assert_eq!(vs[0].accesses.len(), 1, "negative offset");
    assert_eq!(vs[1].accesses.len(), 1, "hex offset");
}

/// A pre-indexed `[sp, #16]!` moves the base register itself; the slot it
/// touches depends on a value this reader does not track. Matching it as a
/// plain frame slot would attribute an access to the wrong variable, so it
/// must not match at all.
#[test]
fn indexed_addressing_is_not_forced_into_a_frame_slot() {
    let d = [decl("v", Place::Frame(Base::Sp, 16))];
    let i = [
        ins(0x100, "stp", "x29, x30, [sp, #16]!"), // pre-indexed
        ins(0x104, "ldr", "w8, [sp, x1]"),         // register-indexed
    ];
    let v = &attribute("aarch64", &d, &i, &no_lines)[0];
    assert!(v.accesses.is_empty(), "{:?}", v.accesses);
}

/// A variable the compiler kept in a register has no frame slot to watch.
/// It is still listed -- it exists -- and the empty access list carries the
/// reason, because an unexplained empty list would read as "never used".
#[test]
fn a_register_variable_is_reported_not_guessed() {
    let d = [decl("r", Place::Elsewhere("the compiler kept it in a register, which has no frame slot to watch"))];
    let i = [ins(0x100, "str", "w0, [sp, #8]")];
    let v = &attribute("aarch64", &d, &i, &no_lines)[0];
    assert!(v.accesses.is_empty());
    assert!(v.unattributed.as_deref().unwrap().contains("register"), "{:?}", v.unattributed);
}

/// Only AArch64's operand grammar is read. On any other image every
/// variable says so, instead of presenting an empty list that would look
/// like dead data.
#[test]
fn another_architecture_is_named_not_silently_empty() {
    let d = [decl("v", Place::Frame(Base::Sp, 8))];
    let i = [ins(0x100, "movl", "%eax, 8(%rsp)")];
    let v = &attribute("x86_64", &d, &i, &no_lines)[0];
    assert!(v.accesses.is_empty());
    let why = v.unattributed.as_deref().unwrap();
    assert!(why.contains("aarch64") && why.contains("x86_64"), "{why}");
}

/// The case above never happens on a real x86-64 image. `declared` reads
/// `DW_AT_frame_base` as AArch64 register numbers, so an x86-64 function's
/// `DW_OP_reg6` (rbp) base is not recognised and every one of its variables
/// arrives as `Elsewhere("its frame base is an expression this reader does
/// not evaluate")` -- which is what an x86-64 build of examples/c-lib showed
/// (#172). The architecture is the reason nothing is attributed, and the
/// reader must be told that, not a DWARF detail that suggests some other
/// image would fare better: on another architecture no place is read at all,
/// so the architecture wins over whatever the place says.
#[test]
fn another_architecture_is_the_reason_whatever_dwarf_said() {
    let d = [
        decl("framed", Place::Elsewhere("its frame base is an expression this reader does not evaluate")),
        decl("in_reg", Place::Elsewhere("the compiler kept it in a register, which has no frame slot to watch")),
    ];
    let i = [ins(0x100, "movl", "%edi, -0x4(%rbp)")];
    for v in attribute("x86_64", &d, &i, &no_lines) {
        assert!(v.accesses.is_empty(), "{}", v.name);
        assert_eq!(v.unattributed.as_deref(), Some("frame-slot accesses are read for aarch64; this image is x86_64"), "{}", v.name);
    }
}

/// A frame variable no instruction touches has an empty list and *no*
/// excuse: that combination is the honest statement "it exists and nothing
/// touches it", and it must stay distinguishable from "we could not tell".
#[test]
fn genuinely_untouched_is_distinguishable_from_unattributable() {
    let d = [decl("unused", Place::Frame(Base::Sp, 24))];
    let i = [ins(0x100, "str", "w0, [sp, #8]")];
    let v = &attribute("aarch64", &d, &i, &no_lines)[0];
    assert!(v.accesses.is_empty());
    assert!(v.unattributed.is_none(), "an untouched variable needs no excuse: {:?}", v.unattributed);
}

// ---------------------------------------------------------------- fixture --

/// The reason every variable on a non-AArch64 image carries, word for word.
fn foreign_reason(arch: &str) -> String {
    format!("frame-slot accesses are read for aarch64; this image is {arch}")
}

/// End to end on the C example: DWARF read from the dSYM (or the ELF
/// itself), attribution against the real disassembly, carried into the
/// evidence. Skipped, not failed, when the fixture has not been built.
///
/// What is true depends on the architecture the example was built for, and
/// both answers are asserted rather than one being skipped (#172). The
/// example is built by the host's compiler, so the image must be the host's
/// architecture -- checked first, so a fixture built for something else
/// fails loudly instead of quietly selecting the other branch. On AArch64
/// the accesses are attributed and ordered; everywhere else the variables
/// are still all there, with no accesses and the reason naming the
/// architecture. That second branch is what hosted CI, on x86-64, runs.
#[test]
fn demo_c_carries_variables_with_ordered_accesses() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c");
    if !root.join("build/demo").exists() {
        butterfly_testsupport::fixture_missing("examples/c/build not present (run examples/c/build.sh)");
        return;
    }
    let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts.source_root = Some(root.clone());
    let ev = butterfly_ingest::ingest(&opts).expect("ingest");
    let arch = ev.image.as_ref().expect("an analysed image").arch.clone();
    assert_eq!(arch, std::env::consts::ARCH, "examples/c/build was not built for this host (rebuild it with examples/c/build.sh)");

    let f = ev.functions.iter().find(|f| f.name == "clamp_altitude").expect("clamp_altitude");
    let names: Vec<&str> = f.variables.iter().map(|v| v.name.as_str()).collect();
    for p in ["alt", "lo", "hi"] {
        assert!(names.contains(&p), "missing param {p} in {names:?}");
    }
    let cs = ev.functions.iter().find(|f| f.name == "checksum").expect("checksum");
    let acc = cs.variables.iter().find(|v| v.name == "acc").expect("local acc");
    assert_eq!(acc.kind, "local");

    if arch != "aarch64" {
        // Declared, listed, and honestly empty: every variable in the image
        // says why, and the why is the architecture -- not a DWARF detail,
        // and never silence that would read as "nothing touches it".
        let why = foreign_reason(&arch);
        let all: Vec<_> = ev.functions.iter().flat_map(|f| f.variables.iter().map(move |v| (f, v))).collect();
        assert!(!all.is_empty(), "the DWARF declarations are read on every architecture");
        for (f, v) in all {
            assert!(v.accesses.is_empty(), "{}::{} has accesses on {arch}: {:?}", f.name, v.name, v.accesses);
            assert_eq!(v.unattributed.as_deref(), Some(why.as_str()), "{}::{}", f.name, v.name);
        }
        for v in &f.variables {
            assert_eq!(v.kind, "param", "{}", v.name);
        }
        return;
    }

    for v in &f.variables {
        assert_eq!(v.kind, "param", "{}", v.name);
        // at -O0 every parameter is spilled before it is used: the first
        // attributed access must be that write, and a read must follow
        assert_eq!(v.accesses.first().map(|a| a.access.as_str()), Some("write"), "{} starts with its spill", v.name);
        assert!(v.accesses.iter().any(|a| a.access == "read"), "{} is read", v.name);
        // in object order, addresses strictly increase
        assert!(v.accesses.windows(2).all(|w| w[0].addr < w[1].addr), "{} out of order", v.name);
        // and each access knows its source line
        assert!(v.accesses.iter().all(|a| a.line.is_some()), "{} lost its lines", v.name);
    }

    // a local with a real read-modify-write life, not just parameters
    let kinds: Vec<&str> = acc.accesses.iter().map(|a| a.access.as_str()).collect();
    assert!(kinds.contains(&"read") && kinds.contains(&"write"), "{kinds:?}");
}

// ─────────────────────────── a variable is a range ───────────────────────

/// `int values[2]` is written at `values[1]`, four bytes past its own
/// offset. Matching the start offset alone found nothing and said nothing --
/// an empty list presented as complete, which is the one thing this module
/// promises never to do (review of #52).
#[test]
fn an_access_inside_a_variable_belongs_to_it() {
    let d = [sized("values", Place::Frame(Base::Sp, 16), Some(8))]; // int[2]
    let i = [
        ins(0x100, "str", "w8, [sp, #20]"), // values[1] = 42
        ins(0x104, "ldr", "w8, [sp, #20]"), // return values[1]
        ins(0x108, "str", "w9, [sp, #24]"), // the *next* variable, not this one
    ];
    let v = &attribute("aarch64", &d, &i, &no_lines)[0];
    let seq: Vec<(&str, u64)> = v.accesses.iter().map(|a| (a.access.as_str(), a.addr)).collect();
    assert_eq!(seq, [("write", 0x100), ("read", 0x104)], "both touches of values[1] belong to values");
    assert!(v.unattributed.is_none(), "a sized variable fully read needs no excuse: {:?}", v.unattributed);
}

/// `stp x0, x1, [sp, #8]` stores sixteen bytes: eight at sp+8 and eight at
/// sp+16. A variable in the second slot was reported untouched.
#[test]
fn a_paired_store_reaches_both_slots_it_writes() {
    let d = [sized("a", Place::Frame(Base::Sp, 8), Some(8)), sized("b", Place::Frame(Base::Sp, 16), Some(8))];
    let i = [ins(0x100, "stp", "x0, x1, [sp, #8]"), ins(0x104, "ldp", "x0, x1, [sp, #8]")];
    let vs = attribute("aarch64", &d, &i, &no_lines);
    assert_eq!(vs[0].accesses.len(), 2, "the first slot: {:?}", vs[0].accesses);
    assert_eq!(vs[1].accesses.len(), 2, "the second slot is written too, and was missed");
    assert_eq!(vs[1].accesses[0].access, "write");
    assert_eq!(vs[1].accesses[1].access, "read");
}

/// A one-byte store cannot reach a variable four bytes further on. Widening
/// the match must not start crediting neighbours.
#[test]
fn a_narrow_access_does_not_reach_the_next_variable() {
    let d = [sized("a", Place::Frame(Base::Sp, 8), Some(4)), sized("b", Place::Frame(Base::Sp, 12), Some(4))];
    let i = [ins(0x100, "strb", "w0, [sp, #8]")];
    let vs = attribute("aarch64", &d, &i, &no_lines);
    assert_eq!(vs[0].accesses.len(), 1, "the byte written is inside a");
    assert!(vs[1].accesses.is_empty(), "b is four bytes away and untouched: {:?}", vs[1].accesses);
}

/// Without a size only the first byte can be recognised. That is exact for a
/// scalar but incomplete for anything larger, and the variable must say so
/// rather than appear unused.
#[test]
fn an_unsized_variable_admits_what_it_could_not_see() {
    let d = [sized("blob", Place::Frame(Base::Sp, 16), None)];
    let i = [ins(0x100, "str", "w8, [sp, #20]")];
    let v = &attribute("aarch64", &d, &i, &no_lines)[0];
    assert!(v.accesses.is_empty());
    assert!(v.unattributed.as_deref().unwrap().contains("no size"), "{:?}", v.unattributed);
}

// ───────────────────────────── address escape ────────────────────────────

/// `sub x0, x29, #4` followed by a call hands the variable's address to
/// somebody else. Skipping every non-load/store made the write that callee
/// performs invisible -- leaving a record showing a read and no write, which
/// reads as a use-before-set bug that is not there.
#[test]
fn taking_the_address_is_recorded_and_the_rest_admitted() {
    let d = [decl("value", Place::Frame(Base::Fp, -4))];
    let i = [
        ins(0x100, "sub", "x0, x29, #4"), // &value
        ins(0x104, "bl", "_initialise"),  // which writes through it
        ins(0x108, "ldur", "w0, [x29, #-4]"),
    ];
    let v = &attribute("aarch64", &d, &i, &no_lines)[0];
    let seq: Vec<&str> = v.accesses.iter().map(|a| a.access.as_str()).collect();
    assert_eq!(seq, ["address", "read"], "the address formation is a touch in its own right");
    let why = v.unattributed.as_deref().expect("and the record is not complete");
    assert!(why.contains("address is taken"), "{why}");
}

/// `add sp, sp, #32` is the frame being torn down, not an address being
/// taken. Reading it as an escape would mark every function's locals
/// unattributed and make the caveat worthless.
#[test]
fn adjusting_the_frame_is_not_taking_an_address() {
    let d = [decl("v", Place::Frame(Base::Sp, 8))];
    let i = [ins(0x100, "sub", "sp, sp, #32"), ins(0x104, "str", "w0, [sp, #8]"), ins(0x108, "add", "sp, sp, #32")];
    let v = &attribute("aarch64", &d, &i, &no_lines)[0];
    assert_eq!(v.accesses.len(), 1, "only the store: {:?}", v.accesses);
    assert!(v.unattributed.is_none(), "no escape happened: {:?}", v.unattributed);
}

// ──────────────────────────── writeback ──────────────────────────────────

/// `ldr x0, [sp], #16` is post-indexed: it reads sp+0 *and* moves sp. The
/// operand splitter yields `[sp]` and `#16` separately, so the bracketed
/// part alone looks like a plain offset of zero -- and every offset after it
/// named a slot that had moved. Two reads were reported where the second
/// touched something else entirely.
#[test]
fn a_post_indexed_access_stops_attribution_rather_than_lying() {
    let d = [decl("v", Place::Frame(Base::Sp, 0))];
    let i = [ins(0x100, "ldr", "x0, [sp], #16"), ins(0x104, "ldr", "x1, [sp]")];
    let v = &attribute("aarch64", &d, &i, &no_lines)[0];
    assert!(v.accesses.is_empty(), "nothing from the writeback on is trustworthy: {:?}", v.accesses);
    let why = v.unattributed.as_deref().expect("and it must say why");
    assert!(why.contains("writes back the frame base"), "{why}");
}

/// The pre-indexed form does the same thing and must be treated the same.
#[test]
fn a_pre_indexed_access_also_stops_attribution() {
    let d = [decl("v", Place::Frame(Base::Sp, 8))];
    let i = [ins(0x100, "stp", "x29, x30, [sp, #-16]!"), ins(0x104, "str", "w0, [sp, #8]")];
    let v = &attribute("aarch64", &d, &i, &no_lines)[0];
    assert!(v.accesses.is_empty(), "{:?}", v.accesses);
    assert!(v.unattributed.as_deref().unwrap().contains("writes back the frame base"));
}

/// `int matrix[2][3]` is one array type with two sibling subranges. Taking
/// only the first gives 2 x 4 = 8 rather than 24, so an access to
/// `matrix[1][2]` lands outside the range and the variable reads as unused
/// again -- the very result the size work was added to prevent (review of
/// #52).
#[test]
fn every_dimension_counts_towards_the_range() {
    // 24 bytes at sp+8; matrix[1][2] is at sp+8+20
    let d = [sized("matrix", Place::Frame(Base::Sp, 8), Some(24))];
    let i = [ins(0x100, "str", "w8, [sp, #28]"), ins(0x104, "ldr", "w8, [sp, #28]")];
    let v = &attribute("aarch64", &d, &i, &no_lines)[0];
    assert_eq!(v.accesses.len(), 2, "both touches of matrix[1][2] belong to matrix");
    assert!(v.unattributed.is_none(), "{:?}", v.unattributed);

    // ...and one past the end still does not
    let past = [ins(0x100, "str", "w8, [sp, #32]")];
    assert!(attribute("aarch64", &d, &past, &no_lines)[0].accesses.is_empty(), "sp+32 is beyond the array");
}

/// `ldr x0, [x1], #8` walks a pointer. It says nothing about the frame, and
/// treating it as a frame writeback erased every stack variable's evidence
/// in any function that walks one -- valid accesses replaced by a caveat
/// about a base that never moved.
#[test]
fn a_writeback_through_another_register_leaves_the_frame_alone() {
    let d = [decl("v", Place::Frame(Base::Sp, 8))];
    let i = [
        ins(0x100, "ldr", "x0, [x1], #8"), // pointer walking, not the frame
        ins(0x104, "str", "w0, [sp, #8]"),
        ins(0x108, "ldr", "w0, [sp, #8]"),
    ];
    let v = &attribute("aarch64", &d, &i, &no_lines)[0];
    let seq: Vec<&str> = v.accesses.iter().map(|a| a.access.as_str()).collect();
    assert_eq!(seq, ["write", "read"], "x1 moving says nothing about sp");
    assert!(v.unattributed.is_none(), "no frame base moved: {:?}", v.unattributed);
}

/// A writeback through sp must not truncate a variable measured from x29,
/// and the reverse. They are different bases and only one of them moved.
#[test]
fn a_writeback_truncates_only_the_base_it_writes() {
    let d = [decl("on_sp", Place::Frame(Base::Sp, 8)), decl("on_fp", Place::Frame(Base::Fp, -8))];
    let i = [
        ins(0x100, "stur", "w0, [x29, #-8]"),
        ins(0x104, "ldr", "x2, [sp], #16"), // sp moves here
        ins(0x108, "stur", "w1, [x29, #-8]"),
    ];
    let vs = attribute("aarch64", &d, &i, &no_lines);
    assert!(vs[0].accesses.is_empty(), "the sp-based one stops at the writeback");
    assert!(vs[0].unattributed.as_deref().unwrap().contains("writes back the frame base"));
    assert_eq!(vs[1].accesses.len(), 2, "the x29-based one is untouched by an sp writeback");
    assert!(vs[1].unattributed.is_none(), "{:?}", vs[1].unattributed);
}

/// The sizes above are handed in directly, so they test the matching but not
/// the *reading*. This one compiles real DWARF and asks `declared()` what it
/// makes of it -- which is where the single-subrange bug actually lived.
/// Skipped, not failed, without a compiler.
#[test]
fn a_multidimensional_array_is_measured_from_real_dwarf() {
    let Some((bin, dsym)) = compile_fixture(
        "matrix",
        "int matrix_case(void) { int matrix[2][3]; matrix[1][2] = 42; return matrix[1][2]; }\n\
         int main(void) { return matrix_case(); }\n",
    ) else {
        return;
    };

    let bytes = std::fs::read(&dsym).expect("dSYM");
    let file = object::File::parse(&*bytes).expect("parse dSYM");
    let decl = butterfly_ingest::vars::declared(&file).expect("declared");

    let m = decl.values().flatten().find(|d| d.name.as_ref() == "matrix").expect("matrix is declared");
    // 2 x 3 ints: both subranges, not just the first (which would be 8)
    assert_eq!(m.size, Some(24), "every dimension counts");

    // and it carries through: the write and read of matrix[1][2] are found
    let lim = butterfly_model::limits::Limits::from_env();
    let b = butterfly_ingest::objectcode::load(&bin, None, &lim).expect("load");
    let sym = b.symbols.iter().find(|s| s.name.contains("matrix_case")).expect("symbol");
    let ins = b.instrs_in(sym.address, sym.address + sym.size).to_vec();
    let d = decl.get(&sym.address).map(|v| v.as_slice()).unwrap_or(&[]);
    let v = attribute(&b.arch, d, &ins, &|a| b.line_at(a).map(|r| r.line)).into_iter().find(|v| v.name == "matrix").expect("attributed");
    // The size above is read from DWARF on every architecture; the accesses
    // only on AArch64, and anywhere else the reason says so (#172).
    assert_eq!(b.arch, std::env::consts::ARCH, "the host compiler builds for the host");
    if b.arch != "aarch64" {
        assert!(v.accesses.is_empty(), "{:?}", v.accesses);
        assert_eq!(v.unattributed, Some(foreign_reason(&b.arch)));
        return;
    }
    assert!(v.accesses.iter().any(|a| a.access == "write"), "matrix[1][2] is written: {:?}", v.accesses);
    assert!(v.accesses.iter().any(|a| a.access == "read"), "and read: {:?}", v.accesses);
    assert!(v.unattributed.is_none(), "nothing to admit here: {:?}", v.unattributed);
}

/// Compile a C snippet to a binary with a .dSYM beside it, or `None` when
/// the toolchain is not here. Two steps on purpose: a single-step compile
/// and link leaves the debug map pointing at a temporary object that is
/// already gone, and dsymutil then writes an empty dSYM.
fn compile_fixture(tag: &str, src: &str) -> Option<(PathBuf, PathBuf)> {
    let cc = std::env::var("CC").unwrap_or_else(|_| "clang".into());
    if Command::new(&cc).arg("--version").output().is_err() {
        butterfly_testsupport::not_applicable(&format!("no {cc}"));
        return None;
    }
    let d = tmp(tag);
    let (c, o, bin) = (d.join("t.c"), d.join("t.o"), d.join("t"));
    fs::write(&c, src).ok()?;
    let ok = |st: std::process::ExitStatus| st.success().then_some(());
    ok(Command::new(&cc).args(["-O0", "-g", "-c"]).arg(&c).arg("-o").arg(&o).status().ok()?)?;
    ok(Command::new(&cc).args(["-O0", "-g"]).arg(&o).arg("-o").arg(&bin).status().ok()?)?;
    // macOS keeps the line table behind a debug map until dsymutil links it
    let dsym = bin.with_extension("dSYM").join("Contents/Resources/DWARF/t");
    if Command::new("dsymutil").arg(&bin).status().map(|s| s.success()).unwrap_or(false) && dsym.exists() {
        Some((bin, dsym))
    } else {
        // elsewhere the DWARF is in the binary itself
        Some((bin.clone(), bin))
    }
}
