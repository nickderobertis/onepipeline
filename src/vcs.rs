//! The `onevcs` seam.
//!
//! Repository identities, sessions, preserved work, and publication stay in that
//! library. A lifecycle node is this crate opening a session there, running its
//! dispatches inside the worktree that session hands back, and publishing
//! through it — never re-deriving a branch name, a merge policy, or a gate.
//!
//! The machine running the dispatch is the one that opens the session, which is
//! what [`WorkspaceSpec::VcsSession`](crate::executor::WorkspaceSpec::VcsSession)
//! means: the clone, worktree, and branch are cut where the work happens.
//!
//! # Reached by calling it, never by spawning it
//!
//! All four operations this crate performs are `onevcs` **library** calls:
//! [`onevcs::Vcs::open_session`], [`onevcs::publish`], [`onevcs::close_session`], and
//! [`EventStream`]. No process is started and no output is parsed, and the
//! values that come back are the sibling's own types rather than a restatement
//! of them here.
//!
//! That is not only about process cost. `onevcs publish` answers a *person* with
//! one line of English — `merged at SHA`, `change request open at URL` — and
//! this crate used to read that line as JSON, so against the real sibling every
//! publication failed as unreadable while the suite stayed green against a
//! double that printed the JSON the parser wanted. A [`Publication`] cannot be
//! misread that way: what the publication did is a case of [`PublishOutcome`],
//! and the compiler checks every reader of it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use onevcs::{
    EventStream, Lifecycle, MergePolicy, Providers, Publication, PublishOutcome, PublishRequest,
    Session, SessionRequest, SessionToken, Subject,
};

use crate::error::{Error, Result};
use crate::event::Envelope;
use crate::filter::EventFilter;

fn sibling(message: impl Into<String>) -> Error {
    Error::Sibling {
        tool: "onevcs",
        message: message.into(),
    }
}

/// A refusal from the sibling, as this crate's own error.
fn refusal(error: onevcs::Error) -> Error {
    sibling(error.to_string())
}

/// Git and GitHub — what every operation here runs against.
///
/// [`Providers::real`] rather than a value held on this module: both defaults are
/// stateless and the sibling hands out one shared pair per process, so there is
/// nothing to keep.
fn providers() -> Providers<'static> {
    Providers::real()
}

/// Open a session over a per-run clone and worktree.
pub fn session_open(request: &SessionRequest) -> Result<Session> {
    providers()
        .vcs
        .open_session(request.clone())
        .map_err(refusal)
}

/// Verify a session's work and publish it under its policy.
///
/// The answer is [`Publication`] — the sibling's own value — so *what happened*
/// is a case to match on rather than a sentence to read. A publication that did
/// not land is [`PublishOutcome::Failed`] and not an `Err`: the sibling draws the
/// line between a refused request and a publication that ran and did not land,
/// and this crate reads its line rather than a second one.
///
/// The title is checked here, where the request is built, because
/// [`Subject`]'s conversion is where the sibling checks it — a title too long to
/// be a commit subject is refused before a session's work is committed rather
/// than after. A title of `None` is the plan stating none: the sibling then
/// derives the subject from the branch's own conventional commits, which is a
/// better subject than anything this crate could compose about work it did not
/// do.
///
/// The body crosses as the prose it is. Nothing checks it, here or there — a
/// host places no shape on a change request's body, so there is no rule to hold
/// it to and inventing one would refuse a body the host would have taken. What
/// it is *not* is a node's `task`: that is the brief its agent was given, not a
/// description of what the branch turned out to hold, so a body is one that was
/// drafted from the diff or there is none.
pub fn publish(
    token: &SessionToken,
    policy: Option<MergePolicy>,
    title: Option<&str>,
    body: Option<&str>,
) -> Result<Publication> {
    let title = title
        .map(|title| title.parse::<Subject>().map_err(sibling))
        .transpose()?;
    onevcs::publish(
        &providers(),
        token,
        &PublishRequest {
            policy,
            title,
            body: body.map(str::to_owned),
        },
    )
    .map_err(refusal)
}

/// How a publication settles the node that made it.
///
/// This crate's own outcome vocabulary, which a plan's readers and `results`
/// render: `no-changes` is the name a node whose steps all declared no diff
/// already settles on, so a publication with nothing to publish reads the same
/// way rather than inventing a second word for it.
pub fn outcome_of(outcome: &PublishOutcome) -> &'static str {
    match outcome {
        PublishOutcome::Merged(_) => "merged",
        PublishOutcome::ChangeOpen(_) => "change-open",
        PublishOutcome::Queued(_) => "queued",
        PublishOutcome::NothingToPublish => "no-changes",
        PublishOutcome::Failed { .. } => "publication-failed",
    }
}

/// Whether the publication's change reached its base branch.
///
/// Read off **what the publication answered**, and off nothing else. The
/// repository's policy is not consulted here and must not be: a `change-direct`
/// or `change-auto` identity asks the host to land the change immediately, and
/// whether it did is the host's answer rather than the ask — a required check
/// still running, a branch protection rule, or a merge queue all leave the same
/// policy sitting at [`PublishOutcome::Queued`]. Deriving "landed" from the
/// policy would report exactly the state this distinction exists to expose.
///
/// [`PublishOutcome::Merged`] is the one landed case, and it is an observation:
/// `onevcs` produces it holding the commit the change reached its base at, from
/// git on the direct path and from the host's own answer on the change-request
/// one.
///
/// `None` where the node has no change of its own to land.
/// [`PublishOutcome::NothingToPublish`] is a branch whose base already carried
/// its content. [`PublishOutcome::Failed`] is here for totality rather than for
/// use: `crate::lifecycle` settles that case before it asks, under its own
/// `failed` status — which no reader mistakes for success, so qualifying it would
/// put a second word on a fact already stated. Both answer `None`, so the arm and
/// the early return agree if that ever changes.
///
/// Nothing here waits. A change request a person has to merge is reported as
/// unlanded and the round moves on; the run neither blocks nor polls for a merge
/// somebody else owns.
pub fn landing_of(outcome: &PublishOutcome) -> Option<crate::graph::Landing> {
    use crate::graph::Landing;
    match outcome {
        PublishOutcome::Merged(_) => Some(Landing::Landed),
        PublishOutcome::ChangeOpen(_) | PublishOutcome::Queued(_) => Some(Landing::Unlanded),
        PublishOutcome::NothingToPublish | PublishOutcome::Failed { .. } => None,
    }
}

/// Where a human reads the change a publication produced, when there is one.
///
/// A change request that is open, or that the host is holding, names its URL. A
/// `local-direct` merge has no change request at all, and a change request the
/// *host* merged is [`PublishOutcome::Merged`], which carries the commit rather
/// than the URL — see the `onevcs` proposal in `docs/contract-divergences.md`.
pub fn change_url(outcome: &PublishOutcome) -> Option<String> {
    match outcome {
        PublishOutcome::ChangeOpen(url) | PublishOutcome::Queued(url) => Some(url.to_string()),
        _ => None,
    }
}

/// One read of an open session's record: where it is being worked in, and what
/// its branch is measured against.
///
/// What the second and later dispatches of one lifecycle node run in. They must
/// **not** open a session of their own: `onevcs` cuts each session its own clone
/// from the execution checkout, so a second one carries none of the first's
/// uncommitted work — and opening it reclaims the first's workspace outright,
/// because a run root whose branch holds no commit the origin lacks is one the
/// sibling reads as abandoned. Both are recorded in
/// `docs/contract-divergences.md`.
///
/// A read, not a claim: [`onevcs::session`] takes no lease, commits nothing, and
/// reclaims nothing, so asking where a session is working cannot disturb it —
/// unlike `adopt`, which commits whatever the worktree holds behind an
/// incomplete-step marker.
///
/// `None` when the record cannot be read, which leaves the caller to open a
/// session as it would have.
pub fn workspace_of(token: &SessionToken) -> Option<Workspace> {
    onevcs::session(&providers(), token)
        .map(|record| Workspace {
            worktree: record.session.worktree,
            base: record.session.base,
        })
        .map_err(|error| {
            eprintln!(
                "onepipeline: cannot read session {}'s record: {error}",
                token.0
            );
            error
        })
        .ok()
}

/// What an open session's own record says about where its work is happening.
///
/// Both answers from one read, because they come from one record and are wanted
/// at one moment: a second read at publication would ask the sibling a question
/// it has already answered, and its failure would be a path nothing can reach —
/// a publication that succeeded is a record that was readable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The worktree the session's dispatches work in.
    pub worktree: std::path::PathBuf,
    /// The base its branch was cut from, and is compared against.
    ///
    /// The sibling's answer rather than the plan's: a node naming no base takes
    /// the identity's default, which this crate never sees.
    pub base: String,
}

/// Release a session's worktree and its occupancy lease.
///
/// Closing is best-effort on the failure path: a node that already failed must
/// not be reported as a different failure because its cleanup also failed.
pub fn session_close(token: &SessionToken) -> Result<Session> {
    onevcs::close_session(&providers(), token).map_err(refusal)
}

/// The change request one session's work reached, when it reached one.
///
/// Read off the session's **own stream**, which is where `onevcs` records a
/// change request as it opens one — `change-opened` carries the URL. That is the
/// only source this crate may ask: which host answers for a repository, and how
/// a change request is addressed on it, are that library's business, and a
/// second route to the same fact here would be host knowledge regrown in the
/// composition layer.
///
/// It matters because the engine is not the only thing that publishes from a
/// session: a dispatch that runs `onevcs publish` in its own final turn opens a
/// change request the engine's publication step never ran, and the record of it
/// is on this stream either way.
///
/// The URL is **validated where it enters**, through the parser `onevcs`
/// re-exports for exactly this — a session's stream is a file on disk that any
/// process holding the token appends to, so its payload is external input here
/// however trusted its usual writer is. A value that is not an absolute URL is
/// no change request a reviewer can open, and putting one on a settlement would
/// hand every reader of that node something to follow that goes nowhere.
///
/// `None` when nothing opened one, when the record names no readable URL, and
/// equally when the stream cannot be read — the caller settles exactly as it
/// would have, because an unreadable record is not evidence of a change nobody
/// opened.
pub fn change_opened_in(token: &SessionToken) -> Option<String> {
    let opened = kind_of(onevcs::EventKind::ChangeOpened);
    // The last one wins: a session that opened a change request, closed it, and
    // opened another names the one it ended with.
    events(token, None)
        .iter()
        .rev()
        .find(|envelope| envelope.kind == opened)
        .and_then(|envelope| envelope.payload.get("url"))
        .and_then(|url| url.as_str())
        // llmlint: ignore-block[changed_behavior_has_e2e] no invocation a user can type
        // reaches the refusal this line makes. The only producer of a `change-opened` is
        // `onevcs`, which builds the payload from its own `Url` — so a record naming
        // something that is not one can only come from a stream a hand-written line was
        // appended to, and writing that line would make this suite an oracle for a
        // payload nothing produces, which is the weakness `crates/testfakes` exists to
        // avoid. The two answers a producer *can* give are both driven end to end in
        // `tests/e2e/lifecycle.rs`: a change request that was opened, and a stream this
        // build cannot read a record off at all.
        .and_then(|url| onevcs::Url::parse(url.trim()).ok())
        // llmlint: ignore-end[changed_behavior_has_e2e]
        .map(|url| url.to_string())
}

/// The sessions holding one repository's workspaces, as `onevcs` reports them.
///
/// The same enumeration the launch interlock reads, asked of one repository:
/// what a node waiting to dispatch into an occupied workspace is waiting for.
///
/// An empty list is a workspace nothing holds. A repository this host cannot
/// answer for is the **error**, and the two are deliberately not the same value:
/// a view never reports an unmeasured thing as a measured nothing, and rendering
/// "nobody could be asked" as "nothing holds it" is what would tell a supervisor
/// to stop looking for what a node is waiting on. The caller renders the
/// refusal rather than deciding for itself what it meant.
pub fn holders_of(repo: &str) -> std::result::Result<Vec<onevcs::SessionHolder>, String> {
    onevcs::session_holders(repo).map_err(|error| {
        eprintln!("onepipeline: cannot read the session holders of {repo}: {error}");
        error.to_string()
    })
}

/// One session's stream, read from the start of what this reader has not seen.
///
/// `None` when the stream cannot be opened or a line of it cannot be read. That
/// is a publication with no *evidence* rather than a publication that did not
/// happen — the node's own settlement stands — but a silent gap in the merged
/// store is what makes a later reader think nothing happened, so it is said out
/// loud.
fn opened(token: &SessionToken, filter: Option<&EventFilter>) -> Option<EventStream> {
    let filter = match filter.map(sibling_filter).transpose() {
        Ok(filter) => filter.unwrap_or_default(),
        Err(error) => {
            eprintln!(
                "onepipeline: cannot follow session {}'s events: {error}",
                token.0
            );
            return None;
        }
    };
    match EventStream::open_filtered(token, filter) {
        Ok(stream) => Some(stream),
        Err(error) => {
            eprintln!(
                "onepipeline: cannot read session {}'s events: {error}",
                token.0
            );
            None
        }
    }
}

/// The next batch of a stream, relayed into this crate's envelope.
///
/// [`EventStream::read`] refuses a whole batch over one line it cannot parse, and
/// its cursor has already moved past that line — so a refusal here is events
/// lost, not events deferred. It is reported for that reason and the follow keeps
/// reading: the alternative is to stop relaying a live publication over one
/// record.
fn next_batch(stream: &mut EventStream, token: &SessionToken) -> Vec<Envelope> {
    match stream.read() {
        Ok(events) => events.into_iter().map(relayed).collect(),
        Err(error) => {
            eprintln!(
                "onepipeline: cannot read session {}'s events: {error}",
                token.0
            );
            Vec::new()
        }
    }
}

/// A session's own event stream, for relaying into the merged one.
pub fn events(token: &SessionToken, filter: Option<&EventFilter>) -> Vec<Envelope> {
    let Some(mut stream) = opened(token, filter) else {
        return Vec::new();
    };
    next_batch(&mut stream, token)
}

/// This crate's filter, as the sibling's own type.
///
/// The filter is handed to `onevcs` as a **value** rather than as a spec each
/// source parses again, which is what its filtered constructor takes — so this
/// is the one conversion, and it crosses at the wire shape the two types share
/// by contract rather than field by field, so a field one of them grows and the
/// other has not is a refusal here rather than a value silently dropped.
fn sibling_filter(filter: &EventFilter) -> Result<onevcs::EventFilter> {
    let document = serde_json::to_string(filter).map_err(|error| Error::Sibling {
        tool: "onevcs",
        message: format!("rendering the event filter: {error}"),
    })?;
    serde_json::from_str(&document).map_err(|error| Error::Sibling {
        tool: "onevcs",
        message: format!("`onevcs` refused the event filter: {error}"),
    })
}

/// One of the sibling's envelopes, as one of this crate's.
///
/// Field for field, out of `onevcs`'s own type: the merged stream keeps a
/// relayed envelope's producer `stream`, `seq`, `source`, and kind exactly as it
/// was written, which is what lets a consumer detect loss per stream.
fn relayed(envelope: onevcs::Envelope) -> Envelope {
    Envelope {
        v: envelope.v,
        ts: envelope.ts,
        stream: envelope.stream,
        seq: envelope.seq,
        source: source_of(envelope.source),
        kind: kind_of(envelope.kind),
        labels: labels_of(envelope.labels),
        payload: envelope.payload,
        artifacts: envelope
            .artifacts
            .into_iter()
            .map(|artifact| crate::event::ArtifactRef {
                id: crate::event::ArtifactId(artifact.id.0),
                kind: artifact.kind,
                bytes: artifact.bytes,
            })
            .collect(),
    }
}

/// Which library produced a relayed envelope.
fn source_of(source: onevcs::Source) -> crate::event::Source {
    match source {
        onevcs::Source::Agentgraph => crate::event::Source::Agentgraph,
        onevcs::Source::Vcs => crate::event::Source::Vcs,
        onevcs::Source::Pipeline => crate::event::Source::Pipeline,
    }
}

/// A kind as the sibling spells it on the wire.
///
/// Through `onevcs`'s own serializer rather than an arm per variant: how a kind
/// is spelled is the sibling's to decide, and a table here would be a second
/// copy of a vocabulary this crate does not own — which is what
/// `src/AGENTS.md` forbids and what let a double script a kind the sibling has
/// never emitted.
fn kind_of(kind: onevcs::EventKind) -> crate::event::EventKind {
    let wire = serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        // `EventKind` is a fieldless enum serialized as a string, so this arm is
        // unreachable today. It carries the variant's own name rather than
        // panicking, because losing a relay thread is a worse answer than an
        // unfamiliar kind in the store.
        .unwrap_or_else(|| format!("{kind:?}"));
    crate::event::EventKind(wire)
}

/// The labels a relayed envelope arrived with.
///
/// `onevcs` names one the merged envelope does not reserve — `member` — so it
/// rides in [`Labels::extra`](crate::event::Labels::extra), which is where the
/// contract puts anything a producer stamps beyond the reserved keys. Dropping
/// it would lose a producer's own attribution in the relay.
fn labels_of(labels: onevcs::Labels) -> crate::event::Labels {
    let mut extra = labels.extra;
    if let Some(member) = labels.member {
        extra.insert("member".to_owned(), serde_json::json!(member));
    }
    crate::event::Labels {
        run_id: labels.run_id,
        round: labels.round,
        node: labels.node,
        step: labels.step,
        persona: labels.persona,
        extra,
    }
}

/// How long a follow may keep reading after its session was closed.
///
/// The reader ends itself on a session it reads as closed, so this only covers
/// the case where the close itself failed and nothing will ever mark it: a node
/// that has already settled must not hang on its own cleanup.
const FOLLOW_GRACE: Duration = Duration::from_secs(5);

/// How often a follow asks the stream for what has been appended since.
const FOLLOW_POLL: Duration = Duration::from_millis(20);

/// A session's own event stream, followed as `onevcs` writes it.
///
/// Read *once at settlement*, a lifecycle node's gate run, push, change
/// request, check polling, and merge are one opaque blocking call: every record
/// appears at once, when it is over — and that stretch is the longest
/// wall-clock segment the node has. This is the same stream read as it grows,
/// through [`EventStream`], which hands back only what has been appended since
/// the last read.
///
/// `None` when the session's stream cannot be opened at all. That is a
/// publication nobody is watching rather than a publication with no record, so
/// it is said out loud and the caller reads the stream once instead.
pub fn follow(
    token: &SessionToken,
    filter: Option<&EventFilter>,
    sink: Box<dyn Fn(Envelope) + Send>,
) -> Option<Follower> {
    let mut stream = opened(token, filter)?;

    let progress = Arc::new(Progress::default());
    let reached = Arc::clone(&progress);
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = Arc::clone(&stop);
    let followed = token.clone();
    let reader = std::thread::Builder::new()
        .name(format!("onevcs-{}-events", token.0))
        .spawn(move || loop {
            // Read *before* asking whether the session closed, so a record
            // written between the two is relayed on the next pass rather than
            // lost to a follow that stopped one read early.
            for envelope in next_batch(&mut stream, &followed) {
                reached.reached(envelope.seq);
                sink(envelope);
            }
            if stopping.load(Ordering::SeqCst) || settled(&followed) {
                return;
            }
            std::thread::sleep(FOLLOW_POLL);
        });
    match reader {
        Ok(reader) => Some(Follower {
            reader: Some(reader),
            stop,
            progress,
        }),
        Err(error) => {
            eprintln!(
                "onepipeline: cannot follow session {}'s events: {error}",
                token.0
            );
            None
        }
    }
}

/// Whether a session has been released, which is what ends a follow.
///
/// A session whose record cannot be read is treated as settled: a follow that
/// kept reading a stream nobody will ever close is a thread this process would
/// never collect.
fn settled(session: &SessionToken) -> bool {
    onevcs::session(&providers(), session)
        .map(|record| record.lifecycle == Lifecycle::Closed)
        .unwrap_or(true)
}

/// How far into a session's stream a follow got.
///
/// The `seq` rather than a count, because that is what says which records are
/// still unread: `onevcs` numbers a stream monotonically from one and resumes
/// the series in the next process that writes to it, so the highest `seq`
/// relayed is exactly the point a second reader continues from.
#[derive(Debug, Default)]
struct Progress {
    /// How many envelopes were relayed.
    count: AtomicU64,
    /// The highest `seq` among them.
    seq: AtomicU64,
}

impl Progress {
    /// Record that one envelope was relayed.
    fn reached(&self, seq: u64) {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.seq.fetch_max(seq, Ordering::SeqCst);
    }

    /// The highest `seq` relayed, or `None` if nothing was.
    ///
    /// Not a bare `0`: a producer numbering from zero would then be
    /// indistinguishable from one that produced nothing, and the caller reading
    /// on from here would skip that stream's first record.
    fn reached_through(&self) -> Option<u64> {
        (self.count.load(Ordering::SeqCst) > 0).then(|| self.seq.load(Ordering::SeqCst))
    }
}

/// One session's stream, being followed.
///
/// Dropping one ends the follow. Not every caller reaches a settlement — a node
/// whose next step needs a person holds its session *open* for them, and returns
/// — and a follow left behind there is a thread nothing would ever collect,
/// reading a stream nobody is waiting for.
#[derive(Debug)]
pub struct Follower {
    /// Taken by [`finish`](Follower::finish), so a drop after one has nothing
    /// left to wait on.
    reader: Option<std::thread::JoinHandle<()>>,
    /// Set to end the follow without waiting for the session to close.
    stop: Arc<AtomicBool>,
    progress: Arc<Progress>,
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Follower {
    /// Stop following, and say how far into the stream it got.
    ///
    /// Called *after* `session close`, which is what ends the follow: the reader
    /// relays everything appended since its last pass and only then asks whether
    /// the session closed, so waiting for it loses nothing.
    ///
    /// The answer is a **floor, never a promise that the rest is not there**.
    /// Closing a session marks the record closed and only then writes the
    /// `session-closed` event, while the follow relays what the stream holds and
    /// *then* asks whether the session closed — so a follow can end cleanly,
    /// successfully, with the last record of the session still unwritten.
    /// Treating a clean end as "everything was relayed" is what dropped that
    /// record out of the merged store; the caller reads the stream once more
    /// from this point instead.
    ///
    /// `None` when it relayed nothing at all, which is the whole stream still to
    /// read rather than a stream that held nothing.
    pub fn finish(mut self) -> Option<u64> {
        let deadline = Instant::now() + FOLLOW_GRACE;
        while self
            .reader
            .as_ref()
            .is_some_and(|reader| !reader.is_finished())
            && Instant::now() < deadline
        {
            std::thread::sleep(FOLLOW_POLL);
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.progress.reached_through()
    }
}

/// The envelope that records a session opening, for the merged stream.
///
/// It carries `Source::Vcs` because `onevcs` is what opened the session: the
/// merge is an interleaving of three streams, and a lifecycle node's branch
/// belongs to that one.
pub fn session_opened_event(session: &Session, labels: &crate::event::Labels) -> Envelope {
    Envelope {
        v: crate::event::ENVELOPE_VERSION,
        ts: crate::sys::now_rfc3339(),
        stream: format!("onevcs-{}", session.token.0),
        seq: 0,
        source: crate::event::Source::Vcs,
        // The sibling's own spelling, through its own serializer: this envelope
        // stands beside the ones `onevcs` writes for the same session, and a
        // reader that folds one of them has to fold both.
        kind: kind_of(onevcs::EventKind::SessionOpened),
        labels: labels.clone(),
        payload: crate::journal::payload(&[
            ("token", serde_json::json!(session.token.0)),
            ("branch", serde_json::json!(session.branch)),
            ("base", serde_json::json!(session.base)),
            ("worktree", serde_json::json!(session.worktree)),
        ]),
        artifacts: Vec::new(),
    }
}

/// The envelope that records a publication, for the merged stream.
///
/// Every field is read off the [`Publication`] rather than off the caller: the
/// branch that carried the change and the policy it landed under are the
/// sibling's answer, and a second copy assembled here could disagree with it.
pub fn published_event(published: &Publication, labels: &crate::event::Labels) -> Envelope {
    Envelope {
        v: crate::event::ENVELOPE_VERSION,
        ts: crate::sys::now_rfc3339(),
        stream: format!("onevcs-{}", published.branch),
        seq: 1,
        source: crate::event::Source::Vcs,
        kind: crate::event::EventKind("published".into()),
        labels: labels.clone(),
        payload: crate::journal::payload(&[
            ("branch", serde_json::json!(published.branch)),
            ("policy", serde_json::json!(published.policy)),
            ("outcome", serde_json::json!(outcome_of(&published.outcome))),
            ("url", serde_json::json!(change_url(&published.outcome))),
            (
                "landing",
                serde_json::json!(landing_of(&published.outcome).map(crate::graph::Landing::as_str)),
            ),
        ]),
        artifacts: Vec::new(),
    }
}

/// Where one node's dispatch is working: the session `onevcs` opened for it.
///
/// The token and the branch together, because either alone leaves work
/// unreachable — the branch says where the commits are and the token says which
/// worktree and clone still hold them — and both are private, so a value of this
/// type is one [`read_from`](Self::read_from) has already checked. There is no
/// other way to make one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchSession {
    token: SessionToken,
    branch: BranchName,
}

/// A branch as a session record named it, checked for what this crate does with
/// it.
///
/// Deliberately **not** a claim that git would accept the name. That parser is
/// git's — `onevcs` asks it, through a type it does not export — and asking it
/// here would mean this crate running git, which no path of it ever has. What
/// this promises is what [`usable`] checks, which is what has to be true before
/// a name is rendered into a view and handed back to the sibling as a pin.
///
/// The field is private and the only constructor is that check, so a branch this
/// crate has not looked at cannot be held in one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchName(String);

impl BranchName {
    /// The name itself, for a caller that needs the string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BranchName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl DispatchSession {
    /// Read a session out of a relayed `session-opened` payload.
    ///
    /// **This is the trust boundary for a session record**, and the envelope
    /// module says so: it declares the wire shape and its bounds and leaves the
    /// semantic checks — that a text field was whole at
    /// [`MAX_PAYLOAD_TEXT_BYTES`](crate::event::MAX_PAYLOAD_TEXT_BYTES) — to the
    /// reader seam that parses a stream, which is this.
    ///
    /// Two steps, and they answer different questions. **Which fields** is
    /// [`Session`]'s to say, so the payload is parsed through the producer's own
    /// declaration rather than by key: the shape this crate expects is then the
    /// shape the producer publishes, and a rename there is a record this build
    /// reads nothing out of instead of a key that silently stops matching. The
    /// fields it does *not* declare are left where they are on purpose — the
    /// same kind is written by `onevcs` itself, whose record carries seven more
    /// of its own, and a reader that refused them would refuse the producer's
    /// own account of the session it just opened. That tolerance is one-way: a
    /// field this crate acts on is required by the type, so a misspelled or
    /// missing `branch` drops the record rather than being half-read into a
    /// manager's only pointer at the work.
    ///
    /// **Whether the values are usable** is this crate's, because it is the one
    /// that acts on them: see [`usable`].
    pub fn read_from(payload: &serde_json::Map<String, serde_json::Value>) -> Option<Self> {
        let session: Session =
            serde_json::from_value(serde_json::Value::Object(payload.clone())).ok()?;
        Some(Self {
            token: SessionToken(usable(&session.token.0)?),
            branch: BranchName(usable(&session.branch)?),
        })
    }

    /// The handle `onevcs` addresses the session by.
    pub fn token(&self) -> &SessionToken {
        &self.token
    }

    /// The branch its worktree has checked out, and therefore the branch the
    /// dispatch's commits are on.
    pub fn branch(&self) -> &BranchName {
        &self.branch
    }
}

/// One payload text field, where it is whole and names something.
///
/// Three checks, each about what this crate does with the value. It is handed
/// back to `onevcs` as a session handle and a branch name, and it is rendered
/// into `results` and onto an operator's terminal:
///
/// * **Not empty.** A branch nobody can name is not a pointer at work, and
///   recording one would put an empty name where a manager reads for one.
/// * **Whole.** A producer cuts a payload text field at
///   [`MAX_PAYLOAD_TEXT_BYTES`](crate::event::MAX_PAYLOAD_TEXT_BYTES) and says
///   so beside it, so a value that long may be a *prefix* — and a truncated
///   branch name addresses a branch that does not exist. Checked per value
///   rather than off the payload's own marker, which says only that something in
///   the record was cut.
/// * **One word, on one line.** Neither a session token nor a branch name
///   carries whitespace or a control character, and both are rendered into
///   line-oriented views where a value carrying a newline forges a line — a
///   record that appears to be about a node nobody dispatched.
fn usable(value: &str) -> Option<String> {
    if value.is_empty() || value.len() >= crate::event::MAX_PAYLOAD_TEXT_BYTES {
        return None;
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    Some(value.to_owned())
}

/// Whether an envelope is a session opening, in `onevcs`'s own vocabulary.
///
/// Asked through that library's enum rather than against a string of this
/// crate's, for the reason [`kind_of`] gives: how a kind is spelled is the
/// sibling's to decide, and a literal here would keep matching after a rename
/// and silently stop folding anything.
pub fn is_session_opened(kind: &crate::event::EventKind) -> bool {
    *kind == kind_of(onevcs::EventKind::SessionOpened)
}

/// The session a lifecycle node asks for.
pub fn request_for(node: &crate::plan::Node) -> Option<SessionRequest> {
    Some(SessionRequest {
        repo: node.repo.clone()?,
        // A `resume` names the branch its continuation lives on, and the
        // reconciler has already pinned `branch` to it, so there is one answer
        // here rather than two.
        branch: node.branch.clone(),
        base: node.base_branch.clone(),
        execution_checkout: node.execution_checkout.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Node;

    /// The payload of a session opening, as one of the two producers writes it.
    ///
    /// This crate's own is built by [`session_opened_event`] rather than spelled
    /// out, so the fixture cannot say a shape the producer does not.
    fn ours(token: &str, branch: &str) -> serde_json::Map<String, serde_json::Value> {
        session_opened_event(
            &Session {
                token: SessionToken(token.to_owned()),
                worktree: std::path::PathBuf::from("/tmp/worktree"),
                branch: branch.to_owned(),
                base: "main".to_owned(),
            },
            &crate::event::Labels::default(),
        )
        .payload
    }

    /// What a session record is read for, and what is refused instead of read.
    ///
    /// The refusals are the point. This record is the only pointer a manager has
    /// at work an adoption left behind, so a value that is not whole, or not a
    /// name, sends them looking for a branch that does not exist — and one
    /// carrying a newline forges a line in a view that is read line by line.
    #[test]
    fn a_session_record_is_read_only_where_every_value_it_names_is_usable() {
        let read = DispatchSession::read_from(&ours("s-abc", "onevcs/s-abc"))
            .expect("a whole session record is read");
        assert_eq!(read.token(), &SessionToken("s-abc".into()));
        assert_eq!(read.branch().as_str(), "onevcs/s-abc");

        // The sibling's own record of the same session: the four fields this
        // crate reads, plus everything else `onevcs` knows about it. Those are
        // kept out of the way rather than refused — it is the producer's account
        // of the session, and a reader that rejected what it had not heard of
        // would drop the record this fold exists to read.
        let mut theirs = ours("s-abc", "onevcs/s-abc");
        for (key, value) in [
            ("identity", serde_json::json!("github.com/owner/service")),
            ("clone", serde_json::json!("/tmp/runs/s-abc/clone")),
            ("execution_checkout", serde_json::json!("/tmp/service")),
            ("publication_checkout", serde_json::json!("/tmp/service")),
            ("reused", serde_json::json!(true)),
        ] {
            theirs.insert(key.to_owned(), value);
        }
        assert_eq!(
            DispatchSession::read_from(&theirs).as_ref(),
            Some(&read),
            "the sibling's own record of a session it opened was not read"
        );

        let without = |key: &str| {
            let mut payload = ours("s-abc", "onevcs/s-abc");
            payload.remove(key);
            payload
        };
        // A value that is *cut* is the one that reads as a name and is not one:
        // `onevcs` bounds a payload text field and says so beside it, so a branch
        // this long may be a prefix of the branch the work is actually on.
        let cut = "b".repeat(crate::event::MAX_PAYLOAD_TEXT_BYTES);
        for (why, payload) in [
            ("a record naming no branch at all", without("branch")),
            ("a record naming no token at all", without("token")),
            ("a branch that is empty", ours("s-abc", "")),
            ("a token that is empty", ours("", "onevcs/s-abc")),
            (
                "a branch carrying a line of its own",
                ours("s-abc", "onevcs/s-abc\n  audit    running"),
            ),
            ("a token carrying a space", ours("s abc", "onevcs/s-abc")),
            (
                "a branch the producer cut at its bound",
                ours("s-abc", &cut),
            ),
        ] {
            assert_eq!(
                DispatchSession::read_from(&payload),
                None,
                "{why} was read as a session a manager can be sent to"
            );
        }
    }

    #[test]
    fn a_lifecycle_node_asks_for_the_session_its_fields_describe() {
        let node = Node {
            id: "service".into(),
            repo: Some("owner/repo".into()),
            branch: Some("feature".into()),
            base_branch: Some("main".into()),
            execution_checkout: Some("primary".into()),
            persona: Some("engineer".into()),
            task: Some("## What\nship".into()),
            ..Node::default()
        };
        let request = request_for(&node).expect("a lifecycle node asks for a session");
        assert_eq!(request.repo, "owner/repo");
        assert_eq!(request.branch.as_deref(), Some("feature"));
        assert_eq!(request.base.as_deref(), Some("main"));
        assert_eq!(request.execution_checkout.as_deref(), Some("primary"));
    }

    #[test]
    fn a_direct_agent_node_asks_for_no_session() {
        let node = Node {
            id: "build".into(),
            persona: Some("engineer".into()),
            task: Some("## What\ndo it".into()),
            ..Node::default()
        };
        assert!(request_for(&node).is_none());
    }

    #[test]
    fn every_ending_a_publication_has_settles_the_node_under_its_own_name() {
        let sha = onevcs::Sha("abc".into());
        let url: onevcs::Url = "https://example.invalid/pull/7".parse().expect("a URL");
        assert_eq!(outcome_of(&PublishOutcome::Merged(sha)), "merged");
        assert_eq!(
            outcome_of(&PublishOutcome::ChangeOpen(url.clone())),
            "change-open"
        );
        assert_eq!(outcome_of(&PublishOutcome::Queued(url)), "queued");
        assert_eq!(outcome_of(&PublishOutcome::NothingToPublish), "no-changes");
        assert_eq!(
            outcome_of(&PublishOutcome::Failed {
                kind: onevcs::FailureKind::Gate,
                reason: "the gate said no".into(),
                retained: None,
            }),
            "publication-failed"
        );
    }

    /// Which endings this crate is willing to call landed.
    ///
    /// Exactly one: the case `onevcs` produces holding the commit the change
    /// reached its base at. The two that carry a change-request URL are the ones
    /// a policy asking for an immediate merge produces when the host has not
    /// merged, so a derivation that read the policy — or that read "the
    /// publication succeeded" — would call both of them landed. That is the
    /// false report this whole distinction exists to remove, so it is stated
    /// case by case here rather than left to a catch-all.
    #[test]
    fn only_a_change_observed_on_its_base_is_called_landed() {
        use crate::graph::Landing;
        let url: onevcs::Url = "https://example.invalid/pull/7".parse().expect("a URL");
        assert_eq!(
            landing_of(&PublishOutcome::Merged(onevcs::Sha("abc".into()))),
            Some(Landing::Landed)
        );
        // A change request somebody has to merge, and one the host is holding
        // behind checks: both are a change that has not reached its base.
        assert_eq!(
            landing_of(&PublishOutcome::ChangeOpen(url.clone())),
            Some(Landing::Unlanded)
        );
        assert_eq!(
            landing_of(&PublishOutcome::Queued(url)),
            Some(Landing::Unlanded)
        );
        // Neither of these has a change of its own to land, and neither is
        // reported as though it might: a branch its base already carried settles
        // `no-changes`, and a publication that failed settles `failed`.
        assert_eq!(landing_of(&PublishOutcome::NothingToPublish), None);
        assert_eq!(
            landing_of(&PublishOutcome::Failed {
                kind: onevcs::FailureKind::Gate,
                reason: "the gate said no".into(),
                retained: None,
            }),
            None
        );
    }

    #[test]
    fn a_change_request_is_where_a_human_reads_it_and_a_local_merge_names_none() {
        let url: onevcs::Url = "https://example.invalid/pull/7".parse().expect("a URL");
        assert_eq!(
            change_url(&PublishOutcome::ChangeOpen(url.clone())).as_deref(),
            Some("https://example.invalid/pull/7")
        );
        assert_eq!(
            change_url(&PublishOutcome::Queued(url)).as_deref(),
            Some("https://example.invalid/pull/7")
        );
        assert_eq!(
            change_url(&PublishOutcome::Merged(onevcs::Sha("abc".into()))),
            None
        );
        assert_eq!(change_url(&PublishOutcome::NothingToPublish), None);
    }

    #[test]
    fn a_publication_records_what_the_sibling_said_it_did() {
        let url: onevcs::Url = "https://example.invalid/pull/7".parse().expect("a URL");
        let published = Publication {
            session: SessionToken("s-1".into()),
            branch: "onepipeline/service".into(),
            policy: MergePolicy::ChangeOpen,
            outcome: PublishOutcome::ChangeOpen(url),
        };
        let event = published_event(&published, &crate::event::Labels::default());
        assert_eq!(event.stream, "onevcs-onepipeline/service");
        assert_eq!(event.payload["branch"], "onepipeline/service");
        assert_eq!(event.payload["policy"], "change-open");
        assert_eq!(event.payload["outcome"], "change-open");
        assert_eq!(event.payload["url"], "https://example.invalid/pull/7");
        // The publication's own record says where the change got to, so a reader
        // watching the stream sees it at the moment it happened rather than only
        // in the settlement folded from it afterwards.
        assert_eq!(event.payload["landing"], "unlanded");

        let merged = Publication {
            outcome: PublishOutcome::Merged(onevcs::Sha("abc".into())),
            ..published
        };
        let event = published_event(&merged, &crate::event::Labels::default());
        assert_eq!(event.payload["landing"], "landed");

        // Nothing to publish is nothing to land, and the record says so by
        // carrying no claim rather than by carrying the convenient one.
        let empty = Publication {
            outcome: PublishOutcome::NothingToPublish,
            ..merged
        };
        let event = published_event(&empty, &crate::event::Labels::default());
        assert_eq!(event.payload["landing"], serde_json::Value::Null);
    }

    /// What this crate reads from a session stream that is not whole.
    ///
    /// `onevcs` appends a record as *two* writes — the line, then its newline —
    /// so a reader can see the line before its terminator, and its typed reader
    /// advances its cursor over whatever `str::lines` yields. That was carried
    /// in as a known risk on exactly the path a publication is followed on, and
    /// a stream read wrongly is a publication that looks like it never
    /// happened. So it is exercised rather than reasoned about.
    ///
    /// One test, not three: `ONEVCS_HOME` is process-global, and separate tests
    /// would set it from separate threads and read one another's state root.
    #[test]
    fn a_session_stream_that_is_not_whole_is_read_for_what_it_holds() {
        let root = std::env::temp_dir().join(format!("onepipeline-stream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("streams")).expect("a scratch state root");
        std::env::set_var(onevcs_home(), &root);

        let record = |token: &str, seq: u64, kind: &str| {
            serde_json::json!({
                "v": 1,
                "ts": "2026-01-01T00:00:00.000Z",
                "stream": token,
                "seq": seq,
                "source": "vcs",
                "kind": kind,
                "labels": {},
                "payload": {},
                "artifacts": [],
            })
            .to_string()
        };
        let write = |token: &str, body: String| {
            std::fs::write(root.join("streams").join(format!("{token}.ndjson")), body)
                .expect("the stream is written");
        };
        let seqs = |envelopes: &[Envelope]| envelopes.iter().map(|e| e.seq).collect::<Vec<_>>();

        // A whole record whose newline has not been written yet. It is read —
        // and read *once*: the cursor that consumed it does not hand it back
        // when the terminator and the next record arrive.
        let torn = "s-unterminated";
        let followed = SessionToken(torn.to_owned());
        write(
            torn,
            format!(
                "{}\n{}",
                record(torn, 1, "session-opened"),
                record(torn, 2, "push")
            ),
        );
        let mut stream = opened(&followed, None).expect("the stream opens");
        assert_eq!(
            seqs(&next_batch(&mut stream, &followed)),
            vec![1, 2],
            "a record whose newline was still unwritten was lost"
        );
        write(
            torn,
            format!(
                "{}\n{}\n{}\n",
                record(torn, 1, "session-opened"),
                record(torn, 2, "push"),
                record(torn, 3, "session-closed")
            ),
        );
        assert_eq!(
            seqs(&next_batch(&mut stream, &followed)),
            vec![3],
            "the terminator arriving handed a record back a second time"
        );

        // A line that is not a whole envelope — a stream cut mid-record. The
        // sibling's typed reader refuses the **batch**, and its cursor has
        // already moved past the line, so the whole records before it in the
        // same read are refused with it. Reported out loud rather than folded
        // into an empty stream, and recorded as a proposal for `onevcs` in
        // `docs/contract-divergences.md`.
        let cut = "s-cutmidline";
        let whole = record(cut, 1, "session-opened");
        let partial = record(cut, 2, "push");
        write(cut, format!("{whole}\n{}", &partial[..20]));
        assert!(
            events(&SessionToken(cut.to_owned()), None).is_empty(),
            "the typed reader now hands back the whole records before a torn one; \
             narrow this assertion to the torn record alone"
        );

        // And a stream nothing wrote at all is not an empty one: it is refused
        // by name, which is what stops a token nobody opened reading as a
        // session that recorded nothing.
        assert!(events(&SessionToken("s-neverwritten".into()), None).is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The variable naming the sibling's state root, spelled once.
    ///
    /// `onevcs` publishes no constant for it, so the test that points it at a
    /// scratch root says it here rather than in three places.
    fn onevcs_home() -> &'static str {
        "ONEVCS_HOME"
    }

    #[test]
    fn a_relayed_envelope_keeps_the_kind_and_attribution_its_producer_wrote() {
        let mut labels = onevcs::Labels {
            member: Some("worker".into()),
            ..onevcs::Labels::default()
        };
        labels
            .extra
            .insert("session".into(), serde_json::json!("s-1"));
        let envelope = relayed(onevcs::Envelope {
            v: 1,
            ts: "2026-01-01T00:00:00.000Z".into(),
            stream: "s-1".into(),
            seq: 4,
            source: onevcs::Source::Vcs,
            kind: onevcs::EventKind::ChangeOpened,
            labels,
            payload: serde_json::Map::new(),
            artifacts: vec![onevcs::ArtifactRef {
                id: onevcs::ArtifactId("a-1".into()),
                kind: "log".into(),
                bytes: 12,
            }],
        });
        assert_eq!(envelope.kind.0, "change-opened");
        assert_eq!(envelope.source, crate::event::Source::Vcs);
        assert_eq!(envelope.seq, 4);
        // A key the merged envelope does not reserve rides in `extra` rather
        // than being dropped in the relay.
        assert_eq!(envelope.labels.extra["member"], "worker");
        assert_eq!(envelope.labels.extra["session"], "s-1");
        assert_eq!(envelope.artifacts[0].id.0, "a-1");
    }
}
