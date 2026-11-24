<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# Bazel

Builds [`../../c-lib`](../../c-lib) — the same sources as `build.sh`, and it
produces [identical evidence](../README.md). Verified with Bazel 9.2.0 on
macOS 15 / arm64. The Linux sample overrides Bazel's default stack-protector
flag to match `c-lib/build.sh`; otherwise the additional guard branch and
helper call change the evidence counts. This override belongs to the
reference-comparison example, not the reusable evidence-collection rule.

```sh
bazel build //flight:evidence \
  -c dbg --strip=never --spawn_strategy=local \
  --define llvm_bin="$(dirname "$(clang -print-prog-name=llvm-profdata)")"

# the coverage export records execroot-relative paths, so ingest from the
# source root
cd ../../c-lib
butterfly ingest ../build-systems/bazel/bazel-bin/flight/evidence \
  --source-root . -o /tmp/evidence.json
butterfly serve /tmp/evidence.json
```

**Those four flags are not optional, and the rule enforces them** (#245):
without `-c dbg` or `--define llvm_bin` it fails at analysis, and without
`--spawn_strategy=local` or `--strip=never` the action fails — one `.su` per
object is required, `dsymutil` must succeed, and the image must still have a
line table. Each failure names the flag. The reasons each matters are below.

The compiler and flags in `build-info.txt` are read from Bazel, not
repeated: the C++ toolchain's compiler (its path and `--version`), its target
CPU, and the toolchain's effective compile command line for each target's own
`copts`.

## Checklist — [the contract](../README.md#the-contract) in this build system

| | how Bazel does it | where |
|---|---|---|
| 1. two builds, one source list | two `cc_binary` from one `//:sources` | `flight/BUILD.bazel` |
| 2. `coverage.json` | in the `butterfly_evidence_dir` action | `flight/butterfly.bzl` |
| 3. `.su` collected | from the analysis target's own `_objs`, in the same action — **needs `--spawn_strategy=local`** | `flight/butterfly.bzl` |
| 4. `build-info.txt` | written by the same action | `flight/butterfly.bzl` |
| 5. `dsymutil` on macOS | in the action — **needs `--spawn_strategy=local`** | `flight/butterfly.bzl` |
| 6. matching LLVM | `--define llvm_bin=…`, since an action has no useful `PATH` | `flight/butterfly.bzl` |

## Bazel fights this harder than CMake or make

Everything here was found by running it. Each produced a build that
*succeeded* while the evidence was wrong.

**There is no build directory.** CMake and make leave everything in one place
to point `ingest` at. Bazel scatters outputs through `bazel-out` and treats
the action sandbox as read-only, so `butterfly_evidence_dir` exists purely to
assemble that directory. That rule *is* the integration; the two `cc_binary`
targets are ordinary.

**`fastbuild` throws the debug info away.** Bazel's default compilation mode
strips it regardless of your `-g`, so the binary has no line table, every
instruction lands in "no source line", and the projection is worthless — 456
of 456 instructions `unknown`, with no error anywhere. `-c dbg --strip=never`
is what makes butterfly's core output mean anything.

**The debug map points into a sandbox that no longer exists.** On macOS,
linking objects leaves the line table in the `.o` files behind a debug map,
and Bazel records those objects at their *sandbox* paths:

```
OSO …/sandbox/darwin-sandbox/70/execroot/_main/bazel-out/…/crc.o
```

That directory is destroyed when the link action finishes, so `dsymutil` can
never resolve it — not from a later action, not from your shell. This is
structural, not a mistake in the rule. `--spawn_strategy=local` runs the
actions in the execroot instead, where the objects persist and the debug map
resolves. (`--apple_generate_dsym` does not help: it applies to the Apple
rules, not to `cc_binary`.)

**`.su` files are undeclared outputs.** Bazel tracks declared outputs, and
`-fstack-usage` writes its files as a side effect nothing declared. The rule
gathers them from the analysis binary's own `_objs` directory — derived from
the binary path, so it is scoped to exactly this target and this
configuration — and **refuses to run if two `.su` files share a basename**,
rather than globbing all of `bazel-out` and letting copy order decide which
stack record survives. It still needs the objects to be on disk, which is the
same `--spawn_strategy=local` again: under sandboxing you get **zero** `.su`
files and every stack frame silently estimated from the prologue.

The declared-outputs rule is the correct fix and would remove both the
scoping and the local-strategy requirement; it is more than an example should
carry, and is noted as the proper answer below.

**Coverage records the compile-time path.** Each compile happens in a
*different* sandbox, so `coverage.json` named directories that no longer
existed and ingest failed outright on the first source it tried to read.
`-ffile-compilation-dir=.` makes clang record execroot-relative paths
instead, which is why `ingest` is run from the source root above.

**An action has no useful `PATH`.** `llvm-profdata` is simply not found, and
taking it from your shell's `PATH` would be wrong anyway: a raw profile
carries a format version that must match the clang that wrote it, and Bazel's
C++ toolchain need not be the clang on your `PATH`. Hence `--define
llvm_bin=…`, explicitly.

## Honest limitations

`--spawn_strategy=local` gives up sandboxing for this build, which is a real
cost — it is what makes Bazel's caching trustworthy. It is used here because
the alternative is evidence that is wrong without saying so. A project that
needs both should generate the dSYM and the `.su` files as **declared
outputs** of their own rules rather than recovering them afterwards; that is
more work than an example should carry, and it is the right answer.

Nothing here is specific to remote execution, which was not tried. Expect the
`_objs` collection to be the first thing that breaks there.

## Sharing sources with the other examples

`src` and `include` are symlinks to `../../c-lib`, so all four build systems
compile one copy. In your own project they are ordinary directories and none
of this applies.

> On a checkout without symlink support (a default Windows clone, `git config
> core.symlinks false`), git materialises these as text files holding the
> target path, and the glob finds no sources. This whole toolchain is
> Unix-only anyway — `dsymutil`, clang instrumentation — so it is a caveat,
> not a regression. In your own project the directories are real and it does
> not arise.
