// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! What `butterfly serve` writes to stderr when it starts.
//!
//! stderr is where a service manager or a CI runner collects and keeps a
//! process's output, so an operator's own `--token` secret must not be in it:
//! echoing it turns a credential the operator already holds into a durable
//! copy in the logs (#3, review of #24).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;

fn evidence() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build/evidence.json");
    if p.exists() {
        Some(p.canonicalize().unwrap())
    } else {
        butterfly_testsupport::fixture_missing("build examples/c first");
        None
    }
}

/// Start the server, collect the banner, stop it. Loopback and port 0 so the
/// test neither needs a free port nor opens one to the network.
///
/// Read up to the line after the bind: the `butterfly: http://HOST:PORT/`
/// line, or -- for a server with a token -- the "open the viewer" line that
/// follows it, which names the bound port and so is printed after the bind.
fn banner_of(args: &[&str]) -> String {
    let with_token = args.iter().any(|a| a.starts_with("--token"));
    let Some(ev) = evidence() else { return String::new() };
    let mut child = Command::new(env!("CARGO_BIN_EXE_butterfly"))
        .arg("serve")
        .arg(ev)
        .args(["--listen", "127.0.0.1:0"])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn butterfly serve");

    let err = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = String::new();
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            let done = if with_token { line.contains("open the viewer at") } else { line.starts_with("butterfly: http://") };
            seen.push_str(&line);
            seen.push('\n');
            if done {
                break;
            }
        }
        let _ = tx.send(seen);
    });

    let out = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap_or_default();
    let _ = child.kill();
    let _ = child.wait();
    out
}

const SECRET: &str = "operator-chose-this-one";

#[test]
fn a_supplied_token_is_absent_from_the_startup_output() {
    if evidence().is_none() {
        return;
    }
    let out = banner_of(&["--token", SECRET]);
    assert!(!out.is_empty(), "the server printed nothing");
    assert!(!out.contains(SECRET), "the operator's own token was echoed to stderr:\n{out}");
    assert!(out.contains("require a bearer token"), "and it still reports the mode:\n{out}");
    assert!(out.contains("open the viewer at"), "and still says where to go:\n{out}");
}

#[test]
fn a_generated_token_is_printed_because_nothing_else_knows_it() {
    if evidence().is_none() {
        return;
    }
    let out = banner_of(&["--token", "generate"]);
    let url = out.lines().find(|l| l.contains("open the viewer at")).unwrap_or_default();
    // in the fragment, which the browser never sends (#65)
    let token = url.rsplit_once("/#token=").map(|(_, t)| t).unwrap_or_default();
    assert_eq!(token.len(), 64, "a minted token is printed in full so the operator can use it: {url}");
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()), "{url}");
    assert!(!out.contains("?token="), "and never in a query, which is sent to the server:\n{out}");
    assert!(!out.contains("deprecated"), "`generate` is not a secret typed on the command line:\n{out}");
}

#[test]
fn no_token_prints_no_bootstrap_url_at_all() {
    if evidence().is_none() {
        return;
    }
    let out = banner_of(&[]);
    assert!(!out.contains("token="), "there is no token to hand out:\n{out}");
    assert!(out.contains("loopback only"), "{out}");
}

/// Asked for port 0, the kernel chooses the port, and the URL line is the
/// only place anyone can learn which one. It printed `:0`, so every HTTP suite
/// had to pick a port itself -- by binding and releasing a socket, which on
/// macOS a child spawned by another test thread could inherit and keep
/// listening. The server then died on `Address already in use` while that
/// child answered the test's connect, and reset it on exiting: the
/// intermittent `ConnectionReset` of #91 and #99.
#[test]
fn the_url_names_the_port_actually_bound_and_that_port_answers() {
    let Some(ev) = evidence() else { return };
    struct Stop(std::process::Child);
    impl Drop for Stop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Stop(
        Command::new(env!("CARGO_BIN_EXE_butterfly"))
            .arg("serve")
            .arg(ev)
            .args(["--listen", "127.0.0.1:0"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn butterfly serve"),
    );
    let err = child.0.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            if line.starts_with("butterfly: http://") {
                let _ = tx.send(line);
                break;
            }
        }
    });
    let line = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("the server printed its URL");
    let port: u16 = line.trim_end_matches('/').rsplit_once(':').and_then(|(_, p)| p.parse().ok()).unwrap_or_else(|| panic!("no port in {line:?}"));
    assert_ne!(port, 0, "the URL must name the port the kernel chose, not the 0 that asked it to: {line}");
    assert_eq!(line, format!("butterfly: http://127.0.0.1:{port}/"), "and otherwise reads as it always has");

    // and it is this server's port: it answers
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the port it named");
    s.set_read_timeout(Some(std::time::Duration::from_secs(30))).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).expect("a response");
    let head = String::from_utf8_lossy(&raw[..raw.len().min(64)]).into_owned();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
}
