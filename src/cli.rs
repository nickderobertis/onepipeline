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
    /// Read a plan and report whether `start` would accept it.
    Validate(ValidateArgs),
    /// Attach a fresh driver to a run whose ledger is intact.
    Adopt(RunArgs),
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
    DriveRun(RunArgs),
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

/// `onepipeline start`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct StartArgs {
    /// The plan file.
    pub plan: PathBuf,
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

/// `onepipeline validate`.
///
/// The operand `start` takes and nothing beside it: this verb is that launch's
/// own validation asked as a question, so a flag here would be a way to be
/// refused differently from the launch it stands for.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ValidateArgs {
    /// The plan file.
    pub plan: PathBuf,
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
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct SurfaceArgs {
    /// The run id.
    pub run: String,
    /// What the surface is asking about.
    #[arg(long, value_enum)]
    pub kind: SurfaceKind,
    /// The surface's text.
    #[arg(long, value_name = "TEXT")]
    pub message: String,
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
