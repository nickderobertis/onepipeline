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

use std::num::NonZeroU64;

use clap::{Args, Parser, Subcommand};

use crate::channel::SurfaceKind;

/// What `--dag-graph` means when it names no graph: no agent graph is launched
/// at all, and deterministic code alone drives the run.
pub const DAG_GRAPH_OFF: &str = "off";

/// The planner-update check-in interval, in seconds, when `start` is given
/// none.
pub const DEFAULT_HEARTBEAT_INTERVAL_SECONDS: u64 = 1_800;

/// The write-back's per-item budget, in seconds, when a launch names none.
///
/// The bottom rung of four: `--writeback-item-budget` beats
/// [`WRITEBACK_ITEM_BUDGET_ENV`], which beats the launch config's own
/// `writeback_item_budget`, and this is what every launch ran under before any
/// of the three existed. Multiplied by the number of items a settlement projects
/// to bound the store's `project copy`, and never below
/// [`WRITEBACK_COMMAND_FLOOR_SECONDS`]. A [`NonZeroU64`] because zero is no
/// budget at all, and every rung that names one refuses it.
pub const DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS: NonZeroU64 = NonZeroU64::new(10).unwrap();

/// The least any store command the write-back spawns is allowed, in seconds.
///
/// The whole deadline for the reads that are not linear in plan size — the
/// project and each page of its tasks — and the floor under the copy's, which
/// the per-item budget lifts once a plan carries enough items to need it. A
/// liveness backstop for an unreachable store rather than a latency target:
/// the projection stays off the reconcile loop while the child runs.
pub const WRITEBACK_COMMAND_FLOOR_SECONDS: u64 = 60;

/// The store commands one write-back attempt runs, and whose failures it reads the store's
/// own class off: the project, each page of its tasks, and the copy.
///
/// Any one of them answering [`WRITEBACK_REFUSED_CLASS`] makes the whole attempt refused,
/// because a projection needs all three.
pub const WRITEBACK_CLASSIFIED_COMMANDS: [&str; 3] = ["project-show", "task-list", "project-copy"];

/// The member of a store's failure document the write-back branches on, as a path.
pub const WRITEBACK_FAILURE_CLASS_MEMBER: &str = "failure.class";

/// The member of each entry of a store's partial answer the write-back reads, as a path.
pub const WRITEBACK_PARTIAL_CLASS_MEMBER: &str = "errors[].class";

/// The class that stops the write-back's retry timer: a failure asking again cannot change.
///
/// A projection refused under it is reported once and attempted again only when a snapshot
/// different from the refused one is published. Every other failure — another class, or no
/// class at all — keeps the growing retry schedule.
pub const WRITEBACK_REFUSED_CLASS: &str = "refused";

/// The exit status a store command writes its failure document under.
pub const WRITEBACK_FAILURE_EXIT: i32 = 1;

/// The exit status a store command writes a partial answer under. Refused only where every
/// entry of its `errors` carries [`WRITEBACK_REFUSED_CLASS`].
pub const WRITEBACK_PARTIAL_EXIT: i32 = 4;

/// The file, in a run's directory, each write-back projection attempt is appended to as one
/// JSON object per line and never rewritten. Its shape is
/// [`ProjectionRecord`](crate::views::ProjectionRecord).
pub const WRITEBACK_PROJECTIONS_FILE: &str = "writeback-projections.jsonl";

/// The schema version every line of [`WRITEBACK_PROJECTIONS_FILE`] is written at.
///
/// Version 2 added `delivered`; version 3 added `actions.reopened`, which every landed attempt
/// at that version names. A line naming no version is version 1, the shape before either, and
/// still reads, as does a version 2 line; a version 1 line naming `delivered`, a version 1 or 2
/// line naming `actions.reopened`, and any version this build has never written, are refused.
pub const WRITEBACK_PROJECTIONS_SCHEMA_VERSION: u32 = 3;

/// The file, in a run's directory, holding whether the store offers a member copy — decided
/// once per run, before its first projection, and read by every later driver of the run.
pub const WRITEBACK_STORE_FILE: &str = "writeback-store.json";

/// The first `onetaskgraph` release whose `project copy` takes `--member` and whose copy
/// report carries `spent`. A store reporting an older version is projected whole.
pub const WRITEBACK_MEMBERS_FROM: &str = "0.2.30";

/// The first `onetaskgraph` release whose status vocabulary carries `queued` and whose tasks
/// carry `delivers`, re-evaluating each delivered task on every write of its deliverer.
///
/// Against a store reporting an older version, a node the run has not started is written
/// `todo`, the shadow task carries no `delivers`, and a plan whose tasks deliver tickets is
/// told once why they are not moved.
pub const WRITEBACK_DELIVERS_FROM: &str = "0.2.32";

/// The store command a member projection reads one named member's destination item with,
/// by the name its capture files and refusals carry. It stands in for
/// [`WRITEBACK_CLASSIFIED_COMMANDS`]' page of tasks, which a member projection never reads,
/// and its failures are classified by the same rule.
pub const WRITEBACK_MEMBER_READ: &str = "task-show";

/// The environment variable naming the write-back's per-item budget, in seconds.
///
/// The middle rung of the three spellings, exactly as `ONEPIPELINE_NODE_VALIDATOR`
/// is for the per-node hook: `--writeback-item-budget` beats it, it beats the
/// launch config's own `writeback_item_budget`, and beneath all three is
/// [`DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS`]. Read once, at the launch, and
/// retained in the launch record — so an `adopt` replays what its launch resolved
/// rather than whatever this variable happens to say later.
pub const WRITEBACK_ITEM_BUDGET_ENV: &str = "ONEPIPELINE_WRITEBACK_ITEM_BUDGET";

/// How long a run-end hook is awaited, in seconds, when a launch names no
/// timeout.
///
/// The bottom rung of two: `--hook-timeout` beats the launch config's own
/// `hook_timeout`. A hook still running when it elapses has its process tree
/// ended. A [`NonZeroU64`] because a timeout of zero ends every hook before it
/// has begun, and both rungs refuse it.
pub const DEFAULT_HOOK_TIMEOUT_SECONDS: NonZeroU64 = NonZeroU64::new(600).unwrap();

/// How long the dispatch-env hook is awaited before each node launch, in
/// seconds, when a launch names no timeout.
///
/// The bottom rung of two: `--dispatch-env-hook-timeout` beats the launch
/// config's own `dispatch_env_hook_timeout`. Shorter than a run-end hook's,
/// because this one runs on the critical path of every dispatch and a launch
/// waits on it. A [`NonZeroU64`] for the run-end timeout's reason: zero ends the
/// hook before it has begun, and both rungs refuse it.
pub const DEFAULT_DISPATCH_ENV_HOOK_TIMEOUT_SECONDS: NonZeroU64 = NonZeroU64::new(60).unwrap();

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
#[allow(
    clippy::large_enum_variant,
    reason = "parsed once per process, so the size of `start`'s arguments costs nothing a \
              box would save; boxing that variant would change the shape of a public enum a \
              consumer matches on"
)]
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
    /// Inspect the channel command group.
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
    Monitor(MonitorArgs),
    /// Block until a run needs a supervisor, saying so as it waits.
    Watch(WatchArgs),
    /// Which of a session's runs has nothing watching it.
    Unwatched(UnwatchedArgs),
    /// Whether a session's turn may end: one verdict over `unwatched`, for a
    /// harness's stop hook.
    StopGuard(StopGuardArgs),
    /// Ask the manager a blocking question over the run's planner channel, and
    /// answer with theirs.
    Ask(AskArgs),
    /// Per-node outcomes, with each node's own evidence.
    Results(RunArgs),
    /// What each run is for, and how far it has got.
    Goals(OptionalRunArgs),
    /// A dispatched turn's tools and reasoning, from the evidence it retained.
    Transcript(TranscriptArgs),
    /// Every oneharness session a run, a node or a project launched, and where
    /// each one's transcript is.
    Agents(AgentsArgs),
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

/// The channel has no engine-owned serving verbs; what it has is a read.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum ChannelCommand {
    /// Parser sentinel: the channel exposes no executable engine verb.
    #[command(hide = true, name = "__no-engine-verb")]
    NoEngineVerb,
    /// Read a run's channel: every surface and where it is, the replies, and the
    /// command queue with the reconciler's answers. Consumes nothing.
    Queue(RunArgs),
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
    /// The node crosses as JSON on its stdin; exit 0 accepts the edit, exit 1
    /// refuses it with the command's own stderr as the reason, and any other
    /// ending gives no verdict and refuses it naming the validator.
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
    /// into, and the run's goal. Exit 0 accepts the envelope and exit 1
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
    /// The command whose output fingerprints the bar the envelope reviewer
    /// judges against.
    ///
    /// Named, each pass the reviewer gives is recorded under the run's own
    /// `validator-passes/`, keyed on the document it judged and on what this
    /// command prints when the pass is looked for: an identical envelope under an
    /// unchanged bar is passed without running the reviewer again, and a bar that
    /// moved runs it again. A command that fails or prints nothing keys nothing,
    /// and the envelope is reviewed uncached. Given here it beats
    /// `ONEPIPELINE_ENVELOPE_REVIEWER_BAR` and the launch config's own field.
    #[arg(long, value_name = "COMMAND")]
    pub envelope_reviewer_bar: Option<String>,
    /// The `onemessagebus` configuration this run's channel is kept under.
    ///
    /// Read at the launch and retained in the launch record: its `authors` block
    /// declares who may issue which operations, and its `validators` judge what
    /// is offered to the channel's queues. Codec and schema-link settings belong
    /// to the host's bus server and are retained without being acted on. Its transport is the run
    /// root's own, so a `transport` other than `local`, a `transport.dir`, a
    /// `profile` other than `planner-channel`, and a `queues` block are refused,
    /// naming the key and the value read, before any run exists. Given here it
    /// beats the launch config's own field.
    #[arg(long, value_name = "PATH")]
    pub bus_config: Option<PathBuf>,
    /// The pool-maintenance schedule this run's idle driver sweeps `onevcs`'s
    /// worktree pool on: a YAML or JSON file naming, per identity, how old a
    /// slot's `last_maintained` may be before it is due.
    ///
    /// `version: 1`, a `default: {every: SPAN}`, and `rules`, first match wins,
    /// each `{match: {host, owner, name, path}, every: SPAN}` — `every` the only
    /// key either carries, a SPAN spelled the way `onevcs` spells one (digits
    /// then s, m, h or d), and `match` the onevcs rules vocabulary. Read at the
    /// launch and retained in the launch record; an unknown key, another
    /// `version`, a rule naming no match field or a malformed span is refused,
    /// naming the key, before any run exists. On a pass that dispatched nothing
    /// and has capacity to spare, the driver runs `onevcs pool maintain` over
    /// every registered identity with that identity's `every`; the sibling's
    /// recorded stamp decides what is due, so nothing is kept here, nothing runs
    /// twice, and two drivers on one host are safe. Naming none is the shipped
    /// default and runs no maintenance at all. Given here it beats the launch
    /// config's own field — including when what it names is blank, which is
    /// this launch saying it has none.
    #[arg(long, value_name = "FILE")]
    pub maintenance_config: Option<String>,
    /// How often the durable planner-update check-in comes due, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = DEFAULT_HEARTBEAT_INTERVAL_SECONDS)]
    pub heartbeat_interval: u64,
    /// How long the settlement write-back allows its store's `project copy` per
    /// item it writes, in seconds.
    ///
    /// The copy's deadline is this multiplied by the number of items the run
    /// projects, and never below the sixty-second floor every other store
    /// command is bounded by, so the backstop that kills an unreachable store's
    /// copy scales with the plan instead of being outgrown by it. A positive
    /// whole number: zero is no budget at all and is refused. Given here it
    /// beats `ONEPIPELINE_WRITEBACK_ITEM_BUDGET` and the launch config's own
    /// field; naming none takes the shipped ten seconds per item.
    #[arg(long, value_name = "SECONDS")]
    pub writeback_item_budget: Option<u64>,
    /// The command run once when this run ends with every node `done`.
    ///
    /// Spawned the way `--node-validator` is, in the launch directory, with the
    /// run named in its environment and one JSON document on its stdin; awaited
    /// for up to `--hook-timeout`, and never able to change how the run settled.
    /// Naming none is the shipped default and is exactly what a launch did before
    /// this flag existed. Given here it beats the launch config's own field —
    /// including when what it names is blank, which is this launch saying it has
    /// none.
    #[arg(long, value_name = "COMMAND")]
    pub success_hook: Option<String>,
    /// The command run once when this run ends any other way: a failed or
    /// skipped node, an unfinished graph with no decision outstanding, or a clean
    /// `stop`.
    ///
    /// Spawned, awaited and overridden exactly as `--success-hook` is. A run
    /// paused on a decision has not ended and fires neither.
    #[arg(long, value_name = "COMMAND")]
    pub failure_hook: Option<String>,
    /// How long a run-end hook is awaited, in seconds, before its process tree is
    /// ended.
    ///
    /// A positive whole number: zero is refused where the flag is parsed, before a
    /// run exists. Given here it beats the launch config's own field; naming none
    /// takes the shipped six hundred seconds.
    #[arg(long, value_name = "SECONDS")]
    pub hook_timeout: Option<NonZeroU64>,
    /// The command run immediately before **every** node-scope dispatch — a
    /// first dispatch, a re-asked one, a retry, a requeue and a lifecycle node's
    /// worker launch alike, and never the dag-scope observer — whose stdout adds
    /// environment to that one child launch.
    ///
    /// Spawned the way a run-end hook is: the command itself, no shell and no
    /// arguments, in the launch directory, with `ONEPIPELINE_HOOK=dispatch-env`,
    /// `ONEPIPELINE_RUN_ID`, `ONEPIPELINE_RUN_ROOT` and `ONEPIPELINE_NODE_ID` in
    /// its environment. It prints exactly one JSON document,
    /// `{"version": 1, "env": {"NAME": "value", ...}}`, whose `env` is overlaid on
    /// the driver's environment for that child launch only; the driver's own
    /// environment is unchanged. After the overlay every `env_from` source the
    /// launch's oneharness configs name is checked for, and a hook that exits
    /// non-zero, cannot start, times out or prints anything else — or a source
    /// still missing — refuses the launch: nothing is dispatched and the node
    /// settles `infrastructure-failure` with a detail naming why. Its stderr is
    /// kept in `hooks/dispatch-env.log` under the run, and no value it prints is
    /// ever written anywhere the run keeps. Naming none is the shipped default
    /// and is exactly what a launch did before this flag existed. Given here it
    /// beats the launch config's own field — including when what it names is
    /// blank, which is this launch saying it has none.
    #[arg(long, value_name = "COMMAND")]
    pub dispatch_env_hook: Option<String>,
    /// How long the dispatch-env hook is awaited, in seconds, before its process
    /// tree is ended and the launch is refused.
    ///
    /// A positive whole number: zero is refused where the flag is parsed, before a
    /// run exists. Given here it beats the launch config's own field; naming none
    /// takes the shipped sixty seconds.
    #[arg(long, value_name = "SECONDS")]
    pub dispatch_env_hook_timeout: Option<NonZeroU64>,
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
    /// Repeatable. `planner` and `detailed` ship and are overridden by name.
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
/// view, which is what these verbs are for.
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

/// `onepipeline monitor`: the run and profile selection every read verb takes,
/// and the cursor `watch` prints and reads back.
///
/// A struct of its own rather than a flag on [`ReadArgs`], because `next` takes
/// those too and a cursor is not something `next` can resume from.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct MonitorArgs {
    /// The run, and the profile its event view is shaped through.
    #[command(flatten)]
    pub read: ReadArgs,
    /// Render only what was recorded after this cursor — one an earlier
    /// `monitor` or `watch` printed.
    #[arg(long, value_name = "CURSOR")]
    pub cursor: Option<String>,
}

/// How long a `watch` waits before giving up, when it is given none.
///
/// One supervisory turn: long enough that a watch is worth making rather than a
/// poll, short enough that a caller which meant to look and move on is not held
/// for the length of the run.
pub const DEFAULT_WATCH_TIMEOUT_SECONDS: u64 = 300;

/// How often a `watch` says it is still there while nothing is happening, when
/// it is given none.
pub const DEFAULT_WATCH_TICK_SECONDS: u64 = 30;

/// The cursor-token prefix a `watch` prints and reads back.
///
/// Versioned because the token is a promise to a *later* invocation, which may
/// be a different build: a token this build cannot place is refused by name
/// rather than resumed from as though it meant a byte count.
pub const WATCH_CURSOR_VERSION: &str = "1";

/// The word a caller spells [`WatchTimeout::Unbounded`] with.
///
/// A word rather than a number, and deliberately not `0`: that value's published
/// meaning is to read the run once and return, so spelling "no bound" with it
/// would have made one value mean both the shortest wait there is and the
/// longest.
pub const WATCH_TIMEOUT_UNBOUNDED: &str = "none";

/// How long a `watch` waits before giving up.
///
/// The two spellings answer two different questions, which is why they are not
/// one number. A caller that wants a look at the run and not a wait asks for `0`;
/// a supervisor that wants to be woken when something worth knowing happens asks
/// for [`WATCH_TIMEOUT_UNBOUNDED`] and is not woken by a clock at all. Every
/// number in between bounds the wait as it always did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchTimeout {
    /// Give up after this many seconds. `0` reads once and returns.
    Bounded(u64),
    /// Never give up on the clock: the wait ends on a condition or not at all.
    Unbounded,
}

impl std::fmt::Display for WatchTimeout {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bounded(seconds) => write!(out, "{seconds}"),
            Self::Unbounded => out.write_str(WATCH_TIMEOUT_UNBOUNDED),
        }
    }
}

impl std::str::FromStr for WatchTimeout {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        if text == WATCH_TIMEOUT_UNBOUNDED {
            return Ok(Self::Unbounded);
        }
        text.parse().map(Self::Bounded).map_err(|_| {
            format!(
                "'{text}' is not a wait this verb can take: a wait is a number of seconds, \
                 where `0` reads the run once and returns, or `{WATCH_TIMEOUT_UNBOUNDED}`, \
                 which does not bound the wait at all"
            )
        })
    }
}

/// Every condition `--until` accepts: how a caller spells it, and what this
/// build reads it as.
///
/// **One table, read by the parser and by the refusal both**, so the message a
/// mistyped condition gets cannot omit a condition this build accepts — the
/// accepted set *is* this list, rather than a second copy of it standing beside
/// a match. The README and the divergence record are reconciled against the
/// spellings here too, so what a supervisor is told to type is what the parser
/// reads.
const CONDITIONS: [(&str, WatchUntil); 5] = [
    ("settled", WatchUntil::Settled),
    ("surface", WatchUntil::Surface),
    ("nothing-driving", WatchUntil::NothingDriving),
    ("node-settled", WatchUntil::NodeSettled),
    // The one entry that is a **shape** rather than a word: what follows the
    // prefix is a node id the caller supplies, so the parser reads this row by
    // its prefix and builds the condition from the text after it. The value
    // beside it is never returned.
    (WATCH_NODE_CONDITION_SHAPE, WatchUntil::Node(String::new())),
];

/// How a caller names one node to return on, with the id it stands in for.
pub const WATCH_NODE_CONDITION_SHAPE: &str = "node=<ID>";

/// What that shape stands in for: the id follows this.
const NODE_ID_PLACEHOLDER: &str = "<ID>";

/// Every condition `--until` accepts, spelled as a caller types it.
///
/// Derived from the one table above rather than restated, so a spelling this
/// build reads is a spelling it names.
pub fn watch_conditions() -> [&'static str; 5] {
    CONDITIONS.map(|(spelling, _)| spelling)
}

/// What ends a `watch`, as a caller names it.
///
/// **Repeatable, and additive to what the verb always returns on.** A run that
/// settles `complete` and a run nothing is driving end every wait whether or not
/// they were asked for, because a wait that could outlive the run it watches is
/// the unbounded silence this verb exists to end — so [`Settled`](Self::Settled)
/// and [`NothingDriving`](Self::NothingDriving) name conditions rather than
/// switch them on, and what `--until settled` *adds* is nothing, which is why it
/// still means "do not return on a blocking surface".
///
/// [`Surface`](Self::Surface) is spelled for the surface rather than for a
/// "decision", which in this crate is the wider fact `status` reports: a ready
/// human action is a decision point too, and a value that claimed to cover both
/// while returning on one of them would be the prose-shaped promise this verb
/// exists to replace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WatchUntil {
    /// Return on a blocking surface waiting to be answered. The default, and on
    /// its own it is the pair a supervisor answers: this or the run finishing.
    #[default]
    Surface,
    /// Return when the run finishes. Named on its own it adds nothing, so it is
    /// still how a caller says a blocking surface should be reported and waited
    /// through rather than returned on.
    Settled,
    /// Return when nothing is driving the run. Always returned on, named here so
    /// a caller can spell the whole vocabulary.
    NothingDriving,
    /// Return when any node of the run settles.
    NodeSettled,
    /// Return when this node of the run settles. Validated against the run's own
    /// graph when the command is invoked.
    Node(String),
}

impl std::fmt::Display for WatchUntil {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Surface => out.write_str("surface"),
            Self::Settled => out.write_str("settled"),
            Self::NothingDriving => out.write_str("nothing-driving"),
            Self::NodeSettled => out.write_str("node-settled"),
            Self::Node(node) => write!(
                out,
                "{}{node}",
                WATCH_NODE_CONDITION_SHAPE
                    .strip_suffix(NODE_ID_PLACEHOLDER)
                    .unwrap_or(WATCH_NODE_CONDITION_SHAPE)
            ),
        }
    }
}

impl std::str::FromStr for WatchUntil {
    type Err = String;

    /// Read one condition, or refuse it naming the vocabulary.
    ///
    /// The refusal is made by the parser, so it happens as the command line is
    /// read: nothing is streamed and nothing waits behind a condition this verb
    /// does not have. What it cannot judge here is a condition that is *spelled*
    /// right and names a node this run does not hold — that one needs the run's
    /// graph, and `src/watch.rs` refuses it before it blocks.
    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        for (spelling, condition) in CONDITIONS {
            match spelling.strip_suffix(NODE_ID_PLACEHOLDER) {
                // A row that stands for a shape matches on its prefix, and the
                // id is what the caller wrote after it. An empty one names no
                // node, so it falls through to the refusal rather than becoming
                // a condition about a node with no name.
                Some(prefix) => {
                    if let Some(node) = text.strip_prefix(prefix).filter(|node| !node.is_empty()) {
                        return Ok(Self::Node(node.to_string()));
                    }
                }
                None if text == spelling => return Ok(condition),
                None => {}
            }
        }
        Err(format!(
            "'{text}' is not a condition this verb returns on; it returns on {}",
            watch_conditions().join(", ")
        ))
    }
}

/// The streaming verb's own run and profile selection, plus the four things a
/// supervisor needs to write no wake loop at all: a bound on the wait — or none
/// — a heartbeat so silence and death are tellable apart, a cursor to resume
/// from, and the conditions it returns on.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct WatchArgs {
    /// The run, and the profile its event view is shaped through — exactly as
    /// `monitor` takes them.
    #[command(flatten)]
    pub read: ReadArgs,
    /// How long to wait before giving up, in seconds. `0` reads once and
    /// returns; `none` does not bound the wait at all.
    #[arg(long, value_name = "SECONDS|none", default_value_t = WatchTimeout::Bounded(DEFAULT_WATCH_TIMEOUT_SECONDS))]
    pub timeout: WatchTimeout,
    /// How long a silence may last before this stream says it is still there,
    /// in seconds. `0` turns the heartbeat off.
    ///
    /// **The stream's** interval, and deliberately not spelled
    /// `--heartbeat-interval`: that one is `start`'s, and it sets how often the
    /// scheduled member surfaces a planner update. The two are different clocks
    /// on different verbs, and neither verb accepts the other's flag.
    #[arg(long, value_name = "SECONDS", default_value_t = DEFAULT_WATCH_TICK_SECONDS)]
    pub tick_interval: u64,
    /// Resume from the cursor an earlier watch printed, emitting nothing it
    /// already did.
    #[arg(long, value_name = "CURSOR")]
    pub cursor: Option<String>,
    /// What ends the wait, beside the run finishing and nothing driving it.
    /// Repeatable: the wait returns on the first of them that fires, and says
    /// which one did.
    #[arg(long, value_name = "CONDITION", default_values_t = [WatchUntil::Surface])]
    pub until: Vec<WatchUntil>,
}

/// `onepipeline unwatched`.
///
/// The session is taken as an option **as well as** from the environment, and
/// that is the whole of the argument surface. Its consumer is a hook that is
/// handed the session it must ask about on standard input, while the environment
/// it runs in carries somebody else's — so a verb that could only read the
/// environment would answer confidently about the wrong session.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct UnwatchedArgs {
    /// The launching session whose runs to ask about. Omitted, the session
    /// `ONEPIPELINE_LAUNCHER_SESSION` names.
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
}

/// `onepipeline stop-guard`.
///
/// The general stop guard: the session whose stop this is and whether the stop
/// continues a block the guard made, as flags or as fields of one object on
/// standard input, and the shape the verdict is rendered in. What it decides
/// and how, and why the session is never read from the environment, is entry 85
/// of `docs/contract-divergences.md`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct StopGuardArgs {
    /// The session whose stop this is. Omitted, it is read from the object on
    /// standard input the format names — never from the environment.
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
    /// This stop follows a block this guard made: answer `none` where the
    /// report it would block on is the one it last blocked this session on.
    #[arg(long)]
    pub continuation: bool,
    /// How the input is read and the verdict rendered.
    #[arg(long, value_enum, default_value_t = StopGuardFormat::Neutral)]
    pub format: StopGuardFormat,
}

/// How the verdict is rendered: the neutral object, or a harness's own shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum StopGuardFormat {
    /// `{"verdict":"block","reason":…}`, `{"verdict":"warn","message":…}` or
    /// `{"verdict":"none"}`; the input is `--session`/`--continuation`, or one
    /// object `{"session":…,"continuation":…}` on standard input.
    #[default]
    Neutral,
    /// Claude Code's `Stop` hook: the payload's `session_id` and
    /// `stop_hook_active` are read off standard input, and the verdict is
    /// `{"decision":"block","reason":…}`, `{"systemMessage":…}` or nothing.
    ClaudeCode,
    /// Codex's `Stop` hook, which reads and answers the same shape Claude
    /// Code's does.
    Codex,
}

/// `onepipeline ask`.
///
/// The question in one of three forms — the argument words, a file, or standard
/// input when neither is given — and what rides beside it. The run is
/// `ONEPIPELINE_RUN_ID`'s and the asker `ONEPIPELINE_CHANNEL_ASKER`'s, read in
/// the binary's arm; entry 87 of `docs/contract-divergences.md` states the
/// verb.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct AskArgs {
    /// The question, as words joined by one space. Omitted, it is read from
    /// `--file`, or from standard input when neither is given.
    #[arg(value_name = "TEXT", conflicts_with = "file")]
    pub text: Vec<String>,
    /// The file the question is read from.
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,
    /// The node the question is about: at most 512 bytes, not blank, with no
    /// control character.
    #[arg(long, value_name = "NODE")]
    pub about: Option<String>,
    /// The reply window, in seconds. Omitted, the `reply_window_seconds` the
    /// run's launch record's bus configuration names for the `surfaces` queue,
    /// or the bus's own default when it names none.
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<u64>,
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
    /// Hold this process open until the graph's own run record carries the
    /// ending it stamps *after* announcing its settlement.
    ///
    /// What the launcher of an observer passes, because it reads that record
    /// rather than the envelopes this process relays — the engine's `Ending`
    /// is where that is written down. Named in plain code rather than linked,
    /// because the engine is private and this struct is public, and rustdoc
    /// refuses that link under this repository's denied warnings. Spelled by
    /// `retained_command` and by nobody else, so it is not an operator-facing
    /// flag; it is on this hidden verb's argv because that argv is how a
    /// retained launch is told anything.
    #[arg(long)]
    pub await_ending: bool,
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
    /// The question a verdict answers, by the correlation its asker was told.
    /// Omitted, the verdict is bound to the question it can be bound to — see
    /// `docs/contract.md`'s channel paragraph.
    #[arg(long, value_name = "C")]
    pub correlation: Option<onemessagebus::Correlation>,
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
    #[arg(long, value_name = "KIND")]
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
    /// One row per run, by run id, rather than grouped by project.
    #[arg(long)]
    pub flat: bool,
}

/// `onepipeline transcript`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TranscriptArgs {
    /// The run id.
    pub run: String,
    /// The node whose transcript to read. Omitted, every node that dispatched.
    pub node: Option<String>,
}

/// `onepipeline agents`.
///
/// One of a run and a project, never both: a run's sessions are read off that
/// run's own pointer file, and a project's off every run its summary names.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
#[command(group = clap::ArgGroup::new("whose").required(true).args(["run", "project"]))]
pub struct AgentsArgs {
    /// The run id.
    pub run: Option<String>,
    /// The node whose sessions to list. Omitted, every session of the run.
    #[arg(requires = "run")]
    pub node: Option<String>,
    /// Every session across the runs launched from this qualified project id.
    #[arg(long, value_name = "PROJECT", conflicts_with_all = ["run", "node"])]
    pub project: Option<String>,
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
