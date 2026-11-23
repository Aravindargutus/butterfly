// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! End-to-end regression for the C++ construct vocabulary (#162), on the real
//! `examples/cpp` and `examples/cpp-lib` builds. The synthetic unit tests in
//! `constructs.rs` pin the vocabulary itself; these pin the *integration
//! path* — that a `.cpp` file is inventoried as C++ once ingested, and that
//! the template-prefix widening in `lib.rs` and the `override`-as-virtual
//! rule actually surface `template`/`virtual` on real functions.
//!
//! Runs only when the fixture has been built (`examples/cpp*/build.sh` need
//! clang++ + rustup llvm-tools + dsymutil); skipped, not failed, otherwise, so
//! `cargo test` stays green on a machine without the C++ toolchain.
use butterfly_model::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name);
    let build = root.join("build");
    if build.join("coverage.json").exists() && build.join("flight").exists() {
        Some(root)
    } else {
        // cpp and cpp-lib are built by ci.yml and ci-local, so they are
        // required where the fixtures are (#242): absent, this fails rather
        // than returning without asserting. Optional locally (var unset).
        butterfly_testsupport::fixture_missing(&format!("examples/{name}/build not present (run examples/{name}/build.sh)"));
        None
    }
}

fn ingest(root: &Path) -> Evidence {
    let mut opts = butterfly_ingest::discover(&root.join("build")).expect("discover");
    opts.source_root = Some(root.to_path_buf());
    butterfly_ingest::ingest(&opts).expect("ingest")
}

/// Sum a construct across every function, and the set of functions that carry it.
fn construct_total(ev: &Evidence, key: &str) -> (u32, Vec<String>) {
    let mut total = 0;
    let mut fns = Vec::new();
    for f in &ev.functions {
        if let Some(&n) = f.constructs.get(key) {
            total += n;
            fns.push(f.name.clone());
        }
    }
    (total, fns)
}

fn aggregate(ev: &Evidence) -> BTreeMap<String, u32> {
    let mut m = BTreeMap::new();
    for f in &ev.functions {
        for (k, v) in &f.constructs {
            *m.entry(k.clone()).or_insert(0) += *v;
        }
    }
    m
}

#[test]
fn cpp_example_is_inventoried_with_the_cpp_vocabulary() {
    let Some(root) = fixture("cpp") else { return };
    let ev = ingest(&root);

    // The C++ vocabulary is active on the real .cpp file (a C vocabulary knows
    // none of these).
    let agg = aggregate(&ev);
    for key in ["template", "virtual", "cpp-cast", "scope-res"] {
        assert!(agg.get(key).copied().unwrap_or(0) > 0, "expected C++ construct {key:?} in examples/cpp, got {agg:?}");
    }

    // template: the widening in lib.rs pulls the `template <typename T>` prefix
    // (one line above the coverage line range) into the slice, so each
    // instantiation of saturating_add reports it.
    let (template_total, template_fns) = construct_total(&ev, "template");
    assert!(template_total >= 1, "no template construct found");
    assert!(template_fns.iter().any(|n| n.contains("saturating_add")), "the template function saturating_add should carry the `template` construct, got {template_fns:?}");

    // virtual: the abstract base's `virtual` keyword has no body, so the
    // constructs surface on the `override` methods (override == virtual).
    let (virtual_total, virtual_fns) = construct_total(&ev, "virtual");
    assert!(virtual_total >= 1, "no virtual construct found");
    assert!(virtual_fns.iter().any(|n| n.contains("apply")), "an overriding apply() should carry the `virtual` construct, got {virtual_fns:?}");

    // A plain C keyword still fires inside these C++ functions (shared c_base).
    assert!(agg.get("if").copied().unwrap_or(0) > 0);
}

#[test]
fn cpp_lib_example_is_inventoried_with_the_cpp_vocabulary() {
    let Some(root) = fixture("cpp-lib") else { return };
    let ev = ingest(&root);
    let agg = aggregate(&ev);
    // The library example has a class hierarchy of tasks (virtual run/name) and
    // a template; both must be inventoried as C++.
    for key in ["template", "virtual", "scope-res"] {
        assert!(agg.get(key).copied().unwrap_or(0) > 0, "expected C++ construct {key:?} in examples/cpp-lib, got {agg:?}");
    }
}
