// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The source definition index, from Universal Ctags.
//!
//! Optional: needs universal-ctags. Where it is absent the produce test skips;
//! CI installs it so the suite runs. The find_ctags test needs nothing.

use std::path::{Path, PathBuf};

/// A path no other test in this binary will use.
///
/// Every test here used `butterfly-tags-ev-{pid}.json` and removed it when
/// it finished. Rust runs them concurrently in one process, so one test
/// could delete the file while another was inside `produce`, whose
/// best-effort `read(...).ok()` then hashed nothing and wrote an empty
/// evidence hash -- the macOS CI failure, and a race rather than anything
/// platform-specific (review of #32).
fn unique_evidence_path(tag: &str) -> PathBuf {
    butterfly_testsupport::scratch_file(&format!("tags-ev-{tag}"), ".json")
}

fn example_evidence(tag: &str) -> Option<(butterfly_model::Evidence, PathBuf, PathBuf)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let build = root.join("examples/c-lib/build");
    if !build.join("coverage.json").exists() {
        butterfly_testsupport::fixture_missing("run ./examples/c-lib/build.sh first");
        return None;
    }
    let src_root = root.join("examples/c-lib");
    // ingest in-process, then write it to a temp file so `produce` has a path
    // to hash for the stale-detection field
    let mut opts = butterfly_ingest::discover(&build).expect("discover");
    opts.source_root = Some(src_root.clone());
    let ev = butterfly_ingest::ingest(&opts).expect("ingest");
    let ev_path = unique_evidence_path(tag);
    std::fs::write(&ev_path, serde_json::to_string(&ev).unwrap()).unwrap();
    Some((ev, ev_path, src_root))
}

#[test]
fn find_ctags_rejects_a_non_universal_binary() {
    // `true` exists and exits 0 but is not ctags: its --version says nothing
    assert!(butterfly_ingest::tags::find_ctags(Some(Path::new("/usr/bin/true"))).is_none());
    // a path that does not exist is not a ctags either
    assert!(butterfly_ingest::tags::find_ctags(Some(Path::new("/nonexistent/ctags"))).is_none());
}

#[test]
fn produce_indexes_functions_headers_and_types() {
    let Some(ctags) = butterfly_ingest::tags::find_ctags(None) else {
        butterfly_testsupport::fixture_missing("universal-ctags not installed");
        return;
    };
    let Some((ev, ev_path, root)) = example_evidence("produce_indexes_functions_headers_and_types") else { return };

    let tags = butterfly_ingest::tags::produce(&ev, &ev_path, &root, &ctags).expect("produce").tags;
    tags.validate(ev.files.len(), ev.functions.len()).expect("valid");
    let by_name = |n: &str| tags.entries.iter().find(|t| t.name == n);

    // an analysed function links to its evidence function -> the viewer can
    // send it to the control-flow view
    let crc = by_name("crc_block").expect("crc_block indexed");
    assert_eq!(crc.kind, "function");
    assert!(crc.file_id.is_some(), "defined in an evidence file");
    assert!(crc.function_id.is_some(), "and linked to the analysed function");

    // A static function IS analysed -- the evidence names it `util.c:abs_i32`
    // to keep it apart from any other file's -- so its tag links to that
    // control-flow graph. Matching only the bare name would have missed it.
    let stat = by_name("abs_i32").expect("static function indexed");
    assert_eq!(stat.kind, "function");
    let fid = stat.function_id.expect("a static function is still analysed, under a file-qualified name");
    assert!(ev.functions[fid].name.ends_with("abs_i32"), "and it links to that one: {}", ev.functions[fid].name);

    // a definition in a header the evidence never compiled: file_id is None,
    // which is how the viewer knows it is not one of the files on the page
    let macro_tag = tags.entries.iter().find(|t| t.kind == "macro" && t.file_id.is_none()).expect("a header-only macro");
    assert!(macro_tag.file.ends_with(".h"), "it is in a header: {}", macro_tag.file);

    // and the types the object model has no notion of are present
    assert!(by_name("flight_state_t").is_some_and(|t| t.kind == "typedef"), "the typedef is indexed");
    assert!(tags.entries.iter().any(|t| t.kind == "member"), "struct members are indexed");

    // provenance and a stale-detection hash
    assert!(tags.generator.contains("Universal Ctags"));
    assert!(!tags.evidence_sha256.is_empty(), "records the evidence hash so a stale index is detectable");

    let _ = std::fs::remove_file(ev_path);
}

/// The filenames in an evidence document are untrusted -- the same input that
/// made arbitrary local files readable in #1. ctags would parse whatever they
/// name and put its symbol names in the index, so a path outside the source
/// root must never be read.
#[test]
fn a_file_named_outside_the_source_root_is_never_read() {
    let Some(ctags) = butterfly_ingest::tags::find_ctags(None) else {
        butterfly_testsupport::fixture_missing("universal-ctags not installed");
        return;
    };
    let Some((mut ev, ev_path, root)) = example_evidence("a_file_named_outside_the_source_root_is_never_read") else { return };

    let d = butterfly_testsupport::scratch_dir("tags-esc");
    let secret = d.join("private.c");
    std::fs::write(&secret, "int SECRET_SYMBOL_NAME(void) { return 42; }\n").unwrap();

    // a crafted evidence names a file outside the root
    ev.files[0].path = secret.display().to_string();
    ev.files[0].display = "private.c".into();

    let produced = butterfly_ingest::tags::produce(&ev, &ev_path, &root, &ctags).expect("produce");
    assert!(!produced.tags.entries.iter().any(|t| t.name.contains("SECRET_SYMBOL_NAME")), "a file outside the source root was read and its symbols indexed");
    assert!(produced.outside.iter().any(|o| o.contains("private.c")), "and the refusal is reported: {:?}", produced.outside);

    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_file(ev_path);
}

/// Two translation units can each define `static helper`. Linking a tag to an
/// analysed function by name alone would attach both to whichever one the
/// evidence carries, sending the viewer to the wrong control-flow graph.
#[test]
fn a_tag_never_links_to_a_function_in_another_file() {
    let Some(ctags) = butterfly_ingest::tags::find_ctags(None) else {
        butterfly_testsupport::fixture_missing("universal-ctags not installed");
        return;
    };
    let Some((ev, ev_path, root)) = example_evidence("a_tag_never_links_to_a_function_in_another_file") else { return };
    let produced = butterfly_ingest::tags::produce(&ev, &ev_path, &root, &ctags).expect("produce");

    for t in &produced.tags.entries {
        if let Some(fid) = t.function_id {
            let f = &ev.functions[fid];
            assert_eq!(Some(f.file_id), t.file_id, "tag `{}` links to a function in a different file", t.name);
        }
    }
    let _ = std::fs::remove_file(ev_path);
}

/// A directory symlink is not followed: one pointing at an ancestor is an
/// infinite descent, and one pointing outside walks past the boundary the
/// caller is enforcing.
#[test]
fn a_directory_symlink_in_the_source_root_is_not_followed() {
    let Some(ctags) = butterfly_ingest::tags::find_ctags(None) else {
        butterfly_testsupport::fixture_missing("universal-ctags not installed");
        return;
    };
    let Some((ev, ev_path, root)) = example_evidence("a_directory_symlink_in_the_source_root_is_not_followed") else { return };

    let d = butterfly_testsupport::scratch_dir("tags-loop");
    std::fs::create_dir_all(d.join("inc")).unwrap();
    std::fs::write(d.join("inc/real.h"), "#define REAL_ONE 1\n").unwrap();
    #[cfg(unix)]
    {
        // a loop back to the root, and an escape out of it
        let _ = std::os::unix::fs::symlink(&d, d.join("inc/loop"));
        let _ = std::os::unix::fs::symlink(&root, d.join("inc/escape"));
    }

    // it must terminate, and must not index through either link
    let produced = butterfly_ingest::tags::produce(&ev, &ev_path, &d, &ctags).expect("produce terminates");
    assert!(produced.tags.entries.iter().any(|t| t.name == "REAL_ONE"), "the real header is indexed");
    assert!(!produced.tags.entries.iter().any(|t| t.file.contains("loop") || t.file.contains("escape")), "nothing was indexed through a directory symlink");

    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_file(ev_path);
}

/// An inline function defined in a header is compiled into every translation
/// unit that includes it, so the evidence carries one analysed function per
/// unit -- all pointing at the same header line. ctags sees one definition.
/// There is no single control-flow graph for it, so it must stay unlinked and
/// route to the source line rather than to an arbitrary instantiation.
#[test]
fn an_inline_function_in_a_header_routes_to_source_not_one_instantiation() {
    let Some(ctags) = butterfly_ingest::tags::find_ctags(None) else {
        butterfly_testsupport::fixture_missing("universal-ctags not installed");
        return;
    };
    let Some((ev, ev_path, root)) = example_evidence("an_inline_function_in_a_header_routes_to_source_not_one_instantiation") else { return };
    let _ = (&ev, &root);

    // A synthetic evidence is enough: two analysed functions in one header,
    // which is exactly what including an inline function twice produces.
    let mut e = butterfly_model::Evidence { schema: butterfly_model::SCHEMA.into(), ..Default::default() };
    let hdr = root.join("include/state.h");
    e.files.push(butterfly_model::SourceFile { id: 0, path: hdr.display().to_string(), display: "state.h".into(), ..Default::default() });
    for (i, unit) in ["a.c", "b.c"].iter().enumerate() {
        e.functions.push(butterfly_model::Function { id: i, name: format!("{unit}:state_altitude"), file_id: 0, line_start: 1, line_end: 2, ..Default::default() });
    }
    e.files[0].function_ids = vec![0, 1];

    let produced = butterfly_ingest::tags::produce(&e, &ev_path, &root, &ctags).expect("produce");
    for t in produced.tags.entries.iter().filter(|t| t.kind == "function" && t.name == "state_altitude") {
        assert!(t.function_id.is_none(), "two instantiations share this definition, so there is no single CFG to open: linked to {:?}", t.function_id);
    }
    let _ = std::fs::remove_file(ev_path);
}
