// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Change impact, the re-verification ruleset (DO-178C 12.1.1).
//!
//! Input: evidence that already carries its change history
//! (`Evidence.changes`, from `butterfly changes`) and the resolutions sidecar.
//! Output: for every function changed since the certified baseline, the
//! verification artefacts the change invalidates and the work it requires —
//! sorted so a reviewer reads blockers first, and grouped per ticket so the
//! answer to "what does ABC-123 cost me?" is one row.
//!
//! The rules are deliberately mechanical and are listed on
//! [`butterfly_model::Obligation`]. Nothing here judges *whether* a change
//! matters; it says what the evidence can no longer vouch for.
//!
//! Coupling is reported one hop out (callers of a changed function, readers
//! of a global it writes). Transitive closure is a judgement about the
//! architecture that belongs to the reviewer, so it is not taken here.

use butterfly_model::{Evidence, Impact, Obligation, Resolutions, TicketImpact};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// SHA-256 of the resolution set an impact was derived from. Entries are taken
/// in their stored order (the viewer and CLI keep them sorted by key), and
/// every field that a resolution asserts is folded in, so any edit -- a new
/// record, a changed justification, a different author or date -- moves the
/// digest.
pub fn resolutions_digest(res: &Resolutions) -> String {
    let mut h = Sha256::new();
    h.update(res.schema.as_bytes());
    for e in &res.entries {
        for field in [&e.key, &e.category, &e.justification, &e.author, &e.date] {
            h.update((field.len() as u64).to_le_bytes());
            h.update(field.as_bytes());
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of the evidence revision an impact was derived against: the hash of
/// every input the evidence was built from (the coverage export, the objects,
/// the sidecars -- each already carries its own SHA-256 in provenance) and the
/// commit the change history was walked at. A rebuild or a re-walk moves it, so
/// an impact computed against an older revision is recognisable as stale.
pub fn evidence_digest(ev: &Evidence) -> String {
    let mut h = Sha256::new();
    let mut inputs: Vec<(&str, &str)> = ev.provenance.inputs.iter().map(|i| (i.role.as_str(), i.sha256.as_str())).collect();
    inputs.sort_unstable();
    for (role, sha) in inputs {
        h.update(role.as_bytes());
        h.update(b"=");
        h.update(sha.as_bytes());
        h.update(b"\n");
    }
    if let Some(ch) = &ev.changes {
        h.update(b"head=");
        h.update(ch.head.as_bytes());
        h.update(b"\nsince=");
        h.update(ch.since.as_deref().unwrap_or("").as_bytes());
    }
    // The build inputs and HEAD are not enough: compute() also reads the
    // derived records -- function attribution and coverage/gaps, the ticket and
    // deletion records, the dead-code candidates, the baseline and the
    // requirements universe. Two revisions from the same build and history can
    // still differ in these after an analysis or configuration change, and a
    // cached impact from one must not read as current for the other (#249). So
    // fold a canonical serialisation of each in -- the model is BTreeMap
    // throughout, so serde_json is deterministic -- never the persisted impact
    // itself, which this digest is stored beside.
    let mut fold = |bytes: Vec<u8>| {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    };
    if let Some(ch) = &ev.changes {
        fold(serde_json::to_vec(&ch.commits).unwrap_or_default());
        fold(serde_json::to_vec(&ch.tickets).unwrap_or_default());
        fold(serde_json::to_vec(&ch.unticketed).unwrap_or_default());
        fold(serde_json::to_vec(&ch.files).unwrap_or_default());
        fold(serde_json::to_vec(&ch.deleted).unwrap_or_default());
    }
    fold(serde_json::to_vec(&ev.functions).unwrap_or_default());
    // The globals carry the data-coupling relationships compute() reads
    // directly (each global's name, writers and readers) to raise
    // `data-coupling` obligations; an analysis revision that re-attributes a
    // reader or writer while leaving every field above unchanged would
    // otherwise share this digest and read as current (#249).
    fold(serde_json::to_vec(&ev.globals).unwrap_or_default());
    fold(serde_json::to_vec(&ev.image).unwrap_or_default());
    fold(serde_json::to_vec(&ev.baseline).unwrap_or_default());
    fold(serde_json::to_vec(&ev.requirements_universe).unwrap_or_default());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Why a persisted impact no longer describes the evidence and resolutions in
/// hand, or `None` when it still does. An impact whose bindings are empty was
/// written before this field existed and cannot be checked -- it is reported
/// as unverifiable rather than silently trusted.
pub fn staleness(imp: &Impact, ev: &Evidence, res: &Resolutions) -> Option<String> {
    if imp.resolutions_digest.is_empty() || imp.evidence_digest.is_empty() {
        return Some("this impact was recorded before it was bound to its inputs; recompute it with `butterfly impact` to confirm it is current".into());
    }
    if imp.evidence_digest != evidence_digest(ev) {
        return Some("the evidence has been rebuilt or its change history re-walked since this impact was recorded; recompute it with `butterfly impact`".into());
    }
    if imp.resolutions_digest != resolutions_digest(res) {
        return Some("the resolutions have changed since this impact was recorded; recompute it with `butterfly impact`".into());
    }
    None
}

/// Threshold for `--fail-on`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    None,
    Blocker,
    Major,
    Any,
}

impl Level {
    pub fn parse(s: &str) -> Result<Level, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Level::None),
            "blocker" | "blockers" => Ok(Level::Blocker),
            "major" => Ok(Level::Major),
            "any" | "advisory" | "all" => Ok(Level::Any),
            other => Err(format!("unknown level {other:?}: expected none, blocker, major or any")),
        }
    }
}

fn rank(severity: &str) -> u8 {
    match severity {
        "blocker" => 0,
        "major" => 1,
        _ => 2,
    }
}

/// Does the impact exceed the threshold?
pub fn fails(imp: &Impact, level: Level) -> bool {
    match level {
        Level::None => false,
        Level::Blocker => imp.obligations.iter().any(|o| o.severity == "blocker"),
        Level::Major => imp.obligations.iter().any(|o| rank(&o.severity) <= 1),
        Level::Any => !imp.obligations.is_empty(),
    }
}

/// Which function(s) a resolution key belongs to. Keys: a gap key
/// `<symbol>@<kind>@<addr>` (matched through the function's own gaps),
/// `s2o:<name>`, `dead:<name>`, `fn:<id>`, `cond:<id>:…`, `branch:<id>:…`,
/// `line:<file>:<line>`.
///
/// Name-based keys (`s2o:`, `dead:`) return *every* function sharing that
/// display name (e.g. same-basename static twins, #249), so a change to any
/// twin flags the resolution stale rather than silently attributing it to an
/// unchanged twin.
fn key_owners(key: &str, ev: &Evidence, gap_owner: &HashMap<&str, usize>, name_owners: &HashMap<&str, Vec<usize>>) -> Vec<usize> {
    if let Some(i) = gap_owner.get(key) {
        return vec![*i];
    }
    let Some((kind, rest)) = key.split_once(':') else { return Vec::new() };
    match kind {
        "s2o" | "dead" => name_owners.get(rest).cloned().unwrap_or_default(),
        "fn" | "cond" | "branch" => rest.split(':').next().and_then(|s| s.parse::<usize>().ok()).filter(|&i| i < ev.functions.len()).into_iter().collect(),
        "line" => {
            let mut it = rest.split(':');
            let (Some(file), Some(line)) = (it.next().and_then(|s| s.parse::<usize>().ok()), it.next().and_then(|s| s.parse::<u32>().ok())) else {
                return Vec::new();
            };
            let Some(f) = ev.files.get(file) else { return Vec::new() };
            f.function_ids.iter().copied().filter(|&i| ev.functions.get(i).map(|x| x.line_start <= line && line <= x.line_end).unwrap_or(false)).collect()
        }
        _ => Vec::new(),
    }
}

/// Compute the obligations. Errors only when the evidence carries no change
/// history; every other oddity degrades to "no obligation" rather than a
/// panic, because this runs on files handed over from elsewhere.
pub fn compute(ev: &Evidence, res: &Resolutions) -> Result<Impact, String> {
    let ch = ev.changes.as_ref().ok_or_else(|| "evidence has no change history: run `butterfly changes <evidence> --since <certified baseline>` first".to_string())?;
    let nfn = ev.functions.len();
    let nc = ch.commits.len();

    let changed: Vec<usize> = ev.functions.iter().enumerate().filter(|(_, f)| f.commits.iter().any(|&c| c < nc)).map(|(i, _)| i).collect();
    let is_changed: BTreeSet<usize> = changed.iter().copied().collect();

    let gap_owner: HashMap<&str, usize> =
        ev.functions.iter().enumerate().flat_map(|(i, f)| f.objects().flat_map(move |o| o.gaps.iter().map(move |g| (g.key.as_str(), i)))).collect();
    let mut name_owners: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, f) in ev.functions.iter().enumerate() {
        name_owners.entry(f.name.as_str()).or_default().push(i);
    }
    let dead: BTreeSet<usize> = ev.image.iter().flat_map(|im| im.dead_candidates.iter().filter_map(|d| d.function_id)).filter(|&i| i < nfn).collect();

    // (severity, function, kind, target, key, requirement, commit, file, ticket)
    // -> detail; the BTreeMap key is also the sort order within a severity
    type Key = (u8, Option<usize>, String, Option<usize>, Option<String>, Option<String>, Option<usize>, Option<String>, Option<usize>);
    let mut set: BTreeMap<Key, String> = BTreeMap::new();
    let mut add_full = |sev: &str,
                        kind: &str,
                        f: Option<usize>,
                        target: Option<usize>,
                        key: Option<String>,
                        req: Option<String>,
                        commit: Option<usize>,
                        file: Option<String>,
                        ticket: Option<usize>,
                        detail: String| {
        set.entry((rank(sev), f, kind.to_string(), target, key, req, commit, file, ticket)).or_insert(detail);
    };
    // ---- ticket-level: deletions and files nothing traces to ----------------
    for d in &ch.deleted {
        let ticket = d.commit.and_then(|ci| ch.commits.get(ci)).and_then(|c| c.tickets.first().copied());
        let by = d
            .commit
            .and_then(|ci| ch.commits.get(ci))
            .map(|c| format!(" by commit {} \"{}\"", c.short.chars().take(7).collect::<String>(), c.subject))
            .unwrap_or_else(|| " (deleting commit not located)".into());
        add_full(
            "blocker",
            "deleted",
            None,
            None,
            None,
            None,
            d.commit,
            Some(d.file.clone()),
            ticket,
            format!(
                "{} ({}:{}-{}) was certified in the baseline and no longer exists{by}: its tests are obsolete and its callers changed",
                d.name, d.file, d.line_start, d.line_end
            ),
        );
    }
    for (ti, t) in ch.tickets.iter().enumerate() {
        // a header llvm-cov lists (macro expansions map to it) is still a
        // file no function lives in: what matters is functions == 0
        for f in t.files.iter().filter(|f| f.functions == 0) {
            add_full("major", "untraced-file", None, None, None, None, None, Some(f.path.clone()), Some(ti), format!("{} {} `{}`, which holds no function in the evidence: a header, linker script or build file can change every includer's object code without touching a function", t.id, f.status, f.path));
        }
    }
    for &ci in &ch.unticketed {
        let Some(c) = ch.commits.get(ci) else { continue };
        let with_fns: std::collections::HashSet<&str> =
            ch.files.iter().filter(|f| ev.files.get(f.file_id).map(|x| !x.function_ids.is_empty()).unwrap_or(false)).map(|f| f.repo_path.as_str()).collect();
        for f in c.files.iter().filter(|f| !with_fns.contains(f.path.as_str())) {
            add_full(
                "major",
                "untraced-file",
                None,
                None,
                None,
                None,
                Some(ci),
                Some(f.path.clone()),
                None,
                format!("unticketed commit {} {} `{}`, which holds no function in the evidence", c.short.chars().take(7).collect::<String>(), f.status, f.path),
            );
        }
    }
    let mut add = |sev: &str, kind: &str, f: usize, target: Option<usize>, key: Option<String>, req: Option<String>, commit: Option<usize>, detail: String| {
        add_full(sev, kind, Some(f), target, key, req, commit, None, None, detail);
    };

    for &fi in &changed {
        let f = &ev.functions[fi];
        // ---- blockers ---------------------------------------------------
        for &ci in f.commits.iter().filter(|&&c| c < nc) {
            let cm = &ch.commits[ci];
            if cm.tickets.is_empty() {
                add(
                    "blocker",
                    "unticketed",
                    fi,
                    None,
                    None,
                    None,
                    Some(ci),
                    format!("commit {} \"{}\" carries no ticket: change outside change control", cm.short.chars().take(7).collect::<String>(), cm.subject),
                );
            }
        }
        if f.unrecorded_within.is_some() {
            // Its 0 is "not counted", not "never ran" (#273); the obligation
            // is the same -- the change has no coverage evidence -- but the
            // reason must be the true one.
            add("blocker", "not-exercised", fi, None, None, None, None, "changed, and has no coverage record of its own: whether any test executes it is not established".into());
        } else if f.count == 0 {
            add("blocker", "not-exercised", fi, None, None, None, None, "changed, and never executed in the coverage run: the change has no test evidence at all".into());
        }
        let reqs: Vec<String> = {
            let mut v: Vec<String> = f.requirements.clone();
            for rs in f.line_requirements.values() {
                for r in rs {
                    if !v.contains(r) {
                        v.push(r.clone());
                    }
                }
            }
            v
        };
        if reqs.is_empty() {
            add("blocker", "no-requirement", fi, None, None, None, None, "changed, but traces to no requirement: nothing says what tests must be re-run".into());
        }
        if dead.contains(&fi) {
            add(
                "blocker",
                "dead-changed",
                fi,
                None,
                None,
                None,
                None,
                "changed, and nothing in the image calls it: either the change is unreachable or the dead-code finding is wrong".into(),
            );
        }
        // ---- major ------------------------------------------------------
        if f.lines_total > f.lines_covered {
            let n = f.lines_total - f.lines_covered;
            add("major", "uncovered-lines", fi, None, None, None, None, format!("{n} of {} lines not covered after the change", f.lines_total));
        }
        let mcdc_missing = f.decisions.iter().filter(|d| !d.mcdc_covered).count();
        if mcdc_missing > 0 {
            add("major", "mcdc-incomplete", fi, None, None, None, None, format!("{mcdc_missing} of {} decision(s) not MC/DC covered after the change", f.decisions.len()));
        }
        // ---- advisory: what to re-run ------------------------------------
        for r in &reqs {
            add("advisory", "retest-requirement", fi, None, None, Some(r.clone()), None, format!("re-run the tests for {r}"));
        }
        let mut seen_callers = BTreeSet::new();
        for c in &f.callers {
            let Some(ci) = c.function_id else { continue };
            if ci == fi || ci >= nfn || !seen_callers.insert(ci) {
                continue;
            }
            let caller = &ev.functions[ci];
            let cr = if caller.requirements.is_empty() { "no requirement tag".to_string() } else { caller.requirements.join(", ") };
            add("advisory", "retest-caller", fi, Some(ci), None, None, None, format!("control coupling: caller {} ({cr}) must be re-tested", caller.name));
        }
    }
    // ---- advisory: data coupling, from the globals' side -----------------
    for g in &ev.globals {
        for &w in g.writers.iter().filter(|&&w| w < nfn && is_changed.contains(&w)) {
            for &r in g.readers.iter().filter(|&&r| r < nfn && r != w) {
                add(
                    "advisory",
                    "data-coupling",
                    w,
                    Some(r),
                    Some(g.name.clone()),
                    None,
                    None,
                    format!("data coupling: {} writes `{}`, which {} reads", ev.functions[w].name, g.name, ev.functions[r].name),
                );
            }
        }
    }
    // ---- blockers from the resolutions sidecar --------------------------
    for r in &res.entries {
        for owner in key_owners(&r.key, ev, &gap_owner, &name_owners) {
            if !is_changed.contains(&owner) {
                continue;
            }
            if r.key.starts_with("s2o:") {
                add(
                    "blocker",
                    "stale-review",
                    owner,
                    None,
                    Some(r.key.clone()),
                    None,
                    None,
                    format!("source-to-object review ({}, {}) predates the change and must be redone", r.category, r.author),
                );
            } else {
                add(
                    "blocker",
                    "stale-resolution",
                    owner,
                    None,
                    Some(r.key.clone()),
                    None,
                    None,
                    format!("resolution `{}` ({}: {}) predates the change and must be re-justified", r.key, r.category, r.justification.chars().take(60).collect::<String>()),
                );
            }
        }
    }

    let obligations: Vec<Obligation> = set
        .into_iter()
        .map(|((rk, f, kind, target, key, req, commit, file, ticket), detail)| Obligation {
            kind,
            severity: match rk {
                0 => "blocker",
                1 => "major",
                _ => "advisory",
            }
            .to_string(),
            function_id: f,
            target_function: target,
            key,
            requirement: req,
            commit,
            file,
            ticket,
            detail,
        })
        .collect();

    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_severity: BTreeMap<String, usize> = BTreeMap::new();
    for o in &obligations {
        *by_kind.entry(o.kind.clone()).or_default() += 1;
        *by_severity.entry(o.severity.clone()).or_default() += 1;
    }
    let by_ticket: Vec<TicketImpact> = ch
        .tickets
        .iter()
        .enumerate()
        .map(|(ti, t)| {
            let fs: BTreeSet<usize> = t.functions.iter().copied().collect();
            // a deletion is recorded once, under its commit's first ticket,
            // but the commit deleted it for every ticket it names (#325 D4)
            let deleted_here = |o: &Obligation| o.kind == "deleted" && o.commit.map(|c| t.commits.contains(&c)).unwrap_or(false);
            let obl: Vec<usize> = obligations
                .iter()
                .enumerate()
                .filter(|(_, o)| o.ticket == Some(ti) || o.function_id.map(|f| fs.contains(&f)).unwrap_or(false) || deleted_here(o))
                .map(|(i, _)| i)
                .collect();
            let blockers = obl.iter().filter(|&&i| obligations[i].severity == "blocker").count();
            TicketImpact { ticket: ti, obligations: obl, blockers }
        })
        .collect();

    let mut caveats = Vec::new();
    if ch.since.is_none() {
        caveats.push("History was read without --since, so every function with any commit counts as changed. For a re-certification answer, run `butterfly changes --since <the certified baseline>`.".to_string());
    }
    caveats.push("Coupling obligations are one hop out: callers of a changed function and readers of the globals it writes. Whether they propagate further is an architectural judgement left to the reviewer.".to_string());
    Ok(Impact {
        bounded: ch.since.is_some(),
        functions_changed: changed,
        obligations,
        by_ticket,
        by_kind,
        by_severity,
        caveats,
        // Bind the persisted impact to exactly the resolutions and evidence
        // revision it was derived from (#249).
        resolutions_digest: resolutions_digest(res),
        evidence_digest: evidence_digest(ev),
    })
}
