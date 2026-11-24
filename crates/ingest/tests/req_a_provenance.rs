// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for how butterfly identifies the helper programs
//! it records in provenance (area A).
//!
//! README.md:1600-1615 "**Helper programs run under one policy.** Every
//! program butterfly runs on its own behalf ... runs by the absolute path it
//! resolved (and records), with the environment cleared to a short
//! allow-list (... no `GIT_DIR`, no token), a deadline and a ceiling on each
//! output stream ... `--version` probes to 64 KiB in 10 s."
//!
//! One test sets process environment variables, so this binary keeps every
//! test that runs a helper serial behind one lock.
#![cfg(unix)]

use butterfly_ingest::provenance::{identify, resolve_on_path};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static ENV: Mutex<()> = Mutex::new(());

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("req-a-prov-{tag}"))
}

fn stub(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// README.md:1604-1606 "with the environment cleared to a short allow-list
/// (`PATH`, `HOME`, the locale, ...; no `GIT_DIR`, no token)" -- a version
/// probe cannot see the operator's token or `GIT_DIR`.
#[test]
fn a_version_probe_does_not_inherit_the_token_or_git_dir() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let d = scratch("env");
    let tool = stub(&d, "fake-tool", "echo \"fake-tool 1.2.3 token=[$BUTTERFLY_TOKEN] gitdir=[$GIT_DIR]\"");
    std::env::set_var("BUTTERFLY_TOKEN", "sekrit-token-value");
    std::env::set_var("GIT_DIR", "/hostile/git/dir");
    let (t, caveat) = identify("fake-tool", &tool);
    std::env::remove_var("BUTTERFLY_TOKEN");
    std::env::remove_var("GIT_DIR");
    assert!(caveat.is_none(), "the version was read: {caveat:?}");
    assert!(t.version.contains("1.2.3"), "{t:?}");
    assert!(!t.version.contains("sekrit-token-value"), "the token reached the helper: {}", t.version);
    assert!(!t.version.contains("/hostile/git/dir"), "GIT_DIR reached the helper: {}", t.version);
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1611-1613 "`--version` probes to 64 KiB" and "A helper past its
/// ceiling is stopped at once" -- what a probe can put in the provenance is
/// bounded.
#[test]
fn a_version_probe_is_held_to_its_output_ceiling() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let d = scratch("ceiling");
    // 256 KiB on one line, with the version digits at the front.
    let tool = stub(&d, "chatty-tool", "head -c 262144 /dev/zero | tr '\\0' 'x' | sed 's/^/chatty 9.9 /'");
    let (t, _) = identify("chatty-tool", &tool);
    assert!(t.version.len() <= 64 * 1024, "the recorded version is {} bytes", t.version.len());
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1604 "runs by the absolute path it resolved (and records)" and
/// docs/butterfly.html:808 caveats rather than silent guesses -- a tool
/// whose version cannot be read is recorded at its path with a caveat.
#[test]
fn a_tool_is_recorded_at_its_absolute_path_and_an_unreadable_version_is_a_caveat() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let sh = resolve_on_path("sh").expect("sh is on PATH");
    assert!(sh.is_absolute(), "{sh:?}");
    assert!(resolve_on_path("butterfly-req-a-no-such-tool").is_none());

    let d = scratch("caveat");
    let tool = stub(&d, "broken-tool", "exit 3");
    let (t, caveat) = identify("broken-tool", &tool);
    assert_eq!(Path::new(&t.path), tool.as_path(), "recorded at the path it was run from");
    let c = caveat.unwrap_or_else(|| panic!("an unreadable version must be a caveat: {t:?}"));
    assert!(c.contains("broken-tool"), "naming the tool: {c}");
    let _ = std::fs::remove_dir_all(d);
}
