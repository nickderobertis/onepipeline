//! The release probe, against the registries themselves.
//!
//! `scripts/release-probe.sh` is what a consuming run asks before launching work
//! that depends on a release of this repository, and its three answers are driven
//! offline in `npm/test/release-targets.test.mjs` against a fixture registry over
//! real HTTP. What no fixture can stand in for is what crates.io, PyPI and npm
//! actually serve — their endpoints, their document shapes, and whether a release
//! is there at all — and a registry standing in for itself is what would then be
//! under test. So that half is asked here, in the tier that is not offline.
//!
//! It needs no credential, and that is the property rather than an omission: the
//! release-target contract spawns a probe with `PATH` and `HOME` and nothing
//! else, precisely so a repository being waited on cannot make a consumer's hold
//! depend on a secret. Every target is on a public registry, where an
//! unauthenticated read is all a probe needs and all it may have — so this runs
//! the script under exactly that environment, and a probe that had come to need
//! more fails here.
//!
//! Like everything else in this binary it **refuses rather than skips**: a
//! registry that cannot be reached, a probe that could not answer, and a declared
//! target no registry serves are each a failure naming what to do about it.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use onevcs::read_release_declaration;

/// The sixty seconds the release-target contract allows one answer.
const BOUND: Duration = Duration::from_secs(60);

/// The repository root, which is the working directory the contract spawns a
/// probe in.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every artifact this repository publishes, read out of `release-targets.toml`
/// rather than listed again here — and read with the canonical reader, so this
/// asks the registries about exactly the identifiers a consumer would.
fn declared_targets() -> Vec<String> {
    let declared = read_release_declaration(&repo_root()).expect(
        "release-targets.toml is checked in at this repository's root and reads as the \
         canonical schema; tests/release_targets.rs is the offline check on that",
    );
    assert!(
        !declared.targets.is_empty(),
        "release-targets.toml declares no [[target]], so a consumer waiting on a release of \
         this repository is told nothing"
    );
    declared
        .targets
        .iter()
        .map(|target| target.id.to_string())
        .collect()
}

fn said(run: &Output) -> String {
    format!(
        "exit {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    )
}

/// Every declared target is answered by the registry it names, with the version
/// that registry serves right now.
///
/// The probe is run exactly as `src/release.rs` runs one: the file itself as a
/// direct subprocess with no shell interposed, one argument, this repository's
/// root as the working directory, and an environment cleared down to a search
/// path and a home directory.
#[test]
fn every_declared_target_is_answered_by_the_registry_it_names() {
    for target in declared_targets() {
        let mut probe = Command::new(repo_root().join("scripts/release-probe.sh"));
        probe.arg(&target).current_dir(repo_root()).env_clear();
        for name in ["PATH", "HOME", "SystemRoot", "USERPROFILE"] {
            if let Some(value) = std::env::var_os(name) {
                probe.env(name, value);
            }
        }

        let started = Instant::now();
        let run = probe.output().unwrap_or_else(|error| {
            panic!(
                "scripts/release-probe.sh did not start as a direct subprocess ({error}), which \
                 is the only way a host runs one"
            )
        });
        let waited = started.elapsed();

        assert!(
            run.status.success(),
            "the probe could not answer for `{target}`, which is never evidence that a release \
             has not happened — a consumer holds indefinitely on it. Its reason is below; check \
             that this host can reach the public registries.\n{}",
            said(&run)
        );
        let answer = String::from_utf8_lossy(&run.stdout);
        assert!(
            !answer.is_empty(),
            "`{target}` is a declared release target, but its registry serves no release of it. \
             Check that .github/workflows/release.yml still publishes it and that nothing has \
             been yanked or unpublished; a consumer waiting for it waits forever.\n{}",
            said(&run)
        );
        let version = answer.strip_suffix('\n').unwrap_or_else(|| {
            panic!(
                "the probe answered for `{target}` without ending its line, and a caller reads \
                 one line:\n{}",
                said(&run)
            )
        });
        assert!(
            !version.contains('\n')
                && version.starts_with(|c: char| c.is_ascii_digit())
                && version.contains('.'),
            "the probe answered `{version}` for `{target}`, which is not a version a consumer \
             can hold a node against:\n{}",
            said(&run)
        );
        assert!(
            waited < BOUND,
            "the probe took {waited:?} to answer for `{target}`, past the sixty seconds the \
             release-target contract allows"
        );
        println!("{target} -> {version} in {waited:?}");
    }
}
