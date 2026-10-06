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
const SOURCE: &str = "hellopatient";

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
fn launched(name: &str, extra: &[&str]) -> Scenario {
    let world = World::new(name).with_env(KEY_ENV, "a-loopback-key");
    world.script("anchor.wait", "hold");
    let linear = Linear::serve(
        STATES,
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
    std::fs::write(launch.join("onetaskgraph.yaml"), configuration(&linear))
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
    scenario
}

/// A command whose store is configured by the launch directory's `onetaskgraph.yaml` alone:
/// every setting this world's commands carry for their own plans source is taken away.
fn from_the_launch_directory(world: &World, dir: &Path, args: &[&str]) -> std::process::Command {
    let mut command = world.cmd(args);
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

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the two journeys below read a
// plan out of a Linear project and write each settlement back through the linked Linear plugin,
// exercising `taskgraph`, `writeback`, `edits` and `driver` together, so the crate is the
// narrowest edge they can honestly sit behind — the grounds `mod multi_source` records in
// `main.rs`. The second waits past the write-back's sixty-second floor by construction, as
// `writeback_budget.rs`'s journeys do: a write held past its deadline cannot be observed sooner.
/// Six consecutive settlements, each projected in an attempt of its own, each landing its own
/// status and settlement record and never an earlier one's: the mutation Linear recorded for it
/// carries that settlement's state and evidence, and the run's landed baseline says the same.
/// Each attempt sends one read of the issue — the description its metadata is merged into, which
/// keeps whatever a person wrote there — and one `issueUpdate`, and at most one resolution read.
/// A state added to the team once the run is under way is found and written.
///
/// What the most expensive of those attempts after the first cost is the
/// `linear-requests-per-writeback-settlement` budget's measure, written to the file
/// `ONEBUDGETSPEC_RESULT` names when the budget's command runs this journey.
#[test]
fn consecutive_linear_settlements_each_land_their_own_status_and_metadata_in_their_own_attempt() {
    let scenario = launched("linear-settlements", &[]);
    let mut costs = Vec::new();
    for (step, outcome) in ["done", "failed", "done", "failed", "done", "failed"]
        .into_iter()
        .enumerate()
    {
        if step == 1 {
            // Added after the run's first writes resolved the team's states, as a person adds a
            // state in Linear's settings: the mapping names it, and the next write finds it.
            scenario.linear.add_state("Needs Attention", "started");
        }
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
        costs.push(spent.requests.len());
    }
    let highest = costs.iter().skip(1).copied().max().unwrap_or_default();
    if let Some(result) = std::env::var_os("ONEBUDGETSPEC_RESULT") {
        let detail = format!(
            "requests the loopback Linear served for each of {} consecutive settlements, each in \
             its own attempt: {costs:?}; the first is not counted",
            costs.len()
        );
        std::fs::write(
            result,
            crate::budget_result::reported(highest, &detail).to_string(),
        )
        .expect("the budget's result is written");
    }
    scenario.finish();
}

/// Each way a Linear call fails leaves the next attempt landing the values current when it is
/// made: a connection Linear closes mid-read is retried on the schedule, a write Linear refuses
/// — one carrying only metadata, and one carrying a state — is attempted again when the graph
/// next changes, a write held past its deadline is cancelled and the attempt after it lands, and
/// an attempt made while the launch directory's `onetaskgraph.yaml` cannot be read is recorded as
/// that failure, with the attempt after its repair landing.
#[test]
fn a_linear_destination_lands_current_values_after_each_way_a_call_fails() {
    let scenario = launched("linear-failures", &["--writeback-item-budget", "1"]);
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

    // A configuration that cannot be read: recorded as that failure, and the attempt after its
    // repair lands what changed meanwhile.
    let document = scenario.launch.join("onetaskgraph.yaml");
    let configured = std::fs::read_to_string(&document).expect("the configuration reads");
    std::fs::remove_file(&document).expect("the configuration is taken away");
    std::fs::create_dir(&document).expect("a directory stands in its place");
    let unread = scenario.replied(
        "an attempt over an unreadable configuration to be recorded",
        &settle("failed", "while the configuration could not be read"),
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
                && scenario.linear.state_of(&scenario.work) == "Needs Attention"
        },
    );
    assert_eq!(outcomes(&repaired), ["projected"], "{:?}", repaired.records);

    // A write held past its deadline — the floor and one item's budget of a second — is
    // cancelled, and the attempt after it lands.
    scenario
        .linear
        .fail_next(Operation::IssueUpdate, Fault::Hold);
    let cancelled = scenario.replied(
        "a write held past its deadline to be cancelled and then landed",
        &settle("done", "after a write held past its deadline"),
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
    assert_eq!(scenario.linear.state_of(&scenario.work), "Done");
    scenario.finish();
}

// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

fn outcomes(spent: &Spent) -> Vec<&str> {
    spent
        .records
        .iter()
        .filter_map(|record| record["outcome"].as_str())
        .collect()
}
