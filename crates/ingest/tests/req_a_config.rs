// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Requirements-based tests for `butterfly.toml` (area A).
//!
//! Expected values come from README.md "Project configuration
//! (`butterfly.toml`)" and "Your own document shape", and from
//! docs/butterfly.html "Configuration".

use butterfly_ingest::config::Config;
use std::path::Path;

fn lines(s: &str) -> Vec<String> {
    s.lines().map(str::to_string).collect()
}

fn cfg(toml: &str) -> Config {
    Config::parse(toml, Path::new("butterfly.toml")).expect("parses")
}

/// README.md:577-578 "`@satisfies LLR-LIM-002` in a comment gives butterfly an
/// **ID**" -- with no configuration at all.
#[test]
fn the_default_configuration_reads_a_satisfies_tag() {
    let f = Config::default().finder().expect("the default patterns compile");
    let src = lines("// @satisfies LLR-LIM-002\nint clamp(int x) {\n  return x;\n}\n");
    assert_eq!(f.find(&src, 2, 4), ["LLR-LIM-002"]);
}

/// README.md:1167-1168 "Requirement-tag patterns are regexes (capture group 1
/// = the ID)", and docs/butterfly.html:619 "Capture group 1 is the
/// identifier, or the whole match when the pattern has no group" -- the
/// README's own example mixes both kinds.
#[test]
fn group_one_is_the_id_and_a_groupless_pattern_yields_the_whole_match() {
    let c = cfg("[requirements]\npatterns = ['@satisfies\\s+([A-Za-z0-9_.:/-]+)', 'REQ-[0-9]+', 'Implements:\\s*(\\S+)']\nscope = \"header\"\nlookback = 30\n");
    let f = c.finder().expect("compiles");
    let src = lines("// @satisfies LLR-A-1\n// REQ-42 applies\n// Implements: SRS-9\nvoid f(void) {\n}\n");
    let mut got = f.find(&src, 4, 5);
    got.sort();
    assert_eq!(got, ["LLR-A-1", "REQ-42", "SRS-9"]);
}

/// README.md:1164 `scope = "header"   # header | body | both`, and
/// docs/butterfly.html:612 "lookback = 30 # comment lines above a function to
/// scan": `header` reads only the comment above, `body` only the function,
/// `both` reads both.
#[test]
fn scope_selects_header_body_or_both() {
    let src = lines("// @satisfies LLR-HDR-1\nvoid f(void) {\n  // @satisfies LLR-BODY-1\n}\n");
    let tags = |scope: &str| {
        let f = cfg(&format!("[requirements]\nscope = \"{scope}\"\n")).finder().unwrap();
        let mut t = f.find(&src, 2, 4);
        t.sort();
        t
    };
    assert_eq!(tags("header"), ["LLR-HDR-1"], "header: the comment above only");
    assert_eq!(tags("body"), ["LLR-BODY-1"], "body: the function only");
    assert_eq!(tags("both"), ["LLR-BODY-1", "LLR-HDR-1"], "both");
}

/// docs/butterfly.html:612 "lookback = 30 # comment lines above a function to
/// scan" -- a tag `lookback` lines above is read; one further up is not.
#[test]
fn lookback_bounds_how_far_above_the_function_a_tag_is_read() {
    let src = lines("// @satisfies LLR-FAR-1\n// filler\n// @satisfies LLR-NEAR-1\n// filler\nvoid f(void) {\n}\n");
    let f = cfg("[requirements]\nscope = \"header\"\nlookback = 3\n").finder().unwrap();
    assert_eq!(f.find(&src, 5, 6), ["LLR-NEAR-1"], "two lines up is in, four is out");
}

/// README.md:1167 the patterns are regexes; one that is not a regex cannot
/// find anything, so the config is refused rather than a pattern dropped.
#[test]
fn a_pattern_that_is_not_a_regex_is_refused() {
    let c = cfg("[requirements]\npatterns = ['@satisfies\\s+(']\n");
    assert!(c.finder().is_err(), "an unclosed group is not a pattern");
}

/// README.md:787-805: every key of the documented `[requirements.keys]` table
/// is accepted and lands where it says, and README.md:1173-1176 `[change]`
/// takes `patterns` and `exclude`.
#[test]
fn the_documented_keys_and_change_tables_are_read() {
    let c = cfg(r#"
[requirements.keys]
list = "llrs"
id = "ref"
text = "statement"
kind = "category"
kinds = ["llr", "derived"]
status = "state"
traceability = "links"
parent = "derives_from"
code = "impl"
tests = "verifies"
detail = ["why"]
csv_list_separator = "|"

[change]
patterns = ['\b([A-Z]{2,10}-\d+)\b', 'refs #(\d+)']
exclude  = ["vendor/**", "generated/**"]
"#);
    let k = &c.requirements.keys;
    assert_eq!((k.list.as_str(), k.id.as_str(), k.text.as_str(), k.kind.as_str(), k.status.as_str()), ("llrs", "ref", "statement", "category", "state"));
    assert_eq!((k.traceability.as_str(), k.parent.as_str(), k.code.as_str(), k.tests.as_str()), ("links", "derives_from", "impl", "verifies"));
    assert_eq!(k.kinds, ["llr", "derived"]);
    assert_eq!(k.detail, ["why"]);
    assert_eq!(k.csv_list_separator, "|");
    assert_eq!(c.change.patterns.len(), 2);
    assert_eq!(c.change.exclude, ["vendor/**", "generated/**"]);
}

/// README.md:1176-1179 "The built-in pattern is Jira-style `ABC-123`. It ...
/// still matches things like `DO-178` or `UTF-8`".
#[test]
fn the_built_in_ticket_pattern_is_jira_style() {
    let c = Config::default();
    let res: Vec<regex::Regex> = c.change.patterns.iter().map(|p| regex::Regex::new(p).unwrap()).collect();
    let hits = |s: &str| res.iter().any(|r| r.is_match(s));
    assert!(hits("fix ABC-123: clamp"), "ABC-123");
    assert!(hits("per DO-178"), "DO-178, as the README warns");
    assert!(hits("decode UTF-8"), "UTF-8, as the README warns");
    assert!(!hits("refactor the limiter"), "and nothing in plain prose");
}

/// A config that is not TOML is refused, naming the file -- the
/// "fails with a sentence naming the file" rule of README.md:1589-1590.
#[test]
fn a_config_that_is_not_toml_is_refused_naming_it() {
    let e = Config::parse("[requirements\npatterns = 3", Path::new("/proj/butterfly.toml")).expect_err("not TOML");
    assert!(format!("{e:#}").contains("butterfly.toml"), "{e:#}");
    assert!(Config::load(Path::new("/nonexistent/butterfly-req-a/butterfly.toml")).is_err(), "a missing file is an error");
}
