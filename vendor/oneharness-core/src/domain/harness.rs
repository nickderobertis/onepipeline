//! The harness registry: one declarative adapter per supported CLI.
//!
//! An adapter is data — a canonical id, a default binary, an install hint, an
//! output format — plus one pure function that builds the argv. Adding a harness
//! is adding an entry here; `run`, the runner, and the report shape are untouched.
//!
//! The flags encoded below mirror the known-good non-interactive invocations used
//! to drive each real CLI headlessly (deny prompts, pick the model, request a
//! parseable format). Source new flags from a working driver, not by guessing.

use crate::domain::control::{ControlShape, ServerSpec, ServerTransport};
use crate::domain::events::TelemetryTrace;
use crate::domain::gate::DenyShape;
use crate::domain::hooks::HookShape;
use crate::domain::mock::{MockDelivery, RewriteShape};
use crate::domain::mode::{ApprovalPosture, ModeHeadless, PermissionMode};
use crate::domain::report::OutputFormat;
use crate::domain::structured::NativeSchema;
use crate::domain::usage::{UsageProbe, UsageSupport};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Everything `build_argv` needs, with no I/O: the resolved binary, the prompt,
/// the optional model, whether to request the harness's "don't prompt" mode, and
/// the effective output format (the harness default, or a `--output-format`
/// override) for harnesses that take a format flag.
pub struct BuildCtx<'a> {
    pub bin: &'a str,
    pub prompt: &'a str,
    pub model: Option<&'a str>,
    /// System prompt to apply. Adapters with a native system flag map it (Claude
    /// Code's `--append-system-prompt`, Goose's `--system`); adapters without one
    /// prepend it to the prompt via `prompt_with_system` so the instructions
    /// still reach the model, rather than dropping it.
    pub system: Option<&'a str>,
    /// Session id to continue, for harnesses that support resumption. Only set
    /// after the command layer has verified the selected harness's
    /// `supports_resume`, so an adapter that maps it can assume support.
    pub resume: Option<&'a str>,
    /// When resuming, branch a *new* session from the resumed one instead of
    /// appending to it — so the original (and its cached prefix) is untouched and
    /// can seed independent follow-ups. Only honored alongside `resume`, and only
    /// set after the command layer has verified `supports_fork`; an adapter maps it
    /// to its native fork flag (Claude Code's `--fork-session`, OpenCode's
    /// `--fork`). Ignored by adapters that cannot fork (they are never selected
    /// with it).
    pub fork: bool,
    /// The normalized approval mode to request. Each adapter maps it to its
    /// harness's native mechanism (argv flags here; any environment via the
    /// matching [`ModeSpec`]). The command layer guarantees the selected harness
    /// actually supports `mode` before calling `build_argv`, so an adapter only
    /// needs correct output for the modes in its [`HarnessSpec::modes`].
    pub mode: PermissionMode,
    pub output_format: OutputFormat,
    /// Inline JSON-Schema text to deliver through the harness's *native*
    /// structured-output flag, set only for an adapter with a
    /// [`HarnessSpec::native_schema`] when a schema run is requested. Adapters
    /// without native support ignore it — the command layer instead appends the
    /// schema instruction to the prompt — so it is never silently dropped.
    pub schema: Option<&'a str>,
    /// Path to a temp file holding the system prompt, set by the command layer
    /// when the system prompt is large enough to risk the argv ceiling
    /// (`E2BIG`) AND the harness declares [`LargeInput::system_file_flag`].
    /// When `Some`, an adapter delivers the system prompt through its file flag
    /// (Claude Code's `--append-system-prompt-file`) instead of inline
    /// `--append-system-prompt`; `system` is left unread. `None` keeps the
    /// ordinary inline path.
    pub system_file: Option<&'a str>,
    /// How the user prompt reaches the harness for this run. One value rather
    /// than a flag per route, because the routes are alternatives: a prompt
    /// cannot ride both an argv positional and a message stream, and the two
    /// stdin shapes ([`PromptDelivery::Stdin`] vs
    /// [`PromptDelivery::ControlStream`]) select different CLI flags.
    pub delivery: PromptDelivery,
    /// Whether the run opts into the harness's plugins feature (the resolved
    /// `plugins` config key, [`crate::domain::config::FileConfig::plugins_for`]).
    /// Only codex reads it: `false` starts Codex with `-c features.plugins=false`
    /// right after its subcommand, so the caller's own args — which ride later —
    /// can still turn plugins back on; `true` adds no plugins flag at all.
    pub plugins: bool,
    /// Whether the run opts into the harness's ChatGPT apps (connectors)
    /// feature (the resolved `apps` config key,
    /// [`crate::domain::config::FileConfig::apps_for`]). Only codex reads it:
    /// `false` adds `-c features.apps=false` right after the plugins setting,
    /// ahead of the caller's args so a caller's own enable still wins; `true`
    /// adds no apps flag at all.
    pub apps: bool,
    /// Whether the run lets the harness send analytics (the resolved
    /// `analytics` config key, [`crate::domain::config::FileConfig::analytics_for`]).
    /// Only codex reads it: `false` adds `-c analytics.enabled=false` beside the
    /// plugins flag, ahead of the caller's args; `true` adds no analytics flag.
    pub analytics: bool,
    /// The shared model catalog file this launch reads instead of fetching
    /// Codex's own (`crate::io::model_catalog`). Only codex reads it: `Some`
    /// adds `-c model_catalog_json=<path>` beside the plugins flag, ahead of the
    /// caller's args; `None` adds nothing and Codex fetches its catalog itself.
    pub model_catalog: Option<&'a Path>,
}

/// How the user prompt reaches the harness.
///
/// The command layer decides; `build_argv` reads. Keeping it one value is what
/// makes "positional prompt *and* `--input-format stream-json`" — a spawn the
/// CLI would reject — impossible to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDelivery {
    /// An argv positional: the ordinary case, and byte-identical to what every
    /// `--print-command` assertion pins.
    Argv,
    /// Piped to the child's **stdin** as one blob, for a prompt large enough to
    /// risk the argv ceiling (`E2BIG`) on a harness that declares
    /// [`LargeInput::prompt_stdin`]. The adapter omits the positional and adds
    /// its stdin-selecting flags (Claude Code's `--input-format text`, Goose's
    /// `-i -`).
    Stdin,
    /// Written to the child's stdin as the first frame of a **message stream**
    /// whose handle then stays open for the run's lifetime — the delivery a
    /// control-enabled run (`--control`) needs, since that same handle is how an
    /// out-of-band interrupt reaches the live turn. Only ever selected for a
    /// harness whose [`HarnessSpec::control`] rides its own stdin.
    ControlStream,
}

impl PromptDelivery {
    /// Whether the adapter should omit the argv positional.
    #[must_use]
    pub fn off_argv(self) -> bool {
        !matches!(self, PromptDelivery::Argv)
    }

    /// Whether the prompt is piped as one blob (the large-prompt route).
    #[must_use]
    pub fn is_stdin_blob(self) -> bool {
        matches!(self, PromptDelivery::Stdin)
    }

    /// Whether the prompt opens a control-enabled message stream.
    #[must_use]
    pub fn is_control_stream(self) -> bool {
        matches!(self, PromptDelivery::ControlStream)
    }
}

/// The CLI token for a format, as the harnesses spell it.
fn format_flag(format: OutputFormat) -> &'static str {
    format.as_str()
}

/// The prompt an adapter should send, with the system instructions prepended when
/// the harness has no native system flag. This is how `--system` reaches models
/// on harnesses like Codex/OpenCode that expose no system-prompt option — without
/// it the instructions would be silently dropped. A blank system prompt is a
/// no-op. Adapters with a native flag (claude-code, goose) pass `c.prompt`
/// directly and map `c.system` separately instead of calling this.
fn prompt_with_system(c: &BuildCtx) -> String {
    prompt_with_system_text(c.system, c.prompt)
}

/// The text form of `prompt_with_system`, for the command layer to assemble the
/// **stdin** payload for a large-prompt run (where `build_argv` omits the
/// positional): the same system-prepended string the adapter would otherwise have
/// inlined, so stdin delivery is byte-for-byte what the model would have seen on
/// the argv. Consulted only for a harness whose system rides the prompt
/// ([`LargeInput::system_rides_prompt`]).
pub fn prompt_with_system_text(system: Option<&str>, prompt: &str) -> String {
    match system {
        Some(s) if !s.is_empty() => format!("{s}\n\n{prompt}"),
        _ => prompt.to_string(),
    }
}

/// How a harness can accept a **large** prompt without inlining it into the argv
/// (which trips `E2BIG` past the OS ceiling). Pure capability data on
/// [`HarnessSpec::large_input`]; the delivery *mechanism* (which flags to add,
/// whether to omit the positional) lives in the per-harness `build_argv`. Sourced
/// from each CLI's headless docs, never guessed.
pub struct LargeInput {
    /// The user prompt can be delivered on the child's **stdin** instead of an
    /// argv positional. When the command layer selects [`PromptDelivery::Stdin`],
    /// `build_argv` omits the positional (and adds any stdin-selecting flags —
    /// Claude Code's `--input-format text`, Goose's `-i -`) and the command layer
    /// pipes the assembled prompt to stdin.
    pub prompt_stdin: bool,
    /// When the prompt rides stdin, whether the **system** prompt must be folded
    /// into that stdin payload. `true` for a harness with no native system flag
    /// (its system normally rides the prompt via `prompt_with_system` — Codex,
    /// OpenCode, Qwen, Crush, Copilot, Cursor); `false` for one that carries the
    /// system separately (Claude Code's file flag, Goose's inline `--system`),
    /// whose system is not part of the user-prompt stream. Only consulted when
    /// `prompt_stdin` is set.
    pub system_rides_prompt: bool,
    /// A CLI flag that reads the **system** prompt from a file, for a harness
    /// whose system prompt is a *separate* argv argument that would itself hit the
    /// ceiling (Claude Code's `--append-system-prompt-file`). When the command
    /// layer sets [`BuildCtx::system_file`], `build_argv` emits this flag with the
    /// temp-file path instead of the inline system flag. `None` when the harness
    /// has no per-run system file mechanism (its large system rides the prompt via
    /// `system_rides_prompt`, or — Goose — has no file route at all).
    pub system_file_flag: Option<&'static str>,
}

impl LargeInput {
    /// Inline only: no off-argv path, so a large prompt/system stays subject to
    /// the argv ceiling. The safe default for a CLI without file/stdin input.
    pub const NONE: LargeInput = LargeInput {
        prompt_stdin: false,
        system_rides_prompt: false,
        system_file_flag: None,
    };

    /// Whether the harness offers any off-argv delivery at all.
    pub fn any(&self) -> bool {
        self.prompt_stdin || self.system_file_flag.is_some()
    }
}

/// A single harness adapter.
pub struct HarnessSpec {
    /// Canonical id used on the CLI and in JSON (e.g. `claude-code`).
    pub id: &'static str,
    /// Human-friendly name for `list`.
    pub display: &'static str,
    /// Binary name looked up on PATH unless overridden.
    pub default_bin: &'static str,
    /// How a user installs the CLI (shown when it is missing).
    pub install_hint: &'static str,
    /// The format the adapter requests, which drives text extraction.
    pub output_format: OutputFormat,
    /// The format to switch this harness to when the caller asks for tool
    /// **events** (`--events` / `--stream`) and its *default* format carries no
    /// machine-readable tool transcript — so `events` can be surfaced without the
    /// caller knowing each CLI's quirk. `None` means either the default already
    /// carries a transcript (OpenCode's `json`, Cursor's `stream-json` — no
    /// upgrade needed) or the harness exposes no events-capable format at all
    /// (the plain-text harnesses); in both cases `--events` leaves the format
    /// unchanged. When `Some`, the command layer selects it under `--events`
    /// unless the caller set an explicit `--output-format`. Claude Code needs
    /// `stream-json` (its default single-document `json` result omits the
    /// transcript). Sourced from each CLI's real output, never guessed; text/usage
    /// extraction must still work under the upgraded format (verified live).
    pub events_format: Option<OutputFormat>,
    /// Machine-readable format and grammar that exposes real provider request
    /// and tool interval boundaries. `None` is an explicit unsupported
    /// capability, not an alias for the default output format.
    pub telemetry: Option<TelemetrySpec>,
    /// Whether this harness can continue a prior session (`run --resume`). When
    /// false, the command layer rejects `--resume` for it rather than silently
    /// starting a fresh session. Kept as data so the capability is introspectable
    /// via `oneharness list`.
    pub supports_resume: bool,
    /// Output formats in which this harness exposes a native session id
    /// headlessly (the [`crate::domain::signals::extract_session`] sources).
    /// The first is the preferred format oneharness selects automatically for
    /// `run --session <name>` when the caller did not explicitly pin a format;
    /// an empty slice means `--session` is unsupported. Keeping the capability
    /// and its transport in one field prevents a harness from claiming session
    /// support while its selected format cannot actually emit the id. Every
    /// non-empty entry implies `supports_resume`. Exposed as the boolean
    /// `session_capable` by `oneharness list`.
    pub session_formats: &'static [OutputFormat],
    /// Whether this harness can *fork* a session when resuming (`run --resume
    /// <id> --fork`): branch a new session id from the resumed one, leaving the
    /// original untouched so its cached prefix seeds independent follow-ups. Only
    /// two CLIs expose a headless fork flag (Claude Code's `--fork-session`,
    /// OpenCode's `--fork`); the rest resume linearly (append in place). When
    /// false, `--fork` is a loud usage error for the harness, never a silent
    /// linear resume. Implies `supports_resume`. Introspectable via `oneharness
    /// list`.
    pub supports_fork: bool,
    /// Whether a forked run *reuses* the parent session's provider prompt-cache
    /// prefix — so a fork-based `min-tokens` batch (warm one prompt, fork the
    /// rest) actually *reduces* tokens. Implies `supports_fork`. This is the gate
    /// for the fork-based batch path: when false, `min-tokens` only orders the
    /// calls (no token saving) rather than forking. Measured by the live e2e
    /// (`oh_batch_fork_enforce`), never guessed: **true for Claude Code** (Anthropic
    /// prompt caching, and `--fork-session` preserves the cached session prefix);
    /// **false for OpenCode**, whose `--fork` re-sends the branched conversation
    /// cold (the fan-out reads nothing and re-writes the whole prefix — measured,
    /// so forking it would *raise* tokens, not lower them). Introspectable via
    /// `oneharness list`.
    pub fork_reuses_cache: bool,
    /// Where this harness reads project-scoped configuration, and how the
    /// unified enforcement settings (`allowed_tools` / `denied_tools` /
    /// `hooks` / `settings`) map into that file. Codex's target is not a
    /// config file but its own execpolicy rules file
    /// ([`SyncFormat::ExecPolicyRules`]): it takes the allow/deny lists,
    /// translated per rule, and nothing else. `None` means the harness has no
    /// project-level file oneharness knows how to write (Goose reads only
    /// user-global config; Copilot takes permission rules as flags,
    /// deliverable via `[harness.copilot] args`) — configuring a sync setting
    /// for it is then a loud usage error, never a silent no-op.
    /// Consumed by `oneharness sync`; nothing here is passed on the argv.
    pub sync: Option<SyncSpec>,
    /// How a normalized pre-tool [`crate::domain::hooks::HookSpec`] is installed
    /// into this harness — the shape its config expects and where the file lands
    /// (a shared config file, a dedicated hooks file, or a plugin). `None` for a
    /// harness oneharness cannot wire a hook into. Consumed by `oneharness sync`
    /// / `src/io/hooks.rs`; nothing here is passed on the argv.
    pub hooks: Option<HookBinding>,
    /// Where this harness reads a *user-global* hook, for an `io::hooks::install`
    /// at [`crate::io::hooks::Scope::Global`] (the project path lives in
    /// [`HookBinding`]). The install strategy is identical to the project one;
    /// only the anchor moves to a `$HOME`/`$XDG_CONFIG_HOME` location. `None` for
    /// a harness with no user-global hook location oneharness knows. Sourced from
    /// the allowlister adapters, never guessed.
    pub global_hook: Option<GlobalHook>,
    /// How this harness expresses a pre-tool *deny* when its installed hook runs
    /// `oneharness gate <id>` — the runtime counterpart to [`HookBinding`]. `None`
    /// for a harness with no gateable pre-tool hook. Consumed by the `gate`
    /// command (`src/commands/gate.rs`); sourced from the allowlister adapters.
    pub gate_deny: Option<DenyShape>,
    /// How this harness expresses a pre-tool *input rewrite* when its installed
    /// hook runs `oneharness mock <id>` — allow the call with substituted
    /// arguments, the mock workhorse (swap a shell command for a stub that
    /// prints canned output). `None` for a harness whose hook protocol has no
    /// rewrite verdict (Goose), or whose support is not yet verified through
    /// `oneharness run` (Codex, Copilot, Cursor — pending the `explore-hooks`
    /// probe; see `docs/mock-spy-design.md`): a rewrite rule for it is then a
    /// loud usage error, never a silent allow. Sourced from each CLI's hook
    /// docs; honored-live is drift-alarmed by the `oh_mock_enforce` e2e phases.
    pub mock_rewrite: Option<RewriteShape>,
    /// How `run --mock-rules`/`run --spy-file` delivers the mock hook for ONE
    /// invocation (see [`MockDelivery`]): Claude Code takes it on the argv via
    /// `--settings` (zero workspace mutation); the rest get a project-scope
    /// install that is snapshotted and restored around the run, layering on top
    /// of any existing config non-destructively. `None` for a harness whose
    /// hooks cannot fire from a project-scope headless run (qwen: user scope
    /// only; copilot: probe-refuted entirely) — the flag is then a loud usage
    /// error, never a silently inert install.
    pub mock_delivery: Option<MockDelivery>,
    /// Environment variables oneharness sets when spawning this harness, so a
    /// headless run is clean without the caller knowing the harness's quirks
    /// (e.g. silencing a startup warning that would otherwise litter `stderr`).
    /// Pure data: the registry declares them; the command/io layer injects them,
    /// and an explicit `--env` always wins over a default here. Empty for most.
    pub default_env: &'static [(&'static str, &'static str)],
    /// How this harness accepts a JSON Schema *natively*, when it does — the
    /// schema is delivered through its own CLI flag and the conforming value
    /// read from a known field, rather than appended to the prompt. `None` means
    /// structured-output runs fall back to the portable prompt-based path (which
    /// works for every harness). Either way oneharness validates the result
    /// itself, so a native flag the harness ignores is still caught.
    pub native_schema: Option<NativeSchema>,
    /// How this harness can take a **large** prompt without inlining it into the
    /// argv, where a single argument over the OS ceiling (`MAX_ARG_STRLEN`, 128
    /// KiB on Linux; the total argv+env cap on macOS/Windows) fails the spawn
    /// with `E2BIG`. Pure capability data: the command layer materializes the
    /// system prompt to a temp file / pipes the user prompt to stdin only when a
    /// prompt clears the size threshold *and* the flag here says the harness can
    /// receive it that way, then `build_argv` reads the matching [`BuildCtx`]
    /// fields ([`BuildCtx::system_file`] / [`BuildCtx::delivery`]). A small
    /// prompt keeps the byte-identical inline argv, so the common case (and every
    /// `--print-command`) is unchanged. Sourced from each CLI's headless docs,
    /// never guessed; [`LargeInput::NONE`] (inline only) is the safe default for a
    /// CLI with no file/stdin input. Introspectable via `oneharness list`.
    pub large_input: LargeInput,
    /// The approval modes this harness can express, each with how it behaves in
    /// a headless run. A [`PermissionMode`] absent from this list is unsupported
    /// for the harness — the command layer turns a request for it into a loud
    /// usage error rather than silently downgrading. Every harness lists
    /// [`PermissionMode::Bypass`] (the headless default) and
    /// [`PermissionMode::Default`]. Sourced from each CLI's docs/behavior, never
    /// guessed (see the README support matrix and `AGENTS.md`).
    pub modes: &'static [ModeSpec],
    /// How this harness accepts a **reasoning / thinking effort** setting on the
    /// argv in a headless run, when it can. The value is an opaque string chosen
    /// by the caller *for their model* (e.g. `high`, `xhigh`) and forwarded
    /// verbatim in the harness's native shape — oneharness does not interpret or
    /// validate it, so an effort level the model rejects surfaces as that
    /// harness's own error (a `nonzero` result), never a oneharness guess.
    /// Reasoning effort is fundamentally a provider/model capability with no
    /// shared spelling (OpenAI's `reasoning_effort` enum vs. Anthropic's
    /// thinking-token budget), so this is a per-harness delivery, not a
    /// normalized spectrum. `None` for a harness with no headless argv surface
    /// for it — the command layer then turns a reasoning request into a loud
    /// usage error rather than silently dropping it (Cursor and Copilot express
    /// effort only through their own config file, the `sync`-path follow-up; the
    /// plain harnesses have no knob at all). Sourced from each CLI's docs, never
    /// guessed. Introspectable via `oneharness list`.
    pub reasoning: Option<ReasoningDelivery>,
    /// How much subscription **headroom** this harness can report to
    /// `oneharness usage`, and by which zero-turn probe. Pure capability data:
    /// the command layer dispatches on it, and a harness that cannot report
    /// headroom says so affirmatively (with which kind of "cannot") rather than
    /// being omitted or rendered as 0% used. Sourced from `docs/harness-usage.md`
    /// — every probe and every negative there is an observation, never a guess.
    pub usage: UsageSupport,
    /// How this harness accepts an **out-of-band interrupt** for an in-flight
    /// turn (`oneharness run --control` + `oneharness interrupt`). `None` means
    /// the lever does not exist for it: `--control` is a loud usage error rather
    /// than a socket that reports success while the turn keeps running. Sourced
    /// from a *proven* live interrupt against the real CLI — a declared shape
    /// that was never exercised is the specific failure this field must not
    /// have (see the capability matrix in `README.md` and
    /// `scripts/explore-control.sh`, the drift alarm).
    // llmlint: ignore-block[invalid_states_unrepresentable] `control` and `server`
    // are two fields by approved design (a mechanism is declared independently of
    // the process that backs it, and Claude Code's needs none), so the
    // relationship is enforced where it can also catch a future harness: the
    // registry invariant test below fails if a server-backed mechanism declares no
    // server, or a server is declared for a harness with no control. Scoped to
    // both fields because the pairing IS the finding — an ignore on `control`
    // alone leaves the half of it that lives on `server` unsuppressed.
    pub control: Option<ControlShape>,
    /// The sidecar server this harness's control mechanism needs, when it needs
    /// one. Declared per harness rather than special-casing the one that does
    /// not (Claude Code, whose control rides the run process's own stdin), and
    /// consumed by the generic pool in [`crate::io::server_pool`]. `None` for a
    /// harness with no server, or with no proven control mechanism at all.
    pub server: Option<ServerSpec>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// Builds the full argv (argv\[0\] is the binary). Pure.
    pub build_argv: fn(&BuildCtx) -> Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelemetrySpec {
    pub format: OutputFormat,
    pub trace: TelemetryTrace,
}

/// How a harness takes a reasoning/effort string on the argv. Rendered by the
/// command layer into the harness's override args (alongside config `args` /
/// passthrough), so `build_argv` stays untouched. The single source of truth for
/// the flag shape; the `--print-command` assertions pin the rendered result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningDelivery {
    /// A dedicated flag whose value is the effort string: `<flag> <value>`
    /// (Claude Code's `--effort <level>`, Copilot's `--reasoning-effort <level>`).
    Flag(&'static str),
    /// A `-c key=value` config override: `-c <key>=<value>` (Codex's
    /// `-c model_reasoning_effort=<level>`; `-c` is Codex's documented headless
    /// override, already exercised live by the codex mock phase). The value is a
    /// bare word — Codex parses a `-c` value as JSON and falls back to a string,
    /// so `model_reasoning_effort=high` lands as the string `"high"`.
    ConfigKv(&'static str),
    /// Effort is baked into the **model id** as a `-<value>` tier suffix:
    /// `<model>-<value>` (Cursor — `claude-opus-4-8` + `high` →
    /// `claude-opus-4-8-high`). cursor-agent has **no** separate effort flag and
    /// REJECTS a bracketed `model[effort=…]` option — verified live: it answers
    /// "Cannot use this model" and lists ids that bake the tier into the name
    /// (`-low`/`-medium`/`-high`/`-xhigh`/`-max`). Unlike the append-style
    /// deliveries this decorates the existing `--model` value rather than adding
    /// args, so it *requires* a model to attach to (the command layer refuses
    /// `--reasoning` without a model). The base model must belong to a family that
    /// accepts the tier suffix (opus-4-8, gpt-5.x, sonnet-5, grok-4.5, …); an
    /// unsupported base surfaces as cursor's own `nonzero` (the opaque-value
    /// contract), and the live `oh_reasoning_enforce` phase is the drift alarm.
    ModelSuffix,
}

impl ReasoningDelivery {
    /// The argv fragment appended to the harness's override args, forwarded
    /// verbatim (no quoting or normalization — see [`HarnessSpec::reasoning`]).
    /// Empty for [`ReasoningDelivery::ModelSuffix`], which decorates the model id
    /// via [`Self::model_suffix`] instead of appending args.
    pub fn args(&self, value: &str) -> Vec<String> {
        match self {
            ReasoningDelivery::Flag(flag) => vec![(*flag).to_string(), value.to_string()],
            ReasoningDelivery::ConfigKv(key) => vec!["-c".to_string(), format!("{key}={value}")],
            ReasoningDelivery::ModelSuffix => Vec::new(),
        }
    }

    /// The tier suffix appended to the model id for a
    /// [`ReasoningDelivery::ModelSuffix`] harness (`-<value>`, e.g. `-high`), or
    /// `None` for the append-style deliveries (which leave the model untouched).
    /// The command layer appends it to the resolved `--model` value.
    pub fn model_suffix(&self, value: &str) -> Option<String> {
        match self {
            ReasoningDelivery::ModelSuffix => Some(format!("-{value}")),
            _ => None,
        }
    }
}

impl HarnessSpec {
    /// The [`ModeSpec`] for `mode`, or `None` when this harness cannot express
    /// it. The lookup the command layer uses to gate a run and to inject any
    /// per-mode environment.
    pub fn mode(&self, mode: PermissionMode) -> Option<&'static ModeSpec> {
        self.modes.iter().find(|m| m.mode == mode)
    }

    /// Whether this harness can back the caller-owned `--session` handle.
    pub fn session_capable(&self) -> bool {
        !self.session_formats.is_empty()
    }

    /// Whether it can back that handle *for this run*, given whether the run is
    /// control-enabled.
    ///
    /// A harness whose control mechanism drives the turn over its own protocol
    /// (Codex's app-server, ACP) mints a thread/session id on the wire even when
    /// none of its ordinary output formats carries one — which is how Copilot and
    /// Goose can take `--session` under `--control` and only there.
    pub fn session_capable_under(&self, control: bool) -> bool {
        self.session_capable() || (control && self.control.is_some())
    }

    /// The session-id-bearing format selected automatically for `--session`.
    pub fn session_format(&self) -> Option<OutputFormat> {
        self.session_formats.first().copied()
    }

    /// Whether `format` can carry this harness's native session id.
    pub fn format_carries_session(&self, format: OutputFormat) -> bool {
        self.session_formats.contains(&format)
    }
}

/// One approval mode a harness supports: its headless behavior and any
/// environment that delivers it. Most modes are expressed on the argv by
/// `build_argv`; a few harnesses (Goose) carry the mode in the environment
/// instead, declared here so the command layer injects it when spawning.
pub struct ModeSpec {
    pub mode: PermissionMode,
    /// Whether this mode blocks on an interactive prompt headlessly.
    pub headless: ModeHeadless,
    /// Environment variables that select this mode (Goose's `GOOSE_MODE`,
    /// OpenCode's `OPENCODE_CONFIG_CONTENT`). Empty when `build_argv` expresses
    /// the mode on the argv. Injected like the harness's `default_env` (so a
    /// config / `--env` value still wins).
    pub env: &'static [(&'static str, &'static str)],
    /// A per-run instruction prepended to the prompt to induce a *behavioral*
    /// mode the harness can't express natively, paired with the enforcement that
    /// `build_argv`/`env` provides. Used for Codex's `plan`: the read-only
    /// sandbox enforces no-mutation and this instruction induces the planning
    /// behavior (mirroring Codex's own interactive Plan-mode template). `None`
    /// for modes a harness expresses natively (their own plan/agent mode already
    /// carries the behavior). Prepended by the command layer; kept single-line.
    pub instruction: Option<&'static str>,
    /// Whether this harness's OWN run acts without asking in this mode.
    ///
    /// A driven turn answers the server's permission requests itself, and the
    /// answer has to be the posture the *same mode* gives without `--control` —
    /// otherwise `--control` silently reshapes the policy, which is the class
    /// of bug that made codex's controlled `bypass` more restricted than its
    /// uncontrolled one. Usually that is the normalized spectrum (`auto` and
    /// `bypass` mean "act without asking"), which is what the `mode` mapping
    /// fills in; it is stated per harness because a CLI can be unable to honor
    /// the spectrum at all — `crush run` auto-approves the whole session, so
    /// its `default` is unattended however it is asked, and a controlled run
    /// that gated it would be stricter than the CLI can be.
    ///
    /// Cross-checked against the argv/environment the harness really builds by
    /// `domain::control`'s `control_mode_parity` grid, so this cannot drift into
    /// a claim the mapping does not back.
    pub posture: ApprovalPosture,
}

/// Shorthand for a mode expressed on the argv (no environment, no instruction)
/// whose posture follows the normalized spectrum: the common case.
const fn mode(mode: PermissionMode, headless: ModeHeadless) -> ModeSpec {
    ModeSpec {
        mode,
        headless,
        env: &[],
        instruction: None,
        posture: ApprovalPosture::of(mode),
    }
}

/// Shorthand for a mode on a harness whose own run cannot gate at all, so it
/// acts without asking whichever mode it was given (crush's `run -q`).
const fn ungated_mode(mode: PermissionMode, headless: ModeHeadless) -> ModeSpec {
    ModeSpec {
        mode,
        headless,
        env: &[],
        instruction: None,
        posture: ApprovalPosture::Unattended,
    }
}

/// A harness's project-scoped config file and the key paths the unified
/// settings merge into. All paths were sourced from each CLI's documentation —
/// never guessed (see the README support matrix for the references).
pub struct SyncSpec {
    /// Project-relative path of the config file to create or merge into.
    pub file: &'static str,
    /// Alternative file names the harness reads with *higher* precedence;
    /// when one exists, oneharness merges into it instead of `file` so it
    /// never creates a second, shadowed config (e.g. crush's `.crush.json`).
    pub alt_files: &'static [&'static str],
    /// Key path `allowed_tools` rules land at in a JSON target (e.g.
    /// `permissions.allow`); `None` there rejects the unified list field for
    /// this harness (OpenCode's `permission` is a policy map, not a list — use
    /// `settings` instead). Always `None` for a
    /// [`SyncFormat::ExecPolicyRules`] target, which has no keys and takes the
    /// list regardless ([`SyncSpec::carries_allowed_tools`]).
    pub allow_path: Option<&'static [&'static str]>,
    /// Key path `denied_tools` rules land at, as `allow_path`. For crush this is
    /// `options.disabled_tools`: the tool is hidden from the agent entirely,
    /// the strongest deny it offers.
    pub deny_path: Option<&'static [&'static str]>,
    /// Key path the `hooks` table lands at (Claude Code's top-level `hooks`).
    pub hooks_path: Option<&'static [&'static str]>,
    /// JSON merged *beneath* any top-level key the fragment touches, so a
    /// partial write still satisfies the harness's schema. Cursor's
    /// `.cursor/cli.json` requires `permissions.allow` and `permissions.deny`
    /// to both exist whenever `permissions` does (its CLI rejects the file
    /// otherwise — caught by the live e2e), so writing only an allow list
    /// must seed an empty deny. Keys the fragment doesn't touch are never
    /// seeded, preserving the "only keys oneharness manages" contract.
    pub schema_seed: Option<&'static str>,
}

/// How a [`SyncSpec`] target file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SyncFormat {
    /// A JSON config file oneharness merges into, key by key.
    Json,
    /// A Codex execpolicy file (`<layer>/rules/*.rules`) holding one
    /// `prefix_rule(...)` per translatable rule, `allowed_tools` as
    /// `decision="allow"` and `denied_tools` as `decision="forbidden"`.
    /// oneharness owns the file wholly and rewrites it whole. It has no keys,
    /// so the spec's key-path fields and `schema_seed` stay `None` (pinned by
    /// the registry test), and it takes both rule lists and nothing else.
    ExecPolicyRules,
}

impl SyncSpec {
    /// The format of [`SyncSpec::file`], read from its extension: Codex loads
    /// execpolicy rules only from files named `*.rules`
    /// (`codex-rs/core/src/exec_policy.rs`'s `RULE_EXTENSION`), so the name
    /// the harness selects the file by is also what declares its format, and
    /// the two cannot drift apart. Every other target is JSON.
    #[must_use]
    pub fn format(&self) -> SyncFormat {
        if self.file.ends_with(".rules") {
            SyncFormat::ExecPolicyRules
        } else {
            SyncFormat::Json
        }
    }

    /// Whether `allowed_tools` has a place in this target: a key path in a
    /// JSON file, or a rule in an execpolicy file.
    #[must_use]
    pub fn carries_allowed_tools(&self) -> bool {
        self.allow_path.is_some() || self.format() == SyncFormat::ExecPolicyRules
    }

    /// Whether `denied_tools` has a place in this target, as
    /// [`SyncSpec::carries_allowed_tools`] asks of `allowed_tools`.
    #[must_use]
    pub fn carries_denied_tools(&self) -> bool {
        self.deny_path.is_some() || self.format() == SyncFormat::ExecPolicyRules
    }
}

/// How a normalized hook reaches one harness. The [`HookShape`] (where present)
/// is the JSON layout [`crate::domain::hooks::render`] produces; the variant is
/// where that JSON — or, for OpenCode, a JS shim — is written. All eight
/// harnesses gate the same pre-tool moment; only the file and the shape differ.
pub enum HookBinding {
    /// Merge the rendered hook under `path` in the harness's *existing* config
    /// file (`SyncSpec.file`/`alt_files`) — hooks share the permissions file
    /// (Claude Code, Qwen, Crush).
    SameFile {
        shape: HookShape,
        path: &'static [&'static str],
    },
    /// Merge into a *dedicated* JSON file, created (seeded with `seed`) if
    /// absent. `file` may contain `{name}` for the plugin identity (Copilot's
    /// per-owner file). Codex, Cursor, Copilot.
    File {
        shape: HookShape,
        file: &'static str,
        path: &'static [&'static str],
        seed: Option<&'static str>,
    },
    /// Goose plugin: a one-time `<plugins_dir>/<name>/plugin.json` manifest plus
    /// the hook merged under `path` in `<plugins_dir>/<name>/hooks/hooks.json`.
    GoosePlugin {
        shape: HookShape,
        plugins_dir: &'static str,
        manifest: &'static str,
        path: &'static [&'static str],
    },
    /// OpenCode can only block from an in-process plugin, so the hook is a JS
    /// shim at `<plugin_dir>/<name>.js` that bridges to the command; rendered
    /// from `template` (not a JSON shape).
    JsPlugin {
        plugin_dir: &'static str,
        template: &'static str,
    },
}

/// The base directory a user-global hook anchors under. Resolved by the I/O
/// layer from the environment; kept abstract here so the registry stays pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookBase {
    /// `$HOME`.
    Home,
    /// `$XDG_CONFIG_HOME`, falling back to `$HOME/.config` (Crush, OpenCode).
    ConfigHome,
}

/// Where a harness reads a *user-global* hook. The install strategy is the
/// matching [`HookBinding`] variant — only the anchor differs, because several
/// harnesses place the global hook at a different relative path than the project
/// one (Copilot's `.github/hooks` becomes `~/.copilot/hooks`; Crush and OpenCode
/// move under the XDG config dir). `{name}` is the plugin identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalHook {
    pub base: HookBase,
    /// Anchor relative to `base`: the settings/hooks file for the JSON-merge
    /// strategies, the `.js` shim for OpenCode, or the plugin directory for
    /// Goose — i.e. the same thing the [`HookBinding`] anchors at under a project.
    pub anchor: &'static str,
}

/// The plan instruction synthesized for Codex, whose `exec` has no native plan
/// mode: the read-only sandbox (`build_argv`) enforces no-mutation and this
/// induces the planning behavior, together reproducing Codex's own interactive
/// Plan mode (which is exactly a read-only sandbox + a plan template). Mirrors
/// the semantics of that template (`codex-rs/.../templates/plan.md`). Kept on a
/// single line — the command layer prepends it to the prompt.
const CODEX_PLAN_INSTRUCTION: &str = "PLAN MODE: research the task and produce an implementation plan only — do not edit or create files and do not run mutating commands (reading and searching files, configs, and docs is allowed); even if asked to execute, treat it as a request to plan the execution. Reply with a short intent paragraph, explicit in-scope vs out-of-scope, then a 6-10 item ordered checklist (discovery, changes, tests, rollout). The task to plan:";

/// Goose plugin manifest, `{name}` filled with the plugin identity. Written once
/// and preserved; the merge is idempotent so re-syncing changes nothing.
const GOOSE_MANIFEST: &str = r#"{
  "name": "{name}",
  "version": "0.1.0",
  "description": "Pre-tool hook installed by oneharness."
}"#;

/// OpenCode plugin shim. `{export}` is a JS-identifier-safe plugin name,
/// `{argv}` the command as a JSON argv array, `{name}` the display name. It
/// spawns the command before every tool call, pipes `{tool_name, tool_input,
/// cwd, session_id}` on stdin (the camelCase `input.sessionID` normalized to
/// snake_case, omitted when absent), throws to block on a
/// `{"decision":"deny"}` reply, and merges a reply's `updated_input` object
/// into the tool's mutable `args` (OpenCode's documented pre-execution
/// mutation point — how `oneharness mock` rewrites a call) — failing open if
/// the command cannot run. Mirrors the known-good allowlister shim; pinned in
/// the `io::hooks` tests.
const OPENCODE_PLUGIN_JS: &str = r#"// {name} OpenCode plugin — installed by oneharness.
//
// OpenCode can block a tool call only from an in-process plugin, so this shim
// bridges to an external command: before any tool runs it spawns the command,
// piping the tool name and arguments as JSON on stdin, throws to block when
// the command replies with {"decision":"deny"}, and applies a reply's
// {"updated_input":{...}} by merging it into the tool's mutable args (an input
// rewrite, for `oneharness mock`). Re-running sync regenerates it.

export const {export} = async ({ directory }) => ({
  "tool.execute.before": async (input, output) => {
    const tool_name = (input && input.tool) || "";
    if (!tool_name) return;
    const args = (output && output.args) || {};
    const cwd = args.workdir || directory || ".";
    const session_id = (input && input.sessionID) || undefined;
    const event = JSON.stringify({ tool_name, tool_input: args, cwd, session_id });

    let stdout = "";
    try {
      const proc = Bun.spawn({argv}, {
        cwd,
        stdin: new TextEncoder().encode(event),
        stdout: "pipe",
        stderr: "ignore",
      });
      stdout = await new Response(proc.stdout).text();
      await proc.exited;
    } catch (_) {
      return; // fail open: if the gate command cannot run, never block
    }

    const trimmed = stdout.trim();
    if (trimmed.length === 0) return; // no objection — let it run
    let decision;
    try {
      decision = JSON.parse(trimmed);
    } catch (_) {
      return; // unparseable output: fail open
    }
    if (decision && decision.decision === "deny") {
      throw new Error(decision.reason || "Blocked by {name}");
    }
    if (
      decision &&
      decision.updated_input &&
      typeof decision.updated_input === "object" &&
      !Array.isArray(decision.updated_input) &&
      output &&
      output.args
    ) {
      Object.assign(output.args, decision.updated_input);
    }
  },
});
"#;

/// All supported harnesses, in a stable order.
pub fn all() -> &'static [HarnessSpec] {
    REGISTRY
}

/// Look up a harness by its canonical id.
pub fn by_id(id: &str) -> Option<&'static HarnessSpec> {
    REGISTRY.iter().find(|h| h.id == id)
}

/// Whether harness `id` has a plugins feature the `plugins` config key toggles.
/// Only Codex does: its `features.plugins` setting is what that key becomes
/// (see [`BuildCtx::plugins`]).
#[must_use]
pub fn has_plugins_toggle(id: &str) -> bool {
    id == "codex"
}

/// Comma-joined list of valid ids, for error messages and help.
pub fn valid_ids() -> String {
    REGISTRY.iter().map(|h| h.id).collect::<Vec<_>>().join(", ")
}

/// Text that is not a well-formed harness identity (see [`HarnessIdentity`] for
/// the guarantee parsing makes, and the one it deliberately does not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessIdentityError;

impl std::fmt::Display for HarnessIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "expected a harness id, optionally qualified by `:<variant>` \
             (`{}`; variant {})",
            valid_ids(),
            crate::domain::config::VARIANT_NAME_PATTERN
        )
    }
}

/// A **well-formed** harness identity: a registry id, optionally qualified by
/// the `:<variant>` naming one auth identity — `claude-code:alternate`, or plain
/// `codex` where no variant is configured.
///
/// This is the granularity a native session token actually belongs to. Each
/// variant points its harness at its own home directory (its own
/// `CLAUDE_CONFIG_DIR`), so each keeps a *disjoint* session namespace and a token
/// minted under one is meaningless under another.
///
/// An unqualified id is a legitimate identity, not a degenerate one: a harness
/// with no `[harness.<id>.variant.*]` has exactly one identity and its selected
/// id carries no suffix, so both spellings parse.
///
/// **What parsing proves, exactly:** the base names a registry harness and the
/// variant — if present — is a legal variant name
/// ([`crate::domain::config::VARIANT_NAME_PATTERN`]). That rules out the text a
/// bare `String` let into the session store: an unknown base id, an empty base or
/// variant around the separator, a second separator, or an out-of-charset
/// variant. Parsing is the only constructor, so holding one is proof of that
/// much.
///
/// **What it deliberately does not prove:** that the variant is *configured*.
/// Variants come from layered config, which this pure type cannot see — and must
/// not, because a stored record outlives the config that minted it. A session
/// bound to `claude-code:alternate` has to stay readable after `alternate` is
/// renamed or dropped; making that record unparseable would turn a config edit
/// into a corrupt store, and a store that cannot say what it was bound to cannot
/// refuse to resume it under something else. Whether an identity is live is
/// settled where the candidates are known — the command layer's session wiring,
/// which continues on it if it is still a candidate and otherwise refuses loudly
/// ([`crate::domain::session::harness_conflict`]) rather than handing its token to
/// a sibling.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct HarnessIdentity(String);

impl HarnessIdentity {
    /// The composed id as written on the wire and matched against a run's
    /// selected ids (`claude-code:alternate`).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The registry id this identity belongs to, without any variant.
    #[must_use]
    pub fn base(&self) -> &str {
        self.0.split_once(':').map_or(self.0.as_str(), |(b, _)| b)
    }

    /// The variant naming the auth identity, or `None` for the harness's single
    /// unqualified identity.
    #[must_use]
    pub fn variant(&self) -> Option<&str> {
        self.0.split_once(':').map(|(_, name)| name)
    }

    /// The registry entry, which parsing has already proven exists.
    #[must_use]
    pub fn spec(&self) -> &'static HarnessSpec {
        by_id(self.base()).expect("a parsed identity names a registry harness")
    }
}

impl std::str::FromStr for HarnessIdentity {
    type Err = HarnessIdentityError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (base, variant) = text
            .split_once(':')
            .map_or((text, None), |(base, name)| (base, Some(name)));
        // A variant that itself contains the separator would re-split differently
        // on the next read, so the charset rule (which excludes `:`) rejects it.
        let variant_ok = variant.is_none_or(crate::domain::config::is_valid_variant_name);
        if by_id(base).is_none() || !variant_ok {
            return Err(HarnessIdentityError);
        }
        Ok(Self(text.to_string()))
    }
}

impl std::fmt::Display for HarnessIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HarnessIdentity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

static REGISTRY: &[HarnessSpec] = &[
    HarnessSpec {
        id: "claude-code",
        display: "Claude Code",
        default_bin: "claude",
        install_hint: "npm install -g @anthropic-ai/claude-code",
        output_format: OutputFormat::Json,
        // The default `json` result carries no transcript; `stream-json` emits the
        // Anthropic content-block stream oneharness normalizes into `events`.
        events_format: Some(OutputFormat::StreamJson),
        // Claude stream-json exposes harness init and terminal aggregation, but
        // no provider-request start; neither is a valid model-latency boundary.
        telemetry: None,
        supports_resume: true,
        session_formats: &[OutputFormat::Json, OutputFormat::StreamJson],
        supports_fork: true,
        fork_reuses_cache: true,
        sync: Some(SyncSpec {
            file: ".claude/settings.json",
            alt_files: &[],
            allow_path: Some(&["permissions", "allow"]),
            deny_path: Some(&["permissions", "deny"]),
            hooks_path: Some(&["hooks"]),
            schema_seed: None,
        }),
        hooks: Some(HookBinding::SameFile {
            shape: HookShape::Nested {
                event: "PreToolUse",
                // Claude Code's PreToolUse hook object accepts a per-hook
                // `timeout` (sourced from the allowlister adapter, which sets
                // one); emit it when a `[[hooks]]` entry provides one.
                with_timeout: true,
            },
            path: &["hooks"],
        }),
        global_hook: Some(GlobalHook {
            base: HookBase::Home,
            anchor: ".claude/settings.json",
        }),
        gate_deny: Some(DenyShape::ClaudeNested),
        // PreToolUse output: `permissionDecision: "allow"` + `updatedInput`
        // (the documented input-rewrite verdict; docs/mock-spy-design.md).
        mock_rewrite: Some(RewriteShape::ClaudeNested),
        // Probe-verified: hooks load from a per-run `--settings <file>` in `-p`
        // mode — the zero-mutation delivery (existing config still applies).
        mock_delivery: Some(MockDelivery::SettingsFlag { flag: "--settings" }),
        default_env: &[],
        native_schema: Some(NativeSchema::ClaudeJsonSchema),
        // Large prompts bypass the argv ceiling: the system prompt rides
        // `--append-system-prompt-file <file>` and the user prompt rides stdin
        // (`-p --input-format text`, positional omitted). Both are documented
        // headless flags (claude 2.1.207). System is carried separately (the file
        // flag), so it is NOT folded into the stdin payload.
        large_input: LargeInput {
            prompt_stdin: true,
            system_rides_prompt: false,
            system_file_flag: Some("--append-system-prompt-file"),
        },
        // `--permission-mode` covers the whole spectrum, all honored under `-p`.
        // `default` maps to `dontAsk` (deny-and-continue), not `default` (which
        // aborts on an un-allowed tool), so the ask flow never hangs headless.
        // `read-only` is `bypassPermissions` with the mutating tools denied (deny
        // rules win even under bypass), distinct from `plan`'s plan workflow.
        modes: &[
            mode(PermissionMode::ReadOnly, ModeHeadless::Clean),
            mode(PermissionMode::Plan, ModeHeadless::Clean),
            mode(PermissionMode::Default, ModeHeadless::Clean),
            mode(PermissionMode::Edit, ModeHeadless::Clean),
            mode(PermissionMode::Auto, ModeHeadless::Clean),
            mode(PermissionMode::Bypass, ModeHeadless::Clean),
        ],
        // `--effort <level>` sets adaptive reasoning headlessly (low/medium/high/
        // max/auto; also `CLAUDE_CODE_EFFORT_LEVEL`). Value forwarded verbatim.
        reasoning: Some(ReasoningDelivery::Flag("--effort")),
        // llmlint: ignore-block[comments_earn_their_place] Why this harness reports the tier it does has to be checkable where the tier is declared; replacing it with a pointer to `docs/harness-usage.md` is what `no_redundant_instruction_pointers` forbids.
        // The `get_usage` control request is the only structured plan-headroom
        // source, and it costs nothing: no user message is sent, so the session
        // reports `num_turns: 0` / `total_cost_usd: 0`.
        // llmlint: ignore-end[comments_earn_their_place]
        usage: UsageSupport::Probed(UsageProbe::ClaudeGetUsage),
        // LIVE-VERIFIED (claude 2.1.220): with `-p --input-format stream-json`
        // the run's own stdin stays open, and a `control_request` /
        // `interrupt` frame aborts the turn (`control_response` success, then a
        // `result` document) while the session survives. The alternative —
        // writing a plain user message mid-turn — was tried and is *silently
        // dropped*, so this is the only mechanism that works. No sidecar server.
        control: Some(ControlShape::ClaudeControlRequest),
        server: None,
        build_argv: argv_claude_code,
    },
    HarnessSpec {
        id: "codex",
        display: "OpenAI Codex CLI",
        default_bin: "codex",
        install_hint: "npm install -g @openai/codex",
        output_format: OutputFormat::Json,
        // The default `codex exec --json` stream already carries both the
        // `thread.started.thread_id` session handle and the `command_execution`
        // transcript, so `--events` needs no format upgrade.
        events_format: None,
        telemetry: Some(TelemetrySpec {
            format: OutputFormat::Json,
            trace: TelemetryTrace::CodexJson,
        }),
        supports_resume: true,
        session_formats: &[OutputFormat::Json, OutputFormat::StreamJson],
        supports_fork: false,
        fork_reuses_cache: false,
        // Codex loads execpolicy rules from `<layer>/rules/*.rules` for every
        // active config layer, the project's `.codex/` included once that
        // project is trusted (`codex-rs/core/src/exec_policy.rs`, codex 0.80.0
        // onward; verified live on 0.156.1 by `oh_codex_rules_enforce`). The
        // file is oneharness's alone and holds only `prefix_rule`s — the
        // `.rules` name is what makes it `SyncFormat::ExecPolicyRules` — so it
        // has no key paths.
        sync: Some(SyncSpec {
            file: ".codex/rules/oneharness.rules",
            alt_files: &[],
            allow_path: None,
            deny_path: None,
            hooks_path: None,
            schema_seed: None,
        }),
        hooks: Some(HookBinding::File {
            shape: HookShape::Nested {
                event: "PreToolUse",
                with_timeout: false,
            },
            file: ".codex/hooks.json",
            path: &["hooks"],
            seed: None,
        }),
        global_hook: Some(GlobalHook {
            base: HookBase::Home,
            anchor: ".codex/hooks.json",
        }),
        gate_deny: Some(DenyShape::ClaudeNested),
        // Probe-verified (2026-07-06, codex v0.142.5): `codex exec` DOES load
        // project `.codex/hooks.json` and honors the claude-nested
        // `updatedInput` rewrite — but ONLY when the invocation carries
        // `-c features.hooks=true --dangerously-bypass-hook-trust` (the
        // `projects.<dir>.trust_level="trusted"` config route yields zero hook
        // events). oneharness does not add those flags itself; the caller
        // passes them per run (config `args` / `--` passthrough), as the
        // `oh_mock_enforce codex` phase does.
        mock_rewrite: Some(RewriteShape::ClaudeNested),
        // Project .codex/hooks.json, restored after the run; the opt-in flags
        // the probe proved necessary are auto-appended to the argv.
        mock_delivery: Some(MockDelivery::ProjectHooks {
            extra_args: &[
                "-c",
                "features.hooks=true",
                "--dangerously-bypass-hook-trust",
            ],
        }),
        default_env: &[],
        // Large prompts ride stdin: `codex exec -` (the `-` sentinel forces the
        // prompt to be read from stdin, and works with `resume <id> -` too —
        // sourced from the exec CLI's clap docs). Codex has no system-prompt flag,
        // so oneharness prepends `--system` into the prompt; that combined text is
        // what rides stdin (`system_rides_prompt`).
        large_input: LargeInput {
            prompt_stdin: true,
            system_rides_prompt: true,
            system_file_flag: None,
        },
        // Codex `exec` *does* have a native schema flag (`--output-schema <file>`),
        // but it takes a schema FILE (not inline) and is reportedly ignored once
        // the agent uses tools: https://github.com/openai/codex/issues/15451 — so
        // structured output uses the more reliable prompt-based path for it today.
        // To wire it up once that's resolved: add a `CodexOutputSchema` variant to
        // `structured::NativeSchema`, set it here, add a `--output-schema` arm to
        // `argv_codex` (the command layer must materialize the schema to a temp
        // file and pass its path via `BuildCtx.schema`), and teach
        // `structured::extract_value` where Codex reports the conforming value.
        native_schema: None,
        // `codex exec` gates by sandbox, not by op-type, and downgrades approval
        // to `never` (it never hangs — out-of-sandbox actions fail closed and the
        // agent continues). `read-only` is the (OS-enforced) read-only sandbox,
        // `auto` is `workspace-write`. Codex has no *native* plan mode in `exec`
        // (its TUI Plan mode = read-only sandbox + a plan instruction, both
        // reproducible here), so `plan` is synthesized: same read-only sandbox
        // plus the `instruction` below. No edit-vs-shell split, so no `edit`.
        modes: &[
            mode(PermissionMode::ReadOnly, ModeHeadless::Clean),
            ModeSpec {
                mode: PermissionMode::Plan,
                headless: ModeHeadless::Clean,
                env: &[],
                posture: ApprovalPosture::Gated,
                instruction: Some(CODEX_PLAN_INSTRUCTION),
            },
            mode(PermissionMode::Default, ModeHeadless::Clean),
            mode(PermissionMode::Auto, ModeHeadless::Clean),
            mode(PermissionMode::Bypass, ModeHeadless::Clean),
        ],
        // `-c model_reasoning_effort=<level>` (minimal/low/medium/high/xhigh).
        // `-c key=value` is Codex's documented headless override and is already
        // exercised live by the codex mock phase (`-c features.hooks=true`); the
        // value is forwarded verbatim (Codex parses it, falling back to a string).
        reasoning: Some(ReasoningDelivery::ConfigKv("model_reasoning_effort")),
        // llmlint: ignore-block[comments_earn_their_place] Why this harness reports the tier it does has to be checkable where the tier is declared; replacing it with a pointer to `docs/harness-usage.md` is what `no_redundant_instruction_pointers` forbids.
        // `exec --json` carries no rate-limit metadata at all, so usage needs its
        // own app-server probe rather than piggybacking on a dispatch.
        // llmlint: ignore-end[comments_earn_their_place]
        usage: UsageSupport::Probed(UsageProbe::CodexAppServer),
        // LIVE-VERIFIED: `turn/interrupt {threadId,turnId}` over the
        // `codex app-server` JSON-RPC stdio protocol stops the turn (step files
        // frozen for 15s). oneharness spawns the app-server as the run's OWN
        // child and drives the thread/turn lifecycle on it, so the interrupt
        // rides the same open stdin — no shared sidecar, hence no `ServerSpec`.
        // Model, cwd, sandbox, and approvals are negotiated on the wire.
        // NOT `app-server daemon`: that needs a managed standalone install this
        // project does not use, and self-updates from a fixed path.
        control: Some(ControlShape::CodexAppServer),
        server: None,
        build_argv: argv_codex,
    },
    HarnessSpec {
        id: "opencode",
        display: "OpenCode",
        default_bin: "opencode",
        install_hint: "npm install -g opencode-ai",
        output_format: OutputFormat::Json,
        // Default `json` (JSONL) already carries the `tool` parts, so no upgrade.
        events_format: None,
        telemetry: Some(TelemetrySpec {
            format: OutputFormat::Json,
            trace: TelemetryTrace::OpenCodeJson,
        }),
        supports_resume: true,
        session_formats: &[OutputFormat::Json],
        supports_fork: true,
        // OpenCode can fork, but its fork re-sends the branched conversation cold
        // (measured: the fan-out reads no cache and re-writes the whole prefix),
        // so a fork-based min-tokens would raise tokens, not lower them.
        fork_reuses_cache: false,
        sync: Some(SyncSpec {
            file: "opencode.json",
            alt_files: &[],
            allow_path: None,
            deny_path: None,
            hooks_path: None,
            schema_seed: None,
        }),
        hooks: Some(HookBinding::JsPlugin {
            plugin_dir: ".opencode/plugin",
            template: OPENCODE_PLUGIN_JS,
        }),
        global_hook: Some(GlobalHook {
            base: HookBase::ConfigHome,
            anchor: "opencode/plugin/{name}.js",
        }),
        gate_deny: Some(DenyShape::Decision("deny")),
        // The oneharness plugin shim applies `updated_input` by merging it into
        // the tool's mutable `args` at `tool.execute.before` (the officially
        // documented mutation point of OpenCode's plugin API).
        mock_rewrite: Some(RewriteShape::OpencodeShim),
        mock_delivery: Some(MockDelivery::ProjectHooks { extra_args: &[] }),
        default_env: &[],
        native_schema: None,
        // Large prompts ride stdin: `opencode run` reads the prompt from stdin
        // when no positional message is given and stdin is piped (auto-detected,
        // no flag/sentinel — sourced from `run.ts`). No system flag, so `--system`
        // is prepended and the combined text rides stdin (`system_rides_prompt`).
        large_input: LargeInput {
            prompt_stdin: true,
            system_rides_prompt: true,
            system_file_flag: None,
        },
        // The built-in `plan` agent is read-only, so `plan` and `read-only` both
        // map to it. `default` is clean headless: OpenCode's out-of-box default
        // is `allow`, and even an `ask` permission *auto-rejects* (deny-and-
        // continue) under `opencode run` rather than blocking — it never hangs.
        // `edit` (auto-approve edits, gate bash) is delivered per-run through the
        // inline-config env var `OPENCODE_CONFIG_CONTENT` (highest-precedence
        // config, set without touching `opencode.json`) — no argv flag exists.
        // Bypass auto-approves all but explicit denies. There is no classifier
        // `auto`.
        modes: &[
            mode(PermissionMode::ReadOnly, ModeHeadless::Clean),
            mode(PermissionMode::Plan, ModeHeadless::Clean),
            mode(PermissionMode::Default, ModeHeadless::Clean),
            ModeSpec {
                mode: PermissionMode::Edit,
                headless: ModeHeadless::Clean,
                env: &[(
                    "OPENCODE_CONFIG_CONTENT",
                    r#"{"permission":{"edit":"allow","bash":"deny"}}"#,
                )],
                instruction: None,
                // The config above IS this mode's policy, and a turn submitted
                // to a pooled server cannot carry it — so `--control --mode
                // edit` is refused outright rather than run under the server's
                // own policy. This posture is only the wire backstop if one ever
                // arrives anyway; declining is the safe way to answer it.
                posture: ApprovalPosture::Gated,
            },
            mode(PermissionMode::Bypass, ModeHeadless::Clean),
        ],
        // OpenCode sets reasoning through provider/model `options` in its config
        // file (`reasoningEffort` / `thinking.budgetTokens`), not a `run` flag —
        // the `sync`-path follow-up, not an argv delivery.
        reasoning: None,
        // llmlint: ignore-block[comments_earn_their_place] Why this harness reports the tier it does has to be checkable where the tier is declared; replacing it with a pointer to `docs/harness-usage.md` is what `no_redundant_instruction_pointers` forbids.
        // OpenCode Zen is pay-as-you-go: you are charged per request and top up a
        // balance. Nothing resets, so "remaining usage against a reset interval"
        // is not a defined quantity here — `opencode stats` is spend-to-date, a
        // different measurement that answers nothing about headroom.
        // llmlint: ignore-end[comments_earn_their_place]
        usage: UsageSupport::NoPlanQuota,
        control: Some(ControlShape::OpencodeHttp),
        server: Some(ServerSpec {
            launch: &["serve"],
            address_args: &["--port", "{address}"],
            key_env: &[],
            transport: ServerTransport::Tcp,
        }),
        build_argv: argv_opencode,
    },
    HarnessSpec {
        id: "goose",
        display: "Goose",
        default_bin: "goose",
        install_hint: "see https://block.github.io/goose/docs/getting-started/installation",
        output_format: OutputFormat::Text,
        // Events pending investigation (see the events matrix).
        events_format: None,
        telemetry: None,
        supports_resume: true,
        session_formats: &[],
        supports_fork: false,
        fork_reuses_cache: false,
        sync: None,
        hooks: Some(HookBinding::GoosePlugin {
            shape: HookShape::Nested {
                event: "PreToolUse",
                with_timeout: true,
            },
            plugins_dir: ".agents/plugins",
            manifest: GOOSE_MANIFEST,
            path: &["hooks"],
        }),
        global_hook: Some(GlobalHook {
            base: HookBase::Home,
            anchor: ".agents/plugins/{name}",
        }),
        gate_deny: Some(DenyShape::Decision("block")),
        // Goose hooks can only block (exit 2 / `decision: "block"`); its
        // protocol has no input-rewrite verdict — deny is its mock ceiling.
        mock_rewrite: None,
        // Deny/spy rules still deliver (its project plugin hooks fire live).
        mock_delivery: Some(MockDelivery::ProjectHooks { extra_args: &[] }),
        default_env: &[],
        native_schema: None,
        // Large prompts ride stdin via the `-i -` sentinel (`--instructions -`
        // reads the whole prompt from stdin — sourced from goose-cli's clap defs);
        // build_argv swaps `-t <prompt>` for `-i -`. Goose's `--system` is inline
        // TEXT with no file/stdin route, so a large *system* prompt has no off-argv
        // path here (goosehints is the file mechanism, but it is project-scoped,
        // not per-run) — hence `system_rides_prompt: false` and no file flag; the
        // command layer warns if a system prompt is too large to inline.
        large_input: LargeInput {
            prompt_stdin: true,
            system_rides_prompt: false,
            system_file_flag: None,
        },
        // Goose has no mode flag on `goose run`; the mode is the `GOOSE_MODE`
        // environment variable (highest precedence over its config.yaml). None of
        // these hang headlessly: `approve`/`smart_approve` fail *closed* with an
        // error when a tool needs approval (exit non-zero rather than block),
        // `auto` approves everything. Goose has no headless plan workflow and no
        // per-run read-only-with-reads (its `chat` mode disables *all* tools,
        // reads included, so it is neither) — `plan`/`read-only`/`edit` are
        // absent. Bypass MUST set `GOOSE_MODE=auto` explicitly: leaving it unset
        // would inherit the user's config, which may be a fail-closed mode.
        modes: &[
            ModeSpec {
                mode: PermissionMode::Default,
                headless: ModeHeadless::Clean,
                env: &[("GOOSE_MODE", "approve")],
                instruction: None,
                posture: ApprovalPosture::Gated,
            },
            ModeSpec {
                mode: PermissionMode::Auto,
                headless: ModeHeadless::Clean,
                env: &[("GOOSE_MODE", "smart_approve")],
                instruction: None,
                posture: ApprovalPosture::Unattended,
            },
            ModeSpec {
                mode: PermissionMode::Bypass,
                headless: ModeHeadless::Clean,
                env: &[("GOOSE_MODE", "auto")],
                instruction: None,
                posture: ApprovalPosture::Unattended,
            },
        ],
        // Goose carries reasoning effort in provider config (`goose configure` /
        // config.yaml), with no per-run headless flag — no argv delivery.
        reasoning: None,
        // llmlint: ignore-block[comments_earn_their_place] Why this harness reports the tier it does has to be checkable where the tier is declared; replacing it with a pointer to `docs/harness-usage.md` is what `no_redundant_instruction_pointers` forbids.
        // Goose is an open-source agent with no first-party inference plan — it
        // routes to whichever provider `GOOSE_PROVIDER` selects, so there is no
        // Goose quota to have headroom in. (Its Copilot passthrough shares the
        // GitHub token, so that headroom is readable under `copilot` instead.)
        // llmlint: ignore-end[comments_earn_their_place]
        usage: UsageSupport::NoPlanQuota,
        // LIVE-VERIFIED: the ACP `session/cancel` NOTIFICATION over `goose acp`
        // stops the turn (step files frozen for 15s) — the same protocol and the
        // same client code copilot is proven on. Two rules the client must
        // honor: answer `session/request_permission` (goose blocks indefinitely
        // and never begins work otherwise), and send cancel WITHOUT an id (with
        // one, goose answers `-32601 Method not found` and the work carries on).
        // Goose then reports `stopReason: "end_turn"` and emits nothing else at
        // all, so the cancellation is recorded from oneharness's own side.
        control: Some(ControlShape::AcpCancel),
        server: None,
        build_argv: argv_goose,
    },
    HarnessSpec {
        id: "qwen",
        display: "Qwen Code",
        default_bin: "qwen",
        install_hint: "npm install -g @qwen-code/qwen-code",
        output_format: OutputFormat::Text,
        // Qwen's `--output-format stream-json` emits the Anthropic content-block
        // stream oneharness normalizes into `events` (its default text has no
        // transcript). Mapped to `--output-format stream-json` in `argv_qwen`.
        events_format: Some(OutputFormat::StreamJson),
        telemetry: None,
        supports_resume: true,
        session_formats: &[OutputFormat::StreamJson, OutputFormat::Json],
        supports_fork: false,
        fork_reuses_cache: false,
        sync: Some(SyncSpec {
            file: ".qwen/settings.json",
            alt_files: &[],
            allow_path: Some(&["permissions", "allow"]),
            deny_path: Some(&["permissions", "deny"]),
            hooks_path: None,
            schema_seed: None,
        }),
        hooks: Some(HookBinding::SameFile {
            shape: HookShape::Nested {
                event: "PreToolUse",
                with_timeout: false,
            },
            path: &["hooks"],
        }),
        global_hook: Some(GlobalHook {
            base: HookBase::Home,
            anchor: ".qwen/settings.json",
        }),
        gate_deny: Some(DenyShape::ClaudeNested),
        // Qwen's docs describe the same `updatedInput` rewrite as Claude Code,
        // but it is NOT honored live: with the hook demonstrably firing (its
        // gate deny passed at the same global scope) and the allow+updatedInput
        // verdict emitted, the ORIGINAL command still ran — measured by
        // oh_mock_enforce on all three OSes (2026-07-06, `--yolo`). Absent per
        // the measured-not-guessed rule until the explore-hooks probe sources a
        // shape qwen actually applies; `oneharness mock qwen` is deny-only.
        mock_rewrite: None,
        // Qwen fires only *user*-scoped hooks headlessly (project hooks sit
        // behind folder trust), so a project-scope one-shot install would be
        // silently inert — refused loudly; use `sync --global` into a
        // redirected HOME instead (the oh_hook_enforce qwen pattern).
        mock_delivery: None,
        default_env: &[("QWEN_CODE_SUPPRESS_YOLO_WARNING", "1")],
        native_schema: None,
        // Large prompts ride stdin: `qwen` reads the prompt from stdin when piped
        // and `-p` is omitted (sourced from qwen-code's `gemini.tsx`). oneharness
        // has no system flag mapping for qwen (it prepends `--system`), so the
        // combined text rides stdin (`system_rides_prompt`). (Qwen also has a
        // file-based system override via the `QWEN_SYSTEM_MD` env var, but it
        // *replaces* the whole system prompt — the prepend-into-stdin path is
        // simpler and consistent with the other prependers.)
        large_input: LargeInput {
            prompt_stdin: true,
            system_rides_prompt: true,
            system_file_flag: None,
        },
        // `--approval-mode` spans the whole spectrum, all clean headless: current
        // qwen-code *deny-and-continues* a gated tool in non-interactive mode
        // (auto-deny + the agent loop proceeds, process exits 0) rather than
        // hanging. So `default` denies gated tools and continues; `auto-edit`
        // genuinely auto-applies edits while denying shell; `auto` runs the
        // classifier; none block. Qwen's only read-only mechanism is its plan
        // mode, so `read-only` coincides with `plan` (both → `--approval-mode
        // plan`).
        modes: &[
            mode(PermissionMode::ReadOnly, ModeHeadless::Clean),
            mode(PermissionMode::Plan, ModeHeadless::Clean),
            mode(PermissionMode::Default, ModeHeadless::Clean),
            mode(PermissionMode::Edit, ModeHeadless::Clean),
            mode(PermissionMode::Auto, ModeHeadless::Clean),
            mode(PermissionMode::Bypass, ModeHeadless::Clean),
        ],
        // Qwen sets reasoning via `settings.json` `samplingParams`
        // (`reasoning_effort` / `thinking.budget_tokens`), not a CLI flag — the
        // `sync`-path follow-up, not an argv delivery.
        reasoning: None,
        // llmlint: ignore-block[comments_earn_their_place] Why this harness reports the tier it does has to be checkable where the tier is declared; replacing it with a pointer to `docs/harness-usage.md` is what `no_redundant_instruction_pointers` forbids.
        // The Alibaba Cloud Coding Plan carries a documented **weekly** quota,
        // but neither its size nor a reader is published: `qwen auth` was removed
        // ("Configure authentication (removed)"), no usage/stats/quota subcommand
        // exists, and the bundle carries a provider binding with no quota
        // accessor. Nothing is readable here — not even the active auth mode.
        // llmlint: ignore-end[comments_earn_their_place]
        usage: UsageSupport::NoHeadroomReader,
        control: None,
        server: None,
        build_argv: argv_qwen,
    },
    HarnessSpec {
        id: "crush",
        display: "Crush",
        default_bin: "crush",
        install_hint: "npm install -g @charmland/crush",
        output_format: OutputFormat::Text,
        // Events pending investigation (see the events matrix).
        events_format: None,
        telemetry: None,
        supports_resume: true,
        session_formats: &[],
        supports_fork: false,
        fork_reuses_cache: false,
        sync: Some(SyncSpec {
            file: "crush.json",
            alt_files: &[".crush.json"],
            allow_path: Some(&["permissions", "allowed_tools"]),
            deny_path: Some(&["options", "disabled_tools"]),
            hooks_path: None,
            schema_seed: None,
        }),
        hooks: Some(HookBinding::SameFile {
            shape: HookShape::Flat {
                event: "PreToolUse",
            },
            path: &["hooks"],
        }),
        global_hook: Some(GlobalHook {
            base: HookBase::ConfigHome,
            anchor: "crush/crush.json",
        }),
        gate_deny: Some(DenyShape::Decision("deny")),
        // Crush's PreToolUse stdout documents `updated_input` (a shallow-merge
        // patch of the tool input) beside `decision: "allow"`.
        mock_rewrite: Some(RewriteShape::CrushFlat),
        mock_delivery: Some(MockDelivery::ProjectHooks { extra_args: &[] }),
        default_env: &[],
        native_schema: None,
        // Large prompts ride stdin: `crush run` prepends piped stdin to any
        // positional, so with the positional omitted the prompt IS the stdin
        // content (`cat file | crush run` / `crush run < file` — sourced from
        // `MaybePrependStdin` in root.go). No system flag, so `--system` is
        // prepended and the combined text rides stdin (`system_rides_prompt`).
        large_input: LargeInput {
            prompt_stdin: true,
            system_rides_prompt: true,
            system_file_flag: None,
        },
        // `crush run` auto-approves the whole session, so it never hangs — but it
        // also cannot gate, so `default` and `bypass` behave the same (bypass
        // adds the explicit `--yolo`). There is no plan/edit/auto mode on `run`.
        modes: &[
            ungated_mode(PermissionMode::Default, ModeHeadless::Clean),
            ungated_mode(PermissionMode::Bypass, ModeHeadless::Clean),
        ],
        // Crush sets reasoning through model options in `crush.json`
        // (`reasoning_effort` / `think`), not a CLI flag — the `sync`-path
        // follow-up, not an argv delivery.
        reasoning: None,
        // llmlint: ignore-block[comments_earn_their_place] Why this harness reports the tier it does has to be checkable where the tier is declared; replacing it with a pointer to `docs/harness-usage.md` is what `no_redundant_instruction_pointers` forbids.
        // Charm Hyper's credits do refresh monthly, so a quota exists — but no
        // balance command or API is documented, and the stripped Go binary
        // carries zero `quota`/`entitlement`/`credits remaining` strings. `crush
        // stats` is local SQLite spend-to-date, not headroom.
        // llmlint: ignore-end[comments_earn_their_place]
        usage: UsageSupport::NoHeadroomReader,
        // LIVE-VERIFIED (crush v0.87.0): `POST
        // /v1/workspaces/{id}/agent/sessions/{sid}/cancel` against a pooled
        // `crush server` stops the turn (step files frozen for 15s). The turn is
        // submitted to that server, never to `crush run` — its `run` has no
        // attach flag, so a CLI-driven turn is unreachable from the route. Two
        // details the client must get right: `client_id` travels in the BODY
        // creating the workspace and as a QUERY parameter everywhere else (a
        // mismatch answers a bare `{"message":"invalid client_id"}`), and the
        // prompt POST answers 202 with the turn running in the background, so
        // completion is read off the event stream.
        control: Some(ControlShape::CrushHttp),
        server: Some(ServerSpec {
            launch: &["server"],
            address_args: &["-H", "unix://{address}"],
            // Crush resolves its provider from the ambient environment, and no
            // single variable selects one — so nothing here narrows the pool
            // key beyond the harness itself.
            key_env: &[],
            transport: ServerTransport::UnixSocket,
        }),
        build_argv: argv_crush,
    },
    HarnessSpec {
        id: "copilot",
        display: "GitHub Copilot CLI",
        default_bin: "copilot",
        install_hint: "npm install -g @github/copilot",
        output_format: OutputFormat::Text,
        // Events pending investigation (see the events matrix).
        events_format: None,
        telemetry: None,
        supports_resume: true,
        session_formats: &[],
        supports_fork: false,
        fork_reuses_cache: false,
        sync: None,
        hooks: Some(HookBinding::File {
            shape: HookShape::CrossShell {
                event: "preToolUse",
            },
            file: ".github/hooks/{name}.json",
            path: &["hooks"],
            seed: Some(r#"{"version":1}"#),
        }),
        global_hook: Some(GlobalHook {
            base: HookBase::Home,
            anchor: ".copilot/hooks/{name}.json",
        }),
        gate_deny: Some(DenyShape::CopilotFlat),
        // Probe-REFUTED (2026-07-06): copilot's repo `.github/hooks/*.json`
        // hooks produced ZERO events across every headless `-p` experiment,
        // even though the agent demonstrably used its shell tool — despite its
        // docs demonstrating `-p` hooks. With no hook firing there is nothing
        // to rewrite; absent until a live run shows its hooks loading at all.
        mock_rewrite: None,
        // No hook has ever fired headlessly (probe: zero events) — nothing any
        // delivery could make the CLI run, so the flag is refused loudly.
        mock_delivery: None,
        default_env: &[],
        native_schema: None,
        // Large prompts ride stdin: piping into `copilot` with NO `-p` is read as
        // the prompt (a `-p` value makes the pipe be ignored — sourced from the
        // Copilot CLI docs), so build_argv drops `-p`/the positional entirely. No
        // system flag, so `--system` is prepended and the combined text rides
        // stdin (`system_rides_prompt`).
        large_input: LargeInput {
            prompt_stdin: true,
            system_rides_prompt: true,
            system_file_flag: None,
        },
        // `--mode plan` is a real read-only plan mode; `read-only` is allow-all
        // with `write`/`shell` denied (deny beats allow); `edit` allows
        // `write`/`read` but not `shell`, so edits run and shell is auto-denied
        // (the headless form of "gate shell"); bypass is the allow-all trio.
        // Without any, `-p` auto-denies gated tools and continues (never hangs),
        // so `default` is `Clean`. No classifier `auto`, so it is absent.
        modes: &[
            mode(PermissionMode::ReadOnly, ModeHeadless::Clean),
            mode(PermissionMode::Plan, ModeHeadless::Clean),
            mode(PermissionMode::Default, ModeHeadless::Clean),
            mode(PermissionMode::Edit, ModeHeadless::Clean),
            mode(PermissionMode::Bypass, ModeHeadless::Clean),
        ],
        // `--reasoning-effort <low|medium|high>` (alias `--effort`), documented in
        // Copilot's CLI flags reference + changelog. NOTE: the docs don't
        // explicitly confirm it is honored under `-p`, and Copilot has a history
        // of headless features silently not firing here (its hooks were
        // probe-refuted headlessly) — so the live `oh_reasoning_enforce` drift
        // alarm is the honoring proof this one especially wants. The full flag
        // name is used (not the `--effort` alias) to stay unambiguous on the argv.
        reasoning: Some(ReasoningDelivery::Flag("--reasoning-effort")),
        // llmlint: ignore-block[comments_earn_their_place] Why this harness reports the tier it does has to be checkable where the tier is declared; replacing it with a pointer to `docs/harness-usage.md` is what `no_redundant_instruction_pointers` forbids.
        // Read out of band: a GitHub bearer token is the entire credential
        // requirement, so this answers with no Copilot CLI installed and before a
        // run rather than after a turn is spent. The CLI's own JSONL quota events
        // are unreachable as oneharness wires Copilot (text mode).
        // llmlint: ignore-end[comments_earn_their_place]
        usage: UsageSupport::Probed(UsageProbe::CopilotUserEndpoint),
        // LIVE-VERIFIED: the ACP `session/cancel` NOTIFICATION over
        // `copilot --acp` stops the turn (step files frozen for 15s). Two rules
        // the client must honor: answer `session/request_permission` (copilot
        // blocks indefinitely and never starts work otherwise), and send cancel
        // WITHOUT an id. Copilot then reports `stopReason: "end_turn"` plus a
        // text chunk reading "Info: Operation cancelled by user", so the
        // cancellation is recorded from oneharness's own side, never read off
        // the harness's stop reason.
        control: Some(ControlShape::AcpCancel),
        server: None,
        build_argv: argv_copilot,
    },
    HarnessSpec {
        id: "cursor",
        display: "Cursor CLI",
        default_bin: "cursor-agent",
        install_hint: "see https://docs.cursor.com/en/cli/overview",
        output_format: OutputFormat::StreamJson,
        // Default `stream-json` already carries the tool transcript, so no upgrade.
        events_format: None,
        telemetry: None,
        supports_resume: true,
        session_formats: &[OutputFormat::StreamJson],
        supports_fork: false,
        fork_reuses_cache: false,
        sync: Some(SyncSpec {
            file: ".cursor/cli.json",
            alt_files: &[],
            allow_path: Some(&["permissions", "allow"]),
            deny_path: Some(&["permissions", "deny"]),
            hooks_path: None,
            schema_seed: Some(r#"{"permissions":{"allow":[],"deny":[]}}"#),
        }),
        hooks: Some(HookBinding::File {
            shape: HookShape::CommandOnly {
                events: &[
                    "beforeShellExecution",
                    "beforeReadFile",
                    "beforeMCPExecution",
                    // `preToolUse` is the ONLY cursor event whose reply can
                    // rewrite the tool's input (`updated_input`, probe-verified
                    // headlessly 2026-07-06); the three `before*` events are
                    // allow/deny-only. Wiring it alongside them means a shell
                    // call invokes the hook command more than once — the gate's
                    // verdicts are idempotent, and the mock's rewrite rides
                    // this event.
                    "preToolUse",
                ],
            },
            file: ".cursor/hooks.json",
            path: &["hooks"],
            seed: Some(r#"{"version":1}"#),
        }),
        global_hook: Some(GlobalHook {
            base: HookBase::Home,
            anchor: ".cursor/hooks.json",
        }),
        gate_deny: Some(DenyShape::CursorPermission),
        // Probe-verified (2026-07-06): cursor honors a `preToolUse` reply of
        // `{"permission":"allow","updated_input":{…}}` headlessly — the
        // original command in the preToolUse event, the rewritten one in the
        // subsequent before/afterShellExecution events. The `preToolUse` event
        // is wired into the hook binding above for exactly this.
        mock_rewrite: Some(RewriteShape::CursorPermission),
        mock_delivery: Some(MockDelivery::ProjectHooks { extra_args: &[] }),
        default_env: &[],
        native_schema: None,
        // Large prompts ride stdin: `cursor-agent -p` reads the prompt from stdin
        // when the positional is omitted — probe-verified live (2026-07-11,
        // scripts/explore-cursor-stdin.sh: piped stdin with no positional
        // round-tripped the marker both with and without `-p`; the `-` sentinel did
        // NOT). Cursor has no system-prompt flag, so `--system` is prepended and
        // the combined text rides stdin (`system_rides_prompt`).
        large_input: LargeInput {
            prompt_stdin: true,
            system_rides_prompt: true,
            system_file_flag: None,
        },
        // `--mode plan` is the read-only plan mode; `--mode ask` is read-only
        // Q&A (no plan workflow) → `read-only`; `--force` is bypass. Without
        // `--force` a gated tool stalls (Cursor proposes-not-applies, with no
        // fail-fast deny flag), so `default` (`--trust` only) is `Hangs`.
        // Edit/shell gating is a `permissions` config concern (synced).
        modes: &[
            mode(PermissionMode::ReadOnly, ModeHeadless::Clean),
            mode(PermissionMode::Plan, ModeHeadless::Clean),
            mode(PermissionMode::Default, ModeHeadless::Hangs),
            mode(PermissionMode::Bypass, ModeHeadless::Clean),
        ],
        // Cursor has no standalone reasoning flag; effort is a `-<tier>` suffix on
        // the model id, `--model 'claude-opus-4-8-high'` (verified live — the CLI
        // rejects a bracketed `model[effort=…]` and lists ids that bake the tier
        // into the name). Requires a model to attach to; the live
        // `oh_reasoning_enforce cursor` phase is the honoring proof + drift alarm.
        reasoning: Some(ReasoningDelivery::ModelSuffix),
        // llmlint: ignore-block[comments_earn_their_place] Why this harness reports the tier it does has to be checkable where the tier is declared; replacing it with a pointer to `docs/harness-usage.md` is what `no_redundant_instruction_pointers` forbids.
        // Plan tier only. Cursor's dollar pools live behind `getCurrentPeriodUsage`,
        // whose sole callsite is the interactive TUI — zero non-interactive or
        // run-output callsites — so `about --format json`'s `subscriptionTier` is
        // the whole non-interactive surface.
        // llmlint: ignore-end[comments_earn_their_place]
        usage: UsageSupport::Probed(UsageProbe::CursorAbout),
        control: None,
        server: None,
        build_argv: argv_cursor,
    },
];

/// The built-in tools a `read-only` Claude Code run may use, delivered as
/// `--tools` — the set it PERMITS, so the set it withholds is everything else.
///
/// It was the mirror image until claude 2.1.220: `bypassPermissions` with
/// `--disallowedTools Bash Edit Write NotebookEdit`. Naming what is forbidden is
/// fail-open, and the CLI's own tool set is what moved under it. That version's
/// built-ins include `Task`, which hands the turn to a subagent carrying the
/// full set, so an agent with no `Bash` of its own delegated the shell call and
/// the write landed (reproduced directly: the same argv plus "use the Agent tool
/// to run `touch …`" creates the file; the live Windows leg that caught it
/// reported `origin: {"kind":"task-notification"}` on a turn that did the write).
/// An allowlist has no such tail: a tool the CLI adds next is out of reach
/// because reaching it means being named here.
///
/// Sourced from `claude --help` (2.1.220) — `--tools` takes names "from the
/// built-in set", and the `system`/`init` frame of a real run echoes back
/// exactly these five.
const CLAUDE_READ_ONLY_TOOLS: &[&str] = &["Read", "Grep", "Glob", "WebFetch", "WebSearch"];

/// Claude Code's `--permission-mode` token for each normalized mode. `Default`
/// maps to `dontAsk` (deny any un-allowed tool and continue) rather than
/// `default` (which *aborts* the `-p` run on an un-allowed tool): the ask flow
/// then completes headlessly instead of failing on the first prompt. `ReadOnly`
/// rides `bypassPermissions` (allow-all, no prompts) with the available tool set
/// narrowed to [`CLAUDE_READ_ONLY_TOOLS`] separately.
fn claude_permission_mode(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Plan => "plan",
        PermissionMode::ReadOnly => "bypassPermissions",
        PermissionMode::Default => "dontAsk",
        PermissionMode::Edit => "acceptEdits",
        PermissionMode::Auto => "auto",
        PermissionMode::Bypass => "bypassPermissions",
    }
}

/// `claude -p <prompt> --permission-mode <mode> [--tools …] [--model M]
/// [--append-system-prompt S] [--resume <id> [--fork-session]] --output-format json`
/// (`--resume` continues a session by id; `--fork-session` branches a new session
/// from it instead of appending — the session id is read from the result JSON's
/// `session_id`).
fn argv_claude_code(c: &BuildCtx) -> Vec<String> {
    // `-p` is print mode. Normally the prompt is the positional after it; for a
    // large prompt the command layer selects `PromptDelivery::Stdin`, so we drop
    // the positional and add `--input-format text` — claude then reads the prompt from stdin,
    // off the argv (no `E2BIG`). Sourced from `claude --help` (2.1.207).
    let mut a = vec![c.bin.into(), "-p".into()];
    if c.delivery.is_control_stream() {
        // A control-enabled run reads a *stream* of JSON messages from stdin
        // rather than one blob, which is what lets the handle stay open past the
        // prompt for an out-of-band `control_request`. The prompt is the first
        // frame the command layer writes, so it leaves the positional off.
        a.push("--input-format".into());
        a.push("stream-json".into());
    } else if c.delivery.is_stdin_blob() {
        a.push("--input-format".into());
        a.push("text".into());
    } else {
        a.push(c.prompt.into());
    }
    a.push("--permission-mode".into());
    a.push(claude_permission_mode(c.mode).into());
    // read-only: narrow the built-in set to the tools that only read.
    if c.mode == PermissionMode::ReadOnly {
        a.push("--tools".into());
        for tool in CLAUDE_READ_ONLY_TOOLS {
            a.push((*tool).into());
        }
    }
    if let Some(m) = c.model {
        a.push("--model".into());
        a.push(m.into());
    }
    // System prompt: for a large one the command layer materializes it to a temp
    // file and sets `system_file`, delivered via `--append-system-prompt-file`
    // (off the argv); otherwise it rides inline `--append-system-prompt`. The file
    // form wins when both are set.
    if let Some(path) = c.system_file {
        a.push("--append-system-prompt-file".into());
        a.push(path.into());
    } else if let Some(s) = c.system {
        a.push("--append-system-prompt".into());
        a.push(s.into());
    }
    if let Some(sid) = c.resume {
        a.push("--resume".into());
        a.push(sid.into());
        // Fork instead of appending: a new session id branches off `sid`, leaving
        // the original (and its cached prefix) untouched. Only meaningful with
        // `--resume`; the command layer only sets `fork` for this verified-capable
        // adapter.
        if c.fork {
            a.push("--fork-session".into());
        }
    }
    a.push("--output-format".into());
    a.push(format_flag(c.output_format).into());
    // Claude Code refuses `-p --output-format stream-json` without `--verbose`
    // ("--print with --output-format=stream-json requires --verbose"). Emit it so
    // the streaming path actually runs — it is what surfaces the Anthropic
    // content-block transcript oneharness normalizes into `events` (the default
    // single-document `json` result carries no transcript). Sourced from the CLI's
    // own error, not guessed.
    if c.output_format == OutputFormat::StreamJson {
        a.push("--verbose".into());
    }
    // Native structured output: `--json-schema <inline>` makes Claude Code return
    // the conforming value in the result document's `structured_output` field
    // (it requires `--output-format json`, already emitted above). Sourced from
    // the headless docs; only set when a schema run selected this adapter.
    if let Some(schema) = c.schema {
        a.push("--json-schema".into());
        a.push(schema.into());
    }
    a
}

/// `codex exec [--sandbox S | --dangerously-bypass-approvals-and-sandbox]
/// [--model M] [--json] <prompt>`, or on a continued turn
/// `codex exec resume [-c sandbox_mode=S | --dangerously-bypass-approvals-and-sandbox]
/// [--model M] [--json] <id> <prompt>`
///
/// The sandbox takes two spellings because `codex exec resume` has no
/// `-s/--sandbox` option (codex exits 2, `unexpected argument '--sandbox'`,
/// before the turn runs): a fresh turn asks with `--sandbox`, a resumed one
/// with the `-c sandbox_mode=` config override it does accept. See
/// [`codex_sandbox`] and [`argv_with_caller_args`] for how the two resolve
/// against a caller's own `sandbox_mode` override.
///
/// Codex exposes no system-prompt flag, so `--system` is prepended to the prompt.
/// The single bypass flag replaces the older `--sandbox danger-full-access -a
/// never`: codex-cli >= 0.135 removed `-a`, and this flag is the supported way to
/// skip every approval prompt and the sandbox for a headless run.
///
/// Continuation is a *subcommand*, not a flag: `codex exec resume <SESSION_ID>
/// <prompt>` replays the stored thread and appends the new turn (linear; Codex's
/// `exec` has no headless fork — `codex fork` is TUI-only, openai/codex#11750). The
/// session handle is the `thread_id` Codex emits under `--json`; oneharness reads
/// it via [`crate::domain::signals::extract_session`].
fn argv_codex(c: &BuildCtx) -> Vec<String> {
    // A control-enabled run drives the turn over the app-server's JSON-RPC
    // protocol instead of `exec`: that is where `turn/interrupt` lives, and it
    // is a second execution model rather than a flag on `exec`. Everything the
    // turn needs (model, cwd, sandbox, approvals, the prompt itself) is
    // negotiated per thread/turn on the wire, so nothing else belongs on the
    // argv. NOT `app-server daemon`: that needs a managed standalone install
    // this project does not use, and self-updates from a fixed path.
    if c.delivery.is_control_stream() {
        let mut a = vec![c.bin.into(), "app-server".into()];
        a.extend(codex_launch_settings(c));
        return a;
    }
    let mut a = vec![c.bin.into(), "exec".into()];
    if c.resume.is_some() {
        a.push("resume".into());
    }
    a.extend(codex_launch_settings(c));
    // The sandbox is the real control surface under `exec` (approval downgrades
    // to `never`). `Default` keeps the exec default (read-only). `Edit` is not a
    // supported mode for codex, so it is never reached. Bypass is one flag both
    // `exec` and `exec resume` accept; a sandbox mode is `--sandbox S` fresh and
    // `-c sandbox_mode=S` resumed, because `exec resume` has no `--sandbox`.
    if c.mode == PermissionMode::Bypass {
        a.push("--dangerously-bypass-approvals-and-sandbox".into());
    } else if let Some(sandbox) = codex_sandbox(c.mode) {
        if c.resume.is_some() {
            a.extend(codex_sandbox_override(sandbox));
        } else {
            a.push("--sandbox".into());
            a.push(sandbox.into());
        }
    }
    if let Some(m) = c.model {
        a.push("--model".into());
        a.push(m.into());
    }
    // Codex's default is its JSON event stream (`--json`): this is what exposes
    // `thread_id`, while `command_execution` items become normalized `events`
    // and the final `agent_message` carries `text`. An explicit Text override
    // stays plain (and is therefore refused with `--session`). Codex has no
    // distinct `stream-json`; `--json` IS its JSONL stream, so both non-text
    // formats map to it. Sourced from `codex exec --help`.
    if c.output_format != OutputFormat::Text {
        a.push("--json".into());
    }
    // The resumed thread's id is the positional that precedes the prompt for
    // `codex exec resume <id> <prompt>` (the `resume` token was pushed above).
    if let Some(sid) = c.resume {
        a.push(sid.into());
    }
    // For a large prompt the command layer selects `PromptDelivery::Stdin` and pipes the
    // (system-prepended) prompt; the `-` sentinel forces `codex exec` to read it
    // from stdin (works with `resume <id> -` too). Otherwise the prompt is the
    // positional.
    if c.delivery.is_stdin_blob() {
        a.push("-".into());
    } else {
        a.push(prompt_with_system(c));
    }
    a
}

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Real Codex cannot be read in the deterministic gate; the live phase `oh_codex_feature_flag_order` (scripts/e2e-lib.sh) checks this spelling's precedence against it.
/// `-c features.plugins=false` unless the run opted into plugins. Codex
/// otherwise downloads its whole remote plugin catalog (~29 MB) on every
/// process start, uncached.
///
/// Spelled as a `-c` override rather than `--disable plugins` so a caller can
/// still turn plugins back on: codex applies `-c` overrides last-wins among
/// themselves and every `--enable`/`--disable` after all of them, with
/// `--disable` beating `--enable` — so after a leading `--disable plugins`
/// nothing a caller adds re-enables them, while after this override a later
/// `-c features.plugins=true` or an `--enable plugins` anywhere does. Real
/// codex's precedence is pinned by `oh_codex_feature_flag_order`
/// (`scripts/e2e-lib.sh`).
fn codex_plugins_off(plugins: bool) -> Vec<String> {
    if plugins {
        Vec::new()
    } else {
        vec!["-c".into(), "features.plugins=false".into()]
    }
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] These are real Codex's config keys, which only an installed Codex can confirm and the deterministic gate does not have; the live codex suite holds them to it: `oh_codex_feature_flag_order` asks real codex every precedence row for `apps` as for `plugins`, and `oh_usage_enforce codex` starts real Codex with `analytics.enabled=false` and `model_catalog_json=` from a cold catalog and fails if the probe or the catalog refresh does (both in scripts/e2e-lib.sh). What each setting saves on the wire is re-measured by scripts/codex-network-breakdown.py and budgeted by the live tier's `codex-launch-network-bytes`, which the `oneharness-live-network-budget` plan node registers.
/// The `-c` settings every Codex launch carries right after its subcommand,
/// ahead of the caller's args so a caller's own later `-c` of the same key wins
/// (Codex applies `-c` overrides last-wins): plugins off unless opted in, apps
/// off unless opted in, analytics off unless opted in, and the shared model
/// catalog when the launch has one.
fn codex_launch_settings(c: &BuildCtx) -> Vec<String> {
    let mut a = codex_plugins_off(c.plugins);
    if !c.apps {
        a.extend(["-c".into(), CODEX_APPS_OFF.into()]);
    }
    if !c.analytics {
        a.extend(["-c".into(), CODEX_ANALYTICS_OFF.into()]);
    }
    if let Some(path) = c.model_catalog {
        a.extend(["-c".into(), codex_model_catalog_setting(path)]);
    }
    a
}

/// The `-c` value that turns Codex's ChatGPT apps (connectors) off: with it on,
/// every session connects to ChatGPT's apps MCP server and downloads its tool
/// list. Spelled as a `-c` override, like the plugins setting, so a caller's
/// later `-c features.apps=true` or an `--enable apps` still turns it on.
pub(crate) const CODEX_APPS_OFF: &str = "features.apps=false";

/// The `-c` value that turns Codex's analytics off: its `exec` reports metrics
/// and product events by default.
pub(crate) const CODEX_ANALYTICS_OFF: &str = "analytics.enabled=false";

/// The `-c` value that points Codex at a static model catalog file, so it never
/// fetches its own. The path is a TOML string, escaped as TOML escapes it,
/// because Codex parses a `-c` value as TOML — a Windows path's backslashes
/// would otherwise read as escapes.
#[must_use]
pub(crate) fn codex_model_catalog_setting(path: &Path) -> String {
    format!(
        "model_catalog_json={}",
        toml::Value::String(path.to_string_lossy().into_owned())
    )
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// The sandbox a codex `exec` turn asks for under `mode`, in codex's own
/// `SandboxMode` spelling; `None` where the mode names no sandbox (`default`
/// keeps exec's own, `bypass` rides its dedicated flag, `edit` is unsupported).
/// `plan` is the read-only sandbox too (its enforcement half); its plan
/// instruction is prepended to the prompt by the command layer.
fn codex_sandbox(mode: PermissionMode) -> Option<&'static str> {
    match mode {
        PermissionMode::ReadOnly | PermissionMode::Plan => Some("read-only"),
        PermissionMode::Auto => Some("workspace-write"),
        PermissionMode::Default | PermissionMode::Edit | PermissionMode::Bypass => None,
    }
}

/// `-c sandbox_mode=<sandbox>`: the sandbox as a config override, the one
/// spelling `codex exec resume` accepts. The value is left bare rather than
/// TOML-quoted: codex parses it as TOML and, failing that, takes the raw text
/// as a string (`CliConfigOverrides::parse_overrides`), so both mean the same
/// value — and a bare one carries no quote for a Windows `.cmd` shim to mangle.
fn codex_sandbox_override(sandbox: &str) -> [String; 2] {
    ["-c".into(), format!("sandbox_mode={sandbox}")]
}

/// Whether a caller's own args set codex's `sandbox_mode` through `-c` /
/// `--config` (either as `-c k=v`, `--config k=v`, `--config=k=v` or `-ck=v`).
/// That codex parses each of these spellings as one last-wins override list is
/// drift-gated against the real CLI by `oh_codex_config_override_order`
/// (`scripts/e2e-lib.sh`).
fn sets_codex_sandbox_mode(extra: &[String]) -> bool {
    let is_sandbox_mode = |kv: &str| {
        kv.split_once('=')
            .is_some_and(|(key, _)| key.trim() == "sandbox_mode")
    };
    extra.iter().enumerate().any(|(i, arg)| {
        if arg == "-c" || arg == "--config" {
            return extra.get(i + 1).is_some_and(|kv| is_sandbox_mode(kv));
        }
        arg.strip_prefix("--config=")
            .or_else(|| arg.strip_prefix("-c").filter(|kv| !kv.is_empty()))
            .is_some_and(is_sandbox_mode)
    })
}

/// An adapter's argv followed by the caller's own args (`[harness.<id>] args`,
/// the reasoning delivery, CLI passthrough), which ride after it verbatim.
///
/// One harness needs more than concatenation. On a fresh codex turn the mode's
/// sandbox rides `--sandbox`, which codex turns into
/// `ConfigOverrides::sandbox_mode`, and that outranks every `-c sandbox_mode=`
/// wherever it sits: `ConfigToml::derive_permission_profile` resolves
/// `sandbox_mode_override.or(self.sandbox_mode)` (codex-rs
/// `config/src/config_toml.rs`, rust-v0.157.1), and `-c` only ever reaches
/// `self.sandbox_mode`. So under a sandbox mode the MODE governs, and a
/// caller's `-c sandbox_mode=` is inert. A resumed turn can only say the mode
/// as `-c sandbox_mode=` too, and between repeated `-c` of one key the LAST one
/// wins — every `-c` is one global, appending list, applied in argv order by
/// `build_cli_overrides_layer` (`config/src/overrides.rs`), each later insert
/// replacing the earlier value. Left as is, the caller's override would win on
/// resume where it lost fresh. So when a caller's args set `sandbox_mode` on a
/// resumed turn under a sandbox mode, the mode's override is stated again
/// after them, and the mode governs a resumed turn exactly as it does a fresh
/// one. Under `default` no mode sandbox exists and the caller's governs both.
pub(crate) fn argv_with_caller_args(
    spec: &HarnessSpec,
    c: &BuildCtx,
    extra: &[String],
) -> Vec<String> {
    let mut argv = (spec.build_argv)(c);
    argv.extend(extra.iter().cloned());
    if spec.id == "codex"
        && c.resume.is_some()
        && !c.delivery.is_control_stream()
        && sets_codex_sandbox_mode(extra)
    {
        if let Some(sandbox) = codex_sandbox(c.mode) {
            argv.extend(codex_sandbox_override(sandbox));
        }
    }
    argv
}

/// `opencode run [--dangerously-skip-permissions] --format json [-m M]
/// [--session SID] <prompt>` (OpenCode continues a session id with `--session`)
///
/// OpenCode's `run` has no system-prompt flag, so `--system` is prepended to the
/// prompt.
fn argv_opencode(c: &BuildCtx) -> Vec<String> {
    let mut a = vec![c.bin.into(), "run".into()];
    // `plan`/`read-only` both select the built-in read-only `plan` agent; bypass
    // auto-approves. Other modes are unsupported and never reach here.
    match c.mode {
        PermissionMode::Bypass => a.push("--dangerously-skip-permissions".into()),
        PermissionMode::Plan | PermissionMode::ReadOnly => {
            a.push("--agent".into());
            a.push("plan".into());
        }
        _ => {}
    }
    a.push("--format".into());
    a.push(format_flag(c.output_format).into());
    if let Some(m) = c.model {
        a.push("-m".into());
        a.push(m.into());
    }
    if let Some(sid) = c.resume {
        a.push("--session".into());
        a.push(sid.into());
        // Branch a new session from `sid` rather than appending in place, so the
        // original's cached prefix can seed independent follow-ups. Only set for
        // this verified-capable adapter, and only alongside `--session`.
        if c.fork {
            a.push("--fork".into());
        }
    }
    // For a large prompt the command layer selects `PromptDelivery::Stdin` and pipes the
    // (system-prepended) prompt; `opencode run` reads stdin when the positional
    // message is omitted, so drop it. Otherwise the prompt is the positional.
    if !c.delivery.is_stdin_blob() {
        a.push(prompt_with_system(c));
    }
    a
}

/// `goose run --with-builtin developer [--system S] [--resume --name <name>]
/// -t <prompt>`
///
/// Goose selects its model from its own config, so `model` is not mapped, and the
/// approval mode is delivered through the `GOOSE_MODE` environment variable (the
/// matching [`ModeSpec::env`]), not the argv — so `c.mode` is intentionally not
/// read here. It does expose a native `--system` flag, so `--system` maps to it
/// rather than being prepended.
fn argv_goose(c: &BuildCtx) -> Vec<String> {
    // Control rides the Agent Client Protocol server (`goose acp`). Its
    // provider/model still come from the environment (GOOSE_PROVIDER /
    // GOOSE_MODEL), exactly as they do for an ordinary `goose run`.
    if c.delivery.is_control_stream() {
        return vec![c.bin.into(), "acp".into()];
    }
    let mut a = vec![
        c.bin.into(),
        "run".into(),
        "--with-builtin".into(),
        "developer".into(),
    ];
    if let Some(s) = c.system {
        a.push("--system".into());
        a.push(s.into());
    }
    // Goose emits no session id to stdout headlessly, so continuation rides a
    // caller-chosen *name*: `--resume --name <name>` resumes that named session
    // (and a fresh `--name <name>` run creates it — create-or-resume). The
    // `--resume` value oneharness forwards is therefore the name. `session_id`
    // stays null for Goose (nothing to extract); the caller owns the handle.
    if let Some(name) = c.resume {
        a.push("--resume".into());
        a.push("--name".into());
        a.push(name.into());
    }
    // For a large prompt the command layer selects `PromptDelivery::Stdin` and pipes it; the
    // `-i -` sentinel (`--instructions -`) makes goose read the prompt from stdin,
    // off the argv. Otherwise the prompt rides `-t`. (`--system` stays inline
    // either way — goose has no per-run system file route.)
    if c.delivery.is_stdin_blob() {
        a.push("-i".into());
        a.push("-".into());
    } else {
        a.push("-t".into());
        a.push(c.prompt.into());
    }
    a
}

/// `qwen [--yolo | --approval-mode <m>] [-m M] [--resume <id>] -p <prompt>` (no
/// system flag, so `--system` is prepended). Bypass uses the dedicated `--yolo`;
/// the other modes use `--approval-mode` (only `plan` and `bypass` run cleanly
/// headless — see the `modes` table — but the flag is mapped for every supported
/// mode). `--resume <id>` continues a prior session by UUID (linear append; no
/// headless fork). The id is the `session_id` Qwen reports under a
/// machine-readable `--output-format`.
fn argv_qwen(c: &BuildCtx) -> Vec<String> {
    let mut a = vec![c.bin.into()];
    match c.mode {
        PermissionMode::Bypass => a.push("--yolo".into()),
        PermissionMode::Plan | PermissionMode::ReadOnly => {
            a.push("--approval-mode".into());
            a.push("plan".into());
        }
        PermissionMode::Default => {
            a.push("--approval-mode".into());
            a.push("default".into());
        }
        PermissionMode::Edit => {
            a.push("--approval-mode".into());
            a.push("auto-edit".into());
        }
        PermissionMode::Auto => {
            a.push("--approval-mode".into());
            a.push("auto".into());
        }
    }
    if let Some(m) = c.model {
        a.push("-m".into());
        a.push(m.into());
    }
    // `--events`/`--stream` and a named `--session` upgrade qwen to
    // `--output-format stream-json` (its NDJSON Anthropic content-block stream),
    // which exposes the session id, normalizes into `events`, and carries the
    // final text. An ordinary run stays plain text. Sourced from `qwen --help`
    // (`-o, --output-format text|json|stream-json`).
    if c.output_format != OutputFormat::Text {
        a.push("--output-format".into());
        a.push(format_flag(c.output_format).into());
    }
    if let Some(sid) = c.resume {
        a.push("--resume".into());
        a.push(sid.into());
    }
    // For a large prompt the command layer selects `PromptDelivery::Stdin` and pipes the
    // (system-prepended) prompt; qwen reads stdin as the prompt when `-p` is
    // omitted, so drop it. Otherwise the prompt rides `-p`.
    if !c.delivery.is_stdin_blob() {
        a.push("-p".into());
        a.push(prompt_with_system(c));
    }
    a
}

/// `crush run -q [--session <id>] [-m M] <prompt>` (`run` is non-interactive; `-q`
/// quiets it; no system flag, so `--system` is prepended). `crush run` already
/// auto-approves the whole session (verified live), so `default` and `bypass` are
/// identical — crush has no per-run permission flag (`--yolo` is rejected on `run`
/// as of v0.80.0), so the mode is not expressed on the argv. `--session <id>`
/// continues a stored session by id (linear append; no headless fork). The id is
/// the `session_id` crush reports under `--format json`.
fn argv_crush(c: &BuildCtx) -> Vec<String> {
    let mut a = vec![c.bin.into(), "run".into(), "-q".into()];
    if let Some(sid) = c.resume {
        a.push("--session".into());
        a.push(sid.into());
    }
    if let Some(m) = c.model {
        a.push("-m".into());
        a.push(m.into());
    }
    // For a large prompt the command layer selects `PromptDelivery::Stdin` and pipes the
    // (system-prepended) prompt; `crush run` reads stdin when the positional is
    // omitted, so drop it. Otherwise the prompt is the positional.
    if !c.delivery.is_stdin_blob() {
        a.push(prompt_with_system(c));
    }
    a
}

/// `copilot -p <prompt> [--allow-all-tools --allow-all-paths --no-ask-user]
/// [--model M] [--resume <id>]` (no system flag, so `--system` is prepended to the
/// prompt; its `--allow-tool`/`--deny-tool` permission flags are not unified —
/// Copilot has no project config file to sync, so rules go via `[harness.copilot]
/// args`). `--resume <id>` continues a session by UUID (linear append; no headless
/// fork). Copilot emits no session id headlessly, and `--resume <uuid>` *creates*
/// the session when the id is new (create-or-resume) — so the caller mints and
/// reuses a UUID; `session_id` stays null (nothing to extract).
fn argv_copilot(c: &BuildCtx) -> Vec<String> {
    // For a large prompt the command layer selects `PromptDelivery::Stdin` and pipes the
    // (system-prepended) prompt; copilot reads stdin as the prompt only when `-p`
    // is ABSENT (a `-p` value makes the pipe be ignored), so drop `-p` entirely.
    // Otherwise the prompt rides `-p`.
    let mut a = if c.delivery.is_control_stream() {
        // Control rides the ACP server, but the approval posture does NOT move
        // to the wire with it: `--acp` is one of copilot's own top-level
        // options, and the permission flags below sit beside it (verified
        // against `copilot --help` 1.0.78 and by handshaking a real
        // `copilot --acp --allow-tool … --no-ask-user`, which answers
        // `initialize` — copilot rejects an option it does not take, so
        // acceptance is the evidence). Carrying them means a controlled turn
        // runs under the mode's OWN mapping rather than a second policy
        // invented for the protocol.
        vec![c.bin.into(), "--acp".into()]
    } else if c.delivery.is_stdin_blob() {
        vec![c.bin.into()]
    } else {
        vec![c.bin.into(), "-p".into(), prompt_with_system(c)]
    };
    // Bypass is the allow-all trio; `plan` is the read-only plan mode;
    // `read-only` is allow-all with `write`/`shell` denied (deny beats allow).
    // Without any, `-p` auto-denies gated tools and continues. Unsupported modes
    // never reach here.
    match c.mode {
        PermissionMode::Bypass => {
            a.push("--allow-all-tools".into());
            a.push("--allow-all-paths".into());
            a.push("--no-ask-user".into());
        }
        PermissionMode::ReadOnly => {
            a.push("--allow-all-tools".into());
            a.push("--allow-all-paths".into());
            a.push("--deny-tool".into());
            a.push("shell".into());
            a.push("--deny-tool".into());
            a.push("write".into());
            a.push("--no-ask-user".into());
        }
        PermissionMode::Edit => {
            // Allow file edits and reads, but not shell — an un-allowed `shell`
            // is auto-denied under `-p`, so edits run and commands are gated.
            a.push("--allow-tool".into());
            a.push("write".into());
            a.push("--allow-tool".into());
            a.push("read".into());
            a.push("--allow-all-paths".into());
            a.push("--no-ask-user".into());
        }
        PermissionMode::Plan => {
            a.push("--mode".into());
            a.push("plan".into());
        }
        _ => {}
    }
    // The mode is the only thing a control launch carries: the model and the
    // session it continues are negotiated per session on the wire, so neither
    // belongs on the argv that starts the server.
    if c.delivery.is_control_stream() {
        return a;
    }
    if let Some(m) = c.model {
        a.push("--model".into());
        a.push(m.into());
    }
    if let Some(sid) = c.resume {
        a.push("--resume".into());
        a.push(sid.into());
    }
    a
}

/// `cursor-agent -p <prompt> [--force|--trust] [--model M] [--resume SID]
/// --output-format stream-json` (Cursor continues a chat id with `--resume`; no
/// system flag, so `--system` is prepended to the prompt)
fn argv_cursor(c: &BuildCtx) -> Vec<String> {
    // `-p` is print mode. For a large prompt the command layer selects `PromptDelivery::Stdin`
    // and pipes the (system-prepended) prompt; cursor reads stdin as the prompt
    // when the positional is omitted (probe-verified 2026-07-11 via
    // scripts/explore-cursor-stdin.sh — piped stdin with `-p` and no positional
    // round-trips; the `-` sentinel does NOT). Otherwise the prompt is the positional.
    let mut a = vec![c.bin.into(), "-p".into()];
    if !c.delivery.is_stdin_blob() {
        a.push(prompt_with_system(c));
    }
    // `--force` is bypass (it also implies trust). Otherwise a headless run still
    // needs `--trust` — Cursor refuses to run an untrusted workspace ("Workspace
    // Trust Required", observed live) — while leaving the permission system
    // active. `plan` additionally selects the read-only `--mode plan`.
    if c.mode.is_bypass() {
        a.push("--force".into());
    } else {
        a.push("--trust".into());
        match c.mode {
            PermissionMode::Plan => {
                a.push("--mode".into());
                a.push("plan".into());
            }
            PermissionMode::ReadOnly => {
                a.push("--mode".into());
                a.push("ask".into());
            }
            _ => {}
        }
    }
    if let Some(m) = c.model {
        a.push("--model".into());
        a.push(m.into());
    }
    if let Some(sid) = c.resume {
        a.push("--resume".into());
        a.push(sid.into());
    }
    a.push("--output-format".into());
    a.push(format_flag(c.output_format).into());
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_identity_parses_only_a_well_formed_id() {
        // Both legitimate spellings: a harness with no configured variant is one
        // unqualified identity; a configured one carries its variant.
        for (text, base, variant) in [
            ("codex", "codex", None),
            ("claude-code", "claude-code", None),
            ("claude-code:alternate", "claude-code", Some("alternate")),
            ("claude-code:alt_2", "claude-code", Some("alt_2")),
        ] {
            let id: HarnessIdentity = text.parse().unwrap_or_else(|_| panic!("{text}"));
            assert_eq!(id.as_str(), text);
            assert_eq!(id.base(), base);
            assert_eq!(id.variant(), variant);
            assert_eq!(id.spec().id, base);
        }

        // Text that names nothing runnable. A session record carrying any of
        // these could never be matched against a candidate, so it must not be
        // constructible — the store reads it back as no record instead.
        for text in [
            "",                   // no harness at all
            "nope",               // not in the registry
            "claude-code:",       // a separator with no identity after it
            ":alternate",         // a variant with no harness
            "claude-code:a:b",    // a second separator would re-split differently
            "claude-code:-lead",  // variant must not lead with a separator char
            "claude-code:has sp", // whitespace is not a variant character
            "claude-code:UPPER!", // punctuation is not a variant character
            "Claude-Code",        // ids are lowercase and exact
            " codex",             // no surrounding whitespace
        ] {
            assert!(
                text.parse::<HarnessIdentity>().is_err(),
                "`{text}` must not parse as an identity"
            );
        }
    }

    #[test]
    fn a_harness_identity_round_trips_through_json() {
        // The session record's wire shape is the plain composed string, so the
        // store stays readable by eye and by an older reader.
        let id: HarnessIdentity = "claude-code:alternate".parse().unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"claude-code:alternate\"");
        assert_eq!(serde_json::from_str::<HarnessIdentity>(&json).unwrap(), id);
        // A wire value that names nothing runnable is refused, not silently kept.
        assert!(serde_json::from_str::<HarnessIdentity>("\"gone:x\"").is_err());
    }

    fn ctx<'a>(bin: &'a str, model: Option<&'a str>, mode: PermissionMode) -> BuildCtx<'a> {
        ctx_fmt(bin, model, mode, OutputFormat::Json)
    }

    fn ctx_fmt<'a>(
        bin: &'a str,
        model: Option<&'a str>,
        mode: PermissionMode,
        output_format: OutputFormat,
    ) -> BuildCtx<'a> {
        BuildCtx {
            bin,
            prompt: "hi",
            model,
            system: None,
            resume: None,
            fork: false,
            mode,
            output_format,
            schema: None,
            system_file: None,
            delivery: PromptDelivery::Argv,
            plugins: false,
            apps: false,
            analytics: false,
            model_catalog: None,
        }
    }

    #[test]
    fn registry_ids_are_unique_and_nonempty() {
        let mut seen = std::collections::HashSet::new();
        for h in all() {
            assert!(!h.id.is_empty());
            assert!(!h.default_bin.is_empty());
            assert!(seen.insert(h.id), "duplicate id {}", h.id);
        }
        assert_eq!(all().len(), 8);
    }

    #[test]
    fn reasoning_delivery_renders_verbatim() {
        // Flag form: `<flag> <value>`. Value passed through untouched.
        assert_eq!(
            ReasoningDelivery::Flag("--effort").args("high"),
            vec!["--effort".to_string(), "high".to_string()]
        );
        // ConfigKv form: `-c key=value`, bare value (no quoting/normalization).
        assert_eq!(
            ReasoningDelivery::ConfigKv("model_reasoning_effort").args("xhigh"),
            vec!["-c".to_string(), "model_reasoning_effort=xhigh".to_string()]
        );
        // ModelSuffix appends no args; it decorates the model id with a `-<tier>`
        // suffix instead (Cursor: `claude-opus-4-8` + `high` → `-high`).
        let suffix = ReasoningDelivery::ModelSuffix;
        assert!(suffix.args("high").is_empty());
        assert_eq!(suffix.model_suffix("high").as_deref(), Some("-high"));
        // The append-style deliveries never decorate the model.
        assert_eq!(
            ReasoningDelivery::Flag("--effort").model_suffix("high"),
            None
        );
    }

    #[test]
    fn only_argv_capable_harnesses_declare_reasoning() {
        // Exactly the harnesses with a doc-sourced headless reasoning surface: a
        // flag (claude-code/copilot), a `-c` override (codex), or a model-id
        // suffix (cursor). The rest express effort only via config (opencode/qwen/
        // crush — sync-path follow-up) or not at all (goose), and are `None` so
        // the command layer refuses loudly.
        let with: Vec<&str> = all()
            .iter()
            .filter(|h| h.reasoning.is_some())
            .map(|h| h.id)
            .collect();
        assert_eq!(with, vec!["claude-code", "codex", "copilot", "cursor"]);
    }

    /// Pin each harness's mock input-rewrite capability: the live-verified
    /// shapes for the five that honor one (oh_mock_enforce + the explore-hooks
    /// probe, 2026-07-06), and — as deliberately as the presences — the
    /// absences: Goose's protocol has no rewrite verdict; Copilot's hooks
    /// never fired headlessly (probe: zero events); and Qwen's documented
    /// `updatedInput` was live-REFUTED (verdict emitted, original command
    /// still ran — see its registry comment), so it stays absent despite its
    /// docs. A rewrite also requires an installable hook and a deny shape
    /// (the mock responder's other verb), so those must accompany it.
    #[test]
    fn registry_mock_rewrite_capability_is_pinned() {
        let shape = |id: &str| by_id(id).unwrap().mock_rewrite;
        assert_eq!(shape("claude-code"), Some(RewriteShape::ClaudeNested));
        assert_eq!(shape("codex"), Some(RewriteShape::ClaudeNested));
        assert_eq!(shape("crush"), Some(RewriteShape::CrushFlat));
        assert_eq!(shape("opencode"), Some(RewriteShape::OpencodeShim));
        assert_eq!(shape("cursor"), Some(RewriteShape::CursorPermission));
        for id in ["goose", "qwen", "copilot"] {
            assert_eq!(shape(id), None, "{id} must stay absent until verified");
        }
        // The one-shot delivery (`run --mock-rules`): claude rides the argv,
        // qwen (user-scope-only hooks) and copilot (hooks never fire) are
        // refused, codex auto-appends its probe-proven opt-in flags, the rest
        // are plain project installs.
        let delivery = |id: &str| by_id(id).unwrap().mock_delivery;
        assert_eq!(
            delivery("claude-code"),
            Some(MockDelivery::SettingsFlag { flag: "--settings" })
        );
        assert_eq!(
            delivery("codex"),
            Some(MockDelivery::ProjectHooks {
                extra_args: &[
                    "-c",
                    "features.hooks=true",
                    "--dangerously-bypass-hook-trust",
                ],
            })
        );
        for id in ["opencode", "goose", "crush", "cursor"] {
            assert_eq!(
                delivery(id),
                Some(MockDelivery::ProjectHooks { extra_args: &[] }),
                "{id}"
            );
        }
        for id in ["qwen", "copilot"] {
            assert_eq!(delivery(id), None, "{id} must be refused loudly");
        }
        for h in all() {
            if h.mock_rewrite.is_some() {
                assert!(
                    h.hooks.is_some() && h.gate_deny.is_some(),
                    "{}: a rewrite shape needs an installable hook and a deny shape",
                    h.id
                );
            }
        }
    }

    /// The OpenCode shim must keep both verdict paths: throw-to-block on a
    /// deny, and merge `updated_input` into the tool's mutable args (the
    /// rewrite `oneharness mock` emits for `RewriteShape::OpencodeShim`).
    #[test]
    fn opencode_shim_applies_updated_input_and_still_blocks_on_deny() {
        assert!(OPENCODE_PLUGIN_JS.contains(r#"decision.decision === "deny""#));
        assert!(OPENCODE_PLUGIN_JS.contains("Object.assign(output.args, decision.updated_input)"));
        // The deny check runs first, so a malformed reply carrying both never
        // rewrites a call that should have been blocked.
        let deny = OPENCODE_PLUGIN_JS
            .find(r#"decision.decision === "deny""#)
            .unwrap();
        let rewrite = OPENCODE_PLUGIN_JS.find("decision.updated_input").unwrap();
        assert!(deny < rewrite, "deny must be evaluated before the rewrite");
    }

    #[test]
    fn claude_argv_bypass_on() {
        let spec = by_id("claude-code").unwrap();
        let argv = (spec.build_argv)(&ctx("claude", None, PermissionMode::Bypass));
        assert_eq!(
            argv,
            vec![
                "claude",
                "-p",
                "hi",
                "--permission-mode",
                "bypassPermissions",
                "--output-format",
                "json"
            ]
        );
    }

    #[test]
    fn claude_argv_default_mode_maps_to_dont_ask() {
        // The normalized `default` ask flow maps to `dontAsk` (deny un-allowed
        // tools and continue), not `--permission-mode default` (which aborts the
        // `-p` run on the first un-allowed tool) — so it never hangs/aborts.
        let spec = by_id("claude-code").unwrap();
        let argv = (spec.build_argv)(&ctx("claude", Some("haiku"), PermissionMode::Default));
        assert_eq!(
            argv,
            vec![
                "claude",
                "-p",
                "hi",
                "--permission-mode",
                "dontAsk",
                "--model",
                "haiku",
                "--output-format",
                "json"
            ]
        );
    }

    #[test]
    fn claude_maps_each_mode_to_its_permission_mode_token() {
        let spec = by_id("claude-code").unwrap();
        for (mode, token) in [
            (PermissionMode::Plan, "plan"),
            (PermissionMode::ReadOnly, "bypassPermissions"),
            (PermissionMode::Default, "dontAsk"),
            (PermissionMode::Edit, "acceptEdits"),
            (PermissionMode::Auto, "auto"),
            (PermissionMode::Bypass, "bypassPermissions"),
        ] {
            let argv = (spec.build_argv)(&ctx("claude", None, mode));
            assert!(
                argv.windows(2).any(|w| w == ["--permission-mode", token]),
                "mode {mode:?} should emit {token}: {argv:?}"
            );
        }
        // read-only additionally narrows the built-in set to the read-only
        // tools, and only read-only does — plan/bypass leave the whole set
        // available to the permission system.
        let ro = (spec.build_argv)(&ctx("claude", None, PermissionMode::ReadOnly));
        let tools: Vec<&str> = ro
            .iter()
            .skip_while(|a| *a != "--tools")
            .skip(1)
            .take(CLAUDE_READ_ONLY_TOOLS.len())
            .map(String::as_str)
            .collect();
        assert_eq!(
            tools, CLAUDE_READ_ONLY_TOOLS,
            "read-only should permit exactly the read-only tools: {ro:?}"
        );
        assert!(
            !(spec.build_argv)(&ctx("claude", None, PermissionMode::Plan))
                .iter()
                .any(|t| t == "--tools"),
            "plan should not narrow the tool set"
        );
    }

    #[test]
    fn mode_native_flags_per_harness() {
        // Pin the mode→flag mapping for the harnesses that express it on the argv
        // (Goose carries it in the environment, asserted via `modes` env below).
        let cases: &[(&str, PermissionMode, &[&str])] = &[
            // read-only: enforced where possible (codex sandbox, copilot/cursor
            // native), coinciding with plan where it's the only mechanism.
            (
                "codex",
                PermissionMode::ReadOnly,
                &["--sandbox", "read-only"],
            ),
            (
                "codex",
                PermissionMode::Auto,
                &["--sandbox", "workspace-write"],
            ),
            ("opencode", PermissionMode::Plan, &["--agent", "plan"]),
            ("opencode", PermissionMode::ReadOnly, &["--agent", "plan"]),
            ("qwen", PermissionMode::Plan, &["--approval-mode", "plan"]),
            (
                "qwen",
                PermissionMode::ReadOnly,
                &["--approval-mode", "plan"],
            ),
            (
                "qwen",
                PermissionMode::Edit,
                &["--approval-mode", "auto-edit"],
            ),
            ("qwen", PermissionMode::Auto, &["--approval-mode", "auto"]),
            ("copilot", PermissionMode::Plan, &["--mode", "plan"]),
            (
                "copilot",
                PermissionMode::ReadOnly,
                &["--deny-tool", "shell"],
            ),
            (
                "copilot",
                PermissionMode::ReadOnly,
                &["--deny-tool", "write"],
            ),
            ("copilot", PermissionMode::Edit, &["--allow-tool", "write"]),
            ("cursor", PermissionMode::Plan, &["--mode", "plan"]),
            ("cursor", PermissionMode::ReadOnly, &["--mode", "ask"]),
        ];
        for (id, mode, want) in cases {
            let spec = by_id(id).unwrap();
            let argv = (spec.build_argv)(&ctx(spec.default_bin, None, *mode));
            assert!(
                argv.windows(want.len()).any(|w| w == *want),
                "harness {id} mode {mode:?} should emit {want:?}; got {argv:?}"
            );
        }
        // copilot `edit` must NOT blanket-allow tools, or shell wouldn't be
        // gated — it allows only write/read, leaving shell to be auto-denied.
        let copilot_edit =
            (by_id("copilot").unwrap().build_argv)(&ctx("copilot", None, PermissionMode::Edit));
        assert!(
            !copilot_edit.iter().any(|t| t == "--allow-all-tools"),
            "copilot edit must gate shell, not allow-all: {copilot_edit:?}"
        );
        // Codex `plan` is synthesized: read-only sandbox (enforcement) + a plan
        // instruction prepended to the prompt (no native exec plan mode).
        let codex_plan = by_id("codex").unwrap().mode(PermissionMode::Plan);
        assert!(
            codex_plan.is_some(),
            "codex should support synthesized plan"
        );
        assert!(
            codex_plan.unwrap().instruction.is_some(),
            "codex plan must carry a plan instruction"
        );
        let codex_plan_argv =
            (by_id("codex").unwrap().build_argv)(&ctx("codex", None, PermissionMode::Plan));
        assert!(
            codex_plan_argv
                .windows(2)
                .any(|w| w == ["--sandbox", "read-only"]),
            "codex plan must enforce read-only: {codex_plan_argv:?}"
        );
        // Goose rejects plan (no plan workflow, and no read-only to enforce it).
        assert!(by_id("goose").unwrap().mode(PermissionMode::Plan).is_none());
        assert!(by_id("goose")
            .unwrap()
            .mode(PermissionMode::ReadOnly)
            .is_none());
        // Crush supports neither plan nor read-only, and has no per-run
        // permission flag (`crush run` auto-approves), so bypass == default.
        let crush = by_id("crush").unwrap();
        assert!(crush.mode(PermissionMode::Plan).is_none());
        assert!(crush.mode(PermissionMode::ReadOnly).is_none());
        let bypass = (crush.build_argv)(&ctx("crush", None, PermissionMode::Bypass));
        let default = (crush.build_argv)(&ctx("crush", None, PermissionMode::Default));
        assert_eq!(bypass, default, "crush has no per-run mode flag");
        assert!(!bypass.iter().any(|t| t == "--yolo"), "{bypass:?}");
    }

    #[test]
    fn every_harness_supports_bypass_and_default_and_goose_carries_mode_env() {
        for h in all() {
            assert!(
                h.mode(PermissionMode::Bypass).is_some(),
                "harness {} must support bypass (the headless default)",
                h.id
            );
            assert!(
                h.mode(PermissionMode::Default).is_some(),
                "harness {} must support the default ask flow",
                h.id
            );
        }
        // Goose delivers the mode via GOOSE_MODE, so every supported mode carries
        // an env mapping (and no other harness does).
        let goose = by_id("goose").unwrap();
        assert_eq!(
            goose.mode(PermissionMode::Bypass).unwrap().env,
            &[("GOOSE_MODE", "auto")]
        );
        assert_eq!(
            goose.mode(PermissionMode::Default).unwrap().env,
            &[("GOOSE_MODE", "approve")]
        );
        // OpenCode delivers `edit` through the inline-config env var (no argv
        // flag exists for its per-tool permission map).
        assert_eq!(
            by_id("opencode")
                .unwrap()
                .mode(PermissionMode::Edit)
                .unwrap()
                .env,
            &[(
                "OPENCODE_CONFIG_CONTENT",
                r#"{"permission":{"edit":"allow","bash":"deny"}}"#
            )]
        );
        // Every other (harness, mode) expresses itself on the argv, not env.
        for h in all() {
            for m in h.modes {
                let env_ok = h.id == "goose"
                    || (h.id == "opencode" && m.mode == PermissionMode::Edit)
                    || m.env.is_empty();
                assert!(
                    env_ok,
                    "harness {} mode {:?} unexpectedly carries env",
                    h.id, m.mode
                );
            }
        }
    }

    #[test]
    fn session_capability_is_defined_by_session_bearing_formats() {
        for spec in all() {
            assert_eq!(
                spec.session_capable(),
                spec.session_format().is_some(),
                "{} session capability must have an automatic format",
                spec.id
            );
            if spec.session_capable() {
                assert!(
                    spec.supports_resume,
                    "{} session capability must imply resume support",
                    spec.id
                );
                assert!(
                    spec.format_carries_session(spec.session_format().unwrap()),
                    "{} preferred session format must carry its id",
                    spec.id
                );
                let event_format = spec.events_format.unwrap_or(spec.output_format);
                assert!(
                    spec.format_carries_session(event_format),
                    "{} --events format must remain compatible with --session",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn codex_explicit_text_uses_exec_without_json() {
        // The adapter still honors an explicit Text override when no named
        // session needs capturing, so this lower-level argv has no `--json`.
        let spec = by_id("codex").unwrap();
        let argv = (spec.build_argv)(&ctx_fmt(
            "codex",
            None,
            PermissionMode::Bypass,
            OutputFormat::Text,
        ));
        assert_eq!(
            argv,
            vec![
                "codex",
                "exec",
                "-c",
                "features.plugins=false",
                "-c",
                "features.apps=false",
                "-c",
                "analytics.enabled=false",
                "--dangerously-bypass-approvals-and-sandbox",
                "hi"
            ]
        );
    }

    #[test]
    fn codex_default_format_adds_json_flag() {
        // Codex defaults to the session-bearing JSONL stream, so `--events`
        // needs no separate upgrade and plain runs still carry `--json`.
        let spec = by_id("codex").unwrap();
        assert_eq!(spec.output_format, OutputFormat::Json);
        assert_eq!(spec.events_format, None);
        let argv = (spec.build_argv)(&ctx_fmt(
            "codex",
            None,
            PermissionMode::Bypass,
            spec.output_format,
        ));
        assert!(argv.iter().any(|t| t == "--json"), "{argv:?}");
    }

    #[test]
    fn qwen_events_format_adds_stream_json_flag() {
        // Qwen's events_format is stream-json → `--output-format stream-json`; the
        // default (text) emits no format flag.
        let spec = by_id("qwen").unwrap();
        assert_eq!(spec.events_format, Some(OutputFormat::StreamJson));
        let stream = (spec.build_argv)(&ctx_fmt(
            "qwen",
            None,
            PermissionMode::Bypass,
            OutputFormat::StreamJson,
        ));
        assert!(
            stream
                .windows(2)
                .any(|w| w == ["--output-format", "stream-json"]),
            "{stream:?}"
        );
        let text = (spec.build_argv)(&ctx_fmt(
            "qwen",
            None,
            PermissionMode::Bypass,
            OutputFormat::Text,
        ));
        assert!(
            !text.iter().any(|t| t == "--output-format"),
            "default text must not add a format flag: {text:?}"
        );
    }

    #[test]
    fn telemetry_capabilities_name_only_verified_boundary_traces() {
        let supported = all()
            .iter()
            .filter_map(|spec| spec.telemetry.map(|telemetry| (spec.id, telemetry)))
            .collect::<Vec<_>>();
        assert_eq!(
            supported.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            ["codex", "opencode"]
        );
        for (id, telemetry) in supported {
            match id {
                "codex" => {
                    assert_eq!(telemetry.format, OutputFormat::Json);
                    assert_eq!(telemetry.trace, TelemetryTrace::CodexJson);
                }
                "opencode" => {
                    assert_eq!(telemetry.format, OutputFormat::Json);
                    assert_eq!(telemetry.trace, TelemetryTrace::OpenCodeJson);
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn goose_ignores_model_and_bypass() {
        let spec = by_id("goose").unwrap();
        let with = (spec.build_argv)(&ctx("goose", Some("gpt"), PermissionMode::Bypass));
        let without = (spec.build_argv)(&ctx("goose", None, PermissionMode::Default));
        assert_eq!(with, without);
        assert_eq!(
            with,
            vec!["goose", "run", "--with-builtin", "developer", "-t", "hi"]
        );
    }

    #[test]
    fn output_format_override_changes_the_emitted_flag() {
        let spec = by_id("claude-code").unwrap();
        let argv = (spec.build_argv)(&ctx_fmt(
            "claude",
            None,
            PermissionMode::Bypass,
            OutputFormat::StreamJson,
        ));
        assert!(
            argv.windows(2)
                .any(|w| w == ["--output-format", "stream-json"]),
            "{argv:?}"
        );
        // opencode spells its flag `--format`.
        let oc = by_id("opencode").unwrap();
        let argv = (oc.build_argv)(&ctx_fmt(
            "opencode",
            None,
            PermissionMode::Bypass,
            OutputFormat::Text,
        ));
        assert!(
            argv.windows(2).any(|w| w == ["--format", "text"]),
            "{argv:?}"
        );
    }

    #[test]
    fn claude_maps_system_to_append_system_prompt() {
        let spec = by_id("claude-code").unwrap();
        let ctx = BuildCtx {
            bin: "claude",
            prompt: "hi",
            model: None,
            system: Some("be terse"),
            resume: None,
            fork: false,
            mode: PermissionMode::Bypass,
            output_format: OutputFormat::Json,
            schema: None,
            system_file: None,
            delivery: PromptDelivery::Argv,
            plugins: false,
            apps: false,
            analytics: false,
            model_catalog: None,
        };
        let argv = (spec.build_argv)(&ctx);
        assert!(
            argv.windows(2)
                .any(|w| w == ["--append-system-prompt", "be terse"]),
            "{argv:?}"
        );
    }

    #[test]
    fn prompt_with_system_prefixes_only_when_present() {
        let spec = by_id("codex").unwrap();
        let none = BuildCtx {
            system: None,
            ..base_ctx(spec)
        };
        assert_eq!(prompt_with_system(&none), "hi");
        let some = BuildCtx {
            system: Some("rules"),
            ..base_ctx(spec)
        };
        assert_eq!(prompt_with_system(&some), "rules\n\nhi");
        // A blank system prompt is a no-op (no stray leading newlines).
        let empty = BuildCtx {
            system: Some(""),
            ..base_ctx(spec)
        };
        assert_eq!(prompt_with_system(&empty), "hi");
    }

    #[test]
    fn goose_maps_system_to_its_native_flag() {
        let spec = by_id("goose").unwrap();
        let argv = (spec.build_argv)(&BuildCtx {
            system: Some("be terse"),
            ..base_ctx(spec)
        });
        assert!(
            argv.windows(2).any(|w| w == ["--system", "be terse"]),
            "{argv:?}"
        );
        // The prompt is delivered via -t and left untouched (not prepended).
        assert!(argv.windows(2).any(|w| w == ["-t", "hi"]), "{argv:?}");
    }

    #[test]
    fn harnesses_without_a_system_flag_prepend_it_to_the_prompt() {
        // Codex/OpenCode/Qwen/Crush/Copilot/Cursor expose no system-prompt flag,
        // so `--system` must be prepended to the prompt — never silently dropped.
        for id in ["codex", "opencode", "qwen", "crush", "copilot", "cursor"] {
            let spec = by_id(id).unwrap();
            let argv = (spec.build_argv)(&BuildCtx {
                system: Some("be terse"),
                ..base_ctx(spec)
            });
            assert!(
                argv.iter().any(|t| t == "be terse\n\nhi"),
                "harness {id} should carry the prepended prompt; got {argv:?}"
            );
            // The un-prefixed prompt must not also be sent on its own.
            assert!(
                !argv.iter().any(|t| t == "hi"),
                "harness {id} should not also send the bare prompt; got {argv:?}"
            );
        }
    }

    fn base_ctx(spec: &'static HarnessSpec) -> BuildCtx<'static> {
        BuildCtx {
            bin: spec.default_bin,
            prompt: "hi",
            model: None,
            system: None,
            resume: None,
            fork: false,
            mode: PermissionMode::Bypass,
            output_format: spec.output_format,
            schema: None,
            system_file: None,
            delivery: PromptDelivery::Argv,
            plugins: false,
            apps: false,
            analytics: false,
            model_catalog: None,
        }
    }

    #[test]
    fn every_declared_control_mechanism_has_the_process_it_needs() {
        // The pairing rule for the two capability fields, checked across the
        // whole registry so a harness added later cannot declare a server-backed
        // mechanism and then fail to start a server at run time.
        for spec in all() {
            match (spec.control, spec.server.is_some()) {
                (Some(shape), has_server) => assert_eq!(
                    shape.needs_pooled_server(),
                    has_server,
                    "`{}` declares `{}` but {} a server",
                    spec.id,
                    shape.as_str(),
                    if has_server {
                        "also declares"
                    } else {
                        "declares no"
                    }
                ),
                (None, has_server) => assert!(
                    !has_server,
                    "`{}` declares a server but no control mechanism to use it",
                    spec.id
                ),
            }
        }
    }

    #[test]
    fn a_turn_driving_control_run_launches_that_harness_protocol_server() {
        // A control run on a turn-driving mechanism spawns the harness's OWN
        // protocol server instead of its headless run, so the launch argv is the
        // whole command: the prompt and the model are negotiated on the wire and
        // must not appear on it. Pinned per harness (each sourced from that CLI
        // and re-proven by `scripts/explore-control.sh`), so a drifted subcommand
        // fails here rather than as a controlled run that never starts a turn.
        //
        // The APPROVAL MODE is the exception, and only where the CLI takes it
        // beside the protocol switch: copilot's permission flags are top-level
        // options that sit next to `--acp`, so a controlled turn runs under the
        // mode's own mapping instead of a posture invented for the protocol
        // (`domain::control`'s `control_mode_parity` holds the two equal). Codex
        // and goose take theirs elsewhere — the app-server negotiates the
        // sandbox per turn, and goose reads `GOOSE_MODE` from the environment.
        let launches = [
            (
                "codex",
                &[
                    "app-server",
                    "-c",
                    "features.plugins=false",
                    "-c",
                    "features.apps=false",
                    "-c",
                    "analytics.enabled=false",
                ][..],
            ),
            ("goose", &["acp"][..]),
            (
                "copilot",
                &[
                    "--acp",
                    "--allow-all-tools",
                    "--allow-all-paths",
                    "--no-ask-user",
                ][..],
            ),
        ];
        for (id, tail) in launches {
            let spec = by_id(id).unwrap();
            assert!(
                spec.control.is_some_and(ControlShape::drives_turn),
                "`{id}` should drive its turn over its own protocol"
            );
            let argv = (spec.build_argv)(&BuildCtx {
                delivery: PromptDelivery::ControlStream,
                model: Some("some-model"),
                system: Some("be terse"),
                ..base_ctx(spec)
            });
            let expected: Vec<String> = std::iter::once(spec.default_bin)
                .chain(tail.iter().copied())
                .map(str::to_string)
                .collect();
            assert_eq!(argv, expected, "`{id}` control launch argv");
        }
        // The other kind of turn-driving harness never spawns its CLI at all:
        // its turn goes to a pooled server, so the launch to pin is the
        // `ServerSpec` the pool starts, plus how the chosen address reaches it.
        let servers = [
            ("opencode", &["serve"][..], &["--port", "{address}"][..]),
            ("crush", &["server"][..], &["-H", "unix://{address}"][..]),
        ];
        for (id, launch, address_args) in servers {
            let spec = by_id(id).unwrap();
            let server = spec
                .server
                .unwrap_or_else(|| panic!("`{id}` should declare the server its control needs"));
            assert_eq!(server.launch, launch, "`{id}` server launch");
            assert_eq!(server.address_args, address_args, "`{id}` address args");
            // Per-turn settings are negotiated on the wire, so nothing about a
            // dispatch may key the pool: a widened key starts one heavyweight
            // server per dispatch instead of sharing one.
            assert!(
                server.key_env.is_empty(),
                "`{id}` keys its pool on {:?}",
                server.key_env
            );
        }
        // A harness whose turn is driven but whose launch is unpinned would ship
        // a guessed subcommand, so the lists above must cover the registry —
        // each harness in exactly the one its mechanism calls for.
        for spec in all() {
            let Some(shape) = spec.control.filter(|shape| shape.drives_turn()) else {
                continue;
            };
            let pinned = if shape.needs_pooled_server() {
                servers.iter().any(|(id, _, _)| *id == spec.id)
            } else {
                launches.iter().any(|(id, _)| *id == spec.id)
            };
            assert!(
                pinned,
                "`{}` drives its own turn but its launch is unpinned",
                spec.id
            );
        }
    }

    #[test]
    fn large_prompt_rides_stdin_off_the_argv_per_harness() {
        // With `PromptDelivery::Stdin` selected (the command layer's large-prompt decision), a
        // stdin-capable adapter must omit the positional prompt — so the prompt
        // never touches the argv (`E2BIG`) — and add whatever stdin-selecting
        // flags its CLI needs. Sourced per-adapter (see each `build_argv` comment).
        let stdin_ctx = |spec: &'static HarnessSpec| BuildCtx {
            delivery: PromptDelivery::Stdin,
            // A system prompt too, to prove it is never inlined either.
            system: Some("be terse"),
            ..base_ctx(spec)
        };
        let has = |a: &[String], t: &str| a.iter().any(|x| x == t);
        let pair = |a: &[String], x: &str, y: &str| a.windows(2).any(|w| w == [x, y]);

        // claude: keep `-p` (print mode), add `--input-format text`, drop the
        // positional. (System rides its own file flag, tested separately.)
        let spec = by_id("claude-code").unwrap();
        let a = (spec.build_argv)(&stdin_ctx(spec));
        assert!(pair(&a, "--input-format", "text"), "{a:?}");
        assert!(has(&a, "-p"), "{a:?}");
        assert!(!has(&a, "hi"), "claude drops the positional prompt: {a:?}");

        // codex: the `-` sentinel forces stdin, replacing the positional.
        let spec = by_id("codex").unwrap();
        let a = (spec.build_argv)(&stdin_ctx(spec));
        assert!(has(&a, "-"), "codex uses the stdin sentinel: {a:?}");
        assert!(!has(&a, "be terse\n\nhi") && !has(&a, "hi"), "{a:?}");

        // goose: `-i -` replaces `-t <prompt>` (system stays on its inline flag).
        let spec = by_id("goose").unwrap();
        let a = (spec.build_argv)(&stdin_ctx(spec));
        assert!(pair(&a, "-i", "-"), "{a:?}");
        assert!(!pair(&a, "-t", "hi"), "goose drops -t <prompt>: {a:?}");

        // opencode / qwen / crush / copilot / cursor: the positional
        // (system-prepended) is omitted entirely — the prompt rides stdin.
        for id in ["opencode", "qwen", "crush", "copilot", "cursor"] {
            let spec = by_id(id).unwrap();
            let a = (spec.build_argv)(&stdin_ctx(spec));
            assert!(
                !has(&a, "hi") && !has(&a, "be terse\n\nhi"),
                "{id} drops the positional prompt: {a:?}"
            );
        }
        // qwen and copilot must also drop `-p` (else the pipe is ignored / a
        // positional is required). cursor KEEPS `-p` (its stdin form uses it —
        // probe-verified).
        for id in ["qwen", "copilot"] {
            let spec = by_id(id).unwrap();
            let a = (spec.build_argv)(&stdin_ctx(spec));
            assert!(!has(&a, "-p"), "{id} drops -p under stdin: {a:?}");
        }
        let spec = by_id("cursor").unwrap();
        assert!(
            has(&(spec.build_argv)(&stdin_ctx(spec)), "-p"),
            "cursor keeps -p under stdin"
        );
    }

    #[test]
    fn large_system_rides_the_file_flag_for_claude() {
        // A large system prompt on claude-code is delivered via
        // `--append-system-prompt-file <tempfile>` (the command layer materializes
        // it), never inline `--append-system-prompt` — off the argv.
        let spec = by_id("claude-code").unwrap();
        let a = (spec.build_argv)(&BuildCtx {
            system: Some("be terse"),
            system_file: Some("/tmp/oneharness-sys.txt"),
            ..base_ctx(spec)
        });
        assert!(
            a.windows(2)
                .any(|w| w == ["--append-system-prompt-file", "/tmp/oneharness-sys.txt"]),
            "{a:?}"
        );
        // The inline flag and value are absent when the file is used.
        assert!(!a.iter().any(|t| t == "--append-system-prompt"), "{a:?}");
        assert!(!a.iter().any(|t| t == "be terse"), "{a:?}");
    }

    #[test]
    fn prompt_with_system_text_matches_the_inline_prepend() {
        // The stdin payload assembly must mirror `prompt_with_system` exactly, so
        // stdin delivery is byte-identical to the inline path it replaces.
        assert_eq!(prompt_with_system_text(Some("sys"), "usr"), "sys\n\nusr");
        assert_eq!(prompt_with_system_text(Some(""), "usr"), "usr");
        assert_eq!(prompt_with_system_text(None, "usr"), "usr");
    }

    #[test]
    fn claude_native_schema_appends_json_schema_flag() {
        // The native structured-output path: claude-code carries the inline
        // schema on `--json-schema`, after `--output-format json`. Only this
        // adapter declares native support today.
        let spec = by_id("claude-code").unwrap();
        assert_eq!(spec.native_schema, Some(NativeSchema::ClaudeJsonSchema));
        let argv = (spec.build_argv)(&BuildCtx {
            schema: Some(r#"{"type":"object"}"#),
            ..base_ctx(spec)
        });
        assert!(
            argv.windows(2)
                .any(|w| w == ["--json-schema", r#"{"type":"object"}"#]),
            "{argv:?}"
        );
        assert!(
            argv.windows(2).any(|w| w == ["--output-format", "json"]),
            "native schema requires json output: {argv:?}"
        );
        // Without a schema the flag is absent.
        let argv = (spec.build_argv)(&base_ctx(spec));
        assert!(!argv.iter().any(|t| t == "--json-schema"), "{argv:?}");
    }

    #[test]
    fn claude_stream_json_adds_verbose_but_default_json_does_not() {
        // `-p --output-format stream-json` requires `--verbose` (Claude Code
        // errors otherwise); it is what surfaces the content-block transcript
        // oneharness normalizes into `events`. The default `json` result carries
        // no transcript and must NOT get `--verbose`.
        let spec = by_id("claude-code").unwrap();
        let stream = (spec.build_argv)(&BuildCtx {
            output_format: OutputFormat::StreamJson,
            ..base_ctx(spec)
        });
        assert!(
            stream
                .windows(2)
                .any(|w| w == ["--output-format", "stream-json"]),
            "{stream:?}"
        );
        assert!(
            stream.iter().any(|t| t == "--verbose"),
            "stream-json needs --verbose: {stream:?}"
        );
        let json = (spec.build_argv)(&base_ctx(spec));
        assert!(
            !json.iter().any(|t| t == "--verbose"),
            "default json must not add --verbose: {json:?}"
        );
    }

    #[test]
    fn claude_maps_resume_to_resume_flag() {
        let spec = by_id("claude-code").unwrap();
        assert!(spec.supports_resume);
        let argv = (spec.build_argv)(&BuildCtx {
            resume: Some("sess-123"),
            ..base_ctx(spec)
        });
        assert!(
            argv.windows(2).any(|w| w == ["--resume", "sess-123"]),
            "{argv:?}"
        );
    }

    #[test]
    fn every_harness_declares_its_usage_tier() {
        // `oneharness usage` covers the whole fleet or it undermines the premise
        // that one command works across every harness. Three report real
        // headroom, one reports a plan tier, and four affirmatively report that
        // they cannot — split by *which* cannot, because "no quota exists" and
        // "a quota exists with no reader" are different answers. Each mapping is
        // sourced from docs/harness-usage.md; changing one means an observation
        // changed, so this pins all eight at once.
        let tiers: Vec<(&str, UsageSupport)> =
            all().iter().map(|spec| (spec.id, spec.usage)).collect();

        assert_eq!(
            tiers,
            vec![
                (
                    "claude-code",
                    UsageSupport::Probed(UsageProbe::ClaudeGetUsage)
                ),
                ("codex", UsageSupport::Probed(UsageProbe::CodexAppServer)),
                ("opencode", UsageSupport::NoPlanQuota),
                ("goose", UsageSupport::NoPlanQuota),
                ("qwen", UsageSupport::NoHeadroomReader),
                ("crush", UsageSupport::NoHeadroomReader),
                (
                    "copilot",
                    UsageSupport::Probed(UsageProbe::CopilotUserEndpoint)
                ),
                ("cursor", UsageSupport::Probed(UsageProbe::CursorAbout)),
            ]
        );
    }

    #[test]
    fn every_harness_supports_resume() {
        // All eight CLIs expose a headless continuation flag (sourced per-adapter
        // from their docs); a new harness without one must flip this expectation
        // deliberately rather than silently start a fresh session.
        let unsupported: Vec<&str> = all()
            .iter()
            .filter(|h| !h.supports_resume)
            .map(|h| h.id)
            .collect();
        assert!(
            unsupported.is_empty(),
            "resume gaps drifted: {unsupported:?}"
        );
    }

    #[test]
    fn fork_supported_set_is_claude_and_opencode() {
        // Only Claude Code (`--fork-session`) and OpenCode (`--fork`) expose a
        // headless session fork; the rest resume linearly. A drift alarm for the
        // capability the fork feature depends on.
        let supported: std::collections::HashSet<&str> = all()
            .iter()
            .filter(|h| h.supports_fork)
            .map(|h| h.id)
            .collect();
        assert_eq!(
            supported,
            ["claude-code", "opencode"].into_iter().collect(),
            "supports_fork set drifted"
        );
        // Fork implies resume: nothing forks that cannot also resume.
        assert!(all().iter().all(|h| !h.supports_fork || h.supports_resume));
    }

    #[test]
    fn claude_maps_fork_to_fork_session_flag() {
        let spec = by_id("claude-code").unwrap();
        assert!(spec.supports_fork);
        let argv = (spec.build_argv)(&BuildCtx {
            resume: Some("sess-123"),
            fork: true,
            ..base_ctx(spec)
        });
        assert!(
            argv.windows(2).any(|w| w == ["--resume", "sess-123"]),
            "{argv:?}"
        );
        assert!(argv.iter().any(|t| t == "--fork-session"), "{argv:?}");
        // Without --fork the flag is absent (plain resume appends in place).
        let argv = (spec.build_argv)(&BuildCtx {
            resume: Some("sess-123"),
            ..base_ctx(spec)
        });
        assert!(!argv.iter().any(|t| t == "--fork-session"), "{argv:?}");
    }

    #[test]
    fn opencode_maps_fork_to_fork_flag() {
        let spec = by_id("opencode").unwrap();
        assert!(spec.supports_fork);
        let argv = (spec.build_argv)(&BuildCtx {
            resume: Some("ses_abc"),
            fork: true,
            ..base_ctx(spec)
        });
        assert!(
            argv.windows(2).any(|w| w == ["--session", "ses_abc"]),
            "{argv:?}"
        );
        assert!(argv.iter().any(|t| t == "--fork"), "{argv:?}");
    }

    #[test]
    fn codex_maps_resume_to_resume_subcommand_before_prompt() {
        // `codex exec resume <id> <prompt>`: the `resume` token follows `exec`,
        // and the id is the positional immediately before the prompt.
        let spec = by_id("codex").unwrap();
        assert!(spec.supports_resume && !spec.supports_fork);
        let argv = (spec.build_argv)(&BuildCtx {
            resume: Some("0199-thread"),
            ..base_ctx(spec)
        });
        assert!(
            argv.windows(2).any(|w| w == ["exec", "resume"]),
            "resume is a subcommand after exec: {argv:?}"
        );
        // id directly precedes the prompt positional.
        assert!(
            argv.windows(2).any(|w| w == ["0199-thread", "hi"]),
            "{argv:?}"
        );
        // No fork token for codex.
        assert!(!argv.iter().any(|t| t == "--fork"), "{argv:?}");
    }

    fn codex_argv(mode: PermissionMode, resume: Option<&'static str>) -> Vec<String> {
        let spec = by_id("codex").unwrap();
        (spec.build_argv)(&BuildCtx {
            mode,
            resume,
            ..base_ctx(spec)
        })
    }

    #[test]
    fn codex_fresh_turn_carries_the_sandbox_as_a_flag_per_mode() {
        // A fresh `codex exec` takes the mode's sandbox as `--sandbox S`; bypass
        // is its own flag and default leaves exec's own sandbox alone.
        let cases: &[(PermissionMode, &[&str])] = &[
            (PermissionMode::ReadOnly, &["--sandbox", "read-only"]),
            (PermissionMode::Plan, &["--sandbox", "read-only"]),
            (PermissionMode::Auto, &["--sandbox", "workspace-write"]),
            (
                PermissionMode::Bypass,
                &["--dangerously-bypass-approvals-and-sandbox"],
            ),
            (PermissionMode::Default, &[]),
        ];
        for (mode, sandbox) in cases {
            let mut want = vec![
                "codex",
                "exec",
                "-c",
                "features.plugins=false",
                "-c",
                "features.apps=false",
                "-c",
                "analytics.enabled=false",
            ];
            want.extend_from_slice(sandbox);
            want.extend(["--json", "hi"]);
            assert_eq!(codex_argv(*mode, None), want, "fresh {mode:?}");
        }
    }

    #[test]
    fn codex_resumed_turn_carries_the_sandbox_as_config_per_mode() {
        // `codex exec resume` has no `--sandbox` (codex exits 2 on it before the
        // turn runs), so a resumed turn states the sandbox through the `-c`
        // config override it does accept. Bypass's flag is accepted by both.
        let cases: &[(PermissionMode, &[&str])] = &[
            (PermissionMode::ReadOnly, &["-c", "sandbox_mode=read-only"]),
            (PermissionMode::Plan, &["-c", "sandbox_mode=read-only"]),
            (
                PermissionMode::Auto,
                &["-c", "sandbox_mode=workspace-write"],
            ),
            (
                PermissionMode::Bypass,
                &["--dangerously-bypass-approvals-and-sandbox"],
            ),
            (PermissionMode::Default, &[]),
        ];
        for (mode, sandbox) in cases {
            let argv = codex_argv(*mode, Some("0199-thread"));
            let mut want = vec![
                "codex",
                "exec",
                "resume",
                "-c",
                "features.plugins=false",
                "-c",
                "features.apps=false",
                "-c",
                "analytics.enabled=false",
            ];
            want.extend_from_slice(sandbox);
            want.extend(["--json", "0199-thread", "hi"]);
            assert_eq!(argv, want, "resumed {mode:?}");
            assert!(
                !argv.iter().any(|t| t == "--sandbox"),
                "`exec resume` rejects --sandbox under {mode:?}: {argv:?}"
            );
        }
    }

    #[test]
    fn codex_plugins_are_off_unless_the_run_opts_in_on_every_launch() {
        // Off by default: `-c features.plugins=false` follows the subcommand on
        // a fresh turn, a resumed one and an app-server launch alike, ahead of
        // the caller's args so a caller's own `-c features.plugins=true` comes
        // later and wins. Opted in, no plugins flag at all.
        let spec = by_id("codex").unwrap();
        let caller = vec!["-c".to_string(), "features.plugins=true".to_string()];
        let launches: [(&[&str], Option<&str>, PromptDelivery); 3] = [
            (&["exec"], None, PromptDelivery::Argv),
            (&["exec", "resume"], Some("th-1"), PromptDelivery::Argv),
            (&["app-server"], None, PromptDelivery::ControlStream),
        ];
        for (subcommand, resume, delivery) in launches {
            let off = BuildCtx {
                resume,
                delivery,
                ..base_ctx(spec)
            };
            let argv = argv_with_caller_args(spec, &off, &caller);
            let mut head = vec!["codex"];
            head.extend_from_slice(subcommand);
            head.extend(["-c", "features.plugins=false"]);
            assert_eq!(argv[..head.len()], head, "{subcommand:?}: {argv:?}");
            assert_eq!(argv[argv.len() - 2..], caller, "{subcommand:?}: {argv:?}");
            let on = argv_with_caller_args(
                spec,
                &BuildCtx {
                    plugins: true,
                    ..off
                },
                &[],
            );
            assert!(
                !on.iter().any(|t| t.contains("plugins")),
                "{subcommand:?}: {on:?}"
            );
        }
        // No other harness reads it.
        for other in all().iter().filter(|spec| spec.id != "codex") {
            assert_eq!(
                (other.build_argv)(&base_ctx(other)),
                (other.build_argv)(&BuildCtx {
                    plugins: true,
                    ..base_ctx(other)
                }),
                "{}",
                other.id
            );
        }
    }

    #[test]
    fn codex_apps_are_off_beside_plugins_unless_the_run_opts_in() {
        // `-c features.apps=false` follows the plugins setting on a fresh
        // turn, a resumed one and an app-server launch alike, ahead of the
        // caller's args, so a caller's own `-c features.apps=true` comes later
        // and wins; opted in, no apps flag at all and nothing else moves.
        let spec = by_id("codex").unwrap();
        let caller = vec!["-c".to_string(), "features.apps=true".to_string()];
        let launches: [(&[&str], Option<&str>, PromptDelivery); 3] = [
            (&["exec"], None, PromptDelivery::Argv),
            (&["exec", "resume"], Some("th-1"), PromptDelivery::Argv),
            (&["app-server"], None, PromptDelivery::ControlStream),
        ];
        for (subcommand, resume, delivery) in launches {
            let off = BuildCtx {
                resume,
                delivery,
                ..base_ctx(spec)
            };
            let argv = argv_with_caller_args(spec, &off, &caller);
            let mut head = vec!["codex"];
            head.extend_from_slice(subcommand);
            head.extend(["-c", "features.plugins=false", "-c", "features.apps=false"]);
            assert_eq!(argv[..head.len()], head, "{subcommand:?}: {argv:?}");
            assert_eq!(argv[argv.len() - 2..], caller, "{subcommand:?}: {argv:?}");
            let on = (spec.build_argv)(&BuildCtx { apps: true, ..off });
            assert!(
                !on.iter().any(|t| t.contains("features.apps")),
                "{subcommand:?}: {on:?}"
            );
            let without_apps: Vec<&String> = argv[..argv.len() - 2]
                .iter()
                .filter(|t| *t != "features.apps=false")
                .collect();
            assert_eq!(without_apps.len(), on.len() + 1, "only `-c` itself remains");
        }
        for other in all().iter().filter(|spec| spec.id != "codex") {
            assert_eq!(
                (other.build_argv)(&base_ctx(other)),
                (other.build_argv)(&BuildCtx {
                    apps: true,
                    ..base_ctx(other)
                }),
                "{}",
                other.id
            );
        }
    }

    #[test]
    fn codex_analytics_and_shared_catalog_ride_beside_the_plugins_flag_on_every_launch() {
        // Analytics off and the shared catalog follow the plugins flag right
        // after the subcommand, ahead of the caller's args, on a fresh turn, a
        // resumed one and an app-server launch alike; each opt-out removes
        // exactly its own flag.
        let spec = by_id("codex").unwrap();
        let caller = vec![
            "-c".to_string(),
            "analytics.enabled=true".to_string(),
            "-c".to_string(),
            "model_catalog_json=\"/caller.json\"".to_string(),
        ];
        let launches: [(&[&str], Option<&str>, PromptDelivery); 3] = [
            (&["exec"], None, PromptDelivery::Argv),
            (&["exec", "resume"], Some("th-1"), PromptDelivery::Argv),
            (&["app-server"], None, PromptDelivery::ControlStream),
        ];
        let catalog = Path::new(r"C:\state\catalog.json");
        for (subcommand, resume, delivery) in launches {
            let ctx = BuildCtx {
                resume,
                delivery,
                model_catalog: Some(catalog),
                ..base_ctx(spec)
            };
            let argv = argv_with_caller_args(spec, &ctx, &caller);
            let mut head = vec!["codex"];
            head.extend_from_slice(subcommand);
            head.extend([
                "-c",
                "features.plugins=false",
                "-c",
                "features.apps=false",
                "-c",
                "analytics.enabled=false",
                "-c",
                r"model_catalog_json='C:\state\catalog.json'",
            ]);
            assert_eq!(argv[..head.len()], head, "{subcommand:?}: {argv:?}");
            assert_eq!(argv[argv.len() - 4..], caller, "{subcommand:?}: {argv:?}");
            let without = |ctx: &BuildCtx| (spec.build_argv)(ctx);
            let analytics_on = without(&BuildCtx {
                analytics: true,
                ..ctx
            });
            assert!(
                !analytics_on.iter().any(|t| t.starts_with("analytics.")),
                "{analytics_on:?}"
            );
            assert!(analytics_on
                .iter()
                .any(|t| t.starts_with("model_catalog_json=")));
            let no_catalog = without(&BuildCtx {
                model_catalog: None,
                ..ctx
            });
            assert!(
                !no_catalog.iter().any(|t| t.contains("model_catalog")),
                "{no_catalog:?}"
            );
            assert!(no_catalog.iter().any(|t| t == "analytics.enabled=false"));
        }
        // Codex reads a `-c` value as TOML, so the path is a TOML string.
        assert_eq!(
            codex_model_catalog_setting(Path::new("/state/a \"b\".json")),
            r#"model_catalog_json='/state/a "b".json'"#
        );
        let parsed: toml::Table =
            toml::from_str(&codex_model_catalog_setting(Path::new(r"C:\a\b.json"))).unwrap();
        assert_eq!(parsed["model_catalog_json"].as_str(), Some(r"C:\a\b.json"));
        // No other harness reads either.
        for other in all().iter().filter(|spec| spec.id != "codex") {
            assert_eq!(
                (other.build_argv)(&base_ctx(other)),
                (other.build_argv)(&BuildCtx {
                    analytics: true,
                    model_catalog: Some(Path::new("/catalog.json")),
                    ..base_ctx(other)
                }),
                "{}",
                other.id
            );
        }
    }

    #[test]
    fn a_resumed_codex_turn_keeps_the_mode_sandbox_over_a_callers_override() {
        // Fresh, codex's `--sandbox` outranks any `-c sandbox_mode=`; resumed,
        // the last `-c` of the key wins. So the mode's override is restated
        // after a caller's, and the mode governs both turns alike.
        let spec = by_id("codex").unwrap();
        let caller = |args: &[&str]| args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        let assembled = |mode, resume, extra: &[String]| {
            argv_with_caller_args(
                spec,
                &BuildCtx {
                    mode,
                    resume,
                    ..base_ctx(spec)
                },
                extra,
            )
        };
        let last_sandbox_mode = |argv: &[String]| {
            argv.windows(2)
                .filter(|w| w[0] == "-c" && w[1].starts_with("sandbox_mode="))
                .map(|w| w[1].clone())
                .next_back()
        };
        for spelling in [
            &["-c", "sandbox_mode=danger-full-access"][..],
            &["--config", "sandbox_mode=\"danger-full-access\""][..],
            &["--config=sandbox_mode=danger-full-access"][..],
            &["-csandbox_mode=danger-full-access"][..],
        ] {
            let extra = caller(spelling);
            for (mode, sandbox) in [
                (PermissionMode::ReadOnly, "read-only"),
                (PermissionMode::Plan, "read-only"),
                (PermissionMode::Auto, "workspace-write"),
            ] {
                // Fresh: the mode's flag, the caller's args after it verbatim.
                let fresh = assembled(mode, None, &extra);
                assert!(
                    fresh.windows(2).any(|w| w == ["--sandbox", sandbox]),
                    "{fresh:?}"
                );
                assert!(fresh.ends_with(&extra), "{fresh:?}");
                // Resumed: the caller's args, then the mode's override last.
                let resumed = assembled(mode, Some("0199-thread"), &extra);
                let mode_override = format!("sandbox_mode={sandbox}");
                assert!(
                    resumed.ends_with(&[extra.clone(), caller(&["-c", &mode_override])].concat()),
                    "{spelling:?} {mode:?}: {resumed:?}"
                );
                assert_eq!(last_sandbox_mode(&resumed), Some(mode_override));
            }
            // Default names no sandbox: the caller's governs, fresh and resumed.
            for resume in [None, Some("0199-thread")] {
                let argv = assembled(PermissionMode::Default, resume, &extra);
                assert!(argv.ends_with(&extra), "{argv:?}");
                assert!(!argv.iter().any(|t| t == "--sandbox"), "{argv:?}");
                assert_eq!(
                    argv.iter().filter(|t| t.contains("sandbox_mode")).count(),
                    1
                );
            }
        }
        // A caller's args that leave `sandbox_mode` alone are appended verbatim:
        // no restatement, on the key or a look-alike one.
        for other in [
            &["-c", "model_reasoning_effort=high"][..],
            &["-c", "sandbox_mode_extra=x"][..],
            &["-c"][..],
            &["--sandbox-mode=workspace-write"][..],
        ] {
            let extra = caller(other);
            let mut want = codex_argv(PermissionMode::Auto, Some("0199-thread"));
            want.extend(extra.clone());
            assert_eq!(
                assembled(PermissionMode::Auto, Some("0199-thread"), &extra),
                want
            );
        }
        // Another harness's args are never touched.
        let claude = by_id("claude-code").unwrap();
        let extra = caller(&["-c", "sandbox_mode=x"]);
        let ctx = BuildCtx {
            mode: PermissionMode::Auto,
            resume: Some("sid"),
            ..base_ctx(claude)
        };
        let mut want = (claude.build_argv)(&ctx);
        want.extend(extra.clone());
        assert_eq!(argv_with_caller_args(claude, &ctx, &extra), want);
    }

    #[test]
    fn goose_maps_resume_to_named_session() {
        // Goose emits no id headlessly; continuation rides a caller-chosen name:
        // `--resume --name <name>`.
        let spec = by_id("goose").unwrap();
        assert!(spec.supports_resume);
        let argv = (spec.build_argv)(&BuildCtx {
            resume: Some("my-session"),
            ..base_ctx(spec)
        });
        assert!(argv.iter().any(|t| t == "--resume"), "{argv:?}");
        assert!(
            argv.windows(2).any(|w| w == ["--name", "my-session"]),
            "{argv:?}"
        );
    }

    #[test]
    fn qwen_maps_resume_to_resume_flag() {
        let spec = by_id("qwen").unwrap();
        assert!(spec.supports_resume);
        let argv = (spec.build_argv)(&BuildCtx {
            resume: Some("uuid-1"),
            ..base_ctx(spec)
        });
        assert!(
            argv.windows(2).any(|w| w == ["--resume", "uuid-1"]),
            "{argv:?}"
        );
    }

    #[test]
    fn crush_maps_resume_to_session_flag() {
        let spec = by_id("crush").unwrap();
        assert!(spec.supports_resume);
        let argv = (spec.build_argv)(&BuildCtx {
            resume: Some("sess-9"),
            ..base_ctx(spec)
        });
        assert!(
            argv.windows(2).any(|w| w == ["--session", "sess-9"]),
            "{argv:?}"
        );
    }

    #[test]
    fn copilot_maps_resume_to_resume_flag() {
        let spec = by_id("copilot").unwrap();
        assert!(spec.supports_resume);
        let argv = (spec.build_argv)(&BuildCtx {
            resume: Some("uuid-c"),
            ..base_ctx(spec)
        });
        assert!(
            argv.windows(2).any(|w| w == ["--resume", "uuid-c"]),
            "{argv:?}"
        );
    }

    #[test]
    fn opencode_maps_resume_to_session_flag() {
        let spec = by_id("opencode").unwrap();
        assert!(spec.supports_resume);
        let argv = (spec.build_argv)(&BuildCtx {
            resume: Some("ses_abc"),
            ..base_ctx(spec)
        });
        assert!(
            argv.windows(2).any(|w| w == ["--session", "ses_abc"]),
            "{argv:?}"
        );
    }

    #[test]
    fn cursor_maps_resume_to_resume_flag() {
        let spec = by_id("cursor").unwrap();
        assert!(spec.supports_resume);
        let argv = (spec.build_argv)(&BuildCtx {
            resume: Some("chat-9"),
            ..base_ctx(spec)
        });
        assert!(
            argv.windows(2).any(|w| w == ["--resume", "chat-9"]),
            "{argv:?}"
        );
    }

    #[test]
    fn cursor_no_bypass_trusts_the_workspace_without_force() {
        let spec = by_id("cursor").unwrap();
        let argv = (spec.build_argv)(&BuildCtx {
            mode: PermissionMode::Default,
            ..base_ctx(spec)
        });
        assert!(argv.iter().any(|t| t == "--trust"), "{argv:?}");
        assert!(!argv.iter().any(|t| t == "--force"), "{argv:?}");
        // Bypass mode keeps the plain --force (which implies trust).
        let argv = (spec.build_argv)(&base_ctx(spec));
        assert!(argv.iter().any(|t| t == "--force"), "{argv:?}");
        assert!(!argv.iter().any(|t| t == "--trust"), "{argv:?}");
    }

    #[test]
    fn qwen_alone_declares_the_yolo_suppression_default_env() {
        // Qwen prints a one-line YOLO/no-sandbox warning to stderr under `--yolo`;
        // oneharness silences it so headless `stderr` stays clean. No other
        // harness needs a default env today — guard that the set hasn't drifted.
        for h in all() {
            if h.id == "qwen" {
                assert_eq!(
                    h.default_env,
                    &[("QWEN_CODE_SUPPRESS_YOLO_WARNING", "1")],
                    "qwen should suppress its YOLO warning"
                );
            } else {
                assert!(
                    h.default_env.is_empty(),
                    "harness {} unexpectedly declares default env",
                    h.id
                );
            }
        }
    }

    #[test]
    fn bin_override_lands_at_argv0_for_every_harness() {
        for h in all() {
            let argv = (h.build_argv)(&ctx("/custom/bin", None, PermissionMode::Bypass));
            assert_eq!(argv[0], "/custom/bin", "harness {}", h.id);
        }
    }

    #[test]
    fn model_flag_is_emitted_for_every_model_aware_harness() {
        // Each harness that accepts a model spells the flag its own way; `--model`
        // must reach the child via that spelling, never be dropped. Goose is the
        // sole exception — it selects its model from its own config — so its argv
        // is identical with and without a model (asserted separately).
        let expected: &[(&str, &[&str])] = &[
            ("claude-code", &["--model", "m"]),
            ("codex", &["--model", "m"]),
            ("opencode", &["-m", "m"]),
            ("qwen", &["-m", "m"]),
            ("crush", &["-m", "m"]),
            ("copilot", &["--model", "m"]),
            ("cursor", &["--model", "m"]),
        ];
        for (id, want) in expected {
            let spec = by_id(id).unwrap();
            let argv = (spec.build_argv)(&ctx(spec.default_bin, Some("m"), PermissionMode::Bypass));
            assert!(
                argv.windows(2).any(|w| w == *want),
                "harness {id} should carry {want:?}; got {argv:?}"
            );
        }
        // Goose deliberately ignores the model: argv is unchanged when one is set.
        let goose = by_id("goose").unwrap();
        let with = (goose.build_argv)(&ctx("goose", Some("m"), PermissionMode::Bypass));
        let without = (goose.build_argv)(&ctx("goose", None, PermissionMode::Bypass));
        assert_eq!(with, without, "goose should ignore --model");
    }
}
