// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The check that keeps `support/matrix.toml` honest (#151, for #38).
//!
//! #38's rule is "no claim of support without a worked example", and a table
//! drifts the moment someone adds a feature, so the rule is enforced here
//! rather than remembered. Every finding names the row id, because a failure
//! that says "the matrix is wrong" sends the reader to a 600-line file.
//!
//! What is checked, per row:
//!
//!   1. a `supported` or `caveats` row names an example directory that exists;
//!   2. a `supported` row's example is built by CI -- by *both*
//!      `.github/workflows/ci.yml` and `scripts/ci-local.sh`, since hosted CI
//!      is not always available and the local script is then the only run.
//!      A `caveats` row may lean on an example CI does not build, but only
//!      if its caveat says so in the words "not built in CI".
//!      The two build lists must agree, with one declared exception: an
//!      example listed under `[[local_only]]` is built by `ci-local.sh`
//!      alone, with the reason written down (#164: a fixture that needs an
//!      emulator costs billed minutes on every hosted run). Such an example
//!      can back `caveats` rows only, and each must say "verified locally,
//!      not in hosted CI". An undeclared difference between the lists fails;
//!      so does a declaration hosted CI has caught up with.
//!      "Built" means the script is a *command* in a step that runs: named in
//!      a comment, echoed in a skip message, quoted inside a `grep`, or sitting
//!      in a step switched off with `if: false`, it builds nothing, and
//!      neither does a line in the body of a here-document, which is input
//!      and not a command. Where a command begins is decided after
//!      tokenizing -- quotes, escapes, `#` comments, `\`-continued lines,
//!      `<<` bodies -- so a `;` inside an argument is text, and a shape the
//!      tokenizer does not model is reported rather than guessed at (see
//!      `shell_commands`);
//!   3. `evidence` (required for a language or toolchain row, checked
//!      wherever it is given) is `path:line` inside that example, the file has that
//!      line, and `pattern` still matches within `EVIDENCE_WINDOW` lines of
//!      it. The window exists so that adding a licence header or a comment
//!      above the construct does not fail the build; deleting or renaming the
//!      construct still does. A language or toolchain row may not cite a
//!      `.md` file: a construct lives in code, and prose outlives it;
//!   4. `proof` is a test file that exists and, where `::name` is given,
//!      still contains that test. A `supported` row's proof must also read
//!      the example it cites: a model test on hand-written evidence proves
//!      the model, not that butterfly handles the example. A `caveats` row
//!      may have no proof, but only if its caveat says "no test";
//!   5. every directory under `examples/` (and each build system under
//!      `examples/build-systems/`) backs at least one row;
//!   6. a `caveats` row states its caveat, a `not-supported` row its reason,
//!      and no id appears twice.
//!
//! This is deliberately textual. It cannot tell whether a test *asserts* the
//! construct, only that the claim still points at something real; the claims
//! audit (#200) is where a person reads each proof. What it buys is that a
//! claim cannot silently outlive its evidence.
//!
//! The rules are a pure function over a `Tree`, so they are tested below
//! against small in-memory trees as well as against the repository: the six
//! rules stay proven without anyone having to break the real tree to see.

use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// How far from the stated line the pattern may be found, in either
/// direction. Small on purpose: wide enough to survive an added comment,
/// narrow enough that the row still points at the construct and not at the
/// file in general.
const EVIDENCE_WINDOW: usize = 3;

/// The words a `caveats` row must use to lean on an example CI does not
/// build. A fixed phrase so the check is mechanical and the rendered page
/// (#153, #154) can rely on it.
const NOT_IN_CI: &str = "not built in ci";

/// The words a `caveats` row must use to lean on a declared local-only
/// example: one `scripts/ci-local.sh` builds and `.github/workflows/ci.yml`
/// deliberately does not. They are #164's words. Distinct from `NOT_IN_CI`
/// because the claim is different -- this example *is* built and its proof
/// *is* run, only not where a reader of the hosted badge would assume.
const LOCAL_ONLY: &str = "verified locally, not in hosted ci";

/// The words a `caveats` row must use to stand with no proof at all. Four
/// examples are read by no test today; a row that leans on one has to put
/// that on the page rather than cite a test that proves something else.
const NO_TEST: &str = "no test";

const CI_YML: &str = ".github/workflows/ci.yml";
const CI_LOCAL: &str = "scripts/ci-local.sh";
const MATRIX: &str = "support/matrix.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Matrix {
    /// Bumped when a field changes meaning, so a generator reading an older
    /// file can refuse it rather than misread it.
    schema: u32,
    /// Examples `ci-local.sh` builds and hosted CI deliberately does not.
    /// The only licensed difference between the two build lists.
    #[serde(default)]
    local_only: Vec<LocalOnly>,
    #[serde(default)]
    row: Vec<Row>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalOnly {
    example: String,
    /// Why hosted CI does not build it. Required: an exception nobody can
    /// explain is a gap, not a policy.
    #[serde(default)]
    reason: String,
    /// Issues where the cost, or the way out of it, is tracked.
    #[serde(default)]
    tracking: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    id: String,
    area: String,
    /// What the row is grouped under when rendered: a language, "Viewer",
    /// "Object format" and so on. Free text, but required.
    #[serde(default)]
    group: String,
    name: String,
    state: String,
    #[serde(default)]
    example: String,
    #[serde(default)]
    evidence: String,
    #[serde(default)]
    pattern: String,
    #[serde(default)]
    proof: String,
    #[serde(default)]
    caveat: String,
    #[serde(default)]
    reason: String,
    /// Issue numbers where a known gap is tracked.
    #[serde(default)]
    tracking: Vec<u32>,
}

/// The part of the repository the rules look at. A trait so the rules can be
/// run against an in-memory tree.
trait Tree {
    fn read(&self, path: &str) -> Option<String>;
    fn is_dir(&self, path: &str) -> bool;
    /// Immediate subdirectories, by name.
    fn subdirs(&self, path: &str) -> Vec<String>;
}

struct Disk(PathBuf);

impl Tree for Disk {
    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(path)).ok()
    }
    fn is_dir(&self, path: &str) -> bool {
        self.0.join(path).is_dir()
    }
    fn subdirs(&self, path: &str) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(self.0.join(path)) else { return vec![] };
        let mut v: Vec<String> = rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect();
        v.sort();
        v
    }
}

/// What one CI script builds -- and every line of it the checker declined to
/// read, which is the other half of the same answer. Rule 2 is only as good
/// as this reading, and the reading can be wrong in both directions: a line
/// misread as a build invents a hosted build and rejects a valid
/// `[[local_only]]` declaration, while a build missed leaves an example
/// looking unbuilt. So a shape the tokenizer does not model is never guessed
/// at; it becomes a finding naming the line (review of #207).
#[derive(Default)]
struct Builds {
    examples: BTreeSet<String>,
    /// One per line, each already saying what about it was not understood.
    unreadable: Vec<String>,
}

/// Example names built by one CI script. Only a build that is actually *run*
/// counts, and the test of that is command position: the build script has to
/// be the command, not a word in one, and not a word inside one. Both scripts
/// name build scripts in comments, in the text of their own skip messages,
/// and -- in `ci.yml` -- inside a `grep` that filters those messages; none of
/// that builds anything, and a matrix that counted it would promote a row on
/// the strength of an `echo` (review of #207).
fn built_by(text: &str, is_yaml: bool) -> Builds {
    let mut out = Builds::default();
    if is_yaml {
        for shell in workflow_run_scripts(text) {
            // a whole `run:` block at once, so a here-document's body is in
            // the same text as the `<<` that opened it; one still pending at
            // the end of the block is a delimiter that never arrives
            for doc in builds_run_by(&shell, &mut out) {
                out.unreadable.push(doc.never_arrives());
            }
        }
    } else {
        // Line by line, because in the local script every build goes through
        // `step "label" cmd`; a `printf` that names the script is reporting a
        // skip, not running it, and the label names the script too without
        // running it. A here-document's body is therefore *not* in the text
        // `shell_commands` was handed -- it is in the lines below -- so the
        // lines it covers are stepped over here instead (review of #207).
        let lines = logical_lines(text);
        let mut i = 0;
        while i < lines.len() {
            let line = &lines[i];
            i += 1;
            let t = line.trim_start();
            if t.starts_with('#') {
                continue;
            }
            // every line is read for a `<<`, not only the `step` lines a build
            // can be in: a body line under a plain `cat <<'EOF'` could itself
            // be spelt like a `step`, and it is still body
            let docs = match t.strip_prefix("step ").and_then(step_command) {
                Some(cmd) => builds_run_by(cmd, &mut out),
                None => here_docs_opened(t, &mut out),
            };
            for doc in docs {
                let Some(n) = lines[i..].iter().position(|l| doc.ends(l)) else {
                    out.unreadable.push(doc.never_arrives());
                    i = lines.len();
                    break;
                };
                if let Err(why) = doc.body_is_readable(&lines[i..i + n].join("\n")) {
                    out.unreadable.push(format!("{why}, in the body of `<<{}`", doc.delim));
                }
                i += n + 1;
            }
        }
    }
    out
}

/// The here-documents a line opens, for a line that runs no build this reader
/// reads. Its commands do not matter; where its body ends does, because the
/// lines it covers must not be read as commands. A line the tokenizer will not
/// interpret might be opening one, so when such a line holds a `<<` at all
/// that is reported rather than passed over.
fn here_docs_opened(shell: &str, out: &mut Builds) -> Vec<HereDoc> {
    match shell_commands(shell) {
        Ok(t) => t.pending,
        Err((_, why)) => {
            if shell.contains("<<") {
                out.unreadable.push(format!("{why}, in `{}`", shell.trim()));
            }
            vec![]
        }
    }
}

/// `"label" cmd args...` -> `cmd args...`. A `step` whose label is not quoted
/// is not one of ours and runs nothing we can read.
fn step_command(rest: &str) -> Option<&str> {
    let rest = rest.trim_start();
    let q = rest.chars().next()?;
    if q != '"' && q != '\'' {
        return None;
    }
    let end = rest[1..].find(q)? + 1;
    Some(rest[end + 1..].trim_start())
}

/// Every example whose `build.sh` is *run* somewhere in a piece of shell.
///
/// The line is tokenized first, and only then cut into commands, because a
/// `;`, `|` or `&` is a boundary only when the shell would see it as one: cut
/// on the raw text instead and `echo "local only; ./examples/x/build.sh"`
/// becomes two commands, the second of which appears to build `x` (review of
/// #207). Each command's head is what counts -- leading `VAR=value`
/// assignments and `env` are stepped over, which is how the GCC example is
/// built, and what follows must be the script and nothing else. So
/// `./examples/c/build.sh`, `"./examples/c/build.sh"` and
/// `env CC=gcc ./examples/c-gcc/build.sh` count, while
/// `echo ./examples/c/build.sh`, `grep -Fv "./examples/c/build.sh"`,
/// `echo "a; ./examples/c/build.sh"` and anything after a `#` do not.
///
/// Returns the here-documents still open at the end of `shell`: their bodies
/// are in text the caller has and this did not, and until they are stepped
/// over their lines are not commands. Nothing at all is counted from a piece
/// of shell the tokenizer refuses -- the finding is the answer.
fn builds_run_by(shell: &str, out: &mut Builds) -> Vec<HereDoc> {
    let path = Regex::new(r"^\./examples/([A-Za-z0-9_./-]+)/build\.sh$").unwrap();
    let assignment = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").unwrap();
    // the whole piece of shell at once, not line by line: a line ending in
    // `\` continues into the next one, so its words belong to the command
    // that began above and are not a command of their own
    let Tokenized { commands, pending } = match shell_commands(shell) {
        Ok(t) => t,
        Err((at, why)) => {
            let line = shell.lines().nth(shell.chars().take(at).filter(|&c| c == '\n').count()).unwrap_or("").trim();
            out.unreadable.push(format!("{why}, in `{line}`"));
            return vec![];
        }
    };
    for cmd in commands {
        let head = cmd.iter().find(|t| t.as_str() != "env" && !assignment.is_match(t));
        if let Some(c) = head.and_then(|h| path.captures(h)) {
            out.examples.insert(c[1].to_string());
        }
    }
    pending
}

/// A here-document the shell has been told to read: `<<WORD`, `<<-WORD`, or
/// either with the word quoted. Its body is the command's input -- text `cat`
/// prints, a file `tee` writes -- and never a list of commands, which is the
/// whole reason the tokenizer has to recognise one (review of #207).
struct HereDoc {
    delim: String,
    /// `<<-`: leading tabs come off the body lines and off the terminator.
    strip_tabs: bool,
    /// The delimiter was unquoted, so the body is still expanded. Nothing in
    /// an expansion runs except a command substitution, and that one does.
    expanded: bool,
}

impl HereDoc {
    /// True when `line` is the terminator: the delimiter alone on its own
    /// line, at column zero -- or after tabs, for `<<-`. Exactly the shell's
    /// test, so that an indented `NOTE` is body, as it would really be.
    fn ends(&self, line: &str) -> bool {
        let l = if self.strip_tabs { line.trim_start_matches('\t') } else { line };
        l == self.delim
    }

    /// The one thing in a body that still runs: a command substitution, which
    /// an unquoted delimiter leaves live. Read under the same rule as one
    /// outside a here-document -- refused when it names a build script, since
    /// that is where not reading it would change the answer.
    fn body_is_readable(&self, body: &str) -> Result<(), String> {
        if !self.expanded {
            return Ok(());
        }
        let cs: Vec<char> = body.chars().collect();
        let mut sink = Words::default();
        let mut i = 0;
        while i < cs.len() {
            i = match cs[i] {
                '\\' if i + 1 < cs.len() => i + 2,
                '`' => read_substitution(&cs, i + 1, '`', &mut sink).map_err(|(_, why)| why)?,
                '$' if cs.get(i + 1) == Some(&'(') => read_substitution(&cs, i + 2, ')', &mut sink).map_err(|(_, why)| why)?,
                _ => i + 1,
            };
        }
        Ok(())
    }

    /// The finding for a here-document whose delimiter is nowhere below it.
    /// Everything after the `<<` is then body or is not, and which of those it
    /// is decides whether any of it is a build.
    fn never_arrives(&self) -> String {
        format!("a here-document whose delimiter `{}` never arrives, so where its body ends is unknown", self.delim)
    }
}

/// The script's lines with `\`-continued ones joined, since a `step` that
/// runs onto a second line is still one command. The backslash and newline
/// are kept, for `shell_commands` to read as the continuation they are.
/// A trailing `\` is a continuation when there is an odd number of them:
/// `\\` at the end of a line is one escaped backslash and ends it.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut carry: Option<String> = None;
    for line in text.lines() {
        let joined = match carry.take() {
            Some(prev) => format!("{prev}\n{line}"),
            None => line.to_string(),
        };
        if (line.len() - line.trim_end_matches('\\').len()) % 2 == 1 {
            carry = Some(joined);
        } else {
            out.push(joined);
        }
    }
    out.extend(carry);
    out
}

/// The commands a piece of shell runs, each as its list of words, together
/// with the here-documents still open when the text ran out -- or the offset
/// and description of a shape this reader will not interpret.
///
/// This is not a shell, and it does not expand anything: it knows exactly as
/// much as it takes to say where a command begins -- single quotes, double
/// quotes, backslash escapes, `#` comments, the separators `; & | newline`,
/// the grouping `( )`, and here-document redirections, whose bodies are
/// input and not commands. Quote removal is deliberate, because quoting does
/// not stop a command running: `"./examples/c/build.sh"` is a build.
/// Everything it is unsure of is an `Err` rather than a guess.
fn shell_commands(shell: &str) -> Result<Tokenized, (usize, String)> {
    let cs: Vec<char> = shell.chars().collect();
    let mut s = Words::default();
    let mut pending: Vec<HereDoc> = vec![];
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            // a `\` newline is a continuation and joins nothing; any other
            // escaped character is that character, separators included
            '\\' => {
                let Some(&next) = cs.get(i + 1) else { return Err((i, "a line ending in a lone `\\`".into())) };
                if next != '\n' {
                    s.push(next);
                }
                i += 2;
            }
            '\'' => {
                let Some(end) = cs[i + 1..].iter().position(|&c| c == '\'') else { return Err((i, "an unterminated `'`".into())) };
                s.start();
                for &c in &cs[i + 1..i + 1 + end] {
                    s.push(c);
                }
                i += end + 2;
            }
            '"' => i = read_double_quoted(&cs, i, &mut s)?,
            // `#` opens a comment only at the start of a word, as in the shell
            '#' if !s.in_word => {
                while i < cs.len() && cs[i] != '\n' {
                    i += 1;
                }
            }
            // a here-document redirection: the operator and its delimiter are
            // no part of any word, and the body starts on the next line
            '<' if cs.get(i + 1) == Some(&'<') => {
                s.end_word();
                i = read_here_doc_redirection(&cs, i, &mut pending)?;
            }
            ';' | '&' | '|' => {
                s.end_command();
                i += 1;
            }
            // the newline that ends the command is also where the bodies of
            // the here-documents opened on it begin, in the order opened
            '\n' => {
                s.end_command();
                i += 1;
                while let Some(doc) = pending.first() {
                    let Some(end) = skip_here_doc_body(&cs, i, doc)? else {
                        // the rest of the text is body; the document stays
                        // open, for the caller to answer out of what it has
                        i = cs.len();
                        break;
                    };
                    i = end;
                    pending.remove(0);
                }
            }
            // a subshell or a brace group runs its contents as commands of
            // their own, so either bracket ends the command before it
            '(' | ')' | '{' | '}' => {
                s.end_command();
                i += 1;
            }
            '`' => i = read_substitution(&cs, i + 1, '`', &mut s)?,
            '$' if cs.get(i + 1) == Some(&'(') => i = read_substitution(&cs, i + 2, ')', &mut s)?,
            c if c.is_whitespace() => {
                s.end_word();
                i += 1;
            }
            c => {
                s.push(c);
                i += 1;
            }
        }
    }
    Ok(Tokenized { commands: s.finish(), pending })
}

/// What one piece of shell held: its commands, and the here-documents whose
/// bodies were not in it.
struct Tokenized {
    commands: Vec<Vec<String>>,
    pending: Vec<HereDoc>,
}

/// A `<<` or `<<-` and the delimiter word after it, from the `<` at `at`.
/// Pushes the here-document it opens and returns where the word ends.
///
/// `<<<` is a here-string, a different thing with no body, and it is refused
/// rather than read as a here-document on the delimiter `<word`.
fn read_here_doc_redirection(cs: &[char], at: usize, pending: &mut Vec<HereDoc>) -> Result<usize, (usize, String)> {
    if cs.get(at + 2) == Some(&'<') {
        return Err((at, "a here-string (`<<<`), whose word this reader does not model".into()));
    }
    let (strip_tabs, from) = if cs.get(at + 2) == Some(&'-') { (true, at + 3) } else { (false, at + 2) };
    let Some((delim, quoted, end)) = here_doc_delimiter(cs, from) else {
        return Err((at, "a here-document redirection with no delimiter word after it".into()));
    };
    pending.push(HereDoc { delim, strip_tabs, expanded: !quoted });
    Ok(end)
}

/// The delimiter word of a here-document, its quoting, and where it ends.
/// Quoting anywhere in the word -- `'EOF'`, `"EOF"`, `\EOF` -- makes the body
/// literal, which is the shell's rule and the one that decides whether the
/// body can still run anything.
fn here_doc_delimiter(cs: &[char], from: usize) -> Option<(String, bool, usize)> {
    let mut i = from;
    while matches!(cs.get(i), Some(' ' | '\t')) {
        i += 1;
    }
    let (mut word, mut quoted) = (String::new(), false);
    while let Some(&c) = cs.get(i) {
        match c {
            '\'' | '"' => {
                let end = cs[i + 1..].iter().position(|&e| e == c)?;
                word.extend(&cs[i + 1..i + 1 + end]);
                quoted = true;
                i += end + 2;
            }
            '\\' => {
                word.push(*cs.get(i + 1)?);
                quoted = true;
                i += 2;
            }
            ';' | '&' | '|' | '<' | '>' | '(' | ')' | '{' | '}' | '#' => break,
            c if c.is_whitespace() => break,
            c => {
                word.push(c);
                i += 1;
            }
        }
    }
    (!word.is_empty()).then_some((word, quoted, i))
}

/// A here-document's body, consumed as the input it is. Returns where the text
/// after the terminator line begins, or `None` when the delimiter never
/// arrives in this text -- the body then runs past what we were handed, which
/// is the caller's question and not this one's.
fn skip_here_doc_body(cs: &[char], from: usize, doc: &HereDoc) -> Result<Option<usize>, (usize, String)> {
    let mut i = from;
    while i < cs.len() {
        let end = cs[i..].iter().position(|&c| c == '\n').map_or(cs.len(), |n| i + n);
        if doc.ends(&cs[i..end].iter().collect::<String>()) {
            let body: String = cs[from..i].iter().collect();
            doc.body_is_readable(&body).map_err(|why| (from, format!("{why}, in the body of `<<{}`", doc.delim)))?;
            return Ok(Some((end + 1).min(cs.len())));
        }
        i = end + 1;
    }
    Ok(None)
}

/// A `"..."` span: its text belongs to the word around it however many
/// separators are inside it. Substitutions inside double quotes still run, so
/// they are handled exactly as they are outside.
fn read_double_quoted(cs: &[char], at: usize, s: &mut Words) -> Result<usize, (usize, String)> {
    let mut i = at + 1;
    s.start();
    while i < cs.len() {
        match cs[i] {
            '\\' if matches!(cs.get(i + 1), Some('"' | '\\' | '$' | '`' | '\n')) => {
                if cs[i + 1] != '\n' {
                    s.push(cs[i + 1]);
                }
                i += 2;
            }
            '"' => return Ok(i + 1),
            '`' => i = read_substitution(cs, i + 1, '`', s)?,
            '$' if cs.get(i + 1) == Some(&'(') => i = read_substitution(cs, i + 2, ')', s)?,
            c => {
                s.push(c);
                i += 1;
            }
        }
    }
    Err((at, "an unterminated `\"`".into()))
}

/// A `$(...)` or backtick span, consumed whole. Its contents are commands
/// this reader does not read: in the two CI scripts a substitution only ever
/// produces a value -- `$(uname -s)`, `$(mktemp -d ...)`, `$(date +%Y%m%d)`.
/// One that names a build script is the single case where that silence would
/// change the answer, and that is refused rather than guessed at.
fn read_substitution(cs: &[char], from: usize, close: char, s: &mut Words) -> Result<usize, (usize, String)> {
    let (mut depth, mut i, mut body) = (1usize, from, String::new());
    while i < cs.len() {
        let c = cs[i];
        if c == '\\' && i + 1 < cs.len() {
            body.push(cs[i + 1]);
            i += 2;
            continue;
        }
        if close == ')' && c == '(' {
            depth += 1;
        }
        if c == close {
            depth -= 1;
            if depth == 0 {
                if body.contains("build.sh") {
                    return Err((from, format!("a command substitution naming a build script -- `{}`", body.trim())));
                }
                // the substitution is part of the word around it, and a word
                // with one in it is not a bare path to a build script
                s.push(SUBSTITUTED);
                return Ok(i + 1);
            }
        }
        body.push(c);
        i += 1;
    }
    Err((from, format!("an unterminated `{}`", if close == ')' { "$(" } else { "`" })))
}

/// Stands in a word for a `$(...)` whose value we did not compute, so that
/// the word can never be mistaken for a literal path. No path holds one.
const SUBSTITUTED: char = '\0';

/// The words and commands `shell_commands` is building up. A word stays open
/// once started even while empty, so that `""` is an (empty) argument and not
/// nothing at all.
#[derive(Default)]
struct Words {
    commands: Vec<Vec<String>>,
    command: Vec<String>,
    word: String,
    in_word: bool,
}

impl Words {
    fn start(&mut self) {
        self.in_word = true;
    }
    fn push(&mut self, c: char) {
        self.word.push(c);
        self.in_word = true;
    }
    fn end_word(&mut self) {
        if self.in_word {
            self.command.push(std::mem::take(&mut self.word));
            self.in_word = false;
        }
    }
    fn end_command(&mut self) {
        self.end_word();
        if !self.command.is_empty() {
            self.commands.push(std::mem::take(&mut self.command));
        }
    }
    fn finish(mut self) -> Vec<Vec<String>> {
        self.end_command();
        self.commands
    }
}

/// The shell every `run:` step of a GitHub workflow holds, in order, skipping
/// steps switched off with `if: false`.
///
/// Hand-written rather than pulled from a YAML crate: this is the only YAML
/// the checker reads, the workspace carries no YAML dependency, and what is
/// wanted is one key. It handles what `ci.yml` uses -- a step is an item of a
/// block sequence, `run:` is either a plain scalar or a `|` block -- and a
/// shape it does not understand yields no build, which fails towards "not
/// built" and so towards a row that cannot be `supported`.
fn workflow_run_scripts(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let indent = |s: &str| s.len() - s.trim_start().len();
    let mut out = vec![];
    let mut i = 0;
    while i < lines.len() {
        let (line, t) = (lines[i], lines[i].trim_start());
        // a sequence item: `- name: ...`. A commented-out step is a comment.
        if !(t.starts_with("- ") || t == "-") {
            i += 1;
            continue;
        }
        let item_indent = indent(line);
        // the item's own lines, the `- ` blanked so that its keys line up
        let mut item = vec![format!("{}  {}", " ".repeat(item_indent), t.strip_prefix("- ").unwrap_or(""))];
        i += 1;
        while i < lines.len() && (lines[i].trim().is_empty() || indent(lines[i]) > item_indent) {
            item.push(lines[i].to_string());
            i += 1;
        }
        if let Some(sh) = run_script_of(&item, item_indent + 2) {
            out.push(sh);
        }
    }
    out
}

/// The `run:` script of one step, or `None` when it has none or is switched
/// off. `key_indent` is where the step's own keys start.
fn run_script_of(item: &[String], key_indent: usize) -> Option<String> {
    let at_key = |l: &String| l.len() - l.trim_start().len() == key_indent && !l.trim_start().starts_with('#');
    for l in item.iter().filter(|l| at_key(l)) {
        if let Some(v) = l.trim_start().strip_prefix("if:") {
            let v = v.trim().trim_start_matches("${{").trim_end_matches("}}").trim().trim_matches(['"', '\'']).to_ascii_lowercase();
            if v == "false" {
                return None;
            }
        }
    }
    let at = item.iter().position(|l| at_key(l) && l.trim_start().starts_with("run:"))?;
    let rest = item[at].trim_start().strip_prefix("run:")?.trim();
    // `run: |` (or `>`): the block is every deeper-indented line that follows,
    // with the block's own indentation taken off -- that indentation is YAML's
    // and the runner strips it before any shell sees the script. It matters
    // because a here-document's terminator has to be at column zero to end the
    // body, so reading the block indented makes a closed here-document look
    // unterminated (review of #207).
    if rest.starts_with('|') || rest.starts_with('>') {
        let body: Vec<&str> = item[at + 1..].iter().take_while(|l| l.trim().is_empty() || l.len() - l.trim_start().len() > key_indent).map(|l| l.as_str()).collect();
        // YAML's rule: the first non-empty line sets it, and a line indented
        // further keeps the difference
        let pad = " ".repeat(body.iter().find(|l| !l.trim().is_empty()).map_or(0, |l| l.len() - l.trim_start().len()));
        return Some(body.iter().map(|l| l.strip_prefix(pad.as_str()).unwrap_or(l.trim_start())).collect::<Vec<_>>().join("\n"));
    }
    Some(yaml_scalar(rest))
}

/// A one-line YAML scalar's text. A *quoted* scalar carries the same quote at
/// both ends and they are YAML's, not the shell's; a plain scalar carries
/// whatever it likes, quotes included. Trimming quotes off either end
/// regardless turned `run: echo "a; ./examples/x/build.sh"` into shell with
/// one unbalanced `"` in it, which the reader then rightly refused to read
/// (review of #207).
fn yaml_scalar(text: &str) -> String {
    let q = text.chars().next();
    if text.len() >= 2 && matches!(q, Some('"') | Some('\'')) && text.chars().last() == q {
        return text[1..text.len() - 1].to_string();
    }
    text.to_string()
}

/// True when `text` names `examples/<example>` in *code*: as a whole path
/// component, so that a test reading `examples/c-lib` is not taken for one
/// reading `examples/c`, and outside a comment.
///
/// Comments are excluded because prose is not a read. `feature.security` was
/// `supported` on a proof whose only mention of `examples/c` was a `//!` line
/// saying the test needs no fixture -- and it does not open one (self-review
/// of #207). A comment is the one place a file can name an example it never
/// touches, so it is the one place that must not count.
fn mentions_example(text: &str, example: &str) -> bool {
    let needle = format!("examples/{example}");
    text.lines()
        .filter(|l| !is_comment_line(l))
        .any(|l| l.match_indices(&needle).any(|(i, _)| !l[i + needle.len()..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')))
}

/// A line that is nothing but a comment, in the languages the proofs are
/// written in: Rust and JavaScript (`//`, `///`, `//!`, and a `*` or `/*`
/// inside a block comment) and shell, TOML or YAML (`#`). Deliberately
/// whole-line: a path in code with a comment after it is still code.
fn is_comment_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with('#') || t.starts_with("/*") || t.starts_with('*')
}

/// Every directory that must back a row. `build-systems` is a folder of
/// examples rather than an example, so its children are what count.
fn example_dirs(tree: &dyn Tree) -> Vec<String> {
    let mut out = vec![];
    for d in tree.subdirs("examples") {
        if d == "build-systems" {
            out.extend(tree.subdirs("examples/build-systems").into_iter().map(|s| format!("build-systems/{s}")));
        } else {
            out.push(d);
        }
    }
    out
}

/// Rule 2, the part that is about the lists rather than about a row: the
/// hosted build list equals the local one minus the declared local-only
/// examples. Returns the examples whose declaration *holds* -- a declaration
/// with a finding against it licenses nothing, so a row leaning on it is
/// judged as if it had not been declared -- which is what the per-row half of
/// the rule keys on. Each declared example maps to whether its declaration
/// holds, so that a row resting on a broken one is told that, rather than
/// told its declaration is missing.
fn check_build_lists(decls: &[LocalOnly], in_yml: &BTreeSet<String>, in_local: &BTreeSet<String>, tree: &dyn Tree, out: &mut Vec<String>) -> BTreeMap<String, bool> {
    let mut declared = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for d in decls {
        let ex = d.example.as_str();
        let mut found = vec![];
        let mut fail = |msg: String| found.push(format!("local_only `{ex}`: {msg}"));
        if ex.is_empty() {
            out.push("local_only ``: names no example".into());
            continue;
        }
        if !seen.insert(ex.to_string()) {
            out.push(format!("local_only `{ex}`: declared twice"));
            continue;
        }
        if d.reason.trim().is_empty() {
            fail("no reason is written: say why hosted CI does not build it".into());
        }
        if d.tracking.contains(&0) {
            fail("tracking names issue #0".into());
        }
        if !tree.is_dir(&format!("examples/{ex}")) {
            fail(format!("example directory `examples/{ex}` does not exist"));
        }
        if !in_local.contains(ex) {
            fail(format!("`examples/{ex}/build.sh` is not run by {CI_LOCAL}: a local-only example is one the local script builds; this one is built nowhere (its rows say \"not built in CI\" instead)"));
        }
        if in_yml.contains(ex) {
            fail(format!("`examples/{ex}/build.sh` is now run by {CI_YML} as well: the declaration is out of date, remove it and the \"verified locally\" caveats with it"));
        }
        declared.insert(ex.to_string(), found.is_empty());
        out.extend(found);
    }
    for ex in in_local.difference(in_yml).filter(|ex| !seen.contains(*ex)) {
        out.push(format!(
            "`examples/{ex}/build.sh` is run by {CI_LOCAL} but not by {CI_YML}, and {MATRIX} does not declare it under [[local_only]]: ci-local exists to mirror CI, so build it in both or declare the exception and its reason"
        ));
    }
    for ex in in_yml.difference(in_local) {
        out.push(format!("`examples/{ex}/build.sh` is run by {CI_YML} but not by {CI_LOCAL}: ci-local exists to mirror CI, and there is no hosted-only exception"));
    }
    declared
}

/// Runs every rule and returns every finding, each naming its row. All of
/// them rather than the first: a matrix edit that breaks three rows should
/// say so once.
fn check(matrix_text: &str, tree: &dyn Tree) -> Vec<String> {
    let m: Matrix = match toml::from_str(matrix_text) {
        Ok(m) => m,
        Err(e) => return vec![format!("{MATRIX} does not parse: {e}")],
    };
    let mut out = vec![];
    if m.schema != 1 {
        out.push(format!("{MATRIX}: schema {} is not one this checker understands (1)", m.schema));
    }
    if m.row.is_empty() {
        out.push(format!("{MATRIX} has no rows"));
    }
    let ci = (tree.read(CI_YML), tree.read(CI_LOCAL));
    let (Some(yml), Some(local)) = ci else {
        out.push(format!("cannot read {CI_YML} and {CI_LOCAL}: the CI build list is unknown"));
        return out;
    };
    let (yml_builds, local_builds) = (built_by(&yml, true), built_by(&local, false));
    // A line the reader would have had to guess at is a finding in its own
    // right, before any row is judged: the build list underneath rule 2 is
    // not known, and an unknown is reported rather than guessed (#207).
    for (file, why) in yml_builds.unreadable.iter().map(|w| (CI_YML, w)).chain(local_builds.unreadable.iter().map(|w| (CI_LOCAL, w))) {
        out.push(format!("{file}: this line's shell is a shape the build-list reader does not interpret -- {why}. Rule 2 rests on which build scripts CI runs, so rewrite the line plainly or teach the reader the shape; it is not assumed either way"));
    }
    let (in_yml, in_local) = (&yml_builds.examples, &local_builds.examples);
    let local_only = check_build_lists(&m.local_only, in_yml, in_local, tree, &mut out);

    let mut seen = BTreeMap::new();
    let mut backed = BTreeSet::new();
    for r in &m.row {
        let id = if r.id.is_empty() { "<row with no id>" } else { r.id.as_str() };
        let mut fail = |msg: String| out.push(format!("row `{id}`: {msg}"));

        if r.id.is_empty() {
            fail(format!("has no id (name: {:?})", r.name));
        } else if let Some(first) = seen.insert(r.id.clone(), r.name.clone()) {
            fail(format!("duplicate id: also used by {first:?}"));
        }
        if r.name.is_empty() {
            fail("has no name".into());
        }
        if r.group.is_empty() {
            fail("has no group: the rendered table needs a heading to put it under".into());
        }
        if r.tracking.contains(&0) {
            fail("tracking names issue #0".into());
        }
        if !["language", "feature", "toolchain", "architecture"].contains(&r.area.as_str()) {
            fail(format!("area {:?} is not one of language, feature, toolchain, architecture", r.area));
        }

        let claims = match r.state.as_str() {
            "supported" => true,
            "caveats" => {
                if r.caveat.trim().is_empty() {
                    fail("state is `caveats` but no caveat is written".into());
                }
                true
            }
            "not-supported" => {
                if r.reason.trim().is_empty() {
                    fail("state is `not-supported` but no reason is written".into());
                }
                false
            }
            other => {
                fail(format!("state {other:?} is not one of supported, caveats, not-supported"));
                continue;
            }
        };
        if r.state == "supported" && !r.caveat.trim().is_empty() {
            fail("state is `supported` but a caveat is written: a row with a limit is `caveats`".into());
        }

        // A not-supported row claims nothing, so it needs no evidence -- but
        // if it names an example anyway, that example is backed by it.
        if !r.example.is_empty() && tree.is_dir(&format!("examples/{}", r.example)) {
            backed.insert(r.example.clone());
        }
        if !claims {
            continue;
        }

        // 1. the example exists
        if r.example.is_empty() {
            fail(format!("state is `{}` but no example is named: no claim of support without a worked example (#38)", r.state));
            continue;
        }
        let dir = format!("examples/{}", r.example);
        if !tree.is_dir(&dir) {
            fail(format!("example directory `{dir}` does not exist"));
            continue;
        }

        // 2. the example is built by CI. A declared local-only example is
        // built, but not where the hosted badge says: its rows carry that.
        let says = |phrase: &str| r.caveat.to_lowercase().contains(phrase);
        let missing: Vec<&str> = [(CI_YML, in_yml), (CI_LOCAL, in_local)].iter().filter(|(_, set)| !set.contains(&r.example)).map(|(f, _)| *f).collect();
        if local_only.get(&r.example) == Some(&false) {
            // The declaration is there and does not hold, so whether this row
            // is licensed cannot be answered yet. Say that, rather than "the
            // declaration is missing", which it is not.
            fail(format!("`{dir}` is declared [[local_only]] and that declaration does not hold (its own finding says why): this row's caveat is judged once it does"));
        } else if local_only.contains_key(&r.example) {
            if r.state == "supported" {
                fail(format!("`{dir}` is declared [[local_only]] -- {CI_YML} does not build it -- so the row cannot be `supported`: make it `caveats` and say it is verified locally, not in hosted CI"));
            } else if !says(LOCAL_ONLY) {
                fail(format!("`{dir}` is declared [[local_only]], and the caveat does not say so (it must contain the words \"verified locally, not in hosted CI\")"));
            }
            if says(NOT_IN_CI) {
                fail(format!("the caveat says the example is not built in CI, but `{dir}` is declared [[local_only]]: {CI_LOCAL} does build it"));
            }
        } else if says(LOCAL_ONLY) {
            fail(format!("the caveat says \"verified locally, not in hosted CI\", but `{dir}` is not declared [[local_only]] in {MATRIX}: the caveat is out of date, or the declaration is missing"));
        } else if !missing.is_empty() {
            let what = format!("`{dir}/build.sh` is not run by {}", missing.join(" or "));
            if r.state == "supported" {
                fail(format!("{what}, so the row cannot be `supported`: make it `caveats` and say the example is not built in CI, or build it (#152)"));
            } else if !says(NOT_IN_CI) {
                fail(format!("{what}, and the caveat does not say so (it must contain the words \"not built in CI\")"));
            }
        } else if says(NOT_IN_CI) {
            fail(format!("the caveat says the example is not built in CI, but `{dir}/build.sh` is run by both {CI_YML} and {CI_LOCAL}: the caveat is out of date"));
        }

        // 3. the construct is still where the row says it is. A language or
        // toolchain row is a claim about a construct, so it must point at
        // one; a feature or an architecture often has no single line to
        // point at, and a forced citation would be noise. Given, it is
        // checked whatever the area.
        let needs_evidence = matches!(r.area.as_str(), "language" | "toolchain");
        if r.evidence.is_empty() && r.pattern.is_empty() && !needs_evidence {
            // nothing claimed, nothing to check
        } else {
            match r.evidence.rsplit_once(':').and_then(|(p, l)| Some((p, l.parse::<usize>().ok().filter(|l| *l > 0)?))) {
                None => fail(format!("evidence {:?} is not `path:line`", r.evidence)),
                Some((path, line)) => {
                    if !path.starts_with(&format!("{dir}/")) {
                        fail(format!("evidence `{path}` is not inside the row's example `{dir}`"));
                    }
                    // A construct lives in code. `rust.prologue-frame` cited a
                    // README sentence with the word "prologue" in it, and would
                    // have gone on passing with the whole crate deleted
                    // (self-review of #207).
                    if needs_evidence && path.to_lowercase().ends_with(".md") {
                        fail(format!(
                            "evidence `{path}` is prose: a {} row must point at the construct in code, not at a sentence about it (the sentence survives deleting the construct)",
                            r.area
                        ));
                    }
                    match (tree.read(path), Regex::new(&r.pattern)) {
                        (None, _) => fail(format!("evidence file `{path}` does not exist")),
                        (_, Err(e)) => fail(format!("pattern {:?} is not a valid regex: {e}", r.pattern)),
                        (Some(_), Ok(_)) if r.pattern.is_empty() => fail("has evidence but no pattern to look for there".into()),
                        (Some(text), Ok(re)) => {
                            let lines: Vec<&str> = text.lines().collect();
                            if line > lines.len() {
                                fail(format!("evidence `{path}:{line}` is past the end of the file ({} lines)", lines.len()));
                            } else {
                                let lo = line.saturating_sub(EVIDENCE_WINDOW).max(1);
                                let hi = (line + EVIDENCE_WINDOW).min(lines.len());
                                if !lines[lo - 1..hi].iter().any(|l| re.is_match(l)) {
                                    fail(format!(
                                        "pattern {:?} no longer matches `{path}` within {EVIDENCE_WINDOW} lines of line {line}: the construct has moved or gone",
                                        r.pattern
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        // 4. the proof is still there
        if r.proof.is_empty() {
            if r.state == "supported" {
                fail("names no proof: which test shows what butterfly does with this?".into());
            } else if !r.caveat.to_lowercase().contains(NO_TEST) {
                fail("names no proof, and the caveat does not say so (it must contain the words \"no test\")".into());
            }
        } else {
            let (path, test) = match r.proof.split_once("::") {
                Some((p, t)) => (p, Some(t)),
                None => (r.proof.as_str(), None),
            };
            match tree.read(path) {
                None => fail(format!("proof file `{path}` does not exist")),
                Some(text) => {
                    if let Some(t) = test {
                        // a Rust test is a function; a node:test case is a title
                        let found = if path.ends_with(".rs") { Regex::new(&format!(r"\bfn\s+{}\s*\(", regex::escape(t))).unwrap().is_match(&text) } else { text.contains(t) };
                        if !found {
                            fail(format!("proof `{path}` no longer contains the test `{t}`"));
                        }
                    }
                    if r.state == "supported" && !mentions_example(&text, &r.example) {
                        fail(format!("proof `{path}` never names `{dir}`: a `supported` row's proof must read the example it cites (otherwise it is `caveats`, and the caveat says what the proof runs on)"));
                    }
                }
            }
        }
    }

    // 5. no orphans
    for d in example_dirs(tree) {
        if !backed.contains(&d) {
            out.push(format!("example `examples/{d}` backs no row in {MATRIX}: add the row it proves, or remove the example"));
        }
    }
    out
}

fn repo() -> Disk {
    Disk(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// The check itself. Needs no built fixture and no compiler, so it never
/// skips: it reads sources, not build outputs.
#[test]
fn every_claim_in_the_support_matrix_still_has_its_evidence() {
    let tree = repo();
    let text = tree.read(MATRIX).unwrap_or_else(|| panic!("{MATRIX} is missing"));
    let findings = check(&text, &tree);
    assert!(findings.is_empty(), "\n{MATRIX} makes {} claim(s) the tree no longer backs:\n  - {}\n", findings.len(), findings.join("\n  - "));
}

/// The build list is read out of two files nobody writes with this test in
/// mind. If a reformat ever made the parser find nothing, rule 2 would fail
/// every row at once with a misleading message; this says what happened.
#[test]
fn the_ci_build_list_is_read_from_both_scripts() {
    let tree = repo();
    let (yml, local) = (built_by(&tree.read(CI_YML).unwrap(), true), built_by(&tree.read(CI_LOCAL).unwrap(), false));
    assert_eq!((&yml.unreadable[..], &local.unreadable[..]), (&[][..], &[][..]), "the build-list reader declined to read a line of the real CI scripts");
    let (yml, local) = (yml.examples, local.examples);
    assert!(yml.contains("c") && local.contains("c"), "examples/c is the fixture everything else stands on: {yml:?} / {local:?}");
    // Whether the two lists *agree* is rule 2's business, and the test above
    // applies it to this tree: hosted == local minus the declared local-only
    // examples. What is pinned here is that the parser still sees the hosted
    // list inside the local one -- nothing is ever hosted-only.
    assert!(yml.is_subset(&local), "{CI_YML} builds examples {CI_LOCAL} does not: {:?}", yml.difference(&local).collect::<Vec<_>>());
}

// ───────────────────────── the rules, on a tree in memory ─────────────────

#[derive(Default, Clone)]
struct Mem(BTreeMap<String, String>);

impl Mem {
    fn with(mut self, path: &str, text: &str) -> Self {
        self.0.insert(path.into(), text.into());
        self
    }
}

impl Tree for Mem {
    fn read(&self, path: &str) -> Option<String> {
        self.0.get(path).cloned()
    }
    fn is_dir(&self, path: &str) -> bool {
        let p = format!("{path}/");
        self.0.keys().any(|k| k.starts_with(&p))
    }
    fn subdirs(&self, path: &str) -> Vec<String> {
        let p = format!("{path}/");
        let set: BTreeSet<String> = self.0.keys().filter_map(|k| k.strip_prefix(&p)).filter_map(|rest| rest.split_once('/')).map(|(d, _)| d.to_string()).collect();
        set.into_iter().collect()
    }
}

/// One example built by both scripts, one test that reads it.
fn small_tree() -> Mem {
    Mem::default()
        .with(CI_YML, "jobs:\n  test:\n    steps:\n      # ./examples/ghost/build.sh is only a comment\n      - run: ./examples/c/build.sh\n")
        .with(CI_LOCAL, "step \"./examples/c/build.sh\" ./examples/c/build.sh\nprintf '%s' \"./examples/ghost/build.sh SKIP\"\n")
        .with("examples/c/build.sh", "clang demo.c\n")
        .with("examples/c/demo.c", "// header\nint f(int a, int b) {\n    if (a && b) {\n        return 1;\n    }\n    return 0;\n}\n")
        // the mention that counts is the one in code; the comment above it is
        // there so the fixture carries both shapes
        .with("crates/ingest/tests/e2e.rs", "// reads ../../examples/c/build\n#[test]\nfn it_works() {\n    ingest(\"../../examples/c/build\");\n}\n")
        .with("ui/model.test.cjs", "test('recursion terminates', () => {});\n")
}

const GOOD: &str = r#"
[[row]]
id = "c.decision"
area = "language"
group = "C"
name = "a decision"
state = "supported"
example = "c"
evidence = "examples/c/demo.c:3"
pattern = 'a && b'
proof = "crates/ingest/tests/e2e.rs::it_works"
"#;

fn matrix(rows: &str) -> String {
    format!("schema = 1\n{rows}")
}

/// `check`, asserting exactly one finding, that it names the row and says
/// what is wrong.
fn only_finding(rows: &str, tree: &Mem, id: &str, says: &str) {
    // A row that loses its example also leaves that example backing
    // nothing; that second finding is correct, and rule 5's own test covers it.
    let f: Vec<String> = check(&matrix(rows), tree).into_iter().filter(|m| !m.contains("backs no row")).collect();
    assert_eq!(f.len(), 1, "{f:#?}");
    assert!(f[0].contains(&format!("`{id}`")), "does not name the row: {}", f[0]);
    assert!(f[0].contains(says), "expected {says:?} in: {}", f[0]);
}

#[test]
fn a_row_with_all_its_evidence_passes() {
    assert_eq!(check(&matrix(GOOD), &small_tree()), Vec::<String>::new());
}

#[test]
fn rule_1_a_missing_example_names_the_row() {
    only_finding(&GOOD.replace("example = \"c\"", "example = \"c-gone\""), &small_tree(), "c.decision", "`examples/c-gone` does not exist");
    only_finding(&GOOD.replace("example = \"c\"\n", ""), &small_tree(), "c.decision", "no example is named");
}

#[test]
fn rule_2_supported_needs_an_example_ci_builds() {
    let tree = small_tree().with("examples/cpp/flight.cpp", "template <typename T>\n");
    let row = r#"
[[row]]
id = "cpp.templates"
area = "language"
group = "C++"
name = "templates"
state = "supported"
example = "cpp"
evidence = "examples/cpp/flight.cpp:1"
pattern = 'template'
proof = "crates/ingest/tests/e2e.rs"
"#;
    // `supported` on an unbuilt example: two findings, because a proof that
    // reads examples/c is not a proof about examples/cpp either
    let f = check(&matrix(&format!("{GOOD}{row}")), &tree);
    assert!(f.iter().any(|m| m.contains("`cpp.templates`") && m.contains("cannot be `supported`")), "{f:#?}");

    // `caveats` is allowed to lean on it, but only by saying so
    let quiet = row.replace("\"supported\"", "\"caveats\"\ncaveat = \"merges instantiations\"");
    only_finding(&format!("{GOOD}{quiet}"), &tree, "cpp.templates", "the caveat does not say so");
    let honest = row.replace("\"supported\"", "\"caveats\"\ncaveat = \"The example is not built in CI (#152).\"");
    assert_eq!(check(&matrix(&format!("{GOOD}{honest}")), &tree), Vec::<String>::new());

    // and the phrase cannot outlive the gap it describes
    let stale = GOOD.replace("\"supported\"", "\"caveats\"\ncaveat = \"not built in CI\"");
    only_finding(&stale, &small_tree(), "c.decision", "the caveat is out of date");
}

#[test]
fn a_build_script_named_only_in_a_comment_or_a_skip_message_is_not_built() {
    let t = small_tree();
    assert_eq!(built_by(&t.read(CI_YML).unwrap(), true).examples.into_iter().collect::<Vec<_>>(), ["c"]);
    assert_eq!(built_by(&t.read(CI_LOCAL).unwrap(), false).examples.into_iter().collect::<Vec<_>>(), ["c"]);
    // an example built by one script and not the other is not "built by CI":
    // the row says so, and so does the comparison of the two lists
    let half = small_tree().with(CI_LOCAL, "# nothing\n");
    let f = check(&matrix(GOOD), &half);
    assert_eq!(f.len(), 2, "{f:#?}");
    assert!(f.iter().any(|m| m.contains("`c.decision`") && m.contains("is not run by scripts/ci-local.sh")), "{f:#?}");
    assert!(f.iter().any(|m| m.contains("`examples/c/build.sh` is run by .github/workflows/ci.yml but not by scripts/ci-local.sh")), "{f:#?}");
}

/// Rule 2 is only as good as the reading of the two scripts, and reading them
/// as text was not good enough: any `ci.yml` line that merely *named* a build
/// script counted as a build, so one `echo` -- or the `grep` that filters the
/// skip report, once someone normalised its path -- promoted a local-only
/// example to `supported` (review of #207). What counts is command position.
#[test]
fn a_build_script_a_workflow_only_mentions_is_not_built_by_it() {
    let yml = r#"
jobs:
  test:
    steps:
      # A step that was commented out builds nothing.
      # - name: Build the commented example
      #   run: ./examples/ghost-comment/build.sh
      - name: Build the C example
        run: ./examples/c/build.sh
      - name: Say what is deliberately not built here
        run: echo "./examples/ghost-echo/build.sh is deliberately NOT run in hosted CI"
      - name: No suite may skip
        run: |
          # ./examples/ghost-hash/build.sh is named in a shell comment
          grep "skipping:" test.log | grep -Fv "./examples/ghost-grep/build.sh" > skipped.txt || true
          printf '%s\n' "./examples/ghost-printf/build.sh"
      - name: Turned off, and still in the file
        if: false
        run: ./examples/ghost-disabled/build.sh
      - name: Two builds, one line, one behind env
        run: ./examples/c-lib/build.sh && env CC=gcc ./examples/c-gcc/build.sh
"#;
    assert_eq!(built_by(yml, true).examples.into_iter().collect::<Vec<_>>(), ["c", "c-gcc", "c-lib"]);

    // the same four disguises in the local script, where a build must also go
    // through `step` -- and where the label names the script without running it
    let sh = r#"
# ./examples/ghost-comment/build.sh
step "./examples/c/build.sh"                ./examples/c/build.sh
step "./examples/c-gcc/build.sh (CC=$CC)"   env CC="$CC" ./examples/c-gcc/build.sh
step "./examples/ghost-label/build.sh"      echo skipped
printf '%-45s %s\n' "./examples/ghost-printf/build.sh" "SKIP"
echo ./examples/ghost-echo/build.sh
grep -Fv "./examples/ghost-grep/build.sh" "$WORK/skips.all" > "$WORK/skips.txt"
"#;
    assert_eq!(built_by(sh, false).examples.into_iter().collect::<Vec<_>>(), ["c", "c-gcc"]);

    // and the reading a false `supported` would have needed: an `echo` in
    // hosted CI does not make a local-only example a hosted build
    let t = qemu_tree();
    let mentioned = format!("{}      - name: Not here\n        run: echo \"./examples/rust-qemu/build.sh is NOT run in hosted CI\"\n", t.read(CI_YML).unwrap());
    let f = check(&qemu_matrix(DECLARED, QEMU_ROW), &t.with(CI_YML, &mentioned));
    assert_eq!(f, Vec::<String>::new(), "an echo was read as a hosted build");
}

/// The same question one layer down: *where does a command end*. Reading the
/// line as text and cutting it at every `;`, `|` and `&` put a build script
/// in command position that was never in one -- it was a `;` inside a quoted
/// argument, or a `;` inside a trailing `#` comment. The reading has to
/// tokenize before it can recognise a boundary (review of #207).
///
/// Note the direction, which is the opposite of the `echo` test above: this
/// one invents a *hosted* build and so rejects a valid `local_only`
/// declaration -- a false failure, where the `echo` hole was a false pass.
#[test]
fn a_separator_inside_a_quoted_argument_is_not_a_command_boundary() {
    // the reviewer's case: a hosted step whose only command is `echo`, the
    // `;` and the path both inside its one argument
    let t = qemu_tree();
    let quoted = format!("{}      - name: Say what hosted CI does not build\n        run: echo \"local only; ./examples/rust-qemu/build.sh\"\n", t.read(CI_YML).unwrap());
    let f = check(&qemu_matrix(DECLARED, QEMU_ROW), &t.clone().with(CI_YML, &quoted));
    assert_eq!(f, Vec::<String>::new(), "a `;` inside a quoted argument was read as a command boundary");

    // single quotes and a backslash-escaped separator say the same thing
    let sq = format!("{}      - name: Single quotes\n        run: echo 'local only; ./examples/rust-qemu/build.sh'\n", t.read(CI_YML).unwrap());
    assert_eq!(check(&qemu_matrix(DECLARED, QEMU_ROW), &t.clone().with(CI_YML, &sq)), Vec::<String>::new());
    let esc = format!("{}      - name: Escaped\n        run: echo local\\ only\\; ./examples/rust-qemu/build.sh\n", t.read(CI_YML).unwrap());
    assert_eq!(check(&qemu_matrix(DECLARED, QEMU_ROW), &t.clone().with(CI_YML, &esc)), Vec::<String>::new());

    // a trailing `#` comment is not a command either, separators and all
    let cmt = format!("{}      - name: Build with a note\n        run: ./examples/c/build.sh  # see; ./examples/rust-qemu/build.sh is local only\n", t.read(CI_YML).unwrap());
    assert_eq!(check(&qemu_matrix(DECLARED, QEMU_ROW), &t.clone().with(CI_YML, &cmt)), Vec::<String>::new(), "a `;` inside a trailing comment was read as a command boundary");

    // the same three in the local script, where a build goes through `step`
    let sh = r#"
step "./examples/c/build.sh"  ./examples/c/build.sh   # and; ./examples/ghost-comment/build.sh
step "quoted"                 echo "a; ./examples/ghost-quoted/build.sh"
step "single"                 echo 'a; ./examples/ghost-single/build.sh'
step "escaped"                echo a\; ./examples/ghost-escaped/build.sh
step "real pair"              ./examples/c-lib/build.sh; env CC=gcc ./examples/c-gcc/build.sh
"#;
    assert_eq!(built_by(sh, false).examples.into_iter().collect::<Vec<_>>(), ["c", "c-gcc", "c-lib"]);
}

/// The other half of the same answer. Tokenizing draws a line the old split
/// did not have: some shell is outside it. The repo's rule is that a thing
/// the tool cannot determine is reported and never guessed, so a shape this
/// reader does not model is a finding naming the line, rather than a build
/// list quietly one example short or one example long (review of #207).
#[test]
fn a_shell_shape_the_reader_will_not_interpret_is_reported_not_guessed() {
    let unreadable = |sh: &str| built_by(&format!("step \"x\" {sh}\n"), false).unreadable;
    // a quote nobody closed: the rest of the script is inside it or not,
    // and which of those it is decides whether anything below is a build
    assert!(unreadable("echo \"unclosed").first().is_some_and(|w| w.contains("unterminated `\"`")), "{:?}", unreadable("echo \"unclosed"));
    assert!(unreadable("echo 'unclosed").first().is_some_and(|w| w.contains("unterminated `'`")));
    assert!(unreadable("echo $(unclosed").first().is_some_and(|w| w.contains("unterminated `$(`")));
    // a substitution runs its contents; this reader does not read them, and
    // one that names a build script is where that silence would matter
    assert!(unreadable("$(./examples/c/build.sh)").first().is_some_and(|w| w.contains("command substitution naming a build script")));
    // the ones it does model are not reported -- and a value substitution in
    // an argument is a value, not a build
    for ok in ["./examples/c/build.sh", "env CC=\"$CC\" ./examples/c-gcc/build.sh", "echo \"host $(uname -s)\"", "printf '%s\\n' \"a; b\"", "cd x && ./examples/c/build.sh"] {
        assert_eq!(unreadable(ok), Vec::<String>::new(), "{ok}");
    }
    // a word carrying a substitution is not a path, however it ends up spelt
    assert_eq!(built_by("step \"x\" \"$(dirname x)./examples/ghost/build.sh\"\n", false).examples, BTreeSet::new());

    // and a reported line is a finding on the tree, not a silent omission
    let t = qemu_tree();
    let bad = format!("{}      - name: Unreadable\n        run: echo 'unclosed\n", t.read(CI_YML).unwrap());
    let f = check(&qemu_matrix(DECLARED, QEMU_ROW), &t.with(CI_YML, &bad));
    assert!(f.iter().any(|m| m.starts_with(CI_YML) && m.contains("does not interpret") && m.contains("unclosed")), "{f:#?}");
}

/// A here-document's body is data the command reads, not commands the shell
/// runs. The tokenizer treated every newline as a command boundary even after
/// a `<<` redirection, so every line of a `cat <<'NOTE'` block was read as a
/// command of its own -- and a body line holding a path to a build script
/// became a build. In a hosted `run:` block that invents a *hosted* build of a
/// declared local-only example and so rejects a valid declaration; in the
/// local script a body line spelt like a `step` invents a local one (review of
/// #207).
///
/// So the body is consumed, and where it cannot be -- a delimiter that never
/// arrives, a `<<<` here-string, a live `$(...)` in an unquoted body -- the
/// line is reported and nothing is counted from it. Both halves are asserted
/// here: that the body yields no build, and that an unreadable shape yields a
/// finding rather than a build.
#[test]
fn a_here_document_body_is_data_not_a_list_of_commands() {
    let t = qemu_tree();
    let base = t.read(CI_YML).unwrap();
    let block = |body: &str| format!("{base}      - name: Say what hosted CI does not build\n        run: |\n{body}");

    // the reviewer's case: `cat` prints the path, hosted CI never runs it
    let quoted = block("          cat <<'NOTE'\n          ./examples/rust-qemu/build.sh\n          NOTE\n");
    assert_eq!(built_by(&quoted, true).examples.into_iter().collect::<Vec<_>>(), ["c"], "a here-document body was read as commands");
    assert_eq!(check(&qemu_matrix(DECLARED, QEMU_ROW), &t.clone().with(CI_YML, &quoted)), Vec::<String>::new(), "a here-document body invented a hosted build");

    // an unquoted delimiter reads the same here; so does `<<-`, whose body and
    // terminator may be indented with tabs
    let bare = block("          cat <<NOTE\n          ./examples/rust-qemu/build.sh\n          NOTE\n");
    assert_eq!(built_by(&bare, true).examples.into_iter().collect::<Vec<_>>(), ["c"]);
    let dash = block("          cat <<-'NOTE'\n          \t./examples/rust-qemu/build.sh\n          \tNOTE\n");
    assert_eq!(built_by(&dash, true).examples.into_iter().collect::<Vec<_>>(), ["c"]);

    // the body ends where the delimiter says, and a real build below it still
    // counts: consuming the body must not swallow the rest of the step
    let after = block("          cat <<'NOTE'\n          ./examples/rust-qemu/build.sh\n          NOTE\n          ./examples/c-lib/build.sh\n");
    assert_eq!(built_by(&after, true).examples.into_iter().collect::<Vec<_>>(), ["c", "c-lib"]);

    // the local script, where a build goes through `step`: a body line spelt
    // like one is still body -- whether the here-document was opened by a
    // `step` or by a plain command
    let sh = "step \"note\" cat <<'EOF'\nstep \"fake\" ./examples/ghost-heredoc/build.sh\nEOF\nstep \"real\" ./examples/c/build.sh\n";
    let b = built_by(sh, false);
    assert_eq!((b.examples.into_iter().collect::<Vec<_>>(), b.unreadable), (vec!["c".to_string()], vec![]));
    let plain = "cat <<'EOF'\nstep \"fake\" ./examples/ghost-heredoc/build.sh\nEOF\nstep \"real\" ./examples/c/build.sh\n";
    let b = built_by(plain, false);
    assert_eq!((b.examples.into_iter().collect::<Vec<_>>(), b.unreadable), (vec!["c".to_string()], vec![]));

    // a delimiter that never arrives: the rest of the text is body, and that
    // is said rather than assumed either way
    let open = "step \"note\" cat <<'EOF'\nstep \"fake\" ./examples/ghost-heredoc/build.sh\n";
    let b = built_by(open, false);
    assert_eq!(b.examples, BTreeSet::new());
    assert!(b.unreadable.first().is_some_and(|w| w.contains("`EOF`") && w.contains("never arrives")), "{:?}", b.unreadable);
    let b = built_by(&block("          cat <<'NOTE'\n          ./examples/rust-qemu/build.sh\n"), true);
    assert_eq!(b.examples.into_iter().collect::<Vec<_>>(), ["c"]);
    assert!(b.unreadable.first().is_some_and(|w| w.contains("`NOTE`") && w.contains("never arrives")), "{:?}", b.unreadable);

    // a `<<<` here-string is a shape this reader does not model at all
    let b = built_by("step \"x\" cat <<< ./examples/ghost-string/build.sh\n", false);
    assert_eq!(b.examples, BTreeSet::new());
    assert!(b.unreadable.first().is_some_and(|w| w.contains("here-string")), "{:?}", b.unreadable);

    // an unquoted body is still expanded, so a `$(...)` in it runs: refused
    // when it names a build script, exactly as one outside a here-document is
    let sub = "step \"note\" cat <<EOF\n$(./examples/ghost-sub/build.sh)\nEOF\n";
    let b = built_by(sub, false);
    assert_eq!(b.examples, BTreeSet::new());
    assert!(b.unreadable.first().is_some_and(|w| w.contains("command substitution naming a build script")), "{:?}", b.unreadable);
    // ... and a quoted delimiter makes the same body literal text
    assert_eq!(built_by(&sub.replace("<<EOF", "<<'EOF'"), false).unreadable, Vec::<String>::new());

    // finally, the finding reaches the tree: an unreadable here-document in
    // the real shape fails the check rather than passing it quietly
    let f = check(&qemu_matrix(DECLARED, QEMU_ROW), &t.with(CI_YML, &block("          cat <<'NOTE'\n          ./examples/rust-qemu/build.sh\n")));
    assert!(f.iter().any(|m| m.starts_with(CI_YML) && m.contains("does not interpret") && m.contains("never arrives")), "{f:#?}");
}

// ── rule 2, the local-only policy (#164's fixture needs QEMU; hosted CI is billed)

/// `small_tree` plus the configuration #208 lands: `examples/rust-qemu`,
/// built by `ci-local.sh` and not by `ci.yml`, read by its own test.
fn qemu_tree() -> Mem {
    let t = small_tree();
    let local = format!("{}step \"./examples/rust-qemu/build.sh\" ./examples/rust-qemu/build.sh\n", t.read(CI_LOCAL).unwrap());
    t.with(CI_LOCAL, &local)
        .with("examples/rust-qemu/build.sh", "qemu-system-aarch64 -d in_asm,exec,nochain\n")
        .with("examples/rust-qemu/src/lib.rs", "#![no_std]\npub fn campaign() -> u32 { 0 }\n")
        .with("crates/ingest/tests/e2e_rust_qemu.rs", "#[test]\nfn the_trace_is_ingested() {\n    ingest(\"../../examples/rust-qemu/build\");\n}\n")
}

const DECLARED: &str = r#"
[[local_only]]
example = "rust-qemu"
reason = "needs QEMU, a nightly and a bare-metal target; hosted minutes are billed"
tracking = [147, 164]
"#;

const QEMU_ROW: &str = r#"
[[row]]
id = "feature.measured-coverage"
area = "feature"
group = "Coverage"
name = "measured coverage from a trace"
state = "caveats"
example = "rust-qemu"
evidence = "examples/rust-qemu/build.sh:1"
pattern = 'in_asm,exec'
proof = "crates/ingest/tests/e2e_rust_qemu.rs::the_trace_is_ingested"
caveat = "Verified locally, not in hosted CI: the fixture needs QEMU."
"#;

/// A matrix with the `GOOD` row, a declaration (or none) and a row on the
/// local-only example (or none).
fn qemu_matrix(decl: &str, row: &str) -> String {
    matrix(&format!("{decl}{GOOD}{row}"))
}

#[test]
fn rule_2_the_agreed_local_only_configuration_passes() {
    assert_eq!(check(&qemu_matrix(DECLARED, QEMU_ROW), &qemu_tree()), Vec::<String>::new());
    // the phrase is read without regard to case, like the other two
    assert_eq!(check(&qemu_matrix(DECLARED, &QEMU_ROW.replace("Verified locally", "verified locally")), &qemu_tree()), Vec::<String>::new());
}

#[test]
fn rule_2_a_local_only_example_must_be_declared() {
    // built locally, not hosted, not declared: the lists simply disagree.
    // The row's own finding (its caveat claims a declaration that is not
    // there) is correct too.
    let f = check(&qemu_matrix("", QEMU_ROW), &qemu_tree());
    assert_eq!(f.len(), 2, "{f:#?}");
    assert!(
        f.iter().any(|m| m.contains("`examples/rust-qemu/build.sh` is run by scripts/ci-local.sh but not by .github/workflows/ci.yml") && m.contains("does not declare it")),
        "{f:#?}"
    );
    assert!(f.iter().any(|m| m.contains("`feature.measured-coverage`") && m.contains("is not declared [[local_only]]")), "{f:#?}");

    // and "not built in CI" is no way round it: that phrase is for an
    // example neither script builds, and the lists still disagree
    let f = check(&qemu_matrix("", &QEMU_ROW.replace("Verified locally, not in hosted CI", "Not built in CI")), &qemu_tree());
    assert_eq!(f.len(), 1, "{f:#?}");
    assert!(f[0].contains("does not declare it"), "{f:#?}");

    // a declaration needs its reason -- and until it has one the declaration
    // does not hold, so it licenses no row either (review of #207: the row was
    // being licensed by a declaration the check had already rejected)
    let bare = DECLARED.replace("reason = \"needs QEMU, a nightly and a bare-metal target; hosted minutes are billed\"\n", "");
    let f = check(&qemu_matrix(&bare, QEMU_ROW), &qemu_tree());
    assert_eq!(f.len(), 2, "{f:#?}");
    assert!(f.iter().any(|m| m.contains("local_only `rust-qemu`") && m.contains("no reason is written")), "{f:#?}");
    assert!(f.iter().any(|m| m.contains("`feature.measured-coverage`") && m.contains("that declaration does not hold")), "{f:#?}");
    let f = check(&qemu_matrix(&format!("{DECLARED}{DECLARED}"), QEMU_ROW), &qemu_tree());
    assert!(f.len() == 1 && f[0].contains("local_only `rust-qemu`") && f[0].contains("declared twice"), "{f:#?}");
}

#[test]
fn rule_2_a_local_only_declaration_cannot_outlive_the_gap() {
    // hosted CI starts building it: the declaration is what fails
    let t = qemu_tree();
    let hosted = format!("{}      - run: ./examples/rust-qemu/build.sh\n", t.read(CI_YML).unwrap());
    let f = check(&qemu_matrix(DECLARED, QEMU_ROW), &t.with(CI_YML, &hosted));
    assert_eq!(f.len(), 2, "{f:#?}");
    assert!(f.iter().any(|m| m.contains("local_only `rust-qemu`") && m.contains("the declaration is out of date")), "{f:#?}");
    // and the row that rested on it is named, so the reader knows what else
    // has to change
    assert!(f.iter().any(|m| m.contains("`feature.measured-coverage`") && m.contains("that declaration does not hold")), "{f:#?}");

    // the declaration goes, and then the phrase is what fails
    let t = qemu_tree().with(CI_YML, &hosted);
    only_finding(&format!("{GOOD}{QEMU_ROW}"), &t, "feature.measured-coverage", "is not declared [[local_only]]");

    // a declaration for an example the local script does not build either
    // is not "local-only", it is unbuilt
    let f = check(&qemu_matrix(DECLARED, QEMU_ROW), &qemu_tree().with(CI_LOCAL, &small_tree().read(CI_LOCAL).unwrap()));
    assert_eq!(f.len(), 2, "{f:#?}");
    assert!(f.iter().any(|m| m.contains("local_only `rust-qemu`") && m.contains("is not run by scripts/ci-local.sh")), "{f:#?}");
    assert!(f.iter().any(|m| m.contains("`feature.measured-coverage`") && m.contains("that declaration does not hold")), "{f:#?}");
    // ... nor for one that is not there
    let f = check(&matrix(&format!("{DECLARED}{GOOD}")), &small_tree());
    assert!(f.iter().any(|m| m.contains("local_only `rust-qemu`") && m.contains("`examples/rust-qemu` does not exist")), "{f:#?}");

    // there is no hosted-only exception to declare
    let t = small_tree().with(CI_YML, &hosted).with("examples/rust-qemu/build.sh", "");
    let f = check(&qemu_matrix(DECLARED, ""), &t);
    assert!(f.iter().any(|m| m.contains("is run by .github/workflows/ci.yml but not by scripts/ci-local.sh")), "{f:#?}");
}

#[test]
fn rule_2_a_row_on_a_local_only_example_is_never_supported() {
    let t = qemu_tree();
    // even with evidence and a proof that reads the example
    let sup = QEMU_ROW.replace("\"caveats\"", "\"supported\"").replace("caveat = \"Verified locally, not in hosted CI: the fixture needs QEMU.\"\n", "");
    let f: Vec<String> = check(&qemu_matrix(DECLARED, &sup), &t);
    assert!(f.len() == 1 && f[0].contains("`feature.measured-coverage`") && f[0].contains("cannot be `supported`") && f[0].contains("[[local_only]]"), "{f:#?}");

    // `caveats`, but silent about where it was verified
    let quiet = QEMU_ROW.replace("Verified locally, not in hosted CI: the fixture needs QEMU.", "Execution counts are an upper bound.");
    let f = check(&qemu_matrix(DECLARED, &quiet), &t);
    assert!(f.len() == 1 && f[0].contains("`feature.measured-coverage`") && f[0].contains("must contain the words \"verified locally, not in hosted CI\""), "{f:#?}");

    // and "not built in CI" is the wrong thing to say: ci-local builds it
    let wrong = QEMU_ROW.replace("the fixture needs QEMU.", "the example is not built in CI.");
    let f = check(&qemu_matrix(DECLARED, &wrong), &t);
    assert!(f.len() == 1 && f[0].contains("scripts/ci-local.sh does build it"), "{f:#?}");

    // a not-supported row claims nothing, so it owes no phrase
    let none = "\n[[row]]\nid = \"rust.qemu\"\narea = \"language\"\ngroup = \"Rust\"\nname = \"n\"\nstate = \"not-supported\"\nexample = \"rust-qemu\"\nreason = \"not yet\"\n";
    assert_eq!(check(&qemu_matrix(DECLARED, none), &t), Vec::<String>::new());
}

#[test]
fn rule_3_evidence_must_still_show_the_construct() {
    let t = small_tree();
    only_finding(&GOOD.replace("demo.c:3", "gone.c:3"), &t, "c.decision", "`examples/c/gone.c` does not exist");
    only_finding(&GOOD.replace("demo.c:3", "demo.c:300"), &t, "c.decision", "past the end of the file");
    only_finding(&GOOD.replace("demo.c:3", "demo.c"), &t, "c.decision", "is not `path:line`");
    only_finding(&GOOD.replace("a && b", r"a \|\| b"), &t, "c.decision", "no longer matches");
    only_finding(&GOOD.replace("'a && b'", "'('"), &t, "c.decision", "not a valid regex");
    // evidence has to be in the example the row names, not merely somewhere
    only_finding(&GOOD.replace("examples/c/demo.c:3", "crates/ingest/tests/e2e.rs:1").replace("a && b", "reads"), &t, "c.decision", "is not inside the row's example");
}

/// A construct lives in code, and a sentence about it survives deleting it.
/// `rust.prologue-frame` -- a `language` row, where evidence is mandatory --
/// cited an English sentence in a README that contains the word "prologue"
/// (self-review of #207).
#[test]
fn rule_3_a_language_or_toolchain_row_may_not_cite_prose() {
    let t = small_tree().with("examples/c/README.md", "# The C example\n\nIt has a decision: `a && b` in demo.c.\n");
    let md = GOOD.replace("examples/c/demo.c:3", "examples/c/README.md:3");
    only_finding(&md, &t, "c.decision", "is prose: a language row must point at the construct in code");
    only_finding(&md.replace("\"language\"", "\"toolchain\""), &t, "c.decision", "is prose: a toolchain row");
    // a feature or architecture row may still cite a README: it is often a
    // claim about the tool rather than about a construct in the example
    assert!(check(&matrix(&md.replace("\"language\"", "\"feature\"")), &t).is_empty());
    // and code is code whatever its extension
    assert!(check(&matrix(GOOD), &t).is_empty());
}

#[test]
fn rule_3_tolerates_a_small_drift_and_no_more() {
    // the construct is on line 3; EVIDENCE_WINDOW lines away still finds it
    let t = small_tree();
    assert!(check(&matrix(&GOOD.replace("demo.c:3", "demo.c:6")), &t).is_empty());
    assert!(check(&matrix(&GOOD.replace("demo.c:3", "demo.c:1")), &t).is_empty());
    only_finding(&GOOD.replace("demo.c:3", "demo.c:7"), &t, "c.decision", "within 3 lines of line 7");
}

#[test]
fn rule_4_the_proof_must_exist_and_still_hold_the_test() {
    let t = small_tree();
    only_finding(&GOOD.replace("e2e.rs", "gone.rs"), &t, "c.decision", "proof file `crates/ingest/tests/gone.rs` does not exist");
    only_finding(&GOOD.replace("::it_works", "::it_worked"), &t, "c.decision", "no longer contains the test `it_worked`");
    // a prefix of a real name is not the test
    only_finding(&GOOD.replace("::it_works", "::it_work"), &t, "c.decision", "no longer contains the test `it_work`");
    only_finding(&GOOD.replace("proof = \"crates/ingest/tests/e2e.rs::it_works\"", ""), &t, "c.decision", "names no proof");
    // a caveats row may stand without one, but has to say so
    let bare = GOOD.replace("proof = \"crates/ingest/tests/e2e.rs::it_works\"", "").replace("\"supported\"", "\"caveats\"\ncaveat = \"approximate\"");
    only_finding(&bare, &t, "c.decision", "the caveat does not say so");
    assert!(check(&matrix(&bare.replace("approximate", "No test reads this example.")), &t).is_empty());
    // a node:test case is found by its title
    let js = GOOD.replace("\"supported\"", "\"caveats\"\ncaveat = \"proven on hand-written evidence\"");
    assert!(check(&matrix(&js.replace("crates/ingest/tests/e2e.rs::it_works", "ui/model.test.cjs::recursion terminates")), &t).is_empty());
    only_finding(&js.replace("crates/ingest/tests/e2e.rs::it_works", "ui/model.test.cjs::recursion halts"), &t, "c.decision", "no longer contains the test");
}

#[test]
fn rule_4_a_supported_rows_proof_must_read_its_example() {
    let t = small_tree();
    only_finding(&GOOD.replace("crates/ingest/tests/e2e.rs::it_works", "ui/model.test.cjs"), &t, "c.decision", "never names `examples/c`");
    // examples/c-lib is not examples/c
    assert!(mentions_example("join(\"../../examples/c/build\")", "c"));
    assert!(mentions_example("see examples/c.", "c"));
    assert!(!mentions_example("join(\"../../examples/c-lib/build\")", "c"));
    assert!(!mentions_example("examples/cpp", "c"));

    // ... and a proof that only *talks* about the example has not read it.
    // `feature.security` stood `supported` for exactly this reason: its proof
    // says in a `//!` line that it needs no fixture, and it does not open one
    // (self-review of #207).
    let prose = "//! The refusals need no fixture -- examples/c is never loaded.\n#[test]\nfn it_works() {\n    serve(d.join(\"never-read.json\"));\n}\n";
    assert!(!mentions_example(prose, "c"));
    only_finding(GOOD, &small_tree().with("crates/ingest/tests/e2e.rs", prose), "c.decision", "never names `examples/c`");
    for comment in ["// examples/c", "  /// examples/c", "//! examples/c", "# examples/c", " * examples/c", "/* examples/c */"] {
        assert!(!mentions_example(comment, "c"), "{comment:?} is a comment, not a read");
    }
    // a comment *after* code on the same line leaves the code a read
    assert!(mentions_example("    ingest(\"examples/c/build\"); // the C example", "c"));
    // and a `caveats` row owes no read at all: it says what its proof runs on
    let cav = GOOD.replace("\"supported\"", "\"caveats\"\ncaveat = \"The proof needs no fixture: it never opens the example.\"");
    assert!(check(&matrix(&cav), &small_tree().with("crates/ingest/tests/e2e.rs", prose)).is_empty());
}

#[test]
fn rule_5_an_example_that_backs_no_row_is_an_orphan() {
    let t = small_tree().with("examples/c-new/build.sh", "").with("examples/build-systems/cmake/CMakeLists.txt", "").with("examples/build-systems/README.md", "");
    let f = check(&matrix(GOOD), &t);
    assert_eq!(f.len(), 2, "{f:#?}");
    assert!(f.iter().any(|m| m.contains("`examples/c-new` backs no row")), "{f:#?}");
    // build-systems is a folder of examples: its children are what count
    assert!(f.iter().any(|m| m.contains("`examples/build-systems/cmake` backs no row")), "{f:#?}");
    // files beside the examples (run-all.sh, README.md) are not examples
    assert!(check(&matrix(GOOD), &small_tree().with("examples/README.md", "")).is_empty());
    // a not-supported row that names the example is enough to own it
    let owned = format!(
        "{GOOD}\n[[row]]\nid = \"c.new\"\narea = \"language\"\ngroup = \"C\"\nname = \"n\"\nstate = \"not-supported\"\nexample = \"c-new\"\nreason = \"nothing reads it yet\"\n"
    );
    let f = check(&matrix(&owned), &t);
    assert_eq!(f.len(), 1, "{f:#?}");
}

#[test]
fn rule_6_caveats_reasons_and_unique_ids() {
    let t = small_tree();
    only_finding(&GOOD.replace("\"supported\"", "\"caveats\""), &t, "c.decision", "no caveat is written");
    only_finding(&GOOD.replace("\"supported\"", "\"caveats\"\ncaveat = \"  \""), &t, "c.decision", "no caveat is written");
    only_finding(
        "[[row]]\nid = \"c.setjmp\"\narea = \"language\"\ngroup = \"C\"\nname = \"setjmp\"\nstate = \"not-supported\"\n",
        &Mem::default().with(CI_YML, "").with(CI_LOCAL, ""),
        "c.setjmp",
        "no reason is written",
    );
    only_finding(&format!("{GOOD}{GOOD}"), &t, "c.decision", "duplicate id");
    only_finding(&GOOD.replace("\"supported\"", "\"supported\"\ncaveat = \"approximate\""), &t, "c.decision", "a row with a limit is `caveats`");
    only_finding(&GOOD.replace("\"supported\"", "\"works\""), &t, "c.decision", "is not one of supported, caveats, not-supported");
    only_finding(&GOOD.replace("\"language\"", "\"lang\""), &t, "c.decision", "area \"lang\"");
    only_finding(&GOOD.replace("group = \"C\"\n", ""), &t, "c.decision", "has no group");
}

#[test]
fn a_feature_row_may_omit_evidence_but_a_language_row_may_not() {
    let t = small_tree();
    let bare = GOOD.replace("evidence = \"examples/c/demo.c:3\"\n", "").replace("pattern = 'a && b'\n", "");
    only_finding(&bare, &t, "c.decision", "is not `path:line`");
    assert!(check(&matrix(&bare.replace("\"language\"", "\"feature\"")), &t).is_empty());
    // given, it is checked whatever the area
    only_finding(&GOOD.replace("\"language\"", "\"feature\"").replace("a && b", "gone"), &t, "c.decision", "no longer matches");
}

#[test]
fn a_misspelt_field_is_an_error_not_a_silently_dropped_claim() {
    let f = check(&matrix(&GOOD.replace("proof =", "prof =")), &small_tree());
    assert!(f.len() == 1 && f[0].contains("does not parse") && f[0].contains("prof"), "{f:#?}");
}
