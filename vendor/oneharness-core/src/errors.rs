//! Typed errors surfaced only at the application boundary (see `dispatch`).
//!
//! These cover usage and configuration faults — the kind that should abort the
//! process. A harness's own behavior (missing binary, non-zero exit, hang) is
//! never an error here: it is recorded as data in the JSON report.

use thiserror::Error;

/// Where the `fallback` run mode a refusal names came from: left unset and
/// resolved to the default, or selected outright (`--run-mode fallback`,
/// `run_mode = "fallback"`, `ONEHARNESS_RUN_MODE=fallback`). Carried on
/// [`OneharnessError::FallbackConflict`] so the diagnostic names the mode as
/// the caller met it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunModeOrigin {
    Default,
    Selected,
}

impl std::fmt::Display for RunModeOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RunModeOrigin::Default => "the default run mode `fallback`",
            RunModeOrigin::Selected => "`--run-mode fallback`",
        })
    }
}

/// Where a run's streaming selection came from: the `--stream` flag, or the
/// `stream` value of a config layer (`stream = true` in the named file, or
/// `ONEHARNESS_STREAM`). Carried on [`JsonOnlySelection::Stream`] so the
/// refusal names the selection as the caller made it: a stream a config file
/// switched on is not `--stream`, and its way out is `--no-stream`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamOrigin {
    Flag,
    ConfigFile { path: std::path::PathBuf },
    Environment,
}

/// What an explicit `--format text` cannot sit beside: each only means
/// anything on a JSON stdout. Closed on purpose — a
/// [`OneharnessError::FormatConflict`] names one of these, never an arbitrary
/// string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonOnlySelection {
    /// The `--compact` flag.
    Compact,
    /// A run that will stream, selected where its [`StreamOrigin`] says.
    ///
    /// No longer raised by `oneharness run`, which streams readable text under
    /// `--format text`; kept so code matching on this enum still compiles.
    Stream(StreamOrigin),
}

impl JsonOnlySelection {
    /// The way out, said in the refusal beside the selection's name.
    fn why(&self) -> &'static str {
        match self {
            JsonOnlySelection::Compact => {
                "--compact is a JSON rendering choice (drop it, or pass --format json)"
            }
            JsonOnlySelection::Stream(StreamOrigin::Flag) => {
                "a streaming run's stdout is its NDJSON event/result protocol (drop --format text, or pass --format json)"
            }
            JsonOnlySelection::Stream(_) => {
                "a streaming run's stdout is its NDJSON event/result protocol (pass --no-stream, drop --format text, or pass --format json)"
            }
        }
    }
}

impl std::fmt::Display for JsonOnlySelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonOnlySelection::Compact => f.write_str("--compact"),
            JsonOnlySelection::Stream(StreamOrigin::Flag) => f.write_str("--stream"),
            JsonOnlySelection::Stream(StreamOrigin::ConfigFile { path }) => {
                write!(
                    f,
                    "`stream = true` in {} (the --stream setting)",
                    path.display()
                )
            }
            JsonOnlySelection::Stream(StreamOrigin::Environment) => {
                f.write_str("ONEHARNESS_STREAM (the --stream setting)")
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum OneharnessError {
    #[error("no harness selected: pass --all or --harness <id>, or set `all`/`harnesses` in oneharness.toml (see `oneharness list`)")]
    NoSelection,

    #[error("unknown harness id `{id}`. valid ids: {valid}")]
    UnknownHarness { id: String, valid: String },

    #[error(
        "unknown harness variant `{id}`; declare it under `[harness.{base}.variant.{variant}]`"
    )]
    UnknownHarnessVariant {
        // llmlint: ignore[invalid_states_unrepresentable] Error payloads preserve the exact untrusted selector text that failed validation; a validated selector cannot represent malformed input.
        id: String,
        // llmlint: ignore[invalid_states_unrepresentable] This is the raw base segment from the rejected selector, retained so the diagnostic can show the declaration path the user attempted.
        base: String,
        // llmlint: ignore[invalid_states_unrepresentable] VariantName cannot represent the malformed or unknown raw segment that this diagnostic must quote.
        variant: String,
    },

    #[error("could not read variant environment file `{path}`: {source}")]
    VariantEnvFile {
        // llmlint: ignore[invalid_states_unrepresentable] The I/O boundary stores the path's lossy display form deliberately because this error is a terminal user-facing diagnostic, not a path used for later I/O.
        path: String,
        source: std::io::Error,
    },

    #[error("variant environment file `{path}` must be a regular file readable only by its owner (mode 0600 or stricter)")]
    VariantEnvFilePermissions {
        // llmlint: ignore[invalid_states_unrepresentable] The checked PathBuf has already served its I/O purpose; the payload retains only its safe display form for the terminal diagnostic.
        path: String,
    },

    #[error("invalid line {line} in variant environment file `{path}`; expected KEY=VALUE")]
    VariantEnvFileLine {
        // llmlint: ignore[invalid_states_unrepresentable] This display string identifies the already-opened file to the user and is never converted back into a filesystem path.
        path: String,
        line: usize,
    },

    #[error("variant environment indirection `{name}` is not set in the parent process")]
    VariantEnvSourceMissing {
        // llmlint: ignore[invalid_states_unrepresentable] The raw configured environment-variable name is the failed lookup key and must be quoted exactly in the diagnostic.
        name: String,
    },

    #[error("harness selections `{first}` and `{second}` resolve to conflicting sync settings for the shared `{base}` config file")]
    VariantSyncConflict {
        // llmlint: ignore[invalid_states_unrepresentable] Selection has validated this base against the registry before conflict comparison; the error keeps its printable spelling.
        base: String,
        // llmlint: ignore[invalid_states_unrepresentable] Selection has validated this base or composed selector before conflict comparison; the diagnostic payload keeps its printable spelling.
        first: String,
        // llmlint: ignore[invalid_states_unrepresentable] Selection has validated this base or composed selector before conflict comparison; the diagnostic payload keeps its printable spelling.
        second: String,
    },

    #[error("--mock-harness `{id}` must name a selected harness")]
    MockHarnessNotSelected { id: String },

    #[error("could not locate the running oneharness executable for --mock-harness: {0}")]
    MockHarnessExecutable(std::io::Error),

    #[error("timeout {seconds}s is too large to represent as a deadline on this platform")]
    TimeoutOutOfRange { seconds: u64 },

    #[error("no prompt provided: pass --prompt <text> or --prompt-file <path>")]
    NoPrompt,

    #[error("--prompt-file - (stdin) can be given only once, but it was passed {count} times")]
    MultipleStdinPrompts { count: usize },

    #[error("stdin ('-') can be read only once, but {count} inputs request it (across --prompt-file and --system-file)")]
    MultipleStdinConsumers { count: usize },

    #[error("a batch run (more than one prompt) needs exactly one harness (a shared cache prefix is per harness/model), but {count} were selected: {selected}. Select one with --harness <id>")]
    BatchMultipleHarnesses { count: usize, selected: String },

    #[error("a batch run (more than one prompt) cannot be combined with --resume/--fork (those continue a single session with a single prompt)")]
    BatchResume,

    /// A fallback chain of two or more candidates carrying a shape a chain
    /// cannot: a batch, or a `--resume`/`--fork` continuation. `origin` says
    /// whether the mode was never chosen — fallback is what an unset mode
    /// resolves to — so the diagnostic names the way out for a caller who
    /// never asked for a chain.
    #[error("{origin} is incompatible with {with} over more than one harness: {why}")]
    FallbackConflict {
        with: &'static str,
        why: &'static str,
        origin: RunModeOrigin,
    },

    #[error("a multi-model run (more than one --model / config `models`) is incompatible with {with} ({why})")]
    MultiModelConflict {
        with: &'static str,
        why: &'static str,
    },

    #[error("--resume needs exactly one harness (a session belongs to one harness), but {count} were selected: {selected}")]
    ResumeMultipleHarnesses { count: usize, selected: String },

    #[error("harness `{id}` does not support --resume. supported: {supported}")]
    ResumeUnsupported { id: String, supported: String },

    #[error("harness `{id}` does not support --fork (it resumes linearly, appending in place). supported: {supported}")]
    ForkUnsupported { id: String, supported: String },

    #[error("--session needs exactly one harness in the parallel run mode (a named session belongs to one harness), but {count} were selected: {selected}. Use --run-mode fallback to bind a named session to a priority chain (it anchors to the first session-capable harness).")]
    SessionMultipleHarnesses { count: usize, selected: String },

    #[error("--session cannot be combined with a batch run (more than one prompt): a named session is one continued conversation, not a fan-out")]
    SessionBatch,

    #[error("harness `{id}` does not support --session: it exposes no session id headlessly, so a named handle cannot be mapped to it. supported: {supported}")]
    SessionUnsupported { id: String, supported: String },

    /// A named handle whose stored conversation cannot be reopened over the
    /// mechanism that would serve this turn.
    ///
    /// A driven turn negotiates its prompt, model, cwd and approvals on the
    /// wire and builds no argv, so the harness's verified `--resume` mapping is
    /// never reached: the only way to continue one conversation is the
    /// protocol's own resume request, and a mechanism without one would open a
    /// new conversation while the store, the report and the flag all looked
    /// healthy. Loud here, before anything spawns, because that is the one
    /// outcome nothing downstream can report.
    #[error("session `{name}` cannot be continued on harness `{id}` under --control: its control mechanism `{mechanism}` drives the turn over its own protocol and implements no resume request, so the turn would open a NEW conversation rather than continue this one. Drop --control to continue this handle on the harness's ordinary headless run, or keep --control and pass a fresh --session name to open a new conversation deliberately (the flag itself cannot be dropped: --control is addressed by the handle). Mechanisms that continue a session under --control: {supported}")]
    SessionControlNoResume {
        name: String,
        // llmlint: ignore[invalid_states_unrepresentable] The id is echoed from the already-validated registry selection purely for the diagnostic.
        id: String,
        mechanism: &'static str,
        supported: String,
    },

    #[error("harness `{id}` cannot capture --session under explicitly selected output format `{format}`: its session id is emitted only in {supported}. Remove --output-format/config `output_format` to let oneharness select a session-bearing format")]
    SessionOutputFormat {
        id: String,
        format: String,
        supported: String,
    },

    #[error("session `{name}` was created on harness `{was}`, so it cannot be continued on `{now}` (a named session is bound to one harness; use a different --session name)")]
    SessionHarnessConflict {
        name: String,
        was: String,
        now: String,
    },

    #[error("cannot resolve a session store directory (no --session-dir and no platform state dir): set --session-dir <DIR>")]
    SessionNoStore,

    #[error("harness `{id}` does not support `--mode {mode}`. supported modes: {supported}")]
    ModeUnsupported {
        id: String,
        mode: String,
        supported: String,
    },

    #[error("harness `{id}` cannot take a reasoning/effort setting headlessly: it exposes no reasoning flag (effort is provider/model config there). harnesses that can: {supported}")]
    ReasoningUnsupported { id: String, supported: String },

    #[error("harness `{id}` delivers reasoning effort through the model id (`--model 'MODEL[effort=…]'`), so `--reasoning` needs a model — set --model or `[harness.{id}] model`")]
    ReasoningNeedsModel { id: String },

    #[error("could not read harness config `{path}`: {source}")]
    HarnessConfigRead {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot sync into `{path}`: {message} (oneharness only rewrites files it can parse, so it never destroys content it does not understand)")]
    HarnessConfigUnmergeable { path: String, message: String },

    #[error("harness `{id}` has no hook mapping, so oneharness cannot install a hook into it")]
    HookUnsupported { id: String },

    #[error("harness `{id}` has no user-global hook location, so oneharness cannot install a global hook into it")]
    HookGlobalUnsupported { id: String },

    #[error("cannot resolve the user-global hook directory for `{id}`: {var} is not set")]
    HookGlobalDirMissing { id: String, var: &'static str },

    #[error("`oneharness sync --global` installs hooks only, but permission rules or `settings` are configured for `{id}` — those are project-scoped (sync them without --global, or remove them)")]
    GlobalSyncOnlyHooks { id: String },

    #[error(
        "harness `{id}` has no pre-tool gate, so `oneharness gate {id}` cannot emit a verdict"
    )]
    GateUnsupported { id: String },

    #[error("could not read mock rules file `{path}`: {source}")]
    MockRulesFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid mock rules file `{path}`: {message}")]
    MockRulesInvalid { path: String, message: String },

    #[error("harness `{id}` cannot express the mock action `{action}`, so `oneharness mock {id}` refuses this ruleset (see `mock_rewrite`/`supports_mock_deny` in `oneharness list`)")]
    MockActionUnsupported { id: String, action: &'static str },

    #[error(
        "harness `{id}` cannot take a one-shot mock hook (`--mock-rules`/`--spy-file`): {reason}"
    )]
    MockDeliveryUnsupported { id: String, reason: &'static str },

    #[error("cannot embed `{path}` into a hook command: it contains whitespace (hook commands are tokenized on spaces by some harnesses; use space-free paths)")]
    MockPathWhitespace { path: String },

    #[error("could not wire the one-shot mock hook: {message}")]
    MockSetup { message: String },

    #[error("could not write harness config `{path}`: {source}")]
    HarnessConfigWrite {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{path} already exists; pass --force to overwrite it (nothing was written)")]
    InitFileExists { path: String },

    #[error("could not write starter config `{path}`: {source}")]
    InitWrite {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not read prompt file `{path}`: {source}")]
    PromptFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not read system prompt file `{path}`: {source}")]
    SystemFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not read schema file `{path}`: {source}")]
    SchemaFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid --schema: {0}")]
    Schema(String),

    #[error("invalid --stream: {0}")]
    StreamInvalid(String),

    /// An explicit `--format text` beside a selection that only means anything
    /// on a JSON stdout. The CLI's stdout defaults to the text view, so either
    /// on its own is fine and a caller who asks for both is asking for two
    /// things one stdout cannot be; the refusal names both, where the other
    /// came from, so the caller can drop the one they did not mean.
    #[error("--format text cannot be combined with {selection}: {}", selection.why())]
    FormatConflict { selection: JsonOnlySelection },

    #[error("could not read config file `{path}`: {source}")]
    ConfigRead {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid config file `{path}`: {message}")]
    ConfigInvalid { path: String, message: String },

    #[error("invalid environment-variable config override: {0}")]
    EnvConfigInvalid(String),

    #[error("invalid --bin override `{0}`: expected the form ID=PATH")]
    BadBinOverride(String),

    #[error("invalid --env `{0}`: expected the form KEY=VALUE")]
    BadEnv(String),

    #[error("could not write output to `{path}`: {source}")]
    OutputDir {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not access history under `{path}`: {source}")]
    HistoryIo {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("no history directory: pass --history-dir, set `history_dir` in config, or ONEHARNESS_HISTORY_DIR (a default under the platform state dir could not be resolved)")]
    HistoryNoDir,

    #[error("harness `{id}` needs output format `{required}` for history telemetry, but `{selected}` was selected; remove the explicit format or incompatible native schema")]
    HistoryTelemetryFormat {
        id: String,
        required: String,
        selected: String,
    },

    #[error("history record `{id}` was not found in the index segment for its date; a run the dated index does not hold (recorded by an older oneharness, or copied in from another store) is found with `--all-time`, or indexed for good by `oneharness history reindex`")]
    HistoryNotFound { id: String },

    #[error("invalid history label: {0}")]
    HistoryLabelInvalid(String),

    #[error("invalid history cursor `{value}`: expected a UUID")]
    HistoryCursorInvalid { value: String },

    #[error("--control requires --session <NAME>: a run with no caller-owned handle has no address an `oneharness interrupt` process could resolve")]
    ControlNeedsSession,

    #[error("harness `{id}` has no out-of-band turn control, so --control cannot be honored (control-capable: {supported})")]
    ControlUnsupported {
        // llmlint: ignore[invalid_states_unrepresentable] The id is echoed from the already-validated registry selection purely for the diagnostic.
        id: String,
        supported: String,
    },

    #[error(
        "--control needs exactly one harness in the parallel run mode (control drives one live turn), but these were selected: {selected}. Use --run-mode fallback to bind control to a priority chain"
    )]
    ControlSingleHarness { selected: String },

    #[error("--control drives one live turn, so it cannot be combined with a batch run (more than one prompt)")]
    ControlBatch,

    #[error("--control needs a unix domain socket, which this platform does not provide")]
    ControlPlatform,

    #[error("harness `{id}` submits its controlled turn to a server rather than running its CLI, so there is no line-by-line output to stream; drop --stream (the report still carries the turn's transcript)")]
    ControlStreamUnsupported { id: String },

    #[error("--control cannot be combined with --schema: the structured-output retry loop re-prompts, which would need a second turn on the control channel's open stdin")]
    ControlSchema,

    #[error("harness `{id}` carries `--mode {mode}` in its own configuration environment, and a controlled turn does not hand that environment to the server it is submitted to — so the turn would run under the server's own policy rather than the one asked for. Use --mode default (deny and continue) or --mode bypass, or drop --control")]
    ControlModeUnsupported {
        // llmlint: ignore[invalid_states_unrepresentable] The id is echoed from the already-validated registry selection purely for the diagnostic.
        id: String,
        mode: &'static str,
    },

    #[error("harness `{id}` needs output format `{required}` for --control, but `{selected}` was selected; drop the explicit --output-format")]
    ControlOutputFormat {
        id: String,
        required: String,
        selected: String,
    },

    /// The address for this run's control socket cannot be bound on this
    /// platform. A usage/config error rather than a degraded capability: the
    /// channel would not exist, and `--control` promises a supervisor it does.
    #[error(
        "{source}, and the session name is already as short as it can be made. Point the session store somewhere shorter (--session-dir, or `session_dir` in config): the socket lives in its `control/` directory"
    )]
    ControlSocketAddress {
        #[source]
        source: crate::domain::control::SocketAddressTooLong,
    },

    #[error("could not open the control socket `{path}`: {source}")]
    ControlSocket {
        // llmlint: ignore[invalid_states_unrepresentable] The failed socket path is retained in display form for a terminal diagnostic and never reused for I/O.
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("no session store directory: pass --session-dir or set `session_dir` in config (a default under the platform state dir could not be resolved)")]
    ControlNoSessionDir,

    /// `--input` was not a message a redirection can carry. A usage error rather
    /// than a refusal frame: nothing was asked of the run, so there is nothing
    /// for it to answer — and a supervisor that mis-spelled its redirection
    /// needs to be told before the turn it meant to redirect ends.
    #[error("--input is not a usable redirection: {reason}")]
    ControlInputInvalid { reason: String },

    #[error("--session-dir `{path}` is not valid UTF-8, so it cannot address a session store")]
    SessionDirInvalid {
        // llmlint: ignore[invalid_states_unrepresentable] The rejected path is kept in its lossy display form solely to quote it back; it is never reused for I/O.
        path: String,
    },

    #[error("failed to write JSON output: {0}")]
    Serialize(#[from] serde_json::Error),

    /// A write to stdout failed — most often a closed downstream reader
    /// (`oneharness usage | head -1`). Reported like any other I/O fault so a
    /// command that emits its report cannot die mid-sentence with a panic; the
    /// report is the deliverable, and a truncated one is worth saying out loud.
    #[error("could not write to stdout: {0}")]
    StdoutWrite(std::io::Error),
}
