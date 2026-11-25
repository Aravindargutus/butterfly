<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 Dhakshnamoorthy Durgaraj -->

# Contributing to Butterfly

## Requirements and verification

For each PR, record the behavioral requirement under `docs/requirements/`,
link the automated tests that exercise it, and report the test results.
See the [versioning and release rules](docs/qualification/versioning.md) when a
feature changes a public interface or the evidence format.

## Adding a new feature

### 1. Add a support-matrix entry

Every claim of support starts as a row in `support/matrix.toml`. See
[support/README.md](support/README.md) for the schema and the three states
(`supported`, `caveats`, `not-supported`). The rule: **no claim without a
worked example.** The CI test `every_claim_in_the_support_matrix_still_has_its_evidence`
enforces this — you cannot add a matrix row without backing it.

```toml
[[row]]
id = "feature.my-feature"
area = "feature"
group = "Features"
name = "Short description of the feature"
state = "caveats"
example = "my-example"
proof = "crates/ingest/tests/my_test.rs::the_feature_works"
caveat = "Explain the limitation."
```

### 2. Write a worked example

Create a minimal project under `examples/` that exercises the feature:

```
examples/my-example/
├── butterfly.toml          # project config
├── build.sh                # builds the example
├── src/                    # source code
└── README.md               # what the example demonstrates
```

The example should be small enough to build in CI and large enough to prove
the feature works. Look at existing examples (e.g. `examples/c`,
`examples/rust-lib`) for the shape.

### 3. Implement the code

The five crates and where features land:

| crate | what goes here |
|---|---|
| `crates/ingest` | Coverage parsing, object-code reading, DWARF, evidence production |
| `crates/model` | Data types shared between ingest and view |
| `crates/view` | The web viewer, report generation, HTTP server |
| `crates/cli` | The `butterfly` CLI binary |
| `crates/testsupport` | Shared test fixtures and helpers |

### 4. Write tests

Tests assert on the worked example's actual output:

- **Rust tests** in `crates/ingest/tests/` or `crates/view/tests/` (and `crates/cli/tests/` for CLI behavior) — prove
  the evidence comes out right on the example.
- **Viewer unit tests** in `ui/*.test.cjs` — if the feature touches the
  viewer's JavaScript.
- **Browser checks** in `ui/browser/checks/` — if the feature adds or
  changes a viewer tab or view.

### 5. Wire it into CI

- Add the example build to `scripts/ci-local.sh` and
  `.github/workflows/ci.yml` so the matrix check sees it.
- Run `scripts/ci-local.sh` locally before pushing — it mirrors every CI job.

## Running tests locally

```sh
cargo test                        # all Rust tests
node --test ui/*.test.cjs         # viewer unit tests
scripts/ci-local.sh               # full CI mirror (builds examples, runs everything)
```

## Branching and PRs

- Branch from `main`, target PRs to `main`.
- One feature per PR. If the matrix entry, example, code and tests are ready,
  bundle them in one PR — splitting them means the matrix check fails on each
  partial PR.
- Run `scripts/ci-local.sh` before requesting review.

## License

By contributing you agree that your contributions are licensed under MIT OR
Apache-2.0, matching the project.
