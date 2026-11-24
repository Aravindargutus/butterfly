// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Running the helper programs butterfly depends on -- git, the LLVM tools,
//! ctags, rustc and the compiler's own probes -- under one policy (#246).
//!
//! Each of these is a program butterfly does not control, pointed at input
//! it does not control. `Command::output()` waits forever and buffers
//! everything the child writes, so a hung helper hung butterfly, and a
//! helper that wrote without end -- a pathological repository, a huge
//! object, or a binary that is not the one we think -- was a way to exhaust
//! memory. Only ctags had been bounded (review of #32), and the viewer's
//! routes (#67); nothing else was. So every helper now runs through [`run`]:
//!
//! - **Identity.** The program is an absolute path the caller resolved and
//!   can record in provenance, never a bare name for the OS to search for
//!   again: the program that runs is the program that was named.
//! - **Environment.** Cleared, then only what a helper legitimately needs
//!   is passed through ([`PASSED_ENV`]). A `GIT_DIR` or `GIT_CONFIG_*` in the
//!   operator's shell cannot redirect git, and the viewer's token never
//!   reaches a child (review of #65).
//! - **A deadline** on the whole run, and a **ceiling on each stream**. A
//!   stream past its ceiling stops the child at once -- it does not wait for
//!   the deadline -- and so does the deadline. The child is always reaped.
//! - **Concurrency.** At most [`max_concurrent`] helpers run at once across
//!   the process, whatever the callers' own thread counts.
//!
//! Failures are a typed [`HelperError`], so a caller can say which bound was
//! hit and which variable raises it.
//!
//! Not covered, by design: the operator's own build -- `build.sh` under
//! `butterfly run`, the compile and test commands of `butterfly build`. Those
//! are the operator's programs with the operator's environment, streamed to
//! the terminal, not helpers reading untrusted input on butterfly's behalf.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The environment a helper gets: enough to find its own libraries and
/// toolchain and to speak the operator's locale, nothing else. `HOME` for
/// git's global config and rustup; the `RUSTUP_*`/`CARGO_HOME` trio so the
/// rustup proxy resolves the same toolchain the operator's shell would.
pub const PASSED_ENV: &[&str] =
    &["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE", "LC_MESSAGES", "TZ", "USER", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "CARGO_HOME", "SDKROOT", "DEVELOPER_DIR"];

/// How long and how much. Build with [`Bounds::standard`] or
/// [`Bounds::probe`] and adjust.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub deadline: Duration,
    pub stdout: u64,
    pub stderr: u64,
}

const MIB: u64 = 1024 * 1024;

impl Bounds {
    /// A real piece of work: `BUTTERFLY_HELPER_TIMEOUT_SECS` (default 600)
    /// and `BUTTERFLY_MAX_HELPER_OUTPUT_MB` (default 4096) on stdout; stderr
    /// is diagnostics and gets 1 MiB.
    pub fn standard() -> Bounds {
        Bounds {
            deadline: Duration::from_secs(env_u64("BUTTERFLY_HELPER_TIMEOUT_SECS", 600)),
            stdout: env_u64("BUTTERFLY_MAX_HELPER_OUTPUT_MB", 4096).saturating_mul(MIB),
            stderr: MIB,
        }
    }

    /// An identity or configuration question (`--version`, `--print
    /// sysroot`): it answers at once or not at all.
    pub fn probe() -> Bounds {
        Bounds { deadline: Duration::from_secs(10), stdout: 64 * 1024, stderr: 64 * 1024 }
    }

    pub fn with_stdout(self, bytes: u64) -> Bounds {
        Bounds { stdout: bytes, ..self }
    }

    pub fn with_deadline(self, d: Duration) -> Bounds {
        Bounds { deadline: d, ..self }
    }
}

fn env_u64(var: &str, default: u64) -> u64 {
    std::env::var(var).ok().and_then(|v| v.trim().parse().ok()).filter(|&n| n > 0).unwrap_or(default)
}

/// Why a helper did not give an answer.
#[derive(Debug)]
pub enum HelperError {
    /// The program was named relatively; its identity is not established.
    Unresolved(PathBuf),
    Spawn {
        tool: PathBuf,
        error: std::io::Error,
    },
    TimedOut {
        tool: PathBuf,
        after: Duration,
    },
    TooMuchOutput {
        tool: PathBuf,
        stream: &'static str,
        cap: u64,
    },
    /// It ran and exited unsuccessfully.
    Failed {
        tool: PathBuf,
        status: ExitStatus,
        stderr: String,
    },
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HelperError::Unresolved(p) => write!(f, "{} is not an absolute path: butterfly runs a helper only by the path it resolved and recorded", p.display()),
            HelperError::Spawn { tool, error } => write!(f, "could not run {}: {error}", tool.display()),
            HelperError::TimedOut { tool, after } => {
                write!(f, "{} did not finish within {}s and was stopped. Raise BUTTERFLY_HELPER_TIMEOUT_SECS if that is legitimate.", tool.display(), after.as_secs())
            }
            HelperError::TooMuchOutput { tool, stream, cap } => write!(
                f,
                "{} wrote more than {} to {stream} and was stopped.{}",
                tool.display(),
                butterfly_model::limits::human(*cap),
                if *stream == "stdout" { " Raise BUTTERFLY_MAX_HELPER_OUTPUT_MB if that is legitimate." } else { "" }
            ),
            HelperError::Failed { tool, status, stderr } => write!(f, "{} failed ({status}): {}", tool.display(), stderr.trim()),
        }
    }
}

impl std::error::Error for HelperError {}

/// `BUTTERFLY_MAX_HELPERS`, default the machine's parallelism.
pub fn max_concurrent() -> usize {
    env_u64("BUTTERFLY_MAX_HELPERS", std::thread::available_parallelism().map(|n| n.get() as u64).unwrap_or(4)) as usize
}

/// A counting semaphore over the whole process. Callers keep their own
/// thread pools (`changes --jobs`); this is what bounds the children.
struct Slots {
    free: Mutex<usize>,
    freed: Condvar,
}

struct Slot(&'static Slots);

impl Slots {
    fn take(&'static self) -> Slot {
        let mut n = self.free.lock().unwrap_or_else(|e| e.into_inner());
        while *n == 0 {
            n = self.freed.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n -= 1;
        Slot(self)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        *self.0.free.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.0.freed.notify_one();
    }
}

fn slots() -> &'static Slots {
    static S: OnceLock<Slots> = OnceLock::new();
    S.get_or_init(|| Slots { free: Mutex::new(max_concurrent().max(1)), freed: Condvar::new() })
}

/// Run `tool` with `args` under `bounds`, feeding it `stdin` if given.
///
/// `Ok` only for a zero exit; anything else is a [`HelperError`]. A caller
/// that needs to read a non-zero exit's output (git's `rev-list` probing,
/// say) uses [`run_any`].
pub fn run(tool: &Path, args: &[&OsStr], stdin: Option<Vec<u8>>, bounds: Bounds) -> Result<Vec<u8>, HelperError> {
    let (status, stdout, stderr) = run_any(tool, args, stdin, bounds)?;
    if !status.success() {
        return Err(HelperError::Failed { tool: tool.to_path_buf(), status, stderr: String::from_utf8_lossy(&stderr).into_owned() });
    }
    Ok(stdout)
}

/// [`run`], in `dir` rather than butterfly's own working directory.
///
/// For a question whose answer depends on where it is asked: a rustup proxy
/// picks its toolchain from the `rust-toolchain.toml` above its working
/// directory, so a compiler is identified in the directory it compiled in,
/// or the identity recorded is of a toolchain that did not run (#287).
pub fn run_in(tool: &Path, args: &[&OsStr], dir: &Path, bounds: Bounds) -> Result<Vec<u8>, HelperError> {
    let (status, stdout, stderr) = spawn(tool, args, None, bounds, &Extra { cwd: Some(dir), env: &[] })?;
    if !status.success() {
        return Err(HelperError::Failed { tool: tool.to_path_buf(), status, stderr: String::from_utf8_lossy(&stderr).into_owned() });
    }
    Ok(stdout)
}

/// [`run_any`], in `dir`: a tool identified where it ran (#298).
pub fn run_any_in(tool: &Path, args: &[&OsStr], dir: &Path, bounds: Bounds) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), HelperError> {
    spawn(tool, args, None, bounds, &Extra { cwd: Some(dir), env: &[] })
}

/// [`run`], returning the exit status and both streams whatever the status.
pub fn run_any(tool: &Path, args: &[&OsStr], stdin: Option<Vec<u8>>, bounds: Bounds) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), HelperError> {
    run_any_with(tool, args, stdin, bounds, &Extra::default())
}

/// What a caller may add to the policy: a working directory for a tool that
/// reports paths relative to it (svn), and variables set on top of the
/// cleared environment (a UTF-8 locale for svn). Nothing here passes the
/// operator's own environment through.
#[derive(Debug, Default, Clone, Copy)]
pub struct Extra<'a> {
    pub cwd: Option<&'a Path>,
    pub env: &'a [(&'a str, &'a OsStr)],
}

/// [`run_any`], with [`Extra`].
pub fn run_any_with(tool: &Path, args: &[&OsStr], stdin: Option<Vec<u8>>, bounds: Bounds, extra: &Extra) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), HelperError> {
    spawn(tool, args, stdin, bounds, extra)
}

fn spawn(tool: &Path, args: &[&OsStr], stdin: Option<Vec<u8>>, bounds: Bounds, extra: &Extra) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), HelperError> {
    if !tool.is_absolute() {
        return Err(HelperError::Unresolved(tool.to_path_buf()));
    }
    let _slot = slots().take();
    let mut cmd = Command::new(tool);
    cmd.env_clear();
    for k in PASSED_ENV {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    for (k, v) in extra.env {
        cmd.env(k, v);
    }
    if let Some(d) = extra.cwd {
        cmd.current_dir(d);
    }
    cmd.args(args).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    // Its own process group, so stopping it stops whatever it started too:
    // a grandchild still holding the pipes would otherwise keep the drains
    // below waiting after the helper itself was killed.
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|error| HelperError::Spawn { tool: tool.to_path_buf(), error })?;

    // Both streams are drained on their own threads before any input is
    // written, so a child that fills a pipe while we are still feeding it
    // cannot deadlock the two of us. A stream past its ceiling raises the
    // flag the loop below watches.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let over = [Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false))];
    fn drain(r: Option<impl Read + Send + 'static>, cap: u64, flag: Arc<AtomicBool>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(r) = r {
                // cap + 1 so "exactly at the cap" is not over
                let _ = r.take(cap.saturating_add(1)).read_to_end(&mut buf);
                if buf.len() as u64 > cap {
                    flag.store(true, Ordering::SeqCst);
                    buf.truncate(cap as usize);
                }
            }
            buf
        })
    }
    let out_t = drain(child.stdout.take(), bounds.stdout, over[0].clone());
    let err_t = drain(child.stderr.take(), bounds.stderr, over[1].clone());
    if let (Some(bytes), Some(mut w)) = (stdin, child.stdin.take()) {
        std::thread::spawn(move || {
            use std::io::Write;
            // a child that stops reading ends this with EPIPE, which is fine
            let _ = w.write_all(&bytes);
        });
    }

    let started = Instant::now();
    let stopped: Option<HelperError> = loop {
        if over[0].load(Ordering::SeqCst) {
            break Some(HelperError::TooMuchOutput { tool: tool.to_path_buf(), stream: "stdout", cap: bounds.stdout });
        }
        if over[1].load(Ordering::SeqCst) {
            break Some(HelperError::TooMuchOutput { tool: tool.to_path_buf(), stream: "stderr", cap: bounds.stderr });
        }
        // Only reap the direct child once both drains have finished (meaning
        // neither the child nor any descendant still holds stdout/stderr open).
        // Keeping the child unreaped until then guarantees the OS cannot
        // recycle its PID/PGID while we may still need to `kill(-pgid, SIGKILL)`
        // lingering descendants on timeout or output overflow (#9.12).
        if out_t.is_finished() && err_t.is_finished() {
            match child.try_wait() {
                Ok(Some(_)) => break None,
                Ok(None) => {}
                Err(error) => break Some(HelperError::Spawn { tool: tool.to_path_buf(), error }),
            }
        }
        if started.elapsed() >= bounds.deadline {
            break Some(HelperError::TimedOut { tool: tool.to_path_buf(), after: bounds.deadline });
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if let Some(e) = stopped {
        // The whole process group; the child has not been reaped yet so its
        // PID/PGID cannot have been recycled. Guard `pid > 0` so a bogus 0
        // could never signal our own process group.
        if let Ok(pid) = libc::pid_t::try_from(child.id()) {
            if pid > 0 {
                unsafe { libc::kill(-pid, libc::SIGKILL) };
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        // the drains end once the child's ends of the pipes close
        let _ = out_t.join();
        let _ = err_t.join();
        return Err(e);
    }
    let status = child.wait().map_err(|error| HelperError::Spawn { tool: tool.to_path_buf(), error })?;
    let stdout = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    // A stream can cross its ceiling in the moment between the child
    // exiting and the last read: that is still too much.
    if over[0].load(Ordering::SeqCst) {
        return Err(HelperError::TooMuchOutput { tool: tool.to_path_buf(), stream: "stdout", cap: bounds.stdout });
    }
    if over[1].load(Ordering::SeqCst) {
        return Err(HelperError::TooMuchOutput { tool: tool.to_path_buf(), stream: "stderr", cap: bounds.stderr });
    }
    Ok((status, stdout, stderr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh() -> PathBuf {
        PathBuf::from("/bin/sh")
    }

    fn script(s: &str) -> Vec<&OsStr> {
        vec![OsStr::new("-c"), OsStr::new(s)]
    }

    #[test]
    fn a_helper_that_hangs_is_stopped_at_the_deadline() {
        let t = Instant::now();
        let e = run(&sh(), &script("sleep 30"), None, Bounds::probe().with_deadline(Duration::from_millis(300))).unwrap_err();
        assert!(matches!(e, HelperError::TimedOut { .. }), "{e}");
        assert!(t.elapsed() < Duration::from_secs(5), "stopped promptly, not after the sleep: {:?}", t.elapsed());
        assert!(e.to_string().contains("BUTTERFLY_HELPER_TIMEOUT_SECS"), "and it names the variable: {e}");
    }

    /// Past the ceiling the child is stopped then and there. It used to be
    /// left blocked on a full pipe until the deadline, so "too much output"
    /// cost the whole deadline to find out.
    #[test]
    fn a_helper_that_writes_without_end_is_stopped_at_the_ceiling_not_the_deadline() {
        let t = Instant::now();
        let e = run(&sh(), &script("yes"), None, Bounds::standard().with_stdout(MIB).with_deadline(Duration::from_secs(60))).unwrap_err();
        assert!(matches!(e, HelperError::TooMuchOutput { stream: "stdout", .. }), "{e}");
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());

        let e = run(&sh(), &script("yes >&2"), None, Bounds::standard().with_deadline(Duration::from_secs(60))).unwrap_err();
        assert!(matches!(e, HelperError::TooMuchOutput { stream: "stderr", .. }), "{e}");
    }

    /// A helper that started children of its own: they are stopped with
    /// it, or they would hold the pipes open and the stop would never end.
    #[test]
    fn a_helper_whose_children_hold_the_pipes_is_still_stopped() {
        let t = Instant::now();
        let e = run(&sh(), &script("sleep 30 & sleep 30"), None, Bounds::probe().with_deadline(Duration::from_millis(300))).unwrap_err();
        assert!(matches!(e, HelperError::TimedOut { .. }), "{e}");
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    /// The distinct lifecycle: the *direct* child exits at once, but a
    /// backgrounded descendant inherits stdout and holds it open. The drain
    /// cannot finish until that pipe closes, so the deadline must stay in force
    /// after the direct child's exit and the whole group be cleaned up.
    /// Before the fix the loop broke on the child's exit and the join below
    /// blocked on the descendant with no bound, leaking the concurrency slot.
    #[test]
    fn a_child_that_exits_leaving_a_descendant_on_the_pipe_is_bounded() {
        let t = Instant::now();
        let e = run(&sh(), &script("sleep 30 & exit 0"), None, Bounds::probe().with_deadline(Duration::from_millis(300))).unwrap_err();
        assert!(matches!(e, HelperError::TimedOut { .. }), "{e}");
        assert!(t.elapsed() < Duration::from_secs(5), "bounded by the deadline, not the descendant's 30s: {:?}", t.elapsed());
    }

    #[test]
    fn exactly_at_the_ceiling_is_not_over_it() {
        let out = run(&sh(), &script("head -c 1024 /dev/zero"), None, Bounds::probe().with_stdout(1024)).unwrap();
        assert_eq!(out.len(), 1024);
    }

    #[test]
    fn an_abnormal_exit_is_a_typed_failure_with_its_diagnostics() {
        let e = run(&sh(), &script("echo broken >&2; exit 3"), None, Bounds::probe()).unwrap_err();
        match e {
            HelperError::Failed { status, stderr, .. } => {
                assert_eq!(status.code(), Some(3));
                assert!(stderr.contains("broken"), "{stderr}");
            }
            other => panic!("{other}"),
        }
        let (status, _, _) = run_any(&sh(), &script("exit 3"), None, Bounds::probe()).unwrap();
        assert_eq!(status.code(), Some(3), "run_any hands the status back instead");
    }

    /// A bare name would be searched for on PATH again at spawn time, and
    /// that is not necessarily the program the caller resolved and recorded.
    #[test]
    fn a_program_named_by_a_bare_name_is_refused() {
        let e = run(Path::new("sh"), &script("true"), None, Bounds::probe()).unwrap_err();
        assert!(matches!(e, HelperError::Unresolved(_)), "{e}");
    }

    /// Only the allow-listed variables reach the child. The viewer's token,
    /// and variables that redirect a helper (`GIT_DIR`), do not.
    #[test]
    fn the_environment_is_cleared_to_the_allow_list() {
        let out = run(&sh(), &script("env"), None, Bounds::probe()).unwrap();
        let env = String::from_utf8_lossy(&out);
        for line in env.lines() {
            let k = line.split('=').next().unwrap_or("");
            // the shell itself sets a few of its own
            if ["PWD", "SHLVL", "_", "OLDPWD"].contains(&k) {
                continue;
            }
            assert!(PASSED_ENV.contains(&k), "{k} reached the helper: {env}");
        }
    }

    /// Input and output both large: the drains start before the input is
    /// written, so neither side waits on the other forever.
    #[test]
    fn a_large_input_and_a_large_output_do_not_deadlock() {
        let input = vec![b'x'; 8 * MIB as usize];
        let out = run(&sh(), &script("cat"), Some(input), Bounds::standard().with_deadline(Duration::from_secs(30))).unwrap();
        assert_eq!(out.len(), 8 * MIB as usize);
    }

    #[test]
    fn the_slots_bound_how_many_run_at_once() {
        static S: OnceLock<Slots> = OnceLock::new();
        let s: &'static Slots = S.get_or_init(|| Slots { free: Mutex::new(2), freed: Condvar::new() });
        let running = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hs: Vec<_> = (0..6)
            .map(|_| {
                let (running, peak) = (running.clone(), peak.clone());
                std::thread::spawn(move || {
                    let _slot = s.take();
                    let now = running.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    peak.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(50));
                    running.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(peak.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
