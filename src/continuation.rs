//! Continuing a project from its store alone, and the best-effort claim on who drives it.
//!
//! A project's store is where its last driver left what it settled: each lineage's
//! `onepipeline.settlement`, the `onepipeline.dispatch` of a node whose session was open,
//! and the project's `onepipeline.driver` claim. A launch reads all three before its first
//! dispatch, so a host holding none of the run's local state carries the project on rather
//! than running it again beside itself — the same drafts, the same stack:
//!
//! - a lineage whose settlement is `done` is **continued**: its settlement is journalled
//!   whole, marked `continued`, and nothing is dispatched for it;
//! - a node that left a branch — settled `failed`, `cancelled` or `parked`, or dispatched
//!   with no settlement since — is **pinned** to that branch, as an adoption pins an
//!   abandoned dispatch, so its next dispatch continues the branch and its publication
//!   adopts the change request already open from it.
//!
//! The claim is **not atomic**: the store offers no conditional write, so it is a record
//! two launches can both read empty. What it holds is narrower — a launch that finds
//! another run's claim is refused unless told `--take-over`, and a driver that finds its
//! claim replaced stops — and `docs/contract-divergences.md` entry 119 says exactly what it
//! does not.
//!
//! A running driver reads the claim only where it must: on an adoption's first pass, and
//! once immediately before each publication it starts, after the fence naming that
//! publication has landed. Nothing reads it on a timer, nothing reads it before a dispatch,
//! and a read that is not answered ends nothing. So the waiting is the adopter's: a
//! `--take-over` writes its claim, and starts nothing until the driver it displaced has
//! acknowledged it — on the claim, once it has read the takeover at a publication, its
//! closeout or its stop — or until [`takeover_wait`] has passed with no publication of that
//! driver's under way. Entry 128 states the contract.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::payload::ClaimLease;
use crate::payload::DriverClaim;
use crate::taskgraph::{
    QualifiedId, Read, Store, DISPATCH_KEY, DRIVER_KEY, PUBLISHING_KEY, SETTLEMENT_KEY,
};

// llmlint: ignore-block[invalid_states_unrepresentable] The store’s existing onepipeline.rework contract uses string identities. This boundary DTO rejects unknown fields; rework_provenance validates its values before continuation or baseline preservation uses them. Changing the serialized fields belongs to the shared contract’s owner.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredRework {
    pub(crate) replaces: String,
    pub(crate) reason: String,
    pub(crate) instruction: String,
    pub(crate) comments: Vec<crate::channel::ReworkComment>,
    /// The superseded node's settled head, absent from a record written before it
    /// was carried (entry 130).
    #[serde(default)]
    pub(crate) replaces_head: Option<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// Parse the store's existing rework provenance at either read boundary.
pub(crate) fn rework_provenance(value: &Value) -> Result<StoredRework> {
    let stored: StoredRework = serde_json::from_value(value.clone())
        .map_err(|error| Error::Invalid(format!("rework provenance: {error}")))?;
    if stored.reason.trim().is_empty()
        || stored.instruction.trim().is_empty()
        || stored.replaces.trim().is_empty()
    {
        return Err(Error::Invalid("blank rework provenance".to_owned()));
    }
    if stored.comments.iter().any(|comment| {
        comment.id.trim().is_empty()
            || comment.id.chars().any(char::is_control)
            || !onevcs::Url::parse(&comment.url)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
    }) {
        return Err(Error::Invalid(
            "invalid comment in rework provenance".to_owned(),
        ));
    }
    // A commit id git names in full — SHA-1's 40 hex digits or SHA-256's 64 — and nothing
    // else, because the drafting asks git for the commits after it.
    if stored.replaces_head.as_deref().is_some_and(|head| {
        !matches!(head.len(), 40 | 64) || !head.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err(Error::Invalid(
            "rework provenance names a superseded head that is no full commit id".to_owned(),
        ));
    }
    Ok(stored)
}

/// The claim interval's default. It paces the one read of the claim still made on a timer —
/// a takeover's, while it waits — and is stated in each claim's `lease` for a build that
/// still sizes a takeover's wait by it.
pub(crate) const CLAIM_INTERVAL: Duration = Duration::from_secs(300);

/// The longest claim interval the setting may name, and a claim's `lease` may state: a day.
/// A build that still sizes a takeover's wait by a claim's interval would wait days on a
/// longer one, and arithmetic on the instants it is added to has to stay representable.
pub(crate) const MAX_CLAIM_INTERVAL: Duration = Duration::from_secs(86_400);

/// The setting naming the claim interval, in whole seconds.
///
/// Each host is given its own, and its claim states it. It governs only how often a
/// takeover reads the claim while it waits; no running driver reads its claim on a timer.
pub(crate) const CLAIM_INTERVAL_ENV: &str = "ONEPIPELINE_CLAIM_INTERVAL_SECONDS";

/// The longest one read of the claim may take before the driver goes on without its answer
/// — a publication waiting on it waits for the next: a reconcile pass that waited on a store
/// for longer would hold every settlement behind a board.
pub(crate) const CLAIM_READ_DEADLINE: Duration = Duration::from_secs(20);

/// What [`takeover_wait`] adds to the displaced host's cancel grace.
const TAKEOVER_SLACK: Duration = Duration::from_secs(10);

/// How long a superseded driver waits, once, for what it settled to land on the store
/// before it goes on passing and says the settlement has not reached it.
pub(crate) const SETTLEMENT_LANDING_WAIT: Duration = Duration::from_secs(20);

/// A takeover reads the claim every this-many-th of an interval while it waits, bounded by
/// [`TAKEOVER_POLL_MIN`] and [`TAKEOVER_POLL_MAX`].
const TAKEOVER_POLLS_PER_INTERVAL: u32 = 10;
const TAKEOVER_POLL_MIN: Duration = Duration::from_secs(1);
const TAKEOVER_POLL_MAX: Duration = Duration::from_secs(30);

/// How soon a driver writes its publications under way again after the store refused it,
/// and reads the claim again before a publication after a read that was not answered.
const FENCE_RETRY: Duration = Duration::from_secs(5);

/// The test failpoint met before each read a takeover makes once its wait has passed, so a
/// journey can hold that read until the displaced driver has got where it needs it.
const TAKEOVER_PAST_WAIT_GATE_ENV: &str = "ONEPIPELINE_TEST_TAKEOVER_PAST_WAIT_GATE";

/// The key a displaced driver's acknowledgement is written under, inside the claim it
/// acknowledges.
pub(crate) const ACKNOWLEDGED_KEY: &str = "acknowledged";

/// The keys a claim carries, and the only ones: the store's copy is external input. A
/// claim a displaced driver acknowledged carries [`ACKNOWLEDGED_KEY`] beside them.
const CLAIM_KEYS: [&str; 6] = ["run", "host", "session", "pid", "claimed_at", "lease"];

/// The keys a claim's `lease` carries, and the only ones.
const LEASE_KEYS: [&str; 2] = ["interval_seconds", "cancel_grace_seconds"];

/// The terms this host's claim states: the claim interval and the cancel grace it was given,
/// both at least a second, as [`claim_interval`] and the grace's own setting read them. A run
/// taking the project over waits out the grace these state rather than its own.
pub(crate) fn lease_terms() -> ClaimLease {
    let positive =
        |seconds: u64| std::num::NonZeroU64::new(seconds).unwrap_or(std::num::NonZeroU64::MIN);
    ClaimLease {
        interval_seconds: positive(claim_interval().as_secs()),
        cancel_grace_seconds: positive(crate::engine::cancel_grace().as_secs()),
    }
}

/// A driver's publications under way, as the project's [`PUBLISHING_KEY`] holds them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Publishing {
    // llmlint: ignore-block[invalid_states_unrepresentable] a store document's fields, as the
    // claim beside them spells them: a run, a host, node ids and an RFC 3339 instant are
    // strings on the store, and `publishing_of` refuses one that is not each.
    /// The run publishing.
    pub(crate) run: String,
    /// The host it is driven on.
    pub(crate) host: String,
    /// The nodes whose publications are under way.
    pub(crate) nodes: Vec<String>,
    /// When this was written, RFC 3339.
    pub(crate) at: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// The project's publications under way, out of its metadata: `None` where the key is
/// absent or `null`, and a refusal where it holds anything that is not a driver's
/// publications.
pub(crate) fn publishing_of(
    project: &str,
    metadata: &BTreeMap<String, Value>,
) -> Result<Option<Publishing>> {
    let refused = |why: String| {
        Error::Invalid(format!(
            "{project}: `{PUBLISHING_KEY}` is not a driver's publications: {why}"
        ))
    };
    let Some(stored) = metadata
        .get(PUBLISHING_KEY)
        .filter(|stored| !stored.is_null())
    else {
        return Ok(None);
    };
    let read: Publishing =
        serde_json::from_value(stored.clone()).map_err(|why| refused(why.to_string()))?;
    if crate::vcs::usable(&read.run).is_none()
        || crate::vcs::usable(&read.host).is_none()
        || read.nodes.is_empty()
        || read
            .nodes
            .iter()
            .any(|node| crate::vcs::usable(node).is_none())
        || crate::projection::millis_of(&read.at).is_none()
    {
        return Err(refused(format!(
            "it names {:?} on {:?} publishing {:?} at {:?}, not a run, a host, the nodes \
             publishing and an instant",
            read.run, read.host, read.nodes, read.at
        )));
    }
    Ok(Some(read))
}

/// A displaced driver's acknowledgement of the claim that displaced it: it has stopped
/// starting dispatches and publications, and none of its own is in progress.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Acknowledgement {
    // llmlint: ignore-block[invalid_states_unrepresentable] a store document's fields, as the
    // claim beside them spells them: a run, a host and an RFC 3339 instant are strings on the
    // store, and `stored_claim_of` refuses one that is not each.
    /// The run that was displaced.
    pub(crate) run: String,
    /// The host it was driven on.
    pub(crate) host: String,
    /// When it acknowledged, RFC 3339.
    pub(crate) at: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// The project's claim, out of its metadata: `None` where the key is absent or `null` —
/// the two read the same, because a release writes `null` — and a refusal naming the key
/// where it holds anything that is not a claim.
pub(crate) fn claim_of(
    project: &str,
    metadata: &BTreeMap<String, Value>,
) -> Result<Option<DriverClaim>> {
    Ok(stored_claim_of(project, metadata)?.map(|(claim, _)| claim))
}

/// The claim to write where the run `run` on `host`, letting go of its project with nothing
/// of its own in progress, finds another run's claim on it that nothing has acknowledged:
/// that claim, acknowledged by this run. `None` where the claim is this run's, already
/// acknowledged, absent or not a claim.
pub(crate) fn acknowledgement_over(
    project: &str,
    metadata: &BTreeMap<String, Value>,
    run: &str,
    host: &str,
) -> Option<Value> {
    let Ok(Some((claim, None))) = stored_claim_of(project, metadata) else {
        return None;
    };
    if claim.names(run, host) {
        return None;
    }
    let mut written = serde_json::to_value(&claim).ok()?;
    written[ACKNOWLEDGED_KEY] = json!(Acknowledgement {
        run: run.to_owned(),
        host: host.to_owned(),
        at: crate::sys::now_rfc3339(),
    });
    Some(written)
}

/// The project's claim and the acknowledgement it carries, if a displaced driver wrote one.
fn stored_claim_of(
    project: &str,
    metadata: &BTreeMap<String, Value>,
) -> Result<Option<(DriverClaim, Option<Acknowledgement>)>> {
    let refused =
        |why: String| Error::Invalid(format!("{project}: `{DRIVER_KEY}` is not a claim: {why}"));
    match metadata.get(DRIVER_KEY) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(claim)) => {
            let mut claim = claim.clone();
            let acknowledged = match claim.remove(ACKNOWLEDGED_KEY) {
                None => None,
                Some(written) => {
                    let read: Acknowledgement = serde_json::from_value(written)
                        .map_err(|why| refused(format!("its `{ACKNOWLEDGED_KEY}`: {why}")))?;
                    if crate::vcs::usable(&read.run).is_none()
                        || crate::vcs::usable(&read.host).is_none()
                        || crate::projection::millis_of(&read.at).is_none()
                    {
                        return Err(refused(format!(
                            "its `{ACKNOWLEDGED_KEY}` names {:?} on {:?} at {:?}, not a run, a \
                             host and an instant",
                            read.run, read.host, read.at
                        )));
                    }
                    Some(read)
                }
            };
            if let Some(key) = claim.keys().find(|key| !CLAIM_KEYS.contains(&key.as_str())) {
                return Err(refused(format!("it carries `{key}`, which no claim does")));
            }
            if let Some(lease) = claim.get("lease") {
                let Some(lease) = lease.as_object() else {
                    return Err(refused(format!("its `lease` is {lease}, not a mapping")));
                };
                if let Some(key) = lease.keys().find(|key| !LEASE_KEYS.contains(&key.as_str())) {
                    return Err(refused(format!(
                        "its `lease` carries `{key}`, which no lease does"
                    )));
                }
            }
            let claim: DriverClaim = serde_json::from_value(Value::Object(claim))
                .map_err(|why| refused(why.to_string()))?;
            // Each name is printed on a refusal's line and compared as an identity, so it has
            // to be one a line can carry: not empty, nothing but what prints.
            for (field, name) in [("run", Some(&claim.run)), ("host", Some(&claim.host))]
                .into_iter()
                .chain([("session", claim.session.as_ref())])
            {
                if let Some(name) = name.filter(|name| crate::vcs::usable(name).is_none()) {
                    return Err(refused(format!(
                        "its `{field}` is {name:?}, not a name a line can carry"
                    )));
                }
            }
            if crate::projection::millis_of(&claim.claimed_at).is_none() {
                return Err(refused(format!(
                    "its `claimed_at` is {:?}, not an RFC 3339 instant",
                    claim.claimed_at
                )));
            }
            // A lease no driver of this build runs on would size a takeover's wait wrong.
            if let Some(lease) = claim
                .lease
                .filter(|lease| lease.interval_seconds.get() > MAX_CLAIM_INTERVAL.as_secs())
            {
                return Err(refused(format!(
                    "its `lease` names a {}s interval, not one of 1 to {} seconds",
                    lease.interval_seconds,
                    MAX_CLAIM_INTERVAL.as_secs()
                )));
            }
            Ok(Some((claim, acknowledged)))
        }
        Some(other) => Err(refused(format!("it holds {other}, not a mapping"))),
    }
}

/// A node's `onepipeline.dispatch`: the branch its open session works on, the base it opened
/// against, and when it opened. The one shape the write-back writes and a launch reads.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredDispatch {
    // llmlint: ignore-block[invalid_states_unrepresentable] a store document's fields, as entry
    // 119 spells them: a branch, a base and an RFC 3339 instant are strings on the store, as on
    // every settlement beside them, and `dispatch_of` refuses one that is not each.
    /// The branch the session works on.
    pub(crate) branch: String,
    /// The base it opened against.
    pub(crate) base: String,
    /// When it opened, RFC 3339.
    pub(crate) opened_at: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// One node's stored dispatch, read at the store boundary: `None` where the task carries
/// none, and a refusal naming the node where it carries anything that is not one.
pub(crate) fn dispatch_of(
    node: &str,
    metadata: &Map<String, Value>,
) -> Result<Option<StoredDispatch>> {
    let refused = |why: String| {
        Error::Invalid(format!(
            "node '{node}': `{DISPATCH_KEY}` is not a dispatch: {why}"
        ))
    };
    let Some(stored) = metadata
        .get(DISPATCH_KEY)
        .filter(|stored| !stored.is_null())
    else {
        return Ok(None);
    };
    let dispatch: StoredDispatch =
        serde_json::from_value(stored.clone()).map_err(|why| refused(why.to_string()))?;
    if crate::vcs::usable(&dispatch.branch).is_none()
        || crate::vcs::usable(&dispatch.base).is_none()
    {
        return Err(refused(format!(
            "its branch {:?} or base {:?} is not one a session can name",
            dispatch.branch, dispatch.base
        )));
    }
    if crate::projection::millis_of(&dispatch.opened_at).is_none() {
        return Err(refused(format!(
            "its `opened_at` is {:?}, not an RFC 3339 instant",
            dispatch.opened_at
        )));
    }
    Ok(Some(dispatch))
}

/// Every key a stored settlement may carry: the registered `node-settled` document's, and
/// `remote`, which the settling writer adds beside them for a node whose branch it put on an
/// origin. `continued` is this build's own record of a launch, and never a stored field.
const SETTLEMENT_KEYS: [&str; 12] = [
    "status",
    "outcome",
    "detail",
    "branch",
    "change_url",
    "cause",
    "head",
    "remote",
    "landing",
    "completed_steps",
    "base",
    "stack",
];

/// Every key a stored settlement's `stack` carries, and every key one of its parents does.
const STACK_KEYS: [&str; 2] = ["parents", "stack_base"];
const STACK_PARENT_KEYS: [&str; 3] = ["node", "branch", "head"];

/// One node's stored settlement, read at the store boundary: `None` where the task carries
/// none, and a refusal naming the node where it carries anything that is not a settlement —
/// a value that is not a mapping, a key no settlement carries, or a field of the wrong shape.
fn settlement_of(node: &str, metadata: Option<&Map<String, Value>>) -> Result<Option<Stored>> {
    let refused = |why: String| {
        Error::Invalid(format!(
            "node '{node}': `{SETTLEMENT_KEY}` is not a settlement: {why}"
        ))
    };
    let Some(stored) = metadata
        .and_then(|metadata| metadata.get(SETTLEMENT_KEY))
        .filter(|stored| !stored.is_null())
    else {
        return Ok(None);
    };
    let Some(settlement) = stored.as_object() else {
        return Err(refused(format!("it holds {stored}, not a mapping")));
    };
    if let Some(key) = settlement
        .keys()
        .find(|key| !SETTLEMENT_KEYS.contains(&key.as_str()))
    {
        return Err(refused(format!(
            "it carries `{key}`, which no settlement does"
        )));
    }
    if let Some(stack) = settlement.get("stack").and_then(Value::as_object) {
        if let Some(key) = stack.keys().find(|key| !STACK_KEYS.contains(&key.as_str())) {
            return Err(refused(format!(
                "its `stack` carries `{key}`, which no stack does"
            )));
        }
        let parents = stack.get("parents").and_then(Value::as_array);
        for parent in parents.into_iter().flatten().filter_map(Value::as_object) {
            if let Some(key) = parent
                .keys()
                .find(|key| !STACK_PARENT_KEYS.contains(&key.as_str()))
            {
                return Err(refused(format!(
                    "a parent of its `stack` carries `{key}`, which no stack parent does"
                )));
            }
        }
    }
    let read = serde_json::from_value::<crate::payload::NodeSettled>(stored.clone())
        .map_err(|why| refused(why.to_string()))?;
    // What the document cannot say in a type: a stack is at least one usable parent, and a
    // `remote` is one of the words putting a branch on an origin answers with.
    if read.stack.as_ref().is_some_and(|stack| !stack.is_usable()) {
        return Err(refused("its `stack` names no usable parent".to_owned()));
    }
    // Every name it carries is printed on a view's line and handed to a session, so each has
    // to be one a line can carry — the check every name a session reports is held to.
    for (field, name) in [
        ("branch", &read.branch),
        ("base", &read.base),
        ("head", &read.head),
        ("change_url", &read.change_url),
    ] {
        if let Some(name) = name
            .as_ref()
            .filter(|name| crate::vcs::usable(name).is_none())
        {
            return Err(refused(format!(
                "its `{field}` is {name:?}, not a name a line can carry"
            )));
        }
    }
    if let Some(remote) = settlement.get("remote") {
        if remote
            .as_str()
            .and_then(crate::vcs::preservation_of)
            .is_none()
        {
            return Err(refused(format!(
                "its `remote` is {remote}, not a word putting a branch on an origin answers with"
            )));
        }
    }
    Ok(Some(Stored {
        status: read.status,
        fields: settlement.clone(),
    }))
}

/// Whether the stored settlement on a task says the engine itself settled it `cancelled`:
/// a park, a stopped dispatch or a superseded attempt, each of which a continuing launch
/// resumes. `false` where the task carries none, and a refusal where it carries anything that
/// is not a settlement.
pub(crate) fn cancelled_by_the_engine(task: &str, metadata: &Map<String, Value>) -> Result<bool> {
    Ok(settlement_of(task, Some(metadata))?
        .is_some_and(|settlement| settlement.status == crate::payload::Status::Cancelled))
}

/// A stored settlement as the boundary read it: its status, typed, and every field it
/// carries, verbatim, for a continuation to journal whole.
#[derive(Debug)]
struct Stored {
    status: crate::payload::Status,
    fields: Map<String, Value>,
}

impl Stored {
    /// The branch a node that ran and did not finish left its work on: a failed, cancelled
    /// or parked one's, and no other's.
    fn left_on(&self) -> Option<crate::vcs::BranchName> {
        use crate::payload::Status;
        matches!(
            self.status,
            Status::Failed | Status::Cancelled | Status::Parked
        )
        .then(|| self.fields.get("branch")?.as_str())
        .flatten()
        .and_then(crate::vcs::BranchName::checked)
    }
}

/// Whether a launch may go ahead over the claim its read found: any claim refuses it, by
/// [`Error::ProjectDriven`], unless the launch takes the project over.
///
/// Every claim, this host's own included: a run of this host is the one to `adopt`, live or
/// not, rather than to start a second run beside — and a live one is driving the project
/// now. The refusal says which, out of the host it names.
pub(crate) fn admit(claim: Option<&DriverClaim>, take_over: bool) -> Result<()> {
    match claim {
        Some(claim) if !take_over => Err(Error::ProjectDriven {
            run: claim.run.clone(),
            host: claim.host.clone(),
            claimed_at: claim.claimed_at.clone(),
        }),
        _ => Ok(()),
    }
}

/// Refuse a `--fresh` launch while a lifecycle node's settlement holds a change request
/// nothing has landed, naming each: running the node again would open a second one beside
/// the one its reviewers are reading.
pub(crate) fn refuse_fresh_over_unlanded(project: &str, read: &Read) -> Result<()> {
    let unlanded: Vec<String> = read
        .plan
        .tasks
        .iter()
        .filter(|node| node.repo.is_some())
        .map(|node| {
            let settlement = settlement_of(&node.id, read.metadata.get(&node.id))?;
            Ok(settlement.and_then(|settlement| {
                let url = settlement.fields.get("change_url")?.as_str()?;
                (settlement.fields.get("landing").and_then(Value::as_str) != Some("landed"))
                    .then(|| format!("'{}' ({url})", node.id))
            }))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    if unlanded.is_empty() {
        return Ok(());
    }
    Err(Error::Refused(format!(
        "--fresh refused: {project} holds change requests nothing has landed, on {}; running \
         them afresh would open a second change request beside each. Launch without --fresh to \
         continue them, or land or close them first",
        unlanded.join(", ")
    )))
}

/// What a launch continues from its project's store.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Continuation {
    // llmlint: ignore-block[invalid_states_unrepresentable] a node id is the plain `String` the
    // plan's own `Node::id` is, which every map of `RunState` keys by and `graph::validate`
    // checked before any of these is read; each is copied from that plan, and a node-id newtype
    // on these two fields alone would disagree with all of them and convert at every use. The
    // halves read off the store are the typed ones: `Continued` and `BranchName`.
    /// Each node continued `done`, in plan order, with the settlement it journals.
    pub(crate) settled: Vec<(String, Continued)>,
    /// Each node pinned to the branch it left, in plan order.
    pub(crate) pinned: Vec<(String, crate::vcs::BranchName)>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// A `node-settled` a launch journals for a node its store held `done`: the stored settlement
/// whole — every field carried verbatim, which is what restores the node — marked `continued`.
///
/// Built only by [`continuation_of`], from a settlement [`settlement_of`] read through the
/// registered document and its own key set, so a payload of this type is always one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Continued(Map<String, Value>);

impl Continued {
    pub(crate) fn payload(&self) -> Map<String, Value> {
        self.0.clone()
    }
}

/// Read what a launch continues out of the project it read.
///
/// Keyed by the plan's own node ids — a lineage root's, carrying its head's definition and,
/// here, its head's settlement — because those are the nodes the launched plan holds.
/// `claim` is what the read found, named on each continued settlement as the run it is
/// continued from.
pub(crate) fn continuation_of(read: &Read, claim: Option<&DriverClaim>) -> Result<Continuation> {
    let mut continued = Continuation::default();
    let from = json!({"source": "store", "run": claim.map(|claim| claim.run.clone())});
    for node in &read.plan.tasks {
        // Both are external input, read through the shape each is held to before anything
        // of either is journalled or pinned.
        let metadata = read.metadata.get(&node.id);
        let settlement = settlement_of(&node.id, metadata)?;
        let dispatch = match metadata {
            Some(metadata) => dispatch_of(&node.id, metadata)?,
            None => None,
        };
        if let Some(done) = settlement
            .as_ref()
            .filter(|settlement| settlement.status == crate::payload::Status::Done)
        {
            let mut payload = done.fields.clone();
            let mut from = from.clone();
            if let Some(metadata) = metadata {
                if let Some(rework) = metadata.get(crate::taskgraph::REWORK_KEY) {
                    let stored = rework_provenance(rework)
                        .map_err(|error| Error::Invalid(format!("node '{}': {error}", node.id)))?;
                    let replacement = metadata
                        .get(crate::taskgraph::NODE_KEY)
                        .and_then(Value::as_str)
                        .filter(|replacement| !replacement.trim().is_empty())
                        .ok_or_else(|| {
                            Error::Invalid(format!(
                                "node '{}': rework provenance has no replacement",
                                node.id
                            ))
                        })?;
                    from["rework"] = json!(crate::payload::ContinuedRework {
                        replacement: replacement.to_owned(),
                        replaces: stored.replaces,
                        comments: stored.comments,
                        reason: stored.reason,
                        instruction: stored.instruction,
                        replaces_head: stored.replaces_head,
                    });
                }
            }
            payload.insert("continued".to_owned(), from);
            continued
                .settled
                .push((node.id.clone(), Continued(payload)));
            continue;
        }
        // A pin is a lifecycle node's alone — only a session cuts a branch — and one the
        // plan already states is the plan's.
        if node.repo.is_none() || node.branch.is_some() {
            continue;
        }
        // A dispatch the store still holds opened after the settlement beside it, since a
        // settlement removes the key: its branch is the one the work was last on.
        let left = settlement.as_ref().and_then(Stored::left_on);
        let dispatched =
            dispatch.and_then(|dispatch| crate::vcs::BranchName::checked(&dispatch.branch));
        if let Some(branch) = dispatched.or(left) {
            continued.pinned.push((node.id.clone(), branch));
        }
    }
    Ok(continued)
}

/// What a driver knows about its project's claim.
///
/// Read only where a driver must know: an adoption's first pass, and once immediately before
/// each publication starts, after the fence naming it has landed — never on a timer, and
/// never before a dispatch. A project changes driver only where somebody says so with
/// `--take-over`, so a driver reading its claim on a clock would spend the board's quota
/// confirming that nothing changed; and a read that cannot be answered ends nothing.
pub(crate) struct ClaimWatch {
    store: Store,
    project: QualifiedId,
    // llmlint: ignore-block[invalid_states_unrepresentable] a run id is `RunPaths::run`'s own
    // `String` and a host `sys::hostname`'s, each validated where it was minted or read, and
    // compared here only against a claim's — itself a wire document of strings; `src/error.rs`
    // records why this crate carries no `RunId` newtype the contract does not name.
    run: String,
    host: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// The run's own directory, where the store's request to be left alone is kept.
    run_dir: std::path::PathBuf,
    /// The claim this run's launch knowingly took over: still on the store until this
    /// driver's first projection replaces it, and no sign that anybody else drives.
    displaced: Option<DriverClaim>,
    /// An adoption whose first read is still to be made: made on its first pass the store
    /// has not asked to be left alone over.
    adopting: bool,
    /// The nodes the project's [`PUBLISHING_KEY`] names as this driver's publications under
    /// way, as far as this driver knows: `None` where it cannot say — an adoption before its
    /// first read, or after a write the store refused.
    fenced: Option<std::collections::BTreeSet<String>>,
    /// When the store last refused that write, so the loop wakes to write it again.
    fence_refused: Option<Instant>,
    /// When the read before a publication last went unanswered, so the loop wakes to read
    /// again.
    read_unanswered: Option<Instant>,
    /// A read found another run's claim: the fence this driver clears may since be that
    /// run's, so it reads before it clears.
    superseded: bool,
    /// The run's term scope, which a public boundary checks this driver's writes over.
    // llmlint: ignore[invalid_states_unrepresentable] the shape the store takes a scope in, as `writeback::Snapshot::term_scope` says.
    term_scope: Vec<String>,
}

/// What the read immediately before a publication lets the driver do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BeforePublishing {
    /// The read found the project this driver's — its own claim, the one it took over, or
    /// none: the publications go ahead.
    Publish,
    /// The read failed, outlasted its deadline, or was not made inside a wait the store asked
    /// for: the publications wait, and the read is made again once [`FENCE_RETRY`] has passed.
    Wait,
    /// The read found another run's claim, which supersedes this driver.
    Superseded(DriverClaim),
}

impl ClaimWatch {
    /// The watch over the project `launch` names, or `None` for a run naming none.
    ///
    /// A launch's driver starts on the read that let it launch — its own, or the one its
    /// takeover made once the driver it displaced had let go — and reads nothing more until
    /// a publication. An adoption's first pass reads.
    pub(crate) fn of_run(
        paths: &crate::ledger::RunPaths,
        launch: &crate::ledger::LaunchRecord,
        term_scope: Vec<String>,
    ) -> Option<Self> {
        let project = launch.project.parse().ok()?;
        let launched = launch.adoptions == 0;
        Some(Self {
            store: Store::at(&launch.dir),
            project,
            // As the store names this run: its public id, where the store does not declare
            // the project's source private.
            run: crate::publicid::on_store(launch),
            host: crate::sys::hostname(),
            run_dir: paths.dir.clone(),
            displaced: taken_over(paths),
            adopting: !launched,
            // A launch minted its run, so nothing on the store names its publications yet.
            fenced: launched.then(std::collections::BTreeSet::new),
            fence_refused: None,
            read_unanswered: None,
            superseded: false,
            term_scope,
        })
    }

    /// The claim this driver writes: this run, this host, the session that launched it,
    /// this process, from `claimed_at`.
    pub(crate) fn claim(&self, session: &str, claimed_at: &str) -> DriverClaim {
        DriverClaim {
            run: self.run.clone(),
            host: self.host.clone(),
            // Only a session a line can carry, which is the only one `claim_of` reads back.
            session: crate::vcs::usable(session),
            pid: crate::sys::pid(),
            claimed_at: claimed_at.to_owned(),
            lease: Some(lease_terms()),
        }
    }

    /// How long until the adoption's first read is due: now, where it is still to be made
    /// and the store has asked for no wait, and `Duration::MAX` once it has been made.
    pub(crate) fn until_adoption_read(&self) -> Duration {
        if !self.adopting {
            return Duration::MAX;
        }
        crate::writeback::store_wait_left(&self.run_dir).unwrap_or(Duration::ZERO)
    }

    /// Make an adoption's first read, once: the claim that supersedes this driver, if one
    /// does. `None` where this driver is no adoption or has read already, and — the read
    /// still due — inside a wait the store asked for.
    ///
    /// A read the store fails is said on standard error and answers nothing: a claim is best
    /// effort, and a store that cannot be read has not said anybody else drives.
    pub(crate) fn adoption_read(&mut self) -> Option<DriverClaim> {
        if !self.adopting {
            return None;
        }
        // A store that asked to be left alone is asked nothing inside the wait: every further
        // call extends a limiter nothing reports. The write-back is waiting it out too, and
        // the first pass after it has passed reads the claim.
        // llmlint: ignore-block[changed_behavior_has_e2e] driven end to end by `tests/e2e/store.rs`'s
        // `a_stop_inside_a_rate_limit_wait_releases_only_once_the_wait_has_passed` and
        // `a_closeout_inside_a_rate_limit_wait_asks_the_store_nothing`, which hold that an adopted
        // driver inside a wait the store asked for makes no store call at all, and that the driver
        // reaches the store again once the wait has passed.
        if crate::writeback::store_wait_left(&self.run_dir).is_some() {
            return None;
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]
        self.adopting = false;
        match self.read() {
            Ok(Some(claim)) if self.supersedes(&claim) => {
                self.superseded = true;
                Some(claim)
            }
            Ok(_) => None,
            Err(why) => {
                eprintln!(
                    "onepipeline: run '{}' could not read the claim on {}: {why}; it goes on \
                     driving, and reads it again before its next publication",
                    self.run,
                    self.project.as_str()
                );
                None
            }
        }
    }

    /// Read the claim immediately before a publication starts, once the fence naming it has
    /// landed: only a claim naming another run holds it back, for good, and a read that was
    /// not answered holds it until a later read is.
    ///
    /// A read finding no claim publishes: the claim may never have reached the board, and a
    /// run that took the project over and finished it has nothing left this driver could
    /// publish beside.
    pub(crate) fn before_publishing(&mut self) -> BeforePublishing {
        if self
            .read_unanswered
            .is_some_and(|at| at.elapsed() < FENCE_RETRY)
        {
            return BeforePublishing::Wait;
        }
        // Inside a wait the store asked for, as the fence's own write is not made in one.
        if let Some(left) = crate::writeback::store_wait_left(&self.run_dir) {
            eprintln!(
                "onepipeline: run '{}' waits {}s, as the store asked, to read the claim on {} \
                 before it publishes",
                self.run,
                left.as_secs(),
                self.project.as_str()
            );
            self.read_unanswered = Some(Instant::now());
            return BeforePublishing::Wait;
        }
        match self.read() {
            Ok(Some(claim)) if self.supersedes(&claim) => {
                self.read_unanswered = None;
                self.superseded = true;
                BeforePublishing::Superseded(claim)
            }
            Ok(_) => {
                self.read_unanswered = None;
                BeforePublishing::Publish
            }
            Err(why) => {
                eprintln!(
                    "onepipeline: run '{}' could not read the claim on {} before it publishes: \
                     {why}; the publication waits, and it reads again in {}s",
                    self.run,
                    self.project.as_str(),
                    FENCE_RETRY.as_secs()
                );
                self.read_unanswered = Some(Instant::now());
                BeforePublishing::Wait
            }
        }
    }

    /// One read of the project's claim, bounded by [`CLAIM_READ_DEADLINE`], which also
    /// refreshes what this driver knows of its fence.
    fn read(&mut self) -> Result<Option<DriverClaim>> {
        crate::loopstats::claim_read();
        self.store
            .project_metadata_within(&self.project, CLAIM_READ_DEADLINE)
            .and_then(|metadata| {
                self.fenced = self.fence_on(&metadata);
                claim_of(self.project.as_str(), &metadata)
            })
    }

    /// Whether `claim` names a run other than this one and other than the one this launch
    /// took the project over from.
    fn supersedes(&self, claim: &DriverClaim) -> bool {
        !claim.names(&self.run, &self.host) && self.displaced.as_ref() != Some(claim)
    }

    /// How long until a write of this driver's publications the store refused, or a read
    /// before a publication it did not answer, is due again.
    pub(crate) fn until_fence_retry(&self) -> Duration {
        [self.fence_refused, self.read_unanswered]
            .into_iter()
            .flatten()
            .map(|at| (at + FENCE_RETRY).saturating_duration_since(Instant::now()))
            .min()
            .unwrap_or(Duration::MAX)
    }

    /// The nodes `metadata`'s [`PUBLISHING_KEY`] names as this driver's: none where it is
    /// absent or another run's, and `None` where it is not a driver's publications at all.
    fn fence_on(
        &self,
        metadata: &BTreeMap<String, Value>,
    ) -> Option<std::collections::BTreeSet<String>> {
        match publishing_of(self.project.as_str(), metadata) {
            Ok(Some(publishing)) if publishing.run == self.run && publishing.host == self.host => {
                Some(publishing.nodes.into_iter().collect())
            }
            Ok(_) => Some(std::collections::BTreeSet::new()),
            Err(_) => None,
        }
    }

    /// Make the project's [`PUBLISHING_KEY`] name `nodes` as this driver's publications under
    /// way — `null` for none — and answer whether it does: a publication starts only once the
    /// fence naming it has landed, so a run taking the project over that finds no fence of
    /// this driver's knows none is under way.
    ///
    /// Written only where it differs from what the store is known to hold, bounded by
    /// [`CLAIM_READ_DEADLINE`], and not inside a wait the store asked for. A superseded
    /// driver reads before it clears, and clears only a fence of its own.
    pub(crate) fn fence(&mut self, nodes: &std::collections::BTreeSet<String>) -> bool {
        if self.fenced.as_ref() == Some(nodes) {
            return true;
        }
        if crate::writeback::store_asked_to_wait(&self.run_dir) {
            return false;
        }
        // A write the store refused is made again when its retry is due, and not on whatever
        // pass something else woke first.
        if self.fence_refused.is_some() && !self.until_fence_retry().is_zero() {
            return false;
        }
        if nodes.is_empty() && self.superseded {
            match self
                .store
                .project_metadata_within(&self.project, CLAIM_READ_DEADLINE)
            {
                Ok(metadata) => {
                    self.fenced = self.fence_on(&metadata);
                    if self.fenced.as_ref() == Some(nodes) {
                        return true;
                    }
                }
                Err(why) => {
                    eprintln!(
                        "onepipeline: run '{}' could not read its publications on {}: {why}; it \
                         clears them at its next pass",
                        self.run,
                        self.project.as_str()
                    );
                    return false;
                }
            }
        }
        let written = if nodes.is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::to_value(Publishing {
                run: self.run.clone(),
                host: self.host.clone(),
                nodes: nodes.iter().cloned().collect(),
                at: crate::sys::now_rfc3339(),
            })
        };
        let landed = written.map_err(|why| why.to_string()).and_then(|value| {
            self.store
                .set_project_metadata_within(
                    &self.project,
                    PUBLISHING_KEY,
                    &value,
                    CLAIM_READ_DEADLINE,
                    &self.term_scope,
                )
                .map_err(|why| why.to_string())
        });
        match landed {
            Ok(()) => {
                self.fenced = Some(nodes.clone());
                self.fence_refused = None;
                true
            }
            Err(why) => {
                eprintln!(
                    "onepipeline: run '{}' could not write its publications under way on {}: \
                     {why}; it starts no publication until a write lands",
                    self.run,
                    self.project.as_str()
                );
                // What the store holds now is not known: the write may yet land.
                self.fenced = None;
                self.fence_refused = Some(Instant::now());
                false
            }
        }
    }

    /// Record on the claim that superseded this driver that it has stopped: written once,
    /// best effort, over the claim exactly as it was read, with the acknowledgement beside
    /// it. What could not be written is said on standard error, and the adopter then waits
    /// out [`takeover_wait`] instead.
    pub(crate) fn acknowledge(&self, by: &DriverClaim) {
        let written = serde_json::to_value(by).map(|mut claim| {
            claim[ACKNOWLEDGED_KEY] = json!(Acknowledgement {
                run: self.run.clone(),
                host: self.host.clone(),
                at: crate::sys::now_rfc3339(),
            });
            claim
        });
        let outcome = match written {
            // llmlint: ignore-block[changed_behavior_has_e2e] the store's own wait is the
            // write-back's to keep, as every read of the claim keeps it; the adopter's journeys
            // drive the wait it falls back to (`a_takeover_whose_displaced_driver_died_proceeds_once_its_wait_has_passed`).
            _ if crate::writeback::store_asked_to_wait(&self.run_dir) => {
                Err("the store asked to be left alone".to_owned())
            }
            // llmlint: ignore-end[changed_behavior_has_e2e]
            Ok(claim) => self
                .store
                .set_project_metadata_within(
                    &self.project,
                    DRIVER_KEY,
                    &claim,
                    CLAIM_READ_DEADLINE,
                    &self.term_scope,
                )
                .map_err(|why| why.to_string()),
            Err(why) => Err(why.to_string()),
        };
        match outcome {
            Ok(()) => eprintln!(
                "onepipeline: run '{}' acknowledged run '{}' on host '{}' as the driver of {}",
                self.run,
                by.run,
                by.host,
                self.project.as_str()
            ),
            Err(why) => eprintln!(
                "onepipeline: run '{}' could not acknowledge run '{}' on host '{}': {why}; that \
                 run starts once its wait has passed with no publication of this one's under way",
                self.run, by.run, by.host
            ),
        }
    }
}

/// How a `--take-over` came to start: on the displaced driver's acknowledgement, or once
/// the wait for one had passed with none of its publications under way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HandedOver {
    /// The displaced driver wrote that it had stopped.
    Acknowledged(Acknowledgement),
    /// Nothing answered within [`takeover_wait`], and the project named none of the
    /// displaced driver's publications under way.
    WaitedOut,
}

/// Take the project over from the driver its claim named: write `mine`, then start nothing
/// until that driver has acknowledged it on the claim — which it does once it has read `mine`
/// before a publication, at its closeout or at its stop, with nothing of its own in progress —
/// or until [`takeover_wait`], sized by the cancel grace **its own claim** states, has passed
/// since the write landed and a read after that finds no publication of its under way on the
/// project's [`PUBLISHING_KEY`].
///
/// No publication of the displaced driver runs beside this run's: that driver writes the
/// fence naming a publication, and only once it has landed reads the claim, publishing only
/// on a read that answered and did not find `mine`; this run writes `mine`, and only once it
/// has landed reads the fence. So one of the two reads finds the other's write. A dispatch is
/// not fenced: a displaced driver that has not read `mine` yet may still start one, which
/// publishes nothing once it does.
///
/// A claim stating no lease terms — one an older build wrote, or a value that is not a
/// claim — names a driver whose fence this build cannot vouch for: the takeover then starts
/// only on an acknowledgement, which a driver of this build writes once it reads `mine`.
///
/// The store is read every tenth of an interval while it waits, at least a second and at
/// most thirty apart: the adopter carries the cost of a takeover, not every running driver.
///
/// # Errors
///
/// [`Error::Refused`] where the claim cannot be written, and [`Error::ProjectDriven`] where
/// another launch's claim replaces this one while it waits.
pub(crate) fn hand_over(
    store: &Store,
    project: &QualifiedId,
    mine: &DriverClaim,
    displaced: Option<&DriverClaim>,
    term_scope: &[String],
) -> Result<HandedOver> {
    store
        .set_project_metadata_within(
            project,
            DRIVER_KEY,
            &serde_json::to_value(mine).map_err(|why| Error::Invalid(why.to_string()))?,
            CLAIM_READ_DEADLINE,
            term_scope,
        )
        .map_err(|why| {
            Error::Refused(format!(
                "--take-over could not write its claim on {}: {why}",
                project.as_str()
            ))
        })?;
    let written = Instant::now();
    let wait = displaced.and_then(|claim| claim.lease).map(takeover_wait);
    let every = takeover_poll(claim_interval());
    let whom = displaced.map_or_else(
        || "the driver its claim named".to_owned(),
        |claim| format!("run '{}' on host '{}'", claim.run, claim.host),
    );
    match (wait, displaced) {
        (Some(wait), _) => eprintln!(
            "onepipeline: --take-over claimed {} for run '{}'; it starts once {whom} has \
             acknowledged, or in {}s once none of its publications is under way",
            project.as_str(),
            mine.run,
            wait.as_secs()
        ),
        (None, displaced) => eprintln!(
            "onepipeline: --take-over claimed {} for run '{}'; {whom} states no lease, so no \
             wait outlasts it: it starts once that driver has acknowledged{}",
            project.as_str(),
            mine.run,
            displaced.map_or_else(String::new, |claim| format!(
                ", or — taken over on host '{}' — once that host's records prove its run gone",
                claim.host
            ))
        ),
    }
    // What has been said of the displaced driver's publications, so each is said once.
    let mut said: Option<std::result::Result<Publishing, String>> = None;
    loop {
        let left = wait.map_or(every, |wait| wait.saturating_sub(written.elapsed()));
        std::thread::sleep(if left.is_zero() {
            every
        } else {
            every.min(left)
        });
        if wait.is_some_and(|wait| written.elapsed() >= wait) {
            let _ = crate::ordering::meet(TAKEOVER_PAST_WAIT_GATE_ENV);
        }
        let read = store
            .project_metadata_within(project, CLAIM_READ_DEADLINE)
            .and_then(|metadata| {
                stored_claim_of(project.as_str(), &metadata)
                    .map(|claim| (claim, publishing_of(project.as_str(), &metadata)))
            });
        let publishing = match read {
            Ok((Some((claim, acknowledged)), publishing))
                if claim.names(&mine.run, &mine.host) && claim.claimed_at == mine.claimed_at =>
            {
                if let Some(acknowledged) = acknowledged {
                    eprintln!(
                        "onepipeline: run '{}' on host '{}' acknowledged the takeover of {}: run \
                         '{}' starts",
                        acknowledged.run,
                        acknowledged.host,
                        project.as_str(),
                        mine.run
                    );
                    return Ok(HandedOver::Acknowledged(acknowledged));
                }
                publishing
            }
            Ok((Some((other, _)), _)) => {
                return Err(Error::ProjectDriven {
                    run: other.run,
                    host: other.host,
                    claimed_at: other.claimed_at,
                })
            }
            // Cleared by somebody else: no answer, and the wait goes on to its end.
            Ok((None, publishing)) => publishing,
            Err(why) => {
                eprintln!(
                    "onepipeline: --take-over could not read the claim on {}: {why}; it goes on \
                     waiting",
                    project.as_str()
                );
                continue;
            }
        };
        let Some(wait) = wait.filter(|wait| written.elapsed() >= *wait) else {
            continue;
        };
        // Past the wait the displaced driver can start nothing, and every fence it wrote for
        // a publication it started has landed: one standing names a publication still under
        // way, and one this read cannot make out could be hiding one.
        let standing = match publishing {
            Ok(Some(publishing))
                if displaced
                    .is_some_and(|claim| claim.names(&publishing.run, &publishing.host)) =>
            {
                Some(Ok(publishing))
            }
            Ok(_) => None,
            Err(why) => Some(Err(why.to_string())),
        };
        match standing {
            None => {
                eprintln!(
                    "onepipeline: {whom} has not acknowledged in {}s, and none of its \
                     publications is under way on {}: run '{}' starts",
                    wait.as_secs(),
                    project.as_str(),
                    mine.run
                );
                return Ok(HandedOver::WaitedOut);
            }
            Some(standing) if said.as_ref() != Some(&standing) => {
                match &standing {
                    Ok(publishing) => eprintln!(
                        "onepipeline: {whom} has not acknowledged in {}s, and its publication of \
                         {} has been under way since {}: run '{}' starts once that has finished, \
                         or once {whom} acknowledges",
                        wait.as_secs(),
                        publishing
                            .nodes
                            .iter()
                            .map(|node| format!("'{node}'"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        publishing.at,
                        mine.run
                    ),
                    Err(why) => eprintln!(
                        "onepipeline: {whom} has not acknowledged in {}s, and {why}, so a \
                         publication of its may be under way: run '{}' starts once {whom} \
                         acknowledges",
                        wait.as_secs(),
                        mine.run
                    ),
                }
                said = Some(standing);
            }
            Some(_) => {}
        }
    }
}

/// Release this run's claim on its project directly, where the closeout's own projection —
/// bounded by the closeout window — has not landed the release yet: a store slower than the
/// window would otherwise leave a run that has let go of the project claiming it, and refuse
/// the next host's launch over a run that is over.
///
/// Read first, as every release is, and written `null` only where the stored claim still
/// names this run on this host; each call bounded by [`CLAIM_READ_DEADLINE`]. Best effort:
/// what could not be done is said on standard error.
pub(crate) fn release_left_standing(
    paths: &crate::ledger::RunPaths,
    launch: &crate::ledger::LaunchRecord,
    term_scope: &[String],
) {
    let Ok(project) = launch.project.parse::<QualifiedId>() else {
        return;
    };
    let host = crate::sys::hostname();
    // llmlint: ignore[changed_behavior_has_e2e] reached only where a closeout projection outlasts its bounded window without failing, which `engine.rs`'s one caller of this function records no journey can drive (the real store suspended mid-command); the name read here is `publicid::on_store`, the one every release uses, and `public_id::a_claim_naming_the_public_id_is_read_back_taken_over_and_released` drives that name through the closeout's and the stop's releases.
    let run = crate::publicid::on_store(launch);
    // Both read as the launch reads a claim: one that is not one is nobody's to release.
    let ours = |metadata: &BTreeMap<String, Value>| {
        matches!(
            claim_of(project.as_str(), metadata),
            Ok(Some(claim)) if claim.names(&run, &host)
        )
    };
    let landed: BTreeMap<String, Value> = crate::writeback::landed_claim(&paths.dir)
        .map(|claim| BTreeMap::from([(DRIVER_KEY.to_owned(), claim)]))
        .unwrap_or_default();
    if !ours(&landed) || crate::writeback::store_asked_to_wait(&paths.dir) {
        return;
    }
    let store = Store::at(&launch.dir);
    let released = store
        .project_metadata_within(&project, CLAIM_READ_DEADLINE)
        .and_then(|metadata| {
            if !ours(&metadata) {
                return Ok(());
            }
            store.set_project_metadata_within(
                &project,
                DRIVER_KEY,
                &Value::Null,
                CLAIM_READ_DEADLINE,
                term_scope,
            )
        });
    if let Err(why) = released {
        eprintln!(
            "onepipeline: run '{}' could not release its claim on {}: {why}; a launch elsewhere \
             is refused over it until it is taken over",
            paths.run,
            project.as_str()
        );
    }
}

/// The claim this run's `start --take-over` passed, off its own `driver-taken-over`.
fn taken_over(paths: &crate::ledger::RunPaths) -> Option<DriverClaim> {
    crate::journal::read(&paths.journal())
        .into_iter()
        .filter(|event| {
            event.source == crate::event::Source::Pipeline
                && event.kind.0 == crate::journal::PipelineKind::DriverTakenOver.as_str()
        })
        .filter_map(|event| {
            serde_json::from_value::<crate::payload::DriverTakenOver>(Value::Object(event.payload))
                .ok()
        })
        .map(|taken| taken.previous)
        .next_back()
}

/// The claim interval: [`CLAIM_INTERVAL_ENV`]'s whole seconds, or [`CLAIM_INTERVAL`].
pub(crate) fn claim_interval() -> Duration {
    claim_interval_of(std::env::var(CLAIM_INTERVAL_ENV).ok().as_deref())
}

/// The interval the setting's value names: a positive whole number of seconds no longer than
/// [`MAX_CLAIM_INTERVAL`], and the default for anything else — unset, blank, zero, longer or
/// not a number — so a setting written wrong never stalls a takeover's reads or overflows a
/// wait.
// llmlint: ignore[changed_behavior_has_e2e] what a fallback produces is the five-minute default, which a journey could only observe by holding a takeover for minutes; every takeover journey drives a parsed value end to end (`tests/e2e/continuation.rs` sets the setting on each host), and `the_claim_interval_is_five_minutes_unless_set_to_a_positive_whole_number_up_to_a_day` holds every value that falls back.
fn claim_interval_of(setting: Option<&str>) -> Duration {
    setting
        .and_then(|seconds| seconds.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .filter(|interval| *interval <= MAX_CLAIM_INTERVAL)
        .unwrap_or(CLAIM_INTERVAL)
}

/// How often a takeover reads the claim while it waits: a tenth of the interval, at least
/// a second and at most thirty apart.
// llmlint: ignore[changed_behavior_has_e2e] the thirty-second cap binds only on an interval over five minutes, which no journey waits a takeover through; every takeover journey polls at its short interval's floor end to end, and `a_takeover_polls_a_tenth_of_its_interval_at_least_a_second_and_at_most_thirty_apart` holds both ends of the clamp.
fn takeover_poll(interval: Duration) -> Duration {
    (interval / TAKEOVER_POLLS_PER_INTERVAL).clamp(TAKEOVER_POLL_MIN, TAKEOVER_POLL_MAX)
}

/// How long an adopter waits for a displaced driver that does not acknowledge it, from the
/// moment its own claim landed, before it starts on finding no publication of that driver's
/// under way: the cancel grace the displaced driver's **own** claim states — the time its
/// host gives a dispatch to wind down, since a host is given its grace by itself — and
/// [`TAKEOVER_SLACK`]. No wait proves a driver that reads nothing on a timer has stopped; it
/// gives a live one the moment to answer, and the fence is what keeps the two drivers'
/// publications apart. Saturating: the grace is an operator's setting no smaller bound
/// holds, and a wait that could not be represented is one no takeover outlives anyway.
pub(crate) fn takeover_wait(terms: ClaimLease) -> Duration {
    Duration::from_secs(terms.cancel_grace_seconds.get()).saturating_add(TAKEOVER_SLACK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_superseded_head_is_read_only_as_a_full_commit_id() {
        let stored = |head: Value| {
            let mut record = json!({"replaces": "a", "reason": "the review asked",
                "instruction": "Rename it", "comments": []});
            if !head.is_null() {
                record["replaces_head"] = head;
            }
            rework_provenance(&record)
        };
        let sha1 = "1234567890abcdef1234567890abcdef12345678";
        let sha256 = "ab".repeat(32);
        assert_eq!(stored(Value::Null).unwrap().replaces_head, None);
        assert_eq!(
            stored(json!(sha1)).unwrap().replaces_head.as_deref(),
            Some(sha1)
        );
        assert_eq!(
            stored(json!(sha256)).unwrap().replaces_head,
            Some(sha256.clone())
        );
        for refused in [
            json!("main"),
            json!("1234567"),
            json!(format!("{sha1}0")),
            json!("g234567890abcdef1234567890abcdef12345678"),
            json!(format!("{sha1}\n")),
            json!(""),
        ] {
            let why = stored(refused.clone()).expect_err("refused");
            assert!(
                why.to_string().contains("no full commit id"),
                "{refused}: {why}"
            );
        }
    }

    /// Lease terms of `interval` and `grace` seconds, each positive.
    fn terms(interval: u64, grace: u64) -> ClaimLease {
        ClaimLease {
            interval_seconds: std::num::NonZeroU64::new(interval).expect("a positive interval"),
            cancel_grace_seconds: std::num::NonZeroU64::new(grace).expect("a positive grace"),
        }
    }

    fn claim(run: &str, host: &str) -> Value {
        json!({"run": run, "host": host, "session": null, "pid": 7,
               "claimed_at": "2026-10-11T00:00:00.000Z"})
    }

    #[test]
    fn the_claim_interval_is_five_minutes_unless_set_to_a_positive_whole_number_up_to_a_day() {
        assert_eq!(CLAIM_INTERVAL, Duration::from_secs(300));
        assert_eq!(claim_interval_of(Some(" 5 ")), Duration::from_secs(5));
        assert_eq!(claim_interval_of(Some("1200")), Duration::from_secs(1200));
        assert_eq!(claim_interval_of(Some("86400")), MAX_CLAIM_INTERVAL);
        for unusable in [
            None,
            Some(""),
            Some("0"),
            Some("-5"),
            Some("soon"),
            Some("86401"),
            Some("18446744073709551615"),
        ] {
            assert_eq!(claim_interval_of(unusable), CLAIM_INTERVAL, "{unusable:?}");
        }
    }

    /// A claim's lease reads only as this build writes one — a positive interval no longer
    /// than a day, a positive grace, and nothing else — and a driver's publications only as
    /// a run, a host, the nodes publishing and an instant.
    #[test]
    fn a_claims_lease_and_a_drivers_publications_read_only_as_this_build_writes_them() {
        let with = |lease: Value| {
            let mut written = claim("a", "h");
            written["lease"] = lease;
            BTreeMap::from([(DRIVER_KEY.to_owned(), written)])
        };
        let read = claim_of(
            "p",
            &with(json!({"interval_seconds": 5, "cancel_grace_seconds": 40})),
        )
        .expect("a lease reads")
        .expect("held");
        assert_eq!(read.lease, Some(terms(5, 40)));
        for (lease, says) in [
            (json!(5), "its `lease` is 5, not a mapping"),
            (
                json!({"interval_seconds": 5, "cancel_grace_seconds": 1, "slack": 3}),
                "its `lease` carries `slack`",
            ),
            (
                json!({"interval_seconds": 0, "cancel_grace_seconds": 1}),
                "expected a nonzero",
            ),
            (
                json!({"interval_seconds": 86_401, "cancel_grace_seconds": 1}),
                "a 86401s interval, not one of 1 to 86400 seconds",
            ),
            (
                json!({"interval_seconds": 5, "cancel_grace_seconds": 0}),
                "expected a nonzero",
            ),
            (json!({"interval_seconds": 5}), "missing field"),
        ] {
            let refused = claim_of("p", &with(lease.clone()))
                .expect_err(&format!("{lease} is not a lease"))
                .to_string();
            assert!(refused.contains(says), "{lease}: {refused}");
        }
        // A claim without one is a claim a build that kept no lease wrote.
        let bare = BTreeMap::from([(DRIVER_KEY.to_owned(), claim("a", "h"))]);
        assert_eq!(
            claim_of("p", &bare).expect("reads").expect("held").lease,
            None
        );

        let fence = |value: Value| BTreeMap::from([(PUBLISHING_KEY.to_owned(), value)]);
        assert_eq!(publishing_of("p", &BTreeMap::new()).expect("absent"), None);
        assert_eq!(publishing_of("p", &fence(Value::Null)).expect("null"), None);
        let at = "2026-10-16T09:40:58.551Z";
        assert_eq!(
            publishing_of(
                "p",
                &fence(json!({"run": "a", "host": "h", "nodes": ["x"], "at": at}))
            )
            .expect("reads"),
            Some(Publishing {
                run: "a".to_owned(),
                host: "h".to_owned(),
                nodes: vec!["x".to_owned()],
                at: at.to_owned()
            })
        );
        for value in [
            json!("x"),
            json!({"run": "a", "host": "h", "nodes": [], "at": at}),
            json!({"run": "a", "host": "", "nodes": ["x"], "at": at}),
            json!({"run": "a", "host": "h", "nodes": [""], "at": at}),
            json!({"run": "a", "host": "h", "nodes": ["x"], "at": "later"}),
            json!({"run": "a", "host": "h", "nodes": ["x"], "at": at, "pid": 3}),
        ] {
            let refused = publishing_of("p", &fence(value.clone()))
                .expect_err(&format!("{value} is not a driver's publications"))
                .to_string();
            assert!(
                refused.contains("`onepipeline.publishing` is not a driver's publications"),
                "{value}: {refused}"
            );
        }
    }

    /// A run letting go acknowledges another run's claim nothing has acknowledged — and no
    /// claim of its own, none acknowledged already, and nothing that is not a claim.
    #[test]
    fn a_run_letting_go_acknowledges_only_another_runs_unacknowledged_claim() {
        let on = |claim: Value| BTreeMap::from([(DRIVER_KEY.to_owned(), claim)]);
        let written = acknowledgement_over("p", &on(claim("b", "host-b")), "a", "host-a")
            .expect("another run's claim is acknowledged");
        let (read, acknowledged) = stored_claim_of("p", &on(written))
            .expect("it reads")
            .expect("held");
        assert!(read.names("b", "host-b"));
        assert!(acknowledged.is_some_and(|ack| ack.run == "a" && ack.host == "host-a"));

        let mut already = claim("b", "host-b");
        already[ACKNOWLEDGED_KEY] =
            json!({"run": "c", "host": "host-c", "at": "2026-10-11T00:00:01.000Z"});
        for left in [
            claim("a", "host-a"),
            already,
            json!("not a claim"),
            Value::Null,
        ] {
            assert_eq!(
                acknowledgement_over("p", &on(left.clone()), "a", "host-a"),
                None,
                "{left}"
            );
        }
    }

    #[test]
    fn a_takeover_polls_a_tenth_of_its_interval_at_least_a_second_and_at_most_thirty_apart() {
        assert_eq!(
            takeover_poll(Duration::from_secs(5)),
            Duration::from_secs(1)
        );
        assert_eq!(
            takeover_poll(Duration::from_secs(60)),
            Duration::from_secs(6)
        );
        assert_eq!(takeover_poll(CLAIM_INTERVAL), Duration::from_secs(30));
        assert_eq!(takeover_poll(MAX_CLAIM_INTERVAL), Duration::from_secs(30));
    }

    /// The adopter's wait is the displaced driver's own cancel grace and the slack, whatever
    /// interval either host reads on.
    #[test]
    fn the_takeover_wait_is_the_displaced_cancel_grace_and_the_slack() {
        assert_eq!(
            takeover_wait(terms(7, 45)),
            Duration::from_secs(45) + TAKEOVER_SLACK
        );
        assert_eq!(
            takeover_wait(terms(7, 45)),
            takeover_wait(terms(86_400, 45))
        );
        // A grace no duration can be added to saturates rather than overflowing.
        assert_eq!(takeover_wait(terms(1, u64::MAX)), Duration::MAX);
    }

    #[test]
    fn a_claim_absent_or_null_is_none_and_anything_else_not_a_claim_refuses() {
        assert_eq!(claim_of("p", &BTreeMap::new()).expect("absent"), None);
        assert_eq!(
            claim_of("p", &BTreeMap::from([(DRIVER_KEY.to_owned(), Value::Null)])).expect("null"),
            None
        );
        let held = claim_of(
            "p",
            &BTreeMap::from([(DRIVER_KEY.to_owned(), claim("ship", "builder"))]),
        )
        .expect("a claim")
        .expect("held");
        assert!(held.names("ship", "builder"));
        assert!(!held.names("ship", "elsewhere"));
        let mut extra = claim("ship", "builder");
        extra["lease"] = json!(30);
        let mut untimed = claim("ship", "builder");
        untimed["claimed_at"] = json!("yesterday");
        let unnamed = claim("ship", "");
        let broken = claim("ship\nrm -rf", "builder");
        for (bad, says) in [
            (extra, "`lease`"),
            (json!("ship"), "not a mapping"),
            (json!({"run": "ship"}), "missing field"),
            (untimed, "not an RFC 3339 instant"),
            (unnamed, "`host`"),
            (broken, "`run`"),
        ] {
            let refused = claim_of("p", &BTreeMap::from([(DRIVER_KEY.to_owned(), bad)]))
                .expect_err("refused")
                .to_string();
            assert!(refused.contains(says), "{refused}");
        }
    }

    /// A claim a displaced driver acknowledged reads as the claim it was, with the
    /// acknowledgement beside it; an acknowledgement that is not one refuses the claim.
    #[test]
    fn an_acknowledged_claim_reads_as_the_claim_with_its_acknowledgement() {
        let mut acknowledged = claim("ship", "builder");
        acknowledged[ACKNOWLEDGED_KEY] =
            json!({"run": "old", "host": "elsewhere", "at": "2026-10-11T00:01:00.000Z"});
        let metadata = BTreeMap::from([(DRIVER_KEY.to_owned(), acknowledged.clone())]);
        let (held, ack) = stored_claim_of("p", &metadata)
            .expect("an acknowledged claim")
            .expect("held");
        assert!(held.names("ship", "builder"));
        assert_eq!(
            ack,
            Some(Acknowledgement {
                run: "old".to_owned(),
                host: "elsewhere".to_owned(),
                at: "2026-10-11T00:01:00.000Z".to_owned()
            })
        );
        assert_eq!(claim_of("p", &metadata).expect("a claim"), Some(held));
        for (bad, says) in [
            (json!("yes"), "invalid type"),
            (json!({"run": "old", "host": "elsewhere"}), "missing field"),
            (
                json!({"run": "old", "host": "elsewhere", "at": "later"}),
                "not a run, a host and an instant",
            ),
            (
                json!({"run": "old", "host": "elsewhere", "at": "2026-10-11T00:01:00.000Z",
                       "pid": 9}),
                "unknown field",
            ),
        ] {
            let mut refused = claim("ship", "builder");
            refused[ACKNOWLEDGED_KEY] = bad;
            let why = claim_of("p", &BTreeMap::from([(DRIVER_KEY.to_owned(), refused)]))
                .expect_err("refused")
                .to_string();
            assert!(why.contains(says), "{why}");
        }
    }

    #[test]
    fn any_claim_refuses_a_launch_that_does_not_take_the_project_over() {
        let held: DriverClaim = serde_json::from_value(claim("ship", "builder")).expect("a claim");
        assert!(matches!(
            admit(Some(&held), false),
            Err(Error::ProjectDriven { run, host, .. }) if run == "ship" && host == "builder"
        ));
        assert!(admit(Some(&held), true).is_ok());
        assert!(admit(None, false).is_ok());
    }

    /// The properties a payload type's registered document names.
    fn properties<T: schemars::JsonSchema>() -> std::collections::BTreeSet<String> {
        let schema = serde_json::to_value(schemars::schema_for!(T)).expect("a schema renders");
        schema["properties"]
            .as_object()
            .expect("a document with properties")
            .keys()
            .cloned()
            .collect()
    }

    /// The keys the store boundary admits are the registered documents' own: a field a
    /// payload type gains is a field the boundary admits, rather than one it refuses.
    #[test]
    fn the_store_boundary_admits_exactly_the_keys_the_documents_name() {
        let claim: std::collections::BTreeSet<String> =
            CLAIM_KEYS.iter().map(|key| (*key).to_owned()).collect();
        assert_eq!(claim, properties::<DriverClaim>());
        let lease: std::collections::BTreeSet<String> =
            LEASE_KEYS.iter().map(|key| (*key).to_owned()).collect();
        assert_eq!(lease, properties::<ClaimLease>());
        let mut settled = properties::<crate::payload::NodeSettled>();
        settled.remove("continued");
        settled.insert("remote".to_owned());
        let stored: std::collections::BTreeSet<String> = SETTLEMENT_KEYS
            .iter()
            .map(|key| (*key).to_owned())
            .collect();
        assert_eq!(stored, settled);
        let set = |keys: &[&str]| -> std::collections::BTreeSet<String> {
            keys.iter().map(|key| (*key).to_owned()).collect()
        };
        assert_eq!(
            set(&STACK_KEYS),
            properties::<crate::payload::SettledStack>()
        );
        assert_eq!(
            set(&STACK_PARENT_KEYS),
            properties::<crate::payload::StackParent>()
        );
    }

    /// Entry 119's block is what this build does: its keys are the ones this build reads and
    /// writes, its dispatch and claim read as the boundary reads them, and its surface is the
    /// one the engine raises. Its idle interval and seam are entry 128's to replace.
    #[test]
    fn entry_119s_block_is_this_builds_continuation() {
        let record = include_str!("../docs/contract-divergences.md");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("119."))
            .expect("the divergence record carries entry 119");
        let block: Value = serde_json::from_str(
            entry
                .split("```json")
                .nth(1)
                .and_then(|rest| rest.split("```").next())
                .expect("entry 119 carries a json block"),
        )
        .expect("the block is JSON");
        assert_eq!(
            block["store_keys"],
            json!({"task": [SETTLEMENT_KEY, DISPATCH_KEY], "project": [DRIVER_KEY]})
        );
        assert_eq!(
            block["surface_kind"],
            json!(crate::engine::DRIVER_SUPERSEDED)
        );
        let metadata: Map<String, Value> = [(DISPATCH_KEY.to_owned(), block["dispatch"].clone())]
            .into_iter()
            .collect();
        assert!(dispatch_of("c", &metadata)
            .expect("the dispatch reads")
            .is_some());
        let project = BTreeMap::from([(DRIVER_KEY.to_owned(), block["driver"].clone())]);
        assert!(claim_of("p", &project).expect("the claim reads").is_some());
        let mut settlement = block["settlement"].clone();
        settlement
            .as_object_mut()
            .expect("a settlement")
            .remove("continued");
        let task: Map<String, Value> = [(SETTLEMENT_KEY.to_owned(), settlement)]
            .into_iter()
            .collect();
        assert!(settlement_of("c", Some(&task))
            .expect("the settlement reads")
            .is_some());
    }

    /// Entry 128's block is what this build does: its setting, interval, deadline, wait and
    /// poll are the constants, its acknowledged claim reads at the boundary, and the README
    /// states the setting and its default.
    #[test]
    fn entry_128s_block_is_what_this_build_does() {
        let record = include_str!("../docs/contract-divergences.md");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("128."))
            .expect("the divergence record carries entry 128");
        let block: Value = serde_json::from_str(
            entry
                .split("```json")
                .nth(1)
                .and_then(|rest| rest.split("```").next())
                .expect("entry 128 carries a json block"),
        )
        .expect("the block is JSON");
        assert_eq!(block["setting"], json!(CLAIM_INTERVAL_ENV));
        assert_eq!(
            block["claim_interval_seconds"],
            json!(CLAIM_INTERVAL.as_secs())
        );
        assert_eq!(
            block["claim_read_deadline_seconds"],
            json!(CLAIM_READ_DEADLINE.as_secs())
        );
        assert_eq!(
            block["max_claim_interval_seconds"],
            json!(MAX_CLAIM_INTERVAL.as_secs())
        );
        assert_eq!(
            block["claim_reads"],
            json!({"timed": false, "before_dispatch": false, "adoption": "its first pass",
                   "before_publication": "once, after its fence has landed"})
        );
        assert_eq!(
            block["unanswered_read_before_publication"],
            json!({"holds": "that publication", "retry_seconds": FENCE_RETRY.as_secs()})
        );
        assert_eq!(
            block["takeover_wait"],
            json!({"cancel_grace": crate::engine::CANCEL_GRACE_ENV,
                   "slack_seconds": TAKEOVER_SLACK.as_secs(),
                   "terms": "the displaced claim's lease",
                   "without_terms": "acknowledgement only"})
        );
        assert_eq!(
            block["settlement_landing_wait_seconds"],
            json!(SETTLEMENT_LANDING_WAIT.as_secs())
        );
        assert_eq!(block["publishing_key"], json!(PUBLISHING_KEY));
        assert_eq!(block["fence_retry_seconds"], json!(FENCE_RETRY.as_secs()));
        let fenced = BTreeMap::from([(PUBLISHING_KEY.to_owned(), block["publishing"].clone())]);
        assert!(publishing_of("p", &fenced)
            .expect("the fence reads")
            .is_some_and(|fence| fence.run == "drafts" && fence.nodes == ["x"]));
        assert_eq!(
            block["takeover_poll"],
            json!({"per_interval": TAKEOVER_POLLS_PER_INTERVAL,
                   "min_seconds": TAKEOVER_POLL_MIN.as_secs(),
                   "max_seconds": TAKEOVER_POLL_MAX.as_secs()})
        );
        assert_eq!(
            block["loop_stats_count"],
            json!(crate::loopstats::CLAIM_READS_KEY)
        );
        // What it retires is read nowhere in this build.
        for retired in block["retired"]
            .as_array()
            .expect("entry 128 names what it retires")
        {
            let retired = retired.as_str().expect("a setting");
            for source in [
                include_str!("continuation.rs"),
                include_str!("engine.rs"),
                include_str!("driver.rs"),
                include_str!("writeback.rs"),
            ] {
                assert!(!source.contains(retired), "{retired} is still read");
            }
        }
        assert_eq!(block["acknowledged_key"], json!(ACKNOWLEDGED_KEY));
        let project = BTreeMap::from([(DRIVER_KEY.to_owned(), block["driver"].clone())]);
        let (held, acknowledged) = stored_claim_of("p", &project)
            .expect("the acknowledged claim reads")
            .expect("held");
        assert!(held.names("drafts-2", "host-b"));
        assert_eq!(
            held.lease,
            Some(terms(
                CLAIM_INTERVAL.as_secs(),
                crate::engine::DEFAULT_CANCEL_GRACE_SECONDS
            ))
        );
        assert!(acknowledged.is_some_and(|ack| ack.run == "drafts" && ack.host == "builder"));
        let readme = include_str!("../README.md")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            readme.contains(&format!(
                "`{CLAIM_INTERVAL_ENV}`, {} by default",
                CLAIM_INTERVAL.as_secs()
            )),
            "the README does not state the claim interval's setting and default"
        );
    }

    /// A dispatch or a settlement that is not one refuses, naming the node and what is wrong.
    #[test]
    fn a_stored_dispatch_or_settlement_that_is_not_one_is_refused_naming_the_node() {
        let dispatch = |value: Value| -> Map<String, Value> {
            [(DISPATCH_KEY.to_owned(), value)].into_iter().collect()
        };
        assert_eq!(dispatch_of("b", &Map::new()).expect("none"), None);
        for (bad, says) in [
            (json!({"branch": "me/b", "base": "main"}), "missing field"),
            (
                json!({"branch": "me/b", "base": "main", "opened_at": "now"}),
                "not an RFC 3339 instant",
            ),
            (
                json!({"branch": "", "base": "main", "opened_at": "2026-10-11T00:00:00.000Z"}),
                "not one a session can name",
            ),
            (
                json!({"branch": "me/b", "base": "main", "opened_at": "2026-10-11T00:00:00.000Z",
                       "host": "builder"}),
                "unknown field",
            ),
        ] {
            let refused = dispatch_of("b", &dispatch(bad))
                .expect_err("refused")
                .to_string();
            assert!(
                refused.contains("node 'b'") && refused.contains(says),
                "{refused}"
            );
        }
        let settlement = |value: Value| -> Map<String, Value> {
            [(SETTLEMENT_KEY.to_owned(), value)].into_iter().collect()
        };
        for (bad, says) in [
            (json!("done"), "not a mapping"),
            (json!({"status": "done", "brnach": "me/b"}), "`brnach`"),
            (json!({"status": "finished"}), "unknown variant"),
            (
                json!({"status": "done", "base": "me/a", "stack": {"parents": [], "stack_base": null}}),
                "no usable parent",
            ),
            (
                json!({"status": "failed", "remote": "teleported"}),
                "`remote`",
            ),
            (
                json!({"status": "failed", "branch": "me/b\nme/c"}),
                "`branch`",
            ),
            (
                json!({"status": "done", "base": "me/a", "stack": {
                    "parents": [{"node": "a", "branch": "me/a", "head": "1f2e", "tip": "x"}],
                    "stack_base": null}}),
                "`tip`",
            ),
            (
                json!({"status": "done", "base": "me/a", "stack": {
                    "parents": [{"node": "a", "branch": "me/a", "head": "1f2e"}],
                    "stack_base": null, "root": "main"}}),
                "`root`",
            ),
        ] {
            let refused = settlement_of("b", Some(&settlement(bad)))
                .expect_err("refused")
                .to_string();
            assert!(
                refused.contains("node 'b'") && refused.contains(says),
                "{refused}"
            );
        }
    }
}
