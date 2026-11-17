//! A create that is safe to retry.
//!
//! Every create a copy makes is preceded by an **intent** recorded on the item being copied,
//! at [`MetadataKey::CREATING_KEY`]: the create's deterministic key, when it was first taken,
//! which copy attempt holds it and until when. The created item carries the same key, at
//! [`MetadataKey::CREATE_KEY`] — or as its own id, on a source that takes an id of the caller's
//! choosing. The intent is removed in the write that records the item's
//! [`MetadataKey::COPIES_KEY`] link, so an item with no create in flight carries none.
//!
//! A copy that finds an intent for its destination resolves it before every other rule: it asks
//! the destination for the item carrying the key ([`TaskSource::created_with_key`]) and, found,
//! finishes and links that one instead of creating a second. A destination whose lookup by key
//! lags a create is given until the source's quiescence window after the intent was first taken
//! to list it, and only then is created in again, with the same key.
//!
//! Who owns an intent depends on the copy's source. An item whose source gives a hold on it
//! through `TaskSource::hold_item` — a folder of Markdown's exclusive lock file beside it — is
//! taken, renewed and released under that hold, so exactly one attempt on one host owns a create.
//! A source that gives none, as a hosted one does, leaves its intent advisory:
//! a live lease another attempt holds still refuses, but two attempts that read before either
//! wrote both proceed, and a destination whose lookup lags is listed again by key after the
//! create so a duplicate is reported rather than linked.
//!
//! [`TaskSource::created_with_key`]: onetaskgraph_plugin_api::TaskSource::created_with_key

use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, Utc};
use onetaskgraph_plugin_api::{MetadataKey, MetadataRecord, NativeId, SourceError, SourceName};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::GlobalId;
use crate::resolve::ResolvedSource;

use super::super::{CreateHolder, Engine, EngineError};
use super::{Level, refused, set_link_versioned};

/// The namespace every create key is a version-5 UUID under: the UUID this store names its
/// creates by, documented in the README so another implementation computes the same keys.
pub const NAMESPACE: uuid::Uuid = uuid::Uuid::from_u128(0x7a1c_3f0e_5b2d_4c8a_9e61_0f4d_2b7c_5a39);

/// How far before an intent's `at` a lookup by key reaches, for a destination clock that runs
/// behind this host's.
pub const CLOCK_SKEW: TimeDelta = TimeDelta::seconds(300);

/// The key of the create that copies `source` into `destination`, replacing the link `replaces`
/// names when it replaces one: the same for every attempt at that create, and a different one
/// for a `--recreate` after a stale link. A project's and a document's name their kind as well,
/// so a task and a project one source spells with one id never share a key.
#[must_use]
pub(in crate::engine) fn key(
    level: Level,
    source: &GlobalId,
    destination: &SourceName,
    replaces: Option<&GlobalId>,
) -> CreateKey {
    let mut name = format!(
        "{source}|{destination}|{}",
        replaces.map(ToString::to_string).unwrap_or_default()
    );
    match level {
        Level::Task => {}
        Level::Project => name.push_str("|project"),
        Level::Document => name.push_str("|document"),
    }
    CreateKey(uuid::Uuid::new_v5(&NAMESPACE, name.as_bytes()))
}

/// A create's key: a version-5 UUID of RFC 4122's variant, as [`key`] computes one, and read
/// back from an intent only as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "uuid::Uuid", into = "uuid::Uuid")]
pub(crate) struct CreateKey(uuid::Uuid);

impl TryFrom<uuid::Uuid> for CreateKey {
    type Error = String;

    fn try_from(uuid: uuid::Uuid) -> Result<Self, Self::Error> {
        of_version(uuid, 5).map(Self)
    }
}

impl From<CreateKey> for uuid::Uuid {
    fn from(key: CreateKey) -> Self {
        key.0
    }
}

/// One engine's name for the copy attempts it makes: a version-4 UUID of RFC 4122's variant,
/// and read back from an intent only as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "uuid::Uuid", into = "uuid::Uuid")]
pub struct Attempt(uuid::Uuid);

impl TryFrom<uuid::Uuid> for Attempt {
    type Error = String;

    fn try_from(uuid: uuid::Uuid) -> Result<Self, Self::Error> {
        of_version(uuid, 4).map(Self)
    }
}

impl From<Attempt> for uuid::Uuid {
    fn from(attempt: Attempt) -> Self {
        attempt.0
    }
}

/// `uuid`, when it is of RFC 4122's variant and of `version`.
fn of_version(uuid: uuid::Uuid, version: usize) -> Result<uuid::Uuid, String> {
    if uuid.get_variant() == uuid::Variant::RFC4122 && uuid.get_version_num() == version {
        Ok(uuid)
    } else {
        Err(format!("{uuid} is not a version-{version} RFC 4122 UUID"))
    }
}

/// One destination's entry of an item's `onetaskgraph.creating`, each member read as what it
/// is: a key and an attempt that are UUIDs, and two RFC 3339 UTC times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Intent {
    /// The create's key.
    pub(crate) key: CreateKey,
    /// When the key was first taken, to the second.
    #[serde(with = "seconds")]
    pub(crate) at: DateTime<Utc>,
    /// The copy attempt holding it.
    pub(crate) attempt: Attempt,
    /// Until when it holds it, to the second.
    #[serde(with = "seconds")]
    pub(crate) lease_until: DateTime<Utc>,
}

impl Intent {
    fn at(&self) -> DateTime<Utc> {
        self.at
    }

    /// How far back a lookup by key reaches: `CLOCK_SKEW` before the intent was first taken.
    fn since(&self) -> Option<DateTime<Utc>> {
        self.at.checked_sub_signed(CLOCK_SKEW)
    }

    /// The key as the created item carries it.
    pub(crate) fn key(&self) -> String {
        self.key.0.hyphenated().to_string()
    }

    /// Whether another attempt than `attempt` holds this intent's lease at `now`.
    fn held_by_another(&self, attempt: Attempt, now: DateTime<Utc>) -> bool {
        self.attempt != attempt && self.lease_until > now
    }
}

/// What a write of a lease is for: renewing it before a destination write, which an intent no
/// longer this attempt's or an item gone refuses, or giving it up, which they leave alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaseChange {
    Renew,
    Release,
}

/// What resolving an earlier attempt's intent may do beyond reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::engine) enum Resolving {
    /// Read only: a dry run and `task locate`, which never wait or write.
    Reads,
    /// Wait out the destination's quiescence window, writing nothing to the copied item: a
    /// `--no-link` copy.
    Waits,
    /// Wait, having first taken the intent over under a lease that outlasts the wait.
    WaitsLeased,
}

/// What taking an intent came to.
pub(in crate::engine) enum Taken {
    /// The intent is recorded, and `exclusive` when the source's hold made it this host's alone.
    Held {
        /// The intent as recorded.
        intent: Intent,
        /// Whether the source gave a hold on the item, so the decision under it was exclusive.
        exclusive: bool,
    },
    /// The source will not keep the key, or no longer holds the record: the copy goes on as it
    /// always did, with no intent.
    Unkept,
}

/// A fresh version-4 UUID naming one engine's copy attempts.
///
/// Its random bits come from the standard library's per-process randomly keyed hasher, mixed
/// with this process's id and the time, rather than from a random-number crate this workspace
/// would otherwise carry twice: an attempt only has to differ from every other process's.
#[must_use]
pub fn attempt() -> Attempt {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    for (index, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_usize(index);
        hasher.write_u32(std::process::id());
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos()),
        );
        chunk.copy_from_slice(&hasher.finish().to_le_bytes());
    }
    Attempt(uuid::Builder::from_random_bytes(bytes).into_uuid())
}

/// An intent's times as RFC 3339 UTC to the second, and read back only as that.
mod seconds {
    use chrono::{DateTime, SecondsFormat, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(
        when: &DateTime<Utc>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&when.to_rfc3339_opts(SecondsFormat::Secs, true))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DateTime<Utc>, D::Error> {
        let spelled = String::deserialize(deserializer)?;
        let when = DateTime::parse_from_rfc3339(&spelled)
            .map(|when| when.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)?;
        // A time no copy could have written: refused, so no lease or window is reckoned from
        // the edge of what a time can hold.
        if !(1970..=9999).contains(&chrono::Datelike::year(&when)) {
            return Err(serde::de::Error::custom(format!(
                "{spelled} is outside the years 1970 to 9999"
            )));
        }
        Ok(when)
    }
}

/// Now, to the second, as an intent records a time.
fn now() -> DateTime<Utc> {
    let now = Utc::now();
    DateTime::from_timestamp(now.timestamp(), 0).unwrap_or(now)
}

/// The intent `metadata` records for `destination`, or why the entry there is not one: a
/// malformed entry is refused rather than read as none, since reading it as none would let a
/// second create through.
pub(crate) fn held(
    metadata: &BTreeMap<String, Value>,
    destination: &SourceName,
) -> Result<Option<Intent>, String> {
    let Some(creating) = metadata.get(MetadataKey::CREATING_KEY) else {
        return Ok(None);
    };
    let Some(entries) = creating.as_object() else {
        return Err(format!(
            "its {} is not an object of intents by destination",
            MetadataKey::CREATING_KEY
        ));
    };
    let Some(entry) = entries.get(destination.as_str()) else {
        return Ok(None);
    };
    serde_json::from_value(entry.clone())
        .map(Some)
        .map_err(|error| {
            format!(
                "its {} entry for {destination} is not an intent to create — {error}",
                MetadataKey::CREATING_KEY
            )
        })
}

/// `onetaskgraph.creating` as `held` holds it, with `destination`'s entry replaced by `intent`
/// — or removed, for `None` — and `None` when nothing is left in it.
pub(crate) fn replaced(
    held: Option<&Value>,
    destination: &SourceName,
    intent: Option<&Intent>,
) -> Option<Value> {
    let mut entries = held.and_then(Value::as_object).cloned().unwrap_or_default();
    match intent {
        Some(intent) => {
            entries.insert(
                destination.to_string(),
                serde_json::to_value(intent).unwrap_or(Value::Null),
            );
        }
        None => {
            entries.remove(destination.as_str());
        }
    }
    (!entries.is_empty()).then_some(Value::Object(entries))
}

/// A source's hold on one item, released when dropped — on every path out of the decision it
/// guards. Taken through [`TaskSource::hold_item`], so how an item is held is its source's:
/// a folder of Markdown takes a lock file beside the item's own.
///
/// [`TaskSource::hold_item`]: onetaskgraph_plugin_api::TaskSource::hold_item
pub(crate) struct Hold<'a> {
    source: &'a ResolvedSource,
    record: MetadataRecord,
    id: NativeId,
    held: bool,
}

impl Hold<'_> {
    /// Whether the source offered a hold, so the decision under it is exclusive on this host.
    pub(crate) fn held(&self) -> bool {
        self.held
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        if self.held {
            self.source.source().release_item(self.record, &self.id);
        }
    }
}

/// Take `source`'s hold on the `level` record `id`.
fn hold<'a>(
    source: &'a ResolvedSource,
    level: Level,
    id: &NativeId,
) -> Result<Hold<'a>, EngineError> {
    let held = source
        .source()
        .hold_item(level.record(), id)
        .map_err(|error| refused(source, error))?;
    Ok(Hold {
        source,
        record: level.record(),
        id: id.clone(),
        held,
    })
}

/// The refusal of a `level` record `id` its source no longer holds.
fn missing(level: Level, id: &GlobalId) -> EngineError {
    let id = id.to_string();
    match level {
        Level::Task => EngineError::NoSuchTask { id },
        Level::Project => EngineError::NoSuchProject { id },
        Level::Document => EngineError::NoSuchDocument { id },
    }
}

/// The metadata of the `level` record `id` as `source` now holds it, or `None` when it holds
/// no such record.
async fn metadata_of(
    source: &ResolvedSource,
    level: Level,
    id: &NativeId,
) -> Result<Option<BTreeMap<String, Value>>, SourceError> {
    Ok(match level {
        Level::Task => source
            .source()
            .get_task(id)
            .await?
            .map(|task| task.metadata),
        Level::Project => source
            .source()
            .get_project(id)
            .await?
            .map(|project| project.metadata),
        Level::Document => source
            .source()
            .get_document(id)
            .await?
            .map(|document| document.metadata),
    })
}

/// `metadata` with `destination`'s intent taken off `onetaskgraph.creating`, written to the
/// `level` record `id` — the remaining intents set, or the key removed once none is left — and
/// the record's version token after it.
async fn write_without(
    engine: &Engine,
    source: &ResolvedSource,
    level: Level,
    id: &NativeId,
    metadata: &BTreeMap<String, Value>,
    destination: &SourceName,
) -> Result<Option<Option<String>>, SourceError> {
    match replaced(metadata.get(MetadataKey::CREATING_KEY), destination, None) {
        Some(rest) => set_link_versioned(source, level, id, &MetadataKey::creating(), &rest).await,
        None => {
            match source
                .source()
                .remove_record_metadata(level.record(), id, &MetadataKey::creating())
                .await
            {
                // A source that cannot take one key off on its own has the record written back
                // whole without it, as a link a copy takes back is.
                Err(SourceError::Refused { .. }) if level != Level::Task => {
                    engine
                        .without_key(source, level, id, &MetadataKey::creating())
                        .await
                }
                answered => answered,
            }
        }
    }
}

/// The test-only point a copy stops at by exiting, as a process killed there would:
/// `OTG_TEST_STOP_AT` naming `after-intent` or `before-link`. Unset, as everywhere but
/// the journeys that prove a retry, it does nothing.
pub(crate) fn stop_at(point: &str) {
    if std::env::var("OTG_TEST_STOP_AT").is_ok_and(|named| named == point) {
        std::process::exit(86);
    }
}

/// The test-only points a copy waits at for a journey to let it go on, so two processes can be
/// interleaved exactly as a race would interleave them: `OTG_TEST_PAUSE_AT` naming
/// `<point>=<directory>` pairs, comma-separated. The points are `before-intent`, `intent-read`,
/// `after-create`, `guarded-write` — before the renewal a write request asks for inside a plugin
/// call that owns a create, and `guarded-write-<n>` before the one numbered `n` from 0 — and `before-comment-<n>`, before the comment write numbered `n`
/// from 0. At a named point the copy creates `<directory>/<point>.paused` and waits until
/// `<directory>/<point>.go` exists, a minute at most.
/// Unset, as everywhere but those journeys, it does nothing.
pub(crate) fn pause_at(point: &str) {
    let Ok(spec) = std::env::var("OTG_TEST_PAUSE_AT") else {
        return;
    };
    for entry in spec.split(',') {
        let Some((named, directory)) = entry.split_once('=') else {
            continue;
        };
        if named != point {
            continue;
        }
        let directory = std::path::Path::new(directory);
        let _ = std::fs::write(directory.join(format!("{point}.paused")), b"");
        let started = std::time::Instant::now();
        while !directory.join(format!("{point}.go")).exists()
            && started.elapsed() < std::time::Duration::from_secs(60)
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// What a copy taking an intent expects the item's link to the destination to be, so it can
/// refuse a create another copy finished and linked since this one planned it.
#[derive(Debug, Clone, Copy)]
pub(in crate::engine) enum PlannedLink<'a> {
    /// Not checked: a renewal before a wait, which planned nothing of the link.
    Unchecked,
    /// Planned with no link to the destination.
    Absent,
    /// Planned replacing this link — a `--recreate` after a stale one.
    Replacing(&'a GlobalId),
}

impl<'a> PlannedLink<'a> {
    /// The link planned from, or `None` when nothing is checked.
    fn expected(self) -> Option<Option<&'a GlobalId>> {
        match self {
            Self::Unchecked => None,
            Self::Absent => Some(None),
            Self::Replacing(link) => Some(Some(link)),
        }
    }
}

impl Engine {
    /// The lease and quiescence `source` is configured with — the defaults where it sets
    /// neither, since a source with no block of its own is held to them too.
    fn create_settings(&self, source: &SourceName) -> crate::config::CreateSettings {
        self.creates.get(source).copied().unwrap_or_default()
    }

    /// The metadata of the `level` record `id` as its source holds it now, read under the
    /// source's hold on it when it offers one — the read an intent is decided on.
    async fn locked_read<'a>(
        &self,
        source: &'a ResolvedSource,
        level: Level,
        id: &GlobalId,
    ) -> Result<(BTreeMap<String, Value>, Hold<'a>), EngineError> {
        let hold = hold(source, level, &id.native)?;
        let metadata = metadata_of(source, level, &id.native)
            .await
            .map_err(|error| refused(source, error))?
            .ok_or_else(|| missing(level, id))?;
        Ok((metadata, hold))
    }

    /// Whether `intent`, written to `id` under an answer the copy could not read, is what `id`
    /// now records for `destination` — read once, and refused as the read before the write is:
    /// a read that fails, an entry that is not an intent, and another attempt's intent each
    /// refuse, since none of them says the create is this copy's to make unprotected. Only an
    /// item holding no intent there, or no longer held, is one the copy goes on without.
    async fn intent_landed(
        &self,
        source: &ResolvedSource,
        level: Level,
        id: &GlobalId,
        destination: &SourceName,
        intent: &Intent,
    ) -> Result<bool, EngineError> {
        let Some(metadata) = metadata_of(source, level, &id.native)
            .await
            .map_err(|error| refused(source, error))?
        else {
            return Ok(false);
        };
        match self.held_or_refused(source, id, &metadata, destination)? {
            None => Ok(false),
            Some(landed) if landed.attempt == intent.attempt && landed.key == intent.key => {
                Ok(true)
            }
            Some(landed) => Err(EngineError::CreateInProgress {
                item: id.clone(),
                by: CreateHolder::Lease {
                    until: landed.lease_until,
                },
            }),
        }
    }

    /// The intent `metadata` records for `destination`, or the refusal of an entry that is not
    /// one.
    pub(crate) fn held_or_refused(
        &self,
        source: &ResolvedSource,
        id: &GlobalId,
        metadata: &BTreeMap<String, Value>,
        destination: &SourceName,
    ) -> Result<Option<Intent>, EngineError> {
        held(metadata, destination).map_err(|problem| {
            refused(
                source,
                onetaskgraph_plugin_api::SourceError::Malformed {
                    message: format!(
                        "{id}: {problem}; next: remove that entry once no copy of {id} is running"
                    ),
                },
            )
        })
    }

    /// Take, or take over, the intent to create `id`'s counterpart in `destination` under
    /// `key`, leased until `lease_until` — refusing as `create-in-progress` when another attempt
    /// holds a live lease, before anything is written.
    pub(in crate::engine) async fn take_intent(
        &self,
        level: Level,
        id: &GlobalId,
        destination: &SourceName,
        key: CreateKey,
        lease_until: Option<DateTime<Utc>>,
        planned_link: PlannedLink<'_>,
    ) -> Result<Taken, EngineError> {
        let source = self.writable(&id.source)?;
        pause_at("before-intent");
        let (metadata, hold) = match self.locked_read(source, level, id).await {
            Err(
                EngineError::NoSuchTask { .. }
                | EngineError::NoSuchProject { .. }
                | EngineError::NoSuchDocument { .. },
            ) => return Ok(Taken::Unkept),
            read => read?,
        };
        pause_at("intent-read");
        let now = now();
        // Another copy that created and linked the counterpart since this one planned to create
        // it has finished what this one would start: it is refused rather than made twice.
        if let Some(planned) = planned_link.expected() {
            let linked = super::link_of(&metadata, destination);
            if linked.as_ref() != planned
                && let Some(linked) = linked
            {
                return Err(EngineError::CreateInProgress {
                    item: id.clone(),
                    by: CreateHolder::Linked {
                        counterpart: linked,
                    },
                });
            }
        }
        let held = self.held_or_refused(source, id, &metadata, destination)?;
        if let Some(held) = &held
            && held.held_by_another(self.attempt, now)
        {
            return Err(EngineError::CreateInProgress {
                item: id.clone(),
                by: CreateHolder::Lease {
                    until: held.lease_until,
                },
            });
        }
        let at = held
            .as_ref()
            .filter(|held| held.key == key)
            .map_or(now, |held| held.at);
        let intent = Intent {
            key,
            at,
            attempt: self.attempt,
            lease_until: lease_until.unwrap_or(now + self.create_settings(&id.source).lease()),
        };
        let value = replaced(
            metadata.get(MetadataKey::CREATING_KEY),
            destination,
            Some(&intent),
        )
        .unwrap_or(Value::Null);
        // A source that will not keep the key — a narrow write it refuses, or answers as one it
        // cannot take — or no longer holds the record, is the one answer that is not a refusal.
        let kept =
            match set_link_versioned(source, level, &id.native, &MetadataKey::creating(), &value)
                .await
            {
                Ok(Some(_)) => true,
                Ok(None) | Err(SourceError::Refused { .. }) => false,
                // An answer it cannot read says nothing of whether it wrote: a plugin refusing the
                // namespace wrote nothing, while a backend whose answer was garbled stored the
                // intent. The item, still under the hold, is read once to say which, so an intent it
                // holds is settled by the link write rather than left to refuse the next copy.
                Err(SourceError::Malformed { .. }) => {
                    self.intent_landed(source, level, id, destination, &intent)
                        .await?
                }
                Err(error) => return Err(refused(source, error)),
            };
        Ok(if kept {
            Taken::Held {
                intent,
                exclusive: hold.held(),
            }
        } else {
            Taken::Unkept
        })
    }

    /// Renew this attempt's intent to create `id`'s counterpart in `destination`, before a
    /// destination write it sends after its create — refusing as `create-in-progress`, and
    /// writing nothing more, once the intent no longer names this attempt: another attempt holds
    /// it, or took it over and has since settled or released it; and refusing as the item's
    /// absence once its source no longer holds it. `Ok` is a lease the source has written.
    pub(in crate::engine) async fn renew_intent(
        &self,
        level: Level,
        id: &GlobalId,
        destination: &SourceName,
    ) -> Result<(), EngineError> {
        let until = now() + self.create_settings(&id.source).lease();
        self.lease_until(level, id, destination, until, LeaseChange::Renew)
            .await
    }

    /// Give up this attempt's lease on its intent to create `id`'s counterpart in `destination`,
    /// keeping the key, so a retry takes over at once: what a copy that could not finish leaves
    /// of a create that may have landed. An intent no longer this attempt's is left as it is.
    pub(in crate::engine) async fn release_lease(
        &self,
        level: Level,
        id: &GlobalId,
        destination: &SourceName,
    ) -> Result<(), EngineError> {
        self.lease_until(level, id, destination, now(), LeaseChange::Release)
            .await
    }

    /// Set this attempt's lease on its intent for `destination` to end at `until`, as `change`
    /// says what an intent no longer this attempt's, or an item gone, comes to.
    async fn lease_until(
        &self,
        level: Level,
        id: &GlobalId,
        destination: &SourceName,
        until: DateTime<Utc>,
        change: LeaseChange,
    ) -> Result<(), EngineError> {
        let source = self.writable(&id.source)?;
        let (metadata, _lock) = self.locked_read(source, level, id).await?;
        let mut held = match self.held_or_refused(source, id, &metadata, destination)? {
            Some(held) if held.attempt == self.attempt => held,
            _ if change == LeaseChange::Release => return Ok(()),
            Some(held) => {
                return Err(EngineError::CreateInProgress {
                    item: id.clone(),
                    by: CreateHolder::Lease {
                        until: held.lease_until,
                    },
                });
            }
            None => {
                return Err(EngineError::CreateInProgress {
                    item: id.clone(),
                    by: super::link_of(&metadata, destination)
                        .map_or(CreateHolder::Released, |counterpart| CreateHolder::Linked {
                            counterpart,
                        }),
                });
            }
        };
        held.lease_until = until;
        let value = replaced(
            metadata.get(MetadataKey::CREATING_KEY),
            destination,
            Some(&held),
        )
        .unwrap_or(Value::Null);
        let written =
            set_link_versioned(source, level, &id.native, &MetadataKey::creating(), &value)
                .await
                .map_err(|error| refused(source, error))?;
        // A source answering that it holds no such item wrote no lease: a renewal stops there.
        if written.is_none() && change == LeaseChange::Renew {
            return Err(missing(level, id));
        }
        Ok(())
    }

    /// Whether a create carrying `intent`'s key that got no answer may have landed in
    /// `destination`: asked of a destination whose lookup by key is current, and assumed of
    /// one whose lookup lags or cannot be made.
    pub(in crate::engine) async fn may_have_landed(
        &self,
        level: Level,
        destination: &SourceName,
        intent: Option<&Intent>,
    ) -> bool {
        let (Some(intent), Ok(destination)) = (intent, self.writable(destination)) else {
            return true;
        };
        if destination.source().create_key_lags() {
            return true;
        }
        self.relisted(level, destination, intent, &[])
            .await
            .map_or(true, |found| !found.is_empty())
    }

    /// Put the `level` record `id` back to `snapshot`, byte for byte, through its source: what
    /// a copy that took every write back leaves of an item whose intent it recorded.
    ///
    /// # Errors
    ///
    /// Whatever kept the source from putting it back.
    pub(in crate::engine) fn restore_snapshot(
        &self,
        level: Level,
        id: &GlobalId,
        snapshot: &[u8],
    ) -> Result<(), EngineError> {
        let source = self.writable(&id.source)?;
        source
            .source()
            .restore_item(level.record(), &id.native, snapshot)
            .map_err(|error| refused(source, error))
    }

    /// Remove this attempt's intent to create `id`'s counterpart in `destination`, once a copy
    /// that could not finish took back everything it wrote: nothing was made to resolve.
    pub(in crate::engine) async fn drop_intent(
        &self,
        level: Level,
        id: &GlobalId,
        destination: &SourceName,
    ) -> Result<(), EngineError> {
        let source = self.writable(&id.source)?;
        let (metadata, _lock) = self.locked_read(source, level, id).await?;
        if held(&metadata, destination)
            .ok()
            .flatten()
            .is_none_or(|held| held.attempt != self.attempt)
        {
            return Ok(());
        }
        write_without(self, source, level, &id.native, &metadata, destination)
            .await
            .map_err(|error| refused(source, error))?;
        Ok(())
    }

    /// Record `links` at `onetaskgraph.copies` on the `level` record `id` and remove its intent
    /// for `destination`: for a task in one write, the link write a copy already makes carrying
    /// the removal with it; for a project or a document in the link write and one more.
    pub(in crate::engine) async fn link_settling_intent(
        &self,
        level: Level,
        id: &GlobalId,
        destination: &SourceName,
        links: Value,
    ) -> Result<Option<Option<String>>, SourceError> {
        let source = self
            .writable(&id.source)
            .map_err(|error| SourceError::Refused {
                message: error.to_string(),
            })?;
        let _hold = hold(source, level, &id.native).map_err(|error| match error {
            EngineError::SourceRefused { error, .. } => error,
            other => SourceError::Refused {
                message: other.to_string(),
            },
        })?;
        if level == Level::Task {
            let Some(task) = source.source().get_task(&id.native).await? else {
                return Ok(None);
            };
            let mut update = onetaskgraph_plugin_api::TaskUpdate::default();
            update.metadata_set.insert(MetadataKey::copies(), links);
            match replaced(
                task.metadata.get(MetadataKey::CREATING_KEY),
                destination,
                None,
            ) {
                Some(rest) => {
                    update.metadata_set.insert(MetadataKey::creating(), rest);
                }
                None => {
                    update.metadata_remove.insert(MetadataKey::creating());
                }
            }
            let outcome = source.source().update_task(&id.native, &update).await?;
            return Ok(outcome.map(|outcome| outcome.task.version));
        }
        let Some(metadata) = metadata_of(source, level, &id.native).await? else {
            return Ok(None);
        };
        if set_link_versioned(source, level, &id.native, &MetadataKey::copies(), &links)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        write_without(self, source, level, &id.native, &metadata, destination).await
    }

    /// Resolve an intent `metadata` records for `destination` before every other rule: the item
    /// the destination holds under its key, when exactly one does; a refusal naming every one
    /// when several do; and `None` when none does — once a destination whose lookup lags has
    /// had until the quiescence window after the intent was first taken to list it.
    ///
    /// `resolving` says whether it may wait and whether it may write the lease that outlasts the
    /// wait: a dry run and `task locate` do neither, and a `--no-link` copy waits unleased, since
    /// it writes nothing to its source.
    pub(in crate::engine) async fn resolve_intent(
        &self,
        level: Level,
        destination: &ResolvedSource,
        id: &GlobalId,
        intent: &Intent,
        repositories: &[onetaskgraph_plugin_api::Repository],
        resolving: Resolving,
    ) -> Result<Option<NativeId>, EngineError> {
        let since = intent.since();
        let mut waited = false;
        loop {
            let found = destination
                .source()
                .created_with_key(
                    level.record(),
                    &intent.key(),
                    since,
                    &onetaskgraph_plugin_api::WriteTarget::New {
                        repositories,
                        project: None,
                    },
                )
                .await
                .map_err(|error| refused(destination, error))?;
            match found.as_slice() {
                [one] => return Ok(Some(one.clone())),
                [] => {}
                several => {
                    return Err(EngineError::CreateDuplicated {
                        item: id.clone(),
                        destination: destination.name().clone(),
                        found: several
                            .iter()
                            .map(|native| GlobalId::new(destination.name().clone(), native.clone()))
                            .collect(),
                    });
                }
            }
            if waited || resolving == Resolving::Reads || !destination.source().create_key_lags() {
                return Ok(None);
            }
            let Some(window_end) = intent
                .at()
                .checked_add_signed(self.create_settings(destination.name()).quiescence())
            else {
                return Ok(None);
            };
            let now = Utc::now();
            if now >= window_end {
                return Ok(None);
            }
            // Taken over by this attempt — refused while another's lease is live — and leased
            // once, to outlast the whole wait, so no renewal falls during it. A source that will
            // not keep the key, or a copy that may not write to it, waits all the same, unleased.
            if resolving == Resolving::WaitsLeased {
                self.take_intent(
                    level,
                    id,
                    destination.name(),
                    intent.key,
                    window_end.checked_add_signed(self.create_settings(&id.source).lease()),
                    PlannedLink::Unchecked,
                )
                .await?;
            }
            let wait = (window_end - now).to_std().unwrap_or_default();
            self.clock.sleep(wait).await;
            waited = true;
        }
    }

    /// Every item `destination` lists under `key` once a create carrying it has been sent,
    /// for a destination whose lookup by key lags: one is the create, and several are a
    /// duplicate reported rather than linked.
    pub(in crate::engine) async fn relisted(
        &self,
        level: Level,
        destination: &ResolvedSource,
        intent: &Intent,
        repositories: &[onetaskgraph_plugin_api::Repository],
    ) -> Result<Vec<NativeId>, EngineError> {
        destination
            .source()
            .created_with_key(
                level.record(),
                &intent.key(),
                intent.since(),
                &onetaskgraph_plugin_api::WriteTarget::New {
                    repositories,
                    project: None,
                },
            )
            .await
            .map_err(|error| refused(destination, error))
    }
}
