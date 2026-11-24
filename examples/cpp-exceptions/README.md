<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# C++ exceptions and RTTI

Every other C++ example is built with `-fno-exceptions -fno-rtti`. This one
is the same program built both ways, so what exceptions and RTTI add to the
object code, and what butterfly makes of it, can be read side by side:

- `build/` — with exceptions and RTTI, clang++'s defaults. `safe_altitude`
  calls `checked_altitude` in a `try`, which **throws** for an out-of-range
  value, and a `catch` handles it; `altitude_of` uses **`dynamic_cast`**.
- `build-noexc/` — with `-fno-exceptions -fno-rtti`. The compiler's feature
  macros (`__cpp_exceptions`, `__cpp_rtti`) switch the same functions to an
  error code and to a virtual call.

`main` runs the throw once, so the catch handler executes.

```sh
./build.sh
butterfly ingest build       --source-root . -o build/evidence.json
butterfly ingest build-noexc --source-root . -o build-noexc/evidence.json
```

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 9 |
| source files | 1 |
| lines covered | 34 / 36 |
| branch outcomes | 5 / 6 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

## The two builds side by side

Measured on this host (Apple clang 17, arm64); another toolchain gives other
counts, and the shape is the point.

| | with exceptions and RTTI | `-fno-exceptions -fno-rtti` |
|---|---|---|
| instructions (this host's) | 214 | 166 |
| instructions with no source line (this host's) | 7 | 3 |
| gaps, by kind (this host's) | 15 helper-call, 5 line-zero, 2 extra-object-branch | 4 helper-call, 3 indirect-branch, 2 line-zero |
| landing pads | reached through the exception table | none |
| C++ runtime calls | `__cxa_throw`, `__cxa_begin_catch`, `__cxa_end_catch`, `__cxa_allocate_exception`, `_Unwind_Resume`, `__dynamic_cast` | none |
| stack worst case | bounded; the unwinder's frames are not counted | 2 chains unbounded: the virtual calls |

## What butterfly shows

- **Landing pads are reached, not unreachable.** The unwinder jumps to a
  landing pad; no branch does. butterfly reads the function's exception table
  — found through `__unwind_info` on Mach-O, `.eh_frame` on ELF — and draws an
  **exception edge** from each block whose calls it covers to the pad. Before,
  the `catch` in `safe_altitude` that had just run was reported unreachable.
- **An exception edge is never projected covered.** A landing pad's code
  borrows lines that also run on the normal path (a closing brace, a
  destructor), so line coverage cannot say whether an exception took the
  edge. It is shown uncovered when the pad never ran, unknown otherwise.
- **The C++ runtime is classified** as the language runtime (`core-lib`):
  `__cxa_*`, `_Unwind_*`, `__dynamic_cast` need their own verification credit
  or a qualified library, like the standard library. `__clang_call_terminate`
  — an exception escaping a `noexcept` function — is a runtime check.
- **The implicit destructors** (`Altimeter::~Altimeter()`, called on the
  unwind path and at scope end) have no coverage record of their own, so
  their calls are `project-uncovered` helper calls, in both builds.
- **`dynamic_cast` replaces the virtual calls**, so the RTTI build has no
  indirect branches here and the no-RTTI build has three.
- **Not shown**: the unwinder's own stack use when an exception is thrown
  (`_Unwind_RaiseException` and the personality routine run outside this
  image), and whether each exception edge was taken — neither is in the
  evidence.

`crates/ingest/tests/cpp_exceptions.rs` asserts the landing pads, the
projection and the classification, and that the no-exceptions build has none
of it.
