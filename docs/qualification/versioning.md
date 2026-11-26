<!--
SPDX-License-Identifier: MIT OR Apache-2.0
Copyright (c) 2026 Dhakshnamoorthy Durgaraj
-->

# Versioning and release rules

Status: normative for this repository. Written for #120, part of the DO-330
work (#112).

## Why this file and not `CONTRIBUTING.md`

When #120 was written, the project had no contributor guide. Its only
documented contributor policy concerned versioning, so a standalone guide
would have left questions about branches, testing and review unanswered.
The qualification directory was chosen to keep the normative versioning
rules alongside their assessment.

These rules also exist for a specific reason: DO-330 §7.1.a and §7.2.7.d
require configuration items to be identified unambiguously, and the project's
internal DO-330 assessment (not published in this repository; findings 4 and
T-8 objectives 1–2) records their absence as the gap this closes. They belong
beside the qualification records that demanded them. The
[contributor guide](../../CONTRIBUTING.md) now links here so there is one
authoritative version of these rules.

## What is versioned

Three things carry versions, and they move independently.

**1. The tool version** — `version` in the workspace `[workspace.package]`
table of `Cargo.toml`. All four crates (`model`, `ingest`, `view`, `cli`)
inherit it, so there is one number for the whole product. It is what
`butterfly --version` prints (clap derives it from `CARGO_PKG_VERSION`,
`crates/cli/src/main.rs:13`) and what `ingest` stamps into every evidence file
as `provenance.toolVersion` (`crates/ingest/src/lib.rs:39` —
`TOOL_VERSION: &str = env!("CARGO_PKG_VERSION")`). This document's rules are
about this number.

**2. The on-disk schema identifiers** — a set of `butterfly-<kind>/<N>`
strings in `crates/model`, one per file format the tool writes:

| Constant | Value today | Where |
| --- | --- | --- |
| `SCHEMA` (evidence) | `butterfly-evidence/1` | `crates/model/src/lib.rs:24` |
| `CHANGES_SCHEMA` | `butterfly-changes/1` | `crates/model/src/lib.rs:69` |
| `BASELINE_SCHEMA` | `butterfly-baseline/2` | `crates/model/src/lib.rs:371` |
| `RESOLUTIONS_SCHEMA` | `butterfly-resolutions/1` | `crates/model/src/lib.rs:1331` |
| `REQTEXT_SCHEMA` | `butterfly-requirements/1` | `crates/model/src/reqtext.rs:39` |
| `TAGS_SCHEMA` | `butterfly-tags/1` | `crates/model/src/tags.rs:22` |

Each `<N>` is a single integer, not a semantic version, and it is compared for
exact equality where it is checked — `Baseline::check_schema`
(`crates/model/src/lib.rs:282`) and the changes cache
(`crates/ingest/src/change.rs:1069`) both reject anything that is not the
string this build writes. `butterfly-baseline` is already at `/2`: #18 added an
object-code fingerprint that a `/1` baseline does not carry, so comparing
against one would have silently under-reported change, and the integer moved
rather than the reader guessing.

**3. Dependency and toolchain pins** — `Cargo.lock` and
`rust-toolchain.toml`. Not versions of butterfly, but part of the
configuration set a release identifies.

## Tool version: what bumps what

Semantic versioning, with "the public interface" read as *everything a user or
a downstream file depends on*, not only the Rust API. Butterfly is not
published to crates.io, so the Rust API matters far less than the CLI, the
evidence file and the exit codes.

**MAJOR** (`X.0.0`) — a user has to change something to keep working:

- A schema integer is raised (see below), i.e. files written by the previous
  version are rejected rather than read.
- A CLI subcommand, flag or positional argument is removed or its meaning
  changes.
- An exit code changes meaning. `butterfly check`'s codes are a build gate;
  moving one silently turns a red build green.
- A default changes such that the same command on the same inputs produces a
  materially different answer.

**MINOR** (`0.Y.0`) — new capability, or a schema change, with existing usage
intact:

- A new subcommand, a new flag, a new output.
- **Any change to the evidence schema — the serde types in `crates/model` that
  `Evidence` reaches — is at least a minor bump.** This is the rule #120 calls
  out, and it is deliberately wider than "the schema integer moved", because
  most evidence changes do *not* move the integer. The type is
  additive-tolerant by construction: fields carry `#[serde(default)]` and
  `#[serde(skip_serializing_if)]`, so a new field is readable by an older
  viewer and the string stays `butterfly-evidence/1`. That tolerance is what
  makes the change easy to under-report. The evidence file is the tool's
  qualification-relevant output — every number the viewer shows is derived
  from it (`crates/model/src/lib.rs:4-11`) — so a build that writes a field
  the previous build did not is not the same tool, and `provenance.toolVersion`
  must be able to say so. Adding a field, changing a field's type, changing
  what a field means, changing what is written into it: minor at least.
- The same rule applies to the other five schemas, for the same reason.
- A new caveat, gap kind or status value that appears in evidence. These are
  strings a reader may switch on.

**PATCH** (`0.0.Z`) — nothing a caller can observe except that a wrong answer
became right:

- A bug fix whose only effect is that a previously incorrect number, symbol
  name, attribution or diagnostic is now correct.
- Performance work with identical output.
- Dependency bumps that change no behaviour.
- Documentation, examples, tests, CI, the showcase page.

A prose-only change need not bump anything; it rides with the next release.

### Where the schema rule and the version rule meet

Two independent decisions, in this order:

1. **Does the schema integer move?** Only if a file written by the previous
   version can no longer be read correctly by this one, or vice versa —
   removed field, changed meaning of an existing field, changed invariant that
   a reader relies on. If yes, raise the integer *and* take a **major** bump,
   because the old file is now rejected.
2. **Did the schema change at all?** If yes but the integer stayed, take a
   **minor** bump.

So: integer moved ⇒ major. Schema touched, integer still ⇒ minor. Schema
untouched ⇒ decide on the CLI/behaviour rules above.

### Pre-1.0

Butterfly is below `1.0.0` and the strict SemVer reading of `0.y.z` ("anything
may break") is not the one used here — a tool whose output is meant as
certification evidence cannot have a version scheme that promises nothing. The
rules above are applied as written: a breaking change takes the major
position, so the first one moves the tool to `1.0.0`. `0.x` means "not yet
qualified and not yet feature-complete", not "unstable on purpose".

## Recording the version

`provenance.toolVersion` alone is not enough to trace evidence to a source
baseline — the DO-330 assessment says so directly (finding under T-8 objective
2, and "Identification and release" in §Gaps): every commit shares one version
string, so two evidence files produced weeks apart by materially different
builds both read `0.1.0`. The fix the assessment names is to additionally
record the git commit and a dirty-tree flag in provenance. That is #117:
`crates/ingest/build.rs` captures `git rev-parse HEAD` and `git status
--porcelain` when the crate is built, and `ingest` writes them as
`provenance.toolCommit` (the full id, or `unknown` when the build was not
from a checkout of butterfly and `BUTTERFLY_BUILD_COMMIT` was not set) and
`provenance.toolDirty` (`true`, `false`, or `null` when it could not be
established). `butterfly --version` prints the same, and `release.yml`
refuses to publish a binary whose own answer is not the tagged commit on a
clean tree. A released tag is still what makes `toolVersion` mean a reviewed
tree rather than some build of butterfly; the commit is what makes evidence
from any build, released or not, traceable to its sources.

## Releasing

1. Decide the number by the rules above.
2. Move `version` in `Cargo.toml` (`[workspace.package]`), and build so
   `Cargo.lock` is updated by cargo rather than by hand.
3. Add the entry to `CHANGELOG.md`, grouped by area, describing what shipped
   and nothing else.
4. Merge.
5. Tag `vX.Y.Z` on the merge commit and push the tag. That triggers
   `.github/workflows/release.yml`, which builds `--locked --release` on
   `ubuntu-24.04`, computes the binary's SHA-256, and attaches both to a
   GitHub Release.

The tag is the release. Nothing else creates one, and a release is never
rebuilt from a different tree: the workflow builds the tagged commit with
`--locked`, so the dependency set is the one in the lockfile at that tag.

## What this does not cover

- **The Tool Configuration Index** (DO-330 T-8 objective 2). A release makes
  one writable; writing it is separate work under #112.
- **Publishing to crates.io.** Not done, not planned here. If it ever is, the
  Rust API becomes part of the public interface and the major-bump rule widens
  accordingly.
- **Supported-version / backport policy.** There has been no release, so there
  is nothing to support. Decide it when a second release exists.
