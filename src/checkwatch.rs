//! Check visibility: every failing check on a change this run published, shown to
//! the manager once, and the checks still running after a publication returned,
//! followed until they settle.
//!
//! **Nothing here decides anything.** A publication's own required checks rule on
//! its outcome inside `onevcs`, exactly as before; what this module adds is reading
//! and reporting. A `check-failed` surface is non-blocking whatever the check, a
//! follow read never moves a node, a dependent or a publication, and a check that
//! stops being followed is named rather than waited on.
//!
//! Three things live here, each driven by the reconcile loop:
//!
//! - [`CheckWatch::observe`] sees every `change-check` the run relays — from a
//!   publication's own watch or from a follow read — and raises one `check-failed`
//!   surface, then its `check-failed` record, per (change request, head commit,
//!   check) whose state is `failed` or `no-verdict`.
//! - [`CheckWatch::pass`] follows a settled node's change request while any check on
//!   the head it pushed is still unsettled by the last `change-check` its session
//!   recorded: one batched `onevcs::follow_checks` per repository at most every
//!   [`FOLLOW_ENV`], off the loop's thread, relaying what each read appended to the
//!   session's stream. It stops on `outstanding: false`, on `superseded: true`, or
//!   once onevcs's checks timeout has passed since the publication returned — and
//!   then names the checks it stopped following in one `checks-unfollowed` surface
//!   and record.
//! - [`ending`] is what a driver's `driver-exited` carries: every check still
//!   followed, and every check a `checks-unfollowed` record named.
//!
//! What it seeds from is the run's own record — the `check-failed` and
//! `checks-unfollowed` records [`crate::checkpoint::recalled`] keeps, and each
//! settled session's own stream — so an adopting driver raises nothing twice and
//! resumes following exactly what its predecessor was.

// llmlint: ignore-file[invalid_states_unrepresentable] a node id, a change request URL, a
// commit and a check name are the plain strings the relayed `change-check` payload and
// the journal payloads spell them as, for the reason `src/plan.rs` records; each key here
// is read off a record another process wrote rather than built from a typed source.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::channel::Surface;
use crate::error::Result;
use crate::event::{Envelope, Source};
use crate::graph::NodeStatus;
use crate::journal::{self, Journal};
use crate::ledger::RunPaths;
use crate::projection::RunState;
use crate::vocabulary::requirement_label;

/// The environment variable bounding how often one repository's followed change
/// requests are read, in seconds (fractions allowed).
pub const FOLLOW_ENV: &str = "ONEPIPELINE_CHECK_FOLLOW_SECONDS";

/// How often a repository's followed checks are read when nothing overrides it: a
/// minute, the bound the release probe is held to.
pub const DEFAULT_FOLLOW_SECONDS: f64 = 60.0;

/// onevcs's own bound on waiting for a change request's checks, which is also how
/// long after a publication returned its checks are followed.
pub const CHECKS_TIMEOUT_ENV: &str = "ONEVCS_CHECKS_TIMEOUT_SECONDS";

/// onevcs's default for [`CHECKS_TIMEOUT_ENV`]: an hour.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] onevcs 0.58.0 declares this default only in its private `gh` module and exports no constant or reader for it, so there is nothing to import; the follow-up asking onevcs to export it is drafted, and until then this is that library's documented default for the variable named beside it.
pub const DEFAULT_CHECKS_TIMEOUT_SECONDS: f64 = 3600.0;

/// The surface kind a failing check is raised under.
pub const FAILED_SURFACE_KIND: &str = "check-failed";

/// The surface kind checks the driver stopped following are named under.
pub const UNFOLLOWED_SURFACE_KIND: &str = "checks-unfollowed";

const NO_ADDRESS: &str = "the host reported no address for this check";

/// A duration read from `name` in seconds, fractions allowed; `default` for an unset
/// value and for one that is not a positive number a `Duration` can hold.
pub(crate) fn seconds_env(name: &str, default: f64) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|seconds| *seconds > 0.0)
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .unwrap_or_else(|| Duration::from_secs_f64(default))
}

/// One `change-check` record's reading of one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reading {
    pub(crate) name: String,
    pub(crate) state: String,
    pub(crate) conclusion: Option<String>,
    pub(crate) required: bool,
    pub(crate) url: Option<String>,
    pub(crate) settled: bool,
}

impl Reading {
    /// The reading a relayed `change-check` carries, or `None` for any other record and
    /// for one that does not say what it read: a name, a state this stack names, and
    /// whether the check is required. A relayed record is another process's output, so a
    /// field missing or of another type is not read as a pending advisory check.
    pub(crate) fn of(event: &Envelope) -> Option<Self> {
        if event.source != Source::Vcs || event.kind.0 != "change-check" {
            return None;
        }
        let text = |key: &str| {
            event
                .payload
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let name = text("name").filter(|name| !name.is_empty())?;
        // Through the sibling's own type, so a state it adds or renames is the sibling's
        // word here too rather than a list copied beside it.
        let state = event
            .payload
            .get("state")
            .cloned()
            .and_then(|state| serde_json::from_value::<onevcs::CheckState>(state).ok())
            .and_then(|state| serde_json::to_value(state).ok())
            .and_then(|state| state.as_str().map(str::to_owned))?;
        let required = event.payload.get("required").and_then(Value::as_bool)?;
        let settled = match event.payload.get("status") {
            // llmlint: ignore[boundary_inputs_validated] onevcs fixes no status vocabulary — the field is the host's own words, passed through — and decides a check settled by `completed` alone (`onevcs::Check::settled`), so any other word is a check still running, read here exactly as the sibling reads it.
            Some(Value::String(status)) => status.eq_ignore_ascii_case("completed"),
            None | Some(Value::Null) => state != "pending",
            Some(_) => return None,
        };
        Some(Self {
            name,
            settled,
            conclusion: text("conclusion"),
            required,
            url: text("url"),
            state,
        })
    }

    /// Whether this reading is one the manager is told about: a check that failed or
    /// ended with no verdict, required or not.
    fn failing(&self) -> bool {
        matches!(self.state.as_str(), "failed" | "no-verdict")
    }
}

/// The one line a `change-check` record is rendered as wherever the run's events are
/// read as lines — `monitor`, `next`, a run's `watch` and a flow's.
///
/// `change-check '<check>' <state> (<requirement label>) [<significance>] log: <url>`,
/// ending at the label's closing bracket when the record carries no address.
pub(crate) fn change_check_line(event: &Envelope) -> Option<String> {
    let reading = Reading::of(event)?;
    let significance = event
        .labels
        .extra
        .get("significance")
        .and_then(Value::as_str)
        .unwrap_or("-");
    let mut line = format!(
        "change-check '{}' {} ({}) [{significance}]",
        reading.name,
        reading.state,
        requirement_label(reading.required)
    );
    if let Some(url) = &reading.url {
        line.push_str(&format!(" log: {url}"));
    }
    Some(line)
}

/// The first twelve characters of a commit, as every surface here names one.
fn short(head: &str) -> &str {
    head.get(..12).unwrap_or(head)
}

/// The `check-failed` surface's one-line message.
fn failed_message(node: &str, change: &str, head: &str, reading: &Reading) -> String {
    let what = if reading.state == "no-verdict" {
        format!(
            "got no verdict ({})",
            reading.conclusion.as_deref().unwrap_or("none")
        )
    } else {
        "failed".to_owned()
    };
    let consequence = if reading.required {
        "The publication's own outcome decides what happens to the node."
    } else {
        "Nothing is held, failed or retried for it."
    };
    format!(
        "check '{name}' {what} on {change} (node '{node}', head {head}): {label}; log: {url}. \
         {consequence}",
        name = reading.name,
        head = short(head),
        label = requirement_label(reading.required),
        url = reading.url.as_deref().unwrap_or(NO_ADDRESS),
    )
}

/// The `checks-unfollowed` surface's one-line message.
fn unfollowed_message(node: &str, change: &str, head: &str, checks: &[(String, bool)]) -> String {
    let named: Vec<String> = checks
        .iter()
        .map(|(name, required)| format!("'{name}' ({})", requirement_label(*required)))
        .collect();
    format!(
        "stopped following {count} check(s) still running on {change} (node '{node}', head \
         {head}) after the checks timeout: {named}; a later result of these is not shown.",
        count = checks.len(),
        head = short(head),
        named = named.join(", "),
    )
}

/// A non-blocking report surface about `node`, as the release wait raises its own.
fn report(kind: &str, node: &str, message: String) -> Surface {
    Surface {
        id: 0,
        kind: kind.to_owned(),
        message,
        source: crate::channel::source::PROPOSAL.to_owned(),
        blocking: false,
        queued_at: crate::sys::now_millis(),
        abandoned: false,
        asker: None,
        workstream: Some(node.to_owned()),
        correlation: None,
    }
}

/// Where a node's publication is, as the records its session relayed say: the change
/// request it opened and the head it pushed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Context {
    change: Option<String>,
    head: Option<String>,
    base: Option<String>,
}

impl Context {
    /// Take up what one of the session's records says about the publication.
    fn note(&mut self, event: &Envelope) {
        if event.source != Source::Vcs {
            return;
        }
        let text = |key: &str| {
            event
                .payload
                .get(key)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        match event.kind.0.as_str() {
            "push" => {
                if let Some(head) = text("head") {
                    self.head = Some(head);
                }
            }
            "change-opened" => {
                self.change = text("url").or(self.change.take());
                self.base = text("base").or(self.base.take());
            }
            "publication-resumed" => {
                self.change = text("change_url").or(self.change.take());
                self.head = text("head").or(self.head.take());
                self.base = text("base").or(self.base.take());
            }
            "checks-settled" | "merge-armed" => {
                self.change = text("url").or(self.change.take());
                self.head = text("head").or(self.head.take());
            }
            _ => {}
        }
    }
}

/// One check a followed change request carries on its head.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Standing {
    required: bool,
    settled: bool,
}

/// A change request being followed after its publication returned.
#[derive(Debug, Clone)]
struct Followed {
    repo: String,
    root: String,
    change: String,
    head: String,
    /// The session's own stream — the one holding its `change-opened` — which is
    /// where a follow read appends what it found.
    stream: Option<String>,
    token: onevcs::SessionToken,
    /// When its publication returned, in epoch milliseconds.
    returned_at: u64,
    checks: BTreeMap<String, Standing>,
}

impl Followed {
    fn running(&self) -> Vec<(String, bool)> {
        self.checks
            .iter()
            .filter(|(_, standing)| !standing.settled)
            .map(|(name, standing)| (name.clone(), standing.required))
            .collect()
    }
}

/// What a settled node's session says about its publication's checks: the change
/// request and head, and the latest reading of each check on that head.
fn followed_of(state: &RunState, node: &str, now: u64) -> Option<Followed> {
    let session = state.sessions.get(node)?;
    let repo = state.graph.get(node).and_then(|node| node.repo.clone())?;
    let read = crate::vcs::events(session.token(), None);
    let mut context = Context::default();
    let mut stream: Option<String> = None;
    let mut latest: BTreeMap<String, Standing> = BTreeMap::new();
    let mut returned_at: Option<u64> = None;
    for event in &read {
        if event.source != Source::Vcs {
            continue;
        }
        if event.kind.0 == "change-opened" {
            stream = Some(event.stream.clone());
        }
        let pushed_before = context.head.clone();
        context.note(event);
        // A push of a new head starts that head's checks afresh.
        if event.kind.0 == "push" && context.head != pushed_before {
            latest.clear();
        }
        if crate::vcs::is_terminator(event) {
            returned_at = crate::projection::millis_of(&event.ts);
        }
        if let Some(reading) = Reading::of(event) {
            latest.insert(
                reading.name.clone(),
                Standing {
                    required: reading.required,
                    settled: reading.settled,
                },
            );
        }
    }
    let change = state.known_change_url(node).or(context.change)?;
    let head = state.heads.get(node).cloned().or(context.head)?;
    Some(Followed {
        repo,
        root: context.base.unwrap_or_else(|| "main".to_owned()),
        change,
        head,
        stream,
        token: session.token().clone(),
        returned_at: returned_at.unwrap_or(now),
        checks: latest,
    })
}

/// Whether a node's status is one its publication has returned at.
fn returned(status: Option<&NodeStatus>) -> bool {
    matches!(
        status,
        Some(
            NodeStatus::Done
                | NodeStatus::CompleteDraft
                | NodeStatus::CompleteHeld
                | NodeStatus::Failed
        )
    )
}

/// Each (change request, head) a follow read in this process answered superseded: its
/// head moved on, so whatever its session last recorded running there is not running on
/// the change request any more, and the run's ending does not list it. Held for the
/// process rather than recorded, because the driver that read it is the one that writes
/// the ending; a driver adopting the run reads the change again and learns it the same way.
fn superseded() -> &'static std::sync::Mutex<BTreeSet<(String, String)>> {
    static SUPERSEDED: std::sync::OnceLock<std::sync::Mutex<BTreeSet<(String, String)>>> =
        std::sync::OnceLock::new();
    SUPERSEDED.get_or_init(Default::default)
}

/// One answered follow read: the repository, the nodes it was about in the order
/// asked, and what the host said or why it did not.
type Answered = (
    String,
    Vec<String>,
    std::result::Result<onevcs::FollowedChecks, String>,
);

/// What a check-visibility pass knows and has said.
pub(crate) struct CheckWatch {
    /// Each (change request, head, check) a `check-failed` was raised for, seeded from
    /// the run's own records.
    raised: BTreeSet<(String, String, String)>,
    /// Each (change request, head) a `checks-unfollowed` was raised for, seeded the
    /// same way.
    unfollowed: BTreeSet<(String, String)>,
    /// Where each node's publication is, as its relayed records say.
    contexts: BTreeMap<String, Context>,
    /// The (node, session) pairs whose settlement this driver has looked at.
    considered: BTreeSet<(String, String)>,
    /// The change requests followed, by node.
    followed: BTreeMap<String, Followed>,
    /// When each repository was last read.
    read_at: BTreeMap<String, Instant>,
    /// The repositories with a read in flight.
    reading: BTreeSet<String>,
    send: Sender<Answered>,
    answers: Receiver<Answered>,
    /// Answers taken up but not yet applied by a pass.
    taken: Vec<Answered>,
    every: Duration,
    timeout: Duration,
}

impl CheckWatch {
    /// Start watching one run, taking up what a previous driver raised.
    pub(crate) fn of_run(paths: &RunPaths) -> Self {
        let mut raised = BTreeSet::new();
        let mut unfollowed = BTreeSet::new();
        for event in crate::checkpoint::recalled(paths) {
            let text = |key: &str| {
                event
                    .payload
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            match journal::PipelineKind::from_wire(&event.kind) {
                Some(journal::PipelineKind::CheckFailed) => {
                    if let (Some(change), Some(head), Some(check)) =
                        (text("change"), text("head"), text("check"))
                    {
                        raised.insert((change, head, check));
                    }
                }
                Some(journal::PipelineKind::ChecksUnfollowed) => {
                    if let (Some(change), Some(head)) = (text("change"), text("head")) {
                        unfollowed.insert((change, head));
                    }
                }
                _ => {}
            }
        }
        let (send, answers) = mpsc::channel();
        Self {
            raised,
            unfollowed,
            contexts: BTreeMap::new(),
            considered: BTreeSet::new(),
            followed: BTreeMap::new(),
            read_at: BTreeMap::new(),
            reading: BTreeSet::new(),
            send,
            answers,
            taken: Vec::new(),
            every: seconds_env(FOLLOW_ENV, DEFAULT_FOLLOW_SECONDS),
            // llmlint: ignore[changed_behavior_has_e2e] the shipped hour cannot be waited out by a journey, and this variable is onevcs's own, which reads it for the publication's watch too — so a journey handing it an unusable value changes the publication under test. `seconds_env`'s fallback is held by this module's `an_unusable_interval_falls_back_to_the_shipped_one`, and the variable's wiring here by every timeout journey in `tests/e2e/check_visibility.rs`.
            timeout: seconds_env(CHECKS_TIMEOUT_ENV, DEFAULT_CHECKS_TIMEOUT_SECONDS),
        }
    }

    /// See one record the run relayed, and raise a `check-failed` for it where it is
    /// a failing check not raised before at this change request and head.
    ///
    /// # Errors
    ///
    /// Where the channel or the journal cannot be written.
    pub(crate) fn observe(
        &mut self,
        paths: &RunPaths,
        journal: &mut Journal,
        state: &RunState,
        event: &Envelope,
    ) -> Result<()> {
        let Some(node) = event.labels.node.clone() else {
            return Ok(());
        };
        if event.source != Source::Vcs {
            return Ok(());
        }
        self.contexts.entry(node.clone()).or_default().note(event);
        let Some(reading) = Reading::of(event) else {
            return Ok(());
        };
        if !reading.failing() {
            return Ok(());
        }
        let context = self.contexts.get(&node).cloned().unwrap_or_default();
        let followed = self.followed.get(&node);
        let Some(change) = followed
            .map(|followed| followed.change.clone())
            .or(context.change)
            .or_else(|| state.known_change_url(&node))
        else {
            return Ok(());
        };
        let head = followed
            .map(|followed| followed.head.clone())
            .or(context.head)
            .or_else(|| state.heads.get(&node).cloned())
            .unwrap_or_default();
        if !self
            .raised
            .insert((change.clone(), head.clone(), reading.name.clone()))
        {
            return Ok(());
        }
        // The surface first and the record second, as the release wait raises its
        // own: a reader holding the record finds the surface it describes.
        crate::engine::raised(
            paths,
            journal,
            report(
                FAILED_SURFACE_KIND,
                &node,
                failed_message(&node, &change, &head, &reading),
            ),
        )?;
        journal.emit(
            journal::PipelineKind::CheckFailed,
            journal::labels(&paths.run, Some(&node)),
            journal::payload(&[
                ("node", json!(node)),
                ("change", json!(change)),
                ("head", json!(head)),
                ("check", json!(reading.name)),
                ("state", json!(reading.state)),
                ("conclusion", json!(reading.conclusion)),
                ("required", json!(reading.required)),
                ("requirement", json!(requirement_label(reading.required))),
                ("url", json!(reading.url)),
            ]),
        )?;
        Ok(())
    }

    /// Take up whatever follow reads have answered, and say whether any did. Never
    /// blocks: what the loop's wait is woken by.
    pub(crate) fn take_up_answers(&mut self) -> bool {
        let before = self.taken.len();
        self.taken.extend(self.answers.try_iter());
        self.taken.len() > before
    }

    /// How long the loop may go before this watch is due again: never while nothing
    /// is followed.
    pub(crate) fn next_due(&self) -> Duration {
        if self.followed.is_empty() {
            return Duration::MAX;
        }
        let now = crate::sys::now_millis();
        let mut next = Duration::MAX;
        for followed in self.followed.values() {
            let deadline = followed
                .returned_at
                .saturating_add(u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX));
            next = next.min(Duration::from_millis(deadline.saturating_sub(now)));
            if self.reading.contains(&followed.repo) {
                continue;
            }
            let due = self
                .read_at
                .get(&followed.repo)
                .map_or(Duration::ZERO, |at| self.every.saturating_sub(at.elapsed()));
            next = next.min(due);
        }
        next
    }

    /// One pass: apply what reads answered, follow what newly settled, stop following
    /// what timed out, and start the reads that are due.
    ///
    /// # Errors
    ///
    /// Where the channel or the journal cannot be written.
    pub(crate) fn pass(
        &mut self,
        paths: &RunPaths,
        journal: &mut Journal,
        state: &RunState,
        statuses: &BTreeMap<String, NodeStatus>,
        filter: Option<&crate::filter::EventFilter>,
    ) -> Result<()> {
        self.take_up_answers();
        for (repo, nodes, answer) in std::mem::take(&mut self.taken) {
            self.reading.remove(&repo);
            self.apply(paths, journal, state, &nodes, answer, filter)?;
        }
        let now = crate::sys::now_millis();
        self.consider(state, statuses, now);
        self.expire(paths, journal, now)?;
        self.start_reads();
        Ok(())
    }

    /// Follow every node whose publication has returned since this driver last
    /// looked, while a check on the head it pushed is unsettled.
    fn consider(&mut self, state: &RunState, statuses: &BTreeMap<String, NodeStatus>, now: u64) {
        for (node, session) in &state.sessions {
            let status = statuses.get(node);
            if !returned(status) {
                // A node running again is followed by its own publication's watch.
                // llmlint: ignore-block[changed_behavior_has_e2e] no command a user can type
                // dispatches a settled node again under its own id — `requeue` is refused for
                // any node that is not parked, and `retry` replaces it under a new id — so this
                // is the guard for a fold that ever answers it `running` again, with no journey
                // to drive it.
                if matches!(status, Some(NodeStatus::Running)) {
                    self.followed.remove(node);
                }
                // llmlint: ignore-end[changed_behavior_has_e2e]
                continue;
            }
            if !self
                .considered
                .insert((node.clone(), session.token().0.clone()))
            {
                continue;
            }
            let Some(followed) = followed_of(state, node, now) else {
                continue;
            };
            if followed.running().is_empty()
                || self
                    .unfollowed
                    .contains(&(followed.change.clone(), followed.head.clone()))
            {
                continue;
            }
            self.contexts.insert(
                node.clone(),
                Context {
                    change: Some(followed.change.clone()),
                    head: Some(followed.head.clone()),
                    base: Some(followed.root.clone()),
                },
            );
            self.followed.insert(node.clone(), followed);
        }
    }

    /// Stop following every change request whose checks timeout has passed since its
    /// publication returned, naming what was still running.
    fn expire(&mut self, paths: &RunPaths, journal: &mut Journal, now: u64) -> Result<()> {
        let timeout = u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX);
        let expired: Vec<String> = self
            .followed
            .iter()
            .filter(|(_, followed)| now >= followed.returned_at.saturating_add(timeout))
            .map(|(node, _)| node.clone())
            .collect();
        for node in expired {
            let Some(followed) = self.followed.remove(&node) else {
                continue;
            };
            let running = followed.running();
            if running.is_empty()
                || !self
                    .unfollowed
                    .insert((followed.change.clone(), followed.head.clone()))
            {
                continue;
            }
            crate::engine::raised(
                paths,
                journal,
                report(
                    UNFOLLOWED_SURFACE_KIND,
                    &node,
                    unfollowed_message(&node, &followed.change, &followed.head, &running),
                ),
            )?;
            journal.emit(
                journal::PipelineKind::ChecksUnfollowed,
                journal::labels(&paths.run, Some(&node)),
                journal::payload(&[
                    ("node", json!(node)),
                    ("change", json!(followed.change)),
                    ("head", json!(followed.head)),
                    ("reason", json!("timeout")),
                    (
                        "checks",
                        Value::Array(
                            running
                                .iter()
                                .map(|(check, required)| {
                                    json!({
                                        "check": check,
                                        "required": required,
                                        "requirement": requirement_label(*required),
                                    })
                                })
                                .collect(),
                        ),
                    ),
                ]),
            )?;
        }
        Ok(())
    }

    /// Start one read per repository that is due and has none in flight: every
    /// followed change request of that repository in the one call.
    fn start_reads(&mut self) {
        let mut by_repo: BTreeMap<String, Vec<(String, Followed)>> = BTreeMap::new();
        for (node, followed) in &self.followed {
            by_repo
                .entry(followed.repo.clone())
                .or_default()
                .push((node.clone(), followed.clone()));
        }
        for (repo, followed) in by_repo {
            if self.reading.contains(&repo) {
                continue;
            }
            if self
                .read_at
                .get(&repo)
                .is_some_and(|at| at.elapsed() < self.every)
            {
                continue;
            }
            let mut changes = Vec::new();
            let mut nodes = Vec::new();
            let mut unreadable = None;
            for (node, one) in &followed {
                match one.change.parse::<onevcs::Url>() {
                    Ok(change) => {
                        changes.push(onevcs::FollowedHead {
                            change,
                            head: onevcs::Sha(one.head.clone()),
                        });
                        nodes.push(node.clone());
                    }
                    Err(error) => unreadable = Some(format!("{}: {error}", one.change)),
                }
            }
            if let Some(why) = unreadable {
                eprintln!("onepipeline: a followed change request of {repo} is not a URL: {why}");
            }
            if changes.is_empty() {
                continue;
            }
            let request = onevcs::FollowChecksRequest {
                repo: repo.clone(),
                root: followed[0].1.root.clone(),
                changes,
            };
            self.read_at.insert(repo.clone(), Instant::now());
            self.reading.insert(repo.clone());
            let send = self.send.clone();
            let asked = repo.clone();
            // Off the loop's thread for the release probe's reason: the read is a
            // host query, and a slow one asked inline would stall every node.
            let spawned = std::thread::Builder::new()
                .name("check-follow".to_owned())
                .spawn(move || {
                    let answer = onevcs::follow_checks(&onevcs::Providers::real(), &request)
                        .map_err(|error| error.to_string());
                    let _ = send.send((asked, nodes, answer));
                });
            // llmlint: ignore-block[changed_behavior_has_e2e] no invocation a user can type
            // reaches this arm: it is a host that will not start a thread, which no plan, flag
            // or environment decides — the reason `release::Asker::start` gives for its own.
            // What it does is leave the repository due again on the interval, as a refused
            // read does, which `a_refused_follow_read_leaves_the_change_followed_until_it_answers`
            // drives end to end.
            if let Err(error) = spawned {
                self.reading.remove(&repo);
                eprintln!("onepipeline: cannot start a check follow read: {error}");
            } // llmlint: ignore-end[changed_behavior_has_e2e]
        }
    }

    /// Apply one repository's answer: relay what it appended, and stop following each
    /// change request it says is settled or superseded.
    fn apply(
        &mut self,
        paths: &RunPaths,
        journal: &mut Journal,
        state: &RunState,
        nodes: &[String],
        answer: std::result::Result<onevcs::FollowedChecks, String>,
        filter: Option<&crate::filter::EventFilter>,
    ) -> Result<()> {
        let answered = match answer {
            Ok(answered) => answered,
            Err(why) => {
                // A failed read leaves every change request followed: the next one
                // is due on the interval like any other.
                eprintln!(
                    "onepipeline: could not read the checks still running on {}: {why}",
                    nodes.join(", ")
                );
                return Ok(());
            }
        };
        for (node, change) in nodes.iter().zip(answered.changes) {
            if !self.followed.contains_key(node) {
                continue;
            }
            self.relay(paths, journal, state, node, filter)?;
            let Some(followed) = self.followed.get_mut(node) else {
                continue;
            };
            for check in &change.checks {
                followed.checks.insert(
                    check.name.clone(),
                    Standing {
                        required: check.required,
                        settled: check.settled(),
                    },
                );
            }
            if !change.outstanding || change.superseded {
                if let Some(stopped) = self.followed.remove(node) {
                    if change.superseded {
                        superseded()
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert((stopped.change, stopped.head));
                    }
                }
            }
        }
        Ok(())
    }

    /// Relay into the run's journal the `change-check` records a follow read
    /// appended to `node`'s session stream, past the mark that stream stands at.
    fn relay(
        &mut self,
        paths: &RunPaths,
        journal: &mut Journal,
        state: &RunState,
        node: &str,
        filter: Option<&crate::filter::EventFilter>,
    ) -> Result<()> {
        let Some(followed) = self.followed.get(node) else {
            return Ok(());
        };
        let token = followed.token.clone();
        let stream = followed.stream.clone();
        let known = crate::engine::dispatch_labels(
            &paths.run,
            node,
            None,
            state
                .graph
                .get(node)
                .and_then(|node| node.persona.as_deref()),
        );
        let mut marks =
            crate::vcs::Watermarks::of_streams(crate::checkpoint::streams_reached(paths));
        for mut envelope in crate::vcs::events(&token, filter) {
            if Reading::of(&envelope).is_none()
                || stream
                    .as_ref()
                    .is_some_and(|stream| *stream != envelope.stream)
                || !marks.beyond(&envelope)
            {
                continue;
            }
            crate::lifecycle::stamp(&mut envelope.labels, &known);
            journal.relay(&envelope)?;
            marks.reached(&envelope);
            self.observe(paths, journal, state, &envelope)?;
        }
        Ok(())
    }
}

/// What a run's ending record carries about checks: every check still followed —
/// unsettled by the last `change-check` its session recorded, on a settled node's
/// change request no `checks-unfollowed` named — and every check a
/// `checks-unfollowed` record named.
///
/// Read from the run's own record rather than off a driver's memory, which is what
/// an adopting driver resumes from too, so the two cannot disagree.
pub(crate) fn ending(
    paths: &RunPaths,
) -> (
    Vec<crate::payload::EndingCheck>,
    Vec<crate::payload::EndingCheck>,
) {
    let named = |node: &str, change: &str, head: &str, check: &str, required: bool| {
        crate::payload::EndingCheck {
            node: node.to_owned(),
            change: change.to_owned(),
            head: head.to_owned(),
            check: check.to_owned(),
            required,
            requirement: requirement_label(required).to_owned(),
        }
    };
    let mut unfollowed_keys = BTreeSet::new();
    let mut unfollowed = Vec::new();
    for event in crate::checkpoint::recalled(paths) {
        if journal::PipelineKind::from_wire(&event.kind)
            != Some(journal::PipelineKind::ChecksUnfollowed)
        {
            continue;
        }
        let text = |key: &str| {
            event
                .payload
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let (node, change, head) = (text("node"), text("change"), text("head"));
        unfollowed_keys.insert((change.clone(), head.clone()));
        for check in event
            .payload
            .get("checks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            unfollowed.push(named(
                &node,
                &change,
                &head,
                check
                    .get("check")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                check
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ));
        }
    }
    let state = crate::checkpoint::current(paths);
    let statuses = state.statuses();
    let now = crate::sys::now_millis();
    let mut running = Vec::new();
    for node in state.sessions.keys() {
        if !returned(statuses.get(node)) {
            continue;
        }
        let Some(followed) = followed_of(&state, node, now) else {
            continue;
        };
        let key = (followed.change.clone(), followed.head.clone());
        let moved_on = superseded()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&key);
        if unfollowed_keys.contains(&key) || moved_on {
            continue;
        }
        for (check, required) in followed.running() {
            running.push(named(
                node,
                &followed.change,
                &followed.head,
                &check,
                required,
            ));
        }
    }
    (running, unfollowed)
}

/// The two sections `results` and `status` print about a run's checks at its end,
/// read off its latest `driver-exited`; empty when it names neither.
pub(crate) fn ending_sections(exited: Option<&Envelope>) -> String {
    let Some(exited) = exited else {
        return String::new();
    };
    let mut out = String::new();
    for (key, heading) in [
        ("checks_running", "checks still running at run end:"),
        (
            "checks_unfollowed",
            "checks no longer followed (timed out):",
        ),
    ] {
        let listed: Vec<&Value> = exited
            .payload
            .get(key)
            .and_then(Value::as_array)
            .map(|list| list.iter().collect())
            .unwrap_or_default();
        if listed.is_empty() {
            continue;
        }
        out.push_str(heading);
        out.push('\n');
        for check in listed {
            let text = |key: &str| check.get(key).and_then(Value::as_str).unwrap_or("-");
            let required = check
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            out.push_str(&format!(
                "  {} on {} (node '{}') — {}\n",
                crate::views::one_line(text("check")),
                crate::views::one_line(text("change")),
                crate::views::one_line(text("node")),
                requirement_label(required),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(payload: Value, significance: &str) -> Envelope {
        let mut labels = crate::event::Labels {
            node: Some("a".into()),
            ..crate::event::Labels::default()
        };
        labels
            .extra
            .insert("significance".into(), json!(significance));
        Envelope {
            v: crate::event::ENVELOPE_VERSION,
            ts: "2026-11-02T00:00:00Z".into(),
            stream: "s".into(),
            seq: 1,
            source: Source::Vcs,
            kind: crate::event::EventKind("change-check".into()),
            dimensions: Default::default(),
            labels,
            payload: payload.as_object().cloned().expect("an object"),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn the_requirement_label_is_stated_once_in_both_its_words() {
        assert_eq!(requirement_label(true), "required — blocks the merge");
        assert_eq!(requirement_label(false), "not required — does not block");
    }

    #[test]
    fn a_change_check_line_names_the_check_its_label_and_its_log() {
        let event = check(
            json!({"name": "slow", "state": "failed", "status": "completed", "required": false,
                   "conclusion": "failure", "url": "https://example.test/job/3"}),
            "attention",
        );
        assert_eq!(
            change_check_line(&event).as_deref(),
            Some(
                "change-check 'slow' failed (not required — does not block) [attention] log: \
                 https://example.test/job/3"
            )
        );
        let bare = check(
            json!({"name": "lint", "state": "passed", "status": "completed", "required": true,
                   "url": null}),
            "progress",
        );
        assert_eq!(
            change_check_line(&bare).as_deref(),
            Some("change-check 'lint' passed (required — blocks the merge) [progress]")
        );
    }

    #[test]
    fn a_change_check_that_does_not_say_what_it_read_is_no_reading() {
        for payload in [
            json!({"state": "failed", "required": false}),
            json!({"name": "slow", "state": "exploded", "required": false}),
            json!({"name": "slow", "state": "failed"}),
            json!({"name": "slow", "state": "failed", "required": "no"}),
            json!({"name": "slow", "state": "failed", "required": false, "status": 3}),
        ] {
            assert_eq!(
                Reading::of(&check(payload.clone(), "attention")),
                None,
                "{payload}"
            );
        }
        let read = Reading::of(&check(
            json!({"name": "slow", "state": "pending", "required": false, "status": null}),
            "progress",
        ))
        .expect("a reading");
        assert!(!read.settled);
    }

    #[test]
    fn a_failed_check_s_message_is_one_line_naming_what_it_does_not_hold() {
        let reading = Reading {
            name: "slow".into(),
            state: "no-verdict".into(),
            conclusion: Some("cancelled".into()),
            required: false,
            url: None,
            settled: true,
        };
        let message = failed_message(
            "a",
            "https://github.com/o/r/pull/1",
            "0123456789abcdef",
            &reading,
        );
        assert_eq!(
            message,
            "check 'slow' got no verdict (cancelled) on https://github.com/o/r/pull/1 (node 'a', \
             head 0123456789ab): not required — does not block; log: the host reported no \
             address for this check. Nothing is held, failed or retried for it."
        );
        assert!(!message.contains('\n'));
        let unfollowed = unfollowed_message(
            "a",
            "https://github.com/o/r/pull/1",
            "0123456789abcdef",
            &[("slow".into(), false), ("gate".into(), true)],
        );
        assert_eq!(
            unfollowed,
            "stopped following 2 check(s) still running on https://github.com/o/r/pull/1 (node \
             'a', head 0123456789ab) after the checks timeout: 'slow' (not required — does not \
             block), 'gate' (required — blocks the merge); a later result of these is not shown."
        );
    }

    #[test]
    fn an_unusable_interval_falls_back_to_the_shipped_one() {
        assert_eq!(
            seconds_env("ONEPIPELINE_TEST_NEVER_SET_SECONDS", 60.0),
            Duration::from_secs(60)
        );
        for (name, value) in [
            ("ONEPIPELINE_TEST_HUGE_SECONDS", "1e100"),
            ("ONEPIPELINE_TEST_NEGATIVE_SECONDS", "-1"),
            ("ONEPIPELINE_TEST_NAN_SECONDS", "NaN"),
            ("ONEPIPELINE_TEST_WORD_SECONDS", "soon"),
        ] {
            // Names no other test reads, so setting them races nothing.
            std::env::set_var(name, value);
            assert_eq!(seconds_env(name, 60.0), Duration::from_secs(60), "{value}");
        }
        std::env::set_var("ONEPIPELINE_TEST_FRACTION_SECONDS", "0.06");
        assert_eq!(
            seconds_env("ONEPIPELINE_TEST_FRACTION_SECONDS", 60.0),
            Duration::from_millis(60)
        );
    }
}
