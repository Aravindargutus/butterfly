// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Data coupling and the stack chain of `examples/c-lib`, asserted on the
//! image the host actually built (#172).
//!
//! Hosted CI runs on x86-64, but until this file the x86-64 data-reference
//! and stack paths were tested only on hand-written instruction lists
//! (`synthetic.rs`), and every end-to-end assertion that named an
//! architecture named AArch64. So the x86-64 row of the support matrix had
//! no test reading a real x86-64 image.
//!
//! One test per architecture, compiled only on that host, each asserting
//! what is true there -- the common facts through one shared check, the
//! architecture-dependent ones (variable attribution, #52) spelled out in
//! each. The example is built by the host's compiler, so each test first
//! checks the image *is* the host's architecture: a fixture built for
//! something else fails, rather than being read against the wrong branch.
//!
//! What is asserted is what the example's README states and its source
//! guarantees, never a byte count: frame sizes are the compiler's (the same
//! source gives 144 B for `main` on arm64 Mach-O and 72 B on x86-64 Mach-O),
//! so the chain is checked for its shape and for adding up, not its total.
//!
//! On a host that is neither, nothing here is compiled: no expectation has
//! been written for it, and an empty test binary says so more honestly than
//! a shared helper left unused.

#![cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]

use butterfly_model::{Evidence, FrameSource, Symbol};
use std::path::PathBuf;

/// The functions `src/sched.c` defines. `s_dispatches` is `static` there,
/// so nothing outside this list can reach it.
const SCHED_C: &[&str] = &["sched_run", "sched_dispatches", "task_climb", "task_hold", "task_trim"];

fn c_lib() -> Option<Evidence> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-lib");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("examples/c-lib/build not present (run examples/c-lib/build.sh)");
        return None;
    }
    let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts.source_root = Some(root);
    Some(butterfly_ingest::ingest(&opts).expect("ingest"))
}

fn sym<'a>(ev: &'a Evidence, name: &str) -> &'a Symbol {
    ev.symbols.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no symbol `{name}`"))
}

fn names(ev: &Evidence, ids: &[usize]) -> Vec<String> {
    let mut v: Vec<String> = ids.iter().map(|&i| ev.symbols[i].name.clone()).collect();
    v.sort();
    v
}

/// What holds on every architecture this example is built for: the three
/// globals and who touches them, and the stack chains through the task
/// table. Returns nothing; panics with the evidence it disagreed with.
fn coupling_and_stack(ev: &Evidence) {
    // ── data coupling: the README's table ────────────────────────────────
    let global = |name: &str| {
        ev.globals.iter().find(|g| g.name == name).unwrap_or_else(|| panic!("global `{name}` not found: {:?}", ev.globals.iter().map(|g| &g.name).collect::<Vec<_>>()))
    };

    // g_state: written by state.c *and* mode.c, read by state.c's own
    // accessors on every format, and by main.c. main reads g_state as an
    // *external* global -- a direct adrp+add reference on Mach-O, which the
    // data-reference scanner attributes, but a GOT-indirect load on ELF, which
    // it does not follow. So a reference to g_state from *another* translation
    // unit -- main.c reading it, mode.c's `mode_advance` writing it -- is an
    // object-format-dependent attribution (variable attribution, #52), not
    // coupling the image lacks. Hold state.c's own same-TU accessors to every
    // format, and assert the cross-unit main/mode_advance only where the
    // reference is direct.
    let g = global("g_state");
    assert_eq!(g.size, 12, "g_state is a 12-byte struct");
    let (r, w) = (names(ev, &g.reader_symbols), names(ev, &g.writer_symbols));
    for reader in ["state_altitude", "state_raise_fault"] {
        assert!(r.iter().any(|n| n == reader), "{reader} reads g_state: readers {r:?}");
    }
    assert!(w.iter().any(|n| n == "state_set_altitude"), "state_set_altitude writes g_state: writers {w:?}");
    if ev.provenance.object_format == "MachO" {
        assert!(r.iter().any(|n| n == "main"), "main reads g_state (a direct cross-unit reference on Mach-O): readers {r:?}");
        assert!(w.iter().any(|n| n == "mode_advance"), "mode_advance writes g_state (a direct cross-unit reference on Mach-O): writers {w:?}");
    }

    // s_dispatches: module-private, so only sched.c's functions touch it,
    // and sched_run is the one that counts
    let s = global("s_dispatches");
    assert_eq!(s.size, 4);
    let (r, w) = (names(ev, &s.reader_symbols), names(ev, &s.writer_symbols));
    assert!(w.iter().any(|n| n == "sched_run"), "sched_run writes s_dispatches: writers {w:?}");
    for n in r.iter().chain(&w) {
        assert!(SCHED_C.contains(&n.as_str()), "`{n}` touches s_dispatches from outside sched.c");
    }

    // TASKS: the table sched_run dispatches through
    let t = global("TASKS");
    assert_eq!(t.size, 24, "three function pointers");
    let r = names(ev, &t.reader_symbols);
    assert!(r.iter().any(|n| n == "sched_run"), "sched_run reads TASKS: readers {r:?}");

    // ── the stack chain ──────────────────────────────────────────────────
    // Frames come from the six `.su` files, one per translation unit, and
    // a chain's worst case is exactly its frames added up.
    let adds_up = |s: &Symbol| {
        let st = &s.stack;
        let sum: u64 = st.path.iter().map(|&i| ev.symbols[i].frame.bytes).sum();
        assert_eq!(st.worst_case, Some(sum), "{}: worst case is the frames along {:?}", s.name, names(ev, &st.path));
        for &i in &st.path {
            assert_eq!(ev.symbols[i].frame.source, FrameSource::StackUsageFile, "{} has the compiler's frame", ev.symbols[i].name);
        }
    };

    // main dispatches through TASKS[idx] inside sched_run: the one call no
    // static analysis can resolve, so the chain is reported unbounded
    for name in ["main", "sched_run"] {
        let s = sym(ev, name);
        assert!(!s.stack.bounded, "{name}: an indirect call must not read as bounded");
        let why = s.stack.reason.as_deref().unwrap_or("");
        assert!(why.contains("indirect call"), "{name}: {why:?}");
        adds_up(s);
    }
    assert_eq!(names(ev, &sym(ev, "main").stack.path[..2]), ["main", "sched_run"], "main's chain goes through the dispatcher");
    assert!(sym(ev, "sched_run").indirect_calls > 0, "the call through TASKS is recorded as indirect");

    // task_climb -> state_set_altitude -> ... crosses files and is bounded:
    // every call on it is direct
    let climb = sym(ev, "task_climb");
    assert!(climb.stack.bounded, "task_climb: {:?}", climb.stack.reason);
    let path = names(ev, &climb.stack.path);
    assert!(climb.stack.path.len() >= 3, "task_climb reaches two files down: {path:?}");
    assert_eq!(ev.symbols[climb.stack.path[1]].name, "state_set_altitude", "{path:?}");
    adds_up(climb);
}

/// Every variable is declared on every architecture; attribution is the
/// part that is not.
fn variables_listed(ev: &Evidence) {
    let f = ev.functions.iter().find(|f| f.name == "clamp_i32").expect("clamp_i32");
    let got: Vec<&str> = f.variables.iter().map(|v| v.name.as_str()).collect();
    for p in ["v", "lo", "hi"] {
        assert!(got.contains(&p), "clamp_i32 param {p}: {got:?}");
    }
}

/// x86-64: the architecture hosted CI runs. The globals, their readers and
/// writers here come from the x86-64 data-reference scanner reading
/// RIP-relative operands, and the unbounded chain from `call *%reg` being
/// classified as indirect -- neither exercised on a real image before this.
/// Variables are listed, with no accesses, each naming the architecture.
#[cfg(target_arch = "x86_64")]
#[test]
fn x86_64_c_lib_couples_globals_and_reports_the_unbounded_chain() {
    let Some(ev) = c_lib() else { return };
    let img = ev.image.as_ref().expect("an analysed image");
    assert_eq!(img.arch, "x86_64", "examples/c-lib/build is not an x86-64 image (rebuild it on this host)");
    coupling_and_stack(&ev);
    variables_listed(&ev);
    for f in &ev.functions {
        for v in &f.variables {
            assert!(v.accesses.is_empty(), "{}::{}: {:?}", f.name, v.name, v.accesses);
            assert_eq!(v.unattributed.as_deref(), Some("frame-slot accesses are read for aarch64; this image is x86_64"), "{}::{}", f.name, v.name);
        }
    }
}

/// AArch64: the same coupling and chain from the AArch64 scanner, and the
/// variables attributed -- the architecture attribution is written for.
#[cfg(target_arch = "aarch64")]
#[test]
fn aarch64_c_lib_couples_globals_and_reports_the_unbounded_chain() {
    let Some(ev) = c_lib() else { return };
    let img = ev.image.as_ref().expect("an analysed image");
    assert_eq!(img.arch, "aarch64", "examples/c-lib/build is not an AArch64 image (rebuild it on this host)");
    coupling_and_stack(&ev);
    variables_listed(&ev);
    let f = ev.functions.iter().find(|f| f.name == "clamp_i32").expect("clamp_i32");
    for v in &f.variables {
        assert!(!v.accesses.is_empty(), "clamp_i32::{} is attributed on aarch64: {:?}", v.name, v.unattributed);
    }
}
