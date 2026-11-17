//! Comments on a task: the four verbs that read and write them, and the task detail
//! `task show` renders them in.
//!
//! Every verb here addresses exactly one task of exactly one source, so none of them fans
//! out, pages for a caller or compensates for anything. What they owe instead is the
//! refusals: a source declaring no comments is refused before anything is read, a source
//! whose comments cannot be written is refused before a write is attempted, and a task or a
//! comment that is not there is named rather than answered with an empty result.
//!
//! Nothing here writes anything down. A list walks the source's pages to the end and hands
//! the caller exactly what it read; a copy never reaches this module at all, which is what
//! keeps a copy from reading or writing a comment at either end.

use chrono::{DateTime, Utc};
use onetaskgraph_plugin_api::{
    Asset, Comment, CommentBody, Cursor, Document, NativeId, NewComment, Page, PageRequest,
    SourceError, SourceName, Task, TaskDetailRead, TaskQuery,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Spent;
use super::assets;
use super::boundary::{Exposure, Held};
use super::fetch::{fits, unrepeated};
use super::{Answer, ConfiguredSource, Engine, EngineError, Qualified, delivery};
use crate::GlobalId;
use crate::cache::{CacheUse, WriteFallback, WriteResult};
use crate::plan::{QueryPlan, QueryResponse, SourceFailure};
use crate::resolve::ResolvedSource;

/// Every comment on one task, oldest first: what `task comment list` answers with.
///
/// An object rather than a bare list, so a later member — a total, say — is an addition a
/// reader already written against this shape can ignore.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommentList {
    /// The task's comments in the order they were written. Empty when it has none.
    pub comments: Vec<Comment>,
    /// How the read used its source's local cache — absent for a source keeping none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheUse>,
}

/// What `task comment delete` answers with: the id of the comment it removed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeletedComment {
    /// The id the comment was removed under, exactly as `list` reported it.
    pub deleted: NativeId,
    /// What each mutation an expected write sent came to — present when the write was made
    /// with `--expect` on a source that reports its writes' parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<WriteResult>,
    /// That an expected write read its target first, and why — absent when it sent only its
    /// mutations, and for a write made without `--expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<WriteFallback>,
    /// What an expected write spent at the sources that meter their own requests, its
    /// fallback read included — absent for a write made without `--expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent: Option<Spent>,
}

/// What `task comment add` and `task comment edit` answer with: the comment as its source now
/// holds it — its id and its address among it, so a caller never needs a lookup for either —
/// and what an expected write came to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommentWritten {
    /// The comment.
    #[serde(flatten)]
    pub comment: Comment,
    /// The qualified id of the task the comment was added to — present on `task comment add`'s
    /// answer, so a caller records what it commented on from the answer alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<GlobalId>,
    /// What each mutation an expected write sent came to — present when the write was made
    /// with `--expect` on a source that reports its writes' parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<WriteResult>,
    /// That an expected write read its target first, and why — absent when it sent only its
    /// mutations, and for a write made without `--expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<WriteFallback>,
    /// What an expected write spent at the sources that meter their own requests, its
    /// fallback read included — absent for a write made without `--expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent: Option<Spent>,
}

/// One task as `task show` reports it: the response every show verb answers with, and the
/// task's comments beside it.
///
/// The response is flattened rather than nested, so a reader of `task show --json` written
/// before comments existed reads exactly the members it read before.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskDetail {
    /// The task, its plan and any failure, exactly as [`Engine::task`] answers.
    #[serde(flatten)]
    pub response: QueryResponse<Qualified<Task>>,
    /// The task's comments, oldest first, for a source whose tasks have comments.
    ///
    /// **Absent** rather than empty for a source declaring none, for a task that was not
    /// found, and for a task whose comments could not be read — the last with the failure in
    /// the response's `errors`, a source refusing the read (a GitHub draft, which has none)
    /// included, so showing such a task is a partial answer that says why. An empty list says
    /// the source has comments and this task holds none, which is a different thing to tell a
    /// reader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<Comment>>,
    /// The image assets the task holds, in the order its content first references them —
    /// `[]` for a task that holds none.
    ///
    /// **Absent** for a task that was not found, and for one whose assets could not be read —
    /// the failure then in the response's `errors`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<Vec<Asset>>,
    /// How the read used its source's local cache: `{"served": "cache" | "remote",
    /// "bypassed": null | "<reason>"}` — absent for a source keeping none, and for an id
    /// naming no configured source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheUse>,
    /// The task's forward dependency edges, exactly as `task deps` answers them — present only
    /// when the read asked for them (`task show --with-deps`), and absent for a task that was not
    /// found, or whose edges could not be read, the failure then in the response's `errors`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<super::QualifiedEdge>>,
}

/// One document as `document show` reports it: the response every show verb answers with,
/// and the document's image assets beside it.
///
/// The response is flattened rather than nested, so a reader of `document show --json`
/// written before assets existed reads exactly the members it read before.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentDetail {
    /// The document, its plan and any failure, exactly as [`Engine::document`] answers.
    #[serde(flatten)]
    pub response: QueryResponse<Qualified<Document>>,
    /// The image assets the document holds, in the order its content first references them
    /// — `[]` for a document that holds none.
    ///
    /// **Absent** for a document that was not found, and for one whose assets could not be
    /// read — the failure then in the response's `errors`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<Vec<Asset>>,
}

/// Several tasks as `task show-many` reports them: one [`TaskDetail`] per id asked for, in
/// the order they were asked for.
///
/// An object rather than a bare list, for the reason [`CommentList`] is one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskDetails {
    /// One detail per id, in request order — the document `task show <ID>` answers for that
    /// id, except that an id it would refuse outright, or answer with nothing, carries why
    /// in that detail's own `errors` instead, so it never refuses the others.
    pub details: Vec<TaskDetail>,
}

impl Engine {
    /// One task by its qualified id, with its comments when its source has them.
    ///
    /// The task is read exactly as [`task`](Self::task) reads it, and the comments are read
    /// only once the task was found — a source declaring no comments is never asked. Both
    /// halves go through the source's one [`get_task_details`] call, so a source that reads
    /// an item and its first page of comments in one request answers in one.
    ///
    /// [`get_task_details`]: onetaskgraph_plugin_api::TaskSource::get_task_details
    ///
    /// # Errors
    ///
    /// As [`task`](Self::task). A comment read that fails is not an error: it lands in the
    /// response's `errors` beside the task that was read.
    pub async fn task_detail(&self, id: &GlobalId) -> Result<TaskDetail, EngineError> {
        let name = self.known(&id.source)?;
        let mut details = self
            .source_details(&name, std::slice::from_ref(id), true)
            .await;
        Ok(details.remove(0))
    }

    /// Several tasks by their qualified ids, each with its comments when `comments` is set and
    /// its source has them — the ids may span sources.
    ///
    /// Each source is asked once, for every id naming it, through its
    /// [`get_task_details`](onetaskgraph_plugin_api::TaskSource::get_task_details); what each
    /// detail holds is what [`task_detail`](Self::task_detail), or [`task`](Self::task) when
    /// `comments` is unset, answers for that id. An id that answer would refuse — it names no
    /// configured source — or answer with nothing — its source holds no such task — carries
    /// that in its own detail's `errors`, so one unreadable id never refuses the others, and a
    /// caller reads every failure from the same place.
    pub async fn task_details(&self, ids: &[GlobalId], comments: bool) -> TaskDetails {
        let mut details: Vec<Option<TaskDetail>> = vec![None; ids.len()];
        let mut named: Vec<&SourceName> = Vec::new();
        for id in ids {
            if !named.contains(&&id.source) {
                named.push(&id.source);
            }
        }
        for name in named {
            let at: Vec<usize> = (0..ids.len())
                .filter(|index| ids[*index].source == *name)
                .collect();
            let asked: Vec<GlobalId> = at.iter().map(|index| ids[*index].clone()).collect();
            let answered = match self.known(name) {
                Ok(name) => self.source_details(&name, &asked, comments).await,
                Err(refusal) => asked
                    .iter()
                    .map(|_| TaskDetail {
                        response: failed_response(
                            name,
                            SourceError::Config {
                                message: refusal.to_string(),
                            },
                        ),
                        comments: None,
                        assets: None,
                        cache: None,
                        depends_on: None,
                    })
                    .collect(),
            };
            for ((index, id), mut detail) in at.into_iter().zip(&asked).zip(answered) {
                if detail.response.items.is_empty() && detail.response.errors.is_empty() {
                    detail.response.errors.push(SourceFailure {
                        source: id.source.clone(),
                        error: SourceError::Refused {
                            message: no_such_task(id).to_string(),
                        },
                    });
                }
                details[index] = Some(detail);
            }
        }
        TaskDetails {
            details: details.into_iter().flatten().collect(),
        }
    }

    /// One detail per id of `ids`, every one of which names the configured source `name`, read
    /// with one call of that source.
    async fn source_details(
        &self,
        name: &SourceName,
        ids: &[GlobalId],
        comments: bool,
    ) -> Vec<TaskDetail> {
        let mut answer = Answer::new();
        let selected = answer.split(self, std::slice::from_ref(name));
        let Some(source) = selected.first() else {
            // A source that never built: every id is answered with the failure `task` answers
            // with, and nothing is asked.
            return ids
                .iter()
                .map(|_| TaskDetail {
                    response: QueryResponse {
                        items: Vec::new(),
                        next: None,
                        plan: QueryPlan::default(),
                        errors: answer.errors.clone(),
                        unanswered: Vec::new(),
                    },
                    comments: None,
                    assets: None,
                    cache: None,
                    depends_on: None,
                })
                .collect();
        };
        let commented = comments && source.source().capabilities().comments.is_native();
        let page = PageRequest {
            cursor: None,
            limit: source.source().capabilities().max_page_size.max(1),
        };
        let natives: Vec<NativeId> = ids.iter().map(|id| id.native.clone()).collect();
        let mut read = source
            .source()
            .get_task_details(&natives, commented.then_some(&page))
            .await;
        if read.len() != ids.len() {
            let message = format!(
                "the source answered {} task details for the {} ids it was asked for",
                read.len(),
                ids.len()
            );
            read = ids
                .iter()
                .map(|_| {
                    Err(SourceError::Malformed {
                        message: message.clone(),
                    })
                })
                .collect();
        }
        let mut details = Vec::with_capacity(ids.len());
        for (id, result) in ids.iter().zip(read) {
            let (found, first) = match result {
                Ok(Some(TaskDetailRead { task, comments })) => (Ok(Some(task)), comments),
                Ok(None) => (Ok(None), None),
                Err(error) => (Err(error), None),
            };
            let qualified = GlobalId::new(source.name().clone(), id.native.clone());
            let mut response = Answer::new().one_response(source, found, |task| {
                delivery::qualified_task(qualified, super::authored_task(source, task))
            });
            let comments = if commented && !response.items.is_empty() {
                let walked = match first {
                    Some(Ok(Some(first))) => walk_from(source, &id.native, first, page.limit).await,
                    Some(Ok(None)) => Ok(None),
                    Some(Err(error)) => Err(error),
                    // A source that was asked for the first page and answered none of it is
                    // asked the way a source of one comment page at a time always is.
                    None => walk(source, &id.native).await,
                };
                match walked {
                    Ok(comments) => comments,
                    Err(error) => {
                        response.errors.push(SourceFailure {
                            source: source.name().clone(),
                            error,
                        });
                        None
                    }
                }
            } else {
                None
            };
            let assets = match response.items.first() {
                Some(task) => match source.source().task_assets(&id.native).await {
                    Ok(listed) => Some(assets::ordered(listed, task.item.content.as_deref())),
                    Err(error) => {
                        response.errors.push(SourceFailure {
                            source: source.name().clone(),
                            error,
                        });
                        None
                    }
                },
                None => None,
            };
            details.push(TaskDetail {
                response,
                comments,
                assets,
                cache: self.cache_use(id),
                depends_on: None,
            });
        }
        details
    }

    /// The image assets the task or document `item`, as `response` read it, holds — in the
    /// order its content first references them — or `None` when it was not found or its
    /// assets could not be read, the failure then pushed onto `response`'s errors.
    async fn assets_of<T>(
        &self,
        response: &mut QueryResponse<Qualified<T>>,
        content: impl Fn(&T) -> Option<&str>,
        owner: assets::Owner,
    ) -> Option<Vec<Asset>> {
        let item = response.items.first()?;
        let source = self
            .ready()
            .find(|source| source.name() == &item.id.source)?;
        let listed = match owner {
            assets::Owner::Document => source.source().document_assets(&item.id.native).await,
            assets::Owner::Task => source.source().task_assets(&item.id.native).await,
        };
        match listed {
            Ok(listed) => Some(assets::ordered(listed, content(&item.item))),
            Err(error) => {
                response.errors.push(SourceFailure {
                    source: source.name().clone(),
                    error,
                });
                None
            }
        }
    }

    /// One task by its qualified id with its image assets beside it, and without its comments
    /// — what `task show --no-comments` answers with.
    ///
    /// # Errors
    ///
    /// As [`task`](Self::task).
    pub async fn task_without_comments(&self, id: &GlobalId) -> Result<TaskDetail, EngineError> {
        let mut response = self.task(id).await?;
        let assets = self
            .assets_of(
                &mut response,
                |task: &Task| task.content.as_deref(),
                assets::Owner::Task,
            )
            .await;
        Ok(TaskDetail {
            response,
            comments: None,
            assets,
            cache: self.cache_use(id),
            depends_on: None,
        })
    }

    /// One document by its qualified id with its image assets beside it: what `document
    /// show` answers with.
    ///
    /// # Errors
    ///
    /// As [`document`](Self::document).
    pub async fn document_detail(&self, id: &GlobalId) -> Result<DocumentDetail, EngineError> {
        let mut response = self.document(id).await?;
        let assets = self
            .assets_of(
                &mut response,
                |document: &Document| document.content.as_deref(),
                assets::Owner::Document,
            )
            .await;
        Ok(DocumentDetail { response, assets })
    }

    /// Every comment on one task, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::NoComments`] for a source whose tasks have none, before
    /// anything is read; [`EngineError::NoSuchTask`] when the source holds no such task; and
    /// [`EngineError::SourceFailed`] when the source could not answer.
    pub async fn comments(&self, task: &GlobalId) -> Result<CommentList, EngineError> {
        let source = self.commented(&task.source)?;
        // A caching source's comments are read beside the task, in the one request its detail
        // read costs, so the read carries the task's version: what keys the entry it refreshes,
        // and what a pin is held to.
        let walked = if self.caches(&task.source) {
            let walked = walk_detail(source, &task.native).await;
            // The task's assets complete its entry, as a `task show` of it does, so a later
            // pinned read of either is answered whole. The source answers them from the read
            // just made, or from the entry itself, and a failure here costs only the entry.
            if matches!(walked, Ok(Some(_))) {
                let _ = source.source().task_assets(&task.native).await;
            }
            walked
        } else {
            walk(source, &task.native).await
        };
        match walked {
            Ok(Some(comments)) => Ok(CommentList {
                comments,
                cache: self.cache_use(task),
            }),
            Ok(None) => Err(no_such_task(task)),
            Err(error) => Err(failed(source, error)),
        }
    }

    /// [`task_detail`](Self::task_detail), answered from `id`'s source's local cache when it
    /// holds the task at `pin` or a later version — see [`Engine::pin`].
    ///
    /// # Errors
    ///
    /// As [`task_detail`](Self::task_detail).
    pub async fn task_detail_pinned(
        &self,
        id: &GlobalId,
        pin: Option<&str>,
    ) -> Result<TaskDetail, EngineError> {
        if let Some(pin) = pin {
            self.pin(id, pin);
        }
        self.task_detail(id).await
    }

    /// [`task_without_comments`](Self::task_without_comments), pinned as
    /// [`task_detail_pinned`](Self::task_detail_pinned) is.
    ///
    /// # Errors
    ///
    /// As [`task`](Self::task).
    pub async fn task_without_comments_pinned(
        &self,
        id: &GlobalId,
        pin: Option<&str>,
    ) -> Result<TaskDetail, EngineError> {
        if let Some(pin) = pin {
            self.pin(id, pin);
        }
        self.task_without_comments(id).await
    }

    /// [`task_details`](Self::task_details), with each id `pins` names pinned to the version
    /// it gives — see [`Engine::pin`].
    pub async fn task_details_pinned(
        &self,
        ids: &[GlobalId],
        comments: bool,
        pins: &[(GlobalId, String)],
    ) -> TaskDetails {
        for (id, version) in pins {
            self.pin(id, version);
        }
        self.task_details(ids, comments).await
    }

    /// [`comments`](Self::comments), pinned as [`task_detail_pinned`](Self::task_detail_pinned)
    /// is.
    ///
    /// # Errors
    ///
    /// As [`comments`](Self::comments).
    pub async fn comments_pinned(
        &self,
        task: &GlobalId,
        pin: Option<&str>,
    ) -> Result<CommentList, EngineError> {
        if let Some(pin) = pin {
            self.pin(task, pin);
        }
        self.comments(task).await
    }

    /// Add one comment to a task, answering with the comment as its source now holds it.
    ///
    /// # Errors
    ///
    /// As [`comments`](Self::comments), plus [`EngineError::CommentsNotWritable`] for a
    /// source whose comments cannot be written, before anything is written.
    pub async fn add_comment(
        &self,
        task: &GlobalId,
        comment: &NewComment,
    ) -> Result<Comment, EngineError> {
        let source = self.writable_comments(&task.source)?;
        let rewrite = self
            .admit_existing(
                source,
                &task.to_string(),
                &task.native,
                Held::Task,
                &Exposure::text(comment.body.as_str())
                    .and(Exposure::metadata(comment.author.iter().cloned())),
            )
            .await?;
        let comment = &NewComment {
            body: rewritten_body(source, &rewrite, &comment.body)?,
            author: comment.author.clone(),
        };
        match source.source().add_comment(&task.native, comment).await {
            Ok(Some(added)) => Ok(added),
            Ok(None) => Err(no_such_task(task)),
            Err(error) => Err(failed(source, error)),
        }
    }

    /// Replace one comment's body, answering with the comment as its source now holds it.
    ///
    /// # Errors
    ///
    /// As [`add_comment`](Self::add_comment), plus [`EngineError::NoSuchComment`] when the
    /// task has no comment under `comment`.
    pub async fn edit_comment(
        &self,
        task: &GlobalId,
        comment: &NativeId,
        body: &CommentBody,
    ) -> Result<Comment, EngineError> {
        let source = self.writable_comments(&task.source)?;
        let rewrite = self
            .admit_existing(
                source,
                &task.to_string(),
                &task.native,
                Held::Task,
                &Exposure::text(body.as_str()),
            )
            .await?;
        let body = &rewritten_body(source, &rewrite, body)?;
        match source
            .source()
            .edit_comment(&task.native, comment, body)
            .await
        {
            Ok(Some(edited)) => Ok(edited),
            Ok(None) => Err(missing(source, task, comment).await),
            Err(error) => Err(failed(source, error)),
        }
    }

    /// Remove one comment from a task, answering with the id it removed.
    ///
    /// # Errors
    ///
    /// As [`edit_comment`](Self::edit_comment).
    pub async fn delete_comment(
        &self,
        task: &GlobalId,
        comment: &NativeId,
    ) -> Result<DeletedComment, EngineError> {
        let source = self.writable_comments(&task.source)?;
        match source.source().delete_comment(&task.native, comment).await {
            Ok(Some(deleted)) => {
                let (result, fallback, spent) = self.expected_parts(task);
                Ok(DeletedComment {
                    deleted,
                    result,
                    fallback,
                    spent,
                })
            }
            Ok(None) => Err(missing(source, task, comment).await),
            Err(error) => Err(failed(source, error)),
        }
    }

    /// The configured source called `name`, in whichever state it is in.
    fn configured(&self, name: &SourceName) -> Option<&ConfiguredSource> {
        self.sources.iter().find(|source| source.name() == name)
    }

    /// The built source called `name`, when its tasks have comments.
    pub(crate) fn commented(&self, name: &SourceName) -> Result<&ResolvedSource, EngineError> {
        let name = self.known(name)?;
        match self.configured(&name) {
            Some(ConfiguredSource::Ready(source)) => {
                if source.source().capabilities().comments.is_native() {
                    Ok(source)
                } else {
                    Err(EngineError::NoComments {
                        name: name.to_string(),
                        kind: source.kind().to_owned(),
                    })
                }
            }
            Some(ConfiguredSource::Unavailable(source)) => Err(EngineError::SourceUnavailable {
                name: name.to_string(),
                error: source.error().clone(),
            }),
            // `known` has just said a source by this name is configured, and `configured`
            // reads the same list it read.
            None => Err(EngineError::NoSources),
        }
    }

    /// The built source called `name`, when its tasks have comments it can write.
    pub(super) fn writable_comments(
        &self,
        name: &SourceName,
    ) -> Result<&ResolvedSource, EngineError> {
        let source = self.commented(name)?;
        if source.source().writes().is_supported() {
            return Ok(source);
        }
        Err(EngineError::CommentsNotWritable {
            name: source.name().to_string(),
            kind: source.kind().to_owned(),
        })
    }
}

/// Every comment on `task`, walked to the end of the source's pages, or `None` when the
/// source holds no such task.
///
/// Each page is asked at the source's own ceiling, and each is held to the two refusals every
/// pagination loop of this engine owes: a page longer than the one asked for, and a cursor
/// handed back unchanged. What the walk accumulates is the caller's answer and nothing else.
async fn walk(
    source: &ResolvedSource,
    task: &NativeId,
) -> Result<Option<Vec<Comment>>, SourceError> {
    let limit = source.source().capabilities().max_page_size.max(1);
    let request = PageRequest {
        cursor: None,
        limit,
    };
    let Some(first) = source.source().task_comments(task, &request).await? else {
        return Ok(None);
    };
    walk_from(source, task, first, limit).await
}

/// [`walk`], starting from the task's detail read rather than its first comment page alone.
async fn walk_detail(
    source: &ResolvedSource,
    task: &NativeId,
) -> Result<Option<Vec<Comment>>, SourceError> {
    let limit = source.source().capabilities().max_page_size.max(1);
    let request = PageRequest {
        cursor: None,
        limit,
    };
    let read = source
        .source()
        .get_task_details(std::slice::from_ref(task), Some(&request))
        .await;
    // One answer, about this task, held to the very check a copy's detail read of one task
    // is: another number of answers, or one about another task, would put some other read's
    // comments under it.
    let read = super::copy::single_detail(task, read)?;
    match read {
        Some(TaskDetailRead {
            comments: Some(first),
            ..
        }) => match first? {
            Some(first) => walk_from(source, task, first, limit).await,
            None => Ok(None),
        },
        Some(_) => walk(source, task).await,
        None => Ok(None),
    }
}

/// [`walk`], from a first page already in hand — the one a detail read carried beside the
/// task — asked at `limit`.
async fn walk_from(
    source: &ResolvedSource,
    task: &NativeId,
    first: Page<Comment>,
    limit: u32,
) -> Result<Option<Vec<Comment>>, SourceError> {
    let mut comments = Vec::new();
    let mut cursor: Option<Cursor> = None;
    let mut page = first;
    loop {
        fits(page.items.len(), limit)?;
        unrepeated(
            page.next.as_ref(),
            cursor.as_ref(),
            "walking a task's comments",
        )?;
        comments.extend(page.items);
        let Some(next) = page.next else {
            return Ok(Some(comments));
        };
        cursor = Some(next);
        let request = PageRequest {
            cursor: cursor.clone(),
            limit,
        };
        // A task that is gone part way through a walk is gone: reporting the comments read
        // before it went would describe a task nobody can address any more.
        let Some(read) = source.source().task_comments(task, &request).await? else {
            return Ok(None);
        };
        page = read;
    }
}

/// Narrow one page of a source's tasks to those with a comment created or last edited at or
/// after `since`, for a source that does not apply that predicate itself.
///
/// This is the one predicate the engine cannot answer from the row: a task does not carry its
/// comments. So each task the other predicates left to the engine already keep — `local` —
/// has its comments read, page by page, until one matches or they run out; a task every
/// other predicate drops is never asked about. What is held is one task's page of comments
/// at a time, and nothing of it outlives the call. A source whose tasks have no comments at
/// all holds no comment activity, so none of its tasks is kept and it is asked nothing, and a
/// task gone from the source by the time its comments are read is gone from the answer.
///
/// # Errors
///
/// Returns whatever the source returned for a comment read, and the two refusals every
/// pagination loop of this engine owes.
pub(super) async fn commented_since(
    source: &ResolvedSource,
    local: &super::local::LocalTasks,
    page: Page<Task>,
    since: DateTime<Utc>,
) -> Result<Page<Task>, SourceError> {
    if !source.source().capabilities().comments.is_native() {
        return Ok(Page {
            items: Vec::new(),
            next: page.next,
        });
    }
    let query = TaskQuery {
        commented_since: Some(since),
        ..TaskQuery::default()
    };
    let mut kept = Vec::new();
    for task in page.items {
        if local.keeps(&task) && any_comment_matches(source, &task.id, &query).await? {
            kept.push(task);
        }
    }
    Ok(Page {
        items: kept,
        next: page.next,
    })
}

/// Whether one of `task`'s comments satisfies `query`'s comment activity, walking the source's
/// comment pages only as far as the first that does.
async fn any_comment_matches(
    source: &ResolvedSource,
    task: &NativeId,
    query: &TaskQuery,
) -> Result<bool, SourceError> {
    let limit = source.source().capabilities().max_page_size.max(1);
    let mut cursor: Option<Cursor> = None;
    loop {
        let request = PageRequest {
            cursor: cursor.clone(),
            limit,
        };
        let Some(page) = source.source().task_comments(task, &request).await? else {
            return Ok(false);
        };
        fits(page.items.len(), limit)?;
        unrepeated(
            page.next.as_ref(),
            cursor.as_ref(),
            "walking a task's comments",
        )?;
        if query.comments_match(&page.items) {
            return Ok(true);
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => return Ok(false),
        }
    }
}

/// Which of the two things an edit or a delete named was not there.
///
/// Asked only once the source has already said one of them is missing, so a comment verb
/// that succeeds costs its source exactly one call.
async fn missing(source: &ResolvedSource, task: &GlobalId, comment: &NativeId) -> EngineError {
    match source.source().get_task(&task.native).await {
        Ok(Some(_)) => EngineError::NoSuchComment {
            task: task.to_string(),
            comment: comment.to_string(),
        },
        Ok(None) => no_such_task(task),
        Err(error) => failed(source, error),
    }
}

/// A response for an id the engine could not ask any source about, carrying why.
fn failed_response(source: &SourceName, error: SourceError) -> QueryResponse<Qualified<Task>> {
    QueryResponse {
        items: Vec::new(),
        next: None,
        plan: QueryPlan::default(),
        errors: vec![SourceFailure {
            source: source.clone(),
            error,
        }],
        unanswered: Vec::new(),
    }
}

fn no_such_task(task: &GlobalId) -> EngineError {
    EngineError::NoSuchTask {
        id: task.to_string(),
    }
}

fn failed(source: &ResolvedSource, error: SourceError) -> EngineError {
    EngineError::SourceFailed {
        name: source.name().to_string(),
        error,
    }
}

/// `body` as a sanitisation passed it, refused — nothing written — when the replacement left
/// it empty, which no source takes as a comment.
pub(super) fn rewritten_body(
    source: &ResolvedSource,
    rewrite: &super::boundary::Rewrite,
    body: &CommentBody,
) -> Result<CommentBody, EngineError> {
    CommentBody::new(rewrite.text(body.as_str())).map_err(|error| failed(source, error))
}
