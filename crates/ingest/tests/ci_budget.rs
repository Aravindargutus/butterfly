// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The hosted-CI budget policy (#308), pinned so that a later edit to a
//! workflow cannot quietly undo it.
//!
//! The private repository's Actions minutes ran out. It was not the cost
//! of one run that did it (macOS was already gone, and the cache and
//! cancellation were in place) but how many ran. Every push to every pull
//! request ran the full Linux suite: work in progress that
//! `scripts/ci-local.sh` validates anyway, and pull requests stacked on
//! another branch that are tested again once retargeted to `main`. So,
//! for the path-filtered auxiliary workflows (audit, docs-numbers and showcase):
//!
//! * it starts only for a pull request whose base is `main`;
//! * every job is skipped while the pull request is a draft. A skipped
//!   job costs no minutes, and a draft cannot be merged. Marking it ready
//!   for review is a listed trigger, so that is when it runs.
//!
//! The required PR gate runs on every PR and selects the reusable full CI suite by path.

use std::path::PathBuf;
use yaml_rust2::{Yaml, YamlLoader};

fn workflows() -> Vec<(String, Yaml)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows");
    let mut out: Vec<(String, Yaml)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "yml" || x == "yaml"))
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            let doc = YamlLoader::load_from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display())).remove(0);
            (p.file_name().unwrap().to_string_lossy().into_owned(), doc)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A workflow's triggers. yaml-rust2 reads YAML 1.2, where a bare `on` key
/// is a string (YAML 1.1 would have read it as `true`).
fn triggers(doc: &Yaml) -> &Yaml {
    &doc["on"]
}

fn strings(y: &Yaml) -> Vec<String> {
    y.as_vec().map(|v| v.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

#[test]
fn a_pull_request_starts_hosted_ci_only_against_main() {
    let mut seen = 0;
    for (name, doc) in pr_workflows() {
        let pr = &triggers(&doc)["pull_request"];
        if pr.is_badvalue() {
            continue;
        }
        seen += 1;
        assert_eq!(
            strings(&pr["branches"]),
            vec!["main".to_string()],
            "{name}: `pull_request` must be limited to `branches: [main]`, so a pull request stacked on another branch does not run until it is retargeted"
        );
        let types = strings(&pr["types"]);
        for t in ["opened", "synchronize", "reopened", "ready_for_review", "edited"] {
            assert!(types.iter().any(|x| x == t), "{name}: `pull_request.types` must list {t:?} (marking a draft ready is when it runs): {types:?}");
        }
    }
    assert!(seen >= 3, "expected showcase, docs-numbers and audit to be started by pull requests; found {seen}");
}

#[test]
fn no_job_runs_on_a_draft_pull_request() {
    for (name, doc) in pr_workflows() {
        if triggers(&doc)["pull_request"].is_badvalue() {
            continue;
        }
        let jobs = doc["jobs"].as_hash().unwrap_or_else(|| panic!("{name}: no jobs"));
        for (id, job) in jobs {
            let cond = job["if"].as_str().unwrap_or("");
            assert!(
                cond.replace(' ', "").contains("!github.event.pull_request.draft"),
                "{name}: job {:?} must be skipped on a draft pull request (`if: ${{{{ !github.event.pull_request.draft }}}}`), found {cond:?}",
                id.as_str().unwrap_or("?")
            );
        }
    }
}

#[test]
fn a_hosted_ci_run_can_be_started_by_hand() {
    let (_, ci) = workflows().into_iter().find(|(n, _)| n == "ci.yml").expect("ci.yml");
    assert!(!triggers(&ci)["workflow_dispatch"].is_badvalue(), "ci.yml must allow `workflow_dispatch`, for a deliberate hosted run on a draft");
}

// ---- event transitions ------------------------------------------------------
//
// The policy is about what happens on real pull-request events, so it is
// checked by evaluating each workflow's own `if:` and `concurrency.group`
// against the event GitHub would send, with a small evaluator for the
// expression forms these workflows use: `!`, `&&`, `||`, `==`, `!=`,
// parentheses, literals, `github.*` property paths and `format()`.

use serde_json::{json, Value};

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

struct Expr<'a> {
    s: &'a [u8],
    i: usize,
    ctx: &'a Value,
}

impl Expr<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(t.as_bytes()) {
            self.i += t.len();
            true
        } else {
            false
        }
    }
    fn or(&mut self) -> Value {
        let mut v = self.and();
        while self.eat("||") {
            let r = self.and();
            if !truthy(&v) {
                v = r;
            }
        }
        v
    }
    fn and(&mut self) -> Value {
        let mut v = self.unary();
        while self.eat("&&") {
            let r = self.unary();
            if truthy(&v) {
                v = r;
            }
        }
        v
    }
    fn unary(&mut self) -> Value {
        self.ws();
        if self.s.get(self.i) == Some(&b'!') && self.s.get(self.i + 1) != Some(&b'=') {
            self.i += 1;
            return Value::Bool(!truthy(&self.unary()));
        }
        let l = self.primary();
        if self.eat("==") {
            let r = self.primary();
            return Value::Bool(text(&l).eq_ignore_ascii_case(&text(&r)) && l.is_null() == r.is_null());
        }
        if self.eat("!=") {
            let r = self.primary();
            return Value::Bool(!(text(&l).eq_ignore_ascii_case(&text(&r)) && l.is_null() == r.is_null()));
        }
        l
    }
    fn primary(&mut self) -> Value {
        self.ws();
        if self.eat("(") {
            let v = self.or();
            assert!(self.eat(")"), "unbalanced parenthesis");
            return v;
        }
        if self.s[self.i] == b'\'' {
            let start = self.i + 1;
            let end = start + self.s[start..].iter().position(|&c| c == b'\'').expect("unterminated string");
            self.i = end + 1;
            return Value::String(String::from_utf8_lossy(&self.s[start..end]).into_owned());
        }
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || b"._-".contains(&self.s[self.i])) {
            self.i += 1;
        }
        let word = std::str::from_utf8(&self.s[start..self.i]).unwrap();
        match word {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "null" => Value::Null,
            "format" => {
                assert!(self.eat("("));
                let mut args = vec![self.or()];
                while self.eat(",") {
                    args.push(self.or());
                }
                assert!(self.eat(")"));
                let mut out = text(&args[0]);
                for (n, a) in args.iter().enumerate().skip(1) {
                    out = out.replace(&format!("{{{}}}", n - 1), &text(a));
                }
                Value::String(out)
            }
            _ if word.chars().all(|c| c.is_ascii_digit()) => json!(word.parse::<u64>().unwrap()),
            path => path.split('.').fold(self.ctx.clone(), |v, k| v.get(k).cloned().unwrap_or(Value::Null)),
        }
    }
}

fn eval(expr: &str, ctx: &Value) -> Value {
    let e = expr.trim().strip_prefix("${{").and_then(|e| e.strip_suffix("}}")).unwrap_or(expr);
    let mut p = Expr { s: e.as_bytes(), i: 0, ctx };
    let v = p.or();
    p.ws();
    assert_eq!(p.i, p.s.len(), "unparsed tail in {expr:?}");
    v
}

/// A string with `${{ ... }}` parts, as `concurrency.group` is.
fn interpolate(s: &str, ctx: &Value) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find("${{") {
        out.push_str(&rest[..a]);
        let b = rest[a..].find("}}").expect("unterminated ${{") + a;
        out.push_str(&text(&eval(&rest[a + 3..b], ctx)));
        rest = &rest[b + 2..];
    }
    out.push_str(rest);
    out
}

/// What GitHub would send, as the `github` context: `pull_request` events
/// on PR 42 into `base`, with `action`, the draft flag and, for an edit,
/// what changed.
fn pr_event(action: &str, base: &str, draft: bool, changes: Value) -> Value {
    json!({ "github": {
        "event_name": "pull_request",
        "ref": "refs/pull/42/merge",
        "run_id": 1000 + action.len() as u64 + changes.to_string().len() as u64,
        "event": { "action": action, "pull_request": { "draft": draft, "base": { "ref": base } }, "changes": changes }
    }})
}

/// Whether `wf` starts at all for this event (its trigger's `types` and
/// `branches`), and if so which of its jobs run and in what concurrency
/// group.
fn outcome(wf: &Yaml, ctx: &Value) -> Option<(Vec<String>, String)> {
    let g = &ctx["github"];
    let name = g["event_name"].as_str().unwrap();
    let trig = &triggers(wf)[name];
    if trig.is_badvalue() {
        return None;
    }
    if name == "pull_request" {
        let action = g["event"]["action"].as_str().unwrap();
        let base = g["event"]["pull_request"]["base"]["ref"].as_str().unwrap();
        if !strings(&trig["types"]).iter().any(|t| t == action) || !strings(&trig["branches"]).iter().any(|b| b == base) {
            return None;
        }
    }
    let jobs = wf["jobs"].as_hash().unwrap();
    let run: Vec<String> = jobs.iter().filter(|(_, j)| j["if"].as_str().is_none_or(|c| truthy(&eval(c, ctx)))).map(|(k, _)| k.as_str().unwrap().to_string()).collect();
    let group = interpolate(wf["concurrency"]["group"].as_str().unwrap_or(""), ctx);
    Some((run, group))
}

fn pr_workflows() -> Vec<(String, Yaml)> {
    workflows().into_iter().filter(|(name, d)| name != "pr-gate.yml" && !triggers(d)["pull_request"].is_badvalue()).collect()
}

/// A ready pull request stacked on another branch runs nothing; retargeting
/// it to `main` -- an `edited` event whose `changes` name the base, with no
/// push -- is when it gets its hosted run (#309 review).
#[test]
fn a_ready_stacked_pull_request_runs_when_it_is_retargeted_to_main() {
    for (name, wf) in pr_workflows() {
        let stacked = pr_event("synchronize", "feature/parent", false, Value::Null);
        assert!(outcome(&wf, &stacked).is_none(), "{name}: a push to a stacked PR must not start it");
        let retarget = pr_event("edited", "main", false, json!({ "base": { "ref": { "from": "feature/parent" } } }));
        let (run, _) = outcome(&wf, &retarget).unwrap_or_else(|| panic!("{name}: retargeting a ready PR to main must start it"));
        assert!(!run.is_empty(), "{name}: retargeted to main, but every job was skipped");
        assert_eq!(run.len(), wf["jobs"].as_hash().unwrap().len(), "{name}: every job runs on the retarget: {run:?}");
    }
}

/// Editing a ready pull request's title or description is also `edited`.
/// It must run nothing, and must not cancel a real run in progress: a run
/// in the same concurrency group with `cancel-in-progress` would stop it and
/// then skip its own jobs, leaving the head unvalidated.
#[test]
fn a_title_or_description_edit_runs_nothing_and_cancels_nothing() {
    for (name, wf) in pr_workflows() {
        let edit = pr_event("edited", "main", false, json!({ "body": { "from": "old text" } }));
        let (run, group) = outcome(&wf, &edit).unwrap_or_else(|| panic!("{name}: `edited` must be a trigger (for the retarget)"));
        assert!(run.is_empty(), "{name}: a description edit ran {run:?}");
        let (_, real) = outcome(&wf, &pr_event("synchronize", "main", false, Value::Null)).expect("a push to a ready PR into main starts it");
        assert_ne!(group, real, "{name}: a description edit shares the concurrency group of a real run and would cancel it");
    }
}

#[test]
fn drafts_run_nothing_and_ready_pull_requests_and_main_run_everything() {
    for (name, wf) in pr_workflows() {
        let all = wf["jobs"].as_hash().unwrap().len();
        for action in ["opened", "synchronize", "reopened"] {
            let (run, _) = outcome(&wf, &pr_event(action, "main", true, Value::Null)).unwrap();
            assert!(run.is_empty(), "{name}: a draft `{action}` ran {run:?}");
            let (run, _) = outcome(&wf, &pr_event(action, "main", false, Value::Null)).unwrap();
            assert_eq!(run.len(), all, "{name}: a ready `{action}` must run every job: {run:?}");
        }
        let (run, _) = outcome(&wf, &pr_event("ready_for_review", "main", false, Value::Null)).unwrap();
        assert_eq!(run.len(), all, "{name}: marking ready must run every job");
        let push = json!({ "github": { "event_name": "push", "ref": "refs/heads/main", "run_id": 7, "event": {} } });
        if let Some((run, _)) = outcome(&wf, &push) {
            assert_eq!(run.len(), all, "{name}: a push to main must run every job");
        }
    }
}

#[test]
fn required_gate_is_triggered_for_all_pr_paths_and_bases() {
    let (_, gate) = workflows().into_iter().find(|(n, _)| n == "pr-gate.yml").unwrap();
    let pr = &triggers(&gate)["pull_request"];
    for filter in ["paths", "paths-ignore", "branches", "branches-ignore"] {
        assert!(pr[filter].is_badvalue(), "required PR workflow must be unfiltered: {filter}");
    }
    for event in ["opened", "synchronize", "reopened", "edited"] {
        assert!(strings(&pr["types"]).iter().any(|s| s == event));
    }
    let job = &gate["jobs"]["gate"];
    assert_eq!(job["name"].as_str(), Some("Butterfly PR check"));
    assert_eq!(job["if"].as_str(), Some("always()"));
    assert_eq!(strings(&job["needs"]), vec!["scope", "suite"]);
}
