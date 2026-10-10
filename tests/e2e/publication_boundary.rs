//! The repository publication boundary: a plan spanning a private and a public
//! repository, where private work reaches the public one only through an export edge.
//!
//! Every journey drives the compiled binary over a real plan store and the linked
//! `onevcs` against real origins on disk. Which repository is public is the operator's
//! statement — a version-4 rules file's `visibility:` — because the host's own answer is
//! a GitHub read nothing here may make; the private one is a synthetic `owner/service`,
//! the public one a synthetic `owner/library`.

// llmlint: ignore-file[e2e_not_mocked] the repository side is not substituted at all:
// `onevcs` is the library this crate links, driving real git against bare origins on disk,
// and `gh` stands in at that library's own `ONEVCS_GH` override only so that nothing here
// reaches GitHub. `oneagentgraph` is the one double, because what these journeys are about
// is the engine's scheduling, loading and closeout around a turn, and a real turn is paid.

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] each journey cuts real
// branches through the linked `onevcs` over real git, and what it exercises is `boundary`,
// `graph`, `taskgraph`, `edits`, `engine` and `lifecycle` together, which any change under
// `src/` can move, so a project edged narrower than the crate would drop them out of
// `nx affected` for the very changes they exist to catch.

use serde_json::{json, Value};

use crate::harness::{lifecycle, plan_of, Repository, World, REFUSED};

/// What `plan check` exits with when the plan carries refusals.
const HAS_REFUSALS: i32 = 1;

/// A world with two registered repositories: `service`, which the rules declare
/// private, and `library`, which they declare public.
pub(crate) fn mixed_world(name: &str) -> (World, Repository, Repository) {
    let world = World::new(name);
    let private = world.repository("local-direct", &[]);
    let public = world.extra_repository("library");
    declare_visibility(&world, &[("service", "private"), ("library", "public")]);
    (world, private, public)
}

/// Rewrite this host's rules file at version 4, declaring each named repository of
/// owner `owner` public or private, everything else publishing `local-direct`.
pub(crate) fn declare_visibility(world: &World, declared: &[(&str, &str)]) {
    let rules: String = declared
        .iter()
        .map(|(name, visibility)| {
            format!("  - match: {{owner: owner, name: {name}}}\n    visibility: {visibility}\n")
        })
        .collect();
    std::fs::write(
        world.onevcs_home().join("rules.yml"),
        format!(
            "version: 4\nrules:\n{rules}  - match: {{owner: owner}}\n    visibility: private\n\
             default:\n  publication: local-direct\n  approvals: none\n"
        ),
    )
    .expect("the rules file is written");
}

/// A private producer: a lifecycle node in `service` that keeps its branch.
pub(crate) fn producer(id: &str, deps: &[&str]) -> Value {
    let mut node = lifecycle(id, deps);
    node["publish"] = json!("preserve");
    node
}

/// A public lifecycle node in `library`.
pub(crate) fn public(id: &str, deps: &[&str]) -> Value {
    let mut node = lifecycle(id, deps);
    node["repo"] = json!("library");
    node
}

/// One export edge.
pub(crate) fn edge(from: &str, to: &str) -> Value {
    json!({
        "from": from,
        "to": to,
        "source_directory": "examples/generic",
        "target_directory": "examples",
    })
}

/// A plan of `nodes` carrying `edges`.
pub(crate) fn mixed_plan(name: &str, nodes: Vec<Value>, edges: Vec<Value>) -> Value {
    let mut plan = plan_of(name, nodes);
    if !edges.is_empty() {
        plan["export_edges"] = Value::Array(edges);
    }
    plan
}

/// The one engine refusal `plan check --json` reports for `plan`.
fn refused_by_check(world: &World, name: &str, plan: &Value) -> String {
    let project = world.plan(name, plan);
    let checked = world.run(&["plan", "check", &project, "--json"]);
    checked.exited(HAS_REFUSALS);
    let answered: Value = serde_json::from_str(checked.stdout.trim()).expect("one JSON object");
    let refusals: Vec<&Value> = answered["refusals"]
        .as_array()
        .expect("a refusals list")
        .iter()
        .filter(|refusal| refusal["source"] == json!("engine"))
        .collect();
    assert_eq!(refusals.len(), 1, "{answered}");
    refusals[0]["reason"].as_str().expect("a reason").to_owned()
}

/// The guidance summary, as the host reads it from `onepipeline boundary guidance`.
fn guidance(world: &World) -> String {
    let printed = world.run(&["boundary", "guidance", "--json"]);
    printed.exited(0);
    let guidance: Value = serde_json::from_str(printed.stdout.trim()).expect("one JSON object");
    let steps: Vec<&str> = guidance["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| step["step"].as_str().expect("a step name"))
        .collect();
    assert_eq!(steps, ["generalize", "preserve", "export", "consume"]);
    guidance["summary"].as_str().expect("a summary").to_owned()
}

#[test]
fn an_ordinary_private_to_public_dependency_is_refused_at_load_carrying_the_hosts_guidance() {
    let (world, _, _) = mixed_world("boundary-load");
    let advice = guidance(&world);

    let reason = refused_by_check(
        &world,
        "ordinary",
        &mixed_plan(
            "ordinary",
            vec![lifecycle("inner", &[]), public("outer", &["inner"])],
            vec![],
        ),
    );
    assert!(reason.contains("ordinary dependency"), "{reason}");
    assert!(
        reason.contains(&advice),
        "the refusal does not carry the guidance the host renders:\n{reason}"
    );

    // The same refusal stops a launch before anything is dispatched.
    let project = world.plan(
        "ordinary-start",
        &mixed_plan(
            "ordinary-start",
            vec![lifecycle("inner", &[]), public("outer", &["inner"])],
            vec![],
        ),
    );
    world
        .run(&["start", &project, "--attach"])
        .exited(REFUSED)
        .err_has("ordinary dependency");
    assert!(
        !world
            .runs
            .join("ordinary-start")
            .join("events.jsonl")
            .is_file(),
        "a refused plan started a run"
    );

    // Single-visibility plans of the same shape load exactly as they always did.
    for (name, repo) in [("all-private", "service"), ("all-public", "library")] {
        let mut first = lifecycle("first", &[]);
        first["repo"] = json!(repo);
        let mut second = lifecycle("second", &["first"]);
        second["repo"] = json!(repo);
        let project = world.plan(name, &mixed_plan(name, vec![first, second], vec![]));
        world.run(&["plan", "check", &project]).exited(0);
    }
}

#[test]
fn every_malformed_export_edge_and_alternate_private_context_is_refused_at_load() {
    let (world, _, _) = mixed_world("boundary-edges");
    let advice = guidance(&world);
    let consumer = || public("examples", &["generalize"]);
    let mut pinned = consumer();
    pinned["base_branch"] = json!("topic");
    let cases: Vec<(&str, Vec<Value>, Vec<Value>, &str)> = vec![
        (
            "dangling",
            vec![producer("generalize", &[]), consumer()],
            vec![edge("generalize", "elsewhere")],
            "dangling",
        ),
        (
            "duplicate",
            vec![producer("generalize", &[]), consumer()],
            vec![
                edge("generalize", "examples"),
                edge("generalize", "examples"),
            ],
            "declared twice",
        ),
        (
            "ambiguous",
            vec![
                producer("generalize", &[]),
                producer("second", &[]),
                public("examples", &["generalize", "second"]),
            ],
            vec![edge("generalize", "examples"), edge("second", "examples")],
            "ambiguous",
        ),
        (
            "disagreeing",
            vec![producer("generalize", &[]), public("examples", &[])],
            vec![edge("generalize", "examples")],
            "disagree",
        ),
        (
            "landing-producer",
            vec![lifecycle("generalize", &[]), consumer()],
            vec![edge("generalize", "examples")],
            "export producer must be",
        ),
        (
            "private-consumer",
            vec![
                producer("generalize", &[]),
                lifecycle("examples", &["generalize"]),
            ],
            vec![edge("generalize", "examples")],
            "export consumer must be a public",
        ),
        (
            "alternate-dependency",
            vec![
                producer("generalize", &[]),
                lifecycle("other", &[]),
                public("examples", &["generalize", "other"]),
            ],
            vec![edge("generalize", "examples")],
            "alternate private context",
        ),
        (
            "alternate-base",
            vec![producer("generalize", &[]), pinned],
            vec![edge("generalize", "examples")],
            "alternate private context",
        ),
        (
            "outside-directory",
            vec![producer("generalize", &[]), consumer()],
            vec![json!({
                "from": "generalize",
                "to": "examples",
                "source_directory": "../elsewhere",
                "target_directory": "examples",
            })],
            "normalized",
        ),
    ];
    for (name, nodes, edges, expected) in cases {
        let reason = refused_by_check(&world, name, &mixed_plan(name, nodes, edges));
        assert!(reason.contains(expected), "{name}: {reason}");
        assert!(reason.contains(&advice), "{name}: {reason}");
    }

    // And a consumer whose public fields carry a private repository's name is refused
    // by the term check, over the run's own scope.
    let mut named = consumer();
    named["title"] = json!("feat: examples drawn from owner/service");
    let reason = refused_by_check(
        &world,
        "named",
        &mixed_plan(
            "named",
            vec![producer("generalize", &[]), named],
            vec![edge("generalize", "examples")],
        ),
    );
    assert!(reason.contains("must be neutral"), "{reason}");
    assert!(
        !reason.contains("owner/service"),
        "a refusal repeated the private name it found:\n{reason}"
    );

    // The well-formed edge loads.
    let project = world.plan(
        "well-formed",
        &mixed_plan(
            "well-formed",
            vec![producer("generalize", &[]), consumer()],
            vec![edge("generalize", "examples")],
        ),
    );
    world.run(&["plan", "check", &project]).exited(0);
}

#[test]
fn a_live_edit_adding_an_ordinary_private_to_public_dependency_is_refused_before_it_commits() {
    let (world, _, _) = mixed_world("boundary-edit");
    world.script("inner.work", "private work\n");
    world.script("inner.wait", "hold");
    let project = world.plan(
        "edited",
        &mixed_plan("edited", vec![lifecycle("inner", &[])], vec![]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the private node to be in flight", |world| {
        !world.events_of("edited", "node-dispatched").is_empty()
    });

    let ordinary = json!({"version": 2, "commands": [
        {"op": "add", "node": public("outer", &["inner"])},
    ]})
    .to_string();
    world
        .run_with_stdin(&["reply", "edited"], &ordinary)
        .exited(REFUSED)
        .err_has("ordinary dependency")
        .err_has("generalize -> preserve -> export -> consume");

    // A node naming no repository inherits the private side, so routing the same
    // dependency through one is refused too.
    let relayed = json!({"version": 2, "commands": [
        {"op": "add", "node": crate::harness::agent("relay", &["inner"])},
        {"op": "add", "node": public("outer", &["relay"])},
    ]})
    .to_string();
    world
        .run_with_stdin(&["reply", "edited"], &relayed)
        .exited(REFUSED)
        .err_has("private node relay");
    assert!(
        world.events_of("edited", "edit-committed").is_empty(),
        "a refused edit was committed"
    );

    // A public node with no private dependency is an ordinary edit.
    let unrelated = json!({"version": 2, "commands": [
        {"op": "add", "node": public("outer", &[])},
    ]})
    .to_string();
    let applied = world.run_with_stdin(&["reply", "edited"], &unrelated);
    assert!(
        [0, onepipeline::error::EXIT_QUEUED].contains(&applied.code),
        "an ordinary edit was not taken: {}",
        applied.stderr
    );
    world.release("inner.go");
    world.until("the run to settle", |world| {
        world.run_file("edited", "result.json").is_file()
    });
}

/// The task prose every dispatch of a node was handed, read off the `--task` the
/// sibling's own launch carried — what its worker and its judge both read.
pub(crate) fn tasks_of(world: &World, node: &str) -> Vec<String> {
    let mine = format!("## What\nShip {node}.");
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .filter_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|arg| arg == "--task")?;
            args.get(at + 1)?.as_str().map(str::to_owned)
        })
        .filter(|task| task.starts_with(&mine))
        .collect()
}

/// `git` in `dir`, answering its stdout trimmed.
pub(crate) fn git_out(world: &World, dir: &std::path::Path, args: &[&str]) -> String {
    crate::harness::git(world, dir, args).trim().to_owned()
}

/// One settled node of a run's `result.json`.
pub(crate) fn settled(world: &World, run: &str, node: &str) -> Value {
    let result = world.run_json(run, "result.json");
    result["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|settled| settled["id"] == node)
        .cloned()
        .unwrap_or_else(|| panic!("no node {node} in {result}\n{}", world.dump()))
}

/// A valid mixed plan: `generalize` keeps a generic example under `examples/generic`
/// in the private repository, and the public `examples` builds on its export.
pub(crate) fn exported_plan(name: &str) -> Value {
    mixed_plan(
        name,
        vec![
            producer("generalize", &[]),
            public("examples", &["generalize"]),
        ],
        vec![edge("generalize", "examples")],
    )
}

#[test]
fn a_public_consumer_starts_from_one_neutral_export_commit_and_the_edge_adds_no_dispatch() {
    let (world, private, library) = mixed_world("boundary-export");
    world.script("generalize.work-at", "examples/generic/sample.md");
    world.script("generalize.work", "a generic example\n");
    world.script("examples.work", "what the public node built on it\n");
    let project = world.plan("exported", &exported_plan("exported"));
    world.run(&["start", &project, "--attach"]).settled();

    let producer = settled(&world, "exported", "generalize");
    assert_eq!(
        producer["outcome"],
        "preserved",
        "{producer}\n{}",
        world.dump()
    );
    let consumer = settled(&world, "exported", "examples");
    assert_eq!(consumer["status"], "done", "{consumer}\n{}", world.dump());

    // Each node is dispatched once, and nothing else is: keeping the producer's work,
    // exporting it and opening the consumer's session spend no model turn.
    let dispatched: Vec<String> = world
        .events_of("exported", "node-dispatched")
        .iter()
        .map(|event| {
            event["labels"]["node"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(dispatched, ["generalize", "examples"], "{}", world.dump());
    let runs = world
        .invocations()
        .iter()
        .filter(|call| call["tool"] == "oneagentgraph" && call["args"][0] == "run")
        .count();
    assert_eq!(runs, 2, "{:?}", world.invocations());

    // Both branches are neutral: neither names the plan or a node.
    let kept = producer["branch"].as_str().expect("a kept branch");
    assert!(
        kept.starts_with("export/") && kept.len() == "export/".len() + 12,
        "{kept}"
    );
    let landed_on = consumer["branch"].as_str().unwrap_or_default().to_owned();
    let segments: Vec<&str> = landed_on.split('/').collect();
    assert_eq!(segments.len(), 3, "{landed_on}");
    assert_eq!(segments[0], "public");
    assert_eq!(segments[1].len(), 12, "{landed_on}");
    assert_eq!(segments[2], "task-2", "{landed_on}");

    // The export is one commit on the public base, under one neutral subject and
    // author, on the branch the consumer's session continued — and what landed on the
    // public base carries the exported file, which the default base never had.
    git_out(&world, &library.checkout, &["fetch", "origin"]);
    // Read off the run's private record of the export, which is the one place that
    // names where it came from as well as what it made.
    let recorded = world.run_json("exported", "exports.json");
    let recorded = recorded.as_array().expect("a list of exports");
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0]["producer"], "generalize");
    assert_eq!(recorded[0]["exported"], consumer["branch"]);
    let export_commit = recorded[0]["exported_head"]
        .as_str()
        .expect("the export's head")
        .to_owned();
    let shown = git_out(
        &world,
        &library.checkout,
        &[
            "show",
            "--no-patch",
            "--format=%an <%ae>|%cn <%ce>|%s|%P",
            &export_commit,
        ],
    );
    let parents = shown.rsplit('|').next().unwrap_or_default();
    assert_eq!(parents.split_whitespace().count(), 1, "{shown}");
    assert!(
        shown.starts_with(
            "Example Export <export@example.invalid>|Example Export <export@example.invalid>|Add generic example fixtures|"
        ),
        "{shown}"
    );
    let base_then = git_out(
        &world,
        &library.checkout,
        &["log", "--format=%H", "-1", "origin/main~1"],
    );
    assert_eq!(parents, base_then, "the export is not on the public base");
    let exported = git_out(
        &world,
        &library.checkout,
        &["show", "origin/main:examples/sample.md"],
    );
    assert_eq!(exported, "a generic example");
    let private_head = producer["head"].as_str().expect("a kept head");
    let reachable = std::process::Command::new("git")
        .args(["cat-file", "-e", &format!("{private_head}^{{commit}}")])
        .current_dir(&library.origin)
        .status()
        .expect("git runs");
    assert!(
        !reachable.success(),
        "the producer's commit reached the public repository"
    );
    let whole = git_out(
        &world,
        &library.checkout,
        &["log", "--all", "--format=%B%an%ae"],
    );
    for private_name in ["service", "generalize", "exported", kept] {
        assert!(
            !whole.contains(private_name),
            "the public history names {private_name}:\n{whole}"
        );
    }
    drop(private);

    // The template context, mixed plans only: each side is told which it is on, the
    // producer where to write and where it lands, the consumer where its base put it.
    let producer_task = tasks_of(&world, "generalize")
        .pop()
        .expect("a producer dispatch");
    assert!(
        producer_task.contains(
            r#"{"boundary":{"mixed":true,"clearance":"private","export_directory":"examples/generic","target_directory":"examples"}}"#
        ),
        "{producer_task}"
    );
    assert!(producer_task.contains("commit subject"), "{producer_task}");
    let consumer_task = tasks_of(&world, "examples")
        .pop()
        .expect("a consumer dispatch");
    assert!(
        consumer_task.contains(
            r#"{"boundary":{"mixed":true,"clearance":"public","target_directory":"examples"}}"#
        ),
        "{consumer_task}"
    );
    for private_name in ["service", "generalize", "exported", kept, private_head] {
        assert!(
            !consumer_task.contains(private_name),
            "the public task names {private_name}:\n{consumer_task}"
        );
    }
    assert!(
        !consumer_task.contains("## Cross-repository references"),
        "{consumer_task}"
    );
}

#[test]
fn a_single_visibility_plan_is_dispatched_without_any_boundary_context() {
    for (name, repo) in [("only-private", "service"), ("only-public", "library")] {
        let (world, _, _) = mixed_world(&format!("boundary-{name}"));
        let mut first = lifecycle("first", &[]);
        first["repo"] = json!(repo);
        let mut second = lifecycle("second", &["first"]);
        second["repo"] = json!(repo);
        world.script("first.work", "one\n");
        world.script("second.work", "two\n");
        let project = world.plan(name, &mixed_plan(name, vec![first, second], vec![]));
        world.run(&["start", &project, "--attach"]).settled();
        for node in ["first", "second"] {
            let tasks = tasks_of(&world, node);
            assert!(!tasks.is_empty(), "{name}: {node} was not dispatched");
            for task in tasks {
                assert!(
                    !task.contains("Repository boundary") && !task.contains("\"boundary\""),
                    "{name}: a single-visibility dispatch carries boundary context:\n{task}"
                );
                assert!(
                    task.ends_with(&format!("- {node} is published.")),
                    "{name}: the task is not composed as it always was:\n{task}"
                );
            }
        }
    }
}

/// Reply to a run with `commands`, taken whether the driver applied or queued them.
pub(crate) fn reply(world: &World, run: &str, commands: Value) {
    let replied = world.run_with_stdin(
        &["reply", run],
        &json!({"version": 3, "commands": commands}).to_string(),
    );
    assert!(
        [0, onepipeline::error::EXIT_QUEUED].contains(&replied.code),
        "the reply was not taken: {}\n{}",
        replied.stderr,
        world.dump()
    );
}

/// Wait until `node` has a settlement in `run`'s journal.
fn until_settled(world: &World, run: &str, node: &str) {
    world.until(&format!("{node} to settle"), |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == node)
    });
}

#[test]
fn the_export_base_holds_through_supersession_retry_and_resume() {
    let (world, _, library) = mixed_world("boundary-lineage");
    // The first producer fails its task; its replacement keeps the example.
    world.script("generalize.fail", "1");
    world.script("generalize-2.work-at", "examples/generic/sample.md");
    world.script("generalize-2.work", "the replacement's generic example\n");
    // The first consumer fails its task; its replacement is cancelled while it holds
    // its session, requeued, and then finishes.
    world.script("examples.fail", "1");
    world.script("examples-2.wait", "hold");
    world.script("examples-2.work", "built on the export\n");
    // A node held open for the whole journey keeps the driver attending the run while
    // each failure waits for its retry.
    world.script("keep.wait", "hold");
    let mut plan = exported_plan("lineage");
    plan["tasks"]
        .as_array_mut()
        .expect("the plan's nodes")
        .push(crate::harness::agent("keep", &[]));
    let project = world.plan("lineage", &plan);
    world.run(&["start", &project, "--detach"]).exited(0);

    until_settled(&world, "lineage", "generalize");
    reply(
        &world,
        "lineage",
        json!([{"op": "retry", "id": "generalize", "node": producer("generalize-2", &[])}]),
    );
    until_settled(&world, "lineage", "examples");
    reply(
        &world,
        "lineage",
        json!([{"op": "retry", "id": "examples", "node": public("examples-2", &[])}]),
    );
    world.until("the replacement consumer to hold its session", |world| {
        world
            .events_of("lineage", "session-opened")
            .iter()
            .any(|event| event["labels"]["node"] == "examples-2")
    });
    reply(
        &world,
        "lineage",
        json!([{"op": "cancel", "id": "examples-2", "reason": "resume it later"}]),
    );
    world.release("examples-2.go");
    until_settled(&world, "lineage", "examples-2");
    std::fs::remove_file(world.fakes.join("examples-2.go")).expect("the release is taken back");
    reply(
        &world,
        "lineage",
        json!([{"op": "requeue", "id": "examples-2"}]),
    );
    world.until("the resumed consumer to reopen its session", |world| {
        world
            .events_of("lineage", "session-opened")
            .iter()
            .filter(|event| event["labels"]["node"] == "examples-2")
            .count()
            >= 2
    });
    world.release("examples-2.go");
    until_settled(&world, "lineage", "examples-2");
    world.release("keep.go");
    world.until("the run to settle", |world| {
        world.run_file("lineage", "result.json").is_file()
    });
    let consumer = settled(&world, "lineage", "examples-2");
    assert_eq!(consumer["status"], "done", "{consumer}\n{}", world.dump());

    // One export, of the replacement producer's kept work — the failed producer kept
    // nothing to export — and every session of every consumer attempt opened on it.
    let recorded = world.run_json("lineage", "exports.json");
    let recorded = recorded.as_array().expect("a list of exports");
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0]["producer"], "generalize-2");
    let exported = recorded[0]["exported"]
        .as_str()
        .expect("a branch")
        .to_owned();
    let opened: Vec<Value> = world
        .events_of("lineage", "session-opened")
        .into_iter()
        .filter(|event| {
            matches!(
                event["labels"]["node"].as_str(),
                Some("examples" | "examples-2")
            )
        })
        .collect();
    assert!(opened.len() >= 3, "{opened:?}");
    for event in &opened {
        assert_eq!(event["payload"]["branch"], exported, "{event}");
    }

    // What landed carries the replacement producer's example, so the consumer built on
    // the export and never on the default base alone.
    git_out(&world, &library.checkout, &["fetch", "origin"]);
    assert_eq!(
        git_out(
            &world,
            &library.checkout,
            &["show", "origin/main:examples/sample.md"]
        ),
        "the replacement's generic example"
    );
}

#[test]
fn an_export_that_changes_a_path_outside_its_directory_is_refused_and_its_consumer_never_starts() {
    let (world, _, library) = mixed_world("boundary-outside");
    // The producer writes outside the edge's source directory.
    world.script("generalize.work-at", "elsewhere/notes.md");
    world.script("generalize.work", "not a generic example\n");
    world.script("examples.work", "never written\n");
    let project = world.plan("outside", &exported_plan("outside"));
    world.run(&["start", &project, "--attach"]).settled();

    assert_eq!(
        settled(&world, "outside", "generalize")["outcome"],
        "preserved"
    );
    let consumer = settled(&world, "outside", "examples");
    assert_eq!(consumer["status"], "failed", "{consumer}\n{}", world.dump());
    assert_eq!(consumer["outcome"], "infrastructure-failure");
    let detail = world
        .events_of("outside", "node-settled")
        .into_iter()
        .find(|event| event["labels"]["node"] == "examples")
        .map(|event| {
            event["payload"]["detail"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default();
    assert!(detail.contains("outside the export directory"), "{detail}");
    for private_name in ["service", "generalize", "elsewhere"] {
        assert!(!detail.contains(private_name), "{detail}");
    }
    // Nothing was dispatched for the consumer, and nothing reached the public repository.
    assert!(tasks_of(&world, "examples").is_empty());
    assert!(!world.run_file("outside", "exports.json").is_file());
    let branches = git_out(
        &world,
        &library.checkout,
        &["branch", "--list", "*public/*"],
    );
    assert!(
        branches.is_empty(),
        "a partial export branch was left: {branches}"
    );
}

/// Commit a `private-terms.toml` declaring `terms` at the root of `repository`, and push
/// it, so its committed declaration is what a term check reads.
pub(crate) fn declare_terms(world: &World, repository: &Repository, terms: &[&str]) {
    let listed: Vec<String> = terms.iter().map(|term| format!("{term:?}")).collect();
    std::fs::write(
        repository.checkout.join("private-terms.toml"),
        format!("schema_version = 1\nterms = [{}]\n", listed.join(", ")),
    )
    .expect("the declaration is written");
    crate::harness::git(world, &repository.checkout, &["add", "-A"]);
    crate::harness::git(
        world,
        &repository.checkout,
        &["commit", "-m", "chore: declare private terms"],
    );
    crate::harness::git(world, &repository.checkout, &["push", "origin", "main"]);
}

#[test]
fn a_producer_named_with_a_private_term_is_given_a_neutral_branch_and_exports_only_clean_metadata()
{
    let (world, private, library) = mixed_world("boundary-terms");
    // A synthetic term of the private repository, which its plan and node names carry.
    declare_terms(&world, &private, &["zephyrine"]);
    world.script("zephyrine-generalize.work-at", "examples/generic/sample.md");
    world.script("zephyrine-generalize.work", "a generic example\n");
    world.script("examples.work", "built on it\n");
    let clean = mixed_plan(
        "zephyrine-clean",
        vec![
            producer("zephyrine-generalize", &[]),
            public("examples", &["zephyrine-generalize"]),
        ],
        vec![edge("zephyrine-generalize", "examples")],
    );
    let project = world.plan("zephyrine-clean", &clean);
    world.run(&["start", &project, "--attach"]).settled();

    // The producer cut a neutral branch, so the commit its session's close left — which
    // names that branch — is clean, and the export went through.
    let producer_node = settled(&world, "zephyrine-clean", "zephyrine-generalize");
    let kept = producer_node["branch"]
        .as_str()
        .expect("a kept branch")
        .to_owned();
    assert!(
        kept.starts_with("export/") && !kept.contains("zephyrine"),
        "{kept}"
    );
    let producer_task = tasks_of(&world, "zephyrine-generalize")
        .pop()
        .expect("a producer dispatch");
    assert!(
        producer_task.contains("`<host prefix>/export/<opaque id>`"),
        "{producer_task}"
    );
    assert!(producer_task.contains("commit subject"), "{producer_task}");
    let consumer = settled(&world, "zephyrine-clean", "examples");
    assert_eq!(consumer["status"], "done", "{consumer}\n{}", world.dump());
    let consumer_task = tasks_of(&world, "examples")
        .pop()
        .expect("a consumer dispatch");
    assert!(!consumer_task.contains("zephyrine"), "{consumer_task}");
    git_out(&world, &library.checkout, &["fetch", "origin"]);
    let public_history = git_out(&world, &library.checkout, &["log", "--all", "--format=%B"]);
    assert!(!public_history.contains("zephyrine"), "{public_history}");

    // A producer whose own commit subject carries the term is refused at the export, and
    // its consumer never starts: the export's check is not weakened for what the run
    // itself named.
    world.script("zephyrine-dirty.work-at", "examples/generic/other.md");
    world.script("zephyrine-dirty.work", "another generic example\n");
    world.script(
        "zephyrine-dirty.commit-saying",
        "feat: add the zephyrine example",
    );
    let dirty = mixed_plan(
        "zephyrine-dirty",
        vec![
            producer("zephyrine-dirty", &[]),
            public("more-examples", &["zephyrine-dirty"]),
        ],
        vec![edge("zephyrine-dirty", "more-examples")],
    );
    let project = world.plan("zephyrine-dirty", &dirty);
    world.run(&["start", &project, "--attach"]).settled();
    assert_eq!(
        settled(&world, "zephyrine-dirty", "zephyrine-dirty")["outcome"],
        "preserved"
    );
    let refused = settled(&world, "zephyrine-dirty", "more-examples");
    assert_eq!(refused["outcome"], "infrastructure-failure", "{refused}");
    let detail = world
        .events_of("zephyrine-dirty", "node-settled")
        .into_iter()
        .find(|event| event["labels"]["node"] == "more-examples")
        .map(|event| {
            event["payload"]["detail"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default();
    assert!(
        detail.contains("public output carries a term of a private repository in a commit message"),
        "{detail}"
    );
    assert!(
        !detail.contains("zephyrine"),
        "the refusal named the term: {detail}"
    );
    assert!(tasks_of(&world, "more-examples").is_empty());
}

/// Judge `document` with the compiled validator, as a bus's `command` validator runs it.
fn validated(world: &World, run: &str, document: &Value) -> crate::harness::Run {
    world.run_with_stdin(
        &["boundary", "validate-envelope", "--run", run],
        &document.to_string(),
    )
}

/// One note command to `id`.
fn note(id: &str, text: &str) -> Value {
    json!({"op": "note", "id": id, "addressee": "worker", "text": text, "deliver": "next"})
}

/// A mixed run held open with its producer in flight, its private repository declaring
/// `terms`.
fn held_mixed_run(name: &str, terms: &[&str]) -> (World, Repository) {
    let (world, private, _) = mixed_world(&format!("boundary-{name}"));
    declare_terms(&world, &private, terms);
    world.script("generalize.wait", "hold");
    let project = world.plan(name, &exported_plan(name));
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the producer to be in flight", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });
    (world, private)
}

#[test]
fn the_compiled_validator_judges_targeted_and_untargeted_content_and_refuses_what_it_cannot_judge()
{
    let (world, private) = held_mixed_run("validated", &["zephyrine", "lanternfish"]);

    // A note to the public consumer carrying a private term is refused, in words that
    // do not repeat it; the same note without it passes.
    let refused = validated(
        &world,
        "validated",
        &json!({"version": 3, "commands": [note("examples", "follow the zephyrine layout")]}),
    );
    refused.exited(1).err_has("public node examples");
    assert!(!refused.stderr.contains("zephyrine"), "{}", refused.stderr);
    validated(
        &world,
        "validated",
        &json!({"version": 3, "commands": [note("examples", "follow the plain layout")]}),
    )
    .exited(0);
    // The private producer may be told anything.
    validated(
        &world,
        "validated",
        &json!({"version": 3, "commands": [note("generalize", "follow the zephyrine layout")]}),
    )
    .exited(0);
    // Text addressed to no node — a verdict's message, a surface — is checked as public,
    // and so is a node an envelope adds and a ruling it amends a public node with.
    validated(
        &world,
        "validated",
        &json!({"completion": false, "message": "keep the zephyrine names out"}),
    )
    .exited(1);
    validated(
        &world,
        "validated",
        &json!({"id": "s-1", "kind": "check-in", "message": "the zephyrine step is slow"}),
    )
    .exited(1);
    let mut added = public("more-examples", &[]);
    added["title"] = json!("feat: zephyrine examples");
    validated(
        &world,
        "validated",
        &json!({"version": 3, "commands": [{"op": "add", "node": added}]}),
    )
    .exited(1);
    validated(
        &world,
        "validated",
        &json!({"version": 3, "commands": [
            {"op": "amend", "id": "examples", "text": "match the zephyrine output"}
        ]}),
    )
    .exited(1);
    // A command naming a node the run does not hold has no clearance to judge it by.
    validated(
        &world,
        "validated",
        &json!({"version": 3, "commands": [note("nowhere", "anything at all")]}),
    )
    .exited(2)
    .err_has("no clearance");

    // An exception the private repository declares is honoured.
    let excepted = json!({"version": 3, "commands": [note("examples", "mind the lanternfish")]});
    validated(&world, "validated", &excepted).exited(1);
    std::fs::write(
        private.checkout.join("private-terms.toml"),
        "schema_version = 1\nterms = [\"zephyrine\", \"lanternfish\"]\n\
         exceptions = [{term = \"lanternfish\", action = \"drop\"}]\n",
    )
    .expect("the declaration is rewritten");
    crate::harness::git(
        &world,
        &private.checkout,
        &["commit", "-qam", "chore: except a word"],
    );
    validated(&world, "validated", &excepted).exited(0);

    // A declaration the check cannot read makes it unavailable, never a pass.
    std::fs::write(
        private.checkout.join("private-terms.toml"),
        "schema_version = 1\nterms = [\n",
    )
    .expect("the declaration is broken");
    crate::harness::git(
        &world,
        &private.checkout,
        &["commit", "-qam", "chore: break it"],
    );
    validated(
        &world,
        "validated",
        &json!({"version": 3, "commands": [note("examples", "follow the plain layout")]}),
    )
    .exited(2)
    .err_has("could not judge");
    world.release("generalize.go");
}

#[test]
fn a_payload_once_passed_is_judged_again_after_the_terms_change_whatever_a_bus_cached() {
    let (world, private, _) = mixed_world("boundary-revalidated");
    declare_terms(&world, &private, &["zephyrine"]);
    world.script("generalize.wait", "hold");
    // The bus this run launched under runs the compiled validator on its reply queue,
    // keeping passes in a cache keyed by the validator's own fingerprint.
    let validator = crate::harness::binary().to_string_lossy().into_owned();
    let cache = world.root.join("validator-passes");
    let config = world.root.join("onemessagebus.yaml");
    std::fs::write(
        &config,
        format!(
            "version: 1\ntransport: {{kind: local}}\nvalidators:\n  - on: replies\n    kind: \
             command\n    command: [{validator:?}, boundary, validate-envelope, --run, \
             bussed]\n    cache:\n      dir: {cache:?}\n      bar_fingerprint: [{validator:?}, \
             boundary, validate-envelope, --run, bussed, --fingerprint]\n"
        ),
    )
    .expect("the bus configuration is written");
    world.script("held.wait", "hold");
    let project = world.plan(
        "bussed",
        &mixed_plan(
            "bussed",
            vec![
                crate::harness::agent("held", &[]),
                producer("generalize", &[]),
                public("examples", &["generalize"]),
            ],
            vec![edge("generalize", "examples")],
        ),
    );
    world
        .run(&[
            "start",
            &project,
            "--bus-config",
            &config.to_string_lossy(),
            "--detach",
        ])
        .exited(0);
    world.until("the held node to be in flight", |world| {
        world
            .events_of("bussed", "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "held")
    });

    let payload =
        json!({"version": 3, "commands": [note("examples", "reuse the quillwort fixtures")]});
    let before = world
        .run(&[
            "boundary",
            "validate-envelope",
            "--run",
            "bussed",
            "--fingerprint",
        ])
        .stdout
        .trim()
        .to_owned();
    let replied = world.run_with_stdin(&["reply", "bussed"], &payload.to_string());
    assert!(
        [0, onepipeline::error::EXIT_QUEUED].contains(&replied.code),
        "{}",
        replied.stderr
    );
    let cached = std::fs::read_dir(&cache)
        .map(|dir| dir.count())
        .unwrap_or(0);
    assert!(
        cached > 0,
        "the bus kept no pass, so it never ran the validator"
    );

    // The private repository now declares the word the payload carries.
    declare_terms(&world, &private, &["zephyrine", "quillwort"]);
    let after = world
        .run(&[
            "boundary",
            "validate-envelope",
            "--run",
            "bussed",
            "--fingerprint",
        ])
        .stdout
        .trim()
        .to_owned();
    assert_ne!(before, after, "the fingerprint did not move with the terms");
    // The same bytes are judged again, through the bus and directly, and refused.
    world
        .run_with_stdin(&["reply", "bussed"], &payload.to_string())
        .exited(REFUSED)
        .err_has("public node examples");
    validated(&world, "bussed", &payload).exited(1);
    let committed: Vec<Value> = world.events_of("bussed", "edit-committed");
    let notes = committed
        .iter()
        .filter(|event| event["payload"]["command"]["op"] == "note")
        .count();
    assert!(notes <= 1, "the refused note was committed: {committed:?}");
    world.release("held.go");
    world.release("generalize.go");
}

#[test]
fn a_direct_reply_and_note_meet_the_same_check_without_any_bus() {
    let (world, _) = held_mixed_run("direct", &["zephyrine"]);
    let leaking =
        json!({"version": 3, "commands": [note("examples", "follow the zephyrine layout")]});
    world
        .run_with_stdin(&["reply", "direct"], &leaking.to_string())
        .exited(REFUSED)
        .err_has("public node examples");
    let unknown = json!({"version": 3, "commands": [note("nowhere", "anything")]});
    world
        .run_with_stdin(&["reply", "direct"], &unknown.to_string())
        .exited(REFUSED);
    assert!(
        world
            .events_of("direct", "edit-committed")
            .iter()
            .all(|event| event["payload"]["command"]["op"] != "note"),
        "a refused note was committed"
    );
    let clean = json!({"version": 3, "commands": [note("examples", "follow the plain layout")]});
    let replied = world.run_with_stdin(&["reply", "direct"], &clean.to_string());
    assert!(
        [0, onepipeline::error::EXIT_QUEUED].contains(&replied.code),
        "{}",
        replied.stderr
    );
    world.release("generalize.go");
}

/// What the reconciler answered each queued envelope with, by envelope id.
fn outcome_of(world: &World, run: &str, id: u64) -> Option<Value> {
    world
        .command_outcomes(run)
        .into_iter()
        .find(|outcome| outcome["id"].as_u64() == Some(id))
}

#[cfg(unix)]
#[test]
fn an_envelope_queued_past_every_submission_check_and_one_replayed_by_an_adopter_meet_the_check() {
    let (world, _) = held_mixed_run("replayed", &["zephyrine"]);
    // Queued straight onto the run's own bus, as a host's channel server queues one:
    // no `reply` judged it, so the reconciler is the first to.
    crate::malformed_envelopes::sent_through_the_bus(
        &world,
        "replayed",
        &json!({"version": 3, "commands": [note("examples", "follow the zephyrine layout")]}),
    );
    world.until("the driver to answer the queued envelope", |world| {
        outcome_of(world, "replayed", 0).is_some()
    });
    let answered = outcome_of(&world, "replayed", 0).expect("an outcome");
    assert!(
        answered.to_string().contains("public node examples"),
        "the queued leak was not refused by the boundary: {answered}"
    );

    // With the driver gone, a second one waits in the queue for whoever takes the run
    // over; the adopter replays it through the same check.
    crate::harness::end_driver(&world, "replayed");
    crate::malformed_envelopes::sent_through_the_bus(
        &world,
        "replayed",
        &json!({"version": 3, "commands": [
            {"op": "amend", "id": "examples", "text": "match the zephyrine output"}
        ]}),
    );
    world.run(&["adopt", "replayed", "--detach"]).exited(0);
    world.until("the adopter to answer the replayed envelope", |world| {
        outcome_of(world, "replayed", 1).is_some()
    });
    let answered = outcome_of(&world, "replayed", 1).expect("an outcome");
    assert!(
        answered.to_string().contains("public node examples"),
        "the replayed leak was not refused by the boundary: {answered}"
    );
    assert!(
        world
            .events_of("replayed", "edit-committed")
            .iter()
            .all(|event| !event["payload"].to_string().contains("zephyrine")),
        "a refused leak was committed"
    );
    world.release("generalize.go");
}

/// A world for the run term scope: the private `service` (identity A) and `beta`
/// (identity B), the public `library`, and a plan store whose configuration opts into
/// the public boundary, so every write a run projects onto it is checked.
fn scoped_world(name: &str) -> (World, Repository, Repository, Repository) {
    let (world, a, library) = mixed_world(name);
    let b = world.extra_repository("beta");
    declare_visibility(
        &world,
        &[
            ("service", "private"),
            ("beta", "private"),
            ("library", "public"),
        ],
    );
    let onevcs = crate::harness::onevcs_binary()
        .to_string_lossy()
        .into_owned();
    let world = world
        .with_env(
            "ONETASKGRAPH_WRITE_POLICY__CHECK_COMMAND",
            &json!([
                onevcs,
                "boundary",
                "check",
                "--destination",
                "public",
                "--input",
                "-"
            ])
            .to_string(),
        )
        .with_env(
            "ONETASKGRAPH_WRITE_POLICY__VISIBILITY_COMMAND",
            &format!("{onevcs},boundary,inspect,--input,-,--json"),
        );
    (world, a, b, library)
}

/// A public node in `library` whose work is a file saying `said`.
fn saying(world: &World, id: &str, said: &str) -> Value {
    world.script(&format!("{id}.work"), &format!("{said}\n"));
    public(id, &[])
}

/// The detail a node settled with.
fn detail_of(world: &World, run: &str, node: &str) -> String {
    world
        .events_of(run, "node-settled")
        .into_iter()
        .rev()
        .find(|event| event["labels"]["node"] == node)
        .map(|event| {
            event["payload"]["detail"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default()
}

/// What the write-back recorded of each attempt to project the run onto its store.
fn projections(world: &World, run: &str) -> String {
    std::fs::read_to_string(world.run_file(run, "writeback-projections.jsonl")).unwrap_or_default()
}

#[test]
fn a_run_naming_a_but_not_b_refuses_as_owner_name_and_passes_bs_through_every_check() {
    let (world, _, _, library) = scoped_world("scope-a-not-b");
    // Two exports: one whose example names A, one whose example names only B.
    world.script("gen-a.work-at", "examples/generic/a.md");
    world.script("gen-a.work", "copied from owner/service\n");
    world.script("gen-b.work-at", "examples/generic/b.md");
    world.script("gen-b.work", "copied from owner/beta\n");
    let mut edge_a = edge("gen-a", "cons-a");
    edge_a["target_directory"] = json!("examples/a");
    let mut edge_b = edge("gen-b", "cons-b");
    edge_b["target_directory"] = json!("examples/b");
    let plan = mixed_plan(
        "scoped",
        vec![
            producer("gen-a", &[]),
            producer("gen-b", &[]),
            public("cons-a", &["gen-a"]),
            public("cons-b", &["gen-b"]),
            // Two publications: one naming A, one naming only B.
            saying(&world, "pub-a", "names owner/service"),
            saying(&world, "pub-b", "names owner/beta"),
        ],
        vec![edge_a, edge_b],
    );
    let project = world.plan("scoped", &plan);
    world.run(&["start", &project, "--attach"]).settled();

    // Export: A's name refused, B's passed.
    assert_eq!(
        settled(&world, "scoped", "cons-a")["outcome"],
        "infrastructure-failure"
    );
    assert!(
        detail_of(&world, "scoped", "cons-a").contains("public output carries a term"),
        "{}",
        detail_of(&world, "scoped", "cons-a")
    );
    assert_eq!(
        settled(&world, "scoped", "cons-b")["status"],
        "done",
        "{}",
        world.dump()
    );
    // Publication: A's name refused, B's passed.
    assert_ne!(settled(&world, "scoped", "pub-a")["status"], "done");
    assert!(
        detail_of(&world, "scoped", "pub-a").contains("public output carries a term"),
        "{}",
        detail_of(&world, "scoped", "pub-a")
    );
    assert_eq!(
        settled(&world, "scoped", "pub-b")["status"],
        "done",
        "{}",
        world.dump()
    );

    // The bus validator, over the same run: A's name refused, B's passed.
    validated(
        &world,
        "scoped",
        &json!({"version": 3, "commands": [note("cons-b", "keep owner/service out")]}),
    )
    .exited(1);
    validated(
        &world,
        "scoped",
        &json!({"version": 3, "commands": [note("cons-b", "owner/beta is fine to name")]}),
    )
    .exited(0);

    // Plan-store writes, in a second run naming A: a node an edit adds is created on the
    // store by the write-back, so its whole task is a write the store checks. One citing
    // only B is created; one citing A is refused before the store is written.
    world.script("gen-c.wait", "hold");
    let leaking = world.plan(
        "leaking",
        &mixed_plan(
            "leaking",
            vec![producer("gen-c", &[]), public("cons-c", &["gen-c"])],
            vec![edge("gen-c", "cons-c")],
        ),
    );
    world.run(&["start", &leaking, "--detach"]).exited(0);
    world.until("the producer to be in flight", |world| {
        !world.events_of("leaking", "node-dispatched").is_empty()
    });
    let citing = |id: &str, cited: &str| {
        let mut node = lifecycle(id, &[]);
        node["parked"] = json!(true);
        node["task"] = json!(format!(
            "## What\nShip {id}.\n\n## Why\nIt cites {cited}.\n\n## Acceptance criteria\n- {id} is published."
        ));
        node
    };
    reply(
        &world,
        "leaking",
        json!([{"op": "add", "node": citing("cites-b", "owner/beta")}]),
    );
    world.until("the node citing B to reach the store", |world| {
        world
            .store_tasks(&leaking)
            .iter()
            .any(|task| task.to_string().contains("It cites owner/beta."))
    });
    reply(
        &world,
        "leaking",
        json!([{"op": "add", "node": citing("cites-a", "owner/service")}]),
    );
    world.until("the write-back to be refused", |world| {
        projections(world, "leaking").contains("boundary-refused")
    });
    let projected = projections(&world, "leaking");
    assert!(
        !projected.contains("owner/service"),
        "the refusal named the term: {projected}"
    );
    assert!(
        world
            .store_tasks(&leaking)
            .iter()
            .all(|task| !task.to_string().contains("It cites owner/service.")),
        "the refused write reached the store"
    );
    world.release("gen-c.go");
    world.until("the second run to settle", |world| {
        world.run_file("leaking", "result.json").is_file()
            || world
                .events_of("leaking", "driver-exited")
                .iter()
                .any(|_| true)
    });

    // Outside any run, the host's registered private identities decide: B is one, so a
    // public branch naming it is refused although no run ever named B.
    crate::harness::git(
        &world,
        &library.checkout,
        &["checkout", "-q", "-b", "notes"],
    );
    std::fs::write(library.checkout.join("NOTES.md"), "see owner/beta\n").expect("a note");
    crate::harness::git(&world, &library.checkout, &["add", "-A"]);
    crate::harness::git(
        &world,
        &library.checkout,
        &["commit", "-qm", "docs: add notes"],
    );
    crate::harness::git(&world, &library.checkout, &["checkout", "-q", "main"]);
    let outside = world.run(&[
        "publish-branch",
        "notes",
        "--repo",
        "library",
        "--title",
        "docs: add notes",
    ]);
    assert_ne!(outside.code, 0, "{}", outside.stdout);
    assert!(
        format!("{}{}", outside.stdout, outside.stderr).contains("public output carries a term"),
        "{}\n{}",
        outside.stdout,
        outside.stderr
    );
}

#[test]
fn a_plan_naming_no_private_repository_derives_no_term_even_where_the_registry_cannot() {
    let (world, a, _, _) = scoped_world("scope-empty");
    // A third registered private identity whose committed declaration cannot be read:
    // every check scoped to the whole registry is unavailable while it stands.
    let c = world.extra_repository("gamma");
    declare_visibility(
        &world,
        &[
            ("service", "private"),
            ("beta", "private"),
            ("gamma", "private"),
            ("library", "public"),
        ],
    );
    std::fs::write(
        c.checkout.join("private-terms.toml"),
        "schema_version = 1\nterms = [\n",
    )
    .expect("a broken declaration");
    crate::harness::git(&world, &c.checkout, &["add", "-A"]);
    crate::harness::git(&world, &c.checkout, &["commit", "-qm", "chore: break it"]);
    let registry_scoped = std::process::Command::new(crate::harness::onevcs_binary())
        .args([
            "boundary",
            "check",
            "--destination",
            "public",
            "--input",
            "-",
        ])
        .env("ONEVCS_HOME", world.onevcs_home())
        .env("GIT_CONFIG_GLOBAL", world.gitconfig())
        .env("ONEVCS_GH", crate::harness::double("fake-gh"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .expect("a stdin")
                .write_all(br#"{"destination":"public","text":["nothing at all"]}"#)?;
            child.wait_with_output()
        })
        .expect("onevcs runs");
    assert!(
        !matches!(registry_scoped.status.code(), Some(0 | 1)),
        "a registry-scoped check decided: {}",
        String::from_utf8_lossy(&registry_scoped.stdout)
    );
    drop(a);

    // An all-public plan: its node's task and its publication name both A and B.
    let mut named = saying(&world, "notes", "names owner/service and owner/beta");
    named["task"] = json!(
        "## What\nShip notes.\n\n## Why\nIt cites owner/service and owner/beta.\n\n\
         ## Acceptance criteria\n- notes is published."
    );
    let project = world.plan(
        "public-only",
        &mixed_plan("public-only", vec![named], vec![]),
    );
    world.run(&["start", &project, "--attach"]).settled();
    assert_eq!(
        settled(&world, "public-only", "notes")["status"],
        "done",
        "{}",
        world.dump()
    );
    let projected = projections(&world, "public-only");
    assert!(
        !projected.contains("boundary"),
        "a plan-store write met the boundary: {projected}"
    );
    let stored = world.store_tasks(&project);
    assert!(
        stored
            .iter()
            .any(|task| task.to_string().contains("owner/service and owner/beta")),
        "the plan store does not hold the run's task: {stored:?}"
    );
    validated(
        &world,
        "public-only",
        &json!({"version": 3, "commands": [note("notes", "owner/service and owner/beta")]}),
    )
    .exited(0);
}

#[test]
fn a_live_edit_naming_b_puts_b_in_the_run_scope_for_every_check_after_it() {
    let (world, _, _, _) = scoped_world("scope-edit");
    world.script("generalize.wait", "hold");
    let project = world.plan("grown", &exported_plan("grown"));
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the producer to be in flight", |world| {
        !world.events_of("grown", "node-dispatched").is_empty()
    });
    let naming_b = json!({"version": 3, "commands": [note("examples", "cite owner/beta")]});
    validated(&world, "grown", &naming_b).exited(0);

    // A node in B's repository joins the plan.
    let mut in_b = lifecycle("in-beta", &[]);
    in_b["repo"] = json!("beta");
    in_b["parked"] = json!(true);
    reply(&world, "grown", json!([{"op": "add", "node": in_b}]));
    world.until("the edit to commit", |world| {
        world
            .events_of("grown", "edit-committed")
            .iter()
            .any(|event| event["payload"]["command"]["op"] == "add")
    });
    validated(&world, "grown", &naming_b).exited(1);
    world
        .run_with_stdin(&["reply", "grown"], &naming_b.to_string())
        .exited(REFUSED);
    world.release("generalize.go");
}

/// One task written through the standalone store's own command line — its configured
/// `write_policy` commands, run as the `onevcs` binary — under the world's environment.
fn store_cli_create(
    world: &World,
    project: &str,
    title: &str,
    body: &str,
    extra: &[&str],
) -> crate::harness::Run {
    let template = world.cmd(&[]);
    let mut command = std::process::Command::new(crate::harness::onetaskgraph_binary());
    for (key, value) in template.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    if let Some(dir) = template.get_current_dir() {
        command.current_dir(dir);
    }
    // The `onevcs` the policy commands name, linked into place again now: a world this
    // process dropped earlier may have taken the directory its alias was first put in.
    let onevcs = crate::harness::onevcs_binary()
        .to_string_lossy()
        .into_owned();
    command
        .env(
            "ONETASKGRAPH_WRITE_POLICY__CHECK_COMMAND",
            format!("{onevcs},boundary,check,--destination,public,--input,-"),
        )
        .env(
            "ONETASKGRAPH_WRITE_POLICY__VISIBILITY_COMMAND",
            format!("{onevcs},boundary,inspect,--input,-,--json"),
        );
    let body_file = world.root.join(format!("{title}.md"));
    std::fs::write(&body_file, body).expect("the body is written");
    let native = project.strip_prefix("plans:").unwrap_or(project);
    let body_path = body_file.to_string_lossy().into_owned();
    let mut args = vec![
        "task",
        "create",
        "plans",
        "--project",
        native,
        "--title",
        title,
        "--body-file",
        &body_path,
        "--no-interactive",
    ];
    args.extend_from_slice(extra);
    command.args(&args);
    world.run_on(command, &format!("onetaskgraph {}", args.join(" ")))
}

/// Launch `nodes` as a run named `name`, held open by an agent node of its own, so a
/// live edit's projection has a driver to make it.
fn held_run(world: &World, name: &str, nodes: Vec<Value>) -> String {
    let keep = format!("keep-{name}");
    world.script(&format!("{keep}.wait"), "hold");
    let mut nodes = nodes;
    nodes.push(crate::harness::agent(&keep, &[]));
    let project = world.plan(name, &mixed_plan(name, nodes, vec![]));
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run's keeper to be in flight", |world| {
        world
            .events_of(name, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == keep.as_str())
    });
    project
}

/// A parked node whose task cites `cited`, in `repo`.
fn citing(id: &str, repo: &str, cited: &str) -> Value {
    let mut node = lifecycle(id, &[]);
    node["repo"] = json!(repo);
    node["parked"] = json!(true);
    node["task"] = json!(format!(
        "## What\nShip {id}.\n\n## Why\nIt cites {cited}.\n\n## Acceptance criteria\n- {id} is published."
    ));
    node
}

#[test]
fn the_store_cli_and_the_engine_refuse_and_pass_the_same_writes_in_the_same_words() {
    let (world, _, _, _) = scoped_world("parity");
    let scope = ["--term-scope", "github.com/owner/service"];

    // The engine: a run naming A, projecting nodes its edits add onto the store.
    let anchored = held_run(
        &world,
        "anchored",
        vec![citing("anchor", "service", "nothing")],
    );
    reply(
        &world,
        "anchored",
        json!([{"op": "add", "node": citing("cites-b", "service", "owner/beta")}]),
    );
    world.until("the allowed write to land", |world| {
        world
            .store_tasks(&anchored)
            .iter()
            .any(|task| task.to_string().contains("It cites owner/beta."))
    });
    reply(
        &world,
        "anchored",
        json!([{"op": "add", "node": citing("cites-a", "service", "owner/service")}]),
    );
    world.until("the denied write to be refused", |world| {
        projections(world, "anchored").contains("boundary-refused")
    });
    let engine_denied = projections(&world, "anchored");

    // The store's own command line, over the same scope: the same two decisions, the
    // refusal in the same words, before anything is written.
    let allowed = store_cli_create(
        &world,
        &anchored,
        "cli-cites-b",
        "## Why\nIt cites owner/beta.\n",
        &scope,
    );
    allowed.exited(0);
    let denied = store_cli_create(
        &world,
        &anchored,
        "cli-cites-a",
        "## Why\nIt cites owner/service.\n",
        &scope,
    );
    assert_ne!(denied.code, 0, "{}", denied.stdout);
    let sentence = "it carries a term of a private repository in its text";
    assert!(denied.stderr.contains(sentence), "{}", denied.stderr);
    assert!(engine_denied.contains(sentence), "{engine_denied}");
    let stored = world.store_tasks(&anchored).to_vec();
    assert!(stored
        .iter()
        .any(|task| task.to_string().contains("cli-cites-b")));
    assert!(
        stored
            .iter()
            .all(|task| !task.to_string().contains("It cites owner/service.")),
        "a refused write reached the store: {stored:?}"
    );
    world.release("keep-anchored.go");

    // A repository's visibility decides an item's classification the same way: an item
    // naming A's repository is private, which a store not declared private never takes.
    let named = held_run(&world, "named", vec![citing("plain", "service", "nothing")]);
    reply(
        &world,
        "named",
        json!([{"op": "add", "node": citing("in-a", "github.com/owner/service", "nothing")}]),
    );
    world.until("the private item to be refused", |world| {
        projections(world, "named").contains("not-private-destination")
    });
    let in_a = store_cli_create(
        &world,
        &named,
        "cli-in-a",
        "## Why\nIt cites nothing.\n",
        &[
            "--repository",
            "github.com/owner/service",
            "--term-scope",
            "github.com/owner/service",
        ],
    );
    assert_ne!(in_a.code, 0, "{}", in_a.stdout);
    // One decision on both sides — the item is private, so a destination not verified
    // private never takes it — met by each verb's own first rule: the CLI files it under
    // a public project, the engine's copy reaches a source not declared private.
    let private_sentence = "is private, and ";
    assert!(in_a.stderr.contains(private_sentence), "{}", in_a.stderr);
    assert!(
        in_a.stderr.contains("nothing was written"),
        "{}",
        in_a.stderr
    );
    assert!(projections(&world, "named").contains(private_sentence));
    assert!(
        world
            .store_tasks(&named)
            .iter()
            .all(|task| !task.to_string().contains("cli-in-a") && task["id"] != "in-a"),
        "a private item reached the store"
    );
    world.release("keep-named.go");

    // And a scope whose private identity's committed declaration cannot be read is
    // unavailable on both, which is never a pass.
    let c = world.extra_repository("gamma");
    declare_visibility(
        &world,
        &[
            ("service", "private"),
            ("beta", "private"),
            ("gamma", "private"),
            ("library", "public"),
        ],
    );
    std::fs::write(
        c.checkout.join("private-terms.toml"),
        "schema_version = 1\nterms = [\n",
    )
    .expect("a broken declaration");
    crate::harness::git(&world, &c.checkout, &["add", "-A"]);
    crate::harness::git(&world, &c.checkout, &["commit", "-qm", "chore: break it"]);
    let unregistered = held_run(
        &world,
        "unregistered",
        vec![citing("elsewhere", "gamma", "nothing")],
    );
    reply(
        &world,
        "unregistered",
        json!([{"op": "add", "node": citing("more", "gamma", "nothing")}]),
    );
    world.until("the write to be unavailable", |world| {
        projections(world, "unregistered").contains("boundary-unavailable")
    });
    let cli_unavailable = store_cli_create(
        &world,
        &unregistered,
        "cli-unregistered",
        "## Why\nIt cites nothing.\n",
        &["--term-scope", "github.com/owner/gamma"],
    );
    assert_ne!(cli_unavailable.code, 0, "{}", cli_unavailable.stdout);
    let unavailable = "the public boundary check is unavailable";
    assert!(
        cli_unavailable.stderr.contains(unavailable),
        "{}",
        cli_unavailable.stderr
    );
    assert!(projections(&world, "unregistered").contains(unavailable));
    world.release("keep-unregistered.go");
}

#[test]
fn every_public_surface_of_a_mixed_run_names_nothing_of_its_private_side() {
    let (world, private, library) = mixed_world("boundary-surfaces");
    declare_terms(&world, &private, &["zephyrine"]);
    // Every private identifier the run holds, synthetic: its plan, its producer's id and
    // title, the private repository, and what the producer's branch is kept as.
    let mut generalize = producer("zephyrine-gen", &[]);
    generalize["title"] = json!("feat: the zephyrine generalizer");
    world.script("zephyrine-gen.work-at", "examples/generic/sample.md");
    world.script("zephyrine-gen.work", "a generic example\n");
    world.script("examples.work", "built on it\n");
    let mut consumer = public("examples", &["zephyrine-gen"]);
    // A consumer waiting for its dependencies' releases waits for none across the edge.
    consumer["adoption"] = json!("published");
    let plan = mixed_plan(
        "zephyrine-plan",
        vec![generalize, consumer],
        vec![edge("zephyrine-gen", "examples")],
    );
    let project = world.plan("zephyrine-plan", &plan);
    let graph = world.pr_author_graph();
    world.script(
        "pr-author.body",
        "## What\nGeneric examples, and what built on them.\n",
    );
    world
        .run(&["start", &project, "--pr-author-graph", &graph, "--attach"])
        .settled();
    let producer_node = settled(&world, "zephyrine-plan", "zephyrine-gen");
    let consumer_node = settled(&world, "zephyrine-plan", "examples");
    assert_eq!(
        consumer_node["status"],
        "done",
        "{consumer_node}\n{}",
        world.dump()
    );
    let private_names: Vec<String> = vec![
        "zephyrine".to_owned(),
        "owner/service".to_owned(),
        "zephyrine-plan-board".to_owned(),
        producer_node["branch"]
            .as_str()
            .expect("a kept branch")
            .to_owned(),
        producer_node["head"]
            .as_str()
            .expect("a kept head")
            .to_owned(),
        private.checkout.to_string_lossy().into_owned(),
    ];
    let clean = |surface: &str, text: &str| {
        for name in &private_names {
            assert!(
                !text.contains(name.as_str()),
                "{surface} names {name}:\n{text}"
            );
        }
    };

    // What the consumer's worker and its judge were handed.
    let tasks = tasks_of(&world, "examples");
    assert!(!tasks.is_empty());
    for task in &tasks {
        clean("the consumer's task", task);
        assert!(!task.contains("## Cross-repository references"), "{task}");
        assert!(!task.contains("have arrived"), "{task}");
    }
    // What its drafter was handed: the run is named by its environment, not spelled out.
    let drafts: Vec<String> = world
        .invocations()
        .into_iter()
        .filter(|call| {
            call["tool"] == "oneagentgraph"
                && call["args"].as_array().is_some_and(|args| {
                    args.iter()
                        .any(|arg| arg == "onepipeline.persona=pr-author")
                        && args.iter().any(|arg| arg == "onepipeline.node=examples")
                })
        })
        .filter_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|arg| arg == "--task")?;
            args.get(at + 1)?.as_str().map(str::to_owned)
        })
        .collect();
    assert!(
        !drafts.is_empty(),
        "the consumer was not drafted for: {}",
        world.dump()
    );
    for draft in &drafts {
        clean("the consumer's drafter context", draft);
        assert!(draft.contains("$ONEPIPELINE_RUN_ID"), "{draft}");
        assert!(draft.contains("## Repository boundary"), "{draft}");
    }
    // Its branch, every session event its run relayed, and its settlement.
    clean(
        "the consumer's branch",
        consumer_node["branch"].as_str().unwrap_or_default(),
    );
    for event in world
        .journal("zephyrine-plan")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "examples")
        .filter(|event| event["source"] == "vcs" || event["kind"] == "node-settled")
    {
        let payload = event["payload"].to_string();
        clean("a consumer's session event", &payload);
    }
    // The public repository's whole history.
    git_out(&world, &library.checkout, &["fetch", "origin"]);
    clean(
        "the public history",
        &git_out(
            &world,
            &library.checkout,
            &["log", "--all", "--format=%an %ae %s%n%b"],
        ),
    );
    // The run's own `results` line for the consumer.
    let results = world.run(&["results", "zephyrine-plan"]);
    results.exited(0);
    for line in results
        .stdout
        .lines()
        .filter(|line| line.contains("examples"))
    {
        clean("the consumer's results line", line);
    }
}

#[test]
fn a_stricter_clearance_is_never_downgraded() {
    let (world, _, _) = mixed_world("boundary-strict");
    // A task classified private, in a public repository, is private: a public node may
    // not depend on it, and a retry stating nothing about it keeps it private.
    let mut classified = public("classified", &[]);
    classified["classification"] = json!("private");
    let reason = refused_by_check(
        &world,
        "strict",
        &mixed_plan(
            "strict",
            vec![classified.clone(), public("downstream", &["classified"])],
            vec![],
        ),
    );
    assert!(reason.contains("private node classified"), "{reason}");

    world.script("classified.fail", "1");
    world.script("held.wait", "hold");
    let project = world.plan(
        "retried-strict",
        &mixed_plan(
            "retried-strict",
            vec![classified, crate::harness::agent("held", &[])],
            vec![],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    until_settled(&world, "retried-strict", "classified");
    reply(
        &world,
        "retried-strict",
        json!([{"op": "retry", "id": "classified", "node": public("classified-2", &[])}]),
    );
    // The replacement is as private as what it replaced: a public node depending on it
    // is still an ordinary private-to-public dependency.
    world
        .run_with_stdin(
            &["reply", "retried-strict"],
            &json!({"version": 3, "commands": [
                {"op": "add", "node": public("downstream", &["classified-2"])}
            ]})
            .to_string(),
        )
        .exited(REFUSED)
        .err_has("private node classified-2");
    world.release("held.go");
}
