//! A node whose prerequisites a private source could not be asked about, held until it
//! can be.
//!
//! The plan store answers a task's dependency read without failing it when a source
//! declared private cannot be asked what it records of the task — its credential absent or
//! refused, or it unreachable or rate-limited — and names that source beside the edges the
//! other sources hold. A reader without an organization's credential still reads the plan,
//! which is the point. What it cannot read is whether that source records a prerequisite of
//! the task that no other source holds, and a node dispatched on the edges it could read
//! would be dispatched as if it had no such prerequisite.
//!
//! So the plan loads and such a node is **held** under a `source` reason rather than
//! dispatched: the launch says so on its first records, and the driver re-reads the node's
//! edges on a paced timer, off the loop's thread because a hosted source can take its time
//! not answering. The hold clears the pass a re-read asks every source and finds no
//! prerequisite the run's graph does not already give the node; one it does find keeps the
//! node held, naming it, until an edit gives the node that edge. While held, the wait is
//! surfaced non-blocking on the cadence a release wait is, naming the source, why it did not
//! answer and what would let it. Nothing times the hold out. A node whose read asked every
//! source loads and dispatches as it always did.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use onetaskgraph_plugin_api::SourceName;
use serde_json::{json, Value};

use crate::channel::Surface;
use crate::error::Result;
use crate::journal::Journal;
use crate::ledger::RunPaths;
use crate::plan::Node;
use crate::taskgraph::{QualifiedId, Reread, Store, Unanswered};

/// The kind a held node's wait is surfaced under.
pub const WAIT_SURFACE_KIND: &str = "source-wait";

/// The word a `node-held` entry carries for this hold.
pub(crate) const HOLD_KIND: &str = "source";

/// The environment variable bounding how often a held node's dependency edges are read
/// again.
pub const POLL_ENV: &str = "ONEPIPELINE_SOURCE_POLL_SECONDS";

/// How often a held node's edges are read again when nothing overrides it: a minute, the
/// interval every other paced read of the loop ships with. A host wanting its store asked
/// less often lengthens [`POLL_ENV`]; a value that is not a positive whole number of
/// seconds is no setting at all, and this stands.
pub const DEFAULT_POLL_SECONDS: u64 = 60;

/// How soon the loop looks again for a re-read it has under way.
const TICK: Duration = Duration::from_millis(200);

/// What a `source` hold carries: each source the node's last dependency read could not ask
/// and why, and each prerequisite that read found which the run's graph does not give the
/// node.
///
/// Never neither: a hold names what holds the node, so the one constructor, [`new`](Self::new),
/// answers no hold at all for a read that asked every source and found nothing missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceHold {
    /// The sources not asked, in name order; empty only where `missing` is not.
    unanswered: Vec<Unanswered>,
    /// The node ids the read resolved a prerequisite to that the node does not depend on in
    /// the run's graph.
    missing: Vec<String>,
}

impl SourceHold {
    /// The hold these sources and prerequisites stand for, or `None` where both are empty
    /// and nothing holds the node.
    pub(crate) fn new(unanswered: Vec<Unanswered>, missing: Vec<String>) -> Option<Self> {
        (!unanswered.is_empty() || !missing.is_empty()).then_some(Self {
            unanswered,
            missing,
        })
    }

    pub(crate) fn unanswered(&self) -> &[Unanswered] {
        &self.unanswered
    }

    /// The `node-held` entry: `missing` only where the read found one.
    pub(crate) fn payload(&self) -> Value {
        let mut entry = json!({
            "kind": HOLD_KIND,
            "unanswered": self
                .unanswered
                .iter()
                .map(|unasked| json!({"source": unasked.source.as_str(), "reason": unasked.reason}))
                .collect::<Vec<_>>(),
        });
        if !self.missing.is_empty() {
            entry["missing"] = json!(self.missing);
        }
        entry
    }

    /// One `node-held` entry read back, or `None` for an entry that is not this hold or that
    /// this build cannot read whole.
    pub(crate) fn of_payload(entry: &Value) -> Option<Self> {
        if entry.get("kind")?.as_str()? != HOLD_KIND {
            return None;
        }
        let unanswered = entry
            .get("unanswered")?
            .as_array()?
            .iter()
            .map(|unasked| {
                Some(Unanswered {
                    source: SourceName::new(unasked.get("source")?.as_str()?).ok()?,
                    reason: unasked.get("reason")?.as_str()?.to_owned(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let missing = match entry.get("missing") {
            None => Vec::new(),
            Some(missing) => missing
                .as_array()?
                .iter()
                .map(|id| id.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()?,
        };
        Self::new(unanswered, missing)
    }

    /// What holds the node, on one line, as a view and the surface both say it.
    pub(crate) fn describe(&self) -> String {
        let mut said: Vec<String> = self
            .unanswered
            .iter()
            .map(|unasked| {
                format!(
                    "source '{}', declared private, could not be asked what it records of \
                     its task: {}",
                    unasked.source, unasked.reason
                )
            })
            .collect();
        if !self.missing.is_empty() {
            said.push(format!(
                "the store records it depending on {}, which this run's graph does not",
                quoted(&self.missing)
            ));
        }
        said.join("; ")
    }

    /// The sources it names, as a list a person reads.
    pub(crate) fn sources(&self) -> String {
        quoted(
            &self
                .unanswered
                .iter()
                .map(|unasked| unasked.source.to_string())
                .collect::<Vec<_>>(),
        )
    }
}

/// The holds a launch's read of `plan` stands for: each node of it whose dependency read
/// could not ask a source declared private, held for those sources.
pub(crate) fn launched(
    plan: &crate::plan::Plan,
    unanswered: &BTreeMap<String, Vec<Unanswered>>,
) -> BTreeMap<String, SourceHold> {
    plan.tasks
        .iter()
        .filter_map(|node| {
            let unasked = unanswered.get(&node.id)?;
            Some((
                node.id.clone(),
                SourceHold::new(unasked.clone(), Vec::new())?,
            ))
        })
        .collect()
}

/// What a launch, and a `plan check`, say on stderr about the nodes `held` holds: one line
/// per source, naming it, why it did not answer, and the nodes it holds. `None` where
/// nothing is held.
pub(crate) fn notice(held: &BTreeMap<String, SourceHold>) -> Option<String> {
    let mut by_source: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for (node, hold) in held {
        for unasked in hold.unanswered() {
            by_source
                .entry((unasked.source.to_string(), unasked.reason.clone()))
                .or_default()
                .push(node.clone());
        }
    }
    if by_source.is_empty() {
        return None;
    }
    Some(
        by_source
            .into_iter()
            .map(|((source, reason), nodes)| {
                format!(
                    "onepipeline: source '{source}', declared private, could not be asked what                      it records of the tasks of {}: {reason} — the plan loads, and each of                      those nodes is held, never dispatched, until a read of its dependencies                      asks every source",
                    quoted(&nodes)
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// What holds a node a re-read of its edges answered, or `None` where nothing does: every
/// source was asked, and every prerequisite the read found is one of `deps`, the node's
/// dependencies in the run's graph.
pub(crate) fn hold_after(deps: &[String], reread: &Reread) -> Option<SourceHold> {
    let mut missing: Vec<String> = reread
        .deps
        .iter()
        .filter(|dep| !deps.contains(dep))
        .cloned()
        .collect();
    missing.sort();
    missing.dedup();
    SourceHold::new(reread.unanswered.clone(), missing)
}

/// A re-read under way, on its own thread.
type Reading = Receiver<Result<BTreeMap<String, Reread>>>;

/// What the loop knows about the nodes it holds for a source that did not answer.
///
/// One value for the whole run, threaded through the pass the way the workspace holds are:
/// the nodes held and why, the re-read under way, and when each held node's wait was last
/// surfaced.
pub(crate) struct SourceHolds {
    held: BTreeMap<String, SourceHold>,
    /// The store the launch read the plan through, and the project: `None` only for a launch
    /// record naming no qualified project, which nothing re-reads.
    store: Option<(Store, QualifiedId)>,
    reading: Option<Reading>,
    read_at: Option<Instant>,
    every: Duration,
    surfaced: crate::release::Resurfacing,
}

impl SourceHolds {
    /// The holds the run's record says are standing, taken up by a driver that re-reads
    /// them through the store configured from `dir` for `project`.
    ///
    /// Seeded from the record, unlike a workspace hold: what decided this one is the
    /// launch's read, which no driver repeats, so a fresh driver that started empty would
    /// dispatch every node the launch held.
    pub(crate) fn of_run(
        holds: &BTreeMap<String, Vec<Value>>,
        dir: &std::path::Path,
        project: &str,
    ) -> Self {
        let held = holds
            .iter()
            .filter_map(|(node, reasons)| {
                reasons
                    .iter()
                    .find_map(SourceHold::of_payload)
                    .map(|hold| (node.clone(), hold))
            })
            .collect();
        let store = project
            .parse::<QualifiedId>()
            .ok()
            .map(|project| (Store::at(dir), project));
        Self {
            held,
            store,
            reading: None,
            read_at: None,
            every: Duration::from_secs(poll_seconds()),
            surfaced: crate::release::Resurfacing::new(),
        }
    }

    /// Forget every node no longer waiting to start: a node an edit dropped, and one that
    /// started or settled, holds nothing.
    pub(crate) fn retain(&mut self, waiting: impl Fn(&str) -> bool) {
        self.held.retain(|node, _| waiting(node));
    }

    /// Take up what a finished re-read answered, and say whether it changed a hold.
    ///
    /// `deps` is a node's dependencies in the run's graph as they stand now, so an edit that
    /// gave a held node the prerequisite a read found is what the next read clears it on.
    pub(crate) fn take_up(&mut self, deps: impl Fn(&str) -> Option<Vec<String>>) -> bool {
        let Some(reading) = &self.reading else {
            return false;
        };
        let answered = match reading.try_recv() {
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                self.reading = None;
                return false;
            }
            Ok(answered) => answered,
        };
        self.reading = None;
        let reread = match answered {
            Ok(reread) => reread,
            Err(why) => {
                // The store refused the read outright, which says nothing about whether the
                // private source would answer: the holds stand as they are, and the next
                // paced read asks again.
                eprintln!(
                    "onepipeline: cannot read again the dependencies of {}, so they stay held: \
                     {why}",
                    quoted(&self.held.keys().cloned().collect::<Vec<_>>())
                );
                return false;
            }
        };
        let mut changed = false;
        for (node, read) in reread {
            if !self.held.contains_key(&node) {
                continue;
            }
            let Some(deps) = deps(&node) else {
                continue;
            };
            match hold_after(&deps, &read) {
                None => {
                    self.held.remove(&node);
                    changed = true;
                }
                Some(hold) => {
                    if self.held.get(&node) != Some(&hold) {
                        self.held.insert(node, hold);
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    /// Start a re-read of every held node's edges, where one is due and none is under way.
    ///
    /// `nodes` is the run's graph: a held node's prerequisites are resolved against every
    /// node of it.
    pub(crate) fn read_again(&mut self, nodes: impl FnOnce() -> Vec<Node>) {
        if self.held.is_empty()
            || self.reading.is_some()
            || self
                .read_at
                .is_some_and(|since| since.elapsed() < self.every)
        {
            return;
        }
        let Some((store, project)) = self.store.clone() else {
            return;
        };
        let asked: BTreeSet<String> = self.held.keys().cloned().collect();
        let nodes = nodes();
        let (answer, reading) = mpsc::channel();
        self.read_at = Some(Instant::now());
        self.reading = Some(reading);
        std::thread::spawn(move || {
            // A loop that has gone has nobody to answer, which is what a failed send says.
            let _ = answer.send(store.reread(&project, &nodes, &asked));
        });
    }

    pub(crate) fn held(&self) -> &BTreeMap<String, SourceHold> {
        &self.held
    }

    /// How long the loop may wait before it next has a re-read to start or take up.
    ///
    /// `Duration::MAX` where nothing is held, so a run whose plan was read whole waits on
    /// the channel alone — and where there is no store to read, which only a launch record
    /// naming no qualified project leaves.
    pub(crate) fn next_read(&self) -> Duration {
        if self.held.is_empty() || self.store.is_none() {
            return Duration::MAX;
        }
        if self.reading.is_some() {
            return TICK;
        }
        self.read_at.map_or(Duration::ZERO, |since| {
            self.every.saturating_sub(since.elapsed())
        })
    }

    /// Surface every held node's wait that is due, and forget the nodes no longer held so a
    /// wait that ends is not repeated after it.
    ///
    /// Non-blocking, on the cadence a release wait is surfaced at: queued when the hold
    /// begins and when what it says changes, and an unchanged one again only on
    /// [`crate::release::Resurfacing`]'s doubling interval. A report rather than a decision:
    /// what clears the hold is the source answering, which nobody answers a surface to bring
    /// about.
    pub(crate) fn surface_waits(&mut self, paths: &RunPaths, journal: &mut Journal) -> Result<()> {
        for (node, hold) in &self.held {
            let content = hold.describe();
            if !self.surfaced.due(paths, node, &content) {
                continue;
            }
            let queued = crate::engine::raised(paths, journal, wait_surface(node, hold))?;
            self.surfaced.queued(node, content, &queued);
        }
        let held = &self.held;
        self.surfaced.retain(|node| held.contains_key(node));
        Ok(())
    }
}

/// The surface a held node's wait raises.
fn wait_surface(node: &str, hold: &SourceHold) -> Surface {
    let (risk, remedy) = if hold.unanswered.is_empty() {
        (
            "so starting it now would start it before that prerequisite",
            format!(
                "Give it those dependencies with an edit, or drop it, and it dispatches the pass \
                 a read finds nothing missing; the edges are read again every {}s.",
                poll_seconds()
            ),
        )
    } else {
        (
            "so it may have a prerequisite this run cannot see",
            format!(
                "Supply the credential {} needs in the secrets file the store reads, or \
                 correct its configuration — both are read again for every read, while a \
                 variable exported now never reaches a driver already running — and it \
                 dispatches the pass a read of its edges asks every source and finds nothing \
                 missing; they are read again every {}s.",
                hold.sources(),
                poll_seconds()
            ),
        )
    };
    Surface {
        id: 0,
        kind: WAIT_SURFACE_KIND.to_owned(),
        message: format!(
            "node '{node}' is held: {what}, {risk}. {remedy} Nothing times this out and \
             nothing will fail the node; or stop the run.",
            what = hold.describe(),
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

/// Ids as a person reads a list of them.
fn quoted(ids: &[String]) -> String {
    ids.iter()
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// How often a held node's edges are read again.
// llmlint: ignore[changed_behavior_has_e2e] the journeys set the variable to a usable value to drive the re-read (`tests/e2e/store.rs`'s private-source journeys read again each second); an unset or unusable one selects the shipped minute, which no journey can wait out, so its parsing is held by `unanswered::tests::the_pace_is_a_positive_whole_number_of_seconds_or_the_default`, as `pool.rs` holds its own.
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

    fn vault(reason: &str) -> Unanswered {
        Unanswered {
            source: SourceName::new("vault").expect("a source name"),
            reason: reason.to_owned(),
        }
    }

    /// The hold's entry reads back whole, with `missing` left out where the read found none.
    #[test]
    fn the_hold_entry_round_trips_and_omits_an_empty_missing() {
        let unasked = SourceHold::new(vec![vault("auth: no token")], Vec::new()).expect("a hold");
        let entry = unasked.payload();
        assert_eq!(
            entry,
            json!({"kind": "source", "unanswered": [{"source": "vault", "reason": "auth: no token"}]})
        );
        assert_eq!(SourceHold::of_payload(&entry), Some(unasked));

        let found = SourceHold::new(Vec::new(), vec!["design".to_owned()]).expect("a hold");
        assert_eq!(SourceHold::of_payload(&found.payload()), Some(found));
        // Another hold, and one this build cannot read whole, are not this one.
        assert_eq!(
            SourceHold::of_payload(&json!({"kind": "release", "awaiting": []})),
            None
        );
        assert_eq!(
            SourceHold::of_payload(&json!({"kind": "source", "unanswered": [{"source": 7}]})),
            None
        );
        // Nor is one naming nothing that holds the node.
        assert_eq!(
            SourceHold::of_payload(&json!({"kind": "source", "unanswered": []})),
            None
        );
    }

    /// The hold's entries, the surface kind and the pacing variable are what divergence
    /// entry 131 records, and each entry reads back whole.
    #[test]
    fn the_hold_is_what_the_divergence_record_fixes_and_round_trips() {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
        )
        .expect("the divergence record reads");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("131."))
            .expect("the divergence record carries entry 131");
        let block: Value = serde_json::from_str(
            entry
                .split("```json")
                .nth(1)
                .and_then(|rest| rest.split("```").next())
                .expect("entry 131 carries the json block this test drives"),
        )
        .expect("entry 131's block is JSON");
        for key in ["hold", "hold_with_missing"] {
            let written = &block[key];
            let hold = SourceHold::of_payload(written)
                .unwrap_or_else(|| panic!("entry 131's {key} reads"));
            assert_eq!(&hold.payload(), written, "{key} does not round-trip");
        }
        assert_eq!(block["surface_kind"], WAIT_SURFACE_KIND);
        assert_eq!(block["poll_env"], POLL_ENV);
        assert_eq!(block["default_poll_seconds"], DEFAULT_POLL_SECONDS);
        assert_eq!(block["surface_cadence_env"], crate::release::SURFACE_ENV);
        let surface = wait_surface(
            "build",
            &SourceHold::of_payload(&block["hold"]).expect("reads"),
        );
        assert_eq!(surface.kind, WAIT_SURFACE_KIND);
        assert!(!surface.blocking);
        assert_eq!(surface.source, crate::channel::source::PROPOSAL);
        assert_eq!(surface.workstream.as_deref(), Some("build"));
    }

    /// A re-read clears the hold only where it asked every source and found no prerequisite
    /// the graph does not give the node; otherwise it holds, saying which.
    #[test]
    fn a_reread_clears_the_hold_only_when_every_source_answered_and_nothing_is_missing() {
        let deps = vec!["design".to_owned()];
        let answered = Reread {
            deps: vec!["design".to_owned()],
            unanswered: Vec::new(),
        };
        assert_eq!(hold_after(&deps, &answered), None);

        let still = Reread {
            deps: vec!["design".to_owned()],
            unanswered: vec![vault("unavailable: down")],
        };
        assert_eq!(
            hold_after(&deps, &still),
            SourceHold::new(vec![vault("unavailable: down")], Vec::new())
        );

        let found = Reread {
            deps: vec![
                "review".to_owned(),
                "design".to_owned(),
                "review".to_owned(),
            ],
            unanswered: Vec::new(),
        };
        let held = hold_after(&deps, &found).expect("a prerequisite the graph lacks holds");
        assert_eq!(held.missing, ["review"]);
        assert!(held.describe().contains("'review'"), "{}", held.describe());
    }

    /// The pace is the variable where it is a positive whole number of seconds, and the
    /// default for anything else, unset included.
    #[test]
    fn the_pace_is_a_positive_whole_number_of_seconds_or_the_default() {
        let _held = crate::vcs::scratch_home_held();
        for unusable in ["", "0", "-5", "soon", "1.5"] {
            std::env::set_var(POLL_ENV, unusable);
            assert_eq!(
                poll_seconds(),
                DEFAULT_POLL_SECONDS,
                "{POLL_ENV}={unusable:?}"
            );
        }
        std::env::set_var(POLL_ENV, "7200");
        assert_eq!(poll_seconds(), 7200);
        std::env::remove_var(POLL_ENV);
        assert_eq!(poll_seconds(), DEFAULT_POLL_SECONDS);
    }
}
