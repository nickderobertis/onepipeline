//! A lifecycle node published through `onevcs`, read off the repository itself.
//!
//! Every lifecycle journey here drives the real repository side — `onevcs` is
//! linked into the binary under test, not substituted — so what this file adds is
//! the assertion none of the others make: that the origin's base branch actually
//! **advanced by the change**, and that a branch the gate rejected did not reach
//! it. A settlement is this crate's account of a publication; the repository is
//! the publication.
//!
//! Offline and hermetic: a bare repository on disk is the origin, the identity
//! publishes `local-direct` so no host is ever asked for anything, and the state
//! root is this world's own. Nothing here reaches the network.
//!
// llmlint: ignore-file[e2e_not_mocked] the sibling under test is *not* substituted here:
// `onevcs` is the library this crate links, driving real git against a real origin.
// `oneagentgraph` is still the double, because what these journeys are about is the
// repository side and a real agent turn is a paid one.

use std::path::Path;

use serde_json::json;

use crate::harness::{plan_of, World};

/// A lifecycle node whose repository is the registered checkout.
///
/// It names its own title, so the run spends no `pr-author` dispatch: which
/// title wins is what `lifecycle.rs` proves, and this journey is about the
/// publication.
fn node(repo: &Path) -> serde_json::Value {
    json!({
        "id": "service",
        "repo": repo.to_string_lossy(),
        "persona": "engineer",
        "title": "feat: land the change the worker made",
        "task": "## What\nShip the service.\n\n## Why\nUsers need it.\n\n## Acceptance criteria\n- It is published.",
    })
}

/// Why a run settled the way it did, as the sibling itself said it.
///
/// `result.json` records the status and the outcome and not a word of the
/// reason, and the reason is the sibling's own refusal — the command it ran and
/// what that printed. It reaches this crate as the sibling error's message and
/// is journalled as the `detail` of `node-settled`, so this is where a failure
/// that only happens on one platform names itself instead of costing a
/// debugging session per platform.
fn why(world: &World, run: &str) -> String {
    let settled: Vec<String> = world
        .events_of(run, "node-settled")
        .iter()
        .map(|event| {
            format!(
                "{} {} {}: {}",
                event["labels"]["node"],
                event["payload"]["status"],
                event["payload"]["outcome"],
                event["payload"]["detail"]
            )
        })
        .collect();
    format!("what the nodes settled on:\n  {}", settled.join("\n  "))
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
    let repo = world.repository("local-direct", &["true"]);
    world.script("service.work", "the worker wrote this\n");

    let path = world.plan("landed", &plan_of("landed", vec![node(&repo.checkout)]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .settled();
    world.until("the run to settle", |world| {
        world.run_file("landed", "result.json").is_file()
    });

    // The node settled on what the sibling actually did, which is the assertion
    // that fails when this crate cannot read the sibling's answer.
    let result = world.run_json("landed", "result.json");
    assert_eq!(
        result["nodes"][0]["status"],
        "done",
        "{result}\n{}",
        why(&world, "landed")
    );
    assert_eq!(
        result["nodes"][0]["outcome"],
        "merged",
        "{result}\n{}",
        why(&world, "landed")
    );
    assert_eq!(result["state"], "complete", "{result}");

    // The work reached the origin's base branch. Nothing about a settlement
    // proves that; this is the repository saying so.
    assert_eq!(
        repo.base_commits(&world),
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
    let repo = world.repository("local-direct", &["false"]);
    world.script("service.work", "the worker wrote this\n");

    let path = world.plan("refused", &plan_of("refused", vec![node(&repo.checkout)]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .settled();
    world.until("the run to settle", |world| {
        world.run_file("refused", "result.json").is_file()
    });

    let result = world.run_json("refused", "result.json");
    assert_eq!(
        result["nodes"][0]["status"],
        "failed",
        "{result}\n{}",
        why(&world, "refused")
    );
    assert_eq!(
        result["nodes"][0]["outcome"],
        "publication-failed",
        "{result}\n{}",
        why(&world, "refused")
    );

    // The one thing a rejected gate has to be true of: nothing landed.
    assert_eq!(
        repo.base_commits(&world),
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

/// `filters.vcs` reaches the real `onevcs` and narrows what it relays.
///
/// The filter is handed to the sibling as a **value**, on the filtered
/// `EventStream` constructor it exposes for exactly this — so what this journey
/// proves is that the source did not relay the records, rather than that this
/// crate read them and threw them away. The evidence for that distinction is the
/// second half: the records the filter admits are all still there, in a run whose
/// publication did the same work.
///
/// Both halves in one journey, run against one repository, because the claim is a
/// *comparison*: an ingestion the filter narrowed against the ingestion a launch
/// naming no `filters:` block gets, which has to be the one it always got.
#[test]
fn a_launchs_vcs_filter_reaches_the_real_sibling_and_narrows_what_it_relays() {
    let world = World::new("real-vcs-filter");
    let repo = world.repository("local-direct", &["true"]);
    world.script("service.work", "the worker wrote this\n");

    // No `filters:` block: ingestion is exactly what it always was, and this is
    // the control the filtered run below is read against.
    let path = world.plan(
        "unfiltered",
        &plan_of("unfiltered", vec![node(&repo.checkout)]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .settled();
    world.until("the unfiltered run to settle", |world| {
        world.run_file("unfiltered", "result.json").is_file()
    });
    let ingested = vcs_kinds(&world, "unfiltered");
    for kind in ["gate-verdict", "push", "session-closed"] {
        assert!(
            ingested.iter().any(|seen| seen == kind),
            "a launch naming no filters did not ingest {kind}: {ingested:?}"
        );
    }

    // Different content, because the first run already landed the last lot: a
    // branch whose base already carries its change publishes nothing, and a
    // comparison against a run that did no work would prove nothing about the
    // filter.
    world.script("service.work", "and then the worker wrote this\n");
    let path = world.plan("filtered", &plan_of("filtered", vec![node(&repo.checkout)]));
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--filter-vcs",
            r#"{"exclude": [{"kind": "gate-verdict"}]}"#,
        ])
        .settled();
    world.until("the filtered run to settle", |world| {
        world.run_file("filtered", "result.json").is_file()
    });

    let kinds = vcs_kinds(&world, "filtered");
    assert!(
        !kinds.iter().any(|kind| kind == "gate-verdict"),
        "the source filter did not reach `onevcs`: {kinds:?}"
    );
    // Narrowed, not silenced — and the *same* publication happened, so the
    // difference between the two runs is the filter and nothing else.
    for kind in ["push", "session-closed"] {
        assert!(
            kinds.iter().any(|seen| seen == kind),
            "the source filter dropped {kind}, which it admits: {kinds:?}"
        );
    }
    let result = world.run_json("filtered", "result.json");
    assert_eq!(
        result["nodes"][0]["outcome"],
        "merged",
        "filtering the stream changed what the run did: {result}\n{}",
        why(&world, "filtered")
    );
}
