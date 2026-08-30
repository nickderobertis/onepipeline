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
//!
//! # Why both sides are parsed rather than scanned
//!
//! What is compared is a *pin*, and a pin that is not a whole one is the failure
//! this exists to catch. So each side is parsed as the TOML it is and every cell
//! is checked before it is compared: a `rev` that is missing, empty, or not a
//! commit is refused by name rather than compared as a value, and an `allow-git`
//! entry naming a repository with no revision permits everything that repository
//! ever carries — which is the drift, not the agreement.

use std::collections::BTreeSet;
use std::path::Path;

/// One patched source: the repository, and the commit the pin names in it.
type Pin = (String, String);

/// Every entry of `[patch.crates-io]`, as the repository and revision each pins.
///
/// A patch entry this cannot read as a git pin is a refusal rather than an
/// omission: a `path` patch, a `branch` where a `rev` belongs, or a `rev` that is
/// not a commit are each something no `allow-git` entry could correctly mirror, so
/// leaving them out would let the comparison below pass on a manifest nobody
/// checked.
fn patched(manifest: &toml::Value) -> BTreeSet<Pin> {
    let Some(table) = manifest
        .get("patch")
        .and_then(|patch| patch.get("crates-io"))
    else {
        return BTreeSet::new();
    };
    let table = table
        .as_table()
        .expect("[patch.crates-io] is a table of patched crates");
    table
        .iter()
        .map(|(name, entry)| {
            let git = entry
                .get("git")
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| panic!("the patch of `{name}` is not a git source"));
            let rev = entry
                .get("rev")
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| panic!("the patch of `{name}` names no `rev`"));
            (git.to_owned(), commit(rev, name))
        })
        .collect()
}

/// Every entry of `[sources] allow-git`, as the repository and revision each
/// permits.
///
/// An entry with no `?rev=` is refused here rather than skipped, because what it
/// permits is every revision that repository will ever carry — which is not a pin
/// at all, and is exactly the state a reader of this file would misread as one.
fn allowed(deny: &toml::Value) -> BTreeSet<Pin> {
    let Some(list) = deny
        .get("sources")
        .and_then(|sources| sources.get("allow-git"))
    else {
        return BTreeSet::new();
    };
    list.as_array()
        .expect("`allow-git` is a list of permitted git sources")
        .iter()
        .map(|entry| {
            let url = entry
                .as_str()
                .unwrap_or_else(|| panic!("an `allow-git` entry is not a string: {entry}"));
            let (repository, rev) = url
                .split_once("?rev=")
                .unwrap_or_else(|| panic!("the `allow-git` entry `{url}` pins no revision"));
            (repository.to_owned(), commit(rev, url))
        })
        .collect()
}

/// A revision, checked to be one before it is compared with another.
///
/// Cargo and `cargo-deny` both spell a pinned revision as a hexadecimal object
/// name, so anything else — an empty cell, a tag, a branch — is a pin this gate
/// cannot hold and says so about, under the name of whatever carried it.
fn commit(rev: &str, whose: &str) -> String {
    assert!(
        rev.len() >= 7 && rev.chars().all(|c| c.is_ascii_hexdigit()),
        "`{whose}` names `{rev}`, which is not a commit"
    );
    rev.to_owned()
}

/// One file of this repository, parsed.
fn read(name: &str) -> toml::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{name} reads: {error}"));
    toml::from_str(&text).unwrap_or_else(|error| panic!("{name} parses: {error}"))
}

#[test]
fn the_audit_permits_exactly_the_revisions_the_manifest_patches() {
    let patched = patched(&read("Cargo.toml"));
    let allowed = allowed(&read("deny.toml"));
    assert_eq!(
        patched, allowed,
        "`Cargo.toml`'s [patch.crates-io] links {patched:?} and `deny.toml`'s allow-git permits \
         {allowed:?}; move both in one change, and delete both together"
    );
}

/// A patch block linking one sibling twice must link it at **one** revision: two
/// copies of a crate in the graph is a compile error, and the manifest's own
/// comment says so.
#[test]
fn every_patched_member_of_one_repository_comes_from_one_revision() {
    let pins = patched(&read("Cargo.toml"));
    let repositories: BTreeSet<&String> = pins.iter().map(|(repository, _)| repository).collect();
    assert_eq!(
        pins.len(),
        repositories.len(),
        "[patch.crates-io] links one repository at more than one revision: {pins:?}"
    );
}

#[test]
fn a_manifest_that_patches_nothing_wants_an_audit_that_permits_nothing() {
    let manifest: toml::Value = toml::from_str("[dependencies]\nserde = \"1\"\n").expect("parses");
    let deny: toml::Value = toml::from_str("[sources]\nallow-git = []\n").expect("parses");
    assert!(patched(&manifest).is_empty());
    assert!(allowed(&deny).is_empty());
}

#[test]
fn a_pin_is_read_out_of_each_files_own_spelling_of_it() {
    let manifest: toml::Value = toml::from_str(
        "[patch.crates-io]\n\
         onevcs = { git = \"https://example.invalid/x\", rev = \"abc1234\" }\n\
         onevcs-testing = { git = \"https://example.invalid/x\", rev = \"abc1234\" }\n\
         \n[profile.release]\nrev = \"not-a-patch\"\n",
    )
    .expect("parses");
    let deny: toml::Value =
        toml::from_str("[sources]\nallow-git = [\"https://example.invalid/x?rev=abc1234\"]\n")
            .expect("parses");
    assert_eq!(patched(&manifest), allowed(&deny));
}

#[test]
#[should_panic(expected = "pins no revision")]
fn an_allow_git_entry_that_pins_no_revision_is_refused_rather_than_ignored() {
    let deny: toml::Value =
        toml::from_str("[sources]\nallow-git = [\"https://example.invalid/x\"]\n").expect("parses");
    let _ = allowed(&deny);
}

#[test]
#[should_panic(expected = "is not a commit")]
fn a_revision_that_is_not_a_commit_is_refused_rather_than_compared() {
    let manifest: toml::Value = toml::from_str(
        "[patch.crates-io]\nonevcs = { git = \"https://example.invalid/x\", rev = \"\" }\n",
    )
    .expect("parses");
    let _ = patched(&manifest);
}

#[test]
#[should_panic(expected = "names no `rev`")]
fn a_patch_entry_with_no_revision_is_refused_rather_than_ignored() {
    let manifest: toml::Value = toml::from_str(
        "[patch.crates-io]\nonevcs = { git = \"https://example.invalid/x\", branch = \"main\" }\n",
    )
    .expect("parses");
    let _ = patched(&manifest);
}
