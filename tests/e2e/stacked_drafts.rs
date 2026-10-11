//! Stacked drafts: a landing node depending on unlanded work of its own repository
//! is placed on that work, and its change request shows only its own change.
//!
//! One stack parent places the node on that parent's branch; several place it on a
//! stack base `onevcs` assembles from the root and each parent; a landed parent
//! places nothing. Every journey drives the compiled binary publishing through the
//! real linked `onevcs` over a real origin on disk, with GitHub standing in at that
//! library's own `ONEVCS_GH` seam — the `fake-gh` double, whose change requests are
//! read back for their base and description — and reads what the run recorded: the
//! settlement's `base`, `head` and `stack`, the worker's and the drafter's tasks,
//! `status`, `results`, and the settlement projected onto the Markdown store.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary and the sibling these journeys are about — `onevcs`, which assembles the stack
// base and publishes each stacked change — is the real library, over real git and a real
// origin on disk. `oneagentgraph` is substituted at its subprocess boundary so a journey
// states what a worker wrote rather than paying for a model turn, and GitHub is
// substituted at `onevcs`'s own `ONEVCS_GH` override so a change request's base, checks
// and description can be stated and read offline. `harness.rs` carries the same
// suppression and the full rationale.

use crate::draft_lifecycle::{team_repository, GREEN};
use crate::harness::{git, lifecycle, plan_of, Repository, World};
use serde_json::{json, Value};

/// The lines the engine's Stack block opens and closes with.
const OPEN: &str = "<!-- onepipeline:stack -->";
const CLOSE: &str = "<!-- /onepipeline:stack -->";

/// What the fake drafter writes when nothing else is scripted.
const DRAFTED: &str = "## What\nDrafted from the diff.";

/// A required check the host reports red.
const RED: &str = "lint completed failure required";

/// The host's branch prefix every journey here runs under, so a stack base's name
/// is shown to carry it.
const PREFIX: &str = "me/";

/// A world on a team repository — `change-open`, approvals required — whose host
/// cuts branches under [`PREFIX`] and reports every required check green.
fn team_world(name: &str) -> World {
    let world = World::new(name).with_env("ONEVCS_BRANCH_PREFIX", PREFIX);
    team_repository(&world);
    world.script("gh.checks", GREEN);
    world
}

/// Launch `nodes` attached, one at a time so the host numbers their change requests
/// in plan order, wait for the run's result, and hand back what the launch printed.
fn settle(world: &World, name: &str, nodes: Vec<Value>, extra: &[&str]) -> crate::harness::Run {
    let mut plan = plan_of(name, nodes);
    plan["concurrency"] = json!(1);
    let path = world.plan(name, &plan);
    let mut args = vec!["start", path.as_str(), "--attach"];
    args.extend_from_slice(extra);
    let launched = world.run_from(&world.project, &args);
    assert!(
        world.run_file(name, "result.json").is_file(),
        "the run did not settle:\n{}\n{}",
        launched.stdout,
        launched.stderr
    );
    launched
}

/// The newest settlement of `node`'s, as the journal recorded it.
fn settled(world: &World, run: &str, node: &str) -> Value {
    world
        .events_of(run, "node-settled")
        .into_iter()
        .rev()
        .find(|event| event["labels"]["node"] == node)
        .map(|event| event["payload"].clone())
        .unwrap_or_else(|| panic!("{node} never settled: {:#?}", world.journal(run)))
}

fn text<'a>(value: &'a Value, what: &str) -> &'a str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("{what} is not a string: {value}"))
}

/// The change request the host holds for `branch`.
fn change_from(world: &World, branch: &str) -> Value {
    world
        .changes_opened()
        .into_iter()
        .find(|change| change["head"] == branch)
        .unwrap_or_else(|| {
            panic!(
                "the host holds no change request from {branch}: {:#?}",
                world.changes_opened()
            )
        })
}

/// Whether the host holds change request `number` as a draft.
fn held_as_draft(world: &World, number: &Value) -> bool {
    std::fs::read_to_string(world.fakes.join("gh").join(number.to_string()))
        .is_ok_and(|state| state.trim() == "draft")
}

/// The files `branch` changes against `base`, on the origin.
fn changed_against(world: &World, repo: &Repository, base: &str, branch: &str) -> Vec<String> {
    git(
        world,
        &repo.origin,
        &["diff", "--name-only", &format!("{base}...{branch}")],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

/// What `branch` holds at `path` on the origin, its trailing newline aside: a
/// scripted worker writes its body without one.
fn file_on(world: &World, repo: &Repository, branch: &str, path: &str) -> String {
    git(world, &repo.origin, &["show", &format!("{branch}:{path}")])
        .trim_end()
        .to_owned()
}

/// The task prose each of `node`'s dispatches under `persona` was handed, read off
/// the turn its worker echoed it in.
fn tasks_of(world: &World, run: &str, node: &str, persona: &str) -> Vec<String> {
    world
        .journal(run)
        .iter()
        .filter(|event| {
            event["kind"] == "turn-activity"
                && event["labels"]["node"] == node
                && event["labels"]["persona"] == persona
        })
        .filter_map(|event| event["payload"]["task"].as_str().map(str::to_owned))
        .collect()
}

/// `description`'s one Stack block, after asserting it holds exactly one and that
/// the text before it is `before`.
fn one_block<'a>(description: &'a str, before: &str) -> &'a str {
    assert_eq!(
        description.matches(OPEN).count(),
        1,
        "not exactly one Stack block:\n{description}"
    );
    assert_eq!(description.matches(CLOSE).count(), 1, "{description}");
    let open = description.find(OPEN).expect("counted above");
    let close = description.find(CLOSE).expect("counted above") + CLOSE.len();
    assert_eq!(description[..open].trim_end(), before, "{description}");
    &description[open..close]
}

/// The repository `team_world` registered.
fn service(world: &World) -> Repository {
    Repository {
        origin: world.root.join("origin.git"),
        checkout: world.root.join("service"),
    }
}

/// The journey the stack exists for, on a team repository: `a → b`, `a, c → e`,
/// `a, b → g`, `f` stacked on `a` whose checks fail, and `b, c → h` — a stack
/// base over a parent that is itself stacked, whose root is the lowest change's. Each change request is a
/// draft based on what is below it and showing only its own change, says so in an
/// engine-written Stack block, and `status` and `results` name each one and what it
/// is drafted against after the whole run has settled.
#[test]
fn landing_dependents_of_unlanded_parents_are_drafted_on_them() {
    // `f` is asked twice, so its second session — continued on its branch, which cuts
    // nothing and so reports no prefix — opens before `h` is placed; `h`'s stack base
    // is named under the prefix all the same.
    let world = team_world("stack-drafts").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "2");
    let repo = service(&world);
    for id in ["a", "b", "c", "e", "g", "h"] {
        world.script(&format!("{id}.work"), &format!("{id}'s change\n"));
    }
    world.script("f.work-anew", "f's change");
    // Checked by the worker before it writes: the parent's work is already in the
    // worktree it was dispatched into, so the session was cut from it.
    world.script("b.requires-file", "a.md");
    world.script("e.requires-file", "c.md");
    world.script("g.requires-file", "b.md");
    // `f`'s change request is the sixth the host opens; its checks fail.
    world.script("gh.checks.6", RED);
    let run = "stack";
    let drafter = world.pr_author_graph();
    settle(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("c", &[]),
            lifecycle("b", &["a"]),
            lifecycle("e", &["a", "c"]),
            lifecycle("g", &["a", "b"]),
            lifecycle("f", &["a"]),
            lifecycle("h", &["b", "c"]),
        ],
        &["--pr-author-graph", &drafter],
    );

    let a = settled(&world, run, "a");
    let c = settled(&world, run, "c");
    for (id, node) in [("a", &a), ("c", &c)] {
        assert_eq!(node["outcome"], "change-review-draft", "{id}: {node}");
        assert_eq!(
            node["base"], "main",
            "{id} is drafted against the root: {node}"
        );
        assert!(
            node.get("stack").is_none(),
            "{id} has no stack parents: {node}"
        );
        let head = text(&node["head"], "a success settlement's head");
        assert_eq!(
            git(
                &world,
                &repo.origin,
                &["rev-parse", text(&node["branch"], "branch")]
            )
            .trim(),
            head,
            "{id}'s head is where its branch stands on the origin"
        );
    }
    let a_branch = text(&a["branch"], "a's branch");
    let c_branch = text(&c["branch"], "c's branch");
    let a_url = text(&a["change_url"], "a's change request");
    let c_url = text(&c["change_url"], "c's change request");

    // One stack parent: `b` is cut from `a`'s branch and drafted against it.
    let b = settled(&world, run, "b");
    assert_eq!(b["status"], "done", "{b}");
    assert_eq!(b["outcome"], "change-review-draft", "{b}");
    assert_eq!(b["base"], a_branch, "{b}");
    assert_eq!(
        b["stack"],
        json!({"parents": [{"node": "a", "branch": a_branch, "head": a["head"]}], "stack_base": null}),
        "{b}"
    );
    let b_branch = text(&b["branch"], "b's branch");
    let b_change = change_from(&world, b_branch);
    assert_eq!(b_change["base"], a_branch, "{b_change}");
    assert!(held_as_draft(&world, &b_change["number"]), "{b_change}");
    assert!(held_as_draft(
        &world,
        &change_from(&world, a_branch)["number"]
    ));
    assert_eq!(changed_against(&world, &repo, a_branch, b_branch), ["b.md"]);

    // Several: `e` is cut from the stack base assembled from the root, then `a`,
    // then `c`, and its change request shows only `e`'s change against it.
    let e = settled(&world, run, "e");
    let stack_base = format!("{PREFIX}stack-base/{run}/e");
    assert_eq!(e["outcome"], "change-review-draft", "{e}");
    assert_eq!(e["base"], stack_base.as_str(), "{e}");
    assert_eq!(
        e["stack"],
        json!({
            "parents": [
                {"node": "a", "branch": a_branch, "head": a["head"]},
                {"node": "c", "branch": c_branch, "head": c["head"]},
            ],
            "stack_base": stack_base,
        }),
        "{e}"
    );
    let e_branch = text(&e["branch"], "e's branch");
    for parent in ["a.md", "c.md"] {
        assert_eq!(
            file_on(&world, &repo, e_branch, parent),
            format!("{}'s change", &parent[..1])
        );
        assert_eq!(
            file_on(&world, &repo, &stack_base, parent),
            format!("{}'s change", &parent[..1])
        );
    }
    assert_eq!(change_from(&world, e_branch)["base"], stack_base.as_str());
    assert_eq!(
        changed_against(&world, &repo, &stack_base, e_branch),
        ["e.md"]
    );

    // `a` is below `b`, so `g` is placed on `b` alone and no stack base is cut.
    let g = settled(&world, run, "g");
    assert_eq!(g["base"], b_branch, "{g}");
    assert_eq!(
        g["stack"],
        json!({"parents": [{"node": "b", "branch": b_branch, "head": b["head"]}], "stack_base": null}),
        "{g}"
    );
    let g_branch = text(&g["branch"], "g's branch");
    assert_eq!(changed_against(&world, &repo, b_branch, g_branch), ["g.md"]);
    // `h` stands on `b`, which stands on `a`: its stack base is cut from the root
    // `a` was drafted against, and carries all three below it.
    let h = settled(&world, run, "h");
    let h_base = format!("{PREFIX}stack-base/{run}/h");
    assert_eq!(h["base"], h_base.as_str(), "{h}");
    assert_eq!(
        h["stack"],
        json!({
            "parents": [
                {"node": "b", "branch": b_branch, "head": b["head"]},
                {"node": "c", "branch": c_branch, "head": c["head"]},
            ],
            "stack_base": h_base,
        }),
        "{h}"
    );
    for (file, by) in [("a.md", "a"), ("b.md", "b"), ("c.md", "c")] {
        assert_eq!(
            file_on(&world, &repo, &h_base, file),
            format!("{by}'s change")
        );
    }
    assert_eq!(
        changed_against(&world, &repo, &h_base, text(&h["branch"], "h's branch")),
        ["h.md"]
    );
    let stack_bases: Vec<String> =
        git(&world, &repo.origin, &["branch", "--list", "*stack-base/*"])
            .lines()
            .map(|line| line.trim().to_owned())
            .collect();
    assert_eq!(
        stack_bases,
        [stack_base.clone(), h_base.clone()],
        "only e and h assembled one"
    );

    // The worker is told what it is stacked on, and so is the drafter.
    let b_task = &tasks_of(&world, run, "b", "engineer")[0];
    assert!(b_task.contains("\n## Stack\n"), "{b_task}");
    assert!(
        b_task.contains(&format!(
            "- stack parent `a`: branch `{a_branch}`, change request {a_url}"
        )),
        "{b_task}"
    );
    assert!(
        b_task.contains(&format!("- base: `{a_branch}`")),
        "{b_task}"
    );
    let e_task = &tasks_of(&world, run, "e", "engineer")[0];
    for (node, branch, url) in [("a", a_branch, a_url), ("c", c_branch, c_url)] {
        assert!(
            e_task.contains(&format!(
                "- stack parent `{node}`: branch `{branch}`, change request {url}"
            )),
            "{e_task}"
        );
    }
    assert!(
        e_task.contains(&format!(
            "- base: the stack base `{stack_base}`, merging `main` and"
        )),
        "{e_task}"
    );

    // Each stacked change request holds the drafter's body unchanged, then one block
    // the engine wrote naming its base and each parent's change request.
    for (id, node, parents) in [("b", &b, vec![a_url]), ("e", &e, vec![a_url, c_url])] {
        let description = change_from(&world, text(&node["branch"], "branch"))["body"]
            .as_str()
            .expect("a body")
            .to_owned();
        let block = one_block(&description, DRAFTED);
        assert!(block.contains(text(&node["base"], "base")), "{id}: {block}");
        for url in parents {
            assert!(block.contains(url), "{id}: {block}");
        }
        let drafting = tasks_of(&world, run, id, "pr-author");
        assert_eq!(drafting.len(), 1, "{id}: {drafting:?}");
        assert!(
            drafting[0].contains("\n## Stack\n") && drafting[0].contains(block),
            "{id}'s drafter was not shown the block:\n{}",
            drafting[0]
        );
    }
    // A change request with no stack parents carries no block.
    assert!(!text(&change_from(&world, a_branch)["body"], "a's body").contains(OPEN));

    // `f`'s change request opened, then its checks failed: it settles failed and
    // still names that change request and its base.
    let f = settled(&world, run, "f");
    assert_eq!(f["status"], "failed", "{f}");
    assert_eq!(f["outcome"], "checks-failed", "{f}");
    assert_eq!(
        world
            .events_of(run, "node-dispatched")
            .iter()
            .filter(|event| event["labels"]["node"] == "f")
            .count(),
        2,
        "f was not asked again on its branch"
    );
    assert_eq!(f["base"], a_branch, "{f}");
    let f_url = text(&f["change_url"], "f's change request");
    assert!(f.get("head").is_some_and(Value::is_string), "{f}");

    let b_url = text(&b["change_url"], "b's change request");
    let e_url = text(&e["change_url"], "e's change request");
    let g_url = text(&g["change_url"], "g's change request");
    let lines = [
        format!("a: done — {a_url} drafted against the root main"),
        format!("c: done — {c_url} drafted against the root main"),
        format!("b: done — {b_url} drafted against a's branch {a_branch} ({a_url})"),
        format!(
            "e: done — {e_url} drafted against stack base {stack_base} merging a ({a_branch}, \
             {a_url}) and c ({c_branch}, {c_url})"
        ),
        format!("g: done — {g_url} drafted against b's branch {b_branch} ({b_url})"),
        format!("f: failed — {f_url} drafted against a's branch {a_branch} ({a_url})"),
    ];
    let status = world.run(&["status", run]);
    status.exited(0);
    for line in &lines {
        status.out_has(line);
    }
    let results = world.run(&["results", run]);
    results.exited(0);
    for line in &lines {
        // `results` puts the status word and any outcome first, then the change.
        let (id, rest) = line.split_once(": ").expect("a node line");
        let change = rest.split_once(" — ").expect("a change").1;
        let found = results
            .stdout
            .lines()
            .find(|row| row.trim_start().starts_with(&format!("{id} ")))
            .unwrap_or_else(|| panic!("results has no line for {id}:\n{}", results.stdout));
        assert!(found.contains(change), "{id}: {found}");
    }
}

/// `a, c → e` where `a` and `c` write one file differently: the stack base's merge
/// of `c` conflicts, `e`'s own worker is dispatched into the stack base's session
/// with the merge in progress, and once it concludes the merge the base is pushed
/// with its resolution and `e` is drafted on it showing only its own change.
#[test]
fn a_conflicting_stack_base_is_concluded_by_the_node_s_own_worker() {
    let world = team_world("stack-conflict");
    let repo = service(&world);
    world.script("a.work", "a wrote this\n");
    world.script("c.work", "c wrote this\n");
    world.script("a.work-file", "shared");
    world.script("c.work-file", "shared");
    world.script("e.work", "e's change\n");
    world.script("e.resolves-merge", "a and c, reconciled\n");
    world.script("e.merges-only", "");
    let run = "conflict";
    settle(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("c", &[]),
            lifecycle("e", &["a", "c"]),
        ],
        &[],
    );

    let e = settled(&world, run, "e");
    let stack_base = format!("{PREFIX}stack-base/{run}/e");
    assert_eq!(e["status"], "done", "{e}");
    assert_eq!(e["outcome"], "change-review-draft", "{e}");
    assert_eq!(e["base"], stack_base.as_str(), "{e}");
    assert_eq!(e["stack"]["stack_base"], stack_base.as_str(), "{e}");

    // The worker was handed the stack base's merge, in the stack base's session,
    // and then its own work in its own.
    let tasks = tasks_of(&world, run, "e", "engineer");
    assert_eq!(tasks.len(), 2, "{tasks:#?}");
    assert!(
        tasks[0].contains("opened with a merge in progress") && tasks[0].contains("shared.md"),
        "{}",
        tasks[0]
    );
    assert!(
        !tasks[1].contains("opened with a merge in progress"),
        "{}",
        tasks[1]
    );

    let e_branch = text(&e["branch"], "e's branch");
    for branch in [stack_base.as_str(), e_branch] {
        assert_eq!(
            file_on(&world, &repo, branch, "shared.md"),
            "a and c, reconciled",
            "{branch}"
        );
    }
    assert!(
        git(&world, &repo.origin, &["log", "--format=%s", &stack_base]).contains("Merge"),
        "the stack base carries its merges"
    );
    assert_eq!(
        changed_against(&world, &repo, &stack_base, e_branch),
        ["e.md"]
    );
    assert_eq!(change_from(&world, e_branch)["base"], stack_base.as_str());
}

/// A stack base whose merge the worker never concludes fails the node once the
/// worker has been handed one merge per parent and the root, naming the stack base,
/// with no session of its own opened — so the settlement names its stack and neither
/// a base nor a head.
#[test]
fn an_unconcluded_stack_base_merge_fails_the_node_naming_it() {
    let world = team_world("stack-unconcluded");
    let repo = service(&world);
    world.script("a.work", "a wrote this\n");
    world.script("c.work", "c wrote this\n");
    world.script("a.work-file", "shared");
    world.script("c.work-file", "shared");
    world.script("e.work", "e's change\n");
    // Handed the merge, it concludes nothing.
    world.script("e.merges-only", "");
    let run = "unconcluded";
    settle(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("c", &[]),
            lifecycle("e", &["a", "c"]),
        ],
        &[],
    );
    let e = settled(&world, run, "e");
    let stack_base = format!("{PREFIX}stack-base/{run}/e");
    assert_eq!(e["status"], "failed", "{e}");
    assert_eq!(e["outcome"], "infrastructure-failure", "{e}");
    let detail = text(&e["detail"], "e's detail");
    assert!(
        detail.contains(&format!(
            "the stack base {stack_base} could not be assembled"
        )) && detail.contains("conflicted again after the worker was handed 3 merge(s)"),
        "{detail}"
    );
    // The root's merge and each parent's: three hand-offs, then the refusal.
    let tasks = tasks_of(&world, run, "e", "engineer");
    assert_eq!(tasks.len(), 3, "{tasks:#?}");
    assert!(tasks
        .iter()
        .all(|task| task.contains("opened with a merge in progress")));
    assert!(e.get("base").is_none() && e.get("head").is_none(), "{e}");
    assert_eq!(e["stack"]["stack_base"], stack_base.as_str(), "{e}");
    assert!(
        git(&world, &repo.origin, &["branch", "--list", &stack_base])
            .trim()
            .is_empty(),
        "a stack base nobody concluded was pushed"
    );
}

/// The node's worker failing in the stack base's session settles the node on that
/// failure, with nothing of its own session opened — on a host that configures no
/// branch prefix, so the stack base is named under none.
#[test]
fn a_worker_that_fails_the_stack_base_merge_fails_the_node() {
    let world = World::new("stack-merge-fails");
    team_repository(&world);
    world.script("gh.checks", GREEN);
    world.script("a.work", "a wrote this\n");
    world.script("c.work", "c wrote this\n");
    world.script("a.work-file", "shared");
    world.script("c.work-file", "shared");
    world.script("e.fail", "1");
    let run = "mergefails";
    settle(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("c", &[]),
            lifecycle("e", &["a", "c"]),
        ],
        &[],
    );
    let e = settled(&world, run, "e");
    assert_eq!(e["status"], "failed", "{e}");
    assert_ne!(
        e["outcome"], "infrastructure-failure",
        "the worker's own ending: {e}"
    );
    let tasks = tasks_of(&world, run, "e", "engineer");
    assert_eq!(tasks.len(), 1, "{tasks:#?}");
    assert!(
        tasks[0].contains("opened with a merge in progress"),
        "{}",
        tasks[0]
    );
    assert!(e.get("base").is_none() && e.get("branch").is_none(), "{e}");
    assert_eq!(
        e["stack"]["stack_base"],
        format!("stack-base/{run}/e"),
        "{e}"
    );
}

/// A node cancelled while its worker is concluding its stack base's merge settles
/// cancelled with nothing of its own session opened, and the stack base it never
/// finished is not pushed.
#[test]
fn a_node_cancelled_while_concluding_its_stack_base_settles_cancelled() {
    let world = team_world("stack-merge-cancelled");
    let repo = service(&world);
    world.script("a.work", "a wrote this\n");
    world.script("c.work", "c wrote this\n");
    world.script("a.work-file", "shared");
    world.script("c.work-file", "shared");
    world.script("e.resolves-merge", "a and c, reconciled\n");
    world.script("e.merges-only", "");
    let meeting = world.rendezvous("e");
    let run = "mergecancelled";
    let mut plan = plan_of(
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("c", &[]),
            lifecycle("e", &["a", "c"]),
        ],
    );
    plan["concurrency"] = json!(1);
    let path = world.plan(run, &plan);
    world.run(&["start", &path, "--detach"]).exited(0);

    // Held inside the dispatch that was handed the stack base's merge.
    let held = meeting.arrived();
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "cancel", "id": "e"}]}).to_string(),
        )
        .exited(0);
    world.until("the cancel to be committed", |world| {
        !world.events_of(run, "edit-committed").is_empty()
    });
    held.release();
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });

    let e = settled(&world, run, "e");
    let stack_base = format!("{PREFIX}stack-base/{run}/e");
    assert_eq!(e["status"], "cancelled", "{e}");
    let tasks = tasks_of(&world, run, "e", "engineer");
    assert_eq!(tasks.len(), 1, "{tasks:#?}");
    assert!(
        tasks[0].contains("opened with a merge in progress"),
        "{}",
        tasks[0]
    );
    assert!(
        e.get("base").is_none() && e.get("branch").is_none(),
        "no session of its own opened: {e}"
    );
    assert_eq!(e["stack"]["stack_base"], stack_base.as_str(), "{e}");
    assert!(
        git(&world, &repo.origin, &["branch", "--list", &stack_base])
            .trim()
            .is_empty(),
        "a stack base the cancelled worker never finished was pushed"
    );
}

/// A Stack block the host refuses to write leaves the node settled on its
/// publication, says so on the driver's output, and leaves the description as the
/// publication opened it.
#[test]
fn a_stack_block_the_host_will_not_take_leaves_the_publication_as_it_settled() {
    let world = team_world("stack-edit-refused");
    world.script("a.work", "a's change\n");
    world.script("b.work", "b's change\n");
    world.script("gh.edit-refused", "the host refused the edit");
    let run = "editrefused";
    let launched = settle(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
        &[],
    );
    let a = settled(&world, run, "a");
    let b = settled(&world, run, "b");
    assert_eq!(b["status"], "done", "{b}");
    assert_eq!(b["outcome"], "change-review-draft", "{b}");
    assert_eq!(b["base"], a["branch"], "{b}");
    let change = change_from(&world, text(&b["branch"], "b's branch"));
    launched.err_has(&format!(
        "node 'b': the Stack block was not written onto {}",
        text(&b["change_url"], "b's change request")
    ));
    assert!(
        !text(&change["body"], "b's body").contains(OPEN),
        "{change}"
    );
}

/// A parent spelled by another name for the same repository — its registered
/// checkout's path rather than its alias — is a stack parent all the same.
#[test]
fn a_parent_spelled_by_another_name_for_the_repository_is_stacked_on() {
    let world = team_world("stack-aliases");
    let repo = service(&world);
    world.script("a.work", "a's change\n");
    world.script("b.work", "b's change\n");
    world.script("b.requires-file", "a.md");
    let mut b = lifecycle("b", &["a"]);
    b["repo"] = json!(repo.checkout.to_string_lossy());
    let run = "aliases";
    settle(&world, run, vec![lifecycle("a", &[]), b], &[]);
    let a = settled(&world, run, "a");
    let b = settled(&world, run, "b");
    assert_eq!(b["status"], "done", "{b}");
    assert_eq!(b["base"], a["branch"], "{b}");
    assert_eq!(b["stack"]["parents"][0]["node"], "a", "{b}");
    assert_eq!(
        changed_against(
            &world,
            &repo,
            text(&a["branch"], "a's branch"),
            text(&b["branch"], "b's branch")
        ),
        ["b.md"]
    );
}

/// A dependent of a parent that landed is placed on the root as it always was, and
/// carries no stack.
#[test]
fn a_dependent_of_a_landed_parent_is_placed_on_the_root() {
    let world = World::new("stack-landed");
    world.repository("local-direct", &[]);
    world.script("a.work", "a's change\n");
    world.script("b.work", "b's change\n");
    world.script("b.requires-file", "a.md");
    let run = "landed";
    settle(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
        &[],
    );
    let a = settled(&world, run, "a");
    assert_eq!(a["landing"], "landed", "{a}");
    let b = settled(&world, run, "b");
    assert_eq!(b["outcome"], "merged", "{b}");
    assert_eq!(b["base"], "main", "{b}");
    assert!(b.get("stack").is_none(), "{b}");
}

/// An explicit `base_branch` is the root a stack base is assembled on, while the
/// stack parents set the node's base. A root the origin does not have fails the
/// node before its session opens: the settlement names the stack it had and
/// neither a base nor a head.
#[test]
fn an_explicit_base_branch_is_the_root_of_the_stack_base() {
    let world = team_world("stack-root");
    let repo = service(&world);
    // `develop` is `main` and one more commit, on the origin.
    let checkout = &repo.checkout;
    git(&world, checkout, &["checkout", "-b", "develop"]);
    std::fs::write(checkout.join("develop.md"), "develop\n").expect("a file");
    git(&world, checkout, &["add", "develop.md"]);
    git(&world, checkout, &["commit", "-m", "chore: develop"]);
    git(&world, checkout, &["push", "origin", "develop"]);
    git(&world, checkout, &["checkout", "main"]);
    for id in ["a", "c", "e", "x"] {
        world.script(&format!("{id}.work"), &format!("{id}'s change\n"));
    }
    let mut e = lifecycle("e", &["a", "c"]);
    e["base_branch"] = json!("develop");
    let mut x = lifecycle("x", &["a", "c"]);
    x["base_branch"] = json!("nowhere");
    let run = "root";
    settle(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("c", &[]), e, x],
        &[],
    );

    let e = settled(&world, run, "e");
    let stack_base = format!("{PREFIX}stack-base/{run}/e");
    assert_eq!(e["status"], "done", "{e}");
    assert_eq!(e["base"], stack_base.as_str(), "{e}");
    assert_eq!(file_on(&world, &repo, &stack_base, "develop.md"), "develop");
    assert_eq!(
        changed_against(&world, &repo, &stack_base, text(&e["branch"], "e's branch")),
        ["e.md"]
    );

    let x = settled(&world, run, "x");
    assert_eq!(x["status"], "failed", "{x}");
    assert_eq!(x["outcome"], "infrastructure-failure", "{x}");
    let detail = text(&x["detail"], "x's detail");
    assert!(
        detail.contains(&format!("{PREFIX}stack-base/{run}/x")),
        "{detail}"
    );
    assert!(
        x.get("base").is_none(),
        "no session opened, so no base: {x}"
    );
    assert!(
        x.get("head").is_none(),
        "no branch was cut, so no head: {x}"
    );
    assert_eq!(
        x["stack"]["stack_base"],
        format!("{PREFIX}stack-base/{run}/x"),
        "{x}"
    );
    assert!(x.get("branch").is_none(), "{x}");
}

/// A second publication of a stacked node — a re-dispatch onto its branch after its
/// checks failed — assembles its stack base again, still leaves exactly one Stack
/// block, and keeps text written outside it between the two publications.
///
/// The root moves between the two attempts, so the second assembly advances the
/// stack base over it rather than finding it as it was, and the continued branch
/// carries it while its change request still shows only its own change.
#[test]
fn a_republished_stack_keeps_one_block_and_the_text_around_it() {
    let world = team_world("stack-republish").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "2");
    let repo = service(&world);
    world.script("a.work", "a's change\n");
    world.script("c.work", "c's change\n");
    world.script("f.work-anew", "f's change");
    // `f`'s change request is the third the host opens; its checks fail.
    world.script("gh.checks.3", RED);
    let meeting = world.rendezvous("f");
    let run = "republish";
    let mut plan = plan_of(
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("c", &[]),
            lifecycle("f", &["a", "c"]),
        ],
    );
    plan["concurrency"] = json!(1);
    let path = world.plan(run, &plan);
    world.run(&["start", &path, "--detach"]).exited(0);
    let stack_base = format!("{PREFIX}stack-base/{run}/f");

    // While the first attempt works, the root moves on.
    let first = meeting.arrived();
    let assembled_first = git(&world, &repo.origin, &["rev-parse", &stack_base]);
    let checkout = &repo.checkout;
    std::fs::write(checkout.join("root.md"), "the root moved\n").expect("a file");
    git(&world, checkout, &["add", "root.md"]);
    git(&world, checkout, &["commit", "-m", "chore: move the root"]);
    git(&world, checkout, &["push", "origin", "main"]);
    first.release();

    let second = meeting.arrived();
    // The first publication opened the change request and wrote its block.
    let opened = world.fakes.join("gh").join("opened.jsonl");
    let changes: Vec<Value> = std::fs::read_to_string(&opened)
        .expect("the host opened change requests")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a change request"))
        .collect();
    let edited: Vec<String> = changes
        .into_iter()
        .map(|mut change| {
            if change["number"] == 3 {
                let body = text(&change["body"], "f's body").to_owned();
                assert!(
                    body.contains(OPEN),
                    "the first publication wrote no block:\n{body}"
                );
                change["body"] = json!(format!("A reviewer wrote this.\n\n{body}\n\nAnd this."));
            }
            change.to_string()
        })
        .collect();
    std::fs::write(&opened, format!("{}\n", edited.join("\n"))).expect("the edit is written");
    second.release();
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });

    let f = settled(&world, run, "f");
    assert_eq!(f["outcome"], "checks-failed", "{f}");
    assert_eq!(f["base"], stack_base.as_str(), "{f}");
    assert_eq!(
        world
            .events_of(run, "node-dispatched")
            .iter()
            .filter(|event| event["labels"]["node"] == "f")
            .count(),
        2,
        "f was published twice"
    );
    // Assembled again over the moved root: advanced, forward-only, and carried.
    let assembled_second = git(&world, &repo.origin, &["rev-parse", &stack_base]);
    assert_ne!(
        assembled_first, assembled_second,
        "the stack base did not advance"
    );
    git(
        &world,
        &repo.origin,
        &[
            "merge-base",
            "--is-ancestor",
            assembled_first.trim(),
            &stack_base,
        ],
    );
    let f_branch = text(&f["branch"], "f's branch");
    for branch in [stack_base.as_str(), f_branch] {
        assert_eq!(file_on(&world, &repo, branch, "root.md"), "the root moved");
    }
    assert_eq!(
        changed_against(&world, &repo, &stack_base, f_branch),
        ["f.md"]
    );

    let description = change_from(&world, f_branch)["body"]
        .as_str()
        .expect("a body")
        .to_owned();
    let block = one_block(&description, "A reviewer wrote this.");
    assert!(
        description.trim_end().ends_with("And this."),
        "{description}"
    );
    assert!(block.contains(&stack_base), "{block}");
    for parent in ["a", "c"] {
        assert!(block.contains(text(&settled(&world, run, parent)["change_url"], "url")));
    }
}

/// The settlement's `base`, `head` and `stack` reach the store the plan was read
/// from, inside the projected `onepipeline.settlement`.
#[test]
fn a_stacked_settlement_is_projected_onto_the_store() {
    let world = team_world("stack-store");
    world.script("a.work", "a's change\n");
    world.script("b.work", "b's change\n");
    let run = "store";
    let mut plan = plan_of(run, vec![lifecycle("a", &[]), lifecycle("b", &["a"])]);
    plan["concurrency"] = json!(1);
    let project = world.plan(run, &plan);
    world.run_from(&world.project, &["start", &project, "--attach"]);
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    let b = settled(&world, run, "b");
    let a_branch = settled(&world, run, "a")["branch"].clone();
    world.until_store("b's settlement to reach the store", |world| {
        world.store_tasks(&project).iter().any(|task| {
            let metadata = &task["item"]["metadata"];
            metadata["onepipeline.id"] == "b"
                && metadata["onepipeline.settlement"]["status"] == "done"
        })
    });
    let tasks = world.store_tasks(&project);
    let projected = tasks
        .iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == "b")
        .map(|task| task["item"]["metadata"]["onepipeline.settlement"].clone())
        .expect("b is on the store");
    assert_eq!(projected["base"], a_branch, "{projected}");
    assert_eq!(projected["head"], b["head"], "{projected}");
    assert_eq!(projected["stack"], b["stack"], "{projected}");
    assert_eq!(projected["stack"]["parents"][0]["node"], "a", "{projected}");
}
