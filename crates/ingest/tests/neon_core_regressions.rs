// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Regression tests for core ingest security, fail-closed, and correctness findings.

#![cfg(unix)]

mod common;

use butterfly_ingest::{baseline, change, config, coverage, fsauth, gate, gcov, impact, pertest, tags, trace, worklist, IngestOptions};
use butterfly_model::*;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_dir(&format!("neon-core-{tag}")).canonicalize().unwrap()
}

fn minimal_export(src: &Path, regions: &str) -> String {
    format!(
        r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[[1,1,1,true,true,false]],"branches":[],"summary":{{}}}}],
           "functions":[{{"name":"f","count":1,"regions":{regions},"branches":[],"filenames":[{p:?}]}}]}}]}}"#,
        p = src.to_str().unwrap()
    )
}

// ---- Finding 1: Git option injection via untrusted butterfly-changes.json --

#[test]
fn poisoned_changes_cache_cannot_inject_git_output_option() {
    let dir = scratch("git-opt-inj");
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .args(args)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    run(&["init", "-q"]);
    let src = dir.join("a.c");
    let code = "int f(void) {\n  return 1;\n}\n";
    fs::write(&src, code).unwrap();
    run(&["add", "a.c"]);
    run(&["commit", "-q", "-m", "ABC-1: initial"]);
    let head = run(&["rev-parse", "HEAD"]);

    let victim_show = dir.join("victim-show.txt");
    let victim_diff = dir.join("victim-diff.txt");

    let cov_path = dir.join("coverage.json");
    fs::write(&cov_path, minimal_export(&src, "[[1,1,3,2,1,0,0,0]]")).unwrap();
    let mut ev = butterfly_ingest::ingest(&IngestOptions { coverage_json: cov_path, source_root: Some(dir.clone()), ..Default::default() }).unwrap();

    // Case 1: malicious commit sha `--output=<victim>` with files: None
    let poisoned_show = change::Cache {
        schema: CHANGES_SCHEMA.into(),
        repo: dir.display().to_string(),
        head: head.clone(),
        since: None,
        since_sha: None,
        range: vec![],
        created_unix: 0,
        patterns: vec![],
        commits: vec![change::CachedCommit {
            sha: format!("--output={}", victim_show.display()),
            author: "a".into(),
            date: "2026-01-01".into(),
            date_unix: 1,
            subject: "s".into(),
            message: "m".into(),
            files: None,
        }],
        files: vec![change::CachedFile {
            path: ev.files[0].path.clone(),
            repo_path: "a.c".into(),
            sha256: ev.files[0].sha256.clone(),
            status: "matches-head".into(),
            functions: vec![change::CachedFn {
                name: ev.functions[0].name.clone(),
                line_start: ev.functions[0].line_start,
                line_end: ev.functions[0].line_end,
                commits: vec![format!("--output={}", victim_show.display())],
                origin: Some(head.clone()),
            }],
        }],
        deleted: vec![],
    };

    let opts = change::Options { since: None, repo: Some(dir.clone()), patterns: vec![], exclude: vec![], jobs: 1, baseline: None };
    let (_cache, out) = change::analyse(&mut ev, &opts, Some(&poisoned_show)).unwrap();
    assert!(!victim_show.exists(), "git show --output must not create arbitrary file");
    assert_eq!(out.reused, 0, "poisoned cache must be rejected, not reused");

    // Case 2: malicious cache.head `--output=<victim>`
    let mut poisoned_diff = poisoned_show;
    poisoned_diff.head = format!("--output={}", victim_diff.display());
    poisoned_diff.commits.clear();
    let _ = change::analyse(&mut ev, &opts, Some(&poisoned_diff)).unwrap();
    let created_diff_victim = fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("victim-diff.txt"));
    assert!(!created_diff_victim, "git diff --output must not create arbitrary file");
}

// ---- Finding 2: Unvalidated coverage line ranges (panics / 4B loop / OOM) --

#[test]
fn invalid_coverage_function_line_ranges_are_rejected_without_panic_or_oom() {
    let dir = scratch("line-range");
    let src = dir.join("a.c");
    fs::write(&src, "int f(void) {\n  return 1;\n}\n").unwrap();

    // line_start == 0
    let c0 = dir.join("c0.json");
    fs::write(&c0, minimal_export(&src, "[[0,1,2,2,1,0,0,0]]")).unwrap();
    assert!(butterfly_ingest::ingest(&IngestOptions { coverage_json: c0, source_root: Some(dir.clone()), ..Default::default() }).is_err());

    // inverted line_start > line_end
    let cinv = dir.join("cinv.json");
    fs::write(&cinv, minimal_export(&src, "[[3,1,1,2,1,0,0,0]]")).unwrap();
    assert!(butterfly_ingest::ingest(&IngestOptions { coverage_json: cinv, source_root: Some(dir.clone()), ..Default::default() }).is_err());

    // two functions sharing 1..=u32::MAX (would spin 4B times and OOM 64 GiB)
    let cbig = dir.join("cbig.json");
    let big_json = format!(
        r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[[1,1,1,true,true,false]],"branches":[],"summary":{{}}}}],
           "functions":[
             {{"name":"f1","count":1,"regions":[[1,1,4294967295,1,1,0,0,0]],"branches":[],"filenames":[{p:?}]}},
             {{"name":"f2","count":1,"regions":[[1,1,4294967295,1,1,0,0,0]],"branches":[],"filenames":[{p:?}]}}
           ]}}]}}"#,
        p = src.to_str().unwrap()
    );
    fs::write(&cbig, big_json).unwrap();
    let err = butterfly_ingest::ingest(&IngestOptions { coverage_json: cbig, source_root: Some(dir.clone()), ..Default::default() }).unwrap_err();
    assert!(format!("{err:#}").contains("spans lines"), "{err:#}");

    // u64 > u32::MAX must not silently truncate to 0
    let cov_overflow = minimal_export(&src, "[[4294967296,1,2,2,1,0,0,0]]");
    assert!(coverage::parse(&cov_overflow).is_err(), "line number > u32::MAX must error");

    // Direct defensive clamping in TagFinder and baseline::from_evidence
    let mut cfg = config::Config::default();
    cfg.requirements.scope = "body".into();
    let finder = cfg.finder().unwrap();
    let text = vec!["// @satisfies REQ-1".to_string(), "int f() {}".to_string()];
    assert_eq!(finder.find(&text, 0, u32::MAX), vec!["REQ-1".to_string()]);
    assert!(finder.find_lines(&text, u32::MAX, u32::MAX).is_empty());
    assert_eq!(finder.find_lines(&text, 0, u32::MAX).len(), 1);

    let mut ev = common::evidence(vec![common::file(&["int f() {}"], &[Some(1)])], vec![common::function(0, "f", 0, u32::MAX)]);
    ev.functions[0].line_start = 0;
    ev.functions[0].line_end = u32::MAX;
    let base = baseline::from_evidence(&ev);
    assert_eq!(base.functions.len(), 1);
}

// ---- Finding 3: tags.rs newline injection, --options=NONE, and hardlinks ---

#[test]
fn tags_produce_blocks_newline_injection_hardlinks_and_passes_options_none() {
    let dir = scratch("tags-sec");
    let root = dir.join("root");
    let outside = dir.join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();

    let good = root.join("good.c");
    fs::write(&good, "int ok(void) { return 1; }\n").unwrap();
    let secret = outside.join("secret.h");
    fs::write(&secret, "int leaked(void);\n").unwrap();
    // Hardlink inside root pointing outside root
    fs::hard_link(&secret, root.join("linked.h")).unwrap();
    // Directory containing a newline whose child path spells `.../a\n/<secret>`
    // so splitting on `\n` in `ctags -L -` injects the absolute path of `secret`!
    let nl_rel = secret.strip_prefix("/").unwrap();
    let nl_path = root.join("a\n").join(nl_rel);
    fs::create_dir_all(nl_path.parent().unwrap()).unwrap();
    fs::write(&nl_path, "int dummy(void);\n").unwrap();

    // Fake Universal Ctags that records argv and stdin
    let args_log = dir.join("args.txt");
    let stdin_log = dir.join("stdin.txt");
    let fake = dir.join("ctags");
    fs::write(
        &fake,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'Universal Ctags 6.1.0'; exit 0; fi\nprintf '%s\\n' \"$@\" > {a:?}\ncat > {s:?}\n",
            a = args_log.to_str().unwrap(),
            s = stdin_log.to_str().unwrap()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();

    let ev_path = dir.join("evidence.json");
    let mut ev = common::evidence(vec![common::file(&["int ok(void) { return 1; }"], &[Some(1)])], vec![common::function(0, "ok", 1, 1)]);
    ev.files[0].path = good.display().to_string();
    fs::write(&ev_path, "{}").unwrap();

    let outcome = tags::produce(&ev, &ev_path, &root, &fake).unwrap();
    let argv = fs::read_to_string(&args_log).unwrap();
    assert_eq!(argv.lines().next(), Some("--options=NONE"), "first arg must disable untrusted .ctags.d loading: {argv}");
    let stdin = fs::read_to_string(&stdin_log).unwrap();
    let fed: Vec<&str> = stdin.lines().collect();
    assert_eq!(fed, vec![good.display().to_string().as_str()], "only good.c may be handed to ctags: {fed:?}");
    assert!(!stdin.contains("secret.h"), "neither newline-injected nor hardlinked secret may reach ctags: {stdin}");
    assert!(outcome.outside.len() >= 2, "both hardlinked and newline-named headers must be reported outside: {:?}", outcome.outside);
}

// ---- Finding 4: Discovered hardlink, untrusted config error, object symlink -

#[test]
fn discovered_hardlinks_and_untrusted_config_values_and_object_symlinks_are_refused() {
    let dir = scratch("disc-hardlink");
    let outside = scratch("disc-hardlink-outside");
    let src = dir.join("a.c");
    fs::write(&src, "int f(void) {\n  return 1;\n}\n").unwrap();
    fs::write(dir.join("coverage.json"), minimal_export(&src, "[[1,1,3,2,1,0,0,0]]")).unwrap();
    let secret = outside.join("creds");
    fs::write(&secret, "aws_secret_access_key=SECRET-HARDLINK-248\n").unwrap();

    // 1. open_discovered refuses hardlinks and symlinks
    let hl = dir.join("build-info.txt");
    fs::hard_link(&secret, &hl).unwrap();
    assert!(fsauth::open_discovered(&hl).is_err(), "open_discovered must refuse hard-linked file");
    let err = format!("{:#}", butterfly_ingest::discover_artifacts(&dir).map(|_| ()).unwrap_err());
    assert!(err.contains("hard link"), "discover_artifacts must refuse hard-linked build-info.txt: {err}");
    assert!(!err.contains("SECRET-HARDLINK-248"));

    // 2. Post-discovery swap to hardlink is refused at ingest read time
    fs::remove_file(&hl).unwrap();
    fs::write(&hl, "compiler=clang\n").unwrap();
    let mut opts = butterfly_ingest::discover(&dir).unwrap();
    opts.source_root = Some(dir.clone());
    fs::remove_file(&hl).unwrap();
    fs::hard_link(&secret, &hl).unwrap();
    let err = format!("{:#}", butterfly_ingest::ingest(&opts).unwrap_err());
    assert!(err.contains("hard link"), "ingest must refuse post-discovery hardlink swap: {err}");
    assert!(!err.contains("SECRET-HARDLINK-248"));
    fs::remove_file(&hl).unwrap();

    // 3. Discovered butterfly.toml parse error uses parse_untrusted (no value leak)
    fs::write(dir.join("butterfly.toml"), "requirements = \"PRIVATE_CFG_TOKEN_346\"\n").unwrap();
    let mut opts = butterfly_ingest::discover(&dir).unwrap();
    opts.source_root = Some(dir.clone());
    let err = format!("{:#}", butterfly_ingest::ingest(&opts).unwrap_err());
    assert!(err.contains("not a valid butterfly config"), "expected untrusted error: {err}");
    assert!(!err.contains("PRIVATE_CFG_TOKEN_346"), "discovered config value must not leak: {err}");
    fs::remove_file(dir.join("butterfly.toml")).unwrap();

    // 4. Discovered object swapped for symlink after discovery is refused at load
    let obj_target = outside.join("real.o");
    fs::write(&obj_target, b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0").unwrap();
    let obj = dir.join("app.bin");
    symlink(&obj_target, &obj).unwrap();
    let opts =
        IngestOptions { coverage_json: dir.join("coverage.json"), source_root: Some(dir.clone()), objects: vec![obj.clone()], found_by_name: vec![obj], ..Default::default() };
    let err = format!("{:#}", butterfly_ingest::ingest(&opts).unwrap_err());
    assert!(err.contains("symlink"), "discovered object symlink must be refused: {err}");
}

// ---- Finding 5: gate::regressions unplaced MC/DC & duplicate static twins --

#[test]
fn gate_regressions_fails_closed_on_unplaced_mcdc_and_tracks_static_twins() {
    let dec = |ok: bool| Decision {
        line: 2,
        col: 5,
        line_end: 2,
        col_end: 15,
        true_count: 1,
        false_count: 1,
        conditions: vec![
            Condition { index: 0, line: 2, col: 5, true_count: 1, false_count: 1, independence_covered: true, pair: None },
            Condition { index: 1, line: 2, col: 10, true_count: 1, false_count: 1, independence_covered: ok, pair: None },
        ],
        vectors: vec![],
        mcdc_covered: ok,
        decision_ordinal: None,
    };

    // Unplaced MC/DC regression fail-closed: prior 3/4 (75%), now 2/2 (100%) + 1 unplaced
    let mut prior = common::evidence(vec![common::file(&["a", "b", "c"], &[Some(1), Some(1), Some(1)])], vec![common::function(0, "f", 1, 3)]);
    prior.functions[0].decisions = vec![dec(true), dec(false)];
    let mut cur = prior.clone();
    cur.functions[0].decisions.pop();
    cur.summary.decisions_unplaced = 1;
    let breaches = gate::regressions(&cur, &Resolutions::default(), &prior, &Resolutions::default());
    assert!(breaches.iter().any(|b| b.kind == "mcdc-incomplete"), "unplaced MC/DC must block regression gate: {breaches:?}");

    // Same-named static twins in two files: regression in the FIRST twin must not be overwritten
    let mut f0 = common::file(&["a", "b", "c"], &[Some(1), Some(1), Some(1)]);
    f0.id = 0;
    f0.path = "/src/alpha/dup.c".into();
    f0.display = "dup.c".into();
    f0.function_ids = vec![0];
    let mut f1 = common::file(&["a", "b", "c"], &[Some(1), Some(1), Some(1)]);
    f1.id = 1;
    f1.path = "/src/beta/dup.c".into();
    f1.display = "dup.c".into();
    f1.function_ids = vec![1];
    let mut fn0 = common::function(0, "dup.c:tally", 1, 3);
    fn0.file_id = 0;
    fn0.lines_total = 3;
    fn0.lines_covered = 3;
    let mut fn1 = common::function(1, "dup.c:tally", 1, 3);
    fn1.file_id = 1;
    fn1.lines_total = 3;
    fn1.lines_covered = 3;
    let prior_twins = common::evidence(vec![f0, f1], vec![fn0, fn1]);
    let mut cur_twins = prior_twins.clone();
    cur_twins.functions[0].lines_covered = 2; // first twin regressed; second twin unchanged
    let breaches = gate::regressions(&cur_twins, &Resolutions::default(), &prior_twins, &Resolutions::default());
    assert!(breaches.iter().any(|b| b.kind == "regression-function"), "regression in first static twin must be caught: {breaches:?}");
}

// ---- Finding 6: coverage::line_counts with line==0 and inverted regions ----

#[test]
fn line_counts_survives_line_zero_segment_and_inverted_regions() {
    let f = coverage::CovFile {
        filename: "a.c".into(),
        direct_lines: None,
        segments: vec![
            coverage::Segment { line: 0, col: 1, count: 0, has_count: false, is_region_entry: false, is_gap: false },
            coverage::Segment { line: 1, col: 1, count: 4, has_count: true, is_region_entry: true, is_gap: false },
            coverage::Segment { line: 3, col: 1, count: 0, has_count: false, is_region_entry: false, is_gap: false },
        ],
    };
    assert_eq!(coverage::line_counts(&f, 4), vec![Some(4), Some(4), Some(4), None]);

    let rec = coverage::CovFunction {
        name: "f".into(),
        count: 1,
        regions: vec![
            coverage::Region { line_start: 0, col_start: 1, line_end: 1, col_end: 1, count: 1, file_id: 0, expanded_file_id: 0, kind: coverage::KIND_CODE },
            coverage::Region { line_start: 5, col_start: 1, line_end: 2, col_end: 1, count: 9, file_id: 0, expanded_file_id: 0, kind: coverage::KIND_CODE },
            coverage::Region { line_start: 1, col_start: 1, line_end: 3, col_end: 2, count: 7, file_id: 0, expanded_file_id: 0, kind: coverage::KIND_CODE },
        ],
        branches: vec![],
        mcdc: vec![],
        filenames: vec!["a.c".into()],
    };
    assert_eq!(coverage::records_line_counts([&rec], "a.c", 3), vec![Some(7), Some(7), Some(7)]);
}

// ---- Finding 7: Multi-decision macro disambiguation ------------------------

#[test]
fn multi_decision_macro_gets_distinct_ordinals_in_ingest_gate_worklist_and_pertest() {
    let dir = scratch("macro-ord");
    let src = dir.join("m.c");
    fs::write(
        &src,
        "#define TWO(a,b,c,d) do { if ((a)&&(b)) r|=1; if ((c)&&(d)) r|=2; } while(0)\n\
         int f(int a,int b,int c,int d){\n\
           int r=0;\n\
           TWO(a,b,c,d);\n\
           return r;\n\
         }\n",
    )
    .unwrap();
    let p = src.to_str().unwrap();
    // Two instantiations of `f` (same name/file/range) using macro expansion file 1 at line 4 col 3.
    // Decision #0 (line 1 col 30..39): covered in inst 1 (true, false), uncovered in inst 2.
    // Decision #1 (line 1 col 50..59): uncovered in inst 1 (false, false), covered in inst 2 (false, true).
    // Without decision_ordinal, inst 2's Decision #1 merges into Decision #0 and falsely makes #0 covered!
    let merged_json = format!(
        r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[[2,1,2,true,true,false],[6,2,0,false,false,false]],"branches":[],"summary":{{}}}}],
          "functions":[
            {{"name":"f","count":1,"filenames":[{p:?},{p:?}],
              "regions":[[2,1,6,2,1,0,0,0],[4,3,4,15,1,0,1,1],[1,22,1,76,1,1,0,0]],
              "branches":[
                [1,31,1,33,1,0,1,0,6],[1,36,1,38,1,0,1,0,6],
                [1,51,1,53,1,0,1,0,6],[1,56,1,58,0,1,1,0,6]
              ],
              "mcdc_records":[
                [1,30,1,39,1,0,1,0,5,[true,false],[{{"conditions":[true,true],"executed":true,"result":true}}]],
                [1,50,1,59,0,1,1,0,5,[false,false],[{{"conditions":[true,false],"executed":true,"result":false}}]]
              ]}},
            {{"name":"f","count":1,"filenames":[{p:?},{p:?}],
              "regions":[[2,1,6,2,1,0,0,0],[4,3,4,15,1,0,1,1],[1,22,1,76,1,1,0,0]],
              "branches":[
                [1,31,1,33,0,1,1,0,6],[1,36,1,38,0,1,1,0,6],
                [1,51,1,53,0,1,1,0,6],[1,56,1,58,1,0,1,0,6]
              ],
              "mcdc_records":[
                [1,30,1,39,0,1,1,0,5,[false,false],[{{"conditions":[false,null],"executed":true,"result":false}}]],
                [1,50,1,59,1,0,1,0,5,[false,true],[{{"conditions":[true,true],"executed":true,"result":true}}]]
              ]}}
          ]}}]}}"#
    );
    let cov_path = dir.join("coverage.json");
    fs::write(&cov_path, &merged_json).unwrap();
    let ev = butterfly_ingest::ingest(&IngestOptions { coverage_json: cov_path, source_root: Some(dir.clone()), ..Default::default() }).unwrap();
    assert_eq!(ev.functions.len(), 1);
    let f = &ev.functions[0];
    assert_eq!(f.decisions.len(), 2);
    assert_eq!(f.decisions[0].decision_ordinal, Some(0));
    assert_eq!(f.decisions[1].decision_ordinal, Some(1));
    assert_eq!(f.decisions[0].site(), "4:#0");
    assert_eq!(f.decisions[1].site(), "4:#1");
    // Decision #0 must NOT inherit Decision #1's condition-1 independence!
    assert!(!f.decisions[0].conditions[1].independence_covered, "instantiation merge must not bleed #1 into #0");
    assert!(f.decisions[1].conditions[1].independence_covered);
    // Branches also carry distinct ordinals
    let b_ords: Vec<Option<u32>> = f.branches.iter().map(|b| b.decision_ordinal).collect();
    assert_eq!(b_ords, vec![Some(0), Some(1), Some(2), Some(3)]);

    // Gate & worklist: justifying cond:0:4:#0:1 must NOT satisfy Decision #1's cond 1 (or #1's cond 0)
    let res = Resolutions {
        schema: RESOLUTIONS_SCHEMA.into(),
        entries: vec![Resolution { key: "cond:0:4:#0:1".into(), category: "x".into(), justification: "j".into(), author: "a".into(), date: "2026-01-01".into() }],
    };
    let th = gate::Thresholds { min_mcdc: Some(100.0), ..Default::default() };
    let breaches = gate::evaluate(&ev, &res, &th);
    assert!(breaches.iter().any(|b| b.kind == "mcdc"), "justifying #0 must not justify #1: {breaches:?}");
    let wl = worklist::compute(&ev, &res);
    assert_eq!(wl.len(), 1, "only decision #1 remains open in worklist: {wl:?}");

    // Per-test vector attribution must match Decision #1's vector onto decision index 1 (not 0)
    let t_json = format!(
        r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[[2,1,1,true,true,false],[6,2,0,false,false,false]],"branches":[],"summary":{{}}}}],
          "functions":[
            {{"name":"f","count":1,"filenames":[{p:?},{p:?}],
              "regions":[[2,1,6,2,1,0,0,0],[4,3,4,15,1,0,1,1],[1,22,1,76,1,1,0,0]],
              "branches":[
                [1,31,1,33,1,0,1,0,6],[1,36,1,38,1,0,1,0,6],
                [1,51,1,53,1,0,1,0,6],[1,56,1,58,0,1,1,0,6]
              ],
              "mcdc_records":[
                [1,30,1,39,1,0,1,0,5,[false,false],[{{"conditions":[true,true],"executed":true,"result":true}}]],
                [1,50,1,59,0,1,1,0,5,[false,false],[{{"conditions":[true,false],"executed":true,"result":false}}]]
              ]}}
          ]}}]}}"#
    );
    let (_run, matches) = pertest::attribute(&ev, 0, "t1", Path::new("t1.json"), &t_json).expect("per-test must match #0 and #1 separately");
    assert!(matches.iter().any(|m| m.decision == 0), "{matches:?}");
    assert!(matches.iter().any(|m| m.decision == 1), "{matches:?}");
}

// ---- Finding 8: Duplicate static twins in impact::compute & pertest --------

#[test]
fn duplicate_static_twins_do_not_hide_stale_reviews_or_misattribute_tests() {
    let mut f0 = common::file(&["int tally(void) {", "  return 1;", "}"], &[Some(2), Some(2), Some(2)]);
    f0.id = 0;
    f0.path = "/src/alpha/dup.c".into();
    f0.display = "dup.c".into();
    f0.function_ids = vec![0];
    let mut f1 = common::file(&["int tally(void) {", "  return 2;", "}"], &[Some(0), Some(0), Some(0)]);
    f1.id = 1;
    f1.path = "/src/beta/dup.c".into();
    f1.display = "dup.c".into();
    f1.function_ids = vec![1];
    let mut fn0 = common::function(0, "dup.c:tally", 1, 3);
    fn0.file_id = 0;
    fn0.count = 2;
    fn0.commits = vec![0]; // alpha changed
    let mut fn1 = common::function(1, "dup.c:tally", 1, 3);
    fn1.file_id = 1;
    fn1.count = 0; // beta unchanged & never executed
    let mut ev = common::evidence(vec![f0, f1], vec![fn0, fn1]);
    ev.changes = Some(Changes {
        repo: "/src".into(),
        head: "a".repeat(40),
        since: Some("b".repeat(40)),
        since_sha: Some("b".repeat(40)),
        commits: vec![Commit { sha: "a".repeat(40), short: "aaaaaaaaaaaa".into(), subject: "ABC-1".into(), tickets: vec![0], functions: vec![0], ..Default::default() }],
        tickets: vec![Ticket { id: "ABC-1".into(), commits: vec![0], functions: vec![0], modified: vec![0], ..Default::default() }],
        ..Default::default()
    });
    let res = Resolutions {
        schema: RESOLUTIONS_SCHEMA.into(),
        entries: vec![Resolution { key: "s2o:dup.c:tally".into(), category: "ok".into(), justification: "reviewed".into(), author: "r".into(), date: "2026-01-01".into() }],
    };
    let imp = impact::compute(&ev, &res).unwrap();
    assert!(imp.obligations.iter().any(|o| o.kind == "stale-review" && o.function_id == Some(0)), "stale review for changed twin must fire: {:?}", imp.obligations);

    // Per-test export executes alpha/dup.c:tally (count=1); beta has count=0 in aggregate.
    // Previously fn_of mapped "dup.c:tally" -> beta (id 1) and falsely rejected or misattributed!
    let t_json = r#"{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{"files":[
        {"filename":"/src/alpha/dup.c","segments":[[1,1,1,true,true,false]],"branches":[],"summary":{}},
        {"filename":"/src/beta/dup.c","segments":[[1,1,0,true,true,false]],"branches":[],"summary":{}}
    ],"functions":[
        {"name":"dup.c:tally","count":1,"regions":[[1,1,3,2,1,0,0,0]],"branches":[],"filenames":["/src/alpha/dup.c"]}
    ]}]}"#;
    let (run, _) = pertest::attribute(&ev, 0, "t1", Path::new("t1.json"), t_json).expect("must resolve alpha twin by file+line");
    assert_eq!(run.functions, vec![0], "must attribute to alpha (fn 0), not beta (fn 1)");
}

// ---- Finding 9: baseline::compare removed build_info & export version ------

#[test]
fn baseline_compare_invalidates_on_removed_toolchain_key_and_export_version_change() {
    let mut ev = common::evidence(vec![common::file(&["int f() { return 1; }"], &[Some(1)])], vec![common::function(0, "f", 1, 1)]);
    ev.provenance.coverage_export_version = "2.0.1".into();
    ev.provenance.build_info.insert("linker".into(), "lld 18".into());
    let base = baseline::from_evidence(&ev);

    let mut dropped = ev.clone();
    dropped.provenance.build_info.remove("linker");
    let chk = baseline::compare(&base, &dropped, "b.json");
    assert_eq!(chk.status, "changed", "removing a recorded toolchain key must invalidate: {chk:?}");
    assert!(chk.differences.iter().any(|d| d.kind == "toolchain" && d.invalidates && d.detail.contains("linker")));

    let mut ver_changed = ev.clone();
    ver_changed.provenance.coverage_export_version = "3.0.0".into();
    let chk2 = baseline::compare(&base, &ver_changed, "b.json");
    assert_eq!(chk2.status, "changed", "changing coverage_export_version must invalidate: {chk2:?}");
}

// ---- Finding 10: gcov::parse multiple `lines` entries for one line_number --

#[test]
fn gcov_parse_groups_branches_across_duplicate_line_entries() {
    let json = r#"{
      "gcc_version": "14.2.0",
      "format_version": "2",
      "current_working_directory": "/w",
      "files": [{
        "file": "a.c",
        "functions": [{"name": "f", "start_line": 1, "end_line": 5, "execution_count": 2}],
        "lines": [
          {"line_number": 3, "count": 2, "branches": [
            {"source_block_id": 10, "destination_block_id": 11, "count": 1, "fallthrough": true, "throw": false}
          ]},
          {"line_number": 3, "count": 2, "branches": [
            {"source_block_id": 10, "destination_block_id": 12, "count": 1, "fallthrough": false, "throw": false},
            {"source_block_id": 20, "destination_block_id": 21, "count": 2, "fallthrough": true, "throw": false},
            {"source_block_id": 20, "destination_block_id": 22, "count": 0, "fallthrough": false, "throw": false}
          ]}
        ]
      }]
    }"#;
    let cov = gcov::parse(json).unwrap();
    let bs = &cov.functions[0].branches;
    assert_eq!(bs.len(), 2, "both decisions must be recovered across duplicate line entries: {bs:?}");
    assert_eq!(bs[0].decision_ordinal, Some(0));
    assert_eq!(bs[1].decision_ordinal, Some(1));
}

// ---- Finding 11: change.rs SHA-256 git repository deletion tracking --------

#[test]
fn change_analyse_locates_deleted_function_in_sha256_git_repo() {
    let dir = scratch("sha256-del");
    let init = Command::new("git").current_dir(&dir).args(["init", "-q", "--object-format=sha256"]).output();
    let Ok(init) = init else { return };
    if !init.status.success() {
        return; // older git without sha256 object-format support
    }
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .args(args)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let src = dir.join("a.c");
    fs::write(&src, "int keep(void) {\n  return 1;\n}\nint gone_fn(void) {\n  return 2;\n}\n").unwrap();
    run(&["add", "a.c"]);
    run(&["commit", "-q", "-m", "ABC-1: add both"]);
    let c1 = run(&["rev-parse", "HEAD"]);
    assert_eq!(c1.len(), 64, "expected SHA-256 commit id");

    // Freeze baseline with both functions
    let cov1 = dir.join("c1.json");
    let p = src.to_str().unwrap();
    fs::write(
        &cov1,
        format!(
            r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[[1,1,1,true,true,false]],"branches":[],"summary":{{}}}}],
              "functions":[
                {{"name":"keep","count":1,"regions":[[1,1,3,2,1,0,0,0]],"branches":[],"filenames":[{p:?}]}},
                {{"name":"gone_fn","count":1,"regions":[[4,1,6,2,1,0,0,0]],"branches":[],"filenames":[{p:?}]}}
              ]}}]}}"#
        ),
    )
    .unwrap();
    let ev1 = butterfly_ingest::ingest(&IngestOptions { coverage_json: cov1, source_root: Some(dir.clone()), ..Default::default() }).unwrap();
    let base = baseline::from_evidence(&ev1);

    // Delete gone_fn
    fs::write(&src, "int keep(void) {\n  return 1;\n}\n").unwrap();
    run(&["add", "a.c"]);
    run(&["commit", "-q", "-m", "ABC-2: delete gone_fn"]);
    let c2 = run(&["rev-parse", "HEAD"]);

    let cov2 = dir.join("c2.json");
    fs::write(
        &cov2,
        format!(
            r#"{{"version":"3.1.0","type":"llvm.coverage.json.export","data":[{{"files":[{{"filename":{p:?},"segments":[[1,1,1,true,true,false]],"branches":[],"summary":{{}}}}],
              "functions":[{{"name":"keep","count":1,"regions":[[1,1,3,2,1,0,0,0]],"branches":[],"filenames":[{p:?}]}}]}}]}}"#
        ),
    )
    .unwrap();
    let mut ev2 = butterfly_ingest::ingest(&IngestOptions { coverage_json: cov2, source_root: Some(dir.clone()), ..Default::default() }).unwrap();
    let opts = change::Options { since: Some(c1), repo: Some(dir.clone()), patterns: vec![r"\b([A-Z]+-\d+)\b".into()], exclude: vec![], jobs: 1, baseline: None };
    let (_cache, outcome) = change::analyse_with_baseline(&mut ev2, &opts, None, Some(&base)).unwrap();
    assert_eq!(outcome.deleted, 1);
    let ch = ev2.changes.as_ref().unwrap();
    assert_eq!(ch.deleted.len(), 1);
    let del_ci = ch.deleted[0].commit.expect("deleting SHA-256 commit must be located");
    assert_eq!(ch.commits[del_ci].sha, c2);
}

// ---- Finding 13: trace::parse pre-header plain-text leak -------------------

#[test]
fn qemu_trace_does_not_keep_pre_header_hex_word_matches() {
    let log = "Dead banner line\nAdd another line\n0x1234 startup\n\
               IN: start\n0x4000:  d503201f  nop\n\n\
               Trace 0: 0x1 [0/0000000000004000/0/0] start\n";
    let mut t = trace::parse(log);
    assert_eq!(t.format, "qemu -d in_asm,exec");
    assert!(t.counts.is_empty(), "pre-header lines must not pollute QEMU counts: {:?}", t.counts);
    t.expand(&|_| vec![]);
    assert_eq!(t.counts.len(), 1);
    assert_eq!(t.counts.get(&0x4000), Some(&1));
}
