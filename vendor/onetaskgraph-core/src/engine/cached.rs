//! What the engine says about, and asks of, the local cache of each source configured with
//! one — see [`crate::cache`] for the rules it keeps.

use onetaskgraph_plugin_api::{NativeId, SourceName};

use super::{Engine, EngineError, Spent};
use crate::GlobalId;
use crate::cache::{CacheUse, ExpectedWrite, SourceCache, WriteFallback, WriteResult};

impl Engine {
    /// The cache of the source called `name`, when it keeps one.
    pub(crate) fn cache_of(&self, name: &SourceName) -> Option<&SourceCache> {
        self.caches.get(name).map(std::convert::AsRef::as_ref)
    }

    /// Whether the source called `name` keeps a local cache.
    #[must_use]
    pub fn caches(&self, name: &SourceName) -> bool {
        self.caches.contains_key(name)
    }

    /// Pin the reads of `id` this command makes to `version`: a read is answered from the
    /// cache, with no request, when it holds the task at that version or a later one, and is
    /// read from the source otherwise.
    ///
    /// A pin means *at least as fresh as the version the caller holds*, not *current*: a read
    /// it answers from the cache may predate a change at the source nobody has read yet. A pin
    /// on a source keeping no cache changes nothing — its reads go to the source, which is
    /// always at least that fresh.
    pub fn pin(&self, id: &GlobalId, version: &str) {
        if let Some(cache) = self.cache_of(&id.source) {
            cache.pin(&id.native, version);
        }
    }

    /// Pin the reads of project `id` this command makes to `version`, on exactly the terms
    /// [`Engine::pin`] pins a task's: answered from the cache, with no request, when it holds the
    /// project at that version or a later one and its source's token covers what is read.
    pub fn pin_project(&self, id: &GlobalId, version: &str) {
        if let Some(cache) = self.cache_of(&id.source) {
            cache.pin_project(&id.native, version);
        }
    }

    /// Pin the reads of document `id` this command makes — its fields, the manifest of its
    /// image assets and their bytes — to `version`, on exactly the terms [`Engine::pin`] pins a
    /// task's.
    pub fn pin_document(&self, id: &GlobalId, version: &str) {
        if let Some(cache) = self.cache_of(&id.source) {
            cache.pin_document(&id.native, version);
        }
    }

    /// How the last read of `id` in this command used its source's cache, or `None` for a
    /// source keeping none.
    #[must_use]
    pub fn cache_use(&self, id: &GlobalId) -> Option<CacheUse> {
        self.cache_of(&id.source)?.used(&id.native)
    }

    /// Expect the next write of `id` to find it at `version`: the write takes its target's ids
    /// and current values from the cache, at that version or a later one, instead of reading
    /// it, and sends only its mutations — falling back to one read when the cache does not hold
    /// what it needs. [`Engine::take_expected`] answers what it came to.
    ///
    /// No backend this build writes to accepts a precondition, so this is a contract about cost
    /// and ids, never a conditional write: a person's edit to a field the write sets, made
    /// after the caller read `version`, is overwritten.
    ///
    /// # Errors
    ///
    /// [`EngineError::NotCached`] for a source keeping no cache, and
    /// [`EngineError::UnknownSource`] for one nothing configures.
    pub fn expect(&self, id: &GlobalId, version: &str) -> Result<(), EngineError> {
        let name = self.known(&id.source)?;
        let cache = self.cache_of(&name).ok_or_else(|| EngineError::NotCached {
            name: name.to_string(),
        })?;
        cache.expect(&id.native, version);
        Ok(())
    }

    /// Hold `request`'s `--expect` for the length of the copy, refusing one it cannot take.
    pub(crate) fn expecting_copy(
        &self,
        request: &super::CopyRequest,
    ) -> Result<CopyExpecting<'_>, EngineError> {
        let Some(version) = &request.expect else {
            return Ok(CopyExpecting(None));
        };
        if request.items.query().is_some()
            || request.items.as_slice().len() != 1
            || request.scope != super::CopyScope::Tasks
        {
            return Err(EngineError::ExpectOneTask);
        }
        let name = self.known(&request.destination)?;
        if !self.caches(&name) {
            return Err(EngineError::NotCached {
                name: name.to_string(),
            });
        }
        *self.held_copy_expect() = Some(version.clone());
        Ok(CopyExpecting(Some(self)))
    }

    /// Whether the copy running now expects a version of its one target, so its read is the
    /// cache's to answer and nothing is read ahead of it.
    pub(crate) fn expects_copy(&self) -> bool {
        self.held_copy_expect().is_some()
    }

    fn held_copy_expect(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.copy_expect
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Expect the write of the destination task `id` a running `task copy --expect` reads,
    /// at the version it names — once, for the first task it reads.
    pub(crate) fn expect_copied(&self, destination: &SourceName, id: &NativeId) {
        let Some(version) = self.held_copy_expect().take() else {
            return;
        };
        if let Some(cache) = self.cache_of(destination) {
            cache.expect(id, &version);
        }
    }

    /// What the last expected write of any source came to, when nothing took it — a write
    /// that failed, whose answer is a failure rather than the verb's own.
    #[must_use]
    pub fn take_last_expected(&self) -> Option<ExpectedWrite> {
        self.caches
            .values()
            .filter_map(|cache| cache.last_made().map(|made| (made, cache)))
            .max_by_key(|(made, _)| *made)
            .and_then(|(_, cache)| cache.take_last())
    }

    /// [`take_expected`](Self::take_expected), as the three members a write's answer carries.
    pub(crate) fn expected_parts(
        &self,
        id: &GlobalId,
    ) -> (Option<WriteResult>, Option<WriteFallback>, Option<Spent>) {
        match self.take_expected(id) {
            Some(expected) => (expected.result, expected.fallback, expected.spent),
            None => (None, None, None),
        }
    }

    /// What the task `id` holds after a write verb wrote it, read off the write's own
    /// responses with no request added: the source's own answer of what it holds after its
    /// write where it gives one, else `known` — the task the verb's write answered with — at no
    /// version, beside what an expected write recorded of its parts.
    pub(crate) async fn verb_result(
        &self,
        source: &crate::resolve::ResolvedSource,
        id: &GlobalId,
        known: Option<&onetaskgraph_plugin_api::Task>,
        expected: Option<WriteResult>,
    ) -> Option<WriteResult> {
        let result = match (source.source().written_task(&id.native).await, known) {
            (Some(task), _) => WriteResult::of_task(&task, task.version.clone()),
            (None, Some(task)) => WriteResult::of_task(task, None),
            (None, None) => WriteResult {
                version: None,
                parts: None,
                status: None,
                priority: None,
            },
        };
        Some(match expected {
            Some(expected) => expected.merged(result),
            None => result,
        })
    }

    /// What the write [`Engine::expect`] expected of `id` came to, ending the expectation.
    #[must_use]
    pub fn take_expected(&self, id: &GlobalId) -> Option<ExpectedWrite> {
        self.cache_of(&id.source)?.take_expected(&id.native)
    }
}

/// A running `task copy --expect`, which lets go of its version however the copy ends.
pub(crate) struct CopyExpecting<'a>(Option<&'a Engine>);

impl Drop for CopyExpecting<'_> {
    fn drop(&mut self) {
        if let Some(engine) = self.0 {
            *engine.held_copy_expect() = None;
        }
    }
}
