// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The variants of a C++ constructor or destructor (#266).
//!
//! The Itanium C++ ABI emits one source constructor as up to three symbols
//! and one destructor as up to three more, distinguished only by a code in
//! the mangled name:
//!
//! | code | what it is |
//! |---|---|
//! | `C1` | complete-object constructor |
//! | `C2` | base-object constructor |
//! | `C3` | complete-object allocating constructor |
//! | `CI1`/`CI2` | inheriting constructor, complete / base |
//! | `D0` | deleting destructor: destroys, then calls `operator delete` |
//! | `D1` | complete-object destructor |
//! | `D2` | base-object destructor |
//!
//! clang puts the body in the base-object variant (`C2`, `D2`) and emits the
//! others as calls to it, or as aliases of it where the object format allows
//! (ELF). Its coverage instrumentation counts the base variant only
//! (`CodeGenPGO::assignRegionCounters` skips the rest, "it would be counted
//! twice otherwise"), and llvm-cov's record names that one symbol.
//!
//! So every other variant is object code in the image with no counter of its
//! own. What source coverage does establish is one direction: each variant
//! runs the body, so if the body never ran, no variant did. The other
//! direction it cannot: the body's count includes every variant's calls and
//! every direct call of the body, and a `D0` reached only through a vtable
//! may never run while the body runs many times.

use butterfly_model::Status;

/// The variant code of `sym` when it is another variant of the constructor
/// or destructor whose coverage record names `body` -- the same mangled name
/// with the base-object code (`C2`, `CI2`, `D2`) replaced by another one.
///
/// Both names are bare: no llvm-cov `file:` qualifier, no Mach-O underscore.
/// Nothing is inferred from a demangled name: two symbols that demangle alike
/// can be different functions, but two mangled names that differ only in the
/// variant digit of a `C`/`D` code are by construction the same source
/// constructor or destructor.
pub fn of(body: &str, sym: &str) -> Option<String> {
    if body.len() != sym.len() || !body.starts_with("_Z") || body == sym {
        return None;
    }
    let (b, s) = (body.as_bytes(), sym.as_bytes());
    let mut diff = (0..b.len()).filter(|&i| b[i] != s[i]);
    let i = diff.next()?;
    if diff.next().is_some() || b[i] != b'2' {
        return None;
    }
    // The code is `C2`/`D2`, whose next byte starts what follows the name
    // (`E`, template args `I`, an ABI tag `B`) and is never another digit of
    // a length prefix; or `CI2` for an inheriting constructor, which is
    // followed by the base class it inherits from. The caller has already
    // resolved both symbols to one source function, so this only has to
    // tell a variant from the body, never one function from another.
    let code_start = match (i.checked_sub(1).map(|j| b[j]), i.checked_sub(2).map(|j| b[j])) {
        (Some(b'C' | b'D'), _) if matches!(b.get(i + 1), Some(b'E' | b'I' | b'B')) => i - 1,
        (Some(b'I'), Some(b'C')) => i - 2,
        _ => return None,
    };
    let ok = match b[code_start] {
        b'C' if code_start + 1 == i => matches!(s[i], b'1' | b'3'),
        b'C' => s[i] == b'1', // CI1
        b'D' => matches!(s[i], b'0' | b'1'),
        _ => false,
    };
    ok.then(|| String::from_utf8_lossy(&s[code_start..=i]).into_owned())
}

/// What a variant is, in the words a reviewer reads.
pub fn role(code: &str) -> &'static str {
    match code {
        "C1" => "complete-object constructor",
        "C3" => "complete-object allocating constructor",
        "CI1" => "complete-object inheriting constructor",
        "D0" => "deleting destructor",
        "D1" => "complete-object destructor",
        _ => "constructor/destructor variant",
    }
}

/// The projected status of every instruction of a variant, from the one
/// count there is: the body's. `Uncovered` when the body never ran -- every
/// variant runs it, so none of them ran either; otherwise `Unknown`, because
/// nothing in the coverage export counts this variant (#266).
pub fn projected(body_count: u64) -> Status {
    if body_count == 0 {
        Status::Uncovered
    } else {
        Status::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_of_a_constructor_and_a_destructor() {
        assert_eq!(of("_ZN2fc7ChannelC2Ei", "_ZN2fc7ChannelC1Ei").as_deref(), Some("C1"));
        assert_eq!(of("_ZN2fc7ChannelC2Ei", "_ZN2fc7ChannelC3Ei").as_deref(), Some("C3"));
        assert_eq!(of("_ZN2fc7ChannelD2Ev", "_ZN2fc7ChannelD0Ev").as_deref(), Some("D0"));
        assert_eq!(of("_ZN2fc7ChannelD2Ev", "_ZN2fc7ChannelD1Ev").as_deref(), Some("D1"));
        // inheriting, templated and ABI-tagged constructors
        assert_eq!(of("_ZN1BCI21AEi", "_ZN1BCI11AEi").as_deref(), Some("CI1"));
        assert_eq!(of("_ZN1SC2IiEET_", "_ZN1SC1IiEET_").as_deref(), Some("C1"));
        assert_eq!(of("_ZN1SC2B5cxx11Ev", "_ZN1SC1B5cxx11Ev").as_deref(), Some("C1"));
    }

    #[test]
    fn anything_else_is_not_a_variant() {
        // the body itself, and a variant of a different body
        assert_eq!(of("_ZN2fc7ChannelD2Ev", "_ZN2fc7ChannelD2Ev"), None);
        assert_eq!(of("_ZN2fc7ChannelC1Ei", "_ZN2fc7ChannelC2Ei"), None);
        // a destructor is not a variant of a constructor
        assert_eq!(of("_ZN2fc7ChannelC2Ei", "_ZN2fc7ChannelD2Ei"), None);
        // a digit that is not a C/D code: a length prefix, a type
        assert_eq!(of("_ZN2fc3AB2Ev", "_ZN2fc3AB1Ev"), None);
        assert_eq!(of("_Z1fILi2EEvv", "_Z1fILi1EEvv"), None);
        // `D2` inside an identifier, followed by more of it
        assert_eq!(of("_ZN4XD2x1fEv", "_ZN4XD1x1fEv"), None);
        // no invalid variant digits
        assert_eq!(of("_ZN1SC2Ev", "_ZN1SC4Ev"), None);
        assert_eq!(of("_ZN1SD2Ev", "_ZN1SD3Ev"), None);
        // not C++, and names that differ in more than the code
        assert_eq!(of("f2", "f1"), None);
        assert_eq!(of("_ZN1SC2Ev", "_ZN1TC1Ev"), None);
        assert_eq!(of("_ZN1SC2Ev", "_ZN1SC1Evv"), None);
    }

    #[test]
    fn a_variant_is_uncovered_only_when_its_body_never_ran() {
        assert_eq!(projected(0), Status::Uncovered);
        assert_eq!(projected(1), Status::Unknown);
        assert_eq!(projected(u64::MAX), Status::Unknown);
    }

    #[test]
    fn roles() {
        assert_eq!(role("D0"), "deleting destructor");
        assert_eq!(role("C1"), "complete-object constructor");
    }
}
