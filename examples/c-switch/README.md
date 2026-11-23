<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# A switch compiled to a jump table

One function, `dispatch`, with a sixteen-case `switch` whose cases each do
something different. That forces the compiler to branch rather than look a
value up, and with this many dense cases it branches **through a table**: an
address computed from the case number, then one indirect branch (`br` on
AArch64, `jmpq *` on x86-64). `examples/c`'s `phase_name` switch is too small
for that and compiles to a chain of compares.

```sh
./build.sh
butterfly ingest build     --source-root . -o build/evidence.json      # -O0
butterfly ingest build-o2  --source-root . -o build-o2/evidence.json   # -O2
```

`main` runs cases 0 to 5 only, so the same indirect branch has covered and
uncovered targets.

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 2 |
| source files | 1 |
| lines covered | 16 / 27 |
| branch outcomes | 25 / 36 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

## What butterfly shows

butterfly reads the table from the image (`crates/ingest/src/jumptable.rs`)
from the instructions that use it: the bounds check that gives the number of
entries, the table's address, the width of an entry and what it is added to.

- **The Flow view draws one edge per case** from the indirect branch to the
  case's block, coloured like any other edge: six covered, ten uncovered at
  `-O0`, because cases 6 to 15 never ran.
- **No case block is called unreachable.** Before the table was read, the
  indirect branch had no successors and all sixteen were.
- **Each case's branch region is located in the object code** — it is the
  table edge into that case, and the `default` is the bounds check's taken
  branch — so none is reported as a predicated decision.
- **The table is data.** At `-O0` on AArch64 clang places it inside
  `dispatch` itself, and it used to be disassembled as sixteen `udf`
  instructions; on x86-64 Mach-O it is inline too. Its bytes are left out of
  the instruction count.
- **The indirect branch is still a gap** (`indirect-branch`), now saying its
  targets were read from the table: a reviewer confirms the table and its
  bound belong to the switch.
- **The bounds check is an `extra-object-branch` gap**: a compiler-generated
  conditional with no source decision of its own.
- **At `-O2` the `puts` calls are tail calls**: `puts(...); break;` becomes a
  branch to `puts` (`b _puts`) instead of a call and a return. A direct branch
  out of the function is a call to the call graph and the helper-call check,
  and ends its block like a return (#264).

`crates/ingest/tests/jump_table.rs` asserts all of this at `-O0` and `-O2`.
The table is only claimed when every piece of the pattern is found; a
compiler that emits the switch another way leaves the branch unresolved,
and then no block behind it is called unreachable either.
