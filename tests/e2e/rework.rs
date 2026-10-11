//! Reworking a draft in review, and restacking every draft stacked on it.
//!
//! A `rework` supersedes a node settled `done` as a review draft with a replacement
//! that works on the node's own branch and republishes onto the change request
//! already open from it; once it pushes, every draft stacked on it is brought onto
//! its new head by a merge, an agent dispatched only where that merge conflicts.
//! Every journey drives the compiled binary publishing through the real linked
//! `onevcs` over a real origin on disk, with GitHub standing in at that library's
//! own `ONEVCS_GH` seam — the `fake-gh` double, whose change requests are read back
//! for their base, state, title and description — and reads what the run recorded.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary and the sibling these journeys are about — `onevcs`, which opens each branch,
// merges its stack base in, assembles stack bases and publishes each change — is the real
// library over real git and a real origin on disk. `oneagentgraph` is substituted at its
// subprocess boundary so a journey states what a worker and a drafter answered rather than
// paying for model turns, and GitHub at `onevcs`'s own `ONEVCS_GH` override so a change
// request's state, title and description can be read offline. `harness.rs` carries the same
// suppression and the full rationale.

use crate::harness::{lifecycle, plan_of, World};
use crate::stacked_drafts::{
    change_from, changed_against, held_as_draft, service, settled, tasks_of, team_world, text,
};
use serde_json::{json, Value};

/// The review comment every rework here answers.
const COMMENT_URL: &str = "https://github.com/owner/service/pull/1#discussion_r101";

/// Launch `nodes` attached at `concurrency` from the Markdown store, wait for the
/// run's result, and hand back the project it was launched from.
fn launched(
    world: &World,
    run: &str,
    nodes: Vec<Value>,
    concurrency: u64,
    extra: &[&str],
) -> String {
    let mut plan = plan_of(run, nodes);
    plan["concurrency"] = json!(concurrency);
    let project = world.plan(run, &plan);
    let mut args = vec!["start", project.as_str(), "--attach"];
    args.extend_from_slice(extra);
    let launched = world.run_from(&world.project, &args);
    assert!(
        world.run_file(run, "result.json").is_file(),
        "the run did not settle:\n{}\n{}",
        launched.stdout,
        launched.stderr
    );
    project
}

/// Reply to `run` with one envelope of `commands`, as the planner, and hand back
/// what the reply printed.
fn reply(world: &World, run: &str, commands: Value) -> crate::harness::Run {
    world.run_with_stdin(
        &["reply", run],
        &json!({"version": onepipeline::channel::REPLY_ENVELOPE_VERSION, "commands": commands})
            .to_string(),
    )
}

/// A `rework` of `id`, as a planner relays a reviewer's request.
fn rework(id: &str, instruction: &str) -> Value {
    json!({
        "op": "rework",
        "id": id,
        "reason": "a reviewer asked for it on the draft",
        "instruction": instruction,
        "comments": [{"id": "101", "url": COMMENT_URL}],
    })
}

/// Drive `run` after a reply made it live again, to its next ending.
fn adopt(world: &World, run: &str) -> crate::harness::Run {
    world.run(&["adopt", run])
}

/// How many change requests the host was asked to open from `branch`.
fn opened_from(world: &World, branch: &str) -> usize {
    world
        .changes_opened()
        .iter()
        .filter(|change| change["head"] == branch)
        .count()
}

/// Whether `ancestor` is an ancestor of `of` on the origin.
fn is_ancestor(world: &World, ancestor: &str, of: &str) -> bool {
    std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, of])
        .current_dir(service(world).origin)
        .status()
        .expect("git runs")
        .success()
}

/// The committed operations of every `edit-committed` the run journalled.
fn committed(world: &World, run: &str) -> Vec<Value> {
    world
        .events_of(run, "edit-committed")
        .iter()
        .flat_map(|event| {
            event["payload"]["operations"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

/// The journey the op exists for: a node settled `change-review-draft` is reworked
/// from the planner's channel, and its replacement continues the node's own branch,
/// republishes onto the change request already open from it — never a second one —
/// which stays a draft, while the node's dependent stays `done`. The rework is on
/// the record with its reason and instruction, and the replacement's task on the
/// Markdown store carries what it answers.
#[test]
fn a_draft_in_review_is_reworked_on_its_own_branch_and_change_request() {
    let world = team_world("rework-draft");
    world.script("a.work", "a's change\n");
    world.script("b.work", "b's change\n");
    world.script("a-rework-1.work", "a's change, as the review asked\n");
    world.script("a-rework-1.work-file", "a");
    world.script("pr-author.title-check", r#"{"verdict": "keeps"}"#);
    let run = "draft";
    let drafter = world.pr_author_graph();
    let project = launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
        1,
        &["--pr-author-graph", &drafter],
    );
    let a = settled(&world, run, "a");
    assert_eq!(a["outcome"], "change-review-draft", "{a}");
    let a_branch = text(&a["branch"], "a's branch").to_owned();
    let a_head = text(&a["head"], "a's head").to_owned();
    let a_number = change_from(&world, &a_branch)["number"].clone();

    reply(
        &world,
        run,
        json!([rework("a", "say what the review asked in a.md")]),
    )
    .exited(0);
    adopt(&world, run).exited(0);

    // On the record: the rework, with what it asked.
    let reworked = committed(&world, run)
        .into_iter()
        .find(|operation| operation["kind"] == "rework-requested")
        .unwrap_or_else(|| panic!("no rework on the record: {:#?}", world.journal(run)));
    assert_eq!(reworked["node"], "a", "{reworked}");
    assert_eq!(reworked["replacement"], "a-rework-1", "{reworked}");
    assert_eq!(
        reworked["reason"], "a reviewer asked for it on the draft",
        "{reworked}"
    );
    assert_eq!(
        reworked["instruction"], "say what the review asked in a.md",
        "{reworked}"
    );

    // The replacement worked on the node's branch, on top of its commits.
    let replacement = settled(&world, run, "a-rework-1");
    assert_eq!(replacement["status"], "done", "{replacement}");
    assert_eq!(
        replacement["outcome"], "change-review-draft",
        "{replacement}"
    );
    assert_eq!(replacement["branch"], a_branch.as_str(), "{replacement}");
    let new_head = text(&replacement["head"], "the replacement's head");
    assert_ne!(new_head, a_head);
    assert!(
        is_ancestor(&world, &a_head, new_head),
        "the node's commits are not under the replacement's"
    );
    let task = &tasks_of(&world, run, "a-rework-1", "engineer")[0];
    assert!(
        task.contains("\n## Rework\n") && task.contains("say what the review asked in a.md"),
        "{task}"
    );
    assert!(task.contains(COMMENT_URL), "{task}");

    // One change request for the branch across the whole journey, still a draft,
    // and the replacement's settlement names it.
    assert_eq!(
        opened_from(&world, &a_branch),
        1,
        "{:#?}",
        world.changes_opened()
    );
    let change = change_from(&world, &a_branch);
    assert_eq!(change["number"], a_number, "{change}");
    assert!(held_as_draft(&world, &a_number), "{change}");
    assert_eq!(replacement["change_url"], a["change_url"], "{replacement}");

    // The dependent was never redone: it stayed `done`, and no worker was
    // dispatched for it again.
    let b = settled(&world, run, "b");
    assert_eq!(b["status"], "done", "{b}");
    assert_eq!(
        world
            .events_of(run, "node-dispatched")
            .iter()
            .filter(|event| event["labels"]["node"] == "b")
            .count(),
        1,
        "b was dispatched again"
    );
    assert_eq!(
        changed_against(&world, &service(&world), &a_branch, text(&b["branch"], "b")),
        ["b.md"]
    );

    // The replacement's task on the Markdown store says what it answers.
    world.until_store("the rework to reach the store", |world| {
        world
            .store_tasks(&project)
            .iter()
            .any(|task| task["item"]["metadata"]["onepipeline.rework"]["replaces"] == "a")
    });
    let projected = world
        .store_tasks(&project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.rework"]["replaces"] == "a")
        .expect("found above");
    assert_eq!(
        projected["item"]["metadata"]["onepipeline.rework"],
        json!({
            "replaces": "a",
            "reason": "a reviewer asked for it on the draft",
            "instruction": "say what the review asked in a.md",
            "comments": [{"id": "101", "url": COMMENT_URL}],
        }),
        "{projected}"
    );

    // Both views read the reworked node as reworked, with its reason and the
    // comment it answers, and its dependent as restacked onto the new head with
    // no agent turn.
    let reworked_line = format!(
        "a: reworked — as a-rework-1: a reviewer asked for it on the draft; answering {COMMENT_URL}"
    );
    let restacked_line = format!(
        "b: restacked onto a-rework-1's branch {a_branch} at {new_head}, without an agent turn"
    );
    let status = world.run(&["status", run]);
    status.exited(0);
    status.out_has(&reworked_line);
    status.out_has(&restacked_line);
    status.out_has("2 node(s) have not landed: a-rework-1, b");
    let results = world.run(&["results", run]);
    results.exited(0);
    results.out_has(&format!(
        "reworked — as a-rework-1: a reviewer asked for it on the draft; answering {COMMENT_URL}"
    ));
    results.out_has(&format!(
        "restacked onto a-rework-1's branch {a_branch} at {new_head}, without an agent turn"
    ));
}
