// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Reading low-level requirement text out of Markdown (#56).
//!
//! The format a requirement is *written* in, rather than exported to. A
//! requirements document that people edit by hand and review in a pull
//! request is Markdown, and the statement -- the part a human writes and a
//! reviewer reads -- should be prose on the page rather than a quoted scalar
//! inside a data structure.
//!
//! # The shape
//!
//! The outer fence here is four backticks so that the inner one is content
//! rather than the end of it -- which is the same rule this reader follows.
//!
//! ````markdown
//! ## LLR-LIM-003
//!
//! Every fault code the component can report shall map to a stable name,
//! including codes outside the defined set.
//!
//! ```yaml
//! type: derived
//! status: implemented
//! traceability:
//!   code: [src/limits.c::fault_name]
//!   tests: [src/tests.c::t_faults]
//! safety:
//!   related: true
//!   failure_condition: minor
//! ```
//! ````
//!
//! A level-2 heading whose text is the requirement ID starts an entry; the
//! prose beneath it is the statement; an optional fenced `yaml` block
//! carries everything else.
//!
//! **Why a YAML block and not bold labels.** The alternative -- `**Status:**
//! implemented` on its own line -- is a line-oriented reader over prose,
//! which is the exact mistake the YAML reader exists to avoid: requirement
//! prose contains colons, and one `rationale` paragraph in the corpus this
//! was built against holds a line beginning `code:`. It also flattens: the
//! `safety:` block is a mapping of mappings, and bold labels have nowhere to
//! put it. The block is parsed by `yaml-rust2` through the **same `Keys`**,
//! so `type`, `status`, `traceability.code`, every `detail` key and the
//! project's own names for them behave exactly as they do in a `.yaml` file,
//! and a project that has described its documents in `butterfly.toml` has
//! described these too. It needs no new key and no separator.
//!
//! # The rules, in full
//!
//! They are few on purpose: a reviewer has to be able to check by eye that a
//! document will be read the way it looks.
//!
//! * **A level-2 ATX heading (`##`) starts an entry, and its text is the
//!   ID.** The text must be a single word. `## LLR-1 the widget shall...`
//!   is refused rather than read as an ID of `LLR-1` with the rest thrown
//!   away, or as an ID with spaces in it that no tag could ever match.
//! * **`#` is a section heading.** It starts no entry and closes the one
//!   open, so a document can be organised by component.
//! * **`###` and deeper belong to the statement**, verbatim, because that is
//!   how a long requirement is structured.
//! * **The statement is the prose of the entry**, kept as written -- not
//!   folded onto one line. A table, a list or a snippet in a statement
//!   survives, and a paragraph that happens to begin with `#` is text.
//! * **One fenced block whose info string is `yaml` is structural**, wherever
//!   it sits in the entry. A second is refused: which one holds the status
//!   is not a thing to guess. A fenced block of anything else (` ```c `) is
//!   part of the statement.
//! * **That block holds one YAML document.** A `---` on its own line inside
//!   it starts a second, and a reader that took the first would drop the
//!   rest -- an entry's `traceability` claims among them -- while reporting
//!   the file as read. It is refused, naming the fence's line and the keys
//!   that would have gone (review of #56); the fields are not merged,
//!   because which document wins is the same guess a second block is
//!   refused for. A `---` inside a block scalar is text and starts nothing.
//! * **Content before the first entry is preamble** -- a title, an
//!   explanation of the document -- and is not read. It carries no ID, so
//!   there is nothing it could be attached to.
//! * **A document with no entries at all is refused**, naming the headings it
//!   does have. That is the flat analogue of YAML's "no `requirements`
//!   list", and it is the failure this reader was hardened against (review
//!   of #56): a file that is not read, and does not say so.
//!
//! Setext headings (`===` underlines) start nothing. They are not refused
//! either -- they are prose, like any other line.
//!
//! # The embedded blocks get the same discipline as a YAML file
//!
//! Each block is walked as a parser event stream before it is loaded, by the
//! same `prescan` the YAML reader uses, so an alias bomb inside a fenced
//! block is refused before it expands rather than after. The file as a whole
//! is read under the auxiliary byte cap, as every other document is.

use crate::reqtext::{flatten, kind_of, list_of, prescan, scalar, Keys, Sink};
use butterfly_model::limits::{self, Kind, Limits};
use butterfly_model::reqtext::{Claim, Requirement, Source};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use yaml_rust2::{Yaml, YamlLoader};

/// What one line of the document turned out to be.
enum Block {
    Heading {
        level: usize,
        text: String,
        raw: String,
        line: u32,
    },
    /// A fenced block whose info string is `yaml`: the structured half.
    Structure {
        body: String,
        line: u32,
    },
    /// Anything else, verbatim.
    Prose(String),
}

/// Up to three leading spaces are indentation, not content (CommonMark).
fn undent(line: &str) -> Option<&str> {
    let n = line.len() - line.trim_start_matches(' ').len();
    (n <= 3).then(|| &line[n..])
}

/// An ATX heading: its level and its text, with any closing `##` removed.
fn atx(line: &str) -> Option<(usize, String)> {
    let s = undent(line)?;
    let level = s.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &s[level..];
    // `#hashtag` is a word, not a heading: the hashes must be followed by a
    // space or by nothing.
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let t = rest.trim();
    let cut = t.trim_end_matches('#');
    let text = if cut.len() < t.len() && (cut.is_empty() || cut.ends_with(' ')) { cut.trim_end() } else { t };
    Some((level, text.to_string()))
}

/// A code fence: its character, its length and its info string.
fn fence(line: &str) -> Option<(char, usize, String)> {
    let s = undent(line)?;
    let ch = s.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = s.chars().take_while(|c| *c == ch).count();
    if len < 3 {
        return None;
    }
    let info = s[len..].trim().to_string();
    // A backtick fence's info string may not contain a backtick, or ``` `x` ```
    // inline would open one.
    if ch == '`' && info.contains('`') {
        return None;
    }
    Some((ch, len, info))
}

/// The fence being read, and what has been collected inside it.
struct Open {
    ch: char,
    len: usize,
    structural: bool,
    line: u32,
    body: String,
}

/// Split the document into headings, structured blocks and prose.
///
/// One pass, fence-aware, because a `##` inside a fenced block is a comment
/// in someone's example and not the start of a requirement.
fn blocks(text: &str, what: &str) -> Result<(Vec<Block>, Vec<String>), String> {
    let mut out = Vec::new();
    let mut headings = Vec::new();
    let mut open: Option<Open> = None;

    for (i, raw) in text.lines().enumerate() {
        let line = (i + 1) as u32;
        if let Some(o) = open.as_mut() {
            let closes = fence(raw).is_some_and(|(c, n, info)| c == o.ch && n >= o.len && info.is_empty());
            if !closes {
                if o.structural {
                    o.body.push_str(raw);
                    o.body.push('\n');
                } else {
                    out.push(Block::Prose(raw.to_string()));
                }
                continue;
            }
            let o = open.take().expect("just checked");
            if o.structural {
                out.push(Block::Structure { body: o.body, line: o.line });
            } else {
                out.push(Block::Prose(raw.to_string()));
            }
            continue;
        }
        if let Some((ch, len, info)) = fence(raw) {
            let structural = info.eq_ignore_ascii_case("yaml") || info.eq_ignore_ascii_case("yml");
            if !structural {
                out.push(Block::Prose(raw.to_string()));
            }
            open = Some(Open { ch, len, structural, line, body: String::new() });
            continue;
        }
        if let Some((level, t)) = atx(raw) {
            headings.push(format!("{} {t}", "#".repeat(level)));
            out.push(Block::Heading { level, text: t, raw: raw.to_string(), line });
            continue;
        }
        out.push(Block::Prose(raw.to_string()));
    }
    if let Some(o) = open {
        // Everything below the opener was read as one block, so a heading in
        // it started no entry. Saying so beats reporting the entries that
        // disappeared into it.
        return Err(format!("{what}: the fenced block opened on line {} is never closed, so everything below it was read as its contents.", o.line));
    }
    Ok((out, headings))
}

/// One entry, before its block is parsed.
struct Entry {
    id: String,
    line: u32,
    prose: Vec<String>,
    structure: Option<(String, u32)>,
}

/// Read one file's low-level requirements.
pub fn read_file(path: &Path, keys: &Keys, limits: &Limits, sink: &mut Sink) -> Result<Source, String> {
    let text = limits::read_bounded_string(path, Kind::Auxiliary, limits)?;
    let what = path.display().to_string();
    let sha256 = format!("{:x}", Sha256::digest(text.as_bytes()));

    let (blocks, headings) = blocks(&text, &what)?;
    let mut entries: Vec<Entry> = Vec::new();
    let mut cur: Option<Entry> = None;
    for b in blocks {
        match b {
            Block::Heading { level: 2, text, line, .. } => {
                if text.split_whitespace().count() != 1 {
                    return Err(format!(
                        "{what}: line {line}: `## {text}` is not a requirement ID. A level-2 heading starts an entry and its text is the ID alone; \
                         write a section heading as `# {text}`, or a heading inside an entry as `### {text}`."
                    ));
                }
                entries.extend(cur.take());
                cur = Some(Entry { id: text, line, prose: Vec::new(), structure: None });
            }
            // A section heading closes the entry it follows; what comes
            // under it is preamble for the next one.
            Block::Heading { level: 1, .. } => entries.extend(cur.take()),
            Block::Heading { raw, .. } => {
                if let Some(e) = cur.as_mut() {
                    e.prose.push(raw);
                }
            }
            Block::Structure { body, line } => {
                let Some(e) = cur.as_mut() else { continue };
                if e.structure.is_some() {
                    return Err(format!(
                        "{what}: line {line}: a second `yaml` block under `## {}`. Which one carries the entry's fields is not a thing this reader may guess.",
                        e.id
                    ));
                }
                e.structure = Some((body, line));
            }
            Block::Prose(l) => {
                if let Some(e) = cur.as_mut() {
                    e.prose.push(l);
                }
            }
        }
    }
    entries.extend(cur.take());

    // A document nobody's entries came out of is a document that was not
    // read, and writing a zero-entry sidecar over it would be the quiet
    // failure this reader exists to prevent (review of #56).
    if entries.is_empty() {
        let shown = if headings.is_empty() { "none".to_string() } else { headings.join(", ") };
        return Err(format!(
            "{what}: no requirements. An entry starts at a level-2 heading whose text is the requirement ID (`## LLR-LIM-003`). The document's heading(s): {shown}"
        ));
    }

    let mut count = 0;
    for e in entries {
        // The fields, parsed exactly as one item of a YAML document's list:
        // same helpers, same keys, same ceilings. An entry with no block is
        // an entry whose fields are absent, which indexes to `BadValue` and
        // reads as absent everywhere below.
        let item = match &e.structure {
            Some((body, line)) => {
                let at = format!("{what}: the `yaml` block at line {line}");
                prescan(body, &at)?;
                let docs = YamlLoader::load_from_str(body).map_err(|err| format!("{at}: {err}"))?;
                // A `---` inside the fence is a second YAML document, and
                // taking the first of them would drop an entry's claims --
                // exactly what `butterfly claims` compares -- while the
                // import reported success. This reader already refuses a
                // second `yaml` block under one heading; a second document
                // inside one block is the same ambiguity through a smaller
                // door, and merging them would be a policy nobody wrote
                // down (review of #56).
                if docs.len() > 1 {
                    let lost: Vec<&str> = docs[1..].iter().filter_map(|d| d.as_hash()).flat_map(|h| h.keys()).filter_map(|k| k.as_str()).collect();
                    let named = if lost.is_empty() { String::new() } else { format!(" (`{}`)", lost.join("`, `")) };
                    return Err(format!(
                        "{at}: {} YAML documents under `## {}`, and only the first would be read. A `---` on its own line starts a new document, \
                         so the fields below it{named} would be dropped without a word. An entry's fields are one document -- write them as one. \
                         A `---` inside a block scalar (`rationale: |`) is text and starts nothing.",
                        docs.len(),
                        e.id
                    ));
                }
                let doc = docs.into_iter().next().unwrap_or(Yaml::Null);
                if matches!(doc, Yaml::Null | Yaml::BadValue) {
                    Yaml::Null
                } else if doc.as_hash().is_none() {
                    return Err(format!("{at}: {}, not a mapping of fields", kind_of(&doc)));
                } else {
                    doc
                }
            }
            None => Yaml::Null,
        };

        let kind = scalar(&item[keys.kind.as_str()]);
        if let Some(k) = &kind {
            if !keys.kinds.iter().any(|a| a == k) {
                *sink.skipped.entry(k.clone()).or_insert(0) += 1;
                continue;
            }
        }
        // The heading is where the ID lives. A block that names one too is
        // not overruled quietly in either direction.
        if let Some(other) = scalar(&item[keys.id.as_str()]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && *s != e.id) {
            return Err(format!("{what}: line {}: the heading says `{}` and the block's `{}` says `{other}`. One document cannot give an entry two IDs.", e.line, e.id, keys.id));
        }

        // The prose is the statement. A block may carry `text` instead, for
        // a document exported into this shape rather than written in it.
        let mut text = e.prose.join("\n").trim().to_string();
        if text.is_empty() {
            text = scalar(&item[keys.text.as_str()]).unwrap_or_default().trim().to_string();
        }
        if text.is_empty() {
            sink.without_text.push(e.id.clone());
        }

        let tr = &item[keys.traceability.as_str()];
        let mut detail = BTreeMap::new();
        for k in &keys.detail {
            if let Some(v) = flatten(&item[k.as_str()]) {
                detail.insert(k.clone(), v);
            }
        }
        sink.out.push(Requirement {
            text,
            status: scalar(&item[keys.status.as_str()]),
            parent: scalar(&tr[keys.parent.as_str()]).or_else(|| scalar(&item[keys.parent.as_str()])),
            claim: Claim { code: list_of(&tr[keys.code.as_str()]), tests: list_of(&tr[keys.tests.as_str()]) },
            detail,
            source: sink.source,
            line: Some(e.line),
            kind,
            id: e.id,
        });
        count += 1;
    }
    Ok(Source { path: what, sha256, count })
}
