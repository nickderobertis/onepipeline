//! Which tracker each source reads, named so a public record can point into a private one
//! without saying anything about it.
//!
//! A source's [`Designator`] is its plugin's answer to [`TaskSource::designator`] — the digest
//! of the backend's own opaque identifier of itself — or, for a backend with none,
//! `alias/<public_alias>` from its configuration. A plugin's answer costs at most one request,
//! and is asked once per host: it is kept in memory for the life of the engine and written to
//! `designator.json` in the directory the source's cache lives in (see
//! [`crate::cache::designator_location`]), held to the digest of the remote it was read from,
//! so a second process configuring the same remote reads it from there instead of asking.
//!
//! An opaque dependency endpoint is resolved by [`Engine::designated`]: the configured sources
//! whose plugin is the designator's prefix — or, for an alias, whose `public_alias` it is — in
//! name order, each asked for its designator until one is it.
//!
//! [`TaskSource::designator`]: onetaskgraph_plugin_api::TaskSource::designator

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use onetaskgraph_plugin_api::{Designator, SourceError, SourceName};
use serde::{Deserialize, Serialize};

use super::Engine;
use crate::config::Config;
use crate::plan::SourceFailure;

/// The schema of `designator.json`. A file in any other is a miss.
const FILE_SCHEMA: u32 = 1;

/// What each source's configuration says of its designator, and what this engine has learned.
#[derive(Debug, Default)]
pub(crate) struct Designators {
    configured: BTreeMap<SourceName, Configured>,
    known: Mutex<BTreeMap<SourceName, Option<Designator>>>,
}

#[derive(Debug, Clone, Default)]
struct Configured {
    /// `alias/<public_alias>`, when the configuration declares one.
    alias: Option<Designator>,
    /// Where a plugin's answer is kept on this host, and the digest of the remote it names.
    file: Option<(PathBuf, String)>,
}

/// `designator.json` as it is written.
#[derive(Serialize, Deserialize)]
struct File {
    schema: u32,
    remote: String,
    designator: Designator,
}

impl Designators {
    /// What `config` declares of each source's designator, with each plugin's answer kept under
    /// `home` unless a source's `cache.dir` says where.
    pub(crate) fn configured(config: &Config) -> Self {
        let home = config
            .cache_home()
            .map(Path::to_path_buf)
            .or_else(crate::cache::process_home);
        Self {
            configured: config
                .sources()
                .iter()
                .map(|(name, source)| {
                    (
                        name.clone(),
                        Configured {
                            alias: source.public_alias().cloned(),
                            file: crate::cache::designator_location(source, home.as_deref()),
                        },
                    )
                })
                .collect(),
            known: Mutex::new(BTreeMap::new()),
        }
    }

    fn known(&self, name: &SourceName) -> Option<Option<Designator>> {
        self.known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    fn learn(&self, name: &SourceName, designator: Option<Designator>) {
        self.known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.clone(), designator);
    }

    fn alias(&self, name: &SourceName) -> Option<&Designator> {
        self.configured.get(name)?.alias.as_ref()
    }
}

/// The designator `path` holds for `remote`, or `None` for a file that is absent, unreadable,
/// in another schema or about another remote — each a miss, never a failure.
fn read(path: &Path, remote: &str) -> Option<Designator> {
    let bytes = std::fs::read(path).ok()?;
    let file: File = serde_json::from_slice(&bytes).ok()?;
    (file.schema == FILE_SCHEMA && file.remote == remote).then_some(file.designator)
}

/// Keep `designator` at `path` for `remote`, whole or not at all: written to a staging file
/// beside it and renamed into place. A write that fails costs the next process one request.
fn write(path: &Path, remote: &str, designator: &Designator) {
    let Some(dir) = path.parent() else {
        return;
    };
    let file = File {
        schema: FILE_SCHEMA,
        remote: remote.to_owned(),
        designator: designator.clone(),
    };
    let Ok(bytes) = serde_json::to_vec(&file) else {
        return;
    };
    let staged = dir.join(format!(".designator.{}.tmp", std::process::id()));
    let written = std::fs::create_dir_all(dir).and_then(|()| {
        let mut staging = std::fs::File::create(&staged)?;
        staging.write_all(&bytes)?;
        staging.sync_all()?;
        std::fs::rename(&staged, path)
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
}

impl Engine {
    /// The designator of the source called `name`: its plugin's, asked at most once per host,
    /// or the one its `public_alias` declares, or `None` when it has neither.
    ///
    /// # Errors
    ///
    /// What the source answered when asked, and [`SourceError::Unavailable`] for a source that
    /// did not build — neither is remembered, so a later read asks again.
    pub(crate) async fn designator(
        &self,
        name: &SourceName,
    ) -> Result<Option<Designator>, SourceError> {
        if let Some(known) = self.designators.known(name) {
            return Ok(known);
        }
        let file = self
            .designators
            .configured
            .get(name)
            .and_then(|configured| configured.file.clone());
        if let Some((path, remote)) = &file
            && let Some(kept) = read(path, remote)
        {
            self.designators.learn(name, Some(kept.clone()));
            return Ok(Some(kept));
        }
        let Some(source) = self.ready_source(name) else {
            return Err(SourceError::Unavailable {
                message: format!("{name} did not start, so it could not say which tracker it is"),
            });
        };
        let answered = source.designator().await?;
        if let (Some(designator), Some((path, remote))) = (&answered, &file) {
            write(path, remote, designator);
        }
        let designator = answered.or_else(|| self.designators.alias(name).cloned());
        self.designators.learn(name, designator.clone());
        Ok(designator)
    }

    /// The designator a copy writes an edge to `far` under, opaquely: its source's, when this
    /// engine has learned it and `far`'s id is not plainly other than the item's native id. A
    /// copy has already refused, before writing, a far end its read found named otherwise.
    pub(crate) fn opaque_designator(&self, far: &crate::GlobalId) -> Option<Designator> {
        if super::boundary::plainly_not_native(far.native.as_str()).is_some() {
            return None;
        }
        self.known_designator(&far.source)
    }

    /// The designator of `name` when this engine has already learned it, without asking.
    pub(crate) fn known_designator(&self, name: &SourceName) -> Option<Designator> {
        self.designators.known(name).flatten()
    }

    /// The configured source `designator` names, and every candidate that could not say which
    /// tracker it is.
    ///
    /// The candidates are the sources whose plugin is the designator's prefix — or, for
    /// `alias/…`, whose `public_alias` it is — in name order, and the first whose designator is
    /// this one is the answer: no candidate after it is asked.
    pub(crate) async fn designated(
        &self,
        designator: &Designator,
    ) -> (Option<SourceName>, Vec<SourceFailure>) {
        let mut candidates: Vec<SourceName> = self
            .ready()
            .filter(|source| {
                if designator.is_alias() {
                    self.designators.alias(source.name()) == Some(designator)
                } else {
                    source.kind() == designator.prefix()
                }
            })
            .map(|source| source.name().clone())
            .collect();
        candidates.sort();
        let mut failed = Vec::new();
        for candidate in candidates {
            match self.designator(&candidate).await {
                Ok(Some(found)) if &found == designator => return (Some(candidate), failed),
                Ok(_) => {}
                Err(error) => failed.push(SourceFailure {
                    source: candidate,
                    error,
                }),
            }
        }
        (None, failed)
    }
}

/// What resolving one read's opaque far ends has learned, so each designator is looked for and
/// each far end read once per read.
#[derive(Debug, Default)]
pub(crate) struct Resolving {
    found: BTreeMap<Designator, Option<SourceName>>,
    /// Each far end read — by its source, its id and its kind, since a task and a project may
    /// share an id — and whether its read let it stand as an edge.
    read: std::collections::HashMap<
        (
            SourceName,
            onetaskgraph_plugin_api::NativeId,
            onetaskgraph_plugin_api::ItemKind,
        ),
        bool,
    >,
    /// Each designator no configured source is, and each candidate or far end that could not be
    /// asked.
    pub(crate) unanswered: Vec<crate::plan::Unanswered>,
    /// Each far end's source that answered its read with a failure.
    pub(crate) failed: Vec<SourceFailure>,
}

impl Resolving {
    /// What this read could not ask, as a read asking for every edge is answered: each source
    /// that could not be asked moved to `errors`, or the refusal of the first designator no
    /// configured source is.
    pub(crate) fn required(
        self,
        id: &str,
        errors: &mut Vec<SourceFailure>,
    ) -> Result<(), super::EngineError> {
        errors.extend(self.failed);
        for entry in self.unanswered {
            match entry {
                crate::plan::Unanswered::Source(failure) => errors.push(failure),
                crate::plan::Unanswered::Designator(unresolved) => {
                    return Err(super::EngineError::DesignatorNotConfigured {
                        id: id.to_owned(),
                        designator: unresolved.designator,
                    });
                }
            }
        }
        Ok(())
    }

    fn unasked(&mut self, failure: SourceFailure) {
        let entry = crate::plan::Unanswered::Source(failure);
        if !self.unanswered.contains(&entry) {
            self.unanswered.push(entry);
        }
    }
}

impl Engine {
    /// Read the far end `id` of `source` an opaque edge under `designator` names, and say whether
    /// it stands as an edge: an item answered under exactly that id does; one answered under
    /// another — a human key a backend also resolves — or no item at all is a far end this
    /// source does not hold by that native id, failed in `resolving.failed` rather than drawn.
    /// A source that could not be asked leaves the edge standing, its qualified id known, and
    /// is named in `resolving.unanswered`; one that answered with a failure fails the read.
    async fn names_natively(
        &self,
        source: &SourceName,
        id: &str,
        kind: onetaskgraph_plugin_api::ItemKind,
        designator: &Designator,
        resolving: &mut Resolving,
    ) -> bool {
        let Some(far) = self.ready_source(source) else {
            return true;
        };
        let native = onetaskgraph_plugin_api::NativeId(id.to_owned());
        let read = match kind {
            onetaskgraph_plugin_api::ItemKind::Task => far
                .get_task(&native)
                .await
                .map(|task| task.map(|task| task.id)),
            onetaskgraph_plugin_api::ItemKind::Project => far
                .get_project(&native)
                .await
                .map(|project| project.map(|project| project.id)),
        };
        let held = match read {
            Ok(held) => held,
            Err(error) => {
                let failure = SourceFailure {
                    source: source.clone(),
                    error,
                };
                if super::dependents::could_not_answer(&failure.error) {
                    resolving.unasked(failure);
                    return true;
                }
                resolving.failed.push(failure);
                return false;
            }
        };
        if held.as_ref() == Some(&native) {
            return true;
        }
        let noun = match kind {
            onetaskgraph_plugin_api::ItemKind::Task => "task",
            onetaskgraph_plugin_api::ItemKind::Project => "project",
        };
        resolving.failed.push(SourceFailure {
            source: source.clone(),
            error: SourceError::Malformed {
                message: format!(
                    "a dependency recorded under designator {designator} names {noun} {id}, and \
                     this source holds no {noun} whose native id that is"
                ),
            },
        });
        false
    }

    /// `endpoint` as this configuration names it: an opaque one qualified to the source its
    /// designator is, once the far end has been read there by its id and found under exactly
    /// that id, and `None` when no configured source is that tracker — named in
    /// `resolving.unanswered` — or that source holds no item by that native id — failed in
    /// `resolving.failed`; never dropped silently. Every other endpoint is answered as it is.
    pub(crate) async fn resolve_endpoint(
        &self,
        endpoint: onetaskgraph_plugin_api::DependencyEndpoint,
        resolving: &mut Resolving,
    ) -> Option<onetaskgraph_plugin_api::DependencyEndpoint> {
        let Some(designator) = endpoint.designator().cloned() else {
            return Some(endpoint);
        };
        let source = match resolving.found.get(&designator) {
            Some(found) => found.clone(),
            None => {
                let (found, failed) = self.designated(&designator).await;
                for failure in failed {
                    resolving.unasked(failure);
                }
                if found.is_none() {
                    resolving
                        .unanswered
                        .push(crate::plan::Unanswered::Designator(
                            crate::plan::UnresolvedDesignator {
                                designator: designator.clone(),
                                reason: crate::plan::UnresolvedReason::NotConfigured,
                            },
                        ));
                }
                resolving.found.insert(designator.clone(), found.clone());
                found
            }
        }?;
        let id = endpoint.id().to_owned();
        let key = (
            source.clone(),
            onetaskgraph_plugin_api::NativeId(id.clone()),
            endpoint.kind,
        );
        let named = match resolving.read.get(&key) {
            Some(named) => *named,
            None => {
                let named = self
                    .names_natively(&source, &id, endpoint.kind, &designator, resolving)
                    .await;
                resolving.read.insert(key, named);
                named
            }
        };
        if !named {
            return None;
        }
        onetaskgraph_plugin_api::DependencyEndpoint::new(format!("{source}:{id}"), endpoint.kind)
            .ok()
    }
}
