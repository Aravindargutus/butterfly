// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests (area E): the helper programs the object-code
//! analyses run (`llvm-objdump` and the rest).
//!
//! README.md:1598-1615 "Every program butterfly runs on its own behalf ...
//! runs by the absolute path it resolved ..., a deadline and a ceiling on
//! each output stream. A helper past its ceiling is stopped at once ...
//! Defaults: `BUTTERFLY_HELPER_TIMEOUT_SECS=600`,
//! `BUTTERFLY_MAX_HELPER_OUTPUT_MB=4096` on stdout (1 MiB of stderr) ... and
//! `--version` probes to 64 KiB in 10 s."
//! docs/butterfly.html, Security posture: "every refusal names the
//! BUTTERFLY_MAX_... variable that would have let it through".

use butterfly_ingest::helper::{max_concurrent, run, run_any, Bounds, HelperError};
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

const SH: &str = "/bin/sh";

fn sh(script: &str) -> Vec<&OsStr> {
    vec![OsStr::new("-c"), OsStr::new(script)]
}

/// README.md:1605-1610: the documented defaults, when the operator set none.
#[test]
fn the_documented_bounds() {
    if std::env::var_os("BUTTERFLY_HELPER_TIMEOUT_SECS").is_none() && std::env::var_os("BUTTERFLY_MAX_HELPER_OUTPUT_MB").is_none() {
        let b = Bounds::standard();
        assert_eq!((b.deadline, b.stdout, b.stderr), (Duration::from_secs(600), 4096 * 1024 * 1024, 1024 * 1024));
    }
    let p = Bounds::probe();
    assert_eq!((p.deadline, p.stdout), (Duration::from_secs(10), 64 * 1024));
    assert!(max_concurrent() >= 1);
}

/// "a ceiling on each output stream": stderr past its own ceiling stops the
/// helper, and the error says which stream. Only stdout's ceiling has a
/// variable to raise, so only that message names one.
#[test]
fn stderr_past_its_ceiling_is_stopped_and_named() {
    let b = Bounds { deadline: Duration::from_secs(30), stdout: 1 << 20, stderr: 1024 };
    let e = run(Path::new(SH), &sh("head -c 4096 /dev/zero >&2; sleep 30"), None, b).unwrap_err();
    match &e {
        HelperError::TooMuchOutput { stream, cap, .. } => assert_eq!((*stream, *cap), ("stderr", 1024)),
        other => panic!("{other:?}"),
    }
    assert!(!e.to_string().contains("BUTTERFLY_MAX_HELPER_OUTPUT_MB"), "{e}");
    let e = run(Path::new(SH), &sh("head -c 4096 /dev/zero"), None, b.with_stdout(100)).unwrap_err();
    assert!(e.to_string().contains("stdout") && e.to_string().contains("BUTTERFLY_MAX_HELPER_OUTPUT_MB"), "{e}");
}

/// "one past its deadline likewise" -- and the message names the variable.
#[test]
fn the_deadline_message_names_its_variable() {
    let e = run(Path::new(SH), &sh("sleep 30"), None, Bounds::probe().with_deadline(Duration::from_millis(200))).unwrap_err();
    assert!(matches!(e, HelperError::TimedOut { .. }), "{e:?}");
    assert!(e.to_string().contains("BUTTERFLY_HELPER_TIMEOUT_SECS"), "{e}");
}

/// "runs by the absolute path it resolved": a relative name is refused with
/// a sentence saying why; an absolute path that does not exist is a spawn
/// failure naming it.
#[test]
fn identity_failures_say_what_was_refused() {
    let e = run(Path::new("llvm-objdump"), &[], None, Bounds::probe()).unwrap_err();
    assert!(matches!(e, HelperError::Unresolved(_)));
    assert!(e.to_string().contains("not an absolute path"), "{e}");
    let e = run(Path::new("/nonexistent/llvm-objdump"), &[], None, Bounds::probe()).unwrap_err();
    assert!(matches!(e, HelperError::Spawn { .. }), "{e:?}");
    assert!(e.to_string().contains("/nonexistent/llvm-objdump"), "{e}");
}

/// A helper that fails is a typed failure carrying its diagnostics;
/// `run_any` hands back the status and both streams instead.
#[test]
fn a_failing_helper_reports_its_stderr() {
    let e = run(Path::new(SH), &sh("echo boom >&2; exit 3"), None, Bounds::probe()).unwrap_err();
    assert!(e.to_string().contains("boom"), "{e}");
    let (status, out, err) = run_any(Path::new(SH), &sh("echo out; echo err >&2; exit 3"), None, Bounds::probe()).unwrap();
    assert_eq!((status.code(), out, err), (Some(3), b"out\n".to_vec(), b"err\n".to_vec()));
}

/// The input a helper is given reaches it.
#[test]
fn stdin_reaches_the_helper() {
    let out = run(Path::new(SH), &sh("cat"), Some(b"abc".to_vec()), Bounds::probe()).unwrap();
    assert_eq!(out, b"abc");
}
