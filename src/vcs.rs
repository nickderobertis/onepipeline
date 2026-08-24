//! The `onevcs` seam.
//!
//! Repository identities, sessions, preserved work, and publication stay in that
//! library. A lifecycle node is this crate opening a session there, running its
//! dispatches inside the worktree that session hands back, and publishing
//! through it — never re-deriving a branch name or a merge policy, and never
//! verifying the change itself: what verifies it is the repository's own merge
//! path, which is the host's required checks for a remote-publishing identity and
//! the repository's `pre-push` hook at the publishing push for a local one.
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
        PublishOutcome::Failed { kind, .. } => failure_of(*kind).outcome(),
    }
}

/// A publication failure a further attempt on the same branch could answer.
///
/// A closed set and not a word, because these five *are* the vocabulary: each
/// one is a settlement the contract publishes, a routing decision this crate
/// makes, and a case a reader of a preserved failure switches on. Carried as a
/// string, a sixth spelling would be constructible everywhere one of these is —
/// in a settlement, in a re-dispatch's reason, in the roll-up a spent budget
/// writes — and every one of those is a word the contract does not name reaching
/// an operator as though it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preserving {
    /// A required check the host reports concluded red.
    ChecksFailed,
    /// The bound on watching the host elapsed with the change still outstanding.
    ChecksUnsettled,
    /// The publishing push was refused by the merge path.
    PushRejected,
    /// The base moved under the publication and the bounded resolve-and-requeue
    /// did not converge.
    SyncConflict,
    /// The publishing push reached the remote and the merge path could not then
    /// be read. The one whose work is already **on the origin**, so what a
    /// further attempt re-reads is that path rather than the push.
    PushedUnverified,
}

impl Preserving {
    /// The word the node settles on.
    #[must_use]
    pub fn outcome(self) -> &'static str {
        match self {
            Self::ChecksFailed => "checks-failed",
            Self::ChecksUnsettled => "checks-unsettled",
            Self::PushRejected => "push-rejected",
            Self::SyncConflict => "sync-conflict",
            Self::PushedUnverified => "pushed-unverified",
        }
    }
}

/// One publication failure, as this crate settles and routes it.
///
/// The word and the routing are **one** value, because they are one judgement:
/// a failure this crate names is exactly a failure it knows what to do about,
/// and one it cannot name is the residual it can only report. Two fields would
/// permit the state that judgement rules out — a word of its own that is never
/// re-dispatched, which reads from every view as a failure that was routed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Its fix is *more work on the same branch* — a red check to make green, a
    /// conflict to resolve, a push a hook refused — and it settles under that
    /// failure's own word. The tree that was rejected is still there, so a
    /// worker sent back to it meets the thing that rejected it.
    Preserving(Preserving),
    /// Nothing a further attempt could answer: a request `onevcs` refused at its
    /// trust boundary, a seam with no implementation, and a gate that ran on the
    /// tree as it stands all answer the same way however many times they are
    /// asked. It settles under [`Failure::RESIDUAL`].
    Terminal,
}

impl Failure {
    /// The word every failure with nothing to continue settles on.
    ///
    /// The one this crate settled *every* publication failure on before any of
    /// them were told apart, kept for exactly the kinds no continuation follows
    /// from — so no reader of it has to relearn anything.
    pub const RESIDUAL: &'static str = "publication-failed";

    /// The word the node settles on.
    #[must_use]
    pub fn outcome(self) -> &'static str {
        match self {
            Self::Preserving(preserving) => preserving.outcome(),
            Self::Terminal => Self::RESIDUAL,
        }
    }
}

/// Which failure a publication ended in, in this crate's own words.
///
/// Arm by arm rather than by a wildcard, deliberately: a kind the sibling adds
/// is a routing decision this crate has to make, and a wildcard would make it
/// silently — reporting a new failure as the residual and never re-dispatching
/// it, which is exactly the undifferentiated settlement this exists to end.
pub fn failure_of(kind: onevcs::FailureKind) -> Failure {
    use onevcs::FailureKind;
    match kind {
        FailureKind::ChecksFailed => Failure::Preserving(Preserving::ChecksFailed),
        FailureKind::ChecksUnsettled => Failure::Preserving(Preserving::ChecksUnsettled),
        FailureKind::PushRejected => Failure::Preserving(Preserving::PushRejected),
        FailureKind::SyncConflict => Failure::Preserving(Preserving::SyncConflict),
        // The push landed and only the *read* behind it did not, which is the
        // opposite of "nothing a further attempt could answer" — so preserving,
        // and under a word of its own rather than the residual.
        FailureKind::PushedUnverified => Failure::Preserving(Preserving::PushedUnverified),
        // `Gate` is a kind no publication produces any more — `onevcs` 0.11.0
        // runs no gate — and it is routed rather than dropped because the
        // sibling still names it: an arm removed here would be a wildcard by
        // another name the day anything emits it again. Terminal beside the
        // other two, on the reading the kind was written for: a verdict on the
        // tree as it stands, which nothing this crate can do from here changes.
        FailureKind::Gate | FailureKind::Invalid | FailureKind::NotImplemented => Failure::Terminal,
    }
}

/// One piece of evidence a session's publication recorded, as a reader fetches
/// it.
///
/// A **pointer**, never the bytes: an artifact is a check's log or a conflict's
/// hunks, which is megabytes of somebody else's output, and what a diagnosis
/// carries is what to ask for it with. Both halves stay the types the envelope
/// they were read off carries them as, so nothing between that stream and the
/// note is a string this crate re-derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    /// Which part of the publication produced it — `onevcs`'s own kind, so
    /// naming one restates no vocabulary this crate does not own.
    pub kind: crate::event::EventKind,
    /// What `onevcs artifact cat` takes.
    pub id: crate::event::ArtifactId,
}

/// The evidence one session's publication recorded.
///
/// Read off the session's **own stream** and **unfiltered**, deliberately: this
/// is not a relay into the merged store but the node's own read of what its
/// publication left behind, and a launch that narrowed what it *ingests* did not
/// ask to be unable to diagnose a failure.
///
/// In stream order and **deduplicated**, because an artifact is stored once and
/// referenced from wherever the publication points at it: a note listing one id
/// twice reads as two runs of whatever produced it, and sends somebody looking
/// for the difference between them. `onevcs` 0.11.0 happens to reference each
/// artifact it stores from one record only — the two-record shape was the
/// `pre-push` gate's, whose verdict *was* the push's own output, and that gate is
/// gone — so nothing downstream of this crate holds the property. It is kept
/// because it is a property of the **note**, not of any one version of the
/// stream behind it.
pub fn evidence_in(token: &SessionToken) -> Vec<Evidence> {
    let mut evidence: Vec<Evidence> = Vec::new();
    for envelope in events(token, None) {
        for artifact in envelope.artifacts {
            let found = Evidence {
                kind: envelope.kind.clone(),
                id: artifact.id,
            };
            if !evidence.iter().any(|kept| kept.id == found.id) {
                evidence.push(found);
            }
        }
    }
    evidence
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

/// One read of an open session's own record: the sibling's own value, carrying
/// the worktree its dispatches work in and the base its branch is measured
/// against. Read once, because a second read at publication would ask a question
/// already answered on a path where the answer cannot have changed.
///
/// Its values are taken as that record states them. A record is `onevcs`'s own
/// state, written under its occupancy lease when it cut this session; a session's
/// **stream** is the untrusted one — a log any process holding the token appends
/// to — and what this crate reads off one is checked where it enters, by
/// [`DispatchSession::read_from`].
///
/// A read, not a claim: [`onevcs::session`] takes no lease, commits nothing, and
/// reclaims nothing, so asking where a session is working cannot disturb it —
/// unlike `adopt`, which commits whatever the worktree holds behind an
/// incomplete-step marker.
///
/// `None` when the record cannot be read, which leaves the caller to open a
/// session as it would have. The second and later dispatches of one lifecycle
/// node run in the worktree it names; they must **not** open a session of their
/// own, because `onevcs` cuts each session its own clone and opening a second one
/// reclaims the first's workspace. Both are recorded in
/// `docs/contract-divergences.md`.
pub fn working_session(token: &SessionToken) -> Option<Session> {
    onevcs::session(&providers(), token)
        .map(|record| record.session)
        .map_err(|error| {
            eprintln!(
                "onepipeline: cannot read session {}'s record: {error}",
                token.0
            );
            error
        })
        .ok()
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
/// Read *once at settlement*, a lifecycle node's push, change request, check
/// polling, and merge are one opaque blocking call: every record appears at
/// once, when it is over — and that stretch is the longest wall-clock segment
/// the node has. This is the same stream read as it grows,
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

/// A branch a stream's record named, and [`usable`] accepted.
///
/// Deliberately **not** a claim that git would accept the name: that parser is
/// git's, and asking it would mean this crate running git, which no path of it
/// ever has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchName(String);

impl BranchName {
    /// A name off a stream's record, where it is one this crate can act on.
    pub fn checked(value: &str) -> Option<Self> {
        usable(value).map(Self)
    }

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
    /// Read a session out of a relayed `session-opened`, where it is one this
    /// crate can act on.
    ///
    /// **The trust boundary for a session record**, which the envelope module
    /// leaves to the reader seam that parses a stream. Three questions, and only
    /// the last two are this crate's:
    ///
    /// * **Which fields** is [`Session`]'s, so the payload is parsed through the
    ///   producer's own declaration rather than by key. The fields it does not
    ///   declare are left alone — `onevcs`'s own record of the same session
    ///   carries seven more — while a field this crate acts on is required by the
    ///   type, so a missing or misspelled `branch` drops the record.
    /// * **Whether the values are usable**: see [`usable`].
    /// * **Whether the record is about the session that carried it**, which no
    ///   value inside it can answer. A stream is a log any process holding the
    ///   token appends to, so a record on one naming a *different* session is a
    ///   pointer at somebody else's work, arriving where nobody can check it.
    pub fn read_from(envelope: &Envelope) -> Option<Self> {
        let session: Session =
            serde_json::from_value(serde_json::Value::Object(envelope.payload.clone())).ok()?;
        let token = token_of(&session.token.0)?;
        if !wrote(&envelope.stream, &token) {
            return None;
        }
        Some(Self {
            token,
            branch: BranchName::checked(&session.branch)?,
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

/// Whether a stream is the one a session writes.
///
/// Two spellings, because two producers write a session's opening: `onevcs`
/// streams a session under its own token, and this crate writes its copy for the
/// merged store under that token namespaced by the sibling it came from — see
/// [`session_opened_event`]. Both are that session's, and neither is another's.
fn wrote(stream: &str, token: &SessionToken) -> bool {
    stream == token.0 || stream == format!("onevcs-{}", token.0)
}

/// One session token off a stream's record, where it is one this crate can hand
/// back.
///
/// [`usable`], and one rule more that is about what a token is *for*: it
/// addresses a session, and both libraries name a file by it, so a value
/// carrying a path separator — or one that is a directory hop — is no handle
/// however well it reads. What a token may otherwise be is `onevcs`'s to say,
/// and it says so by refusing one it does not know.
fn token_of(value: &str) -> Option<SessionToken> {
    let value = usable(value)?;
    if value.contains(['/', '\\']) || value.trim_matches('.').is_empty() {
        return None;
    }
    Some(SessionToken(value))
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

/// Whether an envelope is a publication reaching its base, in `onevcs`'s own
/// vocabulary.
///
/// Asked through that library's enum for the reason
/// [`is_session_opened`] is: how a kind is spelled is the sibling's to decide.
pub fn is_merge_completed(kind: &crate::event::EventKind) -> bool {
    *kind == kind_of(onevcs::EventKind::MergeCompleted)
}

/// One value off a relayed payload, held to what a payload text may carry.
///
/// The same check [`usable`] makes of a session token and a branch name, and for
/// the same reason: a landing commit is rendered into a table and a task, where
/// a value carrying whitespace or a control character forges a row.
pub fn usable_value(value: &str) -> Option<String> {
    usable(value)
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
    use std::collections::BTreeSet;

    use super::*;
    use crate::plan::Node;

    /// Every `FailureKind` the sibling distinguishes.
    ///
    /// Written out because the sibling's enum offers no enumeration of itself.
    /// It does not stand alone: [`failure_of`] matches arm by arm, so a variant
    /// added there fails *that* to compile, and this list is what makes the same
    /// addition fail the document's gate below rather than pass it silently.
    const EVERY_KIND: &[onevcs::FailureKind] = &[
        onevcs::FailureKind::Gate,
        onevcs::FailureKind::Invalid,
        onevcs::FailureKind::SyncConflict,
        onevcs::FailureKind::NotImplemented,
        onevcs::FailureKind::ChecksFailed,
        onevcs::FailureKind::ChecksUnsettled,
        onevcs::FailureKind::PushRejected,
        onevcs::FailureKind::PushedUnverified,
    ];

    /// Every failure a further attempt can answer, as the type spells them.
    ///
    /// Written out for the same reason [`EVERY_KIND`] is, and standing alone no
    /// more than it does: [`Preserving::outcome`] matches arm by arm, so a
    /// variant added there fails *that* to compile, and this list is what makes
    /// the same addition fail the document's gate below rather than pass it
    /// silently.
    const EVERY_PRESERVING: &[Preserving] = &[
        Preserving::ChecksFailed,
        Preserving::ChecksUnsettled,
        Preserving::PushRejected,
        Preserving::SyncConflict,
        Preserving::PushedUnverified,
    ];

    /// The words this crate settles a failed publication on and the words the
    /// contract names are one vocabulary.
    ///
    /// The document is the approved surface and the match above is what a run
    /// actually does; only one of them is compiled, so the other needs a gate —
    /// the same one `crate::lifecycle`'s drafting endings have, for the same
    /// reason. A word in the code the document does not name is a settlement
    /// nobody was promised, and a word in the document nothing produces is a
    /// promise nobody keeps.
    #[test]
    fn the_publication_failure_words_and_the_contract_are_one_vocabulary() {
        let contract = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract.md"),
        )
        .expect("the contract ships");
        for kind in EVERY_KIND {
            let word = failure_of(*kind).outcome();
            assert!(
                contract.contains(&format!("`{word}`")),
                "docs/contract.md does not name the `{word}` outcome this crate settles on"
            );
        }
        // And the other direction. The contract lists them in one clause, so the
        // clause is read and its backticked tokens compared with the set the
        // code produces rather than the whole document searched.
        let clause = contract
            .split_once("under a word of its own:")
            .expect("the contract lists the words a failed publication settles on")
            .1
            .split_once("is the **residual**")
            .expect("the clause ends where the residual is named")
            .0;
        let listed: BTreeSet<&str> = clause.split('`').skip(1).step_by(2).collect();
        let vocabulary: BTreeSet<&str> = EVERY_PRESERVING
            .iter()
            .map(|preserving| preserving.outcome())
            .chain(std::iter::once(Failure::RESIDUAL))
            .collect();
        let produced: BTreeSet<&str> = EVERY_KIND
            .iter()
            .map(|kind| failure_of(*kind).outcome())
            .collect();
        // The routing table and the vocabulary are the same set, in both
        // directions: a `Preserving` variant no `FailureKind` reaches is a
        // settlement nothing can produce, and a word `failure_of` produces from
        // outside the vocabulary is one this gate would otherwise never see.
        assert_eq!(
            produced, vocabulary,
            "the words `failure_of` settles on are not the vocabulary `Preserving` closes"
        );
        assert_eq!(
            listed, vocabulary,
            "the contract's publication-failure words are not the ones this crate settles on"
        );
    }

    /// The README summarises the same vocabulary, so it is gated the same way.
    ///
    /// It is a third copy — the match above, the contract, and the prose an
    /// operator actually reads — and the first two already hold each other. Left
    /// ungated the README is the one that goes quietly stale: nothing compiles
    /// it, and an operator meeting a settlement it does not list has no way to
    /// know which of the two is behind.
    ///
    /// Two facts, because the README states two: that every word it carries is
    /// one this crate settles on, and **which of them are re-dispatched**. The
    /// second is the one an operator plans around — a word that quietly moved
    /// across that line would have them waiting for a retry that never comes —
    /// so the clause naming those is compared as a set with what `Preserving`
    /// closes rather than searched one word at a time.
    ///
    /// Both ends of that clause are anchored on prose that does not count them,
    /// so widening the vocabulary is a README edit rather than a README edit and
    /// a test edit.
    #[test]
    fn the_readmes_publication_failure_summary_is_the_vocabulary_this_crate_settles_on() {
        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"),
        )
        .expect("the README ships");
        // Wrapped prose, so match on its words rather than its line breaks.
        let readme = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        for kind in EVERY_KIND {
            let word = failure_of(*kind).outcome();
            assert!(
                readme.contains(&format!("`{word}`")),
                "the README does not name the `{word}` outcome this crate settles on"
            );
        }
        let clause = readme
            .split_once("settle under a word of their own")
            .expect("the README names the failures that settle under a word of their own")
            .1
            .split_once("leaves the rejected tree")
            .expect("that clause ends where the README says what those failures share")
            .0;
        let listed: BTreeSet<&str> = clause.split('`').skip(1).step_by(2).collect();
        let routed: BTreeSet<&str> = EVERY_PRESERVING
            .iter()
            .map(|preserving| preserving.outcome())
            .collect();
        assert_eq!(
            listed, routed,
            "the README's re-dispatched failures are not the ones this crate re-dispatches"
        );
        assert!(
            readme.contains(&format!("Everything else settles `{}`", Failure::RESIDUAL)),
            "the README does not name `{}` as what everything else settles on",
            Failure::RESIDUAL
        );
    }

    /// Which kinds a further attempt can answer, said kind by kind.
    ///
    /// That a word of its own *is* a routing decision is no longer assertable —
    /// [`Failure`] makes the two one value, so the inconsistent state has no
    /// spelling. What still needs saying is which side of the line each kind
    /// falls on, because that is a judgement about the failure rather than about
    /// the type: a refused request and an unimplemented seam answer the same way
    /// however many times they are asked, and a gate that ran on the tree as it
    /// stands is not the host's report on a change request.
    #[test]
    fn each_kind_is_on_the_side_of_the_line_the_contract_puts_it() {
        let terminal = [
            onevcs::FailureKind::Invalid,
            onevcs::FailureKind::NotImplemented,
            onevcs::FailureKind::Gate,
        ];
        for kind in terminal {
            assert_eq!(
                failure_of(kind),
                Failure::Terminal,
                "{kind:?} is retried, and asking again would reproduce the diagnosis"
            );
        }
        let preserving: BTreeSet<&str> = EVERY_KIND
            .iter()
            .filter(|kind| !terminal.contains(kind))
            .map(|kind| match failure_of(*kind) {
                Failure::Preserving(preserving) => preserving.outcome(),
                Failure::Terminal => panic!("{kind:?} is terminal, so nothing continues it"),
            })
            .collect();
        assert_eq!(
            preserving,
            BTreeSet::from([
                "checks-failed",
                "checks-unsettled",
                "push-rejected",
                "pushed-unverified",
                "sync-conflict"
            ]),
            "the failures a further attempt can answer are not the five the contract names"
        );
        // And the residual is one word for all of them, which is what keeps it a
        // residual rather than a fifth name.
        assert_eq!(Failure::Terminal.outcome(), Failure::RESIDUAL);
    }

    /// The payload of a session opening, as one of the two producers writes it.
    ///
    /// This crate's own is built by [`session_opened_event`] rather than spelled
    /// out, so the fixture cannot say a shape the producer does not.
    fn ours(token: &str, branch: &str) -> Envelope {
        session_opened_event(
            &Session {
                token: SessionToken(token.to_owned()),
                worktree: std::path::PathBuf::from("/tmp/worktree"),
                branch: branch.to_owned(),
                base: "main".to_owned(),
            },
            &crate::event::Labels::default(),
        )
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
        // And on the stream `onevcs` writes it to, which is the session's own
        // token rather than this crate's namespaced spelling of it.
        theirs.stream = "s-abc".to_owned();
        for (key, value) in [
            ("identity", serde_json::json!("github.com/owner/service")),
            ("clone", serde_json::json!("/tmp/runs/s-abc/clone")),
            ("execution_checkout", serde_json::json!("/tmp/service")),
            ("publication_checkout", serde_json::json!("/tmp/service")),
            ("reused", serde_json::json!(true)),
        ] {
            theirs.payload.insert(key.to_owned(), value);
        }
        assert_eq!(
            DispatchSession::read_from(&theirs).as_ref(),
            Some(&read),
            "the sibling's own record of a session it opened was not read"
        );

        let without = |key: &str| {
            let mut event = ours("s-abc", "onevcs/s-abc");
            event.payload.remove(key);
            event
        };
        // A record about a session other than the one whose log carried it: the
        // stream is whose log it is, and a pointer at somebody else's work
        // arriving here is one nobody can check.
        let mut elsewhere = ours("s-elsewhere", "onevcs/s-elsewhere");
        elsewhere.stream = "onevcs-s-abc".to_owned();
        for (why, event) in [
            ("a record naming no branch at all", without("branch")),
            ("a record naming no token at all", without("token")),
            ("a record about another session entirely", elsewhere),
        ] {
            assert_eq!(
                DispatchSession::read_from(&event),
                None,
                "{why} was read as a session a manager can be sent to"
            );
        }
        for (why, value) in unusable() {
            assert_eq!(
                DispatchSession::read_from(&ours("s-abc", &value)),
                None,
                "a branch that is {why} was read as one a manager can be sent to"
            );
        }
        // A token is everything a branch is, and a plain name besides: both
        // libraries name a file by it, and a branch — which may hold a `/` —
        // this crate only ever hands back.
        let hops = [
            ("a directory hop", "..".to_owned()),
            ("carrying a path separator", "onevcs/../x".to_owned()),
        ];
        for (why, value) in unusable().into_iter().chain(hops) {
            let mut record = ours(&value, "onevcs/s-abc");
            record.stream = value.clone();
            assert_eq!(
                DispatchSession::read_from(&record),
                None,
                "a token that is {why} was read as one a session answers to"
            );
        }
    }

    /// Every value neither a branch nor a token may be, and why.
    ///
    /// One table, because one check answers for both and for the base a
    /// publication names beside them: a value that is not whole, or not a name,
    /// is the same fault wherever this crate reads one.
    fn unusable() -> Vec<(&'static str, String)> {
        vec![
            ("empty", String::new()),
            (
                "a line of its own",
                "onevcs/x\n  audit    running".to_owned(),
            ),
            ("carrying a space", "onevcs/ x".to_owned()),
            ("carrying a control character", "onevcs/x\u{7}".to_owned()),
            // A value that is *cut* is the one that reads as a name and is not
            // one: `onevcs` bounds a payload text field and says so beside it, so
            // a name this long may be a prefix of what the work is actually on.
            (
                "as long as the bound a producer cuts text at",
                "b".repeat(crate::event::MAX_PAYLOAD_TEXT_BYTES),
            ),
        ]
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
        // A failed publication settles under the word its **kind** earns, which
        // is what a caller branches on. Every kind, so the residual is proven to
        // be a residual rather than the only answer.
        let failed = |kind| {
            outcome_of(&PublishOutcome::Failed {
                kind,
                reason: "the publication said no".into(),
                retained: None,
            })
        };
        assert_eq!(failed(onevcs::FailureKind::Gate), "publication-failed");
        assert_eq!(failed(onevcs::FailureKind::Invalid), "publication-failed");
        assert_eq!(
            failed(onevcs::FailureKind::NotImplemented),
            "publication-failed"
        );
        assert_eq!(failed(onevcs::FailureKind::ChecksFailed), "checks-failed");
        assert_eq!(
            failed(onevcs::FailureKind::ChecksUnsettled),
            "checks-unsettled"
        );
        assert_eq!(failed(onevcs::FailureKind::PushRejected), "push-rejected");
        assert_eq!(failed(onevcs::FailureKind::SyncConflict), "sync-conflict");
        assert_eq!(
            failed(onevcs::FailureKind::PushedUnverified),
            "pushed-unverified"
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
                kind: onevcs::FailureKind::PushRejected,
                reason: "the merge path refused the publishing push".into(),
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
