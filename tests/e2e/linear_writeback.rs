//! The settlement write-back against a `linear` destination, driven end to end: the compiled
//! binary reads its plan out of a Linear project and writes every settlement back to it through
//! the Linear plugin the engine links, configured with the per-kind `status_mapping` an
//! ai-orchestrator launch's `onetaskgraph.yaml` carries. Linear itself is
//! `tests/e2e/linear_loopback.rs`, a GraphQL endpoint on `127.0.0.1` that logs every request
//! it is sent, which is what each attempt's cost is read off.
//!
//! The plan holds two nodes at concurrency one: `anchor`, whose dispatch is held for the whole
//! journey so the run stays live, and `work`, which never dispatches and is settled from
//! evidence over and over — `done`, then `failed`, then `done` again — each settlement to a status
//! and a settlement record the one before it did not carry, and each projected in an attempt of
//! its own.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// Linear is the loopback endpoint beside this file — real HTTP on a real socket, reached by the
// real Linear plugin — because an offline check may not reach the hosted API. `harness.rs`
// carries the same suppression and the full rationale.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{World, STORE_SOURCE};
use crate::linear_loopback::{described, metadata_of, Fault, Linear, Operation, PATIENCE, PROJECT};

/// The source the launch directory's `onetaskgraph.yaml` names the loopback Linear as.
const SOURCE: &str = "tracker";

/// The variable that source reads its key from.
const KEY_ENV: &str = "ONEPIPELINE_LOOPBACK_LINEAR_KEY";

/// The per-kind mapping, exactly as an ai-orchestrator launch's tracked `onetaskgraph.yaml`
/// states it, indented as it sits under the source's `config`.
const MAPPING: &str = "      status_mapping:
        backlog:     { task: Proposed,        project: Proposal }
        draft:       { task: Backlog,         project: Idea }
        todo:        { task: Todo,            project: Planned }
        queued:      { task: Queued,          project: Accepted }
        in-progress: In Progress
        unknown:     { task: Needs Attention, project: Blocked }
        done:        { task: Done,            project: Completed }
        cancelled:   Canceled
";

/// The team's workflow states, by Linear's `WorkflowState.type`. `Needs Attention` is not among
/// them: a journey adds it once the run is under way, as a person would in Linear's settings.
const STATES: &[(&str, &str)] = &[
    ("Proposed", "backlog"),
    ("Backlog", "backlog"),
    ("Todo", "unstarted"),
    ("Queued", "unstarted"),
    ("In Progress", "started"),
    ("Done", "completed"),
    ("Canceled", "canceled"),
];

/// The workspace's project statuses: a vocabulary of their own, sharing two names with the
/// team's states and no more.
const STATUSES: &[(&str, &str)] = &[
    ("Proposal", "backlog"),
    ("Idea", "backlog"),
    ("Planned", "planned"),
    ("Accepted", "planned"),
    ("In Progress", "started"),
    ("Blocked", "started"),
    ("Completed", "completed"),
    ("Canceled", "canceled"),
];

/// The settlement record's key on a destination item.
const SETTLEMENT: &str = "onepipeline.settlement";

/// One run against the loopback Linear: the world, the endpoint, the issue each node lives in,
/// and the directory the launch ran from, whose `onetaskgraph.yaml` every write-back reads.
struct Scenario {
    world: World,
    linear: Linear,
    run: String,
    anchor: String,
    work: String,
    launch: PathBuf,
}

fn task_text(id: &str) -> String {
    format!("## What\nDo {id}.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- {id} is done.")
}

/// The store configuration a launch directory holds: one `linear` source over the loopback.
fn configuration(linear: &Linear) -> String {
    format!(
        "sources:\n  {SOURCE}:\n    plugin: linear\n    config:\n      api_key_env: {KEY_ENV}\n      \
         team: {}\n      endpoint: {}\n{MAPPING}",
        crate::linear_loopback::TEAM,
        linear.endpoint()
    )
}

/// Launch a run of the two-node plan held in the loopback's project, from a directory whose
/// `onetaskgraph.yaml` is the only store configuration any command of it reads, and wait for
/// its first projection to come to rest: `anchor` in progress and `work` queued, each carrying
/// the run's own metadata.
fn launched_with_states(name: &str, extra: &[&str], states: &[(&str, &str)]) -> Scenario {
    launched_configured(name, extra, states, false)
}

fn launched_configured(
    name: &str,
    extra: &[&str],
    states: &[(&str, &str)],
    boundary: bool,
) -> Scenario {
    let world = World::new(name).with_env(KEY_ENV, "a-loopback-key");
    world.script("anchor.wait", "hold");
    let linear = Linear::serve(
        states,
        STATUSES,
        name,
        &described(
            "",
            &json!({
                "onepipeline.schema_version": onepipeline::plan::PLAN_SCHEMA_VERSION,
                "onepipeline.concurrency": 1,
                "onepipeline.goal": {"text": format!("Deliver {name}")},
            }),
        ),
        "Planned",
    );
    let issue = |id: &str| {
        linear.file(
            &format!("chore: {id}"),
            &described(
                &task_text(id),
                &json!({"onepipeline.id": id, "onepipeline.persona": "engineer"}),
            ),
            "Todo",
        )
    };
    let anchor = issue("anchor");
    let work = issue("work");
    let launch = world.root.join("launch-directory");
    std::fs::create_dir_all(&launch).expect("a launch directory");
    let mut config = configuration(&linear);
    if boundary {
        std::fs::create_dir_all(world.store()).expect("the boundary source's root");
        let quote =
            |path: &Path| serde_json::to_string(&path.to_string_lossy()).expect("a quoted path");
        config.push_str(&format!(
            "  boundary:\n    plugin: subprocess\n    config:\n      command: {}\n      deadline_ms: 120000\n      settings:\n        root: {}\n        script: {}\n        key: {}\n",
            quote(&crate::harness::double("scripted-source")), quote(&world.store()),
            quote(&world.fakes), crate::harness::SCRIPTED_KEY,
        ));
    }
    std::fs::write(launch.join("onetaskgraph.yaml"), config)
        .expect("the launch directory's store configuration");

    let project = format!("{SOURCE}:{PROJECT}");
    let mut args = vec!["start", project.as_str(), "--detach"];
    args.extend_from_slice(extra);
    let start = from_the_launch_directory(&world, &launch, &args);
    world.run_on(start, "start --detach").exited(0);
    // A run is named for its plan, and this plan's name is its project's.
    let run = name.to_owned();
    let scenario = Scenario {
        world,
        linear,
        run,
        anchor,
        work,
        launch,
    };
    scenario.until(
        "the claim and the running node to reach Linear",
        |scenario| {
            scenario.linear.state_of(&scenario.anchor) == "In Progress"
                && scenario.linear.state_of(&scenario.work) == "Queued"
        },
    );
    scenario.at_rest("the launch's projections");
    assert_eq!(
        scenario
            .linear
            .log()
            .iter()
            .filter(|request| request.operation.resolves())
            .count(),
        1,
        "the first worker attempt must resolve exactly once"
    );
    scenario
}

/// A command whose store is configured by the launch directory's `onetaskgraph.yaml` alone:
/// every setting this world's commands carry for their own plans source is taken away.
fn from_the_launch_directory(world: &World, dir: &Path, args: &[&str]) -> std::process::Command {
    let mut command = world.cmd(args);
    // This journey's file is the only source configuration: inherited host settings
    // must not introduce another source, endpoint or credential into the measurement.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("ONETASKGRAPH_") {
            command.env_remove(key);
        }
    }
    command
        .current_dir(dir)
        .env_remove("ONETASKGRAPH_DEFAULT_SOURCES")
        .env_remove(format!(
            "ONETASKGRAPH_SOURCES__{}__PLUGIN",
            STORE_SOURCE.to_uppercase()
        ))
        .env_remove(crate::harness::store_root_env());
    command
}

/// What one attempt sent Linear, and how it ended.
struct Spent {
    requests: Vec<crate::linear_loopback::Request>,
    records: Vec<Value>,
}

impl Spent {
    fn of(&self, operation: Operation) -> usize {
        self.requests
            .iter()
            .filter(|request| request.operation == operation)
            .count()
    }

    fn resolutions(&self) -> usize {
        self.requests
            .iter()
            .filter(|request| request.operation.resolves())
            .count()
    }

    /// The `issueUpdate` this stretch sent `issue`, which there has to be exactly one of.
    fn update_of(&self, issue: &str) -> &Value {
        let updates: Vec<&Value> = self
            .requests
            .iter()
            .filter(|request| {
                request.operation == Operation::IssueUpdate && request.variables["id"] == issue
            })
            .map(|request| &request.variables["input"])
            .collect();
        match updates.as_slice() {
            [one] => one,
            other => panic!("expected one issueUpdate of {issue}, found {other:?}"),
        }
    }
}

impl Scenario {
    fn until(&self, what: &str, mut ready: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !ready(self) {
            assert!(
                Instant::now() < deadline,
                "{what} did not happen; Linear was sent {:?}\nthe runs root held:\n{}",
                self.linear
                    .log()
                    .iter()
                    .map(|request| request.operation)
                    .collect::<Vec<_>>(),
                self.world.dump()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn records(&self) -> Vec<Value> {
        std::fs::read_to_string(
            self.world
                .run_file(&self.run, onepipeline::cli::WRITEBACK_PROJECTIONS_FILE),
        )
        .map(|text| {
            text.lines()
                .map(|line| serde_json::from_str(line).expect("a record line is JSON"))
                .collect()
        })
        .unwrap_or_default()
    }

    /// Wait until the write-back is at rest: its last attempt recorded, and Linear sent nothing
    /// more for a while.
    fn at_rest(&self, what: &str) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let sent = self.linear.log().len();
            let recorded = self.records().len();
            std::thread::sleep(Duration::from_millis(1_500));
            if self.linear.log().len() == sent && self.records().len() == recorded {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the write-back did not come to rest after {what}; the runs root held:\n{}",
                self.world.dump()
            );
        }
    }

    /// Send the run one reply, and answer what Linear was sent and the run recorded from then
    /// until the write-back came to rest again, once `landed` holds.
    fn replied(&self, what: &str, command: &Value, landed: impl FnMut(&Self) -> bool) -> Spent {
        let sent = self.linear.log().len();
        let recorded = self.records().len();
        self.reply(command);
        self.until(what, landed);
        self.at_rest(what);
        Spent {
            requests: self.linear.log()[sent..].to_vec(),
            records: self.records()[recorded..].to_vec(),
        }
    }

    fn reply(&self, command: &Value) {
        self.world
            .run_with_stdin(
                &["reply", &self.run],
                &json!({"version": 2, "commands": [command]}).to_string(),
            )
            .exited(0);
    }

    /// The settlement evidence one issue's metadata slot now carries.
    fn evidence_on(&self, issue: &str) -> Option<String> {
        metadata_of(&self.linear.description_of(issue))
            .get(SETTLEMENT)
            .and_then(|settlement| settlement["detail"].as_str())
            .map(str::to_owned)
    }

    /// What the run's landed baseline says `work` holds: its word and its settlement evidence.
    fn landed_work(&self) -> (Value, Value) {
        let landed = self
            .world
            .run_json(&self.run, onepipeline::cli::WRITEBACK_LANDED_FILE);
        let work = &landed["items"]["work"];
        (
            work["status"].clone(),
            work["metadata"][SETTLEMENT]["detail"].clone(),
        )
    }

    /// Let the held dispatch go, so the run settles and its driver leaves.
    fn finish(&self) {
        self.linear.release();
        self.world.release("anchor.go");
        self.world.until("the run to settle", |world| {
            world.run_file(&self.run, "result.json").is_file()
        });
    }
}

fn settle(outcome: &str, evidence: &str) -> Value {
    json!({"op": "settle", "id": "work", "outcome": outcome, "evidence": evidence})
}

fn note(text: &str) -> Value {
    json!({"op": "note", "id": "anchor", "addressee": "worker", "text": text, "deliver": "next"})
}

/// The name the per-kind mapping gives a task at the word a settlement projects.
fn mapped(outcome: &str) -> &'static str {
    match outcome {
        "done" => "Done",
        // `failed` has no category of its own and is written `unknown`.
        "failed" => "Needs Attention",
        other => panic!("no settlement outcome {other}"),
    }
}

/// Six consecutive settlements of `work`, each projected in an attempt of its own, each landing
/// its own status and settlement record and never an earlier one's: the mutation Linear recorded
/// for it carries that settlement's state and evidence, the run's landed baseline says the same,
/// and the shadow snapshot under the run's write-back root holds that settlement's values.
/// `each` is handed every settlement's step and what its attempt sent, once all of that holds;
/// what is answered is how many requests Linear was sent for each attempt.
fn six_settlements(scenario: &Scenario, mut each: impl FnMut(usize, &Spent)) -> Vec<usize> {
    let mut costs = Vec::new();
    for (step, outcome) in ["done", "failed", "done", "failed", "done", "failed"]
        .into_iter()
        .enumerate()
    {
        let evidence = format!("settlement {step}: the operator saw it {outcome}");
        let spent = scenario.replied(
            &format!("settlement {step} to reach Linear"),
            &settle(outcome, &evidence),
            |scenario| {
                scenario.linear.state_of(&scenario.work) == mapped(outcome)
                    && scenario.evidence_on(&scenario.work).as_deref() == Some(evidence.as_str())
            },
        );
        assert_eq!(
            spent.records.len(),
            1,
            "settlement {step} was not projected in one attempt of its own: {:?}",
            spent.records
        );
        assert_eq!(
            spent.records[0]["outcome"], "projected",
            "{:?}",
            spent.records
        );
        assert_eq!(
            spent.records[0]["items"],
            json!(["work"]),
            "{:?}",
            spent.records
        );

        let written = spent.update_of(&scenario.work);
        assert_eq!(
            written["stateId"]
                .as_str()
                .and_then(|state| scenario.linear.state_named(state)),
            Some(mapped(outcome).to_owned()),
            "settlement {step}'s mutation does not carry its own status: {written}"
        );
        let carried = written["description"]
            .as_str()
            .map(metadata_of)
            .unwrap_or_default();
        assert_eq!(
            carried[SETTLEMENT]["detail"], evidence,
            "settlement {step}'s mutation does not carry its own settlement record: {written}"
        );
        assert!(
            written["description"]
                .as_str()
                .is_some_and(|description| description.starts_with(&task_text("work"))),
            "settlement {step}'s mutation did not keep the issue's content: {written}"
        );
        assert_eq!(
            scenario.landed_work(),
            (json!(outcome), json!(evidence)),
            "the landed baseline does not hold settlement {step}"
        );
        let shadow = scenario
            .world
            .run_file(&scenario.run, "writeback")
            .join("tasks")
            .join(
                format!("{SOURCE}:{PROJECT}")
                    .bytes()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
            )
            .join("776f726b.md");
        let text = std::fs::read_to_string(&shadow)
            .unwrap_or_else(|error| panic!("{}: {error}", shadow.display()));
        assert!(
            text.contains(&evidence),
            "the shadow kept old metadata: {text}"
        );
        assert!(
            text.contains(&format!("status: {outcome}")),
            "the shadow kept old status: {text}"
        );
        each(step, &spent);
        costs.push(spent.requests.len());
    }
    costs
}

/// The team's states with `Needs Attention` among them, so every name the mapping writes is
/// known from the run's first resolution read.
fn every_mapped_state() -> Vec<(&'static str, &'static str)> {
    let mut states = STATES.to_vec();
    states.push(("Needs Attention", "started"));
    states
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the journeys below read a
// plan from a hosted source and write settlements through the linked hosted plugins,
// exercising `taskgraph`, `writeback`, `edits` and `driver` together, so the crate is the
// narrowest edge they can honestly sit behind — the grounds `mod multi_source` records in
// `main.rs`. The third waits past the write-back's sixty-second floor by construction, as
// `writeback_budget.rs`'s journeys do: a write held past its deadline cannot be observed sooner.
/// Six consecutive settlements, each landing its own current values ([`six_settlements`]), on
/// one store: each attempt sends one read of the issue — the description its metadata is merged
/// into, which keeps whatever a person wrote there — and one `issueUpdate`, and no resolution
/// read, because every name it writes was resolved by the run's first attempt. Its vocabulary
/// is present before launch; the failure journey separately adds a state after the cache fills
/// and proves that one refresh finds it.
#[test]
fn consecutive_linear_settlements_each_land_their_own_status_and_metadata_in_their_own_attempt() {
    let scenario = launched_with_states("linear-settlements", &[], &every_mapped_state());
    six_settlements(&scenario, |step, spent| {
        let sent: Vec<Operation> = spent
            .requests
            .iter()
            .map(|request| request.operation)
            .collect();
        assert!(
            spent.of(Operation::Issue) == 1
                && spent.of(Operation::IssueUpdate) == 1
                && spent.resolutions() <= 1
                && sent.len() == 2 + spent.resolutions(),
            "settlement {step} sent Linear more than one read of the issue, one write and at \
             most one resolution: {sent:?}"
        );
        assert_eq!(
            spent.resolutions(),
            0,
            "a reused store resolved again: {sent:?}"
        );
    });
    scenario.finish();
}

/// The `linear-requests-per-writeback-settlement` budget's measure: the same six settlements,
/// each held to landing its own current values and nothing about what it cost, and the most
/// requests Linear was sent for one attempt after the run's first, which also resolves the
/// vocabulary. Written to the file `ONEBUDGETSPEC_RESULT` names when the budget's command runs
/// this journey; whether that figure is within the budget is the checker's to say, never this
/// journey's, so an attempt that cost more is reported rather than failed.
#[test]
fn the_costliest_linear_settlement_after_the_first_is_reported_as_measured() {
    let scenario = launched_with_states("linear-settlement-budget", &[], &every_mapped_state());
    let costs = six_settlements(&scenario, |_, _| {});
    scenario.finish();
    let highest = costs.iter().skip(1).copied().max().unwrap_or_default();
    let detail = format!(
        "the most requests the loopback Linear served for one attempt over {} consecutive \
         settlements after the run's first, each in its own attempt: {:?}",
        costs.len() - 1,
        &costs[1..]
    );
    if let Some(result) = std::env::var_os("ONEBUDGETSPEC_RESULT") {
        std::fs::write(
            result,
            crate::budget_result::reported(highest, &detail).to_string(),
        )
        .expect("the budget's result is written");
    }
}

/// Each way a Linear call fails leaves the next attempt landing the values current when it is
/// made: a connection Linear closes mid-read is retried on the schedule, a write Linear refuses
/// — one carrying only metadata, and one carrying a state — is attempted again when the graph
/// next changes, a write held past its deadline is cancelled and the attempt after it lands, and
/// an attempt made while the launch directory's `onetaskgraph.yaml` cannot be read is recorded as
/// that failure, with the attempt after its repair landing.
#[test]
fn a_linear_destination_lands_current_values_after_each_way_a_call_fails() {
    let scenario = launched_configured(
        "linear-failures",
        &["--writeback-item-budget", "1"],
        STATES,
        true,
    );
    let opened = scenario.world.store_asked("initialize");
    scenario.linear.add_state("Needs Attention", "started");

    // A connection closed mid-read: transient, so retried on the schedule, and the retry lands.
    scenario.linear.fail_next(Operation::Issue, Fault::Drop);
    let spent = scenario.replied(
        "a settlement after a dropped connection to reach Linear",
        &settle("done", "after a dropped connection"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref() == Some("after a dropped connection")
        },
    );
    assert_eq!(
        outcomes(&spent),
        ["failed", "projected"],
        "{:?}",
        spent.records
    );
    assert_eq!(scenario.linear.state_of(&scenario.work), "Done");
    assert_eq!(
        spent.resolutions(),
        0,
        "a network failure rebuilt the store"
    );

    // A metadata-only write Linear refuses: not retried on a timer, attempted again with the
    // next change, which lands both.
    scenario
        .linear
        .fail_next(Operation::IssueUpdate, Fault::Refuse);
    let refused = scenario.replied(
        "a refused note to be recorded",
        &note("a note Linear refused the first time"),
        |scenario| {
            scenario
                .records()
                .last()
                .is_some_and(|record| record["outcome"] == "failed")
        },
    );
    assert_eq!(outcomes(&refused), ["failed"], "{:?}", refused.records);
    assert_eq!(
        refused.records[0]["class"], "refused",
        "{:?}",
        refused.records
    );
    let landed = scenario.replied(
        "the change after a refused metadata write to reach Linear",
        &settle("failed", "after a refused metadata write"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref()
                == Some("after a refused metadata write")
                && scenario
                    .linear
                    .description_of(&scenario.anchor)
                    .contains("a note Linear refused the first time")
        },
    );
    assert_eq!(outcomes(&landed), ["projected"], "{:?}", landed.records);
    assert_eq!(scenario.linear.state_of(&scenario.work), "Needs Attention");
    assert_eq!(
        landed.resolutions(),
        1,
        "the newly added state needs one refresh"
    );

    // A write carrying a state that Linear refuses: the next change lands the state and the
    // evidence the refused one carried, beside its own.
    scenario
        .linear
        .fail_next(Operation::IssueUpdate, Fault::Refuse);
    scenario.replied(
        "a refused status write to be recorded",
        &settle("done", "a status Linear refused the first time"),
        |scenario| {
            scenario
                .records()
                .last()
                .is_some_and(|record| record["outcome"] == "failed")
        },
    );
    assert_eq!(scenario.linear.state_of(&scenario.work), "Needs Attention");
    let landed = scenario.replied(
        "the change after a refused status write to reach Linear",
        &note("a note after a refused status write"),
        |scenario| {
            scenario.linear.state_of(&scenario.work) == "Done"
                && scenario.evidence_on(&scenario.work).as_deref()
                    == Some("a status Linear refused the first time")
        },
    );
    assert_eq!(outcomes(&landed), ["projected"], "{:?}", landed.records);
    assert_eq!(
        landed.resolutions(),
        1,
        "a refused held vocabulary id needs one refresh"
    );

    // A network failure after a mutation carried a cached state id invalidates that
    // vocabulary, while a metadata-only mutation failure leaves it reusable.
    scenario
        .linear
        .fail_next(Operation::IssueUpdate, Fault::Drop);
    let dropped_state = scenario.replied(
        "a dropped status mutation to be retried",
        &settle("failed", "after a dropped status mutation"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref()
                == Some("after a dropped status mutation")
        },
    );
    assert_eq!(outcomes(&dropped_state), ["failed", "projected"]);
    assert_eq!(
        dropped_state.resolutions(),
        1,
        "a failed cached state id needs one refresh"
    );
    scenario
        .linear
        .fail_next(Operation::IssueUpdate, Fault::Drop);
    let dropped_metadata = scenario.replied(
        "a dropped metadata mutation to be retried",
        &note("after a dropped metadata mutation"),
        |scenario| {
            scenario
                .linear
                .description_of(&scenario.anchor)
                .contains("after a dropped metadata mutation")
        },
    );
    assert_eq!(outcomes(&dropped_metadata), ["failed", "projected"]);
    assert_eq!(
        dropped_metadata.resolutions(),
        0,
        "metadata failures keep the vocabulary"
    );

    // The shadow filesystem can fail independently of the destination. Record the failure
    // and replace its old contents once the directory is writable again.
    // llmlint: ignore-block[tests_mirror_real_usage] a shadow write fails only when the
    // filesystem under the run directory does (a full disk, a lost permission), and no command
    // of the binary produces that. Obstructing the directory is that fault, injected at the
    // filesystem; the binary is still driven only through `reply`, and what is asserted is its
    // record and what reached Linear.
    let tasks = scenario
        .world
        .run_file(&scenario.run, "writeback")
        .join("tasks");
    let saved = tasks.with_file_name("saved-tasks");
    std::fs::rename(&tasks, &saved).expect("the old shadow is retained for recovery");
    std::fs::write(&tasks, "a file prevents writing the shadow directory")
        .expect("block the shadow");
    let shadow_failed = scenario.replied(
        "a shadow failure to be recorded",
        &settle("done", "while the shadow was unwritable"),
        |scenario| {
            scenario.records().iter().any(|record| {
                record["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("cannot refresh the shadow store"))
            })
        },
    );
    assert!(shadow_failed
        .records
        .iter()
        .any(|record| record["outcome"] == "failed"));
    std::fs::remove_file(&tasks).expect("remove the obstruction");
    std::fs::rename(&saved, &tasks).expect("restore the shadow directory");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let shadow_repaired = scenario.replied(
        "the shadow repair to carry current values",
        &settle("failed", "after repairing the shadow"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref() == Some("after repairing the shadow")
        },
    );
    assert_eq!(outcomes(&shadow_repaired), ["projected"]);
    assert_eq!(
        shadow_repaired.resolutions(),
        0,
        "a filesystem failure does not poison the engine"
    );

    assert_eq!(
        scenario.world.store_asked("initialize"),
        opened,
        "refused writes, network errors and shadow failures must reuse every source"
    );

    // A configuration that cannot be read: recorded as that failure, and the attempt after its
    // repair lands what changed meanwhile.
    let document = scenario.launch.join("onetaskgraph.yaml");
    let configured = std::fs::read_to_string(&document).expect("the configuration reads");
    std::fs::remove_file(&document).expect("the configuration is taken away");
    std::fs::create_dir(&document).expect("a directory stands in its place");
    let unread = scenario.replied(
        "an attempt over an unreadable configuration to be recorded",
        &settle("done", "while the configuration could not be read"),
        |scenario| {
            scenario.records().last().is_some_and(|record| {
                record["outcome"] == "failed" && record["kind"] == "config-read"
            })
        },
    );
    assert!(
        unread.requests.is_empty(),
        "an unreadable configuration reached Linear"
    );
    std::fs::remove_dir(&document).expect("the directory is taken away");
    std::fs::write(&document, &configured).expect("the configuration is put right");
    let repaired = scenario.replied(
        "the change after the repair to reach Linear",
        &note("a note once the configuration was put right"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref()
                == Some("while the configuration could not be read")
                && scenario.linear.state_of(&scenario.work) == "Done"
        },
    );
    assert_eq!(outcomes(&repaired), ["projected"], "{:?}", repaired.records);
    assert_eq!(
        repaired.resolutions(),
        1,
        "a repaired configuration needs a new store"
    );
    assert_eq!(scenario.world.store_asked("initialize"), opened + 1);

    // A write held past its deadline — the floor and one item's budget of a second — is
    // cancelled, and the attempt after it lands.
    scenario
        .linear
        .fail_next(Operation::IssueUpdate, Fault::Hold);
    let cancelled = scenario.replied(
        "a write held past its deadline to be cancelled and then landed",
        &settle("failed", "after a write held past its deadline"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref()
                == Some("after a write held past its deadline")
        },
    );
    assert_eq!(
        outcomes(&cancelled),
        ["failed", "projected"],
        "{:?}",
        cancelled.records
    );
    assert!(
        cancelled.records[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("task-update")),
        "the cancelled attempt does not name the call it cancelled: {:?}",
        cancelled.records
    );
    assert_eq!(scenario.linear.state_of(&scenario.work), "Needs Attention");
    assert_eq!(
        cancelled.resolutions(),
        1,
        "the cancelled store must be rebuilt"
    );
    assert_eq!(scenario.world.store_asked("initialize"), opened + 2);
    scenario.finish();
}

/// Valid configuration changes take effect on the next attempt. Rewriting the same
/// resolved content, even with different comments, retains the rebuilt store.
#[test]
fn valid_configuration_edits_rebuild_only_when_resolved_content_changes() {
    let mut states = STATES.to_vec();
    states.push(("Needs Attention", "unstarted"));
    states.push(("Finished", "completed"));
    let scenario = launched_configured("configuration-content", &[], &states, true);
    let opened = scenario.world.store_asked("initialize");
    let document = scenario.launch.join("onetaskgraph.yaml");
    let original = std::fs::read_to_string(&document).expect("the original mapping");
    let changed = original.replace(
        "{ task: Done,            project: Completed }",
        "{ task: Finished,        project: Completed }",
    );
    assert_ne!(
        changed, original,
        "the mapping edit must change resolved content"
    );
    std::fs::write(&document, &changed).expect("a valid mapping edit");
    let changed_attempt = scenario.replied(
        "the edited mapping to take effect",
        &settle("done", "under the edited mapping"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref() == Some("under the edited mapping")
        },
    );
    assert_eq!(outcomes(&changed_attempt), ["projected"]);
    assert_eq!(scenario.linear.state_of(&scenario.work), "Finished");
    assert_eq!(changed_attempt.resolutions(), 1);
    assert_eq!(scenario.world.store_asked("initialize"), opened + 1);
    std::fs::write(
        &document,
        format!("# A comment changes the file, not its configuration.\n{changed}"),
    )
    .expect("rewrite the same resolved content");
    let unchanged_attempt = scenario.replied(
        "the unchanged configuration to reuse its store",
        &settle("failed", "under unchanged resolved content"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref()
                == Some("under unchanged resolved content")
        },
    );
    assert_eq!(outcomes(&unchanged_attempt), ["projected"]);
    assert_eq!(scenario.linear.state_of(&scenario.work), "Needs Attention");
    assert_eq!(unchanged_attempt.resolutions(), 0);
    assert_eq!(scenario.world.store_asked("initialize"), opened + 1);
    scenario.finish();
}

/// A store holding a source it could not build keeps that source unavailable for as long as it
/// lives, so the worker discards it after the attempt, and the attempt after the source's root
/// returns builds it again on a store it then keeps. The destination is unaffected throughout:
/// every attempt lands on Linear.
#[test]
fn a_store_holding_a_source_it_could_not_build_is_rebuilt_once_the_source_returns() {
    let mut states = STATES.to_vec();
    states.push(("Needs Attention", "unstarted"));
    let scenario = launched_configured("unbuilt-source", &[], &states, true);
    let opened = scenario.world.store_asked("initialize");
    let document = scenario.launch.join("onetaskgraph.yaml");
    let root = scenario.world.root.join("spare-board");
    let mut configured = std::fs::read_to_string(&document).expect("the launch's configuration");
    configured.push_str(&format!(
        "  spare:\n    plugin: local-md\n    config:\n      root: {}\n",
        serde_json::to_string(&root.to_string_lossy()).expect("a quoted path")
    ));
    std::fs::write(&document, configured).expect("a source whose root is not there yet");

    let gone = scenario.replied(
        "a settlement while a source cannot be built",
        &settle("done", "while the spare source was gone"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref()
                == Some("while the spare source was gone")
        },
    );
    assert_eq!(outcomes(&gone), ["projected"], "{:?}", gone.records);
    assert_eq!(scenario.linear.state_of(&scenario.work), "Done");
    assert_eq!(
        gone.resolutions(),
        1,
        "an edited configuration needs a new store"
    );
    assert_eq!(scenario.world.store_asked("initialize"), opened + 1);

    std::fs::create_dir_all(&root).expect("the spare source's root returns");
    let returned = scenario.replied(
        "a settlement once the source's root returned",
        &settle("failed", "once the spare source returned"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref()
                == Some("once the spare source returned")
        },
    );
    assert_eq!(outcomes(&returned), ["projected"], "{:?}", returned.records);
    assert_eq!(scenario.linear.state_of(&scenario.work), "Needs Attention");
    assert_eq!(
        returned.resolutions(),
        1,
        "a store holding a source it could not build was kept for the next attempt"
    );
    assert_eq!(scenario.world.store_asked("initialize"), opened + 2);

    let kept = scenario.replied(
        "a settlement on the store whose sources all built",
        &settle("done", "on the store whose sources all built"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref()
                == Some("on the store whose sources all built")
        },
    );
    assert_eq!(outcomes(&kept), ["projected"], "{:?}", kept.records);
    assert_eq!(scenario.linear.state_of(&scenario.work), "Done");
    assert_eq!(
        kept.resolutions(),
        0,
        "a store whose sources all built was discarded"
    );
    assert_eq!(scenario.world.store_asked("initialize"), opened + 2);
    scenario.finish();
}

/// A real filesystem read stalled at a FIFO cannot hold the retained worker past its
/// configuration deadline. Its next attempt starts new sources and lands the current snapshot.
#[cfg(unix)]
#[test]
fn a_retained_stores_blocked_configuration_read_is_bounded_and_rebuilt() {
    let mut states = STATES.to_vec();
    states.push(("Needs Attention", "unstarted"));
    let scenario = launched_configured("configuration-read-deadline", &[], &states, true);
    let opened = scenario.world.store_asked("initialize");
    let document = scenario.launch.join("onetaskgraph.yaml");
    let configured = std::fs::read_to_string(&document).expect("readable configuration");
    std::fs::remove_file(&document).expect("replace configuration with a FIFO");
    assert!(std::process::Command::new("mkfifo")
        .arg(&document)
        .status()
        .expect("create the filesystem FIFO")
        .success());
    let sent = scenario.linear.log().len();
    let recorded = scenario.records().len();
    scenario.reply(&settle("done", "after a blocked configuration read"));
    let (opened_fifo, received) = std::sync::mpsc::channel();
    let fifo = document.clone();
    let writer = std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(fifo)
            .expect("the configuration reader meets the FIFO writer");
        let _ = opened_fifo.send(file);
    });
    let held = received
        .recv_timeout(Duration::from_secs(10))
        .expect("the retained-store validation must open the FIFO");
    writer.join().expect("the writer thread finished");
    scenario.until("the configuration read deadline", |scenario| {
        scenario.records()[recorded..].iter().any(|record| {
            record["outcome"] == "failed"
                && record["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("store-open exceeded 60 seconds"))
        })
    });
    assert_eq!(
        scenario.linear.log().len(),
        sent,
        "a blocked configuration reached Linear"
    );
    std::fs::remove_file(&document).expect("withdraw the FIFO");
    std::fs::write(&document, configured).expect("restore the readable configuration");
    drop(held);
    let repaired = scenario.replied(
        "the configuration deadline recovery",
        &note("readable again"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref()
                == Some("after a blocked configuration read")
                && scenario.linear.state_of(&scenario.work) == "Done"
        },
    );
    assert!(
        !repaired.records.is_empty()
            && outcomes(&repaired)
                .iter()
                .all(|outcome| *outcome == "projected"),
        "every recovery snapshot must land: {:?}",
        repaired.records
    );
    assert_eq!(
        repaired.resolutions(),
        1,
        "the timed-out validation must discard the store"
    );
    assert_eq!(scenario.world.store_asked("initialize"), opened + 1);
    scenario.finish();
}

fn outcomes(spent: &Spent) -> Vec<&str> {
    spent
        .records
        .iter()
        .filter_map(|record| record["outcome"].as_str())
        .collect()
}

/// A plugin-agnostic command-end failure or cancellation discards the store, including its
/// Linear resolution cache. Every subsequent attempt lands its current values on a new source.
#[test]
fn command_end_refusal_and_timeout_rebuild_the_workers_store() {
    let mut states = STATES.to_vec();
    states.push(("Needs Attention", "started"));
    let scenario = launched_configured("command-end-recovery", &[], &states, true);
    let opened = || scenario.world.store_asked("initialize");
    let before = opened();
    scenario.world.store_refuses_once(
        "end_command",
        &json!({"kind":"refused", "message":"command snapshot refused"}),
    );
    let refused = scenario.replied(
        "the command-end refusal to be reported",
        &settle("done", "before command-end refusal"),
        |scenario| {
            std::fs::read_to_string(scenario.world.run_file(&scenario.run, "driver.log")).is_ok_and(
                |log| {
                    log.contains("end-command failed") && log.contains("command snapshot refused")
                },
            )
        },
    );
    assert_eq!(outcomes(&refused), ["projected"]);
    assert!(refused.records[0]["reason"].is_null());
    assert!(refused.records[0]["class"].is_null());
    let recovered = scenario.replied(
        "the change after command-end refusal to land",
        &settle("failed", "after command-end refusal"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref() == Some("after command-end refusal")
        },
    );
    assert_eq!(outcomes(&recovered), ["projected"]);
    assert_eq!(recovered.resolutions(), 1);
    assert_eq!(
        opened(),
        before + 1,
        "the boundary source must also be rebuilt"
    );

    scenario
        .linear
        .fail_next(Operation::IssueUpdate, Fault::Refuse);
    scenario.world.store_refuses_once(
        "end_command",
        &json!({"kind":"unavailable", "message":"ending after a refused write"}),
    );
    let failed_write = scenario.replied(
        "a failed write and command end to be reported independently",
        &settle("done", "a refused write before failed command end"),
        |scenario| {
            std::fs::read_to_string(scenario.world.run_file(&scenario.run, "driver.log"))
                .is_ok_and(|log| log.contains("ending after a refused write"))
        },
    );
    assert_eq!(outcomes(&failed_write), ["failed"]);
    assert_eq!(failed_write.records[0]["class"], "refused");
    assert!(failed_write.records[0]["reason"]
        .as_str()
        .is_some_and(|reason| reason.contains("task-update") && !reason.contains("end-command")));
    let recovered = scenario.replied(
        "the values after the failed write to reach a rebuilt store",
        &note("after the write and command end both failed"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref()
                == Some("a refused write before failed command end")
        },
    );
    assert_eq!(outcomes(&recovered), ["projected"]);
    assert_eq!(recovered.resolutions(), 1);
    assert_eq!(opened(), before + 2);

    let hold_name = format!("{}.end_command", crate::harness::SCRIPTED_KEY);
    let meeting = scenario.world.rendezvous(&hold_name);
    let recorded = scenario.records().len();
    scenario.reply(&settle("failed", "before command-end deadline"));
    let held = meeting.arrived();
    scenario.until("the command-end deadline to be reported", |scenario| {
        std::fs::read_to_string(scenario.world.run_file(&scenario.run, "driver.log"))
            .is_ok_and(|log| log.contains("end-command exceeded"))
            && scenario.records().len() > recorded
    });
    let cancelled = &scenario.records()[recorded..];
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0]["outcome"], "projected");
    assert!(cancelled[0]["reason"].is_null());
    scenario.world.unscript(&format!("{hold_name}.rendezvous"));
    held.release();
    scenario.at_rest("the command-end deadline");
    let before = opened();
    let recovered = scenario.replied(
        "the change after command-end cancellation to land",
        &settle("done", "after command-end deadline"),
        |scenario| {
            scenario.evidence_on(&scenario.work).as_deref() == Some("after command-end deadline")
        },
    );
    assert_eq!(outcomes(&recovered), ["projected"]);
    assert_eq!(recovered.resolutions(), 1);
    assert_eq!(opened(), before + 1);
    scenario.finish();
}

/// The loopback GitHub GraphQL endpoint `tests/e2e/github_loopback.py` serves, for as long as
/// this is held: its directory, where `state.json` holds the board and every request it was
/// sent, and the URL it listens on.
struct GitHubEndpoint {
    child: std::process::Child,
    dir: PathBuf,
    url: String,
}

impl GitHubEndpoint {
    /// Serve the board under `world`, seeding each issue named in `seeded` with that caller
    /// metadata beside its own.
    fn serve(world: &World, seeded: &Value) -> Self {
        let dir = world.root.join("github-endpoint");
        std::fs::create_dir_all(&dir).expect("endpoint directory");
        let seed = dir.join("seeded.json");
        std::fs::write(&seed, seeded.to_string()).expect("seeded metadata");
        let child = std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/e2e/github_loopback.py"))
            .arg(&dir)
            .arg(&seed)
            .spawn()
            .expect("loopback endpoint starts");
        world.until("the GitHub endpoint to listen", |_| {
            dir.join("endpoint").is_file()
        });
        let url = std::fs::read_to_string(dir.join("endpoint")).expect("endpoint URL");
        Self { child, dir, url }
    }

    fn state(&self) -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(self.dir.join("state.json")).expect("endpoint state"),
        )
        .expect("endpoint JSON")
    }
}

impl Drop for GitHubEndpoint {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A reused GitHub Projects source must release its command snapshot: a metadata-only
/// settlement keeps both a person's new body and a Status move made since the previous one.
#[test]
fn a_reused_github_store_keeps_body_edits_and_status_moves_between_settlements() {
    let world = World::new("github-command-boundary")
        .with_env("ONEPIPELINE_LOOPBACK_BOARD_TOKEN", "loopback-token");
    world.script("anchor.wait", "hold");
    let served = GitHubEndpoint::serve(&world, &json!({}));
    let endpoint = served.url.clone();
    let rejected = std::process::Command::new("python3").args(["-c", r#"
import json, sys, urllib.request, urllib.error
base = sys.argv[1]
invalid = [
    ("/graphql", [], {}),
    ("/graphql", {"query": 1, "variables": {}}, {}),
    ("/graphql", {"query": "query {}", "variables": []}, {}),
    ("/human", {"body": "edit", "status": "Queued", "extra": True}, {}),
    ("/human", {"body": 1, "status": "Queued"}, {}),
    ("/human", {"body": "edit", "status": "Missing"}, {}),
    ("/graphql", {"query": "mutation {updateIssue(input:$input){issue{id}}}",
                  "variables": {"input": {"id": "missing", "body": "wrong"}}}, {}),
    ("/graphql", {"query": "mutation {updateProjectV2ItemFieldValue(input:$input){projectV2Item{id}}}",
                  "variables": {"input": {"projectId": "BOARD", "itemId": "ITEM-work", "fieldId": "STATUS",
                                           "value": {"singleSelectOptionId": "Missing"}}}}, {}),
    ("/graphql", {}, {"Content-Length": "262145"}),
]
for path, body, headers in invalid:
    request = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers=headers)
    try:
        urllib.request.urlopen(request).read()
    except urllib.error.HTTPError as error:
        assert error.code == 400, error.code
    else:
        raise AssertionError("invalid input was accepted")
def graphql(query, variables):
    request = urllib.request.Request(base + "/graphql", data=json.dumps({"query":query,"variables":variables}).encode())
    return json.loads(urllib.request.urlopen(request).read())["data"]
mutation = "mutation {updateIssue(input:$input){issue{id}}}"
graphql(mutation, {"input":{"id":"work","title":"edited title"}})
assert graphql("query {node(id:$id){id}}", {"id":"work"})["node"]["title"] == "edited title"
graphql(mutation, {"input":{"id":"work","title":"work"}})
"#, &endpoint]).status().expect("invalid requests reach the loopback boundary");
    assert!(
        rejected.success(),
        "the loopback must refuse malformed requests before a valid run"
    );
    let launch = world.root.join("github-launch");
    std::fs::create_dir_all(&launch).expect("launch directory");
    std::fs::write(launch.join("onetaskgraph.yaml"), format!(
        "sources:\n  board:\n    plugin: github-projects\n    config:\n      owner: generic\n      project_number: 1\n      token_env: ONEPIPELINE_LOOPBACK_BOARD_TOKEN\n      endpoint: {endpoint}/graphql\n      status_mapping:\n        todo: Todo\n        queued: Queued\n        in-progress: In Progress\n        unknown: Needs Attention\n        done: Done\n        cancelled: Canceled\n"
    )).expect("store configuration");
    let started = from_the_launch_directory(&world, &launch, &["start", "board:plan", "--detach"]);
    world.run_on(started, "start --detach").exited(0);
    let run = "github-edits";
    let read = || served.state();
    world.until("the launch projections", |_| {
        read()["issues"]["anchor"]["status"] == "In Progress"
    });
    let reply = |commands: Vec<Value>, evidence| {
        world
            .run_with_stdin(
                &["reply", run],
                &json!({"version": 2, "commands": commands}).to_string(),
            )
            .exited(0);
        world.until("the settlement metadata to land", |_| {
            read()["issues"]["work"]["body"]
                .as_str()
                .is_some_and(|body| body.contains(evidence))
        });
        world.until("the projection record to land", |world| {
            std::fs::read_to_string(
                world.run_file(run, onepipeline::cli::WRITEBACK_PROJECTIONS_FILE),
            )
            .is_ok_and(|text| {
                text.lines()
                    .last()
                    .is_some_and(|line| line.contains("projected"))
            })
        });
        std::thread::sleep(Duration::from_millis(1500));
    };
    reply(
        vec![settle("failed", "first board settlement")],
        "first board settlement",
    );
    let body = read()["issues"]["work"]["body"]
        .as_str()
        .expect("body")
        .to_owned();
    let edited = format!("A person's edit above the task.\n{body}");
    let human = std::process::Command::new("python3").args(["-c",
        "import sys, urllib.request; urllib.request.urlopen(urllib.request.Request(sys.argv[1], data=sys.argv[2].encode(), headers={'Content-Type':'application/json'})).read()",
        &format!("{endpoint}/human"), &json!({"body": edited, "status": "Queued"}).to_string()
    ]).status().expect("person's edit reaches the endpoint");
    assert!(human.success());
    let mut second = settle("done", "second board settlement");
    second["id"] = json!("other");
    let mut work_note = note("a note beside the second board settlement");
    work_note["id"] = json!("work");
    reply(
        vec![second, work_note],
        "a note beside the second board settlement",
    );
    let current = read();
    assert!(
        current["issues"]["work"]["body"]
            .as_str()
            .is_some_and(|body| body.starts_with("A person's edit above the task.")),
        "{current}"
    );
    assert_eq!(current["issues"]["work"]["status"], "Queued", "{current}");
    world.release("anchor.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
}

/// The `Host` value the adopting host files a follow-up with, under the key and path its
/// `followups` source projects onto the board.
const FOLLOW_UP_HOST: &str = "build-box-7.example";

/// Every board-field write in `requests` to one item, in order: the field's id and the value
/// written, `Value::Null` for a clear. The batched field-update document is refused by the
/// endpoint, so a write the store batched fails the journey rather than passing unread here.
fn field_writes(requests: &[Value], item: &str) -> Vec<(String, Value)> {
    let mut writes = Vec::new();
    for request in requests {
        let query = request["query"].as_str().unwrap_or_default();
        let variables = &request["variables"];
        let inputs: Vec<(&Value, bool)> = if query.contains("updateProjectV2ItemFieldValue(") {
            vec![(&variables["input"], false)]
        } else if query.contains("clearProjectV2ItemFieldValue(") {
            vec![(&variables["input"], true)]
        } else {
            Vec::new()
        };
        for (input, clear) in inputs {
            if input["itemId"] == format!("ITEM-{item}") {
                let value = if clear {
                    Value::Null
                } else {
                    input["value"].clone()
                };
                writes.push((
                    input["fieldId"].as_str().unwrap_or_default().to_owned(),
                    value,
                ));
            }
        }
    }
    writes
}

fn body_writes(requests: &[Value], issue: &str) -> Vec<String> {
    requests
        .iter()
        .filter(|request| {
            request["query"]
                .as_str()
                .is_some_and(|query| query.contains("updateIssue("))
                && request["variables"]["input"]["id"] == issue
        })
        .filter_map(|request| {
            request["variables"]["input"]["body"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

/// A plan held on a GitHub board whose source projects `orchestrator.follow-up.host` onto the
/// board's `Host` text field — the `metadata_fields` entry the adopting host's `followups` source
/// ships — is read by the engine without refusal, and settled through it. Each settlement's
/// write-back reaches its own item through the linked store, and neither writes `Host`: the field
/// follows the value only when a write changes it, and a settlement changes no
/// `orchestrator.follow-up` — the copy that files a follow-up is what puts it there. The log's
/// reader is shown able to see a `Host` write, so its silence is evidence.
#[test]
fn a_github_source_projecting_a_follow_ups_host_is_read_and_settled_without_writing_host() {
    let world = World::new("github-host-field")
        .with_env("ONEPIPELINE_LOOPBACK_BOARD_TOKEN", "loopback-token");
    world.script("anchor.wait", "hold");
    let served = GitHubEndpoint::serve(
        &world,
        &json!({"work": {"orchestrator.follow-up": {"host": FOLLOW_UP_HOST}}}),
    );
    let launch = world.root.join("github-launch");
    std::fs::create_dir_all(&launch).expect("launch directory");
    std::fs::write(launch.join("onetaskgraph.yaml"), format!(
        "sources:\n  board:\n    plugin: github-projects\n    config:\n      owner: generic\n      project_number: 1\n      token_env: ONEPIPELINE_LOOPBACK_BOARD_TOKEN\n      endpoint: {}/graphql\n      metadata_fields: [{{field: Host, key: orchestrator.follow-up, path: [host]}}]\n      status_mapping:\n        todo: Todo\n        queued: Queued\n        in-progress: In Progress\n        unknown: Needs Attention\n        done: Done\n        cancelled: Canceled\n",
        served.url
    )).expect("store configuration");
    let started = from_the_launch_directory(&world, &launch, &["start", "board:plan", "--detach"]);
    world.run_on(started, "start --detach").exited(0);
    let run = "github-edits";
    world.until("the launch projections", |_| {
        served.state()["issues"]["anchor"]["status"] == "In Progress"
    });
    let requests = || {
        served.state()["requests"]
            .as_array()
            .expect("a request log")
            .clone()
    };
    // Settle one task, and answer what the endpoint was sent from the reply until the write-back
    // came to rest, once its status and its evidence are both on the board.
    let settled = |id: &str| -> (String, Vec<Value>) {
        let sent = requests().len();
        let evidence = format!("{id} settled on the board");
        let mut command = settle("done", &evidence);
        command["id"] = json!(id);
        world
            .run_with_stdin(
                &["reply", run],
                &json!({"version": 2, "commands": [command]}).to_string(),
            )
            .exited(0);
        world.until("the settlement to land", |_| {
            let state = served.state();
            state["issues"][id]["status"] == "Done"
                && state["issues"][id]["body"]
                    .as_str()
                    .is_some_and(|body| body.contains(&evidence))
        });
        world.until("the projection record to land", |world| {
            std::fs::read_to_string(
                world.run_file(run, onepipeline::cli::WRITEBACK_PROJECTIONS_FILE),
            )
            .is_ok_and(|text| {
                text.lines()
                    .last()
                    .is_some_and(|line| line.contains("projected"))
            })
        });
        std::thread::sleep(Duration::from_millis(1500));
        (evidence, requests()[sent..].to_vec())
    };

    for id in ["work", "other"] {
        let (evidence, spent) = settled(id);
        assert_eq!(
            field_writes(&spent, id),
            [("STATUS".to_owned(), json!({"singleSelectOptionId": "Done"}))],
            "{id}'s settlement must move its Status and write no other board field"
        );
        assert!(
            body_writes(&spent, id)
                .iter()
                .any(|body| body.contains(&evidence)),
            "{id}'s settlement evidence never reached its issue: {spent:?}"
        );
    }
    let state = served.state();
    let log = requests();
    for id in ["anchor", "work", "other"] {
        assert!(
            field_writes(&log, id)
                .iter()
                .all(|(field, _)| field != "HOST"),
            "the run wrote {id}'s Host field: {:?}",
            field_writes(&log, id)
        );
        assert_eq!(state["issues"][id]["host"], Value::Null, "{state}");
    }
    assert!(
        state["issues"]["work"]["body"]
            .as_str()
            .is_some_and(|body| body.contains(FOLLOW_UP_HOST)),
        "the follow-up's own host value must survive its settlement: {state}"
    );

    world.release("anchor.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });

    // The reader above is not blind to a Host write: one sent as the store sends a lone field
    // write is read back as what it set.
    let wrote = std::process::Command::new("python3").args(["-c", r#"
import json, sys, urllib.request
host = {"projectId": "BOARD", "itemId": "ITEM-other", "fieldId": "HOST", "value": {"text": sys.argv[2]}}
query = "mutation($input:UpdateProjectV2ItemFieldValueInput!){updateProjectV2ItemFieldValue(input:$input){projectV2Item{id}}}"
body = json.dumps({"query": query, "variables": {"input": host}}).encode()
urllib.request.urlopen(urllib.request.Request(sys.argv[1] + "/graphql", data=body)).read()
"#, &served.url, FOLLOW_UP_HOST]).status().expect("the Host write reaches the endpoint");
    assert!(wrote.success());
    assert_eq!(
        field_writes(&requests(), "other").last(),
        Some(&("HOST".to_owned(), json!({"text": FOLLOW_UP_HOST})))
    );
    assert_eq!(served.state()["issues"]["other"]["host"], FOLLOW_UP_HOST);
}

// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
