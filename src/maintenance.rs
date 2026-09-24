//! The **pool-maintenance schedule**: a persistent schedule a launch names, run
//! by an idle driver over the sibling's worktree pool.
//!
//! `onevcs` keeps warm worktree slots per identity and, since 0.27.0, carries
//! `pool_maintain`: run the host's own `maintain` command in each idle slot and
//! stamp the attempt on the slot as `last_maintained`. That library holds **no
//! schedule** — when maintenance runs is the caller's — and this crate holds **no
//! state**: the schedule here is a pure function of a host file and the
//! sibling's recorded stamp, so a slot is due exactly when
//! `last_maintained < now − every`, and the sibling decides it. That is what makes
//! late fine, never twice, and two drivers on one host safe by the same fact: a
//! second driver meeting the first inside an identity is answered `Claimed`, and
//! one arriving after it is answered `NotDue`. Between runs nothing runs and
//! nothing needs to — a per-run timer would sweep on every run of a busy host
//! and never on a quiet one, and maintenance intervals are usually longer than a
//! DAG run.
//!
//! [`MaintenanceConfig`] is the file, handed to every launch the way
//! `--bus-config` is and retained in the launch record as its parsed document.
//! Its grammar is the sibling's: `every` is a `onevcs::Span` and `match` is a
//! `onevcs::rules::RuleMatch`, resolved through `onevcs::first_matching`, so the
//! span grammar and the match vocabulary have one implementation each and this
//! crate restates neither. `Sweep` is the **one** thread an idle driver starts:
//! it visits every registered identity in sorted order, resolves `every` (rule,
//! else default) and calls `onevcs::pool_maintain(Scope::Repo(identity),
//! Some(every))`. It is bounded by construction — every command runs under the
//! identity's own `timeout` — and a driver closing out joins it. Which identities
//! exist is [`onevcs::registered_identities`], the library form of the unindented
//! lines `onevcs repos` prints: the registry is keyed by normalized origin, so
//! what it answers is already the sorted order a sweep visits in, and a registry
//! this host cannot read is an error rather than a host with nothing registered.
//!
//! What reaches the run's record is **one** `pool-maintenance` entry per sweep
//! that did something — a slot ran, a due slot could not be maintained, an
//! identity was `Claimed`, or one failed — carrying per identity and slot what
//! ran and how it ended. A sweep on which every identity answered
//! `NoMaintainCommand`, `NoSlots`, `NotDue` or `InUse` writes nothing: those are
//! the schedule and the pool working, they are the common case on a schedule
//! measured in days, and a record of them would be a record of nothing.

use std::path::Path;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use onevcs::rules::RuleMatch;
use onevcs::{IdentityOutcome, Scope, SlotOutcome, Span};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};

use crate::engine::Message;
use crate::error::{Error, Result};
use crate::ledger::RunPaths;

/// The flag a launch names the schedule with.
pub const FLAG: &str = "--maintenance-config";

/// The launch-config key a launch names the schedule with.
pub const KEY: &str = "maintenance_config";

/// The one version of the schedule document this build reads.
pub const SCHEDULE_VERSION: u32 = 1;

/// The environment variable bounding how often an idle driver starts a sweep.
///
/// In-memory pacing and nothing more: what decides whether a slot is maintained
/// is the sibling's own stamp, so the pace only bounds how often an idle driver
/// asks. Read from the environment of the process driving the run, as every
/// other bound this crate takes is, so a journey about it can move it.
pub const PACE_ENV: &str = "ONEPIPELINE_MAINTENANCE_PACE_SECONDS";

/// How often an idle driver starts a sweep when nothing overrides it.
///
/// Ten minutes. A sweep on a schedule measured in days answers `NotDue` for
/// every slot almost every time it is asked, and each ask is a survey of every
/// identity's records on this host — cheap, but not free, and not worth paying
/// on every idle pass of a loop that otherwise waits on its channel alone.
pub const DEFAULT_PACE_SECONDS: u64 = 600;

/// How long a driver whose pace has come due but whose pass was not idle waits
/// before it asks itself again.
///
/// The bound every other answer the loop owes on a clock is held to: a driver
/// at its concurrency ceiling or on a host with no free slot re-asks on this
/// cadence rather than on every pass, and a dispatch settling wakes it sooner.
const RECHECK: Duration = Duration::from_secs(60);

/// The schedule document a launch names: how often each identity's idle slots
/// are maintained.
///
/// External input — a file an operator wrote — so it is versioned and closed:
/// an unknown key is refused by name rather than dropped, a `version` this
/// build does not read is refused by its number, a rule naming no match field
/// is refused as one, and a malformed span is refused by the sibling's own
/// grammar. All of it before a run is minted, so a launch that could not be
/// honoured never cuts a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceConfig {
    /// Schema version; [`SCHEDULE_VERSION`] is the one this build reads.
    // llmlint: ignore[invalid_states_unrepresentable] the number the document declares, as `LaunchConfig::schema_version` and `Plan::schema_version` carry theirs: the contract's own block fixes this field as `version: 1`, a refusal by number is the loader's — `load` turns down every other value before anything reads the document — and a version enum here would be a public shape the contract does not name, refusing a later document by failing to parse rather than by naming the number it declares.
    pub version: u32,
    /// The cadence every identity no rule matches is maintained on.
    pub default: MaintenanceDefault,
    /// The rules, first match wins. Omitted when empty, so a document naming
    /// none round-trips as the file wrote it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<MaintenanceRule>,
}

/// The cadence for every identity no rule matches.
///
/// `every` is the only key it carries: no `at`, no cron shape. A schedule whose
/// only state is the sibling's recorded stamp has nothing to say about *when*
/// beyond how long ago is too long.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceDefault {
    /// How old a slot's `last_maintained` may be before it is due, as the
    /// sibling spells a span: digits then `s`, `m`, `h` or `d`, nothing else.
    #[serde(deserialize_with = "every")]
    pub every: Span,
}

/// One rule: which identities it applies to, and their cadence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceRule {
    /// What this rule applies to, in the sibling's own rules vocabulary. At
    /// least one of its fields is named; the ones that are must all match.
    #[serde(rename = "match", deserialize_with = "matching")]
    pub r#match: RuleMatch,
    /// The cadence for the identities it matches.
    #[serde(deserialize_with = "every")]
    pub every: Span,
}

impl MaintenanceConfig {
    /// Read a schedule file: JSON, or the YAML the document is written in, of
    /// which JSON is a subset.
    ///
    /// `spelling` is what carried the path — the flag or the launch-config key
    /// — and every refusal opens with it and the path, so an operator reads
    /// which of the two documents to fix.
    ///
    /// # Errors
    ///
    /// [`Error::Ledger`] for a file that cannot be read, and [`Error::Invalid`]
    /// for a document this schema does not accept: an unknown key, a `version`
    /// other than [`SCHEDULE_VERSION`], a rule naming no match field, or a span
    /// the sibling's grammar refuses — each naming the key at fault.
    pub fn load(spelling: &str, path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Ledger {
            path: path.to_path_buf(),
            source,
        })?;
        let named = |why: String| Error::Invalid(format!("{spelling} {}: {why}", path.display()));
        let config: Self =
            serde_norway::from_str(&text).map_err(|failure| named(failure.to_string()))?;
        if config.version != SCHEDULE_VERSION {
            return Err(named(format!(
                "`version` is {}, and this build reads {SCHEDULE_VERSION} — set `version: \
                 {SCHEDULE_VERSION}`",
                config.version
            )));
        }
        Ok(config)
    }

    /// The cadence one identity is maintained on: its first matching rule's,
    /// else the default's.
    ///
    /// Matched by the sibling's own matcher over the sibling's own resolution of
    /// the identity, so `match:` here reads exactly as it does in that library's
    /// rules, releases and workspaces files.
    ///
    /// # Errors
    ///
    /// The sibling's own refusal, where `identity` resolves to nothing it has
    /// registered.
    pub fn every_for(&self, identity: &str) -> std::result::Result<Span, String> {
        let criteria: Vec<RuleMatch> = self.rules.iter().map(|rule| rule.r#match.clone()).collect();
        let matched =
            onevcs::first_matching(&criteria, identity).map_err(|failure| failure.to_string())?;
        Ok(matched.map_or(self.default.every, |index| self.rules[index].every))
    }
}

/// Read a span as the sibling's grammar reads one, refused by this key's name.
///
/// Through `Span`'s own `FromStr`, so what may spell a span is that library's
/// list and never a second one here; the refusal carries its sentence, which
/// ends with the grammar.
fn every<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Span, D::Error> {
    let value = Value::deserialize(deserializer)?;
    let text = match &value {
        Value::String(text) => text.as_str(),
        other => {
            return Err(serde::de::Error::custom(format!(
                "`every` holds {other}, which is not a span — write digits then one of s, m, h \
                 or d, such as 7d"
            )))
        }
    };
    text.parse::<Span>()
        .map_err(|why| serde::de::Error::custom(format!("`every`: {why}")))
}

/// Read a `match` as the sibling's own [`RuleMatch`], refusing a field it does
/// not have and a rule that names none.
///
/// The sibling's type drops a key it does not know rather than refusing it, so
/// the document's keys are compared against what that type kept: a key that
/// did not survive the round trip is one the vocabulary has no field for. That
/// asks the sibling which fields exist rather than keeping a list here.
fn matching<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<RuleMatch, D::Error> {
    let written = Map::<String, Value>::deserialize(deserializer)?;
    let read: RuleMatch = serde_json::from_value(Value::Object(written.clone()))
        .map_err(|why| serde::de::Error::custom(format!("`match`: {why}")))?;
    let kept = serde_json::to_value(&read)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if let Some(unknown) = written.keys().find(|key| !kept.contains_key(*key)) {
        let known: Vec<String> = kept.keys().cloned().collect();
        return Err(serde::de::Error::custom(format!(
            "`match` names `{unknown}`, which is not a field a match has{}",
            if known.is_empty() {
                String::new()
            } else {
                format!("; it names {}", known.join(", "))
            }
        )));
    }
    if kept.is_empty() {
        return Err(serde::de::Error::custom(
            "`match` names no field — a rule matches on at least one of the identity's host, \
             owner, name or path",
        ));
    }
    Ok(read)
}

/// The pace an idle driver starts sweeps at, from the environment or the default.
fn pace() -> Duration {
    Duration::from_secs(
        std::env::var(PACE_ENV)
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DEFAULT_PACE_SECONDS),
    )
}

/// Whether this host has room for a sweep: the local executor reports a free
/// slot.
///
/// The executor's own report, read the way `start_ready` reads it before a
/// dispatch — cores less the load, or what [`crate::executor::LOAD1_ENV`]
/// states for a host whose load average is not this process's measure.
pub(crate) fn host_has_room() -> bool {
    crate::executor::Executor::capacity(&crate::executor::LocalExecutor).slots_free > 0
}

/// One identity, as one sweep left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Maintained {
    /// The identity key.
    pub(crate) identity: String,
    /// The cadence it was maintained on.
    pub(crate) every: Span,
    /// What the sibling answered, or why it could not be asked.
    pub(crate) outcome: std::result::Result<IdentityOutcome, String>,
}

impl Maintained {
    fn is_recorded(&self) -> bool {
        match &self.outcome {
            Err(_) | Ok(IdentityOutcome::Claimed { .. }) => true,
            Ok(IdentityOutcome::Slots(slots)) => slots.iter().any(|slot| {
                matches!(
                    slot.outcome,
                    SlotOutcome::Ran { .. }
                        | SlotOutcome::Unavailable { .. }
                        | SlotOutcome::Broken { .. }
                )
            }),
            Ok(IdentityOutcome::NoMaintainCommand | IdentityOutcome::NoSlots) => false,
        }
    }

    /// The record's entry for this identity: the sibling's outcome as the
    /// sibling serializes it, or the reason there is none — an outcome the
    /// sibling's own `Serialize` refused is recorded as that refusal rather than
    /// dropped or panicked over.
    fn payload(&self) -> Value {
        let mut entry = json!({
            "identity": self.identity,
            "every": self.every.to_string(),
        });
        let answer = match &self.outcome {
            Ok(outcome) => serde_json::to_value(outcome)
                .map_err(|why| format!("the sibling's outcome could not be recorded: {why}")),
            Err(why) => Err(why.clone()),
        };
        match answer {
            Ok(outcome) => entry["outcome"] = outcome,
            Err(why) => entry["error"] = json!(crate::engine::bounded(&why)),
        }
        entry
    }
}

/// What one sweep did: every identity it visited, and how the enumeration went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Swept {
    /// When the sweep started, RFC3339, stamped by `sys::now_rfc3339` and carried
    /// to the record as the record's own document spells it.
    // llmlint: ignore[invalid_states_unrepresentable] the instant is a `String` on every record this crate writes — `LaunchRecord::started_at`, the envelope's `ts` — and on the sibling's `SlotStatus::last_maintained` beside it; the one writer is `sys::now_rfc3339`, and `payload::PoolMaintenance` declares the same shape.
    pub(crate) started_at: String,
    /// Every identity, in sorted order.
    pub(crate) identities: Vec<Maintained>,
    /// Why the identities could not be enumerated at all, where they could not.
    pub(crate) failure: Option<String>,
}

impl Swept {
    /// Whether this sweep is written into the record at all.
    fn is_recorded(&self) -> bool {
        self.failure.is_some() || self.identities.iter().any(Maintained::is_recorded)
    }

    /// The `pool-maintenance` record's payload.
    fn payload(&self) -> Map<String, Value> {
        let mut payload = crate::journal::payload(&[
            ("started_at", json!(self.started_at)),
            (
                "identities",
                Value::Array(
                    self.identities
                        .iter()
                        .filter(|identity| identity.is_recorded())
                        .map(Maintained::payload)
                        .collect(),
                ),
            ),
        ]);
        if let Some(failure) = &self.failure {
            payload.insert("error".to_owned(), json!(crate::engine::bounded(failure)));
        }
        payload
    }
}

fn identities() -> std::result::Result<Vec<String>, String> {
    onevcs::registered_identities()
        .map_err(|error| format!("the host's registered identities could not be read: {error}"))
}

/// Maintain every registered identity once, on the schedule.
fn sweep(config: &MaintenanceConfig) -> Swept {
    let started_at = crate::sys::now_rfc3339();
    let keys = match identities() {
        Ok(keys) => keys,
        Err(failure) => {
            return Swept {
                started_at,
                identities: Vec::new(),
                failure: Some(failure),
            }
        }
    };
    let identities = keys
        .into_iter()
        .map(|identity| {
            let every = match config.every_for(&identity) {
                Ok(every) => every,
                Err(why) => {
                    return Maintained {
                        identity,
                        every: config.default.every,
                        outcome: Err(why),
                    }
                }
            };
            let outcome = onevcs::pool_maintain(Scope::Repo(identity.clone()), Some(every))
                .map_err(|failure| failure.to_string())
                .and_then(|report| {
                    report
                        .identities
                        .into_iter()
                        .next()
                        .map(|maintained| maintained.outcome)
                        .ok_or_else(|| "the sibling answered for no identity".to_owned())
                });
            Maintained {
                identity,
                every,
                outcome,
            }
        })
        .collect();
    Swept {
        started_at,
        identities,
        failure: None,
    }
}

/// The one maintenance thread a driver runs at a time.
///
/// Started on an idle pass, it hands its [`Swept`] back over the loop's own
/// channel and is joined where the loop takes it up — or, where the loop ends
/// first, when this is dropped, which is what makes a driver closing out wait
/// for it. The join is bounded by construction: every command the sweep runs is
/// under its identity's own `timeout`.
///
/// While it is live the run root carries a marker, which is what lets `status`
/// — another process — say a maintenance is in progress.
pub(crate) struct Sweep {
    handle: Option<JoinHandle<()>>,
    paths: RunPaths,
}

impl Sweep {
    /// Start one sweep. `None` where this host will not start a thread, which
    /// is reported and costs nothing else: the schedule is asked again at the
    /// next pace.
    fn start(config: MaintenanceConfig, paths: &RunPaths, tx: Sender<Message>) -> Option<Self> {
        let started_at = crate::sys::now_rfc3339();
        // The marker before the thread, so no reader meets a live sweep with
        // nothing on disk saying so. A marker that could not be written costs
        // the sweep nothing: `status` then says nothing about it, which is the
        // safe direction.
        // llmlint: ignore-block[changed_behavior_has_e2e] no invocation a user can type
        // reaches a marker the run root refuses to take: the run's own journal is
        // written beside it by the same process a moment later, so a root that refuses
        // this write refuses the sweep's record too and the loop fails on *that*, with
        // its reason — which `driver.rs`'s unwritable-root journeys drive. What is
        // decided here is only that the marker is not what stops a sweep.
        let _ = crate::ledger::write_json(
            &paths.maintenance(),
            &json!({"started_at": started_at, "pid": crate::sys::pid()}),
        ); // llmlint: ignore-end[changed_behavior_has_e2e]
        let handle = std::thread::Builder::new()
            .name("pool-maintenance".to_owned())
            .spawn(move || {
                let swept = sweep(&config);
                // A loop that has gone has nobody to record it; the slots keep
                // their own stamps regardless.
                let _ = tx.send(Message::Maintained(Box::new(swept)));
            });
        match handle {
            Ok(handle) => {
                // Counted here rather than above, so the count is of sweeps this
                // driver **started**: a host that would not give it a thread did
                // not sweep, and the arm below says so. Still before the thread is
                // joined, because what a reader of the counts is asking is whether
                // the driver asked at all, not whether the asking has finished.
                crate::loopstats::maintenance_sweep_started();
                Some(Self {
                    handle: Some(handle),
                    paths: paths.clone(),
                })
            }
            // llmlint: ignore-block[changed_behavior_has_e2e] no invocation a user can
            // type reaches this arm: it is a host that will not start a thread at all,
            // which no plan, flag or environment of this crate's decides. What it does
            // when reached is the safe direction — no sweep, the marker taken back, and
            // the schedule asked again at the next pace.
            Err(error) => {
                eprintln!("onepipeline: cannot start the pool-maintenance sweep: {error}");
                let _ = std::fs::remove_file(paths.maintenance());
                None
            } // llmlint: ignore-end[changed_behavior_has_e2e]
        }
    }

    /// Wait for the thread, and take the marker back.
    fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let _ = std::fs::remove_file(self.paths.maintenance());
    }
}

impl Drop for Sweep {
    fn drop(&mut self) {
        self.join();
    }
}

/// What `status` says about a sweep in progress, or nothing.
///
/// Read off the marker the driver keeps while its sweep thread is live, and
/// only while something is driving the run: a marker a dead driver left names
/// a sweep nothing is running, and the driver that next adopts the run takes it
/// back before its first pass.
pub(crate) fn status_line(view: &crate::views::RunView) -> String {
    if view.liveness() == crate::views::DriverLiveness::Driving {
        if let Some(marker) = crate::ledger::read_json_opt::<Value>(&view.paths.maintenance()) {
            let since = marker["started_at"]
                .as_str()
                .unwrap_or("an unrecorded time");
            return format!(
                "  pool maintenance: a sweep of the host's worktree pools is in progress, \
                 started {since}\n"
            );
        }
    }
    String::new()
}

/// What `results` says about the last sweep that did something, or nothing.
///
/// The record's own words: each identity's outcome is read back through the
/// sibling's type, so what a slot's ending is called here is what `onevcs pool
/// maintain` calls it. Under the graph, because maintenance is the host's
/// rather than any node's.
pub(crate) fn results_lines(view: &crate::views::RunView) -> String {
    let Some(record) = view.events.iter().rev().find(|event| {
        crate::journal::PipelineKind::from_wire(&event.kind)
            == Some(crate::journal::PipelineKind::PoolMaintenance)
    }) else {
        return String::new();
    };
    let mut out = format!(
        "  pool maintenance: last sweep that did something started {}\n",
        record
            .payload
            .get("started_at")
            .and_then(Value::as_str)
            .map_or_else(
                || "at an unrecorded time".to_owned(),
                crate::views::one_line
            )
    );
    if let Some(error) = record.payload.get("error").and_then(Value::as_str) {
        out.push_str(&format!(
            "      the host's identities could not be enumerated: {}\n",
            crate::views::one_line(error)
        ));
    }
    for entry in record
        .payload
        .get("identities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let identity = entry
            .get("identity")
            .and_then(Value::as_str)
            .map_or_else(|| "an unnamed identity".to_owned(), crate::views::one_line);
        let every = entry
            .get("every")
            .and_then(Value::as_str)
            .map_or_else(|| "?".to_owned(), crate::views::one_line);
        let what = match (
            entry.get("error").and_then(Value::as_str),
            entry.get("outcome"),
        ) {
            (Some(error), _) => format!("failed — {}", crate::views::one_line(error)),
            (None, Some(outcome)) => {
                match serde_json::from_value::<IdentityOutcome>(outcome.clone()) {
                    Ok(outcome) => identity_phrase(&outcome),
                    Err(_) => "an outcome this build does not read".to_owned(),
                }
            }
            (None, None) => "no outcome recorded".to_owned(),
        };
        out.push_str(&format!("      {identity} (every {every}): {what}\n"));
    }
    out
}

/// One identity's outcome, in the sibling's own words.
fn identity_phrase(outcome: &IdentityOutcome) -> String {
    match outcome {
        IdentityOutcome::NoMaintainCommand => "no maintain command".to_owned(),
        IdentityOutcome::NoSlots => "no slots".to_owned(),
        IdentityOutcome::Claimed { by_pid } => {
            format!("claimed — another pool maintain (pid {by_pid}) was maintaining it")
        }
        IdentityOutcome::Slots(slots) => slots
            .iter()
            .map(|slot| format!("slot {} {}", slot.number, slot_phrase(&slot.outcome)))
            .collect::<Vec<_>>()
            .join("; "),
    }
}

/// One slot's outcome, in the sibling's own words.
///
/// **Busy is not broken.** A slot the sibling answers `unavailable` for is a
/// healthy one something else holds right now — a live maintenance claim, an
/// occupancy this run could not take, a process still working inside it — and a
/// later sweep finds it clear; `broken` is the slot whose clone, worktree or
/// record is not usable, which nothing waiting clears. So the two are phrased
/// apart here rather than both rendered as a reason a reader has to classify, and
/// a supervisor reading a busy pool is not told its worktrees are damaged.
///
/// Every arm is one the sibling can answer, and `tests::every_slot_outcome_has_a_phrase`
/// holds each spelling; `tests/e2e/maintenance.rs` drives the ones a journey can
/// produce — a command that succeeded, failed, and timed out, a slot whose
/// occupancy another process holds, and one whose worktree is gone — through
/// `results`.
// llmlint: ignore-block[changed_behavior_has_e2e] `in-use` and a command ended by a signal
// are answered by the sibling under conditions a journey cannot schedule from this
// crate's interface — a session opened into the slot between the survey and the claim, a
// process the host killed — and a fixture that forged the record would prove the
// fixture; each phrase is held by the unit test the doc names, over the sibling's own
// type. `unavailable` and `broken` are **not** in that list any more:
// `a_slot_another_process_holds_is_busy_and_one_whose_worktree_is_gone_is_broken` takes
// the slot's occupancy lease the way `session_reuse.rs` takes a run root's, and then
// removes the slot's worktree, driving the real binary into each.
fn slot_phrase(outcome: &SlotOutcome) -> String {
    match outcome {
        SlotOutcome::NotDue { last_maintained } => {
            format!(
                "not due, last maintained {}",
                crate::views::one_line(last_maintained)
            )
        }
        SlotOutcome::InUse { session } => {
            format!(
                "kept: session {} is working in it",
                crate::views::one_line(&session.0)
            )
        }
        SlotOutcome::Unavailable { holder } => {
            format!("kept: busy — {}", crate::views::one_line(holder))
        }
        SlotOutcome::Broken { reason } => {
            format!("kept: broken — {}", crate::views::one_line(reason))
        }
        SlotOutcome::Ran {
            outcome,
            duration_ms,
            log,
        } => {
            let ended = match outcome {
                onevcs::MaintenanceOutcome::Succeeded => "succeeded".to_owned(),
                onevcs::MaintenanceOutcome::Failed { exit: Some(exit) } => {
                    format!("failed (exit {exit})")
                }
                onevcs::MaintenanceOutcome::Failed { exit: None } => {
                    "failed (ended by a signal, or never started)".to_owned()
                }
                onevcs::MaintenanceOutcome::TimedOut => "timed out".to_owned(),
            };
            let log = log
                .as_ref()
                .map(|log| format!(", log {}", crate::views::one_line(&log.0)))
                .unwrap_or_default();
            format!("ran — {ended} in {duration_ms} ms{log}")
        }
    }
} // llmlint: ignore-end[changed_behavior_has_e2e]

/// The schedule as one driver runs it: when it last started a sweep, and the
/// sweep it is running now.
///
/// In-memory and this driver's alone. A fresh driver — an `adopt`, a relaunch —
/// starts with no last sweep and asks on its first idle pass, which is exactly
/// right: what stops it maintaining a slot twice is the slot's own stamp, and
/// never anything remembered here.
pub(crate) struct Maintenance {
    config: Option<MaintenanceConfig>,
    pace: Duration,
    last_started: Option<Instant>,
    sweep: Option<Sweep>,
}

impl Maintenance {
    /// The schedule the launch record names, or none.
    pub(crate) fn of_launch(config: Option<MaintenanceConfig>, paths: &RunPaths) -> Self {
        // A marker a driver that died left behind names a sweep nothing is
        // running: taken back here, before this driver's own first pass, so a
        // `status` between the two does not report the dead driver's.
        let _ = std::fs::remove_file(paths.maintenance());
        Self {
            config,
            pace: pace(),
            last_started: None,
            sweep: None,
        }
    }

    /// Whether the launch named a schedule at all.
    fn is_scheduled(&self) -> bool {
        self.config.is_some()
    }

    /// Start a sweep on this pass, where the pass was idle and one is due.
    ///
    /// **Idle** is the caller's to say — it dispatched nothing, nothing became
    /// ready, the run is below its own concurrency ceiling, and the host has
    /// room — and what this adds is the pacing: no sweep of this driver's is
    /// running, and the pace has elapsed since it last started one. A launch
    /// naming no schedule starts nothing, ever.
    pub(crate) fn consider(&mut self, idle: bool, paths: &RunPaths, tx: &Sender<Message>) {
        let Some(config) = &self.config else {
            return;
        };
        if !idle || self.sweep.is_some() || !crate::engine::due(self.last_started, self.pace) {
            return;
        }
        self.last_started = Some(Instant::now());
        self.sweep = Sweep::start(config.clone(), paths, tx.clone());
    }

    /// How long until this driver could next start a sweep, for the loop's wait.
    ///
    /// [`Duration::MAX`] where it never will — no schedule, or a sweep already
    /// running, whose ending is a message on the channel and wakes the wait by
    /// itself. A pace already due is re-asked on [`RECHECK`] rather than at
    /// once, because a pass that reached here with the pace due was not idle,
    /// and asking again on the next pass would be asking forty times a second.
    pub(crate) fn next_due(&self) -> Duration {
        if !self.is_scheduled() || self.sweep.is_some() {
            return Duration::MAX;
        }
        let until = self.last_started.map_or(Duration::ZERO, |last| {
            self.pace.saturating_sub(last.elapsed())
        });
        if until.is_zero() {
            RECHECK
        } else {
            until
        }
    }

    /// Take up a finished sweep: join its thread, take the marker back, and
    /// write the one record it earned, if it earned one.
    ///
    /// # Errors
    ///
    /// The reason the run's journal could not be written.
    pub(crate) fn record(
        &mut self,
        paths: &RunPaths,
        journal: &mut crate::journal::Journal,
        swept: &Swept,
    ) -> Result<()> {
        if let Some(mut sweep) = self.sweep.take() {
            sweep.join();
        }
        if let Some(failure) = &swept.failure {
            eprintln!("onepipeline: the pool-maintenance sweep could not enumerate this host's identities: {failure}");
        }
        if !swept.is_recorded() {
            return Ok(());
        }
        journal.emit(
            crate::journal::PipelineKind::PoolMaintenance,
            crate::journal::labels(&paths.run, None),
            swept.payload(),
        )
    }

    /// Wait for a sweep still running as the driver closes out, and record what
    /// it did.
    ///
    /// The thread hands its report over the loop's channel, which the loop has
    /// stopped reading; so it is joined here and the channel drained for its
    /// report. Nothing else can be queued there by then — the loop closes out
    /// with nothing in flight, and every dispatch thread has settled — so the
    /// drain takes the sweep's report and nothing of consequence with it.
    ///
    /// # Errors
    ///
    /// The reason the run's journal could not be written.
    pub(crate) fn close(
        mut self,
        paths: &RunPaths,
        journal: &mut crate::journal::Journal,
        rx: &std::sync::mpsc::Receiver<Message>,
    ) -> Result<()> {
        let Some(mut sweep) = self.sweep.take() else {
            return Ok(());
        };
        sweep.join();
        drop(sweep);
        let swept = rx.try_iter().find_map(|message| match message {
            Message::Maintained(swept) => Some(swept),
            _ => None,
        });
        match swept {
            Some(swept) => self.record(paths, journal, &swept),
            // llmlint: ignore[changed_behavior_has_e2e] unreachable by construction:
            // the thread sends its report before it ends, and it was joined above, so
            // the report is on the channel — nothing else drains it once the loop's own
            // reads have ended.
            None => Ok(()),
        }
    }

    /// Whether a sweep of this driver's is running now.
    #[cfg(test)]
    pub(crate) fn is_sweeping(&self) -> bool {
        self.sweep.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onevcs::{IdentityMaintenance, MaintenanceOutcome, SlotMaintenance};

    fn written(text: &str) -> Result<MaintenanceConfig> {
        static NTH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "onepipeline-maintenance-{}-{}",
            std::process::id(),
            NTH.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.join("maintenance.yml");
        std::fs::write(&path, text).expect("the schedule is written");
        let read = MaintenanceConfig::load(FLAG, &path);
        let _ = std::fs::remove_dir_all(&dir);
        read
    }

    fn refused(text: &str) -> String {
        match written(text) {
            Err(Error::Invalid(why)) => why,
            other => panic!("{text:?} was not refused as invalid: {other:?}"),
        }
    }

    /// The contract's own example parses through the sibling's types and comes
    /// back out as the file wrote it.
    #[test]
    fn the_schedule_reads_through_the_siblings_span_and_match_and_round_trips() {
        let config = written(
            "version: 1\ndefault:\n  every: 7d\nrules:\n  - match: {host: github.com, owner: \
             nickderobertis, name: onevcs}\n    every: 3d\n",
        )
        .expect("the schedule reads");
        assert_eq!(config.version, SCHEDULE_VERSION);
        assert_eq!(config.default.every.to_string(), "7d");
        assert_eq!(config.rules.len(), 1);
        assert_eq!(config.rules[0].every.to_string(), "3d");
        assert_eq!(
            config.rules[0].r#match,
            RuleMatch {
                host: Some("github.com".into()),
                owner: Some("nickderobertis".into()),
                name: Some("onevcs".into()),
                path: None,
            }
        );
        let text = serde_json::to_string(&config).expect("it serializes");
        assert_eq!(
            serde_json::from_str::<MaintenanceConfig>(&text).expect("it re-parses"),
            config
        );
        // A document naming no rule writes none.
        let bare = written("version: 1\ndefault:\n  every: 36h\n").expect("it reads");
        assert!(bare.rules.is_empty());
        assert_eq!(
            serde_json::to_string(&bare).expect("serializes"),
            r#"{"version":1,"default":{"every":"36h"}}"#
        );
    }

    /// Each refusal names the key at fault and the spelling that carried the
    /// path, before anything downstream reads the document.
    #[test]
    fn a_schedule_is_refused_by_the_key_at_fault() {
        let why = refused("version: 1\ndefault:\n  every: 7d\nat: \"03:00\"\n");
        assert!(why.starts_with(FLAG), "{why}");
        assert!(why.contains("unknown field `at`"), "{why}");

        let why = refused("version: 2\ndefault:\n  every: 7d\n");
        assert!(why.contains("`version` is 2"), "{why}");
        assert!(why.contains("this build reads 1"), "{why}");

        let why =
            refused("version: 1\ndefault:\n  every: 7d\nrules:\n  - match: {}\n    every: 1d\n");
        assert!(why.contains("`match` names no field"), "{why}");

        let why = refused(
            "version: 1\ndefault:\n  every: 7d\nrules:\n  - match: {repo: onevcs}\n    every: 1d\n",
        );
        assert!(why.contains("`match` names `repo`"), "{why}");

        let why = refused("version: 1\ndefault:\n  every: 7x\n");
        assert!(why.contains("`every`"), "{why}");
        assert!(why.contains("not a unit letter"), "{why}");

        let why = refused("version: 1\ndefault:\n  every: 7\n");
        assert!(why.contains("`every` holds 7"), "{why}");

        let why = refused("version: 1\ndefault:\n  every: 7d\n  at: never\n");
        assert!(why.contains("unknown field `at`"), "{why}");

        let why = refused("version: 1\ndefault:\n  every: 7d\nrules:\n  - every: 1d\n");
        assert!(why.contains("missing field `match`"), "{why}");
    }

    /// The pace is read from the environment as every other bound is, and an
    /// unusable value takes the default rather than a pace of nothing.
    #[test]
    fn the_pace_is_read_from_the_environment() {
        let _held = crate::vcs::scratch_home_held();
        for unusable in ["", "0", "-1", "soon"] {
            std::env::set_var(PACE_ENV, unusable);
            assert_eq!(
                pace(),
                Duration::from_secs(DEFAULT_PACE_SECONDS),
                "{PACE_ENV}={unusable:?}"
            );
        }
        std::env::set_var(PACE_ENV, "7");
        assert_eq!(pace(), Duration::from_secs(7));
        std::env::remove_var(PACE_ENV);
        assert_eq!(pace(), Duration::from_secs(DEFAULT_PACE_SECONDS));
    }

    fn ran(identity: &str) -> Maintained {
        Maintained {
            identity: identity.into(),
            every: "7d".parse().expect("a span"),
            outcome: Ok(IdentityOutcome::Slots(vec![
                SlotMaintenance {
                    number: 1,
                    outcome: SlotOutcome::Ran {
                        outcome: MaintenanceOutcome::Succeeded,
                        duration_ms: 12,
                        log: None,
                    },
                },
                SlotMaintenance {
                    number: 2,
                    outcome: SlotOutcome::NotDue {
                        last_maintained: "2026-09-19T00:00:00.000Z".into(),
                    },
                },
            ])),
        }
    }

    fn quiet(identity: &str, outcome: IdentityOutcome) -> Maintained {
        Maintained {
            identity: identity.into(),
            every: "7d".parse().expect("a span"),
            outcome: Ok(outcome),
        }
    }

    /// A sweep on which nothing was due and nothing was wrong writes nothing; one
    /// on which a slot ran, a due slot could not be maintained, an identity was
    /// claimed or one failed writes one record carrying exactly those identities,
    /// each with the sibling's own outcome shape.
    #[test]
    fn a_sweep_is_recorded_where_something_ran_or_a_due_slot_could_not_and_never_otherwise() {
        let nothing = Swept {
            started_at: "2026-09-20T00:00:00.000Z".into(),
            identities: vec![
                quiet("a", IdentityOutcome::NoMaintainCommand),
                quiet("b", IdentityOutcome::NoSlots),
                quiet(
                    "c",
                    IdentityOutcome::Slots(vec![
                        SlotMaintenance {
                            number: 1,
                            outcome: SlotOutcome::NotDue {
                                last_maintained: "2026-09-19T00:00:00.000Z".into(),
                            },
                        },
                        SlotMaintenance {
                            number: 2,
                            outcome: SlotOutcome::InUse {
                                session: onevcs::SessionToken("s-working".into()),
                            },
                        },
                    ]),
                ),
            ],
            failure: None,
        };
        assert!(!nothing.is_recorded());

        // A **due** slot the sweep could not maintain is news on its own, with
        // nothing beside it that ran: something else held it, or it is not
        // usable.
        for kept in [
            SlotOutcome::Unavailable {
                holder: "a command is working in it right now".into(),
            },
            SlotOutcome::Broken {
                reason: "its worktree is not a repository".into(),
            },
        ] {
            let held = Swept {
                started_at: "2026-09-20T00:00:00.000Z".into(),
                identities: vec![quiet(
                    "a",
                    IdentityOutcome::Slots(vec![SlotMaintenance {
                        number: 1,
                        outcome: kept.clone(),
                    }]),
                )],
                failure: None,
            };
            assert!(held.is_recorded(), "{kept:?} was not recorded");
        }

        let something = Swept {
            started_at: "2026-09-20T00:00:00.000Z".into(),
            identities: vec![
                quiet("a", IdentityOutcome::NoMaintainCommand),
                ran("b"),
                quiet("c", IdentityOutcome::Claimed { by_pid: 42 }),
                Maintained {
                    identity: "d".into(),
                    every: "1d".parse().expect("a span"),
                    outcome: Err("the registry could not be read".into()),
                },
            ],
            failure: None,
        };
        assert!(something.is_recorded());
        let payload = Value::Object(something.payload());
        assert_eq!(payload["started_at"], "2026-09-20T00:00:00.000Z");
        assert!(payload.get("error").is_none());
        let identities = payload["identities"].as_array().expect("an array");
        assert_eq!(
            identities
                .iter()
                .map(|entry| entry["identity"].as_str().expect("a key"))
                .collect::<Vec<_>>(),
            ["b", "c", "d"],
            "{payload}"
        );
        assert_eq!(identities[0]["every"], "7d");
        assert_eq!(
            identities[0]["outcome"]["slots"][0]["outcome"]["ran"]["outcome"],
            "succeeded"
        );
        assert_eq!(
            identities[0]["outcome"]["slots"][1]["outcome"]["not-due"]["last_maintained"],
            "2026-09-19T00:00:00.000Z"
        );
        assert_eq!(identities[1]["outcome"]["claimed"]["by_pid"], 42);
        assert_eq!(identities[2]["error"], "the registry could not be read");
        assert!(identities[2].get("outcome").is_none());
        // The sibling's shape, as the sibling reads it back.
        let read: IdentityOutcome = serde_json::from_value(identities[0]["outcome"].clone())
            .expect("the sibling reads its own outcome back");
        assert_eq!(read, ran("b").outcome.expect("an outcome"));
        let _ = IdentityMaintenance {
            identity: "b".into(),
            outcome: read,
        };

        let failed = Swept {
            started_at: "2026-09-20T00:00:00.000Z".into(),
            identities: Vec::new(),
            failure: Some("the host's registered identities could not be read".into()),
        };
        assert!(failed.is_recorded());
        let payload = Value::Object(failed.payload());
        assert_eq!(
            payload["error"],
            "the host's registered identities could not be read"
        );
        assert_eq!(payload["identities"], json!([]));
    }

    /// Every outcome the sibling can answer for a slot or an identity has a
    /// phrase of its own in the sibling's own words, and none is left as the
    /// unreadable case.
    #[test]
    fn every_slot_outcome_has_a_phrase() {
        let phrases = [
            (
                SlotOutcome::NotDue {
                    last_maintained: "2026-09-19T00:00:00.000Z".into(),
                },
                "not due, last maintained 2026-09-19T00:00:00.000Z",
            ),
            (
                SlotOutcome::InUse {
                    session: onevcs::SessionToken("s-abc".into()),
                },
                "kept: session s-abc is working in it",
            ),
            (
                SlotOutcome::Unavailable {
                    holder: "a command is working in it right now".into(),
                },
                "kept: busy — a command is working in it right now",
            ),
            (
                SlotOutcome::Broken {
                    reason: "its record could not be read".into(),
                },
                "kept: broken — its record could not be read",
            ),
            (
                SlotOutcome::Ran {
                    outcome: MaintenanceOutcome::Succeeded,
                    duration_ms: 12,
                    log: Some(onevcs::ArtifactId("a-1".into())),
                },
                "ran — succeeded in 12 ms, log a-1",
            ),
            (
                SlotOutcome::Ran {
                    outcome: MaintenanceOutcome::Failed { exit: Some(3) },
                    duration_ms: 12,
                    log: None,
                },
                "ran — failed (exit 3) in 12 ms",
            ),
            (
                SlotOutcome::Ran {
                    outcome: MaintenanceOutcome::Failed { exit: None },
                    duration_ms: 12,
                    log: None,
                },
                "ran — failed (ended by a signal, or never started) in 12 ms",
            ),
            (
                SlotOutcome::Ran {
                    outcome: MaintenanceOutcome::TimedOut,
                    duration_ms: 1_000,
                    log: None,
                },
                "ran — timed out in 1000 ms",
            ),
        ];
        for (outcome, phrase) in phrases {
            assert_eq!(slot_phrase(&outcome), phrase);
        }
        assert_eq!(
            identity_phrase(&IdentityOutcome::Claimed { by_pid: 7 }),
            "claimed — another pool maintain (pid 7) was maintaining it"
        );
        assert_eq!(
            identity_phrase(&IdentityOutcome::NoMaintainCommand),
            "no maintain command"
        );
        assert_eq!(identity_phrase(&IdentityOutcome::NoSlots), "no slots");
        assert_eq!(
            identity_phrase(&IdentityOutcome::Slots(vec![
                SlotMaintenance {
                    number: 1,
                    outcome: SlotOutcome::NotDue {
                        last_maintained: "t".into()
                    },
                },
                SlotMaintenance {
                    number: 2,
                    outcome: SlotOutcome::Broken { reason: "r".into() },
                },
            ])),
            "slot 1 not due, last maintained t; slot 2 kept: broken — r"
        );
    }

    /// A slot the sibling reports **busy** reads as a slot something holds, and a
    /// stored record written before that vocabulary existed still reads.
    ///
    /// Two facts about the same boundary. `unavailable` is the transient state —
    /// a live claim, an occupancy, a process inside the slot — that a later sweep
    /// finds clear, and reporting it as breakage is what would send a supervisor
    /// looking for a damaged worktree on a merely busy host; so the two phrases
    /// are held apart here rather than only held to exist. And the payload a
    /// driver wrote at the previous pin carries only the variants that pin had, so
    /// this parses one of those documents verbatim — no round trip through
    /// today's type, which would prove the type and not the record — and reads its
    /// phrase, because `results` renders a run's stored history and a record it
    /// cannot read renders as an outcome this build does not read.
    #[test]
    fn busy_is_not_broken_and_a_record_written_before_the_vocabulary_still_reads() {
        let busy = slot_phrase(&SlotOutcome::Unavailable {
            holder: "a maintenance run this one did not start (pid 7) has claimed it since t"
                .to_owned(),
        });
        let broken = slot_phrase(&SlotOutcome::Broken {
            reason: "its worktree is not a repository".to_owned(),
        });
        assert!(busy.contains("busy"), "{busy}");
        assert!(!busy.contains("broken"), "{busy}");
        assert!(broken.contains("broken"), "{broken}");
        assert_ne!(busy, broken);

        // The `outcome` object of a `pool-maintenance` record as a driver at the
        // previous pin wrote it: every variant that pin could answer, and none of
        // the one it could not.
        let stored = json!({
            "slots": [
                {"number": 1, "outcome": {"ran": {
                    "outcome": "succeeded", "duration_ms": 12, "log": "a-1"
                }}},
                {"number": 2, "outcome": {"not-due": {"last_maintained": "2026-09-19T00:00:00.000Z"}}},
                {"number": 3, "outcome": {"in-use": {"session": "s-old"}}},
                {"number": 4, "outcome": {"broken": {"reason": "its record could not be read"}}},
            ]
        });
        let read: IdentityOutcome =
            serde_json::from_value(stored).expect("a record written before this change still reads");
        assert_eq!(
            identity_phrase(&read),
            "slot 1 ran — succeeded in 12 ms, log a-1; \
             slot 2 not due, last maintained 2026-09-19T00:00:00.000Z; \
             slot 3 kept: session s-old is working in it; \
             slot 4 kept: broken — its record could not be read"
        );

        // And one written by this build, read back the same way.
        let now = json!({"slots": [
            {"number": 1, "outcome": {"unavailable": {"holder": "a command is working in it right now"}}}
        ]});
        let read: IdentityOutcome =
            serde_json::from_value(now).expect("the vocabulary this build writes reads back");
        assert_eq!(
            identity_phrase(&read),
            "slot 1 kept: busy — a command is working in it right now"
        );
    }

    /// A driver with no schedule starts nothing and waits on nothing; one with a
    /// schedule waits until its pace, and — where the pace is due but the pass
    /// was not idle — asks again on the recheck rather than at once.
    #[test]
    fn the_pace_decides_the_wait_and_only_an_idle_pass_starts_a_sweep() {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-maintenance-pace-{}",
            std::process::id()
        ));
        let paths = RunPaths::under(&root, "paced");
        let (tx, _rx) = std::sync::mpsc::channel();

        let none = Maintenance::of_launch(None, &paths);
        assert_eq!(none.next_due(), Duration::MAX);
        assert!(!none.is_sweeping());

        let config = MaintenanceConfig {
            version: SCHEDULE_VERSION,
            default: MaintenanceDefault {
                every: "7d".parse().expect("a span"),
            },
            rules: Vec::new(),
        };
        let mut scheduled = Maintenance::of_launch(Some(config), &paths);
        scheduled.pace = Duration::from_secs(3_600);
        // Due now, and the pass was not idle: nothing starts, and the wait is
        // the recheck rather than zero.
        assert_eq!(scheduled.next_due(), RECHECK);
        scheduled.consider(false, &paths, &tx);
        assert!(!scheduled.is_sweeping());
        assert_eq!(scheduled.next_due(), RECHECK);
        // Started once: the wait is then the pace, and a second idle pass
        // inside it starts nothing more.
        scheduled.last_started = Some(Instant::now());
        scheduled.consider(true, &paths, &tx);
        assert!(!scheduled.is_sweeping());
        let until = scheduled.next_due();
        assert!(
            until > Duration::from_secs(3_500) && until <= Duration::from_secs(3_600),
            "{until:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
