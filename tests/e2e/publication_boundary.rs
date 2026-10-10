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
            "version: 4\nrules:\n{rules}default:\n  publication: local-direct\n  approvals: none\n"
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
    let (world, private, public) = mixed_world("boundary-export");
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
    git_out(&world, &public.checkout, &["fetch", "origin"]);
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
        &public.checkout,
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
        &public.checkout,
        &["log", "--format=%H", "-1", "origin/main~1"],
    );
    assert_eq!(parents, base_then, "the export is not on the public base");
    let exported = git_out(
        &world,
        &public.checkout,
        &["show", "origin/main:examples/sample.md"],
    );
    assert_eq!(exported, "a generic example");
    let private_head = producer["head"].as_str().expect("a kept head");
    let reachable = std::process::Command::new("git")
        .args(["cat-file", "-e", &format!("{private_head}^{{commit}}")])
        .current_dir(&public.origin)
        .status()
        .expect("git runs");
    assert!(
        !reachable.success(),
        "the producer's commit reached the public repository"
    );
    let whole = git_out(
        &world,
        &public.checkout,
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
