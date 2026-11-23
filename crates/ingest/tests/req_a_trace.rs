// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the `--trace` input formats (area A).
//!
//! README.md:288-289 "Optional `--trace`: QEMU `-d in_asm,exec,nochain -D
//! trace.log`, the QEMU `execlog` plugin output, or a plain list of executed
//! addresses", and README.md:1620 "**Counters saturate.** A corrupt trace
//! cannot overflow an execution count."

use butterfly_ingest::trace::parse;

/// README.md:289 the QEMU `execlog` plugin's output: one line per executed
/// instruction, `cpu, 0xPC, 0xINSN, "disassembly"`. The disassembly of most
/// instructions contains commas of its own.
#[test]
fn execlog_output_counts_each_executed_instruction() {
    let log = "0, 0x40080000, 0xd53800a0, \"mrs x0, mpidr_el1\"\n\
               0, 0x40080004, 0x92401c00, \"and x0, x0, #0xff\"\n\
               0, 0x40080000, 0xd53800a0, \"mrs x0, mpidr_el1\"\n";
    let t = parse(log);
    assert_ne!(t.format, "unrecognised");
    assert_eq!(t.counts.get(&0x40080000), Some(&2));
    assert_eq!(t.counts.get(&0x40080004), Some(&1));
}

/// The execlog plugin appends the memory access to a load or store line
/// (`, load, 0xADDR` / `, store, 0xADDR`); the executed PC is still the
/// second field.
#[test]
fn execlog_lines_with_a_memory_access_still_count_their_pc() {
    let log = "0, 0x40080010, 0xf9000020, \"str x0, [x1]\", store, 0x40100000\n\
               0, 0x40080014, 0xf9400020, \"ldr x0, [x1]\", load, 0x40100000\n";
    let t = parse(log);
    assert_eq!(t.counts.get(&0x40080010), Some(&1));
    assert_eq!(t.counts.get(&0x40080014), Some(&1));
    assert!(!t.counts.contains_key(&0x40100000), "the data address is not an executed instruction");
}

/// README.md:289 "a plain list of executed addresses" -- an address with no
/// count is one execution, and blank lines are not addresses.
#[test]
fn a_plain_address_list_counts_each_line() {
    let t = parse("0x1000\n\n0x1000\n0x2000\n");
    assert_eq!(t.counts.get(&0x1000), Some(&2));
    assert_eq!(t.counts.get(&0x2000), Some(&1));
    assert_eq!(t.counts.len(), 2);
}

/// README.md:1620 "Counters saturate" -- in the QEMU exec format too: a
/// block's count and every instruction's count stop at the maximum.
#[test]
fn qemu_exec_counts_are_read_per_block_and_expanded_to_instructions() {
    let log = "IN: f\n0x40080000:  d503201f  nop\n0x40080004:  d65f03c0  ret\n\n\
               Trace 0: 0x1 [00004000/0000000040080000/00000521/ff020200] f\n\
               Trace 0: 0x1 [00004000/0000000040080100/00000521/ff020200] g\n\
               Trace 0: 0x1 [00004000/0000000040080000/00000521/ff020200] f\n";
    let mut t = parse(log);
    assert_eq!(t.tb_counts.get(&0x40080000), Some(&2));
    // The second block has no `IN:` listing: its extent has to come from
    // elsewhere, and docs/butterfly.html:808 "Every one of these is a caveat
    // on the evidence rather than a silent guess."
    t.expand(&|pc| if pc == 0x40080100 { vec![0x40080100, 0x40080104] } else { vec![] });
    assert_eq!(t.counts.get(&0x40080000), Some(&2));
    assert_eq!(t.counts.get(&0x40080004), Some(&2));
    assert_eq!(t.counts.get(&0x40080104), Some(&1), "the reconstructed block's instructions are counted");
    assert!(!t.caveats.is_empty(), "and the reconstruction is a caveat, not silent");
}
