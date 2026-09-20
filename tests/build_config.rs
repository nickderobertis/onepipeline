//! The build configuration every cargo invocation inside this clone reads, held
//! to what `.cargo/config.toml` promises.
//!
//! That file carries two keys, and each is a promise about *every* build under
//! this clone rather than about the workspace: `build.target-dir` puts a crate
//! outside the workspace into the same `<clone>/target` the members use, so one
//! clone never compiles its dependency graph into two trees, and
//! `profile.dev.debug = 1` keeps line tables and nothing more on every dev and
//! test build. Neither can be read off the manifest — a `Cargo.toml` profile
//! reaches only its own workspace — and neither is proven by the file parsing,
//! because cargo merges config files from the working directory *upward* and
//! resolves each `target-dir` against its own file's location, which is exactly
//! where a copy of the file goes wrong.
//!
//! So these ask cargo. A probe crate is written under `target/build-config/`,
//! outside the workspace and inside the clone, and what is asserted is what cargo
//! reports for it and what it hands `rustc` when it builds it. The overrides the
//! environment may carry (`CARGO_TARGET_DIR`, `RUSTFLAGS`) are stripped from the
//! probe's environment on purpose: each outranks the file, and what is under
//! test is the file.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The repository root: `CARGO_MANIFEST_DIR` is the root crate's directory.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The one build configuration file under test, read from the tree as committed.
fn build_config() -> String {
    let path = repo_root().join(".cargo/config.toml");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{} reads: {error}", path.display()))
}

/// A crate that is inside the clone but outside the workspace: its own
/// `[workspace]` table stops cargo from folding it into the root's, which is the
/// case a manifest-declared profile could never reach.
///
/// Written fresh on every call, and under `target/` for the reason
/// `linked_engines` puts its fixtures there: a failed run leaves the tree that
/// failed where the other build artifacts are.
fn probe_crate(case: &str) -> PathBuf {
    let root = repo_root().join(format!("target/build-config/{case}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).expect("a probe crate directory");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"build-config-probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .expect("a probe manifest");
    fs::write(root.join("src/lib.rs"), "pub fn probe() {}\n").expect("a probe crate");
    root
}

/// Run cargo from `dir` with everything that outranks the config file removed.
///
/// `CARGO` is the cargo running this test, as the e2e harness uses it, so the
/// probe is built by the same toolchain the tree is.
fn cargo<I, S>(dir: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.args(args).current_dir(dir);
    for outranking in [
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_PROFILE_DEV_DEBUG",
        "CARGO_PROFILE_TEST_DEBUG",
        "CARGO_PROFILE_RELEASE_DEBUG",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
    ] {
        command.env_remove(outranking);
    }
    let output = command.output().expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// What cargo reports as the target directory for the crate in `dir`.
fn reported_target_directory(dir: &Path) -> PathBuf {
    let output = cargo(dir, ["metadata", "--format-version", "1", "--no-deps"]);
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata is JSON");
    PathBuf::from(
        metadata["target_directory"]
            .as_str()
            .expect("cargo metadata reports target_directory"),
    )
}

/// The `rustc` command line a verbose build of the probe crate handed rustc,
/// chosen by `marker` where one build compiles the crate twice.
///
/// Cargo prints it only when it compiles, so a probe cargo judged fresh fails
/// here rather than passing on a line from an earlier run.
fn rustc_invocation(output: &Output, marker: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr
        .lines()
        .find(|line| {
            line.contains("Running")
                && line.contains("--crate-name build_config_probe")
                && line.contains(marker)
        })
        .unwrap_or_else(|| {
            panic!("no rustc invocation for the probe crate ({marker}) in:\n{stderr}")
        })
        .to_owned()
}

#[test]
fn the_config_file_carries_the_contracts_two_keys_and_nothing_that_moves_release() {
    let config: toml::Value = toml::from_str(&build_config()).expect(".cargo/config.toml parses");
    assert_eq!(
        config["build"]["target-dir"].as_str(),
        Some("target"),
        "build.target-dir is the string \"target\""
    );
    assert_eq!(
        config["profile"]["dev"]["debug"].as_integer(),
        Some(1),
        "profile.dev.debug is the integer 1"
    );
    let profiles = config["profile"].as_table().expect("[profile] is a table");
    assert_eq!(
        profiles.keys().collect::<Vec<_>>(),
        ["dev"],
        "the config file touches no profile but dev; release stays Cargo.toml's"
    );
}

#[test]
fn every_crate_in_the_clone_builds_into_the_clones_own_target_directory() {
    let clone_target = repo_root().join("target");
    assert_eq!(
        reported_target_directory(&repo_root()),
        clone_target,
        "the workspace root builds into <clone>/target"
    );
    let outside = probe_crate("outside-workspace");
    assert_eq!(
        reported_target_directory(&outside),
        clone_target,
        "a crate outside the workspace, inside the clone, builds into the same <clone>/target"
    );
}

#[test]
fn a_second_checkout_carrying_its_own_copy_of_the_file_builds_into_its_own_root() {
    // A worktree nested inside a clone is the case that matters: cargo reads
    // both files, and the deeper one's `target-dir` has to win *and* resolve
    // against the deeper root, or two checkouts silently share one target.
    let nested_root = probe_crate("nested-checkout");
    fs::create_dir_all(nested_root.join(".cargo")).expect("a nested config directory");
    fs::write(nested_root.join(".cargo/config.toml"), build_config()).expect("a copied config");
    assert_eq!(
        reported_target_directory(&nested_root),
        nested_root.join("target"),
        "a checkout with its own copy of the file resolves target-dir against its own root"
    );
}

#[test]
fn dev_and_test_builds_carry_line_tables_only_and_release_carries_no_debuginfo() {
    let probe = probe_crate("debuginfo");
    let deps = repo_root().join("target/debug/deps");

    let dev = rustc_invocation(
        &cargo(&probe, ["build", "-v", "--offline"]),
        "--crate-type lib",
    );
    assert!(
        dev.contains("-C debuginfo=1"),
        "a dev build carries line tables only: {dev}"
    );
    assert!(
        dev.contains(&format!("--out-dir {}", deps.display())),
        "a dev build of a crate outside the workspace lands in <clone>/target/debug: {dev}"
    );

    let test = rustc_invocation(
        &cargo(&probe, ["test", "-v", "--no-run", "--offline"]),
        "--test",
    );
    assert!(
        test.contains("-C debuginfo=1"),
        "the test profile inherits dev's line tables: {test}"
    );

    let release = rustc_invocation(
        &cargo(&probe, ["build", "-v", "--offline", "--release"]),
        "--crate-type lib",
    );
    // `-C strip=debuginfo` is cargo's own release default and names no level;
    // what the dev profile would have added is a `-C debuginfo=` level.
    assert!(
        !release.contains("-C debuginfo="),
        "a release build is untouched by the dev profile: {release}"
    );
}
