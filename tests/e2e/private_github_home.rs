//! A plan homed on a GitHub Projects board declared private, written to through the store this
//! crate links: a live edit that adds a task naming only a public repository creates that task's
//! canonical issue in a private repository, on the private board, and the task's one public
//! presence is the store's neutral projection on the public board its route names.
//!
//! GitHub is `tests/e2e/github_world.py`: a loopback holding repositories with a visibility,
//! Projects with a `public` flag and the issues and items on them, answering every GraphQL
//! document the store sends by executing it against that model, and each repository's
//! visibility over REST — checking every field it answers against the GitHub schema the linked
//! `onetaskgraph-github-projects` release pins. Everything else is the compiled binary and the
//! real linked store.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// GitHub is the loopback beside this file — real HTTP on a real socket, reached by the real
// `github-projects` plugin the engine links — because an offline check may not reach the
// hosted API. `harness.rs` carries the same suppression and the full rationale.

use std::path::Path;

use serde_json::{json, Map, Value};

use crate::harness::World;
use crate::linear_writeback::{from_the_launch_directory, projection_records};

/// The repository the private board files into when nothing else decides, declared private.
const PRIVATE: &str = "owner/service";
/// The repository the added task names, declared public.
const PUBLIC: &str = "owner/library";
/// The private board, homing the plan.
const HOME_BOARD: u64 = 1;
/// The public board the private home's route sends public work to.
const PUBLIC_BOARD: u64 = 2;
/// The environment variable both board sources read their token from.
const TOKEN_ENV: &str = "ONEPIPELINE_LOOPBACK_BOARD_TOKEN";

/// The loopback GitHub, for as long as this is held.
struct GitHub {
    child: onepipeline_testfakes::profile::process::Child,
    url: String,
}

impl GitHub {
    /// Serve the model `seed` describes, in the shape `github_world.py` documents.
    fn serve(world: &World, seed: &Value) -> Self {
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
    fn board(&self, number: u64) -> String {
        format!("{}/board/{number}/graphql", self.url)
    }

    /// The whole model and every request it was sent.
    fn state(&self) -> Value {
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
fn described(content: &str, metadata: &Value) -> String {
    format!("{content}\n\n<!-- onetaskgraph.metadata\n{metadata}\n-->")
}

/// The caller metadata an issue body carries in its trailing slot.
fn metadata_of(body: &str) -> Map<String, Value> {
    body.rsplit_once("<!-- onetaskgraph.metadata")
        .and_then(|(_, slot)| slot.rsplit_once("-->"))
        .and_then(|(json, _)| serde_json::from_str(json.trim()).ok())
        .unwrap_or_default()
}

fn issues(state: &Value) -> &Map<String, Value> {
    state["issues"].as_object().expect("the model's issues")
}

/// The node ids of the issues board `number` holds an item for.
fn on_board(state: &Value, number: u64) -> Vec<String> {
    state["boards"][number.to_string()]["items"]
        .as_array()
        .expect("the board's items")
        .iter()
        .map(|item| {
            state["items"][item.as_str().expect("an item id")]["content"]
                .as_str()
                .expect("an item's issue")
                .to_owned()
        })
        .collect()
}

/// A live edit adding a task that names only a public repository, to a plan homed on a GitHub
/// board declared private, is created through the linked store as a canonical item of the
/// private board — its issue filed in the private repository the placement rule names, the
/// plan's own — while still naming its public repository; and the task's only public presence
/// is the store's neutral projection on the public board, which the home records.
#[test]
fn a_private_github_home_files_a_live_added_public_task_in_its_private_repository() {
    let name = "github-private-home";
    let (world, _private, _public) = crate::publication_boundary::mixed_world(name);
    let onevcs = crate::harness::onevcs_binary()
        .to_string_lossy()
        .into_owned();
    let world = world
        .with_env(TOKEN_ENV, "loopback-token")
        .with_env(
            "ONETASKGRAPH_WRITE_POLICY__CHECK_COMMAND",
            &format!("{onevcs},boundary,check,--destination,public,--input,-"),
        )
        .with_env(
            "ONETASKGRAPH_WRITE_POLICY__VISIBILITY_COMMAND",
            &format!("{onevcs},boundary,inspect,--input,-,--json"),
        );
    world.script("anchor.wait", "hold");
    let github = GitHub::serve(
        &world,
        &json!({
            "owner": "owner",
            "repositories": {PRIVATE: "private", PUBLIC: "public"},
            "boards": {HOME_BOARD.to_string(): {"public": false},
                       PUBLIC_BOARD.to_string(): {"public": true}},
            "issues": [
                {"id": "plan", "repository": PRIVATE, "title": name, "board": HOME_BOARD,
                 "status": "Todo", "parent": null,
                 "body": described("## Plan\n- Keep this home private.", &json!({
                     "onetaskgraph.classification": "private",
                     "onepipeline.schema_version": onepipeline::plan::PLAN_SCHEMA_VERSION,
                     "onepipeline.concurrency": 1,
                     "onepipeline.goal": {"text": "Deliver the plan"},
                 }))},
                {"id": "anchor", "repository": PRIVATE, "title": "chore: anchor",
                 "board": HOME_BOARD, "status": "Todo", "parent": "plan",
                 "body": described(
                     "## What\nDo anchor.\n\n## Why\nSo the run can settle.\n\n\
                      ## Acceptance criteria\n- anchor is done.",
                     &json!({"onepipeline.id": "anchor", "onepipeline.persona": "engineer",
                             "onetaskgraph.classification": "private"}))},
            ],
        }),
    );
    let launch = world.root.join("launch-directory");
    std::fs::create_dir_all(&launch).expect("the launch directory");
    let mapping = "      status_mapping:\n        todo: Todo\n        queued: Queued\n        \
                   in-progress: In Progress\n        unknown: Needs Attention\n        \
                   done: Done\n        cancelled: Canceled\n";
    std::fs::write(
        launch.join("onetaskgraph.yaml"),
        format!(
            "sources:\n  board:\n    plugin: github-projects\n    visibility: private\n    \
             routes:\n      - to: site\n        repositories: [github.com/{PUBLIC}]\n    \
             config:\n      owner: owner\n      project_number: {HOME_BOARD}\n      \
             repository: {PRIVATE}\n      token_env: {TOKEN_ENV}\n      endpoint: {}\n{mapping}\
             \x20 site:\n    plugin: github-projects\n    visibility: public\n    config:\n      \
             owner: owner\n      project_number: {PUBLIC_BOARD}\n      repository: {PUBLIC}\n      \
             token_env: {TOKEN_ENV}\n      endpoint: {}\n{mapping}",
            github.board(HOME_BOARD),
            github.board(PUBLIC_BOARD),
        ),
    )
    .expect("the store configuration");
    let start = from_the_launch_directory(&world, &launch, &["start", "board:plan", "--detach"]);
    world
        .run_on(start, "start the private GitHub home")
        .exited(0);
    world.until("the anchor to dispatch", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });

    let mut node = crate::harness::agent("examples", &[]);
    node["repo"] = json!(format!("github.com/{PUBLIC}"));
    node["classification"] = json!("public");
    node["parked"] = json!(true);
    world
        .run_with_stdin(
            &["reply", name],
            &json!({"version": 3, "commands": [{"op": "add", "node": node}]}).to_string(),
        )
        .exited(0);
    world.until("the live addition's write-back to finish", |world| {
        projection_records(world, name)
            .iter()
            .any(|record| record.to_string().contains("examples"))
    });
    let records = projection_records(&world, name);
    let state = github.state();
    assert!(
        records
            .iter()
            .any(|record| record["outcome"] == "projected"
                && record.to_string().contains("examples")),
        "the live addition was not projected: {records:#?}\nunsupported: {}",
        state["unsupported"]
    );

    // The canonical task: one issue carrying the node, filed in the private repository the
    // placement rule names — the plan's own — and an item of the private board.
    let canonical: Vec<(&String, &Value)> = issues(&state)
        .iter()
        .filter(|(_, issue)| {
            metadata_of(issue["body"].as_str().unwrap_or_default()).get("onepipeline.id")
                == Some(&json!("examples"))
        })
        .collect();
    assert_eq!(
        canonical.len(),
        1,
        "exactly one issue carries the added node: {:#}",
        state["issues"]
    );
    let (canonical_id, canonical_issue) = canonical[0];
    assert_eq!(
        canonical_issue["repository"], PRIVATE,
        "the canonical issue is filed in the private repository: {canonical_issue:#}"
    );
    assert_eq!(
        canonical_issue["parent"], "plan",
        "the canonical issue is a task of the private home: {canonical_issue:#}"
    );
    assert!(
        on_board(&state, HOME_BOARD).contains(canonical_id),
        "the canonical issue is an item of the private board: {:#}",
        state["boards"]
    );
    assert!(
        !on_board(&state, PUBLIC_BOARD).contains(canonical_id),
        "the canonical issue is not on the public board: {:#}",
        state["boards"]
    );

    // Its one public presence: the store's neutral projection on the public board — a task
    // carrying the node's public work, under the neutral project that holds public work — which
    // the home records against the canonical task, and none of which carries the home's text.
    let public: Vec<&Value> = issues(&state)
        .values()
        .filter(|issue| issue["repository"] == PUBLIC)
        .collect();
    let (projects, projections): (Vec<&Value>, Vec<&Value>) =
        public.iter().partition(|issue| issue["parent"].is_null());
    assert_eq!(
        (projects.len(), projections.len()),
        (1, 1),
        "the public repository holds the projection and its neutral project alone: {:#}",
        state["issues"]
    );
    let (project, projected) = (projects[0], projections[0]);
    let project_id = project["id"].as_str().expect("the neutral project's id");
    let projected_id = projected["id"].as_str().expect("the projection's id");
    assert_eq!(projected["parent"], project_id, "{projected:#}");
    let mut held = on_board(&state, PUBLIC_BOARD);
    held.sort();
    let mut expected = vec![project_id.to_owned(), projected_id.to_owned()];
    expected.sort();
    assert_eq!(
        held, expected,
        "the public board holds the projection and its neutral project alone: {:#}",
        state["boards"]
    );
    assert!(
        !on_board(&state, HOME_BOARD)
            .iter()
            .any(|id| id == project_id || id == projected_id),
        "nothing public is on the private board: {:#}",
        state["boards"]
    );
    assert!(
        projected.to_string().contains("Do examples."),
        "{projected:#}"
    );
    for item in [project, projected] {
        let shown = item.to_string();
        for private in [
            "Keep this home private",
            "anchor",
            "service",
            "onepipeline.",
        ] {
            assert!(
                !shown.contains(private),
                "the public board carries the private {private:?}: {item:#}"
            );
        }
    }
    let home = metadata_of(
        state["issues"]["plan"]["body"]
            .as_str()
            .expect("the home's body"),
    );
    let map = home
        .get("onetaskgraph.public_projections")
        .unwrap_or_else(|| panic!("the home records no public projection: {home:#?}"));
    assert_eq!(
        map["tasks"],
        json!([{"task": format!("board:{canonical_id}"), "projection": format!("site:{projected_id}")}]),
        "{map:#}"
    );
    assert_eq!(map["projects"][0]["project"], project_id, "{map:#}");

    // The placement read the named repository's visibility before filing the issue elsewhere.
    let read: Vec<&str> = state["requests"]
        .as_array()
        .expect("the request log")
        .iter()
        .filter_map(|request| request["path"].as_str())
        .filter(|path| path.starts_with("/repos/"))
        .collect();
    assert!(read.contains(&"/repos/owner/library"), "{read:?}");
    world.release("anchor.go");
}
