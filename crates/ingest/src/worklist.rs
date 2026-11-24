// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! What to test next, ranked by what one test would close (#44).
//!
//! butterfly is very good at saying what is wrong and says nothing about
//! what to do first. A reader facing dozens of open items has no order to
//! work in; this gives them one, by leverage: an uncovered *function* ranks
//! above an untaken outcome inside a covered one, because a single test that
//! reaches the function may close everything behind it at once.
//!
//! Scope is deliberate: these are **test** obligations -- things a new test
//! input closes. Unresolved gaps are *reviewer* obligations (a
//! justification, 6.4.4.3/6.4.4.2.b) and live in the Gaps tab and
//! `butterfly check`; putting them in a test worklist would tell someone to
//! write a test for something no test can close.
//!
//! Justified items drop out -- `fn:<id>`, `dead:<name>`, `cond:...`,
//! `branch:...` resolutions -- so the list can reach zero, which is the
//! difference between a worklist and a complaint.

use butterfly_model::{Evidence, Resolutions};
use serde::Serialize;
use std::collections::HashSet;

/// One entry: something a test would close, and how to close it.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// How many open obligations one test could close here, at most. The
    /// sort key, and an honest "up to": reaching a function does not promise
    /// covering all of it.
    pub closes: usize,
    /// `function`, `decision`, or `outcome`.
    pub kind: String,
    pub function_id: usize,
    pub function: String,
    pub line: u32,
    /// What is open.
    pub what: String,
    /// The concrete next step.
    pub next: String,
}

/// The source text of one condition, for saying "reach `speed_ok` with it
/// false" instead of "condition 2".
///
/// The slice runs from this condition's column to the next condition on the
/// line (or the decision's end), then sheds the operators and any unbalanced
/// bracket the slice dragged in -- the same trim the MC/DC panel does.
fn cond_text(text: &[String], d: &butterfly_model::Decision, i: usize) -> String {
    let c = &d.conditions[i];
    // A line number is 1-based, so line 0 has no text to slice. Evidence
    // that says 0 is malformed, but this is a naming helper: degrade to the
    // positional label rather than underflow on the way to it (self-review
    // of #44).
    let Some(line) = c.line.checked_sub(1).and_then(|n| text.get(n as usize)) else { return format!("condition {}", i + 1) };
    let start = (c.col as usize).saturating_sub(1).min(line.len());
    let end = match d.conditions.get(i + 1) {
        Some(n) if n.line == c.line => (n.col as usize).saturating_sub(1).min(line.len()),
        _ if c.line == d.line_end => (d.col_end as usize).saturating_sub(1).min(line.len()),
        _ => line.len(),
    };
    let mut t = line.get(start..end).unwrap_or("").trim().trim_end_matches("&&").trim_end_matches("||").trim().to_string();
    let bal = |s: &str| s.chars().fold(0i32, |n, ch| n + (ch == '(') as i32 - (ch == ')') as i32);
    while t.ends_with(')') && bal(&t) < 0 {
        t.pop();
        t = t.trim_end().to_string();
    }
    while t.starts_with('(') && bal(&t) > 0 {
        t.remove(0);
        t = t.trim_start().to_string();
    }
    if t.is_empty() {
        format!("condition {}", i + 1)
    } else {
        t
    }
}

/// The open obligations, best first.
pub fn compute(ev: &Evidence, res: &Resolutions) -> Vec<Item> {
    let done: HashSet<&str> = res.entries.iter().map(|e| e.key.as_str()).collect();
    let justified = |key: &str| done.contains(key);
    let mut out = Vec::new();

    for f in &ev.functions {
        // ---- a function nothing ever called ------------------------------
        //
        // One with no coverage record of its own has a count of 0 because
        // nothing counted it, not because nothing called it (#273). It is not
        // a test obligation this list can score; its no-coverage-record gaps
        // are reviewer work, in the Gaps tab.
        if f.unrecorded_within.is_some() {
            continue;
        }
        if f.count == 0 {
            if justified(&format!("fn:{}", f.id)) || justified(&format!("dead:{}", f.name)) {
                continue;
            }
            // Everything *open* behind the call: lines, outcomes and
            // conditions are unreachable until something reaches it, but an
            // item someone already justified is closed work and must not
            // inflate the score -- a justified obligation is off the list
            // whichever function it sits in (review of #44).
            let file = &ev.files[f.file_id];
            // Clamped to the file at both ends, and the cost of this loop is
            // therefore the file's length rather than whatever the document
            // asked for.
            //
            // `.max(1)` because lines are 1-based, so a function claiming to
            // start at line 0 would underflow the index below. `.min(len)`
            // because a function claiming to *end* at u32::MAX would
            // otherwise spin some four billion times before `summary` could
            // answer -- a denial of service written into a supplier's
            // evidence file. Neither is a range any real function has, and
            // this branch's validator does not yet refuse them, so the
            // consumer holds the line on its own rather than relying on
            // another branch landing first (review of #44).
            let last = file.lines.len().min(u32::MAX as usize) as u32;
            let lines_open = (f.line_start.max(1)..=f.line_end.min(last))
                .filter(|&ln| file.lines.get(ln as usize - 1).map(|l| l.count.is_some()).unwrap_or(false) && !justified(&format!("line:{}:{ln}", f.file_id)))
                .count();
            let outcomes_open = f.branches.iter().map(|b| b.outcome_keys().iter().filter(|o| !justified(&format!("branch:{}:{}:{o}", f.id, b.site()))).count()).sum::<usize>();
            let conds_open = f
                .decisions
                .iter()
                .map(|d| {
                    let site = d.site();
                    (0..d.conditions.len()).filter(|i| !justified(&format!("cond:{}:{site}:{i}", f.id))).count()
                })
                .sum::<usize>();
            let closes = 1 + lines_open + outcomes_open + conds_open;
            out.push(Item {
                closes,
                kind: "function".into(),
                function_id: f.id,
                function: f.name.clone(),
                line: f.line_start,
                what: format!("never executed: {lines_open} line(s), {outcomes_open} outcome(s) and {conds_open} condition(s) are unreachable until something calls it"),
                next: "add a test that calls it — the Calls tab shows who could".into(),
            });
            continue; // its inner items are subsumed by the call itself
        }

        // ---- decisions missing independence pairs -------------------------
        for d in &f.decisions {
            let site = d.site();
            let open: Vec<usize> =
                d.conditions.iter().enumerate().filter(|(i, c)| !c.independence_covered && !justified(&format!("cond:{}:{site}:{i}", f.id))).map(|(i, _)| i).collect();
            if open.is_empty() {
                continue;
            }
            // one test often closes more than one condition of a decision,
            // which is why they group rather than list one by one
            let steps: Vec<String> = open
                .iter()
                .map(|&i| {
                    let c = &d.conditions[i];
                    let name = cond_text(&ev.files[f.file_id].text, d, i);
                    if c.true_count.saturating_add(c.false_count) == 0 {
                        format!("reach `{name}` at all (every vector short-circuited before it)")
                    } else if c.false_count == 0 {
                        format!("reach `{name}` with it false")
                    } else if c.true_count == 0 {
                        format!("reach `{name}` with it true")
                    } else {
                        format!("flip `{name}` while the other conditions hold still")
                    }
                })
                .collect();
            out.push(Item {
                closes: open.len(),
                kind: "decision".into(),
                function_id: f.id,
                function: f.name.clone(),
                line: d.line,
                what: format!("{} of {} MC/DC condition(s) unproven at L{}", open.len(), d.conditions.len(), d.line),
                next: steps.join("; "),
            });
        }

        // ---- untaken outcomes outside any decision ------------------------
        // a branch that belongs to a decision is already covered by the
        // MC/DC entry above; listing it again would double-count
        let in_decision: HashSet<(u32, u32)> = f.decisions.iter().flat_map(|d| d.conditions.iter().map(|c| (c.line, c.col))).collect();
        for b in &f.branches {
            if in_decision.contains(&(b.line, b.col)) {
                continue;
            }
            // Named by what the producer could actually establish: `true`
            // and `false` where the polarity is known, and neutral wording
            // where it is not, so a gcov decision is never reported as a
            // missing *true* outcome that nothing identified as true (#45).
            let (keys, words, counts) = (b.outcome_keys(), b.outcome_words(), b.outcome_counts());
            for i in 0..2 {
                let (word, taken, other) = (words[i], counts[i], counts[1 - i]);
                if taken > 0 || justified(&format!("branch:{}:{}:{}", f.id, b.site(), keys[i])) {
                    continue;
                }
                out.push(Item {
                    closes: 1,
                    kind: "outcome".into(),
                    function_id: f.id,
                    function: f.name.clone(),
                    line: b.line,
                    what: if b.direction_known {
                        format!("the {word} outcome at L{} was never taken", b.site())
                    } else {
                        format!("{word} at L{} was never taken (gcov does not say which is true)", b.site())
                    },
                    next: if other > 0 {
                        // Name the *other* outcome the same way this one was
                        // named. Saying "the true outcome ran 1x" about a
                        // gcov decision would put back the claim the rest of
                        // this was careful not to make.
                        format!("{} ran {other}x, so this is one input away", if b.direction_known { format!("the {} outcome", words[1 - i]) } else { words[1 - i].to_string() })
                    } else {
                        "neither outcome ran; a test must reach this line first".into()
                    },
                });
            }
        }
    }

    // best leverage first; ties by function then line, so the order is stable
    out.sort_by(|a, b| b.closes.cmp(&a.closes).then(a.function.cmp(&b.function)).then(a.line.cmp(&b.line)));
    out
}

#[cfg(test)]
mod tests {
    // compute() is exercised end to end in tests/worklist.rs against the C
    // example; here lives the text extraction, whose edges a fixture cannot
    // easily force.
    use super::*;
    use butterfly_model::{Condition, Decision};

    fn dec(line: u32, cols: &[u32], line_end: u32, col_end: u32) -> Decision {
        Decision {
            line,
            col: cols[0],
            line_end,
            col_end,
            conditions: cols
                .iter()
                .enumerate()
                .map(|(i, &col)| Condition { index: i, line, col, true_count: 0, false_count: 0, independence_covered: false, pair: None })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn condition_text_is_the_operand_without_its_operators() {
        let text = vec!["    if ((gear_down && speed_ok) || wow) {".to_string()];
        let d = dec(1, &[10, 23, 36], 1, 39);
        assert_eq!(cond_text(&text, &d, 0), "gear_down");
        assert_eq!(cond_text(&text, &d, 1), "speed_ok");
        assert_eq!(cond_text(&text, &d, 2), "wow");
    }

    #[test]
    fn condition_text_survives_a_line_past_the_file() {
        let d = dec(999, &[1], 999, 2);
        assert_eq!(cond_text(&[], &d, 0), "condition 1");
    }
}
