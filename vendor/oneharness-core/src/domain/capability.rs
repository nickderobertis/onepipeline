//! The declared capability surface every consumer surface is measured against.
//!
//! This is the single source the parity gate reconciles three ways:
//!
//! * against **clap** — `tests/capability.rs` in the binary crate walks the real
//!   command tree, so a flag added to `src/cli.rs` without a decision here (bind
//!   it, or declare it uncovered with a reason) fails the build;
//! * against the **language SDKs** — `sdk_schema::bundle` emits this manifest
//!   into each generated package, and `scripts/sdk-coverage.mjs` asserts every
//!   capability has a method whose argv builder emits every bound flag;
//! * against **`oneharness-core`** — each capability names the library entry
//!   point a Rust consumer calls instead of spawning the CLI, and
//!   `tests/library_surface.rs` exercises every one of them.
//!
//! It is deliberately data rather than a hand-maintained checklist: the
//! bindings ARE how the SDKs build their argv, so a binding that is wrong is a
//! broken call rather than a stale note.

use serde::Serialize;

/// How one SDK option reaches the CLI.
///
/// The flag lives *in* the variants that have one, so "a positional argument
/// carrying `--harness`" and "a `--flag VALUE` binding with no flag" are not
/// states this type can hold. They used to be: `flag` and `kind` sat beside
/// each other and a runtime assertion in `tests/capability.rs` was all that
/// stood between the manifest and an SDK rendering `--` as an argument.
///
/// The serialized shape is unchanged — flat `{"flag": …, "kind": …}`, with
/// `""` for the flagless variants — because both SDK generators read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    /// A bare positional argument, appended in binding order.
    Positional,
    /// `--flag VALUE`, once.
    Value(&'static str),
    /// `--flag VALUE`, once per array element.
    Repeated(&'static str),
    /// `--flag`, present only when the option is true.
    Switch(&'static str),
    /// `--flag KEY=VALUE`, once per map entry.
    KeyValue(&'static str),
    /// Every array element appended verbatim after a `--` separator.
    Trailing,
    /// A [`HistoryWindow`](crate::domain::history_index::HistoryWindow) value,
    /// rendered as exactly one of its flags: `{"recent": {"days": N}}` as
    /// `--days N`, `{"since": D}` as `--since D`, and `"allTime"` as
    /// `--all-time`. One option carrying the whole window is what keeps a
    /// since-date and all-time from being stated together.
    Window(WindowFlags),
}

/// The three CLI spellings a [`FlagKind::Window`] option renders one of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowFlags {
    /// Rendered with the day count of a `recent` window.
    pub days: &'static str,
    /// Rendered with the date of a `since` window.
    pub since: &'static str,
    /// Rendered alone for the `allTime` window.
    pub all_time: &'static str,
}

/// The history verbs' window flags, one spelling on every verb that reads a
/// window.
pub const HISTORY_WINDOW_FLAGS: WindowFlags = WindowFlags {
    days: "--days",
    since: "--since",
    all_time: "--all-time",
};

impl FlagKind {
    /// The CLI spelling this binding renders, or `None` when it renders a bare
    /// argument. A caller that wants the wire spelling wants
    /// [`OptionBinding::flag`], which is `""` rather than absent.
    #[must_use]
    pub const fn flag(self) -> Option<&'static str> {
        match self {
            FlagKind::Positional | FlagKind::Trailing | FlagKind::Window(_) => None,
            FlagKind::Value(flag)
            | FlagKind::Repeated(flag)
            | FlagKind::Switch(flag)
            | FlagKind::KeyValue(flag) => Some(flag),
        }
    }

    /// The discriminant the SDK generators switch on.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            FlagKind::Positional => "positional",
            FlagKind::Value(_) => "value",
            FlagKind::Repeated(_) => "repeated",
            FlagKind::Switch(_) => "switch",
            FlagKind::KeyValue(_) => "key-value",
            FlagKind::Trailing => "trailing",
            FlagKind::Window(_) => "window",
        }
    }

    /// Every CLI spelling this binding can render: its one flag, or each of a
    /// window's three. What the clap reconciliation counts as decided.
    #[must_use]
    pub fn spellings(self) -> Vec<&'static str> {
        match self {
            FlagKind::Window(flags) => vec![flags.days, flags.since, flags.all_time],
            other => other.flag().into_iter().collect(),
        }
    }
}

impl Serialize for FlagKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.wire_name())
    }
}

/// What it means for both members of a suppressed pair to render an argument.
///
/// Suppression alone cannot say it. Every pair here is a clap conflict, but the
/// two kinds of request that reach one are opposites: `{session, last: true}` is
/// a lookup the union deliberately accepts and resolves to "the most recent",
/// while `{all: true, harnesses: ["codex"]}` is a caller asking for every
/// harness and for one harness in the same breath. Editing the conflict out of
/// the second — which is what a bare suppression does — spends a paid turn on an
/// identity the caller did not choose, and tells them only through behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlessResolution {
    /// Both rendering is a contradiction: the SDKs refuse the call, naming both
    /// options, before anything is spawned.
    Refuse,
    /// Both rendering is deliberate precedence: the suppressor wins, quietly,
    /// because the request still has one meaning.
    Prefer,
}

impl UnlessResolution {
    /// The discriminant the SDK argv builders switch on.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            UnlessResolution::Refuse => "refuse",
            UnlessResolution::Prefer => "prefer",
        }
    }
}

impl Serialize for UnlessResolution {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.wire_name())
    }
}

/// Another option that suppresses a binding *when it renders an argument*, and
/// what the pair means when both do.
///
/// The one thing a flat binding list cannot say by itself: `history show` takes
/// `--last` OR a session name and clap refuses both, so a lookup carrying
/// `{session, last: true}` must render only `--last`. Declaring the suppression
/// keeps that rule in the manifest instead of in each SDK.
///
/// The conflict being encoded is clap's, and clap conflicts on a flag being
/// *present*, so the test each SDK applies is whether the named option renders
/// anything — never the host language's truthiness. The two differ:
/// `{all: true, harnesses: []}` — the shape a caller assembling options
/// programmatically produces — sends no `--harness`, so it must keep `--all`,
/// yet an empty array is truthy in JavaScript. Reading it as truth there dropped
/// the only selection such a call carried.
///
/// [`UnlessResolution::Refuse`] asks something narrower still, because a
/// contradiction takes two positive assertions: an empty value renders — and so
/// suppresses — while asking for nothing, so `{system: "", systemFile: …}` is a
/// defaulted key beside a real choice rather than a caller wanting two things.
///
/// The resolution lives here rather than beside `unless` so a suppression
/// without one cannot be written at all — the accident this type exists to
/// prevent is a new pair inheriting the old silent behavior by omission.
#[derive(Debug, Clone, Copy)]
pub struct Suppression {
    /// The sibling option that suppresses this binding.
    pub option: &'static str,
    /// What both rendering means.
    pub resolution: UnlessResolution,
}

/// One SDK option and the CLI flag it renders to.
#[derive(Debug, Clone, Copy)]
pub struct OptionBinding {
    /// The option's name in the SDK input contract (camelCase; the Python SDK
    /// accepts the same key, so one spelling serves both).
    pub option: &'static str,
    /// How it renders, including the flag when it has one.
    pub kind: FlagKind,
    /// The sibling option that suppresses this one, and what both rendering
    /// means. See [`Suppression`].
    pub unless: Option<Suppression>,
}

impl OptionBinding {
    /// The CLI spelling this binding renders, `""` when it renders a bare
    /// argument — the wire spelling both SDK generators read.
    #[must_use]
    pub const fn flag(&self) -> &'static str {
        match self.kind.flag() {
            Some(flag) => flag,
            None => "",
        }
    }
}

impl Serialize for OptionBinding {
    /// Flattened by hand so the enum's payload stays out of the wire shape:
    /// the generators have always read `{option, flag, kind, unless}`, and
    /// making the invalid combinations unrepresentable in Rust is not a reason
    /// to reshape a contract two SDKs generate from.
    ///
    /// `unless_resolution` is additive for the same reason: it is a new optional
    /// key beside the four, emitted only where there is a suppression to resolve,
    /// so a binding without one serializes byte for byte as it always has.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let window = match self.kind {
            FlagKind::Window(flags) => Some(flags),
            _ => None,
        };
        let fields = 4 + usize::from(self.unless.is_some()) + usize::from(window.is_some());
        let mut out = serializer.serialize_struct("OptionBinding", fields)?;
        out.serialize_field("option", self.option)?;
        out.serialize_field("flag", self.flag())?;
        out.serialize_field("kind", &self.kind)?;
        out.serialize_field("unless", &self.unless.map(|unless| unless.option))?;
        if let Some(unless) = self.unless {
            out.serialize_field("unless_resolution", &unless.resolution)?;
        }
        // Additive like `unless_resolution`: only a window binding carries the
        // three spellings it chooses among, so every other binding is unchanged.
        if let Some(flags) = window {
            out.serialize_field("window", &flags)?;
        }
        out.end()
    }
}

/// A CLI flag no SDK option renders, and why that is correct.
///
/// Silence is not an option here: a flag that is neither bound nor listed with
/// a reason fails the clap reconciliation.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
pub struct UncoveredFlag {
    pub flag: &'static str,
    pub reason: &'static str,
}

/// How a verb's stdout reaches an SDK caller.
///
/// The output contract's schema root lives *in* the variants that validate
/// one, so "JSON stdout with nothing to validate it against" and "text stdout
/// carrying an output schema" cannot be written. The first of those is the
/// defect that ships as an SDK method typed `unknown`; it was previously held
/// off by a runtime assertion rather than by the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdoutShape {
    /// One JSON document, validated against the named schema root.
    Json(&'static str),
    /// One validated JSON document per line, yielded as it arrives.
    Jsonl(&'static str),
    /// Not a contract at all. `init` writes a human confirmation line by
    /// design — its deliverable is the file — and `gate`/`mock` answer with a
    /// harness's own native verdict, or with nothing at all when the call is
    /// allowed through. The SDK returns the text (or `null`) rather than
    /// pretending it parsed a document. There is no schema root because there
    /// is no document.
    Text,
}

impl StdoutShape {
    /// The schema root this capability's output validates against, or `None`
    /// for [`StdoutShape::Text`], which has no document.
    #[must_use]
    pub const fn output(self) -> Option<&'static str> {
        match self {
            StdoutShape::Json(root) | StdoutShape::Jsonl(root) => Some(root),
            StdoutShape::Text => None,
        }
    }

    /// The discriminant the SDK generators switch on.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            StdoutShape::Json(_) => "json",
            StdoutShape::Jsonl(_) => "jsonl",
            StdoutShape::Text => "text",
        }
    }
}

impl Serialize for StdoutShape {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.wire_name())
    }
}

/// One thing this CLI can do, and how each consumer surface reaches it.
#[derive(Debug, Clone, Copy)]
pub struct Capability {
    /// The SDK method name in camelCase. The Python SDK snake-cases it, so
    /// `historyList` is `history_list` there — one declaration, both spellings.
    pub method: &'static str,
    /// The verb path this capability invokes, e.g. `["history", "list"]`.
    pub argv: &'static [&'static str],
    /// The schema root of its input contract, or `None` for a verb that takes
    /// no options at all.
    pub options: Option<&'static str>,
    /// How the SDK reads the verb's stdout, and — for the JSON shapes — the
    /// schema root its output validates against.
    ///
    /// The root cannot disagree with the shape, because it lives inside it.
    /// `every_named_schema_root_is_emitted_by_the_bundle` still refuses a root
    /// with no source behind it, so "an SDK method returning `unknown`" is
    /// unreachable from both directions: one by the type, one by that test.
    pub stdout: StdoutShape,
    /// Whether the call writes a payload to the CLI's stdin (the hook verbs).
    pub stdin: bool,
    /// The `oneharness-core` entry point a Rust consumer calls instead of
    /// spawning. `tests/library_surface.rs` exercises every one of them, and
    /// `scripts/check-capability-surface.sh` holds the two to each other.
    pub rust: &'static str,
    /// Argv fragments this capability always emits, whatever its options say —
    /// `--compact` for the JSON verbs, `--stream` for the streaming run. Counted
    /// as covered, because the method does emit them.
    pub always: &'static [&'static str],
    pub bindings: &'static [OptionBinding],
    pub uncovered: &'static [UncoveredFlag],
}

impl Serialize for Capability {
    /// Flattened by hand for the same reason as [`OptionBinding`]: the wire
    /// shape both SDK generators read keeps `stdout` and `output` as separate
    /// keys, and folding the root into the variant is a Rust-side invariant,
    /// not a contract change.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("Capability", 9)?;
        out.serialize_field("method", self.method)?;
        out.serialize_field("argv", self.argv)?;
        out.serialize_field("options", &self.options)?;
        out.serialize_field("output", &self.output())?;
        out.serialize_field("stdout", &self.stdout)?;
        out.serialize_field("stdin", &self.stdin)?;
        out.serialize_field("rust", self.rust)?;
        out.serialize_field("always", self.always)?;
        out.serialize_field("bindings", self.bindings)?;
        out.serialize_field("uncovered", self.uncovered)?;
        out.end()
    }
}

impl Capability {
    /// The schema root of its output contract, or `None` for a text verb.
    #[must_use]
    pub const fn output(&self) -> Option<&'static str> {
        self.stdout.output()
    }

    /// The Python SDK's spelling of [`Capability::method`].
    #[must_use]
    pub fn python_method(&self) -> String {
        let mut out = String::with_capacity(self.method.len() + 2);
        for ch in self.method.chars() {
            if ch.is_ascii_uppercase() {
                out.push('_');
                out.push(ch.to_ascii_lowercase());
            } else {
                out.push(ch);
            }
        }
        out
    }
}

const fn bind(option: &'static str, kind: FlagKind) -> OptionBinding {
    OptionBinding {
        option,
        kind,
        unless: None,
    }
}

// Every suppressed pair below, and why it resolves the way it does. The
// question each answers is what a caller *meant* by rendering both, since one
// meaning is a mistake to report and the other is a request to serve:
//
// * `all` / `harnesses` (run, runStream, detect, usage) — refuse. "Every
//   harness" and "these harnesses" are different selections, and quietly
//   keeping the narrower one bills a turn to an identity nobody chose.
// * `systemFile` / `system` (run) — refuse. Two sources for one system prompt;
//   dropping either sends the agent instructions the caller did not write.
// * `config` / `noConfig` (every verb that layers config) — refuse. "Layer these
//   files" and "layer nothing" cannot both hold, and the loser decides which
//   model, mode and history store the run uses.
// * `history` / `noHistory` (run, runStream) — refuse. Recording a run and not
//   recording it are opposites; the quiet answer is a diagnosis read from a
//   store that was never written.
// * `project` / `allProjects` (history show, list, watch, clear) — refuse. One
//   project's store or every project's; `historyClear` makes the wrong answer
//   destructive, and the rest hand back records from a store nobody asked for.
// * `after` / `window` (history watch) — refuse. A cursor and a start are two
//   answers to where a watch begins, and the CLI refuses each pair. The
//   window's own three answers (`--days`, `--since`, `--all-time`) need no
//   pair at all: they are one `HistoryWindow` value, so no call can hold two.
// * `session` / `last` (history show) — prefer. The lookup union deliberately
//   accepts `{session, last: true}` and defines it as "the most recent", so the
//   request has one meaning and `--last` is it. This is the pair the mechanism
//   was built for, and the only one whose members are not rival answers.

/// Bind an option whose sibling `unless` contradicts it: a call rendering both
/// asked for two different things, and the SDKs refuse it rather than pick.
const fn bind_refuse(option: &'static str, kind: FlagKind, unless: &'static str) -> OptionBinding {
    OptionBinding {
        option,
        kind,
        unless: Some(Suppression {
            option: unless,
            resolution: UnlessResolution::Refuse,
        }),
    }
}

/// Bind an option its sibling `unless` deliberately outranks: rendering both is
/// a request with one meaning, and the suppressor is it.
const fn bind_prefer(option: &'static str, kind: FlagKind, unless: &'static str) -> OptionBinding {
    OptionBinding {
        option,
        kind,
        unless: Some(Suppression {
            option: unless,
            resolution: UnlessResolution::Prefer,
        }),
    }
}

const fn skip(flag: &'static str, reason: &'static str) -> UncoveredFlag {
    UncoveredFlag { flag, reason }
}

/// The argv every JSON-document capability always emits: the SDKs parse the
/// JSON, so the rendering is compact and the format is stated outright rather
/// than left to the CLI's default — which is what lets that default move to
/// the human-readable view later without any SDK noticing. `--format text` is
/// therefore never an option: an SDK consumes the contract, and text is the
/// view of the same data a person reads.
const JSON_DOCUMENT: &[&str] = &["--compact", "--format", "json"];

/// Bindings shared by `run` and `runStream`: both drive the same verb, and the
/// two methods differ only in how they read its stdout.
const RUN_BINDINGS: &[OptionBinding] = &[
    bind("prompt", FlagKind::Value("--prompt")),
    bind("batchPrompts", FlagKind::Repeated("--prompt")),
    bind("promptFiles", FlagKind::Repeated("--prompt-file")),
    bind("harnesses", FlagKind::Repeated("--harness")),
    bind("mockHarnesses", FlagKind::Repeated("--mock-harness")),
    bind_refuse("all", FlagKind::Switch("--all"), "harnesses"),
    bind("exclude", FlagKind::Repeated("--exclude")),
    bind("models", FlagKind::Repeated("--model")),
    bind("system", FlagKind::Value("--system")),
    bind_refuse("systemFile", FlagKind::Value("--system-file"), "system"),
    bind("reasoning", FlagKind::Value("--reasoning")),
    bind("resume", FlagKind::Value("--resume")),
    bind("fork", FlagKind::Switch("--fork")),
    bind("session", FlagKind::Value("--session")),
    bind("sessionDir", FlagKind::Value("--session-dir")),
    bind("control", FlagKind::Switch("--control")),
    bind("outputFormat", FlagKind::Value("--output-format")),
    bind("events", FlagKind::Switch("--events")),
    bind("mockRules", FlagKind::Value("--mock-rules")),
    bind("spyFile", FlagKind::Value("--spy-file")),
    bind("schema", FlagKind::Value("--schema")),
    bind("schemaMaxRetries", FlagKind::Value("--schema-max-retries")),
    bind(
        "serverOverloadedMaxRetries",
        FlagKind::Value("--server-overloaded-max-retries"),
    ),
    bind("outputDir", FlagKind::Value("--output-dir")),
    bind("timeoutSeconds", FlagKind::Value("--timeout")),
    bind("cwd", FlagKind::Value("--cwd")),
    bind("env", FlagKind::KeyValue("--env")),
    bind("mode", FlagKind::Value("--mode")),
    bind("permitPrompts", FlagKind::Switch("--permit-prompts")),
    bind_refuse("config", FlagKind::Repeated("--config"), "noConfig"),
    bind("noConfig", FlagKind::Switch("--no-config")),
    bind("maxParallel", FlagKind::Value("--max-parallel")),
    bind("batchStrategy", FlagKind::Value("--batch-strategy")),
    bind("runMode", FlagKind::Value("--run-mode")),
    bind("printCommand", FlagKind::Switch("--print-command")),
    bind("bins", FlagKind::KeyValue("--bin")),
    bind("requireAvailable", FlagKind::Switch("--require-available")),
    bind_refuse("history", FlagKind::Switch("--history"), "noHistory"),
    bind("noHistory", FlagKind::Switch("--no-history")),
    bind("historyDir", FlagKind::Value("--history-dir")),
    bind(
        "historyPointerFile",
        FlagKind::Value("--history-pointer-file"),
    ),
    bind("historyName", FlagKind::Value("--history-name")),
    bind("historyLabels", FlagKind::KeyValue("--history-label")),
    bind("passthrough", FlagKind::Trailing),
];

/// The `run` flags neither run method binds. The two `--mode` shorthands are
/// refused for the same reason on both — one setting with two spellings is how
/// a caller ends up passing both, which clap then refuses — and each method
/// declines the streaming half the other one owns.
const RUN_UNCOVERED: &[UncoveredFlag] = &[
    skip("--bypass", "`mode: \"bypass\"` is the same request"),
    skip("--no-bypass", "`mode: \"default\"` is the same request"),
    skip(
        "--stream",
        "`runStream()` is the streaming method; this one returns one report",
    ),
];

const RUN_STREAM_UNCOVERED: &[UncoveredFlag] = &[
    skip("--bypass", "`mode: \"bypass\"` is the same request"),
    skip("--no-bypass", "`mode: \"default\"` is the same request"),
    skip(
        "--no-stream",
        "this method streams by definition, so the negative half cannot apply",
    ),
];

/// Every capability this CLI exposes, and how each consumer surface reaches it.
///
/// Adding a verb or a flag means adding a row (or a binding) here: the clap
/// reconciliation in `tests/capability.rs` fails on a flag with no decision, and
/// `scripts/check-capability-surface.sh` fails on a capability an SDK does not
/// implement. Neither can be satisfied by editing a checklist — the bindings ARE
/// how the SDKs build their argv.
pub const CAPABILITIES: &[Capability] = &[
    Capability {
        method: "run",
        argv: &["run"],
        options: Some("run_options"),
        stdout: StdoutShape::Json("run_report"),
        stdin: false,
        rust: "oneharness_core::io::run::run",
        always: &["--compact", "--format", "json", "--no-stream"],
        bindings: RUN_BINDINGS,
        uncovered: RUN_UNCOVERED,
    },
    Capability {
        method: "runStream",
        argv: &["run"],
        options: Some("run_options"),
        stdout: StdoutShape::Jsonl("run_stream_envelope"),
        stdin: false,
        rust: "oneharness_core::io::run::run",
        // A stream prints text unless JSON is asked for by name, so the SDK
        // asks: it parses the NDJSON protocol, never the view a person reads.
        always: &["--compact", "--format", "json", "--stream"],
        bindings: RUN_BINDINGS,
        uncovered: RUN_STREAM_UNCOVERED,
    },
    Capability {
        method: "list",
        // The one verb with no options at all: it describes the registry, and
        // the CLI gives it nothing to narrow or steer.
        argv: &["list"],
        options: None,
        stdout: StdoutShape::Json("list_report"),
        stdin: false,
        rust: "oneharness_core::io::registry::list",
        always: JSON_DOCUMENT,
        bindings: &[],
        uncovered: &[],
    },
    Capability {
        method: "detect",
        argv: &["detect"],
        options: Some("detect_options"),
        stdout: StdoutShape::Json("detect_report"),
        stdin: false,
        rust: "oneharness_core::io::detect::detect",
        always: JSON_DOCUMENT,
        bindings: &[
            bind("harnesses", FlagKind::Repeated("--harness")),
            bind_refuse("all", FlagKind::Switch("--all"), "harnesses"),
            bind("exclude", FlagKind::Repeated("--exclude")),
            bind("bins", FlagKind::KeyValue("--bin")),
            bind_refuse("config", FlagKind::Repeated("--config"), "noConfig"),
            bind("noConfig", FlagKind::Switch("--no-config")),
            bind("requireAvailable", FlagKind::Switch("--require-available")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "config",
        argv: &["config"],
        options: Some("config_options"),
        stdout: StdoutShape::Json("config_report"),
        stdin: false,
        rust: "oneharness_core::domain::config::explain",
        always: JSON_DOCUMENT,
        bindings: &[
            bind("cwd", FlagKind::Value("--cwd")),
            bind_refuse("config", FlagKind::Repeated("--config"), "noConfig"),
            bind("noConfig", FlagKind::Switch("--no-config")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "sync",
        argv: &["sync"],
        options: Some("sync_options"),
        stdout: StdoutShape::Json("sync_report"),
        stdin: false,
        rust: "oneharness_core::io::sync::sync_with",
        always: JSON_DOCUMENT,
        bindings: &[
            bind("cwd", FlagKind::Value("--cwd")),
            bind("harnesses", FlagKind::Repeated("--harness")),
            bind("check", FlagKind::Switch("--check")),
            bind("exact", FlagKind::Switch("--exact")),
            bind("global", FlagKind::Switch("--global")),
            bind_refuse("config", FlagKind::Repeated("--config"), "noConfig"),
            bind("noConfig", FlagKind::Switch("--no-config")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "init",
        argv: &["init"],
        options: Some("init_options"),
        stdout: StdoutShape::Text,
        stdin: false,
        rust: "oneharness_core::io::init::init",
        always: &[],
        bindings: &[
            bind("path", FlagKind::Positional),
            bind("force", FlagKind::Switch("--force")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "usage",
        argv: &["usage"],
        options: Some("usage_options"),
        stdout: StdoutShape::Json("usage_report"),
        stdin: false,
        rust: "oneharness_core::io::usage::report",
        always: JSON_DOCUMENT,
        bindings: &[
            bind("harnesses", FlagKind::Repeated("--harness")),
            bind_refuse("all", FlagKind::Switch("--all"), "harnesses"),
            bind("exclude", FlagKind::Repeated("--exclude")),
            bind("bins", FlagKind::KeyValue("--bin")),
            bind("cwd", FlagKind::Value("--cwd")),
            bind("timeoutSeconds", FlagKind::Value("--timeout")),
            bind_refuse("config", FlagKind::Repeated("--config"), "noConfig"),
            bind("noConfig", FlagKind::Switch("--no-config")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "gate",
        argv: &["gate"],
        options: Some("gate_options"),
        stdout: StdoutShape::Text,
        stdin: true,
        rust: "oneharness_core::domain::gate::render_deny",
        always: &[],
        bindings: &[
            bind("harness", FlagKind::Positional),
            bind("denyIfContains", FlagKind::Value("--deny-if-contains")),
            bind("reason", FlagKind::Value("--reason")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "mock",
        argv: &["mock"],
        options: Some("mock_options"),
        stdout: StdoutShape::Text,
        stdin: true,
        rust: "oneharness_core::domain::mock::decide",
        always: &[],
        bindings: &[
            bind("harness", FlagKind::Positional),
            bind("rules", FlagKind::Value("--rules")),
            bind("spyFile", FlagKind::Value("--spy-file")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "interrupt",
        argv: &["interrupt"],
        options: Some("interrupt_options"),
        stdout: StdoutShape::Json("interrupt_response"),
        stdin: false,
        rust: "oneharness_core::io::control::send",
        always: JSON_DOCUMENT,
        bindings: &[
            bind("session", FlagKind::Value("--session")),
            bind("input", FlagKind::Value("--input")),
            bind("sessionDir", FlagKind::Value("--session-dir")),
            bind("cwd", FlagKind::Value("--cwd")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "history",
        argv: &["history", "show"],
        options: Some("history_lookup"),
        stdout: StdoutShape::Json("history_show_entries"),
        stdin: false,
        rust: "oneharness_core::io::history::read_session_display",
        always: JSON_DOCUMENT,
        bindings: &[
            bind_prefer("session", FlagKind::Positional, "last"),
            bind("last", FlagKind::Switch("--last")),
            bind("all", FlagKind::Switch("--all")),
            bind_refuse("project", FlagKind::Value("--project"), "allProjects"),
            bind("allProjects", FlagKind::Switch("--all-projects")),
            bind("window", FlagKind::Window(HISTORY_WINDOW_FLAGS)),
            bind("historyDir", FlagKind::Value("--history-dir")),
            bind_refuse("config", FlagKind::Repeated("--config"), "noConfig"),
            bind("noConfig", FlagKind::Switch("--no-config")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "historyList",
        argv: &["history", "list"],
        options: Some("history_list_options"),
        stdout: StdoutShape::Json("history_list"),
        stdin: false,
        rust: "oneharness_core::io::history::list_sessions",
        always: JSON_DOCUMENT,
        bindings: &[
            bind("variant", FlagKind::Value("--variant")),
            bind_refuse("project", FlagKind::Value("--project"), "allProjects"),
            bind("allProjects", FlagKind::Switch("--all-projects")),
            bind("window", FlagKind::Window(HISTORY_WINDOW_FLAGS)),
            bind("historyDir", FlagKind::Value("--history-dir")),
            bind_refuse("config", FlagKind::Repeated("--config"), "noConfig"),
            bind("noConfig", FlagKind::Switch("--no-config")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "historyWatch",
        argv: &["history", "watch"],
        options: Some("history_watch_options"),
        stdout: StdoutShape::Jsonl("history_stream_envelope"),
        stdin: false,
        rust: "oneharness_core::io::history::HistoryWatcher",
        always: &["--format", "jsonl"],
        bindings: &[
            bind_refuse("after", FlagKind::Value("--after"), "window"),
            bind("labels", FlagKind::KeyValue("--label")),
            bind("variant", FlagKind::Value("--variant")),
            bind("session", FlagKind::Value("--session")),
            bind_refuse("project", FlagKind::Value("--project"), "allProjects"),
            bind("allProjects", FlagKind::Switch("--all-projects")),
            bind("historyDir", FlagKind::Value("--history-dir")),
            bind("events", FlagKind::Switch("--events")),
            bind("window", FlagKind::Window(HISTORY_WINDOW_FLAGS)),
            bind_refuse("config", FlagKind::Repeated("--config"), "noConfig"),
            bind("noConfig", FlagKind::Switch("--no-config")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "historyClear",
        argv: &["history", "clear"],
        options: Some("history_clear_options"),
        stdout: StdoutShape::Json("history_clear_report"),
        stdin: false,
        rust: "oneharness_core::io::history::remove_sessions",
        always: JSON_DOCUMENT,
        bindings: &[
            bind_refuse("project", FlagKind::Value("--project"), "allProjects"),
            bind("allProjects", FlagKind::Switch("--all-projects")),
            bind("yes", FlagKind::Switch("--yes")),
            bind("historyDir", FlagKind::Value("--history-dir")),
            bind_refuse("config", FlagKind::Repeated("--config"), "noConfig"),
            bind("noConfig", FlagKind::Switch("--no-config")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "historyMigrate",
        argv: &["history", "migrate"],
        options: Some("history_migrate_options"),
        stdout: StdoutShape::Json("history_migrate_report"),
        stdin: false,
        rust: "oneharness_core::io::history::migrate",
        always: JSON_DOCUMENT,
        bindings: &[
            bind("historyDir", FlagKind::Value("--history-dir")),
            bind_refuse("config", FlagKind::Repeated("--config"), "noConfig"),
            bind("noConfig", FlagKind::Switch("--no-config")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "historyReindex",
        argv: &["history", "reindex"],
        options: Some("history_reindex_options"),
        stdout: StdoutShape::Json("history_reindex_report"),
        stdin: false,
        rust: "oneharness_core::io::history::reindex",
        always: JSON_DOCUMENT,
        bindings: &[
            bind("historyDir", FlagKind::Value("--history-dir")),
            bind_refuse("config", FlagKind::Repeated("--config"), "noConfig"),
            bind("noConfig", FlagKind::Switch("--no-config")),
        ],
        uncovered: &[],
    },
    Capability {
        method: "historyPointers",
        argv: &["history", "pointers"],
        options: Some("history_pointers_options"),
        stdout: StdoutShape::Json("history_pointers"),
        stdin: false,
        rust: "oneharness_core::io::history::read_pointers",
        always: JSON_DOCUMENT,
        // The file is the verb's one positional; it reads no config, since the
        // pointer file is the caller's own artifact, not the store's.
        bindings: &[bind("file", FlagKind::Positional)],
        uncovered: &[],
    },
];
