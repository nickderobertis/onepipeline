//! Every page of a task listing, walked to exhaustion: `task list --all`.
//!
//! A caller that wants every match has no use for the page token in between; walking it here,
//! once, is what keeps every such caller from writing the same loop — and from looping forever
//! on a source that answers a cursor it was just given. A page token the walk has already sent
//! is refused, naming it, rather than asked for again.

use std::collections::BTreeSet;

use std::num::NonZeroU32;

use onetaskgraph_plugin_api::{Direction, Task};

use super::{
    Completeness, DependencyRequest, Engine, EngineError, Paging, Qualified, QualifiedEdge,
    TaskDetail, TaskRequest,
};
use crate::{GlobalId, QueryResponse};

impl Engine {
    /// [`tasks`](Self::tasks), every page of it: the walk starts from `request`'s own page
    /// token, follows each page's `next` until none is left, and answers one response holding
    /// every item in the order the pages gave them, the first page's plan, and every source
    /// failure and unanswered private source any page reported, each once.
    ///
    /// # Errors
    ///
    /// As [`tasks`](Self::tasks), and [`EngineError::RepeatedPage`] when a page answers a
    /// token the walk has already followed — a source repeating a cursor, which would
    /// otherwise be asked for the same page forever.
    pub async fn tasks_all(
        &self,
        request: &TaskRequest,
    ) -> Result<QueryResponse<Qualified<Task>>, EngineError> {
        let mut request = request.clone();
        let mut followed: BTreeSet<String> = request
            .paging
            .token
            .iter()
            .map(ToString::to_string)
            .collect();
        let mut all = self.tasks(&request).await?;
        while let Some(next) = all.next.take() {
            let token = next.to_string();
            if !followed.insert(token.clone()) {
                return Err(EngineError::RepeatedPage { token });
            }
            request.paging.token = Some(next);
            let page = self.tasks(&request).await?;
            all.items.extend(page.items);
            for error in page.errors {
                if !all.errors.contains(&error) {
                    all.errors.push(error);
                }
            }
            for unanswered in page.unanswered {
                if !all.unanswered.contains(&unanswered) {
                    all.unanswered.push(unanswered);
                }
            }
            all.next = page.next;
        }
        Ok(all)
    }
}

impl Engine {
    /// [`task_dependencies`](Self::task_dependencies), every page of it, walked as
    /// [`tasks_all`](Self::tasks_all) walks a listing.
    ///
    /// # Errors
    ///
    /// As [`task_dependencies`](Self::task_dependencies), and [`EngineError::RepeatedPage`]
    /// when a page answers a token the walk has already followed.
    pub async fn task_dependencies_all(
        &self,
        request: &DependencyRequest,
    ) -> Result<QueryResponse<QualifiedEdge>, EngineError> {
        let mut request = request.clone();
        let mut followed: BTreeSet<String> = request
            .paging
            .token
            .iter()
            .map(ToString::to_string)
            .collect();
        let mut all = self.task_dependencies(&request).await?;
        while let Some(next) = all.next.take() {
            let token = next.to_string();
            if !followed.insert(token.clone()) {
                return Err(EngineError::RepeatedPage { token });
            }
            request.paging.token = Some(next);
            let page = self.task_dependencies(&request).await?;
            all.items.extend(page.items);
            for error in page.errors {
                if !all.errors.contains(&error) {
                    all.errors.push(error);
                }
            }
            for unanswered in page.unanswered {
                if !all.unanswered.contains(&unanswered) {
                    all.unanswered.push(unanswered);
                }
            }
            all.next = page.next;
        }
        Ok(all)
    }

    /// `detail`, a read of the task `id`, with that task's forward dependency edges beside it
    /// — every edge `task deps` answers for it, walked to its last page — so a caller reading a
    /// task and its edges asks for both at once. A source that already read the edges with the
    /// task (a GitHub board reads `blockedBy` with the item) answers them from that read.
    ///
    /// A task that was not found carries none. A failure reading the edges is not an error: it
    /// joins the response's `errors`, and `depends_on` stays absent.
    ///
    /// # Errors
    ///
    /// [`EngineError::RepeatedPage`] when the edge walk repeats a page token.
    pub async fn with_task_dependencies(
        &self,
        id: &GlobalId,
        mut detail: TaskDetail,
    ) -> Result<TaskDetail, EngineError> {
        if detail.response.items.is_empty() {
            return Ok(detail);
        }
        let request = DependencyRequest {
            id: id.clone(),
            direction: Direction::DependsOn,
            completeness: Completeness::NamingUnasked,
            paging: Paging {
                limit: NonZeroU32::new(100).expect("a non-zero page"),
                token: None,
            },
        };
        match self.task_dependencies_all(&request).await {
            Ok(edges) => {
                // Edges the task's own source could not answer are not its edges: absent, the
                // failure beside the task.
                let answered = !edges
                    .errors
                    .iter()
                    .any(|failure| failure.source == id.source);
                for error in edges.errors {
                    if !detail.response.errors.contains(&error) {
                        detail.response.errors.push(error);
                    }
                }
                for unanswered in edges.unanswered {
                    if !detail.response.unanswered.contains(&unanswered) {
                        detail.response.unanswered.push(unanswered);
                    }
                }
                detail.depends_on = answered.then_some(edges.items);
            }
            Err(error @ EngineError::RepeatedPage { .. }) => return Err(error),
            Err(error) => detail.response.errors.push(crate::SourceFailure {
                source: id.source.clone(),
                error: onetaskgraph_plugin_api::SourceError::Refused {
                    message: error.to_string(),
                },
            }),
        }
        Ok(detail)
    }
}
