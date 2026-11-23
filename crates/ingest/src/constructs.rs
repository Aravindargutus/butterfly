// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Source-construct inventory for CAST-12 style source-to-object traceability
//! analysis: which language constructs each function uses, so the reviewed
//! sample can be shown to cover every construct the code base relies on.
//!
//! Heuristic, token-level; comments and string literals are stripped first.
//! Being a lexical heuristic, it counts what is unambiguous as a token. It
//! deliberately does **not** try to count constructs that a token stream
//! cannot tell apart from something else: a C++ reference declarator (`int&`)
//! is the same `&` as address-of and bitwise-and, and a constructor call is
//! the same `Name(` as any other call, so neither is reported rather than
//! reported wrongly. The language is chosen from the file extension (`Lang`);
//! a `.h` header defaults to the C vocabulary because its extension does not
//! say which language it is.

use std::collections::BTreeMap;

/// The vocabulary to inventory a source file against, selected from its
/// extension. A `.h` header is treated as C: the extension does not
/// distinguish a C header from a C++ one, and C is the safe default.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Rust,
    Cpp,
    C,
}

impl Lang {
    /// Choose a vocabulary from a source path's extension.
    pub fn from_path(path: &str) -> Lang {
        let ext = path.rsplit('.').next().unwrap_or("");
        // `.C` (capital) is a traditional C++ extension; match it before
        // lowercasing so it is not folded onto `.c`.
        if ext == "C" {
            return Lang::Cpp;
        }
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Lang::Rust,
            // The C++ source and header extensions clang accepts.
            "cpp" | "cc" | "cxx" | "c++" | "cp" | "hpp" | "hh" | "hxx" | "h++" | "ipp" | "tpp" => Lang::Cpp,
            // `.c`, `.h`, `.i` and anything unrecognised fall back to C.
            _ => Lang::C,
        }
    }
}

fn strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let b: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut in_block = false;
    while i < b.len() {
        if in_block {
            if b[i] == '*' && i + 1 < b.len() && b[i + 1] == '/' {
                in_block = false;
                i += 2;
                continue;
            }
            if b[i] == '\n' {
                out.push('\n');
            }
            i += 1;
            continue;
        }
        if b[i] == '/' && i + 1 < b.len() && b[i + 1] == '*' {
            in_block = true;
            i += 2;
            continue;
        }
        if b[i] == '/' && i + 1 < b.len() && b[i + 1] == '/' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == '"' || (b[i] == '\'' && i + 2 < b.len() && (b[i + 2] == '\'' || (b[i + 1] == '\\' && i + 3 < b.len() && b[i + 3] == '\''))) {
            let q = b[i];
            out.push(q);
            i += 1;
            while i < b.len() && b[i] != q {
                if b[i] == '\\' {
                    i += 1;
                }
                if i < b.len() && b[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            out.push(q);
            i += 1;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn count_word(s: &str, w: &str) -> u32 {
    let mut n = 0;
    let bytes = s.as_bytes();
    let mut from = 0;
    while let Some(pos) = s[from..].find(w) {
        let a = from + pos;
        let e = a + w.len();
        let before_ok = a == 0 || !(bytes[a - 1].is_ascii_alphanumeric() || bytes[a - 1] == b'_');
        let after_ok = e >= bytes.len() || !(bytes[e].is_ascii_alphanumeric() || bytes[e] == b'_');
        if before_ok && after_ok {
            n += 1;
        }
        from = e;
    }
    n
}

fn count_str(s: &str, w: &str) -> u32 {
    s.matches(w).count() as u32
}

fn put(m: &mut BTreeMap<String, u32>, k: &str, v: u32) {
    if v > 0 {
        m.insert(k.to_string(), v);
    }
}

/// The `||` in `text` that are logical ORs rather than closures with no
/// parameters (`move || x`, `run(|| 3)`). An OR follows an operand -- a
/// name, a literal, or a closing bracket -- and a closure does not; `move`
/// and `return` are names that are not operands (#167).
fn rust_logical_ors(text: &str) -> u32 {
    let mut n = 0;
    let mut from = 0;
    while let Some(pos) = text[from..].find("||") {
        let at = from + pos;
        let before = text[..at].trim_end();
        let word: String = before.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<Vec<_>>().into_iter().rev().collect();
        let operand = match before.chars().last() {
            Some(c) if c.is_alphanumeric() || c == '_' => word != "move" && word != "return",
            Some(')' | ']' | '}' | '\'' | '"') => true,
            _ => false,
        };
        if operand {
            n += 1;
        }
        from = at + 2;
    }
    n
}

/// The Rust vocabulary.
fn rust_vocab(m: &mut BTreeMap<String, u32>, text: &str) {
    put(m, "if", count_word(text, "if"));
    put(m, "else", count_word(text, "else"));
    put(m, "match", count_word(text, "match"));
    put(m, "loop", count_word(text, "loop") + count_word(text, "while") + count_word(text, "for"));
    put(m, "&&", count_str(text, "&&"));
    let ors = rust_logical_ors(text);
    put(m, "||", ors);
    put(m, "?-operator", text.matches(")?").count() as u32 + text.matches("?;").count() as u32 + text.matches("?.").count() as u32);
    // Pipes come in pairs, and a logical `||` is a pair that is not a
    // closure. A `||` that is not an OR is a closure with no parameters, and
    // its pair of pipes is already one closure in the division. Cannot
    // underflow: every OR counted consumes two of the pipes divided.
    put(m, "closure", text.matches('|').count() as u32 / 2 - ors);
    put(m, "division", count_str(text, " / ") + count_str(text, " % "));
    put(m, "float", count_word(text, "f32") + count_word(text, "f64"));
    put(m, "index", count_str(text, "["));
    put(m, "deref", count_str(text, "*"));
    put(m, "as-cast", count_word(text, "as"));
    put(m, "unsafe", count_word(text, "unsafe"));
    put(m, "return", count_word(text, "return"));
    put(m, "call", text.matches("(").count() as u32);
    put(m, "shift/bitwise", count_str(text, "<<") + count_str(text, ">>") + count_str(text, " & ") + count_str(text, " | ") + count_str(text, " ^ "));
    put(m, "checked-arith", count_str(text, "checked_") + count_str(text, "wrapping_") + count_str(text, "saturating_"));
    put(m, "panic/unwrap", count_word(text, "unwrap") + count_word(text, "expect") + count_word(text, "panic") + count_word(text, "assert"));
}

/// The C vocabulary. C++ shares it (a C++ function uses the same conditionals,
/// loops, arithmetic and pointer constructs) and adds `cpp_extra` on top.
fn c_base(m: &mut BTreeMap<String, u32>, text: &str) {
    put(m, "if", count_word(text, "if"));
    put(m, "else", count_word(text, "else"));
    put(m, "switch", count_word(text, "switch"));
    put(m, "case", count_word(text, "case") + count_word(text, "default"));
    put(m, "for", count_word(text, "for"));
    put(m, "while", count_word(text, "while") + count_word(text, "do"));
    put(m, "&&", count_str(text, "&&"));
    put(m, "||", count_str(text, "||"));
    put(m, "ternary", text.matches(" ? ").count() as u32);
    put(m, "goto", count_word(text, "goto"));
    put(m, "division", count_str(text, " / ") + count_str(text, " % ") + count_str(text, "/=") + count_str(text, "%="));
    put(m, "float", count_word(text, "float") + count_word(text, "double"));
    put(m, "int64", count_word(text, "int64_t") + count_word(text, "uint64_t") + count_word(text, "long"));
    put(m, "index", count_str(text, "["));
    put(m, "pointer", count_str(text, "->") + count_str(text, "*"));
    put(m, "cast", text.matches(")(").count() as u32 + text.matches("_t)").count() as u32);
    put(m, "return", count_word(text, "return"));
    put(m, "call", text.matches("(").count() as u32);
    put(
        m,
        "shift/bitwise",
        count_str(text, "<<")
            + count_str(text, ">>")
            + count_str(text, " & ")
            + count_str(text, " | ")
            + count_str(text, " ^ ")
            + count_str(text, "&=")
            + count_str(text, "|=")
            + count_str(text, "^="),
    );
    put(m, "sizeof", count_word(text, "sizeof"));
    put(m, "struct-copy", count_str(text, "= *"));
    put(m, "volatile", count_word(text, "volatile"));
}

/// The C++ constructs that a C reviewer's vocabulary misses. Every entry is a
/// keyword or fixed token that survives the strip pass unambiguously; the
/// object code of these is exactly what a C++ reviewer needs the reviewed
/// sample to exercise (virtual dispatch, template instantiation, exceptions,
/// heap allocation). Reference declarators and constructor calls are omitted
/// on purpose — see the module note; a token stream cannot separate them from
/// address-of / bitwise-and and ordinary calls.
fn cpp_extra(m: &mut BTreeMap<String, u32>, text: &str) {
    put(m, "template", count_word(text, "template"));
    // A method marked `override` is a virtual method: its object code is a
    // virtual function reached through the vtable, which is exactly what the
    // reviewer wants the sample to exercise. The `virtual` keyword itself sits
    // on the base-class declaration (no body, no object code), so counting only
    // it would report virtual dispatch nowhere it actually runs.
    put(m, "virtual", count_word(text, "virtual") + count_word(text, "override"));
    put(m, "class", count_word(text, "class"));
    put(m, "namespace", count_word(text, "namespace"));
    put(m, "new", count_word(text, "new"));
    put(m, "delete", count_word(text, "delete"));
    put(m, "throw", count_word(text, "throw"));
    put(m, "try/catch", count_word(text, "try") + count_word(text, "catch"));
    put(m, "operator", count_word(text, "operator"));
    // The four named C++ casts, each a real object-code operation (a
    // `dynamic_cast` is a runtime call, a `reinterpret_cast` is none). Kept
    // separate from the C `cast` heuristic above.
    put(m, "cpp-cast", count_word(text, "static_cast") + count_word(text, "dynamic_cast") + count_word(text, "reinterpret_cast") + count_word(text, "const_cast"));
    // Scope resolution: a strong signal of namespace / class-qualified use.
    put(m, "scope-res", count_str(text, "::"));
}

/// Inventory the constructs in `lines` (the function body) against `lang`'s
/// vocabulary.
pub fn inventory(lines: &[String], lang: Lang) -> BTreeMap<String, u32> {
    let text = strip(&lines.join("\n"));
    let mut m = BTreeMap::new();
    match lang {
        Lang::Rust => rust_vocab(&mut m, &text),
        Lang::C => c_base(&mut m, &text),
        Lang::Cpp => {
            c_base(&mut m, &text);
            cpp_extra(&mut m, &text);
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_from_path() {
        assert_eq!(Lang::from_path("src/main.rs"), Lang::Rust);
        assert_eq!(Lang::from_path("flight.cpp"), Lang::Cpp);
        assert_eq!(Lang::from_path("include/flight/sched.hpp"), Lang::Cpp);
        assert_eq!(Lang::from_path("legacy.C"), Lang::Cpp);
        assert_eq!(Lang::from_path("demo.c"), Lang::C);
        // A `.h` header is ambiguous; it defaults to C.
        assert_eq!(Lang::from_path("api.h"), Lang::C);
        assert_eq!(Lang::from_path("Makefile"), Lang::C);
    }

    #[test]
    fn c_constructs() {
        let src: Vec<String> = ["int f(int a, int b) { /* if */ if ((a && b) || a > 2) { return a / b; } else { return a ? 1 : 0; } }"].iter().map(|s| s.to_string()).collect();
        let m = inventory(&src, Lang::C);
        assert_eq!(m["if"], 1);
        assert_eq!(m["&&"], 1);
        assert_eq!(m["||"], 1);
        assert_eq!(m["division"], 1);
        assert_eq!(m["ternary"], 1);
        assert_eq!(m["else"], 1);
        // The C vocabulary knows nothing of these.
        assert!(!m.contains_key("template"));
        assert!(!m.contains_key("virtual"));
    }

    #[test]
    fn cpp_constructs() {
        // A C++ fragment exercising the constructs a C vocabulary misses.
        let src: Vec<String> = [
            "namespace flight {",
            "template <typename T> struct Limiter { virtual int32_t apply(int32_t v) const = 0; };",
            "struct HardLimit final : Limiter { int32_t apply(int32_t v) const override { return v; } };",
            "int32_t use() {",
            "  Limiter* p = new HardLimit();",
            "  int32_t r = static_cast<int32_t>(flight::detail::apply(p, 3));",
            "  delete p;",
            "  if (r > 0) { return r; } else { return 0; }",
            "}",
            "}",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let m = inventory(&src, Lang::Cpp);
        // The hard requirement of #162: virtual and template are now listed.
        assert_eq!(m["template"], 1);
        // `virtual` counts the base declaration and the `override` method both.
        assert_eq!(m["virtual"], 2);
        // The rest of the C++ vocabulary.
        assert_eq!(m["namespace"], 1);
        assert_eq!(m["new"], 1);
        assert_eq!(m["delete"], 1);
        assert_eq!(m["cpp-cast"], 1);
        assert_eq!(m["scope-res"], 2); // flight::detail::apply
                                       // The shared C base still fires inside a C++ function.
        assert_eq!(m["if"], 1);
        assert_eq!(m["else"], 1);
        // An honest heuristic does not invent a reference or constructor count.
        assert!(!m.contains_key("reference"));
    }

    /// A closure with no parameters is written `||`, the same two characters
    /// as a logical OR. Counted as an OR, `move || owned.len()` was reported
    /// as a decision operator and not as a closure (#167). What precedes the
    /// `||` tells them apart: an OR follows an operand, a closure does not.
    #[test]
    fn a_closure_with_no_parameters_is_not_a_logical_or() {
        let src: Vec<String> = [
            "fn f(v: Vec<i32>, a: bool, b: bool) -> i32 {",
            "    let n = once(move || v.len());",
            "    let k = run(|| 3);",
            "    let m = each(&v, |x| a || x > 0);",
            "    if a || b { n + k } else { m }",
            "}",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let m = inventory(&src, Lang::Rust);
        assert_eq!(m.get("closure"), Some(&3), "move ||, || and |x|: {m:?}");
        assert_eq!(m.get("||"), Some(&2), "`a || x > 0` and `a || b` are the two ORs: {m:?}");
    }

    #[test]
    fn rust_still_isolated_from_cpp_vocab() {
        let src: Vec<String> = ["fn f(x: i32) -> i32 { if x > 0 { x } else { 0 } }"].iter().map(|s| s.to_string()).collect();
        let m = inventory(&src, Lang::Rust);
        assert_eq!(m["if"], 1);
        assert!(!m.contains_key("template"));
        assert!(!m.contains_key("virtual"));
    }
}
