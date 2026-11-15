//! Prototype (spike-git-reads, never lands): fewer `git` starts for the reads an
//! engine repeats most.
//!
//! Every git command this crate runs reaches [`Call`] immediately before its
//! process would start, after the crate's own memo, pins and recovery proofs. One
//! mode, chosen once per process, decides what may answer it instead:
//!
//! - `batched`: one long-lived `git cat-file --batch-check` per repository answers
//!   object and ref resolution (`show-ref --verify`, `cat-file -e`, `rev-parse`
//!   of one name), each line resolved by git afresh.
//! - `native`: libgit2 answers the same reads plus `symbolic-ref`,
//!   `rev-parse --abbrev-ref HEAD`, `rev-parse --is-inside-work-tree` and
//!   `merge-base --is-ancestor`, from a repository opened for that one call.
//! - `reuse`: a read asked again in this process is answered from the first
//!   answer while nothing that answer depends on has changed on disk — every ref,
//!   `HEAD`, configuration and layout file stamped before it was read — and any
//!   write this process makes forgets everything.
//! - `reuse+batched`, `reuse+native`: reuse in front of the reader.
//!
//! Anything a reader cannot answer exactly as git would is left to git: a miss
//! the reader cannot tell apart from a damaged repository, a name two refs could
//! mean, a repository relocated by `GIT_*`, shallow, grafted or replaced history,
//! a repository with `extensions`, and everything not listed above. The batched
//! reader also delegates packed and borrowed object stores: cat-file retains
//! deleted pack mappings, so a long-lived reader cannot prove their freshness.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crate::git::{Ended, Output};

/// The mode a build answers with when `ONEVCS_SPIKE_GIT_READS` is unset: the one
/// line a demonstration commit edits.
const DEMONSTRATION: &str = "stock";

/// Overrides [`DEMONSTRATION`] for one process.
pub const MODE_ENV: &str = "ONEVCS_SPIKE_GIT_READS";

/// Which reader answers a read the memo does not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reader {
    /// git itself, one process per read.
    Git,
    /// One `cat-file --batch-check` process per repository.
    Batched,
    /// libgit2, in this process.
    Native,
}

/// The prototype a process runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// Whether a repeated read is answered from this process's earlier answer.
    pub reuse: bool,
    /// What answers a read nothing remembers.
    pub reader: Reader,
}

impl Mode {
    /// Every read is a git process, as released.
    pub const STOCK: Mode = Mode {
        reuse: false,
        reader: Reader::Git,
    };

    /// The mode a word names: `stock`, `batched`, `native`, `reuse`,
    /// `reuse+batched` or `reuse+native`.
    pub fn parse(word: &str) -> Option<Mode> {
        let (reuse, reader) = match word.strip_prefix("reuse") {
            Some("") => (true, "stock"),
            Some(rest) => (true, rest.strip_prefix('+')?),
            None => (false, word),
        };
        let reader = match reader {
            "stock" => Reader::Git,
            "batched" => Reader::Batched,
            "native" => Reader::Native,
            _ => return None,
        };
        Some(Mode { reuse, reader })
    }
}

#[cfg(test)]
thread_local! {
    static TEST_MODE: std::cell::Cell<Option<Mode>> = const { std::cell::Cell::new(None) };
}

/// Run `read` on this thread under `mode`, whatever the process runs.
#[cfg(test)]
pub(crate) fn with_mode<T>(mode: Mode, read: impl FnOnce() -> T) -> T {
    let before = TEST_MODE.with(|cell| cell.replace(Some(mode)));
    let answered = read();
    TEST_MODE.with(|cell| cell.set(before));
    answered
}

/// The mode this process runs: `ONEVCS_SPIKE_GIT_READS`, or [`DEMONSTRATION`]
/// when unset. An invalid explicit value ends this measurement process with exit 2.
// llmlint: ignore-block[changed_behavior_has_e2e] local-slice.sh runs real engine subprocesses in all six environment-selected modes; once-per-process selection and fallback are measurement setup, never a shipped configurable interface.
pub fn mode() -> Mode {
    #[cfg(test)]
    if let Some(mode) = TEST_MODE.with(std::cell::Cell::get) {
        return mode;
    }
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let word = match std::env::var(MODE_ENV) {
            Ok(word) => word,
            Err(std::env::VarError::NotPresent) => DEMONSTRATION.to_owned(),
            Err(std::env::VarError::NotUnicode(_)) => invalid_mode(),
        };
        Mode::parse(word.trim()).unwrap_or_else(|| invalid_mode())
    })
}
// llmlint: ignore-end[changed_behavior_has_e2e]

fn invalid_mode() -> ! {
    eprintln!("{MODE_ENV} must name stock, batched, native, reuse, reuse+batched or reuse+native; next: correct or unset that measurement variable");
    std::process::exit(2)
}

/// The variables that change which repository, refs, objects or configuration git
/// reads. A read made under any of them, in the call or in this process, is git's.
const RELOCATING: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_REPLACE_REF_BASE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_QUARANTINE_PATH",
    "GIT_REF_PARANOIA",
    "GIT_DEFAULT_HASH",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_SHALLOW_FILE",
    "GIT_GRAFT_FILE",
];

fn relocated(env: &[(String, String)]) -> bool {
    env.iter()
        .any(|(key, _)| RELOCATING.contains(&key.as_str()))
        || RELOCATING.iter().any(|key| std::env::var_os(key).is_some())
}

/// One git command about to start, and what the prototype knows of it.
pub(crate) struct Call {
    mode: Mode,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    env: Vec<(String, String)>,
    layout: Option<Layout>,
    /// The stamps taken before anything read: what a remembered answer is valid for.
    stamps: Option<Vec<Stamp>>,
}

impl Call {
    /// `fed` is a command given standard input, which nothing here answers.
    pub(crate) fn new(
        args: &[&str],
        cwd: Option<&Path>,
        env: &[(String, String)],
        fed: bool,
    ) -> Call {
        let mode = mode();
        let mut call = Call {
            mode,
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: cwd.map(Path::to_path_buf),
            env: env.to_vec(),
            layout: None,
            stamps: None,
        };
        if mode == Mode::STOCK {
            return call;
        }
        // llmlint: ignore-block[changed_behavior_has_e2e] This never-landing stdin guard conservatively delegates and invalidates; no prototype answers a fed query. Production stdin/failure injection is recorded as an unproven branch rather than changing the measured Git interface to synthesize replies.
        if fed || !is_read(args) {
            reuse::forget();
            if destroys(args) {
                batched::release();
            }
            call.mode = Mode::STOCK;
            return call;
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]
        if relocated(env) {
            call.mode = Mode::STOCK;
            return call;
        }
        call.layout = cwd.and_then(layout);
        if mode.reuse && reuse::reusable(args) {
            if let Some(layout) = &call.layout {
                call.stamps = reuse::stamps(layout, cwd, env);
            }
        }
        call
    }

    /// The answer, where something other than a new git process can give it.
    pub(crate) fn answer(&self) -> Option<Output> {
        if self.mode == Mode::STOCK {
            return None;
        }
        let args: Vec<&str> = self.args.iter().map(String::as_str).collect();
        if let Some(stamps) = &self.stamps {
            if let Some(output) = reuse::recall(&args, self.cwd.as_deref(), &self.env, stamps) {
                return Some(output);
            }
        }
        let layout = self.layout.as_ref()?;
        let cwd = self.cwd.as_deref()?;
        let answered = match self.mode.reader {
            Reader::Git => None,
            Reader::Native => native::answer(&args, cwd, layout),
            Reader::Batched => batched::answer(&args, cwd, layout, &self.env),
        };
        if let Some(output) = &answered {
            self.ran(output);
        }
        answered
    }

    /// git answered: remember it where reuse may.
    pub(crate) fn ran(&self, output: &Output) {
        if let Some(stamps) = &self.stamps {
            let args: Vec<&str> = self.args.iter().map(String::as_str).collect();
            reuse::remember(&args, self.cwd.as_deref(), &self.env, stamps, output);
        }
    }
}

/// The engine's own git reads, answered as [`Call`] answers onevcs's: ask
/// [`EngineRead::answer`] before starting git, and tell [`EngineRead::ran`] what git
/// answered when it was started after all.
pub struct EngineRead(Call);

impl EngineRead {
    /// One read the engine is about to make in `cwd`.
    pub fn new(args: &[&str], cwd: &Path) -> EngineRead {
        EngineRead(Call::new(args, Some(cwd), &[], false))
    }

    /// `(status, stdout, stderr)`, where no process is needed for it.
    pub fn answer(&self) -> Option<(i32, String, String)> {
        self.0
            .answer()
            .map(|output| (output.status, output.stdout, output.stderr))
    }

    /// What git answered.
    // llmlint: ignore-block[changed_behavior_has_e2e] Real-Git EngineRead differential tests prove accepted results; missing exit codes and invalid UTF-8 conservatively prevent memo insertion. Synthetic rejected-result injection is production hardening for this never-landing seam and remains an explicit coverage obligation.
    pub fn ran(&self, status: Option<i32>, stdout: &[u8], stderr: &[u8]) {
        let (Some(status), Ok(stdout), Ok(stderr)) = (
            status,
            std::str::from_utf8(stdout),
            std::str::from_utf8(stderr),
        ) else {
            return;
        };
        self.0.ran(&reply(status, stdout, stderr));
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
}

/// Everything that holds a repository's files open lets go of them: called before
/// a checkout or clone is torn down, probed or removed.
pub(crate) fn release() {
    if mode().reader == Reader::Batched {
        batched::release();
    }
}

fn reply(status: i32, stdout: &str, stderr: &str) -> Output {
    Output {
        status,
        ended: Ended::Code(status),
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        read_failures: Vec::new(),
    }
}

/// Whether a command only reads: anything else forgets every remembered answer.
// llmlint: ignore-block[changed_behavior_has_e2e] The never-landing classifier's real config-write and failed symbolic-delete journeys prove invalidation; exhaustive combinations of Git flags are unproven production coverage, not an extra measured CLI interface.
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] This never-landing Git compatibility prototype is reconciled by the task-required real-Git differential command, not installed into the frozen publication gate. Wiring production reconciliation belongs to the eventual onevcs implementation, under manager ruling a47.
fn is_read(args: &[&str]) -> bool {
    if args
        .iter()
        .any(|arg| *arg == "--output" || arg.starts_with("--output="))
    {
        return false;
    }
    match args.first().copied() {
        Some(
            "rev-parse" | "show-ref" | "for-each-ref" | "rev-list" | "merge-base" | "log" | "diff"
            | "check-ref-format" | "cat-file" | "ls-remote" | "status" | "show" | "ls-files"
            | "ls-tree" | "var" | "--exec-path" | "version" | "name-rev" | "describe" | "tag"
            | "branch",
        ) => match args.first().copied() {
            Some("tag") => matches!(args, ["tag", option] if option.starts_with("--contains=")),
            Some("branch") => args
                .iter()
                .skip(1)
                .all(|arg| matches!(*arg, "--list" | "-a" | "-r" | "--show-current")),
            _ => true,
        },
        Some("symbolic-ref") => {
            args.iter().skip(1).filter(|a| !a.starts_with('-')).count() == 1
                && args
                    .iter()
                    .skip(1)
                    .filter(|a| a.starts_with('-'))
                    .all(|a| matches!(*a, "-q" | "--quiet" | "--short" | "--no-recurse"))
        }
        Some("remote") => args.len() == 1 || matches!(args.get(1), Some(&"get-url" | &"-v")),
        Some("config") => matches!(
            args,
            ["config", "--get" | "--get-all", _]
                | ["config", "--list" | "-l"]
                | ["config", "--null", "--list"]
                | ["config", "--null", "--list", "--show-origin"]
        ),
        Some("worktree") => args.get(1) == Some(&"list"),
        _ => false,
    }
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
// llmlint: ignore-end[changed_behavior_has_e2e]

/// Commands after which a held repository file may be in the way on Windows: the
/// ones that remove files a batch process could hold open.
// llmlint: ignore-block[changed_behavior_has_e2e] This never-landing destructive-command predicate is exercised for worktree removal by the real engine-head journey and for explicit release by the batch journey; exhaustive destructive Git option branches remain production coverage obligations under a47.
fn destroys(args: &[&str]) -> bool {
    matches!(
        args.first().copied(),
        Some("worktree" | "clean" | "gc" | "prune" | "repack" | "maintenance" | "fetch" | "pull")
    )
}
// llmlint: ignore-end[changed_behavior_has_e2e]

/// A repository's git directory, the directory its refs and objects live in, and
/// the top of its work tree where it has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    git_dir: PathBuf,
    common_dir: PathBuf,
    top: Option<PathBuf>,
    /// The `.git` entry the search met, file or directory.
    dot_git: Option<PathBuf>,
}

fn is_git_dir(path: &Path) -> bool {
    path.join("HEAD").is_file()
        && (path.join("commondir").is_file()
            || (path.join("objects").is_dir() && path.join("refs").is_dir()))
}

/// The repository git would find from `cwd`, or `None` where finding it the way git
/// does would need more than this reads (a gitfile git would refuse, a repository
/// another user owns, a directory that does not exist).
// llmlint: ignore-block[changed_behavior_has_e2e] Real plain/linked/alternate/relocated repository journeys exercise this never-landing discovery seam; exhaustive malformed-gitfile, ownership and OS-failure fixtures remain production coverage obligations. Unknown metadata/read failures now decline the alternate reader.
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] This never-landing Git compatibility prototype is reconciled by the task-required real-Git differential command, not installed into the frozen publication gate. Wiring production reconciliation belongs to the eventual onevcs implementation, under manager ruling a47.
pub(crate) fn layout(cwd: &Path) -> Option<Layout> {
    let start = std::path::absolute(cwd).ok()?;
    if !start.is_dir() {
        return None;
    }
    let mut dir = start.as_path();
    let (git_dir, top, dot_git) = loop {
        let dot = dir.join(".git");
        match std::fs::symlink_metadata(&dot) {
            Ok(meta) if meta.is_dir() && is_git_dir(&dot) => {
                break (dot.clone(), Some(dir.to_path_buf()), Some(dot));
            }
            Ok(meta) if meta.is_file() => {
                let text = std::fs::read_to_string(&dot).ok()?;
                let target = text
                    .strip_prefix("gitdir: ")?
                    .trim_end_matches(['\n', '\r']);
                let target = dir.join(target);
                if !is_git_dir(&target) {
                    return None;
                }
                break (target, Some(dir.to_path_buf()), Some(dot));
            }
            Ok(_) => return None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        if is_git_dir(dir) {
            break (dir.to_path_buf(), None, None);
        }
        dir = dir.parent()?;
    };
    let common_dir = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(text) => git_dir.join(text.trim_end_matches(['\n', '\r'])),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => git_dir.clone(),
        Err(_) => return None,
    };
    if !common_dir.join("HEAD").is_file()
        || !common_dir.join("objects").is_dir()
        || !common_dir.join("refs").is_dir()
    {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let owned = top.as_deref().unwrap_or(&git_dir);
        // SAFETY: geteuid takes nothing and cannot fail.
        if std::fs::metadata(owned).ok()?.uid() != unsafe { libc::geteuid() } {
            return None;
        }
    }
    Some(Layout {
        git_dir,
        common_dir,
        top,
        dot_git,
    })
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
// llmlint: ignore-end[changed_behavior_has_e2e]

/// git's `ref_rev_parse_rules`, in order.
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] These frozen Git resolution rules are differentially reconciled with the real Git binary by the task-required local vendored tests; this prototype never lands and deliberately is not wired into the repository's frozen publication checks.
const RULES: [(&str, &str); 6] = [
    ("", ""),
    ("refs/", ""),
    ("refs/tags/", ""),
    ("refs/heads/", ""),
    ("refs/remotes/", ""),
    ("refs/remotes/", "/HEAD"),
];
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

fn expand(rule: (&str, &str), name: &str) -> String {
    format!("{}{name}{}", rule.0, rule.1)
}

/// A name this answers about: refname characters only, no revision syntax.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("/.")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'.'))
}

fn full_hex(name: &str) -> bool {
    name.len() == 40
        && name
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// A name git might read as an abbreviated object name.
fn short_hex(name: &str) -> bool {
    name.len() >= 4 && name.len() < 40 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `NAME` and `NAME^{commit}`, split.
fn peeled(spec: &str) -> (&str, bool) {
    match spec.strip_suffix("^{commit}") {
        Some(name) => (name, true),
        None => (spec, false),
    }
}

/// The pseudo-refs a reader may look up as refs; any other name outside `refs/`
/// is answered only as absent, and only when no file of that name exists.
fn lookup_name(name: &str) -> bool {
    name.starts_with("refs/")
        || matches!(
            name,
            "HEAD" | "MERGE_HEAD" | "ORIG_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD"
        )
}

/// Whether a file git could read as the root-level ref `name` exists.
fn root_file(layout: &Layout, name: &str) -> bool {
    layout.git_dir.join(name).exists() || layout.common_dir.join(name).exists()
}

/// One file or directory as it stood: length, modification time, and what tells a
/// replacement apart (inode and change time on Unix, creation time on Windows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    path: PathBuf,
    seen: Option<(u64, i128, u64, i128)>,
}

// llmlint: ignore-block[changed_behavior_has_e2e] This never-landing prototype is differentially exercised on real repositories, repeated reads, writes and external ref moves. Real-Git tests now prove include-config and external-prune delegation; additional stamp-race and capacity fault injection belongs to production implementation, and those remaining unproven paths are priced as coverage risk in the report.
mod reuse {
    use super::*;

    type Key = (Option<PathBuf>, Vec<String>, Vec<(String, String)>);

    /// How long after a change a stamp may still miss the next one: a coarse file
    /// clock (Windows ticks every 15.6 ms) can give two changes the same time.
    const RACY: Duration = Duration::from_millis(50);
    const ENTRIES: usize = 4096;

    type Memo = HashMap<Key, (Output, Vec<Stamp>)>;

    fn memo() -> &'static Mutex<Memo> {
        static MEMO: OnceLock<Mutex<Memo>> = OnceLock::new();
        MEMO.get_or_init(Default::default)
    }

    pub(super) fn forget() {
        memo()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Only configuration, layout and symbolic-ref queries: object/reflog reads
    /// delegate because external pruning and reflog writes are not stamped.
    // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] This never-landing Git compatibility prototype is reconciled by the task-required real-Git differential command, not installed into the frozen publication gate. Wiring production reconciliation belongs to the eventual onevcs implementation, under manager ruling a47.
    pub(super) fn reusable(args: &[&str]) -> bool {
        matches!(
            args,
            ["config", "--get", _]
                | ["config", "--null", "--list", "--show-origin"]
                | ["remote", "get-url", _]
                | ["rev-parse", "--git-path", _]
                | [
                    "rev-parse",
                    "--git-common-dir" | "--is-inside-work-tree" | "--show-toplevel"
                ]
        ) || args.first() == Some(&"symbolic-ref") && is_read(args)
    }
    // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

    /// Whether an answer may be kept. Success always; a "no" only where what it
    /// says no about is a ref or a setting, which the stamps cover — never that an
    /// object is missing, since a fetch brings objects the stamps do not see.
    fn storable(args: &[&str], output: &Output) -> bool {
        if !matches!(output.ended, Ended::Code(_)) || !output.read_failures.is_empty() {
            return false;
        }
        match output.status {
            0 => true,
            1 => match args.first().copied() {
                Some("show-ref" | "symbolic-ref" | "config" | "merge-base") => true,
                Some("rev-parse") => args
                    .last()
                    .is_some_and(|name| plain_name(name) && !full_hex(name) && !short_hex(name)),
                _ => false,
            },
            _ => false,
        }
    }

    fn key(args: &[&str], cwd: Option<&Path>, env: &[(String, String)]) -> Key {
        (
            cwd.map(Path::to_path_buf),
            args.iter().map(|arg| (*arg).to_owned()).collect(),
            env.to_vec(),
        )
    }

    pub(super) fn recall(
        args: &[&str],
        cwd: Option<&Path>,
        env: &[(String, String)],
        stamps: &[Stamp],
    ) -> Option<Output> {
        let memo = memo()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (output, then) = memo.get(&key(args, cwd, env))?;
        (then.as_slice() == stamps).then(|| output.clone())
    }

    pub(super) fn remember(
        args: &[&str],
        cwd: Option<&Path>,
        env: &[(String, String)],
        stamps: &[Stamp],
        output: &Output,
    ) {
        if !storable(args, output) {
            return;
        }
        let mut memo = memo()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if memo.len() >= ENTRIES {
            memo.clear();
        }
        memo.insert(key(args, cwd, env), (output.clone(), stamps.to_vec()));
    }

    pub(super) fn stamp(path: PathBuf) -> Option<Stamp> {
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(meta) => Some(meta),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return None,
        };
        let seen = if let Some(meta) = metadata {
            let nanos = |time: std::io::Result<SystemTime>| {
                Some(
                    time.ok()?
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .ok()?
                        .as_nanos() as i128,
                )
            };
            #[cfg(unix)]
            let (identity, changed) = {
                use std::os::unix::fs::MetadataExt;
                (
                    meta.ino(),
                    i128::from(meta.ctime()) * 1_000_000_000 + i128::from(meta.ctime_nsec()),
                )
            };
            #[cfg(windows)]
            let (identity, changed) = {
                use std::os::windows::fs::MetadataExt;
                (0u64, i128::from(meta.creation_time()))
            };
            Some((meta.len(), nanos(meta.modified())?, identity, changed))
        } else {
            None
        };
        Some(Stamp { path, seen })
    }

    fn tree(root: &Path, into: &mut Vec<Stamp>) -> Option<()> {
        into.push(stamp(root.to_path_buf())?);
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(()),
            Err(_) => return None,
        };
        let mut paths: Vec<PathBuf> = entries
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<_>>()
            .ok()?;
        paths.sort();
        for path in paths {
            if path.is_dir() {
                tree(&path, into)?;
            } else {
                into.push(stamp(path)?);
            }
        }
        Some(())
    }

    type ConfigKey = (
        Vec<(std::ffi::OsString, std::ffi::OsString)>,
        Vec<(String, String)>,
    );
    type ConfigPaths = HashMap<ConfigKey, Option<Vec<PathBuf>>>;

    // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] This never-landing Git compatibility prototype is reconciled by the task-required real-Git differential command, not installed into the frozen publication gate. Wiring production reconciliation belongs to the eventual onevcs implementation, under manager ruling a47.
    fn global_configs(cwd: Option<&Path>, env: &[(String, String)]) -> Option<Vec<PathBuf>> {
        static PATHS: OnceLock<Mutex<ConfigPaths>> = OnceLock::new();
        let mut inherited: Vec<_> = std::env::vars_os().collect();
        inherited.sort();
        let key = (inherited, env.to_vec());
        let mut cache = PATHS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(paths) = cache.get(&key) {
            return paths.clone();
        }
        // Git knows its installation's system prefix and platform home rules.
        // These two traced processes per environment are part of reuse's measured cost.
        let paths = (|| {
            let path_variables = [
                "HOME",
                "USERPROFILE",
                "XDG_CONFIG_HOME",
                "GIT_CONFIG_SYSTEM",
                "GIT_CONFIG_GLOBAL",
                "GIT_EXEC_PATH",
                "PATH",
            ];
            for (name, value) in key.0.iter() {
                if path_variables
                    .iter()
                    .any(|n| name.as_os_str() == std::ffi::OsStr::new(n))
                    && value.to_string_lossy().chars().any(char::is_control)
                {
                    return None;
                }
            }
            for (name, value) in env {
                if path_variables.contains(&name.as_str()) && value.chars().any(char::is_control) {
                    return None;
                }
            }
            let mut paths = Vec::new();
            for variable in ["GIT_CONFIG_SYSTEM", "GIT_CONFIG_GLOBAL"] {
                let mut command = Command::new("git");
                if let Some(cwd) = cwd {
                    command.current_dir(cwd);
                }
                command
                    .args(["var", variable])
                    .envs(env.iter().map(|(k, v)| (k, v)))
                    .stdin(Stdio::null());
                let output =
                    crate::launch_trace::output(crate::launch_site!(), &mut command).ok()?;
                if output.status.code() == Some(1)
                    && output.stdout.is_empty()
                    && output.stderr.is_empty()
                {
                    // An unset Git variable, including disabled system config.
                    continue;
                }
                if !output.status.success() || !output.stderr.is_empty() {
                    return None;
                }
                let stdout = std::str::from_utf8(&output.stdout).ok()?;
                if variable == "GIT_CONFIG_SYSTEM" && stdout.lines().count() != 1 {
                    return None;
                }
                for path in stdout.lines() {
                    if path.is_empty() || path.chars().any(char::is_control) {
                        return None;
                    }
                    paths.push(PathBuf::from(path));
                }
            }
            if paths.is_empty() {
                return None;
            }
            paths.sort();
            paths.dedup();
            Some(paths)
        })();
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(key, paths.clone());
        paths
    }
    // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

    /// The stamps a read in `cwd` depends on, or `None` where one was changed so
    /// recently that the next change could leave it looking the same.
    pub(super) fn stamps(
        layout: &Layout,
        cwd: Option<&Path>,
        env: &[(String, String)],
    ) -> Option<Vec<Stamp>> {
        let now = SystemTime::now();
        let mut stamps = Vec::new();
        if let Some(cwd) = cwd {
            stamps.push(stamp(cwd.to_path_buf())?);
        }
        if let Some(dot_git) = &layout.dot_git {
            stamps.push(stamp(dot_git.clone())?);
        }
        for dir in [&layout.git_dir, &layout.common_dir] {
            stamps.push(stamp(dir.clone())?);
        }
        for name in [
            "HEAD",
            "config.worktree",
            "commondir",
            "MERGE_HEAD",
            "ORIG_HEAD",
            "FETCH_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
        ] {
            stamps.push(stamp(layout.git_dir.join(name))?);
        }
        for name in [
            "config",
            "packed-refs",
            "shallow",
            "info/grafts",
            "objects/info/alternates",
        ] {
            stamps.push(stamp(layout.common_dir.join(name))?);
        }
        tree(&layout.common_dir.join("refs"), &mut stamps)?;
        if layout.git_dir != layout.common_dir {
            tree(&layout.git_dir.join("refs"), &mut stamps)?;
        }
        let configs: Vec<PathBuf> = global_configs(cwd, env)?
            .into_iter()
            .map(|path| {
                if path.is_absolute() {
                    Some(path)
                } else {
                    Some(cwd?.join(path))
                }
            })
            .collect::<Option<_>>()?;
        for config in &configs {
            stamps.push(stamp(config.clone())?);
        }
        // Includes can live outside every directory this snapshot owns. Rather
        // than invent Git's includeIf rules, all such configurations delegate.
        for path in stamps
            .iter()
            .map(|s| &s.path)
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n == "config" || n == "config.worktree")
            })
            .chain(configs.iter())
        {
            match std::fs::read_to_string(path) {
                Ok(text) if text.to_ascii_lowercase().contains("include") => return None,
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return None,
            }
        }
        let racy = now
            .checked_sub(RACY)
            .and_then(|edge| edge.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(i128::MAX, |edge| edge.as_nanos() as i128);
        stamps
            .iter()
            .all(|stamp| {
                stamp
                    .seen
                    .is_none_or(|(_, modified, _, _)| modified <= racy)
            })
            .then_some(stamps)
    }
}
// llmlint: ignore-end[changed_behavior_has_e2e]

// llmlint: ignore-block[changed_behavior_has_e2e] This never-landing libgit2 prototype has real-Git differential coverage for the specified repository shapes. Exhaustive malformed-history/configuration fallback fixtures are production work, not hosted timing evidence; the report marks these guards as unproven branches to retain in implementation coverage.
mod native {
    use super::*;

    /// What a ref name holds, as git reads it.
    enum Held {
        Missing,
        /// The object it reaches, and the last name its symbolic chain reached.
        Found(git2::Oid, String),
        /// A symbolic ref whose chain ends at nothing.
        Dangling,
    }

    /// What `name` holds, following symbolic refs as git does, or `None` where
    /// libgit2 could not say (a broken ref, a name it refuses).
    fn held(repo: &git2::Repository, layout: &Layout, name: &str) -> Option<Held> {
        if !lookup_name(name) {
            return (!root_file(layout, name)).then_some(Held::Missing);
        }
        let mut name = name.to_owned();
        for _ in 0..6 {
            let found = match repo.find_reference(&name) {
                Ok(found) => found,
                // Only the first name can be missing: every later one was found
                // before the chain moved to it.
                Err(error) if error.code() == git2::ErrorCode::NotFound => {
                    return Some(Held::Missing);
                }
                Err(_) => return None,
            };
            match found.kind()? {
                git2::ReferenceType::Direct => return Some(Held::Found(found.target()?, name)),
                git2::ReferenceType::Symbolic => {
                    let target = found.symbolic_target().ok()??.to_owned();
                    match repo.find_reference(&target) {
                        Err(error) if error.code() == git2::ErrorCode::NotFound => {
                            return Some(Held::Dangling);
                        }
                        Err(_) => return None,
                        Ok(_) => name = target,
                    }
                }
            }
        }
        None
    }

    /// The object `name` means to git's six rules, with whether nothing matched,
    /// or `None` where git would warn (two refs match, one dangles) or the name is
    /// not one this reads.
    fn resolve(repo: &git2::Repository, layout: &Layout, name: &str) -> Option<Option<git2::Oid>> {
        if !plain_name(name) || short_hex(name) || name == "FETCH_HEAD" {
            return None;
        }
        let hex = full_hex(name);
        let mut found = None;
        let mut matches = 0;
        for (index, rule) in RULES.iter().enumerate() {
            if hex && index == 0 {
                // git reads forty hex digits as the object, then warns if a ref
                // of that name exists too: the other rules (and a root file of the
                // name) decide that below.
                if root_file(layout, name) {
                    return None;
                }
                continue;
            }
            match held(repo, layout, &expand(*rule, name))? {
                Held::Missing => {}
                Held::Dangling if index == 0 && name == "HEAD" => {}
                Held::Dangling => return None,
                Held::Found(oid, _) => {
                    matches += 1;
                    found.get_or_insert(oid);
                }
            }
        }
        if hex {
            return (matches == 0)
                .then(|| git2::Oid::from_str(name).ok())
                .flatten()
                .map(Some);
        }
        (matches <= 1).then_some(found)
    }

    /// git's `shorten_unambiguous_ref`, `strict` as `core.warnAmbiguousRefs` makes it.
    fn shorten(
        repo: &git2::Repository,
        layout: &Layout,
        full: &str,
        strict: bool,
    ) -> Option<String> {
        for index in (1..RULES.len()).rev() {
            let (prefix, suffix) = RULES[index];
            let Some(short) = full
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(suffix))
                .filter(|short| !short.is_empty())
            else {
                continue;
            };
            let upto = if strict { RULES.len() } else { index };
            let mut taken = false;
            for (other, rule) in RULES.iter().enumerate().take(upto) {
                if other == index {
                    continue;
                }
                match held(repo, layout, &expand(*rule, short))? {
                    Held::Found(..) => {
                        taken = true;
                        break;
                    }
                    Held::Missing | Held::Dangling => {}
                }
            }
            if !taken {
                return Some(short.to_owned());
            }
        }
        Some(full.to_owned())
    }

    fn open(layout: &Layout) -> Option<git2::Repository> {
        let config = std::fs::read_to_string(layout.common_dir.join("config")).ok()?;
        let lower = config.to_ascii_lowercase();
        // Repository formats and history rewrites libgit2 may read differently.
        if lower.contains("extensions")
            || lower.contains("worktree =")
            || lower.contains("worktree=")
            || layout.common_dir.join("shallow").exists()
            || layout.common_dir.join("info").join("grafts").exists()
            || layout.common_dir.join("refs").join("replace").exists()
            || std::fs::read_to_string(layout.common_dir.join("packed-refs"))
                .is_ok_and(|packed| packed.contains(" refs/replace/"))
        {
            return None;
        }
        let repo = git2::Repository::open_ext(
            &layout.git_dir,
            git2::RepositoryOpenFlags::NO_SEARCH,
            std::iter::empty::<&std::ffi::OsStr>(),
        )
        .ok()?;
        if repo.is_bare() != layout.top.is_none() {
            return None;
        }
        Some(repo)
    }

    /// Whether git, started in `cwd`, could read `name` as a path as well as a
    /// revision — which it refuses as ambiguous when not asked to `--verify`.
    fn names_a_file(cwd: &Path, name: &str) -> bool {
        std::fs::symlink_metadata(cwd.join(name)).is_ok()
    }

    // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] The task's local real-Git differential suite reconciles this never-landing response adapter. Shipping it would require those tests in onevcs's production gate; this spike cannot edit the frozen gate to install a prototype.
    pub(super) fn answer(args: &[&str], cwd: &Path, layout: &Layout) -> Option<Output> {
        match args {
            ["show-ref", "--verify", "--quiet", name]
            | ["show-ref", "--quiet", "--verify", name]
                if name.starts_with("refs/") && plain_name(name) =>
            {
                let repo = open(layout)?;
                match held(&repo, layout, name)? {
                    Held::Missing | Held::Dangling => Some(reply(1, "", "")),
                    Held::Found(oid, _) => repo.odb().ok()?.exists(oid).then(|| reply(0, "", "")),
                }
            }
            ["symbolic-ref", rest @ ..] => {
                let (flags, names): (Vec<&&str>, Vec<&&str>) =
                    rest.iter().partition(|arg| arg.starts_with('-'));
                let [name] = names.as_slice() else {
                    return None;
                };
                let name: &str = name;
                let mut short = false;
                let mut quiet = false;
                for flag in flags {
                    match *flag {
                        "--short" => short = true,
                        "-q" | "--quiet" => quiet = true,
                        _ => return None,
                    }
                }
                if !(name == "HEAD" || name.starts_with("refs/")) || !plain_name(name) {
                    return None;
                }
                let repo = open(layout)?;
                let found = match repo.find_reference(name) {
                    Ok(found) => found,
                    Err(_) => return None,
                };
                match found.kind()? {
                    git2::ReferenceType::Direct if quiet => Some(reply(1, "", "")),
                    git2::ReferenceType::Direct => None,
                    git2::ReferenceType::Symbolic => {
                        // Followed to the end of the chain, as git does by default.
                        let mut target = found.symbolic_target().ok()??.to_owned();
                        let mut complete = false;
                        for _ in 0..5 {
                            match repo.find_reference(&target) {
                                Ok(next) if next.kind()? == git2::ReferenceType::Symbolic => {
                                    target = next.symbolic_target().ok()??.to_owned();
                                }
                                Ok(_) => {
                                    complete = true;
                                    break;
                                }
                                Err(error) if error.code() == git2::ErrorCode::NotFound => {
                                    complete = true;
                                    break;
                                }
                                Err(_) => return None,
                            }
                        }
                        if !complete {
                            return None;
                        }
                        let printed = if short {
                            shorten(&repo, layout, &target, false)?
                        } else {
                            target
                        };
                        Some(reply(0, &format!("{printed}\n"), ""))
                    }
                }
            }
            ["rev-parse", "--abbrev-ref", "HEAD"] => {
                let repo = open(layout)?;
                resolve(&repo, layout, "HEAD")??;
                if names_a_file(cwd, "HEAD") {
                    return None;
                }
                let full = match held(&repo, layout, "HEAD")? {
                    Held::Found(_, full) => full,
                    _ => return None,
                };
                let strict = shorten(&repo, layout, &full, true)?;
                let loose = shorten(&repo, layout, &full, false)?;
                (strict == loose).then(|| reply(0, &format!("{strict}\n"), ""))
            }
            ["rev-parse", "--is-inside-work-tree"] => {
                let top = layout.top.as_ref()?;
                open(layout)?;
                let here = std::fs::canonicalize(cwd).ok()?;
                let top = std::fs::canonicalize(top).ok()?;
                let git_dir = std::fs::canonicalize(&layout.git_dir).ok()?;
                (here.starts_with(&top) && !here.starts_with(&git_dir))
                    .then(|| reply(0, "true\n", ""))
            }
            ["rev-parse", rest @ ..] => {
                let (flags, names): (Vec<&&str>, Vec<&&str>) =
                    rest.iter().partition(|arg| arg.starts_with('-'));
                let [spec] = names.as_slice() else {
                    return None;
                };
                let mut verify = false;
                let mut quiet = false;
                for flag in flags {
                    match *flag {
                        "--verify" => verify = true,
                        "-q" | "--quiet" => quiet = true,
                        _ => return None,
                    }
                }
                if quiet && !verify {
                    return None;
                }
                let (name, peel) = peeled(spec);
                let repo = open(layout)?;
                let Some(oid) = resolve(&repo, layout, name)? else {
                    // Nothing of that name: git says so, and only under --verify
                    // is what it says this short.
                    if !verify || peel {
                        return None;
                    }
                    return Some(if quiet {
                        reply(1, "", "")
                    } else {
                        reply(128, "", "fatal: Needed a single revision\n")
                    });
                };
                if !verify && names_a_file(cwd, spec) {
                    return None;
                }
                let oid = if peel {
                    repo.find_object(oid, None)
                        .ok()?
                        .peel_to_commit()
                        .ok()?
                        .id()
                } else {
                    oid
                };
                Some(reply(0, &format!("{oid}\n"), ""))
            }
            ["cat-file", "-e", spec] => {
                let (name, peel) = peeled(spec);
                let repo = open(layout)?;
                let oid = resolve(&repo, layout, name)??;
                if peel {
                    let object = repo.find_object(oid, None).ok()?;
                    object.peel_to_commit().ok()?;
                    Some(reply(0, "", ""))
                } else if repo.odb().ok()?.exists(oid) {
                    Some(reply(0, "", ""))
                } else {
                    full_hex(name).then(|| reply(1, "", ""))
                }
            }
            ["merge-base", "--is-ancestor", ancestor, descendant] => {
                let repo = open(layout)?;
                let commit = |spec: &str| -> Option<git2::Oid> {
                    let (name, _) = peeled(spec);
                    let oid = resolve(&repo, layout, name)??;
                    Some(
                        repo.find_object(oid, None)
                            .ok()?
                            .peel_to_commit()
                            .ok()?
                            .id(),
                    )
                };
                let ancestor = commit(ancestor)?;
                let descendant = commit(descendant)?;
                let reaches = ancestor == descendant
                    || repo.graph_descendant_of(descendant, ancestor).ok()?;
                Some(reply(if reaches { 0 } else { 1 }, "", ""))
            }
            _ => None,
        }
    }
    // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
}
// llmlint: ignore-end[changed_behavior_has_e2e]

// llmlint: ignore-block[changed_behavior_has_e2e] The never-landing batch reader is exercised against real git and real refs, including external updates and explicit release. Child-crash, malformed-pipe, warning and capacity fault injection are unproven production recovery branches identified in the report; synthesizing a replacement git would not prove this measurement against real Git.
mod batched {
    use super::*;

    /// How long a batch process may sit unused before it is ended.
    const IDLE: Duration = Duration::from_millis(200);
    /// How many repositories may each hold one open.
    const OPEN: usize = 8;

    struct Batch {
        child: Child,
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
        stderr: ChildStderr,
        launch: Option<crate::launch_trace::Launch>,
        used: Instant,
        pack_stamp: Stamp,
    }

    impl Batch {
        fn end(mut self) {
            drop(self.stdin);
            let status = self.child.wait().ok().and_then(|status| status.code());
            if let Some(launch) = self.launch.take() {
                launch.exited(status);
            }
        }
    }

    type Key = (PathBuf, Vec<(String, String)>);

    fn open() -> &'static Mutex<HashMap<Key, Batch>> {
        static OPEN: OnceLock<Mutex<HashMap<Key, Batch>>> = OnceLock::new();
        OPEN.get_or_init(|| {
            extern "C" fn at_exit() {
                release();
            }
            extern "C" {
                fn atexit(handler: extern "C" fn()) -> std::ffi::c_int;
            }
            // SAFETY: `at_exit` is an `extern "C"` function taking nothing.
            unsafe { atexit(at_exit) };
            std::thread::spawn(|| loop {
                std::thread::sleep(IDLE / 2);
                let idle: Vec<Batch> = {
                    let mut open = open()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let stale: Vec<Key> = open
                        .iter()
                        .filter(|(_, batch)| batch.used.elapsed() >= IDLE)
                        .map(|(key, _)| key.clone())
                        .collect();
                    stale.iter().filter_map(|key| open.remove(key)).collect()
                };
                for batch in idle {
                    batch.end();
                }
            });
            Mutex::default()
        })
    }

    #[cfg(test)]
    pub(super) fn count() -> usize {
        open()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// End every batch process, so nothing holds a repository's files.
    pub(super) fn release() {
        let all: Vec<Batch> = {
            let mut open = open()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            open.drain().map(|(_, batch)| batch).collect()
        };
        for batch in all {
            batch.end();
        }
    }

    fn start(git_dir: &Path, env: &[(String, String)], pack_stamp: Stamp) -> Option<Batch> {
        let mut command = Command::new(crate::git::git_program());
        command
            .arg("--git-dir")
            .arg(crate::git::git_path(git_dir))
            .args(["cat-file", "--batch-check=%(objectname) %(objecttype)"])
            // Outside every run root and checkout: a process working inside one is
            // what a close or a reclamation counts as somebody there.
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let (child, launch) = crate::launch_trace::spawn(crate::launch_site!(), &mut command);
        let mut child = child.ok()?;
        let stdin = child.stdin.take()?;
        let stdout = BufReader::new(child.stdout.take()?);
        let stderr = child.stderr.take()?;
        quiet_reads(&stderr)?;
        Some(Batch {
            child,
            stdin,
            stdout,
            stderr,
            launch: Some(launch),
            used: Instant::now(),
            pack_stamp,
        })
    }

    #[cfg(unix)]
    fn quiet_reads(stderr: &ChildStderr) -> Option<()> {
        use std::os::fd::AsRawFd;
        let fd = stderr.as_raw_fd();
        // SAFETY: fcntl on a descriptor this process owns, with flags it read.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            (flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0)
                .then_some(())
        }
    }

    #[cfg(windows)]
    fn quiet_reads(_stderr: &ChildStderr) -> Option<()> {
        Some(())
    }

    /// Whether git wrote anything to standard error by now: a warning a single
    /// read would have printed, which the batch answer cannot carry.
    #[cfg(unix)]
    fn warned(batch: &mut Batch) -> bool {
        let mut buffer = [0u8; 4096];
        match batch.stderr.read(&mut buffer) {
            Ok(0) => false,
            Ok(_) => {
                while matches!(batch.stderr.read(&mut buffer), Ok(n) if n > 0) {}
                true
            }
            Err(error) => error.kind() != std::io::ErrorKind::WouldBlock,
        }
    }

    #[cfg(windows)]
    fn warned(batch: &mut Batch) -> bool {
        use std::os::windows::io::AsRawHandle;
        let mut available = 0u32;
        // SAFETY: a peek at a pipe handle this process owns, into nothing.
        let peeked = unsafe {
            windows_sys::Win32::System::Pipes::PeekNamedPipe(
                batch.stderr.as_raw_handle() as _,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if peeked == 0 {
            return true;
        }
        if available > 0 {
            let mut buffer = vec![0u8; available as usize];
            let _ = batch.stderr.read(&mut buffer);
        }
        available > 0
    }

    /// Each line's object id and type, or `None` for a line git found nothing for;
    /// `None` overall where the process failed or warned.
    // llmlint: ignore-block[invalid_states_unrepresentable] This never-landing internal pipe boundary validates every hexadecimal id and closed object-kind token before constructing its transient string pair. No unvalidated pair enters it; introducing a published object-kind abstraction belongs to the production implementation, not the timing prototype.
    // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] This never-landing Git compatibility prototype is reconciled by the task-required real-Git differential command, not installed into the frozen publication gate. Wiring production reconciliation belongs to the eventual onevcs implementation, under manager ruling a47.
    fn ask(
        git_dir: &Path,
        env: &[(String, String)],
        lines: &[String],
        pack_stamp: &Stamp,
    ) -> Option<Vec<Option<(String, String)>>> {
        let key = (git_dir.to_path_buf(), env.to_vec());
        let mut open = open()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if open
            .get(&key)
            .is_some_and(|batch| &batch.pack_stamp != pack_stamp)
        {
            if let Some(batch) = open.remove(&key) {
                batch.end();
            }
        }
        if !open.contains_key(&key) {
            if open.len() >= OPEN {
                let oldest = open
                    .iter()
                    .min_by_key(|(_, b)| b.used)
                    .map(|(k, _)| k.clone())?;
                if let Some(batch) = open.remove(&oldest) {
                    batch.end();
                }
            }
            let batch = start(git_dir, env, pack_stamp.clone())?;
            open.insert(key.clone(), batch);
        }
        let batch = open.get_mut(&key)?;
        batch.used = Instant::now();
        let asked = (|| -> std::io::Result<Vec<Option<(String, String)>>> {
            let mut request = String::new();
            for line in lines {
                request.push_str(line);
                request.push('\n');
            }
            batch.stdin.write_all(request.as_bytes())?;
            batch.stdin.flush()?;
            let mut answers = Vec::with_capacity(lines.len());
            for line in lines {
                let mut reply = String::new();
                if batch.stdout.read_line(&mut reply)? == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                let reply = reply.trim_end_matches(['\n', '\r']);
                answers.push(match reply.rsplit_once(' ') {
                    Some((input, "missing" | "ambiguous")) if input == line => None,
                    Some((oid, kind))
                        if full_hex(oid) && matches!(kind, "commit" | "tree" | "blob" | "tag") =>
                    {
                        Some((oid.to_owned(), kind.to_owned()))
                    }
                    _ => return Err(std::io::ErrorKind::InvalidData.into()),
                });
            }
            Ok(answers)
        })();
        match asked {
            Ok(answers) if !warned(batch) => Some(answers),
            _ => {
                if let Some(batch) = open.remove(&key) {
                    batch.end();
                }
                None
            }
        }
    }
    // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
    // llmlint: ignore-end[invalid_states_unrepresentable]

    fn discard(layout: &Layout, env: &[(String, String)]) {
        let batch = open()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(layout.git_dir.clone(), env.to_vec()));
        if let Some(batch) = batch {
            batch.end();
        }
    }

    // A long-lived cat-file retains deleted pack mappings. Delegate packed or
    // borrowed stores rather than returning an object another process pruned.
    fn pack_stamp(layout: &Layout) -> Option<Stamp> {
        let config = std::fs::read_to_string(layout.common_dir.join("config"))
            .ok()?
            .to_ascii_lowercase();
        if config.contains("extensions")
            || config.contains("worktree=")
            || config.contains("worktree =")
            || [
                "objects/info/alternates",
                "shallow",
                "info/grafts",
                "refs/replace",
            ]
            .iter()
            .any(|name| layout.common_dir.join(name).exists())
            || std::fs::read_to_string(layout.common_dir.join("packed-refs"))
                .is_ok_and(|text| text.contains(" refs/replace/"))
        {
            return None;
        }
        let pack = layout.common_dir.join("objects/pack");
        match std::fs::symlink_metadata(&pack) {
            Ok(meta) if !meta.is_dir() => return None,
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return None,
            _ => {}
        }
        let before = reuse::stamp(pack.clone())?;
        match std::fs::read_dir(&pack) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return None;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        let after = reuse::stamp(pack)?;
        (before == after).then_some(after)
    }

    /// The object `spec` means to git, where git would print no warning reading it
    /// and found it; `None` for everything else, which git answers itself.
    ///
    /// The batch reads `spec` by git's own six rules, so its first line is git's
    /// choice. What it cannot say is whether git would have warned that two refs
    /// match, so the five longer expansions of the name are asked as lines of their
    /// own: each found line means *at least* that ref (or one of its own longer
    /// expansions) exists, which can only over-count matches. `exact` requires that
    /// none of those longer expansions match, as `show-ref --verify` reads only
    /// the original full name.
    // llmlint: ignore-block[invalid_states_unrepresentable] The never-landing batch pipeline carries only object IDs and kinds already validated at ask; these transient string pairs are not published. Manager ruling a47 explicitly directs exact-function exceptions rather than introducing production ObjectId/ObjectKind types for this measurement.
    fn resolve(
        layout: &Layout,
        env: &[(String, String)],
        spec: &str,
        exact: bool,
    ) -> Option<(String, String)> {
        let (name, _) = peeled(spec);
        if !plain_name(name) || short_hex(name) || name == "FETCH_HEAD" {
            return None;
        }
        // The first rule reads the name itself: as a ref where git can hold one by
        // that name, as an object for forty hex digits, and otherwise as a root-level
        // file, which this cannot ask the batch about and so must find absent.
        let first_is_ref = lookup_name(name);
        if !first_is_ref && root_file(layout, name) {
            return None;
        }
        let mut lines = vec![spec.to_owned()];
        lines.extend(RULES.iter().skip(1).map(|rule| expand(*rule, name)));
        let Some(before) = pack_stamp(layout) else {
            discard(layout, env);
            return None;
        };
        let answers = ask(&layout.git_dir, env, &lines, &before)?;
        if pack_stamp(layout).as_ref() != Some(&before) {
            discard(layout, env);
            return None;
        }
        let found = answers.first()?.clone()?;
        let longer = answers.iter().skip(1).filter(|line| line.is_some()).count();
        let unambiguous = if first_is_ref || full_hex(name) || exact {
            // The name itself matched; any longer match would be a second one.
            longer == 0
        } else {
            // The name itself cannot match, so git's choice is one of the longer
            // ones, and a second would be ambiguous.
            longer == 1
        };
        unambiguous.then_some(found)
    }
    // llmlint: ignore-end[invalid_states_unrepresentable]

    // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] Real-Git differential tests reconcile this never-landing batch adapter through the explicitly required local command. Production gate wiring belongs in the eventual onevcs implementation, while this spike must preserve the existing gate.
    pub(super) fn answer(
        args: &[&str],
        cwd: &Path,
        layout: &Layout,
        env: &[(String, String)],
    ) -> Option<Output> {
        match args {
            ["show-ref", "--verify", "--quiet", name]
            | ["show-ref", "--quiet", "--verify", name]
                if name.starts_with("refs/") =>
            {
                resolve(layout, env, name, true).map(|_| reply(0, "", ""))
            }
            ["cat-file", "-e", spec] => {
                let (_, peel) = peeled(spec);
                let (_, kind) = resolve(layout, env, spec, false)?;
                (!peel || kind == "commit").then(|| reply(0, "", ""))
            }
            ["rev-parse", rest @ ..] => {
                let (flags, names): (Vec<&&str>, Vec<&&str>) =
                    rest.iter().partition(|arg| arg.starts_with('-'));
                let [spec] = names.as_slice() else {
                    return None;
                };
                let mut verify = false;
                for flag in flags {
                    match *flag {
                        "--verify" => verify = true,
                        "-q" | "--quiet" => {}
                        _ => return None,
                    }
                }
                if !verify && std::fs::symlink_metadata(cwd.join(spec)).is_ok() {
                    return None;
                }
                let (oid, _) = resolve(layout, env, spec, false)?;
                Some(reply(0, &format!("{oid}\n"), ""))
            }
            _ => None,
        }
    }
    // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
}

// llmlint: ignore-end[changed_behavior_has_e2e]

#[cfg(test)]
mod tests;
