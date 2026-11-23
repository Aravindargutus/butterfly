// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Project configuration (`butterfly.toml`): how requirement tags are found.
//!
//! ```toml
//! [requirements]
//! patterns = ['@satisfies\s+([A-Za-z0-9_.:-]+)', 'REQ-[0-9]+']
//! scope = "header"      # header | body | both
//! lookback = 30          # comment lines above the function to scan
//! ```
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub requirements: Requirements,
    #[serde(default)]
    pub change: Change,
}

/// `[change]`: how tickets are recognised in commit messages, and which
/// files are not walked.
///
/// ```toml
/// [change]
/// patterns = ['\b([A-Z]{2,10}-\d+)\b', 'refs #(\d+)']
/// exclude  = ["vendor/**", "generated/**"]
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct Change {
    #[serde(default = "default_ticket_patterns")]
    pub patterns: Vec<String>,
    /// Glob patterns (`*`, `**`, `?`) on repository-relative paths.
    #[serde(default)]
    pub exclude: Vec<String>,
}
impl Default for Change {
    fn default() -> Self {
        Self { patterns: default_ticket_patterns(), exclude: Vec::new() }
    }
}
/// Jira-style `ABC-123`. Deliberately narrow: it still matches things like
/// `DO-178` in a message, which is why the tickets found are listed in the
/// evidence for a human to sanity-check.
fn default_ticket_patterns() -> Vec<String> {
    vec![r"\b([A-Z][A-Z0-9]{1,9}-[0-9]{1,6})\b".into()]
}

#[derive(Debug, Clone, Deserialize)]
pub struct Requirements {
    #[serde(default = "default_patterns")]
    pub patterns: Vec<String>,
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default = "default_lookback")]
    pub lookback: usize,
    /// Which keys hold what in the requirement documents `butterfly
    /// requirements` reads.
    ///
    /// Every project shapes its requirements file differently, so the reader
    /// must not assume one. The defaults are the shape this was measured
    /// against; a project with other names says so here and needs no code
    /// change -- the same principle the tag `patterns` above already follow.
    #[serde(default)]
    pub keys: Keys,
}
impl Default for Requirements {
    fn default() -> Self {
        Self { patterns: default_patterns(), scope: default_scope(), lookback: default_lookback(), keys: Keys::default() }
    }
}

fn k_list() -> String {
    "requirements".into()
}
fn k_id() -> String {
    "id".into()
}
fn k_text() -> String {
    "text".into()
}
fn k_kind() -> String {
    "type".into()
}
fn k_kinds() -> Vec<String> {
    // `derived` counts: it is a low-level requirement with no parent, and
    // DO-178C 5.1.2.h routes it to the safety assessment for that reason.
    vec!["low-level".into(), "derived".into()]
}
fn k_status() -> String {
    "status".into()
}
fn k_parent() -> String {
    "parent".into()
}
fn k_trace() -> String {
    "traceability".into()
}
fn k_code() -> String {
    "code".into()
}
fn k_tests() -> String {
    "tests".into()
}
fn k_detail() -> Vec<String> {
    vec!["rationale".into(), "safety".into(), "verification_criteria".into(), "source".into()]
}
/// What separates several claims inside one CSV cell.
///
/// A comma is out: it separates the cells themselves. A colon is out too:
/// a claim is `path::symbol`, split at its first `::`
/// (`butterfly_model::claimcheck`), so a colon separator could not be told
/// from the claim's own. A semicolon is what a spreadsheet writes when it is
/// asked for a list in one cell, and appears in no path or symbol.
fn k_csv_list_separator() -> String {
    ";".into()
}

/// Where each field lives in a requirement document.
#[derive(Debug, Clone, Deserialize)]
pub struct Keys {
    #[serde(default = "k_list")]
    pub list: String,
    #[serde(default = "k_id")]
    pub id: String,
    #[serde(default = "k_text")]
    pub text: String,
    #[serde(default = "k_kind")]
    pub kind: String,
    #[serde(default = "k_kinds")]
    pub kinds: Vec<String>,
    #[serde(default = "k_status")]
    pub status: String,
    #[serde(default = "k_parent")]
    pub parent: String,
    #[serde(default = "k_trace")]
    pub traceability: String,
    #[serde(default = "k_code")]
    pub code: String,
    #[serde(default = "k_tests")]
    pub tests: String,
    /// Kept whole, out of the inline path.
    #[serde(default = "k_detail")]
    pub detail: Vec<String>,
    /// CSV only: what separates several claims inside one `code` or `tests`
    /// cell. CSV has no lists, so the cell needs a separator of its own.
    #[serde(default = "k_csv_list_separator")]
    pub csv_list_separator: String,
}
impl Default for Keys {
    fn default() -> Self {
        Self {
            list: k_list(),
            id: k_id(),
            text: k_text(),
            kind: k_kind(),
            kinds: k_kinds(),
            status: k_status(),
            parent: k_parent(),
            traceability: k_trace(),
            code: k_code(),
            tests: k_tests(),
            detail: k_detail(),
            csv_list_separator: k_csv_list_separator(),
        }
    }
}
fn default_patterns() -> Vec<String> {
    vec![r"@satisfies\s+([A-Za-z0-9_.:/-]+)".into(), r"@verifies\s+([A-Za-z0-9_.:/-]+)".into(), r"@req(?:uirement)?\s+([A-Za-z0-9_.:/-]+)".into()]
}
fn default_scope() -> String {
    "header".into()
}
fn default_lookback() -> usize {
    30
}

/// A TOML error as its message and position, without toml's source excerpt
/// (#342).
///
/// `toml::de::Error`'s `Display` quotes the offending line of the input with a
/// caret under it; this leaves that out. The message itself is not
/// content-free, though: a type mismatch reads `invalid type: string "…",
/// expected …`, quoting the value from the file (#346 review). So this is for
/// a file the operator chose; a config named by an evidence file or found
/// beside one is reported through [`toml_error_untrusted`].
pub fn toml_error(text: &str, e: &toml::de::Error) -> String {
    let msg = e.message().trim();
    match toml_position(text, e) {
        Some(at) => format!("{msg} ({at})"),
        None => msg.to_string(),
    }
}

/// A TOML error as a fixed sentence and its position: nothing of the file.
///
/// For a config the operator did not name. Its contents may be any file they
/// can read, and both toml's excerpt and serde's message can carry them, so
/// the error says only that the file is not a valid config and where.
pub fn toml_error_untrusted(text: &str, e: &toml::de::Error) -> String {
    match toml_position(text, e) {
        Some(at) => format!("not a valid butterfly config ({at})"),
        None => "not a valid butterfly config".to_string(),
    }
}

/// `line L, column C` of the error, counted in the input, if toml gave a span.
fn toml_position(text: &str, e: &toml::de::Error) -> Option<String> {
    let r = e.span()?;
    let before = &text.as_bytes()[..r.start.min(text.len())];
    let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
    let start = before.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let column = String::from_utf8_lossy(&before[start..]).chars().count() + 1;
    Some(format!("line {line}, column {column}"))
}

pub struct TagFinder {
    pub res: Vec<Regex>,
    pub scope: String,
    pub lookback: usize,
    pub patterns: Vec<String>,
}

impl Config {
    /// Parse a config that has already been read.
    ///
    /// The caller does the reading so it happens once, under the run's
    /// budget, and the same bytes serve the parse and the provenance hash.
    /// `load` read the file a second time for provenance, and neither read
    /// was bounded (review of #5).
    ///
    /// A parse error names the file and the position, never the line: toml
    /// prints the offending source line under its message, and a config path
    /// can come from an evidence file, so the "config" may be any file the
    /// operator can read (#342).
    pub fn parse(text: &str, path: &Path) -> Result<Self> {
        toml::from_str(text).map_err(|e| anyhow::anyhow!("parsing {}: {}", path.display(), toml_error(text, &e)))
    }

    /// [`parse`](Self::parse) for a config the operator did not name -- one an
    /// evidence file names, or one found beside it. Its error carries the file
    /// and the position but nothing the file says (#346 review).
    pub fn parse_untrusted(text: &str, path: &Path) -> Result<Self> {
        toml::from_str(text).map_err(|e| anyhow::anyhow!("parsing {}: {}", path.display(), toml_error_untrusted(text, &e)))
    }

    /// Read and parse, under the run's budget, for callers that do not also
    /// need the bytes.
    pub fn load(path: &Path) -> Result<Self> {
        let lim = butterfly_model::limits::Limits::from_env();
        let t = butterfly_model::limits::read_bounded_string(path, butterfly_model::limits::Kind::Auxiliary, &lim).map_err(|e| anyhow::anyhow!("{e}"))?;
        Self::parse(&t, path)
    }
    pub fn finder(&self) -> Result<TagFinder> {
        let mut res = Vec::new();
        for p in &self.requirements.patterns {
            res.push(Regex::new(p).with_context(|| format!("requirement pattern {p:?}"))?);
        }
        Ok(TagFinder { res, scope: self.requirements.scope.clone(), lookback: self.requirements.lookback, patterns: self.requirements.patterns.clone() })
    }
}

impl TagFinder {
    fn tags_in(&self, line: &str, out: &mut Vec<String>) {
        for re in &self.res {
            for c in re.captures_iter(line) {
                let tag = c.get(1).map(|m| m.as_str()).unwrap_or_else(|| c.get(0).unwrap().as_str()).to_string();
                if !tag.is_empty() && !out.contains(&tag) {
                    out.push(tag);
                }
            }
        }
    }

    /// Tags on individual lines of the body, regardless of scope: the object
    /// code of those lines implements them.
    pub fn find_lines(&self, text: &[String], line_start: u32, line_end: u32) -> std::collections::BTreeMap<u32, Vec<String>> {
        let mut m = std::collections::BTreeMap::new();
        let last = text.len().min(u32::MAX as usize) as u32;
        let Some(start) = line_start.checked_add(1) else { return m };
        let end = line_end.min(last);
        for l in start..=end {
            if let Some(s) = text.get((l - 1) as usize) {
                let mut tags = Vec::new();
                self.tags_in(s, &mut tags);
                if !tags.is_empty() {
                    m.insert(l, tags);
                }
            }
        }
        m
    }

    /// Tags for the function at `line_start..=line_end` (1-based).
    pub fn find(&self, text: &[String], line_start: u32, line_end: u32) -> Vec<String> {
        let mut tags = Vec::new();
        if self.scope == "header" || self.scope == "both" {
            let mut header = Vec::new();
            let mut l = line_start as usize;
            let mut looked = 0;
            while l > 1 && looked < self.lookback {
                l -= 1;
                looked += 1;
                // `line_start` comes from the coverage export and the text
                // from the file on disk, and the two need not agree: a stale
                // export, evidence handed over from another build, or a file
                // edited since ingest all name lines the file no longer has.
                // The two body loops in this file already ask with `get`;
                // this one indexed directly and panicked mid-ingest, before
                // `Evidence::validate` exists to have an opinion.
                let Some(line) = text.get(l - 1) else { continue };
                let s = line.trim();
                let is_comment = s.starts_with("//")
                    || s.starts_with("/*")
                    || s.starts_with('*')
                    || s.starts_with("*/")
                    || s.starts_with("#[")
                    || s.starts_with("--")
                    || s.starts_with('#')
                    || s.starts_with("///")
                    || s.starts_with("//!");
                if !is_comment && !s.is_empty() {
                    break;
                }
                self.tags_in(s, &mut header);
            }
            header.reverse();
            for t in header {
                if !tags.contains(&t) {
                    tags.push(t);
                }
            }
        }
        if self.scope == "body" || self.scope == "both" {
            let last = text.len().min(u32::MAX as usize) as u32;
            for l in line_start.max(1)..=line_end.min(last) {
                if let Some(s) = text.get((l - 1) as usize) {
                    self.tags_in(s, &mut tags);
                }
            }
        }
        tags
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A coverage export names a function starting past the end of the source
    /// file on disk -- a stale export, evidence from another build, or a file
    /// edited since ingest. The header lookback walked backwards from that
    /// line and indexed the text directly.
    #[test]
    fn a_function_starting_past_the_end_of_the_file_does_not_panic() {
        let cfg = Config::default().finder().unwrap();
        let text: Vec<String> = ["int f(void) {", "  return 1;", "}"].iter().map(|s| s.to_string()).collect();
        // the file has 3 lines; the export claims the function starts at 40
        let tags = cfg.find(&text, 40, 45);
        assert!(tags.is_empty(), "nothing to find, and nothing to panic over: {tags:?}");
        let per_line = cfg.find_lines(&text, 40, 45);
        assert!(per_line.is_empty());
    }

    /// The message and where, never the line itself (#342).
    #[test]
    fn a_parse_error_names_the_position_but_not_the_source_line() {
        let text = "[requirements]\nSECRET-342 has no equals sign\n";
        let e = format!("{:#}", Config::parse(text, Path::new("/x/butterfly.toml")).unwrap_err());
        assert!(e.contains("/x/butterfly.toml") && e.contains("line 2, column"), "{e}");
        assert!(!e.contains("SECRET-342"), "the offending line is not echoed: {e}");
        assert!(!e.contains('|'), "nor toml's source excerpt: {e}");
    }

    /// serde's message quotes a mistyped value; an untrusted config's error
    /// does not carry it (#346 review). A syntax error is still placed.
    #[test]
    fn an_untrusted_parse_error_carries_no_value_from_the_file() {
        let typed = "requirements = \"DUMMY_PRIVATE_VALUE\"\n";
        let e = format!("{:#}", Config::parse_untrusted(typed, Path::new("/x/butterfly.toml")).unwrap_err());
        assert!(e.contains("/x/butterfly.toml") && e.contains("line 1, column"), "{e}");
        assert!(!e.contains("DUMMY_PRIVATE_VALUE"), "the mistyped value is not echoed: {e}");

        let syntax = "[requirements]\nSECRET-346 has no equals sign\n";
        let e = format!("{:#}", Config::parse_untrusted(syntax, Path::new("/x/butterfly.toml")).unwrap_err());
        assert!(e.contains("line 2, column") && !e.contains("SECRET-346"), "{e}");
    }

    #[test]
    fn custom_patterns_and_scopes() {
        let cfg: Config = toml::from_str(
            r#"[requirements]
patterns = ['REQ-[0-9]+', 'Implements:\s*(\S+)']
scope = "both"
"#,
        )
        .unwrap();
        let f = cfg.finder().unwrap();
        let text: Vec<String> = ["// Implements: SRS-42", "int f(void) {", "  /* REQ-7 */", "}"].iter().map(|s| s.to_string()).collect();
        assert_eq!(f.find(&text, 2, 4), vec!["SRS-42".to_string(), "REQ-7".to_string()]);
        let d = Config::default().finder().unwrap();
        let text: Vec<String> = ["/* @satisfies LLR-1 */", "int g() {}"].iter().map(|s| s.to_string()).collect();
        assert_eq!(d.find(&text, 2, 2), vec!["LLR-1".to_string()]);
    }
}
