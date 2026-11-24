// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Who may read the evidence, and who may change a review decision.
//!
//! The evidence embeds the complete analysed source, and the resolution
//! endpoints record a reviewer's DO-178C justifications. Deciding both from
//! the bind address alone is too coarse: `--allow-remote` used to hand every
//! client that could reach the port the source tree *and* the ability to
//! rewrite or delete signed-off resolutions (#3).
//!
//! So the decision is made once at startup and enforced per route:
//!
//! * **loopback** — reads and writes, no credentials. Unchanged, and what the
//!   SSH-tunnel recommendation relies on.
//! * **non-loopback** — refused unless the operator passes both
//!   `--allow-remote` and `--insecure-http` (#65), and refused without a
//!   token unless they also pass `--allow-unauthenticated-remote` (#67). Such
//!   an open listener is read-only: exposing a viewer should not also expose
//!   the review record.
//! * **a token** (`--token-file`, `BUTTERFLY_TOKEN`, or the deprecated
//!   `--token`) — required on every API route, reads included, whatever the
//!   bind address.
//! * **`--read-only`** — no writes at all, however authenticated.

use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::path::Path;

/// What this server permits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    /// Mutation routes are refused outright.
    pub read_only: bool,
    /// SHA-256 of the required bearer token, when one is required. The digest
    /// rather than the token so a comparison cannot leak it by length, and so
    /// the token is not sitting in the process's state.
    token_digest: Option<[u8; 32]>,
    /// Why the mode is what it is, for the startup banner.
    pub because: &'static str,
}

/// Why a request was refused, as the status and body to return.
#[derive(Debug, PartialEq, Eq)]
pub enum Denied {
    /// No credential, or the wrong one.
    Unauthorized(String),
    /// Correct credentials would not help.
    Forbidden(String),
}

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

impl Access {
    /// Decide the mode from the bind address and the operator's flags.
    pub fn decide(loopback: bool, read_only_flag: bool, token: Option<&str>) -> Access {
        let token_digest = token.map(digest);
        // A viewer put on the network is read-only until the operator says
        // how writes will be authenticated.
        let remote_default_read_only = !loopback && token_digest.is_none();
        Access {
            read_only: read_only_flag || remote_default_read_only,
            token_digest,
            because: if read_only_flag {
                "--read-only"
            } else if remote_default_read_only {
                "a non-loopback listener with no token"
            } else if token_digest.is_some() {
                // not "--token": it may have come from a file or the environment
                "a bearer token"
            } else {
                "a loopback listener"
            },
        }
    }

    pub fn requires_token(&self) -> bool {
        self.token_digest.is_some()
    }

    /// May this request read? `auth` is the `Authorization` header, if any.
    pub fn may_read(&self, auth: Option<&str>) -> Result<(), Denied> {
        self.check_token(auth)
    }

    /// May this request change a resolution?
    pub fn may_write(&self, auth: Option<&str>) -> Result<(), Denied> {
        if self.read_only {
            return Err(Denied::Forbidden(format!(
                "butterfly is serving read-only ({}). Resolutions cannot be created, changed or deleted through this server.\n\
                 To record review decisions remotely, restart with --token-file <path> (or BUTTERFLY_TOKEN set) and enter the token in the viewer.\n\
                 The recommended alternative is to serve on loopback and reach it over an SSH tunnel.\n",
                self.because
            )));
        }
        self.check_token(auth)
    }

    fn check_token(&self, auth: Option<&str>) -> Result<(), Denied> {
        let Some(want) = self.token_digest else { return Ok(()) };
        let got = auth
            // exact bytes after the scheme: hyper has already stripped the
            // field's surrounding whitespace, so anything left is the token
            .and_then(|a| a.strip_prefix("Bearer ").or_else(|| a.strip_prefix("bearer ")))
            .filter(|t| !t.is_empty());
        match got {
            // digests are fixed width, and the comparison does not stop early
            Some(t) if constant_time_eq(&digest(t), &want) => Ok(()),
            Some(_) => Err(Denied::Unauthorized("butterfly: the bearer token is not the one this server was started with.\n".into())),
            None => Err(Denied::Unauthorized(
                "butterfly requires a bearer token on this route.\nSend `Authorization: Bearer <secret>`, or open the viewer and enter the token when it asks, and it will do so for you.\n"
                    .into(),
            )),
        }
    }
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A bearer token and where it came from.
///
/// The provenance has to survive as far as the startup banner. butterfly
/// mints a token for `--token generate` and has to print it -- nobody else
/// knows it -- but a token the operator supplied is already theirs, and
/// echoing it buys nothing while turning stderr into a durable copy of the
/// credential wherever a service or CI job collects logs (review of #24).
#[derive(Clone, PartialEq, Eq)]
pub struct Token {
    value: String,
    generated: bool,
}

/// Never the value: anything that derives `Debug` over a `Token` --
/// `ServeOptions`, an error, a `dbg!` left in -- would otherwise print the
/// credential (review of #65).
impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.generated { "Token(<generated, redacted>)" } else { "Token(<redacted>)" })
    }
}

impl Token {
    /// A token butterfly minted. Safe to print: it is the only way the
    /// operator can learn it.
    pub fn generated(value: String) -> Token {
        Token { value, generated: true }
    }

    /// A token the operator supplied. Never printed.
    pub fn supplied(value: String) -> Token {
        Token { value, generated: false }
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// The bootstrap URL to print, or `None` when printing it would echo a
    /// secret the operator already has.
    ///
    /// The token rides in the *fragment*, never the query. A browser does not
    /// send anything after `#` to the server, so the token is not in the
    /// request line, in a proxy's access log, or in a `Referer`; the viewer
    /// lifts it into the tab's session storage and clears the fragment before
    /// it makes a single request. `?token=` sent the credential in clear on
    /// the very first request, before any script could move it (#65).
    pub fn bootstrap_url(&self, listen: &str) -> Option<String> {
        self.generated.then(|| format!("http://{listen}/#token={}", self.value))
    }
}

/// The environment variable a token may be supplied in.
pub const TOKEN_ENV: &str = "BUTTERFLY_TOKEN";

/// The largest token file butterfly will read. A token is a line; anything
/// bigger is the wrong file, and `/dev/zero` must not be read forever.
const MAX_TOKEN_FILE: u64 = 4096;

/// Everywhere a token can come from, as the operator gave them.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokenSources<'a> {
    /// `--token <value>`: a literal secret (deprecated), or `generate`.
    pub literal: Option<&'a str>,
    /// `--token-file <path>`.
    pub file: Option<&'a Path>,
    /// The value of `BUTTERFLY_TOKEN`, if it is set at all.
    pub env: Option<&'a OsStr>,
}

/// A token, and what the operator should be told about how it was given.
#[derive(Debug, Default)]
pub struct ResolvedToken {
    pub token: Option<Token>,
    /// Warnings for stderr. None of them contains the token.
    pub warnings: Vec<String>,
}

/// Decide the token from its sources.
///
/// There is no precedence. Two sources at once is refused, whichever they
/// are: a `BUTTERFLY_TOKEN` left exported in a shell would otherwise quietly
/// win over, or lose to, the file the operator just named, and a server would
/// come up protected by a credential nobody meant it to have. Refusing costs
/// one `unset`.
///
/// A literal `--token <secret>` still works but is deprecated: it is in the
/// process list for every local user and in the shell's history (#65).
/// `--token generate` is not a secret the operator typed and is not
/// deprecated.
///
/// No message returned from here contains the token.
pub fn resolve_token(src: TokenSources) -> Result<ResolvedToken, String> {
    let given: Vec<&str> =
        [src.literal.map(|_| if src.literal == Some("generate") { "--token generate" } else { "--token" }), src.file.map(|_| "--token-file"), src.env.map(|_| TOKEN_ENV)]
            .into_iter()
            .flatten()
            .collect();
    if given.len() > 1 {
        return Err(format!(
            "the bearer token is given more than once: by {}.\nbutterfly will not pick one, because a token that silently shadows another protects the server with a credential nobody chose. Give exactly one{}.",
            given.join(" and "),
            if src.env.is_some() { format!(" (`unset {TOKEN_ENV}` if it was left over in this shell)") } else { String::new() }
        ));
    }
    let mut out = ResolvedToken::default();
    if let Some(lit) = src.literal {
        if lit == "generate" {
            out.token = Some(Token::generated(generate_token()));
            return Ok(out);
        }
        validate(lit, "--token")?;
        out.warnings.push(format!(
            "butterfly: --token <secret> is deprecated: a secret on the command line is visible to every local user in the process list and is kept in shell history. \
             Put it in a file only you can read and pass --token-file <path>, or set {TOKEN_ENV}."
        ));
        out.token = Some(Token::supplied(lit.to_string()));
    } else if let Some(path) = src.file {
        let (value, warning) = read_token_file(path)?;
        out.warnings.extend(warning);
        out.token = Some(Token::supplied(value));
    } else if let Some(env) = src.env {
        let value = env.to_str().ok_or_else(|| format!("{TOKEN_ENV} is not valid UTF-8"))?;
        if value.is_empty() {
            // Set-but-empty is almost always a mistake (`export BUTTERFLY_TOKEN=$(cat missing)`),
            // and reading it as "no token" would serve without one.
            return Err(format!("{TOKEN_ENV} is set but empty. Unset it to serve without a token, or give it the token."));
        }
        validate(value, TOKEN_ENV)?;
        out.token = Some(Token::supplied(value.to_string()));
    }
    Ok(out)
}

/// A token has to survive the trip into an `Authorization` header unchanged:
/// long enough to resist guessing, and nothing a browser or proxy would alter
/// or refuse. Whitespace in particular would be trimmed off the header, so a
/// token with a trailing space could never be matched.
fn validate(t: &str, from: &str) -> Result<(), String> {
    if t.is_empty() {
        return Err(format!("the token from {from} is empty"));
    }
    if t.chars().count() < 16 {
        return Err(format!("the token from {from} is shorter than 16 characters; use --token generate to mint one"));
    }
    if !t.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(format!("the token from {from} contains whitespace, control or non-ASCII characters, which an HTTP header cannot carry unchanged"));
    }
    Ok(())
}

/// Read a token from a file: the file's first and only line.
///
/// The trailing newline an editor or `echo` adds is removed, and nothing
/// else: a token is compared byte for byte, so butterfly does not guess at
/// what else might be decoration. On Unix a file that group or others have
/// any access to earns a warning -- that is the exposure `--token-file`
/// exists to avoid -- but not a refusal, because a secrets mount may be mode
/// 0440 on purpose.
///
/// Only a regular file is read. A FIFO would block the open until something
/// wrote to it, and a device may never end, so the open is non-blocking and
/// anything but a regular file is refused (review of #65).
fn read_token_file(path: &Path) -> Result<(String, Option<String>), String> {
    use std::io::Read;
    let cannot = |e: std::io::Error| format!("cannot read the token file {}: {e}", path.display());
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NONBLOCK);
    }
    let f = open.open(path).map_err(cannot)?;
    let meta = f.metadata().map_err(cannot)?;
    if !meta.is_file() {
        return Err(format!("the token file {} is not a regular file; --token-file needs a file that holds the token", path.display()));
    }
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut warning = None;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            warning = Some(format!(
                "butterfly: WARNING — the token file {} is accessible to other users (mode {mode:04o}); `chmod 600 {}` so the token is yours alone.",
                path.display(),
                path.display()
            ));
        }
    }
    let mut buf = Vec::new();
    f.take(MAX_TOKEN_FILE + 1).read_to_end(&mut buf).map_err(|e| format!("cannot read the token file {}: {e}", path.display()))?;
    if buf.len() as u64 > MAX_TOKEN_FILE {
        return Err(format!("the token file {} is larger than {MAX_TOKEN_FILE} bytes; it should hold one line, the token", path.display()));
    }
    let text = String::from_utf8(buf).map_err(|_| format!("the token file {} is not UTF-8 text", path.display()))?;
    let value = text.trim_end_matches(['\n', '\r']);
    if value.is_empty() {
        return Err(format!("the token file {} is empty", path.display()));
    }
    validate(value, &format!("the token file {}", path.display()))?;
    Ok((value.to_string(), warning))
}

/// What the bind address means, once the operator's flags have allowed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub loopback: bool,
    /// The host part of `--listen`, brackets removed and lowercased.
    pub host: String,
}

/// May butterfly listen on `listen` at all?
///
/// Loopback always. Anything else needs two separate acknowledgements, because
/// they are two separate exposures: `--allow-remote` that the evidence (the
/// complete source text) is reachable from the network, and `--insecure-http`
/// that it and any bearer token cross that network in clear. The built-in
/// server has no TLS, and a startup warning did not stop the credential being
/// captured -- it only described it (#65).
pub fn listener(listen: &str, allow_remote: bool, insecure_http: bool) -> Result<Listener, String> {
    listener_with(listen, allow_remote, insecure_http, resolve_listen)
}

/// What `--listen` resolves to, the same way the bind will resolve it.
pub fn resolve_listen(listen: &str) -> std::io::Result<Vec<std::net::SocketAddr>> {
    use std::net::ToSocketAddrs;
    Ok(listen.to_socket_addrs()?.collect())
}

/// [`listener`], with name resolution passed in, so a test can say what a
/// name resolves to without depending on anyone's DNS.
///
/// Loopback is a property of the addresses, not of how they are spelled.
/// Deciding it from the text let `127.x.0.0.0.0.nip.io` -- which starts with
/// `127.` and resolves to 0.0.0.0 -- through as loopback: no --allow-remote,
/// no --insecure-http, no token, and writes open to the network (review of
/// #65). So the name is resolved, and it is loopback only if every address
/// it resolves to is: the bind may use any of them.
pub fn listener_with(listen: &str, allow_remote: bool, insecure_http: bool, resolve: impl Fn(&str) -> std::io::Result<Vec<std::net::SocketAddr>>) -> Result<Listener, String> {
    let bind_host = listen.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or_else(|| listen.to_string());
    let host = bind_host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    let addrs = resolve(listen).map_err(|e| format!("cannot resolve --listen {listen}: {e}"))?;
    if addrs.is_empty() {
        return Err(format!("--listen {listen} resolves to no address"));
    }
    let loopback = addrs.iter().all(|a| a.ip().is_loopback());
    let port = listen.rsplit_once(':').map(|(_, p)| p).filter(|p| !p.is_empty() && *p != "0").unwrap_or("8787");
    if !loopback && !allow_remote {
        return Err(format!(
            "refusing to bind {listen}: that address is reachable from the network, and the evidence contains the full source text.\n\
             Serve on loopback and use an SSH tunnel:\n\n  ssh -N -L {port}:127.0.0.1:{port} <host>\n\n\
             To expose it anyway, pass --allow-remote --insecure-http (see --help for what the second one acknowledges)."
        ));
    }
    if !loopback && !insecure_http {
        return Err(format!(
            "refusing to serve {listen} over plain HTTP.\n\
             butterfly's built-in server has no TLS. Off loopback, the evidence -- which embeds the complete source text -- and any bearer token cross the network in clear: \
             anyone on the path can read both, and replay the token to change review decisions.\n\n\
             Instead, either:\n\
             \x20 * keep butterfly on loopback (--listen 127.0.0.1:{port}) and forward the port from your own machine:\n\
             \x20     ssh -N -L {port}:127.0.0.1:{port} <host>\n\
             \x20 * or put a TLS-terminating reverse proxy in front of it. On the same host, keep butterfly on loopback and no flag is needed;\n\
             \x20   on another host, pass --insecure-http to acknowledge the plain-HTTP hop between the proxy and butterfly."
        ));
    }
    Ok(Listener { loopback, host })
}

/// A token with 256 bits of entropy from the OS, for an operator who asked
/// for remote writes without choosing one.
pub fn generate_token() -> String {
    let mut buf = [0u8; 32];
    // getrandom(2) via /dev/urandom: no extra dependency for one call
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut buf).is_ok() {
            return buf.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    // never silently fall back to something guessable
    panic!("butterfly: cannot read /dev/urandom to generate an access token; pass --token-file <path> instead");
}

/// Must this listener authenticate its readers?
///
/// Off loopback, yes -- read-only included -- unless the operator names the
/// exception (#67). An unauthenticated network listener hands anyone who can
/// reach it the complete source text, and lets them keep the server busy
/// rendering reports and hashing sources for as long as they like. That can
/// be what an operator wants on a network they trust, so it stays possible,
/// but as a flag that says what it does rather than as the default.
pub fn require_credentials(l: &Listener, listen: &str, has_token: bool, allow_unauthenticated_remote: bool) -> Result<(), String> {
    if l.loopback || has_token || allow_unauthenticated_remote {
        return Ok(());
    }
    Err(format!(
        "refusing to serve {listen} without authentication.\n\
         A network listener must require a bearer token, read-only included: without one, anyone who can reach the port reads the complete source text the evidence embeds, \
         and can keep the server busy rendering reports and hashing sources.\n\n\
         Give it a token, from exactly one of:\n\
         \x20 --token-file <path>      one line, mode 600\n\
         \x20 BUTTERFLY_TOKEN=<token>  in the environment\n\
         \x20 --token generate         mints one and prints the link to open\n\n\
         To serve it to anyone who can reach the port anyway, pass --allow-unauthenticated-remote."
    ))
}
