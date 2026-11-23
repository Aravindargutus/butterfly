// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Replacing the resolutions sidecar without destroying it.
//!
//! The sidecar is a reviewer's record: which gaps were justified, by whom,
//! against which requirement. Writing it with `fs::write` truncates in place,
//! so an interrupted write leaves a half-document where a signed-off review
//! used to be, and a symlink at the destination is followed -- letting anyone
//! who can plant that link redirect the write onto a file butterfly can write
//! (#2).
//!
//! So: write a uniquely named temporary file beside the destination, flush it
//! to disk, and rename it over the top. Rename replaces the destination entry
//! itself, which is what makes it both atomic -- a reader sees the old
//! document or the new one, never a partial one -- and symlink-proof: a link
//! is replaced rather than written through.
//!
//! The temporary file has to be created the same way. `File::create` follows a
//! symlink, so a predictable temporary name could simply be preplanted as a
//! link to whatever butterfly may write, and the next update truncated the
//! victim before the rename ever happened -- the same arbitrary overwrite,
//! one file to the left (review of #22).
//!
//! The writer itself now lives in `butterfly_ingest::fsauth`, so the change
//! cache and every other output go through the same one (#248).

pub use butterfly_ingest::fsauth::{write_atomic, Wrote};
use std::path::{Path, PathBuf};

/// Where one variant's resolutions live, in a multi-variant session.
///
/// `safe_path_component` is not injective, and its result named the file a
/// reviewer's decisions are written to. `a/b` and `a_b` both became `a_b`;
/// anything past 64 characters kept only the prefix; `.` and `...` both
/// became `unnamed`. Two variants could therefore overwrite each other's
/// record, and a hostile evidence file could aim at another variant's
/// sidecar deliberately (#6).
///
/// The readable part is kept -- `O0` is still `O0` at a glance -- and a
/// digest of the *original* name is appended, so distinct names cannot
/// produce the same file. The caller still checks the derived paths for
/// collisions: the digest makes one vanishingly unlikely, and the check is
/// what makes it impossible.
pub fn path_for_variant(base: &Path, variant: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "resolutions".into());
    // 48 of the readable name, so a long one still leaves the tag visible
    let readable: String = butterfly_model::safe_path_component(variant).chars().take(48).collect();
    let tag: String = Sha256::digest(variant.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect();
    base.with_file_name(format!("{stem}-{readable}-{tag}.json"))
}

/// What that path used to be, so a session started before #6 can be told
/// where its recorded decisions are rather than silently getting none.
pub fn legacy_path_for_variant(base: &Path, variant: &str) -> PathBuf {
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "resolutions".into());
    base.with_file_name(format!("{stem}-{}.json", butterfly_model::safe_path_component(variant)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `safe_path_component` maps many names to one, and its result named
    /// the file a reviewer's decisions go in. Each of these pairs shared a
    /// sidecar, so one variant's record overwrote the other's (#6).
    #[test]
    fn distinct_variant_names_get_distinct_sidecars() {
        let base = Path::new("/p/resolutions.json");
        let long_a = format!("{}A", "x".repeat(64));
        let long_b = format!("{}B", "x".repeat(64));
        for (a, b, why) in [
            ("a/b", "a_b", "a separator and the character it sanitises to"),
            ("a b", "a_b", "a space and the character it sanitises to"),
            ("a:b", "a/b", "two different separators"),
            (long_a.as_str(), long_b.as_str(), "names differing only past the 64-character cut"),
            (".", "...", "names that both sanitise away to `unnamed`"),
        ] {
            // the pair really did collide, or the case proves nothing
            assert_eq!(legacy_path_for_variant(base, a), legacy_path_for_variant(base, b), "{why}: these are supposed to have collided");
            assert_ne!(path_for_variant(base, a), path_for_variant(base, b), "{why}");
        }
    }

    /// The name still has to be legible in a directory listing, and it still
    /// has to be a single component beside the base sidecar.
    #[test]
    fn a_variant_sidecar_stays_readable_and_stays_one_component() {
        let base = Path::new("/p/resolutions.json");
        let p = path_for_variant(base, "O0");
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("resolutions-O0-"), "the variant is still visible: {name}");
        assert!(name.ends_with(".json"), "{name}");
        assert_eq!(p.parent(), base.parent(), "it stays beside the base sidecar");

        // a name from a hostile evidence file cannot climb out of the
        // directory or become more than one component
        for hostile in ["../../etc/passwd", "/etc/passwd", "..", "a/b/c"] {
            let q = path_for_variant(base, hostile);
            assert_eq!(q.parent(), base.parent(), "{hostile}: {q:?}");
            assert_eq!(q.components().count(), base.components().count(), "{hostile}: {q:?}");
            assert!(!q.file_name().unwrap().to_string_lossy().contains('/'), "{hostile}");
        }
    }

    /// Same name, same file, run after run -- a restart that chose a
    /// different one would look like a reviewer's decisions had vanished.
    #[test]
    fn the_same_variant_name_always_gets_the_same_sidecar() {
        let base = Path::new("/p/resolutions.json");
        assert_eq!(path_for_variant(base, "O0"), path_for_variant(base, "O0"));
        // the base's stem is carried through, the rest of the name is not
        let a = path_for_variant(base, "O0").file_name().unwrap().to_string_lossy().into_owned();
        let b = path_for_variant(Path::new("/p/other.json"), "O0").file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(a.strip_prefix("resolutions"), b.strip_prefix("other"), "only the stem differs: {a} vs {b}");
    }
}
