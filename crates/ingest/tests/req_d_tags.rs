// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests, area D: the definition index (`butterfly tags`).
//!
//! Expected values come from README.md "Go to definition (optional)"
//! (README.md:138-174), not from the implementation. A stand-in ctags (a
//! shell script that answers like Universal Ctags) keeps these deterministic
//! and independent of whichever ctags the machine has.

mod common;

use butterfly_ingest::tags;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let d = butterfly_testsupport::scratch_dir(&format!("reqd-tags-{tag}"));
    d.canonicalize().unwrap()
}

/// A script that prints `version` for `--version` and otherwise runs `body`.
fn fake_ctags(dir: &Path, version: &str, body: &str) -> PathBuf {
    let p = dir.join("fake-ctags");
    fs::write(&p, format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '{version}'; exit 0; fi\ncat > /dev/null\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// Evidence for `<root>/a.c` holding `f` (lines 1-3).
fn evidence(root: &Path) -> butterfly_model::Evidence {
    let text = "int f(void) {\n    return 1;\n}\n";
    fs::write(root.join("a.c"), text).unwrap();
    fs::write(root.join("only.h"), "#define LIMIT 3\n").unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let mut f = common::file(&lines, &[Some(1), Some(1), Some(1)]);
    f.path = root.join("a.c").display().to_string();
    f.display = "a.c".into();
    f.function_ids = vec![0];
    common::evidence(vec![f], vec![common::function(0, "f", 1, 3)])
}

/// README.md:172-174 -- "The system `ctags` on macOS is BSD ctags and will not
/// do": a binary that is not Universal Ctags is refused, and an explicit
/// choice that does not exist is not swapped for another one.
#[test]
fn g01_only_universal_ctags_is_accepted() {
    let d = scratch("find");
    let bsd = fake_ctags(&d, "ctags: illegal option -- -", "exit 1");
    assert!(tags::find_ctags(Some(&bsd)).is_none(), "BSD ctags will not do");
    assert!(tags::find_ctags(Some(Path::new("/nonexistent/ctags"))).is_none(), "a missing explicit choice is refused");
    let uni = fake_ctags(&d, "Universal Ctags 6.1.0, Copyright (C) 2015-2023 Universal Ctags Team", "exit 0");
    assert_eq!(tags::find_ctags(Some(&uni)).as_deref(), Some(uni.as_path()));
    let _ = fs::remove_dir_all(&d);
}

/// README.md:146-152 -- the index links a definition back to its analysed
/// function when there is one, and a definition in a header the evidence
/// never compiled "is marked as such, rather than pretended to be one of the
/// files already on the page". Lines ctags prints that are not tags are
/// ignored.
#[test]
fn g02_analysed_functions_link_back_and_header_definitions_are_marked() {
    let d = scratch("produce");
    let src = d.join("src");
    fs::create_dir_all(&src).unwrap();
    let ev = evidence(&src);
    let a = src.join("a.c").display().to_string();
    let h = src.join("only.h").display().to_string();
    let body = format!(
        "echo '{{\"_type\": \"ptag\", \"name\": \"JSON_OUTPUT_VERSION\", \"path\": \"0.0\", \"line\": 1}}'\necho 'not json at all'\necho '{{\"_type\": \"tag\", \"name\": \"f\", \"path\": \"{a}\", \"line\": 1, \"end\": 3, \"kind\": \"function\", \"language\": \"C\"}}'\necho '{{\"_type\": \"tag\", \"name\": \"LIMIT\", \"path\": \"{h}\", \"line\": 1, \"kind\": \"macro\", \"language\": \"C\"}}'"
    );
    let ctags = fake_ctags(&d, "Universal Ctags 6.1.0", &body);
    let evp = d.join("evidence.json");
    fs::write(&evp, "{}").unwrap();
    let out = tags::produce(&ev, &evp, &src, &ctags).unwrap();
    let t = &out.tags;
    assert!(t.check_schema().is_ok());
    assert!(t.validate(ev.files.len(), ev.functions.len()).is_ok());
    assert_eq!(t.entries.len(), 2, "only the two tag records: {:?}", t.entries);
    let f = t.entries.iter().find(|e| e.name == "f").unwrap();
    assert_eq!((f.file_id, f.function_id, f.end), (Some(0), Some(0), Some(3)), "an analysed function links to its control-flow view");
    let m = t.entries.iter().find(|e| e.name == "LIMIT").unwrap();
    assert_eq!((m.file_id, m.function_id), (None, None), "a header the evidence never carried is not one of its files");
    assert_eq!(m.kind, "macro");
    assert!(!t.evidence_sha256.is_empty(), "built against this evidence");
    let _ = fs::remove_dir_all(&d);
}

/// README.md:128 ("its absence changes nothing else") -- a ctags that fails
/// is an error that says so, never an empty index passed off as complete.
#[test]
fn g03_a_failing_ctags_is_an_error() {
    let d = scratch("fail");
    let src = d.join("src");
    fs::create_dir_all(&src).unwrap();
    let ev = evidence(&src);
    let ctags = fake_ctags(&d, "Universal Ctags 6.1.0", "echo 'ctags: cannot parse' >&2\nexit 2");
    let err = tags::produce(&ev, &d.join("evidence.json"), &src, &ctags).err().expect("a failing ctags is an error");
    assert!(format!("{err:#}").contains("ctags"), "{err:#}");
    let _ = fs::remove_dir_all(&d);
}
