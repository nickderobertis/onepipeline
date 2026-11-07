//! A world holding one private and one public repository, as its rules declare them.

use crate::harness::{Repository, World};

/// A world with two registered repositories: `service`, which the rules declare
/// private, and `library`, which they declare public.
pub(crate) fn mixed_world(name: &str) -> (World, Repository, Repository) {
    let world = World::new(name);
    let private = world.repository("local-direct", &[]);
    let public = world.extra_repository("library");
    declare_visibility(&world, &[("service", "private"), ("library", "public")]);
    (world, private, public)
}

/// Rewrite this host's rules file at version 4, declaring each named repository of
/// owner `owner` public or private, everything else publishing `local-direct`.
pub(crate) fn declare_visibility(world: &World, declared: &[(&str, &str)]) {
    let rules: String = declared
        .iter()
        .map(|(name, visibility)| {
            format!("  - match: {{owner: owner, name: {name}}}\n    visibility: {visibility}\n")
        })
        .collect();
    std::fs::write(
        world.onevcs_home().join("rules.yml"),
        format!(
            "version: 4\nrules:\n{rules}  - match: {{owner: owner}}\n    visibility: private\n\
             default:\n  publication: local-direct\n  approvals: none\n"
        ),
    )
    .expect("the rules file is written");
}

/// Commit the synthetic replacement declaration the linked boundary reads from the repository.
pub(crate) fn declare_replacement(world: &World, repository: &Repository) {
    std::fs::write(
        repository.checkout.join("private-terms.toml"),
        "schema_version = 2\nterms = [\"velvetquasar\"]\nreplacements = [{term = \"velvetquasar\", with = \"sample\"}]\n",
    ).expect("the declaration is written");
    crate::harness::git(world, &repository.checkout, &["add", "-A"]);
    crate::harness::git(
        world,
        &repository.checkout,
        &["commit", "-qm", "chore: declare replacement"],
    );
    crate::harness::git(world, &repository.checkout, &["push", "origin", "main"]);
}
