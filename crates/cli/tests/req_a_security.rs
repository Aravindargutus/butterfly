// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests of `butterfly serve`'s security posture over real
//! HTTP (area A). Expected values come from README.md "Security posture"
//! (README.md:1488-1634), docs/butterfly.html "Security posture" and
//! SECURITY.md "What the viewer exposes".
//!
//! Each test serves a small evidence file of its own, so none needs a fixture.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

mod common;
use common::Server;

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("req-a-sec-{tag}"))
}

fn evidence(dir: &Path) -> PathBuf {
    let ev = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
    let p = dir.join("evidence.json");
    std::fs::write(&p, serde_json::to_string(&ev).unwrap()).unwrap();
    p
}

fn start(dir: &Path, args: &[&str]) -> Server {
    let ev = evidence(dir);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    cmd.env_remove("BUTTERFLY_TOKEN").stdin(Stdio::null());
    cmd.arg("serve").arg(&ev).arg("--resolutions").arg(dir.join("resolutions.json")).args(args);
    common::serve(&mut cmd)
}

/// One raw request; returns the status, the header block (lower-cased) and
/// the body. The shared helper always sends `Host: 127.0.0.1` and drops the
/// headers, and both are what these tests are about.
fn raw(port: u16, method: &str, target: &str, host: &str, extra: &[(&str, &str)], body: &str) -> (u16, String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut req = format!("{method} {target} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if method != "GET" {
        req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let code = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (code, head.to_ascii_lowercase(), body.to_string())
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| l.strip_prefix(&format!("{name}:")).map(str::trim))
}

/// README.md:1578-1581 "**Content-Security-Policy** (`default-src 'none';
/// script-src 'self'; …`) on the viewer and on the generated report, plus
/// `nosniff` and `no-referrer`."
#[test]
fn the_viewer_page_carries_the_documented_security_headers() {
    let d = scratch("csp-viewer");
    let srv = start(&d, &[]);
    let (code, head, _) = raw(srv.port, "GET", "/", "127.0.0.1", &[], "");
    assert_eq!(code, 200, "{head}");
    let csp = header(&head, "content-security-policy").unwrap_or_else(|| panic!("no CSP on the viewer:\n{head}"));
    assert!(csp.contains("default-src 'none'") && csp.contains("script-src 'self'"), "{csp}");
    assert_eq!(header(&head, "x-content-type-options"), Some("nosniff"), "{head}");
    assert_eq!(header(&head, "referrer-policy"), Some("no-referrer"), "{head}");
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1578-1581, the same sentence, for the report the server
/// generates: CSP, `nosniff` and `no-referrer`.
#[test]
fn the_served_report_carries_the_documented_security_headers() {
    let d = scratch("csp-report");
    let srv = start(&d, &[]);
    let (code, head, body) = raw(srv.port, "GET", "/api/report", "127.0.0.1", &[], "");
    assert_eq!(code, 200, "{head}\n{body}");
    let csp = header(&head, "content-security-policy").unwrap_or_else(|| panic!("no CSP on the report:\n{head}"));
    assert!(csp.contains("default-src 'none'"), "{csp}");
    assert_eq!(header(&head, "x-content-type-options"), Some("nosniff"), "{head}");
    assert_eq!(header(&head, "referrer-policy"), Some("no-referrer"), "{head}");
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1567-1571 "**Host-header allow-list.** Requests are answered only
/// for `localhost`, `127.0.0.1`, `::1` (and the address you bound) ...
/// Cross-origin reads are additionally blocked by the absence of CORS
/// headers."
#[test]
fn only_loopback_host_names_are_answered_and_no_cors_is_offered() {
    let d = scratch("host");
    let srv = start(&d, &[]);
    let port = srv.port;
    for host in ["127.0.0.1".to_string(), format!("127.0.0.1:{port}"), format!("localhost:{port}"), "[::1]".to_string(), format!("[::1]:{port}")] {
        let (code, head, _) = raw(port, "GET", "/api/variants", &host, &[], "");
        assert_eq!(code, 200, "Host {host} is a loopback name:\n{head}");
    }
    for host in ["evil.example", "127.0.0.1.evil.example", "localhost.evil.example:8787"] {
        let (code, _, body) = raw(port, "GET", "/api/evidence", host, &[], "");
        assert!(!(200..300).contains(&code), "Host {host} must be refused, got {code}");
        assert!(!body.contains("\"schema\""), "and nothing of the evidence is in the refusal: {body}");
    }
    let (code, head, _) = raw(port, "GET", "/api/evidence", "127.0.0.1", &[("Origin", "http://evil.example")], "");
    assert_eq!(code, 200);
    assert!(header(&head, "access-control-allow-origin").is_none(), "no CORS header: {head}");
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1517-1518 "`--read-only` refuses writes on any address, token
/// or not" -- loopback included, and nothing is written.
#[test]
fn read_only_refuses_a_write_on_loopback_and_writes_nothing() {
    let d = scratch("ro");
    let srv = start(&d, &["--read-only"]);
    let body = r#"{"key":"k","category":"justified","justification":"j","author":"a","date":"2026-01-01"}"#;
    let (code, _, _) = raw(srv.port, "POST", "/api/resolutions", "127.0.0.1", &[], body);
    assert!(code == 403 || code == 405, "a write is refused, got {code}");
    assert!(!d.join("resolutions.json").exists(), "and nothing was written");
    let (code, _, _) = raw(srv.port, "GET", "/api/variants", "127.0.0.1", &[], "");
    assert_eq!(code, 200, "reads still work");
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1511-1513 "A URL with `?token=` is refused with a 400: by the
/// time the server sees it, the token has already crossed the network in
/// the request line" -- on any route, wherever the parameter sits in the
/// query, and on loopback too (the token crossed the wire all the same).
#[test]
fn a_token_in_the_query_is_refused_on_every_route_and_position() {
    let d = scratch("qtoken");
    let srv = start(&d, &[]);
    for target in ["/?token=abc", "/api/variants?token=abc", "/api/evidence?variant=x&token=abc", "/app.js?token=abc", "/api/requirements?ids=A&token="] {
        let (code, _, body) = raw(srv.port, "GET", target, "127.0.0.1", &[], "");
        assert_eq!(code, 400, "{target}: {body}");
    }
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}

/// README.md:1596-1599 "Every refusal names the variable that would have let
/// it through ... A `BUTTERFLY_UI_DIR` override is read under the auxiliary
/// cap too." An override file past the cap is refused -- and the refusal,
/// like every other, says which variable would have let it through, rather
/// than the server quietly serving something else in its place.
#[test]
fn an_oversized_ui_override_is_refused_naming_the_aux_variable() {
    let d = scratch("uidir");
    let ui = d.join("ui");
    std::fs::create_dir_all(&ui).unwrap();
    let marker = "/* the operator's override */";
    let mut big = String::from(marker);
    big.push_str(&"x".repeat(1 << 20));
    std::fs::write(ui.join("app.js"), &big).unwrap();
    let ev = evidence(&d);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    cmd.env_remove("BUTTERFLY_TOKEN").env("BUTTERFLY_UI_DIR", &ui).env("BUTTERFLY_MAX_AUX_MB", "1").stdin(Stdio::null());
    cmd.arg("serve").arg(&ev).arg("--resolutions").arg(d.join("resolutions.json"));
    let srv = common::serve(&mut cmd);
    let (_, _, body) = raw(srv.port, "GET", "/app.js", "127.0.0.1", &[], "");
    assert!(!body.starts_with(marker), "the oversized override was served whole");
    let said = format!("{body}\n{}", srv.stderr());
    assert!(said.contains("BUTTERFLY_MAX_AUX_MB"), "the refusal names the variable, in the response or on stderr:\n{}", &said[..said.len().min(2000)]);
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}
