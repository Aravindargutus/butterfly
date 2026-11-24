// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! One place that decides what a missing fixture means (#227).
//!
//! Many suites here return early when the thing they test against is not on
//! the machine:
//!
//! ```ignore
//! let Some(d) = scratch("note") else { return };
//! ```
//!
//! `cargo test` counts that as a pass. In a worktree with no built examples,
//! `crates/cli/tests/check_gate.rs` reports `13 passed` having executed one
//! assertion; the only thing distinguishing it from a real run is wall-clock
//! time, which is not in the summary line anyone reads. It has misled people
//! in both directions on the same day: once as "my fix is pinned, 13 passed",
//! once as "the test is worthless, it passes without the fix".
//!
//! Two different things were being called "skip", and separating them is most
//! of the fix:
//!
//!   * [`fixture_missing`] -- something that is *supposed* to be here: a
//!     built example, a tool CI installs. Absent, this is a skip on a
//!     developer's unbuilt worktree and a **failure** anywhere the fixtures
//!     are required. A suite must not be able to report success in the
//!     environment where it is meant to be authoritative.
//!   * [`not_applicable`] -- something this machine cannot provide at all:
//!     the wrong operating system, a privilege that changes what is under
//!     test, a toolchain nothing here installs. Requiring it would be
//!     requiring a different machine, so it is always a skip.
//!
//! Both print the same `skipping: ...` line the suites have always printed,
//! byte for byte, so the greps in `.github/workflows/ci.yml` and
//! `scripts/ci-local.sh` keep behaving exactly as they do today. This adds a
//! guard; it does not move one.
//!
//! # Fixtures are required when
//!
//!   * `BUTTERFLY_REQUIRE_FIXTURES` says so, or
//!   * `CI` is set and `BUTTERFLY_REQUIRE_FIXTURES` does not say otherwise.
//!
//! `CI=true` is set by GitHub Actions for every step, and by every other CI
//! system worth naming. `BUTTERFLY_REQUIRE_FIXTURES=1` is how a developer --
//! or an agent about to report a green run as evidence -- gets the honest
//! answer on their own worktree; `BUTTERFLY_REQUIRE_FIXTURES=0` is the way
//! out for a machine that sets `CI` but genuinely cannot build the fixtures.

/// A fixture this suite needs is not here.
///
/// `what` is the remedy, not the symptom: it is printed after `skipping: `
/// exactly as the suites already phrase it ("run examples/c/build.sh first"),
/// and it is the first thing in the panic.
///
/// Returns so the caller can go on to `return` or `return None` as it
/// always has -- when this returns at all.
#[track_caller]
pub fn fixture_missing(what: &str) {
    if fixtures_required() {
        panic!(
            "this suite needs a fixture that is not here: {what}\n\
             \n\
             It would otherwise return without asserting anything and be counted as a pass, \
             which is what #227 is about -- so where the fixtures are required it fails instead. \
             Build the fixture, or set BUTTERFLY_REQUIRE_FIXTURES=0 to go back to skipping here \
             (and then a green run means less than it says)."
        );
    }
    eprintln!("{}", line(what));
}

/// This machine cannot provide what the test needs, and no amount of
/// building would change that: the wrong operating system, running as root,
/// a toolchain neither CI nor `scripts/ci-local.sh` installs.
///
/// Always a skip. Never a failure, in CI or anywhere else.
pub fn not_applicable(why: &str) {
    eprintln!("{}", line(why));
}

/// A fresh, private scratch directory for one test (#344).
///
/// The suites used to build `temp_dir()/butterfly-<tag>-<pid>`, remove
/// whatever was there and `create_dir_all` it. On a shared machine anyone can
/// pre-plant that predictable name as a symlink, and the test then writes --
/// or `remove_dir_all`s -- through it. This creates the directory under a
/// random name with an exclusive `mkdir` (mode 0700 on Unix), so nothing can
/// be there first. `tag` is only a readable prefix.
///
/// The directory is not removed on drop: the suites already clean up the
/// ones they mean to, and a failing test's leftovers are worth keeping.
pub fn scratch_dir(tag: &str) -> std::path::PathBuf {
    tempfile::Builder::new().prefix(&format!("butterfly-{tag}-")).tempdir().expect("create a scratch directory in the system temp dir").keep()
}

/// A fresh, empty scratch file for one test, under a random name ending in
/// `suffix` -- the file counterpart of [`scratch_dir`], for the same reason.
/// Created exclusively, so nothing can be at that name first; the caller
/// writes over it and removes it.
pub fn scratch_file(tag: &str, suffix: &str) -> std::path::PathBuf {
    let (_, path) = tempfile::Builder::new()
        .prefix(&format!("butterfly-{tag}-"))
        .suffix(suffix)
        .tempfile()
        .expect("create a scratch file in the system temp dir")
        .keep()
        .expect("keep the scratch file");
    path
}

/// What both of them print. Byte for byte what the suites printed before
/// this module existed, because `ci.yml` and `scripts/ci-local.sh` grep for
/// it and two of their exclusions match on the text after the colon.
fn line(what: &str) -> String {
    format!("skipping: {what}")
}

/// Are fixtures required in this environment?
fn fixtures_required() -> bool {
    required_from(std::env::var("BUTTERFLY_REQUIRE_FIXTURES").ok().as_deref(), std::env::var("CI").ok().as_deref())
}

/// The decision, as a function of the two variables and nothing else, so it
/// can be tested without an environment.
///
/// An explicit `BUTTERFLY_REQUIRE_FIXTURES` always wins: it is the only way
/// to *lower* the bar inside CI, and the only way to raise it outside.
pub fn required_from(explicit: Option<&str>, ci: Option<&str>) -> bool {
    match explicit {
        Some(v) => truthy(v),
        // GitHub Actions sets CI=true; some runners set it to "1" or just to
        // an empty value, which is not a claim that this is CI.
        None => ci.map(truthy).unwrap_or(false),
    }
}

/// `""`, `0`, `false`, `no`, `off` are false; anything else present is true.
fn truthy(v: &str) -> bool {
    !matches!(v.trim().to_ascii_lowercase().as_str(), "" | "0" | "false" | "no" | "off")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: `CI` turns a skip into a failure, and an explicit
    /// setting overrides it in both directions.
    #[test]
    fn ci_requires_fixtures_and_an_explicit_setting_overrides_it() {
        assert!(!required_from(None, None), "a developer's worktree skips, as it always did");
        assert!(required_from(None, Some("true")), "CI=true requires them");
        assert!(required_from(None, Some("1")), "and so does CI=1");

        // an unset-but-present CI is not a claim that this is CI
        assert!(!required_from(None, Some("")), "CI= is not CI");
        assert!(!required_from(None, Some("false")), "nor is CI=false");

        // the explicit variable wins, both ways
        assert!(required_from(Some("1"), None), "a developer can ask for the honest answer");
        assert!(!required_from(Some("0"), Some("true")), "and a CI that cannot build fixtures can opt out");
        assert!(!required_from(Some("false"), Some("1")), "by name as well as by number");
    }

    #[test]
    fn what_counts_as_set() {
        for t in ["1", "true", "TRUE", "yes", "on", "anything"] {
            assert!(truthy(t), "{t}");
        }
        for f in ["", " ", "0", "false", "False", "no", "off"] {
            assert!(!truthy(f), "{f:?}");
        }
    }

    /// The printed line is not ours to restyle: `ci.yml` and
    /// `scripts/ci-local.sh` grep for `skipping:` and exclude two of them by
    /// the text that follows it.
    #[test]
    fn the_line_is_the_one_the_greps_already_look_for() {
        assert_eq!(line("run examples/c/build.sh first"), "skipping: run examples/c/build.sh first");
        for excused in ["butterfly build examples/c/butterfly-build.toml", "examples/rust-qemu/build.sh"] {
            assert!(line(&format!("run `{excused}` first")).contains(excused), "the exclusions in ci.yml still match: {excused}");
        }
    }

    /// The property, end to end and through a real test harness: *the same
    /// test* is a reported skip on a developer's worktree and a failing test
    /// where the fixtures are required.
    ///
    /// It runs [`a_suite_whose_fixture_is_absent`] below in a child process,
    /// twice, because that is the only way to observe what the harness does
    /// with a panic -- and because setting an environment variable in-process
    /// would race every other test in this binary.
    #[test]
    fn a_missing_fixture_is_a_skip_here_and_a_failure_where_fixtures_are_required() {
        use std::process::Command;
        let exe = std::env::current_exe().expect("this test binary");
        let subject = "tests::a_suite_whose_fixture_is_absent";
        let run = |require: &str| {
            Command::new(&exe)
                .args(["--exact", subject, "--ignored", "--nocapture", "--test-threads", "1"])
                .env("BUTTERFLY_REQUIRE_FIXTURES", require)
                .output()
                .expect("re-run this test binary")
        };

        let required = run("1");
        let said = String::from_utf8_lossy(&required.stdout).into_owned() + &String::from_utf8_lossy(&required.stderr);
        assert!(!required.status.success(), "required: the suite must fail, not pass: {said}");
        assert!(said.contains("needs a fixture that is not here"), "and say so in those words: {said}");
        assert!(said.contains("0 passed; 1 failed"), "the summary line is the whole point: {said}");

        let skipped = run("0");
        let said = String::from_utf8_lossy(&skipped.stdout).into_owned() + &String::from_utf8_lossy(&skipped.stderr);
        assert!(skipped.status.success(), "not required: the same test still skips: {said}");
        assert!(said.contains("skipping: a fixture this test invented"), "printing the line it always printed: {said}");
        assert!(said.contains("1 passed"), "{said}");
    }

    /// The subject of the test above, and nothing else: on its own it does
    /// exactly what every fixture-dependent suite here does.
    ///
    /// `#[ignore]` so an ordinary run never executes it -- it would print a
    /// `skipping:` line of its own, and the whole point of the greps in
    /// `ci.yml` and `scripts/ci-local.sh` is that such a line is a failure.
    /// The test above runs it on purpose, with `--ignored`.
    #[test]
    #[ignore = "the test above runs this in a child process, twice"]
    fn a_suite_whose_fixture_is_absent() {
        fixture_missing("a fixture this test invented, so it is never there");
    }
}
