//! The driver contract: launching a run, owning it, attaching to it, and
//! handing it to a fresh driver when its own dies.
//!
//! `onepipeline start` drives the run itself: it runs [`engine::drive`]'s
//! continuous loop under the run's ownership lock, in this process. No agent is
//! required to execute a plan. `--dag-graph REF` attaches an agent graph as an
//! **observer** — it watches the stream and authors channel surfaces, and it
//! never drives the engine. This crate never decides what the graph should be;
//! it schedules, dispatches, and closes out.
//!
//! Runs belong to the session that launched them. `stop` refuses another
//! session's run and `--force` names the owner; `adopt` has no `--force` at all,
//! because taking over ongoing work is exactly the case where a second opinion
//! is worth more than an override.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::agentgraph;
use crate::channel::{Author, ChannelState, Command, Reply, Surface, SurfaceKind};
use crate::cli::{
    AdoptArgs, AttestArgs, ChannelCommand, Cli, DriveRunArgs, OptionalRunArgs, ReadArgs, ReplyArgs,
    RunArgs, RunsArgs, StartArgs, StopArgs, SurfaceArgs, TelemetryArgs, TranscriptArgs, ADOPT_FLAG,
    DAG_GRAPH_OFF,
};
use crate::concurrency::{self, Liveness, State};
use crate::edits::{self, Frontier};
use crate::engine;
use crate::error::{Error, Result, EXIT_NOTHING_DRIVING, EXIT_QUEUED, EXIT_SUCCESS};
use crate::filter::{self, EventFilter};
use crate::graph::{self, GraphState};
use crate::journal::{self, Journal};
use crate::ledger::{self, LaunchRecord, RunPaths};
use crate::plan::Plan;
use crate::sys;
use crate::telemetry;
use crate::views::{self, RunView};

/// How often an attach re-reads the run to see whether it has settled.
const ATTACH_POLL: Duration = Duration::from_millis(50);

/// How long an attach collects a departed observer's last envelopes before
/// settling without them.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// How long `start --detach` waits for its retained driver to claim the run.
///
/// Process startup plus one read of the ledger, with room for a loaded host: a
/// launch that waited less would report a driver that had not started, and one
/// that waited longer would make a genuinely failed launch look like a slow one.
const DRIVER_HANDOVER: Duration = Duration::from_secs(30);

/// How long a `stop` watches the tree it signalled before saying it is still
/// there.
///
/// The polite signal is `SIGTERM` and nothing a run is made of installs a
/// handler for it, so a process that has taken one is gone in milliseconds; what
/// this waits out is a loaded host and the moment between a parent dying and
/// `init` reaping what it left. Long enough that an ordinary teardown never
/// reports a survivor it merely outran, short enough that an operator whose run
/// really is wedged hears about it rather than watching a command hang.
const TEARDOWN_PATIENCE: Duration = Duration::from_secs(5);

/// How many of a failed driver's last lines a refusal repeats.
///
/// Enough for the sibling's own refusal and the sentence around it, and few
/// enough that a driver that failed after working for an hour does not put its
/// whole log on one line.
const DRIVER_LOG_LINES: usize = 8;

/// Execute one parsed command line.
pub fn dispatch(cli: Cli) -> Result<i32> {
    use crate::cli::Command as Verb;
    match cli.command {
        Verb::Start(args) => start(&args),
        Verb::Plan(crate::cli::PlanCommand::Check(args)) => crate::plancheck::check(&args),
        Verb::Adopt(args) => adopt(&args),
        Verb::DriveRun(args) => drive_run(&args),
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
            let view = RunView::open(&resolve(&args.run)?)?;
            let filter = read_filter(&view, &args)?;
            print!("{}", views::monitor(&view, &filter));
            Ok(EXIT_SUCCESS)
        }
        Verb::Watch(args) => {
            // Both refusals are made before anything blocks: a run that is not
            // there, and a profile this run does not have. A watch that waited
            // out its whole timeout to report a mistyped profile name would be
            // worse than the shell loop it replaces.
            let paths = resolve(&args.read.run)?;
            let view = RunView::open(&paths)?;
            let filter = read_filter(&view, &args.read)?;
            crate::watch::watch(&args, &paths, &filter)
        }
        Verb::Results(args) => {
            print!("{}", views::results(&RunView::open(&resolve(&args.run)?)?));
            Ok(EXIT_SUCCESS)
        }
        Verb::Goals(args) => report(&args, views::goals),
        Verb::Transcript(args) => transcript(&args),
        Verb::Telemetry(args) => report_telemetry(&args),
        Verb::Drive(args) => agentgraph::drive(
            &args.graph,
            &args.task,
            &args.dir,
            &args.labels,
            &args.sets,
            args.event_filter.as_deref(),
        ),
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
// absolute here, and the nonempty launch-record invariant is checked before every run.
fn resolve_graph(reference: &str, base: &Path) -> Result<String> {
    // Refused before anything is joined or opened, because a blank reference
    // resolves to `base` itself — the launch directory — and what happens next
    // is whatever the host's file API answers for opening a directory, which is
    // read on Linux and refused on Windows. A launch that starts on one platform
    // and not another is not a launch anybody wrote. What names no graph at all
    // is the `None` its caller holds; a reference that is *there* and holds
    // nothing names one that cannot be found, and says so the same way
    // everywhere.
    if reference.trim().is_empty() {
        return Err(Error::Invalid(
            "graph reference is blank: name a path, an `https://` URL, or no graph at all"
                .to_string(),
        ));
    }
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

/// Mint a run id from the plan's name or the project's, made unique.
///
/// The plan's own name first, which a project states as `onepipeline.name` or
/// leaves to its own title; the native half of the qualified id otherwise, which
/// is what a person launching it typed.
fn mint_run_id(plan: &Plan, native: &str, root: &Path) -> String {
    let base = plan
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| native.to_string());
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

/// The `filters:` block one launch declared, read and checked at its boundary.
///
/// The launch config is the base and each flag overrides the part of it that it
/// names — the two source filters wholesale, and a profile *by name*, so a
/// config holding a team's five profiles beside a plan can have one of them
/// replaced for one launch without restating the other four. A launch naming no
/// config is the same code path with an empty base, which is what makes the two
/// surfaces the same block rather than two blocks that have to agree.
///
/// Every spec is read where the operator wrote it and refused there, with the
/// offending matcher named — the two source filters because a launch cannot
/// honour a spec its sources will not take, and the profiles because a profile
/// only refused at read time would be refused to a planner who did not write it,
/// long after the launch that did.
fn declared_filters(
    config: crate::filter::Filters,
    args: &StartArgs,
) -> Result<crate::filter::Filters> {
    let mut filters = config;
    for declaration in &args.filter_profiles {
        let (name, spec) = declaration.split_once('=').ok_or_else(|| {
            Error::Invalid(format!(
                "--filter-profile takes NAME=SPEC; '{declaration}' names no profile"
            ))
        })?;
        if name.trim().is_empty() {
            return Err(Error::Invalid(format!(
                "--filter-profile takes NAME=SPEC; '{declaration}' has an empty name"
            )));
        }
        filters
            .profiles
            .insert(name.to_string(), EventFilter::read(spec)?);
    }
    if let Some(spec) = args.filter_agentgraph.as_deref() {
        filters.agentgraph = Some(EventFilter::read(spec)?);
    }
    if let Some(spec) = args.filter_vcs.as_deref() {
        filters.vcs = Some(EventFilter::read(spec)?);
    }
    Ok(filters)
}

/// The filter a read verb shapes its event view with.
///
/// `--all` is no filter at all, `--filter` is a profile this run has or a spec
/// spelled inline, and naming neither is the shipped default profile — which is
/// what makes `next` and `monitor` the planner's view unless a reader says
/// otherwise.
///
/// A name is tried as a profile *first*: a profile name and a filter spec cannot
/// be confused for one another — a spec is a mapping and starts with `{`, and a
/// profile name is a bare word — so the only ambiguity would be a file on disk
/// named `planner`, and a run's own vocabulary is what a reader of that run
/// means.
fn read_filter(view: &RunView, args: &ReadArgs) -> Result<EventFilter> {
    if args.all {
        return Ok(EventFilter::default());
    }
    let named = args.filter.as_deref().unwrap_or(filter::DEFAULT_PROFILE);
    if named.trim_start().starts_with('{') {
        return EventFilter::read(named);
    }
    match view.launch.filters.profile(named) {
        Ok(filter) => Ok(filter),
        // A spec named as a path is still a spec: the profile lookup is what
        // failed, and this is the second reading rather than a fallback that
        // hides it — a name that is neither a profile nor a readable file is
        // answered with the profile refusal, which names the ones this run has.
        Err(unknown) => match Path::new(named).is_file() {
            true => EventFilter::read(named),
            false => Err(unknown),
        },
    }
}

/// `onepipeline start`.
fn start(args: &StartArgs) -> Result<i32> {
    // The binary first, and its version with it: a plan that cannot be read is
    // a launch that never starts, and a store this build cannot read a project
    // out of is named here rather than by a node that fails on its first
    // dispatch. Nothing is dispatched and no run root is created before this.
    let store = crate::taskgraph::Store::resolve()?;
    // Parsed once, here: a bare id names nothing a store can answer for, and
    // this is where a person typed it.
    let project: crate::taskgraph::QualifiedId = args.project.parse()?;
    let mut plan = store.plan(&project)?;
    graph::validate(&plan)?;
    let launch_dir = launch_dir()?;
    // The launch config is read once, here, and both halves of what it declares
    // are taken off that one strict reading: a second, later read would be a
    // second chance for a document this build refuses to reach a run.
    let declared = match &args.launch_config {
        Some(path) => crate::filter::LaunchConfig::load(path)?,
        None => crate::filter::LaunchConfig::default(),
    };
    // Resolved only when one was named: `off` is the shipped default, and a
    // launch that names no observer resolves nothing and launches nothing.
    let graph_ref: Option<String> = match args.dag_graph.as_str() {
        DAG_GRAPH_OFF => None,
        reference => Some(resolve_graph(reference, &launch_dir)?),
    };
    // The same, for the graph a change request's body is drafted by: naming none
    // is the shipped default, and the flag overrides the config that names one.
    // Resolved against the launch directory like every other reference, so the
    // record carries what a driver started from anywhere else replays.
    //
    // A rung that is *there* and blank names no graph, exactly as the node
    // validator's three rungs below read a blank one: the flag still overrides
    // the config it names, and what it overrides it to is "this launch drafts
    // nothing" rather than the config underneath. `LaunchConfig::load` keeps a
    // blank `pr_author_graph` rather than refusing it — the key has shipped
    // since schema 2 and a document already on disk may carry one — so the
    // blank arrives here, and here is where it means the same thing as the
    // document that omits the key: nothing to resolve, and nothing read off a
    // disk to decide it, because what a filesystem answers for an empty path is
    // a property of the platform rather than of the launch.
    // llmlint: ignore-block[invalid_states_unrepresentable] a resolved reference stays the
    // `String` `resolve_graph` answers with, from here into `LaunchRecord` and back out of
    // it, for the reason that function's own suppression gives: the durable record and
    // oneagentgraph's transparent `ConfigRef` are both string-valued, and a newtype here
    // would duplicate the sibling's type without adding an invariant. It sits beside
    // `graph_ref` above and is carried exactly as that one is.
    let pr_author_graph_ref: Option<String> = match args
        .pr_author_graph
        .as_deref()
        .or(declared.pr_author_graph.as_deref())
        .filter(|reference| !reference.trim().is_empty())
    {
        Some(reference) => Some(resolve_graph(reference, &launch_dir)?),
        None => None,
    }; // llmlint: ignore-end[invalid_states_unrepresentable]

    // llmlint: ignore-block[invalid_states_unrepresentable] the resolved command stays the
    // `String` `LaunchRecord`'s schema declares, exactly as `pr_author_graph_ref` above
    // does. The one invariant a newtype could carry — non-blank — is the `filter` below;
    // that the command *runs* is establishable only by running it, which
    // `edits::offer_to_validator` does, failing closed.
    //
    // The **presence** of a rung decides which one answers, so a flag or a variable that
    // is there and blank means this launch names none rather than falling through to the
    // rung under it. A blank config key never reaches here: `LaunchConfig::load` refuses
    // one by name. Not resolved against the launch directory — a command the host names
    // may as legitimately be a name on `PATH` as a path.
    let named = match args.node_validator.clone() {
        // The flag wins outright, so a variable this build cannot read is never
        // consulted by a launch that was not going to use it.
        flag @ Some(_) => flag,
        None => match engine::configured_node_validator()? {
            variable @ Some(_) => variable,
            None => declared.node_validator.clone(),
        },
    };
    let node_validator: Option<String> = named
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty());

    // The envelope reviewer, resolved by the same three rungs and for the same
    // reasons, so a launch names both hooks the one way. It is a second hook
    // rather than a mode of the first: the validator judges one node on its own,
    // and this one judges a whole envelope against the plan and the goal.
    let asked = match args.envelope_reviewer.clone() {
        flag @ Some(_) => flag,
        None => match engine::configured_envelope_reviewer()? {
            variable @ Some(_) => variable,
            None => declared.envelope_reviewer.clone(),
        },
    };
    let envelope_reviewer: Option<String> = asked
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty());
    // llmlint: ignore-end[invalid_states_unrepresentable]
    let node_graph_ref = resolve_graph(&engine::configured_node_graph(), &launch_dir)?;
    resolve_plan_graphs(&mut plan, &launch_dir)?;
    // Before the run directory exists. A spec that could not be honoured is the
    // exit 2 it is, rather than a launch that has already minted a run and cut
    // sessions for it before a source refuses the filter it was handed.
    let filters = declared_filters(declared.filters, args)?;

    let root = ledger::runs_root();
    let run = mint_run_id(&plan, project.native(), &root);
    let holders = concurrency::holders(&plan)?;
    // Every stale holder the sibling still has anybody to answer for. Since
    // `onevcs` 0.17.1 a record whose owner process has gone, whose run root
    // nothing is working inside, and whose branch carries nothing unpublished is
    // left out of the enumeration above rather than handed over, so what is left
    // here is the one an operator can act on: a session somebody is still working
    // in whose launcher died. Reported and proceeded past — refusing is the live
    // holder's business, below.
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
        project: args.project.clone(),
        // Absolute, once, here: this is the only process that knows where the
        // operator launched from, and every later driver — including the one a
        // fresh `adopt` starts from some other directory — replays this value
        // rather than reading its own.
        dir: launch_dir.clone(),
        graph: graph_ref.clone().unwrap_or_default(),
        // Replaced below by the graph run's own id, which does not exist until
        // the launch below has produced it.
        graph_run: String::new(),
        observer_runs: Vec::new(),
        observer_ending: String::new(),
        node_graph: node_graph_ref,
        pr_author_graph: pr_author_graph_ref.unwrap_or_default(),
        node_validator: node_validator.unwrap_or_default(),
        envelope_reviewer: envelope_reviewer.unwrap_or_default(),
        launcher: sys::launcher(),
        session: sys::launching_session(),
        // Claimed by this process immediately below, through the one writer of
        // all three: which process, on which host, and the stamp that says the
        // pid is still that process. Replaced again by the graph process's own
        // claim — what drives the run is that process, not this one: `--detach`
        // returns immediately, so a record naming this pid would read as a dead
        // driver the moment it did. Until that process exists, this one is what
        // is driving the run, and the record has to say so — see
        // `launch_graph`'s ordering.
        pid: 0,
        host: String::new(),
        started: String::new(),
        started_at: sys::now_rfc3339(),
        heartbeat_interval: args.heartbeat_interval,
        dag_sets: args.dag_sets.clone(),
        node_sets: args.node_sets.clone(),
        adoptions: 0,
        filters,
    };
    record.driven_by_this_process();

    let mut open = Journal::open(&paths);
    if !live.is_empty() {
        open.emit(
            journal::PipelineKind::ConcurrentAcknowledged,
            journal::labels(&run, None),
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
        journal::labels(&run, None),
        journal::payload(&[
            ("plan", json!(plan)),
            ("graph", json!(graph_ref)),
            // Stated on the run's own first record, so a reader never has to
            // infer which directory a run's members worked in from the process
            // that happened to launch them.
            ("dir", json!(launch_dir)),
            ("heartbeat_interval", json!(args.heartbeat_interval)),
        ]),
    )?;

    // The record is durable *before* anything that reads it exists. The engine
    // loop opens the launch record for the node graph it dispatches under, and
    // a detached driver is a separate process that would otherwise die on a file
    // nobody had written yet — leaving a run stuck at `run-started` with nothing
    // driving it. It is written again below, once the pids and the observer's
    // graph run are known.
    ledger::write_json(&paths.launch(), &record)?;
    if args.detach {
        // Before the driver exists, and only on this path: a detaching launcher
        // is about to exit, and the driver it is about to start must not hold
        // this process's streams open behind it. See
        // [`sys::disown_standard_handles`] for what inherits what.
        sys::disown_standard_handles();
    }

    if args.detach {
        // A retained process, because the loop that drives a run cannot outlive
        // a launcher that is about to return. It is *this* build at its own
        // hidden verb, so the run is driven by the same engine an attached
        // launch would have run in-process — and it launches the observer
        // itself, so `stop` reaches that graph through the driver's own process
        // tree rather than leaving an agent running beside a stopped run.
        let mut driver = retain_driver(&paths, Retained::Driving)?;
        let pid = driver.id();
        // The driver records its own pid and its observer's graph run, so there
        // is one writer of those fields and no race between this process and the
        // one it just started. Waiting for it also means a launch that could not
        // start a driver says so, rather than printing a pid for a process that
        // is already gone.
        confirm_driving(&paths, &mut driver)?;
        announce_launch(&run, pid);
        return Ok(EXIT_SUCCESS);
    }

    let goal = plan.goal.as_ref().map(|goal| goal.text.clone());
    // The observer, if the launch named one. It watches and reports; the loop
    // below is what drives the run either way, so a graph that refuses to start
    // fails the launch rather than leaving a run nothing executes.
    // The lock before anything is launched: this process is about to be the
    // run's single writer, and a launch that started an observer and then lost
    // that race would leave a graph watching a run it does not drive.
    let lock = engine::claim(&paths)?;
    let output = agentgraph::GraphOutput::Relayed;
    let mut observer = observe(&paths, &mut record, goal.as_deref(), output)?;
    ledger::write_json(&paths.launch(), &record)?;
    let mut watch = ObserverWatch::of(&paths, record, goal, output);
    attach(&paths, observer.as_mut(), &mut watch, lock)
}

/// Launch the run's observer graph, when it was launched with one.
///
/// Records the graph run it minted, which is what a later `next` addresses the
/// pacemaker by. `output` is the caller's promise about itself: an attaching
/// launcher stays and relays what the observer says into the merged store, and
/// a retained driver hands it the run's own driver log instead.
fn observe(
    paths: &RunPaths,
    record: &mut LaunchRecord,
    goal: Option<&str>,
    output: agentgraph::GraphOutput<'_>,
) -> Result<Option<agentgraph::GraphRun>> {
    if record.observer_graph().is_none() {
        return Ok(None);
    }
    let launched = launch_graph(paths, record, goal, output)?;
    record.watched_by(
        launched
            .run_id()
            .map(ToString::to_string)
            .unwrap_or_default(),
    );
    Ok(Some(launched))
}

/// Wait for a retained driver to claim the run, or report that it never did.
///
/// A detached launch is the one caller that never waits for what it started, so
/// a driver that died on its way up — an observer graph that refused, a ledger it
/// could not read — would otherwise be reported as a running one: an exit 0 and a
/// pid for a process that is already gone. The driver's own words come back with
/// the refusal, because they are in a file the launcher is about to walk away
/// from and nothing else will ever read them out loud.
fn confirm_driving(paths: &RunPaths, driver: &mut std::process::Child) -> Result<()> {
    let pid = driver.id();
    let deadline = Instant::now() + DRIVER_HANDOVER;
    while Instant::now() < deadline {
        let recorded: Option<LaunchRecord> = ledger::read_json_opt(&paths.launch());
        if recorded.is_some_and(|record| record.pid == pid) {
            return Ok(());
        }
        // `try_wait` rather than a pid probe, and reaping is the point: a child
        // nobody waits on stays a zombie, and a zombie answers a liveness probe
        // as alive — so a driver that died on its way up would be waited out to
        // the whole backstop instead of reported at once.
        if matches!(driver.try_wait(), Ok(Some(_)) | Err(_)) {
            break;
        }
        std::thread::sleep(ATTACH_POLL);
    }
    Err(Error::Refused(format!(
        "the driver retained for run '{}' did not claim it: {}",
        paths.run,
        driver_said(paths)
    )))
}

/// What a retained driver wrote before it gave up, bounded to what a refusal can
/// carry.
fn driver_said(paths: &RunPaths) -> String {
    let log = paths.driver_log();
    let said = std::fs::read_to_string(&log).unwrap_or_default();
    let tail: String = said
        .lines()
        .rev()
        .take(DRIVER_LOG_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("; ");
    if tail.trim().is_empty() {
        return format!("it said nothing; its output is in {}", log.display());
    }
    format!("{tail} (its whole output is in {})", log.display())
}

/// What a detached launch prints once its driver has the run: the run it left
/// behind, the process now driving it, and the two verbs that reach it.
///
/// One shape for both detaching verbs — a launch and an adoption leave the same
/// thing behind, so an operator reads the same record either way, and
/// `--detach` means on `adopt` exactly what it means on `start`.
fn announce_launch(run: &str, pid: u32) {
    println!(
        "{}",
        json!({
            "run_id": run,
            "pid": pid,
            "commands": {
                "next": format!("onepipeline next {run}"),
                "monitor": format!("onepipeline monitor {run}"),
            },
        })
    );
}

/// What a retained driver is being started to do.
///
/// The run it is handed is either one nothing has driven yet or one it is taking
/// over, and the difference is a whole sequence — the adoption's counter, the
/// dead driver's record moved aside, and the journal entry — that belongs under
/// the ownership lock. Only the retained process holds that lock, so only it can
/// do it: a launcher that adopted on its behalf would be writing on behalf of a
/// driver that does not exist yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retained {
    /// `start --detach`: the run has never been driven.
    Driving,
    /// `adopt --detach`: the run is being taken over from the driver that had
    /// it.
    Adopting,
}

/// Start the retained driver a detached launch leaves behind.
///
/// This executable, at [`engine::DRIVE_VERB`], with its output in the run's own
/// driver log: the process that returns from `start --detach` must not be
/// holding the pipe a driver writes to.
fn retain_driver(paths: &RunPaths, retained: Retained) -> Result<std::process::Child> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.driver_log())
        .map_err(|source| Error::Ledger {
            path: paths.driver_log(),
            source,
        })?;
    let errors = log.try_clone().map_err(|source| Error::Ledger {
        path: paths.driver_log(),
        source,
    })?;
    let exe = std::env::current_exe().map_err(|e| {
        Error::Invalid(format!(
            "cannot find this executable to retain a driver: {e}"
        ))
    })?;
    let mut command = std::process::Command::new(exe);
    command.arg(engine::DRIVE_VERB).arg(&paths.run);
    if retained == Retained::Adopting {
        command.arg(format!("--{ADOPT_FLAG}"));
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(errors)
        .spawn()
        .map_err(|e| Error::Invalid(format!("cannot retain a driver for '{}': {e}", paths.run)))
}

/// `onepipeline drive-run` — the retained driver of a detached launch.
///
/// Hidden, and the same loop an attached launch runs in-process: what differs is
/// only which process it runs in. It claims the run in the launch record — one
/// writer of that field, so the launcher never races it — and launches the
/// observer itself, so a `stop` that reaps this process's tree reaps the
/// observer with it.
///
/// `--adopt` says the run is one it is **taking over**, and the adoption is
/// recorded here for the same reason the pid is: this is the process that holds
/// the lock, and the counter, the dead driver's record moved aside, and the
/// journal entry are all things an adoption does under it. The record itself is
/// written once, below, with the observer up and this process's pid on it —
/// so a detaching launcher's wait ends on a driver that is running, and no
/// reader ever sees the run naming a process that is not driving it.
fn drive_run(args: &DriveRunArgs) -> Result<i32> {
    let paths = resolve(&args.run)?;
    // The lock before the claim: a driver that wrote its pid into the record and
    // then lost the race for the lock would leave the run naming a process that
    // is gone, and every reader would call it undriven while the driver that
    // won was still working.
    let lock = engine::claim(&paths)?;
    let mut record: LaunchRecord = ledger::read_json(&paths.launch())?;
    let view = RunView::open(&paths)?;
    let log = paths.driver_log();
    if args.adopt {
        take_the_run_over(&paths, &mut record)?;
        report_and_journal_adoption(&paths, &record, &view)?;
    }
    let goal = view
        .state
        .plan
        .as_ref()
        .and_then(|plan| plan.goal.as_ref())
        .map(|goal| goal.text.clone());
    let output = agentgraph::GraphOutput::Logged(&log);
    let mut observer = observe(&paths, &mut record, goal.as_deref(), output)?;
    record.driven_by_this_process();
    ledger::write_json(&paths.launch(), &record)?;

    // Scoped, so the watch is joined before this returns rather than left holding
    // the observer of a driver that has finished with it.
    let settled = {
        let driving = AtomicBool::new(true);
        let watched = observer.as_mut();
        // The record as it stands *now*, with this process's own claim on it:
        // the watch is the only writer of it from here, so a restart records the
        // graph it started beside the driver that started it.
        let mut watch = ObserverWatch::of(&paths, record, goal, output);
        std::thread::scope(|scope| {
            scope.spawn(|| keep_the_run_watched(watched, &mut watch, &driving));
            let settled = engine::drive_holding(&paths, lock);
            driving.store(false, Ordering::Release);
            settled
        })?
    };
    if let Some(run) = observer.as_mut() {
        run.cancel();
    }
    Ok(settled.exit_code())
}

/// Keep the run watched, and reap the graph that stopped watching it.
///
/// The reaping is load-bearing on its own: unreaped, a dead observer is a zombie
/// for the life of this driver, and a zombie answers a liveness probe as the
/// live process it is not — so the `owner.lock` the views read the observer's
/// verdict off would go on naming a graph nothing is running.
///
/// What it does *next* is start another one. A run whose observer has gone
/// executes with nothing comparing what it is doing against what it was asked to
/// do, while every view goes on reporting a driver hard at work — and the only
/// remedy an operator had was to `adopt`, which ends every dispatch running
/// beside it. So the driver that noticed is the one that fixes it.
///
/// Bounded by [`ObserverWatch`], and it stops at the first restart that will not
/// start: a relaunch loop against a graph that cannot run is worse than no
/// relaunch, because it spends a whole agent-graph launch per turn of this loop
/// and reports nothing for any of them.
fn keep_the_run_watched(
    observer: Option<&mut agentgraph::GraphRun>,
    watch: &mut ObserverWatch<'_>,
    driving: &AtomicBool,
) {
    let Some(observer) = observer else {
        return;
    };
    while driving.load(Ordering::Acquire) {
        if observer.has_exited() {
            observer_stopped_watching(&watch.paths.run);
            if watch.restart(observer).is_none() {
                return;
            }
        }
        std::thread::sleep(ATTACH_POLL);
    }
}

/// What the driver says on its own log when the graph watching the run stops.
///
/// One sentence for both driving paths, because an operator reading a detached
/// run's log and one watching an attached launch are reading the same event.
fn observer_stopped_watching(run: &str) {
    eprintln!(
        "onepipeline: the observer graph for '{run}' has stopped watching; \
         the run is still being driven"
    );
}

/// How many times one driver starts a run's observer graph again after it has
/// stopped watching.
///
/// A bound on **spending**, not a retry schedule: every restart is a whole
/// agent-graph launch, and a graph that starts and stops at once would otherwise
/// be relaunched for as long as the run is driven. Eight covers the failure this
/// exists for over a day-long run — an observer whose conversation runs out of
/// turns, which recurs on the scale of hours — while an observer that cannot
/// stay up spends the whole budget in seconds and then stops. What the bound did
/// is written to the launch record either way, so a run nothing is going to
/// watch again reads as unwatched rather than as one being quietly retried.
const DEFAULT_OBSERVER_RESTARTS: u32 = 8;

/// The environment variable that moves that bound.
const OBSERVER_RESTARTS_ENV: &str = "ONEPIPELINE_OBSERVER_RESTARTS";

/// The bound this driver restarts under.
///
/// `0` is a **value** here rather than an unusable one, which is why it is not
/// filtered away as the other bounds in this crate filter theirs: it says never
/// restart, which is exactly what every run did before this existed and is the
/// off switch for an operator who would rather be told and intervene. Only an
/// absent or unreadable value falls back to the default.
fn observer_restart_limit() -> u32 {
    std::env::var(OBSERVER_RESTARTS_ENV)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_OBSERVER_RESTARTS)
}

/// What starts another observer for a run being driven, and what stops it doing
/// so.
///
/// Held by the driver rather than by the graph, because a replacement is a
/// *launch* — the graph reference, the directory, the overrides and the source
/// filter the run was launched with — and the launch record is where all of that
/// is. It carries the record because this process is that record's single
/// writer: a restart moves the graph run a later `next` addresses the pacemaker
/// by, and a reader that went on addressing the graph that died would reset a
/// pacemaker nothing is running.
struct ObserverWatch<'a> {
    paths: &'a RunPaths,
    /// The run's launch record, as this driver holds it.
    record: LaunchRecord,
    /// The plan's goal, which is the only thing besides the run id that reaches
    /// the observer's task.
    goal: Option<String>,
    /// Where a replacement's envelopes go — the promise the driver that opened
    /// this watch made about itself. See [`launch_graph`].
    output: agentgraph::GraphOutput<'a>,
    /// The bound, and how much of it has been spent.
    limit: u32,
    restarted: u32,
}

impl<'a> ObserverWatch<'a> {
    /// The watch a driver opens over the observer it has just launched.
    fn of(
        paths: &'a RunPaths,
        record: LaunchRecord,
        goal: Option<String>,
        output: agentgraph::GraphOutput<'a>,
    ) -> Self {
        Self {
            paths,
            record,
            goal,
            output,
            limit: observer_restart_limit(),
            restarted: 0,
        }
    }

    /// Start another observer in place of the one that has stopped.
    ///
    /// `Some` where the run is being watched again — the handle is replaced in
    /// place, so a caller that relays what an observer says wires itself to the
    /// graph now in it. `None` where nothing will start another, and the launch
    /// record then says why, unless restarting was switched off in the first
    /// place.
    fn restart(&mut self, observer: &mut agentgraph::GraphRun) -> Option<()> {
        // Restarting switched off, which is a run's operator saying they would
        // rather be told and intervene. Nothing is recorded for it: the record
        // says why *a driver that restarts observers* stopped, and a driver that
        // was never going to restart has nothing to say beyond the sentence it
        // has already put on its log — leaving the verdict to the liveness probe
        // every build before this answered with.
        if self.limit == 0 {
            return None;
        }
        if self.restarted >= self.limit {
            return self.gave_out(format!(
                "this driver's bound of {} restart(s) is spent; take the run over to start \
                 another: onepipeline adopt {}",
                self.limit, self.paths.run
            ));
        }
        let started =
            match launch_graph(self.paths, &self.record, self.goal.as_deref(), self.output) {
                Ok(started) => started,
                // The launcher already refuses a graph that would not start, so this
                // is that refusal rather than a guess: one attempt, reported, and no
                // more. Retrying a graph that cannot run buys nothing and hides the
                // reason under the next attempt's.
                Err(error) => return self.gave_out(format!("it would not start again: {error}")),
            };
        self.restarted += 1;
        self.record.watched_by(
            started
                .run_id()
                .map(ToString::to_string)
                .unwrap_or_default(),
        );
        self.write_down();
        eprintln!(
            "onepipeline: started another observer graph for '{}' ({} of {} restart(s))",
            self.paths.run, self.restarted, self.limit
        );
        *observer = started;
        Some(())
    }

    /// Record that nothing is starting this run's observer again, and why.
    ///
    /// Always `None`, so the one call site reads as the answer it is: this watch
    /// is over, and the reason is now on the record every view reads.
    fn gave_out(&mut self, reason: String) -> Option<()> {
        eprintln!(
            "onepipeline: no observer graph is watching '{}': {reason}",
            self.paths.run
        );
        self.record.observer_ending = reason;
        self.write_down();
        None
    }

    /// Put what this watch has just recorded onto the run.
    ///
    /// Best effort, and the same answer either way: a record this driver could
    /// not write leaves a later `next` addressing the pacemaker of a graph that
    /// is gone, or a view reading a watch that is over as one still going. Both
    /// are worth saying out loud on the driver's own log, and neither is worth
    /// ending a working observer over.
    fn write_down(&self) {
        if let Err(error) = ledger::write_json(&self.paths.launch(), &self.record) {
            eprintln!(
                "onepipeline: could not record what is watching '{}': {error}",
                self.paths.run
            );
        }
    }
}

/// Start the dag-scope graph that **observes** the run.
///
/// `output` is the launcher's promise about itself: an attaching launcher stays
/// and relays what the observer produces, and a detaching one is about to exit,
/// so the observer is given a file instead of a pipe whose reader is going away.
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
    let mut launched = agentgraph::GraphRun::start(&agentgraph::Launch {
        graph: &record.graph,
        task: &task,
        dir: &recorded_dir(record)?,
        labels: &journal::labels(&paths.run, None),
        env: &[
            (agentgraph::RUN_ID_ENV.to_string(), paths.run.clone()),
            (
                ledger::RUNS_DIR_ENV.to_string(),
                ledger::runs_root().to_string_lossy().into_owned(),
            ),
        ],
        // Both pairs are the run's, and one driver drives one run, so one
        // process-wide copy is this launch's own answer as well as everyone
        // else's — which is what lets the observer stay in the driver.
        environment: agentgraph::Environment::Shared,
        sets: &record.dag_sets,
        // The observer graph is an `oneagentgraph` launch this run starts, so
        // the run's own say over that source reaches it like any other: the
        // launch is what does not relay, rather than this process reading the
        // firehose and dropping most of it.
        filter: record.filters.agentgraph.as_ref(),
        output,
    })?;
    // A launcher is the one caller that never waits for what it started, so a
    // graph that refused this launch would otherwise be reported as a running
    // observer — an exit 0 and a pid for a process that is already gone.
    launched.confirm_started()?;
    Ok(launched)
}

/// What the run is, for the one `--task` the observer graph is launched with.
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
/// so the next move is the planner's. Deliberately neither "the graph finished"
/// — the loop returns while independent branches may still have been dispatched
/// — nor "the observer exited", which says nothing about the run at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settlement {
    /// The graph completed successfully.
    Complete,
    /// A **blocking** decision point is outstanding and nothing else can move:
    /// the run will not advance until a reply or an attestation clears it.
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

/// Relay one observer graph's envelopes to the attach loop.
///
/// One relay per graph run, because a `GraphRun` hands its stream over once:
/// the observer an attach opens with and every replacement started after it each
/// get their own, all feeding the one channel the loop drains — so the merged
/// store holds what *this run's* observer said rather than what its first one
/// said.
///
fn relay_observer(
    paths: &RunPaths,
    run: &mut agentgraph::GraphRun,
    tx: std::sync::mpsc::Sender<crate::event::Envelope>,
) -> Result<()> {
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
        .map(|_| ())
        .map_err(|e| Error::Invalid(format!("cannot start the attach relay: {e}")))
}

/// Run the engine loop in this process and stream the run until it settles.
///
/// The loop runs on a thread of its own so this one can render. Reading it
/// inline would make the attach silent for the whole run, which is the opposite
/// of the streaming contract: surfaces and events reach the operator as they
/// occur.
///
/// The loop is what decides when to stop. It waits out every decision point that
/// still has work running beside it — an `attest` or a `reply` arriving then
/// resumes the paused subtree inside the running loop, with no external driver
/// action — and returns when the graph is terminal or when nothing at all can
/// move without the channel. Only then does this return.
fn attach(
    paths: &RunPaths,
    observer: Option<&mut agentgraph::GraphRun>,
    watch: &mut ObserverWatch<'_>,
    lock: ledger::OwnershipLock,
) -> Result<i32> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watched = observer;
    // The sender is kept as well as given away, because a restart needs one for
    // the observer it starts: what an attach relays into the merged store is
    // whichever graph is watching now, not the one this launch happened to open
    // with. It is released the moment nothing will start another, so the drain
    // below still ends on the relay closing rather than on its whole grace.
    let mut relay = Some(tx.clone());
    if let Some(run) = watched.as_deref_mut() {
        relay_observer(paths, run, tx)?;
    }
    let mut reported = 0usize;
    let mut observer_gone = false;
    let mut journal = Journal::open(paths);

    let driving = paths.clone();
    let engine = std::thread::Builder::new()
        .name(format!("engine-{}", paths.run))
        .spawn(move || engine::drive_holding(&driving, lock))
        .map_err(|e| Error::Invalid(format!("cannot start the engine loop: {e}")))?;

    loop {
        // Everything the observer emits joins the merged store, so an attach and
        // a later replay see the same stream.
        while let Ok(envelope) = rx.try_recv() {
            journal.relay(&envelope)?;
        }

        // Asked *before* the state is read, so the two cannot disagree in the
        // one direction that matters: the state is then at least as new as the
        // proof that the loop has stopped, and a run its own loop finished
        // settles as the `complete` it is rather than as an `unattended` this
        // loop merely looked at too early.
        // Reaped rather than merely probed: an observer nobody waits on stays a
        // zombie, and a zombie answers a liveness probe as alive. Said each time
        // one goes, so a graph that stopped watching a live run is visible to
        // the operator reading the stream — and then another is started, because
        // an attached run is being driven exactly as a detached one is and an
        // unwatched run is the same failure either way.
        if let Some(run) = watched.as_deref_mut() {
            if run.has_exited() && !observer_gone {
                observer_stopped_watching(&paths.run);
                match watch.restart(run) {
                    // Whatever the replacement says is still the run's, so it is
                    // relayed like its predecessor's. The sender is here to be
                    // cloned: it is released only once the loop has finished, and
                    // a finished loop starts no observer to relay.
                    Some(()) => {
                        if let Some(tx) = relay.as_ref() {
                            relay_observer(paths, run, tx.clone())?;
                        }
                    }
                    None => observer_gone = true,
                }
            }
        }

        let concluded = engine.is_finished();
        if concluded {
            // Nothing will start another observer for a run whose loop has
            // finished, so the sender kept for one is released here: the drain
            // below is waiting for every writing handle to close, and one held
            // for a relaunch that will never happen is one that never closes.
            relay = None;
            // The observer's last envelopes are still in flight between the
            // relay thread and this one. Collecting them before the settlement
            // is what makes the merged store the whole of what it said.
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
        // Unfiltered: an attached launch is streaming its own run's progress to
        // the person who started it, and a profile is what a *reader* of a run
        // chooses. There is nobody else here to have chosen one.
        let lines: Vec<String> = views::monitor(&view, &EventFilter::default())
            .lines()
            .map(str::to_string)
            .collect();
        for line in lines.iter().skip(reported) {
            eprintln!("{line}");
        }
        reported = lines.len();

        if concluded {
            // The observer has nothing left to observe — stopped **before** the
            // loop's own verdict is unwrapped, because that verdict may be a
            // refusal, and a launch that returned one having left a graph
            // running would leave an agent working on a run nobody is driving.
            if let Some(run) = watched.as_deref_mut() {
                run.cancel();
            }
            // Whatever the loop reported is this launch's failure to report: a
            // lock it could not take, a ledger it could not read.
            engine.join().map_err(|_| {
                Error::Invalid(format!("the engine loop for '{}' panicked", paths.run))
            })??;
            let settlement = settlement_of(&view);
            println!(
                "{}",
                json!({"run_id": paths.run, "settlement": settlement.as_str()})
            );
            return Ok(settlement.exit_code());
        }
        std::thread::sleep(ATTACH_POLL);
    }
}

/// How a run that has stopped advancing settled.
///
/// Re-derived from the graph and the channel rather than from any round state:
/// what makes a run `awaiting-planner` is an outstanding **decision point** —
/// a ready human action, or a blocking surface nobody has answered.
fn settlement_of(view: &RunView) -> Settlement {
    let statuses = view.state.statuses();
    if !statuses.is_empty() && graph::state_of(&statuses) == GraphState::Complete {
        return Settlement::Complete;
    }
    // A *non-blocking* surface is deliberately not `awaiting-planner`: it is a
    // report, and it holds nothing back.
    if views::decision_outstanding(&view.state, &view.paths) {
        return Settlement::AwaitingPlanner;
    }
    Settlement::Unattended
}

/// `onepipeline adopt`.
///
/// Adoption keeps everything the run owns and replaces only the driver: the run
/// id, the journal, and the ledger are the ones it already had.
///
/// `--attach` and `--detach` mean here what they mean on `start`, with the same
/// default: attached, this process is the fresh driver; detached, the driver it
/// retains is, and this one returns as soon as that driver has claimed the run.
fn adopt(args: &AdoptArgs) -> Result<i32> {
    let paths = resolve(&args.run)?;
    let (mut record, view) = validate_and_displace_for_adoption(&paths)?;

    if args.detach {
        // Nothing is written here, and the lock is not taken here either. The
        // process that will drive the run is the one that takes the lock, bumps
        // the adoption, and names itself — one writer of each — so a reader
        // between this line and the driver being up sees the run exactly as the
        // refusals above left it: undriven, and adoptable again. A launcher that
        // claimed on the driver's behalf would instead be publishing its own pid
        // for a run it is about to walk away from.
        sys::disown_standard_handles();
        let mut driver = retain_driver(&paths, Retained::Adopting)?;
        let pid = driver.id();
        confirm_driving(&paths, &mut driver)?;
        announce_launch(&paths.run, pid);
        return Ok(EXIT_SUCCESS);
    }

    // And the lock decides, before anything is written: an adoption that
    // recorded itself and *then* lost the race would leave the record naming
    // this process while the driver that won carried on.
    let lock = engine::claim(&paths)?;

    take_the_run_over(&paths, &mut record)?;
    ledger::write_json(&paths.launch(), &record)?;
    report_and_journal_adoption(&paths, &record, &view)?;

    // Relayed: an adoption attaches, so this process stays to read it. The goal
    // comes off the run's own projected plan rather than off the project the
    // launch named: a run whose graph the planner has edited since is still the
    // run this driver is adopting, and that project may have moved on or gone.
    //
    // The observer only, and only when the run was launched with one: what
    // adoption is *for* is the loop below, which this process runs itself. A
    // fresh observer is a fresh graph run with an id of its own, and the
    // pacemaker is addressed by that id — so [`observe`] is what records it,
    // exactly as it does for a launch, and the run names what is watching it now
    // rather than the graph that died.
    let goal = view
        .state
        .plan
        .as_ref()
        .and_then(|plan| plan.goal.as_ref())
        .map(|goal| goal.text.clone());
    let output = agentgraph::GraphOutput::Relayed;
    let mut observer = observe(&paths, &mut record, goal.as_deref(), output)?;
    ledger::write_json(&paths.launch(), &record)?;
    let mut watch = ObserverWatch::of(&paths, record, goal, output);
    // An adoption resumes the graph exactly where the journal left it, including
    // mid-decision: the fold reconstructs the outstanding decision points from
    // the settlements that produced them, so a subtree that was paused stays
    // paused and is released by the same `attest` it always was.
    attach(&paths, observer.as_mut(), &mut watch, lock)
}

/// The checks every adoption makes before anything is written, and the parked
/// driver it ends.
///
/// Both launch paths refuse for the same reasons and refuse **here**, in the
/// process the operator is watching: a detaching adoption that left them to the
/// driver it retains would answer "not yours" and "still being driven" through a
/// log file nobody is reading.
///
/// The view is the one the liveness verdict was read off, and it is what the
/// adoption goes on to say the dead driver abandoned — the same read, so the two
/// cannot disagree about the run they are describing.
fn validate_and_displace_for_adoption(paths: &RunPaths) -> Result<(LaunchRecord, RunView)> {
    let session = sys::launching_session();
    let record: LaunchRecord = ledger::read_json(&paths.launch())?;

    // Ownership is the same rule `stop` keeps, including `unknown` never being
    // yours.
    if !record.owned_by(&session) {
        return Err(Error::NotOwned {
            run: paths.run.clone(),
            owner: record.owner_label(&session),
        });
    }
    let view = RunView::open(paths)?;
    if !view.liveness().is_undriven() {
        return Err(Error::Refused(format!(
            "run '{}' is still being driven; end it with `onepipeline stop {}` first",
            paths.run, paths.run
        )));
    }
    // A driver this host has proved is *not working* still holds the run's
    // ownership lock, and the loop this adoption is about to start is the run's
    // single writer — so taking the run over means ending the process that had
    // it. Only ever a driver the verdict above already called undriven: a run
    // still being driven was refused, and this is the same taking-over `adopt`
    // has always been. A dead one is signalled to no effect, which is the
    // ordinary case.
    displace_the_parked_driver(&record);
    Ok((record, view))
}

/// Count the adoption, claim the run for this process, and move the dead
/// driver's record aside.
///
/// Called only under the ownership lock, by the process that holds it — which
/// attached is the adopting launcher and detached is the driver it retained.
/// The copy is taken before the record is written, because what it preserves is
/// what the record said *before* this adoption: it is the first thing to read
/// after adopting, and truncating it would lose the account of the driver that
/// died.
fn take_the_run_over(paths: &RunPaths, record: &mut LaunchRecord) -> Result<()> {
    record.adoptions += 1;
    record.driven_by_this_process();
    let previous = paths
        .dir
        .join(format!("launch.pre-adopt-{}.json", record.adoptions));
    std::fs::copy(paths.launch(), &previous).map_err(|source| Error::Ledger {
        path: previous,
        source,
    })?;
    Ok(())
}

/// Say what the driver being replaced was in the middle of, on the run's own
/// journal and on this process's stderr.
///
/// Recorded with the adoption itself, because the fresh loop will dispatch these
/// nodes again and the session each one was working in is the only record of
/// where its commits are: named here, that branch is in the run's own account of
/// the adoption rather than only in a process that has exited.
fn report_and_journal_adoption(
    paths: &RunPaths,
    record: &LaunchRecord,
    view: &RunView,
) -> Result<()> {
    let abandoned = view.state.sessions_in_flight();
    for (node, session) in &abandoned {
        eprintln!(
            "onepipeline: '{node}' had a dispatch in flight; its work is on branch \
             '{}' in onevcs session {}, and the node is pinned there so the run \
             continues that branch rather than cutting a second one beside it",
            session.branch(),
            session.token().0
        );
    }
    let mut adopted = vec![
        ("adoption", json!(record.adoptions)),
        ("pid", json!(record.pid)),
    ];
    if !abandoned.is_empty() {
        adopted.push((
            journal::ADOPTED_ABANDONED,
            json!(abandoned
                .iter()
                .map(|(node, session)| json!({
                    "node": node,
                    "session": session.token().0,
                    "branch": session.branch().as_str(),
                }))
                .collect::<Vec<_>>()),
        ));
    }
    let mut journal = Journal::open(paths);
    journal.emit(
        journal::PipelineKind::DriverAdopted,
        journal::labels(&paths.run, None),
        journal::payload(&adopted),
    )
}

/// End a driver that holds a run nothing is driving, and wait for it to go.
///
/// The lock the engine loop takes is reclaimable only from a holder this host
/// can prove is gone, so an adoption that took its lock beside a parked driver
/// would lose the race — leaving the one documented way back from `PARKED`
/// closed. The wait is bounded and answers nothing itself: whether the run may
/// be taken over is the **lock's** question, and a driver that outlasts this is
/// answered by the lock's own refusal, which names the pid still holding it.
fn displace_the_parked_driver(record: &LaunchRecord) {
    if record.host != sys::hostname() || !sys::process_may_be_live(record.pid) {
        return;
    }
    eprintln!(
        "onepipeline: run '{}' is held by driver pid {}, which is not working; \
         ending it to adopt the run",
        record.run_id, record.pid
    );
    sys::stop(record.pid, sys::Stop::Politely);
    let deadline = Instant::now() + DRIVER_HANDOVER;
    while Instant::now() < deadline && sys::process_may_be_live(record.pid) {
        std::thread::sleep(ATTACH_POLL);
    }
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

    // Attempted before the record is written, so the record says what happened
    // rather than what was about to be tried — and refused before either, where
    // this build cannot establish what the run is running. Nothing is signalled
    // and no stop is recorded on that path: a run whose registry cannot be read
    // is one nobody can say is idle, and writing "stopped" over it would be the
    // same false completion this verb refuses everywhere else.
    let teardown = terminate(&paths, &record).map_err(|why| {
        Error::Refused(format!(
            "run '{}' was not stopped: this build cannot establish what it is running — {why}. \
             The run is untouched; nothing was signalled. Fix or remove the entry the path \
             above names and run `onepipeline stop {}` again",
            paths.run, paths.run
        ))
    })?;
    let established = match teardown {
        None => journal::StopTeardown::Elsewhere,
        Some(sys::Teardown::Signalled) => journal::StopTeardown::Signalled,
        Some(sys::Teardown::NothingToStop) => journal::StopTeardown::NothingToStop,
        Some(sys::Teardown::IdentityDeclined) => journal::StopTeardown::IdentityDeclined,
        Some(sys::Teardown::NotAttempted) => journal::StopTeardown::NotAttempted,
        Some(sys::Teardown::PartlySignalled) => journal::StopTeardown::PartlySignalled,
        // Unix-only, as the variant is: no Windows teardown establishes it.
        #[cfg(unix)]
        Some(sys::Teardown::Refused) => journal::StopTeardown::Refused,
    };
    let mut journal = Journal::open(&paths);
    journal.emit(
        journal::PipelineKind::RunStopped,
        journal::labels(&paths.run, None),
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
                "run '{run}' was not stopped: this host gave no answer its tree could be \
                 read from — no process listing, or nothing that says whether a pid it \
                 recorded is still the process it named, each said above — so the \
                 processes the run started could not be found, and ending its driver \
                 alone would have orphaned them. The run is untouched — run \
                 `onepipeline stop {run}` again once this host answers"
            )));
        }
        Some(sys::Teardown::PartlySignalled) => {
            return Err(Error::Refused(format!(
                "run '{run}' was only partly stopped: part of its process tree was \
                 signalled and at least one process in it is still running — one this \
                 session could not signal, or one that took the ask and stayed. Find it \
                 in this host's process list and end it as the user that owns it"
            )));
        }
        Some(sys::Teardown::IdentityDeclined) => {
            return Err(Error::Refused(format!(
                "run '{run}' was not stopped: live processes were found, but every recorded \
                 identity disagreed with the process now holding its pid, so none was safe \
                 to signal. This is distinct from a run with nothing left to stop; inspect \
                 the declined claims above and retry only after correcting the run records"
            )));
        }
        // llmlint: ignore-block[changed_behavior_has_e2e] this arm has no journey and
        // cannot have one: reaching it takes a run every process of which refuses this
        // user's signal, and a process this user may not signal is not a thing for a
        // suite to go and make — the same reason `sys::established` is a fold driven
        // from the answers a round of signalling gives rather than from signals, and the
        // reason the Windows teardown arm carries this directive too. What the arm is
        // built from is proved there, at
        // `a_teardown_refused_by_everything_it_aimed_at_reports_no_signal_at_all` and
        // `a_stop_that_could_signal_nothing_it_aimed_at_says_so`; every other outcome
        // this match renders is driven end to end in `tests/e2e/driver.rs`.
        #[cfg(unix)]
        Some(sys::Teardown::Refused) => {
            return Err(Error::Refused(format!(
                "run '{run}' was not stopped: its process tree was found and every \
                 process in it refused this session's signal, so nothing was signalled \
                 and all of it is still running. Running `onepipeline stop {run}` again \
                 as this user will be refused the same way — find the tree in this \
                 host's process list and end it as the user that owns it"
            )));
        } // llmlint: ignore-end[changed_behavior_has_e2e]
        None | Some(sys::Teardown::Signalled) | Some(sys::Teardown::NothingToStop) => {}
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

/// Ask everything driving this run on this host to stop, and watch it go.
///
/// Politely: a driver takes the ask first so it records its own abandonment
/// rather than vanishing. The host check is this caller's alone — a pid means
/// nothing across machines, and the ledger's records name which one each was
/// taken on.
///
/// `None` when nothing this run names is a process on this host, where nothing
/// was attempted and this host has nothing to promise either way.
///
/// A live pid no record can prove is **not** signalled and is not silently
/// dropped either: it downgrades what this promises, to `NotAttempted` where
/// nothing was signalled at all and to `PartlySignalled` where the rest of the
/// run was, so `stop` refuses rather than reporting a teardown over a process it
/// could not place. A teardown every process refused needs no downgrade — it
/// already says nothing was signalled and the run is still running — and keeps
/// its own answer. All three are what [`Aim::unproven`] exists to carry.
// llmlint: ignore-block[changed_behavior_has_e2e] every branch is driven end to end in
// `tests/e2e/driver.rs`, against real drivers and real dispatches: the proved claims and the
// stale record in `stopping_a_run_ends_the_tree_its_lock_names_when_the_record_names_a_dead_driver`,
// the reissued pid in `a_stop_never_signals_a_pid_the_host_has_given_to_another_process`, and
// the unprovable pid in `a_stop_that_cannot_read_the_process_table_refuses_and_leaves_the_run_retryable`,
// whose faulty `ps` is a host that will not say when anything started. What has no journey is a
// stop of two *live* roots, and that is a state a run cannot be in: the ownership lock is a
// single-writer lock, so one run has one driver, and the pair a stop can meet — the pid a
// stale record names beside the pid the lock stamps — is what the first of those walks over
// one listing.
fn terminate(paths: &RunPaths, record: &LaunchRecord) -> Result<Option<sys::Teardown>> {
    let Aim::Here {
        roots,
        unproven,
        declined,
    } = roots_to_stop(paths, record)?
    else {
        return Ok(None);
    };
    if roots.is_empty() && unproven.is_empty() && !declined.is_empty() {
        return Ok(Some(sys::Teardown::IdentityDeclined));
    }
    let established = sys::stop_and_confirm(&roots, sys::Stop::Politely, TEARDOWN_PATIENCE);
    if unproven.is_empty() {
        return Ok(Some(established));
    }
    Ok(Some(match established {
        // Nothing was signalled and the run is exactly as it was, which is what
        // a retry rests on.
        sys::Teardown::NothingToStop | sys::Teardown::NotAttempted => sys::Teardown::NotAttempted,
        // `stop_and_confirm` never manufactures this caller-level outcome, but
        // if another platform can establish it directly it remains the most
        // precise answer after adding an unproven claim beside it.
        sys::Teardown::IdentityDeclined => sys::Teardown::IdentityDeclined,
        // Part of the run was signalled and something on this host is still
        // running that this teardown was not entitled to touch.
        sys::Teardown::Signalled | sys::Teardown::PartlySignalled => sys::Teardown::PartlySignalled,
        // Nothing was signalled either, but a retry does not rest on it: what
        // stood in the way was this user's own entitlement, which is what stands
        // in the way of the unproven pid beside it too. Left as it is rather
        // than downgraded to `NotAttempted`, whose promise — the same ask works
        // once the host answers — is the one thing that is not true here.
        #[cfg(unix)]
        sys::Teardown::Refused => sys::Teardown::Refused,
    }))
}

/// What a stop of this run may aim at on this host.
///
/// Two answers rather than a flag beside a list, because the flag's other
/// combination is not a state a run can be in: a pid this host may aim at is one
/// a record of this host's named, so "nothing here is this run's" and "here is
/// what this run is running" cannot both be true, and only one of them can carry
/// pids.
#[derive(Debug, PartialEq, Eq)]
enum Aim {
    /// Nothing this run names is a process on this host. Nothing is attempted
    /// and this host promises nothing either way — deliberately not the same
    /// answer as a run of this host's whose processes are all over, which is a
    /// teardown that looked and found nothing left to stop.
    Elsewhere,
    /// The run is this host's, as far as its records say.
    Here {
        /// The roots, in the order they are signalled: every claim whose own
        /// stamp proves its pid is still the process the record named.
        roots: Vec<u32>,
        /// Live pids on this host that no record could place either way. What
        /// stood in the way of proving each is said on stderr where it is met.
        ///
        /// Never signalled — that is the whole point — and never ignored either.
        /// A teardown that dropped these would report a clean stop over a
        /// process that may well be the run's own driver.
        unproven: Vec<u32>,
        /// Live pids whose readable start token disagreed with their record.
        /// They are strangers, so they are never signalled; retaining them is
        /// what distinguishes an all-declined walk from an empty one.
        declined: Vec<u32>,
    },
}

/// Every process on this host a stop of this run aims at, or why this build
/// cannot say.
///
/// Three records, three questions. The **launch record** names the driver the run
/// was launched or last adopted with, which is a claim about the past: a driver
/// that died leaves its pid sitting there until something adopts the run. The
/// **ownership lock** names whatever is driving the run now. The **registry**
/// names the work itself — the one record that survives the driver that started
/// it, and the only way a stop reaches a dispatch that has outlived one.
///
/// Reading the registry is therefore the one failure that is **fatal to the
/// stop**. The other two can only add a root, so one that cannot be read costs
/// reach; a registry that cannot be read is a run nobody can say is idle, and the
/// caller refuses rather than reporting a teardown it did not make.
///
/// **Every** claim is aimed at only where its own stamp proves it, the launch
/// record included, because a pid the host has since reissued is somebody else's
/// tree — and the pid a stop is likeliest to have reissued is exactly that one:
/// it is the oldest claim a run holds, it is left behind by every driver that
/// dies, and nothing rewrites it until an adoption does.
fn roots_to_stop(paths: &RunPaths, record: &LaunchRecord) -> Result<Aim> {
    let here = sys::hostname();
    let mut on_this_host = false;
    let mut roots: Vec<u32> = Vec::new();
    let mut unproven: Vec<u32> = Vec::new();
    let mut declined: Vec<u32> = Vec::new();
    // Each claim in turn, and the launch record first, so a teardown asks the
    // driver to go before the work it started: the record's driver, then the
    // lock's holder, then every dispatch the run has recorded.
    let claimed = std::iter::once((
        RECORDED_DRIVER,
        record.pid,
        record.host.clone(),
        record.started.clone(),
    ))
    .chain(lock_held_on(paths).map(|held| (LOCK_HOLDER, held.pid, held.host, held.started)))
    .chain(ledger::dispatches_of(paths)?.into_iter().map(|running| {
        (
            REGISTERED_DISPATCH,
            running.pid,
            running.host,
            running.started,
        )
    }));
    for (named_by, pid, host, started) in claimed {
        if host != here {
            continue;
        }
        on_this_host = true;
        if roots.contains(&pid) || unproven.contains(&pid) {
            continue;
        }
        match claim_on(pid, &started) {
            Claim::Proved => roots.push(pid),
            Claim::Gone => {}
            Claim::Reissued => {
                eprintln!(
                    "onepipeline: run '{}': the {named_by} names pid {pid}, which this host has \
                     since given to another process, so it was not signalled",
                    paths.run
                );
                declined.push(pid);
            }
            Claim::Unstamped => {
                left_alone(
                    &paths.run,
                    named_by,
                    pid,
                    "its record carries no start token",
                );
                unproven.push(pid);
            }
            Claim::HostSilent => {
                left_alone(
                    &paths.run,
                    named_by,
                    pid,
                    "this host will not say when it started",
                );
                unproven.push(pid);
            }
        }
    }
    if !on_this_host {
        return Ok(Aim::Elsewhere);
    }
    Ok(Aim::Here {
        roots,
        unproven,
        declined,
    })
}

/// Say out loud that a live pid was left alone, and what stood in the way of
/// placing it.
///
/// A teardown that is narrower than the one an operator asked for has to say so
/// where it happens: the refusal that follows knows only that something could
/// not be established, and this is the line that names which pid and why.
fn left_alone(run: &str, named_by: &str, pid: u32, why: &str) {
    eprintln!(
        "onepipeline: run '{run}': the {named_by} names pid {pid}, which is running on this \
         host — {why}, so nothing says it is still this run's process and it was not signalled"
    );
}
// llmlint: ignore-end[changed_behavior_has_e2e]

/// How a teardown names the record a pid came off, when it says out loud that it
/// did not aim there.
const RECORDED_DRIVER: &str = "launch record";
/// The claim being driven now.
const LOCK_HOLDER: &str = "ownership lock";
/// The claim that outlives the driver that wrote it.
const REGISTERED_DISPATCH: &str = "dispatch registry";

/// What one record says about one pid on this host.
#[derive(Debug, PartialEq, Eq)]
enum Claim {
    /// Still the process the record was written for: its stamp says so.
    Proved,
    /// Not a process this teardown has anything to end — the pid is gone.
    Gone,
    /// A live process this host says is **not** the one the record named. The
    /// recorded process is over and its pid has been handed on, so there is
    /// nothing here to stop and nothing unresolved either.
    Reissued,
    /// A live process whose record carries no stamp to compare — every record a
    /// build before the field existed wrote.
    Unstamped,
    /// A live process this host would not describe, so there was nothing to
    /// compare its record against.
    HostSilent,
}

/// Whether `pid` is still the process a record stamped `started` was written
/// for.
///
/// The order of the answers is the point. A stamp that matches is the only
/// proof, and everything else is read against whether the pid is a process at
/// all: one that is gone ends the question, and one that is live is either
/// somebody else's — the host answered with a different stamp — or a pid this
/// build has nothing to compare, which is *cannot say* rather than *nothing is
/// running there*.
fn claim_on(pid: u32, started: &str) -> Claim {
    let reading = sys::process_start_token(pid);
    match reading {
        Some(ref token) if token.matches(started) => Claim::Proved,
        _ if !sys::process_may_be_live(pid) => Claim::Gone,
        Some(_) if !started.is_empty() => Claim::Reissued,
        Some(_) => Claim::Unstamped,
        None => Claim::HostSilent,
    }
}

/// The run's ownership lock, as a record, when there is one this build can read.
///
/// A lock that is **not there** and one this build cannot read are different
/// answers and only the first is silent. Neither gives a teardown a pid it may
/// aim at — an unreadable claim names nobody — but the second means the run is
/// held by something whose record this build does not understand, and a stop
/// that swallowed that would leave an operator reading a teardown narrower than
/// they asked for with nothing saying why. Said out loud and not refused: the
/// recorded driver is still aimed at, and refusing the stop over a corrupt lock
/// would leave a live run running.
fn lock_held_on(paths: &RunPaths) -> Option<ledger::LockRecord> {
    let path = paths.lock();
    match ledger::read_json::<ledger::LockRecord>(&path) {
        Ok(held) => Some(held),
        Err(Error::Ledger { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            eprintln!(
                "onepipeline: the ownership lock of run '{}' cannot be read, so this stop aims \
                 only at the driver the launch record names: {error}",
                paths.run
            );
            None
        }
    }
}

/// `onepipeline next` — the channel's only consumer.
///
/// Rendering is not reading: `monitor` shows a pending surface without
/// consuming it, and this is what advances the queue and resets the pacemaker.
fn next(args: &ReadArgs) -> Result<i32> {
    let paths = resolve(&args.run)?;
    let view = RunView::open(&paths)?;
    // Read before anything is claimed, so a spec this run cannot honour refuses
    // the read rather than consuming a surface into an output that then fails to
    // render.
    let filter = read_filter(&view, args)?;
    let events = views::shaped(&view, &filter);
    let channel = ChannelState::new(&paths);

    let Some(surface) = channel.claim()? else {
        let settled = view.liveness().is_undriven();
        let status = if settled { "finished" } else { "running" };
        println!(
            "{}",
            json!({"status": status, "surface": null, "events": events})
        );
        return Ok(EXIT_SUCCESS);
    };

    let mut journal = Journal::open(&paths);
    journal.emit(
        journal::PipelineKind::PlannerSurfaced,
        journal::labels(&paths.run, surface.workstream.as_deref()),
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

    // The surface is delivered whatever the profile said. A profile shapes the
    // **event view** and nothing else: which surfaces exist, and the unread
    // accounting over them, belong to the channel, so a blocking surface reaches
    // its planner under the narrowest profile a run has.
    println!(
        "{}",
        json!({"status": "surface", "surface": surface, "events": events})
    );
    Ok(EXIT_SUCCESS)
}

/// The surface's text: `--message`, the named file, or stdin, in that order.
///
/// Trimmed at its ends as `reply` trims the envelope it reads, so `echo` and a
/// heredoc do not queue a trailing newline; nothing inside it is touched.
fn surface_message(args: &SurfaceArgs) -> Result<String> {
    let (body, whence) = match (&args.message, &args.file) {
        (Some(message), _) => (message.clone(), "`--message`"),
        (None, Some(path)) => (
            std::fs::read_to_string(path).map_err(|e| Error::Ledger {
                path: path.clone(),
                source: e,
            })?,
            "the file",
        ),
        (None, None) => {
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string_compat(&mut buffer)
                .map_err(|e| Error::Refused(format!("cannot read the message from stdin: {e}")))?;
            (buffer, "stdin")
        }
    };
    let body = body.trim();
    if body.is_empty() {
        return Err(Error::Refused(format!(
            "a surface carries what it has to say and {whence} carried nothing; \
             give the message on stdin, as a file argument, or with `--message TEXT`"
        )));
    }
    Ok(body.to_string())
}

/// `onepipeline surface`.
fn surface(args: &SurfaceArgs) -> Result<i32> {
    let paths = resolve(&args.run)?;
    // Read the body before anything is queued, so an unreadable file or an empty
    // stdin refuses the command rather than queuing a surface with nothing in it.
    let message = surface_message(args)?;
    // What it is about, and what raised it, are two facts: a check-in is the
    // pacemaker's own, and a finding typed here is advice like any other.
    let source = match args.kind {
        SurfaceKind::CheckIn => crate::channel::source::CHECK_IN,
        SurfaceKind::Finding => crate::channel::source::PROPOSAL,
    };
    let queued = ChannelState::new(&paths).push(Surface {
        id: 0,
        kind: args.kind.as_str().to_string(),
        message,
        source: source.to_string(),
        // Neither is a request: a pacemaker update and a finding typed at this
        // verb are reports, and never hold a subtree back waiting for a
        // decision. A finding that means to stop one says so through the
        // envelope's `finding` op, which carries `blocking`.
        blocking: false,
        queued_at: sys::now_millis(),
        abandoned: false,
        workstream: None,
    })?;
    let mut journal = Journal::open(&paths);
    journal.emit(
        journal::PipelineKind::PlannerSurfaceQueued,
        journal::labels(&paths.run, None),
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
            // The person who took the action, through the planner's own channel:
            // `attest` is not an op an observer may issue at all.
            author: Author::Planner,
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
    // planner mistake as a task carrying one, and deserves the same answer.
    let envelope: Reply = serde_json::from_str(text.trim()).map_err(|e| {
        let why = serde_json::from_str::<serde_json::Value>(text.trim())
            .ok()
            .as_ref()
            .and_then(crate::plan::retired_field_refusal)
            .unwrap_or_else(|| e.to_string());
        Error::Refused(format!("the reply is malformed: {why}"))
    })?;
    submit(&paths, &envelope)
}

/// What one submitted envelope became.
///
/// An answer rather than an exit code, because two callers read it: `reply`
/// prints the state it names and returns the code that goes with it, and
/// [`crate::note::deliver`] reads what the delivery answered out of it.
///
/// **Applied is two variants and not one with an `Option`**, because which
/// process applied the commands decides both of the other two facts and they do
/// not vary independently: the process that compiled them has them in hand and
/// has no queue id, and the one that handed them to the run's reconciler has the
/// queue id and never sees what they became. A single variant carrying both as
/// options would spell two more states — a local apply with nothing compiled, a
/// reconciled one carrying operations — that no path can reach and every reader
/// would still have to answer for.
enum Submitted {
    /// A commandless verdict, queued for whichever reader the run owes one.
    Answered {
        /// The reply's id in the channel.
        reply: u64,
    },
    /// Every command applied **by this process**, which took the run's ownership
    /// lock because nothing was driving it.
    AppliedHere {
        /// What they compiled to. In hand, so a caller needs no second read of
        /// the record to find out what it just did.
        operations: Vec<edits::Operation>,
    },
    /// Every command applied **by the run's own reconciler**, which is the
    /// writer while a driver holds the run. What they became is in its record
    /// rather than here.
    AppliedByRun {
        /// The reply's id in the channel.
        reply: u64,
    },
    /// Accepted and durable, and not reconciled within the reply timeout. Still
    /// queued: **not** an instruction to send it again.
    Queued {
        /// The reply's id in the channel.
        reply: u64,
    },
}

/// Validate a reply, queue it, and report which of the four true things happened.
fn submit(paths: &RunPaths, envelope: &Reply) -> Result<i32> {
    match submit_envelope(paths, envelope)? {
        Submitted::Answered { reply } => {
            println!("{}", json!({"reply": reply, "state": "delivered"}));
            Ok(EXIT_SUCCESS)
        }
        // `0` is this process's own apply: there was no queue to put it in, so
        // there is no id in the channel to name.
        Submitted::AppliedHere { .. } => {
            println!("{}", json!({"reply": 0, "state": "applied"}));
            Ok(EXIT_SUCCESS)
        }
        Submitted::AppliedByRun { reply } => {
            println!("{}", json!({"reply": reply, "state": "applied"}));
            Ok(EXIT_SUCCESS)
        }
        Submitted::Queued { reply } => {
            println!("{}", json!({"reply": reply, "state": "queued"}));
            Ok(EXIT_QUEUED)
        }
    }
}

/// Deliver one note through the channel's own path, and answer what the
/// conversation said.
///
/// [`crate::note::deliver`]'s whole implementation, kept here because it is the
/// submission path rather than a second one beside it: the op a planner types and
/// the call a consumer makes are validated by one check, judged by one reconciler,
/// and recorded once.
///
/// # Errors
///
/// [`Error::Refused`] for a note that was not delivered, in the conversation's own
/// words; or the reason the ask was not one this run could act on.
pub(crate) fn deliver_note_envelope(
    paths: &RunPaths,
    envelope: &Reply,
) -> Result<crate::note::Delivered> {
    let [Command::Note { id, text, .. }] = &envelope.commands[..] else {
        return Err(Error::Refused(
            "a note is delivered one at a time, and this envelope carries something else"
                .to_string(),
        ));
    };
    let (id, text) = (id.clone(), text.clone());
    match submit_envelope(paths, envelope)? {
        // Compiled here, so what it answered is in hand.
        Submitted::AppliedHere { operations } => reached_in(&operations)
            .map(crate::note::Delivered::To)
            .ok_or_else(|| {
                Error::Refused(format!(
                    "note: node '{id}': the delivery recorded no disposition"
                ))
            }),
        // Compiled by the run's own reconciler, which recorded it: the record is
        // written before the outcome this returned on, so it is there to be read.
        Submitted::AppliedByRun { .. } => last_note_delivered(paths, &id, &text)?
            .map(crate::note::Delivered::To)
            .ok_or_else(|| {
                Error::Refused(format!(
                    "note: node '{id}': the run applied the note and recorded no disposition \
                     for it"
                ))
            }),
        // llmlint: ignore-block[changed_behavior_has_e2e] this arm is the reply
        // timeout elapsing, which is read from the *calling process's*
        // environment — and this call is a library call, so a journey driving it
        // would have to mutate the test binary's own environment while its other
        // journeys run in parallel threads. The op's own timeout path is driven
        // end to end by `context_delivery`, through the envelope, where the bound
        // is a subprocess's to set.
        Submitted::Queued { .. } => Ok(crate::note::Delivered::Queued),
        // llmlint: ignore-end[changed_behavior_has_e2e]
        Submitted::Answered { .. } => Err(Error::Refused(
            "a note carries a command, and this envelope was answered as a verdict".to_string(),
        )),
    }
}

/// The disposition one compiled note recorded.
fn reached_in(operations: &[edits::Operation]) -> Option<crate::note::Reached> {
    operations.iter().find_map(|operation| match operation {
        edits::Operation::NoteDelivered { reached, .. } => Some(reached.clone()),
        _ => None,
    })
}

/// What the run recorded for the last note of this text delivered to this node,
/// or nothing where it recorded none.
///
/// # Errors
///
/// [`Error::Invalid`] for a committed edit whose recorded operations are not a
/// shape this build reads. That is the run's own record being unreadable, and it
/// is **not** the same answer as no disposition: the caller turns the second into
/// "the run applied the note and recorded no disposition for it", which would
/// send a manager looking for a delivery that is sitting in the journal all
/// along. So it is said rather than folded into that.
fn last_note_delivered(
    paths: &RunPaths,
    node: &str,
    text: &crate::note::NoteText,
) -> Result<Option<crate::note::Reached>> {
    for envelope in journal::read(&paths.journal()).into_iter().rev() {
        if envelope.kind.0 != journal::PipelineKind::EditCommitted.as_str() {
            continue;
        }
        let Some(recorded) = envelope.payload.get("operations") else {
            continue;
        };
        let operations: Vec<edits::Operation> =
            serde_json::from_value(recorded.clone()).map_err(|why| {
                Error::Invalid(format!(
                    "run '{}': a committed edit at seq {} records operations this build does \
                     not read, so what it did with a note cannot be said: {why}",
                    paths.run, envelope.seq
                ))
            })?;
        let reached = operations.iter().find_map(|operation| match operation {
            edits::Operation::NoteDelivered {
                node: written,
                text: said,
                reached,
                ..
            } if written == node && said == text => Some(reached.clone()),
            _ => None,
        });
        if reached.is_some() {
            return Ok(reached);
        }
    }
    Ok(None)
}

/// Validate a reply and queue it, or apply it, and say which happened.
///
/// The author's op allowlist is enforced here, before anything is queued: a
/// monitor that asks for an op it may not issue is refused with the reason, and
/// nothing durable is written on its behalf.
fn submit_envelope(paths: &RunPaths, envelope: &Reply) -> Result<Submitted> {
    let view = RunView::open(paths)?;
    let channel = ChannelState::new(paths);

    if envelope.commands.is_empty() {
        // The verdict is subject to the author's allowlist exactly as an op is:
        // a commandless reply declaring the run finished says what `complete`
        // says, and an allowlist that guarded only the ops would let it past.
        crate::channel::allows_completion(envelope.author, envelope.completion)?;
        // A settled run has no reader left, now or later, so queuing a reply to
        // it would park it where nothing drains it. A surface still awaiting an
        // answer outranks that: the run asked for the reply.
        //
        // Decided from the run's **own** settled state and never from the
        // liveness verdict, which answers a different question: a driver writes
        // the result, releases the lock, and only then exits, so a settled run
        // reads as `ACTIVE` until it does — and a run driven from another host
        // reads that way for good.
        if channel.pending().is_none() && crate::views::has_settled(&view) {
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
            journal::labels(&paths.run, None),
            journal::payload(&[
                ("author", json!(envelope.author)),
                ("completion", json!(envelope.completion)),
                ("reason", json!(envelope.reason)),
            ]),
        )?;
        if let Some(reason) = &envelope.reason {
            if envelope.completion == Some(true) {
                journal.emit(
                    journal::PipelineKind::CompletionRequested,
                    journal::labels(&paths.run, None),
                    journal::payload(&[("reason", json!(reason))]),
                )?;
            }
        }
        return Ok(Submitted::Answered { reply: id });
    }

    if envelope.version != Some(crate::channel::REPLY_ENVELOPE_VERSION) {
        return Err(Error::Refused(format!(
            "an edit envelope requires version {}",
            crate::channel::REPLY_ENVELOPE_VERSION
        )));
    }

    // What this author may ask for at all, before what this graph will accept:
    // an op outside the allowlist is refused by name and with the reason, and
    // never reaches the durable queue.
    for command in &envelope.commands {
        crate::channel::allows(envelope.author, command)?;
    }

    // Every edit is validated against the graph projected from the journal,
    // through the reconciler's own validator, so the answer is the one the
    // reconciler would give — before anything is queued or sent.
    let mut projected = view.state.graph.clone();
    // The validator this run was launched with, off the launch record rather
    // than out of this process's environment: a `reply` typed in another shell
    // is judged by the rules the run was started under.
    let frontier = Frontier {
        node_validator: view.launch.node_validator().map(str::to_owned),
        ..view.state.frontier()
    };
    // Advanced as it goes, and on a **copy**, because two of the facts an edit is
    // judged against move *within* an envelope: a monitor that parks a node and
    // requeues it in one reply is undoing its own park, and a frontier held still
    // for the whole envelope would tell it the planner had made that park. The
    // copy is what keeps this a check: the applying pass below judges the same
    // commands again, and it has to start from the run's own frontier rather than
    // from one this pass has already walked forward.
    let mut checking = frontier.clone();
    for command in &envelope.commands {
        let operations = edits::compile(&mut projected, &checking, envelope.author, command)?;
        edits::advance(&mut checking, &operations);
    }

    // And, once every command in it has passed both, the envelope as a whole —
    // offered to the reviewer this run was launched with, before anything of it
    // is queued or committed, so a refusal turns the whole envelope away rather
    // than half of it. The graph it is handed is the one the envelope leaves
    // behind, which is what a review of the *edit* rather than of one node is
    // about, and the plan it came from is where the run's goal is stated.
    edits::offer_envelope_to_reviewer(
        view.launch.envelope_reviewer(),
        &envelope.commands,
        &projected,
        view.state.plan.as_ref(),
    )?;

    // Whether a reconciler is running is asked by *taking the run's lock*, which
    // is the same question and the only answer that cannot be raced: with a
    // driver alive the lock is held and the command goes to its durable queue,
    // and with nothing driving the run this process becomes the single writer
    // and applies the edit itself. Execution is continuous, so there is no
    // boundary at which an edit has nothing to apply to.
    match ledger::OwnershipLock::acquire(paths, "reply") {
        Ok(lock) => {
            let mut journal = Journal::open(paths);
            let mut graph = view.state.graph.clone();
            let mut compiled: Vec<edits::Operation> = Vec::new();
            // This pass's own copy, walked forward as each command commits, for
            // the reason the checking pass above holds one.
            let mut frontier = frontier.clone();
            for command in &envelope.commands {
                // Nothing is driving this run, so nothing of it is in flight —
                // the note is still asked of the member the node's last dispatch
                // reported, and the conversation's own account of how it ended is
                // what refuses it.
                let operations = match apply_here(
                    paths,
                    &mut journal,
                    &mut graph,
                    &frontier,
                    envelope.author,
                    command,
                ) {
                    Ok(operations) => operations,
                    Err(error) => {
                        lock.release();
                        return Err(error);
                    }
                };
                edits::advance(&mut frontier, &operations);
                compiled.extend(operations.iter().cloned());
                journal.emit(
                    journal::PipelineKind::EditCommitted,
                    journal::labels(&paths.run, None),
                    journal::payload(&[
                        ("author", json!(envelope.author)),
                        ("command", json!(command)),
                        ("operations", json!(operations)),
                    ]),
                )?;
                engine::record_operation_facts(paths, &mut journal, envelope.author, &operations)?;
                // The planner is told what the monitor did here as well as in
                // the loop: which of the two applied an edit is an accident of
                // whether anything was driving the run, and the planner owns the
                // graph either way.
                if envelope.author == Author::Monitor {
                    if let Some(raised) = engine::monitor_edit(command) {
                        engine::raise(paths, &mut journal, raised)?;
                    }
                }
            }
            lock.release();
            channel.answer_if_verdict(envelope)?;
            Ok(Submitted::AppliedHere {
                operations: compiled,
            })
        }
        Err(Error::Locked { .. }) => {
            let id = channel.submit(envelope.author, &envelope.commands)?;
            let deadline = Instant::now() + Duration::from_secs(reply_timeout_seconds());
            while Instant::now() < deadline {
                if let Some(outcome) = channel.outcome_of(id) {
                    channel.answer_if_verdict(envelope)?;
                    if outcome.applied {
                        return Ok(Submitted::AppliedByRun { reply: id });
                    }
                    return Err(Error::Refused(
                        outcome
                            .reason
                            .unwrap_or_else(|| "the reconciler rejected the edit".into()),
                    ));
                }
                std::thread::sleep(ATTACH_POLL);
            }

            // Accepted and durable, but not reconciled within the timeout: they
            // remain queued, and this is not an instruction to resend. The
            // verdict half does not wait on that — it answers a question rather
            // than the graph, and the reader waiting for it is not the reader
            // waiting for the edits — so it is delivered here as it is on every
            // other path, and only the edits are reported still queued.
            channel.answer_if_verdict(envelope)?;
            Ok(Submitted::Queued { reply: id })
        }
        Err(other) => Err(other),
    }
}

/// Compile one command in the process that is applying it, delivering what only a
/// delivery can answer.
///
/// The reconciler's [`compile_and_deliver`](crate::engine) for the other side of
/// the same fork: when nothing is driving a run, `reply` becomes its single writer
/// and has to do everything the loop would have done — including handing a note to
/// the node's conversation, and recording the refusal when it will never be read.
fn apply_here(
    paths: &RunPaths,
    journal: &mut Journal,
    graph: &mut crate::graph::Graph,
    frontier: &Frontier,
    author: Author,
    command: &Command,
) -> Result<Vec<edits::Operation>> {
    let operations = edits::compile(graph, frontier, author, command)?;
    let Command::Note {
        id,
        addressee,
        text,
        criterion,
    } = command
    else {
        return Ok(operations);
    };
    match engine::deliver_manager_note(paths, *addressee, id, text, criterion.as_ref(), None) {
        Ok(operations) => Ok(operations),
        Err(error) => {
            // The refusal is the run's record as much as the caller's answer: a
            // note that reached nobody is exactly what a manager needs to find in
            // the journal afterwards, and with nothing driving the run this is the
            // only writer that can put it there.
            engine::record_rejection(paths, journal, author, command, &error)?;
            Err(error)
        }
    }
}

fn reply_timeout_seconds() -> u64 {
    std::env::var(crate::channel::REPLY_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(crate::channel::DEFAULT_REPLY_TIMEOUT_SECONDS)
}

/// When this serving session stops of its own accord, or `None` for a session
/// that runs until its member's stream ends.
///
/// Absent is the default and is what
/// [`SERVE_SESSION_ENV`](crate::channel::SERVE_SESSION_ENV) documents. Anything
/// *present* is external input at a trust boundary and is **refused** unless it
/// is a whole number of seconds greater than zero: read as unset instead, a
/// mistyped bound would silently give the session the one behaviour the operator
/// was trying to change, and a zero one would end it before it carried anything.
/// The refusal is made before the first frame is read, so a session that cannot
/// honour its bound never raises a surface it will not stay for.
fn serve_session_deadline() -> Result<Option<Instant>> {
    let key = crate::channel::SERVE_SESSION_ENV;
    let Some(value) = std::env::var_os(key) else {
        return Ok(None);
    };
    let value = value.to_string_lossy().into_owned();
    let seconds = value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| {
            Error::Refused(format!(
                "{key} is a whole number of seconds greater than zero, and this session was \
                 given '{value}'; leave it unset for a session that serves until its member's \
                 frame stream ends"
            ))
        })?;
    // A bound this host's clock cannot name is refused rather than added: the
    // sum is what every later comparison reads, and `Instant` addition panics on
    // an overflow rather than saturating.
    Instant::now()
        .checked_add(Duration::from_secs(seconds))
        .map(Some)
        .ok_or_else(|| {
            Error::Refused(format!(
                "{key} of {seconds} seconds is further ahead than this host's clock can name"
            ))
        })
}

/// Why a serving session stopped, which is the whole of what decides whether
/// what it raised is still owed an answer.
///
/// The discriminator is the **asker**, never the server. Read off the server's
/// own exit instead, a question would be taken out from under a member that is
/// still working and still waiting for it.
enum Served {
    /// The frame stream ended, so the side that was asking ended with it: this
    /// loop was the only reader an answer to any of these had.
    AskerGone,
    /// The member declared its work complete in a verdict this session carried
    /// back to it. Its conversation is over too, so nothing is left to read an
    /// answer to whatever it left outstanding — the same fact as the stream
    /// ending, reached by the member saying so rather than by its stream closing.
    Completed,
    /// This session reached its own bound with the stream still open. The member
    /// is still there, so nothing it raised is withdrawn.
    SessionOver,
}

/// `onepipeline channel serve` — an observer member's judge side.
///
/// The observer emits one JSON frame on stdout whenever it has something to
/// raise; this relays it to the planner as a surface, waits for the answer, and
/// writes the verdict back. A frame is advice: this side never authors an edit
/// on the observer's behalf, and an edit it wants comes through `reply` with its
/// own author, where the allowlist applies.
fn serve(args: &RunArgs) -> Result<i32> {
    let paths = resolve(&args.run)?;
    let channel = ChannelState::new(&paths);
    let stdin = std::io::stdin();
    // Every surface this session raised, so that what it leaves behind can be
    // said out loud rather than left standing as a question with no asker.
    let mut raised: Vec<u64> = Vec::new();
    // Checked between exchanges rather than during one: a bound that cut a frame
    // off mid-answer would leave the member waiting on a verdict this process
    // had already decided not to write. A session blocked on a stream that has
    // gone quiet therefore stays until the next frame or the stream's end, which
    // is the same wait it would be in with no bound at all.
    let session = serve_session_deadline()?;
    let mut ending = Served::AskerGone;

    for line in stdin.lock().lines() {
        // End of input and a broken pipe are not the same fact. Read as one,
        // the observer's judge side exits 0 on a stream that failed mid-frame,
        // so the question it was carrying never reaches the planner and nothing
        // anywhere says why.
        let line = line.map_err(|e| Error::Sibling {
            tool: "oneagentgraph",
            message: format!("the observer's frame stream could not be read: {e}"),
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let frame: ObserverFrame = serde_json::from_str(line.trim())
            .map_err(|e| Error::Refused(format!("the observer emitted a bad frame: {e}")))?;
        // The node is external input like the rest of the frame, and it decides
        // what a *blocking* frame holds back: a name the graph does not carry
        // would pass validation and then hold nothing, so a question raised
        // about work nobody is doing would read as one the run is waiting on.
        // Judged against the graph as it stands, because that is what the
        // subtree is derived from.
        if let Some(node) = &frame.node {
            let graph = RunView::open(&paths)?.state.graph;
            if !graph.contains(node) {
                return Err(Error::Refused(format!(
                    "the observer raised a frame about node '{node}', which run '{}' does not \
                     have; it has: {}",
                    paths.run,
                    graph.ids().cloned().collect::<Vec<_>>().join(", ")
                )));
            }
        }
        let queued = channel.push(Surface {
            id: 0,
            kind: frame.kind,
            message: frame.message,
            source: crate::channel::source::PROPOSAL.to_string(),
            blocking: frame.blocking,
            queued_at: sys::now_millis(),
            abandoned: false,
            workstream: frame.node,
        })?;
        let mut journal = Journal::open(&paths);
        journal.emit(
            journal::PipelineKind::PlannerSurfaceQueued,
            journal::labels(&paths.run, queued.workstream.as_deref()),
            journal::payload(&[
                ("kind", json!(queued.kind)),
                ("message", json!(queued.message)),
                ("source", json!(queued.source)),
                ("blocking", json!(queued.blocking)),
            ]),
        )?;
        raised.push(queued.id);

        // Wait for whichever reader claims the planner's verdict first. A reply
        // reaches exactly one reader, and at a boundary this is it — and a live
        // edit arriving while this waits is not one of them: it carries no
        // ruling, so it goes to the command path and leaves this wait standing
        // rather than ending the member with an envelope it cannot read.
        let answer = wait_for_reply(&channel)?;
        println!(
            "{}",
            serde_json::to_string(&answer).map_err(|e| Error::Invalid(format!("verdict: {e}")))?
        );
        std::io::stdout()
            .flush()
            .map_err(|e| Error::Refused(format!("cannot write the verdict: {e}")))?;
        if answer.completion == Some(true) {
            ending = Served::Completed;
            break;
        }
        if session.is_some_and(|deadline| Instant::now() >= deadline) {
            // This process is done; the member that spawned it is not. Said on
            // stderr because an operator reading a run whose server went needs
            // to know the questions are still standing rather than lost — and
            // because it is the one visible difference between the two endings.
            eprintln!(
                "onepipeline: this channel session reached its {} bound with the observer's \
                 stream still open; the {} surface(s) it raised stay in the queue, still \
                 waiting for an answer",
                crate::channel::SERVE_SESSION_ENV,
                raised.len()
            );
            ending = Served::SessionOver;
            break;
        }
    }
    // Which ending this was decides it, and the question each is answering is
    // whether the side that asked is still there. A stream that ended and a
    // member that declared itself complete both say it is not: this loop was the
    // only reader an answer to anything it raised had, so what it leaves behind
    // is marked — rather than deleted, which [`ChannelState::abandon`] says why —
    // so the run stops reporting a decision nobody is waiting on while the text
    // stays where a manager can still read it. A session that reached its own
    // bound says nothing of the kind: the member is still holding the stream open
    // and still owed every answer, so nothing is withdrawn and everything stays
    // counted.
    //
    // Deliberately not reached by the `?` paths above either: a refused frame is
    // this server rejecting what the member said, and the member is still there
    // to be told. Only a stream that ended proves the asker did.
    if matches!(ending, Served::AskerGone | Served::Completed) {
        channel.abandon(&raised)?;
    }
    Ok(EXIT_SUCCESS)
}

/// What an observer emits when it has something to raise.
///
/// External input, so it has a schema: an unknown key or a missing `kind` or
/// `message` is refused by name rather than defaulted into a surface the
/// planner then has to interpret.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserverFrame {
    /// What the surface is asking about, in the observer persona's own
    /// vocabulary.
    kind: String,
    /// Its text.
    message: String,
    /// Whether the run should wait on the answer. A frame that says nothing is
    /// blocking, because an observer that stopped to ask is waiting; a monitor
    /// reporting what it saw says `false` and holds nothing back.
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
        if let Some(claimed) = channel.claim_reply()? {
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

// llmlint: ignore-block[cli_output_contract] a refused run root is part of the answer these
// views were asked for, not a failure of the command, so it goes to stdout and the exit
// code stays 0: failing `runs` because one stray directory sits beside the runs would break
// every wrapper over it. A caller that named *one* run and could not have it is a different
// case, and `resolve` and `RunView::open` still refuse it outright.
/// `onepipeline runs`.
fn runs(args: &RunsArgs) -> Result<i32> {
    print!(
        "{}",
        views::runs(&ledger::runs_root(), args.mine, &sys::launching_session())
    );
    Ok(EXIT_SUCCESS)
}

/// A view that covers one run, or every run when given none.
fn report(args: &OptionalRunArgs, render: fn(&views::Survey) -> String) -> Result<i32> {
    let survey = match &args.run {
        Some(run) => views::Survey::of_one(RunView::open(&resolve(run)?)?),
        None => views::Survey::of(&ledger::runs_root()),
    };
    print!("{}", render(&survey));
    Ok(EXIT_SUCCESS)
}
// llmlint: ignore-end[cli_output_contract]

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
    let survey = match &args.run {
        Some(run) => views::Survey::of_one(RunView::open(&resolve(run)?)?),
        None => views::Survey::of(&ledger::runs_root()),
    };
    for view in &survey.views {
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
        "observe",
        "Observe",
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
        let project = "plans:release";
        assert_eq!(
            mint_run_id(&plan(Some("tracked-release")), project, &root),
            "tracked-release"
        );

        std::fs::create_dir_all(root.join("tracked-release")).expect("an existing run");
        assert_eq!(
            mint_run_id(&plan(Some("tracked-release")), project, &root),
            "tracked-release-2"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A project states its plan's name as its own title where the reserved key
    /// says nothing, so a nameless plan is rare — but a store may hold one, and
    /// what names the run then is the project a person launched.
    #[test]
    fn a_nameless_plan_takes_its_run_id_from_the_project_it_was_launched_by() {
        let root = scratch("mint-project");
        assert_eq!(mint_run_id(&plan(None), "release", &root), "release");
        // A run id names a directory, so anything that could not be one becomes
        // a character that can.
        assert_eq!(mint_run_id(&plan(None), "odd name!", &root), "odd-name-");
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

    /// The prose that told a launched member to drive, which no member is asked
    /// to do any more.
    #[test]
    fn the_launched_graphs_task_never_asks_a_member_to_drive_the_engine() {
        let task = run_description("tracked-release", Some("close the coverage gap"));
        for verb in ["round run", "round next", "drive-run"] {
            assert!(
                !task.contains(verb),
                "the observer's task names {verb}: {task}"
            );
        }
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

    /// A launch record naming a driver, as `stop` reads one: the pid, the host
    /// it means something on, and the stamp that says it is still that process.
    fn launched_by(pid: u32, host: &str, started: &str) -> LaunchRecord {
        LaunchRecord {
            run_id: "stopped".into(),
            project: "plans:demo".into(),
            dir: PathBuf::from("/tmp/launch"),
            graph: String::new(),
            graph_run: String::new(),
            observer_runs: Vec::new(),
            observer_ending: String::new(),
            node_graph: "graphs/node-scope.yaml".into(),
            pr_author_graph: String::new(),
            node_validator: String::new(),
            envelope_reviewer: String::new(),
            launcher: "e2e".into(),
            session: "session-a".into(),
            pid,
            host: host.to_string(),
            started: started.to_string(),
            started_at: sys::now_rfc3339(),
            heartbeat_interval: 1_800,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
            adoptions: 0,
            filters: crate::filter::Filters::default(),
        }
    }

    /// What a stop aims at is every claim the run holds and can prove — and
    /// never a pid nothing proves, the launch record's included.
    ///
    /// Three records, three questions. The launch record's answer is about the
    /// past: a driver that died leaves its pid there, and a stop that aimed at it
    /// alone signalled nothing while the run's dispatches kept running. The lock
    /// is the claim made now. The registry is the only one that names the work
    /// rather than a driver, which is what a stop is actually for.
    ///
    /// Each of the three is aimed at only where its own start token says its pid
    /// is still the process the record named, because a teardown aimed at a pid
    /// the host has since reissued is a teardown of somebody else's work.
    #[test]
    fn a_stop_aims_at_every_stamped_claim_the_run_holds_and_never_at_a_pid_nothing_proves() {
        let root = scratch("roots");
        let paths = RunPaths::under(&root, "stopped");
        paths.create().expect("the run directory");
        let here = sys::hostname();
        let dead = sys::reaped_pid();
        let stamp = |started: &str| ledger::LockRecord {
            pid: sys::pid(),
            host: here.clone(),
            acquired_at: sys::now_rfc3339(),
            verb: "drive".into(),
            started: started.to_string(),
        };
        let held = |record: &ledger::LockRecord| {
            ledger::write_json(&paths.lock(), record).expect("a held lock");
        };
        let proven = sys::process_start_token(sys::pid())
            .expect("this host says when a process started")
            .recorded()
            .to_string();
        let aimed_at =
            |record: &LaunchRecord| roots_to_stop(&paths, record).expect("this run's records read");
        let roots = |record: &LaunchRecord| match aimed_at(record) {
            Aim::Here { roots, .. } => roots,
            Aim::Elsewhere => panic!("a run this host's own records name read as another host's"),
        };

        // Nothing holds the lock, and the record names a driver that has died:
        // the run is this host's and there is nothing here to aim at.
        assert_eq!(
            aimed_at(&launched_by(dead, &here, &proven)),
            Aim::Here {
                roots: Vec::new(),
                unproven: Vec::new(),
                declined: Vec::new(),
            }
        );
        // The same record naming this live process, which its stamp proves.
        assert_eq!(
            roots(&launched_by(sys::pid(), &here, &proven)),
            vec![sys::pid()]
        );
        // And naming a live pid stamped as a process it is not, which is what a
        // host that has reissued that pid leaves behind: the process the record
        // was written for is over, this one is a stranger's, and a teardown
        // aimed there would end work this run never started. Nothing unresolved
        // either — the stamp *answered*, and its answer was no.
        assert_eq!(
            aimed_at(&launched_by(
                sys::pid(),
                &here,
                "the driver it named, which is not this process",
            )),
            Aim::Here {
                roots: Vec::new(),
                unproven: Vec::new(),
                declined: vec![sys::pid()],
            },
            "a stop aimed at a pid the host has since given to another process"
        );
        // A record from a build that predates the stamp proves nothing about its
        // pid either way, so it is not aimed at — and, unlike the two above, the
        // stop may not call that pid gone.
        assert_eq!(
            aimed_at(&launched_by(sys::pid(), &here, "")),
            Aim::Here {
                roots: Vec::new(),
                unproven: vec![sys::pid()],
                declined: Vec::new(),
            }
        );

        // A lock this build's own stamp proves.
        held(&stamp(&proven));
        assert_eq!(
            roots(&launched_by(dead, &here, &proven)),
            vec![sys::pid()],
            "a stop did not aim at the process the lock stamps as driving the run"
        );
        // The same live pid, stamped as a process it is not. This is the case
        // that makes the stamp worth reading: the pid answers a liveness probe,
        // and it is not the driver.
        held(&stamp("the process that took it, which is not this one"));
        assert!(
            roots(&launched_by(dead, &here, &proven)).is_empty(),
            "a stop aimed at a pid the lock's own stamp disowns"
        );
        // A lock an older build wrote carries no stamp, and an unproven pid is
        // not one a teardown may aim at either.
        held(&stamp(""));
        assert_eq!(
            aimed_at(&launched_by(dead, &here, &proven)),
            Aim::Here {
                roots: Vec::new(),
                unproven: vec![sys::pid()],
                declined: Vec::new(),
            }
        );
        // A lock taken on another machine, where a pid means nothing.
        held(&ledger::LockRecord {
            host: "a-host-this-is-not".into(),
            ..stamp(&proven)
        });
        assert!(roots(&launched_by(dead, &here, &proven)).is_empty());
        // A lock this build cannot read names nobody, so it adds no root. Unlike
        // the registry below it is not fatal: the lock narrows what a teardown
        // reaches, while the registry decides whether anything is known about the
        // work at all.
        std::fs::write(paths.lock(), "not json at all").expect("a lock nobody can read");
        assert!(roots(&launched_by(dead, &here, &proven)).is_empty());

        // And a run whose every record is another host's leaves this one nothing
        // to aim at and nothing to promise, which is what `stop` reports as
        // `elsewhere` rather than as a run whose work is over.
        assert_eq!(
            aimed_at(&launched_by(dead, "a-host-this-is-not", &proven)),
            Aim::Elsewhere
        );

        // The registry, which names the work rather than a driver. Its entries
        // are proved one at a time and on their own stamps, so a dispatch is
        // aimed at whether or not anything is still driving the run — which is
        // the case it exists for, because the driver is dead.
        std::fs::remove_file(paths.lock()).expect("the lock is given up");
        let running = |pid: u32, host: &str, started: &str| ledger::DispatchRecord {
            node: "build".into(),
            pid,
            host: host.to_string(),
            dispatched_at: sys::now_rfc3339(),
            started: started.to_string(),
        };
        let recorded = |record: &ledger::DispatchRecord| {
            ledger::write_json(&paths.dispatch(record.pid, 0), record)
                .expect("a recorded dispatch");
        };
        recorded(&running(sys::pid(), &here, &proven));
        assert_eq!(
            roots(&launched_by(dead, &here, &proven)),
            vec![sys::pid()],
            "a stop did not aim at the process the registry says the run's work is in"
        );
        // The same live pid, recorded as a process it is not, and one recorded on
        // another machine. Neither proves a pid on this host, and a teardown aims
        // at neither — and neither is a registry this build cannot read, so the
        // stop still goes ahead with what it can prove.
        for disowned in [
            running(
                sys::pid(),
                &here,
                "the process that took it, which is not this one",
            ),
            running(sys::pid(), "a-host-this-is-not", &proven),
        ] {
            recorded(&disowned);
            assert!(
                roots(&launched_by(dead, &here, &proven)).is_empty(),
                "a stop aimed at a pid the registry cannot prove: {disowned:?}"
            );
        }
        // One process named twice is one root: the driver that took the lock is
        // usually the one the record names.
        recorded(&running(sys::pid(), &here, &proven));
        held(&stamp(&proven));
        assert_eq!(
            roots(&launched_by(sys::pid(), &here, &proven)),
            vec![sys::pid()]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A registry this build cannot read stops the stop, rather than narrowing
    /// it.
    ///
    /// The distinction the whole boundary rests on. Every other record a stop
    /// consults can only *add* a root, so one it cannot read costs reach and
    /// nothing else. The registry is what says whether a run has work running at
    /// all, so a reader that met an entry it could not parse and carried on would
    /// be reporting "there was nothing to stop" about a run it never managed to
    /// ask — which is the false completion, one layer further in.
    ///
    /// An entry carrying a field this build does not know is not one of those.
    /// It is another build of this crate saying a process is running, and the
    /// stop **aims at it**: refusing the registry over the key would have taken
    /// the reach away, and dropping the entry would have left the dispatch
    /// running under a run reported stopped.
    #[test]
    fn a_registry_this_build_cannot_read_refuses_the_stop_rather_than_narrowing_it() {
        let root = scratch("roots-unreadable");
        let paths = RunPaths::under(&root, "stopped");
        paths.create().expect("the run directory");
        let here = sys::hostname();
        let launch = launched_by(sys::reaped_pid(), &here, "a driver that has since died");
        let usable = ledger::DispatchRecord {
            node: "build".into(),
            pid: sys::pid(),
            host: here.clone(),
            dispatched_at: sys::now_rfc3339(),
            started: sys::process_start_token(sys::pid())
                .expect("this host says when a process started")
                .recorded()
                .to_string(),
        };
        // A registry that is there and empty is an answer: this run has nothing
        // running, and the stop proceeds on the records that remain.
        assert!(roots_to_stop(&paths, &launch).is_ok());

        std::fs::write(
            paths.dispatch(usable.pid, 0),
            serde_json::to_string(&serde_json::json!({
                "node": "build",
                "pid": sys::pid(),
                "host": here,
                "dispatched_at": sys::now_rfc3339(),
                "started": usable.started,
                "reaped_by": "a build that came later",
            }))
            .expect("an entry from a newer writer"),
        )
        .expect("an entry");
        assert_eq!(
            roots_to_stop(&paths, &launch).expect("an entry from a newer writer reads"),
            Aim::Here {
                roots: vec![usable.pid],
                unproven: Vec::new(),
                declined: Vec::new(),
            },
            "a dispatch recorded with a field this build does not know was left running"
        );

        for (what, entry) in [
            (
                "a record that is not JSON at all",
                "not an entry".to_string(),
            ),
            (
                "a record whose stamp proves nothing",
                serde_json::to_string(&ledger::DispatchRecord {
                    started: String::new(),
                    ..usable.clone()
                })
                .expect("an unstamped entry"),
            ),
        ] {
            std::fs::write(paths.dispatch(usable.pid, 0), entry).expect("an entry");
            let refused = roots_to_stop(&paths, &launch)
                .expect_err(&format!("{what} was read as a registry to act on"));
            assert!(
                refused.to_string().contains(&usable.pid.to_string()),
                "the refusal does not name the entry that caused it: {refused}"
            );
        }

        // And a registry that is not there at all, which every run this build
        // creates has: its absence is something having taken it away, and what
        // the run is running cannot be established without it.
        std::fs::remove_dir_all(paths.dispatches()).expect("the registry is taken away");
        let refused = roots_to_stop(&paths, &launch)
            .expect_err("a run with no registry at all was read as a run with nothing running");
        assert!(
            refused
                .to_string()
                .contains(&paths.dispatches().display().to_string()),
            "the refusal does not name the registry it could not read: {refused}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
