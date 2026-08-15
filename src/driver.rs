//! The driver contract: launching a run, owning it, attaching to it, and
//! handing it to a fresh driver when its own dies.
//!
//! `onepipeline start` launches the dag-scope agent graph — the shipped default
//! is an `orchestrator` member plus a resettable-cron `check-in` member — and
//! that orchestrator drives the engine verbs under the run's ownership lock.
//! This crate never decides what the graph should be; it schedules, dispatches,
//! transitions, and closes out.
//!
//! Runs belong to the session that launched them. `stop` refuses another
//! session's run and `--force` names the owner; `adopt` has no `--force` at all,
//! because taking over ongoing work is exactly the case where a second opinion
//! is worth more than an override.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::agentgraph;
use crate::channel::{ChannelState, Command, Reply, Surface, SurfaceKind};
use crate::cli::{
    AttestArgs, ChannelCommand, Cli, OptionalRunArgs, ReplyArgs, RoundCommand, RunArgs, RunsArgs,
    StartArgs, StopArgs, SurfaceArgs, TelemetryArgs, TranscriptArgs,
};
use crate::concurrency::{self, Liveness, State};
use crate::edits;
use crate::engine;
use crate::error::{Error, Result, EXIT_NOTHING_DRIVING, EXIT_QUEUED, EXIT_SUCCESS};
use crate::graph::{self, GraphState};
use crate::journal::{self, Journal};
use crate::ledger::{self, LaunchRecord, RunPaths};
use crate::plan::Plan;
use crate::sys;
use crate::telemetry;
use crate::views::{self, RunView};

/// The environment variable naming the dag-scope agent graph `start` launches.
pub const DAG_GRAPH_ENV: &str = "ONEPIPELINE_DAG_GRAPH";

/// The dag-scope agent graph shipped with this crate.
pub const DEFAULT_DAG_GRAPH: &str = "graphs/dag-scope.yaml";

/// How often an attach re-reads the run to see whether it has settled.
const ATTACH_POLL: Duration = Duration::from_millis(50);

/// How long an attach collects a departed driver's last envelopes before
/// settling without them.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Execute one parsed command line.
pub fn dispatch(cli: Cli) -> Result<i32> {
    use crate::cli::Command as Verb;
    match cli.command {
        Verb::Start(args) => start(&args),
        Verb::Adopt(args) => adopt(&args),
        Verb::Round(RoundCommand::Run(args)) => {
            Ok(engine::round_run(&resolve(&args.run)?)?.exit_code())
        }
        Verb::Round(RoundCommand::Next(args)) => {
            engine::round_next(&resolve(&args.run)?)?;
            Ok(EXIT_SUCCESS)
        }
        Verb::Channel(ChannelCommand::Serve(args)) => serve(&args),
        Verb::Next(args) => next(&args),
        Verb::Reply(args) => reply(&args),
        Verb::Surface(args) => surface(&args),
        Verb::Attest(args) => attest(&args),
        Verb::Stop(args) => stop(&args),
        Verb::Runs(args) => runs(&args),
        Verb::Status(args) => report(&args, views::status),
        Verb::Host => report(&OptionalRunArgs { run: None }, views::host),
        Verb::Monitor(args) => {
            print!("{}", views::monitor(&RunView::open(&resolve(&args.run)?)?));
            Ok(EXIT_SUCCESS)
        }
        Verb::Results(args) => {
            print!("{}", views::results(&RunView::open(&resolve(&args.run)?)?));
            Ok(EXIT_SUCCESS)
        }
        Verb::Goals(args) => report(&args, views::goals),
        Verb::Transcript(args) => transcript(&args),
        Verb::Telemetry(args) => report_telemetry(&args),
        Verb::Drive(args) => {
            agentgraph::drive(&args.graph, &args.task, &args.dir, &args.labels, &args.sets)
        }
    }
}

/// The paths for a run that exists, or a refusal naming the root searched.
fn resolve(run: &str) -> Result<RunPaths> {
    // Before it is joined onto anything. A run id that navigates is not a run
    // this root holds, and reporting it as merely missing would leave a caller
    // believing the path they typed was looked for where they meant.
    if !ledger::is_valid_run_id(run) {
        return Err(Error::Invalid(format!(
            "'{run}' is not a run id: a run id names one directory under the runs root, \
             so it may not be a path"
        )));
    }
    let paths = RunPaths::new(run);
    if !paths.exists() {
        return Err(Error::NoSuchRun {
            run: run.to_string(),
            root: ledger::runs_root(),
        });
    }
    Ok(paths)
}

fn dag_graph() -> String {
    std::env::var(DAG_GRAPH_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_DAG_GRAPH.to_string())
}

fn launch_dir() -> Result<PathBuf> {
    std::env::current_dir()
        .map_err(|error| Error::Invalid(format!("cannot read the launch directory: {error}")))
}

/// The directory a launch replays, or this process's own when the record
/// predates the field.
///
/// A record written before `dir` existed carries none, and the reading a
/// driver gave such a run was its own working directory — so that is what it
/// keeps getting, rather than a directory this build invented for it.
///
/// A recorded one is **external input**: the launch record is a file on disk
/// that this process re-reads, and this field is the directory every member of
/// the run works in. Both refusals below are the invariant the field is
/// documented to hold, checked where it is read rather than assumed: a relative
/// value would resolve against whichever process spawns the graph — the exact
/// ambiguity the field exists to remove — and one that is not a directory fails
/// each member separately, deep inside the sibling, with nothing naming the
/// record it came from.
fn recorded_dir(record: &LaunchRecord) -> Result<PathBuf> {
    if record.dir.as_os_str().is_empty() {
        return launch_dir();
    }
    if !record.dir.is_absolute() {
        return Err(Error::Invalid(format!(
            "run '{}' records the relative working directory '{}'; a run's directory has to be \
             absolute, because the process that resolves it is not the one that launched it",
            record.run_id,
            record.dir.display()
        )));
    }
    if !record.dir.is_dir() {
        return Err(Error::Invalid(format!(
            "run '{}' records the working directory '{}', which is not a directory on {}",
            record.run_id,
            record.dir.display(),
            sys::hostname()
        )));
    }
    Ok(record.dir.clone())
}

/// Resolve a relative filesystem graph reference at the launch boundary,
/// before any session worktree exists. URLs and absolute paths retain their
/// established oneagentgraph validation semantics and exact spelling.
// llmlint: ignore-block[invalid_states_unrepresentable] the resolved graph stays a
// string from this source through LaunchRecord because that durable internal schema and
// oneagentgraph's transparent ConfigRef are already string-valued. A second newtype would
// duplicate the sibling type without adding an invariant: relative references are made
// absolute here, and the nonempty launch-record invariant is checked before every round.
fn resolve_graph(reference: &str, base: &Path) -> Result<String> {
    // llmlint: ignore-block[boundary_inputs_validated] absolute paths and URLs are
    // oneagentgraph's existing input boundary: it reads/fetches them and returns its own
    // config refusal. This boundary resolves only relative paths because onepipeline is
    // the sole owner of their launch-directory base; validating absolute references here
    // would change the documented and e2e-guarded sibling-error contract.
    if reference.starts_with("https://") || Path::new(reference).is_absolute() {
        return Ok(reference.to_string());
    }
    // llmlint: ignore-end[boundary_inputs_validated]
    let resolved = base.join(reference);
    std::fs::File::open(&resolved).map_err(|error| {
        Error::Invalid(format!(
            "cannot read graph '{}' resolved against launch directory '{}': {error}",
            reference,
            base.display()
        ))
    })?;
    Ok(resolved.to_string_lossy().into_owned())
}
// llmlint: ignore-end[invalid_states_unrepresentable]

fn resolve_plan_graphs(plan: &mut Plan, base: &Path) -> Result<()> {
    for node in &mut plan.tasks {
        if let Some(reference) = &mut node.agent_graph {
            reference.0 = resolve_graph(&reference.0, base)?;
        }
        if let Some(steps) = &mut node.steps {
            for step in steps {
                if let Some(reference) = &mut step.agent_graph {
                    reference.0 = resolve_graph(&reference.0, base)?;
                }
            }
        }
    }
    Ok(())
}

/// Mint a run id from the plan's name or the file's, made unique.
fn mint_run_id(plan: &Plan, path: &Path, root: &Path) -> String {
    let base = plan
        .name
        .clone()
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.trim_end_matches(".plan").to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "run".to_string());
    let base: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if !root.join(&base).exists() {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !root.join(candidate).exists())
        .unwrap_or(base)
}

/// `onepipeline start`.
fn start(args: &StartArgs) -> Result<i32> {
    let mut plan = Plan::load(&args.plan)?;
    graph::validate(&plan)?;
    let launch_dir = launch_dir()?;
    let graph_ref = resolve_graph(&dag_graph(), &launch_dir)?;
    let node_graph_ref = resolve_graph(&engine::configured_node_graph(), &launch_dir)?;
    resolve_plan_graphs(&mut plan, &launch_dir)?;

    let root = ledger::runs_root();
    let run = mint_run_id(&plan, &args.plan, &root);
    let holders = concurrency::holders(&plan)?;
    for holder in holders
        .iter()
        .filter(|holder| holder.state == State::Open && holder.liveness == Liveness::Stale)
    {
        eprintln!(
            "onepipeline: stale repository holder: identity '{}' session '{}' owner_pid {}; proceeding",
            holder.identity, holder.token.0, holder.owner_pid
        );
    }
    let live: Vec<_> = holders
        .iter()
        .filter(|holder| holder.state == State::Open && holder.liveness == Liveness::Live)
        .collect();
    if !live.is_empty() && !args.acknowledge_concurrent {
        let shared = live
            .iter()
            .map(|holder| {
                format!(
                    "identity '{}' held by session '{}' (owner_pid {})",
                    holder.identity, holder.token.0, holder.owner_pid
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::Refused(format!(
            "concurrent project work refused for run '{run}': {shared}; pass --acknowledge-concurrent to proceed deliberately"
        )));
    }
    if !live.is_empty() {
        let shared = live
            .iter()
            .map(|holder| {
                format!(
                    "'{}' with session '{}' (owner_pid {})",
                    holder.identity, holder.token.0, holder.owner_pid
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "onepipeline: --acknowledge-concurrent: launch '{run}' is proceeding alongside live run(s): {shared}"
        );
    }
    let paths = RunPaths::under(&root, &run);
    paths.create()?;
    ledger::write_json(&paths.plan(), &plan)?;

    let mut record = LaunchRecord {
        run_id: run.clone(),
        plan: args.plan.clone(),
        // Absolute, once, here: this is the only process that knows where the
        // operator launched from, and every later driver — including the one a
        // fresh `adopt` starts from some other directory — replays this value
        // rather than reading its own.
        dir: launch_dir.clone(),
        graph: graph_ref.clone(),
        // Replaced below by the graph run's own id, which does not exist until
        // the launch below has produced it.
        graph_run: String::new(),
        node_graph: node_graph_ref,
        launcher: sys::launcher(),
        session: sys::launching_session(),
        // Replaced below by the graph process's own pid. What drives the run
        // is that process, not this one: `--detach` returns immediately, so a
        // record naming this pid would read as a dead driver the moment it did.
        // Until that process exists, this one is what is driving the run, and
        // the record has to say so — see `launch_graph`'s ordering.
        pid: sys::pid(),
        host: sys::hostname(),
        started_at: sys::now_rfc3339(),
        round_budget: args.round_budget,
        heartbeat_interval: args.heartbeat_interval,
        dag_sets: args.dag_sets.clone(),
        node_sets: args.node_sets.clone(),
        adoptions: 0,
    };

    let mut open = Journal::open(&paths);
    if !live.is_empty() {
        open.emit(
            journal::PipelineKind::ConcurrentAcknowledged,
            journal::labels(&run, None, None),
            journal::payload(&[
                (
                    "shared_identities",
                    json!(live
                        .iter()
                        .map(|holder| holder.identity.to_string())
                        .collect::<Vec<_>>()),
                ),
                (
                    "runs",
                    json!({
                        "launching": run,
                        "holding_sessions": live
                            .iter()
                            .map(|holder| holder.token.0.clone())
                            .collect::<Vec<_>>(),
                    }),
                ),
                (
                    "holders",
                    json!(live
                        .iter()
                        .map(|holder| json!({
                            "session": holder.token.0.clone(),
                            "owner_pid": holder.owner_pid,
                        }))
                        .collect::<Vec<_>>()),
                ),
            ]),
        )?;
    }
    open.emit(
        journal::PipelineKind::RunStarted,
        journal::labels(&run, None, None),
        journal::payload(&[
            ("plan", json!(plan)),
            ("graph", json!(graph_ref)),
            // Stated on the run's own first record, so a reader never has to
            // infer which directory a run's members worked in from the process
            // that happened to launch them.
            ("dir", json!(launch_dir)),
            ("round_budget", json!(args.round_budget)),
            ("heartbeat_interval", json!(args.heartbeat_interval)),
        ]),
    )?;

    // The record is durable *before* the process that reads it exists. The
    // driver's first act is `onepipeline round run`, which opens the launch
    // record, so a driver that wins the race against its own launcher dies on a
    // file nobody had written yet — and the run then sits at `run-started`
    // with nothing driving it. Writing it twice is the price of that ordering:
    // the first record names this process, which is what drives the run until
    // the graph process it starts takes over.
    ledger::write_json(&paths.launch(), &record)?;
    if args.detach {
        // Before the driver exists, and only on this path: a detaching launcher
        // starts nothing else, and the driver it is about to start must not
        // hold this process's streams open behind it. See
        // [`sys::disown_standard_handles`] for what inherits what.
        sys::disown_standard_handles();
    }
    let log = paths.driver_log();
    let mut launched = launch_graph(
        &paths,
        &record,
        plan.goal.as_ref().map(|goal| goal.text.as_str()),
        if args.detach {
            agentgraph::GraphOutput::Logged(&log)
        } else {
            agentgraph::GraphOutput::Relayed
        },
    )?;
    record.pid = launched.pid();
    record.graph_run = launched
        .run_id()
        .map(ToString::to_string)
        .unwrap_or_default();
    ledger::write_json(&paths.launch(), &record)?;

    if args.detach {
        // The launch record and nothing else: a run that should go unattended.
        println!(
            "{}",
            json!({
                "run_id": run,
                "pid": launched.pid(),
                "commands": {
                    "next": format!("onepipeline next {run}"),
                    "monitor": format!("onepipeline monitor {run}"),
                },
            })
        );
        return Ok(EXIT_SUCCESS);
    }
    attach(&paths, Some(&mut launched))
}

/// Start the dag-scope graph that drives the run.
///
/// `output` is the launcher's promise about itself: an attaching launcher stays
/// and relays what the driver produces, and a detaching one is about to exit, so
/// the driver is given a file instead of a pipe whose reader is going away.
///
/// The directory comes from the **record** rather than from this process, which
/// is what makes `start` and `adopt` name the same place for one run: an
/// adoption runs wherever the operator happened to be, and the run's members
/// must not move with it. See [`LaunchRecord::dir`].
///
/// `goal` is the plan's, and it is the only thing besides the run id that
/// reaches the graph's task — see [`run_description`] for why that is all of it.
fn launch_graph(
    paths: &RunPaths,
    record: &LaunchRecord,
    goal: Option<&str>,
    output: agentgraph::GraphOutput<'_>,
) -> Result<agentgraph::GraphRun> {
    let task = run_description(&paths.run, goal);
    let mut launched = agentgraph::GraphRun::start(
        &record.graph,
        &task,
        &recorded_dir(record)?,
        &journal::labels(&paths.run, None, None),
        &[
            (agentgraph::RUN_ID_ENV.to_string(), paths.run.clone()),
            (
                ledger::RUNS_DIR_ENV.to_string(),
                ledger::runs_root().to_string_lossy().into_owned(),
            ),
        ],
        &record.dag_sets,
        output,
    )?;
    // A launcher is the one caller that never waits for what it started, so a
    // graph that refused this launch would otherwise be reported as a running
    // driver — an exit 0 and a pid for a process that is already gone.
    launched.confirm_started()?;
    Ok(launched)
}

/// What the run is, for the one `--task` the dag-scope graph is launched with.
///
/// **Role-neutral on purpose:** `oneagentgraph` hands this to every member of
/// the graph carrying none of its own, so a role stated here is stated to
/// members whose job is not the driver's — and the shipped pacemaker once acted
/// on one. Which member drives is the consuming graph's to say, in that member's
/// persona or in its own `task` composed from `{task}`, which expands to this
/// text from graph schema
/// [`FIRST_TASK_TOKEN_VERSION`](oneagentgraph::config::FIRST_TASK_TOKEN_VERSION)
/// onwards.
fn run_description(run: &str, goal: Option<&str>) -> String {
    // A goal that is here has words in it: [`graph::validate`] refuses one whose
    // text is blank, so the only case to answer for is a plan that stated none.
    let goal = goal.unwrap_or(crate::plan::NO_GOAL);
    format!("onepipeline run `{run}`.\n\nGoal: {goal}")
}

/// How an attach ended.
///
/// **Settled** is a property of the run: it is no longer advancing on its own,
/// so the next move is the planner's. Deliberately neither "the round finished"
/// — which returns while the driver is still scheduling — nor "the driver
/// exited", which never returns on the ordinary case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settlement {
    /// The graph completed successfully.
    Complete,
    /// A **blocking** planner surface is pending: the run will not move until a
    /// reply answers it.
    AwaitingPlanner,
    /// Nothing is driving the run.
    Unattended,
}

impl Settlement {
    fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::AwaitingPlanner => "awaiting-planner",
            Self::Unattended => "unattended",
        }
    }

    fn exit_code(self) -> i32 {
        match self {
            // Exits non-zero because it is the state a planner must intervene
            // in, and because a launch that parked reads exactly like one that
            // is merely quiet to anyone who is not watching the stream.
            Self::Unattended => EXIT_NOTHING_DRIVING,
            _ => EXIT_SUCCESS,
        }
    }
}

/// Stream the run's events and return when it settles.
///
/// The graph process is drained on a thread of its own. Reading it inline would
/// make the attach wait for the driver to *exit*, which is exactly the reading
/// the settlement contract rejects: on the ordinary case that process stays
/// alive holding a question nobody is answering.
fn attach(paths: &RunPaths, launched: Option<&mut agentgraph::GraphRun>) -> Result<i32> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut driver = launched;
    if let Some(run) = driver.as_deref_mut() {
        let events = run.events();
        std::thread::Builder::new()
            .name(format!("attach-{}", paths.run))
            .spawn(move || {
                for envelope in events.flatten() {
                    if tx.send(envelope).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| Error::Invalid(format!("cannot start the attach relay: {e}")))?;
    }
    let mut reported = 0usize;
    let mut journal = Journal::open(paths);

    loop {
        // Everything the graph process emits joins the merged store, so an
        // attach and a later replay see the same stream.
        while let Ok(envelope) = rx.try_recv() {
            journal.relay(&envelope)?;
        }

        // Asked *before* the state is read, so the two cannot disagree in the
        // one direction that matters. A driver this process started and then
        // collected is proof that nothing is driving the run any more —
        // stronger than probing its pid, which a zombie would answer as alive.
        // Reading the state afterwards means the state is at least as new as
        // that proof, so a run its driver finished and exited on settles as the
        // `complete` it is rather than as an `unattended` this loop merely
        // looked at too early.
        let driver_gone = driver
            .as_deref_mut()
            .is_some_and(agentgraph::GraphRun::has_exited);
        if driver_gone {
            // Its last envelopes are still in flight between the relay thread
            // and this one. Collecting them before the settlement is what makes
            // the merged store the whole of what the driver said, rather than
            // whatever happened to have arrived. Bounded, because a grandchild
            // that inherited the pipe can hold it open past its parent's exit.
            let deadline = std::time::Instant::now() + DRAIN_GRACE;
            while std::time::Instant::now() < deadline {
                match rx.recv_timeout(ATTACH_POLL) {
                    Ok(envelope) => journal.relay(&envelope)?,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        }

        let view = RunView::open(paths)?;
        // The stream is progress for a person; the settlement below is the one
        // record a caller parses. Keeping them on separate descriptors is what
        // lets a script read `stdout` as JSON while a terminal still follows
        // the run.
        let lines: Vec<String> = views::monitor(&view).lines().map(str::to_string).collect();
        for line in lines.iter().skip(reported) {
            eprintln!("{line}");
        }
        reported = lines.len();

        if let Some(settlement) = settlement_of(&view, driver_gone) {
            println!(
                "{}",
                json!({"run_id": paths.run, "settlement": settlement.as_str()})
            );
            return Ok(settlement.exit_code());
        }
        std::thread::sleep(ATTACH_POLL);
    }
}

/// Whether the run has settled, and how.
fn settlement_of(view: &RunView, driver_gone: bool) -> Option<Settlement> {
    let statuses = view.state.statuses();
    if !view.state.round_open
        && !statuses.is_empty()
        && graph::state_of(&statuses) == GraphState::Complete
    {
        return Some(Settlement::Complete);
    }
    // A *non-blocking* surface is deliberately not `awaiting-planner`: the
    // driver continues past a heartbeat update without waiting for a reply, so
    // returning there would walk away from a working run.
    if let Some(pending) = ChannelState::new(&view.paths).pending() {
        if pending.blocking {
            return Some(Settlement::AwaitingPlanner);
        }
    }
    if driver_gone || view.liveness().is_undriven() {
        return Some(Settlement::Unattended);
    }
    None
}

/// `onepipeline adopt`.
///
/// Adoption keeps everything the run owns and replaces only the driver: the run
/// id, the journal, and the ledger are the ones it already had.
fn adopt(args: &RunArgs) -> Result<i32> {
    let paths = resolve(&args.run)?;
    let session = sys::launching_session();
    let mut record: LaunchRecord = ledger::read_json(&paths.launch())?;

    // Ownership is the same rule `stop` keeps, including `unknown` never being
    // yours.
    if !record.owned_by(&session) {
        return Err(Error::NotOwned {
            run: paths.run.clone(),
            owner: record.owner_label(&session),
        });
    }
    let view = RunView::open(&paths)?;
    if !view.liveness().is_undriven() {
        return Err(Error::Refused(format!(
            "run '{}' is still being driven; end it with `onepipeline stop {}` first",
            paths.run, paths.run
        )));
    }

    record.adoptions += 1;
    record.pid = sys::pid();
    record.host = sys::hostname();
    // The dead driver's evidence moves aside rather than being truncated: it is
    // the first thing to read after adopting.
    let previous = paths
        .dir
        .join(format!("launch.pre-adopt-{}.json", record.adoptions));
    let _ = std::fs::copy(paths.launch(), previous);
    ledger::write_json(&paths.launch(), &record)?;

    let mut journal = Journal::open(&paths);
    journal.emit(
        journal::PipelineKind::DriverAdopted,
        journal::labels(&paths.run, Some(view.state.round), None),
        journal::payload(&[
            ("adoption", json!(record.adoptions)),
            ("pid", json!(record.pid)),
        ]),
    )?;

    // Relayed: an adoption attaches, so this process stays to read it. The goal
    // comes off the run's own projected plan rather than off the plan file the
    // launch named: a run whose graph the planner has edited since is still the
    // run this driver is adopting, and that file may no longer exist at all.
    let mut launched = launch_graph(
        &paths,
        &record,
        view.state
            .plan
            .as_ref()
            .and_then(|plan| plan.goal.as_ref())
            .map(|goal| goal.text.as_str()),
        agentgraph::GraphOutput::Relayed,
    )?;
    // A fresh driver is a fresh graph run with an id of its own, and the
    // pacemaker is addressed by that id — so the record names the run that is
    // driving now rather than the one that died.
    record.graph_run = launched
        .run_id()
        .map(ToString::to_string)
        .unwrap_or_default();
    ledger::write_json(&paths.launch(), &record)?;
    attach(&paths, Some(&mut launched))
}

/// `onepipeline stop`.
fn stop(args: &StopArgs) -> Result<i32> {
    let paths = resolve(&args.run)?;
    let session = sys::launching_session();
    let record: LaunchRecord = ledger::read_json(&paths.launch())?;
    let owner = record.owner_label(&session);

    if !record.owned_by(&session) {
        if !args.force {
            return Err(Error::NotOwned {
                run: paths.run.clone(),
                owner,
            });
        }
        // `--force` prints who owns it before it proceeds.
        eprintln!(
            "onepipeline: run '{}' belongs to {owner}; stopping it anyway",
            paths.run
        );
    }

    let view = RunView::open(&paths)?;
    // Attempted before the record is written, so the record says what happened
    // rather than what was about to be tried.
    let teardown = terminate(record.pid, &record.host);
    let established = match teardown {
        None => journal::StopTeardown::Elsewhere,
        Some(sys::Teardown::Signalled) => journal::StopTeardown::Signalled,
        Some(sys::Teardown::NotAttempted) => journal::StopTeardown::NotAttempted,
        Some(sys::Teardown::PartlySignalled) => journal::StopTeardown::PartlySignalled,
    };
    let mut journal = Journal::open(&paths);
    journal.emit(
        journal::PipelineKind::RunStopped,
        journal::labels(&paths.run, Some(view.state.round), None),
        journal::payload(&[
            ("owner", json!(owner)),
            ("forced", json!(args.force)),
            (journal::STOP_TEARDOWN, json!(established)),
        ]),
    )?;
    // Deliberately neither `stopped: true` nor exit 0 for either of these: a run
    // whose processes were not all reached is still running, and reporting that
    // as a clean stop is the false completion this refusal removes. The two say
    // different things because they leave the operator in different places.
    let run = &paths.run;
    match teardown {
        Some(sys::Teardown::NotAttempted) => {
            return Err(Error::Refused(format!(
                "run '{run}' was not stopped: this host gave no process listing its tree \
                 could be read from, so the processes the run started could not be found, \
                 and ending its driver alone would have orphaned them. The run is \
                 untouched — run `onepipeline stop {run}` again once `ps` answers"
            )));
        }
        Some(sys::Teardown::PartlySignalled) => {
            return Err(Error::Refused(format!(
                "run '{run}' was only partly stopped: part of its process tree was \
                 signalled and at least one process in it could not be, so that one is \
                 still running and is not this session's to end. Find it in this host's \
                 process list and end it as the user that owns it"
            )));
        }
        None | Some(sys::Teardown::Signalled) => {}
    }
    // `teardown` qualifies `stopped`: the ledger record is what stops a run, and
    // it is written either way.
    println!(
        "{}",
        json!({
            "run_id": paths.run,
            "stopped": true,
            "owner": owner,
            journal::STOP_TEARDOWN: established,
        })
    );
    Ok(EXIT_SUCCESS)
}

/// Ask the recorded driver to stop, on the host its pid means something on.
///
/// Politely: the driver takes the ask first so it records its own abandonment
/// rather than vanishing. The host check is this caller's alone — a pid means
/// nothing across machines, and the ledger's record names which one it was
/// taken on.
/// `None` when the pid is another host's, where nothing was attempted and this
/// host has nothing to promise either way.
fn terminate(pid: u32, host: &str) -> Option<sys::Teardown> {
    if host != sys::hostname() {
        return None;
    }
    Some(sys::stop(pid, sys::Stop::Politely))
}

/// `onepipeline next` — the channel's only consumer.
///
/// Rendering is not reading: `monitor` shows a pending surface without
/// consuming it, and this is what advances the queue and resets the pacemaker.
fn next(args: &RunArgs) -> Result<i32> {
    let paths = resolve(&args.run)?;
    let view = RunView::open(&paths)?;
    let channel = ChannelState::new(&paths);

    let Some(surface) = channel.claim(view.state.round)? else {
        let settled = !view.state.round_open && view.liveness().is_undriven();
        println!(
            "{}",
            if settled {
                json!({"status": "finished", "surface": null})
            } else {
                json!({"status": "running", "surface": null})
            }
        );
        return Ok(EXIT_SUCCESS);
    };

    let mut journal = Journal::open(&paths);
    journal.emit(
        journal::PipelineKind::PlannerSurfaced,
        journal::labels(
            &paths.run,
            Some(view.state.round),
            surface.workstream.as_deref(),
        ),
        journal::payload(&[
            ("kind", json!(surface.kind)),
            ("message", json!(surface.message)),
            ("source", json!(surface.source)),
            ("blocking", json!(surface.blocking)),
        ]),
    )?;

    // Consumption is what restarts the check-in clock — the whole pacemaker
    // reset contract. Addressed by the **graph** run's id, which is what the
    // sibling minted and the only id its signals answer to; this run's id names
    // a run `oneagentgraph` has never heard of. A failure to reach the sibling
    // is reported and does not fail the read: the planner has the surface either
    // way.
    if let Err(error) = agentgraph::recorded_graph_run(&view.launch.graph_run, &paths.run)
        .and_then(|graph_run| agentgraph::reset_timer(&graph_run, agentgraph::CHECK_IN_MEMBER))
    {
        eprintln!("onepipeline: could not reset the check-in pacemaker: {error}");
    }

    println!("{}", json!({"status": "surface", "surface": surface}));
    Ok(EXIT_SUCCESS)
}

/// `onepipeline surface`.
fn surface(args: &SurfaceArgs) -> Result<i32> {
    let paths = resolve(&args.run)?;
    let view = RunView::open(&paths)?;
    let kind = match args.kind {
        SurfaceKind::CheckIn => crate::channel::source::CHECK_IN,
    };
    let queued = ChannelState::new(&paths).push(Surface {
        id: 0,
        kind: kind.to_string(),
        message: args.message.clone(),
        source: kind.to_string(),
        // A pacemaker update is a report, not a request: it never blocks the
        // graph frontier waiting for a decision.
        blocking: false,
        round: view.state.round,
        queued_at: sys::now_millis(),
        workstream: None,
    })?;
    let mut journal = Journal::open(&paths);
    journal.emit(
        journal::PipelineKind::PlannerSurfaceQueued,
        journal::labels(&paths.run, Some(view.state.round), None),
        journal::payload(&[
            ("kind", json!(queued.kind)),
            ("message", json!(queued.message)),
            ("source", json!(queued.source)),
            ("blocking", json!(false)),
        ]),
    )?;
    println!("{}", json!({"surface": queued.id, "state": "queued"}));
    Ok(EXIT_SUCCESS)
}

/// `onepipeline attest` — the shorthand for a reply carrying one `attest`.
fn attest(args: &AttestArgs) -> Result<i32> {
    submit(
        &resolve(&args.run)?,
        &Reply {
            version: Some(crate::channel::REPLY_ENVELOPE_VERSION),
            commands: vec![Command::Attest {
                reference: args.reference.clone(),
            }],
            ..Reply::default()
        },
    )
}

/// `onepipeline reply`.
fn reply(args: &ReplyArgs) -> Result<i32> {
    let paths = resolve(&args.run)?;
    let text = match &args.file {
        Some(path) => std::fs::read_to_string(path).map_err(|e| Error::Ledger {
            path: path.clone(),
            source: e,
        })?,
        None => {
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string_compat(&mut buffer)
                .map_err(|e| Error::Refused(format!("cannot read the reply from stdin: {e}")))?;
            buffer
        }
    };
    // A reply this schema refuses is read a second time, leniently, to see
    // whether a retired plan field is why — an `add` carrying one is the same
    // planner mistake as a plan file carrying one, and deserves the same answer.
    let envelope: Reply = serde_json::from_str(text.trim()).map_err(|e| {
        let why = serde_json::from_str::<serde_json::Value>(text.trim())
            .ok()
            .as_ref()
            .and_then(crate::plan::retired_field)
            .unwrap_or_else(|| e.to_string());
        Error::Refused(format!("the reply is malformed: {why}"))
    })?;
    submit(&paths, &envelope)
}

/// Validate a reply, queue it, and report which of the two true things happened.
fn submit(paths: &RunPaths, envelope: &Reply) -> Result<i32> {
    let view = RunView::open(paths)?;
    let channel = ChannelState::new(paths);

    if envelope.commands.is_empty() {
        // A settled run has no reader left, now or later, so queuing a reply to
        // it would park it where nothing drains it. A surface still awaiting an
        // answer outranks that: the run asked for the reply.
        if channel.pending().is_none() && view.liveness().is_undriven() {
            return Err(Error::Refused(format!(
                "run '{}' has settled, so nothing will ever read a reply to it; \
                 no reply was queued",
                paths.run
            )));
        }
        let id = channel.answer(envelope)?;
        let mut journal = Journal::open(paths);
        journal.emit(
            journal::PipelineKind::PlannerReplied,
            journal::labels(&paths.run, Some(view.state.round), None),
            journal::payload(&[
                ("completion", json!(envelope.completion)),
                ("reason", json!(envelope.reason)),
            ]),
        )?;
        if let Some(reason) = &envelope.reason {
            if envelope.completion == Some(true) {
                journal.emit(
                    journal::PipelineKind::CompletionRequested,
                    journal::labels(&paths.run, Some(view.state.round), None),
                    journal::payload(&[("reason", json!(reason))]),
                )?;
            }
        }
        println!("{}", json!({"reply": id, "state": "delivered"}));
        return Ok(EXIT_SUCCESS);
    }

    if envelope.version != Some(crate::channel::REPLY_ENVELOPE_VERSION) {
        return Err(Error::Refused(format!(
            "an edit envelope requires version {}",
            crate::channel::REPLY_ENVELOPE_VERSION
        )));
    }

    // Edits require a live round: replying with one when no round is executing
    // is refused with that reason. Two commands are not edits and stay legal at
    // a round boundary — a bare `complete` verdict, and an `attest`, which
    // records that a person did something rather than mutating the graph.
    // Refusing an attestation at a boundary would strand every human-gated run:
    // its round has settled, and no later round can open until the action is
    // recorded.
    let structural = envelope
        .commands
        .iter()
        .any(|command| !matches!(command, Command::Complete { .. } | Command::Attest { .. }));
    if structural && !view.state.round_open {
        return Err(Error::Refused(format!(
            "run '{}' has no round executing, and an edit needs a live round to apply to",
            paths.run
        )));
    }

    // Every edit is validated against the graph projected from the journal,
    // through the reconciler's own validator, so the answer is the one the
    // reconciler would give — before anything is queued or sent.
    let mut projected = view.state.graph.clone();
    let frontier = view.state.frontier();
    for command in &envelope.commands {
        edits::compile(&mut projected, &frontier, command)?;
    }

    // With a round executing, the reconciler is the single writer and the
    // command goes to its durable queue. Without one there is no other writer,
    // so the boundary-legal commands are recorded here under the ownership lock.
    if !view.state.round_open {
        let lock = ledger::OwnershipLock::acquire(paths, "reply")?;
        let mut journal = Journal::open(paths);
        for command in &envelope.commands {
            match command {
                Command::Complete { reason } => journal.emit(
                    journal::PipelineKind::CompletionRequested,
                    journal::labels(&paths.run, Some(view.state.round), None),
                    journal::payload(&[("reason", json!(reason))]),
                )?,
                Command::Attest { reference } => journal.emit(
                    journal::PipelineKind::HumanAttested,
                    journal::labels(&paths.run, Some(view.state.round), Some(reference)),
                    journal::payload(&[("ref", json!(reference))]),
                )?,
                _ => {}
            }
        }
        lock.release();
        channel.answer(envelope)?;
        println!("{}", json!({"reply": 0, "state": "applied"}));
        return Ok(EXIT_SUCCESS);
    }

    let id = channel.submit(&envelope.commands)?;

    let deadline = Instant::now() + Duration::from_secs(reply_timeout_seconds());
    while Instant::now() < deadline {
        if let Some(outcome) = channel.outcome_of(id) {
            channel.answer(envelope)?;
            if outcome.applied {
                println!("{}", json!({"reply": id, "state": "applied"}));
                return Ok(EXIT_SUCCESS);
            }
            return Err(Error::Refused(
                outcome
                    .reason
                    .unwrap_or_else(|| "the reconciler rejected the edit".into()),
            ));
        }
        std::thread::sleep(ATTACH_POLL);
    }

    // Accepted and durable, but not reconciled within the timeout: they remain
    // queued, and this is not an instruction to resend.
    println!("{}", json!({"reply": id, "state": "queued"}));
    Ok(EXIT_QUEUED)
}

fn reply_timeout_seconds() -> u64 {
    std::env::var(crate::channel::REPLY_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(crate::channel::DEFAULT_REPLY_TIMEOUT_SECONDS)
}

/// `onepipeline channel serve` — the orchestrator member's judge side.
///
/// The orchestrator emits one JSON frame per round boundary on stdout; this
/// relays it to the planner as a blocking surface, waits for the answer, and
/// writes the verdict back. Only the planner can issue edits: a worker's
/// proposal is advice, and this side never authors one.
fn serve(args: &RunArgs) -> Result<i32> {
    let paths = resolve(&args.run)?;
    let channel = ChannelState::new(&paths);
    let stdin = std::io::stdin();

    for line in stdin.lock().lines() {
        // End of input and a broken pipe are not the same fact. Read as one,
        // the orchestrator's judge side exits 0 on a stream that failed
        // mid-round, so the boundary question it was carrying never reaches the
        // planner and nothing anywhere says why.
        let line = line.map_err(|e| Error::Sibling {
            tool: "oneagentgraph",
            message: format!("the orchestrator's frame stream could not be read: {e}"),
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let frame: BoundaryFrame = serde_json::from_str(line.trim())
            .map_err(|e| Error::Refused(format!("the orchestrator emitted a bad frame: {e}")))?;
        let view = RunView::open(&paths)?;
        let queued = channel.push(Surface {
            id: 0,
            kind: frame.kind,
            message: frame.message,
            source: crate::channel::source::PROPOSAL.to_string(),
            blocking: frame.blocking,
            round: view.state.round,
            queued_at: sys::now_millis(),
            workstream: frame.node,
        })?;
        let mut journal = Journal::open(&paths);
        journal.emit(
            journal::PipelineKind::PlannerSurfaceQueued,
            journal::labels(&paths.run, Some(view.state.round), None),
            journal::payload(&[
                ("kind", json!(queued.kind)),
                ("message", json!(queued.message)),
                ("source", json!(queued.source)),
                ("blocking", json!(queued.blocking)),
            ]),
        )?;

        // Wait for whichever reader claims the planner's answer first. A reply
        // reaches exactly one reader, and at a boundary this is it.
        let answer = wait_for_reply(&channel)?;
        println!(
            "{}",
            serde_json::to_string(&answer).map_err(|e| Error::Invalid(format!("verdict: {e}")))?
        );
        std::io::stdout()
            .flush()
            .map_err(|e| Error::Refused(format!("cannot write the verdict: {e}")))?;
        if answer.completion == Some(true) {
            break;
        }
    }
    Ok(EXIT_SUCCESS)
}

/// What the orchestrator emits at a round boundary.
///
/// External input, so it has a schema: an unknown key or a missing `kind` or
/// `message` is refused by name rather than defaulted into a surface the
/// planner then has to interpret.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundaryFrame {
    /// What the surface is asking about, in the orchestrator persona's own
    /// boundary vocabulary.
    kind: String,
    /// Its text.
    message: String,
    /// Whether the orchestrator is waiting on the answer. A boundary frame is
    /// blocking unless it says otherwise; a worker's advice is not.
    #[serde(default = "blocking_by_default")]
    blocking: bool,
    /// The node that provoked it, when one did.
    #[serde(default)]
    node: Option<String>,
}

fn blocking_by_default() -> bool {
    true
}

fn wait_for_reply(channel: &ChannelState) -> Result<Reply> {
    let deadline = Instant::now() + Duration::from_secs(reply_timeout_seconds());
    while Instant::now() < deadline {
        if let Some(claimed) = channel.claim_replies()?.into_iter().next_back() {
            return Ok(claimed.reply);
        }
        std::thread::sleep(ATTACH_POLL);
    }
    // Nothing answered in time. A synthesized continuing verdict keeps the run
    // moving rather than wedging the orchestrator on a planner who is away.
    Ok(Reply {
        completion: Some(false),
        message: Some("no planner reply within the timeout; continue".into()),
        reason: Some("the channel timed out waiting for a verdict".into()),
        ..Reply::default()
    })
}

/// `onepipeline runs`.
fn runs(args: &RunsArgs) -> Result<i32> {
    print!(
        "{}",
        views::runs(&ledger::runs_root(), args.mine, &sys::launching_session())
    );
    Ok(EXIT_SUCCESS)
}

/// A view that covers one run, or every run when given none.
fn report(args: &OptionalRunArgs, render: fn(&[RunView]) -> String) -> Result<i32> {
    let views = match &args.run {
        Some(run) => vec![RunView::open(&resolve(run)?)?],
        None => RunView::all(&ledger::runs_root()),
    };
    print!("{}", render(&views));
    Ok(EXIT_SUCCESS)
}

/// `onepipeline transcript`.
///
/// A node this run never dispatched is refused rather than answered with an
/// empty transcript: the two read alike, and only one of them means the reader
/// typed a name that is not in this run.
fn transcript(args: &TranscriptArgs) -> Result<i32> {
    let view = RunView::open(&resolve(&args.run)?)?;
    if let Some(node) = &args.node {
        if views::nodes_with_agent_records(&view, Some(node)).is_empty() {
            let recorded = views::nodes_with_agent_records(&view, None);
            return Err(Error::Refused(format!(
                "run '{}' has recorded nothing for node '{node}'; it has records for: {}",
                args.run,
                if recorded.is_empty() {
                    "nothing yet".to_string()
                } else {
                    recorded.join(", ")
                }
            )));
        }
    }
    print!("{}", views::transcript(&view, args.node.as_deref()));
    Ok(EXIT_SUCCESS)
}

/// `onepipeline telemetry`.
fn report_telemetry(args: &TelemetryArgs) -> Result<i32> {
    let views = match &args.run {
        Some(run) => vec![RunView::open(&resolve(run)?)?],
        None => RunView::all(&ledger::runs_root()),
    };
    for view in &views {
        let aggregated = telemetry::of_run(&view.paths, &view.events);
        if args.breakdown {
            print!("{}", telemetry::render_breakdown(&aggregated));
        } else {
            println!(
                "{}",
                serde_json::to_string(&aggregated)
                    .map_err(|e| Error::Invalid(format!("telemetry: {e}")))?
            );
        }
    }
    Ok(EXIT_SUCCESS)
}

/// `Stdin::read_to_string` under a name that does not collide with the trait
/// method callers would otherwise have to import.
trait ReadToStringCompat {
    fn read_to_string_compat(&self, buffer: &mut String) -> std::io::Result<usize>;
}

impl ReadToStringCompat for std::io::Stdin {
    fn read_to_string_compat(&self, buffer: &mut String) -> std::io::Result<usize> {
        use std::io::Read;
        self.lock().read_to_string(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Node, PLAN_SCHEMA_VERSION};
    use crate::views::DriverLiveness;
    use std::path::PathBuf;

    fn plan(name: Option<&str>) -> Plan {
        Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: None,
            name: name.map(str::to_string),
            concurrency: 4,
            tasks: vec![Node {
                id: "build".into(),
                persona: Some("engineer".into()),
                task: Some("## What\ndo it".into()),
                ..Node::default()
            }],
        }
    }

    /// The role prose [`run_description`] must never carry again.
    ///
    /// Listed rather than left to a reviewer, because a line of it reaching that
    /// task is an instruction to every member whose job it is not. This is where
    /// prose put back fails, rather than in a consumer.
    const ROLE_PROSE: &[&str] = &[
        "Drive",
        "drive",
        "to settlement",
        "onepipeline round run",
        "onepipeline round next",
        "round next",
        "nothing else",
        "run state",
        "you",
        "You",
    ];

    fn assert_role_neutral(task: &str) {
        for prose in ROLE_PROSE {
            assert!(
                !task.contains(prose),
                "the launched graph's task tells a member what to do ({prose:?}): {task}"
            );
        }
    }

    #[test]
    fn the_launched_graphs_task_names_the_run_and_its_goal_and_no_role() {
        let task = run_description("tracked-release", Some("close the coverage gap"));
        assert!(
            task.contains("tracked-release"),
            "the task does not name the run: {task}"
        );
        assert!(
            task.contains("close the coverage gap"),
            "the task does not say what the run is for: {task}"
        );
        assert_role_neutral(&task);
    }

    /// A goal is optional in the schema, and the task's shape is not: a member
    /// composing `{task}` plus its own prose reads the same document either way.
    #[test]
    fn a_run_whose_plan_states_no_goal_says_so_in_the_same_shape() {
        let task = run_description("nameless", None);
        assert!(
            task.contains("nameless") && task.contains(crate::plan::NO_GOAL),
            "the task for a goalless run reads: {task}"
        );
        assert_role_neutral(&task);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("onepipeline-driver-{name}-{}", sys::pid()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch root");
        dir
    }

    #[test]
    fn a_run_id_comes_from_the_plans_name_and_is_made_unique() {
        let root = scratch("mint");
        let path = Path::new("plans/release.plan.json");
        assert_eq!(
            mint_run_id(&plan(Some("tracked-release")), path, &root),
            "tracked-release"
        );

        std::fs::create_dir_all(root.join("tracked-release")).expect("an existing run");
        assert_eq!(
            mint_run_id(&plan(Some("tracked-release")), path, &root),
            "tracked-release-2"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_nameless_plan_takes_its_run_id_from_the_file() {
        let root = scratch("mint-file");
        assert_eq!(
            mint_run_id(&plan(None), Path::new("plans/release.plan.json"), &root),
            "release"
        );
        assert_eq!(
            mint_run_id(&plan(None), Path::new("plans/odd name!.json"), &root),
            "odd-name-"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn every_settlement_carries_the_exit_code_the_contract_assigns() {
        assert_eq!(Settlement::Complete.exit_code(), EXIT_SUCCESS);
        assert_eq!(Settlement::AwaitingPlanner.exit_code(), EXIT_SUCCESS);
        assert_eq!(Settlement::Unattended.exit_code(), EXIT_NOTHING_DRIVING);
        assert_eq!(Settlement::Complete.as_str(), "complete");
        assert_eq!(Settlement::AwaitingPlanner.as_str(), "awaiting-planner");
        assert_eq!(Settlement::Unattended.as_str(), "unattended");
    }

    #[test]
    fn the_dag_graph_comes_from_the_environment_or_falls_back_to_the_shipped_one() {
        assert!(!dag_graph().is_empty());
        assert!(dag_graph().contains("dag-scope") || std::env::var(DAG_GRAPH_ENV).is_ok());
    }

    #[test]
    fn an_undriven_run_is_the_settlement_a_planner_must_intervene_in() {
        // Assembled from the same parts the view reads, so the verdict under
        // test is the one `attach` returns rather than a restatement of it.
        assert!(DriverLiveness::DriverDead.is_undriven());
        assert!(DriverLiveness::Parked.is_undriven());
        assert!(!DriverLiveness::Driving.is_undriven());
    }

    #[test]
    fn the_reply_timeout_falls_back_when_the_environment_is_unusable() {
        assert!(reply_timeout_seconds() > 0);
    }
}
