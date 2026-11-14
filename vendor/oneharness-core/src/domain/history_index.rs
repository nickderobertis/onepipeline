//! The dated, append-only history index: the pure half. Which segment an entry
//! belongs to, what an entry carries, and which segments a reader's window
//! names are all decided here, from values — the clock read that says what
//! "today" is and every file touched live in `src/io/history.rs`.
//!
//! The contract these types implement is declared once, in
//! `docs/history-index.md`; its field tables are test-pinned to the entry types
//! below.

use std::borrow::Cow;
use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::domain::history::{civil_from_epoch, HistoryId, HistoryLabels, SESSION_FILE_EXT};
use crate::domain::usage::UtcInstant;

/// The version every index entry this crate writes declares. The index is its
/// own contract, independent of the session file's `schema_version`.
pub const INDEX_SCHEMA_VERSION: &str = "1.0";

/// The directory, directly under the history store, that holds the dated
/// segments. A leading dot and an extension no released reader treats as a
/// session (`.ndjson`, never `.jsonl`) keep older cores from reading or
/// deleting what is in it.
pub const INDEX_DIR: &str = ".index.d";

/// The segment extension: line-delimited JSON like a session, but never
/// spelled `.jsonl`, which every released reader takes for a session file.
pub const SEGMENT_EXT: &str = "ndjson";

/// The index an older core keeps, directly under the store. Read (never
/// written) by the all-time readers and tailed by a watcher.
pub const LEGACY_INDEX_FILE: &str = ".index.jsonl";

/// The live-event index an older core keeps beside [`LEGACY_INDEX_FILE`].
pub const LEGACY_EVENT_INDEX_FILE: &str = ".event-index.jsonl";

/// A calendar date in UTC — the unit a segment is named for. Held as days since
/// 1970-01-01, and spelled `YYYY-MM-DD` on every surface (the segment names,
/// `--since`, the SDK option). Only a real calendar date between the years 1000
/// and 9999 parses, so `2026-02-30` is refused rather than rolled over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcDate(i64);

/// The error returned when text is not a `YYYY-MM-DD` calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("must be a calendar date spelled YYYY-MM-DD (UTC), with a year from 1000 to 9999")]
pub struct UtcDateError;

/// The date spelling the SDK validators check, stating exactly what
/// [`UtcDate::from_str`] accepts: the day bound follows the month, and
/// February 29th only in a leap year. Anchored at both ends, with the length
/// fixed at ten by the schema, so no trailing newline fits.
const UTC_DATE_PATTERN: &str = concat!(
    "^(?:[1-9][0-9]{3}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12][0-9]|3[01])",
    "|(?:0[469]|11)-(?:0[1-9]|[12][0-9]|30)|02-(?:0[1-9]|1[0-9]|2[0-8]))",
    "|(?:[1-9][0-9](?:0[48]|[2468][048]|[13579][26])",
    "|(?:1[26]|2[048]|3[26]|4[048]|5[26]|6[048]|7[26]|8[048]|9[26])00)-02-29)$"
);

impl UtcDate {
    /// The UTC date an instant (seconds since the epoch) falls on.
    #[must_use]
    pub fn from_epoch_secs(secs: i64) -> Self {
        Self(secs.div_euclid(86_400))
    }

    /// The date `days` after this one (before it, for a negative count).
    #[must_use]
    pub fn add_days(self, days: i64) -> Self {
        Self(self.0.saturating_add(days))
    }

    /// The date a UUIDv7 history id was minted on. `None` for an id carrying no
    /// timestamp — a legacy UUIDv5, which no date names.
    #[must_use]
    pub fn of_history_id(id: HistoryId) -> Option<Self> {
        let uuid = id.as_uuid();
        if uuid.get_version_num() != 7 {
            return None;
        }
        let (secs, _) = uuid.get_timestamp()?.to_unix();
        i64::try_from(secs).ok().map(Self::from_epoch_secs)
    }

    /// The UTC date an instant falls on. A [`UtcInstant`] is validated and
    /// held in its canonical `YYYY-MM-DDThh:mm:ssZ` spelling, so its date is
    /// its first ten characters; `None` only for a year outside the range a
    /// [`UtcDate`] spells.
    #[must_use]
    pub fn of_instant(instant: &UtcInstant) -> Option<Self> {
        instant
            .as_str()
            .get(..10)
            .and_then(|date| date.parse().ok())
    }

    fn days_in_month(year: i64, month: u32) -> u32 {
        match month {
            4 | 6 | 9 | 11 => 30,
            2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
            2 => 28,
            _ => 31,
        }
    }

    /// Howard Hinnant's `days_from_civil`, the inverse of
    /// [`civil_from_epoch`]'s date half.
    fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
        let year = if month <= 2 { year - 1 } else { year };
        let era = year.div_euclid(400);
        let yoe = year - era * 400;
        let mp = i64::from((month + 9) % 12);
        let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }
}

impl fmt::Display for UtcDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day, ..) = civil_from_epoch(self.0.saturating_mul(86_400));
        write!(f, "{year:04}-{month:02}-{day:02}")
    }
}

impl FromStr for UtcDate {
    type Err = UtcDateError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = value.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return Err(UtcDateError);
        }
        let number = |range: std::ops::Range<usize>| -> Result<u32, UtcDateError> {
            let part = &value[range];
            if part.bytes().all(|byte| byte.is_ascii_digit()) {
                part.parse().map_err(|_| UtcDateError)
            } else {
                Err(UtcDateError)
            }
        };
        let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
        let year = i64::from(year);
        if !(1000..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day == 0
            || day > Self::days_in_month(year, month)
        {
            return Err(UtcDateError);
        }
        Ok(Self(Self::days_from_civil(year, month, day)))
    }
}

impl Serialize for UtcDate {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for UtcDate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for UtcDate {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("UtcDate")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "minLength": 10,
            "maxLength": 10,
            "pattern": UTC_DATE_PATTERN,
        })
    }
}

/// Which dates of the index a listing reads.
///
/// It is also the one value an SDK history lookup, listing or watch takes as
/// its `window`, so "from this date" and "from the beginning" cannot both be
/// stated. On the wire it is externally tagged in camelCase —
/// `{"recent": {"days": 3}}`, `{"since": "2026-01-05"}`, or `"allTime"` — and
/// renders to the CLI's `--days`, `--since` or `--all-time`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "HistoryWindow")]
pub enum HistoryWindow {
    /// The last `days` UTC days, today included (`1` means today alone).
    Recent {
        /// How many UTC days, today included; at least one.
        days: NonZeroU32,
    },
    /// Every date from this one on.
    Since(UtcDate),
    /// Every segment, plus the legacy index files an older core keeps.
    AllTime,
}

impl Default for HistoryWindow {
    /// The last 7 UTC days, today included.
    fn default() -> Self {
        HistoryWindow::Recent { days: WEEK }
    }
}

/// The default window's length, the one place a non-zero day count is spelt.
const WEEK: NonZeroU32 = match NonZeroU32::new(7) {
    Some(days) => days,
    None => unreachable!(),
};

impl HistoryWindow {
    /// The earliest segment date this window reads, given today's UTC date;
    /// `None` for [`HistoryWindow::AllTime`], which reads every date.
    #[must_use]
    pub fn earliest(self, today: UtcDate) -> Option<UtcDate> {
        match self {
            HistoryWindow::Recent { days } => Some(today.add_days(1 - i64::from(days.get()))),
            HistoryWindow::Since(date) => Some(date),
            HistoryWindow::AllTime => None,
        }
    }

    /// Whether the legacy index files are part of what this window reads.
    #[must_use]
    pub fn reads_legacy(self) -> bool {
        self == HistoryWindow::AllTime
    }
}

/// Which of a date's two segments an entry belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SegmentKind {
    /// `runs-YYYY-MM-DD.ndjson`: one entry per closing run line.
    Runs,
    /// `events-YYYY-MM-DD.ndjson`: one entry per event line.
    Events,
}

impl SegmentKind {
    fn prefix(self) -> &'static str {
        match self {
            SegmentKind::Runs => "runs-",
            SegmentKind::Events => "events-",
        }
    }

    /// The file name of this kind's segment for `date`.
    #[must_use]
    pub fn file_name(self, date: UtcDate) -> String {
        format!("{}{date}.{SEGMENT_EXT}", self.prefix())
    }

    /// Read a segment file name back into its kind and date; `None` for any
    /// other name, which no reader treats as a segment.
    #[must_use]
    pub fn parse_file_name(name: &str) -> Option<(SegmentKind, UtcDate)> {
        let stem = name.strip_suffix(SEGMENT_EXT)?.strip_suffix('.')?;
        [SegmentKind::Runs, SegmentKind::Events]
            .into_iter()
            .find_map(|kind| {
                let date = stem.strip_prefix(kind.prefix())?.parse().ok()?;
                Some((kind, date))
            })
    }
}

/// Where in its session file the line an entry points at sits: a hint, which
/// a reader checks against what it reads there and, on a mismatch, answers by
/// reading that one session file instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineSpan {
    /// The byte offset the line starts at.
    pub offset: u64,
    /// The line's length in bytes, its newline included — never zero, so an
    /// entry naming an empty span does not parse.
    pub length: std::num::NonZeroU64,
}

/// A session file's path relative to the store, as an entry names it: exactly
/// `<project-slug>/<session>.jsonl` (see [`valid_session_path`]). Validated
/// wherever it is parsed — an entry is read from disk, so one naming a path
/// that climbs out of the store does not parse at all.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionPath(String);

/// The error returned when text is not a store-relative session path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("must be `<project-slug>/<session>.jsonl`, relative to the history store")]
pub struct SessionPathError;

impl SessionPath {
    /// The session path for a session id under a project slug, spelled with
    /// `/` on every platform; `None` when either could name another place.
    #[must_use]
    pub fn new(project_slug: &str, session: &str) -> Option<Self> {
        session_path(project_slug, session).parse().ok()
    }

    /// The path as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The project slug and the session id (file stem) it names.
    #[must_use]
    pub fn parts(&self) -> (&str, &str) {
        session_path_parts(&self.0).expect("a SessionPath is valid by construction")
    }
}

impl FromStr for SessionPath {
    type Err = SessionPathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if valid_session_path(value) {
            Ok(Self(value.to_string()))
        } else {
            Err(SessionPathError)
        }
    }
}

impl<'de> Deserialize<'de> for SessionPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl SessionPath {
    /// The session file this path names under the store `dir`, joined one
    /// component at a time. Never `dir.join` of the `/`-spelled text: under a
    /// Windows verbatim (`\\?\`) directory — what canonicalizing gives — a
    /// `/` is not a separator, so the joined path names no file.
    #[must_use]
    pub fn under(&self, dir: &std::path::Path) -> std::path::PathBuf {
        let (slug, stem) = self.parts();
        dir.join(slug).join(format!("{stem}.{SESSION_FILE_EXT}"))
    }
}

impl fmt::Display for SessionPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A session file's path relative to the store as an older core wrote it to
/// the legacy index: one project directory, then one `<session>.jsonl` file.
/// Looser than [`SessionPath`] only where released cores were: their
/// reconcile indexed every `<subdir>/*.jsonl` it found, so a legacy line can
/// name a file whose name holds `:`. Each part must still be one plain path
/// component on this platform — never empty, `.`, `..`, a root or a drive
/// prefix — so no legacy path reaches outside the store; and the project is
/// never [`INDEX_DIR`], which no released core wrote into (see
/// [`LegacyEntryPath`]). Every [`SessionPath`] is one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LegacySessionPath {
    project_slug: String,
    session: String,
}

impl LegacySessionPath {
    /// The project slug and the session id (file stem) it names.
    #[must_use]
    pub fn parts(&self) -> (&str, &str) {
        (&self.project_slug, &self.session)
    }

    /// The session file this path names under the store `dir`, joined one
    /// component at a time (see [`SessionPath::under`]).
    #[must_use]
    pub fn under(&self, dir: &std::path::Path) -> std::path::PathBuf {
        dir.join(&self.project_slug)
            .join(format!("{}.{SESSION_FILE_EXT}", self.session))
    }
}

impl From<&SessionPath> for LegacySessionPath {
    fn from(path: &SessionPath) -> Self {
        let (project_slug, session) = path.parts();
        Self {
            project_slug: project_slug.to_string(),
            session: session.to_string(),
        }
    }
}

impl FromStr for LegacySessionPath {
    type Err = SessionPathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let one_component = |part: &str| {
            let mut components = std::path::Path::new(part).components();
            matches!(
                (components.next(), components.next()),
                (Some(std::path::Component::Normal(_)), None)
            )
        };
        let mut parts = value.split(['/', '\\']);
        let (Some(project), Some(file), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(SessionPathError);
        };
        let session = file
            .strip_suffix(SESSION_FILE_EXT)
            .and_then(|stem| stem.strip_suffix('.'))
            .filter(|stem| !stem.is_empty())
            .ok_or(SessionPathError)?;
        if project == INDEX_DIR || !one_component(project) || !one_component(file) {
            return Err(SessionPathError);
        }
        Ok(Self {
            project_slug: project.to_string(),
            session: session.to_string(),
        })
    }
}

/// What a legacy index entry's `session_path` names. `.index.d/` did not
/// exist before dated segments, so no released core wrote an entry pointing
/// into it; one that does is kept as its text so a reader can refuse it by
/// path rather than skip the line silently. Any other path that is not a
/// [`LegacySessionPath`] does not parse, like any malformed line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyEntryPath {
    /// A session file a released core could have indexed.
    Session(LegacySessionPath),
    /// A path into [`INDEX_DIR`], as the entry spelled it.
    IntoIndexDir(String),
}

impl FromStr for LegacyEntryPath {
    type Err = SessionPathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Ok(path) = value.parse() {
            return Ok(Self::Session(path));
        }
        match value.split_once(['/', '\\']) {
            Some((INDEX_DIR, _)) => Ok(Self::IntoIndexDir(value.to_string())),
            _ => Err(SessionPathError),
        }
    }
}

impl<'de> Deserialize<'de> for LegacyEntryPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// One closing run line, indexed. Every field is bounded by what names the
/// run — never by its prompt, output or events.
///
/// `session`, `name`, `project_slug` and `harness_id` are copied verbatim from
/// the session line the entry points at, so a listing built from the index
/// says exactly what the record says; they stay text because a record migrated
/// from a 0.x store carries names and ids no current type accepts (`legacy
/// 0.3`), and reindexing one must not drop it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunIndexEntry {
    // llmlint: ignore[invalid_states_unrepresentable] The index is read from disk and written by more than one release, so its version stays the wire string every other history contract uses; a reader accepts any entry that parses.
    pub schema_version: String,
    pub history_id: HistoryId,
    /// The session file, relative to the store: `<project-slug>/<session>.jsonl`.
    pub session_path: SessionPath,
    /// The session id — the session file's stem.
    // llmlint: ignore[invalid_states_unrepresentable] Copied verbatim from the record this entry points at (see the type's doc): a reindexed 0.x store's session ids are not in the shape a current writer mints, and refusing them here would drop those runs from the index.
    pub session: String,
    /// The session's name.
    // llmlint: ignore[invalid_states_unrepresentable] Copied verbatim from the record (see the type's doc); a migrated 0.x record's name, such as `legacy 0.3`, is not a sanitized session name.
    pub name: String,
    // llmlint: ignore[invalid_states_unrepresentable] The slug is also `session_path`'s first component, which is the validated one every reader uses; this copy is the contract's filterable field.
    pub project_slug: String,
    // llmlint: ignore[invalid_states_unrepresentable] Copied verbatim from the record's `harness_id`, so a filter on the index agrees with the record; a harness id a later release no longer registers must still index.
    pub harness_id: String,
    #[serde(default, skip_serializing_if = "HistoryLabels::is_empty")]
    pub labels: HistoryLabels,
    /// The record's `timestamp`: when its closing line was written.
    pub recorded_at: UtcInstant,
    #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
    pub span: Option<LineSpan>,
}

/// One event line, indexed. Like a run entry, its size never depends on the
/// event's content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventIndexEntry {
    // llmlint: ignore[invalid_states_unrepresentable] Same wire-string version as `RunIndexEntry::schema_version`, for the same reason.
    pub schema_version: String,
    /// The id of the run the event belongs to.
    pub run_id: HistoryId,
    /// The event's `index` within its run.
    pub event_index: usize,
    pub session_path: SessionPath,
    // llmlint: ignore[invalid_states_unrepresentable] As on `RunIndexEntry`: `session_path`'s validated first component is what readers use; this is the contract's filterable copy.
    pub project_slug: String,
    // llmlint: ignore[invalid_states_unrepresentable] As on `RunIndexEntry`: copied verbatim from the event line so the index agrees with it.
    pub harness_id: String,
    #[serde(default)]
    pub labels: HistoryLabels,
    #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
    pub span: Option<LineSpan>,
}

/// One line of a segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryIndexEntry {
    Run(RunIndexEntry),
    Event(EventIndexEntry),
}

impl HistoryIndexEntry {
    /// The segment this entry is appended to. The date is the one its id was
    /// minted on; an id carrying none (a legacy UUIDv5) falls back to
    /// `fallback`, the date its session's own evidence names.
    #[must_use]
    pub fn segment(&self, fallback: Option<UtcDate>) -> Option<(SegmentKind, UtcDate)> {
        let (kind, id) = match self {
            HistoryIndexEntry::Run(run) => (SegmentKind::Runs, run.history_id),
            HistoryIndexEntry::Event(event) => (SegmentKind::Events, event.run_id),
        };
        UtcDate::of_history_id(id)
            .or(fallback)
            .map(|date| (kind, date))
    }

    /// The session file this entry points at, relative to the store.
    #[must_use]
    pub fn session_path(&self) -> &SessionPath {
        match self {
            HistoryIndexEntry::Run(run) => &run.session_path,
            HistoryIndexEntry::Event(event) => &event.session_path,
        }
    }

    /// What identifies the line this entry indexes: the run id, and — for an
    /// event — its index within the run. Two entries with one key index one line.
    #[must_use]
    pub fn key(&self) -> IndexKey {
        match self {
            HistoryIndexEntry::Run(run) => IndexKey::Run(run.history_id),
            HistoryIndexEntry::Event(event) => IndexKey::Event(event.run_id, event.event_index),
        }
    }
}

/// What `reindex` de-duplicates on: a segment holding an entry with a line's
/// key already indexes that line, whatever else the entries carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IndexKey {
    Run(HistoryId),
    Event(HistoryId, usize),
}

/// The session path an index entry may name: exactly
/// `<project-slug>/<session>.jsonl` (either separator), with no component that
/// is empty or climbs. An entry is read from disk, so a path that would reach
/// outside the store is refused rather than opened.
#[must_use]
pub fn valid_session_path(path: &str) -> bool {
    let mut parts = path.split(['/', '\\']);
    let (Some(project), Some(file), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let plain = |part: &str| !part.is_empty() && part != "." && part != ".." && !part.contains(':');
    plain(project)
        && project != INDEX_DIR
        && plain(file)
        && file
            .strip_suffix(SESSION_FILE_EXT)
            .and_then(|stem| stem.strip_suffix('.'))
            .is_some_and(|stem| !stem.is_empty())
}

/// The session id (file stem) and project slug a valid session path names.
#[must_use]
pub fn session_path_parts(path: &str) -> Option<(&str, &str)> {
    if !valid_session_path(path) {
        return None;
    }
    let (project, file) = path.split_once(['/', '\\'])?;
    let stem = file.strip_suffix(SESSION_FILE_EXT)?.strip_suffix('.')?;
    Some((project, stem))
}

/// The session path for a session id under a project slug, spelled with `/`
/// on every platform.
#[must_use]
pub fn session_path(project_slug: &str, session: &str) -> String {
    format!("{project_slug}/{session}.{SESSION_FILE_EXT}")
}

/// The name a writer minted a session id from: every id is
/// `<name>-<YYYYMMDDThhmmssZ>-<pid>`, so it is the id less its last two parts.
/// `None` for a stem in any other shape.
#[must_use]
pub fn session_name_from_id(id: &str) -> Option<&str> {
    crate::domain::history::session_started_from_id(id)?;
    id.rsplitn(3, '-').nth(2)
}

/// The UTC date a session id records its start on.
#[must_use]
pub fn session_date_from_id(id: &str) -> Option<UtcDate> {
    crate::domain::history::session_started_from_id(id)
        .and_then(|at| at.parse::<UtcInstant>().ok())
        .and_then(|at| UtcDate::of_instant(&at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_round_trips_through_its_spelling_and_refuses_a_rolled_over_day() {
        for text in [
            "1970-01-01",
            "2000-02-29",
            "2026-09-29",
            "9999-12-31",
            "1000-01-01",
        ] {
            let date: UtcDate = text.parse().unwrap();
            assert_eq!(date.to_string(), text);
        }
        assert_eq!("1970-01-01".parse::<UtcDate>().unwrap(), UtcDate(0));
        for bad in [
            "",
            "2026-9-29",
            "2026-09-29T",
            "2026/09/29",
            "2026-02-30",
            "2025-02-29",
            "1900-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-01-00",
            "0999-01-01",
            "+026-01-01",
            "2026-01-3a",
        ] {
            assert!(bad.parse::<UtcDate>().is_err(), "{bad}");
        }
        assert_eq!(
            UtcDate::from_epoch_secs(1_790_700_180).to_string(),
            "2026-09-29"
        );
        assert_eq!(UtcDate::from_epoch_secs(-1).to_string(), "1969-12-31");
    }

    #[test]
    fn the_schema_pattern_accepts_exactly_what_the_parser_accepts() {
        // Every day of a leap year, a common year, a century that is not leap
        // and one that is, plus each month's first impossible day: the SDK
        // validators and the CLI must agree on all of them.
        let pattern = regex::Regex::new(UTC_DATE_PATTERN).unwrap();
        for year in [1000, 1900, 2000, 2024, 2026, 2100, 2400, 9999] {
            for month in 1..=12 {
                for day in 0..=32 {
                    let text = format!("{year:04}-{month:02}-{day:02}");
                    assert_eq!(
                        pattern.is_match(&text),
                        text.parse::<UtcDate>().is_ok(),
                        "{text}"
                    );
                }
            }
        }
        assert!(!pattern.is_match("0999-01-01"));
        assert!(!pattern.is_match("2026-01-01\n"));
    }

    #[test]
    fn a_v7_id_is_dated_by_its_timestamp_and_a_v5_id_by_nothing() {
        let at = uuid::Timestamp::from_unix(uuid::NoContext, 1_790_700_180, 0);
        let id = HistoryId::from_uuid(uuid::Uuid::new_v7(at));
        assert_eq!(
            UtcDate::of_history_id(id).unwrap().to_string(),
            "2026-09-29"
        );
        assert_eq!(UtcDate::of_history_id(HistoryId::legacy(b"x")), None);
    }

    #[test]
    fn an_instant_is_dated_by_its_utc_day_and_malformed_text_never_reaches_one() {
        let instant: UtcInstant = "2026-09-29T23:59:59Z".parse().unwrap();
        assert_eq!(
            UtcDate::of_instant(&instant).unwrap().to_string(),
            "2026-09-29"
        );
        // A timestamp in another spelling of UTC is normalized first.
        let offset: UtcInstant = "2026-09-30T00:00:00+00:00".parse().unwrap();
        assert_eq!(
            UtcDate::of_instant(&offset).unwrap().to_string(),
            "2026-09-30"
        );
        assert!("2026-01-01junk".parse::<UtcInstant>().is_err());
    }

    #[test]
    fn a_window_names_its_earliest_date() {
        let today: UtcDate = "2026-09-29".parse().unwrap();
        let earliest = |window: HistoryWindow| window.earliest(today).map(|d| d.to_string());
        assert_eq!(
            earliest(HistoryWindow::default()).as_deref(),
            Some("2026-09-23")
        );
        assert_eq!(
            earliest(HistoryWindow::Recent {
                days: NonZeroU32::MIN
            })
            .as_deref(),
            Some("2026-09-29")
        );
        assert_eq!(
            earliest(HistoryWindow::Since("2020-01-01".parse().unwrap())).as_deref(),
            Some("2020-01-01")
        );
        assert_eq!(earliest(HistoryWindow::AllTime), None);
        assert!(HistoryWindow::AllTime.reads_legacy());
        assert!(!HistoryWindow::default().reads_legacy());
    }

    #[test]
    fn segment_names_round_trip_and_nothing_else_reads_as_one() {
        let date: UtcDate = "2026-09-29".parse().unwrap();
        assert_eq!(SegmentKind::Runs.file_name(date), "runs-2026-09-29.ndjson");
        assert_eq!(
            SegmentKind::Events.file_name(date),
            "events-2026-09-29.ndjson"
        );
        for kind in [SegmentKind::Runs, SegmentKind::Events] {
            assert_eq!(
                SegmentKind::parse_file_name(&kind.file_name(date)),
                Some((kind, date))
            );
        }
        for foreign in [
            "runs-2026-09-29.jsonl",
            "runs-2026-02-30.ndjson",
            "other-2026-09-29.ndjson",
            "runs-2026-09-29.ndjson.tmp",
            "runs-.ndjson",
        ] {
            assert_eq!(SegmentKind::parse_file_name(foreign), None, "{foreign}");
        }
    }

    #[test]
    fn a_session_path_is_one_project_and_one_session_file() {
        assert!(valid_session_path("proj/s-20260929T000000Z-1.jsonl"));
        assert!(valid_session_path("proj\\s.jsonl"));
        assert_eq!(
            session_path_parts("proj/s-20260929T000000Z-1.jsonl"),
            Some(("proj", "s-20260929T000000Z-1"))
        );
        for bad in [
            "",
            "s.jsonl",
            "../s.jsonl",
            "proj/../s.jsonl",
            "proj/sub/s.jsonl",
            "/proj/s.jsonl",
            "proj/.jsonl",
            "proj/s.ndjson",
            ".index.d/runs-2026-09-29.jsonl",
            "C:/s.jsonl",
        ] {
            assert!(!valid_session_path(bad), "{bad}");
        }
        assert_eq!(session_path("p", "s"), "p/s.jsonl");
    }

    #[test]
    fn a_legacy_session_path_takes_what_released_cores_indexed_and_stays_in_the_store() {
        let parts = |text: &str| {
            text.parse::<LegacySessionPath>()
                .ok()
                .map(|path| (path.parts().0.to_string(), path.parts().1.to_string()))
        };
        let owned = |slug: &str, stem: &str| Some((slug.to_string(), stem.to_string()));
        assert_eq!(parts("proj/s.jsonl"), owned("proj", "s"));
        assert_eq!(parts("proj\\s.jsonl"), owned("proj", "s"));
        assert_eq!(parts("proj/..jsonl"), owned("proj", "."));
        // A `:` is part of a name on Unix and a drive prefix on Windows, where
        // it would re-root the join; there no released core could name one.
        if cfg!(windows) {
            assert_eq!(parts("C:/s.jsonl"), None);
            assert_eq!(parts("proj/C:s.jsonl"), None);
        } else {
            assert_eq!(parts("proj/a:b.jsonl"), owned("proj", "a:b"));
            assert_eq!(parts("C:/s.jsonl"), owned("C:", "s"));
        }
        for bad in [
            "",
            "s.jsonl",
            "../s.jsonl",
            "./s.jsonl",
            "proj/../s.jsonl",
            "proj/sub/s.jsonl",
            "/proj/s.jsonl",
            "proj/.jsonl",
            "proj/s.ndjson",
            ".index.d/s.jsonl",
        ] {
            assert_eq!(parts(bad), None, "{bad}");
        }
        assert_eq!(
            ".index.d/s.jsonl".parse::<LegacyEntryPath>(),
            Ok(LegacyEntryPath::IntoIndexDir(
                ".index.d/s.jsonl".to_string()
            ))
        );
        assert_eq!(
            ".index.d\\runs-2026-09-29.ndjson".parse::<LegacyEntryPath>(),
            Ok(LegacyEntryPath::IntoIndexDir(
                ".index.d\\runs-2026-09-29.ndjson".to_string()
            ))
        );
        assert_eq!(
            "proj/s.jsonl".parse::<LegacyEntryPath>(),
            Ok(LegacyEntryPath::Session("proj/s.jsonl".parse().unwrap()))
        );
        assert_eq!(
            "../s.jsonl".parse::<LegacyEntryPath>(),
            Err(SessionPathError)
        );
        let dated: SessionPath = "proj/s.jsonl".parse().unwrap();
        assert_eq!(
            LegacySessionPath::from(&dated),
            "proj/s.jsonl".parse().unwrap()
        );
    }

    #[test]
    fn a_session_id_names_its_session_and_its_date() {
        let id = "fix-the-bug-20260929T235959Z-42";
        assert_eq!(session_name_from_id(id), Some("fix-the-bug"));
        assert_eq!(session_date_from_id(id).unwrap().to_string(), "2026-09-29");
        assert_eq!(session_name_from_id("not-a-session"), None);
        assert_eq!(session_date_from_id("not-a-session"), None);
    }

    #[test]
    fn an_entry_carries_its_kind_and_omits_an_absent_span() {
        let id: HistoryId = "0192b2a0-0000-7000-8000-000000000001".parse().unwrap();
        let entry = HistoryIndexEntry::Event(EventIndexEntry {
            schema_version: INDEX_SCHEMA_VERSION.to_string(),
            run_id: id,
            event_index: 3,
            session_path: "p/s.jsonl".parse().unwrap(),
            project_slug: "p".to_string(),
            harness_id: "codex".to_string(),
            labels: HistoryLabels::default(),
            span: None,
        });
        let value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["kind"], "event");
        assert!(value.get("offset").is_none());
        assert_eq!(value["labels"], serde_json::json!({}));
        assert_eq!(
            serde_json::from_value::<HistoryIndexEntry>(value).unwrap(),
            entry
        );
        assert_eq!(entry.key(), IndexKey::Event(id, 3));
        assert_eq!(
            entry.segment(None).map(|(kind, _)| kind),
            Some(SegmentKind::Events)
        );
    }
}
