//! The envelopes and parameter shapes of `docs/plugin-protocol.md`.
//!
//! Every type here is a restatement of that document, and every *work* type inside one
//! comes from `onetaskgraph-plugin-api` unchanged — the protocol says the JSON shape of a
//! contract type "is what `onetaskgraph schema` emits for the type of the same name", so
//! re-spelling `Task` or `Capabilities` here would create a second place the contract
//! lives and a way for the two to disagree. What this module adds is only what the
//! contract has no type for: the envelope, the handshake, and the per-method wrappers.
//!
//! Nothing here uses `deny_unknown_fields`, and that is §2.1 rather than an oversight: a
//! reader on either side ignores members it does not know, which is what lets a later
//! version add an optional field without a version bump.

use std::collections::BTreeMap;
use std::path::Path;

use onetaskgraph_plugin_api::{
    AssetWrite, Capabilities, Comment, CommentBody, CommentCopy, Direction, Document,
    DocumentQuery, IssueHolding, IssueRef, ItemKind, ItemWrite, MetadataKey, Metering, NativeId,
    NewComment, Page, PageRequest, Priority, Project, ProjectQuery, SourceError, Status,
    StatusCategory, StatusContradiction, Task, TaskDetailRead, TaskQuery, TaskRef, TaskUpdate,
    TaskUpdateOutcome, WriteSupport,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version this build speaks. `docs/plugin-protocol.md` specifies 2.
///
/// Version 2 added `delete_task` and `delete_project` (§4.10). The handshake matches
/// versions exactly, so a plugin speaking 1 is refused by name rather than being asked
/// for a method it has never heard of halfway through undoing a copy.
pub(crate) const PROTOCOL_VERSION: u32 = 2;

/// Every status category this build knows, in the order the vocabulary lists them (§3.5).
pub(crate) const STATUS_VOCABULARY: [StatusCategory; 8] = [
    StatusCategory::Draft,
    StatusCategory::Backlog,
    StatusCategory::Todo,
    StatusCategory::Queued,
    StatusCategory::InProgress,
    StatusCategory::Done,
    StatusCategory::Cancelled,
    StatusCategory::Unknown,
];

/// Whether `category` is one a peer that lists no statuses was not written against (§3.5).
///
/// A wildcard-free match, so a category added to the vocabulary does not compile until
/// somebody decides here whether a peer that predates it can be handed it.
pub(crate) const fn after_the_first_vocabulary(category: StatusCategory) -> bool {
    match category {
        StatusCategory::Queued => true,
        StatusCategory::Draft
        | StatusCategory::Backlog
        | StatusCategory::Todo
        | StatusCategory::InProgress
        | StatusCategory::Done
        | StatusCategory::Cancelled
        | StatusCategory::Unknown => false,
    }
}

/// One category as the wire spells it.
pub(crate) fn spelled(category: StatusCategory) -> String {
    serde_json::to_value(category)
        .ok()
        .and_then(|word| word.as_str().map(str::to_owned))
        .expect("a status category serialises as its own word")
}

/// Every category this build knows, as a handshake lists them.
pub(crate) fn vocabulary() -> Vec<String> {
    STATUS_VOCABULARY.into_iter().map(spelled).collect()
}

/// Whether a handshake's `statuses` member lists every category this build knows.
///
/// An absent member is the first vocabulary, so it lists none of the categories added
/// after it.
pub(crate) fn knows_every_category(statuses: Option<&[String]>) -> bool {
    let Some(listed) = statuses else {
        return false;
    };
    STATUS_VOCABULARY
        .into_iter()
        .filter(|category| after_the_first_vocabulary(*category))
        .all(|category| listed.contains(&spelled(category)))
}

/// One request line: `{ "id": …, "method": …, "params": … }` (§2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Request {
    /// Unique within the connection, echoed in the response.
    pub(crate) id: String,
    /// One of the names in §4, or `initialize`.
    pub(crate) method: String,
    /// Present even when empty, which is why it is not `Option`.
    pub(crate) params: Value,
}

/// One response line: an `id` and exactly one of `result` and `error` (§2).
///
/// Both members are optional here so that "both present" and "neither present" are
/// *representable* — they are the protocol violations §6.3 names, and a shape that could
/// not hold them would turn a violation into a parse error that says something else.
/// [`Response::outcome`] is where the pair is reduced to the one thing it may be.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Response {
    /// The request's `id`, echoed.
    pub(crate) id: String,
    /// Present on success.
    ///
    /// A `result` of `null` is present, not absent: `set_project_status` (§4.17a) answers
    /// `null` for no such project, and reading it as "neither" would refuse that answer.
    #[serde(
        default,
        deserialize_with = "held_whatever_its_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) result: Option<Value>,
    /// Present on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<SourceError>,
}

/// A member that is there, whatever its value — `null` included.
///
/// `#[serde(default)]` supplies `None` only when the member is missing, so this is called
/// exactly when it is present.
fn held_whatever_its_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl Response {
    /// A response carrying a result.
    pub(crate) fn ok(id: String, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    /// A response carrying a failure.
    pub(crate) fn failed(id: String, error: SourceError) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }

    /// The one thing this envelope may say, or `None` when it says both or neither.
    ///
    /// Returning `None` rather than a guess is §6.3: an envelope with both members is a
    /// protocol violation, and picking one of them would run the caller against a shape
    /// the peer did not mean.
    pub(crate) fn outcome(self) -> Option<Result<Value, SourceError>> {
        match (self.result, self.error) {
            (Some(result), None) => Some(Ok(result)),
            (None, Some(error)) => Some(Err(error)),
            _ => None,
        }
    }
}

/// `initialize` parameters (§3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InitializeParams {
    /// The version the engine is speaking.
    pub(crate) protocol_version: u32,
    /// For the plugin's diagnostics only.
    pub(crate) engine: EngineIdentity,
    /// The configured name — for error messages, and for recognising itself in a
    /// recorded qualified id (§3.2).
    pub(crate) source_name: String,
    /// This source's settings, verbatim.
    pub(crate) config: Value,
    /// Only the variables this plugin's configuration names (§3.1).
    pub(crate) secrets: BTreeMap<String, String>,
    /// The status categories this engine knows (§3.5).
    ///
    /// Optional, and absent means the first vocabulary: an engine written before `queued`
    /// sends none, and a plugin answering it reports a category outside that vocabulary as
    /// `unknown`, keeping its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) statuses: Option<Vec<String>>,
    /// The absolute directory holding the configuration document that supplied this
    /// source's block (§3).
    ///
    /// Optional, and absent when the block came from no one document — the environment
    /// layer or a flag — in which case a relative path the plugin declares keeps resolving
    /// against the working directory. A plugin written before the member ignores it (§2.1)
    /// and behaves exactly as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) document_dir: Option<DocumentDir>,
}

/// `document_dir` (§3.8): an absolute directory whose name is a string.
///
/// Checked wherever one is made — by the engine from the configuration layer, and by the
/// reference host from the wire — so a relative one cannot be sent or measured from. A
/// relative directory would be measured from the child's working directory, which is the
/// very answer the member exists to replace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub(crate) struct DocumentDir(String);

impl DocumentDir {
    /// `directory`, or why it cannot be sent as one.
    pub(crate) fn new(directory: &Path) -> Result<Self, String> {
        let spelled = directory.to_str().ok_or_else(|| {
            format!(
                "the document directory {} is not valid UTF-8, so it cannot be written into \
                 the handshake",
                directory.display()
            )
        })?;
        if !directory.is_absolute() {
            return Err(format!(
                "the document directory {spelled:?} is not an absolute path, so there is no \
                 one directory to measure this source's relative paths from"
            ));
        }
        Ok(Self(spelled.to_owned()))
    }

    /// The directory itself.
    pub(crate) fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl TryFrom<String> for DocumentDir {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(Path::new(&value))
    }
}

impl From<DocumentDir> for String {
    fn from(value: DocumentDir) -> Self {
        value.0
    }
}

/// Who is asking, for the plugin's diagnostics (§3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EngineIdentity {
    /// The engine's name.
    pub(crate) name: String,
    /// The engine's own version. Advisory.
    pub(crate) version: String,
}

/// The `initialize` result (§3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InitializeResult {
    /// The version the plugin will speak.
    ///
    /// Optional so that an omitted one is a refusal this engine can *name* — §6.2 makes
    /// omitting it and answering in another version the same failure, and a required
    /// field would surface it as a parse error naming neither version.
    #[serde(default)]
    pub(crate) protocol_version: Option<u32>,
    /// The plugin kind, as the plugin reports it.
    pub(crate) kind: HandshakePluginKind,
    /// Read once; the engine does not ask again.
    pub(crate) capabilities: Capabilities,
    /// Whether this plugin can be written through (§3.3).
    ///
    /// Optional, and absent means [`WriteSupport::Unsupported`]: §2.1 lets a later
    /// version add a member without a version bump, and a plugin written against version
    /// 1 before there was a write side says nothing here and is read as the read-only
    /// source it is.
    #[serde(default)]
    pub(crate) writes: Option<WriteSupport>,
    /// Whether this plugin answers `metering` (§3.4).
    ///
    /// Optional, and absent means it does not: a plugin written before there was metering
    /// says nothing here, is never sent the method, and is reported as not metering.
    #[serde(default)]
    pub(crate) meters: bool,
    /// The status categories this plugin knows (§3.5).
    ///
    /// Optional, and absent means the first vocabulary — the seven categories before
    /// `queued` — so a plugin written before there was a `queued` is never handed one.
    /// Strings rather than categories, so a plugin listing a category this engine has never
    /// heard of is read rather than refused.
    #[serde(default)]
    pub(crate) statuses: Option<Vec<String>>,
    /// Whether this plugin answers `set_task_status` and `set_delivered_by`, and holds a
    /// task's `delivers` and `delivered_by` (§3.6).
    ///
    /// Optional, and absent means it does not: such a plugin is never sent either method, and
    /// never handed a task carrying either list.
    #[serde(default)]
    pub(crate) task_updates: bool,
    /// Whether this plugin answers `set_task_metadata`, `set_project_metadata` and
    /// `set_document_metadata` (§3.7).
    ///
    /// Optional, and absent means it does not: such a plugin is never sent any of the three,
    /// and each is refused by name before anything is sent.
    #[serde(default)]
    pub(crate) metadata_updates: bool,
    /// Whether this plugin answers `set_task_content` (§3.9).
    ///
    /// Optional, and absent means it does not: such a plugin is never sent the method, and a
    /// content write naming it is refused by name before anything is sent.
    #[serde(default)]
    pub(crate) content_updates: bool,
    /// Whether this plugin answers `update_task` (§3.10).
    ///
    /// Optional, and absent means it does not: such a plugin is never sent the method, and a
    /// targeted update naming it is carried out through the reads and `write_task` it already
    /// answers — correct, and merely not minimal.
    #[serde(default)]
    pub(crate) targeted_updates: bool,
    /// Whether this plugin answers `end_command` (§3.11).
    ///
    /// Optional, and absent means it does not: such a plugin is never sent the method, which
    /// §3.11 lets only a plugin holding nothing between requests that a person can change
    /// leave out. Omitted when `false`, so what a plugin answering no such thing writes is
    /// what it wrote before the member existed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) ends_commands: bool,
    /// Whether this plugin answers `write_comment_copy` (§3.12).
    ///
    /// Optional, and absent means it does not: such a plugin is never sent the method, and a
    /// comment a copy carries to it is written through `add_comment` and `edit_comment` with
    /// the origin's hidden marker appended to its body (§4.16a). Omitted when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) comment_copies: bool,
    /// Whether this plugin answers `get_task_detail` (§3.13).
    ///
    /// Optional, and absent means it does not: such a plugin is never sent the method, and a
    /// task and a page of its comments are read from it through `get_task` and `task_comments`,
    /// one call each. Omitted when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) task_details: bool,
    /// Whether this plugin answers `check_task_write` (§3.14).
    ///
    /// Optional, and absent means it does not: such a plugin is never sent the method, a copy
    /// reads every task it plans as one it can represent, and a write it refuses as one it
    /// cannot takes the whole copy back, as before. Omitted when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) write_checks: bool,
    /// Whether this plugin answers `set_project_status` (§3.15).
    ///
    /// Optional, and absent means it does not: such a plugin is never sent the method, and a
    /// project status write naming it is refused before anything is sent, in the words the
    /// contract's default refuses it with. Omitted when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) project_status: bool,
    /// Whether this plugin answers `stored_status`, and says in each content write's result
    /// what its backend stores (§3.16).
    ///
    /// Optional, and absent means it does not: such a plugin is never sent `stored_status`, the
    /// engine writes the status it was given, and nothing is written again after a write.
    /// Omitted when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) stored_forms: bool,
}

/// The `metering` result (§4.14).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MeteringResult {
    /// The plugin's running totals, or `null` when it has none to give.
    #[serde(default)]
    pub(crate) metering: Option<Metering>,
}

/// A plugin's non-empty, open-vocabulary kind from the handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct HandshakePluginKind(String);

impl HandshakePluginKind {
    /// Validate a kind at the process boundary.
    pub(crate) fn new(kind: impl Into<String>) -> Result<Self, &'static str> {
        let kind = kind.into();
        if kind.trim().is_empty() {
            Err("plugin kind must contain a non-whitespace character")
        } else {
            Ok(Self(kind))
        }
    }

    /// Recover the peer's spelling after it has been validated.
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

impl TryFrom<String> for HandshakePluginKind {
    type Error = &'static str;

    fn try_from(kind: String) -> Result<Self, Self::Error> {
        Self::new(kind)
    }
}

impl From<HandshakePluginKind> for String {
    fn from(kind: HandshakePluginKind) -> Self {
        kind.0
    }
}

/// `get_task` and `get_project` parameters (§4.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct IdParams {
    /// The source's own opaque id.
    pub(crate) id: NativeId,
}

/// The `get_task` result (§4.4): a task, or `null` when there is no such task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TaskResult {
    /// The task, or `null`.
    #[serde(default)]
    pub(crate) task: Option<Task>,
}

/// The `get_project` result (§4.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProjectResult {
    /// The project, or `null`.
    #[serde(default)]
    pub(crate) project: Option<Project>,
}

/// The `get_document` result (§4.11): a document, or `null` when there is no such one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DocumentResult {
    /// The document, or `null`.
    #[serde(default)]
    pub(crate) document: Option<Document>,
}

/// `query_tasks` parameters (§4.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TaskQueryParams {
    /// The predicates the plugin is being asked to apply.
    pub(crate) query: TaskQuery,
    /// Where to resume and how much to return.
    pub(crate) page: PageRequest,
}

/// `query_projects` parameters (§4.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProjectQueryParams {
    /// The predicates the plugin is being asked to apply.
    pub(crate) query: ProjectQuery,
    /// Where to resume and how much to return.
    pub(crate) page: PageRequest,
}

/// `labels` parameters (§4.7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LabelParams {
    /// Where to resume and how much to return.
    pub(crate) page: PageRequest,
}

/// `query_documents` parameters (§4.11).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DocumentQueryParams {
    /// The predicates the plugin is being asked to apply.
    pub(crate) query: DocumentQuery,
    /// Where to resume and how much to return.
    pub(crate) page: PageRequest,
}

/// `task_dependencies` and `project_dependencies` parameters (§4.8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DependencyParams {
    /// The item whose edges are wanted.
    pub(crate) id: NativeId,
    /// Which way to walk.
    pub(crate) direction: Direction,
    /// Where to resume and how much to return.
    pub(crate) page: PageRequest,
}

/// `write_task` parameters (§4.9).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TaskWriteParams {
    /// The item to create or update, and what to write into it.
    pub(crate) write: ItemWrite<Task>,
}

/// The params of a `write_task` that carries a record's image assets: the write, and beside
/// it the two members of [`AssetWrite`] — `assets` and `recorded_assets` — which are the
/// in-process trait's own argument, flattened, so the two cannot spell a field differently.
///
/// A plain write never carries the `assets` member at all, which is what lets a plugin written
/// before assets keep reading every write it is sent; one that carries it, even empty, is a
/// write of the record's whole asset set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TaskAssetWriteParams {
    pub(crate) write: ItemWrite<Task>,
    #[serde(flatten)]
    pub(crate) assets: AssetWrite,
}

/// [`TaskAssetWriteParams`] for a document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DocumentAssetWriteParams {
    pub(crate) write: ItemWrite<Document>,
    #[serde(flatten)]
    pub(crate) assets: AssetWrite,
}

/// A `write_task` or `write_document` as the reference host reads it: a plain write, or —
/// when `assets` is present — one carrying the record's whole asset set.
///
/// Read through [`RawServedWrite`], so a write carrying `recorded_assets` without `assets` —
/// a shape §4.9a does not have — is refused as malformed where it is decoded rather than
/// represented here.
#[derive(Debug, Clone, Deserialize)]
#[serde(
    try_from = "RawServedWrite<T>",
    bound(deserialize = "T: Deserialize<'de>")
)]
pub(crate) struct ServedWriteParams<T> {
    pub(crate) write: ItemWrite<T>,
    /// The record's assets, for a write that carries them.
    pub(crate) assets: Option<AssetWrite>,
}

/// [`ServedWriteParams`] exactly as the line spells it, before its members are checked
/// against one another.
#[derive(Debug, Clone, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub(crate) struct RawServedWrite<T> {
    write: ItemWrite<T>,
    /// Absent is a plain write; `null` is not a shape §4.9a has, and is refused.
    #[serde(default, deserialize_with = "not_null")]
    assets: Option<Vec<onetaskgraph_plugin_api::AssetPayload>>,
    #[serde(default, deserialize_with = "not_null")]
    recorded_assets: Option<onetaskgraph_plugin_api::AssetUploads>,
}

/// An optional member that, when present, is its value: absent reads as `None` through
/// `#[serde(default)]`, and an explicit `null` is refused as the shape it is not.
fn not_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl<T> TryFrom<RawServedWrite<T>> for ServedWriteParams<T> {
    type Error = String;

    fn try_from(raw: RawServedWrite<T>) -> Result<Self, Self::Error> {
        let assets = match (raw.assets, raw.recorded_assets) {
            (None, Some(_)) => {
                return Err(
                    "a write carrying `recorded_assets` names its assets in `assets` \
                            (docs/plugin-protocol.md §4.9a)"
                        .to_owned(),
                );
            }
            (None, None) => None,
            (Some(assets), recorded_assets) => Some(AssetWrite {
                assets,
                recorded_assets,
            }),
        };
        Ok(Self {
            write: raw.write,
            assets,
        })
    }
}

/// `write_project` parameters (§4.9).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProjectWriteParams {
    /// The item to create or update, and what to write into it.
    pub(crate) write: ItemWrite<Project>,
}

/// `write_document` parameters (§4.12).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DocumentWriteParams {
    /// The document to create or update, and what to write into it.
    pub(crate) write: ItemWrite<Document>,
}

/// `delete_task`, `delete_project` and `delete_document` parameters (§4.10, §4.12).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DeleteParams {
    /// The destination item to remove.
    pub(crate) id: NativeId,
}

/// `task_comments` parameters (§4.15).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CommentsParams {
    /// The task whose comments are wanted.
    pub(crate) task: NativeId,
    /// Where to resume and how much to return.
    pub(crate) page: PageRequest,
}

/// `get_task_detail` parameters (§4.4a).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TaskDetailParams {
    /// The task.
    pub(crate) id: NativeId,
    /// The page of its comments wanted beside it.
    pub(crate) comments: PageRequest,
}

/// The `get_task_detail` result (§4.4a): the task and that page of its comments, read as one
/// revision of it — both `null` when there is no such task.
// llmlint: ignore-block[invalid_states_unrepresentable] This is the §4.4a answer as it crosses the wire, where each member is optional so a plugin can omit what it does not hold; the contradictory shapes the decoder admits never leave this module, because `TaskDetailResult::into_read` refuses each of them as malformed before the engine sees a read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TaskDetailResult {
    /// The task, or `null`.
    #[serde(default)]
    pub(crate) task: Option<Task>,
    /// The page of its comments, or `null` with the task — and `null` beside a task whose
    /// comments could not be read, the refusal then in `comments_error`.
    #[serde(default)]
    pub(crate) page: Option<Page<Comment>>,
    /// Why the task's comments could not be read, when the task itself could — a board draft,
    /// which has none — so the task is still answered. Omitted when they were read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) comments_error: Option<SourceError>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl TaskDetailResult {
    /// The read the answer states, refused as malformed when its members contradict each other:
    /// no task beside a page or a comment refusal, or a task with neither or both — the three
    /// shapes the wire allows and §4.4a does not.
    pub(crate) fn into_read(self) -> Result<Option<TaskDetailRead>, SourceError> {
        let comments = match (self.task.is_some(), self.page, self.comments_error) {
            (false, None, None) => return Ok(None),
            (true, Some(page), None) => Ok(Some(page)),
            (true, None, Some(error)) => Err(error),
            (false, ..) => {
                return Err(SourceError::Malformed {
                    message: "get_task_detail answered comments for no task".into(),
                });
            }
            (true, page, _) => {
                return Err(SourceError::Malformed {
                    message: if page.is_some() {
                        "get_task_detail answered both a page and a comment refusal"
                    } else {
                        "get_task_detail answered a task with neither a page nor a comment refusal"
                    }
                    .into(),
                });
            }
        };
        Ok(self.task.map(|task| TaskDetailRead {
            task,
            comments: Some(comments),
        }))
    }
}

/// The `task_comments` result (§4.15): a page, or `null` when there is no such task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CommentsResult {
    /// The page, or `null`.
    #[serde(default)]
    pub(crate) page: Option<Page<Comment>>,
}

/// `add_comment` parameters (§4.16).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AddCommentParams {
    /// The task to comment on.
    pub(crate) task: NativeId,
    /// What to add.
    pub(crate) comment: NewComment,
}

/// `edit_comment` parameters (§4.16).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EditCommentParams {
    /// The task the comment is on.
    pub(crate) task: NativeId,
    /// The comment to edit.
    pub(crate) comment: NativeId,
    /// Its new body.
    pub(crate) body: CommentBody,
}

/// `write_comment_copy` parameters (§4.16a).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CommentCopyParams {
    /// The task the comment is on.
    pub(crate) task: NativeId,
    /// The comment a copy wrote before and now makes say `copy`, or `null` for a new one.
    #[serde(default)]
    pub(crate) existing: Option<NativeId>,
    /// What it says, and the comment it is a copy of.
    pub(crate) copy: CommentCopy,
}

/// `delete_comment` parameters (§4.16).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DeleteCommentParams {
    /// The task the comment is on.
    pub(crate) task: NativeId,
    /// The comment to remove.
    pub(crate) comment: NativeId,
}

/// The `add_comment` and `edit_comment` result (§4.16): the comment as the source now holds
/// it, or `null` when there is no such task or no such comment on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CommentResult {
    /// The comment, or `null`.
    #[serde(default)]
    pub(crate) comment: Option<Comment>,
}

/// The `delete_comment` result (§4.16): the id removed, or `null` when there was nothing
/// under it to remove.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DeletedCommentResult {
    /// The removed comment's id, or `null`.
    #[serde(default)]
    pub(crate) deleted: Option<NativeId>,
}

/// `set_task_status` parameters (§4.17).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StatusParams {
    /// The task whose status is set.
    pub(crate) id: NativeId,
    /// The category to set it to.
    pub(crate) category: StatusCategory,
}

/// The `set_task_status` result (§4.17): the status as the plugin now reads it, or `null`
/// when there is no such task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StatusResult {
    /// The status, or `null`.
    #[serde(default)]
    pub(crate) status: Option<Status>,
}

/// `check_task_write` parameters (§4.9b): the write a copy plans, asked about before it is
/// sent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CheckWriteParams {
    /// The write.
    pub(crate) write: ItemWrite<Task>,
}

/// The `status_contradictions` result (§4.23): every task whose card contradicts its issue's
/// closure, with the status that brings it in line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ContradictionsResult {
    /// The tasks, in the order the plugin read them.
    pub(crate) items: Vec<StatusContradiction>,
}

/// `issue_holdings` parameters (§4.24).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HoldingsParams {
    /// The issue references, each `owner/name#N`, once each and in the order asked.
    pub(crate) issues: Vec<IssueRef>,
}

/// The `issue_holdings` result (§4.24): one holding per reference, in the order asked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HoldingsResult {
    /// The holdings.
    pub(crate) holdings: Vec<IssueHolding>,
}

/// `set_task_priority` parameters (§4.19).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PriorityParams {
    /// The task whose priority is set.
    pub(crate) id: NativeId,
    /// The priority to set it to; `none` clears it.
    pub(crate) priority: Priority,
}

/// The `set_task_priority` result (§4.19): the priority as the plugin now reads it, or
/// `null` when there is no such task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PriorityResult {
    /// The priority, or `null`. Required: an answer without the member says nothing about
    /// whether the task is there, so it is malformed rather than read as `null`.
    #[serde(deserialize_with = "present")]
    pub(crate) priority: Option<Priority>,
}

/// `set_task_content` parameters (§4.20).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ContentParams {
    /// The task whose content is replaced.
    pub(crate) id: NativeId,
    /// What it holds afterwards, byte for byte.
    pub(crate) content: String,
}

/// The `set_task_content` result (§4.20): the id of the task written, or `null` when there is
/// no such task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ContentResult {
    /// The task's id, or `null`. Required, for the reason [`PriorityResult::priority`] is.
    #[serde(deserialize_with = "present")]
    pub(crate) id: Option<NativeId>,
}

/// `update_task` parameters (§4.21).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UpdateParams {
    /// The task updated.
    pub(crate) id: NativeId,
    /// Every field to write, and nothing else.
    pub(crate) update: TaskUpdate,
}

/// The `update_task` result (§4.21): what the update came to, or `null` when there is no such
/// task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UpdateResult {
    /// The outcome, or `null`. Required, for the reason [`PriorityResult::priority`] is.
    #[serde(deserialize_with = "present")]
    pub(crate) outcome: Option<TaskUpdateOutcome>,
}

/// A member that may be `null` and may not be absent.
///
/// Serde reads an absent `Option` member as `None` of its own accord; naming a deserializer
/// takes that away, so an answer that leaves the member out is refused as the malformed
/// answer it is rather than read as one saying there is no such task.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// `set_delivered_by` parameters (§4.17).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DeliveredByParams {
    /// The task whose list is replaced.
    pub(crate) id: NativeId,
    /// The whole list it holds afterwards.
    pub(crate) delivered_by: Vec<TaskRef>,
}

/// The `set_delivered_by` result (§4.17): the list the task now holds, or `null` when there
/// is no such task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DeliveredByResult {
    /// The list, or `null`.
    #[serde(default)]
    pub(crate) delivered_by: Option<Vec<TaskRef>>,
}

/// `set_task_metadata`, `set_project_metadata` and `set_document_metadata` parameters
/// (§4.18).
///
/// Built only through [`MetadataParams::new`], which deserialization goes through too, so no
/// value of this type holds [`MetadataKey::COPIES_KEY`] with a value that is not links.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "UncheckedMetadataParams")]
pub(crate) struct MetadataParams {
    /// The record whose metadata key is set.
    id: NativeId,
    /// The key: a caller's own dotted key outside the `onetaskgraph.` namespace, or one of the
    /// two reserved keys a copy records where an item landed under, [`MetadataKey::COPIES_KEY`]
    /// and [`MetadataKey::COPY_LOCATIONS_KEY`].
    key: MetadataKey,
    /// The value to hold under it: any JSON, `null` included — and for
    /// [`MetadataKey::COPIES_KEY`], only an object of destination source names to qualified
    /// ids of that source, and for [`MetadataKey::COPY_LOCATIONS_KEY`] only one of destination
    /// source names to an id and a location.
    value: Value,
}

impl MetadataParams {
    /// One narrow metadata write, once its value is one its key may hold.
    ///
    /// # Errors
    ///
    /// Returns why, when `key` is [`MetadataKey::COPIES_KEY`] and `value` is not links.
    pub(crate) fn new(id: NativeId, key: MetadataKey, value: Value) -> Result<Self, String> {
        if key.is_copies()
            && let Some(malformed) = crate::engine::malformed_links(&value)
        {
            return Err(malformed);
        }
        if key.is_copy_locations()
            && let Some(malformed) = crate::engine::malformed_locations(&value)
        {
            return Err(malformed);
        }
        Ok(Self { id, key, value })
    }

    /// The record whose metadata key is set.
    pub(crate) fn id(&self) -> &NativeId {
        &self.id
    }

    /// The key.
    pub(crate) fn key(&self) -> &MetadataKey {
        &self.key
    }

    /// The value to hold under it.
    pub(crate) fn value(&self) -> &Value {
        &self.value
    }
}

/// [`MetadataParams`] as they arrive, before the value of the one reserved key is checked.
#[derive(Deserialize)]
struct UncheckedMetadataParams {
    id: NativeId,
    #[serde(deserialize_with = "caller_or_copies")]
    key: MetadataKey,
    value: Value,
}

impl TryFrom<UncheckedMetadataParams> for MetadataParams {
    type Error = String;

    fn try_from(params: UncheckedMetadataParams) -> Result<Self, Self::Error> {
        Self::new(params.id, params.key, params.value)
    }
}

/// A narrow metadata write's key as §4.18 admits it: any key [`MetadataKey::new`] accepts,
/// and [`MetadataKey::COPIES_KEY`] and [`MetadataKey::COPY_LOCATIONS_KEY`], which the engine
/// alone sends and nothing a caller types can name.
fn caller_or_copies<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<MetadataKey, D::Error> {
    let key = String::deserialize(deserializer)?;
    if key == MetadataKey::COPIES_KEY {
        return Ok(MetadataKey::copies());
    }
    if key == MetadataKey::COPY_LOCATIONS_KEY {
        return Ok(MetadataKey::copy_locations());
    }
    MetadataKey::new(key).map_err(serde::de::Error::custom)
}

/// The result of any write method (§4.9, §4.12): the id the destination holds the item
/// under.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WriteResult {
    /// The destination's own id for the item that was written.
    pub(crate) id: NativeId,
    /// The content the write's own answer says the backend stores (§4.9), from a plugin that
    /// declared `stored_forms`. Omitted when the plugin has nothing to say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stored_content: Option<String>,
}

/// The `params` of `stored_status` (§4.25): the status an item of one kind would be written at.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredStatusParams {
    /// Whether the item written is a task or a project.
    pub(crate) kind: ItemKind,
    /// The status it would be written at.
    pub(crate) status: Status,
}

/// The result of `stored_status` (§4.25): the status the backend would hold.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredStatusResult {
    /// The status, as the backend would hold it.
    pub(crate) status: Status,
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use onetaskgraph_plugin_api::{
        AssetName, AssetPayload, AssetUpload, AssetUploads, AssetWrite, AssetsWritten, Document,
        ItemWrite, NativeId,
    };
    use serde_json::{Value, json};

    use super::{DocumentAssetWriteParams, TaskAssetWriteParams};

    /// The member names of one object.
    fn members(value: &Value) -> BTreeSet<String> {
        value
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect()
    }

    /// The property names a root of the emitted schema declares.
    fn declared(root: &str) -> BTreeSet<String> {
        members(&crate::schema_bundle()["roots"][root]["properties"])
    }

    /// The trait's own argument, every member present.
    fn whole() -> AssetWrite {
        let name = AssetName::new("before.png").expect("a name");
        AssetWrite {
            assets: vec![AssetPayload::of(name.clone(), vec![1, 2, 3])],
            recorded_assets: Some(AssetUploads(BTreeMap::from([(
                name,
                AssetUpload {
                    sha256: "00".to_owned(),
                    url: "https://example.invalid/before.png".to_owned(),
                },
            )]))),
        }
    }

    #[test]
    fn a_writes_asset_members_are_the_trait_arguments_and_the_emitted_schemas() {
        let document: Document = serde_json::from_value(json!({
            "id": "D-1", "title": "Design", "content": "![b](./before.png)", "labels": []
        }))
        .expect("a document");
        let wire = serde_json::to_value(DocumentAssetWriteParams {
            write: ItemWrite {
                target: Some(NativeId::from("D-1")),
                item: document,
                depends_on: Vec::new(),
            },
            assets: whole(),
        })
        .expect("serializes");
        let mut beside_the_write = members(&wire);
        assert!(beside_the_write.remove("write"));
        // What crosses beside a write is exactly the in-process argument's members, and those
        // are exactly what the emitted `AssetWrite` root declares.
        assert_eq!(
            beside_the_write,
            members(&serde_json::to_value(whole()).unwrap())
        );
        assert_eq!(beside_the_write, declared("AssetWrite"));
        assert_eq!(members(&wire["assets"][0]), declared("AssetPayload"));
        assert_eq!(
            members(&wire["recorded_assets"]["before.png"]),
            declared("AssetUpload")
        );
        let answered = serde_json::to_value(AssetsWritten {
            id: NativeId::from("D-1"),
            content: Some("![b](https://example.invalid/before.png)".to_owned()),
        })
        .unwrap();
        assert_eq!(members(&answered), declared("AssetsWritten"));

        // A task's asset write carries the same members beside its own `write`.
        let task = serde_json::to_value(TaskAssetWriteParams {
            write: ItemWrite {
                target: None,
                item: serde_json::from_value(json!({
                    "id": "T-1", "title": "Alpha", "content": null,
                    "status": {"category": "todo", "name": "Todo"}, "labels": []
                }))
                .expect("a task"),
                depends_on: Vec::new(),
            },
            assets: whole(),
        })
        .unwrap();
        let mut task_members = members(&task);
        assert!(task_members.remove("write"));
        assert_eq!(task_members, beside_the_write);

        // The reference host reads what the engine sends through a shape of its own; both
        // halves of it read back exactly the trait's argument, and a plain write reads as none.
        for sent in [wire, task] {
            let served: super::ServedWriteParams<Value> =
                serde_json::from_value(sent).expect("the host reads the write");
            assert_eq!(served.assets, Some(whole()));
        }
        let plain: super::ServedWriteParams<Value> = serde_json::from_value(json!({
            "write": {"target": null, "item": {}, "depends_on": []}
        }))
        .expect("the host reads a plain write");
        assert!(plain.assets.is_none());
        for null in ["assets", "recorded_assets"] {
            let mut sent = json!({"write": {"target": null, "item": {}, "depends_on": []}});
            sent[null] = Value::Null;
            assert!(
                serde_json::from_value::<super::ServedWriteParams<Value>>(sent).is_err(),
                "{null}: null is refused rather than read as absent"
            );
        }
        // The emitted schema and the decoders agree about `null`: neither member admits it.
        let bundle = crate::schema_bundle();
        let bytes = &bundle["roots"]["AssetPayload"]["properties"]["bytes"];
        assert_eq!(bytes["type"], json!("string"), "{bytes}");
        let recorded = bundle["roots"]["AssetWrite"]["properties"]["recorded_assets"].to_string();
        assert!(!recorded.contains("null"), "{recorded}");
        let mut payload = serde_json::to_value(&whole().assets[0]).unwrap();
        payload["bytes"] = Value::Null;
        assert!(serde_json::from_value::<AssetPayload>(payload).is_err());
        assert!(
            serde_json::from_value::<AssetWrite>(json!({"assets": [], "recorded_assets": null}))
                .is_err()
        );
        let orphaned = serde_json::from_value::<super::ServedWriteParams<Value>>(json!({
            "write": {"target": null, "item": {}, "depends_on": []},
            "recorded_assets": {}
        }))
        .expect_err("recorded uploads without assets are refused");
        assert!(
            orphaned.to_string().contains("recorded_assets"),
            "{orphaned}"
        );
    }

    /// The checked-in golden of the members `docs/plugin-protocol.md` §3.16, §4.9 and §4.25
    /// added to version 2 without a bump — `stored_form` among them no longer, since the engine
    /// retired it.
    fn stored_forms_golden() -> Value {
        serde_json::from_str(include_str!("../../tests/fixtures/stored-forms-wire.json"))
            .expect("the golden is JSON")
    }

    /// `value` read as `T` and written back, without the `capabilities` a handshake carries:
    /// §4.2 reconciles those on their own, and they are written back with every default.
    fn round_trip<T: serde::Serialize + for<'de> serde::Deserialize<'de>>(value: &Value) -> Value {
        let read: T = serde_json::from_value(value.clone()).expect("the golden reads");
        uncapable(serde_json::to_value(read).expect("it writes back"))
    }

    /// `value` without its `capabilities` member.
    fn uncapable(mut value: Value) -> Value {
        if let Some(members) = value.as_object_mut() {
            members.remove("capabilities");
        }
        value
    }

    #[test]
    fn the_stored_forms_wire_shapes_round_trip_byte_for_byte_and_omit_what_is_absent() {
        use super::{InitializeResult, StoredStatusParams, StoredStatusResult, WriteResult};
        let golden = stored_forms_golden();
        for plugin in ["declaring_plugin", "silent_plugin"] {
            for (member, written) in [
                (
                    "initialize",
                    round_trip::<InitializeResult>(&golden[plugin]["initialize"]),
                ),
                (
                    "write_task",
                    round_trip::<WriteResult>(&golden[plugin]["write_task"]),
                ),
            ] {
                assert_eq!(
                    written,
                    uncapable(golden[plugin][member].clone()),
                    "{plugin} {member}"
                );
            }
        }
        let declaring = &golden["declaring_plugin"];
        assert_eq!(
            round_trip::<StoredStatusResult>(&declaring["stored_status"]),
            declaring["stored_status"]
        );
        let requests = &golden["requests"];
        assert_eq!(
            round_trip::<StoredStatusParams>(&requests["stored_status"]),
            requests["stored_status"]
        );

        // A plugin written before the members omits them and is read as declaring nothing:
        // `stored_forms` false, no `stored_content` — and written back without either.
        let silent: InitializeResult =
            serde_json::from_value(golden["silent_plugin"]["initialize"].clone()).unwrap();
        assert!(!silent.stored_forms);
        let quiet: WriteResult =
            serde_json::from_value(golden["silent_plugin"]["write_task"].clone()).unwrap();
        assert_eq!(quiet.stored_content, None);
        // An explicit `null` reads as absent, and a `false` as not declaring; neither is
        // written back.
        let nulled: WriteResult =
            serde_json::from_value(json!({"id": "I-3", "stored_content": null})).unwrap();
        assert_eq!(serde_json::to_value(nulled).unwrap(), json!({"id": "I-3"}));
        let mut declared_false = golden["silent_plugin"]["initialize"].clone();
        declared_false["stored_forms"] = json!(false);
        assert_eq!(
            round_trip::<InitializeResult>(&declared_false),
            uncapable(golden["silent_plugin"]["initialize"].clone())
        );
        // And a declaring plugin's members come back as it sent them.
        let declaring: InitializeResult =
            serde_json::from_value(golden["declaring_plugin"]["initialize"].clone()).unwrap();
        assert!(declaring.stored_forms);
        let stored: WriteResult =
            serde_json::from_value(golden["declaring_plugin"]["write_task"].clone()).unwrap();
        assert_eq!(stored.stored_content.as_deref(), Some("* one"));
    }
}
