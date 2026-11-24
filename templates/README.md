# Build template

`build.sh` is a starting point for a project that wants butterfly evidence.
Copy it to your project root, edit the block at the top, run it.

```sh
cp templates/build.sh myproject/
cd myproject && $EDITOR build.sh     # set PROJECT, SRC, INCLUDE
./build.sh                           # clang
CC=gcc ./build.sh                    # GCC
butterfly doctor build               # says what, if anything, is missing
butterfly ingest build -o evidence.json
```

You own this file. butterfly reads what it produces and never runs it for you
— see [SECURITY.md](../SECURITY.md) for why.

## What it produces, and why each piece matters

| In `build/` | Why butterfly needs it |
|---|---|
| `coverage.json` | which lines, branches and conditions the tests reached |
| `<project>` | the **non-instrumented** binary — the object code the evidence is actually about |
| `*.su` | stack usage per function (DO-178C §6.3.4.f) |
| `build-info.txt` | compiler and flags, so the build can be reproduced (§11.2) |
| `*.dSYM` (macOS) | line table; without it every instruction lands in "no source line" |

## Two builds, on purpose

The script compiles your sources twice. Instrumented code is not the code you
ship, so coverage is measured on one binary and projected onto another that was
built clean. Mixing the two is the mistake this layout exists to prevent.

## Toolchain differences you should expect

| | clang | GCC |
|---|---|---|
| MC/DC | yes, `-fcoverage-mcdc` | **no** — gcov has no MC/DC |
| Conditions | yes | GCC 14+, `-fcondition-coverage`, and it is *not* MC/DC |
| Branch polarity | true/false named | gcov does not say which outcome is true; butterfly reports them as "one outcome"/"the other" rather than guessing |

If you need MC/DC — DO-178C level A, Table A-7 objective 5 — you need clang.
The script warns on stderr when the compiler cannot provide it.

## Other build systems

The same artefacts are what matter, however they are produced. `butterfly
doctor <dir>` checks a directory built by Make, CMake, Bazel or anything else
and names what is missing.
