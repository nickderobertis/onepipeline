//! Streaming, reading, and managing the standardized run history on disk. This
//! is the I/O half of the feature: it reads the clock (to mint session ids and
//! record timestamps), resolves the platform state directory, and writes/reads
//! the JSONL history files. The record *shape*, the index entry shape and all
//! string formatting stay pure in `src/domain/history.rs` and
//! `src/domain/history_index.rs`.
//!
//! Layout: `<dir>/<project-slug>/<session>.jsonl`. One file per `oneharness run`
//! invocation (the "session"), partitioned by a slug of the project directory, so
//! runs from different projects never interleave. Each line is one
//! [`crate::domain::history::HistoryRecord`], appended as a harness run finalizes.
//!
//! Beside the sessions, `<dir>/.index.d/` holds the dated, append-only index:
//! one small pointer entry per session line, in the segment named for the UTC
//! date its run's id was minted on. Recording a run appends to one segment and
//! reads nothing else; a reader reads only the segments its window names; and
//! only [`reindex`], [`migrate`], [`remove_sessions`] and the
//! [`HistoryWindow::AllTime`] readers ever read the whole store. The contract is
//! `docs/history-index.md`.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::harness::HarnessIdentity;
use crate::domain::history::{
    self, HistoryEventLine, HistoryId, HistoryLabels, HistoryLine, HistoryPointer, HistoryRecord,
    HistoryRunRecord, HistorySessionId, HistorySessionName, HistorySessionSelector,
    HistoryShowEntry, IncompleteHistoryRun, PointerSession,
};
use crate::domain::history_index::{
    self as index, EventIndexEntry, HistoryIndexEntry, IndexKey, LegacyEntryPath,
    LegacySessionPath, LineSpan, RunIndexEntry, SegmentKind, INDEX_DIR, INDEX_SCHEMA_VERSION,
    LEGACY_EVENT_INDEX_FILE, LEGACY_INDEX_FILE,
};
pub use crate::domain::history_index::{HistoryWindow, UtcDate};
use crate::domain::mode::PermissionMode;
use crate::domain::report::RunResult;
use crate::domain::sdk::{LiteralFalse, LiteralTrue};
use crate::domain::usage::UtcInstant;
use crate::errors::OneharnessError;

/// The file extension for every session log (line-delimited JSON) — the one
/// the pointer line's `history_file` is checked against.
const SESSION_EXT: &str = history::SESSION_FILE_EXT;

/// Seconds since the UNIX epoch, UTC. The single clock read the history feature
/// makes — kept here in the I/O layer so `domain::history` stays pure.
fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        // A clock before 1970 is implausible; fall back to the epoch rather than
        // panic — history is a best-effort side channel.
        .unwrap_or(0)
}

/// Today's UTC date: what a default window and a cursor-less watch start from.
fn today() -> UtcDate {
    UtcDate::from_epoch_secs(now_epoch_secs())
}

/// The per-user state directory, resolved like [`crate::io::config`]'s config
/// dir but for state/logs: `%LOCALAPPDATA%` on Windows; `$XDG_STATE_HOME` (else
/// `~/.local/state`) everywhere else.
fn state_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
    }
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg));
    }
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|home| PathBuf::from(home).join(".local").join("state"))
}

/// The effective history directory: the configured path if given, else the
/// platform default `<state dir>/oneharness/history`. `None` only when no path
/// was configured and the platform state dir cannot be resolved (no `$HOME`).
pub fn resolve_dir(configured: Option<&str>) -> Option<PathBuf> {
    match configured {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => state_dir().map(|d| d.join("oneharness").join("history")),
    }
}

/// A handle to one session's history file, opened once per run and appended to as
/// each harness result finalizes.
///
/// It holds nothing whose size depends on the store: no parsed index and no
/// set of indexed ids, for any part of its life.
pub struct HistoryWriter {
    dir: history::AbsoluteHistoryDir,
    path: PathBuf,
    relative_path: index::SessionPath,
    session: history::HistoryPathComponent,
    name: HistorySessionName,
    labels: HistoryLabels,
    project: String,
    project_slug: history::HistoryPathComponent,
    /// The run's pointer file, when one was named: every harness run this
    /// writer begins appends one [`HistoryPointer`] line to it.
    pointer_file: Option<PathBuf>,
    /// Whether a pointer append has already failed and warned this run — the
    /// pointer is best-effort, and a file that refuses once is said once.
    pointer_warned: AtomicBool,
}

#[derive(Debug)]
pub struct EventAppendOutcome {
    pub index_error: Option<std::io::Error>,
}

impl HistoryWriter {
    /// Mint the id shared by a live run's incremental event lines and closing run line.
    pub fn begin_run(&self) -> HistoryId {
        HistoryId::from_uuid(uuid::Uuid::now_v7())
    }

    /// Name the pointer file every harness run this writer begins appends its
    /// [`HistoryPointer`] line to. `None` (the default) writes no pointer.
    #[must_use]
    pub fn with_pointer_file(mut self, pointer_file: Option<PathBuf>) -> Self {
        self.pointer_file = pointer_file;
        self
    }

    /// The pointer file this writer appends to, if one was named.
    pub fn pointer_file(&self) -> Option<&Path> {
        self.pointer_file.as_deref()
    }

    /// Begin one harness run: mint its id and flush the complete prompt in an
    /// opening history line. When a pointer file is named, append its [`HistoryPointer`]
    /// line to it — one write, before the harness is spawned. Best-effort like
    /// the store itself: a pointer file that cannot be opened or written warns
    /// on stderr once per run and the line is skipped, never the run.
    pub fn begin_harness_run(&self, harness_id: &HarnessIdentity, prompt: &str) -> HistoryId {
        let run_id = self.begin_run();
        let line = HistoryLine::RunStarted(history::HistoryRunStarted {
            schema_version: history::RunStartedSchemaVersion,
            history_id: run_id,
            harness_id: harness_id.clone(),
            prompt: prompt.to_string(),
            started: UtcInstant::from_epoch(now_epoch_secs()),
        });
        if let Err(error) = SessionFile::open(&self.path).and_then(|mut file| file.append(&line)) {
            eprintln!("oneharness: warning: could not append opening history line: {error}");
        }
        if let Some(pointer_file) = &self.pointer_file {
            let written = self.pointer_session().and_then(|session| {
                let pointer = HistoryPointer::new(
                    &session,
                    run_id,
                    harness_id,
                    UtcInstant::from_epoch(now_epoch_secs()),
                )
                .map_err(|error| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
                })?;
                append_pointer_line(pointer_file, &pointer)
            });
            if let Err(err) = written {
                if !self.pointer_warned.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "oneharness: warning: could not append to history pointer file `{}`: \
                         {err}; skipping pointer lines for this run",
                        pointer_file.display()
                    );
                }
            }
        }
        run_id
    }

    /// Locate the file this writer appends for a begun candidate.
    pub fn run_begun(
        &self,
        harness_id: &HarnessIdentity,
        history_id: HistoryId,
    ) -> history::RunBegun {
        history::RunBegun {
            harness_id: harness_id.clone(),
            history_id,
            history_dir: self.dir.clone(),
            history_project: self.project_slug.clone(),
            history_session: self.session.clone(),
        }
    }

    /// What every pointer line of this session repeats. `dir` and `path` are
    /// canonical since [`open`](Self::open) and `path` sits one project
    /// directory under `dir`, so this is the constructor's own layout; a
    /// refusal here would mean the writer's paths and the pointer's rule have
    /// drifted apart, which is worth a loud warning rather than a bad line.
    fn pointer_session(&self) -> std::io::Result<PointerSession> {
        PointerSession::new(
            &self.dir,
            &self.path,
            self.name.as_str(),
            &self.project,
            self.labels.clone(),
        )
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// Durably append one live event and make it visible to event-mode watchers.
    pub fn append_event(
        &self,
        run_id: HistoryId,
        harness: &str,
        event: crate::domain::events::ActionEvent,
    ) -> std::io::Result<()> {
        match self
            .append_event_tracked(run_id, harness, event)?
            .index_error
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Append an event while distinguishing session durability from a later
    /// best-effort event-index failure.
    // llmlint: ignore[invalid_states_unrepresentable] This I/O boundary receives the normalized composed id already stored on the run result, then atomically derives all three wire fields; history materialization validates their consistency and CLI round-trip tests cover variant filtering.
    pub fn append_event_tracked(
        &self,
        run_id: HistoryId,
        harness: &str,
        event: crate::domain::events::ActionEvent,
    ) -> std::io::Result<EventAppendOutcome> {
        let (base, variant) = harness
            .split_once(':')
            .map_or((harness, None), |(base, variant)| (base, Some(variant)));
        let line = HistoryEventLine {
            schema_version: if event.timing_source.is_some() {
                history::EVENT_SCHEMA_VERSION
            } else {
                history::PREVIOUS_CURRENT_SCHEMA_VERSION
            }
            .to_string(),
            run_id,
            harness: base.to_string(),
            variant: variant.map(str::to_string),
            harness_id: Some(harness.to_string()),
            event,
            session_name: Some(self.name.clone()),
        };
        let span = SessionFile::open(&self.path)?.append(&HistoryLine::Event(line.clone()))?;
        let index_error = self.append_index(&self.event_entry(&line, span)).err();
        Ok(EventAppendOutcome { index_error })
    }

    /// Open (create) the session file under `dir` for a run in `project`. Mints
    /// the session id from the sanitized `name`, the current instant, and the pid
    /// so concurrent runs never collide: `<name>-<YYYYMMDDThhmmssZ>-<pid>`. The
    /// project subdirectory is created now; the file itself is created on the
    /// first [`append`](Self::append).
    ///
    /// Opening reads no index and walks no directory: the only thing it
    /// touches beyond canonicalizing its two paths is its own project
    /// directory, which it creates.
    pub fn open(
        dir: &Path,
        project: &Path,
        name: &str,
        labels: HistoryLabels,
    ) -> std::io::Result<HistoryWriter> {
        let name = HistorySessionName::sanitize(name);
        let project = fs::canonicalize(project)?;
        let project_display = project.display().to_string();
        let slug = history::project_slug(&project_display);
        let session = format!(
            "{name}-{}-{}",
            history::format_compact_utc(now_epoch_secs()),
            std::process::id()
        );
        fs::create_dir_all(dir)?;
        let dir = fs::canonicalize(dir)?;
        let project_dir = dir.join(&slug);
        fs::create_dir_all(&project_dir)?;
        let path = project_dir.join(format!("{session}.{SESSION_EXT}"));
        let relative_path = index::SessionPath::new(&slug, &session).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("`{slug}/{session}` cannot name a session file in the store"),
            )
        })?;
        let address_error = |error: history::HistoryAddressError| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, error)
        };
        Ok(HistoryWriter {
            relative_path,
            dir: dir.try_into().map_err(address_error)?,
            path,
            session: session.try_into().map_err(address_error)?,
            name,
            labels,
            project: project_display,
            project_slug: slug.try_into().map_err(address_error)?,
            pointer_file: None,
            pointer_warned: AtomicBool::new(false),
        })
    }

    /// The absolute-or-relative path of the session file (as opened).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one harness result as ordered event lines followed by its terminal
    /// run line, stamped with the current instant. Creates the file on first write.
    pub fn append(
        &self,
        mode: PermissionMode,
        model: Option<&str>,
        run_prompt: &str,
        result: &RunResult,
    ) -> std::io::Result<()> {
        self.append_with_id(self.begin_run(), mode, model, run_prompt, result, None)
    }

    /// Append the terminal line for a run whose events were already persisted live.
    pub fn append_streamed(
        &self,
        run_id: HistoryId,
        mode: PermissionMode,
        model: Option<&str>,
        run_prompt: &str,
        result: &RunResult,
        persisted_event_indexes: &std::collections::BTreeSet<usize>,
    ) -> std::io::Result<()> {
        self.append_with_id(
            run_id,
            mode,
            model,
            run_prompt,
            result,
            Some(persisted_event_indexes),
        )
    }

    fn append_with_id(
        &self,
        run_id: HistoryId,
        mode: PermissionMode,
        model: Option<&str>,
        run_prompt: &str,
        result: &RunResult,
        persisted_event_indexes: Option<&std::collections::BTreeSet<usize>>,
    ) -> std::io::Result<()> {
        let record = HistoryRecord::from_result(
            run_id,
            &self.session,
            self.name.as_str(),
            &self.labels,
            &self.project,
            history::format_rfc3339(now_epoch_secs()),
            mode,
            model,
            run_prompt,
            result,
        );
        let run = HistoryRunRecord::from_record(&record);
        if !record.complete() || !run.valid() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "new history run lacks complete v1.0 telemetry",
            ));
        }
        let mut lines = result.events.clone().unwrap_or_default();
        if let Some(persisted) = persisted_event_indexes {
            lines.retain(|event| !persisted.contains(&event.index));
        }
        lines.sort_by_key(|event| event.index);
        let mut file = SessionFile::open(&self.path)?;
        let mut entries = Vec::with_capacity(lines.len() + 1);
        for event in lines {
            let line = HistoryEventLine {
                schema_version: history::EVENT_SCHEMA_VERSION.to_string(),
                run_id: run.history_id,
                harness: run.harness.clone(),
                variant: run.variant.clone(),
                harness_id: run.harness_id.clone(),
                event,
                session_name: Some(self.name.clone()),
            };
            let span = file.append(&HistoryLine::Event(line.clone()))?;
            entries.push(self.event_entry(&line, span));
        }
        let run_entry = RunIndexEntry {
            schema_version: INDEX_SCHEMA_VERSION.to_string(),
            history_id: run.history_id,
            session_path: self.relative_path.clone(),
            session: self.session.to_string(),
            name: self.name.as_str().to_string(),
            project_slug: self.project_slug.to_string(),
            harness_id: record.harness_id.clone(),
            labels: self.labels.clone(),
            recorded_at: record.timestamp.parse().map_err(|error| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{error}"))
            })?,
            span: None,
        };
        let span = file.append(&HistoryLine::Run(run))?;
        entries.push(HistoryIndexEntry::Run(RunIndexEntry {
            span: Some(span),
            ..run_entry
        }));
        for entry in &entries {
            self.append_index(entry)?;
        }
        Ok(())
    }

    fn event_entry(&self, line: &HistoryEventLine, span: LineSpan) -> HistoryIndexEntry {
        HistoryIndexEntry::Event(EventIndexEntry {
            schema_version: INDEX_SCHEMA_VERSION.to_string(),
            run_id: line.run_id,
            event_index: line.event.index,
            session_path: self.relative_path.clone(),
            project_slug: self.project_slug.to_string(),
            harness_id: line
                .harness_id
                .clone()
                .unwrap_or_else(|| line.harness.clone()),
            labels: self.labels.clone(),
            span: Some(span),
        })
    }

    /// Append one entry to the segment its id's date names — the only index
    /// file a recording run ever touches.
    fn append_index(&self, entry: &HistoryIndexEntry) -> std::io::Result<()> {
        append_index_entry(&self.dir, entry, Some(today()))
    }
}

/// A run's own session file, opened for append. Only this process's writer
/// appends to it, so reading its last byte is the whole of what a torn tail
/// costs: a line an interrupted write left without its newline is closed off
/// before the next one goes out, so the next is never read as its tail.
struct SessionFile {
    file: File,
}

impl SessionFile {
    fn open(path: &Path) -> std::io::Result<SessionFile> {
        let mut file = open_for_append(path)?;
        if ends_torn(&mut file)? {
            file.write_all(b"\n")?;
        }
        Ok(SessionFile { file })
    }

    /// Write one line and say where it landed: the span a reader checks
    /// before trusting it, so a position the platform reports differently
    /// costs a reread rather than a wrong answer.
    fn append(&mut self, line: &HistoryLine) -> std::io::Result<LineSpan> {
        let mut bytes = serde_json::to_vec(line)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        self.file.write_all(&bytes)?;
        self.file.flush()?;
        let end = self.file.stream_position()?;
        let length = bytes.len() as u64;
        Ok(LineSpan {
            offset: end.saturating_sub(length),
            // The serialized line and its newline: at least one byte.
            length: std::num::NonZeroU64::new(length)
                .ok_or_else(|| std::io::Error::other("an empty session line"))?,
        })
    }
}

fn open_for_append(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
}

/// Whether a file's last byte is something other than a newline: a line an
/// interrupted writer left unfinished. Reads that one byte and nothing else.
fn ends_torn(file: &mut File) -> std::io::Result<bool> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(false);
    }
    let mut last = [0u8; 1];
    file.seek(SeekFrom::Start(len - 1))?;
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

/// Append one index entry to its segment: the segment named for the UTC date
/// the entry's id was minted on (else `fallback`), created by its date's first
/// append. One append-mode write of one complete line, after reading at most
/// the segment's last byte — so no lock, no duplicate check and no other read —
/// and a torn tail left by an interrupted writer is closed off in that same
/// write rather than swallowing this entry.
fn append_index_entry(
    dir: &Path,
    entry: &HistoryIndexEntry,
    fallback: Option<UtcDate>,
) -> std::io::Result<()> {
    let (kind, date) = entry.segment(fallback).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "an index entry with no date to name its segment",
        )
    })?;
    let segments = dir.join(INDEX_DIR);
    fs::create_dir_all(&segments)?;
    let mut file = open_for_append(&segments.join(kind.file_name(date)))?;
    let mut bytes = Vec::new();
    if ends_torn(&mut file)? {
        bytes.push(b'\n');
    }
    serde_json::to_writer(&mut bytes, entry)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    write_whole_line(&mut file, &bytes)
}

/// Append one pointer line as ONE write: the file is opened for append (so the
/// OS positions every write at the end, whoever else holds it open) and the
/// serialized line plus its newline go out in a single `write` call (see
/// [`write_whole_line`]), so two concurrent runs pointing at the same file
/// never interleave inside a line. The file is created on first append, with
/// its parent directory made if missing. No lock and no index: the pointer
/// file is a plain shared log.
fn append_pointer_line(path: &Path, pointer: &HistoryPointer) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec(pointer)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    write_whole_line(&mut file, &bytes)
}

/// Write `line` in exactly one `write` call, never a loop. `write_all` would
/// answer a short write with a second call, and between the two another
/// process's line can land on the shared file — splitting this one around it
/// into two torn pieces. A short write is instead reported as the failure it
/// is: what it left behind is one torn tail, which the reader already counts as
/// skipped, and the caller warns rather than pretending the line went out. An
/// `Interrupted` write wrote nothing, so asking again is the same one write.
fn write_whole_line(sink: &mut impl Write, line: &[u8]) -> std::io::Result<()> {
    let written = loop {
        match sink.write(line) {
            Ok(written) => break written,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    };
    if written != line.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WriteZero,
            format!(
                "short write: {written} of {} bytes of the pointer line",
                line.len()
            ),
        ));
    }
    Ok(())
}

/// What [`read_pointers`] read: every well-formed [`HistoryPointer`] line in
/// file order, and how many lines were not one.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, JsonSchema)]
pub struct HistoryPointers {
    /// The pointer lines, in the order they were appended.
    pub pointers: Vec<HistoryPointer>,
    /// Lines that were not one complete pointer object — a torn tail left by
    /// an interrupted writer, or a foreign line — counted rather than failing
    /// the read.
    pub skipped: usize,
}

/// The reader's result under the name the shared pointer contract gives it.
pub type Pointers = HistoryPointers;

/// Read a run's pointer file: the lines every harness run with history on
/// appended (see [`HistoryWriter::begin_harness_run`]). A missing file reads as
/// empty with nothing skipped; a line that does not parse as one complete
/// [`HistoryPointer`] — a torn tail, a foreign object, a line whose spellings
/// do not compose (see [`HistoryPointer`]) — is counted in `skipped` and never
/// fails the read, so a consumer reads a file that is still being appended to.
/// A line is complete only once its newline has landed: the writer sends the
/// object and its terminator as one write, so bytes at the end of the file
/// with no newline after them are a torn tail even when they happen to parse,
/// and are skipped rather than read as a record. A blank line is neither a
/// record nor a skipped one. Only a file that exists and cannot be read is an
/// error.
pub fn read_pointers(path: &Path) -> Result<HistoryPointers, OneharnessError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HistoryPointers::default());
        }
        Err(source) => {
            return Err(OneharnessError::HistoryIo {
                path: path.display().to_string(),
                source,
            });
        }
    };
    let mut read = HistoryPointers::default();
    // Decoded per line, not per file: a foreign line's bytes, or a tail torn
    // inside a multi-byte character, are that one line's to skip, never the
    // whole file's to refuse.
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        let Some(line) = line.strip_suffix(b"\n") else {
            // The file's last bytes, unterminated: a write still in flight or
            // one that ended short, never a record.
            read.skipped += 1;
            break;
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match std::str::from_utf8(line).map(serde_json::from_str::<HistoryPointer>) {
            Ok(Ok(pointer)) => read.pointers.push(pointer),
            Ok(Err(_)) | Err(_) => read.skipped += 1,
        }
    }
    Ok(read)
}

/// Outcome for one session file processed by [`migrate`]. Counts refer to
/// whole legacy records and current v1.0 lines; unreadable lines are preserved
/// byte-for-byte and reported as skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct MigrationSummary {
    // llmlint: ignore[invalid_states_unrepresentable] The session location is reported as its portable display string, which is what a JSON/SDK consumer reads back; this is an output projection, not a path this crate then does I/O through.
    pub path: String,
    pub records_migrated: usize,
    pub skipped: usize,
    pub already_current: usize,
}

/// The `oneharness history migrate` output contract.
///
/// A type rather than the inline `json!` literal it replaced, because an SDK
/// cannot validate a document that has no schema — which is what left this
/// capability's `output` unbacked in the capability manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct HistoryMigrateReport {
    /// One entry per session file the migration touched.
    pub files: Vec<MigrationSummary>,
    /// `files.len()`, carried so a consumer reading only the summary does not
    /// have to walk the array.
    pub files_processed: usize,
}

impl HistoryMigrateReport {
    /// Summarize what [`migrate`] did.
    #[must_use]
    pub fn new(files: Vec<MigrationSummary>) -> Self {
        HistoryMigrateReport {
            files_processed: files.len(),
            files,
        }
    }
}

/// The `oneharness history clear` output contract.
///
/// `clear` is a dry run until `--yes`, and the two phases have always printed
/// *different* documents: a real run reports `removed`, a dry run reports
/// `would_remove` plus the hint that makes it real. That difference is the
/// contract, so this is a sum of the two frames rather than one struct with
/// both counts optional — which would let a consumer read a deletion count off
/// a run that deleted nothing. Untagged, because `dry_run` is the discriminant
/// the published shape already carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum HistoryClearReport {
    Removed(HistoryClearRemoved),
    DryRun(HistoryClearDryRun),
}

/// The frame a `--yes` run prints: these files are gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct HistoryClearRemoved {
    pub removed: usize,
    // llmlint: ignore[invalid_states_unrepresentable] These are portable display strings for a JSON/SDK consumer, the same projection `SessionSummary::path` already publishes; they are never read back as paths this crate acts on.
    pub files: Vec<String>,
    /// Always `false` — the field is what tells the two frames apart, so it is
    /// a literal rather than a flag either constructor could get wrong.
    pub dry_run: LiteralFalse,
}

/// The frame a run without `--yes` prints: these files *would* go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct HistoryClearDryRun {
    pub would_remove: usize,
    // llmlint: ignore[invalid_states_unrepresentable] Same output-only projection as the sibling frame above; the deletion itself works from the typed paths `remove_sessions` walked, never from these strings.
    pub files: Vec<String>,
    pub dry_run: LiteralTrue,
    /// How to make the dry run real. Constant text, carried in the document so
    /// a JSON consumer gets the same guidance the text view prints.
    pub hint: &'static str,
}

impl HistoryClearReport {
    /// What [`remove_sessions`] deleted.
    #[must_use]
    pub fn removed(files: Vec<String>) -> Self {
        HistoryClearReport::Removed(HistoryClearRemoved {
            removed: files.len(),
            files,
            dry_run: LiteralFalse,
        })
    }

    /// What a `--yes` re-run would delete, having deleted nothing.
    #[must_use]
    pub fn dry_run(files: Vec<String>) -> Self {
        HistoryClearReport::DryRun(HistoryClearDryRun {
            would_remove: files.len(),
            files,
            dry_run: LiteralTrue,
            hint: "re-run with --yes to delete",
        })
    }
}

/// Rewrite every legacy session in a history store to v1.0. Each session is
/// replaced from a fully flushed sibling temp file, so a failed conversion never
/// leaves a partially written target. Session files are all it writes: no
/// segment and no legacy index file is touched (a migrated legacy run reaches
/// the dated index through [`reindex`]).
pub fn migrate(dir: &Path) -> Result<Vec<MigrationSummary>, OneharnessError> {
    let mut summaries = Vec::new();
    for project_dir in read_subdirs_if_present(dir)? {
        for path in read_session_files(&project_dir)? {
            summaries.push(migrate_file(dir, &path)?);
        }
    }
    summaries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(summaries)
}

fn migrate_file(dir: &Path, path: &Path) -> Result<MigrationSummary, OneharnessError> {
    let text = fs::read_to_string(path).map_err(|source| history_io_error(path, source))?;
    let relative = path.strip_prefix(dir).unwrap_or(path).display().to_string();
    let mut output = Vec::new();
    let mut summary = MigrationSummary {
        path: path.display().to_string(),
        records_migrated: 0,
        skipped: 0,
        already_current: 0,
    };
    for (index, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            summary.skipped += 1;
            output.extend_from_slice(raw.as_bytes());
            output.push(b'\n');
            continue;
        };
        if serde_json::from_value::<HistoryLine>(value.clone()).is_ok() {
            summary.already_current += 1;
            output.extend_from_slice(raw.as_bytes());
            output.push(b'\n');
            continue;
        }
        let identity = format!("{}:{}", relative, index + 1);
        let Ok(record) = HistoryRecord::from_legacy_value(value, &identity) else {
            summary.skipped += 1;
            output.extend_from_slice(raw.as_bytes());
            output.push(b'\n');
            continue;
        };
        if let Some(events) = &record.events {
            for event in events {
                append_json_line(
                    &mut output,
                    &HistoryLine::Event(HistoryEventLine {
                        schema_version: history::EVENT_SCHEMA_VERSION.to_string(),
                        run_id: record.history_id,
                        harness: record.harness.clone(),
                        variant: record.variant.clone(),
                        harness_id: Some(record.harness_id.clone()),
                        event: event.clone(),
                        session_name: None,
                    }),
                )?;
            }
        }
        append_json_line(
            &mut output,
            &HistoryLine::Run(HistoryRunRecord::from_record(&record)),
        )?;
        summary.records_migrated += 1;
    }
    atomic_write(path, &output)?;
    Ok(summary)
}

fn append_json_line<T: Serialize>(output: &mut Vec<u8>, value: &T) -> Result<(), OneharnessError> {
    serde_json::to_writer(&mut *output, value).map_err(OneharnessError::Serialize)?;
    output.push(b'\n');
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), OneharnessError> {
    let tmp = path.with_extension(format!("jsonl.{}.oneharness.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|source| history_io_error(path, source))
}

/// The entry shape an older core writes to [`LEGACY_INDEX_FILE`]: the whole
/// closing record. Read only — by the all-time readers and a watcher's tail.
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct LegacyIndexEntry {
    session_path: LegacyEntryPath,
    record: HistoryRunRecord,
}

/// The entry shape an older core writes to [`LEGACY_EVENT_INDEX_FILE`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct LegacyEventIndexEntry {
    session_path: LegacyEntryPath,
    labels: HistoryLabels,
    line: HistoryEventLine,
}

/// The session file a legacy entry names — or, for one pointing into the
/// index directory, which no released core wrote, an error naming the path it
/// gives, the way an unreadable segment fails a read: refused, never skipped.
fn legacy_session<'a>(
    dir: &Path,
    path: &'a LegacyEntryPath,
) -> Result<&'a LegacySessionPath, OneharnessError> {
    match path {
        LegacyEntryPath::Session(session) => Ok(session),
        LegacyEntryPath::IntoIndexDir(text) => Err(history_io_error(
            &dir.join(text),
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "a legacy index entry names this path inside `{INDEX_DIR}/`, which holds \
                     index segments and no session a released core wrote; it is refused"
                ),
            ),
        )),
    }
}

/// Stream the complete lines of an index file from byte `offset`, handing each
/// non-blank one to `each` until it breaks. Returns the offset just past the
/// last complete line read — bytes after it with no newline yet are a write
/// still in flight (or a torn tail) and are left for the next read — or `None`
/// when the file does not exist. The file is opened read-only, and memory is
/// bounded by one line whatever the file's size. A file that exists and cannot
/// be read is an error naming its path.
fn stream_lines(
    path: &Path,
    offset: u64,
    mut each: impl FnMut(&[u8]) -> ControlFlow<()>,
) -> Result<Option<u64>, OneharnessError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(history_io_error(path, source)),
    };
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|source| history_io_error(path, source))?;
    let mut position = offset;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|source| history_io_error(path, source))?;
        if read == 0 || line.last() != Some(&b'\n') {
            return Ok(Some(position));
        }
        position += read as u64;
        let body = &line[..line.len() - 1];
        if body.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if each(body).is_break() {
            return Ok(Some(position));
        }
    }
}

/// Every segment in the store's index directory, by date then kind. One
/// directory listing; a store with no index yet has none.
fn list_segments(dir: &Path) -> Result<Vec<(UtcDate, SegmentKind, PathBuf)>, OneharnessError> {
    let segments = dir.join(INDEX_DIR);
    let entries = match fs::read_dir(&segments) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(history_io_error(&segments, source)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| history_io_error(&segments, source))?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if let Some((kind, date)) = SegmentKind::parse_file_name(&name) {
            found.push((date, kind, entry.path()));
        }
    }
    found.sort();
    Ok(found)
}

fn segment_path(dir: &Path, kind: SegmentKind, date: UtcDate) -> PathBuf {
    dir.join(INDEX_DIR).join(kind.file_name(date))
}

/// Read the one line a span names and parse it, when it is a whole line. The
/// span is read from disk, so it is trusted only as far as the session file
/// reaches — one that runs past the file's end is refused — and its `length`
/// is never allocated up front: the bytes are parsed as they are read, so what
/// is held is what the file actually holds there, and a span that does not
/// open on a record fails at its first byte however long it claims to be.
fn read_span(path: &Path, span: LineSpan) -> Option<HistoryLine> {
    let mut file = File::open(path).ok()?;
    let end = span.offset.checked_add(span.length.get())?;
    if end > file.metadata().ok()?.len() {
        return None;
    }
    file.seek(SeekFrom::Start(end - 1)).ok()?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last).ok()?;
    if last != *b"\n" {
        return None;
    }
    file.seek(SeekFrom::Start(span.offset)).ok()?;
    // The closing newline is JSON whitespace, and anything past the one value
    // the span holds — part of a neighbouring line — is refused as trailing.
    serde_json::from_reader(BufReader::new(file.take(span.length.get()))).ok()
}

/// Find one line of a session file: at its span when the span holds it, else
/// by reading that one session file line by line. `None` when the file is gone
/// or holds no such line.
fn find_session_line(
    path: &Path,
    span: Option<LineSpan>,
    wanted: impl Fn(&HistoryLine) -> bool,
) -> Result<Option<HistoryLine>, OneharnessError> {
    if let Some(line) = span.and_then(|span| read_span(path, span)) {
        if wanted(&line) {
            return Ok(Some(line));
        }
    }
    let mut found = None;
    stream_lines(path, 0, |raw| {
        match serde_json::from_slice::<HistoryLine>(raw) {
            Ok(line) if wanted(&line) => {
                found = Some(line);
                ControlFlow::Break(())
            }
            _ => ControlFlow::Continue(()),
        }
    })?;
    Ok(found)
}

fn find_run_line(
    path: &Path,
    id: HistoryId,
    span: Option<LineSpan>,
) -> Result<Option<HistoryRunRecord>, OneharnessError> {
    Ok(find_session_line(
        path,
        span,
        |line| matches!(line, HistoryLine::Run(run) if run.history_id == id),
    )?
    .and_then(|line| match line {
        HistoryLine::Run(run) => Some(run),
        HistoryLine::Event(_) | HistoryLine::RunStarted(_) => None,
    }))
}

fn find_event_line(
    path: &Path,
    run_id: HistoryId,
    event_index: usize,
    span: Option<LineSpan>,
) -> Result<Option<HistoryEventLine>, OneharnessError> {
    Ok(find_session_line(path, span, |line| {
        matches!(line, HistoryLine::Event(event) if event.run_id == run_id && event.event.index == event_index)
    })?
    .and_then(|line| match line {
        HistoryLine::Event(event) => Some(event),
        HistoryLine::Run(_) | HistoryLine::RunStarted(_) => None,
    }))
}

/// A reindex spill: the candidate entries for one segment, kept on disk until
/// their segment is reconciled, so memory never holds the store's entries.
struct Spill {
    root: crate::io::scratch::ScratchDir,
    open: HashMap<(SegmentKind, UtcDate), BufWriter<File>>,
}

impl Spill {
    /// At most this many spill files stay open at once; the rest are reopened
    /// on demand, so a store spanning years never exhausts descriptors.
    const OPEN_LIMIT: usize = 64;

    fn new() -> std::io::Result<Spill> {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let tag = format!(
            "reindex-{}-{}",
            now_epoch_secs(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        Ok(Spill {
            root: crate::io::scratch::ScratchDir::new(&tag)?,
            open: HashMap::new(),
        })
    }

    fn path(&self, kind: SegmentKind, date: UtcDate) -> PathBuf {
        self.root.path().join(kind.file_name(date))
    }

    fn push(
        &mut self,
        kind: SegmentKind,
        date: UtcDate,
        entry: &HistoryIndexEntry,
    ) -> std::io::Result<()> {
        if !self.open.contains_key(&(kind, date)) {
            if self.open.len() >= Self::OPEN_LIMIT {
                for (_, mut writer) in self.open.drain() {
                    writer.flush()?;
                }
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.path(kind, date))?;
            self.open.insert((kind, date), BufWriter::new(file));
        }
        let writer = self
            .open
            .get_mut(&(kind, date))
            .expect("the spill writer was just opened");
        serde_json::to_writer(&mut *writer, entry)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        writer.write_all(b"\n")
    }

    /// Close every writer and name each spilled segment, in name order.
    fn finish(&mut self) -> std::io::Result<Vec<(SegmentKind, UtcDate)>> {
        for (_, mut writer) in self.open.drain() {
            writer.flush()?;
        }
        let mut spilled = Vec::new();
        for entry in fs::read_dir(self.root.path())? {
            let name = entry?.file_name();
            if let Some(segment) = name.to_str().and_then(SegmentKind::parse_file_name) {
                spilled.push(segment);
            }
        }
        spilled.sort_by_key(|(kind, date)| kind.file_name(*date));
        Ok(spilled)
    }
}

/// What [`reindex`] appended to one segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SegmentReindexSummary {
    /// The segment's file name (`runs-YYYY-MM-DD.ndjson` or
    /// `events-YYYY-MM-DD.ndjson`).
    pub segment: String,
    /// The segment's path, as a display string.
    // llmlint: ignore[invalid_states_unrepresentable] An output projection for a JSON/SDK consumer, like `SessionSummary::path`; reindex appends through the typed path it built, never through this string.
    pub path: String,
    /// How many entries this run appended to it.
    pub added: usize,
}

/// A session file [`reindex`] could not read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct UnreadableSessionFile {
    // llmlint: ignore[invalid_states_unrepresentable] The unreadable file's portable display string, reported to a JSON/SDK consumer; nothing reads a file back through it.
    pub path: String,
    /// Why it could not be read, as the operating system said it.
    pub error: String,
}

/// The `oneharness history reindex` output contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct HistoryReindexReport {
    /// One row per segment this run appended to, in file-name order; a
    /// segment that already held every entry is not listed.
    pub segments: Vec<SegmentReindexSummary>,
    /// The sum of `segments[].added`.
    pub entries_added: usize,
    /// How many session files were read.
    pub files_read: usize,
    /// Every session file (or project directory) that could not be read,
    /// with why. The rest are indexed regardless.
    pub unreadable: Vec<UnreadableSessionFile>,
}

/// Index every session line the index lacks: stream every session file in the
/// store, and append one entry for each run or event line its date's segment
/// does not hold yet. The only code path that walks the session tree to build
/// index entries, and never called implicitly.
///
/// Idempotent — a second run appends nothing — and append-only: a segment's
/// existing bytes stay its leading bytes, and no legacy index file or session
/// file is written. A line's date is its run id's; a legacy UUIDv5 id, which
/// carries none, takes the date in its session id (else, for a run line, its
/// record's `timestamp`). A file that cannot be read is named in the report and
/// skipped, never failing the rest.
///
/// Memory does not grow with the store: candidates are spilled to a scratch
/// directory per segment, then each segment is reconciled against the keys it
/// already holds by an external sort, one date at a time — so a date holding
/// ten times the sessions costs ten times the scratch space, not the memory.
pub fn reindex(dir: &Path) -> Result<HistoryReindexReport, OneharnessError> {
    let mut report = HistoryReindexReport {
        segments: Vec::new(),
        entries_added: 0,
        files_read: 0,
        unreadable: Vec::new(),
    };
    if !dir.exists() {
        return Ok(report);
    }
    let scratch_error = |source| history_io_error(&std::env::temp_dir(), source);
    let mut spill = Spill::new().map_err(scratch_error)?;
    for project_dir in read_subdirs(dir)? {
        let unreadable_dir = |source: std::io::Error| UnreadableSessionFile {
            path: project_dir.display().to_string(),
            error: source.to_string(),
        };
        // Streamed rather than collected, so a directory's size costs no memory.
        let files = match fs::read_dir(&project_dir) {
            Ok(files) => files,
            Err(source) => {
                report.unreadable.push(unreadable_dir(source));
                continue;
            }
        };
        let Some(slug) = project_dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        for file in files {
            let path = match file {
                Ok(file) => file.path(),
                Err(source) => {
                    report.unreadable.push(unreadable_dir(source));
                    break;
                }
            };
            if path.extension().and_then(|ext| ext.to_str()) != Some(SESSION_EXT) {
                continue;
            }
            match spill_session(&path, slug, &mut spill) {
                Ok(unmigrated) => {
                    report.files_read += 1;
                    if unmigrated && !UNMIGRATED_REPORTED.swap(true, Ordering::Relaxed) {
                        eprintln!(
                            "oneharness: warning: skipped unmigrated history lines in `{}`; run `oneharness history migrate`",
                            path.display()
                        );
                    }
                }
                Err(SpillError::Session(source)) => report.unreadable.push(UnreadableSessionFile {
                    path: path.display().to_string(),
                    error: source.to_string(),
                }),
                Err(SpillError::Scratch(source)) => return Err(scratch_error(source)),
            }
        }
    }
    report.unreadable.sort_by(|a, b| a.path.cmp(&b.path));
    for (kind, date) in spill.finish().map_err(scratch_error)? {
        let added = reconcile_segment(dir, kind, date, &spill.path(kind, date), spill.root.path())?;
        if added > 0 {
            report.entries_added += added;
            report.segments.push(SegmentReindexSummary {
                segment: kind.file_name(date),
                path: segment_path(dir, kind, date).display().to_string(),
                added,
            });
        }
    }
    Ok(report)
}

/// The UTC date a record's `timestamp` names. It is read from a session file,
/// so it is validated as an RFC 3339 UTC instant first; anything else names
/// no date.
fn timestamp_date(timestamp: &str) -> Option<UtcDate> {
    timestamp
        .parse::<UtcInstant>()
        .ok()
        .and_then(|instant| UtcDate::of_instant(&instant))
}

enum SpillError {
    /// The session file could not be read: report it and go on.
    Session(std::io::Error),
    /// The scratch space could not be written: nothing can go on.
    Scratch(std::io::Error),
}

/// Spill one session file's candidate entries, saying whether it holds
/// unmigrated legacy lines (which are not indexed). Its labels live on its run
/// lines, which follow the events they label, so the first run line is found
/// first; then every complete line is streamed with its span.
fn spill_session(path: &Path, slug: &str, spill: &mut Spill) -> Result<bool, SpillError> {
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(false);
    };
    let open = || File::open(path).map(|file| BufReader::with_capacity(64 * 1024, file));
    let mut labels = HistoryLabels::default();
    let mut first_run_date = None;
    let mut unmigrated = false;
    let mut reader = open().map_err(SpillError::Session)?;
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(SpillError::Session)?
            == 0
        {
            break;
        }
        if let Ok(HistoryLine::Run(run)) = serde_json::from_slice::<HistoryLine>(&line) {
            first_run_date = timestamp_date(&run.timestamp);
            labels = run.labels;
            break;
        }
    }
    let Some(session_path) = index::SessionPath::new(slug, stem) else {
        return Ok(false);
    };
    let session_date = index::session_date_from_id(stem);
    let event_fallback = session_date.or(first_run_date);
    let mut reader = open().map_err(SpillError::Session)?;
    let mut offset = 0u64;
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(SpillError::Session)?;
        if read == 0 || line.last() != Some(&b'\n') {
            return Ok(unmigrated);
        }
        // `read` counts the newline just checked for, so it is never zero.
        let Some(length) = std::num::NonZeroU64::new(read as u64) else {
            continue;
        };
        let span = LineSpan { offset, length };
        offset += read as u64;
        let (entry, fallback) = match serde_json::from_slice::<HistoryLine>(&line) {
            Ok(HistoryLine::Run(run)) => {
                // A closing line whose timestamp is not an instant names no
                // time it was recorded at; it is left out rather than guessed.
                let Ok(recorded_at) = run.timestamp.parse::<UtcInstant>() else {
                    continue;
                };
                let fallback = session_date.or_else(|| UtcDate::of_instant(&recorded_at));
                let harness_id = run.harness_id.clone().unwrap_or_else(|| {
                    run.variant.as_ref().map_or(run.harness.clone(), |variant| {
                        format!("{}:{variant}", run.harness)
                    })
                });
                (
                    HistoryIndexEntry::Run(RunIndexEntry {
                        schema_version: INDEX_SCHEMA_VERSION.to_string(),
                        history_id: run.history_id,
                        session_path: session_path.clone(),
                        session: stem.to_string(),
                        name: run.name,
                        project_slug: slug.to_string(),
                        harness_id,
                        labels: run.labels,
                        recorded_at,
                        span: Some(span),
                    }),
                    fallback,
                )
            }
            Ok(HistoryLine::Event(event)) => (
                HistoryIndexEntry::Event(EventIndexEntry {
                    schema_version: INDEX_SCHEMA_VERSION.to_string(),
                    run_id: event.run_id,
                    event_index: event.event.index,
                    session_path: session_path.clone(),
                    project_slug: slug.to_string(),
                    harness_id: event.harness_id.unwrap_or(event.harness),
                    labels: labels.clone(),
                    span: Some(span),
                }),
                event_fallback,
            ),
            Ok(HistoryLine::RunStarted(_)) => continue,
            Err(_) => {
                unmigrated |= serde_json::from_slice::<Value>(&line)
                    .is_ok_and(|value| value.is_object() && value.get("type").is_none());
                continue;
            }
        };
        if let Some((kind, date)) = entry.segment(fallback) {
            spill
                .push(kind, date, &entry)
                .map_err(SpillError::Scratch)?;
        }
    }
}

/// Append to one segment every spilled entry it lacks, returning how many
/// went out. The segment's keys and the spilled candidates are merged by an
/// external sort under `scratch`, so memory holds one sort chunk however many
/// entries the date has; the segment's bytes are never rewritten. Its new
/// entries go out in key order.
fn reconcile_segment(
    dir: &Path,
    kind: SegmentKind,
    date: UtcDate,
    spilled: &Path,
    scratch: &Path,
) -> Result<usize, OneharnessError> {
    let target = segment_path(dir, kind, date);
    let scratch_error = |source| history_io_error(scratch, source);
    let mut sort = ExternalSort::new(scratch, ExternalSort::CHUNK_BYTES, ExternalSort::FAN_IN)
        .map_err(scratch_error)?;
    let mut sort_failure = None;
    for (path, held) in [(target.as_path(), true), (spilled, false)] {
        stream_lines(path, 0, |line| {
            let Ok(entry) = serde_json::from_slice::<HistoryIndexEntry>(line) else {
                return ControlFlow::Continue(());
            };
            let pushed = sort.push(entry.key(), if held { None } else { Some(line) });
            match pushed {
                Ok(()) => ControlFlow::Continue(()),
                Err(source) => {
                    sort_failure = Some(source);
                    ControlFlow::Break(())
                }
            }
        })?;
        if let Some(source) = sort_failure.take() {
            return Err(scratch_error(source));
        }
    }
    let mut writer: Option<File> = None;
    let mut added = 0usize;
    let mut write_failure = None;
    let mut current = [0u8; SORT_KEY_LEN];
    let mut settled = false;
    sort.finish(|record| {
        let (key, rest) = record.split_at(SORT_KEY_LEN);
        if key != current {
            current.copy_from_slice(key);
            settled = false;
        }
        // The segment's own record sorts first within a key, so a key it holds
        // settles before any candidate for it is seen.
        if settled {
            return ControlFlow::Continue(());
        }
        settled = true;
        let Some((&SORT_CANDIDATE, line)) = rest.split_first() else {
            return ControlFlow::Continue(());
        };
        let written = (|| {
            let file = match &mut writer {
                Some(file) => file,
                None => {
                    fs::create_dir_all(dir.join(INDEX_DIR))?;
                    let mut file = open_for_append(&target)?;
                    if ends_torn(&mut file)? {
                        write_whole_line(&mut file, b"\n")?;
                    }
                    writer.insert(file)
                }
            };
            let mut bytes = line.to_vec();
            bytes.push(b'\n');
            write_whole_line(file, &bytes)
        })();
        match written {
            Ok(()) => {
                added += 1;
                ControlFlow::Continue(())
            }
            Err(source) => {
                write_failure = Some(source);
                ControlFlow::Break(())
            }
        }
    })
    .map_err(scratch_error)?;
    match write_failure {
        Some(source) => Err(history_io_error(&target, source)),
        None => Ok(added),
    }
}

/// The width of a sort record's key: a kind byte, the hyphenated id, and a
/// zero-padded event index (zero for a run), so every key is one width.
const SORT_KEY_LEN: usize = 1 + 36 + 20;
/// The byte after the key marking a key the segment already holds; it sorts
/// before [`SORT_CANDIDATE`].
const SORT_HELD: u8 = b'0';
/// The byte after the key marking a spilled candidate, followed by its line.
const SORT_CANDIDATE: u8 = b'1';

fn sort_key(key: IndexKey) -> String {
    match key {
        IndexKey::Run(id) => format!("r{id}{:020}", 0),
        IndexKey::Event(id, index) => format!("e{id}{index:020}"),
    }
}

/// A bounded-memory external sort of reindex records: `<key><0>` for a key a
/// segment holds, `<key><1><entry line>` for a candidate. Records are sorted in
/// memory up to `chunk_bytes`, written out as a sorted run, and runs are merged
/// `fan_in` at a time level by level, so memory holds one chunk plus `fan_in`
/// read buffers however many records go in.
struct ExternalSort {
    root: crate::io::scratch::ScratchDir,
    chunk_bytes: usize,
    fan_in: usize,
    buffer: Vec<Vec<u8>>,
    buffered: usize,
    /// Sorted runs by level; a level reaching `fan_in` merges into the next.
    levels: Vec<Vec<PathBuf>>,
    written: usize,
}

impl ExternalSort {
    const CHUNK_BYTES: usize = 4 << 20;
    const FAN_IN: usize = 16;
    /// What one buffered record costs beyond its bytes: its `Vec` header and
    /// allocation.
    const RECORD_OVERHEAD: usize = 48;

    fn new(parent: &Path, chunk_bytes: usize, fan_in: usize) -> std::io::Result<ExternalSort> {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let tag = format!("sort-{}", SEQ.fetch_add(1, Ordering::Relaxed));
        Ok(ExternalSort {
            root: crate::io::scratch::ScratchDir::under(parent, &tag)?,
            chunk_bytes,
            fan_in: fan_in.max(2),
            buffer: Vec::new(),
            buffered: 0,
            levels: Vec::new(),
            written: 0,
        })
    }

    fn push(&mut self, key: IndexKey, line: Option<&[u8]>) -> std::io::Result<()> {
        let key = sort_key(key);
        let mut record = Vec::with_capacity(SORT_KEY_LEN + 1 + line.map_or(0, <[u8]>::len));
        record.extend_from_slice(key.as_bytes());
        match line {
            None => record.push(SORT_HELD),
            Some(line) => {
                record.push(SORT_CANDIDATE);
                record.extend_from_slice(line);
            }
        }
        self.buffered += record.len() + Self::RECORD_OVERHEAD;
        self.buffer.push(record);
        if self.buffered >= self.chunk_bytes {
            self.flush()?;
        }
        Ok(())
    }

    fn next_path(&mut self) -> PathBuf {
        self.written += 1;
        self.root.path().join(format!("run-{}", self.written))
    }

    /// Write the buffer out as one sorted run at level zero, cascading any
    /// level that reaches `fan_in` into one run at the next.
    fn flush(&mut self) -> std::io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        self.buffer.sort_unstable();
        let path = self.next_path();
        let mut out = BufWriter::new(File::create(&path)?);
        for record in self.buffer.drain(..) {
            out.write_all(&record)?;
            out.write_all(b"\n")?;
        }
        out.flush()?;
        self.buffered = 0;
        self.buffer = Vec::new();
        let mut level = 0;
        let mut run = path;
        loop {
            if self.levels.len() == level {
                self.levels.push(Vec::new());
            }
            self.levels[level].push(run);
            if self.levels[level].len() < self.fan_in {
                return Ok(());
            }
            let inputs = std::mem::take(&mut self.levels[level]);
            run = self.merge_to_file(&inputs)?;
            level += 1;
        }
    }

    fn merge_to_file(&mut self, inputs: &[PathBuf]) -> std::io::Result<PathBuf> {
        let path = self.next_path();
        let mut out = BufWriter::new(File::create(&path)?);
        let mut failure = None;
        merge_runs(inputs, |record| {
            match out.write_all(record).and_then(|()| out.write_all(b"\n")) {
                Ok(()) => ControlFlow::Continue(()),
                Err(error) => {
                    failure = Some(error);
                    ControlFlow::Break(())
                }
            }
        })?;
        if let Some(error) = failure {
            return Err(error);
        }
        out.flush()?;
        for input in inputs {
            fs::remove_file(input)?;
        }
        Ok(path)
    }

    /// Hand every record to `each` in sorted order, merging at most `fan_in`
    /// runs at once.
    fn finish(mut self, each: impl FnMut(&[u8]) -> ControlFlow<()>) -> std::io::Result<()> {
        self.flush()?;
        let mut runs: VecDeque<PathBuf> = self.levels.drain(..).flatten().collect();
        while runs.len() > self.fan_in {
            let inputs: Vec<PathBuf> = runs.drain(..self.fan_in).collect();
            let merged = self.merge_to_file(&inputs)?;
            runs.push_back(merged);
        }
        merge_runs(runs.make_contiguous(), each)
    }
}

/// A k-way merge of sorted runs of newline-terminated records.
fn merge_runs(
    inputs: &[PathBuf],
    mut each: impl FnMut(&[u8]) -> ControlFlow<()>,
) -> std::io::Result<()> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let next = |reader: &mut BufReader<File>| -> std::io::Result<Option<Vec<u8>>> {
        let mut record = Vec::new();
        if reader.read_until(b'\n', &mut record)? == 0 {
            return Ok(None);
        }
        if record.last() == Some(&b'\n') {
            record.pop();
        }
        Ok(Some(record))
    };
    let mut readers = Vec::with_capacity(inputs.len());
    let mut heap = BinaryHeap::with_capacity(inputs.len());
    for (source, path) in inputs.iter().enumerate() {
        let mut reader = BufReader::with_capacity(64 * 1024, File::open(path)?);
        if let Some(record) = next(&mut reader)? {
            heap.push(Reverse((record, source)));
        }
        readers.push(reader);
    }
    while let Some(Reverse((record, source))) = heap.pop() {
        if each(&record).is_break() {
            return Ok(());
        }
        if let Some(record) = next(&mut readers[source])? {
            heap.push(Reverse((record, source)));
        }
    }
    Ok(())
}

/// A resumable reader over the dated index. It reads the segments dated from
/// its start on — its cursor's date, its window's first date, or today — and
/// on each [`poll`](Self::poll) lists the index directory once and tails every
/// such segment by byte offset, a segment created after it opened included.
/// The day before its start is tailed from its size at open, so a run begun
/// just before midnight UTC that closes after the watcher opened is still
/// followed. While the legacy index files exist it tails them from their size
/// at open (from the beginning under [`HistoryWindow::AllTime`]), so what an
/// older core appends is followed too; a record reached both ways is emitted
/// once. Its memory grows only with the records it has emitted.
pub struct HistoryWatcher {
    dir: PathBuf,
    /// The first date whose segments it reads from the beginning; `None`
    /// reads every segment.
    earliest: Option<UtcDate>,
    /// The date before `earliest`, tailed from its size at open.
    lookback: Option<UtcDate>,
    offsets: BTreeMap<(SegmentKind, UtcDate), u64>,
    legacy_offsets: [u64; 2],
    events: bool,
    /// Events of runs begun at or before the cursor were emitted before it.
    events_after: Option<HistoryId>,
    pending: VecDeque<HistoryRecord>,
    pending_events: VecDeque<HistoryEventLine>,
    seen: HashSet<HistoryId>,
    labels: HistoryLabels,
    project_slug: Option<String>,
    session: Option<SessionFilter>,
}

/// The one session a watcher follows: a session id, given or resolved from a
/// name — held there, so a later session reusing the name is not mixed in — or
/// a name no session in scope carries yet, awaiting the first that does.
#[derive(Debug)]
enum SessionFilter {
    Following(HistorySessionId),
    Awaiting(HistorySessionName),
}

/// Where a [`HistoryWatcher`] begins: strictly after a record it already
/// emitted, or from the beginning of a window. The two are one answer, so a
/// watch cannot be asked to start at both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchStart {
    /// Resume strictly after this record, from its date's runs segment.
    After(HistoryId),
    /// Start at the beginning of this window's earliest date.
    Window(HistoryWindow),
}

impl HistoryWatcher {
    /// Prepare to emit records strictly after `after` — the entries after the
    /// cursor in its date's segment, and every later-dated segment — or, with
    /// no cursor, every record of the current UTC day so far. A cursor its
    /// date's segment does not hold is [`OneharnessError::HistoryNotFound`].
    pub fn open(
        dir: &Path,
        after: Option<HistoryId>,
        labels: HistoryLabels,
        project_slug: Option<String>,
        events: bool,
    ) -> Result<Self, OneharnessError> {
        Self::open_in(
            dir,
            after.map(WatchStart::After),
            labels,
            project_slug,
            events,
            None,
        )
    }

    /// [`open`](Self::open), narrowed to one session when `session` names one:
    /// a session id (its file stem) selects that session; any other value is a
    /// session name, which selects the newest session carrying it in scope
    /// whose labels match `labels` — a run still in its first turn included,
    /// by the labels its events were indexed under — or, when none exists yet, the
    /// first matching one to appear. A name is non-unique, so the labels pick
    /// among its sessions rather than filtering only the newest one to nothing.
    /// Labels and project scope still apply on top.
    pub fn open_session(
        dir: &Path,
        after: Option<HistoryId>,
        labels: HistoryLabels,
        project_slug: Option<String>,
        events: bool,
        session: Option<&HistorySessionSelector>,
    ) -> Result<Self, OneharnessError> {
        Self::open_in(
            dir,
            after.map(WatchStart::After),
            labels,
            project_slug,
            events,
            session,
        )
    }

    /// [`open_session`](Self::open_session), starting where `start` says:
    /// after a cursor, or from the beginning of a window's first date
    /// ([`HistoryWindow::AllTime`] reads every segment, and the legacy index
    /// files from their first byte). `None` starts at the current UTC day.
    pub fn open_in(
        dir: &Path,
        start: Option<WatchStart>,
        labels: HistoryLabels,
        project_slug: Option<String>,
        events: bool,
        session: Option<&HistorySessionSelector>,
    ) -> Result<Self, OneharnessError> {
        let today = today();
        let mut offsets = BTreeMap::new();
        let after = match start {
            Some(WatchStart::After(cursor)) => Some(cursor),
            _ => None,
        };
        let (earliest, window) = match start {
            Some(WatchStart::After(cursor)) => {
                let not_found = || OneharnessError::HistoryNotFound {
                    id: cursor.to_string(),
                };
                let date = UtcDate::of_history_id(cursor).ok_or_else(not_found)?;
                let mut found = false;
                let path = segment_path(dir, SegmentKind::Runs, date);
                let end = stream_lines(&path, 0, |line| {
                    match serde_json::from_slice::<HistoryIndexEntry>(line) {
                        Ok(HistoryIndexEntry::Run(run)) if run.history_id == cursor => {
                            found = true;
                            ControlFlow::Break(())
                        }
                        _ => ControlFlow::Continue(()),
                    }
                })?;
                let after_cursor = end.filter(|_| found).ok_or_else(not_found)?;
                offsets.insert((SegmentKind::Runs, date), after_cursor);
                (Some(date), HistoryWindow::Since(date))
            }
            Some(WatchStart::Window(window)) => (window.earliest(today), window),
            None => {
                let window = HistoryWindow::Recent {
                    days: NonZeroU32::MIN,
                };
                (window.earliest(today), window)
            }
        };
        let lookback = earliest.map(|date| date.add_days(-1));
        if let Some(date) = lookback {
            for kind in [SegmentKind::Runs, SegmentKind::Events] {
                if let Ok(meta) = fs::metadata(segment_path(dir, kind, date)) {
                    offsets.insert((kind, date), meta.len());
                }
            }
        }
        let legacy_start = |name: &str| {
            if window.reads_legacy() {
                0
            } else {
                fs::metadata(dir.join(name)).map_or(0, |meta| meta.len())
            }
        };
        let session = match session {
            Some(HistorySessionSelector::Id(id)) => Some(SessionFilter::Following(id.clone())),
            Some(HistorySessionSelector::Name(name)) => {
                let mut id = None;
                for found in collect_sessions(dir, project_slug.as_deref(), window)? {
                    if found.summary.name != name.as_str() {
                        continue;
                    }
                    // A file stem no writer could have minted names no
                    // session this watcher can follow.
                    let Ok(found_id) = found.summary.id.parse::<HistorySessionId>() else {
                        continue;
                    };
                    let matched = if found.summary.record_count == 0 {
                        // A session still in its first turn has no closing
                        // record to state its labels yet; its event entries
                        // carry them.
                        found
                            .event_labels
                            .as_ref()
                            .is_none_or(|known| known.matches(&labels))
                    } else {
                        found.summary.labels.matches(&labels)
                    };
                    if matched {
                        id = Some(found_id);
                        break;
                    }
                }
                Some(id.map_or_else(
                    || SessionFilter::Awaiting(name.clone()),
                    SessionFilter::Following,
                ))
            }
            None => None,
        };
        let mut watcher = Self {
            dir: dir.to_path_buf(),
            earliest,
            lookback,
            offsets,
            legacy_offsets: [
                legacy_start(LEGACY_EVENT_INDEX_FILE),
                legacy_start(LEGACY_INDEX_FILE),
            ],
            events,
            events_after: after,
            pending: VecDeque::new(),
            pending_events: VecDeque::new(),
            seen: HashSet::new(),
            labels,
            project_slug,
            session,
        };
        watcher.scan()?;
        Ok(watcher)
    }

    /// Return all records currently available, preserving append order.
    pub fn drain_available(&mut self) -> Vec<HistoryRecord> {
        self.pending.drain(..).collect()
    }

    pub fn drain_events(&mut self) -> Vec<HistoryEventLine> {
        self.pending_events.drain(..).collect()
    }

    /// Read newly appended complete index lines. A concurrent partial write is
    /// retained at the current offset and retried only after its newline lands.
    pub fn poll(&mut self) -> Result<Vec<HistoryRecord>, OneharnessError> {
        self.scan()?;
        Ok(self.drain_available())
    }

    /// One pass: list the index directory once, tail every segment this
    /// watcher reads — events before runs, so a run's events precede its
    /// record — then the legacy files.
    fn scan(&mut self) -> Result<(), OneharnessError> {
        let segments = list_segments(&self.dir)?;
        for wanted in [SegmentKind::Events, SegmentKind::Runs] {
            if wanted == SegmentKind::Events && !self.events {
                continue;
            }
            for (date, kind, path) in &segments {
                if *kind != wanted {
                    continue;
                }
                let tailed = self.earliest.is_none_or(|earliest| *date >= earliest)
                    || Some(*date) == self.lookback;
                if !tailed {
                    continue;
                }
                let start = self.offsets.get(&(*kind, *date)).copied().unwrap_or(0);
                let end = stream_lines(path, start, |line| {
                    if let Ok(entry) = serde_json::from_slice::<HistoryIndexEntry>(line) {
                        match entry {
                            HistoryIndexEntry::Run(run) => self.accept_run(run),
                            HistoryIndexEntry::Event(event) => self.accept_event(event),
                        }
                    }
                    ControlFlow::Continue(())
                })?;
                self.offsets.insert((*kind, *date), end.unwrap_or(start));
            }
        }
        if self.events {
            let path = self.dir.join(LEGACY_EVENT_INDEX_FILE);
            let start = self.legacy_offsets[0];
            let mut refused = None;
            let end = stream_lines(&path, start, |line| {
                if let Ok(entry) = serde_json::from_slice::<LegacyEventIndexEntry>(line) {
                    if let Err(error) = self.accept_legacy_event(entry) {
                        refused = Some(error);
                        return ControlFlow::Break(());
                    }
                }
                ControlFlow::Continue(())
            })?;
            if let Some(error) = refused {
                return Err(error);
            }
            if let Some(end) = end {
                self.legacy_offsets[0] = end;
            }
        }
        let path = self.dir.join(LEGACY_INDEX_FILE);
        let start = self.legacy_offsets[1];
        let mut refused = None;
        let end = stream_lines(&path, start, |line| {
            if let Ok(entry) = serde_json::from_slice::<LegacyIndexEntry>(line) {
                if let Err(error) = self.accept_legacy_run(entry) {
                    refused = Some(error);
                    return ControlFlow::Break(());
                }
            }
            ControlFlow::Continue(())
        })?;
        if let Some(error) = refused {
            return Err(error);
        }
        if let Some(end) = end {
            self.legacy_offsets[1] = end;
        }
        Ok(())
    }

    fn in_project(&self, slug: &str) -> bool {
        self.project_slug
            .as_deref()
            .is_none_or(|wanted| wanted == slug)
    }

    fn accept_run(&mut self, entry: RunIndexEntry) {
        let (slug, stem) = entry.session_path.parts();
        if self.seen.contains(&entry.history_id)
            || !self.in_project(slug)
            || !entry.labels.matches(&self.labels)
            || !self.in_session(stem, Some(&entry.name))
        {
            return;
        }
        let path = entry.session_path.under(&self.dir);
        if let Ok(Some(run)) = find_run_line(&path, entry.history_id, entry.span) {
            self.seen.insert(entry.history_id);
            self.pending.push_back(run.materialize(Vec::new()));
        }
    }

    fn accept_legacy_run(&mut self, entry: LegacyIndexEntry) -> Result<(), OneharnessError> {
        let session = legacy_session(&self.dir, &entry.session_path)?;
        let (slug, stem) = session.parts();
        let record = entry.record;
        if self.seen.contains(&record.history_id)
            || !self.in_project(slug)
            || !record.labels.matches(&self.labels)
            || !self.in_session(stem, Some(&record.name))
            || !session.under(&self.dir).is_file()
        {
            return Ok(());
        }
        self.seen.insert(record.history_id);
        self.pending.push_back(record.materialize(Vec::new()));
        Ok(())
    }

    fn accept_event(&mut self, entry: EventIndexEntry) {
        let (slug, stem) = entry.session_path.parts();
        if self
            .events_after
            .is_some_and(|cursor| entry.run_id <= cursor)
            || !self.in_project(slug)
            || !entry.labels.matches(&self.labels)
            || !self.in_session(stem, index::session_name_from_id(stem))
        {
            return;
        }
        let path = entry.session_path.under(&self.dir);
        if let Ok(Some(line)) = find_event_line(&path, entry.run_id, entry.event_index, entry.span)
        {
            self.pending_events.push_back(line);
        }
    }

    fn accept_legacy_event(&mut self, entry: LegacyEventIndexEntry) -> Result<(), OneharnessError> {
        let session = legacy_session(&self.dir, &entry.session_path)?;
        let (slug, stem) = session.parts();
        if self
            .events_after
            .is_some_and(|cursor| entry.line.run_id <= cursor)
            || !self.in_project(slug)
            || !entry.labels.matches(&self.labels)
            || !self.in_session(
                stem,
                entry
                    .line
                    .session_name
                    .as_ref()
                    .map(HistorySessionName::as_str),
            )
            || !session.under(&self.dir).is_file()
        {
            return Ok(());
        }
        self.pending_events.push_back(entry.line);
        Ok(())
    }

    /// Whether an in-scope entry belongs to the followed session (always, when
    /// none is). The first entry a still-unresolved name matches pins the
    /// session to that entry's id — only when its file stem is a session id a
    /// writer could have minted, since the index is read from disk.
    fn in_session(&mut self, stem: &str, name: Option<&str>) -> bool {
        let Some(filter) = &mut self.session else {
            return true;
        };
        match filter {
            SessionFilter::Following(id) => id.as_str() == stem,
            SessionFilter::Awaiting(wanted) if name == Some(wanted.as_str()) => {
                match stem.parse::<HistorySessionId>() {
                    Ok(id) => {
                        *filter = SessionFilter::Following(id);
                        true
                    }
                    Err(_) => false,
                }
            }
            SessionFilter::Awaiting(_) => false,
        }
    }
}

/// A one-line summary of a session, for `oneharness history list`, read from
/// both its closing `run` records and its event lines. `name`/`project`/
/// `started` come from the first record; a session still in its first turn has
/// none yet, so they fall back to the events' `session_name`, the project
/// directory's slug, and the instant in the session id. `harnesses` is the
/// distinct set across records and events, and `running` says an event's run
/// has no closing record yet.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[schemars(rename = "HistorySessionSummary")]
pub struct SessionSummary {
    /// The session id (the file stem), unique and sortable by start time.
    pub id: String,
    /// The human-meaningful session name (non-unique).
    pub name: String,
    /// Labels shared by every record in the session. Omitted when empty.
    #[serde(default, skip_serializing_if = "HistoryLabels::is_empty")]
    pub labels: HistoryLabels,
    /// The project directory the run operated in.
    pub project: String,
    /// The RFC3339 UTC start time (first record's timestamp); empty if unknown.
    pub started: String,
    /// How many harness-run records the session holds.
    pub record_count: usize,
    /// The distinct harness ids the session touched, in first-seen order.
    pub harnesses: Vec<String>,
    /// The absolute path of the session file.
    pub path: String,
    /// Whether a harness run in this session has written events but not yet
    /// its closing record — the run is still going (or ended without one: a
    /// killed process leaves the same file). Omitted when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub running: bool,
}

/// A session as a listing collects it from the index, before its summary is
/// finished. `event_labels` are what a record-less session's event entries
/// carry — how a watcher resolves a running session by name and labels.
struct CollectedSession {
    summary: SessionSummary,
    event_labels: Option<HistoryLabels>,
}

/// The first closing record a listing met for a session: an index entry (its
/// project is on the session line it points at) or a legacy entry (which
/// carries the whole record).
enum FirstRun {
    Entry(RunIndexEntry),
    Legacy(Box<HistoryRunRecord>),
}

impl FirstRun {
    fn id(&self) -> HistoryId {
        match self {
            FirstRun::Entry(entry) => entry.history_id,
            FirstRun::Legacy(record) => record.history_id,
        }
    }
}

#[derive(Default)]
struct SessionAccumulator {
    first: Option<FirstRun>,
    closed: HashSet<HistoryId>,
    event_runs: HashSet<HistoryId>,
    harnesses: Vec<String>,
    event_labels: Option<HistoryLabels>,
}

impl SessionAccumulator {
    fn touch(&mut self, harness_id: &str) {
        if !self.harnesses.iter().any(|known| known == harness_id) {
            self.harnesses.push(harness_id.to_string());
        }
    }

    fn close(&mut self, first: FirstRun, harness_id: &str) {
        let id = first.id();
        if !self.closed.insert(id) {
            return;
        }
        self.touch(harness_id);
        if self.first.as_ref().is_none_or(|known| id < known.id()) {
            self.first = Some(first);
        }
    }
}

/// Collect the sessions a window's index entries name, keyed by session path.
/// Reads the segments dated inside the window — every segment, plus the legacy
/// index files, for [`HistoryWindow::AllTime`] — and never the session tree.
fn collect_sessions(
    dir: &Path,
    project_slug: Option<&str>,
    window: HistoryWindow,
) -> Result<Vec<CollectedSession>, OneharnessError> {
    let earliest = window.earliest(today());
    // Keyed by the looser legacy path, which every dated one also is.
    let mut sessions: BTreeMap<LegacySessionPath, SessionAccumulator> = BTreeMap::new();
    let in_scope = |session_path: &LegacySessionPath| {
        project_slug.is_none_or(|wanted| wanted == session_path.parts().0)
    };
    for (date, _, path) in list_segments(dir)? {
        if earliest.is_some_and(|earliest| date < earliest) {
            continue;
        }
        stream_lines(&path, 0, |line| {
            match serde_json::from_slice::<HistoryIndexEntry>(line) {
                Ok(HistoryIndexEntry::Run(run)) => {
                    let session_path = LegacySessionPath::from(&run.session_path);
                    if !in_scope(&session_path) {
                        return ControlFlow::Continue(());
                    }
                    let harness_id = run.harness_id.clone();
                    sessions
                        .entry(session_path)
                        .or_default()
                        .close(FirstRun::Entry(run), &harness_id);
                }
                Ok(HistoryIndexEntry::Event(event)) => {
                    let session_path = LegacySessionPath::from(&event.session_path);
                    if !in_scope(&session_path) {
                        return ControlFlow::Continue(());
                    }
                    let session = sessions.entry(session_path).or_default();
                    session.event_runs.insert(event.run_id);
                    session.touch(&event.harness_id);
                    session.event_labels.get_or_insert(event.labels);
                }
                _ => {}
            }
            ControlFlow::Continue(())
        })?;
    }
    if window.reads_legacy() {
        let mut refused = None;
        stream_lines(&dir.join(LEGACY_INDEX_FILE), 0, |line| {
            if let Ok(entry) = serde_json::from_slice::<LegacyIndexEntry>(line) {
                let session_path = match legacy_session(dir, &entry.session_path) {
                    Ok(session_path) => session_path.clone(),
                    Err(error) => {
                        refused = Some(error);
                        return ControlFlow::Break(());
                    }
                };
                if in_scope(&session_path) {
                    let record = entry.record;
                    let harness_id = record.harness_id.clone().unwrap_or(record.harness.clone());
                    sessions
                        .entry(session_path)
                        .or_default()
                        .close(FirstRun::Legacy(Box::new(record)), &harness_id);
                }
            }
            ControlFlow::Continue(())
        })?;
        if let Some(error) = refused {
            return Err(error);
        }
        stream_lines(&dir.join(LEGACY_EVENT_INDEX_FILE), 0, |line| {
            if let Ok(entry) = serde_json::from_slice::<LegacyEventIndexEntry>(line) {
                let session_path = match legacy_session(dir, &entry.session_path) {
                    Ok(session_path) => session_path.clone(),
                    Err(error) => {
                        refused = Some(error);
                        return ControlFlow::Break(());
                    }
                };
                if in_scope(&session_path) {
                    let session = sessions.entry(session_path).or_default();
                    session.event_runs.insert(entry.line.run_id);
                    session.touch(
                        entry
                            .line
                            .harness_id
                            .as_ref()
                            .unwrap_or(&entry.line.harness),
                    );
                    session.event_labels.get_or_insert(entry.labels);
                }
            }
            ControlFlow::Continue(())
        })?;
        if let Some(error) = refused {
            return Err(error);
        }
    }
    let mut collected = Vec::new();
    for (session_path, found) in sessions {
        let running = found
            .event_runs
            .iter()
            .any(|run| !found.closed.contains(run));
        if found.closed.is_empty() && !running {
            continue;
        }
        let (slug, stem) = session_path.parts();
        let path = session_path.under(dir);
        // An entry whose session was cleared is skipped here, at one `stat`.
        if !path.is_file() {
            continue;
        }
        let (name, labels, project, started) = match &found.first {
            Some(FirstRun::Legacy(record)) => (
                record.name.clone(),
                record.labels.clone(),
                record.project.clone(),
                record.timestamp.clone(),
            ),
            Some(FirstRun::Entry(entry)) => {
                let project = find_run_line(&path, entry.history_id, entry.span)?
                    .map_or_else(|| slug.to_string(), |run| run.project);
                (
                    entry.name.clone(),
                    entry.labels.clone(),
                    project,
                    entry.recorded_at.as_str().to_string(),
                )
            }
            None => (
                index::session_name_from_id(stem)
                    .unwrap_or(stem)
                    .to_string(),
                HistoryLabels::default(),
                slug.to_string(),
                history::session_started_from_id(stem).unwrap_or_default(),
            ),
        };
        collected.push(CollectedSession {
            summary: SessionSummary {
                id: stem.to_string(),
                name,
                labels,
                project,
                started,
                record_count: found.closed.len(),
                harnesses: found.harnesses,
                path: path.display().to_string(),
                running,
            },
            event_labels: found.event_labels,
        });
    }
    // Newest first. RFC3339 sorts lexically as chronologically; a session with no
    // readable timestamp (empty `started`) sorts last.
    collected.sort_by(|a, b| {
        b.summary
            .started
            .cmp(&a.summary.started)
            .then(a.summary.id.cmp(&b.summary.id))
    });
    Ok(collected)
}

/// List the sessions a window's index entries name, newest first. When
/// `project_slug` is `Some`, only that project's sessions are listed. Reads the
/// segments dated inside `window` (and, for [`HistoryWindow::AllTime`], the
/// legacy index files an older core keeps) — never the session tree — plus,
/// for each session listed, the one line of its file that says its project. A
/// session's counts are those of its entries inside the window, and a session
/// file that is gone is skipped. A missing `dir` lists nothing.
pub fn list_sessions(
    dir: &Path,
    project_slug: Option<&str>,
    window: HistoryWindow,
) -> Result<Vec<SessionSummary>, OneharnessError> {
    Ok(collect_sessions(dir, project_slug, window)?
        .into_iter()
        .map(|found| found.summary)
        .collect())
}

/// The sessions whose id OR name equals `needle`, newest first (a name is
/// non-unique). Pure over an already-listed set, so the caller walks the fs once.
pub fn match_sessions<'a>(sessions: &'a [SessionSummary], needle: &str) -> Vec<&'a SessionSummary> {
    sessions
        .iter()
        .filter(|s| s.id == needle || s.name == needle)
        .collect()
}

/// Read a session file and materialize each completed run with its ordered events.
/// Malformed, partial, and legacy lines are skipped.
pub fn read_session(path: &Path) -> Result<Vec<HistoryRecord>, OneharnessError> {
    let text = fs::read_to_string(path).map_err(|source| OneharnessError::HistoryIo {
        path: path.display().to_string(),
        source,
    })?;
    Ok(parse_records(path, &text))
}

/// Read the display view, including event-only runs whose terminal line has not
/// landed. Completed entries retain the established materialized record shape.
pub fn read_session_display(path: &Path) -> Result<Vec<HistoryShowEntry>, OneharnessError> {
    let text = fs::read_to_string(path).map_err(|source| OneharnessError::HistoryIo {
        path: path.display().to_string(),
        source,
    })?;
    let mut dangling: BTreeMap<HistoryId, (String, Vec<_>)> = BTreeMap::new();
    let mut values = Vec::new();
    for line in parse_lines(path, &text) {
        match line {
            HistoryLine::RunStarted(_) => {}
            HistoryLine::Event(line) => {
                dangling
                    .entry(line.run_id)
                    .or_insert_with(|| (line.harness, Vec::new()))
                    .1
                    .push(line.event);
            }
            HistoryLine::Run(run) => {
                let events = dangling
                    .remove(&run.history_id)
                    .map(|(_, events)| events)
                    .unwrap_or_default();
                values.push(HistoryShowEntry::Record(run.materialize(events)));
            }
        }
    }
    for (run_id, (harness, events)) in dangling {
        // Every dangling run was entered by reading one of its events, so the
        // constructor's empty-events `None` cannot arise here.
        values.extend(
            IncompleteHistoryRun::new(run_id, harness, events).map(HistoryShowEntry::Incomplete),
        );
    }
    Ok(values)
}

/// Whether text can name one file directly under a directory: not empty, not
/// `.`/`..`, no separator and no drive prefix. A lookup key is caller input,
/// so one that could reach elsewhere names nothing.
fn plain_file_component(text: &str) -> bool {
    !text.is_empty() && text != "." && text != ".." && !text.contains(['/', '\\', ':'])
}

/// Resolve a session by its id (file stem) — for a run a listing did not
/// surface: outside the listing's window, never indexed, or a file whose every
/// line was unreadable. With a project slug it opens `<dir>/<slug>/<id>.jsonl`
/// by name, needing no index; without one it reads the segments for the date
/// the session id embeds.
pub fn find_session_path(
    dir: &Path,
    project_slug: Option<&str>,
    id: &str,
) -> Result<Option<PathBuf>, OneharnessError> {
    if !plain_file_component(id) {
        return Ok(None);
    }
    if let Some(slug) = project_slug {
        if !plain_file_component(slug) || slug == INDEX_DIR {
            return Ok(None);
        }
        let path = dir.join(slug).join(format!("{id}.{SESSION_EXT}"));
        return Ok(path.is_file().then_some(path));
    }
    let Some(date) = index::session_date_from_id(id) else {
        return Ok(None);
    };
    let mut found = None;
    for kind in [SegmentKind::Runs, SegmentKind::Events] {
        stream_lines(&segment_path(dir, kind, date), 0, |line| {
            let Ok(entry) = serde_json::from_slice::<HistoryIndexEntry>(line) else {
                return ControlFlow::Continue(());
            };
            if entry.session_path().parts().1 == id {
                found = Some(entry.session_path().under(dir));
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })?;
        if let Some(path) = found.take().filter(|path| path.is_file()) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Find exactly one history record by its UUID, across all projects: the one
/// runs segment for the date the id was minted on, then that entry's session
/// file. A miss — an id no segment of its date holds, or a legacy id with no
/// date — is [`OneharnessError::HistoryNotFound`]; there is no fallback read
/// of the legacy index (see [`find_record_by_id_in`] for the all-time lookup).
pub fn find_record_by_id(dir: &Path, id: HistoryId) -> Result<HistoryRecord, OneharnessError> {
    find_record_by_id_in(dir, id, HistoryWindow::default())
}

/// [`find_record_by_id`], with the reach stated. Under
/// [`HistoryWindow::AllTime`] it reads every runs segment and then streams the
/// legacy `.index.jsonl` line by line — opened read-only, in memory bounded by
/// one line — stopping at the first entry with the id and opening the session
/// file it names; that is how a run recorded before the dated index, and never
/// reindexed, is found. Any other window reads the one segment the id's date
/// names.
pub fn find_record_by_id_in(
    dir: &Path,
    id: HistoryId,
    window: HistoryWindow,
) -> Result<HistoryRecord, OneharnessError> {
    let not_found = || OneharnessError::HistoryNotFound { id: id.to_string() };
    let segments: Vec<PathBuf> = if window.reads_legacy() {
        list_segments(dir)?
            .into_iter()
            .rev()
            .filter(|(_, kind, _)| *kind == SegmentKind::Runs)
            .map(|(_, _, path)| path)
            .collect()
    } else {
        let date = UtcDate::of_history_id(id).ok_or_else(not_found)?;
        vec![segment_path(dir, SegmentKind::Runs, date)]
    };
    for path in segments {
        let mut session_path = None;
        stream_lines(&path, 0, |line| match serde_json::from_slice(line) {
            Ok(HistoryIndexEntry::Run(run)) if run.history_id == id => {
                session_path = Some(run.session_path.under(dir));
                ControlFlow::Break(())
            }
            _ => ControlFlow::Continue(()),
        })?;
        if let Some(record) = match session_path {
            Some(path) => record_in_session(&path, id)?,
            None => None,
        } {
            return Ok(record);
        }
    }
    if window.reads_legacy() {
        let needle = id.to_string();
        let mut session_path = None;
        stream_lines(&dir.join(LEGACY_INDEX_FILE), 0, |line| {
            // The id's text is on the line of any entry that is its; checking
            // for it first spares parsing every other run's whole record.
            if !line
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
            {
                return ControlFlow::Continue(());
            }
            match serde_json::from_slice::<LegacyIndexEntry>(line) {
                Ok(entry) if entry.record.history_id == id => {
                    session_path =
                        Some(legacy_session(dir, &entry.session_path).map(|path| path.under(dir)));
                    ControlFlow::Break(())
                }
                _ => ControlFlow::Continue(()),
            }
        })?;
        if let Some(record) = match session_path.transpose()? {
            Some(path) => record_in_session(&path, id)?,
            None => None,
        } {
            return Ok(record);
        }
    }
    Err(not_found())
}

/// The record `id` names in the session file an entry points at, with its
/// events; `None` when the file is gone or holds no such record.
fn record_in_session(path: &Path, id: HistoryId) -> Result<Option<HistoryRecord>, OneharnessError> {
    match read_session(path) {
        Ok(records) => Ok(records.into_iter().find(|record| record.history_id == id)),
        Err(OneharnessError::HistoryIo { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn history_io_error(path: &Path, source: std::io::Error) -> OneharnessError {
    OneharnessError::HistoryIo {
        path: path.display().to_string(),
        source,
    }
}

/// The session files [`remove_sessions`] would delete under `dir` (optionally
/// restricted to one project slug), sorted — what a dry-run `history clear`
/// reports. A missing `dir` names none.
pub fn list_session_files(
    dir: &Path,
    project_slug: Option<&str>,
) -> Result<Vec<String>, OneharnessError> {
    let mut files = Vec::new();
    for pdir in project_dirs(dir, project_slug)? {
        for path in read_session_files(&pdir)? {
            files.push(path.display().to_string());
        }
    }
    files.sort();
    Ok(files)
}

/// Delete every session file under `dir` (optionally restricted to one project
/// slug), returning the paths removed. Empty project subdirectories left behind
/// are pruned. A missing `dir` removes nothing. Session files are all it
/// deletes: the index directory, its segments and the legacy index files stay.
pub fn remove_sessions(
    dir: &Path,
    project_slug: Option<&str>,
) -> Result<Vec<String>, OneharnessError> {
    let mut removed = Vec::new();
    for pdir in project_dirs(dir, project_slug)? {
        for path in read_session_files(&pdir)? {
            fs::remove_file(&path).map_err(|source| OneharnessError::HistoryIo {
                path: path.display().to_string(),
                source,
            })?;
            removed.push(path.display().to_string());
        }
        // Prune the project subdir if it is now empty (best-effort).
        if read_session_files(&pdir)?.is_empty() {
            let _ = fs::remove_dir(&pdir);
        }
    }
    removed.sort();
    Ok(removed)
}

/// The project directories a clear reaches: the one named, or every one.
fn project_dirs(dir: &Path, project_slug: Option<&str>) -> Result<Vec<PathBuf>, OneharnessError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    Ok(match project_slug {
        Some(slug) if plain_file_component(slug) && slug != INDEX_DIR => {
            vec![dir.join(slug)]
        }
        Some(_) => Vec::new(),
        None => read_subdirs(dir)?,
    }
    .into_iter()
    .filter(|pdir| pdir.is_dir())
    .collect())
}

/// The immediate subdirectories of `dir` (the project slugs) — never the index
/// directory, which holds no sessions.
fn read_subdirs(dir: &Path) -> Result<Vec<PathBuf>, OneharnessError> {
    let mut dirs = Vec::new();
    for entry in read_dir(dir)? {
        let path = entry.path();
        if path.is_dir() && entry.file_name() != INDEX_DIR {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn read_subdirs_if_present(dir: &Path) -> Result<Vec<PathBuf>, OneharnessError> {
    if dir.exists() {
        read_subdirs(dir)
    } else {
        Ok(Vec::new())
    }
}

/// The `*.jsonl` session files directly inside a project subdirectory.
fn read_session_files(pdir: &Path) -> Result<Vec<PathBuf>, OneharnessError> {
    let mut files = Vec::new();
    for entry in read_dir(pdir)? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some(SESSION_EXT) {
            files.push(path);
        }
    }
    Ok(files)
}

fn read_dir(dir: &Path) -> Result<Vec<fs::DirEntry>, OneharnessError> {
    fs::read_dir(dir)
        .map_err(|source| OneharnessError::HistoryIo {
            path: dir.display().to_string(),
            source,
        })?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|source| OneharnessError::HistoryIo {
            path: dir.display().to_string(),
            source,
        })
}

/// Parse a JSONL blob into values, skipping malformed or partial lines.
#[cfg(test)]
fn parse_values(text: &str) -> Vec<Value> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn parse_records(path: &Path, text: &str) -> Vec<HistoryRecord> {
    let mut events: BTreeMap<HistoryId, Vec<_>> = BTreeMap::new();
    let mut records = Vec::new();
    for line in parse_lines(path, text) {
        match line {
            HistoryLine::RunStarted(_) => {}
            HistoryLine::Event(line) => events.entry(line.run_id).or_default().push(line.event),
            HistoryLine::Run(run) => {
                let mut run_events = events.remove(&run.history_id).unwrap_or_default();
                run_events.sort_by_key(|event| event.index);
                records.push(run.materialize(run_events));
            }
        }
    }
    records
}

/// Whether this process has already said that it skipped unmigrated lines —
/// said once, naming the first file, however many files carry them.
static UNMIGRATED_REPORTED: AtomicBool = AtomicBool::new(false);

fn parse_lines(path: &Path, text: &str) -> Vec<HistoryLine> {
    let mut legacy = false;
    let lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let value: Value = serde_json::from_str(line).ok()?;
            let line_type = value.get("type").and_then(Value::as_str);
            if line_type.is_none() {
                legacy = true;
                return None;
            }
            serde_json::from_value(value).ok()
        })
        .collect();
    if legacy && !UNMIGRATED_REPORTED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "oneharness: warning: skipped unmigrated history lines in `{}`; run `oneharness history migrate`",
            path.display()
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::report::{ExecutionTelemetry, OutputFormat, Status};
    use crate::domain::signals::{FailureKind, Usage};
    use crate::io::scratch::ScratchDir;
    use std::collections::BTreeSet;

    /// Every entry in every segment of `dir`, in segment then line order.
    fn segment_entries(dir: &Path) -> Vec<HistoryIndexEntry> {
        let mut entries = Vec::new();
        for (_, _, path) in list_segments(dir).unwrap() {
            stream_lines(&path, 0, |line| {
                if let Ok(entry) = serde_json::from_slice(line) {
                    entries.push(entry);
                }
                ControlFlow::Continue(())
            })
            .unwrap();
        }
        entries
    }

    fn run_entries(dir: &Path) -> Vec<RunIndexEntry> {
        segment_entries(dir)
            .into_iter()
            .filter_map(|entry| match entry {
                HistoryIndexEntry::Run(run) => Some(run),
                HistoryIndexEntry::Event(_) => None,
            })
            .collect()
    }

    fn temp_dir(tag: &str) -> ScratchDir {
        ScratchDir::new(&format!("hist-{tag}-{}", now_epoch_secs())).unwrap()
    }

    fn result(harness: &str) -> RunResult {
        RunResult {
            harness: harness.to_string(),
            variant: None,
            harness_id: harness.to_string(),
            bin: "bin".to_string(),
            available: true,
            status: Status::Ok,
            prompt: None,
            model: None,
            observed_model: None,
            exit_code: Some(0),
            duration_ms: Some(10),
            telemetry: Some(ExecutionTelemetry::ProviderMeasured {
                started_at: "2026-07-19T00:00:00.000Z".parse().unwrap(),
                finished_at: Some("2026-07-19T00:00:00.000Z".parse().unwrap()),
                model_ms: Some(7),
                tool_ms: Some(0),
                time_to_first_token_ms: None,
            }),
            command: vec!["bin".to_string()],
            output_format: OutputFormat::Json,
            text: Some("hi".to_string()),
            text_source: Some("raw".to_string()),
            usage: Usage::default(),
            usage_source: None,
            session_id: None,
            events: None,
            events_source: None,
            structured: None,
            schema_valid: None,
            schema_attempts: None,
            schema_error: None,
            failure_kind: None,
            work: None,
            failure_kind_source: None,
            stdout: "hi".to_string(),
            stderr: String::new(),
            error: None,
        }
    }

    #[test]
    fn resolve_dir_prefers_configured_then_default() {
        assert_eq!(
            resolve_dir(Some("/tmp/custom")),
            Some(PathBuf::from("/tmp/custom"))
        );
        // Empty configured value falls through to the platform default (if any).
        // We can't assert the default path portably, but it must end with the
        // history segments when a state dir is resolvable.
        if let Some(def) = resolve_dir(None) {
            assert!(def.ends_with("oneharness/history"));
        }
    }

    #[test]
    fn clean_exit_classified_failures_round_trip_without_complete_timing() {
        let dir = temp_dir("clean-classified-timing");
        let project = temp_dir("clean-classified-project");
        let writer = HistoryWriter::open(
            &dir,
            &project,
            "clean classified timing",
            HistoryLabels::default(),
        )
        .unwrap();

        for telemetry in [
            None,
            Some(ExecutionTelemetry::PartialInvocation {
                started_at: "2026-07-19T00:00:00.000Z".parse().unwrap(),
            }),
        ] {
            let mut overloaded = result("codex");
            overloaded.telemetry = telemetry;
            overloaded.text = None;
            overloaded.text_source = None;
            overloaded.failure_kind = Some(FailureKind::ServerOverloaded);
            overloaded.stdout = r#"{"error":{"codex_error_info":"server_overloaded"}}"#.into();

            writer
                .append(PermissionMode::Default, None, "retry later", &overloaded)
                .unwrap();
        }

        let records = read_session(writer.path()).unwrap();
        assert_eq!(records.len(), 2);
        assert!(records
            .iter()
            .all(|record| record.failure_kind == Some(FailureKind::ServerOverloaded)));
        assert!(records[0].started_at.is_none());
        assert_eq!(
            records[1].started_at.as_deref(),
            Some("2026-07-19T00:00:00Z")
        );
        assert!(records.iter().all(HistoryRecord::complete));
    }

    #[test]
    fn open_creates_project_subdir_and_expected_path() {
        let dir = temp_dir("open");
        let project = dir.join("My Proj");
        fs::create_dir_all(&project).unwrap();
        let canonical = fs::canonicalize(&project).unwrap();
        let w = HistoryWriter::open(&dir, &project, "Fix Bug!", HistoryLabels::default()).unwrap();
        // Project slug subdir exists.
        assert!(dir
            .join(history::project_slug(&canonical.display().to_string()))
            .is_dir());
        // Session id: sanitized name, compact UTC, pid.
        let stem = w.path().file_stem().unwrap().to_str().unwrap();
        assert!(stem.starts_with("fix-bug-"), "{stem}");
        assert!(w.path().extension().unwrap() == "jsonl");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_writes_parseable_jsonl_lines() {
        let dir = temp_dir("append");
        let project = dir.join("project-a");
        fs::create_dir_all(&project).unwrap();
        let w = HistoryWriter::open(
            &dir,
            &project,
            "my-session",
            history::parse_labels(["graph=deploy"]).unwrap(),
        )
        .unwrap();
        let mut first = result("claude-code");
        first.events = Some(vec![crate::domain::events::ActionEvent {
            kind: "message".to_string(),
            name: None,
            input: None,
            output: Some("observed".to_string()),
            index: 0,
            tool_call_id: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            status: None,
            timing_source: None,
        }]);
        w.append(PermissionMode::Bypass, Some("sonnet"), "do it", &first)
            .unwrap();
        w.append(
            PermissionMode::Bypass,
            Some("sonnet"),
            "do it",
            &result("codex"),
        )
        .unwrap();
        let records = read_session(w.path()).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].harness, "claude-code");
        assert_eq!(records[0].name, "my-session");
        assert_eq!(
            records[0].project,
            fs::canonicalize(&project).unwrap().display().to_string()
        );
        assert_eq!(records[0].model.as_deref(), Some("sonnet"));
        assert_eq!(records[0].labels.as_map().get("graph").unwrap(), "deploy");
        assert_eq!(
            records[0].events.as_ref().unwrap()[0].output.as_deref(),
            Some("observed")
        );
        assert_eq!(records[1].harness, "codex");
        assert_eq!(records[0].history_id.as_uuid().get_version_num(), 7);
        // A minted id must be text the public cursor contract accepts back, or
        // `history watch --after` could not resume from the record it just wrote.
        assert_eq!(
            records[0]
                .history_id
                .to_string()
                .parse::<HistoryId>()
                .unwrap(),
            records[0].history_id
        );
        let lines = parse_lines(w.path(), &fs::read_to_string(w.path()).unwrap());
        assert!(matches!(lines[0], HistoryLine::Event(_)));
        assert!(matches!(lines[1], HistoryLine::Run(_)));
        assert!(matches!(lines[2], HistoryLine::Run(_)));
        // One entry per session line: the event, then each closing run line,
        // each pointing back at the line it indexes.
        let entries = segment_entries(&dir);
        assert_eq!(entries.len(), 3, "{entries:?}");
        assert_eq!(run_entries(&dir).len(), 2);
        let session = fs::read(w.path()).unwrap();
        for entry in &entries {
            let span = match entry {
                HistoryIndexEntry::Run(run) => run.span,
                HistoryIndexEntry::Event(event) => event.span,
            }
            .unwrap();
            let start = span.offset as usize;
            let line = &session[start..start + span.length.get() as usize];
            let parsed: HistoryLine = serde_json::from_slice(&line[..line.len() - 1]).unwrap();
            match (entry, parsed) {
                (HistoryIndexEntry::Run(run), HistoryLine::Run(line)) => {
                    assert_eq!(run.history_id, line.history_id);
                }
                (HistoryIndexEntry::Event(event), HistoryLine::Event(line)) => {
                    assert_eq!(
                        (event.run_id, event.event_index),
                        (line.run_id, line.event.index)
                    );
                }
                other => panic!("an entry pointing at the wrong kind of line: {other:?}"),
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_live_session_append_is_retried_at_finalization() {
        let dir = temp_dir("live-fallback");
        let project = temp_dir("live-fallback-project");
        let writer = HistoryWriter::open(&dir, &project, "live", HistoryLabels::default()).unwrap();
        let run_id = writer.begin_run();
        let event = crate::domain::events::ActionEvent {
            kind: "message".to_string(),
            name: None,
            input: None,
            output: Some("late".to_string()),
            index: 0,
            tool_call_id: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            status: None,
            timing_source: None,
        };
        fs::create_dir(writer.path()).unwrap();
        let error = writer
            .append_event_tracked(run_id, "codex", event.clone())
            .unwrap_err();
        assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
        fs::remove_dir(writer.path()).unwrap();
        let mut completed = result("codex");
        completed.events = Some(vec![event]);
        writer
            .append_streamed(
                run_id,
                PermissionMode::Default,
                None,
                "prompt",
                &completed,
                &BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(
            read_session(writer.path()).unwrap()[0]
                .events
                .as_ref()
                .unwrap()
                .len(),
            1
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn event_index_failure_still_reports_the_session_line_as_persisted() {
        let dir = temp_dir("live-index-failure");
        let project = temp_dir("live-index-project");
        let writer = HistoryWriter::open(&dir, &project, "live", HistoryLabels::default()).unwrap();
        // A directory where today's events segment would go: the session line
        // lands, the index append does not.
        let blocked = segment_path(&dir, SegmentKind::Events, today());
        fs::create_dir_all(&blocked).unwrap();
        let event = crate::domain::events::ActionEvent {
            kind: "message".to_string(),
            name: None,
            input: None,
            output: None,
            index: 0,
            tool_call_id: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            status: None,
            timing_source: None,
        };
        let outcome = writer
            .append_event_tracked(writer.begin_run(), "codex", event.clone())
            .unwrap();
        assert!(outcome.index_error.is_some());
        assert!(writer
            .append_event(writer.begin_run(), "codex", event.clone())
            .is_err());
        fs::remove_dir(&blocked).unwrap();
        writer
            .append_event(writer.begin_run(), "codex", event)
            .unwrap();
        assert_eq!(
            parse_lines(writer.path(), &fs::read_to_string(writer.path()).unwrap(),).len(),
            3
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn list_sessions_summarizes_and_orders_newest_first() {
        let dir = temp_dir("list");
        fs::create_dir_all(dir.join("proj-a")).unwrap();
        fs::create_dir_all(dir.join("proj-b")).unwrap();
        let line = |name: &str, project: &str, timestamp: &str, harness: &str| {
            let record = HistoryRecord::from_result(
                HistoryId::from_uuid(uuid::Uuid::now_v7()),
                name,
                name,
                &HistoryLabels::default(),
                project,
                timestamp.to_string(),
                PermissionMode::Default,
                None,
                "prompt",
                &result(harness),
            );
            format!(
                "{}\n",
                serde_json::to_string(&HistoryLine::Run(HistoryRunRecord::from_record(&record)))
                    .unwrap()
            )
        };
        fs::write(
            dir.join("proj-a").join("old-20240101T000000Z-1.jsonl"),
            line("old", "/proj/a", "2024-01-01T00:00:00Z", "codex"),
        )
        .unwrap();
        fs::write(
            dir.join("proj-b").join("new-20260101T000000Z-2.jsonl"),
            format!(
                "{}{}",
                line("new", "/proj/b", "2026-01-01T00:00:00Z", "claude-code"),
                line("new", "/proj/b", "2026-01-01T00:00:01Z", "codex")
            ),
        )
        .unwrap();
        // Hand-written session files are not indexed until `reindex` says so.
        assert!(list_sessions(&dir, None, HistoryWindow::default())
            .unwrap()
            .is_empty());
        reindex(&dir).unwrap();
        let all = list_sessions(&dir, None, HistoryWindow::default()).unwrap();
        assert_eq!(all.len(), 2);
        // Newest first.
        assert_eq!(all[0].name, "new");
        assert_eq!(all[0].record_count, 2);
        assert_eq!(all[0].harnesses, vec!["claude-code", "codex"]);
        assert_eq!(all[1].name, "old");
        // Project filter restricts to one subdir.
        let just_a = list_sessions(&dir, Some("proj-a"), HistoryWindow::default()).unwrap();
        assert_eq!(just_a.len(), 1);
        assert_eq!(just_a[0].name, "old");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_ignores_an_empty_session_file() {
        let dir = temp_dir("emptyfile");
        fs::create_dir_all(dir.join("some-proj")).unwrap();
        fs::write(dir.join("some-proj").join("stub-1.jsonl"), "\n").unwrap();
        reindex(&dir).unwrap();
        let sessions = list_sessions(&dir, None, HistoryWindow::AllTime).unwrap();
        assert!(sessions.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dangling_events_are_displayed_as_incomplete_and_listed_as_running() {
        let dir = temp_dir("dangling");
        let project_dir = dir.join("project");
        fs::create_dir_all(&project_dir).unwrap();
        let path = project_dir.join("interrupted.jsonl");
        let run_id = HistoryId::from_uuid(uuid::Uuid::now_v7());
        let event = crate::domain::events::ActionEvent {
            kind: "message".to_string(),
            name: None,
            input: None,
            output: Some("partial".to_string()),
            index: 0,
            tool_call_id: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            status: None,
            timing_source: None,
        };
        let line = HistoryLine::Event(HistoryEventLine {
            schema_version: history::SCHEMA_VERSION.to_string(),
            run_id,
            harness: "codex".to_string(),
            variant: None,
            harness_id: Some("codex".to_string()),
            event: event.clone(),
            session_name: None,
        });
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&line).unwrap()),
        )
        .unwrap();

        // Listed as a session still running: no closing record yet, and no
        // session name on a line written before lines carried one.
        reindex(&dir).unwrap();
        let listed = list_sessions(&dir, None, HistoryWindow::default()).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].running);
        assert_eq!(listed[0].record_count, 0);
        assert_eq!(listed[0].name, "interrupted");
        assert_eq!(listed[0].harnesses, ["codex"]);
        let expected = IncompleteHistoryRun::new(run_id, "codex".to_string(), vec![event]);
        assert_eq!(
            read_session_display(&path).unwrap(),
            [HistoryShowEntry::Incomplete(expected.unwrap())]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_lines_are_skipped_without_panicking() {
        let dir = temp_dir("legacy");
        let path = dir.join("legacy.jsonl");
        fs::write(
            &path,
            "{\"schema_version\":\"0.3\",\"history_id\":\"0198f0d0-7b31-7000-8000-000000000001\"}\n",
        )
        .unwrap();
        assert!(read_session(&path).unwrap().is_empty());
        assert!(read_session_display(&path).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_dir_lists_nothing() {
        let dir = std::env::temp_dir().join("oneharness-hist-absent-does-not-exist-xyz");
        let _ = fs::remove_dir_all(&dir);
        assert!(list_sessions(&dir, None, HistoryWindow::AllTime)
            .unwrap()
            .is_empty());
        assert!(remove_sessions(&dir, None).unwrap().is_empty());
        assert_eq!(reindex(&dir).unwrap().entries_added, 0);
        assert!(!dir.exists(), "a reindex of no store creates none");
    }

    #[test]
    fn match_sessions_by_id_or_name_newest_first() {
        let sessions = list_from(&[
            ("dup-20240101T000000Z-1", "dup", "2024-01-01T00:00:00Z"),
            ("dup-20260101T000000Z-2", "dup", "2026-01-01T00:00:00Z"),
            ("other-20250101T000000Z-3", "other", "2025-01-01T00:00:00Z"),
        ]);
        // By name: both `dup` sessions, newest first.
        let m = match_sessions(&sessions, "dup");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].id, "dup-20260101T000000Z-2");
        // By exact id: just that one.
        let m = match_sessions(&sessions, "other-20250101T000000Z-3");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].name, "other");
        assert!(match_sessions(&sessions, "nope").is_empty());
    }

    /// Build a pre-sorted (newest-first) summary list for the pure matcher test.
    fn list_from(rows: &[(&str, &str, &str)]) -> Vec<SessionSummary> {
        let mut v: Vec<SessionSummary> = rows
            .iter()
            .map(|(id, name, started)| SessionSummary {
                id: id.to_string(),
                name: name.to_string(),
                labels: HistoryLabels::default(),
                project: "/p".to_string(),
                started: started.to_string(),
                record_count: 1,
                harnesses: vec!["codex".to_string()],
                path: format!("/h/{id}.jsonl"),
                running: false,
            })
            .collect();
        v.sort_by(|a, b| b.started.cmp(&a.started));
        v
    }

    #[test]
    fn remove_sessions_deletes_and_prunes() {
        let dir = temp_dir("remove");
        let project = dir.join("project-x");
        fs::create_dir_all(&project).unwrap();
        let w = HistoryWriter::open(&dir, &project, "s", HistoryLabels::default()).unwrap();
        w.append(PermissionMode::Default, None, "p", &result("codex"))
            .unwrap();
        assert_eq!(
            list_sessions(&dir, None, HistoryWindow::default())
                .unwrap()
                .len(),
            1
        );
        let segments_before = segment_entries(&dir);
        assert_eq!(list_session_files(&dir, None).unwrap().len(), 1);
        let removed = remove_sessions(&dir, None).unwrap();
        assert_eq!(removed.len(), 1);
        // The session's entries stay; the listing skips an entry whose
        // session file is gone.
        assert_eq!(segment_entries(&dir), segments_before);
        assert!(list_sessions(&dir, None, HistoryWindow::default())
            .unwrap()
            .is_empty());
        // The now-empty project subdir was pruned; the index directory stayed.
        assert!(!project.exists());
        assert!(dir.join(INDEX_DIR).is_dir());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_values_skips_blank_and_bad_lines() {
        let text = "{\"a\":1}\n\n  \nnot json\n{\"b\":2}\n";
        let vals = parse_values(text);
        assert_eq!(vals.len(), 2);
        assert_eq!(vals[0]["a"], 1);
        assert_eq!(vals[1]["b"], 2);
    }

    #[test]
    fn concurrent_index_appends_are_complete_and_unique() {
        let dir = temp_dir("concurrent-index");
        let project = temp_dir("concurrent-project");
        let mut threads = Vec::new();
        for index in 0..12 {
            let dir = dir.to_path_buf();
            let project = project.to_path_buf();
            threads.push(std::thread::spawn(move || {
                let writer = HistoryWriter::open(
                    &dir,
                    &project,
                    &format!("session-{index}"),
                    HistoryLabels::default(),
                )
                .unwrap();
                writer
                    .append(
                        PermissionMode::Default,
                        None,
                        &format!("prompt-{index}"),
                        &result("codex"),
                    )
                    .unwrap();
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }

        let entries = run_entries(&dir);
        assert_eq!(entries.len(), 12);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.history_id)
                .collect::<BTreeSet<_>>()
                .len(),
            12
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn watcher_resumes_after_cursor_without_duplication_and_filters_labels() {
        let dir = temp_dir("watch-resume");
        let project = temp_dir("watch-project");
        let writer = HistoryWriter::open(
            &dir,
            &project,
            "watched",
            history::parse_labels(["graph=release", "task=test"]).unwrap(),
        )
        .unwrap();
        writer
            .append(PermissionMode::Default, None, "first", &result("codex"))
            .unwrap();
        let first_id = read_session(writer.path()).unwrap()[0].history_id;
        let mut watcher = HistoryWatcher::open(
            &dir,
            Some(first_id),
            history::parse_labels(["graph=release"]).unwrap(),
            None,
            false,
        )
        .unwrap();
        assert!(watcher.drain_available().is_empty());

        writer
            .append(PermissionMode::Default, None, "second", &result("codex"))
            .unwrap();
        let records = watcher.poll().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].prompt, "second");
        assert_ne!(records[0].history_id, first_id);
        assert!(watcher.poll().unwrap().is_empty());

        let mut no_match = HistoryWatcher::open(
            &dir,
            None,
            history::parse_labels(["graph=other"]).unwrap(),
            None,
            false,
        )
        .unwrap();
        assert!(no_match.drain_available().is_empty());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn a_torn_final_segment_line_never_swallows_the_next_writers_entry() {
        let dir = temp_dir("partial-index");
        let project = temp_dir("partial-project");
        let writer =
            HistoryWriter::open(&dir, &project, "partial", HistoryLabels::default()).unwrap();
        writer
            .append(PermissionMode::Default, None, "torn", &result("codex"))
            .unwrap();
        // An interrupted writer: half of the entry, no newline.
        let segment = segment_path(&dir, SegmentKind::Runs, today());
        let len = fs::metadata(&segment).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(&segment)
            .unwrap()
            .set_len(len / 2)
            .unwrap();
        let torn = fs::read(&segment).unwrap();

        writer
            .append(PermissionMode::Default, None, "next", &result("codex"))
            .unwrap();
        let bytes = fs::read(&segment).unwrap();
        assert!(
            bytes.starts_with(&torn),
            "the torn bytes are never rewritten"
        );
        assert!(bytes.ends_with(b"\n"));
        let entries = run_entries(&dir);
        assert_eq!(entries.len(), 1, "only the whole entry reads: {entries:?}");
        let mut watcher =
            HistoryWatcher::open(&dir, None, HistoryLabels::default(), None, false).unwrap();
        let recovered = watcher.drain_available();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].prompt, "next");
        assert_eq!(recovered[0].history_id, entries[0].history_id);
    }

    #[test]
    fn a_span_the_session_file_cannot_hold_is_refused_and_the_file_read_instead() {
        let dir = temp_dir("span-bound");
        let project = temp_dir("span-bound-project");
        let writer = HistoryWriter::open(&dir, &project, "span", HistoryLabels::default()).unwrap();
        writer
            .append(PermissionMode::Default, None, "spanned", &result("codex"))
            .unwrap();
        let run = run_entries(&dir).remove(0);
        let good = run.span.unwrap();
        assert!(read_span(writer.path(), good).is_some());
        // A length no file this size holds — a corrupt or hostile entry — is
        // refused before anything is allocated for it.
        for bad in [
            LineSpan {
                offset: good.offset,
                length: std::num::NonZeroU64::new(u64::MAX / 2).unwrap(),
            },
            LineSpan {
                offset: u64::MAX,
                length: std::num::NonZeroU64::MIN,
            },
        ] {
            assert!(read_span(writer.path(), bad).is_none(), "{bad:?}");
            let found = find_run_line(writer.path(), run.history_id, Some(bad))
                .unwrap()
                .expect("the session file itself still answers");
            assert_eq!(found.prompt, "spanned");
        }
        let zero = r#"{"schema_version":"1.0","kind":"event","run_id":"0192b2a0-0000-7000-8000-000000000001","event_index":0,"session_path":"p/s.jsonl","project_slug":"p","harness_id":"codex","labels":{},"offset":0,"length":0}"#;
        assert!(serde_json::from_str::<HistoryIndexEntry>(zero)
            .map(|entry| match entry {
                HistoryIndexEntry::Event(event) => event.span.is_none(),
                HistoryIndexEntry::Run(_) => false,
            })
            .unwrap_or(true));
    }

    // A sparse file is how a span can claim far more bytes than memory holds
    // while still fitting its file; Windows would write every one of them.
    #[cfg(unix)]
    #[test]
    fn a_span_over_bytes_that_hold_no_record_is_refused_without_allocating_its_length() {
        let dir = temp_dir("span-sparse");
        let project = temp_dir("span-sparse-project");
        let writer =
            HistoryWriter::open(&dir, &project, "sparse", HistoryLabels::default()).unwrap();
        writer
            .append(PermissionMode::Default, None, "sparse", &result("codex"))
            .unwrap();
        let run = run_entries(&dir).remove(0);
        let good = run.span.unwrap();
        let recorded = fs::metadata(writer.path()).unwrap().len();
        // 64 GiB of holes closed by one newline: a span over them fits the
        // file, so only reading it as it parses keeps this from allocating it.
        let claimed: u64 = 64 << 30;
        let file = OpenOptions::new().write(true).open(writer.path()).unwrap();
        file.set_len(recorded + claimed - 1).unwrap();
        drop(file);
        OpenOptions::new()
            .append(true)
            .open(writer.path())
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        let hole = LineSpan {
            offset: recorded,
            length: std::num::NonZeroU64::new(claimed).unwrap(),
        };
        assert!(read_span(writer.path(), hole).is_none());
        // A span that starts inside the record rather than at it is refused
        // too, even though it ends on that record's newline.
        let inside = LineSpan {
            offset: good.offset + 1,
            length: std::num::NonZeroU64::new(good.length.get() - 1).unwrap(),
        };
        assert!(read_span(writer.path(), inside).is_none());
        assert!(read_span(writer.path(), good).is_some());
        let found = find_run_line(writer.path(), run.history_id, Some(hole))
            .unwrap()
            .expect("the record ahead of the holes still answers");
        assert_eq!(found.prompt, "sparse");
    }

    fn id_at(secs: u64, counter: u16) -> HistoryId {
        let at = uuid::Timestamp::from_unix(uuid::NoContext, secs, u32::from(counter));
        HistoryId::from_uuid(uuid::Uuid::new_v7(at))
    }

    fn now_secs() -> u64 {
        now_epoch_secs() as u64
    }

    fn event_at(index: usize) -> crate::domain::events::ActionEvent {
        crate::domain::events::ActionEvent {
            kind: "message".to_string(),
            name: None,
            input: None,
            output: Some(format!("event {index}")),
            index,
            tool_call_id: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            status: None,
            timing_source: None,
        }
    }

    /// Record one closed run under a chosen id in its own session; returns the
    /// session file.
    fn closed_run(
        dir: &Path,
        project: &Path,
        name: &str,
        id: HistoryId,
        labels: &[&str],
    ) -> PathBuf {
        let writer = HistoryWriter::open(
            dir,
            project,
            name,
            history::parse_labels(labels.iter().copied()).unwrap(),
        )
        .unwrap();
        writer.append_event(id, "codex", event_at(0)).unwrap();
        writer
            .append_streamed(
                id,
                PermissionMode::Default,
                None,
                name,
                &result("codex"),
                &BTreeSet::from([0]),
            )
            .unwrap();
        writer.path().to_path_buf()
    }

    /// The legacy index line an older core's reconcile writes for a run.
    fn legacy_run_line(dir: &Path, session: &Path, id: HistoryId) -> String {
        let text = fs::read_to_string(session).unwrap();
        let run = parse_lines(session, &text)
            .into_iter()
            .find_map(|line| match line {
                HistoryLine::Run(run) if run.history_id == id => Some(run),
                _ => None,
            })
            .unwrap();
        let relative = session.strip_prefix(dir).unwrap().display().to_string();
        format!(
            "{}\n",
            serde_json::json!({"session_path": relative, "record": run})
        )
    }

    fn legacy_event_line(dir: &Path, session: &Path, id: HistoryId, labels: &str) -> String {
        let relative = session.strip_prefix(dir).unwrap().display().to_string();
        let line = HistoryEventLine {
            schema_version: history::PREVIOUS_CURRENT_SCHEMA_VERSION.to_string(),
            run_id: id,
            harness: "codex".to_string(),
            variant: None,
            harness_id: Some("codex".to_string()),
            event: event_at(7),
            session_name: None,
        };
        format!(
            "{}\n",
            serde_json::json!({
                "session_path": relative,
                "labels": serde_json::from_str::<Value>(labels).unwrap(),
                "line": line,
            })
        )
    }

    /// Move a run's session into a store's project directory with no index
    /// entry for it, as an older core would have left it.
    fn unindexed_copy(dir: &Path, source: &Path) -> PathBuf {
        let slug = source.parent().unwrap().file_name().unwrap();
        let target = dir.join(slug).join(source.file_name().unwrap());
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(source, &target).unwrap();
        target
    }

    #[test]
    fn a_watcher_tails_the_legacy_files_from_their_size_at_open_and_everything_under_all_time() {
        let dir = temp_dir("watch-legacy");
        let other = temp_dir("watch-legacy-source");
        let project = temp_dir("watch-legacy-project");
        let dir = fs::canonicalize(&dir).unwrap();
        let other = fs::canonicalize(&other).unwrap();
        let before_open = id_at(now_secs() - 3 * 86_400, 1);
        let early = unindexed_copy(
            &dir,
            &closed_run(&other, &project, "early", before_open, &["k=v"]),
        );
        fs::write(
            dir.join(LEGACY_INDEX_FILE),
            legacy_run_line(&dir, &early, before_open),
        )
        .unwrap();
        fs::write(
            dir.join(LEGACY_EVENT_INDEX_FILE),
            legacy_event_line(&dir, &early, before_open, r#"{"k":"v"}"#),
        )
        .unwrap();

        // Today's start: what the legacy files held at open is never read.
        let mut watcher =
            HistoryWatcher::open(&dir, None, HistoryLabels::default(), None, true).unwrap();
        assert!(watcher.drain_available().is_empty());
        assert!(watcher.drain_events().is_empty());
        let appended = id_at(now_secs() - 86_400 * 2, 2);
        let late = unindexed_copy(
            &dir,
            &closed_run(&other, &project, "appended", appended, &[]),
        );
        let mut legacy = OpenOptions::new()
            .append(true)
            .open(dir.join(LEGACY_INDEX_FILE))
            .unwrap();
        legacy
            .write_all(legacy_run_line(&dir, &late, appended).as_bytes())
            .unwrap();
        // A line naming a path outside the store, or a session that is gone,
        // is skipped.
        legacy
            .write_all(b"{\"session_path\":\"../escape.jsonl\",\"record\":{}}\n")
            .unwrap();
        let mut events = OpenOptions::new()
            .append(true)
            .open(dir.join(LEGACY_EVENT_INDEX_FILE))
            .unwrap();
        events
            .write_all(legacy_event_line(&dir, &late, appended, "{}").as_bytes())
            .unwrap();
        events
            .write_all(b"{\"session_path\":\"../escape.jsonl\",\"labels\":{},\"line\":{}}\n")
            .unwrap();
        let records = watcher.poll().unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.history_id)
                .collect::<Vec<_>>(),
            vec![appended]
        );
        assert_eq!(watcher.drain_events().len(), 1);
        // Re-appended by a later reconcile: emitted once.
        legacy
            .write_all(legacy_run_line(&dir, &late, appended).as_bytes())
            .unwrap();
        assert!(watcher.poll().unwrap().is_empty());

        // All time: every line from the first byte, labels and project applied.
        let mut all = HistoryWatcher::open_in(
            &dir,
            Some(WatchStart::Window(HistoryWindow::AllTime)),
            history::parse_labels(["k=v"]).unwrap(),
            None,
            true,
            None,
        )
        .unwrap();
        assert_eq!(
            all.drain_available()
                .iter()
                .map(|record| record.history_id)
                .collect::<Vec<_>>(),
            vec![before_open]
        );
        assert_eq!(all.drain_events().len(), 1);
        let mut elsewhere = HistoryWatcher::open_in(
            &dir,
            Some(WatchStart::Window(HistoryWindow::AllTime)),
            HistoryLabels::default(),
            Some("another-project".to_string()),
            true,
            None,
        )
        .unwrap();
        assert!(elsewhere.drain_available().is_empty());
        assert!(elsewhere.drain_events().is_empty());
    }

    #[test]
    fn a_watcher_tails_the_day_before_its_start_from_its_end_and_nothing_earlier() {
        let dir = temp_dir("watch-lookback");
        let project = temp_dir("watch-lookback-project");
        let now = now_secs();
        let yesterday = id_at(now - 86_400, 1);
        let older = id_at(now - 3 * 86_400, 2);
        closed_run(&dir, &project, "yesterday", yesterday, &[]);
        closed_run(&dir, &project, "older", older, &[]);
        let mut watcher =
            HistoryWatcher::open(&dir, None, HistoryLabels::default(), None, true).unwrap();
        assert!(watcher.drain_available().is_empty());
        assert!(watcher.drain_events().is_empty());
        // A run begun yesterday that closes now lands in yesterday's segment.
        let closing = id_at(now - 86_400 + 1, 3);
        closed_run(&dir, &project, "closing", closing, &[]);
        // One begun three days ago does not reach a watcher that started today.
        closed_run(&dir, &project, "stale", id_at(now - 3 * 86_400 + 1, 4), &[]);
        assert_eq!(
            watcher
                .poll()
                .unwrap()
                .iter()
                .map(|record| record.history_id)
                .collect::<Vec<_>>(),
            vec![closing]
        );
        assert_eq!(watcher.drain_events().len(), 1);
        // A cursor with no date, or none its date's segment holds, is not found.
        for cursor in [HistoryId::legacy(b"old"), id_at(now, 9)] {
            assert!(matches!(
                HistoryWatcher::open(&dir, Some(cursor), HistoryLabels::default(), None, false),
                Err(OneharnessError::HistoryNotFound { .. })
            ));
        }
    }

    #[test]
    fn a_watched_session_name_resolves_to_the_newest_matching_session_running_ones_included() {
        let dir = temp_dir("watch-name");
        let project = temp_dir("watch-name-project");
        let now = now_secs();
        closed_run(&dir, &project, "named", id_at(now - 10, 1), &["team=a"]);
        // A second session so named, still in its first turn: only its events
        // carry its labels.
        let running = HistoryWriter::open(
            &dir,
            &project,
            "named-again",
            history::parse_labels(["team=b"]).unwrap(),
        )
        .unwrap();
        let running_id = id_at(now - 5, 2);
        running
            .append_event(running_id, "codex", event_at(0))
            .unwrap();
        let selector = |name: &str| HistorySessionSelector::Name(name.parse().unwrap());
        let open = |name: &str, labels: &[&str]| {
            HistoryWatcher::open_session(
                &dir,
                None,
                history::parse_labels(labels.iter().copied()).unwrap(),
                None,
                true,
                Some(&selector(name)),
            )
            .unwrap()
        };
        let mut by_label = open("named-again", &["team=b"]);
        assert_eq!(by_label.drain_events().len(), 1);
        let mut closed = open("named", &["team=a"]);
        assert_eq!(closed.drain_available().len(), 1);
        // A name nothing carries yet waits for the first session that does.
        let mut awaiting = open("not-yet", &[]);
        assert!(awaiting.drain_available().is_empty());
        let later = id_at(now, 3);
        closed_run(&dir, &project, "not-yet", later, &[]);
        assert_eq!(
            awaiting
                .poll()
                .unwrap()
                .iter()
                .map(|record| record.history_id)
                .collect::<Vec<_>>(),
            vec![later]
        );
        // Following an id: nothing from any other session.
        let id: HistorySessionId = running
            .path()
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        let mut following = HistoryWatcher::open_session(
            &dir,
            None,
            HistoryLabels::default(),
            None,
            true,
            Some(&HistorySessionSelector::Id(id)),
        )
        .unwrap();
        assert!(following.drain_available().is_empty());
        assert_eq!(following.drain_events().len(), 1);
    }

    #[test]
    fn an_all_time_listing_reads_the_legacy_index_and_skips_what_it_cannot_resolve() {
        let dir = temp_dir("list-legacy");
        let other = temp_dir("list-legacy-source");
        let project = temp_dir("list-legacy-project");
        let dir = fs::canonicalize(&dir).unwrap();
        let other = fs::canonicalize(&other).unwrap();
        let id = id_at(1_700_000_000, 1);
        let session = unindexed_copy(&dir, &closed_run(&other, &project, "legacy", id, &["k=v"]));
        let running = id_at(1_700_000_100, 2);
        let mut index = legacy_run_line(&dir, &session, id);
        index.push_str(&legacy_run_line(&dir, &session, id));
        index.push_str("{\"session_path\":\"../escape.jsonl\",\"record\":{}}\n");
        fs::write(dir.join(LEGACY_INDEX_FILE), index).unwrap();
        let mut events = legacy_event_line(&dir, &session, running, r#"{"k":"v"}"#);
        events.push_str("{\"session_path\":\"x\",\"labels\":{},\"line\":{}}\n");
        fs::write(dir.join(LEGACY_EVENT_INDEX_FILE), events).unwrap();

        assert!(list_sessions(&dir, None, HistoryWindow::default())
            .unwrap()
            .is_empty());
        let listed = list_sessions(&dir, None, HistoryWindow::AllTime).unwrap();
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(
            listed[0].record_count, 1,
            "one record, however often indexed"
        );
        assert!(listed[0].running, "an event of a run with no record");
        assert_eq!(listed[0].name, "legacy");
        assert_eq!(listed[0].labels.as_map().get("k").unwrap(), "v");
        assert!(
            list_sessions(&dir, Some("elsewhere"), HistoryWindow::AllTime)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            find_record_by_id_in(&dir, id, HistoryWindow::AllTime)
                .unwrap()
                .history_id,
            id
        );
        // Gone from disk: the lookup skips it rather than failing.
        fs::remove_file(&session).unwrap();
        assert!(list_sessions(&dir, None, HistoryWindow::AllTime)
            .unwrap()
            .is_empty());
        assert!(matches!(
            find_record_by_id_in(&dir, id, HistoryWindow::AllTime),
            Err(OneharnessError::HistoryNotFound { .. })
        ));
    }

    #[test]
    fn a_session_is_found_by_its_ids_date_or_by_name_under_a_project() {
        let dir = temp_dir("find-session");
        // The writer records under the canonical store (`/private/var/...` on
        // macOS, a `\\?\` verbatim path on Windows), and a lookup answers
        // under the directory it is handed, so the two are compared as one.
        let dir = fs::canonicalize(&dir).unwrap();
        let project = temp_dir("find-session-project");
        let session = closed_run(&dir, &project, "findable", id_at(now_secs(), 1), &[]);
        let stem = session.file_stem().unwrap().to_str().unwrap();
        let slug = session
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(
            find_session_path(&dir, None, stem).unwrap(),
            Some(session.clone())
        );
        assert_eq!(
            find_session_path(&dir, Some(slug), stem).unwrap(),
            Some(dir.join(slug).join(format!("{stem}.jsonl")))
        );
        for (slug, id) in [
            (None, "../escape"),
            (None, ""),
            (Some(".index.d"), stem),
            (Some("../up"), stem),
            (None, "no-date-in-it"),
            (None, "absent-20200101T000000Z-1"),
        ] {
            assert_eq!(
                find_session_path(&dir, slug, id).unwrap(),
                None,
                "{slug:?} {id}"
            );
        }
        assert!(remove_sessions(&dir, Some("../up")).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn reindex_names_what_it_cannot_read_and_dates_what_carries_no_timestamp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("reindex-edges");
        let project = temp_dir("reindex-edges-project");
        let other = temp_dir("reindex-edges-source");
        // A legacy 0.1-style run: a UUIDv5 id and a session stem with no date,
        // so its date is its record's timestamp.
        let minted = id_at(now_secs(), 1);
        let source = closed_run(&other, &project, "undated", minted, &[]);
        let text = fs::read_to_string(&source).unwrap();
        let v5 = HistoryId::legacy(b"undated");
        let rewritten = text.replace(&minted.to_string(), &v5.to_string());
        let slug_dir = dir.join("legacy-project");
        fs::create_dir_all(&slug_dir).unwrap();
        fs::write(slug_dir.join("undated.jsonl"), &rewritten).unwrap();
        // Unmigrated whole-record lines are not indexed, and said once.
        fs::write(
            slug_dir.join("unmigrated.jsonl"),
            "{\"schema_version\":\"0.3\",\"name\":\"old\"}\n",
        )
        .unwrap();
        // An unreadable file and an unreadable project directory.
        let unreadable = slug_dir.join("locked-20260101T000000Z-1.jsonl");
        fs::write(&unreadable, "{}\n").unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        let locked_dir = dir.join("locked-project");
        fs::create_dir_all(&locked_dir).unwrap();
        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o000)).unwrap();
        // SAFETY: geteuid has no preconditions. Root reads a mode-000 file.
        let root = unsafe { libc::geteuid() } == 0;

        let report = reindex(&dir).unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o755)).unwrap();
        if !root {
            assert_eq!(report.unreadable.len(), 2, "{report:?}");
        }
        let record_date = UtcDate::from_epoch_secs(now_epoch_secs()).to_string();
        assert!(report
            .segments
            .iter()
            .any(|segment| segment.segment == format!("runs-{record_date}.ndjson")));
        assert_eq!(
            find_record_by_id_in(&dir, v5, HistoryWindow::AllTime)
                .unwrap()
                .history_id,
            v5
        );
        assert!(matches!(
            find_record_by_id(&dir, v5),
            Err(OneharnessError::HistoryNotFound { .. })
        ));
    }

    #[test]
    fn the_reindex_sort_merges_many_runs_into_one_order_keeping_held_keys_first() {
        let scratch = temp_dir("reindex-sort");
        // A tiny chunk and a fan-in of two: every few records is a run, runs
        // cascade through several levels, and finishing merges what is left.
        let mut sort = ExternalSort::new(scratch.path(), 256, 2).unwrap();
        let ids: Vec<HistoryId> = (0..40u16).map(|n| id_at(now_secs(), n + 1)).collect();
        for (n, id) in ids.iter().enumerate().rev() {
            sort.push(
                IndexKey::Event(*id, n),
                Some(format!("candidate-{n}").as_bytes()),
            )
            .unwrap();
            if n % 3 == 0 {
                sort.push(IndexKey::Event(*id, n), None).unwrap();
            }
            sort.push(IndexKey::Run(*id), Some(b"run")).unwrap();
        }
        let mut records = Vec::new();
        sort.finish(|record| {
            records.push(record.to_vec());
            ControlFlow::Continue(())
        })
        .unwrap();
        assert_eq!(records.len(), 40 * 2 + 14);
        assert!(records.windows(2).all(|pair| pair[0] <= pair[1]));
        for (n, id) in ids.iter().enumerate() {
            let key = sort_key(IndexKey::Event(*id, n));
            let group: Vec<&Vec<u8>> = records
                .iter()
                .filter(|record| record.starts_with(key.as_bytes()))
                .collect();
            if n % 3 == 0 {
                assert_eq!(group[0][SORT_KEY_LEN], SORT_HELD);
            }
            assert!(group
                .last()
                .unwrap()
                .ends_with(format!("candidate-{n}").as_bytes()));
        }
        // A consumer that stops is handed no further record.
        let mut sort = ExternalSort::new(scratch.path(), 256, 2).unwrap();
        sort.push(IndexKey::Run(ids[0]), None).unwrap();
        let mut stopped = 0;
        sort.finish(|_| {
            stopped += 1;
            ControlFlow::Break(())
        })
        .unwrap();
        assert_eq!(stopped, 1);
    }

    #[test]
    fn exact_id_lookup_has_a_typed_not_found_error() {
        let dir = temp_dir("exact-id");
        let missing = HistoryId::from_uuid(uuid::Uuid::now_v7());
        let error = find_record_by_id(&dir, missing).unwrap_err();
        assert!(matches!(
            error,
            OneharnessError::HistoryNotFound { id } if id == missing.to_string()
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_begun_harness_run_appends_one_pointer_line_the_reader_gives_back() {
        let dir = temp_dir("pointer-store");
        let project = temp_dir("pointer-project");
        let pointers = temp_dir("pointer-file");
        // A parent directory that does not exist yet is made on first append.
        let pointer_file = pointers.path().join("nested").join("pointers.jsonl");
        let labels = HistoryLabels::new(BTreeMap::from([(
            "graph".to_string(),
            "release".to_string(),
        )]))
        .unwrap();
        let writer = HistoryWriter::open(&dir, &project, "point at me", labels.clone())
            .unwrap()
            .with_pointer_file(Some(pointer_file.clone()));
        assert_eq!(writer.pointer_file(), Some(pointer_file.as_path()));

        let first =
            writer.begin_harness_run(&"claude-code:primary".parse().unwrap(), "test prompt");
        let second = writer.begin_harness_run(&"codex".parse().unwrap(), "test prompt");
        writer
            .append_streamed(
                second,
                PermissionMode::Default,
                None,
                "prompt",
                &result("codex"),
                &BTreeSet::new(),
            )
            .unwrap();

        let read = read_pointers(&pointer_file).unwrap();
        assert_eq!(read.skipped, 0);
        assert_eq!(read.pointers.len(), 2, "one line per begun harness run");
        let [a, b] = read.pointers.as_slice() else {
            unreachable!()
        };
        assert_eq!(a.history_id(), first);
        assert_eq!(b.history_id(), second);
        assert_eq!(a.harness(), "claude-code");
        assert_eq!(a.variant(), Some("primary"));
        assert_eq!(a.harness_id(), "claude-code:primary");
        assert_eq!(b.harness(), "codex");
        assert_eq!(b.variant(), None);
        for pointer in [a, b] {
            assert_eq!(pointer.schema_version(), history::POINTER_SCHEMA_VERSION);
            assert_eq!(pointer.history_file(), writer.path().display().to_string());
            assert_eq!(pointer.history_dir(), writer.dir.display().to_string());
            assert_eq!(pointer.history_session(), writer.session.as_str());
            assert_eq!(pointer.history_project(), writer.relative_path.parts().0);
            assert_eq!(
                Path::new(&pointer.history_dir())
                    .join(pointer.history_project())
                    .join(format!("{}.{SESSION_EXT}", pointer.history_session())),
                writer.path()
            );
            assert_eq!(pointer.name(), "point-at-me");
            assert_eq!(pointer.project(), writer.project);
            assert_eq!(pointer.labels(), &labels);
            assert!(pointer.started().as_str().ends_with('Z'));
        }
        // The line's id is the record's id: the store's closing record for the
        // second run is what `history show <history-id>` resolves.
        let record = find_record_by_id(&dir, second).unwrap();
        assert_eq!(record.history_id, second);
    }

    #[test]
    fn a_pointer_line_goes_out_in_one_write_or_is_reported_short() {
        // A sink that takes only part of what it is offered: `write_all` would
        // come back for the rest, and the line's two halves could land around
        // another process's line. One write, and the short one is an error.
        struct Short(Vec<u8>);
        impl Write for Short {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let half = buf.len() / 2;
                self.0.extend_from_slice(&buf[..half]);
                Ok(half)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut short = Short(Vec::new());
        let error = write_whole_line(&mut short, b"{\"a\":1}\n").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WriteZero);
        assert_eq!(
            error.to_string(),
            "short write: 4 of 8 bytes of the pointer line"
        );
        assert_eq!(
            short.0, b"{\"a\"",
            "only the one write's bytes, never a second"
        );

        // An interrupted write wrote nothing, so the retry is still one write.
        struct InterruptedOnce(bool, Vec<u8>);
        impl Write for InterruptedOnce {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
                }
                self.1.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut interrupted = InterruptedOnce(false, Vec::new());
        write_whole_line(&mut interrupted, b"{\"a\":1}\n").unwrap();
        assert_eq!(interrupted.1, b"{\"a\":1}\n");
    }

    #[test]
    fn read_pointers_tolerates_a_missing_file_a_torn_tail_and_a_foreign_line() {
        let scratch = temp_dir("pointer-read");
        let missing = scratch.path().join("never-written.jsonl");
        assert_eq!(read_pointers(&missing).unwrap(), HistoryPointers::default());

        let dir = temp_dir("pointer-read-store");
        let project = temp_dir("pointer-read-project");
        let pointer_file = scratch.path().join("pointers.jsonl");
        let writer = HistoryWriter::open(&dir, &project, "torn", HistoryLabels::default())
            .unwrap()
            .with_pointer_file(Some(pointer_file.clone()));
        let first = writer.begin_harness_run(&"codex".parse().unwrap(), "test prompt");
        fs::OpenOptions::new()
            .append(true)
            .open(&pointer_file)
            .unwrap()
            .write_all(b"{\"not\": \"a pointer\"}\n")
            .unwrap();
        let second = writer.begin_harness_run(&"goose".parse().unwrap(), "test prompt");
        fs::OpenOptions::new()
            .append(true)
            .open(&pointer_file)
            .unwrap()
            .write_all(b"{\"schema_version\": \"1.0\", \"history_id\": \"0192")
            .unwrap();

        let read = read_pointers(&pointer_file).unwrap();
        assert_eq!(read.skipped, 2);
        assert_eq!(
            read.pointers
                .iter()
                .map(|pointer| pointer.history_id())
                .collect::<Vec<_>>(),
            vec![first, second],
            "lines come back in file order"
        );

        // A write that ended one byte short leaves a whole object with no
        // newline after it: still a torn tail, never a third record. Replace
        // the partial tail with the first pointer's own bytes, unterminated.
        let partial = b"{\"schema_version\": \"1.0\", \"history_id\": \"0192";
        let mut whole = fs::read(&pointer_file).unwrap();
        whole.truncate(whole.len() - partial.len());
        whole.extend_from_slice(&serde_json::to_vec(&read.pointers[0]).unwrap());
        fs::write(&pointer_file, &whole).unwrap();

        let read = read_pointers(&pointer_file).unwrap();
        assert_eq!(
            read.skipped, 2,
            "the unterminated tail replaces the partial one as the torn line"
        );
        assert_eq!(
            read.pointers
                .iter()
                .map(|pointer| pointer.history_id())
                .collect::<Vec<_>>(),
            vec![first, second],
            "a record without its newline is not read"
        );

        // A line that is not UTF-8 — a foreign line's bytes, or a tail torn
        // inside a multi-byte character — is that line's to skip, never the
        // whole file's to refuse; a blank line is neither read nor skipped.
        let mut whole = fs::read(&pointer_file).unwrap();
        whole.push(b'\n');
        whole.extend_from_slice(b"\n");
        whole.extend_from_slice(b"\xff\xfe not text\n");
        whole.extend_from_slice(b"   \n");
        whole.extend_from_slice(b"{\"schema_version\": \"1.0\", \"name\": \"caf\xc3");
        fs::write(&pointer_file, &whole).unwrap();
        let read = read_pointers(&pointer_file).unwrap();
        assert_eq!(
            read.pointers
                .iter()
                .map(|pointer| pointer.history_id())
                .collect::<Vec<_>>(),
            vec![first, second, first],
            "the copied record is complete now that its newline has landed"
        );
        assert_eq!(
            read.skipped, 3,
            "the foreign object, the non-UTF-8 line and the torn tail; not the blank lines"
        );
    }

    #[test]
    fn read_pointers_reports_a_path_that_exists_but_cannot_be_read() {
        // A directory where the file should be exists, so this is not the
        // missing-file case; it cannot be read as a file, and the read says so
        // rather than answering empty.
        let scratch = temp_dir("pointer-read-dir");
        let error = read_pointers(scratch.path()).unwrap_err();
        match &error {
            OneharnessError::HistoryIo { path, .. } => {
                assert_eq!(path, &scratch.path().display().to_string());
            }
            other => panic!("expected HistoryIo, got {other:?}"),
        }
        assert!(
            error
                .to_string()
                .starts_with("could not access history under "),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_pointer_file_skips_the_line_and_keeps_the_run() {
        let dir = temp_dir("pointer-unwritable-store");
        let project = temp_dir("pointer-unwritable-project");
        let scratch = temp_dir("pointer-unwritable");
        // A path under a regular file cannot be created.
        let blocker = scratch.path().join("blocker");
        fs::write(&blocker, b"").unwrap();
        let pointer_file = blocker.join("pointers.jsonl");
        let writer = HistoryWriter::open(&dir, &project, "blocked", HistoryLabels::default())
            .unwrap()
            .with_pointer_file(Some(pointer_file.clone()));
        let run_id = writer.begin_harness_run(&"codex".parse().unwrap(), "test prompt");
        let _ = writer.begin_harness_run(&"codex".parse().unwrap(), "test prompt");
        assert!(writer.pointer_warned.load(Ordering::Relaxed));
        writer
            .append_streamed(
                run_id,
                PermissionMode::Default,
                None,
                "prompt",
                &result("codex"),
                &BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(find_record_by_id(&dir, run_id).unwrap().history_id, run_id);
        assert!(!pointer_file.exists());
    }
}
