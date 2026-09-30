//! Every agent a run launches is visible through oneharness's own run history,
//! with **nothing configured in the target repository** — against the real
//! `oneagentgraph` and the real linked `oneharness-core`.
//!
//! What these journeys hold is the property `src/agents.rs` exists for: a run
//! finds every oneharness session under it through one small file of its own,
//! whatever the agent structure of the repository — a two-party member whose
//! turns are spawned `oneharness` processes, a single-sided member whose turn
//! is an in-process library call, a nested tool a worker runs that is itself an
//! oneharness run — and the transcripts stay wherever each oneharness put them.

// llmlint: ignore-file[e2e_not_mocked] the layer under test is the environment the
// engine overlays on a dispatch and the pointer file every oneharness under it
// appends to, and both halves are real here: the sibling's own compiled binary
// resolves the graph and runs each member, a single-sided member's turn goes through
// the real linked `oneharness-core`, and the store and the pointer file are that
// library's own writer's. What stands in is the innermost layer — the paid model turn
// at `ONEHARNESS_BIN_CLAUDE_CODE`, and, for a two-party member, the `oneharness`
// process onejudge spawns per turn, at `ONEAGENTGRAPH_ONEHARNESS_BIN` — and that
// double resolves its history through the same library, from the same environment,
// as the real CLI does.

use std::path::{Path, PathBuf};

use crate::harness::{agent, human, plan_of, World};
use oneharness_core::domain::history::HistoryPointer;
use oneharness_core::io::history::{find_session_path, read_pointers};
use onepipeline::agents::{
    Scope, ATTEMPT_LABEL, HISTORY_DIR_ENV, HISTORY_ENV, LABELS_ENV, NODE_LABEL, POINTER_FILE_ENV,
    PROJECT_LABEL, RUN_ID_LABEL, SCOPE_LABEL, SESSIONS_FILE, STEP_LABEL,
};
use serde_json::{json, Value};

/// The run's pointer file, read through the linked library's own reader — the
/// same read `onepipeline agents` makes — with every line whole.
fn pointers(world: &World, run: &str) -> Vec<HistoryPointer> {
    let file = world.run_file(run, SESSIONS_FILE);
    let read = read_pointers(&file)
        .unwrap_or_else(|error| panic!("{} does not read: {error}", file.display()));
    assert_eq!(
        read.skipped,
        0,
        "the pointer file carries a line the reader could not read: {}",
        std::fs::read_to_string(&file).unwrap_or_default()
    );
    assert!(
        !read.pointers.is_empty(),
        "no oneharness under {run} appended a pointer line: {}",
        world.dump()
    );
    read.pointers
}

/// One label of a line, or none.
fn label<'a>(pointer: &'a HistoryPointer, key: &str) -> Option<&'a str> {
    pointer.labels().as_map().get(key).map(String::as_str)
}

/// The lines whose `onepipeline.scope` is `scope`.
fn scoped(pointers: &[HistoryPointer], scope: Scope) -> Vec<&HistoryPointer> {
    pointers
        .iter()
        .filter(|pointer| label(pointer, SCOPE_LABEL) == Some(scope.as_str()))
        .collect()
}

/// The three-field reference every line carries resolves, through the same
/// call the `oneharness_session` reference is resolved with, to the session
/// file the line names — and that file exists in the store the line names.
fn resolves(pointer: &HistoryPointer) -> PathBuf {
    let file = find_session_path(
        Path::new(pointer.history_dir()),
        Some(pointer.history_project()),
        pointer.history_session(),
    )
    .unwrap_or_else(|error| panic!("the store {} does not read: {error}", pointer.history_dir()))
    .unwrap_or_else(|| {
        panic!(
            "the store {} holds no session {} under {}",
            pointer.history_dir(),
            pointer.history_session(),
            pointer.history_project()
        )
    });
    assert_eq!(
        file,
        Path::new(pointer.history_file()),
        "the resolved session is not the file the line names"
    );
    assert!(file.is_file(), "{} is not a file", file.display());
    file
}

/// What a session's lines under a run all say: the run, the project the launch
/// record names, and a store that resolves.
fn every_line_names(world: &World, run: &str, pointers: &[HistoryPointer]) {
    let project = world.run_json(run, "launch.json")["project"]
        .as_str()
        .expect("a launched run names its project")
        .to_string();
    for pointer in pointers {
        assert_eq!(label(pointer, RUN_ID_LABEL), Some(run), "{pointer:?}");
        assert_eq!(
            label(pointer, PROJECT_LABEL),
            Some(project.as_str()),
            "{pointer:?}"
        );
        assert_eq!(pointer.harness_id(), "claude-code", "{pointer:?}");
        resolves(pointer);
    }
}

/// A lifecycle node with one declared step, so a step's dispatch is one of the
/// launches under test.
fn stepped(id: &str) -> Value {
    json!({
        "id": id,
        "repo": "service",
        "title": format!("feat: ship {id}"),
        "steps": [
            {"id": "implement", "persona": "engineer", "task": format!("## What\nImplement {id}.\n\n## Acceptance criteria\n- {id} is implemented.")},
        ],
    })
}

/// The harness process's own environment, as the in-process turns recorded it:
/// `(prompt, name) -> state`, off `turn.report-env`.
fn handed(world: &World, prompt_carries: &str, name: &str) -> (String, String) {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "turn-env")
        .find(|row| {
            row["args"][0]
                .as_str()
                .is_some_and(|prompt| prompt.contains(prompt_carries))
                && row["args"][1] == name
        })
        .map(|row| {
            (
                row["args"][2].as_str().unwrap_or_default().to_string(),
                row["args"][3].as_str().unwrap_or_default().to_string(),
            )
        })
        .unwrap_or_else(|| {
            panic!("no turn whose prompt carries {prompt_carries:?} reported {name}")
        })
}

/// Over a repository that configures nothing, every dispatch the engine starts —
/// a node-scope dispatch, a lifecycle step, the observer graph and the drafting
/// graph — appends its harness runs to the run's pointer file with the engine's
/// labels, the spawned double's turns and the in-process turns alike; the store
/// each line names is the one the process resolved, never one the engine chose;
/// a nested tool a worker runs is visible the same way, and one run with
/// `--no-history` leaves no line; and `onepipeline agents` lists the sessions
/// per run, per node, and per project across two runs of one project.
#[test]
fn every_dispatch_of_a_run_is_visible_through_its_pointer_file_with_nothing_configured() {
    let world = World::new("agents-visible");
    world.write_graphs();
    // A two-party worker, so the node-scope turns are spawned `oneharness`
    // processes — the shipped graph's shape — while the observer and the
    // drafter stay single-sided and in-process.
    world.write_supervised_node_graph();
    world.repository("change-open", &[]);
    world.script("harness.work", "the worker wrote this");
    world.script("harness.body", "## What\nRead off the diff.");
    // A worker that runs two nested oneharness turns of its own: one opted out
    // on the CLI, one saying nothing and so taking the launch's environment.
    world.script(
        "harness.nested",
        "--no-history --history-name nested-off\n--history-name nested-on\n",
    );
    // What the in-process harness turns were handed, off the harness process.
    world.script(
        "turn.report-env",
        &format!("{HISTORY_ENV}\n{HISTORY_DIR_ENV}\n{POINTER_FILE_ENV}\n{LABELS_ENV}\n"),
    );
    // The property under test, stated up front: no history configuration
    // anywhere the launch reads — not the launch directory, not the repository.
    for dir in [world.project.clone(), world.root.join("service")] {
        for file in ["oneharness.toml", ".oneharness.toml"] {
            assert!(
                !dir.join(file).exists(),
                "{} carries a oneharness config, which is the case this journey is not about",
                dir.join(file).display()
            );
        }
    }
    let drafting = world.pr_author_graph();
    let run = "visible";
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), stepped("service")]),
    );
    world
        .run_on_agentgraph(&[
            "start",
            &path,
            "--attach",
            "--dag-graph",
            &world.dag_graph(),
            "--pr-author-graph",
            &drafting,
        ])
        .exited(0)
        .settled();
    let opened = world.changes_opened();
    assert_eq!(opened.len(), 1, "{opened:?}\n{}", world.dump());

    // The file is where the record and the summary say it is.
    let file = world.run_file(run, SESSIONS_FILE);
    assert_eq!(
        world.run_json(run, "launch.json")["oneharness_sessions"],
        json!(file.display().to_string()),
        "the launch record does not name the pointer file"
    );
    assert_eq!(
        world.run_json(run, "summary.json")["oneharness_sessions"],
        json!(file.display().to_string()),
        "the summary does not carry the pointer file"
    );
    let lines = pointers(&world, run);
    every_line_names(&world, run, &lines);

    // The store every line names is the platform default the process resolved
    // under this world's own state directory — the engine set none.
    let store = std::fs::canonicalize(world.history_store())
        .expect("the store the processes resolved exists");
    for pointer in &lines {
        assert_eq!(
            Path::new(pointer.history_dir()),
            store,
            "a line names a store nobody in this world configured: {pointer:?}"
        );
    }

    // The node-scope dispatch: the spawned double's turns — both sides of the
    // conversation — under the node, attempt 1, no step.
    let build: Vec<&HistoryPointer> = scoped(&lines, Scope::Node)
        .into_iter()
        .filter(|pointer| label(pointer, NODE_LABEL) == Some("build"))
        .collect();
    assert!(
        build.len() >= 3,
        "a two-party dispatch runs at least an agent turn, a supervisor turn and a verdict, \
         and {} line(s) name build: {lines:?}",
        build.len()
    );
    for pointer in &build {
        assert_eq!(label(pointer, ATTEMPT_LABEL), Some("1"), "{pointer:?}");
        assert_eq!(label(pointer, STEP_LABEL), None, "{pointer:?}");
    }
    // The nested tool the worker ran: the run that said nothing took the
    // launch's environment and is on the file under the node; the one that
    // opted out on the CLI ran — the double recorded its exit — and left none.
    let nested: Vec<Value> = world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneharness-nested")
        .collect();
    for wanted in ["nested-off", "nested-on"] {
        assert!(
            nested.iter().any(|row| row["args"][0]
                .as_str()
                .is_some_and(|extra| extra.contains(wanted))
                && row["args"][1] == "0"),
            "the worker's nested {wanted} run did not run or did not exit 0: {nested:?}"
        );
    }
    assert!(
        build.iter().any(|pointer| pointer.name() == "nested-on"),
        "the nested run that took the launch's environment left no line: {lines:?}"
    );
    assert!(
        !lines.iter().any(|pointer| pointer.name() == "nested-off"),
        "the nested run that opted out with --no-history left a line: {lines:?}"
    );

    // The lifecycle step: under the node and the step, attempt 1.
    let step: Vec<&HistoryPointer> = scoped(&lines, Scope::Node)
        .into_iter()
        .filter(|pointer| label(pointer, NODE_LABEL) == Some("service"))
        .collect();
    assert!(step.len() >= 3, "{lines:?}");
    for pointer in &step {
        assert_eq!(label(pointer, STEP_LABEL), Some("implement"), "{pointer:?}");
        assert_eq!(label(pointer, ATTEMPT_LABEL), Some("1"), "{pointer:?}");
    }

    // The drafting graph: the in-process single-sided turn, under the node,
    // the node's own attempt, and no step.
    let drafted = scoped(&lines, Scope::PrAuthor);
    assert_eq!(drafted.len(), 1, "{lines:?}");
    assert_eq!(label(drafted[0], NODE_LABEL), Some("service"));
    assert_eq!(label(drafted[0], ATTEMPT_LABEL), Some("1"));
    assert_eq!(label(drafted[0], STEP_LABEL), None);

    // The observer graph: the in-process turn under the observer's scope, with
    // no node, no step and no attempt.
    let observed = scoped(&lines, Scope::Observer);
    assert!(!observed.is_empty(), "{lines:?}");
    for pointer in &observed {
        for absent in [NODE_LABEL, STEP_LABEL, ATTEMPT_LABEL] {
            assert_eq!(label(pointer, absent), None, "{pointer:?}");
        }
    }
    assert_eq!(
        lines.len(),
        build.len() + step.len() + drafted.len() + observed.len(),
        "a line carries a scope this journey does not know: {lines:?}"
    );

    // What the in-process harness processes were handed: history on, the run's
    // own pointer file, and **no** store of the engine's.
    for prompt in ["Observe this run", "Read this branch's diff"] {
        assert_eq!(
            handed(&world, prompt, HISTORY_ENV),
            ("set".into(), "1".into())
        );
        assert_eq!(
            handed(&world, prompt, POINTER_FILE_ENV),
            ("set".into(), file.display().to_string())
        );
        assert_eq!(handed(&world, prompt, HISTORY_DIR_ENV).0, "unset");
    }

    // The verb, per run and per node: every session, then a node's.
    let listed = world.run_on_agentgraph(&["agents", run]);
    listed.exited(0);
    for pointer in &lines {
        listed.out_has(pointer.history_session());
    }
    listed.out_has("labels ");
    let one = world.run_on_agentgraph(&["agents", run, "build"]);
    one.exited(0);
    for pointer in &build {
        one.out_has(pointer.history_session());
    }
    for pointer in step.iter().chain(&drafted).chain(&observed) {
        assert!(
            !one.stdout.contains(pointer.history_session()),
            "a node's listing carries a session of another launch: {}",
            one.stdout
        );
    }

    // A second run of the same project, launched with a store and a label set
    // of its own in the environment: the store is honoured — the engine set
    // none — the repository's label survives beside the engine's keys, and a
    // stale engine key the launch inherited is replaced.
    let own_store = world.root.join("own-store");
    // The same project, re-planned with two direct nodes: the second stands
    // where the lifecycle node stood, so the plan reads clean of what the
    // first run wrote back onto it.
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), agent("service", &["build"])]),
    );
    let mut command = world.agentgraph_cmd(&["start", &path, "--attach"]);
    command
        .env(HISTORY_DIR_ENV, &own_store)
        .env(LABELS_ENV, "owner=ci,onepipeline.node=other");
    world
        .run_on(command, "start visible-2 inheriting a store")
        .exited(0)
        .settled();
    let again = "visible-2";
    let inherited = pointers(&world, again);
    every_line_names(&world, again, &inherited);
    let own_store = std::fs::canonicalize(&own_store).expect("the inherited store was written");
    for pointer in &inherited {
        assert_eq!(
            Path::new(pointer.history_dir()),
            own_store,
            "the launch did not land its sessions under the store it inherited: {pointer:?}"
        );
        assert_eq!(label(pointer, "owner"), Some("ci"), "{pointer:?}");
        assert!(
            matches!(label(pointer, NODE_LABEL), Some("build" | "service")),
            "{pointer:?}"
        );
        assert!(
            !pointer
                .labels()
                .as_map()
                .values()
                .any(|value| value == "other"),
            "the stale engine key the launch inherited survived: {pointer:?}"
        );
    }

    // And per project, across both runs of it.
    let project = world.run_json(run, "launch.json")["project"]
        .as_str()
        .expect("the run names its project")
        .to_string();
    assert_eq!(
        world.run_json(again, "launch.json")["project"],
        json!(project),
        "the second launch is not of the same project"
    );
    let across = world.run_on_agentgraph(&["agents", "--project", &project]);
    across.exited(0);
    for pointer in lines.iter().chain(&inherited) {
        across.out_has(pointer.history_session());
    }
}

/// A dispatch asked again carries the attempt its `node-dispatched` records: a
/// publication whose checks the host rejects is re-dispatched on the branch it
/// preserved, and that dispatch's lines say `2` where the first said `1`.
#[test]
fn a_re_asked_dispatchs_lines_carry_the_attempt_the_record_names() {
    let world = World::new("agents-reasked");
    world.write_graphs();
    world.write_supervised_node_graph();
    // `change-auto` watches the host's checks to their conclusion, which is
    // where a red one is observed at all.
    world.repository("change-auto", &[]);
    world.script("harness.work", "the worker wrote this\n");
    world.script("gh.checks", "llmlint completed failure required");
    let run = "reasked";
    let path = world.plan(run, &plan_of(run, vec![stepped("service")]));
    world
        .run_on_agentgraph(&["start", &path, "--detach"])
        .exited(0);
    world.until("the host to report its check red", |world| {
        world
            .events_of(run, "change-check")
            .iter()
            .any(|event| event["payload"]["conclusion"] == "failure")
    });
    // CI ran again and nothing blocks the merge, so the host lands what the
    // next attempt pushes. And the next attempt *changes* the branch it was
    // handed, as its task tells it to — "republishing it unaltered meets the
    // same refusal" — which is also what keeps this journey about the attempt
    // number: a retry that rewrote the very bytes the first attempt committed
    // leaves a tree `git status` reads as touched on Windows, where a rewrite
    // refreshes the stat data, and `git commit` reads as clean, and `onevcs`'s
    // preserve refuses on the gap between the two.
    std::fs::remove_file(world.fakes.join("gh.checks")).expect("the red check is cleared");
    world.script("gh.merged", "");
    world.script("harness.work", "the worker fixed what the check named\n");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    let result = world.run_json(run, "result.json");
    assert_eq!(
        result["nodes"][0]["status"],
        "done",
        "{result}\n{}",
        world.dump()
    );
    let dispatched: Vec<Value> = world
        .events_of(run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "service")
        .collect();
    assert!(dispatched.len() >= 2, "{dispatched:?}\n{}", world.dump());
    assert_eq!(dispatched[1]["payload"]["attempt"], 2, "{}", dispatched[1]);
    // Each attempt left a commit of its own on the branch: the second changed
    // what it was handed rather than rewriting it.
    let preserved = world.events_of(run, "commit-preserved").len();
    assert_eq!(
        preserved,
        dispatched.len(),
        "an attempt preserved no commit of its own\n{}",
        world.dump()
    );

    let lines = pointers(&world, run);
    every_line_names(&world, run, &lines);
    let attempts = |attempt: &str| -> Vec<&HistoryPointer> {
        lines
            .iter()
            .filter(|pointer| label(pointer, ATTEMPT_LABEL) == Some(attempt))
            .collect()
    };
    assert!(
        !attempts("1").is_empty() && !attempts("2").is_empty(),
        "the two dispatches' lines do not carry their attempts: {lines:?}"
    );
    for pointer in attempts("2") {
        assert_eq!(label(pointer, NODE_LABEL), Some("service"), "{pointer:?}");
        assert_eq!(label(pointer, STEP_LABEL), Some("implement"), "{pointer:?}");
        assert_eq!(label(pointer, SCOPE_LABEL), Some(Scope::Node.as_str()));
    }
    world
        .run_on_agentgraph(&["agents", run, "service"])
        .exited(0)
        .out_has(&format!("{ATTEMPT_LABEL}=2"));
}

/// A run whose launch record predates the field is told where its sessions go
/// by the adoption that dispatches for it: the record names the pointer file
/// from then on, and the dispatches the adopting driver makes are listed off
/// it like any other.
#[test]
fn an_adopted_run_whose_record_predates_the_field_names_the_file_and_lists_its_dispatches() {
    let world = World::new("agents-adopted");
    world.write_graphs();
    let run = "adopted";
    let path = world.plan(
        run,
        &plan_of(
            run,
            vec![human("approve", &[]), agent("ship", &["approve"])],
        ),
    );
    // Settled at the human gate with the driver gone, then attested: the state
    // an `adopt` picks up with work still to do.
    world
        .run_on_agentgraph(&["start", &path, "--attach"])
        .exited(0);
    world
        .run_on_agentgraph(&["attest", run, "approve"])
        .exited(0);
    assert!(
        !world.run_file(run, SESSIONS_FILE).exists(),
        "a run that dispatched nothing yet has a pointer file: {}",
        world.dump()
    );
    // llmlint: ignore-block[tests_mirror_real_usage] no verb writes a launch record
    // naming no pointer file — every launch this build makes names one — so the state is
    // one an *earlier build* left, and it is reached by taking the one key that build
    // never wrote off a record this build wrote. Every claim after it is read off the
    // compiled binary's own records and stdout.
    let launch = world.run_file(run, "launch.json");
    let mut record: Value =
        serde_json::from_str(&std::fs::read_to_string(&launch).expect("the launch record reads"))
            .expect("a launch record");
    assert!(
        record
            .as_object_mut()
            .expect("a launch record is an object")
            .remove("oneharness_sessions")
            .is_some(),
        "the launch did not record the pointer file"
    );
    std::fs::write(&launch, record.to_string()).expect("the launch record is rewritten");
    // llmlint: ignore-end[tests_mirror_real_usage]

    world.run_on_agentgraph(&["adopt", run]).exited(0).settled();
    let file = world.run_file(run, SESSIONS_FILE);
    assert_eq!(
        world.run_json(run, "launch.json")["oneharness_sessions"],
        json!(file.display().to_string()),
        "the adoption did not fill in the pointer file"
    );
    let lines = pointers(&world, run);
    every_line_names(&world, run, &lines);
    let shipped: Vec<&HistoryPointer> = scoped(&lines, Scope::Node)
        .into_iter()
        .filter(|pointer| label(pointer, NODE_LABEL) == Some("ship"))
        .collect();
    assert!(!shipped.is_empty(), "{lines:?}");
    for pointer in &shipped {
        assert_eq!(label(pointer, ATTEMPT_LABEL), Some("1"), "{pointer:?}");
    }
    let listed = world.run_on_agentgraph(&["agents", run, "ship"]);
    listed.exited(0);
    for pointer in &shipped {
        listed.out_has(pointer.history_session());
    }
}

/// A pointer file is append-only and shared by every oneharness under the
/// run, so the two things a reader meets that are not a pointer are real: the
/// torn tail an interrupted writer left mid-line, and a line something else
/// wrote into the file. The verb reads past both, lists every session it can,
/// and says how many lines it could not read — and a file it cannot read at
/// all is a refusal naming the file, never an empty listing.
#[test]
fn the_verb_reads_past_a_torn_tail_and_a_foreign_line_and_refuses_a_file_it_cannot_read() {
    let world = World::new("agents-torn");
    world.write_graphs();
    world.script("harness.work", "the worker wrote this\n");
    let run = "torn";
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&["start", &path, "--attach"])
        .exited(0)
        .settled();
    let lines = pointers(&world, run);
    every_line_names(&world, run, &lines);

    // An oneharness interrupted mid-write leaves the start of a line with no
    // newline; a process that took the file for something else leaves a line
    // that is JSON and not a pointer.
    let file = world.run_file(run, SESSIONS_FILE);
    let mut text = std::fs::read_to_string(&file).expect("the pointer file reads");
    text.push_str("{\"schema_version\":\"1.0\",\"history_id\":\"not a pointer\"}\n");
    text.push_str("{\"schema_version\":\"1.0\",\"history_");
    std::fs::write(&file, text).expect("the pointer file is appended to");

    let listed = world.run_on_agentgraph(&["agents", run]);
    listed.exited(0);
    for pointer in &lines {
        listed.out_has(pointer.history_session());
    }
    listed.out_has("2 line(s) skipped: torn or not a pointer");
    let one = world.run_on_agentgraph(&["agents", run, "build"]);
    one.exited(0);
    for pointer in &lines {
        one.out_has(pointer.history_session());
    }
    // The count is the file's, whatever the node asked for.
    one.out_has("2 line(s) skipped: torn or not a pointer");

    // The store the line names is still where every listed session is.
    for pointer in &lines {
        assert!(resolves(pointer).is_file(), "{pointer:?}");
    }

    // A pointer file the reader cannot open is not a run with no sessions.
    std::fs::remove_file(&file).expect("the pointer file is removed");
    std::fs::create_dir(&file).expect("something else takes its path");
    let refused = world.run_on_agentgraph(&["agents", run]);
    refused.exited(crate::harness::REFUSED);
    refused.err_has("cannot read the run's pointer file");
    refused.err_has(SESSIONS_FILE);
    refused.out_lacks("no sessions recorded");
}

/// Recording a turn reads no history index and walks no store, and reading the
/// run's sessions back opens only what its pointer lines name — the property
/// `oneharness-core` 0.24.0 is linked for. A turn on a host records through
/// whichever core the binary under it links, so this drives the real binary on
/// a plan whose node runs a turn with history on, into a store built to make
/// any scan observable: legacy `.index.jsonl` and `.event-index.jsonl` present
/// and unopenable (mode `000`), over a thousand other sessions' files under
/// several project directories, and today's segments already long. The
/// process tree runs under `strace`, and every open, listing and read that
/// lands in the store is held to what the history contract permits.
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] about 40 seconds for both, and what they guard is which `oneharness-core` the crate links — a `Cargo.lock` move, which only this crate's own project sees — so an edge narrower than this target would drop them out of `nx affected` for exactly the change they exist to catch. Same grounds as the `mod agents` declaration in `main.rs`.
#[cfg(target_os = "linux")]
mod unscanned {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::SystemTime;

    use super::{every_line_names, pointers};
    use crate::harness::{agent, plan_of, World};
    use oneharness_core::domain::history::HistoryPointer;
    use oneharness_core::domain::history_index::{
        HistoryIndexEntry, SegmentKind, UtcDate, INDEX_DIR, LEGACY_EVENT_INDEX_FILE,
        LEGACY_INDEX_FILE,
    };
    use oneharness_core::io::history::read_session;
    use onepipeline::agents::HISTORY_DIR_ENV;

    /// How many other sessions the store holds before the run, spread over
    /// [`PROJECTS`] project directories.
    const OTHER_SESSIONS: usize = 1_200;
    const PROJECTS: usize = 6;
    /// What a recording writer may read of a segment through one descriptor:
    /// its last byte, to close off a torn tail (oneharness
    /// `docs/history-index.md`).
    const TRAILING_BYTES: u64 = 1;
    /// The lock an older core took around its index. The new one never does.
    const INDEX_LOCK: &str = ".index.lock";

    /// A file's observable state, compared before and after.
    #[derive(Debug, PartialEq, Eq)]
    struct Seen {
        len: u64,
        modified: SystemTime,
        mode: u32,
    }

    fn seen(path: &Path) -> Seen {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::symlink_metadata(path)
            .unwrap_or_else(|error| panic!("{} does not stat: {error}", path.display()));
        Seen {
            len: meta.len(),
            modified: meta.modified().expect("the platform reports mtimes"),
            mode: meta.permissions().mode(),
        }
    }

    fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .unwrap_or_else(|error| panic!("{} does not chmod: {error}", path.display()));
    }

    /// The store as a host that has run for a long time leaves it.
    struct Store {
        root: PathBuf,
        /// Every other session's file, with its bytes.
        others: BTreeMap<PathBuf, (Seen, Vec<u8>)>,
        /// The legacy indexes, with the bytes written into them.
        legacy: Vec<(PathBuf, Seen, Vec<u8>)>,
        /// Today's and tomorrow's segments as they stood before the run.
        segments: BTreeMap<PathBuf, Vec<u8>>,
    }

    impl Store {
        fn build(root: &Path) -> Self {
            std::fs::create_dir_all(root).expect("the store directory is made");
            let root = std::fs::canonicalize(root).expect("the store resolves");
            let mut others = BTreeMap::new();
            for n in 0..OTHER_SESSIONS {
                let project = root.join(format!("-srv-other-project-{}", n % PROJECTS));
                std::fs::create_dir_all(&project).expect("a project directory is made");
                let file = project.join(format!("other-{n}-20250101T000000Z-{n}.jsonl"));
                let bytes = format!(
                    "{{\"schema_version\":\"1.0\",\"session\":\"other-{n}\",\"note\":\"another session's line\"}}\n"
                )
                .into_bytes();
                std::fs::write(&file, &bytes).expect("another session's file is written");
                others.insert(file, bytes);
            }
            let others = others
                .into_iter()
                .map(|(file, bytes)| {
                    let state = seen(&file);
                    (file, (state, bytes))
                })
                .collect();
            let mut legacy = Vec::new();
            for name in [LEGACY_INDEX_FILE, LEGACY_EVENT_INDEX_FILE] {
                let file = root.join(name);
                let bytes = "{\"legacy\":\"an older core's index line\"}\n"
                    .repeat(4_096)
                    .into_bytes();
                std::fs::write(&file, &bytes).expect("a legacy index is written");
                chmod(&file, 0o000);
                let state = seen(&file);
                legacy.push((file, state, bytes));
            }
            // Today's segments (and tomorrow's, for a run that crosses UTC
            // midnight) already long, so a writer that read one whole reads
            // far more than its last byte.
            let today = UtcDate::from_epoch_secs(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("the clock is past the epoch")
                    .as_secs()
                    .try_into()
                    .expect("the clock fits"),
            );
            let dir = root.join(INDEX_DIR);
            std::fs::create_dir_all(&dir).expect("the segment directory is made");
            let mut segments = BTreeMap::new();
            for date in [today, today.add_days(1)] {
                for kind in [SegmentKind::Runs, SegmentKind::Events] {
                    let file = dir.join(kind.file_name(date));
                    let bytes = "{\"filler\":\"an earlier run's entry\"}\n"
                        .repeat(8_192)
                        .into_bytes();
                    std::fs::write(&file, &bytes).expect("a segment is written");
                    segments.insert(file, bytes);
                }
            }
            assert!(!root.join(INDEX_LOCK).exists());
            Store {
                root,
                others,
                legacy,
                segments,
            }
        }

        /// Nothing that was here before the run changed: the legacy indexes,
        /// every other session, no lock, and each segment only grew by
        /// appended lines.
        fn untouched(&self) {
            assert!(
                !self.root.join(INDEX_LOCK).exists(),
                "{INDEX_LOCK} was created"
            );
            for (file, before, bytes) in &self.legacy {
                assert_eq!(&seen(file), before, "{} changed", file.display());
                chmod(file, 0o600);
                let after = std::fs::read(file).expect("the legacy index reads");
                chmod(file, 0o000);
                assert!(&after == bytes, "{}'s bytes changed", file.display());
            }
            for (file, (before, bytes)) in &self.others {
                assert_eq!(&seen(file), before, "{} changed", file.display());
                let after = std::fs::read(file).expect("another session's file reads");
                assert!(&after == bytes, "{}'s bytes changed", file.display());
            }
            for (file, bytes) in &self.segments {
                let after = std::fs::read(file).expect("a segment reads");
                assert!(
                    after.starts_with(bytes),
                    "{} was rewritten rather than appended to",
                    file.display()
                );
            }
        }

        fn under(&self, path: &Path) -> bool {
            path.starts_with(&self.root)
        }
    }

    /// One syscall line of an `strace -y` trace: its name, its arguments, and
    /// what it returned.
    struct Call<'a> {
        name: &'a str,
        args: &'a str,
        ret: &'a str,
    }

    fn parse(line: &str) -> Option<Call<'_>> {
        let open = line.find('(')?;
        let name = &line[..open];
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
        let close = line.rfind(") = ")?;
        Some(Call {
            name,
            args: &line[open + 1..close],
            ret: line[close + 4..].trim(),
        })
    }

    /// The path `-y` prints after a descriptor: `5</a/b>` names `/a/b`.
    fn fd_path(arg: &str) -> Option<PathBuf> {
        let start = arg.find('<')?;
        let end = arg.rfind('>')?;
        (end > start).then(|| PathBuf::from(&arg[start + 1..end]))
    }

    /// The first quoted argument, unescaped enough for a path.
    fn quoted(args: &str) -> Option<String> {
        let (_, rest) = args.split_once('"')?;
        let (path, _) = rest.split_once('"')?;
        Some(path.to_owned())
    }

    /// The path an open names, resolved against its directory descriptor when
    /// it is relative.
    fn opened_path(call: &Call<'_>) -> Option<PathBuf> {
        let path = PathBuf::from(quoted(call.args)?);
        if path.is_absolute() || call.name == "open" || call.name == "creat" {
            return Some(path);
        }
        let dirfd = call.args.split(',').next().unwrap_or_default();
        Some(match fd_path(dirfd) {
            Some(dir) => dir.join(path),
            None => path,
        })
    }

    fn returned(ret: &str) -> Option<u64> {
        ret.split_whitespace().next()?.parse().ok()
    }

    /// What the process tree did to the store, off one trace: every path it
    /// opened (a failed open counts), every store directory it listed, and the
    /// bytes it read through each descriptor it opened on a segment.
    #[derive(Default, Debug)]
    struct Observed {
        opened: BTreeSet<PathBuf>,
        listed: BTreeSet<PathBuf>,
        /// Bytes read per `(pid, open ordinal)` on each segment.
        segment_reads: Vec<(PathBuf, u64)>,
        /// Calls that read a store file other than by `read(2)` and friends.
        unbounded: Vec<String>,
        /// How many calls the trace recorded at all.
        calls: usize,
    }

    fn observe(store: &Store, trace_prefix: &Path) -> Observed {
        let mut observed = Observed::default();
        let dir = trace_prefix.parent().expect("the trace has a directory");
        let stem = trace_prefix
            .file_name()
            .and_then(|name| name.to_str())
            .expect("the trace prefix is a name");
        for entry in std::fs::read_dir(dir).expect("the trace directory lists") {
            let file = entry.expect("a trace file").path();
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !name.starts_with(&format!("{stem}.")) {
                continue;
            }
            let text = std::fs::read_to_string(&file).expect("a per-process trace reads");
            // Per process, so a descriptor number means one file from its
            // open to its close.
            let mut live: BTreeMap<String, usize> = BTreeMap::new();
            for line in text.lines() {
                let Some(call) = parse(line) else { continue };
                observed.calls += 1;
                match call.name {
                    "open" | "openat" | "openat2" | "creat" | "open_by_handle_at" => {
                        let Some(path) = opened_path(&call) else {
                            continue;
                        };
                        if store.under(&path) {
                            observed.opened.insert(path.clone());
                            if path.parent().is_some_and(|p| p.ends_with(INDEX_DIR)) {
                                if let Some(fd) = call.ret.split('<').next() {
                                    observed.segment_reads.push((path, 0));
                                    live.insert(
                                        fd.trim().to_owned(),
                                        observed.segment_reads.len() - 1,
                                    );
                                }
                            }
                        }
                    }
                    "getdents" | "getdents64" => {
                        if let Some(path) = fd_path(call.args.split(',').next().unwrap_or_default())
                        {
                            if store.under(&path) {
                                observed.listed.insert(path);
                            }
                        }
                    }
                    "read" | "pread64" | "readv" | "preadv" | "preadv2" => {
                        let first = call.args.split(',').next().unwrap_or_default();
                        let fd = first.split('<').next().unwrap_or_default().trim();
                        if let (Some(at), Some(n)) = (live.get(fd), returned(call.ret)) {
                            observed.segment_reads[*at].1 += n;
                        }
                    }
                    "close" => {
                        let fd = call.args.split('<').next().unwrap_or_default().trim();
                        live.remove(fd);
                    }
                    "mmap" | "mmap2" | "sendfile" | "sendfile64" | "copy_file_range" | "splice"
                        if call
                            .args
                            .split(',')
                            .filter_map(fd_path)
                            .any(|path| store.under(&path)) =>
                    {
                        observed.unbounded.push(line.to_owned());
                    }
                    _ => {}
                }
            }
        }
        observed
    }

    /// The command, run under `strace -f` with one trace file per process.
    /// Refuses rather than passes where the tracer will not run: an
    /// observation nobody made is not an observation of nothing.
    fn traced(inner: &Command, prefix: &Path) -> std::process::Output {
        let mut traced = Command::new("strace");
        traced
            .arg("-ff")
            .arg("-qq")
            .arg("-y")
            .arg("-s")
            .arg("0")
            .arg("-e")
            .arg("trace=%file,%desc,getdents,getdents64")
            .arg("-o")
            .arg(prefix)
            .arg(inner.get_program())
            .args(inner.get_args())
            .stdin(std::process::Stdio::null());
        for (key, value) in inner.get_envs() {
            match value {
                Some(value) => traced.env(key, value),
                None => traced.env_remove(key),
            };
        }
        if let Some(dir) = inner.get_current_dir() {
            traced.current_dir(dir);
        }
        traced.output().unwrap_or_else(|error| {
            panic!(
                "this journey's claim is what the process tree did to the history store, \
                 and the tracer would not run: strace: {error}. Install strace, or run the \
                 suite where ptrace is permitted."
            )
        })
    }

    /// The segment each pointer's run lands in, and the session files the
    /// pointers name.
    fn permitted(pointers: &[HistoryPointer]) -> (BTreeSet<PathBuf>, BTreeSet<PathBuf>) {
        let mut segments = BTreeSet::new();
        let mut sessions = BTreeSet::new();
        for pointer in pointers {
            let date = UtcDate::of_history_id(pointer.history_id())
                .expect("a minted history id carries its date");
            let dir = Path::new(pointer.history_dir()).join(INDEX_DIR);
            // A run's closing entry goes to its runs segment and each of its
            // events' entries to its events segment: one segment per append.
            segments.insert(dir.join(SegmentKind::Runs.file_name(date)));
            segments.insert(dir.join(SegmentKind::Events.file_name(date)));
            sessions.insert(PathBuf::from(pointer.history_file()));
        }
        (segments, sessions)
    }

    /// Launch one run of one agent node into `store`, traced, then read its
    /// sessions back through `onepipeline agents`, traced, and hold both
    /// traces and the store to the history contract.
    fn records_and_reads_without_a_scan(world: &World, run: &str) -> Vec<HistoryPointer> {
        world.script("harness.work", "the worker wrote this\n");
        let store = Store::build(&world.root.join("long-lived-store"));
        let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));

        let traces = world.root.join("traces");
        std::fs::create_dir_all(&traces).expect("a directory for the traces");
        let mut start = world.agentgraph_cmd(&["start", &path, "--attach"]);
        start.env(HISTORY_DIR_ENV, &store.root);
        let recording = traces.join("record");
        let launched = traced(&start, &recording);
        assert!(
            launched.status.success(),
            "`start` exited {:?}: {}\n{}",
            launched.status.code(),
            String::from_utf8_lossy(&launched.stderr),
            world.dump()
        );

        let lines = pointers(world, run);
        every_line_names(world, run, &lines);
        let (segments, sessions) = permitted(&lines);
        let recorded = observe(&store, &recording);
        assert!(
            recorded.listed.is_empty(),
            "recording listed store directories: {:?}",
            recorded.listed
        );
        let strays: Vec<&PathBuf> = recorded
            .opened
            .iter()
            .filter(|path| !segments.contains(*path) && !sessions.contains(*path))
            .collect();
        assert!(
            strays.is_empty(),
            "recording opened store files other than its own sessions and the segments it \
             appends to: {strays:?}"
        );
        for (segment, read) in &recorded.segment_reads {
            assert!(
                *read <= TRAILING_BYTES,
                "recording read {read} bytes of {} through one descriptor, past the \
                 {TRAILING_BYTES}-byte trailing bound",
                segment.display()
            );
        }
        assert!(
            recorded.unbounded.is_empty(),
            "recording read a store file by other than read(2): {:?}",
            recorded.unbounded
        );
        // The positive control: the tracer did see the appends it permits.
        assert!(
            !recorded.segment_reads.is_empty(),
            "the tracer saw no append to a segment, so it observed nothing: {recorded:?}"
        );
        // And at least one turn emitted an event, so the events segment's
        // append is among what was traced and bounded, not merely permitted.
        assert!(
            recorded.segment_reads.iter().any(|(segment, _)| {
                segment
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("events-"))
            }),
            "no turn appended an event, so the events segment went untested: {recorded:?}"
        );
        let evented = lines.iter().any(|pointer| {
            let date = UtcDate::of_history_id(pointer.history_id()).expect("a dated id");
            let segment = store
                .root
                .join(INDEX_DIR)
                .join(SegmentKind::Events.file_name(date));
            std::fs::read_to_string(&segment)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str::<HistoryIndexEntry>(line).ok())
                .any(|entry| {
                    matches!(entry, HistoryIndexEntry::Event(event)
                        if event.run_id == pointer.history_id())
                })
        });
        assert!(
            evented,
            "no run's event landed in the events segment of its date"
        );
        for pointer in &lines {
            assert_eq!(
                Path::new(pointer.history_dir()),
                store.root,
                "a turn recorded somewhere other than the store it was given: {pointer:?}"
            );
            // Its session holds its run's record, and its runs segment the
            // entry pointing at it.
            let file = Path::new(pointer.history_file());
            let records = read_session(file)
                .unwrap_or_else(|error| panic!("{} does not read: {error}", file.display()));
            assert!(
                records
                    .iter()
                    .any(|record| record.history_id == pointer.history_id()),
                "{} holds no record of run {}",
                file.display(),
                pointer.history_id()
            );
            let date = UtcDate::of_history_id(pointer.history_id()).expect("a dated id");
            let segment = store
                .root
                .join(INDEX_DIR)
                .join(SegmentKind::Runs.file_name(date));
            let text = std::fs::read_to_string(&segment)
                .unwrap_or_else(|error| panic!("{} does not read: {error}", segment.display()));
            let indexed = text
                .lines()
                .filter_map(|line| serde_json::from_str::<HistoryIndexEntry>(line).ok())
                .any(|entry| match entry {
                    HistoryIndexEntry::Run(entry) => {
                        entry.history_id == pointer.history_id()
                            && entry.session_path.under(&store.root) == file
                    }
                    HistoryIndexEntry::Event(_) => false,
                });
            assert!(
                indexed,
                "{} carries no entry for run {} of {}",
                segment.display(),
                pointer.history_id(),
                file.display()
            );
        }
        store.untouched();

        // The read: the verb, traced, lists each session from the store.
        let reading = traces.join("read");
        let listed = traced(&world.agentgraph_cmd(&["agents", run]), &reading);
        let stdout = String::from_utf8_lossy(&listed.stdout);
        assert!(
            listed.status.success(),
            "`agents` exited {:?}: {}",
            listed.status.code(),
            String::from_utf8_lossy(&listed.stderr)
        );
        for pointer in &lines {
            for field in [
                pointer.history_session(),
                pointer.history_file(),
                pointer.history_dir(),
            ] {
                assert!(
                    stdout.contains(field),
                    "`agents` does not name {field}: {stdout}"
                );
            }
            assert!(
                stdout.contains(&pointer.history_id().to_string()),
                "`agents` does not name run {}: {stdout}",
                pointer.history_id()
            );
        }
        let read = observe(&store, &reading);
        assert!(read.calls > 0, "the tracer saw nothing of the read");
        assert!(
            read.listed.is_empty(),
            "the read listed store directories: {:?}",
            read.listed
        );
        let strays: Vec<&PathBuf> = read
            .opened
            .iter()
            .filter(|path| !sessions.contains(*path))
            .collect();
        assert!(
            strays.is_empty(),
            "the read opened store files its pointer lines do not name: {strays:?}"
        );
        assert!(read.unbounded.is_empty(), "{:?}", read.unbounded);
        store.untouched();
        lines
    }

    /// A two-party member: each turn of both sides is a spawned
    /// `fake-oneharness`, recording through the linked core's `HistoryWriter`.
    #[test]
    fn a_spawned_oneharness_turn_records_and_reads_back_without_scanning_the_store() {
        let world = World::new("agents-unscanned-spawned");
        world.write_graphs();
        world.write_supervised_node_graph();
        let lines = records_and_reads_without_a_scan(&world, "unscanned-spawned");
        assert!(
            world
                .invocations()
                .iter()
                .any(|call| call["tool"] == "oneharness"),
            "no turn went through the spawned oneharness double: {lines:?}"
        );
    }

    /// A single-sided `kind: oneharness` member: its turn is a library call in
    /// the sibling's own process, recording through the same linked core.
    #[test]
    fn an_in_process_oneharness_turn_records_and_reads_back_without_scanning_the_store() {
        let world = World::new("agents-unscanned-in-process");
        world.write_graphs();
        let lines = records_and_reads_without_a_scan(&world, "unscanned-in-process");
        assert!(
            !world
                .invocations()
                .iter()
                .any(|call| call["tool"] == "oneharness"),
            "a turn went through the spawned oneharness double, so the in-process path \
             was not what recorded: {lines:?}"
        );
    }
}
