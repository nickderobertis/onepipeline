//! The two network plugins are cargo features of this crate, and no default enables them.
//!
//! Two halves, proven against the real thing rather than by reading the manifest. What a
//! dependent links is asked of cargo itself: a crate outside this workspace takes the engine
//! by path, and `cargo tree` resolves it with the lockfile this workspace ships, once with
//! the default features and once with both network features — the second being what shows
//! the first could have failed. And what a build without them does with a configuration
//! naming one is asked of the engine's own two front doors, the configuration document and
//! the subprocess serve loop, each of which has to name the feature that would compile it.
//!
//! This file runs twice in this crate's `test` target: under `--all-features` like every
//! other test here, and on its own with the default features, which is the only build in
//! which the refusals below exist.

use std::path::{Path, PathBuf};
use std::process::Command;

use onetaskgraph_core::{Config, serve};
use serde_json::{Value, json};
use tempfile::TempDir;

/// Packages whose presence marks the network plugins, or the HTTP and TLS stack behind them,
/// as linked — not the whole of that stack.
///
/// Only its platform-neutral part is named: `native-tls` sits on OpenSSL on
/// Linux, the Security framework on macOS and SChannel on Windows, so naming any of those
/// would make this assertion mean something different on each lane.
const NETWORK_MARKERS: [&str; 5] = [
    "onetaskgraph-github-projects",
    "onetaskgraph-linear",
    "reqwest",
    "hyper",
    "native-tls",
];

/// The package names `cargo tree` reaches from a dependent that takes this crate with
/// `features`, over the normal dependency edges a build of that dependent compiles.
fn packages_reached_with(features: &[&str]) -> Vec<String> {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = engine
        .parent()
        .and_then(Path::parent)
        .expect("the engine sits two levels under the workspace root");
    let dependent = TempDir::new().expect("a scratch directory");
    let features = features
        .iter()
        .map(|feature| format!("{feature:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    // An empty `[workspace]` keeps cargo from adopting whatever workspace encloses the
    // scratch directory, so this is a dependent of its own rather than a member of one.
    let manifest = format!(
        "[package]\nname = \"local-md-host\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
         publish = false\n\n[workspace]\n\n[dependencies]\n\
         onetaskgraph-core = {{ path = {:?}, features = [{features}] }}\n",
        engine.display().to_string()
    );
    std::fs::write(dependent.path().join("Cargo.toml"), manifest).expect("a manifest");
    std::fs::create_dir(dependent.path().join("src")).expect("a source directory");
    std::fs::write(dependent.path().join("src/lib.rs"), "").expect("a library root");
    // The workspace's own lockfile, so what resolves is the versions this repository ships
    // and nothing is asked of a registry: `--offline` below holds cargo to that.
    std::fs::copy(
        workspace.join("Cargo.lock"),
        dependent.path().join("Cargo.lock"),
    )
    .expect("the workspace lockfile");

    let cargo = std::env::var_os("CARGO").map_or_else(|| PathBuf::from("cargo"), PathBuf::from);
    let output = Command::new(cargo)
        .current_dir(dependent.path())
        .args([
            "tree",
            "--offline",
            "--edges",
            "normal",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo tree could not resolve a dependent of the engine:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut packages: Vec<String> = String::from_utf8(output.stdout)
        .expect("cargo tree prints UTF-8")
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    packages.sort();
    packages.dedup();
    packages
}

#[test]
fn a_dependent_taking_the_default_features_links_none_of_the_network_markers() {
    let packages = packages_reached_with(&[]);
    assert!(
        packages
            .iter()
            .any(|package| package == "onetaskgraph-local-md"),
        "the local store is part of every build: {packages:?}"
    );
    let linked: Vec<&str> = NETWORK_MARKERS
        .into_iter()
        .filter(|name| packages.iter().any(|package| package == name))
        .collect();
    assert!(
        linked.is_empty(),
        "a default-features dependent of onetaskgraph-core links {linked:?}; each network \
         plugin must stay behind its own optional feature"
    );
}

#[test]
fn a_dependent_enabling_both_network_features_links_every_network_marker() {
    let packages = packages_reached_with(&["github-projects", "linear"]);
    let missing: Vec<&str> = NETWORK_MARKERS
        .into_iter()
        .filter(|name| !packages.iter().any(|package| package == name))
        .collect();
    assert!(
        missing.is_empty(),
        "enabling both features did not link {missing:?}, so the default-features \
         assertion above could not have seen them either"
    );
}

fn naming(plugin: &str) -> Value {
    json!({"sources": {"work": {"plugin": plugin, "config": {}}}})
}

fn served_handshake(kind: &str) -> Value {
    let request = json!({
        "id": "0",
        "method": "initialize",
        "params": {
            "protocol_version": 2,
            "engine": {"name": "onetaskgraph", "version": "0.1.0"},
            "source_name": "work",
            "config": {"kind": kind, "config": {}},
            "secrets": {}
        }
    });
    let input = format!("{request}\n");
    let mut output: Vec<u8> = Vec::new();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(serve(input.as_bytes(), &mut output))
        .expect("the streams are in memory and cannot fail");
    serde_json::from_slice(&output).expect("one JSON answer")
}

#[cfg(not(feature = "github-projects"))]
#[test]
fn a_build_without_github_projects_refuses_it_by_name_with_the_feature_to_enable() {
    let error = Config::from_document(naming("github-projects"))
        .expect_err("this build did not compile the plugin")
        .to_string();
    assert!(error.contains("sources.work.plugin"), "{error}");
    assert!(
        error.contains("\"github-projects\" plugin is not compiled into this binary"),
        "{error}"
    );
    assert!(error.contains("`github-projects` feature"), "{error}");

    let answer = served_handshake("github-projects");
    let message = answer["error"]["message"].as_str().expect("a refusal");
    assert!(
        message.contains("enable the `github-projects` feature of onetaskgraph-core"),
        "{answer}"
    );
}

#[cfg(not(feature = "linear"))]
#[test]
fn a_build_without_linear_refuses_it_by_name_with_the_feature_to_enable() {
    let error = Config::from_document(naming("linear"))
        .expect_err("this build did not compile the plugin")
        .to_string();
    assert!(error.contains("sources.work.plugin"), "{error}");
    assert!(
        error.contains("\"linear\" plugin is not compiled into this binary"),
        "{error}"
    );
    assert!(error.contains("`linear` feature"), "{error}");

    let answer = served_handshake("linear");
    let message = answer["error"]["message"].as_str().expect("a refusal");
    assert!(
        message.contains("enable the `linear` feature of onetaskgraph-core"),
        "{answer}"
    );
}

#[cfg(all(feature = "github-projects", feature = "linear"))]
#[test]
fn a_build_with_both_features_registers_both_network_plugins() {
    let kinds = onetaskgraph_core::plugin_kinds();
    assert!(kinds.contains(&"github-projects"), "{kinds:?}");
    assert!(kinds.contains(&"linear"), "{kinds:?}");
    Config::from_document(naming("linear")).expect("linear is compiled into this build");
    Config::from_document(naming("github-projects"))
        .expect("github-projects is compiled into this build");

    // The serve loop reaches Linear's own factory, which refuses for want of a credential
    // rather than because the build has no such plugin.
    let answer = served_handshake("linear");
    let message = answer["error"]["message"].as_str().expect("a refusal");
    assert!(message.contains("LINEAR_API_KEY"), "{answer}");
}

#[test]
fn a_name_no_feature_answers_to_is_still_refused_as_unknown() {
    let error = Config::from_document(naming("jira"))
        .expect_err("nothing is called jira")
        .to_string();
    assert!(error.contains("no plugin named \"jira\""), "{error}");
    assert!(!error.contains("feature"), "{error}");
}
