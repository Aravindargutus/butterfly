// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! One variant, one sidecar.
//!
//! A multi-variant session derives each variant's resolutions file from its
//! name. That derivation was not injective, so two variants could be handed
//! the same file and overwrite each other's recorded review decisions -- and
//! a hostile evidence file could aim at another variant's sidecar on purpose
//! (#6).

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Evidence for the C example, ingested here if it is not already there.
///
/// `examples/c/build/evidence.json` is written by CI's *documented workflow*
/// step, which runs after the tests -- so depending on it means skipping, and
/// `cargo test` captures `eprintln!` from a passing test, so the "no suite
/// was skipped" gate cannot see it. Build what is needed instead: `build.sh`
/// has produced the coverage export by now, and ingesting it is one call.
fn evidence() -> Option<PathBuf> {
    static ONCE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let shared = root.join("examples/c/build/evidence.json");
        if shared.exists() {
            return Some(shared.canonicalize().unwrap());
        }
        if !root.join("examples/c/build/coverage.json").exists() {
            butterfly_testsupport::fixture_missing("run ./examples/c/build.sh first");
            return None;
        }
        let out = butterfly_testsupport::scratch_dir("variants-ev").join("evidence.json");
        let o = Command::new(env!("CARGO_BIN_EXE_butterfly"))
            .args(["ingest", root.join("examples/c/build").to_str().unwrap(), "--source-root", root.join("examples/c").to_str().unwrap(), "-o", out.to_str().unwrap()])
            .output()
            .expect("spawn butterfly ingest");
        assert!(o.status.success(), "ingesting the example: {}", String::from_utf8_lossy(&o.stderr));
        Some(out)
    })
    .clone()
}

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("variants-{tag}"))
}

/// A copy of the example evidence whose `variant` build-info key is `name`.
fn variant_at(dir: &Path, file: &str, name: &str) -> PathBuf {
    let src = evidence().unwrap();
    let mut ev: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(src).unwrap()).unwrap();
    ev["provenance"]["buildInfo"]["variant"] = serde_json::Value::String(name.into());
    let p = dir.join(file);
    std::fs::write(&p, serde_json::to_string(&ev).unwrap()).unwrap();
    p
}

/// What one `butterfly serve` did at startup.
struct Run {
    /// `Some` when it exited on its own -- every refusal here does.
    exited: Option<std::process::ExitStatus>,
    stderr: String,
}

impl Run {
    fn refused(&self) -> bool {
        self.exited.is_some_and(|s| !s.success())
    }
}

/// Kills and reaps whatever it holds, on every path out.
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Start `serve` and return once it has decided: either it refused and
/// exited, or it came up and is listening. Killed either way -- these cases
/// are about what it works out before serving anything.
fn serve(dir: &Path, evidences: &[PathBuf]) -> Run {
    let log = common::log_path("variants");
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.arg("serve");
    for e in evidences {
        c.arg(e);
    }
    // Port 0, and "came up" is the server's own URL line rather than a connect
    // to a port picked here. A port picked by binding and releasing a socket
    // can still be held by another test's child: the server then refuses to
    // start, on `Address already in use` rather than anything these cases are
    // about, and the connect succeeds anyway (#91, #99; see `common`).
    c.args(["--listen", "127.0.0.1:0", "--resolutions", dir.join("resolutions.json").to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::from(std::fs::File::create(&log).expect("create server log")));
    let mut child = Child(c.spawn().expect("spawn butterfly serve"));
    let exited = match common::wait_for_listener(&mut child.0, &log, Duration::from_secs(30)) {
        Some(common::Started::Exited(st)) => Some(st),
        // it came up, or had not decided in time; `Child` stops it either way
        _ => None,
    };
    drop(child); // killed and reaped, so its stderr is complete
    let stderr = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    Run { exited, stderr }
}

/// Two evidence files claiming the same variant would share one sidecar.
#[test]
fn two_evidence_files_with_the_same_variant_name_are_refused() {
    let Some(_) = evidence() else { return };
    let d = scratch("dupe-name");
    let a = variant_at(&d, "evidence-a.json", "O0");
    let b = variant_at(&d, "evidence-b.json", "O0");

    let o = serve(&d, &[a, b]);
    assert!(o.refused(), "a shared sidecar must not be served: {}", o.stderr);
    let e = o.stderr;
    assert!(e.contains("`O0`"), "the error names the variant: {e}");
    assert!(e.contains("evidence-a.json") && e.contains("evidence-b.json"), "and both files: {e}");
    assert!(e.to_lowercase().contains("overwrite"), "and says what would happen: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// The pair from the issue: distinct names that used to sanitise to one file.
#[test]
fn names_that_used_to_share_a_sidecar_now_serve_side_by_side() {
    let Some(_) = evidence() else { return };
    let d = scratch("collide");
    let a = variant_at(&d, "evidence-a.json", "a/b");
    let b = variant_at(&d, "evidence-b.json", "a_b");

    // no refusal: the two now have distinct sidecars
    let e = serve(&d, &[a, b]).stderr;
    assert!(!e.contains("overwrite"), "these no longer collide: {e}");
    assert!(e.contains("variant `a/b`") && e.contains("variant `a_b`"), "both loaded: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// Nothing may be read from a sidecar before the collision check has run --
/// otherwise the check runs after the damage.
#[test]
fn a_collision_is_refused_before_any_sidecar_is_read() {
    let Some(_) = evidence() else { return };
    let d = scratch("before-read");
    let a = variant_at(&d, "evidence-a.json", "O0");
    let b = variant_at(&d, "evidence-b.json", "O0");

    // a sidecar that would be a hard error if it were ever opened
    let side = butterfly_view::sidecar::path_for_variant(&d.join("resolutions.json"), "O0");
    std::fs::write(&side, "this is not a resolutions file").unwrap();

    let o = serve(&d, &[a, b]);
    assert!(o.refused(), "{}", o.stderr);
    let e = o.stderr;
    assert!(e.to_lowercase().contains("overwrite"), "the collision is what stopped it: {e}");
    assert!(!e.contains("not a resolutions file"), "the sidecar was never opened: {e}");
    let _ = std::fs::remove_dir_all(d);
}

/// A single-variant session is unchanged: it uses the path it was given.
#[test]
fn one_evidence_file_uses_the_resolutions_path_verbatim() {
    let Some(ev) = evidence() else { return };
    let d = scratch("single");
    let side = d.join("resolutions.json");
    std::fs::write(&side, r#"{"schema":"butterfly-resolutions/1","entries":[]}"#).unwrap();

    let e = serve(&d, &[ev]).stderr;
    assert!(!e.contains("overwrite"), "{e}");
    assert!(side.exists(), "the file it was pointed at is still the one it uses");
    let _ = std::fs::remove_dir_all(d);
}

/// Decisions recorded before the name carried a digest are in a different
/// file. Coming up anyway would show no reviews for that variant and write a
/// second sidecar beside the first -- the misleading empty state the whole
/// check exists to prevent, and a warning on stderr does not stop it because
/// a launcher or service manager swallows stderr (review of #6).
#[test]
fn a_sidecar_written_under_the_old_name_stops_startup() {
    let Some(_) = evidence() else { return };
    let d = scratch("legacy");
    let a = variant_at(&d, "evidence-a.json", "O0");
    let b = variant_at(&d, "evidence-b.json", "O1");
    let legacy = butterfly_view::sidecar::legacy_path_for_variant(&d.join("resolutions.json"), "O0");
    std::fs::write(&legacy, r#"{"schema":"butterfly-resolutions/1","entries":[]}"#).unwrap();

    let o = serve(&d, &[a.clone(), b.clone()]);
    assert!(o.refused(), "it must not serve a variant whose decisions it cannot see: {}", o.stderr);
    let e = &o.stderr;
    assert!(e.contains(&legacy.file_name().unwrap().to_string_lossy().into_owned()), "it names the old file: {e}");
    assert!(e.contains("mv "), "and gives the command that fixes it: {e}");
    assert!(e.contains("variant `O0`"), "and which variant: {e}");

    // once moved, it starts and picks the decisions up
    let want = butterfly_view::sidecar::path_for_variant(&d.join("resolutions.json"), "O0");
    std::fs::rename(&legacy, &want).unwrap();
    let o = serve(&d, &[a, b]);
    assert!(!o.refused(), "after the move it serves: {}", o.stderr);
    assert!(o.stderr.contains("variant `O0`") && o.stderr.contains("variant `O1`"), "{}", o.stderr);
    let _ = std::fs::remove_dir_all(d);
}

/// The refusal happens before any sidecar is read, like the collision check.
#[test]
fn the_legacy_refusal_comes_before_any_sidecar_is_opened() {
    let Some(_) = evidence() else { return };
    let d = scratch("legacy-before-read");
    let a = variant_at(&d, "evidence-a.json", "O0");
    let b = variant_at(&d, "evidence-b.json", "O1");
    std::fs::write(butterfly_view::sidecar::legacy_path_for_variant(&d.join("resolutions.json"), "O0"), "{}").unwrap();
    // O1's sidecar would be a hard error if it were ever opened
    std::fs::write(butterfly_view::sidecar::path_for_variant(&d.join("resolutions.json"), "O1"), "this is not a resolutions file").unwrap();

    let o = serve(&d, &[a, b]);
    assert!(o.refused(), "{}", o.stderr);
    assert!(o.stderr.contains("older sidecar name"), "the legacy file is what stopped it: {}", o.stderr);
    assert!(!o.stderr.contains("not a resolutions file"), "and nothing was opened: {}", o.stderr);
    let _ = std::fs::remove_dir_all(d);
}
