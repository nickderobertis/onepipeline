//! A write held to the version of the task it writes: `--if-version`.
//!
//! [`Engine::held_write`] runs a write inside a hold on one task and answers
//! [`EngineError::VersionMoved`] when the task was at any other version. The hold is task-local
//! to that one call, so concurrent held writes on one engine — of one task or of several — never
//! see each other's, and it is made good at the one place every call to a source passes —
//! `HeldOff`, which every ready source sits behind — on the first write of that task the call
//! sends, and nowhere after it: the writes a verb sends after its first are its own
//! continuation, not a competitor.
//!
//! How the check is made is the source's to say, through
//! [`TaskSource::holds_versions`](onetaskgraph_plugin_api::TaskSource::holds_versions): a source
//! that serializes its own writes checks inside the write, as one step with it; for any other,
//! `HeldOff` reads the task through the source immediately before the write and refuses there,
//! the time between that read and the mutation being the backend's residual race.
//!
//! A verb that ends up sending no write at all — every field already as named — is still held:
//! the task is read once afterwards, and a moved version refused, so a stale caller is told
//! whatever the write turned out to need.

use std::cmp::Ordering;
use std::future::Future;
use std::sync::{Arc, Mutex};

use onetaskgraph_plugin_api::{HeldVersion, NativeId, SourceError, SourceName, TaskSource};

use super::delivery::source_failed;
use super::{CopyReport, CopyRequest, CopyScope, Engine, EngineError};
use crate::GlobalId;

tokio::task_local! {
    static HOLD: Arc<Hold>;
}

/// One held call's hold on one task: which task, once known, and what its check came to.
#[derive(Debug)]
struct Hold {
    version: String,
    state: Mutex<HoldState>,
}

#[derive(Debug, Default)]
struct HoldState {
    /// The task held — named up front by a write, and by the copy once it reads its target.
    target: Option<(SourceName, NativeId)>,
    /// What checking the hold came to.
    check: Check,
}

/// What checking a hold came to: not yet made, made and passed, or made and refused at the
/// version the task was found at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Check {
    /// No write of the task has reached its source yet.
    #[default]
    Unchecked,
    /// The first write of the task reached its source, at the version held.
    Passed,
    /// The first write of the task found it at this version instead, and wrote nothing.
    Refused(String),
}

impl Hold {
    fn new(version: &str, target: Option<(SourceName, NativeId)>) -> Arc<Self> {
        Arc::new(Self {
            version: version.to_owned(),
            state: Mutex::new(HoldState {
                target,
                ..HoldState::default()
            }),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, HoldState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The version the next write of `id` in `source` is held to — once: the write that takes
    /// it is the one checked, and every later write of the call is its continuation.
    fn take_for_write(&self, source: &SourceName, id: &NativeId) -> Option<String> {
        let mut state = self.state();
        let held = state
            .target
            .as_ref()
            .is_some_and(|(name, native)| name == source && native == id);
        if !held || state.check != Check::Unchecked {
            return None;
        }
        state.check = Check::Passed;
        Some(self.version.clone())
    }
}

/// Run `write`, a write of `id` in `source`, held to the version the call in progress holds it
/// to when it is that call's first write of it: inside the source's own check when `inner`
/// makes one, and otherwise after a read of the task through `inner` immediately before it.
pub(crate) async fn held<T>(
    source: &SourceName,
    inner: &dyn TaskSource,
    id: &NativeId,
    write: impl Future<Output = Result<T, SourceError>>,
) -> Result<T, SourceError> {
    let Ok(hold) = HOLD.try_with(Arc::clone) else {
        return write.await;
    };
    let Some(version) = hold.take_for_write(source, id) else {
        return write.await;
    };
    if inner.holds_versions() {
        let held = Arc::new(HeldVersion::new(id.clone(), version));
        let answered = HeldVersion::scope(Arc::clone(&held), write).await;
        if let Some(found) = held.found() {
            hold.state().check = Check::Refused(found);
        }
        return answered;
    }
    let found = inner.get_task(id).await?.and_then(|task| task.version);
    if !at(inner, found.as_deref(), &version) {
        let found = found.unwrap_or_default();
        hold.state().check = Check::Refused(found.clone());
        return Err(SourceError::Refused {
            message: format!(
                "task {id} is at version {found:?}, not the version the write is held to, so \
                 nothing was written"
            ),
        });
    }
    write.await
}

/// Whether `found` is the version `version` names, in `source`'s own order of its tokens.
fn at(source: &dyn TaskSource, found: Option<&str>, version: &str) -> bool {
    found.is_some_and(|found| source.order_versions(found, version) == Some(Ordering::Equal))
}

impl Engine {
    /// Run `write`, a write of the task `id`, held to `version`: the write is refused, writing
    /// nothing, when the task is at any other version, rather than overwriting whoever moved it.
    ///
    /// The first write of `id` the call sends is the one held. A source that serializes its
    /// own writes (a folder of Markdown, an in-memory source) checks the version and makes that
    /// write as one step, so no competing write lands between them; for any other source the
    /// task is read through it immediately before the write, and a change landing between that
    /// read and the mutation is the backend's residual race, which its plugin's documentation
    /// states. A write that ends up sending nothing is checked by one read after it.
    ///
    /// The hold is the call's own, so held writes made concurrently on one engine are each held
    /// to their own version. Distinct from [`Engine::expect`], which is about cost and ids and
    /// holds nothing.
    ///
    /// # Errors
    ///
    /// [`EngineError::VersionMoved`] naming the version the task was found at when it was at
    /// another, [`EngineError::UnknownSource`] for a source nothing configures, and whatever
    /// `write` fails with.
    pub async fn held_write<T>(
        &self,
        id: &GlobalId,
        version: &str,
        write: impl Future<Output = Result<T, EngineError>>,
    ) -> Result<T, EngineError> {
        let name = self.known(&id.source)?;
        let hold = Hold::new(version, Some((name, id.native.clone())));
        let written = HOLD.scope(Arc::clone(&hold), write).await;
        self.settled(&hold, written).await
    }

    /// [`Engine::copy`] of one task, held by `--if-version` to `version`: the copy's write of
    /// the destination task it finds is refused, writing nothing there, when that task is at any
    /// other version — checked as [`Engine::held_write`] checks a write. A copy that creates its
    /// counterpart holds nothing, since there is no version for it to be at.
    ///
    /// # Errors
    ///
    /// [`EngineError::IfVersionOneTask`] beside more than one item, a query, a project or a
    /// document, refused before anything is read; [`EngineError::VersionMoved`] when the
    /// destination task was at another version; and whatever [`Engine::copy`] fails with.
    pub async fn copy_held(
        &self,
        request: &CopyRequest,
        version: &str,
    ) -> Result<CopyReport, EngineError> {
        if request.items.query().is_some()
            || request.items.as_slice().len() != 1
            || request.scope != CopyScope::Tasks
        {
            return Err(EngineError::IfVersionOneTask);
        }
        let hold = Hold::new(version, None);
        let copied = HOLD.scope(Arc::clone(&hold), self.copy(request)).await;
        self.settled(&hold, copied).await
    }

    /// Whether the copy in progress holds its one target to a version, so the target's read and
    /// its write are made by themselves, with nothing read ahead of them or held back.
    pub(crate) fn holds_copy(&self) -> bool {
        HOLD.try_with(|_| ()).is_ok()
    }

    /// Hold the first write of the destination task `id` the copy in progress reads, when that
    /// copy is held by `--if-version` — the first task it reads, and no other.
    pub(crate) fn hold_copied(&self, destination: &SourceName, id: &NativeId) {
        let _ = HOLD.try_with(|hold| {
            let mut state = hold.state();
            if state.target.is_none() {
                state.target = Some((destination.clone(), id.clone()));
            }
        });
    }

    /// Whether the call in progress holds `id` in `destination` and its check refused the write,
    /// so that write sent nothing a copy has to take back.
    pub(crate) fn hold_refused(&self, destination: &SourceName, id: &NativeId) -> bool {
        HOLD.try_with(|hold| {
            let state = hold.state();
            matches!(state.check, Check::Refused(_))
                && state
                    .target
                    .as_ref()
                    .is_some_and(|(name, native)| name == destination && native == id)
        })
        .unwrap_or(false)
    }

    /// What a held call came to once it has run: refused when its check found the task moved,
    /// and — when nothing reached the source to be checked — when one read now does.
    async fn settled<T>(
        &self,
        hold: &Hold,
        written: Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let (target, check) = {
            let state = hold.state();
            (state.target.clone(), state.check.clone())
        };
        let Some((source, native)) = target else {
            return written;
        };
        let id = GlobalId { source, native };
        match check {
            Check::Refused(found) => return Err(moved_error(&id, &hold.version, found)),
            Check::Passed => return written,
            Check::Unchecked if written.is_err() => return written,
            Check::Unchecked => {}
        }
        // Nothing reached the source: the version is still the caller's to be told about.
        let source = self.built(&id.source)?;
        let found = source
            .source()
            .get_task(&id.native)
            .await
            .map_err(|error| source_failed(source, error))?
            .and_then(|task| task.version);
        if at(source.source(), found.as_deref(), &hold.version) {
            written
        } else {
            Err(moved_error(&id, &hold.version, found.unwrap_or_default()))
        }
    }
}

fn moved_error(id: &GlobalId, expected: &str, found: String) -> EngineError {
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
