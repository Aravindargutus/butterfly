// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `/api/requirements` over real HTTP (#56).
//!
//! The universe is thousands of entries in a real programme and the source
//! pane needs the few on one function, so this route answers by ID and the
//! viewer never loads the file. These tests hold that shape.
//!
//! Runs only when examples/c-per-test has been built; skipped otherwise.

mod common;

use common::{get, try_get, Server};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

/// Build evidence and, unless `bare`, the requirement sidecar beside it.
fn serve(tag: &str, bare: bool) -> Option<Server> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-per-test");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c-per-test/build.sh first");
        return None;
    }
    let d = butterfly_testsupport::scratch_dir(&format!("reqserve-{tag}"));
    let bin = env!("CARGO_BIN_EXE_butterfly");

    let ok = Command::new(bin)
        .args([
            "ingest",
            root.join("build").to_str().unwrap(),
            "--source-root",
            root.to_str().unwrap(),
            "--tests",
            root.join("build/tests").to_str().unwrap(),
            "-o",
            d.join("evidence.json").to_str().unwrap(),
        ])
        .output()
        .expect("ingest");
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    if !bare {
        let ok =
            Command::new(bin).args(["requirements", root.join("llr.yaml").to_str().unwrap(), "-o", d.join("requirements.json").to_str().unwrap()]).output().expect("requirements");
        assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    }

    // On a port the server chooses, and ready only once it has answered a
    // request rather than when a TCP connect succeeds. A port picked here by
    // binding and releasing a socket could be answered by a listener another
    // test's child had inherited, while this server died on `Address already
    // in use` -- see `common` (#91).
    Some(common::serve(Command::new(bin).args(["serve", d.join("evidence.json").to_str().unwrap()])))
}

/// The statement comes back for the IDs asked for, and the analysis around it
/// does not: that split is the whole reason the viewer can afford this.
#[test]
fn the_statement_comes_back_and_the_analysis_only_on_request() {
    let Some(mut s) = serve("split", false) else { return };
    let (code, body) = get(&mut s, "/api/requirements?ids=LLR-LIM-001");
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["entries"][0]["id"], "LLR-LIM-001");
    assert!(v["entries"][0]["text"].as_str().unwrap().contains("altitude"), "{body}");
    assert!(v["entries"][0]["detail"].is_null(), "the analysis is not in the inline half: {body}");
    assert!(v["entries"][0]["claim"].is_null(), "nor the claim: {body}");

    let (_, full) = get(&mut s, "/api/requirements?ids=LLR-LIM-001&full=1");
    let f: serde_json::Value = serde_json::from_str(&full).expect("json");
    assert!(!f["entries"][0]["detail"].is_null(), "full=1 carries it: {full}");
    assert!(full.len() > body.len(), "and is the larger half");
}

/// Only what was asked for. A route that quietly returned more would defeat
/// the reason it exists.
#[test]
fn only_the_requested_ids_come_back_and_unknown_ones_are_named() {
    let Some(mut s) = serve("subset", false) else { return };
    let (_, body) = get(&mut s, "/api/requirements?ids=LLR-LIM-002,LLR-NOT-A-THING");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    let ids: Vec<&str> = v["entries"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["LLR-LIM-002"], "{body}");
    assert_eq!(v["unknown"][0], "LLR-NOT-A-THING", "a missing one is said, not left blank: {body}");
}

/// A request for the universe is refused. The cap is what keeps this route
/// from becoming the bulk download it was built to avoid.
#[test]
fn a_request_for_too_many_ids_is_refused() {
    let Some(mut s) = serve("cap", false) else { return };
    let ids = (0..300).map(|i| format!("LLR-{i}")).collect::<Vec<_>>().join(",");
    let (code, body) = get(&mut s, &format!("/api/requirements?ids={ids}"));
    assert_eq!(code, 400, "{body}");
}

/// No sidecar is not an error. Text is a reading aid; its absence means a tag
/// shows its ID alone and nothing else changes.
#[test]
fn without_a_sidecar_the_route_says_so_rather_than_failing() {
    let Some(mut s) = serve("bare", true) else { return };
    let (code, body) = get(&mut s, "/api/requirements?ids=LLR-LIM-001");
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["present"], false, "{body}");
    assert!(v["entries"].as_array().unwrap().is_empty());
    // and the evidence is served exactly as before
    let (code, _) = get(&mut s, "/api/evidence");
    assert_eq!(code, 200);
}

/// Two variants, the same requirement ID, different text.
///
/// The server serves a sidecar per evidence file, so `LLR-1` may say
/// different things in each. The route must answer per variant -- and the
/// viewer must not carry one variant's statement over to the other, which is
/// why its cache is cleared on switch and in-flight replies are dropped
/// (review of #56).
#[test]
fn each_variant_answers_with_its_own_text_for_the_same_id() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-per-test");
    if !root.join("build/coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run examples/c-per-test/build.sh first");
        return;
    }
    let d = butterfly_testsupport::scratch_dir("reqvariants");
    let bin = env!("CARGO_BIN_EXE_butterfly");

    let base = d.join("base.json");
    let ok =
        Command::new(bin).args(["ingest", root.join("build").to_str().unwrap(), "--source-root", root.to_str().unwrap(), "-o", base.to_str().unwrap()]).output().expect("ingest");
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));

    // the variant name is the file stem with `evidence-` stripped
    for (name, text) in [("alpha", "the alpha statement"), ("beta", "the beta statement")] {
        std::fs::copy(&base, d.join(format!("evidence-{name}.json"))).unwrap();
        // a sidecar named for that evidence wins over a shared one
        let doc = serde_json::json!({
            "schema": "butterfly-requirements/1",
            "generator": "test",
            "sources": [{ "path": format!("{name}.yaml"), "sha256": "0".repeat(64), "count": 1 }],
            "entries": [{ "id": "LLR-SAME", "text": text, "source": 0 }],
        });
        std::fs::write(d.join(format!("evidence-{name}.requirements.json")), serde_json::to_string(&doc).unwrap()).unwrap();
    }

    // As in `serve`: wait for an answered request, not a TCP connect, and
    // treat never answering as the failure it is. This loop previously fell
    // through on timeout and issued the requests anyway, so a server that
    // never came up produced a confusing assertion failure rather than a
    // clear one (#91).
    let mut srv = common::serve(Command::new(bin).args(["serve", d.join("evidence-alpha.json").to_str().unwrap(), d.join("evidence-beta.json").to_str().unwrap()]));

    for (name, want) in [("alpha", "the alpha statement"), ("beta", "the beta statement")] {
        let (code, body) = get(&mut srv, &format!("/api/requirements?v={name}&ids=LLR-SAME"));
        assert_eq!(code, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert_eq!(v["entries"][0]["text"], want, "variant `{name}` must answer with its own text: {body}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// A server that answers one request with exactly these bytes and closes.
/// Needs no fixture, so these run on every host.
fn stub(response: &'static [u8]) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut c, _)) = l.accept() {
            // Read the whole request before answering, and drain to EOF
            // before closing. Closing a socket that still holds unread
            // bytes makes the kernel send RST instead of FIN, and the peer
            // then loses the response that was already written -- which is
            // how a single `read` here produced spurious ConnectionResets
            // in the very tests meant to check for truncation.
            let mut req = Vec::new();
            let mut buf = [0u8; 512];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match c.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            let _ = c.write_all(response);
            let _ = c.flush();
            let _ = c.shutdown(std::net::Shutdown::Write);
            let _ = c.read_to_end(&mut req);
        }
    });
    port
}

/// The finding: a response promising more body than it sends, then closing
/// cleanly, was read as a successful 200. `read_to_end` succeeds on a clean
/// close whether or not the promised body arrived, so the declared framing
/// is the only thing that can tell them apart (review of #91).
#[test]
fn a_body_shorter_than_its_content_length_is_an_error() {
    let port = stub(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}");
    let e = try_get(port, "/").expect_err("a truncated body must not read as success");
    let m = e.to_string();
    assert!(m.contains("Content-Length promised 100"), "{m}");
    assert!(m.contains("2 byte(s)"), "it must say what actually arrived: {m}");
}

/// A status line with no header terminator was accepted, with the body
/// reported as empty rather than as missing.
#[test]
fn headers_that_never_terminate_are_an_error() {
    let port = stub(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n");
    let e = try_get(port, "/").expect_err("unterminated headers must not read as success");
    assert!(e.to_string().contains("headers never terminated"), "{e}");
}

/// The reviewer's first case: a chunk that promises more than it delivers,
/// whose data happens to end in the terminator's bytes. A suffix check reads
/// that as complete; walking the chunks does not.
#[test]
fn a_chunk_shorter_than_it_promised_is_an_error_even_ending_in_the_terminator() {
    // 0x64 = 100 bytes promised, a handful delivered, ending in `0\r\n\r\n`
    let port = stub(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n64\r\nshort0\r\n\r\n");
    let e = try_get(port, "/").expect_err("a truncated chunk must not read as success");
    let m = e.to_string();
    assert!(m.contains("promised 100"), "it must say what was promised: {m}");
}

/// The reviewer's second case: a complete body followed by a trailer. The
/// terminal chunk is not the last thing in the response, and refusing it was
/// the same bug from the other side.
#[test]
fn a_complete_chunked_body_with_a_trailer_is_accepted() {
    let port = stub(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\nExpires: soon\r\n\r\n");
    let (code, body) = try_get(port, "/").expect("a trailer does not make a complete body incomplete");
    assert_eq!(code, 200);
    assert_eq!(body, "{}", "the decoded body is the chunk data, without the framing");
}

/// ...but the trailer section has to end. The terminal chunk ends the chunk
/// data, not the message, so a close before the empty line that closes the
/// trailers is a truncation like any other (review of #91).
#[test]
fn a_chunked_body_whose_trailers_never_end_is_an_error() {
    for response in [
        // the terminal chunk, and nothing after it
        &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n"[..],
        // a trailer field cut off mid-line
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\nX-Test: unfinished",
        // a whole trailer field, but not the empty line after it
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\nX-Test: done\r\n",
    ] {
        let port = stub(response);
        let e = try_get(port, "/").expect_err(&format!("unterminated trailers must not read as success: {:?}", String::from_utf8_lossy(response)));
        assert!(e.to_string().contains("trailer section never terminated"), "{e}");
    }
}

/// A chunk whose data is not followed by CRLF is malformed, however its
/// length reads.
#[test]
fn a_chunk_not_followed_by_crlf_is_an_error() {
    let port = stub(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}XX0\r\n\r\n");
    let e = try_get(port, "/").expect_err("a chunk must be followed by CRLF");
    assert!(e.to_string().contains("not followed by CRLF"), "{e}");
}

/// Chunk extensions are legal and carry no length.
#[test]
fn a_chunk_extension_does_not_confuse_the_length() {
    let port = stub(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2;name=value\r\n{}\r\n0\r\n\r\n");
    let (code, body) = try_get(port, "/").expect("an extension is not an error");
    assert_eq!((code, body.as_str()), (200, "{}"));
}

/// A body that simply stops, with no terminal chunk and nothing to read a
/// next chunk size from.
#[test]
fn a_chunked_body_without_its_terminal_chunk_is_an_error() {
    let port = stub(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n");
    let e = try_get(port, "/").expect_err("an unterminated chunked body must not read as success");
    // The walk now says where it ran out, rather than only that the bytes
    // did not end the way a complete body ends.
    assert!(e.to_string().contains("chunk size line is unterminated"), "{e}");
}

/// ...and the checks above accept what is actually complete, so they are
/// rejecting the truncation rather than every response.
#[test]
fn complete_responses_are_accepted_in_each_framing() {
    let exact = stub(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
    assert_eq!(try_get(exact, "/").unwrap(), (200, "{}".to_string()));

    let chunked = stub(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n");
    assert_eq!(try_get(chunked, "/").unwrap().0, 200);

    // neither header: the close itself delimits the body, which is legal
    // and carries no length to check
    let closed = stub(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\nnope");
    assert_eq!(try_get(closed, "/").unwrap(), (404, "nope".to_string()));
}
