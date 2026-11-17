//! Batched store calls: many searches, many comments and many held updates of one source, each
//! in one call with one answer per item.
//!
//! A caller doing one thing to many items asks once. The engine checks every item on its own
//! before anything is sent — a malformed item is that item's `error`, never the batch's — and
//! hands the rest to the source in one call, which a source whose backend batches turns into as
//! few requests as its API allows ([`TaskSource::query_tasks_batch`],
//! [`TaskSource::add_comments`], [`TaskSource::update_tasks`]). Every item stays its own write:
//! one item's failure never writes or refuses another's.
//!
//! The documents read and answered here are declared once, in this module, and published as
//! roots of the schema bundle, so both SDKs model them from these types.
//!
//! [`TaskSource::query_tasks_batch`]: onetaskgraph_plugin_api::TaskSource::query_tasks_batch
//! [`TaskSource::add_comments`]: onetaskgraph_plugin_api::TaskSource::add_comments
//! [`TaskSource::update_tasks`]: onetaskgraph_plugin_api::TaskSource::update_tasks

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use chrono::SecondsFormat;
use onetaskgraph_plugin_api::{
    BatchAnswer, BatchComment, BatchUpdate, Comment, Cursor, LabelFilter, MetadataKey,
    MetadataMatch, NativeId, NewComment, PageRequest, Priority, ProjectFilter, Repository,
    SourceName, Status, StatusCategory, Task, TaskUpdate, TextFields, TextQuery,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::boundary::{Exposure, Held};
use super::copy::readings;
use super::delivery::qualified_task;
use super::{Engine, EngineError, Filters, PriorityCondition, PriorityGuard, Qualified};
use super::{TaskUpdated, shape_tasks};
use crate::GlobalId;

/// The one version of every batch document this module reads and answers.
pub const BATCH_SCHEMA_VERSION: u32 = 1;

/// What `task find` reads: many narrowed searches of one source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindRequest {
    /// `1`.
    pub schema_version: u32,
    /// The searches, each answered under its own key.
    pub queries: Vec<FindQuery>,
}

/// One search of a [`FindRequest`]: every task matching all it names.
///
/// It must be narrowed by at least one of `text`, `metadata` and `origin`; one narrowed by none
/// would answer the whole source, and is refused naming its key.
// llmlint: ignore[invalid_states_unrepresentable] This is the wire document `task find` reads, its members the contract states literally; `Engine::find_tasks` turns each into the search it walks only once it is narrowed, refusing one that is not by its key before anything is read (`a_search_narrowed_by_nothing_is_refused_by_its_key_reading_nothing`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindQuery {
    /// The caller's own name for this search, which its answer carries back. Unique within the
    /// request.
    pub key: String,
    /// Free text the task's title, content or either holds, as `in` says.
    #[serde(default)]
    pub text: Option<String>,
    /// Where `text` is looked for.
    #[serde(default, rename = "in")]
    pub fields: FindIn,
    /// Caller-defined metadata values the task holds, each at `<KEY>[/<SEGMENT>…]`, as
    /// `task list --metadata` takes them.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// The qualified id the task was copied from.
    #[serde(default)]
    pub origin: Option<GlobalId>,
    /// The status categories to keep; empty keeps every one.
    #[serde(default)]
    pub status: Vec<StatusCategory>,
}

/// Which fields a [`FindQuery`]'s text is looked for in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FindIn {
    /// Titles only.
    Title,
    /// Contents only.
    Content,
    /// Either.
    #[default]
    Both,
}

/// What `task find` answers: one answer per search, in the order asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FoundTasks {
    /// `1`.
    pub schema_version: u32,
    /// One per search.
    pub answers: Vec<FindAnswer>,
}

/// What one search of a [`FindRequest`] found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FindAnswer {
    /// The search's key.
    pub key: String,
    /// Every task it matched, across every page, each as `task list` reports it.
    pub items: Vec<Qualified<Task>>,
    // llmlint: ignore-block[invalid_states_unrepresentable] The answer's shape is the contract this task states literally — `complete` and `reason` side by side, `reason` null when complete — which other nodes build against; `FindAnswer::complete_with` and `FindAnswer::cut_short` are the only constructors, and they set the pair together.
    /// Whether `items` is every match: `false` when a page of this search could not be read,
    /// and `items` holds what the pages before it answered.
    pub complete: bool,
    /// Why the answer is incomplete, naming the page refused — `null` when it is complete.
    pub reason: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// What `task comment add-many` reads: many comments, each on one task of one source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentBatch {
    /// `1`.
    pub schema_version: u32,
    /// The comments, each written on its own.
    pub comments: Vec<CommentBatchItem>,
}

/// One comment of a [`CommentBatch`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentBatchItem {
    /// The task's qualified id, `<source>:<native-id>`, of the source the batch names.
    // llmlint: ignore[invalid_states_unrepresentable] Read as text so a malformed id is that item's `error`, naming it, rather than a refusal of the whole batch; `Engine::add_comments` parses it as a `GlobalId` before anything is sent.
    pub item: String,
    /// The comment's Markdown.
    pub body: String,
    /// The version the task must be at for the comment to be written, as a read reported it.
    #[serde(default)]
    pub if_version: Option<String>,
}

/// What `task comment add-many` answers: one result per comment, in the order given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommentsAdded {
    /// `1`.
    pub schema_version: u32,
    /// One per comment.
    pub results: Vec<CommentAddResult>,
}

/// What became of one comment of a [`CommentBatch`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommentAddResult {
    // llmlint: ignore-block[invalid_states_unrepresentable] The shape is the contract the task fixes literally — `{"item", "outcome", "comment", "current_version", "reason"}` with `outcome` a tag beside flat members — which other nodes of the plan build against; every result is made by `Engine::add_comments` or `comment_error` in this module, each setting the members its outcome carries and no other.
    /// The item as the batch named it.
    pub item: String,
    /// What became of it.
    pub outcome: BatchOutcome,
    /// The comment written — `null` unless `outcome` is `written`.
    pub comment: Option<AddedComment>,
    /// The version the task is at — present when `outcome` is `version-moved` and the source
    /// can name it, which it cannot just after another write of this batch moved the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_version: Option<String>,
    /// Why it was not written — present when `outcome` is `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// One comment a batch wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AddedComment {
    /// Its id in the source.
    pub id: NativeId,
    /// Where it is shown, when the source has an address for it.
    pub url: Option<String>,
    /// Its version, as `task comments` reports one.
    pub version: Option<String>,
}

/// What `task update-many` reads: many held updates of tasks of one source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateBatch {
    /// `1`.
    pub schema_version: u32,
    /// The updates, each one held write of every field it names.
    pub updates: Vec<UpdateBatchItem>,
}

/// One update of an [`UpdateBatch`]: every field it names, written together in one held write,
/// and nothing else. Every member but `item` is optional.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateBatchItem {
    /// The task's qualified id, `<source>:<native-id>`, of the source the batch names.
    // llmlint: ignore[invalid_states_unrepresentable] As `CommentBatchItem::item`: read as text so a malformed id is that item's own `error`.
    pub item: String,
    /// The version the task must be at for anything to be written.
    #[serde(default)]
    pub if_version: Option<String>,
    /// Its title.
    #[serde(default)]
    pub title: Option<String>,
    /// Its content, replaced whole.
    #[serde(default)]
    pub body: Option<String>,
    /// Its status category.
    #[serde(default)]
    pub status: Option<StatusCategory>,
    /// Its priority; `none` clears it.
    #[serde(default)]
    pub priority: Option<Priority>,
    /// Write only while the task holds one of these priorities — `none` for a task with none.
    /// At any other, nothing of the update is written and it answers `held`. Empty sets no
    /// guard.
    #[serde(default)]
    pub when_priority: Vec<Priority>,
    /// Caller-owned metadata keys set, each to one JSON value; every other key is kept.
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    /// Caller-owned metadata keys removed.
    #[serde(default)]
    pub remove_metadata: Vec<String>,
    /// Its repositories, replaced whole; an empty list clears them.
    #[serde(default)]
    pub repositories: Option<Vec<Repository>>,
    /// The one person it is assigned to, replacing every person it holds — `null` unassigns
    /// it, and leaving the member out leaves its assignees as they are. Only for a source that
    /// opted in with `config.assignees: true`.
    // llmlint: ignore[invalid_states_unrepresentable] The contract states this member literally as `"<person>" | null` beside absent, three states a caller writes by hand; `Option<Option<String>>` is that wire shape and nothing wider, read by `present` so `null` is not mistaken for absent.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>", extend("x-null-unsets" = true))]
    pub assignee: Option<Option<String>>,
    /// Write only while the task is assigned to exactly these people, whatever their order —
    /// `[]` only while it is unassigned. Otherwise nothing of the update is written and it
    /// answers `held`. Left out, it sets no guard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_assignees: Option<Vec<String>>,
}

/// A member that may be `null`, read so that `null` is present: `Some(None)`, where a member
/// left out is `None`.
fn present<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

/// What `task update-many` answers: one result per update, in the order given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TasksUpdated {
    /// `1`.
    pub schema_version: u32,
    /// One per update.
    pub results: Vec<UpdateResult>,
}

/// What became of one update of an [`UpdateBatch`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateResult {
    // llmlint: ignore-block[invalid_states_unrepresentable] As `CommentAddResult`: `{"item", "outcome", "version", "current_version", "reason"}` is the contract the task fixes literally, and every result is made by `Engine::update_tasks` or `update_error` in this module, each setting the members its outcome carries and no other.
    /// The item as the batch named it.
    pub item: String,
    /// What became of it.
    pub outcome: BatchOutcome,
    /// The task's version after the write — present when `outcome` is `written` and the
    /// write's own answers determine it, as `task update`'s `result.version` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The version the task is at — present when `outcome` is `version-moved` and the source
    /// can name it, which it cannot just after another write of this batch moved the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_version: Option<String>,
    /// Why it is answered `error` — present when `outcome` is `error`: that it was not written,
    /// or that it was and keeping the tasks it delivers in step then failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// What became of one item of a batched write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum BatchOutcome {
    /// It was written.
    Written,
    /// Its guard declined: the task held a priority `when_priority` does not name, or not
    /// exactly the people `when_assignees` names, and nothing of it was written.
    Held,
    /// The task was at another version than the one it was held to, and nothing was written.
    VersionMoved,
    /// It could not be written; `reason` says why, naming it.
    Error,
}

impl BatchOutcome {
    /// Whether this item ended as its caller asked — written, or declined by its own guard.
    #[must_use]
    pub const fn settled(self) -> bool {
        matches!(self, Self::Written | Self::Held)
    }
}

impl FindAnswer {
    fn complete_with(key: String, items: Vec<Qualified<Task>>) -> Self {
        Self {
            key,
            items,
            complete: true,
            reason: None,
        }
    }

    fn cut_short(key: String, items: Vec<Qualified<Task>>, reason: String) -> Self {
        Self {
            key,
            items,
            complete: false,
            reason: Some(reason),
        }
    }
}

/// `answers`, a source's answer to a batched call of `asked` items, as exactly one answer per
/// item. Fewer answers than items leave each item past the last answer unanswered, its answer
/// malformed; more answers than items say nothing of which answer is whose, so every item's
/// answer is malformed, naming both counts — each item was sent, so each may have been written.
pub(crate) fn one_each<T>(
    source: &dyn std::fmt::Display,
    asked: usize,
    answers: Vec<Result<T, onetaskgraph_plugin_api::SourceError>>,
) -> Vec<Result<T, onetaskgraph_plugin_api::SourceError>> {
    let answered = answers.len();
    let malformed = |why: &str| {
        Err(onetaskgraph_plugin_api::SourceError::Malformed {
            message: format!(
                "{source} answered {answered} answers to the {asked} items of one batched call, \
                 {why}"
            ),
        })
    };
    if answered > asked {
        return (0..asked)
            .map(|_| {
                malformed(
                    "so which answer is this item's cannot be told; it was sent and may have \
                     been written, so read it before sending it again",
                )
            })
            .collect();
    }
    let mut answers = answers;
    while answers.len() < asked {
        answers.push(malformed("leaving this one unanswered"));
    }
    answers
}

/// The batched calls' schema roots: what `task find`, `task comment add-many` and `task
/// update-many` read, what each answers, and the outcome vocabulary a caller branches on per
/// item — in the order the bundle publishes them.
pub(crate) fn schemas() -> [(&'static str, schemars::Schema); 7] {
    use crate::schema::write_answer_schema;
    use schemars::schema_for;
    [
        ("FindRequest", schema_for!(FindRequest)),
        ("FoundTasks", schema_for!(FoundTasks)),
        ("CommentBatch", schema_for!(CommentBatch)),
        ("CommentsAdded", write_answer_schema::<CommentsAdded>()),
        ("UpdateBatch", schema_for!(UpdateBatch)),
        ("TasksUpdated", write_answer_schema::<TasksUpdated>()),
        ("BatchOutcome", schema_for!(BatchOutcome)),
    ]
}

/// A batch document of another schema version than this engine reads, refused whole.
fn check_version(what: &str, found: u32) -> Result<(), EngineError> {
    if found == BATCH_SCHEMA_VERSION {
        return Ok(());
    }
    Err(EngineError::BatchDocument {
        what: what.to_owned(),
        why: format!(
            "its schema_version is {found}, and this release reads {BATCH_SCHEMA_VERSION}"
        ),
    })
}

/// A metadata match spelled `<KEY>[/<SEGMENT>…]` → `value`.
fn metadata_match(location: &str, value: &str) -> Result<MetadataMatch, String> {
    let mut segments = location.split('/').map(str::to_owned);
    let key = segments.next().unwrap_or_default();
    MetadataMatch::new(key, segments.collect(), value)
}

/// One search, checked and shaped for its source.
struct Search {
    key: String,
    pushed: onetaskgraph_plugin_api::TaskQuery,
    local: super::local::LocalTasks,
}

/// One search's walk: where it stands, the cursors it has followed, and what it has found.
struct Walk {
    stand: Stand,
    followed: BTreeSet<String>,
    pages: u32,
    items: Vec<Qualified<Task>>,
}

/// Where one search's walk stands.
enum Stand {
    /// Its next page is to be asked for from this cursor — the first page from none.
    Walking(Option<Cursor>),
    /// Its last page has been read.
    Complete,
    /// It was stopped before its last page, for this reason.
    Stopped(String),
}

impl Engine {
    /// Every search `request` names over the tasks of `source`, each walked to its last page,
    /// answered under its own key in the order asked.
    ///
    /// Every search is checked before anything is read: one narrowed by none of `text`,
    /// `metadata` and `origin`, a key used twice and a malformed metadata location are refused,
    /// naming the key, and nothing is read. The searches are then asked together, a page of
    /// each still walking per round, through
    /// [`TaskSource::query_tasks_batch`](onetaskgraph_plugin_api::TaskSource::query_tasks_batch),
    /// and every predicate the source declares unsupported is applied here, as `task list`
    /// applies it. A page the source refuses ends that search's walk and answers it
    /// `complete: false`, naming the page; every other search goes on.
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownSource`] for a source nothing configures,
    /// [`EngineError::BatchDocument`] for a request of another schema version or with a key
    /// used twice, and [`EngineError::UnnarrowedQuery`] for a search narrowed by nothing.
    pub async fn find_tasks(
        &self,
        source: &SourceName,
        request: &FindRequest,
        page_size: NonZeroU32,
    ) -> Result<FoundTasks, EngineError> {
        check_version("the find request", request.schema_version)?;
        let resolved = self.built(source)?;
        let capabilities = resolved.source().capabilities();
        let mut keys = BTreeSet::new();
        let mut searches = Vec::with_capacity(request.queries.len());
        for query in &request.queries {
            if !keys.insert(query.key.clone()) {
                return Err(EngineError::BatchDocument {
                    what: "the find request".to_owned(),
                    why: format!("the key {:?} names two queries", query.key),
                });
            }
            let text = query.text.as_ref().filter(|text| !text.trim().is_empty());
            if text.is_none() && query.metadata.is_empty() && query.origin.is_none() {
                return Err(EngineError::UnnarrowedQuery {
                    key: query.key.clone(),
                });
            }
            let metadata = query
                .metadata
                .iter()
                .map(|(location, value)| metadata_match(location, value))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|why| EngineError::BatchDocument {
                    what: format!("query {:?}", query.key),
                    why: format!("{why}; a metadata location is <KEY>[/<SEGMENT>...]"),
                })?;
            let filters = Filters {
                text: text.map(|terms| TextQuery {
                    terms: terms.clone(),
                    fields: match query.fields {
                        FindIn::Title => TextFields::Title,
                        FindIn::Content => TextFields::Content,
                        FindIn::Both => TextFields::TitleOrContent,
                    },
                }),
                labels: LabelFilter::default(),
                statuses: query.status.clone(),
            };
            let shape = shape_tasks(
                &capabilities,
                &filters,
                &ProjectFilter::Any,
                &[],
                None,
                &metadata,
                query.origin.as_ref(),
            );
            searches.push(Search {
                key: query.key.clone(),
                pushed: shape.pushed,
                local: shape.local,
            });
        }
        let limit = page_size.get().min(capabilities.max_page_size.max(1));
        let mut walks: Vec<Walk> = searches
            .iter()
            .map(|_| Walk {
                stand: Stand::Walking(None),
                followed: BTreeSet::new(),
                pages: 0,
                items: Vec::new(),
            })
            .collect();
        loop {
            let asks: Vec<(usize, (onetaskgraph_plugin_api::TaskQuery, PageRequest))> = walks
                .iter()
                .enumerate()
                .filter_map(|(at, walk)| match &walk.stand {
                    Stand::Walking(cursor) => Some((
                        at,
                        (
                            searches[at].pushed.clone(),
                            PageRequest {
                                cursor: cursor.clone(),
                                limit,
                            },
                        ),
                    )),
                    Stand::Complete | Stand::Stopped(_) => None,
                })
                .collect();
            if asks.is_empty() {
                break;
            }
            let (walking, asks): (Vec<usize>, Vec<_>) = asks.into_iter().unzip();
            let answered = one_each(
                source,
                asks.len(),
                resolved.source().query_tasks_batch(&asks).await,
            );
            for (at, answer) in walking.into_iter().zip(answered) {
                let walk = &mut walks[at];
                walk.pages += 1;
                match answer {
                    Ok(page) => {
                        walk.items.extend(
                            page.items
                                .into_iter()
                                .filter(|task| searches[at].local.keeps(task))
                                .map(|task| {
                                    qualified_task(
                                        GlobalId::new(source.clone(), task.id.clone()),
                                        task,
                                    )
                                }),
                        );
                        walk.stand = match page.next {
                            None => Stand::Complete,
                            Some(next) if !walk.followed.insert(next.0.clone()) => {
                                Stand::Stopped(format!(
                                    "page {} of query {:?} answered the cursor {:?}, which this \
                                     search had already followed, so its walk was stopped there",
                                    walk.pages, searches[at].key, next.0
                                ))
                            }
                            Some(next) => Stand::Walking(Some(next)),
                        };
                    }
                    Err(error) => {
                        walk.stand = Stand::Stopped(format!(
                            "page {} of query {:?} was refused by {source}: {error}",
                            walk.pages, searches[at].key
                        ));
                    }
                }
            }
        }
        let answers = searches
            .into_iter()
            .zip(walks)
            .map(|(search, walk)| match walk.stand {
                Stand::Stopped(reason) => FindAnswer::cut_short(search.key, walk.items, reason),
                // The walk above ends only once no search is still walking.
                Stand::Complete | Stand::Walking(_) => {
                    FindAnswer::complete_with(search.key, walk.items)
                }
            })
            .collect();
        Ok(FoundTasks {
            schema_version: BATCH_SCHEMA_VERSION,
            answers,
        })
    }

    /// Every comment `batch` names, each added to its task of `source` on its own, answered one
    /// result per comment in the order given.
    ///
    /// Each comment is checked before anything is sent — an id that is not a qualified id of
    /// `source`, an empty body, a body the write policy refuses — and one that fails is answered
    /// `error`, naming it, while every other goes on. The rest are sent together through
    /// [`TaskSource::add_comments`](onetaskgraph_plugin_api::TaskSource::add_comments): one held
    /// to a version its task has moved past answers `version-moved` with that version, one
    /// whose task is not there or that the source refuses answers `error`.
    ///
    /// # Errors
    ///
    /// [`EngineError::BatchDocument`] for a batch of another schema version, and the refusals
    /// of [`add_comment`](Self::add_comment) about the source itself — unknown, without
    /// comments, or with comments it cannot write — each before anything is sent.
    pub async fn add_comments(
        &self,
        source: &SourceName,
        batch: &CommentBatch,
    ) -> Result<CommentsAdded, EngineError> {
        check_version("the comment batch", batch.schema_version)?;
        let resolved = self.writable_comments(source)?;
        let mut results: Vec<Option<CommentAddResult>> = Vec::with_capacity(batch.comments.len());
        let mut sent: Vec<(usize, BatchComment)> = Vec::new();
        for (at, each) in batch.comments.iter().enumerate() {
            match self.checked_comment(resolved, source, each).await {
                Ok(comment) => {
                    results.push(None);
                    sent.push((at, comment));
                }
                Err(reason) => results.push(Some(comment_error(&each.item, reason))),
            }
        }
        let writes: Vec<BatchComment> = sent.iter().map(|(_, each)| each.clone()).collect();
        let answers = if writes.is_empty() {
            Vec::new()
        } else {
            one_each(
                source,
                writes.len(),
                resolved.source().add_comments(&writes).await,
            )
        };
        for ((at, written), answer) in sent.into_iter().zip(answers) {
            let item = batch.comments[at].item.clone();
            results[at] = Some(match answer {
                Ok(BatchAnswer::Written(comment)) => CommentAddResult {
                    item,
                    outcome: BatchOutcome::Written,
                    comment: Some(added(&comment)),
                    current_version: None,
                    reason: None,
                },
                Ok(BatchAnswer::VersionMoved { found }) => CommentAddResult {
                    item,
                    outcome: BatchOutcome::VersionMoved,
                    comment: None,
                    current_version: Some(found.unwrap_or_default()),
                    reason: None,
                },
                Ok(
                    BatchAnswer::Missing
                    | BatchAnswer::PriorityHeld(_)
                    | BatchAnswer::AssigneesHeld(_),
                ) => {
                    let reason = format!(
                        "{source} holds no task {}, so nothing was written on it",
                        GlobalId::new(source.clone(), written.task.clone())
                    );
                    comment_error(&item, reason)
                }
                Err(error) => {
                    let reason = format!("{source} refused the comment on {item}: {error}");
                    comment_error(&item, reason)
                }
            });
        }
        Ok(CommentsAdded {
            schema_version: BATCH_SCHEMA_VERSION,
            results: results.into_iter().flatten().collect(),
        })
    }

    /// One comment of a batch as its source is to be sent it, or why it cannot be.
    async fn checked_comment(
        &self,
        resolved: &super::ResolvedSource,
        source: &SourceName,
        each: &CommentBatchItem,
    ) -> Result<BatchComment, String> {
        let id = item_of(&each.item, source)?;
        let body = onetaskgraph_plugin_api::CommentBody::new(each.body.clone())
            .map_err(|error| format!("the comment on {}: {error}", each.item))?;
        let rewrite = self
            .admit_existing(
                resolved,
                &id.to_string(),
                &id.native,
                Held::Task,
                &Exposure::text(body.as_str()),
            )
            .await
            .map_err(|error| format!("the comment on {}: {error}", each.item))?;
        let body = super::comment::rewritten_body(resolved, &rewrite, &body)
            .map_err(|error| format!("the comment on {}: {error}", each.item))?;
        Ok(BatchComment {
            task: id.native,
            comment: NewComment { body, author: None },
            if_version: each.if_version.clone(),
        })
    }

    /// Every update `batch` names, each one held write of every field it names on its task of
    /// `source`, answered one result per update in the order given.
    ///
    /// Each update is checked before anything is sent, exactly as
    /// [`update_task`](Self::update_task) checks one — an id that is not a qualified id of
    /// `source`, an update naming no field, a reserved or malformed metadata key, a priority
    /// the source holds none of — and one that fails is answered `error`, naming it, while every
    /// other goes on. The rest are sent together through
    /// [`TaskSource::update_tasks`](onetaskgraph_plugin_api::TaskSource::update_tasks): one held
    /// to a version its task has moved past answers `version-moved` with that version, one whose
    /// priority guard declines answers `held`, and each written one keeps the tasks it delivers
    /// in step exactly as `task update` does.
    ///
    /// # Errors
    ///
    /// [`EngineError::BatchDocument`] for a batch of another schema version, and
    /// [`EngineError::UnknownSource`] or [`EngineError::UpdateNotWritable`] for the source
    /// itself, each before anything is sent.
    pub async fn update_tasks(
        &self,
        source: &SourceName,
        batch: &UpdateBatch,
    ) -> Result<TasksUpdated, EngineError> {
        check_version("the update batch", batch.schema_version)?;
        let resolved = self.updatable(source)?;
        let mut results: Vec<Option<UpdateResult>> = Vec::with_capacity(batch.updates.len());
        let mut sent: Vec<(usize, GlobalId, BatchUpdate)> = Vec::new();
        for (at, each) in batch.updates.iter().enumerate() {
            match self.checked_update(resolved, source, each).await {
                Ok((id, update)) => {
                    results.push(None);
                    sent.push((at, id, update));
                }
                Err(reason) => results.push(Some(update_error(&each.item, reason))),
            }
        }
        let writes: Vec<BatchUpdate> = sent.iter().map(|(_, _, each)| each.clone()).collect();
        let before = readings(&[resolved]).await;
        let answers = if writes.is_empty() {
            Vec::new()
        } else {
            one_each(
                source,
                writes.len(),
                resolved.source().update_tasks(&writes).await,
            )
        };
        for ((at, id, written), answer) in sent.into_iter().zip(answers) {
            let item = batch.updates[at].item.clone();
            results[at] = Some(match answer {
                Ok(BatchAnswer::Written(outcome)) => {
                    match self
                        .updated(resolved, &id, &written.update, outcome, &before)
                        .await
                    {
                        // A delivered task the rule could not keep in step leaves this update's
                        // work unfinished, as `task update` exits `4` for it.
                        Ok(updated) if updated.delivered.iter().any(super::Delivered::failed) => {
                            let failures: Vec<String> = updated
                                .delivered
                                .iter()
                                .filter_map(|entry| match &entry.outcome {
                                    super::DeliveryOutcome::Failed { failure, .. } => {
                                        Some(format!("{}: {}", entry.ticket, failure.message()))
                                    }
                                    _ => None,
                                })
                                .collect();
                            update_error(
                                &item,
                                format!(
                                    "{item} was written, then keeping the tasks it delivers in \
                                     step failed — {}",
                                    failures.join("; ")
                                ),
                            )
                        }
                        Ok(updated) => UpdateResult {
                            item,
                            outcome: BatchOutcome::Written,
                            // What the write's own answers determine, as `task update`'s
                            // `result` says it: a source whose mutation answers cannot vouch for
                            // a version answers none rather than the one it read before.
                            version: updated.result.and_then(|result| result.version),
                            current_version: None,
                            reason: None,
                        },
                        Err(error) => update_error(
                            &item,
                            format!("{item} was written, then keeping it in step failed: {error}"),
                        ),
                    }
                }
                Ok(BatchAnswer::VersionMoved { found }) => UpdateResult {
                    item,
                    outcome: BatchOutcome::VersionMoved,
                    version: None,
                    current_version: Some(found.unwrap_or_default()),
                    reason: None,
                },
                Ok(BatchAnswer::PriorityHeld(_) | BatchAnswer::AssigneesHeld(_)) => UpdateResult {
                    item,
                    outcome: BatchOutcome::Held,
                    version: None,
                    current_version: None,
                    reason: None,
                },
                Ok(BatchAnswer::Missing) => {
                    let reason =
                        format!("{source} holds no task {id}, so nothing was written to it");
                    update_error(&item, reason)
                }
                Err(error) => {
                    let reason = format!("{source} refused the update of {item}: {error}");
                    update_error(&item, reason)
                }
            });
        }
        Ok(TasksUpdated {
            schema_version: BATCH_SCHEMA_VERSION,
            results: results.into_iter().flatten().collect(),
        })
    }

    /// One update of a batch as its source is to be sent it, or why it cannot be.
    async fn checked_update(
        &self,
        resolved: &super::ResolvedSource,
        source: &SourceName,
        each: &UpdateBatchItem,
    ) -> Result<(GlobalId, BatchUpdate), String> {
        let id = item_of(&each.item, source)?;
        let named = |why: String| format!("the update of {}: {why}", each.item);
        let update = TaskUpdate {
            title: each.title.clone(),
            content: each.body.clone(),
            status: each.status.map(status_of),
            priority: each.priority,
            metadata_set: each
                .metadata
                .iter()
                .map(|(key, value)| Ok((MetadataKey::new(key.as_str())?, value.clone())))
                .collect::<Result<BTreeMap<_, _>, String>>()
                .map_err(named)?,
            metadata_remove: each
                .remove_metadata
                .iter()
                .map(|key| MetadataKey::new(key.as_str()))
                .collect::<Result<BTreeSet<_>, String>>()
                .map_err(named)?,
            delivers: None,
            depends_on: None,
            repositories: each.repositories.clone(),
            assignees: each
                .assignee
                .as_ref()
                .map(|person| person.iter().cloned().collect()),
        };
        if update.is_empty() {
            return Err(named(
                "it names no field to write; name at least one of title, body, status, \
                 priority, metadata, remove_metadata, repositories or assignee"
                    .to_owned(),
            ));
        }
        for guarded in &each.when_priority {
            super::narrow::holds_priority(resolved, &id.to_string(), *guarded)
                .map_err(|error| named(error.to_string()))?;
        }
        if each.when_assignees.is_some() {
            super::assignees::holds_assignees(resolved, &id.to_string())
                .map_err(|error| named(error.to_string()))?;
        }
        let update = self
            .prepared_update(resolved, &id, &update)
            .await
            .map_err(|error| named(error.to_string()))?;
        Ok((
            id.clone(),
            BatchUpdate {
                id: id.native,
                update,
                if_version: each.if_version.clone(),
                when_priority: each.when_priority.clone(),
                when_assignees: each.when_assignees.clone(),
            },
        ))
    }

    /// [`update_task`](Self::update_task) held to `if_version` and guarded by `when`: written
    /// only while the task is at that version and holds one of those priorities — the update of
    /// one item that `task update --when-priority` makes, every field it names written
    /// together. A guard that declines answers the task as read, with nothing written and the
    /// guard saying so.
    ///
    /// # Errors
    ///
    /// As [`update_task`](Self::update_task), and [`EngineError::VersionMoved`] when the task
    /// is at another version than `if_version`.
    pub async fn update_task_when(
        &self,
        id: &GlobalId,
        update: &TaskUpdate,
        when: &[Priority],
        if_version: Option<&str>,
    ) -> Result<TaskUpdated, EngineError> {
        let resolved = self.updatable(&id.source)?;
        for guarded in when {
            super::narrow::holds_priority(resolved, &id.to_string(), *guarded)?;
        }
        let update = self.prepared_update(resolved, id, update).await?;
        // A guard is decided on a read the engine makes, so what it found is what it reports;
        // the write is then held to the version that read found, so a change landing between
        // the two is refused rather than written over.
        let mut condition = None;
        let mut held_to = if_version.map(str::to_owned);
        if !when.is_empty() {
            let task = resolved
                .source()
                .get_task(&id.native)
                .await
                .map_err(|error| super::delivery::source_failed(resolved, error))?
                .ok_or_else(|| EngineError::NoSuchTask { id: id.to_string() })?;
            if let Some(version) = if_version
                && !onetaskgraph_plugin_api::version_at(
                    resolved.source(),
                    task.version.as_deref(),
                    version,
                )
            {
                return Err(moved(id, version, task.version.unwrap_or_default()));
            }
            let met = when.contains(&task.priority);
            let found = PriorityCondition {
                held: task.priority,
                met,
            };
            if !met {
                return Ok(TaskUpdated {
                    id: id.clone(),
                    guard: Some(PriorityGuard::of(found)),
                    task: qualified_task(id.clone(), task).item,
                    written: BTreeSet::new(),
                    delivered: Vec::new(),
                    spent: None,
                    result: None,
                    fallback: None,
                    projection: None,
                });
            }
            condition = Some(found);
            held_to = task.version;
        }
        let before = readings(&[resolved]).await;
        // A read that gave no version leaves the write nothing to be held to, so the guard goes
        // with it, for the source to decide again on its own read just before it writes.
        let when_priority = if held_to.is_none() {
            when.to_vec()
        } else {
            Vec::new()
        };
        let batch = [BatchUpdate {
            id: id.native.clone(),
            update: update.clone(),
            if_version: held_to.clone(),
            when_priority,
            when_assignees: None,
        }];
        let answer = one_each(&id.source, 1, resolved.source().update_tasks(&batch).await)
            .into_iter()
            .next()
            .unwrap_or(Ok(BatchAnswer::Missing))
            .map_err(|error| super::delivery::source_failed(resolved, error))?;
        match answer {
            BatchAnswer::Written(outcome) => {
                let mut updated = self
                    .updated(resolved, id, &update, outcome, &before)
                    .await?;
                updated.guard = condition.map(PriorityGuard::of);
                Ok(updated)
            }
            BatchAnswer::PriorityHeld(task) => Ok(TaskUpdated {
                id: id.clone(),
                guard: Some(PriorityGuard::of(PriorityCondition {
                    held: task.priority,
                    met: false,
                })),
                task: qualified_task(id.clone(), *task).item,
                written: BTreeSet::new(),
                delivered: Vec::new(),
                spent: None,
                result: None,
                fallback: None,
                projection: None,
            }),
            BatchAnswer::VersionMoved { found } => Err(moved(
                id,
                held_to.as_deref().unwrap_or_default(),
                found.unwrap_or_default(),
            )),
            BatchAnswer::Missing => Err(EngineError::NoSuchTask { id: id.to_string() }),
            // This write names no assignee guard, so a source answering one held it by a guard
            // it was never sent.
            BatchAnswer::AssigneesHeld(_) => Err(super::delivery::source_failed(
                resolved,
                onetaskgraph_plugin_api::SourceError::Malformed {
                    message: format!(
                        "{id} was answered held by an assignee guard this update never set"
                    ),
                },
            )),
        }
    }
}

/// The refusal of a write of `id` held to `expected` that found it at `found`.
fn moved(id: &GlobalId, expected: &str, found: String) -> EngineError {
    EngineError::VersionMoved {
        id: id.to_string(),
        expected: expected.to_owned(),
        held: Box::new(crate::cache::WriteResult {
            version: Some(found.clone()),
            parts: None,
            status: None,
            priority: None,
        }),
        found,
    }
}

/// `item` as a qualified id of `source`, or why it is not one.
fn item_of(item: &str, source: &SourceName) -> Result<GlobalId, String> {
    let id = item.parse::<GlobalId>().map_err(|error| {
        format!("{item:?} is not a qualified task id, `<source>:<native-id>`: {error}")
    })?;
    if &id.source != source {
        return Err(format!(
            "{item} is a task of {}, not of {source}, which this batch writes",
            id.source
        ));
    }
    Ok(id)
}

/// A status of `category`, named by the category's own word.
fn status_of(category: StatusCategory) -> Status {
    Status {
        category,
        name: serde_json::to_value(category)
            .ok()
            .and_then(|word| word.as_str().map(str::to_owned))
            .unwrap_or_default(),
    }
}

/// The comment a batch wrote, as its result reports it.
fn added(comment: &Comment) -> AddedComment {
    AddedComment {
        id: comment.id.clone(),
        url: comment.url.clone(),
        version: comment
            .updated_at
            .or(comment.created_at)
            .map(|at| at.to_rfc3339_opts(SecondsFormat::AutoSi, true)),
    }
}

fn comment_error(item: &str, reason: String) -> CommentAddResult {
    CommentAddResult {
        item: item.to_owned(),
        outcome: BatchOutcome::Error,
        comment: None,
        current_version: None,
        reason: Some(reason),
    }
}

fn update_error(item: &str, reason: String) -> UpdateResult {
    UpdateResult {
        item: item.to_owned(),
        outcome: BatchOutcome::Error,
        version: None,
        current_version: None,
        reason: Some(reason),
    }
}
