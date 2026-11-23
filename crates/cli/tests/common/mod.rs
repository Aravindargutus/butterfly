// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! A real `butterfly serve`, and a client for it, for the suites that test
//! over HTTP (#91, #99).
//!
//! **The server chooses its own port.** Every suite used to pick one with
//! `free_port()` -- bind 127.0.0.1:0, read the port, drop the listener -- and
//! hand it to the server. On macOS that listener can outlive the drop: std
//! creates a socket and marks it close-on-exec in two separate calls there
//! (there is no `SOCK_CLOEXEC`), and `posix_spawn` does not close inherited
//! descriptors, so a child spawned by another test thread in between keeps
//! the socket -- still listening -- across its `exec`. The server handed that
//! port then fails to bind (`Address already in use`) and exits; the test's
//! connect succeeds anyway, into the orphan's backlog; and when the child
//! holding it exits, the kernel resets every connection still queued there.
//! That is `ConnectionReset` at `read_to_end`, only when another test is
//! spawning at the same instant, which is why it appeared under full-suite
//! load and never in a focused rerun.
//!
//! No choice of port on the test side can close that window, because any
//! socket the test process creates has it. Passing `--listen 127.0.0.1:0` and
//! reading the port from the server's own URL line leaves nothing to leak.
//!
//! Nothing here treats a reset as success or retries a request: a missing or
//! truncated response is still a failure, reported with the server's own
//! account of itself (#91).
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub struct Server {
    child: Child,
    pub port: u16,
    /// The server's own stderr. Discarding it meant a failed request could
    /// only ever report the client-side symptom -- `ConnectionReset` and
    /// nothing else -- which cannot tell a server that died from an endpoint
    /// that mishandled the close (#91).
    log: PathBuf,
}

impl Server {
    /// What the server has to say for itself: whether it is still running,
    /// and everything it wrote to stderr.
    pub fn diagnose(&mut self) -> String {
        let state = match self.child.try_wait() {
            Ok(Some(st)) => format!("the server had already exited: {st}"),
            Ok(None) => "the server was still running".to_string(),
            Err(e) => format!("could not determine the server's state: {e}"),
        };
        let log = std::fs::read_to_string(&self.log).unwrap_or_else(|e| format!("<stderr unreadable: {e}>"));
        format!("{state}\n--- server stderr ---\n{}", if log.trim().is_empty() { "<empty>" } else { log.trim() })
    }

    /// Everything the server has written to stderr so far, for a test whose
    /// assertion is about what the server said rather than what it served.
    pub fn stderr(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Kill and reap the server. Safe to call more than once; for a caller
    /// whose own cleanup has to wait until the server is gone.
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_file(&self.log);
    }
}

/// A stderr file no other server in this process is writing: cargo runs a
/// suite's tests concurrently, and some start two servers in one test.
pub fn log_path(what: &str) -> PathBuf {
    butterfly_testsupport::scratch_file(what, ".stderr")
}

/// Start `cmd` -- a `butterfly serve` with everything but its listener -- on
/// a port of the server's choosing, and return once it has answered a
/// request. A server that exits or never answers fails here, naming what it
/// said, rather than as a reset on the first request (#91).
pub fn serve(cmd: &mut Command) -> Server {
    serve_on(cmd, "127.0.0.1:0")
}

/// [`serve`] on another listener -- still port 0, so the server chooses, and
/// still reached over loopback. For a suite whose subject is a non-loopback
/// bind (`0.0.0.0:0`).
pub fn serve_on(cmd: &mut Command, listen: &str) -> Server {
    let log = log_path("serve");
    let child =
        cmd.args(["--listen", listen]).stdout(Stdio::null()).stderr(Stdio::from(std::fs::File::create(&log).expect("create server log"))).spawn().expect("spawn butterfly serve");
    let mut srv = Server { child, port: 0, log };
    match wait_for_listener(&mut srv.child, &srv.log, Duration::from_secs(30)) {
        Some(Started::Listening(port)) => srv.port = port,
        Some(Started::Exited(st)) => panic!("serve exited during startup: {st}\n{}", srv.diagnose()),
        None => panic!("serve never said which port it bound\n{}", srv.diagnose()),
    }
    wait_until_answering(&mut srv);
    srv
}

/// How a server's startup ended.
pub enum Started {
    /// It bound this port, in its own words.
    Listening(u16),
    /// It exited first -- every refusal at startup does.
    Exited(ExitStatus),
}

/// Wait until the server has bound its listener and said which port, or has
/// exited; `None` if it did neither in time.
///
/// The port comes from the server's own URL line, which it prints only after
/// the bind succeeds. A successful TCP connect proves only that *something*
/// is listening on a port, and a leaked listener passes that test too.
pub fn wait_for_listener(child: &mut Child, log: &Path, within: Duration) -> Option<Started> {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(st)) => return Some(Started::Exited(st)),
            Ok(None) => {}
            Err(e) => panic!("could not poll serve: {e}"),
        }
        if let Some(port) = std::fs::read_to_string(log).ok().as_deref().and_then(bound_port) {
            return Some(Started::Listening(port));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// The port in `butterfly: http://127.0.0.1:NNNNN/`, once that line is whole.
fn bound_port(stderr: &str) -> Option<u16> {
    let line = stderr.lines().find(|l| l.starts_with("butterfly: http://") && l.ends_with('/'))?;
    let (_, port) = line.trim_end_matches('/').rsplit_once(':')?;
    port.parse().ok().filter(|p| *p != 0)
}

/// Block until the server answers a request, or fail saying why.
///
/// Readiness is a *completed HTTP exchange*, not a TCP connect. `connect`
/// succeeds as soon as the kernel completes the handshake into the listen
/// backlog, which says nothing about the server being ready to answer -- so
/// the first real request could race startup. Probing that way also opened
/// and dropped a connection carrying no request, which the server then had
/// to reap alongside the request under test.
pub fn wait_until_answering(srv: &mut Server) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = "no attempt completed".to_string();
    while Instant::now() < deadline {
        match srv.child.try_wait() {
            Ok(Some(st)) => panic!("serve exited during startup: {st}\n{}", srv.diagnose()),
            Ok(None) => {}
            Err(e) => panic!("could not poll serve: {e}"),
        }
        match try_get(srv.port, "/") {
            Ok(_) => return,
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("serve never answered a request: {last}\n{}", srv.diagnose());
}

/// One GET. Any failure names the server's own account of itself and the
/// bytes that did arrive, instead of a bare errno (#91).
pub fn get(srv: &mut Server, path: &str) -> (u16, String) {
    request(srv, "GET", path, &[], "")
}

/// One request, with extra header lines and a body. Fails as `get` does.
pub fn request(srv: &mut Server, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    let port = srv.port;
    match try_request(port, method, path, headers, body) {
        Ok(r) => r,
        Err(e) => panic!("{method} {path} failed on port {port}: {e}\n{}", srv.diagnose()),
    }
}

/// A GET as a `Result`, so readiness can poll without aborting the test on
/// the attempts that are expected to fail.
pub fn try_get(port: u16, path: &str) -> std::io::Result<(u16, String)> {
    try_request(port, "GET", path, &[], "")
}

/// One request as a `Result`. `Content-Length` is sent whenever there is a
/// body or the method expects one.
///
/// A truncated or missing response is still an error: nothing here treats
/// `ConnectionReset` as success, it is only reported with enough context to
/// tell which side caused it.
pub fn try_request(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> std::io::Result<(u16, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() || method != "GET" {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("Connection: close\r\n\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes())?;
    s.flush()?;
    let mut raw = Vec::new();
    if let Err(e) = s.read_to_end(&mut raw) {
        return Err(std::io::Error::new(e.kind(), format!("{e} -- {} byte(s) arrived first: {:?}", raw.len(), String::from_utf8_lossy(&raw))));
    }
    parse_response(&raw)
}

/// One complete HTTP/1.1 response, or an error saying what is missing.
///
/// `read_to_end` returns `Ok` whenever the peer closes cleanly, which it
/// will do in the middle of a promised body just as readily as at the end
/// of one. Checking only that a status line parses accepted
/// `Content-Length: 100` followed by `{}` and a FIN as a successful 200,
/// so a status-only assertion could pass on a truncated response and the
/// readiness probe was not necessarily a completed exchange (review of #91).
///
/// Framing is checked on bytes, not on the lossy string: a body's length
/// has to be the length the header promised, and a non-UTF-8 byte must not
/// be able to change the count.
pub fn parse_response(raw: &[u8]) -> std::io::Result<(u16, String)> {
    let show = |b: &[u8]| format!("{:?}", String::from_utf8_lossy(b));
    // Headers must be terminated. Without this a status line alone was
    // accepted, with the body silently reported as empty.
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::other(format!("the headers never terminated -- {} byte(s) arrived: {}", raw.len(), show(raw))))?;
    let (head, body) = (&raw[..end], &raw[end + 4..]);
    let head = String::from_utf8_lossy(head).into_owned();
    let mut lines = head.split("\r\n");

    let status_line = lines.next().unwrap_or_default().to_string();
    let status: u16 = status_line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| std::io::Error::other(format!("no status line in {:?}", status_line)))?;

    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))).collect();
    let header = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());

    // Whichever framing the response declared has to be complete. A response
    // that declares neither is delimited by the close itself, which is legal
    // and carries no length to check against.
    if let Some(te) = header("transfer-encoding") {
        if te.eq_ignore_ascii_case("chunked") {
            // Walk the chunks. Looking for `0\r\n\r\n` at the end instead was
            // wrong in both directions: an incomplete chunk whose *data*
            // happens to end in those bytes was accepted, and a complete
            // response carrying a trailer was refused because the terminal
            // chunk is not the last thing in it (review of #91).
            return decode_chunked(body).map(|decoded| (status, String::from_utf8_lossy(&decoded).into_owned()));
        }
    } else if let Some(cl) = header("content-length") {
        let want: usize = cl.parse().map_err(|e| std::io::Error::other(format!("Content-Length {cl:?} is not a length: {e}")))?;
        if body.len() != want {
            return Err(std::io::Error::other(format!("the body is {} byte(s) but Content-Length promised {want} -- the response was cut short: {}", body.len(), show(body))));
        }
    }
    Ok((status, String::from_utf8_lossy(body).into_owned()))
}

/// Decode a chunked body, or say where it stops being well formed.
///
/// Each chunk is a hex length, an optional `;ext` the standard allows and
/// this ignores, CRLF, exactly that many bytes, then CRLF. A zero-length
/// chunk ends the chunk data; trailer fields may follow it, each ended by
/// CRLF, and the message is complete only at the empty line after them.
pub fn decode_chunked(body: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = body;
    loop {
        let line_end =
            rest.windows(2).position(|w| w == b"\r\n").ok_or_else(|| std::io::Error::other(format!("a chunk size line is unterminated after {} byte(s) of body", out.len())))?;
        let head = &rest[..line_end];
        // `;` introduces a chunk extension, which carries no length
        let digits = head.split(|b| *b == b';').next().unwrap_or(head);
        let text = std::str::from_utf8(digits).map_err(|_| std::io::Error::other("a chunk size is not text"))?.trim();
        let size = usize::from_str_radix(text, 16).map_err(|e| std::io::Error::other(format!("chunk size {text:?} is not hexadecimal: {e}")))?;
        rest = &rest[line_end + 2..];

        if size == 0 {
            // A zero-size chunk ends the chunk *data*, not the message: the
            // trailer section follows, and it is complete only at its empty
            // line (RFC 9112 §7.1). Returning here accepted `0\r\n` and
            // `0\r\nX-Test: unfinished` -- a close partway through the
            // trailers -- as a finished exchange (review of #91). Trailers
            // themselves are still no reason to reject a complete body.
            let mut fields = 0;
            loop {
                let end = rest.windows(2).position(|w| w == b"\r\n").ok_or_else(|| {
                    std::io::Error::other(format!(
                        "the trailer section never terminated after {fields} field line(s) -- the response was cut short: {:?}",
                        String::from_utf8_lossy(rest)
                    ))
                })?;
                if end == 0 {
                    return Ok(out);
                }
                fields += 1;
                rest = &rest[end + 2..];
            }
        }
        if rest.len() < size + 2 {
            return Err(std::io::Error::other(format!("a chunk promised {size} byte(s) but only {} remain -- the response was cut short", rest.len().saturating_sub(2))));
        }
        out.extend_from_slice(&rest[..size]);
        if &rest[size..size + 2] != b"\r\n" {
            return Err(std::io::Error::other(format!("a chunk of {size} byte(s) is not followed by CRLF")));
        }
        rest = &rest[size + 2..];
    }
}
