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
//! claim replaced stops at its next check — and `docs/contract-divergences.md` entry 116
//! says exactly what it does not.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::payload::DriverClaim;
use crate::taskgraph::{QualifiedId, Read, Store, DISPATCH_KEY, DRIVER_KEY, SETTLEMENT_KEY};

/// The longest an idle driver goes between two reads of its project's claim.
///
/// A driver that dispatches or publishes reads the claim at the start of each pass that
/// does, so this bounds only the driver with nothing to do: how long after a takeover it
/// can go on holding dispatches it will not publish.
pub(crate) const IDLE_CLAIM_CHECK: Duration = Duration::from_secs(600);

/// The **test seam** shortening [`IDLE_CLAIM_CHECK`], in whole seconds.
///
/// Read only so a journey can show an idle driver noticing a takeover without waiting
/// ten minutes; entry 116 names it as a seam, and nothing an operator configures sets it.
pub(crate) const IDLE_CLAIM_CHECK_ENV: &str = "ONEPIPELINE_IDLE_CLAIM_CHECK_SECONDS";

/// The longest one read of the claim may take before the driver goes on without its answer:
/// a store that does not answer has not said anybody else drives, and a reconcile pass that
/// waited on it for longer would hold every dispatch behind a board.
pub(crate) const CLAIM_READ_DEADLINE: Duration = Duration::from_secs(20);

/// The keys a claim carries, and the only ones: the store's copy is external input.
const CLAIM_KEYS: [&str; 5] = ["run", "host", "session", "pid", "claimed_at"];

/// The settlement statuses that leave a node's branch to be continued rather than its
/// work to be kept: the node ran, did not finish, and left its work on a branch.
const PINNING: [&str; 3] = ["failed", "cancelled", "parked"];

/// The project's claim, out of its metadata: `None` where the key is absent or `null` —
/// the two read the same, because a release writes `null` — and a refusal naming the key
/// where it holds anything that is not a claim.
pub(crate) fn claim_of(
    project: &str,
    metadata: &BTreeMap<String, Value>,
) -> Result<Option<DriverClaim>> {
    let refused =
        |why: String| Error::Invalid(format!("{project}: `{DRIVER_KEY}` is not a claim: {why}"));
    match metadata.get(DRIVER_KEY) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(claim)) => {
            if let Some(key) = claim.keys().find(|key| !CLAIM_KEYS.contains(&key.as_str())) {
                return Err(refused(format!("it carries `{key}`, which no claim does")));
            }
            serde_json::from_value(Value::Object(claim.clone()))
                .map(Some)
                .map_err(|why| refused(why.to_string()))
        }
        Some(other) => Err(refused(format!("it holds {other}, not a mapping"))),
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
        .filter_map(|node| {
            let settlement = stored_settlement(read, &node.id)?;
            let url = settlement.get("change_url")?.as_str()?;
            (settlement.get("landing").and_then(Value::as_str) != Some("landed"))
                .then(|| format!("'{}' ({url})", node.id))
        })
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
    /// Each node continued `done`, in plan order, with the settlement it journals: the
    /// stored one whole, marked `continued`.
    pub(crate) settled: Vec<(String, Map<String, Value>)>,
    /// Each node pinned to the branch it left, in plan order.
    pub(crate) pinned: Vec<(String, String)>,
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
        let settlement = stored_settlement(read, &node.id);
        if let Some(settlement) = settlement {
            // The settlement is external input, so it is read through the document a
            // `node-settled` is held to before anything of it is journalled.
            serde_json::from_value::<crate::payload::NodeSettled>(Value::Object(
                settlement.clone(),
            ))
            .map_err(|why| {
                Error::Invalid(format!(
                    "node '{}': `{SETTLEMENT_KEY}` is not a settlement: {why}",
                    node.id
                ))
            })?;
        }
        let status = settlement
            .and_then(|settlement| settlement.get("status"))
            .and_then(Value::as_str);
        if status == Some("done") {
            let mut payload = settlement.cloned().unwrap_or_default();
            payload.insert("continued".to_owned(), from.clone());
            continued.settled.push((node.id.clone(), payload));
            continue;
        }
        // A pin is a lifecycle node's alone — only a session cuts a branch — and one the
        // plan already states is the plan's.
        if node.repo.is_none() || node.branch.is_some() {
            continue;
        }
        let dispatched = read
            .metadata
            .get(&node.id)
            .and_then(|metadata| metadata.get(DISPATCH_KEY))
            .and_then(|dispatch| dispatch.get("branch"))
            .and_then(Value::as_str);
        let left = settlement
            .filter(|_| status.is_some_and(|status| PINNING.contains(&status)))
            .and_then(|settlement| settlement.get("branch"))
            .and_then(Value::as_str);
        if let Some(branch) = dispatched.or(left).and_then(crate::vcs::usable) {
            continued.pinned.push((node.id.clone(), branch));
        }
    }
    Ok(continued)
}

/// The settlement one node's task carries, where it carries one as a mapping.
fn stored_settlement<'a>(read: &'a Read, node: &str) -> Option<&'a Map<String, Value>> {
    read.metadata.get(node)?.get(SETTLEMENT_KEY)?.as_object()
}

/// What a driver knows about its project's claim, and when it last asked.
///
/// One store read per check, and a check only where the engine is about to do something a
/// takeover should stop — at launch and adoption, at the start of a pass that would dispatch
/// or publish, and on its idle interval — never before each dispatch or publication.
pub(crate) struct ClaimWatch {
    store: Store,
    project: QualifiedId,
    run: String,
    host: String,
    /// The run's own directory, where the store's request to be left alone is kept.
    run_dir: std::path::PathBuf,
    /// Whether this driver is a launch's, whose own read of the claim — the one that let it
    /// launch — covers its first pass; an adoption's first pass reads it again.
    launched: bool,
    /// The claim this run's launch knowingly took over: still on the store until this
    /// driver's first projection replaces it, and no sign that anybody else drives.
    displaced: Option<DriverClaim>,
    last: Option<Instant>,
    every: Duration,
}

impl ClaimWatch {
    /// The watch over the project `launch` names, or `None` for a run naming none.
    pub(crate) fn of_run(
        paths: &crate::ledger::RunPaths,
        launch: &crate::ledger::LaunchRecord,
    ) -> Option<Self> {
        let project = launch.project.parse().ok()?;
        Some(Self {
            store: Store::at(&launch.dir),
            project,
            run: paths.run.clone(),
            host: crate::sys::hostname(),
            run_dir: paths.dir.clone(),
            launched: launch.adoptions == 0,
            displaced: taken_over(paths),
            last: None,
            every: idle_claim_check(),
        })
    }

    /// The claim this driver writes: this run, this host, the session that launched it,
    /// this process, from `claimed_at`.
    pub(crate) fn mine(&self, session: &str, claimed_at: &str) -> DriverClaim {
        DriverClaim {
            run: self.run.clone(),
            host: self.host.clone(),
            session: Some(session.to_owned()).filter(|session| !session.is_empty()),
            pid: crate::sys::pid(),
            claimed_at: claimed_at.to_owned(),
        }
    }

    /// Read the claim, and answer the claim that supersedes this driver, if one does.
    ///
    /// A read the store fails is said on standard error and answers nothing: a claim is
    /// best effort, and a store that cannot be read has not said anybody else drives.
    pub(crate) fn check(&mut self) -> Option<DriverClaim> {
        self.last = Some(Instant::now());
        // A store that asked to be left alone is asked nothing inside the wait: every further
        // call extends a limiter nothing reports. The write-back is waiting it out too, and
        // the next check after it has passed reads the claim.
        if crate::writeback::store_asked_to_wait(&self.run_dir) {
            return None;
        }
        let read = self
            .store
            .project_metadata_within(&self.project, CLAIM_READ_DEADLINE)
            .and_then(|metadata| claim_of(self.project.as_str(), &metadata));
        match read {
            Ok(claim) => claim.filter(|claim| self.supersedes(claim)),
            Err(why) => {
                eprintln!(
                    "onepipeline: run '{}' could not read the claim on {}: {why}; it goes on \
                     driving and reads it again at its next check",
                    self.run,
                    self.project.as_str()
                );
                None
            }
        }
    }

    /// Whether `claim` names a run other than this one and other than the one this launch
    /// took the project over from.
    fn supersedes(&self, claim: &DriverClaim) -> bool {
        !claim.names(&self.run, &self.host) && self.displaced.as_ref() != Some(claim)
    }

    /// How long until the idle check is due: now where this driver has never checked.
    pub(crate) fn until_idle_check(&self) -> Duration {
        self.last.map_or(Duration::ZERO, |last| {
            self.every.saturating_sub(last.elapsed())
        })
    }

    /// Whether the idle check is due.
    pub(crate) fn idle_check_due(&self) -> bool {
        self.until_idle_check() == Duration::ZERO
    }

    /// Whether the first pass is covered by the read that let this run launch: a launch's
    /// driver is, an adoption's is not. Marks the idle clock from now, so a covered first pass
    /// is not read again as an idle one.
    pub(crate) fn covered_by_the_launch(&mut self) -> bool {
        if self.launched {
            self.last = Some(Instant::now());
        }
        self.launched
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

/// The idle check's interval: [`IDLE_CLAIM_CHECK`], or the test seam's whole seconds.
fn idle_claim_check() -> Duration {
    std::env::var(IDLE_CLAIM_CHECK_ENV)
        .ok()
        .and_then(|seconds| seconds.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map_or(IDLE_CLAIM_CHECK, Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(run: &str, host: &str) -> Value {
        json!({"run": run, "host": host, "session": null, "pid": 7,
               "claimed_at": "2026-10-11T00:00:00Z"})
    }

    #[test]
    fn an_idle_driver_reads_its_claim_at_least_every_ten_minutes() {
        assert!(IDLE_CLAIM_CHECK <= Duration::from_secs(600));
        assert_eq!(IDLE_CLAIM_CHECK, Duration::from_secs(600));
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
        for (bad, says) in [
            (extra, "`lease`"),
            (json!("ship"), "not a mapping"),
            (json!({"run": "ship"}), "missing field"),
        ] {
            let refused = claim_of("p", &BTreeMap::from([(DRIVER_KEY.to_owned(), bad)]))
                .expect_err("refused")
                .to_string();
            assert!(refused.contains(says), "{refused}");
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
}
