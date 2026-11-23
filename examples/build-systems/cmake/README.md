<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# CMake

Builds [`../../c-lib`](../../c-lib) — the same sources as `build.sh`, and it
produces [identical evidence](../README.md).

```sh
cmake -S . -B build -DCMAKE_C_COMPILER=clang
cmake --build build
butterfly ingest build --source-root ../../c-lib -o build/evidence.json
butterfly serve build/evidence.json
```

`-DCMAKE_C_COMPILER=clang` matters: CMake's default compiler may be gcc, which
has no `-fprofile-instr-generate`. Point `-DLLVM_BIN=...` at an LLVM install if
your clang does not ship `llvm-profdata` beside itself.

## Checklist — [the contract](../README.md#the-contract) in this build system

| | how CMake does it | where |
|---|---|---|
| 1. two builds, one source list | two `add_executable` from one `${SOURCES}` | `CMakeLists.txt` |
| 2. `coverage.json` | `add_custom_command` chaining profdata → cov | `CMakeLists.txt` |
| 3. `.su` collected | `POST_BUILD` running `collect_su.cmake` | `collect_su.cmake` |
| 4. `build-info.txt` | `add_custom_command` running `build_info.cmake` | `build_info.cmake` |
| 5. `dsymutil` on macOS | `POST_BUILD` guarded by `if(APPLE)` | `CMakeLists.txt` |
| 6. matching LLVM | `clang -print-prog-name=llvm-profdata`, override `-DLLVM_BIN` | `CMakeLists.txt` |

## What CMake makes awkward

**`-fstack-usage` writes each `.su` beside its object file**, which for CMake
means `CMakeFiles/flight.dir/<the full source path>/`. butterfly looks in the
directory it is ingesting, so a `POST_BUILD` step copies them up. This is the
step most likely to be forgotten, and forgetting it produces no error: every
stack frame is silently estimated from the prologue instead.

**Two `OUTPUT`s on one `add_custom_command` share one recipe.** Writing

```cmake
OUTPUT flight.profraw run.txt      # don't
```

makes two make targets with the same recipe, and a parallel build can run it
twice at once — which produced a `coverage.json` from a profile still being
written. One `OUTPUT`, with `BYPRODUCTS` for the rest.

**`VERBATIM` already quotes each argument.** Quoting inside a `=value` too:

```cmake
COMMAND llvm-cov export -instr-profile="${DIR}/x.profdata"   # don't
```

reaches the tool as a path containing literal `"` characters, and it reports
`could not read profile data`. Both mistakes are in this history; both cost a
build.

## Ninja

Works unchanged — `cmake -S . -B build -G Ninja`. Only the Makefile generator
was exercised here.
