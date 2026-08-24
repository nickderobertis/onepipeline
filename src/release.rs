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
            Ok(ReleaseStatus::Released { version, .. }) => Self::Released {
                version: version.clone(),
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

/// One question the asker puts to `onevcs`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Question {
    /// The node and dependency the answer belongs to.
    key: Key,
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
    answers: Receiver<(Key, Answer)>,
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
        let (answered, answers): (Sender<(Key, Answer)>, Receiver<(Key, Answer)>) =
            mpsc::channel();
        // Detached: it holds no run state, writes nothing, and ends when the
        // reconcile loop drops its end of `questions`.
        std::thread::Builder::new()
            .name("release-asker".to_owned())
            .spawn(move || ask_until_dropped(&asked, &answered, poll))
            .map(|handle| drop(handle))
            .unwrap_or_else(|error| {
                // A thread this host would not start is reported and nothing
                // more: every answer then stays unarrived, which holds a
                // published node exactly as an unanswered probe does and leaves
                // a fast node with the git pin it launched under. Neither is a
                // node failed for a reason that is not about the node.
                eprintln!("onepipeline: cannot start the release watch: {error}");
            });
        Self { questions, answers }
    }

    /// Hand over the questions to ask from now on.
    fn ask(&self, questions: Vec<Question>) {
        // A dead asker is not an error here for the reason its refusal to start
        // is not: it leaves every answer unarrived, which is the safe direction.
        let _ = self.questions.send(questions);
    }

    /// Everything answered since the last look. Never blocks.
    fn answered(&self) -> Vec<(Key, Answer)> {
        self.answers.try_iter().collect()
    }
}

/// The asker's own loop.
fn ask_until_dropped(
    asked: &Receiver<Vec<Question>>,
    answered: &Sender<(Key, Answer)>,
    poll: Duration,
) {
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
            if answered.send((question.key.clone(), answer)).is_err() {
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
    asker: Asker,
}

impl Watch {
    /// Start watching one run, taking up whatever a previous driver said.
    pub(crate) fn of_run(paths: &RunPaths) -> Self {
        let mut adopted = BTreeSet::new();
        let mut arrived = BTreeSet::new();
        for event in journal::read(&paths.journal()) {
            let node = event.labels.node.clone().unwrap_or_default();
            match journal::PipelineKind::from_wire(&event.kind) {
                Some(journal::PipelineKind::ReleaseAdopted) => {
                    adopted.insert(node);
                }
                Some(journal::PipelineKind::ReleaseArrived) => {
                    if let Some(dep) = event.payload.get("dep").and_then(Value::as_str) {
                        arrived.insert((node, dep.to_owned()));
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
            asker: Asker::start(Duration::from_secs(poll_seconds())),
        }
    }

    /// The rows a node's dispatch is handed, in the order its `deps` name them.
    pub(crate) fn references(&self, node: &str) -> Vec<CrossRepoReference> {
        self.dependencies
            .get(node)
            .map(|dependencies| dependencies.iter().map(Dependency::row).collect())
            .unwrap_or_default()
    }

    /// Take up the answers that have arrived, and ask about what is awaited now.
    ///
    /// `watching` is the nodes whose releases matter this pass: every node that
    /// is ready to start, and every fast-adoption node still running. Neither
    /// blocks on anything.
    pub(crate) fn refresh(&mut self, paths: &RunPaths, state: &RunState, watching: &[Node]) {
        for (key, answer) in self.asker.answered() {
            self.answers.insert(key, answer);
        }
        let now = crate::sys::now_millis();
        let mut questions: Vec<Question> = Vec::new();
        for node in watching {
            let dependencies = self.resolve(paths, state, node);
            for dependency in dependencies {
                let key = (node.id.clone(), dependency.dep.clone());
                self.since.entry(key.clone()).or_insert(now);
                let Some(reference) = dependency.reference() else {
                    continue;
                };
                let Some(style) = dependency.style else {
                    // The repository declares no target that answers, so there is
                    // nothing to ask. The wait stands: an unanswerable question is
                    // not an answer that the release has happened.
                    continue;
                };
                questions.push(Question {
                    key,
                    reference: reference.to_owned(),
                    target: dependency.target.clone(),
                    style,
                });
            }
        }
        self.asker.ask(questions);
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
                        ("target", json!(dependency.target.as_ref().map(ToString::to_string))),
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
            let awaiting = self.awaiting(node);
            journal.emit(
                journal::PipelineKind::ReleaseWait,
                journal::labels(&paths.run, Some(node)),
                journal::payload(&[("node", json!(node)), ("awaiting", json!(awaiting))]),
            )?;
            crate::engine::raise(paths, journal, self.wait_surface(node))?;
        }
        // A node that is no longer held says nothing more: the arrival is
        // reported by `release-arrived`, and repeating the wait after it ended
        // would report a run as waiting on something it has.
        self.surfaced.retain(|node, _| held.contains(node));
        Ok(())
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
                entry.insert("since".to_owned(), json!(crate::sys::rfc3339_from_millis(since)));
                entry.insert(
                    "waited_seconds".to_owned(),
                    json!(now.saturating_sub(since) / 1_000),
                );
                entry.insert(
                    "last_answer".to_owned(),
                    json!(self
                        .answers
                        .get(&key)
                        .map_or(Answer::NotAnswered.as_str(), Answer::as_str)),
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
        for dependency in self.dependencies.get(node).map(Vec::as_slice).unwrap_or(&[]) {
            let key = (node.to_owned(), dependency.dep.clone());
            if self.answers.get(&key).and_then(Answer::version).is_some() {
                continue;
            }
            let waited = crate::telemetry::duration(
                now.saturating_sub(self.since.get(&key).copied().unwrap_or(now)),
            );
            let answered = self
                .answers
                .get(&key)
                .map_or(Answer::NotAnswered.as_str(), Answer::as_str);
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
    pub(crate) fn ready_to_adopt(&self, running: &[String]) -> Vec<(String, Vec<Released>)> {
        running
            .iter()
            .filter(|node| !self.adopted.contains(*node))
            .filter(|node| self.all_released(node))
            .map(|node| (node.clone(), self.released(node)))
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
    pub(crate) fn of_payload(payload: &Value) -> Vec<Self> {
        payload
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|entry| {
                let field = |key: &str| {
                    entry
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                Self {
                    identity: field("identity"),
                    target: field("target"),
                    version: field("version"),
                }
            })
            .collect()
    }
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

/// The nodes whose releases matter on this pass.
///
/// Every node that is ready to start — which is where a hold applies and where a
/// dispatch's reference block is composed — and every fast-adoption node still
/// running, which is where an arrival note is delivered.
pub(crate) fn watching(
    state: &RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    running: &BTreeSet<String>,
) -> Vec<Node> {
    state
        .graph
        .iter()
        .filter(|node| {
            statuses.get(&node.id) == Some(&NodeStatus::Ready) || running.contains(&node.id)
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
