//! Release adoption: when a node launches relative to its dependencies'
//! *releases* rather than only their branches.
//!
//! A plan node declares an [`Adoption`] mode, and the mode decides one thing:
//! whether a dependency's finished **work** is enough to launch the node, or
//! whether the node waits for the **release** that carries that work.
//!
//! Under `fast` the node launches on branch readiness alone — today's readiness,
//! unchanged — and its dispatch is handed the git references of every dependency
//! that lands outside its own repository, so the worker pins against git rather
//! than against a version that does not exist yet. When those releases arrive it
//! is sent a note naming the versions, into the live turn where the dispatch has
//! a controllable one and onto its next dispatch where it does not.
//!
//! Under `published` the node is not scheduled at all until every one of those
//! dependencies answers released. That wait **blocks indefinitely and never
//! fails a node**: there is no timeout, no deadline, no retry budget, and no
//! automatic degrade to fast adoption. Only an answer of *released* starts it,
//! and "not answered" is never evidence that a release has not happened.
//!
//! `onevcs` records a release **style** per target and this module consumes it.
//! The scheduler's behaviour is identical for both styles — one hold, indefinite,
//! never failing — and what differs is only where the readiness answer comes from
//! and what is reported: an automated target's answer is its probe, which is a
//! subprocess and is therefore paced on its own interval and asked off the
//! reconcile loop's thread, and a human-step target's answer is the
//! acknowledgement record `onevcs release acknowledge` writes, for which this
//! crate runs no probe because there is none to run. Nothing here performs a
//! human release step, prompts for one, or acknowledges one on somebody's behalf.

// llmlint: ignore-file[invalid_states_unrepresentable] a node id, a dependency
// reference, and a repository identity are the plain strings the plan schema spells and
// the journal payload carries, for the reason `src/plan.rs` records; and a
// [`Dependency`]'s cells are each `Option` because the whole point of the rendering is
// that a cell the run cannot name is *empty* rather than the row being dropped.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use onevcs::releases::{ReleaseStyle, RepositoryReleases, TargetName};
use onevcs::{Adoption, ReleaseStatus};
use serde_json::{json, Value};

use crate::channel::Surface;
use crate::error::Result;
use crate::graph::NodeStatus;
use crate::journal::{self, Journal};
use crate::ledger::RunPaths;
use crate::plan::{CrossRepoReference, Node};
use crate::projection::RunState;

/// The environment variable bounding how often an **automated** target's probe
/// is run.
pub const POLL_ENV: &str = "ONEPIPELINE_RELEASE_POLL_SECONDS";

/// How often an automated target's probe is run when nothing overrides it.
///
/// A probe is a subprocess, so this is a bound on how much a held run costs the
/// host rather than a promise about latency: a release that arrives between two
/// asks is noticed at the second one.
pub const DEFAULT_POLL_SECONDS: u64 = 120;

/// The environment variable bounding how often a held node's wait is surfaced.
pub const SURFACE_ENV: &str = "ONEPIPELINE_RELEASE_SURFACE_SECONDS";

/// How often a held node's wait is surfaced to the planner when nothing
/// overrides it.
///
/// Much longer than [`DEFAULT_POLL_SECONDS`], and deliberately: asking whether a
/// release has happened is cheap and repeating the question to a person is not.
/// The wait is repeated rather than stated once so it cannot go silent, and a
/// person reading it decides whether to keep waiting, flip the node to fast
/// adoption, or stop the run.
pub const DEFAULT_SURFACE_SECONDS: u64 = 900;

/// The kind a held node's wait is surfaced under.
pub const WAIT_SURFACE_KIND: &str = "release-wait";

/// What one awaited release is tracked by: the node waiting, and the dependency
/// it is waiting on.
type Key = (String, String);

/// One answer, on its way back from the asker to the reconcile loop, and every
/// wait [`questions_of`] put the question on behalf of.
///
/// One message for all of them, so the loop applies them in one go and two nodes
/// awaiting one release are never caught disagreeing about it.
type Answered = (Vec<Key>, Answer);

/// Which rung of the adoption chain one node resolves to.
///
/// Exactly four rungs, in this order and with no fifth:
///
/// 1. the node's own [`adoption`](Node::adoption);
/// 2. the repository rung, and
/// 3. the global rung — both of which are `onevcs`'s and are answered together
///    by [`onevcs::adoption_for`], which falls from the first to the second
///    itself;
/// 4. [`Adoption::Fast`].
///
/// There is deliberately no plan-level tier and no run-only override: the
/// operator specified four rungs, and a fifth changes what they asked for.
///
/// A node with no `repo` has no repository rung. The global rung is not
/// reachable without naming a repository — see `docs/contract-divergences.md`
/// entry 40 — so such a node falls to the floor, which is what the global rung
/// answers on any host that has not configured otherwise.
pub(crate) fn adoption_of(node: &Node) -> Adoption {
    if let Some(declared) = node.adoption {
        return declared;
    }
    if let Some(repo) = node.repo.as_deref() {
        if let Ok(resolved) = onevcs::adoption_for(repo) {
            return resolved;
        }
    }
    Adoption::Fast
}

/// The last thing `onevcs` said about one awaited release.
///
/// Five answers, and **none of them is folded into another**. "Awaiting a human
/// step" is not "not released", which means a probe answered and the version has
/// not moved, and it is not "not answered", which means a probe failed — folding
/// it into the last would report a perfectly healthy wait on a person as a broken
/// probe. And "not answered" is never recorded as "not released" anywhere: not in
/// the scheduler, not in an event payload, and not in a rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    /// A release carrying the dependency's work is out, at this version. The one
    /// answer that releases a hold.
    Released {
        /// The version that carries it.
        version: String,
    },
    /// A probe answered, and the baseline has not been passed.
    NotReleased,
    /// It landed, and nobody has recorded the human step yet.
    AwaitingHumanStep,
    /// The question was not answered. **Never "not released".**
    NotAnswered,
    /// The work has not reached its base, so there is no release to ask about.
    NotLanded,
}

/// How a wait names an awaited release **nothing has answered yet**.
///
/// Deliberately not [`Answer::NotAnswered`], which is a probe this host *ran*
/// and got no usable answer out of and which sends its reader to go and look at
/// that probe. This one is a question still out, which is the state every wait
/// is in before its first probe completes.
///
/// A wait with no question to put at all — no reference the sibling resolves work
/// by, or no target that answers — is neither: nothing will ever answer it, and
/// [`Answer::NotAnswered`] is what that is.
const NO_ANSWER_YET: &str = "no-answer-yet";

impl Answer {
    /// The word an event payload and a rendering name this answer with.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Released { .. } => "released",
            Self::NotReleased => "not-released",
            Self::AwaitingHumanStep => "awaiting-human-step",
            Self::NotAnswered => "not-answered",
            Self::NotLanded => "not-landed",
        }
    }

    /// The version this answer carries, when it is the one that releases a hold.
    fn version(&self) -> Option<&str> {
        match self {
            Self::Released { version } => Some(version),
            _ => None,
        }
    }

    /// What the sibling answered, read as this crate's own vocabulary.
    ///
    /// A refusal — the repository declares no targets, no default target, or the
    /// reference names nothing — is [`NotAnswered`](Self::NotAnswered) and never
    /// anything else: a question that could not be put is not an answer that the
    /// release has not happened.
    fn of(status: &onevcs::Result<ReleaseStatus>) -> Self {
        match status {
            // A version this crate cannot **render** is one it cannot report: it
            // is written into a worker's own note, into a surface, and into an
            // event payload, so one carrying whitespace or a control character
            // forges a line in each. Answered unusably is the sibling's own
            // reading of a probe whose output is not one usable line, and it is
            // this one's of a version that is not one usable word — which holds
            // the node, exactly as every answer but a release does.
            Ok(ReleaseStatus::Released { version, .. }) => match renderable(version) {
                Some(version) => Self::Released { version },
                None => Self::NotAnswered,
            },
            Ok(ReleaseStatus::NotReleased { .. }) => Self::NotReleased,
            Ok(ReleaseStatus::AwaitingHumanStep { .. }) => Self::AwaitingHumanStep,
            Ok(ReleaseStatus::NotAnswered { .. }) | Err(_) => Self::NotAnswered,
            Ok(ReleaseStatus::NotLanded) => Self::NotLanded,
        }
    }
}

/// One dependency of a node whose work lands **outside** that node's repository,
/// in a repository that releases something.
///
/// A dependency inside the same repository is not one of these: the lifecycle
/// already prepares the stacked or merged-stacked branch for it, and nothing
/// here changes that. Neither is one whose repository declares no release
/// targets — there is no release to wait for, which is every repository on a host
/// that has configured none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Dependency {
    /// The dependency as the plan names it: a node id, or a cross-DAG
    /// `run:<id>#<node>` reference.
    pub dep: String,
    /// The repository identity its work lands in.
    pub identity: String,
    /// The branch the work is on, where the run recorded one.
    pub branch: Option<String>,
    /// The commit that work reached its base at, where the run observed one.
    pub commit: Option<String>,
    /// The release target this node consumes that repository at.
    pub target: Option<TargetName>,
    /// How that target is released.
    pub style: Option<ReleaseStyle>,
    /// What a person has to do, for a human-step target.
    pub action: Option<String>,
}

impl Dependency {
    /// What `onevcs` is asked about.
    ///
    /// The **branch**, because that is the spelling the sibling resolves work by:
    /// a reference is a change request's URL, a session token, a branch a
    /// registered checkout or run clone holds, or a commit one of *those branches*
    /// carries — and a landing commit sitting on the base alone is none of them.
    /// The sibling resolves the branch to the landing itself, which is what a
    /// release is measured against; the commit is what the reference block shows a
    /// worker, and the fallback for work whose branch this run did not record.
    fn reference(&self) -> Option<&str> {
        self.branch.as_deref().or(self.commit.as_deref())
    }

    /// Whether there is a question about this dependency to put at all.
    ///
    /// Both halves are needed to ask one: the reference the sibling resolves the
    /// work by, and the style that says where the answer comes from. A dependency
    /// missing either is one no probe is ever run for, so its wait is not one
    /// still expecting a first answer — it is [`Answer::NotAnswered`], which is
    /// what a question that could not be put is.
    fn askable(&self) -> bool {
        self.reference().is_some() && self.style.is_some()
    }

    /// The row this dependency renders as in a fast-adoption node's task.
    fn row(&self) -> CrossRepoReference {
        CrossRepoReference {
            dependency: self.dep.clone(),
            repository: self.identity.clone(),
            branch: self.branch.clone().unwrap_or_default(),
            commit: self.commit.clone().unwrap_or_default(),
            release_target: self
                .target
                .as_ref()
                .map(TargetName::to_string)
                .unwrap_or_default(),
        }
    }

    /// How this dependency's release is named where one is reported.
    fn named(&self) -> String {
        match &self.target {
            Some(target) => format!("{} {target}", self.identity),
            None => self.identity.clone(),
        }
    }
}

/// What each repository releases, read once per driver.
///
/// Cached because asking is not free — `onevcs` reads the release-targets
/// document and inspects the repository's publication checkout to decide where a
/// script probe could run — and a held node asks the same question on every
/// reconcile pass. Configuration is a fact about the host rather than about the
/// run, so one read per driver is the right number.
#[derive(Debug, Default)]
struct Repositories {
    known: BTreeMap<String, Option<RepositoryReleases>>,
}

impl Repositories {
    /// What one repository releases, or `None` where the sibling could not say.
    fn of(&mut self, repo: &str) -> Option<&RepositoryReleases> {
        self.known
            .entry(repo.to_owned())
            .or_insert_with(|| onevcs::release_targets(repo).ok())
            .as_ref()
    }
}

/// One question the asker puts to `onevcs`, and every wait it answers — see
/// [`questions_of`] for why those are not the same count.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Question {
    /// Every node-and-dependency pair this one answer belongs to.
    keys: Vec<Key>,
    /// What the landed work is named by.
    reference: String,
    /// The target, or `None` for the repository's own default.
    target: Option<TargetName>,
    /// Which of the two styles this is, which decides only how it is paced.
    style: ReleaseStyle,
}

/// The thread that asks `onevcs` whether a release has happened.
///
/// Off the reconcile loop's own thread, and that is the whole reason it exists: an
/// automated target's answer is a **subprocess**, and a slow or hanging probe asked
/// inline would stall the loop every other node in the run depends on. The loop
/// hands over the current question set and reads whatever answers have arrived; it
/// never waits for one.
struct Asker {
    /// The current question set. Dropping this ends the thread.
    questions: Sender<Vec<Question>>,
    /// What has been answered since the loop last looked.
    answers: Receiver<Answered>,
}

/// How long the asker waits for a new question set before re-asking the one it
/// has.
///
/// Short, because a **human-step** answer is a record read rather than a probe
/// and is therefore asked as fast as it completes rather than on the probe's
/// interval; long enough that asking is not a spin. An automated question is
/// skipped on a tick the poll interval has not come due on.
const TICK: Duration = Duration::from_millis(250);

impl Asker {
    /// Start asking, pacing automated questions at `poll`.
    fn start(poll: Duration) -> Self {
        let (questions, asked): (Sender<Vec<Question>>, Receiver<Vec<Question>>) = mpsc::channel();
        let (answered, answers): (Sender<Answered>, Receiver<Answered>) = mpsc::channel();
        // Detached: it holds no run state, writes nothing, and ends when the
        // reconcile loop drops its end of `questions`.
        // llmlint: ignore-block[changed_behavior_has_e2e] no invocation a user can type
        // reaches this arm: it is a host that will not start a thread at all, which no
        // plan, flag, or environment of this crate's decides. What it does when it is
        // reached is the safe direction of every answer staying unarrived, which is the
        // hold a published node is under before its first probe answers and is driven end
        // to end by `tests/e2e/adoption.rs`.
        std::thread::Builder::new()
            .name("release-asker".to_owned())
            .spawn(move || ask_until_dropped(&asked, &answered, poll))
            .map(drop)
            .unwrap_or_else(|error| {
                // A thread this host would not start is reported and nothing
                // more: every answer then stays unarrived, which holds a
                // published node exactly as an unanswered probe does and leaves
                // a fast node with the git pin it launched under. Neither is a
                // node failed for a reason that is not about the node.
                eprintln!("onepipeline: cannot start the release watch: {error}");
            }); // llmlint: ignore-end[changed_behavior_has_e2e]
        Self { questions, answers }
    }

    /// Hand over the questions to ask from now on.
    fn ask(&self, questions: Vec<Question>) {
        // A dead asker is not an error here for the reason its refusal to start
        // is not: it leaves every answer unarrived, which is the safe direction.
        let _ = self.questions.send(questions);
    }

    /// Everything answered since the last look. Never blocks.
    fn answered(&self) -> Vec<Answered> {
        self.answers.try_iter().collect()
    }
}

/// The asker's own loop.
fn ask_until_dropped(asked: &Receiver<Vec<Question>>, answered: &Sender<Answered>, poll: Duration) {
    let mut questions: Vec<Question> = Vec::new();
    let mut probed: Option<Instant> = None;
    loop {
        match asked.recv_timeout(TICK) {
            Ok(fresh) => questions = fresh,
            Err(RecvTimeoutError::Timeout) => {}
            // The reconcile loop has gone, so there is nobody to answer.
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // Every question set the loop queued while this one was being asked, so
        // a slow probe leaves the asker working from the newest set rather than
        // from a backlog of stale ones.
        while let Ok(fresh) = asked.try_recv() {
            questions = fresh;
        }
        let due = probed.is_none_or(|last| last.elapsed() >= poll);
        let mut ran_a_probe = false;
        for question in &questions {
            if question.style == ReleaseStyle::Automated {
                if !due {
                    continue;
                }
                ran_a_probe = true;
            }
            // The one call, for both styles. An automated target is answered by
            // running its probe under the probe's own timeout; a human-step
            // target executes nothing at all and is answered from the
            // acknowledgement record. There is no spelling of this that could
            // start a subprocess for a human-step target, because the probe
            // lives on the other variant.
            let answer = Answer::of(&onevcs::release_status(
                &question.reference,
                question.target.as_ref(),
            ));
            if answered.send((question.keys.clone(), answer)).is_err() {
                return;
            }
        }
        if ran_a_probe {
            probed = Some(Instant::now());
        }
    }
}

/// The releases this run is waiting on, and what it has already said about them.
pub(crate) struct Watch {
    repositories: Repositories,
    /// The out-of-repository dependencies of each node, once the run could name
    /// them.
    ///
    /// Frozen per node the first time every one of its dependencies resolves,
    /// because by then each has settled `done` and a settled node's repository,
    /// branch, and landing commit do not move: a `retry` replaces the node
    /// under a new id, and a `requeue` continues the branch this already names.
    dependencies: BTreeMap<String, Vec<Dependency>>,
    /// The last answer about each awaited release.
    answers: BTreeMap<Key, Answer>,
    /// When each wait was first observed, in epoch milliseconds.
    since: BTreeMap<Key, u64>,
    /// The nodes an arrival note has already reached, seeded from the journal so
    /// a fresh driver does not deliver one twice.
    adopted: BTreeSet<String>,
    /// The awaited releases already reported as arrived, seeded the same way.
    arrived: BTreeSet<Key>,
    /// When each held node's wait was last surfaced.
    surfaced: BTreeMap<String, Instant>,
    /// How often a held node's wait is surfaced.
    surface_every: Duration,
    /// How often the identity's release records are read for what they say
    /// about this run's own landed work.
    ///
    /// The **probe's** interval, because it is the same question asked from the
    /// other end: a release nobody has asked about yet has nothing recorded
    /// about it either, so reading more often than the run asks would read the
    /// same file again for an answer nothing has written.
    relay_every: Duration,
    /// When the release records were last read. `None` before the first read,
    /// which is due immediately.
    relayed: Option<Instant>,
    asker: Asker,
}

impl Watch {
    /// Start watching one run, taking up whatever a previous driver said.
    ///
    /// The journal is external input, and what it seeds here **suppresses** a
    /// delivery — so a record that does not name both halves of what it claims
    /// was said is skipped rather than read as something. A record naming a node
    /// or a dependency this graph does not have suppresses nothing, because
    /// nothing is ever looked up under it; what a lax reading would cost is a
    /// record with an *empty* node or dep, which is a key a real one could
    /// collide with.
    pub(crate) fn of_run(paths: &RunPaths) -> Self {
        let mut adopted = BTreeSet::new();
        let mut arrived = BTreeSet::new();
        for event in journal::read(&paths.journal()) {
            let Some(node) = event.labels.node.clone().and_then(|node| renderable(&node)) else {
                continue;
            };
            match journal::PipelineKind::from_wire(&event.kind) {
                Some(journal::PipelineKind::ReleaseAdopted) => {
                    // Usable only if it names a release this build can read: a
                    // record whose versions are gone is one nothing can say what
                    // the node was told, so suppressing on it would leave a node
                    // never told anything and never told again.
                    if !Released::of_payload(event.payload.get("versions").unwrap_or(&Value::Null))
                        .is_empty()
                    {
                        adopted.insert(node);
                    }
                }
                Some(journal::PipelineKind::ReleaseArrived) => {
                    if let Some(dep) = event
                        .payload
                        .get("dep")
                        .and_then(Value::as_str)
                        .and_then(renderable)
                    {
                        arrived.insert((node, dep));
                    }
                }
                _ => {}
            }
        }
        Self {
            repositories: Repositories::default(),
            dependencies: BTreeMap::new(),
            answers: BTreeMap::new(),
            since: BTreeMap::new(),
            adopted,
            arrived,
            surfaced: BTreeMap::new(),
            surface_every: Duration::from_secs(surface_every_seconds()),
            relay_every: Duration::from_secs(poll_seconds()),
            relayed: None,
            asker: Asker::start(Duration::from_secs(poll_seconds())),
        }
    }

    /// The rows a node's dispatch is handed, in the order its `deps` name them.
    ///
    /// **Fast adoption only.** A `published` node is not started until every one
    /// of these has answered released, so it launches against versions rather
    /// than against a git pin — and the block's own words say it launched under
    /// fast adoption, which for that node would not be true.
    pub(crate) fn references(&self, node: &Node) -> Vec<CrossRepoReference> {
        if adoption_of(node) != Adoption::Fast {
            return Vec::new();
        }
        self.dependencies
            .get(&node.id)
            .map(|dependencies| dependencies.iter().map(Dependency::row).collect())
            .unwrap_or_default()
    }

    /// Take up the answers that have arrived, and ask about what is awaited now.
    ///
    /// `watching` is the nodes whose releases matter this pass: every node that
    /// is ready to start, and every fast-adoption node still running. Neither
    /// blocks on anything.
    pub(crate) fn refresh(&mut self, paths: &RunPaths, state: &RunState, watching: &[Node]) {
        for (keys, answer) in self.asker.answered() {
            for key in keys {
                self.answers.insert(key, answer.clone());
            }
        }
        let now = crate::sys::now_millis();
        let mut waits: Vec<(Key, Dependency)> = Vec::new();
        for node in watching {
            for dependency in self.resolve(paths, state, node) {
                let key = (node.id.clone(), dependency.dep.clone());
                self.since.entry(key.clone()).or_insert(now);
                waits.push((key, dependency));
            }
        }
        self.asker.ask(questions_of(&waits));
    }

    /// Whether every release one node awaits has arrived.
    ///
    /// `false` where it awaits none, so a node with no out-of-repository
    /// dependency is never reported as having adopted anything.
    fn all_released(&self, node: &str) -> bool {
        let dependencies = self.dependencies.get(node).filter(|of| !of.is_empty());
        dependencies.is_some_and(|dependencies| {
            dependencies.iter().all(|dependency| {
                self.answers
                    .get(&(node.to_owned(), dependency.dep.clone()))
                    .and_then(Answer::version)
                    .is_some()
            })
        })
    }

    /// The nodes a release hold will not let start yet.
    ///
    /// A `published` node whose out-of-repository dependencies have not all
    /// answered released. Nothing else holds anything: a `fast` node launches on
    /// branch readiness alone.
    pub(crate) fn held(&self, watching: &[Node]) -> BTreeSet<String> {
        watching
            .iter()
            .filter(|node| adoption_of(node) == Adoption::Published)
            .filter(|node| !self.all_released(&node.id))
            .filter(|node| {
                self.dependencies
                    .get(&node.id)
                    .is_some_and(|dependencies| !dependencies.is_empty())
            })
            .map(|node| node.id.clone())
            .collect()
    }

    /// Report every release that has arrived, and every wait that is still on.
    ///
    /// The wait is surfaced when it begins and again on its own interval, so it
    /// cannot go silent; the arrival of one release is reported once.
    pub(crate) fn report(
        &mut self,
        paths: &RunPaths,
        journal: &mut Journal,
        held: &BTreeSet<String>,
        watching: &[Node],
    ) -> Result<()> {
        for node in watching {
            let dependencies = self.dependencies.get(&node.id).cloned().unwrap_or_default();
            for dependency in &dependencies {
                let key = (node.id.clone(), dependency.dep.clone());
                let Some(version) = self.answers.get(&key).and_then(Answer::version) else {
                    continue;
                };
                if !self.arrived.insert(key) {
                    continue;
                }
                journal.emit(
                    journal::PipelineKind::ReleaseArrived,
                    journal::labels(&paths.run, Some(&node.id)),
                    journal::payload(&[
                        ("node", json!(node.id)),
                        ("dep", json!(dependency.dep)),
                        ("identity", json!(dependency.identity)),
                        (
                            "target",
                            json!(dependency.target.as_ref().map(ToString::to_string)),
                        ),
                        ("style", json!(dependency.style.map(|style| style.as_str()))),
                        ("version", json!(version)),
                    ]),
                )?;
            }
        }
        for node in held {
            let due = self
                .surfaced
                .get(node)
                .is_none_or(|last| last.elapsed() >= self.surface_every);
            if !due {
                continue;
            }
            self.surfaced.insert(node.clone(), Instant::now());
            // The surface first and the record second. They are two appends
            // saying one thing, so a reader holding the record and reading the
            // surface beside it gets whichever surface was there when it looked:
            // raised second, that is the previous one, carrying the previous
            // answer about a probe that has since stopped answering. This way
            // round the surface is never older than the record beside it.
            crate::engine::raise(paths, journal, self.wait_surface(node))?;
            let awaiting = self.awaiting(node);
            journal.emit(
                journal::PipelineKind::ReleaseWait,
                journal::labels(&paths.run, Some(node)),
                journal::payload(&[("node", json!(node)), ("awaiting", json!(awaiting))]),
            )?;
        }
        // A node that is no longer held says nothing more: the arrival is
        // reported by `release-arrived`, and repeating the wait after it ended
        // would report a run as waiting on something it has.
        self.surfaced.retain(|node, _| held.contains(node));
        Ok(())
    }

    /// Relay what the sibling recorded about the releases carrying this run's
    /// own landed work.
    ///
    /// A release happens long after the dispatch that produced the work has
    /// ended, outside every session — so `onevcs` records it on the repository's
    /// own release record and joins it back to the session whose **landing
    /// commit** it names. That join is the sibling's, and the address of that
    /// record is the sibling's too: this asks only for the session it already
    /// knows, through the same phase-aware reader every followed session is read
    /// through, and takes back whatever that reader says belongs to it.
    ///
    /// Only the [`Release`](crate::event::Phase::Release) phase, and only past
    /// the mark each stream already stands at in this run's own store. The
    /// session's own records were relayed by the follow that watched them, and
    /// `release-probed` — which the publication wrote on the session's stream
    /// while that follow was reading it — is among them: reporting one ask as
    /// two is the same defect as losing it.
    ///
    /// The marks are the store's own, so what stops a record arriving twice is
    /// the record.
    // llmlint: ignore-block[changed_behavior_has_e2e] the two failures this reaches are
    // not decided here: an unreadable stream is [`crate::vcs::events`]'s own answer and
    // an unwritable journal is [`Journal::relay`]'s, each held where it lives.
    pub(crate) fn relay_releases(
        &mut self,
        paths: &RunPaths,
        journal: &mut Journal,
        state: &RunState,
        filter: Option<&crate::filter::EventFilter>,
    ) -> Result<()> {
        if !self
            .relayed
            .is_none_or(|last| last.elapsed() >= self.relay_every)
        {
            return Ok(());
        }
        self.relayed = Some(Instant::now());
        let statuses = state.statuses();
        let mut relayed = crate::vcs::Watermarks::of_relayed(&journal::read(&paths.journal()));
        for (node, session) in &state.sessions {
            // A dispatch still in flight has a follow of its own reading that
            // session as it is written, and two readers of one stream deciding
            // separately what the store already holds is how a record arrives
            // twice. Its releases are read once it has settled, which is the
            // earliest its work can have landed anyway.
            if statuses.get(node) == Some(&NodeStatus::Running) {
                continue;
            }
            let releases_nothing = state
                .graph
                .get(node)
                .and_then(|node| node.repo.clone())
                .and_then(|repo| self.repositories.of(&repo))
                .is_none_or(|releases| releases.targets.is_empty());
            // A repository that declares no release targets releases nothing, so
            // there is nothing recorded about it to read — which is every host
            // that has configured none, and is what keeps such a run's store
            // exactly the store it held before there was a release record at all.
            if releases_nothing {
                continue;
            }
            let known = crate::engine::dispatch_labels(
                &paths.run,
                node,
                None,
                state
                    .graph
                    .get(node)
                    .and_then(|node| node.persona.as_deref()),
            );
            for mut envelope in crate::vcs::events(session.token(), filter) {
                if envelope.phase != Some(crate::event::Phase::Release) {
                    continue;
                }
                if !relayed.beyond(&envelope) {
                    continue;
                }
                // An enricher, never a rewriter: the producer could not know
                // which node's work this release carries, and everything it did
                // stamp stands.
                crate::lifecycle::stamp(&mut envelope.labels, &known);
                journal.relay(&envelope)?;
                relayed.reached(&envelope);
            }
        }
        Ok(())
    } // llmlint: ignore-end[changed_behavior_has_e2e]

    /// The word a payload and a surface name one awaited release's last answer
    /// by.
    ///
    /// Three states and not two, because a reader acts differently on each: the
    /// answer this run has, a question still out ([`NO_ANSWER_YET`]), and a
    /// question that could never be put — no reference to ask about or no target
    /// that answers — which is [`Answer::NotAnswered`] and never
    /// [`Answer::NotReleased`].
    fn last_answer(&self, key: &Key, dependency: &Dependency) -> &'static str {
        match self.answers.get(key) {
            Some(answer) => answer.as_str(),
            None if dependency.askable() => NO_ANSWER_YET,
            None => Answer::NotAnswered.as_str(),
        }
    }

    /// The `awaiting` list one held node's wait carries.
    fn awaiting(&self, node: &str) -> Vec<Value> {
        let now = crate::sys::now_millis();
        self.dependencies
            .get(node)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter(|dependency| {
                self.answers
                    .get(&(node.to_owned(), dependency.dep.clone()))
                    .and_then(Answer::version)
                    .is_none()
            })
            .map(|dependency| {
                let key = (node.to_owned(), dependency.dep.clone());
                let since = self.since.get(&key).copied().unwrap_or(now);
                let mut entry = journal::payload(&[
                    ("dep", json!(dependency.dep)),
                    ("identity", json!(dependency.identity)),
                    (
                        "target",
                        json!(dependency.target.as_ref().map(ToString::to_string)),
                    ),
                    ("style", json!(dependency.style.map(|style| style.as_str()))),
                ]);
                // Only a human-step wait carries the action: it is the text a
                // person needs, and an automated wait has nobody to hand it to.
                if dependency.style == Some(ReleaseStyle::HumanStep) {
                    entry.insert("action".to_owned(), json!(dependency.action));
                }
                entry.insert(
                    "since".to_owned(),
                    json!(crate::sys::rfc3339_from_millis(since)),
                );
                entry.insert(
                    "waited_seconds".to_owned(),
                    json!(now.saturating_sub(since) / 1_000),
                );
                entry.insert(
                    "last_answer".to_owned(),
                    json!(self.last_answer(&key, dependency)),
                );
                Value::Object(entry)
            })
            .collect()
    }

    /// The surface a held node's wait raises.
    ///
    /// Non-blocking: the hold is the scheduler's, and a blocking surface would
    /// hold the same subtree twice while reading, in every planner view, as a
    /// decision somebody has to answer before the run can move. This one is a
    /// report — the decision it informs is whether to go on waiting at all.
    fn wait_surface(&self, node: &str) -> Surface {
        let now = crate::sys::now_millis();
        let mut lines: Vec<String> = Vec::new();
        for dependency in self
            .dependencies
            .get(node)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let key = (node.to_owned(), dependency.dep.clone());
            if self.answers.get(&key).and_then(Answer::version).is_some() {
                continue;
            }
            let waited = crate::telemetry::duration(
                now.saturating_sub(self.since.get(&key).copied().unwrap_or(now)),
            );
            let answered = self.last_answer(&key, dependency);
            // The style is named in the sentence itself, so an automated wait
            // and a wait on a person are tellable apart from this text alone —
            // without the reader opening the release-targets file to find out
            // which kind of wait they are looking at.
            let style = match (dependency.style, dependency.action.as_deref()) {
                (Some(ReleaseStyle::HumanStep), Some(action)) => {
                    format!("human-step release — a person has to: {action}")
                }
                (Some(ReleaseStyle::HumanStep), None) => "human-step release".to_owned(),
                (Some(ReleaseStyle::Automated), _) => "automated release".to_owned(),
                (None, _) => "no release target this host can name".to_owned(),
            };
            lines.push(format!(
                "- {named} — {style}, waited {waited}, last answer: {answered}",
                named = dependency.named(),
            ));
        }
        Surface {
            id: 0,
            kind: WAIT_SURFACE_KIND.to_owned(),
            message: format!(
                "node '{node}' is held under published adoption, waiting on {count} \
                 release(s):\n{lines}\nNothing times this out and nothing will fail the node. \
                 Keep waiting, flip this node to `adoption: fast` by live edit, or stop the run.",
                count = lines.len(),
                lines = lines.join("\n"),
            ),
            source: crate::channel::source::PROPOSAL.to_owned(),
            blocking: false,
            queued_at: now,
            workstream: Some(node.to_owned()),
        }
    }

    /// The nodes whose awaited releases have all arrived and which have not been
    /// told yet, with the versions to tell them.
    ///
    /// **Fast adoption only**, for the reason [`references`](Self::references)
    /// gives: a `published` node launched against those versions in the first
    /// place, so a note telling it to move off a git pin it never held is noise
    /// aimed at a worker who cannot act on it.
    ///
    /// `told` is every node an arrival is owed to: the ones still running, whose
    /// live turn it reaches, **and** the ones whose change is held as a draft,
    /// which is where it does more than inform — the note is what puts a worker
    /// back on that branch to move the pin, and lifting the draft is what that
    /// worker's own publication then does.
    pub(crate) fn ready_to_adopt(&self, told: &[Node]) -> Vec<(String, Vec<Released>)> {
        told.iter()
            .filter(|node| adoption_of(node) == Adoption::Fast)
            .filter(|node| !self.adopted.contains(&node.id))
            .filter(|node| self.all_released(&node.id))
            .map(|node| (node.id.clone(), self.released(&node.id)))
            .collect()
    }

    /// The versions one node's awaited releases arrived at.
    fn released(&self, node: &str) -> Vec<Released> {
        self.dependencies
            .get(node)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|dependency| {
                Some(Released {
                    identity: dependency.identity.clone(),
                    target: dependency
                        .target
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                    version: self
                        .answers
                        .get(&(node.to_owned(), dependency.dep.clone()))
                        .and_then(Answer::version)?
                        .to_owned(),
                })
            })
            .collect()
    }

    /// Record that one node has been told, so it is told exactly once.
    pub(crate) fn adopted(&mut self, node: &str) {
        self.adopted.insert(node.to_owned());
    }

    /// One node's out-of-repository dependencies, resolved once and then kept.
    fn resolve(&mut self, paths: &RunPaths, state: &RunState, node: &Node) -> Vec<Dependency> {
        if let Some(known) = self.dependencies.get(&node.id) {
            return known.clone();
        }
        let mine = node
            .repo
            .as_deref()
            .and_then(|repo| self.repositories.of(repo))
            .map(|releases| releases.identity.clone());
        let mut resolved: Vec<Dependency> = Vec::new();
        for dep in &node.deps {
            match self.dependency(paths, state, node, dep, mine.as_deref()) {
                // A dependency this run cannot describe at all leaves the whole
                // set unfrozen: it is answered again next pass rather than the
                // node being launched against a set with a row missing from it.
                Resolution::Unreadable => return Vec::new(),
                Resolution::NothingToAwait => {}
                Resolution::Outside(dependency) => resolved.push(dependency),
            }
        }
        self.dependencies.insert(node.id.clone(), resolved.clone());
        resolved
    }

    /// What one `deps` entry is, to the node that names it.
    fn dependency(
        &mut self,
        paths: &RunPaths,
        state: &RunState,
        node: &Node,
        dep: &str,
        mine: Option<&str>,
    ) -> Resolution {
        let target = node.consumes.get(dep).cloned();
        // A cross-DAG dependency is out-of-repository whatever repository it
        // lands in: the branch is another run's, so the stacked-branch machinery
        // this crate has cannot reach it and a git pin is the only thing a
        // worker can hold.
        if let Some(reference) = crate::crossdag::parse(dep) {
            let Some(upstream) = upstream_of(paths, &reference) else {
                return Resolution::Unreadable;
            };
            let Some(repo) = upstream
                .graph
                .get(&reference.node)
                .and_then(|node| node.repo.clone())
            else {
                // The upstream node lands in no repository, so it releases
                // nothing and there is nothing to pin against.
                return Resolution::NothingToAwait;
            };
            return self.outside(
                dep,
                &repo,
                upstream.branches.get(&reference.node).cloned(),
                upstream.landing_commits.get(&reference.node).cloned(),
                target,
            );
        }
        let Some(upstream) = state.graph.get(dep) else {
            return Resolution::Unreadable;
        };
        let Some(repo) = upstream.repo.clone() else {
            // A dependency that lands in no repository releases nothing.
            return Resolution::NothingToAwait;
        };
        let identity = self.repositories.of(&repo).map(|it| it.identity.clone());
        if identity.is_none() {
            return Resolution::Unreadable;
        }
        if identity.as_deref() == mine {
            // The lifecycle already prepares the stacked or merged-stacked
            // branch for this one, exactly as it does today.
            return Resolution::NothingToAwait;
        }
        self.outside(
            dep,
            &repo,
            state.branches.get(dep).cloned(),
            state.landing_commits.get(dep).cloned(),
            target,
        )
    }

    /// One out-of-repository dependency, with what its repository releases.
    fn outside(
        &mut self,
        dep: &str,
        repo: &str,
        branch: Option<String>,
        commit: Option<String>,
        named: Option<TargetName>,
    ) -> Resolution {
        let Some(releases) = self.repositories.of(repo) else {
            return Resolution::Unreadable;
        };
        // A repository that declares **no release targets releases nothing**, so
        // there is no release to wait for and nothing to pin against instead of
        // one. That is every repository on a host that has configured none —
        // which is every host there was before `onevcs` had a release-targets
        // document at all — and it is what keeps a plan naming neither new field
        // producing exactly the run it produced then: no row, no hold, and a
        // rendered task byte-identical to the one it rendered before.
        if releases.targets.is_empty() {
            return Resolution::NothingToAwait;
        }
        let identity = releases.identity.clone();
        // A repository declaring no target that answers to this name leaves the
        // cell empty rather than the row absent: a worker still needs to see the
        // dependency, and a `published` node still waits — an unanswerable
        // question is not an answer that the release has happened.
        let selected = releases.select(named.as_ref()).ok();
        let (target, style, action) = match selected {
            Some(target) => (
                Some(target.name.clone()),
                Some(target.style()),
                target.action().map(str::to_owned),
            ),
            None => (named, None, None),
        };
        Resolution::Outside(Dependency {
            dep: dep.to_owned(),
            identity,
            branch,
            commit,
            target,
            style,
            action,
        })
    }
}

/// What one `deps` entry turned out to be.
///
// llmlint: ignore[changed_behavior_has_e2e] [`Unreadable`](Resolution::Unreadable) is not
// reachable from a run, and the graph is why: this is only ever asked about a node whose
// dependencies have all settled `done`, and each of the three ways a dependency can be
// unreadable stops that happening. A dep that is not in the graph is refused by
// `graph::validate`; a dep whose repository `onevcs` cannot answer for is a node whose own
// session could not open, so it settles `failed` and its consumer stays blocked; and a
// cross-DAG dep whose upstream ledger cannot be read is an edge that does not resolve, so
// its consumer stays blocked too. What the arm does — leave the set unfrozen and ask
// again next pass — is what keeps a node from launching against a row that is missing.
enum Resolution {
    /// There is no release to wait for: it lands in this node's own repository,
    /// in none at all, or in one that declares no release targets.
    NothingToAwait,
    /// It lands elsewhere, and this is what the run can say about it.
    Outside(Dependency),
    /// The run cannot say yet.
    Unreadable,
}

/// One release a node was waiting on, as the note and the event name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Released {
    /// The repository identity that released it.
    pub identity: String,
    /// The target that carries the work.
    pub target: String,
    /// The version it arrived at.
    pub version: String,
}

impl Released {
    /// The payload entry this release is recorded as.
    pub(crate) fn payload(&self) -> Value {
        json!({"identity": self.identity, "target": self.target, "version": self.version})
    }

    /// This release, as the note names it.
    fn line(&self) -> String {
        format!("- {} — {} {}", self.identity, self.target, self.version)
    }

    /// The releases an event payload recorded, read back.
    ///
    /// A journal is external input like any other — a record this build reads may
    /// have been written by a different one, and it is a file on disk either way —
    /// so every field is checked where it is read and an entry that fails is
    /// **skipped**, exactly as every other reader of a journal skips a record it
    /// cannot read. What is at stake is the rendering: all three land in a
    /// worker's own task, and one carrying a control character forges a line
    /// there.
    pub(crate) fn of_payload(payload: &Value) -> Vec<Self> {
        payload
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| {
                let field = |key: &str| renderable(entry.get(key)?.as_str()?);
                Some(Self {
                    identity: field("identity")?,
                    // The sibling's own conversion, which is the one thing that
                    // decides what may spell a release target: a replayed name
                    // this host would refuse to configure is refused here too.
                    target: entry
                        .get("target")?
                        .as_str()?
                        .parse::<TargetName>()
                        .ok()?
                        .to_string(),
                    version: field("version")?,
                })
            })
            .collect()
    }
}

/// One replayed field, held to what it will be rendered as.
///
/// The same rule `src/vcs.rs` holds a session token and a branch name to, and for
/// the same reason: these are printed into a task, a surface, and an event
/// payload, so a value carrying whitespace or a control character renders as
/// something other than what it is wherever it lands. `None` for one that would.
fn renderable(value: &str) -> Option<String> {
    if value.is_empty() || value.len() >= crate::event::MAX_PAYLOAD_TEXT_BYTES {
        return None;
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    Some(value.to_owned())
}

/// Why one fast-adoption node's change request is opened as a **draft**, asked of
/// `onevcs` at the moment the publication is composed.
///
/// The rows are the node's own reference block, so what is asked about is exactly
/// what the worker was told to pin against. `None` is a node with nothing to wait
/// for.
///
/// **Asked here rather than off the reconcile loop's [`Watch`]** because a
/// dispatch is long and a release can arrive in the middle of one: the question is
/// whether the pin is still temporary *now*.
///
/// A row this run cannot put a question about is **not** drafted against — the
/// judgement [`Dependency::askable`] already makes for the hold — because a draft
/// nothing can ever lift is a change request nobody can finish.
pub(crate) fn draft_reason(references: &[CrossRepoReference]) -> Option<onevcs::DraftReason> {
    // llmlint: ignore-block[changed_behavior_has_e2e] no plan reaches the dropped
    // half of `askable`: a row is only composed for a dependency that settled `done`
    // in a repository declaring release targets, so it has the branch or the landing
    // commit the run recorded, and its target cell is either one that repository
    // declares or the one the plan's own `consumes` named — both of which parse.
    // What the arm does is leave such a row undrafted, which is this crate behaving
    // exactly as it did before drafting existed.
    let unreleased: Vec<(&CrossRepoReference, &str, TargetName)> = references
        .iter()
        .filter_map(|row| {
            let (reference, target) = askable(row)?;
            Some((row, reference, target))
        }) // llmlint: ignore-end[changed_behavior_has_e2e]
        .filter(|(_, reference, target)| !released_already(reference, target))
        .collect();
    let (row, reference, target) = unreleased.first()?;
    // One reason names one dependency, because [`onevcs::DraftReason`] carries one
    // target — so how many are being waited on is said in the sentence a person
    // reads rather than left for them to count.
    Some(onevcs::DraftReason {
        because: format!(
            "this node adopted {identity} early and is pinned to {reference} rather than to a \
             released version; it is one of {count} release(s) this node adopted early, and \
             landing now would make that pin permanent",
            identity = row.repository,
            count = unreleased.len(),
        ),
        awaiting: row.repository.clone(),
        target: target.clone(),
        reference: (*reference).to_owned(),
    })
}

/// What a draft reason names a settlement by, on the one line a settlement holds.
///
/// Three of the reason's four fields — the target, the repository, and the
/// reference — read off the value the publication was made with, so the settlement
/// cannot come to name a different release from the one the host is holding the
/// change for. `because` is not among them: it is the sentence composed for a
/// person reading the change request's own record, and a settlement's detail is
/// read on one line beside an outcome word that has already said the node is a
/// draft. Rendered here rather than in `src/lifecycle.rs` so there is one
/// spelling of it.
pub(crate) fn drafted_detail(reason: &onevcs::DraftReason) -> String {
    format!(
        "complete, and held as a draft: awaiting the {target} release of {awaiting}, pinned \
         to {reference} until it arrives",
        target = reason.target,
        awaiting = reason.awaiting,
        reference = reason.reference,
    )
}

/// The pair a reference row can be asked about: what the sibling resolves the
/// work by, and which of that repository's targets carries it.
///
/// Both cells are empty where the run could not name one — the block renders an
/// empty cell rather than dropping the row — so this is where a row that cannot
/// be a question is told from one that can.
fn askable(row: &CrossRepoReference) -> Option<(&str, TargetName)> {
    let reference = [row.branch.as_str(), row.commit.as_str()]
        .into_iter()
        .find(|value| renderable(value).is_some())?;
    // The sibling's own conversion, which is the one thing that decides what may
    // spell a release target — the same rule [`Released::of_payload`] reads a
    // replayed one back through.
    Some((reference, row.release_target.parse::<TargetName>().ok()?))
}

/// Whether the release one reference row awaits has already happened.
///
/// [`Answer::of`] and no second reading of the sibling: *released* is the one
/// answer that means the pin can go, and every other — not released, awaiting a
/// person, a probe that could not answer — leaves the change a draft. "Not
/// answered" is never "not released" here either; what it is is a change that
/// must not land yet, which is the safe direction and the only one.
fn released_already(reference: &str, target: &TargetName) -> bool {
    Answer::of(&onevcs::release_status(reference, Some(target)))
        .version()
        .is_some()
}

/// The note a fast-adoption node is sent when the releases it was waiting on
/// arrive.
///
/// It **adds no bar**: it reports observed state and says what to do with it, in
/// the same frame a carried planner note is rendered in, so no worker can read it
/// as a new acceptance criterion. One function, called both where the note is
/// delivered and where a journalled delivery is folded back, so a note replayed
/// from the record is the note that was sent.
pub(crate) fn arrival_note(released: &[Released]) -> String {
    format!(
        "The releases this node was waiting on have arrived:\n\n{}\n\nMove from the git pin to \
         that released version.",
        released
            .iter()
            .map(Released::line)
            .collect::<Vec<String>>()
            .join("\n"),
    )
}

/// The distinct questions one pass has to put, and every wait each one answers.
///
/// [`onevcs::release_status`] is answered by the reference and the target alone,
/// so two waits naming the same pair are asking the **identical** question: it is
/// put once and its answer belongs to both. Put once per waiting node instead,
/// each copy runs the probe subprocess again on every poll and the last node in
/// the list waits out every probe before it — which is how it comes to read as a
/// release nothing has answered while a node beside it reads as answered.
///
/// A wait with nothing to ask — no reference the sibling resolves work by, or no
/// target that answers — puts no question and joins none. The wait stands: an
/// unanswerable question is not an answer that the release has happened.
fn questions_of(waits: &[(Key, Dependency)]) -> Vec<Question> {
    let mut questions: Vec<Question> = Vec::new();
    // Where the question about one release already stands, so the next wait
    // naming it joins that one. Keyed by everything the answer depends on — the
    // reference and the target — with the style beside them, so a pairing this
    // crate has not foreseen joins nothing rather than taking an answer obtained
    // another way.
    let mut asked: BTreeMap<(&str, Option<&TargetName>, &'static str), usize> = BTreeMap::new();
    for (key, dependency) in waits {
        let Some(reference) = dependency.reference() else {
            continue;
        };
        let Some(style) = dependency.style else {
            continue;
        };
        let about = (reference, dependency.target.as_ref(), style.as_str());
        match asked.get(&about) {
            Some(&already) => questions[already].keys.push(key.clone()),
            None => {
                asked.insert(about, questions.len());
                questions.push(Question {
                    keys: vec![key.clone()],
                    reference: reference.to_owned(),
                    target: dependency.target.clone(),
                    style,
                });
            }
        }
    }
    questions
}

/// The nodes whose releases matter on this pass.
///
/// Every node that is ready to start — which is where a hold applies and where a
/// dispatch's reference block is composed — every fast-adoption node still
/// running, which is where an arrival note is delivered, and every node whose
/// change is **held as a draft**, which is where the release that lifts it is
/// still being watched for. The third is the one that matters after a node has
/// stopped running: a draft nobody goes on asking about is a change request that
/// never becomes ready.
pub(crate) fn watching(
    state: &RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    running: &BTreeSet<String>,
) -> Vec<Node> {
    state
        .graph
        .iter()
        .filter(|node| {
            matches!(
                statuses.get(&node.id),
                Some(&NodeStatus::Ready | &NodeStatus::CompleteDraft)
            ) || running.contains(&node.id)
        })
        .cloned()
        .collect()
}

/// Another run's folded state, for a cross-DAG dependency.
fn upstream_of(paths: &RunPaths, reference: &crate::crossdag::Reference) -> Option<RunState> {
    let root = paths.dir.parent()?;
    let upstream = RunPaths::under(root, &reference.run);
    if !upstream.exists() {
        return None;
    }
    Some(crate::projection::fold(&journal::read(&upstream.journal())))
}

/// How often an automated target's probe is run.
///
/// An unusable value falls back to the default rather than to zero, which would
/// spend the host on probes, or to no bound at all, which would ask once and
/// never again.
fn poll_seconds() -> u64 {
    std::env::var(POLL_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_POLL_SECONDS)
}

/// How often a held node's wait is surfaced.
fn surface_every_seconds() -> u64 {
    std::env::var(SURFACE_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_SURFACE_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use onevcs::Baseline;

    fn dependency(target: Option<&str>, style: Option<ReleaseStyle>) -> Dependency {
        Dependency {
            dep: "engine".to_owned(),
            identity: "github.com/owner/engine".to_owned(),
            branch: Some("onevcs/s-1".to_owned()),
            commit: Some("9f3c1ab".to_owned()),
            target: target.map(|name| name.parse().expect("a target name")),
            style,
            action: style
                .filter(|style| *style == ReleaseStyle::HumanStep)
                .map(|_| "cut a release on PyPI".to_owned()),
        }
    }

    /// Every answer `onevcs` can give, and the refusal it can give instead, read
    /// as this crate's own vocabulary.
    ///
    /// Arm by arm, because the whole design rests on three of them staying
    /// apart: "awaiting a human step" is neither of its neighbours, and neither a
    /// probe that failed nor a question that could not be *put* is ever "not
    /// released".
    #[test]
    fn no_answer_the_sibling_gives_is_folded_into_another() {
        let cases: Vec<(onevcs::Result<ReleaseStatus>, &str)> = vec![
            (
                Ok(ReleaseStatus::Released {
                    target: "crate".parse().expect("a target name"),
                    style: ReleaseStyle::Automated,
                    version: "0.2.0".to_owned(),
                }),
                "released",
            ),
            (
                Ok(ReleaseStatus::NotReleased {
                    at_landing: Baseline::At {
                        version: "0.1.0".to_owned(),
                    },
                    now: "0.1.0".to_owned(),
                }),
                "not-released",
            ),
            (
                Ok(ReleaseStatus::AwaitingHumanStep {
                    target: "wheel".parse().expect("a target name"),
                    action: "cut a release on PyPI".to_owned(),
                    since: "2026-08-24T00:00:00.000Z".to_owned(),
                }),
                "awaiting-human-step",
            ),
            (
                Ok(ReleaseStatus::NotAnswered {
                    reason: "the probe timed out".to_owned(),
                }),
                "not-answered",
            ),
            (Ok(ReleaseStatus::NotLanded), "not-landed"),
            // A question that could not be *put* at all — the repository
            // declares no target answering to the name, or names no default.
            // Not an answer that the release has not happened.
            (
                Err(onevcs::Error::Invalid {
                    reason: "the repository declares no release targets".to_owned(),
                }),
                "not-answered",
            ),
        ];
        for (status, expected) in cases {
            assert_eq!(
                Answer::of(&status).as_str(),
                expected,
                "{status:?} was read as another answer"
            );
        }
        // Exactly one of them releases a hold, and it is the one that names a
        // version.
        assert_eq!(
            Answer::Released {
                version: "0.2.0".to_owned()
            }
            .version(),
            Some("0.2.0")
        );
        for answer in [
            Answer::NotReleased,
            Answer::AwaitingHumanStep,
            Answer::NotAnswered,
            Answer::NotLanded,
        ] {
            assert_eq!(answer.version(), None, "{answer:?} released a hold");
        }
    }

    /// The two rungs a plan and this crate own: the node's own field, and the
    /// floor beneath every rung.
    ///
    /// The two in between are `onevcs`'s, and a node held against them is
    /// `tests/e2e/adoption.rs`'s
    /// `the_adoption_mode_resolves_through_exactly_four_rungs`, which drives all
    /// four against a real host — a rung is not a value anything reports, so what
    /// it decides is whether the node is scheduled.
    #[test]
    fn the_node_rung_wins_outright_and_a_node_with_no_repository_falls_to_the_floor() {
        let stated = Node {
            id: "stated".to_owned(),
            adoption: Some(Adoption::Published),
            ..Node::default()
        };
        assert_eq!(adoption_of(&stated), Adoption::Published);
        // No repository, so no repository rung — and no way to reach the global
        // one without naming one, which is the floor.
        assert_eq!(adoption_of(&Node::default()), Adoption::Fast);
        // A repository the sibling cannot answer for is a question that was not
        // put, which is the floor too rather than a node held for ever.
        let unknown = Node {
            id: "unknown".to_owned(),
            repo: Some("no-such-repository-on-this-host".to_owned()),
            ..Node::default()
        };
        assert_eq!(adoption_of(&unknown), Adoption::Fast);
    }

    /// A cell the run cannot name is **empty**, and the row is still there.
    #[test]
    fn a_dependency_the_run_cannot_fully_name_is_rendered_with_the_cell_empty() {
        let named = dependency(Some("crate"), Some(ReleaseStyle::Automated)).row();
        assert_eq!(named.repository, "github.com/owner/engine");
        assert_eq!(named.branch, "onevcs/s-1");
        assert_eq!(named.commit, "9f3c1ab");
        assert_eq!(named.release_target, "crate");

        // A repository declaring targets but no default, asked for none: the
        // sibling names no target, so the cell is empty and the row stands.
        let mut unnamed = dependency(None, None);
        unnamed.branch = None;
        unnamed.commit = None;
        let row = unnamed.row();
        assert_eq!(row.dependency, "engine");
        assert_eq!(row.repository, "github.com/owner/engine");
        assert!(row.branch.is_empty() && row.commit.is_empty() && row.release_target.is_empty());
    }

    /// The branch is what the sibling is asked about, and the commit is the
    /// fallback — see [`Dependency::reference`].
    #[test]
    fn the_reference_the_sibling_is_asked_about_is_the_branch() {
        assert_eq!(
            dependency(Some("crate"), None).reference(),
            Some("onevcs/s-1")
        );
        let mut branchless = dependency(Some("crate"), None);
        branchless.branch = None;
        assert_eq!(branchless.reference(), Some("9f3c1ab"));
        branchless.commit = None;
        assert_eq!(branchless.reference(), None);
    }

    /// The note reports observed state and adds no bar, and round-trips through
    /// the payload it is journalled as.
    #[test]
    fn the_arrival_note_names_the_versions_and_states_no_criterion() {
        let released = vec![Released {
            identity: "github.com/nickderobertis/onevcs".to_owned(),
            target: "crate".to_owned(),
            version: "0.13.0".to_owned(),
        }];
        let note = arrival_note(&released);
        assert_eq!(
            note,
            "The releases this node was waiting on have arrived:\n\n\
             - github.com/nickderobertis/onevcs — crate 0.13.0\n\n\
             Move from the git pin to that released version."
        );
        assert!(!note.to_lowercase().contains("acceptance criteria"));
        assert!(!note.to_lowercase().contains("must"));

        let payload = json!(released.iter().map(Released::payload).collect::<Vec<_>>());
        assert_eq!(
            arrival_note(&Released::of_payload(&payload)),
            note,
            "a note replayed from the record is not the note that was sent"
        );

        // A record this build cannot read whole is skipped rather than rendered
        // with a blank where the version should be: a journal is external input,
        // and a note naming no version tells a worker nothing it can act on.
        for unreadable in [
            json!([{"identity": "a", "target": "crate"}]),
            json!([{"identity": "a", "target": "crate", "version": 13}]),
            json!([{"identity": "a", "target": "crate", "version": ""}]),
            json!([{"target": "crate", "version": "0.13.0"}]),
            json!("not a list at all"),
            // A value that would forge a line in the task it is rendered into.
            json!([{"identity": "a\n- b — c 9.9.9", "target": "crate", "version": "0.13.0"}]),
            json!([{"identity": "a", "target": "crate", "version": "0.13.0\u{7}"}]),
            json!([{"identity": "a", "target": "crate", "version": "0.13.0 and more"}]),
            // A target name the sibling's own conversion refuses.
            json!([{"identity": "a", "target": "not a target name", "version": "0.13.0"}]),
            json!([{"identity": "a", "target": "-leading", "version": "0.13.0"}]),
        ] {
            assert!(
                Released::of_payload(&unreadable).is_empty(),
                "{unreadable} was read as a release"
            );
        }
    }

    /// Which waits share a question and which do not, arm by arm. What sharing
    /// one *costs* a run is driven end to end by `tests/e2e/adoption.rs`'s
    /// `nodes_awaiting_one_release_put_one_question_and_are_answered_together`.
    #[test]
    fn waits_naming_one_release_put_one_question_between_them() {
        let wait = |node: &str, dependency: Dependency| {
            ((node.to_owned(), dependency.dep.clone()), dependency)
        };
        let automated = || dependency(Some("crate"), Some(ReleaseStyle::Automated));

        let questions = questions_of(&[
            wait("first", automated()),
            wait("second", automated()),
            wait("third", automated()),
        ]);
        assert_eq!(questions.len(), 1, "{questions:?}");
        assert_eq!(
            questions[0].keys,
            vec![
                ("first".to_owned(), "engine".to_owned()),
                ("second".to_owned(), "engine".to_owned()),
                ("third".to_owned(), "engine".to_owned()),
            ],
        );
        assert_eq!(questions[0].reference, "onevcs/s-1");
        assert_eq!(questions[0].style, ReleaseStyle::Automated);

        // One target's answer says nothing about another's.
        let mut wheel = automated();
        wheel.target = Some("wheel".parse().expect("a target name"));
        wheel.style = Some(ReleaseStyle::HumanStep);
        let questions = questions_of(&[wait("first", automated()), wait("second", wheel.clone())]);
        assert_eq!(questions.len(), 2, "{questions:?}");
        assert_eq!(questions[1].style, ReleaseStyle::HumanStep);

        // And one landing's says nothing about another's.
        let mut other_branch = automated();
        other_branch.branch = Some("onevcs/s-2".to_owned());
        let questions = questions_of(&[
            wait("first", automated()),
            wait("second", other_branch.clone()),
        ]);
        assert_eq!(questions.len(), 2, "{questions:?}");

        let mut styleless = automated();
        styleless.style = None;
        let mut referenceless = automated();
        referenceless.branch = None;
        referenceless.commit = None;
        let questions = questions_of(&[
            wait("unanswerable", styleless),
            wait("unnameable", referenceless),
            wait("asked", automated()),
        ]);
        assert_eq!(questions.len(), 1, "{questions:?}");
        assert_eq!(
            questions[0].keys,
            vec![("asked".to_owned(), "engine".to_owned())]
        );
        assert!(questions_of(&[]).is_empty());
    }

    /// The three states of a wait with no version yet, read off both places the
    /// distinction has to exist: the payload and the surface a person reads.
    #[test]
    fn a_wait_still_expecting_its_first_answer_is_not_a_probe_that_could_not_answer() {
        let mut watch = Watch::of_run(&RunPaths::under(std::path::Path::new("/nowhere"), "demo"));
        watch.dependencies.insert(
            "asked".to_owned(),
            vec![dependency(Some("crate"), Some(ReleaseStyle::Automated))],
        );
        // A target this host declares nothing for: no question can be put, so
        // nothing will ever answer it.
        watch
            .dependencies
            .insert("unanswerable".to_owned(), vec![dependency(None, None)]);

        assert_eq!(
            watch.awaiting("asked")[0]["last_answer"],
            json!("no-answer-yet"),
            "a probe that has not come back yet was reported as one that failed"
        );
        assert!(watch
            .wait_surface("asked")
            .message
            .contains("last answer: no-answer-yet"));
        assert_eq!(
            watch.awaiting("unanswerable")[0]["last_answer"],
            json!("not-answered"),
            "a question that could not be put was reported as one still in flight"
        );

        watch.answers.insert(
            ("asked".to_owned(), "engine".to_owned()),
            Answer::NotAnswered,
        );
        assert_eq!(
            watch.awaiting("asked")[0]["last_answer"],
            json!("not-answered")
        );
        watch.answers.insert(
            ("asked".to_owned(), "engine".to_owned()),
            Answer::NotReleased,
        );
        assert_eq!(
            watch.awaiting("asked")[0]["last_answer"],
            json!("not-released")
        );
    }

    /// The surface a held node raises names the **style** of each release it
    /// awaits, and a human-step wait carries the action a person needs.
    ///
    /// Read off the surface's own text, because that is the promise: a reader
    /// tells the two waits apart without opening a configuration file.
    #[test]
    fn a_wait_on_a_machine_and_a_wait_on_a_person_read_differently() {
        let mut watch = Watch::of_run(&RunPaths::under(std::path::Path::new("/nowhere"), "demo"));
        watch.dependencies.insert(
            "auto".to_owned(),
            vec![dependency(Some("crate"), Some(ReleaseStyle::Automated))],
        );
        watch.dependencies.insert(
            "person".to_owned(),
            vec![dependency(Some("wheel"), Some(ReleaseStyle::HumanStep))],
        );
        watch.answers.insert(
            ("auto".to_owned(), "engine".to_owned()),
            Answer::NotReleased,
        );
        watch.answers.insert(
            ("person".to_owned(), "engine".to_owned()),
            Answer::AwaitingHumanStep,
        );

        let automated = watch.wait_surface("auto").message;
        assert!(
            automated.contains("automated release") && !automated.contains("human-step"),
            "{automated}"
        );
        assert!(
            automated.contains("last answer: not-released"),
            "{automated}"
        );
        let person = watch.wait_surface("person").message;
        assert!(
            person.contains("human-step release — a person has to: cut a release on PyPI"),
            "{person}"
        );
        assert!(
            person.contains("last answer: awaiting-human-step"),
            "a wait on a person read as a probe that failed: {person}"
        );
        // Neither is a decision point: the hold is the scheduler's.
        for surface in [watch.wait_surface("auto"), watch.wait_surface("person")] {
            assert!(!surface.blocking, "a release wait held a subtree twice");
            assert_eq!(surface.kind, WAIT_SURFACE_KIND);
        }

        // The payload carries the same distinction, so the surface is not the
        // only place it exists.
        let entries = watch.awaiting("person");
        assert_eq!(entries[0]["style"], json!("human-step"));
        assert_eq!(entries[0]["last_answer"], json!("awaiting-human-step"));
        assert_eq!(entries[0]["action"], json!("cut a release on PyPI"));
        assert!(watch.awaiting("auto")[0].get("action").is_none());
    }

    /// Only an answer of released lets a `published` node start, and a node the
    /// run cannot name a dependency for is not held at all.
    #[test]
    fn nothing_but_released_releases_a_hold() {
        let published = Node {
            id: "held".to_owned(),
            adoption: Some(Adoption::Published),
            ..Node::default()
        };
        let mut watch = Watch::of_run(&RunPaths::under(std::path::Path::new("/nowhere"), "demo"));
        watch.dependencies.insert(
            "held".to_owned(),
            vec![dependency(Some("crate"), Some(ReleaseStyle::Automated))],
        );
        let watching = vec![published.clone()];
        // No answer at all holds it, and so does every answer but one.
        assert!(watch.held(&watching).contains("held"));
        for answer in [
            Answer::NotReleased,
            Answer::AwaitingHumanStep,
            Answer::NotAnswered,
            Answer::NotLanded,
        ] {
            watch
                .answers
                .insert(("held".to_owned(), "engine".to_owned()), answer.clone());
            assert!(
                watch.held(&watching).contains("held"),
                "{answer:?} released the hold"
            );
        }
        watch.answers.insert(
            ("held".to_owned(), "engine".to_owned()),
            Answer::Released {
                version: "0.2.0".to_owned(),
            },
        );
        assert!(watch.held(&watching).is_empty());

        // A node with nothing outside its repository is never held, whatever it
        // declares — there is no release for it to be waiting on.
        watch.dependencies.insert("held".to_owned(), Vec::new());
        watch.answers.clear();
        assert!(watch.held(&watching).is_empty());
        // Nor is a `fast` node, ever.
        let fast = Node {
            adoption: Some(Adoption::Fast),
            ..published
        };
        watch.dependencies.insert(
            "held".to_owned(),
            vec![dependency(Some("crate"), Some(ReleaseStyle::Automated))],
        );
        assert!(watch.held(&[fast]).is_empty());
    }

    /// A fresh driver takes up what its predecessor already said, so a node is
    /// told **once** across a driver that died holding it.
    ///
    /// Read out of the journal, which is the only thing that outlives a driver.
    /// Without it the node a fresh driver finds still running is told a second
    /// time — a correction the worker has already acted on, arriving again with
    /// nothing to tell it from a new one.
    ///
    /// Held here rather than by a journey, and the reason is written down in
    /// `docs/contract-divergences.md` entry 40: a journey for it has to kill a
    /// driver mid-dispatch, adopt the run, and get a *second* node told before it
    /// can assert about the first. One was written and it is green on its own —
    /// and it timed out against the 120-second deadline on three of four runs of
    /// the instrumented suite, while holding e2e slots the rest of it needs. A
    /// test that reports the tree's health by how loaded the host was is worse
    /// than none.
    ///
    /// What the seeding *is*, is a fold of a durable record, and that is what this
    /// drives — both directions, so it holds that the seeding **narrowed** rather
    /// than silenced. The deliveries either side of it, into a live turn and onto
    /// the next dispatch, are driven end to end in `tests/e2e/adoption.rs`.
    #[test]
    fn a_fresh_driver_takes_up_what_its_predecessor_already_said() {
        let root = std::env::temp_dir().join(format!("op-release-seed-{}", std::process::id()));
        let paths = RunPaths::under(&root, "restarted");
        std::fs::create_dir_all(&paths.dir).expect("a scratch run directory");
        let record = |kind: journal::PipelineKind, node: &str, payload: Value| {
            serde_json::json!({
                "v": 1,
                "ts": "2026-08-24T00:00:00.000Z",
                "stream": "predecessor",
                "seq": 0,
                "source": "pipeline",
                "kind": kind.as_str(),
                "labels": {"run_id": "restarted", "node": node},
                "payload": payload,
            })
            .to_string()
        };
        std::fs::write(
            paths.journal(),
            format!(
                "{}\n{}\n",
                record(
                    journal::PipelineKind::ReleaseAdopted,
                    "told",
                    json!({
                        "node": "told",
                        "delivery": "live",
                        "versions": [{
                            "identity": "github.com/owner/engine",
                            "target": "crate",
                            "version": "0.2.0"
                        }]
                    }),
                ),
                record(
                    journal::PipelineKind::ReleaseArrived,
                    "told",
                    json!({"node": "told", "dep": "engine"}),
                ),
            ),
        )
        .expect("the predecessor's journal is written");

        let mut watch = Watch::of_run(&paths);
        watch.dependencies.insert(
            "told".to_owned(),
            vec![dependency(Some("crate"), Some(ReleaseStyle::Automated))],
        );
        watch.answers.insert(
            ("told".to_owned(), "engine".to_owned()),
            Answer::Released {
                version: "0.2.0".to_owned(),
            },
        );
        let running = vec![Node {
            id: "told".to_owned(),
            adoption: Some(Adoption::Fast),
            ..Node::default()
        }];
        assert!(
            watch.ready_to_adopt(&running).is_empty(),
            "a fresh driver told a node its releases had arrived a second time"
        );
        assert!(
            watch
                .arrived
                .contains(&("told".to_owned(), "engine".to_owned())),
            "a fresh driver did not take up the arrival its predecessor reported"
        );

        // A record naming no release this build can read says nothing about what
        // its node was told, so it suppresses nothing.
        std::fs::write(
            paths.journal(),
            format!(
                "{}\n",
                record(
                    journal::PipelineKind::ReleaseAdopted,
                    "told",
                    json!({"node": "told", "delivery": "live", "versions": [{"identity": ""}]}),
                ),
            ),
        )
        .expect("the predecessor's journal is written");
        assert!(
            Watch::of_run(&paths).adopted.is_empty(),
            "an unreadable record suppressed a delivery nothing can say happened"
        );

        // A node its predecessor never told is told, which is what says the
        // seeding narrowed rather than silenced.
        let fresh = vec![Node {
            id: "fresh".to_owned(),
            adoption: Some(Adoption::Fast),
            ..Node::default()
        }];
        watch.dependencies.insert(
            "fresh".to_owned(),
            vec![dependency(Some("crate"), Some(ReleaseStyle::Automated))],
        );
        watch.answers.insert(
            ("fresh".to_owned(), "engine".to_owned()),
            Answer::Released {
                version: "0.2.0".to_owned(),
            },
        );
        assert_eq!(watch.ready_to_adopt(&fresh).len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }

    /// The two bounds fall back rather than to zero or to no bound at all.
    #[test]
    fn an_unusable_bound_falls_back_to_the_shipped_one() {
        for (key, read) in [
            (POLL_ENV, poll_seconds as fn() -> u64),
            (SURFACE_ENV, surface_every_seconds as fn() -> u64),
        ] {
            let shipped = read();
            for unusable in ["0", "", "soon", "-1"] {
                std::env::set_var(key, unusable);
                assert_eq!(read(), shipped, "{key}={unusable:?}");
            }
            std::env::set_var(key, "7");
            assert_eq!(read(), 7);
            std::env::remove_var(key);
        }
        assert_eq!(poll_seconds(), DEFAULT_POLL_SECONDS);
        assert_eq!(surface_every_seconds(), DEFAULT_SURFACE_SECONDS);
    }
}
