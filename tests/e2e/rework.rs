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

use crate::harness::{git, lifecycle, plan_of, World};
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

/// The `title_check` a drafter answers when the title still says what the change
/// does.
const KEEPS: &str = r#"{"verdict": "keeps"}"#;

/// What `branch` stands at on the origin.
fn head_of(world: &World, branch: &str) -> String {
    git(world, &service(world).origin, &["rev-parse", branch])
        .trim()
        .to_owned()
}

/// The commits a branch gained between `from` and `to` that are not merges, on the
/// origin: its own first-parent line, so what a merge brought in from the parent
/// is the parent's and not counted. A restack that only merges adds none.
fn non_merges(world: &World, from: &str, to: &str) -> Vec<String> {
    git(
        world,
        &service(world).origin,
        &[
            "rev-list",
            "--first-parent",
            "--no-merges",
            &format!("{from}..{to}"),
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

/// The hunk a review thread on `path`'s first line sits in, in the change request
/// from `head` into `base`: GitHub marks a thread outdated when the diff hunk it was
/// left on changes, so a thread is still on its line exactly when this is unchanged.
fn thread_hunk(world: &World, base: &str, head: &str, path: &str) -> String {
    let diff = git(
        world,
        &service(world).origin,
        &[
            "diff",
            "--no-color",
            &format!("{base}...{head}"),
            "--",
            path,
        ],
    );
    diff.split_once("\n@@")
        .map(|(_, hunk)| format!("@@{hunk}"))
        .unwrap_or_else(|| panic!("{path} has no hunk in {base}...{head}:\n{diff}"))
}

/// The position of the first event of `kind` labelled `node` in the run's journal,
/// and of any event at all where `kind` is `None`.
fn position(journal: &[Value], kind: &str, node: &str) -> usize {
    journal
        .iter()
        .position(|event| event["kind"] == kind && event["labels"]["node"] == node)
        .unwrap_or_else(|| panic!("no {kind} of {node} in the journal"))
}

/// A rework at the bottom of a stack of drafts moves every draft above it by merges
/// alone: `a ← b ← c` and `a, d ← e`, reworked at `a`. `e`'s stack base is advanced
/// before `e` is restacked, `b` and `e` — independent of each other — are restacked
/// concurrently, and `c` only after `b` pushed; no agent and no drafter is dispatched
/// for any of them; every branch only grows, by merge commits, with no forced push;
/// each change request keeps its base and shows only its own change; a review thread
/// seeded on `b` stays on its line; and every draft is a draft again at the end.
#[test]
fn a_rework_restacks_every_draft_stacked_on_it_by_merges_alone() {
    let world = team_world("rework-stack");
    let repo = service(&world);
    for id in ["a", "b", "c", "d", "e"] {
        world.script(&format!("{id}.work"), &format!("{id}'s change\n"));
    }
    world.script("a-rework-1.work", "a's change, as the review asked\n");
    world.script("a-rework-1.work-file", "a");
    world.script("pr-author.title-check", KEEPS);
    let run = "stack";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("d", &[]),
            lifecycle("b", &["a"]),
            lifecycle("c", &["b"]),
            lifecycle("e", &["a", "d"]),
        ],
        3,
        &["--pr-author-graph", &drafter],
    );
    let before: std::collections::BTreeMap<&str, Value> = ["a", "b", "c", "d", "e"]
        .into_iter()
        .map(|id| (id, settled(&world, run, id)))
        .collect();
    for (id, settlement) in &before {
        assert_eq!(
            settlement["outcome"], "change-review-draft",
            "{id}: {settlement}"
        );
    }
    let branch = |id: &str| text(&before[id]["branch"], "a branch").to_owned();
    let head = |id: &str| text(&before[id]["head"], "a head").to_owned();
    let stack_base = text(&before["e"]["base"], "e's stack base").to_owned();
    let stack_base_before = head_of(&world, &stack_base);
    let bases: std::collections::BTreeMap<&str, Value> = ["b", "c", "e"]
        .into_iter()
        .map(|id| (id, change_from(&world, &branch(id))["base"].clone()))
        .collect();
    // A review thread on `b`'s first line, as its change request shows it now.
    let thread = thread_hunk(&world, &branch("a"), &branch("b"), "b.md");
    // Every push the origin takes from here on has to fast-forward.
    git(
        &world,
        &repo.origin,
        &["config", "receive.denyNonFastForwards", "true"],
    );
    let journal_before = world.journal(run).len();

    reply(
        &world,
        run,
        json!([rework("a", "say what the review asked in a.md")]),
    )
    .exited(0);
    adopt(&world, run).exited(0);

    let journal = world.journal(run);
    let a_new = text(
        &settled(&world, run, "a-rework-1")["head"],
        "the rework's head",
    )
    .to_owned();
    let restacked: std::collections::BTreeMap<&str, Value> = ["b", "c", "e"]
        .into_iter()
        .map(|id| {
            let record = world
                .events_of(run, "node-restacked")
                .into_iter()
                .find(|event| event["labels"]["node"] == id)
                .unwrap_or_else(|| panic!("{id} was not restacked: {:#?}", world.kinds(run)));
            (id, record["payload"].clone())
        })
        .collect();
    for (id, record) in &restacked {
        assert_eq!(record["agent_dispatched"], false, "{id}: {record}");
        assert_eq!(record["outcome"], "change-review-draft", "{id}: {record}");
        // No worker and no drafter was dispatched for it again.
        assert_eq!(
            world
                .events_of(run, "node-dispatched")
                .iter()
                .filter(|event| event["labels"]["node"] == *id)
                .count(),
            1,
            "{id} was dispatched again"
        );
        assert_eq!(tasks_of(&world, run, id, "engineer").len(), 1, "{id}");
        assert_eq!(tasks_of(&world, run, id, "pr-author").len(), 1, "{id}");
        // Its branch grew from where it stood, by merges alone.
        let new = head_of(&world, &branch(id));
        assert_eq!(record["head"], new.as_str(), "{id}");
        assert!(is_ancestor(&world, &head(id), &new), "{id} was rewritten");
        assert_eq!(
            non_merges(&world, &head(id), &new),
            Vec::<String>::new(),
            "{id}: {}",
            git(
                &world,
                &repo.origin,
                &["log", "--graph", "--format=%h %s %p", &branch(id)]
            )
        );
        // Its change request kept its base and shows only its own change, and is a
        // draft again.
        let change = change_from(&world, &branch(id));
        assert_eq!(change["base"], bases[id], "{id}");
        assert_eq!(
            changed_against(&world, &repo, text(&change["base"], "base"), &branch(id)),
            [format!("{id}.md")],
            "{id}"
        );
        assert!(held_as_draft(&world, &change["number"]), "{id}: {change}");
        assert_eq!(opened_from(&world, &branch(id)), 1, "{id}");
    }
    assert!(held_as_draft(
        &world,
        &change_from(&world, &branch("a"))["number"]
    ));
    assert!(held_as_draft(
        &world,
        &change_from(&world, &branch("d"))["number"]
    ));

    // `b` and `e` were restacked side by side: each started before either ended.
    let started = |id: &str| position(&journal, "node-restack-started", id);
    let ended = |id: &str| position(&journal, "node-restacked", id);
    assert!(started("b") > journal_before && started("e") > journal_before);
    assert!(
        started("b") < ended("e") && started("e") < ended("b"),
        "{:#?}",
        world.kinds(run)
    );
    // `b` onto the rework's head, and `c` only once `b` had pushed its merge — onto
    // that very head.
    assert_eq!(
        journal[started("b")]["payload"]["onto"],
        json!([{"branch": branch("a"), "head": a_new}])
    );
    let b_new = text(&restacked["b"]["head"], "b's new head").to_owned();
    let b_pushed = journal
        .iter()
        .position(|event| {
            event["kind"] == "push"
                && event["labels"]["node"] == "b"
                && event["payload"]["head"] == b_new.as_str()
        })
        .expect("b's restack pushed its merge");
    assert!(b_pushed < started("c"), "c was restacked before b pushed");
    assert_eq!(
        journal[started("c")]["payload"]["onto"],
        json!([{"branch": branch("b"), "head": b_new}])
    );
    // `e`'s stack base was advanced onto the rework before `e` merged it.
    let stack_base_after = head_of(&world, &stack_base);
    assert_ne!(stack_base_after, stack_base_before);
    assert!(is_ancestor(&world, &a_new, &stack_base_after));
    assert!(is_ancestor(
        &world,
        &stack_base_after,
        &head_of(&world, &branch("e"))
    ));
    assert_eq!(
        journal[started("e")]["payload"]["onto"],
        json!([
            {"branch": branch("a"), "head": a_new},
            {"branch": branch("d"), "head": head("d")},
        ])
    );

    // The origin took nothing but fast-forwards.
    for push in world.events_of(run, "push") {
        assert_eq!(push["payload"]["accepted"], true, "{push}");
        assert!(
            !text(&push["payload"]["output"], "push output").contains("forced update"),
            "{push}"
        );
    }
    // The review thread on `b` is still on its line, in an unchanged hunk.
    assert_eq!(
        thread_hunk(&world, &branch("a"), &branch("b"), "b.md"),
        thread,
        "the thread on b was outdated"
    );
}

/// A rework that conflicts with the draft above it: `b`'s own worker is dispatched
/// with the merge in progress, and once it concludes it `b` republishes on its
/// existing change request — drafted again, since its content changed — and settles
/// as a review draft; `c` is then restacked onto `b`'s new head with no agent.
#[test]
fn a_rework_conflicting_with_the_draft_above_is_concluded_by_that_draft_s_worker() {
    let world = team_world("rework-conflict");
    world.script("a.work", "a wrote this\n");
    world.script("a.work-file", "shared");
    world.script("b.work", "b changed this\n");
    world.script("b.work-file", "shared");
    world.script("b.resolves-merge", "a reworked this, and b changed it\n");
    world.script("c.work", "c's change\n");
    world.script("a-rework-1.work", "a reworked this\n");
    world.script("a-rework-1.work-file", "shared");
    world.script("pr-author.title-check", KEEPS);
    let run = "conflict";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &["a"]),
            lifecycle("c", &["b"]),
        ],
        3,
        &["--pr-author-graph", &drafter],
    );
    let b_before = settled(&world, run, "b");
    let b_branch = text(&b_before["branch"], "b's branch").to_owned();
    let c_before = settled(&world, run, "c");
    // `b`'s worker is asked only to conclude the merge from here: its drafter runs
    // under the same script key, and must not write `b`'s first change back over it.
    world.unscript("b.work");

    reply(&world, run, json!([rework("a", "rewrite the shared file")])).exited(0);
    adopt(&world, run).exited(0);

    // `b`'s worker was handed its merge with the rework, in its own session.
    let tasks = tasks_of(&world, run, "b", "engineer");
    assert_eq!(tasks.len(), 2, "{tasks:#?}");
    assert!(
        tasks[1].contains("opened with a merge in progress") && tasks[1].contains("shared.md"),
        "{}",
        tasks[1]
    );
    let b = settled(&world, run, "b");
    assert_eq!(b["status"], "done", "{b}");
    assert_eq!(b["outcome"], "change-review-draft", "{b}");
    let b_restacked = world
        .events_of(run, "node-restacked")
        .into_iter()
        .find(|event| event["labels"]["node"] == "b")
        .expect("b was restacked");
    assert_eq!(b_restacked["payload"]["agent_dispatched"], true);
    // Republished onto its own change request, drafted again, and a draft.
    assert_eq!(opened_from(&world, &b_branch), 1);
    assert_eq!(tasks_of(&world, run, "b", "pr-author").len(), 2);
    let change = change_from(&world, &b_branch);
    assert!(held_as_draft(&world, &change["number"]), "{change}");
    assert_eq!(b["change_url"], b_before["change_url"], "{b}");
    let b_new = text(&b["head"], "b's new head").to_owned();
    assert_eq!(
        crate::stacked_drafts::file_on(&world, &service(&world), &b_branch, "shared.md"),
        "a reworked this, and b changed it"
    );

    // `c` followed `b`'s new head, merging it with no agent.
    let c_started = world
        .events_of(run, "node-restack-started")
        .into_iter()
        .find(|event| event["labels"]["node"] == "c")
        .expect("c was restacked");
    assert_eq!(
        c_started["payload"]["onto"],
        json!([{"branch": b_branch, "head": b_new}])
    );
    let c_restacked = world
        .events_of(run, "node-restacked")
        .into_iter()
        .find(|event| event["labels"]["node"] == "c")
        .expect("c published its restack");
    assert_eq!(c_restacked["payload"]["agent_dispatched"], false);
    assert_eq!(tasks_of(&world, run, "c", "engineer").len(), 1);
    assert_eq!(tasks_of(&world, run, "c", "pr-author").len(), 1);
    assert!(is_ancestor(
        &world,
        text(&c_before["head"], "c's head"),
        text(&c_restacked["payload"]["head"], "c's new head")
    ));
    let status = world.run(&["status", run]);
    status.out_has(&format!(
        "b: restacked onto a-rework-1's branch {} at ",
        text(&settled(&world, run, "a-rework-1")["branch"], "a's branch")
    ));
    status.out_has(", with an agent turn");
}

/// A draft is restacked once its reworked parent has pushed, and not once its
/// parent's checks have settled: the rework's checks are held pending until the
/// draft above it has started its restack.
#[test]
fn a_restack_starts_before_its_reworked_parent_s_checks_have_settled() {
    let world = team_world("rework-early").with_env("ONEVCS_CHECKS_TIMEOUT_SECONDS", "120");
    world.script("a.work", "a's change\n");
    world.script("b.work", "b's change\n");
    world.script("a-rework-1.work", "a's change, as the review asked\n");
    world.script("a-rework-1.work-file", "a");
    world.script("pr-author.title-check", KEEPS);
    let run = "early";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
        2,
        &["--pr-author-graph", &drafter],
    );
    let a_branch = text(&settled(&world, run, "a")["branch"], "a's branch").to_owned();
    let a_number = change_from(&world, &a_branch)["number"].clone();
    // The rework's change request reports its required check running, until the
    // journey says otherwise.
    world.script(
        &format!("gh.checks.{a_number}"),
        "lint in_progress - required",
    );

    reply(
        &world,
        run,
        json!([rework("a", "say what the review asked in a.md")]),
    )
    .exited(0);
    let mut driving = world.cmd(&["adopt", run]);
    driving
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let driver = driving.spawn().expect("the adopting driver starts");
    world.until("b's restack to start", |world| {
        !world.events_of(run, "node-restack-started").is_empty()
    });
    // The rework had pushed and has not settled: its checks are still running.
    assert!(
        !world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "a-rework-1"),
        "the rework settled before the restack started"
    );
    assert!(world
        .events_of(run, "push")
        .iter()
        .any(|event| event["labels"]["node"] == "a-rework-1"));
    world.script(
        &format!("gh.checks.{a_number}"),
        crate::draft_lifecycle::GREEN,
    );
    let ended = driver.wait_with_output().expect("the driver ends");
    assert!(ended.status.success(), "{ended:?}");
    let journal = world.journal(run);
    assert!(
        position(&journal, "node-restack-started", "b")
            < position(&journal, "node-settled", "a-rework-1")
    );
    assert_eq!(
        settled(&world, run, "a-rework-1")["outcome"],
        "change-review-draft"
    );
    assert_eq!(settled(&world, run, "b")["outcome"], "change-review-draft");
}
