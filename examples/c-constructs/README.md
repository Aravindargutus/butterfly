<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# c-constructs — the C that no other example has

One file, `constructs.c`, with one small function per construct (#168):

| construct | functions | what `main` does with it |
| --- | --- | --- |
| dynamic stack frames | `vla_sum` (a VLA), `alloca_fill`, and `calls_vla`, which calls `vla_sum` | runs them |
| `goto` | `goto_cleanup` (the cleanup idiom), `goto_retry` (a backward goto), `goto_into_loop` (a goto into a loop body: irreducible flow) | never takes a cleanup goto, or the goto into the loop |
| a union | `float_bits`, `low_byte` on the global `shared_word`; `pack_local` on a local | writes one member, reads another |
| a bitfield struct | `arm`, `is_armed` on the global `unit_status`; `pack_local` on a local | sets fields |
| a variadic function | `sum_ints(int count, ...)` | sums three |
| a macro that expands to control flow | `checked_scale`, three uses of `REQUIRE(cond)`, a plain-block `{ if (!(cond)) return -1; }`; `in_range`, a two-condition `IN_RANGE` in an `if` | never makes a `REQUIRE` return; one value in range |

```sh
./examples/c-constructs/build.sh
target/debug/butterfly ingest examples/c-constructs/build \
  --source-root examples/c-constructs -o /tmp/ev.json
target/debug/butterfly serve /tmp/ev.json
```

`-O0`, so each construct reaches the object code as written.
`_FORTIFY_SOURCE` is pinned off, as in `examples/c-callgraph`: with it on,
the SDK rewrites `alloca_fill`'s `memset` to `__memset_chk`, and the
`memset` written in the source is then reported as a folded call.

## The numbers

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 15 |
| source files | 1 |
| lines covered | 120 / 134 |
| branch outcomes | 20 / 30 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

Asserted by `crates/ingest/tests/example_numbers.rs`. The branch outcomes
include the three `REQUIRE` decisions and the two conditions of `IN_RANGE`,
which llvm-cov records inside the macro and butterfly places at each use.

## What butterfly shows

Observed on Apple clang 17 / arm64 Mach-O; `crates/ingest/tests/c_constructs.rs`
asserts each point in a form that holds on x86-64 too.

**Dynamic frames are not bounded.** The Stack tab reports `vla_sum` and
`alloca_fill` not bounded, reason *dynamic frame (alloca / VLA)*, and so
`calls_vla` and `main` above them. The size shown is the static part of the
frame only. The `.su` marks the two frames `dynamic`; without a `.su` the
run-time stack-pointer write in the function body is recognised instead.
Before #168 that second path saw only the prologue and reported the VLA
bounded by its static frame on a target with no stack probe (on this host,
the probe call made it unbounded for the wrong reason). On AArch64 Darwin
that probe, `___chkstk_darwin`, is called through its GOT slot
(`adrp x16; ldr x16, [x16, #off]; blr x16`). butterfly reads which symbol
the image binds the slot to, so the `blr` is a direct call to the probe — a
`helper-call` gap of class `runtime-check` — and not a function-pointer
call; until #271 it was reported as an indirect call.

**`goto` is an ordinary jump.** No block of any of the three functions is
called unreachable. The never-taken gotos, and the label code only they reach,
are uncovered; the retry's backward goto is a covered back edge; the goto into
the loop is an uncovered second entry into a covered loop body. The single
return block of `goto_cleanup` borrows the closing brace from DWARF, and
llvm-cov counts that line inside the zero-count `undo_a:` label, so its line
reads 0. It ran on every call, though: the covered `return rc;` block holds
the only code of its line, makes no call and jumps straight into it. butterfly
infers it covered from that edge and shows it as `shared-line`, with the
reasoning in the block note. Until #270 it was projected uncovered, and a
reviewer was sent to cover code that had run.

**A decision inside a macro is placed at the macro's use.** llvm-cov puts the
`if` of each `REQUIRE` at the `#define` line, in a file id of its own per
use. butterfly used to drop those regions: the three object branches were
then `extra-object-branch` gaps ("no source-level decision"), and the
never-run `return -1` path — every instruction of which DWARF attributes to
the covered use line — projected **covered**. Now each use is a decision on its
own line, true 0 / false 1, and the return path is projected uncovered by
branch-outcome inference, shown as `shared-line`. `in_range` has a decision
of two conditions inside a macro, `IN_RANGE(v, lo, hi)`. Its MC/DC record is
placed at the macro's use too, with its conditions in the `#define`'s order
(true 1 / false 0 each; one run shows neither independent). Until #269 that
decision was reported at the `#define` line, outside the function, with a
`predicated-decision` gap claiming it had no object code. The LLVM 18 record
layout (Apple clang 17 here, clang 18 in CI) names no file for a decision, so
butterfly recovers the file from where the decision's conditions lie. If
that does not settle it, for example two uses of one macro in a function
with different MC/DC results, the decision is left out and a caveat names it.

**Unions and bitfields are whole objects.** `shared_word` is one global
that `float_bits` and `low_byte` both read and write, whichever member they
name; `unit_status` likewise. A bitfield write is the read-modify-write of
the word that holds it, so `arm` both reads and writes `unit_status` on
each field-assignment line. As locals in `pack_local`, the variables panel
attributes the same accesses to `s` and `w` as wholes (AArch64 only; on
other architectures the variables are listed with the reason). Which field
or member is not reported.

**Varargs.** `sum_ints` has a static, bounded frame and an ordinary loop
decision. `va_start` takes `ap`'s address, so what `va_arg` reads through it
is not attributed, and the variables panel says so. On this host `va_list`
is a plain pointer and `va_arg` has no branch; clang's x86-64 code for the
same function branches in `va_arg` and in the prologue, which appear there
as `extra-object-branch` gaps.

**Found while writing this, and fixed in the same change:** in `arm`,
`if (count < 15) count++;` ran with the condition true, yet the block on
that path was inferred uncovered, because the join after the `if` had run
and was taken as evidence for the other outcome. An inference now uses a
successor's coverage only if the other successor cannot flow into it.
