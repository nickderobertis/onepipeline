//! A caching source: the plugin's own source, answering from the cache what a pin vouches for.
//!
//! The engine wraps every source configured with `cache.enabled` in one of these, so every read
//! path the engine has — `task show`, `show-many`, a comment listing and every read a copy
//! makes — meets the cache at one seam, and no verb decides for itself whether an entry is
//! fresh. Everything is forwarded to the plugin's own source except:
//!
//! - **A listing** of tasks, projects or documents is always sent, and each item it carries pins
//!   the reads of it that follow in the same command. It fills no entry.
//! - **A read by id** — a task's, a project's or a document's — is answered from the cache when
//!   it is pinned — by a caller, by a listing, or by a write expecting a version — and the entry
//!   is at that version or a later one, by the source's own ordering. Every other read is sent,
//!   and what it answered is written to the entry. A part the source's token does not cover is
//!   never served.
//! - **A task's forward edges** are served only where the source's token covers them: from the
//!   task's entry, or from an entry of their own kept at the version the source's dependency
//!   read spelled ([`TaskSource::edges_version`]). **A project's forward edges, and a task's
//!   and a project's reverse ones,** are served on the same terms from an entry of their own,
//!   where the source's coverage says its token covers them
//!   ([`ProjectVersionCoverage::forward_edges`], [`TaskSource::reverse_edges_coverage`]).
//! - **A task's comments** are served from the task's entry where the source's token covers
//!   them. A later page of comments is never served or kept, since no one page past the first is
//!   the whole of them.
//! - **An image asset's bytes** are served only for a manifest the cache holds at a covered
//!   version, by the digest it names, and only when the kept bytes still hash to it.
//! - **A write** is sent, and leaves every entry naming its item unserveable — and, where it can
//!   change a relation recorded on another item, every entry of the edges that relation reaches
//!   — unless it was an expected task write, when the source is primed with what the entry kept so it need not read
//!   first, and the entry is advanced to what the write's own last response showed whole, when it
//!   showed it.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use onetaskgraph_plugin_api::{
    Asset, AssetName, AssetWrite, AssetsWritten, Capabilities, Comment, CommentBody, CommentCopy,
    DependencyEdge, Direction, Document, DocumentQuery, DocumentVersionCoverage, Health,
    IssueHolding, IssueRef, ItemKind, ItemWrite, Label, Location, MetadataKey, MetadataRecord,
    Metering, NativeId, NewComment, Page, PageRequest, PartOutcome, Priority, Project,
    ProjectQuery, ProjectVersionCoverage, SourceError, Status, StatusCategory, StatusContradiction,
    Task, TaskDetailRead, TaskQuery, TaskRef, TaskSource, TaskUpdate, TaskUpdateOutcome,
    VersionCoverage, Visibility, WriteJournal, WriteSupport, WriteTarget,
};
use serde_json::Value;

use super::kinds::{ByteDigest, DocumentEntry, EdgeKind, KIND_ENTRY_SCHEMA};
use super::{
    CacheLocation, CacheUse, ENTRY_SCHEMA, Entry, ExpectedWrite, Priming, SourceCache,
    WriteFallback, WritePart, WriteResult,
};

/// One write's future, boxed so one helper can run any of the trait's writes.
type Writing<'a, T> = Pin<Box<dyn Future<Output = Result<T, SourceError>> + Send + 'a>>;

/// One batched detail read's future.
type Reading<'a> =
    Pin<Box<dyn Future<Output = Vec<Result<Option<TaskDetailRead>, SourceError>>> + Send + 'a>>;

/// Why a read could not be answered from the cache.
enum Miss {
    /// Nothing pins it: an unpinned read goes to the source, as it always did.
    Unpinned,
    /// A part it needs is one the source's token does not cover.
    Bypassed(String),
    /// No entry at the pinned version or a later one.
    Behind,
}

/// The plugin's own source, behind the cache of it.
pub(crate) struct CachedSource {
    inner: Box<dyn TaskSource>,
    cache: Arc<SourceCache>,
}

impl CachedSource {
    /// `inner`, behind a cache at `location`, and that cache.
    ///
    /// The source is told the cache stands in front of it — [`TaskSource::attach_cache`], once —
    /// before anything is read and before its coverage is, so a source that widens its reads for
    /// the cache declares the coverage that widening buys.
    pub(crate) fn new(
        inner: Box<dyn TaskSource>,
        location: CacheLocation,
    ) -> (Self, Arc<SourceCache>) {
        inner.attach_cache();
        let cache = SourceCache::new(
            location,
            inner.version_coverage(),
            inner.project_version_coverage(),
            inner.document_version_coverage(),
        );
        (
            Self {
                inner,
                cache: cache.clone(),
            },
            cache,
        )
    }

    /// `inner`, behind a cache already made — for this module's tests, which watch the cache
    /// from beside the source.
    #[cfg(test)]
    fn over(inner: Box<dyn TaskSource>, cache: Arc<SourceCache>) -> Self {
        Self { inner, cache }
    }

    /// The version reads of `id` are pinned to in this command, if any: a caller's pin, a
    /// write expecting it, or the version a listing carried.
    fn pin(&self, id: &NativeId) -> Option<String> {
        let command = self.cache.command();
        command
            .pins
            .get(id)
            .or_else(|| {
                command
                    .expected
                    .as_ref()
                    .filter(|expected| expected.id == *id)
                    .map(|expected| &expected.version)
            })
            .or_else(|| command.listed.get(id))
            .cloned()
    }

    /// The entry of task `id`, unless it was recorded under another assignees setting than
    /// the source now has: an entry written before a source opted in to assignees holds none,
    /// and one written while it had holds some a source that has since opted out never reports,
    /// so either is a miss rather than an answer the source would not give.
    fn entry(&self, id: &NativeId) -> Option<Entry> {
        let holds = self.inner.capabilities().assignees.is_native();
        self.cache
            .read(id)
            .filter(|entry| entry.task.assignees.is_some() == holds)
    }

    /// Whether `held` is at `pin` or a later version of the same task.
    fn fresh(&self, held: &str, pin: &str) -> bool {
        matches!(
            self.inner.order_versions(held, pin),
            Some(Ordering::Equal | Ordering::Greater)
        )
    }

    /// The entry a read of `id` may be answered from: pinned, covered, and at the pin or later.
    fn servable(&self, id: &NativeId, comments: bool) -> Result<Entry, Miss> {
        let pin = self.pin(id).ok_or(Miss::Unpinned)?;
        if let Some(reason) = self.cache.coverage.fields.reason() {
            return Err(Miss::Bypassed(reason.to_owned()));
        }
        if comments && let Some(reason) = self.cache.coverage.comments.reason() {
            return Err(Miss::Bypassed(reason.to_owned()));
        }
        let entry = self.entry(id).ok_or(Miss::Behind)?;
        let fresh = entry.version().is_some_and(|held| self.fresh(held, &pin));
        if fresh && (!comments || entry.comments.is_some()) {
            Ok(entry)
        } else {
            Err(Miss::Behind)
        }
    }

    /// Record that the read of `id` was answered from `entry`.
    fn served(&self, entry: &Entry) {
        let mut command = self.cache.command();
        command
            .used
            .insert(entry.task.id.clone(), CacheUse::cache());
        command.served.insert(entry.task.id.clone(), entry.clone());
    }

    /// Record that the read of `id` went to the source, for `miss`, and answered `version`.
    fn missed(&self, id: &NativeId, miss: &Miss, version: Option<&str>) {
        let bypassed = match miss {
            Miss::Bypassed(reason) => Some(reason.clone()),
            Miss::Unpinned | Miss::Behind => self
                .cache
                .coverage
                .fields
                .reason()
                .map(str::to_owned)
                .or_else(|| {
                    version.is_none().then(|| {
                        "the source gave this task no change token, so the cache cannot \
                         vouch for any copy of it"
                            .to_owned()
                    })
                }),
        };
        let mut command = self.cache.command();
        command.used.insert(id.clone(), CacheUse::remote(bypassed));
        command.served.remove(id);
        match version {
            Some(version) => command.read.insert(id.clone(), version.to_owned()),
            None => command.read.remove(id),
        };
    }

    /// Write what a read of `task` from the source answered, keeping what the entry already
    /// held at the very same version and the read did not carry.
    ///
    /// Comments are kept only where the source's token covers them: one that does not would
    /// never have them served, and nothing a reader never sees — a comment's body, its author's
    /// name — is written down.
    ///
    /// A task read without a token is not written at all: an entry is keyed on its version, and
    /// one without could vouch for nothing.
    fn remember(&self, task: &Task, comments: Option<Vec<Comment>>) {
        if task.version.is_none() {
            return;
        }
        let comments = comments.filter(|_| self.cache.coverage.comments.reason().is_none());
        let held = self
            .cache
            .read(&task.id)
            .filter(|held| held.version().is_some() && held.version() == task.version.as_deref());
        let (assets, edges, kept) = match held {
            Some(held) => (held.assets, held.edges, held.comments),
            None => (None, None, None),
        };
        let kept = kept.filter(|_| self.cache.coverage.comments.reason().is_none());
        self.cache.write(&Entry {
            schema: ENTRY_SCHEMA,
            remote: self.cache.remote.clone(),
            task: task.clone(),
            comments: comments.or(kept),
            assets,
            edges,
            context: self.inner.write_context(&task.id),
        });
    }

    /// Add `assets` or `edges` to the entry of `id`, when the read that answered them followed
    /// a read of the task from the source at the entry's own version in this command.
    fn complete(&self, id: &NativeId, assets: Option<&[Asset]>, edges: Option<&[DependencyEdge]>) {
        let read = self.cache.command().read.get(id).cloned();
        let Some(read) = read else {
            return;
        };
        let Some(mut entry) = self.entry(id) else {
            return;
        };
        if entry.version() != Some(read.as_str()) {
            return;
        }
        if let Some(assets) = assets {
            entry.assets = Some(assets.to_vec());
        }
        if let Some(edges) = edges {
            entry.edges = Some(edges.to_vec());
        }
        self.cache.write(&entry);
    }

    /// The entry `id` was served from in this command, or one a pin vouches for.
    fn held(&self, id: &NativeId) -> Option<Entry> {
        let served = self.cache.command().served.get(id).cloned();
        served.or_else(|| self.servable(id, false).ok())
    }

    /// Answer `ids` from the cache where a pin vouches for them, and the rest with one read of
    /// the source through `read`, in the order asked.
    async fn details<'a>(
        &'a self,
        ids: &[NativeId],
        page: Option<&PageRequest>,
        edges: bool,
        read: impl FnOnce(Vec<NativeId>) -> Reading<'a>,
    ) -> Vec<Result<Option<TaskDetailRead>, SourceError>> {
        let mut answers: Vec<Option<Result<Option<TaskDetailRead>, SourceError>>> =
            Vec::with_capacity(ids.len());
        let mut unread = Vec::new();
        let mut misses = Vec::new();
        // A later page of comments is never the whole of them, so it is neither served from an
        // entry, which holds them whole, nor kept as if it were.
        let later = page.is_some_and(|page| page.cursor.is_some());
        for id in ids {
            let usable = if later {
                Err(Miss::Behind)
            } else {
                self.servable(id, page.is_some())
            };
            let usable = usable.and_then(|entry| {
                let fits = page.is_none_or(|page| {
                    entry
                        .comments
                        .as_ref()
                        .is_some_and(|comments| comments.len() <= page.limit as usize)
                });
                // A detail read is followed by a read of the task's assets, and a copy's by its
                // edges and by the bytes of every asset its content names: an entry missing what
                // follows would send a request the uncached read carried for nothing, so it is
                // not served.
                let follows = if edges {
                    // A source primed for a write expecting this task answers its edges from what
                    // it was primed with.
                    (self.held_edges(id).is_some() || self.primed_for(id))
                        && self.bytes_kept_for(&entry)
                } else {
                    entry.assets.is_some()
                };
                if fits && follows {
                    Ok(entry)
                } else {
                    Err(Miss::Behind)
                }
            });
            match usable {
                Ok(entry) => {
                    self.served(&entry);
                    let comments = page.map(|_| {
                        Ok(Some(Page {
                            items: entry.comments.clone().unwrap_or_default(),
                            next: None,
                        }))
                    });
                    answers.push(Some(Ok(Some(TaskDetailRead {
                        task: entry.task,
                        comments,
                    }))));
                }
                Err(miss) => {
                    answers.push(None);
                    unread.push(id.clone());
                    misses.push(miss);
                }
            }
        }
        if !unread.is_empty() {
            let asked = unread.clone();
            let mut read = read(asked).await;
            // llmlint: ignore-block[changed_behavior_has_e2e] A source answering another number of details than it was asked for breaks the trait's own contract, and no source this build ships can: it is answered exactly as `Engine::task_details` answers the same miscount — every id of the read refused as malformed, none shifted onto another's slot — which the engine's own `ShortDetails` test proves. No real interface reaches this branch to drive it end to end.
            if read.len() != unread.len() {
                let message = format!(
                    "the source answered {} task details for the {} ids it was asked for",
                    read.len(),
                    unread.len()
                );
                read = unread
                    .iter()
                    .map(|_| {
                        Err(SourceError::Malformed {
                            message: message.clone(),
                        })
                    })
                    .collect();
            }
            // llmlint: ignore-end[changed_behavior_has_e2e]
            for ((id, miss), answer) in unread.iter().zip(&misses).zip(read) {
                match &answer {
                    // Recorded only for the task it is about: an answer naming another task is
                    // the engine's to refuse, and nothing of it reaches this task's entry.
                    Ok(Some(detail)) if detail.task.id == *id => {
                        let comments = match &detail.comments {
                            Some(Ok(Some(page))) if page.next.is_none() && !later => {
                                Some(page.items.clone())
                            }
                            _ => None,
                        };
                        self.missed(id, miss, detail.task.version.as_deref());
                        self.remember(&detail.task, comments);
                    }
                    _ => self.missed(id, miss, None),
                }
                if let Some(slot) = answers.iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some(answer);
                }
            }
        }
        answers.into_iter().flatten().collect()
    }

    /// Whether the cache keeps the verified bytes of every asset `entry`'s content names, under
    /// the digest its manifest gives each — so a copy of it reads none of them from the source.
    fn bytes_kept_for(&self, entry: &Entry) -> bool {
        let named = onetaskgraph_plugin_api::asset_references(
            entry.task.content.as_deref().unwrap_or_default(),
        );
        named.is_empty()
            || entry.assets.as_ref().is_some_and(|assets| {
                named
                    .iter()
                    .all(|name| kept_bytes(&self.cache, Some(assets.clone()), name).is_some())
            })
    }

    /// Whether the source was primed for a write expecting `id`.
    fn primed_for(&self, id: &NativeId) -> bool {
        self.cache
            .command()
            .expected
            .as_ref()
            .is_some_and(|expected| {
                expected.id == *id && expected.primed.as_ref().is_some_and(Priming::primed)
            })
    }

    /// Prime the source with what the cache kept for `id`, once, when a write expecting it
    /// is under way: whether it was primed, and why a read follows when it was not.
    fn prime_expected(&self, id: &NativeId) -> Option<Priming> {
        let version = {
            let command = self.cache.command();
            let expected = command
                .expected
                .as_ref()
                .filter(|expected| expected.id == *id)?;
            if let Some(primed) = &expected.primed {
                return Some(primed.clone());
            }
            expected.version.clone()
        };
        let entry = self.entry(id);
        let decided = match &entry {
            None => Priming::Unprimed {
                reason: "the cache holds no entry for this task, so its target was read".to_owned(),
            },
            Some(entry)
                if !entry
                    .version()
                    .is_some_and(|held| self.fresh(held, &version)) =>
            {
                Priming::Unprimed {
                    reason: "the cache holds this task only at an older version than the one \
                             expected, so its target was read"
                        .to_owned(),
                }
            }
            Some(entry) => match &entry.context {
                Some(context) => {
                    let board = self.cache.board();
                    if self.inner.prime_write(id, context, board.as_ref()) {
                        Priming::Primed {
                            reason: "the cache did not hold all the write needed — the board's \
                                     field ids — so it was read"
                                .to_owned(),
                        }
                    } else {
                        Priming::Unprimed {
                            reason: "the source could not take what the cache kept for this \
                                     task, so its target was read"
                                .to_owned(),
                        }
                    }
                }
                None => Priming::Unprimed {
                    reason: "the cache kept nothing a write of this task could take instead of \
                             reading it, so its target was read"
                        .to_owned(),
                },
            },
        };
        let mut command = self.cache.command();
        if let Some(expected) = command
            .expected
            .as_mut()
            .filter(|expected| expected.id == *id)
        {
            expected.primed = Some(decided.clone());
        }
        Some(decided)
    }

    /// Run one write of task `id` through `write`, its entry taken out of service before
    /// anything is sent — the write refused, sending nothing, when it cannot be: plainly,
    /// leaving that entry unserveable, or —
    /// when the engine expects it — primed with what the entry kept, journaled, and sent once
    /// more after one read if a primed id turned out stale.
    async fn write<'a, T>(
        &'a self,
        id: Option<&NativeId>,
        write: impl Fn() -> Writing<'a, T>,
    ) -> Result<T, SourceError> {
        let expected = id.and_then(|id| {
            let command = self.cache.command();
            command
                .expected
                .as_ref()
                .filter(|expected| expected.id == *id)
                .map(|expected| expected.version.clone())
        });
        let (Some(id), Some(_)) = (id, expected) else {
            if let Some(id) = id {
                self.cache.retire_before_write(id)?;
            }
            let written = write().await;
            if let Some(id) = id {
                self.cache.invalidate(id);
            }
            return written;
        };
        let before = self.inner.metering().await.ok().flatten();
        let priming = self
            .prime_expected(id)
            .unwrap_or_else(|| Priming::Unprimed {
                reason: "the write was not expected when it began, so its target was read"
                    .to_owned(),
            });
        // Retired after priming, which is what took the entry's ids and values into the source.
        // A write refused here was never made, so neither the priming nor the expectation
        // outlives it: a later write of the task through this source is a plain one.
        if let Err(error) = self.cache.retire_before_write(id) {
            self.inner.unprime(id);
            self.cache.take_expected(id);
            return Err(error);
        }
        let primed = priming.primed();
        let mut reason = priming.reason().to_owned();
        self.inner.begin_journal();
        let mut written = write().await;
        let mut journal = self.inner.end_journal();
        if written.is_err() && primed && journal.as_ref().is_some_and(WriteJournal::stale) {
            // One read, then the refused part once more: the read refreshes what the cache
            // kept, so the same stale id cannot be sent again.
            let refused = journal
                .as_ref()
                .and_then(|journal| {
                    journal.mutations().find_map(|entry| match &entry.outcome {
                        PartOutcome::Rejected { error, stale: true } => Some(error.clone()),
                        _ => None,
                    })
                })
                .unwrap_or_default();
            reason = format!(
                "an id the cache kept was stale — the source refused it: {refused} — so the \
                 target was read and the refused part sent once more"
            );
            self.inner.unprime(id);
            self.cache.drop_board();
            self.inner.begin_journal();
            written = write().await;
            if let (Some(first), Some(again)) = (journal.as_mut(), self.inner.end_journal()) {
                first.requests.extend(again.requests);
            }
        }
        if let Some(board) = self.inner.board_context() {
            self.cache.write_board(board);
        }
        let after = self.inner.metering().await.ok().flatten();
        // Settled before the source lets go of what it was primed with: the entry takes the
        // record the write's own response left behind.
        let report = self.settle(id, journal.as_ref(), reason, before, after);
        self.inner.unprime(id);
        self.cache.report(id, report);
        written
    }

    /// Advance the entry of `id` to what `journal`'s last mutation showed whole, or leave it
    /// unserveable; and answer what the write came to.
    fn settle(
        &self,
        id: &NativeId,
        journal: Option<&WriteJournal>,
        reason: String,
        before: Option<Metering>,
        after: Option<Metering>,
    ) -> ExpectedWrite {
        let spent = crate::engine::spent_between(&[before], &[after]);
        let Some(journal) = journal else {
            // A source that records no journal wrote as it always does, and says nothing about
            // what the write left behind.
            self.cache.invalidate(id);
            return ExpectedWrite {
                result: None,
                fallback: None,
                spent,
            };
        };
        let last = journal.mutations().last();
        let landed_last = last.filter(|entry| entry.outcome == PartOutcome::Landed);
        // Taken only when it is this write's own task: a response showing another task says
        // nothing about this one, whose entry is then left unserveable.
        let whole = landed_last
            .and_then(|entry| entry.shown.whole())
            .filter(|whole| whole.task().id == *id);
        match whole {
            Some(whole) => {
                self.cache.write(&Entry {
                    schema: ENTRY_SCHEMA,
                    remote: self.cache.remote.clone(),
                    task: whole.task().clone(),
                    comments: Some(whole.comments().to_vec()),
                    assets: Some(whole.assets().to_vec()),
                    edges: whole.edges().map(<[DependencyEdge]>::to_vec),
                    context: self.inner.write_context(id),
                });
                let mut command = self.cache.command();
                command.served.remove(id);
                command.read.remove(id);
            }
            _ => self.cache.invalidate(id),
        }
        ExpectedWrite {
            result: Some(WriteResult {
                version: landed_last.and_then(|entry| entry.shown.version().map(str::to_owned)),
                parts: Some(
                    journal
                        .mutations()
                        .map(|entry| WritePart {
                            part: entry.part.clone(),
                            outcome: entry.outcome.clone(),
                            fields: entry.fields.clone(),
                        })
                        .collect(),
                ),
                status: None,
                priority: None,
            }),
            fallback: journal
                .read()
                .then_some(WriteFallback { read: true, reason }),
            spent,
        }
    }
}

impl CachedSource {
    /// The version reads of project `id` are pinned to in this command: a caller's pin, or the
    /// version a listing carried.
    fn project_pin(&self, id: &NativeId) -> Option<String> {
        let command = self.cache.command();
        command
            .project_pins
            .get(id)
            .or_else(|| command.project_listed.get(id))
            .cloned()
    }

    /// The version reads of document `id` are pinned to in this command, as
    /// [`project_pin`](Self::project_pin) says for a project.
    fn document_pin(&self, id: &NativeId) -> Option<String> {
        let command = self.cache.command();
        command
            .document_pins
            .get(id)
            .or_else(|| command.document_listed.get(id))
            .cloned()
    }

    /// The project a pinned read of `id` may be answered with: its fields covered, and kept at
    /// the pin or later.
    fn servable_project(&self, id: &NativeId) -> Option<Project> {
        let pin = self.project_pin(id)?;
        self.cache
            .project_coverage
            .fields
            .reason()
            .is_none()
            .then_some(())?;
        let entry = self.cache.read_project(id)?;
        let held = entry.project.version.as_deref()?;
        self.fresh(held, &pin).then_some(entry.project)
    }

    /// The document entry a pinned read of `id` may be answered from: its fields covered, and
    /// kept at the pin or later.
    fn servable_document(&self, id: &NativeId) -> Option<DocumentEntry> {
        let pin = self.document_pin(id)?;
        self.cache
            .document_coverage
            .fields
            .reason()
            .is_none()
            .then_some(())?;
        let entry = self.cache.read_document(id)?;
        let held = entry.document.version.as_deref()?;
        self.fresh(held, &pin).then_some(entry)
    }

    /// The manifest of document `id`'s image assets the cache holds at a covered version: the
    /// one served with the document in this command, the one a pin vouches for, or the one
    /// kept at the version the document was read from the source at in this command.
    fn document_manifest(&self, id: &NativeId) -> Option<Vec<Asset>> {
        self.cache
            .document_coverage
            .assets
            .reason()
            .is_none()
            .then_some(())?;
        let (served, read) = {
            let command = self.cache.command();
            (
                command.document_served.get(id).cloned(),
                command.document_read.get(id).cloned(),
            )
        };
        if let Some(entry) = served.or_else(|| self.servable_document(id)) {
            return entry.assets;
        }
        let entry = self.cache.read_document(id)?;
        (entry.document.version.as_deref() == read.as_deref()).then_some(())?;
        entry.assets
    }

    /// The manifest of task `id`'s image assets the cache holds at a covered version, as
    /// [`document_manifest`](Self::document_manifest) says for a document.
    fn task_manifest(&self, id: &NativeId) -> Option<Vec<Asset>> {
        if let Some(entry) = self.held(id) {
            return entry.assets;
        }
        let read = self.cache.command().read.get(id).cloned()?;
        let entry = self.entry(id)?;
        (entry.version() == Some(read.as_str())).then_some(())?;
        entry.assets
    }

    /// Task `id`'s forward edges, when the source's token covers them and the cache holds them
    /// at a version a pin vouches for: with the task's entry, or in an entry of their own kept
    /// at the version the dependency read that answered them spelled.
    fn held_edges(&self, id: &NativeId) -> Option<Vec<DependencyEdge>> {
        self.cache
            .coverage
            .forward_edges
            .reason()
            .is_none()
            .then_some(())?;
        if let Some(edges) = self.held(id).and_then(|entry| entry.edges) {
            return Some(edges);
        }
        let pin = self.pin(id)?;
        let entry = self.cache.read_edges(EdgeKind::TaskForward, id)?;
        self.fresh(&entry.version, &pin).then_some(entry.edges)
    }

    /// The `kind` item `id`'s reverse edges, when the source's token covers them and the cache
    /// holds them, in an entry of their own, at a version a pin of the item vouches for.
    fn held_reverse_edges(&self, kind: ItemKind, id: &NativeId) -> Option<Vec<DependencyEdge>> {
        self.inner
            .reverse_edges_coverage(kind)
            .reason()
            .is_none()
            .then_some(())?;
        let (pin, edges) = match kind {
            ItemKind::Task => (self.pin(id)?, EdgeKind::TaskReverse),
            ItemKind::Project => (self.project_pin(id)?, EdgeKind::ProjectReverse),
        };
        let entry = self.cache.read_edges(edges, id)?;
        self.fresh(&entry.version, &pin).then_some(entry.edges)
    }

    /// One reverse-edge read of the `kind` item `id`: answered from the cache only from its first
    /// page, which is all an entry of them holds, and kept only when one response answered every
    /// edge.
    async fn read_reverse(
        &self,
        kind: ItemKind,
        id: &NativeId,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        if page.cursor.is_none()
            && let Some(edges) = self
                .held_reverse_edges(kind, id)
                .filter(|edges| edges.len() <= page.limit as usize)
        {
            return Ok(Page {
                items: edges,
                next: None,
            });
        }
        let read = match kind {
            ItemKind::Task => {
                self.inner
                    .task_dependencies(id, Direction::DependedOnBy, page)
                    .await?
            }
            ItemKind::Project => {
                self.inner
                    .project_dependencies(id, Direction::DependedOnBy, page)
                    .await?
            }
        };
        let edges = match kind {
            ItemKind::Task => EdgeKind::TaskReverse,
            ItemKind::Project => EdgeKind::ProjectReverse,
        };
        if page.cursor.is_none()
            && read.next.is_none()
            && self.inner.reverse_edges_coverage(kind).reason().is_none()
            && let Some(version) = self.inner.edges_version(kind, id, Direction::DependedOnBy)
        {
            self.cache.write_edges(edges, id, &version, &read.items);
        }
        Ok(read)
    }

    /// Project `id`'s forward edges, when the source's token covers them and the cache holds
    /// them, in an entry of their own, at a version a pin of the project vouches for.
    fn held_project_edges(&self, id: &NativeId) -> Option<Vec<DependencyEdge>> {
        self.cache
            .project_coverage
            .forward_edges
            .reason()
            .is_none()
            .then_some(())?;
        let pin = self.project_pin(id)?;
        let entry = self.cache.read_edges(EdgeKind::ProjectForward, id)?;
        self.fresh(&entry.version, &pin).then_some(entry.edges)
    }

    /// Run one write that can change a relation recorded on items other than the one it names,
    /// every `kind` edges entry taken out of service before anything is sent — the write
    /// refused, sending nothing, when that cannot be done — and left unserveable after it.
    async fn relating<T>(
        &self,
        kind: EdgeKind,
        write: impl Future<Output = Result<T, SourceError>>,
    ) -> Result<T, SourceError> {
        self.cache.retire_edges_before_write(kind)?;
        let written = write.await;
        self.cache.invalidate_every(kind);
        written
    }

    /// Record one page of a forward-edge walk of task `id` from the source, `so_far` the edges
    /// its earlier pages answered — `None` for the walk's first page: kept for the next page while
    /// the walk goes on, and, once it ends, kept as the task's forward edges — in their own entry
    /// at the version the source's reads spelled, or, when they spelled none and one response
    /// answered the whole walk, with the task's entry at the version it was read at in this
    /// command. A walk of several pages that spelled no version may have read them at several,
    /// so it is kept nowhere.
    ///
    /// A project's walk is kept only in an entry of its own: a project has no task entry to keep
    /// it beside.
    fn walked(
        &self,
        kind: ItemKind,
        id: &NativeId,
        so_far: Option<Vec<DependencyEdge>>,
        read: &Page<DependencyEdge>,
    ) {
        let one_response = so_far.is_none();
        let mut so_far = so_far.unwrap_or_default();
        so_far.extend(read.items.iter().cloned());
        let walk = (kind, id.clone());
        if let Some(next) = &read.next {
            self.cache
                .command()
                .walking
                .insert(walk, (so_far, next.clone()));
            return;
        }
        self.cache.command().walking.remove(&walk);
        let edges = match kind {
            ItemKind::Project => EdgeKind::ProjectForward,
            ItemKind::Task => EdgeKind::TaskForward,
        };
        match self.inner.edges_version(kind, id, Direction::DependsOn) {
            Some(version) => self.cache.write_edges(edges, id, &version, &so_far),
            None if one_response && kind == ItemKind::Task => {
                self.complete(id, None, Some(&so_far));
            }
            None => {}
        }
    }

    /// One forward-edge read of the `kind` item `id` from the source, continuing — and, once it
    /// ends, keeping — the walk its first page began in this command, when it resumes exactly
    /// where that page left off.
    async fn walk_forward(
        &self,
        kind: ItemKind,
        id: &NativeId,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        let read = |page| async move {
            match kind {
                ItemKind::Task => {
                    self.inner
                        .task_dependencies(id, Direction::DependsOn, page)
                        .await
                }
                ItemKind::Project => {
                    self.inner
                        .project_dependencies(id, Direction::DependsOn, page)
                        .await
                }
            }
        };
        let Some(cursor) = &page.cursor else {
            let answered = read(page).await?;
            self.walked(kind, id, None, &answered);
            return Ok(answered);
        };
        let so_far = {
            let mut command = self.cache.command();
            match command.walking.remove(&(kind, id.clone())) {
                Some((so_far, next)) if next == *cursor => Some(so_far),
                _ => None,
            }
        };
        let answered = read(page).await?;
        if let Some(so_far) = so_far {
            self.walked(kind, id, Some(so_far), &answered);
        }
        Ok(answered)
    }

    /// Whether this cache answers a copy's read of `id` with no request: a task or a project
    /// only when it holds its forward edges too, which a copy reads of it next — where the token
    /// does not cover them, or the entry holds none, its edge reads would each be a request of
    /// their own, and reading it ahead carries them in the one request instead.
    fn answers_ahead(&self, id: &NativeId) -> bool {
        let task = self.servable(id, false).is_ok() && self.held_edges(id).is_some();
        let project = self.servable_project(id).is_some()
            && self.cache.project_coverage.forward_edges.reason().is_none()
            && self.held_project_edges(id).is_some();
        task || project || self.servable_document(id).is_some()
    }

    /// A batched write of `items`, each answered on its own: every item's entries — those of
    /// the task `id_of` names — retired before it is sent, and invalidated after, whatever it
    /// came to. An item whose entries cannot be retired is answered with that refusal and not
    /// sent; `write` sends the rest together.
    async fn retiring_each<I: Clone, T, F>(
        &self,
        items: &[I],
        id_of: impl Fn(&I) -> &NativeId,
        write: impl FnOnce(Vec<I>) -> F,
    ) -> Vec<Result<T, SourceError>>
    where
        F: Future<Output = Vec<Result<T, SourceError>>>,
    {
        let mut answers: Vec<Option<Result<T, SourceError>>> = items.iter().map(|_| None).collect();
        let mut sent: Vec<usize> = Vec::with_capacity(items.len());
        for (at, item) in items.iter().enumerate() {
            match self.cache.retire_before_write(id_of(item)) {
                Ok(()) => sent.push(at),
                Err(error) => answers[at] = Some(Err(error)),
            }
        }
        if sent.len() == items.len() {
            // Every item sent: the answers go back as the source gave them, for the engine to
            // hold to one per item.
            let written = write(items.to_vec()).await;
            for item in items {
                self.cache.invalidate(id_of(item));
            }
            return written;
        }
        if !sent.is_empty() {
            let written = write(sent.iter().map(|at| items[*at].clone()).collect()).await;
            // Spliced back by position, so held to one answer per item sent first — the
            // engine's own rule for a batched answer.
            let written = crate::engine::batch::one_each(&self.inner.kind(), sent.len(), written);
            for (at, answer) in sent.iter().zip(written) {
                answers[*at] = Some(answer);
                self.cache.invalidate(id_of(&items[*at]));
            }
        }
        answers.into_iter().flatten().collect()
    }

    /// Run one write naming `id`, every entry naming it taken out of service before anything is
    /// sent — the write refused, sending nothing, when that cannot be done — and left
    /// unserveable after it.
    async fn retiring<T>(
        &self,
        id: Option<&NativeId>,
        write: impl Future<Output = Result<T, SourceError>>,
    ) -> Result<T, SourceError> {
        if let Some(id) = id {
            self.cache.retire_before_write(id)?;
        }
        let written = write.await;
        if let Some(id) = id {
            self.cache.invalidate(id);
        }
        written
    }

    /// Keep `bytes` an asset read from the source answered, under their own digest, when the
    /// source's token covers the manifest that will name them.
    fn keep_bytes(&self, covered: bool, bytes: Option<&Vec<u8>>) {
        if let Some(bytes) = bytes.filter(|_| covered) {
            self.cache.write_bytes(bytes);
        }
    }
}

/// The bytes of the asset called `name` in `manifest`, when the cache keeps them whole.
fn kept_bytes(
    cache: &SourceCache,
    manifest: Option<Vec<Asset>>,
    name: &AssetName,
) -> Option<Vec<u8>> {
    let manifest = manifest?;
    let asset = manifest.iter().find(|asset| asset.name == *name)?;
    cache.read_bytes(&ByteDigest::parse(&asset.sha256)?)
}

#[async_trait::async_trait]
impl TaskSource for CachedSource {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn order_versions(&self, a: &str, b: &str) -> Option<Ordering> {
        self.inner.order_versions(a, b)
    }

    fn holds_versions(&self) -> bool {
        self.inner.holds_versions()
    }

    fn comment_author_kind(
        &self,
        comment: &onetaskgraph_plugin_api::Comment,
    ) -> onetaskgraph_plugin_api::AuthorKind {
        self.inner.comment_author_kind(comment)
    }

    fn comment_consistency_window(&self) -> std::time::Duration {
        self.inner.comment_consistency_window()
    }

    fn served_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.inner.served_at()
    }

    fn version_coverage(&self) -> VersionCoverage {
        self.inner.version_coverage()
    }
    /// The source's own: a copy's skip asks what the plugin's token vouches for, not what the
    /// cache may serve.
    fn copy_version_coverage(&self) -> VersionCoverage {
        self.inner.copy_version_coverage()
    }

    fn project_version_coverage(&self) -> ProjectVersionCoverage {
        self.inner.project_version_coverage()
    }

    fn document_version_coverage(&self) -> DocumentVersionCoverage {
        self.inner.document_version_coverage()
    }

    /// Nothing: [`CachedSource::new`] told the source, once, as it wrapped it.
    fn attach_cache(&self) {}

    fn forward_edges_version(&self, id: &NativeId) -> Option<String> {
        self.inner.forward_edges_version(id)
    }

    fn edges_version(&self, kind: ItemKind, id: &NativeId, direction: Direction) -> Option<String> {
        self.inner.edges_version(kind, id, direction)
    }

    fn reverse_edges_coverage(&self, kind: ItemKind) -> onetaskgraph_plugin_api::Coverage {
        self.inner.reverse_edges_coverage(kind)
    }

    fn write_context(&self, id: &NativeId) -> Option<Value> {
        self.inner.write_context(id)
    }

    fn board_context(&self) -> Option<Value> {
        self.inner.board_context()
    }

    fn prime_write(&self, id: &NativeId, context: &Value, board: Option<&Value>) -> bool {
        self.inner.prime_write(id, context, board)
    }

    fn placement_context(&self) -> Option<Value> {
        self.inner.placement_context()
    }

    fn prime_placement(&self, context: &Value) -> bool {
        self.inner.prime_placement(context)
    }

    fn written_context(&self, id: &NativeId) -> Option<Value> {
        self.inner.written_context(id)
    }

    fn prime_written(&self, id: &NativeId, context: &Value, task: &Task) -> bool {
        self.inner.prime_written(id, context, task)
    }

    fn unprime(&self, id: &NativeId) {
        self.inner.unprime(id);
    }

    fn begin_journal(&self) {
        self.inner.begin_journal();
    }

    fn end_journal(&self) -> Option<WriteJournal> {
        self.inner.end_journal()
    }

    async fn health(&self) -> Result<Health, SourceError> {
        self.inner.health().await
    }

    async fn designator(&self) -> Result<Option<onetaskgraph_plugin_api::Designator>, SourceError> {
        self.inner.designator().await
    }

    async fn get_task(&self, id: &NativeId) -> Result<Option<Task>, SourceError> {
        self.prime_expected(id);
        match self.servable(id, false) {
            Ok(entry) => {
                self.served(&entry);
                Ok(Some(entry.task))
            }
            Err(miss) => {
                let read = self.inner.get_task(id).await;
                match &read {
                    // Recorded only for the task it is about, as a batched detail read is.
                    Ok(Some(task)) if task.id == *id => {
                        self.missed(id, &miss, task.version.as_deref());
                        self.remember(task, None);
                    }
                    _ => self.missed(id, &miss, None),
                }
                read
            }
        }
    }

    async fn get_task_details(
        &self,
        ids: &[NativeId],
        comments: Option<&PageRequest>,
    ) -> Vec<Result<Option<TaskDetailRead>, SourceError>> {
        for id in ids {
            self.prime_expected(id);
        }
        self.details(ids, comments, false, |unread| {
            Box::pin(async move {
                // One task with its comments is read the way a copy reads it — the same one
                // request — so what the source keeps of it carries its `blockedBy` too, and a
                // write expecting it later reads none of its edges either.
                match (unread.as_slice(), comments) {
                    ([_], Some(page)) => self.inner.get_task_details_for_copy(&unread, page).await,
                    _ => self.inner.get_task_details(&unread, comments).await,
                }
            })
        })
        .await
    }

    async fn get_task_details_for_copy(
        &self,
        ids: &[NativeId],
        comments: &PageRequest,
    ) -> Vec<Result<Option<TaskDetailRead>, SourceError>> {
        for id in ids {
            self.prime_expected(id);
        }
        self.details(ids, Some(comments), true, |unread| {
            Box::pin(async move {
                self.inner
                    .get_task_details_for_copy(&unread, comments)
                    .await
            })
        })
        .await
    }

    /// Only what the cache cannot answer is read ahead: an id whose task or document entry a
    /// pin in this command vouches for is served by its own read with no request, so reading
    /// it again ahead of that would spend a request on an answer already held. A task or a
    /// project is served only when the cache holds its forward edges too, which a copy reads of
    /// it next: where the token does not cover them, or the entry holds none, its edge reads
    /// would each be a request of their own, and reading it ahead carries them in the one
    /// request instead.
    async fn read_ahead_records_for_copy(
        &self,
        records: &[(MetadataRecord, NativeId)],
    ) -> Result<(), SourceError> {
        let unserved: Vec<(MetadataRecord, NativeId)> = records
            .iter()
            .filter(|(_, id)| !self.answers_ahead(id))
            .cloned()
            .collect();
        // Handed on even when the cache answers every id, as `read_ahead_for_copy` hands it on.
        self.inner.read_ahead_records_for_copy(&unserved).await
    }

    async fn read_ahead_for_copy(&self, ids: &[NativeId]) -> Result<(), SourceError> {
        let unserved: Vec<NativeId> = ids
            .iter()
            .filter(|id| !self.answers_ahead(id))
            .cloned()
            .collect();
        // Handed on even when the cache answers every id: what the source holds back for the
        // read ahead — the origins asked just before — is sent by this call.
        self.inner.read_ahead_for_copy(&unserved).await
    }

    async fn read_ahead_origins_for_copy(&self, origins: &[String]) -> Result<(), SourceError> {
        self.inner.read_ahead_origins_for_copy(origins).await
    }

    async fn member_classifications(
        &self,
        project: &NativeId,
    ) -> Result<
        Option<
            Vec<(
                onetaskgraph_plugin_api::Classification,
                Vec<onetaskgraph_plugin_api::Repository>,
            )>,
        >,
        SourceError,
    > {
        self.inner.member_classifications(project).await
    }

    fn records_created_together(&self) -> bool {
        self.inner.records_created_together()
    }

    fn create_batch_limit(&self) -> usize {
        self.inner.create_batch_limit()
    }

    async fn write_tasks(&self, writes: &[ItemWrite<Task>]) -> Result<Vec<NativeId>, SourceError> {
        // Every one a create, so nothing of this cache names an item they land on; what they can
        // change of it is the reverse edges of the tasks they name, as one create's can.
        self.relating(EdgeKind::TaskReverse, self.inner.write_tasks(writes))
            .await
    }

    fn begin_deferred_writes(&self) {
        self.inner.begin_deferred_writes();
    }

    async fn finish_writes(&self) -> Result<(), SourceError> {
        // What was held back includes the edges of the tasks it created, which move the reverse
        // edges of the tasks they name, exactly as a write landing them whole would.
        self.relating(EdgeKind::TaskReverse, self.inner.finish_writes())
            .await
    }

    async fn created_with_key(
        &self,
        record: MetadataRecord,
        key: &str,
        since: Option<chrono::DateTime<chrono::Utc>>,
        target: &WriteTarget<'_>,
    ) -> Result<Vec<NativeId>, SourceError> {
        self.inner
            .created_with_key(record, key, since, target)
            .await
    }

    fn create_key_lags(&self) -> bool {
        self.inner.create_key_lags()
    }

    fn hold_item(&self, record: MetadataRecord, id: &NativeId) -> Result<bool, SourceError> {
        self.inner.hold_item(record, id)
    }

    fn release_item(&self, record: MetadataRecord, id: &NativeId) {
        self.inner.release_item(record, id);
    }

    fn item_snapshot(&self, record: MetadataRecord, id: &NativeId) -> Option<Vec<u8>> {
        self.inner.item_snapshot(record, id)
    }

    fn restore_item(
        &self,
        record: MetadataRecord,
        id: &NativeId,
        snapshot: &[u8],
    ) -> Result<(), SourceError> {
        self.inner.restore_item(record, id, snapshot)
    }

    async fn written_task(&self, id: &NativeId) -> Option<Task> {
        self.inner.written_task(id).await
    }

    async fn written_version(&self, record: MetadataRecord, id: &NativeId) -> Option<String> {
        self.inner.written_version(record, id).await
    }

    fn written_location(&self, record: MetadataRecord, id: &NativeId) -> Option<Location> {
        self.inner.written_location(record, id)
    }

    async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
        if let Some(project) = self.servable_project(id) {
            return Ok(Some(project));
        }
        let read = self.inner.get_project(id).await;
        // Kept only for the project it is about, and only where the token covers its fields.
        if let Ok(Some(project)) = &read
            && project.id == *id
            && self.cache.project_coverage.fields.reason().is_none()
        {
            self.cache.write_project(project);
        }
        read
    }

    async fn query_tasks(
        &self,
        query: &TaskQuery,
        page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        let listed = self.inner.query_tasks(query, page).await?;
        let mut command = self.cache.command();
        for task in &listed.items {
            if let Some(version) = &task.version {
                command.listed.insert(task.id.clone(), version.clone());
            }
        }
        drop(command);
        Ok(listed)
    }

    async fn query_tasks_batch(
        &self,
        asks: &[(TaskQuery, PageRequest)],
    ) -> Vec<Result<Page<Task>, SourceError>> {
        let answers = self.inner.query_tasks_batch(asks).await;
        let mut command = self.cache.command();
        for task in answers.iter().flatten().flat_map(|page| &page.items) {
            if let Some(version) = &task.version {
                command.listed.insert(task.id.clone(), version.clone());
            }
        }
        drop(command);
        answers
    }

    async fn query_projects(
        &self,
        query: &ProjectQuery,
        page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        let listed = self.inner.query_projects(query, page).await?;
        let mut command = self.cache.command();
        for project in &listed.items {
            if let Some(version) = &project.version {
                command
                    .project_listed
                    .insert(project.id.clone(), version.clone());
            }
        }
        drop(command);
        Ok(listed)
    }

    async fn query_projects_by_origin(
        &self,
        origin: &str,
        page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        let listed = self.inner.query_projects_by_origin(origin, page).await?;
        let mut command = self.cache.command();
        for project in &listed.items {
            if let Some(version) = &project.version {
                command
                    .project_listed
                    .insert(project.id.clone(), version.clone());
            }
        }
        drop(command);
        Ok(listed)
    }

    async fn labels(&self, page: &PageRequest) -> Result<Page<Label>, SourceError> {
        self.inner.labels(page).await
    }

    async fn task_dependencies(
        &self,
        id: &NativeId,
        direction: Direction,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.prime_expected(id);
        if direction == Direction::DependedOnBy {
            return self.read_reverse(ItemKind::Task, id, page).await;
        }
        // Every read of a source whose token does not cover a task's forward edges goes to the
        // source — which answers a write expecting the task from what it was primed with.
        if self.cache.coverage.forward_edges.reason().is_some() {
            return self.inner.task_dependencies(id, direction, page).await;
        }
        if page.cursor.is_none()
            && let Some(edges) = self
                .held_edges(id)
                .filter(|edges| edges.len() <= page.limit as usize)
        {
            return Ok(Page {
                items: edges,
                next: None,
            });
        }
        self.walk_forward(ItemKind::Task, id, page).await
    }

    async fn project_dependencies(
        &self,
        id: &NativeId,
        direction: Direction,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        if direction == Direction::DependedOnBy {
            return self.read_reverse(ItemKind::Project, id, page).await;
        }
        // Every forward read of a source whose token does not cover a project's goes to the
        // source.
        if self.cache.project_coverage.forward_edges.reason().is_some() {
            return self.inner.project_dependencies(id, direction, page).await;
        }
        if page.cursor.is_none()
            && let Some(edges) = self
                .held_project_edges(id)
                .filter(|edges| edges.len() <= page.limit as usize)
        {
            return Ok(Page {
                items: edges,
                next: None,
            });
        }
        self.walk_forward(ItemKind::Project, id, page).await
    }

    fn writes(&self) -> WriteSupport {
        self.inner.writes()
    }

    async fn stored_status(&self, kind: ItemKind, status: &Status) -> Result<Status, SourceError> {
        self.inner.stored_status(kind, status).await
    }

    fn stored_content(&self, record: MetadataRecord, id: &NativeId) -> Option<String> {
        self.inner.stored_content(record, id)
    }

    fn stored_headings(
        &self,
        record: MetadataRecord,
        id: &NativeId,
    ) -> Option<Vec<onetaskgraph_plugin_api::StoredHeading>> {
        self.inner.stored_headings(record, id)
    }

    fn attach_body_markup(&self, markup: onetaskgraph_plugin_api::SharedBodyMarkup) {
        self.inner.attach_body_markup(markup);
    }

    async fn write_task(&self, write: &ItemWrite<Task>) -> Result<NativeId, SourceError> {
        self.relating(
            EdgeKind::TaskReverse,
            self.write(write.target.as_ref(), || {
                Box::pin(self.inner.write_task(write))
            }),
        )
        .await
    }

    async fn write_project(&self, write: &ItemWrite<Project>) -> Result<NativeId, SourceError> {
        self.relating(
            EdgeKind::ProjectReverse,
            self.retiring(write.target.as_ref(), self.inner.write_project(write)),
        )
        .await
    }

    async fn check_status_write(
        &self,
        kind: ItemKind,
        category: StatusCategory,
        target: Option<&NativeId>,
    ) -> Result<(), SourceError> {
        if let Some(target) = target {
            self.prime_expected(target);
        }
        self.inner.check_status_write(kind, category, target).await
    }

    fn status_as_written(&self, kind: ItemKind, status: &Status) -> Status {
        self.inner.status_as_written(kind, status)
    }

    async fn set_task_status(
        &self,
        id: &NativeId,
        category: StatusCategory,
    ) -> Result<Option<Status>, SourceError> {
        self.write(Some(id), || {
            Box::pin(self.inner.set_task_status(id, category))
        })
        .await
    }

    // Both reads go straight to the source and are kept nowhere: each asks a question no
    // pinned version answers — which items are out of line now, which items these issues are —
    // and the status writes that follow go through `set_task_status` above, which retires
    // what they change.
    async fn status_contradictions(&self) -> Result<Vec<StatusContradiction>, SourceError> {
        self.inner.status_contradictions().await
    }

    async fn check_task_write(&self, write: &ItemWrite<Task>) -> Result<(), SourceError> {
        self.inner.check_task_write(write).await
    }

    // Answered without a request, so nothing is held off or cached for it.
    fn task_body_size(
        &self,
        write: &ItemWrite<Task>,
        assets: Option<&AssetWrite>,
    ) -> Option<onetaskgraph_plugin_api::BodySize> {
        self.inner.task_body_size(write, assets)
    }

    fn project_body_size(
        &self,
        write: &ItemWrite<Project>,
    ) -> Option<onetaskgraph_plugin_api::BodySize> {
        self.inner.project_body_size(write)
    }

    fn document_body_size(
        &self,
        write: &ItemWrite<Document>,
        assets: Option<&AssetWrite>,
    ) -> Option<onetaskgraph_plugin_api::BodySize> {
        self.inner.document_body_size(write, assets)
    }

    async fn issue_holdings(&self, issues: &[IssueRef]) -> Result<Vec<IssueHolding>, SourceError> {
        self.inner.issue_holdings(issues).await
    }

    async fn set_task_status_reading(
        &self,
        id: &NativeId,
        category: StatusCategory,
    ) -> Result<Option<Task>, SourceError> {
        // An expected write reads the task it answers with from the cache, at the version
        // expected, rather than letting the source read it first: the two calls a source's own
        // answer is defined as, each through this cache.
        let expected = self
            .cache
            .command()
            .expected
            .as_ref()
            .is_some_and(|expected| expected.id == *id);
        if expected && self.servable(id, false).is_ok() {
            let Some(mut task) = self.get_task(id).await? else {
                return Ok(None);
            };
            let Some(status) = self.set_task_status(id, category).await? else {
                return Ok(None);
            };
            task.status = status;
            return Ok(Some(task));
        }
        self.write(Some(id), || {
            Box::pin(self.inner.set_task_status_reading(id, category))
        })
        .await
    }

    async fn set_task_priority(
        &self,
        id: &NativeId,
        priority: Priority,
    ) -> Result<Option<Priority>, SourceError> {
        self.write(Some(id), || {
            Box::pin(self.inner.set_task_priority(id, priority))
        })
        .await
    }

    async fn set_task_content(
        &self,
        id: &NativeId,
        content: &str,
    ) -> Result<Option<()>, SourceError> {
        self.write(Some(id), || {
            Box::pin(self.inner.set_task_content(id, content))
        })
        .await
    }

    async fn set_delivered_by(
        &self,
        id: &NativeId,
        delivered_by: &[TaskRef],
    ) -> Result<Option<()>, SourceError> {
        self.write(Some(id), || {
            Box::pin(self.inner.set_delivered_by(id, delivered_by))
        })
        .await
    }

    async fn set_task_metadata(
        &self,
        id: &NativeId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<Option<Task>, SourceError> {
        self.write(Some(id), || {
            Box::pin(self.inner.set_task_metadata(id, key, value))
        })
        .await
    }

    async fn update_task(
        &self,
        id: &NativeId,
        update: &TaskUpdate,
    ) -> Result<Option<TaskUpdateOutcome>, SourceError> {
        let write = self.write(Some(id), || Box::pin(self.inner.update_task(id, update)));
        // Only an update naming dependencies writes a relation, which the far end records too.
        if update.depends_on.is_some() {
            self.relating(EdgeKind::TaskReverse, write).await
        } else {
            write.await
        }
    }

    async fn set_project_status(
        &self,
        id: &NativeId,
        category: StatusCategory,
    ) -> Result<Option<Status>, SourceError> {
        self.retiring(Some(id), self.inner.set_project_status(id, category))
            .await
    }

    async fn set_project_metadata(
        &self,
        id: &NativeId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<Option<Project>, SourceError> {
        self.retiring(Some(id), self.inner.set_project_metadata(id, key, value))
            .await
    }

    async fn set_document_metadata(
        &self,
        id: &NativeId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<Option<Document>, SourceError> {
        self.retiring(Some(id), self.inner.set_document_metadata(id, key, value))
            .await
    }

    async fn remove_record_metadata(
        &self,
        record: MetadataRecord,
        id: &NativeId,
        key: &MetadataKey,
    ) -> Result<Option<Option<String>>, SourceError> {
        self.retiring(Some(id), self.inner.remove_record_metadata(record, id, key))
            .await
    }

    async fn delete_task(&self, id: &NativeId) -> Result<(), SourceError> {
        self.relating(
            EdgeKind::TaskReverse,
            self.write(Some(id), || Box::pin(self.inner.delete_task(id))),
        )
        .await
    }

    async fn delete_project(&self, id: &NativeId) -> Result<(), SourceError> {
        self.relating(
            EdgeKind::ProjectReverse,
            self.relating(
                EdgeKind::ProjectForward,
                self.retiring(Some(id), self.inner.delete_project(id)),
            ),
        )
        .await
    }

    async fn get_document(&self, id: &NativeId) -> Result<Option<Document>, SourceError> {
        if let Some(entry) = self.servable_document(id) {
            let document = entry.document.clone();
            let mut command = self.cache.command();
            command.document_read.remove(id);
            command.document_served.insert(id.clone(), entry);
            return Ok(Some(document));
        }
        let read = self.inner.get_document(id).await;
        let mut kept = None;
        // Kept only for the document it is about, and only where the token covers its fields —
        // with the manifest an entry at that very version already held.
        if let Ok(Some(document)) = &read
            && document.id == *id
            && self.cache.document_coverage.fields.reason().is_none()
            && let Some(version) = document.version.clone()
        {
            let assets = self
                .cache
                .read_document(id)
                .filter(|held| held.document.version.as_deref() == Some(version.as_str()))
                .and_then(|held| held.assets);
            self.cache.write_document(&DocumentEntry {
                schema: KIND_ENTRY_SCHEMA,
                kind: super::kinds::DocumentTag::Document,
                remote: self.cache.remote.clone(),
                document: document.clone(),
                assets,
            });
            kept = Some(version);
        }
        let mut command = self.cache.command();
        command.document_served.remove(id);
        match kept {
            Some(version) => command.document_read.insert(id.clone(), version),
            None => command.document_read.remove(id),
        };
        drop(command);
        read
    }

    async fn query_documents(
        &self,
        query: &DocumentQuery,
        page: &PageRequest,
    ) -> Result<Page<Document>, SourceError> {
        let listed = self.inner.query_documents(query, page).await?;
        let mut command = self.cache.command();
        for document in &listed.items {
            if let Some(version) = &document.version {
                command
                    .document_listed
                    .insert(document.id.clone(), version.clone());
            }
        }
        drop(command);
        Ok(listed)
    }

    async fn query_documents_by_origin(
        &self,
        origin: &str,
        page: &PageRequest,
    ) -> Result<Page<Document>, SourceError> {
        let listed = self.inner.query_documents_by_origin(origin, page).await?;
        let mut command = self.cache.command();
        for document in &listed.items {
            if let Some(version) = &document.version {
                command
                    .document_listed
                    .insert(document.id.clone(), version.clone());
            }
        }
        drop(command);
        Ok(listed)
    }

    async fn write_document(&self, write: &ItemWrite<Document>) -> Result<NativeId, SourceError> {
        self.retiring(write.target.as_ref(), self.inner.write_document(write))
            .await
    }

    async fn delete_document(&self, id: &NativeId) -> Result<(), SourceError> {
        self.retiring(Some(id), self.inner.delete_document(id))
            .await
    }

    async fn task_comments(
        &self,
        task: &NativeId,
        page: &PageRequest,
    ) -> Result<Option<Page<Comment>>, SourceError> {
        if page.cursor.is_none()
            && let Ok(entry) = self.servable(task, true)
            && let Some(comments) = entry
                .comments
                .clone()
                .filter(|comments| comments.len() <= page.limit as usize)
        {
            self.served(&entry);
            return Ok(Some(Page {
                items: comments,
                next: None,
            }));
        }
        self.inner.task_comments(task, page).await
    }

    async fn add_comment(
        &self,
        task: &NativeId,
        comment: &NewComment,
    ) -> Result<Option<Comment>, SourceError> {
        self.write(Some(task), || {
            Box::pin(self.inner.add_comment(task, comment))
        })
        .await
    }

    async fn add_comments(
        &self,
        comments: &[onetaskgraph_plugin_api::BatchComment],
    ) -> Vec<Result<onetaskgraph_plugin_api::BatchAnswer<Comment>, SourceError>> {
        self.retiring_each(
            comments,
            |each| &each.task,
            |sent| async move { self.inner.add_comments(&sent).await },
        )
        .await
    }

    async fn update_tasks(
        &self,
        updates: &[onetaskgraph_plugin_api::BatchUpdate],
    ) -> Vec<Result<onetaskgraph_plugin_api::BatchAnswer<TaskUpdateOutcome>, SourceError>> {
        self.retiring_each(
            updates,
            |each| &each.id,
            |sent| async move { self.inner.update_tasks(&sent).await },
        )
        .await
    }

    /// Never served from the cache: a relation read reaches the source every time, so no
    /// pinned token can be answered with relations older than it.
    async fn read_relations(
        &self,
        ids: &[NativeId],
    ) -> Vec<Result<Option<onetaskgraph_plugin_api::Relations>, SourceError>> {
        self.inner.read_relations(ids).await
    }

    async fn set_parents(
        &self,
        writes: &[onetaskgraph_plugin_api::ParentWrite],
    ) -> Vec<Result<onetaskgraph_plugin_api::ParentAnswer, SourceError>> {
        self.retiring_each(
            writes,
            |each| &each.child,
            |sent| async move { self.inner.set_parents(&sent).await },
        )
        .await
    }

    fn link_types(&self) -> onetaskgraph_plugin_api::LinkTypes {
        self.inner.link_types()
    }

    async fn add_native_links(
        &self,
        links: &[onetaskgraph_plugin_api::NativeLink],
    ) -> Vec<Result<(), SourceError>> {
        self.retiring_each(
            links,
            |each| &each.from,
            |sent| async move { self.inner.add_native_links(&sent).await },
        )
        .await
    }

    async fn remove_native_links(
        &self,
        links: &[onetaskgraph_plugin_api::NativeLink],
    ) -> Vec<Result<(), SourceError>> {
        self.retiring_each(
            links,
            |each| &each.from,
            |sent| async move { self.inner.remove_native_links(&sent).await },
        )
        .await
    }

    async fn edit_comment(
        &self,
        task: &NativeId,
        comment: &NativeId,
        body: &CommentBody,
    ) -> Result<Option<Comment>, SourceError> {
        self.write(Some(task), || {
            Box::pin(self.inner.edit_comment(task, comment, body))
        })
        .await
    }

    async fn delete_comment(
        &self,
        task: &NativeId,
        comment: &NativeId,
    ) -> Result<Option<NativeId>, SourceError> {
        self.write(Some(task), || {
            Box::pin(self.inner.delete_comment(task, comment))
        })
        .await
    }

    async fn write_comment_copy(
        &self,
        task: &NativeId,
        existing: Option<&NativeId>,
        copy: &CommentCopy,
    ) -> Result<Option<Comment>, SourceError> {
        self.write(Some(task), || {
            Box::pin(self.inner.write_comment_copy(task, existing, copy))
        })
        .await
    }

    fn keeps_template_answers(&self) -> bool {
        self.inner.keeps_template_answers()
    }

    async fn task_template_answers(
        &self,
        id: &NativeId,
    ) -> Result<Option<BTreeMap<String, Value>>, SourceError> {
        self.inner.task_template_answers(id).await
    }

    async fn document_template_answers(
        &self,
        id: &NativeId,
    ) -> Result<Option<BTreeMap<String, Value>>, SourceError> {
        self.inner.document_template_answers(id).await
    }

    async fn project_template_answers(
        &self,
        id: &NativeId,
    ) -> Result<Option<BTreeMap<String, Value>>, SourceError> {
        self.inner.project_template_answers(id).await
    }

    async fn write_task_rendered(
        &self,
        write: &ItemWrite<Task>,
        answers: &BTreeMap<String, Value>,
    ) -> Result<NativeId, SourceError> {
        self.relating(
            EdgeKind::TaskReverse,
            self.write(write.target.as_ref(), || {
                Box::pin(self.inner.write_task_rendered(write, answers))
            }),
        )
        .await
    }

    async fn write_document_rendered(
        &self,
        write: &ItemWrite<Document>,
        answers: &BTreeMap<String, Value>,
    ) -> Result<NativeId, SourceError> {
        self.retiring(
            write.target.as_ref(),
            self.inner.write_document_rendered(write, answers),
        )
        .await
    }

    async fn write_project_rendered(
        &self,
        write: &ItemWrite<Project>,
        answers: &BTreeMap<String, Value>,
    ) -> Result<NativeId, SourceError> {
        self.relating(
            EdgeKind::ProjectReverse,
            self.retiring(
                write.target.as_ref(),
                self.inner.write_project_rendered(write, answers),
            ),
        )
        .await
    }

    async fn set_task_rendering(
        &self,
        id: &NativeId,
        content: &str,
        provenance: &Value,
        answers: &BTreeMap<String, Value>,
    ) -> Result<Option<()>, SourceError> {
        self.write(Some(id), || {
            Box::pin(
                self.inner
                    .set_task_rendering(id, content, provenance, answers),
            )
        })
        .await
    }

    async fn set_document_rendering(
        &self,
        id: &NativeId,
        content: &str,
        provenance: &Value,
        answers: &BTreeMap<String, Value>,
    ) -> Result<Option<()>, SourceError> {
        self.retiring(
            Some(id),
            self.inner
                .set_document_rendering(id, content, provenance, answers),
        )
        .await
    }

    async fn set_project_rendering(
        &self,
        id: &NativeId,
        content: &str,
        provenance: &Value,
        answers: &BTreeMap<String, Value>,
    ) -> Result<Option<()>, SourceError> {
        self.retiring(
            Some(id),
            self.inner
                .set_project_rendering(id, content, provenance, answers),
        )
        .await
    }

    async fn task_assets(&self, id: &NativeId) -> Result<Vec<Asset>, SourceError> {
        self.prime_expected(id);
        if let Some(assets) = self.held(id).and_then(|entry| entry.assets) {
            return Ok(assets);
        }
        let read = self.inner.task_assets(id).await?;
        self.complete(id, Some(&read), None);
        Ok(read)
    }

    async fn document_assets(&self, id: &NativeId) -> Result<Vec<Asset>, SourceError> {
        if self.cache.document_coverage.assets.reason().is_none() {
            let served = self.cache.command().document_served.get(id).cloned();
            if let Some(assets) = served
                .or_else(|| self.servable_document(id))
                .and_then(|entry| entry.assets)
            {
                return Ok(assets);
            }
        }
        let read = self.inner.document_assets(id).await?;
        // Kept with the document only when it was read from the source at the entry's own
        // version in this command, and only where the token covers the manifest.
        let at = self.cache.command().document_read.get(id).cloned();
        if self.cache.document_coverage.assets.reason().is_none()
            && let Some(at) = at
            && let Some(mut entry) = self.cache.read_document(id)
            && entry.document.version.as_deref() == Some(at.as_str())
        {
            entry.assets = Some(read.clone());
            self.cache.write_document(&entry);
        }
        Ok(read)
    }

    async fn task_asset(
        &self,
        id: &NativeId,
        name: &AssetName,
    ) -> Result<Option<Vec<u8>>, SourceError> {
        let covered = self.cache.coverage.fields.reason().is_none();
        if covered && let Some(bytes) = kept_bytes(&self.cache, self.task_manifest(id), name) {
            return Ok(Some(bytes));
        }
        let read = self.inner.task_asset(id, name).await?;
        self.keep_bytes(covered, read.as_ref());
        Ok(read)
    }

    async fn document_asset(
        &self,
        id: &NativeId,
        name: &AssetName,
    ) -> Result<Option<Vec<u8>>, SourceError> {
        let covered = self.cache.document_coverage.assets.reason().is_none();
        if let Some(bytes) = kept_bytes(&self.cache, self.document_manifest(id), name) {
            return Ok(Some(bytes));
        }
        let read = self.inner.document_asset(id, name).await?;
        self.keep_bytes(covered, read.as_ref());
        Ok(read)
    }

    async fn write_task_with_assets(
        &self,
        write: &ItemWrite<Task>,
        answers: Option<&BTreeMap<String, Value>>,
        assets: &AssetWrite,
    ) -> Result<AssetsWritten, SourceError> {
        self.relating(
            EdgeKind::TaskReverse,
            self.write(write.target.as_ref(), || {
                Box::pin(self.inner.write_task_with_assets(write, answers, assets))
            }),
        )
        .await
    }

    async fn write_document_with_assets(
        &self,
        write: &ItemWrite<Document>,
        answers: Option<&BTreeMap<String, Value>>,
        assets: &AssetWrite,
    ) -> Result<AssetsWritten, SourceError> {
        self.retiring(
            write.target.as_ref(),
            self.inner
                .write_document_with_assets(write, answers, assets),
        )
        .await
    }

    async fn set_task_rendering_with_assets(
        &self,
        id: &NativeId,
        content: &str,
        provenance: &Value,
        answers: &BTreeMap<String, Value>,
        assets: &AssetWrite,
    ) -> Result<Option<AssetsWritten>, SourceError> {
        self.write(Some(id), || {
            Box::pin(
                self.inner
                    .set_task_rendering_with_assets(id, content, provenance, answers, assets),
            )
        })
        .await
    }

    async fn set_document_rendering_with_assets(
        &self,
        id: &NativeId,
        content: &str,
        provenance: &Value,
        answers: &BTreeMap<String, Value>,
        assets: &AssetWrite,
    ) -> Result<Option<AssetsWritten>, SourceError> {
        self.retiring(
            Some(id),
            self.inner
                .set_document_rendering_with_assets(id, content, provenance, answers, assets),
        )
        .await
    }

    async fn metering(&self) -> Result<Option<Metering>, SourceError> {
        self.inner.metering().await
    }

    async fn end_command(&self) -> Result<(), SourceError> {
        self.cache.end_command();
        self.inner.end_command().await
    }

    async fn visibility(&self, target: &WriteTarget<'_>) -> Result<Visibility, SourceError> {
        self.inner.visibility(target).await
    }

    async fn creation_repository(
        &self,
        target: &WriteTarget<'_>,
    ) -> Result<Option<onetaskgraph_plugin_api::Repository>, SourceError> {
        self.inner.creation_repository(target).await
    }

    async fn creation_refusal(
        &self,
        target: &WriteTarget<'_>,
    ) -> Result<Option<String>, SourceError> {
        self.inner.creation_refusal(target).await
    }

    async fn visibility_with_project(
        &self,
        target: &WriteTarget<'_>,
        project: &WriteTarget<'_>,
    ) -> Result<Visibility, SourceError> {
        self.inner.visibility_with_project(target, project).await
    }

    fn declare_visibility(&self, declared: Visibility) {
        self.inner.declare_visibility(declared);
    }

    fn scope_visibility<'a>(
        &'a self,
        operation: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        self.inner.scope_visibility(operation)
    }

    fn hold_visibility(&self) {
        self.inner.hold_visibility();
    }

    fn release_visibility(&self) {
        self.inner.release_visibility();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use onetaskgraph_in_memory::{InMemoryConfig, InMemorySource};
    use onetaskgraph_plugin_api::{
        Cursor, DependencyEndpoint, DependencyKind, ItemKind, JournalEntry, RequestKind, Shown,
        StatusCategory, WholeTask,
    };

    use super::super::CacheLocation;
    use super::*;

    /// An in-memory source whose read by id answers `answer`, whatever id it was asked for.
    struct Answering {
        inner: InMemorySource,
        answer: Task,
        /// What it saw of each write that reached it, and which tasks it is primed for.
        probe: Arc<std::sync::Mutex<Probe>>,
        /// Another process's view of the cache, read at each write.
        watching: Arc<SourceCache>,
    }

    /// What the source beneath a caching one saw.
    #[derive(Default)]
    struct Probe {
        /// For each priority write that reached it, in order: whether it was primed for the
        /// task, and whether another process could read the task's entry at that moment.
        writes: Vec<(bool, bool)>,
        primed: std::collections::HashSet<NativeId>,
        primings: usize,
        /// For each project or document write that reached it, in order: whether another
        /// process could read that item's entry at that moment.
        kind_writes: Vec<bool>,
        /// How many times it was asked whether it is answering.
        healths: usize,
    }

    #[async_trait::async_trait]
    impl TaskSource for Answering {
        fn kind(&self) -> &'static str {
            self.inner.kind()
        }
        fn capabilities(&self) -> Capabilities {
            self.inner.capabilities()
        }
        async fn health(&self) -> Result<Health, SourceError> {
            self.probe.lock().expect("the probe").healths += 1;
            self.inner.health().await
        }
        async fn get_task(&self, _id: &NativeId) -> Result<Option<Task>, SourceError> {
            Ok(Some(self.answer.clone()))
        }
        async fn set_task_priority(
            &self,
            id: &NativeId,
            priority: Priority,
        ) -> Result<Option<Priority>, SourceError> {
            let visible = self.watching.read(id).is_some();
            let mut probe = self.probe.lock().expect("the probe");
            let primed = probe.primed.contains(id);
            probe.writes.push((primed, visible));
            Ok(Some(priority))
        }
        async fn set_project_metadata(
            &self,
            id: &NativeId,
            _key: &MetadataKey,
            _value: &Value,
        ) -> Result<Option<Project>, SourceError> {
            let visible = self.watching.read_project(id).is_some();
            self.probe
                .lock()
                .expect("the probe")
                .kind_writes
                .push(visible);
            Ok(None)
        }
        async fn set_document_metadata(
            &self,
            id: &NativeId,
            _key: &MetadataKey,
            _value: &Value,
        ) -> Result<Option<Document>, SourceError> {
            let visible = self.watching.read_document(id).is_some();
            self.probe
                .lock()
                .expect("the probe")
                .kind_writes
                .push(visible);
            Ok(None)
        }
        fn prime_write(&self, id: &NativeId, _context: &Value, _board: Option<&Value>) -> bool {
            let mut probe = self.probe.lock().expect("the probe");
            probe.primings += 1;
            probe.primed.insert(id.clone());
            true
        }
        fn unprime(&self, id: &NativeId) {
            self.probe.lock().expect("the probe").primed.remove(id);
        }
        async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
            self.inner.get_project(id).await
        }
        async fn query_tasks(
            &self,
            query: &TaskQuery,
            page: &PageRequest,
        ) -> Result<Page<Task>, SourceError> {
            self.inner.query_tasks(query, page).await
        }
        async fn query_projects(
            &self,
            query: &ProjectQuery,
            page: &PageRequest,
        ) -> Result<Page<Project>, SourceError> {
            self.inner.query_projects(query, page).await
        }
        async fn labels(&self, page: &PageRequest) -> Result<Page<Label>, SourceError> {
            self.inner.labels(page).await
        }
        async fn task_dependencies(
            &self,
            id: &NativeId,
            direction: Direction,
            page: &PageRequest,
        ) -> Result<Page<DependencyEdge>, SourceError> {
            self.inner.task_dependencies(id, direction, page).await
        }
        async fn project_dependencies(
            &self,
            id: &NativeId,
            direction: Direction,
            page: &PageRequest,
        ) -> Result<Page<DependencyEdge>, SourceError> {
            self.inner.project_dependencies(id, direction, page).await
        }
    }

    /// An in-memory source whose task `T-1` has two forward edges, one a page, read at `v1`: a
    /// first page answers the first and the cursor `1`, and any other cursor the second and
    /// nothing after it.
    struct Paging {
        inner: InMemorySource,
    }

    #[async_trait::async_trait]
    impl TaskSource for Paging {
        fn kind(&self) -> &'static str {
            self.inner.kind()
        }
        fn capabilities(&self) -> Capabilities {
            self.inner.capabilities()
        }
        fn forward_edges_version(&self, _id: &NativeId) -> Option<String> {
            Some("v1".to_owned())
        }
        async fn health(&self) -> Result<Health, SourceError> {
            self.inner.health().await
        }
        async fn get_task(&self, id: &NativeId) -> Result<Option<Task>, SourceError> {
            self.inner.get_task(id).await
        }
        async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
            self.inner.get_project(id).await
        }
        async fn query_tasks(
            &self,
            query: &TaskQuery,
            page: &PageRequest,
        ) -> Result<Page<Task>, SourceError> {
            self.inner.query_tasks(query, page).await
        }
        async fn query_projects(
            &self,
            query: &ProjectQuery,
            page: &PageRequest,
        ) -> Result<Page<Project>, SourceError> {
            self.inner.query_projects(query, page).await
        }
        async fn labels(&self, page: &PageRequest) -> Result<Page<Label>, SourceError> {
            self.inner.labels(page).await
        }
        async fn task_dependencies(
            &self,
            id: &NativeId,
            _direction: Direction,
            page: &PageRequest,
        ) -> Result<Page<DependencyEdge>, SourceError> {
            let edge = |to: &str| DependencyEdge {
                from: DependencyEndpoint::from_native(id.clone(), ItemKind::Task),
                to: DependencyEndpoint::from_native(NativeId::from(to), ItemKind::Task),
                kind: DependencyKind::Blocks,
            };
            Ok(match &page.cursor {
                None => Page {
                    items: vec![edge("T-2")],
                    next: Some(Cursor("1".to_owned())),
                },
                Some(_) => Page {
                    items: vec![edge("T-3")],
                    next: None,
                },
            })
        }
        async fn project_dependencies(
            &self,
            id: &NativeId,
            direction: Direction,
            page: &PageRequest,
        ) -> Result<Page<DependencyEdge>, SourceError> {
            self.inner.project_dependencies(id, direction, page).await
        }
    }

    /// A walk of `T-1`'s forward edges through a caching [`Paging`] source in `dir`, its second
    /// page resumed from `cursor`: what it answered, and whether an entry of them was kept.
    async fn walk_resumed_from(dir: &std::path::Path, cursor: &str) -> (usize, bool) {
        let cache = SourceCache::new(
            CacheLocation {
                dir: dir.to_path_buf(),
                remote: "remote".to_owned(),
            },
            VersionCoverage::whole(),
            ProjectVersionCoverage::whole(),
            DocumentVersionCoverage::whole(),
        );
        let source = CachedSource::over(
            Box::new(Paging {
                inner: InMemorySource::new(InMemoryConfig::default()).expect("an empty source"),
            }),
            cache.clone(),
        );
        let page = |cursor: Option<&str>| PageRequest {
            cursor: cursor.map(|cursor| Cursor(cursor.to_owned())),
            limit: 1,
        };
        let first = source
            .task_dependencies(&id("T-1"), Direction::DependsOn, &page(None))
            .await
            .expect("the first page");
        let second = source
            .task_dependencies(&id("T-1"), Direction::DependsOn, &page(Some(cursor)))
            .await
            .expect("the second page");
        (
            first.items.len() + second.items.len(),
            cache
                .read_edges(EdgeKind::TaskForward, &id("T-1"))
                .is_some(),
        )
    }

    #[tokio::test]
    async fn a_walk_resumed_from_a_cursor_its_first_page_never_gave_keeps_no_edges() {
        let dir = tempfile::tempdir().expect("a directory");
        assert_eq!(
            walk_resumed_from(dir.path(), "1").await,
            (2, true),
            "a walk resumed where its first page left off is whole, and kept"
        );
        let dir = tempfile::tempdir().expect("a directory");
        assert_eq!(
            walk_resumed_from(dir.path(), "elsewhere").await,
            (2, false),
            "one resumed from another cursor answers what the source answers and keeps nothing"
        );
    }

    /// An in-memory source whose detail read of any task answers it at `v1` with one comment,
    /// `C-<page>`: the first page's `C-1` with no further page, and a page resumed from a cursor
    /// `C-2`, the last of them.
    struct Paged {
        inner: InMemorySource,
        reads: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl TaskSource for Paged {
        fn kind(&self) -> &'static str {
            self.inner.kind()
        }
        fn capabilities(&self) -> Capabilities {
            self.inner.capabilities()
        }
        async fn health(&self) -> Result<Health, SourceError> {
            self.inner.health().await
        }
        async fn get_task(&self, id: &NativeId) -> Result<Option<Task>, SourceError> {
            Ok(Some(task(id.as_str(), "v1")))
        }
        async fn get_task_details(
            &self,
            ids: &[NativeId],
            comments: Option<&PageRequest>,
        ) -> Vec<Result<Option<TaskDetailRead>, SourceError>> {
            self.reads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let page = if comments.is_some_and(|page| page.cursor.is_some()) {
                "C-2"
            } else {
                "C-1"
            };
            ids.iter()
                .map(|id| {
                    Ok(Some(TaskDetailRead {
                        task: task(id.as_str(), "v1"),
                        comments: comments.map(|_| {
                            Ok(Some(Page {
                                items: vec![Comment {
                                    id: NativeId::from(page),
                                    author: None,
                                    created_at: None,
                                    updated_at: None,
                                    body: page.to_owned(),
                                    url: None,
                                    origin: None,
                                }],
                                next: None,
                            }))
                        }),
                    }))
                })
                .collect()
        }
        async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
            self.inner.get_project(id).await
        }
        async fn query_tasks(
            &self,
            query: &TaskQuery,
            page: &PageRequest,
        ) -> Result<Page<Task>, SourceError> {
            self.inner.query_tasks(query, page).await
        }
        async fn query_projects(
            &self,
            query: &ProjectQuery,
            page: &PageRequest,
        ) -> Result<Page<Project>, SourceError> {
            self.inner.query_projects(query, page).await
        }
        async fn labels(&self, page: &PageRequest) -> Result<Page<Label>, SourceError> {
            self.inner.labels(page).await
        }
        async fn task_dependencies(
            &self,
            id: &NativeId,
            direction: Direction,
            page: &PageRequest,
        ) -> Result<Page<DependencyEdge>, SourceError> {
            self.inner.task_dependencies(id, direction, page).await
        }
        async fn project_dependencies(
            &self,
            id: &NativeId,
            direction: Direction,
            page: &PageRequest,
        ) -> Result<Page<DependencyEdge>, SourceError> {
            self.inner.project_dependencies(id, direction, page).await
        }
    }

    #[tokio::test]
    async fn a_later_page_of_comments_is_never_served_from_an_entry_or_kept_as_the_whole() {
        let dir = tempfile::tempdir().expect("a directory");
        let cache = SourceCache::new(
            CacheLocation {
                dir: dir.path().to_path_buf(),
                remote: "remote".to_owned(),
            },
            VersionCoverage::whole(),
            ProjectVersionCoverage::whole(),
            DocumentVersionCoverage::whole(),
        );
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let paged = Paged {
            inner: InMemorySource::new(InMemoryConfig::default()).expect("an empty source"),
            reads: reads.clone(),
        };
        let source = CachedSource::over(Box::new(paged), cache.clone());
        let sent = || reads.load(std::sync::atomic::Ordering::Relaxed);
        let bodies = |answer: &[Result<Option<TaskDetailRead>, SourceError>]| -> Vec<String> {
            match &answer[0] {
                Ok(Some(TaskDetailRead {
                    comments: Some(Ok(Some(page))),
                    ..
                })) => page
                    .items
                    .iter()
                    .map(|comment| comment.body.clone())
                    .collect(),
                other => panic!("a page of comments: {other:?}"),
            }
        };
        let first = PageRequest {
            cursor: None,
            limit: 10,
        };
        let later = PageRequest {
            cursor: Some(Cursor("C-1".to_owned())),
            limit: 10,
        };

        // A later page, with nothing kept: read from the source, and not kept as every comment.
        cache.pin(&id("T-1"), "v1");
        let read = source.get_task_details(&[id("T-1")], Some(&later)).await;
        assert_eq!(bodies(&read), ["C-2"]);
        assert_eq!(sent(), 1);
        assert_eq!(
            cache.read(&id("T-1")).map(|entry| entry.comments),
            Some(None),
            "the task is kept, and its comments are not"
        );

        // The first page fills them whole; a later page pinned to the same version is still the
        // source's, never the entry's whole list.
        assert_eq!(
            bodies(&source.get_task_details(&[id("T-1")], Some(&first)).await),
            ["C-1"]
        );
        assert_eq!(sent(), 2, "the first page fills");
        // As `task show` completes it: a detail read is served only beside the assets after it.
        source.task_assets(&id("T-1")).await.expect("its assets");
        assert_eq!(
            bodies(&source.get_task_details(&[id("T-1")], Some(&first)).await),
            ["C-1"]
        );
        assert_eq!(sent(), 2, "and is then served");
        let read = source.get_task_details(&[id("T-1")], Some(&later)).await;
        assert_eq!(bodies(&read), ["C-2"], "a later page is the source's");
        assert_eq!(sent(), 3);
        assert_eq!(
            cache
                .read(&id("T-1"))
                .and_then(|entry| entry.comments)
                .map(|comments| comments.len()),
            Some(1),
            "and leaves the whole comments it kept as they were"
        );
    }

    fn id(value: &str) -> NativeId {
        NativeId::from(value)
    }

    fn task(of: &str, version: &str) -> Task {
        Task {
            version: Some(version.to_owned()),
            id: id(of),
            key: None,
            title: format!("Task {of}"),
            content: None,
            status: Status {
                category: StatusCategory::Todo,
                name: "Todo".to_owned(),
            },
            priority: Priority::None,
            labels: Vec::new(),
            project: None,
            url: None,
            location: None,
            created_at: None,
            updated_at: None,
            metadata: BTreeMap::new(),
            repositories: Vec::new(),
            delivers: Vec::new(),
            delivered_by: Vec::new(),
            classification: Default::default(),
            assignees: None,
        }
    }

    /// A caching source over a source whose every read by id answers `answer`, in `dir`.
    fn cached(dir: &std::path::Path, answer: Task) -> (CachedSource, Arc<SourceCache>) {
        let (source, cache, _) = counting(dir, answer);
        (source, cache)
    }

    /// [`cached`], with what the source beneath it saw.
    fn counting(
        dir: &std::path::Path,
        answer: Task,
    ) -> (CachedSource, Arc<SourceCache>, Arc<std::sync::Mutex<Probe>>) {
        let cache = SourceCache::new(
            CacheLocation {
                dir: dir.to_path_buf(),
                remote: "remote".to_owned(),
            },
            VersionCoverage::whole(),
            ProjectVersionCoverage::whole(),
            DocumentVersionCoverage::whole(),
        );
        let inner = Answering {
            inner: InMemorySource::new(InMemoryConfig::default()).expect("an empty source"),
            answer,
            probe: Arc::default(),
            watching: another_process(dir),
        };
        let probe = inner.probe.clone();
        (
            CachedSource::over(Box::new(inner), cache.clone()),
            cache,
            probe,
        )
    }

    /// A caching source in `dir` over a source that holds assignees when `assignees` is set,
    /// whose every read by id answers `answer`.
    fn cached_holding(dir: &std::path::Path, answer: Task, assignees: bool) -> CachedSource {
        let cache = SourceCache::new(
            CacheLocation {
                dir: dir.to_path_buf(),
                remote: "remote".to_owned(),
            },
            VersionCoverage::whole(),
            ProjectVersionCoverage::whole(),
            DocumentVersionCoverage::whole(),
        );
        let inner = Answering {
            inner: InMemorySource::new(InMemoryConfig {
                assignees,
                ..InMemoryConfig::default()
            })
            .expect("an empty source"),
            answer,
            probe: Arc::default(),
            watching: another_process(dir),
        };
        CachedSource::over(Box::new(inner), cache)
    }

    /// The title a pinned read of `T-1` at `v1` answers, through `source`.
    async fn pinned_title(source: &CachedSource) -> String {
        source
            .cache
            .command()
            .pins
            .insert(id("T-1"), "v1".to_owned());
        source
            .get_task(&id("T-1"))
            .await
            .expect("the read")
            .expect("the task")
            .title
    }

    #[tokio::test]
    async fn an_entry_recorded_under_another_assignees_setting_is_never_served() {
        let fresh = |assignees: Option<Vec<String>>| Task {
            title: "From the source".to_owned(),
            assignees,
            ..task("T-1", "v1")
        };
        // Recorded while the source did not hold assignees: served to it again at its pin...
        let dir = tempfile::tempdir().expect("a directory");
        cached_holding(dir.path(), task("T-1", "v1"), false)
            .get_task(&id("T-1"))
            .await
            .expect("the first read");
        let same = cached_holding(dir.path(), fresh(None), false);
        assert_eq!(pinned_title(&same).await, "Task T-1", "the entry answers");
        // ...and never to it once it opts in, though the token is the same.
        let opted = cached_holding(dir.path(), fresh(Some(vec!["ana".to_owned()])), true);
        assert_eq!(pinned_title(&opted).await, "From the source");

        // Recorded while it held assignees, and read after it opted out: also a miss.
        let dir = tempfile::tempdir().expect("a directory");
        let held = Task {
            assignees: Some(vec!["ana".to_owned()]),
            ..task("T-1", "v1")
        };
        cached_holding(dir.path(), held, true)
            .get_task(&id("T-1"))
            .await
            .expect("the first read");
        let out = cached_holding(dir.path(), fresh(None), false);
        assert_eq!(pinned_title(&out).await, "From the source");
    }

    #[tokio::test]
    async fn a_read_answered_with_another_task_writes_no_entry() {
        let dir = tempfile::tempdir().expect("a directory");
        let (source, cache) = cached(dir.path(), task("T-2", "v2"));
        source.get_task(&id("T-1")).await.expect("the read");
        assert!(cache.read(&id("T-1")).is_none());
        assert!(cache.read(&id("T-2")).is_none());
        assert!(!cache.command().read.contains_key(&id("T-1")));
    }

    #[tokio::test]
    async fn a_read_answered_with_its_own_task_writes_its_entry() {
        let dir = tempfile::tempdir().expect("a directory");
        let (source, cache) = cached(dir.path(), task("T-1", "v1"));
        source.get_task(&id("T-1")).await.expect("the read");
        assert_eq!(
            cache.read(&id("T-1")).and_then(|entry| entry.task.version),
            Some("v1".to_owned())
        );
    }

    fn journal(shown: Task) -> WriteJournal {
        WriteJournal {
            requests: vec![JournalEntry {
                part: "issue".to_owned(),
                kind: RequestKind::Mutation,
                outcome: PartOutcome::Landed,
                fields: BTreeMap::new(),
                shown: Shown::Whole {
                    task: Box::new(
                        WholeTask::new(shown, Vec::new(), Vec::new(), None).expect("a version"),
                    ),
                },
            }],
        }
    }

    #[test]
    fn a_write_whose_response_shows_another_task_advances_no_entry() {
        let dir = tempfile::tempdir().expect("a directory");
        let (source, cache) = cached(dir.path(), task("T-1", "v1"));
        cache.write(&Entry {
            schema: ENTRY_SCHEMA,
            remote: "remote".to_owned(),
            task: task("T-1", "v1"),
            comments: Some(Vec::new()),
            assets: None,
            edges: None,
            context: None,
        });
        let written = source.settle(
            &id("T-1"),
            Some(&journal(task("T-2", "v2"))),
            String::new(),
            None,
            None,
        );
        assert!(
            cache.read(&id("T-1")).is_none(),
            "this task's entry is left unserveable"
        );
        assert!(
            cache.read(&id("T-2")).is_none(),
            "nothing is written for the other task"
        );
        assert_eq!(
            written
                .result
                .and_then(|result| result.parts)
                .map(|parts| parts.len()),
            Some(1)
        );
    }

    #[test]
    fn a_write_whose_response_shows_its_own_task_whole_advances_its_entry() {
        let dir = tempfile::tempdir().expect("a directory");
        let (source, cache) = cached(dir.path(), task("T-1", "v1"));
        source.settle(
            &id("T-1"),
            Some(&journal(task("T-1", "v2"))),
            String::new(),
            None,
            None,
        );
        assert_eq!(
            cache.read(&id("T-1")).and_then(|entry| entry.task.version),
            Some("v2".to_owned())
        );
    }

    /// The entry the tests below start from: `T-1` at `v1`, served.
    fn held_at_v1(cache: &SourceCache) {
        cache.write(&Entry {
            schema: ENTRY_SCHEMA,
            remote: "remote".to_owned(),
            task: task("T-1", "v1"),
            comments: Some(Vec::new()),
            assets: None,
            edges: None,
            context: Some(serde_json::json!({"node": "I_1"})),
        });
        assert!(cache.read(&id("T-1")).is_some(), "the entry starts served");
    }

    /// Another process's view of the same cache directory.
    fn another_process(dir: &std::path::Path) -> Arc<SourceCache> {
        SourceCache::new(
            CacheLocation {
                dir: dir.to_path_buf(),
                remote: "remote".to_owned(),
            },
            VersionCoverage::whole(),
            ProjectVersionCoverage::whole(),
            DocumentVersionCoverage::whole(),
        )
    }

    /// Sets a path's mode for as long as it lives, and gives the old one back.
    #[cfg(unix)]
    struct Mode {
        path: std::path::PathBuf,
        was: u32,
    }

    #[cfg(unix)]
    impl Mode {
        fn set(path: &std::path::Path, mode: u32) -> Self {
            use std::os::unix::fs::PermissionsExt as _;
            let was = std::fs::metadata(path)
                .expect("the path")
                .permissions()
                .mode();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                .expect("the mode");
            Self {
                path: path.to_path_buf(),
                was,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for Mode {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.was));
        }
    }

    /// Make the entries' directory refuse every removal, rename and new file, answering `None`
    /// on a host where that does not take — a superuser's — after saying so.
    #[cfg(unix)]
    fn refusing(dir: &std::path::Path) -> Option<Mode> {
        refusing_in(&dir.join("tasks"))
    }

    /// Make `directory` refuse every removal, rename and new file, as [`refusing`] does.
    #[cfg(unix)]
    fn refusing_in(directory: &std::path::Path) -> Option<Mode> {
        let mode = Mode::set(directory, 0o555);
        if std::fs::File::create(directory.join("probe")).is_ok() {
            eprintln!(
                "skipped: this host lets a read-only directory be written, so no removal can be \
                 made to fail here"
            );
            return None;
        }
        Some(mode)
    }

    #[cfg(unix)]
    fn entry_file(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("tasks")
            .join(format!("{}.json", super::super::digest("T-1")))
    }

    #[cfg(unix)]
    #[test]
    fn an_invalidation_the_filesystem_will_not_remove_still_serves_the_old_entry_to_no_one() {
        let dir = tempfile::tempdir().expect("a directory");
        let (source, cache) = cached(dir.path(), task("T-1", "v1"));
        held_at_v1(&cache);
        let Some(_refusing) = refusing(dir.path()) else {
            return;
        };
        // A response showing another task cannot vouch for this one, so it invalidates.
        source.settle(
            &id("T-1"),
            Some(&journal(task("T-2", "v2"))),
            String::new(),
            None,
            None,
        );
        assert!(
            entry_file(dir.path()).exists(),
            "the removal was refused, so the file is still there"
        );
        assert!(cache.read(&id("T-1")).is_none(), "this process serves it");
        assert!(
            another_process(dir.path()).read(&id("T-1")).is_none(),
            "another process serves it"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_write_the_filesystem_will_not_take_leaves_the_old_entry_served_to_no_one() {
        let dir = tempfile::tempdir().expect("a directory");
        let (source, cache) = cached(dir.path(), task("T-1", "v1"));
        held_at_v1(&cache);
        let refused = refusing(dir.path());
        if refused.is_none() {
            return;
        }
        // A response showing the task whole at `v2`, whose entry cannot be written.
        source.settle(
            &id("T-1"),
            Some(&journal(task("T-1", "v2"))),
            String::new(),
            None,
            None,
        );
        assert!(cache.read(&id("T-1")).is_none(), "this process serves `v1`");
        assert!(
            another_process(dir.path()).read(&id("T-1")).is_none(),
            "another process serves `v1`"
        );
        drop(refused);
        source.settle(
            &id("T-1"),
            Some(&journal(task("T-1", "v3"))),
            String::new(),
            None,
            None,
        );
        assert_eq!(
            another_process(dir.path())
                .read(&id("T-1"))
                .and_then(|entry| entry.task.version),
            Some("v3".to_owned())
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_entry_whose_own_mode_refuses_writes_is_still_taken_out_of_service() {
        let dir = tempfile::tempdir().expect("a directory");
        let (_, cache) = cached(dir.path(), task("T-1", "v1"));
        held_at_v1(&cache);
        let _read_only = Mode::set(&entry_file(dir.path()), 0o444);
        let Some(_refusing) = refusing(dir.path()) else {
            return;
        };
        cache.invalidate(&id("T-1"));
        assert!(
            another_process(dir.path()).read(&id("T-1")).is_none(),
            "another process serves it"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_board_context_the_filesystem_will_not_replace_or_drop_is_served_to_no_one() {
        let dir = tempfile::tempdir().expect("a directory");
        let (_, cache) = cached(dir.path(), task("T-1", "v1"));
        cache.write_board(serde_json::json!({"option": "stale"}));
        assert!(cache.board().is_some(), "the board starts served");
        {
            let Some(_refusing) = refusing_in(dir.path()) else {
                return;
            };
            cache.write_board(serde_json::json!({"option": "fresh"}));
            assert!(
                dir.path().join("board.json").exists(),
                "the file is still there"
            );
            assert!(
                cache.board().is_none(),
                "the replaced context is not served"
            );
            assert!(
                another_process(dir.path()).board().is_none(),
                "nor to another process"
            );
        }
        cache.write_board(serde_json::json!({"option": "stale"}));
        let Some(_refusing) = refusing_in(dir.path()) else {
            return;
        };
        cache.drop_board();
        assert!(cache.board().is_none(), "the dropped context is not served");
        assert!(
            another_process(dir.path()).board().is_none(),
            "nor to another process"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_entry_that_is_a_symlink_never_changes_the_file_it_points_at() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("a directory");
        let outside = tempfile::tempdir().expect("a directory outside the cache");
        let precious = outside.path().join("precious.json");
        let (source, cache, probe) = counting(dir.path(), task("T-1", "v1"));
        held_at_v1(&cache);
        std::fs::rename(entry_file(dir.path()), &precious).expect("the entry moved out");
        std::os::unix::fs::symlink(&precious, entry_file(dir.path())).expect("the symlink");
        let _read_only = Mode::set(&precious, 0o444);
        let held = std::fs::read(&precious).expect("the file pointed at");
        let Some(_refusing) = refusing(dir.path()) else {
            return;
        };

        cache.invalidate(&id("T-1"));
        source
            .set_task_priority(&id("T-1"), Priority::High)
            .await
            .expect_err("a write whose entry cannot be retired is refused");
        assert_eq!(
            std::fs::read(&precious).expect("the file"),
            held,
            "its bytes"
        );
        assert_eq!(
            std::fs::metadata(&precious)
                .expect("the file")
                .permissions()
                .mode()
                & 0o777,
            0o444,
            "its mode"
        );
        assert!(
            probe.lock().expect("the probe").writes.is_empty(),
            "nothing was sent"
        );
    }

    /// The path is swapped for another file between the look and the open, which no test can
    /// time, so the look is handed in: another file's, standing for the one the path held when
    /// it was inspected.
    #[cfg(unix)]
    #[test]
    fn a_file_replaced_after_it_was_inspected_is_never_emptied() {
        let dir = tempfile::tempdir().expect("a directory");
        let looked_at = dir.path().join("looked-at.json");
        let replacement = dir.path().join("replacement.json");
        std::fs::write(&looked_at, b"{}").expect("the file looked at");
        std::fs::write(&replacement, b"a person's file").expect("the replacement");
        let inspected = std::fs::symlink_metadata(&looked_at).expect("the look");
        let refused = super::super::empty_inspected(&replacement, &inspected)
            .expect_err("the replacement is not emptied");
        assert!(refused.to_string().contains("replaced"), "{refused}");
        assert_eq!(
            std::fs::read(&replacement).expect("the replacement"),
            b"a person's file"
        );
    }

    /// Windows refuses to delete, or open for writing, a file another handle holds open without
    /// sharing those: a real refusal, needing no privilege, of everything retirement can try
    /// there.
    #[cfg(windows)]
    #[tokio::test]
    async fn an_entry_held_open_against_deletion_is_withheld_its_write_refused_and_then_recovers() {
        use std::os::windows::fs::OpenOptionsExt as _;
        /// `FILE_SHARE_READ`: others may read the file, and neither write nor delete it.
        const SHARE_READ_ONLY: u32 = 0x1;
        let dir = tempfile::tempdir().expect("a directory");
        let (source, cache, probe) = counting(dir.path(), task("T-1", "v1"));
        held_at_v1(&cache);
        let path = dir
            .path()
            .join("tasks")
            .join(format!("{}.json", super::super::digest("T-1")));
        let holding = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(SHARE_READ_ONLY)
            .open(&path)
            .expect("the entry held open");

        cache.invalidate(&id("T-1"));
        assert!(path.exists(), "the removal was refused");
        assert!(cache.read(&id("T-1")).is_none(), "the entry is withheld");
        assert!(another_process(dir.path()).read(&id("T-1")).is_some());
        let refused = source
            .set_task_priority(&id("T-1"), Priority::High)
            .await
            .expect_err("the write is refused");
        assert!(refused.to_string().contains("was not sent"), "{refused}");
        assert!(
            probe.lock().expect("the probe").writes.is_empty(),
            "nothing was sent"
        );

        drop(holding);
        source
            .set_task_priority(&id("T-1"), Priority::High)
            .await
            .expect("the write is sent");
        assert_eq!(probe.lock().expect("the probe").writes, [(false, false)]);
        assert!(!path.exists(), "the entry was retired before it was sent");
    }

    /// Set in the child [`a_cache_directory_mounted_read_only_withholds_refuses_and_recovers`]
    /// runs itself as, naming the directory mounted read-only over it.
    #[cfg(target_os = "linux")]
    const READ_ONLY_CHILD: &str = "ONETASKGRAPH_TEST_READ_ONLY_CACHE";

    /// A cache directory none of whose files can be removed, overwritten, emptied or re-moded:
    /// this test runs itself again under an unprivileged read-only bind mount of it, where
    /// `EROFS` refuses each of those for real, then lifts the mount to watch it recover.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_cache_directory_mounted_read_only_withholds_refuses_and_recovers() {
        if let Some(dir) = std::env::var_os(READ_ONLY_CHILD) {
            read_only_child(std::path::Path::new(&dir)).await;
            println!("read-only child ran");
            return;
        }
        let dir = tempfile::tempdir().expect("a directory");
        let (_, cache) = cached(dir.path(), task("T-1", "v1"));
        held_at_v1(&cache);
        cache.write_board(serde_json::json!({"option": "stale"}));
        let output = std::process::Command::new("unshare")
            .args(["--user", "--map-root-user", "--mount", "sh", "-c"])
            .arg(r#"mount --bind -o ro "$1" "$1" || exit 77; ulimit -n 256; shift; exec "$@""#)
            .arg("sh")
            .arg(dir.path())
            .arg(std::env::current_exe().expect("this test binary"))
            .args([
                "cache::source::tests::a_cache_directory_mounted_read_only_withholds_refuses_and_recovers",
                "--exact",
                "--nocapture",
            ])
            .env(READ_ONLY_CHILD, dir.path())
            .output();
        // `unshare` refusing to make the namespace, or `mount` the bind, is a host that cannot.
        let Some(output) = output.ok().filter(|output| {
            output.status.code() != Some(77)
                && !String::from_utf8_lossy(&output.stderr).starts_with("unshare:")
        }) else {
            eprintln!(
                "skipped: this host cannot mount a directory read-only without privileges, so no \
                 cache file can be made to refuse every removal, overwrite and re-mode here"
            );
            return;
        };
        let printed = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && printed.contains("read-only child ran"),
            "the child under the read-only mount failed\n{printed}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// What [`a_cache_directory_mounted_read_only_withholds_refuses_and_recovers`] proves,
    /// under the read-only mount of `dir`.
    #[cfg(target_os = "linux")]
    async fn read_only_child(dir: &std::path::Path) {
        let (source, cache, probe) = counting(dir, task("T-1", "v1"));
        assert!(cache.read(&id("T-1")).is_some(), "the entry can be read");
        assert!(cache.board().is_some(), "the board can be read");

        // An expected write primes the source from the entry, then cannot retire it: it is
        // refused unsent, and the source is primed no longer.
        cache.expect(&id("T-1"), "v1");
        let refused = source
            .set_task_priority(&id("T-1"), Priority::High)
            .await
            .expect_err("an expected write is refused");
        assert!(
            refused.to_string().contains(&dir.display().to_string()),
            "{refused}"
        );
        {
            let probe = probe.lock().expect("the probe");
            assert!(probe.writes.is_empty(), "nothing was sent");
            assert_eq!(probe.primings, 1, "the source was primed from the entry");
            assert!(probe.primed.is_empty(), "the source was unprimed");
        }

        // This process out of descriptors cannot open the entry, which says nothing about
        // whether another can: the write is refused all the same.
        let mut descriptors = Vec::new();
        while let Ok(file) = std::fs::File::open("/dev/null") {
            descriptors.push(file);
            assert!(descriptors.len() < 4096, "the descriptor limit is in force");
        }
        let written = source.set_task_priority(&id("T-1"), Priority::High).await;
        drop(descriptors);
        assert!(
            written.is_err(),
            "a write whose entry is still there is refused"
        );
        assert!(
            probe.lock().expect("the probe").writes.is_empty(),
            "nothing was sent"
        );
        let read = std::process::Command::new("cat")
            .arg(entry_file(dir))
            .output()
            .expect("a separate reader");
        let seen: Entry = serde_json::from_slice(&read.stdout).expect("the entry it read");
        assert_eq!(
            (seen.task.version.as_deref(), seen.task.priority),
            (Some("v1"), Priority::None),
            "what a separate reader is served is what the source still holds"
        );

        // Nothing can take them out of service on disk, so this process withholds them from
        // itself, and another process can still read them — which is why a plain write is
        // refused too.
        cache.invalidate(&id("T-1"));
        cache.drop_board();
        assert!(entry_file(dir).exists() && dir.join("board.json").exists());
        assert!(cache.read(&id("T-1")).is_none(), "the entry is withheld");
        assert!(cache.board().is_none(), "the board is withheld");
        assert!(another_process(dir).read(&id("T-1")).is_some());
        source
            .set_task_priority(&id("T-1"), Priority::High)
            .await
            .expect_err("a plain write is refused");
        assert!(
            probe.lock().expect("the probe").writes.is_empty(),
            "nothing was sent"
        );

        // Writable again, the next write is a plain one — neither the refused write's priming
        // nor its expectation outlived it — and what this process writes is served again.
        let lifted = std::process::Command::new("umount")
            .arg(dir)
            .status()
            .expect("umount");
        assert!(lifted.success(), "the mount is lifted");
        source
            .set_task_priority(&id("T-1"), Priority::High)
            .await
            .expect("the write is sent");
        assert_eq!(
            probe.lock().expect("the probe").writes,
            [(false, false)],
            "one write, unprimed"
        );
        assert!(
            cache.take_expected(&id("T-1")).is_none(),
            "the write was not taken for the refused expected one"
        );
        cache.write(&Entry {
            schema: ENTRY_SCHEMA,
            remote: "remote".to_owned(),
            task: task("T-1", "v2"),
            comments: Some(Vec::new()),
            assets: None,
            edges: None,
            context: None,
        });
        assert_eq!(
            cache.read(&id("T-1")).and_then(|entry| entry.task.version),
            Some("v2".to_owned())
        );
        cache.write_board(serde_json::json!({"option": "fresh"}));
        assert_eq!(cache.board(), Some(serde_json::json!({"option": "fresh"})));
    }

    #[tokio::test]
    async fn a_write_is_sent_only_once_no_other_process_can_read_its_entry() {
        let dir = tempfile::tempdir().expect("a directory");
        for expected in [false, true] {
            let (source, cache, probe) = counting(dir.path(), task("T-1", "v1"));
            held_at_v1(&cache);
            if expected {
                cache.expect(&id("T-1"), "v1");
            }
            source
                .set_task_priority(&id("T-1"), Priority::High)
                .await
                .expect("the write is sent");
            assert_eq!(
                probe.lock().expect("the probe").writes,
                [(expected, false)],
                "at the moment the write was sent, another process could not read the entry \
                 (expected: {expected})"
            );
        }
    }

    #[tokio::test]
    async fn a_project_or_document_write_is_sent_only_once_no_other_process_can_read_its_entry() {
        let dir = tempfile::tempdir().expect("a directory");
        let (source, cache, probe) = counting(dir.path(), task("T-1", "v1"));
        let project = Project {
            version: Some("v1".to_owned()),
            id: id("P-1"),
            title: "Plan".to_owned(),
            content: None,
            status: Status {
                category: StatusCategory::Todo,
                name: "Todo".to_owned(),
            },
            labels: Vec::new(),
            url: None,
            location: None,
            created_at: None,
            updated_at: None,
            metadata: BTreeMap::new(),
            repositories: Vec::new(),
            classification: Default::default(),
        };
        cache.write_project(&project);
        let document = Document {
            version: Some("v1".to_owned()),
            id: id("D-1"),
            title: "Design".to_owned(),
            content: None,
            project: None,
            labels: Vec::new(),
            url: None,
            location: None,
            created_at: None,
            updated_at: None,
            metadata: BTreeMap::new(),
            repositories: Vec::new(),
            classification: Default::default(),
        };
        cache.write_document(&super::DocumentEntry {
            schema: KIND_ENTRY_SCHEMA,
            kind: super::super::kinds::DocumentTag::Document,
            remote: "remote".to_owned(),
            document,
            assets: None,
        });
        assert!(
            another_process(dir.path())
                .read_project(&id("P-1"))
                .is_some()
        );
        assert!(
            another_process(dir.path())
                .read_document(&id("D-1"))
                .is_some()
        );
        let key = MetadataKey::new("caller.note").expect("a key");
        source
            .set_project_metadata(&id("P-1"), &key, &serde_json::json!("noted"))
            .await
            .expect("the write is sent");
        source
            .set_document_metadata(&id("D-1"), &key, &serde_json::json!("noted"))
            .await
            .expect("the write is sent");
        assert_eq!(
            probe.lock().expect("the probe").kind_writes,
            [false, false],
            "at the moment each write was sent, another process could not read its entry"
        );
        // And neither entry is served afterwards: the write's own response vouches for nothing.
        assert!(cache.read_project(&id("P-1")).is_none());
        assert!(cache.read_document(&id("D-1")).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_project_write_whose_entry_cannot_be_retired_is_refused_unsent() {
        let dir = tempfile::tempdir().expect("a directory");
        let (source, cache, probe) = counting(dir.path(), task("T-1", "v1"));
        cache.write_project(&Project {
            version: Some("v1".to_owned()),
            id: id("P-1"),
            title: "Plan".to_owned(),
            content: None,
            status: Status {
                category: StatusCategory::Todo,
                name: "Todo".to_owned(),
            },
            labels: Vec::new(),
            url: None,
            location: None,
            created_at: None,
            updated_at: None,
            metadata: BTreeMap::new(),
            repositories: Vec::new(),
            classification: Default::default(),
        });
        let Some(_refusing) =
            refusing_in(&dir.path().join(format!("projects-v{}", KIND_ENTRY_SCHEMA)))
        else {
            return;
        };
        let _read_only = Mode::set(
            &dir.path()
                .join(format!("projects-v{}", KIND_ENTRY_SCHEMA))
                .join(format!("{}.json", super::super::digest("P-1"))),
            0o444,
        );
        let refused = source
            .set_project_metadata(
                &id("P-1"),
                &MetadataKey::new("caller.note").expect("a key"),
                &serde_json::json!("noted"),
            )
            .await;
        // Emptied in place where its directory refuses the removal: a miss for every process,
        // so the write is sent; refused only where even that is refused.
        match refused {
            Ok(_) => assert_eq!(probe.lock().expect("the probe").kind_writes, [false]),
            Err(error) => {
                assert!(error.to_string().contains("was not sent"), "{error}");
                assert!(probe.lock().expect("the probe").kind_writes.is_empty());
            }
        }
        assert!(
            another_process(dir.path())
                .read_project(&id("P-1"))
                .is_none(),
            "no other process is served the entry"
        );
    }

    /// No command line verb and no engine path asks a source whether it is answering, so this is
    /// the one place it can be held to: the cache answers it never, whatever an entry holds.
    #[tokio::test]
    async fn whether_a_source_is_answering_is_asked_of_the_source_every_time() {
        let dir = tempfile::tempdir().expect("a directory");
        let (source, cache, probe) = counting(dir.path(), task("T-1", "v1"));
        held_at_v1(&cache);
        cache.pin(&id("T-1"), "v1");
        for _ in 0..2 {
            source.health().await.expect("answering");
        }
        assert_eq!(probe.lock().expect("the probe").healths, 2);
    }

    /// A source answering every batch of comments with one answer more than it was given. Only
    /// the test that withholds a directory's write permission uses it, which is unix-only.
    #[cfg(unix)]
    struct Overanswering {
        inner: InMemorySource,
    }

    #[cfg(unix)]
    #[async_trait::async_trait]
    impl TaskSource for Overanswering {
        fn kind(&self) -> &'static str {
            self.inner.kind()
        }
        fn capabilities(&self) -> Capabilities {
            self.inner.capabilities()
        }
        async fn health(&self) -> Result<Health, SourceError> {
            self.inner.health().await
        }
        async fn get_task(&self, id: &NativeId) -> Result<Option<Task>, SourceError> {
            self.inner.get_task(id).await
        }
        async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
            self.inner.get_project(id).await
        }
        async fn query_tasks(
            &self,
            query: &TaskQuery,
            page: &PageRequest,
        ) -> Result<Page<Task>, SourceError> {
            self.inner.query_tasks(query, page).await
        }
        async fn query_projects(
            &self,
            query: &ProjectQuery,
            page: &PageRequest,
        ) -> Result<Page<Project>, SourceError> {
            self.inner.query_projects(query, page).await
        }
        async fn labels(&self, page: &PageRequest) -> Result<Page<Label>, SourceError> {
            self.inner.labels(page).await
        }
        async fn task_dependencies(
            &self,
            id: &NativeId,
            direction: Direction,
            page: &PageRequest,
        ) -> Result<Page<DependencyEdge>, SourceError> {
            self.inner.task_dependencies(id, direction, page).await
        }
        async fn project_dependencies(
            &self,
            id: &NativeId,
            direction: Direction,
            page: &PageRequest,
        ) -> Result<Page<DependencyEdge>, SourceError> {
            self.inner.project_dependencies(id, direction, page).await
        }
        async fn add_comments(
            &self,
            comments: &[onetaskgraph_plugin_api::BatchComment],
        ) -> Vec<Result<onetaskgraph_plugin_api::BatchAnswer<Comment>, SourceError>> {
            (0..=comments.len())
                .map(|_| Ok(onetaskgraph_plugin_api::BatchAnswer::Missing))
                .collect()
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_batch_answered_with_a_surplus_is_never_cut_to_look_whole() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("a directory");
        let cache = SourceCache::new(
            CacheLocation {
                dir: dir.path().to_path_buf(),
                remote: "remote".to_owned(),
            },
            VersionCoverage::whole(),
            ProjectVersionCoverage::whole(),
            DocumentVersionCoverage::whole(),
        );
        let source = CachedSource::over(
            Box::new(Overanswering {
                inner: InMemorySource::new(InMemoryConfig::default()).expect("an empty source"),
            }),
            cache.clone(),
        );
        let comment = |task: &str| onetaskgraph_plugin_api::BatchComment {
            task: id(task),
            comment: onetaskgraph_plugin_api::NewComment {
                body: CommentBody::new("A note.".to_owned()).expect("a body"),
                author: None,
            },
            if_version: None,
        };
        let batch = [comment("T-1"), comment("T-2"), comment("T-3")];

        // Every item sent: the source's answer goes back as it gave it, surplus and all, for
        // the engine to hold to one per item.
        let answered = source.add_comments(&batch).await;
        assert_eq!(answered.len(), 4, "the surplus is passed on");

        // T-2's entry cannot be retired, so it alone is refused; the other two are sent, and
        // their surplus answer is held to one per item — each malformed, none silently dropped.
        cache.write(&Entry {
            schema: ENTRY_SCHEMA,
            remote: "remote".to_owned(),
            task: task("T-2", "v1"),
            comments: None,
            assets: None,
            edges: None,
            context: None,
        });
        let path = cache.task_path(&id("T-2"));
        let tasks = path.parent().expect("the entries' directory").to_path_buf();
        let held = tasks.join("held-elsewhere");
        std::fs::rename(&path, &held).expect("moved aside");
        std::os::unix::fs::symlink(&held, &path).expect("a symlink");
        std::fs::set_permissions(&tasks, std::fs::Permissions::from_mode(0o555))
            .expect("read-only");
        if std::fs::remove_file(&path).is_ok() {
            // Running as a user no directory mode stops — nothing to prove here.
            return;
        }
        let answered = source.add_comments(&batch).await;
        std::fs::set_permissions(&tasks, std::fs::Permissions::from_mode(0o755)).expect("writable");
        assert_eq!(answered.len(), 3, "one answer per item");
        assert!(
            matches!(&answered[1], Err(SourceError::Config { message })
                if message.contains("the write was not sent")),
            "{:?}",
            answered[1]
        );
        for at in [0, 2] {
            assert!(
                matches!(&answered[at], Err(SourceError::Malformed { message })
                    if message.contains("answered 3 answers to the 2 items")),
                "{:?}",
                answered[at]
            );
        }
    }
}
