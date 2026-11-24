// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! `butterfly-view`: a read-only HTTP viewer over one or more evidence
//! databases (variants of the same sources, e.g. different optimisation
//! levels).
//!
//! Presentation only. Evidence files are never modified; the only thing the
//! viewer writes is the per-variant resolutions sidecar.

pub mod access;
pub mod freshness;
pub mod report;
pub mod sidecar;

use anyhow::{bail, Context, Result};
use axum::{
    body::Bytes,
    extract::{Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use butterfly_model::{Resolution, Resolutions, RESOLUTIONS_SCHEMA};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{watch, Semaphore};

struct Variant {
    name: String,
    path: PathBuf,
    /// The evidence exactly as read, served verbatim by `/api/evidence`.
    /// Shared bytes rather than a `String` because it is immutable for the
    /// life of the process and is usually the largest thing sent: a response
    /// takes a reference-counted handle to this one buffer instead of a copy
    /// of it, so a slow reader holds no memory of its own (#73).
    evidence_json: Bytes,
    summary: serde_json::Value,
    resolutions: Mutex<Resolutions>,
    resolutions_path: PathBuf,
    /// What the freshness check needs: recorded HEAD, repo, file hashes.
    fresh: freshness::Input,
    /// Resolutions written by this process since it loaded the evidence.
    resolution_edits: std::sync::atomic::AtomicUsize,
    /// The optional definition index from `butterfly tags`, as JSON, when one
    /// sits beside the evidence and matches it. `None` means the viewer
    /// simply offers no jumps -- it is a navigation aid, never evidence.
    /// Shared for the same reason as `evidence_json`.
    tags_json: Option<Bytes>,
    /// Requirement text from `butterfly requirements`, when a sidecar sits
    /// beside the evidence. Parsed rather than kept as JSON: the viewer asks
    /// for a handful of IDs at a time and never the whole universe, so the
    /// server needs to look them up.
    requirements: Option<butterfly_model::reqtext::Requirements>,
    /// Reasons loaded requirement source documents changed or could not be
    /// verified against the baselined sidecar, surfaced in `/api/requirements`,
    /// `/api/freshness`, and `/api/report`.
    requirements_stale: Vec<String>,
    /// The evidence, parsed once at startup and shared. The report, the
    /// gates and review validation all need it as a structure, and parsing
    /// the whole document again on every request is what made /api/report an
    /// amplifier (#67). It is held beside `evidence_json`, which
    /// /api/evidence serves as it is; the parse happened at startup anyway.
    evidence: Arc<butterfly_model::Evidence>,
    /// The rendered report, keyed on the review generation it was built at.
    /// The report is a pure function of the evidence and the requirements
    /// (both fixed for the life of the process) and the resolutions, and
    /// every change to those goes through `update`, which bumps
    /// `resolution_edits` -- so that counter is a complete key and a hit can
    /// never be stale.
    report_cache: Arc<Cached>,
    /// The freshness answer, keyed the same way. Unlike the report it is not
    /// a function of process state -- the working tree and the repository
    /// move underneath it -- so it also expires after `Bounds::fresh_ttl`.
    fresh_cache: Arc<Cached>,
}

/// One cached answer, computed at most once at a time (#67).
///
/// The job that computes an answer stores it itself when it finishes, not
/// the request that started it. A request stops waiting at the job timeout
/// and is told to try again; the job carries on, and the retry is served
/// from what it stored instead of starting the same work again. A render
/// longer than the timeout used to be dropped on the floor each time, so a
/// few retrying clients filled every slot with duplicate renders and a large
/// report never arrived (review of #67). While a job runs, every request for
/// the same key waits on that job, so concurrent misses share one
/// computation and one slot.
#[derive(Default)]
struct Cached {
    state: Mutex<CacheState>,
    /// How many times the answer was actually computed. What the tests
    /// assert on; a counter is cheaper than any way of guessing from timing.
    computed: AtomicUsize,
}

/// The value published to the requests waiting on a computation: `None`
/// until it finishes.
type Outcome = Option<Result<Bytes, String>>;

#[derive(Default)]
struct CacheState {
    /// The latest answer: its key, when it was made, and the bytes.
    done: Option<(usize, Instant, Bytes)>,
    /// The computation in flight, and the key it is for.
    running: Option<(usize, watch::Receiver<Outcome>)>,
}

impl Cached {
    fn lock(&self) -> std::sync::MutexGuard<'_, CacheState> {
        // nothing is ever left half-written under this lock
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The stored answer for `key`, if there is one younger than `ttl`.
    fn hit(&self, key: usize, ttl: Option<Duration>) -> Option<Bytes> {
        let st = self.lock();
        let (k, at, body) = st.done.as_ref()?;
        (*k == key && ttl.is_none_or(|t| at.elapsed() < t)).then(|| body.clone())
    }
}

/// Clears a computation's in-flight mark when its job ends, however it ends:
/// a job that panicked must not leave later requests waiting on a
/// computation that is gone.
struct Finished(Arc<Cached>, usize);

impl Drop for Finished {
    fn drop(&mut self) {
        let mut st = self.0.lock();
        if st.running.as_ref().is_some_and(|(k, _)| *k == self.1) {
            st.running = None;
        }
    }
}

impl Variant {
    fn lock_resolutions(&self) -> std::sync::MutexGuard<'_, Resolutions> {
        self.resolutions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The resolutions and the generation they are at, read together: a
    /// cache key that described a different set of reviews than the body
    /// built from it would serve a stale report as current.
    fn reviews(&self) -> (usize, Resolutions) {
        let r = self.lock_resolutions();
        (self.resolution_edits.load(Ordering::Relaxed), r.clone())
    }
}

/// How much expensive work the server takes on at once, and for how long
/// (#67). Rendering the report, hashing every source for freshness and
/// evaluating the gates are real CPU, disk and subprocess work. Unbounded,
/// one client could queue as much of it as it liked, and while it ran on the
/// async threads it starved the cheap routes too.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    /// Expensive jobs running at once, server-wide. Past this the answer is
    /// 503 with Retry-After, not a queue: a queue turns a burst into memory
    /// and into waiting behind work nobody will read.
    jobs: usize,
    /// How long a request waits for its job before it is answered 503. A
    /// running job cannot be cancelled, so it keeps its slot until it ends:
    /// the cap bounds work being done, not just requests waiting for it.
    job_timeout: Duration,
    /// The longest any request is served for, reading its body included.
    request_timeout: Duration,
    /// How long one freshness answer is reused.
    fresh_ttl: Duration,
}

impl Default for Bounds {
    fn default() -> Bounds {
        Bounds { jobs: 4, job_timeout: Duration::from_secs(30), request_timeout: Duration::from_secs(60), fresh_ttl: Duration::from_secs(5) }
    }
}

/// Says whether an expensive answer came from the cache. An operator looking
/// at a slow viewer can tell which side of the cache they are on, and a test
/// can assert the bound over HTTP instead of guessing from timing.
const X_CACHE: header::HeaderName = header::HeaderName::from_static("x-butterfly-cache");

/// Seconds a client refused for load is told to wait.
const RETRY_AFTER_SECS: &str = "2";

struct AppState {
    variants: Vec<Variant>,
    ui_dir: Option<PathBuf>,
    /// What this server permits, decided once at startup (#3).
    access: access::Access,
    /// Host header values this server answers to. The viewer has no
    /// authentication: a browser on any page the user visits can send
    /// requests to a loopback port, so a request whose `Host` is a name the
    /// attacker controls (DNS rebinding) must be refused before it can read
    /// the evidence — which embeds the full source text.
    hosts: HostPolicy,
    bounds: Bounds,
    /// One permit per expensive job that may run now (`Bounds::jobs`).
    jobs: Arc<Semaphore>,
    /// What the freshness check may read and run git in, decided once at
    /// startup from the operator's flags -- never from the evidence (#248).
    authority: freshness::Authority,
}

/// A refusal for load, with the time to come back. Deterministic: this is
/// the answer whenever the server is at its limit, not a symptom of it
/// falling over.
fn unavailable(why: String) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, [(header::RETRY_AFTER, RETRY_AFTER_SECS), (header::CACHE_CONTROL, "no-store")], why).into_response()
}

/// A JSON body with the headers every JSON route sends: not sniffed as
/// anything else, and never stored -- the answers carry review records and
/// change with every write (#340).
fn json_response(body: impl Into<axum::body::Body>) -> Response {
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")], body.into()).into_response()
}

/// [`json_response`] for a value to serialise. Serialising the structures
/// here cannot fail; if it ever did, the client gets a bare 500.
fn json_value<T: Serialize>(v: &T) -> Response {
    match serde_json::to_vec(v) {
        Ok(b) => json_response(b),
        Err(e) => {
            eprintln!("butterfly: could not serialise a response: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Run `job` on the blocking pool, within the server-wide cap (#67).
///
/// Filesystem, git, parsing and rendering do not belong on the async
/// threads: while they run there, every other request waits. The slot is
/// taken before anything starts and refused at once when there is none, and
/// it moves into the job, so it is released when the work ends -- even if
/// the request stopped waiting for it, and even if the job panics.
async fn run_blocking<T: Send + 'static>(s: &AppState, job: impl FnOnce() -> T + Send + 'static) -> Result<T, Response> {
    let Ok(slot) = s.jobs.clone().try_acquire_owned() else { return Err(at_limit(s)) };
    let task = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        job()
    });
    match tokio::time::timeout(s.bounds.job_timeout, task).await {
        Ok(Ok(v)) => Ok(v),
        // The JoinError's text can carry a panic message, which is for the
        // operator, not the client (#340).
        Ok(Err(e)) => {
            eprintln!("butterfly: a request's job failed: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CACHE_CONTROL, "no-store"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
                "butterfly: the request failed; the server log has the detail.\n",
            )
                .into_response())
        }
        Err(_) => Err(unavailable(format!("butterfly: the request took longer than {:?}; it is finishing in the background. Try again shortly.\n", s.bounds.job_timeout))),
    }
}

/// The refusal when every job slot is taken.
fn at_limit(s: &AppState) -> Response {
    unavailable(format!("butterfly is already running {} expensive requests (report, freshness, gates), which is its limit. Try again shortly.\n", s.bounds.jobs))
}

/// The answer for `key` from `c`: stored, being computed, or computed now.
///
/// A stored answer younger than `ttl` is a hit and needs no slot. A
/// computation already running for the same key is joined. Otherwise a slot
/// is taken (or the request refused at once) and `job` starts on the
/// blocking pool, where it stores its own result -- never over a newer key --
/// before telling the requests waiting on it. A request waits at most
/// `Bounds::job_timeout`; the job does not stop when the request does.
async fn single_flight(
    s: &AppState,
    c: &Arc<Cached>,
    key: usize,
    ttl: Option<Duration>,
    job: impl FnOnce() -> Result<Bytes, String> + Send + 'static,
) -> Result<(Bytes, bool), Response> {
    let mut rx = {
        let mut st = c.lock();
        if let Some((k, at, body)) = &st.done {
            if *k == key && ttl.is_none_or(|t| at.elapsed() < t) {
                return Ok((body.clone(), true));
            }
        }
        let joined = st.running.as_ref().filter(|(k, _)| *k == key).map(|(_, rx)| rx.clone());
        match joined {
            Some(rx) => rx,
            None => {
                let Ok(slot) = s.jobs.clone().try_acquire_owned() else { return Err(at_limit(s)) };
                let (tx, rx) = watch::channel(None);
                st.running = Some((key, rx.clone()));
                let c2 = c.clone();
                tokio::task::spawn_blocking(move || {
                    let _slot = slot;
                    let _finished = Finished(c2.clone(), key);
                    c2.computed.fetch_add(1, Ordering::Relaxed);
                    let out = job();
                    if let Ok(body) = &out {
                        let mut st = c2.lock();
                        // an older key finishing late must not replace a newer answer
                        if st.done.as_ref().is_none_or(|(k, _, _)| *k <= key) {
                            st.done = Some((key, Instant::now(), body.clone()));
                        }
                    }
                    let _ = tx.send(Some(out));
                });
                rx
            }
        }
    };
    // bound to a local so the borrow of `rx` ends before `rx` does
    let answer = match tokio::time::timeout(s.bounds.job_timeout, rx.wait_for(Option::is_some)).await {
        Ok(Ok(v)) => match &*v {
            Some(Ok(body)) => Ok((body.clone(), false)),
            Some(Err(e)) => Err((StatusCode::INTERNAL_SERVER_ERROR, format!("butterfly: {e}\n")).into_response()),
            None => Err((StatusCode::INTERNAL_SERVER_ERROR, "butterfly: the computation ended without an answer\n").into_response()),
        },
        // the sender went away without an answer: the job panicked
        Ok(Err(_)) => Err((StatusCode::INTERNAL_SERVER_ERROR, "butterfly: the computation failed\n").into_response()),
        Err(_) => Err(unavailable(format!(
            "butterfly: the answer is taking longer than {:?}. It is still being computed and will be kept, so the next request gets it: try again shortly.\n",
            s.bounds.job_timeout
        ))),
    };
    answer
}

/// No request is served for longer than `Bounds::request_timeout`, reading
/// its body included: a client that trickles a POST body, or a request
/// queued behind a busy cache, is answered instead of held open. Headers are
/// read before this runs; bounding a client that trickles those is the job
/// of the reverse proxy a network deployment should have in front (#67).
async fn limit_time(State(s): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    match tokio::time::timeout(s.bounds.request_timeout, next.run(req)).await {
        Ok(r) => r,
        Err(_) => unavailable(format!("butterfly: the request took longer than {:?} and was abandoned.\n", s.bounds.request_timeout)),
    }
}

/// Content-Security-Policy for the served pages: no scripts other than our
/// own file, no network access, nothing embeddable. Any HTML that slipped
/// through escaping is inert under this policy.
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// Content-Security-Policy for the standalone Software Verification Results
/// report: completely scriptless, inline styles only, nothing embeddable.
const REPORT_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The host part of a `Host` value, lowercased: `[::1]:8787` -> `::1`;
/// `localhost:8787` -> `localhost`; a bare IPv6 literal (several colons, no
/// brackets) carries no port.
fn host_part(host: &str) -> String {
    let h = host.trim();
    let h = if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else if h.matches(':').count() == 1 {
        h.split(':').next().unwrap_or(h)
    } else {
        h
    };
    h.to_ascii_lowercase()
}

/// The `Host` values this server answers to (README "Security posture").
///
/// The loopback names, and the address that was bound. On a wildcard
/// listener (`0.0.0.0`, `[::]`) "the address that was bound" is whichever
/// local address a request arrived on, written as an IP literal -- never the
/// wildcard itself, which no client can address, and which `--allow-remote`
/// used to add whatever was bound. A wildcard listener also refuses a
/// request with no `Host`: nothing about it names an address this server
/// was asked to answer on (#325).
#[derive(Debug, Clone, Default)]
pub struct HostPolicy {
    names: Vec<String>,
    wildcard: bool,
}

impl HostPolicy {
    /// For a server whose `--listen` host part was `bare` and whose socket
    /// is bound to `bound`.
    pub fn for_bind(bare: &str, bound: std::net::IpAddr) -> HostPolicy {
        let mut names: Vec<String> = ["127.0.0.1", "localhost", "::1"].iter().map(|s| s.to_string()).collect();
        let bare = host_part(bare);
        let wild_literal = bare.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_unspecified());
        if !bare.is_empty() && !wild_literal && !names.contains(&bare) {
            names.push(bare);
        }
        HostPolicy { names, wildcard: bound.is_unspecified() }
    }

    /// Whether a request whose `Host` is `host`, arriving on the local
    /// address `arrived_on` (when known), is answered.
    pub fn admits(&self, host: Option<&str>, arrived_on: Option<std::net::IpAddr>) -> bool {
        let Some(h) = host else {
            // A browser always sends Host, so on a specific address a
            // request without one cannot be a rebinding attack from a page.
            return !self.wildcard;
        };
        let h = host_part(h);
        if self.names.contains(&h) {
            return true;
        }
        // The literal the client wrote must be the address it reached: a
        // rebinding page writes its own name, never this.
        self.wildcard
            && match (h.parse::<std::net::IpAddr>(), arrived_on) {
                (Ok(written), Some(local)) => !local.is_unspecified() && written.to_canonical() == local.to_canonical(),
                _ => false,
            }
    }

    /// The allow-list in words, for a refusal.
    fn describe(&self) -> String {
        let names = self.names.join(", ");
        if self.wildcard {
            format!("{names} and the IP address a request arrives on")
        } else {
            names
        }
    }
}

/// The local address a connection arrived on, for [`HostPolicy::admits`].
#[derive(Debug, Clone, Copy)]
struct Arrived(Option<std::net::IpAddr>);

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_>> for Arrived {
    fn connect_info(target: axum::serve::IncomingStream<'_>) -> Self {
        Arrived(target.local_addr().ok().map(|a| a.ip()))
    }
}

fn deny(d: access::Denied) -> Response {
    match d {
        access::Denied::Unauthorized(m) => (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer realm=\"butterfly\"")], m).into_response(),
        access::Denied::Forbidden(m) => (StatusCode::FORBIDDEN, m).into_response(),
    }
}

fn bearer(req: &Request) -> Option<String> {
    req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_string)
}

async fn guard_read(State(s): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    match s.access.may_read(bearer(&req).as_deref()) {
        Ok(()) => next.run(req).await,
        Err(d) => deny(d),
    }
}

async fn guard_write(State(s): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    match s.access.may_write(bearer(&req).as_deref()) {
        Ok(()) => next.run(req).await,
        Err(d) => deny(d),
    }
}

async fn guard_host(State(s): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    // Decided from the Host header itself. Where it is absent, a wildcard
    // listener refuses outright: an absolute-form target
    // (`GET http://127.0.0.1:PORT/... HTTP/1.1`) names an authority, but that
    // is not the header the policy promises to require (#335 review). A
    // specific listener answers a request with no Host, but where the target
    // names an authority, that authority must be on the list.
    let header = req.headers().get(header::HOST).map(|v| v.to_str().ok().map(str::to_string));
    let host = match header {
        Some(h) => Some(h.unwrap_or_default()),
        None if s.hosts.wildcard => None,
        None => req.uri().host().map(str::to_string),
    };
    let arrived = req.extensions().get::<axum::extract::ConnectInfo<Arrived>>().and_then(|c| c.0 .0);
    if s.hosts.admits(host.as_deref(), arrived) {
        return next.run(req).await;
    }
    let what = match &host {
        Some(h) => format!("a request addressed to host {h:?}"),
        None => "a request with no Host header".to_string(),
    };
    // One status for every refusal here, whatever was wrong with the Host.
    (
        StatusCode::FORBIDDEN,
        format!(
            "butterfly refused {what}.\nThis server answers only to {} (protection against DNS rebinding).\nUse --listen with the address you want, and --allow-remote to serve a non-loopback address.\n",
            s.hosts.describe()
        ),
    )
        .into_response()
}

// `static`, not `const`: responses borrow these bytes in place, and only a
// static is one object with one address -- a const may be inlined per use.
static INDEX_HTML: &str = include_str!("../../../ui/index.html");
static APP_JS: &str = include_str!("../../../ui/app.js");
static APP_CSS: &str = include_str!("../../../ui/app.css");
static BRAND_LOGO: &[u8] = include_bytes!("../../../ui/butterfly-logo.png");

/// What butterfly says about itself at every start: the exposure, the
/// authentication mode, and how to open the viewer.
///
/// A function returning lines rather than a run of `eprintln!` so the one
/// thing that must never appear here can be asserted on: an operator's own
/// `--token` secret. stderr is collected and kept by service managers and CI
/// runners, so echoing a supplied credential turns it into a durable log
/// secret, and butterfly gains nothing by repeating a string the operator
/// typed (review of #24).
fn startup_banner(listen: &str, loopback: bool, read_only_flag: bool, access: &access::Access, token: Option<&access::Token>) -> Vec<String> {
    let mut out = vec![
        format!("butterfly: listening on {listen} ({})", if loopback { "loopback only" } else { "REACHABLE FROM THE NETWORK, plain HTTP, accepted with --insecure-http" }),
        format!(
            "butterfly: reads   {}",
            if access.requires_token() {
                "require a bearer token"
            } else if loopback {
                "open to anyone who can reach this port — the evidence embeds the complete source text"
            } else {
                // only possible with the override: say which flag did it
                "OPEN TO ANYONE WHO CAN REACH THIS PORT (--allow-unauthenticated-remote) — the evidence embeds the complete source text"
            }
        ),
        format!(
            "butterfly: writes  {} ({})",
            if access.read_only {
                "REFUSED — read-only"
            } else if access.requires_token() {
                "require a bearer token"
            } else {
                "allowed with no authentication"
            },
            access.because
        ),
    ];
    if !loopback && access.read_only && !read_only_flag {
        out.push("butterfly: give a token (--token-file <path>, BUTTERFLY_TOKEN, or --token generate) to record review decisions over the network, or serve on loopback through an SSH tunnel.".into());
    }
    if token.is_some() {
        // Plain HTTP off loopback: the bearer token and the source text it
        // protects both cross the network in clear. --allow-remote is the
        // operator's explicit acceptance of that exposure, but a token must
        // not be presented as if it made the hop safe.
        if !loopback {
            let port = listen.rsplit_once(':').map(|(_, p)| p).unwrap_or("8787");
            out.push("butterfly: WARNING — this listener is plain HTTP. The bearer token and the source text it protects".into());
            out.push("butterfly: WARNING — travel unencrypted; anyone on the path can read both, then write resolutions.".into());
            out.push("butterfly: WARNING — a token is not a substitute for a tunnel. Serve on loopback and forward the port:".into());
            out.push(format!("butterfly: WARNING —   ssh -N -L {port}:127.0.0.1:{port} <host>"));
        }
    }
    out
}

/// Where to open the viewer, for a server that needs a token.
///
/// Printed once the listener is bound, with the address it actually got, so
/// a `--listen …:0` server names its real port rather than `:0`. It follows
/// the `butterfly: http://HOST:PORT/` line and never takes that form itself:
/// a harness reading the URL line must not be able to pick up a token.
fn open_line(bound: &str, token: Option<&access::Token>) -> Option<String> {
    Some(match token?.bootstrap_url(bound) {
        // the one line that carries a token: butterfly minted it and nobody
        // else has it. It is in the fragment, which the browser never sends
        // (#65).
        Some(url) => format!("butterfly: open the viewer at {url}"),
        // the operator chose this token; it is not ours to write down
        None => format!("butterfly: open the viewer at http://{bound}/ and enter your token when it asks"),
    })
}

/// How the viewer is exposed, as the operator's flags said.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    pub listen: String,
    /// `--allow-remote`: a non-loopback address may be bound.
    pub allow_remote: bool,
    /// `--insecure-http`: the operator accepts that a non-loopback listener
    /// is plain HTTP (#65).
    pub insecure_http: bool,
    pub read_only: bool,
    pub token: Option<access::Token>,
    /// `--allow-unauthenticated-remote`: a non-loopback listener may serve
    /// without a token, reads open to anyone who can reach it (#67).
    pub allow_unauthenticated_remote: bool,
    /// `--source-root`: the tree the freshness check may read sources from.
    /// Default: the working directory, as `ingest` uses (#248).
    pub source_root: Option<PathBuf>,
    /// `--repo`: the repository git may be run in. Default: the one that
    /// contains the source root (#248).
    pub repo: Option<PathBuf>,
}

impl ServeOptions {
    /// Whether these flags may serve at all. Cheap and loads nothing, so
    /// `run` and `build --serve` can ask before a build that takes minutes
    /// rather than refuse after it.
    pub fn check(&self) -> Result<access::Listener> {
        let l = access::listener(&self.listen, self.allow_remote, self.insecure_http).map_err(anyhow::Error::msg)?;
        access::require_credentials(&l, &self.listen, self.token.is_some(), self.allow_unauthenticated_remote).map_err(anyhow::Error::msg)?;
        Ok(l)
    }
}

/// Does the socket agree with the exposure decided before the bind?
fn bound_agrees(decided_loopback: bool, bound: std::net::SocketAddr, listen: &str) -> Result<()> {
    if bound.ip().is_loopback() != decided_loopback {
        bail!(
            "refusing to serve: --listen {listen} was judged {} when it was checked, but the socket was bound to {bound}.\n\
             The name resolved differently between the check and the bind. Pass an IP address to --listen.",
            if decided_loopback { "loopback" } else { "not loopback" }
        );
    }
    Ok(())
}

/// A token in the query string has already been exposed by the time anyone
/// can read it: it was in the request line, in clear, and in whatever log a
/// proxy or server keeps of request targets. butterfly never reads one --
/// credentials travel only in `Authorization` -- but it used to *print* URLs
/// that carried one, and a bookmark of such a URL still sends it. Refusing it
/// says so, instead of letting the page load and fail with a less useful
/// "needs a token" (#65).
async fn refuse_token_in_url(req: Request, next: Next) -> Response {
    let named = req.uri().query().is_some_and(|q| q.split('&').any(|kv| kv.split('=').next().is_some_and(|k| k.eq_ignore_ascii_case("token"))));
    if named {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CACHE_CONTROL, "no-store"), (header::REFERRER_POLICY, "no-referrer")],
            "butterfly does not take a token in the URL query: it has just been sent in the request line, in clear, where a proxy or server log may keep it.\n\
             If that was a real token, treat it as exposed and replace it.\n\
             Open the viewer at / and enter the token when it asks, or use /#token=<secret> -- the part after # never leaves the browser.\n\
             (In a URL, write a % in the token as %25.)\n",
        )
            .into_response();
    }
    next.run(req).await
}

pub async fn serve(evidence: Vec<PathBuf>, resolutions: PathBuf, opts: ServeOptions) -> Result<()> {
    if evidence.is_empty() {
        bail!("no evidence file given");
    }
    // Loopback unless the operator asks for more, twice: the evidence embeds
    // the complete source text, and this server speaks plain HTTP.
    let net = opts.check()?;
    let ServeOptions { listen, read_only, token, source_root, repo, .. } = opts;
    let authority = freshness::Authority::resolve(source_root.as_deref(), repo.as_deref())?;
    eprintln!(
        "butterfly: freshness reads sources only under {}, and runs git only in {}",
        authority.source_root.display(),
        authority.repo.as_ref().map(|r| r.display().to_string()).unwrap_or_else(|| "no repository (none above the source root)".into())
    );
    let (loopback, bare) = (net.loopback, net.host);
    let access = access::Access::decide(loopback, read_only, token.as_ref().map(access::Token::as_str));
    for line in startup_banner(&listen, loopback, read_only, &access, token.as_ref()) {
        eprintln!("{line}");
    }
    let multi = evidence.len() > 1;
    // one budget across every file this server reads at startup (#5)
    let lim = butterfly_model::limits::Limits::from_env();

    // Load and name every variant before any sidecar is touched. Two of them
    // sharing a name, or a derived sidecar path, means one reviewer's
    // decisions would be written over another's -- so that has to be found
    // before the first read, not after the first write (#6).
    let mut loaded = Vec::new();
    for path in &evidence {
        let evidence_json = butterfly_model::limits::read_bounded_string(path, butterfly_model::limits::Kind::Evidence, &lim).map_err(|e| anyhow::anyhow!("{e}"))?;
        let ev: butterfly_model::Evidence = serde_json::from_str(&evidence_json).with_context(|| format!("{} is not a butterfly evidence file", path.display()))?;
        ev.validate().map_err(|e| anyhow::anyhow!("{}: inconsistent evidence — {e}", path.display()))?;
        // The API gate and report read the summary; bind it to the records it
        // is derived from before serving (#249).
        ev.check_summary_derivation().map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let name = ev
            .provenance
            .build_info
            .get("variant")
            .cloned()
            .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().trim_start_matches("evidence-").to_string()).unwrap_or_else(|| "default".into()));
        // `name` comes from the evidence file: never let it shape the path
        let resolutions_path = if multi { sidecar::path_for_variant(&resolutions, &name) } else { resolutions.clone() };
        loaded.push((path, evidence_json, ev, name, resolutions_path));
    }

    if multi {
        for (i, (pi, _, _, ni, si)) in loaded.iter().enumerate() {
            for (pj, _, _, nj, sj) in loaded.iter().skip(i + 1) {
                if ni == nj {
                    bail!(
                        "two evidence files are both the variant `{ni}`:\n  {}\n  {}\n\nThey would share one resolutions sidecar, so each would overwrite the other's review decisions. Give them distinct `variant` values in build-info, or serve them separately.",
                        pi.display(),
                        pj.display()
                    );
                }
                if si == sj {
                    bail!(
                        "variants `{ni}` and `{nj}` both resolve to the sidecar {}:\n  {}\n  {}\n\nOne would overwrite the other's review decisions.",
                        si.display(),
                        pi.display(),
                        pj.display()
                    );
                }
            }
        }

        // Decisions recorded before the sidecar name carried a digest are in
        // a different file. Saying so on stderr is not enough: a launcher or
        // a service manager swallows it, and the viewer then comes up showing
        // no reviews at all -- the misleading empty state this whole check
        // exists to prevent. Refuse until the operator has moved the file
        // (review of #6).
        for (path, _, _, name, side) in &loaded {
            if side.exists() {
                continue;
            }
            let legacy = sidecar::legacy_path_for_variant(&resolutions, name);
            if legacy != *side && legacy.exists() {
                bail!(
                    "variant `{name}` ({}) has review decisions recorded under the older sidecar name:\n  {}\n\nThis butterfly writes:\n  {}\n\nThe name now carries a digest of the variant, so that two variants cannot be given one file. Serving would show no reviews for this variant and write a second sidecar beside the first, so it stops here instead. Move the file and start again:\n\n  mv {:?} {:?}",
                    path.display(),
                    legacy.display(),
                    side.display(),
                    legacy.display(),
                    side.display()
                );
            }
        }
    }

    let mut variants = Vec::new();
    for (path, evidence_json, ev, name, resolutions_path) in loaded {
        // Absent is a normal first run. Present-but-unreadable is not: it
        // would become an empty set here and be written back over the real
        // file on the next mutation, losing every recorded decision. Ask
        // about the directory entry, so a dangling link is "broken" rather
        // than "absent" (#9), then read it under the run's budget (#5).
        let res = match std::fs::symlink_metadata(&resolutions_path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Resolutions { schema: RESOLUTIONS_SCHEMA.into(), entries: Vec::new() },
            _ => {
                let t =
                    butterfly_model::limits::read_bounded_string(&resolutions_path, butterfly_model::limits::Kind::Auxiliary, &lim).map_err(|e| anyhow::anyhow!("reading {e}"))?;
                let r: Resolutions = serde_json::from_str(&t).with_context(|| format!("{} is not a resolutions file", resolutions_path.display()))?;
                // Schema, and every field of every entry -- not just that the
                // file carries the schema tag this build understands
                // (review of #66).
                r.validate().map_err(|e| anyhow::anyhow!("{}: {e}", resolutions_path.display()))?;
                // Stale keys are reported, never fatal: a recompile shifts the
                // addresses gap keys are built from, and refusing to serve
                // would take the viewer down over reviews the reviewer needs
                // the viewer to show them.
                let stale = r.stale_keys(&ev);
                if !stale.is_empty() {
                    eprintln!(
                        "butterfly: {}: {} review(s) name nothing in this evidence and close nothing — most likely stale after a rebuild: {}{}",
                        resolutions_path.display(),
                        stale.len(),
                        stale.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
                        if stale.len() > 3 { ", …" } else { "" }
                    );
                }
                r
            }
        };
        let summary = serde_json::to_value(&ev.summary)?;
        eprintln!("butterfly: variant `{name}`: {} functions from {}", ev.functions.len(), path.display());
        // The definition index, if `butterfly tags` has been run. Optional in
        // every direction: absent, unreadable, a foreign schema or one built
        // against different evidence all mean "no jumps", never an error --
        // it is a navigation aid and must not be able to stop the viewer
        // serving the evidence.
        let tags_json = load_tags(path, &evidence_json, &ev, &name, &lim).map(Bytes::from);
        // Requirement text, if it has been produced. Optional in every
        // direction, like the definition index: absent or unreadable means
        // tags show their ID alone, never an error.
        let (requirements, requirements_stale) = load_requirements(path, &ev, &name, &authority, &lim);

        let mut fresh = freshness::Input::of(&ev);
        // Bind the persisted impact to the resolutions actually loaded: a
        // sidecar that drifted between runs (or an evidence rebuilt since the
        // impact was written) is then stale from the first request, without
        // waiting for this process to write a resolution (#249).
        fresh.impact_stale = ev.changes.as_ref().and_then(|c| c.impact.as_ref()).and_then(|imp| butterfly_ingest::impact::staleness(imp, &ev, &res));
        fresh.requirements_stale = requirements_stale.clone();
        // say it at startup too: a stale history should not wait for someone to open the page
        let fr = freshness::check(&fresh, 0, path, &authority);
        for r in &fr.reasons {
            eprintln!("butterfly: variant `{name}`: change history is STALE — {r}");
        }
        if let Some(cmd) = &fr.rerun {
            eprintln!("butterfly: variant `{name}`: re-run `{cmd}`");
        }

        variants.push(Variant {
            name,
            path: path.clone(),
            // takes over the String's allocation; nothing is copied
            evidence_json: Bytes::from(evidence_json),
            summary,
            resolutions: Mutex::new(res),
            resolutions_path,
            fresh,
            resolution_edits: std::sync::atomic::AtomicUsize::new(0),
            tags_json,
            requirements,
            requirements_stale,
            evidence: Arc::new(ev),
            report_cache: Arc::default(),
            fresh_cache: Arc::default(),
        });
    }
    let ui_dir = std::env::var_os("BUTTERFLY_UI_DIR").map(PathBuf::from);
    let bounds = Bounds::default();
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    // The address actually bound. It is the one `--listen` gave unless that
    // asked for port 0, and then the two lines below are the only way anyone
    // can learn which port the kernel chose -- the URL line used to print
    // `:0`. A caller that cannot let the server choose has to pick a port for
    // it, and a port picked by binding and releasing a socket is one another
    // process may still be holding (#91, #99).
    let bind_host = listen.rsplit_once(':').map_or(listen.as_str(), |(h, _)| h);
    // The policy was decided from what --listen resolved to a moment ago;
    // the socket is what actually got bound. If a name resolved differently
    // the second time, nothing is served (review of #65).
    bound_agrees(loopback, listener.local_addr()?, &listen)?;
    // The allow-list follows the socket, as the exposure check above does:
    // whether this is a wildcard listener is the bound address's to say.
    let hosts = HostPolicy::for_bind(&bare, listener.local_addr()?.ip());
    let state = Arc::new(AppState { variants, ui_dir, hosts, access, bounds, jobs: Arc::new(Semaphore::new(bounds.jobs)), authority });
    let app = app(state);
    let bound = format!("{bind_host}:{}", listener.local_addr()?.port());
    eprintln!("butterfly: http://{bound}/");
    if let Some(line) = open_line(&bound, token.as_ref()) {
        eprintln!("{line}");
    }
    axum::serve(listener, app.into_make_service_with_connect_info::<Arrived>()).await?;
    Ok(())
}

/// The routes, their guards and the server-wide limits, over loaded state.
fn app(state: Arc<AppState>) -> Router {
    // Authorization is attached to the routes, not inferred from the bind
    // address: reads and writes carry different guards (#3).
    let reads = Router::new()
        .route("/api/variants", get(variants_handler))
        .route("/api/report", get(report_handler))
        .route("/api/evidence", get(evidence_handler))
        .route("/api/freshness", get(freshness_handler))
        .route("/api/tags", get(tags_handler))
        .route("/api/requirements", get(requirements_handler))
        .route("/api/gates", get(gates_handler))
        .route("/api/resolutions", get(get_resolutions))
        .layer(middleware::from_fn_with_state(state.clone(), guard_read));
    let writes = Router::new()
        .route("/api/resolutions", post(upsert_resolution))
        .route("/api/resolutions/delete", post(delete_resolution))
        .layer(middleware::from_fn_with_state(state.clone(), guard_write));
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/app.css", get(app_css))
        .route("/butterfly-logo.png", get(brand_logo))
        .merge(reads)
        .merge(writes)
        .layer(middleware::from_fn_with_state(state.clone(), limit_time))
        .layer(middleware::from_fn_with_state(state.clone(), guard_host))
        .layer(middleware::from_fn(refuse_token_in_url))
        .with_state(state)
}

#[derive(Deserialize)]
struct VQuery {
    v: Option<String>,
}

fn cache_word(hit: bool) -> &'static str {
    if hit {
        "hit"
    } else {
        "miss"
    }
}

/// Is the change history in this variant still the repository's history?
/// A handful of git calls and one hash per analysed file, so it runs off the
/// async threads, within the job cap, and one answer is reused for a few
/// seconds rather than recomputed for every request (#67).
async fn freshness_handler(State(s): State<Arc<AppState>>, Query(q): Query<VQuery>) -> Response {
    let i = match pick_index(&s, &q) {
        Ok(i) => i,
        Err(r) => return r,
    };
    match freshness_body(&s, i).await {
        Ok((body, hit)) => {
            ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff"), (X_CACHE, cache_word(hit))], body)
                .into_response()
        }
        Err(r) => r,
    }
}

/// The freshness answer for variant `i` as JSON, and whether it was cached.
///
/// Reused while it is younger than `Bounds::fresh_ttl` and no review has
/// been written since: a review changes the answer, so it is part of the
/// key. A burst of requests runs git once, not once each, and a check that
/// outlasts one request's patience is still kept for the next.
async fn freshness_body(s: &Arc<AppState>, i: usize) -> Result<(Bytes, bool), Response> {
    let v = &s.variants[i];
    let edits = v.resolution_edits.load(Ordering::Relaxed);
    let st = s.clone();
    single_flight(s, &v.fresh_cache, edits, Some(s.bounds.fresh_ttl), move || {
        let v = &st.variants[i];
        serde_json::to_vec(&freshness::check(&v.fresh, edits, &v.path, &st.authority)).map(Bytes::from).map_err(|e| e.to_string())
    })
    .await
}

/// The variant a request names, by position. The expensive routes need an
/// index rather than a reference: their work runs on the blocking pool,
/// which takes the shared state by value, not a borrow of it.
// The error *is* the HTTP response to send, which is how axum handlers are
// written; boxing it would only move the bytes and complicate every caller.
#[allow(clippy::result_large_err)]
fn pick_index(s: &AppState, q: &VQuery) -> Result<usize, Response> {
    match &q.v {
        None if !s.variants.is_empty() => Ok(0),
        None => Err((StatusCode::NOT_FOUND, "no evidence loaded").into_response()),
        Some(n) => s.variants.iter().position(|v| &v.name == n).ok_or_else(|| (StatusCode::NOT_FOUND, format!("no variant {n:?}")).into_response()),
    }
}

#[allow(clippy::result_large_err)]
fn pick<'a>(s: &'a AppState, q: &VQuery) -> Result<&'a Variant, Response> {
    pick_index(s, q).map(|i| &s.variants[i])
}

/// A file from the `BUTTERFLY_UI_DIR` override, read under the auxiliary
/// cap like every other file butterfly reads (#246); `Ok(None)` (the
/// override has no such file) sends the embedded copy instead. It is read on
/// every request, so it was a way to make a running server read a file of
/// any size, over and over. A file that is there but refused -- past the
/// cap, or unreadable -- is an error naming the variable, not a quiet swap
/// for the embedded copy: the operator asked for their file (#325 A3).
fn ui_override(dir: &std::path::Path, name: &str) -> Result<Option<Bytes>, String> {
    ui_override_with(dir, name, &butterfly_model::limits::Limits::from_env())
}

fn ui_override_with(dir: &std::path::Path, name: &str, lim: &butterfly_model::limits::Limits) -> Result<Option<Bytes>, String> {
    let path = dir.join(name);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        _ => {}
    }
    butterfly_model::limits::read_bounded(&path, butterfly_model::limits::Kind::Auxiliary, lim).map(|b| Some(Bytes::from(b)))
}

/// The refusal of an override file: the detail -- which names the file, and
/// so the override directory -- on the server's log, and only what happened
/// in the response (#340).
fn ui_refused(e: String) -> Response {
    eprintln!("butterfly: the BUTTERFLY_UI_DIR override was not served: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CACHE_CONTROL, "no-store"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
        "butterfly: the BUTTERFLY_UI_DIR override was not served; the server log says why.\n",
    )
        .into_response()
}

/// The embedded copy is served straight out of the binary. Only a
/// `BUTTERFLY_UI_DIR` override allocates, because it is read afresh each time.
fn static_file(state: &AppState, name: &str, embedded: &'static str, mime: &'static str) -> Response {
    let embedded = || Bytes::from_static(embedded.as_bytes());
    let body = match &state.ui_dir {
        Some(d) => match ui_override(d, name) {
            Ok(b) => b.unwrap_or_else(embedded),
            Err(e) => return ui_refused(e),
        },
        None => embedded(),
    };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_SECURITY_POLICY, CSP),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        body,
    )
        .into_response()
}

async fn index(State(s): State<Arc<AppState>>) -> Response {
    static_file(&s, "index.html", INDEX_HTML, "text/html; charset=utf-8")
}
async fn app_js(State(s): State<Arc<AppState>>) -> Response {
    static_file(&s, "app.js", APP_JS, "application/javascript; charset=utf-8")
}
async fn app_css(State(s): State<Arc<AppState>>) -> Response {
    static_file(&s, "app.css", APP_CSS, "text/css; charset=utf-8")
}

async fn brand_logo(State(s): State<Arc<AppState>>) -> Response {
    // 6 MB; copying it into a fresh Vec per request was the same cost as #73
    let body = match s.ui_dir.as_ref().map(|d| ui_override(d, "butterfly-logo.png")).transpose() {
        Ok(b) => b.flatten().unwrap_or_else(|| Bytes::from_static(BRAND_LOGO)),
        Err(e) => return ui_refused(e),
    };
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_SECURITY_POLICY, CSP),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        body,
    )
        .into_response()
}

async fn variants_handler(State(s): State<Arc<AppState>>) -> Response {
    json_value(&serde_json::json!({
        // the path as `/api/gates` prints it (#214), not the operator's
        // directory layout (#340)
        "variants": s
            .variants
            .iter()
            .map(|v| serde_json::json!({ "name": v.name, "path": for_display(&v.path), "summary": v.summary }))
            .collect::<Vec<_>>(),
        // so the viewer can say "read-only" instead of offering a button
        // that will come back 403
        "access": { "readOnly": s.access.read_only, "requiresToken": s.access.requires_token(), "because": s.access.because },
    }))
}

/// Read `<evidence-stem>.tags.json` beside the evidence, if it is there and
/// belongs to this evidence.
///
/// Every failure is a `None`, deliberately: a stale, malformed or foreign
/// index must cost the viewer nothing more than the jump links. It is checked
/// against the evidence's own hash so an index built for a different document
/// is ignored rather than pointing at lines that have moved.
/// The definition index beside an evidence file, if it can be trusted.
///
/// The sidecar sits in the same directory as the evidence and is served
/// verbatim to the browser, so it is untrusted input on the same footing as
/// everything else ingest reads. Every failure returns `None` -- the index is
/// a navigation aid and must never stop the viewer serving the evidence --
/// but a failure it does not *detect* is worse than no index at all:
///
///   * unbounded, it could exhaust memory before anything looked at it, so
///     it is read under the run's shared auxiliary budget (#5);
///   * unvalidated, an out-of-range `fileId`/`functionId` reaches the client
///     and throws when the reader hovers the identifier that carries it;
///   * with an empty hash accepted as a match, an index built against
///     different evidence routes go-to-definition to unrelated code.
///
/// So: read under the run's auxiliary cap, require the exact hash (this
/// schema's producer always writes one), and range-check every id against
/// the evidence it will be served with (review of #32).
/// Open an auto-discovered sidecar without following a final symlink and
/// refusing hard-linked or non-regular files (#341). Returns `Ok(None)` when
/// the file simply does not exist.
fn open_discovered_sidecar(path: &std::path::Path) -> Result<Option<std::fs::File>, String> {
    let f = match butterfly_ingest::fsauth::open_nofollow(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let meta = f.metadata().map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{}: not a regular file", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 {
            return Err(format!("{}: hard-linked ({} names), so its place cannot be established", path.display(), meta.nlink()));
        }
    }
    Ok(Some(f))
}

fn load_tags(evidence_path: &std::path::Path, evidence_json: &str, ev: &butterfly_model::Evidence, variant: &str, lim: &butterfly_model::limits::Limits) -> Option<String> {
    let stem = evidence_path.file_stem()?.to_string_lossy().into_owned();
    let p = evidence_path.parent()?.join(format!("{stem}.tags.json"));
    let f = match open_discovered_sidecar(&p) {
        Ok(Some(f)) => f,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("butterfly: variant `{variant}`: {e}; no definition jumps");
            return None;
        }
    };
    let text = match butterfly_model::limits::read_bounded_string_from(f, &p, butterfly_model::limits::Kind::Auxiliary, lim) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("butterfly: variant `{variant}`: {e}; no definition jumps");
            return None;
        }
    };
    let tags: butterfly_model::tags::Tags = serde_json::from_str(&text).ok()?;
    if tags.check_schema().is_err() {
        eprintln!("butterfly: variant `{variant}`: {} is not a supported tags index; no definition jumps", p.display());
        return None;
    }
    let want = {
        use sha2::{Digest, Sha256};
        Sha256::digest(evidence_json.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    // An absent hash is not a pass. `butterfly tags` always writes one, so a
    // sidecar without it was not written by this producer.
    if tags.evidence_sha256 != want {
        let why = if tags.evidence_sha256.is_empty() { "carries no evidence hash" } else { "was built against different evidence" };
        eprintln!("butterfly: variant `{variant}`: {} {why}; no definition jumps until `butterfly tags` is re-run", p.display());
        return None;
    }
    if let Err(e) = tags.validate(ev.files.len(), ev.functions.len()) {
        eprintln!("butterfly: variant `{variant}`: {} is inconsistent with this evidence — {e}; no definition jumps", p.display());
        return None;
    }
    eprintln!("butterfly: variant `{variant}`: {} definition(s) for go-to-definition", tags.entries.len());
    Some(text)
}

#[derive(serde::Deserialize)]
struct GateQuery {
    #[serde(default)]
    v: Option<String>,
    /// Percentage floors, as `butterfly check` takes them.
    #[serde(default)]
    min_lines: Option<f64>,
    #[serde(default)]
    min_outcomes: Option<f64>,
    #[serde(default)]
    min_mcdc: Option<f64>,
    #[serde(default)]
    max_unresolved_gaps: Option<usize>,
}

/// Quote a path for the shell only when it needs it, so the common case
/// stays readable and the uncommon one stays correct.
/// A path as it should appear in a command the reader is meant to run.
///
/// The Gates box prints "the equivalent command", and it printed the evidence
/// and sidecar paths exactly as the server was given them. Started with an
/// absolute path -- which is what a script, an IDE or a shell completion
/// usually produces -- the operator's own directory layout appeared in the
/// UI, in every screenshot of it and in any report of it (#214).
///
/// A path UNDER the working directory is printed relative to it: that is the
/// path a reader who follows the command from the same directory will use,
/// and it is the same file. Everything else is printed exactly as it was
/// given. A path outside the working directory, or on another volume, has no
/// relative form the reader could run, and the rule is never to invent a path
/// the user did not give.
///
/// Deliberately no `canonicalize()`: resolving symlinks would print a path
/// the operator never typed, which is the same defect from the other side.
/// That does mean a `/tmp/x` given while the cwd resolves to `/private/tmp`
/// stays absolute -- the honest outcome, not a guess.
pub(crate) fn for_display(p: &std::path::Path) -> String {
    let as_given = p.display().to_string();
    // A relative path is already the reader's path; only an absolute one can
    // carry the operator's machine into the UI.
    if !p.is_absolute() {
        return as_given;
    }
    match std::env::current_dir() {
        Ok(cwd) => match p.strip_prefix(&cwd) {
            // `strip_prefix` of the cwd itself yields "", which is not a path
            // anyone can run.
            Ok(rel) if !rel.as_os_str().is_empty() => {
                let rel = rel.display().to_string();
                // Shortening must not turn a path into an option. A leading
                // `-` -- on the file itself, or on the first directory
                // component -- makes clap read the argument as a flag, so the
                // printed command failed with `unexpected argument` on the
                // very path the absolute form ran fine (review of #230).
                // Quoting does not help: the shell strips the quotes and clap
                // still sees a leading `-`. `./` is what makes it a path
                // again, and it is what a person would type.
                if rel.starts_with('-') {
                    format!("./{rel}")
                } else {
                    rel
                }
            }
            _ => as_given,
        },
        Err(_) => as_given,
    }
}

pub(crate) fn sh_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-/+=:,@".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// A percentage floor, or nothing.
///
/// The same constraint `butterfly check` enforces at the flag: `-1` passes
/// anything, `101` is treated as 100, and NaN breaches every measurement.
/// A gate whose threshold is nonsense is worse than no gate, and the viewer
/// must not be the way round it (#43).
fn pct(v: Option<f64>) -> Result<Option<f64>, String> {
    match v {
        None => Ok(None),
        Some(x) if x.is_finite() && (0.0..=100.0).contains(&x) => Ok(Some(x)),
        Some(x) => Err(format!("{x} is not a percentage between 0 and 100")),
    }
}

/// What the gates say about the evidence this server already holds.
///
/// `check`, `claims`, `impact` and the worklist are each a pure function of
/// the evidence and the resolutions -- both loaded at startup. So this adds
/// no subprocess, no shell, no filesystem access and no client-supplied
/// path: it computes over data any reader of `/api/evidence` already has,
/// which is what makes it safe to expose (#75).
///
/// Producing evidence is deliberately *not* here. Running a build means
/// executing arbitrary project code, and no confirmation dialog makes that
/// safe to invoke from a web page.
///
/// Evaluating them is still real work over the whole evidence, so it runs
/// off the async threads and within the job cap (#67). Unlike the report it
/// is not cached: the thresholds come from the request.
async fn gates_handler(State(s): State<Arc<AppState>>, Query(q): Query<GateQuery>) -> Response {
    let i = match pick_index(&s, &VQuery { v: q.v.clone() }) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let th = match (pct(q.min_lines), pct(q.min_outcomes), pct(q.min_mcdc)) {
        (Ok(l), Ok(o), Ok(m)) => butterfly_ingest::gate::Thresholds { min_lines: l, min_outcomes: o, min_mcdc: m, max_unresolved_gaps: q.max_unresolved_gaps },
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    let st = s.clone();
    match run_blocking(&s, move || gates_body(&st.variants[i], &th)).await {
        Ok(body) => json_response(body),
        Err(r) => r,
    }
}

/// The gates' answer for one variant, as JSON: the work behind /api/gates.
fn gates_body(v: &Variant, th: &butterfly_ingest::gate::Thresholds) -> String {
    let ev = &*v.evidence;
    let res = v.lock_resolutions().clone();

    let asked = th.min_lines.is_some() || th.min_outcomes.is_some() || th.min_mcdc.is_some() || th.max_unresolved_gaps.is_some();
    let breaches = if asked { butterfly_ingest::gate::evaluate(ev, &res, th) } else { Vec::new() };
    let work = butterfly_ingest::worklist::compute(ev, &res);
    let impact = butterfly_ingest::impact::compute(ev, &res).ok();
    let claims = v.requirements.as_ref().map(|q| butterfly_model::claimcheck::compare(ev, q));

    // The command that does the same thing, so a reader can hand it to an
    // engineer or put it in CI. The GUI teaching the CLI beats the GUI
    // replacing it -- which only works if the command really is the same
    // thing: the evidence path and, above all, the review sidecar this
    // endpoint just evaluated with. Left out, `check` discovers a different
    // sidecar (or none) and the exit code on screen describes a command
    // nobody was given (review of #75).
    let mut cmd = format!("butterfly check {}", sh_quote(&for_display(&v.path)));
    match std::fs::symlink_metadata(&v.resolutions_path) {
        // No sidecar yet. Omitting the flag would NOT reproduce this state:
        // `check` then goes looking, and an unrelated resolutions.json beside
        // the evidence would justify gaps this endpoint is counting against
        // the build -- the same disagreement as naming the wrong file, from
        // the other side. Say "no reviews" outright (review of #75).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => cmd.push_str(" --no-resolutions"),
        _ => {
            cmd.push_str(" --resolutions ");
            cmd.push_str(&sh_quote(&for_display(&v.resolutions_path)));
        }
    }
    for (flag, val) in [("--min-lines", th.min_lines), ("--min-outcomes", th.min_outcomes), ("--min-mcdc", th.min_mcdc)] {
        if let Some(x) = val {
            cmd.push_str(&format!(" {flag} {x}"));
        }
    }
    if let Some(n) = th.max_unresolved_gaps {
        cmd.push_str(&format!(" --max-unresolved-gaps {n}"));
    }

    serde_json::json!({
        "thresholdsGiven": asked,
        "breaches": breaches,
        "command": cmd,
        "worklist": work.iter().take(20).collect::<Vec<_>>(),
        "worklistTotal": work.len(),
        "impact": impact,
        "claims": claims,
        // `check` exits 3 on a breach; say the same thing here so the two
        // cannot disagree about whether a build passes.
        "wouldExit": if breaches.is_empty() { 0 } else { 3 },
    })
    .to_string()
}

/// The requirement text beside the evidence, if `butterfly requirements` has
/// been run.
///
/// `requirements.json` is a project artifact, not an evidence-derived one:
/// the same requirements answer for many builds, so several evidence files
/// legitimately share one and it is not named after any of them.
///
/// It is optional in every direction. Absent, unreadable or a foreign schema
/// all mean "tags show their ID alone", never an error -- text is a reading
/// aid and must not be able to stop the viewer serving the evidence.
fn load_requirements(
    evidence_path: &std::path::Path,
    ev: &butterfly_model::Evidence,
    variant: &str,
    authority: &freshness::Authority,
    lim: &butterfly_model::limits::Limits,
) -> (Option<butterfly_model::reqtext::Requirements>, Vec<String>) {
    let Some(dir) = evidence_path.parent() else { return (None, Vec::new()) };
    let Some(stem) = evidence_path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { return (None, Vec::new()) };
    // A sidecar named for this evidence wins, then the shared one. Open with
    // O_NOFOLLOW and refuse hard-linked files so a planted link beside the
    // evidence cannot redirect the read (#341).
    let mut opened = None;
    for cand in [dir.join(format!("{stem}.requirements.json")), dir.join("requirements.json")] {
        match open_discovered_sidecar(&cand) {
            Ok(Some(f)) => {
                opened = Some((cand, f));
                break;
            }
            Ok(None) => continue,
            Err(e) => {
                eprintln!("butterfly: variant `{variant}`: {e}; requirement tags will show their ID alone");
                return (None, Vec::new());
            }
        }
    }
    let Some((p, f)) = opened else { return (None, Vec::new()) };

    let text = match butterfly_model::limits::read_bounded_string_from(f, &p, butterfly_model::limits::Kind::Auxiliary, lim) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("butterfly: variant `{variant}`: {e}; requirement tags will show their ID alone");
            return (None, Vec::new());
        }
    };
    let reqs: butterfly_model::reqtext::Requirements = match serde_json::from_str(&text) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("butterfly: variant `{variant}`: {} is not readable ({e}); requirement tags will show their ID alone", p.display());
            return (None, Vec::new());
        }
    };
    if let Err(e) = reqs.validate() {
        eprintln!("butterfly: variant `{variant}`: {} {e}; requirement tags will show their ID alone", p.display());
        return (None, Vec::new());
    }

    // Staleness is measured against the *documents*, not the evidence: a
    // requirements file that moved after this sidecar was built means the
    // text on screen is not the text that was baselined, and showing that
    // without saying so is worse than showing nothing.
    //
    // Every path comes out of the sidecar, which is untrusted, so it is bound
    // to `authority.source_root` via `freshness::read_approved` (no `..`, no
    // reads outside the approved tree, no symlinks escaping it, no hardlinks),
    // sharing the startup `lim` budget (#248).
    let mut stale = Vec::new();
    for src in &reqs.sources {
        let sp = std::path::Path::new(&src.path);
        let shown = for_display(sp);
        let outcome = if sp.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            freshness::Analysed::Unapproved("outside the approved source root")
        } else {
            let candidate = if sp.is_absolute() { sp.to_path_buf() } else { authority.source_root.join(sp) };
            freshness::read_approved(&candidate, &authority.source_root, butterfly_model::limits::Kind::Auxiliary, lim)
        };
        match outcome {
            freshness::Analysed::Text(bytes) => {
                use sha2::{Digest, Sha256};
                let now: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
                if now != src.sha256 {
                    let msg = format!("{shown} has changed since the requirement text was read — re-run `butterfly requirements`");
                    eprintln!("butterfly: variant `{variant}`: {msg}");
                    stale.push(msg);
                }
            }
            freshness::Analysed::Unreadable(w) => {
                let msg = format!("{shown} could not be read ({w}); whether the requirement text is still current was not checked");
                eprintln!("butterfly: variant `{variant}`: {msg}");
                stale.push(msg);
            }
            freshness::Analysed::Unapproved(w) => {
                eprintln!("butterfly: variant `{variant}`: {shown} was not checked ({w})");
                if w.contains("resolves outside") || w.contains("hard-linked") {
                    stale.push(format!("{shown} could not be verified ({w}); whether the requirement text is still current was not checked"));
                }
            }
            // Absent on this machine: requirements docs are often not beside
            // the evidence, and their absence says nothing about staleness.
            freshness::Analysed::Missing => {}
        }
    }

    // Both directions of "unmatched", said once at startup rather than left
    // for a reader to notice a blank tooltip and wonder.
    let tagged: std::collections::BTreeSet<&str> =
        ev.functions.iter().flat_map(|f| f.requirements.iter().chain(f.line_requirements.values().flatten())).map(String::as_str).collect();
    let missing = tagged.iter().filter(|id| reqs.get(id).is_none()).count();
    if missing > 0 {
        eprintln!("butterfly: variant `{variant}`: {missing} requirement(s) tagged in the source have no text in {}", p.display());
    }
    eprintln!("butterfly: variant `{variant}`: {} requirement(s) of text", reqs.entries.len());
    (Some(reqs), stale)
}

/// At most this many IDs in one request. The viewer asks for the handful on
/// one function; anything larger is either a mistake or an attempt to pull
/// the universe through an endpoint built to avoid exactly that.
const MAX_REQ_IDS: usize = 256;

#[derive(serde::Deserialize)]
struct ReqQuery {
    /// Same variant selector the other read routes use.
    #[serde(default)]
    v: Option<String>,
    /// Comma-separated requirement IDs.
    #[serde(default)]
    ids: String,
    /// Include the analysis around the statement (rationale, safety,
    /// verification criteria) and the document's traceability claim.
    #[serde(default)]
    full: Option<String>,
}

/// Requirement text for the IDs asked for, and only those.
///
/// The universe is thousands of entries and a source pane needs the few on
/// one function, so this answers by ID rather than handing over the file.
/// The statement is always returned; the analysis around it only on `full`,
/// because it is some six-sevenths of a record and is not wanted to put one
/// sentence beside a line.
async fn requirements_handler(State(s): State<Arc<AppState>>, Query(q): Query<ReqQuery>) -> Response {
    let v = match pick(&s, &VQuery { v: q.v.clone() }) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(reqs) = &v.requirements else {
        return json_response(r#"{"present":false,"entries":[],"unknown":[]}"#);
    };
    let want: Vec<&str> = q.ids.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if want.len() > MAX_REQ_IDS {
        return (StatusCode::BAD_REQUEST, format!("at most {MAX_REQ_IDS} ids per request")).into_response();
    }
    let full = q.full.as_deref().is_some_and(|f| f == "1" || f == "true");

    let mut entries = Vec::new();
    let mut unknown = Vec::new();
    for id in want {
        match reqs.get(id) {
            None => unknown.push(id),
            Some(r) => {
                let mut o = serde_json::json!({ "id": r.id, "text": r.text });
                if let Some(k) = &r.kind {
                    o["kind"] = serde_json::json!(k);
                }
                if let Some(st) = &r.status {
                    o["status"] = serde_json::json!(st);
                }
                if full {
                    o["parent"] = serde_json::json!(r.parent);
                    o["detail"] = serde_json::json!(r.detail);
                    o["claim"] = serde_json::json!(r.claim);
                    o["source"] = serde_json::json!(reqs.sources.get(r.source).map(|s| for_display(std::path::Path::new(&s.path))));
                }
                entries.push(o);
            }
        }
    }
    let body = serde_json::json!({ "present": true, "entries": entries, "unknown": unknown, "stale": v.requirements_stale }).to_string();
    json_response(body)
}

/// The definition index for a variant, or an empty one when there is none, so
/// the viewer has a single shape to consume.
async fn tags_handler(State(s): State<Arc<AppState>>, Query(q): Query<VQuery>) -> Response {
    let v = match pick(&s, &q) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let body =
        v.tags_json.clone().unwrap_or_else(|| Bytes::from(format!(r#"{{"schema":"{}","generator":"","evidenceSha256":"","entries":[]}}"#, butterfly_model::tags::TAGS_SCHEMA)));
    json_response(body)
}

async fn evidence_handler(State(s): State<Arc<AppState>>, Query(q): Query<VQuery>) -> Response {
    let v = match pick(&s, &q) {
        Ok(v) => v,
        Err(r) => return r,
    };
    // A reference-count increment, not a copy of the document (#73). The
    // explicit Content-Type replaces the octet-stream one `Bytes` implies.
    json_response(v.evidence_json.clone())
}

/// The standalone report. It reparsed the whole evidence and rendered a new
/// document for every request, synchronously on an async thread -- the
/// cheapest way for any client that could reach the port to spend the host's
/// CPU (#67). It is now rendered once per review generation, off the async
/// threads and within the job cap, and served from the cache in between.
async fn report_handler(State(s): State<Arc<AppState>>, Query(q): Query<VQuery>) -> Response {
    let i = match pick_index(&s, &q) {
        Ok(i) => i,
        Err(r) => return r,
    };
    match report_body(&s, i).await {
        Ok((html, hit)) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
                (header::CONTENT_SECURITY_POLICY, REPORT_CSP),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (header::REFERRER_POLICY, "no-referrer"),
                (X_CACHE, cache_word(hit)),
            ],
            html,
        )
            .into_response(),
        Err(r) => r,
    }
}

/// The rendered report for variant `i`, and whether it came from the cache:
/// it does whenever no review has been written since it was built. A hit
/// hands out the cached bytes themselves -- a reference count, not a copy --
/// and needs no job slot, so a saturated server still serves it.
async fn report_body(s: &Arc<AppState>, i: usize) -> Result<(Bytes, bool), Response> {
    let v = &s.variants[i];
    // A hit needs only the generation, not a copy of every review.
    if let Some(body) = v.report_cache.hit(v.resolution_edits.load(Ordering::Relaxed), None) {
        return Ok((body, true));
    }
    let (edits, res) = v.reviews();
    let st = s.clone();
    single_flight(s, &v.report_cache, edits, None, move || {
        let v = &st.variants[i];
        Ok(Bytes::from(report::render_with_warnings(
            &v.evidence,
            &res,
            v.requirements.as_ref(),
            if st.variants.len() > 1 { Some(v.name.as_str()) } else { None },
            &v.requirements_stale,
        )))
    })
    .await
}

async fn get_resolutions(State(s): State<Arc<AppState>>, Query(q): Query<VQuery>) -> Response {
    match pick(&s, &q) {
        Ok(v) => json_value(&*v.lock_resolutions()),
        Err(r) => r,
    }
}

#[derive(Deserialize)]
struct DeleteReq {
    key: String,
}

async fn upsert_resolution(State(s): State<Arc<AppState>>, Query(q): Query<VQuery>, Json(r): Json<Resolution>) -> Response {
    let i = match pick_index(&s, &q) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let st = s.clone();
    match tokio::task::spawn_blocking(move || {
        let key = r.key.clone();
        update(&st.variants[i], Some(&key), |res| {
            match res.entries.iter_mut().find(|e| e.key == r.key) {
                Some(e) => *e = r,
                None => res.entries.push(r),
            }
            res.entries.sort_by(|a, b| a.key.cmp(&b.key));
        })
    })
    .await
    {
        Ok(resp) => resp,
        Err(e) => {
            eprintln!("butterfly: resolution write task failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CACHE_CONTROL, "no-store"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
                "butterfly: the request failed; the server log has the detail.\n",
            )
                .into_response()
        }
    }
}

async fn delete_resolution(State(s): State<Arc<AppState>>, Query(q): Query<VQuery>, Json(d): Json<DeleteReq>) -> Response {
    let i = match pick_index(&s, &q) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let st = s.clone();
    match tokio::task::spawn_blocking(move || update(&st.variants[i], None, |res| res.entries.retain(|e| e.key != d.key))).await {
        Ok(resp) => resp,
        Err(e) => {
            eprintln!("butterfly: resolution delete task failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CACHE_CONTROL, "no-store"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
                "butterfly: the request failed; the server log has the detail.\n",
            )
                .into_response()
        }
    }
}

/// Apply `mutate` to the variant's resolutions and record the result, with
/// the lock held across both.
///
/// Mutating under the lock and then persisting a clone outside it let two
/// requests serialise as A-then-B in memory but land on disk as B-then-A,
/// leaving the sidecar without B while the UI showed it (#4). The write is
/// small and local, so the simplest correct answer is to make the mutation
/// and its write one critical section: whatever a 200 acknowledged is on
/// disk, and cannot be undone by an older snapshot.
///
/// A mutation that could not be written is rolled back rather than left in
/// memory. Evidence that the viewer shows but the sidecar does not have is
/// worse than an error the reviewer can retry.
fn update(v: &Variant, written: Option<&str>, mutate: impl FnOnce(&mut Resolutions)) -> Response {
    let mut res = v.lock_resolutions();
    let previous = res.clone();
    mutate(&mut res);
    // Validated against the evidence this server actually holds, after the
    // mutation and before anything is written: a bad record must never
    // reach disk. This is the one place both API writes go through, so it
    // is also the one place that has to check (review of #66). The evidence
    // is the copy parsed at startup, not a fresh parse per write (#67).
    let ev = &*v.evidence;
    // Two different questions, and conflating them made the viewer read-only
    // the moment any review went stale (review of #66).
    //
    // The file's own integrity -- schema, fields, uniqueness, count -- is a
    // property of the whole result, so it is checked over everything that
    // survives the mutation.
    if let Err(e) = res.validate() {
        *res = previous;
        return (StatusCode::BAD_REQUEST, e).into_response();
    }
    // Whether a key names a real obligation is asked only of the record just
    // written. Stale entries already on file are history: `stale_keys` on
    // load reports them and they close nothing, and requiring them to be
    // current here would refuse a perfectly good new review -- and refuse the
    // deletion that would have cleaned them up, which is the only way out.
    if let Some(k) = written {
        if res.stale_keys(ev).contains(&k) {
            *res = previous;
            return (StatusCode::BAD_REQUEST, format!("key {k:?} does not correspond to any obligation in this evidence")).into_response();
        }
    }
    let written = serde_json::to_vec_pretty(&*res).map_err(|e| std::io::Error::other(e.to_string())).and_then(|bytes| sidecar::write_atomic(&v.resolutions_path, &bytes));
    match written {
        Ok(how) => {
            if let sidecar::Wrote::ReplacedSymlink(target) = how {
                // say it once, loudly: someone put a link where the review
                // record goes, and butterfly did not write through it
                eprintln!(
                    "butterfly: variant `{}`: {} was a symlink to {} — it was REPLACED with a regular file, not written through. \
                     The target is untouched. If that link was yours, point --resolutions at a real path.",
                    v.name,
                    v.resolutions_path.display(),
                    target.display()
                );
            }
            v.resolution_edits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            json_value(&*res)
        }
        Err(e) => {
            *res = previous;
            // The path and the OS error are the operator's to read; the
            // client is told what happened and that nothing changed (#340).
            eprintln!("butterfly: variant `{}`: cannot write resolutions to {}: {e} — the change was NOT recorded", v.name, v.resolutions_path.display());
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CACHE_CONTROL, "no-store"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
                "cannot write the resolutions file — the change was NOT recorded and the previous file is unchanged; the server log says why",
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the paths the Gates command prints (#214) ------------------------
    //
    // `current_dir()` is process-global, so nothing here changes it: these
    // build their inputs FROM the real working directory instead, which is
    // also the only way they can run beside the other tests in this file.

    #[test]
    fn a_path_under_the_working_directory_is_printed_relative_to_it() {
        let cwd = std::env::current_dir().unwrap();
        // The shape the defect was reported in: the operator started the
        // server with an absolute path, and the UI printed their directory
        // layout back at every reader of the page.
        let abs = cwd.join("examples/c/build/evidence.json");
        assert_eq!(for_display(&abs), "examples/c/build/evidence.json");
        assert!(!for_display(&abs).contains(cwd.to_str().unwrap()));
    }

    #[test]
    fn a_relative_path_is_printed_exactly_as_it_was_given() {
        // Already the reader's path. Nothing to shorten, and nothing may be
        // added to it either.
        for p in ["examples/c/build/evidence.json", "./evidence.json", "../sibling/evidence.json"] {
            assert_eq!(for_display(std::path::Path::new(p)), p);
        }
    }

    #[test]
    fn a_path_outside_the_working_directory_keeps_the_path_that_was_given() {
        // There is no relative form a reader could run from here, so the
        // absolute path stays: the rule is never to invent one. `/` has no
        // parent to walk up from, and the working directory is never `/`
        // during a test run, so this is outside it whatever it is.
        let outside = std::path::Path::new("/evidence-for-214.json");
        assert_eq!(for_display(outside), "/evidence-for-214.json");
    }

    #[test]
    fn the_working_directory_itself_is_not_printed_as_the_empty_string() {
        // `strip_prefix` of the cwd yields "", which is not a path anyone can
        // run and would have produced `butterfly check ''`.
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(for_display(&cwd), cwd.display().to_string());
        assert!(!for_display(&cwd).is_empty());
    }

    /// Shortening an absolute path must not turn it into a command-line
    /// option (review of #230).
    ///
    /// A file called `-evidence.json`, or one inside a directory called
    /// `-dir`, shortened to a bare relative path, is read by clap as a flag:
    /// the printed command died with `unexpected argument` on a path whose
    /// absolute form ran fine, so the Gates box printed a command that did
    /// not reproduce its own verdict. Quoting does not fix it -- the shell
    /// removes the quotes and clap still sees the `-`.
    #[test]
    fn a_shortened_path_is_never_printed_as_an_option() {
        let cwd = std::env::current_dir().unwrap();
        for name in ["-evidence.json", "-dir/evidence.json", "--evidence.json"] {
            let shown = for_display(&cwd.join(name));
            assert_eq!(shown, format!("./{name}"), "a leading `-` needs `./` to stay a path");
            assert!(!shown.starts_with('-'), "{shown} would be parsed as an option");
        }
        // and an ordinary path keeps its plain relative form, with no `./`
        // noise added to the common case
        assert_eq!(for_display(&cwd.join("evidence.json")), "evidence.json");
        assert_eq!(for_display(&cwd.join("build/evidence.json")), "build/evidence.json");
    }

    // ---- concurrent resolution updates (#4) ------------------------------
    // These reach `update` directly: it is the whole critical section, and a
    // test that went through HTTP would prove less about the ordering.

    mod concurrency {
        use super::*;
        use butterfly_model::{Resolution, Resolutions, RESOLUTIONS_SCHEMA};
        use std::fs;
        use std::path::Path;

        pub(super) fn scratch(tag: &str) -> PathBuf {
            // a fresh, unpredictable directory, not a name another user
            // could plant a symlink at (#344)
            tempfile::Builder::new().prefix(&format!("butterfly-conc-{tag}-")).tempdir().unwrap().keep()
        }

        /// A real, minimal evidence document with `n` functions -- `"{}"`
        /// was enough of an evidence file before `update` had any reason to
        /// parse it, but it stopped being one the moment resolution writes
        /// started validating their key against real obligations (review of
        /// #66). These tests are about write-path concurrency, not
        /// resolution semantics, so the fixture carries just enough real
        /// functions for every `fn:<n>` key they write to be a genuine one.
        pub(super) fn evidence_json_with_functions(n: usize) -> String {
            serde_json::to_string(&evidence_with_functions(n)).unwrap()
        }

        /// The same document as a structure. It carries one source file,
        /// which every function names: the report renders a function's
        /// file, and a function pointing past the end of `files` would make
        /// the fixture, not the code under test, the thing that fails.
        pub(super) fn evidence_with_functions(n: usize) -> butterfly_model::Evidence {
            let mut ev = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
            ev.files = vec![butterfly_model::SourceFile { display: "f.c".into(), path: "f.c".into(), ..Default::default() }];
            ev.functions = (0..n).map(|i| butterfly_model::Function { id: i, name: format!("f{i}"), ..Default::default() }).collect();
            ev
        }

        fn variant(dir: &Path) -> Variant {
            variant_with(dir, 32)
        }

        pub(super) fn variant_with(dir: &Path, functions: usize) -> Variant {
            Variant {
                name: "v".into(),
                path: dir.join("evidence.json"),
                evidence_json: evidence_json_with_functions(functions).into(),
                summary: serde_json::json!({}),
                resolutions: Mutex::new(Resolutions { schema: RESOLUTIONS_SCHEMA.into(), entries: Vec::new() }),
                resolutions_path: dir.join("resolutions.json"),
                fresh: freshness::Input {
                    files: vec![],
                    repo: None,
                    head: None,
                    since: None,
                    has_changes: false,
                    has_impact: false,
                    impact_stale: None,
                    requirements_stale: vec![],
                },
                resolution_edits: std::sync::atomic::AtomicUsize::new(0),
                tags_json: None,
                requirements: None,
                requirements_stale: Vec::new(),
                evidence: Arc::new(evidence_with_functions(functions)),
                report_cache: Arc::default(),
                fresh_cache: Arc::default(),
            }
        }

        pub(super) fn entry(key: &str) -> Resolution {
            Resolution { key: key.into(), category: "a".into(), justification: "j".into(), author: "t".into(), date: "2026-01-01".into() }
        }

        /// What is actually on disk, as a restarted server would read it.
        fn on_disk(v: &Variant) -> Resolutions {
            serde_json::from_str(&fs::read_to_string(&v.resolutions_path).unwrap()).expect("the sidecar is valid JSON")
        }

        #[test]
        fn every_acknowledged_update_survives_a_restart() {
            let d = scratch("keys");
            let v = variant(&d);
            std::thread::scope(|s| {
                for n in 0..32 {
                    let v = &v;
                    s.spawn(move || {
                        let r = update(v, Some(&format!("fn:{n}")), |res| res.entries.push(entry(&format!("fn:{n}"))));
                        assert_eq!(r.status(), StatusCode::OK);
                    });
                }
            });
            let memory = v.resolutions.lock().unwrap().clone();
            assert_eq!(memory.entries.len(), 32, "every mutation is in memory");
            let disk = on_disk(&v);
            assert_eq!(disk.entries.len(), 32, "and none was overwritten by an older snapshot");
            let mut a: Vec<&str> = memory.entries.iter().map(|e| e.key.as_str()).collect();
            let mut b: Vec<&str> = disk.entries.iter().map(|e| e.key.as_str()).collect();
            a.sort_unstable();
            b.sort_unstable();
            assert_eq!(a, b);
            let _ = fs::remove_dir_all(d);
        }

        #[test]
        fn concurrent_updates_and_deletes_leave_disk_agreeing_with_memory() {
            let d = scratch("mixed");
            let v = variant(&d);
            for n in 0..16 {
                update(&v, Some(&format!("fn:{n}")), |res| res.entries.push(entry(&format!("fn:{n}"))));
            }
            std::thread::scope(|s| {
                for n in 0..16 {
                    let v = &v;
                    s.spawn(move || {
                        if n % 2 == 0 {
                            update(v, None, |res| res.entries.retain(|e| e.key != format!("fn:{n}")));
                        } else {
                            update(v, None, |res| {
                                let k = format!("fn:{n}");
                                if let Some(e) = res.entries.iter_mut().find(|e| e.key == k) {
                                    e.justification = "revised".into();
                                }
                            });
                        }
                    });
                }
            });
            let memory = v.resolutions.lock().unwrap().clone();
            assert_eq!(memory.entries.len(), 8, "the eight even keys were deleted");
            assert!(memory.entries.iter().all(|e| e.justification == "revised"));
            let disk = on_disk(&v);
            assert_eq!(serde_json::to_value(&disk).unwrap(), serde_json::to_value(&memory).unwrap(), "the sidecar is the in-memory state, not an older snapshot");
            let _ = fs::remove_dir_all(d);
        }

        #[cfg(unix)]
        #[test]
        fn a_failed_write_is_not_acknowledged_and_does_not_linger_in_memory() {
            use std::os::unix::fs::PermissionsExt;
            let d = scratch("failure");
            let v = variant(&d);
            assert_eq!(update(&v, Some("fn:1"), |res| res.entries.push(entry("fn:1"))).status(), StatusCode::OK);
            let before = fs::read_to_string(&v.resolutions_path).unwrap();

            fs::set_permissions(&d, fs::Permissions::from_mode(0o500)).unwrap();
            let r = update(&v, Some("fn:2"), |res| res.entries.push(entry("fn:2")));
            fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();

            assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(
                v.resolutions.lock().unwrap().entries.len(),
                1,
                "a mutation that could not be recorded must not stay in memory: the UI would show a decision the sidecar does not have"
            );
            assert_eq!(fs::read_to_string(&v.resolutions_path).unwrap(), before, "and the previous document is whole");
            let _ = fs::remove_dir_all(d);
        }

        #[test]
        fn only_persisted_updates_count_towards_freshness() {
            let d = scratch("edits");
            let v = variant(&d);
            update(&v, Some("fn:1"), |res| res.entries.push(entry("fn:1")));
            update(&v, Some("fn:2"), |res| res.entries.push(entry("fn:2")));
            assert_eq!(v.resolution_edits.load(std::sync::atomic::Ordering::Relaxed), 2);
            let _ = fs::remove_dir_all(d);
        }
    }

    // ---- immutable bodies are shared, not copied (#73) --------------------
    // The evidence is the largest thing this server sends and it never
    // changes after startup. Cloning it into every response cost a
    // payload-sized allocation and copy per request, held for as long as the
    // client took to read it -- four slow readers of a 16 MiB document held
    // 65 MiB. Equal bytes would not show that, so these ask whether the body
    // a handler returns *is* the stored buffer: the same allocation.

    /// The UI override is read on every request, under the auxiliary cap
    /// (#246). Past it the file is refused, naming the variable (#325 A3);
    /// an override with no such file falls back to the embedded copy.
    #[test]
    fn an_oversized_ui_override_is_not_read() {
        let d = tempfile::Builder::new().prefix("butterfly-uidir-").tempdir().unwrap().keep();
        std::fs::write(d.join("app.js"), vec![b'x'; 4096]).unwrap();
        let tiny = butterfly_model::limits::Limits::new(1024, 1024, 1024, 1024, 1024, 1024, 1 << 20);
        let e = ui_override_with(&d, "app.js", &tiny).expect_err("4 KiB against a 1 KiB cap");
        assert!(e.contains("BUTTERFLY_MAX_AUX_MB"), "{e}");
        let roomy = butterfly_model::limits::Limits::new(1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20);
        assert_eq!(ui_override_with(&d, "app.js", &roomy).map(|b| b.map(|b| b.len())), Ok(Some(4096)), "the control");
        assert_eq!(ui_override_with(&d, "app.css", &roomy), Ok(None), "no override file: the embedded copy");
        let _ = std::fs::remove_dir_all(&d);
    }

    mod shared_bodies {
        use super::*;

        fn state(v: Variant) -> Arc<AppState> {
            state_with(v, None)
        }

        fn state_with(v: Variant, ui_dir: Option<PathBuf>) -> Arc<AppState> {
            let bounds = Bounds::default();
            Arc::new(AppState {
                variants: vec![v],
                ui_dir,
                access: access::Access::decide(true, false, None),
                hosts: HostPolicy::default(),
                bounds,
                jobs: Arc::new(Semaphore::new(bounds.jobs)),
                authority: freshness::Authority { source_root: std::env::temp_dir().canonicalize().unwrap(), repo: None },
            })
        }

        async fn collect(r: Response) -> (StatusCode, axum::http::HeaderMap, axum::body::Bytes) {
            let (parts, body) = r.into_parts();
            (parts.status, parts.headers, axum::body::to_bytes(body, usize::MAX).await.unwrap())
        }

        fn json_headers(h: &axum::http::HeaderMap) {
            assert_eq!(h[header::CONTENT_TYPE], "application/json");
            assert_eq!(h[header::CACHE_CONTROL], "no-store");
            assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        }

        #[tokio::test]
        async fn every_evidence_response_is_the_stored_buffer_not_a_copy() {
            let s = state(super::concurrency::variant_with(std::path::Path::new("unused"), 4));
            let stored = &s.variants[0].evidence_json;
            for _ in 0..2 {
                let (st, h, body) = collect(evidence_handler(State(s.clone()), Query(VQuery { v: None })).await).await;
                assert_eq!(st, StatusCode::OK);
                json_headers(&h);
                assert_eq!(body, super::concurrency::evidence_json_with_functions(4).as_bytes(), "the document is served verbatim");
                assert_eq!((body.as_ptr(), body.len()), (stored.as_ptr(), stored.len()), "the response copied the evidence instead of sharing it");
            }
        }

        #[tokio::test]
        async fn a_tags_index_is_shared_and_its_absence_still_has_one_shape() {
            let mut v = super::concurrency::variant_with(std::path::Path::new("unused"), 4);
            let text = r#"{"schema":"x","entries":[]}"#.to_string();
            v.tags_json = Some(text.clone().into());
            let s = state(v);
            let stored = s.variants[0].tags_json.as_ref().unwrap();
            for _ in 0..2 {
                let (st, h, body) = collect(tags_handler(State(s.clone()), Query(VQuery { v: None })).await).await;
                assert_eq!(st, StatusCode::OK);
                json_headers(&h);
                assert_eq!(body, text.as_bytes());
                assert_eq!((body.as_ptr(), body.len()), (stored.as_ptr(), stored.len()), "the response copied the tags index instead of sharing it");
            }

            let s = state(super::concurrency::variant_with(std::path::Path::new("unused"), 4));
            let (_, h, body) = collect(tags_handler(State(s), Query(VQuery { v: None })).await).await;
            json_headers(&h);
            let want = format!(r#"{{"schema":"{}","generator":"","evidenceSha256":"","entries":[]}}"#, butterfly_model::tags::TAGS_SCHEMA);
            assert_eq!(body, want.as_bytes(), "no index is still an empty index, byte for byte");
        }

        // ---- what a client is told, and with which headers (#340) --------

        /// `/api/variants` and `GET /api/resolutions` answer like every other
        /// JSON route -- `nosniff`, `no-store` -- and a variant's path is the
        /// one `/api/gates` prints, not the operator's directory layout.
        #[tokio::test]
        async fn variants_and_resolutions_send_the_json_headers_and_no_absolute_path() {
            let cwd = std::env::current_dir().unwrap();
            let s = state(super::concurrency::variant_with(&cwd.join("build-340"), 1));
            let (st, h, body) = collect(variants_handler(State(s.clone())).await).await;
            assert_eq!(st, StatusCode::OK);
            json_headers(&h);
            let body = String::from_utf8(body.to_vec()).unwrap();
            assert!(!body.contains(cwd.to_str().unwrap()), "the working directory reached the client: {body}");
            let j: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(j["variants"][0]["path"], "build-340/evidence.json", "{body}");
            let (st, h, _) = collect(get_resolutions(State(s), Query(VQuery { v: None })).await).await;
            assert_eq!(st, StatusCode::OK);
            json_headers(&h);
        }

        /// A refused `BUTTERFLY_UI_DIR` file is a 500 whose body says what
        /// happened, not where the operator keeps the override.
        #[tokio::test]
        async fn a_refused_ui_override_does_not_name_its_path_to_the_client() {
            let d = super::concurrency::scratch("ui-refused-340");
            std::fs::create_dir_all(d.join("app.js")).unwrap(); // not a regular file
            let s = state_with(super::concurrency::variant_with(&d, 1), Some(d.clone()));
            let (st, h, body) = collect(app_js(State(s)).await).await;
            assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(h[header::CACHE_CONTROL], "no-store");
            assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
            let body = String::from_utf8(body.to_vec()).unwrap();
            assert!(body.contains("BUTTERFLY_UI_DIR"), "it says which setting: {body}");
            assert!(!body.contains(d.to_str().unwrap()) && !body.contains("regular file"), "the detail is for the server log: {body}");
            let _ = std::fs::remove_dir_all(&d);
        }

        /// A job that panics is a 500 that does not carry the panic message.
        #[tokio::test]
        async fn a_panicking_job_does_not_send_its_message_to_the_client() {
            let s = state(super::concurrency::variant_with(std::path::Path::new("unused"), 1));
            let r = run_blocking(&s, || -> () { panic!("secret-340 /home/operator/private") }).await.expect_err("the job panicked");
            let (st, _, body) = collect(r).await;
            assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR);
            let body = String::from_utf8(body.to_vec()).unwrap();
            assert!(!body.contains("secret-340") && !body.contains("/home/operator"), "{body}");
            assert!(run_blocking(&s, || ()).await.is_ok(), "and the slot came back");
        }

        /// A sidecar write that fails says so, without the sidecar's absolute
        /// path or the OS error: those are on the server's log.
        #[cfg(unix)]
        #[tokio::test]
        async fn a_failed_resolutions_write_tells_the_client_nothing_of_the_machine() {
            use std::os::unix::fs::PermissionsExt;
            let d = super::concurrency::scratch("write-340");
            let v = super::concurrency::variant_with(&d, 2);
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o500)).unwrap();
            let r = update(&v, Some("fn:1"), |res| res.entries.push(super::concurrency::entry("fn:1")));
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
            let (st, _, body) = collect(r).await;
            assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR);
            let body = String::from_utf8(body.to_vec()).unwrap();
            assert!(body.contains("NOT recorded"), "it still says the change was not kept: {body}");
            assert!(!body.contains(d.to_str().unwrap()), "the sidecar path reached the client: {body}");
            assert!(!body.contains("ermission") && !body.contains("os error"), "the OS error reached the client: {body}");
            // and a write that works answers with the JSON headers
            let (st, h, _) = collect(update(&v, Some("fn:1"), |res| res.entries.push(super::concurrency::entry("fn:1")))).await;
            assert_eq!(st, StatusCode::OK);
            json_headers(&h);
            let _ = std::fs::remove_dir_all(&d);
        }

        /// The freshness answer names the approved root, the approved
        /// repository and the evidence as the operator's relative paths.
        #[test]
        fn freshness_names_no_absolute_path_under_the_working_directory() {
            let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
            let inp = freshness::Input {
                files: vec![("a.c".into(), "/nonexistent-340/a.c".into(), "0".repeat(64))],
                repo: Some("/nonexistent-340/repo".into()),
                head: Some("0".repeat(40)),
                since: Some("v1".into()),
                has_changes: true,
                has_impact: false,
                impact_stale: None,
                requirements_stale: vec![],
            };
            let auth = freshness::Authority { source_root: cwd.join("src"), repo: Some(cwd.join("repo")) };
            let f = freshness::check(&inp, 0, &cwd.join("build/evidence.json"), &auth);
            let j = serde_json::to_string(&f).unwrap();
            assert!(f.stale, "{j}");
            assert!(!j.contains(cwd.to_str().unwrap()), "the working directory reached the client: {j}");
            assert!(j.contains("source root, src,") && j.contains("(repo)"), "the approved paths are still named: {j}");
            assert_eq!(f.rerun.as_deref(), Some("butterfly changes build/evidence.json --since v1"), "{j}");
            // the working directory itself is named as such, not printed
            let auth = freshness::Authority { source_root: cwd.clone(), repo: None };
            let f = freshness::check(&inp, 0, &cwd.join("build/evidence.json"), &auth);
            let j = serde_json::to_string(&f).unwrap();
            assert!(!j.contains(cwd.to_str().unwrap()), "{j}");
            assert!(j.contains("the working directory"), "{j}");
        }

        /// The embedded assets are `'static` already; the logo alone is 6 MB,
        /// and it was copied into a fresh `Vec` on every request.
        #[tokio::test]
        async fn embedded_assets_are_served_from_the_binary_not_copied() {
            let s = state(super::concurrency::variant_with(std::path::Path::new("unused"), 1));
            let (_, h, body) = collect(brand_logo(State(s.clone())).await).await;
            assert_eq!(h[header::CONTENT_TYPE], "image/png");
            assert_eq!((body.as_ptr(), body.len()), (BRAND_LOGO.as_ptr(), BRAND_LOGO.len()));
            let (_, h, body) = collect(app_js(State(s)).await).await;
            assert_eq!(h[header::CONTENT_TYPE], "application/javascript; charset=utf-8");
            assert_eq!(h[header::CONTENT_SECURITY_POLICY], CSP);
            assert_eq!((body.as_ptr(), body.len()), (APP_JS.as_ptr(), APP_JS.len()));
        }

        #[tokio::test]
        async fn report_handler_sets_no_store_and_scriptless_csp() {
            let mut v = super::concurrency::variant_with(std::path::Path::new("unused"), 1);
            v.requirements_stale = vec!["reqs.md has changed".into()];
            let s = state(v);
            let (st, h, body) = collect(report_handler(State(s), Query(VQuery { v: None })).await).await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(h[header::CACHE_CONTROL], "no-store");
            assert_eq!(h[header::CONTENT_SECURITY_POLICY], REPORT_CSP);
            assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
            assert_eq!(h[header::REFERRER_POLICY], "no-referrer");
            assert!(!body.is_empty());
        }

        #[tokio::test]
        async fn requirements_full_shortens_source_path_and_exposes_stale_warnings() {
            let cwd = std::env::current_dir().unwrap();
            let mut v = super::concurrency::variant_with(&cwd.join("build-req"), 1);
            v.requirements = Some(butterfly_model::reqtext::Requirements {
                schema: butterfly_model::reqtext::REQTEXT_SCHEMA.into(),
                generator: "test".into(),
                sources: vec![butterfly_model::reqtext::Source { path: cwd.join("docs/llr.md").display().to_string(), sha256: "0".repeat(64), count: 1 }],
                entries: vec![butterfly_model::reqtext::Requirement {
                    id: "LLR-1".into(),
                    text: "The system shall work.".into(),
                    kind: None,
                    status: None,
                    parent: None,
                    detail: Default::default(),
                    claim: Default::default(),
                    source: 0,
                    ..Default::default()
                }],
                ..Default::default()
            });
            v.requirements_stale = vec!["docs/llr.md has changed".into()];
            let s = state(v);
            let (st, _, body) = collect(requirements_handler(State(s), Query(ReqQuery { v: None, ids: "LLR-1".into(), full: Some("1".into()) })).await).await;
            assert_eq!(st, StatusCode::OK);
            let body = String::from_utf8(body.to_vec()).unwrap();
            assert!(!body.contains(cwd.to_str().unwrap()), "absolute cwd leaked in source: {body}");
            let j: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(j["entries"][0]["source"], "docs/llr.md");
            assert_eq!(j["stale"][0], "docs/llr.md has changed");
        }

        #[cfg(unix)]
        #[test]
        fn discovered_tags_and_requirements_refuse_symlinks_hardlinks_and_outside_sources() {
            let d = super::concurrency::scratch("sidecar-sec").canonicalize().unwrap();
            let root = d.join("root");
            let outside = d.join("outside");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(&outside).unwrap();

            let ev = super::concurrency::evidence_with_functions(1);
            let ev_json = serde_json::to_string(&ev).unwrap();
            let ev_path = root.join("evidence.json");
            std::fs::write(&ev_path, &ev_json).unwrap();
            let ev_sha: String = {
                use sha2::{Digest, Sha256};
                Sha256::digest(ev_json.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
            };
            let tags_body = serde_json::json!({
                "schema": butterfly_model::tags::TAGS_SCHEMA,
                "generator": "t",
                "evidenceSha256": ev_sha,
                "entries": [{ "name": "f0", "kind": "function", "file": "f.c", "line": 1, "fileId": 0, "functionId": 0 }]
            })
            .to_string();

            let lim = butterfly_model::limits::Limits::new(1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20, 1 << 20);
            // Symlink at evidence.tags.json must be refused
            let real_tags = outside.join("real.tags.json");
            std::fs::write(&real_tags, &tags_body).unwrap();
            let tags_path = root.join("evidence.tags.json");
            std::os::unix::fs::symlink(&real_tags, &tags_path).unwrap();
            assert!(load_tags(&ev_path, &ev_json, &ev, "v", &lim).is_none(), "symlink tags sidecar must be refused");
            std::fs::remove_file(&tags_path).unwrap();
            // Hardlink at evidence.tags.json must be refused
            std::fs::hard_link(&real_tags, &tags_path).unwrap();
            assert!(load_tags(&ev_path, &ev_json, &ev, "v", &lim).is_none(), "hardlink tags sidecar must be refused");
            std::fs::remove_file(&tags_path).unwrap();

            // Requirements sidecar: symlink / hardlink refused, and sources[].path confined to source_root
            let secret = outside.join("secret.md");
            std::fs::write(&secret, "SECRET\n").unwrap();
            let modified = root.join("llr.md");
            std::fs::write(&modified, "UPDATED\n").unwrap();
            let reqs_body = serde_json::json!({
                "schema": butterfly_model::reqtext::REQTEXT_SCHEMA,
                "generator": "t",
                "sources": [
                    { "path": secret.display().to_string(), "sha256": "0".repeat(64), "count": 1 },
                    { "path": "../outside/secret.md", "sha256": "0".repeat(64), "count": 0 },
                    { "path": modified.display().to_string(), "sha256": "0".repeat(64), "count": 0 }
                ],
                "entries": [{ "id": "LLR-1", "text": "x", "source": 0 }]
            })
            .to_string();
            let real_reqs = outside.join("real.requirements.json");
            std::fs::write(&real_reqs, &reqs_body).unwrap();
            let reqs_path = root.join("requirements.json");
            let auth = freshness::Authority { source_root: root.clone(), repo: None };

            std::os::unix::fs::symlink(&real_reqs, &reqs_path).unwrap();
            assert!(load_requirements(&ev_path, &ev, "v", &auth, &lim).0.is_none(), "symlink requirements sidecar must be refused");
            std::fs::remove_file(&reqs_path).unwrap();

            std::fs::hard_link(&real_reqs, &reqs_path).unwrap();
            assert!(load_requirements(&ev_path, &ev, "v", &auth, &lim).0.is_none(), "hardlink requirements sidecar must be refused");
            std::fs::remove_file(&reqs_path).unwrap();

            // Regular file: outside source is NOT read (no "has changed"), modified inside source IS flagged stale
            std::fs::write(&reqs_path, &reqs_body).unwrap();
            let (reqs, stale) = load_requirements(&ev_path, &ev, "v", &auth, &lim);
            assert!(reqs.is_some());
            assert_eq!(stale.len(), 1, "only the in-root modified doc should be read and flagged: {stale:?}");
            assert!(stale[0].contains("llr.md") && stale[0].contains("has changed"), "{stale:?}");
            assert!(!stale.iter().any(|s| s.contains("secret.md")), "outside file must never be read: {stale:?}");

            let _ = std::fs::remove_dir_all(&d);
        }
    }

    /// The check after the bind refuses whenever the socket and the decision
    /// disagree -- above all a "loopback" decision on a wildcard socket.
    #[test]
    fn a_socket_that_disagrees_with_the_exposure_decision_is_refused() {
        let at = |s: &str| s.parse::<std::net::SocketAddr>().unwrap();
        assert!(bound_agrees(true, at("127.0.0.1:1"), "l").is_ok());
        assert!(bound_agrees(true, at("[::1]:1"), "l").is_ok());
        assert!(bound_agrees(false, at("0.0.0.0:1"), "l").is_ok());
        let e = bound_agrees(true, at("0.0.0.0:1"), "127.x.example:0").unwrap_err().to_string();
        assert!(e.contains("0.0.0.0:1") && e.contains("IP address"), "{e}");
        assert!(bound_agrees(false, at("127.0.0.1:1"), "l").is_err());
    }

    #[test]
    fn host_guard_accepts_loopback_only() {
        let a = HostPolicy::for_bind("127.0.0.1", std::net::Ipv4Addr::LOCALHOST.into());
        let here = Some(std::net::Ipv4Addr::LOCALHOST.into());
        for good in ["127.0.0.1", "127.0.0.1:8787", "localhost", "localhost:8787", "LocalHost:8787", "[::1]:8787", "::1"] {
            assert!(a.admits(Some(good), here), "should accept {good}");
        }
        // DNS rebinding: the attacker controls the name, not the address
        for bad in ["evil.example", "evil.example:8787", "butterfly.attacker.test", "127.0.0.1.evil.example", "0.0.0.0:8787"] {
            assert!(!a.admits(Some(bad), here), "should refuse {bad}");
        }
    }

    /// README "Security posture", host allow-list, as decided in #325
    /// (ambiguity 10): on a wildcard listener the wildcard literal is not an
    /// address that was bound, and a request with no Host is refused, with
    /// the status a foreign Host gets. Over a real socket, through
    /// `serve`'s own connection setup -- but on a loopback port: the
    /// wildcard policy is given to the state, so no test opens a port to the
    /// network.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wildcard_host_policy_over_http() {
        use std::io::{Read, Write};
        let d = concurrency::scratch("wildcard-host");
        let bounds = Bounds::default();
        let state = Arc::new(AppState {
            variants: vec![concurrency::variant_with(&d, 1)],
            ui_dir: None,
            access: access::Access::decide(true, false, None),
            hosts: HostPolicy::for_bind("0.0.0.0", std::net::Ipv4Addr::UNSPECIFIED.into()),
            bounds,
            jobs: Arc::new(Semaphore::new(bounds.jobs)),
            authority: freshness::Authority { source_root: std::env::temp_dir().canonicalize().unwrap(), repo: None },
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(l, app(state).into_make_service_with_connect_info::<Arrived>()).await.unwrap() });
        let status = |host: Option<String>| async move {
            tokio::task::spawn_blocking(move || {
                let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
                let host = host.map(|h| format!("Host: {h}\r\n")).unwrap_or_default();
                write!(s, "GET /api/variants HTTP/1.1\r\n{host}Connection: close\r\n\r\n").unwrap();
                let mut raw = Vec::new();
                s.read_to_end(&mut raw).unwrap();
                let text = String::from_utf8_lossy(&raw).into_owned();
                (text.split_whitespace().nth(1).and_then(|c| c.parse::<u16>().ok()).unwrap_or(0), text)
            })
            .await
            .unwrap()
        };
        let (foreign, _) = status(Some("evil.example".into())).await;
        assert_eq!(foreign, 403, "the control: a foreign Host is refused");
        for host in [Some("0.0.0.0".to_string()), Some(format!("0.0.0.0:{port}")), Some(format!("[::]:{port}")), None] {
            let (code, text) = status(host.clone()).await;
            assert_eq!(code, foreign, "{host:?} is refused as a foreign Host is: {text}");
            assert!(!text.contains("\"variants\""), "{host:?} was handed data: {text}");
        }
        let (_, text) = status(None).await;
        assert!(text.contains("no Host header"), "the refusal says what was wrong: {text}");
        // the loopback names, and the address the request arrived on (here
        // 127.0.0.1 is both), keep working
        for host in [format!("127.0.0.1:{port}"), "localhost".to_string(), format!("localhost:{port}")] {
            let (code, text) = status(Some(host.clone())).await;
            assert_eq!(code, 200, "{host}: {text}");
        }
        let _ = std::fs::remove_dir_all(d);
    }

    /// #335 review, README "Security posture": the no-Host refusal on a
    /// wildcard listener is decided from the `Host` header itself. An
    /// absolute-form request (`GET http://127.0.0.1:PORT/api/... HTTP/1.1`)
    /// with no `Host` names an authority in its target, but it still has no
    /// header, and is refused with the same 403 -- the URI's authority is not
    /// a stand-in for the header. On a specific listener a request with no
    /// `Host` is answered only when an authority it names is on the list.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_absolute_form_request_without_host_is_decided_by_the_header() {
        use std::io::{Read, Write};
        let serve = |bind: &'static str, ip: std::net::IpAddr, tag: &'static str| async move {
            let d = concurrency::scratch(tag);
            let bounds = Bounds::default();
            let state = Arc::new(AppState {
                variants: vec![concurrency::variant_with(&d, 1)],
                ui_dir: None,
                access: access::Access::decide(true, false, None),
                hosts: HostPolicy::for_bind(bind, ip),
                bounds,
                jobs: Arc::new(Semaphore::new(bounds.jobs)),
                authority: freshness::Authority { source_root: std::env::temp_dir().canonicalize().unwrap(), repo: None },
            });
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = l.local_addr().unwrap().port();
            tokio::spawn(async move { axum::serve(l, app(state).into_make_service_with_connect_info::<Arrived>()).await.unwrap() });
            (d, port)
        };
        let send = |port: u16, head: String| async move {
            tokio::task::spawn_blocking(move || {
                let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
                write!(s, "{head}Connection: close\r\n\r\n").unwrap();
                let mut raw = Vec::new();
                s.read_to_end(&mut raw).unwrap();
                let text = String::from_utf8_lossy(&raw).into_owned();
                (text.split_whitespace().nth(1).and_then(|c| c.parse::<u16>().ok()).unwrap_or(0), text)
            })
            .await
            .unwrap()
        };

        // wildcard listener
        let (d, port) = serve("0.0.0.0", std::net::Ipv4Addr::UNSPECIFIED.into(), "absolute-wild").await;
        let (control, _) = send(port, format!("GET /api/variants HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n")).await;
        assert_eq!(control, 200, "the control: an allowed Host is answered");
        for authority in [format!("127.0.0.1:{port}"), format!("localhost:{port}")] {
            let (code, text) = send(port, format!("GET http://{authority}/api/variants HTTP/1.1\r\n")).await;
            assert_eq!(code, 403, "absolute-form to {authority} with no Host header is refused: {text}");
            assert!(!text.contains("\"variants\""), "{authority} was handed data: {text}");
            assert!(text.contains("no Host header"), "the refusal says what was wrong: {text}");
        }
        // with the header, absolute-form is decided by the header as usual
        let (code, text) = send(port, format!("GET http://127.0.0.1:{port}/api/variants HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n")).await;
        assert_eq!(code, 200, "{text}");
        let _ = std::fs::remove_dir_all(d);

        // specific listener: no Host, so an authority the target names must be on the list
        let (d, port) = serve("127.0.0.1", std::net::Ipv4Addr::LOCALHOST.into(), "absolute-specific").await;
        let (code, text) = send(port, format!("GET http://127.0.0.1:{port}/api/variants HTTP/1.1\r\n")).await;
        assert_eq!(code, 200, "{text}");
        let (code, text) = send(port, "GET http://evil.example/api/variants HTTP/1.1\r\n".to_string()).await;
        assert_eq!(code, 403, "{text}");
        let (code, text) = send(port, "GET /api/variants HTTP/1.1\r\n".to_string()).await;
        assert_eq!(code, 200, "origin-form with no Host on a specific listener: {text}");
        // a Host that is there but is not readable text is not an absent one
        let (code, text) = send(port, format!("GET http://127.0.0.1:{port}/api/variants HTTP/1.1\r\nHost: h\u{e9}st\r\n")).await;
        assert_eq!(code, 403, "{text}");
        let _ = std::fs::remove_dir_all(d);
    }

    // ---- bounded expensive work, and who may start it (#67) --------------
    // Over a real socket where the claim is about what a client sees, and
    // against `run_blocking` directly where it is about how many jobs run:
    // a slot can be held there deterministically, while over HTTP whether
    // requests overlap depends on the host.

    mod bounded {
        use super::*;
        use std::io::{Read, Write};
        use std::sync::atomic::AtomicBool;

        const TOKEN: &str = "a-token-at-least-sixteen-characters";

        fn state_with(tag: &str, access: access::Access, bounds: Bounds) -> Arc<AppState> {
            let d = concurrency::scratch(tag);
            Arc::new(AppState {
                variants: vec![concurrency::variant_with(&d, 8)],
                ui_dir: None,
                access,
                hosts: HostPolicy::for_bind("127.0.0.1", std::net::Ipv4Addr::LOCALHOST.into()),
                bounds,
                jobs: Arc::new(Semaphore::new(bounds.jobs)),
                authority: freshness::Authority { source_root: std::env::temp_dir().canonicalize().unwrap(), repo: None },
            })
        }

        fn loopback() -> access::Access {
            access::Access::decide(true, false, None)
        }

        /// The real router on a real loopback socket, for as long as the
        /// test's runtime lives.
        async fn start(state: Arc<AppState>) -> u16 {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = l.local_addr().unwrap().port();
            tokio::spawn(async move { axum::serve(l, app(state)).await.unwrap() });
            port
        }

        /// One GET: (status, head, body). On the blocking pool, as a
        /// separate client would be, so it cannot starve the server it tests.
        async fn get(port: u16, target: &str, bearer: Option<&str>) -> (u16, String, String) {
            let (target, bearer) = (target.to_string(), bearer.map(str::to_string));
            tokio::task::spawn_blocking(move || {
                let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
                let auth = bearer.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
                write!(s, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Connection: close\r\n\r\n").unwrap();
                let mut raw = Vec::new();
                s.read_to_end(&mut raw).unwrap();
                let text = String::from_utf8_lossy(&raw).into_owned();
                let (head, body) = text.split_once("\r\n\r\n").map(|(h, b)| (h.to_string(), b.to_string())).unwrap_or_default();
                (head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0), head, body)
            })
            .await
            .unwrap()
        }

        fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
            head.lines().skip(1).find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim()))
        }

        /// Poll a condition rather than sleep a guessed interval.
        async fn wait_until(what: &str, cond: impl Fn() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !cond() {
                assert!(Instant::now() < deadline, "timed out waiting until {what}");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }

        fn renders(s: &AppState) -> usize {
            s.variants[0].report_cache.computed.load(Ordering::SeqCst)
        }
        fn checks(s: &AppState) -> usize {
            s.variants[0].fresh_cache.computed.load(Ordering::SeqCst)
        }

        /// The finding: every /api/report request reparsed the whole
        /// evidence and rendered a new document. Now one render serves every
        /// request until a review changes what the report says.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn repeated_report_requests_render_once_until_a_review_changes() {
            let s = state_with("report", loopback(), Bounds::default());
            let port = start(s.clone()).await;
            let mut seen = Vec::new();
            for _ in 0..3 {
                let (code, head, body) = get(port, "/api/report", None).await;
                assert_eq!(code, 200, "{body}");
                assert!(!body.is_empty());
                seen.push(header(&head, "x-butterfly-cache").unwrap_or("absent").to_string());
            }
            assert_eq!(seen, ["miss", "hit", "hit"]);
            assert_eq!(renders(&s), 1, "three requests, one render");

            let Ok((a, _)) = report_body(&s, 0).await else { panic!("the report failed") };
            let Ok((b, hit)) = report_body(&s, 0).await else { panic!("the report failed") };
            assert!(hit);
            assert_eq!(a.as_ptr(), b.as_ptr(), "a hit is the cached bytes themselves, not an equal copy rebuilt");

            let r = update(&s.variants[0], Some("fn:1"), |res| res.entries.push(concurrency::entry("fn:1")));
            assert_eq!(r.status(), StatusCode::OK);
            let (code, head, _) = get(port, "/api/report", None).await;
            assert_eq!((code, header(&head, "x-butterfly-cache")), (200, Some("miss")), "a recorded review rebuilds the report");
            assert_eq!(renders(&s), 2);
            let Ok((c, _)) = report_body(&s, 0).await else { panic!("the report failed") };
            assert_ne!(c.as_ptr(), a.as_ptr(), "and the old body is not served for the new reviews");
        }

        /// Freshness hashes every source and runs git. Reused briefly, and
        /// recomputed when a review changes the answer or it grows old.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn freshness_is_reused_briefly_and_recomputed_when_stale() {
            let s = state_with("fresh", loopback(), Bounds::default());
            let port = start(s.clone()).await;
            let mut seen = Vec::new();
            for _ in 0..3 {
                let (code, head, body) = get(port, "/api/freshness", None).await;
                assert_eq!(code, 200, "{body}");
                seen.push(header(&head, "x-butterfly-cache").unwrap_or("absent").to_string());
            }
            assert_eq!(seen, ["miss", "hit", "hit"]);
            assert_eq!(checks(&s), 1, "three requests, one check");

            // a review changes the answer, so it is part of the key
            update(&s.variants[0], Some("fn:2"), |res| res.entries.push(concurrency::entry("fn:2")));
            let (_, head, body) = get(port, "/api/freshness", None).await;
            assert_eq!(header(&head, "x-butterfly-cache"), Some("miss"));
            assert!(body.contains("\"resolutionEdits\":1"), "and the new answer counts it: {body}");
            assert_eq!(checks(&s), 2);

            // an answer older than the TTL is not reused: backdate it rather
            // than sleep through it
            {
                let mut st = s.variants[0].fresh_cache.lock();
                let e = st.done.as_mut().expect("cached");
                e.1 = e.1.checked_sub(s.bounds.fresh_ttl).expect("the clock goes back that far");
            }
            assert_eq!(header(&get(port, "/api/freshness", None).await.1, "x-butterfly-cache"), Some("miss"));
            assert_eq!(header(&get(port, "/api/freshness", None).await.1, "x-butterfly-cache"), Some("hit"));
            assert_eq!(checks(&s), 3);
        }

        /// The overload answer is deterministic: at the cap, an expensive
        /// route is 503 with Retry-After and does nothing; the cheap routes
        /// are not behind the cap at all; and a cached report needs no slot.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn past_the_cap_expensive_routes_are_refused_with_retry_after() {
            let s = state_with("cap", loopback(), Bounds { jobs: 1, ..Bounds::default() });
            let port = start(s.clone()).await;
            let held = s.jobs.clone().try_acquire_owned().unwrap();
            for route in ["/api/report", "/api/freshness", "/api/gates"] {
                let (code, head, body) = get(port, route, None).await;
                assert_eq!(code, 503, "{route}: {body}");
                assert_eq!(header(&head, "retry-after"), Some(RETRY_AFTER_SECS), "{route}: {head}");
                assert!(body.contains("limit"), "{route} says why: {body}");
            }
            assert_eq!(renders(&s) + checks(&s), 0, "and nothing ran");
            for route in ["/api/variants", "/api/evidence", "/api/resolutions", "/"] {
                assert_eq!(get(port, route, None).await.0, 200, "{route} is not behind the cap");
            }

            drop(held);
            let (code, head, _) = get(port, "/api/report", None).await;
            assert_eq!((code, header(&head, "x-butterfly-cache")), (200, Some("miss")), "a slot is free again");
            // The cache-miss worker publishes its response (tx.send) just
            // before its blocking closure returns and drops the permit, so a
            // permit is not necessarily free the instant the response arrives.
            // Wait for it (bounded) rather than try_acquire immediately, which
            // raced with worker teardown and failed with NoPermits.
            let _held =
                tokio::time::timeout(Duration::from_secs(5), s.jobs.clone().acquire_owned()).await.expect("the cache-miss worker frees its sole permit well within 5s").unwrap();
            let (code, head, _) = get(port, "/api/report", None).await;
            assert_eq!((code, header(&head, "x-butterfly-cache")), (200, Some("hit")), "a saturated server still serves a cached report");
        }

        /// However many ask, no more than the cap run -- and the rest are
        /// told so at once rather than queued.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn no_more_than_the_cap_of_expensive_jobs_ever_run_at_once() {
            let s = state_with("jobs", loopback(), Bounds { jobs: 2, job_timeout: Duration::from_secs(5), ..Bounds::default() });
            let running = Arc::new(AtomicUsize::new(0));
            let most = Arc::new(AtomicUsize::new(0));
            let release = Arc::new(AtomicBool::new(false));
            let job = |running: Arc<AtomicUsize>, most: Arc<AtomicUsize>, release: Arc<AtomicBool>| {
                move || {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    while !release.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    running.fetch_sub(1, Ordering::SeqCst);
                }
            };
            let mut within = Vec::new();
            for _ in 0..2 {
                let (s2, j) = (s.clone(), job(running.clone(), most.clone(), release.clone()));
                within.push(tokio::spawn(async move { run_blocking(&s2, j).await.is_ok() }));
            }
            wait_until("both jobs are running", || running.load(Ordering::SeqCst) == 2).await;

            for n in 0..6 {
                let t = Instant::now();
                match run_blocking(&s, job(running.clone(), most.clone(), release.clone())).await {
                    Ok(()) => panic!("job {n} ran while the cap was full"),
                    Err(r) => {
                        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
                        assert_eq!(r.headers().get(header::RETRY_AFTER).and_then(|v| v.to_str().ok()), Some(RETRY_AFTER_SECS));
                    }
                }
                assert!(t.elapsed() < Duration::from_secs(1), "job {n} was refused at once, not after waiting");
            }

            release.store(true, Ordering::SeqCst);
            for h in within {
                assert!(h.await.unwrap(), "the jobs within the cap complete");
            }
            assert_eq!(most.load(Ordering::SeqCst), 2, "never more than the cap at once");
            assert_eq!(s.jobs.available_permits(), 2, "and every slot came back");
        }

        /// A request stops waiting at the job timeout, but the job cannot be
        /// cancelled -- so its slot stays taken until it really ends.
        /// Otherwise a client could start unbounded work by timing out.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_job_that_overruns_is_answered_but_holds_its_slot_until_it_ends() {
            let s = state_with("overrun", loopback(), Bounds { jobs: 1, job_timeout: Duration::from_millis(100), ..Bounds::default() });
            let release = Arc::new(AtomicBool::new(false));
            let r = release.clone();
            let t = Instant::now();
            let outcome = run_blocking(&s, move || {
                while !r.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
            .await;
            let Err(resp) = outcome else { panic!("a job that overran was answered as if it had finished") };
            assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert!(resp.headers().contains_key(header::RETRY_AFTER));
            assert!(t.elapsed() < Duration::from_secs(5), "answered at the timeout, not when the job ended");

            assert_eq!(s.jobs.available_permits(), 0, "the job is still running, and still counts against the cap");
            assert!(matches!(run_blocking(&s, || ()).await, Err(r) if r.status() == StatusCode::SERVICE_UNAVAILABLE));

            release.store(true, Ordering::SeqCst);
            wait_until("the overrunning job ends and frees its slot", || s.jobs.available_permits() == 1).await;
            assert!(run_blocking(&s, || ()).await.is_ok());
        }

        /// No request is held open past the request timeout, whatever it is
        /// waiting on: here a handler that never finishes, behind the same
        /// layer every route has.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn no_request_is_held_open_past_the_request_timeout() {
            let s = state_with("slow", loopback(), Bounds { request_timeout: Duration::from_millis(200), ..Bounds::default() });
            let slow = Router::new()
                .route(
                    "/slow",
                    // axum's, not this module's HTTP helper of the same name
                    axum::routing::get(|| async {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                        "late"
                    }),
                )
                .route("/quick", axum::routing::get(|| async { "ok" }))
                .layer(middleware::from_fn_with_state(s.clone(), limit_time))
                .with_state(s.clone());
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = l.local_addr().unwrap().port();
            tokio::spawn(async move { axum::serve(l, slow).await.unwrap() });
            let t = Instant::now();
            let (code, head, body) = get(port, "/slow", None).await;
            assert_eq!(code, 503, "{body}");
            assert_eq!(header(&head, "retry-after"), Some(RETRY_AFTER_SECS));
            assert!(t.elapsed() < Duration::from_secs(5), "answered at the timeout");
            assert_eq!(get(port, "/quick", None).await.0, 200);
        }

        /// The finding: a report or freshness job that overran the wait was
        /// dropped, and the retry the 503 asked for started the same work
        /// again -- which overran again. Now the job keeps its answer, a
        /// retry while it runs joins it, and one after it ends is a hit
        /// (review of #67).
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_retry_after_an_overrun_is_served_by_the_job_that_overran() {
            let s = state_with("retry", loopback(), Bounds { jobs: 2, job_timeout: Duration::from_millis(100), ..Bounds::default() });
            let c = Arc::new(Cached::default());
            let release = Arc::new(AtomicBool::new(false));
            let r = release.clone();
            let slow = move || {
                while !r.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(Bytes::from_static(b"the answer"))
            };
            let Err(resp) = single_flight(&s, &c, 7, None, slow).await else { panic!("the overrunning job was answered as finished") };
            assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert!(resp.headers().contains_key(header::RETRY_AFTER));

            // a retry while it still runs joins it: no second computation, no second slot
            let Err(resp) = single_flight(&s, &c, 7, None, || panic!("a second computation was started")).await else { panic!("answered before the job ended") };
            assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(c.computed.load(Ordering::SeqCst), 1);
            assert_eq!(s.jobs.available_permits(), 1, "one slot is in use, not two");

            release.store(true, Ordering::SeqCst);
            wait_until("the job has stored its answer", || c.hit(7, None).is_some()).await;
            let Ok((body, hit)) = single_flight(&s, &c, 7, None, || panic!("the answer was computed again")).await else { panic!("the retry failed") };
            assert!(hit, "the retry is a hit");
            assert_eq!(&body[..], b"the answer");
            assert_eq!(c.computed.load(Ordering::SeqCst), 1);
            wait_until("the slot is released", || s.jobs.available_permits() == 2).await;
        }

        /// Concurrent misses for one answer share one computation and one
        /// slot, and all get the same bytes.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn concurrent_misses_share_one_computation() {
            let s = state_with("share", loopback(), Bounds::default());
            let c = Arc::new(Cached::default());
            let release = Arc::new(AtomicBool::new(false));
            let mut waiting = Vec::new();
            for _ in 0..5 {
                let (s2, c2, r) = (s.clone(), c.clone(), release.clone());
                waiting.push(tokio::spawn(async move {
                    single_flight(&s2, &c2, 1, None, move || {
                        while !r.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        Ok(Bytes::from(vec![7u8; 64]))
                    })
                    .await
                    .ok()
                }));
            }
            wait_until("the computation is running", || c.computed.load(Ordering::SeqCst) == 1).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(s.jobs.available_permits(), s.bounds.jobs - 1, "one slot for five requests");
            release.store(true, Ordering::SeqCst);
            let mut bodies = Vec::new();
            for w in waiting {
                bodies.push(w.await.unwrap().expect("every request is answered").0);
            }
            assert_eq!(c.computed.load(Ordering::SeqCst), 1, "five requests, one computation");
            assert!(bodies.iter().all(|b| b.as_ptr() == bodies[0].as_ptr()), "and the same bytes for all");
        }

        /// Off loopback a client without the token cannot start any of this
        /// work: the refusal comes before the handler, and so before a slot.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn off_loopback_an_unauthenticated_client_starts_no_work() {
            let s = state_with("remote", access::Access::decide(false, false, Some(TOKEN)), Bounds::default());
            let port = start(s.clone()).await;
            for route in ["/api/report", "/api/freshness", "/api/gates", "/api/variants"] {
                assert_eq!(get(port, route, None).await.0, 401, "{route}");
                assert_eq!(get(port, route, Some("not-the-token-000000000000")).await.0, 401, "{route}");
            }
            assert_eq!(renders(&s) + checks(&s), 0, "the refusal comes before any work");
            for route in ["/api/report", "/api/freshness", "/api/gates"] {
                let (code, _, body) = get(port, route, Some(TOKEN)).await;
                assert_eq!(code, 200, "{route}: {body}");
            }
        }

        /// The regression the issue asks for: non-loopback reads require
        /// credentials unless an explicitly named override is used -- and
        /// `--read-only` is no exception.
        #[test]
        fn a_network_listener_needs_a_token_unless_opened_by_name() {
            let o = |token: Option<access::Token>, open: bool, read_only: bool| ServeOptions {
                listen: "0.0.0.0:9000".into(),
                allow_remote: true,
                insecure_http: true,
                read_only,
                token,
                allow_unauthenticated_remote: open,
                source_root: None,
                repo: None,
            };
            for read_only in [false, true] {
                let e = o(None, false, read_only).check().unwrap_err().to_string();
                assert!(e.contains("read-only included"), "{e}");
                assert!(e.contains("--token-file") && e.contains("BUTTERFLY_TOKEN"), "{e}");
                assert!(e.contains("--allow-unauthenticated-remote"), "{e}");
                assert!(o(Some(access::Token::supplied(TOKEN.into())), false, read_only).check().is_ok(), "a token is enough");
                assert!(o(None, true, read_only).check().is_ok(), "and the override is honoured");
            }
            let local = ServeOptions {
                listen: "127.0.0.1:8787".into(),
                allow_remote: false,
                insecure_http: false,
                read_only: false,
                token: None,
                allow_unauthenticated_remote: false,
                source_root: None,
                repo: None,
            };
            assert!(local.check().is_ok(), "loopback is unchanged: no token, no flags");
        }
    }
}

#[cfg(test)]
mod banner_tests {
    use super::*;
    use access::{Access, Token};

    const SECRET: &str = "operator-chose-this-one";

    /// Everything butterfly prints about itself, in order: the banner, then
    /// -- after the bind, which these tests stand in for -- where to open it.
    fn banner(listen: &str, loopback: bool, read_only: bool, token: Option<Token>) -> String {
        let a = Access::decide(loopback, read_only, token.as_ref().map(Token::as_str));
        let mut lines = startup_banner(listen, loopback, read_only, &a, token.as_ref());
        lines.extend(open_line(listen, token.as_ref()));
        lines.join("\n")
    }

    /// The finding: every token was echoed verbatim into a plaintext URL on
    /// stderr, which service managers and CI runners keep. A token the
    /// operator supplied is already theirs; printing it only creates a second
    /// copy in the logs (review of #24).
    #[test]
    fn a_supplied_token_never_appears_in_the_startup_output() {
        for (listen, loopback) in [("127.0.0.1:8787", true), ("0.0.0.0:8787", false)] {
            let out = banner(listen, loopback, false, Some(Token::supplied(SECRET.into())));
            assert!(!out.contains(SECRET), "the operator's secret is in the startup output:\n{out}");
            assert!(!out.contains("?token=o"), "nor the beginning of it:\n{out}");
            assert!(out.contains("open the viewer at"), "the operator is still told where to go:\n{out}");
        }
    }

    /// A minted token is the one butterfly must print: nobody else knows it.
    #[test]
    fn a_generated_token_is_printed_because_it_is_the_only_copy() {
        let minted = "0123456789abcdef0123456789abcdef";
        let out = banner("127.0.0.1:8787", true, false, Some(Token::generated(minted.into())));
        assert!(out.contains(&format!("http://127.0.0.1:8787/#token={minted}")), "{out}");
    }

    /// The printed link must not put the token in the request target. In
    /// `?token=` it reached the server, and every proxy log on the way, in
    /// clear on the first request -- before the page's script could move it.
    /// After `#` a browser never sends it at all (#65).
    #[test]
    fn the_bootstrap_link_keeps_the_token_out_of_the_request_target() {
        let minted = "0123456789abcdef0123456789abcdef";
        for (listen, loopback) in [("127.0.0.1:8787", true), ("0.0.0.0:8787", false)] {
            let out = banner(listen, loopback, false, Some(Token::generated(minted.into())));
            let line = out.lines().find(|l| l.contains("open the viewer at")).unwrap_or_default();
            // Test harnesses find the port in the first `butterfly: http://…/`
            // line. This one carries a token, so it must never look like that.
            assert!(!(line.starts_with("butterfly: http://") && line.ends_with('/')), "the link could be taken for the URL line: {line}");
            let url = line.split_whitespace().last().unwrap_or_default();
            let target = url.split_once('#').map(|(before, _)| before).unwrap_or(url);
            assert!(!target.contains(minted), "the token is in the part of the URL that is sent: {url}");
            assert!(!target.contains("token"), "{url}");
            assert!(url.ends_with(&format!("#token={minted}")), "and it is in the fragment instead: {url}");
        }
    }

    /// An operator's own token is entered in the page, not written into a
    /// URL for them to paste: a URL is kept in browser history.
    #[test]
    fn a_supplied_token_is_entered_not_linked() {
        let out = banner("127.0.0.1:8787", true, false, Some(Token::supplied(SECRET.into())));
        assert!(out.contains("open the viewer at http://127.0.0.1:8787/ and enter your token"), "{out}");
        assert!(!out.contains("token="), "no link carries a token placeholder either: {out}");
    }

    /// `--token` authenticates; it does not encrypt. Off loopback the token
    /// and the source both cross the network in clear, and the banner has to
    /// say so rather than let a token read as "now it is secure".
    #[test]
    fn a_token_off_loopback_is_not_presented_as_a_secure_channel() {
        let out = banner("0.0.0.0:9000", false, false, Some(Token::generated("0123456789abcdef0123456789abcdef".into())));
        assert!(out.contains("WARNING"), "a plain-HTTP token deserves a warning:\n{out}");
        assert!(out.to_lowercase().contains("unencrypted"), "and says what the exposure is:\n{out}");
        assert!(out.contains("ssh -N -L 9000:127.0.0.1:9000"), "and offers the tunnel, on the port in use:\n{out}");
    }

    /// On loopback nothing leaves the machine, so the warning would be noise.
    #[test]
    fn loopback_gets_no_plaintext_warning() {
        let out = banner("127.0.0.1:8787", true, false, Some(Token::generated("0123456789abcdef0123456789abcdef".into())));
        assert!(!out.contains("WARNING"), "{out}");
    }

    /// The modes the banner already reported must keep reporting.
    #[test]
    fn the_banner_still_states_the_exposure_and_the_mode() {
        let remote = banner("0.0.0.0:8787", false, false, None);
        assert!(remote.contains("REACHABLE FROM THE NETWORK"), "{remote}");
        assert!(remote.contains("REFUSED — read-only"), "{remote}");
        assert!(remote.contains("--token-file"), "{remote}");
        assert!(remote.contains("plain HTTP"), "a network listener says it is unencrypted: {remote}");

        let local = banner("127.0.0.1:8787", true, false, None);
        assert!(local.contains("loopback only"), "{local}");
        assert!(local.contains("a loopback listener"), "{local}");
    }
}
