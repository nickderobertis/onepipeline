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

/// The round budget, in seconds, when `start` is given none.
pub const DEFAULT_ROUND_BUDGET_SECONDS: u64 = 14_400;

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
    /// Launch a plan's dag-scope agent graph and drive it.
    Start(StartArgs),
    /// Attach a fresh driver to a run whose ledger is intact.
    Adopt(RunArgs),
    /// The engine verbs the orchestrator member drives.
    #[command(subcommand)]
    Round(RoundCommand),
    /// The channel's server side.
    #[command(subcommand)]
    Channel(ChannelCommand),
    /// Read the next planner surface.
    Next(RunArgs),
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
    Monitor(RunArgs),
    /// Per-node outcomes, with each node's own evidence.
    Results(RunArgs),
    /// What each run is for, and how far it has got.
    Goals(OptionalRunArgs),
    /// A dispatched turn's tools and reasoning, from the evidence it retained.
    Transcript(TranscriptArgs),
    /// Session timing and usage.
    Telemetry(TelemetryArgs),
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
    /// How long one round may run, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = DEFAULT_ROUND_BUDGET_SECONDS)]
    pub round_budget: u64,
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
}

/// The engine verbs, guarded by the run ownership lock: single writer.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum RoundCommand {
    /// Execute the current round.
    Run(RunArgs),
    /// Transition to the next round.
    Next(RunArgs),
}

/// The channel's server side.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum ChannelCommand {
    /// Serve the channel as the orchestrator member's judge-side command
    /// provider.
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
