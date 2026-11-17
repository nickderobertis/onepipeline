//! Reading one task and its comments as one revision of it.
//!
//! A `--comments` copy writes a task and its comments together, so what it writes has to be
//! one state the source really held: never a body from before an edit beside comments from
//! after it. A task whose comments fit the first page is read whole in one request — the
//! detail read that names it — and is one revision by construction. One whose comments run
//! longer is read a page at a time, each further page through the same detail read, so every
//! request carries the task beside the page it answers. The pages are one revision when every
//! request carried the same task: the same change token where the source's version coverage
//! says its token covers a task's comments, and otherwise the same task in every member it
//! reports.
//!
//! Where no token covering comments is given — a task with no token, such as a GitHub task
//! changed within the minute before the read, and a task whose source's token does not cover
//! its comments, such as Linear's without its cache attached — the task's own members are all
//! two page reads can be compared by, and a comment edited alone between two pages moves none
//! of them. So such a task whose comments run past one page is read again in full until two
//! consecutive passes agree on every comment, at most [`FULL_PASSES`] of them; one that never
//! settles is left unwritten and reported failed. Whether a token covers comments is the
//! source's [`version_coverage`](onetaskgraph_plugin_api::TaskSource::version_coverage), never
//! the token's presence.
//!
//! When they disagree the task is read again from its first page, once. A second
//! disagreement is not chased further: the task is left unwritten and the copy reports it as
//! failed, because a read that keeps losing a race with a person editing the item would
//! otherwise spend an allowance every session on the host shares, without bound.

use onetaskgraph_plugin_api::{
    Comment, Cursor, NativeId, Page, PageRequest, SourceError, Task, TaskDetailRead,
};

use crate::resolve::ResolvedSource;

use super::super::fetch::{fits, unrepeated};

/// One task as a copy reads it: the task, and every one of its comments.
pub(super) struct Snapshot {
    /// The task, as the first request of the read reported it.
    pub(super) task: Task,
    /// Its comments, oldest first, every page of them.
    pub(super) comments: Vec<Comment>,
}

/// What one attempt at reading a task whole came to.
pub(super) enum Taken {
    /// Every request agreed: one revision of the task.
    Whole(Box<Snapshot>),
    /// The source no longer holds the task.
    Gone,
    /// A later request carried another revision of the task than the first.
    Changed,
    /// A task with no token covering its comments whose comments run past one page, read in
    /// [`FULL_PASSES`] full passes no two consecutive of which agreed.
    Unsettled,
}

/// The page size a copy reads comments in: the most the source declares it answers.
pub(super) fn comment_page(source: &ResolvedSource) -> PageRequest {
    PageRequest {
        cursor: None,
        limit: source.source().capabilities().max_page_size.max(1),
    }
}

/// Read the first page of `id`'s comments beside the task, in one detail read.
///
/// # Errors
///
/// Whatever the source answered the read with.
pub(super) async fn first(
    source: &ResolvedSource,
    id: &NativeId,
) -> Result<Option<TaskDetailRead>, SourceError> {
    let answers = source
        .source()
        .get_task_details_for_copy(std::slice::from_ref(id), &comment_page(source))
        .await;
    single(id, answers)
}

/// The one answer a detail read of `id` alone owes: refused as malformed when the source gave
/// any other number of them, or answered about another task — either would put some other
/// read's content under `id`.
pub(crate) fn single(
    id: &NativeId,
    mut answers: Vec<Result<Option<TaskDetailRead>, SourceError>>,
) -> Result<Option<TaskDetailRead>, SourceError> {
    if answers.len() != 1 {
        return Err(SourceError::Malformed {
            message: format!(
                "the source answered {} details for one read of task {id}",
                answers.len()
            ),
        });
    }
    let answer = answers.pop().expect("exactly one answer")?;
    about(id, answer.as_ref())?;
    Ok(answer)
}

/// Refuse a detail answered about another task than `id`.
fn about(id: &NativeId, read: Option<&TaskDetailRead>) -> Result<(), SourceError> {
    match read {
        Some(read) if read.task.id != *id => Err(SourceError::Malformed {
            message: format!(
                "the source answered a read of task {id} with task {}",
                read.task.id
            ),
        }),
        _ => Ok(()),
    }
}

/// How many full passes over a task's comments a copy makes, at most, looking for two
/// consecutive ones that agree, when no token covering comments vouches for them.
///
/// A task with no token covering its comments leaves nothing a page read carries that moves when
/// a comment alone is edited, so a task whose comments run past one page is read again in full
/// until two passes in a row agree on every comment. Three is two passes and one more for a
/// person who was mid-edit during the first: a task still changing after that is left unwritten
/// and reported failed rather than chased.
pub(super) const FULL_PASSES: usize = 3;

/// Finish reading `id` from the detail read `read` answered with: every further page of its
/// comments, each held to the task the first request carried — and, for a task with no change
/// token covering its comments whose comments run past one page, further full passes until two consecutive ones agree
/// (see [`FULL_PASSES`]).
///
/// # Errors
///
/// Whatever the source answered a page with, the refusals every pagination loop of this engine
/// owes, and [`SourceError::Malformed`] for an answer without the comments it was asked for —
/// on the first page as on any later one. Read as none, it would have a copy delete every
/// copy the destination holds of them.
pub(super) async fn take(
    source: &ResolvedSource,
    id: &NativeId,
    read: TaskDetailRead,
) -> Result<Taken, SourceError> {
    // A batched read answered ahead of this one is held to its id here, as `first` holds its own.
    about(id, Some(&read))?;
    let TaskDetailRead { task, comments } = read;
    let page = match first_page(id, comments)? {
        Some(page) => page,
        None => return Ok(Taken::Gone),
    };
    let paged = page.next.is_some();
    let held_by = HeldBy::of(source);
    let mut held = match walk(source, id, &task, page, held_by).await? {
        Walked::All(comments) => comments,
        Walked::Gone => return Ok(Taken::Gone),
        Walked::Changed => return Ok(Taken::Changed),
    };
    if !(held_by == HeldBy::Token && task.version.is_some()) && paged {
        let mut settled = false;
        for _ in 1..FULL_PASSES {
            let start = PageRequest {
                cursor: None,
                limit: comment_page(source).limit,
            };
            let again = single(
                id,
                source
                    .source()
                    .get_task_details(std::slice::from_ref(id), Some(&start))
                    .await,
            )?;
            let Some(again) = again else {
                return Ok(Taken::Gone);
            };
            if !reads_alike(&task, &again.task, held_by) {
                return Ok(Taken::Changed);
            }
            let Some(page) = first_page(id, again.comments)? else {
                return Ok(Taken::Gone);
            };
            let pass = match walk(source, id, &task, page, held_by).await? {
                Walked::All(comments) => comments,
                Walked::Gone => return Ok(Taken::Gone),
                Walked::Changed => return Ok(Taken::Changed),
            };
            let agrees = same_comments(&held, &pass);
            held = pass;
            if agrees {
                settled = true;
                break;
            }
        }
        if !settled {
            return Ok(Taken::Unsettled);
        }
    }
    // Oldest first, whichever way the source walks its pages: Linear's run from the newest
    // back, each page oldest first within itself. A stable sort leaves a source that walks
    // forward, and comments sharing an instant, in the order the source gave them.
    held.sort_by_key(|comment| comment.created_at);
    Ok(Taken::Whole(Box::new(Snapshot {
        task,
        comments: held,
    })))
}

/// What one walk over a task's comment pages came to.
enum Walked {
    /// Every page, each request carrying the task as the first did.
    All(Vec<Comment>),
    /// The source no longer holds the task.
    Gone,
    /// A later request carried another revision of the task than the first.
    Changed,
}

/// The first page a detail read of `id` answered with, or `None` when the task is gone.
fn first_page(
    id: &NativeId,
    comments: Option<Result<Option<Page<Comment>>, SourceError>>,
) -> Result<Option<Page<Comment>>, SourceError> {
    match comments {
        None => Err(commentless(id)),
        Some(Err(error)) => Err(error),
        Some(Ok(page)) => Ok(page),
    }
}

/// Every comment of `id`, from `page` — its first — through every further page, each request
/// held to `task` as `held_by` says.
async fn walk(
    source: &ResolvedSource,
    id: &NativeId,
    task: &Task,
    page: Page<Comment>,
    held_by: HeldBy,
) -> Result<Walked, SourceError> {
    let limit = comment_page(source).limit;
    fits(page.items.len(), limit)?;
    let mut held = page.items;
    let mut next = page.next;
    let mut asked: Option<Cursor> = None;
    while let Some(cursor) = next {
        unrepeated(Some(&cursor), asked.as_ref(), "reading a task's comments")?;
        let request = PageRequest {
            cursor: Some(cursor.clone()),
            limit,
        };
        let answer = single(
            id,
            source
                .source()
                .get_task_details(std::slice::from_ref(id), Some(&request))
                .await,
        )?;
        let Some(later) = answer else {
            return Ok(Walked::Gone);
        };
        if !reads_alike(task, &later.task, held_by) {
            return Ok(Walked::Changed);
        }
        let Some(page) = first_page(id, later.comments)? else {
            return Ok(Walked::Gone);
        };
        fits(page.items.len(), limit)?;
        held.extend(page.items);
        asked = Some(cursor);
        next = page.next;
    }
    Ok(Walked::All(held))
}

/// Whether two full passes over one task's comments read every comment alike: the same ids,
/// in the same order, each with the same body and the same last-changed time.
fn same_comments(first: &[Comment], later: &[Comment]) -> bool {
    first.len() == later.len()
        && first.iter().zip(later).all(|(first, later)| {
            first.id == later.id && first.body == later.body && first.updated_at == later.updated_at
        })
}

/// What a detail read answered without the comments it was asked for is refused as.
fn commentless(id: &NativeId) -> SourceError {
    SourceError::Malformed {
        message: format!("the source answered a read of task {id}'s comments without them"),
    }
}

/// What two reads of one task are held to when a walk of its comments asks whether it moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeldBy {
    /// Its change token, which the source declares covers a task's comments — the one thing
    /// that lets two equal tokens stand for one state of them.
    Token,
    /// Every member but the token, which says nothing of a change to a comment alone.
    Fields,
}

impl HeldBy {
    fn of(source: &ResolvedSource) -> Self {
        if source
            .source()
            .version_coverage()
            .comments
            .reason()
            .is_none()
        {
            Self::Token
        } else {
            Self::Fields
        }
    }
}

/// Whether two reads of one task agree on it: the same change token when both gave one and
/// `held_by` is the token, and otherwise the same task in every member but the token.
fn reads_alike(first: &Task, later: &Task, held_by: HeldBy) -> bool {
    if let (HeldBy::Token, Some(first), Some(later)) = (held_by, &first.version, &later.version) {
        return first == later;
    }
    let untokened = |task: &Task| Task {
        version: None,
        ..task.clone()
    };
    untokened(first) == untokened(later)
}
