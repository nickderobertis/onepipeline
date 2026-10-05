//! Each change's cycle time, aggregated from the signals a run records.
//!
//! No single record is a change's cycle time. It is read off four streams that
//! meet only in a run's merged store: this crate's own `node-dispatched` and
//! `node-settled`, the `oneagentgraph` turn that did the work, and the `onevcs`
//! session that published it — whose `gate-run` records time each gate, and
//! whose landing records carry `landed_at` and `landing`. So the aggregate lives
//! here, where all of them are folded, rather than in a consumer that would have
//! to join three vocabularies to reach it.
//!
//! **A change is a lineage.** A `retry` chain is one change: its id is the
//! chain's last member, its clock starts at the chain's first dispatch, and
//! everything every member recorded is counted toward it.
//!
//! **The segments sum exactly** to the interval they divide — to `cycle_seconds`
//! for a landed change, and to the run's last record minus the first dispatch
//! for one that has not landed — in whole milliseconds, by construction rather
//! than by balancing: every boundary any record states is cut once, and every
//! slice between two cuts is named by exactly one segment. A slice whose records
//! cannot decide what the change was doing is named `other`, and the segment it
//! might have belonged to is listed in `not_measured`; nothing is estimated.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{Envelope, PipelineKind, Source};
use crate::projection::{self, RunState};

/// The schema version of the per-change telemetry document.
pub const CHANGE_TELEMETRY_SCHEMA_VERSION: u32 = 1;

/// Every change one run made, with its cycle time and where that time went.
///
/// What `onepipeline telemetry RUN --changes --json` prints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeTelemetry {
    /// The schema version. Read back only as the one this build writes.
    #[serde(deserialize_with = "this_version")]
    pub schema_version: u32,
    /// The run.
    pub run_id: String,
    /// One entry per lineage that was dispatched, in order of its first dispatch.
    pub changes: Vec<ChangeCycle>,
}

/// Read the version, refusing a document this build cannot honestly read.
fn this_version<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<u32, D::Error> {
    let found = u32::deserialize(reader)?;
    if found != CHANGE_TELEMETRY_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "change telemetry schema_version {found}, and this build reads \
             {CHANGE_TELEMETRY_SCHEMA_VERSION}"
        )));
    }
    Ok(found)
}

/// One change: a lineage of dispatches, from its first to its landing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeCycle {
    /// The lineage's last member: the node that settles the change.
    pub node: String,
    /// Every member, first id first.
    pub lineage: Vec<String>,
    /// The repository identity its sessions published to, as `onevcs` names it;
    /// `null` for a node that never opened a session.
    pub repository: Option<String>,
    /// The branch the change was made on.
    pub branch: Option<String>,
    /// Where a person reads the change request it opened.
    pub change_url: Option<String>,
    /// The settling node's outcome word, or its status where it recorded none.
    // llmlint: ignore[invalid_states_unrepresentable] the outcome vocabulary is open by
    // contract: a settlement's word is this crate's, a publication's or a sibling's
    // classification of a death, and every other document that carries one — `results`,
    // the run reading's `EndedNode`, the summary — carries it as the producer's string,
    // so a word a newer producer settles under reads rather than failing the view.
    pub outcome: String,
    /// The lineage's first `node-dispatched`.
    pub dispatched_at: Stamp,
    /// When the base received it, from the `onevcs` landing record.
    pub landed_at: Option<Stamp>,
    /// The commit it landed at, from the same record.
    pub landing: Option<String>,
    /// `landed_at - dispatched_at`; `null` where the change has not landed.
    pub cycle_seconds: Option<f64>,
    /// `node-dispatched` records across the lineage, re-dispatches included.
    pub dispatches: u64,
    /// Dispatches whose closeout published, re-dispatches after a preserving
    /// failure included.
    pub publication_attempts: u64,
    /// Every `gate-run` across the lineage's sessions, in time order.
    pub gate_runs: Vec<GateRun>,
    /// The sum of [`gate_runs`](Self::gate_runs)' seconds.
    pub gate_seconds: f64,
    /// Where the interval went; the eight sum exactly to it.
    pub segments: Segments,
    /// Each segment the records could not decide. Its time is in `other`.
    pub not_measured: Vec<Segment>,
}

/// An instant as every producer in this stack stamps one: RFC 3339, UTC, to the
/// millisecond — `2026-10-05T03:00:00.000Z`.
///
/// Read only in that shape, so a document carrying anything else is refused
/// where it is read rather than measured as though it were a time.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Stamp(String);

impl Stamp {
    /// The stamp of `millis` since the epoch.
    fn at(millis: u64) -> Self {
        Self(crate::sys::rfc3339_from_millis(millis))
    }

    /// A stamp in the one shape this view reads, or `None`.
    fn read(spelled: &str) -> Option<Self> {
        projection::millis_of(spelled).map(|_| Self(spelled.to_owned()))
    }

    /// As it is spelled.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Milliseconds since the epoch.
    pub fn millis(&self) -> u64 {
        projection::millis_of(&self.0).unwrap_or(0)
    }
}

impl<'de> Deserialize<'de> for Stamp {
    fn deserialize<D: serde::Deserializer<'de>>(reader: D) -> Result<Self, D::Error> {
        let spelled = String::deserialize(reader)?;
        Self::read(&spelled).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "{spelled:?} is not a UTC millisecond stamp, YYYY-MM-DDThh:mm:ss.sssZ"
            ))
        })
    }
}

/// One completed gate run, as `onevcs` recorded it.
///
/// Read through the same checks wherever it is read — off a `gate-run` record,
/// or back out of a document: an interval that ends before it starts, or a
/// `seconds` that is not exactly its two stamps' difference, is refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "GateRunFields")]
pub struct GateRun {
    /// Which gate ran.
    pub gate: GateName,
    /// The publication attempt it belongs to, numbered per session.
    pub attempt: u64,
    /// When it started.
    pub started_at: Stamp,
    /// When it ended.
    pub ended_at: Stamp,
    /// How long it took: exactly `ended_at - started_at`.
    pub seconds: f64,
    /// How it ended.
    pub verdict: GateVerdict,
}

/// A [`GateRun`] as it arrives, before the checks that make it one.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GateRunFields {
    gate: GateName,
    attempt: u64,
    started_at: Stamp,
    ended_at: Stamp,
    seconds: f64,
    verdict: GateVerdict,
}

impl TryFrom<GateRunFields> for GateRun {
    type Error = String;

    fn try_from(fields: GateRunFields) -> Result<Self, String> {
        let (started, ended) = (fields.started_at.millis(), fields.ended_at.millis());
        if ended < started {
            return Err(format!(
                "a gate run ending at {} before it started at {}",
                fields.ended_at.as_str(),
                fields.started_at.as_str()
            ));
        }
        // Exactly, as `onevcs` writes it: the stamps' difference over a thousand.
        if fields.seconds != seconds(ended - started) {
            return Err(format!(
                "a gate run of {} seconds between stamps {}ms apart",
                fields.seconds,
                ended - started
            ));
        }
        Ok(Self {
            gate: fields.gate,
            attempt: fields.attempt,
            started_at: fields.started_at,
            ended_at: fields.ended_at,
            seconds: fields.seconds,
            verdict: fields.verdict,
        })
    }
}

/// Which gate a [`GateRun`] was: `onevcs`'s own two words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateName {
    /// The repository's own `pre-push` hook, run by the publishing push.
    PrePush,
    /// A change request's required checks, watched by the publication.
    RequiredChecks,
}

/// How a [`GateRun`] ended: `onevcs`'s own four words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateVerdict {
    /// Every check passed.
    Passed,
    /// Something it ran refused the change.
    Failed,
    /// The watch's bound elapsed, or a required check ended with no verdict.
    NoVerdict,
    /// Every required check passed or was skipped, and one was skipped.
    PassedWithSkipped,
}

/// Where a change's interval went, in seconds.
///
/// Each is a whole number of milliseconds, and the eight sum exactly to the
/// interval in milliseconds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segments {
    /// A dispatch's own work: from its `node-dispatched` until its agent settled.
    pub agent: f64,
    /// Waiting on a decision: after a settlement that did not finish the change
    /// — a failure, a requeue, a hold — until the lineage's next dispatch.
    pub scheduling: f64,
    /// A gate running, read off each `gate-run`'s own `started_at`/`ended_at`.
    pub gate: f64,
    /// The closeout and publication outside a gate: from the agent settling to
    /// the change request's checks settling or the landing.
    pub publication: f64,
    /// Waiting on a person: a green change kept as a draft for its user's
    /// review, from its checks settling, or a draft its plan held for a person
    /// to mark ready.
    pub review_wait: f64,
    /// Waiting for its turn to land: from `merge-queued` — a host's queue, or a
    /// local landing's — to the landing.
    pub merge_queue: f64,
    /// A change held for a release: a node held waiting on releases, or a
    /// draft opened to wait for a release it pins.
    pub release_wait: f64,
    /// Time the records do not decide: a change request waiting to merge with
    /// nothing recording whether on a reviewer or on its host, a publication in
    /// a run that recorded no gate run, and a change settled `done` without
    /// landing.
    pub other: f64,
}

/// A segment's name, as `not_measured` lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Segment {
    /// [`Segments::agent`].
    Agent,
    /// [`Segments::scheduling`].
    Scheduling,
    /// [`Segments::gate`].
    Gate,
    /// [`Segments::publication`].
    Publication,
    /// [`Segments::review_wait`].
    ReviewWait,
    /// [`Segments::merge_queue`].
    MergeQueue,
    /// [`Segments::release_wait`].
    ReleaseWait,
    /// [`Segments::other`].
    Other,
}

impl Segment {
    /// Every segment, in the order the document and the text name them.
    pub const ALL: [Self; 8] = [
        Self::Agent,
        Self::Scheduling,
        Self::Gate,
        Self::Publication,
        Self::ReviewWait,
        Self::MergeQueue,
        Self::ReleaseWait,
        Self::Other,
    ];

    /// The word this segment is written and rendered as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Scheduling => "scheduling",
            Self::Gate => "gate",
            Self::Publication => "publication",
            Self::ReviewWait => "review_wait",
            Self::MergeQueue => "merge_queue",
            Self::ReleaseWait => "release_wait",
            Self::Other => "other",
        }
    }
}

impl Segments {
    /// One segment's seconds.
    pub fn get(&self, segment: Segment) -> f64 {
        match segment {
            Segment::Agent => self.agent,
            Segment::Scheduling => self.scheduling,
            Segment::Gate => self.gate,
            Segment::Publication => self.publication,
            Segment::ReviewWait => self.review_wait,
            Segment::MergeQueue => self.merge_queue,
            Segment::ReleaseWait => self.release_wait,
            Segment::Other => self.other,
        }
    }

    fn of(ms: &BTreeMap<Segment, u64>) -> Self {
        let at = |segment: Segment| seconds(ms.get(&segment).copied().unwrap_or(0));
        Self {
            agent: at(Segment::Agent),
            scheduling: at(Segment::Scheduling),
            gate: at(Segment::Gate),
            publication: at(Segment::Publication),
            review_wait: at(Segment::ReviewWait),
            merge_queue: at(Segment::MergeQueue),
            release_wait: at(Segment::ReleaseWait),
            other: at(Segment::Other),
        }
    }
}

fn seconds(ms: u64) -> f64 {
    ms as f64 / 1_000.0
}

/// What a change is doing, carried forward from the record that put it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Agent,
    Scheduling,
    Publication,
    ReviewWait,
    MergeQueue,
    ReleaseWait,
    /// A change request whose required checks settled and which has not
    /// merged: waiting on a reviewer or on the host, and nothing recorded says
    /// which.
    AwaitingMerge,
    /// Settled `done` without landing: the run is finished with it, and nothing
    /// it records says what the change waits on now.
    Finished,
}

/// The records a publication writes and nothing else does — what makes a
/// dispatch's closeout a publication attempt rather than a closeout that ended
/// before it reached one.
const PUBLISHING: [&str; 10] = [
    "published",
    "push",
    "gate-run",
    "change-opened",
    "checks-settled",
    "merge-queued",
    "merge-completed",
    "change-merged",
    "sync-conflict",
    "draft-kept-for-review",
];

/// The `onevcs` records of a landing, each carrying `landed_at` and `landing`.
const LANDED: [&str; 2] = ["merge-completed", "change-merged"];

/// A draft is a change request too: once either is open, checks that settle
/// leave the change waiting on its merge rather than on its publication.
const OPENED: [&str; 2] = ["change-opened", "change-drafted"];

/// Every change one run made, folded from its merged store.
pub fn changes_of_run(paths: &crate::ledger::RunPaths, events: &[Envelope]) -> ChangeTelemetry {
    changes(&paths.run, &projection::fold(events), events)
}

/// The same, over a fold the caller already holds.
pub(crate) fn changes(run: &str, state: &RunState, events: &[Envelope]) -> ChangeTelemetry {
    let lineages = lineages(state, events);
    let mut owner: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, lineage) in lineages.iter().enumerate() {
        for member in lineage {
            owner.insert(member.as_str(), index);
        }
    }
    let mut records: Vec<Vec<&Envelope>> = vec![Vec::new(); lineages.len()];
    let mut last = None;
    let mut gates_recorded = false;
    for event in events {
        if let Some(ms) = projection::millis_of(&event.ts) {
            last = Some(last.map_or(ms, |last: u64| last.max(ms)));
        }
        gates_recorded |= event.source == Source::Vcs && event.kind.0 == "gate-run";
        if let Some(index) = event
            .labels
            .node
            .as_deref()
            .and_then(|node| owner.get(node))
        {
            records[*index].push(event);
        }
    }
    let mut changes: Vec<ChangeCycle> = lineages
        .iter()
        .zip(&records)
        .filter_map(|(lineage, records)| {
            cycle(state, lineage, records, last.unwrap_or(0), gates_recorded)
        })
        .collect();
    changes.sort_by(|a, b| (&a.dispatched_at, &a.node).cmp(&(&b.dispatched_at, &b.node)));
    ChangeTelemetry {
        schema_version: CHANGE_TELEMETRY_SCHEMA_VERSION,
        run_id: run.to_owned(),
        changes,
    }
}

/// Every lineage the run holds, first id first: each node no `retry` replaced
/// another with, followed by its replacements.
fn lineages(state: &RunState, events: &[Envelope]) -> Vec<Vec<String>> {
    let replaced: BTreeSet<&str> = state.superseded.values().map(String::as_str).collect();
    let mut nodes: BTreeSet<&str> = state.dispatched_at.keys().map(String::as_str).collect();
    nodes.extend(state.superseded.keys().map(String::as_str));
    nodes.extend(events.iter().filter_map(|event| {
        (PipelineKind::from_wire(&event.kind) == Some(PipelineKind::NodeDispatched))
            .then_some(event.labels.node.as_deref())
            .flatten()
    }));
    nodes
        .into_iter()
        .filter(|node| !replaced.contains(node))
        .map(|root| {
            let mut chain = vec![root.to_owned()];
            let mut at = root;
            // A journal a person edited can hold a cycle; a lineage is walked
            // once whatever it holds.
            while let Some(next) = state.superseded.get(at) {
                if chain.iter().any(|seen| seen == next) {
                    break;
                }
                chain.push(next.clone());
                at = next;
            }
            chain
        })
        .collect()
}

/// One lineage's change, or `None` for a lineage nothing dispatched.
fn cycle(
    state: &RunState,
    lineage: &[String],
    records: &[&Envelope],
    last: u64,
    gates_recorded: bool,
) -> Option<ChangeCycle> {
    let head = lineage.last()?.clone();
    let lifecycle = lineage.iter().any(|member| {
        state
            .graph
            .get(member)
            .is_some_and(|node| node.repo.is_some())
    }) || records.iter().any(|event| event.source == Source::Vcs);

    let mut dispatched_at: Option<u64> = None;
    let mut dispatches = 0;
    let mut publication_attempts = 0;
    // Per member: whether its current dispatch's closeout has published yet.
    let mut publishing: BTreeMap<&str, bool> = BTreeMap::new();
    let mut transitions: Vec<(u64, State)> = Vec::new();
    let mut state_now: Option<State> = None;
    let mut open_members: BTreeSet<String> = BTreeSet::new();
    let mut opened_change = false;
    // What a change request opened as a draft for a reason is waiting on: a
    // person to mark it ready, or a release it pins.
    let mut held_for: Option<State> = None;
    let mut gate_runs: Vec<(u64, GateRun)> = Vec::new();
    let mut gate_spans: Vec<(u64, u64)> = Vec::new();
    let mut landed: Option<(Option<Stamp>, Option<String>)> = None;
    let mut repository: Option<String> = None;
    let mut session_branch: Option<String> = None;
    let mut opened_url: Option<String> = None;

    for event in records {
        let Some(node) = event.labels.node.as_deref() else {
            continue;
        };
        let ms = projection::millis_of(&event.ts);
        let kind = event.kind.0.as_str();
        let mut to: Option<State> = None;
        match event.source {
            Source::Pipeline => match PipelineKind::from_wire(&event.kind) {
                Some(PipelineKind::NodeDispatched) => {
                    dispatches += 1;
                    if let Some(ms) = ms {
                        dispatched_at = Some(dispatched_at.map_or(ms, |first| first.min(ms)));
                    }
                    // A dispatch ending closes the window the previous one of the
                    // same node published in.
                    if publishing.insert(node, false) == Some(true) {
                        publication_attempts += 1;
                    }
                    open_members.clear();
                    to = Some(State::Agent);
                }
                Some(PipelineKind::NodeSettled) => {
                    let outcome = event.payload.get("outcome").and_then(Value::as_str);
                    to = Some(match outcome {
                        Some(crate::vcs::REVIEW_DRAFTED) => State::ReviewWait,
                        Some(crate::vcs::DRAFTED) => held_for.unwrap_or(State::ReleaseWait),
                        // llmlint: ignore[changed_behavior_has_e2e] `queued` is a host's merge
                        // queue taking the change, which the `gh` double offers no queue for;
                        // `a_requeue_and_a_release_wait_after_a_dispatch_are_each_their_own_wait`
                        // drives the arm, and the queue a local landing records is driven end to
                        // end through `merge-queued` in `tests/e2e/change_telemetry.rs`.
                        Some("queued") => State::MergeQueue,
                        Some("change-open") => State::AwaitingMerge,
                        // A failure waits on whoever decides whether to retry it.
                        _ if event.payload.get("status").and_then(Value::as_str)
                            != Some("done") =>
                        {
                            State::Scheduling
                        }
                        // Finished here without landing, and waiting on nothing
                        // the run records.
                        _ => State::Finished,
                    });
                }
                // llmlint: ignore-block[changed_behavior_has_e2e] each is a record of a
                // node *after* its first dispatch: a requeue is an identity admitting no
                // session to a re-dispatch, a hold one on a node whose lineage already ran,
                // and a release wait a fast-adoption node's — none of which a journey can
                // reach without a second repository's release, and every one of which the
                // view reads the way it reads a dispatch. The arms are driven by
                // `a_requeue_and_a_release_wait_after_a_dispatch_are_each_their_own_wait`.
                Some(PipelineKind::NodeRequeued | PipelineKind::NodeHeld) => {
                    to = Some(State::Scheduling);
                }
                Some(PipelineKind::ReleaseWait) => to = Some(State::ReleaseWait),
                // llmlint: ignore-end[changed_behavior_has_e2e]
                _ => {}
            },
            Source::Agentgraph => match kind {
                // llmlint: ignore[changed_behavior_has_e2e] a dispatch of more than one
                // member is a real graph's paid turns; the double runs one member per
                // dispatch, so the journeys drive one, and
                // `a_dispatch_is_the_agents_until_every_member_it_started_has_settled` drives
                // several.
                "member-started" => {
                    if let Some(member) = &event.labels.member {
                        open_members.insert(member.clone());
                    }
                    to = Some(State::Agent);
                }
                crate::report::MEMBER_SETTLED => {
                    if let Some(member) = &event.labels.member {
                        open_members.remove(member);
                    }
                    // A node that publishes nothing is in its dispatch until it
                    // settles; one that does is in its closeout from here.
                    if lifecycle && open_members.is_empty() && state_now == Some(State::Agent) {
                        to = Some(State::Publication);
                    }
                }
                _ => {}
            },
            Source::Vcs => {
                if let Some(identity) = event
                    .labels
                    .extra
                    .get("identity")
                    .or_else(|| event.payload.get("identity"))
                    .and_then(Value::as_str)
                {
                    repository = Some(identity.to_owned());
                }
                if kind == "session-opened" {
                    if let Some(branch) = event.payload.get("branch").and_then(Value::as_str) {
                        session_branch = Some(branch.to_owned());
                    }
                }
                // What the worker does with its own session during its turn is
                // the turn; only the closeout's records are the publication's.
                let closing = state_now.is_some_and(|now| now != State::Agent);
                if closing && PUBLISHING.contains(&kind) {
                    publishing.insert(node, true);
                }
                if OPENED.contains(&kind) {
                    opened_change = true;
                    if let Some(url) = event.payload.get("url").and_then(Value::as_str) {
                        opened_url = Some(url.to_owned());
                    }
                }
                // A lifecycle draft opened while the checks run is no reason to
                // hold anything; the two kinds a draft is held for are.
                if kind == "change-drafted" {
                    match event.payload.get("kind").and_then(Value::as_str) {
                        Some("held") => held_for = Some(State::ReviewWait),
                        Some("awaiting-release") => held_for = Some(State::ReleaseWait),
                        _ => {}
                    }
                }
                if kind == "gate-run" {
                    if let Some(run) = gate_run(&event.payload) {
                        let span = (run.started_at.millis(), run.ended_at.millis());
                        gate_spans.push(span);
                        gate_runs.push((span.0, run));
                    }
                }
                if LANDED.contains(&kind) && landed.is_none() {
                    let landed_at = event
                        .payload
                        .get("landed_at")
                        .and_then(Value::as_str)
                        .and_then(Stamp::read);
                    // llmlint: ignore[changed_behavior_has_e2e] a landing with `sha` and no
                    // `landing` is what an `onevcs` before 0.42.0 wrote, which no build this
                    // crate links can produce; the unit test
                    // `a_landing_an_older_onevcs_recorded_names_its_commit_and_no_time`
                    // drives it, and the journeys drive the current shape.
                    let landing = event
                        .payload
                        .get("landing")
                        .or_else(|| event.payload.get("sha"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    landed = Some((landed_at, landing));
                }
                if closing {
                    to = match kind {
                        // llmlint: ignore[changed_behavior_has_e2e] a draft opened to wait for
                        // a release is a fast-adoption node's, which no journey reaches without
                        // a second repository cutting a release; the `held` kind beside it is
                        // driven by `a_draft_the_plan_held_waits_on_review`, and both by
                        // `a_draft_held_for_a_person_waits_on_review_and_one_awaiting_a_release_on_it`.
                        "checks-settled" if held_for.is_some() => held_for,
                        "checks-settled" if opened_change => Some(State::AwaitingMerge),
                        "draft-kept-for-review" => Some(State::ReviewWait),
                        "merge-queued" => Some(State::MergeQueue),
                        // llmlint: ignore[changed_behavior_has_e2e] each is the publication
                        // going on, so each leaves the change where a publication is; `push`,
                        // `change-opened`, `change-drafted` and `draft-lifted` are driven off real
                        // records by `tests/e2e/change_telemetry.rs`, and a `sync-conflict` — a
                        // base moving under a publication — is one more record of the same state.
                        "push" | "change-opened" | "change-drafted" | "draft-lifted"
                        | "sync-conflict" => Some(State::Publication),
                        _ => None,
                    };
                }
            }
        }
        // A draft kept for its user's review says what the change has been
        // waiting on since its checks settled: the review, not an unexplained
        // wait for its merge.
        if kind == "draft-kept-for-review" {
            if let Some(last) = transitions.last_mut() {
                if last.1 == State::AwaitingMerge {
                    last.1 = State::ReviewWait;
                }
            }
        }
        if let Some(to) = to {
            state_now = Some(to);
            if let Some(ms) = ms {
                transitions.push((ms, to));
            }
        }
    }
    publication_attempts += publishing.values().filter(|published| **published).count() as u64;

    let start = dispatched_at?;
    let (landed_at, landing) = landed.unwrap_or((None, None));
    let landed_ms = landed_at.as_ref().map(Stamp::millis);
    let end = landed_ms.unwrap_or(last).max(start);

    gate_runs.sort_by_key(|(at, _)| *at);
    let gate_runs: Vec<GateRun> = gate_runs.into_iter().map(|(_, run)| run).collect();
    let gate_ms: u64 = gate_runs.iter().map(|run| millis(run.seconds)).sum();

    let (mut spent, awaiting) = slices(start, end, &transitions, &gate_spans);
    let mut not_measured = Vec::new();
    // A store whose `onevcs` recorded no gate run at all cannot say whether a
    // gate ran inside a publication, so neither the gate nor the publication
    // around it is told apart from the other.
    if !gates_recorded {
        not_measured.push(Segment::Gate);
        if lifecycle {
            let undecided = spent.remove(&Segment::Publication).unwrap_or(0);
            *spent.entry(Segment::Other).or_insert(0) += undecided;
            not_measured.push(Segment::Publication);
        }
    }
    // A change request waiting to merge after its checks settled was waiting on
    // a reviewer or on its host, and nothing recorded says which.
    if awaiting > 0 {
        not_measured.push(Segment::ReviewWait);
    }
    not_measured.sort();
    not_measured.dedup();

    let branch = lineage
        .iter()
        .rev()
        .find_map(|member| state.branches.get(member).cloned())
        .or(session_branch);
    // A landing's settlement names no change request, so the lineage's own
    // record of opening one is where a landed change's URL is read.
    let change_url = lineage
        .iter()
        .rev()
        .find_map(|member| state.change_urls.get(member).cloned())
        .or(opened_url);
    let outcome = state.outcomes.get(&head).cloned().unwrap_or_else(|| {
        state
            .recorded
            .get(&head)
            .map_or("pending", |recorded| recorded.status().as_str())
            .to_owned()
    });

    Some(ChangeCycle {
        node: head,
        lineage: lineage.to_vec(),
        repository,
        branch,
        change_url,
        outcome,
        dispatched_at: Stamp::at(start),
        cycle_seconds: landed_ms.map(|_| seconds(end - start)),
        landed_at,
        landing,
        dispatches,
        publication_attempts,
        gate_runs,
        gate_seconds: seconds(gate_ms),
        segments: Segments::of(&spent),
        not_measured,
    })
}

/// Seconds as whole milliseconds, which is the precision `onevcs` writes them at.
fn millis(seconds: f64) -> u64 {
    (seconds * 1_000.0).round().max(0.0) as u64
}

/// One `gate-run` payload, where it reads.
///
/// Through [`GateRun`]'s own checks, over exactly the six fields the document
/// carries — so a payload is held to what a document read back is held to. One
/// this build cannot read — a gate or a verdict a newer `onevcs` names, or an
/// interval that does not hold together — is passed over rather than failing
/// the view.
// llmlint: ignore[changed_behavior_has_e2e] the linked `onevcs` writes only gates and
// verdicts this build names, and only intervals that hold together, so passing one over
// is reachable from no journey. The unit tests
// `a_gate_run_whose_interval_does_not_hold_together_is_passed_over` and
// `a_gate_run_this_build_cannot_read_is_passed_over_rather_than_failing_the_view` drive
// it, and the journeys drive `pre-push`, `required-checks`, `passed`, `failed` and
// `passed-with-skipped` off real records. `no-verdict` is a watch's bound elapsing, which
// re-dispatches until a budget is spent — a word this reads, not a branch it takes.
fn gate_run(payload: &serde_json::Map<String, Value>) -> Option<GateRun> {
    let fields: serde_json::Map<String, Value> = [
        "gate",
        "attempt",
        "started_at",
        "ended_at",
        "seconds",
        "verdict",
    ]
    .into_iter()
    .filter_map(|name| Some((name.to_owned(), payload.get(name)?.clone())))
    .collect();
    serde_json::from_value(Value::Object(fields)).ok()
}

/// Divide `[start, end)` among the segments, exactly.
///
/// Every transition and every gate boundary inside the interval is a cut; each
/// slice between two cuts is a gate's where a gate run covers it, and otherwise
/// the state the last transition at or before it put the change in. A slice
/// before any transition — which a well-formed store never has, because the
/// interval opens at a dispatch — is `other`. Beside the division, how much of
/// `other` was a change request awaiting its merge.
fn slices(
    start: u64,
    end: u64,
    transitions: &[(u64, State)],
    gates: &[(u64, u64)],
) -> (BTreeMap<Segment, u64>, u64) {
    let mut sorted = transitions.to_vec();
    sorted.sort_by_key(|(at, _)| *at);
    let mut cuts: Vec<u64> = vec![start, end];
    cuts.extend(sorted.iter().map(|(at, _)| *at));
    cuts.extend(gates.iter().flat_map(|(from, to)| [*from, *to]));
    cuts.retain(|at| (start..=end).contains(at));
    cuts.sort_unstable();
    cuts.dedup();

    let mut spent: BTreeMap<Segment, u64> = BTreeMap::new();
    let mut awaiting = 0;
    let mut next = 0;
    let mut now: Option<State> = None;
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        while next < sorted.len() && sorted[next].0 <= from {
            now = Some(sorted[next].1);
            next += 1;
        }
        let segment = if gates.iter().any(|(s, e)| *s <= from && to <= *e) {
            Segment::Gate
        } else {
            match now {
                Some(State::Agent) => Segment::Agent,
                Some(State::Scheduling) => Segment::Scheduling,
                Some(State::Publication) => Segment::Publication,
                Some(State::ReviewWait) => Segment::ReviewWait,
                Some(State::MergeQueue) => Segment::MergeQueue,
                Some(State::ReleaseWait) => Segment::ReleaseWait,
                Some(State::AwaitingMerge) => {
                    awaiting += to - from;
                    Segment::Other
                }
                Some(State::Finished) | None => Segment::Other,
            }
        };
        *spent.entry(segment).or_insert(0) += to - from;
    }
    (spent, awaiting)
}

/// The text `telemetry RUN --changes` prints: one line per change.
pub fn render_changes(telemetry: &ChangeTelemetry) -> String {
    let mut out = String::new();
    for change in &telemetry.changes {
        let cycle = match change.cycle_seconds {
            Some(cycle) => format!("cycle {}", crate::telemetry::duration(millis(cycle))),
            None => "not landed".to_owned(),
        };
        let mut line = format!(
            "{}  {}  {}  {cycle}  dispatches {}  publications {}  gates {} ({})",
            change.node,
            change.repository.as_deref().unwrap_or("-"),
            change.outcome,
            change.dispatches,
            change.publication_attempts,
            change.gate_runs.len(),
            crate::telemetry::duration(millis(change.gate_seconds)),
        );
        for segment in Segment::ALL {
            let shown = if change.not_measured.contains(&segment) {
                crate::telemetry::UNMEASURED.to_owned()
            } else {
                crate::telemetry::duration(millis(change.segments.get(segment)))
            };
            line.push_str(&format!("  {} {shown}", segment.as_str()));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, Labels, ENVELOPE_VERSION};
    use serde_json::json;

    /// One record at `seconds` past a fixed instant, under node `service`.
    fn at(seconds: u64, source: Source, kind: &str, payload: Value) -> Envelope {
        Envelope {
            v: ENVELOPE_VERSION,
            ts: stamp(seconds),
            stream: format!("{source:?}"),
            seq: seconds,
            source,
            dimensions: Default::default(),
            kind: EventKind(kind.into()),
            labels: Labels {
                run_id: Some("demo".into()),
                node: Some("service".into()),
                ..Labels::default()
            },
            payload: match payload {
                Value::Object(fields) => fields,
                _ => Default::default(),
            },
            artifacts: Vec::new(),
        }
    }

    fn stamp(seconds: u64) -> String {
        crate::sys::rfc3339_from_millis(1_786_000_000_000 + seconds * 1_000)
    }

    fn pipeline(seconds: u64, kind: &str, payload: Value) -> Envelope {
        at(seconds, Source::Pipeline, kind, payload)
    }

    fn vcs(seconds: u64, kind: &str, payload: Value) -> Envelope {
        at(seconds, Source::Vcs, kind, payload)
    }

    fn settled_agent(seconds: u64) -> Envelope {
        at(
            seconds,
            Source::Agentgraph,
            crate::report::MEMBER_SETTLED,
            json!({}),
        )
    }

    fn gate(from: u64, to: u64, gate: &str, verdict: &str) -> Envelope {
        vcs(
            to,
            "gate-run",
            json!({"gate": gate, "attempt": 1, "started_at": stamp(from),
                   "ended_at": stamp(to), "seconds": (to - from) as f64,
                   "verdict": verdict, "checks": []}),
        )
    }

    /// A change request's publication up to its checks settling at 20s.
    fn opened_and_checked() -> Vec<Envelope> {
        vec![
            pipeline(0, "node-dispatched", json!({"attempt": 1})),
            vcs(1, "session-opened", json!({"branch": "onevcs/s-1"})),
            settled_agent(10),
            vcs(12, "push", json!({"accepted": true})),
            vcs(
                13,
                "change-opened",
                json!({"url": "https://example.test/pull/1"}),
            ),
            gate(13, 20, "required-checks", "passed"),
            vcs(20, "checks-settled", json!({"verdict": "passed"})),
        ]
    }

    fn one(events: &[Envelope]) -> ChangeCycle {
        let mut document = changes("demo", &projection::fold(events), events);
        assert_eq!(document.changes.len(), 1, "{document:?}");
        document.changes.remove(0)
    }

    fn ms_of(change: &ChangeCycle) -> BTreeMap<Segment, u64> {
        Segment::ALL
            .into_iter()
            .map(|segment| (segment, millis(change.segments.get(segment))))
            .collect()
    }

    fn sum(change: &ChangeCycle) -> u64 {
        ms_of(change).values().sum()
    }

    #[test]
    fn a_change_request_awaiting_its_merge_with_nothing_saying_why_is_other_and_review_unmeasured()
    {
        let mut events = opened_and_checked();
        events.push(vcs(
            51,
            "merge-completed",
            json!({"sha": "abc", "landing": "abc", "landed_at": stamp(50)}),
        ));
        let change = one(&events);
        assert_eq!(change.cycle_seconds, Some(50.0));
        assert_eq!(change.landing.as_deref(), Some("abc"));
        assert_eq!(change.publication_attempts, 1);
        let ms = ms_of(&change);
        assert_eq!(ms[&Segment::Agent], 10_000);
        assert_eq!(ms[&Segment::Publication], 3_000);
        assert_eq!(ms[&Segment::Gate], 7_000);
        assert_eq!(ms[&Segment::Other], 30_000);
        assert_eq!(change.not_measured, vec![Segment::ReviewWait]);
        assert_eq!(sum(&change), 50_000);
    }

    #[test]
    fn a_draft_kept_for_review_and_a_merge_queue_are_each_their_own_wait() {
        let mut events = opened_and_checked();
        events.push(vcs(22, "draft-kept-for-review", json!({})));
        events.push(vcs(40, "draft-lifted", json!({})));
        events.push(vcs(41, "merge-queued", json!({})));
        events.push(vcs(
            50,
            "change-merged",
            json!({"sha": "abc", "landing": "abc", "landed_at": stamp(50)}),
        ));
        let change = one(&events);
        let ms = ms_of(&change);
        // Kept for review is what the change waited on from its checks settling
        // at 20s, until the draft was lifted at 40s.
        assert_eq!(ms[&Segment::ReviewWait], 20_000);
        assert_eq!(ms[&Segment::MergeQueue], 9_000);
        assert_eq!(ms[&Segment::Other], 0);
        assert_eq!(ms[&Segment::Publication], 3_000 + 1_000);
        assert!(change.not_measured.is_empty(), "{:?}", change.not_measured);
        assert_eq!(sum(&change), 50_000);
    }

    #[test]
    fn a_change_left_a_draft_for_its_release_waits_until_the_runs_last_record() {
        let mut events = opened_and_checked();
        events.push(pipeline(
            21,
            "node-settled",
            json!({"status": "complete-but-draft", "outcome": crate::vcs::DRAFTED}),
        ));
        events.push(pipeline(60, "release-wait", json!({})));
        // A record of another node is still the run's last record.
        let mut elsewhere = pipeline(70, "node-ready", json!({}));
        elsewhere.labels.node = Some("another".into());
        events.push(elsewhere);
        let change = one(&events);
        assert_eq!(change.cycle_seconds, None);
        assert_eq!(change.landed_at, None);
        let ms = ms_of(&change);
        assert_eq!(ms[&Segment::ReleaseWait], 49_000);
        assert_eq!(ms[&Segment::Other], 1_000);
        assert_eq!(sum(&change), 70_000);
    }

    #[test]
    fn a_failure_waits_on_scheduling_and_a_done_change_that_did_not_land_is_other() {
        let events = vec![
            pipeline(0, "node-dispatched", json!({"attempt": 1})),
            settled_agent(5),
            pipeline(
                6,
                "node-settled",
                json!({"status": "failed", "outcome": "task-failed"}),
            ),
            pipeline(16, "node-dispatched", json!({"attempt": 2})),
            settled_agent(20),
            pipeline(21, "node-settled", json!({"status": "done"})),
            pipeline(30, "run-stopped", json!({})),
        ];
        let change = one(&events);
        // No `onevcs` record and no repository in the graph: a direct node.
        assert_eq!(change.repository, None);
        assert_eq!(change.dispatches, 2);
        assert_eq!(change.publication_attempts, 0);
        let ms = ms_of(&change);
        assert_eq!(ms[&Segment::Agent], 5_000 + 1_000 + 5_000);
        assert_eq!(ms[&Segment::Scheduling], 10_000);
        assert_eq!(ms[&Segment::Other], 9_000);
        assert_eq!(sum(&change), 30_000);
        // The run recorded no gate run, and a direct node has no publication
        // for one to have hidden in.
        assert_eq!(change.not_measured, vec![Segment::Gate]);
    }

    #[test]
    fn a_requeue_and_a_release_wait_after_a_dispatch_are_each_their_own_wait() {
        let events = vec![
            pipeline(0, "node-dispatched", json!({"attempt": 1})),
            pipeline(2, "node-requeued", json!({"reason": "workspace-exhausted"})),
            pipeline(5, "node-dispatched", json!({"attempt": 1})),
            settled_agent(9),
            pipeline(
                10,
                "node-settled",
                json!({"status": "done", "outcome": "queued"}),
            ),
            pipeline(14, "release-wait", json!({})),
            pipeline(20, "node-dispatched", json!({"attempt": 2})),
            settled_agent(21),
            pipeline(22, "node-settled", json!({"status": "done"})),
        ];
        let change = one(&events);
        let ms = ms_of(&change);
        // A direct node is in its dispatch until it settles.
        assert_eq!(ms[&Segment::Agent], 2_000 + 5_000 + 2_000);
        assert_eq!(ms[&Segment::Scheduling], 3_000);
        assert_eq!(ms[&Segment::MergeQueue], 4_000);
        assert_eq!(ms[&Segment::ReleaseWait], 6_000);
        assert_eq!(change.dispatches, 3);
        assert_eq!(sum(&change), 22_000);
    }

    #[test]
    fn a_dispatch_is_the_agents_until_every_member_it_started_has_settled() {
        let member = |seconds: u64, kind: &str, name: &str| {
            let mut event = at(seconds, Source::Agentgraph, kind, json!({}));
            event.labels.member = Some(name.into());
            event
        };
        let events = vec![
            pipeline(0, "node-dispatched", json!({"attempt": 1})),
            member(1, "member-started", "worker"),
            member(2, "member-started", "reviewer"),
            member(6, crate::report::MEMBER_SETTLED, "worker"),
            member(9, crate::report::MEMBER_SETTLED, "reviewer"),
            vcs(10, "push", json!({"accepted": true})),
            vcs(
                11,
                "merge-completed",
                json!({"landing": "abc", "landed_at": stamp(11)}),
            ),
        ];
        let change = one(&events);
        let ms = ms_of(&change);
        assert_eq!(ms[&Segment::Agent], 9_000);
        assert_eq!(change.cycle_seconds, Some(11.0));
    }

    #[test]
    fn a_draft_held_for_a_person_waits_on_review_and_one_awaiting_a_release_on_it() {
        for (kind, waits) in [
            ("held", Segment::ReviewWait),
            ("awaiting-release", Segment::ReleaseWait),
        ] {
            let mut events = opened_and_checked();
            events.insert(4, vcs(13, "change-drafted", json!({"kind": kind})));
            events.push(pipeline(
                21,
                "node-settled",
                json!({"status": "complete-but-draft", "outcome": crate::vcs::DRAFTED}),
            ));
            events.push(pipeline(40, "run-stopped", json!({})));
            let change = one(&events);
            let ms = ms_of(&change);
            assert_eq!(ms[&waits], 20_000, "{kind}");
            assert_eq!(ms[&Segment::Other], 0, "{kind}");
            assert!(
                !change.not_measured.contains(&Segment::ReviewWait),
                "{kind}"
            );
        }
    }

    #[test]
    fn a_gate_run_whose_interval_does_not_hold_together_is_passed_over() {
        let mut events = opened_and_checked();
        events.push(vcs(
            30,
            "gate-run",
            json!({"gate": "pre-push", "attempt": 2, "started_at": stamp(29),
                   "ended_at": stamp(25), "seconds": -4.0, "verdict": "passed"}),
        ));
        events.push(vcs(
            31,
            "gate-run",
            json!({"gate": "pre-push", "attempt": 2, "started_at": stamp(25),
                   "ended_at": stamp(29), "seconds": 9.5, "verdict": "passed"}),
        ));
        let change = one(&events);
        assert_eq!(change.gate_runs.len(), 1);
        assert_eq!(change.gate_seconds, 7.0);
    }

    #[test]
    fn a_landing_an_older_onevcs_recorded_names_its_commit_and_no_time() {
        let events = vec![
            pipeline(0, "node-dispatched", json!({"attempt": 1})),
            settled_agent(10),
            vcs(12, "push", json!({"accepted": true})),
            vcs(13, "merge-completed", json!({"sha": "abc"})),
        ];
        let change = one(&events);
        assert_eq!(change.landing.as_deref(), Some("abc"));
        assert_eq!(change.landed_at, None);
        assert_eq!(change.cycle_seconds, None);
        assert!(change.gate_runs.is_empty());
        assert_eq!(
            change.not_measured,
            vec![Segment::Gate, Segment::Publication]
        );
        let ms = ms_of(&change);
        assert_eq!(ms[&Segment::Agent], 10_000);
        assert_eq!(ms[&Segment::Other], 3_000);
        assert_eq!(sum(&change), 13_000);
    }

    #[test]
    fn a_gate_run_this_build_cannot_read_is_passed_over_rather_than_failing_the_view() {
        let mut events = opened_and_checked();
        events.push(gate(21, 25, "a-gate-from-the-future", "passed"));
        let change = one(&events);
        assert_eq!(change.gate_runs.len(), 1);
        assert_eq!(change.gate_runs[0].gate, GateName::RequiredChecks);
        assert_eq!(change.gate_seconds, 7.0);
    }

    #[test]
    fn the_document_round_trips_and_refuses_another_version() {
        let mut events = opened_and_checked();
        events.push(vcs(
            30,
            "merge-completed",
            json!({"landing": "abc", "landed_at": stamp(30)}),
        ));
        let document = changes("demo", &projection::fold(&events), &events);
        let written = serde_json::to_value(&document).expect("it serialises");
        let read: ChangeTelemetry = serde_json::from_value(written.clone()).expect("it reads back");
        assert_eq!(read, document);
        let mut later = written;
        later["schema_version"] = json!(2);
        let refused = serde_json::from_value::<ChangeTelemetry>(later).expect_err("refused");
        assert!(
            refused.to_string().contains("schema_version 2"),
            "{refused}"
        );
    }

    #[test]
    fn the_text_is_one_line_per_change_naming_what_was_not_measured() {
        let events = vec![
            pipeline(0, "node-dispatched", json!({"attempt": 1})),
            settled_agent(90),
            pipeline(91, "node-settled", json!({"status": "done"})),
        ];
        let document = changes("demo", &projection::fold(&events), &events);
        assert_eq!(
            render_changes(&document),
            "service  -  done  not landed  dispatches 1  publications 0  gates 0 (0s)  \
             agent 1m31s  scheduling 0s  gate not measured  publication 0s  review_wait 0s  \
             merge_queue 0s  release_wait 0s  other 0s\n"
        );
    }
}
