//! The run engine: drive selected harnesses and return one [`RunReport`].
//!
//! This is `oneharness run` minus the process it used to require. The verb's
//! whole orchestration lives here — selection, validation, job construction,
//! the four execution models (parallel, fallback, streamed, server-submitted),
//! history, sessions, and report assembly — behind [`run`], which **returns**
//! the report instead of printing it. Nothing on this path writes to the
//! process's stdout; the `oneharness` binary is the thin shell that prints what
//! it gets back, and an in-process caller reads the same value.
//!
//! Four things a caller supplies that a subprocess hop used to give for free —
//! three through [`RunControls`], the fourth through [`run_supervised`]:
//!
//! * an [`EventSink`], so normalized events arrive **as they occur** rather than
//!   when the run ends (the CLI's sink is the one that writes the NDJSON stream
//!   protocol to its stdout);
//! * a [`CancelToken`], so the caller can tear the harness tree down through the
//!   ordinary [`crate::io::runner`] `Finish::Terminate` path — a harness is its
//!   own process-group leader, so nothing the caller signals reaches it;
//! * whether oneharness may own the host's SIGINT/SIGTERM disposition
//!   ([`RunControls::signal_cancel`]), which the CLI wants and an embedder with
//!   its own signal handling does not;
//! * a [`ProcessSupervisor`] ([`run_supervised`]), so the caller can put each
//!   harness child into the process group / job object it supervises — the
//!   grouping the subprocess hop provided, without which a watchdog cannot see
//!   the harness subtree as one unit and the caller's own kill does not reap it.
//!
//! Diagnostics still go to the host's stderr (`eprintln!`), exactly as the CLI
//! emitted them, so a warning a run produces is never silently dropped.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Finite backstop applied only when an omitted timeout meets a mode that may
/// wait for interactive approval headlessly.
pub const APPROVAL_WAIT_TIMEOUT_SECS: u64 = 120;

use crate::domain::batch::{self, BatchStrategy};
use crate::domain::control::{self, ControlReport, ControlShape, DialAddress};
use crate::domain::dialogue::{Dialogue, DialogueConfig, DialogueResume};
use crate::domain::events::ActionEvent;
use crate::domain::fallback::{self, RunMode};
use crate::domain::harness::{self, BuildCtx, HarnessIdentity, HarnessSpec, PromptDelivery};
use crate::domain::http::{self, HttpShape};
use crate::domain::mock::{self, MockDelivery};
use crate::domain::mode::{ApprovalPosture, ModeHeadless, PermissionMode};
use crate::domain::report::{
    BatchReport, Capture, FallThrough, FallbackReport, OutputFormat, RunReport, RunResult,
    SessionReport, Status, SCHEMA_VERSION,
};
use crate::domain::select::select_specs;
use crate::domain::session::{self, SessionPlan, SessionRecord};
use crate::domain::signals::Usage;
use crate::domain::structured::{self, Schema};
use crate::domain::{events, normalize, signals};
use crate::errors::{OneharnessError, RunModeOrigin};
use crate::io::cancel::{self, CancelToken};
use crate::io::config as config_io;
use crate::io::control as control_io;
use crate::io::detect::{self, BinOverrides};
use crate::io::history::{self, HistoryWriter};
use crate::io::hooks::{self as hooks_io, HookSnapshot, Scope};
use crate::io::http_turn;
use crate::io::identity::{
    variant_environment, variant_unprovisioned_identity, UnprovisionedIdentity,
};
use crate::io::model_catalog;
use crate::io::runner::{self, Job, NextRun, Outcome, ProcessSupervisor, SpawnControls};
use crate::io::server_pool;
use crate::io::session as session_io;
use std::path::PathBuf;

/// Every harness the run drove succeeded (or was tolerably absent).
pub const EXIT_OK: i32 = 0;
/// At least one harness failed — see `results[].status` / `results[].error`.
/// (The `oneharness` CLI reserves 2 for its own usage/configuration faults,
/// which surface here as an [`OneharnessError`] instead.)
pub const EXIT_FAILURE: i32 = 1;

/// What an [`EventSink`] asks the run to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkStep {
    /// Keep running — deliver the next event when it arrives.
    Continue,
    /// Stop now: the consumer has seen enough (it short-circuited on an observed
    /// action, or its own output went away). The run tears the harness tree down
    /// through the ordinary terminate path and still returns a report.
    Stop,
}

pub use crate::domain::history::RunBegun;

/// Where a run announces its history location and publishes streamed events.
///
/// The primitive one layer down ([`crate::io::runner::run_job_streaming`]) has
/// always taken a per-line callback; this is what carries it out to a caller.
/// The `oneharness` binary's sink writes the NDJSON stream protocol to its own
/// stdout, which is why nothing in this module does.
pub trait EventSink {
    /// Locate a history-enabled candidate as it begins, whether or not the run streams.
    fn run_began(&mut self, _begun: &RunBegun) -> SinkStep {
        SinkStep::Continue
    }

    /// Deliver one event, attributed to the plan entry that produced it
    /// (`harness_id` is the variant-qualified selector, so a model fan-out's
    /// repeated harness is still attributable). Returning [`SinkStep::Stop`]
    /// ends the run.
    fn event(&mut self, harness_id: &str, event: &ActionEvent) -> SinkStep;
}

struct StreamFilteredSink<'a> {
    sink: &'a mut dyn EventSink,
    stream: bool,
}

impl EventSink for StreamFilteredSink<'_> {
    fn run_began(&mut self, begun: &RunBegun) -> SinkStep {
        self.sink.run_began(begun)
    }

    fn event(&mut self, harness_id: &str, event: &ActionEvent) -> SinkStep {
        if self.stream {
            self.sink.event(harness_id, event)
        } else {
            SinkStep::Continue
        }
    }
}

/// The caller-owned side channels of a run: where events go, how it is
/// cancelled, whose signal disposition applies, and which version the report
/// names.
///
/// [`Default`] is the embedder's baseline — no sink, a fresh token, the host's
/// own signal handling left alone, and the engine's version on the report.
///
/// Every field is public and the struct is exhaustively constructible, which is
/// exactly why a *new* side channel does not arrive here: adding a field breaks
/// every literal a consumer has already written, so a capability that is purely
/// additive would ship as a major bump. New ones take their own entry point
/// instead — [`run_supervised`] is the first, and this struct passes through it
/// unchanged.
#[derive(Default)]
pub struct RunControls<'a> {
    /// Receives each normalized event of a **streaming** run
    /// ([`RunRequest::stream`], or `stream` in config / `ONEHARNESS_STREAM`).
    /// A buffered run publishes nothing incrementally, so this is never called
    /// for one — its events are on the returned report.
    pub events: Option<&'a mut dyn EventSink>,
    /// Cancels this run. Cancelling terminates each harness tree the run spawned
    /// through `Finish::Terminate` — a harness is its own process-group leader,
    /// so a signal the caller sends its own group never reaches one — and the
    /// run still returns a report, with the cut-short harnesses as
    /// [`Status::Cancelled`].
    pub cancel: CancelToken,
    /// Install SIGINT/SIGTERM (Unix) / console-control (Windows) handlers that
    /// cancel the run instead of killing the host. Process-global and therefore
    /// opt-in: the CLI sets it, an embedder with its own signal handling leaves
    /// it `false` and cancels [`RunControls::cancel`] itself.
    // llmlint: ignore[changed_behavior_has_e2e] The `true` arm is already driven end to end by `a_host_signal_cancels_the_run_and_terminates_a_silent_harness` (tests/cli.rs), which SIGTERMs a real `oneharness run` — the CLI is the only caller that sets this — and asserts the harness's descendant stopped and the report still landed; the `false` arm is what every test in tests/library.rs runs under. A second test would have to install a process-wide handler and signal its own test process, which under the `cargo test` fallback (one process, many tests) poisons its siblings.
    pub signal_cancel: bool,
    /// The version the report attributes the run to. `None` uses this engine's
    /// own crate version; the CLI passes its binary version so `oneharness run`
    /// keeps naming the shipped artifact.
    pub version: Option<String>,
}

/// What a finished [`run`] hands back: the report, the exit code it maps to, and
/// the two things the CLI shell needs to reproduce its output byte for byte.
pub struct RunOutcome {
    /// The report — the same document `oneharness run` prints on stdout.
    pub report: RunReport,
    /// [`EXIT_OK`] or [`EXIT_FAILURE`], by the run mode's own success rule.
    pub exit_code: i32,
    /// Whether the run published incrementally, so a consumer that read the
    /// events closes with the terminal `{"type":"result",…}` envelope.
    pub streamed: bool,
    /// The one-line summary a non-zero `exit_code` deserves, naming which
    /// harnesses did not succeed. `None` on success. The CLI writes it to stderr
    /// after the report so the stdout document stays first.
    pub failure_summary: Option<String>,
}

/// A session continuation: the harness's own session id, and whether to branch
/// a new session from it instead of appending to it.
///
/// One value rather than two fields, because forking is a property *of* a
/// resume — there is nothing to branch from without one, and no adapter emits
/// its fork flag ([`crate::domain::harness::BuildCtx::fork`]) outside the
/// `--resume` arm.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resume {
    /// The harness's native session id, as it reported one.
    // llmlint: ignore[invalid_states_unrepresentable] A native session id is opaque provider text with no shared grammar across the eight harnesses (a UUID here, a thread id there); the engine forwards it verbatim through each adapter's verified `--resume` mapping, and a harness that cannot resolve it answers with the `session_not_found` classification — the only validation that could be honest.
    pub session: String,
    /// Branch a new session from [`Resume::session`] instead of appending, so
    /// the original (and its cached prefix) is untouched. Only for a harness
    /// that declares `supports_fork`; others are a loud usage error.
    pub fork: bool,
}

/// One run, as data.
///
/// Field-for-field the `oneharness run` flag surface (the binary's clap `RunArgs`
/// converts into this), minus `--compact`: that is about how the shell *prints*
/// the report, not how the engine produces it. Every field is a plain owned
/// value, so a library caller builds one with [`Default`] and struct-update
/// syntax — which also keeps a future field from breaking them:
///
/// ```
/// use oneharness_core::io::run::RunRequest;
/// let request = RunRequest {
///     harness: vec!["claude-code".to_string()],
///     prompt: vec!["say hi".to_string()],
///     ..RunRequest::default()
/// };
/// assert!(!request.all);
/// ```
///
/// The layered defaults still apply: anything left unset falls through to the
/// discovered `oneharness.toml` files and the `ONEHARNESS_*` environment
/// overrides, exactly as it does for the CLI. Set [`RunRequest::no_config`] for a
/// hermetic run.
///
/// Where the CLI spells a choice as a mutually-exclusive *pair* of flags, this
/// carries the one value they resolve to, so the conflicting state cannot be
/// built at all: `--stream`/`--no-stream` and `--history`/`--no-history` are one
/// `Option<bool>` each (`None` = defer to config), `--bypass`/`--no-bypass` fold
/// into [`RunRequest::mode`] (they are shorthands for one), and `--fork` lives
/// inside the [`Resume`] it has no meaning without. The binary's own conversion
/// applies clap's precedence on the way in.
#[derive(Debug, Clone, Default)]
pub struct RunRequest {
    /// Run against every supported harness.
    pub all: bool,
    /// Harness id(s) to run, in priority order (`base` or `base:variant`).
    pub harness: Vec<String>,
    /// Replace this selected harness's provider process with oneharness's own
    /// deterministic mock responder (a test hook).
    pub mock_harness: Vec<String>,
    /// Harness id(s) to exclude under [`RunRequest::all`].
    pub exclude: Vec<String>,
    /// The prompt(s) to send. More than one makes this a **batch**: one harness
    /// fanned over each prompt, sharing the cacheable system/model prefix.
    pub prompt: Vec<String>,
    /// Files to read further whole prompts from; `-` reads stdin once.
    pub prompt_file: Vec<String>,
    /// The model(s). More than one is a **model fan-out** (harness × model).
    pub model: Vec<String>,
    /// System prompt, delivered through each harness's native mechanism.
    pub system: Option<String>,
    /// Reasoning / thinking effort, forwarded verbatim to a harness that
    /// exposes one on the argv.
    pub reasoning: Option<String>,
    /// Read the system prompt from a file (`-` for stdin) instead of inline.
    pub system_file: Option<String>,
    /// Continue a prior session by its native id, optionally branching it.
    pub resume: Option<Resume>,
    /// Continue (or start) a conversation by a stable, caller-owned name.
    pub session: Option<String>,
    /// Where the [`RunRequest::session`] store lives.
    pub session_dir: Option<PathBuf>,
    /// Open the out-of-band turn-control socket for the run's lifetime.
    pub control: bool,
    /// Pin the output format requested from each harness.
    pub output_format: Option<OutputFormat>,
    /// Surface each harness's normalized tool-call `events`.
    pub events: bool,
    /// Publish events incrementally to [`RunControls::events`] as they occur.
    /// `None` defers to `stream` in config / `ONEHARNESS_STREAM` (off by
    /// default); `Some` overrides it in either direction.
    pub stream: Option<bool>,
    /// A one-shot mock/spy ruleset for the selected harnesses' tool calls.
    pub mock_rules: Option<PathBuf>,
    /// Append one JSONL record per observed tool call to this file.
    pub spy_file: Option<PathBuf>,
    /// Constrain each harness's final answer to this JSON Schema file.
    pub schema: Option<PathBuf>,
    /// Max re-prompts when a response fails schema validation (default 2).
    pub schema_max_retries: Option<u32>,
    /// Max retries for a zero-work Codex server-overloaded refusal; omitted
    /// uses [`crate::domain::config::SERVER_OVERLOADED_MAX_RETRIES_DEFAULT`].
    pub server_overloaded_max_retries: Option<u32>,
    /// Also write each harness's raw stdout/stderr under this directory.
    pub output_dir: Option<PathBuf>,
    /// Per-harness timeout in seconds; omitted or zero means no timeout.
    pub timeout: Option<u64>,
    /// Working directory each harness process runs in; also where project
    /// config discovery starts.
    pub cwd: Option<PathBuf>,
    /// Extra `KEY=VALUE` environment for each harness process.
    pub env: Vec<String>,
    /// The approval mode requested from each harness. `None` defers to config
    /// `mode` (then the legacy config `bypass`, then [`PermissionMode::Default`]).
    /// The CLI's `--bypass` / `--no-bypass` shorthands resolve into this.
    pub mode: Option<PermissionMode>,
    /// Silence the warning that the chosen mode may block on an approval prompt.
    pub permit_prompts: bool,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery — the repeatable `--config` flag. Each later file
    /// (its own `extends` chain beneath it) overrides the earlier ones; empty
    /// means discovery. See [`crate::io::config::load_layers`].
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Vec<PathBuf>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. Takes
    /// precedence over `config`: set together, no file is loaded.
    pub no_config: bool,
    /// Maximum harnesses (or batch prompts) to run concurrently.
    pub max_parallel: Option<usize>,
    /// How a batch run schedules its calls.
    pub batch_strategy: Option<BatchStrategy>,
    /// The fallback priority chain (the default when unset here and in every
    /// config layer) or parallel.
    pub run_mode: Option<RunMode>,
    /// Build and report each command without executing it (dry run).
    pub print_command: bool,
    /// Harness binary overrides, `ID=PATH`.
    pub bin: Vec<String>,
    /// Treat a not-installed harness as a failure.
    pub require_available: bool,
    /// Record this run to the normalized cross-harness history store. `None`
    /// defers to `history` in config / `ONEHARNESS_HISTORY` (off by default);
    /// `Some` overrides it in either direction.
    pub history: Option<bool>,
    /// Directory history is written to.
    pub history_dir: Option<PathBuf>,
    /// The run's pointer file: with history on, every harness run this run
    /// begins appends one [`crate::domain::history::HistoryPointer`] line here
    /// saying where its session went. `None` defers to config
    /// `history_pointer_file` / `ONEHARNESS_HISTORY_POINTER_FILE`; unset
    /// everywhere writes none.
    pub history_pointer_file: Option<PathBuf>,
    /// Human-meaningful name for this history session.
    pub history_name: Option<String>,
    /// Validated `KEY=VALUE` labels attached to every history record.
    pub history_label: Vec<String>,
    /// Extra arguments appended verbatim to each harness command.
    pub passthrough: Vec<String>,
}

/// Byte length past which a prompt or system prompt is delivered off the argv
/// (temp file / stdin) for a harness that supports it, instead of inline — so a
/// large value never trips the OS argument ceiling (`E2BIG`: Linux caps a single
/// argv string at 128 KiB, macOS/Windows cap the whole argv+env). 64 KiB is well
/// under every ceiling (leaving headroom for the rest of the argv and env) yet far
/// above any ordinary prompt, so the common case keeps its byte-identical inline
/// argv and only genuinely-large prompts switch delivery. See `LargeInput` and
/// issue #1115.
const LARGE_INPUT_THRESHOLD: usize = 64 * 1024;

/// How long a freshly launched control server is given to start answering. A
/// launched server is not a reachable one — opencode takes seconds to bind, and
/// crush's socket file appears before it accepts — so this is generous; the
/// run's own timeout bounds it further.
const SERVER_READY_WINDOW: Duration = Duration::from_secs(90);

/// How many times one dispatch will launch a control server before giving up.
/// One relaunch, and only for a server that exited during bring-up — see
/// [`bring_up_server`] for which failure earns it and why the other does not.
const SERVER_START_ATTEMPTS: usize = 2;

/// Temp files holding off-argv prompt/system text for the duration of a run,
/// removed on drop — so every early return (stream path, an I/O error, normal
/// completion) cleans them up, like the mock hook's snapshot-and-restore. Writes
/// are best-effort; a removal failure is ignored (a leftover temp file is
/// harmless and the OS reclaims its temp dir).
#[derive(Default)]
struct TempPromptFiles(Vec<PathBuf>);

impl Drop for TempPromptFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl TempPromptFiles {
    /// Write `contents` to a fresh temp file (labelled by harness id + a unique
    /// index) and return its path, registering it for cleanup. The path is
    /// process- and index-unique so concurrent units never collide.
    fn write(
        &mut self,
        id: &str,
        index: usize,
        contents: &str,
    ) -> Result<PathBuf, OneharnessError> {
        let path = std::env::temp_dir().join(format!(
            "oneharness-input-{id}-{}-{index}.txt",
            std::process::id()
        ));
        std::fs::write(&path, contents).map_err(|source| OneharnessError::PromptFile {
            path: path.display().to_string(),
            source,
        })?;
        self.0.push(path.clone());
        Ok(path)
    }
}

/// Decide and apply off-argv delivery for a unit whose prompt or system prompt is
/// large enough to risk the argv ceiling. Mutates `plan` in place (setting
/// `system_file` / `prompt_stdin`, which the structured-output retry then reuses)
/// and, for a system prompt on a harness with a system-file flag, writes it to a
/// temp file. Small prompts return with `plan` untouched (byte-identical inline
/// argv). When a large value cannot be moved off the argv for the harness (no
/// stdin/file route), it stays inline and a warning names the risk rather than
/// silently letting the spawn fail later.
fn plan_large_input(
    plan: &mut HarnessPlan,
    spec: &HarnessSpec,
    system: Option<&str>,
    index: usize,
    temp_files: &mut TempPromptFiles,
) -> Result<(), OneharnessError> {
    let prompt_large = plan.base_prompt.len() > LARGE_INPUT_THRESHOLD;
    let system_large = system.is_some_and(|s| s.len() > LARGE_INPUT_THRESHOLD);
    if !prompt_large && !system_large {
        return Ok(());
    }
    let li = &spec.large_input;
    // System prompt via a file flag (Claude Code): materialize it and let
    // build_argv emit the flag instead of the inline value.
    let use_system_file = system_large && li.system_file_flag.is_some();
    if use_system_file {
        let path = temp_files.write(spec.id, index, system.unwrap_or_default())?;
        plan.system_file = Some(path.display().to_string());
    }
    // User prompt (and, for a system-rides-prompt harness, the system with it) via
    // stdin. Also triggered by a large *system* alone on such a harness, since its
    // system rides the same stream.
    let use_stdin = li.prompt_stdin && (prompt_large || (li.system_rides_prompt && system_large));
    if use_stdin {
        // A control-enabled run already owns stdin as a message stream; a large
        // prompt rides that stream's first frame, so the blob route never
        // displaces it.
        if !plan.delivery.is_control_stream() {
            plan.delivery = PromptDelivery::Stdin;
        }
    }
    // Loud when a large value is stuck on the argv anyway (e.g. Goose's inline
    // `--system`, or any harness oneharness has not wired for off-argv input).
    if prompt_large && !use_stdin {
        eprintln!(
            "oneharness: warning: the prompt for harness `{}` is {} KiB and cannot be delivered \
             off the argv (no stdin/file route wired) — it stays inline and may exceed the OS \
             argument limit (E2BIG) at spawn.",
            spec.id,
            plan.base_prompt.len() / 1024
        );
    }
    let system_off_argv = use_system_file || (use_stdin && li.system_rides_prompt);
    if system_large && !system_off_argv {
        eprintln!(
            "oneharness: warning: the --system prompt for harness `{}` is {} KiB and cannot be \
             delivered off the argv (no system file flag; its system does not ride the prompt) — \
             it stays inline and may exceed the OS argument limit (E2BIG) at spawn.",
            spec.id,
            system.map_or(0, str::len) / 1024
        );
    }
    Ok(())
}

/// Drive the requested harnesses and return the report.
///
/// The whole `oneharness run` verb, minus its process: selection and validation
/// (every refusable shape is a loud [`OneharnessError`] before anything spawns),
/// job construction, the execution model the request implies, history, the
/// session store, and the assembled [`RunReport`]. A harness's own behavior is
/// never an error here — a missing binary is `skipped`, a non-zero exit is
/// `nonzero`, a hang is `timeout`, a cancellation is `cancelled` — so `Err` means
/// the *request* could not be honored, and `Ok` always carries a report.
///
/// Nothing on this path writes to the process's stdout. Warnings go to stderr.
///
/// ```no_run
/// use oneharness_core::io::run::{run, RunControls, RunRequest};
///
/// let request = RunRequest {
///     harness: vec!["claude-code".to_string()],
///     prompt: vec!["say hi".to_string()],
///     ..RunRequest::default()
/// };
/// let outcome = run(&request, RunControls::default())?;
/// println!("{}", outcome.report.results[0].text.as_deref().unwrap_or(""));
/// # Ok::<(), oneharness_core::errors::OneharnessError>(())
/// ```
pub fn run(args: &RunRequest, controls: RunControls<'_>) -> Result<RunOutcome, OneharnessError> {
    run_supervised(args, controls, None)
}

/// [`run`], with the caller's own claim on every harness child it spawns.
///
/// `supervisor` puts each one into the **process group (POSIX) or job object
/// (Windows) the caller supervises** — the grouping a consumer had for free
/// while it drove the `oneharness` binary as a subprocess, and which calling the
/// engine in-process otherwise takes away: without it an activity watchdog
/// cannot see a harness subtree as one unit (a busy member reads as idle), and a
/// kill the caller issues does not reap the tree, leaving paid harness processes
/// running. `None` is [`run`] exactly.
///
/// oneharness still owns and tears down every tree it spawns; only a `spawning`
/// hook that re-parents the child's process group moves that responsibility, and
/// then only for the part it took. [`ProcessSupervisor`] states the division and
/// where each hook sits.
///
/// It is a second entry point rather than a field on [`RunControls`] because
/// that struct is exhaustively constructible by every embedder: a field there
/// would break the literals consumers have already written, turning an
/// otherwise additive capability into a major release. Pass the same
/// [`RunControls`] through here.
///
/// ```no_run
/// use std::process::Child;
/// use oneharness_core::io::run::{run_supervised, RunControls, RunRequest};
/// use oneharness_core::io::runner::ProcessSupervisor;
///
/// struct Watchdog;
/// impl ProcessSupervisor for Watchdog {
///     fn spawned(&self, child: &Child) {
///         // The harness leads its own process group, so this pid is the pgid a
///         // watchdog can poll and a kill can reap.
///         eprintln!("harness pid {}", child.id());
///     }
/// }
///
/// let request = RunRequest::default();
/// let outcome = run_supervised(&request, RunControls::default(), Some(&Watchdog))?;
/// # let _ = outcome;
/// # Ok::<(), oneharness_core::errors::OneharnessError>(())
/// ```
pub fn run_supervised(
    args: &RunRequest,
    controls: RunControls<'_>,
    supervisor: Option<&dyn ProcessSupervisor>,
) -> Result<RunOutcome, OneharnessError> {
    let RunControls {
        events: mut event_sink,
        cancel,
        signal_cancel,
        version,
    } = controls;
    // The two side channels every spawn in this run answers to, resolved once so
    // each driver hands the runner the same pair: the token that tears a harness
    // tree down, and the caller that may also own it.
    let spawn = SpawnControls {
        cancel: &cancel,
        supervisor,
    };
    // Project config is discovered from where the harnesses will run (--cwd,
    // else the current directory): the project being operated on is the one
    // whose config should apply.
    let project_start = match &args.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    };
    let loaded = config_io::load(&args.config, args.no_config, &project_start)?;
    let cfg = &loaded.config;

    // stdin can be consumed only once total, so `--prompt-file -` and
    // `--system-file -` cannot both read it (repeated `--prompt-file -` is caught
    // in resolve_prompts). Validated before any read so it never blocks on stdin.
    if args.system_file.as_deref() == Some("-") && args.prompt_file.iter().any(|p| p == "-") {
        return Err(OneharnessError::MultipleStdinConsumers { count: 2 });
    }

    let prompts = resolve_prompts(args)?;
    // A batch run is "one harness over N prompts that share a cacheable prefix"
    // (the same --system/model). It is signalled simply by more than one prompt;
    // a single prompt keeps the ordinary "one prompt across the selected
    // harnesses" behavior.
    let batch_run = prompts.len() > 1;
    let batch_strategy = args.batch_strategy.unwrap_or(BatchStrategy::Speed);
    // The model axis. An explicit list — repeated `--model` (CLI beats config)
    // or config `models` — turns the run into a **model fan-out**: the harness ×
    // model cross-product in parallel, or the (harness, model) priority chain in
    // fallback. Absent, each harness resolves its own single model (`model_for`),
    // exactly as before. A one-element list is not a fan-out (it behaves like the
    // historical single `--model`), so `multi_model` gates on more than one.
    let explicit_models: Option<Vec<String>> = if !args.model.is_empty() {
        Some(args.model.clone())
    } else {
        cfg.models.clone()
    };
    let multi_model = explicit_models.as_ref().is_some_and(|m| m.len() > 1);
    // Structured output: an optional compiled schema and the per-harness retry
    // budget. Compiling here (once) turns an invalid schema into a loud usage
    // error before any harness is spawned.
    let schema = load_schema(args, cfg, &project_start)?;
    let max_retries = args
        .schema_max_retries
        .or(cfg.schema_max_retries)
        .unwrap_or(2);
    let server_overloaded_max_retries = args
        .server_overloaded_max_retries
        .or(cfg.server_overloaded_max_retries)
        .unwrap_or(crate::domain::config::SERVER_OVERLOADED_MAX_RETRIES_DEFAULT);
    // A CLI selection (--all / --harness) replaces the config selection
    // entirely; config `exclude` still applies unless --exclude is given.
    let (all, include) = if args.all || !args.harness.is_empty() {
        (args.all, args.harness.clone())
    } else {
        (
            cfg.all.unwrap_or(false),
            cfg.harnesses.clone().unwrap_or_default(),
        )
    };
    let exclude = if args.exclude.is_empty() {
        cfg.exclude.clone().unwrap_or_default()
    } else {
        args.exclude.clone()
    };
    for id in include.iter().chain(exclude.iter()) {
        if let Some((base, variant)) = id.split_once(':') {
            if cfg.variant_for(id).is_none() {
                return Err(OneharnessError::UnknownHarnessVariant {
                    id: id.clone(),
                    base: base.to_string(),
                    variant: variant.to_string(),
                });
            }
        }
    }
    let specs = select_specs(all, &include, &exclude)?;
    let selected_ids: Vec<String> = if all {
        specs.iter().map(|spec| spec.id.to_string()).collect()
    } else {
        let mut seen = BTreeSet::new();
        include
            .iter()
            .filter(|id| seen.insert((*id).clone()))
            .cloned()
            .collect()
    };
    // `--run-mode` (CLI beats config; an unset mode is `RunMode::default()` —
    // the one source, which `config::explain` reports from too — on every
    // surface: a library caller, the CLI and the SDKs; `parallel` is the
    // opt-in). Fallback
    // runs the selected harnesses in priority order, stopping at the first that
    // runs and falling through only harnesses that cannot run at all. It is
    // single-outcome by nature, so it refuses the multi-prompt / continuation
    // shapes up front — but the *whole candidate set* still flows through every
    // capability validator below, so a flag unsupported by ANY listed harness
    // fails fast even though only one harness will run (the command must be
    // valid for the whole set).
    let run_mode = args.run_mode.or(cfg.run_mode).unwrap_or_default();
    let run_mode_origin = if args.run_mode.is_none() && cfg.run_mode.is_none() {
        RunModeOrigin::Default
    } else {
        RunModeOrigin::Selected
    };
    // A chain is a chain only with something to fall through to. The two
    // shapes a chain cannot carry — a batch, and a `--resume`/`--fork`
    // continuation — are refused over two or more candidates, and run on a
    // one-candidate chain as the single-harness run they are (`fallback` is
    // then `null` in the report, since no chain ran). Without that, an unset
    // mode would refuse every bare single-harness continuation and batch. So
    // what the rest of this function reads is the execution decision — whether
    // THIS invocation is driven as a chain — not the selected mode alone.
    let continuation = args.resume.is_some();
    let single_candidate_batch_or_continuation = specs.len() == 1 && (batch_run || continuation);
    let drive_fallback_chain =
        run_mode == RunMode::Fallback && !single_candidate_batch_or_continuation;
    // The driver is an implementation detail; the exit code is not. A chain of
    // one that carries a batch or continuation keeps fallback's outcome rule
    // (issue #1324): when its only candidate cannot run, nothing did the task,
    // and the process says so with exit 1 exactly as the plain-prompt chain
    // does — `skipped` stays the result's own status, never a successful
    // process outcome. Only `--run-mode parallel` keeps its documented
    // tolerance of a missing harness.
    let undriven_chain_of_one =
        run_mode == RunMode::Fallback && single_candidate_batch_or_continuation;
    // Streaming is a CLI flag with a config/env layer, resolved once here so every
    // validator, the format selection, and the driver choice read the same
    // effective value.
    let stream = resolve_stream(args, cfg);
    if run_mode == RunMode::Fallback && specs.len() > 1 {
        validate_fallback(batch_run, args, run_mode_origin)?;
    }
    // `--control` is validated before the session is resolved so its own
    // vocabulary wins the diagnostic: a supervisor who passed `--control` needs
    // to be told which control rule they broke, not a session rule that happens
    // to catch the same shape first.
    let explicit_format = args.output_format.or(cfg.output_format);
    let mode = resolve_mode(args, cfg);
    let control_shape = validate_control(
        args,
        &specs,
        explicit_format,
        schema.is_some(),
        batch_run,
        multi_model,
        run_mode,
        stream,
        mode,
    )?;
    // A model fan-out multiplies the run into several (harness, model) units, so —
    // like a batch — it refuses every single-unit shape up front (loud usage
    // errors). It is *compatible* with fallback: the model list is exactly the
    // fallback chain there.
    if multi_model {
        validate_multi_model(batch_run, drive_fallback_chain, stream, args)?;
    }
    // Selection already preserves explicit caller/config order (and uses registry
    // order for `--all`). Fallback treats that sequence as its priority chain;
    // parallel schedules it concurrently while retaining the same report order.
    // A batch (multi-prompt) run is single-harness by nature — a provider cache
    // prefix is per harness/model/tools — and is a fresh fan-out, not a session
    // continuation. Refuse both before anything spawns (loud usage errors).
    if batch_run {
        validate_batch(&specs, args.resume.is_some())?;
    }
    let resume = args.resume.as_ref().map(|r| r.session.as_str());
    let fork = args.resume.as_ref().is_some_and(|r| r.fork);
    validate_resume(resume, &specs)?;
    // `--fork` (clap-guaranteed to imply `--resume`) branches a new session
    // instead of appending; refused before spawning for a harness that can't fork.
    validate_fork(fork, &specs)?;
    // `--session <name>`: resolve the uniform handle to the harness's native
    // token (via the session store) before building argv. Validates capability +
    // no-batch loudly; in parallel it is single-harness, in fallback it binds to
    // the anchor (the identity the stored record belongs to when it is still a
    // candidate, else the first session-capable one). On a continue it
    // yields the token to resume with, reusing the harness's verified `--resume`
    // mapping. `None` when the flag was not passed.
    let session_wiring = setup_session(
        args,
        &selected_ids,
        batch_run,
        drive_fallback_chain,
        &project_start,
        control_shape.is_some(),
    )?;
    let session_resume: Option<String> = session_wiring
        .as_ref()
        .and_then(|w| w.plan.resume_token.clone());
    // The variant-qualified id the session is bound to (the anchor in fallback,
    // the single harness in parallel). The resume token is applied ONLY to this
    // unit's argv below, never to a different fallback candidate that happens to
    // win — and never to a *sibling variant*, whose identity cannot resolve it.
    let session_anchor: Option<HarnessIdentity> =
        session_wiring.as_ref().map(|w| w.harness.clone());
    // An explicitly selected format keeps its authority, but a named session can
    // use it only when that harness actually emits an id in the format. Refuse a
    // lossy pin before spawning instead of accepting `--session` and silently
    // leaving the store empty. With no explicit format, the plan loop below
    // selects the anchor harness's preferred session-bearing format.
    // A mechanism that drives the turn over its own protocol captures the
    // session id on the wire, so the harness's stdout format has no bearing on
    // it — checking one here would refuse a perfectly good pairing. Read off the
    // ANCHOR's own mechanism rather than the chain's first: this is a question
    // about the identity the session is bound to, and in a chain whose
    // candidates control differently those are not the same harness.
    let anchor_control = control_shape.and_then(|_| {
        let anchor = session_anchor.as_ref()?;
        let index = selected_ids.iter().position(|id| id == anchor.as_str())?;
        specs[index].control
    });
    if !anchor_control.is_some_and(ControlShape::drives_turn) {
        validate_session_output_format(session_wiring.as_ref(), explicit_format)?;
    }
    validate_stream(
        stream,
        &specs,
        batch_run,
        schema.is_some(),
        drive_fallback_chain,
    )?;
    // Resolve the approval mode (CLI --mode > --bypass/--no-bypass > config
    // `mode` > config `bypass` > the built-in default, which is `default`). A
    // A mode a selected harness cannot express is refused here. A prompt-capable
    // mode still runs, with a safety deadline unless the caller selected zero.
    validate_modes(mode, &specs)?;
    let requested_timeout = args.timeout.or(cfg.timeout);
    let hang_prone_ids = hang_prone(mode, &specs);
    // Ordinary turns follow the caller's lifecycle when no deadline is asked
    // for. Prompt-capable headless modes are the exception: omission must not
    // turn an unattended approval prompt into a silent infinite stall. An
    // explicit zero remains the deliberate opt-out introduced in 0.6.16.
    // Validate an explicit invocation-wide timeout once. When it is omitted,
    // each job resolves its own safety deadline from its harness's mode below.
    effective_timeout(requested_timeout, TimeoutPolicy::Ordinary)?;
    // A reasoning/effort setting for a harness that has no headless argv surface
    // for it is refused here (no way to deliver it) — a loud usage error rather
    // than a silent drop, mirroring an unsupported mode.
    validate_reasoning(args, cfg, &specs)?;
    if !args.permit_prompts {
        for id in hang_prone_ids {
            let protection = if requested_timeout == Some(0) {
                "--timeout 0 disables the deadline, so this approval wait is unbounded"
            } else if requested_timeout.is_none() {
                "applying a 120s approval-wait safety deadline (pass --timeout 0 to opt out)"
            } else {
                "relying on the requested --timeout as the deadline backstop"
            };
            eprintln!(
                "oneharness: warning: `--mode {}` may block on an interactive approval prompt for \
                 harness `{id}` headlessly; {protection}. Sync allow-rules (and pass \
                 --permit-prompts to silence this), or use --mode bypass / read-only.",
                mode.as_str()
            );
        }
    }
    let config_bins: std::collections::HashMap<String, String> = cfg
        .harness
        .iter()
        .filter_map(|(id, h)| h.bin.clone().map(|bin| (id.clone(), bin)))
        .collect();
    let mut config_bins = config_bins;
    for (base, harness) in &cfg.harness {
        for name in harness.variant.keys() {
            let id = format!("{base}:{name}");
            if let Some(bin) = cfg.bin_for(&id) {
                config_bins.insert(id, bin.to_string());
            }
        }
    }
    for id in &args.mock_harness {
        if !specs.iter().any(|spec| spec.id == id) {
            return Err(OneharnessError::MockHarnessNotSelected { id: id.clone() });
        }
    }
    let mut bin_args = args.bin.clone();
    let current_exe = std::env::current_exe().map_err(OneharnessError::MockHarnessExecutable)?;
    for id in &args.mock_harness {
        bin_args.push(format!("{id}={}", current_exe.display()));
    }
    let overrides = BinOverrides::parse(&bin_args)?.with_config_bins(config_bins);
    // One-shot mock/spy wiring (`--mock-rules` / `--spy-file`): validate the
    // ruleset and every selected harness's capability loudly, then deliver the
    // hook ephemerally — on the argv where the harness supports it, else via a
    // snapshotted project-scope install restored after the run.
    let mock_wiring = setup_mock(args, &specs, &project_start, &overrides)?;
    let mut env_args = args.env.clone();
    if !args.mock_harness.is_empty() {
        env_args.push(format!("{}=1", crate::domain::app_server::MOCK_HARNESS_VAR));
    }
    let cli_env = parse_env(&env_args)?;
    // The effective top-level model for the report/history: the first fan-out
    // model when a list was given, else the single configured/CLI model. Each
    // result's own `model` (set per unit below) is authoritative on a fan-out.
    let top_model: Option<String> = explicit_models
        .as_ref()
        .and_then(|l| l.first().cloned())
        .or_else(|| cfg.model.clone());
    let model = top_model.as_deref();
    // Present on the report only for a real fan-out (more than one model), the
    // signal a consumer keys on to read each result's own `model`.
    let report_models: Option<Vec<String>> = if multi_model {
        explicit_models.clone()
    } else {
        None
    };
    // The effective system prompt comes from `--system` xor `--system-file` (the
    // argv-limit escape hatch, mirroring `--prompt-file`), then config `system`.
    let system_text: Option<String> = resolve_system(args)?.or_else(|| cfg.system.clone());
    let system = system_text.as_deref();
    let require_available =
        args.require_available || cfg.require_available.unwrap_or(false) || undriven_chain_of_one;

    // History (opt-in): --history/--no-history beats config `history`; the
    // directory is --history-dir, else config `history_dir`, else the platform
    // default. Never recorded under --print-command (nothing runs). Best-effort —
    // a store that cannot be opened warns and disables history for the run, so a
    // history problem never takes the results down (see "never panic on a
    // harness's behavior"). The absolute path is echoed in the report as the
    // programmatic handle a consumer reads the session back with.
    let history_writer = open_history_writer(args, cfg, &project_start, &prompts)?;
    let history_file = history_writer.as_ref().map(|w| {
        std::path::absolute(w.path())
            .unwrap_or_else(|_| w.path().to_path_buf())
            .display()
            .to_string()
    });

    // The (harness, model, prompt) units to run:
    //  - a **batch** run is the single selected harness against each prompt, one
    //    model (batch refuses a fan-out), so `results` is one entry per prompt;
    //  - a **model fan-out** is the harness × model cross-product (harness-major,
    //    model-minor), one prompt, so each harness repeats once per model;
    //  - an ordinary run is each selected harness against the one prompt, with its
    //    own resolved model (`model_for`) — the historical behavior.
    // In fallback mode `specs` is already in priority order, so this ordering is
    // the fallback chain.
    let units: Vec<(&'static HarnessSpec, String, Option<String>, &str)> = if batch_run {
        let m = explicit_models
            .as_ref()
            .and_then(|l| l.first().cloned())
            .or_else(|| cfg.model_for(specs[0].id).map(str::to_string));
        prompts
            .iter()
            .map(|p| (specs[0], selected_ids[0].clone(), m.clone(), p.as_str()))
            .collect()
    } else if let Some(list) = &explicit_models {
        let mut units = Vec::with_capacity(specs.len() * list.len());
        for (spec, selected_id) in specs.iter().zip(&selected_ids) {
            for m in list {
                units.push((
                    *spec,
                    selected_id.clone(),
                    Some(m.clone()),
                    prompts[0].as_str(),
                ));
            }
        }
        units
    } else {
        specs
            .iter()
            .zip(&selected_ids)
            .map(|(s, selected_id)| {
                (
                    *s,
                    selected_id.clone(),
                    cfg.model_for(selected_id).map(str::to_string),
                    prompts[0].as_str(),
                )
            })
            .collect()
    };
    // The same units as the typed identity a history record binds to and a
    // pointer line names. Selection already rejected an unknown harness or a
    // malformed variant, so every composed id here parses.
    let unit_identities: Vec<HarnessIdentity> = units
        .iter()
        .map(|(_, id, _, _)| {
            id.parse()
                .expect("harness selection validated every composed id")
        })
        .collect();

    // Build a plan entry for every unit; queue jobs only for the ones that are
    // available and actually being executed. `job_plans` parallels `jobs` and
    // retains what the structured-output retry loop needs to rebuild a unit's
    // argv with a feedback prompt.
    let mut plan: Vec<Plan> = Vec::with_capacity(units.len());
    let mut jobs: Vec<Job> = Vec::new();
    let mut job_plans: Vec<HarnessPlan> = Vec::new();
    // The assembled prompt each control-enabled candidate delivers as its first
    // stdin frame (the adapter left the positional off), by plan-entry index.
    //
    // Per candidate rather than one for the run: a chain binds the mechanism of
    // whoever serves, and the prompt is that candidate's own — a mode's
    // `instruction` is prepended per harness, so two candidates can assemble
    // different text from the same `--prompt`. `None` for an entry that never
    // built one (a skipped candidate, or any entry at all when control is off).
    let mut control_prompts: Vec<Option<String>> = Vec::with_capacity(units.len());
    // Per plan entry beside it: `Some` for a codex candidate whose turn runs on
    // the shared app-server (`app_server = "shared"`, the default), so the
    // driver leases the pooled server instead of spawning its own.
    let mut control_app_servers: Vec<Option<SharedAppServer>> = Vec::with_capacity(units.len());
    // Temp files backing off-argv system prompts, cleaned up on drop (covers every
    // return path below). Never populated under --print-command (nothing spawns).
    let mut temp_files = TempPromptFiles::default();

    for (spec, selected_id, unit_model, unit_prompt) in &units {
        let spec = *spec;
        // THIS candidate's control mechanism, not the run's: every one of them
        // can serve the turn, so each is planned for the delivery and output
        // format its own mechanism needs. `validate_control` already refused a
        // controlled selection holding a candidate that declares none.
        let unit_control = control_shape.and(spec.control);
        // The prompt this candidate would open its turn with, kept only when it
        // is planned for the control delivery (so it never appears on the argv).
        let mut unit_control_prompt: Option<String> = None;
        let unit_app_server = (unit_control == Some(control::ControlShape::CodexAppServer)
            && cfg.app_server_for(selected_id) == crate::domain::config::AppServerMode::Shared)
            .then(|| SharedAppServer {
                plugins: cfg.plugins_for(selected_id),
            });
        // On a batch run each result records the prompt it ran (they differ);
        // on an ordinary run the single top-level `prompt` covers them all.
        let result_prompt = batch_run.then(|| unit_prompt.to_string());
        let resolved = detect::resolve_named(spec, selected_id, &overrides);
        // Explicit format (CLI or config) always wins (and was validated above
        // when a named session is in play). Otherwise events/streaming selects the
        // harness's transcript-bearing format; absent that, the named-session
        // anchor selects its id-bearing format. Ordinary runs keep the default.
        let want_events = args.events || stream || history_writer.is_some();
        // Timing is a best-effort normalized signal, like usage. Harnesses that
        // expose a provider/tool boundary trace select its required format;
        // others still write history with the timing fields absent.
        let telemetry_spec = history_writer.is_some().then_some(spec.telemetry).flatten();
        let chosen_format = unit_control
            .and_then(ControlShape::required_format)
            .unwrap_or_else(|| {
                explicit_format.unwrap_or_else(|| {
                    if let Some(telemetry) = telemetry_spec {
                        telemetry.format
                    } else if want_events {
                        spec.events_format.unwrap_or(spec.output_format)
                    } else if session_anchor.as_ref().map(HarnessIdentity::as_str)
                        == Some(selected_id.as_str())
                    {
                        // A control run on a protocol-driven harness may have no
                        // session-bearing format at all: its id comes off the
                        // wire, not out of the harness's stdout document.
                        spec.session_format().unwrap_or(spec.output_format)
                    } else {
                        spec.output_format
                    }
                })
            });
        // A native-schema harness must receive its schema as JSON; force the
        // format so the conforming value lands where we read it (Claude Code's
        // `structured_output`, which needs `--output-format json`).
        let native = schema.is_some() && spec.native_schema.is_some();
        let output_format = if native {
            OutputFormat::Json
        } else {
            chosen_format
        };
        if let Some(telemetry) = telemetry_spec {
            if output_format != telemetry.format {
                return Err(OneharnessError::HistoryTelemetryFormat {
                    id: spec.id.to_string(),
                    required: telemetry.format.as_str().to_string(),
                    selected: output_format.as_str().to_string(),
                });
            }
        }
        // Reasoning / thinking effort is delivered in the harness's native shape,
        // resolved per harness so effort can sit next to each harness's own model
        // (CLI `--reasoning` beats config). Append-style deliveries (Claude's
        // `--effort`, Codex's `-c model_reasoning_effort=`, Copilot's
        // `--reasoning-effort`) lead `extra` so a raw `--` passthrough can still
        // override them; a model-suffix delivery (Cursor's `model[effort=…]`)
        // instead decorates the resolved model, so it needs one. A harness with no
        // reasoning surface was already refused by `validate_reasoning`, so
        // `spec.reasoning` is `Some` whenever a value is.
        let mut extra: Vec<String> = Vec::new();
        let mut plan_model = unit_model.clone();
        if let Some(value) = args
            .reasoning
            .as_deref()
            .or_else(|| cfg.reasoning_for(selected_id))
        {
            let delivery = spec.reasoning.expect(
                "validate_reasoning refused a reasoning value for a harness without delivery",
            );
            if let Some(suffix) = delivery.model_suffix(value) {
                match plan_model.as_mut() {
                    Some(m) => m.push_str(&suffix),
                    None => {
                        return Err(OneharnessError::ReasoningNeedsModel {
                            id: spec.id.to_string(),
                        });
                    }
                }
            } else {
                extra.extend(delivery.args(value));
            }
        }
        extra.extend(cfg.args_for(selected_id).iter().cloned());
        extra.extend(args.passthrough.iter().cloned());
        if let Some(wiring) = &mock_wiring {
            extra.extend(wiring.extra_args_for(spec.id));
        }
        let mut harness_plan = HarnessPlan {
            spec,
            bin: resolved.bin.clone(),
            // The model the argv carries: the unit's resolved model (a fan-out
            // model, the batch's single model, or the harness's own `model_for`
            // — per-harness `[harness.<id>]` beating the top-level), decorated
            // with a reasoning suffix above for a model-suffix harness (cursor).
            // The *recorded* model (in the result) stays the plain `unit_model`.
            model: plan_model,
            system: system.map(str::to_string),
            // A `--session` continue supplies the native token to resume with,
            // reusing the harness's verified `--resume` mapping; a create (or no
            // session) leaves it to the explicit `--resume` value (they are
            // mutually exclusive, so at most one is `Some`). The session token is
            // scoped to the anchor *identity*: in fallback the chain holds several
            // candidates, but a native token belongs to exactly one of them, so a
            // *different* candidate that ends up winning must never be handed it
            // (it would resume the wrong harness — or the same harness under a
            // different home directory — with a foreign id). In parallel the
            // anchor is the only unit, so this filter is a no-op there.
            resume: session_resume
                .clone()
                .filter(|_| {
                    session_anchor.as_ref().map(HarnessIdentity::as_str)
                        == Some(selected_id.as_str())
                })
                .or_else(|| resume.map(str::to_string)),
            fork,
            mode,
            output_format,
            native,
            base_prompt: unit_prompt.to_string(),
            extra,
            system_file: None,
            // Every candidate of a controlled run, not just the session anchor:
            // the one that ends up serving opens its turn over the control
            // channel, and which one that is, is decided while the chain runs.
            // A candidate planned for the argv delivery it would get without
            // `--control` could not be interrupted at all if it served.
            delivery: if unit_control.is_some() {
                PromptDelivery::ControlStream
            } else {
                PromptDelivery::Argv
            },
            plugins: cfg.plugins_for(selected_id),
            apps: cfg.apps_for(selected_id),
            analytics: cfg.analytics_for(selected_id),
            model_catalog: None,
        };
        let shared_model_catalog =
            harness::has_plugins_toggle(spec.id) && cfg.shared_model_catalog_for(selected_id);

        if args.print_command {
            // A dry run cannot ask the executable for the version the catalog
            // path is keyed by, so it names the flag with a placeholder.
            if shared_model_catalog {
                harness_plan.model_catalog =
                    Some(PathBuf::from(model_catalog::PLANNED_MODEL_CATALOG));
            }
            // --print-command never spawns, so nothing is materialized off-argv:
            // the printed command is the deterministic inline form (large prompts
            // that would actually run via file/stdin are shown inline).
            //
            // A server-submitted control run would launch the harness's SERVER,
            // never its CLI, so that is the command a dry run must show — with
            // the address left as its placeholder, because the pool picks one
            // only when it actually starts a server.
            let planned_command = unit_control
                .and_then(HttpShape::of)
                .and(spec.server)
                .map(|server| {
                    std::iter::once(resolved.bin.clone())
                        .chain(server.launch.iter().map(|arg| (*arg).to_string()))
                        .chain(server.address_args.iter().map(|arg| (*arg).to_string()))
                        .collect()
                })
                .unwrap_or_else(|| harness_plan.build(schema.as_ref(), None).argv);
            plan.push(Plan::Ready(Box::new(planned_result(
                spec,
                &resolved.bin,
                resolved.available,
                planned_command,
                output_format,
                result_prompt,
                unit_model.clone(),
            ))));
        } else if !resolved.available {
            plan.push(Plan::Ready(Box::new(skipped_result(
                spec,
                &resolved.bin,
                harness_plan.build(schema.as_ref(), None).argv,
                output_format,
                result_prompt,
                unit_model.clone(),
            ))));
        } else if let Some(identity) = variant_unprovisioned_identity(cfg, selected_id) {
            // The identity this variant selects has no home directory on disk, so
            // it holds no credentials: an `auth` failure a chain falls through,
            // exactly like the empty-directory state the harness itself reports.
            // Not spawning is the point — the harness would either refuse
            // unreadably or create the directory for an account nobody has logged
            // into.
            plan.push(Plan::Ready(Box::new(unprovisioned_result(
                spec,
                &resolved.bin,
                harness_plan.build(schema.as_ref(), None).argv,
                output_format,
                result_prompt,
                unit_model.clone(),
                &identity,
            ))));
        } else {
            let job_index = jobs.len();
            let job_timeout = effective_timeout(requested_timeout, timeout_policy(spec, mode))?;
            // Large prompt / system: deliver it off the argv (temp file / stdin)
            // where the harness supports it, so it never trips the OS argv ceiling.
            // Mutates `harness_plan` (so the structured-output retry rebuilds the
            // same delivery) and may write a temp file for the system prompt.
            plan_large_input(&mut harness_plan, spec, system, job_index, &mut temp_files)?;
            // Env layers, applied in order (the runner is last-write-wins):
            // the harness's declared defaults, then any env that delivers the
            // approval mode (Goose's GOOSE_MODE), then config ([env], then
            // [harness.<id>.env]), then the explicit `--env`, which always wins.
            let mut job_env: Vec<(String, String)> = spec
                .default_env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            if let Some(ms) = spec.mode(mode) {
                job_env.extend(ms.env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
            }
            job_env.extend(variant_environment(cfg, selected_id, &project_start)?);
            job_env.extend(cli_env.iter().cloned());
            let env_remove = cfg
                .variant_for(selected_id)
                .map_or_else(Vec::new, |variant| variant.unset_env.clone());
            if shared_model_catalog {
                let choice = model_catalog::resolve(&model_catalog::CatalogLaunch {
                    bin: &resolved.bin,
                    cwd: args.cwd.as_deref(),
                    env: &job_env,
                    env_remove: &env_remove,
                    refresh_seconds: cfg.model_catalog_refresh_seconds_for(selected_id),
                });
                if let Some(warning) = choice.warning {
                    eprintln!("oneharness: warning: {selected_id}: {warning}");
                }
                harness_plan.model_catalog = choice.catalog;
            }
            let built = harness_plan.build(schema.as_ref(), None);
            if unit_control.is_some() {
                unit_control_prompt = Some(built.prompt.clone());
            }
            jobs.push(Job {
                argv: built.argv,
                cwd: args.cwd.clone(),
                env: job_env,
                env_remove,
                timeout: job_timeout,
                stdin: built.stdin,
            });
            plan.push(Plan::Pending {
                spec,
                bin: resolved.bin,
                output_format,
                job_index,
                prompt: result_prompt,
                model: unit_model.clone(),
            });
            job_plans.push(harness_plan);
        }
        control_prompts.push(unit_control_prompt);
        control_app_servers.push(unit_app_server);
    }
    debug_assert_eq!(control_prompts.len(), plan.len());

    // Own SIGINT/SIGTERM for the spawn phase only. A harness is its own
    // process-group leader, so a signal that killed oneharness outright would
    // leave the harness (and its descendants) running and billing; instead the
    // signal cancels the in-flight runs, which tears each tree down and still
    // emits the report — with the cut-short harnesses as `status: "cancelled"`.
    // A second signal exits immediately, so an operator is never trapped.
    //
    // Installed *here*, not at the top of the verb: everything above may block on
    // stdin (`--prompt-file -`), and a handler over a restarted read would swallow
    // the interrupt instead of ending it. Skipped under `--print-command`, which
    // spawns nothing to cancel — and skipped entirely unless the caller asked for
    // it, since it takes over the host's signal disposition (an embedder cancels
    // its own `CancelToken` instead).
    if signal_cancel && !args.print_command {
        if let Err(err) = cancel::install_signal_cancel() {
            eprintln!(
                "oneharness: warning: could not install the cancellation signal handler ({err}); \
                 an interrupt will kill oneharness and may leave a harness running"
            );
        }
    }

    // Schedule and run the jobs. `--print-command` never executes, so it always
    // takes the last branch, which emits the planned rows.
    let stream_run = stream && !args.print_command;
    // Keep candidate announcements in buffered mode, but publish action events
    // only when the caller requested streaming.
    let mut filtered_sink = event_sink.take().map(|sink| StreamFilteredSink {
        sink,
        stream: stream_run,
    });
    let mut event_sink = filtered_sink
        .as_mut()
        .map(|sink| sink as &mut dyn EventSink);
    let mut forked = false;
    // Open the control socket before anything spawns, so a supervisor that races
    // the dispatch finds an address rather than a gap. The ADDRESS is all that is
    // opened here: which mechanism sits behind it is the serving candidate's, and
    // a chain has not chosen one yet — so the socket is bound unbound, and an
    // interrupt racing the first spawn is an honest `no_active_turn`.
    // `--print-command` executes nothing, so it opens nothing.
    let mut control_listener = match control_shape.filter(|_| !args.print_command) {
        Some(starts_on) => {
            let wiring = session_wiring
                .as_ref()
                .expect("validate_control refuses --control without --session");
            // Before anything spawns, and loud: an address past this platform's
            // `sun_path` budget is a run whose control channel could never have
            // existed, and a supervisor told the lever is there must not find out
            // by interrupting into nothing.
            let path = control::socket_path(&wiring.dir, &wiring.name)
                .map_err(|source| OneharnessError::ControlSocketAddress { source })?;
            Some(control_io::bind(&path, starts_on).map_err(|source| {
                OneharnessError::ControlSocket {
                    path: path.display().to_string(),
                    source,
                }
            })?)
        }
        None => None,
    };
    // Everything the sequential driver needs to bind a candidate's mechanism as
    // it takes the turn. The per-turn values (cwd, mode) are the run's; the
    // prompt, the model and the mechanism are the candidate's — the model
    // deliberately NOT the run-level `model` above, see `ControlledRun::bind`.
    let controlled = match control_listener.as_ref() {
        Some(listener) => Some(ControlledRun {
            handle: listener.handle_ref(),
            prompts: &control_prompts,
            app_servers: &control_app_servers,
            cwd: control_cwd(args)?,
            mode,
            // The same token the argv path puts on the anchor's `--resume`, on
            // the one route a driven turn has for it. `setup_session` already
            // refused a continue whose mechanism cannot ask for one, so a
            // token here always has a protocol request waiting to carry it.
            resume: session_anchor
                .clone()
                .zip(session_resume.clone())
                .map(|(anchor, token)| ControlResume { anchor, token }),
        }),
        None => None,
    };
    // A server-submitted mechanism takes this driver too. It never spawns the
    // harness CLI, so the driver submits its turn to the pooled server instead of
    // running the job — which is what lets a chain hold both kinds. Outside a
    // chain the single-turn HTTP branch below is unchanged.
    let controlled_fallback = drive_fallback_chain && controlled.is_some() && !args.print_command;
    // The third element is one entry per result, minted BEFORE its harness
    // spawns so the pointer file (if any) names the run as it begins: the
    // sequential driver mints per candidate as the chain reaches it, and each
    // buffered branch mints per plan entry before its first spawn.
    let (mut results, mut fallback_report, begun_history): (
        Vec<RunResult>,
        Option<FallbackReport>,
        Vec<BegunHistory>,
    ) = if stream_run || controlled_fallback {
        // One id per plan entry, not per selected harness: a model fan-out
        // repeats a harness once per model, so `selected_ids` is the wrong axis
        // to attribute a streamed event to.
        let streamed = drive_plan_sequentially(
            plan,
            &jobs,
            drive_fallback_chain,
            multi_model,
            history_writer.as_ref(),
            &unit_identities,
            &prompts[0],
            controlled.as_ref(),
            &mut event_sink,
            spawn,
            server_overloaded_max_retries,
        );
        // A driven turn's signals are applied per candidate, inside the driver:
        // they come off that candidate's own protocol conversation, and the next
        // candidate's binding replaces it.
        (streamed.results, streamed.fallback, streamed.history)
    } else if let Some((shape, listener)) = control_shape
        .and_then(HttpShape::of)
        .filter(|_| !args.print_command)
        .zip(control_listener.as_ref())
    {
        // The third execution model: the harness CLI is never spawned at all.
        // Its interrupt only reaches a turn the SERVER is running (driving one
        // from the CLI was live-refuted for both harnesses), so oneharness
        // submits the turn over HTTP and the socket's interrupt is one more
        // request against that same session.
        //
        // Parallel selection is exactly one harness under `--control`, so this
        // is that one candidate's turn: `run_http_controlled` binds the channel
        // to its mechanism for the turn's lifetime, exactly as a chain does.
        //
        // The empty prompt is part of the pattern, not an `expect`: a harness
        // whose binary is missing never reaches the branch that assembles one,
        // and its plan holds a `skipped` row already. Falling through publishes
        // that row — an absent CLI is data in the report, never a panic.
        let begun = begin_each_plan_entry(
            history_writer.as_ref(),
            &plan,
            &unit_identities,
            &prompts[0],
            &mut event_sink,
            &cancel,
        );
        let results = if cancel.is_cancelled() {
            unstarted_results(plan, &jobs, schema.as_ref())
        } else {
            run_http_controlled(
                shape,
                listener.handle_ref(),
                plan,
                &control_prompts
                    .first()
                    .cloned()
                    .flatten()
                    .unwrap_or_default(),
                &control_cwd(args)?,
                mode,
                effective_timeout(requested_timeout, timeout_policy(specs[0], mode))?,
            )
        };
        (results, None, begun)
    } else if let Some(chain) = controlled.as_ref().filter(|_| !jobs.is_empty()) {
        // `--schema` is refused alongside `--control` up front. Parallel
        // selection is one harness under `--control`, so the single candidate
        // binds the channel once while any zero-work overload retries repeat
        // its turn — the same binding a chain does per candidate, on a chain
        // of one.
        let shape = specs[0]
            .control
            .expect("validate_control refuses a controlled harness with no mechanism");
        let prompt = chain.prompt(0);
        let candidate_model = match &plan[0] {
            Plan::Pending { model, .. } => model.clone(),
            Plan::Ready(_) => None,
        };
        let begun = begin_each_plan_entry(
            history_writer.as_ref(),
            &plan,
            &unit_identities,
            &prompts[0],
            &mut event_sink,
            &cancel,
        );
        let mut attempts = 0;
        // A sink that stopped at the announcement leaves nothing to bind or spawn.
        let results = if cancel.is_cancelled() {
            unstarted_results(plan, &jobs, schema.as_ref())
        } else {
            loop {
                attempts += 1;
                let pooled = chain.bind(
                    shape,
                    specs[0],
                    &selected_ids[0],
                    candidate_model.as_deref(),
                    &prompt,
                    0,
                    &jobs[0],
                );
                let input = runner::ControlledInput {
                    handle: chain.handle,
                    prompt: prompt.clone(),
                    app_server: pooled.as_ref(),
                };
                let capture =
                    runner::run_job_streaming_supervised(&jobs[0], Some(&input), spawn, |_| {
                        runner::StreamStep::Continue
                    });
                let results: Vec<_> = plan
                    .iter()
                    .map(|entry| match entry {
                        Plan::Ready(result) => (**result).clone(),
                        Plan::Pending {
                            spec,
                            bin,
                            output_format,
                            job_index,
                            prompt,
                            model,
                        } => {
                            let mut result = executed_result(
                                spec,
                                bin.clone(),
                                jobs[*job_index].argv.clone(),
                                *output_format,
                                &capture,
                                schema.as_ref(),
                                attempts,
                                prompt.clone(),
                                model.clone(),
                            );
                            // Read off the conversation before the mechanism is
                            // released: a driven turn's session id and answer are
                            // knowable only from its protocol frames.
                            apply_dialogue_signals(&mut result, chain.handle);
                            result
                        }
                    })
                    .collect();
                chain.handle.release();
                let retry = attempts <= server_overloaded_max_retries
                    && results.iter().any(|result| {
                        result.failure_kind == Some(signals::FailureKind::ServerOverloaded)
                            && fallback::RunWork::from_result(result) == fallback::RunWork::None
                    });
                if !retry {
                    break results;
                }
                server_overloaded_backoff(attempts);
            }
        };
        (results, None, begun)
    } else if drive_fallback_chain && !args.print_command {
        // Sequential fallback: run the priority chain until one harness runs.
        // The workspace-restoring mock finish happens below, after every spawn
        // this branch does is complete.
        let (results, fb, begun) = run_fallback(
            plan,
            &jobs,
            &job_plans,
            schema.as_ref(),
            RetryLimits {
                schema: max_retries,
                server_overloaded: server_overloaded_max_retries,
            },
            multi_model,
            history_writer.as_ref(),
            &unit_identities,
            &prompts[0],
            &mut event_sink,
            spawn,
        );
        (results, Some(fb), begun)
    } else {
        let max_parallel = args
            .max_parallel
            .or(cfg.max_parallel)
            .unwrap_or(jobs.len().max(1));
        // Fork-based `min-tokens`: when a batch's single harness can fork, the
        // warm-up (prompt[0]) establishes a session and the fan-out branches
        // forks of it, so each reuses the warmed cached prefix — the realizable
        // token saving on these CLIs (a static --system is re-created per
        // process, so plain warm-then-fan saves nothing). It needs the warm-up's
        // *runtime* session id, so it cannot run under --print-command.
        // Codex cannot enter this branch: `server_overloaded` is a Codex-only
        // failure dialect, while only Claude Code declares `fork_reuses_cache`.
        // Codex batches therefore use `run_in_waves` below, including its
        // same-candidate overload retry policy.
        let fork_batch = batch_run
            && batch_strategy == BatchStrategy::MinTokens
            && specs[0].fork_reuses_cache
            && !args.print_command
            && !jobs.is_empty();
        // `min-tokens` reduces tokens only when the harness has a *cache-reusing*
        // fork (the warm-up writes the shared prefix, the forked fan-out reads
        // it). When it does not — no fork at all, or a fork that re-sends the
        // prefix cold, like OpenCode — `min-tokens` can only order the calls; say
        // so rather than imply a saving the harness can't deliver.
        if batch_run
            && batch_strategy == BatchStrategy::MinTokens
            && !specs[0].fork_reuses_cache
            && !args.print_command
        {
            eprintln!(
                    "oneharness: warning: `--batch-strategy min-tokens` cannot reduce tokens on `{}` \
                     (no cache-reusing fork available); it only orders the calls. Token savings need a \
                     harness whose fork reuses the prompt cache (see `fork_reuses_cache` in \
                     `oneharness list`).",
                    specs[0].id
                );
        }
        // Nothing falls through on this branch, so every plan entry is begun
        // now, before the first wave spawns. Under `--print-command` the
        // writer is `None`, so nothing is minted.
        let begun = begin_each_plan_entry(
            history_writer.as_ref(),
            &plan,
            &unit_identities,
            &prompts[0],
            &mut event_sink,
            &cancel,
        );
        let outcomes = if fork_batch {
            let o = run_fork_batch(
                &mut jobs,
                &mut job_plans,
                schema.as_ref(),
                max_retries,
                max_parallel,
                spawn,
            );
            // The fan-out actually forked iff the warm-up exposed a session to
            // branch (run_fork_batch sets the fan-out plans' `resume` only then).
            forked = job_plans.len() > 1 && job_plans[1].resume.is_some();
            o
        } else {
            let waves = if batch_run {
                batch::waves(batch_strategy, jobs.len())
            } else if jobs.is_empty() {
                Vec::new()
            } else {
                vec![(0..jobs.len()).collect()]
            };
            run_in_waves(
                &jobs,
                &job_plans,
                schema.as_ref(),
                RetryLimits {
                    schema: max_retries,
                    server_overloaded: server_overloaded_max_retries,
                },
                max_parallel,
                &waves,
                spawn,
            )
        };

        let results: Vec<RunResult> = plan
            .into_iter()
            .map(|entry| match entry {
                Plan::Ready(result) => *result,
                Plan::Pending {
                    spec,
                    bin,
                    output_format,
                    job_index,
                    prompt,
                    model,
                } => {
                    let outcome = &outcomes[job_index];
                    // The argv actually run (fork-batch rewrites the fan-out jobs
                    // to resume+fork the warmed session, so read it back).
                    let command = jobs[job_index].argv.clone();
                    executed_result(
                        spec,
                        bin,
                        command,
                        output_format,
                        &outcome.capture,
                        schema.as_ref(),
                        outcome.attempts,
                        prompt,
                        model,
                    )
                }
            })
            .collect();
        (results, None, begun)
    };
    for (result, (_, selected_id, _, _)) in results.iter_mut().zip(&units) {
        apply_result_identity(result, selected_id);
    }
    if let Some(report) = fallback_report.as_mut() {
        for (fallthrough, result) in report.fell_through.iter_mut().zip(&results) {
            fallthrough.harness = result.harness_id.clone();
        }
        if report.ran.is_some() {
            report.ran = results.last().map(|result| result.harness_id.clone());
        }
    }

    // Every job is done (or `print_command` ran none): put the workspace back
    // before anything else can fail, so a later I/O error never leaves the
    // ephemeral hook behind. Unconditional for that reason — a dry run that also
    // asked for mock wiring (which the CLI's own flags refuse) still restores.
    let mock_report = mock_wiring.map(MockWiring::finish);

    record_begun_history(&history_writer, mode, &prompts[0], &results, &begun_history);
    // Persist the captured session token (if `--session` was in play) and build
    // its report block, binding it to the candidate that actually did the turn:
    // the one the fallback chain stopped at, or — in parallel, which `--session`
    // holds to one harness — the single result.
    let session_ran = match &fallback_report {
        Some(fb) => fb
            .ran
            .as_deref()
            .and_then(|id| results.iter().find(|result| result.harness_id == id)),
        None => results.first(),
    };
    let session_report = finalize_session(session_wiring, session_ran, args.print_command);
    // Every interrupt this run served, read once the listener has stopped
    // serving (`finish`) and before it is dropped (which removes the socket).
    // Not off the live handle: the turn is over, but the request that ended it
    // may still be between delivering its abort and recording itself, and a
    // report assembled in that window would say `served` with an empty
    // `interrupts`.
    // `bind` canonicalized the socket path, so it is absolute by construction;
    // a run that somehow held a relative one has no address to publish.
    let control_report = match control_listener.as_mut() {
        Some(listener) => Some(ControlReport {
            socket: control::AbsolutePath::new(listener.path()).map_err(|message| {
                OneharnessError::ControlSocket {
                    path: listener.path().display().to_string(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidInput, message),
                }
            })?,
            mechanism: listener.handle_ref().shape(),
            interrupts: listener.finish(),
            app_server: listener.handle_ref().app_server(),
        }),
        None => None,
    };

    if let Some(dir) = &args.output_dir {
        write_output_dir(dir, &results)?;
    }

    // Fallback has its own success rule: a hard failure only when NO candidate
    // could run (nothing executed), else the outcome of the one harness that ran
    // — the fallen-through candidates never count against it. Parallel mode keeps
    // the "any harness failed" rule.
    let exit = match &fallback_report {
        Some(fb) => fallback_exit(&results, fb.ran.is_some()),
        None => exit_code(&results, require_available),
    };

    let report = build_report(
        results,
        &prompts,
        model,
        report_models,
        args,
        mode,
        schema.as_ref(),
        max_retries,
        batch_run.then_some(BatchReport {
            strategy: batch_strategy,
            prompt_count: prompts.len(),
            forked,
        }),
        fallback_report,
        loaded.files,
        mock_report,
        history_file,
        session_report,
        control_report,
        version,
    );

    Ok(RunOutcome {
        failure_summary: (exit != EXIT_OK).then(|| failure_summary(&report, require_available)),
        report,
        exit_code: exit,
        streamed: stream_run,
    })
}

/// The one-line explanation a non-zero exit owes its caller, naming what did not
/// succeed. Each run mode has its own shape: a fallback chain that never got a
/// candidate off the ground, a fallback candidate that ran and failed, or the
/// parallel count of failed harnesses.
///
/// A chain that stopped at a candidate says which of three endings it was —
/// the candidate failed at a task it is known to have done, showed nothing
/// for itself, or failed for a cause this crate could not classify — and
/// carries that candidate's own account ([`failure_account`]) in the sentence.
/// This line is what a supervisor publishes when it reports the run, and for a
/// detached run it is the only thing that reaches one: the per-result fields
/// live in the report and the process's stderr is a log nobody opens, so the
/// sentence must carry the cause itself rather than point at a report field.
fn failure_summary(report: &RunReport, require_available: bool) -> String {
    match &report.fallback {
        // Fallback where nothing could run: every candidate failed to start.
        Some(fb) if fb.ran.is_none() => {
            let chain = fb
                .fell_through
                .iter()
                .map(|f| format!("{} [{}]", f.harness, f.reason.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "oneharness: no selected harness could be run — all {} fallback candidate(s) \
                 failed to start ({chain}); nothing executed",
                fb.fell_through.len()
            )
        }
        // Fallback where a candidate ran and stopped the chain: both drivers
        // push the fallen-through results first and the one that ran last, so
        // the candidate is the last result. Named by its variant-qualified id,
        // because on a chain of identities the identity is the finding.
        Some(fb) => {
            let candidate = report.results.last();
            let ran = candidate
                .map(|r| r.harness_id.as_str())
                .or(fb.ran.as_deref())
                .unwrap_or_default();
            let account = candidate.map(failure_account).unwrap_or_default();
            let envelope = candidate.map(failure_envelope).unwrap_or_default();
            // The `stopped_without_work` reading is the report's own, read back
            // rather than re-derived; the other two are told apart by the
            // candidate's published `work` — `done` where the crate saw the
            // task done and still has no name for the failure, `null` where a
            // classifier (or the status alone) already says what happened.
            let ending = if fb.stopped_without_work {
                // A candidate that showed nothing for itself. Said in the
                // summary and not only in the report, because this sentence is
                // what a supervisor quotes: "ran but did not succeed" is the
                // reading that made a five-identity chain look like a
                // candidate's own task failure and hid the untried rest.
                "failed with nothing to show for it — no tool call, no billed usage, and no \
                 cause it could classify"
            } else if candidate.is_some_and(|r| r.work == Some(fallback::RunWork::Done)) {
                // A candidate the crate saw do the task's work, whose failure
                // it could not name: the stop is the right verdict (the work
                // was billed), and the cause is whatever the harness said.
                "did the task's work and did not succeed, for a cause it could not classify"
            } else {
                // A candidate that failed at its task with a named cause, or a
                // status that is its own explanation.
                "ran but did not succeed"
            };
            format!(
                "oneharness: fallback harness `{ran}` {ending} ({envelope}) — so the chain \
                 stopped there and tried no candidate after it; it said: {account}"
            )
        }
        // Nothing executed because no selected harness is installed: the same
        // sentence the chain above says, since to the supervisor quoting it a
        // one-candidate batch or continuation IS that chain (issue #1324).
        None if require_available
            && !report.results.is_empty()
            && report
                .results
                .iter()
                .all(|r| r.status == Status::Skipped && !r.available) =>
        {
            let mut missing: Vec<&str> = Vec::new();
            for result in &report.results {
                if !missing.contains(&result.harness_id.as_str()) {
                    missing.push(&result.harness_id);
                }
            }
            format!(
                "oneharness: no selected harness could be run — not installed: {}; nothing \
                 executed",
                missing.join(", ")
            )
        }
        None => {
            let failed = report
                .results
                .iter()
                .filter(|r| {
                    is_failure(
                        r.status,
                        r.available,
                        require_available,
                        r.schema_valid,
                        r.failure_kind,
                    )
                })
                .count();
            format!(
                "oneharness: {failed}/{} harness run(s) did not succeed (see results[].status and results[].error)",
                report.results.len()
            )
        }
    }
}

/// The status half of a stopped candidate's one-line account: the report's own
/// tokens (`status`, `exit_code`, `failure_kind`), so a reader can find the same
/// values in the JSON beside the sentence.
fn failure_envelope(result: &RunResult) -> String {
    let mut parts = vec![format!("status {}", result.status.as_str())];
    if let Some(code) = result.exit_code {
        parts.push(format!("exit {code}"));
    }
    if let Some(kind) = result.failure_kind {
        parts.push(format!("failure_kind {}", kind.as_str()));
    }
    parts.join(", ")
}

/// Longest account a summary carries of what a candidate said; past this the
/// line stops being one a supervisor can publish, and the report has the rest.
const FAILURE_ACCOUNT_CHARS: usize = 400;

/// A stopped candidate's **own** account of its failure, for the summary line:
/// the result's `error` where oneharness composed one (a deadline, a
/// cancellation, a refusal the provider stated); else the schema verdict on a
/// run whose answer did not conform; else the last thing the harness wrote to
/// stderr, then to stdout — a plain non-zero exit composes no `error` at all,
/// so the harness's last words are the only cause there is. Never empty: a
/// candidate that died silently says so, because "nothing" is itself the
/// finding a reader needs. Folded onto one line, stripped of control
/// characters (harness output is untrusted, and a terminal escape has no place
/// in a line a supervisor publishes), and bounded at [`FAILURE_ACCOUNT_CHARS`].
///
/// The last words are a **tail of lines**, not the last line: a diagnostic is
/// written after the warnings that preceded it, so the tail is the right
/// region, but its final line is the least of it — a Node crash ends in a
/// version footer, a shell wrapper in the exit line, a Python traceback in the
/// exception with its frames above. So [`last_words`] keeps as many trailing
/// non-blank lines as fit the bound, whole when the diagnostic does, and a
/// leading `…` says where earlier lines were dropped.
fn failure_account(result: &RunResult) -> String {
    let account = result
        .error
        .clone()
        .or_else(|| {
            result
                .schema_error
                .as_ref()
                .map(|why| format!("the answer did not conform to the schema: {why}"))
        })
        .or_else(|| last_words(&result.stderr, "(stderr) "))
        .or_else(|| last_words(&result.stdout, "(stdout) "))
        .unwrap_or_else(|| "nothing — it wrote no error, no stderr and no stdout".to_string());
    let folded = account
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    if folded.chars().count() <= FAILURE_ACCOUNT_CHARS {
        return folded;
    }
    let mut cut = folded
        .chars()
        .take(FAILURE_ACCOUNT_CHARS)
        .collect::<String>();
    cut.push('…');
    cut
}

/// The trailing non-blank lines of a captured stream that fit
/// [`FAILURE_ACCOUNT_CHARS`] behind `tag`, joined on one line — at least the
/// final line, however long (the caller's bound then cuts it). `None` for a
/// stream with no words at all. A leading `…` marks lines dropped before the
/// kept ones, so a reader knows the diagnostic began earlier than the line
/// shows and the report has it whole.
fn last_words(captured: &str, tag: &str) -> Option<String> {
    let budget = FAILURE_ACCOUNT_CHARS.saturating_sub(tag.chars().count());
    let mut kept: Vec<&str> = Vec::new();
    let mut width = 0;
    let mut dropped = false;
    for line in captured
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        // One separator per join, plus the marker a drop would add.
        let cost = line.chars().count() + usize::from(!kept.is_empty());
        if !kept.is_empty() && width + cost + 1 > budget {
            dropped = true;
            break;
        }
        width += cost;
        kept.push(line);
    }
    if kept.is_empty() {
        return None;
    }
    kept.reverse();
    let marker = if dropped { "…" } else { "" };
    Some(format!("{tag}{marker}{}", kept.join(" ")))
}

fn apply_result_identity(result: &mut RunResult, composed: &str) {
    // llmlint: ignore[invalid_states_unrepresentable] This private helper is called only with ids returned by select_specs after successful config variant lookup; retaining &str avoids introducing a competing identity representation behind that validation boundary.
    result.harness_id = composed.to_string();
    result.variant = composed
        .split_once(':')
        .map(|(_, variant)| variant.to_string());
}

/// What `--mock-rules`/`--spy-file` wired up before spawning, and how to undo
/// it. Built by [`setup_mock`]; consumed by [`MockWiring::finish`] the moment
/// every job is done — the restore must run on the same code path as the run
/// itself, so a later failure (report I/O) can never leave the ephemeral hook
/// installed in the workspace.
struct MockWiring {
    /// Per-harness argv additions: Claude Code's `--settings <tempfile>`,
    /// Codex's hook-engine opt-in flags.
    extra_args: std::collections::HashMap<&'static str, Vec<String>>,
    /// Byte snapshots of every project config file the installs touched
    /// (existing files restored verbatim, created ones deleted).
    snapshot: HookSnapshot,
    /// Temp settings files to delete afterwards (the `SettingsFlag` delivery).
    temp_files: Vec<std::path::PathBuf>,
    /// What the report records.
    rules: Option<serde_json::Value>,
    spy_file: Option<String>,
}

/// The report-facing remainder of a finished [`MockWiring`].
struct MockReport {
    rules: Option<serde_json::Value>,
    spy_file: Option<String>,
}

impl MockWiring {
    fn extra_args_for(&self, id: &str) -> Vec<String> {
        self.extra_args.get(id).cloned().unwrap_or_default()
    }

    /// Undo everything: restore the snapshotted config files and delete the
    /// temp settings files. Best-effort with a stderr warning per failure — a
    /// restore problem must never take the run's results down with it.
    fn finish(self) -> MockReport {
        for (path, err) in self.snapshot.restore() {
            eprintln!(
                "oneharness: warning: could not restore `{}` after the mocked run: {err}",
                path.display()
            );
        }
        for path in &self.temp_files {
            if let Err(err) = std::fs::remove_file(path) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "oneharness: warning: could not remove temp settings `{}`: {err}",
                        path.display()
                    );
                }
            }
        }
        MockReport {
            rules: self.rules,
            spy_file: self.spy_file,
        }
    }
}

/// Validate and deliver the one-shot mock hook for every selected harness.
/// `None` when neither `--mock-rules` nor `--spy-file` was given. Everything
/// refusable is refused here, before any file is touched or process spawned:
/// a harness with no one-shot delivery (qwen, copilot), a rule action a
/// harness cannot express, an unreadable/invalid ruleset, or a path that
/// cannot be embedded into a hook command. Only then are the hooks delivered —
/// on the argv where possible, else installed at project scope in the working
/// directory (layering onto existing config via the non-destructive merge)
/// with every touched file snapshotted for [`MockWiring::finish`].
fn setup_mock(
    args: &RunRequest,
    specs: &[&'static HarnessSpec],
    project_dir: &std::path::Path,
    overrides: &BinOverrides,
) -> Result<Option<MockWiring>, OneharnessError> {
    if args.mock_rules.is_none() && args.spy_file.is_none() {
        return Ok(None);
    }

    // Parse + validate the ruleset (loud), and keep the raw value for the report.
    let mut rules_value = None;
    let mut parsed_rules = None;
    if let Some(path) = &args.mock_rules {
        let text =
            std::fs::read_to_string(path).map_err(|source| OneharnessError::MockRulesFile {
                path: path.display().to_string(),
                source,
            })?;
        let rules =
            mock::parse_rules(&text).map_err(|message| OneharnessError::MockRulesInvalid {
                path: path.display().to_string(),
                message,
            })?;
        rules_value = serde_json::from_str(&text).ok();
        parsed_rules = Some(rules);
    }

    // Every selected harness must be able to take the hook AND express every
    // rule action — refused before anything is delivered anywhere.
    for spec in specs {
        if spec.mock_delivery.is_none() {
            return Err(OneharnessError::MockDeliveryUnsupported {
                id: spec.id.to_string(),
                reason: match spec.id {
                    "qwen" => {
                        "its hooks fire only at user scope headlessly (project hooks sit behind \
                         folder trust) — sync a [[hooks]] mock with --global into a redirected \
                         HOME instead (see the README)"
                    }
                    _ => {
                        "its hooks never fire in a headless run (probe-refuted), so no delivery \
                          could make the CLI honor them"
                    }
                },
            });
        }
        if let Some(rules) = &parsed_rules {
            if let Some(action) = mock::unsupported_action(rules, spec.gate_deny, spec.mock_rewrite)
            {
                return Err(OneharnessError::MockActionUnsupported {
                    id: spec.id.to_string(),
                    action,
                });
            }
        }
    }

    // The hook command embeds this binary and the (absolutized) paths — the
    // hook runs from the harness's own cwd, and some harnesses tokenize the
    // command on whitespace, so space-bearing paths are refused loudly.
    let exe = std::env::current_exe().map_err(|err| OneharnessError::MockSetup {
        message: format!("could not resolve the oneharness binary path: {err}"),
    })?;
    let rules_abs = args
        .mock_rules
        .as_deref()
        .map(std::path::absolute)
        .transpose()
        .map_err(|err| OneharnessError::MockSetup {
            message: format!("could not absolutize --mock-rules: {err}"),
        })?;
    let spy_abs = args
        .spy_file
        .as_deref()
        .map(std::path::absolute)
        .transpose()
        .map_err(|err| OneharnessError::MockSetup {
            message: format!("could not absolutize --spy-file: {err}"),
        })?;
    let embed = |p: &std::path::Path| -> Result<String, OneharnessError> {
        // Forward slashes work everywhere Windows paths are consumed here, and
        // keep the string safe for JSON/TOML/shim embedding.
        let text = p.display().to_string().replace('\\', "/");
        if text.chars().any(char::is_whitespace) {
            return Err(OneharnessError::MockPathWhitespace { path: text });
        }
        Ok(text)
    };
    let exe = embed(&exe)?;
    let rules_str = rules_abs.as_deref().map(embed).transpose()?;
    let spy_str = spy_abs.as_deref().map(embed).transpose()?;

    let mut wiring = MockWiring {
        extra_args: std::collections::HashMap::new(),
        snapshot: HookSnapshot::default(),
        temp_files: Vec::new(),
        rules: rules_value,
        spy_file: spy_str.clone(),
    };

    for spec in specs {
        // A missing binary is a skipped result; do not touch the workspace for it.
        if !detect::resolve(spec, overrides).available {
            continue;
        }
        let command = mock::hook_command(&exe, spec.id, rules_str.as_deref(), spy_str.as_deref());
        match spec.mock_delivery.expect("validated above") {
            MockDelivery::SettingsFlag { flag } => {
                let path = std::env::temp_dir().join(format!(
                    "oneharness-mock-{}-{}.json",
                    spec.id,
                    std::process::id()
                ));
                std::fs::write(&path, mock::settings_hooks_json(&command)).map_err(|err| {
                    OneharnessError::MockSetup {
                        message: format!(
                            "could not write temp settings `{}`: {err}",
                            path.display()
                        ),
                    }
                })?;
                let path_str = embed(&path)?;
                wiring.temp_files.push(path);
                wiring
                    .extra_args
                    .insert(spec.id, vec![flag.to_string(), path_str]);
            }
            MockDelivery::ProjectHooks { extra_args } => {
                let hook = crate::domain::hooks::HookSpec {
                    plugin_name: Some("oneharness-mock".into()),
                    ..crate::domain::hooks::HookSpec::command(&command)
                };
                // Plan first (check mode) to learn which files the install will
                // touch, snapshot exactly those, then install for real. Captured
                // per spec so one harness's install is never re-captured as
                // another's pre-existing state.
                let planned = hooks_io::install(Scope::Project(project_dir), spec, &hook, true)?;
                let paths: Vec<std::path::PathBuf> = planned.into_iter().map(|w| w.path).collect();
                wiring.snapshot.extend(HookSnapshot::capture(&paths));
                hooks_io::install(Scope::Project(project_dir), spec, &hook, false)?;
                if !extra_args.is_empty() {
                    wiring
                        .extra_args
                        .insert(spec.id, extra_args.iter().map(|a| a.to_string()).collect());
                }
            }
        }
    }
    Ok(Some(wiring))
}

/// Assemble the top-level [`RunReport`] from the finished results and the shared
/// run metadata. Extracted so the normal and streaming paths emit an identical
/// envelope shape (the streaming path passes `batch: None`).
// llmlint: ignore[suppressions_justified] The allow is justified here: this
// assembles the report's own top-level fields, so the parameter list IS the
// contract's shape — bundling it into a struct would create a second definition
// of `RunReport` that has to be kept in step with the first.
#[allow(clippy::too_many_arguments)]
fn build_report(
    results: Vec<RunResult>,
    prompts: &[String],
    model: Option<&str>,
    models: Option<Vec<String>>,
    args: &RunRequest,
    mode: PermissionMode,
    schema: Option<&Schema>,
    max_retries: u32,
    batch: Option<BatchReport>,
    fallback: Option<FallbackReport>,
    config_files: Vec<String>,
    mock: Option<MockReport>,
    history_file: Option<String>,
    session: Option<SessionReport>,
    control: Option<ControlReport>,
    version: Option<String>,
) -> RunReport {
    RunReport {
        schema_version: SCHEMA_VERSION.to_string(),
        // The caller's own version when it has one (the CLI names the shipped
        // binary), else this engine's.
        oneharness_version: version.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
        // On a batch run the per-result `prompt` is authoritative; the top-level
        // field repeats the first prompt for back-compat (it is always present).
        prompt: prompts[0].clone(),
        // The effective top-level model (first fan-out model, else the single
        // CLI/config model); each result's own `model` is authoritative.
        model: model.map(str::to_string),
        // The model fan-out list, present only on a multi-model run.
        models,
        resume: args.resume.as_ref().map(|r| r.session.clone()),
        fork: args.resume.as_ref().is_some_and(|r| r.fork),
        session,
        permission_mode: mode,
        bypass_permissions: mode.is_bypass(),
        dry_run: args.print_command,
        schema: schema.map(|s| s.as_value().clone()),
        schema_max_retries: schema.map(|_| max_retries),
        batch,
        fallback,
        mock_rules: mock.as_ref().and_then(|m| m.rules.clone()),
        spy_file: mock.and_then(|m| m.spy_file),
        history_file,
        config_files,
        control,
        results,
    }
}

/// The resolved `--session` context, carried from validation to finalization:
/// which named session, on which harness/project, where its store file lives,
/// what it already held, and the create-vs-continue plan.
struct SessionWiring {
    name: String,
    /// The anchor's **variant-qualified** identity (`claude-code:alternate`) —
    /// the one candidate whose argv carries the stored token, because that token
    /// exists only in the session namespace of the identity that minted it. Its
    /// registry entry (for the session-bearing output format) comes from
    /// [`HarnessIdentity::spec`] rather than a second field that could disagree.
    harness: HarnessIdentity,
    project: PathBuf,
    /// The resolved session store directory — also the anchor for the run's
    /// `control/<name>.sock`, so both addresses come from one resolution.
    dir: PathBuf,
    path: PathBuf,
    existing: Option<SessionRecord>,
    plan: SessionPlan,
}

/// The comma-joined ids of every session-capable harness, for the "supported:"
/// hint on a `--session` capability error.
fn session_capable_ids() -> String {
    harness::all()
        .iter()
        .filter(|s| s.session_capable())
        .map(|s| s.id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The comma-joined ids of every harness whose control mechanism can continue a
/// named session, for the "Mechanisms that continue a session under --control:"
/// hint on a [`OneharnessError::SessionControlNoResume`].
fn control_session_capable_ids() -> String {
    let ids: Vec<&str> = harness::all()
        .iter()
        .filter(|spec| spec.control.is_some_and(ControlShape::carries_session))
        .map(|spec| spec.id)
        .collect();
    if ids.is_empty() {
        "none".to_string()
    } else {
        ids.join(", ")
    }
}

/// Validate and resolve a `--session <name>` request against the store, or
/// `Ok(None)` when the flag was not passed. Loud usage errors up front (nothing
/// spawns): a batch run, a harness that exposes no session id (`session_capable`),
/// an unresolvable store directory, or a name already bound to a different harness.
/// On success the returned plan says whether to create fresh or continue a stored
/// token.
///
/// Which harness the session binds to depends on the run mode:
///
/// * **Parallel** — the run is single-harness by contract, so more than one
///   selected harness makes a single session name ambiguous
///   ([`OneharnessError::SessionMultipleHarnesses`]).
/// * **Fallback** — the whole priority chain is selected, but exactly ONE harness
///   ultimately runs (fallback stops at the first that runs), so `--session` is
///   allowed. It binds to the *anchor*: the candidate the stored record already
///   belongs to when that candidate is still in the chain, else the first
///   session-capable one. Preferring the record's own identity is what keeps a
///   session that moved (see [`finalize_session`]) continuing where it lives
///   instead of conflicting with the head of the chain. A chain with no
///   session-capable harness at all cannot carry a named handle
///   ([`OneharnessError::SessionUnsupported`]).
///
/// The anchor is a **variant-qualified** id, because that is the granularity a
/// native token actually belongs to: each variant points its harness at its own
/// home directory, so `claude-code:alternate`'s session store knows nothing of
/// `claude-code:primary`'s ids. The token is applied to the anchor's argv *only*
/// (see the job loop's `session_anchor` filter), so a different candidate that
/// ends up winning is never handed a token its identity cannot resolve.
fn setup_session(
    args: &RunRequest,
    selected_ids: &[String],
    batch_run: bool,
    fallback_mode: bool,
    project: &std::path::Path,
    control: bool,
) -> Result<Option<SessionWiring>, OneharnessError> {
    let Some(name) = args.session.as_deref() else {
        return Ok(None);
    };
    if batch_run {
        return Err(OneharnessError::SessionBatch);
    }
    // A `--session-dir` that cannot be spelled as UTF-8 is refused rather than
    // dropped, exactly as the `interrupt` verb refuses it: silently falling
    // back to the default store would put the session handle somewhere the
    // caller did not ask for, and leave the interrupt looking for it there.
    let configured = match args.session_dir.as_deref() {
        Some(path) => Some(
            path.to_str()
                .ok_or_else(|| OneharnessError::SessionDirInvalid {
                    path: path.display().to_string(),
                })?,
        ),
        None => None,
    };
    let dir = session_io::resolve_dir(configured).ok_or(OneharnessError::SessionNoStore)?;
    let path = session_io::session_path(&dir, project, name);
    // A record this build cannot resume is no record at all: the run creates a
    // fresh session rather than guessing which identity minted a legacy token.
    let existing = session::resumable(session_io::read(&path));
    // Selection already rejected an unknown harness or a malformed variant, so
    // every composed id here parses; the typed identity is what the record binds
    // to and what the argv filter matches on.
    let candidates: Vec<HarnessIdentity> = selected_ids
        .iter()
        .map(|id| {
            id.parse()
                .expect("harness selection validated every composed id")
        })
        .collect();
    let id = if fallback_mode {
        // Continue where the session already lives when that identity is still a
        // candidate; otherwise bind to the first session-capable one in priority
        // order. A chain with none cannot carry a named handle (list the whole
        // selection in the error, since no single harness is the offender).
        candidates
            .iter()
            .find(|id| {
                id.spec().session_capable_under(control)
                    && existing.as_ref().is_some_and(|r| &r.harness == *id)
            })
            .or_else(|| {
                candidates
                    .iter()
                    .find(|id| id.spec().session_capable_under(control))
            })
            .ok_or_else(|| OneharnessError::SessionUnsupported {
                id: selected_ids.join(", "),
                supported: session_capable_ids(),
            })?
            .clone()
    } else {
        if candidates.len() != 1 {
            return Err(OneharnessError::SessionMultipleHarnesses {
                count: candidates.len(),
                selected: selected_ids.join(", "),
            });
        }
        let id = candidates[0].clone();
        if !id.spec().session_capable_under(control) {
            return Err(OneharnessError::SessionUnsupported {
                id: id.to_string(),
                supported: session_capable_ids(),
            });
        }
        id
    };
    if let Some(was) = session::harness_conflict(existing.as_ref(), &id) {
        return Err(OneharnessError::SessionHarnessConflict {
            name: name.to_string(),
            was: was.to_string(),
            now: id.to_string(),
        });
    }
    let plan = SessionPlan::decide(existing.as_ref());
    // The handle and the mechanism have to agree about what a turn is. A driven
    // turn negotiates prompt, model, cwd and approvals on the wire and builds no
    // argv at all, so the harness's verified `--resume` mapping is never reached
    // and the protocol's own resume request is the ONLY way one conversation
    // continues. Without one, a continue would open a new conversation and then
    // overwrite the stored token with its id — the flag accepted, the store
    // healthy, the report normal, and every earlier turn gone.
    //
    // A *create* is honest on any mechanism (a new conversation is what was
    // asked for), so it runs — and says, once, that this handle will not
    // continue, rather than leaving the next turn to discover it.
    if control {
        if let Some(shape) = id.spec().control.filter(|s| !s.carries_session()) {
            if plan.phase == session::SessionPhase::Continue {
                return Err(OneharnessError::SessionControlNoResume {
                    name: name.to_string(),
                    id: id.to_string(),
                    mechanism: shape.as_str(),
                    supported: control_session_capable_ids(),
                });
            }
            eprintln!(
                "oneharness: warning: session `{name}` starts a NEW conversation on `{id}`, and \
                 its control mechanism `{}` implements no resume request — so the next \
                 `--control --session {name}` turn will be refused rather than silently starting \
                 over. Mechanisms that continue a session under --control: {}",
                shape.as_str(),
                control_session_capable_ids()
            );
        }
    }
    Ok(Some(SessionWiring {
        name: name.to_string(),
        harness: id,
        project: project.to_path_buf(),
        dir,
        path,
        existing,
        plan,
    }))
}

/// Refuse a caller-pinned output format that cannot carry the named session's
/// native id. `None` means the normal automatic selection remains in force.
/// Only the session anchor is checked: in fallback mode the other candidates do
/// not own this named session and must retain their ordinary format behavior.
fn validate_session_output_format(
    wiring: Option<&SessionWiring>,
    explicit_format: Option<OutputFormat>,
) -> Result<(), OneharnessError> {
    let (Some(wiring), Some(format)) = (wiring, explicit_format) else {
        return Ok(());
    };
    let (spec, id) = (wiring.harness.spec(), wiring.harness.as_str());
    if spec.format_carries_session(format) {
        return Ok(());
    }
    Err(OneharnessError::SessionOutputFormat {
        id: id.to_string(),
        format: format.as_str().to_string(),
        supported: spec
            .session_formats
            .iter()
            .map(|format| format.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    })
}

/// Persist the session token this run captured (best-effort) and build the
/// report block. On a create the token is the result's extracted `session_id`;
/// on a continue it is re-affirmed (a harness may rotate it). Under
/// `--print-command` nothing ran, so nothing is written and the block echoes the
/// stored token (or null on a fresh create). A create run that exposed no session
/// id warns — the handle cannot be continued — rather than storing an empty one.
///
/// `ran` is the result of the candidate that **did the turn**, which is what the
/// record must bind to. In parallel single-harness mode that is the only result.
/// In fallback it is the one the chain stopped at — *not* necessarily the anchor:
/// when the anchor falls through (its identity is out of quota, or the stored
/// token predates it), a later candidate does the work and exposes a token in its
/// own session namespace. Storing the anchor's id beside that token, or picking
/// the first result that shares the anchor's *base* id (a fell-through variant),
/// is exactly the "wrong token" this function exists to rule out.
fn finalize_session(
    wiring: Option<SessionWiring>,
    ran: Option<&RunResult>,
    dry_run: bool,
) -> Option<SessionReport> {
    let wiring = wiring?;
    let captured = ran.and_then(|r| r.session_id.clone());
    // The identity the record binds to: whoever ran. With nothing run there is no
    // new token either, so the anchor's binding stands unchanged.
    let bound = ran.map_or_else(
        || wiring.harness.clone(),
        |r| {
            r.harness_id
                .parse()
                .expect("a result's identity came from the validated selection")
        },
    );
    if !dry_run {
        match &captured {
            Some(token) => {
                if bound != wiring.harness {
                    eprintln!(
                        "oneharness: warning: session `{}` was bound to `{}`, but `{bound}` ran \
                         this turn, so the handle now continues on `{bound}` (a native session \
                         token is not portable between identities)",
                        wiring.name, wiring.harness
                    );
                }
                if let Err(err) = session_io::write(
                    &wiring.path,
                    &wiring.project,
                    &bound,
                    &wiring.name,
                    token,
                    wiring
                        .existing
                        .as_ref()
                        .filter(|record| record.harness == bound),
                ) {
                    eprintln!(
                        "oneharness: warning: could not write session store `{}`: {err}",
                        wiring.path.display()
                    );
                }
            }
            None => eprintln!(
                "oneharness: warning: harness `{}` exposed no session id, so `--session {}` \
                 cannot be continued (nothing was stored)",
                bound, wiring.name
            ),
        }
    }
    // Report the fresh capture if any, else the token we resumed with.
    let token = captured.or_else(|| wiring.plan.resume_token.clone());
    // The planned phase describes the *anchor*. A candidate that is not the anchor
    // never received the stored token (the job loop's `session_anchor` filter), so
    // it started a new conversation whatever the plan intended — reporting
    // `continue` there would claim a continuation nothing performed.
    let phase = if bound == wiring.harness {
        wiring.plan.phase
    } else {
        session::SessionPhase::Create
    };
    let store_file = std::path::absolute(&wiring.path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| wiring.path.display().to_string());
    Some(SessionReport {
        // Echo the sanitized handle — exactly what the store keyed the file on.
        name: crate::domain::history::sanitize_name(&wiring.name),
        phase,
        token,
        store_file: Some(store_file),
    })
}

/// Open the history session writer for this run, or `None` when history is off,
/// under `--print-command`, or the store cannot be opened. Best-effort: every
/// failure warns on stderr and disables history rather than aborting the run.
fn open_history_writer(
    args: &RunRequest,
    cfg: &crate::domain::config::FileConfig,
    project_start: &std::path::Path,
    prompts: &[String],
) -> Result<Option<HistoryWriter>, OneharnessError> {
    let cli_labels =
        crate::domain::history::parse_labels(args.history_label.iter().map(String::as_str))
            .map_err(OneharnessError::HistoryLabelInvalid)?;
    let mut labels = cfg.history_labels.clone().unwrap_or_default();
    labels.extend(&cli_labels);
    if args.print_command {
        return Ok(None);
    }
    let enabled = args.history.unwrap_or_else(|| cfg.history.unwrap_or(false));
    if !enabled {
        return Ok(None);
    }
    let configured = args
        .history_dir
        .as_deref()
        .map(|p| p.display().to_string())
        .or_else(|| cfg.history_dir.clone());
    let Some(dir) = history::resolve_dir(configured.as_deref()) else {
        eprintln!(
            "oneharness: warning: history is enabled but no history directory could be resolved \
             (pass --history-dir, set `history_dir`, or ONEHARNESS_HISTORY_DIR); \
             skipping history for this run"
        );
        return Ok(None);
    };
    let name = args.history_name.clone().unwrap_or_else(|| {
        crate::domain::history::session_name(prompts.first().map(String::as_str).unwrap_or(""))
    });
    // The pointer file layers like the directory: --history-pointer-file, else
    // config `history_pointer_file` (which `from_env` already fed from
    // ONEHARNESS_HISTORY_POINTER_FILE); an empty value is unset.
    let pointer_file = args
        .history_pointer_file
        .clone()
        .or_else(|| cfg.history_pointer_file.as_deref().map(PathBuf::from))
        .filter(|path| !path.as_os_str().is_empty());
    match HistoryWriter::open(&dir, project_start, &name, labels) {
        Ok(writer) => Ok(Some(writer.with_pointer_file(pointer_file))),
        Err(err) => {
            eprintln!(
                "oneharness: warning: could not open a history file under `{}`: {err}; \
                 skipping history for this run",
                dir.display()
            );
            Ok(None)
        }
    }
}

/// One result's begun history: `run_id` is `None` when history is off, and the
/// indexes are the events a streamed run already appended live, which the
/// closing record must not write again (empty on the buffered path).
struct BegunHistory {
    run_id: Option<crate::domain::history::HistoryId>,
    persisted_event_indexes: BTreeSet<usize>,
}

/// The rows of a plan whose candidates were announced but never started,
/// because the caller's sink stopped the run at its announcement.
fn unstarted_results(plan: Vec<Plan>, jobs: &[Job], schema: Option<&Schema>) -> Vec<RunResult> {
    plan.into_iter()
        .map(|entry| match entry {
            Plan::Ready(result) => *result,
            Plan::Pending {
                spec,
                bin,
                output_format,
                job_index,
                prompt,
                model,
            } => {
                let job = &jobs[job_index];
                executed_result(
                    spec,
                    bin,
                    job.argv.clone(),
                    output_format,
                    &runner::unstarted_capture(job),
                    schema,
                    0,
                    prompt,
                    model,
                )
            }
        })
        .collect()
}

fn begin_candidate(
    writer: &HistoryWriter,
    identity: &HarnessIdentity,
    prompt: &str,
    sink: &mut Option<&mut dyn EventSink>,
    cancel: &CancelToken,
) -> crate::domain::history::HistoryId {
    let run_id = writer.begin_harness_run(identity, prompt);
    if sink
        .as_mut()
        .is_some_and(|sink| sink.run_began(&writer.run_begun(identity, run_id)) == SinkStep::Stop)
    {
        cancel.cancel();
    }
    run_id
}

/// Begin every entry of a plan a buffered branch runs in full: one id per plan
/// entry, minted now — and its pointer line written now — before any of them
/// spawns. Every entry, not only the pending ones: a row the plan already
/// resolved (a harness whose binary is missing) closes as its own `skipped`
/// record exactly as it did before pointers existed, and a record no pointer
/// line names cannot be found from the file. `unit_ids` is index-aligned with
/// the plan, so entry `i`'s identity is `unit_ids[i]`.
fn begin_each_plan_entry(
    writer: Option<&HistoryWriter>,
    plan: &[Plan],
    unit_ids: &[HarnessIdentity],
    default_prompt: &str,
    sink: &mut Option<&mut dyn EventSink>,
    cancel: &CancelToken,
) -> Vec<BegunHistory> {
    (0..plan.len())
        .map(|index| BegunHistory {
            run_id: writer.map(|writer| {
                begin_candidate(
                    writer,
                    &unit_ids[index],
                    plan[index].prompt(default_prompt),
                    sink,
                    cancel,
                )
            }),
            persisted_event_indexes: BTreeSet::new(),
        })
        .collect()
}

/// Close each result's history record under the id its run was begun with, if
/// history is on. Best-effort per record: a write failure warns and moves on
/// (the run's stdout report is authoritative; history is a side channel). Each
/// record carries the result's own `model`, so a model fan-out records the
/// model each harness ran. A result with no begun id (a branch that minted
/// none) is not recorded — there is no id a pointer line could have named.
fn record_begun_history(
    writer: &Option<HistoryWriter>,
    mode: PermissionMode,
    run_prompt: &str,
    results: &[RunResult],
    begun: &[BegunHistory],
) {
    let Some(writer) = writer else { return };
    debug_assert_eq!(
        results.len(),
        begun.len(),
        "every result was begun before it ran"
    );
    for (result, begun) in results.iter().zip(begun) {
        let Some(run_id) = begun.run_id else {
            continue;
        };
        if let Err(err) = writer.append_streamed(
            run_id,
            mode,
            result.model.as_deref(),
            run_prompt,
            result,
            &begun.persisted_event_indexes,
        ) {
            eprintln!(
                "oneharness: warning: could not write history record for `{}`: {err}",
                result.harness
            );
        }
    }
}

/// A codex candidate whose controlled turn runs on the shared app-server.
#[derive(Clone, Copy)]
struct SharedAppServer {
    /// The candidate's resolved `plugins`, part of the key of the server it leases.
    plugins: bool,
}

/// Everything a controlled run's driver needs to bind the mechanism of the
/// candidate that is about to serve: the run-wide per-turn settings plus every
/// candidate's assembled prompt. The rest — mechanism, conversation, approval
/// posture — comes from the candidate itself as it takes the turn.
struct ControlledRun<'a> {
    /// The shared handle behind the run's socket address. The address is the
    /// run's for its whole lifetime; what sits behind it changes per candidate.
    handle: &'a control_io::ControlHandle,
    /// One entry per plan entry: the prompt that candidate opens its turn with,
    /// or `None` for an entry that never assembled one (a skipped candidate).
    prompts: &'a [Option<String>],
    /// One entry per plan entry: `Some` for a candidate whose turn runs on the
    /// shared Codex app-server.
    app_servers: &'a [Option<SharedAppServer>],
    cwd: control::AbsolutePath,
    mode: PermissionMode,
    /// The stored conversation this run continues, and the identity that minted
    /// it. `None` on a fresh session (or with no `--session` at all).
    resume: Option<ControlResume>,
}

/// A `--session` continue as the control channel needs it: the native token,
/// and the one identity it means anything to.
///
/// The two travel together for the same reason [`HarnessPlan::resume`] is
/// filtered by the session anchor on the argv path: a native token exists only
/// in the session namespace of the identity that minted it, so handing it to a
/// different fallback candidate would ask one harness — or one variant's home
/// directory — to reopen a conversation it has never heard of.
struct ControlResume {
    anchor: HarnessIdentity,
    token: String,
}

impl ControlledRun<'_> {
    /// Bind the channel to `spec`'s mechanism for the turn it is about to
    /// serve, building the protocol conversation the shape needs.
    ///
    /// `harness_id` is the variant-qualified selector of the candidate taking
    /// the turn — the axis the session token is scoped to, since a chain holds
    /// several candidates and the token belongs to exactly one of them.
    ///
    /// `model` is the candidate's **own** resolved model (`unit.model`, the
    /// value the result's and the record's `model` report), never the
    /// run-level one.
    // llmlint: ignore[comments_earn_their_place] The measured mechanism behind issue #1283 is stated here by that issue's own resolution: the issue's diagnosis was wrong, and this is the one place a reader of the binding finds the corrected one. Until this change `bind` took the run-level model (`--model`, else top-level `model`), which no other path used, so a per-harness `[harness.codex].model` never reached `thread/start` — the frame went out as `{"approvalPolicy":"never","cwd":…}` — and codex ran its own default, which moved from `gpt-5.6-sol` to `gpt-6-astra` at codex 0.153; the top-level `model` workaround did reach the wire, which is what made the defect read as a version regression.
    ///
    /// Answers the shared app-server plan when the candidate's turn runs on
    /// one: its `job` is the launch its own child would have had, which keys
    /// the pooled server, and the plan's per-thread `config` rides the
    /// conversation built here.
    #[allow(clippy::too_many_arguments)] // llmlint: ignore[suppressions_justified] Each parameter is one fact about the candidate taking the turn — its mechanism, registry entry, selector, model, prompt, plan position and launch — read at the one call site per execution model; a struct would only move the list.
    fn bind(
        &self,
        shape: ControlShape,
        spec: &'static HarnessSpec,
        harness_id: &str,
        model: Option<&str>,
        prompt: &str,
        index: usize,
        job: &Job,
    ) -> Option<crate::io::app_server::PooledDispatch> {
        let pooled = self
            .app_servers
            .get(index)
            .copied()
            .flatten()
            .filter(|_| shape == ControlShape::CodexAppServer)
            .and_then(|shared| {
                crate::io::app_server::PooledDispatch::plan(
                    &job.argv,
                    &job.env,
                    &job.env_remove,
                    self.cwd.as_path(),
                    shared.plugins,
                )
            });
        let dialogue = Dialogue::new(
            shape,
            DialogueConfig {
                prompt: prompt.to_string(),
                cwd: self.cwd.clone(),
                model: model.map(str::to_string),
                mode: self.mode,
                // Only the anchor's own turn continues the stored conversation:
                // every other candidate opens a fresh one, exactly as the argv
                // path's `session_anchor` filter leaves it holding no `--resume`.
                // `DialogueResume` then answers for the mechanism — a shape with
                // no resume request has no continuation to build, which is the
                // pairing the command layer already refused before spawning.
                resume: self
                    .resume
                    .as_ref()
                    .filter(|resume| resume.anchor.as_str() == harness_id)
                    .and_then(|resume| DialogueResume::new(shape, resume.token.clone())),
                // The harness's own posture for this mode, not the spectrum's:
                // goose and copilot share one ACP shape and do not share a
                // mapping, and a driven turn must answer with what the same mode
                // gives without `--control`. Read off the candidate that is
                // SERVING, so a chain holding both answers as whichever is live.
                posture: spec
                    .mode(self.mode)
                    .map_or(ApprovalPosture::of(self.mode), |declared| declared.posture),
                shell_environment_policy: pooled.as_ref().map(|pooled| pooled.policy().clone()),
            },
        );
        // One arm per mechanism family, chosen from the shape right here: a
        // conversation binds as itself and carries its own protocol, so nothing
        // downstream is handed a shape that could contradict it.
        self.handle.bind(match dialogue {
            Some(dialogue) => control_io::Binding::Dialogue(Box::new(dialogue)),
            None => match HttpShape::of(shape) {
                Some(http) => control_io::Binding::PooledServer(http),
                None => control_io::Binding::Stdin,
            },
        });
        pooled
    }

    fn prompt(&self, index: usize) -> String {
        self.prompts
            .get(index)
            .cloned()
            .flatten()
            .unwrap_or_default()
    }
}

/// What [`drive_plan_sequentially`] produced; `fallback` is `Some` only in fallback mode.
struct StreamedPlan {
    results: Vec<RunResult>,
    fallback: Option<FallbackReport>,
    history: Vec<BegunHistory>,
}

/// Drive the plan one candidate at a time, publishing each one's normalized
/// events to `sink` as they arrive. In fallback mode a candidate is one plan
/// entry — a harness, or a (harness, model) pair when a model list is the chain.
///
/// Two callers want this rather than the parallel driver, and only one of them
/// is `--stream`: a controlled fallback chain needs the sequential order too,
/// because control drives one live turn. There `sink` is `None` (the caller
/// asked for a buffered report), so the live half is history alone — which is
/// why the name is the ordering, not the protocol.
///
/// Publishing a candidate before the chain has settled is safe: one that
/// published an event has, by construction, a tool event in its result (the
/// streamed line and the reported array come from the same recognizer over the
/// same stdout), which is [`fallback::RunWork::Done`] — so it cannot then fall
/// through, and a candidate that does fall through published nothing.
// llmlint: ignore[suppressions_justified] The allow is justified here: every parameter is one already-resolved input of the streamed chain — the plan and its jobs, the two mode flags the chain reads, and the four side channels (history, attribution, control, sink/cancel) — assembled by the single caller from four different places, so a struct would move the list one call up rather than shorten it.
#[allow(clippy::too_many_arguments)]
fn drive_plan_sequentially(
    plan: Vec<Plan>,
    jobs: &[Job],
    fallback_mode: bool,
    multi_model: bool,
    history_writer: Option<&HistoryWriter>,
    unit_ids: &[HarnessIdentity],
    default_prompt: &str,
    controlled: Option<&ControlledRun<'_>>,
    sink: &mut Option<&mut dyn EventSink>,
    spawn: SpawnControls<'_>,
    server_overloaded_max_retries: u32,
) -> StreamedPlan {
    let mut results: Vec<RunResult> = Vec::new();
    let mut history: Vec<BegunHistory> = Vec::new();
    let mut fallback_report = FallbackReport {
        ran: None,
        fell_through: Vec::new(),
        stopped_without_work: false,
    };
    for (index, entry) in plan.into_iter().enumerate() {
        // Begun as the chain reaches it — an already-resolved row too, since
        // it closes as its own record (see `begin_each_plan_entry`).
        let run_id = history_writer
            .map(|writer| writer.begin_harness_run(&unit_ids[index], entry.prompt(default_prompt)));
        let mut stop_at_opening = false;
        if let Some((writer, run_id)) = history_writer.zip(run_id) {
            if sink.as_mut().is_some_and(|sink| {
                sink.run_began(&writer.run_begun(&unit_ids[index], run_id)) == SinkStep::Stop
            }) {
                stop_at_opening = true;
                spawn.cancel.cancel();
            }
        }
        let streamed = match entry {
            Plan::Ready(result) => StreamedHarness {
                result: *result,
                persisted_event_indexes: BTreeSet::new(),
            },
            Plan::Pending {
                spec,
                bin,
                output_format,
                job_index,
                prompt,
                model,
            } => {
                let unit = StreamedUnit {
                    job: &jobs[job_index],
                    spec,
                    bin: &bin,
                    output_format,
                    prompt,
                    model,
                    harness_id: unit_ids[index].as_str(),
                };
                let mut run_candidate = || {
                    if crate::io::cancel::cancellation_requested(spawn.cancel) {
                        return cancelled_streamed_harness(unit.clone());
                    }
                    match controlled {
                        // Every candidate, not just one chosen up front: the channel
                        // binds to whichever is serving and releases when its turn
                        // ends, so a chain that falls through carries control with
                        // it instead of leaving the rest of the run unaddressable.
                        Some(chain) => drive_controlled_candidate(
                            unit.clone(),
                            chain,
                            index,
                            history_writer.zip(run_id),
                            sink,
                            spawn,
                        ),
                        None => stream_one_harness(
                            unit.clone(),
                            history_writer.zip(run_id),
                            None,
                            sink,
                            spawn,
                        ),
                    }
                };
                let mut streamed = run_candidate();
                for retry in 0..server_overloaded_max_retries {
                    if streamed.result.failure_kind != Some(signals::FailureKind::ServerOverloaded)
                        || fallback::RunWork::from_result(&streamed.result)
                            != fallback::RunWork::None
                    {
                        break;
                    }
                    server_overloaded_backoff(retry + 1);
                    streamed = run_candidate();
                }
                streamed
            }
        };
        let StreamedHarness {
            result,
            persisted_event_indexes,
        } = streamed;
        let keep_going = fallback_step(&result, multi_model, &mut fallback_report);
        results.push(result);
        history.push(BegunHistory {
            run_id,
            persisted_event_indexes,
        });
        if stop_at_opening || !fallback_mode || !keep_going {
            break;
        }
    }
    StreamedPlan {
        results,
        fallback: fallback_mode.then_some(fallback_report),
        history,
    }
}

/// Run one candidate of a controlled run with the channel bound to **its**
/// mechanism, and release it the moment that candidate's turn ends.
///
/// This is where a chain stops reasoning over the candidate set and starts
/// reasoning over the candidate that is serving. The mechanism is read off
/// `unit.spec`, not off the selection, so a chain whose candidates control
/// differently serves each one over its own — one at a time, which is the only
/// way a chain ever runs them. Between two candidates nothing is bound at all,
/// so an interrupt that arrives across a fall-through is `no_active_turn` rather
/// than a frame written at a mechanism nobody is on.
fn drive_controlled_candidate(
    unit: StreamedUnit<'_>,
    chain: &ControlledRun<'_>,
    index: usize,
    history: Option<(&HistoryWriter, crate::domain::history::HistoryId)>,
    sink: &mut Option<&mut dyn EventSink>,
    spawn: SpawnControls<'_>,
) -> StreamedHarness {
    let spec = unit.spec;
    let shape = spec
        .control
        .expect("validate_control refuses a controlled candidate with no mechanism");
    let prompt = chain.prompt(index);
    // A server-submitted candidate never spawns the harness CLI at all, so it
    // takes its own execution model here rather than the job the plan built for
    // it. Nothing about that is shared with the candidate beside it in the
    // chain — which is exactly why the mechanism is bound per candidate.
    if let Some(http) = HttpShape::of(shape) {
        return StreamedHarness {
            result: http_controlled_result(
                http,
                chain.handle,
                HttpCandidate {
                    spec,
                    bin: unit.bin.to_string(),
                    output_format: unit.output_format,
                    result_prompt: unit.prompt,
                    model: unit.model,
                    timeout: unit.job.timeout,
                },
                &prompt,
                &chain.cwd,
                chain.mode,
            ),
            persisted_event_indexes: BTreeSet::new(),
        };
    }
    let pooled = chain.bind(
        shape,
        spec,
        unit.harness_id,
        unit.model.as_deref(),
        &prompt,
        index,
        unit.job,
    );
    let mut streamed = stream_one_harness(
        unit,
        history,
        Some(&runner::ControlledInput {
            handle: chain.handle,
            prompt,
            app_server: pooled.as_ref(),
        }),
        sink,
        spawn,
    );
    // Read off the conversation before it is released: the session id and the
    // answer a driven turn produced are knowable only from its protocol frames,
    // and the next candidate's binding replaces them.
    apply_dialogue_signals(&mut streamed.result, chain.handle);
    chain.handle.release();
    streamed
}

/// One harness's finished streaming run. The events *outside*
/// `persisted_event_indexes` are still owed to history by the closing record.
struct StreamedHarness {
    result: RunResult,
    persisted_event_indexes: BTreeSet<usize>,
}

/// One plan entry, resolved, as the streaming driver needs it.
#[derive(Clone)]
struct StreamedUnit<'a> {
    job: &'a Job,
    spec: &'static HarnessSpec,
    bin: &'a str,
    output_format: OutputFormat,
    /// The per-result prompt on a batch run; `None` otherwise.
    prompt: Option<String>,
    /// The model this unit ran with; `None` when the harness used its own.
    model: Option<String>,
    /// The variant-qualified selector this entry was planned from — the axis a
    /// streamed event is attributed to, since a model fan-out repeats a harness.
    harness_id: &'a str,
}

fn cancelled_streamed_harness(unit: StreamedUnit<'_>) -> StreamedHarness {
    let capture = runner::unstarted_capture(unit.job);
    StreamedHarness {
        result: executed_result(
            unit.spec,
            unit.bin.to_string(),
            unit.job.argv.clone(),
            unit.output_format,
            &capture,
            None,
            0,
            unit.prompt,
            unit.model,
        ),
        persisted_event_indexes: BTreeSet::new(),
    }
}

/// Run one harness with streaming: feed each stdout line through the event
/// extractor as it arrives, publish any new normalized events to `sink`, then
/// return the same [`RunResult`] a batch run would produce (from the accumulated
/// output). A sink that answers [`SinkStep::Stop`] (the consumer closed the
/// stream — short-circuiting on what it saw) tells the runner to stop and tear
/// the child down. `schema` is always `None` here (`--stream` and `--schema` are
/// mutually exclusive, enforced up front).
fn stream_one_harness(
    unit: StreamedUnit<'_>,
    history: Option<(&HistoryWriter, crate::domain::history::HistoryId)>,
    controlled: Option<&runner::ControlledInput>,
    sink: &mut Option<&mut dyn EventSink>,
    spawn: SpawnControls<'_>,
) -> StreamedHarness {
    use crate::io::runner::StreamStep;
    use serde_json::Value;

    let harness_id = unit.harness_id;
    // The same fold `extract_events` makes over the captured stdout, so each
    // event streamed here carries the index the report gives it — which is
    // what lets the closing history record skip exactly the ones already
    // persisted. An open item (a codex call's `item.started`) yields nothing
    // until the record that completes it arrives.
    let mut stream = events::EventStream::default();
    let mut persisted_event_indexes = BTreeSet::new();
    let capture = runner::run_job_streaming_supervised(unit.job, controlled, spawn, |line| {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return StreamStep::Continue;
        };
        let evs = stream.push(&value);
        for ev in &evs {
            if let Some((writer, run_id)) = history {
                match writer.append_event_tracked(run_id, harness_id, ev.clone()) {
                    Ok(outcome) => {
                        persisted_event_indexes.insert(ev.index);
                        if let Some(err) = outcome.index_error {
                            eprintln!(
                                "oneharness: warning: could not index history event for `{}`: {err}",
                                harness_id
                            );
                        }
                    }
                    Err(err) => {
                        eprintln!(
                            "oneharness: warning: could not write history event for `{}`: {err}",
                            harness_id
                        );
                    }
                }
            }
            // A sink that cannot take the event (the CLI's stdout pipe broke:
            // the consumer short-circuited and left) is the stop signal —
            // stop reading and tear the child down.
            if let Some(sink) = sink.as_mut() {
                if sink.event(harness_id, ev) == SinkStep::Stop {
                    return StreamStep::Stop;
                }
            }
        }
        StreamStep::Continue
    });
    let result = executed_result(
        unit.spec,
        unit.bin.to_string(),
        unit.job.argv.clone(),
        unit.output_format,
        &capture,
        None,
        1,
        unit.prompt,
        unit.model,
    );
    StreamedHarness {
        result,
        persisted_event_indexes,
    }
}

/// Validate a `--control` request and return the mechanism the channel *starts*
/// on — the first candidate's — or `None` when the flag was not passed (which
/// must change nothing at all: no socket, no extra process, and a byte-identical
/// argv).
///
/// `Some` means "this run is controlled", not "this run is on one mechanism". A
/// fallback chain binds each candidate's own as it takes the turn, so the value
/// here is only what the report names until the first one does.
///
/// Every check here is a *loud usage error* before anything spawns, because the
/// failure this feature must never have is a supervisor being told the lever
/// exists when it does not. In order: a caller-owned handle to address the run
/// by (oneharness never infers one — an unaddressable run is the whole reason
/// `--session` is required), one prompt and exactly one concurrent turn (one
/// harness in parallel mode, or a sequential fallback chain; a batch or fan-out
/// has no single turn to interrupt), then — for *every* candidate, since every
/// candidate can serve — a *proven* control mechanism, an expressible mode, and
/// an explicit output format compatible with that mechanism, and — last — a
/// platform with unix sockets.
///
/// The approval mode is almost never among them: `--control` no longer derives
/// a posture for the wire, so whatever a harness supports uncontrolled it
/// supports controlled, and an unsupported mode is `validate_modes`' refusal —
/// the same one an ordinary run gets. The one exception is a mode delivered
/// through the harness's own environment on a server-submitted turn; see below.
#[allow(clippy::fn_params_excessive_bools, clippy::too_many_arguments)] // llmlint: ignore[suppressions_justified] The parameters ARE the list of independently-resolved run properties the control rules must check, each decided separately by the caller; folding them into a struct used at this single call site would hide the list rather than shorten it.
fn validate_control(
    args: &RunRequest,
    specs: &[&'static HarnessSpec],
    explicit_format: Option<OutputFormat>,
    schema: bool,
    batch_run: bool,
    multi_model: bool,
    run_mode: RunMode,
    stream: bool,
    mode: PermissionMode,
) -> Result<Option<ControlShape>, OneharnessError> {
    if !args.control {
        return Ok(None);
    }
    if args.session.is_none() {
        return Err(OneharnessError::ControlNeedsSession);
    }
    // A batch fans one harness over N prompts, so there is no single live turn
    // to address. `--session` refuses a batch too, but say it in the control
    // vocabulary rather than leaving a supervisor to infer it.
    if batch_run {
        return Err(OneharnessError::ControlBatch);
    }
    // A model fan-out multiplies the run into several (harness, model) units,
    // and the controlled path drives exactly one live turn. `--session` refuses
    // a fan-out too, but a supervisor who passed `--control` needs to be told
    // which control rule they broke.
    if multi_model {
        return Err(OneharnessError::MultiModelConflict {
            with: "--control",
            why: "control drives one live turn, and a fan-out has no single turn to interrupt",
        });
    }
    // The validate/retry loop re-prompts, which is a second turn — and the
    // control channel owns the one open stdin. Refuse rather than silently
    // running with retries disabled.
    if schema {
        return Err(OneharnessError::ControlSchema);
    }
    // Parallel selection starts every harness together, while fallback starts
    // candidates one at a time. Control needs the former to name one harness;
    // the latter is already one live turn, and the channel binds to whichever
    // candidate is serving it.
    if specs.len() != 1 && run_mode != RunMode::Fallback {
        return Err(OneharnessError::ControlSingleHarness {
            selected: specs
                .iter()
                .map(|spec| spec.id)
                .collect::<Vec<_>>()
                .join(", "),
        });
    }
    // Every candidate is checked, because every candidate can serve: a chain
    // reaches candidate N+1 only when candidate N has finished, so which one
    // holds the channel is decided while the chain runs, not here.
    //
    // What is decided here is only what would be true of a candidate WHATEVER it
    // ends up doing — it declares no control surface at all, it cannot express
    // this mode, its mechanism pins a format the caller pinned differently. Each
    // is a property of the request, so each is a loud usage error before
    // anything spawns; a supervisor told the lever exists must never find that
    // the candidate serving them has none.
    //
    // What is NOT decided here is which mechanism the channel speaks. That is
    // the serving candidate's, bound as it takes the turn and released when the
    // turn ends (`ControlHandle::bind`), so a chain whose candidates control
    // differently is a chain of differing mechanisms served one at a time —
    // never two at once, and never one harness's mechanism bound to another's
    // turn.
    for spec in specs {
        let Some(shape) = spec.control else {
            return Err(OneharnessError::ControlUnsupported {
                id: spec.id.to_string(),
                supported: control_capable_ids(),
            });
        };
        // Almost no mode is refused here. Where the mode's policy travels on the
        // controlled launch itself — copilot's permission flags beside `--acp`,
        // Claude Code's ordinary `-p` argv — a controlled run is under exactly
        // the policy `spec.modes` declares, and an unexpressible mode is
        // `validate_modes`' refusal, the same one an ordinary run gets.
        //
        // The exception is a mode whose policy is the harness's OWN environment
        // on a turn submitted to a pooled server (opencode's `edit`, carried in
        // `OPENCODE_CONFIG_CONTENT`). That environment belongs to the server
        // process, not to the turn, and handing it over there was reverted: it
        // made the mode a component of the pool key and gave a controlled
        // `--mode default` turn a policy it never ended under (see the known gap
        // recorded in `control_mode_parity`). Delivering nothing would run the
        // turn under whatever policy the server was started with, which is the
        // silent reshaping `--control` must never do.
        if shape.needs_pooled_server()
            && spec
                .mode(mode)
                .is_some_and(|declared| !declared.env.is_empty())
        {
            return Err(OneharnessError::ControlModeUnsupported {
                id: spec.id.to_string(),
                mode: mode.as_str(),
            });
        }
        // A server-submitted turn never spawns the harness CLI, so there is no
        // stdout to publish line by line. Refusing is what keeps `--stream` from
        // silently selecting the ordinary run — whose interrupt does NOT reach
        // the turn, which is the whole reason this mechanism exists. Refused for
        // ANY candidate that declares one, not just the first: `--stream` is a
        // promise about this run's stdout made before a candidate is chosen, and
        // discovering mid-chain that the serving one cannot keep it is the
        // silent downgrade the flag exists to prevent.
        if stream && HttpShape::of(shape).is_some() {
            return Err(OneharnessError::ControlStreamUnsupported {
                id: spec.id.to_string(),
            });
        }
        if let (Some(required), Some(explicit)) = (shape.required_format(), explicit_format) {
            if required != explicit {
                return Err(OneharnessError::ControlOutputFormat {
                    id: spec.id.to_string(),
                    required: required.as_str().to_string(),
                    selected: explicit.as_str().to_string(),
                });
            }
        }
    }
    let shape = specs
        .first()
        .and_then(|spec| spec.control)
        .expect("a selection always has at least one harness, and each declares control here");
    // Last, and only for a run that would actually open one. Every check above
    // states something about the REQUEST — this harness has no control surface,
    // this mode is not expressible, this format is incompatible — and each is
    // true on every platform, so answering with the platform first would hand a
    // supervisor the one reason that disappears when they change machines while
    // hiding the one that does not.
    //
    // A dry run is exempt outright: `--print-command` opens no socket and spawns
    // nothing (the listener is bound only when it is absent), and the argv it
    // answers with — the control-stream flags, or the server a submitted turn
    // would be launched against — is a platform-independent fact. The report
    // still carries a null `control` block, so nothing claims a channel exists.
    if !args.print_command && !control_io::supported() {
        return Err(OneharnessError::ControlPlatform);
    }
    Ok(Some(shape))
}

/// Drive one turn on a pooled control server over HTTP, and shape it into the
/// ordinary result envelope.
///
/// This is the third execution model: nothing about the harness's own CLI run
/// is involved, because its interrupt does not reach one (live-REFUTED for both
/// harnesses). The recorded `command` is therefore the SERVER's launch argv —
/// what oneharness actually ran — rather than a headless invocation that never
/// happened.
///
/// Every failure here is a result, never a panic: a server that will not start
/// or a route that refuses is `spawn_error`/`nonzero` with the reason in
/// `error`, exactly like a harness that could not be spawned.
fn run_http_controlled(
    shape: HttpShape,
    handle: &control_io::ControlHandle,
    plan: Vec<Plan>,
    prompt: &str,
    cwd: &control::AbsolutePath,
    mode: PermissionMode,
    timeout: Duration,
) -> Vec<RunResult> {
    plan.into_iter()
        .map(|entry| match entry {
            Plan::Ready(result) => *result,
            Plan::Pending {
                spec,
                bin,
                output_format,
                prompt: result_prompt,
                model,
                ..
            } => http_controlled_result(
                shape,
                handle,
                HttpCandidate {
                    spec,
                    bin,
                    output_format,
                    result_prompt,
                    model,
                    timeout,
                },
                prompt,
                cwd,
                mode,
            ),
        })
        .collect()
}

/// One plan entry, as the server-submitted execution model needs it.
///
/// Its own type because this model shares nothing with the CLI job the plan also
/// built for the entry: the argv is never spawned, and the `command` the result
/// records is the SERVER's launch instead.
struct HttpCandidate {
    spec: &'static HarnessSpec,
    bin: String,
    output_format: OutputFormat,
    /// The per-result prompt on a batch run; `None` otherwise.
    result_prompt: Option<String>,
    model: Option<String>,
    timeout: Duration,
}

/// Submit one candidate's turn to its pooled control server and shape the answer
/// into the ordinary result envelope.
///
/// Shared by the single-turn HTTP driver and the sequential chain driver, so a
/// server-submitted candidate reached through a fallback chain is exactly the
/// turn it is on its own — the same lease, the same session, the same recorded
/// server argv.
fn http_controlled_result(
    shape: HttpShape,
    handle: &control_io::ControlHandle,
    candidate: HttpCandidate,
    prompt: &str,
    cwd: &control::AbsolutePath,
    mode: PermissionMode,
) -> RunResult {
    let HttpCandidate {
        spec,
        bin,
        output_format,
        result_prompt,
        model,
        timeout,
    } = candidate;
    let outcome = drive_http_turn(
        shape,
        handle,
        spec,
        &bin,
        prompt,
        cwd,
        mode,
        timeout,
        model.as_deref(),
    );
    let (command, capture, session_id) = match outcome {
        Ok((command, outcome, session_id)) => (command, outcome, Some(session_id)),
        Err(err) => (vec![bin.clone()], http_turn::TurnOutcome::failed(err), None),
    };
    let mut result = executed_result(
        spec,
        bin,
        command,
        output_format,
        &capture.to_capture(),
        None,
        1,
        result_prompt,
        model,
    );
    // The turn's own signals: the server's session id, and the text the event
    // stream carried. Both `None` rather than guessed when the turn produced
    // neither.
    result.session_id = session_id;
    if let Some(text) = capture.text() {
        result.text = Some(text.to_string());
        result.text_source = Some(format!("http:{}", shape.shape().as_str()));
    }
    result
}

/// Bring up the harness's control server (reusing a pooled one where a live
/// dispatch already has it), open a session on it, and run the turn.
#[allow(clippy::too_many_arguments)] // llmlint: ignore[suppressions_justified] Every parameter is one input the server bring-up genuinely needs — harness identity, address, prompt, and posture — and grouping them into a struct used at one call site would hide the list rather than shorten it.
fn drive_http_turn(
    shape: HttpShape,
    handle: &control_io::ControlHandle,
    spec: &'static HarnessSpec,
    bin: &str,
    prompt: &str,
    cwd: &control::AbsolutePath,
    mode: PermissionMode,
    timeout: Duration,
    model: Option<&str>,
) -> Result<(Vec<String>, http_turn::TurnOutcome, String), String> {
    // Parsed before anything is brought up: a model this protocol cannot name
    // is refused rather than dropped, because dropping it runs the turn on
    // whatever the server picks — which is how a controlled opencode turn came
    // to run on a free model that answers 401.
    let session_model = match (shape, model) {
        (HttpShape::Opencode, Some(model)) => Some(http::OpencodeModel::parse(model).ok_or_else(
            || {
                format!(
                    "a controlled `{}` turn names its model to the session it opens, and that route takes a provider and an id: `--model {model}` names no provider. Use the fully-qualified `<provider>/<model>` form (e.g. `anthropic/claude-haiku-4-5`), the same id `opencode run --model` takes",
                    spec.id
                )
            },
        )?),
        // Crush's model is the server's, settled at launch.
        _ => None,
    };
    let server = spec.server.ok_or_else(|| {
        format!(
            "`{}` declares HTTP control but no server to run it",
            spec.id
        )
    })?;
    let root = server_pool::resolve_root(None)
        .ok_or_else(|| "no state directory to keep the control-server pool in".to_string())?;
    // The harness's own mapping for this mode. Resolved here rather than from
    // the normalized spectrum, so a controlled turn runs under exactly the
    // policy the same mode gives without `--control`. Support was checked before
    // anything spawned, so an absent one is a bug rather than a user error.
    let mode_spec = spec
        .mode(mode)
        .ok_or_else(|| format!("`{}` cannot express `--mode {}`", spec.id, mode.as_str()))?;
    // The mode's own environment does NOT travel to the server. Handing it over
    // was tried and reverted: it made the approval mode a component of the pool
    // key, and the controlled `--mode default` turn it was meant to prove never
    // ended on opencode across four CI cycles. A mode that can only be delivered
    // that way is refused before anything spawns (`validate_control`), so the
    // server is never launched under a policy other than its own — and the gap
    // is recorded rather than papered over (`control_mode_parity`).
    // Per-turn settings are deliberately not in the key either: they are
    // negotiated on the wire, and keying on them would start a fresh server per
    // dispatch.
    let key_env: Vec<(String, Option<String>)> = server
        .key_env
        .iter()
        .map(|name| ((*name).to_string(), std::env::var(name).ok()))
        .collect();
    let key = control::pool_key(spec.id, &key_env, &[]);
    let (lease, address) = bring_up_server(shape, spec, bin, &root, &key, timeout)?;
    let command = lease.record().argv.as_slice().to_vec();

    let decision = http::permits_action(mode_spec);
    let turn = http_turn::open(
        shape,
        address,
        cwd,
        decision,
        &http_turn::client_id(spec.id),
        session_model.as_ref(),
    )
    .map_err(|err| format!("{err}"))?;
    let session_id = turn.session_id().to_string();

    // Addressable from the socket thread only while the turn is in flight, so
    // an interrupt before or after it is an honest `no_active_turn`. The
    // mechanism is bound here rather than for the run, so a chain that reaches
    // this candidate after falling through a CLI-driven one moves the channel
    // onto the server with it — and releases it again below, leaving the next
    // candidate free to bind its own.
    // Named by the server it is submitted to, so the binding carries no shape
    // that could disagree with the mechanism actually serving.
    handle.bind(control_io::Binding::PooledServer(shape));
    handle.begin_http_turn(turn.clone());
    // The driver asks for a redirection each time a turn ends: an interrupt
    // commits its message to the handle, and this is where the run hands it to a
    // session that has actually gone idle — and tells the handle whether the
    // session took it, which is when that interrupt is answered.
    let outcome = http_turn::drive(&turn, prompt, decision, timeout, handle);
    handle.end_http_turn();
    handle.release();
    // The lease is released here (not at process exit), so a server nobody is
    // using can be reclaimed once its linger expires.
    drop(lease);
    Ok((command, outcome, session_id))
}

/// Lease a running control server for this dispatch: pick a candidate address,
/// have the pool start (or reuse) a server there, and wait until it answers.
///
/// The wait can end two ways, and only one of them is retried. A server that
/// EXITED during bring-up is relaunched once at a *fresh* candidate address,
/// because losing the address is one of the ways a server dies at once: a TCP
/// port is reserved by binding and letting go, so between the reservation and
/// the launch it belongs to whoever asks the kernel next, and the loser dies on
/// `EADDRINUSE`. A server that is merely SILENT is never relaunched — it is
/// still running, and re-rolling a window the caller already bounded would just
/// spend the budget twice.
fn bring_up_server(
    shape: HttpShape,
    spec: &'static HarnessSpec,
    bin: &str,
    root: &std::path::Path,
    key: &control::PoolKey,
    timeout: Duration,
) -> Result<(server_pool::ServerLease, DialAddress), String> {
    // Re-read rather than taken as a parameter: the caller already proved it is
    // there, and this is the one place the launch is assembled.
    let server = spec.server.ok_or_else(|| {
        format!(
            "`{}` declares HTTP control but no server to run it",
            spec.id
        )
    })?;
    let mut attempts_left = SERVER_START_ATTEMPTS;
    loop {
        attempts_left -= 1;
        let candidate = candidate_address(server.transport, root, key)?;
        let plan = server_pool::LaunchPlan::new(bin, &server, &[], candidate, Vec::new())
            .map_err(|err| format!("could not plan the control server launch: {err}"))?;
        let lease = server_pool::acquire(root, key, &plan, server_pool::DEFAULT_LINGER)
            .map_err(|err| format!("could not start the control server: {err}"))?;
        // Narrowed to a dialable address once, here, where the HTTP control path
        // takes hold of the running server: everything downstream is then handed
        // an address it can actually open a socket to.
        let address = DialAddress::try_from(lease.record().address.clone()).map_err(|err| {
            format!(
                "the control server for `{}` cannot be reached over HTTP: {err}",
                spec.id
            )
        })?;
        let record = lease.record().clone();
        // Bounded by the run's own timeout as well as the window, because a
        // server that never comes up must not hold a dispatch past the budget
        // its caller set: a `--timeout 5` run waiting 90s for a bring-up is a
        // hang as far as the caller is concerned.
        let ready_timeout = if timeout.is_zero() {
            SERVER_READY_WINDOW
        } else {
            SERVER_READY_WINDOW.min(timeout)
        };
        match http_turn::await_ready(shape, &address, ready_timeout, &|| record.is_running()) {
            Ok(()) => return Ok((lease, address)),
            Err(not_ready) => {
                // Released before relaunching, so the pool sees the dead entry
                // for what it is and starts a server this dispatch can vouch for.
                drop(lease);
                if attempts_left == 0 || !not_ready.exited() {
                    return Err(not_ready.to_string());
                }
            }
        }
    }
}

/// Where a freshly launched server should listen. A reused one keeps its own
/// address; this is only the candidate the pool uses if it has to start one.
fn candidate_address(
    transport: control::ServerTransport,
    root: &std::path::Path,
    key: &control::PoolKey,
) -> Result<control::ServerAddress, String> {
    match transport {
        control::ServerTransport::Tcp => {
            // Ask the OS for a free port by binding and immediately dropping:
            // the same trick every test harness uses, and the only way to pick
            // one that is not already taken.
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|err| {
                format!("could not find a free port for the control server: {err}")
            })?;
            let port = listener
                .local_addr()
                .map_err(|err| format!("could not read the chosen control-server port: {err}"))?
                .port();
            control::Port::new(port)
                .map(|port| control::ServerAddress::Tcp { port })
                .map_err(|err| format!("the OS offered no usable port: {err}"))
        }
        control::ServerTransport::UnixSocket => {
            let path = root.join(key.as_str()).join("server.sock");
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("could not prepare the control-server socket: {err}"))?;
            }
            control::AbsolutePath::new(&path)
                .map(|path| control::ServerAddress::UnixSocket { path })
                .map_err(|err| format!("the control-server socket path is unusable: {err}"))
        }
        control::ServerTransport::Stdio => {
            Err("a stdio server is not reached over HTTP".to_string())
        }
    }
}

/// Take the normalized signals a protocol-driven control run produced from the
/// dialogue that produced them.
///
/// A server-backed mechanism's stdout is a JSON-RPC stream, not the harness's
/// ordinary output document, so the generic extractors read nothing from it.
/// The dialogue already parsed the same stream to drive the turn, and is the
/// only thing that knows which frame carried the session id, which carried the
/// answer, and which carried the server's own statement of the model the
/// thread runs under. It leaves each `null` when the turn produced none — a
/// protocol run never fabricates a signal any more than a plain one does.
///
/// A dialogue that ended in a [`DialogueRefusal`] never opened a turn, and the
/// runner's teardown of the server it was talking to is not that server
/// failing — so the refusal is written over the envelope here: a non-`ok`
/// status, the classified kind, and an `error` naming both models. The
/// `exit_code` stays `null`, since the server was torn down rather than exited.
///
/// [`DialogueRefusal`]: crate::domain::dialogue::DialogueRefusal
fn apply_dialogue_signals(result: &mut RunResult, handle: &crate::io::control::ControlHandle) {
    if !handle.drives_turn_over_stdin() {
        return;
    }
    result.session_id = handle.session_id();
    result.observed_model = handle.observed_model();
    if let Some((text, source)) = handle.text() {
        result.text = Some(text);
        result.text_source = Some(source.to_string());
    }
    // The server's own per-turn token accounting, priced exactly as an exec
    // turn's is, and re-read as work evidence since the envelope's was read
    // before the dialogue's signals arrived.
    if let Some((mut usage, source)) = handle.usage() {
        signals::apply_model_price(&mut usage, &result.harness_id, result.model.as_deref());
        result.usage = usage;
        result.usage_source = Some(source.to_string());
        result.refresh_work_evidence();
    }
    // A turn on the shared app-server records the server's launch — what
    // oneharness actually ran — and, when the server went away under the live
    // turn, the retryable kind that says so rather than a turn failure.
    if let Some(command) = handle.app_server_command() {
        result.command = command;
    }
    if handle.app_server_lost() {
        result.status = Status::Nonzero;
        result.exit_code = None;
        result.failure_kind = Some(signals::FailureKind::AppServerLost);
        result.failure_kind_source = Some("pool:codex-app-server".to_string());
        result.work = None;
        return;
    }
    if let Some((refusal, source)) = handle.refusal() {
        // Matched exhaustively on purpose: a new refusal kind is a deliberate
        // answer to which `FailureKind` it publishes as, not an inherited one.
        let kind = match &refusal {
            crate::domain::dialogue::DialogueRefusal::ModelMismatch { .. } => {
                signals::FailureKind::ModelMismatch
            }
        };
        result.status = Status::Nonzero;
        result.exit_code = None;
        result.failure_kind = Some(kind);
        result.failure_kind_source = Some(source.to_string());
        result.error = Some(refusal.error(&result.harness));
        // Re-derived from the envelope just written, exactly as every
        // constructor derives it: a classified refusal publishes no reading.
        result.work =
            crate::domain::report::unclassified_failure(result.status, result.failure_kind)
                .then(|| fallback::RunWork::from_result(result));
    }
}

/// The absolute working directory a controlled turn runs in.
///
/// A server-backed mechanism negotiates the directory on the wire rather than
/// inheriting it from a spawn, and the server may well resolve a relative path
/// against its own cwd rather than the dispatch's — so it is made absolute here,
/// once, from the same `--cwd` an ordinary run would spawn into.
fn control_cwd(args: &RunRequest) -> Result<control::AbsolutePath, OneharnessError> {
    let cwd = args
        .cwd
        .clone()
        .unwrap_or_else(|| PathBuf::from(std::path::Component::CurDir.as_os_str()));
    let absolute = if cwd.is_absolute() {
        cwd
    } else {
        std::env::current_dir()
            .map(|base| base.join(&cwd))
            .unwrap_or(cwd)
    };
    let absolute = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    control::AbsolutePath::new(&absolute).map_err(|message| OneharnessError::ControlSocket {
        path: absolute.display().to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, message),
    })
}

/// The comma-joined ids of every control-capable harness, for the "control
/// capable:" hint on a `--control` capability error.
fn control_capable_ids() -> String {
    let ids: Vec<&str> = harness::all()
        .iter()
        .filter(|spec| spec.control.is_some())
        .map(|spec| spec.id)
        .collect();
    if ids.is_empty() {
        "none".to_string()
    } else {
        ids.join(", ")
    }
}

/// Refuse `--stream` combined with anything it cannot serve: a batch
/// (multi-prompt) run or structured output — each needs the whole output at
/// once, which streaming does not provide — and, in `parallel` mode, more than
/// one harness. A loud usage error before anything spawns.
///
/// A **fallback** chain may list several candidates — harnesses, and each
/// harness's models (the multi-model half is refused in [`validate_multi_model`],
/// which allows it here for the same reason): only the candidate that runs ever
/// publishes (see [`drive_plan_sequentially`]), so there is nothing to interleave.
fn validate_stream(
    stream: bool,
    specs: &[&'static HarnessSpec],
    batch_run: bool,
    has_schema: bool,
    fallback_mode: bool,
) -> Result<(), OneharnessError> {
    if !stream {
        return Ok(());
    }
    if specs.len() > 1 && !fallback_mode {
        return Err(OneharnessError::StreamInvalid(
            "--stream runs a single harness; select exactly one with --harness <id>, or use \
             --run-mode fallback to stream the first candidate that runs (a multi-harness \
             parallel stream would interleave unrelated event streams on one stdout)"
                .to_string(),
        ));
    }
    if batch_run {
        return Err(OneharnessError::StreamInvalid(
            "--stream is incompatible with a batch (multiple prompts): the batch fan-out needs \
             each run's whole output. Stream one prompt at a time instead."
                .to_string(),
        ));
    }
    if has_schema {
        return Err(OneharnessError::StreamInvalid(
            "--stream is incompatible with --schema: structured-output validation and its retry \
             loop need the complete answer, not an incremental stream."
                .to_string(),
        ));
    }
    Ok(())
}

/// Whether this run streams: `--stream` / `--no-stream` (clap-exclusive, so at
/// most one is set) beat the layered `stream` config value, which is off by
/// default.
///
/// The flag wins in both directions on purpose. A consumer that always reads
/// events declares `stream = true` once instead of injecting the flag per
/// invocation, and a single call that needs the buffered report back says
/// `--no-stream` instead of having to unset the config it inherited.
fn resolve_stream(args: &RunRequest, cfg: &crate::domain::config::FileConfig) -> bool {
    args.stream.unwrap_or_else(|| cfg.stream.unwrap_or(false))
}

/// Run `jobs` wave by wave, preserving global order in the returned outcomes.
/// Each wave is a set of job indices run concurrently (up to `max_parallel`),
/// with a barrier between waves — so a `min-tokens` batch's warm-up call fully
/// completes (writing the shared cache prefix) before the readers fan out. The
/// structured-output retry closure is rebound per wave so its local index maps
/// back to the right `job_plans` entry. Every index in `0..jobs.len()` must
/// appear in exactly one wave (guaranteed by `batch::waves` and the single-wave
/// fallback), so every outcome slot is filled exactly once.
fn run_in_waves(
    jobs: &[Job],
    job_plans: &[HarnessPlan],
    schema: Option<&Schema>,
    retry_limits: RetryLimits,
    max_parallel: usize,
    waves: &[Vec<usize>],
    spawn: SpawnControls<'_>,
) -> Vec<Outcome> {
    let mut slots: Vec<Option<Outcome>> = (0..jobs.len()).map(|_| None).collect();
    for wave in waves {
        let wave_jobs: Vec<Job> = wave.iter().map(|&i| jobs[i].clone()).collect();
        let overload_attempts: Vec<_> = wave.iter().map(|_| AtomicU32::new(0)).collect();
        let schema_attempts: Vec<_> = wave.iter().map(|_| AtomicU32::new(0)).collect();
        let outs = match schema {
            // After each run, first retry a zero-work overload (including its
            // bounded backoff), otherwise validate structured output and, when
            // retries remain, rebuild with a feedback prompt. The runner owns
            // the spawning.
            Some(sch) => runner::run_jobs_supervised(
                &wave_jobs,
                max_parallel,
                spawn,
                |k, _attempt, capture| {
                    let overload_attempt = overload_attempts[k].load(Ordering::Relaxed) + 1;
                    if let Some(next) = server_overloaded_retry_after_backoff(
                        &job_plans[wave[k]],
                        overload_attempt,
                        retry_limits.server_overloaded,
                        capture,
                        Some(sch),
                    ) {
                        overload_attempts[k].fetch_add(1, Ordering::Relaxed);
                        return Some(next);
                    }
                    if is_server_overloaded_without_work(&job_plans[wave[k]], capture) {
                        return None;
                    }
                    let schema_attempt = schema_attempts[k].load(Ordering::Relaxed) + 1;
                    let next = retry_decision(
                        &job_plans[wave[k]],
                        sch,
                        schema_attempt,
                        retry_limits.schema,
                        capture,
                    );
                    if next.is_some() {
                        schema_attempts[k].fetch_add(1, Ordering::Relaxed);
                    }
                    next
                },
            ),
            None => runner::run_jobs_supervised(
                &wave_jobs,
                max_parallel,
                spawn,
                |k, _attempt, capture| {
                    let overload_attempt = overload_attempts[k].load(Ordering::Relaxed) + 1;
                    server_overloaded_retry_after_backoff(
                        &job_plans[wave[k]],
                        overload_attempt,
                        retry_limits.server_overloaded,
                        capture,
                        None,
                    )
                    .inspect(|_| {
                        overload_attempts[k].fetch_add(1, Ordering::Relaxed);
                    })
                },
            ),
        };
        for (k, out) in outs.into_iter().enumerate() {
            slots[wave[k]] = Some(out);
        }
    }
    slots
        .into_iter()
        .map(|o| o.expect("every job is scheduled into exactly one wave"))
        .collect()
}

/// Execute a fork-based `min-tokens` batch (job 0 is the warm-up, jobs 1.. the
/// fan-out). Runs the warm-up to establish a session, then — if it exposed a
/// session id — rewrites each fan-out job to `--resume <sid> --fork` (dropping
/// `--system`, which the session already carries) so the fan-out reuses the
/// warmed cached prefix, and runs them concurrently. Falls back to plain
/// independent fan-out (a warning, no reuse) when no session id is exposed.
/// Mutates `jobs`/`job_plans` in place (the caller reads back the rewritten argv
/// for the report, and checks `job_plans[1].resume` to learn whether it forked).
/// Returns outcomes in job order. The structured-output retry closure is rebound
/// per wave so its local index maps to the right `job_plans` entry.
fn run_fork_batch(
    jobs: &mut [Job],
    job_plans: &mut [HarnessPlan],
    schema: Option<&Schema>,
    max_retries: u32,
    max_parallel: usize,
    spawn: SpawnControls<'_>,
) -> Vec<Outcome> {
    let n = jobs.len();
    // Warm-up: job 0 alone (its own wave).
    let warm = runner::run_jobs_supervised(&jobs[0..1], 1, spawn, |_, attempt, capture| {
        schema.and_then(|s| retry_decision(&job_plans[0], s, attempt, max_retries, capture))
    })
    .into_iter()
    .next()
    .expect("one warm-up outcome");
    if n == 1 {
        return vec![warm];
    }
    // Branch the fan-out from the warmed session, when one was exposed. A
    // fork-capable harness (claude-code, opencode) emits a session id headlessly;
    // if absent, leave the fan-out as independent runs so the batch still
    // produces results (no cache reuse).
    match signals::extract_session(&warm.capture.stdout) {
        Some(sid) => {
            for plan in job_plans.iter_mut().skip(1) {
                plan.resume = Some(sid.clone());
                plan.fork = true;
                plan.system = None; // the session already carries --system
                plan.system_file = None; // ...so drop any off-argv system delivery too
            }
            for (i, job) in jobs.iter_mut().enumerate().skip(1) {
                let built = job_plans[i].build(schema, None);
                job.argv = built.argv;
                job.stdin = built.stdin;
            }
        }
        None => eprintln!(
            "oneharness: warning: the batch warm-up exposed no session id to fork; \
             the fan-out runs independently (no cache reuse)."
        ),
    }
    // Fan-out: jobs 1..n concurrently (local index k → job 1 + k).
    let fan =
        runner::run_jobs_supervised(&jobs[1..n], max_parallel, spawn, |k, attempt, capture| {
            schema.and_then(|s| retry_decision(&job_plans[1 + k], s, attempt, max_retries, capture))
        });
    let mut outcomes = Vec::with_capacity(n);
    outcomes.push(warm);
    outcomes.extend(fan);
    outcomes
}

/// Refuse the run shapes a fallback chain cannot express, before anything spawns.
/// Fallback drives several harnesses in priority order for one prompt, stopping
/// at the first that runs — so a multi-prompt batch and the explicit `--resume` /
/// `--fork` continuations (each pins one *specific* harness's native id) are loud
/// usage errors here. Called only for a chain of two or more candidates: a
/// one-candidate chain has nothing to fall through to, so it carries both
/// shapes as the single-harness run (see the
/// `single_candidate_batch_or_continuation` resolution in [`run`]). `origin`
/// is whether the mode was left unset — fallback is the default — so the
/// diagnostic can say which mode the caller met.
/// `--stream` is *not* refused (see [`drive_plan_sequentially`]).
/// `--session` is *not* refused either: the
/// higher-level named handle binds to the anchor (the first session-capable
/// harness in the chain), which fallback settles on under stable availability —
/// see [`setup_session`], which does the capability check for it. The *capability*
/// validation of the requested features against every listed harness stays in the
/// shared validators (`validate_modes`, `setup_mock`, …), which run over all specs
/// regardless of mode — so a flag no candidate could honor still fails fast even
/// though only one harness will run.
fn validate_fallback(
    batch_run: bool,
    args: &RunRequest,
    origin: RunModeOrigin,
) -> Result<(), OneharnessError> {
    let conflict = |with, why| Err(OneharnessError::FallbackConflict { with, why, origin });
    if batch_run {
        return conflict(
            "a batch run (more than one prompt)",
            "a chain tries harnesses in order for one prompt, while a batch fans one harness over many prompts; select one harness",
        );
    }
    if args.resume.is_some() {
        return conflict(
            "--resume/--fork",
            "a resumed session belongs to one specific harness, so it cannot fall through to another; select that harness alone, use --session (which binds to the chain's anchor), or pass --run-mode parallel",
        );
    }
    Ok(())
}

/// Refuse the run shapes a model fan-out cannot express, before anything spawns.
/// Fanning over models multiplies the run into several (harness, model) units, so
/// every single-unit shape is a loud usage error: a batch (its shared cache prefix
/// is per harness/model, so it cannot also vary the model), and each single-harness
/// continuation — `--resume` / `--fork` / `--session` (bound to one model context).
/// `--run-mode fallback` is deliberately *not* refused: the model list is exactly
/// the fallback chain there.
///
/// `--stream` follows from that. In `parallel` the fan-out really is several
/// concurrent results whose event streams would interleave on one stdout, so it
/// stays refused; in `fallback` the (harness, model) pairs are a priority chain
/// run one at a time with a single outcome — the same shape a multi-harness
/// chain streams in (see [`drive_plan_sequentially`] and [`validate_stream`]).
fn validate_multi_model(
    batch_run: bool,
    fallback_mode: bool,
    stream: bool,
    args: &RunRequest,
) -> Result<(), OneharnessError> {
    let conflict = |with, why| Err(OneharnessError::MultiModelConflict { with, why });
    if batch_run {
        return conflict(
            "a batch run (more than one prompt)",
            "a batch shares one cacheable prefix, which is per harness/model — fan out over models or over prompts, not both",
        );
    }
    if args.resume.is_some() {
        return conflict(
            "--resume/--fork",
            "a resumed session is tied to one model, so it cannot fan out over several",
        );
    }
    if args.session.is_some() {
        return conflict(
            "--session",
            "a named session is tied to one model, so it cannot fan out over several",
        );
    }
    if stream && !fallback_mode {
        return conflict(
            "--stream",
            "streaming emits one incremental output; a parallel model fan-out produces several results at once. Use --run-mode fallback to stream the first (harness, model) pair that runs",
        );
    }
    Ok(())
}

/// Run a single harness job under the overload and structured-output retry
/// loops — the one-harness analogue of a [`run_in_waves`] wave of size one —
/// returning its outcome. Used by the fallback driver, which spawns harnesses
/// one at a time.
#[derive(Clone, Copy)]
struct RetryLimits {
    schema: u32,
    server_overloaded: u32,
}

fn run_one_job(
    job: &Job,
    plan: &HarnessPlan,
    schema: Option<&Schema>,
    retry_limits: RetryLimits,
    spawn: SpawnControls<'_>,
) -> Outcome {
    let jobs = std::slice::from_ref(job);
    let overload_attempts = AtomicU32::new(0);
    let schema_attempts = AtomicU32::new(0);
    let outs = runner::run_jobs_supervised(jobs, 1, spawn, |_, _attempt, capture| {
        let overload_attempt = overload_attempts.load(Ordering::Relaxed) + 1;
        if let Some(next) = server_overloaded_retry_after_backoff(
            plan,
            overload_attempt,
            retry_limits.server_overloaded,
            capture,
            schema,
        ) {
            overload_attempts.fetch_add(1, Ordering::Relaxed);
            return Some(next);
        }
        if is_server_overloaded_without_work(plan, capture) {
            return None;
        }
        schema.and_then(|sch| {
            let schema_attempt = schema_attempts.load(Ordering::Relaxed) + 1;
            let next = retry_decision(plan, sch, schema_attempt, retry_limits.schema, capture);
            if next.is_some() {
                schema_attempts.fetch_add(1, Ordering::Relaxed);
            }
            next
        })
    });
    outs.into_iter().next().expect("one job, one outcome")
}

/// Drive the selected harnesses in priority order, stopping at the first that
/// actually runs the task (success OR real failure). A harness that could not run
/// at all — not installed, unspawnable, or rejected before doing any work (auth /
/// quota) — is fallen through to the next. Returns one result per *attempted*
/// harness (the fallen-through ones in order, then the one that ran), plus the
/// fallback report; candidates after the one that ran are never spawned and do
/// not appear. `plan`/`jobs`/`job_plans` are the same structures the parallel
/// path builds — a `Ready` entry is an already-resolved (here, always `Skipped`)
/// row, a `Pending` entry carries a job to spawn.
// llmlint: ignore[suppressions_justified] Like `drive_plan_sequentially`, every parameter is one already-resolved input of the chain the single caller assembles from different places; the two history parameters are the per-candidate begin the pointer file needs, which cannot be minted before the chain reaches a candidate.
#[allow(clippy::too_many_arguments)]
fn run_fallback(
    plan: Vec<Plan>,
    jobs: &[Job],
    job_plans: &[HarnessPlan],
    schema: Option<&Schema>,
    retry_limits: RetryLimits,
    multi_model: bool,
    history_writer: Option<&HistoryWriter>,
    unit_ids: &[HarnessIdentity],
    default_prompt: &str,
    sink: &mut Option<&mut dyn EventSink>,
    spawn: SpawnControls<'_>,
) -> (Vec<RunResult>, FallbackReport, Vec<BegunHistory>) {
    let mut results: Vec<RunResult> = Vec::new();
    let mut history: Vec<BegunHistory> = Vec::new();
    let mut fallback_report = FallbackReport {
        ran: None,
        fell_through: Vec::new(),
        stopped_without_work: false,
    };
    for (index, entry) in plan.into_iter().enumerate() {
        // Begun as the chain reaches it — a candidate after the one that ran is
        // never begun, so it gets no id and no pointer line.
        let run_id = history_writer.map(|writer| {
            begin_candidate(
                writer,
                &unit_ids[index],
                entry.prompt(default_prompt),
                sink,
                spawn.cancel,
            )
        });
        let result = match entry {
            Plan::Ready(result) => *result,
            Plan::Pending {
                spec,
                bin,
                output_format,
                job_index,
                prompt,
                model,
            } => {
                let outcome = run_one_job(
                    &jobs[job_index],
                    &job_plans[job_index],
                    schema,
                    retry_limits,
                    spawn,
                );
                let command = jobs[job_index].argv.clone();
                executed_result(
                    spec,
                    bin,
                    command,
                    output_format,
                    &outcome.capture,
                    schema,
                    outcome.attempts,
                    prompt,
                    model,
                )
            }
        };
        let keep_going = fallback_step(&result, multi_model, &mut fallback_report);
        results.push(result);
        history.push(BegunHistory {
            run_id,
            persisted_event_indexes: BTreeSet::new(),
        });
        if spawn.cancel.is_cancelled() || !keep_going {
            break;
        }
    }
    (results, fallback_report, history)
}

/// Apply the fallback verdict to one finished candidate: record why it fell
/// through, or name it as the harness that ran. Returns whether the chain should
/// try the next candidate.
///
/// The record carries the candidate's own `error` alongside oneharness's reason
/// token, so the cause travels with the classification: for a precondition
/// refusal that is the provider's machine-readable object (see
/// [`refusal_error`]), and for a missing binary or a spawn fault it is the
/// diagnostic the result already holds. Copied from the finished result rather
/// than re-read out of stdout — the layer above should never have to rediscover
/// something this run already classified.
///
/// Both drivers call this — the buffered [`run_fallback`] and the streaming
/// [`drive_plan_sequentially`] — on the same normalized [`RunResult`], so a streamed chain
/// and a buffered chain cannot select different candidates.
fn fallback_step(result: &RunResult, multi_model: bool, report: &mut FallbackReport) -> bool {
    let work = fallback::RunWork::from_result(result);
    // A model the candidate's *account* may not use is a per-identity
    // rejection with its own verdict (why: `account_model_refusal_reason`);
    // every other outcome, other `model_not_found`s included, keeps the
    // generic one.
    let account_refusal = result.failure_kind == Some(signals::FailureKind::ModelNotFound)
        && harness::by_id(&result.harness_id).is_some_and(|spec| {
            signals::model_refused_for_account(failure_dialect(spec), &result.stdout)
        });
    let reason = if account_refusal {
        fallback::account_model_refusal_reason(result.status, work)
    } else {
        fallback::startup_failure_reason(result.status, result.failure_kind, multi_model, work)
    };
    match reason {
        Some(reason) => {
            report.fell_through.push(FallThrough {
                harness: result.harness.clone(),
                reason,
                detail: result.error.clone(),
            });
            true
        }
        None => {
            report.ran = Some(result.harness.clone());
            // An unclassified failure stops here under either work reading
            // (why: `startup_failure_reason`); what is decided here is only
            // the attribution. The candidate that stops the chain is the one a
            // reader is left looking at, so it is here that "it failed and
            // nothing says why, and nothing says it did anything either" has
            // to be said. Read off the result's own published reading rather
            // than re-derived, so the attribution and the verdict above cannot
            // disagree.
            report.stopped_without_work = result.work == Some(fallback::RunWork::None);
            false
        }
    }
}

/// Exit code for a fallback run: a hard failure when no candidate could run at
/// all (nothing executed), otherwise the success of the one harness that ran —
/// which is always the last result. A real task failure or timeout there is a
/// failure; the fallen-through candidates never count against it, which is the
/// whole point of the mode. `require_available` does not apply (a missing harness
/// is the expected, tolerated case in fallback).
fn fallback_exit(results: &[RunResult], ran: bool) -> i32 {
    if !ran {
        return EXIT_FAILURE;
    }
    let last = results.last().expect("a harness ran, so there is a result");
    if is_failure(
        last.status,
        last.available,
        false,
        last.schema_valid,
        last.failure_kind,
    ) {
        EXIT_FAILURE
    } else {
        EXIT_OK
    }
}

/// A planned harness: either fully resolved (skipped/planned) or awaiting a job.
/// `Ready` is boxed because `RunResult` is far larger than `Pending`'s fields.
enum Plan {
    Ready(Box<RunResult>),
    Pending {
        spec: &'static HarnessSpec,
        bin: String,
        output_format: OutputFormat,
        job_index: usize,
        /// The per-result prompt on a batch run; `None` otherwise.
        prompt: Option<String>,
        /// The model this unit ran with (fan-out / per-harness); `None` when the
        /// harness used its own default.
        model: Option<String>,
    },
}

impl Plan {
    fn prompt<'a>(&'a self, default: &'a str) -> &'a str {
        match self {
            Self::Ready(result) => result.prompt.as_deref().unwrap_or(default),
            Self::Pending { prompt, .. } => prompt.as_deref().unwrap_or(default),
        }
    }
}

fn planned_result(
    spec: &HarnessSpec,
    bin: &str,
    available: bool,
    command: Vec<String>,
    output_format: OutputFormat,
    prompt: Option<String>,
    model: Option<String>,
) -> RunResult {
    RunResult {
        harness: spec.id.to_string(),
        variant: None,
        harness_id: spec.id.to_string(),
        bin: bin.to_string(),
        available,
        status: Status::Planned,
        prompt,
        model,
        observed_model: None,
        exit_code: None,
        duration_ms: None,
        telemetry: None,
        command,
        output_format,
        text: None,
        text_source: None,
        usage: Usage::default(),
        usage_source: None,
        session_id: None,
        events: None,
        events_source: None,
        structured: None,
        schema_valid: None,
        schema_attempts: None,
        schema_error: None,
        failure_kind: None,
        work: None,
        failure_kind_source: None,
        stdout: String::new(),
        stderr: String::new(),
        error: None,
    }
    .with_work_evidence()
}

fn skipped_result(
    spec: &HarnessSpec,
    bin: &str,
    command: Vec<String>,
    output_format: OutputFormat,
    prompt: Option<String>,
    model: Option<String>,
) -> RunResult {
    RunResult {
        harness: spec.id.to_string(),
        variant: None,
        harness_id: spec.id.to_string(),
        bin: bin.to_string(),
        available: false,
        status: Status::Skipped,
        prompt,
        model,
        observed_model: None,
        exit_code: None,
        duration_ms: None,
        telemetry: None,
        command,
        output_format,
        text: None,
        text_source: None,
        usage: Usage::default(),
        usage_source: None,
        session_id: None,
        events: None,
        events_source: None,
        structured: None,
        schema_valid: None,
        schema_attempts: None,
        schema_error: None,
        failure_kind: None,
        work: None,
        failure_kind_source: None,
        stdout: String::new(),
        stderr: String::new(),
        error: Some(format!(
            "`{bin}` not found on PATH; harness skipped. Install it: {}",
            spec.install_hint
        )),
    }
    .with_work_evidence()
}

/// A candidate whose variant selects an identity with no home directory on disk.
///
/// Reported as an `auth` failure that was **not run**: the binary is installed
/// (`available: true`, unlike [`skipped_result`]) and the argv is recorded, but
/// there are no credentials at the path the indirection names, so spawning could
/// only produce the harness's own unreadable refusal — or, worse, leave a config
/// directory behind for an account nobody has authenticated. `auth` is the
/// classification a fallback chain routes around, so an unauthenticated
/// candidate costs a chain nothing, exactly as an *empty* home directory already
/// does (see [`variant_unprovisioned_identity`]).
fn unprovisioned_result(
    spec: &HarnessSpec,
    bin: &str,
    command: Vec<String>,
    output_format: OutputFormat,
    prompt: Option<String>,
    model: Option<String>,
    identity: &UnprovisionedIdentity,
) -> RunResult {
    RunResult {
        available: true,
        status: Status::Skipped,
        failure_kind: Some(signals::FailureKind::Auth),
        // llmlint: ignore[invalid_states_unrepresentable] `failure_kind_source` is an open serialized string by contract (see its field doc); this is its one pre-spawn producer, pinned by the integration test that reads the emitted value.
        failure_kind_source: Some("config:env_from".to_string()),
        error: Some(format!(
            "`{target}` (from `{source}`) points at `{path}`, which does not exist, so this \
             identity has no credentials; harness not run. Create and authenticate that directory, \
             or drop this candidate from the selection.",
            target = identity.target,
            source = identity.source,
            path = identity.path.display(),
        )),
        ..skipped_result(spec, bin, command, output_format, prompt, model)
    }
    .with_work_evidence()
}

fn failure_dialect(spec: &HarnessSpec) -> signals::FailureDialect {
    match spec.id {
        "claude-code" => signals::FailureDialect::ClaudeCode,
        "codex" => signals::FailureDialect::Codex,
        _ => signals::FailureDialect::Generic,
    }
}

// llmlint: ignore[suppressions_justified] The allow is justified here: each
// argument is a separately-owned piece of the run this result freezes — the
// registry spec, what was actually spawned, what came back, the schema, and the
// per-unit prompt/model a batch or model fan-out varies. Grouping them into a
// struct would move the same list one call up, where three different callers
// assemble it from three different places.
#[allow(clippy::too_many_arguments)]
fn executed_result(
    spec: &HarnessSpec,
    bin: String,
    command: Vec<String>,
    output_format: OutputFormat,
    capture: &Capture,
    schema: Option<&Schema>,
    attempts: u32,
    prompt: Option<String>,
    model: Option<String>,
) -> RunResult {
    // A timeout does not invalidate bytes already captured from the process.
    // Normalize them best-effort exactly like an exited run. SpawnError retains
    // its existing null-signal semantics because a failed wait cannot establish
    // that its captured output is complete or trustworthy.
    // A run cut short still gets its captured bytes normalized: the same rule a
    // timeout follows, for the same reason — output already produced is evidence,
    // and discarding it would be the only thing that made the stop lossy.
    let normalize_capture = matches!(
        capture.status,
        Status::Ok | Status::Nonzero | Status::Timeout | Status::Cancelled
    );
    let extracted = normalize_capture
        .then(|| normalize::extract(&capture.stdout, output_format))
        .flatten();
    let (text, text_source) = match &extracted {
        Some(e) => (Some(e.text.clone()), Some(e.source.clone())),
        None => (None, None),
    };
    // Structured output: re-derive the validated value for the final capture so
    // the report and the retry loop share one source of truth. Null fields when
    // no schema was requested, or when the run produced nothing to validate.
    let (structured, schema_valid, schema_attempts, schema_error) = match schema {
        Some(sch) if matches!(capture.status, Status::Ok | Status::Nonzero) => {
            let answer = extracted.as_ref().map(|e| e.text.as_str()).unwrap_or("");
            let check = structured::check(sch, spec.native_schema, answer, &capture.stdout);
            if check.is_valid() {
                (check.value, Some(true), Some(attempts), None)
            } else {
                (
                    check.value,
                    Some(false),
                    Some(attempts),
                    Some(check.errors.join("; ")),
                )
            }
        }
        // Ran under a schema but timed out / could not be spawned: nothing to
        // validate, but the attempt count is still meaningful.
        Some(_) => (None, None, Some(attempts), None),
        None => (None, None, None, None),
    };
    let usage_reading = normalize_capture
        .then(|| signals::extract_usage(&capture.stdout))
        .flatten();
    let (mut usage, usage_source) = match usage_reading {
        Some(r) => (r.usage, Some(r.source)),
        None => (Usage::default(), None),
    };
    signals::apply_model_price(&mut usage, spec.id, model.as_deref());
    let session_id = normalize_capture
        .then(|| signals::extract_session(&capture.stdout))
        .flatten();
    let events_reading = normalize_capture
        .then(|| events::extract_events(&capture.stdout, output_format))
        .flatten();
    let (mut normalized_events, events_source) = match events_reading {
        Some(r) => (Some(r.events), Some(r.source)),
        None => (None, None),
    };
    let timing = spec
        .telemetry
        .filter(|telemetry| telemetry.format == output_format)
        .map(|telemetry| {
            let mut no_events = Vec::new();
            let timed_events = normalized_events.as_mut().unwrap_or(&mut no_events);
            events::apply_observed_timing(
                timed_events,
                &capture.stdout_observations,
                capture.status,
                capture.duration_ms,
                telemetry.trace,
            )
        });
    let observed_tool_ms = if timing.is_none() {
        let mut no_events = Vec::new();
        let timed_events = normalized_events.as_mut().unwrap_or(&mut no_events);
        events::apply_stdout_observed_tool_timing(
            timed_events,
            &capture.stdout_observations,
            capture.status,
            capture.duration_ms,
        )
    } else {
        None
    };
    // Whether this harness/format pair advertises a provider trace at all: only
    // then is an incomplete one a *shortfall* whose invocation bounds are worth
    // preserving. A harness that declares no trace has nothing to fall short of.
    let attempted_trace = timing.is_some();
    let telemetry = timing
        .filter(|timing| timing.trace_complete)
        // The runner mints both bounds, so parsing them is a boundary check
        // rather than a doubt — the same one the partial arm below applies.
        // Text that is not a millisecond-precision UTC instant yields no
        // telemetry rather than a measurement nothing measured.
        .and_then(|timing| {
            capture.started_at.parse().ok().map(|started_at| {
                crate::domain::report::ExecutionTelemetry::ProviderMeasured {
                    started_at,
                    finished_at: capture
                        .finished_at
                        .as_deref()
                        .and_then(|text| text.parse().ok()),
                    model_ms: timing.model_ms,
                    tool_ms: timing.tool_ms,
                    time_to_first_token_ms: timing.time_to_first_token_ms,
                }
            })
        })
        .or_else(|| {
            observed_tool_ms.map(|observed_tool_ms| {
                crate::domain::report::ExecutionTelemetry::StdoutObserved {
                    tool_ms: observed_tool_ms,
                }
            })
        })
        .or_else(|| {
            // A trace-capable run that failed before its trace completed. The
            // provider/tool split was never derivable, but the instant the runner
            // itself watched the invocation start is measured, not inferred — so
            // it is preserved rather than discarded with the split.
            // The runner mints this instant, so the parse is a boundary check
            // rather than a doubt: text that is not a canonical UTC instant
            // yields no telemetry rather than a claim about when the run began.
            // `attempts == 0` is the one case with nothing to preserve: the job
            // was cancelled while still queued, so it has no invocation to bound
            // and saying when it "started" would be the fabrication this arm
            // exists to avoid.
            (attempts > 0 && attempted_trace && crate::domain::history::run_failed(capture.status))
                .then(|| capture.started_at.parse().ok())
                .flatten()
                .map(
                    |started_at| crate::domain::report::ExecutionTelemetry::PartialInvocation {
                        started_at,
                    },
                )
        });
    // A deferred-tool dead-end: the harness completed cleanly (exit 0) but only
    // *deferred* a builtin tool call instead of running it (Claude Code bridge
    // deployments — issue #1114). It exits 0, so it is not caught by the non-zero
    // classification below; detect it from the output shape and give it a distinct
    // `tool_deferred` kind + an actionable `error`, rather than letting it look
    // like an empty/invalid answer. Checked for any run that produced output.
    let deferred = match capture.status {
        Status::Ok | Status::Nonzero => signals::detect_deferred_tool(&capture.stdout),
        _ => None,
    };
    // Classify only an actual non-zero run: timeouts/spawn failures already carry
    // a oneharness-generated `error`, and `status` explains them. A detected
    // deferral is more specific and actionable, so it wins over a coarse match.
    let provider_failure = match capture.status {
        Status::Ok | Status::Nonzero => {
            signals::detect_harness_provider_failure(failure_dialect(spec), &capture.stdout)
        }
        _ => None,
    };
    let failure = match (&deferred, provider_failure, capture.status) {
        (Some(_), _, _) => Some(signals::FailureReading {
            kind: signals::FailureKind::ToolDeferred,
            source: "stdout".to_string(),
            detail: None,
        }),
        (None, Some(failure), _) => Some(failure),
        (None, None, Status::Nonzero) => signals::classify_harness_failure(
            failure_dialect(spec),
            &capture.stdout,
            &capture.stderr,
        ),
        (None, None, _) => None,
    };
    let (failure_kind, failure_kind_source, failure_detail) = match failure {
        Some(f) => (Some(f.kind), Some(f.source), f.detail),
        None => (None, None, None),
    };
    // A deferral produced no answer, so surface an actionable `error` in place of
    // the harness's (absent) one — even though the process exited 0.
    let error = match &deferred {
        Some(d) => Some(deferred_tool_error(spec.id, d.tool.as_deref())),
        None if capture.status == Status::Timeout => capture
            .error
            .as_ref()
            .map(|why| format!("harness `{}` hit its oneharness deadline: {why}", spec.id)),
        // A refusal the provider stated in machine-readable terms says more than
        // the exit code does, and a non-zero run has no oneharness-generated
        // `error` of its own to displace. Carrying it here is what puts the cause
        // in front of the caller — and, through the fallback block, in front of a
        // supervisor — instead of leaving it in stdout to be rediscovered.
        None => failure_detail
            .as_ref()
            .map(|detail| refusal_error(spec.id, detail))
            .or_else(|| capture.error.clone()),
    };
    RunResult {
        harness: spec.id.to_string(),
        variant: None,
        harness_id: spec.id.to_string(),
        bin,
        available: true,
        status: capture.status,
        prompt,
        model,
        observed_model: None,
        exit_code: capture.exit_code,
        duration_ms: capture.duration_ms,
        telemetry,
        command,
        output_format,
        text,
        text_source,
        usage,
        usage_source,
        session_id,
        events: normalized_events,
        events_source,
        structured,
        schema_valid,
        schema_attempts,
        schema_error,
        failure_kind,
        work: None,
        failure_kind_source,
        stdout: capture.stdout.clone(),
        stderr: capture.stderr.clone(),
        error,
    }
    .with_work_evidence()
}

/// The `error` for a refusal the provider stated in machine-readable terms:
/// oneharness names the harness that refused, then quotes the provider's own
/// object **verbatim** (`{"input_error_code":"input_too_large","max_chars":…}`).
/// Quoted rather than paraphrased because a caller acts on the code and the
/// numbers — shard the input to `max_chars`, pick a candidate with the room —
/// and a rewording would be oneharness inventing a contract the provider owns.
fn refusal_error(harness_id: &str, detail: &str) -> String {
    format!("harness `{harness_id}` refused the request before running it: {detail}")
}

/// The actionable `error` for a deferred-tool dead-end (issue #1114): the harness
/// deferred `tool` (when named) instead of executing it, so the run produced
/// nothing. Names the cause (a bridged/managed deployment) and the way out.
fn deferred_tool_error(harness_id: &str, tool: Option<&str>) -> String {
    let what = match tool {
        Some(name) => format!("a `{name}` tool call"),
        None => "a builtin tool call".to_string(),
    };
    format!(
        "harness `{harness_id}` deferred {what} instead of executing it, so the run produced no \
         result — you appear to be in a bridged/managed deployment where builtin tools are \
         deferred (empty `tengu_non_deferrable_builtins`). Tool-using runs need a deployment that \
         executes tools inline: run from a standalone environment/CI, or select a harness that \
         executes tools inline."
    )
}

/// Everything needed to (re)build one harness's argv, retained so the
/// structured-output loop can re-run it with a feedback prompt. Holds owned
/// data because the retry closure runs on the runner's worker threads.
struct HarnessPlan {
    spec: &'static HarnessSpec,
    bin: String,
    model: Option<String>,
    system: Option<String>,
    resume: Option<String>,
    fork: bool,
    mode: PermissionMode,
    output_format: OutputFormat,
    /// The harness takes the schema through a native flag (so the prompt is left
    /// alone); otherwise the schema instruction is appended to the prompt.
    native: bool,
    base_prompt: String,
    /// Config `args` + CLI passthrough, appended verbatim after the built argv.
    extra: Vec<String>,
    /// Path to a temp file holding the system prompt, when it is large enough to
    /// deliver off the argv on a harness with a system-file flag (Claude Code).
    /// `None` keeps the system inline. Set by the command layer before spawning.
    system_file: Option<String>,
    /// How the user prompt reaches the harness. [`PromptDelivery::Stdin`] (a
    /// large prompt on a stdin-capable harness) makes `build` omit the
    /// positional and return the assembled prompt as [`BuiltCommand::stdin`];
    /// [`PromptDelivery::ControlStream`] (a `--control` run) makes it the first
    /// frame the control channel writes. [`PromptDelivery::Argv`] keeps the argv
    /// byte-identical to an ordinary run.
    delivery: PromptDelivery,
    /// The identity's resolved `plugins` opt-in ([`FileConfig::plugins_for`]),
    /// carried apart from `extra` so a variant's own `args` cannot drop it.
    plugins: bool,
    /// The identity's resolved `apps` opt-in ([`FileConfig::apps_for`]),
    /// carried apart from `extra` like `plugins`.
    apps: bool,
    /// The identity's resolved `analytics` ([`FileConfig::analytics_for`]),
    /// carried apart from `extra` like `plugins`.
    analytics: bool,
    /// The shared model catalog this launch reads
    /// ([`model_catalog::resolve`]), or `None` for Codex's own fetch.
    model_catalog: Option<PathBuf>,
}

/// The result of building one attempt: the argv to spawn and, when the prompt is
/// delivered off the argv, the bytes to pipe to stdin.
struct BuiltCommand {
    argv: Vec<String>,
    stdin: Option<String>,
    /// The fully assembled prompt (mode instruction + schema/retry additions).
    /// A control-enabled run delivers it as the first stdin frame rather than an
    /// argv positional, so it needs the same text the argv would have carried.
    prompt: String,
}

impl HarnessPlan {
    /// Build the argv (and any stdin payload) for one attempt. `schema` drives
    /// structured output: non-native harnesses get the schema instruction appended
    /// to the prompt, native ones get it on the flag. `feedback` (the prior answer
    /// + validation errors) is appended on a retry so the model can correct itself.
    ///
    /// Under [`PromptDelivery::Stdin`] the assembled prompt is returned as
    /// [`BuiltCommand::stdin`] instead of riding the argv (the adapter omits the
    /// positional), with the system prompt folded in for a harness whose system
    /// rides the prompt ([`LargeInput::system_rides_prompt`]) — so the bytes the
    /// model sees are identical to the inline path. Under
    /// [`PromptDelivery::ControlStream`] the adapter also omits the positional,
    /// and [`BuiltCommand::prompt`] is what the control channel writes as its
    /// first frame.
    fn build(&self, schema: Option<&Schema>, feedback: Option<(&str, &[String])>) -> BuiltCommand {
        let mut prompt = self.base_prompt.clone();
        // A mode that synthesizes a behavioral posture from an instruction
        // (Codex's `plan`) prepends it so it frames the task. Single-line +
        // space-joined, matching the structured-output convention, so the prompt
        // argument stays newline-free for a `.cmd`-shim harness on Windows.
        if let Some(instruction) = self.spec.mode(self.mode).and_then(|m| m.instruction) {
            prompt = format!("{instruction} {prompt}");
        }
        if let Some(sch) = schema {
            // Join with a space, not a newline: the structured-output additions
            // must keep the prompt argument newline-free so it can still be passed
            // to a `.bat`/`.cmd` harness shim on Windows (Rust's std rejects an
            // argument with `\n`/`\r` there — see `structured::prompt_instruction`).
            if !self.native {
                prompt.push(' ');
                prompt.push_str(&structured::prompt_instruction(sch.as_text()));
            }
            if let Some((previous, errors)) = feedback {
                prompt.push(' ');
                prompt.push_str(&structured::retry_instruction(
                    sch.as_text(),
                    previous,
                    errors,
                ));
            }
        }
        // When the prompt rides stdin, assemble the exact payload the adapter would
        // have inlined: for a harness whose system rides the prompt, prepend the
        // system (mirroring `prompt_with_system`); otherwise the system is carried
        // separately (Claude's file flag, Goose's inline `--system`).
        let stdin = if self.delivery.is_stdin_blob() {
            Some(if self.spec.large_input.system_rides_prompt {
                harness::prompt_with_system_text(self.system.as_deref(), &prompt)
            } else {
                prompt.clone()
            })
        } else {
            None
        };
        let ctx = BuildCtx {
            bin: &self.bin,
            prompt: &prompt,
            model: self.model.as_deref(),
            system: self.system.as_deref(),
            resume: self.resume.as_deref(),
            fork: self.fork,
            mode: self.mode,
            output_format: self.output_format,
            schema: if self.native {
                schema.map(Schema::as_text)
            } else {
                None
            },
            system_file: self.system_file.as_deref(),
            delivery: self.delivery,
            plugins: self.plugins,
            apps: self.apps,
            analytics: self.analytics,
            model_catalog: self.model_catalog.as_deref(),
        };
        let argv = harness::argv_with_caller_args(self.spec, &ctx, &self.extra);
        let prompt =
            if self.delivery.is_control_stream() && self.spec.large_input.system_rides_prompt {
                harness::prompt_with_system_text(self.system.as_deref(), &prompt)
            } else {
                prompt
            };
        BuiltCommand {
            argv,
            stdin,
            prompt,
        }
    }
}

/// Decide whether to re-run one harness under the structured-output loop. Returns
/// the next attempt's argv when the response failed validation and retries
/// remain, else `None`. `attempt` is the number of runs completed so far.
fn retry_decision(
    plan: &HarnessPlan,
    schema: &Schema,
    attempt: u32,
    max_retries: u32,
    capture: &Capture,
) -> Option<NextRun> {
    // Only a run that produced output can be validated; a timeout / spawn error
    // is not a validation failure and re-running it would just burn the budget.
    if !matches!(capture.status, Status::Ok | Status::Nonzero) {
        return None;
    }
    // A deferred-tool dead-end (issue #1114) is deterministic — the deployment
    // will defer again on every retry — so re-prompting only burns real model
    // calls without ever producing a value. Stop immediately; the result carries
    // the `tool_deferred` classification either way.
    if signals::detect_deferred_tool(&capture.stdout).is_some() {
        return None;
    }
    let answer = normalize::extract(&capture.stdout, plan.output_format).map(|e| e.text);
    let check = structured::check(
        schema,
        plan.spec.native_schema,
        answer.as_deref().unwrap_or(""),
        &capture.stdout,
    );
    if check.is_valid() || attempt > max_retries {
        return None;
    }
    // Feed back what the harness actually said (its extracted answer, else the
    // raw stdout) so the correction prompt is grounded in its own output. The
    // rebuild reuses the unit's delivery (a stdin prompt re-prompts via stdin).
    let previous = match &answer {
        Some(text) if !text.is_empty() => text.clone(),
        _ => capture.stdout.trim().to_string(),
    };
    let built = plan.build(Some(schema), Some((&previous, &check.errors)));
    Some(NextRun {
        argv: built.argv,
        stdin: built.stdin,
    })
}

/// Initial and maximum delay for same-candidate Codex overload retries. The
/// exponential is saturated and capped, so every wait remains bounded even for
/// an unusually large configured retry count.
const SERVER_OVERLOADED_BACKOFF_INITIAL_MS: u64 = 100;
const SERVER_OVERLOADED_BACKOFF_MAX_MS: u64 = 1_000;
const SERVER_OVERLOADED_BACKOFF_INITIAL: Duration =
    Duration::from_millis(SERVER_OVERLOADED_BACKOFF_INITIAL_MS);
const SERVER_OVERLOADED_BACKOFF_MAX: Duration =
    Duration::from_millis(SERVER_OVERLOADED_BACKOFF_MAX_MS);

fn server_overloaded_retry_after_backoff(
    plan: &HarnessPlan,
    attempt: u32,
    max_retries: u32,
    capture: &Capture,
    schema: Option<&Schema>,
) -> Option<NextRun> {
    if attempt > max_retries || !is_server_overloaded_without_work(plan, capture) {
        return None;
    }
    server_overloaded_backoff(attempt);
    let built = plan.build(schema, None);
    Some(NextRun {
        argv: built.argv,
        stdin: built.stdin,
    })
}

fn is_server_overloaded_without_work(plan: &HarnessPlan, capture: &Capture) -> bool {
    if !matches!(capture.status, Status::Ok | Status::Nonzero) {
        return false;
    }
    let failure =
        signals::detect_harness_provider_failure(failure_dialect(plan.spec), &capture.stdout);
    if failure.map(|failure| failure.kind) != Some(signals::FailureKind::ServerOverloaded) {
        return false;
    }
    let billed_work = signals::extract_usage(&capture.stdout)
        .is_some_and(|reading| reading.usage.reports_billed_work());
    let used_tools = events::extract_events(&capture.stdout, plan.output_format)
        .is_some_and(|reading| reading.events.iter().any(ActionEvent::is_tool_activity));
    // On Codex, normalized text comes only from an agent-message item; the
    // terminal overload object's diagnostic is not an answer. Retrying after
    // an emitted answer could repeat work even when usage was not reported.
    let produced_answer = normalize::extract(&capture.stdout, plan.output_format)
        .is_some_and(|reading| !reading.text.is_empty());
    !billed_work && !used_tools && !produced_answer
}

fn server_overloaded_backoff(attempt: u32) {
    std::thread::sleep(server_overloaded_backoff_duration(attempt));
}

fn server_overloaded_backoff_duration(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(31);
    let millis = SERVER_OVERLOADED_BACKOFF_INITIAL
        .as_millis()
        .saturating_mul(1u128 << shift)
        .min(SERVER_OVERLOADED_BACKOFF_MAX.as_millis()) as u64;
    Duration::from_millis(millis)
}

/// Load and compile the structured-output schema, if one was requested. A
/// `--schema` path is relative to the process's working directory; a config
/// `schema_file` is relative to the project directory (where config was
/// discovered), mirroring how each source is written.
fn load_schema(
    args: &RunRequest,
    cfg: &crate::domain::config::FileConfig,
    project_start: &std::path::Path,
) -> Result<Option<Schema>, OneharnessError> {
    let path = if let Some(p) = &args.schema {
        p.clone()
    } else if let Some(rel) = &cfg.schema_file {
        project_start.join(rel)
    } else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(&path).map_err(|source| OneharnessError::SchemaFile {
        path: path.display().to_string(),
        source,
    })?;
    Schema::compile(&text)
        .map(Some)
        .map_err(OneharnessError::Schema)
}

/// A harness "failed" when it ran and did not exit cleanly, when it could not be
/// spawned, when it was cancelled before finishing (no answer, whoever asked for
/// the stop), when — under `--require-available` — it was skipped as missing, when
/// a structured-output run never produced a schema-conforming answer (a run you
/// asked for JSON from and didn't get is a failure, regardless of exit code), or
/// when it dead-ended by deferring a tool call (`tool_deferred`) — a clean exit
/// that nonetheless did no useful work (issue #1114).
fn is_failure(
    status: Status,
    available: bool,
    require_available: bool,
    schema_valid: Option<bool>,
    failure_kind: Option<signals::FailureKind>,
) -> bool {
    if schema_valid == Some(false) {
        return true;
    }
    if failure_kind.is_some() {
        return true;
    }
    match status {
        Status::Nonzero | Status::Timeout | Status::SpawnError | Status::Cancelled => true,
        Status::Skipped => require_available && !available,
        Status::Ok | Status::Planned => false,
    }
}

fn exit_code(results: &[RunResult], require_available: bool) -> i32 {
    let failed = results.iter().any(|r| {
        is_failure(
            r.status,
            r.available,
            require_available,
            r.schema_valid,
            r.failure_kind,
        )
    });
    if failed {
        EXIT_FAILURE
    } else {
        EXIT_OK
    }
}

/// Resolve the prompt list, in order: every `--prompt` value, then every
/// `--prompt-file` (each file read whole as one prompt; `-` reads stdin once).
/// More than one prompt makes this a batch run. Empty is a usage error.
fn resolve_prompts(args: &RunRequest) -> Result<Vec<String>, OneharnessError> {
    let mut prompts: Vec<String> = args.prompt.clone();
    // stdin can be consumed only once; reading it twice would block/return empty.
    let stdin_count = args.prompt_file.iter().filter(|p| *p == "-").count();
    if stdin_count > 1 {
        return Err(OneharnessError::MultipleStdinPrompts { count: stdin_count });
    }
    for path in &args.prompt_file {
        if path == "-" {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).map_err(|source| {
                OneharnessError::PromptFile {
                    path: "<stdin>".to_string(),
                    source,
                }
            })?;
            prompts.push(buf);
        } else {
            let text =
                std::fs::read_to_string(path).map_err(|source| OneharnessError::PromptFile {
                    path: path.clone(),
                    source,
                })?;
            prompts.push(text);
        }
    }
    if prompts.is_empty() {
        return Err(OneharnessError::NoPrompt);
    }
    Ok(prompts)
}

/// Resolve the effective system prompt from the mutually-exclusive `--system`
/// (inline argv) and `--system-file` (file, or `-` for stdin). `--system-file` is
/// the argv-limit escape hatch mirroring `--prompt-file`: a system prompt too
/// large for a single argv string trips `E2BIG` at spawn, so it is read from a
/// file instead. Returns `None` when neither flag is set, so the caller's config
/// `system` fallback applies. The `-`/stdin collision with `--prompt-file -` is
/// guarded before any read, so this never double-consumes stdin.
fn resolve_system(args: &RunRequest) -> Result<Option<String>, OneharnessError> {
    if let Some(text) = &args.system {
        return Ok(Some(text.clone()));
    }
    let Some(path) = &args.system_file else {
        return Ok(None);
    };
    let text = if path == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).map_err(|source| {
            OneharnessError::SystemFile {
                path: "<stdin>".to_string(),
                source,
            }
        })?;
        buf
    } else {
        std::fs::read_to_string(path).map_err(|source| OneharnessError::SystemFile {
            path: path.clone(),
            source,
        })?
    };
    Ok(Some(text))
}

/// A batch run (more than one prompt) fans **one** harness over the prompts so
/// they share a provider cache prefix, which is per harness/model/tools — so it
/// requires exactly one selected harness — and is a fresh fan-out rather than a
/// session continuation, so it cannot combine with `--resume`/`--fork`. Both are
/// usage errors caught before anything spawns.
fn validate_batch(
    specs: &[&'static HarnessSpec],
    resume_or_fork: bool,
) -> Result<(), OneharnessError> {
    if specs.len() != 1 {
        return Err(OneharnessError::BatchMultipleHarnesses {
            count: specs.len(),
            selected: specs.iter().map(|s| s.id).collect::<Vec<_>>().join(", "),
        });
    }
    if resume_or_fork {
        return Err(OneharnessError::BatchResume);
    }
    Ok(())
}

/// Write each result's raw stdout/stderr to `<dir>/<harness>.{stdout,stderr}`.
/// Lets consumers (e.g. allowlister's e2e scripts) read the transcript from files
/// without a JSON parser, preserving their existing `$stream`/`$stream.err`
/// contract.
fn write_output_dir(dir: &std::path::Path, results: &[RunResult]) -> Result<(), OneharnessError> {
    std::fs::create_dir_all(dir).map_err(|source| OneharnessError::OutputDir {
        path: dir.display().to_string(),
        source,
    })?;
    // On a batch run every result is the same harness, so a bare `<harness>`
    // stem would overwrite itself. Disambiguate with the index whenever a harness
    // id repeats; unique ids (the ordinary `--all` case) keep their plain name.
    let counts = results.iter().fold(
        std::collections::HashMap::<&str, usize>::new(),
        |mut m, r| {
            *m.entry(r.harness.as_str()).or_default() += 1;
            m
        },
    );
    for (index, result) in results.iter().enumerate() {
        let stem = if counts.get(result.harness.as_str()).copied().unwrap_or(0) > 1 {
            format!("{}-{index}", result.harness)
        } else {
            result.harness.clone()
        };
        for (suffix, contents) in [("stdout", &result.stdout), ("stderr", &result.stderr)] {
            let path = dir.join(format!("{stem}.{suffix}"));
            std::fs::write(&path, contents).map_err(|source| OneharnessError::OutputDir {
                path: path.display().to_string(),
                source,
            })?;
        }
    }
    Ok(())
}

/// Resume is stateful and single-session: it requires exactly one selected
/// harness, and that harness must support continuation. Both are usage errors,
/// caught before any process is spawned (`--all` is already excluded by clap).
fn validate_resume(
    resume: Option<&str>,
    specs: &[&'static HarnessSpec],
) -> Result<(), OneharnessError> {
    if resume.is_none() {
        return Ok(());
    }
    if specs.len() != 1 {
        return Err(OneharnessError::ResumeMultipleHarnesses {
            count: specs.len(),
            selected: specs.iter().map(|s| s.id).collect::<Vec<_>>().join(", "),
        });
    }
    let spec = specs[0];
    if !spec.supports_resume {
        return Err(OneharnessError::ResumeUnsupported {
            id: spec.id.to_string(),
            supported: harness::all()
                .iter()
                .filter(|s| s.supports_resume)
                .map(|s| s.id)
                .collect::<Vec<_>>()
                .join(", "),
        });
    }
    Ok(())
}

/// `--fork` branches a new session from the resumed one instead of appending. It
/// implies `--resume` (clap-enforced), so the single-harness constraint is already
/// guaranteed by [`validate_resume`]; the only extra check is that the selected
/// harness can actually fork — otherwise it is a usage error, never a silent
/// linear resume. Caught before any process is spawned.
fn validate_fork(fork: bool, specs: &[&'static HarnessSpec]) -> Result<(), OneharnessError> {
    if !fork {
        return Ok(());
    }
    // clap guarantees `--fork` implies `--resume`, and `validate_resume` already
    // proved exactly one harness; guard defensively all the same.
    let Some(spec) = specs.first() else {
        return Ok(());
    };
    if !spec.supports_fork {
        return Err(OneharnessError::ForkUnsupported {
            id: spec.id.to_string(),
            supported: harness::all()
                .iter()
                .filter(|s| s.supports_fork)
                .map(|s| s.id)
                .collect::<Vec<_>>()
                .join(", "),
        });
    }
    Ok(())
}

/// Resolve the effective approval mode. Precedence, highest first: CLI `--mode`,
/// then the CLI `--bypass` / `--no-bypass` shorthands, then config `mode`, then
/// the legacy config `bypass` boolean, then the built-in default — `default`
/// (the harness's normal permission posture, mapped to its cleanest
/// non-interactive variant), *not* bypass: bypass is the opt-in.
fn resolve_mode(args: &RunRequest, cfg: &crate::domain::config::FileConfig) -> PermissionMode {
    if let Some(m) = args.mode {
        m
    } else if let Some(m) = cfg.mode {
        m
    } else {
        // Legacy `bypass = true/false` maps to bypass/default; unset → default.
        cfg.bypass
            .map(PermissionMode::from_bypass)
            .unwrap_or(PermissionMode::Default)
    }
}

/// Refuse — before any process is spawned — a mode a selected harness *cannot
/// express*: there is no command to build for it, so it is a loud usage error
/// ([`OneharnessError::ModeUnsupported`]) rather than a silent downgrade. A mode
/// that is supported but might *block on a prompt* headlessly is not refused
/// here — see [`hang_prone`] (it is warned about and run with the approval-wait
/// safety deadline). Reports the first offending harness, mirroring `validate_resume`.
fn validate_modes(
    mode: PermissionMode,
    specs: &[&'static HarnessSpec],
) -> Result<(), OneharnessError> {
    for spec in specs {
        if spec.mode(mode).is_none() {
            return Err(OneharnessError::ModeUnsupported {
                id: spec.id.to_string(),
                mode: mode.as_str().to_string(),
                supported: spec
                    .modes
                    .iter()
                    .map(|m| m.mode.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
    }
    Ok(())
}

/// Refuse — before anything spawns — a reasoning/effort setting for a selected
/// harness that has no headless argv surface for it ([`HarnessSpec::reasoning`]
/// is `None`): there is nothing to deliver it through, so it is a loud usage
/// error ([`OneharnessError::ReasoningUnsupported`]) rather than a silent drop.
/// Resolves the effective value per harness exactly as the run does (CLI
/// `--reasoning` beats config `[harness.<id>]`/top-level `reasoning`), so a
/// value scoped to a capable harness never trips one that isn't selected for it.
fn validate_reasoning(
    args: &RunRequest,
    cfg: &crate::domain::config::FileConfig,
    specs: &[&'static HarnessSpec],
) -> Result<(), OneharnessError> {
    for spec in specs {
        let set = args.reasoning.is_some() || cfg.reasoning_for(spec.id).is_some();
        if set && spec.reasoning.is_none() {
            return Err(OneharnessError::ReasoningUnsupported {
                id: spec.id.to_string(),
                supported: harness::all()
                    .iter()
                    .filter(|h| h.reasoning.is_some())
                    .map(|h| h.id)
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
    }
    Ok(())
}

/// The selected harnesses for which `mode` is supported but would block on an
/// interactive approval prompt headlessly (`ModeHeadless::Hangs`). The caller
/// warns about each (unless `--permit-prompts`) but still runs them with the
/// approval-wait safety deadline when the caller omitted a timeout.
fn hang_prone(mode: PermissionMode, specs: &[&'static HarnessSpec]) -> Vec<&'static str> {
    specs
        .iter()
        .filter(|spec| {
            spec.mode(mode)
                .is_some_and(|m| m.headless == ModeHeadless::Hangs)
        })
        .map(|spec| spec.id)
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimeoutPolicy {
    Ordinary,
    ApprovalWaitSafety,
}

fn timeout_policy(spec: &HarnessSpec, mode: PermissionMode) -> TimeoutPolicy {
    if spec
        .mode(mode)
        .is_some_and(|mode| mode.headless == ModeHeadless::Hangs)
    {
        TimeoutPolicy::ApprovalWaitSafety
    } else {
        TimeoutPolicy::Ordinary
    }
}

fn effective_timeout(
    requested: Option<u64>,
    policy: TimeoutPolicy,
) -> Result<Duration, OneharnessError> {
    let seconds = requested.unwrap_or(match policy {
        TimeoutPolicy::Ordinary => 0,
        TimeoutPolicy::ApprovalWaitSafety => APPROVAL_WAIT_TIMEOUT_SECS,
    });
    let timeout = Duration::from_secs(seconds);
    if !timeout.is_zero() && Instant::now().checked_add(timeout).is_none() {
        return Err(OneharnessError::TimeoutOutOfRange { seconds });
    }
    Ok(timeout)
}

fn parse_env(values: &[String]) -> Result<Vec<(String, String)>, OneharnessError> {
    values
        .iter()
        .map(|v| {
            v.split_once('=')
                .filter(|(k, _)| !k.is_empty())
                .map(|(k, val)| (k.to_string(), val.to_string()))
                .ok_or_else(|| OneharnessError::BadEnv(v.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(status: Status, available: bool) -> RunResult {
        RunResult {
            harness: "x".into(),
            variant: None,
            harness_id: "x".into(),
            bin: "x".into(),
            available,
            status,
            prompt: None,
            model: None,
            observed_model: None,
            exit_code: None,
            duration_ms: None,
            telemetry: None,
            command: vec![],
            output_format: OutputFormat::Text,
            text: None,
            text_source: None,
            usage: Usage::default(),
            usage_source: None,
            session_id: None,
            events: None,
            events_source: None,
            structured: None,
            schema_valid: None,
            schema_attempts: None,
            schema_error: None,
            failure_kind: None,
            work: None,
            failure_kind_source: None,
            stdout: String::new(),
            stderr: String::new(),
            error: None,
        }
    }

    #[test]
    fn a_stopped_candidate_accounts_for_itself_from_whatever_it_left() {
        // `error` first: oneharness composed it (a deadline, a refusal object)
        // and it already says the most.
        let mut r = result(Status::Timeout, true);
        r.error = Some("harness `x` hit its oneharness deadline: 1s".into());
        r.stderr = "ignored when error speaks\n".into();
        assert_eq!(
            failure_account(&r),
            "harness `x` hit its oneharness deadline: 1s"
        );
        assert_eq!(failure_envelope(&r), "status timeout");

        // A conforming-answer failure exits 0 with no error: the schema verdict
        // is the cause.
        let mut r = result(Status::Ok, true);
        r.exit_code = Some(0);
        r.schema_error = Some("\"answer\" is a required property".into());
        assert_eq!(
            failure_account(&r),
            "the answer did not conform to the schema: \"answer\" is a required property"
        );
        assert_eq!(failure_envelope(&r), "status ok, exit 0");

        // A plain non-zero exit composes no `error`, so the harness's last
        // words on stderr are what there is — blank lines dropped, the rest
        // folded onto one line.
        let mut r = result(Status::Nonzero, true);
        r.exit_code = Some(2);
        r.failure_kind = Some(signals::FailureKind::ModelNotFound);
        r.stderr = "warning: first\nmodel not found:\n\t gpt-9  \n\n".into();
        assert_eq!(
            failure_account(&r),
            "(stderr) warning: first model not found: gpt-9"
        );
        assert_eq!(
            failure_envelope(&r),
            "status nonzero, exit 2, failure_kind model_not_found"
        );

        // A multi-line diagnostic that fits is carried whole: the crash's own
        // message, not the footer a Node process signs off with.
        let mut r = result(Status::Nonzero, true);
        r.stderr = concat!(
            "(node:41) Warning: deprecated\n",
            "Error: ENOENT: no such file or directory, open '/home/u/.claude/settings.json'\n",
            "    at Object.<anonymous> (/usr/lib/node_modules/claude/cli.js:12:11)\n",
            "\n",
            "Node.js v22.14.0\n",
        )
        .into();
        assert_eq!(
            failure_account(&r),
            "(stderr) (node:41) Warning: deprecated Error: ENOENT: no such file or directory, \
             open '/home/u/.claude/settings.json' at Object.<anonymous> \
             (/usr/lib/node_modules/claude/cli.js:12:11) Node.js v22.14.0"
        );

        // One that does not fit keeps its tail — the lines nearest the end —
        // and says that earlier lines were dropped.
        let mut r = result(Status::Nonzero, true);
        let frames = (0..20)
            .map(|n| format!("  File \"/srv/agent/step{n}.py\", line {n}, in run\n"))
            .collect::<String>();
        r.stderr = format!("Traceback (most recent call last):\n{frames}KeyError: 'session'\n");
        let account = failure_account(&r);
        assert!(account.starts_with("(stderr) …File \""), "{account}");
        assert!(account.ends_with("KeyError: 'session'"), "{account}");
        assert!(
            account.chars().count() <= FAILURE_ACCOUNT_CHARS,
            "{account}"
        );
        assert!(
            !account.contains("Traceback"),
            "the head was dropped: {account}"
        );

        // Then stdout, where a harness that wrote nothing to stderr may have
        // left an error record.
        let mut r = result(Status::Nonzero, true);
        r.stdout = "{\"type\":\"error\",\"message\":\"boom\"}\n".into();
        assert_eq!(
            failure_account(&r),
            "(stdout) {\"type\":\"error\",\"message\":\"boom\"}"
        );

        // Control characters are the harness's, not the summary's: a
        // terminal escape or a bell is dropped at this boundary.
        let mut r = result(Status::Nonzero, true);
        r.stderr = "\u{1b}[31mred\u{7} words\u{1b}[0m".into();
        assert_eq!(failure_account(&r), "(stderr) [31mred words[0m");

        // Silence is itself the finding, never an empty sentence.
        let r = result(Status::Nonzero, true);
        assert_eq!(
            failure_account(&r),
            "nothing — it wrote no error, no stderr and no stdout"
        );

        // Bounded, in characters, so the line stays one a supervisor can
        // publish; the report keeps the whole text.
        let mut r = result(Status::Nonzero, true);
        r.stderr = "é".repeat(FAILURE_ACCOUNT_CHARS + 50);
        let account = failure_account(&r);
        assert_eq!(account.chars().count(), FAILURE_ACCOUNT_CHARS + 1);
        assert!(account.ends_with('…'), "{account}");
        assert!(account.starts_with("(stderr) éé"), "{account}");
    }

    fn crush_plan() -> HarnessPlan {
        HarnessPlan {
            spec: harness::by_id("crush").unwrap(),
            bin: "crush".into(),
            model: None,
            system: None,
            resume: None,
            fork: false,
            mode: PermissionMode::Bypass,
            output_format: OutputFormat::Text,
            native: false,
            base_prompt: "p".into(),
            extra: Vec::new(),
            system_file: None,
            delivery: PromptDelivery::Argv,
            plugins: false,
            apps: false,
            analytics: false,
            model_catalog: None,
        }
    }

    fn capture(status: Status, stdout: &str) -> Capture {
        Capture {
            status,
            exit_code: None,
            duration_ms: Some(1),
            stdout: stdout.into(),
            stderr: String::new(),
            error: None,
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            finished_at: Some("2026-01-01T00:00:00.001Z".to_string()),
            stdout_observations: Vec::new(),
        }
    }

    #[test]
    fn plan_large_input_warns_and_stays_inline_when_unwired() {
        // A harness with LargeInput::NONE (a future/unverified adapter) has no
        // off-argv route, so a large prompt AND system both stay inline —
        // plan_large_input changes nothing and warns on both (the honest guard
        // against a silent E2BIG). Every real harness is wired, so this synthetic
        // no-capability spec is the only witness for the warning branches.
        let big = "x".repeat(LARGE_INPUT_THRESHOLD + 1);
        let mut plan = crush_plan();
        plan.spec = unsupported_spec();
        plan.base_prompt = big.clone();
        let spec = plan.spec;
        let mut temps = TempPromptFiles::default();
        plan_large_input(&mut plan, spec, Some(&big), 0, &mut temps).unwrap();
        assert!(
            !plan.delivery.is_stdin_blob(),
            "no stdin route → prompt stays inline"
        );
        assert!(
            plan.system_file.is_none(),
            "no file route → system stays inline"
        );
        assert!(temps.0.is_empty(), "nothing materialized");
    }

    #[test]
    fn plan_large_input_is_a_noop_for_small_prompts() {
        // Below the threshold, nothing moves off the argv even on a wired harness.
        let mut plan = crush_plan(); // crush is wired (prompt_stdin)
        let spec = plan.spec;
        let mut temps = TempPromptFiles::default();
        plan_large_input(&mut plan, spec, Some("small"), 0, &mut temps).unwrap();
        assert!(!plan.delivery.is_stdin_blob());
        assert!(temps.0.is_empty());
    }

    #[test]
    fn schema_argv_prompt_stays_newline_free_including_on_retry() {
        // Windows passes the prompt as one argv element to a `.cmd` harness shim,
        // which Rust's std refuses if it contains `\n`/`\r`. The structured-output
        // additions (instruction + retry feedback, even with a multi-line prior
        // answer) must never introduce one.
        let schema = Schema::compile(r#"{"type":"object"}"#).unwrap();
        // Prompt-based (non-native): first attempt appends the instruction.
        let plan = crush_plan();
        let argv = plan.build(Some(&schema), None).argv;
        assert!(argv.iter().all(|a| !a.contains('\n')), "{argv:?}");
        // ... and a retry with a multi-line prior answer + errors.
        let argv = plan
            .build(
                Some(&schema),
                Some((
                    "multi\nline\r\nanswer",
                    &["e1".to_string(), "e2".to_string()],
                )),
            )
            .argv;
        assert!(
            argv.iter().all(|a| !a.contains('\n') && !a.contains('\r')),
            "{argv:?}"
        );
        // Native (claude) retry: schema rides the flag, feedback rides the prompt.
        let mut native = crush_plan();
        native.spec = harness::by_id("claude-code").unwrap();
        native.native = true;
        native.output_format = OutputFormat::Json;
        let argv = native
            .build(Some(&schema), Some(("multi\nline", &["e".to_string()])))
            .argv;
        assert!(argv.iter().all(|a| !a.contains('\n')), "{argv:?}");
    }

    #[test]
    fn retry_decision_covers_valid_invalid_exhausted_and_timeout() {
        let schema = Schema::compile(
            r#"{"type":"object","required":["a"],"properties":{"a":{"type":"integer"}}}"#,
        )
        .unwrap();
        let plan = crush_plan();
        // Conforming → stop.
        assert!(retry_decision(&plan, &schema, 1, 2, &capture(Status::Ok, r#"{"a":1}"#)).is_none());
        // Non-conforming with budget left → re-run with a feedback prompt.
        let next = retry_decision(&plan, &schema, 1, 2, &capture(Status::Ok, r#"{"a":"x"}"#))
            .expect("should retry");
        assert!(next.argv.iter().any(|a| a.contains("did not conform")));
        // Budget spent → stop even though still invalid.
        assert!(retry_decision(&plan, &schema, 3, 2, &capture(Status::Ok, "{}")).is_none());
        // A timeout is not a validation failure, so it is never retried.
        assert!(retry_decision(&plan, &schema, 1, 2, &capture(Status::Timeout, "")).is_none());
        // No extractable answer falls back to the raw stdout in the feedback.
        assert!(retry_decision(&plan, &schema, 1, 2, &capture(Status::Ok, "  ")).is_some());
        // A deferred-tool dead-end is deterministic, so it is never retried even
        // with budget left (issue #1114) — re-prompting would only burn calls.
        let deferred = capture(
            Status::Ok,
            r#"{"stop_reason":"tool_deferred","result":"","deferred_tool_use":{"name":"Read"}}"#,
        );
        assert!(retry_decision(&plan, &schema, 1, 2, &deferred).is_none());
    }

    #[test]
    fn deferred_tool_error_names_the_tool_or_stays_generic() {
        // Both arms of the actionable message: a named tool is quoted; an unnamed
        // deferral falls back to "a builtin tool call" (never a fabricated name).
        let named = deferred_tool_error("claude-code", Some("Read"));
        assert!(named.contains("`Read`"), "{named}");
        assert!(named.contains("inline"), "actionable: {named}");
        let generic = deferred_tool_error("claude-code", None);
        assert!(generic.contains("a builtin tool call"), "{generic}");
        assert!(!generic.contains("``"), "no empty backtick pair: {generic}");
    }

    #[test]
    fn is_failure_treats_provider_failures_as_failure_on_a_clean_exit() {
        // A provider-declared failure can exit 0 (Status::Ok), which is normally
        // a success; the typed signal is what makes it fail so exit_code and
        // fallback orchestration see the dead-end. Without a signal, it succeeds.
        assert!(is_failure(
            Status::Ok,
            true,
            false,
            None,
            Some(signals::FailureKind::ToolDeferred)
        ));
        assert!(is_failure(
            Status::Ok,
            true,
            false,
            None,
            Some(signals::FailureKind::Auth)
        ));
        assert!(!is_failure(Status::Ok, true, false, None, None));
    }

    #[test]
    fn executed_result_classifies_a_deferred_dead_end() {
        // A clean (exit 0) capture that only deferred a tool becomes a
        // tool_deferred result with an actionable error — not an empty answer.
        let spec = harness::by_id("claude-code").unwrap();
        let cap = capture(
            Status::Ok,
            r#"{"type":"result","stop_reason":"tool_deferred","result":"",
               "deferred_tool_use":{"name":"Read"}}"#,
        );
        let r = executed_result(
            spec,
            "claude".into(),
            vec!["claude".into()],
            OutputFormat::Json,
            &cap,
            None,
            1,
            None,
            None,
        );
        assert_eq!(r.status, Status::Ok);
        assert_eq!(r.failure_kind, Some(signals::FailureKind::ToolDeferred));
        assert_eq!(r.failure_kind_source.as_deref(), Some("stdout"));
        assert!(r.error.as_deref().unwrap().contains("`Read`"));
    }

    #[test]
    fn executed_result_normalizes_a_partial_timeout_transcript() {
        // A timeout is still a timeout, but every complete JSONL record captured
        // before the deadline remains usable. The truncated tail is ignored by
        // each best-effort extractor rather than invalidating earlier evidence.
        let transcript = concat!(
            "{\"type\":\"text\",\"sessionID\":\"ses-timeout\",\"part\":",
            "{\"type\":\"text\",\"text\":\"partial answer\"}}\n",
            "{\"type\":\"tool_use\",\"sessionID\":\"ses-timeout\",\"part\":",
            "{\"type\":\"tool\",\"tool\":\"bash\",\"state\":",
            "{\"input\":{\"command\":\"echo hi\"},\"output\":\"hi\"}}}\n",
            "{\"type\":\"step_finish\",\"sessionID\":\"ses-timeout\",\"part\":",
            "{\"cost\":0.01,\"tokens\":{\"input\":12,\"output\":3,",
            "\"cache\":{\"read\":9,\"write\":4}}}}\n",
            "{\"type\":\"task_complete\",\"text\":\"emitted before exit\"}\n",
            "{\"type\":\"incomplete\"",
        );
        let cap = capture(Status::Timeout, transcript);
        let r = executed_result(
            harness::by_id("opencode").unwrap(),
            "opencode".into(),
            vec!["opencode".into()],
            OutputFormat::Json,
            &cap,
            None,
            1,
            None,
            None,
        );

        assert_eq!(r.status, Status::Timeout);
        assert_eq!(r.text.as_deref(), Some("partial answer"));
        assert_eq!(r.text_source.as_deref(), Some("json:opencode-parts"));
        assert_eq!(r.usage.input_tokens, Some(12));
        assert_eq!(r.usage.output_tokens, Some(3));
        assert_eq!(r.usage.cache_read_tokens, Some(9));
        assert_eq!(r.usage.cache_write_tokens, Some(4));
        assert_eq!(r.usage.cost_usd, Some(0.01));
        assert_eq!(r.session_id.as_deref(), Some("ses-timeout"));
        let events = r.events.expect("the complete events survive");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "message");
        assert_eq!(events[0].output.as_deref(), Some("partial answer"));
        assert_eq!(events[1].name.as_deref(), Some("bash"));
    }

    #[test]
    fn a_never_invoked_cancelled_job_claims_no_invocation_bounds() {
        // A job cancelled while still queued has `attempts == 0`. Its status is a
        // failed one on a trace-capable harness, which is exactly the shape that
        // would otherwise report invocation bounds — but there was no invocation,
        // so reporting when it "started" would be a measurement of nothing. The
        // same run *after* a spawn keeps its bounds, which is the contrast here.
        let cap = capture(Status::Cancelled, "");
        let result_for = |attempts| {
            executed_result(
                harness::by_id("opencode").unwrap(),
                "opencode".into(),
                vec!["opencode".into()],
                OutputFormat::Json,
                &cap,
                None,
                attempts,
                None,
                None,
            )
        };
        assert!(result_for(0).telemetry.is_none());
        assert!(matches!(
            result_for(1).telemetry,
            Some(crate::domain::report::ExecutionTelemetry::PartialInvocation { .. })
        ));
    }

    #[test]
    fn executed_spawn_error_keeps_null_signals_even_with_output() {
        let transcript = concat!(
            "{\"type\":\"text\",\"sessionID\":\"ses-untrusted\",\"part\":",
            "{\"type\":\"text\",\"text\":\"untrusted answer\"}}\n",
            "{\"type\":\"step_finish\",\"sessionID\":\"ses-untrusted\",\"part\":",
            "{\"cost\":0.01,\"tokens\":{\"input\":12,\"output\":3}}}\n",
        );
        let cap = capture(Status::SpawnError, transcript);
        let r = executed_result(
            harness::by_id("opencode").unwrap(),
            "opencode".into(),
            vec!["opencode".into()],
            OutputFormat::Json,
            &cap,
            None,
            1,
            None,
            None,
        );
        assert!(r.text.is_none());
        assert_eq!(r.usage, Usage::default());
        assert!(r.session_id.is_none());
        assert!(r.events.is_none());
    }

    #[test]
    fn executed_result_fills_schema_fields_per_status() {
        let schema = Schema::compile(r#"{"type":"object","required":["a"]}"#).unwrap();
        let spec = harness::by_id("crush").unwrap();
        // A conforming ok run: valid, with the value and attempt count.
        let ok = capture(Status::Ok, r#"{"a":1}"#);
        let r = executed_result(
            spec,
            "crush".into(),
            vec!["crush".into()],
            OutputFormat::Text,
            &ok,
            Some(&schema),
            1,
            Some("the batch prompt".into()),
            Some("sonnet".into()),
        );
        assert_eq!(r.schema_valid, Some(true));
        assert_eq!(r.schema_attempts, Some(1));
        assert!(r.structured.is_some());
        // The per-result prompt and model are carried through verbatim.
        assert_eq!(r.prompt.as_deref(), Some("the batch prompt"));
        assert_eq!(r.model.as_deref(), Some("sonnet"));
        // Timed out under a schema: nothing to validate, attempts still recorded.
        let to = capture(Status::Timeout, "");
        let r = executed_result(
            spec,
            "crush".into(),
            vec![],
            OutputFormat::Text,
            &to,
            Some(&schema),
            1,
            None,
            None,
        );
        assert!(r.schema_valid.is_none());
        assert_eq!(r.schema_attempts, Some(1));
        // No prompt / model recorded on an ordinary (non-batch, no-model) result.
        assert!(r.prompt.is_none());
        assert!(r.model.is_none());
        // No schema requested: every structured field is null.
        let r = executed_result(
            spec,
            "crush".into(),
            vec![],
            OutputFormat::Text,
            &ok,
            None,
            1,
            None,
            None,
        );
        assert!(r.schema_valid.is_none());
        assert!(r.schema_attempts.is_none());
        assert!(r.structured.is_none());
    }

    #[test]
    fn schema_invalid_result_is_a_failure_even_when_status_ok() {
        // A structured-output run that exited cleanly but never conformed is a
        // failure (non-zero exit), so a consumer can gate on it.
        let mut r = result(Status::Ok, true);
        r.schema_valid = Some(false);
        assert_eq!(exit_code(std::slice::from_ref(&r), false), EXIT_FAILURE);
        // A conforming (or schema-less) ok run still passes.
        r.schema_valid = Some(true);
        assert_eq!(exit_code(std::slice::from_ref(&r), false), EXIT_OK);
    }

    #[test]
    fn ok_and_skipped_pass_by_default() {
        let results = vec![result(Status::Ok, true), result(Status::Skipped, false)];
        assert_eq!(exit_code(&results, false), EXIT_OK);
    }

    #[test]
    fn nonzero_fails() {
        let results = vec![result(Status::Ok, true), result(Status::Nonzero, true)];
        assert_eq!(exit_code(&results, false), EXIT_FAILURE);
    }

    #[test]
    fn skipped_fails_under_require_available() {
        let results = vec![result(Status::Skipped, false)];
        assert_eq!(exit_code(&results, true), EXIT_FAILURE);
        assert_eq!(exit_code(&results, false), EXIT_OK);
    }

    #[test]
    fn validate_resume_ok_for_single_supported_harness() {
        let claude = harness::by_id("claude-code").unwrap();
        assert!(validate_resume(Some("sid"), &[claude]).is_ok());
        // No --resume: anything goes.
        assert!(validate_resume(None, &[claude, claude]).is_ok());
    }

    #[test]
    fn validate_resume_rejects_multiple_harnesses() {
        let claude = harness::by_id("claude-code").unwrap();
        assert!(matches!(
            validate_resume(Some("sid"), &[claude, claude]),
            Err(OneharnessError::ResumeMultipleHarnesses { count: 2, .. })
        ));
    }

    /// A no-op argv builder, for the synthetic spec below.
    fn noop_argv(_: &BuildCtx) -> Vec<String> {
        Vec::new()
    }

    /// A synthetic harness that supports neither resume nor fork, to exercise the
    /// capability guards. Every real harness now supports resume (and two support
    /// fork), so the rejection branches have no real-spec witness; this stands in
    /// without weakening the guard. Promoted to `'static` (all-const fields).
    fn unsupported_spec() -> &'static HarnessSpec {
        &HarnessSpec {
            id: "test-unsupported",
            display: "Test Unsupported",
            default_bin: "test-unsupported",
            install_hint: "",
            output_format: OutputFormat::Text,
            events_format: None,
            telemetry: None,
            supports_resume: false,
            session_formats: &[],
            supports_fork: false,
            fork_reuses_cache: false,
            sync: None,
            hooks: None,
            global_hook: None,
            gate_deny: None,
            mock_rewrite: None,
            mock_delivery: None,
            default_env: &[],
            native_schema: None,
            large_input: crate::domain::harness::LargeInput::NONE,
            modes: &[],
            reasoning: None,
            usage: crate::domain::usage::UsageSupport::NoPlanQuota,
            control: None,
            server: None,
            build_argv: noop_argv,
        }
    }

    #[test]
    fn validate_resume_rejects_unsupported_harness() {
        assert!(matches!(
            validate_resume(Some("sid"), &[unsupported_spec()]),
            Err(OneharnessError::ResumeUnsupported { .. })
        ));
    }

    #[test]
    fn validate_fork_ok_for_capable_harness_and_noop_without_fork() {
        let claude = harness::by_id("claude-code").unwrap();
        assert!(validate_fork(true, &[claude]).is_ok());
        // Without --fork, any selection passes (the flag wasn't requested).
        let codex = harness::by_id("codex").unwrap();
        assert!(validate_fork(false, &[codex]).is_ok());
        // Defensive empty-selection guard (clap prevents this in practice).
        assert!(validate_fork(true, &[]).is_ok());
    }

    #[test]
    fn validate_fork_rejects_resume_only_harness() {
        // Codex supports --resume but resumes linearly (no fork).
        let codex = harness::by_id("codex").unwrap();
        assert!(matches!(
            validate_fork(true, &[codex]),
            Err(OneharnessError::ForkUnsupported { .. })
        ));
    }

    fn run_args() -> RunRequest {
        // A minimal request with only the mode-relevant fields set; the rest keep
        // their defaults — exactly what the CLI's own conversion produces for
        // `oneharness run --prompt hi`.
        RunRequest {
            prompt: vec!["hi".to_string()],
            ..RunRequest::default()
        }
    }

    fn cfg_with(
        mode: Option<PermissionMode>,
        bypass: Option<bool>,
    ) -> crate::domain::config::FileConfig {
        crate::domain::config::FileConfig {
            mode,
            bypass,
            ..Default::default()
        }
    }

    #[test]
    fn resolve_mode_precedence() {
        let empty = cfg_with(None, None);
        // Built-in default is `default`, not bypass.
        assert_eq!(resolve_mode(&run_args(), &empty), PermissionMode::Default);
        // Legacy config bypass=true → bypass; bypass=false → default.
        assert_eq!(
            resolve_mode(&run_args(), &cfg_with(None, Some(true))),
            PermissionMode::Bypass
        );
        assert_eq!(
            resolve_mode(&run_args(), &cfg_with(None, Some(false))),
            PermissionMode::Default
        );
        // config mode beats config bypass.
        assert_eq!(
            resolve_mode(
                &run_args(),
                &cfg_with(Some(PermissionMode::Plan), Some(true))
            ),
            PermissionMode::Plan
        );
        // An explicit request mode beats every config layer. (The CLI's
        // `--bypass` / `--no-bypass` shorthands resolve into this same field
        // before the engine sees them — pinned in the binary's own conversion
        // test, since the precedence between the three is a clap-surface rule.)
        let mut a = run_args();
        a.mode = Some(PermissionMode::Edit);
        assert_eq!(
            resolve_mode(&a, &cfg_with(Some(PermissionMode::Plan), None)),
            PermissionMode::Edit
        );
        let mut a = run_args();
        a.mode = Some(PermissionMode::Default);
        assert_eq!(
            resolve_mode(&a, &cfg_with(Some(PermissionMode::Plan), None)),
            PermissionMode::Default
        );
        let mut a = run_args();
        a.mode = Some(PermissionMode::Bypass);
        assert_eq!(
            resolve_mode(&a, &cfg_with(None, Some(false))),
            PermissionMode::Bypass
        );
    }

    #[test]
    fn validate_modes_refuses_only_unsupported() {
        let crush = harness::by_id("crush").unwrap();
        let cursor = harness::by_id("cursor").unwrap();
        let claude = harness::by_id("claude-code").unwrap();
        // crush has no plan mode → unsupported (hard error, no command to build).
        assert!(matches!(
            validate_modes(PermissionMode::Plan, &[crush]),
            Err(OneharnessError::ModeUnsupported { .. })
        ));
        // A supported-but-hang-prone mode is not refused; cursor `default` is
        // valid and receives its safety deadline later in run planning.
        assert!(validate_modes(PermissionMode::Default, &[cursor]).is_ok());
        assert!(validate_modes(PermissionMode::Plan, &[claude]).is_ok());
        assert!(validate_modes(PermissionMode::Bypass, &[crush, cursor, claude]).is_ok());
    }

    #[test]
    fn hang_prone_lists_only_supported_hang_modes() {
        let cursor = harness::by_id("cursor").unwrap();
        let claude = harness::by_id("claude-code").unwrap();
        let crush = harness::by_id("crush").unwrap();
        // cursor `default` hangs; claude `default` is clean; crush doesn't support
        // plan at all (so it's not "hang-prone", it's unsupported → not listed).
        assert_eq!(hang_prone(PermissionMode::Default, &[cursor]), ["cursor"]);
        assert!(hang_prone(PermissionMode::Default, &[claude]).is_empty());
        assert!(hang_prone(PermissionMode::Plan, &[crush]).is_empty());
        assert!(hang_prone(PermissionMode::Bypass, &[cursor, claude]).is_empty());
    }

    #[test]
    fn only_an_omitted_timeout_in_a_prompt_capable_mode_gets_the_safety_deadline() {
        assert_eq!(
            effective_timeout(None, TimeoutPolicy::Ordinary).unwrap(),
            Duration::ZERO
        );
        assert_eq!(
            effective_timeout(None, TimeoutPolicy::ApprovalWaitSafety).unwrap(),
            Duration::from_secs(APPROVAL_WAIT_TIMEOUT_SECS)
        );
        assert_eq!(
            effective_timeout(Some(0), TimeoutPolicy::ApprovalWaitSafety).unwrap(),
            Duration::ZERO
        );
        assert_eq!(
            effective_timeout(Some(17), TimeoutPolicy::ApprovalWaitSafety).unwrap(),
            Duration::from_secs(17)
        );
        assert!(matches!(
            effective_timeout(Some(u64::MAX), TimeoutPolicy::Ordinary),
            Err(OneharnessError::TimeoutOutOfRange { .. })
        ));
    }

    #[test]
    fn parse_env_rejects_malformed() {
        assert!(parse_env(&["KEY=val".into()]).is_ok());
        assert!(parse_env(&["noeq".into()]).is_err());
        assert!(parse_env(&["=val".into()]).is_err());
    }

    #[test]
    fn parse_env_allows_empty_value_and_equals_in_value() {
        let parsed = parse_env(&["A=".into(), "B=x=y".into()]).unwrap();
        assert_eq!(
            parsed,
            vec![("A".into(), "".into()), ("B".into(), "x=y".into())]
        );
    }

    /// The composed selection `setup_session` reads, in the owned form the run
    /// path hands it — selection has already validated every id, so each one
    /// parses into an identity.
    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|id| (*id).to_string()).collect()
    }

    /// A `RunArgs` carrying a `--session <name>` request pointed at an isolated,
    /// non-existent store directory. `resolve_dir` returns the path verbatim and
    /// the (absent) record reads back as `None`, so `setup_session` resolves a
    /// fresh *create* without touching a real store — enough to assert which
    /// harness the session anchors to.
    fn session_args(name: &str) -> RunRequest {
        let mut a = run_args();
        a.session = Some(name.to_string());
        a.session_dir = Some(std::env::temp_dir().join("oh-unit-no-such-session-store"));
        a
    }

    #[test]
    fn setup_session_in_fallback_anchors_to_first_session_capable() {
        // A fallback chain with more than one harness is ACCEPTED (unlike parallel)
        // and the session binds to the first *session-capable* harness in priority
        // order — here `codex`, skipping the non-capable `goose` ahead of it.
        let wiring = setup_session(
            &session_args("greet"),
            &ids(&["goose", "codex", "claude-code"]),
            false,
            true,
            std::path::Path::new("/proj"),
            false,
        )
        .expect("fallback + multi-harness --session is allowed")
        .expect("a wiring is returned when --session is set");
        assert_eq!(wiring.harness.as_str(), "codex");
        // A fresh store means a create plan (no token to resume).
        assert_eq!(wiring.plan.phase, session::SessionPhase::Create);
        assert!(wiring.plan.resume_token.is_none());
    }

    #[test]
    fn setup_session_parallel_still_rejects_multiple_harnesses() {
        // Parallel mode is single-harness by contract: more than one selected
        // harness makes a single session name ambiguous, exactly as before.
        assert!(matches!(
            setup_session(
                &session_args("x"),
                &ids(&["claude-code", "codex"]),
                false,
                false,
                std::path::Path::new("/proj"),
                false,
            ),
            Err(OneharnessError::SessionMultipleHarnesses { count: 2, .. })
        ));
    }

    #[test]
    fn setup_session_fallback_with_no_session_capable_harness_rejects() {
        // A fallback chain where NO harness exposes a session id headlessly cannot
        // carry a named handle — a loud SessionUnsupported, never a silent fresh
        // start on whichever harness happens to win.
        assert!(matches!(
            setup_session(
                &session_args("x"),
                &ids(&["goose", "crush"]),
                false,
                true,
                std::path::Path::new("/proj"),
                false,
            ),
            Err(OneharnessError::SessionUnsupported { .. })
        ));
    }

    #[test]
    fn server_overloaded_backoff_is_exponential_and_capped() {
        // Integration journeys prove that configured waits happen. Keep the
        // saturation proof structural: observing a full capped sleep end to
        // end would add a deliberate second to the ordinary unit-test tier for
        // no additional boundary coverage.
        assert_eq!(
            server_overloaded_backoff_duration(1),
            Duration::from_millis(100)
        );
        assert_eq!(
            server_overloaded_backoff_duration(2),
            Duration::from_millis(200)
        );
        assert_eq!(
            server_overloaded_backoff_duration(32),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn server_overloaded_readme_values_match_runtime_constants() {
        let readme = include_str!("../../../../README.md").replace("\r\n", "\n");
        let cli = include_str!("../../../../src/cli.rs");
        assert!(readme.contains(&format!(
            "before fallback (default {};",
            crate::domain::config::SERVER_OVERLOADED_MAX_RETRIES_DEFAULT
        )));
        assert!(readme.contains(&format!(
            "from {} ms, capped at\n  {} second per retry",
            SERVER_OVERLOADED_BACKOFF_INITIAL_MS,
            SERVER_OVERLOADED_BACKOFF_MAX_MS / 1_000
        )));
        assert!(readme.contains(&format!(
            "server_overloaded_max_retries = {} # --server-overloaded-max-retries (default {})",
            crate::domain::config::SERVER_OVERLOADED_MAX_RETRIES_DEFAULT,
            crate::domain::config::SERVER_OVERLOADED_MAX_RETRIES_DEFAULT
        )));
        assert!(readme.contains(&format!(
            "fall through — `{}`",
            fallback::FallThroughReason::ServerOverloaded.as_str()
        )));
        assert!(cli.contains(&format!(
            "/// {}). Each retry waits with bounded exponential backoff; 0 disables retry.",
            crate::domain::config::SERVER_OVERLOADED_MAX_RETRIES_DEFAULT
        )));
    }
}
