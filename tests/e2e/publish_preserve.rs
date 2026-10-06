//! A lifecycle node whose work is **kept, not landed**: `publish: "preserve"`.
//!
//! Every journey here drives the compiled binary over a real plan store and a
//! real `onevcs` against a real origin on disk, and reads the answer off the
//! repository itself as well as off the run: a kept branch is only worth anything
//! if it is on the origin under the name the settlement gives, at the commit the
//! settlement names, with the base untouched.

// llmlint: ignore-file[e2e_not_mocked] the repository side is not substituted at all:
// `onevcs` is the library this crate links, driving real git against a bare origin on
// disk, and `gh` stands in at that library's own `ONEVCS_GH` override only so that a
// change request nobody should open would be recorded if one were. `oneagentgraph` is the
// one double, because what these journeys are about is the closeout and a real agent
// turn is a paid one.

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] about 25 seconds under
// the suite's parallelism, each journey but the loader's cutting a real branch through the
// linked `onevcs` over real git. What they exercise is `lifecycle`'s closeout, `graph`'s and
// `taskgraph`'s load rules, `edits`, `projection`, `summary`, `checkpoint` and `views`
// together, which any change under `src/` can move, so a project edged narrower than the
// crate would drop them out of `nx affected` for the very changes they exist to catch — the
// ground `mod lifecycle` and `mod draft_lifecycle` sit on, whose closeout this one varies.

use std::path::Path;

use serde_json::{json, Value};

use crate::harness::{agent, git, human, lifecycle, plan_of, World, REFUSED};

/// What `plan check` exits with when the plan carries refusals.
const HAS_REFUSALS: i32 = 1;

/// A lifecycle node that keeps its branch.
fn kept(id: &str, deps: &[&str]) -> Value {
    let mut node = lifecycle(id, deps);
    node["publish"] = json!("preserve");
    node
}

/// The one JSON object `plan check --json` prints.
fn answer(run: &crate::harness::Run) -> Value {
    serde_json::from_str(run.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "`onepipeline {}` did not print one JSON object ({error}):\n{}",
            run.args, run.stdout
        )
    })
}

/// The engine's own refusals, in the order it made them.
fn engine_refusals(answered: &Value) -> Vec<Value> {
    answered["refusals"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer has no refusals list: {answered}"))
        .iter()
        .filter(|refusal| refusal["source"] == json!("engine"))
        .cloned()
        .collect()
}

/// Run `plan check --json` over a plan and hand back its one engine refusal.
fn refused_by_check(world: &World, name: &str, plan: &Value) -> Value {
    let project = world.plan(name, plan);
    let checked = world.run(&["plan", "check", &project, "--json"]);
    checked.exited(HAS_REFUSALS);
    let answered = answer(&checked);
    let refusals = engine_refusals(&answered);
    assert_eq!(refusals.len(), 1, "{answered}");
    refusals[0].clone()
}

/// What the origin carries under a branch name, if anything.
fn origin_tip(origin: &Path, branch: &str) -> Option<String> {
    let shown = std::process::Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch}^{{commit}}"),
        ])
        .current_dir(origin)
        .output()
        .expect("git runs");
    shown
        .status
        .success()
        .then(|| String::from_utf8_lossy(&shown.stdout).trim().to_owned())
}

/// One file's contents at a commit of a repository.
fn file_at(world: &World, repo: &Path, commit: &str, name: &str) -> String {
    git(world, repo, &["show", &format!("{commit}:{name}")])
}

/// The one node of a settled run's result document.
fn settled_node(world: &World, run: &str) -> Value {
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    world.run_json(run, "result.json")["nodes"][0].clone()
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

/// The worktree the run's one session opened, as its `session-opened` named it.
fn opened_worktree(world: &World, run: &str) -> String {
    let opened = world.events_of(run, "session-opened");
    assert!(!opened.is_empty(), "no session opened: {}", world.dump());
    opened[0]["payload"]["worktree"]
        .as_str()
        .expect("a session names its worktree")
        .to_owned()
}

/// The whole journey: a plan holding one kept node, run against a real origin
/// with a drafting graph named and an identity that opens change requests — so
/// the drafter and the change request are each something that **would** have
/// happened under a publication, and did not.
#[test]
fn a_preserved_node_puts_its_branch_on_the_origin_and_lands_nothing() {
    let world = World::new("preserve-kept");
    let repo = world.repository("change-open", &[]);
    world.script("spike.work", "budget: 40\n");
    let drafting = world.pr_author_graph();
    // A bar naming a value in the file the spike writes, so the branch is read
    // against it before the close releases the worktree.
    let mut spike = kept("spike", &[]);
    spike["task"] = json!(
        "## What\nMeasure the budget.\n\n## Why\nThe feature is sized by it.\n\n\
         ## Acceptance criteria\n\n- the measured row in `spike.md` is `budget: 40`\n"
    );
    let path = world.plan("kept", &plan_of("kept", vec![spike]));
    world
        .run(&["start", &path, "--attach", "--pr-author-graph", &drafting])
        .settled();

    let node = settled_node(&world, "kept");
    assert_eq!(node["status"], "done", "{node}\n{}", world.dump());
    assert_eq!(node["outcome"], "preserved", "{node}");
    assert_eq!(node["remote"], "pushed", "{node}");
    assert!(
        node.get("landing").is_none(),
        "a kept branch was recorded as a landing: {node}"
    );
    let branch = node["branch"].as_str().expect("a branch").to_owned();
    let head = node["head"].as_str().expect("a head").to_owned();
    assert_eq!(head.len(), 40, "the head is not a full commit: {head}");
    assert_eq!(world.run_json("kept", "result.json")["state"], "complete");

    // The run's summary, as the engine wrote it, before any view reads the run.
    let summary = world.run_json("kept", "summary.json");
    assert_eq!(
        summary["preserved"]["spike"],
        json!({"outcome": "preserved", "branch": branch, "head": head, "remote": "pushed"}),
        "{summary}"
    );
    assert!(
        summary
            .get("landings")
            .is_none_or(|landings| landings.get("spike").is_none()),
        "a kept branch was counted as a landing: {summary}"
    );

    assert_eq!(
        origin_tip(&repo.origin, &branch).as_deref(),
        Some(head.as_str()),
        "the origin does not carry {branch} at the head the settlement names"
    );
    assert_eq!(
        file_at(&world, &repo.origin, &head, "spike.md").trim(),
        "budget: 40"
    );
    assert_eq!(
        repo.base_commits(&world),
        vec!["chore: seed the repository".to_string()],
        "a kept branch reached the base"
    );

    // No drafter ran and no change request was opened, though both were wired.
    let drafted: Vec<Value> = world
        .invocations()
        .into_iter()
        .filter(|call| {
            call["tool"] == "oneagentgraph"
                && call["args"].as_array().is_some_and(|args| {
                    args.iter()
                        .any(|arg| arg == "onepipeline.persona=pr-author")
                })
        })
        .collect();
    assert!(drafted.is_empty(), "a drafter ran: {drafted:?}");
    assert!(
        world.changes_opened().is_empty(),
        "a change request was opened: {:?}",
        world.changes_opened()
    );
    let kinds = vcs_kinds(&world, "kept");
    for published in ["push", "published", "change-opened", "merge-completed"] {
        assert!(
            !kinds.iter().any(|kind| kind == published),
            "a kept branch was published ({published}): {kinds:?}"
        );
    }
    // The sibling's own record of the preservation reached the run's journal,
    // and the session closed with its worktree released.
    assert!(
        kinds.iter().any(|kind| kind == "branch-preserved"),
        "{kinds:?}"
    );
    assert!(
        kinds.iter().any(|kind| kind == "session-closed"),
        "{kinds:?}"
    );
    let worktree = opened_worktree(&world, "kept");
    assert!(
        !Path::new(&worktree).exists(),
        "the session's worktree {worktree} was not released"
    );
    // The branch was read against the node's bar while it was still in hand.
    let compared: Vec<Value> = world
        .events_of("kept", "criterion-checked")
        .into_iter()
        .map(|event| event["payload"].clone())
        .collect();
    assert_eq!(compared.len(), 1, "{compared:?}");
    assert_eq!(compared[0]["file"], "spike.md", "{compared:?}");
    assert_eq!(compared[0]["answer"], "match", "{compared:?}");

    world
        .run(&["results", "kept"])
        .exited(0)
        .out_has("done (preserved)")
        .out_has(&format!("kept on {branch} at {head} (pushed)"));
    world
        .run(&["status", "kept"])
        .exited(0)
        .out_has(&format!("spike: preserved — kept on {branch} at {head}"));
    let settled_line = format!("node-settled done preserved branch={branch} head={head}");
    world
        .run(&["monitor", "kept"])
        .exited(0)
        .out_has(&settled_line)
        .out_has("remote=pushed");
    // `watch` writes its human line to standard error and its record to
    // standard output, and both say where the work is.
    let watched = world.run(&["watch", "kept", "--timeout", "1"]);
    assert!(
        watched.stderr.contains(&settled_line) && watched.stderr.contains("remote=pushed"),
        "`watch` did not render the kept branch:\n{}",
        watched.stderr
    );
    let record: Value = watched
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["event"]["kind"] == "node-settled")
        .unwrap_or_else(|| panic!("`watch` recorded no settlement:\n{}", watched.stdout));
    assert_eq!(
        record["event"]["payload"]["outcome"], "preserved",
        "{record}"
    );
    assert_eq!(
        record["event"]["payload"]["branch"],
        json!(branch),
        "{record}"
    );
    assert_eq!(record["event"]["payload"]["head"], json!(head), "{record}");
    assert_eq!(record["event"]["payload"]["remote"], "pushed", "{record}");
}

/// A node that states `land` and a node that states nothing publish through the
/// same closeout as before the field existed: the base advances by the change.
#[test]
fn a_node_stating_land_publishes_exactly_as_one_stating_nothing() {
    for (name, publish) in [("omitted", None), ("stated", Some("land"))] {
        let world = World::new(&format!("preserve-land-{name}"));
        let repo = world.repository("local-direct", &[]);
        world.script("service.work", "the worker wrote this\n");
        let mut node = lifecycle("service", &[]);
        if let Some(publish) = publish {
            node["publish"] = json!(publish);
        }
        let path = world.plan(name, &plan_of(name, vec![node]));
        world.run(&["start", &path, "--attach"]).settled();
        let node = settled_node(&world, name);
        assert_eq!(node["status"], "done", "{name}: {node}\n{}", world.dump());
        assert_eq!(node["outcome"], "merged", "{name}: {node}");
        assert_eq!(node["landing"], "landed", "{name}: {node}");
        assert!(node.get("remote").is_none(), "{name}: {node}");
        assert_eq!(
            repo.base_commits(&world),
            vec![
                "feat: ship service".to_string(),
                "chore: seed the repository".to_string()
            ],
            "{name}: the base did not advance by the published change"
        );
    }
}

/// A kept node whose worker committed nothing settles `empty-branch`, exactly as
/// a published one does, and puts nothing on the origin.
#[test]
fn a_preserved_node_level_with_its_base_settles_empty_branch() {
    let world = World::new("preserve-level");
    let repo = world.repository("local-direct", &[]);
    let path = world.plan("level", &plan_of("level", vec![kept("spike", &[])]));
    world.run(&["start", &path, "--attach"]).settled();

    let node = settled_node(&world, "level");
    assert_eq!(node["status"], "failed", "{node}\n{}", world.dump());
    assert_eq!(node["outcome"], "empty-branch", "{node}");
    let branch = node["branch"].as_str().expect("a branch");
    assert_eq!(
        origin_tip(&repo.origin, branch),
        None,
        "a level branch was put on the origin"
    );
    assert!(
        !vcs_kinds(&world, "level")
            .iter()
            .any(|kind| kind == "branch-preserved"),
        "a level branch was preserved"
    );
}

/// A kept node whose worker landed its own commit on the base settles
/// `no-changes`, exactly as a published one does, and keeps nothing: the base
/// already carries the work, so there is nothing a branch could add.
#[test]
fn a_preserved_node_whose_base_already_carries_its_commit_settles_no_changes() {
    let world = World::new("preserve-carried");
    let repo = world.repository("local-direct", &[]);
    world.script("spike.lands-on-base", "main");
    let path = world.plan("carried", &plan_of("carried", vec![kept("spike", &[])]));
    world.run(&["start", &path, "--attach"]).settled();

    let node = settled_node(&world, "carried");
    assert_eq!(node["status"], "done", "{node}\n{}", world.dump());
    assert_eq!(node["outcome"], "no-changes", "{node}");
    assert!(node.get("remote").is_none(), "{node}");
    let branch = node["branch"].as_str().expect("a branch");
    assert_eq!(origin_tip(&repo.origin, branch), None);
    let kinds = vcs_kinds(&world, "carried");
    assert!(
        kinds.iter().any(|kind| kind == "session-closed"),
        "{kinds:?}"
    );
    assert!(
        !kinds.iter().any(|kind| kind == "branch-preserved"),
        "a branch the base already carries was preserved: {kinds:?}"
    );
}

/// A preservation the origin refuses settles `infrastructure-failure`, naming
/// the branch, the commit and the sibling's reason — and once the session has
/// closed, the branch is still on this host at that commit with the work on it.
#[test]
fn a_refused_preservation_fails_the_node_and_leaves_the_work_on_the_local_branch() {
    let world = World::new("preserve-refused");
    let repo = world.repository("local-direct", &[]);
    // The origin itself turns every push down, which no `--no-verify` reaches.
    let hook = repo.origin.join("hooks").join("pre-receive");
    std::fs::write(
        &hook,
        "#!/bin/sh\necho 'this origin takes no pushes' >&2\nexit 1\n",
    )
    .expect("the origin's hook is written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
            .expect("the hook is executable");
    }
    world.script("spike.work", "what the spike measured\n");
    let path = world.plan("refused", &plan_of("refused", vec![kept("spike", &[])]));
    world.run(&["start", &path, "--attach"]).settled();

    let node = settled_node(&world, "refused");
    assert_eq!(node["status"], "failed", "{node}\n{}", world.dump());
    assert_eq!(node["outcome"], "infrastructure-failure", "{node}");
    assert!(node.get("remote").is_none(), "{node}");
    let branch = node["branch"].as_str().expect("a branch").to_owned();
    let head = node["head"].as_str().expect("a head").to_owned();
    let detail = world.events_of("refused", "node-settled")[0]["payload"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(detail.contains(&branch), "{detail}");
    assert!(detail.contains("could not preserve"), "{detail}");
    assert!(detail.contains("push rejected"), "{detail}");

    assert!(
        vcs_kinds(&world, "refused")
            .iter()
            .any(|kind| kind == "session-closed"),
        "the session never closed"
    );
    assert!(
        repo.has_branch(&world, &branch),
        "the branch is gone from the checkout"
    );
    assert_eq!(
        git(&world, &repo.checkout, &["rev-parse", &branch]).trim(),
        head,
        "the local branch is not at the head the settlement names"
    );
    assert_eq!(
        file_at(&world, &repo.checkout, &head, "spike.md").trim(),
        "what the spike measured"
    );
    assert_eq!(origin_tip(&repo.origin, &branch), None);
    assert_eq!(
        repo.base_commits(&world),
        vec!["chore: seed the repository".to_string()]
    );
}

/// An identity with no origin keeps the branch on this host, and says so: the
/// node still settles `preserved`, under `no-remote`, naming the commit nothing
/// outside this host carries.
#[test]
fn a_preserved_node_whose_identity_has_no_origin_settles_no_remote() {
    let world = World::new("preserve-no-remote");
    let repo = world.repository("local-direct", &[]);
    git(&world, &repo.checkout, &["remote", "remove", "origin"]);
    world.script("spike.work", "what the spike measured\n");
    let path = world.plan("local", &plan_of("local", vec![kept("spike", &[])]));
    world.run(&["start", &path, "--attach"]).settled();

    let node = settled_node(&world, "local");
    assert_eq!(node["status"], "done", "{node}\n{}", world.dump());
    assert_eq!(node["outcome"], "preserved", "{node}");
    assert_eq!(node["remote"], "no-remote", "{node}");
    let branch = node["branch"].as_str().expect("a branch").to_owned();
    let head = node["head"].as_str().expect("a head").to_owned();
    assert_eq!(
        git(&world, &repo.checkout, &["rev-parse", &branch]).trim(),
        head,
        "the branch kept on this host is not at the head the settlement names"
    );
    assert_eq!(origin_tip(&repo.origin, &branch), None);
    assert_eq!(
        world.run_json("local", "summary.json")["preserved"]["spike"]["remote"],
        "no-remote"
    );
    world
        .run(&["results", "local"])
        .exited(0)
        .out_has(&format!("kept on {branch} at {head} (no-remote)"));
    world.run(&["status", "local"]).exited(0).out_has(&format!(
        "spike: preserved — kept on {branch} at {head} (no-remote)"
    ));
}

/// A branch the origin already carries at its tip is not pushed again: the node
/// is pinned to a branch somebody already put there, adds nothing to it, and
/// settles `preserved` under `already-on-origin` at that same commit.
#[test]
fn a_preserved_branch_the_origin_already_carries_settles_already_on_origin() {
    let world = World::new("preserve-already");
    let repo = world.repository("local-direct", &[]);
    git(
        &world,
        &repo.checkout,
        &["checkout", "-q", "-b", "spike/kept"],
    );
    std::fs::write(repo.checkout.join("spike.md"), "measured earlier\n")
        .expect("the earlier work is written");
    git(&world, &repo.checkout, &["add", "-A"]);
    git(
        &world,
        &repo.checkout,
        &["commit", "-q", "-m", "feat: measure earlier"],
    );
    git(
        &world,
        &repo.checkout,
        &["push", "-q", "origin", "spike/kept"],
    );
    let pushed = git(&world, &repo.checkout, &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    git(&world, &repo.checkout, &["checkout", "-q", "main"]);

    let mut node = kept("spike", &[]);
    node["branch"] = json!("spike/kept");
    let path = world.plan("already", &plan_of("already", vec![node]));
    world.run(&["start", &path, "--attach"]).settled();

    let node = settled_node(&world, "already");
    assert_eq!(node["status"], "done", "{node}\n{}", world.dump());
    assert_eq!(node["outcome"], "preserved", "{node}");
    assert_eq!(node["remote"], "already-on-origin", "{node}");
    assert_eq!(node["branch"], "spike/kept", "{node}");
    assert_eq!(node["head"], json!(pushed), "{node}");
    assert_eq!(
        origin_tip(&repo.origin, "spike/kept").as_deref(),
        Some(pushed.as_str())
    );
    world
        .run(&["results", "already"])
        .exited(0)
        .out_has(&format!(
            "kept on spike/kept at {pushed} (already-on-origin)"
        ));
}

/// A live edit cannot bring back what a launch refuses: an `add` whose node
/// depends on a kept node is refused, naming both, and nothing is committed.
#[test]
fn a_live_edit_adding_a_dependent_of_a_preserved_node_is_refused() {
    let world = World::new("preserve-edit");
    world.repository("local-direct", &[]);
    world.script("spike.work", "what the spike measured\n");
    world.script("spike.wait", "hold");
    let path = world.plan("edited", &plan_of("edited", vec![kept("spike", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the kept node to be in flight", |world| {
        !world.events_of("edited", "node-dispatched").is_empty()
    });

    let envelope = json!({"version": 2, "commands": [
        {"op": "add", "node": lifecycle("feature", &["spike"])},
    ]})
    .to_string();
    world
        .run_with_stdin(&["reply", "edited"], &envelope)
        .exited(REFUSED)
        .err_has("node 'feature' depends on 'spike'")
        .err_has("publish: \"preserve\"");
    assert!(
        world.events_of("edited", "edit-committed").is_empty(),
        "a refused edit was committed"
    );

    world.release("spike.go");
    let node = settled_node(&world, "edited");
    assert_eq!(node["outcome"], "preserved", "{node}\n{}", world.dump());
    assert_eq!(
        world.run_json("edited", "result.json")["nodes"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "the refused node reached the graph"
    );
}

/// End the process a `<key>.lingers` worker left behind, by the pid it recorded.
///
/// The one process a journey here may signal: the fake started it for this
/// journey and wrote down which one it is.
#[cfg(unix)]
fn end_the_lingering(world: &World, key: &str) {
    // Only a pid the fake could have written: a positive number past `init`, so a
    // file that held anything else signals nothing rather than a process group or
    // every process this user owns.
    let recorded = std::fs::read_to_string(world.fakes.join(format!("{key}.lingering")))
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok())
        .filter(|pid| *pid > 1);
    if let Some(pid) = recorded {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .status();
    }
}

/// A close refused for a moment — the worker left a process in its worktree that
/// ends a second or two later — is asked again before anything is pushed, so what
/// is kept is everything the session made: the work the worker never committed is
/// committed by the close and is on the origin at the head the node settles on.
#[cfg(unix)]
#[test]
fn a_close_refused_for_a_moment_is_asked_again_and_everything_the_session_made_is_kept() {
    let world = World::new("preserve-close-retried");
    let repo = world.repository("local-direct", &[]);
    world.script("spike.work", "what the spike measured\n");
    world.script("spike.lingers", "2");
    let path = world.plan("retried", &plan_of("retried", vec![kept("spike", &[])]));
    world.run(&["start", &path, "--attach"]).settled();
    end_the_lingering(&world, "spike");

    let node = settled_node(&world, "retried");
    assert_eq!(node["status"], "done", "{node}\n{}", world.dump());
    assert_eq!(node["outcome"], "preserved", "{node}");
    let branch = node["branch"].as_str().expect("a branch").to_owned();
    let head = node["head"].as_str().expect("a head").to_owned();
    assert_eq!(
        origin_tip(&repo.origin, &branch).as_deref(),
        Some(head.as_str()),
        "the origin is not at the head the settlement names"
    );
    assert_eq!(
        file_at(&world, &repo.origin, &head, "spike.md").trim(),
        "what the spike measured",
        "the kept branch is missing the work the close committed"
    );
    assert_eq!(
        git(&world, &repo.checkout, &["rev-parse", &branch]).trim(),
        head,
        "the local branch moved past the head that was kept"
    );
}

/// A session that will not close is not preserved at all: nothing is pushed from
/// a session still open, and the node says why.
#[cfg(unix)]
#[test]
fn a_session_that_will_not_close_is_not_preserved() {
    let world = World::new("preserve-unclosed");
    let repo = world.repository("local-direct", &[]);
    world.script("spike.work", "what the spike measured\n");
    world.script("spike.lingers", "60");
    let path = world.plan("unclosed", &plan_of("unclosed", vec![kept("spike", &[])]));
    world.run(&["start", &path, "--attach"]).settled();
    end_the_lingering(&world, "spike");

    let node = settled_node(&world, "unclosed");
    assert_eq!(node["status"], "failed", "{node}\n{}", world.dump());
    assert_eq!(node["outcome"], "infrastructure-failure", "{node}");
    assert!(node.get("remote").is_none(), "{node}");
    let branch = node["branch"].as_str().expect("a branch").to_owned();
    let detail = world.events_of("unclosed", "node-settled")[0]["payload"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(detail.contains("would not close"), "{detail}");
    assert!(detail.contains(&branch), "{detail}");
    assert_eq!(
        origin_tip(&repo.origin, &branch),
        None,
        "a session that never closed was pushed"
    );
    assert!(
        !vcs_kinds(&world, "unclosed")
            .iter()
            .any(|kind| kind == "branch-preserved"),
        "a session that never closed was preserved"
    );
}

/// The loader accepts the field on a lifecycle node, stated either way.
#[test]
fn the_loader_accepts_publish_on_a_lifecycle_node() {
    let world = World::new("preserve-accepted");
    world.repository("local-direct", &[]);
    let mut landing = lifecycle("service", &[]);
    landing["publish"] = json!("land");
    let project = world.plan(
        "accepted",
        &plan_of("accepted", vec![kept("spike", &[]), landing]),
    );
    world.run(&["plan", "check", &project]).exited(0);
}

/// Every shape the field is refused on, each named by node, field and rule.
#[test]
fn the_loader_refuses_publish_preserve_where_nothing_could_be_kept() {
    let world = World::new("preserve-refusals");
    world.repository("change-open", &[]);

    let mut direct = agent("direct", &[]);
    direct["publish"] = json!("preserve");
    let mut person = human("person", &[]);
    person["publish"] = json!("preserve");
    let mut quiet = lifecycle("quiet", &[]);
    quiet["publish"] = json!("preserve");
    quiet["expects_no_diff"] = json!(true);
    quiet.as_object_mut().expect("a node").remove("persona");
    let mut drafted = kept("drafted", &[]);
    drafted["draft"] = json!(true);
    for (node, rule) in [
        (direct, "a direct node"),
        (person, "a `kind: human` node"),
        (quiet, "an `expects_no_diff` node"),
        (drafted, "`draft: true`"),
    ] {
        let id = node["id"].as_str().expect("an id").to_owned();
        let refusal = refused_by_check(&world, &id, &plan_of(&id, vec![node]));
        assert_eq!(refusal["node"], json!(id), "{refusal}");
        assert_eq!(refusal["field"], json!("publish"), "{refusal}");
        let message = refusal["reason"].as_str().expect("a reason");
        assert!(message.contains(&format!("node '{id}'")), "{message}");
        assert!(message.contains(rule), "{message}");
    }

    // A dependent of a kept node: nothing it builds on would reach a base.
    let refusal = refused_by_check(
        &world,
        "dependent",
        &plan_of(
            "dependent",
            vec![kept("spike", &[]), lifecycle("feature", &["spike"])],
        ),
    );
    assert_eq!(refusal["node"], json!("feature"), "{refusal}");
    assert_eq!(refusal["field"], json!("deps"), "{refusal}");
    let message = refusal["reason"].as_str().expect("a reason");
    assert!(
        message.contains("node 'feature' depends on 'spike'"),
        "{message}"
    );
    assert!(message.contains("publish: \"preserve\""), "{message}");

    let mut typo = lifecycle("misspelt", &[]);
    typo["publish"] = json!("keep");
    let refusal = refused_by_check(
        &world,
        "unknown-value",
        &plan_of("unknown-value", vec![typo]),
    );
    let message = refusal["reason"].as_str().expect("a reason");
    assert!(message.contains("node 'misspelt'"), "{message}");
    assert!(
        message.contains("`publish` is `land` or `preserve`"),
        "{message}"
    );
    assert!(message.contains("\"keep\""), "{message}");

    // And `start` refuses the same plan before anything is dispatched.
    let project = world.plan(
        "started",
        &plan_of(
            "started",
            vec![kept("spike", &[]), lifecycle("feature", &["spike"])],
        ),
    );
    world
        .run(&["start", &project, "--detach"])
        .exited(REFUSED)
        .err_has("node 'feature' depends on 'spike'");
    assert!(
        world.invocations().is_empty(),
        "a plan refused at load dispatched something: {:?}",
        world.invocations()
    );
}

/// Below schema 3 the field is refused by its own name, whatever it carries.
#[test]
fn the_loader_refuses_publish_below_schema_3_by_name() {
    let world = World::new("preserve-schema");
    world.repository("local-direct", &[]);
    for publish in ["preserve", "land"] {
        let mut node = lifecycle("spike", &[]);
        node["publish"] = json!(publish);
        let name = format!("older-{publish}");
        let mut plan = plan_of(&name, vec![node]);
        plan["schema_version"] = json!(2);
        let refusal = refused_by_check(&world, &name, &plan);
        assert_eq!(refusal["field"], json!("publish"), "{refusal}");
        let message = refusal["reason"].as_str().expect("a reason");
        assert!(message.contains("node 'spike'"), "{message}");
        assert!(
            message
                .contains("`publish` is a schema 3 field and this plan declares schema_version 2"),
            "{message}"
        );
    }
}

/// Check ancestry on the actual origin, rather than inferring it from settlements.
fn descends(world: &World, origin: &Path, ancestor: &str, branch: &str) {
    git(
        world,
        origin,
        &["merge-base", "--is-ancestor", ancestor, branch],
    );
}

#[test]
fn stacked_spikes_fan_out_from_one_kept_harness() {
    let world = World::new("stacked-spikes-stage");
    let repo = world.repository("change-open", &[]);
    std::fs::write(world.onevcs_home().join("workspaces.yml"),
        "version: 1\nrules:\n  - match: {host: github.com, owner: owner, name: service}\n    pool: 0\n    overflow: 4\n").unwrap();
    world.script("H.work", "shared measurement harness\n");
    let mut nodes = vec![kept("H", &[])];
    for id in ["A", "B", "C", "D"] {
        world.script(&format!("{id}.requires-file"), "H.md");
        world.script(&format!("{id}.work"), "area measurement\n");
        world.script(&format!("{id}.wait"), "hold");
        nodes.push(kept(id, &["H"]));
    }
    let path = world.plan("stacked-stage", &plan_of("stacked-stage", nodes));
    world.run(&["plan", "check", &path]).exited(0);
    world
        .run(&[
            "start",
            &path,
            "--detach",
            "--branch-template",
            "spikes/{{ node.id }}",
        ])
        .exited(0);
    world.until("all four dependents to dispatch concurrently", |world| {
        world.events_of("stacked-stage", "node-dispatched").len() == 5
    });
    world.until("all four dependent sessions to be open together", |world| {
        let opened = world.events_of("stacked-stage", "session-opened");
        ["A", "B", "C", "D"]
            .iter()
            .all(|id| opened.iter().any(|event| event["labels"]["node"] == *id))
    });
    let events = world.journal("stacked-stage");
    let harness_settled = events
        .iter()
        .position(|event| event["kind"] == "node-settled" && event["labels"]["node"] == "H")
        .unwrap();
    for (index, event) in events.iter().enumerate() {
        if event["kind"] == "node-dispatched" && event["labels"]["node"] != "H" {
            assert!(index > harness_settled, "{events:?}");
        }
    }
    for id in ["A", "B", "C", "D"] {
        world.release(&format!("{id}.go"));
    }
    world.until("the fan-out run to settle", |world| {
        world.run_file("stacked-stage", "result.json").is_file()
    });
    let result = world.run_json("stacked-stage", "result.json");
    assert_eq!(result["state"], "complete", "{result}\n{}", world.dump());
    let nodes = result["nodes"].as_array().unwrap();
    let harness = nodes.iter().find(|node| node["id"] == "H").unwrap();
    for node in nodes {
        assert_eq!(node["outcome"], "preserved", "{node}");
        let id = node["id"].as_str().unwrap();
        assert_eq!(node["branch"], format!("spikes/{id}"));
        assert_eq!(
            origin_tip(&repo.origin, node["branch"].as_str().unwrap()).as_deref(),
            node["head"].as_str()
        );
        if id != "H" {
            descends(
                &world,
                &repo.origin,
                harness["head"].as_str().unwrap(),
                node["branch"].as_str().unwrap(),
            );
        }
    }
    assert_eq!(
        repo.base_commits(&world),
        vec!["chore: seed the repository"]
    );
    assert!(world.changes_opened().is_empty());
    let timing = world.run(&["telemetry", "stacked-stage"]);
    timing.exited(0);
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/budget-records");
    std::fs::create_dir_all(&directory).unwrap();
    let telemetry: onepipeline::views::RunTelemetry = serde_json::from_str(&timing.stdout).unwrap();
    std::fs::write(directory.join("stacked-spikes-stage.json"), &timing.stdout).unwrap();
    let result_file = world.root.join("budget-result.json");
    let reported = budget_report(Path::new(env!("CARGO_MANIFEST_DIR")), &result_file);
    assert!(
        reported.status.success(),
        "{}",
        String::from_utf8_lossy(&reported.stderr)
    );
    let result: Value = serde_json::from_slice(&std::fs::read(result_file).unwrap()).unwrap();
    assert_eq!(result["value"], telemetry.wall_ms as f64 / 1000.0);
    assert!(result["detail"]
        .as_str()
        .unwrap()
        .contains(&telemetry.run_id));
}

#[test]
fn a_stacking_chain_selects_the_descendant_and_an_empty_dependent_fails() {
    let world = World::new("stacked-chain");
    let repo = world.repository("local-direct", &[]);
    for id in ["H", "A", "B"] {
        world.script(&format!("{id}.work"), "measurement\n");
    }
    world.script("A.requires-file", "H.md");
    world.script("B.requires-file", "A.md");
    let path = world.plan(
        "chain",
        &plan_of(
            "chain",
            vec![
                kept("H", &[]),
                kept("A", &["H"]),
                kept("B", &["H", "A"]),
                kept("empty", &["B"]),
            ],
        ),
    );
    world.run(&["start", &path, "--attach"]).settled();
    let result = world.run_json("chain", "result.json");
    let nodes = result["nodes"].as_array().unwrap();
    let node = |id| nodes.iter().find(|node| node["id"] == id).unwrap();
    for id in ["H", "A"] {
        descends(
            &world,
            &repo.origin,
            node(id)["head"].as_str().unwrap(),
            node("B")["branch"].as_str().unwrap(),
        );
    }
    assert_eq!(node("empty")["status"], "failed", "{result}");
    assert_eq!(node("empty")["outcome"], "empty-branch", "{result}");
}

#[test]
fn separate_stacking_chains_and_an_explicit_base_are_refused() {
    let world = World::new("stacked-refusals");
    world.repository("local-direct", &[]);
    world.extra_repository("other");
    for mixed in [false, true] {
        let mut nodes = vec![kept("H", &[])];
        if mixed {
            let mut x = kept("X", &["H"]);
            x["repo"] = json!("other");
            nodes.push(x);
        }
        nodes.push(kept("A", if mixed { &["X"] } else { &[] }));
        nodes.push(kept("D", &["H", "A"]));
        let refusal = refused_by_check(
            &world,
            if mixed { "mixed" } else { "independent" },
            &plan_of("fan-in", nodes),
        );
        let reason = refusal["reason"].as_str().unwrap();
        assert!(
            reason.contains("H")
                && reason.contains("A")
                && reason.contains("node 'D'")
                && reason.contains("merging kept branches is not supported"),
            "{reason}"
        );
    }
    let mut dependent = kept("A", &["H"]);
    dependent["base_branch"] = json!("main");
    let refusal = refused_by_check(
        &world,
        "two-bases",
        &plan_of("two-bases", vec![kept("H", &[]), dependent]),
    );
    assert!(
        refusal["reason"].as_str().unwrap().contains("two answers"),
        "{refusal}"
    );
}

fn reply(world: &World, run: &str, commands: Value) -> crate::harness::Run {
    world.run_with_stdin(
        &["reply", run],
        &json!({"version": 3, "commands": commands}).to_string(),
    )
}

#[test]
fn preserve_dependency_rules_hold_at_every_live_edit_boundary() {
    let world = World::new("stacked-edits");
    world.repository("local-direct", &[]);
    world.extra_repository("other");
    world.script("H.work", "harness\n");
    world.script("H.wait", "hold");
    let mut x = kept("X", &["H"]);
    x["repo"] = json!("other");
    let path = world.plan(
        "edits",
        &plan_of(
            "edits",
            vec![
                kept("H", &[]),
                x,
                kept("A", &["X"]),
                human("gate", &[]),
                lifecycle("landing", &["gate"]),
                kept("dependent", &["H"]),
            ],
        ),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("H to dispatch", |world| {
        !world.events_of("edits", "node-dispatched").is_empty()
    });
    for publish in [None, Some("land")] {
        let mut node = lifecycle("new", &["H"]);
        if let Some(publish) = publish {
            node["publish"] = json!(publish);
        }
        reply(&world, "edits", json!([{"op":"add","node":node}]))
            .exited(REFUSED)
            .err_has("node 'new' depends on 'H'");
    }
    let mut draft = lifecycle("drafted", &["H"]);
    draft["draft"] = json!(true);
    reply(&world, "edits", json!([{"op":"add","node":draft}]))
        .exited(REFUSED)
        .err_has("node 'drafted' depends on 'H'");
    reply(
        &world,
        "edits",
        json!([{"op":"reparent","id":"landing","deps":["H"]}]),
    )
    .exited(REFUSED)
    .err_has("node 'landing' depends on 'H'");
    reply(
        &world,
        "edits",
        json!([{"op":"cancel","id":"dependent","reason":"amend placement"}]),
    )
    .exited(0);
    reply(
        &world,
        "edits",
        json!([{"op":"requeue","id":"dependent","amend":{"publish":"land"}}]),
    )
    .exited(REFUSED)
    .err_has("node 'dependent' depends on 'H'");
    reply(
        &world,
        "edits",
        json!([{"op":"add","node":kept("D", &["H", "A"])}]),
    )
    .exited(REFUSED)
    .err_has("H")
    .err_has("A")
    .err_has("merging kept branches is not supported");
    reply(
        &world,
        "edits",
        json!([{"op":"add","node":kept("independent", &[])}]),
    )
    .exited(0);
    reply(
        &world,
        "edits",
        json!([{"op":"add","node":kept("D", &["H", "independent"])}]),
    )
    .exited(REFUSED)
    .err_has("H")
    .err_has("independent")
    .err_has("merging kept branches is not supported");
    assert!(!world
        .events_of("edits", "node-dispatched")
        .iter()
        .any(|event| event["labels"]["node"] == "D"));
    world.release("H.go");
    world.until("the edited run to settle", |world| {
        world.run_file("edits", "result.json").is_file()
    });
}

#[test]
fn cross_repository_preserve_dependencies_only_order_sessions() {
    let world = World::new("stacked-cross-repo");
    let first = world.repository("local-direct", &[]);
    let second = world.extra_repository("other");
    world.script("H.work", "harness\n");
    world.script("A.work", "different repository measurement\n");
    let mut a = kept("A", &["H"]);
    a["repo"] = json!("other");
    let path = world.plan("cross", &plan_of("cross", vec![kept("H", &[]), a]));
    world.run(&["start", &path, "--attach"]).settled();
    let result = world.run_json("cross", "result.json");
    assert_eq!(result["state"], "complete", "{result}\n{}", world.dump());
    let nodes = result["nodes"].as_array().unwrap();
    let a = nodes.iter().find(|node| node["id"] == "A").unwrap();
    let head = a["head"].as_str().unwrap();
    assert_eq!(
        file_at(&world, &second.origin, head, "A.md").trim(),
        "different repository measurement"
    );
    let absent = std::process::Command::new("git")
        .args(["cat-file", "-e", &format!("{head}:H.md")])
        .current_dir(&second.origin)
        .output()
        .unwrap();
    assert!(
        !absent.status.success(),
        "the other repository inherited H's file"
    );
    descends(&world, &second.origin, "main", head);
    assert_eq!(
        first.base_commits(&world),
        vec!["chore: seed the repository"]
    );
    let journal = world.journal("cross");
    let h = journal
        .iter()
        .position(|e| e["kind"] == "node-settled" && e["labels"]["node"] == "H")
        .unwrap();
    let a = journal
        .iter()
        .position(|e| e["kind"] == "node-dispatched" && e["labels"]["node"] == "A")
        .unwrap();
    assert!(h < a);
}

#[test]
fn retrying_the_failed_harness_repoints_and_places_its_skipped_dependents() {
    let world = World::new("stacked-retry");
    let repo = world.repository("local-direct", &[]);
    world.script("H.fail", "1");
    for id in ["replacement", "A", "B"] {
        world.script(&format!("{id}.work"), "measurement\n");
    }
    for id in ["A", "B"] {
        world.script(&format!("{id}.requires-file"), "replacement.md");
    }
    let path = world.plan(
        "retry",
        &plan_of(
            "retry",
            vec![kept("H", &[]), kept("A", &["H"]), kept("B", &["H"])],
        ),
    );
    world.run(&["start", &path, "--attach"]).settled();
    let result = world.run_json("retry", "result.json");
    assert_eq!(result["state"], "failed", "{result}");
    for node in result["nodes"].as_array().unwrap() {
        assert_eq!(
            node["status"],
            if node["id"] == "H" {
                "failed"
            } else {
                "skipped"
            },
            "{result}"
        );
    }
    reply(
        &world,
        "retry",
        json!([{"op":"retry","id":"H","node":kept("replacement", &[])}]),
    )
    .exited(0);
    world.run(&["adopt", "retry"]).settled();
    let result = world.run_json("retry", "result.json");
    let nodes = result["nodes"].as_array().unwrap();
    let replacement = nodes
        .iter()
        .find(|node| node["id"] == "replacement")
        .unwrap();
    for id in ["A", "B"] {
        let node = nodes.iter().find(|node| node["id"] == id).unwrap();
        assert_eq!(node["outcome"], "preserved", "{result}\n{}", world.dump());
        descends(
            &world,
            &repo.origin,
            replacement["head"].as_str().unwrap(),
            node["branch"].as_str().unwrap(),
        );
    }
}

#[test]
fn a_live_preserve_add_starts_from_the_already_settled_kept_branch() {
    let world = World::new("stacked-live-add");
    let repo = world.repository("local-direct", &[]);
    world.script("H.work", "harness\n");
    let path = world.plan("added", &plan_of("added", vec![kept("H", &[])]));
    world.run(&["start", &path, "--attach"]).settled();
    let h = settled_node(&world, "added");
    world.script("A.requires-file", "H.md");
    world.script("A.work", "area measurement\n");
    reply(
        &world,
        "added",
        json!([{"op":"add","node":kept("A", &["H"])}]),
    )
    .exited(0);
    world.run(&["adopt", "added"]).settled();
    let result = world.run_json("added", "result.json");
    let a = result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "A")
        .unwrap();
    assert_eq!(a["outcome"], "preserved", "{result}");
    descends(
        &world,
        &repo.origin,
        h["head"].as_str().unwrap(),
        a["branch"].as_str().unwrap(),
    );
}

#[test]
fn retrying_a_preserve_dependent_keeps_its_base_and_refuses_landing_replacements() {
    let world = World::new("stacked-dependent-retry");
    let repo = world.repository("local-direct", &[]);
    world.script("H.work", "harness\n");
    let path = world.plan(
        "dependent-retry",
        &plan_of("dependent-retry", vec![kept("H", &[]), kept("A", &["H"])]),
    );
    world.run(&["start", &path, "--attach"]).settled();
    let result = world.run_json("dependent-retry", "result.json");
    let a = result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "A")
        .unwrap();
    assert_eq!(a["outcome"], "empty-branch", "{result}");
    let branch = a["branch"].clone();
    reply(
        &world,
        "dependent-retry",
        json!([{"op":"retry","id":"A","node":lifecycle("landing", &["H"])}]),
    )
    .exited(REFUSED)
    .err_has("node 'landing' depends on 'H'");
    let mut replacement = kept("A2", &["H"]);
    replacement["branch"] = branch.clone();
    reply(
        &world,
        "dependent-retry",
        json!([{"op":"retry","id":"A","node":replacement}]),
    )
    .exited(0);
    world.run(&["adopt", "dependent-retry"]).settled();
    let result = world.run_json("dependent-retry", "result.json");
    let a2 = result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "A2")
        .unwrap();
    assert_eq!(a2["branch"], branch);
    assert_eq!(
        a2["outcome"], "empty-branch",
        "a continued empty branch must compare with H, not main: {result}"
    );
    world.script("A3.requires-file", "H.md");
    world.script("A3.work", "area measurement\n");
    let mut replacement = kept("A3", &["H"]);
    replacement["branch"] = branch.clone();
    reply(
        &world,
        "dependent-retry",
        json!([{"op":"retry","id":"A2","node":replacement}]),
    )
    .exited(0);
    world.run(&["adopt", "dependent-retry"]).settled();
    let result = world.run_json("dependent-retry", "result.json");
    let nodes = result["nodes"].as_array().unwrap();
    let h = nodes.iter().find(|node| node["id"] == "H").unwrap();
    let a3 = nodes.iter().find(|node| node["id"] == "A3").unwrap();
    assert_eq!(a3["branch"], branch);
    assert_eq!(a3["outcome"], "preserved", "{result}");
    descends(
        &world,
        &repo.origin,
        h["head"].as_str().unwrap(),
        branch.as_str().unwrap(),
    );
}

/// An old or damaged settled record can name no branch; adoption must fail
/// safely before giving a dependent an unrelated default base.
#[test]
fn a_done_base_dependency_without_a_branch_fails_before_dispatch() {
    let world = World::new("stacked-missing-branch");
    world.repository("local-direct", &[]);
    world.script("H.work", "harness\n");
    let path = world.plan("missing", &plan_of("missing", vec![kept("H", &[])]));
    world.run(&["start", &path, "--attach"]).settled();
    // llmlint: ignore-block[tests_mirror_real_usage] The required done-without-a-branch
    // recovery state cannot be emitted by current preserve writers, which always
    // carry a branch. This models an older or damaged durable record at the real
    // adoption boundary; the compiled binary must refuse dispatch rather than
    // silently use main. No engine, session, or repository operation is mocked.
    let mut events = world.journal("missing");
    for event in &mut events {
        if event["labels"]["node"] == "H" {
            if let Some(payload) = event["payload"].as_object_mut() {
                payload.remove("branch");
            }
        }
    }
    let contents = events
        .iter()
        .map(|event| format!("{event}\n"))
        .collect::<String>();
    std::fs::write(world.run_file("missing", "events.jsonl"), contents).unwrap();
    let checkpoint = world.run_file("missing", "checkpoint.json");
    if checkpoint.exists() {
        std::fs::remove_file(checkpoint).unwrap();
    }
    // llmlint: ignore-end[tests_mirror_real_usage]
    reply(
        &world,
        "missing",
        json!([{"op":"add","node":kept("A", &["H"])}]),
    )
    .exited(0);
    world.run(&["adopt", "missing"]).settled();
    let result = world.run_json("missing", "result.json");
    let a = result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "A")
        .unwrap();
    assert_eq!(a["status"], "failed", "{result}");
    assert_eq!(a["outcome"], "infrastructure-failure", "{result}");
    let settled = world.events_of("missing", "node-settled");
    let a = settled
        .iter()
        .find(|event| event["labels"]["node"] == "A")
        .unwrap();
    assert!(a["payload"]["detail"].as_str().unwrap().contains("'H'"));
    assert!(!world
        .events_of("missing", "node-dispatched")
        .iter()
        .any(|event| event["labels"]["node"] == "A"));
    assert_eq!(
        world.events_of("missing", "session-opened").len(),
        2,
        "only H's engine and sibling session records may exist"
    );
}

#[test]
fn all_landing_spellings_are_refused_but_preserve_edges_load() {
    let world = World::new("stacked-load-boundaries");
    world.repository("local-direct", &[]);
    let accepted = world.plan(
        "accepted-edges",
        &plan_of("accepted-edges", vec![kept("H", &[]), kept("A", &["H"])]),
    );
    world.run(&["plan", "check", &accepted]).exited(0);
    for (name, publish, draft) in [
        ("omitted", None, false),
        ("land", Some("land"), false),
        ("draft", None, true),
    ] {
        let mut node = lifecycle("landing", &["H"]);
        if let Some(publish) = publish {
            node["publish"] = json!(publish);
        }
        if draft {
            node["draft"] = json!(true);
        }
        let plan = plan_of(name, vec![kept("H", &[]), node]);
        let refusal = refused_by_check(&world, name, &plan);
        assert!(refusal["reason"]
            .as_str()
            .unwrap()
            .contains("node 'landing' depends on 'H'"));
        let path = world.plan(&format!("start-{name}"), &plan);
        world
            .run(&["start", &path, "--detach"])
            .exited(REFUSED)
            .err_has("node 'landing' depends on 'H'");
    }
    assert!(world.invocations().is_empty());
}

#[test]
fn no_remote_kept_work_is_also_the_dependent_session_base() {
    let world = World::new("stacked-no-remote");
    let repo = world.repository("local-direct", &[]);
    git(&world, &repo.checkout, &["remote", "remove", "origin"]);
    world.script("H.work", "harness\n");
    world.script("A.requires-file", "H.md");
    world.script("A.work", "area measurement\n");
    let path = world.plan(
        "local-stack",
        &plan_of("local-stack", vec![kept("H", &[]), kept("A", &["H"])]),
    );
    world.run(&["start", &path, "--attach"]).settled();
    let result = world.run_json("local-stack", "result.json");
    assert_eq!(result["state"], "complete", "{result}\n{}", world.dump());
    let nodes = result["nodes"].as_array().unwrap();
    let h = nodes.iter().find(|node| node["id"] == "H").unwrap();
    let a = nodes.iter().find(|node| node["id"] == "A").unwrap();
    assert_eq!(h["remote"], "no-remote");
    assert_eq!(a["remote"], "no-remote");
    descends(
        &world,
        &repo.checkout,
        h["head"].as_str().unwrap(),
        a["branch"].as_str().unwrap(),
    );
}

fn budget_report(cwd: &Path, result: &Path) -> std::process::Output {
    std::process::Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/stacked-spikes-budget.mjs"))
        .current_dir(cwd)
        .env("ONEBUDGETSPEC_RESULT", result)
        .output()
        .expect("the committed reporter runs")
}

#[test]
fn the_budget_reporter_refuses_missing_and_malformed_records() {
    let world = World::new("stacked-budget-errors");
    let result = world.root.join("result.json");
    let record = world
        .root
        .join("target/budget-records/stacked-spikes-stage.json");
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    for body in [
        None,
        Some("{"),
        Some("{}"),
        Some(r#"{"wall_ms":true,"run_id":"run"}"#),
        Some(r#"{"wall_ms":-1,"run_id":"run"}"#),
        Some(r#"{"wall_ms":"1000","run_id":"run"}"#),
        Some(r#"{"wall_ms":1.5,"run_id":"run"}"#),
        Some(r#"{"wall_ms":18446744073709551616,"run_id":"run"}"#),
        Some(r#"{"wall_ms":9007199254740993,"run_id":"run"}"#),
        Some(r#"{"wall_ms":1000}"#),
        Some(r#"{"wall_ms":1000,"run_id":null}"#),
        Some(r#"{"wall_ms":1000,"run_id":""}"#),
        Some(r#"{"wall_ms":1000,"run_id":"   "}"#),
    ] {
        if let Some(body) = body {
            std::fs::write(&record, body).unwrap();
        }
        let output = budget_report(&world.root, &result);
        assert!(!output.status.success(), "{body:?} was accepted");
        assert!(!result.exists(), "a failed measurement reported a value");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("target/budget-records/stacked-spikes-stage.json")
                && stderr
                    .contains("just test-e2e 'test(publish_preserve::stacked_spikes_fan_out)'"),
            "{stderr}"
        );
    }
    std::fs::write(&record, r#"{"wall_ms":1000,"run_id":"run"}"#).unwrap();
    let missing_env = std::process::Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/stacked-spikes-budget.mjs"))
        .current_dir(&world.root)
        .env_remove("ONEBUDGETSPEC_RESULT")
        .output()
        .unwrap();
    assert!(!missing_env.status.success());
    assert!(
        String::from_utf8_lossy(&missing_env.stderr).contains("ONEBUDGETSPEC_RESULT is missing")
    );
    let cannot_write = budget_report(&world.root, &world.root);
    assert!(!cannot_write.status.success());
    let error = String::from_utf8_lossy(&cannot_write.stderr);
    assert!(
        error.contains("target/budget-records/stacked-spikes-stage.json")
            && error.contains("ONEBUDGETSPEC_RESULT names a writable file"),
        "{error}"
    );
}

#[test]
fn repository_aliases_stack_and_unreadable_unequal_spellings_are_refused() {
    let world = World::new("stacked-repo-aliases");
    let repo = world.repository("local-direct", &[]);
    world.script("H.work", "harness\n");
    world.script("A.work", "area measurement\n");
    world.script("A.requires-file", "H.md");
    let mut a = kept("A", &["H"]);
    a["repo"] = json!(repo.checkout.to_string_lossy());
    let path = world.plan("aliases", &plan_of("aliases", vec![kept("H", &[]), a]));
    world.run(&["start", &path, "--attach"]).settled();
    let result = world.run_json("aliases", "result.json");
    assert_eq!(result["state"], "complete", "{result}");
    let nodes = result["nodes"].as_array().unwrap();
    let h = nodes.iter().find(|node| node["id"] == "H").unwrap();
    let a = nodes.iter().find(|node| node["id"] == "A").unwrap();
    descends(
        &world,
        &repo.origin,
        h["head"].as_str().unwrap(),
        a["branch"].as_str().unwrap(),
    );
    let mut unknown = kept("unknown", &["H"]);
    unknown["repo"] = json!("unregistered");
    let refusal = refused_by_check(
        &world,
        "unreadable",
        &plan_of("unreadable", vec![kept("H", &[]), unknown.clone()]),
    );
    let reason = refusal["reason"].as_str().unwrap();
    assert!(
        reason.contains("service")
            && reason.contains("unregistered")
            && reason.contains("cannot both be read"),
        "{reason}"
    );
    reply(&world, "aliases", json!([{"op":"add","node":unknown}]))
        .exited(REFUSED)
        .err_has("service")
        .err_has("unregistered")
        .err_has("cannot both be read");
    let mut h = kept("H", &[]);
    h["repo"] = json!("unregistered");
    let mut a = kept("A", &["H"]);
    a["repo"] = json!("unregistered");
    let path = world.plan("same-unreadable", &plan_of("same-unreadable", vec![h, a]));
    world.run(&["plan", "check", &path]).exited(0);
    let mut conflicting = kept("A", &["H"]);
    conflicting["repo"] = json!("unregistered");
    conflicting["base_branch"] = json!("main");
    let mut h = kept("H", &[]);
    h["repo"] = json!("unregistered");
    let refusal = refused_by_check(
        &world,
        "unreadable-conflict",
        &plan_of("unreadable-conflict", vec![h, conflicting]),
    );
    assert!(
        refusal["reason"].as_str().unwrap().contains("two answers"),
        "{refusal}"
    );
}

#[test]
fn changed_repository_identities_fail_the_dependent_before_dispatch() {
    let world = World::new("stacked-repo-changed");
    world.repository("local-direct", &[]);
    let other = world.extra_repository("other");
    world.script("H.work", "harness\n");
    world.script("H.wait", "hold");
    let mut a = kept("A", &["H"]);
    a["repo"] = json!("other");
    let path = world.plan("changed", &plan_of("changed", vec![kept("H", &[]), a]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("H to hold its session", |world| {
        !world.events_of("changed", "node-dispatched").is_empty()
    });
    world.register(
        &other.checkout,
        Some("https://github.com/owner/service.git"),
    );
    world.release("H.go");
    world.until("the changed-identity run to settle", |world| {
        world.run_file("changed", "result.json").is_file()
    });
    let result = world.run_json("changed", "result.json");
    let a = result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "A")
        .unwrap();
    assert_eq!(
        a["outcome"],
        "infrastructure-failure",
        "{result}\n{}",
        world.dump()
    );
    let settled = world.events_of("changed", "node-settled");
    let a = settled
        .iter()
        .find(|event| event["labels"]["node"] == "A")
        .unwrap();
    assert!(
        a["payload"]["detail"]
            .as_str()
            .unwrap()
            .contains("base changed"),
        "{a}"
    );
    assert!(!world
        .events_of("changed", "node-dispatched")
        .iter()
        .any(|event| event["labels"]["node"] == "A"));
}

#[test]
fn a_cancelled_and_requeued_preserve_dependent_continues_its_branch_from_the_kept_base() {
    let world = World::new("stacked-requeue");
    let repo = world.repository("local-direct", &[]);
    world.script("H.work", "harness\n");
    world.script("A.wait", "hold");
    world.script("K.wait", "hold");
    world.script("K.work", "keep the driver active\n");
    let path = world.plan(
        "continued",
        &plan_of(
            "continued",
            vec![kept("H", &[]), kept("A", &["H"]), kept("K", &["H"])],
        ),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("A and K to hold their sessions", |world| {
        let opened = world.events_of("continued", "session-opened");
        ["A", "K"]
            .iter()
            .all(|id| opened.iter().any(|event| event["labels"]["node"] == *id))
    });
    let opened = world.events_of("continued", "session-opened");
    let a = opened
        .iter()
        .find(|event| event["labels"]["node"] == "A")
        .unwrap();
    let branch = a["payload"]["branch"].as_str().unwrap().to_owned();
    let settlements = world.events_of("continued", "node-settled");
    let h = &settlements
        .iter()
        .find(|event| event["labels"]["node"] == "H")
        .unwrap()["payload"];
    let base = h["branch"].as_str().unwrap().to_owned();
    let head = h["head"].as_str().unwrap().to_owned();
    reply(
        &world,
        "continued",
        json!([{"op":"cancel","id":"A","reason":"continue this session branch later"}]),
    )
    .exited(0);
    world.release("A.go");
    world.until("A's cancellation to settle", |world| {
        world
            .events_of("continued", "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "A")
    });
    std::fs::remove_file(world.fakes.join("A.go")).unwrap();
    reply(&world, "continued", json!([{"op":"requeue","id":"A"}])).exited(0);
    world.until("A to reopen its own branch", |world| {
        world
            .events_of("continued", "session-opened")
            .iter()
            .filter(|event| event["labels"]["node"] == "A" && event["payload"]["base"].is_string())
            .count()
            >= 3
    });
    for event in world
        .events_of("continued", "session-opened")
        .iter()
        .filter(|event| event["labels"]["node"] == "A")
    {
        assert_eq!(event["payload"]["branch"], branch, "{event}");
        assert_eq!(event["payload"]["base"], base, "{event}");
    }
    world.release("A.go");
    world.release("K.go");
    world.until("the continuation run to settle", |world| {
        world.run_file("continued", "result.json").is_file()
    });
    let result = world.run_json("continued", "result.json");
    let a = result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "A")
        .unwrap();
    assert_eq!(a["outcome"], "empty-branch", "{result}\n{}", world.dump());
    assert_eq!(a["branch"], branch);
    descends(&world, &repo.checkout, &head, &branch);
}

#[test]
fn an_unreadable_preserve_edge_fails_only_its_subtree_before_dispatch() {
    let world = World::new("stacked-unreadable-subtree")
        .with_env("ONEPIPELINE_WORKSPACE_POLL_SECONDS", "1");
    let repo = world.repository("local-direct", &[]);
    world.extra_repository("other");
    std::fs::write(world.onevcs_home().join("workspaces.yml"),
        "version: 1\nrules:\n  - match: {host: github.com, owner: owner, name: other}\n    pool: 1\n    overflow: 0\n").unwrap();
    world.script("K.wait", "hold");
    world.script("K.work", "occupy the other repository\n");
    world.script("H.work", "harness\n");
    world.script("B.requires-file", "H.md");
    world.script("B.work", "area measurement\n");
    let mut k = kept("K", &[]);
    k["repo"] = json!("other");
    let mut a = kept("A", &["H"]);
    a["repo"] = json!("other");
    let path = world.plan(
        "unreadable-subtree",
        &plan_of(
            "unreadable-subtree",
            vec![
                k,
                kept("H", &[]),
                a,
                human("gate", &[]),
                kept("B", &["H", "gate"]),
            ],
        ),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until(
        "A to wait on workspace capacity after its release lookup",
        |world| {
            world
                .events_of("unreadable-subtree", "node-held")
                .iter()
                .any(|event| {
                    event["labels"]["node"] == "A"
                        && event["payload"]["reasons"]
                            .as_array()
                            .is_some_and(|reasons| {
                                reasons.iter().any(|reason| reason["kind"] == "workspace")
                            })
                })
        },
    );
    world.releases("version: 999\n");
    world
        .run(&["attest", "unreadable-subtree", "gate"])
        .exited(0);
    world.until("A's refusal and B's preservation", |world| {
        let settled = world.events_of("unreadable-subtree", "node-settled");
        ["A", "B"]
            .iter()
            .all(|id| settled.iter().any(|event| event["labels"]["node"] == *id))
    });
    let settled = world.events_of("unreadable-subtree", "node-settled");
    let a = &settled
        .iter()
        .find(|event| event["labels"]["node"] == "A")
        .unwrap()["payload"];
    let b = &settled
        .iter()
        .find(|event| event["labels"]["node"] == "B")
        .unwrap()["payload"];
    let h = &settled
        .iter()
        .find(|event| event["labels"]["node"] == "H")
        .unwrap()["payload"];
    assert_eq!(a["outcome"], "infrastructure-failure", "{a}");
    assert!(
        a["detail"]
            .as_str()
            .unwrap()
            .contains("cannot both be read"),
        "{a}"
    );
    assert_eq!(b["outcome"], "preserved", "{b}\n{}", world.dump());
    descends(
        &world,
        &repo.origin,
        h["head"].as_str().unwrap(),
        b["branch"].as_str().unwrap(),
    );
    assert!(!world
        .events_of("unreadable-subtree", "node-dispatched")
        .iter()
        .any(|event| event["labels"]["node"] == "A"));
    std::fs::remove_file(world.onevcs_home().join("releases.yml")).unwrap();
    world.release("K.go");
    world.until("the unrelated worker to finish", |world| {
        world
            .run_file("unreadable-subtree", "result.json")
            .is_file()
    });
}

#[test]
fn the_worker_file_check_refuses_a_missing_worktree_argument_without_panicking() {
    let world = World::new("stacked-worker-bad-dir");
    world.write_graphs();
    world.script("A.requires-file", "H.md");
    let repo = world.repository("local-direct", &[]);
    world.script("A.work", "area measurement\n");
    let run_worker = |workspace: Option<&Path>| {
        let mut command = std::process::Command::new(crate::harness::double("fake-oneagentgraph"));
        command.args([
            "run",
            world.graphs().join("node-scope.yaml").to_str().unwrap(),
            "--task",
            "measure",
            "--output",
            "json",
            "--label",
            "onepipeline.node=A",
        ]);
        if let Some(workspace) = workspace {
            command.arg("--dir").arg(workspace);
        }
        command
            .env("ONEPIPELINE_FAKE_DIR", &world.fakes)
            .env("ONEPIPELINE_NODE_SCRATCH_DIR", &world.root)
            .output()
            .unwrap()
    };
    let output = run_worker(None);
    assert_eq!(
        output.status.code(),
        Some(78),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("requires --dir") && !error.contains("panicked"),
        "{error}"
    );
    let missing_file = run_worker(Some(&repo.checkout));
    assert_eq!(missing_file.status.code(), Some(78));
    assert!(String::from_utf8_lossy(&missing_file.stderr)
        .contains("dependency file H.md is missing before worker writes"));
    assert!(!repo.checkout.join("A.md").exists());
    std::fs::write(repo.checkout.join("H.md"), "shared harness\n").unwrap();
    let supplied_file = run_worker(Some(&repo.checkout));
    assert!(
        supplied_file.status.success(),
        "{}",
        String::from_utf8_lossy(&supplied_file.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(repo.checkout.join("A.md")).unwrap(),
        "area measurement"
    );
}
