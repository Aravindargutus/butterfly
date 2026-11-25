# Requirements and tests for pull requests

For every PR, add or revise a testable statement in an area-specific TOML file
here. Keep an existing ID when refining the same behavior. Add or update an
automated test for each statement, and list its repository-relative suite path.

```toml
[BF-AREA-001]
statement = "State the observable result and the conditions under which it applies."
tests = ["crates/cli/tests/example.rs"]
```

The required **Butterfly PR check** compares the full PR to its merge base,
including for documentation-only changes. Each new or revised statement needs
at least one linked suite with added or updated test code in the same PR.
Supported suites are Cargo workspace integration tests directly under
`crates/<member>/tests/*.rs`, `ui/*.test.cjs` Node tests, and
`scripts/test_pr_requirements.py` for the gate itself. For documentation,
test a concrete claim, example command, generated table, or link in the
relevant docs suite. All linked suites must continue to exist.

Reviewers check that the assertions exercise the requirement and its failure
cases. A matching path alone cannot establish that. Run the policy locally
with Python 3.11 or later after committing your changes:

```sh
python3 scripts/test_pr_requirements.py
python3 scripts/check_pr_requirements.py origin/main HEAD
```
