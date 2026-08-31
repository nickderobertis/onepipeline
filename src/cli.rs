//! The command-line argument surface.
//!
//! Exactly the commands, positionals, and flags `docs/contract.md` lists —
//! parsing only. Nothing here starts, adopts, replies to, attests, stops, or
//! reports on a run; the binary parses one of these and refuses.

// llmlint: ignore-file[invalid_states_unrepresentable] every identifier here is the
// argument `docs/contract.md` spells, and this is the parsing layer only. A `RunId` or
// `NodeRef` newtype would be a public item the contract does not name, and parsing a run
// id or a `run:<id>#<node>` reference into one is the implementation the interface-only
// stage forbids (see AGENTS.md).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::channel::SurfaceKind;

/// What `--dag-graph` means when it names no graph: no agent graph is launched
/// at all, and deterministic code alone drives the run.
pub const DAG_GRAPH_OFF: &str = "off";

/// The planner-update pacemaker interval, in seconds, when `start` is given
/// none.
pub const DEFAULT_HEARTBEAT_INTERVAL_SECONDS: u64 = 1_800;

/// Execute a task DAG over oneagentgraph and onevcs, merging their event
/// streams into one.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "onepipeline", version, about, long_about = None)]
pub struct Cli {
    /// The command to run.
    #[command(subcommand)]
    pub command: Command,
}

/// The top-level commands.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum Command {
    /// Execute a plan: drive its DAG continuously to settlement.
    Start(StartArgs),
    /// Read a plan without launching it.
    #[command(subcommand)]
    Plan(PlanCommand),
    /// Attach a fresh driver to a run whose ledger is intact.
    Adopt(AdoptArgs),
    /// The channel's server side.
    #[command(subcommand)]
    Channel(ChannelCommand),
    /// Read the next planner surface.
    Next(ReadArgs),
    /// Reply to a surface, with a verdict, graph edits, or both.
    Reply(ReplyArgs),
    /// Raise a surface to the planner.
    Surface(SurfaceArgs),
    /// Complete a ready, waiting human action.
    Attest(AttestArgs),
    /// End a run and its whole dispatch tree.
    Stop(StopArgs),
    /// List recorded runs.
    Runs(RunsArgs),
    /// A run's live state: what is driving it, and what is running.
    Status(OptionalRunArgs),
    /// Every live dispatch on this host, with its owner and load contribution.
    Host,
    /// Stream a run's merged events.
    Monitor(ReadArgs),
    /// Per-node outcomes, with each node's own evidence.
    Results(RunArgs),
    /// What each run is for, and how far it has got.
    Goals(OptionalRunArgs),
    /// A dispatched turn's tools and reasoning, from the evidence it retained.
    Transcript(TranscriptArgs),
    /// Session timing and usage.
    Telemetry(TelemetryArgs),
    /// Drive one run's engine loop in this process.
    ///
    /// Not part of the documented surface and hidden from `--help`: it is the
    /// process `start --detach` retains, because the loop that drives a run
    /// cannot outlive a launcher that is about to exit. Nothing but this
    /// crate's own launcher spells it.
    #[command(hide = true, name = crate::engine::DRIVE_VERB)]
    DriveRun(DriveRunArgs),
    /// Drive one agent graph in this process, relaying its envelopes as NDJSON.
    ///
    /// Not part of the documented surface and hidden from `--help`: it is how
    /// `start --detach` retains a driver that composes the **same**
    /// `oneagentgraph` an attached launch validates and runs with. Nothing but
    /// this crate's own launcher spells it, and it names no run — it is a graph
    /// and a task, exactly as the sibling's own `run` takes them.
    #[command(hide = true, name = crate::agentgraph::DRIVE_VERB)]
    Drive(DriveArgs),
}

/// What a plan may be asked, short of running it.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum PlanCommand {
    /// Run the engine's own plan loader, and every registered check, over one
    /// project.
    Check(PlanCheckArgs),
}

/// `onepipeline plan check`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct PlanCheckArgs {
    /// The qualified onetaskgraph project id the plan is read from,
    /// `<source>:<native>`, exactly as `start` takes it.
    pub project: String,
    /// One executable to offer the loaded plan to, repeatable and run in the
    /// order the flags are given.
    ///
    /// Resolved against the working directory this command was run from. The
    /// plan crosses its stdin as one JSON document, with
    /// `ONEPIPELINE_PLAN_CHECK_SCHEMA=1` in its environment; it answers on
    /// stdout with `{"refusals": [...]}` and exit 0. Naming none runs the
    /// loader alone.
    #[arg(long = "check", value_name = "PATH")]
    pub checks: Vec<PathBuf>,
    /// Print one JSON object rather than a line per refusal.
    #[arg(long)]
    pub json: bool,
}

/// `onepipeline start`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct StartArgs {
    /// The qualified onetaskgraph project id the plan is read from,
    /// `<source>:<native>`.
    ///
    /// Qualified by **source**, so a `local-md` project is launchable directly
    /// with no copy into a remote system first, and nothing special-cases a
    /// remote source.
    pub project: String,
    /// Stay attached, streaming the run's events and returning when it settles.
    /// The default.
    #[arg(long, conflicts_with = "detach")]
    pub attach: bool,
    /// Print the launch record and return, leaving the run unattended.
    #[arg(long)]
    pub detach: bool,
    /// The dag-scope agent graph to attach as an observer, or `off` for none.
    ///
    /// `off` is the shipped default: no agent is required to run a plan. A
    /// graph named here observes the run and authors channel surfaces; it never
    /// drives the engine.
    #[arg(long, value_name = "REF", default_value = DAG_GRAPH_OFF)]
    pub dag_graph: String,
    /// The agent graph a lifecycle node's change request body is drafted by.
    ///
    /// Naming none is the shipped default, exactly as `--dag-graph` defaults to
    /// `off`: this crate ships the flag and not the document, and a launch that
    /// names no graph opens its change requests with the body its plan states,
    /// or with none. Given here it overrides the launch config's own field.
    #[arg(long, value_name = "REF")]
    pub pr_author_graph: Option<String>,
    /// The command every op that introduces or changes a node's task is offered
    /// to before it is applied.
    ///
    /// The node crosses as JSON on its stdin; exit 0 accepts the edit and a
    /// non-zero exit refuses it, with the command's own stderr as the reason.
    /// Naming none is the shipped default and is exactly what a launch did
    /// before this flag existed. Given here it beats `ONEPIPELINE_NODE_VALIDATOR`
    /// and the launch config's own field — including when what it names is
    /// blank, which is this launch saying it has none rather than a fall-through
    /// to the rung below.
    #[arg(long, value_name = "COMMAND")]
    pub node_validator: Option<String>,
    /// The command every reply envelope carrying edits is offered to whole,
    /// after every one of its commands has passed this crate's own validation
    /// and the node validator above.
    ///
    /// One document crosses its stdin: every node the envelope introduces or
    /// changes with the op that produced each, the plan they are being edited
    /// into, and the run's goal. Exit 0 accepts the envelope and a non-zero exit
    /// refuses it whole, with the command's own stderr as the reason and the
    /// node it declared on an `objection: ID` line of that stderr named as the
    /// one it objected to. Naming
    /// none is the shipped default and is exactly what a launch did before this
    /// flag existed. Given here it beats `ONEPIPELINE_ENVELOPE_REVIEWER` and the
    /// launch config's own field — including when what it names is blank, which
    /// is this launch saying it has none rather than a fall-through to the rung
    /// below.
    #[arg(long, value_name = "COMMAND")]
    pub envelope_reviewer: Option<String>,
    /// How often the durable planner-update pacemaker comes due, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = DEFAULT_HEARTBEAT_INTERVAL_SECONDS)]
    pub heartbeat_interval: u64,
    /// Override one dag-scope graph config field. Passed opaquely to
    /// `oneagentgraph run`, in command-line order.
    #[arg(long = "set", value_name = "PATH=VALUE")]
    pub dag_sets: Vec<String>,
    /// Override one node-scope graph config field. Passed opaquely to every
    /// node's `oneagentgraph run`, in command-line order.
    #[arg(long = "node-set", value_name = "PATH=VALUE")]
    pub node_sets: Vec<String>,
    /// Proceed even when another live session holds a targeted repository.
    #[arg(long)]
    pub acknowledge_concurrent: bool,
    /// The launch config: what this launch declares about its run, as one
    /// document. Each flag below overrides the part of it that it names.
    #[arg(long, value_name = "FILE")]
    pub launch_config: Option<PathBuf>,
    /// Keep only the events a filter admits out of every `oneagentgraph` launch
    /// this run starts, as a file path or inline JSON.
    #[arg(long, value_name = "SPEC")]
    pub filter_agentgraph: Option<String>,
    /// Keep only the events a filter admits out of every `onevcs` session this
    /// run follows, as a file path or inline JSON.
    #[arg(long, value_name = "SPEC")]
    pub filter_vcs: Option<String>,
    /// Define or override one named read-time profile, as `NAME=SPEC`.
    /// Repeatable. `planner` and `monitor` ship and are overridden by name.
    #[arg(long = "filter-profile", value_name = "NAME=SPEC")]
    pub filter_profiles: Vec<String>,
}

/// `onepipeline adopt`.
///
/// The same attach/detach pair [`StartArgs`] has, with the same default and the
/// same meaning: attached, this process drives the run it took over; detached,
/// the driver it retains does, and the launcher returns once that driver has
/// claimed the run.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct AdoptArgs {
    /// The run id.
    pub run: String,
    /// Stay attached, driving the adopted run and returning when it settles.
    /// The default.
    #[arg(long, conflicts_with = "detach")]
    pub attach: bool,
    /// Print the launch record and return, leaving the fresh driver unattended.
    #[arg(long)]
    pub detach: bool,
}

/// The flag that tells a retained driver it is taking a run over rather than
/// driving one nothing has driven yet.
///
/// Named here, beside the argument it parses, because the launcher spells it on
/// a command line: a spelling only one side of that knew could drift.
pub(crate) const ADOPT_FLAG: &str = "adopt";

/// `onepipeline drive-run` — the retained driver of a detached launch.
///
/// The run it drives, and whether it is **adopting** it: the bookkeeping an
/// adoption does belongs under the ownership lock, and this is the process that
/// takes that lock, so a detaching adoption hands the work here rather than
/// doing it on behalf of a driver that does not exist yet.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct DriveRunArgs {
    /// The run id.
    pub run: String,
    /// Take the run over from the driver that had it, recording the adoption
    /// under the lock this process is the one to hold.
    #[arg(long = ADOPT_FLAG)]
    pub adopt: bool,
}

/// A read verb that shapes its event view through a filter profile.
///
/// Naming neither reads through the shipped [`DEFAULT_PROFILE`] — the planner's
/// view, which is what these two verbs are for.
///
/// [`DEFAULT_PROFILE`]: crate::filter::DEFAULT_PROFILE
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ReadArgs {
    /// The run id.
    pub run: String,
    /// The profile to read through: a name this run has, or a filter spec as a
    /// file path or inline JSON.
    #[arg(long, value_name = "NAME|SPEC", conflicts_with = "all")]
    pub filter: Option<String>,
    /// Read every event in the store, through no profile at all.
    #[arg(long)]
    pub all: bool,
}

/// `onepipeline drive` — the retained driver a detached launch starts.
///
/// The arguments a graph run needs and no more, spelled as `oneagentgraph run`
/// spells them: this is the same launch, made by this build's own copy of that
/// library rather than by whichever one the host has installed.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct DriveArgs {
    /// The agent-graph config to run.
    pub graph: String,
    /// The task prose every member without its own is given.
    #[arg(long, value_name = "TEXT")]
    pub task: String,
    /// The directory the graph's members work in.
    #[arg(long, value_name = "DIR")]
    pub dir: PathBuf,
    /// One `k=v` label stamped on every envelope, repeatable.
    #[arg(long = "label", value_name = "KEY=VALUE")]
    pub labels: Vec<String>,
    /// One opaque graph-config override, repeatable, applied in order.
    #[arg(long = "set", value_name = "PATH=VALUE")]
    pub sets: Vec<String>,
    /// The source filter this launch relays through, inline as JSON. Spelled as
    /// `oneagentgraph run` spells it, because an overridden binary is what
    /// receives it.
    #[arg(long, value_name = "SPEC")]
    pub event_filter: Option<String>,
}

/// The channel's server side.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum ChannelCommand {
    /// Serve the channel as an observer member's judge-side command provider.
    Serve(RunArgs),
}

/// A command that names one run and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct RunArgs {
    /// The run id.
    pub run: String,
}

/// A view that defaults to every run when given none.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct OptionalRunArgs {
    /// The run id. Omitted, the view covers every run.
    pub run: Option<String>,
}

/// `onepipeline reply`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ReplyArgs {
    /// The run id.
    pub run: String,
    /// The reply envelope. Omitted, it is read from stdin.
    pub file: Option<PathBuf>,
}

/// `onepipeline surface`.
///
/// The message body arrives the way [`ReplyArgs`]'s envelope does — from a file,
/// or from stdin when none is named — so agent-authored prose never has to pass
/// through a shell. Divergence 38 records why. `--message` still works, and is
/// refused beside a file.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct SurfaceArgs {
    /// The run id.
    pub run: String,
    /// The file the surface's text is read from. Omitted, it is read from
    /// stdin — unless `--message` carried it.
    #[arg(conflicts_with = "message")]
    pub file: Option<PathBuf>,
    /// What the surface is asking about.
    #[arg(long, value_enum)]
    pub kind: SurfaceKind,
    /// The surface's text, inline. Prefer the file or the stdin form: whatever
    /// is written here is read by a shell first.
    #[arg(long, value_name = "TEXT")]
    pub message: Option<String>,
}

/// `onepipeline attest`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct AttestArgs {
    /// The run id.
    pub run: String,
    /// The human action's reference.
    pub reference: String,
}

/// `onepipeline stop`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct StopArgs {
    /// The run id.
    pub run: String,
    /// Stop a run this session does not own. The owner is named either way.
    #[arg(long)]
    pub force: bool,
}

/// `onepipeline runs`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct RunsArgs {
    /// List only the runs this session launched.
    #[arg(long)]
    pub mine: bool,
}

/// `onepipeline transcript`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TranscriptArgs {
    /// The run id.
    pub run: String,
    /// The node whose transcript to read. Omitted, every node that dispatched.
    pub node: Option<String>,
}

/// `onepipeline telemetry`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TelemetryArgs {
    /// The run id. Omitted, the view covers every run.
    pub run: Option<String>,
    /// Break the wall clock down into buckets that sum exactly.
    #[arg(long)]
    pub breakdown: bool,
}
