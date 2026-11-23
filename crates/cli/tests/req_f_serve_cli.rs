// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for `butterfly serve` as a command (area F): the
//! parts that live in the binary's flags, its stderr and its environment.
//! The HTTP routes themselves are in crates/view/tests/req_f_server.rs.
//!
//! Every expected value comes from README.md, SECURITY.md or
//! docs/butterfly.html; each test names the requirement it checks.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MIB: usize = 1 << 20;

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("req-f-cli-{tag}"))
}

/// The smallest evidence `serve` loads, so these tests need no fixture.
fn evidence(dir: &Path) -> PathBuf {
    let ev = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
    let p = dir.join("evidence.json");
    std::fs::write(&p, serde_json::to_string(&ev).unwrap()).unwrap();
    p
}

/// The binary, with no token or cap inherited from whoever runs the tests.
fn butterfly() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.env_remove("BUTTERFLY_TOKEN").env_remove("BUTTERFLY_UI_DIR").env_remove("BUTTERFLY_MAX_AUX_MB").stdin(Stdio::null());
    c
}

/// Run a command that has to refuse, and return its stderr. Still running
/// after 20 s means it is serving, which is the failure.
fn refused(mut cmd: Command) -> String {
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn().expect("spawn butterfly");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(st) = child.try_wait().unwrap() {
            let mut err = String::new();
            std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
            assert!(!st.success(), "exited 0 where it had to refuse:\n{err}");
            return err;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("it did not refuse: still running after 20 s, so it was serving");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// REQ-F-DEFAULT-LISTEN: "`serve <evidence…>` | the browser UI on
/// `127.0.0.1:8787`." (README.md:121; docs/butterfly.html:432, 444)
/// Checked from `--help` rather than by binding the fixed port.
#[test]
fn req_f_serve_listens_on_loopback_8787_by_default() {
    let out = butterfly().args(["serve", "--help"]).output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(help.contains("[default: 127.0.0.1:8787]"), "the default listener is not 127.0.0.1:8787:\n{help}");
    // `build --serve` shares the same flags (README.md:1428)
    let help = String::from_utf8_lossy(&butterfly().args(["build", "--help"]).output().unwrap().stdout).into_owned();
    assert!(help.contains("[default: 127.0.0.1:8787]") && help.contains("--serve"), "{help}");
}

/// REQ-F-BANNER: "Startup prints the exposure and the authentication mode
/// every time." (README.md:1521-1522; docs/butterfly.html:838) -- including
/// the default, open loopback mode.
#[test]
fn req_f_startup_states_exposure_and_authentication_on_plain_loopback() {
    let d = scratch("banner");
    let ev = evidence(&d);
    let srv = common::serve(butterfly().arg("serve").arg(&ev).arg("--resolutions").arg(d.join("r.json")));
    let err = srv.stderr();
    let listening = err.lines().find(|l| l.contains("listening on")).unwrap_or_else(|| panic!("no exposure line:\n{err}"));
    assert!(listening.contains("loopback"), "the exposure is named: {listening}");
    let reads = err.lines().find(|l| l.trim_start_matches("butterfly:").trim_start().starts_with("reads")).unwrap_or_else(|| panic!("no read-mode line:\n{err}"));
    let writes = err.lines().find(|l| l.trim_start_matches("butterfly:").trim_start().starts_with("writes")).unwrap_or_else(|| panic!("no write-mode line:\n{err}"));
    assert!(reads.contains("open"), "{reads}");
    assert!(writes.contains("no authentication") || writes.contains("allowed"), "{writes}");
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-UI-DIR: "`BUTTERFLY_UI_DIR=ui butterfly serve …` serves the UI from
/// disk for live editing." (README.md:1474; docs/butterfly.html:856) and "A
/// `BUTTERFLY_UI_DIR` override is read under the auxiliary cap too."
/// (README.md:1596-1597)
#[test]
fn req_f_the_ui_dir_override_is_served_and_capped() {
    let d = scratch("ui-dir");
    let ev = evidence(&d);
    let ui = d.join("ui");
    std::fs::create_dir_all(&ui).unwrap();
    std::fs::write(ui.join("index.html"), "<!doctype html><title>REQ-F-OVERRIDE</title>").unwrap();
    // 2 MiB against a 1 MiB cap
    std::fs::write(ui.join("app.css"), format!("/* REQ-F-BIG */{}", " ".repeat(2 * MIB))).unwrap();
    let mut srv = common::serve(butterfly().arg("serve").arg(&ev).arg("--resolutions").arg(d.join("r.json")).env("BUTTERFLY_UI_DIR", &ui).env("BUTTERFLY_MAX_AUX_MB", "1"));
    let (code, body) = common::get(&mut srv, "/");
    assert_eq!(code, 200);
    assert!(body.contains("REQ-F-OVERRIDE"), "the page on disk was not served: {}", &body[..body.len().min(200)]);
    let (_, css) = common::get(&mut srv, "/app.css");
    assert!(!css.contains("REQ-F-BIG"), "a {}-byte override was served past a 1 MiB auxiliary cap", css.len());
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-RES-CAP: "... as are the evidence and the resolutions sidecar `serve`
/// loads. A crafted or truncated input fails with a sentence naming the file
/// ... Every refusal names the variable that would have let it through."
/// (README.md:1579-1589; docs/butterfly.html:830) -- the sidecar is on the
/// auxiliary cap.
#[test]
fn req_f_an_oversized_resolutions_sidecar_is_refused_by_name() {
    let d = scratch("res-cap");
    let ev = evidence(&d);
    let side = d.join("resolutions.json");
    let pad = " ".repeat(2 * MIB);
    std::fs::write(&side, format!(r#"{{"schema":"{}","entries":[]{pad}}}"#, butterfly_model::RESOLUTIONS_SCHEMA)).unwrap();
    let mut c = butterfly();
    c.arg("serve").arg(&ev).arg("--resolutions").arg(&side).args(["--listen", "127.0.0.1:0"]).env("BUTTERFLY_MAX_AUX_MB", "1");
    let err = refused(c);
    assert!(err.contains("BUTTERFLY_MAX_AUX_MB"), "names the variable: {err}");
    assert!(err.contains("resolutions.json"), "names the file: {err}");
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-SYMLINK-SAID: "A symlink at the destination is displaced rather than
/// followed, and butterfly says so on stderr." (README.md:1569-1570;
/// docs/butterfly.html:836)
#[cfg(unix)]
#[test]
fn req_f_a_displaced_sidecar_link_is_reported_on_stderr() {
    let Some(src) = c_evidence() else { return };
    let d = scratch("link-said");
    let ev = d.join("evidence.json");
    std::fs::copy(&src, &ev).unwrap();
    let victim = d.join("victim.json");
    let empty = format!(r#"{{"schema":"{}","entries":[]}}"#, butterfly_model::RESOLUTIONS_SCHEMA);
    std::fs::write(&victim, &empty).unwrap();
    let link = d.join("resolutions.json");
    std::os::unix::fs::symlink(&victim, &link).unwrap();
    let mut srv = common::serve(butterfly().arg("serve").arg(&ev).arg("--resolutions").arg(&link));
    let review = r#"{"key":"fn:0","category":"a","justification":"j","author":"a","date":"2026-01-01"}"#;
    let (code, body) = common::request(&mut srv, "POST", "/api/resolutions", &[("Content-Type", "application/json")], review);
    assert_eq!(code, 200, "{body}");
    let err = srv.stderr();
    assert!(err.contains("symlink") && err.contains("victim.json"), "the displacement is not reported on stderr:\n{err}");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), empty, "and the target is untouched");
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}

/// REQ-F-UNAUTH-403: with `--allow-unauthenticated-remote`, "resolution
/// writes refused with a 403 explaining how to enable them."
/// (README.md:1511-1513; docs/butterfly.html:838)
#[test]
fn req_f_an_open_network_listener_says_how_to_enable_writes() {
    let d = scratch("open-remote");
    let ev = evidence(&d);
    let mut srv = common::serve_on(
        butterfly().arg("serve").arg(&ev).arg("--resolutions").arg(d.join("r.json")).args(["--allow-remote", "--insecure-http", "--allow-unauthenticated-remote"]),
        "0.0.0.0:0",
    );
    let (code, body) = common::request(
        &mut srv,
        "POST",
        "/api/resolutions",
        &[("Content-Type", "application/json")],
        r#"{"key":"x","category":"a","justification":"j","author":"a","date":"2026-01-01"}"#,
    );
    assert_eq!(code, 403, "{body}");
    assert!(body.contains("--token-file") || body.contains("BUTTERFLY_TOKEN") || body.contains("--token"), "the 403 does not say how to enable writes: {body}");
    // reads need no token here, the evidence included (README.md:1511-1512)
    assert_eq!(common::get(&mut srv, "/api/evidence").0, 200);
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}

fn c_evidence() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build/evidence.json");
    if p.exists() {
        Some(p)
    } else {
        butterfly_testsupport::fixture_missing("build examples/c and ingest it to examples/c/build/evidence.json first");
        None
    }
}
