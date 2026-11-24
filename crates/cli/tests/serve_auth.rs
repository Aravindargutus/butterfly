// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! A token-protected server, exercised over real HTTP.
//!
//! `--token` puts a bearer check on every API route, reads included. The
//! viewer therefore has to send the header on *every* request it makes, not
//! just the ones on the first screen: Compare loads the variants the user has
//! not opened yet, and that request went out bare, came back 401, and had the
//! plaintext refusal parsed as JSON (#3, review of #24).

mod common;

use common::Server;
use std::path::PathBuf;
use std::process::Command;

fn evidence() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build/evidence.json");
    if p.exists() {
        Some(p.canonicalize().unwrap())
    } else {
        butterfly_testsupport::fixture_missing("build examples/c first");
        None
    }
}

const TOKEN: &str = "a-token-at-least-sixteen-characters";

/// Two variants of the same evidence, served with a token.
fn serve_two_variants(dir: &PathBuf) -> Option<Server> {
    let ev = evidence()?;
    std::fs::create_dir_all(dir).unwrap();
    // the variant name is the file stem with `evidence-` stripped
    let a = dir.join("evidence-alpha.json");
    let b = dir.join("evidence-beta.json");
    std::fs::copy(&ev, &a).unwrap();
    std::fs::copy(&ev, &b).unwrap();

    // Ready once it has answered `GET /` -- which the token does not guard --
    // on a port of its own choosing, with its stderr kept for a failure to
    // report. The connect this waited for could be answered by a listener
    // another test's child had inherited, while this server died on `Address
    // already in use` -- see `common` (#91, #99).
    Some(common::serve(Command::new(env!("CARGO_BIN_EXE_butterfly")).arg("serve").arg(&a).arg(&b).args(["--token", TOKEN])))
}

/// One GET, bearing `bearer` if given. Returns (status, body); a failed or
/// truncated response fails with the server's own account of itself.
fn get(srv: &mut Server, path: &str, bearer: Option<&str>) -> (u16, String) {
    let auth = bearer.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.as_deref().map(|a| ("Authorization", a)).into_iter().collect();
    common::request(srv, "GET", path, &headers, "")
}

/// Every variant needs the token, not just the one the viewer opens first.
/// `allVariants` fetched the rest without it, so Compare broke in exactly the
/// mode this PR introduces.
#[test]
fn every_variants_evidence_needs_the_token() {
    let Some(ev) = evidence() else { return };
    let _ = ev;
    let dir = butterfly_testsupport::scratch_dir("auth");
    let Some(mut srv) = serve_two_variants(&dir) else { return };

    for v in ["alpha", "beta"] {
        let path = format!("/api/evidence?v={v}");
        let (status, body) = get(&mut srv, &path, None);
        assert_eq!(status, 401, "{path} without a token must be refused, not served");
        assert!(serde_json::from_str::<serde_json::Value>(&body).is_err(), "the refusal is plain text, which is why it must never be parsed as JSON: {body}");

        let (status, body) = get(&mut srv, &path, Some(TOKEN));
        assert_eq!(status, 200, "{path} with the token must be served: {body}");
        let ev: serde_json::Value = serde_json::from_str(&body).expect("evidence is JSON");
        assert!(ev["functions"].is_array(), "{path} returned something that is not evidence");
    }

    // and the variant listing itself, which is what bootstraps the viewer
    assert_eq!(get(&mut srv, "/api/variants", None).0, 401);
    let (status, body) = get(&mut srv, "/api/variants", Some(TOKEN));
    assert_eq!(status, 200);
    let boot: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(boot["variants"].as_array().map(Vec::len), Some(2), "both variants are listed: {body}");

    drop(srv);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The viewer must not reach the network without the header. Guarding the
/// Rust side proves the server refuses; this proves the browser asks
/// correctly, which no Rust test can observe.
#[test]
fn the_viewer_sends_the_token_on_every_api_request() {
    let js = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/app.js")).unwrap();
    let bare: Vec<(usize, &str)> = js
        .lines()
        .enumerate()
        .filter(|(_, l)| {
            // `fetch(` not preceded by `auth`, on a line that names an API route
            l.contains("/api/") && l.match_indices("fetch(").any(|(i, _)| !l[..i].ends_with("auth"))
        })
        .map(|(n, l)| (n + 1, l.trim()))
        .collect();
    assert!(bare.is_empty(), "these call fetch directly instead of authFetch/authJson, so they carry no bearer token: {bare:#?}");
}
