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

    // The settlement: done, under its own word, naming where the work is.
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

    // The origin carries the branch at exactly that commit, with the work on it.
    assert_eq!(
        origin_tip(&repo.origin, &branch).as_deref(),
        Some(head.as_str()),
        "the origin does not carry {branch} at the head the settlement names"
    );
    assert_eq!(
        file_at(&world, &repo.origin, &head, "spike.md").trim(),
        "budget: 40"
    );
    // And the base did not move.
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

    // Every view says `preserved`, with the branch and the head.
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

    // The session closed, and the branch is on this host at the commit the
    // settlement names, carrying the work.
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
    if let Ok(pid) = std::fs::read_to_string(world.fakes.join(format!("{key}.lingering"))) {
        let _ = std::process::Command::new("kill").arg(pid.trim()).status();
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

    // A value that is neither word.
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
