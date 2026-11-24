// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! How a bearer token reaches `butterfly serve`, and where it must never
//! appear, exercised against the real binary (#65).
//!
//! * A network listener is plain HTTP, so it refuses to start until the
//!   operator passes `--insecure-http` -- with or without a token.
//! * A token comes from `--token-file`, `BUTTERFLY_TOKEN`, or the deprecated
//!   literal `--token`; two at once is refused.
//! * A supplied token appears nowhere in the server's output, and the URL
//!   butterfly prints for a minted one keeps it out of the request target.
//!
//! The refusals need no fixture -- they happen before any evidence is read --
//! so they always run. The rest need examples/c ingested and skip without it.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod common;
use common::Server;

const SECRET: &str = "a-secret-the-operator-chose-0123";

/// One way of handing the server a token: a name for messages, the flags,
/// and the environment.
type Case<'a> = (&'a str, Vec<&'a str>, Vec<(&'a str, &'a str)>);

/// A small, valid evidence file of its own, so these tests always run: what
/// they exercise is the server's handling of tokens, not the contents of any
/// example build. Waiting on examples/c let a run without fixtures pass
/// having tested nothing (review of #65). Always `Some`; the `Option` keeps
/// the callers' shape.
fn evidence() -> Option<PathBuf> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = scratch(&format!("evidence-{}", N.fetch_add(1, Ordering::Relaxed)));
    // No files and no functions: the smallest document `serve` validates and
    // loads, which is all that token handling needs.
    let ev = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
    let p = d.join("evidence.json");
    std::fs::write(&p, serde_json::to_string(&ev).unwrap()).unwrap();
    Some(p)
}

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("exposure-{tag}"))
}

fn private_file(dir: &Path, contents: &str) -> PathBuf {
    let p = dir.join("token");
    std::fs::write(&p, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    p
}

/// The binary, with no token inherited from whoever runs the tests.
fn butterfly() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_butterfly"));
    c.env_remove("BUTTERFLY_TOKEN").stdin(Stdio::null()).stdout(Stdio::null());
    c
}

/// Run a command that is expected to refuse, and return its stderr. A
/// command that starts serving instead would never exit, so it is given a
/// deadline and killed -- and that is a failure, not a hang.
fn refused(mut cmd: Command) -> String {
    let mut child = cmd.stderr(Stdio::piped()).spawn().expect("spawn butterfly");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let mut err = String::new();
            child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
            assert!(!status.success(), "butterfly exited 0 where it had to refuse:\n{err}");
            return err;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("butterfly did not refuse: it was still running after 20 s, so it was serving");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Serve `ev` on loopback with `args` and `env`, through the shared helper
/// every HTTP suite uses: `--listen 127.0.0.1:0`, the port read back from
/// the server's own URL line, and ready only once it has answered a request
/// (#91, #99). The refusals above keep their own process handling, because
/// for them a server that starts is the failure.
fn start(dir: &Path, ev: &Path, args: &[&str], env: &[(&str, &str)]) -> Server {
    start_on(dir, ev, "127.0.0.1:0", args, env)
}

/// [`start`] on another address -- always port 0, reached over loopback.
fn start_on(dir: &Path, ev: &Path, listen: &str, args: &[&str], env: &[(&str, &str)]) -> Server {
    let mut cmd = butterfly();
    cmd.arg("serve").arg(ev).arg("--resolutions").arg(dir.join("resolutions.json")).args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    common::serve_on(&mut cmd, listen)
}

/// One GET, with the token as a header when there is one. A failed request
/// names the server's own account of itself, as every shared-helper request
/// does.
fn get(srv: &mut Server, target: &str, bearer: Option<&str>) -> (u16, String) {
    let auth = bearer.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.as_deref().map(|a| ("Authorization", a)).into_iter().collect();
    common::request(srv, "GET", target, &headers, "")
}

/// The regression the issue asks for: non-loopback authenticated mode must
/// not start without a declared insecure override. Before, `--allow-remote`
/// alone served the token and the source over plain HTTP with a warning.
#[test]
fn a_network_listener_refuses_to_start_without_insecure_http() {
    let d = scratch("remote");
    let tf = private_file(&d, &format!("{SECRET}\n"));
    // nothing is loaded before this refusal, so no fixture is needed
    let ev = d.join("never-read.json");
    let tf_arg = tf.to_str().unwrap();
    for extra in [vec![], vec!["--token-file", tf_arg], vec!["--token", "generate"], vec!["--read-only"]] {
        let mut cmd = butterfly();
        cmd.arg("serve").arg(&ev).args(["--listen", "0.0.0.0:0", "--allow-remote"]).args(&extra);
        let err = refused(cmd);
        assert!(err.contains("--insecure-http"), "{extra:?}: the refusal names the acknowledgement:\n{err}");
        assert!(err.contains("plain HTTP") && err.contains("reverse proxy") && err.contains("ssh -N -L"), "{extra:?}: and says why and what to do instead:\n{err}");
        assert!(!err.contains("listening on"), "{extra:?}: and nothing was served:\n{err}");
        assert!(!err.contains(SECRET), "{extra:?}: {err}");
    }

    // the same refusal comes before `run` builds anything
    let mut cmd = butterfly();
    cmd.arg("run").arg(&d).args(["--build-script", "/nonexistent/never-run.sh", "--listen", "0.0.0.0:0", "--allow-remote"]);
    let err = refused(cmd);
    assert!(err.contains("--insecure-http") && !err.contains("never-run"), "`run` refuses before the build step:\n{err}");
    let _ = std::fs::remove_dir_all(d);
}

/// Nothing butterfly starts inherits the viewer's token. `run`'s build
/// script is the operator's own code, and whenever BUTTERFLY_TOKEN supplied
/// the token, the script -- and every compiler it ran -- had it in its
/// environment (review of #65).
#[cfg(unix)]
#[test]
fn the_build_script_run_starts_does_not_inherit_the_token() {
    use std::os::unix::fs::PermissionsExt;
    let d = scratch("child-env");
    let script = d.join("build.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf '%s' \"${BUTTERFLY_TOKEN-unset}\" > \"$(dirname \"$0\")/seen\"\nexit 1\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = butterfly().arg("run").arg(&d).arg("--no-serve").env("BUTTERFLY_TOKEN", SECRET).stderr(Stdio::piped()).output().unwrap();
    let seen = std::fs::read_to_string(d.join("seen")).unwrap_or_else(|_| panic!("the build script did not run:\n{}", String::from_utf8_lossy(&out.stderr)));
    assert_eq!(seen, "unset", "the build script saw the viewer's token");
    let _ = std::fs::remove_dir_all(d);
}

/// One POST with a JSON body. Returns (status, body).
fn post(srv: &mut Server, target: &str, json: &str) -> (u16, String) {
    common::request(srv, "POST", target, &[("Content-Type", "application/json")], json)
}

/// `--allow-unauthenticated-remote` is the one named way to serve a network
/// listener without a token: reads open to anyone who can reach it, writes
/// refused (#67). It has to bind a non-loopback address to be that mode at
/// all, so it listens on 0.0.0.0, port 0, and is reached over loopback.
#[test]
fn an_explicitly_opened_network_listener_serves_reads_and_refuses_writes() {
    let Some(ev) = evidence() else { return };
    let d = scratch("open-remote");
    let mut srv = start_on(&d, &ev, "0.0.0.0:0", &["--allow-remote", "--insecure-http", "--allow-unauthenticated-remote"], &[]);
    let out = srv.stderr();
    assert!(out.contains("OPEN TO ANYONE WHO CAN REACH THIS PORT (--allow-unauthenticated-remote)"), "the banner names the flag that opened it:\n{out}");

    let (code, body) = get(&mut srv, "/api/variants", None);
    assert_eq!(code, 200, "reads need no token in this mode: {body}");
    assert!(body.contains("\"readOnly\":true"), "and the viewer is told it is read-only: {body}");
    assert_eq!(get(&mut srv, "/api/report", None).0, 200);

    let review = r#"{"key":"fn:0","category":"a","justification":"j","author":"t","date":"2026-01-01"}"#;
    let (code, body) = post(&mut srv, "/api/resolutions", review);
    assert_eq!(code, 403, "writes are refused: {body}");
    assert!(body.contains("read-only"), "{body}");
    assert_eq!(post(&mut srv, "/api/resolutions/delete", r#"{"key":"fn:0"}"#).0, 403);
    assert!(!d.join("resolutions.json").exists(), "and nothing was written");
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}

/// A network listener must authenticate its readers, read-only included,
/// unless the operator names the exception (#67). Before, `--allow-remote`
/// with no token served the source to anyone who could reach the port, and
/// let them keep the server busy rendering reports.
#[test]
fn a_network_listener_without_a_token_refuses_unless_explicitly_opened() {
    let d = scratch("unauthenticated");
    // nothing is loaded before this refusal either
    let ev = d.join("never-read.json");
    for extra in [vec![], vec!["--read-only"]] {
        let mut cmd = butterfly();
        cmd.arg("serve").arg(&ev).args(["--listen", "0.0.0.0:0", "--allow-remote", "--insecure-http"]).args(&extra);
        let err = refused(cmd);
        assert!(err.contains("without authentication") && err.contains("read-only included"), "{extra:?}: says why:\n{err}");
        assert!(err.contains("--token-file") && err.contains("BUTTERFLY_TOKEN") && err.contains("--token generate"), "{extra:?}: names every token source:\n{err}");
        assert!(err.contains("--allow-unauthenticated-remote"), "{extra:?}: and the override, by name:\n{err}");
        assert!(!err.contains("listening on"), "{extra:?}: and nothing was served:\n{err}");
    }
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn two_token_sources_are_refused_and_neither_is_printed() {
    let d = scratch("conflict");
    let tf = private_file(&d, &format!("{SECRET}\n"));
    let ev = d.join("never-read.json");

    let mut cmd = butterfly();
    cmd.arg("serve").arg(&ev).arg("--token-file").arg(&tf).env("BUTTERFLY_TOKEN", SECRET);
    let err = refused(cmd);
    assert!(err.contains("--token-file") && err.contains("BUTTERFLY_TOKEN"), "{err}");
    assert!(!err.contains(SECRET), "{err}");

    let mut cmd = butterfly();
    cmd.arg("serve").arg(&ev).arg("--token-file").arg(&tf).args(["--token", SECRET]);
    let err = refused(cmd);
    assert!(err.contains("--token-file") && err.contains("--token"), "{err}");
    assert!(!err.contains(SECRET), "{err}");

    let empty = d.join("empty");
    std::fs::write(&empty, "\n").unwrap();
    let mut cmd = butterfly();
    cmd.arg("serve").arg(&ev).arg("--token-file").arg(&empty);
    assert!(refused(cmd).contains("is empty"), "an empty token file is refused, not read as 'no token'");
    let _ = std::fs::remove_dir_all(d);
}

/// A supplied token works from a file and from the environment, is required
/// on the API, and is written nowhere in the server's output -- after
/// startup, a good request, a bad one, and a refused `?token=` URL.
#[test]
fn a_token_from_a_file_or_the_environment_is_required_and_never_printed() {
    let Some(ev) = evidence() else { return };
    let d = scratch("sources");
    let tf = private_file(&d, &format!("{SECRET}\n"));
    let tf_arg = tf.to_str().unwrap().to_string();
    let cases: [Case; 2] = [("file", vec!["--token-file", &tf_arg], vec![]), ("env", vec![], vec![("BUTTERFLY_TOKEN", SECRET)])];
    for (name, args, env) in cases {
        let mut srv = start(&d, &ev, &args, &env);
        assert_eq!(get(&mut srv, "/api/variants", None).0, 401, "{name}: reads need the token");
        assert_eq!(get(&mut srv, "/api/variants", Some("not-the-token-at-all-0000")).0, 401, "{name}");
        let (code, body) = get(&mut srv, "/api/variants", Some(SECRET));
        assert_eq!(code, 200, "{name}: {body}");

        // The old bootstrap link. The token in it has already been sent by
        // the time the server sees it; it is refused and not echoed.
        let (code, body) = get(&mut srv, &format!("/?token={SECRET}"), None);
        assert_eq!(code, 400, "{name}: a token in the query is refused: {body}");
        assert!(body.contains("#token=") && !body.contains(SECRET), "{name}: {body}");
        assert_eq!(get(&mut srv, &format!("/api/variants?v=x&token={SECRET}"), Some(SECRET)).0, 400, "{name}: on every route");

        let out = srv.stderr();
        assert!(out.contains("require a bearer token"), "{name}: {out}");
        assert!(out.contains("enter your token"), "{name}: {out}");
        assert!(!out.contains(SECRET), "{name}: the supplied token is in the server's output:\n{out}");
        assert!(!out.contains("deprecated"), "{name}: these are the recommended sources:\n{out}");
    }
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_literal_token_still_works_but_is_deprecated_without_being_echoed() {
    let Some(ev) = evidence() else { return };
    let d = scratch("literal");
    let mut srv = start(&d, &ev, &["--token", SECRET], &[]);
    assert_eq!(get(&mut srv, "/api/variants", Some(SECRET)).0, 200, "existing scripts keep working");
    let out = srv.stderr();
    assert!(out.contains("deprecated") && out.contains("--token-file"), "and are told what to use instead:\n{out}");
    assert!(!out.contains(SECRET), "{out}");
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}

/// Follow the bootstrap exactly as a browser does: the request it makes for
/// the printed URL is everything before `#`, and that must carry no token.
/// The token then goes in the header, where it is accepted.
#[test]
fn the_bootstrap_link_never_sends_the_token_in_the_request_target() {
    let Some(ev) = evidence() else { return };
    let d = scratch("bootstrap");
    let mut srv = start(&d, &ev, &["--token", "generate"], &[]);
    let out = srv.stderr();
    let url = out.lines().find(|l| l.contains("open the viewer at")).and_then(|l| l.split_whitespace().last()).unwrap_or_else(|| panic!("no link printed:\n{out}")).to_string();
    let (sent, fragment) = url.split_once('#').unwrap_or_else(|| panic!("the link has no fragment: {url}"));
    let token = fragment.strip_prefix("token=").unwrap_or_else(|| panic!("{url}"));
    assert_eq!(token.len(), 64);
    let target = sent.strip_prefix("http://").and_then(|r| r.find('/').map(|i| &r[i..])).unwrap_or_else(|| panic!("{url}"));
    assert_eq!(target, "/", "the request target is the bare page: {url}");
    assert_eq!(sent, format!("http://127.0.0.1:{}/", srv.port), "and the link names the port actually bound, not :0: {url}");

    let (code, page) = get(&mut srv, target, None);
    assert_eq!(code, 200);
    assert!(page.contains("app.js"), "that request loads the viewer");
    assert_eq!(get(&mut srv, "/api/variants", Some(token)).0, 200, "and the token works as a header");
    assert_eq!(out.matches(token).count(), 1, "the minted token is printed once, in that link:\n{out}");
    drop(srv);
    let _ = std::fs::remove_dir_all(d);
}
