//! Dispatch scratch has the lifetime of an ending, independent of run-end hooks.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Instant;

static TELEMETRY_ROOT: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

/// The CLI resolves its optional telemetry destination before executing a verb.
pub(crate) fn telemetry_to(root: Option<std::path::PathBuf>) {
    let _ = TELEMETRY_ROOT.set(root);
}

use serde_json::json;

use crate::event::{Envelope, PipelineKind};
use crate::journal::{self, Journal};
use crate::ledger::{self, RunPaths};
use crate::payload::{
    AbsolutePath, ScratchDir, ScratchEnding, ScratchError, ScratchFailure, ScratchRemoved,
    ScratchRetained, ScratchTrigger,
};
use crate::views::{self, RunView};

/// One walk's accounting, kept off the journal's pinned payload surface.
#[derive(Default)]
struct Walk {
    bytes: u64,
    entries: u64,
}

/// `du` counts a hard-linked inode once, without keeping an entry for every
/// ordinary file in a large tree.
#[derive(Default)]
struct Seen {
    links: std::collections::HashSet<(u64, u64)>,
}

#[cfg(unix)]
fn allocated(_path: &Path, metadata: &fs::Metadata, seen: &mut Seen) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    if !metadata.is_dir() {
        let inode = (metadata.dev(), metadata.ino());
        // Removal changes the remaining alias's link count. Remember an inode
        // first seen with multiple links even after unlinking its earlier name.
        if (metadata.nlink() > 1 && !seen.links.insert(inode))
            || (metadata.nlink() == 1 && !seen.links.is_empty() && seen.links.contains(&inode))
        {
            return Ok(0);
        }
    }
    Ok(metadata.blocks() * 512)
}

#[cfg(windows)]
fn allocated(path: &Path, _metadata: &fs::Metadata, seen: &mut Seen) -> io::Result<u64> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileStandardInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_STANDARD_INFO,
    };
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let mut info = std::mem::MaybeUninit::<FILE_STANDARD_INFO>::uninit();
    // SAFETY: the live file handle is opened on the link itself, and `info` is
    // writable storage of the requested class, with its exact size passed.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStandardInfo,
            info.as_mut_ptr().cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: success above promises that the whole standard-info struct was filled.
    let info = unsafe { info.assume_init() };
    if !info.Directory && (info.NumberOfLinks > 1 || !seen.links.is_empty()) {
        let mut identity = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: this is the same live handle and writable storage of the exact type.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), identity.as_mut_ptr()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: success above promises that the whole identity struct was filled.
        let identity = unsafe { identity.assume_init() };
        let inode = (
            u64::from(identity.dwVolumeSerialNumber),
            (u64::from(identity.nFileIndexHigh) << 32) | u64::from(identity.nFileIndexLow),
        );
        if (info.NumberOfLinks > 1 && !seen.links.insert(inode))
            || (info.NumberOfLinks == 1 && seen.links.contains(&inode))
        {
            return Ok(0);
        }
    }
    u64::try_from(info.AllocationSize)
        .map_err(|_| io::Error::other("negative filesystem allocation size"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    Measure,
    Remove,
}

/// Links are measured and unlinked themselves, never traversed.
fn walk(path: &Path, operation: Operation, totals: &mut Walk, seen: &mut Seen) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    totals.entries += 1;
    totals.bytes += allocated(path, &metadata, seen)?;
    if metadata.is_dir() {
        let anchored = anchor(path)?;
        let mut first_error = None;
        for entry in fs::read_dir(&anchored.path)? {
            let result = entry.and_then(|entry| walk(&entry.path(), operation, totals, seen));
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        if operation == Operation::Remove {
            if let Err(error) = fs::remove_dir(path) {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    } else if operation == Operation::Remove {
        fs::remove_file(path)
    } else {
        Ok(())
    }
}

/// Linux traversal pins each directory before reading it. A replacement link
/// cannot redirect a later stat or unlink into its target.
struct Anchored {
    path: std::path::PathBuf,
    #[cfg(target_os = "linux")]
    _file: Option<fs::File>,
}

fn anchor(path: &Path) -> io::Result<Anchored> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let path = std::path::PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
        Ok(Anchored {
            path,
            _file: Some(file),
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(Anchored {
            path: path.to_path_buf(),
        })
    }
}

struct ScratchTree {
    root: std::path::PathBuf,
    access: Anchored,
    dirs: Vec<std::path::PathBuf>,
}

impl ScratchTree {
    fn walk(
        &self,
        path: &Path,
        operation: Operation,
        totals: &mut Walk,
        seen: &mut Seen,
    ) -> io::Result<()> {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("scratch directory has no name"))?;
        walk(&self.access.path.join(name), operation, totals, seen)
    }
}

fn directories(paths: &RunPaths) -> io::Result<ScratchTree> {
    let root = fs::canonicalize(&paths.dir)?.join("scratch");
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ScratchTree {
                access: Anchored {
                    path: root.clone(),
                    #[cfg(target_os = "linux")]
                    _file: None,
                },
                root,
                dirs: Vec::new(),
            })
        }
        Err(error) => return Err(error),
        Ok(metadata) if metadata.is_symlink() => {
            return Err(io::Error::other(format!(
                "{} is a symlink; nothing was deleted",
                root.display()
            )));
        }
        Ok(_) => {}
    }
    AbsolutePath::new(root.clone()).map_err(io::Error::other)?;
    let access = anchor(&root)?;
    let dirs = fs::read_dir(&access.path)?
        .map(|entry| {
            let path = root.join(entry?.file_name());
            AbsolutePath::new(path.clone()).map_err(io::Error::other)?;
            Ok(path)
        })
        .collect::<io::Result<_>>()?;
    Ok(ScratchTree { root, access, dirs })
}

fn absolute(path: &Path) -> crate::Result<AbsolutePath> {
    AbsolutePath::new(path.to_path_buf()).map_err(|error| crate::Error::Invalid(error.into()))
}

fn failure_reason(error: io::Error) -> crate::Result<ScratchError> {
    ScratchError::new(error.to_string()).map_err(|error| crate::Error::Invalid(error.into()))
}

fn largest_first(dirs: &mut [ScratchDir]) {
    dirs.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
}

fn emit<T: serde::Serialize>(paths: &RunPaths, kind: PipelineKind, value: &T) -> crate::Result<()> {
    let payload =
        serde_json::to_value(value).map_err(|error| crate::Error::Invalid(error.to_string()))?;
    let payload = payload
        .as_object()
        .cloned()
        .ok_or_else(|| crate::Error::Invalid("scratch record is not an object".into()))?;
    Journal::open(paths).emit(kind, journal::labels(&paths.run, None), payload)
}

/// A record stands until a graph edit changes its ending.
fn already_recorded(events: &[Envelope]) -> bool {
    let mut state = crate::projection::RunState {
        strict: true,
        ..Default::default()
    };
    let mut recorded = false;
    for event in events {
        let kind = PipelineKind::from_wire(&event.kind);
        let before = graph_ending(&state);
        crate::projection::fold_one(&mut state, event);
        if kind == Some(PipelineKind::EditCommitted) && before != graph_ending(&state) {
            recorded = false;
        }
        if matches!(
            kind,
            Some(PipelineKind::ScratchRemoved | PipelineKind::ScratchRetained)
        ) {
            recorded = true;
        }
    }
    recorded
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GraphEnding {
    Live,
    Success,
    Failure,
}

fn graph_ending(state: &crate::projection::RunState) -> GraphEnding {
    let statuses = state.statuses();
    if statuses.values().any(|status| {
        matches!(
            status,
            crate::graph::NodeStatus::Ready
                | crate::graph::NodeStatus::Running
                | crate::graph::NodeStatus::Waiting
                | crate::graph::NodeStatus::CompleteDraft
        )
    }) {
        GraphEnding::Live
    } else if statuses
        .values()
        .all(|status| *status == crate::graph::NodeStatus::Done)
    {
        GraphEnding::Success
    } else {
        GraphEnding::Failure
    }
}

/// The first ending producer records it, under the same gate as reply handover.
pub(crate) fn at_ending(paths: &RunPaths) {
    if let Err(error) = ending(paths) {
        eprintln!(
            "onepipeline: scratch of run {} could not be handled: {error}",
            paths.run
        );
    }
}

/// A driver or undriven reply calls this while it still owns the run and holds
/// the final handover. Its own fold decides the ending; liveness is irrelevant.
pub(crate) fn at_writer_ending(paths: &RunPaths, state: &crate::projection::RunState) {
    if let Err(error) = handle_ending(paths, state, &journal::read(&paths.journal())) {
        eprintln!(
            "onepipeline: scratch of run {} could not be handled: {error}",
            paths.run
        );
    }
}

fn ending(paths: &RunPaths) -> crate::Result<()> {
    let _handover = ledger::Handover::hold(paths)?;
    let view = RunView::open(paths)?;
    if views::reading(&view).driven {
        return Ok(());
    }
    let _ownership = ledger::OwnershipLock::acquire(paths, "scratch")?;
    handle_ending(paths, &view.state, &view.events)
}

fn handle_ending(
    paths: &RunPaths,
    state: &crate::projection::RunState,
    events: &[Envelope],
) -> crate::Result<()> {
    if !state.strict || crate::shutdown::begun(paths) || already_recorded(events) {
        return Ok(());
    }
    let present: std::collections::BTreeSet<_> = state.statuses().values().copied().collect();
    if !state.stop_recorded()
        && present.iter().any(|status| {
            matches!(
                status,
                crate::graph::NodeStatus::Ready | crate::graph::NodeStatus::Running
            )
        })
    {
        return Ok(());
    }
    let verdict = crate::hooks::verdict(&present, || views::decision_outstanding(state, paths));
    let ending = match verdict {
        crate::hooks::Verdict::Success => None,
        _ if state.stop_recorded() => Some(ScratchEnding::Stopped),
        crate::hooks::Verdict::Paused => Some(ScratchEnding::Paused),
        crate::hooks::Verdict::Failure(crate::hooks::ReasonKind::Nodes) => {
            Some(ScratchEnding::Failed)
        }
        crate::hooks::Verdict::Failure(crate::hooks::ReasonKind::Unfinished) => {
            Some(ScratchEnding::Unfinished)
        }
        crate::hooks::Verdict::Failure(crate::hooks::ReasonKind::Stopped) => {
            Some(ScratchEnding::Stopped)
        }
        crate::hooks::Verdict::NotEnded => return Ok(()),
    };
    let began = Instant::now();
    let dirs = directories(paths).map_err(|source| crate::Error::Ledger {
        path: paths.dir.join("scratch"),
        source,
    })?;
    if let Some(ending) = ending {
        let mut measured = Vec::new();
        let mut entries = 0;
        let mut seen = Seen::default();
        for path in &dirs.dirs {
            let mut total = Walk::default();
            dirs.walk(path, Operation::Measure, &mut total, &mut seen)
                .map_err(|source| crate::Error::Ledger {
                    path: path.clone(),
                    source,
                })?;
            entries += total.entries;
            measured.push(ScratchDir {
                path: absolute(path)?,
                bytes: total.bytes,
            });
        }
        largest_first(&mut measured);
        let retained = ScratchRetained {
            ending,
            total_bytes: measured.iter().map(|dir| dir.bytes).sum(),
            dirs: measured,
        };
        telemetry(
            paths,
            Measurement::Retained,
            entries,
            retained.dirs.len(),
            retained.total_bytes,
            began.elapsed().as_secs_f64(),
        );
        emit(paths, PipelineKind::ScratchRetained, &retained)
    } else {
        let removed = remove(paths, dirs, ScratchTrigger::Success)?;
        telemetry(
            paths,
            Measurement::Success,
            removed.entries,
            removed.walked_dirs,
            removed.walked_bytes,
            began.elapsed().as_secs_f64(),
        );
        Ok(())
    }
}

struct Removal {
    record: ScratchRemoved,
    removed: Vec<ScratchDir>,
    entries: u64,
    walked_dirs: usize,
    walked_bytes: u64,
}

fn remove(paths: &RunPaths, dirs: ScratchTree, trigger: ScratchTrigger) -> crate::Result<Removal> {
    let mut result = Removal {
        record: ScratchRemoved {
            trigger,
            dirs: dirs.dirs.len() as u64,
            bytes: 0,
            failed: Vec::new(),
        },
        removed: Vec::new(),
        entries: 0,
        walked_dirs: dirs.dirs.len(),
        walked_bytes: 0,
    };
    let mut seen = Seen::default();
    for path in &dirs.dirs {
        let mut total = Walk::default();
        match dirs.walk(path, Operation::Remove, &mut total, &mut seen) {
            Ok(()) => result.removed.push(ScratchDir {
                path: absolute(path)?,
                bytes: total.bytes,
            }),
            Err(error) => result.record.failed.push(ScratchFailure {
                path: absolute(path)?,
                error: failure_reason(error)?,
            }),
        }
        result.entries += total.entries;
        result.walked_bytes += total.bytes;
    }
    let root = dirs.root;
    if result.record.failed.is_empty() {
        // llmlint: ignore-block[changed_behavior_has_e2e] A root-only rmdir refusal
        // needs foreign ownership, a mount or a race: making the run dir readonly
        // prevents a CLI claim or driver closeout before reaching this branch. The
        // manager approved this defensive site; dispatch-dir refusal is real e2e.
        if let Err(error) = fs::remove_dir(&root) {
            if error.kind() != io::ErrorKind::NotFound {
                result.record.failed.push(ScratchFailure {
                    path: absolute(&root)?,
                    error: failure_reason(error)?,
                });
            }
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]
    }
    largest_first(&mut result.removed);
    result.record.dirs = result.removed.len() as u64;
    result.record.bytes = result.removed.iter().map(|dir| dir.bytes).sum();
    emit(paths, PipelineKind::ScratchRemoved, &result.record)?;
    Ok(result)
}

#[derive(Clone, Copy)]
enum Measurement {
    Retained,
    Success,
    Discard,
}

impl Measurement {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Retained => "retained",
            Self::Success => "success",
            Self::Discard => "discard",
        }
    }
}

fn telemetry(
    paths: &RunPaths,
    trigger: Measurement,
    entries: u64,
    dirs: usize,
    bytes: u64,
    seconds: f64,
) {
    // Opt-in measurements belong to the workload invocation, never a run's journal.
    if let Some(root) = TELEMETRY_ROOT.get().and_then(Option::as_ref) {
        let path = root.join(format!("{}-{}.json", paths.run, trigger.as_str()));
        if let Err(error) = ledger::write_json(
            &path,
            &json!({ "entries": entries, "dirs": dirs, "bytes": bytes, "seconds": seconds }),
        ) {
            eprintln!("onepipeline: scratch telemetry: {error}");
        }
    }
}

#[derive(Debug)]
pub(crate) enum DiscardOutcome {
    Refused(String),
    Removed {
        run: String,
        dirs: Vec<ScratchDir>,
        failed: Vec<ScratchFailure>,
    },
}

impl DiscardOutcome {
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::Refused(_) => 2,
            Self::Removed { failed, .. } => i32::from(!failed.is_empty()),
        }
    }

    pub(crate) fn stdout(&self) -> String {
        let Self::Removed { run, dirs, .. } = self else {
            return String::new();
        };
        if dirs.is_empty() {
            return format!("removed 0 scratch dir(s) of run {run}, 0.0 GiB freed\n");
        }
        let mut out = format!(
            "removed {} scratch dir(s) of run {run}, {} freed:\n",
            dirs.len(),
            gib(dirs.iter().map(|dir| dir.bytes).sum())
        );
        for dir in dirs {
            out.push_str(&format!("  {}  {}\n", dir.path.display(), gib(dir.bytes)));
        }
        out
    }

    pub(crate) fn stderr(&self) -> String {
        match self {
            Self::Refused(why) => format!("onepipeline scratch discard: {why}\n"),
            Self::Removed { failed, .. } => failed
                .iter()
                .map(|failure| {
                    format!(
                        "onepipeline scratch discard: could not remove {}: {}\n",
                        failure.path.display(),
                        failure.error
                    )
                })
                .collect(),
        }
    }
}

pub(crate) fn discard(root: &Path, run: &str) -> crate::Result<DiscardOutcome> {
    let began = Instant::now();
    let paths = RunPaths::under(root, run);
    let valid = ledger::is_valid_run_id(run);
    if !valid
        || !paths.exists()
        || fs::symlink_metadata(&paths.dir).is_ok_and(|metadata| metadata.is_symlink())
    {
        return Ok(DiscardOutcome::Refused(format!(
            "no run named '{run}' under the runs root; deleted nothing"
        )));
    }
    let _handover = ledger::Handover::hold(&paths)?;
    let view = RunView::open(&paths)?;
    let reading = views::reading(&view);
    let word = if views::host_shutdown(&view) {
        "HOST SHUTDOWN"
    } else {
        reading.word
    };
    if !view.state.strict
        || reading.driven
        || views::host_shutdown(&view)
        || reading.ending.is_none()
    {
        return Ok(DiscardOutcome::Refused(format!(
            "run {run} is {word}, not ended; its scratch is kept and nothing was deleted"
        )));
    }
    let _ownership = ledger::OwnershipLock::acquire(&paths, "scratch")?;
    let dirs = match directories(&paths) {
        Ok(dirs) => dirs,
        Err(error) => return Ok(DiscardOutcome::Refused(error.to_string())),
    };
    let state_seconds = began.elapsed().as_secs_f64();
    let removal_began = Instant::now();
    let result = remove(&paths, dirs, ScratchTrigger::Discard)?;
    telemetry(
        &paths,
        Measurement::Discard,
        result.entries,
        result.walked_dirs,
        result.walked_bytes,
        removal_began.elapsed().as_secs_f64(),
    );
    if let Some(root) = TELEMETRY_ROOT.get().and_then(Option::as_ref) {
        if let Err(error) = ledger::write_json(
            &root.join(format!("{run}-discard-state.json")),
            &json!({"seconds": state_seconds}),
        ) {
            eprintln!("onepipeline: scratch telemetry: {error}");
        }
    }
    Ok(DiscardOutcome::Removed {
        run: run.to_string(),
        dirs: result.removed,
        failed: result.record.failed,
    })
}

fn gib(bytes: u64) -> String {
    crate::freespace::gib(bytes)
}

/// Views read only the latest record; the tree can have changed since it was kept.
pub(crate) fn lines(view: &RunView, cap: Option<usize>) -> String {
    if views::host_shutdown(view) || view.liveness() == views::DriverLiveness::Parked {
        return String::new();
    }
    let Some(event) = view.events.iter().rev().find(|event| {
        matches!(
            PipelineKind::from_wire(&event.kind),
            Some(PipelineKind::ScratchRemoved | PipelineKind::ScratchRetained)
        )
    }) else {
        return String::new();
    };
    if PipelineKind::from_wire(&event.kind) == Some(PipelineKind::ScratchRemoved) {
        let Ok(record) = serde_json::from_value::<ScratchRemoved>(json!(event.payload)) else {
            return String::new();
        };
        return record.failed.first().map_or_else(String::new, |first| {
            format!(
                "  scratch: {} dir(s) could not be removed: {}: {}\n",
                record.failed.len(),
                first.path.display(),
                first.error
            )
        });
    }
    let Ok(mut record) = serde_json::from_value::<ScratchRetained>(json!(event.payload)) else {
        return String::new();
    };
    largest_first(&mut record.dirs);
    let suffix = if views::reading(view).paused.is_some() {
        format!(
            "kept while paused; discard with onepipeline scratch discard {} once the run ends",
            view.paths.run
        )
    } else {
        format!(
            "remove with: onepipeline scratch discard {}",
            view.paths.run
        )
    };
    let mut out = format!(
        "  scratch kept: {} dir(s), {} — {suffix}\n",
        record.dirs.len(),
        gib(record.total_bytes)
    );
    let shown = cap.unwrap_or(record.dirs.len()).min(record.dirs.len());
    for dir in &record.dirs[..shown] {
        out.push_str(&format!("    {}  {}\n", dir.path.display(), gib(dir.bytes)));
    }
    if shown < record.dirs.len() {
        out.push_str(&format!(
            "    … and {} more, {} — full list: onepipeline results {}\n",
            record.dirs.len() - shown,
            gib(record.dirs[shown..].iter().map(|dir| dir.bytes).sum()),
            view.paths.run
        ));
    }
    out
}

#[cfg(all(test, target_os = "linux"))]
mod traversal_boundary {
    #[test]
    fn an_open_directory_stays_pinned_when_its_name_becomes_an_external_link() {
        use super::*;
        use std::os::unix::fs::symlink;
        let temp =
            std::env::temp_dir().join(format!("onepipeline-pinned-scratch-{}", std::process::id()));
        fs::create_dir(&temp).unwrap();
        let original = temp.join("dispatch");
        let displaced = temp.join("displaced");
        let outside = temp.join("outside");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(original.join("inside"), b"dispatch bytes").unwrap();
        fs::write(outside.join("target"), b"external bytes").unwrap();
        // This deterministic filesystem seam proves the replacement race at the
        // exact opened-handle boundary; a subprocess race cannot guarantee that
        // replacement falls between open and traversal without adding a test hook.
        let pinned = anchor(&original).unwrap();
        fs::rename(&original, &displaced).unwrap();
        symlink(&outside, &original).unwrap();
        let mut total = Walk::default();
        walk(
            &pinned.path.join("inside"),
            Operation::Remove,
            &mut total,
            &mut Seen::default(),
        )
        .unwrap();
        assert!(!displaced.join("inside").exists());
        assert_eq!(fs::read(outside.join("target")).unwrap(), b"external bytes");
        assert!(fs::symlink_metadata(&original).unwrap().is_symlink());
        assert!(anchor(&original).is_err());
        fs::remove_dir_all(temp).unwrap();
    }
}
