//! `onepipeline shutdown`: ending the running work on a host the way a person
//! would want it ended.
//!
//! Nothing here is a second liveness test, a second interrupt path, or a second
//! process-walking strategy. It is the two mechanisms this crate already has,
//! applied across runs with a longer default grace:
//!
//! 1. the cooperative cancel's **interrupt-then-deadline** — `CANCEL_INPUT`
//!    through `agentgraph::interrupt`, per turn address, and the three answers
//!    none of which is a failure;
//! 2. `stop`'s **resolve-pids-and-walk-the-tree** teardown — `driver::terminate`
//!    over the launch record, the ownership lock and the dispatch registry,
//!    answering `stop`'s own [`StopTeardown`] vocabulary unchanged;
//!
//! and then `onevcs::preserve` for every branch the run's records name, so the
//! commits a worker made survive the machine going away.
//!
//! The one thing that is genuinely new is that this runs **outside the driver
//! process**. The driver learns a dispatch's turn addresses from the live
//! stream; a shutdown has no stream, so it reads them back out of what the run
//! recorded — the graph run and member `oneagentgraph` stamped on that
//! dispatch's own relayed envelopes, which is the only way this crate knows
//! either.
//!
//! **A shutdown is not a stop.** It journals no `run-stopped` and fires no
//! run-end hook: the run has not ended, it has been put down mid-flight to be
//! picked up again, and a failure hook here would launch follow-up work on a
//! machine that is going away. Nothing is parked, nothing is settled, nothing is
//! written back to the plan store and no claim is released — a node whose
//! dispatch was in flight is left in flight, which is exactly the state `adopt`
//! already reconciles.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::agentgraph::{self, Interrupted, TurnAddress};
use crate::driver;
use crate::engine::CANCEL_INPUT;
use crate::error::{Error, Result, EXIT_REFUSED, EXIT_SUCCESS};
use crate::event::{Envelope, Phase, Source};
use crate::journal::{self, Journal, StopTeardown};
use crate::ledger::{self, DispatchRecord, RunPaths};
use crate::sys;
use crate::views::{RunView, Survey};

/// The file a run's directory carries for as long as a shutdown of it stands.
///
/// It is what makes *nothing new starts* true: the reconcile loop reads it
/// before it dispatches, and a run carrying one dispatches no further node. The
/// driver is the run's scheduler and is also a process this is about to end, so
/// the property cannot rest on the driver having noticed anything — it rests on
/// a file the driver reads and the shutdown wrote before it signalled anything.
///
/// Removed by an adoption, which is the deliberate decision to run the run
/// again. Nothing else removes it, so a run whose teardown left a driver
/// standing goes on dispatching nothing.
const MARKER: &str = "shutting-down.json";

/// How often the wait looks at the dispatches it is watching.
///
/// A granularity rather than a rate: the grace is minutes and this only decides
/// how promptly a dispatch that ended early is *noticed*, so it is short enough
/// that a journey with a one-second grace still tells the two endings apart.
const POLL: Duration = Duration::from_millis(50);

/// The word a `dispatch-stopped` carries when the running turn took the
/// redirection.
const DELIVERED: &str = "delivered";
/// The word for a dispatch that had no controllable turn to redirect. A fact
/// about the lever rather than a failure.
const NO_TURN: &str = "no-turn";
/// The word for a lever that was pulled and broke. Also not a failure of the
/// shutdown: the deadline applies either way.
const FAILED: &str = "failed";
/// The word for the `--force` path, where nothing was asked at all.
const NOT_ASKED: &str = "not-asked";

/// Which runs a shutdown acts on.
///
/// Exactly one, and it is required: naming none, or naming two, is refused
/// before anything is signalled. The three behave **identically** — interrupt,
/// wait, forceful stop of survivors, then the preserving push — and differ only
/// in which runs they enumerate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownScope {
    /// One run, by id.
    Run(String),
    /// Every run this session owns, as `runs --mine` selects them.
    Mine,
    /// Every run under the runs root this invocation reads, whoever owns it.
    Host,
}

impl ShutdownScope {
    /// The word this scope travels as on a `host-shutdown` record.
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Run(_) => "run",
            Self::Mine => "mine",
            Self::Host => "host",
        }
    }

    /// Whether this scope keeps `stop`'s ownership rule.
    ///
    /// `--host` deliberately does not: shutting a host down is a decision about
    /// the host rather than about one run, and what stands in for the refusal is
    /// the report, which names every run's owner.
    const fn keeps_the_ownership_rule(&self) -> bool {
        !matches!(self, Self::Host)
    }
}

/// What a caller is asking a shutdown for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownRequest {
    /// Which runs.
    pub scope: ShutdownScope,
    /// The session acting. Passed in, never read from the environment here, for
    /// the reason [`crate::verbs::StopRequest`] gives.
    pub session: String,
    /// How long a dispatch has to end itself after it is asked.
    pub grace: Duration,
    /// Skip the interrupt and the wait and go straight to the teardown. A grace
    /// of zero takes the same path.
    pub force: bool,
}

impl ShutdownRequest {
    /// Whether anything is asked of a dispatch before it is torn down.
    fn asks(&self) -> bool {
        !self.force && !self.grace.is_zero()
    }
}

/// How one dispatch a shutdown acted on ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchEnding {
    /// It ended within the grace, on its own terms.
    Graceful,
    /// It was still there at the deadline and the teardown reaped it.
    Killed,
    /// The teardown did not end it and it is still running.
    StillRunning,
}

impl DispatchEnding {
    /// The word this ending travels as on a `dispatch-stopped` record.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Graceful => "graceful",
            Self::Killed => "killed",
            Self::StillRunning => "still-running",
        }
    }
}

/// What preserving one branch found to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preserved {
    /// It was pushed; the origin now carries it under its own name.
    Pushed,
    /// The origin already had it at this commit. Nothing was pushed.
    AlreadyOnOrigin,
    /// The identity has no origin to push to. Nothing was attempted.
    NoRemote,
    /// The push was refused, and [`BranchPreserved::detail`] says why.
    Refused,
}

impl Preserved {
    /// The word this outcome travels as on a `host-shutdown` record.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pushed => "pushed",
            Self::AlreadyOnOrigin => "already-on-origin",
            Self::NoRemote => "no-remote",
            Self::Refused => "refused",
        }
    }
}

/// One live dispatch a shutdown acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchStopped {
    /// The node it was running.
    pub node: String,
    /// The process the work was in.
    pub pid: u32,
    /// What the interrupt was answered with: `delivered`, `no-turn`, `failed`,
    /// or `not-asked` for the forced path.
    pub interrupt: String,
    /// That answer in the words a reader is shown, and — for a dispatch the
    /// deadline reaped inside a publication of its own — what that leaves behind
    /// and the verb that resumes it.
    pub detail: String,
    /// How it ended.
    pub ended: DispatchEnding,
    /// How long it was watched for, from the moment it was asked.
    pub waited: Duration,
}

/// One branch the shutdown offered to `onevcs::preserve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchPreserved {
    /// The repository identity, as the sibling resolved it — or as the run's own
    /// record named it, for a request that was refused before it resolved.
    pub identity: String,
    /// The branch, under the name it already had.
    pub branch: String,
    /// What preserving it found to do.
    pub outcome: Preserved,
    /// The origin it went to, where there was one.
    pub remote: Option<String>,
    /// The commit it stands at, where the preservation read one.
    pub commit: Option<String>,
    /// What the sibling said.
    pub detail: String,
}

/// What a shutdown did to one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunShutdown {
    /// The run.
    pub run: String,
    /// Who owns it, as the shutdown journalled it.
    pub owner: String,
    /// Whether this acted on a run another session owns, which only `--host`
    /// does.
    pub forced_over_owner: bool,
    /// One entry per live dispatch the shutdown acted on.
    pub dispatches: Vec<DispatchStopped>,
    /// What the teardown established, in `stop`'s own vocabulary.
    pub teardown: StopTeardown,
    /// One entry per branch the run's own records named.
    pub branches: Vec<BranchPreserved>,
}

impl RunShutdown {
    /// Whether this run's shutdown did everything it was asked to.
    ///
    /// A dispatch killed **at the deadline** is a worker that was given the
    /// chance to finish its thought and did not take it, which is the verb not
    /// having done what it was asked. One the forced path tore down was never
    /// given that chance — the person asked for exactly the teardown it got — so
    /// it is not held against the shutdown; its teardown still is.
    fn clean(&self) -> bool {
        matches!(
            self.teardown,
            StopTeardown::Signalled | StopTeardown::NothingToStop | StopTeardown::Elsewhere
        ) && self.dispatches.iter().all(|stopped| match stopped.ended {
            DispatchEnding::Graceful => true,
            DispatchEnding::Killed => stopped.interrupt == NOT_ASKED,
            DispatchEnding::StillRunning => false,
        }) && self
            .branches
            .iter()
            .all(|branch| branch.outcome != Preserved::Refused)
    }
}

/// What `onepipeline shutdown` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shutdown {
    /// The runs root it read. Named on the report, because a run under another
    /// runs root is one this will never list.
    pub root: PathBuf,
    /// The scope that selected the runs.
    pub scope: ShutdownScope,
    /// The grace each dispatch had.
    pub grace: Duration,
    /// Whether the interrupt and the wait were skipped.
    pub forced: bool,
    /// One entry per run acted on.
    pub runs: Vec<RunShutdown>,
    /// Every unpublished branch on this host the shutdown did **not** push, read
    /// from `onevcs::recoverable(&Scope::All)`.
    pub not_pushed: Vec<(String, String)>,
    /// Named when that read could not be made, so an empty list is never read as
    /// "there are none".
    pub not_pushed_unread: Option<String>,
}

impl Shutdown {
    /// The status the binary exits with.
    ///
    /// [`EXIT_REFUSED`] when anything was killed at the deadline, when a
    /// teardown was not clean, or when a push was refused; [`EXIT_SUCCESS`]
    /// otherwise. A branch already level with its origin, a branch whose
    /// identity has no origin at all, and a run with nothing live to interrupt
    /// are facts this reports at exit 0.
    pub fn exit_code(&self) -> i32 {
        if self.runs.iter().all(RunShutdown::clean) {
            EXIT_SUCCESS
        } else {
            EXIT_REFUSED
        }
    }
}

/// Whether a shutdown of this run stands, so nothing new may start for it.
///
/// Asked by the reconcile loop before it dispatches, and by the liveness reading
/// — a run put down by a shutdown is not one a driver is moving, however alive
/// the process holding it still is.
pub(crate) fn begun(paths: &RunPaths) -> bool {
    marker(paths).exists()
}

/// Forget a shutdown, because the run is being taken up again.
///
/// Called by the adoption that claims the run, under its ownership lock: an
/// `adopt` is the deliberate decision to run this run again, and it is the only
/// thing that lifts the hold.
pub(crate) fn adopted(paths: &RunPaths) {
    let _ = std::fs::remove_file(marker(paths));
}

fn marker(paths: &RunPaths) -> PathBuf {
    paths.dir.join(MARKER)
}

/// `onepipeline shutdown`: end the running work under `root`, gently.
///
/// Per run, in order: nothing new starts, every live dispatch is asked to stop,
/// the wait, the teardown of whatever is still standing and then the run's own
/// driver, and the preserving push of every branch the run's records name. The
/// pushes are attempted whether or not the teardown ended everything — a branch
/// is not less worth keeping because a process would not go.
///
/// # Errors
///
/// [`Error::NotOwned`] for a run another session owns under the two scopes that
/// keep `stop`'s ownership rule, refused before anything is signalled;
/// [`Error::NoSuchRun`] for a run id this root does not hold; and a run whose
/// dispatch registry cannot be read, which is a run nobody can say is idle.
pub fn shutdown(root: &Path, request: ShutdownRequest) -> Result<Shutdown> {
    let selected = select(root, &request)?;
    let mut runs = Vec::new();
    let mut pushed: BTreeSet<(String, String)> = BTreeSet::new();
    for view in &selected {
        let one = shut_one_down(root, view, &request)?;
        for branch in &one.branches {
            if branch.outcome == Preserved::Pushed {
                pushed.insert((branch.identity.clone(), branch.branch.clone()));
            }
        }
        runs.push(one);
    }
    let (not_pushed, not_pushed_unread) = elsewhere_on_this_host(&pushed);
    Ok(Shutdown {
        root: root.to_path_buf(),
        scope: request.scope,
        grace: request.grace,
        forced: request.force,
        runs,
        not_pushed,
        not_pushed_unread,
    })
}

/// Which runs this scope names, with the ownership rule already applied.
///
/// A refusal here has signalled nothing: the whole selection is made before the
/// first run's shutdown begins, so a `RUN` or a `--mine` that meets a run it does
/// not own leaves every run on the host exactly as it was.
fn select(root: &Path, request: &ShutdownRequest) -> Result<Vec<RunView>> {
    match &request.scope {
        ShutdownScope::Run(run) => {
            let view = RunView::open(&crate::verbs::resolved(root, run)?)?;
            if !view.launch.owned_by(&request.session) {
                return Err(Error::NotOwned {
                    run: view.paths.run.clone(),
                    owner: view.launch.owner_label(&request.session),
                });
            }
            Ok(vec![view])
        }
        // `Survey::of` is the enumeration `host` already reads every run on this
        // host through, refused roots and all — so a shutdown lists exactly what
        // that view lists rather than a second reading of the same root.
        ShutdownScope::Mine => Ok(Survey::of(root)
            .views
            .into_iter()
            .filter(|view| {
                let mine = view.launch.owned_by(&request.session);
                if !mine {
                    // Named rather than silently skipped: `--mine` acting on a
                    // shared host has to say which runs it left running and
                    // whose they are, or an operator reads a partial shutdown as
                    // a whole one.
                    eprintln!(
                        "onepipeline: run '{}' belongs to {}; `--mine` shuts down only this \
                         session's runs, so it was left running and nothing was signalled for it",
                        view.paths.run,
                        view.launch.owner_label(&request.session)
                    );
                }
                mine
            })
            .collect()),
        ShutdownScope::Host => Ok(Survey::of(root).views),
    }
}

/// The whole verb, for one run.
fn shut_one_down(root: &Path, view: &RunView, request: &ShutdownRequest) -> Result<RunShutdown> {
    let paths = &view.paths;
    let owner = view.launch.owner_label(&request.session);
    let forced_over_owner =
        !request.scope.keeps_the_ownership_rule() && !view.launch.owned_by(&request.session);
    if forced_over_owner {
        eprintln!(
            "onepipeline: run '{}' belongs to {owner}; shutting this host down includes it",
            paths.run
        );
    }

    // 1. Nothing new starts — written before anything is signalled, because the
    //    driver goes on scheduling for as long as the grace lasts.
    hold_the_run(paths)?;

    // The work itself, off the registry with its start token, exactly as
    // `roots_to_stop` reads it and never off `ps`. A registry that cannot be
    // read is a run nobody can say is idle, so it refuses here as `stop` refuses
    // — before the interrupt, so nothing has been asked of anybody.
    let watched = in_flight(view, live_dispatches(paths)?);
    // Where the record stood when the shutdown began, so the wait reads only
    // what the run has written since rather than the whole of its history.
    let from = std::fs::metadata(paths.journal()).map_or(0, |held| held.len());
    let mut journal = Journal::open(paths);

    // 2. Every live dispatch is asked to stop, through the lever a `cancel`
    //    already pulls. One inside a publication has no turn to ask, and is
    //    bounded by the grace like any other.
    let addresses = addresses_by_node(&view.events);
    let mut asked: Vec<(Watched, String, String)> = Vec::new();
    for dispatch in watched {
        let (word, detail) = if !request.asks() {
            (
                NOT_ASKED.to_string(),
                "nothing was asked of this dispatch: the shutdown was forced, so it went \
                 straight to the teardown and whatever its turn had not committed is gone"
                    .to_string(),
            )
        } else if dispatch.in_the_driver {
            (
                NO_TURN.to_string(),
                format!(
                    "it was inside a publication of its own, which has no turn to interrupt, \
                     so there was nothing to ask; it is killed in {}s if the publication has \
                     not settled by then",
                    request.grace.as_secs()
                ),
            )
        } else {
            ask_it_to_stop(
                &mut journal,
                &dispatch.record.node,
                addresses
                    .get(&dispatch.record.node)
                    .map_or(&[][..], Vec::as_slice),
                request.grace,
            )
        };
        asked.push((dispatch, word, detail));
    }

    // 3. The wait, watching the processes the registry names — and, for a node
    //    whose work is the driver's publication, the run's own record of it.
    let asked_at = Instant::now();
    let waited = wait_for_them(paths, from, &asked, asked_at, request);

    // 4. The teardown: whatever is still standing, and then the run's driver,
    //    ended the way `stop` ends them.
    let teardown = tear_the_run_down(paths, view);

    // Read again, because what a publication had reached is what the report
    // names, and one may have begun after the shutdown did.
    let now = RunView::open(paths).ok();
    let dispatches: Vec<DispatchStopped> = asked
        .into_iter()
        .map(|(dispatch, interrupt, detail)| {
            let record = dispatch.record;
            let publishing = now
                .as_ref()
                .is_some_and(|now| publishing_phase(now, &record.node).is_some());
            let (how, waited) = match (
                waited.ended.get(&record.node),
                waited.exited.get(&record.node),
            ) {
                (Some(after), _) => (DispatchEnding::Graceful, *after),
                // Its process went of its own accord and nothing of it was left
                // in a publication: the driver had simply not recorded it yet.
                (None, Some(after)) if !publishing => (DispatchEnding::Graceful, *after),
                _ if sys::claim_on(record.pid, &record.started).is_over() => {
                    (DispatchEnding::Killed, asked_at.elapsed())
                }
                _ => (DispatchEnding::StillRunning, asked_at.elapsed()),
            };
            DispatchStopped {
                detail: with_the_publication_state(
                    now.as_ref().unwrap_or(view),
                    &record.node,
                    how,
                    detail,
                ),
                node: record.node,
                pid: record.pid,
                interrupt,
                ended: how,
                waited,
            }
        })
        .collect();
    for stopped in &dispatches {
        journal.emit(
            journal::PipelineKind::DispatchStopped,
            journal::labels(&paths.run, Some(&stopped.node)),
            journal::payload(&[
                ("pid", json!(stopped.pid)),
                ("interrupt", json!(stopped.interrupt)),
                ("detail", json!(stopped.detail)),
                ("ended", json!(stopped.ended.as_str())),
                (
                    "waited_ms",
                    json!(u64::try_from(stopped.waited.as_millis()).unwrap_or(u64::MAX)),
                ),
            ]),
        )?;
    }

    // 5. The preserving push, for every branch this run's own records name.
    //    Attempted whether or not the teardown ended everything: a branch is not
    //    less worth keeping because a process would not go.
    let branches = preserve_every_branch(view);

    let shutdown = RunShutdown {
        run: paths.run.clone(),
        owner,
        forced_over_owner,
        dispatches,
        teardown,
        branches,
    };
    // Written after the teardown and the pushes, so the record says what
    // happened rather than what was about to be tried. Deliberately **not** a
    // `run-stopped`, and no run-end hook fires: the run has not ended.
    journal.emit(
        journal::PipelineKind::HostShutdown,
        journal::labels(&paths.run, None),
        journal::payload(&[
            ("scope", json!(request.scope.as_str())),
            ("owner", json!(shutdown.owner)),
            ("forced", json!(request.force)),
            ("grace_seconds", json!(request.grace.as_secs())),
            (
                "dispatches",
                json!(u32::try_from(shutdown.dispatches.len()).unwrap_or(u32::MAX)),
            ),
            (
                "graceful",
                json!(counted(&shutdown, DispatchEnding::Graceful)),
            ),
            ("killed", json!(counted(&shutdown, DispatchEnding::Killed))),
            (journal::STOP_TEARDOWN, json!(shutdown.teardown)),
            ("root", json!(root.display().to_string())),
            (
                "branches",
                json!(shutdown
                    .branches
                    .iter()
                    .map(|branch| json!({
                        "identity": branch.identity,
                        "branch": branch.branch,
                        "result": branch.outcome.as_str(),
                        "remote": branch.remote,
                        "commit": branch.commit,
                        "detail": branch.detail,
                    }))
                    .collect::<Vec<_>>()),
            ),
        ]),
    )?;
    Ok(shutdown)
}

fn counted(shutdown: &RunShutdown, ending: DispatchEnding) -> u32 {
    u32::try_from(
        shutdown
            .dispatches
            .iter()
            .filter(|stopped| stopped.ended == ending)
            .count(),
    )
    .unwrap_or(u32::MAX)
}

/// Write the hold that stops this run dispatching anything else.
fn hold_the_run(paths: &RunPaths) -> Result<()> {
    let path = marker(paths);
    std::fs::write(
        &path,
        json!({"at": sys::now_rfc3339(), "pid": sys::pid()}).to_string(),
    )
    .map_err(|source| Error::Ledger { path, source })
}

/// Every dispatch of this run that is a live process on this host.
///
/// The registry, with its start token, exactly as `roots_to_stop` reads it: a
/// pid the host has since given to somebody else is not this run's work and is
/// never asked, watched or counted.
fn live_dispatches(paths: &RunPaths) -> Result<Vec<DispatchRecord>> {
    let here = sys::hostname();
    Ok(ledger::dispatches_of(paths)
        .map_err(|why| {
            Error::Refused(format!(
                "run '{}' was not shut down: this build cannot establish what it is running — \
                 {why}. The run is untouched; nothing was signalled",
                paths.run
            ))
        })?
        .into_iter()
        .filter(|record| record.host == here)
        .filter(|record| {
            matches!(
                sys::claim_on(record.pid, &record.started),
                sys::Claim::Proved
            )
        })
        .collect())
}

/// One dispatch the shutdown acts on, and where its work is.
struct Watched {
    /// The node, and the process the work is in with the stamp that proves it:
    /// the registry's own entry, or — for a node whose work is the driver's
    /// publication — the driver's claim, stated in the same shape.
    record: DispatchRecord,
    /// Whether the work is the **driver's**: a lifecycle node's publication,
    /// which runs in the driver after its agent has exited and so has no process
    /// of its own and no turn. It ends when its node settles, and it is ended
    /// with the driver when it has not.
    in_the_driver: bool,
}

/// Every piece of in-flight work this run has on this host.
///
/// The registry's live dispatches, and beside them every node still recorded
/// running whose work is a publication the driver is carrying — a dispatch that
/// is inside a publication has no turn to interrupt, and is bounded by the grace
/// exactly like any other. Only where the driver is proved to be the process it
/// was recorded as: a node whose driver is gone has nothing publishing it.
fn in_flight(view: &RunView, live: Vec<DispatchRecord>) -> Vec<Watched> {
    let mut watched: Vec<Watched> = live
        .into_iter()
        .map(|record| Watched {
            record,
            in_the_driver: false,
        })
        .collect();
    let Some(driver) = live_driver(view) else {
        return watched;
    };
    for (node, status) in view.state.statuses() {
        if status != crate::graph::NodeStatus::Running
            || watched.iter().any(|dispatch| dispatch.record.node == node)
            || publishing_phase(view, &node).is_none()
        {
            continue;
        }
        watched.push(Watched {
            record: DispatchRecord {
                node,
                ..driver.clone()
            },
            in_the_driver: true,
        });
    }
    watched
}

/// The run's driver, as a claim this host has proved, where it has one.
fn live_driver(view: &RunView) -> Option<DispatchRecord> {
    let launch = &view.launch;
    let pid = launch.driver_pid()?.get();
    let started = launch.driver_stamp()?;
    (launch.recorded_host() == Some(sys::hostname().as_str())
        && matches!(sys::claim_on(pid, started), sys::Claim::Proved))
    .then(|| DispatchRecord {
        node: String::new(),
        pid,
        host: sys::hostname(),
        dispatched_at: String::new(),
        started: started.to_string(),
    })
}

/// Ask one dispatch's turns to stop, and say what each answered.
///
/// The same lever, the same redirection and the same three answers the
/// cancellation arm uses — none of them a failure. A dispatch that has named no
/// turn is recorded as having none to ask, and the deadline applies to it
/// exactly as it does to one that was reached.
fn ask_it_to_stop(
    journal: &mut Journal,
    node: &str,
    addresses: &[TurnAddress],
    grace: Duration,
) -> (String, String) {
    if addresses.is_empty() {
        return (
            NO_TURN.to_string(),
            format!(
                "nothing of this dispatch has named a turn to interrupt, so there was nothing \
                 to ask; it is killed in {}s if it has not exited by then",
                grace.as_secs()
            ),
        );
    }
    let mut word = NO_TURN;
    let mut answers = Vec::new();
    for address in addresses {
        let interrupt = agentgraph::interrupt(address, CANCEL_INPUT);
        // Whatever it answered, the sibling published an envelope saying the
        // lever was pulled and what came of it. It belongs in the merged store
        // like any other, stamped with the node it is about — which its producer
        // could not know.
        for mut event in interrupt.events {
            if event.labels.node.is_none() {
                event.labels.node = Some(node.to_string());
            }
            let _ = journal.relay(&event);
        }
        // Delivered outranks a lever that broke, which outranks a turn that was
        // not there: one dispatch may have named several members, and what the
        // reader has to know is whether *anything* took the ask.
        let (rank, answer) = match &interrupt.outcome {
            Interrupted::Delivered => (DELIVERED, "the running turn took the redirection".into()),
            Interrupted::Failed(why) => (FAILED, format!("the lever failed ({why})")),
            Interrupted::NoTurn(why) => (NO_TURN, format!("no turn to redirect ({why})")),
        };
        if rank == DELIVERED || (rank == FAILED && word != DELIVERED) {
            word = rank;
        }
        answers.push(format!("{}: {answer}", address.member()));
    }
    (
        word.to_string(),
        format!(
            "asked the {} turn(s) this dispatch had named to stop, commit, and end — {}. It is \
             killed in {}s if it has not exited by then",
            addresses.len(),
            answers.join("; "),
            grace.as_secs()
        ),
    )
}

/// Watch the dispatches until they are gone or the grace runs out.
///
/// Answers, per node whose work ended, how long after the ask it was observed
/// to. A dispatch with a process of its own has ended when the registry's claim
/// on that process is over — and, where its driver is alive to carry it on,
/// when its node has also left flight: a lifecycle node whose agent exits goes
/// on to publish in the driver, and that publication is the same dispatch's
/// work, bounded by the same grace. One whose work was the driver's all along
/// has ended when its node settles.
///
/// Nothing is watched at all on the forced path: there was no ask, so there is
/// no deadline to wait out.
fn wait_for_them(
    paths: &RunPaths,
    from: u64,
    asked: &[(Watched, String, String)],
    asked_at: Instant,
    request: &ShutdownRequest,
) -> Waited {
    let mut waited = Waited::default();
    if !request.asks() {
        return waited;
    }
    let deadline = asked_at + request.grace;
    let driven = asked.iter().any(|(dispatch, ..)| dispatch.in_the_driver)
        || RunView::open(paths)
            .ok()
            .as_ref()
            .and_then(live_driver)
            .is_some();
    loop {
        let left_flight = if driven {
            left_flight_since(paths, from)
        } else {
            BTreeSet::new()
        };
        let mut standing = false;
        for (dispatch, ..) in asked {
            let record = &dispatch.record;
            if waited.ended.contains_key(&record.node) {
                continue;
            }
            let process_over =
                dispatch.in_the_driver || sys::claim_on(record.pid, &record.started).is_over();
            if process_over && !dispatch.in_the_driver {
                waited
                    .exited
                    .entry(record.node.clone())
                    .or_insert_with(|| asked_at.elapsed());
            }
            if process_over && (!driven || left_flight.contains(&record.node)) {
                waited.ended.insert(record.node.clone(), asked_at.elapsed());
            } else {
                standing = true;
            }
        }
        if !standing || Instant::now() >= deadline {
            return waited;
        }
        std::thread::sleep(POLL);
    }
}

/// What the wait saw, per node.
#[derive(Default)]
struct Waited {
    /// The node's work ended — its process and whatever the driver went on to
    /// do with it — and how long after the ask.
    ended: BTreeMap<String, Duration>,
    /// The dispatch's own process exited on its own, and how long after the
    /// ask, whether or not the driver had finished with the node by the
    /// deadline.
    exited: BTreeMap<String, Duration>,
}

/// Every node the run's own record has taken out of flight since `from`: settled,
/// or handed back to the queue.
///
/// Read off the journal's tail rather than off a fold of the whole run, because
/// it is asked on every pass of the wait.
fn left_flight_since(paths: &RunPaths, from: u64) -> BTreeSet<String> {
    ledger::read_envelope_lines(&paths.journal(), from)
        .into_iter()
        .filter_map(|line| line.envelope)
        .filter(|event| {
            matches!(
                journal::PipelineKind::from_wire(&event.kind),
                Some(journal::PipelineKind::NodeSettled | journal::PipelineKind::NodeRequeued)
            )
        })
        .filter_map(|event| event.labels.node)
        .collect()
}

/// End whatever is still standing, and then the run's driver.
///
/// `driver::terminate` is `stop`'s own teardown, unchanged: SIGTERM first,
/// escalation after, the whole descendant tree and never a process group,
/// survivors reported by pid. A teardown this build cannot even *read* the tree
/// for is [`StopTeardown::NotAttempted`] — the answer a `stop` refuses on — and
/// the shutdown carries on to the pushes rather than abandoning the branches,
/// because a branch is not less worth keeping because a listing failed.
fn tear_the_run_down(paths: &RunPaths, view: &RunView) -> StopTeardown {
    match driver::terminate(paths, &view.launch) {
        Ok(teardown) => driver::established(teardown),
        Err(why) => {
            eprintln!(
                "onepipeline: run '{}': this build cannot establish what it is running — {why}; \
                 nothing was signalled, and the branches below are preserved anyway",
                paths.run
            );
            StopTeardown::NotAttempted
        }
    }
}

/// The turn addresses each node's own relayed envelopes stamped.
///
/// The graph run and member `oneagentgraph` put on the dispatch's stream, which
/// is the only way this crate knows either — and, outside the driver process,
/// the only place to read them is what the run recorded. An address an earlier,
/// finished attempt of the same node left behind is kept: interrupting it
/// answers `no-turn`, which is a fact rather than a failure, and dropping it
/// would mean guessing which attempt a stored envelope belonged to.
fn addresses_by_node(events: &[Envelope]) -> BTreeMap<String, Vec<TurnAddress>> {
    let mut by_node: BTreeMap<String, Vec<TurnAddress>> = BTreeMap::new();
    for event in events {
        if event.source != Source::Agentgraph {
            continue;
        }
        let (Some(node), Some(run), Some(member)) = (
            event.labels.node.as_deref(),
            event.labels.run_id.as_deref(),
            event.labels.member.as_deref(),
        ) else {
            continue;
        };
        let Some(address) = TurnAddress::of(run, member) else {
            continue;
        };
        let named = by_node.entry(node.to_string()).or_default();
        if !named.contains(&address) {
            named.push(address);
        }
    }
    by_node
}

/// What a dispatch the deadline reaped inside a publication leaves behind.
///
/// Its own case on the report, because "it had no turn and it was killed" is
/// true of it and is not what a reader has to act on: the publication was in
/// flight, so the change request or the branch is in a state the engine already
/// knows how to resume, and the report says which and how. A dispatch that
/// ended on its own terms is left with the answer its interrupt gave.
fn with_the_publication_state(
    view: &RunView,
    node: &str,
    ending: DispatchEnding,
    detail: String,
) -> String {
    if ending == DispatchEnding::Graceful {
        return detail;
    }
    let Some(phase) = publishing_phase(view, node) else {
        return detail;
    };
    let state = match view.state.change_urls.get(node) {
        Some(url) => format!(
            "its change request is open at {url}, pushed but with no merge-path verdict on \
             this attempt"
        ),
        None => match view.state.sessions.get(node) {
            Some(session) => format!(
                "its work is on branch {}, and nothing has merged it — the publication was \
                 stopped before its merge path answered",
                session.branch()
            ),
            None => "nothing has merged what it was publishing — the publication was stopped \
                     before its merge path answered"
                .to_string(),
        },
    };
    format!(
        "{detail}. It was inside a publication of its own when the deadline reaped it (phase \
         {phase}): {state}. `onepipeline adopt {}` re-dispatches the node pinned to that \
         branch, which takes the publication up from where it stopped",
        view.paths.run
    )
}

/// The phase the last thing `onevcs` said about this node puts it in, where that
/// is past development — which is what *inside a publication* means.
fn publishing_phase(view: &RunView, node: &str) -> Option<Phase> {
    view.events
        .iter()
        .rev()
        .filter(|event| event.source == Source::Vcs)
        .find(|event| event.labels.node.as_deref() == Some(node))
        .and_then(|event| event.dimensions.phase)
        .filter(|phase| !matches!(phase, Phase::Development))
}

/// Put every branch this run's records name on its identity's origin.
///
/// The session each node worked in, in flight or closed, and every branch the
/// run preserved. A push that fails for one branch is reported and does not stop
/// the others, which is the whole reason this is a loop over a list rather than
/// a `?` over an iterator.
fn preserve_every_branch(view: &RunView) -> Vec<BranchPreserved> {
    let mut preserved = Vec::new();
    for (repo, branch) in branches_of(view) {
        let request = onevcs::PreserveRequest {
            repo: repo.clone(),
            branch: branch.clone(),
        };
        preserved.push(match onevcs::preserve(&request) {
            Ok(done) => BranchPreserved {
                identity: done.identity,
                branch: done.branch,
                outcome: match done.outcome {
                    onevcs::Preservation::Pushed => Preserved::Pushed,
                    onevcs::Preservation::AlreadyOnOrigin => Preserved::AlreadyOnOrigin,
                    onevcs::Preservation::NoRemote => Preserved::NoRemote,
                },
                remote: done.remote,
                commit: done.commit,
                detail: format!("preserved from {}", done.from.display()),
            },
            Err(why) => BranchPreserved {
                identity: repo,
                branch,
                outcome: Preserved::Refused,
                remote: None,
                commit: None,
                detail: why.to_string(),
            },
        });
    }
    preserved
}

/// Every (identity, branch) pair this run's own records name, once each.
///
/// Three records, one question. `sessions` is the session each node's latest
/// dispatch opened, in flight or closed; `abandoned` is where a dispatch an
/// adoption cleared was working; and `branches` is what each settled node's
/// dispatch left behind — which for an unpinned lifecycle node is the only
/// record of where the work is.
///
/// A node no longer in flight whose change **landed** is left out of all three:
/// its work is on its base, on the origin, already — the same line
/// `onevcs::recoverable` draws — and a session branch its publication has
/// finished with may no longer be anywhere to push from, which would report a
/// refusal over work nobody could lose. Work in flight is always offered.
fn branches_of(view: &RunView) -> Vec<(String, String)> {
    let statuses = view.state.statuses();
    let finished_landing = |node: &str| {
        view.state.landings.get(node) == Some(&crate::graph::Landing::Landed)
            && statuses.get(node) != Some(&crate::graph::NodeStatus::Running)
    };
    let repo_of = |node: &str| {
        view.state
            .graph
            .get(node)
            .and_then(|node| node.repo.clone())
    };
    let named = view
        .state
        .sessions
        .iter()
        .chain(view.state.abandoned.iter())
        .map(|(node, session)| (node, session.branch().as_str().to_string()))
        .chain(
            view.state
                .branches
                .iter()
                .map(|(node, branch)| (node, branch.clone())),
        );
    let mut found: Vec<(String, String)> = Vec::new();
    for (node, branch) in named {
        if finished_landing(node) {
            continue;
        }
        if let Some(repo) = repo_of(node) {
            let pair = (repo, branch);
            if !found.contains(&pair) {
                found.push(pair);
            }
        }
    }
    found
}

/// Every other unpublished branch on this host, and what stood in the way of
/// asking.
///
/// A fact rather than a failure: somebody decommissioning the machine can see
/// them and push them by hand. A read that could not be made says so rather than
/// reporting none — an empty list from a failed enumeration is the one answer
/// this must never give.
fn elsewhere_on_this_host(
    pushed: &BTreeSet<(String, String)>,
) -> (Vec<(String, String)>, Option<String>) {
    match onevcs::recoverable(&onevcs::Scope::All) {
        Ok(rows) => (
            rows.into_iter()
                .map(|row| (row.identity, row.branch.branch))
                .filter(|pair| !pushed.contains(pair))
                .collect(),
            None,
        ),
        Err(why) => (Vec::new(), Some(why.to_string())),
    }
}

/// What a branch that was pushed is said to be, in the words the repository's
/// own `AGENTS.md` uses for the same carve-out.
///
/// Carried by **every** pushed branch rather than stated once at the foot of the
/// report: a reader scanning for one branch reads one line, and a footnote three
/// screens away is a footnote nobody read.
const UNPROVEN: &str = "on its origin unproven — the branch reached its origin without that \
                        repository's own hook or merge path having run, and nothing can merge \
                        it without a publication that does run one";

/// The text `onepipeline shutdown` prints.
pub fn render_shutdown(shutdown: &Shutdown) -> String {
    let mut out = format!(
        "shutdown  scope {}  grace {}s{}  runs root {}\n",
        shutdown.scope.as_str(),
        shutdown.grace.as_secs(),
        if shutdown.forced { "  forced" } else { "" },
        shutdown.root.display()
    );
    if shutdown.runs.is_empty() {
        out.push_str("  no runs selected: nothing was signalled\n");
    }
    for run in &shutdown.runs {
        out.push_str(&format!(
            "  {}  owner {}  selected by --{}{}\n",
            run.run,
            run.owner,
            shutdown.scope.as_str(),
            if run.forced_over_owner {
                ", which is another session's run"
            } else {
                ""
            }
        ));
        if run.dispatches.is_empty() {
            out.push_str("    no live dispatch to interrupt\n");
        }
        for stopped in &run.dispatches {
            out.push_str(&format!(
                "    {} (pid {}): interrupt {} — {}; ended {}, after {}\n",
                stopped.node,
                stopped.pid,
                stopped.interrupt,
                stopped.detail,
                stopped.ended.as_str(),
                crate::telemetry::duration(
                    u64::try_from(stopped.waited.as_millis()).unwrap_or(u64::MAX)
                )
            ));
        }
        out.push_str(&format!("    teardown: {}\n", teardown_said(run)));
        if run.branches.is_empty() {
            out.push_str("    no branch this run's records name\n");
        }
        for branch in &run.branches {
            out.push_str(&format!("    {}\n", branch_said(branch)));
        }
    }
    out.push_str(&last_section(shutdown));
    out
}

/// The teardown in `stop`'s own words, and any survivor by pid.
fn teardown_said(run: &RunShutdown) -> String {
    let said = crate::verbs::Stopped {
        run: run.run.clone(),
        owner: run.owner.clone(),
        forced: run.forced_over_owner,
        teardown: run.teardown,
        clean: false,
    }
    .refusal()
    .unwrap_or_else(|| {
        format!(
            "{} — every process this run named was reached",
            run.teardown.word()
        )
    });
    let survivors: Vec<String> = run
        .dispatches
        .iter()
        .filter(|stopped| stopped.ended == DispatchEnding::StillRunning)
        .map(|stopped| format!("{} (pid {})", stopped.node, stopped.pid))
        .collect();
    if survivors.is_empty() {
        said
    } else {
        format!("{said}. Still running: {}", survivors.join(", "))
    }
}

/// What one branch's line says.
fn branch_said(branch: &BranchPreserved) -> String {
    let what = match branch.outcome {
        Preserved::Pushed => format!(
            "{UNPROVEN}{}",
            branch
                .remote
                .as_deref()
                .map_or(String::new(), |remote| format!(" ({remote})"))
        ),
        Preserved::AlreadyOnOrigin => {
            "already on its origin at this commit; nothing was pushed".to_string()
        }
        Preserved::NoRemote => {
            "this identity has no origin to push to, so nothing outside this host carries it"
                .to_string()
        }
        Preserved::Refused => format!("could not be preserved: {}", branch.detail),
    };
    format!(
        "{}@{}: {what}{}",
        branch.identity,
        branch.branch,
        branch
            .commit
            .as_deref()
            .map_or(String::new(), |commit| format!(" [{commit}]"))
    )
}

/// The report's last section: every other unpublished branch on this host.
fn last_section(shutdown: &Shutdown) -> String {
    if let Some(why) = &shutdown.not_pushed_unread {
        return format!(
            "  other unpublished branches on this host: not read — {why}. This is not a count \
             of zero: there may be work here nothing outside this machine carries\n"
        );
    }
    if shutdown.not_pushed.is_empty() {
        return "  other unpublished branches on this host: none\n".to_string();
    }
    let mut out = format!(
        "  other unpublished branches on this host, which this shutdown did not push ({}):\n",
        shutdown.not_pushed.len()
    );
    for (identity, branch) in &shutdown.not_pushed {
        out.push_str(&format!("    {identity}@{branch}\n"));
    }
    out
}
