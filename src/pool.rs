//! The sibling's worktree pool, as the scheduler reads it.
//!
//! `onevcs` keeps warm worktree slots per identity and admits sessions past them
//! up to an overflow bound, both sized by the host in its `workspaces.yml`. A
//! finite overflow means an identity can be **full**, and a session open against
//! a full identity is refused with [`onevcs::Error::PoolExhausted`] rather than
//! made to wait. Nothing about git enters here — every call is the linked
//! library's — and this crate adds no vocabulary of its own: the pool, the
//! overflow, the bound and what `0` and `unlimited` mean are the sibling's.
//!
//! What the scheduler does with that is three things. **Before** a queued
//! lifecycle node comes forward it asks [`onevcs::workspace_capacity`] with the
//! same request the dispatch would open with, and holds the node under
//! [`WorkspaceHold`] rather than dispatching into a refusal — a dispatch that
//! went ahead would spend the node's whole boundary budget on refusals and
//! settle it `failed` for a host being busy. The read is **advisory**: a read
//! that fails holds nothing, because `open` is authoritative and refuses on its
//! own account. **After** a refusal the read could not foresee — the race
//! between the read and the open, which nothing closes — the node is handed back
//! to the queue at no cost, and held on the refusal's own reading until the
//! next paced re-read; a re-dispatch refused this way keeps what it stood on,
//! which [`Workspaces::resuming`] hands back to the dispatch that resumes it,
//! so no refusal on any attempt of any node settles it or spends a boundary
//! attempt. And **while** any node is held, the wait is surfaced
//! non-blocking on the cadence a release wait is, and capacity is re-read on a
//! paced timer as well as on every pass: a session another run closes is not an
//! event this run sees.
//!
//! The hold sits **beside** the executor's host-wide `capacity()` — slots, load,
//! memory — and is never folded into it: one is what this host can run, the
//! other is what one repository identity can place.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use onevcs::{Bound, SessionRequest, WorkspaceCapacity};
use serde_json::{json, Value};

use crate::channel::Surface;
use crate::error::Result;
use crate::journal::Journal;
use crate::ledger::RunPaths;

/// The kind a held node's wait is surfaced under.
pub const WAIT_SURFACE_KIND: &str = "workspace-wait";

/// The word a `node-requeued` record carries for a node the identity's pool
/// refused a session to.
pub const EXHAUSTED_REASON: &str = "workspace-exhausted";

/// The word a `node-held` entry carries for this hold.
pub(crate) const HOLD_KIND: &str = "workspace";

/// The `node-requeued` a refused open is recorded as, in the shape the
/// divergence record fixes: the reason, the sibling's own account bounded as
/// every payload text is, and — for a re-dispatch that keeps its pin through
/// the queue — the branch the wait is against and the attempt it interrupted.
/// A first attempt has no pin and its record carries neither.
pub(crate) fn requeue_payload(
    because: &str,
    pinned: Option<(&str, std::num::NonZeroU32)>,
) -> serde_json::Map<String, Value> {
    let mut payload = crate::journal::payload(&[
        ("reason", json!(EXHAUSTED_REASON)),
        ("detail", json!(crate::engine::bounded(because))),
    ]);
    if let Some((branch, attempt)) = pinned {
        payload.insert("branch".to_owned(), json!(branch));
        payload.insert("attempt".to_owned(), json!(attempt));
    }
    payload
}

/// The environment variable bounding how often a held node's identity is
/// re-read for capacity when nothing else has moved.
pub const POLL_ENV: &str = "ONEPIPELINE_WORKSPACE_POLL_SECONDS";

/// How often a held node's identity is re-read when nothing overrides it.
///
/// A minute, and never longer — the bound every other answer the loop owes on a
/// clock is held to, and the same one the release watch's probe runs under. The
/// read is a survey of the identity's own records on this host, so it costs no
/// process and no network; what paces it is that a session another run closes
/// is not an event this run sees, and a re-read is how it finds out.
pub const DEFAULT_POLL_SECONDS: u64 = 60;

/// What a `workspace` hold carries: the identity's capacity as it was read.
///
/// The read's own numbers and nothing derived from them — a reader working out
/// why a node is waiting wants the pool the request resolved to, how many slots
/// exist and how many of those are idle or being maintained, and the overflow
/// bound with how much of it is spent. `in_use` is not carried because the
/// three that are, and `slots`, already say it; the holders themselves are the
/// sibling's to name, which the surface says how to ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceHold {
    /// The identity key the request resolved to.
    pub identity: String,
    /// The pool size the request resolves to.
    pub pool: u32,
    /// How many slots exist, whatever their state.
    pub slots: u32,
    /// How many of them are idle.
    pub idle: u32,
    /// How many a live maintenance run claims.
    pub maintaining: u32,
    /// The overflow bound the request resolves to.
    pub overflow: Bound,
    /// How many sessions of the identity are open under `runs/`, which is what
    /// the overflow bound counts.
    pub overflow_in_use: u32,
}

impl WorkspaceHold {
    /// The hold one capacity reading stands for.
    pub(crate) fn of(capacity: &WorkspaceCapacity) -> Self {
        Self {
            identity: capacity.identity.clone(),
            pool: capacity.pool,
            slots: capacity.slots,
            idle: capacity.idle,
            maintaining: capacity.maintaining,
            overflow: capacity.overflow,
            overflow_in_use: capacity.overflow_in_use,
        }
    }

    /// The `node-held` entry, as the divergence record fixes it: the bound is
    /// the sibling's own spelling, `"unlimited"` or the number.
    pub(crate) fn payload(&self) -> Value {
        json!({
            "kind": HOLD_KIND,
            "identity": self.identity,
            "pool": self.pool,
            "slots": self.slots,
            "idle": self.idle,
            "maintaining": self.maintaining,
            "overflow": self.overflow,
            "overflow_in_use": self.overflow_in_use,
        })
    }

    /// One `node-held` entry read back, or `None` for an entry that is not
    /// this hold or that this build cannot read whole.
    pub(crate) fn of_payload(entry: &Value) -> Option<Self> {
        if entry.get("kind")?.as_str()? != HOLD_KIND {
            return None;
        }
        let count = |key: &str| -> Option<u32> { u32::try_from(entry.get(key)?.as_u64()?).ok() };
        Some(Self {
            identity: entry.get("identity")?.as_str()?.to_owned(),
            pool: count("pool")?,
            slots: count("slots")?,
            idle: count("idle")?,
            maintaining: count("maintaining")?,
            overflow: serde_json::from_value(entry.get("overflow")?.clone()).ok()?,
            overflow_in_use: count("overflow_in_use")?,
        })
    }

    /// The numbers, on one line, as a view and the surface both say them.
    pub(crate) fn describe(&self) -> String {
        format!(
            "pool {} ({} slot(s): {} idle, {} maintaining), overflow {} with {} in use",
            self.pool, self.slots, self.idle, self.maintaining, self.overflow, self.overflow_in_use
        )
    }
}

/// Whether one node's session would be placed now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// It would: the dispatch goes ahead, and this pass has counted it.
    Admitted,
    /// It would not, on this reading: the node is held under it, and
    /// [`Workspaces::held`] carries the reading.
    Held,
    /// The identity could not be asked — unregistered, or a record that would
    /// not read. The dispatch goes ahead exactly as before there was a pool,
    /// because `open` is what decides and refuses on its own account, with the
    /// sibling's own reason.
    ///
    /// **Never a reading that found room.** The sibling refuses an unreadable
    /// session record rather than counting the slot it names as free, and this
    /// carries that refusal through rather than dropping it: the pass says what
    /// it could not read, on stderr, so a host whose sessions directory has gone
    /// is not a host whose pool reads have merely gone quiet.
    Unread,
}

/// What the loop knows about the workspaces its lifecycle nodes wait on.
///
/// One value for the whole run, threaded through the pass the way the release
/// watch is: the per-pass count of dispatches per identity, the nodes held and
/// why, the refusals still standing, and when each held node's wait was last
/// surfaced.
pub(crate) struct Workspaces {
    /// The nodes dispatched onto each identity whose session has not been seen
    /// to open yet, by node.
    ///
    /// A dispatch opens its session on its own thread, so the reading a second
    /// node of the same identity takes — in the same pass, or on the next one a
    /// second later — does not see the first's session yet, and two nodes
    /// leaving on one reading of a single free slot is exactly the refusal the
    /// read exists to prevent. So the loop keeps its own view of what it has
    /// placed and not yet seen placed, decrements the reading's `admits` by it,
    /// and trusts a reading whole only where it has nothing outstanding on that
    /// identity. An entry ends when the session's opening is relayed, or when
    /// the node leaves flight without one.
    placing: BTreeMap<String, String>,
    /// The nodes held this pass, and the reading each is held on.
    held: BTreeMap<String, WorkspaceHold>,
    /// The nodes whose open the identity refused, held on the refusal's own
    /// reading until the next paced re-read comes due.
    ///
    /// A refusal is the newest thing known about the identity — newer than any
    /// read, since it is what the open met — so a pass that re-read straight
    /// away and found room would dispatch into the same refusal again, and a
    /// pass that found none would say what the refusal already said. Either
    /// way the next read is the paced one.
    refused: BTreeMap<String, (Instant, WorkspaceHold)>,
    /// Where each refused re-dispatch stood, for the dispatch that resumes it.
    ///
    /// A first attempt refused has nothing behind it and is absent here: it
    /// resumes from the plan's own node. A later one is pinned to the branch
    /// the attempt before preserved, and the queue would not know to pin it
    /// there — so what the loop had in hand travels through the queue beside
    /// the hold, and the node leaves on it. Kept until the node leaves.
    resuming: BTreeMap<String, Box<crate::lifecycle::Continuation>>,
    /// When the identities were last read for a held node.
    read_at: Option<Instant>,
    /// How often a held node's identity is re-read.
    every: Duration,
    /// When each held node's wait was last surfaced.
    surfaced: BTreeMap<String, Instant>,
    /// How often a held node's wait is surfaced.
    surface_every: Duration,
}

impl Workspaces {
    pub(crate) fn new() -> Self {
        Self {
            placing: BTreeMap::new(),
            held: BTreeMap::new(),
            refused: BTreeMap::new(),
            resuming: BTreeMap::new(),
            read_at: None,
            every: Duration::from_secs(poll_seconds()),
            surfaced: BTreeMap::new(),
            surface_every: Duration::from_secs(crate::release::surface_every_seconds()),
        }
    }

    /// Open a pass: forget the last pass's holds, and every placement whose
    /// node is no longer in flight. What is held this pass is decided by the
    /// reads this pass makes.
    pub(crate) fn begin_pass(&mut self, in_flight: impl Fn(&str) -> bool) {
        self.held.clear();
        self.placing.retain(|node, _| in_flight(node));
    }

    /// The session `node`'s dispatch opened has been seen: it is on the
    /// identity's own records now, so a reading counts it.
    pub(crate) fn placed(&mut self, node: &str) {
        self.placing.remove(node);
    }

    /// Whether `node`'s session, asked for by `request`, would be placed now.
    ///
    /// Reads the identity fresh for every node asked — the pass is the unit of
    /// freshness, and a node held on a stale reading is a node held a pass too
    /// long — then applies this pass's own dispatches over the reading.
    pub(crate) fn admit(&mut self, node: &str, request: &SessionRequest) -> Admission {
        // A refusal the identity made stands until the paced re-read.
        if let Some((at, hold)) = self.refused.get(node) {
            if at.elapsed() < self.every {
                self.held.insert(node.to_owned(), hold.clone());
                return Admission::Held;
            }
            self.refused.remove(node);
        }
        let capacity = match crate::vcs::workspace_capacity(request) {
            Ok(capacity) => capacity,
            // See [`Admission::Unread`].
            Err(why) => {
                eprintln!(
                    "onepipeline: cannot read what the '{}' workspace admits, so '{node}' is \
                     dispatched without that reading and its own open decides: {why}",
                    request.repo
                );
                return Admission::Unread;
            }
        };
        self.read_at = Some(Instant::now());
        let already = u32::try_from(
            self.placing
                .values()
                .filter(|identity| **identity == capacity.identity)
                .count(),
        )
        .unwrap_or(u32::MAX);
        // The reading is what the open would meet, so it is trusted whole where
        // the loop has nothing outstanding on the identity — a pinned branch an
        // open session already holds is admitted without a slot, and only the
        // reading knows that. Past that, what is left is the headroom the
        // reading counted, less what the loop has placed and not yet seen.
        let admitted = capacity.admitted && (already == 0 || capacity.admits.admits(already));
        if !admitted {
            self.held
                .insert(node.to_owned(), WorkspaceHold::of(&capacity));
            return Admission::Held;
        }
        self.placing.insert(node.to_owned(), capacity.identity);
        Admission::Admitted
    }

    /// `node`'s open was just refused: hold it on the reading taken after the
    /// refusal until the next paced read — where there is one; a read that
    /// failed holds nothing, and the node is queued again — and keep where it
    /// stood, for the dispatch that resumes it.
    pub(crate) fn refused(
        &mut self,
        node: &str,
        hold: Option<WorkspaceHold>,
        resume: Option<Box<crate::lifecycle::Continuation>>,
    ) {
        if let Some(hold) = hold {
            self.refused.insert(node.to_owned(), (Instant::now(), hold));
        }
        match resume {
            Some(continuation) => self.resuming.insert(node.to_owned(), continuation),
            None => self.resuming.remove(node),
        };
    }

    /// Where `node` resumes from, taken for the dispatch about to leave on it:
    /// `None` for a node that starts from the plan's own node.
    pub(crate) fn resuming(&mut self, node: &str) -> Option<Box<crate::lifecycle::Continuation>> {
        self.resuming.remove(node)
    }

    /// The dispatch did not leave after all — the node was held — so what it
    /// would have resumed from is kept for the pass it does.
    pub(crate) fn keep_resuming(
        &mut self,
        node: &str,
        resume: Option<Box<crate::lifecycle::Continuation>>,
    ) {
        if let Some(continuation) = resume {
            self.resuming.insert(node.to_owned(), continuation);
        }
    }

    /// The nodes held this pass, and the reading each is held on.
    pub(crate) fn held(&self) -> &BTreeMap<String, WorkspaceHold> {
        &self.held
    }

    /// How long the loop may wait before a held identity is due to be re-read.
    ///
    /// `Duration::MAX` where nothing is held, so a run with no pool business
    /// waits on the channel alone.
    pub(crate) fn next_read(&self) -> Duration {
        if self.held.is_empty() && self.refused.is_empty() {
            return Duration::MAX;
        }
        let until = |since: Instant| self.every.saturating_sub(since.elapsed());
        let refusals = self.refused.values().map(|(at, _)| until(*at));
        let reads = self.read_at.map(until).into_iter();
        refusals.chain(reads).min().unwrap_or(Duration::ZERO)
    }

    /// Surface every held node's wait that is due, and forget the nodes no
    /// longer held so a wait that ends is not repeated after it.
    ///
    /// Non-blocking, on the cadence a release wait is surfaced at: the hold is
    /// the scheduler's, and a blocking surface would hold the same subtree
    /// twice while reading, in every planner view, as a decision somebody has to
    /// answer before the run can move. This one is a report — the decision it
    /// informs is whether to keep waiting, close a session somewhere, or resize
    /// the identity.
    pub(crate) fn surface_waits(&mut self, paths: &RunPaths, journal: &mut Journal) -> Result<()> {
        for (node, hold) in &self.held {
            let due = self
                .surfaced
                .get(node)
                .is_none_or(|last| last.elapsed() >= self.surface_every);
            if !due {
                continue;
            }
            self.surfaced.insert(node.clone(), Instant::now());
            crate::engine::raise(paths, journal, wait_surface(node, hold))?;
        }
        let held = &self.held;
        self.surfaced.retain(|node, _| held.contains_key(node));
        Ok(())
    }
}

/// The surface a held node's wait raises.
fn wait_surface(node: &str, hold: &WorkspaceHold) -> Surface {
    Surface {
        id: 0,
        kind: WAIT_SURFACE_KIND.to_owned(),
        message: format!(
            "node '{node}' is held: the '{identity}' workspace admits no more sessions now — \
             {numbers}. `onevcs pool status {identity}` names the holders. Nothing times this \
             out and nothing will fail the node: it dispatches the pass the identity admits \
             it. Close a session, raise pool or overflow for this identity in the host's \
             workspaces.yml, or stop the run.",
            identity = hold.identity,
            numbers = hold.describe(),
        ),
        source: crate::channel::source::PROPOSAL.to_owned(),
        blocking: false,
        queued_at: crate::sys::now_millis(),
        abandoned: false,
        asker: None,
        workstream: Some(node.to_owned()),
        correlation: None,
    }
}

/// How often a held node's identity is re-read.
fn poll_seconds() -> u64 {
    std::env::var(POLL_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_POLL_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_hold() -> WorkspaceHold {
        WorkspaceHold {
            identity: "github.com/owner/service".into(),
            pool: 1,
            slots: 1,
            idle: 0,
            maintaining: 0,
            overflow: Bound::Bounded(0),
            overflow_in_use: 0,
        }
    }

    /// The hold's entry is what the divergence record fixes, and it reads back
    /// whole — with the bound in the sibling's own spelling both ways.
    #[test]
    fn the_hold_entry_is_what_the_divergence_record_fixes_and_round_trips() {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
        )
        .expect("the divergence record reads");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("82."))
            .expect("the divergence record still carries entry 82");
        let block = entry
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("entry 82 carries the json block this test drives");
        let block: Value = serde_json::from_str(block).expect("entry 82's block is JSON");

        let written = block["hold"].clone();
        let hold = WorkspaceHold::of_payload(&written).expect("entry 82's hold entry reads");
        assert_eq!(
            hold.payload(),
            written,
            "entry 82's hold entry does not round-trip as written"
        );
        assert_eq!(block["surface_kind"], json!(WAIT_SURFACE_KIND));
        assert_eq!(block["requeue"]["reason"], json!(EXHAUSTED_REASON));
        // The requeue carries the fields the entry names and no others: a first
        // attempt's the two, a pinned re-dispatch's those and the pin.
        let fields = |payload: serde_json::Map<String, Value>| -> Vec<String> {
            payload.keys().cloned().collect()
        };
        let named = |key: &str| -> Vec<String> {
            serde_json::from_value(block["requeue"][key].clone()).expect("a field list")
        };
        let mut unpinned = fields(requeue_payload("pool exhausted", None));
        unpinned.sort();
        let mut first = named("fields");
        first.sort();
        assert_eq!(unpinned, first);
        let pinned = requeue_payload(
            "pool exhausted",
            Some((
                "onepipeline/service",
                std::num::NonZeroU32::new(2).expect("two"),
            )),
        );
        assert_eq!(pinned["reason"], json!(EXHAUSTED_REASON));
        assert_eq!(pinned["branch"], json!("onepipeline/service"));
        assert_eq!(pinned["attempt"], json!(2));
        let mut all = fields(pinned.clone());
        all.sort();
        let mut both = named("fields");
        both.extend(named("pinned_fields"));
        both.sort();
        assert_eq!(all, both);
        // And the registered `node-requeued` document names both shapes: the
        // entry's two fields are what every writer writes, the pin's two are
        // named and optional — a document that merely tolerated them as unknown
        // keys would say nothing about a pinned record — and each shape is
        // admitted as this crate writes it.
        let registry = crate::event::registry();
        let id = crate::payload::schema_of(crate::event::PipelineKind::NodeRequeued);
        let document = registry.schema(&id).expect("node-requeued is registered");
        let required: Vec<String> =
            serde_json::from_value(document["required"].clone()).expect("required keys");
        for key in named("fields") {
            assert!(
                required.contains(&key),
                "`{key}` is not required: {document}"
            );
        }
        for key in named("pinned_fields") {
            assert!(
                document["properties"].get(&key).is_some(),
                "the document does not name the pin's `{key}`: {document}"
            );
            assert!(
                !required.contains(&key),
                "the pin's `{key}` is required: {document}"
            );
        }
        for shape in [requeue_payload("pool exhausted", None), pinned] {
            registry
                .check(&id, &Value::Object(shape.clone()))
                .unwrap_or_else(|refusal| {
                    panic!("the document refuses a requeue this crate writes: {refusal}\n{shape:?}")
                });
        }

        let unlimited = WorkspaceHold {
            overflow: Bound::Unlimited,
            ..a_hold()
        };
        assert_eq!(unlimited.payload()["overflow"], json!("unlimited"));
        assert_eq!(
            WorkspaceHold::of_payload(&unlimited.payload()),
            Some(unlimited)
        );
        // Another hold's entry is not this one, nor is one missing a number.
        assert_eq!(
            WorkspaceHold::of_payload(&json!({"kind": "release", "awaiting": []})),
            None
        );
        let mut short = a_hold().payload();
        short.as_object_mut().expect("an object").remove("idle");
        assert_eq!(WorkspaceHold::of_payload(&short), None);
    }

    /// The surface names the identity, the numbers, the sibling's verb that
    /// names the holders, and that nothing times it out.
    #[test]
    fn the_wait_surface_says_what_a_reader_needs_and_blocks_nothing() {
        let surface = wait_surface("service", &a_hold());
        assert_eq!(surface.kind, WAIT_SURFACE_KIND);
        assert!(!surface.blocking);
        assert_eq!(surface.workstream.as_deref(), Some("service"));
        for names in [
            "'github.com/owner/service' workspace admits no more sessions",
            "pool 1 (1 slot(s): 0 idle, 0 maintaining), overflow 0 with 0 in use",
            "onevcs pool status github.com/owner/service",
            "Nothing times this out",
        ] {
            assert!(
                surface.message.contains(names),
                "the surface does not say {names:?}: {}",
                surface.message
            );
        }
    }

    /// The poll bound falls back to the shipped one rather than to zero or to
    /// no bound at all — on `release::tests`' terms, and held here structurally
    /// for the same reason that one is: the journeys set the knob to a usable
    /// value to drive the re-read, and a value nothing can wait on has no
    /// journey, since the behaviour it selects is the shipped minute.
    #[test]
    fn an_unusable_poll_bound_falls_back_to_the_shipped_one() {
        let _held = crate::vcs::scratch_home_held();
        for unusable in ["0", "", "soon", "-1"] {
            std::env::set_var(POLL_ENV, unusable);
            assert_eq!(
                poll_seconds(),
                DEFAULT_POLL_SECONDS,
                "{POLL_ENV}={unusable:?}"
            );
        }
        std::env::set_var(POLL_ENV, "7");
        assert_eq!(poll_seconds(), 7);
        assert_eq!(Workspaces::new().every, Duration::from_secs(7));
        std::env::remove_var(POLL_ENV);
        assert_eq!(poll_seconds(), DEFAULT_POLL_SECONDS);
    }

    /// A refusal stands until the paced read comes due, and the next read is
    /// timed from it; nothing held means nothing to wait for.
    #[test]
    fn a_refusal_holds_the_node_until_the_paced_read_and_paces_the_wait() {
        let mut workspaces = Workspaces::new();
        assert_eq!(workspaces.next_read(), Duration::MAX);
        workspaces.every = Duration::from_secs(3_600);
        workspaces.refused("service", Some(a_hold()), None);
        let request = SessionRequest {
            repo: "nowhere".into(),
            branch: None,
            base: None,
            execution_checkout: None,
            pool: None,
            overflow: None,
            labels: Default::default(),
        };
        // Held on the refusal's reading, with no read made: the request names
        // an identity no registry holds, which a read would have refused.
        workspaces.begin_pass(|_| false);
        assert_eq!(workspaces.admit("service", &request), Admission::Held);
        assert_eq!(workspaces.held().get("service"), Some(&a_hold()));
        assert!(workspaces.next_read() <= Duration::from_secs(3_600));
        assert!(workspaces.next_read() > Duration::from_secs(3_000));
    }
}
