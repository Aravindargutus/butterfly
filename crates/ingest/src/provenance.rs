// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! External-tool identity for evidence provenance (DO-330 §5.2.2.2.g,
//! §6.1.3.3.e, Appendix C FAQ C.2). The object-code evidence *is* the output
//! of an external disassembler, so which disassembler produced it — and its
//! version — is configuration-management data that belongs in the evidence.
//!
//! `identify` runs `<path> --version`, keeps the first non-empty line, and
//! returns an [`ExternalTool`] plus an optional caveat when the version could
//! not be read: the tool ran, so it is recorded either way, but a blank
//! version must be explained rather than silently shown.

use butterfly_model::ExternalTool;
use std::path::{Path, PathBuf};

/// The version-identifying line of `<path> --version`: the first non-empty
/// line that contains a digit, since that is where the version number lives
/// (`llvm-objdump` prints a URL banner first and the version on line two;
/// git, clang and gcc put the version on line one). Falls back to the first
/// non-empty line when none contains a digit. stdout is read first, then
/// stderr (some tools print their banner there). `None` when the process
/// could not be run, exited unsuccessfully, or wrote no non-empty line.
pub fn version_line(path: &Path) -> Option<String> {
    version_line_at(path, None)
}

/// [`version_line`], asked in `dir`: the directory the tool ran in. A tool
/// chosen by where it runs -- a rustup proxy under a `rust-toolchain.toml`,
/// a directory-scoped wrapper -- answers for the one that ran only there
/// (#287, #298).
pub fn version_line_in(path: &Path, dir: &Path) -> Option<String> {
    version_line_at(path, Some(dir))
}

fn version_line_at(path: &Path, dir: Option<&Path>) -> Option<String> {
    // bounded like every helper, as an identity probe: it answers at once
    // or not at all (#246)
    let exe = std::path::absolute(path).ok()?;
    let args = [std::ffi::OsStr::new("--version")];
    let bounds = crate::helper::Bounds::probe();
    let (status, stdout, stderr) = match dir {
        Some(d) => crate::helper::run_any_in(&exe, &args, d, bounds),
        None => crate::helper::run_any(&exe, &args, None, bounds),
    }
    .ok()?;
    if !status.success() {
        return None;
    }
    let pick = |s: &str| -> Option<String> {
        let lines: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        lines.iter().find(|l| l.chars().any(|c| c.is_ascii_digit())).or_else(|| lines.first()).map(|l| l.to_string())
    };
    pick(&String::from_utf8_lossy(&stdout)).or_else(|| pick(&String::from_utf8_lossy(&stderr)))
}

/// The caveat recorded when a tool ran but its version line could not be read.
/// Shared with [`record`] so a refreshed identity can retract the exact string
/// a prior blank-version probe pushed, rather than leave it contradicting the
/// version that was later read.
fn version_caveat(name: &str, path: &str) -> String {
    format!("could not read `{name} --version` at {path}; the tool ran but its version is not recorded")
}

/// Identify a tool butterfly resolved to `path`. Returns the record and, when
/// the version line could not be read, a caveat naming the tool and path.
pub fn identify(name: &str, path: &Path) -> (ExternalTool, Option<String>) {
    let path_str = path.display().to_string();
    match version_line(path) {
        Some(version) => (ExternalTool { name: name.into(), path: path_str, version }, None),
        None => {
            let caveat = version_caveat(name, &path_str);
            (ExternalTool { name: name.into(), path: path_str, version: String::new() }, Some(caveat))
        }
    }
}

/// Resolve a bare command name against `PATH` the way the OS resolves it for
/// execution: the first regular file that is *executable*, returned as an
/// absolute path. Matching the loader's rule matters -- a non-executable file
/// of the same name in an earlier `PATH` entry is skipped by execution, so it
/// must be skipped here too; recording it would name a tool that did not run.
/// Used for tools invoked by name (`git`).
pub fn resolve_on_path(name: &str) -> Option<PathBuf> {
    resolve_in(&std::env::var_os("PATH")?, name)
}

pub(crate) fn resolve_in(path_var: &std::ffi::OsStr, name: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_var) {
        let cand = dir.join(name);
        if is_executable_file(&cand) {
            return Some(std::path::absolute(&cand).unwrap_or(cand));
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}
#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// Add `tool` to `tools`, keeping one record per (name, path). The same
/// disassembler recorded for several objects appears once.
///
/// A *re-analysis* is different: `butterfly changes` loads an existing evidence
/// document and appends tool identity to its provenance, so the same path can
/// be recorded twice across runs. When the second observation reads a different
/// version -- git upgraded in place, or a probe that now succeeds where it
/// failed before -- it describes the tool that actually produced the new change
/// data, so its version *replaces* the stale one instead of being silently
/// dropped (which left the evidence misidentifying its producer). The blank
/// version caveat of the record it replaces is retracted, and the new record's
/// caveat (if any) recorded, so the version and its caveat never disagree.
pub fn record(tools: &mut Vec<ExternalTool>, caveats: &mut Vec<String>, tool: ExternalTool, caveat: Option<String>) {
    if let Some(existing) = tools.iter_mut().find(|t| t.name == tool.name && t.path == tool.path) {
        if existing.version == tool.version {
            return; // identical record (same tool, several objects): nothing new to say
        }
        let stale = version_caveat(&existing.name, &existing.path);
        caveats.retain(|c| *c != stale);
        *existing = tool;
    } else {
        tools.push(tool);
    }
    if let Some(c) = caveat {
        caveats.push(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    /// Write an executable shell stub into `dir` that prints `body` and exits 0.
    fn stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "#!/bin/sh\n{body}").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn identify_reads_the_first_version_line() {
        let dir = tempdir();
        let p = stub(&dir, "faketool", "echo 'faketool 9.9.9 (abcdef)'\necho 'second line'");
        let (tool, caveat) = identify("faketool", &p);
        assert_eq!(tool.name, "faketool");
        assert_eq!(tool.version, "faketool 9.9.9 (abcdef)");
        assert_eq!(tool.path, p.display().to_string());
        assert!(caveat.is_none());
    }

    #[test]
    fn version_line_skips_a_digitless_banner() {
        // llvm-objdump prints a URL banner first and the version on line two.
        let dir = tempdir();
        let p = stub(&dir, "llvmish", "echo 'LLVM (http://llvm.org/):'\necho '  LLVM version 22.1.6-rust'");
        assert_eq!(version_line(&p).as_deref(), Some("LLVM version 22.1.6-rust"));
    }

    #[test]
    fn identify_records_a_caveat_when_version_is_unreadable() {
        let dir = tempdir();
        // Exits non-zero: the version line cannot be read.
        let p = stub(&dir, "brokentool", "exit 1");
        let (tool, caveat) = identify("brokentool", &p);
        assert_eq!(tool.name, "brokentool");
        assert!(tool.version.is_empty());
        let c = caveat.expect("a caveat must explain the blank version");
        assert!(c.contains("brokentool"), "{c}");
        assert!(c.contains("--version"), "{c}");
    }

    #[test]
    fn record_dedups_on_name_and_path() {
        let mut tools = Vec::new();
        let mut caveats = Vec::new();
        let t = ExternalTool { name: "x".into(), path: "/bin/x".into(), version: "1".into() };
        record(&mut tools, &mut caveats, t.clone(), None);
        record(&mut tools, &mut caveats, t, None);
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn record_refreshes_version_and_retracts_stale_caveat() {
        let mut tools = Vec::new();
        let mut caveats = Vec::new();
        // First analysis: the version probe failed, so a blank version and a caveat.
        let blank = ExternalTool { name: "git".into(), path: "/usr/bin/git".into(), version: String::new() };
        record(&mut tools, &mut caveats, blank, Some(version_caveat("git", "/usr/bin/git")));
        assert_eq!(tools.len(), 1);
        assert_eq!(caveats.len(), 1);
        // Re-analysis at the same path now reads a version: it replaces the blank
        // record, and the stale "could not read version" caveat is retracted.
        let good = ExternalTool { name: "git".into(), path: "/usr/bin/git".into(), version: "git version 2.99".into() };
        record(&mut tools, &mut caveats, good, None);
        assert_eq!(tools.len(), 1, "still one record for the same path");
        assert_eq!(tools[0].version, "git version 2.99", "the fresh version replaces the blank one");
        assert!(caveats.is_empty(), "the stale unreadable-version caveat is retracted");
        // A later in-place upgrade: the version changes again, still one record.
        let upgraded = ExternalTool { name: "git".into(), path: "/usr/bin/git".into(), version: "git version 3.0".into() };
        record(&mut tools, &mut caveats, upgraded, None);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].version, "git version 3.0");
    }

    #[test]
    fn resolve_on_path_skips_a_non_executable_earlier_name() {
        let early = tempdir();
        let late = tempdir();
        // A non-executable file named `git` in the earlier PATH entry: the OS
        // skips it when executing, so resolution must skip it too.
        let ne = early.join("git");
        std::fs::write(&ne, "not a program").unwrap();
        std::fs::set_permissions(&ne, std::fs::Permissions::from_mode(0o644)).unwrap();
        // A real executable `git` in the later entry is the one that would run.
        let exe = stub(&late, "git", "echo git version 2.99.0");
        let path = std::env::join_paths([early.as_os_str(), late.as_os_str()]).unwrap();
        let got = resolve_in(&path, "git").expect("the executable git resolves");
        assert_eq!(got, std::path::absolute(&exe).unwrap_or(exe));
        assert!(got.is_absolute(), "the recorded path is absolute: {got:?}");
    }

    fn tempdir() -> PathBuf {
        butterfly_testsupport::scratch_dir("prov")
    }
}
