// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! A report must never quietly claim there were no reviews.
//!
//! Every command that reads the resolutions sidecar has to fail closed: an
//! explicitly named file that cannot be read, cannot be parsed, or is not a
//! resolutions document is an error, because the alternative is a verification
//! report that silently omits the justifications a reviewer signed.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

fn evidence() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c/build/evidence.json");
    if p.exists() {
        Some(p.canonicalize().unwrap())
    } else {
        butterfly_testsupport::fixture_missing("build examples/c first");
        None
    }
}

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("res-{tag}"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_butterfly")).args(args).output().expect("spawn butterfly")
}
fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

const GOOD: &str = r#"{"schema":"butterfly-resolutions/1","entries":[
  {"key":"s2o:main","category":"traceable","justification":"reviewed","author":"me","date":"2026-01-01"}]}"#;

#[test]
fn malformed_explicit_resolutions_fail_the_report_and_name_the_file() {
    let Some(ev) = evidence() else { return };
    let d = scratch("malformed");
    let bad = d.join("bad-resolutions.json");
    fs::write(&bad, "not json").unwrap();
    let out = d.join("r.html");
    let o = run(&["report", ev.to_str().unwrap(), "--resolutions", bad.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert!(!o.status.success(), "a malformed sidecar must not produce a report: {}", err(&o));
    assert!(err(&o).contains("bad-resolutions.json"), "the error names the file: {}", err(&o));
    assert!(!out.exists(), "no report is written when the reviews could not be read");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn an_unsupported_resolution_schema_fails() {
    let Some(ev) = evidence() else { return };
    let d = scratch("schema");
    let p = d.join("resolutions.json");
    fs::write(&p, r#"{"schema":"butterfly-resolutions/99","entries":[]}"#).unwrap();
    let o = run(&["report", ev.to_str().unwrap(), "--resolutions", p.to_str().unwrap(), "-o", d.join("r.html").to_str().unwrap()]);
    assert!(!o.status.success(), "an unknown schema must not be read as empty: {}", err(&o));
    assert!(err(&o).contains("butterfly-resolutions/99"), "the error names what it found: {}", err(&o));
    let _ = fs::remove_dir_all(d);
}

#[test]
fn an_explicit_path_that_does_not_exist_fails() {
    let Some(ev) = evidence() else { return };
    let d = scratch("missing");
    let o = run(&["report", ev.to_str().unwrap(), "--resolutions", d.join("nope.json").to_str().unwrap(), "-o", d.join("r.html").to_str().unwrap()]);
    assert!(!o.status.success(), "asking for a specific sidecar that is absent is an error, not an empty set: {}", err(&o));
    assert!(err(&o).contains("nope.json"), "{}", err(&o));
    let _ = fs::remove_dir_all(d);
}

#[test]
fn an_unreadable_explicit_path_fails() {
    let Some(ev) = evidence() else { return };
    if unsafe { libc_geteuid() } == 0 {
        butterfly_testsupport::not_applicable("running as root, permissions do not apply");
        return;
    }
    let d = scratch("perm");
    let p = d.join("resolutions.json");
    fs::write(&p, GOOD).unwrap();
    let mut perms = fs::metadata(&p).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o000);
    }
    fs::set_permissions(&p, perms).unwrap();
    let o = run(&["report", ev.to_str().unwrap(), "--resolutions", p.to_str().unwrap(), "-o", d.join("r.html").to_str().unwrap()]);
    assert!(!o.status.success(), "an unreadable sidecar must not become an empty review set: {}", err(&o));
    assert!(err(&o).contains("resolutions.json"), "{}", err(&o));
    let _ = fs::remove_dir_all(d);
}

#[test]
fn a_valid_sidecar_is_read_and_reaches_the_report() {
    let Some(ev) = evidence() else { return };
    let d = scratch("good");
    let p = d.join("resolutions.json");
    fs::write(&p, GOOD).unwrap();
    let out = d.join("r.html");
    let o = run(&["report", ev.to_str().unwrap(), "--resolutions", p.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert!(o.status.success(), "{}", err(&o));
    let html = fs::read_to_string(&out).unwrap();
    assert!(html.contains("traceable") && html.contains("reviewed"), "the justification appears in the report");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn a_missing_default_sidecar_is_fine_and_says_so() {
    let Some(ev) = evidence() else { return };
    let d = scratch("default");
    // copy the evidence somewhere with no sidecar beside it
    let ev2 = d.join("evidence.json");
    fs::copy(&ev, &ev2).unwrap();
    let out = d.join("r.html");
    let o = Command::new(env!("CARGO_BIN_EXE_butterfly")).args(["report", ev2.to_str().unwrap(), "-o", out.to_str().unwrap()]).current_dir(&d).output().unwrap();
    assert!(o.status.success(), "no sidecar at all is a normal first run: {}", err(&o));
    assert!(err(&o).to_lowercase().contains("no resolutions"), "and it says so rather than staying silent: {}", err(&o));
    assert!(out.exists());
    let _ = fs::remove_dir_all(d);
}

/// `--resolutions resolutions.json` used to be indistinguishable from the
/// flag being omitted: the argument carried `resolutions.json` as its clap
/// default, so the code inferred explicitness by comparing the parsed value
/// against that same string. Naming the default filename explicitly there-
/// fore failed open -- exit 0, "no resolutions sidecar found", and a
/// complete-looking report (review of #19).
#[test]
fn explicitly_naming_the_default_filename_still_has_to_load() {
    let Some(ev) = evidence() else { return };
    let d = scratch("explicit-default-name");
    let out = d.join("r.html");
    // cwd has no resolutions.json, and neither does the evidence directory
    let o = Command::new(env!("CARGO_BIN_EXE_butterfly"))
        .args(["report", ev.to_str().unwrap(), "--resolutions", "resolutions.json", "-o", out.to_str().unwrap()])
        .current_dir(&d)
        .output()
        .unwrap();
    assert!(!o.status.success(), "a sidecar the operator named must load, whatever it is called: {}", err(&o));
    assert!(err(&o).contains("resolutions.json"), "the error names the file: {}", err(&o));
    assert!(!out.exists(), "no report is written when the named reviews could not be read");
    let _ = fs::remove_dir_all(d);
}

/// Discovery prefers the sidecar beside the evidence over the one in the
/// working directory. The same defect broke this: a `resolutions.json` in
/// the current directory was treated as an explicit path and won, so a
/// report could be built from another project's decisions (review of #19).
#[test]
fn the_sidecar_beside_the_evidence_wins_over_the_working_directory() {
    let Some(ev) = evidence() else { return };
    let d = scratch("precedence");
    let beside = d.join("beside");
    let cwd = d.join("cwd");
    fs::create_dir_all(&beside).unwrap();
    fs::create_dir_all(&cwd).unwrap();

    let ev2 = beside.join("evidence.json");
    fs::copy(&ev, &ev2).unwrap();
    fs::write(beside.join("resolutions.json"), GOOD.replace("reviewed", "beside-the-evidence")).unwrap();
    fs::write(cwd.join("resolutions.json"), GOOD.replace("reviewed", "in-the-working-directory")).unwrap();

    let out = d.join("r.html");
    let o = Command::new(env!("CARGO_BIN_EXE_butterfly")).args(["report", ev2.to_str().unwrap(), "-o", out.to_str().unwrap()]).current_dir(&cwd).output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    let html = fs::read_to_string(&out).unwrap();
    assert!(html.contains("beside-the-evidence"), "the evidence's own decisions are the ones reported");
    assert!(!html.contains("in-the-working-directory"), "another project's decisions must not be picked up: {}", err(&o));
    let _ = fs::remove_dir_all(d);
}

#[cfg(unix)]
unsafe fn libc_geteuid() -> u32 {
    extern "C" {
        fn geteuid() -> u32;
    }
    geteuid()
}
#[cfg(not(unix))]
unsafe fn libc_geteuid() -> u32 {
    1
}

/// Discovery asked `exists()`, which follows the link and answers "no" for a
/// dangling one. A `resolutions.json` whose target had been removed was
/// therefore not "a broken sidecar" but "no sidecar at all": exit 0, "no
/// resolutions sidecar found", and a report with none of the recorded review
/// decisions in it (review of #19).
#[cfg(unix)]
#[test]
fn a_discovered_dangling_sidecar_is_an_error_not_an_empty_review_set() {
    let Some(ev) = evidence() else { return };
    for beside_the_evidence in [true, false] {
        let d = scratch(if beside_the_evidence { "dangling-beside" } else { "dangling-cwd" });
        let evdir = d.join("ev");
        let cwd = d.join("cwd");
        fs::create_dir_all(&evdir).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let ev2 = evdir.join("evidence.json");
        fs::copy(&ev, &ev2).unwrap();

        // a sidecar that is there, but points at nothing
        let at = if beside_the_evidence { &evdir } else { &cwd };
        std::os::unix::fs::symlink(d.join("target-was-removed.json"), at.join("resolutions.json")).unwrap();

        let out = d.join("r.html");
        let o = Command::new(env!("CARGO_BIN_EXE_butterfly")).args(["report", ev2.to_str().unwrap(), "-o", out.to_str().unwrap()]).current_dir(&cwd).output().unwrap();
        assert!(!o.status.success(), "a broken sidecar is not an absent one: {}", err(&o));
        assert!(err(&o).contains("resolutions.json"), "the error names the file: {}", err(&o));
        assert!(!err(&o).to_lowercase().contains("no resolutions sidecar found"), "and does not claim there were none: {}", err(&o));
        assert!(!out.exists(), "no report is written when the reviews could not be read");
        let _ = fs::remove_dir_all(d);
    }
}

/// The other half: nothing there at all is still a normal first run, and a
/// link that resolves is still read.
#[cfg(unix)]
#[test]
fn a_sidecar_reached_through_a_working_link_is_read() {
    let Some(ev) = evidence() else { return };
    let d = scratch("live-link");
    let real = d.join("elsewhere.json");
    fs::write(&real, GOOD).unwrap();
    let ev2 = d.join("evidence.json");
    fs::copy(&ev, &ev2).unwrap();
    std::os::unix::fs::symlink(&real, d.join("resolutions.json")).unwrap();

    let out = d.join("r.html");
    let o = Command::new(env!("CARGO_BIN_EXE_butterfly")).args(["report", ev2.to_str().unwrap(), "-o", out.to_str().unwrap()]).current_dir(&d).output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    assert!(fs::read_to_string(&out).unwrap().contains("reviewed"), "the decisions behind the link are in the report");
    let _ = fs::remove_dir_all(d);
}

/// The reported exploit (#66), against the real CLI paths it named: a
/// syntactically fine resolutions file, a real key, and a category/blank
/// fields that pass no field validation at all. `report`, `check` and
/// `impact` must all refuse it -- reading it as "no reviews" would silently
/// drop the malformed record and let the run "succeed" with the gap it
/// claimed to resolve counted right back as unresolved-but-invisible; the
/// correct failure is loud, not quiet.
#[test]
fn the_reported_exploit_fails_closed_on_report_check_and_impact() {
    let Some(ev) = evidence() else { return };
    let d = scratch("exploit");
    let bad = d.join("resolutions.json");
    // fn:0 is a real obligation on any evidence with at least one function --
    // deliberately not a gap key, so this test does not depend on exactly
    // which gaps this build happens to produce.
    fs::write(
        &bad,
        r#"{"schema":"butterfly-resolutions/1","entries":[
          {"key":"fn:0","category":"totally-invalid","justification":"","author":"","date":""}]}"#,
    )
    .unwrap();

    let out = d.join("r.html");
    let o = run(&["report", ev.to_str().unwrap(), "--resolutions", bad.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert!(!o.status.success(), "report must refuse the exploit record: {}", err(&o));
    assert!(err(&o).contains("category"), "the error should name what is wrong: {}", err(&o));
    assert!(!out.exists());

    let o = run(&["check", ev.to_str().unwrap(), "--resolutions", bad.to_str().unwrap(), "--max-unresolved-gaps", "999999"]);
    assert!(!o.status.success(), "check must refuse it too, whatever the threshold: {}", err(&o));

    let o = run(&["impact", ev.to_str().unwrap(), "--resolutions", bad.to_str().unwrap()]);
    assert!(!o.status.success(), "and impact: {}", err(&o));

    let _ = fs::remove_dir_all(d);
}

/// The same record, but well-formed: every field valid and the key real.
/// Proves the tests above are rejecting the *invalidity*, not merely the
/// presence of a --resolutions flag or some unrelated breakage.
#[test]
fn a_well_formed_record_over_the_same_key_is_accepted() {
    let Some(ev) = evidence() else { return };
    let d = scratch("wellformed");
    let good = d.join("resolutions.json");
    fs::write(
        &good,
        r#"{"schema":"butterfly-resolutions/1","entries":[
      {"key":"fn:0","category":"a","justification":"covered by analysis","author":"me","date":"2026-01-01"}]}"#,
    )
    .unwrap();
    let out = d.join("r.html");
    let o = run(&["report", ev.to_str().unwrap(), "--resolutions", good.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert!(o.status.success(), "a well-formed record over a real key must be accepted: {}", err(&o));
    assert!(out.exists());
    let _ = fs::remove_dir_all(d);
}

/// A rebuild moves the addresses gap keys are built from, so reviews going
/// stale is routine, not an error. Failing the load would mean a code change
/// bricks `report` and `check` -- and `impact`, whose whole job is to say
/// that reviews have gone stale. Found by self-review of the first version
/// of #66, which did exactly that.
#[test]
fn a_review_gone_stale_after_a_rebuild_warns_but_does_not_fail() {
    let Some(ev) = evidence() else { return };
    let d = scratch("stale");

    // one real gap key, justified properly
    let text = fs::read_to_string(&ev).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    let gap = parsed["functions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| f["object"]["gaps"].as_array().cloned().unwrap_or_default())
        .find_map(|g| g["key"].as_str().map(str::to_string))
        .expect("the fixture has gaps");
    let sidecar = d.join("resolutions.json");
    fs::write(
        &sidecar,
        serde_json::json!({"schema":"butterfly-resolutions/1","entries":[
            {"key": gap, "category":"analysis","justification":"compiler-generated, reviewed","author":"me","date":"2026-09-01"}]})
        .to_string(),
    )
    .unwrap();

    // ...and then a rebuild: every address-keyed gap moves. Nothing about
    // the review is wrong; it simply describes the previous build.
    let mut rebuilt: serde_json::Value = serde_json::from_str(&text).unwrap();
    for f in rebuilt["functions"].as_array_mut().unwrap() {
        let Some(gaps) = f["object"]["gaps"].as_array_mut() else { continue };
        for g in gaps {
            let Some(k) = g["key"].as_str() else { continue };
            let mut parts: Vec<&str> = k.rsplitn(3, '@').collect();
            parts.reverse(); // [symbol, kind, tail]
            if parts.len() == 3 {
                if let Ok(addr) = u64::from_str_radix(parts[2], 16) {
                    g["key"] = serde_json::Value::String(format!("{}@{}@{:x}", parts[0], parts[1], addr + 0x20));
                }
            }
        }
    }
    let rebuilt_path = d.join("evidence-rebuilt.json");
    fs::write(&rebuilt_path, rebuilt.to_string()).unwrap();

    let out = d.join("r.html");
    let o = run(&["report", rebuilt_path.to_str().unwrap(), "--resolutions", sidecar.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert!(o.status.success(), "a stale review must not fail the report: {}", err(&o));
    assert!(out.exists(), "and the report is still written");
    assert!(err(&o).contains("stale"), "but it is reported, not silent: {}", err(&o));

    let o = run(&["check", rebuilt_path.to_str().unwrap(), "--resolutions", sidecar.to_str().unwrap(), "--max-unresolved-gaps", "999"]);
    assert!(o.status.success(), "nor the gate: {}", err(&o));

    let _ = fs::remove_dir_all(d);
}
