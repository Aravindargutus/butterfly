// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The document's traceability claim against the measurement (#57).

mod common;

use butterfly_model::claimcheck::compare;
use butterfly_model::reqtext::{Claim, Requirement, Requirements, Source, REQTEXT_SCHEMA};
use butterfly_model::{Evidence, TestRun};
use std::collections::BTreeMap;

/// Two functions in one file, the first tagged and reached by `t_one`, the
/// second tagged and reached by nothing.
fn evidence() -> Evidence {
    let f = common::file(&["a();", "b();"], &[Some(1), Some(1)]);
    let mut reached = common::function(0, "reached", 1, 1);
    reached.requirements = vec!["REQ-1".into()];
    reached.tests = vec![0];
    let mut unreached = common::function(1, "unreached", 2, 2);
    unreached.requirements = vec!["REQ-2".into()];
    let mut ev = common::evidence(vec![f], vec![reached, unreached]);
    ev.files[0].function_ids = vec![0, 1];
    let mut lines = BTreeMap::new();
    lines.insert(0usize, vec![(1u32, 1u32)]);
    ev.tests = vec![TestRun { id: 0, name: "t_one".into(), path: "t_one.json".into(), functions: vec![0], lines }];
    ev
}

fn reqs(entries: Vec<Requirement>) -> Requirements {
    let mut entries = entries;
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    Requirements {
        schema: REQTEXT_SCHEMA.into(),
        generator: "test".into(),
        sources: vec![Source { path: "llr.yaml".into(), sha256: "0".repeat(64), count: entries.len() }],
        entries,
        skipped: BTreeMap::new(),
        without_text: Vec::new(),
    }
}

fn req(id: &str, status: &str, code: &[&str], tests: &[&str]) -> Requirement {
    Requirement {
        id: id.into(),
        text: "a statement".into(),
        kind: Some("low-level".into()),
        status: Some(status.into()),
        parent: None,
        claim: Claim { code: code.iter().map(|s| (*s).into()).collect(), tests: tests.iter().map(|s| (*s).into()).collect() },
        detail: BTreeMap::new(),
        source: 0,
        line: None,
    }
}

/// `compare`, without the `tag-not-in-document` findings.
///
/// `evidence()` tags REQ-1 and REQ-2, and most documents below hold one of
/// them, written for the one claim the test is about; the other's tag then
/// cites an ID the document does not hold. That finding is pinned by its own
/// test, and is beside the point of these.
fn compare_own(ev: &Evidence, reqs: &Requirements) -> butterfly_model::claimcheck::Report {
    let mut r = compare(ev, reqs);
    r.disagreements.retain(|d| d.kind != "tag-not-in-document");
    r
}

fn kinds(r: &butterfly_model::claimcheck::Report) -> Vec<&str> {
    r.disagreements.iter().map(|d| d.kind.as_str()).collect()
}

/// The case this whole module exists for: the document asserts a test
/// verifies a requirement, and the measurement says that test never entered
/// the code implementing it. Invisible from either document alone.
#[test]
fn a_test_claimed_to_verify_code_it_never_entered_is_reported() {
    let r = compare_own(&evidence(), &reqs(vec![req("REQ-2", "implemented", &["demo.c::unreached"], &["tests.c::t_one"])]));
    assert_eq!(kinds(&r), ["test-does-not-reach"], "{:#?}", r.disagreements);
    assert!(r.disagreements[0].measured.contains("executed none of the lines"), "{:?}", r.disagreements[0]);
}

/// A claim naming code that is not there at all -- renamed, deleted, never
/// built -- is the commonest way a traceability argument rots.
///
/// A rename reports *both* halves, and that pairing is the useful part: the
/// name the document still uses, and the tagged function it does not
/// mention. Between them they say what to change and to what.
#[test]
fn a_claim_naming_code_that_does_not_exist_is_reported() {
    let r = compare_own(&evidence(), &reqs(vec![req("REQ-1", "implemented", &["demo.c::renamed_away"], &[])]));
    assert!(kinds(&r).contains(&"code-not-found"), "{:#?}", r.disagreements);
    assert!(kinds(&r).contains(&"tagged-not-claimed"), "and names the function that is tagged: {:#?}", r.disagreements);
    assert!(r.disagreements.iter().any(|d| d.measured.contains("reached")), "by name: {:#?}", r.disagreements);
}

/// The document names code that exists but carries no tag for it.
#[test]
fn code_claimed_but_not_tagged_is_reported() {
    let r = compare_own(&evidence(), &reqs(vec![req("REQ-1", "implemented", &["demo.c::unreached"], &[])]));
    assert!(kinds(&r).contains(&"claimed-not-tagged"), "{:#?}", r.disagreements);
}

/// The source is tagged with a requirement the document's traceability does
/// not mention: usually the document is behind.
#[test]
fn code_tagged_but_not_claimed_is_reported() {
    // claims only `reached`, but REQ-1 is also... no: tag a second function
    let mut ev = evidence();
    ev.functions[1].requirements = vec!["REQ-1".into()];
    let r = compare_own(&ev, &reqs(vec![req("REQ-1", "implemented", &["demo.c::reached"], &[])]));
    assert!(kinds(&r).contains(&"tagged-not-claimed"), "{:#?}", r.disagreements);
}

/// `implemented` with nothing behind it. Distinct from `planned`, which is
/// expected to have none -- and in the corpus this was built against, 212 of
/// 304 are planned, so conflating them would bury this.
#[test]
fn implemented_with_no_code_is_reported_but_planned_is_not() {
    let r = compare_own(&evidence(), &reqs(vec![req("REQ-9", "implemented", &[], &[])]));
    assert_eq!(kinds(&r), ["implemented-no-code"]);

    let r = compare_own(&evidence(), &reqs(vec![req("REQ-9", "planned", &[], &[])]));
    assert!(r.is_empty(), "a planned requirement with no code is expected: {:#?}", r.disagreements);
    assert_eq!(r.unclaimed, ["REQ-9"], "and is counted as claiming nothing, not as agreeing");
}

/// The other direction: the source cites an ID the document does not hold.
/// Every other kind starts from the document, so without this a tag for a
/// misspelt or retired requirement was compared with nothing and passed --
/// code claiming a requirement nobody wrote (#325). One finding per tagged
/// function, after the document's own.
#[test]
fn a_tag_citing_an_id_the_document_does_not_hold_is_reported() {
    let mut ev = evidence();
    ev.functions[1].line_requirements.insert(2, vec!["REQ-TYPO".into()]);
    ev.functions[0].requirements.push("REQ-TYPO".into());
    let r = compare(&ev, &reqs(vec![req("REQ-2", "implemented", &["demo.c::unreached"], &[])]));
    let found: Vec<(&str, &str, &str)> = r.disagreements.iter().map(|d| (d.kind.as_str(), d.requirement.as_str(), d.measured.as_str())).collect();
    assert_eq!(
        found,
        [
            ("tag-not-in-document", "REQ-1", "`reached` is tagged with it in the source"),
            ("tag-not-in-document", "REQ-TYPO", "`reached` is tagged with it in the source"),
            ("tag-not-in-document", "REQ-TYPO", "`unreached` is tagged with it in the source"),
        ],
        "{:#?}",
        r.disagreements
    );
    assert_eq!(r.disagreements[0].claimed, "not in the requirements document");
    assert!(!r.is_empty(), "a disagreement like any other, so the gate fails on it");
    assert_eq!(r.agreed, ["REQ-2"], "and the requirement the document does hold still agrees");

    // a document holding every ID the source cites has nothing of this kind
    let r = compare(&evidence(), &reqs(vec![req("REQ-1", "implemented", &["demo.c::reached"], &[]), req("REQ-2", "implemented", &["demo.c::unreached"], &[])]));
    assert!(r.is_empty(), "{:#?}", r.disagreements);
}

/// Agreement is stated as plainly as disagreement, or a check that never
/// looked reads like a check that found nothing.
#[test]
fn a_claim_the_measurement_bears_out_is_recorded_as_agreement() {
    let r = compare_own(&evidence(), &reqs(vec![req("REQ-1", "implemented", &["demo.c::reached"], &["tests.c::t_one"])]));
    assert!(r.is_empty(), "{:#?}", r.disagreements);
    assert_eq!(r.agreed, ["REQ-1"]);
}

/// A near-miss must not read as agreement: reporting an unverified
/// requirement as traced is worse than a false alarm.
#[test]
fn matching_is_exact_and_a_near_miss_does_not_pass() {
    // `ached` is a suffix of `reached`, and `emo.c` of `demo.c`. Neither may
    // resolve: a near-miss that resolved would report an unverified
    // requirement as traced, which is worse than any false alarm.
    let r = compare_own(&evidence(), &reqs(vec![req("REQ-1", "implemented", &["demo.c::ached"], &[])]));
    assert!(kinds(&r).contains(&"code-not-found"), "a partial symbol is not a match: {:#?}", r.disagreements);
    assert!(r.agreed.is_empty(), "and certainly not agreement");

    let r = compare_own(&evidence(), &reqs(vec![req("REQ-1", "implemented", &["emo.c::reached"], &[])]));
    assert!(kinds(&r).contains(&"code-not-found"), "a partial path component is not a match: {:#?}", r.disagreements);
    assert!(r.agreed.is_empty());
}

/// The document's path may be longer than the evidence's display path, which
/// is the ordinary case: `src/limits.c` against `limits.c`.
#[test]
fn a_longer_claim_path_still_matches_on_whole_components() {
    let r = compare_own(&evidence(), &reqs(vec![req("REQ-1", "implemented", &["src/demo.c::reached"], &[])]));
    assert!(r.is_empty(), "{:#?}", r.disagreements);
}

/// Without per-test data a test claim cannot be checked, and inventing a
/// disagreement would be as wrong as inventing an agreement.
#[test]
fn without_per_test_data_a_test_claim_is_not_judged() {
    let mut ev = evidence();
    ev.tests.clear();
    ev.functions[0].tests.clear();
    let r = compare_own(&ev, &reqs(vec![req("REQ-1", "implemented", &["demo.c::reached"], &["tests.c::t_one"])]));
    assert!(r.is_empty(), "no per-test data means no verdict: {:#?}", r.disagreements);
}

/// A tag on one line claims that line. A test that enters the function
/// elsewhere and never executes the tagged line has verified nothing --
/// counting it would report an unverified requirement as verified, the same
/// inversion #42 fixed in the matrix (review of #57).
#[test]
fn a_test_that_enters_the_function_but_misses_the_tagged_line_is_not_verification() {
    let f0 = common::file(&["a();", "b();"], &[Some(1), Some(1)]);
    let mut fun = common::function(0, "two_liner", 1, 2);
    // only line 2 implements it
    fun.line_requirements.insert(2, vec!["REQ-LINE".into()]);
    let mut ev = common::evidence(vec![f0], vec![fun]);
    ev.files[0].function_ids = vec![0];
    // the test enters the function, but only reaches line 1
    let mut lines = BTreeMap::new();
    lines.insert(0usize, vec![(1u32, 1u32)]);
    ev.tests = vec![TestRun { id: 0, name: "t".into(), path: "t.json".into(), functions: vec![0], lines }];
    ev.functions[0].tests = vec![0];

    let r = compare_own(&ev, &reqs(vec![req("REQ-LINE", "implemented", &["demo.c::two_liner"], &["tests.c::t"])]));
    assert!(kinds(&r).contains(&"test-does-not-reach"), "entering the function is not verifying the line: {:#?}", r.disagreements);
    assert!(r.agreed.is_empty(), "and certainly not agreement");
}

/// The same fixture, with the test actually reaching the tagged line.
#[test]
fn a_test_that_executes_the_tagged_line_does_verify_it() {
    let f0 = common::file(&["a();", "b();"], &[Some(1), Some(1)]);
    let mut fun = common::function(0, "two_liner", 1, 2);
    fun.line_requirements.insert(2, vec!["REQ-LINE".into()]);
    let mut ev = common::evidence(vec![f0], vec![fun]);
    ev.files[0].function_ids = vec![0];
    let mut lines = BTreeMap::new();
    lines.insert(0usize, vec![(2u32, 2u32)]); // reaches line 2
    ev.tests = vec![TestRun { id: 0, name: "t".into(), path: "t.json".into(), functions: vec![0], lines }];
    ev.functions[0].tests = vec![0];

    let r = compare_own(&ev, &reqs(vec![req("REQ-LINE", "implemented", &["demo.c::two_liner"], &["tests.c::t"])]));
    assert!(r.is_empty(), "{:#?}", r.disagreements);
    assert_eq!(r.agreed, ["REQ-LINE"]);
}

/// A namespaced symbol keeps its namespace. Splitting the claim at the last
/// `::` read `demo.cpp::ns::run` as `run` inside a file called
/// `demo.cpp::ns`, and rejected a claim that was correct.
#[test]
fn a_namespaced_claim_resolves_to_its_namespaced_function() {
    let f0 = common::file(&["a();"], &[Some(1)]);
    let mut fun = common::function(0, "ns::run", 1, 1);
    fun.requirements = vec!["REQ-NS".into()];
    let mut ev = common::evidence(vec![f0], vec![fun]);
    ev.files[0].function_ids = vec![0];

    let r = compare_own(&ev, &reqs(vec![req("REQ-NS", "implemented", &["demo.c::ns::run"], &[])]));
    assert!(r.is_empty(), "the qualified claim resolves: {:#?}", r.disagreements);
    assert_eq!(r.agreed, ["REQ-NS"]);
}

/// ...and an under-qualified one does not quietly match it. This was the
/// false agreement: `demo.c::run` matching an evidence `ns::run`.
#[test]
fn an_under_qualified_claim_does_not_match_a_namespaced_function() {
    let f0 = common::file(&["a();"], &[Some(1)]);
    let mut fun = common::function(0, "ns::run", 1, 1);
    fun.requirements = vec!["REQ-NS".into()];
    let mut ev = common::evidence(vec![f0], vec![fun]);
    ev.files[0].function_ids = vec![0];

    let r = compare_own(&ev, &reqs(vec![req("REQ-NS", "implemented", &["demo.c::run"], &[])]));
    assert!(kinds(&r).contains(&"code-not-found"), "an under-qualified claim is not a match: {:#?}", r.disagreements);
    assert!(r.agreed.is_empty(), "and must never read as agreement");
}

/// The `file.c:name` encoding for a file-scoped static still resolves --
/// that specific form is what the looser rule was there for.
#[test]
fn the_static_encoding_still_resolves() {
    let f0 = common::file(&["a();"], &[Some(1)]);
    let mut fun = common::function(0, "demo.c:helper", 1, 1);
    fun.requirements = vec!["REQ-S".into()];
    let mut ev = common::evidence(vec![f0], vec![fun]);
    ev.files[0].function_ids = vec![0];

    let r = compare_own(&ev, &reqs(vec![req("REQ-S", "implemented", &["demo.c::helper"], &[])]));
    assert!(r.is_empty(), "{:#?}", r.disagreements);
}

/// The defect #233 reports: a test claim the evidence cannot check counted
/// as agreement, with nothing in the report to say otherwise.
///
/// `without_per_test_data_a_test_claim_is_not_judged` above pins the other
/// half -- that no disagreement is invented. Both halves matter, and only
/// together do they say what the tool established: nothing, about this
/// claim. A gate reading `agreed` was being told the opposite.
#[test]
fn a_test_claim_that_could_not_be_checked_is_not_agreement() {
    let mut ev = evidence();
    ev.tests.clear();
    ev.functions[0].tests.clear();
    let r = compare_own(&ev, &reqs(vec![req("REQ-1", "implemented", &["demo.c::reached"], &["tests.c::t_one"])]));

    assert!(r.is_empty(), "still no invented disagreement: {:#?}", r.disagreements);
    assert!(r.agreed.is_empty(), "a claim nobody could look at is not agreement (#233): {:?}", r.agreed);
    assert!(r.unclaimed.is_empty(), "the document did make a claim, so it is not `unclaimed` either: {:?}", r.unclaimed);

    assert_eq!(r.not_checked.len(), 1, "{:#?}", r.not_checked);
    assert_eq!(r.not_checked[0].kind, "no-per-test-data");
    assert_eq!(r.not_checked[0].requirement, "REQ-1");
    assert_eq!(r.not_checked[0].claimed, "tests.c::t_one", "the claim is echoed in the document's own words");
    assert!(r.not_checked[0].reason.contains("--tests"), "and the reason names what would let it be checked: {}", r.not_checked[0].reason);
    assert_eq!(r.not_checked_requirements(), ["REQ-1"]);
}

/// The code half of the same requirement really was checked and really did
/// agree. Suppressing the whole requirement from `agreed` is deliberate --
/// `agreed` is documented as "whose *every* claim agreed" -- and the
/// evidence that the code claim was examined is that a wrong one still
/// disagrees here, rather than disappearing into `not_checked`.
#[test]
fn an_unchecked_test_claim_does_not_mask_a_wrong_code_claim() {
    let mut ev = evidence();
    ev.tests.clear();
    ev.functions[0].tests.clear();
    let r = compare_own(&ev, &reqs(vec![req("REQ-1", "implemented", &["demo.c::renamed_away"], &["tests.c::t_one"])]));

    assert!(kinds(&r).contains(&"code-not-found"), "the code claim is still checked: {:#?}", r.disagreements);
    assert_eq!(r.not_checked.len(), 1, "and the test claim is still reported unchecked: {:#?}", r.not_checked);
    assert!(r.agreed.is_empty());
}

/// The second way a test claim goes unchecked, which #233 does not mention:
/// per-test data is present and the named test really ran, but nothing
/// locates the requirement in the code -- no tag in the evidence, no code
/// claim in the document -- so the reach check has no lines to ask about and
/// is skipped.
///
/// With a status other than `implemented` no `implemented-no-code`
/// disagreement fires either, so before this fix the requirement fell
/// straight through to `agreed`: a second false agreement by the same
/// mechanism, reached without the missing-per-test-data precondition the
/// issue describes.
#[test]
fn a_test_claim_with_nothing_to_scope_it_is_not_agreement_either() {
    let r = compare_own(&evidence(), &reqs(vec![req("REQ-NOSCOPE", "planned", &[], &["tests.c::t_one"])]));

    assert!(r.is_empty(), "nothing to disagree with: {:#?}", r.disagreements);
    assert!(r.agreed.is_empty(), "the reach check never ran, so this is not agreement (#233): {:?}", r.agreed);
    assert_eq!(r.not_checked.len(), 1, "{:#?}", r.not_checked);
    assert_eq!(r.not_checked[0].kind, "claim-has-no-scope");
    assert!(r.not_checked[0].reason.contains("no tag"), "{}", r.not_checked[0].reason);
}

/// A claim the measurement really did answer stays agreement, and carries no
/// unchecked marker. Without this the fix could be "mark everything
/// unchecked", which would be as useless as marking nothing.
#[test]
fn a_claim_the_measurement_could_answer_is_still_agreement() {
    let r = compare_own(&evidence(), &reqs(vec![req("REQ-1", "implemented", &["demo.c::reached"], &["tests.c::t_one"])]));
    assert!(r.is_empty(), "{:#?}", r.disagreements);
    assert_eq!(r.agreed, ["REQ-1"]);
    assert!(r.not_checked.is_empty(), "this one was checked: {:#?}", r.not_checked);
}

/// `--json` must carry the distinction, which is the half of #233 that no
/// terminal note could cover: a CI gate reads this and nothing else.
#[test]
fn the_json_report_distinguishes_checked_agreement_from_an_unchecked_claim() {
    let mut ev = evidence();
    ev.tests.clear();
    ev.functions[0].tests.clear();
    let r = compare_own(&ev, &reqs(vec![req("REQ-1", "implemented", &["demo.c::reached"], &["tests.c::t_one"])]));

    let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(v["agreed"].as_array().unwrap().len(), 0, "{v}");
    assert_eq!(v["notChecked"][0]["kind"], "no-per-test-data", "{v}");
    assert_eq!(v["notChecked"][0]["requirement"], "REQ-1", "{v}");
    assert!(v["notChecked"][0]["reason"].is_string(), "{v}");

    // and a report written before the field existed still loads
    let old: butterfly_model::claimcheck::Report = serde_json::from_str(r#"{"disagreements":[],"agreed":["X"],"unclaimed":[]}"#).expect("older reports still deserialise");
    assert!(old.not_checked.is_empty());
}
