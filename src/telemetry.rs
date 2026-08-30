//! Session timing and usage, aggregated from the merged event store.
//!
//! The one property that makes this view usable is that **the buckets sum
//! exactly to WALL**. A breakdown whose parts do not add up to the whole cannot
//! answer "where did the time go?", which is the only question it is for — so
//! the residue is a bucket of its own rather than a rounding error hidden in the
//! others.
//!
//! Time is attributed over the run's *wall clock*, not by adding up
//! per-dispatch durations: nodes overlap, so a sum of durations exceeds the
//! elapsed time and the answer stops meaning anything. Where two nodes are doing
//! different things across one millisecond, the millisecond is named by the
//! **more specific** of the two — which is what keeps publication time and lock
//! waiting separable from agent time instead of buried in it.
//!
//! A bucket or a party nothing in the stack measures is served **absent**. A
//! zero reads as a measurement, and an unmeasured span reported as zero is the
//! answer that makes a run look cheaper than it was.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use oneagentgraph::event::Role;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{Envelope, Source};
use crate::graph::NodeStatus;
use crate::journal;
use crate::ledger::RunPaths;
use crate::projection;

/// The schema version of the telemetry document.
///
/// `2` widened the four buckets this crate shipped to the eight the contract
/// names, and added per-party [`usage`](RunTelemetry::usage). Both are breaking
/// changes to the document: a consumer filtering on `dispatching` finds no such
/// bucket under `2`.
pub const TELEMETRY_SCHEMA_VERSION: u32 = 2;

/// One run's timing and usage, with a breakdown that sums exactly to its wall
/// clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTelemetry {
    /// The schema version. Read back only as the one this build writes.
    #[serde(deserialize_with = "this_version")]
    pub schema_version: u32,
    /// The run.
    pub run_id: String,
    /// The whole elapsed time, in milliseconds.
    pub wall_ms: u64,
    /// The buckets, whose measured spans sum exactly to
    /// [`wall_ms`](Self::wall_ms). Exactly the eight
    /// [`BucketName::ALL`] names, once each, in that order.
    #[serde(deserialize_with = "every_bucket")]
    pub buckets: Vec<Bucket>,
    /// What each party spent. A party nothing reported for is absent from the
    /// map rather than present and zero.
    pub usage: BTreeMap<Party, Usage>,
    /// How many dispatches the run started.
    pub dispatches: u64,
    /// How many of them settled `done`.
    pub settled_done: u64,
    /// How many nodes settled without a dispatch because they expected no diff.
    pub no_diff: u64,
    /// How many surfaces were sent, and how many a planner read.
    pub surfaces_queued: u64,
    /// Surfaces a planner actually consumed.
    pub surfaces_read: u64,
}

/// Read the version, refusing a document this build cannot honestly read.
///
/// The number is the whole compatibility statement, so a reader that took any
/// value would be honouring none of it: schema `1` named four spans —
/// `dispatching`, `awaiting-planner`, `awaiting-human`, `orchestrating` — and
/// carried no `usage` at all, and reading one as a `2` would drop a span or
/// report a run as having spent nothing. Refused by name, with both numbers.
fn this_version<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<u32, D::Error> {
    let found = u32::deserialize(reader)?;
    if found != TELEMETRY_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "telemetry schema_version {found}, and this build reads \
             {TELEMETRY_SCHEMA_VERSION}"
        )));
    }
    Ok(found)
}

/// Read the buckets, refusing any set that is not the eight.
///
/// The document's one property is that its measured spans sum exactly to the
/// wall clock, and that means nothing over a set that may be missing a bucket,
/// carry one twice, or arrive in another order — a consumer indexing the eighth
/// would be reading whichever happened to be there.
fn every_bucket<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<Vec<Bucket>, D::Error> {
    let found = Vec::<Bucket>::deserialize(reader)?;
    let named: Vec<BucketName> = found.iter().map(|bucket| bucket.name).collect();
    if named != BucketName::ALL {
        return Err(serde::de::Error::custom(format!(
            "buckets {:?}, and a telemetry document carries exactly {:?}",
            named.iter().map(|name| name.as_str()).collect::<Vec<_>>(),
            BucketName::ALL
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
        )));
    }
    Ok(found)
}

/// One span of the run's wall clock, named by what the run was doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bucket {
    /// What the run was doing.
    pub name: BucketName,
    /// For how long, in milliseconds — absent when nothing in the stack
    /// measures this bucket, which is not the same fact as a measured zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
}

/// What a run's wall clock is spent on.
///
/// Closed on purpose: the measured buckets sum *exactly* to the wall clock, and
/// that invariant only holds while every millisecond has one of a known set of
/// homes. A bucket named by a free string could be added without anything
/// noticing that the parts no longer add up to the whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BucketName {
    /// Wall time with at least one agent dispatch in flight and nothing more
    /// specific happening.
    Agent,
    /// Wall time a judge side of a dispatch was running.
    Judge,
    /// Wall time an LLM-lint pass was running.
    Llmlint,
    /// Wall time a repository's own verification gate was running.
    ///
    /// **Served absent by every run this build produces.** Nothing in the stack
    /// runs a gate any more: `onevcs` names none, and what verifies a change is
    /// the repository's own merge path — the host's required checks, or the
    /// `pre-push` hook git runs at the publishing push, whose wall time is the
    /// publication's. The bucket is kept because the contract fixes the eight,
    /// and it is still filled from a store an older `onevcs` wrote.
    Gate,
    /// Wall time a publication was in progress — the push, the change request,
    /// the checks, and the merge.
    PublicationWait,
    /// Wall time blocked on a repository identity's lock.
    LockWait,
    /// Wall time preparing a workspace: the clone, the worktree, the fetch.
    Setup,
    /// Everything else the run's own clock covers — waiting on a decision point
    /// or on a person, and the gaps between dispatches.
    Scheduling,
}

impl BucketName {
    /// Every bucket, in the order the breakdown renders them.
    pub const ALL: [Self; 8] = [
        Self::Agent,
        Self::Judge,
        Self::Llmlint,
        Self::Gate,
        Self::PublicationWait,
        Self::LockWait,
        Self::Setup,
        Self::Scheduling,
    ];

    /// The word this bucket is written and rendered as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Judge => "judge",
            Self::Llmlint => "llmlint",
            Self::Gate => "gate",
            Self::PublicationWait => "publication_wait",
            Self::LockWait => "lock_wait",
            Self::Setup => "setup",
            Self::Scheduling => "scheduling",
        }
    }
}

/// Who spent a run's tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Party {
    /// The side doing the work.
    Agent,
    /// The side supervising it.
    Judge,
    /// The LLM-lint pass.
    Llmlint,
    /// Everything the run spent, however it was split.
    Total,
}

impl Party {
    /// Every party, in the order the breakdown renders them.
    pub const ALL: [Self; 4] = [Self::Agent, Self::Judge, Self::Llmlint, Self::Total];

    /// The word this party is written and rendered as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Judge => "judge",
            Self::Llmlint => "llmlint",
            Self::Total => "total",
        }
    }
}

/// What one party consumed.
///
/// Every field is independently optional, and `None` means **no signal** rather
/// than zero: not every harness reports every number — cost is commonly absent
/// on subscription auth, and cache counts only where the provider surfaces them
/// — and a run whose cost cannot be answered must not read as a run that was
/// free.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    /// Input tokens billed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    /// Output tokens billed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
    /// Prompt tokens served from the provider's cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<u64>,
    /// Prompt tokens written to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<u64>,
    /// What it cost, in US dollars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl Usage {
    /// Whether nothing at all was reported.
    pub fn is_empty(&self) -> bool {
        self.input.is_none()
            && self.output.is_none()
            && self.cache_read.is_none()
            && self.cache_write.is_none()
            && self.cost_usd.is_none()
    }

    /// Fold another reading in. A field stays absent until something reports a
    /// real number for it, and accumulates from there.
    pub fn add(&mut self, other: &Self) {
        let sum = |into: &mut Option<u64>, value: Option<u64>| {
            if let Some(value) = value {
                *into = Some(into.unwrap_or(0).saturating_add(value));
            }
        };
        sum(&mut self.input, other.input);
        sum(&mut self.output, other.output);
        sum(&mut self.cache_read, other.cache_read);
        sum(&mut self.cache_write, other.cache_write);
        if let Some(cost) = other.cost_usd {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + cost);
        }
    }

    /// Read one usage object off a sibling's payload.
    ///
    /// Two spellings, because the stack has had two. The linked `oneagentgraph`
    /// declares and emits `input_tokens` and `cost_usd` — the same names the
    /// onejudge report has always carried — but a build before 0.3.6 spelled the
    /// same numbers `tokens_in` and `cost` on that payload, and a run's journal
    /// outlives the build that wrote it: `telemetry` reads a store recorded
    /// weeks ago as readily as a live one. Reading both means neither stream
    /// goes silently unaccounted, which for a host whose routine failure is
    /// quota exhaustion is the whole point of the number.
    fn of(value: &Value) -> Self {
        let count = |names: [&str; 2]| {
            names
                .iter()
                .find_map(|name| value.get(*name).and_then(Value::as_u64))
        };
        Self {
            input: count(["input_tokens", "tokens_in"]),
            output: count(["output_tokens", "tokens_out"]),
            cache_read: count(["cache_read_tokens", "cache_read"]),
            cache_write: count(["cache_write_tokens", "cache_write"]),
            cost_usd: ["cost_usd", "cost"]
                .iter()
                .find_map(|name| value.get(*name).and_then(Value::as_f64)),
        }
    }
}

/// What one repository session is doing, from its own stream.
///
/// A session records the phases of a node's workspace and publication, and its
/// last record is what it is doing until the next one — so this is carried
/// forward rather than derived per event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Preparing the workspace: the clone, the worktree, the fetch.
    Setup,
    /// Blocked on the identity's lock.
    LockWait,
    /// The repository's own verification gate is running.
    ///
    /// Reachable only from a store an older `onevcs` wrote: no release since the
    /// merge path became the only verifier emits the kinds below.
    Gate,
    /// Pushing, opening the change, waiting on its checks, merging.
    Publication,
}

impl Phase {
    /// The phase a `onevcs` event puts its session in, when it names one.
    ///
    /// `None` for a kind that ends the session or that this build does not
    /// know: the session keeps whatever it was doing, because a kind a newer
    /// sibling emits is not evidence that the work stopped.
    fn of(kind: &str) -> Option<Self> {
        match kind {
            "session-opened" | "fetch" | "commit-preserved" | "lock-acquired"
            | "recovery-attested" => Some(Self::Setup),
            "lock-wait" => Some(Self::LockWait),
            "gate-started" => Some(Self::Gate),
            // The verdict is the gate ending, and what followed a passed gate was
            // the publication it gated. Both kinds are read for a store an older
            // `onevcs` wrote; no release since the merge path became the only
            // verifier emits either.
            "gate-verdict" | "push" | "change-opened" | "change-check" | "merge-queued"
            | "change-merged" | "merge-completed" | "sync-conflict" => Some(Self::Publication),
            _ => None,
        }
    }

    /// The bucket this phase's wall time belongs to.
    fn bucket(self) -> BucketName {
        match self {
            Self::Setup => BucketName::Setup,
            Self::LockWait => BucketName::LockWait,
            Self::Gate => BucketName::Gate,
            Self::Publication => BucketName::PublicationWait,
        }
    }

    /// The order a millisecond is named in when two sessions disagree: the more
    /// specific state wins, blocked before working.
    const PRECEDENCE: [Self; 4] = [Self::LockWait, Self::Gate, Self::Publication, Self::Setup];
}

/// The kind that begins a session.
const SESSION_OPENED: &str = "session-opened";

/// The kind that ends a session, whatever it was doing.
const SESSION_CLOSED: &str = "session-closed";

/// The kinds that say a dispatch's own agent is working in the workspace.
///
/// One of these ends whatever phase the session was in: the workspace is ready
/// and the turn is running, so the time is the agent's and not the setup's.
const WORKING: [&str; 5] = [
    "member-started",
    "turn-started",
    "turn-activity",
    "turn-completed",
    crate::report::MEMBER_SETTLED,
];

/// The payload key naming which side of a two-party conversation an event came
/// from.
const ROLE: &str = "role";

/// Which side of a two-party member a relayed event came from, when its
/// producer said.
///
/// Parsed through **`oneagentgraph`'s own `Role`** rather than matched as a
/// string: the vocabulary is that library's, it is a direct dependency, and a
/// side it renames is then a compile error here rather than a branch that
/// silently stops matching. Nothing stamps this on a turn today — divergence 10
/// in `docs/contract-divergences.md` is the proposal that it should — so this is
/// what a producer that starts is read with.
fn side_of(event: &Envelope) -> Option<Role> {
    serde_json::from_value(event.payload.get(ROLE)?.clone()).ok()
}

/// Aggregate one run's telemetry from its merged event store.
///
/// The per-party split is read from the onejudge reports the run's
/// `member-settled` events named — which is where a two-party member records
/// what each side spent — through **this run's own copies** of them, so a
/// document a journal line points at is never opened. A report the run did not
/// keep contributes nothing rather than a zero.
pub fn of_run(paths: &RunPaths, events: &[Envelope]) -> RunTelemetry {
    let state = projection::fold(events);
    let stamps: Vec<(u64, &Envelope)> = events
        .iter()
        .filter_map(|event| projection::millis_of(&event.ts).map(|ms| (ms, event)))
        .collect();

    let first = stamps.first().map_or(0, |(ms, _)| *ms);
    let last = stamps.last().map_or(first, |(ms, _)| *ms);
    let wall_ms = last.saturating_sub(first);

    // Walk the timeline once, attributing each span between consecutive events
    // to whatever the run was doing across it. Every millisecond of the wall
    // clock lands in exactly one bucket, which is what makes the sum exact.
    let mut totals: BTreeMap<BucketName, u64> = BTreeMap::new();
    let mut dispatched: BTreeSet<String> = BTreeSet::new();
    let mut phases: BTreeMap<String, Phase> = BTreeMap::new();
    let mut judging: BTreeSet<String> = BTreeSet::new();
    let mut judge_measured = false;
    let mut gate_measured = false;
    let mut previous = first;

    for (ms, event) in &stamps {
        let span = ms.saturating_sub(previous);
        if span > 0 {
            *totals
                .entry(now(&phases, &judging, &dispatched))
                .or_insert(0) += span;
        }
        previous = *ms;

        // A session is keyed by the node it belongs to where one is known, and
        // by its own stream where it is not: both are stable for the session's
        // whole life, which is what a phase carried forward needs.
        let whose = event
            .labels
            .node
            .clone()
            .unwrap_or_else(|| event.stream.clone());
        match event.source {
            Source::Vcs => match event.kind.0.as_str() {
                SESSION_CLOSED => {
                    phases.remove(&whose);
                }
                // A session opening is the *start* of a session's life, so it
                // never moves one already under way backwards. A node holds more
                // than one session — its steps' and the one the change request
                // is drafted in — and there are no cross-stream ordering
                // promises inside a millisecond, so a second `session-opened`
                // landing after a `lock-wait` is a tie the merge broke, not a
                // publication that went back to preparing its workspace.
                SESSION_OPENED => {
                    phases.entry(whose).or_insert(Phase::Setup);
                }
                kind => {
                    if let Some(phase) = Phase::of(kind) {
                        gate_measured |= phase == Phase::Gate;
                        phases.insert(whose, phase);
                    }
                }
            },
            Source::Agentgraph if WORKING.contains(&event.kind.0.as_str()) => {
                // The workspace is ready and a turn is running in it.
                phases.remove(&whose);
                match side_of(event) {
                    Some(Role::Judge) => {
                        judge_measured = true;
                        judging.insert(whose);
                    }
                    // A turn the producer attributed to the agent side ends the
                    // judge's, as does the member settling.
                    Some(Role::Agent) => {
                        judge_measured = true;
                        judging.remove(&whose);
                    }
                    None if event.kind.0 == crate::report::MEMBER_SETTLED => {
                        judging.remove(&whose);
                    }
                    None => {}
                }
            }
            Source::Pipeline => match journal::PipelineKind::from_wire(&event.kind) {
                Some(journal::PipelineKind::NodeDispatched) => {
                    dispatched.insert(whose);
                }
                Some(journal::PipelineKind::NodeSettled) => {
                    dispatched.remove(&whose);
                    judging.remove(&whose);
                    phases.remove(&whose);
                }
                _ => {}
            },
            Source::Agentgraph => {}
        }
    }

    let measured = |name: BucketName| match name {
        // Nothing in this stack runs an LLM-lint pass, so its bucket is
        // unmeasured rather than zero.
        BucketName::Llmlint => None,
        // The merged stream distinguishes a judge-side turn only where the
        // producer stamped one. Absent, the judge's time is inside `agent` and
        // saying `0` here would claim it was not spent.
        BucketName::Judge if !judge_measured => None,
        // Nothing in this stack runs a gate: what verifies a change is the
        // repository's own merge path, whose wall time is the publication's. So
        // a run this build produces measures no gate at all, and a `0` would
        // claim a tier ran and cost nothing rather than that none ran. A store
        // an older `onevcs` wrote still carries the records, and there the
        // bucket is a measurement again.
        BucketName::Gate if !gate_measured => None,
        _ => Some(totals.get(&name).copied().unwrap_or(0)),
    };
    let mut buckets: Vec<Bucket> = BucketName::ALL
        .into_iter()
        .map(|name| Bucket {
            name,
            ms: measured(name),
        })
        .collect();
    balance(&mut buckets, wall_ms);

    RunTelemetry {
        schema_version: TELEMETRY_SCHEMA_VERSION,
        run_id: paths.run.clone(),
        wall_ms,
        buckets,
        usage: usage_of(paths, events),
        dispatches: state.dispatched_at.len() as u64,
        settled_done: state
            .recorded
            .values()
            .filter(|recorded| recorded.status() == NodeStatus::Done)
            .count() as u64,
        no_diff: state
            .outcomes
            .values()
            .filter(|outcome| *outcome == "no-changes")
            .count() as u64,
        surfaces_queued: state.surfaces_queued,
        surfaces_read: state.surfaces_read,
    }
}

/// What the run is doing across one span, given everything open across it.
///
/// One bucket per millisecond, and the most specific open state names it: a
/// publication running while another node's agent works is *publication* time,
/// which is what makes a publication and a lock wait answerable at all.
fn now(
    phases: &BTreeMap<String, Phase>,
    judging: &BTreeSet<String>,
    dispatched: &BTreeSet<String>,
) -> BucketName {
    for phase in Phase::PRECEDENCE {
        if phases.values().any(|open| *open == phase) {
            return phase.bucket();
        }
    }
    if !judging.is_empty() {
        return BucketName::Judge;
    }
    if !dispatched.is_empty() {
        return BucketName::Agent;
    }
    BucketName::Scheduling
}

/// Make the measured buckets add up to the wall clock, in **both** directions.
///
/// Enforced rather than asserted, and both directions happen. The store is
/// three producers' records merged and their clocks are not one clock, so the
/// walk over it both under- and overcounts: a stamp that moves backwards
/// between two records leaves a span nothing charged, and a stamp *past* the
/// last record the merge put at the end leaves spans charged beyond the wall
/// clock they are measured against.
///
/// An **undercount** lands in `scheduling`: the residue is time the run cannot
/// be shown to have been doing anything nameable across. An **overcount** is
/// taken back off the measured buckets, largest first, until it is gone — the
/// residue is a clock artifact spread over whichever spans were mismeasured,
/// and the largest bucket is the one it distorts least. It always fits, because
/// the buckets are what overcounted. An unmeasured bucket is left unmeasured
/// either way: a `0` there would claim a measurement nothing took.
///
/// The one case that is not exact is a caller that passes no `scheduling`
/// bucket at all — an undercount then has nowhere to go and the parts are left
/// short of the whole. Nothing here does that, because what reaches it is
/// [`BucketName::ALL`], so every document this crate emits sums exactly.
fn balance(buckets: &mut [Bucket], wall_ms: u64) {
    let counted: u64 = buckets.iter().filter_map(|bucket| bucket.ms).sum();
    match counted.cmp(&wall_ms) {
        Ordering::Equal => {}
        Ordering::Less => {
            if let Some(residue) = buckets
                .iter_mut()
                .find(|bucket| bucket.name == BucketName::Scheduling)
            {
                residue.ms = Some(residue.ms.unwrap_or(0).saturating_add(wall_ms - counted));
            }
        }
        Ordering::Greater => drain(buckets, counted - wall_ms),
    }
}

/// Take `excess` milliseconds back off the measured buckets, largest first.
///
/// Largest first, and never below zero: an overcount is a clock artifact of
/// unknown origin, and charging it to the longest span is what keeps it from
/// rewriting a short one — thirteen milliseconds off nine thousand seconds of
/// agent time says the same thing about that run, where the same thirteen off a
/// one-second gate would not. Ties keep [`BucketName::ALL`]'s order, so one
/// store always balances the same way.
fn drain(buckets: &mut [Bucket], mut excess: u64) {
    let mut largest_first: Vec<usize> = (0..buckets.len())
        .filter(|index| buckets[*index].ms.is_some())
        .collect();
    largest_first.sort_by_key(|index| std::cmp::Reverse(buckets[*index].ms));
    for index in largest_first {
        if excess == 0 {
            return;
        }
        let ms = buckets[index].ms.unwrap_or(0);
        let taken = ms.min(excess);
        buckets[index].ms = Some(ms - taken);
        excess -= taken;
    }
}

/// What each party spent, from the evidence the run's own store carries.
///
/// `total` is the sum of every `turn-completed`, which is what a member reports
/// for the whole of its conversation. The split between the two sides is only in
/// the report that member settled with, so `agent` and `judge` come from there —
/// and a run whose members were single-sided, or whose reports this host cannot
/// read, has no split to report and says so by absence.
fn usage_of(paths: &RunPaths, events: &[Envelope]) -> BTreeMap<Party, Usage> {
    let mut totals: BTreeMap<Party, Usage> = BTreeMap::new();
    let mut fold = |party: Party, usage: &Usage| {
        if !usage.is_empty() {
            totals.entry(party).or_default().add(usage);
        }
    };

    for event in events
        .iter()
        .filter(|event| event.source == Source::Agentgraph && event.kind.0 == "turn-completed")
    {
        if let Some(usage) = event.payload.get("usage").filter(|value| value.is_object()) {
            fold(Party::Total, &Usage::of(usage));
        }
    }
    for retained in crate::report::evidence(paths, events) {
        let Some(document) = crate::report::read(&retained.kept) else {
            continue;
        };
        let Some(telemetry) = document.get("telemetry") else {
            continue;
        };
        for (party, side) in [(Party::Agent, "agent"), (Party::Judge, "judge")] {
            if let Some(usage) = telemetry
                .get(side)
                .and_then(|side| side.get("usage"))
                .filter(|value| value.is_object())
            {
                fold(party, &Usage::of(usage));
            }
        }
    }
    totals
}

/// What an unmeasured number reads as, everywhere it is rendered.
///
/// One spelling, and never `0`: an operator scanning the column has to be able
/// to tell "nothing was spent" from "nobody measured it".
pub const UNMEASURED: &str = "not measured";

/// Render the operator's breakdown.
pub fn render_breakdown(telemetry: &RunTelemetry) -> String {
    let mut out = format!(
        "{}  WALL {}\n",
        telemetry.run_id,
        duration(telemetry.wall_ms)
    );
    for bucket in &telemetry.buckets {
        match bucket.ms {
            None => out.push_str(&format!(
                "  {:<18} {:>10}\n",
                bucket.name.as_str(),
                UNMEASURED
            )),
            Some(ms) => {
                let share = (ms * 100).checked_div(telemetry.wall_ms).unwrap_or(0);
                out.push_str(&format!(
                    "  {:<18} {:>10}  {share:>3}%\n",
                    bucket.name.as_str(),
                    duration(ms)
                ));
            }
        }
    }
    for party in Party::ALL {
        out.push_str(&format!("  usage {:<12} ", party.as_str()));
        match telemetry.usage.get(&party) {
            None => out.push_str(&format!("{UNMEASURED}\n")),
            Some(usage) => out.push_str(&format!(
                "in {}  out {}  cache r {} w {}  ${}\n",
                tokens(usage.input),
                tokens(usage.output),
                tokens(usage.cache_read),
                tokens(usage.cache_write),
                usage
                    .cost_usd
                    .map_or_else(|| UNMEASURED.to_string(), |cost| format!("{cost:.4}"))
            )),
        }
    }
    out.push_str(&format!(
        "  {} dispatch(es), {} done, {} no-diff; {} surface(s) sent, {} read\n",
        telemetry.dispatches,
        telemetry.settled_done,
        telemetry.no_diff,
        telemetry.surfaces_queued,
        telemetry.surfaces_read
    ));
    out
}

/// A token count, or the one word an unreported one reads as.
fn tokens(count: Option<u64>) -> String {
    count.map_or_else(|| UNMEASURED.to_string(), |count| count.to_string())
}

/// A duration in milliseconds, rendered for a person.
pub fn duration(ms: u64) -> String {
    let seconds = ms / 1_000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    if seconds < 3_600 {
        return format!("{}m{:02}s", seconds / 60, seconds % 60);
    }
    format!("{}h{:02}m", seconds / 3_600, (seconds % 3_600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, Labels, ENVELOPE_VERSION};
    use crate::plan::{Node, Plan, PLAN_SCHEMA_VERSION};
    use serde_json::json;

    fn stamped(
        seconds: u64,
        source: Source,
        kind: EventKind,
        node: Option<&str>,
        fields: &[(&str, serde_json::Value)],
    ) -> Envelope {
        Envelope {
            v: ENVELOPE_VERSION,
            ts: crate::sys::rfc3339_from_millis(1_786_000_000_000 + seconds * 1_000),
            stream: "s".into(),
            seq: seconds,
            source,
            phase: None,
            kind,
            labels: Labels {
                run_id: Some("demo".into()),
                node: node.map(str::to_string),
                ..Labels::default()
            },
            payload: journal::payload(fields),
            artifacts: Vec::new(),
        }
    }

    fn at(
        seconds: u64,
        kind: journal::PipelineKind,
        node: Option<&str>,
        fields: &[(&str, serde_json::Value)],
    ) -> Envelope {
        stamped(seconds, Source::Pipeline, kind.into(), node, fields)
    }

    /// One relayed `onevcs` record, which is what a session's phases are read
    /// from.
    fn session(seconds: u64, kind: &str, node: Option<&str>) -> Envelope {
        stamped(seconds, Source::Vcs, EventKind(kind.into()), node, &[])
    }

    /// One relayed `oneagentgraph` record.
    fn turn(
        seconds: u64,
        kind: &str,
        node: Option<&str>,
        fields: &[(&str, serde_json::Value)],
    ) -> Envelope {
        stamped(
            seconds,
            Source::Agentgraph,
            EventKind(kind.into()),
            node,
            fields,
        )
    }

    /// One run's paths, under a scratch root this test owns.
    ///
    /// Real paths rather than a stand-in: the per-party usage is read out of
    /// this run's *own* copy of a report, so a test that proves the split has to
    /// put one where the reader will look for it.
    fn paths() -> RunPaths {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-telemetry-{}-{:?}",
            crate::sys::pid(),
            std::thread::current().id()
        ));
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        paths
    }

    fn plan() -> Plan {
        Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: None,
            name: Some("demo".into()),
            concurrency: 4,
            tasks: vec![Node {
                id: "build".into(),
                persona: Some("engineer".into()),
                task: Some("## What\ndo it".into()),
                ..Node::default()
            }],
        }
    }

    fn started() -> Envelope {
        at(
            0,
            journal::PipelineKind::RunStarted,
            None,
            &[("plan", json!(plan()))],
        )
    }

    /// Every measured bucket, summed. The unmeasured ones are absent, and an
    /// absent bucket contributes nothing to a total it was never part of.
    fn summed(telemetry: &RunTelemetry) -> u64 {
        telemetry.buckets.iter().filter_map(|b| b.ms).sum()
    }

    fn bucket_of(telemetry: &RunTelemetry, name: BucketName) -> Option<u64> {
        telemetry
            .buckets
            .iter()
            .find(|b| b.name == name)
            .unwrap_or_else(|| panic!("a {} bucket", name.as_str()))
            .ms
    }

    #[test]
    fn the_buckets_sum_exactly_to_the_wall_clock() {
        let events = vec![
            started(),
            at(
                10,
                journal::PipelineKind::NodeDispatched,
                Some("build"),
                &[],
            ),
            at(
                70,
                journal::PipelineKind::NodeSettled,
                Some("build"),
                &[("status", json!("done"))],
            ),
            at(100, journal::PipelineKind::RunStopped, None, &[]),
        ];
        let telemetry = of_run(&paths(), &events);
        assert_eq!(telemetry.wall_ms, 100_000);
        assert_eq!(
            summed(&telemetry),
            telemetry.wall_ms,
            "{:?}",
            telemetry.buckets
        );
        assert_eq!(bucket_of(&telemetry, BucketName::Agent), Some(60_000));
        assert_eq!(bucket_of(&telemetry, BucketName::Scheduling), Some(40_000));
        assert_eq!(telemetry.dispatches, 1);
        assert_eq!(telemetry.settled_done, 1);
    }

    /// The split the whole eight-way breakdown exists for: a stretch of a
    /// publication and a lock wait are answerable *apart from* the agent time
    /// they sit inside.
    ///
    /// Driven over the kinds an **older** `onevcs` wrote, because they are the
    /// only ones that reach the `gate` bucket: no release since the repository's
    /// merge path became the only verifier emits a `gate-started`. A store an
    /// operator already has still carries them, and reading one must still add
    /// up — `telemetry_separates_publication_and_lock_time_from_agent_time` in
    /// `tests/e2e/views.rs` is the same split over a run this build produces.
    #[test]
    fn gate_time_and_lock_waiting_are_separable_from_agent_time() {
        let events = vec![
            started(),
            at(
                10,
                journal::PipelineKind::NodeDispatched,
                Some("service"),
                &[],
            ),
            // The session is opened before the turn runs in it.
            session(10, "session-opened", Some("service")),
            turn(20, "turn-started", Some("service"), &[]),
            // Then the publication: a lock wait, a gate that build ran, and the
            // change.
            session(50, "lock-wait", Some("service")),
            session(60, "lock-acquired", Some("service")),
            session(70, "gate-started", Some("service")),
            session(100, "gate-verdict", Some("service")),
            session(110, "session-closed", Some("service")),
            at(
                120,
                journal::PipelineKind::NodeSettled,
                Some("service"),
                &[("status", json!("done"))],
            ),
        ];
        let telemetry = of_run(&paths(), &events);
        assert_eq!(
            summed(&telemetry),
            telemetry.wall_ms,
            "{:?}",
            telemetry.buckets
        );
        // The workspace before the turn ran in it, and again between the lock
        // and the gate that build ran.
        assert_eq!(bucket_of(&telemetry, BucketName::Setup), Some(20_000));
        // The turn itself, and the stretch after the session closed with the
        // node still in flight.
        assert_eq!(bucket_of(&telemetry, BucketName::Agent), Some(40_000));
        assert_eq!(bucket_of(&telemetry, BucketName::LockWait), Some(10_000));
        assert_eq!(bucket_of(&telemetry, BucketName::Gate), Some(30_000));
        assert_eq!(
            bucket_of(&telemetry, BucketName::PublicationWait),
            Some(10_000)
        );
        assert_eq!(bucket_of(&telemetry, BucketName::Scheduling), Some(10_000));
    }

    /// A node holds more than one session — its steps' and the one its change
    /// request is drafted in — and there are no cross-stream ordering promises
    /// inside a millisecond. So a second `session-opened` that the merge broke a
    /// tie in favour of must not rewind a publication already waiting on a lock:
    /// read as one, the whole wait is charged to preparing a workspace.
    #[test]
    fn a_second_sessions_opening_does_not_rewind_a_publication_already_under_way() {
        let mut opened = session(50, "session-opened", Some("service"));
        // The tie the merge breaks by stream id, with the second session's
        // record landing after the lock wait it did not interrupt.
        opened.stream = "z-second-session".into();
        let telemetry = of_run(
            &paths(),
            &[
                started(),
                at(
                    10,
                    journal::PipelineKind::NodeDispatched,
                    Some("service"),
                    &[],
                ),
                session(20, "session-opened", Some("service")),
                turn(30, "turn-activity", Some("service"), &[]),
                session(50, "lock-wait", Some("service")),
                opened,
                session(90, "lock-acquired", Some("service")),
                at(
                    100,
                    journal::PipelineKind::NodeSettled,
                    Some("service"),
                    &[("status", json!("done"))],
                ),
            ],
        );
        assert_eq!(bucket_of(&telemetry, BucketName::LockWait), Some(40_000));
        assert_eq!(summed(&telemetry), telemetry.wall_ms);
    }

    /// Nothing in this stack runs an LLM-lint pass, and the merged stream does
    /// not say which side of a two-party member a turn came from. Both read as
    /// absent — a `0` there would say the time was measured and found to be
    /// none.
    #[test]
    fn a_bucket_nothing_measures_is_absent_rather_than_zero() {
        let telemetry = of_run(
            &paths(),
            &[
                started(),
                at(
                    10,
                    journal::PipelineKind::NodeDispatched,
                    Some("build"),
                    &[],
                ),
                turn(20, "turn-activity", Some("build"), &[]),
            ],
        );
        assert_eq!(bucket_of(&telemetry, BucketName::Llmlint), None);
        assert_eq!(bucket_of(&telemetry, BucketName::Judge), None);
        assert_eq!(bucket_of(&telemetry, BucketName::Agent), Some(10_000));

        let document = serde_json::to_value(&telemetry).expect("it serialises");
        let llmlint = document["buckets"]
            .as_array()
            .expect("buckets")
            .iter()
            .find(|bucket| bucket["name"] == "llmlint")
            .expect("the llmlint bucket is still named");
        assert!(
            llmlint.get("ms").is_none(),
            "an unmeasured bucket carried a number: {llmlint}"
        );
        assert!(render_breakdown(&telemetry).contains("llmlint"));
        assert!(render_breakdown(&telemetry).contains(UNMEASURED));
    }

    /// The producer does not stamp which side a turn came from today. Where one
    /// does, it is read rather than ignored — the bucket becomes measured, and
    /// its time comes out of the agent's.
    #[test]
    fn a_turn_a_producer_attributes_to_the_judge_is_measured_as_the_judges() {
        let telemetry = of_run(
            &paths(),
            &[
                started(),
                at(
                    10,
                    journal::PipelineKind::NodeDispatched,
                    Some("build"),
                    &[],
                ),
                turn(
                    20,
                    "turn-started",
                    Some("build"),
                    &[("role", json!("judge"))],
                ),
                turn(
                    50,
                    "turn-started",
                    Some("build"),
                    &[("role", json!("agent"))],
                ),
                at(
                    60,
                    journal::PipelineKind::NodeSettled,
                    Some("build"),
                    &[("status", json!("done"))],
                ),
            ],
        );
        assert_eq!(bucket_of(&telemetry, BucketName::Judge), Some(30_000));
        assert_eq!(bucket_of(&telemetry, BucketName::Agent), Some(20_000));
        assert_eq!(summed(&telemetry), telemetry.wall_ms);
    }

    #[test]
    fn an_empty_run_has_a_zero_wall_clock_and_still_balances() {
        let telemetry = of_run(&paths(), &[]);
        assert_eq!(telemetry.wall_ms, 0);
        assert_eq!(summed(&telemetry), 0);
        assert!(render_breakdown(&telemetry).contains("WALL 0s"));
    }

    /// The breakdown renders one spelling and the telemetry document writes
    /// another, and an operator reading `telemetry --breakdown` against the JSON
    /// has to see the same words in both.
    #[test]
    fn a_bucket_and_a_party_serialise_as_the_words_the_breakdown_renders() {
        for name in BucketName::ALL {
            let json = serde_json::to_string(&name).expect("a bucket name serialises");
            assert_eq!(json, format!("\"{}\"", name.as_str()));
            assert_eq!(
                serde_json::from_str::<BucketName>(&json).expect("it reads back"),
                name
            );
        }
        for party in Party::ALL {
            let json = serde_json::to_string(&party).expect("a party serialises");
            assert_eq!(json, format!("\"{}\"", party.as_str()));
            assert_eq!(
                serde_json::from_str::<Party>(&json).expect("it reads back"),
                party
            );
        }
    }

    #[test]
    fn a_clock_that_moved_backwards_still_leaves_the_buckets_summing_to_wall() {
        let mut events = vec![
            started(),
            at(
                60,
                journal::PipelineKind::NodeDispatched,
                Some("build"),
                &[],
            ),
            at(30, journal::PipelineKind::RunStopped, None, &[]),
        ];
        // Deliberately out of order: the wall clock is first-to-last as read.
        events.reverse();
        let telemetry = of_run(&paths(), &events);
        assert_eq!(
            summed(&telemetry),
            telemetry.wall_ms,
            "{:?}",
            telemetry.buckets
        );
    }

    /// The direction the balance never held: measured buckets that add up to
    /// **more** than the wall clock, with `scheduling` already at zero.
    ///
    /// A settlement stamped before a turn already in the store leaves the walk's
    /// forward spans longer than a wall clock read first-to-last, and the old
    /// rule put that residue in `scheduling` — which at zero has none to give.
    #[test]
    fn buckets_that_overcount_a_zero_scheduling_run_still_sum_to_wall() {
        let telemetry = of_run(
            &paths(),
            &[
                started(),
                // The same instant, so nothing is charged to `scheduling` before
                // the dispatch and the bucket that has to absorb the residue is
                // measured at zero.
                at(0, journal::PipelineKind::NodeDispatched, Some("build"), &[]),
                turn(100, "turn-started", Some("build"), &[]),
                at(
                    90,
                    journal::PipelineKind::NodeSettled,
                    Some("build"),
                    &[("status", json!("done"))],
                ),
            ],
        );
        assert_eq!(telemetry.wall_ms, 90_000);
        assert_eq!(
            summed(&telemetry),
            telemetry.wall_ms,
            "the parts overcount the whole: {:?}",
            telemetry.buckets
        );
        assert_eq!(bucket_of(&telemetry, BucketName::Agent), Some(90_000));
        assert_eq!(bucket_of(&telemetry, BucketName::Scheduling), Some(0));
    }

    /// The document that was actually emitted, balanced.
    ///
    /// Pinned rather than paraphrased, because the margin is what makes it hard:
    /// thirteen milliseconds over nine thousand seconds, with `scheduling` at
    /// zero.
    #[test]
    fn the_document_measured_thirteen_milliseconds_over_balances_on_its_longest_span() {
        let mut buckets = vec![
            Bucket {
                name: BucketName::Agent,
                ms: Some(9_098_048),
            },
            Bucket {
                name: BucketName::Gate,
                ms: Some(1_229),
            },
            Bucket {
                name: BucketName::PublicationWait,
                ms: Some(7_975),
            },
            Bucket {
                name: BucketName::LockWait,
                ms: Some(0),
            },
            Bucket {
                name: BucketName::Setup,
                ms: Some(83_808),
            },
            Bucket {
                name: BucketName::Scheduling,
                ms: Some(0),
            },
        ];
        balance(&mut buckets, 9_191_047);
        assert_eq!(
            buckets.iter().filter_map(|bucket| bucket.ms).sum::<u64>(),
            9_191_047,
            "{buckets:?}"
        );
        assert_eq!(buckets[0].ms, Some(9_098_048 - 13), "{buckets:?}");
        assert_eq!(buckets[1].ms, Some(1_229), "{buckets:?}");
        assert_eq!(buckets[5].ms, Some(0), "{buckets:?}");
    }

    /// An overcount larger than any one bucket comes off all of them, and an
    /// unmeasured bucket stays unmeasured rather than being charged a span
    /// nothing measured.
    #[test]
    fn an_overcount_past_the_longest_span_is_taken_off_the_rest_in_turn() {
        let mut buckets = vec![
            Bucket {
                name: BucketName::Agent,
                ms: Some(40),
            },
            Bucket {
                name: BucketName::Judge,
                ms: None,
            },
            Bucket {
                name: BucketName::Setup,
                ms: Some(30),
            },
            Bucket {
                name: BucketName::Scheduling,
                ms: Some(0),
            },
        ];
        balance(&mut buckets, 10);
        assert_eq!(buckets[0].ms, Some(0), "{buckets:?}");
        assert_eq!(buckets[1].ms, None, "{buckets:?}");
        assert_eq!(buckets[2].ms, Some(10), "{buckets:?}");
        assert_eq!(buckets[3].ms, Some(0), "{buckets:?}");
    }

    /// The one case the doc comment says is not exact, held to what it says: an
    /// undercount with no `scheduling` bucket to put it in leaves the parts
    /// short of the whole rather than inventing a home for it.
    #[test]
    fn an_undercount_with_no_scheduling_bucket_is_left_where_it_is() {
        let mut buckets = vec![Bucket {
            name: BucketName::Agent,
            ms: Some(10),
        }];
        balance(&mut buckets, 25);
        assert_eq!(buckets[0].ms, Some(10), "{buckets:?}");
    }

    #[test]
    fn the_breakdown_names_every_bucket_and_every_party() {
        let rendered = render_breakdown(&of_run(
            &paths(),
            &[
                started(),
                at(
                    10,
                    journal::PipelineKind::NodeDispatched,
                    Some("build"),
                    &[],
                ),
                at(
                    20,
                    journal::PipelineKind::NodeSettled,
                    Some("build"),
                    &[("status", json!("done"))],
                ),
            ],
        ));
        for name in BucketName::ALL {
            assert!(
                rendered.contains(name.as_str()),
                "{rendered} omits {}",
                name.as_str()
            );
        }
        for party in Party::ALL {
            assert!(
                rendered.contains(&format!("usage {}", party.as_str())),
                "{rendered} omits the {} party",
                party.as_str()
            );
        }
        assert!(rendered.contains('%'), "{rendered}");
    }

    /// The number a run is budgeted against. `turn-completed` carries what a
    /// member spent; the split between its two sides is only in the report it
    /// settled with.
    #[test]
    fn usage_is_totalled_from_the_turns_and_split_by_the_report_they_settled_with() {
        let paths = paths();
        // The run's *own* copy, where a reader looks — put there by ingest for
        // the settlement below, which the fixture stands in for.
        let stored = paths.report_for("s", 20);
        std::fs::create_dir_all(paths.reports_dir()).expect("the run's report storage");
        std::fs::write(
            &stored,
            json!({
                "telemetry": {
                    "agent": {"usage": {
                        "input_tokens": 1_000, "output_tokens": 300,
                        "cache_read_tokens": 900, "cost_usd": 0.40,
                    }},
                    "judge": {"usage": {"input_tokens": 200, "output_tokens": 40}},
                },
            })
            .to_string(),
        )
        .expect("a stored report");

        let telemetry = of_run(
            &paths,
            &[
                started(),
                turn(
                    10,
                    "turn-completed",
                    Some("build"),
                    &[(
                        "usage",
                        json!({
                            "input_tokens": 1_200, "output_tokens": 340,
                            "cache_read_tokens": 900, "cache_write_tokens": 120,
                            "cost_usd": 0.42,
                        }),
                    )],
                ),
                turn(
                    20,
                    crate::report::MEMBER_SETTLED,
                    Some("build"),
                    // The path the producer named, which is never opened: the
                    // reader goes to this run's own copy of it.
                    &[(crate::report::REPORT_PATH, json!("/elsewhere/report.json"))],
                ),
            ],
        );

        let total = &telemetry.usage[&Party::Total];
        assert_eq!(total.input, Some(1_200));
        assert_eq!(total.output, Some(340));
        assert_eq!(total.cache_read, Some(900));
        assert_eq!(total.cache_write, Some(120));
        assert_eq!(total.cost_usd, Some(0.42));

        let agent = &telemetry.usage[&Party::Agent];
        assert_eq!(agent.input, Some(1_000));
        assert_eq!(agent.cost_usd, Some(0.40));
        let judge = &telemetry.usage[&Party::Judge];
        assert_eq!(judge.input, Some(200));
        // The judge reported no cache and no cost, so neither is claimed.
        assert_eq!(judge.cache_read, None);
        assert_eq!(judge.cost_usd, None);
        // Nothing in this stack runs an LLM-lint pass.
        assert!(!telemetry.usage.contains_key(&Party::Llmlint));

        let rendered = render_breakdown(&telemetry);
        assert!(rendered.contains("in 1200"), "{rendered}");
        assert!(rendered.contains("$0.4200"), "{rendered}");
        let llmlint = rendered
            .lines()
            .find(|line| line.trim_start().starts_with("usage llmlint"))
            .expect("the llmlint party is still named");
        assert!(llmlint.contains(UNMEASURED), "{llmlint}");
        std::fs::remove_dir_all(&paths.dir).ok();
    }

    /// The producer's own two spellings for the same numbers: the report's, and
    /// the one the type it declares for that payload uses. Neither may go
    /// unaccounted on a host whose routine failure is quota exhaustion.
    #[test]
    fn usage_is_read_in_either_spelling_the_producer_uses() {
        let declared = Usage::of(&json!({
            "tokens_in": 10, "tokens_out": 4,
            "cache_read": 3, "cache_write": 2, "cost": 0.5,
        }));
        assert_eq!(declared.input, Some(10));
        assert_eq!(declared.output, Some(4));
        assert_eq!(declared.cache_read, Some(3));
        assert_eq!(declared.cache_write, Some(2));
        assert_eq!(declared.cost_usd, Some(0.5));
        assert!(Usage::of(&json!({})).is_empty());
    }

    /// A report the machine that ran the dispatch stored elsewhere leaves the
    /// split unreported rather than reported as nothing spent.
    #[test]
    fn a_report_this_host_cannot_read_leaves_the_split_absent() {
        let telemetry = of_run(
            &paths(),
            &[
                started(),
                turn(
                    10,
                    "turn-completed",
                    Some("build"),
                    &[("usage", json!({"input_tokens": 5}))],
                ),
                turn(
                    20,
                    crate::report::MEMBER_SETTLED,
                    Some("build"),
                    &[(
                        crate::report::REPORT_PATH,
                        json!("/nowhere/onepipeline/report.json"),
                    )],
                ),
            ],
        );
        assert_eq!(telemetry.usage[&Party::Total].input, Some(5));
        assert!(!telemetry.usage.contains_key(&Party::Agent));
        assert!(!telemetry.usage.contains_key(&Party::Judge));
    }

    #[test]
    fn a_usage_total_accumulates_only_the_fields_something_reported() {
        let mut total = Usage::default();
        assert!(total.is_empty());
        total.add(&Usage {
            input: Some(10),
            cost_usd: Some(0.5),
            ..Usage::default()
        });
        total.add(&Usage {
            input: Some(5),
            output: Some(3),
            ..Usage::default()
        });
        assert_eq!(total.input, Some(15));
        assert_eq!(total.output, Some(3));
        assert_eq!(total.cache_read, None);
        assert_eq!(total.cost_usd, Some(0.5));
        assert!(!total.is_empty());
    }

    /// The checked-in shape of a schema-2 document.
    ///
    /// Read rather than restated: this is the wire a consumer parses, and the
    /// only thing that stops a field being renamed, an absent bucket becoming a
    /// zero, or the version moving without anyone deciding to move it.
    const GOLDEN: &str = include_str!("../tests/golden/telemetry-v2.json");

    /// The document the golden pins, built through the types.
    fn golden() -> RunTelemetry {
        let usage = |input, output, cache: Option<(u64, u64)>, cost| Usage {
            input: Some(input),
            output: Some(output),
            cache_read: cache.map(|(read, _)| read),
            cache_write: cache.map(|(_, write)| write),
            cost_usd: cost,
        };
        RunTelemetry {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            run_id: "golden".into(),
            wall_ms: 120_000,
            buckets: vec![
                Bucket {
                    name: BucketName::Agent,
                    ms: Some(40_000),
                },
                // Unmeasured, and absent from the wire because of it.
                Bucket {
                    name: BucketName::Judge,
                    ms: None,
                },
                Bucket {
                    name: BucketName::Llmlint,
                    ms: None,
                },
                Bucket {
                    name: BucketName::Gate,
                    ms: Some(30_000),
                },
                Bucket {
                    name: BucketName::PublicationWait,
                    ms: Some(10_000),
                },
                // Measured, and nothing waited: a real zero, on the wire.
                Bucket {
                    name: BucketName::LockWait,
                    ms: Some(0),
                },
                Bucket {
                    name: BucketName::Setup,
                    ms: Some(20_000),
                },
                Bucket {
                    name: BucketName::Scheduling,
                    ms: Some(20_000),
                },
            ],
            usage: BTreeMap::from([
                (
                    Party::Agent,
                    usage(1_000, 300, Some((900, 120)), Some(0.40)),
                ),
                // A side that reported no cache and no cost claims neither.
                (Party::Judge, usage(200, 40, None, None)),
                (
                    Party::Total,
                    usage(1_200, 340, Some((900, 120)), Some(0.42)),
                ),
            ]),
            dispatches: 1,
            settled_done: 1,
            no_diff: 0,
            surfaces_queued: 2,
            surfaces_read: 1,
        }
    }

    #[test]
    fn a_schema_2_document_is_the_shape_the_golden_pins() {
        let rendered = serde_json::to_string_pretty(&golden()).expect("it serialises");
        assert_eq!(
            rendered.trim(),
            GOLDEN.trim(),
            "the telemetry document changed shape. If that was deliberate, bump \
             TELEMETRY_SCHEMA_VERSION and update tests/golden/telemetry-v2.json together"
        );
        // The measured spans still add up, in the document as much as in the code.
        assert_eq!(summed(&golden()), golden().wall_ms);
    }

    #[test]
    fn a_schema_2_document_round_trips_through_the_types() {
        let value = golden();
        let read: RunTelemetry =
            serde_json::from_str(GOLDEN).expect("the golden reads back into the types");
        assert_eq!(read, value);
        let again: RunTelemetry =
            serde_json::from_str(&serde_json::to_string(&value).expect("it serialises"))
                .expect("it reads back");
        assert_eq!(again, value);
    }

    /// The distinction the whole document turns on, held at the wire: an
    /// unmeasured bucket carries no `ms` **key**, and a measured zero carries
    /// `0`. A reader that could not tell them apart would report a run that
    /// nobody measured as a run that spent nothing.
    #[test]
    fn an_unmeasured_bucket_omits_its_span_and_a_measured_zero_keeps_it() {
        let document: Value =
            serde_json::from_str(&serde_json::to_string(&golden()).expect("it serialises"))
                .expect("it is JSON");
        let bucket = |name: &str| {
            document["buckets"]
                .as_array()
                .expect("buckets")
                .iter()
                .find(|bucket| bucket["name"] == name)
                .unwrap_or_else(|| panic!("a {name} bucket"))
                .clone()
        };
        for absent in ["judge", "llmlint"] {
            assert!(
                bucket(absent).get("ms").is_none(),
                "the {absent} bucket carried a span nothing measured"
            );
        }
        assert_eq!(bucket("lock_wait")["ms"], 0);

        // And back: an absent key is `None`, never `Some(0)`.
        let read: RunTelemetry = serde_json::from_value(document).expect("it reads back");
        assert_eq!(bucket_of(&read, BucketName::Judge), None);
        assert_eq!(bucket_of(&read, BucketName::LockWait), Some(0));
    }

    /// The same rule for usage: a field nothing reported is not written, and a
    /// party that reported nothing at all is not on the map.
    #[test]
    fn an_unreported_usage_field_is_omitted_and_round_trips_as_absent() {
        let document: Value =
            serde_json::from_str(&serde_json::to_string(&golden()).expect("it serialises"))
                .expect("it is JSON");
        let judge = &document["usage"]["judge"];
        assert_eq!(judge["input"], 200);
        for unreported in ["cache_read", "cache_write", "cost_usd"] {
            assert!(
                judge.get(unreported).is_none(),
                "the judge claimed a {unreported} nothing reported: {judge}"
            );
        }
        assert!(
            document["usage"].get("llmlint").is_none(),
            "a party nothing reported for is on the wire: {}",
            document["usage"]
        );

        // A usage with no signal at all serialises to an empty object and reads
        // back as one, rather than as five zeros.
        let empty = Usage::default();
        let rendered = serde_json::to_string(&empty).expect("it serialises");
        assert_eq!(rendered, "{}");
        let read: Usage = serde_json::from_str(&rendered).expect("it reads back");
        assert_eq!(read, empty);
        assert!(read.is_empty());
    }

    /// A schema-1 document is **refused**, by name.
    ///
    /// `1` named four buckets this build does not have and carried no `usage` at
    /// all, so there is no reading of one that is not a guess: a `dispatching`
    /// span silently dropped, or a run reported as having spent nothing. The
    /// version moved for exactly that reason, and the refusal is what makes the
    /// move mean something to a consumer.
    #[test]
    fn a_schema_1_document_is_refused_rather_than_read_as_a_newer_one() {
        let v1 = json!({
            "schema_version": 1,
            "run_id": "old",
            "wall_ms": 100_000,
            "buckets": [
                {"name": "dispatching", "ms": 60_000},
                {"name": "awaiting-planner", "ms": 0},
                {"name": "awaiting-human", "ms": 0},
                {"name": "orchestrating", "ms": 40_000},
            ],
            "dispatches": 1,
            "settled_done": 1,
            "no_diff": 0,
            "surfaces_queued": 0,
            "surfaces_read": 0,
        });
        let refusal = serde_json::from_value::<RunTelemetry>(v1.clone())
            .expect_err("a schema-1 document was read as a schema-2 one");
        assert!(
            refusal.to_string().contains("schema_version 1")
                && refusal
                    .to_string()
                    .contains(&TELEMETRY_SCHEMA_VERSION.to_string()),
            "the refusal names neither the version it met nor the one it reads: {refusal}"
        );

        // Stamped `2` and it is still refused, on every other way it is not one:
        // four buckets this build does not have, and no `usage` at all — and a
        // run with no usage field is not a run that spent nothing.
        let mut relabelled = v1;
        relabelled["schema_version"] = json!(TELEMETRY_SCHEMA_VERSION);
        let refusal = serde_json::from_value::<RunTelemetry>(relabelled.clone())
            .expect_err("a schema-1 body under a schema-2 stamp was read");
        assert!(
            refusal.to_string().contains("dispatching"),
            "the refusal does not name what it could not read: {refusal}"
        );

        let mut renamed = relabelled;
        renamed["buckets"] = json!([{"name": "agent", "ms": 100_000}]);
        let refusal = serde_json::from_value::<RunTelemetry>(renamed)
            .expect_err("a document carrying one bucket was read");
        assert!(
            refusal.to_string().contains("exactly"),
            "the refusal does not say the set is fixed: {refusal}"
        );
    }

    /// The eight, once each, in the order the breakdown renders them. A set
    /// that is short one, carries one twice, or arrives shuffled makes the sum
    /// invariant unreadable — and a consumer indexing the eighth would be
    /// reading whichever happened to be there.
    #[test]
    fn a_bucket_set_that_is_not_the_eight_is_refused() {
        let document = |buckets: Value| {
            let mut value = serde_json::to_value(golden()).expect("it serialises");
            value["buckets"] = buckets;
            serde_json::from_value::<RunTelemetry>(value)
        };
        let whole = serde_json::to_value(golden()).expect("it serialises")["buckets"].clone();
        assert!(document(whole.clone()).is_ok(), "the eight were refused");

        let short: Vec<Value> = whole.as_array().expect("buckets")[..7].to_vec();
        assert!(document(json!(short)).is_err(), "a set of seven was read");

        let mut twice = whole.as_array().expect("buckets").clone();
        twice[1] = twice[0].clone();
        assert!(document(json!(twice)).is_err(), "a doubled bucket was read");

        let mut shuffled = whole.as_array().expect("buckets").clone();
        shuffled.reverse();
        assert!(
            document(json!(shuffled)).is_err(),
            "a shuffled set was read"
        );
    }

    /// The version is a decision, not an accident: it moves when the shape does,
    /// and the golden is named for the one it pins.
    #[test]
    fn the_schema_version_and_the_golden_name_the_same_number() {
        assert_eq!(TELEMETRY_SCHEMA_VERSION, 2);
        assert_eq!(golden().schema_version, TELEMETRY_SCHEMA_VERSION);
        let document: Value = serde_json::from_str(GOLDEN).expect("the golden is JSON");
        assert_eq!(document["schema_version"], TELEMETRY_SCHEMA_VERSION);
    }

    #[test]
    fn a_duration_reads_in_the_units_its_size_calls_for() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(45_000), "45s");
        assert_eq!(duration(125_000), "2m05s");
        assert_eq!(duration(7_500_000), "2h05m");
    }
}
