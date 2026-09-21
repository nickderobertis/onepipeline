//! Landing a branch outside a run: `onepipeline publish-branch` and
//! `onepipeline repo-recover`, the engine's drafter in front of the linked
//! `onevcs` verbs an operator lands a branch with by hand.
//!
//! Every journey drives the compiled binary against a real repository registered
//! with the real linked `onevcs`, over a real origin on disk. What stands in is
//! what every lifecycle journey substitutes: `gh` at `onevcs`'s own `ONEVCS_GH`
//! seam, so a change request is opened offline and recorded, and `oneagentgraph`
//! at its subprocess boundary, so a drafting turn states its answer rather than
//! paying for one. The drafting graph itself is the real document a run's
//! closeout drafts with.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary and the sibling these journeys land through — `onevcs` — is the real library, over
// real git and a real origin on disk. `oneagentgraph` is substituted at its subprocess
// boundary so a journey states a drafting outcome rather than paying for a model turn, and
// GitHub at `onevcs`'s own `ONEVCS_GH` override so a change request can be opened offline.
// `harness.rs` carries the same suppression and the full rationale.

use std::path::Path;

use crate::harness::{git, onevcs_binary, Repository, World};
use serde_json::Value;

/// The opening sentence every drafting dispatch is given, byte for byte, out of
/// band as in a run's closeout.
const DRAFTING_TASK: &str = "Read this branch's diff and write the change request's body, \
     following the repository's own template. The task this branch delivered:";

/// The branch every journey lands.
const BRANCH: &str = "widget";

/// Cut [`BRANCH`] in the checkout with one commit whose message says why, and
/// leave the checkout back on its base — the state an operator lands from.
fn branch_with_work(world: &World, repository: &Repository) -> String {
    branch_with_work_under(world, repository, "feat: add the widget")
}

/// [`branch_with_work`], its commit under `subject`.
fn branch_with_work_under(world: &World, repository: &Repository, subject: &str) -> String {
    let checkout = &repository.checkout;
    git(world, checkout, &["checkout", "-b", BRANCH]);
    std::fs::write(checkout.join("widget.txt"), "a widget\n").expect("the work is written");
    git(world, checkout, &["add", "-A"]);
    git(
        world,
        checkout,
        &[
            "commit",
            "-m",
            &format!("{subject}\n\nOperators asked for a widget to land by hand."),
        ],
    );
    let head = git(world, checkout, &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    git(world, checkout, &["checkout", "main"]);
    head
}

/// Everything about a checkout a landing promises to leave as it found it: its
/// worktree list, its working tree, and what it has checked out.
fn state_of(world: &World, checkout: &Path) -> (String, String, String) {
    (
        git(world, checkout, &["worktree", "list", "--porcelain"]),
        git(world, checkout, &["status", "--porcelain", "--ignored"]),
        git(world, checkout, &["rev-parse", "HEAD"]),
    )
}

/// Every drafting dispatch the doubled `oneagentgraph` was asked for, in order.
fn drafting_dispatches(world: &World) -> Vec<Value> {
    world
        .invocations()
        .into_iter()
        .filter(|call| {
            call["tool"] == "oneagentgraph"
                && call["args"].as_array().is_some_and(|args| {
                    args.iter()
                        .any(|arg| arg == "onepipeline.persona=pr-author")
                })
        })
        .collect()
}

/// The value one recorded invocation passed after `flag`.
fn flag_of(call: &Value, flag: &str) -> String {
    let args = call["args"].as_array().expect("an invocation carries args");
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|at| args.get(at + 1))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the invocation passed no {flag}: {call}"))
        .to_owned()
}

/// The change requests the host was asked to open, as `(title, body)`.
fn opened(world: &World) -> Vec<(String, String)> {
    world
        .changes_opened()
        .iter()
        .map(|change| {
            (
                change["title"].as_str().unwrap_or_default().to_owned(),
                change["body"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// The journey the verbs exist for: a branch landed by hand onto an identity that
/// opens change requests gets the body the engine's drafter writes, drafted in a
/// temporary detached worktree of the branch that is gone afterwards, with the
/// named checkout left exactly as it was — and `--repo` naming the registered
/// alias rather than a path.
#[test]
fn publish_branch_lands_a_change_request_with_the_body_its_branch_was_drafted() {
    let world = World::new("oob-drafted");
    let repository = world.repository("change-open", &[]);
    let head = branch_with_work(&world, &repository);
    world.script(
        "pr-author.body",
        "## What\nA widget, drafted out of band.\n",
    );
    world.script("pr-author.records-tree", "1");
    let graph = world.pr_author_graph();
    let before = state_of(&world, &repository.checkout);

    let landed = world.run(&[
        "publish-branch",
        BRANCH,
        "--repo",
        "service",
        "--title",
        "feat: add the widget",
        "--pr-author-graph",
        &graph,
    ]);
    landed.exited(0);

    // The body the drafter wrote is what the change request opened with.
    assert_eq!(
        opened(&world),
        vec![(
            "feat: add the widget".to_owned(),
            "## What\nA widget, drafted out of band.".to_owned()
        )],
        "the drafted body did not reach the change request\n{}",
        world.dump()
    );

    // Exactly one drafting turn, through the graph that was named, on the task a
    // run's closeout composes with the branch standing in for the node.
    let drafts = drafting_dispatches(&world);
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    assert_eq!(drafts[0]["args"][1], graph.as_str(), "{}", drafts[0]);
    let task = flag_of(&drafts[0], "--task");
    assert!(task.starts_with(DRAFTING_TASK), "{task}");
    for said in [
        "This branch is `widget`, and its base is `origin/main`.",
        "the full commit messages of `origin/main..widget`",
        "A thin `## Why` is a correct outcome here.",
        "feat: add the widget",
        "Operators asked for a widget to land by hand.",
    ] {
        assert!(
            task.contains(said),
            "the task does not say {said:?}:\n{task}"
        );
    }
    for absent in ["## Change request", "## Worker transcript"] {
        assert!(
            !task.contains(absent),
            "the task carries {absent:?}:\n{task}"
        );
    }

    // It ran in a detached worktree of the branch — not the checkout — which is
    // gone now, and the checkout is byte-for-byte what it was.
    let dir = flag_of(&drafts[0], "--dir");
    assert_ne!(Path::new(&dir), repository.checkout, "{dir}");
    assert!(
        !Path::new(&dir).exists(),
        "the drafting worktree outlived the landing: {dir}"
    );
    let trees: Vec<Value> = std::fs::read_to_string(world.fakes.join("pr-author-tree.jsonl"))
        .expect("the drafting turn recorded its tree")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a recorded tree"))
        .collect();
    assert_eq!(trees.len(), 1, "{trees:?}");
    assert_eq!(trees[0]["dir"], dir.as_str());
    assert_eq!(
        trees[0]["head"],
        head.as_str(),
        "the drafter read another tree"
    );
    assert_eq!(
        trees[0]["detached"], true,
        "the drafter's worktree was not detached"
    );
    assert_eq!(
        state_of(&world, &repository.checkout),
        before,
        "the landing left the named checkout changed"
    );
}

/// `repo-recover` is the same drafter in front of `onevcs recover`: a preserved
/// branch landed by hand opens its change request with the drafted body, under the
/// title the caller passed through.
#[test]
fn repo_recover_lands_a_preserved_branch_with_the_body_it_was_drafted() {
    let world = World::new("oob-recover");
    let repository = world.repository("change-open", &[]);
    // What `recover` lands is interrupted work: a commit a run left marked as an
    // incomplete step, which is what `onevcs` reads that off.
    branch_with_work_under(
        &world,
        &repository,
        "feat: add the widget (incomplete step)",
    );
    let preserved = world
        .cmd_on(&onevcs_binary(), &["preserve", BRANCH, "--repo", "service"])
        .output()
        .expect("onevcs runs");
    assert!(
        preserved.status.success(),
        "the branch could not be preserved: {}",
        String::from_utf8_lossy(&preserved.stderr)
    );
    world.script("pr-author.body", "## What\nA recovered widget.\n");
    let graph = world.pr_author_graph();

    world
        .run(&[
            "repo-recover",
            BRANCH,
            "--pr-author-graph",
            &graph,
            "--repo",
            &repository.checkout.to_string_lossy(),
            "--title",
            "feat: recover the widget",
        ])
        .exited(0);

    assert_eq!(
        opened(&world),
        vec![(
            "feat: recover the widget".to_owned(),
            "## What\nA recovered widget.".to_owned()
        )],
        "{}",
        world.dump()
    );
    assert_eq!(drafting_dispatches(&world).len(), 1);
}

/// A `local-direct` identity opens no change request, so there is no description
/// for a body to be: it lands on its base with no drafting turn spent, even with a
/// drafting graph named — and with none named it is not refused for the lack.
#[test]
fn a_local_direct_identity_lands_with_no_drafting_turn_spent() {
    for named in [true, false] {
        let world = World::new("oob-direct");
        let repository = world.repository("local-direct", &[]);
        branch_with_work(&world, &repository);
        let graph = world.pr_author_graph();
        let mut args = vec!["publish-branch", BRANCH, "--repo", "service"];
        if named {
            args.extend(["--pr-author-graph", &graph]);
        }
        world.run(&args).exited(0);

        assert!(
            drafting_dispatches(&world).is_empty(),
            "a local-direct landing spent a drafting turn (graph named: {named})"
        );
        assert!(opened(&world).is_empty(), "{:?}", opened(&world));
        assert_eq!(
            repository.base_file("widget.txt").as_deref(),
            Some("a widget\n"),
            "the branch did not land on its base (graph named: {named})\n{}",
            world.dump()
        );
    }
}

/// A caller who said `--no-draft`, or brought a body of their own, spends no
/// drafting turn: the change request opens with their body, or with none.
#[test]
fn no_draft_and_a_callers_own_body_land_with_no_drafting_turn_spent() {
    for (case, extra, expected) in [
        ("no-draft", vec!["--no-draft"], ""),
        (
            "body",
            vec!["--body", "## What\nThe operator's own."],
            "## What\nThe operator's own.",
        ),
        (
            "body-file",
            vec!["--body-file", "FILE"],
            "## What\nFrom the operator's file.",
        ),
    ] {
        let world = World::new(&format!("oob-{case}"));
        let repository = world.repository("change-open", &[]);
        branch_with_work(&world, &repository);
        let graph = world.pr_author_graph();
        let file = world.root.join("body.md");
        std::fs::write(&file, "## What\nFrom the operator's file.\n").expect("a body file");
        let file = file.to_string_lossy().into_owned();
        let mut args = vec!["publish-branch", BRANCH, "--repo", "service"];
        args.extend(
            extra
                .iter()
                .map(|arg| if *arg == "FILE" { file.as_str() } else { arg }),
        );
        args.extend([
            "--title",
            "feat: add the widget",
            "--pr-author-graph",
            &graph,
        ]);
        world.run(&args).exited(0);

        assert!(
            drafting_dispatches(&world).is_empty(),
            "{case}: a drafting turn was spent"
        );
        let opened = opened(&world);
        assert_eq!(opened.len(), 1, "{case}: {opened:?}\n{}", world.dump());
        assert_eq!(opened[0].1.trim_end(), expected, "{case}: {opened:?}");
    }
}

/// Every way a drafting graph can end without a body — a member that died, every
/// answer the schema refused, an answer with nothing in it — lands the branch
/// anyway with no body, and standard error names which ending it was in the run
/// path's own words, with `oneagentgraph`'s classification of the death where a
/// member died.
#[test]
fn a_drafting_graph_that_ends_without_a_body_still_lands_the_branch() {
    for (case, script, body, ending, says) in [
        (
            "died",
            "unknown.died-as",
            "unstartable spawn the harness could not be started\n",
            "dispatch-failed",
            "the drafting dispatch settled without succeeding",
        ),
        (
            "unschematic",
            "pr-author.unschematic",
            "1",
            "schema-refused",
            "answered nothing the schema it was validated against accepted",
        ),
        (
            "bodyless",
            "pr-author.bodyless",
            "1",
            "no-body",
            "succeeded and there was no body in what it answered with",
        ),
    ] {
        let world = World::new(&format!("oob-{case}"));
        let repository = world.repository("change-open", &[]);
        branch_with_work(&world, &repository);
        world.script(script, body);
        let graph = world.pr_author_graph();
        let before = state_of(&world, &repository.checkout);

        let landed = world.run(&[
            "publish-branch",
            BRANCH,
            "--repo",
            "service",
            "--title",
            "feat: add the widget",
            "--pr-author-graph",
            &graph,
        ]);
        landed.exited(0).err_has(&format!(
            "the change request's body was not drafted: the drafting dispatch {}",
            says.trim_start_matches("the drafting dispatch ")
        ));
        landed.err_has(&format!("({ending}), so widget lands with no body"));
        if case == "died" {
            landed.err_has(
                "worker died: rule=unstartable cause=spawn detail=the harness could not be \
                 started",
            );
        } else {
            landed.err_lacks(" died: ");
        }

        assert_eq!(drafting_dispatches(&world).len(), 1, "{case}: no retry");
        assert_eq!(
            opened(&world),
            vec![("feat: add the widget".to_owned(), String::new())],
            "{case}: the landing did not open its change request with no body\n{}",
            world.dump()
        );
        assert_eq!(
            state_of(&world, &repository.checkout),
            before,
            "{case}: the named checkout was left changed"
        );
    }
}

/// The passthrough: what the caller typed reaches the `onevcs` verb unchanged and
/// in order — this crate's two flags anywhere among it — and a refused argument is
/// that verb's own refusal, with its usage and its exit status, and nothing
/// drafted. A landing that would draft with no graph named is refused before
/// anything runs.
#[test]
fn every_argument_reaches_the_onevcs_verb_and_its_refusals_are_its_own() {
    let world = World::new("oob-passthrough");
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository);
    let graph = world.pr_author_graph();

    // An argument `onevcs publish-branch` does not take is its refusal.
    world
        .run(&[
            "publish-branch",
            BRANCH,
            "--repo",
            "service",
            "--pr-author-graph",
            &graph,
            "--draft-it-please",
        ])
        .exited(2)
        .err_has("unexpected argument '--draft-it-please'")
        .err_has("Usage: onevcs publish-branch");
    // As is a `--policy` its parser does not know, value and all.
    world
        .run(&[
            "publish-branch",
            BRANCH,
            "--repo",
            "service",
            "--policy",
            "sideways",
        ])
        .exited(2)
        .err_has("sideways");
    // Drafting would happen, and nothing can draft: refused, with nothing landed.
    world
        .run(&["publish-branch", BRANCH, "--repo", "service"])
        .exited(2)
        .err_has("--pr-author-graph");
    assert!(drafting_dispatches(&world).is_empty());
    assert!(opened(&world).is_empty(), "{:?}", opened(&world));

    // And a line interleaving both crates' arguments — a title that reads like
    // this crate's flag, the graph named mid-line with `=`, `--no-draft` last —
    // lands with the caller's title verbatim and nothing drafted.
    world
        .run(&[
            "publish-branch",
            "--title",
            "feat: land --no-draft as a title",
            &format!("--pr-author-graph={graph}"),
            BRANCH,
            "--repo=service",
            "--no-draft",
        ])
        .exited(0);
    assert!(drafting_dispatches(&world).is_empty());
    assert_eq!(
        opened(&world),
        vec![("feat: land --no-draft as a title".to_owned(), String::new())],
        "{}",
        world.dump()
    );
}
