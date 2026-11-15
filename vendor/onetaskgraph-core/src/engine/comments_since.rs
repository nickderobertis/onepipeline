//! Every comment one source's tasks hold that was made or edited since an instant, with the
//! watermark a later read may start from: `task comments <SOURCE> --since`.
//!
//! The listing is composed of what every source already answers — the task narrowing `task
//! list --commented-since` makes, walked to its last page, and each matching task's comments —
//! so it needs nothing new of a plugin but what only a plugin can say: who wrote a comment
//! ([`TaskSource::comment_author_kind`]), how long its reads may lag its writes
//! ([`TaskSource::comment_consistency_window`]), and the time its backend last stated
//! ([`TaskSource::served_at`]). The watermark is the earlier of that time and this host's clock
//! at the read's start, set back by the window, so a caller resuming from it never needs to know
//! a destination's lag: a comment the source had not yet shown is read on the next pass, and
//! one returned twice carries the same id and version both times.
//!
//! [`TaskSource::comment_author_kind`]: onetaskgraph_plugin_api::TaskSource::comment_author_kind
//! [`TaskSource::comment_consistency_window`]: onetaskgraph_plugin_api::TaskSource::comment_consistency_window
//! [`TaskSource::served_at`]: onetaskgraph_plugin_api::TaskSource::served_at

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use chrono::{DateTime, SecondsFormat, Utc};
use onetaskgraph_plugin_api::{
    AuthorKind, MetadataMatch, NativeId, PageRequest, SourceName, StatusCategory, TaskDetailRead,
    TaskSource,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Engine, EngineError, Filters, Paging, ProjectSelector, TaskRequest};
use crate::GlobalId;

/// The version of the [`CommentsSince`] document's shape, written into every answer.
pub const COMMENTS_SINCE_SCHEMA_VERSION: u32 = 1;

/// What `task comments <SOURCE> --since` answers with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommentsSince {
    /// The version of this document's shape: `1`.
    pub schema_version: u32,
    /// The instant a later read may start from: the earlier of the source's own time and this
    /// host's at the read's start, set back by the source's consistency window. A later read
    /// from it may return a comment again, with the same `id` and `version`.
    pub watermark: DateTime<Utc>,
    /// Every comment made or edited at or after the instant on a matching task, oldest first by
    /// when it last changed.
    pub comments: Vec<CommentActivity>,
}

/// One comment of a [`CommentsSince`] answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommentActivity {
    /// The task it is on, qualified: `<source>:<native id>`.
    pub item: GlobalId,
    /// Its own id, exactly as its source issued it.
    pub id: NativeId,
    /// Where a person can open it, or `null`.
    // llmlint: ignore[invalid_states_unrepresentable, boundary_inputs_validated] As `Comment::url`, which this is: every entity of the contract carries its web address as an optional string, read back as the source gave it.
    pub url: Option<String>,
    /// Who wrote it, in the source's own spelling, or `null`.
    pub author: Option<String>,
    /// Whether that is a person, a bot, or neither the source can tell.
    pub author_kind: AuthorKind,
    /// When it was written.
    pub created_at: DateTime<Utc>,
    /// When it last changed — its creation, for one never edited.
    pub updated_at: DateTime<Utc>,
    /// The comment's version: when it last changed, at the backend's own precision. Two reads
    /// answering one comment unchanged answer the same version.
    // llmlint: ignore[invalid_states_unrepresentable] Opaque by contract: a caller compares two of them and reads nothing inside one, so a narrower type would promise a format the next backend need not keep.
    pub version: String,
    /// What it says, byte for byte.
    pub body: String,
}

/// A request for every comment one source's tasks hold since an instant.
#[derive(Debug, Clone, PartialEq)]
pub struct CommentsSinceRequest {
    /// The source whose tasks are read.
    pub source: SourceName,
    /// Comments made or edited at or after this instant are answered.
    pub since: DateTime<Utc>,
    /// Keep the tasks holding every one of these, as `task list --metadata` keeps them.
    pub metadata: Vec<MetadataMatch>,
    /// Keep the tasks at one of these categories. Empty keeps every category.
    pub statuses: Vec<StatusCategory>,
    /// Keep only these tasks, each of `source`. Empty keeps every task.
    pub items: Vec<GlobalId>,
    /// How many tasks each page of the listing asks for; every page is read whatever it is.
    pub page_size: NonZeroU32,
}

impl Engine {
    /// Every comment made or edited at or after `request.since` on the tasks of
    /// `request.source` that match its narrowing, oldest first, with the watermark a later read
    /// may start from. Every page of the matching tasks and of each one's comments is read.
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownSource`] for a source nothing configures,
    /// [`EngineError::ItemOutsideSource`] for an `items` entry of another source,
    /// [`EngineError::NoComments`] for a source whose tasks have none, and
    /// [`EngineError::SourceRefused`] carrying the source's failure when it could not be read.
    pub async fn comments_since(
        &self,
        request: &CommentsSinceRequest,
    ) -> Result<CommentsSince, EngineError> {
        let started = Utc::now();
        let source = self.commented(&request.source)?;
        if let Some(outside) = request
            .items
            .iter()
            .find(|item| item.source != request.source)
        {
            return Err(EngineError::ItemOutsideSource {
                item: outside.to_string(),
                source_name: request.source.to_string(),
            });
        }
        let listing = TaskRequest {
            sources: vec![request.source.clone()],
            filters: Filters {
                statuses: request.statuses.clone(),
                ..Filters::default()
            },
            project: ProjectSelector::Any,
            priorities: Vec::new(),
            commented_since: Some(request.since),
            metadata: request.metadata.clone(),
            origin: None,
            include_members: false,
            paging: Paging {
                limit: request.page_size,
                token: None,
            },
        };
        let tasks = self.tasks_all(&listing).await?;
        if let Some(failure) = tasks.errors.into_iter().next() {
            return Err(EngineError::SourceRefused {
                name: failure.source.to_string(),
                error: failure.error,
            });
        }
        let wanted: BTreeSet<String> = request.items.iter().map(ToString::to_string).collect();
        let matched: Vec<GlobalId> = tasks
            .items
            .into_iter()
            .filter(|task| wanted.is_empty() || wanted.contains(&task.id.to_string()))
            .map(|task| task.id)
            .collect();
        // Every matched task's first page of comments in the source's one batched detail read —
        // a source that reads many items together answers them in few requests — and only a
        // task holding more than a page, or one that read could not answer, walked on its own.
        // A caching source's comments are read through `comments`, which keeps its entries.
        let firsts = if self.caches(&request.source) || matched.is_empty() {
            Vec::new()
        } else {
            let page = PageRequest {
                cursor: None,
                limit: source.source().capabilities().max_page_size.max(1),
            };
            let natives: Vec<NativeId> = matched.iter().map(|id| id.native.clone()).collect();
            let read = source
                .source()
                .get_task_details(&natives, Some(&page))
                .await;
            // An answer of another count says nothing of which answer is whose, so every task
            // is read on its own instead.
            if read.len() == natives.len() {
                read
            } else {
                Vec::new()
            }
        };
        let mut comments = Vec::new();
        for (at, task) in matched.iter().enumerate() {
            // Taken only when it is this task's page, whole: anything else is read on its own.
            let first = match firsts.get(at) {
                Some(Ok(Some(TaskDetailRead {
                    task: read,
                    comments: Some(Ok(Some(page))),
                }))) if page.next.is_none() && read.id == task.native => Some(page.items.clone()),
                _ => None,
            };
            let held = match first {
                Some(held) => held,
                None => self.comments(task).await?.comments,
            };
            for comment in held {
                let Some(created_at) = comment.created_at.or(comment.updated_at) else {
                    continue;
                };
                let updated_at = comment.updated_at.unwrap_or(created_at).max(created_at);
                if updated_at < request.since {
                    continue;
                }
                comments.push(CommentActivity {
                    item: task.clone(),
                    author_kind: source.source().comment_author_kind(&comment),
                    id: comment.id,
                    url: comment.url,
                    author: comment.author,
                    created_at,
                    updated_at,
                    version: updated_at.to_rfc3339_opts(SecondsFormat::AutoSi, true),
                    body: comment.body,
                });
            }
        }
        comments.sort_by(|a, b| {
            (a.updated_at, a.created_at, a.item.to_string(), &a.id).cmp(&(
                b.updated_at,
                b.created_at,
                b.item.to_string(),
                &b.id,
            ))
        });
        Ok(CommentsSince {
            schema_version: COMMENTS_SINCE_SCHEMA_VERSION,
            watermark: watermark(source.source(), started),
            comments,
        })
    }
}

/// The instant a read of `source` begun at `started` may later be resumed from.
fn watermark(source: &dyn TaskSource, started: DateTime<Utc>) -> DateTime<Utc> {
    let now = source
        .served_at()
        .map_or(started, |served| served.min(started));
    let window = chrono::TimeDelta::from_std(source.comment_consistency_window())
        .unwrap_or(chrono::TimeDelta::MAX);
    now.checked_sub_signed(window)
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}
