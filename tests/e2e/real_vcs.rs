//! A lifecycle node published by the **real** `onevcs`.
//!
//! Every other lifecycle journey states its scenario through the `onevcs`
//! double, which is how a rejected gate or a held publication is reachable
//! without paid turns and real hosts. What a double cannot prove is the seam
//! itself: the argv this crate builds, the exit codes it reads, and — the one
//! that mattered — *the shape of what the sibling prints back*. This crate used
//! to read `onevcs publish`'s stdout as JSON; the real command prints one line
//! of prose, so against the real sibling every publication failed as unreadable
//! while the suite stayed green against a double that printed JSON. That is the
//! same defect as R0, in the path that merges work.
//!
//! Offline and hermetic: a bare repository on disk is the origin, the identity
//! publishes `local-direct` so no host is ever asked for anything, and the state
//! root is this world's own. Nothing here reaches the network.

// llmlint: ignore-file[e2e_not_mocked] the sibling under test is *not* substituted here:
// `onevcs` is the real binary, built from the version `Cargo.lock` pins, driving real git
// against a real origin. `oneagentgraph` is still the double, because what these journeys
// are about is the repository side and a real agent turn is a paid one.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use crate::harness::{onevcs_binary, plan_of, World, GIT_EMAIL, GIT_WHO};

/// A registered repository: a bare origin, a checkout of it, and the rules that
/// decide how work published from it lands.
struct Registered {
    /// The bare repository that stands in for the remote.
    origin: PathBuf,
    /// The registered execution and publication checkout.
    checkout: PathBuf,
}

impl Registered {
    /// What the origin's base branch carries now.
    fn base_commits(&self) -> Vec<String> {
        git(&self.origin, &["log", "--format=%s", "main"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// One file's contents on the origin's base branch, if it carries one.
    fn base_file(&self, name: &str) -> Option<String> {
        let shown = Command::new("git")
            .arg("show")
            .arg(format!("main:{name}"))
            .current_dir(&self.origin)
            .output()
            .expect("git runs");
        shown
            .status
            .success()
            .then(|| String::from_utf8_lossy(&shown.stdout).into_owned())
    }
}

/// Run git in a repository, refusing to continue on anything it rejects.
fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", GIT_WHO)
        .env("GIT_AUTHOR_EMAIL", GIT_EMAIL)
        .env("GIT_COMMITTER_NAME", GIT_WHO)
        .env("GIT_COMMITTER_EMAIL", GIT_EMAIL)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A repository the real `onevcs` knows about, publishing under `gate`.
///
/// `local-direct`, so the whole publication is git: the branch is squashed onto
/// the base and pushed, and no remote host is asked for anything. That is the
/// one policy whose every step this journey can actually reach offline.
fn registered(world: &World, gate: &[&str]) -> Registered {
    let origin = world.root.join("origin.git");
    let checkout = world.root.join("checkout");
    let home = world.onevcs_home();
    for dir in [&origin, &home] {
        std::fs::create_dir_all(dir).expect("a scratch directory");
    }
    git(&origin, &["init", "--bare", "--initial-branch=main"]);
    git(
        &world.root,
        &["clone", &origin.to_string_lossy(), "checkout"],
    );
    std::fs::write(checkout.join("README.md"), "the repository under test\n")
        .expect("the seed file is written");
    git(&checkout, &["add", "-A"]);
    git(&checkout, &["commit", "-m", "chore: seed the repository"]);
    git(&checkout, &["push", "-u", "origin", "main"]);

    std::fs::write(
        home.join("rules.yml"),
        format!(
            "version: 2\nrules: []\ndefault:\n  publication: local-direct\n  approvals: none\n  \
             gate:\n    command: {}\n",
            json!(gate)
        ),
    )
    .expect("the rules file is written");

    let registration = Command::new(onevcs_binary())
        .arg("register")
        .arg(&checkout)
        .env("ONEVCS_HOME", &home)
        .output()
        .expect("the real onevcs runs");
    assert!(
        registration.status.success(),
        "onevcs register refused {}: {}",
        checkout.display(),
        String::from_utf8_lossy(&registration.stderr)
    );

    Registered { origin, checkout }
}

/// A lifecycle node whose repository is the registered checkout.
///
/// It names its own title, so the run spends no `pr-author` dispatch: that
/// dispatch opens a second session on the same branch, and the real `onevcs`
/// holds a live session's branch under an occupancy lease. Which title wins is
/// what `lifecycle.rs` proves; this journey is about the publication.
fn node(repo: &Path) -> serde_json::Value {
    json!({
        "id": "service",
        "repo": repo.to_string_lossy(),
        "persona": "engineer",
        "title": "feat: land the change the worker made",
        "task": "## What\nShip the service.\n\n## Why\nUsers need it.\n\n## Acceptance criteria\n- It is published.",
    })
}

/// Every `onevcs`-produced event one run recorded, by kind.
fn vcs_kinds(world: &World, run: &str) -> Vec<String> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "vcs")
        .filter_map(|event| event["kind"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn a_lifecycle_node_publishes_through_the_real_onevcs_and_the_base_advances() {
    let world = World::new("real-vcs-publish");
    let repo = registered(&world, &["true"]);
    world.script("service.work", "the worker wrote this\n");

    let path = world.plan("landed", &plan_of("landed", vec![node(&repo.checkout)]));
    world
        .run_on_vcs(&["start", &path.to_string_lossy(), "--attach"])
        .settled();
    world.until("the run to settle", |world| {
        !world.events_of("landed", "round-finished").is_empty()
    });

    // The node settled on what the sibling actually did, which is the assertion
    // that fails when this crate cannot read the sibling's answer.
    let result = world.run_json("landed", "round-01/result.json");
    assert_eq!(result["nodes"][0]["status"], "done", "{result}");
    assert_eq!(result["nodes"][0]["outcome"], "merged", "{result}");
    assert_eq!(result["state"], "complete", "{result}");

    // The work reached the origin's base branch. Nothing about a settlement
    // proves that; this is the repository saying so.
    assert_eq!(
        repo.base_commits(),
        vec![
            "feat: land the change the worker made".to_string(),
            "chore: seed the repository".to_string(),
        ],
        "the base did not advance by exactly the published change"
    );
    assert_eq!(
        repo.base_file("service.md").as_deref().map(str::trim),
        Some("the worker wrote this"),
        "the base advanced without the work the dispatch made"
    );

    // And the sibling's own record of it joined the merged store, which is what
    // a person reads afterwards — **once each**. The publication is followed as
    // it happens and read once more if the follow relayed nothing, so a record
    // that arrives twice is the recovery covering for a follow that worked.
    let kinds = vcs_kinds(&world, "landed");
    for kind in ["gate-verdict", "push", "merge-completed", "session-closed"] {
        let seen = kinds.iter().filter(|seen| *seen == kind).count();
        assert_eq!(
            seen, 1,
            "the publication's {kind} reached the merged store {seen} time(s): {kinds:?}"
        );
    }

    // Under the node it belongs to. A `onevcs` session does not know it is a
    // graph node — the real one stamps its own token and identity and nothing
    // else — so without the enricher a whole real publication lands in the store
    // belonging to nobody.
    let verdict = &world.events_of("landed", "gate-verdict")[0];
    assert_eq!(verdict["labels"]["node"], "service", "{verdict}");
    assert_eq!(verdict["labels"]["run_id"], "landed", "{verdict}");
    assert!(
        verdict["labels"]["session"].is_string(),
        "the sibling's own label was rewritten: {verdict}"
    );
    world
        .run(&["results", "landed"])
        .exited(0)
        .out_has("service")
        .out_has("done");
}

#[test]
fn a_real_gate_that_rejects_the_branch_fails_the_node_and_leaves_the_base_alone() {
    let world = World::new("real-vcs-gate");
    let repo = registered(&world, &["false"]);
    world.script("service.work", "the worker wrote this\n");

    let path = world.plan("refused", &plan_of("refused", vec![node(&repo.checkout)]));
    world
        .run_on_vcs(&["start", &path.to_string_lossy(), "--attach"])
        .settled();
    world.until("the run to settle", |world| {
        !world.events_of("refused", "round-finished").is_empty()
    });

    let result = world.run_json("refused", "round-01/result.json");
    assert_eq!(result["nodes"][0]["status"], "failed", "{result}");
    assert_eq!(
        result["nodes"][0]["outcome"], "publication-failed",
        "{result}"
    );

    // The one thing a rejected gate has to be true of: nothing landed.
    assert_eq!(
        repo.base_commits(),
        vec!["chore: seed the repository".to_string()],
        "a branch the gate rejected still reached the base"
    );
    assert!(
        vcs_kinds(&world, "refused")
            .iter()
            .any(|kind| kind == "gate-verdict"),
        "the gate's verdict never reached the merged store"
    );
}
