// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Analysis baseline (DO-178C 4.4.2.c): a frozen fingerprint of the compiler,
//! its options and the code, so a later ingest can say whether earlier
//! coverage and source-to-object analyses are still valid.

use butterfly_model::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Canonicalise one instruction's operands for the fingerprint: load
/// addresses removed, so the same code compares equal wherever it was
/// linked, while everything semantic survives.
///
/// The default is to keep every number exactly as objdump printed it. An
/// immediate is part of what the code does, and treating one as an address
/// makes two different constants fingerprint alike -- which is how AArch64's
/// signed `#-0x1` and RISC-V's bare `lui a0, 0xdeadc` were being lost
/// (review of #12). A value is rewritten only when something positively
/// identifies it as an address:
///
///   * it lands inside this function, so it is a reference to its own code:
///     it becomes an offset from the entry, which is stable under
///     relocation and still tells one target from another; or
///   * objdump printed a `<symbol>` annotation beside it -- it does that for
///     addresses and nothing else. The raw value becomes a placeholder
///     because the annotation carries the identity and is hashed with it, so
///     `bl 0x2000 <alpha>` and `bl 0x2000 <beta>` differ while the same
///     callee relocated elsewhere does not.
///
/// An external target objdump could not name keeps its raw value: nothing
/// else distinguishes it, and for a baseline a spurious "changed" is safe
/// where a missed change is not.
///
/// The rule needs no architecture: it reads the operand's own shape, so a
/// bare hexadecimal immediate is kept on every target that writes one.
fn normalize_operands(ops: &str, base: u64, size: u64) -> String {
    let b = ops.as_bytes();
    let mut out = String::with_capacity(ops.len());
    let mut i = 0;
    while i < b.len() {
        if !(b[i] == b'0' && i + 1 < b.len() && (b[i + 1] | 32) == b'x') {
            out.push(b[i] as char);
            i += 1;
            continue;
        }
        // `#0x1`, `$0x1`, `#-0x1`: an immediate, whatever the architecture
        let prev = ops[..i].trim_end_matches(['-', '+']);
        let is_immediate = prev.ends_with('#') || prev.ends_with('$');
        let mut j = i + 2;
        while j < b.len() && (b[j] as char).is_ascii_hexdigit() {
            j += 1;
        }
        let annotated = ops[j..].trim_start().starts_with('<');
        match u64::from_str_radix(&ops[i + 2..j], 16) {
            Ok(v) if !is_immediate && v >= base && v < base.saturating_add(size) => out.push_str(&format!("@+{:x}", v - base)),
            Ok(_) if !is_immediate && annotated => out.push_str("@ext"),
            _ => out.push_str(&ops[i..j]),
        }
        i = j;
    }
    out
}

/// SHA-256 over a function's normalised object code and control flow.
///
/// Instruction and block counts are not a fingerprint: an opcode, an
/// immediate or a branch target can change while both stay equal, and the
/// baseline would report the object code unchanged.
fn object_digest(o: &ObjectFn) -> String {
    let mut h = Sha256::new();
    for (bi, b) in o.blocks.iter().enumerate() {
        h.update(format!("b{bi}\n").as_bytes());
        for ins in &b.instrs {
            h.update(ins.mnemonic.as_bytes());
            h.update(b"\t");
            h.update(normalize_operands(&ins.operands, o.address, o.size).as_bytes());
            h.update(b"\n");
        }
        for e in &b.succs {
            h.update(format!(">{}:{:?}\n", e.to, e.kind).as_bytes());
        }
    }
    h.finalize().iter().map(|x| format!("{x:02x}")).collect()
}

/// [`object_digest`] of each variant of a constructor or destructor, keyed by
/// its variant code (#280). A code seen twice keeps both, the second as
/// `<code>#2`, so neither body is dropped from the fingerprint.
fn variant_digests(f: &Function) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for v in &f.variants {
        let code = v.variant.clone().unwrap_or_else(|| "variant".into());
        let mut key = code.clone();
        let mut n = 1;
        while out.contains_key(&key) {
            n += 1;
            key = format!("{code}#{n}");
        }
        out.insert(key, object_digest(v));
    }
    out
}

/// Whether two fingerprints of one function differ in object code: its body
/// (digest and shape) or any variant, including one that appeared or went.
fn object_differs(a: &FnFingerprint, b: &FnFingerprint) -> bool {
    a.object_sha256 != b.object_sha256 || a.instr_total != b.instr_total || a.blocks != b.blocks || a.variants != b.variants
}

/// The key a fingerprint is stored under. The display name is not unique --
/// two internal-linkage functions (a `static` in each of two same-basename
/// units) can share one -- so keying the map by name alone let the second
/// overwrite the first, and a certified function's fingerprint vanished before
/// it could be compared (#249). Name plus file and start line is unique across
/// the functions one image can hold. The unit separator cannot appear in a
/// name or a path, so the parts stay unambiguous.
pub(crate) fn fp_key(name: &str, file: &str, line_start: u32) -> String {
    format!("{name}\u{1f}{file}\u{1f}{line_start}")
}

/// A fingerprint's (name, file): what a moved function keeps.
type NameFile = (String, String);
/// Indices into the removed and the added side of one (name, file) group.
type Sides = (Vec<usize>, Vec<usize>);

/// How the fingerprints left unmatched by the exact identity key pair up as
/// MOVED functions: same name and file, different start line.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Moves {
    /// (removed index, added index): exactly one candidate on each side of a
    /// (name, file) group -- the same function, moved.
    pub pairs: Vec<(usize, usize)>,
    /// (removed indices, added indices) of a group with candidates on both
    /// sides but more than one on at least one side: no reliable pairing.
    pub ambiguous: Vec<Sides>,
}

/// Group `removed` and `added` (each a (name, file) per unmatched fingerprint)
/// by (name, file). A removed entry in no group with an added one is simply
/// gone; an added entry in none is simply new. Deterministic (BTreeMap order).
pub(crate) fn reconcile_moves(removed: &[NameFile], added: &[NameFile]) -> Moves {
    let mut groups: BTreeMap<&NameFile, Sides> = BTreeMap::new();
    for (i, k) in removed.iter().enumerate() {
        groups.entry(k).or_default().0.push(i);
    }
    for (i, k) in added.iter().enumerate() {
        groups.entry(k).or_default().1.push(i);
    }
    let mut m = Moves::default();
    for (_, (ris, ais)) in groups {
        match (ris.len(), ais.len()) {
            (0, _) | (_, 0) => {}
            (1, 1) => m.pairs.push((ris[0], ais[0])),
            _ => m.ambiguous.push((ris, ais)),
        }
    }
    m
}

/// Baseline functions that no longer exist in `ev` -- the deletions
/// `change::analyse` has to locate. The identity is the same composite key
/// `compare()` uses, and so is the move reconciliation: a function whose start
/// line merely shifted (one candidate on each side of its (name, file)) is a
/// move, not a deletion plus an addition. In an ambiguous group, a deletion is
/// certain only when the group shrank (more baseline candidates than live
/// ones); which one went can't be told, so every baseline candidate of that
/// group is reported -- over-reporting a deletion costs a reviewer a look,
/// missing one loses a retired requirement. A group that did not shrink is
/// taken as moves (and `compare()` invalidates it), so no deletion is invented.
pub(crate) fn gone<'a>(base: &'a Baseline, ev: &Evidence) -> Vec<&'a FnFingerprint> {
    let live: Vec<(String, NameFile)> =
        ev.functions.iter().filter_map(|f| ev.files.get(f.file_id).map(|file| (fp_key(&f.name, &file.path, f.line_start), (f.name.clone(), file.path.clone())))).collect();
    let live_keys: BTreeSet<&str> = live.iter().map(|(k, _)| k.as_str()).collect();
    let removed: Vec<&FnFingerprint> = base.functions.iter().filter(|(k, _)| !live_keys.contains(k.as_str())).map(|(_, fp)| fp).collect();
    let added: Vec<NameFile> = live.iter().filter(|(k, _)| !base.functions.contains_key(k)).map(|(_, nf)| nf.clone()).collect();
    let moves = reconcile_moves(&removed.iter().map(|fp| (fp.name.clone(), fp.file.clone())).collect::<Vec<_>>(), &added);
    let mut keep = vec![true; removed.len()];
    for &(ri, _) in &moves.pairs {
        keep[ri] = false;
    }
    for (ris, ais) in &moves.ambiguous {
        if ris.len() <= ais.len() {
            for &ri in ris {
                keep[ri] = false;
            }
        }
    }
    removed.into_iter().zip(keep).filter(|(_, k)| *k).map(|(fp, _)| fp).collect()
}

pub fn from_evidence(ev: &Evidence) -> Baseline {
    let mut constructs = BTreeSet::new();
    let mut functions = BTreeMap::new();
    for f in &ev.functions {
        for k in f.constructs.keys() {
            constructs.insert(k.clone());
        }
        let file = &ev.files[f.file_id];
        let mut h = Sha256::new();
        let last = file.text.len().min(u32::MAX as usize) as u32;
        for l in f.line_start.max(1)..=f.line_end.min(last) {
            if let Some(t) = file.text.get((l - 1) as usize) {
                h.update(t.trim().as_bytes());
                h.update(b"\n");
            }
        }
        functions.insert(
            fp_key(&f.name, &file.path, f.line_start),
            FnFingerprint {
                name: f.name.clone(),
                source_sha256: h.finalize().iter().map(|b| format!("{b:02x}")).collect(),
                constructs: f.constructs.clone(),
                instr_total: f.object.as_ref().map(|o| o.instr_total).unwrap_or(0),
                blocks: f.object.as_ref().map(|o| o.blocks.len()).unwrap_or(0),
                object_sha256: f.object.as_ref().map(object_digest).unwrap_or_default(),
                variants: variant_digests(f),
                file: file.path.clone(),
                line_start: f.line_start,
                line_end: f.line_end,
            },
        );
    }
    let bi = &ev.provenance.build_info;
    Baseline {
        schema: BASELINE_SCHEMA.into(),
        created_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        compiler: bi.get("compiler").cloned().unwrap_or_default(),
        flags_target: bi.get("flags_target").cloned().unwrap_or_default(),
        flags_instr: bi.get("flags_instr").cloned().unwrap_or_default(),
        arch: ev.provenance.arch.clone(),
        object_format: ev.provenance.object_format.clone(),
        coverage_export_version: ev.provenance.coverage_export_version.clone(),
        build_info: bi.clone(),
        constructs: constructs.into_iter().collect(),
        functions,
    }
}

/// The build-info keys naming the tools that produced the coverage export,
/// and how a difference names each: the compiler of the instrumented build,
/// the `llvm-profdata` and `llvm-cov` that merged and exported its profile
/// (#298), and for a GCC build the `gcov` that exported it (#289). The same
/// keys are written by `butterfly build` and by the shipped adapters.
pub const COVERAGE_TOOLCHAIN: [(&str, &str); 8] = [
    ("compiler_instr", "coverage compiler"),
    ("compiler_instr_path", "coverage compiler path"),
    ("llvm_profdata", "llvm-profdata"),
    ("llvm_profdata_path", "llvm-profdata path"),
    ("llvm_cov", "llvm-cov"),
    ("llvm_cov_path", "llvm-cov path"),
    ("gcov", "gcov"),
    ("gcov_path", "gcov path"),
];

pub fn compare(base: &Baseline, ev: &Evidence, path: &str) -> BaselineCheck {
    let cur = from_evidence(ev);
    let mut d = Vec::new();
    let mut diff = |kind: &str, detail: String, invalidates: bool| d.push(BaselineDifference { kind: kind.into(), detail, invalidates });
    if base.compiler != cur.compiler {
        diff("compiler", format!("compiler changed: `{}` -> `{}`", base.compiler, cur.compiler), true);
    }
    if base.flags_target != cur.flags_target {
        diff("flags", format!("target build flags changed: `{}` -> `{}`", base.flags_target, cur.flags_target), true);
    }
    if base.flags_instr != cur.flags_instr {
        diff("flags", format!("instrumented build flags changed: `{}` -> `{}`", base.flags_instr, cur.flags_instr), false);
    }
    if base.arch != cur.arch || base.object_format != cur.object_format {
        diff("arch", format!("target changed: {} {} -> {} {}", base.object_format, base.arch, cur.object_format, cur.arch), true);
    }
    if base.coverage_export_version != cur.coverage_export_version {
        diff("coverage-toolchain", format!("coverage export version changed: `{}` -> `{}`", base.coverage_export_version, cur.coverage_export_version), true);
    }
    // What produced the coverage export, each tool named as the variant
    // compiler is (#298). Profile and coverage-mapping formats belong to one
    // LLVM, so coverage measured with another toolchain is not the coverage
    // the baseline's analyses rest on. A baseline frozen before these were
    // recorded says nothing about them, which is a note; evidence that no
    // longer records one the baseline names cannot show it unchanged.
    for (k, what) in COVERAGE_TOOLCHAIN {
        match (base.build_info.get(k), cur.build_info.get(k)) {
            (Some(b), Some(c)) if b != c => diff("coverage-toolchain", format!("{what} changed: `{b}` -> `{c}`"), true),
            (None, Some(c)) => diff("coverage-toolchain", format!("{what}: not recorded when the baseline was frozen; now `{c}`"), false),
            (Some(b), None) => diff("coverage-toolchain", format!("{what}: `{b}` in the baseline, not recorded now, so it cannot be shown unchanged"), true),
            _ => {}
        }
    }
    let is_handled_key =
        |k: &str| matches!(k, "compiler" | "flags_target" | "flags_instr" | "variant" | "project" | "butterfly_build" | "note") || COVERAGE_TOOLCHAIN.iter().any(|(c, _)| *c == k);
    for (k, v) in &cur.build_info {
        if is_handled_key(k) {
            continue;
        }
        match base.build_info.get(k) {
            Some(b) if b != v => diff("toolchain", format!("{k}: `{b}` -> `{v}`"), true),
            None => diff("toolchain", format!("{k}: (absent) -> `{v}`"), false),
            _ => {}
        }
    }
    for (k, b) in &base.build_info {
        if is_handled_key(k) || cur.build_info.contains_key(k) {
            continue;
        }
        diff("toolchain", format!("{k}: `{b}` in the baseline, not recorded now, so it cannot be shown unchanged"), true);
    }
    let base_c: BTreeSet<&String> = base.constructs.iter().collect();
    let new_constructs: Vec<String> = cur.constructs.iter().filter(|c| !base_c.contains(c)).cloned().collect();
    if !new_constructs.is_empty() {
        diff("constructs", format!("constructs not present when the baseline was frozen: {}", new_constructs.join(", ")), true);
    }
    let mut src_changed = Vec::new();
    let mut obj_changed = Vec::new();
    // Exact-key matches first (same name, file and start line): unchanged
    // position, so a body/object edit is caught directly. Whatever is left on
    // either side is a fingerprint with no exact counterpart -- new, gone, or a
    // function that MOVED (its start line, part of the key, shifted).
    let mut added_fp: Vec<&FnFingerprint> = Vec::new();
    for (key, fp) in &cur.functions {
        match base.functions.get(key) {
            None => added_fp.push(fp),
            Some(b) => {
                if b.source_sha256 != fp.source_sha256 {
                    src_changed.push(fp.name.clone());
                } else if object_differs(b, fp) {
                    obj_changed.push(fp.name.clone());
                }
            }
        }
    }
    let removed_fp: Vec<&FnFingerprint> = base.functions.iter().filter(|(k, _)| !cur.functions.contains_key(*k)).map(|(_, fp)| fp).collect();

    // Reconcile MOVED functions. `line_start` is in the identity key (it
    // disambiguates two same-name statics), so a function whose preceding lines
    // shifted appears as one removed (old key) and one added (new key). Counted
    // only as add+remove -- both non-invalidating -- a function that moved AND
    // changed its body would slip through as `changed-minor` with no
    // source/object-changed entry, and a name-based review (ui/app.js `stale()`,
    // impact/report) would stay "current" over a changed body (#249). Pair them
    // by (name, file) and classify by their digests; an ambiguous group (more
    // than one candidate on a side) can't be paired reliably, so it invalidates
    // rather than downgrades. `change::analyse` finds deletions with the same
    // pairing (see `gone`), so a move is never a deletion there either.
    let nf = |fp: &&FnFingerprint| (fp.name.clone(), fp.file.clone());
    let moves = reconcile_moves(&removed_fp.iter().map(nf).collect::<Vec<_>>(), &added_fp.iter().map(nf).collect::<Vec<_>>());
    let mut add_used = vec![false; added_fp.len()];
    let mut rem_used = vec![false; removed_fp.len()];
    for &(ri, ai) in &moves.pairs {
        let (a, b) = (added_fp[ai], removed_fp[ri]);
        if a.source_sha256 != b.source_sha256 {
            src_changed.push(a.name.clone());
        } else if object_differs(a, b) {
            obj_changed.push(a.name.clone());
        }
        // else: a benign move -- identical source and object -- no difference
        add_used[ai] = true;
        rem_used[ri] = true;
    }
    for (ris, ais) in &moves.ambiguous {
        // ambiguous: cannot pair them, so invalidate every such move
        for &ai in ais {
            src_changed.push(added_fp[ai].name.clone());
            add_used[ai] = true;
        }
        for &ri in ris {
            rem_used[ri] = true;
        }
    }
    let added: Vec<String> = added_fp.iter().enumerate().filter(|(i, _)| !add_used[*i]).map(|(_, fp)| fp.name.clone()).collect();
    let removed: Vec<String> = removed_fp.iter().enumerate().filter(|(i, _)| !rem_used[*i]).map(|(_, fp)| fp.name.clone()).collect();
    if !src_changed.is_empty() {
        diff("functions", format!("{} function(s) changed source since the baseline", src_changed.len()), true);
    }
    if !obj_changed.is_empty() {
        diff("functions", format!("{} function(s) have different object code for unchanged source (compiler/options effect)", obj_changed.len()), true);
    }
    if !added.is_empty() {
        diff("functions", format!("{} function(s) added since the baseline", added.len()), false);
    }
    if !removed.is_empty() {
        diff("functions", format!("{} function(s) removed since the baseline", removed.len()), false);
    }
    let invalid = d.iter().any(|x| x.invalidates);
    BaselineCheck {
        path: path.to_string(),
        created_unix: base.created_unix,
        status: if d.is_empty() {
            "matches".into()
        } else if invalid {
            "changed".into()
        } else {
            "changed-minor".into()
        },
        differences: d,
        functions_source_changed: src_changed,
        functions_object_changed: obj_changed,
        functions_added: added,
        functions_removed: removed,
        new_constructs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One function, one block, at a chosen load address.
    fn func(name: &str, base: u64, instrs: &[(&str, &str)]) -> Function {
        let o = ObjectFn {
            symbol: name.into(),
            address: base,
            size: (instrs.len() * 4) as u64,
            instr_total: instrs.len(),
            blocks: vec![Block {
                id: 0,
                address: base,
                instrs: instrs
                    .iter()
                    .enumerate()
                    .map(|(i, (m, ops))| Instr { addr: base + (i * 4) as u64, mnemonic: (*m).into(), operands: (*ops).into(), ..Default::default() })
                    .collect(),
                succs: vec![],
                ..Default::default()
            }],
            ..Default::default()
        };
        Function { id: 0, name: name.into(), line_start: 1, line_end: 3, object: Some(o), ..Default::default() }
    }

    fn ev_of(f: Function) -> Evidence {
        let file = SourceFile {
            id: 0,
            path: "/s/a.c".into(),
            display: "a.c".into(),
            text: vec!["int f(void) {".into(), "  return 1;".into(), "}".into()],
            function_ids: vec![0],
            ..Default::default()
        };
        Evidence { schema: SCHEMA.into(), files: vec![file], functions: vec![f], ..Default::default() }
    }

    fn digest_of(f: Function) -> String {
        from_evidence(&ev_of(f)).functions.values().next().unwrap().object_sha256.clone()
    }

    /// Every hexadecimal address outside the function collapsed to `@ext`,
    /// so two calls to different places fingerprinted identically and the
    /// baseline reported the object code unchanged when the callee had been
    /// swapped (review of #12).
    #[test]
    fn different_external_targets_do_not_share_a_fingerprint() {
        // unannotated: objdump could not name the target
        assert_ne!(
            digest_of(func("f", 0x1000, &[("bl", "0x2000")])),
            digest_of(func("f", 0x1000, &[("bl", "0x3000")])),
            "two calls to different unresolved targets are not the same code"
        );
        // annotated: the symbol is the identity, and the raw address may move
        assert_ne!(
            digest_of(func("f", 0x1000, &[("bl", "0x2000 <alpha>")])),
            digest_of(func("f", 0x1000, &[("bl", "0x2000 <beta>")])),
            "two calls to different named targets are not the same code"
        );
        assert_eq!(
            digest_of(func("f", 0x1000, &[("bl", "0x2000 <alpha>")])),
            digest_of(func("f", 0x1000, &[("bl", "0x9000 <alpha>")])),
            "the same named target relocated elsewhere is the same code"
        );
    }

    /// An architecture-specific immediate is not an address. AArch64 signs
    /// its immediates (`#-0x1`) and RISC-V writes them bare (`lui a0,
    /// 0xdeadc`); both were normalised as external addresses, so changing
    /// the constant did not change the fingerprint (review of #12).
    #[test]
    fn architecture_immediates_are_not_addresses() {
        assert_ne!(
            digest_of(func("f", 0x1000, &[("mov", "w8, #-0x1")])),
            digest_of(func("f", 0x1000, &[("mov", "w8, #-0x2")])),
            "a signed AArch64 immediate is part of what the code does"
        );
        assert_ne!(digest_of(func("f", 0x1000, &[("lui", "a0, 0x1")])), digest_of(func("f", 0x1000, &[("lui", "a0, 0x2")])), "a bare RISC-V immediate is too");
        assert_ne!(digest_of(func("f", 0x1000, &[("add", "sp, sp, #0x10")])), digest_of(func("f", 0x1000, &[("add", "sp, sp, #0x20")])), "a stack adjustment changes the frame");
    }

    /// The offset inside an annotation is part of the target's identity.
    /// It was itself a bare `0x…` outside the function, so it collapsed to
    /// `@ext` along with the address it qualifies: every branch into the
    /// same symbol fingerprinted alike. Found while fixing the review
    /// findings; it is the same defect one level down.
    #[test]
    fn different_offsets_into_the_same_symbol_do_not_share_a_fingerprint() {
        assert_ne!(
            digest_of(func("f", 0x1000, &[("b", "0x5008 <other+0x8>")])),
            digest_of(func("f", 0x1000, &[("b", "0x5020 <other+0x20>")])),
            "two branches into different parts of the same function are not the same code"
        );
    }

    /// An annotation's offset is relative to its symbol, so both move
    /// together when the image is relinked and the fingerprint must not
    /// notice. This is what makes it safe to keep those offsets verbatim.
    #[test]
    fn an_annotation_offset_survives_relinking() {
        let at = |delta: u64| {
            digest_of(func(
                "f",
                0x1000 + delta,
                &[
                    ("adrp", &format!("x8, {:#x} <page+0x4000>", 0x8000 + delta)),
                    ("bl", &format!("{:#x} <helper+0x10>", 0x9000 + delta)),
                    ("b", &format!("{:#x} <f+0x8>", 0x1008 + delta)),
                    ("ret", ""),
                ],
            ))
        };
        assert_eq!(at(0), at(0x300000), "relocating the whole image is not a code change");
    }

    /// The property the normalisation exists for must survive all of it: a
    /// branch within the function is an offset from its entry, so the same
    /// code loaded at a different address keeps its fingerprint.
    #[test]
    fn relocating_the_function_does_not_change_its_fingerprint() {
        let at = |base: u64| digest_of(func("f", base, &[("b.ge", &format!("{:#x} <f+0x8>", base + 8)), ("mov", "w0, #0x1"), ("ret", "")]));
        assert_eq!(at(0x1000), at(0x400000), "a branch to its own +0x8 is the same code wherever it is loaded");
    }

    /// Instruction and block counts are not a fingerprint of the code: an
    /// opcode, an immediate or a branch target can change while both stay
    /// equal, and the baseline reported the object code unchanged (#12).
    #[test]
    fn a_changed_opcode_changes_the_fingerprint() {
        let a = digest_of(func("f", 0x1000, &[("add", "w0, w0, w1"), ("ret", "")]));
        let b = digest_of(func("f", 0x1000, &[("sub", "w0, w0, w1"), ("ret", "")]));
        assert_ne!(a, b, "add vs sub must not fingerprint alike");
    }

    #[test]
    fn a_changed_immediate_changes_the_fingerprint() {
        let a = digest_of(func("f", 0x1000, &[("mov", "w8, #0x1"), ("ret", "")]));
        let b = digest_of(func("f", 0x1000, &[("mov", "w8, #0x2"), ("ret", "")]));
        assert_ne!(a, b, "an immediate is semantic and must be fingerprinted");
    }

    #[test]
    fn a_changed_branch_target_changes_the_fingerprint() {
        let a = digest_of(func("f", 0x1000, &[("bl", "0x2000 <helper_a>"), ("ret", "")]));
        let b = digest_of(func("f", 0x1000, &[("bl", "0x2000 <helper_b>"), ("ret", "")]));
        assert_ne!(a, b, "calling a different symbol must invalidate");
        // an internal branch identified only by address: relative offset differs
        let c = digest_of(func("f", 0x1000, &[("b", "0x1004"), ("ret", "")]));
        let d = digest_of(func("f", 0x1000, &[("b", "0x1008"), ("ret", "")]));
        assert_ne!(c, d, "a different in-function target must invalidate");
    }

    #[test]
    fn the_same_code_at_a_different_load_address_is_equal() {
        // every address moves: the function, its instructions and an internal
        // branch. A reproducible rebuild elsewhere in memory is not a change.
        let a = digest_of(func("f", 0x1000, &[("b", "0x1008"), ("mov", "w8, #0x1"), ("bl", "0x9000 <helper>"), ("ret", "")]));
        let b = digest_of(func("f", 0x8000, &[("b", "0x8008"), ("mov", "w8, #0x1"), ("bl", "0xf000 <helper>"), ("ret", "")]));
        assert_eq!(a, b, "identical code relocated must compare equal");
    }

    #[test]
    fn control_flow_is_part_of_the_fingerprint() {
        let mut one = func("f", 0x1000, &[("cbz", "w0, 0x1008"), ("ret", "")]);
        let mut two = one.clone();
        one.object.as_mut().unwrap().blocks[0].succs = vec![Edge { to: 0, kind: EdgeKind::Taken, ..Default::default() }];
        two.object.as_mut().unwrap().blocks[0].succs = vec![Edge { to: 0, kind: EdgeKind::Fallthrough, ..Default::default() }];
        assert_ne!(digest_of(one), digest_of(two), "a changed edge kind must invalidate");
    }

    #[test]
    fn compare_reports_object_change_when_counts_are_identical() {
        let base = from_evidence(&ev_of(func("f", 0x1000, &[("add", "w0, w0, w1"), ("ret", "")])));
        let cur = ev_of(func("f", 0x1000, &[("sub", "w0, w0, w1"), ("ret", "")]));
        let chk = compare(&base, &cur, "b.json");
        assert_eq!(chk.functions_object_changed, vec!["f"], "same counts, different code: {chk:?}");
        assert!(chk.functions_source_changed.is_empty());
        assert_eq!(chk.status, "changed");
        // and the identical build compares clean
        let same = ev_of(func("f", 0x1000, &[("add", "w0, w0, w1"), ("ret", "")]));
        let chk = compare(&base, &same, "b.json");
        assert!(chk.functions_object_changed.is_empty() && chk.status == "matches", "{chk:?}");
    }

    /// A destructor whose body is unchanged but whose deleting variant (`D0`)
    /// now calls a different `operator delete`, as `-fsized-deallocation`
    /// does. The fingerprint covered the body only, so the function was not
    /// object-changed (#280). Every body in `Function::objects()` counts, each
    /// under its variant code, in place and when the function moved.
    #[test]
    fn a_change_confined_to_a_variant_marks_the_function_object_changed() {
        let dtor = |delete_call: &str, line: u32| {
            let mut f = func("fc::Channel::~Channel()", 0x1000, &[("ldr", "x0, [x0]"), ("ret", "")]);
            let mut d0 = func("d0", 0x2000, &[("bl", &format!("0x3000 <{delete_call}>")), ("ret", "")]).object.unwrap();
            d0.variant = Some("D0".into());
            f.variants = vec![d0];
            f.line_start = line;
            f.line_end = line + 2;
            f
        };
        let base = from_evidence(&ev_of(dtor("__ZdlPv", 1)));
        let same = compare(&base, &ev_of(dtor("__ZdlPv", 1)), "b.json");
        assert_eq!(same.status, "matches", "the same variants must fingerprint alike: {same:?}");
        let chk = compare(&base, &ev_of(dtor("__ZdlPvm", 1)), "b.json");
        assert_eq!(chk.functions_object_changed, vec!["fc::Channel::~Channel()".to_string()], "{chk:?}");
        assert_eq!(chk.status, "changed");
        // a variant that appears or disappears is a change too
        let mut none = dtor("__ZdlPv", 1);
        none.variants.clear();
        assert_eq!(compare(&base, &ev_of(none), "b.json").functions_object_changed, vec!["fc::Channel::~Channel()".to_string()]);
        // and a moved function is judged by its variants as well
        let mut ev = ev_of(dtor("__ZdlPvm", 1));
        ev.files[0].text.insert(0, "// moved".into());
        ev.functions[0].line_start = 2;
        ev.functions[0].line_end = 4;
        let moved = compare(&base, &ev, "b.json");
        assert_eq!(moved.functions_object_changed, vec!["fc::Channel::~Channel()".to_string()], "{moved:?}");
    }

    /// A `butterfly-baseline/3` file carries no variant digests: compared as
    /// it is, every constructor and destructor would read changed (or a real
    /// change would pass unseen). It is refused with the re-freeze message.
    #[test]
    fn a_baseline_without_variant_digests_is_refused() {
        let mut old = from_evidence(&ev_of(func("f", 0x1000, &[("add", "w0, w0, w1")])));
        old.schema = "butterfly-baseline/3".into();
        let err = old.check_schema().unwrap_err();
        assert!(err.contains("butterfly baseline") && err.contains("butterfly-baseline/3"), "{err}");
    }

    /// A baseline frozen before object fingerprints cannot answer the
    /// question, so it must say so rather than silently comparing counts.
    #[test]
    fn an_older_baseline_is_rejected_with_a_migration_message() {
        let mut old = from_evidence(&ev_of(func("f", 0x1000, &[("add", "w0, w0, w1")])));
        old.schema = "butterfly-baseline/1".into();
        let err = old.check_schema().unwrap_err();
        assert!(err.contains("butterfly baseline"), "names the command to re-freeze: {err}");
        assert!(err.contains("butterfly-baseline/1"), "names what it found: {err}");
        let current = from_evidence(&ev_of(func("f", 0x1000, &[("add", "w0, w0, w1")])));
        assert!(current.check_schema().is_ok());
    }

    /// #249: two functions can share a display name -- an internal-linkage
    /// `static` in each of two same-basename units. Keyed by name alone, the
    /// second overwrote the first, so one certified function vanished from the
    /// baseline before it could be compared. Keyed by (name, file, line) both
    /// survive, and a change to either is seen.
    #[test]
    fn two_functions_of_one_name_are_not_collapsed() {
        // Two files, each with a function llvm-cov names "dup.c:tally".
        let mk = |op: &str| {
            let mut ev = Evidence { schema: SCHEMA.into(), ..Default::default() };
            for (fid, path) in [(0usize, "/s/alpha/dup.c"), (1, "/s/beta/dup.c")] {
                ev.files.push(SourceFile {
                    id: fid,
                    path: path.into(),
                    display: "dup.c".into(),
                    text: vec!["int tally(void) {".into(), format!("  return {op};"), "}".into()],
                    function_ids: vec![fid],
                    ..Default::default()
                });
                let o = ObjectFn {
                    symbol: "_tally".into(),
                    address: 0x1000 + fid as u64 * 0x100,
                    size: 8,
                    instr_total: 2,
                    blocks: vec![Block {
                        id: 0,
                        address: 0x1000 + fid as u64 * 0x100,
                        instrs: vec![
                            Instr { addr: 0x1000, mnemonic: "mov".into(), operands: format!("w0, #{op}"), ..Default::default() },
                            Instr { addr: 0x1004, mnemonic: "ret".into(), operands: String::new(), ..Default::default() },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                };
                // Both functions carry the same DISPLAY name.
                ev.functions.push(Function { id: fid, name: "dup.c:tally".into(), file_id: fid, line_start: 1, line_end: 3, object: Some(o), ..Default::default() });
            }
            ev
        };

        let base = from_evidence(&mk("1"));
        // Both are fingerprinted -- a name-keyed map would hold only one.
        assert_eq!(base.functions.len(), 2, "both same-named functions must be kept distinct");
        assert!(base.functions.values().all(|fp| fp.name == "dup.c:tally"));

        // Change only the beta copy: it must be reported as changed, and the
        // alpha copy must not be reported as removed or added (the collapse
        // used to hide exactly this).
        let mut changed = mk("1");
        changed.files[1].text[1] = "  return 2;".into();
        changed.functions[1].object.as_mut().unwrap().blocks[0].instrs[0].operands = "w0, #2".into();
        let chk = compare(&base, &changed, "b.json");
        assert!(chk.functions_source_changed.iter().any(|_| true) || chk.status == "changed", "a change to one twin must be seen: {chk:?}");
        assert!(chk.functions_added.is_empty(), "the unchanged twin must not read as added: {chk:?}");
        assert!(chk.functions_removed.is_empty(), "the unchanged twin must not read as removed: {chk:?}");
    }

    /// One function at `line_start`, its 3-line body taken from `text`.
    fn ev_at(name: &str, path: &str, line_start: u32, text: &[&str], addr: u64, ops: &str) -> Evidence {
        let o = ObjectFn {
            symbol: name.into(),
            address: addr,
            size: 4,
            instr_total: 1,
            blocks: vec![Block {
                id: 0,
                address: addr,
                instrs: vec![Instr { addr, mnemonic: "mov".into(), operands: ops.into(), ..Default::default() }],
                succs: vec![],
                ..Default::default()
            }],
            ..Default::default()
        };
        let file = SourceFile { id: 0, path: path.into(), display: path.into(), text: text.iter().map(|s| s.to_string()).collect(), function_ids: vec![0], ..Default::default() };
        let f = Function { id: 0, name: name.into(), file_id: 0, line_start, line_end: line_start + 2, object: Some(o), ..Default::default() };
        Evidence { schema: SCHEMA.into(), files: vec![file], functions: vec![f], ..Default::default() }
    }

    #[test]
    fn a_function_that_moves_and_changes_body_is_source_changed_not_add_remove() {
        // base: f at line 1, body A. current: two lines inserted above it (so it
        // moves to line 3, a new identity key) AND its body changes to B.
        let base = from_evidence(&ev_at("f", "/s/a.c", 1, &["a1", "a2", "a3"], 0x1000, "w0, #1"));
        let cur = ev_at("f", "/s/a.c", 3, &["pad", "pad", "b1", "b2", "b3"], 0x1000, "w0, #1");
        let chk = compare(&base, &cur, "b.json");
        assert!(chk.functions_source_changed.contains(&"f".to_string()), "a moved+edited function is source-changed: {chk:?}");
        assert!(chk.functions_added.is_empty() && chk.functions_removed.is_empty(), "not add+remove: {chk:?}");
        assert_eq!(chk.status, "changed", "and it invalidates the review: {chk:?}");
    }

    #[test]
    fn a_same_name_function_in_a_different_file_is_add_and_remove_not_a_move() {
        // `foo` leaves a.c and a same-named `foo` appears in b.c: a different
        // function, not a move -- the (name, file) key keeps them apart.
        let base = from_evidence(&ev_at("foo", "/s/a.c", 1, &["a1", "a2", "a3"], 0x1000, "w0, #1"));
        let cur = ev_at("foo", "/s/b.c", 1, &["a1", "a2", "a3"], 0x1000, "w0, #1");
        let chk = compare(&base, &cur, "b.json");
        assert!(chk.functions_added.contains(&"foo".to_string()), "the b.c copy is added: {chk:?}");
        assert!(chk.functions_removed.contains(&"foo".to_string()), "the a.c copy is removed: {chk:?}");
        assert!(chk.functions_source_changed.is_empty(), "a different file is not a move: {chk:?}");
    }

    #[test]
    fn a_function_that_only_moves_does_not_invalidate() {
        // moved to line 3 but the body and object are identical: a benign move.
        let base = from_evidence(&ev_at("f", "/s/a.c", 1, &["a1", "a2", "a3"], 0x1000, "w0, #1"));
        let cur = ev_at("f", "/s/a.c", 3, &["pad", "pad", "a1", "a2", "a3"], 0x1000, "w0, #1");
        let chk = compare(&base, &cur, "b.json");
        assert!(chk.functions_source_changed.is_empty() && chk.functions_object_changed.is_empty(), "a pure move is not a source/object change: {chk:?}");
        assert!(chk.functions_added.is_empty() && chk.functions_removed.is_empty(), "and not add+remove: {chk:?}");
        assert_ne!(chk.status, "changed", "a benign move does not invalidate: {chk:?}");
    }

    /// Two same-name functions in one file (C++ overloads share a display
    /// name), each at `starts`.
    fn ev_two(name: &str, path: &str, starts: &[u32]) -> Evidence {
        let mut ev = ev_at(name, path, starts[0], &["a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9", "a10"], 0x1000, "w0, #1");
        for (i, &ls) in starts.iter().enumerate().skip(1) {
            let mut f = ev.functions[0].clone();
            f.id = i;
            f.line_start = ls;
            f.line_end = ls + 2;
            ev.files[0].function_ids.push(i);
            ev.functions.push(f);
        }
        ev
    }

    #[test]
    fn a_function_that_only_moved_is_not_a_deletion() {
        // change::analyse finds deletions with this: a function whose start
        // line shifted is the same function, not one gone and one new (#249).
        let base = from_evidence(&ev_at("f", "/s/a.c", 1, &["a1", "a2", "a3"], 0x1000, "w0, #1"));
        let moved = ev_at("f", "/s/a.c", 3, &["pad", "pad", "a1", "a2", "a3"], 0x1000, "w0, #1");
        assert!(gone(&base, &moved).is_empty(), "a pure move is not a deletion");
        let edited = ev_at("f", "/s/a.c", 3, &["pad", "pad", "b1", "b2", "b3"], 0x1000, "w0, #1");
        assert!(gone(&base, &edited).is_empty(), "a moved+edited function is source-changed (compare), not deleted");
    }

    #[test]
    fn a_function_that_left_its_file_is_a_deletion() {
        // same name, different file: a different function, so the a.c one is gone
        let base = from_evidence(&ev_at("foo", "/s/a.c", 1, &["a1", "a2", "a3"], 0x1000, "w0, #1"));
        let cur = ev_at("foo", "/s/b.c", 1, &["a1", "a2", "a3"], 0x1000, "w0, #1");
        let g: Vec<(&str, &str)> = gone(&base, &cur).iter().map(|fp| (fp.name.as_str(), fp.file.as_str())).collect();
        assert_eq!(g, [("foo", "/s/a.c")]);
        // and with nothing of that name left at all
        let mut empty = cur.clone();
        empty.functions.clear();
        empty.files[0].function_ids.clear();
        assert_eq!(gone(&base, &empty).len(), 1);
    }

    #[test]
    fn an_ambiguous_group_is_a_deletion_only_when_it_shrank() {
        let base = from_evidence(&ev_two("C::m", "/s/a.cc", &[1, 5]));
        // both overloads moved: the count held, so nothing was deleted
        assert!(gone(&base, &ev_two("C::m", "/s/a.cc", &[2, 6])).is_empty(), "two moved overloads are not deletions");
        // one moved, one of the originals kept its line: an exact match plus a
        // 1:1 move -- nothing gone
        assert!(gone(&base, &ev_two("C::m", "/s/a.cc", &[1, 7])).is_empty());
        // three baseline overloads, two left and both moved: one was deleted
        // but which can't be told, so every candidate is reported
        let base3 = from_evidence(&ev_two("C::m", "/s/a.cc", &[1, 4, 7]));
        assert_eq!(gone(&base3, &ev_two("C::m", "/s/a.cc", &[2, 5])).len(), 3, "a shrunk ambiguous group reports every candidate");
        // and compare() invalidates that ambiguous group rather than downgrading it
        let chk = compare(&base3, &ev_two("C::m", "/s/a.cc", &[2, 5]), "b.json");
        assert_eq!(chk.status, "changed", "{chk:?}");
    }

    /// Evidence whose build-info records the given coverage toolchain.
    fn ev_built_with(keys: &[(&str, &str)]) -> Evidence {
        let mut ev = ev_of(func("f", 0x1000, &[("ret", "")]));
        ev.provenance.build_info.insert("compiler".into(), "clang version 18.1.8".into());
        for (k, v) in keys {
            ev.provenance.build_info.insert(k.to_string(), v.to_string());
        }
        ev
    }

    const RECORDED: [(&str, &str); 6] = [
        ("compiler_instr", "clang version 18.1.8"),
        ("compiler_instr_path", "/usr/lib/llvm-18/bin/clang"),
        ("llvm_profdata", "LLVM version 18.1.8"),
        ("llvm_profdata_path", "/usr/lib/llvm-18/bin/llvm-profdata"),
        ("llvm_cov", "LLVM version 18.1.8"),
        ("llvm_cov_path", "/usr/lib/llvm-18/bin/llvm-cov"),
    ];

    /// The tools that produced `coverage.json` are compared as the variant
    /// compiler is: each one named, and a change invalidates (#298). Profile
    /// and coverage-mapping formats belong to one LLVM, so coverage measured
    /// with another is not the coverage the baseline's analyses rest on.
    #[test]
    fn a_changed_coverage_toolchain_is_named_and_invalidates() {
        let base = from_evidence(&ev_built_with(&RECORDED));
        assert_eq!(compare(&base, &ev_built_with(&RECORDED), "b.json").status, "matches");
        for (key, what) in [
            ("compiler_instr", "coverage compiler"),
            ("compiler_instr_path", "coverage compiler path"),
            ("llvm_profdata", "llvm-profdata"),
            ("llvm_profdata_path", "llvm-profdata path"),
            ("llvm_cov", "llvm-cov"),
            ("llvm_cov_path", "llvm-cov path"),
        ] {
            let mut keys = RECORDED;
            keys.iter_mut().find(|(k, _)| *k == key).unwrap().1 = "19.1.0 elsewhere";
            let chk = compare(&base, &ev_built_with(&keys), "b.json");
            assert_eq!(chk.differences.len(), 1, "{key}: one difference, reported once: {chk:?}");
            let d = &chk.differences[0];
            assert_eq!(d.kind, "coverage-toolchain", "{key}: {d:?}");
            assert!(d.detail.starts_with(&format!("{what} changed: ")) && d.detail.contains("19.1.0 elsewhere"), "{key}: {d:?}");
            assert!(d.invalidates, "{key}: a changed coverage toolchain invalidates: {d:?}");
            assert_eq!(chk.status, "changed", "{key}");
        }
    }

    /// A baseline frozen before the coverage toolchain was recorded says
    /// nothing about it: that is a note, not a change. Evidence that no
    /// longer records a tool the baseline names cannot show it unchanged,
    /// and invalidates.
    #[test]
    fn a_coverage_toolchain_recorded_on_one_side_only_is_reported() {
        let old = from_evidence(&ev_built_with(&[]));
        let chk = compare(&old, &ev_built_with(&RECORDED), "b.json");
        assert_eq!(chk.differences.len(), RECORDED.len(), "{chk:?}");
        assert!(chk.differences.iter().all(|d| d.kind == "coverage-toolchain" && !d.invalidates), "{chk:?}");
        assert_eq!(chk.status, "changed-minor", "{chk:?}");

        let base = from_evidence(&ev_built_with(&RECORDED));
        let chk = compare(&base, &ev_built_with(&RECORDED[2..]), "b.json");
        assert_eq!(chk.differences.len(), 2, "{chk:?}");
        assert!(chk.differences.iter().all(|d| d.kind == "coverage-toolchain" && d.invalidates && d.detail.contains("not recorded")), "{chk:?}");
        assert_eq!(chk.status, "changed", "{chk:?}");
    }
}
