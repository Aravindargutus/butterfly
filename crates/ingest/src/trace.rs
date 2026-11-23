// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Execution-trace import: the measured counterpart to projected coverage.
//!
//! Accepted formats (auto-detected):
//! * QEMU `-d in_asm,exec,nochain` log: `IN:` blocks list a translation
//!   block's instruction addresses; `Trace N: <host> [cs_base/pc/flags/cflags]`
//!   lines record each execution of a block. `N` is the index of the CPU
//!   that entered it (`cpu->cpu_index` in QEMU's `log_cpu_exec`), and the
//!   log interleaves every CPU's records: what ran after what is only
//!   meaningful within one CPU's records, so that is how it is read.
//! * QEMU `execlog` plugin: `N, 0xPC, 0xINSN, "mnemonic"` per instruction.
//! * Plain text: one address per line, optionally followed by a count.

use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Default)]
pub struct Trace {
    pub format: String,
    /// Executions per instruction address.
    pub counts: HashMap<u64, u64>,
    /// Executions per translation-block start (QEMU exec format).
    pub tb_counts: HashMap<u64, u64>,
    /// Instruction addresses per translation block (QEMU in_asm format).
    pub tb_instrs: HashMap<u64, Vec<u64>>,
    pub block_executions: u64,
    pub caveats: Vec<String>,
    /// (from TB start, to TB start) -> count, from consecutive exec records
    /// OF THE SAME CPU, summed over CPUs. Two records that are neighbours in
    /// the log but come from different CPUs are not a transition: nothing
    /// went from one block to the other.
    pub transitions: HashMap<(u64, u64), u64>,
    /// CPU indices seen in exec records.
    pub cpus: BTreeSet<u64>,
    /// Exec records whose block was read but whose CPU index was not. They
    /// are counted as executions of that block, but which stream they belong
    /// to is unknown, so no transition is drawn from or to them, none is
    /// drawn ACROSS one either, and nothing is ever discarded from a log
    /// that has any.
    pub records_without_cpu: u64,
    /// Exec records nothing could be read from but the fact that one
    /// happened: brackets missing or the wrong way round, or a PC that is
    /// not a number. Which block ran, and on which CPU, are both unknown, so
    /// they are in no block's count -- and they break adjacency exactly as
    /// the ones above do, because one of them ran between the records either
    /// side of it, and stop the log's repeat cleanup exactly as the ones
    /// above do, for the same reason (review of #208).
    pub records_without_pc: u64,
    /// Lines inside an `IN:` block that could not be read as an instruction
    /// (a truncated final line, say). The block's extent is short by that
    /// many instructions.
    pub unreadable_in_asm: u64,
}

/// The share of a QEMU log's exec records that may be unreadable before the
/// log is refused outright instead of reported with a caveat, as a
/// percentage.
///
/// One unreadable record is survivable, and is handled honestly: it costs
/// the adjacency around it and the whole log's repeat cleanup, and the
/// caveat says so. Many is a different thing. A log that damaged is not a
/// trace with a gap in it; what it has to do with the run is unknown. Yet
/// reported rather than refused it would still be *measured* coverage -- the
/// strongest claim this tool makes, and the one a Level A programme leans on
/// hardest -- with the damage confined to a sentence most readers of a
/// coverage number never reach. One percent is a judgement, not a
/// derivation: the QEMU logs this was built on have none at all, so anything
/// above noise means the file was truncated, interleaved with other output,
/// or is not what the caller thinks it is.
const MAX_UNREADABLE_PERCENT: u64 = 1;

/// What one `Trace ` line was found to hold.
///
/// Reading the line and deciding what it does to the execution streams are
/// deliberately two steps. Every way a record can be unreadable arrives here
/// as a variant, and one `match` in `parse` decides what each does, so a new
/// parse failure cannot be added without someone choosing its effect on
/// stream adjacency. The shape this replaces decided adjacency at the end of
/// a chain of `if let`s, and so reset the streams only for the one failure
/// that had been thought about: a record with a missing or reversed bracket,
/// or a PC that is not a number, fell through untouched and the records
/// either side of it were joined into a transition that never happened
/// (review of #208).
enum Record {
    /// A block entry whose CPU index and PC were both read.
    Entered { cpu: u64, pc: u64 },
    /// A block entry whose PC was read and whose CPU index was not -- older
    /// QEMU wrote `Trace 0x<host> [pc]`, and a damaged field reads the same
    /// way. It may be the missing successor of any stream.
    EnteredUnknownCpu { pc: u64 },
    /// A line that says an execution happened and nothing else that can be
    /// trusted. Which block, and whose, are both unknown.
    Unreadable,
}

/// Read one `Trace ` record from the text after `Trace `. Every failure
/// leaves by a variant; nothing falls through.
fn trace_record(rest: &str) -> Record {
    // Trace 0: 0x7f.. [cs_base/pc/flags/cflags] name
    // `b > a` because the trace is untrusted input: `Trace 0] x [0/..]`
    // would otherwise slice a range that runs backwards and panic.
    let (Some(a), Some(b)) = (rest.find('['), rest.find(']')) else { return Record::Unreadable };
    if b <= a {
        return Record::Unreadable;
    }
    let fields: Vec<&str> = rest[a + 1..b].split('/').collect();
    let pc_s = if fields.len() >= 2 { fields[1] } else { fields[0] };
    let Ok(pc) = u64::from_str_radix(pc_s.trim_start_matches("0x"), 16) else { return Record::Unreadable };
    match rest[..a].split_once(':').and_then(|(n, _)| n.trim().parse::<u64>().ok()) {
        Some(cpu) => Record::Entered { cpu, pc },
        None => Record::EnteredUnknownCpu { pc },
    }
}

pub fn parse(text: &str) -> Trace {
    let mut t = Trace::default();
    let mut cur_tb: Option<(u64, Vec<u64>)> = None;
    // the last block each CPU entered
    let mut prev_pc: HashMap<u64, u64> = HashMap::new();
    let mut qemu_lines = 0usize;
    let mut execlog_lines = 0usize;
    let mut plain_lines = 0usize;

    /// One more execution of the block starting at `pc`.
    fn count_entry(t: &mut Trace, pc: u64) {
        let e = t.tb_counts.entry(pc).or_default();
        *e = e.saturating_add(1);
        t.block_executions = t.block_executions.saturating_add(1);
    }

    let flush = |cur: &mut Option<(u64, Vec<u64>)>, t: &mut Trace| {
        if let Some((start, v)) = cur.take() {
            if !v.is_empty() {
                t.tb_instrs.insert(start, v);
            }
        }
    };

    for line in text.lines() {
        let l = line.trim_end();
        if l.starts_with("IN:") {
            flush(&mut cur_tb, &mut t);
            cur_tb = Some((0, Vec::new()));
            qemu_lines += 1;
            continue;
        }
        // `l` has been right-trimmed, so a record stripped of everything
        // after its name is the bare word; it is still an exec record.
        if let Some(rest) = l.strip_prefix("Trace").filter(|r| r.is_empty() || r.starts_with(' ')) {
            // Recognising the line as an exec record is where each stream's
            // continuity is decided, whatever else about the line could or
            // could not be read. An execution happened here; a record that
            // cannot be placed is still a record that sits BETWEEN the ones
            // either side of it, so those two are not adjacent any more.
            qemu_lines += 1;
            match trace_record(rest) {
                Record::Entered { cpu, pc } => {
                    count_entry(&mut t, pc);
                    t.cpus.insert(cpu);
                    if let Some(p) = prev_pc.insert(cpu, pc) {
                        let e = t.transitions.entry((p, pc)).or_default();
                        *e = e.saturating_add(1);
                    }
                }
                // Whose record this is cannot be known. It cannot be placed
                // after any other record -- and no other record can be placed
                // after the ones before it either: this one may be the
                // missing successor of any of them. So every stream boundary
                // is dropped, and the next record of each CPU starts a stream
                // afresh. Keeping them bridged the gap: a block ending in
                // `b.hs` followed by an unreadable record and then the branch
                // target was read as a taken edge, measured, that nobody took.
                Record::EnteredUnknownCpu { pc } => {
                    count_entry(&mut t, pc);
                    t.records_without_cpu = t.records_without_cpu.saturating_add(1);
                    prev_pc.clear();
                }
                // The same reasoning, and one thing more: not even the block
                // is known, so this execution is in no block's count. The
                // counts are short by one, which is the direction that
                // under-reports coverage rather than inventing it.
                Record::Unreadable => {
                    t.records_without_pc = t.records_without_pc.saturating_add(1);
                    prev_pc.clear();
                }
            }
            continue;
        }
        if l.is_empty() || l.starts_with("----") {
            flush(&mut cur_tb, &mut t);
            continue;
        }
        if let Some((start, v)) = cur_tb.as_mut() {
            // 0x40080000:  d53800a0  mrs x0, mpidr_el1
            if let Some((addr_s, _)) = l.trim_start().split_once(':') {
                if let Ok(addr) = u64::from_str_radix(addr_s.trim_start_matches("0x"), 16) {
                    if v.is_empty() {
                        *start = addr;
                    }
                    v.push(addr);
                    continue;
                }
            }
            t.unreadable_in_asm = t.unreadable_in_asm.saturating_add(1);
        }
        // Once this is a QEMU log, it is only a QEMU log. A line inside an
        // `IN:` block that the disassembly parser could not read is a damaged
        // instruction line -- a log cut off mid-line by build.sh's watchdog,
        // say -- and handing it to the plain-address parser below invented an
        // execution at a truncated address (`0x1000` -> a count at 0x100)
        // while the format still read "qemu -d in_asm,exec".
        if qemu_lines > 0 {
            continue;
        }
        // execlog plugin: "0, 0x40080000, 0xd53800a0, \"mrs ...\""
        let parts: Vec<&str> = l.split(',').map(str::trim).collect();
        if parts.len() >= 3 && parts[1].starts_with("0x") && parts[2].starts_with("0x") {
            if let Ok(addr) = u64::from_str_radix(&parts[1][2..], 16) {
                let e = t.counts.entry(addr).or_default();
                *e = e.saturating_add(1);
                execlog_lines += 1;
                continue;
            }
        }
        // plain: "0x1234" | "1234" | "0x1234 5"
        let mut it = l.split_whitespace();
        if let Some(a) = it.next() {
            if let Ok(addr) = u64::from_str_radix(a.trim_start_matches("0x"), 16) {
                let n = it.next().and_then(|c| c.parse::<u64>().ok()).unwrap_or(1);
                let e = t.counts.entry(addr).or_default();
                *e = e.saturating_add(n);
                plain_lines += 1;
            }
        }
    }
    flush(&mut cur_tb, &mut t);

    if qemu_lines > 0 {
        // Any leading non-QEMU lines before the first `IN:`/`Trace` line may
        // have matched the plain-text hex parser; discard those counts so
        // QEMU measured counts come strictly from `tb_counts` via `expand`.
        t.counts.clear();
        t.format = "qemu -d in_asm,exec".into();
        if t.tb_instrs.is_empty() {
            t.caveats.push("trace has exec records but no in_asm blocks: block extents were reconstructed from the disassembly".into());
        }
        if t.unreadable_records() > 0 {
            t.caveats.push(format!(
                "{} of {} exec record(s) could not be read in full: {}. An execution happened at each of them, so the records either side of one are not known to be consecutive -- no block-to-block transition was drawn from or to such a record, nor across one, and every CPU's stream restarts at each of them, because the record may be the missing successor of any of them. Measured branch outcomes and indirect-call targets that depended on those transitions are missing, not zero. No repeated record was discarded from this log either.",
                t.unreadable_records(),
                t.exec_records(),
                unreadable_summary(t.records_without_cpu, t.records_without_pc),
            ));
        }
        if t.unreadable_in_asm > 0 {
            t.caveats.push(format!(
                "{} line(s) inside an `IN:` block could not be read as an instruction (a log cut off mid-line, say). Those blocks are short by that many instructions, so instructions the run did execute may be reported as never executed.",
                t.unreadable_in_asm
            ));
        }
        if t.cpus.len() > 1 {
            t.caveats.push(format!(
                "the trace interleaves records from {} CPUs; block-to-block transitions were drawn within each CPU's records only, and execution counts are summed over CPUs.",
                t.cpus.len()
            ));
        }
    } else if execlog_lines > 0 {
        t.format = "qemu execlog plugin".into();
        t.block_executions = execlog_lines as u64;
    } else if plain_lines > 0 {
        t.format = "address list".into();
        t.block_executions = t.counts.values().fold(0u64, |a, b| a.saturating_add(*b));
    } else {
        t.format = "unrecognised".into();
    }
    t
}

/// The clause naming what was wrong with the unreadable records, so that the
/// caveat counts them honestly whichever kind they are. The two are not the
/// same loss: one is missing an ordering, the other is missing from the
/// counts as well, and a reader deciding what the number is worth needs to
/// know which (review of #208).
fn unreadable_summary(without_cpu: u64, without_pc: u64) -> String {
    let cpu = format!("{without_cpu} had a readable block but no readable CPU index (`Trace N:`), so they are counted as executions of that block but belong to no known stream");
    let pc = format!(
        "{without_pc} could not be read at all (brackets missing or the wrong way round, or a PC that is not a number), so the block each one entered is unknown and they are in no block's count -- the counts below are short by that many executions, and an instruction the run did execute may be reported as never executed"
    );
    match (without_cpu, without_pc) {
        (0, _) => pc,
        (_, 0) => cpu,
        _ => format!("{cpu}; {pc}"),
    }
}

impl Trace {
    /// Exec records seen, readable or not.
    pub fn exec_records(&self) -> u64 {
        self.block_executions.saturating_add(self.records_without_pc)
    }

    /// Exec records that could not be read in full, of either kind. Any of
    /// them means the log's order is not known everywhere, so no transition
    /// crosses one and nothing is discarded from the log at all.
    pub fn unreadable_records(&self) -> u64 {
        self.records_without_cpu.saturating_add(self.records_without_pc)
    }

    /// Why this log is too damaged to stand as measured evidence, when it
    /// is. `None` means report it, caveats and all; `Some` means the caller
    /// must refuse it rather than publish a coverage number derived from it.
    ///
    /// The line is `MAX_UNREADABLE_PERCENT`; see there for why there is one
    /// and why it sits where it does. Everything below the line is still
    /// said, in full, in the caveat -- refusing is not a substitute for that.
    pub fn refusal(&self) -> Option<String> {
        let (bad, all) = (self.records_without_pc, self.exec_records());
        if bad == 0 || bad.saturating_mul(100) <= all.saturating_mul(MAX_UNREADABLE_PERCENT) {
            return None;
        }
        Some(format!(
            "{bad} of {all} execution records in this trace could not be read ({}), which is more than {MAX_UNREADABLE_PERCENT}% of them. A log that damaged is not a trace with a gap in it -- what it has to do with the run is unknown -- so no measured coverage was produced from it. Reporting it would put a coverage number on a file butterfly could not read, with the damage in a caveat. Check that the log is a complete QEMU `-d in_asm,exec` log and was not truncated or interleaved with other output.",
            unreadable_summary(self.records_without_cpu, self.records_without_pc),
        ))
    }

    /// Discard block executions the log records but the program cannot have
    /// performed, and return how many there were. Call before `expand`.
    ///
    /// QEMU writes a `Trace` record when it ENTERS a translation block. A
    /// block entered with an exit request already pending is abandoned at its
    /// head, before its first instruction, and entered again afterwards --
    /// so it is logged twice and ran once. On a real bare-metal run this
    /// happens to the very first block about one time in three, which made
    /// the same image yield 148 block executions or 149 depending on the run.
    ///
    /// The log itself does not mark the abandoned entry, so only the case
    /// that can be proven is removed: block A directly followed by block A
    /// ON THE SAME CPU when A's last instruction cannot lead to A's start.
    /// Two CPUs entering the same block one after the other in the log --
    /// every secondary core coming through `_start` -- are two executions;
    /// `transitions` is per CPU, so they never appear here. If any record
    /// could not be read in full -- its CPU, or its block, or neither -- the
    /// streams cannot be told apart with certainty and nothing is discarded
    /// at all.
    ///
    /// `may_follow(from_last, to)` answers, from the disassembly, whether
    /// control can go from the instruction at `from_last` to a block starting
    /// at `to`; it must say `true` whenever it is not sure.
    ///
    /// The abandoned entry that is NOT followed by the same block -- an
    /// interrupt vector taken in between, `A, V, A` -- cannot be proven away,
    /// and it leaves two marks rather than one: A is counted twice, and
    /// `transitions` gains `(A, V)`, an edge out of a block abandoned before
    /// its first instruction. So both are reported: every transition the
    /// disassembly says cannot happen is counted and named in a caveat.
    /// Execution counts AND outcome edges from this format are upper bounds;
    /// coverage -- count above zero -- is unaffected either way.
    pub fn discard_impossible_repeats(&mut self, may_follow: &dyn Fn(u64, u64) -> bool) -> u64 {
        let mut discarded = 0u64;
        // Either kind of unreadable record, not one of them: a record nothing
        // could be read from at all breaks the streams exactly as one with no
        // CPU index does, and the log's caveat already tells the reader that
        // nothing was discarded. Testing `records_without_cpu` alone left one
        // unreadable PC among 203 records removing 201 measured block
        // executions while that sentence stood (review of #208).
        if self.unreadable_records() > 0 {
            return 0;
        }
        // the last instruction address of each block whose extent the log listed
        let lasts: HashMap<u64, u64> = self.tb_instrs.iter().filter_map(|(s, v)| v.last().map(|l| (*s, *l))).collect();
        let repeats: Vec<(u64, u64)> = self.transitions.iter().filter(|((a, b), _)| a == b).map(|((a, _), n)| (*a, *n)).collect();
        for (pc, n) in repeats {
            let Some(&last) = lasts.get(&pc) else { continue };
            if may_follow(last, pc) {
                continue;
            }
            self.transitions.remove(&(pc, pc));
            if let Some(c) = self.tb_counts.get_mut(&pc) {
                *c = c.saturating_sub(n);
            }
            self.block_executions = self.block_executions.saturating_sub(n);
            discarded = discarded.saturating_add(n);
        }
        if discarded > 0 {
            self.caveats.push(format!(
                "{discarded} block execution(s) in the log were discarded: the same block is recorded twice in a row by the same CPU although its last instruction cannot branch back to its start. QEMU logs a block on entry and re-enters one it abandoned for a pending exit request, so the repeat never ran."
            ));
        }
        // What is left that the disassembly forbids: an entry abandoned with
        // something else logged before the re-entry, or control leaving a
        // block asynchronously. Neither can be told from the other, so
        // neither is removed -- but neither is a branch outcome, and the
        // reader is told so.
        let impossible: u64 =
            self.transitions.iter().filter(|((a, b), _)| lasts.get(a).is_some_and(|last| !may_follow(*last, *b))).map(|(_, n)| *n).fold(0u64, |acc, n| acc.saturating_add(n));
        if impossible > 0 {
            self.caveats.push(format!(
                "{impossible} recorded block-to-block transition(s) are not edges the disassembly allows: the source block's last instruction cannot lead to the target. Either the source block was abandoned before its first instruction and something else ran before it was re-entered (QEMU logs a block on entry), or control left it asynchronously. They are left in place because the two cannot be told apart -- so this trace's execution counts and measured branch outcomes are upper bounds, and an outcome edge shown as taken may be one of these."
            ));
        }
        discarded
    }

    /// Expand block executions into per-instruction counts. `extent` returns
    /// the instruction addresses of a block starting at `pc` when the trace
    /// itself did not list them (walk the disassembly to the next control
    /// transfer).
    pub fn expand(&mut self, extent: &dyn Fn(u64) -> Vec<u64>) {
        let mut missing = 0usize;
        let tbs: Vec<(u64, u64)> = self.tb_counts.iter().map(|(a, n)| (*a, *n)).collect();
        for (pc, n) in tbs {
            let instrs = match self.tb_instrs.get(&pc) {
                Some(v) => v.clone(),
                None => {
                    missing += 1;
                    extent(pc)
                }
            };
            for a in instrs {
                let e = self.counts.entry(a).or_default();
                *e = e.saturating_add(n);
            }
        }
        if missing > 0 && !self.tb_instrs.is_empty() {
            self.caveats.push(format!("{missing} executed block(s) had no in_asm listing; their extent was reconstructed from the disassembly"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qemu_format() {
        let log = "IN: start\n0x40080000:  d53800a0  mrs x0, mpidr_el1\n0x40080004:  92401c00  and x0, x0, #0xff\n\nTrace 0: 0x300000180 [00004000/0000000040080000/00000521/ff020200] \nTrace 0: 0x300000180 [00004000/0000000040080000/00000521/ff020200] \n";
        let mut t = parse(log);
        assert_eq!(t.block_executions, 2);
        t.expand(&|_| vec![]);
        assert_eq!(t.counts[&0x40080000], 2);
        assert_eq!(t.counts[&0x40080004], 2);
        assert_eq!(t.transitions[&(0x40080000, 0x40080000)], 1);
    }

    /// The first block of a real QEMU run, logged twice: it ends in a call
    /// to somewhere else, so the second record cannot be a second execution.
    #[test]
    fn a_block_logged_twice_that_cannot_loop_is_counted_once() {
        let log = "IN: \n0x40080000:  d503201f  nop\n0x40080004:  94000165  bl #0x400805a0\n\n\
                   Trace 0: 0x1 [00004000/0000000040080000/00000521/ff020200] \n\
                   Trace 0: 0x1 [00004000/0000000040080000/00000521/ff020200] \n\
                   IN: kmain\n0x400805a0:  d65f03c0  ret\n\n\
                   Trace 0: 0x2 [00004000/00000000400805a0/00000521/ff020200] kmain\n";
        let mut t = parse(log);
        assert_eq!(t.block_executions, 3);
        assert_eq!(t.discard_impossible_repeats(&|from_last, to| (from_last, to) != (0x40080004, 0x40080000)), 1);
        assert_eq!(t.block_executions, 2);
        assert!(!t.transitions.contains_key(&(0x40080000, 0x40080000)));
        assert_eq!(t.transitions[&(0x40080000, 0x400805a0)], 1);
        t.expand(&|_| vec![]);
        assert_eq!(t.counts[&0x40080004], 1);
        assert!(t.caveats.iter().any(|c| c.contains("1 block execution(s)")), "{:?}", t.caveats);
    }

    /// The reviewer's case on #208. Two CPUs come through the same startup
    /// block, back to back in the log; it ends in a direct call elsewhere.
    /// Both entries ran. With one global "previous record" this was read as
    /// startup -> startup, proven impossible, and one real execution was
    /// subtracted with a caveat saying it never happened.
    #[test]
    fn two_cpus_entering_the_same_block_back_to_back_are_two_executions() {
        let log = "IN: \n0x40080000:  d503201f  nop\n0x40080004:  94000165  bl #0x400805a0\n\n\
                   Trace 0: 0x1 [00004000/0000000040080000/00000521/ff020200] \n\
                   Trace 1: 0x1 [00004000/0000000040080000/00000521/ff020200] \n\
                   IN: kmain\n0x400805a0:  d65f03c0  ret\n\n\
                   Trace 0: 0x2 [00004000/00000000400805a0/00000521/ff020200] kmain\n";
        let mut t = parse(log);
        assert_eq!((t.tb_counts[&0x40080000], t.block_executions), (2, 3));
        assert!(!t.transitions.contains_key(&(0x40080000, 0x40080000)), "{:?}", t.transitions);
        // the same oracle as the single-CPU test: startup cannot re-enter itself
        assert_eq!(t.discard_impossible_repeats(&|from_last, to| (from_last, to) != (0x40080004, 0x40080000)), 0);
        assert_eq!((t.tb_counts[&0x40080000], t.block_executions), (2, 3));
        assert!(t.caveats.iter().all(|c| !c.contains("discarded")), "{:?}", t.caveats);
        assert!(t.caveats.iter().any(|c| c.contains("2 CPUs")), "{:?}", t.caveats);
        t.expand(&|_| vec![]);
        assert_eq!(t.counts[&0x40080004], 2);
        // CPU 0 went startup -> kmain; CPU 1 has gone nowhere yet
        assert_eq!(t.transitions, HashMap::from([((0x40080000, 0x400805a0), 1)]));
    }

    /// ... and an abandoned entry is still found when another CPU's records
    /// sit between the two halves of it in the log.
    #[test]
    fn an_abandoned_entry_is_still_found_within_one_cpus_stream() {
        let log = "IN: \n0x40080000:  d503201f  nop\n0x40080004:  94000165  bl #0x400805a0\n\n\
                   IN: kmain\n0x400805a0:  d65f03c0  ret\n\n\
                   Trace 0: 0x1 [0/0000000040080000/0/0] \n\
                   Trace 1: 0x1 [0/0000000040080000/0/0] \n\
                   Trace 0: 0x1 [0/0000000040080000/0/0] \n\
                   Trace 1: 0x2 [0/00000000400805a0/0/0] kmain\n\
                   Trace 0: 0x2 [0/00000000400805a0/0/0] kmain\n";
        let mut t = parse(log);
        assert_eq!(t.tb_counts[&0x40080000], 3);
        assert_eq!(t.discard_impossible_repeats(&|from_last, to| (from_last, to) != (0x40080004, 0x40080000)), 1);
        assert_eq!((t.tb_counts[&0x40080000], t.block_executions), (2, 4));
        assert_eq!(t.transitions, HashMap::from([((0x40080000, 0x400805a0), 2)]));
    }

    /// Edges come from the same consecutive records, so they had the same
    /// flaw. CPU 0 runs the fall-through of a conditional branch while CPU 1
    /// happens to enter the taken target: read as one stream that is a taken
    /// edge nobody took (0x1000 -> 0x1010), and a return into the middle of
    /// nowhere (0x1010 -> 0x1008).
    #[test]
    fn interleaved_cpus_give_per_cpu_transitions() {
        let tr = |cpu: u32, pc: u64| format!("Trace {cpu}: 0x1 [0/{pc:016x}/0/0] \n");
        // CPU 0: 0x1000 (ends b.hs 0x1010) -> 0x1008 (fall-through) -> 0x2000
        // CPU 1: 0x3000 -> 0x1010 -> 0x2000
        let log = [tr(1, 0x3000), tr(0, 0x1000), tr(1, 0x1010), tr(0, 0x1008), tr(1, 0x2000), tr(0, 0x2000)].concat();
        let t = parse(&log);
        let want = HashMap::from([((0x1000, 0x1008), 1), ((0x1008, 0x2000), 1), ((0x3000, 0x1010), 1), ((0x1010, 0x2000), 1)]);
        assert_eq!(t.transitions, want);
        assert_eq!(t.cpus.len(), 2);
        assert_eq!(t.block_executions, 6);
    }

    /// The three records the caveat is about, at their smallest. A block
    /// ending in a conditional branch, the record that would say which way it
    /// went, and the taken target -- with the middle record's CPU unreadable.
    ///
    /// Keeping each CPU's `prev_pc` across the unreadable record bridged the
    /// gap and drew (0x1000 -> 0x2000): a taken edge, measured, that nobody
    /// took. The real successor was the fall-through at 0x1008, which is what
    /// the unreadable record holds. This is the worst thing this tool can do,
    /// so the answer is no edge at all (self-review of #208).
    #[test]
    fn no_transition_is_drawn_across_a_record_whose_cpu_cannot_be_read() {
        let log = "IN: \n0x1000:  f1000400  subs x0, x0, #1\n0x1004:  54000062  b.hs #0x2000\n\n\
                   Trace 0: 0x1 [0/0000000000001000/0/0] \n\
                   Trace ?: 0x1 [0/0000000000001008/0/0] \n\
                   Trace 0: 0x1 [0/0000000000002000/0/0] \n";
        let mut t = parse(log);
        assert_eq!(t.transitions, HashMap::new(), "an edge was invented across the unreadable record");
        assert_eq!((t.block_executions, t.records_without_cpu), (3, 1));
        // the executions are all still there: only the ORDER is unknown
        assert_eq!((t.tb_counts[&0x1000], t.tb_counts[&0x1008], t.tb_counts[&0x2000]), (1, 1, 1));
        assert_eq!(t.discard_impossible_repeats(&|_, _| false), 0);
        assert!(t.caveats.iter().any(|c| c.contains("no block-to-block transition was drawn from or to such a record, nor across one")), "{:?}", t.caveats);
    }

    /// A record whose CPU cannot be read is an execution, and nothing more:
    /// no transition touches it and nothing is discarded from the log, even
    /// the repeat the single-CPU test removes.
    #[test]
    fn a_record_with_no_cpu_index_is_counted_but_never_reasoned_about() {
        let body = "IN: \n0x40080000:  d503201f  nop\n0x40080004:  94000165  bl #0x400805a0\n\n\
                    IN: kmain\n0x400805a0:  d65f03c0  ret\n\n";
        for who in ["Trace 0x7f00aa:", "Trace", "Trace x:", "Trace -1:"] {
            let log = format!("{body}{who} 0x1 [0/0000000040080000/0/0] \n{who} 0x1 [0/0000000040080000/0/0] \n{who} 0x2 [0/00000000400805a0/0/0] kmain\n");
            let mut t = parse(&log);
            assert_eq!((t.block_executions, t.records_without_cpu), (3, 3), "{who}");
            assert!(t.transitions.is_empty(), "{who}: {:?}", t.transitions);
            assert_eq!(t.discard_impossible_repeats(&|_, _| false), 0, "{who}");
            assert_eq!(t.tb_counts[&0x40080000], 2, "{who}");
            assert!(
                t.caveats.iter().any(|c| c.contains("3 of 3 exec record(s) could not be read in full")
                    && c.contains("no readable CPU index")
                    && c.contains("No repeated record was discarded")),
                "{who}: {:?}",
                t.caveats
            );
        }
        // One unreadable record among good ones. The transitions BEFORE it
        // stand -- they are two adjacent records of one readable CPU -- and
        // nothing is drawn across it: the last record cannot be placed after
        // the second, because the unreadable one may be CPU 0's own successor.
        // The subtraction is still off, as the caveat says.
        let log = format!("{body}Trace 0: 0x1 [0/0000000040080000/0/0] \nTrace 0: 0x1 [0/0000000040080000/0/0] \nTrace ?: 0x1 [0/0000000040080000/0/0] \nTrace 0: 0x2 [0/00000000400805a0/0/0] \n");
        let mut t = parse(&log);
        assert_eq!(t.transitions, HashMap::from([((0x40080000, 0x40080000), 1)]));
        assert_eq!(t.discard_impossible_repeats(&|_, _| false), 0);
        assert_eq!(t.tb_counts[&0x40080000], 3);
    }

    /// The same three records, with the middle one unreadable for every
    /// other reason a `Trace` line can be unreadable. The reviewer's point on
    /// #208: the boundary reset only ran once the PC had parsed and the CPU
    /// had not, so a record with a missing or reversed bracket, or a PC that
    /// is not a number, sailed past it and the records either side were
    /// bridged into a transition that never happened.
    ///
    /// Block A ends in `b.hs 0x2000`. Between A and 0x2000 is a record
    /// nobody can read. (A, 0x2000) is the taken edge; it must not appear,
    /// because whatever that middle record was, it ran in between.
    ///
    /// It calls `discard_impossible_repeats` too, and that call proves
    /// nothing: `transitions` is asserted empty two lines above it, so there
    /// is no repeat to remove and `0` comes back whatever the guard says.
    /// `a_log_with_an_unreadable_pc_has_no_repeat_discarded_either` is what
    /// covers the guard (review of #208).
    #[test]
    fn no_transition_is_drawn_across_any_unreadable_execution_record() {
        let head = "IN: \n0x1000:  f1000400  subs x0, x0, #1\n0x1004:  54000062  b.hs #0x2000\n\n\
                    Trace 0: 0x1 [0/0000000000001000/0/0] \n";
        let tail = "Trace 0: 0x1 [0/0000000000002000/0/0] \n";
        // every shape of unreadable record, one at a time
        for bad in [
            "Trace 0: 0x1 [0/000000000000zzzz/0/0] ", // a PC that is not a number
            "Trace 0: 0x1 [0/0000000000001008/0/0",   // no closing bracket
            "Trace 0: 0x1 0/0000000000001008/0/0] ",  // no opening bracket
            "Trace 0: 0x1 ]0/0000000000001008/0/0[ ", // brackets the wrong way round
            "Trace 0: 0x1 [] ",                       // nothing between them
            "Trace ",                                 // nothing at all
        ] {
            let mut t = parse(&format!("{head}{bad}\n{tail}"));
            assert_eq!(t.transitions, HashMap::new(), "an edge was invented across `{bad}`");
            // the two readable entries are still counted, and the unreadable
            // record is not in any block's count -- it is in no known block
            assert_eq!((t.tb_counts[&0x1000], t.tb_counts[&0x2000]), (1, 1), "{bad}");
            assert_eq!((t.block_executions, t.records_without_pc, t.unreadable_records()), (2, 1, 1), "{bad}");
            // and nothing is discarded from a log with one in it
            assert_eq!(t.discard_impossible_repeats(&|_, _| false), 0, "{bad}");
            assert!(t.caveats.iter().any(|c| c.contains("nor across one") && c.contains("in no block's count")), "{bad}: {:?}", t.caveats);
        }
        // the unknown-CPU record is the same decision, and still is
        let mut t = parse(&format!("{head}Trace ?: 0x1 [0/0000000000001008/0/0] \n{tail}"));
        assert_eq!(t.transitions, HashMap::new());
        assert_eq!((t.block_executions, t.records_without_cpu, t.records_without_pc), (3, 1, 0));
        assert_eq!(t.discard_impossible_repeats(&|_, _| false), 0);
    }

    /// The repeat cleanup is off for a log with ANY unreadable exec record,
    /// and the caveat on such a log says so in as many words -- "No repeated
    /// record was discarded from this log either". The guard tested only
    /// `records_without_cpu`, so a log whose unreadable records were of the
    /// other kind had repeats removed anyway and the caveat was untrue
    /// (review of #208). What that costs is not small: one unreadable PC
    /// among 203 records, the rest a block whose last instruction cannot
    /// branch back to its start, and 201 *measured* block executions were
    /// subtracted from a log the tool reports as incomplete.
    ///
    /// `no_transition_is_drawn_across_any_unreadable_execution_record` cannot
    /// catch this. It asserts `transitions` is empty two lines before it
    /// calls the cleanup, so there is no repeat to remove and `0` is the only
    /// answer the guard could give either way. The case here has a removable
    /// repeat in it, and refusing to remove it is the whole assertion.
    #[test]
    fn a_log_with_an_unreadable_pc_has_no_repeat_discarded_either() {
        // 0x1000 ends in a `nop`, which cannot branch back to 0x1000, so every
        // adjacent pair of its records is a repeat the cleanup would remove
        let head = "IN: \n0x1000:  d503201f  nop\n0x1004:  d503201f  nop\n\n";
        let good = "Trace 0: 0x1 [0/0000000000001000/0/0] \n";
        let bad = "Trace 0: 0x1 [0/000000000000zzzz/0/0] \n";
        let cannot_loop = |from_last: u64, to: u64| (from_last, to) != (0x1004, 0x1000);

        // one unreadable PC among 203 records: below the refusal threshold, so
        // the log is reported rather than refused -- and what its caveat
        // promises has to hold
        let mut t = parse(&format!("{head}{bad}{}", good.repeat(202)));
        assert_eq!((t.records_without_cpu, t.records_without_pc, t.unreadable_records()), (0, 1, 1));
        assert_eq!((t.block_executions, t.exec_records()), (202, 203));
        assert_eq!(t.refusal(), None, "1 in 203 is below the refusal threshold: this is the case the guard has to cover");
        assert_eq!(t.transitions[&(0x1000, 0x1000)], 201, "the repeats are there to be removed");
        assert!(t.caveats.iter().any(|c| c.contains("No repeated record was discarded")), "{:?}", t.caveats);
        assert_eq!(t.discard_impossible_repeats(&cannot_loop), 0, "measured block executions were discarded from a log the tool reports as incomplete");
        assert_eq!((t.block_executions, t.tb_counts[&0x1000]), (202, 202), "the counts were reduced anyway");
        assert!(t.caveats.iter().all(|c| !c.contains("were discarded")), "{:?}", t.caveats);

        // the same log with every record readable, so that the assertion above
        // is about the guard and not about an oracle that never fires
        let mut t = parse(&format!("{head}{}", good.repeat(202)));
        assert_eq!(t.unreadable_records(), 0);
        assert_eq!(t.discard_impossible_repeats(&cannot_loop), 201);
        assert_eq!((t.block_executions, t.tb_counts[&0x1000]), (1, 1));

        // and the kind the guard did cover reads the same, which is the point:
        // either kind means the streams cannot be told apart with certainty
        let mut t = parse(&format!("{head}Trace ?: 0x1 [0/0000000000002000/0/0] \n{}", good.repeat(202)));
        assert_eq!((t.records_without_cpu, t.records_without_pc), (1, 0));
        assert_eq!(t.discard_impossible_repeats(&cannot_loop), 0);
        assert_eq!(t.block_executions, 203);
    }

    /// A log whose exec records are mostly unreadable is not a trace with a
    /// gap in it; what it has to do with the run is unknown. Reported rather
    /// than refused, it would still be *measured* coverage -- the strongest
    /// claim this tool makes -- with the damage in a sentence at the bottom.
    #[test]
    fn a_log_of_mostly_unreadable_records_is_refused_not_caveated() {
        let good = "Trace 0: 0x1 [0/0000000000001000/0/0] \n";
        let bad = "Trace 0: 0x1 [0/000000000000zzzz/0/0] \n";
        // one bad record in 200 is a gap: reported, and everything it costs
        // is said
        let t = parse(&format!("{}{bad}{}", good.repeat(100), good.repeat(99)));
        assert_eq!((t.records_without_pc, t.exec_records()), (1, 200));
        assert_eq!(t.refusal(), None, "one record in 200 is a caveat, not a refusal");
        assert!(!t.caveats.is_empty());
        // a log that is half unreadable is refused, and the refusal counts
        let t = parse(&format!("{}{}", good.repeat(5), bad.repeat(5)));
        let why = t.refusal().expect("a half-unreadable log was reported, not refused");
        assert!(why.contains("5 of 10"), "{why}");
        assert!(why.contains("no measured coverage"), "{why}");
    }

    /// A `Trace` line whose brackets are the wrong way round used to panic:
    /// `rest[a + 1..b]` with `b < a` is a slice that runs backwards. The trace
    /// is untrusted input -- it is why `lib.rs` caps how much of it is read --
    /// so a malformed line has to be skipped, not fatal (self-review of #208).
    #[test]
    fn a_malformed_trace_line_is_skipped_not_fatal() {
        for bad in ["Trace 0] x [0/0000000000001000/0/0", "Trace ]", "Trace ][", "Trace 0]0["] {
            let t = parse(&format!("{bad}\nTrace 0: 0x1 [0/0000000000001000/0/0] \n"));
            assert_eq!(t.tb_counts.get(&0x1000), Some(&1), "{bad}");
        }
    }

    /// A one-block loop really does follow itself, and must be left alone --
    /// as must any block whose extent the log never listed.
    #[test]
    fn a_block_that_can_loop_to_itself_keeps_every_execution() {
        let log = "IN: spin\n0x1000:  f1000400  subs x0, x0, #1\n0x1004:  54ffffe1  b.ne #0x1000\n\n\
                   Trace 0: 0x1 [0/0000000000001000/0/0] spin\nTrace 0: 0x1 [0/0000000000001000/0/0] spin\n\
                   Trace 0: 0x1 [0/0000000000002000/0/0] \nTrace 0: 0x1 [0/0000000000002000/0/0] \n";
        let mut t = parse(log);
        // 0x2000 has no in_asm listing, so the question is never even asked
        assert_eq!(t.discard_impossible_repeats(&|_, to| matches!(to, 0x1000 | 0x2000)), 0);
        assert_eq!((t.tb_counts[&0x1000], t.tb_counts[&0x2000], t.block_executions), (2, 2, 4));
        assert!(t.caveats.iter().all(|c| !c.contains("discarded")));
    }

    /// An entry abandoned with something else logged in between -- `A, V, A`,
    /// an interrupt vector taken before the re-entry -- cannot be told from a
    /// real interrupt after A ran, so nothing is discarded. What it leaves is
    /// two marks, not one: A counted twice, and an edge out of a block
    /// abandoned before its first instruction. Both are now said out loud
    /// (self-review of #208).
    #[test]
    fn an_abandoned_entry_with_a_block_in_between_says_its_edge_is_not_an_outcome() {
        let log = "IN: \n0x1000:  d503201f  nop\n0x1004:  94000165  bl #0x3000\n\n\
                   IN: vector\n0x9000:  d65f03c0  ret\n\n\
                   Trace 0: 0x1 [0/0000000000001000/0/0] \n\
                   Trace 0: 0x1 [0/0000000000009000/0/0] vector\n\
                   Trace 0: 0x1 [0/0000000000001000/0/0] \n\
                   Trace 0: 0x1 [0/0000000000003000/0/0] \n";
        // from 0x1004 (`bl #0x3000`) control goes to 0x3000 or back to 0x1008;
        // 0x9000 is neither, and from 0x9000 (`ret`) nothing is known
        let mut t = parse(log);
        assert_eq!(t.discard_impossible_repeats(&|from_last, to| from_last != 0x1004 || to == 0x3000 || to == 0x1008), 0);
        // the count stays an upper bound: A really is logged twice
        assert_eq!((t.tb_counts[&0x1000], t.block_executions), (2, 4));
        assert_eq!(t.transitions[&(0x1000, 0x9000)], 1);
        assert!(t.caveats.iter().any(|c| c.contains("1 recorded block-to-block transition(s) are not edges the disassembly allows")), "{:?}", t.caveats);
        // and the same log with the vector reachable says nothing at all
        let mut t = parse(log);
        assert_eq!(t.discard_impossible_repeats(&|_, _| true), 0);
        assert!(t.caveats.iter().all(|c| !c.contains("not edges the disassembly allows")), "{:?}", t.caveats);
    }

    /// `build.sh` kills QEMU after 60 s, so the log can end mid-line -- and a
    /// truncated instruction line inside an `IN:` block fell through to the
    /// plain-address parser, which read `0x1000` out of `0x1000` and counted
    /// an execution at an address no instruction has, while `format` went on
    /// saying `qemu -d in_asm,exec` (self-review of #208).
    #[test]
    fn a_truncated_line_inside_an_in_asm_block_invents_no_count() {
        let log = "IN: \n0x1000:  d503201f  nop\n0x1004:  94000165  bl #0x3000\n\
                   0x100";
        let t = parse(log);
        assert_eq!(t.format, "qemu -d in_asm,exec");
        assert_eq!(t.counts.get(&0x100), None, "a count at a truncated address: {:?}", t.counts);
        assert_eq!(t.tb_instrs[&0x1000], vec![0x1000, 0x1004]);
        assert_eq!(t.unreadable_in_asm, 1);
        assert!(t.caveats.iter().any(|c| c.contains("1 line(s) inside an `IN:` block could not be read")), "{:?}", t.caveats);
    }

    /// A file butterfly does not understand is not an empty trace.
    #[test]
    fn a_file_in_no_known_format_is_unrecognised_not_empty() {
        let t = parse("<html><body>404 Not Found</body></html>\n");
        assert_eq!(t.format, "unrecognised");
        assert!(t.counts.is_empty() && t.tb_counts.is_empty());
    }

    #[test]
    fn counters_saturate_on_absurd_input() {
        // a hostile or corrupt trace must not overflow the counters
        let t = parse("0x1000 18446744073709551615\n0x1000 18446744073709551615\n");
        assert_eq!(t.counts[&0x1000], u64::MAX);
    }

    #[test]
    fn plain_format() {
        let t = parse("0x1000\n0x1004 3\n");
        assert_eq!(t.counts[&0x1004], 3);
        assert_eq!(t.format, "address list");
    }
}
