// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! A source definition index, for "go to definition" in the viewer.
//!
//! This is a *navigation aid*, produced by `butterfly tags` from an external
//! ctags, and it is deliberately **not** part of the evidence: the evidence is
//! the certification record, and where a macro is defined is not analysis.
//! It rides beside the evidence as `tags.json`, is entirely optional, and its
//! absence changes nothing else -- the viewer simply offers no jump links.
//!
//! What it adds over the evidence is the source the object-code model has no
//! notion of: types, macros, typedefs, struct members, and definitions that
//! live in headers or in code that was never analysed. Functions the evidence
//! *did* analyse are linked back to it (`function_id`), so the viewer can send
//! those to the control-flow view while a header-only inline function or a
//! bare type goes to its source line -- there is no CFG for a definition the
//! object model never saw, and the index says which is which.

use serde::{Deserialize, Serialize};

pub const TAGS_SCHEMA: &str = "butterfly-tags/1";

/// The definition index beside one evidence file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tags {
    pub schema: String,
    /// The tool and version that produced this, recorded so a reader knows
    /// what parsed the sources.
    pub generator: String,
    /// The evidence this was produced against, by SHA-256 of its file, so a
    /// viewer can tell a stale index from a current one.
    #[serde(default)]
    pub evidence_sha256: String,
    pub entries: Vec<Tag>,
}

/// One definition.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub name: String,
    /// `function`, `typedef`, `macro`, `struct`, `member`, `variable`,
    /// `enumerator`, ... -- ctags' own kind, passed through.
    pub kind: String,
    /// ctags' language, e.g. `C`, `C++`.
    #[serde(default)]
    pub language: String,
    /// Display path of the file the definition is in.
    pub file: String,
    /// 1-based definition line, and its end line where ctags gave one.
    pub line: u32,
    #[serde(default)]
    pub end: Option<u32>,
    /// The evidence file this is defined in, when the evidence carries it.
    /// A header the evidence never saw is `None` -- the viewer can still name
    /// the location, but it is not one of the files already on the page.
    #[serde(default)]
    pub file_id: Option<usize>,
    /// The analysed function this tag *is*, when a `function` tag matches one
    /// in the evidence. `Some` means a control-flow graph exists and the jump
    /// can go there; `None` on a function means it was inlined-only or never
    /// analysed, so the jump goes to the source line instead.
    #[serde(default)]
    pub function_id: Option<usize>,
    /// Enclosing scope ctags reported -- the struct for a member, the class
    /// for a method -- so two `count` members can be told apart.
    #[serde(default)]
    pub scope: Option<String>,
}

impl Tags {
    pub fn check_schema(&self) -> Result<(), String> {
        if self.schema == TAGS_SCHEMA {
            Ok(())
        } else {
            Err(format!("this is {:?}, but this butterfly writes {TAGS_SCHEMA}. Re-run `butterfly tags`.", self.schema))
        }
    }

    /// Every `file_id` and `function_id` must be in range for the evidence
    /// this index sits beside, or the viewer would index past its arrays.
    pub fn validate(&self, n_files: usize, n_functions: usize) -> Result<(), String> {
        for (i, t) in self.entries.iter().enumerate() {
            if let Some(f) = t.file_id {
                if f >= n_files {
                    return Err(format!("tags[{i}] `{}` fileId {f} out of range (evidence has {n_files} files)", t.name));
                }
            }
            if let Some(f) = t.function_id {
                if f >= n_functions {
                    return Err(format!("tags[{i}] `{}` functionId {f} out of range (evidence has {n_functions} functions)", t.name));
                }
            }
            if t.line == 0 {
                return Err(format!("tags[{i}] `{}` has line 0 (lines are 1-based)", t.name));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str, file_id: Option<usize>, function_id: Option<usize>) -> Tag {
        Tag { name: name.into(), kind: "function".into(), line: 1, file_id, function_id, ..Default::default() }
    }

    #[test]
    fn validate_range_checks_both_ids_and_the_line() {
        let mut t = Tags { schema: TAGS_SCHEMA.into(), entries: vec![tag("a", Some(0), Some(0))], ..Default::default() };
        assert!(t.validate(1, 1).is_ok(), "in range");

        t.entries = vec![tag("a", Some(5), None)];
        assert!(t.validate(2, 1).unwrap_err().contains("fileId 5 out of range"));

        t.entries = vec![tag("a", None, Some(9))];
        assert!(t.validate(2, 3).unwrap_err().contains("functionId 9 out of range"));

        // a header not in the evidence is a legitimate None, not an error
        t.entries = vec![tag("MACRO", None, None)];
        assert!(t.validate(2, 3).is_ok(), "a header-only tag has no file_id and that is fine");

        t.entries = vec![Tag { name: "x".into(), line: 0, ..Default::default() }];
        assert!(t.validate(1, 1).unwrap_err().contains("line 0"));
    }

    #[test]
    fn an_older_schema_is_rejected() {
        let mut t = Tags { schema: "butterfly-tags/0".into(), ..Default::default() };
        assert!(t.check_schema().unwrap_err().contains("butterfly tags"));
        t.schema = TAGS_SCHEMA.into();
        assert!(t.check_schema().is_ok());
    }
}
