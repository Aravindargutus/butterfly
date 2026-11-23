# The GCC example

The same shape as `examples/c`, built with **GCC** instead of clang, so
butterfly's gcov front end is exercised on real GCC output rather than on a
fixture somebody wrote by hand (#45).

```sh
./build.sh                       # CC=gcc-14 ./build.sh to pick a compiler
butterfly ingest build --source-root . -o build/evidence.json
butterfly summary build/evidence.json
```

## What GCC gives, and what it does not

| | from GCC |
|---|---|
| statement coverage (A-7 obj 7) | **yes** — gcov states a count for every line |
| decision coverage (obj 6) | **yes**, with `gcov -b` |
| condition coverage | with `-fcondition-coverage` (GCC 14+) |
| **MC/DC (obj 5)** | **no** |
| the whole object-code side | **yes** — DWARF, symbols, disassembly, CFG, stack |

**MC/DC cannot come from GCC.** `-fcondition-coverage` records that each
condition was taken both ways; MC/DC requires an *independence pair* for each
condition — two runs differing in that condition alone and in the decision's
outcome. GCC emits nothing those could be reconstructed from. A GCC project
gets objective 5 from the object-level path instead: measured branch outcomes
from a trace, which butterfly already supports.

`mode_ok` in `limits.c` is here to make the difference visible. Called once
with `(1, 1, 0)`, the short circuit means `wow` is never evaluated at all —
so its condition has neither outcome, and no amount of condition coverage
turns that into an independence pair.

## Why the outcomes are not called "true" and "false"

gcov reports each branch as a **CFG edge** with a `fallthrough` flag, not as
the polarity of the condition. Which edge carries the true outcome depends on
the construct — measured on GCC 16:

| construct | `fallthrough: true` is |
|---|---|
| `if`, `if`/`else` | the condition being **true** |
| `&&`, `\|\|` | the condition being **true** |
| `while`, `for`, `do`/`while` | the loop **exit**, so **false** |

Reading `fallthrough` as polarity would report every loop backwards. That
would not just mislabel a column: the direction is part of a resolution key,
so a reviewer's justification would be recorded against the outcome they did
not justify.

So butterfly reports both outcomes of a gcov decision and names neither.
Decision coverage is unaffected — "were both taken" does not depend on which
is which — and `sum_to` is here as the loop that makes the inversion real.

Naming them properly means reading the branch instruction in the object code,
which butterfly already disassembles. That is a separate change, not a guess
made in the reader.

## What is in `limits.c`

| function | why |
|---|---|
| `clamp_alt` | two `if`s; the second is reached once, so one outcome is missing |
| `mode_ok` | short-circuit `&&`/`\|\|`; `wow` is never evaluated |
| `sum_to` | a `for` loop — the construct whose gcov direction inverts |
| `unreachable_helper` | never called, so the example contains a real finding |
| `rate_ok` (`util.c`) | a **second translation unit** — gcov writes one JSON document per data file, and a reader that mishandles that drops every unit after the first |
