//! Adoption: when a node launches relative to its dependencies' **releases**.
//!
//! Every journey here drives the real repository side. `onevcs` is a library this
//! crate calls, so nothing substitutes what a publication did, what a release
//! probe answered, or what an acknowledgement recorded: the probe is a real
//! script committed into a real repository and run as a real subprocess, and the
//! human step is the sibling's own `acknowledge` operation, called the way a
//! person's `onevcs release acknowledge` calls it.
//!
//! The three behaviours, one journey each: a plan naming neither field behaving
//! exactly as it did before there were fields, a fast-adoption node receiving its
//! reference block and then its arrival note, and a published-adoption node held
//! and then started when its dependency's release answers. A fourth drives the
//! two release **styles** side by side, and proves the only differences between
//! them are where the readiness answer comes from and what is reported.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary, and the sibling these journeys are about — `onevcs` — is the real library, over
// real git, a real origin on disk, a real probe subprocess, and its own real acknowledge
// operation. `oneagentgraph` is substituted at its subprocess boundary so a journey states
// a dispatch outcome rather than paying for a model turn. `harness.rs` carries the same
// suppression and the full rationale.

use std::path::Path;

use crate::harness::{lifecycle, plan_of, Repository, World};
use onepipeline::plan::CROSS_REPO_REFERENCES_HEADING;
use serde_json::{json, Value};

/// The repository the *dependency* lands in, which is the one that releases.
const ENGINE: &str = "engine";

/// A release-targets document declaring one automated target for the engine
/// repository, answered by the probe the journey committed into it.
fn automated(script: &str) -> String {
    document(script, "")
}

/// The same, plus a **human-step** target beside the automated one — a target no
/// probe can answer, whose version is whatever a person records afterwards.
fn both_styles(script: &str) -> String {
    document(
        script,
        &format!("    - name: wheel\n      style: human-step\n      action: \"{ACTION}\""),
    )
}

/// This host's release-targets document: what the engine repository releases, and
/// what every other repository adopts.
///
/// Written at the one conventional path under the state root, which is the only
/// place `onevcs` looks — deliberately not reachable through a key on the
/// registry, because every build already in the field refuses a key it does not
/// know and the first host to configure a release target would stop them all.
fn document(script: &str, extra: &str) -> String {
    let extra: String = extra.lines().map(|line| format!("{line}\n")).collect();
    format!(
        "{}{extra}default:\n\x20 adoption: fast\n",
        repositories(script)
    )
}

/// The document's version and its one rule for the engine repository, up to but
/// not including whatever a journey states after them.
fn repositories(script: &str) -> String {
    format!(
        "version: 1\n\
         repositories:\n\
         \x20 - match: {{host: github.com, owner: owner, name: engine}}\n\
         \x20   default_target: crate\n\
         \x20   targets:\n\
         \x20   - name: crate\n\
         \x20     style: automated\n\
         \x20     probe: {{script: {script}, timeout_seconds: 30}}\n"
    )
}

/// What a person has to do for the human-step target, as the document states it
/// and as every rendering of the wait must carry it.
const ACTION: &str = "build the wheel and upload it to PyPI, then run onevcs release acknowledge";

/// A world with two repositories: the one that releases, and the one whose node
/// depends on it.
///
/// The dependency lands *outside* the consumer's repository, which is the whole
/// condition every behaviour here is keyed to.
fn two_repositories(world: &World) -> (Repository, Repository) {
    let consumer = world.repository("local-direct", &[]);
    let engine = world.extra_repository(ENGINE);
    (engine, consumer)
}

/// The consumer node: a lifecycle node in the *other* repository, depending on
/// the engine node.
fn consumer(adoption: Option<&str>) -> Value {
    let mut node = lifecycle("consumer", &["engine"]);
    if let Some(adoption) = adoption {
        node["adoption"] = json!(adoption);
    }
    node
}

/// The engine node: a lifecycle node in the repository that releases.
fn engine() -> Value {
    let mut node = lifecycle(ENGINE, &[]);
    node["repo"] = json!(ENGINE);
    node
}

/// Say what the probe answers from now on.
fn releases_at(answer: &Path, version: &str) {
    std::fs::write(answer, format!("{version}\n")).expect("the probe's answer is written");
}

/// The task prose one node's dispatch was handed, read off the `--task` the
/// launch really carried.
///
/// Off the invocation rather than off the stream, because a journey that holds a
/// turn open asserts on the prose *while it is held* — and a held turn has not
/// reported an activity yet. The launch's own argv is what the dispatch was
/// composed with, which is exactly the question.
fn task_of(world: &World, node: &str) -> String {
    tasks_of(world, node).pop().unwrap_or_default()
}

/// The task prose every one of a node's dispatches was handed, in order.
fn tasks_of(world: &World, node: &str) -> Vec<String> {
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

/// Whether a node has been dispatched at all.
fn dispatched(world: &World, run: &str, node: &str) -> bool {
    world
        .events_of(run, "node-dispatched")
        .iter()
        .any(|event| event["labels"]["node"] == node)
}

/// The `awaiting` entries of the last `release-wait` raised about one node.
fn awaiting(world: &World, run: &str, node: &str) -> Vec<Value> {
    world
        .events_of(run, "release-wait")
        .into_iter()
        .rfind(|event| event["labels"]["node"] == node)
        .and_then(|event| event["payload"]["awaiting"].as_array().cloned())
        .unwrap_or_default()
}

/// The answer the last `release-wait` recorded for the one release a node awaits.
///
/// `None` before any wait has been raised about it, which is a run that has not
/// got there yet rather than a wait with no answer — `not-answered` is a thing
/// the payload says out loud.
fn answered(world: &World, run: &str, node: &str) -> Option<String> {
    awaiting(world, run, node)
        .first()?
        .get("last_answer")?
        .as_str()
        .map(str::to_owned)
}

/// The text of the last release-wait surface raised about one node.
fn wait_surface(world: &World, run: &str, node: &str) -> String {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .rfind(|event| {
            event["payload"]["kind"] == "release-wait" && event["labels"]["node"] == node
        })
        .map(|event| {
            event["payload"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default()
}

/// Start a run detached, so the journey can move the world under a live loop.
///
/// Every node writes a file, because a lifecycle node whose dispatch changed
/// nothing publishes nothing — and a dependency that never landed has no release
/// to ask about. What each of these journeys is keyed to is a *landing*, so each
/// node's work has to be real.
fn start(world: &World, name: &str, nodes: Vec<Value>) -> String {
    for node in &nodes {
        let id = node["id"].as_str().expect("every node has an id");
        world.script(&format!("{id}.work"), &format!("{id} did its work\n"));
    }
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    name.to_string()
}

/// A world whose release watch answers on this journey's timescale rather than on
/// an operator's.
///
/// The two bounds are the shipped ones — 120 seconds between probes and 900
/// between surfaces — which are right for a run that waits days for a release and
/// wrong for a test that has to see both happen. Nothing else about the watch
/// changes: one hold, indefinite, released only by an answer of released.
fn watching(name: &str) -> World {
    World::new(name)
        .with_env("ONEPIPELINE_RELEASE_POLL_SECONDS", "1")
        .with_env("ONEPIPELINE_RELEASE_SURFACE_SECONDS", "1")
}

/// A plan naming neither new field produces exactly the run it produced before
/// there were fields: no reference block, no hold, and a task that is the node's
/// own prose and nothing else.
///
/// The compatibility promise, driven where it is actually at risk — a run over
/// **two repositories**, which is the shape that grows a reference block the
/// moment a node opts in. A host with no release-targets document at all is the
/// other half of it, and this journey is that too.
#[test]
fn a_plan_naming_neither_field_runs_exactly_as_it_did() {
    let world = World::new("adoption-unchanged");
    world.write_graphs();
    let (_engine, _consumer) = two_repositories(&world);

    let run = start(&world, "adoption-unchanged", vec![engine(), consumer(None)]);
    world.until("both nodes to settle", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });

    let task = task_of(&world, "consumer");
    assert!(
        !task.contains(CROSS_REPO_REFERENCES_HEADING),
        "a node naming no adoption gained a reference block: {task}"
    );
    assert_eq!(
        task,
        lifecycle("consumer", &["engine"])["task"]
            .as_str()
            .expect("the node states its task"),
        "the rendered task is not byte-identical to the node's own prose"
    );
    for kind in ["release-wait", "release-arrived", "release-adopted"] {
        assert!(
            world.events_of(&run, kind).is_empty(),
            "a plan naming neither field recorded a `{kind}`"
        );
    }
}

/// A fast-adoption node launches on its dependency's **branch** readiness, is
/// handed the git references of the work it cannot yet pin a version to, and is
/// told — while it is still running — the moment the release arrives.
///
/// The whole arc in one journey, because the two halves are one promise: the
/// worker is given something to pin against *and* the correction that moves it
/// off that pin, without a person noticing and intervening.
#[test]
fn a_fast_node_pins_against_git_and_is_told_when_the_release_arrives() {
    let world = watching("adoption-fast");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    // What is released when the engine's work lands, which is the baseline the
    // arrival is measured against.
    releases_at(&answer, "0.1.0");

    // The consumer's turn is held open, so the note has a running turn to reach.
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");

    let run = start(
        &world,
        "adoption-fast",
        vec![engine(), consumer(Some("fast"))],
    );
    world.until("the consumer's turn to open", |world| {
        world
            .events_of(&run, "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == "consumer")
    });

    // It launched on the branch, not on a version: the block names the
    // repository, the branch, the landing commit, and the target.
    let task = task_of(&world, "consumer");
    let block = task
        .split_once(CROSS_REPO_REFERENCES_HEADING)
        .map(|(_, rest)| rest.to_owned())
        .unwrap_or_else(|| panic!("the dispatched task carries no reference block:\n{task}"));
    assert!(
        block.contains("| dependency | repository | branch | commit | release target |"),
        "the block carries no table:\n{block}"
    );
    let row = block
        .lines()
        .find(|line| line.starts_with("| engine |"))
        .unwrap_or_else(|| panic!("no row for the engine dependency:\n{block}"));
    let cells: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
    assert_eq!(cells[1], "github.com/owner/engine", "row: {row}");
    assert!(!cells[2].is_empty(), "the branch cell is empty: {row}");
    assert_eq!(
        cells[3],
        landing_commit(&world, &run, "engine"),
        "the commit cell is not the landing the run observed: {row}"
    );
    assert_eq!(cells[4], "crate", "row: {row}");
    assert!(
        task.contains("Pin against the git references below rather than against a version"),
        "the block does not say what it is for:\n{task}"
    );

    // Nothing has arrived yet: the probe answers exactly the baseline.
    assert!(
        world.events_of(&run, "release-adopted").is_empty(),
        "a note was delivered before any release arrived"
    );

    // The release happens. The still-running node is told, once, into its live
    // turn — no person, no reply, no dispatch of its own.
    releases_at(&answer, "0.2.0");
    world.until("the release to be adopted", |world| {
        !world.events_of(&run, "release-adopted").is_empty()
    });

    let arrived = world.events_of(&run, "release-arrived");
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    assert_eq!(arrived[0]["payload"]["node"], json!("consumer"));
    assert_eq!(arrived[0]["payload"]["dep"], json!("engine"));
    assert_eq!(
        arrived[0]["payload"]["identity"],
        json!("github.com/owner/engine")
    );
    assert_eq!(arrived[0]["payload"]["target"], json!("crate"));
    assert_eq!(arrived[0]["payload"]["style"], json!("automated"));
    assert_eq!(arrived[0]["payload"]["version"], json!("0.2.0"));

    let adopted = world.events_of(&run, "release-adopted");
    assert_eq!(adopted.len(), 1, "the note was delivered more than once");
    assert_eq!(
        adopted[0]["payload"]["delivery"],
        json!("live"),
        "the note did not reach the running turn"
    );
    assert_eq!(
        adopted[0]["payload"]["versions"],
        json!([{"identity": "github.com/owner/engine", "target": "crate", "version": "0.2.0"}])
    );

    // The lever really was pulled, and the sibling's own record of it reached the
    // merged store stamped with the node it was about.
    let interrupted = world.events_of(&run, "turn-interrupted");
    eprintln!("DIAG kinds={:?}", world.kinds(&run));
    eprintln!(
        "DIAG invocations={:?}",
        world
            .invocations()
            .iter()
            .filter(|c| c["tool"] == "oneagentgraph")
            .map(|c| c["args"][0].clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(interrupted.len(), 1, "{interrupted:?}");
    assert_eq!(interrupted[0]["payload"]["delivered"], json!(true));
    assert_eq!(interrupted[0]["labels"]["node"], json!("consumer"));

    world.release("consumer.go");
    world.until("the run to settle", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });

    // And the worker was told what the versions are, in a note that adds no bar.
    // Its own task prose cannot have carried this — it was rendered before the
    // release existed — so the redirection is the only way it got there.
    let note = redirected(&world, &run, "consumer");
    assert!(
        note.contains("github.com/owner/engine — crate 0.2.0")
            && note.contains("Move from the git pin to that released version"),
        "the running turn was not told which version arrived:\n{note}"
    );
    assert!(
        !note.to_lowercase().contains("acceptance criteria"),
        "the arrival note reads as a new bar:\n{note}"
    );
}

/// What one node's running turn was redirected with.
fn redirected(world: &World, run: &str, node: &str) -> String {
    world
        .journal(run)
        .into_iter()
        .rfind(|event| {
            event["labels"]["node"] == node
                && event["source"] == "agentgraph"
                && event["kind"] == "turn-activity"
                && event["payload"]["redirected"].is_string()
        })
        .map(|event| {
            event["payload"]["redirected"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default()
}

/// A fast-adoption node whose running turn has no lever is **not** told the note
/// reached one: the lever is really pulled, it really answers that there is no
/// turn, and the note is owed to the node's next dispatch instead.
///
/// The other half of `auto`, and the compatibility half: a harness with no
/// out-of-band turn control is what every `context` edit written before delivery
/// had modes ran under, and the note must be owed rather than lost. What a
/// deferred note then does — ride the next dispatch and be consumed by it — is
/// the `context` mechanism's own, driven end to end in `context_delivery.rs` and
/// folded from this record in `projection.rs`.
#[test]
fn an_arrival_note_with_no_live_turn_to_reach_is_owed_to_the_next_dispatch() {
    let world = watching("adoption-deferred");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    // A member on a harness with no out-of-band turn control: it runs, and there
    // is nothing to redirect.
    world.script("consumer.no-lever", "");
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");
    let run = start(
        &world,
        "adoption-deferred",
        vec![engine(), consumer(Some("fast"))],
    );

    // After the engine has landed — so the baseline its publication captured is
    // the version that was out then rather than the one about to be — and after
    // the consumer's turn has opened, so what the note meets is a turn that
    // exists and has no lever rather than a dispatch that has not spoken yet.
    world.until("the consumer's turn to open", |world| {
        world
            .events_of(&run, "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == "consumer")
    });
    releases_at(&answer, "0.2.0");
    world.until("the release to be adopted", |world| {
        !world.events_of(&run, "release-adopted").is_empty()
    });

    let adopted = world.events_of(&run, "release-adopted");
    assert_eq!(adopted.len(), 1, "the note was delivered more than once");
    assert_eq!(
        adopted[0]["payload"]["delivery"],
        json!("next"),
        "a note with no turn to reach was recorded as having reached one"
    );
    // The lever was pulled and answered, which is what tells this apart from a
    // note nobody tried to deliver.
    let interrupted = world.events_of(&run, "turn-interrupted");
    assert_eq!(interrupted.len(), 1, "{interrupted:?}");
    assert_eq!(interrupted[0]["payload"]["delivered"], json!(false));
    assert_eq!(interrupted[0]["labels"]["node"], json!("consumer"));

    world.release("consumer.go");
    world.until("the run to settle", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });
    // The turn that was running never saw it, and its own prose could not have
    // carried it — the task was rendered before the release existed.
    let dispatched = tasks_of(&world, "consumer");
    assert_eq!(dispatched.len(), 1, "{dispatched:?}");
    assert!(
        !dispatched[0].contains("0.2.0"),
        "a version that did not exist at launch reached the dispatch that launched:\n{}",
        dispatched[0]
    );
    assert!(redirected(&world, &run, "consumer").is_empty());
}

/// A `run:<id>#<node>` dependency is pinned against git like any other outside
/// the node's repository, and its row is read out of the **upstream run's own
/// ledger**.
///
/// It is out-of-repository whatever repository it lands in: the branch belongs to
/// another run, so the stacked-branch machinery this crate has cannot reach it
/// and a git pin is the only thing a worker can hold.
#[test]
fn a_cross_dag_dependency_is_pinned_against_git_and_named_from_the_upstreams_ledger() {
    let world = watching("adoption-crossdag");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    // The upstream lands the engine's work in a run of its own, and settles.
    let upstream = start(&world, "adoption-upstream", vec![engine()]);
    world.until("the upstream to settle", |world| {
        world.run_file(&upstream, "result.json").is_file()
    });

    // A second run, whose one node depends on that node of that run.
    let mut across = consumer(Some("fast"));
    across["deps"] = json!([format!("run:{upstream}#engine")]);
    across["consumes"] = json!({format!("run:{upstream}#engine"): "crate"});
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");
    let run = start(&world, "adoption-crossdag", vec![across]);
    world.until("the consumer's turn to open", |world| {
        !world.events_of(&run, "turn-started").is_empty()
    });

    // The row is the upstream run's, read off its ledger: this run never
    // dispatched the engine and has no settlement of its own to read.
    let task = task_of(&world, "consumer");
    let row = task
        .lines()
        .find(|line| line.starts_with(&format!("| run:{upstream}#engine |")))
        .unwrap_or_else(|| panic!("no row for the cross-DAG dependency:\n{task}"))
        .to_owned();
    let cells: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
    assert_eq!(cells[1], "github.com/owner/engine", "row: {row}");
    assert_eq!(
        cells[2],
        branch_of(&world, &upstream, "engine"),
        "the branch cell is not the branch the upstream published from: {row}"
    );
    assert_eq!(
        cells[3],
        landing_commit(&world, &upstream, "engine"),
        "the commit cell is not the landing the upstream observed: {row}"
    );
    assert_eq!(cells[4], "crate", "row: {row}");

    // And the release it is waiting on reaches it, named by the dependency the
    // plan wrote rather than by a node this graph has.
    releases_at(&answer, "0.2.0");
    world.until("the release to be adopted", |world| {
        !world.events_of(&run, "release-adopted").is_empty()
    });
    let arrived = world.events_of(&run, "release-arrived");
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    assert_eq!(
        arrived[0]["payload"]["dep"],
        json!(format!("run:{upstream}#engine"))
    );
    assert_eq!(arrived[0]["payload"]["version"], json!("0.2.0"));

    world.release("consumer.go");
    world.until("the run to settle", |world| {
        !world.events_of(&run, "node-settled").is_empty()
    });
}

/// A delivery that was attempted and **broke** leaves the note owed rather than
/// recorded, and the node is told once the lever works again.
///
/// The one answer that is neither "it reached the turn" nor "there was no turn to
/// reach": a run that recorded the note as delivered when the lever failed would
/// never try again, and the worker would go on pinning against git with the
/// release out.
#[test]
fn a_delivery_that_broke_leaves_the_note_owed_and_is_tried_again() {
    let world = watching("adoption-lever-broken");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    let broken = world.fakes.join("interrupt.fail");
    world.script("interrupt.fail", "");
    world.script("consumer.turn-open", "");
    world.script("consumer.wait", "hold");
    let run = start(
        &world,
        "adoption-lever-broken",
        vec![engine(), consumer(Some("fast"))],
    );
    world.until("the consumer's turn to open", |world| {
        world
            .events_of(&run, "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == "consumer")
    });

    releases_at(&answer, "0.2.0");
    // The release arrives and the delivery breaks: the arrival is reported, and
    // the adoption is not — because it has not happened.
    world.until("the release to arrive", |world| {
        !world.events_of(&run, "release-arrived").is_empty()
    });
    assert!(
        world.events_of(&run, "release-adopted").is_empty(),
        "a note whose delivery broke was recorded as delivered"
    );

    // Mend the lever, and the note this run still owes is delivered.
    std::fs::remove_file(&broken).expect("the broken lever is mended");
    world.until("the note to be delivered", |world| {
        !world.events_of(&run, "release-adopted").is_empty()
    });
    let adopted = world.events_of(&run, "release-adopted");
    assert_eq!(adopted.len(), 1, "the note was delivered more than once");
    assert_eq!(adopted[0]["payload"]["delivery"], json!("live"));

    world.release("consumer.go");
    world.until("the run to settle", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });
    assert!(redirected(&world, &run, "consumer").contains("crate 0.2.0"));
}

/// A fast-adoption node whose dependency lands in its **own** repository gets no
/// reference block and waits for no release: the lifecycle already puts that
/// dependency's work under it, and nothing here changes that.
///
/// The other half of the fast-adoption promise, and the one that is easy to
/// break: the block exists for work a worker cannot reach from its own branch,
/// and a dependency in the same repository is exactly the work it can.
#[test]
fn a_dependency_inside_the_nodes_own_repository_is_not_pinned_against_git() {
    let world = watching("adoption-same-repo");
    world.write_graphs();
    let repository = world.repository("local-direct", &[]);
    let (script, answer) = world.probe_in(&repository, "service");
    // The consumer's *own* repository releases something, so nothing here is
    // spared a row by there being no release to wait for — which is what would
    // make this journey pass without proving anything.
    world.releases(&document(&script, "").replace("name: engine", "name: service"));
    releases_at(&answer, "0.1.0");
    let declares = world.on_onevcs(|| onevcs::release_targets("service"));
    assert!(
        declares.is_ok_and(|releases| !releases.targets.is_empty()),
        "this journey's own repository declares no release target, so it proves nothing"
    );

    let mut first = lifecycle("first", &[]);
    first["title"] = json!("feat: ship first");
    let mut second = lifecycle("second", &["first"]);
    second["title"] = json!("feat: ship second");
    second["adoption"] = json!("fast");
    let run = start(&world, "adoption-same-repo", vec![first, second]);
    world.until("both nodes to settle", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });

    let task = task_of(&world, "second");
    assert!(
        !task.contains(CROSS_REPO_REFERENCES_HEADING),
        "a dependency in the node's own repository was rendered as a git pin:\n{task}"
    );
    for kind in ["release-wait", "release-arrived", "release-adopted"] {
        assert!(
            world.events_of(&run, kind).is_empty(),
            "a dependency in the node's own repository started a `{kind}`"
        );
    }
    // And both nodes' work reached the base, which is the second having been cut
    // from a base that already carried the first — the stacking this crate has
    // always done, unchanged.
    for node in ["first", "second"] {
        assert!(
            repository.base_file(&format!("{node}.md")).is_some(),
            "{node}'s work did not reach the base"
        );
    }
}

/// A published-adoption node is **not scheduled at all** while its
/// out-of-repository dependency is unreleased, and is started by nothing but an
/// answer of released.
#[test]
fn a_published_node_is_held_until_the_release_answers_and_by_nothing_else() {
    let world = watching("adoption-published");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&automated(&script));
    releases_at(&answer, "0.1.0");

    let run = start(
        &world,
        "adoption-published",
        vec![engine(), consumer(Some("published"))],
    );
    world.until("the engine to settle", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "engine")
    });
    // The wait is surfaced, and goes on being surfaced once the probe has
    // answered: a probe that ran and said the version has not moved is
    // `not-released`, which is an answer and still not a release.
    world.until("the probe's answer to reach the wait", |world| {
        answered(world, &run, "consumer") == Some("not-released".to_owned())
    });

    // Its dependency has settled `done`, so it is ready by every rule the graph
    // has — and it has not been dispatched, because the release has not arrived.
    assert!(
        !dispatched(&world, &run, "consumer"),
        "a published node was dispatched with its dependency unreleased"
    );
    let entries = awaiting(&world, &run, "consumer");
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["identity"], json!("github.com/owner/engine"));
    assert_eq!(entries[0]["target"], json!("crate"));
    assert_eq!(entries[0]["style"], json!("automated"));
    assert!(
        entries[0]["waited_seconds"].is_number() && entries[0]["since"].is_string(),
        "the wait does not say how long it has been: {entries:?}"
    );
    assert!(
        entries[0].get("action").is_none(),
        "an automated wait carries an action nobody has to perform: {entries:?}"
    );
    let surface = wait_surface(&world, &run, "consumer");
    assert!(
        surface.contains("automated release") && surface.contains("waiting on 1 release"),
        "the surface does not name what is awaited or how:\n{surface}"
    );
    assert!(
        surface.contains("Nothing times this out and nothing will fail the node"),
        "the surface does not say the wait is indefinite:\n{surface}"
    );

    // The wait is repeated rather than stated once, so it cannot go silent.
    world.until("the wait to be surfaced again", |world| {
        world
            .events_of(&run, "planner-surface-queued")
            .iter()
            .filter(|event| event["payload"]["kind"] == "release-wait")
            .count()
            > 1
    });
    // And nothing about the elapsed time started it.
    assert!(
        !dispatched(&world, &run, "consumer"),
        "waiting longer is what started a held node"
    );

    // Only the release does.
    releases_at(&answer, "0.2.0");
    world.until("the held node to run", |world| {
        world.events_of(&run, "node-settled").len() == 2
    });
    assert!(dispatched(&world, &run, "consumer"));
    // It launched *after* the release, so it has a version to pin against and no
    // git reference block telling it otherwise.
    let task = task_of(&world, "consumer");
    assert!(
        !task.contains(CROSS_REPO_REFERENCES_HEADING),
        "a node that waited for the release was told it had launched without one:\n{task}"
    );
    let settled = world.events_of(&run, "node-settled");
    for event in &settled {
        assert_ne!(
            event["payload"]["status"],
            json!("failed"),
            "a node the wait held was failed: {event}"
        );
    }
}

/// The adoption mode resolves through **exactly four rungs**, and each of them
/// decides a node the rung beneath it would have decided differently.
///
/// Driven as behaviour rather than as a lookup, because the mode is not a value
/// anything reports: what a rung decides is whether the node is scheduled. So
/// each rung is proved by a pair — one node it holds beside one the next rung
/// down would have let go, and the other way round.
#[test]
fn the_adoption_mode_resolves_through_exactly_four_rungs() {
    let world = watching("adoption-rungs");
    world.write_graphs();
    let consumer_repo = world.repository("local-direct", &[]);
    let engine_repo = world.extra_repository(ENGINE);
    let unruled = world.extra_repository("tool");
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    // Rung 3, the global one, says `published`. Rung 2 says `fast` for the
    // `service` repository and says nothing at all for `tool`, which is what
    // leaves `tool` on rung 3.
    world.releases(&format!(
        "{}  - match: {{host: github.com, owner: owner, name: service}}\n\
         \x20   adoption: fast\n\
         default:\n\
         \x20 adoption: published\n",
        repositories(&script),
    ));
    releases_at(&answer, "0.1.0");

    // Rung 1 — the node's own field — against rung 4, the floor: two nodes with
    // no repository at all, one stating `published` and one stating nothing.
    let mut stated = crate::harness::agent("stated", &[ENGINE]);
    stated["adoption"] = json!("published");
    let floor = crate::harness::agent("floor", &[ENGINE]);
    // Rung 2 against rung 3: one node in the repository a rule names `fast`, and
    // one in a repository no rule names, which takes the global `published`.
    let mut by_repository = lifecycle("by-repository", &[ENGINE]);
    by_repository["title"] = json!("feat: ship by-repository");
    let mut by_global = lifecycle("by-global", &[ENGINE]);
    by_global["repo"] = json!("tool");
    by_global["title"] = json!("feat: ship by-global");

    let run = start(
        &world,
        "adoption-rungs",
        vec![engine(), stated, floor, by_repository, by_global],
    );
    world.until("the two waits to carry their own answer", |world| {
        answered(world, &run, "stated") == Some("not-released".to_owned())
            && answered(world, &run, "by-global") == Some("not-released".to_owned())
    });

    // The floor let a node go that the global rung above it would have held, and
    // the node's own field held one the floor would have let go.
    assert!(
        dispatched(&world, &run, "floor"),
        "rung 4 did not decide a node with no repository and no field of its own"
    );
    assert!(
        !dispatched(&world, &run, "stated"),
        "rung 1 did not win over the floor beneath it"
    );
    // The repository rung let a node go that the global rung would have held.
    assert!(
        dispatched(&world, &run, "by-repository"),
        "rung 2 did not win over rung 3"
    );
    assert!(
        !dispatched(&world, &run, "by-global"),
        "rung 3 did not decide a node no rule names"
    );

    releases_at(&answer, "0.2.0");
    world.until("every node to settle", |world| {
        world.events_of(&run, "node-settled").len() == 5
    });
    let _ = (consumer_repo, unruled);
}

/// Both release styles, side by side, through the sibling's own interface: one
/// node awaiting an automated target whose real probe answers, and one awaiting a
/// human-step target that only a person's acknowledgement can answer.
///
/// The point is what is *the same* — one hold, indefinite, neither failing nor
/// timing out — and what differs: where the readiness answer is obtained, and
/// what is reported.
#[test]
fn the_two_release_styles_take_one_scheduling_path_and_are_reported_apart() {
    let world = watching("adoption-styles");
    world.write_graphs();
    let (engine_repo, _consumer) = two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, ENGINE);
    world.releases(&both_styles(&script));
    releases_at(&answer, "0.1.0");

    let mut on_the_wheel = consumer(Some("published"));
    on_the_wheel["id"] = json!("packager");
    on_the_wheel["title"] = json!("feat: ship packager");
    on_the_wheel["consumes"] = json!({"engine": "wheel"});
    let run = start(
        &world,
        "adoption-styles",
        vec![engine(), consumer(Some("published")), on_the_wheel],
    );
    world.until("both waits to carry their own answer", |world| {
        answered(world, &run, "consumer") == Some("not-released".to_owned())
            && answered(world, &run, "packager") == Some("awaiting-human-step".to_owned())
    });

    // The same hold: neither is dispatched, and neither is failed.
    for node in ["consumer", "packager"] {
        assert!(!dispatched(&world, &run, node), "{node} was dispatched");
    }

    // Told apart by the answer each obtained, and by what each reports.
    let automated = awaiting(&world, &run, "consumer");
    assert_eq!(automated[0]["style"], json!("automated"));
    assert!(automated[0].get("action").is_none());

    let human = awaiting(&world, &run, "packager");
    assert_eq!(human[0]["style"], json!("human-step"));
    assert_eq!(human[0]["target"], json!("wheel"));
    assert_eq!(
        human[0]["action"],
        json!(ACTION),
        "the wait does not carry the text the person needs"
    );

    let surface = wait_surface(&world, &run, "packager");
    assert!(
        surface.contains("human-step release") && surface.contains(ACTION),
        "the surface does not say a person has to act, or what they have to do:\n{surface}"
    );
    assert!(
        !wait_surface(&world, &run, "consumer").contains("human-step"),
        "an automated wait reads as one somebody has to act on"
    );

    // **No probe ran for the human-step target**, and the sibling's own
    // `release-probed` — relayed unchanged, like every other `onevcs` kind — is
    // the evidence: the publication's baseline capture probed the automated
    // target and nothing else.
    let probed = world.events_of(&run, "release-probed");
    assert!(!probed.is_empty(), "no probe was relayed at all");
    for event in &probed {
        assert_eq!(
            event["source"],
            json!("vcs"),
            "a relayed kind was rewritten"
        );
        assert_eq!(
            event["payload"]["target"],
            json!("crate"),
            "a probe was run for a target that has none: {event}"
        );
    }

    // The automated one is answered by its probe.
    releases_at(&answer, "0.2.0");
    world.until("the automated wait to end", |world| {
        dispatched(world, &run, "consumer")
    });
    assert!(
        !dispatched(&world, &run, "packager"),
        "the human-step wait ended when the automated one did"
    );

    // The human-step one is answered by the real acknowledge operation, run the
    // way the person who performed the release runs it.
    let landed = branch_of(&world, &run, "engine");
    world.on_onevcs(|| {
        onevcs::acknowledge_release(
            &landed,
            &"wheel".parse().expect("a target name"),
            "1.0.0",
            false,
        )
        .expect("the release is acknowledged")
    });
    world.until("the human-step wait to end", |world| {
        dispatched(world, &run, "packager")
    });

    world.until("the run to settle", |world| {
        world.events_of(&run, "node-settled").len() == 3
    });
    for event in world.events_of(&run, "node-settled") {
        assert_ne!(
            event["payload"]["status"],
            json!("failed"),
            "a node one of the two waits held was failed: {event}"
        );
    }
    let arrived: Vec<Value> = world.events_of(&run, "release-arrived");
    assert!(
        arrived
            .iter()
            .any(|event| event["payload"]["style"] == json!("human-step")
                && event["payload"]["version"] == json!("1.0.0")),
        "the human-step release was not reported as one: {arrived:?}"
    );
}

/// The branch one node's work was published from, as its settlement recorded it.
///
/// The spelling `onevcs` resolves landed work by, which is what a person's own
/// `onevcs release acknowledge` is given.
fn branch_of(world: &World, run: &str, node: &str) -> String {
    world
        .events_of(run, "node-settled")
        .into_iter()
        .find(|event| event["labels"]["node"] == node)
        .and_then(|event| event["payload"]["branch"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("nothing recorded which branch {node} published from"))
}

/// The commit one node's change reached its base at, as the run observed it.
fn landing_commit(world: &World, run: &str, node: &str) -> String {
    world
        .journal(run)
        .into_iter()
        .find(|event| {
            event["source"] == "vcs"
                && event["kind"] == "merge-completed"
                && event["labels"]["node"] == node
        })
        .and_then(|event| event["payload"]["sha"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("nothing recorded where {node}'s work landed"))
}
