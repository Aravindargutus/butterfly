// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! The `BUTTERFLY_MAX_*_MB` overrides (area A). One test in its own binary,
//! because it sets process environment variables.

use butterfly_model::limits::{Kind, Limits};

const MIB: u64 = 1 << 20;

/// README.md:1594-1596 "raise any with `BUTTERFLY_MAX_SOURCE_MB`,
/// `BUTTERFLY_MAX_OBJECT_MB`, `BUTTERFLY_MAX_COVERAGE_MB`,
/// `BUTTERFLY_MAX_EVIDENCE_MB`, `BUTTERFLY_MAX_TRACE_MB`,
/// `BUTTERFLY_MAX_AUX_MB` or `BUTTERFLY_MAX_TOTAL_MB`" -- each variable, in
/// MiB, sets its own cap and no other.
#[test]
fn each_variable_sets_its_own_cap_in_mebibytes() {
    let vars = [
        (Kind::Coverage, "BUTTERFLY_MAX_COVERAGE_MB", 3),
        (Kind::Evidence, "BUTTERFLY_MAX_EVIDENCE_MB", 5),
        (Kind::Source, "BUTTERFLY_MAX_SOURCE_MB", 7),
        (Kind::Object, "BUTTERFLY_MAX_OBJECT_MB", 11),
        (Kind::Trace, "BUTTERFLY_MAX_TRACE_MB", 13),
        (Kind::Auxiliary, "BUTTERFLY_MAX_AUX_MB", 17),
    ];
    for (_, v, mb) in vars {
        std::env::set_var(v, mb.to_string());
    }
    std::env::set_var("BUTTERFLY_MAX_TOTAL_MB", "19");
    let l = Limits::from_env();
    for (k, v, mb) in vars {
        assert_eq!(l.cap(k), mb * MIB, "{v}");
    }
    assert_eq!(l.remaining(), 19 * MIB, "BUTTERFLY_MAX_TOTAL_MB");
    for (_, v, _) in vars {
        std::env::remove_var(v);
    }
    std::env::remove_var("BUTTERFLY_MAX_TOTAL_MB");
}
