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

/// Put [`BRANCH`] on the origin the way a run's publication leaves interrupted
/// work — its commit marked an incomplete step — which is what `recover` lands.
fn preserved_branch(world: &World, repository: &Repository) {
    branch_with_work_under(world, repository, "feat: add the widget (incomplete step)");
    let preserved = world
        .cmd_on(&onevcs_binary(), &["preserve", BRANCH, "--repo", "service"])
        .output()
        .expect("onevcs runs");
    assert!(
        preserved.status.success(),
        "the branch could not be preserved: {}",
        String::from_utf8_lossy(&preserved.stderr)
    );
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
    preserved_branch(&world, &repository);
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

/// The synopsis entry 88 of `docs/contract-divergences.md` proposes is what each
/// verb's `--help` prints, byte for byte — so the register and the binary cannot
/// come to describe two different commands.
#[test]
fn the_synopsis_the_register_proposes_is_the_one_the_binary_prints() {
    let register = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
    )
    .expect("the register ships");
    let entry = register
        .split("\n## 88. ")
        .nth(1)
        .expect("the register carries entry 88");
    let proposed: Vec<&str> = entry
        .split("```")
        .nth(1)
        .expect("entry 88 states its synopsis in a fenced block")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let world = World::new("oob-synopsis");
    let printed: Vec<String> = ["publish-branch", "repo-recover"]
        .into_iter()
        .map(|verb| {
            let help = world.run(&[verb, "--help"]);
            help.exited(0);
            help.stdout
                .lines()
                .find_map(|line| line.strip_prefix("Usage: "))
                .unwrap_or_else(|| panic!("`{verb} --help` prints no usage: {}", help.stdout))
                .to_owned()
        })
        .collect();
    assert_eq!(proposed, printed);
}

/// `repo-recover` makes every decision `publish-branch` does: no turn for
/// `--no-draft`, a caller's own body, or a `local-direct` identity; a refusal
/// where it would draft with no graph named; and a landing with no body, its
/// ending named, where the draft dies.
#[test]
fn repo_recover_drafts_only_where_publish_branch_would_and_lands_through_a_dead_draft() {
    // Held across the cases: the last world a process drops takes this process's
    // doubles with it, the `onevcs` binary `preserved_branch` runs included.
    let _held = World::new("oob-recover-held");
    for case in ["no-draft", "body", "local-direct", "no-graph", "died"] {
        let world = World::new(&format!("oob-recover-{case}"));
        let publication = if case == "local-direct" {
            "local-direct"
        } else {
            "change-open"
        };
        // A recovery attests the work complete, so a `local-direct` identity needs
        // something on its merge path to verify it: a `pre-push` hook that lets the
        // publishing push through.
        let pre_push: &[&str] = if case == "local-direct" {
            &["true"]
        } else {
            &[]
        };
        let repository = world.repository(publication, pre_push);
        preserved_branch(&world, &repository);
        let graph = world.pr_author_graph();
        if case == "died" {
            world.script("unknown.died-as", "unstartable spawn no harness to start\n");
        }
        let mut args = vec![
            "repo-recover",
            BRANCH,
            "--repo",
            "service",
            "--title",
            "feat: recover the widget",
        ];
        match case {
            "no-draft" => args.push("--no-draft"),
            "body" => args.extend(["--body", "## What\nRecovered by hand."]),
            _ => {}
        }
        if case != "no-graph" {
            args.extend(["--pr-author-graph", &graph]);
        }
        let landed = world.run(&args);

        let turns = drafting_dispatches(&world).len();
        match case {
            "no-graph" => {
                landed.exited(2).err_has("--pr-author-graph");
                assert_eq!(turns, 0, "{case}");
                assert!(opened(&world).is_empty(), "{case}: {:?}", opened(&world));
                continue;
            }
            "died" => {
                landed
                    .exited(0)
                    .err_has("worker died: rule=unstartable cause=spawn")
                    .err_has("(dispatch-failed), so widget lands with no body");
                assert_eq!(turns, 1, "{case}");
            }
            _ => {
                landed.exited(0);
                assert_eq!(turns, 0, "{case}: a drafting turn was spent");
            }
        }
        if case == "local-direct" {
            assert!(opened(&world).is_empty(), "{:?}", opened(&world));
            assert_eq!(
                repository.base_file("widget.txt").as_deref(),
                Some("a widget\n"),
                "{}",
                world.dump()
            );
        } else {
            let opened = opened(&world);
            assert_eq!(opened.len(), 1, "{case}: {opened:?}\n{}", world.dump());
            let expected = if case == "body" {
                "## What\nRecovered by hand."
            } else {
                ""
            };
            assert_eq!(opened[0].1.trim_end(), expected, "{case}: {opened:?}");
        }
    }
}

/// A `--policy` a `publish-branch` is narrowed to decides whether a body is
/// drafted, over the identity's rules: a `local-direct` identity narrowed to
/// `change-open` opens a change request, so its body is drafted.
#[test]
fn a_policy_narrowed_to_open_a_change_request_is_drafted_for() {
    let world = World::new("oob-narrowed");
    let repository = world.repository("local-direct", &[]);
    branch_with_work(&world, &repository);
    world.script("pr-author.body", "## What\nA widget, reviewed.\n");
    let graph = world.pr_author_graph();
    world
        .run(&[
            "publish-branch",
            BRANCH,
            "--repo",
            "service",
            "--policy",
            "change-open",
            "--title",
            "feat: add the widget",
            "--pr-author-graph",
            &graph,
        ])
        .exited(0);

    assert_eq!(drafting_dispatches(&world).len(), 1, "{}", world.dump());
    assert_eq!(
        opened(&world),
        vec![(
            "feat: add the widget".to_owned(),
            "## What\nA widget, reviewed.".to_owned()
        )],
        "{}",
        world.dump()
    );
}

/// The base a drafter is shown is the origin's default branch however the
/// checkout knows it: from its own record of the origin's `HEAD`, from what the
/// origin advertises when it has no record, and from the one branch it tracks
/// there when the origin advertises nothing.
#[test]
fn the_base_is_found_however_the_checkout_knows_its_origins_default() {
    for case in ["recorded", "advertised", "only-tracked"] {
        let world = World::new(&format!("oob-base-{case}"));
        let repository = world.repository("change-open", &[]);
        branch_with_work(&world, &repository);
        match case {
            "recorded" => {
                git(
                    &world,
                    &repository.checkout,
                    &["remote", "set-head", "origin", "main"],
                );
            }
            "only-tracked" => {
                git(
                    &world,
                    &repository.origin,
                    &["symbolic-ref", "HEAD", "refs/heads/gone"],
                );
            }
            _ => {}
        }
        let graph = world.pr_author_graph();
        world.script("pr-author.body", "## What\nA widget.\n");
        world
            .run(&[
                "publish-branch",
                BRANCH,
                "--repo",
                "service",
                "--pr-author-graph",
                &graph,
            ])
            .exited(0);

        let drafts = drafting_dispatches(&world);
        assert_eq!(drafts.len(), 1, "{case}");
        let task = flag_of(&drafts[0], "--task");
        assert!(
            task.contains("its base is `origin/main`"),
            "{case}: the task names another base:\n{task}"
        );
        assert_eq!(
            opened(&world).first().map(|(_, body)| body.as_str()),
            Some("## What\nA widget."),
            "{case}\n{}",
            world.dump()
        );
    }
}

/// A branch carrying no commit its base does not is drafted from its diff alone,
/// and the task says so rather than listing nothing under a heading.
#[test]
fn a_branch_with_nothing_past_its_base_is_drafted_from_its_diff_alone() {
    let world = World::new("oob-empty");
    let repository = world.repository("change-open", &[]);
    git(&world, &repository.checkout, &["branch", BRANCH]);
    let graph = world.pr_author_graph();
    world.run(&[
        "publish-branch",
        BRANCH,
        "--repo",
        "service",
        "--pr-author-graph",
        &graph,
    ]);

    let drafts = drafting_dispatches(&world);
    assert_eq!(drafts.len(), 1, "{}", world.dump());
    let task = flag_of(&drafts[0], "--task");
    assert!(
        task.contains(
            "The branch carries no commit its base does not, so its diff is the only record \
             there is of what it did."
        ),
        "{task}"
    );
    assert!(!task.contains("The commit messages of"), "{task}");
}

/// A draft that cannot even start — a `--repo` nothing resolves, a branch the
/// checkout does not have, a temporary directory the host will not give — spends
/// no turn and still hands the landing to `onevcs`, saying on standard error which
/// ending it was and why.
#[test]
fn a_draft_that_cannot_start_hands_the_landing_to_onevcs_and_says_why() {
    let _held = World::new("oob-unstarted-held");
    for case in ["unresolved", "no-branch", "no-tmp"] {
        let world = World::new(&format!("oob-unstarted-{case}"));
        let world = if case == "no-tmp" {
            let nowhere = world
                .root
                .join("no-such-tmp")
                .to_string_lossy()
                .into_owned();
            world.with_env("TMPDIR", &nowhere)
        } else {
            world
        };
        let repository = world.repository("change-open", &[]);
        branch_with_work(&world, &repository);
        let graph = world.pr_author_graph();
        let (branch, repo) = match case {
            "unresolved" => (BRANCH, "nowhere-registered"),
            "no-branch" => ("never-cut", "service"),
            _ => (BRANCH, "service"),
        };
        let landed = world.run(&[
            "publish-branch",
            branch,
            "--repo",
            repo,
            "--title",
            "feat: add the widget",
            "--pr-author-graph",
            &graph,
        ]);

        landed
            .err_has("the change request's body was not drafted: the drafting dispatch could not start: ")
            .err_has(&format!("(dispatch-failed), so {branch} lands with no body"));
        let why = match case {
            "unresolved" => "`onevcs resolve nowhere-registered` refused",
            "no-branch" => "has no branch 'never-cut', locally or on its origin",
            _ => "could not be created",
        };
        landed.err_has(why);
        assert!(drafting_dispatches(&world).is_empty(), "{case}");
        // `onevcs` then answered for the landing, in its own words and at its own
        // code: it refuses the two it cannot land, and lands the third bodyless.
        if case == "no-tmp" {
            landed.exited(0);
            assert_eq!(
                opened(&world),
                vec![("feat: add the widget".to_owned(), String::new())],
                "{}",
                world.dump()
            );
        } else {
            assert_ne!(landed.code, 0, "{case}: {}", landed.stderr);
            landed.err_has("onevcs: ");
            assert!(opened(&world).is_empty(), "{case}: {:?}", opened(&world));
        }
    }
}

/// A drafting worktree that cannot be taken back out — the drafter left something
/// in it nobody but its owner can delete — does not cost the landing: the body
/// still lands, and standard error names the worktree, the checkout and the
/// command that removes it, and the directory left for the operator.
#[cfg(unix)]
#[test]
fn a_drafting_worktree_that_cannot_be_removed_is_named_for_the_operator() {
    let world = World::new("oob-unremovable");
    let tmp = world.root.join("tmp");
    std::fs::create_dir_all(&tmp).expect("a temporary directory");
    let world = world.with_env("TMPDIR", &tmp.to_string_lossy());
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository);
    world.script("pr-author.body", "## What\nA widget.\n");
    world.script("pr-author.leaves-unremovable", "1");
    let graph = world.pr_author_graph();
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

    // Given back to the world before anything can fail, so its own removal is not
    // what this journey leaves behind.
    let restored = std::process::Command::new("chmod")
        .args(["-R", "u+w"])
        .arg(&tmp)
        .status()
        .expect("chmod runs");
    assert!(restored.success());

    landed
        .exited(0)
        .err_has("onepipeline: the drafting worktree at ")
        .err_has(&format!(
            "could not be removed from {}",
            repository.checkout.display()
        ))
        .err_has("worktree remove --force")
        .err_has("could not be removed: ")
        .err_has("remove it by hand");
    assert_eq!(
        opened(&world),
        vec![(
            "feat: add the widget".to_owned(),
            "## What\nA widget.".to_owned()
        )],
        "{}",
        world.dump()
    );
}

/// A world whose drafting turns make their directories under its own root, so a
/// landing that failed to remove one leaves it where the world is cleaned up.
#[cfg(unix)]
fn world_with_its_own_tmp(name: &str) -> World {
    let world = World::new(name);
    let tmp = world.root.join("tmp");
    std::fs::create_dir_all(&tmp).expect("a temporary directory");
    world.with_env("TMPDIR", &tmp.to_string_lossy())
}

/// Start `publish-branch` of [`BRANCH`] drafting through `graph`, in a process
/// group of its own so a signal to that group reaches nothing else, behind
/// `launcher` where one is given.
#[cfg(unix)]
fn start_landing(world: &World, graph: &str, launcher: Option<&str>) -> std::process::Child {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    let args = [
        "publish-branch",
        BRANCH,
        "--repo",
        "service",
        "--title",
        "feat: add the widget",
        "--pr-author-graph",
        graph,
    ];
    let mut landing = match launcher {
        None => world.cmd(&args),
        Some(launcher) => {
            let binary = crate::harness::binary().display().to_string();
            let mut words = vec![binary.as_str()];
            words.extend(args);
            world.cmd_on(Path::new(launcher), &words)
        }
    };
    landing.process_group(0);
    landing
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the landing starts")
}

/// The drafting worktree the double recorded once its turn was in flight.
#[cfg(unix)]
fn drafting_worktree(world: &World) -> std::path::PathBuf {
    let recorded = world.fakes.join("pr-author-tree.jsonl");
    world.until("the drafting turn to be in flight", |_| {
        std::fs::read_to_string(&recorded).is_ok_and(|said| said.ends_with('\n'))
    });
    let tree: Value = serde_json::from_str(
        std::fs::read_to_string(&recorded)
            .expect("the drafting turn recorded its tree")
            .trim(),
    )
    .expect("a recorded tree");
    Path::new(tree["dir"].as_str().expect("the tree names its directory")).to_path_buf()
}

/// Send `signal` to the landing, or to its whole group.
#[cfg(unix)]
fn signal_landing(landing: &std::process::Child, signal: i32, whole_group: bool) {
    let pid = i32::try_from(landing.id()).expect("a pid fits");
    // SAFETY: `kill` and `killpg` take an id and a signal and touch no memory;
    // the id is the landing's own, started in a group of its own, so the signal
    // reaches nothing this journey did not start.
    let sent = unsafe {
        if whole_group {
            libc::killpg(pid, signal)
        } else {
            libc::kill(pid, signal)
        }
    };
    assert_eq!(sent, 0, "the landing could not be signalled");
}

/// That a signalled landing ended of `signal` with nothing landed, the checkout's
/// worktree list as `before` had it, and neither the drafting worktree nor its
/// turn's directory left behind.
#[cfg(unix)]
fn ended_leaving_nothing(
    world: &World,
    repository: &Repository,
    ended: &std::process::Output,
    signal: i32,
    before: &str,
    worktree: &Path,
    name: &str,
) {
    use std::os::unix::process::ExitStatusExt;

    let stderr = String::from_utf8_lossy(&ended.stderr);
    assert_eq!(
        ended.status.signal(),
        Some(signal),
        "{name}: the landing did not end of the signal it was sent: {:?}\n{stderr}",
        ended.status
    );
    assert_eq!(
        state_of(world, &repository.checkout).0,
        before,
        "{name}: the checkout's worktree list still names the drafting worktree\n{stderr}"
    );
    assert!(
        !worktree.exists(),
        "{name}: the drafting worktree outlived the landing: {}",
        worktree.display()
    );
    let turn = worktree
        .parent()
        .expect("the worktree is inside its turn's directory");
    assert!(
        !turn.exists(),
        "{name}: the drafting turn's directory outlived the landing: {}",
        turn.display()
    );
    assert_eq!(
        opened(world),
        Vec::new(),
        "{name}: an interrupted landing landed"
    );
    assert!(
        stderr.contains("interrupted while drafting; removing the drafting worktree"),
        "{name}: {stderr}"
    );
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the edge these journeys
// need is the crate under test itself — its own interrupt handler, a real `onevcs`-registered
// checkout, real git and the drafting double — and the one separately edged test project's
// input begins at `{workspaceRoot}/src/**/*`, so a project of their own would declare the same
// dependency and skip nothing. They live beside the other landing journeys in this file, which
// is where a reader looks for one.
/// A landing interrupted mid-draft — the moment a person at a terminal is most
/// likely to give up on it — leaves the named checkout's worktree list as it
/// was and the drafting worktree's directory gone, and still ends of the signal
/// it was sent, with nothing landed.
///
/// Every way it is asked to end: `SIGINT` to its whole process group, as a
/// terminal's Ctrl-C sends it, so the drafter dies of it too; and `SIGTERM` and
/// `SIGHUP` to the landing alone, so the drafter is still holding its worktree
/// when the landing goes.
///
/// `#[cfg(unix)]` because it signals by pid and group.
#[cfg(unix)]
#[test]
fn a_landing_interrupted_mid_draft_takes_its_drafting_worktree_out_of_the_checkout() {
    for (signal, name, whole_group) in [
        (libc::SIGINT, "sigint", true),
        (libc::SIGTERM, "sigterm", false),
        (libc::SIGHUP, "sighup", false),
    ] {
        let world = world_with_its_own_tmp(&format!("oob-interrupted-{name}"));
        let repository = world.repository("change-open", &[]);
        branch_with_work(&world, &repository);
        world.script("pr-author.body", "## What\nA widget.\n");
        world.script("pr-author.records-tree", "1");
        world.script("pr-author.holds", "1");
        let graph = world.pr_author_graph();
        let before = state_of(&world, &repository.checkout);

        let landing = start_landing(&world, &graph, None);
        let worktree = drafting_worktree(&world);
        assert!(
            git(
                &world,
                &repository.checkout,
                &["worktree", "list", "--porcelain"]
            )
            .contains(&worktree.display().to_string()),
            "{name}: the drafting worktree is not in the checkout's list while it drafts"
        );
        signal_landing(&landing, signal, whole_group);
        let ended = landing.wait_with_output().expect("the landing ends");
        // Released only now, so a drafter the signal did not reach goes too.
        world.release("pr-author.go");
        ended_leaving_nothing(
            &world,
            &repository,
            &ended,
            signal,
            &before.0,
            &worktree,
            name,
        );
    }
}

/// A landing signalled while git is still adding its drafting worktree waits for
/// that worktree and then takes it back out, rather than removing nothing and
/// leaving the entry git goes on to write.
///
/// The add is held open by a real `post-checkout` hook, which `git worktree add`
/// runs once the worktree is in the checkout's list; the landing is sent
/// `SIGTERM` alone, so git and its hook outlive the signal, and the hook is
/// released only after the landing has had time to act on it.
#[cfg(unix)]
#[test]
fn a_landing_signalled_while_its_worktree_is_being_added_still_removes_it() {
    use std::os::unix::fs::PermissionsExt;

    let world = world_with_its_own_tmp("oob-interrupted-adding");
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository);
    world.script("pr-author.body", "## What\nA widget.\n");
    let hooks = world.root.join("adding-hooks");
    std::fs::create_dir_all(&hooks).expect("a hooks directory");
    let hook = hooks.join("post-checkout");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\n\
             pwd > '{adding}'\n\
             tries=0\n\
             while [ ! -f '{go}' ] && [ \"$tries\" -lt 1200 ]; do sleep 0.05; tries=$((tries + 1)); done\n",
            adding = world.fakes.join("adding").display(),
            go = world.fakes.join("adding.go").display(),
        ),
    )
    .expect("the hook is written");
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
        .expect("the hook is executable");
    git(
        &world,
        &repository.checkout,
        &["config", "core.hooksPath", &hooks.to_string_lossy()],
    );
    let graph = world.pr_author_graph();
    let before = state_of(&world, &repository.checkout);

    let landing = start_landing(&world, &graph, None);
    let adding = world.fakes.join("adding");
    world.until("git to be adding the drafting worktree", |_| {
        std::fs::read_to_string(&adding).is_ok_and(|said| said.ends_with('\n'))
    });
    let worktree = Path::new(
        std::fs::read_to_string(&adding)
            .expect("the hook said where")
            .trim(),
    )
    .to_path_buf();
    signal_landing(&landing, libc::SIGTERM, false);
    // Long enough for the landing to have acted on the signal before git finishes,
    // which is the ordering this journey is about.
    std::thread::sleep(std::time::Duration::from_millis(500));
    world.release("adding.go");
    let ended = landing.wait_with_output().expect("the landing ends");
    // Nothing to wait out after this: the landing waits on git's add before it
    // records the worktree, so a landing that has ended is one whose git has.
    ended_leaving_nothing(
        &world,
        &repository,
        &ended,
        libc::SIGTERM,
        &before.0,
        &worktree,
        "adding",
    );
}

/// A landing started with a signal ignored — under `nohup`, whose hangup the
/// caller asked to survive — is not ended by that signal mid-draft: it drafts,
/// lands with the drafted body, and removes its worktree as any landing does.
#[cfg(unix)]
#[test]
fn a_landing_under_nohup_drafts_and_lands_through_a_hangup() {
    let world = world_with_its_own_tmp("oob-nohup");
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository);
    world.script("pr-author.body", "## What\nA widget.\n");
    world.script("pr-author.records-tree", "1");
    world.script("pr-author.holds", "1");
    let graph = world.pr_author_graph();
    let before = state_of(&world, &repository.checkout);

    let mut landing = start_landing(&world, &graph, Some("nohup"));
    let worktree = drafting_worktree(&world);
    signal_landing(&landing, libc::SIGHUP, false);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        landing
            .try_wait()
            .expect("the landing is asked about")
            .is_none(),
        "a landing started ignoring SIGHUP was ended by one"
    );
    world.release("pr-author.go");
    let ended = landing.wait_with_output().expect("the landing ends");
    assert!(
        ended.status.success(),
        "{:?}\n{}",
        ended.status,
        String::from_utf8_lossy(&ended.stderr)
    );
    assert_eq!(
        opened(&world),
        vec![(
            "feat: add the widget".to_owned(),
            "## What\nA widget.".to_owned()
        )],
        "{}",
        world.dump()
    );
    assert_eq!(state_of(&world, &repository.checkout), before);
    assert!(!worktree.exists(), "{}", worktree.display());
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
