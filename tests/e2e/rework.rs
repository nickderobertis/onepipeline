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

use crate::harness::{double, git, lifecycle, plan_of, World};
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
    onepipeline_testfakes::profile::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, of])
        .current_dir(service(world).origin)
        .status()
        .expect("git runs")
        .success()
}

/// Make the host refuse every standings read of the service repository.
fn cold_reads_fail(world: &World) {
    world.script("gh.standings-outage.owner.service", "");
}

#[test]
fn a_cold_republication_without_a_head_record_watches_the_same_draft() {
    let world = team_world("rework-cold-watch");
    world.script("a.work", "first draft\n");
    let project = launched(&world, "coldwatch", vec![lifecycle("a", &[])], 1, &[]);
    let original = settled(&world, "coldwatch", "a");
    let branch = text(&original["branch"], "original branch");
    // Every standings read the cold host's triggers make fails, so nothing on it
    // records which branch the draft carries.
    cold_reads_fail(&world);
    let cold = world.on_host("without-change-heads");
    cold.run_from(&cold.project, &["start", &project, "--attach"])
        .exited(0);
    assert!(!cold.onevcs_home().join("change-heads").exists());
    cold.script("a-rework-1.work", "changed on the cold host\n");
    cold.script("a-rework-1.work-file", "a");
    reply(
        &cold,
        "coldwatch",
        json!([rework("a", "address the review on this host")]),
    )
    .exited(0);
    let before = cold.invocations().len();
    let output = adopt(&cold, "coldwatch");
    output
        .exited(0)
        .err_has("no local head-branch record; watching its existing draft instead");
    let replacement = settled(&cold, "coldwatch", "a-rework-1");
    assert_eq!(replacement["outcome"], "change-review-draft");
    assert_eq!(replacement["change_url"], original["change_url"]);
    assert_eq!(replacement["branch"], original["branch"]);
    assert_eq!(opened_from(&cold, branch), 1);
    assert!(held_as_draft(&cold, &change_from(&cold, branch)["number"]));
    assert!(cold
        .invocations()
        .iter()
        .skip(before)
        .any(|call| call["tool"] == "gh"
            && call["args"][0] == "pr"
            && call["args"][1] == "checks"));
    assert!(is_ancestor(
        &cold,
        text(&original["head"], "original head"),
        text(&replacement["head"], "replacement head")
    ));
}

/// A republication of a review draft settles `change-review-draft-unchecked` at its push,
/// watching no checks; the next trigger reads its verdict, and a green one reads as the
/// review draft it is from then on.
fn read_green(world: &World, run: &str, nodes: &[&str]) {
    for node in nodes {
        let settlement = settled(world, run, node);
        assert_eq!(
            settlement["outcome"], "change-review-draft-unchecked",
            "{node}: {settlement}"
        );
    }
    world.run(&["readjust", run]).exited(0);
    for node in nodes {
        let read = world
            .events_of(run, "node-checks-read")
            .into_iter()
            .rev()
            .find(|event| event["labels"]["node"] == *node)
            .unwrap_or_else(|| panic!("no trigger read {node}'s verdict"));
        assert_eq!(read["payload"]["verdict"], "passed", "{node}: {read}");
    }
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
    let publication_calls = world.invocations();
    assert!(
        publication_calls.iter().any(|call| call["tool"] == "gh"
            && call["args"][0] == "pr"
            && call["args"][1] == "checks"),
        "first publications must watch their checks"
    );
    let before_rework = publication_calls.len();

    reply(
        &world,
        run,
        json!([rework("a", "say what the review asked in a.md")]),
    )
    .exited(0);
    adopt(&world, run).exited(0);
    let republication_calls = world.invocations();
    assert!(
        republication_calls.iter().skip(before_rework).all(|call| {
            call["tool"] != "gh" || !(call["args"][0] == "pr" && call["args"][1] == "checks")
        }),
        "a rework and its clean restack must defer their check watches"
    );

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
        replacement["outcome"], "change-review-draft-unchecked",
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
            // Where the rework's own commits begin, so a host continuing the lineage can
            // tell them from the original worker's (entry 130).
            "replaces_head": a_head,
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

    // The projected rework and its lineage survive a launch on a cold host:
    // the store's settlement is continued, with no new worker or change request.
    let cold = world.on_host("rework-cold");
    cold.run_from(&cold.project, &["start", &project, "--attach"])
        .exited(0);
    assert!(cold.events_of(run, "node-dispatched").is_empty());
    let continued = settled(&cold, run, "a");
    assert_eq!(continued["continued"]["source"], "store", "{continued}");
    assert_eq!(continued["branch"], replacement["branch"], "{continued}");
    assert_eq!(continued["head"], replacement["head"], "{continued}");
    assert_eq!(opened_from(&world, &a_branch), 1);
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

/// Leave a review thread on `line` of `path` in `change` — a change request as the
/// host recorded it — on the diff it shows now: its base's tip and its head's.
fn seed_thread(world: &World, change: &Value, path: &str, line: u32) {
    let thread = json!({
        "path": path,
        "line": line,
        "base": head_of(world, text(&change["base"], "a base")),
        "head": head_of(world, text(&change["head"], "a head")),
        "body": "say what this line is for",
    });
    world.script(
        &format!("gh.review-threads.{}", change["number"]),
        &format!("{thread}\n"),
    );
}

/// Every review thread comment on `change`, as `onevcs`'s own review read answers
/// it through the host: where each thread sits, and whether it is outdated.
fn review_threads(world: &World, change: &Value) -> Vec<Value> {
    let url = format!(
        "https://github.com/{}/pull/{}",
        text(&change["repo"], "a repository"),
        change["number"]
    );
    let read = world
        .cmd_on(&double("review-read"), &[&url])
        .output()
        .expect("review-read runs");
    assert!(
        read.status.success(),
        "the review read of {url} failed: {}",
        String::from_utf8_lossy(&read.stderr)
    );
    let comments: Vec<Value> =
        serde_json::from_slice(&read.stdout).expect("the review read answers JSON");
    comments
        .into_iter()
        .filter(|comment| comment["kind"]["type"] == "review-thread")
        .collect()
}

/// The position of the first event of `kind` labelled `node` in the run's journal.
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
/// seeded on `b` stays on its line, while one on the line of `a` the rework rewrote
/// is outdated; and every draft is a draft again at the end.
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
    seed_thread(&world, &change_from(&world, &branch("b")), "b.md", 1);
    let on_line = |threads: Vec<Value>| {
        assert_eq!(threads.len(), 1, "{threads:#?}");
        let kind = &threads[0]["kind"];
        assert_eq!(kind["path"], "b.md", "{kind}");
        assert_eq!(kind["line"], 1, "the thread on b is on no line: {kind}");
        assert_eq!(
            kind["outdated"], false,
            "the thread on b was outdated: {kind}"
        );
    };
    on_line(review_threads(&world, &change_from(&world, &branch("b"))));
    // And one on the line of `a.md` the rework is about to rewrite.
    seed_thread(&world, &change_from(&world, &branch("a")), "a.md", 1);
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
        assert_eq!(
            record["outcome"], "change-review-draft-unchecked",
            "{id}: {record}"
        );
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
    // What moved each draft: the rework moved `b` and `e`, and `b`'s own restack `c`.
    for (id, cause) in [("b", "rework"), ("e", "rework"), ("c", "restack")] {
        assert_eq!(journal[started(id)]["payload"]["cause"], cause, "{id}");
    }
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
    // The review thread on `b` is still on its line and not outdated, as the host
    // reports it through `onevcs`'s review read.
    on_line(review_threads(&world, &change_from(&world, &branch("b"))));
    // The one on the line of `a` the rework rewrote is outdated, and on no line.
    let on_a = review_threads(&world, &change_from(&world, &branch("a")));
    assert_eq!(on_a.len(), 1, "{on_a:#?}");
    let kind = &on_a[0]["kind"];
    assert_eq!(kind["path"], "a.md", "{kind}");
    assert_eq!(
        kind["outdated"], true,
        "the thread on a was not outdated: {kind}"
    );
    assert_eq!(
        kind["line"],
        Value::Null,
        "the thread on a is on a line: {kind}"
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
    read_green(&world, run, &["b"]);
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
///
/// What settles the rework's checks is the trigger that reads their verdict — its
/// `node-checks-read` — and not its node's settlement: a rework republished as a
/// review draft settles `change-review-draft-unchecked` at its push, on its
/// dispatch's thread, while the restack starts on the driver's next pass, and
/// nothing orders those two. So the order held here is the restack's start against
/// the verdict, which only the trigger this journey runs once the checks have
/// passed can read.
///
/// And `status` is read while the restack is still running because its push is
/// held: unchecked, the restack settles at that push too, a moment after it
/// starts, which no poll of `status` is guaranteed to land inside. The rework's
/// push and the restack's go through one hook, so the hook is switched to the
/// restack's hold while the rework's push is the one it is holding.
#[test]
fn a_restack_starts_before_its_reworked_parent_s_checks_have_settled() {
    let world = World::new("rework-early")
        .with_env("ONEVCS_BRANCH_PREFIX", crate::stacked_drafts::PREFIX)
        .with_env("ONEVCS_CHECKS_TIMEOUT_SECONDS", "120");
    let rework_push = crate::harness::held_publication(&world, &world.fakes.join("rework.go"));
    // The first settlement's pushes go straight through.
    rework_push.release();
    world.repository("change-open", &rework_push.argv());
    crate::draft_lifecycle::rules(&world, "change-open", "required", None);
    world.script("gh.checks", crate::draft_lifecycle::GREEN);
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
    // The rework's change request and the draft's above it report their required
    // check running, until the journey says otherwise.
    let b_number = change_from(
        &world,
        text(&settled(&world, run, "b")["branch"], "b's branch"),
    )["number"]
        .clone();
    for number in [&a_number, &b_number] {
        world.script(
            &format!("gh.checks.{number}"),
            "lint in_progress - required",
        );
    }

    rework_push.rearm();
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
    world.until("the rework's push to be held", |_| rework_push.holding());
    let restack_push = crate::harness::held_publication(&world, &world.fakes.join("restack.go"));
    crate::harness::hold_later_pushes_at(&world, &restack_push);
    rework_push.release();
    world.until("b's restack to start", |world| {
        !world.events_of(run, "node-restack-started").is_empty()
    });
    // The rework had pushed and its checks have not settled: they are still
    // running, no verdict on them has been read, and if the rework's node has
    // settled already it settled without one.
    assert!(
        !world
            .events_of(run, "node-checks-read")
            .iter()
            .any(|event| event["labels"]["node"] == "a-rework-1"),
        "the rework's checks settled before the restack started"
    );
    for settled in world
        .events_of(run, "node-settled")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "a-rework-1")
    {
        assert_eq!(
            settled["payload"]["outcome"], "change-review-draft-unchecked",
            "the rework settled on its checks before the restack started: {settled}"
        );
    }
    assert!(world
        .events_of(run, "push")
        .iter()
        .any(|event| event["labels"]["node"] == "a-rework-1"));
    // `status` reads the draft as restacking onto the rework's pushed head while
    // its own publication is still under way.
    world.until("b's restack to reach its push", |_| restack_push.holding());
    let a_pushed = world
        .events_of(run, "node-restack-started")
        .first()
        .map(|event| event["payload"]["onto"][0]["head"].clone())
        .expect("the restack names the head");
    world.until("b's restack to read as running", |world| {
        world.run(&["status", run]).stdout.contains(&format!(
            "; restacking onto {a_branch} at {}",
            text(&a_pushed, "the head")
        ))
    });
    restack_push.release();
    for number in [&a_number, &b_number] {
        world.script(
            &format!("gh.checks.{number}"),
            crate::draft_lifecycle::GREEN,
        );
    }
    let ended = driver.wait_with_output().expect("the driver ends");
    assert!(ended.status.success(), "{ended:?}");
    read_green(&world, run, &["a-rework-1", "b"]);
    let journal = world.journal(run);
    assert!(
        position(&journal, "node-restack-started", "b")
            < position(&journal, "node-checks-read", "a-rework-1")
    );
}

/// A `retitle` reaching a run while its driver is live is staged by that driver
/// and applied through the host as it is to a run nothing is driving: `c` is
/// retitled while `a`'s rework waits on its checks.
#[test]
fn a_retitle_reaching_a_driven_run_is_applied_by_its_driver() {
    let world = team_world("retitle-driven").with_env("ONEVCS_CHECKS_TIMEOUT_SECONDS", "120");
    world.script("a.work", "a's change\n");
    world.script("c.work", "c's change\n");
    world.script("a-rework-1.work", "a's change, as the review asked\n");
    world.script("a-rework-1.work-file", "a");
    world.script("pr-author.title-check", KEEPS);
    let run = "retitle-driven";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("c", &[])],
        2,
        &["--pr-author-graph", &drafter],
    );
    let a_branch = text(&settled(&world, run, "a")["branch"], "a's branch").to_owned();
    let c_branch = text(&settled(&world, run, "c")["branch"], "c's branch").to_owned();
    let a_number = change_from(&world, &a_branch)["number"].clone();
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
    world.until("the rework to push", |world| {
        world
            .events_of(run, "push")
            .iter()
            .any(|event| event["labels"]["node"] == "a-rework-1")
    });
    // The driver is live: the rework is waiting on its checks, and has not settled.
    assert!(!world
        .events_of(run, "node-settled")
        .iter()
        .any(|event| event["labels"]["node"] == "a-rework-1"));

    reply(
        &world,
        run,
        json!([{"op": "retitle", "id": "c", "title": "fix(c): say what c does",
                "reason": "the reviewer read the title as wrong"}]),
    )
    .exited(0);
    world.until("the driver to retitle c", |world| {
        !retitled(world, run, "c").is_empty()
    });
    assert_eq!(
        retitled(&world, run, "c"),
        [
            json!({"node": "c", "from": "feat: ship c", "to": "fix(c): say what c does", "by": "retitle"})
        ]
    );
    assert_eq!(
        change_from(&world, &c_branch)["title"],
        "fix(c): say what c does"
    );

    world.script(
        &format!("gh.checks.{a_number}"),
        crate::draft_lifecycle::GREEN,
    );
    let ended = driver.wait_with_output().expect("the driver ends");
    assert!(ended.status.success(), "{ended:?}");
    read_green(&world, run, &["a-rework-1"]);
    assert_eq!(
        change_from(&world, &c_branch)["title"],
        "fix(c): say what c does"
    );
    assert_eq!(change_from(&world, &a_branch)["title"], "feat: ship a");
}

/// The title a node of the cache journey opened under.
const CACHE_TITLE: &str = "feat(cache): add a read-through cache to the plan reader";

/// The subject the drafter answers that change outgrew it with.
const RETRY_TITLE: &str = "feat(reader): retry a failed plan read";

/// Commit `file` holding `body` onto the origin's `main`, as the base every node of
/// a journey is cut from.
fn on_the_base(world: &World, file: &str, body: &str) {
    let checkout = service(world).checkout;
    std::fs::write(checkout.join(file), body).expect("the base file is written");
    git(world, &checkout, &["add", "-A"]);
    git(world, &checkout, &["commit", "-m", "feat: the plan reader"]);
    git(world, &checkout, &["push", "origin", "main"]);
}

/// The `change-retitled` records the run journalled about `node`.
fn retitled(world: &World, run: &str, node: &str) -> Vec<Value> {
    world
        .events_of(run, "change-retitled")
        .into_iter()
        .filter(|event| event["labels"]["node"] == node)
        .map(|event| event["payload"].clone())
        .collect()
}

/// The `title-unverified` records the run journalled about `node`.
fn unverified(world: &World, run: &str, node: &str) -> Vec<Value> {
    world
        .events_of(run, "title-unverified")
        .into_iter()
        .filter(|event| event["labels"]["node"] == node)
        .map(|event| event["payload"].clone())
        .collect()
}

/// The section of a drafting task between two of its headings.
fn section<'a>(task: &'a str, heading: &str, next: &str) -> &'a str {
    let after = task
        .split_once(&format!("\n{heading}\n"))
        .unwrap_or_else(|| panic!("the task has no {heading}:\n{task}"))
        .1;
    after
        .split_once(&format!("\n{next}\n"))
        .map_or(after, |(within, _)| within)
}

/// A rework whose net change no longer matches its title: the drafter is shown the
/// change's real patch against its base beside the title, answers `replaced`, and
/// the new subject is written onto the change request, journalled as the drafter's,
/// moved onto the node's title in the graph and projected onto the store — and a
/// later restack republishing that node keeps it.
#[test]
fn a_rework_whose_change_outgrew_its_title_is_retitled_by_the_drafter() {
    let world = team_world("rework-title");
    on_the_base(&world, "reader.md", "read the plan from disk\n");
    world.script("p.work", "p's change\n");
    world.script("a.work-file", "reader");
    world.script("a.work", "read the plan through a read-through cache\n");
    world.script("a-rework-1.work-file", "reader");
    world.script(
        "a-rework-1.work",
        "read the plan from disk, retrying a read that fails\n",
    );
    world.script(
        "pr-author.title-check",
        &json!({"verdict": "replaced", "title": RETRY_TITLE}).to_string(),
    );
    let mut a = lifecycle("a", &["p"]);
    a["title"] = json!(CACHE_TITLE);
    let run = "title";
    let drafter = world.pr_author_graph();
    let project = launched(
        &world,
        run,
        vec![lifecycle("p", &[]), a],
        2,
        &["--pr-author-graph", &drafter],
    );
    let a_branch = text(&settled(&world, run, "a")["branch"], "a's branch").to_owned();
    assert_eq!(change_from(&world, &a_branch)["title"], CACHE_TITLE);
    // An ordinary publication reads no verdict, whatever its drafter answered.
    assert!(world.events_of(run, "change-retitled").is_empty());

    reply(
        &world,
        run,
        json!([rework(
            "a",
            "remove the cache and retry a failed read instead"
        )]),
    )
    .exited(0);
    adopt(&world, run).exited(0);

    // The drafter was shown the title beside the change's actual patch.
    let drafting = tasks_of(&world, run, "a-rework-1", "pr-author");
    assert_eq!(drafting.len(), 1, "{drafting:#?}");
    let task = &drafting[0];
    assert!(task.contains("\n## Rework\n"), "{task}");
    assert_eq!(
        section(task, "## Current title", "## Net diff").trim(),
        CACHE_TITLE
    );
    let net = section(task, "## Net diff", "## Title check");
    assert!(net.contains("```diff"), "{net}");
    assert!(
        net.contains("--- a/reader.md") && net.contains("+++ b/reader.md"),
        "{net}"
    );
    assert!(net.contains("@@ "), "no hunk header: {net}");
    assert!(net.contains("\n-read the plan from disk\n"), "{net}");
    assert!(
        net.contains("\n+read the plan from disk, retrying a read that fails\n"),
        "{net}"
    );
    assert!(!net.contains("cache"), "the net diff names a cache: {net}");
    assert!(task.contains("`title_check`"), "{task}");

    // The drafter's subject is on the change request, on the record, and in the graph.
    assert_eq!(change_from(&world, &a_branch)["title"], RETRY_TITLE);
    assert_eq!(
        retitled(&world, run, "a-rework-1"),
        [json!({"node": "a-rework-1", "from": CACHE_TITLE, "to": RETRY_TITLE, "by": "drafter"})]
    );
    assert!(unverified(&world, run, "a-rework-1").is_empty());
    world.until_store("the new title to reach the store", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "a"
                && task["item"]["title"] == RETRY_TITLE
        })
    });

    // A restack republishing it later keeps the subject: `p` is reworked, and `a`'s
    // replacement is brought onto it without an agent and keeps its title.
    world.script("pr-author.title-check", KEEPS);
    world.script("p-rework-1.work", "p's change, reworked\n");
    world.script("p-rework-1.work-file", "p");
    reply(&world, run, json!([rework("p", "tighten p")])).exited(0);
    adopt(&world, run).exited(0);
    let restacked = world
        .events_of(run, "node-restacked")
        .into_iter()
        .find(|event| event["labels"]["node"] == "a-rework-1")
        .expect("a's replacement was restacked");
    assert_eq!(restacked["payload"]["agent_dispatched"], false);
    assert_eq!(change_from(&world, &a_branch)["title"], RETRY_TITLE);
    assert_eq!(retitled(&world, run, "a-rework-1").len(), 1);
    assert!(world.events_of(run, "title-unverified").is_empty());
}

/// The op's title outranks the drafter's, and a `keeps` leaves the title alone:
/// `x` is reworked stating a title while its drafter answers `replaced` with another,
/// `y` is reworked stating none while its drafter answers `keeps`, and `z`'s drafter
/// answers `replaced` with the title it already has.
#[test]
fn the_op_s_title_outranks_the_drafter_s_and_a_keeps_leaves_the_title_alone() {
    let world = team_world("rework-precedence");
    for id in ["x", "y", "z"] {
        world.script(&format!("{id}.work"), &format!("{id}'s change\n"));
        world.script(
            &format!("{id}-rework-1.work"),
            &format!("{id}'s change, reworked\n"),
        );
        world.script(&format!("{id}-rework-1.work-file"), id);
    }
    // `z`'s rework writes a file its branch never carried, and the fake worker
    // commits nothing: the drafter reads the worktree before the publication
    // commits it, so the new file is untracked when the net diff is taken.
    world.script("z-rework-1.work-file", "z-retry");
    world.script(
        "pr-author.title-check",
        r#"{"verdict": "replaced", "title": "feat(x): what the drafter would say"}"#,
    );
    // `x`'s rework names its replacement.
    world.script("x-again.work", "x's change, reworked\n");
    world.script("x-again.work-file", "x");
    let run = "precedence";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![
            lifecycle("x", &[]),
            lifecycle("y", &[]),
            lifecycle("z", &[]),
        ],
        3,
        &["--pr-author-graph", &drafter],
    );
    let branch = |id: &str| text(&settled(&world, run, id)["branch"], "a branch").to_owned();

    let mut stated = rework("x", "say it the op's way");
    stated["title"] = json!("fix(x): what the op says");
    stated["replacement"] = json!("x-again");
    reply(&world, run, json!([stated])).exited(0);
    adopt(&world, run).exited(0);
    let again = settled(&world, run, "x-again");
    assert_eq!(again["outcome"], "change-review-draft-unchecked", "{again}");
    assert_eq!(again["branch"], branch("x").as_str(), "{again}");
    assert_eq!(
        change_from(&world, &branch("x"))["title"],
        "fix(x): what the op says"
    );
    assert_eq!(
        retitled(&world, run, "x-again"),
        [
            json!({"node": "x-again", "from": "feat: ship x", "to": "fix(x): what the op says", "by": "op"})
        ]
    );

    world.script("pr-author.title-check", KEEPS);
    reply(&world, run, json!([rework("y", "say it again")])).exited(0);
    adopt(&world, run).exited(0);
    assert_eq!(
        settled(&world, run, "y-rework-1")["outcome"],
        "change-review-draft-unchecked"
    );
    assert_eq!(change_from(&world, &branch("y"))["title"], "feat: ship y");
    assert!(retitled(&world, run, "y-rework-1").is_empty());
    assert!(world.events_of(run, "title-unverified").is_empty());

    // A `replaced` naming the title the change request already carries changes
    // nothing, and records nothing.
    world.script(
        "pr-author.title-check",
        r#"{"verdict": "replaced", "title": "feat: ship z"}"#,
    );
    reply(&world, run, json!([rework("z", "say it the same way")])).exited(0);
    adopt(&world, run).exited(0);
    let drafting = tasks_of(&world, run, "z-rework-1", "pr-author");
    assert_eq!(drafting.len(), 1, "{drafting:#?}");
    let net = section(&drafting[0], "## Net diff", "## Title check");
    assert!(
        net.contains("new file mode") && net.contains("+++ b/z-retry.md"),
        "the untracked file is missing from the net diff: {net}"
    );
    assert!(net.contains("\n+z's change, reworked\n"), "{net}");
    assert_eq!(change_from(&world, &branch("z"))["title"], "feat: ship z");
    assert!(
        retitled(&world, run, "z-rework-1").is_empty(),
        "{:#?}\n{:#?}",
        retitled(&world, run, "z-rework-1"),
        world.changes_opened()
    );
    assert!(world.events_of(run, "title-unverified").is_empty());
}

/// [`held_unverified`] for a rework that has the run to itself — a launch with
/// no drafter has one case to show, so nothing is gained by batching it.
fn left_unverified(world: &World, run: &str, id: &str, title: &str, because: &str) {
    reply(world, run, json!([rework(id, "rework it")])).exited(0);
    adopt(world, run).exited(0);
    let shown = Shown::of(world, run);
    held_unverified(world, run, &shown, id, title, because);
}

/// What `status` and `results` print for a run, read once for every node a
/// journey asks about.
struct Shown {
    status: String,
    results: String,
}

impl Shown {
    fn of(world: &World, run: &str) -> Self {
        Self {
            status: world.run(&["status", run]).exited(0).stdout.clone(),
            results: world.run(&["results", run]).exited(0).stdout.clone(),
        }
    }
}

/// Hold that `id`'s rework settled a review draft still under `title`, with that
/// title recorded unverified for a reason containing `because`, shown by `status`,
/// and surfaced to the planner naming the node and the remedy.
fn held_unverified(world: &World, run: &str, shown: &Shown, id: &str, title: &str, because: &str) {
    let replacement = format!("{id}-rework-1");
    let settlement = settled(world, run, &replacement);
    assert_eq!(settlement["status"], "done", "{settlement}");
    assert_eq!(
        settlement["outcome"], "change-review-draft-unchecked",
        "{settlement}"
    );
    let branch = text(&settlement["branch"], "a branch");
    assert_eq!(change_from(world, branch)["title"], title);
    let records = unverified(world, run, &replacement);
    assert_eq!(records.len(), 1, "{records:#?}");
    assert_eq!(records[0]["title"], title);
    assert!(
        text(&records[0]["reason"], "a reason").contains(because),
        "{}",
        records[0]
    );
    assert!(retitled(world, run, &replacement).is_empty());
    let line = format!("{replacement}: title unverified — {title:?} on ");
    assert!(shown.status.contains(&line), "{line:?}:\n{}", shown.status);
    let line = format!("      title unverified — {title:?} on ");
    assert!(
        shown.results.contains(&line),
        "{line:?}:\n{}",
        shown.results
    );
    let surfaced = world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .find(|event| {
            event["payload"]["kind"] == "title-unverified"
                && event["labels"]["node"] == replacement.as_str()
        })
        .unwrap_or_else(|| panic!("no title-unverified surface for {replacement}"));
    assert_eq!(surfaced["payload"]["blocking"], false, "{surfaced}");
    let message = text(&surfaced["payload"]["message"], "the surface's message");
    assert!(
        message.contains(title) && message.contains("`retitle`") && message.contains("/pull/"),
        "{message}"
    );
}

/// A title nothing could verify is left as it is and said to be unverified — for a
/// drafting dispatch that failed, an answer with no `title_check`, an unknown
/// verdict, and a `replaced` subject the title rule refuses — while the node still
/// settles as a review draft; a `retitle` then sets one through the host and clears
/// it, and each case a `retitle` is refused for appends nothing. An ordinary
/// publication whose drafter answered no `title_check` publishes its body as always.
#[test]
fn a_title_nothing_verified_is_left_unverified_until_a_retitle_sets_one() {
    let world = team_world("rework-unverified");
    for id in ["a", "b", "c", "d", "e"] {
        world.script(&format!("{id}.work"), &format!("{id}'s change\n"));
        world.script(
            &format!("{id}-rework-1.work"),
            &format!("{id}'s change, reworked\n"),
        );
        world.script(&format!("{id}-rework-1.work-file"), id);
    }
    world.script("kept.work", "kept's change\n");
    let mut kept = lifecycle("kept", &[]);
    kept["publish"] = json!("preserve");
    // A node whose plan states its change request's body: no drafter reads it.
    let mut planned = lifecycle("e", &[]);
    planned["body"] = json!("## What\nThe plan's own body.");
    let run = "unverified";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &[]),
            lifecycle("c", &[]),
            lifecycle("d", &[]),
            planned,
            kept,
            crate::harness::agent("notes", &[]),
        ],
        7,
        &["--pr-author-graph", &drafter],
    );
    // Published as they always were: the drafter answered a body and no verdict.
    for id in ["a", "b", "c", "d"] {
        let change = change_from(&world, text(&settled(&world, run, id)["branch"], "branch"));
        assert!(
            text(&change["body"], "a body").starts_with(crate::stacked_drafts::DRAFTED),
            "{change}"
        );
    }
    assert!(world.events_of(run, "title-unverified").is_empty());

    // One rework per way a title goes unverified, reworked at once: each drafter
    // answers on its own node's script, so none waits on another's case.
    world.script("a-rework-1.pr-author.fail", "1");
    world.script(
        "c-rework-1.pr-author.title-check",
        r#"{"verdict": "maybe"}"#,
    );
    world.script(
        "d-rework-1.pr-author.title-check",
        r#"{"verdict": "replaced", "title": "make the reader retry"}"#,
    );
    let cases = [
        ("a", "the drafting dispatch"),
        ("b", "carried no `title_check`"),
        ("c", "nothing the schema it was validated against accepted"),
        ("d", "which the title rule refuses"),
        ("e", "the plan states the body"),
    ];
    let reworks: Vec<Value> = cases
        .iter()
        .map(|(id, _)| rework(id, "rework it"))
        .collect();
    reply(&world, run, json!(reworks)).exited(0);
    adopt(&world, run).exited(0);
    let shown = Shown::of(&world, run);
    for (id, because) in cases {
        held_unverified(
            &world,
            run,
            &shown,
            id,
            &format!("feat: ship {id}"),
            because,
        );
    }
    // The remedy: a retitle sets the title through the host and clears it, and
    // leaves the description the change request already carries as it was.
    let a_branch = text(&settled(&world, run, "a-rework-1")["branch"], "a's branch").to_owned();
    let a_body = change_from(&world, &a_branch)["body"].clone();
    assert!(
        a_body.as_str().is_some_and(|body| !body.is_empty()),
        "{a_body}"
    );
    let committed_before = world.events_of(run, "edit-committed").len();
    reply(
        &world,
        run,
        json!([{"op": "retitle", "id": "a-rework-1", "title": "fix(a): say what a does now",
                "reason": "nothing verified the title the rework kept"}]),
    )
    .exited(0);
    assert_eq!(
        change_from(&world, &a_branch)["title"],
        "fix(a): say what a does now"
    );
    assert_eq!(change_from(&world, &a_branch)["body"], a_body);
    assert_eq!(
        retitled(&world, run, "a-rework-1"),
        [
            json!({"node": "a-rework-1", "from": "feat: ship a", "to": "fix(a): say what a does now", "by": "retitle"})
        ]
    );
    assert_eq!(
        world.events_of(run, "edit-committed").len(),
        committed_before + 1
    );
    let status = world.run(&["status", run]);
    assert!(
        !status.stdout.contains("a-rework-1: title unverified"),
        "{}",
        status.stdout
    );
    status.out_has("b-rework-1: title unverified");

    // Each case a retitle is refused for, refused naming it, with nothing appended.
    let committed = world.events_of(run, "edit-committed").len();
    for (command, says) in [
        (
            json!({"op": "retitle", "id": "nope", "title": "fix: x", "reason": "why"}),
            "no node 'nope'",
        ),
        (
            json!({"op": "retitle", "id": "notes", "title": "fix: x", "reason": "why"}),
            "is not a lifecycle node",
        ),
        (
            json!({"op": "retitle", "id": "kept", "title": "fix: x", "reason": "why"}),
            "has no open change request",
        ),
        (
            json!({"op": "retitle", "id": "b-rework-1", "title": "make it retry", "reason": "why"}),
            "Conventional Commit",
        ),
        (
            json!({"op": "retitle", "id": "b-rework-1", "title": "fix: x", "reason": " "}),
            "the reason cannot be blank",
        ),
    ] {
        reply(&world, run, json!([command]))
            .exited(crate::harness::REFUSED)
            .err_has(says);
    }
    assert_eq!(world.events_of(run, "edit-committed").len(), committed);
    assert_eq!(
        change_from(
            &world,
            text(&settled(&world, run, "b-rework-1")["branch"], "b")
        )["title"],
        "feat: ship b"
    );
}

/// A title the host will not take leaves the one it has, unverified — and a
/// `retitle` it will not take is refused with nothing appended.
#[test]
fn a_title_the_host_will_not_take_is_left_unverified_and_its_retitle_refused() {
    let world = team_world("rework-host-refuses");
    world.script("f.work", "f's change\n");
    world.script("f-rework-1.work", "f's change, reworked\n");
    world.script("f-rework-1.work-file", "f");
    let run = "host-refuses";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![lifecycle("f", &[])],
        1,
        &["--pr-author-graph", &drafter],
    );
    world.script("gh.edit-refused", "the change request is locked");
    let mut stated = rework("f", "rework it");
    stated["title"] = json!("fix(f): what the op says");
    reply(&world, run, json!([stated])).exited(0);
    adopt(&world, run).exited(0);
    let f = unverified(&world, run, "f-rework-1");
    assert_eq!(f.len(), 1, "{f:#?}");
    assert!(
        text(&f[0]["reason"], "a reason").contains("did not take the title"),
        "{}",
        f[0]
    );
    let f_branch = text(&settled(&world, run, "f-rework-1")["branch"], "f").to_owned();
    assert_eq!(change_from(&world, &f_branch)["title"], "feat: ship f");
    let committed = world.events_of(run, "edit-committed").len();
    reply(
        &world,
        run,
        json!([{"op": "retitle", "id": "f-rework-1", "title": "fix(f): again", "reason": "why"}]),
    )
    .exited(crate::harness::REFUSED)
    .err_has("would not take the title");
    assert_eq!(world.events_of(run, "edit-committed").len(), committed);
    assert!(retitled(&world, run, "f-rework-1").is_empty());
}

/// A drafter handed the pid its node's worker had — as Windows hands a freed pid
/// straight to the next process — is read as its own answer rather than as the
/// worker's: its body is published and its `title_check` read, on the node's first
/// publication and on its rework's alike. The node's own `title_check` is the one
/// read, over the one every other drafter answers.
#[test]
fn a_drafter_that_reuses_its_workers_pid_is_read_as_itself() {
    let world = team_world("rework-reused-pid");
    world.script("oneagentgraph.pid", "4242");
    world.script("pr-author.title-check", KEEPS);
    world.script("a.work", "a's change\n");
    world.script("a-rework-1.work", "a's change, reworked\n");
    world.script("a-rework-1.work-file", "a");
    world.script(
        "a-rework-1.pr-author.title-check",
        r#"{"verdict": "replaced", "title": "fix(a): say what a does now"}"#,
    );
    let run = "reused-pid";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![lifecycle("a", &[])],
        1,
        &["--pr-author-graph", &drafter],
    );
    let branch = text(&settled(&world, run, "a")["branch"], "a's branch").to_owned();
    assert_eq!(
        change_from(&world, &branch)["body"],
        crate::stacked_drafts::DRAFTED
    );

    reply(&world, run, json!([rework("a", "rework it")])).exited(0);
    adopt(&world, run).exited(0);
    read_green(&world, run, &["a-rework-1"]);
    let change = change_from(&world, &branch);
    assert_eq!(change["body"], crate::stacked_drafts::DRAFTED, "{change}");
    assert_eq!(change["title"], "fix(a): say what a does now", "{change}");
    assert_eq!(
        retitled(&world, run, "a-rework-1"),
        [
            json!({"node": "a-rework-1", "from": "feat: ship a", "to": "fix(a): say what a does now", "by": "drafter"})
        ]
    );
    assert!(world.events_of(run, "title-unverified").is_empty());
    // Every dispatch did stream under the one pid, worker and drafter alike.
    let streams: std::collections::BTreeSet<String> = world
        .journal(run)
        .iter()
        .filter(|event| event["labels"]["node"] == "a-rework-1" && event["source"] == "agentgraph")
        .filter_map(|event| event["stream"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(streams.len(), 2, "{streams:?}");
    assert!(
        streams.iter().all(|stream| stream.ends_with("-4242")),
        "{streams:?}"
    );
}

/// The condition `cross (windows-latest)` hit: a rework's drafter, handed its
/// worker's freed pid, answers with no body. That answer is still read as its
/// own — the node settles a review draft with its title left unverified for the
/// body it did not draft — rather than taken for the worker's report or never
/// read at all.
#[test]
fn a_bodyless_drafter_that_reuses_its_workers_pid_is_read_as_itself() {
    let world = team_world("rework-reused-pid-bodyless");
    world.script("a.work", "a's change\n");
    world.script("a-rework-1.work", "a's change, reworked\n");
    world.script("a-rework-1.work-file", "a");
    let run = "reused-pid-bodyless";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![lifecycle("a", &[])],
        1,
        &["--pr-author-graph", &drafter],
    );
    let branch = text(&settled(&world, run, "a")["branch"], "a's branch").to_owned();
    let body = change_from(&world, &branch)["body"].clone();
    assert_eq!(body, crate::stacked_drafts::DRAFTED);

    // Pin only the rework: the original publication must succeed before the
    // body-less answer exercises reuse of the rework worker's pid.
    world.script("oneagentgraph.pid", "4242");
    world.script("pr-author.bodyless", "1");
    left_unverified(
        &world,
        run,
        "a",
        "feat: ship a",
        "there was no body in what it answered with",
    );
    assert_eq!(change_from(&world, &branch)["body"], body);
    let streams: std::collections::BTreeSet<String> = world
        .journal(run)
        .iter()
        .filter(|event| event["labels"]["node"] == "a-rework-1" && event["source"] == "agentgraph")
        .filter_map(|event| event["stream"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(streams.len(), 2, "{streams:?}");
    assert!(
        streams.iter().all(|stream| stream.ends_with("-4242")),
        "{streams:?}"
    );
}

/// With no body drafter configured, a rework's title is left unverified — the
/// publication itself is exactly what it would be, and nothing holds the node.
#[test]
fn a_rework_with_no_body_drafter_configured_leaves_its_title_unverified() {
    let world = team_world("rework-undrafted");
    world.script("a.work", "a's change\n");
    world.script("a-rework-1.work", "a's change, reworked\n");
    world.script("a-rework-1.work-file", "a");
    world.script("b.work", "b's change\n");
    world.script("b-rework-1.work", "b's change, reworked\n");
    world.script("b-rework-1.work-file", "b");
    let run = "undrafted";
    launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &[])],
        2,
        &[],
    );
    left_unverified(
        &world,
        run,
        "a",
        "feat: ship a",
        "no body drafter is configured",
    );

    // A rework stating its title needs no drafter for it: the title is written
    // onto the change request over the description the host holds, untouched.
    let b_branch = text(&settled(&world, run, "b")["branch"], "b's branch").to_owned();
    let body = change_from(&world, &b_branch)["body"].clone();
    let mut stated = rework("b", "rework it");
    stated["title"] = json!("fix(b): what the op says");
    reply(&world, run, json!([stated])).exited(0);
    adopt(&world, run).exited(0);
    let change = change_from(&world, &b_branch);
    assert_eq!(change["title"], "fix(b): what the op says");
    assert_eq!(change["body"], body, "the description moved: {change}");
    assert_eq!(
        retitled(&world, run, "b-rework-1"),
        [
            json!({"node": "b-rework-1", "from": "feat: ship b", "to": "fix(b): what the op says", "by": "op"})
        ]
    );
    assert!(unverified(&world, run, "b-rework-1").is_empty());
}

/// A rework of a run that had settled brings it back to life as any accepted edit
/// does: the run reads as live once the edit is committed, the run-end hook fires
/// again at its new ending, and `status` reads the reworked and restacked nodes as
/// such.
#[cfg(unix)]
#[test]
fn a_rework_of_a_settled_run_makes_it_live_and_its_hook_fires_again_at_the_new_ending() {
    use crate::run_end_hooks::{hook, invocations, records, HOOK_TIMEOUT, RECORD_ENV};
    let world = team_world("rework-revive");
    std::fs::create_dir_all(records(&world)).expect("a record directory");
    let record = records(&world).to_string_lossy().into_owned();
    let world = world.with_env(RECORD_ENV, &record);
    let hook = hook(&world);
    world.script("a.work", "a's change\n");
    world.script("b.work", "b's change\n");
    world.script("a-rework-1.work", "a's change, as the review asked\n");
    world.script("a-rework-1.work-file", "a");
    world.script("pr-author.title-check", KEEPS);
    let run = "revive";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
        2,
        &[
            "--pr-author-graph",
            &drafter,
            "--success-hook",
            &hook,
            "--failure-hook",
            &hook,
            "--hook-timeout",
            HOOK_TIMEOUT,
        ],
    );
    assert_eq!(invocations(&world, run), ["success"]);
    world.run(&["status", run]).out_has("SETTLED");

    reply(
        &world,
        run,
        json!([rework("a", "say what the review asked in a.md")]),
    )
    .exited(0);
    // Live again: the edit added work the run has not done.
    let live = world.run(&["status", run]);
    assert!(!live.stdout.contains("SETTLED"), "{}", live.stdout);

    adopt(&world, run).exited(0);
    assert_eq!(
        invocations(&world, run),
        ["success", "success"],
        "{}",
        world.dump()
    );
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 2);
    let a_branch = text(&settled(&world, run, "a-rework-1")["branch"], "a's branch").to_owned();
    let status = world.run(&["status", run]);
    status.exited(0);
    status.out_has("SETTLED");
    status.out_has(&format!(
        "a: reworked — as a-rework-1: a reviewer asked for it on the draft; answering {COMMENT_URL}"
    ));
    status.out_has(&format!(
        "b: restacked onto a-rework-1's branch {a_branch} at "
    ));
    status.out_has(", without an agent turn");
    // The first ending's hook is read as superseded by the edit that reopened the
    // run, which says what it reworked.
    world
        .run(&["results", run])
        .out_has("edit committed at ")
        .out_has(": a reworked as a-rework-1");
}

/// The planner is the only author `rework` and `retitle` are granted to: a bus
/// configuration granting a second author every other op still leaves those two
/// refused for it, naming the op and the author, with nothing appended — while the
/// planner's are applied.
#[test]
fn only_the_planner_may_rework_or_retitle() {
    let world = team_world("rework-grants");
    world.script("a.work", "a's change\n");
    world.script("a-rework-1.work", "a's change, as the review asked\n");
    world.script("a-rework-1.work-file", "a");
    world.script("pr-author.title-check", KEEPS);
    let others: Vec<&str> = onepipeline::channel::layout::Op::ALL
        .into_iter()
        .filter(|op| {
            !matches!(
                op,
                onepipeline::channel::layout::Op::Rework
                    | onepipeline::channel::layout::Op::Retitle
                    | onepipeline::channel::layout::Op::Decline
            )
        })
        .map(onepipeline::channel::layout::Op::word)
        .collect();
    let config = world.root.join("onemessagebus.yaml");
    std::fs::write(
        &config,
        format!(
            "version: 1\ntransport: {{kind: local}}\nauthors:\n  sentinel:\n    capabilities: [{}]\n",
            others.join(", ")
        ),
    )
    .expect("the bus configuration is written");
    let config = config.to_string_lossy().into_owned();
    let run = "grants";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![lifecycle("a", &[])],
        1,
        &["--pr-author-graph", &drafter, "--bus-config", &config],
    );
    let commands = world.run_file(run, "channel/commands.jsonl");
    let appended = || std::fs::read_to_string(&commands).unwrap_or_default();
    let before = appended();
    let committed = world.events_of(run, "edit-committed").len();
    let as_sentinel = |command: Value| {
        world.run_with_stdin(
            &["reply", run],
            &json!({"version": onepipeline::channel::REPLY_ENVELOPE_VERSION,
                    "author": "sentinel", "commands": [command]})
            .to_string(),
        )
    };
    for (command, op) in [
        (rework("a", "the sentinel's idea"), "rework"),
        (
            json!({"op": "retitle", "id": "a", "title": "fix: the sentinel's", "reason": "why"}),
            "retitle",
        ),
        (
            json!({"op": "decline", "id": "a", "reason": "why",
                   "comments": [{"id": "IC_kwABzgAAAfU", "url": "https://github.com/owner/service/pull/1#issuecomment-501"}]}),
            "decline",
        ),
    ] {
        as_sentinel(command)
            .exited(crate::harness::REFUSED)
            .err_has(&format!("'{op}' is not an op the sentinel may issue"));
    }
    assert_eq!(appended(), before, "a refused envelope was appended");
    assert_eq!(world.events_of(run, "edit-committed").len(), committed);
    // A granted op of the sentinel's own is still applied: the refusal is the two ops'.
    as_sentinel(json!({"op": "finding", "id": "a", "message": "the sentinel noticed"})).exited(0);

    reply(
        &world,
        run,
        json!([rework("a", "say what the review asked in a.md")]),
    )
    .exited(0);
    adopt(&world, run).exited(0);
    read_green(&world, run, &["a-rework-1"]);
    reply(
        &world,
        run,
        json!([{"op": "retitle", "id": "a-rework-1", "title": "fix(a): the planner's",
                "reason": "the planner decides it"}]),
    )
    .exited(0);
    let branch = text(&settled(&world, run, "a-rework-1")["branch"], "a's branch").to_owned();
    assert_eq!(
        change_from(&world, &branch)["title"],
        "fix(a): the planner's"
    );
}

/// Each `rework` a run can be asked for and refuses, refused through the binary
/// naming why with nothing appended; a second rework of the same lineage numbered
/// past the first and refused a replacement id the lineage already used, its item
/// on the store naming what the newest rework answers.
#[test]
fn a_rework_the_run_cannot_take_is_refused_naming_why() {
    let world = team_world("rework-refused");
    world.script("a.work", "a's change\n");
    world.script("broken.fail", "1");
    world.script("a-rework-1.work", "a's change, reworked\n");
    world.script("a-rework-1.work-file", "a");
    world.script("a-rework-2.work", "a's change, reworked again\n");
    world.script("a-rework-2.work-file", "a");
    world.script("pr-author.title-check", KEEPS);
    let run = "refused";
    let drafter = world.pr_author_graph();
    let project = launched(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("broken", &[]),
            crate::harness::agent("notes", &[]),
        ],
        3,
        &["--pr-author-graph", &drafter],
    );
    let committed = world.events_of(run, "edit-committed").len();
    let with = |field: &str, value: Value| {
        let mut command = rework("a", "change it");
        command[field] = value;
        command
    };
    for (command, says) in [
        (rework("nope", "change it"), "no node 'nope'"),
        (rework("notes", "change it"), "is not a lifecycle node"),
        (rework("broken", "change it"), "is failed, not done"),
        (with("reason", json!(" ")), "the reason cannot be blank"),
        (
            with("instruction", json!("")),
            "the instruction cannot be blank",
        ),
        (
            with("title", json!("make it better")),
            "Conventional Commit",
        ),
        (
            with("replacement", json!("broken")),
            "replacement id 'broken' must be new",
        ),
        (
            with("replacement", json!(" ")),
            "the replacement needs a non-empty id",
        ),
    ] {
        reply(&world, run, json!([command]))
            .exited(crate::harness::REFUSED)
            .err_has(says);
    }
    // Each way a subject is not one, refused by the title rule naming why.
    for (title, says) in [
        ("fix: one line\nand another", "a line break"),
        ("fix(reader: unclosed", "its scope is not closed"),
        ("Fix: shouting", "is not a lowercase word"),
        ("fix(): nothing scoped", "its scope is empty"),
        ("fix((a)): nested", "its scope is empty"),
        ("fix(reader):  ", "no `type: description` separator"),
    ] {
        reply(&world, run, json!([with("title", json!(title))]))
            .exited(crate::harness::REFUSED)
            .err_has(says);
    }
    assert_eq!(world.events_of(run, "edit-committed").len(), committed);

    // `broken` failed, so the run ends unfinished whatever the rework does: the
    // adopting driver is read by what it settled, not by its exit.
    reply(&world, run, json!([rework("a", "change it")])).exited(0);
    adopt(&world, run);
    // The lineage's own ids are spent: the node it replaced cannot be named again,
    // and the next rework of it is numbered past the first.
    reply(
        &world,
        run,
        json!([with("replacement", json!("a")).as_object().map(|command| {
            let mut command = command.clone();
            command.insert("id".into(), json!("a-rework-1"));
            Value::Object(command)
        })]),
    )
    .exited(crate::harness::REFUSED)
    .err_has("replacement id 'a' must be new");
    // A rework naming no review comment says so in both views.
    let mut uncommented = rework("a-rework-1", "change it again");
    uncommented
        .as_object_mut()
        .expect("an op")
        .remove("comments");
    reply(&world, run, json!([uncommented])).exited(0);
    adopt(&world, run);
    let again = settled(&world, run, "a-rework-2");
    assert_eq!(again["outcome"], "change-review-draft-unchecked", "{again}");
    assert_eq!(again["branch"], settled(&world, run, "a")["branch"]);
    let said = "reworked — as a-rework-2: a reviewer asked for it on the draft; naming no \
                review comment";
    world
        .run(&["status", run])
        .out_has(&format!("a-rework-1: {said}"));
    world.run(&["results", run]).out_has(said);
    // A breaking change's subject is one.
    reply(
        &world,
        run,
        json!([{"op": "retitle", "id": "a-rework-2", "title": "feat(a)!: break a",
                "reason": "the change breaks a"}]),
    )
    .exited(0);
    assert_eq!(
        change_from(&world, text(&again["branch"], "a's branch"))["title"],
        "feat(a)!: break a"
    );

    // The item the lineage projected onto carries what its latest rework answers.
    world.until_store("the second rework to reach the store", |world| {
        world
            .store_tasks(&project)
            .iter()
            .any(|task| task["item"]["metadata"]["onepipeline.rework"]["replaces"] == "a-rework-1")
    });
}

/// A restack whose conflict the draft's own worker fails to conclude settles that
/// draft `failed`, recorded as the restack it was — an agent turn spent, nothing
/// pushed — and moves nothing above it: its branch never moved, so the draft
/// stacked on it owes no restack.
#[test]
fn a_restack_whose_worker_fails_its_merge_settles_the_draft_failed() {
    let world = team_world("rework-conflict-failed");
    world.script("a.work", "a wrote this\n");
    world.script("a.work-file", "shared");
    world.script("b.work", "b changed this\n");
    world.script("b.work-file", "shared");
    world.script("c.work", "c's change\n");
    world.script("a-rework-1.work", "a reworked this\n");
    world.script("a-rework-1.work-file", "shared");
    world.script("pr-author.title-check", KEEPS);
    let run = "failed";
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
    let b_branch = text(&settled(&world, run, "b")["branch"], "b's branch").to_owned();
    let b_head = head_of(&world, &b_branch);
    world.unscript("b.work");
    world.script("b.fail", "1");

    reply(&world, run, json!([rework("a", "rewrite the shared file")])).exited(0);
    // The run ends unfinished, with `b` failed: read by what it settled.
    adopt(&world, run);

    let b = settled(&world, run, "b");
    assert_eq!(b["status"], "failed", "{b}");
    let restacked = world
        .events_of(run, "node-restacked")
        .into_iter()
        .find(|event| event["labels"]["node"] == "b")
        .expect("b's restack is on the record");
    assert_eq!(restacked["payload"]["agent_dispatched"], true);
    assert_eq!(head_of(&world, &b_branch), b_head, "b's branch moved");
    assert!(
        !world
            .events_of(run, "node-restack-started")
            .iter()
            .any(|event| event["labels"]["node"] == "c"),
        "c was restacked onto a branch that never moved"
    );
    assert_eq!(settled(&world, run, "c")["status"], "done");
}

/// A restack whose republication the host's checks refuse goes back to the draft's
/// own worker on the same branch once a trigger reads that verdict red, as a
/// `checks-failed` publication's re-dispatch does — drafted again, since that worker may
/// change the content — and reads as a review draft once a later trigger reads its
/// repair green.
#[test]
fn a_restack_its_checks_refuse_is_asked_of_the_draft_s_worker_again() {
    let world = team_world("rework-restack-retry");
    world.script("a.work", "a's change\n");
    world.script("b.work-anew", "b's change");
    world.script("a-rework-1.work", "a's change, as the review asked\n");
    world.script("a-rework-1.work-file", "a");
    world.script("pr-author.title-check", KEEPS);
    let run = "retry";
    let drafter = world.pr_author_graph();
    launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
        2,
        &["--pr-author-graph", &drafter],
    );
    let b_branch = text(&settled(&world, run, "b")["branch"], "b's branch").to_owned();
    let b_number = change_from(&world, &b_branch)["number"].clone();

    reply(
        &world,
        run,
        json!([rework("a", "say what the review asked in a.md")]),
    )
    .exited(0);
    adopt(&world, run).exited(0);
    let restacked = settled(&world, run, "b");
    assert_eq!(
        restacked["outcome"], "change-review-draft-unchecked",
        "{restacked}"
    );
    // `b`'s restacked head is red on the host, and the next trigger reads it so.
    let red = text(&restacked["head"], "b's head").to_owned();
    std::fs::create_dir_all(world.fakes.join("gh-head-verdict")).expect("a verdict directory");
    std::fs::write(
        world.fakes.join("gh-head-verdict").join(&red),
        "failed:lint",
    )
    .expect("the verdict is written");
    world.run(&["readjust", run]).exited(0);

    let b = settled(&world, run, "b");
    assert_eq!(b["status"], "done", "{b}");
    assert_ne!(b["head"], red.as_str(), "{b}");
    let restack = world
        .events_of(run, "node-restacked")
        .into_iter()
        .find(|event| event["labels"]["node"] == "b")
        .expect("b's restack is on the record");
    assert_eq!(restack["payload"]["agent_dispatched"], false);
    let redispatched = world
        .events_of(run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "b")
        .nth(1)
        .expect("b was asked again");
    assert!(
        text(&redispatched["payload"]["reason"], "a reason").starts_with("checks-failed"),
        "{redispatched}"
    );
    // Drafted again on its retry, onto the one change request it has.
    assert_eq!(tasks_of(&world, run, "b", "pr-author").len(), 2);
    assert_eq!(opened_from(&world, &b_branch), 1);
    assert!(held_as_draft(&world, &b_number));
    read_green(&world, run, &["b"]);
}

/// A rework cleared for publication before a takeover may finish its push, but
/// the superseded driver must not begin the descendant restack that push owes.
#[test]
fn a_takeover_before_a_reworks_push_prevents_its_driver_from_starting_a_restack() {
    let world = World::new("rework-takeover")
        .with_env("ONEVCS_BRANCH_PREFIX", "me/")
        .with_env("ONEPIPELINE_CLAIM_INTERVAL_SECONDS", "10")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let go = world.fakes.join("push.go");
    let held = crate::harness::held_publication(&world, &go);
    world.repository("change-open", &held.argv());
    crate::draft_lifecycle::rules(&world, "change-open", "required", None);
    world.script("gh.checks", crate::draft_lifecycle::GREEN);
    world.script("a.work", "a's change\n");
    world.script("b.work", "b's change\n");
    world.script("a-rework-1.work", "a's change, reworked\n");
    world.script("a-rework-1.work-file", "a");
    held.release();
    let run = "takeover";
    let project = launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
        2,
        &[],
    );
    let a_branch = text(&settled(&world, run, "a")["branch"], "a's branch").to_owned();
    let old_head = head_of(&world, &a_branch);
    for marker in [go.clone(), go.with_extension("go.holding")] {
        std::fs::remove_file(marker).expect("the initial publication released its gate");
    }
    reply(&world, run, json!([rework("a", "rework it")])).exited(0);
    world.run(&["adopt", run, "--detach"]).exited(0);
    world.until("the rework's cleared publication to be held", |_| {
        held.holding()
    });
    // The successor claims the project before the first driver's accepted push makes b's
    // restack due, and starts nothing until that driver has let go.
    let successor = world.on_host("takeover-host");
    let mut taking = successor.cmd(&["start", &project, "--detach", "--take-over"]);
    let taking = taking
        .current_dir(&successor.project)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the takeover starts");
    world.until("the first driver to read the takeover", |world| {
        !world.events_of(run, "driver-superseded").is_empty()
    });
    held.release();
    world.until("the superseded driver to end", |world| {
        world.run_file(run, "result.json").is_file()
    });
    let taken = taking.wait_with_output().expect("the takeover answers");
    assert_eq!(
        taken.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&taken.stderr)
    );
    assert_ne!(
        head_of(&world, &a_branch),
        old_head,
        "the already cleared push did not finish"
    );
    assert!(
        world.events_of(run, "node-restack-started").is_empty(),
        "a superseded driver began a restack: {:#?}",
        world.journal(run)
    );
    successor.run(&["stop", run]).exited(0);
    assert_eq!(successor.events_of(run, "run-stopped").len(), 1);
    assert_eq!(opened_from(&world, &a_branch), 1);
}

/// The real graph and harness validate the drafter's title check before this
/// engine consumes it: a valid replacement retitles the draft, and an unknown
/// verdict is refused by the producer without holding the rework's publication.
#[test]
fn a_rework_title_check_is_validated_by_the_real_producer() {
    let world = team_world("rework-real-title-check");
    world.write_graphs();
    world.script("harness.work", "the original change");
    world.script("harness.body", "## What\nThe original change.");
    let drafter = world.pr_author_graph();
    let run = "real-title-check";
    let project = world.plan(run, &plan_of(run, vec![lifecycle("a", &[])]));
    world
        .run_on_agentgraph(&["start", &project, "--attach", "--pr-author-graph", &drafter])
        .exited(0);
    let branch = text(&settled(&world, run, "a")["branch"], "a's branch").to_owned();
    world.script("harness.work", "the change after review");
    world.script(
        "harness.title-check",
        r#"{"verdict":"replaced","title":"fix(a): answer the review"}"#,
    );
    reply(&world, run, json!([rework("a", "answer the review")])).exited(0);
    world.run_on_agentgraph(&["adopt", run]).exited(0);
    assert_eq!(
        change_from(&world, &branch)["title"],
        "fix(a): answer the review"
    );
    assert_eq!(retitled(&world, run, "a-rework-1")[0]["by"], "drafter");
    assert!(unverified(&world, run, "a-rework-1").is_empty());
    let reports: Vec<Value> = std::fs::read_dir(world.run_file(run, "reports"))
        .expect("the real producer's reports were retained")
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    assert!(
        reports.iter().any(
            |report| report["results"].as_array().is_some_and(|results| {
                results.iter().any(|result| {
                    result["schema_valid"] == true
                        && result["structured"]["title_check"]["title"]
                            == "fix(a): answer the review"
                })
            })
        ),
        "no real producer validated the title check: {reports:#?}"
    );

    world.script("harness.work", "another change after review");
    world.script("harness.title-check", r#"{"verdict":"maybe"}"#);
    reply(
        &world,
        run,
        json!([rework("a-rework-1", "review it again")]),
    )
    .exited(0);
    world.run_on_agentgraph(&["adopt", run]).exited(0);
    read_green(&world, run, &["a-rework-2"]);
    assert_eq!(
        change_from(&world, &branch)["title"],
        "fix(a): answer the review"
    );
    assert_eq!(unverified(&world, run, "a-rework-2").len(), 1);
    assert_eq!(opened_from(&world, &branch), 1);
    assert!(held_as_draft(
        &world,
        &change_from(&world, &branch)["number"]
    ));
}
