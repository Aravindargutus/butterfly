<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# The support matrix

`matrix.toml` is the one place butterfly says what it supports: one row per
language construct, tool feature, toolchain, architecture and object format.
The tables in the README and the showcase page are to be rendered from it
(#153, #154), so nothing is claimed there that is not a row here.

The rule it exists to enforce comes from #38: **no claim of support without a
worked example.** A table drifts the moment someone adds a feature, so the
rule is a test, not a convention.

## The three states

| state | means | a row must have |
|---|---|---|
| `supported` | butterfly analyses it and the output is meaningful | an `example` that CI builds, the `evidence` line showing the construct, and a `proof` test that reads that example |
| `caveats` | it works, but something is approximated, under-reported, not yet proven end to end, or proven only off hosted CI | an `example`, and the limit written out in `caveat` |
| `not-supported` | not claimed | a one-line `reason` |

"Not yet demonstrated" is `not-supported`. A code path nobody has run on an
example is not a claim; the row says so, and `tracking` names the issue whose
example would promote it.

## A row

```toml
[[row]]
id = "c.header-static-inline"        # unique; <group>.<construct>
area = "language"                    # language | feature | toolchain | architecture
group = "C"                          # the heading it is rendered under
name = "A header `static inline` compiled into several translation units"
state = "supported"
example = "c-header-inline"          # a directory under examples/
evidence = "examples/c-header-inline/inc/geom.h:15"   # path:line, inside the example
pattern = 'static inline int32_t geom_clamp'          # regex the line must still match
proof = "crates/ingest/tests/dup_static_symbols.rs::each_translation_units_copy_keeps_its_own_object_code"
caveat = ""                          # required when state = "caveats"
reason = ""                          # required when state = "not-supported"
tracking = [36]                      # issues where a known gap is tracked
```

`proof` is a file, optionally followed by `::` and a test: a Rust test
function, or the title of a `node:test` case or browser check. `evidence` and
`pattern` are required for `language` and `toolchain` rows and optional for
`feature` and `architecture` rows, which often have no single line to point
at; where they are given they are checked. Point them at the construct, not
at a comment about it: the check reads a window of lines, so a `///` line
that names the function passes just as well as the function, and goes on
passing once the function is gone.

## What the check refuses

`crates/ingest/tests/support_matrix.rs` fails, naming the row, when:

1. a `supported` or `caveats` row names an example directory that does not exist;
2. a `supported` row's example is not built by **both** `.github/workflows/ci.yml`
   and `scripts/ci-local.sh`. A `caveats` row may lean on an example CI does
   not build only if its caveat contains the words **"not built in CI"** — and
   once CI does build it, that phrase is itself a failure, so the caveat
   cannot outlive the gap. The two build lists must also agree with each
   other, with one declared exception — see *Local-only examples* below.
   "Built" means the build script is the **command** of a step that runs: a
   script named in a comment, echoed in a skip message, quoted inside the
   `grep` that filters the skip report, or sitting in a step turned off with
   `if: false` builds nothing, and none of those count. See *What the
   build-list reader can and cannot determine* below for how far that reading
   goes, and what it refuses to guess at;
3. the `evidence` file or line is gone, or `pattern` no longer matches within
   3 lines of it. The window lets a comment be added above a construct without
   failing the build; it does not let the construct be deleted. A `language`
   or `toolchain` row may not cite a `.md` file: a construct lives in code,
   and a sentence about one outlives it;
4. the `proof` file is gone or no longer contains the named test; a
   `supported` row's proof never names `examples/<example>` **in code** (a
   model test on hand-written evidence proves the model, not the example, and
   a comment that mentions the example is prose, not a read); or a `caveats`
   row has no proof and its caveat does not contain the words **"no test"**;
5. a directory under `examples/` — or a build system under
   `examples/build-systems/` — backs no row;
6. a `caveats` row has no caveat, a `not-supported` row has no reason, a
   `supported` row carries a caveat, an id is used twice, or a field is
   misspelt.

The check is textual. It cannot tell whether a test really asserts the
construct, only that the claim still points at something that exists; reading
each proof is the claims audit's job (#200). What it guarantees is narrower
and still worth having: a claim cannot silently outlive its evidence.

## What the build-list reader can and cannot determine

Rule 2 rests on one question asked of two shell scripts: **which build scripts
does CI actually run?** The check answers it by reading the text — it never
runs either script — so it is worth being exact about how much of a shell it
understands, because the answer can be wrong in both directions. A line
misread as a build invents a hosted build and then rejects a valid
`[[local_only]]` declaration; a real build missed leaves an example looking
unbuilt and blocks a `supported` row it had earned.

**It determines**, for `ci.yml`, which steps run (a step under `if: false`
does not, and a commented-out step is a comment) and what the `run:` script of
each is, whether a plain scalar or a `|` block — with the block's YAML
indentation taken off, as the runner takes it off before any shell sees the
script; for `ci-local.sh`, which lines are a `step "label" command`. It then
tokenizes that shell the way a shell reads words — single quotes, double
quotes, backslash escapes, `#` comments, `\`-continued lines — before deciding
where one command ends and the next begins at `;`, `&`, `|`, a newline or a
bracket. Only the **head** of each command counts, after stepping over leading
`VAR=value` assignments and `env`. Quoting a command does not stop it running,
so `"./examples/c/build.sh"` is a build; a separator inside an argument or
inside a trailing comment is text, so
`echo "local only; ./examples/x/build.sh"` is not (#207 — reading the raw line
and cutting it at every `;` said it was).

It also recognises a **here-document** — `<<WORD`, `<<-WORD`, and either with
the word quoted — and consumes its body as what it is: input the command
reads, not commands the shell runs. A `cat <<'NOTE'` whose body holds
`./examples/rust-qemu/build.sh` prints that path and builds nothing, and the
terminator is matched the way a shell matches it, alone on its line at column
zero (after tabs, for `<<-`). Before this, every newline ended a command even
inside a body, so such a block invented a hosted build and rejected the valid
`[[local_only]]` declaration resting on it (review of #207).

**It does not determine** anything that needs the script to run. It does not
expand variables, so a build reached through `$SCRIPT` is invisible to it; it
does not read the contents of a `$(...)` or backtick substitution, and it does
not follow a `source`d file, a function body or a loop. It cannot tell a build
guarded by `if [ -x ... ]` from one that always runs — "built" here means the
line is there, not that your last run reached it.

**What it refuses.** Where the two part company — a quote nobody closed, a
lone trailing `\`, a substitution whose contents name a `build.sh`, a `<<<`
here-string, a here-document whose delimiter never arrives, or a `$(...)`
inside the body of an *unquoted* here-document, which the shell would still
run — it does not pick the likelier reading. It fails, naming the file and
quoting the line, and says the build list is unknown — and a refusal is a
finding, so the check fails on it rather than reporting a build list it does
not stand behind. A `run:` block or a `step` line the tokenizer refuses
outright contributes no builds at all. That is the same rule the rest of this
checker follows: a thing the tool cannot determine is reported, never guessed.
If you write a line it refuses, either rewrite it plainly or teach the reader
the shape and pin it with a test; do not let it guess.

## Local-only examples

`scripts/ci-local.sh` exists to mirror hosted CI, and the check holds it to
that: the examples `ci.yml` builds must equal the examples `ci-local.sh` builds
**minus those declared local-only** at the top of `matrix.toml`:

```toml
[[local_only]]
example = "rust-qemu"
reason = "Needs QEMU, a nightly and a bare-metal target; hosted minutes are billed (#147)."
tracking = [147, 164]
```

This is for a fixture whose toolchain is too costly to install on every
hosted run (#164 allows exactly this), not a way to leave an example out.

The check reads script text, so what it establishes is that `ci-local.sh`
*would* run the build — not that your last run did. Build steps in that
script are guarded by what the host has (`if [ -z "$QEMU_SKIP" ]`, a real
`gcc`), and a host without the toolchain prints a named SKIP instead. So
"verified locally" means *on a host that has the toolchain*, and the caveats
say that; `ci-local.sh` is what makes it true, by failing if the example's
suite skips on a host where the fixture was built. The check fails when:

- an example is built by `ci-local.sh` and not by `ci.yml`, and is **not
  declared** — or is built by `ci.yml` and not by `ci-local.sh`, for which
  there is no exception at all;
- a declaration has no `reason`, names a directory that does not exist, names
  an example `ci-local.sh` does not build either (that is "not built in CI",
  a different caveat), or appears twice;
- a declared example **is** built by `ci.yml`: the declaration is out of date.
  Remove it, and then
- a row whose caveat says "verified locally, not in hosted CI" rests on an
  example that is not declared: the phrase is out of date too. Neither can
  outlive the gap, the same way "not built in CI" cannot;
- a row backed by a declared example is `supported`. It is `caveats` at best,
  and its caveat must contain the words **"verified locally, not in hosted
  CI"** — and must not say "not built in CI", which is untrue of it.

A local-only row is otherwise an ordinary `caveats` row: its evidence and its
proof are checked like any other, and the proof runs wherever the fixture has
been built.

## Adding or promoting a row

1. Add the construct to an example (or a new example, with a `build.sh`, and
   add that to `ci.yml` and `ci-local.sh`).
2. Write a test that ingests the example and asserts what butterfly reports
   for the construct.
3. Add the row, or change its `state`, and delete the caveat that no longer
   applies.
4. Run the check:

```sh
cargo test -p butterfly-ingest --test support_matrix
```

It needs no compiler and no built fixture — it reads sources, not build
outputs — so it runs, and never skips, on every host. The same file tests the
rules themselves against small in-memory trees, so each rule stays proven
without anyone having to break the real tree to see it fire.

## Why this is not under `docs/`

Since #147 a change that touches only `docs/**` or `*.md` skips CI. A matrix
under `docs/` could be edited into a false claim and the check guarding it
would never run. `support/matrix.toml` matches neither pattern, so every edit
to it is tested. (This README does match, and it describes the six rules
rather than enforcing them: `support_matrix.rs` is where each of them lives,
and it does not match either. A README that drifted from the rules would be
wrong about them, and could not make the matrix wrong.)
