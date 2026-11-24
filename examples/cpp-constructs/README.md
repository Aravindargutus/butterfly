<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# Everyday C++: constructors, operators, lambdas, new/delete

`constructs.cpp` is the ordinary C++ the other examples avoid (#166). Each
construct puts object code in the image that the source spells differently,
or does not spell at all:

| construct | where | what the compiler makes of it |
|---|---|---|
| user-written constructor and destructor | `Channel`, `Boosted` | several symbols per source function: `C1`/`C2`, and `D0`/`D1`/`D2` for a virtual destructor |
| destructor of a local | the block in `lifetimes` | a call on the closing `}`, with no statement of its own |
| overloaded operators | `operator+`, `operator+=`, `operator==`, `operator[]` | ordinary functions, called where the source writes `a + b` |
| lambdas | `lambdas`, `Channel::peak` | a closure type with an `operator()` whose name the compiler chooses |
| `new` / `delete` | `Channel`'s buffer, `lifetimes` | calls into the C++ runtime; `delete` through a base pointer is a virtual call |

```sh
./examples/cpp-constructs/build.sh
cargo run -- ingest examples/cpp-constructs/build --source-root examples/cpp-constructs -o examples/cpp-constructs/build/evidence.json
cargo run -- serve examples/cpp-constructs/build/evidence.json
```

Built like [`examples/cpp`](../cpp): `clang++ -std=c++17 -fno-exceptions
-fno-rtti`, `-O0`. Exceptions and RTTI are a separate example (#165).

<!-- butterfly:numbers -->
| | |
|---|---|
| functions | 21 |
| source files | 1 |
| lines covered | 83 / 86 |
| branch outcomes | 19 / 24 |
| instructions | host-dependent |
<!-- /butterfly:numbers -->

The 21 functions include the lambda's conversion operator and `__invoke`,
which have no coverage record: they count as functions, but neither as
executed nor as unexecuted (#273).

## What butterfly shows

Observed on Apple clang 17, arm64 Mach-O. `crates/ingest/tests/cpp_constructs.rs`
asserts each point below that is not marked as a limitation.

**Lambdas.** Each of the four closures is a function in the list with its own
object code and coverage. The three inside the non-inline `lambdas` are named
`$_0`, `$_1`, `$_2` and carry llvm-cov's `constructs.cpp:` qualifier (they have
internal linkage); the one inside the inline `Channel::peak` is named by its
signature, `{lambda(int, int)#1}`. The Calls tab has the edges into each
`operator()`: from the `count_if` template instantiation, from `lambdas`
directly, and from `peak`.

The lambda converted to a function pointer brings two functions the source
never names: a conversion operator and a static `__invoke`. Neither has a
coverage record, so each is a function of its own inside `lambdas`, with its
instructions projected unknown rather than borrowing `lambdas`' line counts
(#273). The call to the conversion is an ordinary call edge, and the call
through the pointer is an `indirect-branch`. The closure's `operator()` is
called only from `__invoke`, whose address the conversion operator takes, so it
is not a dead-code candidate.

**Overloaded operators** are plain functions with demangled names
(`fc::operator==(fc::Vec2 const&, fc::Vec2 const&)`), inventoried with the
`operator` construct. `a + b` in `main` is a call edge to `operator+`, which
calls `operator+=`. `operator==` is a two-condition decision with MC/DC; `main`
compares once, so neither condition is shown independent.

**Constructors and destructors.** llvm-cov records each one once, under the
variant that holds the body (`C2`, `D2`). On Mach-O the complete-object
variants `C1` and `D1` are separate thunks that call it, and `D0` (the
deleting destructor) calls `D1` and then `operator delete`. Every variant is
attributed to its one source function, and the body variant is the object code
analysed, so `Boosted`'s constructor is shown calling `Channel`'s. Before #166
the first variant by address was analysed instead: `Boosted`'s constructor
showed the `C1` thunk calling itself, and its call to the base
constructor was missing.

The destructor of the block-scoped `Channel local` is a call on the line of the
closing `}`. The null check `delete[] samples_` compiles to, in `~Channel`, is
an `extra-object-branch` gap: a conditional branch with no source decision.

The other variants are analysed too, as the function's `variants` (#266):
blocks, gaps and a projected status for each, in the Object tab under the
body. Here that is six symbols, 296 bytes. On ELF, `C1` and `D1` are aliases
of `C2` and `D2` (the same address), so they are the body and only the `D0`s
are variants there.

A variant has no coverage counter of its own: clang counts the body only. So
when the body ran, nothing says whether a given variant did, and butterfly
does not guess. Each variant's instructions are projected unknown, not
covered. Each is an `uncounted-variant` gap. None of its bytes are in the
image's covered-bytes figure. `Channel`'s `D0` never runs, because nothing
deletes a `Channel` through a pointer. It is reported, but it is not shown as
never run: from source coverage it looks the same as `Boosted`'s `D0`, which
does run. A trace is what tells the two apart. (Had the body never run, its
variants would be projected uncovered: every variant runs the body.) Each
`D0`'s call to `operator delete` is a `core-lib` helper call.

**`new` and `delete`.** The calls to `operator new`, `operator new[]` and
`operator delete[]` are `helper-call` gaps of class `core-lib`, the class
`std::` code gets. They are the C++ standard library's and need their own
verification credit. A project that replaces them has them in its covered
source set, so the call is then an ordinary one. `delete c`, with `c` a
`Channel *` holding a `Boosted`, is two gaps on its line: an
`extra-object-branch` for the null check the source does not write, and an
`indirect-branch` for the virtual call to the deleting destructor.

**Stack.** `main`, `lambdas` and `lifetimes` are unbounded, because each
reaches an indirect call: the virtual `gain()` and `delete`, and the call
through the lambda's function pointer.

**Uncovered.** `Channel::gain()` never runs, because the only `gain()` called
is `Boosted`'s override. It is reached through the vtable, so it is a test
obligation in the worklist and not a dead-code candidate.
