//! **Flows**: a launched process a manager must supervise that is not itself a
//! run — a planning launcher that starts runs between stages of its own, or a
//! deploy that starts none.
//!
//! What a flow promises — the verb, the launch record's `flow`, what
//! `unwatched` owes for one, `watch --flow` and the flow's own channel — is
//! stated once, in the Flows section of `docs/stop-guard.md`, and entry 114 of
//! `docs/contract-divergences.md` records why it exists; neither is restated
//! here, on the terms [`crate::unwatched`] keeps beside its own entry.
//!
//! What belongs beside the code is the layout, which is this repository's alone.
//! Every flow sits under one directory of the runs root, [`FLOWS_DIR`], which
//! run-root discovery passes over: an engine before flows lists it as one
//! skipped root and the same runs it always listed. One flow is one directory
//! there, laid out as a run's where a run's reader is reused on it — its
//! channel, its watch leases and their terms, its acknowledgements — so
//! [`paths`] answers a [`RunPaths`] whose `run` is the flow's id. Beside those:
//! `flow.json`, the record its holder wrote; `ending.json`, the status the
//! holder recorded when its program ended; and `members/`, one document per run
//! launched in it, which is what a watch of the flow lists rather than reading
//! every launch record under the root.

use std::ffi::OsString;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::channel::{ChannelState, Surface, SurfaceKind};
use crate::cli::{FlowRunArgs, FLOW_ENV};
use crate::error::{Error, Result};
use crate::ledger::{self, RecordedBusConfig, RunPaths};
use crate::sys;

/// The directory under the runs root every flow is kept in.
pub(crate) const FLOWS_DIR: &str = ".flows";

/// The schema version of every document a flow keeps, read closed.
pub(crate) const FLOW_SCHEMA_VERSION: u32 = 1;

/// Read the version, refusing a document this build cannot honestly read.
fn this_version<'de, D: serde::Deserializer<'de>>(reader: D) -> std::result::Result<u32, D::Error> {
    let found = u32::deserialize(reader)?;
    if found != FLOW_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "flow schema_version {found}, and this build reads {FLOW_SCHEMA_VERSION}"
        )));
    }
    Ok(found)
}

/// Text that says something: not blank.
fn said<'de, D: serde::Deserializer<'de>>(reader: D) -> std::result::Result<String, D::Error> {
    let found = String::deserialize(reader)?;
    if found.trim().is_empty() {
        return Err(serde::de::Error::custom("a blank value says nothing"));
    }
    Ok(found)
}

/// An RFC 3339 instant.
fn an_instant<'de, D: serde::Deserializer<'de>>(
    reader: D,
) -> std::result::Result<String, D::Error> {
    let found = String::deserialize(reader)?;
    if !crate::watchers::is_rfc3339(&found) {
        return Err(serde::de::Error::custom(format!(
            "'{found}' is not an RFC 3339 instant"
        )));
    }
    Ok(found)
}

/// What a flow's holder wrote when it registered the flow: whose it is, which
/// process holds it, and the channel's configuration.
// llmlint: ignore-block[invalid_states_unrepresentable] the id, the session, the host and the
// two stamps are `String`s for the reason the watcher record's own block in
// `src/watchers.rs` states — a record read back as another process wrote it, in a vocabulary
// the contract names no type for — and each is checked where it is read: the version, a
// session that says something, `began_at` as an instant, and `started` only ever compared
// through `sys::StartToken::matches`. The id is checked against the directory it sits in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FlowRecord {
    /// The record's own version. See [`FLOW_SCHEMA_VERSION`].
    #[serde(deserialize_with = "this_version")]
    pub(crate) schema_version: u32,
    /// The flow's id, as `flow run` printed it.
    pub(crate) flow_id: String,
    /// The session that owns the flow.
    #[serde(deserialize_with = "said")]
    pub(crate) session: String,
    /// The program and its arguments, as text, for a person reading the record.
    pub(crate) program: Vec<String>,
    /// The holder: the `flow run` process waiting for the program.
    pub(crate) pid: NonZeroU32,
    /// The host that pid means something on.
    pub(crate) host: String,
    /// The holder's start token, empty where this host would not say.
    pub(crate) started: String,
    /// When the flow was registered.
    #[serde(deserialize_with = "an_instant")]
    pub(crate) began_at: String,
    /// The bus configuration the flow's channel is kept under, where the flow
    /// was registered with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) bus_config: Option<RecordedBusConfig>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// How a flow's program ended, as its holder recorded it.
// llmlint: ignore-block[invalid_states_unrepresentable] the id and the instant are `String`s for `FlowRecord`'s reason, and checked where they are read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FlowEnding {
    #[serde(deserialize_with = "this_version")]
    schema_version: u32,
    flow_id: String,
    /// The program's exit status, `128 + the signal` for one a signal ended.
    status: i32,
    #[serde(deserialize_with = "an_instant")]
    at: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// A session's word that a flow it owns is owed nothing further:
/// `onepipeline unwatched --acknowledge-flow ID --reason TEXT`, kept beside the
/// flow.
// llmlint: ignore-block[invalid_states_unrepresentable] the id, the session and the instant are `String`s for `FlowRecord`'s reason, and checked where they are read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FlowAcknowledgement {
    #[serde(deserialize_with = "this_version")]
    schema_version: u32,
    flow_id: String,
    #[serde(deserialize_with = "said")]
    session: String,
    #[serde(deserialize_with = "said")]
    reason: String,
    #[serde(deserialize_with = "an_instant")]
    at: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl FlowAcknowledgement {
    /// The line the binary prints for a recorded acknowledgement.
    pub(crate) fn line(&self) -> String {
        format!(
            "flow {}: acknowledged for session {} — {}\n",
            self.flow_id, self.session, self.reason
        )
    }
}

/// One run launched in a flow, as `start` recorded it in the flow's `members/`.
// llmlint: ignore-block[invalid_states_unrepresentable] the run id and the instant are `String`s for `FlowRecord`'s reason; a member is read back by its file name, which is checked as a run id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    #[serde(deserialize_with = "this_version")]
    schema_version: u32,
    run_id: String,
    #[serde(deserialize_with = "an_instant")]
    at: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// The directory under a flow every manager landing it owns is kept in, one
/// directory per landing, named by its id.
pub(crate) const LANDINGS_DIR: &str = "landings";

/// The engine verb a manager landing forwards to the linked `onevcs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LandingOperation {
    /// `onepipeline publish-branch`, over `onevcs publish-branch`.
    PublishBranch,
    /// `onepipeline repo-recover`, over `onevcs recover`.
    RepoRecover,
    /// `onepipeline integrate`, over `onevcs integrate`.
    Integrate,
}

impl LandingOperation {
    /// The word the sidecar and every payload spell it with.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::PublishBranch => "publish-branch",
            Self::RepoRecover => "repo-recover",
            Self::Integrate => "integrate",
        }
    }
}

/// An identity a landing names: an absolute local identity, or the key `onevcs`
/// gives a hosted origin, held to the structural rule [`is_normalized_origin`]
/// states.
fn an_identity<'de, D: serde::Deserializer<'de>>(
    reader: D,
) -> std::result::Result<String, D::Error> {
    let found = said(reader)?;
    if !(Path::new(&found).is_absolute() || is_normalized_origin(&found)) {
        return Err(serde::de::Error::custom(format!(
            "'{found}' is neither a normalized onevcs origin nor an absolute local identity"
        )));
    }
    Ok(found)
}

/// Whether `text` could be an identity `onevcs` keys a hosted repository by.
///
/// How an origin normalizes is `onevcs`'s, and private to it; what is held here
/// is only the structural half of that crate's own rule for a key it reads back
/// — text with no whitespace or control character, which is no URL left
/// unnormalized — so this never refuses a key the sibling writes and never
/// admits one that names a scheme, a credential or nothing. Which spellings the
/// sibling turns into which keys is reconciled against the linked release by
/// registering them, in `tests/e2e/manager_landings.rs`.
fn is_normalized_origin(text: &str) -> bool {
    !text.contains("://")
        && !text.contains('@')
        && !text.starts_with('/')
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
        && text.split('/').all(|segment| !segment.is_empty())
        && text.contains('/')
}

/// An id a cursor names: the flow-id alphabet, so neither `:` nor `@` nor `/`
/// can make a printed cursor ambiguous.
fn an_id<'de, D: serde::Deserializer<'de>>(reader: D) -> std::result::Result<String, D::Error> {
    let found = String::deserialize(reader)?;
    if !is_valid_flow_id(&found) {
        return Err(serde::de::Error::custom(format!(
            "'{found}' is not an id: an id is letters, digits, `.`, `_` and `-`"
        )));
    }
    Ok(found)
}

/// The subjects a landing advances: at least one.
fn some_subjects<'de, D: serde::Deserializer<'de>>(
    reader: D,
) -> std::result::Result<Vec<LandingSubject>, D::Error> {
    let found = Vec::<LandingSubject>::deserialize(reader)?;
    if found.is_empty() {
        return Err(serde::de::Error::custom(
            "a landing advances at least one branch, and this names none",
        ));
    }
    Ok(found)
}

/// Run ids, each one and each named once.
fn unique_runs<'de, D: serde::Deserializer<'de>>(
    reader: D,
) -> std::result::Result<Vec<String>, D::Error> {
    let found = Vec::<String>::deserialize(reader)?;
    let mut seen = std::collections::BTreeSet::new();
    for run in &found {
        if !ledger::is_valid_run_id(run) {
            return Err(serde::de::Error::custom(format!("'{run}' is not a run id")));
        }
        if !seen.insert(run) {
            return Err(serde::de::Error::custom(format!(
                "run '{run}' is named twice"
            )));
        }
    }
    Ok(found)
}

/// One branch a landing advances, in the identity it belongs to.
// llmlint: ignore-block[invalid_states_unrepresentable] both are `String`s because the approved sidecar declares them as strings, as another process wrote them; each is checked as it is read — the identity as a normalized origin or an absolute local identity, the branch as text that says something.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct LandingSubject {
    /// The `onevcs` identity key the branch belongs to.
    #[serde(deserialize_with = "an_identity")]
    pub(crate) identity: String,
    /// The branch.
    #[serde(deserialize_with = "said")]
    pub(crate) branch: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// `landings/<landing_id>/request.json`: what a manager landing's process
/// registered before it drafted, forwarded or touched anything — whose it is,
/// which flow holds it, every branch it can advance, the runs those branches
/// came from, and the process that does the work. Written once, atomically, and
/// read closed: a sidecar this build cannot read refuses an acknowledgement
/// rather than clearing one.
// llmlint: ignore-block[invalid_states_unrepresentable] the ids, the session, the host and the two stamps are `String`s for `FlowRecord`'s reason, and checked where they are read: the version, the ids against the directories they sit in, a session that says something, `began_at` as an instant, the runs as unique run ids, and `started` only ever compared through `sys::StartToken::matches`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LandingRequest {
    /// The sidecar's version. See [`FLOW_SCHEMA_VERSION`].
    #[serde(deserialize_with = "this_version")]
    pub(crate) schema_version: u32,
    /// The landing's id, unique within its flow: its directory's name.
    #[serde(deserialize_with = "an_id")]
    pub(crate) landing_id: String,
    /// The flow that holds it.
    #[serde(deserialize_with = "an_id")]
    pub(crate) flow_id: String,
    /// The session that owns it: the flow's own.
    #[serde(deserialize_with = "said")]
    pub(crate) session: String,
    /// The verb it forwards.
    pub(crate) operation: LandingOperation,
    /// Every branch it can advance, resolved before any side effect — a whole
    /// integration train, not only the member being landed.
    #[serde(deserialize_with = "some_subjects")]
    pub(crate) subjects: Vec<LandingSubject>,
    /// Every originating run whose own record names one of the subjects.
    #[serde(deserialize_with = "unique_runs")]
    pub(crate) runs: Vec<String>,
    /// The process doing the landing.
    pub(crate) pid: NonZeroU32,
    /// The host that pid means something on.
    #[serde(deserialize_with = "said")]
    pub(crate) host: String,
    /// That process's start token, empty where this host would not say.
    pub(crate) started: String,
    /// When it registered.
    #[serde(deserialize_with = "an_instant")]
    pub(crate) began_at: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// `landings/<landing_id>/ending.json`: the forwarded command's exit status, as
/// the landing's own process recorded it. Absent, the landing is live or died —
/// its process's liveness says which — and never inferred to have succeeded.
// llmlint: ignore-block[invalid_states_unrepresentable] the id and the instant are `String`s for `FlowRecord`'s reason, and checked where they are read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LandingEnding {
    #[serde(deserialize_with = "this_version")]
    pub(crate) schema_version: u32,
    #[serde(deserialize_with = "an_id")]
    pub(crate) landing_id: String,
    /// The forwarded command's exit status.
    pub(crate) status: i32,
    #[serde(deserialize_with = "an_instant")]
    pub(crate) at: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// `landings/<landing_id>/operation.json`: the `onevcs` operation handle the
/// landing prepared before the linked call, so any reader — a death reader
/// above all — reopens that one invocation's own stream. Absent, the landing
/// is legacy coarse-only work.
// llmlint: ignore-block[invalid_states_unrepresentable] the operation id is `onevcs`'s opaque handle, a `String` because `OperationHandle::open` takes one and is the check: an id it cannot open is refused there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LandingOperationRecord {
    #[serde(deserialize_with = "this_version")]
    pub(crate) schema_version: u32,
    #[serde(deserialize_with = "said")]
    pub(crate) operation_id: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// What every landing transition's payload carries.
// llmlint: ignore-block[invalid_states_unrepresentable] the ids and the message are text a reader is shown, `String`s as the approved payload declares them; the ids are the sidecar's own, checked there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub(crate) struct LandingTransition {
    /// The landing.
    pub(crate) landing_id: String,
    /// The flow that holds it.
    pub(crate) flow_id: String,
    /// The verb it forwards.
    pub(crate) operation: LandingOperation,
    /// Every branch it can advance.
    pub(crate) subjects: Vec<LandingSubject>,
    /// What happened, for a person.
    pub(crate) message: String,
}

/// A terminal transition's payload: the transition, and the forwarded
/// command's exit status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub(crate) struct LandingTerminal {
    /// The transition.
    #[serde(flatten)]
    pub(crate) transition: LandingTransition,
    /// The forwarded command's exit status.
    pub(crate) status: i32,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// The audience a landing transition of `kind` is for, as the engine stamps it:
/// registration, drafting and the start are progress, the finish is the final
/// result's milestone, and a refusal or a death is a blocker. `None` for any
/// kind that is not a landing transition.
pub(crate) const fn landing_significance(
    kind: crate::event::PipelineKind,
) -> Option<onevcs::Significance> {
    use crate::event::PipelineKind;
    match kind {
        PipelineKind::LandingRegistered
        | PipelineKind::LandingDrafting
        | PipelineKind::LandingStarted => Some(onevcs::Significance::Progress),
        PipelineKind::LandingFinished => Some(onevcs::Significance::Milestone),
        PipelineKind::LandingRefused | PipelineKind::LandingDied => {
            Some(onevcs::Significance::Blocker)
        }
        _ => None,
    }
}

/// Whether a kind ends a landing: its finish, its refusal, or its death — the
/// three a death reader will not write over and a holder's flow ending reports.
pub(crate) fn is_terminal_landing_kind(kind: &str) -> bool {
    use crate::event::PipelineKind;
    [
        PipelineKind::LandingFinished,
        PipelineKind::LandingRefused,
        PipelineKind::LandingDied,
    ]
    .iter()
    .any(|terminal| terminal.as_str() == kind)
}

/// One manager landing a flow holds: where it is, and what it registered.
#[derive(Debug, Clone)]
pub(crate) struct Landing {
    /// `landings/<landing_id>` under its flow.
    pub(crate) dir: PathBuf,
    /// What it registered.
    pub(crate) request: LandingRequest,
}

impl Landing {
    /// Its own journal: every transition, and every `onevcs` operation event
    /// relayed from its handle.
    pub(crate) fn journal(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }

    pub(crate) fn id(&self) -> &str {
        &self.request.landing_id
    }

    /// The status its process recorded, nothing where it recorded none, or why
    /// the record cannot be read.
    pub(crate) fn ending(&self) -> std::result::Result<Option<i32>, String> {
        let path = self.dir.join("ending.json");
        let unreadable = |why: String| {
            format!(
                "landing '{}''s ending cannot be read, so whether it ended cannot be said: {}: \
                 {why}",
                self.id(),
                path.display()
            )
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let ending = serde_json::from_str::<LandingEnding>(&text)
                    .map_err(|error| unreadable(error.to_string()))?;
                if ending.landing_id != self.id() {
                    return Err(unreadable(format!(
                        "it is landing '{}''s ending, not this landing's",
                        ending.landing_id
                    )));
                }
                Ok(Some(ending.status))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unreadable(error.to_string())),
        }
    }

    /// The `onevcs` operation id it prepared, nothing for legacy coarse-only
    /// work, or why the record cannot be read.
    pub(crate) fn operation_id(&self) -> std::result::Result<Option<String>, String> {
        let path = self.dir.join("operation.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<LandingOperationRecord>(&text)
                .map(|record| Some(record.operation_id))
                .map_err(|error| {
                    format!(
                        "landing '{}''s operation record cannot be read: {}: {error}",
                        self.id(),
                        path.display()
                    )
                }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!(
                "landing '{}''s operation record cannot be read: {}: {error}",
                self.id(),
                path.display()
            )),
        }
    }

    /// Where the landing stands now, on the rule a flow's holder is judged by:
    /// its recorded ending, else its process's liveness by pid and start token.
    ///
    /// # Errors
    ///
    /// An ending that is there and cannot be read.
    pub(crate) fn standing(&self) -> std::result::Result<Standing, String> {
        if let Some(status) = self.ending()? {
            return Ok(Standing::Ended(status));
        }
        let holder = crate::watchers::holder_standing(
            &self.request.host,
            self.request.pid,
            &self.request.started,
        );
        if !holder.proved_gone() {
            return Ok(Standing::Live);
        }
        // Asked again: the process records its ending and then exits.
        Ok(match self.ending()? {
            Some(status) => Standing::Ended(status),
            None => Standing::Died,
        })
    }

    /// Whether the landing is live: an ending that cannot be read is no
    /// evidence it ended, so such a landing is still owed supervision.
    pub(crate) fn is_live(&self) -> bool {
        !matches!(self.standing(), Ok(Standing::Ended(_) | Standing::Died))
    }

    /// Whether this landing's process is its flow's own holder — the flow was
    /// registered for this landing alone — rather than one landing inside a
    /// longer flow.
    pub(crate) fn holds(&self, flow: &Flow) -> bool {
        self.request.pid == flow.record.pid
            && self.request.host == flow.record.host
            && self.request.started == flow.record.started
    }

    /// Whether a subject of this landing is `branch` of `identity`.
    pub(crate) fn names(&self, identity: &str, branch: &str) -> bool {
        self.request
            .subjects
            .iter()
            .any(|subject| subject.identity == identity && subject.branch == branch)
    }
}

/// Every manager landing under every flow of `root`, of every session.
///
/// # Errors
///
/// Any flow's landing state that cannot be read: whether that landing is live
/// cannot be said, and a caller deciding an acknowledgement refuses on it.
pub(crate) fn every_landing(root: &Path) -> std::result::Result<Vec<(Flow, Landing)>, String> {
    let dir = root.join(FLOWS_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "the flows under {} cannot be read: {error}",
                dir.display()
            ))
        }
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!("the flows under {} cannot be read: {error}", dir.display())
        })?;
        let kind = entry.file_type().map_err(|error| {
            format!(
                "{} cannot be read, so whether it holds a live landing cannot be said: {error}",
                entry.path().display()
            )
        })?;
        if !kind.is_dir() {
            continue;
        }
        let path = entry.path();
        let Ok(id) = entry.file_name().into_string() else {
            return Err(format!(
                "{} is a flow directory whose name is not text, so whether it holds a live \
                 landing cannot be said",
                path.display()
            ));
        };
        let flow = match Flow::open(root, &id) {
            Ok(flow) => flow,
            // A directory holding no landings is no landing's, whatever its record
            // says: registration writes a flow's record before any landing.
            Err(_) if !path.join(LANDINGS_DIR).exists() => continue,
            Err(why) => {
                return Err(format!(
                    "flow {id} holds landings and its record cannot be read, so whether one is \
                     live cannot be said: {why}"
                ))
            }
        };
        for landing in flow.landings().map_err(|why| format!("flow {id}: {why}"))? {
            found.push((flow.clone(), landing));
        }
    }
    Ok(found)
}

/// The lock a landing's registration and a branch acknowledgement both take, so
/// neither a check nor a write of one can fall between the other's.
pub(crate) fn registration_lock(root: &Path) -> Result<std::fs::File> {
    let flows = root.join(FLOWS_DIR);
    std::fs::create_dir_all(&flows).map_err(|source| Error::Ledger {
        path: flows.clone(),
        source,
    })?;
    let path = flows.join("landings.lock");
    sys::open_locked_append(&path).map_err(|source| Error::Ledger { path, source })
}

/// Whether a flow id names one flow under the runs root and nothing else: the
/// alphabet a minted id is spelled in, `[A-Za-z0-9._-]`, and not only dots.
///
/// External input on every verb that takes `--flow`, and joined onto the runs
/// root, so it is held to the alphabet rather than merely to one segment.
pub(crate) fn is_valid_flow_id(id: &str) -> bool {
    !id.is_empty()
        && !id.chars().all(|c| c == '.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Where the flow `id` under `root` keeps its state, laid out as a run's.
pub(crate) fn paths(root: &Path, id: &str) -> RunPaths {
    RunPaths::under(&root.join(FLOWS_DIR), id)
}

fn record_path(paths: &RunPaths) -> PathBuf {
    paths.dir.join("flow.json")
}

fn ending_path(paths: &RunPaths) -> PathBuf {
    paths.dir.join("ending.json")
}

fn members_dir(paths: &RunPaths) -> PathBuf {
    paths.dir.join("members")
}

/// Where a flow stands now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    /// Its holder is alive — or nothing here can prove it is not, which is read
    /// as alive, because a flow read as gone is a session let go.
    Live,
    /// Its program ended with this status, and its holder recorded it.
    Ended(i32),
    /// Its holder is proved gone, with no ending recorded.
    Died,
}

/// One registered flow: where it is, and what its holder recorded.
#[derive(Debug, Clone)]
pub(crate) struct Flow {
    /// Its state, laid out as a run's.
    pub(crate) paths: RunPaths,
    /// What its holder recorded.
    pub(crate) record: FlowRecord,
}

impl Flow {
    /// The flow `id` under `root`.
    ///
    /// # Errors
    ///
    /// An id that is not one, a flow that is not there, and a record this build
    /// cannot read — each naming the flow.
    pub(crate) fn open(root: &Path, id: &str) -> Result<Self> {
        if !is_valid_flow_id(id) {
            return Err(Error::Invalid(format!(
                "'{id}' is not a flow id: a flow id is letters, digits, `.`, `_` and `-`, as \
                 `onepipeline flow run` printed it"
            )));
        }
        let paths = paths(root, id);
        let text = match std::fs::read_to_string(record_path(&paths)) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::Invalid(format!(
                    "no flow '{id}' under {}: a flow is what `onepipeline flow run` registered \
                     and printed the id of",
                    root.display()
                )))
            }
            Err(error) => {
                return Err(Error::Invalid(format!(
                    "flow '{id}''s record cannot be read: {error}"
                )))
            }
        };
        let record: FlowRecord = serde_json::from_str(&text).map_err(|error| {
            Error::Invalid(format!("flow '{id}''s record cannot be read: {error}"))
        })?;
        if record.flow_id != id {
            return Err(Error::Invalid(format!(
                "flow '{id}''s record names flow '{}', so it is not this flow's",
                record.flow_id
            )));
        }
        Ok(Self { paths, record })
    }

    /// The flow's id.
    pub(crate) fn id(&self) -> &str {
        &self.paths.run
    }

    /// The id, typed: [`Flow::open`] admitted it before reading the flow, and
    /// registration minted it from a `--name` already admitted.
    pub(crate) fn flow_id(&self) -> FlowId {
        FlowId(self.paths.run.clone())
    }

    /// Where the flow stands now.
    ///
    /// # Errors
    ///
    /// An ending that is there and cannot be read, which says neither that the
    /// flow ended nor that it did not.
    pub(crate) fn standing(&self) -> std::result::Result<Standing, String> {
        let held = self.holder_standing()?;
        if held == Standing::Live {
            return Ok(held);
        }
        // A landing whose own process outlived the flow's holder is still work
        // a manager owes, so the flow stays live while any of its landings is:
        // an outer flow ending or dying cannot hide a landing still running.
        if self.landings()?.iter().any(Landing::is_live) {
            return Ok(Standing::Live);
        }
        Ok(held)
    }

    /// Where the flow's own holder stands, its landings aside.
    fn holder_standing(&self) -> std::result::Result<Standing, String> {
        if let Some(status) = self.ending()? {
            return Ok(Standing::Ended(status));
        }
        let holder = crate::watchers::holder_standing(
            &self.record.host,
            self.record.pid,
            &self.record.started,
        );
        if !holder.proved_gone() {
            return Ok(Standing::Live);
        }
        // Asked again: a holder records the ending and then exits, so one read
        // between the two would otherwise call a flow that ended a death.
        Ok(match self.ending()? {
            Some(status) => Standing::Ended(status),
            None => Standing::Died,
        })
    }

    /// Every manager landing this flow holds, in id order.
    ///
    /// A directory whose name starts with `.` is a registration still being
    /// staged, and is no landing until it is renamed into place. Absent
    /// `landings/`, the flow is one from before landings, holding none.
    ///
    /// # Errors
    ///
    /// A landing directory, or its request, that cannot be read or does not name
    /// this landing of this flow: whether it is live cannot be said.
    pub(crate) fn landings(&self) -> std::result::Result<Vec<Landing>, String> {
        let dir = self.paths.dir.join(LANDINGS_DIR);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!(
                    "its landings cannot be read, so whether one is live cannot be said: {}: \
                     {error}",
                    dir.display()
                ))
            }
        };
        let mut landings = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "its landings cannot be read, so whether one is live cannot be said: {}: \
                     {error}",
                    dir.display()
                )
            })?;
            let Ok(name) = entry.file_name().into_string() else {
                return Err(format!(
                    "{} holds an entry whose name is not text, so whether it is a live landing \
                     cannot be said",
                    dir.display()
                ));
            };
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path().join("request.json");
            let unreadable = |why: String| {
                format!(
                    "landing '{name}''s request cannot be read, so whether it is live cannot be \
                     said: {}: {why}",
                    path.display()
                )
            };
            let text = std::fs::read_to_string(&path).map_err(|e| unreadable(e.to_string()))?;
            let request: LandingRequest =
                serde_json::from_str(&text).map_err(|e| unreadable(e.to_string()))?;
            if request.landing_id != name || request.flow_id != self.id() {
                return Err(unreadable(format!(
                    "it names landing '{}' of flow '{}'",
                    request.landing_id, request.flow_id
                )));
            }
            // A landing is supervised by the session that owns its flow; one
            // claiming another is not this flow's to answer for.
            if request.session != self.record.session {
                return Err(unreadable(format!(
                    "it names session [{}], and the flow belongs to session [{}]",
                    sys::session_digest(&request.session),
                    sys::session_digest(&self.record.session)
                )));
            }
            landings.push(Landing {
                dir: entry.path(),
                request,
            });
        }
        landings.sort_by(|a, b| a.id().cmp(b.id()));
        Ok(landings)
    }

    /// Whether the flow is live now: its standing, with an ending that cannot be
    /// read taken as no evidence it ended — so such a flow is still owed a watch
    /// rather than closed, and `unwatched` names the ending it could not read.
    pub(crate) fn is_live(&self) -> bool {
        !matches!(self.standing(), Ok(Standing::Ended(_) | Standing::Died))
    }

    fn ending(&self) -> std::result::Result<Option<i32>, String> {
        let path = ending_path(&self.paths);
        let unreadable = |why: String| {
            format!(
                "its ending cannot be read, so whether it ended cannot be said: {}: {why}",
                path.display()
            )
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let ending = serde_json::from_str::<FlowEnding>(&text)
                    .map_err(|error| unreadable(error.to_string()))?;
                // An ending another flow wrote is not this flow's: read as its
                // status it could close a flow whose program is still running.
                if ending.flow_id != self.id() {
                    return Err(unreadable(format!(
                        "it is flow '{}''s ending, not this flow's",
                        ending.flow_id
                    )));
                }
                Ok(Some(ending.status))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unreadable(error.to_string())),
        }
    }

    /// The flow's own channel, under the configuration it was registered with.
    pub(crate) fn channel(&self) -> ChannelState {
        ChannelState::configured(
            &self.paths,
            self.record
                .bus_config
                .as_ref()
                .map(RecordedBusConfig::config),
        )
    }

    /// The bus configuration the flow's channel is kept under, where it names
    /// one.
    pub(crate) fn bus_config(&self) -> Option<&onemessagebus::Config> {
        self.record
            .bus_config
            .as_ref()
            .map(RecordedBusConfig::config)
    }

    /// Every run launched in this flow, by id, in id order.
    ///
    /// Only a `<run>.json` that reads as the membership `start` writes, naming
    /// that same run, names a member: a writer's temporary sibling, a document
    /// this build did not write, and anything else left in the directory are
    /// passed over.
    ///
    /// # Errors
    ///
    /// A membership directory that is there and cannot be read: a watch that
    /// went on without it could never see a run join, so it is said instead.
    pub(crate) fn members(&self) -> Result<Vec<String>> {
        let dir = members_dir(&self.paths);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(Error::Invalid(format!(
                    "flow '{}''s runs cannot be read, so whether one joined cannot be said: {}: \
                     {error}",
                    self.id(),
                    dir.display()
                )))
            }
        };
        let mut members: Vec<String> = entries
            .filter_map(std::result::Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| name.strip_suffix(".json").map(str::to_owned))
            .filter(|run| ledger::is_valid_run_id(run))
            .filter(|run| {
                std::fs::read_to_string(dir.join(format!("{run}.json")))
                    .ok()
                    .and_then(|text| serde_json::from_str::<Member>(&text).ok())
                    .is_some_and(|member| member.run_id == *run)
            })
            .collect();
        members.sort();
        Ok(members)
    }

    /// Whether `session` has acknowledged this flow.
    ///
    /// # Errors
    ///
    /// An acknowledgement that cannot be read, where none other closes it: it
    /// may be the word that would have.
    pub(crate) fn acknowledged_by(&self, session: &str) -> std::result::Result<bool, String> {
        let dir = self.paths.acknowledgements();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "whether it is closed cannot be said: its acknowledgements cannot be read: \
                     {}: {error}",
                    dir.display()
                ))
            }
        };
        let mut unreadable: Option<String> = None;
        for path in entries
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
        {
            // A writer's temporary sibling is not a record until it is renamed.
            if path.extension().and_then(|suffix| suffix.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|text| {
                    serde_json::from_str::<FlowAcknowledgement>(&text)
                        .map_err(|error| error.to_string())
                }) {
                Ok(acknowledged) => {
                    if acknowledged.flow_id == self.id() && acknowledged.session == session {
                        return Ok(true);
                    }
                }
                Err(why) => unreadable = Some(format!("{}: {why}", path.display())),
            }
        }
        match unreadable {
            Some(why) => Err(format!(
                "whether it is closed cannot be said: an acknowledgement of it cannot be read: \
                 {why}"
            )),
            None => Ok(false),
        }
    }

    /// `onepipeline unwatched --acknowledge-flow ID --reason TEXT`: record that
    /// `session` owes this flow nothing further.
    ///
    /// # Errors
    ///
    /// A blank reason, a flow another session owns, a flow still live — which a
    /// watch is owed, not a closure — and a record that could not be written.
    pub(crate) fn acknowledge(&self, session: &str, reason: &str) -> Result<FlowAcknowledgement> {
        if reason.trim().is_empty() {
            return Err(Error::Invalid(
                "an acknowledgement needs a reason that says something: `--reason` is blank, \
                 and nothing was recorded"
                    .to_owned(),
            ));
        }
        if self.record.session != session {
            return Err(Error::Refused(format!(
                "flow '{}' belongs to session [{}], not to this session; nothing was recorded",
                self.id(),
                sys::session_digest(&self.record.session)
            )));
        }
        if self.is_live() {
            return Err(Error::Invalid(format!(
                "flow '{}' is live, so it is owed a watch rather than a closure: watch it with \
                 `onepipeline watch --flow {}`; nothing was recorded",
                self.id(),
                self.id()
            )));
        }
        let acknowledged = FlowAcknowledgement {
            schema_version: FLOW_SCHEMA_VERSION,
            flow_id: self.id().to_owned(),
            session: session.to_owned(),
            reason: reason.to_owned(),
            at: sys::now_rfc3339(),
        };
        ledger::write_json(
            &self
                .paths
                .acknowledgements()
                .join(format!("{}.json", crate::watchers::nonce())),
            &acknowledged,
        )?;
        Ok(acknowledged)
    }
}

/// Every flow under `root` that `session` owns, in id order.
///
/// Ownership is a positive claim, exactly as a run's is: a flow whose record is
/// absent or cannot be read belongs to nobody and is passed over in silence.
///
/// # Errors
///
/// A flows directory that exists and cannot be read — the question cannot be
/// asked, and answering it as "nothing is owed" is the silence the question
/// exists to end.
pub(crate) fn of_session(root: &Path, session: &str) -> Result<Vec<Flow>> {
    let dir = root.join(FLOWS_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(Error::Ledger {
                path: dir,
                source: error,
            })
        }
    };
    let mut flows: Vec<Flow> = entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|id| Flow::open(root, &id).ok())
        .filter(|flow| flow.record.session == session)
        .collect();
    flows.sort_by(|a, b| a.id().cmp(b.id()));
    Ok(flows)
}

/// The flow this process was started in, as `ONEPIPELINE_FLOW` named it.
static INHERITED: OnceLock<Option<OsString>> = OnceLock::new();

/// A flow id: one that names a flow under the runs root and nothing else, in
/// the alphabet a minted one is spelled in. See [`is_valid_flow_id`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FlowId(String);

impl FlowId {
    /// `text` as a flow id, or nothing where it is not one.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        is_valid_flow_id(text).then(|| Self(text.to_owned()))
    }

    /// The id, as `flow run` printed it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Read `ONEPIPELINE_FLOW` once and take it out of this process's environment,
/// whatever it holds.
///
/// Called by the binary's entry before anything could spawn, so nothing this
/// process starts inherits it — a node dispatch above all — and the one child
/// that has to carry it, a flow's program, is handed it by [`run`] explicitly.
pub(crate) fn take_inherited() {
    if INHERITED
        .get_or_init(|| std::env::var_os(FLOW_ENV))
        .is_some()
    {
        // Before any thread this process starts exists: the binary's entry.
        std::env::remove_var(FLOW_ENV);
    }
}

/// `ONEPIPELINE_FLOW` as this process was started with it, whatever it holds.
fn inherited_as_started() -> Option<&'static OsString> {
    INHERITED
        .get_or_init(|| std::env::var_os(FLOW_ENV))
        .as_ref()
}

/// The flow this process was started in, where `ONEPIPELINE_FLOW` named one: a
/// value that is not a flow id names none.
pub(crate) fn inherited() -> Option<FlowId> {
    FlowId::parse(inherited_as_started()?.to_str()?)
}

/// The live flow `session` is launching in under `root`, where this process was
/// started in one: what `start` records on a run's launch record, and what a
/// nested `flow run` joins.
///
/// A flow of another session, of another runs root, or not live is none — a
/// run launched there belongs to no flow, and is owed its own watch.
pub(crate) fn launching_in(root: &Path, session: &str) -> Option<Flow> {
    let id = inherited()?;
    let flow = Flow::open(root, id.as_str()).ok()?;
    (flow.record.session == session && flow.is_live()).then_some(flow)
}

/// Record `run` as launched in the live flow this process was started in, and
/// answer that flow's id — or nothing, for a run of no flow.
///
/// A membership that cannot be written is said on standard error and leaves the
/// run in no flow: a run a flow watch cannot see must not be counted as watched
/// by it, and judged on its own it is owed a watch of its own.
pub(crate) fn join(root: &Path, run: &str, session: &str) -> Option<FlowId> {
    let flow = launching_in(root, session)?;
    let member = Member {
        schema_version: FLOW_SCHEMA_VERSION,
        run_id: run.to_owned(),
        at: sys::now_rfc3339(),
    };
    match ledger::write_json(
        &members_dir(&flow.paths).join(format!("{run}.json")),
        &member,
    ) {
        Ok(()) => Some(flow.flow_id()),
        // llmlint: ignore-block[changed_behavior_has_e2e] a flow directory the launching
        // process cannot write while its record reads is a host condition no portable journey
        // stages; what it must do is fall back to a run of no flow, which every journey
        // launching outside a flow drives.
        Err(error) => {
            eprintln!(
                "onepipeline: run '{run}' could not be recorded as a member of flow '{}', so it \
                 belongs to no flow and is owed a watch of its own: {error}",
                flow.id()
            );
            None
        } // llmlint: ignore-end[changed_behavior_has_e2e]
    }
}

/// `onepipeline flow run`: run a program as a flow, and exit with its status.
///
/// # Errors
///
/// A `--name` outside the id alphabet, a `--bus-config` this build refuses, and
/// a flow that could not be registered — each before the program starts.
pub(crate) fn run(root: &Path, args: &FlowRunArgs) -> Result<i32> {
    let Some((program, arguments)) = args.program.split_first() else {
        return Err(Error::Invalid(
            "`flow run` needs a program to run, after `--`".to_owned(),
        ));
    };
    let session = args
        .session
        .clone()
        .or_else(|| std::env::var(sys::LAUNCHER_SESSION_ENV).ok())
        .filter(|session| !session.trim().is_empty());
    let Some(session) = session else {
        eprintln!(
            "onepipeline: flow: no session owns this program — neither `--session` nor {} names \
             one — so it runs as no flow, and nothing will hold a session for it",
            sys::LAUNCHER_SESSION_ENV
        );
        // This process's environment, as it was started: the flow it was in, if
        // any, is handed back rather than dropped.
        return supervise(
            program,
            arguments,
            inherited_as_started().map(OsString::as_os_str),
            None,
        );
    };
    if let Some(parent) = launching_in(root, &session) {
        return supervise(
            program,
            arguments,
            Some(std::ffi::OsStr::new(parent.id())),
            None,
        );
    }
    let name = match &args.name {
        Some(name) if is_valid_flow_id(name) => name.clone(),
        Some(name) => {
            return Err(Error::Invalid(format!(
                "`--name {name}` is not a name a flow id can be minted from: letters, digits, \
                 `.`, `_` and `-`; nothing was run"
            )))
        }
        None => name_of(program),
    };
    let bus_config = match &args.bus_config {
        Some(path) => Some(crate::channel::launch_bus_config(path)?.into()),
        None => None,
    };
    let flow = register(root, &name, &session, program, arguments, bus_config)?;
    eprintln!(
        "flow {id}: watch it with: onepipeline watch --flow {id}",
        id = flow.id()
    );
    supervise(
        program,
        arguments,
        Some(std::ffi::OsStr::new(flow.id())),
        Some(&flow),
    )
}

/// The name a flow is minted from when `--name` gives none: the program's file
/// name, every character outside the id alphabet a `-`.
fn name_of(program: &OsString) -> String {
    let named = Path::new(program)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mapped: String = named
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if is_valid_flow_id(&mapped) {
        mapped
    } else {
        "flow".to_owned()
    }
}

/// Mint the flow's id — the first free of `name`, `name-2`, ... — by creating
/// its directory, and write its record.
pub(crate) fn register(
    root: &Path,
    name: &str,
    session: &str,
    program: &OsString,
    arguments: &[OsString],
    bus_config: Option<RecordedBusConfig>,
) -> Result<Flow> {
    let flows = root.join(FLOWS_DIR);
    std::fs::create_dir_all(&flows).map_err(|source| Error::Ledger {
        path: flows.clone(),
        source,
    })?;
    let mut nth: u32 = 1;
    let paths = loop {
        let id = match nth {
            1 => name.to_owned(),
            _ => format!("{name}-{nth}"),
        };
        let paths = paths(root, &id);
        // Created rather than tested, so two flows registering one name at once
        // are two ids: the directory that exists is the claim.
        match std::fs::create_dir(&paths.dir) {
            Ok(()) => break paths,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => nth += 1,
            Err(source) => {
                return Err(Error::Ledger {
                    path: paths.dir,
                    source,
                })
            }
        }
    };
    let pid = sys::pid();
    let record = FlowRecord {
        schema_version: FLOW_SCHEMA_VERSION,
        flow_id: paths.run.clone(),
        session: session.to_owned(),
        program: std::iter::once(program)
            .chain(arguments)
            .map(|part| part.to_string_lossy().into_owned())
            .collect(),
        // A pid of zero is not a process on any platform this builds for; see
        // the watcher record, which keeps its own the same way.
        pid: NonZeroU32::new(pid).unwrap_or(NonZeroU32::MIN),
        host: sys::hostname(),
        started: sys::process_start_token(pid)
            .map(|token| token.recorded().to_string())
            .unwrap_or_default(),
        began_at: sys::now_rfc3339(),
        bus_config,
    };
    // The channel's directory first, so a flow whose record reads has a channel
    // to raise on; the record last, because it is what makes the flow one.
    std::fs::create_dir_all(paths.channel_dir()).map_err(|source| Error::Ledger {
        path: paths.channel_dir(),
        source,
    })?;
    ledger::write_json(&record_path(&paths), &record)?;
    Ok(Flow { paths, record })
}

/// Run the program with `flow` in its environment — or none — forwarding the
/// terminating signals to it, record its ending on `registered` where this
/// process holds a flow, and answer its status.
fn supervise(
    program: &OsString,
    arguments: &[OsString],
    flow: Option<&std::ffi::OsStr>,
    registered: Option<&Flow>,
) -> Result<i32> {
    let mut command = std::process::Command::new(program);
    command.args(arguments);
    match flow {
        Some(id) => command.env(FLOW_ENV, id),
        None => command.env_remove(FLOW_ENV),
    };
    let status = match command.spawn() {
        Ok(mut child) => {
            sys::forward_terminating_signals(child.id());
            match child.wait() {
                Ok(status) => status_of(status),
                // llmlint: ignore-block[changed_behavior_has_e2e] a child this process
                // spawned and then cannot wait for is a host condition no journey stages;
                // it ends like a program that could not be started, recorded and said.
                Err(error) => {
                    eprintln!(
                        "onepipeline: flow: {} could not be waited for: {error}",
                        Path::new(program).display()
                    );
                    NO_STATUS
                } // llmlint: ignore-end[changed_behavior_has_e2e]
            }
        }
        Err(error) => {
            eprintln!(
                "onepipeline: flow: {} could not be started: {error}",
                Path::new(program).display()
            );
            NO_STATUS
        }
    };
    if let Some(flow) = registered {
        record_ending(flow, status);
    }
    Ok(status)
}

/// Record that `flow`'s program ended with `status`, saying on standard error
/// where it could not be.
pub(crate) fn record_ending(flow: &Flow, status: i32) {
    let ending = FlowEnding {
        schema_version: FLOW_SCHEMA_VERSION,
        flow_id: flow.id().to_owned(),
        status,
        at: sys::now_rfc3339(),
    };
    if let Err(error) = ledger::write_json(&ending_path(&flow.paths), &ending) {
        // Unrecorded, the flow reads as died once this process exits, which
        // is owed closure: the safe direction for a status nobody kept.
        eprintln!(
            "onepipeline: flow {}: its ending (status {status}) could not be recorded, so it \
             will read as died: {error}",
            flow.id()
        );
    }
}

/// The status recorded for a program this process has no status of — one that
/// could not be started, or not waited for: the shell's own for a command it
/// could not run.
const NO_STATUS: i32 = 127;

/// A program's exit status as a number: its code, or `128 + the signal` for one
/// a signal ended.
fn status_of(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(NO_STATUS)
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(NO_STATUS)
    }
}

/// `onepipeline surface --flow ID`: raise a surface on the flow's own channel.
///
/// # Errors
///
/// A message with nothing in it once trimmed, and a channel that refuses the
/// push.
pub(crate) fn surface(
    flow: &Flow,
    kind: SurfaceKind,
    message: String,
) -> Result<crate::verbs::Surfaced> {
    let message = message.trim().to_owned();
    if message.is_empty() {
        return Err(Error::Refused(
            "a surface carries what it has to say and this one carried nothing".to_owned(),
        ));
    }
    let source = if kind.as_str() == SurfaceKind::CHECK_IN {
        crate::channel::source::CHECK_IN
    } else {
        crate::channel::source::PROPOSAL
    };
    let queued = flow.channel().push(Surface {
        id: 0,
        kind: kind.as_str().to_string(),
        message,
        source: source.to_string(),
        // A report, as a run's `surface` raises: a blocking question is `ask`'s.
        blocking: false,
        queued_at: sys::now_millis(),
        abandoned: false,
        asker: None,
        workstream: None,
        correlation: None,
    })?;
    Ok(crate::verbs::Surfaced { surface: queued.id })
}

/// `onepipeline next --flow ID`: claim the flow's next surface.
///
/// A flow's event view is its landings' journals, shaped through `filter`; a
/// flow holding no landing answers `events` empty, and nothing is journalled of
/// the claim. With nothing waiting, the status is `running` while the flow is
/// live and `finished` once it is not.
///
/// # Errors
///
/// A channel that refuses the claim.
pub(crate) fn next(
    root: &Path,
    flow: &Flow,
    filter: &crate::filter::EventFilter,
) -> Result<crate::verbs::Next> {
    let (events, _) = crate::flowwatch::events(root, flow, filter, None)?;
    let surface = flow.channel().claim()?;
    let status = match (&surface, flow.is_live()) {
        (Some(_), _) => crate::verbs::NextStatus::Surface,
        (None, true) => crate::verbs::NextStatus::Running,
        (None, false) => crate::verbs::NextStatus::Finished,
    };
    Ok(crate::verbs::Next {
        status,
        surface,
        events,
    })
}

/// `onepipeline reply --flow ID`: answer on the flow's own channel.
///
/// A flow has no graph, so a reply to it is a verdict alone: one carrying
/// commands is refused whole, with nothing queued. The verdict is bound as a
/// run's commandless verdict is — to the question `--correlation` names, or by
/// the channel's own binding — and judged first by every validator the flow's
/// configuration names.
///
/// # Errors
///
/// A malformed envelope, one carrying commands or no verdict, an author or a
/// completion the configuration does not allow, and every refusal the channel
/// makes.
pub(crate) fn reply(
    flow: &Flow,
    correlation: Option<&onemessagebus::Correlation>,
    envelope_json: &str,
) -> Result<crate::verbs::Receipt> {
    let envelope: crate::channel::Reply = serde_json::from_str(envelope_json.trim())
        .map_err(|error| Error::Refused(format!("the reply is malformed: {error}")))?;
    if !envelope.commands.is_empty() {
        return Err(Error::Refused(format!(
            "flow '{}' has no graph, so a reply to it carries a verdict alone — `completion`, \
             `message` or `reason` — and no commands; nothing was queued",
            flow.id()
        )));
    }
    if !envelope.carries_verdict() {
        return Err(Error::Refused(format!(
            "the reply to flow '{}' carries no verdict — no `completion`, `message` or `reason` \
             — so there is nothing to answer with; nothing was queued",
            flow.id()
        )));
    }
    let channel = flow.channel();
    channel.declares(&envelope.author)?;
    channel.allows_completion(envelope.author.clone(), envelope.completion)?;
    channel.judge_reply(&envelope, None::<crate::edits::EnvelopeReview>)?;
    let reply = channel.answer_bound(&envelope, correlation)?;
    Ok(crate::verbs::Receipt {
        outcome: crate::verbs::ReplyOutcome::Answered {
            reply,
            verdict: crate::verbs::VerdictHalf::OnTheQueue,
        },
        advice: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flow id is the alphabet a minted one is spelled in, and nothing that
    /// navigates: what `--flow` is handed is joined onto the runs root.
    #[test]
    fn a_flow_id_is_the_minted_alphabet_and_nothing_that_navigates() {
        for id in ["plan", "plan-2", "finish.plan", "a_b"] {
            assert!(is_valid_flow_id(id), "{id}");
        }
        for id in ["", ".", "..", "a/b", "a\\b", "a:b", "a b", "../x"] {
            assert!(!is_valid_flow_id(id), "{id}");
        }
        assert_eq!(name_of(&OsString::from("/usr/bin/just")), "just");
        assert_eq!(name_of(&OsString::from("./my tool")), "my-tool");
        assert_eq!(name_of(&OsString::from("..")), "flow");
    }

    /// Entry 123 of `docs/contract-divergences.md` records the landing sidecars
    /// and payloads, and this is what holds that record to the types declared
    /// here: every example reads and writes back byte for byte, and each field's
    /// own rule refuses what it should.
    #[test]
    fn the_landing_declarations_are_what_the_divergence_record_names() {
        let record = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs")
                .join("contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("123."))
            .expect("entry 123 is recorded");
        let block: serde_json::Value = serde_json::from_str(
            entry
                .split("```json")
                .nth(1)
                .and_then(|rest| rest.split("```").next())
                .expect("entry 123 carries its block"),
        )
        .expect("the block is JSON");

        let request: LandingRequest =
            serde_json::from_value(block["request"].clone()).expect("the request reads");
        assert_eq!(
            serde_json::to_value(&request).expect("it writes"),
            block["request"]
        );
        assert_eq!(request.operation, LandingOperation::Integrate);
        assert_eq!(request.subjects.len(), 3, "a train names every member");
        for (key, value) in [
            ("schema_version", serde_json::json!(2)),
            ("session", serde_json::json!(" ")),
            ("landing_id", serde_json::json!("")),
            ("landing_id", serde_json::json!("landing:1@2")),
            ("host", serde_json::json!(" ")),
            ("flow_id", serde_json::json!("a/b")),
            ("subjects", serde_json::json!([])),
            ("operation", serde_json::json!("rebase")),
            ("runs", serde_json::json!(["build", "build"])),
            ("runs", serde_json::json!(["a/b"])),
            ("pid", serde_json::json!(0)),
            ("began_at", serde_json::json!("today")),
            (
                "subjects",
                serde_json::json!([{"identity": "not absolute", "branch": "a"}]),
            ),
            (
                "subjects",
                serde_json::json!([{"identity": "service", "branch": "a"}]),
            ),
            (
                "subjects",
                serde_json::json!([{"identity": "https://github.com/o/s", "branch": "a"}]),
            ),
            (
                "subjects",
                serde_json::json!([{"identity": "/abs", "branch": " "}]),
            ),
            (
                "subjects",
                serde_json::json!([{"identity": "/abs", "branch": "a", "tip": "x"}]),
            ),
            ("watching", serde_json::json!(true)),
        ] {
            let mut document = block["request"].clone();
            document[key] = value.clone();
            assert!(
                serde_json::from_value::<LandingRequest>(document).is_err(),
                "a request with {key}={value} was read"
            );
        }
        // A host with no `.` in its name is one `onevcs` keys by as readily.
        let mut single = block["request"].clone();
        single["subjects"] = serde_json::json!([{"identity": "git/owner/repo", "branch": "a"}]);
        serde_json::from_value::<LandingRequest>(single).expect("a single-label host reads");
        // An absolute local identity may carry what an origin never does.
        let mut local = block["request"].clone();
        local["subjects"] = serde_json::json!([{"identity": "/home/a b/service", "branch": "a"}]);
        serde_json::from_value::<LandingRequest>(local).expect("an absolute local identity reads");

        let ending: LandingEnding =
            serde_json::from_value(block["ending"].clone()).expect("the ending reads");
        assert_eq!(
            serde_json::to_value(&ending).expect("it writes"),
            block["ending"]
        );
        let mut unknown = block["ending"].clone();
        unknown["signal"] = serde_json::json!(9);
        assert!(serde_json::from_value::<LandingEnding>(unknown).is_err());

        let operation: LandingOperationRecord =
            serde_json::from_value(block["operation"].clone()).expect("the operation reads");
        assert_eq!(
            serde_json::to_value(&operation).expect("it writes"),
            block["operation"]
        );
        let mut blank = block["operation"].clone();
        blank["operation_id"] = serde_json::json!("");
        assert!(serde_json::from_value::<LandingOperationRecord>(blank).is_err());

        let transition: LandingTransition =
            serde_json::from_value(block["transition"].clone()).expect("the transition reads");
        assert_eq!(
            serde_json::to_value(&transition).expect("it writes"),
            block["transition"]
        );
        let terminal: LandingTerminal =
            serde_json::from_value(block["terminal"].clone()).expect("the terminal reads");
        assert_eq!(
            serde_json::to_value(&terminal).expect("it writes"),
            block["terminal"]
        );

        // Each kind's significance is the one the block records, and its terminal
        // kinds are the ones a reader ends a landing on.
        for kind in crate::event::PIPELINE_KINDS {
            let recorded = &block["significance"][kind.as_str()];
            match landing_significance(*kind) {
                Some(stamped) => assert_eq!(
                    &serde_json::to_value(stamped).expect("a word"),
                    recorded,
                    "{kind}"
                ),
                None => assert!(recorded.is_null(), "{kind} is not a landing kind"),
            }
            let terminal = block["terminal_kinds"]
                .as_array()
                .expect("terminal kinds")
                .iter()
                .any(|named| named == kind.as_str());
            assert_eq!(is_terminal_landing_kind(kind.as_str()), terminal, "{kind}");
        }
    }

    /// Every document a flow keeps is read closed: another version, a key this
    /// build does not know, a blank session and a stamp that is not an instant
    /// are each refused.
    #[test]
    fn a_flow_document_this_build_did_not_write_is_refused() {
        let record = serde_json::json!({
            "schema_version": FLOW_SCHEMA_VERSION,
            "flow_id": "plan",
            "session": "a-session",
            "program": ["just", "plan"],
            "pid": 4_242,
            "host": "a-host",
            "started": "linux-proc-stat:1",
            "began_at": "2026-10-10T00:00:00.000Z",
        });
        let read: FlowRecord = serde_json::from_value(record.clone()).expect("a record");
        assert_eq!(
            serde_json::to_value(&read).expect("it serializes"),
            record,
            "a record without a bus configuration omits it"
        );
        for (key, value) in [
            ("schema_version", serde_json::json!(2)),
            ("session", serde_json::json!(" ")),
            ("began_at", serde_json::json!("yesterday")),
            ("watching", serde_json::json!(true)),
        ] {
            let mut document = record.clone();
            document[key] = value;
            serde_json::from_value::<FlowRecord>(document)
                .expect_err("a record this build did not write is refused");
        }
        let ending = serde_json::json!({
            "schema_version": FLOW_SCHEMA_VERSION,
            "flow_id": "plan",
            "status": 3,
            "at": "2026-10-10T00:00:00.000Z",
        });
        serde_json::from_value::<FlowEnding>(ending.clone()).expect("an ending");
        let mut unknown = ending;
        unknown["signal"] = serde_json::json!(9);
        serde_json::from_value::<FlowEnding>(unknown).expect_err("an unknown key is refused");
    }
}
