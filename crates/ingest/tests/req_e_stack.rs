// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests (area E): stack usage.
//!
//! - README.md:238-241 "Frame sizes come from a `.stack_sizes` section ...,
//!   from `.su` files ..., or -- with no extra flags -- are estimated from
//!   the function prologue in the disassembly. The source is shown next to
//!   every number."
//! - README.md:242-245 "Worst case = own frame + deepest chain of *direct*
//!   calls ... Recursion, indirect calls and dynamic frames are reported as
//!   'not bounded' with the reason; nothing is silently capped."
//! - docs/butterfly.html, The other analyses / Stack: "Recursion, indirect
//!   calls and dynamic frames are reported unbounded -- never estimated."
//! - matrix.toml:189 (c.dynamic-frame): "A VLA and an alloca are reported not
//!   bounded, reason \"dynamic frame (alloca / VLA)\", and so is every caller
//!   above them ... The body scan knows clang's AArch64, x86-64, Arm and
//!   RISC-V shapes and fails closed".

mod common;
use butterfly_ingest::stack::{allocates_dynamically, frame_from_prologue, frame_unknown, parse_su, su_lookup, worst_case, StackNode};
use butterfly_model::FrameSource;
use common::ri;

fn node(frame: u64, callees: &[usize]) -> StackNode {
    StackNode { frame, dynamic: false, callees: callees.to_vec(), indirect_calls: 0, unknown: false }
}

// ------------------------------------------------------------- prologue

/// README.md:240 "estimated from the function prologue": x86-64 pushes are 8
/// bytes each, plus the `sub $N, %rsp`; the scan stops at the first
/// control transfer, and the source says it is a prologue estimate.
#[test]
fn x86_64_prologue_estimate() {
    let f = frame_from_prologue(
        "x86_64",
        &[ri(0, "pushq", "%rbp"), ri(1, "movq", "%rsp, %rbp"), ri(4, "pushq", "%rbx"), ri(5, "subq", "$0x28, %rsp"), ri(9, "callq", "0x100 <g>"), ri(14, "retq", "")],
    );
    assert_eq!((f.bytes, f.source, f.dynamic), (8 + 8 + 0x28, FrameSource::Prologue, false));
    // a stack allocation *after* a call (e.g. stack probe) must fail closed, not understate
    let post = frame_from_prologue(
        "x86_64",
        &[ri(0, "pushq", "%rbp"), ri(1, "movq", "%rsp, %rbp"), ri(4, "pushq", "%rbx"), ri(5, "subq", "$0x28, %rsp"), ri(9, "callq", "0x100 <g>"), ri(14, "subq", "$0x1000, %rsp")],
    );
    assert_eq!(post.source, FrameSource::Unknown);
    // `enter $N, $0` allocates N plus the saved frame pointer
    let f = frame_from_prologue("x86_64", &[ri(0, "enter", "$0x20, $0x0"), ri(4, "retq", "")]);
    assert_eq!(f.bytes, 0x28);
    // a register-sized `sub %rax, %rsp` is a run-time size
    let f = frame_from_prologue("x86_64", &[ri(0, "pushq", "%rbp"), ri(1, "subq", "%rax, %rsp")]);
    assert!(f.dynamic, "{f:?}");
}

/// AArch64: a pre-indexed `stp ... [sp, #-N]!` and `sub sp, sp, #N` add up;
/// a leaf with no frame is zero.
#[test]
fn aarch64_prologue_estimate() {
    let f = frame_from_prologue("aarch64", &[ri(0, "stp", "x29, x30, [sp, #-0x20]!"), ri(4, "mov", "x29, sp"), ri(8, "sub", "sp, sp, #0x30"), ri(12, "bl", "0x100")]);
    assert_eq!((f.bytes, f.source, f.dynamic), (0x50, FrameSource::Prologue, false));
    let f = frame_from_prologue("aarch64", &[ri(0, "add", "w0, w0, #1"), ri(4, "ret", "")]);
    assert_eq!((f.bytes, f.dynamic), (0, false));
    let f = frame_from_prologue("aarch64", &[ri(0, "sub", "sp, sp, x8")]);
    assert!(f.dynamic);
}

/// The Arm and RISC-V prologue shapes the matrix names (matrix.toml:189).
#[test]
fn arm_and_riscv_prologue_estimates() {
    let f = frame_from_prologue("arm", &[ri(0, "push", "{r4, r5, r7, lr}"), ri(2, "vpush", "{d8-d9}"), ri(6, "sub", "sp, #0x10"), ri(8, "bl", "0x100")]);
    assert_eq!(f.bytes, 4 * 4 + 8 * 2 + 0x10);
    let f = frame_from_prologue("riscv64", &[ri(0, "addi", "sp, sp, -0x20"), ri(4, "sd", "ra, 0x18(sp)"), ri(8, "call", "0x100")]);
    assert_eq!(f.bytes, 0x20);
}

/// stack.rs:116-117 / StackNode.unknown: an architecture the scan does not know
/// has an Unknown frame, which must fail closed (not read as 0 bytes).
#[test]
fn an_unknown_architecture_has_an_unknown_frame() {
    let f = frame_from_prologue("mips", &[ri(0, "addiu", "sp, sp, -32")]);
    assert_eq!(f.source, FrameSource::Unknown);
    assert!(frame_unknown(f.source, false));
    // and a prologue estimate standing in for a malformed .stack_sizes is unknown too
    assert!(frame_unknown(FrameSource::Prologue, true));
    assert!(!frame_unknown(FrameSource::Prologue, false));
}

/// Fail closed on hostile immediates: frames that sum past u64 must not
/// panic. (The worst-case sum is checked -- stack.rs:383-396 -- and the
/// prologue sum must be too.)
#[test]
fn huge_prologue_immediates_do_not_panic() {
    // and the sum that does not fit is unknown, never wrapped to a small frame
    for f in [
        frame_from_prologue("aarch64", &[ri(0, "sub", "sp, sp, #0xffffffffffffffff"), ri(4, "sub", "sp, sp, #0x10")]),
        frame_from_prologue("x86_64", &[ri(0, "enter", "$0xffffffffffffffff, $0x0")]),
        frame_from_prologue("x86_64", &[ri(0, "subq", "$0xffffffffffffffff, %rsp"), ri(4, "pushq", "%rbp")]),
    ] {
        assert_eq!(f.source, FrameSource::Unknown, "{f:?}");
    }
}

// ------------------------------------------------------- dynamic frames

/// matrix.toml:189 "the run-time `sp` write is recognised in the function
/// body ... its x86-64 shape was checked on clang's disassembly". Restoring
/// the stack pointer from the frame pointer is an epilogue, not an
/// allocation.
#[test]
fn x86_64_dynamic_allocation_in_the_body() {
    for (m, o) in [("movq", "%rax, %rsp"), ("subq", "%rcx, %rsp"), ("leaq", "-0x10(%rax), %rsp"), ("addq", "%rdx, %rsp")] {
        assert!(allocates_dynamically("x86_64", &[ri(0, "pushq", "%rbp"), ri(1, m, o)]), "{m} {o}");
    }
    for (m, o) in [("movq", "%rbp, %rsp"), ("leaq", "-0x8(%rbp), %rsp"), ("subq", "$0x10, %rsp"), ("andq", "$-0x10, %rsp"), ("movq", "%rsp, %rbp")] {
        assert!(!allocates_dynamically("x86_64", &[ri(1, m, o)]), "{m} {o}");
    }
}

#[test]
fn aarch64_arm_riscv_dynamic_allocation_in_the_body() {
    assert!(allocates_dynamically("aarch64", &[ri(0, "mov", "sp, x8")]));
    assert!(allocates_dynamically("aarch64", &[ri(0, "sub", "sp, x8, x9")]));
    assert!(!allocates_dynamically("aarch64", &[ri(0, "mov", "sp, x29")]), "an epilogue");
    assert!(!allocates_dynamically("aarch64", &[ri(0, "sub", "sp, sp, #0x40")]));
    assert!(allocates_dynamically("arm", &[ri(0, "sub.w", "sp, sp, r0")]));
    assert!(!allocates_dynamically("arm", &[ri(0, "sub", "sp, #0x8")]));
    assert!(allocates_dynamically("riscv64", &[ri(0, "sub", "sp, sp, a0")]));
    assert!(allocates_dynamically("riscv64", &[ri(0, "mv", "sp, a1")]));
    assert!(!allocates_dynamically("riscv64", &[ri(0, "mv", "sp, s0")]));
    assert!(!allocates_dynamically("mips", &[ri(0, "move", "sp, a0")]), "unknown architecture: nothing recognised");
}

// ------------------------------------------------------------ .su files

/// README.md:239 "`.su` files (`-fstack-usage`)": `file:line:col:function`,
/// bytes, and a qualifier that may say dynamic.
#[test]
fn su_lines_give_name_bytes_and_dynamic() {
    let su =
        "demo.c:10:5:clamp_altitude\t16\tstatic\ndemo.c:21:mode\t48\tdynamic\ndemo.c:30:vla\t64\tdynamic,bounded\nbroken line\ndemo.c:40:f\tnotanumber\tstatic\ndemo.c:50:g\t32\n";
    let v = parse_su(su);
    assert_eq!(v, vec![("clamp_altitude".to_string(), 16, false), ("mode".to_string(), 48, true), ("vla".to_string(), 64, true)]);
    let m = su_lookup(&v);
    assert_eq!(m.get("mode"), Some(&(48, true)));
    assert!(parse_su("").is_empty());
}

/// The `.su` fields as GCC documents them, `file:line:col:function<TAB>size
/// <TAB>qualifiers`, decided in #325 (ambiguity 12): the function is the
/// whole of what follows the location, not whatever follows its last `:`.
/// GCC writes a C++ function by its printable name, which has `::` in it --
/// these lines are g++ 16's, from `namespace ns { int f(int); struct C {
/// int m(int); template<class T> T t(T); }; }` -- and splitting on the last
/// `:` kept `f(int)` of `int ns::f(int)`, the same key as any other `f(int)`.
#[test]
fn a_gcc_cpp_su_name_with_scope_colons_is_kept_whole() {
    let su = "a.cpp:1:20:int ns::f(int)\t16\tstatic\n\
              a.cpp:1:113:int ns::C::m(int)\t16\tstatic\n\
              a.cpp:1:88:T ns::C::t(T) [with T = long int]\t16\tstatic\n\
              a.cpp:2:5:int g()\t48\tdynamic,bounded\n\
              a.cpp:4:1:{anonymous}::h()\t8\tstatic\n";
    let names: Vec<String> = parse_su(su).into_iter().map(|(n, _, _)| n).collect();
    assert_eq!(names, ["int ns::f(int)", "int ns::C::m(int)", "T ns::C::t(T) [with T = long int]", "int g()", "{anonymous}::h()"]);
    assert_eq!(parse_su(su)[3], ("int g()".to_string(), 48, true));
}

/// What already worked keeps working: clang writes `file:line:function`
/// with no column, and a C++ function by its mangled name; GCC's C names
/// have no `:` of their own; and a Windows path's drive letter is part of the
/// file, not a field.
#[test]
fn clang_su_lines_and_windows_paths_still_give_the_function() {
    let su = "a.cpp:1:_ZN2ns1fEi\t16\tstatic\n\
              a.cpp:3:h\t0\tstatic\n\
              c.c:1:5:k\t0\tstatic\n\
              C:\\src\\limits.c:10:5:clamp\t32\tstatic\n\
              C:\\src\\ns.cpp:7:3:void ns::run()\t24\tdynamic\n\
              /home/u/p:1/x.c:12:5:odd_path\t8\tstatic\n";
    let got: Vec<(String, u64, bool)> = parse_su(su);
    let names: Vec<&str> = got.iter().map(|(n, _, _)| n.as_str()).collect();
    assert_eq!(names, ["_ZN2ns1fEi", "h", "k", "clamp", "void ns::run()", "odd_path"]);
    assert_eq!(got[4], ("void ns::run()".to_string(), 24, true));
}

// ------------------------------------------------------------ worst case

/// README.md:242 "Worst case = own frame + deepest chain of direct calls":
/// the chain is the one with the largest total (a stack bound), and the path
/// names it.
#[test]
fn worst_case_is_own_frame_plus_the_heaviest_chain() {
    // 0 -> 1 -> 2, 0 -> 3 (a shorter chain with a bigger frame)
    let r = worst_case(&[node(16, &[1, 3]), node(32, &[2]), node(8, &[]), node(100, &[])]);
    assert_eq!(r[0].worst_case, Some(116));
    assert_eq!(r[0].path, vec![0, 3]);
    assert!(r[0].bounded && r[0].reason.is_none());
    assert_eq!(r[1].worst_case, Some(40));
    assert_eq!((r[2].worst_case, r[2].depth), (Some(8), 1));
}

/// "indirect calls ... are reported as 'not bounded' with the reason" -- and
/// so is every caller above one, whichever sibling chain is heavier.
#[test]
fn an_indirect_call_unbounds_every_caller_with_its_reason() {
    let mut ind = node(8, &[]);
    ind.indirect_calls = 2;
    // 0 -> {1 (indirect, light), 2 (heavy, bounded)}
    let r = worst_case(&[node(16, &[1, 2]), ind, node(500, &[])]);
    assert!(!r[1].bounded);
    assert_eq!(r[1].reason.as_deref(), Some("2 indirect call(s): callee set unknown"));
    assert!(!r[0].bounded, "the heavier bounded sibling must not hide the indirect call");
    assert!(r[0].reason.as_deref().is_some_and(|s| s.contains("indirect")), "{:?}", r[0].reason);
    assert!(r[2].bounded);
}

/// matrix.toml:189 "reported not bounded, reason \"dynamic frame (alloca /
/// VLA)\", and so is every caller above them".
#[test]
fn a_dynamic_frame_unbounds_every_caller() {
    let mut vla = node(32, &[]);
    vla.dynamic = true;
    let r = worst_case(&[node(16, &[1]), node(8, &[2]), vla]);
    assert_eq!(r.len(), 3);
    for (k, x) in r.iter().enumerate() {
        assert!(!x.bounded, "{k}");
        assert_eq!(x.reason.as_deref(), Some("dynamic frame (alloca / VLA)"), "{k}");
    }
    assert_eq!(r[0].worst_case, Some(56), "the static part of the frame is shown");
}

/// "Recursion ... reported as 'not bounded'"; a symbol that only reaches a
/// cycle is not bounded either, and the traversal terminates.
#[test]
fn recursion_is_never_bounded() {
    // 0 -> 1 <-> 2, 3 -> 3
    let r = worst_case(&[node(4, &[1]), node(8, &[2]), node(16, &[1]), node(32, &[3])]);
    assert_eq!(r.len(), 4);
    for (k, x) in r.iter().enumerate() {
        assert!(!x.bounded, "{k}: {x:?}");
        assert!(x.reason.as_deref().is_some_and(|s| s.contains("recursion")), "{k}: {:?}", x.reason);
    }
    // whichever node the analysis starts from, the verdict is the same
    let r2 = worst_case(&[node(16, &[1]), node(8, &[0])]);
    assert!(!r2[0].bounded && !r2[1].bounded);
}

/// A frame that could not be determined fails closed (StackNode.unknown).
#[test]
fn an_unknown_frame_is_not_bounded() {
    let mut u = node(0, &[]);
    u.unknown = true;
    let r = worst_case(&[node(16, &[1]), u]);
    assert!(!r[1].bounded && !r[0].bounded);
    assert_eq!(r[1].reason.as_deref(), Some("frame size could not be determined"));
}

/// "nothing is silently capped": a total past u64 is not a small bounded
/// number.
#[test]
fn an_overflowing_total_is_not_bounded() {
    let r = worst_case(&[node(u64::MAX, &[1]), node(16, &[])]);
    assert!(!r[0].bounded);
    assert_eq!(r[0].worst_case, Some(u64::MAX));
}

/// An empty graph is an empty answer.
#[test]
fn no_symbols_no_results() {
    assert!(worst_case(&[]).is_empty());
}
