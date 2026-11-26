# Requirements and tests for pull requests

For every PR, add or revise a testable statement in an area-specific TOML file
here. IDs start with `BF-`, followed by one or more uppercase alphanumeric area segments separated by hyphens and a final decimal number (for example, `BF-CLI-002`). Keep an existing ID when refining the same behavior. Add or update an
automated test for each statement, and list its repository-relative suite path.

```toml
[BF-AREA-001]
statement = "State the observable result and the conditions under which it applies."
tests = ["crates/cli/tests/example.rs"]
```

The required **Butterfly PR check** compares the full PR to its merge base,
including for documentation-only changes. Each new or revised statement needs
at least one linked suite with added, edited, or removed test code in the same PR.
Supported suites are Cargo workspace integration tests directly under
`crates/<member>/tests/*.rs`, `ui/*.test.cjs` Node tests, the browser checks
`ui/browser/checks/*.cjs` (register a new one in `ui/browser/run.cjs`, or it
does not run), and `scripts/test_pr_requirements.py` for the gate itself. For documentation,
test a concrete claim, example command, generated table, or link in the
relevant docs suite. IDs must be unique and all linked suites must exist in the prospective merge tree.

Changing only the `tests` list of a requirement counts as revising it: one of
the suites it now names needs a change in the same PR. A PR that removes a
behavior may delete its requirement instead of revising one; a suite the
deleted requirement named must then be deleted or changed in that PR.

Reviewers check that the assertions exercise the requirement and its failure
cases. The checker recognizes file and line changes. A no-op can pass, and edits confined to comment lines (`//` lines, `/* ... */` blocks and, in Python, `#` lines; this includes Rust doc examples) do not count. Review the actual assertions. Run the policy locally
with Git 2.38 or later and Python 3.11 or later after committing your changes:

```sh
python3 scripts/test_pr_requirements.py
python3 scripts/check_pr_requirements.py origin/main HEAD
```

If the branch conflicts with its base, the checker cannot build the tree that
would be merged and says so. Merge or rebase onto the base first.
