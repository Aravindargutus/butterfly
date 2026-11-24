<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# GNU make

Builds [`../../c-lib`](../../c-lib) — the same sources as `build.sh`, and it
produces [identical evidence](../README.md).

```sh
make
butterfly ingest build --source-root ../../c-lib -o build/evidence.json
butterfly serve build/evidence.json

make evidence      # the two steps above in one
make clean
```

Override anything the usual way: `make CC=clang-18 LLVM_BIN=/usr/lib/llvm-18/bin`.

## Checklist — [the contract](../README.md#the-contract) in this build system

| | how make does it | where |
|---|---|---|
| 1. two builds, one source list | `$(SRC)` used by both `flight-instr` and `$(OBJ)` | `Makefile` |
| 2. `coverage.json` | one rule, profdata → cov | `Makefile` |
| 3. `.su` collected | `mv` in the per-object recipe | `Makefile` |
| 4. `build-info.txt` | its own rule | `Makefile` |
| 5. `dsymutil` on macOS | after the link, guarded by `command -v` | `Makefile` |
| 6. matching LLVM | `$(CC) -print-prog-name=llvm-profdata`, override `LLVM_BIN` | `Makefile` |

## What make makes awkward

**`CC ?= clang` does not work.** Make predefines `CC` as `cc`, so `?=` never
fires. On macOS `cc` *is* clang and the mistake hides completely; on Linux `cc`
is usually gcc, which has no `-fprofile-instr-generate` and the build fails
somewhere confusing. Override only the built-in default:

```make
ifeq ($(origin CC),default)
CC := clang
endif
```

This is in this example's history — it built fine here and would have failed
on the Linux CI runner.

**Compile one translation unit at a time.** `-fstack-usage` writes one `.su`
per object, so a single compile-and-link of all six sources produces one `.su`
for the lot, and every function falls back to a prologue estimate. The
per-object rule exists for that reason, not for incremental builds.

**`-fstack-usage` writes the `.su` next to the object**, so the recipe moves it
to the ingest directory immediately. Doing it in a separate pass invites a
parallel build to race it.
