//! The unreleased revision this tree links is named in two files, and they agree.
//!
//! `Cargo.toml`'s `[patch.crates-io]` decides what is *linked*; `deny.toml`'s
//! `allow-git` decides what the audit will *permit*. Nothing derives one from the
//! other, so a revision moved in the manifest and left behind in the audit fails
//! `just deps-check` — a recipe outside the deterministic gate, which needs the
//! network, and which therefore reports the mistake long after it was made and
//! nowhere near the change that made it.
//!
//! This is the drift gate. It reads both real files and holds the two sets equal,
//! including when both are empty: the block is deleted in the same change that
//! moves the requirements to the release, and this then insists the audit's
//! exemption goes with it rather than outliving the reason for it.

use std::path::Path;

/// Every `rev = "…"` under `[patch.crates-io]`, in the order the manifest states
/// them.
///
/// Read to the next table header, so a `rev` belonging to some other section is
/// not counted as a patched one.
fn patched_revisions(manifest: &str) -> Vec<String> {
    manifest
        .lines()
        .skip_while(|line| line.trim() != "[patch.crates-io]")
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with('['))
        .filter_map(|line| quoted_after(line, "rev = \""))
        .collect()
}

/// Every revision `deny.toml`'s `allow-git` permits, in the order it lists them.
///
/// One entry is one `?rev=` in a git URL. An entry naming a repository and no
/// revision permits everything that repository ever carries, which is not a pin at
/// all — it yields nothing here and so is reported as drift.
fn allowed_revisions(deny: &str) -> Vec<String> {
    let Some((_, rest)) = deny.split_once("allow-git") else {
        return Vec::new();
    };
    let list = rest.split_once(']').map(|(list, _)| list).unwrap_or(rest);
    list.split('"')
        .filter_map(|value| value.split_once("?rev=").map(|(_, rev)| rev.to_string()))
        .collect()
}

/// The value of a `"`-quoted assignment on one line, if the line makes it.
fn quoted_after(line: &str, key: &str) -> Option<String> {
    let (_, rest) = line.split_once(key)?;
    let (value, _) = rest.split_once('"')?;
    Some(value.to_string())
}

#[test]
fn the_audit_permits_exactly_the_revisions_the_manifest_patches() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("the manifest reads");
    let deny = std::fs::read_to_string(root.join("deny.toml")).expect("the audit config reads");

    let patched = patched_revisions(&manifest);
    let allowed = allowed_revisions(&deny);

    let mut wanted = patched.clone();
    wanted.sort();
    wanted.dedup();
    let mut permitted = allowed.clone();
    permitted.sort();
    permitted.dedup();

    assert_eq!(
        wanted, permitted,
        "`deny.toml`'s allow-git permits {allowed:?} and `Cargo.toml`'s [patch.crates-io] links \
         {patched:?}; move both in one change, and delete both together"
    );
}

/// A patch block linking one sibling twice must link it at **one** revision: two
/// copies of a crate in the graph is a compile error, and the manifest's own
/// comment says so.
#[test]
fn every_patched_member_of_one_sibling_comes_from_one_revision() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("the manifest reads");

    let mut revisions = patched_revisions(&manifest);
    revisions.sort();
    revisions.dedup();
    assert!(
        revisions.len() <= 1,
        "[patch.crates-io] links more than one revision: {revisions:?}"
    );
}

#[test]
fn a_manifest_that_patches_nothing_wants_an_audit_that_permits_nothing() {
    assert!(patched_revisions("[dependencies]\nserde = \"1\"\n").is_empty());
    assert!(allowed_revisions("[sources]\nallow-git = []\n").is_empty());
}

#[test]
fn a_revision_is_read_out_of_each_files_own_spelling_of_it() {
    let manifest = "[patch.crates-io]\n\
                    onevcs = { git = \"https://example.invalid/x\", rev = \"abc123\" }\n\
                    onevcs-testing = { git = \"https://example.invalid/x\", rev = \"abc123\" }\n\
                    \n[profile.release]\nrev = \"not-a-patch\"\n";
    assert_eq!(patched_revisions(manifest), ["abc123", "abc123"]);

    let deny = "[sources]\nallow-git = [\"https://example.invalid/x?rev=abc123\"]\n";
    assert_eq!(allowed_revisions(deny), ["abc123"]);

    // A repository named with no revision pins nothing, so it is not a revision
    // this reads — which makes it drift against any manifest that patches one.
    let unpinned = "[sources]\nallow-git = [\"https://example.invalid/x\"]\n";
    assert!(allowed_revisions(unpinned).is_empty());
}
