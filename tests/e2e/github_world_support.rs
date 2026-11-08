//! The loopback GitHub `github_world.py` serves, and the metadata slot its issue bodies carry.

use std::path::Path;

use serde_json::{Map, Value};

use crate::harness::World;

/// The loopback GitHub, for as long as this is held.
pub(crate) struct GitHub {
    child: onepipeline_testfakes::profile::process::Child,
    pub(crate) url: String,
}

impl GitHub {
    /// Serve the model `seed` describes, in the shape `github_world.py` documents.
    pub(crate) fn serve(world: &World, seed: &Value) -> Self {
        let dir = world.root.join("github-world");
        std::fs::create_dir_all(&dir).expect("the loopback's directory");
        let seeded = dir.join("seed.json");
        std::fs::write(&seeded, seed.to_string()).expect("the loopback's seed");
        let mut child = onepipeline_testfakes::profile::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/e2e/github_world.py"))
            .arg(&dir)
            .arg(&seeded)
            .arg(linked_schema())
            .spawn()
            .expect("the loopback GitHub starts");
        let endpoint = dir.join("endpoint");
        world.until("the loopback GitHub to listen", |_| {
            assert!(
                child.try_wait().expect("the loopback's status").is_none(),
                "the loopback GitHub exited before it listened"
            );
            endpoint.is_file()
        });
        let url = std::fs::read_to_string(&endpoint).expect("the loopback's URL");
        Self { child, url }
    }

    /// The GraphQL endpoint board `number`'s source names.
    pub(crate) fn board(&self, number: u64) -> String {
        format!("{}/board/{number}/graphql", self.url)
    }

    /// The whole model and every request it was sent.
    pub(crate) fn state(&self) -> Value {
        use std::io::{Read, Write};
        let address = self.url.strip_prefix("http://").expect("a loopback URL");
        let mut socket = std::net::TcpStream::connect(address).expect("the loopback listens");
        write!(
            socket,
            "GET /state HTTP/1.0\r\nHost: {address}\r\nConnection: close\r\n\r\n"
        )
        .expect("the snapshot request");
        let mut response = String::new();
        socket.read_to_string(&mut response).expect("the snapshot");
        let (headers, body) = response.split_once("\r\n\r\n").expect("an HTTP response");
        assert!(headers.starts_with("HTTP/1.0 200 "), "{response}");
        serde_json::from_str(body).expect("the snapshot is JSON")
    }
}

impl Drop for GitHub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The GitHub GraphQL schema subset the linked `onetaskgraph-github-projects` release ships and
/// validates its own documents against, read from that release's source as `Cargo.lock` resolves
/// it, so the double is held to the very schema the store it answers was checked by.
fn linked_schema() -> std::path::PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = onepipeline_testfakes::profile::process::Command::new(cargo)
        .args(["metadata", "--offline", "--locked", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("cargo metadata is JSON");
    let manifest = metadata["packages"]
        .as_array()
        .expect("the packages")
        .iter()
        .find(|package| package["name"] == "onetaskgraph-github-projects")
        .and_then(|package| package["manifest_path"].as_str())
        .expect("the linked onetaskgraph-github-projects");
    let schema = Path::new(manifest)
        .parent()
        .expect("the crate's directory")
        .join("tests/fixtures/schema.graphql");
    assert!(schema.is_file(), "{} is not shipped", schema.display());
    schema
}

/// `content` with `metadata` in the trailing slot the `github-projects` plugin keeps it in.
pub(crate) fn described(content: &str, metadata: &Value) -> String {
    format!("{content}\n\n<!-- onetaskgraph.metadata\n{metadata}\n-->")
}

/// The caller metadata an issue body carries in its trailing slot.
pub(crate) fn metadata_of(body: &str) -> Map<String, Value> {
    body.rsplit_once("<!-- onetaskgraph.metadata")
        .and_then(|(_, slot)| slot.rsplit_once("-->"))
        .and_then(|(json, _)| serde_json::from_str(json.trim()).ok())
        .unwrap_or_default()
}
