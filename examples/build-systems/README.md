<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# butterfly in a real build system

The worked examples build through a hand-written `build.sh`, which is clear
about *what* butterfly needs but is nobody's actual build. These show the same
thing in the build systems people have:

| | builds | verified here |
|---|---|---|
| [`cmake/`](cmake) | CMake ≥ 3.20, Makefile or Ninja generator | **yes** — built and ingested |
| [`make/`](make) | GNU make, no generator | **yes** — built and ingested |
| [`bazel/`](bazel) | Bazel 9 with Bzlmod | **yes** — built and ingested, with [four required flags](bazel/README.md) |

All three build **the same sources** as [`../c-lib`](../c-lib) — one copy, four
build systems — so the evidence they produce can be compared directly. It is
identical:

| | `build.sh` | CMake | make | Bazel |
|---|---|---|---|---|
| files / functions / symbols | 8 / 18 / 18 | 8 / 18 / 18 | 8 / 18 / 18 | 8 / 18 / 18 |
| lines covered | 87/112 | 87/112 | 87/112 | 87/112 |
| branches covered | 25/40 | 25/40 | 25/40 | 25/40 |
| gaps | 30 | 30 | 30 | 30 |
| `.su` files ingested | 6 | 6 | 6 | 6 |
| frames from `.su` (not estimated) | 18 | 18 | 18 | 18 |

Bazel needs [four flags](bazel/README.md) to get there. Without them it builds
happily and produces evidence in which every instruction is `unknown`.

## The contract

Independent of build system, butterfly needs six things. Every README here is
a checklist against this list, and so is any integration you write yourself.

1. **Two builds of the same sources.**
   - **instrumented** — `-fprofile-instr-generate -fcoverage-mapping`
     (`-fcoverage-mcdc` for Level A), run to produce a `.profraw`.
   - **analysis** — no instrumentation, `-g`, `-fstack-usage`.
     **This is the binary that is certified and the one butterfly
     disassembles.** The coverage is only *projected* onto it.

   They must come from one source list. Two lists drift, and then the coverage
   you measured is not the code you analysed — which nothing downstream can
   detect.

2. **`coverage.json`** — `llvm-profdata merge -sparse` then
   `llvm-cov export -format=text`.

3. **`.su` files** in the directory being ingested. Without them butterfly
   estimates every stack frame from the prologue, which is weaker evidence
   (§6.3.4.f) and says so.

4. **`build-info.txt`** — `key=value` lines recorded in the evidence
   provenance. `compiler`, `flags_instr` and `flags_target` are the ones the
   baseline names; anything else you add is carried through. Record the
   compiler that really ran (`compiler_path`, `target`), and in
   `flags_target` the flags the analysed image was **really** built with:
   butterfly holds the build to them (below). Record what produced
   `coverage.json` too, under the keys `butterfly build` writes: the compiler
   of the instrumented build (`compiler_instr`, `compiler_instr_path`) and
   the tools that read its profile (`llvm_profdata`, `llvm_profdata_path`,
   `llvm_cov`, `llvm_cov_path`), each version the line of `--version` that
   carries a number. The baseline compares them as a `coverage-toolchain`
   difference (#298).

5. **`dsymutil` on macOS**, after any link of separate `.o` files.

6. **`llvm-profdata` matching the clang that wrote the profile.** Raw profiles
   carry a format version that has to agree exactly.

## What butterfly now refuses

`ingest` holds a build to what its `build-info.txt` declares, whatever built
it ([#245](https://github.com/dhaksh64/butterfly/issues/245)):

- `flags_target` with no effective `-g` (none, or a later `-g0`): refused.
- `-g` declared, and the image has no line table — stripped, or no `.dSYM` on
  macOS: refused.
- `-fstack-usage` declared, and an analysed function has no `.su` record — a
  file lost, or two flattened onto one name: refused, naming the functions.
- `.su` files the recorded flags do not declare: accepted, with a caveat that
  the provenance does not describe the build.

The shell examples share [`examples/adapter.sh`](../adapter.sh) for the
three steps that used to end in `|| true`: one object per unit
(`compile_each`), collecting `.su` files without letting one overwrite
another (`collect_su`), and a `dsymutil` that must succeed (`link_debug`).
CMake and Make stop on a missing or colliding `.su` too, and the Bazel rule
checks each of its four flags itself.

## The four that used to fail quietly

Before those checks, not one of these produced an error. Each produced
evidence that looked complete and was wrong. The last three are now refused
by `ingest` when `build-info.txt` declares what the build did; the first is
caught by the instrumentation preflight.

| mistake | what you see instead of an error |
|---|---|
| ingesting the **instrumented** binary | coverage that looks perfect — you are analysing the counters, not the delivered code |
| no `dsymutil` on macOS | every instruction in "no source line"; the projection is worthless |
| `.su` files left where the compiler dropped them | every frame silently estimated from the prologue |
| compiling all units in one command with `-fstack-usage` | **one** `.su` for the whole program instead of one per unit |

## Checklist for your own integration

- [ ] One source list feeds both builds
- [ ] The analysis build has **no** `-fprofile-*` or `-fcoverage-*` flag
- [ ] The analysis build has `-g` (butterfly needs the line table) and `-fstack-usage`
- [ ] Each translation unit is compiled separately, so each gets its own `.su`
- [ ] `.su` files end up in the directory you will ingest
- [ ] The instrumented binary is run with `LLVM_PROFILE_FILE` set
- [ ] `llvm-profdata` and `llvm-cov` come from the same LLVM as the compiler
- [ ] `dsymutil` runs on macOS after linking objects
- [ ] `build-info.txt` records the compiler (banner, absolute path, target) and **both** flag sets, as really used
- [ ] `butterfly ingest <dir> --source-root <src>` succeeds with no other flags
- [ ] The summary it prints matches what you expect: function count, and
      `stack ... via symbol` rather than everything estimated

## Verifying an integration

Compare it against a build you trust, rather than reading the numbers in
isolation:

```sh
./examples/c-lib/build.sh
butterfly ingest examples/c-lib/build --source-root examples/c-lib -o /tmp/ref.json

cmake -S examples/build-systems/cmake -B /tmp/cm -DCMAKE_C_COMPILER=clang && cmake --build /tmp/cm
butterfly ingest /tmp/cm --source-root examples/c-lib -o /tmp/mine.json

butterfly summary /tmp/ref.json
butterfly summary /tmp/mine.json      # the two should agree
```

Small differences in symbol *ordering* are expected — link order is a build
system's own business. Differences in **counts** are not, and mean a step in
the contract above was missed.

`scripts/verify-build-systems.sh` does this for CMake, Make and Bazel, and
also runs each in an altered mode — one `.su` removed from its output, and
for Bazel each mandatory flag left out — and requires that to be refused.
