// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for the viewer's HTTP surface (area F).
//!
//! Every expected value here comes from the documentation -- README.md
//! "Security posture", "The gates in the viewer", "Exploratory builds and
//! variant comparison", "Layout", "Reading it in the viewer", SECURITY.md and
//! docs/butterfly.html -- never from what the implementation happens to do.
//! Each test names the requirement it checks.
//!
//! The server runs **in this process** (`butterfly_view::serve` on a tokio
//! runtime of the test's own), not as a `butterfly serve` child. A child is
//! stopped with SIGKILL, which never writes its coverage profile, so the
//! HTTP suites in crates/cli exercise these routes without the coverage
//! report ever seeing them. Here the routes are counted.
//!
//! Nothing here touches the process environment: those cases
//! (`BUTTERFLY_UI_DIR`, the `BUTTERFLY_MAX_*` caps) are in
//! crates/cli/tests/req_f_serve_cli.rs, where each server has its own.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use butterfly_view::access::Token;
use butterfly_view::ServeOptions;

const TOKEN: &str = "req-f-a-token-of-at-least-thirty-two-chars";

// ---------------------------------------------------------------- fixtures

fn scratch(tag: &str) -> PathBuf {
    // a fresh, unpredictable directory, not a name another user could
    // plant a symlink at (#344)
    tempfile::Builder::new().prefix(&format!("butterfly-req-f-{tag}-")).tempdir().unwrap().keep()
}

/// The smallest evidence `serve` loads: no files, no functions.
fn empty_evidence(dir: &Path, name: &str) -> PathBuf {
    let ev = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
    let p = dir.join(name);
    std::fs::write(&p, serde_json::to_string(&ev).unwrap()).unwrap();
    p
}

/// examples/c's evidence, copied into `dir` as `name`, optionally renamed to
/// another variant through its build-info. `None` (after saying so) when the
/// fixture has not been built.
fn c_evidence(dir: &Path, name: &str, variant: Option<&str>) -> Option<PathBuf> {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build/evidence.json");
    if !src.exists() {
        butterfly_testsupport::fixture_missing("build examples/c and ingest it to examples/c/build/evidence.json first");
        return None;
    }
    let text = std::fs::read_to_string(&src).unwrap();
    let out = dir.join(name);
    match variant {
        None => std::fs::write(&out, text).unwrap(),
        Some(v) => {
            let mut j: serde_json::Value = serde_json::from_str(&text).unwrap();
            j["provenance"]["buildInfo"]["variant"] = serde_json::json!(v);
            std::fs::write(&out, serde_json::to_string(&j).unwrap()).unwrap();
        }
    }
    Some(out)
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn opts(token: Option<&str>, read_only: bool, root: &Path) -> ServeOptions {
    ServeOptions {
        listen: String::new(),
        allow_remote: false,
        insecure_http: false,
        read_only,
        token: token.map(|t| Token::supplied(t.to_string())),
        allow_unauthenticated_remote: false,
        source_root: Some(root.to_path_buf()),
        repo: None,
    }
}

// ------------------------------------------------------------------ server

/// A viewer running on a runtime owned by this test; shut down on drop.
struct Srv {
    rt: Option<tokio::runtime::Runtime>,
    port: u16,
}

impl Drop for Srv {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

/// Start `serve` on a loopback port and return once it has answered.
///
/// The port is picked by binding and releasing a socket. This test binary
/// spawns no children, so the macOS descriptor inheritance described in
/// crates/cli/tests/common does not apply; and a bind that loses a race
/// anyway ends the task with an error, which is retried on another port.
fn start(evidence: Vec<PathBuf>, resolutions: PathBuf, mut o: ServeOptions) -> Srv {
    let bearer = o.token.as_ref().map(|t| format!("Bearer {}", t.as_str()));
    for _attempt in 0..8 {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        o.listen = format!("127.0.0.1:{port}");
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let task = rt.spawn(butterfly_view::serve(evidence.clone(), resolutions.clone(), o.clone()));
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut up = false;
        while Instant::now() < deadline {
            if task.is_finished() {
                break;
            }
            let h: Vec<(&str, &str)> = bearer.as_deref().map(|b| ("Authorization", b)).into_iter().collect();
            if let Ok((200, _, body)) = try_request(port, "GET", "/api/variants", &h, "") {
                // ours, not some other listener that took the port
                if evidence.iter().all(|e| body.contains(&*e.file_name().unwrap().to_string_lossy())) && !task.is_finished() {
                    up = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if up {
            return Srv { rt: Some(rt), port };
        }
        if task.is_finished() {
            let r = rt.block_on(task).unwrap();
            let e = r.expect_err("serve returned Ok without serving");
            if e.to_string().contains("in use") {
                continue;
            }
            panic!("serve failed to start: {e:#}");
        }
        panic!("serve never answered on port {port}");
    }
    panic!("no free port after 8 attempts");
}

/// What `serve` says when it refuses to start: it must return, not serve.
fn refuse(evidence: Vec<PathBuf>, resolutions: PathBuf, o: ServeOptions) -> String {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let mut o = o;
    o.listen = "127.0.0.1:0".into();
    let r = rt.block_on(async { tokio::time::timeout(Duration::from_secs(20), butterfly_view::serve(evidence, resolutions, o)).await });
    match r {
        Err(_) => panic!("serve did not refuse: it was still serving after 20 s"),
        Ok(Ok(())) => panic!("serve returned Ok where it had to refuse"),
        Ok(Err(e)) => format!("{e:#}"),
    }
}

type Headers = Vec<(String, String)>;

fn try_request(port: u16, method: &str, target: &str, headers: &[(&str, &str)], body: &str) -> std::io::Result<(u16, Headers, String)> {
    try_request_host(port, "127.0.0.1", method, target, headers, body)
}

fn try_request_host(port: u16, host: &str, method: &str, target: &str, headers: &[(&str, &str)], body: &str) -> std::io::Result<(u16, Headers, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut req = format!("{method} {target} HTTP/1.1\r\n");
    if !host.is_empty() {
        req.push_str(&format!("Host: {host}\r\n"));
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() || method != "GET" {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("Connection: close\r\n\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| std::io::Error::other("headers never terminated"))?;
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok()).ok_or_else(|| std::io::Error::other("no status"))?;
    let hdrs: Headers = lines.filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))).collect();
    let mut body = raw[end + 4..].to_vec();
    if hdrs.iter().any(|(k, v)| k == "transfer-encoding" && v.eq_ignore_ascii_case("chunked")) {
        let mut out = Vec::new();
        let mut rest = &body[..];
        loop {
            let le = rest.windows(2).position(|w| w == b"\r\n").ok_or_else(|| std::io::Error::other("bad chunk"))?;
            let n = usize::from_str_radix(String::from_utf8_lossy(&rest[..le]).split(';').next().unwrap().trim(), 16).map_err(std::io::Error::other)?;
            rest = &rest[le + 2..];
            if n == 0 {
                break;
            }
            out.extend_from_slice(&rest[..n]);
            rest = &rest[n + 2..];
        }
        body = out;
    }
    Ok((status, hdrs, String::from_utf8_lossy(&body).into_owned()))
}

impl Srv {
    fn req(&self, method: &str, target: &str, headers: &[(&str, &str)], body: &str) -> (u16, Headers, String) {
        try_request(self.port, method, target, headers, body).unwrap_or_else(|e| panic!("{method} {target}: {e}"))
    }
    fn get(&self, target: &str) -> (u16, String) {
        let (s, _, b) = self.req("GET", target, &[], "");
        (s, b)
    }
    fn get_auth(&self, target: &str, auth: &str) -> (u16, String) {
        let (s, _, b) = self.req("GET", target, &[("Authorization", auth)], "");
        (s, b)
    }
    fn post_json(&self, target: &str, json: &str, auth: Option<&str>) -> (u16, String) {
        let mut h = vec![("Content-Type", "application/json")];
        if let Some(a) = auth {
            h.push(("Authorization", a));
        }
        let (s, _, b) = self.req("POST", target, &h, json);
        (s, b)
    }
}

fn header<'a>(h: &'a Headers, name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// A well-formed review of `fn:0`, which examples/c has (it has functions).
fn review(justification: &str) -> String {
    format!(r#"{{"key":"fn:0","category":"a","justification":"{justification}","author":"req-f","date":"2026-01-01"}}"#)
}

/// The key of the first object-code gap in an evidence file.
fn first_gap_key(ev: &Path) -> String {
    let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(ev).unwrap()).unwrap();
    j["functions"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|f| f["object"]["gaps"].as_array().and_then(|g| g.first()).map(|g| g["key"].as_str().unwrap().to_string()))
        .expect("examples/c has gaps")
}

/// Every read route the API has, as README "Security posture" requires the
/// token on each of them (README.md:1513-1514).
const READ_ROUTES: [&str; 8] = ["/api/variants", "/api/report", "/api/evidence", "/api/freshness", "/api/tags", "/api/requirements?ids=X", "/api/gates", "/api/resolutions"];

// ===================================================== host allow-list / CORS

/// REQ-F-HOST: "Requests are answered only for `localhost`, `127.0.0.1`,
/// `::1` and the address you bound ... Without this, any web page the user
/// visits could reach the port by DNS rebinding and read the evidence."
/// (README.md:1698-1707; docs/butterfly.html:844; SECURITY.md:21-22)
#[test]
fn req_f_host_allow_list_answers_loopback_names_and_refuses_others() {
    let d = scratch("host");
    let ev = empty_evidence(&d, "evidence.json");
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));
    let port = srv.port;

    for host in [format!("127.0.0.1:{port}"), format!("localhost:{port}"), "localhost".into(), format!("[::1]:{port}"), "127.0.0.1".into()] {
        let (code, _, body) = try_request_host(port, &host, "GET", "/api/variants", &[], "").unwrap();
        assert_eq!(code, 200, "Host {host:?} is on the allow-list and must be answered: {body}");
    }
    for host in ["evil.example", "evil.example:8787", "127.0.0.1.evil.example", "localhost.evil.example", "attacker.test:1234"] {
        for target in ["/api/variants", "/api/evidence", "/"] {
            let (code, _, body) = try_request_host(port, host, "GET", target, &[], "").unwrap();
            assert!((400..500).contains(&code), "Host {host:?} on {target} must be refused, got {code}: {body}");
            assert!(!body.contains("\"variants\"") && !body.contains("\"schema\""), "a refused host was handed data: {body}");
        }
        // and a write under a foreign Host changes nothing
        let (code, _, _) = try_request_host(port, host, "POST", "/api/resolutions/delete", &[("Content-Type", "application/json")], r#"{"key":"fn:0"}"#).unwrap();
        assert!((400..500).contains(&code), "a write under Host {host:?} must be refused, got {code}");
    }
    assert!(!d.join("resolutions.json").exists(), "a refused write created the sidecar");
}

/// REQ-F-HOST-WILDCARD: README "Security posture", host allow-list
/// (README.md:1698-1707), as decided in #325 (ambiguity 10). "The address
/// you bound" is an address a client can reach; on a wildcard listener (`--listen 0.0.0.0` or `[::]`)
/// that is the address a request actually arrived on, never the wildcard
/// itself, and a request with no `Host` is refused like a foreign one. The
/// loopback names keep working.
///
/// The policy is checked directly: a test must not open a port to the
/// network. The same refusal over a real socket is
/// `wildcard_host_policy_over_http` in crates/view/src/lib.rs, on a loopback
/// port.
#[test]
fn req_f_a_wildcard_listener_does_not_admit_the_wildcard_or_a_missing_host() {
    use butterfly_view::HostPolicy;
    use std::net::IpAddr;
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    for (bare, bound) in [("0.0.0.0", ip("0.0.0.0")), ("::", ip("::"))] {
        let p = HostPolicy::for_bind(bare, bound);
        let lan = Some(ip("192.0.2.7"));
        for good in ["127.0.0.1", "127.0.0.1:8787", "localhost", "LocalHost:8787", "[::1]:8787", "::1"] {
            assert!(p.admits(Some(good), lan), "{bare}: a loopback name must still be answered: {good}");
        }
        for wild in ["0.0.0.0", "0.0.0.0:8787", "[::]", "[::]:8787", "::", "[0:0:0:0:0:0:0:0]:8787"] {
            assert!(!p.admits(Some(wild), lan), "{bare}: the wildcard is not an address: {wild}");
            assert!(!p.admits(Some(wild), Some(bound)), "{bare}: not even if the socket reports it: {wild}");
        }
        assert!(!p.admits(None, lan), "{bare}: a request with no Host is refused on a wildcard listener");
        assert!(!p.admits(None, None), "{bare}: with no Host and no known arrival address either");
        // the address the request arrived on is the address that was bound
        assert!(p.admits(Some("192.0.2.7:8787"), lan), "{bare}: the arrival address is answered");
        assert!(p.admits(Some("192.0.2.7"), lan));
        for other in ["192.0.2.8:8787", "evil.example", "evil.example:8787", "192.0.2.7.evil.example"] {
            assert!(!p.admits(Some(other), lan), "{bare}: {other} is not the arrival address");
        }
        assert!(!p.admits(Some("192.0.2.7"), None), "{bare}: with no arrival address there is nothing to match");
    }
    // a dual-stack `[::]` socket reports an IPv4 client's arrival address
    // IPv4-mapped; the client wrote the IPv4 literal
    let p = HostPolicy::for_bind("::", ip("::"));
    assert!(p.admits(Some("192.0.2.7:8787"), Some(ip("::ffff:192.0.2.7"))));
}

/// REQ-F-HOST-BOUND: the same rule off the wildcard. A specific address is
/// answered as itself, and the wildcard literal is not answered there either
/// -- `--allow-remote` used to add `0.0.0.0` whatever was bound (#325).
/// Loopback, where nothing changes, still answers a request with no `Host`:
/// no page can send one.
#[test]
fn req_f_a_specific_listener_answers_its_own_address_and_not_the_wildcard() {
    use butterfly_view::HostPolicy;
    use std::net::IpAddr;
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    let p = HostPolicy::for_bind("192.0.2.7", ip("192.0.2.7"));
    assert!(p.admits(Some("192.0.2.7:8787"), Some(ip("192.0.2.7"))));
    assert!(p.admits(Some("localhost"), Some(ip("192.0.2.7"))));
    assert!(!p.admits(Some("0.0.0.0:8787"), Some(ip("192.0.2.7"))));
    assert!(!p.admits(Some("[::]:8787"), Some(ip("192.0.2.7"))));
    let p = HostPolicy::for_bind("127.0.0.1", ip("127.0.0.1"));
    assert!(p.admits(None, Some(ip("127.0.0.1"))), "loopback: unchanged");
    assert!(!p.admits(Some("0.0.0.0"), Some(ip("127.0.0.1"))));
}

/// REQ-F-CORS: "Cross-origin reads are additionally blocked by the absence of
/// CORS headers." (README.md:1574; SECURITY.md:26 "There is no CORS layer")
#[test]
fn req_f_no_cors_headers_even_when_a_foreign_origin_asks() {
    let d = scratch("cors");
    let ev = empty_evidence(&d, "evidence.json");
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));
    for (method, target) in [("GET", "/api/evidence"), ("GET", "/api/variants"), ("OPTIONS", "/api/resolutions"), ("GET", "/")] {
        let (_, h, _) = srv.req(method, target, &[("Origin", "http://evil.example"), ("Access-Control-Request-Method", "POST")], "");
        let cors: Vec<_> = h.iter().filter(|(k, _)| k.starts_with("access-control-")).collect();
        assert!(cors.is_empty(), "{method} {target} answered a foreign Origin with CORS headers: {cors:?}");
    }
}

// ============================================================ headers / CSP

/// REQ-F-CSP-VIEWER: "Content-Security-Policy (`default-src 'none';
/// script-src 'self'; …`) on the viewer ..., plus `nosniff` and
/// `no-referrer`." (README.md:1575-1576; docs/butterfly.html:829)
#[test]
fn req_f_viewer_pages_carry_csp_nosniff_and_no_referrer() {
    let d = scratch("csp");
    let ev = empty_evidence(&d, "evidence.json");
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));
    for target in ["/", "/app.js", "/app.css", "/butterfly-logo.png"] {
        let (code, h, _) = srv.req("GET", target, &[], "");
        assert_eq!(code, 200, "{target}");
        let csp = header(&h, "content-security-policy").unwrap_or_else(|| panic!("{target}: no Content-Security-Policy"));
        assert!(csp.starts_with("default-src 'none'; script-src 'self'"), "{target}: CSP is {csp:?}");
        assert_eq!(header(&h, "x-content-type-options"), Some("nosniff"), "{target}");
        assert_eq!(header(&h, "referrer-policy"), Some("no-referrer"), "{target}");
    }
    // the page is HTML and the script is JavaScript, so nosniff does not
    // stop the browser using them
    let (_, h, body) = srv.req("GET", "/", &[], "");
    assert!(header(&h, "content-type").unwrap_or_default().starts_with("text/html"));
    assert!(body.contains("<html") || body.contains("<!doctype") || body.contains("<!DOCTYPE"), "/ is the viewer page");
    let (_, h, _) = srv.req("GET", "/app.js", &[], "");
    assert!(header(&h, "content-type").unwrap_or_default().contains("javascript"));
}

/// REQ-F-CSP-REPORT: the CSP is "on the viewer and on the generated report,
/// plus `nosniff` and `no-referrer`. The standalone report carries the policy
/// in a `<meta>` tag" and is "script-free". (README.md:1575-1578, 1356)
#[test]
fn req_f_served_report_carries_the_policy_and_is_script_free() {
    let d = scratch("report-csp");
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));
    let (code, h, html) = srv.req("GET", "/api/report", &[], "");
    assert_eq!(code, 200);
    let csp = header(&h, "content-security-policy").expect("the served report has a CSP header");
    assert!(csp.starts_with("default-src 'none'"), "{csp}");
    assert_eq!(header(&h, "x-content-type-options"), Some("nosniff"));
    assert!(html.contains("http-equiv=\"Content-Security-Policy\""), "the report carries its policy in a <meta> tag");
    assert!(!html.to_ascii_lowercase().contains("<script"), "the report is script-free");
}

/// REQ-F-REFERRER-REPORT: "Content-Security-Policy, nosniff and no-referrer
/// on both the viewer and the report" (docs/butterfly.html:829; README.md:
/// 1575-1576). The viewer's pages send `Referrer-Policy: no-referrer`; the
/// report, served or standalone, carries no referrer policy at all.
#[test]
fn req_f_served_report_carries_no_referrer() {
    let d = scratch("report-referrer");
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));
    let (code, h, html) = srv.req("GET", "/api/report", &[], "");
    assert_eq!(code, 200);
    // no-referrer: as a header, or in the document itself so the policy
    // travels with a mailed copy
    let referrer_header = header(&h, "referrer-policy") == Some("no-referrer");
    let referrer_meta = html.contains("name=\"referrer\"") && html.contains("no-referrer");
    assert!(referrer_header || referrer_meta, "the report has no no-referrer policy, neither as a header nor as a <meta>");
}

// ================================================================== tokens

/// REQ-F-TOKEN-EVERY-ROUTE: "A token is required on **every** API route,
/// reads included, as `Authorization: Bearer <secret>`."
/// (README.md:1513-1514; SECURITY.md:48 "any, with a token | bearer token |
/// bearer token"; docs/butterfly.html:838)
#[test]
fn req_f_every_api_route_needs_the_bearer_token_and_accepts_it() {
    let d = scratch("token-routes");
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    let srv = start(vec![ev], d.join("resolutions.json"), opts(Some(TOKEN), false, &d));
    let right = format!("Bearer {TOKEN}");
    let wrong_creds = [
        None,
        Some("Bearer not-the-token-not-the-token-not".to_string()),
        Some(format!("Bearer {TOKEN}x")),
        Some(TOKEN.to_string()),        // no scheme
        Some(format!("Basic {TOKEN}")), // wrong scheme
        Some("Bearer ".to_string()),
    ];
    for route in READ_ROUTES {
        for bad in &wrong_creds {
            let (code, body) = match bad {
                None => srv.get(route),
                Some(a) => srv.get_auth(route, a),
            };
            assert_eq!(code, 401, "GET {route} with {bad:?} must be refused: {body}");
            assert!(!body.contains(TOKEN), "a refusal echoed the token: {body}");
        }
        let (code, body) = srv.get_auth(route, &right);
        assert_eq!(code, 200, "GET {route} with the right token must be served: {body}");
    }
    for (route, json) in [("/api/resolutions", review("token write")), ("/api/resolutions/delete", r#"{"key":"fn:0"}"#.to_string())] {
        for bad in &wrong_creds {
            let (code, body) = srv.post_json(route, &json, bad.as_deref());
            assert_eq!(code, 401, "POST {route} with {bad:?} must be refused: {body}");
        }
    }
    assert!(!d.join("resolutions.json").exists(), "a refused write reached the disk");
    let (code, body) = srv.post_json("/api/resolutions", &review("token write"), Some(&right));
    assert_eq!(code, 200, "a write with the token is allowed (SECURITY.md:48): {body}");
    assert!(std::fs::read_to_string(d.join("resolutions.json")).unwrap().contains("token write"));
    let (code, body) = srv.post_json("/api/resolutions/delete", r#"{"key":"fn:0"}"#, Some(&right));
    assert_eq!(code, 200, "{body}");

    // The page itself is not an API route: "The viewer asks for it in a
    // password field", which it cannot do if the page needs the token.
    for page in ["/", "/app.js", "/app.css"] {
        assert_eq!(srv.get(page).0, 200, "{page} must load without the token so the viewer can ask for it");
    }
}

/// REQ-F-TOKEN-QUERY: "A URL with `?token=` is refused with a 400"
/// (README.md:1517-1519; SECURITY.md:70-71) -- on any route and method, and
/// whether or not the server has a token at all.
#[test]
fn req_f_a_token_in_the_query_is_refused_everywhere() {
    let d = scratch("token-query");
    let ev = empty_evidence(&d, "evidence.json");
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));
    for target in ["/?token=abc", "/app.js?token=abc", "/api/variants?token=abc", "/api/evidence?v=evidence&token=abc", "/?a=1&token="] {
        let (code, body) = srv.get(target);
        assert_eq!(code, 400, "{target}: {body}");
        assert!(!body.contains("abc"), "{target}: the refused token was echoed: {body}");
    }
    let (code, _) = srv.post_json("/api/resolutions?token=abc", &review("q"), None);
    assert_eq!(code, 400, "a POST carrying ?token= is refused too");
    assert!(!d.join("resolutions.json").exists());
    // a parameter merely *containing* the word is not a token
    assert_eq!(srv.get("/api/variants?tokens=1").0, 200);
}

// ============================================================ write posture

/// REQ-F-READONLY: "`--read-only` refuses writes on any address, token or
/// not." (README.md:1519-1520; SECURITY.md:49)
#[test]
fn req_f_read_only_refuses_writes_on_loopback_with_or_without_the_token() {
    let d = scratch("ro");
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    let srv = start(vec![ev.clone()], d.join("resolutions.json"), opts(None, true, &d));
    assert_eq!(srv.get("/api/evidence").0, 200, "reads still work");
    let (code, body) = srv.post_json("/api/resolutions", &review("ro"), None);
    assert_eq!(code, 403, "loopback --read-only must refuse a write: {body}");
    assert_eq!(srv.post_json("/api/resolutions/delete", r#"{"key":"fn:0"}"#, None).0, 403);
    let (_, boot) = srv.get("/api/variants");
    assert!(boot.contains("\"readOnly\":true"), "the viewer is told: {boot}");
    drop(srv);

    let d2 = scratch("ro-token");
    let ev2 = c_evidence(&d2, "evidence.json", None).unwrap();
    let srv = start(vec![ev2], d2.join("resolutions.json"), opts(Some(TOKEN), true, &d2));
    let (code, body) = srv.post_json("/api/resolutions", &review("ro"), Some(&format!("Bearer {TOKEN}")));
    assert_eq!(code, 403, "--read-only refuses a write even with the right token: {body}");
    assert!(!d.join("resolutions.json").exists() && !d2.join("resolutions.json").exists(), "nothing was written");
}

/// REQ-F-LOOPBACK-OPEN: "Loopback with no flags is unchanged: reads and
/// writes, no credentials." (README.md:1520-1521; SECURITY.md:45)
/// REQ-F-EVIDENCE-UNMODIFIED: "`butterfly serve` never modifies the
/// evidence." (README.md:1462-1463; docs/butterfly.html:855)
#[test]
fn req_f_loopback_writes_need_no_credentials_and_never_touch_the_evidence() {
    let d = scratch("open");
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    let before = sha256_hex(&std::fs::read(&ev).unwrap());
    let srv = start(vec![ev.clone()], d.join("resolutions.json"), opts(None, false, &d));

    let (code, body) = srv.post_json("/api/resolutions", &review("open write"), None);
    assert_eq!(code, 200, "{body}");
    let side = std::fs::read_to_string(d.join("resolutions.json")).expect("the sidecar is written");
    assert!(side.contains("open write"), "{side}");
    let (_, got) = srv.get("/api/resolutions");
    assert!(got.contains("open write"), "{got}");

    // replace, then delete
    assert_eq!(srv.post_json("/api/resolutions", &review("second thoughts"), None).0, 200);
    let (_, got) = srv.get("/api/resolutions");
    assert!(got.contains("second thoughts") && !got.contains("open write"), "an upsert replaces the review for the same key: {got}");
    assert_eq!(srv.post_json("/api/resolutions/delete", r#"{"key":"fn:0"}"#, None).0, 200);
    let (_, got) = srv.get("/api/resolutions");
    assert!(!got.contains("second thoughts"), "{got}");
    assert!(!std::fs::read_to_string(d.join("resolutions.json")).unwrap().contains("second thoughts"), "deleted on disk too");

    let _ = srv.get("/api/report");
    let _ = srv.get("/api/gates?min_lines=50");
    assert_eq!(sha256_hex(&std::fs::read(&ev).unwrap()), before, "serve modified the evidence file");
}

/// REQ-F-REPORT-CACHE: "The rendered report is cached until a review changes
/// it." (README.md:1540; docs/butterfly.html:837) -- so a review written
/// through the viewer is in the next report, and a deleted one is gone.
#[test]
fn req_f_the_report_follows_reviews_written_through_the_viewer() {
    let d = scratch("report-cache");
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));
    // "gaps with their resolutions" (README.md:1358): a review of a gap
    let mark = "REQF-UNIQUE-JUSTIFICATION-7731";
    let key = first_gap_key(&d.join("evidence.json"));
    let body = format!(r#"{{"key":"{key}","category":"analysis","justification":"{mark}","author":"req-f","date":"2026-01-01"}}"#);
    let (_, first) = srv.get("/api/report");
    assert!(!first.contains(mark));
    let (_, again) = srv.get("/api/report");
    assert_eq!(first, again, "nothing changed, so the same report");
    assert_eq!(srv.post_json("/api/resolutions", &body, None).0, 200);
    let (_, after) = srv.get("/api/report");
    assert!(after.contains(mark), "the report did not pick up the review just written");
    assert_eq!(srv.post_json("/api/resolutions/delete", &format!(r#"{{"key":"{key}"}}"#), None).0, 200);
    let (_, gone) = srv.get("/api/report");
    assert!(!gone.contains(mark), "the report still shows a deleted review");
}

/// REQ-F-SIDECAR-PER-VARIANT: "`butterfly serve a.json b.json …` serves any
/// set of evidence files as variants (one resolutions sidecar per variant)."
/// (README.md:1447-1448; docs/butterfly.html:827)
#[test]
fn req_f_each_variant_records_reviews_in_its_own_sidecar() {
    let d = scratch("per-variant");
    let Some(a) = c_evidence(&d, "evidence-alpha.json", None) else { return };
    let b = c_evidence(&d, "evidence-beta.json", None).unwrap();
    let srv = start(vec![a, b], d.join("resolutions.json"), opts(None, false, &d));
    let (_, boot) = srv.get("/api/variants");
    let boot: serde_json::Value = serde_json::from_str(&boot).unwrap();
    let names: Vec<&str> = boot["variants"].as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["alpha", "beta"], "variants are named and listed in order");

    assert_eq!(srv.post_json("/api/resolutions?v=beta", &review("only beta"), None).0, 200);
    let (_, ra) = srv.get("/api/resolutions?v=alpha");
    let (_, rb) = srv.get("/api/resolutions?v=beta");
    assert!(rb.contains("only beta") && !ra.contains("only beta"), "alpha={ra} beta={rb}");
    let files: Vec<String> =
        std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n != "evidence-alpha.json" && n != "evidence-beta.json").collect();
    assert_eq!(files.len(), 1, "exactly one sidecar, the one beta wrote: {files:?}");
    assert!(std::fs::read_to_string(d.join(&files[0])).unwrap().contains("only beta"));
    assert_ne!(files[0], "resolutions.json", "with several variants none of them writes the shared base name");
}

/// REQ-F-NAMES-NO-PATHS: "Names from data never shape file paths. The
/// resolutions sidecar path is built with a sanitised variant name."
/// (README.md:1620-1621; docs/butterfly.html:827, 833)
#[test]
fn req_f_a_hostile_variant_name_cannot_place_the_sidecar_elsewhere() {
    let root = scratch("hostile-name");
    let d = root.join("served");
    std::fs::create_dir_all(&d).unwrap();
    let Some(a) = c_evidence(&d, "evidence-a.json", Some("../../escaped")) else { return };
    let b = c_evidence(&d, "evidence-b.json", Some("/tmp/absolute")).unwrap();
    let srv = start(vec![a, b], d.join("resolutions.json"), opts(None, false, &d));
    for v in ["../../escaped", "/tmp/absolute"] {
        let q = v.replace('/', "%2F");
        let (code, body) = srv.post_json(&format!("/api/resolutions?v={q}"), &review("hostile"), None);
        assert_eq!(code, 200, "{v}: {body}");
    }
    // every file written is inside the resolutions directory
    let outside: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).filter(|n| n != "served").collect();
    assert!(outside.is_empty(), "a sidecar was written outside the resolutions directory: {outside:?}");
    let sidecars = std::fs::read_dir(&d).unwrap().filter(|e| !e.as_ref().unwrap().file_name().to_string_lossy().starts_with("evidence-")).count();
    assert_eq!(sidecars, 2, "one sidecar per variant, both beside --resolutions");
}

/// REQ-F-SYMLINK: "A symlink at the destination is displaced rather than
/// followed" -- the sidecars included (README.md:1565-1570;
/// docs/butterfly.html:836).
#[cfg(unix)]
#[test]
fn req_f_a_symlinked_sidecar_is_replaced_not_written_through() {
    let d = scratch("symlink");
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    // a valid, empty review set that the link points at
    let victim = d.join("victim.json");
    let empty = serde_json::json!({ "schema": butterfly_model::RESOLUTIONS_SCHEMA, "entries": [] }).to_string();
    std::fs::write(&victim, &empty).unwrap();
    let link = d.join("resolutions.json");
    std::os::unix::fs::symlink(&victim, &link).unwrap();

    let srv = start(vec![ev], link.clone(), opts(None, false, &d));
    let (code, body) = srv.post_json("/api/resolutions", &review("through a link"), None);
    assert_eq!(code, 200, "{body}");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), empty, "the link's target was written through");
    let md = std::fs::symlink_metadata(&link).unwrap();
    assert!(md.file_type().is_file(), "the link was not displaced by a regular file");
    assert!(std::fs::read_to_string(&link).unwrap().contains("through a link"));
}

// ============================================================ variants / 404

/// REQ-F-VARIANT-SELECT: "Several files = variants of the same sources"
/// (README.md:121). A variant that is not being served is not answered with
/// another one's data. The status is not documented; anything but success.
#[test]
fn req_f_an_unknown_variant_is_not_answered_with_another_ones_data() {
    let d = scratch("unknown-variant");
    let a = empty_evidence(&d, "evidence-one.json");
    let b = empty_evidence(&d, "evidence-two.json");
    let srv = start(vec![a, b], d.join("resolutions.json"), opts(None, false, &d));
    for route in ["/api/evidence", "/api/report", "/api/freshness", "/api/tags", "/api/requirements", "/api/gates", "/api/resolutions"] {
        let (code, body) = srv.get(&format!("{route}?v=three"));
        assert!(!(200..300).contains(&code), "{route}?v=three was answered {code}: {body}");
        assert_eq!(srv.get(&format!("{route}?v=two")).0, 200, "{route}?v=two");
    }
    let (code, _) = srv.post_json("/api/resolutions?v=three", r#"{"key":"fn:0","category":"a","justification":"j","author":"a","date":"2026-01-01"}"#, None);
    assert!(!(200..300).contains(&code));
    let (code, _) = srv.post_json("/api/resolutions/delete?v=three", r#"{"key":"fn:0"}"#, None);
    assert!(!(200..300).contains(&code));
}

/// REQ-F-NO-BUILD-ENDPOINT: "It does not **build** anything ... the endpoint
/// runs no subprocess ... Producing evidence stays with the CLI."
/// (README.md:1066-1075); "the viewer has no such endpoint" (SECURITY.md:30-33)
#[test]
fn req_f_the_viewer_has_no_endpoint_that_builds_or_runs() {
    let d = scratch("no-build");
    let ev = empty_evidence(&d, "evidence.json");
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));
    for (m, t) in [("POST", "/api/build"), ("POST", "/api/run"), ("GET", "/api/build"), ("POST", "/api/ingest"), ("POST", "/api/gates"), ("POST", "/api/report")] {
        let (code, _, _) = srv.req(m, t, &[("Content-Type", "application/json")], "{}");
        assert!(code == 404 || code == 405, "{m} {t} answered {code}");
    }
}

/// REQ-F-SIMPLE-POST: a cross-site page can send a "simple `POST`
/// (form-encoded or `text/plain`) ... without a preflight, and the side
/// effect happens anyway" (SECURITY.md:26-28). On the open loopback default
/// such a request must not record a review decision.
#[test]
fn req_f_a_cross_site_simple_post_records_nothing() {
    let d = scratch("simple-post");
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));
    for ct in ["text/plain", "application/x-www-form-urlencoded", "multipart/form-data; boundary=x"] {
        let (code, _, _) = srv.req("POST", "/api/resolutions", &[("Content-Type", ct), ("Origin", "http://evil.example")], &review("cross site"));
        assert!(!(200..300).contains(&code), "a {ct} POST was accepted ({code})");
    }
    assert!(!d.join("resolutions.json").exists(), "a simple cross-site POST recorded a decision");
}

// ================================================================== gates

/// REQ-F-GATES: "With no floors set it says *nothing is being gated* rather
/// than implying a pass"; with a floor it "reports the same verdict `butterfly
/// check` would" and "always shows **the command that does the same thing**"
/// with "exits 3" on a breach. (README.md:1045-1062)
#[test]
fn req_f_gates_judge_only_when_asked_and_name_the_command() {
    let d = scratch("gates");
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));

    let (code, body) = srv.get("/api/gates");
    assert_eq!(code, 200, "{body}");
    let g: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(g["thresholdsGiven"], false, "{body}");
    assert_eq!(g["breaches"].as_array().map(Vec::len), Some(0), "no floor, no breach: {body}");
    assert_eq!(g["wouldExit"], 0);

    // examples/c deliberately leaves outcomes uncovered (README "What it shows")
    let (code, body) = srv.get("/api/gates?min_outcomes=100");
    assert_eq!(code, 200, "{body}");
    let g: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(g["thresholdsGiven"], true);
    assert!(!g["breaches"].as_array().unwrap().is_empty(), "a 100% outcome floor must breach on examples/c: {body}");
    assert_eq!(g["wouldExit"], 3);
    let cmd = g["command"].as_str().unwrap();
    assert!(cmd.starts_with("butterfly check ") && cmd.contains("--min-outcomes 100"), "{cmd}");

    // every floor is carried into the command
    let (_, body) = srv.get("/api/gates?min_lines=1&min_outcomes=2&min_mcdc=3&max_unresolved_gaps=4");
    let g: serde_json::Value = serde_json::from_str(&body).unwrap();
    let cmd = g["command"].as_str().unwrap();
    for f in ["--min-lines 1", "--min-outcomes 2", "--min-mcdc 3", "--max-unresolved-gaps 4"] {
        assert!(cmd.contains(f), "{f} missing from {cmd}");
    }
}

/// REQ-F-GATES-NO-PATH: the gates endpoint "accepts no path from the client.
/// It computes over data any reader of `/api/evidence` already has."
/// (README.md:1068-1070)
#[test]
fn req_f_gates_ignore_any_path_the_client_sends() {
    let d = scratch("gates-path");
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));
    let (code, body) = srv.get("/api/gates?evidence=%2Fetc%2Fpasswd&resolutions=%2Fetc%2Fhosts&path=%2Fetc&min_lines=1");
    // refusing the unknown parameters would also satisfy the requirement
    if code == 200 {
        let g: serde_json::Value = serde_json::from_str(&body).unwrap();
        let cmd = g["command"].as_str().unwrap();
        assert!(!cmd.contains("/etc/"), "a client-supplied path reached the command: {cmd}");
        assert!(cmd.contains("evidence.json"), "{cmd}");
    } else {
        assert!((400..500).contains(&code), "{code}: {body}");
    }
}

// ============================================================ tags sidecar

/// REQ-F-TAGS: "`serve` picks the sidecar up automatically when it sits
/// beside the evidence and was built against it — a stale one is ignored
/// with a note" (README.md:155-157); "its absence changes nothing"
/// (README.md:128).
#[test]
fn req_f_a_tags_sidecar_is_served_only_when_built_for_this_evidence() {
    let d = scratch("tags");
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    let ev_text = std::fs::read(&ev).unwrap();
    let tag = serde_json::json!({ "name": "req_f_marker", "kind": "macro", "language": "C", "file": "demo.c", "line": 3 });
    let mk = |hash: &str| serde_json::json!({ "schema": butterfly_model::tags::TAGS_SCHEMA, "generator": "req-f", "evidenceSha256": hash, "entries": [tag] }).to_string();

    // absent: an empty index, and the viewer still serves
    let srv = start(vec![ev.clone()], d.join("resolutions.json"), opts(None, false, &d));
    let (code, body) = srv.get("/api/tags");
    assert_eq!(code, 200);
    assert!(!body.contains("req_f_marker"), "{body}");
    drop(srv);

    // built against this evidence: served
    std::fs::write(d.join("evidence.tags.json"), mk(&sha256_hex(&ev_text))).unwrap();
    let srv = start(vec![ev.clone()], d.join("resolutions.json"), opts(None, false, &d));
    let (_, body) = srv.get("/api/tags");
    assert!(body.contains("req_f_marker"), "a matching sidecar was not served: {body}");
    drop(srv);

    // built against other evidence: ignored, and the evidence still served
    std::fs::write(d.join("evidence.tags.json"), mk(&"0".repeat(64))).unwrap();
    let srv = start(vec![ev.clone()], d.join("resolutions.json"), opts(None, false, &d));
    let (_, body) = srv.get("/api/tags");
    assert!(!body.contains("req_f_marker"), "a stale sidecar was served: {body}");
    assert_eq!(srv.get("/api/evidence").0, 200);
    drop(srv);

    // a foreign schema or garbage: ignored the same way
    for junk in [mk(&sha256_hex(&ev_text)).replace(butterfly_model::tags::TAGS_SCHEMA, "someone-else/9"), "{not json".to_string()] {
        std::fs::write(d.join("evidence.tags.json"), junk).unwrap();
        let srv = start(vec![ev.clone()], d.join("resolutions.json"), opts(None, false, &d));
        let (code, body) = srv.get("/api/tags");
        assert_eq!(code, 200);
        assert!(!body.contains("req_f_marker"), "{body}");
    }
}

// ===================================================== requirements sidecar

fn reqtext(entries: &[(&str, &str)]) -> String {
    let e: Vec<serde_json::Value> =
        entries.iter().map(|(id, text)| serde_json::json!({ "id": id, "text": text, "detail": { "rationale": format!("why {id}") }, "source": 0 })).collect();
    serde_json::json!({
        "schema": butterfly_model::reqtext::REQTEXT_SCHEMA,
        "generator": "req-f",
        "sources": [{ "path": "/nonexistent/req-f/llr.yaml", "sha256": "0".repeat(64), "count": entries.len() }],
        "entries": e,
    })
    .to_string()
}

/// REQ-F-REQTEXT: "`butterfly serve` picks up `requirements.json` from beside
/// the evidence (or `<evidence-stem>.requirements.json`, which wins if both
/// are there)"; the viewer "asks `/api/requirements?ids=…` ... the statement
/// is all that comes back until something asks for more — `full=1` adds the
/// analysis"; "Absence changes nothing". (README.md:733-734, 747-750, 762)
#[test]
fn req_f_requirement_text_stem_sidecar_wins_and_full_adds_the_analysis() {
    let d = scratch("reqtext");
    let ev = empty_evidence(&d, "evidence.json");
    std::fs::write(d.join("requirements.json"), reqtext(&[("LLR-1", "shared text"), ("LLR-2", "only shared")])).unwrap();
    std::fs::write(d.join("evidence.requirements.json"), reqtext(&[("LLR-1", "stem text")])).unwrap();
    let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));

    let (code, body) = srv.get("/api/requirements?ids=LLR-1,LLR-2");
    assert_eq!(code, 200, "{body}");
    let j: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(j["present"], true);
    assert_eq!(j["entries"][0]["text"], "stem text", "the stem-named sidecar wins: {body}");
    assert_eq!(j["unknown"], serde_json::json!(["LLR-2"]), "the shared file is not merged in: {body}");
    assert!(!body.contains("why LLR-1"), "the analysis comes only on request: {body}");

    let (_, body) = srv.get("/api/requirements?ids=LLR-1&full=1");
    assert!(body.contains("why LLR-1"), "full=1 adds the analysis: {body}");
}

/// REQ-F-REQTEXT-ABSENT: "Absence changes nothing: with no sidecar a tag
/// shows its ID" (README.md:762) -- and an unreadable one is the same.
#[test]
fn req_f_a_broken_requirements_sidecar_changes_nothing() {
    for (tag, body) in
        [("garbage", "{not json".to_string()), ("schema", reqtext(&[("A", "a")]).replace("butterfly-requirements/1", "other/2")), ("unsorted", reqtext(&[("B", "b"), ("A", "a")]))]
    {
        let d = scratch(&format!("reqtext-{tag}"));
        let ev = empty_evidence(&d, "evidence.json");
        std::fs::write(d.join("requirements.json"), body).unwrap();
        let srv = start(vec![ev], d.join("resolutions.json"), opts(None, false, &d));
        let (code, body) = srv.get("/api/requirements?ids=A");
        assert_eq!(code, 200, "{tag}: {body}");
        let j: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(j["present"], false, "{tag}: a broken sidecar must read as absent: {body}");
        assert_eq!(srv.get("/api/evidence").0, 200, "{tag}");
    }
}

// =========================================================== load refusals

/// REQ-F-VALIDATE-ON-LOAD: "Evidence files are validated on load. Every
/// index ... is range-checked before anything renders, and a foreign
/// `schema` is rejected: a truncated or hostile evidence file produces a
/// named error, never a panic mid-render." (README.md:1616-1619;
/// docs/butterfly.html:832)
#[test]
fn req_f_foreign_truncated_or_inconsistent_evidence_is_a_named_error() {
    let d = scratch("validate");
    let foreign = d.join("foreign.json");
    std::fs::write(&foreign, serde_json::json!({ "schema": "not-butterfly/99" }).to_string()).unwrap();
    let truncated = d.join("truncated.json");
    let whole = serde_json::to_string(&butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() }).unwrap();
    std::fs::write(&truncated, &whole[..whole.len() / 2]).unwrap();

    for p in [&foreign, &truncated] {
        let err = refuse(vec![p.clone()], d.join("resolutions.json"), opts(None, false, &d));
        assert!(err.contains(&*p.file_name().unwrap().to_string_lossy()), "the error does not name {}: {err}", p.display());
    }

    // an index out of range: a function pointing at a file that is not there
    let Some(ev) = c_evidence(&d, "evidence.json", None) else { return };
    let mut j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&ev).unwrap()).unwrap();
    j["functions"][0]["fileId"] = serde_json::json!(9999);
    let bad = d.join("out-of-range.json");
    std::fs::write(&bad, j.to_string()).unwrap();
    let err = refuse(vec![bad], d.join("resolutions.json"), opts(None, false, &d));
    assert!(err.contains("out-of-range.json"), "{err}");
}

/// REQ-F-NO-EVIDENCE: `serve <evidence…>` (README.md:121) needs at least one.
#[test]
fn req_f_serve_with_no_evidence_is_an_error() {
    let d = scratch("none");
    let err = refuse(vec![], d.join("resolutions.json"), opts(None, false, &d));
    assert!(err.to_lowercase().contains("evidence"), "{err}");
}

/// REQ-F-RESOLUTIONS-LOAD: the resolutions sidecar `serve` loads is
/// validated, and a present-but-broken one is never read as "no reviews"
/// (README.md:1362-1366 for the report; the same rule for the viewer's
/// sidecar, README.md:1579-1583). A broken sidecar would otherwise be
/// written over, with every recorded decision in it, on the next review.
#[test]
fn req_f_a_present_but_broken_resolutions_sidecar_stops_the_viewer() {
    let d = scratch("bad-res");
    let ev = empty_evidence(&d, "evidence.json");
    for (name, body) in [("garbage.json", "{not json"), ("schema.json", r#"{"schema":"other/3","entries":[]}"#)] {
        let side = d.join(name);
        std::fs::write(&side, body).unwrap();
        let err = refuse(vec![ev.clone()], side.clone(), opts(None, false, &d));
        assert!(err.contains(name), "the refusal names the sidecar: {err}");
        assert_eq!(std::fs::read_to_string(&side).unwrap(), body, "and leaves it as it was");
    }
    #[cfg(unix)]
    {
        let dangling = d.join("dangling.json");
        std::os::unix::fs::symlink(d.join("does-not-exist"), &dangling).unwrap();
        let err = refuse(vec![ev.clone()], dangling, opts(None, false, &d));
        assert!(err.contains("dangling.json"), "a dangling link is broken, not absent: {err}");
    }
}

// ======================================================= listener decisions

/// REQ-F-REMOTE-TWO-FLAGS: "Binding a non-loopback address is refused unless
/// you pass both `--allow-remote` and `--insecure-http`." (README.md:1494-1495;
/// SECURITY.md:73-74) -- `--insecure-http` alone is not enough either.
#[test]
fn req_f_a_non_loopback_bind_needs_both_flags_whichever_is_missing() {
    let base = ServeOptions {
        listen: "0.0.0.0:0".into(),
        allow_remote: false,
        insecure_http: false,
        read_only: false,
        token: Some(Token::supplied(TOKEN.into())),
        allow_unauthenticated_remote: false,
        source_root: None,
        repo: None,
    };
    for (allow_remote, insecure_http) in [(false, false), (false, true), (true, false)] {
        let o = ServeOptions { allow_remote, insecure_http, ..base.clone() };
        assert!(o.check().is_err(), "allow_remote={allow_remote} insecure_http={insecure_http} must refuse a non-loopback bind");
    }
    let o = ServeOptions { allow_remote: true, insecure_http: true, ..base.clone() };
    assert!(!o.check().unwrap().loopback, "both flags and a token: allowed, and not loopback");
    // REQ-F-REMOTE-TOKEN: "Off loopback butterfly refuses to start without a
    // token" ... "`--allow-unauthenticated-remote` is the named exception"
    // (README.md:1506-1511)
    let o = ServeOptions { allow_remote: true, insecure_http: true, token: None, ..base.clone() };
    assert!(o.check().is_err(), "no token off loopback must refuse");
    let o = ServeOptions { allow_remote: true, insecure_http: true, token: None, allow_unauthenticated_remote: true, ..base.clone() };
    assert!(o.check().is_ok());
    // loopback needs none of it (README.md:1520-1521)
    for l in ["127.0.0.1:0", "[::1]:0", "localhost:0"] {
        let o = ServeOptions { listen: l.into(), token: None, ..base.clone() };
        assert!(o.check().unwrap().loopback, "{l}");
    }
}
