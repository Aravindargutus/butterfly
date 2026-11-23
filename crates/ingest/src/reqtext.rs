// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Reading low-level requirement text: the YAML reader, and the entry point
//! that picks a reader.
//!
//! `read` is what a caller uses, and it chooses by file extension: `.yaml`
//! and `.yml` here, `.csv` in [`crate::reqcsv`], `.md` and `.markdown` in
//! [`crate::reqmd`] -- whose fenced `yaml` blocks come back through this
//! module's parser and helpers, so the two formats cannot drift. Whatever
//! the format, the
//! entries come out of one wrapper that sorts them by ID, refuses an ID
//! defined twice, and numbers each file -- so the contracts a consumer
//! depends on cannot differ between formats.
//!
//! Only LLRs are read. DO-178C traces source code to low-level requirements;
//! high-level ones trace through them, so an HLR here would be a category
//! error.
//!
//! **Derived requirements count.** `type: derived` marks a low-level
//! requirement that no HLR gave rise to; DO-178C 5.1.2.h sends those to the
//! safety assessment precisely because there is no parent to validate them
//! against. They are implemented by code and traced like any other LLR, so
//! filtering them out would drop the entries under the most scrutiny -- 59
//! of 304 in the corpus this was built against, including implemented ones.
//! Accepted kinds are configurable; whatever is not accepted is counted and
//! reported rather than dropped in silence.
//!
//! # Why a parser and not a scan
//!
//! A requirements document is prose, and prose contains colons. One
//! `rationale: >-` block in the corpus this was built against holds a line
//! beginning `code:`; a line-oriented reader takes it for structure and
//! invents a traceability claim nobody wrote. A YAML parser sees one scalar.
//!
//! # Anchors and aliases are refused
//!
//! YAML aliases expand, and expansion multiplies: 236 bytes of nested
//! aliases took 1.6 seconds to load in a measurement on this reader, and
//! each further level multiplies that by the fan-out. A byte cap cannot see
//! it -- the file is tiny; the *expansion* is not. So the document is walked
//! as a parser event stream first, where an alias is one event rather than a
//! subtree, and a document that uses them is refused before anything is
//! expanded. The same scan took 586 microseconds.
//!
//! Refusing outright rather than bounding: a requirements document is a flat
//! list of records, anchors buy it nothing, and "no anchors, no aliases" is a
//! rule a reviewer can check by eye. An anchor is refused even with no alias
//! using it: it is half of a bomb, and the README promises both (#325 A1).
//!
//! # Malformed entries do not vanish
//!
//! An entry with no ID cannot be referenced, cannot be traced, and cannot be
//! reported against -- so the document is refused, naming the file and the
//! position, rather than read as an unknown subset of itself. An entry that
//! has an ID but no statement is a different thing: it is traceable, so it
//! is kept and listed in `without_text` for the caller to raise. Neither is
//! dropped in silence (review of #56).

use butterfly_model::limits::{self, Kind, Limits};
use butterfly_model::reqtext::{Claim, Requirement, Requirements, Source, REQTEXT_SCHEMA};
use sha2::{Digest, Sha256};
use std::path::Path;
use yaml_rust2::parser::{Event, Parser};
use yaml_rust2::{Yaml, YamlLoader};

/// Where each field lives in a requirement document.
///
/// Defined in `config` so a project can say it in `butterfly.toml` rather
/// than needing a code change; re-exported here because this is where it is
/// used.
pub use crate::config::Keys;

/// Where a reader puts what it finds.
///
/// One struct rather than a row of out-parameters: the formats append to
/// exactly the same places, and sharing the shape is what keeps the
/// contracts identical between them -- the source index an entry carries,
/// the counted-not-dropped kinds, the IDs with no statement.
pub struct Sink<'a> {
    /// Every requirement read so far, across every file.
    pub out: &'a mut Vec<Requirement>,
    /// This file's index into `Requirements::sources`.
    pub source: usize,
    /// Entries read and *not* kept, by the kind that excluded them.
    pub skipped: &'a mut std::collections::BTreeMap<String, usize>,
    /// IDs carrying no statement.
    pub without_text: &'a mut Vec<String>,
    /// Advisory notes for the caller to show, never errors: the document was
    /// read. This crate prints nothing itself -- it is the qualification-path
    /// component and has no terminal -- so a reader with something to say,
    /// as the CSV one has about a column it could not place, hands it back.
    pub notes: &'a mut Vec<String>,
}

/// Ceilings on the event stream, so a document cannot cost more than it looks.
const MAX_EVENTS: usize = 2_000_000;
const MAX_DEPTH: usize = 64;

/// Walk the events one at a time, refusing what would cost before paying it.
///
/// Pulled with `next_token()` rather than handed to `load()`: a ceiling
/// checked after the whole document has been parsed is a report, not a
/// limit -- the cost is already spent by the time it fires. Stopping at the
/// event that crosses it is what makes these numbers ceilings (review of
/// #56).
///
/// Aliases are not expanded here: one is a single event rather than the
/// subtree it stands for, which is why the bomb is cheap to see and
/// expensive to load. Anchors are refused too, alias or not.
pub(crate) fn prescan(text: &str, what: &str) -> Result<(), String> {
    let mut p = Parser::new_from_str(text);
    let (mut depth, mut events) = (0usize, 0usize);
    loop {
        let (ev, _) = p.next_token().map_err(|e| format!("{what}: {e}"))?;
        events += 1;
        if events > MAX_EVENTS {
            return Err(format!("{what}: over {MAX_EVENTS} parser events; refused without reading the rest"));
        }
        match ev {
            Event::StreamEnd => return Ok(()),
            Event::Alias(_) => {
                return Err(format!(
                    "{what}: uses a YAML alias. Aliases expand, and nested ones expand multiplicatively, so the cost of a document is \
                     not visible in its size. A requirements file has no use for them -- write the entries out."
                ))
            }
            // anchor id 0 is "no anchor"; the parser numbers anchors from 1
            Event::Scalar(_, _, a, _) | Event::SequenceStart(a, _) | Event::MappingStart(a, _) if a != 0 => {
                return Err(format!(
                    "{what}: defines a YAML anchor (`&name`). Anchors exist to be expanded by aliases, which are refused because nested \
                     ones expand multiplicatively; a requirements file has no use for either -- write the entries out."
                ))
            }
            Event::SequenceStart(..) | Event::MappingStart(..) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(format!("{what}: nested deeper than {MAX_DEPTH}; refused without reading the rest"));
                }
            }
            Event::SequenceEnd | Event::MappingEnd => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
}

/// What a node is, for an error a reader can act on.
pub(crate) fn kind_of(y: &Yaml) -> &'static str {
    match y {
        Yaml::Hash(_) => "a mapping",
        Yaml::String(_) => "a string",
        Yaml::Integer(_) | Yaml::Real(_) => "a number",
        Yaml::Boolean(_) => "a boolean",
        Yaml::Null => "empty",
        _ => "not a list",
    }
}

pub(crate) fn scalar(y: &Yaml) -> Option<String> {
    match y {
        Yaml::String(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Boolean(b) => Some(b.to_string()),
        Yaml::Real(r) => Some(r.clone()),
        _ => None,
    }
}

/// Render a nested block (`safety:` is a map) as text, for the detail pane.
pub(crate) fn flatten(y: &Yaml) -> Option<String> {
    match y {
        Yaml::Hash(h) => {
            let mut parts = Vec::new();
            for (k, v) in h {
                let (Some(k), Some(v)) = (scalar(k), flatten(v)) else { continue };
                parts.push(format!("{k}: {v}"));
            }
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        Yaml::Array(a) => {
            let parts: Vec<String> = a.iter().filter_map(flatten).collect();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        _ => scalar(y),
    }
}

pub(crate) fn list_of(y: &Yaml) -> Vec<String> {
    y.as_vec().map(|v| v.iter().filter_map(scalar).collect()).unwrap_or_default()
}

/// The line of each entry's `id` key: one list per document, one slot per
/// item of that document's `list`, in the order the loader gives them.
///
/// The loader returns the document's structure and forgets where each node
/// sat, so the positions come from a second walk of the event stream, which
/// does keep them. Only after `prescan` has passed: there are no aliases to
/// follow and the depth is bounded, so the walk costs what the file looks
/// like. An item that is not a mapping, or has no `id`, gets `None` -- the
/// loader's own reading decides what happens to it.
fn id_lines(text: &str, keys: &Keys) -> Vec<Vec<Option<u32>>> {
    type P<'a> = Parser<std::str::Chars<'a>>;
    fn next(p: &mut P) -> Option<(Event, u32)> {
        p.next_token().ok().map(|(e, m)| (e, u32::try_from(m.line()).unwrap_or(u32::MAX)))
    }
    // Consume the rest of a node whose first event has been read.
    fn skip(p: &mut P, first: &Event) -> Option<()> {
        if !matches!(first, Event::SequenceStart(..) | Event::MappingStart(..)) {
            return Some(());
        }
        let mut depth = 1usize;
        while depth > 0 {
            match next(p)?.0 {
                Event::SequenceStart(..) | Event::MappingStart(..) => depth += 1,
                Event::SequenceEnd | Event::MappingEnd => depth -= 1,
                _ => {}
            }
        }
        Some(())
    }
    // Inside a mapping: the line of the key named `want`, the last one if it
    // is written twice, as the loader keeps the last value.
    fn key_line(p: &mut P, want: &str) -> Option<Option<u32>> {
        let mut found = None;
        loop {
            let (k, line) = next(p)?;
            if matches!(k, Event::MappingEnd) {
                return Some(found);
            }
            if matches!(&k, Event::Scalar(s, ..) if s == want) {
                found = Some(line);
            }
            skip(p, &k)?;
            let (v, _) = next(p)?;
            skip(p, &v)?;
        }
    }
    fn document(p: &mut P, keys: &Keys) -> Option<Vec<Option<u32>>> {
        let (root, _) = next(p)?;
        let mut items = Vec::new();
        if !matches!(root, Event::MappingStart(..)) {
            skip(p, &root)?;
            return Some(items);
        }
        loop {
            let (k, _) = next(p)?;
            if matches!(k, Event::MappingEnd) {
                return Some(items);
            }
            let is_list = matches!(&k, Event::Scalar(s, ..) if *s == keys.list);
            skip(p, &k)?;
            let (v, _) = next(p)?;
            if !(is_list && matches!(v, Event::SequenceStart(..))) {
                skip(p, &v)?;
                continue;
            }
            // as `key_line`: a list written twice is read as its last
            items.clear();
            loop {
                let (item, _) = next(p)?;
                match item {
                    Event::SequenceEnd => break,
                    Event::MappingStart(..) => items.push(key_line(p, &keys.id)?),
                    other => {
                        skip(p, &other)?;
                        items.push(None);
                    }
                }
            }
        }
    }
    let mut p = Parser::new_from_str(text);
    let mut docs = Vec::new();
    while let Some((ev, _)) = next(&mut p) {
        match ev {
            Event::DocumentStart => match document(&mut p, keys) {
                Some(d) => docs.push(d),
                None => break,
            },
            Event::StreamEnd => break,
            _ => {}
        }
    }
    docs
}

/// Read one YAML file's low-level requirements.
pub fn read_file(path: &Path, keys: &Keys, limits: &Limits, sink: &mut Sink) -> Result<Source, String> {
    let text = limits::read_bounded_string(path, Kind::Auxiliary, limits)?;
    let what = path.display().to_string();
    prescan(&text, &what)?;
    let docs = YamlLoader::load_from_str(&text).map_err(|e| format!("{what}: {e}"))?;

    let lines = id_lines(&text, keys);

    let sha256 = format!("{:x}", Sha256::digest(text.as_bytes()));
    let mut count = 0;
    for (d, doc) in docs.iter().enumerate() {
        // A document that says nothing is fine. One that says something
        // other than the expected list is not: `requirement:` for
        // `requirements:`, or a mapping where a sequence belongs, would
        // otherwise write a zero-entry sidecar and exit 0 -- the whole file
        // silently not read, which is the failure this reader is meant to
        // make impossible (review of #56).
        if matches!(doc, Yaml::Null | Yaml::BadValue) {
            continue;
        }
        let items = match &doc[keys.list.as_str()] {
            Yaml::Array(v) => v,
            Yaml::BadValue => {
                let near: Vec<&str> = doc.as_hash().map(|h| h.keys().filter_map(|k| k.as_str()).collect()).unwrap_or_default();
                return Err(format!("{what}: no `{}` list. The document's top-level key(s): {}", keys.list, if near.is_empty() { "none".to_string() } else { near.join(", ") }));
            }
            other => {
                return Err(format!("{what}: `{}` is {}, not a list of requirements", keys.list, kind_of(other)));
            }
        };
        for (n, item) in items.iter().enumerate() {
            // An entry that names a kind must name one we take. An entry
            // with no kind is taken at its word: the file it sits in already
            // said what it holds. Anything refused is counted by kind, never
            // simply dropped.
            let kind = scalar(&item[keys.kind.as_str()]);
            if let Some(k) = &kind {
                if !keys.kinds.iter().any(|a| a == k) {
                    *sink.skipped.entry(k.clone()).or_insert(0) += 1;
                    continue;
                }
            }
            // No ID is not a requirement that can be dropped quietly: it
            // cannot be traced to, reported on, or matched against a tag, and
            // reading the rest would mean answering questions about a
            // document while silently ignoring part of it.
            let Some(id) = scalar(&item[keys.id.as_str()]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
                return Err(format!("{what}: entry {} of `{}` has no `{}`", n + 1, keys.list, keys.id));
            };
            let text = scalar(&item[keys.text.as_str()]).unwrap_or_default().trim().to_string();
            if text.is_empty() {
                sink.without_text.push(id.clone());
            }
            let tr = &item[keys.traceability.as_str()];
            let mut detail = std::collections::BTreeMap::new();
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
                // The line of its `id`, as the CSV and Markdown readers
                // record theirs; the loader forgets it, so it comes from the
                // event walk above (#325).
                line: lines.get(d).and_then(|l| l.get(n)).copied().flatten(),
                kind,
                id,
            });
            count += 1;
        }
    }
    Ok(Source { path: what, sha256, count })
}

/// Pick the reader for one document by its extension.
///
/// By extension and nothing else. Sniffing the contents would mean a `.csv`
/// whose first line happens to parse as YAML is read as YAML -- a document
/// whose format depends on what it says, which is not a thing a reviewer can
/// check. An extension this does not know is refused for the same reason the
/// YAML reader refuses a document with no `requirements` list: reading it as
/// something it is not, and writing a sidecar either way, is the silent
/// failure this feature is built to prevent (#56).
fn read_one(path: &Path, keys: &Keys, limits: &Limits, sink: &mut Sink) -> Result<Source, String> {
    // Compared lowercased: `LLR.CSV` off a Windows share is the same document.
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("yaml" | "yml") => read_file(path, keys, limits, sink),
        Some("csv") => crate::reqcsv::read_file(path, keys, limits, sink),
        Some("md" | "markdown") => crate::reqmd::read_file(path, keys, limits, sink),
        _ => Err(format!(
            "{}: not a requirements document this reader understands. The formats are YAML (.yaml, .yml), CSV (.csv) and Markdown (.md, .markdown), chosen by the file's extension.",
            path.display()
        )),
    }
}

/// Read every file into one sidecar, sorted by ID.
///
/// `notes` collects anything the operator should see that is not an error;
/// the caller decides how to show it (this crate prints nothing).
pub fn read(paths: &[std::path::PathBuf], keys: &Keys, limits: &Limits, notes: &mut Vec<String>) -> Result<Requirements, String> {
    let mut entries = Vec::new();
    let mut sources = Vec::new();
    let mut skipped = std::collections::BTreeMap::new();
    let mut without_text = Vec::new();
    for p in paths {
        let mut sink = Sink { out: &mut entries, source: sources.len(), skipped: &mut skipped, without_text: &mut without_text, notes: &mut *notes };
        let s = read_one(p, keys, limits, &mut sink)?;
        sources.push(s);
    }
    without_text.sort();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    // Two documents claiming one ID is ambiguity a reader cannot resolve, and
    // silently keeping one would decide it for them. One document claiming it
    // twice is the same ambiguity, which is why the line is named where a
    // reader knows it -- otherwise the answer is the same file twice over.
    if let Some(w) = entries.windows(2).find(|w| w[0].id == w[1].id) {
        let at = |r: &Requirement| match r.line {
            Some(l) => format!("{} line {l}", sources[r.source].path),
            None => sources[r.source].path.clone(),
        };
        return Err(format!("`{}` is defined twice: {} and {}", w[0].id, at(&w[0]), at(&w[1])));
    }
    Ok(Requirements { schema: REQTEXT_SCHEMA.into(), generator: format!("butterfly {}", env!("CARGO_PKG_VERSION")), sources, entries, skipped, without_text })
}
