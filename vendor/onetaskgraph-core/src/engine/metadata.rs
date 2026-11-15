//! One key of one record's metadata, set on its own.
//!
//! The three verbs — `task`, `project` and `document metadata set` — share one answer and one
//! path through the engine, which differ only in which of the source's three narrow writes is
//! called and what a missing record is called. Nothing here reads the record first: the
//! source's own write says whether it holds the record, and the record it answers with is
//! what the answer is read off, so what is reported is what the source now holds rather than
//! what it was handed.
//!
//! Metadata is not status, so no delivered task is re-evaluated after one of these writes.

use onetaskgraph_plugin_api::{Location, MetadataKey, MetadataRecord, SourceError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Spent;
use super::boundary::{Exposure, Held};
use super::delivery::source_failed;
use super::{Engine, EngineError};
use crate::GlobalId;
use crate::cache::{WriteFallback, WriteResult};
use crate::resolve::ResolvedSource;

/// What `task`, `project` and `document metadata set` answer with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MetadataSet {
    /// The record whose metadata key was set.
    pub id: GlobalId,
    /// The key that was set.
    pub key: MetadataKey,
    /// The value the source holds under `key` as it reads the record back after the write —
    /// which is not always the value it was handed.
    pub value: Value,
    /// Where the source reports the record to be, left out when it does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
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

impl Engine {
    /// Set one key of one task's metadata, and nothing else about it.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownSource`] for a source nothing configures,
    /// [`EngineError::MetadataNotWritable`] for one with no write side,
    /// [`EngineError::NoSuchTask`] when the task is not there, and
    /// [`EngineError::SourceFailed`] when the source refuses — a plugin that cannot write one
    /// key on its own included.
    pub async fn set_task_metadata(
        &self,
        id: &GlobalId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<MetadataSet, EngineError> {
        let source = self.metadata_writable(id, MetadataRecord::Task)?;
        let rewrite = self
            .admit_existing(
                source,
                &id.to_string(),
                &id.native,
                Held::Task,
                &Exposure::of_entry(key.as_str(), value),
            )
            .await?;
        let mut value = value.clone();
        rewrite.value(&mut value);
        let value = &value;
        let task = source
            .source()
            .set_task_metadata(&id.native, key, value)
            .await
            .map_err(|error| source_failed(source, error))?
            .ok_or_else(|| EngineError::NoSuchTask { id: id.to_string() })?;
        let mut set = answer(source, id, key, &task.metadata, task.location.clone())?;
        (set.result, set.fallback, set.spent) = self.expected_parts(id);
        set.result = self.verb_result(source, id, Some(&task), set.result).await;
        Ok(set)
    }

    /// Set one key of one project's metadata, and nothing else about it.
    ///
    /// # Errors
    ///
    /// As [`set_task_metadata`](Self::set_task_metadata), with
    /// [`EngineError::NoSuchProject`] when the project is not there.
    pub async fn set_project_metadata(
        &self,
        id: &GlobalId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<MetadataSet, EngineError> {
        let source = self.metadata_writable(id, MetadataRecord::Project)?;
        let rewrite = self
            .admit_existing(
                source,
                &id.to_string(),
                &id.native,
                Held::Project,
                &Exposure::of_entry(key.as_str(), value),
            )
            .await?;
        let mut value = value.clone();
        rewrite.value(&mut value);
        let value = &value;
        let project = source
            .source()
            .set_project_metadata(&id.native, key, value)
            .await
            .map_err(|error| source_failed(source, error))?
            .ok_or_else(|| EngineError::NoSuchProject { id: id.to_string() })?;
        answer(source, id, key, &project.metadata, project.location)
    }

    /// Set one key of one document's metadata, and nothing else about it.
    ///
    /// # Errors
    ///
    /// As [`set_task_metadata`](Self::set_task_metadata), with [`EngineError::NoDocuments`]
    /// for a source declaring it has none — which is never asked — and
    /// [`EngineError::NoSuchDocument`] when the document is not there.
    pub async fn set_document_metadata(
        &self,
        id: &GlobalId,
        key: &MetadataKey,
        value: &Value,
    ) -> Result<MetadataSet, EngineError> {
        self.write_document_metadata(id, key, value, None).await
    }

    /// [`set_document_metadata`](Self::set_document_metadata), only while the document is at
    /// `version`: the document is read first, and a read answering any other version — a
    /// change made at the source since the caller read it, whether or not anything here has
    /// seen it — refuses the write with nothing sent.
    ///
    /// The read is one request more than the write alone, and it is made because no backend this
    /// build writes to accepts a precondition: GitHub's `UpdateIssueInput` takes no expected
    /// version, and its REST API supports no conditional write. What the read answered is what
    /// the write then takes instead of reading again, so the whole is that read and the write.
    /// A change landing between the two — the time one request takes — is still overwritten.
    ///
    /// # Errors
    ///
    /// As [`set_document_metadata`](Self::set_document_metadata), and
    /// [`EngineError::VersionMoved`] when the document is not at `version`.
    pub async fn set_document_metadata_at(
        &self,
        id: &GlobalId,
        key: &MetadataKey,
        value: &Value,
        version: &str,
    ) -> Result<MetadataSet, EngineError> {
        self.write_document_metadata(id, key, value, Some(version))
            .await
    }

    async fn write_document_metadata(
        &self,
        id: &GlobalId,
        key: &MetadataKey,
        value: &Value,
        version: Option<&str>,
    ) -> Result<MetadataSet, EngineError> {
        let source = self.metadata_writable(id, MetadataRecord::Document)?;
        // A write held to a version reads the document once, and who may read it is taken from
        // that same read rather than from a second one.
        let mut stored = None;
        if let Some(version) = version {
            let read = source
                .source()
                .get_document(&id.native)
                .await
                .map_err(|error| source_failed(source, error))?
                .ok_or_else(|| EngineError::NoSuchDocument { id: id.to_string() })?;
            let at = read.version.unwrap_or_default();
            if source.source().order_versions(&at, version) != Some(std::cmp::Ordering::Equal) {
                return Err(EngineError::VersionMoved {
                    id: id.to_string(),
                    expected: version.to_owned(),
                    held: Box::new(crate::cache::WriteResult {
                        version: Some(at.clone()),
                        parts: None,
                        status: None,
                        priority: None,
                    }),
                    found: at,
                });
            }
            stored = Some((read.classification, read.repositories, read.project));
        }
        self.write_read_document(source, id, key, value, stored)
            .await
    }

    /// The write half of [`write_document_metadata`](Self::write_document_metadata): admitted
    /// on `stored` — what a read just found of the document — when there is one, and the
    /// source's write primed with that read so it does not read again.
    async fn write_read_document(
        &self,
        source: &ResolvedSource,
        id: &GlobalId,
        key: &MetadataKey,
        value: &Value,
        stored: Option<(
            onetaskgraph_plugin_api::Classification,
            Vec<onetaskgraph_plugin_api::Repository>,
            Option<onetaskgraph_plugin_api::NativeId>,
        )>,
    ) -> Result<MetadataSet, EngineError> {
        let read = stored.is_some();
        let exposure = Exposure::of_entry(key.as_str(), value);
        let rewrite = match stored {
            Some(stored) => {
                self.admit_read(
                    source,
                    &id.to_string(),
                    &id.native,
                    Held::Document,
                    stored,
                    &exposure,
                )
                .await?
            }
            None => {
                self.admit_existing(
                    source,
                    &id.to_string(),
                    &id.native,
                    Held::Document,
                    &exposure,
                )
                .await?
            }
        };
        let mut value = value.clone();
        rewrite.value(&mut value);
        let value = &value;
        // The write takes what the read above left, rather than reading the document again.
        let primed = read
            && source
                .source()
                .write_context(&id.native)
                .is_some_and(|context| source.source().prime_write(&id.native, &context, None));
        let written = source
            .source()
            .set_document_metadata(&id.native, key, value)
            .await;
        if primed {
            source.source().unprime(&id.native);
        }
        let document = written
            .map_err(|error| source_failed(source, error))?
            .ok_or_else(|| EngineError::NoSuchDocument { id: id.to_string() })?;
        answer(source, id, key, &document.metadata, document.location)
    }

    /// Set one key of the document `read` is, as that read found it: who may read it is taken
    /// from the read, and the source's write takes what the read left rather than reading
    /// again — so the whole is that read and this write.
    pub(super) async fn set_read_document_metadata(
        &self,
        source: &ResolvedSource,
        id: &GlobalId,
        key: &MetadataKey,
        value: &Value,
        read: &onetaskgraph_plugin_api::Document,
    ) -> Result<MetadataSet, EngineError> {
        self.write_read_document(
            source,
            id,
            key,
            value,
            Some((
                read.classification,
                read.repositories.clone(),
                read.project.clone(),
            )),
        )
        .await
    }

    /// The built source `id` names, when a metadata write of `record` may be sent to it.
    pub(super) fn metadata_writable(
        &self,
        id: &GlobalId,
        record: MetadataRecord,
    ) -> Result<&ResolvedSource, EngineError> {
        let source = self.built(&id.source)?;
        if !source.source().writes().is_supported() {
            return Err(EngineError::MetadataNotWritable {
                name: source.name().to_string(),
                kind: source.kind().to_owned(),
                record,
            });
        }
        if record == MetadataRecord::Document
            && !source.source().capabilities().documents.is_native()
        {
            return Err(EngineError::NoDocuments {
                name: source.name().to_string(),
                kind: source.kind().to_owned(),
            });
        }
        Ok(source)
    }
}

/// The answer, read off the record the source answered the write with.
///
/// A record that does not hold the key it was just written under is not an answer this can
/// report a value from, so it is refused as malformed rather than reported as `null`.
fn answer(
    source: &ResolvedSource,
    id: &GlobalId,
    key: &MetadataKey,
    metadata: &std::collections::BTreeMap<String, Value>,
    location: Option<Location>,
) -> Result<MetadataSet, EngineError> {
    let value = metadata.get(key.as_str()).cloned().ok_or_else(|| {
        source_failed(
            source,
            SourceError::Malformed {
                message: format!(
                    "the record {id} it answered the write with does not hold the key {key} it \
                     was just written under"
                ),
            },
        )
    })?;
    Ok(MetadataSet {
        id: id.clone(),
        key: key.clone(),
        value,
        location,
        result: None,
        fallback: None,
        spent: None,
    })
}
