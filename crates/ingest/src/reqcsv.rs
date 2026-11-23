// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Reading low-level requirement text out of CSV (#56).
//!
//! CSV is what a requirements tool exports when it exports anything: DOORS,
//! Polarion, Jama and a spreadsheet all write it, and it needs no new
//! dependency to read. A requirements management tool's own format (ReqIF)
//! is XML, which does, so it stays a separate decision.
//!
//! # Hand-written, and why that is not a shortcut
//!
//! The rules are RFC 4180 and there are five of them: a comma separates
//! fields, a newline separates records, a field may be wrapped in quotes,
//! `""` inside a quoted field is one quote, and a quoted field may contain
//! commas and newlines. The whole of it is the state machine in `parse`.
//! `split(',')` is the version of this that appears to work until a
//! requirement statement contains a comma -- which every requirement
//! statement eventually does.
//!
//! Two concessions to the files that actually arrive: a leading byte-order
//! mark is skipped (Excel writes one), and a record may end `\r\n` or `\n`.
//!
//! # The shape
//!
//! A header row names the columns, and the names are the **same `Keys`** the
//! YAML reader uses -- so a project that has already described its documents
//! in `butterfly.toml` does not describe them twice.
//!
//! Two of those keys have no CSV meaning, and are ignored deliberately
//! rather than by accident:
//!
//! * `keys.list` names the sequence holding the entries. In CSV the *file*
//!   is that sequence; there is nothing for the name to point at.
//! * `keys.traceability` names the nesting level `code`, `tests` and
//!   `parent` sit under. CSV is flat and has no nesting level, so those
//!   three are top-level columns here.
//!
//! A column named after either is therefore an unknown column, and warned
//! about like any other.
//!
//! # A cell holding a list
//!
//! `code` and `tests` are lists, and a cell is one string, so they need a
//! separator inside the cell: `keys.csv_list_separator`, `;` by default.
//! It cannot be a comma -- that is the field separator -- and it cannot be
//! `:`, because a claim is `path::symbol` and is split at its first `::`
//! (`butterfly_model::claimcheck`). A project that needs another says so in
//! `butterfly.toml`.
//!
//! # What is refused, and what is merely reported
//!
//! The failure this whole reader was hardened against (review of #56) is a
//! document that is not read and does not say so. So:
//!
//! * **No `id` column** refuses the file, naming the columns it does have --
//!   the flat analogue of YAML's "no `requirements` list".
//! * **A row with more cells than the header** is refused: a cell that maps
//!   to no column is data being dropped, and the usual cause is a quote that
//!   was never closed further up.
//! * **An empty `id` cell** refuses the file, naming the line.
//! * **A column nobody mapped** is dropped, as an unknown YAML key is -- but
//!   a header row is written once and read for every row, so a misspelling
//!   there loses a whole column silently. It is reported as a note.
//! * A row with *fewer* cells than the header is fine: the trailing fields
//!   are simply absent, which is a thing a document is allowed to say.

use crate::reqtext::{Keys, Sink};
use butterfly_model::limits::{self, Kind, Limits};
use butterfly_model::reqtext::{Claim, Requirement, Source};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

/// One record, and the 1-based line its first cell began on.
///
/// The line is not the record's index: a quoted cell may hold newlines, so
/// the tenth record of a file can begin on its fortieth line. Counting them
/// here is what lets an entry carry a `line` a reader can be sent to.
struct Row {
    cells: Vec<String>,
    line: u32,
}

/// Close the record being built, unless the line held nothing at all.
///
/// A blank line is nothing, not a record of one empty field. A line of bare
/// commas *is* a record of empty fields, and is refused further down for
/// having no ID -- which is right, because someone wrote it.
fn end_row(cells: &mut Vec<String>, cell: &mut String, row_line: u32, any_quoted: bool, rows: &mut Vec<Row>) {
    cells.push(std::mem::take(cell));
    if !any_quoted && cells.len() == 1 && cells[0].trim().is_empty() {
        cells.clear();
        return;
    }
    rows.push(Row { cells: std::mem::take(cells), line: row_line });
}

/// RFC 4180, as a state machine over the characters.
fn parse(text: &str, what: &str) -> Result<Vec<Row>, String> {
    let mut rows: Vec<Row> = Vec::new();
    let mut cells: Vec<String> = Vec::new();
    let mut cell = String::new();
    let (mut line, mut row_line, mut quote_line) = (1u32, 1u32, 1u32);
    // `in_record` distinguishes "between records" from "at the start of an
    // empty first cell", which is what makes a blank line skippable and a
    // line of bare commas a row of empty cells.
    let (mut in_record, mut in_quotes, mut quoted, mut any_quoted) = (false, false, false, false);

    let mut it = text.chars().peekable();
    // Excel writes a BOM. Left in place it becomes part of the first column
    // name, and the `id` column is then not found -- a refusal, but for a
    // reason nobody could see by reading the file.
    if it.peek() == Some(&'\u{feff}') {
        it.next();
    }

    while let Some(c) = it.next() {
        if !in_record {
            (in_record, row_line, any_quoted) = (true, line, false);
        }
        if in_quotes {
            match c {
                '"' => {
                    if it.peek() == Some(&'"') {
                        it.next();
                        cell.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                // A newline inside quotes belongs to the value. Normalised,
                // so a document written on Windows and one written on Unix
                // produce the same statement and the same digest comparison
                // downstream.
                '\r' => {
                    if it.peek() == Some(&'\n') {
                        it.next();
                    }
                    line += 1;
                    cell.push('\n');
                }
                '\n' => {
                    line += 1;
                    cell.push('\n');
                }
                _ => cell.push(c),
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() && !quoted => {
                (in_quotes, quoted, any_quoted, quote_line) = (true, true, true, line);
            }
            ',' => {
                cells.push(std::mem::take(&mut cell));
                quoted = false;
            }
            '\r' | '\n' => {
                if c == '\r' && it.peek() == Some(&'\n') {
                    it.next();
                }
                end_row(&mut cells, &mut cell, row_line, any_quoted, &mut rows);
                line += 1;
                (in_record, quoted) = (false, false);
            }
            // Anything after a closing quote but before the comma. Spaces
            // are tolerated; a character is not, because the usual cause is
            // an unescaped quote *inside* the value (`"he said "no""`), and
            // taking it literally would silently join two fields into one.
            _ if quoted && !c.is_whitespace() => {
                return Err(format!(
                    "{what}: line {line}: `{c}` after the closing quote of a field. A quote inside a quoted field is written twice (`\"\"`); \
                     as written, this field runs into the next one."
                ));
            }
            _ if quoted => {}
            _ => cell.push(c),
        }
    }
    if in_quotes {
        return Err(format!("{what}: the quoted field beginning on line {quote_line} is never closed. Everything after it was read as part of that one value."));
    }
    if in_record {
        end_row(&mut cells, &mut cell, row_line, any_quoted, &mut rows);
    }
    Ok(rows)
}

/// A cell holding several claims, split on the configured separator.
fn list_cell(v: Option<&str>, sep: &str) -> Vec<String> {
    let Some(v) = v else { return Vec::new() };
    // An empty separator cannot split anything, so the cell is one item
    // rather than an infinite sequence of empty ones.
    if sep.is_empty() {
        return vec![v.to_string()];
    }
    v.split(sep).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// Read one file's low-level requirements.
pub fn read_file(path: &Path, keys: &Keys, limits: &Limits, sink: &mut Sink) -> Result<Source, String> {
    let text = limits::read_bounded_string(path, Kind::Auxiliary, limits)?;
    let what = path.display().to_string();
    let sha256 = format!("{:x}", Sha256::digest(text.as_bytes()));

    let rows = parse(&text, &what)?;
    let mut rows = rows.into_iter();
    let Some(header) = rows.next() else {
        return Err(format!("{what}: no rows at all. A CSV requirements document begins with a header row naming its columns, `{}` among them.", keys.id));
    };

    // Column name to position. Two columns of one name is ambiguity a reader
    // cannot resolve, and taking either would decide it for them.
    let mut cols: BTreeMap<&str, usize> = BTreeMap::new();
    let mut names: Vec<&str> = Vec::new();
    for (i, raw) in header.cells.iter().enumerate() {
        let n = raw.trim();
        if n.is_empty() {
            continue;
        }
        names.push(n);
        if let Some(prev) = cols.insert(n, i) {
            return Err(format!("{what}: `{n}` names both column {} and column {}; which one holds the value is not a thing this reader may guess.", prev + 1, i + 1));
        }
    }
    let shown = if names.is_empty() { "none".to_string() } else { names.join(", ") };
    if !cols.contains_key(keys.id.as_str()) {
        return Err(format!("{what}: no `{}` column. The header names: {shown}", keys.id));
    }

    // Everything this reader knows how to place. A column outside it is
    // dropped -- but said out loud, because a header is written once and a
    // misspelling there costs the whole column rather than one entry.
    let mut known: Vec<&str> = vec![keys.id.as_str(), keys.text.as_str(), keys.kind.as_str(), keys.status.as_str(), keys.parent.as_str(), keys.code.as_str(), keys.tests.as_str()];
    known.extend(keys.detail.iter().map(String::as_str));
    let unknown: Vec<&str> = names.iter().copied().filter(|n| !known.contains(n)).collect();
    if !unknown.is_empty() {
        sink.notes.push(format!("{what}: {} column(s) this reader has no place for, and dropped: {}", unknown.len(), unknown.join(", ")));
    }

    let mut count = 0;
    for row in rows {
        if row.cells.len() > header.cells.len() {
            return Err(format!(
                "{what}: line {}: {} cells, but the header names {} column(s). A cell that maps to no column would be dropped; the usual cause is a quote opened and never closed above.",
                row.line,
                row.cells.len(),
                header.cells.len()
            ));
        }
        let get = |name: &str| cols.get(name).and_then(|i| row.cells.get(*i)).map(|s| s.trim()).filter(|s| !s.is_empty());

        // An entry that names a kind must name one we take; one that names
        // none is taken at its word, as in YAML. Nothing refused is dropped
        // without being counted.
        let kind = get(keys.kind.as_str()).map(String::from);
        if let Some(k) = &kind {
            if !keys.kinds.iter().any(|a| a == k) {
                *sink.skipped.entry(k.clone()).or_insert(0) += 1;
                continue;
            }
        }
        let Some(id) = get(keys.id.as_str()).map(String::from) else {
            return Err(format!(
                "{what}: line {}: the `{}` cell is empty. A requirement with no ID cannot be traced to, reported on, or matched against a tag.",
                row.line, keys.id
            ));
        };
        let text = get(keys.text.as_str()).unwrap_or_default().to_string();
        if text.is_empty() {
            sink.without_text.push(id.clone());
        }
        let mut detail = BTreeMap::new();
        for k in &keys.detail {
            if let Some(v) = get(k.as_str()) {
                detail.insert(k.clone(), v.to_string());
            }
        }
        sink.out.push(Requirement {
            text,
            status: get(keys.status.as_str()).map(String::from),
            parent: get(keys.parent.as_str()).map(String::from),
            claim: Claim { code: list_cell(get(keys.code.as_str()), &keys.csv_list_separator), tests: list_cell(get(keys.tests.as_str()), &keys.csv_list_separator) },
            detail,
            source: sink.source,
            line: Some(row.line),
            kind,
            id,
        });
        count += 1;
    }
    Ok(Source { path: what, sha256, count })
}
