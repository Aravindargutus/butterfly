// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `--allow-remote` used to hand every client that could reach the port the
//! complete source tree *and* the ability to rewrite or delete signed-off
//! resolutions. The mode is now decided once and enforced per route (#3).

use butterfly_view::access::{generate_token, Access, Denied};

fn bearer(t: &str) -> String {
    format!("Bearer {t}")
}

#[test]
fn loopback_is_unchanged_reads_and_writes_with_no_credentials() {
    let a = Access::decide(true, false, None);
    assert!(!a.read_only);
    assert!(!a.requires_token());
    assert_eq!(a.may_read(None), Ok(()));
    assert_eq!(a.may_write(None), Ok(()), "the SSH-tunnel workflow must stay convenient");
    assert_eq!(a.because, "a loopback listener");
}

#[test]
fn a_remote_listener_is_read_only_until_the_operator_says_how_writes_are_authenticated() {
    let a = Access::decide(false, false, None);
    assert!(a.read_only, "read-only is the default for a non-loopback listener");
    assert_eq!(a.may_read(None), Ok(()), "but it is still a viewer");
    let Err(Denied::Forbidden(msg)) = a.may_write(None) else { panic!("a remote write must be refused") };
    assert!(msg.contains("--token-file"), "and the refusal says how to enable writes: {msg}");
    assert!(msg.contains("SSH tunnel"), "and keeps recommending the tunnel: {msg}");
}

#[test]
fn read_only_refuses_writes_even_on_loopback_and_even_with_the_token() {
    let a = Access::decide(true, true, Some("s3cret"));
    assert!(a.read_only);
    assert_eq!(a.may_read(Some(&bearer("s3cret"))), Ok(()));
    let Err(Denied::Forbidden(msg)) = a.may_write(Some(&bearer("s3cret"))) else { panic!("--read-only must win") };
    assert!(msg.contains("--read-only"), "{msg}");
}

#[test]
fn a_token_gates_every_route_including_reads() {
    let a = Access::decide(false, false, Some("s3cret"));
    assert!(!a.read_only, "supplying a token is how an operator enables remote writes");
    assert!(a.requires_token());
    assert!(matches!(a.may_read(None), Err(Denied::Unauthorized(_))), "the evidence embeds the whole source");
    assert!(matches!(a.may_write(None), Err(Denied::Unauthorized(_))));
    assert_eq!(a.may_read(Some(&bearer("s3cret"))), Ok(()));
    assert_eq!(a.may_write(Some(&bearer("s3cret"))), Ok(()));
}

#[test]
fn a_wrong_token_is_refused_however_close_it_is() {
    let a = Access::decide(false, false, Some("s3cret"));
    for wrong in ["", "s3cre", "s3cretx", "S3CRET", " s3cret", "s3cret "] {
        let r = a.may_write(Some(&bearer(wrong)));
        assert!(matches!(r, Err(Denied::Unauthorized(_))), "{wrong:?} must not pass: {r:?}");
    }
    assert_eq!(a.may_write(Some(&bearer("s3cret"))), Ok(()));
}

#[test]
fn a_malformed_authorization_header_is_refused_not_ignored() {
    let a = Access::decide(false, false, Some("s3cret"));
    for header in ["s3cret", "Basic s3cret", "Bearer", "Bearer ", "Token s3cret"] {
        assert!(matches!(a.may_write(Some(header)), Err(Denied::Unauthorized(_))), "{header:?} must not pass");
    }
    assert_eq!(a.may_write(Some("bearer s3cret")), Ok(()), "the scheme is case-insensitive, as HTTP says");
}

#[test]
fn a_generated_token_is_high_entropy_and_never_repeats() {
    let a = generate_token();
    let b = generate_token();
    assert_eq!(a.len(), 64, "256 bits, hex encoded");
    assert_ne!(a, b);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    let access = Access::decide(false, false, Some(&a));
    assert_eq!(access.may_write(Some(&bearer(&a))), Ok(()));
    assert!(matches!(access.may_write(Some(&bearer(&b))), Err(Denied::Unauthorized(_))));
}

#[test]
fn the_startup_banner_can_explain_every_mode() {
    assert_eq!(Access::decide(true, false, None).because, "a loopback listener");
    assert_eq!(Access::decide(false, false, None).because, "a non-loopback listener with no token");
    // not "--token": the token may have come from a file or the environment
    assert_eq!(Access::decide(false, false, Some("t")).because, "a bearer token");
    assert_eq!(Access::decide(true, false, Some("t")).because, "a bearer token", "a token is the reason on loopback too");
    assert_eq!(Access::decide(true, true, None).because, "--read-only");
}

// ---- the listener and the token's sources (#65) ---------------------------

use butterfly_view::access::{listener, resolve_token, TokenSources};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const SECRET: &str = "a-secret-the-operator-chose-0123";

fn scratch(tag: &str) -> PathBuf {
    // a fresh, unpredictable directory, not a name another user could
    // plant a symlink at (#344)
    tempfile::Builder::new().prefix(&format!("butterfly-token-{tag}-")).tempdir().unwrap().keep()
}

/// A token file as an operator would write one: private, with the newline an
/// editor or `echo` leaves.
fn token_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    p
}

#[test]
fn loopback_needs_no_acknowledgement() {
    for l in ["127.0.0.1:8787", "localhost:8787", "[::1]:8787", "127.0.0.2:1"] {
        let got = listener(l, false, false).unwrap_or_else(|e| panic!("{l} is loopback: {e}"));
        assert!(got.loopback, "{l}");
    }
}

/// The finding: a warning described the plaintext exposure but did not stop
/// it. A network listener now needs the operator to say, in a flag, that they
/// accept plain HTTP -- and the refusal says why and what to do instead.
#[test]
fn a_network_listener_refuses_plain_http_until_it_is_acknowledged() {
    let e = listener("0.0.0.0:9000", false, false).unwrap_err();
    assert!(e.contains("--allow-remote"), "without --allow-remote the refusal names it: {e}");

    let e = listener("0.0.0.0:9000", true, false).unwrap_err();
    assert!(e.contains("--insecure-http"), "names the acknowledgement: {e}");
    assert!(e.contains("plain HTTP") && e.contains("bearer token"), "says why: {e}");
    assert!(e.contains("ssh -N -L 9000:127.0.0.1:9000"), "offers the tunnel on the port in use: {e}");
    assert!(e.contains("reverse proxy"), "and the TLS proxy: {e}");

    let ok = listener("0.0.0.0:9000", true, true).expect("both acknowledgements given");
    assert!(!ok.loopback);
    assert_eq!(ok.host, "0.0.0.0");
    assert!(listener("192.0.2.10:9000", false, true).is_err(), "--insecure-http alone does not open the network");
}

#[test]
fn a_token_file_is_read_with_its_trailing_newline_removed() {
    let d = scratch("file");
    for (name, body) in [("lf", format!("{SECRET}\n")), ("crlf", format!("{SECRET}\r\n")), ("bare", SECRET.to_string())] {
        let p = token_file(&d, name, &body);
        let r = resolve_token(TokenSources { file: Some(&p), ..Default::default() }).unwrap();
        let t = r.token.expect("a token");
        assert_eq!(t.as_str(), SECRET, "{name}");
        assert_eq!(t.bootstrap_url("127.0.0.1:1"), None, "a token from a file is the operator's own and is never printed");
        assert!(r.warnings.is_empty(), "a private file with a good token earns no warning: {:?}", r.warnings);
    }
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn an_empty_or_malformed_token_file_is_refused() {
    let d = scratch("bad");
    for (name, body, why) in [
        ("empty", String::new(), "empty"),
        ("newline", "\n".to_string(), "empty"),
        ("short", "abc\n".to_string(), "shorter than 16"),
        ("space", format!("{SECRET} \n"), "whitespace"),
        ("two-lines", format!("{SECRET}\nsecond\n"), "whitespace"),
    ] {
        let p = token_file(&d, name, &body);
        let e = resolve_token(TokenSources { file: Some(&p), ..Default::default() }).unwrap_err();
        assert!(e.contains(why), "{name}: {e}");
        assert!(!e.contains(SECRET), "{name}: the refusal must not quote the token: {e}");
    }
    let big = token_file(&d, "big", &"x".repeat(5000));
    assert!(resolve_token(TokenSources { file: Some(&big), ..Default::default() }).unwrap_err().contains("larger than"));
    let missing = d.join("nope");
    assert!(resolve_token(TokenSources { file: Some(&missing), ..Default::default() }).unwrap_err().contains("cannot read the token file"));
    let _ = std::fs::remove_dir_all(d);
}

#[cfg(unix)]
#[test]
fn a_token_file_other_users_can_read_is_warned_about() {
    use std::os::unix::fs::PermissionsExt;
    let d = scratch("perm");
    let p = token_file(&d, "t", &format!("{SECRET}\n"));
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    let r = resolve_token(TokenSources { file: Some(&p), ..Default::default() }).unwrap();
    assert_eq!(r.token.map(|t| t.as_str().to_string()).as_deref(), Some(SECRET), "a warning, not a refusal");
    assert!(r.warnings.iter().any(|w| w.contains("0644") && w.contains("chmod 600")), "{:?}", r.warnings);
    assert!(r.warnings.iter().all(|w| !w.contains(SECRET)));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_environment_is_a_token_source() {
    let r = resolve_token(TokenSources { env: Some(OsStr::new(SECRET)), ..Default::default() }).unwrap();
    assert_eq!(r.token.as_ref().map(|t| t.as_str()), Some(SECRET));
    assert!(r.warnings.is_empty());
    let e = resolve_token(TokenSources { env: Some(OsStr::new("")), ..Default::default() }).unwrap_err();
    assert!(e.contains("set but empty"), "an empty variable is not 'no token': {e}");
}

/// A literal secret on the command line is in `ps` for every local user and
/// in shell history. It keeps working -- scripts use it -- but says so, and
/// the saying does not repeat the secret.
#[test]
fn a_literal_token_is_deprecated_but_still_works() {
    let r = resolve_token(TokenSources { literal: Some(SECRET), ..Default::default() }).unwrap();
    assert_eq!(r.token.as_ref().map(|t| t.as_str()), Some(SECRET));
    let w = r.warnings.join("\n");
    assert!(w.contains("deprecated") && w.contains("--token-file") && w.contains("BUTTERFLY_TOKEN"), "{w}");
    assert!(!w.contains(SECRET), "{w}");

    let g = resolve_token(TokenSources { literal: Some("generate"), ..Default::default() }).unwrap();
    assert!(g.warnings.is_empty(), "`generate` is not a secret on the command line");
    assert!(g.token.and_then(|t| t.bootstrap_url("127.0.0.1:1")).is_some(), "and a minted token is printed");
}

/// There is no precedence to remember: two sources is an error, naming both.
#[test]
fn two_token_sources_are_refused_whichever_they_are() {
    let d = scratch("conflict");
    let p = token_file(&d, "t", &format!("{SECRET}\n"));
    let env = OsStr::new(SECRET);
    let cases = [
        (TokenSources { literal: Some(SECRET), file: Some(&p), env: None }, ["--token", "--token-file"]),
        (TokenSources { literal: None, file: Some(&p), env: Some(env) }, ["--token-file", "BUTTERFLY_TOKEN"]),
        (TokenSources { literal: Some(SECRET), file: None, env: Some(env) }, ["--token", "BUTTERFLY_TOKEN"]),
        (TokenSources { literal: Some("generate"), file: None, env: Some(env) }, ["--token generate", "BUTTERFLY_TOKEN"]),
    ];
    for (src, names) in cases {
        let e = resolve_token(src).unwrap_err();
        for n in names {
            assert!(e.contains(n), "the refusal names {n}: {e}");
        }
        assert!(!e.contains(SECRET), "{e}");
    }
    let _ = std::fs::remove_dir_all(d);
}

// ---- review of #65 --------------------------------------------------------

use butterfly_view::access::listener_with;
use std::net::SocketAddr;

/// A resolver that knows one name, and resolves addresses as themselves.
fn resolver(name: &'static str, to: &'static [&'static str]) -> impl Fn(&str) -> std::io::Result<Vec<SocketAddr>> {
    move |listen: &str| {
        let (host, port) = listen.rsplit_once(':').unwrap();
        let port: u16 = port.parse().unwrap();
        if host == name {
            Ok(to.iter().map(|a| SocketAddr::new(a.parse().unwrap(), port)).collect())
        } else {
            butterfly_view::access::resolve_listen(listen)
        }
    }
}

/// The finding: loopback was decided from the spelling of --listen, so a name
/// that starts with `127.` but resolves to 0.0.0.0 was served as loopback --
/// no --allow-remote, no --insecure-http, no token, writes open.
#[test]
fn a_name_that_looks_like_loopback_but_resolves_elsewhere_is_not_loopback() {
    let r = resolver("127.x.0.0.0.0.example", &["0.0.0.0"]);
    let e = listener_with("127.x.0.0.0.0.example:0", false, false, &r).unwrap_err();
    assert!(e.contains("--allow-remote"), "it needs the acknowledgements of any network listener: {e}");
    let l = listener_with("127.x.0.0.0.0.example:0", true, true, &r).unwrap();
    assert!(!l.loopback);

    // and one loopback address among others is not enough: the bind may use any
    let mixed = resolver("localhost", &["127.0.0.1", "192.0.2.7"]);
    assert!(!listener_with("localhost:1", true, true, &mixed).unwrap().loopback);
    let bad = |_: &str| Err(std::io::Error::other("no such host"));
    assert!(listener_with("nowhere.invalid:1", true, true, bad).unwrap_err().contains("cannot resolve"));
}

#[test]
fn real_loopback_names_stay_loopback() {
    for l in ["localhost:0", "127.0.0.1:0", "127.0.0.2:0", "[::1]:0"] {
        assert!(listener(l, false, false).unwrap_or_else(|e| panic!("{l}: {e}")).loopback, "{l}");
    }
}

#[test]
fn a_token_is_never_in_its_own_debug_output() {
    use butterfly_view::access::Token;
    for t in [Token::supplied(SECRET.into()), Token::generated(SECRET.into())] {
        let d = format!("{t:?} {:?}", Some(&t));
        assert!(!d.contains(SECRET), "{d}");
        assert!(d.contains("redacted"), "{d}");
    }
}

/// A FIFO used to hang the open until something wrote to it.
#[cfg(unix)]
#[test]
fn a_token_file_that_is_not_a_regular_file_is_refused_without_blocking() {
    let d = scratch("fifo");
    let fifo = d.join("fifo");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
    let (tx, rx) = std::sync::mpsc::channel();
    let f2 = fifo.clone();
    std::thread::spawn(move || {
        let _ = tx.send(resolve_token(TokenSources { file: Some(&f2), ..Default::default() }).map(|_| ()));
    });
    let r = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("reading a FIFO as a token file blocked");
    assert!(r.unwrap_err().contains("not a regular file"));
    let e = resolve_token(TokenSources { file: Some(&d), ..Default::default() }).unwrap_err();
    assert!(e.contains("not a regular file") || e.contains("cannot read"), "a directory: {e}");
    let _ = std::fs::remove_dir_all(d);
}
