//! Fallback run mode: drive the selected harnesses in **priority order**,
//! stopping at the first one that actually runs the task — success *or* real
//! failure — and falling through only the ones that could not run at all. The
//! point is graceful degradation across a set of harnesses a repo declares it
//! supports: whichever one the user has installed and authenticated runs, and a
//! genuine task failure (a non-zero result, a timeout) is *never* mistaken for
//! "try the next harness".
//!
//! This module is pure policy only: the [`RunMode`] knob (accepted on the CLI
//! and in config) and the classification of one harness's outcome as a *startup
//! failure* (fall through) versus a *real run* (stop here). The command layer
//! owns the sequential spawning; keeping the order and the stop/continue verdict
//! here makes both unit-testable against the mock harness.
//!
//! The verdict is a function of the finished [`RunResult`] alone — status,
//! classified `failure_kind`, and [`RunWork`] evidence — so it is the same
//! whether the chain was driven buffered or under `run --stream`. There is no
//! streaming-specific rule.

use serde::{Deserialize, Serialize};

use crate::domain::report::{RunResult, Status};
use crate::domain::signals::FailureKind;

/// How the selected harnesses are run. Accepted as a CLI value (`--run-mode`,
/// parsed in the `oneharness` binary) and a config-file value (`run_mode`, via
/// `Deserialize`); the CLI parsing lives in the binary so this core crate stays
/// free of `clap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    /// Run every selected harness at once, each an independent subprocess, and
    /// report them all. The opt-in (`--run-mode parallel`, `run_mode =
    /// "parallel"`, `ONEHARNESS_RUN_MODE=parallel`); it was the default before
    /// the fallback chain became one.
    Parallel,
    /// Run the selected harnesses in priority order (the `--harness` / config
    /// order, else registry order under `--all`), stopping at the first that
    /// actually runs the task; fall through only the candidates that cannot run
    /// at all (see [`startup_failure_reason`]). The default wherever a run mode
    /// is left unset — in the request, every config layer and the environment.
    Fallback,
}

/// The mode an unset selection resolves to, on every surface — a library
/// caller whose request and config layers leave it unset, the CLI, and the
/// SDKs. This impl is the ONE place that decides it: `io::run` resolves an
/// unset mode from it and `config::explain` reports the same value from it,
/// so the run and the `config` verb's explanation cannot drift apart.
impl Default for RunMode {
    fn default() -> Self {
        RunMode::Fallback
    }
}

impl RunMode {
    /// Every mode, for the CLI's possible-value list.
    pub const ALL: [RunMode; 2] = [RunMode::Parallel, RunMode::Fallback];

    /// The CLI/JSON token for this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::Parallel => "parallel",
            RunMode::Fallback => "fallback",
        }
    }

    /// Parse a CLI/config token into a mode; `None` for an unrecognized token.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "parallel" => Some(RunMode::Parallel),
            "fallback" => Some(RunMode::Fallback),
            _ => None,
        }
    }
}

/// Whether a candidate's normalized result carries **evidence it did the task's
/// work** — the first thing [`startup_failure_reason`] consults.
///
/// Two independent witnesses, either of which is decisive:
///
/// - **Tool events.** A recorded tool call is the harness acting on the task.
/// - **Usage accounting.** [`Usage::reports_billed_work`][billed] — the same definition
///   `signals::record_work_evidence` reads a raw harness record with, and it answers
///   in this same type, so the two readings of "billed" are one contract with one
///   implementation.
///
/// It is also a **published reading**, not only an internal one: a run that
/// failed with nothing to classify carries it as [`RunResult::work`] and in its
/// history record, because there the question "did this candidate do anything?"
/// is the only one left. One type for both, so the value a reader sees is the
/// same value the fall-through verdict consulted.
///
/// [billed]: crate::domain::signals::Usage::reports_billed_work
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RunWork {
    /// The candidate ran the task, whatever its terminal record then said.
    Done,
    /// Nothing says the candidate got as far as doing work.
    None,
}

impl RunWork {
    /// Every reading, for the schema/test surfaces that enumerate them.
    pub const ALL: [RunWork; 2] = [RunWork::Done, RunWork::None];

    /// The JSON token this reading serializes to.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RunWork::Done => "done",
            RunWork::None => "none",
        }
    }

    /// Read the evidence off a finished result. A `spawn_error` nulls every
    /// signal by contract, so its evidence is always [`RunWork::None`].
    pub fn from_result(result: &RunResult) -> Self {
        // Tool activity only: an agent `message` is the same text a refusal
        // (Claude's quota response) is delivered as, so it is not work.
        let used_tools = result.events.as_ref().is_some_and(|events| {
            events
                .iter()
                .any(crate::domain::events::ActionEvent::is_tool_activity)
        });
        // Refusal text is not work (Claude's quota response is normalized into
        // `text`), but a Codex overload after an agent message is: Codex's
        // terminal overload object itself is not normalized as answer text.
        let overloaded_after_answer = result.failure_kind == Some(FailureKind::ServerOverloaded)
            && result.text.as_ref().is_some_and(|text| !text.is_empty());
        if used_tools || result.usage.reports_billed_work() || overloaded_after_answer {
            RunWork::Done
        } else {
            RunWork::None
        }
    }
}

/// Why a candidate fell through — the closed set [`startup_failure_reason`]
/// decides and [`crate::domain::report::FallThrough`] reports.
///
/// A type rather than a token, because the set is closed and every reader
/// downstream branches on it: a value no classifier produced cannot be built,
/// and the JSON spelling below is the schema's rather than each call site's. A
/// new variant is a report `schema_version` bump like any other enum value, since
/// a consumer matching exhaustively learns of it only from the version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FallThroughReason {
    /// The binary was not on PATH.
    NotInstalled,
    /// It resolved but could not be executed.
    SpawnError,
    /// This identity is not authenticated.
    Auth,
    /// This account is out of quota or credits.
    Quota,
    /// The session asked for does not exist for this identity.
    SessionNotFound,
    /// The harness refused the working directory before making the request.
    UntrustedDirectory,
    /// The harness refused the input's size before making the request.
    InputTooLarge,
    /// The harness reported it would run under a model other than the one
    /// requested, so the turn was refused before it started.
    ModelMismatch,
    /// This model is unavailable — on a model fan-out, or on any chain when the
    /// harness said the model is not available to the account it is signed in
    /// with (Codex's `400 invalid_request_error` "… is not supported when using
    /// Codex with a ChatGPT account").
    ModelNotFound,
    /// This candidate is rate limited and cannot serve the request right now.
    RateLimit,
    /// Codex serving capacity remained exhausted after local retries.
    ServerOverloaded,
    /// The shared Codex app-server went away under the candidate's live turn.
    AppServerLost,
}

impl FallThroughReason {
    /// The JSON token this reason serializes to, for the diagnostics and tests
    /// that need the raw string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FallThroughReason::NotInstalled => "not-installed",
            FallThroughReason::SpawnError => "spawn-error",
            FallThroughReason::Auth => "auth",
            FallThroughReason::Quota => "quota",
            FallThroughReason::SessionNotFound => "session-not-found",
            FallThroughReason::UntrustedDirectory => "untrusted-directory",
            FallThroughReason::InputTooLarge => "input-too-large",
            FallThroughReason::ModelMismatch => "model-mismatch",
            FallThroughReason::ModelNotFound => "model-not-found",
            FallThroughReason::RateLimit => "rate-limit",
            FallThroughReason::ServerOverloaded => "server-overloaded",
            FallThroughReason::AppServerLost => "app-server-lost",
        }
    }
}

/// Why a harness **could not run the task at all** — the conditions that make a
/// fallback run fall through to the next candidate. Returns the reason when the
/// outcome is a startup failure, or `None` when the harness *did* run (so a
/// fallback run stops there).
///
/// [`RunWork::Done`] short-circuits every reason below: falling through a
/// candidate that already worked burns the next one's quota re-running what
/// happened. This is the "work done, not error text" rule the Claude
/// session-limit classifier applies (issue #1211), lifted to the whole verdict so
/// it also covers the surfaces with no accounting of their own to gate on (a
/// generic `401` scanned out of a transcript, Codex's `turn.failed` usage limit).
///
/// With no work evidence the reasons are:
///
/// - [`Status::Skipped`] → `"not-installed"` (the binary was not on PATH).
/// - [`Status::SpawnError`] → `"spawn-error"` (resolved but could not execute).
/// - [`Status::Nonzero`], [`Status::Ok`] or [`Status::Skipped`] with `failure_kind == "auth"` →
///   `"auth"` (rejected before doing any work — bad/absent credentials). A
///   classified `auth` outranks the skip reason: a candidate the command layer
///   declined to run because the identity it selects is unprovisioned (an
///   `env_from` home directory that is not on disk) reads as the credential
///   problem it is, not as a missing binary. [`Status::Skipped`] is listed only
///   for `auth`, because that refusal is the only one decided before a spawn;
///   every other kind is read out of a process that ran.
/// - [`Status::Nonzero`] or [`Status::Ok`] with `failure_kind == "quota"` → `"quota"` (the account
///   has no credit/quota to do work — a provisioning problem like `auth`).
/// - [`Status::Nonzero`] with `failure_kind == "session_not_found"` →
///   `"session-not-found"` (the run asked to continue a session this identity's
///   store has never seen, so it refused before doing anything). Non-zero only,
///   unlike `quota`: every CLI observed refusing an unknown session exits 1, and
///   the classifier reads that refusal only off a non-zero run — a clean-exit arm
///   would be behavior no harness can produce. It belongs with
///   `auth` and `quota` for the same reason: the *task* is fine and the next
///   candidate can still do it.
/// - [`Status::Nonzero`] or [`Status::Ok`] with `failure_kind == "rate_limit"` →
///   `"rate-limit"` (the provider will not serve *this identity* right now). It
///   belongs beside `quota` for the same reason `quota` is there: the task is
///   fine and the next candidate — a different identity, with its own limit —
///   can still do it. It is not gated on a model list, because the limit belongs
///   to whoever is being billed rather than to the model. A clean exit counts
///   exactly as it does for `quota`: a harness that reports a provider's `429`
///   in its terminal record and exits 0 has still been refused, and the work
///   reading is what separates that refusal from a completed run — a record
///   that billed for the task has [`RunWork::Done`], which short-circuits above
///   (issue #1297: this arm was non-zero only, so a clean-exit rate limit ended
///   a chain on an identity whose neighbour had quota).
/// - [`Status::Nonzero`] with `failure_kind == "untrusted_directory"` →
///   `"untrusted-directory"`, and with `"input_too_large"` →
///   `"input-too-large"`: the two **precondition** refusals
///   ([`signals::precondition_refusal`][pre]), where the harness's own check
///   failed before the request was made — no model was called, no tokens spent.
///   Both meet this function's criterion exactly, and both are things another
///   candidate may not fail: a different identity's trust list may cover the
///   directory, and a different model's window may hold the input. Non-zero
///   only, for [`FailureKind::SessionNotFound`]'s reason: both refusals were
///   captured off a CLI that exited 1, and a clean-exit arm would be behavior no
///   harness has been observed producing.
///
/// [pre]: crate::domain::signals
/// [cbr]: crate::domain::report::completed_run_that_did_work
///
/// Everything else is a **real run**, so `None`: a clean [`Status::Ok`]; a
/// [`Status::Timeout`] (a genuine, if slow, run — falling through it would let a
/// long real run masquerade as a setup problem); a [`Status::Cancelled`] one
/// (nothing about the candidate failed, and falling through would spawn the very
/// next harness the caller just cancelled); a plain non-zero task failure;
/// and — by default — a non-zero classified `model_not_found` (a configuration
/// mistake the user should see, not silently route around). A [`Status::Planned`]
/// dry-run row is not a run at all and is `None` too (the fallback driver never
/// executes under `--print-command`).
///
/// **An unclassified failure stops the chain whatever its work reading** — a
/// decision, not an oversight. With `failure_kind == None` the crate cannot
/// name why the candidate failed, and [`RunWork::None`] does not fill that gap:
/// it is the *absence* of work evidence, not proof nothing was spent. A harness
/// killed at its deadline mid-call, or one that crashed after its request went
/// out, leaves the same empty accounting as a launcher shim that never found
/// the binary, and this function cannot tell them apart. Handing such a
/// candidate on would re-bill the first of those on the next identity, and
/// would re-run a task that hangs once per identity in the chain. So the
/// fall-through set is exactly the refusals a classifier decided from the
/// harness's own words — the ones known to have been raised *before* a request
/// was made — and a zero-work failure with a recognizable phrasing joins it by
/// being taught to [`signals`][crate::domain::signals], never by inference from
/// empty accounting. The cost of stopping is borne by the report instead: the
/// stop is attributed ([`FallbackReport::stopped_without_work`][swo]) and the
/// failure summary carries the candidate's own words, so the reader left
/// holding it can see what was untried and why.
///
/// [swo]: crate::domain::report::FallbackReport::stopped_without_work
///
/// `model_fallback` widens the fall-through set for a run that is **trying
/// several models in priority order** (`--model` given more than once, or config
/// `models`). There, a per-model rejection *is* the signal to try the next
/// candidate: `model_not_found` (this model is unavailable — reason
/// `"model-not-found"`) falls through, since the point of a model list is
/// graceful degradation across models exactly as the harness list degrades
/// across harnesses. With a single model (`model_fallback == false`) the
/// historical rule stands: it stops the chain.
///
/// A model the harness's *account* may not use is decided by
/// [`account_model_refusal_reason`] instead, on any chain.
pub fn startup_failure_reason(
    status: Status,
    failure_kind: Option<FailureKind>,
    model_fallback: bool,
    work: RunWork,
) -> Option<FallThroughReason> {
    if work == RunWork::Done {
        return None;
    }
    match (status, failure_kind) {
        (_, Some(FailureKind::Auth))
            if matches!(status, Status::Ok | Status::Nonzero | Status::Skipped) =>
        {
            Some(FallThroughReason::Auth)
        }
        (_, Some(FailureKind::Quota)) if matches!(status, Status::Ok | Status::Nonzero) => {
            Some(FallThroughReason::Quota)
        }
        (Status::Nonzero | Status::Ok, Some(FailureKind::RateLimit)) => {
            Some(FallThroughReason::RateLimit)
        }
        (Status::Nonzero | Status::Ok, Some(FailureKind::ServerOverloaded)) => {
            Some(FallThroughReason::ServerOverloaded)
        }
        // The shared server under the turn crashed or restarted: the candidate
        // is fine and so is the task, so the next candidate takes it. Read off a
        // non-zero result only, because the pooled driver is its one producer
        // and always reports the lost turn as one.
        (Status::Nonzero, Some(FailureKind::AppServerLost)) => {
            Some(FallThroughReason::AppServerLost)
        }
        (Status::Nonzero, Some(FailureKind::SessionNotFound)) => {
            Some(FallThroughReason::SessionNotFound)
        }
        (Status::Nonzero, Some(FailureKind::UntrustedDirectory)) => {
            Some(FallThroughReason::UntrustedDirectory)
        }
        (Status::Nonzero, Some(FailureKind::InputTooLarge)) => {
            Some(FallThroughReason::InputTooLarge)
        }
        // The third precondition: the harness said which model the thread would
        // run under before any token was spent, and it was not the one asked
        // for. No work was done, so the next identity — which may honour the
        // model — gets the task on ANY chain, not only a model fan-out.
        (Status::Nonzero, Some(FailureKind::ModelMismatch)) => {
            Some(FallThroughReason::ModelMismatch)
        }
        (Status::Skipped, _) => Some(FallThroughReason::NotInstalled),
        (Status::SpawnError, _) => Some(FallThroughReason::SpawnError),
        (Status::Nonzero, _) => match failure_kind {
            // Only when a model list is being tried: an unusable model means
            // "try the next model", not "stop with a real failure". With one
            // model it is a configuration mistake the user should see.
            Some(FailureKind::ModelNotFound) if model_fallback => {
                Some(FallThroughReason::ModelNotFound)
            }
            _ => None,
        },
        (Status::Ok | Status::Timeout | Status::Cancelled | Status::Planned, _) => None,
    }
}

/// The verdict for a candidate whose harness refused the model **for the
/// account it is signed in with** ([`signals::model_refused_for_account`][mra]),
/// which the command layer consults in place of [`startup_failure_reason`] for
/// that refusal alone.
///
/// The refusal is the identity's, not the configuration's, so the next
/// candidate — another harness, another account — can still run the task: it
/// hands on as `"model-not-found"` on any chain, model list or not. A clean
/// exit counts as a non-zero one, as it does for `quota`, because Codex reports
/// the refused turn in a `turn.failed` record and can still exit 0. Every other
/// `model_not_found` keeps [`startup_failure_reason`]'s rule unchanged. Work
/// evidence still short-circuits, and a timeout or cancellation is still a run.
///
/// [mra]: crate::domain::signals::model_refused_for_account
#[must_use]
pub fn account_model_refusal_reason(status: Status, work: RunWork) -> Option<FallThroughReason> {
    (work == RunWork::None && matches!(status, Status::Nonzero | Status::Ok))
        .then_some(FallThroughReason::ModelNotFound)
}

/// Whether `status`/`failure_kind` mean the harness could not run the task at
/// all, so a fallback run should try the next candidate. The boolean face of
/// [`startup_failure_reason`].
pub fn is_startup_failure(
    status: Status,
    failure_kind: Option<FailureKind>,
    model_fallback: bool,
    work: RunWork,
) -> bool {
    startup_failure_reason(status, failure_kind, model_fallback, work).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::signals::Usage;

    #[test]
    fn an_unset_run_mode_resolves_to_fallback() {
        // The user's contract: fallback is the default, parallel the opt-in.
        // Both resolution sites read this impl, so this is the one pin.
        assert_eq!(RunMode::default(), RunMode::Fallback);
        assert_eq!(RunMode::default().as_str(), "fallback");
    }

    #[test]
    fn mode_token_round_trips_for_every_variant() {
        for m in RunMode::ALL {
            assert_eq!(RunMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(RunMode::parse("nope"), None);
        // The serialized form matches the CLI token (kebab-case).
        assert_eq!(
            serde_json::to_string(&RunMode::Fallback).unwrap(),
            "\"fallback\""
        );
        assert_eq!(
            serde_json::to_string(&RunMode::Parallel).unwrap(),
            "\"parallel\""
        );
    }

    #[test]
    fn work_reading_token_round_trips_for_every_variant() {
        // The reading is on the wire now — in a result, in a history record, and
        // in the SDK schemas generated from both — so its tokens are a contract,
        // pinned the way [`RunMode`]'s are.
        for reading in RunWork::ALL {
            let token = format!("\"{}\"", reading.as_str());
            assert_eq!(serde_json::to_string(&reading).unwrap(), token);
            assert_eq!(
                serde_json::from_str::<RunWork>(&token).unwrap(),
                reading,
                "{token} must read back as the value that wrote it"
            );
        }
        assert!(serde_json::from_str::<RunWork>("\"partial\"").is_err());
    }

    #[test]
    fn startup_failures_fall_through() {
        // Structural "could not run": not installed, or resolved-but-unspawnable.
        assert_eq!(
            startup_failure_reason(Status::Skipped, None, false, RunWork::None),
            Some(FallThroughReason::NotInstalled)
        );
        assert_eq!(
            startup_failure_reason(Status::SpawnError, None, false, RunWork::None),
            Some(FallThroughReason::SpawnError)
        );
        // Provisioning "could not run": rejected before any work.
        assert_eq!(
            startup_failure_reason(
                Status::Nonzero,
                Some(FailureKind::Auth),
                false,
                RunWork::None
            ),
            Some(FallThroughReason::Auth)
        );
        assert_eq!(
            startup_failure_reason(
                Status::Nonzero,
                Some(FailureKind::Quota),
                false,
                RunWork::None
            ),
            Some(FallThroughReason::Quota)
        );
        // A candidate that was not run *because* its identity is unprovisioned
        // carries the credential reason, not the missing-binary one.
        assert_eq!(
            startup_failure_reason(
                Status::Skipped,
                Some(FailureKind::Auth),
                false,
                RunWork::None
            ),
            Some(FallThroughReason::Auth)
        );
        for status in [Status::Skipped, Status::SpawnError] {
            assert!(is_startup_failure(status, None, false, RunWork::None));
        }
        assert!(is_startup_failure(
            Status::Nonzero,
            Some(FailureKind::Auth),
            false,
            RunWork::None
        ));
        assert!(is_startup_failure(
            Status::Nonzero,
            Some(FailureKind::Quota),
            false,
            RunWork::None
        ));
        // A resume the harness could not resolve: this identity does not hold the
        // session, but the task is untouched, so the next candidate gets it.
        assert_eq!(
            startup_failure_reason(
                Status::Nonzero,
                Some(FailureKind::SessionNotFound),
                false,
                RunWork::None
            ),
            Some(FallThroughReason::SessionNotFound)
        );
        assert!(is_startup_failure(
            Status::Nonzero,
            Some(FailureKind::SessionNotFound),
            false,
            RunWork::None
        ));
        // Non-zero only: no observed CLI refuses an unknown session on a clean
        // exit, so a zero-exit run keeps whatever its own record said.
        assert_eq!(
            startup_failure_reason(
                Status::Ok,
                Some(FailureKind::SessionNotFound),
                false,
                RunWork::None
            ),
            None
        );
        // The precondition refusals: the harness's own check failed before the
        // request was made, so the task is untouched and the next candidate —
        // which may hold the trust, or the room — gets it.
        for (kind, reason) in [
            (
                FailureKind::UntrustedDirectory,
                FallThroughReason::UntrustedDirectory,
            ),
            (FailureKind::InputTooLarge, FallThroughReason::InputTooLarge),
            (FailureKind::ModelMismatch, FallThroughReason::ModelMismatch),
        ] {
            assert_eq!(
                startup_failure_reason(Status::Nonzero, Some(kind), false, RunWork::None),
                Some(reason)
            );
            assert!(is_startup_failure(
                Status::Nonzero,
                Some(kind),
                false,
                RunWork::None
            ));
            // Non-zero only, like `session_not_found`: both refusals were
            // captured off a CLI that exited 1, and a clean-exit arm would be
            // behavior no harness has been observed producing.
            assert_eq!(
                startup_failure_reason(Status::Ok, Some(kind), false, RunWork::None),
                None,
                "{kind:?} on a clean exit is not a refusal any CLI has produced"
            );
        }
    }

    #[test]
    fn a_turn_whose_shared_app_server_went_away_falls_through() {
        // The server under the turn crashed or restarted; the candidate and the
        // task are both fine, so the next candidate takes it.
        assert_eq!(
            startup_failure_reason(
                Status::Nonzero,
                Some(FailureKind::AppServerLost),
                false,
                RunWork::None
            ),
            Some(FallThroughReason::AppServerLost)
        );
        assert_eq!(FallThroughReason::AppServerLost.as_str(), "app-server-lost");
        assert_eq!(FailureKind::AppServerLost.as_str(), "app_server_lost");
        // Not a refusal: the turn was accepted and running when the server
        // went, so work it billed first is what happened.
        assert!(!FailureKind::AppServerLost.is_refusal());
    }

    #[test]
    fn a_real_run_does_not_fall_through() {
        // A clean run, a slow run that timed out, and a dry-run row all stay.
        assert_eq!(
            startup_failure_reason(Status::Ok, None, false, RunWork::None),
            None
        );
        assert_eq!(
            startup_failure_reason(Status::Timeout, None, false, RunWork::None),
            None
        );
        assert_eq!(
            startup_failure_reason(Status::Planned, None, false, RunWork::None),
            None
        );
        assert!(!is_startup_failure(Status::Ok, None, false, RunWork::None));
        assert!(!is_startup_failure(
            Status::Timeout,
            None,
            false,
            RunWork::None
        ));
    }

    #[test]
    fn a_cancelled_candidate_does_not_fall_through() {
        // Nothing about the candidate failed — the caller stopped it. Falling
        // through would spawn the very next harness the cancellation was meant
        // to prevent, which is the opposite of what was asked for.
        assert_eq!(
            startup_failure_reason(Status::Cancelled, None, false, RunWork::None),
            None
        );
        assert!(!is_startup_failure(
            Status::Cancelled,
            None,
            false,
            RunWork::None
        ));
    }

    #[test]
    fn a_real_failure_does_not_fall_through() {
        // A plain non-zero task failure (no classified reason) is a real run.
        assert_eq!(
            startup_failure_reason(Status::Nonzero, None, false, RunWork::None),
            None
        );
        assert!(!is_startup_failure(
            Status::Nonzero,
            None,
            false,
            RunWork::None
        ));
        // Configuration / did-run reasons are NOT setup failures for a
        // single-model run: an unknown model is a config mistake the user sees,
        // and a deferred-tool dead-end is a harness that *ran* (so the chain
        // stops there). A rate limit is neither — see the test below.
        for kind in [FailureKind::ModelNotFound, FailureKind::ToolDeferred] {
            assert_eq!(
                startup_failure_reason(Status::Nonzero, Some(kind), false, RunWork::None),
                None,
                "failure_kind {kind:?} must not fall through without a model list"
            );
            assert!(!is_startup_failure(
                Status::Nonzero,
                Some(kind),
                false,
                RunWork::None
            ));
        }
    }

    #[test]
    fn a_rate_limited_identity_falls_through_without_a_model_list() {
        // A rate limit is a property of whoever is being billed, not of the
        // model, so it hands the turn to the next *identity* exactly as `quota`
        // does — with a model list or without one. It used to fall through only
        // under `model_fallback`, and one rate-limited identity therefore ended a
        // chain that four further identities could have served.
        for model_fallback in [false, true] {
            assert_eq!(
                startup_failure_reason(
                    Status::Nonzero,
                    Some(FailureKind::RateLimit),
                    model_fallback,
                    RunWork::None
                ),
                Some(FallThroughReason::RateLimit),
                "a rate limit must hand the turn on (model_fallback={model_fallback})"
            );
            assert!(is_startup_failure(
                Status::Nonzero,
                Some(FailureKind::RateLimit),
                model_fallback,
                RunWork::None
            ));
        }
        // A clean exit hands on too, as `quota` and `auth` do: a provider that
        // answered `429` before any token was spent refused the identity whatever
        // exit code the harness chose. Issue #1297 — this read `None`, so one
        // clean-exit rate limit stopped a chain with quota still on it.
        for model_fallback in [false, true] {
            assert_eq!(
                startup_failure_reason(
                    Status::Ok,
                    Some(FailureKind::RateLimit),
                    model_fallback,
                    RunWork::None
                ),
                Some(FallThroughReason::RateLimit),
                "a clean-exit rate limit must hand the turn on (model_fallback={model_fallback})"
            );
        }
    }

    #[test]
    fn a_clean_exit_model_rejection_stops_even_with_a_model_list() {
        // The generic rule reads a model rejection off a non-zero exit only, as
        // it always has: a clean-exit one stops the chain, list or not.
        for model_fallback in [false, true] {
            assert_eq!(
                startup_failure_reason(
                    Status::Ok,
                    Some(FailureKind::ModelNotFound),
                    model_fallback,
                    RunWork::None
                ),
                None,
                "model_fallback={model_fallback}"
            );
        }
    }

    #[test]
    fn an_account_model_refusal_hands_on_from_either_exit_unless_it_ran() {
        // The account's refusal is the identity's, so it hands on from a clean
        // exit as from a non-zero one — and never once the candidate did work,
        // nor from a timeout or a cancellation, which are runs.
        for status in [Status::Nonzero, Status::Ok] {
            assert_eq!(
                account_model_refusal_reason(status, RunWork::None),
                Some(FallThroughReason::ModelNotFound),
                "{status:?}"
            );
            assert_eq!(
                account_model_refusal_reason(status, RunWork::Done),
                None,
                "{status:?}"
            );
        }
        for status in [Status::Timeout, Status::Cancelled, Status::Planned] {
            assert_eq!(
                account_model_refusal_reason(status, RunWork::None),
                None,
                "{status:?}"
            );
        }
    }

    #[test]
    fn model_errors_fall_through_only_with_a_model_list() {
        // Trying several models in order: an unusable model is "try the next
        // model", so it falls through with its own reason.
        assert_eq!(
            startup_failure_reason(
                Status::Nonzero,
                Some(FailureKind::ModelNotFound),
                true,
                RunWork::None
            ),
            Some(FallThroughReason::ModelNotFound)
        );
        assert_eq!(
            startup_failure_reason(
                Status::Nonzero,
                Some(FailureKind::ModelNotFound),
                false,
                RunWork::None
            ),
            None
        );
        assert!(is_startup_failure(
            Status::Nonzero,
            Some(FailureKind::ModelNotFound),
            true,
            RunWork::None
        ));
        // A plain task failure still stops the chain even with a model list — it
        // is a real run, not a per-model provisioning problem. A deferred-tool
        // dead-end likewise ran, so it stops the chain too.
        assert_eq!(
            startup_failure_reason(Status::Nonzero, None, true, RunWork::None),
            None
        );
        assert_eq!(
            startup_failure_reason(
                Status::Nonzero,
                Some(FailureKind::ToolDeferred),
                true,
                RunWork::None
            ),
            None
        );
        // The structural/provisioning reasons are unchanged by the model flag.
        assert_eq!(
            startup_failure_reason(Status::Skipped, None, true, RunWork::None),
            Some(FallThroughReason::NotInstalled)
        );
        assert_eq!(
            startup_failure_reason(
                Status::Nonzero,
                Some(FailureKind::Auth),
                true,
                RunWork::None
            ),
            Some(FallThroughReason::Auth)
        );
    }

    /// The README's fall-through table is prose over this function, and its
    /// "exited …" column is where a reader learns which exit codes a refusal
    /// hands on from. Each row below is derived from what the function answers
    /// for `Ok` and `Nonzero`, so a row cannot say "non-zero" of a kind that
    /// also falls through from a clean exit (issue #1297 shipped with the
    /// `rate_limit` row and the code agreeing on the narrower answer).
    #[test]
    fn readme_fall_through_rows_match_the_verdict() {
        let readme = include_str!("../../../../README.md").replace("\r\n", "\n");
        for (kind, label) in [
            (FailureKind::Auth, "`auth`"),
            (FailureKind::Quota, "`quota` (no credit)"),
            (FailureKind::RateLimit, "`rate_limit`"),
        ] {
            let on = |status| startup_failure_reason(status, Some(kind), false, RunWork::None);
            let reason = on(Status::Nonzero).expect("every kind here falls through non-zero");
            let exits = if on(Status::Ok) == Some(reason) {
                "zero or non-zero"
            } else {
                "non-zero"
            };
            let row = format!(
                "| Ran, exited {exits}, classified {label}, no work done | ✅ fall through — `{}` |",
                reason.as_str()
            );
            assert!(
                readme.contains(&row),
                "README.md must carry the fall-through row `{row}`"
            );
        }
    }

    #[test]
    fn a_candidate_that_did_work_never_falls_through() {
        // Every pair below falls through without work evidence, so each one shows
        // the short circuit overriding a reason that would otherwise apply.
        for (status, kind, model_fallback) in [
            (Status::Nonzero, Some(FailureKind::Auth), false),
            (Status::Nonzero, Some(FailureKind::Quota), false),
            (Status::Ok, Some(FailureKind::Quota), false),
            (Status::Nonzero, Some(FailureKind::SessionNotFound), false),
            (
                Status::Nonzero,
                Some(FailureKind::UntrustedDirectory),
                false,
            ),
            (Status::Nonzero, Some(FailureKind::InputTooLarge), false),
            (Status::Nonzero, Some(FailureKind::ModelMismatch), false),
            (Status::Nonzero, Some(FailureKind::AppServerLost), false),
            (Status::Nonzero, Some(FailureKind::RateLimit), false),
            (Status::Ok, Some(FailureKind::RateLimit), false),
            (Status::Nonzero, Some(FailureKind::ModelNotFound), true),
            (Status::Skipped, None, false),
            (Status::SpawnError, None, false),
        ] {
            assert!(
                startup_failure_reason(status, kind, model_fallback, RunWork::None).is_some(),
                "{status:?}/{kind:?} must fall through with no work evidence"
            );
            assert_eq!(
                startup_failure_reason(status, kind, model_fallback, RunWork::Done),
                None,
                "{status:?}/{kind:?} did work, so it must not fall through"
            );
            assert!(!is_startup_failure(
                status,
                kind,
                model_fallback,
                RunWork::Done
            ));
        }
    }

    #[test]
    fn work_evidence_reads_tool_events_and_billed_usage() {
        let mut result = zero_work_result();
        assert_eq!(RunWork::from_result(&result), RunWork::None);
        // ...and so is a result with no accounting at all (a bare limit line on
        // stderr carries none), the shared `Usage::reports_billed_work` rule.
        result.usage = Usage::default();
        assert_eq!(RunWork::from_result(&result), RunWork::None);
        result.events = Some(Vec::new());
        assert_eq!(RunWork::from_result(&result), RunWork::None);

        // Any single billed count is decisive.
        for billed in [
            Usage {
                input_tokens: Some(1),
                ..Usage::default()
            },
            Usage {
                output_tokens: Some(1),
                ..Usage::default()
            },
            Usage {
                cache_read_tokens: Some(1),
                ..Usage::default()
            },
            Usage {
                cache_write_tokens: Some(1),
                ..Usage::default()
            },
            Usage {
                cost_usd: Some(0.01),
                ..Usage::default()
            },
        ] {
            let mut worked = zero_work_result();
            worked.usage = billed;
            assert_eq!(RunWork::from_result(&worked), RunWork::Done);
        }

        // A tool call alone is decisive too, with no usage reported at all.
        let mut used_tools = zero_work_result();
        used_tools.events = Some(vec![tool_call()]);
        assert_eq!(RunWork::from_result(&used_tools), RunWork::Done);

        let mut overloaded_after_answer = zero_work_result();
        overloaded_after_answer.failure_kind = Some(FailureKind::ServerOverloaded);
        overloaded_after_answer.text = Some("partial answer".to_string());
        assert_eq!(
            RunWork::from_result(&overloaded_after_answer),
            RunWork::Done
        );
    }

    /// A result shaped like the real zero-work Claude session-limit rejection.
    fn zero_work_result() -> RunResult {
        RunResult {
            harness: "claude-code".to_string(),
            variant: None,
            harness_id: "claude-code".to_string(),
            bin: "claude".to_string(),
            available: true,
            status: Status::Nonzero,
            prompt: None,
            model: None,
            observed_model: None,
            exit_code: Some(1),
            duration_ms: Some(401),
            telemetry: None,
            command: vec!["claude".to_string()],
            output_format: crate::domain::report::OutputFormat::Json,
            text: Some("You've hit your session limit".to_string()),
            text_source: Some("json:result".to_string()),
            usage: Usage {
                input_tokens: Some(0),
                output_tokens: Some(0),
                cache_read_tokens: Some(0),
                cache_write_tokens: Some(0),
                cost_usd: Some(0.0),
            },
            usage_source: Some("json".to_string()),
            session_id: None,
            events: None,
            events_source: None,
            structured: None,
            schema_valid: None,
            schema_attempts: None,
            schema_error: None,
            failure_kind: Some(FailureKind::Quota),
            work: None,
            failure_kind_source: Some("stdout".to_string()),
            stdout: String::new(),
            stderr: String::new(),
            error: None,
        }
    }

    fn tool_call() -> crate::domain::events::ActionEvent {
        crate::domain::events::ActionEvent {
            kind: "tool_call".to_string(),
            name: Some("Bash".to_string()),
            input: None,
            output: None,
            index: 0,
            tool_call_id: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            status: None,
            timing_source: None,
        }
    }
}
