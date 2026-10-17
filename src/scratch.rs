//! Dispatch scratch has the lifetime of an ending, independent of run-end hooks.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Instant;

static TELEMETRY_ROOT: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

/// The CLI resolves its optional telemetry destination before executing a verb.
///
/// The destination is an environment value, so it is checked here, where it
/// enters: an unset or empty value records nothing, and one that is not an
/// absolute path is refused with a diagnostic and records nothing either, never
/// writing beside whatever directory the verb happened to start in. Telemetry is
/// optional, so a refused destination leaves the verb's outcome unchanged.
pub(crate) fn telemetry_to(value: Option<std::ffi::OsString>) {
    let root = value.filter(|value| !value.is_empty()).and_then(|value| {
        let root = std::path::PathBuf::from(&value);
        if root.is_absolute() {
            Some(root)
        } else {
            eprintln!(
                "onepipeline: scratch telemetry: ONEPIPELINE_SCRATCH_TELEMETRY_DIR must be \
                 an absolute path, not '{}'; nothing is recorded",
                root.display()
            );
            None
        }
    });
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
fn counted(is_dir: bool, inode: (u64, u64), nlink: u64, blocks: u64, seen: &mut Seen) -> u64 {
    // Removal changes the remaining alias's link count. Remember an inode
    // first seen with multiple links even after unlinking its earlier name.
    if !is_dir
        && ((nlink > 1 && !seen.links.insert(inode))
            || (nlink == 1 && !seen.links.is_empty() && seen.links.contains(&inode)))
    {
        return 0;
    }
    blocks * 512
}

#[cfg(all(unix, not(target_os = "linux")))]
fn allocated(_path: &Path, metadata: &fs::Metadata, seen: &mut Seen) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(counted(
        metadata.is_dir(),
        (metadata.dev(), metadata.ino()),
        metadata.nlink(),
        metadata.blocks(),
        seen,
    ))
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
#[cfg(not(target_os = "linux"))]
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
        // A Windows directory link or junction is a directory entry, which
        // `DeleteFileW` refuses; removing it removes the link, not its target.
        #[cfg(windows)]
        if std::os::windows::fs::FileTypeExt::is_symlink_dir(&metadata.file_type()) {
            return fs::remove_dir(path);
        }
        fs::remove_file(path)
    } else {
        Ok(())
    }
}

/// Linux traversal names every entry relative to its parent's open directory:
/// one lookup per entry, and a replacement link cannot redirect a later stat or
/// unlink into its target.
#[cfg(target_os = "linux")]
mod at {
    use super::{counted, Operation, Seen, Walk};
    use std::ffi::{CStr, CString};
    use std::io;
    use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    fn checked(result: libc::c_int) -> io::Result<libc::c_int> {
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result)
        }
    }

    // `nlink_t` is `u64` on x86_64 and `u32` on aarch64 Linux.
    #[allow(clippy::useless_conversion)]
    fn links(stat: &libc::stat) -> u64 {
        u64::from(stat.st_nlink)
    }

    pub(super) fn walk(
        parent: BorrowedFd<'_>,
        name: &CStr,
        operation: Operation,
        totals: &mut Walk,
        seen: &mut Seen,
    ) -> io::Result<()> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `parent` is an open directory, `name` is NUL-terminated, and
        // `stat` is writable storage of the type `fstatat` fills.
        checked(unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        })?;
        // SAFETY: success above promises the whole struct was filled.
        let stat = unsafe { stat.assume_init() };
        let is_dir = stat.st_mode & libc::S_IFMT == libc::S_IFDIR;
        totals.entries += 1;
        totals.bytes += counted(
            is_dir,
            (stat.st_dev, stat.st_ino),
            links(&stat),
            u64::try_from(stat.st_blocks).unwrap_or(0),
            seen,
        );
        if !is_dir {
            return match operation {
                Operation::Remove => unlink(parent, name, 0),
                Operation::Measure => Ok(()),
            };
        }
        let dir = open_dir(parent, name)?;
        let mut first_error = None;
        for child in entries(dir.as_fd())? {
            let result = child.and_then(|child| walk(dir.as_fd(), &child, operation, totals, seen));
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        if operation == Operation::Remove {
            if let Err(error) = unlink(parent, name, libc::AT_REMOVEDIR) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// `O_NOFOLLOW` refuses a name that became a link after it was stat'ed.
    pub(super) fn open_dir(parent: BorrowedFd<'_>, name: &CStr) -> io::Result<OwnedFd> {
        // SAFETY: `parent` is an open directory and `name` is NUL-terminated.
        let fd = checked(unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?;
        // SAFETY: `openat` returned a new descriptor that nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn unlink(parent: BorrowedFd<'_>, name: &CStr, flags: libc::c_int) -> io::Result<()> {
        // SAFETY: `parent` is an open directory and `name` is NUL-terminated.
        checked(unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) }).map(drop)
    }

    /// The open directory's own names, read through its descriptor: one
    /// resolution per directory rather than per entry.
    pub(super) fn entries(
        dir: BorrowedFd<'_>,
    ) -> io::Result<impl Iterator<Item = io::Result<CString>>> {
        Ok(
            std::fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))?
                .map(|entry| CString::new(entry?.file_name().as_bytes()).map_err(io::Error::other)),
        )
    }
}

/// The scratch root, pinned open before its directories are listed.
struct Anchored {
    path: std::path::PathBuf,
    #[cfg(target_os = "linux")]
    file: Option<fs::File>,
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
            file: Some(file),
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
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsFd;
            use std::os::unix::ffi::OsStrExt;
            let root = self
                .access
                .file
                .as_ref()
                .ok_or_else(|| io::Error::other("scratch root is not open"))?;
            let name = std::ffi::CString::new(name.as_bytes()).map_err(io::Error::other)?;
            at::walk(root.as_fd(), &name, operation, totals, seen)
        }
        #[cfg(not(target_os = "linux"))]
        walk(&self.access.path.join(name), operation, totals, seen)
    }
}

/// A run's scratch root, in the one spelling both the directory a dispatch is handed
/// and the record naming it are joined onto, so the two are equal on every platform.
pub(crate) fn root(paths: &RunPaths) -> io::Result<std::path::PathBuf> {
    Ok(root_spelled(fs::canonicalize(&paths.dir)?, cfg!(windows)))
}

/// The scratch root under a run directory's canonical path: the plain spelling, never
/// Windows' verbatim `\\?\` one, which nothing else on that platform spells a directory in.
fn root_spelled(canonical_run_dir: std::path::PathBuf, windows: bool) -> std::path::PathBuf {
    crate::templates::plain_on(canonical_run_dir, windows).join("scratch")
}

fn directories(paths: &RunPaths) -> io::Result<ScratchTree> {
    let root = root(paths)?;
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ScratchTree {
                access: Anchored {
                    path: root.clone(),
                    #[cfg(target_os = "linux")]
                    file: None,
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

/// A record stands for the ending it names until an edit changes the graph's
/// ending or a new dispatch makes the run live again, including work added while
/// an existing decision waits. A stop over a paused or failed run is an ending of
/// its own, so a record of the earlier one does not stand for it.
fn already_recorded(events: &[Envelope], ending: Option<ScratchEnding>) -> bool {
    let mut state = crate::projection::RunState {
        strict: true,
        ..Default::default()
    };
    let mut recorded = None;
    for event in events {
        let kind = PipelineKind::from_wire(&event.kind);
        let before = graph_ending(&state);
        crate::projection::fold_one(&mut state, event);
        if kind == Some(PipelineKind::NodeDispatched)
            || (kind == Some(PipelineKind::EditCommitted) && before != graph_ending(&state))
        {
            recorded = None;
        }
        match kind {
            Some(PipelineKind::ScratchRemoved) => recorded = Some(None),
            // A payload that does not read stands for no ending, so it can never
            // suppress a fresh record; the strict fold has admitted it in any case.
            Some(PipelineKind::ScratchRetained) => {
                recorded = serde_json::from_value::<ScratchRetained>(json!(event.payload))
                    .ok()
                    .map(|record| Some(record.ending));
            }
            _ => {}
        }
    }
    recorded == Some(ending)
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
    if !state.strict || crate::shutdown::begun(paths) {
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
    if already_recorded(events, ending) {
        return Ok(());
    }
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
pub(crate) struct ScratchRun(String);

impl ScratchRun {
    fn new(run: &str) -> Option<Self> {
        ledger::is_valid_run_id(run).then(|| Self(run.to_owned()))
    }
}

impl std::fmt::Display for ScratchRun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn no_run(run: &str) -> DiscardOutcome {
    DiscardOutcome::Refused(format!(
        "no run named '{run}' under the runs root; deleted nothing"
    ))
}

#[derive(Debug)]
pub(crate) enum DiscardOutcome {
    Refused(String),
    Removed {
        run: ScratchRun,
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

/// The refusal of a run that is still driven, unproven, or not at an ending.
fn not_ended(view: &RunView, run: &str) -> Option<DiscardOutcome> {
    let reading = views::reading(view);
    let shutdown = views::host_shutdown(view);
    let word = if shutdown {
        "HOST SHUTDOWN"
    } else {
        reading.word
    };
    (!view.state.strict || reading.driven || shutdown || reading.ending.is_none()).then(|| {
        DiscardOutcome::Refused(format!(
            "run {run} is {word}, not ended; its scratch is kept and nothing was deleted"
        ))
    })
}

pub(crate) fn discard(root: &Path, run: &str) -> crate::Result<DiscardOutcome> {
    let began = Instant::now();
    let Some(identity) = ScratchRun::new(run) else {
        return Ok(no_run(run));
    };
    let paths = RunPaths::under(root, run);
    if !paths.exists()
        || fs::symlink_metadata(&paths.dir).is_ok_and(|metadata| metadata.is_symlink())
    {
        return Ok(no_run(run));
    }
    // A refusal deletes nothing, so it is answered from the run as it reads now:
    // a shut-down driver journals its word before it lets go inside the gate.
    if let Some(refused) = not_ended(&RunView::open(&paths)?, run) {
        return Ok(refused);
    }
    let _handover = ledger::Handover::hold(&paths)?;
    if let Some(refused) = not_ended(&RunView::open(&paths)?, run) {
        return Ok(refused);
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
        run: identity,
        dirs: result.removed,
        failed: result.record.failed,
    })
}

fn gib(bytes: u64) -> String {
    crate::freespace::gib(bytes)
}

/// Views read only the latest record; the tree can have changed since it was kept.
///
/// The record speaks only while the run reads as an ending or a pause, each of
/// which writes its own, and a kept record only while the run reads as the ending
/// it was written at: a run made live again reads as what it is now, so a kept
/// notice and its discard instruction, which the verb would refuse, wait for the
/// next ending.
pub(crate) fn lines(view: &RunView, cap: Option<usize>) -> String {
    if views::host_shutdown(view) || view.liveness() == views::DriverLiveness::Parked {
        return String::new();
    }
    let reading = views::reading(view);
    let current = match (&reading.ending, &reading.paused) {
        (Some(ending), _) => match ending.kind {
            views::EndingKind::Complete => None,
            views::EndingKind::Failed => Some(ScratchEnding::Failed),
            views::EndingKind::Unfinished => Some(ScratchEnding::Unfinished),
            views::EndingKind::Stopped => Some(ScratchEnding::Stopped),
        },
        (None, Some(_)) => Some(ScratchEnding::Paused),
        (None, None) => return String::new(),
    };
    let Some(event) = view.events.iter().rev().find(|event| {
        matches!(
            PipelineKind::from_wire(&event.kind),
            Some(PipelineKind::ScratchRemoved | PipelineKind::ScratchRetained)
        )
    }) else {
        return String::new();
    };
    if PipelineKind::from_wire(&event.kind) == Some(PipelineKind::ScratchRemoved) {
        // llmlint: ignore-block[changed_behavior_has_e2e] The real journal boundary
        // rejects malformed registered payloads before rendering, proven by
        // contract::scratch_payloads_reject_relative_paths_and_empty_errors_at_the_registered_boundary.
        // This defensive refusal cannot be reached by a current CLI producer.
        let Ok(record) = serde_json::from_value::<ScratchRemoved>(json!(event.payload)) else {
            return String::new();
        };
        // llmlint: ignore-end[changed_behavior_has_e2e]
        return record.failed.first().map_or_else(String::new, |first| {
            format!(
                "  scratch: {} dir(s) could not be removed: {}: {}\n",
                record.failed.len(),
                first.path.display(),
                first.error
            )
        });
    }
    // llmlint: ignore-block[changed_behavior_has_e2e] The real journal boundary
    // rejects malformed registered payloads before rendering, proven by
    // contract::scratch_payloads_reject_relative_paths_and_empty_errors_at_the_registered_boundary.
    // This defensive refusal cannot be reached by a current CLI producer.
    let Ok(mut record) = serde_json::from_value::<ScratchRetained>(json!(event.payload)) else {
        return String::new();
    };
    // llmlint: ignore-end[changed_behavior_has_e2e]
    // llmlint: ignore-block[changed_behavior_has_e2e] every current producer
    // records the ending the run reads as (a stop over a pause or a failure
    // records its own, proven by
    // scratch::a_stop_over_a_paused_or_failed_run_records_its_stopped_ending), so a
    // mismatched record is reached only from an earlier build's journal, which
    // scratch::a_retained_record_for_another_ending_than_the_runs_reading_prints_no_notice
    // stands in for through the real status and results commands.
    if current != Some(record.ending) {
        return String::new();
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
    largest_first(&mut record.dirs);
    let suffix = if record.ending == ScratchEnding::Paused {
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
        let pinned = pinned.file.unwrap();
        let mut total = Walk::default();
        at::walk(
            std::os::fd::AsFd::as_fd(&pinned),
            c"inside",
            Operation::Remove,
            &mut total,
            &mut Seen::default(),
        )
        .unwrap();
        assert!(!displaced.join("inside").exists());
        assert_eq!(fs::read(outside.join("target")).unwrap(), b"external bytes");
        assert!(fs::symlink_metadata(&original).unwrap().is_symlink());
        assert!(anchor(&original).is_err());
        // A directory name swapped for a link between its stat and its open is
        // refused at the open, so nothing beneath the link's target is walked.
        let parent = anchor(&temp).unwrap().file.unwrap();
        assert!(at::open_dir(std::os::fd::AsFd::as_fd(&parent), c"dispatch").is_err());
        assert_eq!(fs::read(outside.join("target")).unwrap(), b"external bytes");
        fs::remove_dir_all(temp).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows' `canonicalize` answers a run directory in the verbatim form, and the
    /// scratch root a dispatch is handed and the record names is the plain one.
    #[test]
    fn a_verbatim_run_directory_spells_the_scratch_root_the_record_names() {
        for (verbatim, spelled) in [
            (r"\\?\C:\op\runs\r", r"C:\op\runs\r"),
            (r"\\?\UNC\server\share\runs\r", r"\\server\share\runs\r"),
        ] {
            let root = root_spelled(verbatim.into(), true);
            assert_eq!(root, std::path::Path::new(spelled).join("scratch"));
            assert_eq!(root, root_spelled(spelled.into(), true));
            assert!(!root.to_str().unwrap().starts_with(r"\\?\"), "{root:?}");
        }
        let unix = std::path::PathBuf::from("/home/me/runs/r");
        assert_eq!(root_spelled(unix.clone(), false), unix.join("scratch"));
    }
}
