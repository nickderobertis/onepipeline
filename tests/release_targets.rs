//! The release declaration, held to the canonical schema by the reader that
//! defines it.
//!
//! The schema `release-targets.toml` is written against is not this repository's:
//! it is `onevcs`'s, and six repositories write the same shape so a host-side
//! reader can list what any of them publishes. So this drives that crate's own
//! reader — a second implementation here would be a second opinion about a shape
//! whose whole point is that there is one.
//!
//! The other half, reconciling the document against what this repository actually
//! publishes, is `npm/test/release-targets.test.mjs`.

use std::fs;
use std::path::{Path, PathBuf};

use onevcs::declaration::{FILE, OLDEST_SCHEMA_VERSION, SCHEMA_VERSION};
use onevcs::{read_release_declaration, validate_release_declaration};

/// The repository root, which is where the schema fixes the document.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn document() -> String {
    fs::read_to_string(repo_root().join(FILE))
        .unwrap_or_else(|error| panic!("{FILE} is checked in at this repository's root: {error}"))
}

/// What the canonical reader makes of a document this one is a mutation of.
///
/// Every refusal below starts from the real file and changes one thing, so a
/// refusal is attributable to that change rather than to a fixture written to
/// fail. `the_checked_in_declaration_is_what_the_canonical_reader_accepts` is the
/// control: the same text, unchanged, is accepted.
fn refusal(document: &str) -> String {
    match validate_release_declaration(document, FILE) {
        Ok(_) => {
            panic!("the canonical reader accepted a document it should have refused:\n{document}")
        }
        Err(error) => error.to_string(),
    }
}

/// Exactly one occurrence of `find`, replaced — so a mutation that stopped
/// applying fails here rather than leaving the document unchanged and the
/// refusal below coming from something else.
fn mutate(document: &str, find: &str, replace: &str) -> String {
    assert_eq!(
        document.matches(find).count(),
        1,
        "{FILE} no longer carries `{find}` exactly once, so this journey mutates nothing"
    );
    document.replace(find, replace)
}

/// The document this repository ships is one the canonical reader accepts, whole.
///
/// `read_release_declaration` is the entry point a consumer uses — it takes the
/// repository root and finds the file itself — so this asks it the way a host
/// asks it, rather than reading the bytes here and validating them.
#[test]
fn the_checked_in_declaration_is_what_the_canonical_reader_accepts() {
    let declared = read_release_declaration(&repo_root()).unwrap_or_else(|error| {
        panic!(
            "{FILE} is what this repository publishes, and a consumer that cannot read it stops \
             waiting for a release that is coming: {error}"
        )
    });

    assert_eq!(
        declared.schema_version, SCHEMA_VERSION,
        "{FILE} is written against the schema this build knows"
    );
    assert!(
        !declared.targets.is_empty(),
        "{FILE} declares no [[target]], which a consumer cannot tell from nobody having said \
         anything"
    );
}

/// The linked `onevcs` reads the release-declaration schema this repository
/// writes.
///
/// The floor `onevcs` 0.16.2 holds, and why it is carried by `Cargo.lock` rather
/// than by the requirement, are with the pin in `Cargo.toml`. It is the one
/// engine gap in that refresh that moved a library surface: 0.16.0 knew a single
/// schema version and refused anything a name it did not recognise, and 0.16.2
/// reads a **range** — [`OLDEST_SCHEMA_VERSION`] up to the [`SCHEMA_VERSION`] a
/// producer writes — with npm's scoped `@scope/name` newly expressible in a
/// `target.id`.
///
/// This crate depends on the move because it is a *producer*: the number
/// `release-targets.toml` states is the one the linked build writes, and
/// `the_checked_in_declaration_is_what_the_canonical_reader_accepts` above holds
/// the file to it. Below the floor this does not compile at all —
/// `OLDEST_SCHEMA_VERSION` is not a symbol there — which is why the scoped
/// identifier is asserted through the reader rather than through
/// `declaration::RegistryId`'s own parser: a refusal is what 0.16.0 answers, and
/// a value is what this one does.
#[test]
fn the_linked_onevcs_reads_the_release_declaration_schema_this_repository_writes() {
    assert_eq!(
        (OLDEST_SCHEMA_VERSION, SCHEMA_VERSION),
        (1, 2),
        "the linked onevcs does not read release declarations across the range this \
         repository's own document is written inside: the pair ships in 0.16.2, and \
         `Cargo.toml` requires the newest release, which is above that floor — so \
         `Cargo.lock` is behind the manifest too and `cargo update -p onevcs` is the whole \
         of the fix; `just engines-current` names it without running the suite"
    );

    // A version-1 declaration is still read, which is the half of the pair that
    // keeps a consumer able to read the siblings that have not moved their own
    // document.
    let older = mutate(
        &document(),
        "\nschema_version = 2\n",
        "\nschema_version = 1\n",
    );
    let declared = validate_release_declaration(&older, FILE).unwrap_or_else(|error| {
        panic!(
            "the linked onevcs refuses a schema_version {OLDEST_SCHEMA_VERSION} declaration, so \
             a consumer reading the repositories that have not moved theirs learns nothing \
             about what they publish: {error}"
        )
    });
    assert_eq!(declared.schema_version, OLDEST_SCHEMA_VERSION);

    // And the identifier the version number exists for: npm serves a scoped
    // package, and 0.16.0 refused the spelling outright as a name no registry
    // serves. Driven through the reader, so this asserts what a declaration
    // naming one is *read* as rather than what a parser accepts in isolation.
    let scoped = mutate(
        &document(),
        "id = \"npm:onepipeline-cli\"",
        "id = \"npm:@onepipeline/cli\"",
    );
    let declared = validate_release_declaration(&scoped, FILE).unwrap_or_else(|error| {
        panic!(
            "the linked onevcs refuses `npm:@onepipeline/cli`, a name npm genuinely serves, so \
             a producer here could not declare a scoped package it publishes: {error}"
        )
    });
    assert!(
        declared
            .targets
            .iter()
            .any(|target| target.id.name() == "@onepipeline/cli"),
        "the scoped identifier was read as some other name, so what a consumer would wait on \
         is not what the document declared"
    );
}

/// Every path the declaration names is one this checkout actually carries.
///
/// The schema refuses a path that leaves the repository, which is a check on the
/// spelling; whether the file is *there* is something only this repository can
/// say, and a `probe` or `manifest` naming a file nobody moved with the rest is
/// exactly the drift a host reading this document cannot detect.
#[test]
fn every_path_the_declaration_names_is_in_this_checkout() {
    let declared = read_release_declaration(&repo_root()).expect("the declaration reads");
    let root = repo_root();

    let probe = declared
        .probe
        .as_ref()
        .expect("this repository answers its targets with a script, so it declares a `probe`");
    let probe_path = root.join(probe.as_path());
    assert!(
        probe_path.is_file(),
        "{FILE} names `{}` as its probe, and this checkout has no such file",
        probe.as_path().display()
    );
    assert!(
        is_executable(&probe_path),
        "{} is not executable, so the host cannot spawn it directly",
        probe.as_path().display()
    );

    for target in &declared.targets {
        let manifest = target
            .manifest
            .as_ref()
            .unwrap_or_else(|| panic!("{} is versioned from a manifest in this tree", target.id));
        assert!(
            root.join(manifest.as_path()).is_file(),
            "{} names the manifest `{}`, and this checkout has no such file",
            target.id,
            manifest.as_path().display()
        );
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// A required field dropped is refused, not read as an absent one.
///
/// `what` is the sentence a consumer reads to know what it is waiting for, so a
/// document missing it is one that says less than it looks like it says.
#[test]
fn a_target_missing_a_required_field_is_refused() {
    let text = document();
    let dropped = mutate(
        &text,
        "what = \"The onepipeline library and the `onepipeline` binary, as a Rust dependent takes them with `cargo add onepipeline`.\"\n",
        "",
    );
    let refused = refusal(&dropped);
    assert!(
        refused.contains("missing field `what`"),
        "the refusal does not name the field that is missing: {refused}"
    );
}

/// An identifier that is not `<registry>:<name>` is refused.
///
/// The qualification is the whole point of the identifier here: this repository
/// serves `onepipeline-cli` from two registries, so a bare name is two artifacts
/// and a consumer waiting on it cannot say which one it got.
#[test]
fn an_identifier_that_is_not_registry_qualified_is_refused() {
    let text = document();
    let unqualified = mutate(
        &text,
        "id = \"pypi:onepipeline-cli\"",
        "id = \"onepipeline-cli\"",
    );
    let refused = refusal(&unqualified);
    assert!(
        refused.contains("\"onepipeline-cli\" names no registry"),
        "the refusal does not name the identifier it refused, or why: {refused}"
    );
}

/// Two targets taking one short name is refused rather than one silently winning.
///
/// The short name is what a host document and a plan node's `consumes` map wait
/// on, so a document where two artifacts answer to one name is one where the
/// artifact a consumer gets depends on which entry the reader kept.
#[test]
fn two_targets_taking_one_short_name_are_refused() {
    let text = document();
    let repeated = mutate(&text, "name = \"pypi\"", "name = \"crate\"");
    let refused = refusal(&repeated);
    assert!(
        refused.contains("taking the short name \"crate\""),
        "the refusal does not name the short name that is taken twice: {refused}"
    );
}

/// A key this schema does not declare is refused *by name*.
///
/// A typo is the likeliest defect in a hand-written document, and reading
/// `manifset` as an absent `manifest` publishes an answer nobody declared.
#[test]
fn a_key_this_schema_does_not_declare_is_refused_by_name() {
    let text = document();
    let typo = mutate(
        &text,
        "manifest = \"pyproject.toml\"",
        "manifset = \"pyproject.toml\"",
    );
    let refused = refusal(&typo);
    assert!(
        refused.contains("names \"manifset\""),
        "the refusal does not name the key it did not recognise: {refused}"
    );
}

/// A repository carrying no declaration is refused, not answered with an empty one.
///
/// "This repository publishes nothing" and "nobody has said what this repository
/// publishes" are different answers, and a consumer holding a node on a release
/// acts differently on each. Driven here because it is the answer a host gets for
/// this repository the day somebody deletes the file.
#[test]
fn a_root_with_no_declaration_is_refused_rather_than_read_as_publishing_nothing() {
    let empty =
        std::env::temp_dir().join(format!("onepipeline-no-declaration-{}", std::process::id()));
    fs::create_dir_all(&empty).expect("a scratch directory standing in for a repository root");
    let answer = read_release_declaration(&empty);
    fs::remove_dir_all(&empty).ok();
    assert!(
        answer.is_err(),
        "a root with no {FILE} was answered as a repository that declares nothing"
    );
}
