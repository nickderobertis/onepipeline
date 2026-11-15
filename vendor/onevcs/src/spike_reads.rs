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
//! a repository with `extensions`, and everything not listed above.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crate::git::{Ended, Output};

/// The mode a build answers with when `ONEVCS_SPIKE_GIT_READS` is unset: the one
/// line a demonstration commit edits.
const DEMONSTRATION: &str = "native";

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

/// The mode this process runs: `ONEVCS_SPIKE_GIT_READS` when it names one, else
/// [`DEMONSTRATION`].
// llmlint: ignore[changed_behavior_has_e2e] local-slice.sh runs real engine subprocesses in all six environment-selected modes; once-per-process selection and fallback are measurement setup, never a shipped configurable interface.
pub fn mode() -> Mode {
    #[cfg(test)]
    if let Some(mode) = TEST_MODE.with(std::cell::Cell::get) {
        return mode;
    }
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        std::env::var(MODE_ENV)
            .ok()
            .and_then(|word| Mode::parse(word.trim()))
            .or_else(|| Mode::parse(DEMONSTRATION))
            .unwrap_or(Mode::STOCK)
    })
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
        if fed || !is_read(args) {
            reuse::forget();
            if destroys(args) {
                batched::release();
            }
            call.mode = Mode::STOCK;
            return call;
        }
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
fn is_read(args: &[&str]) -> bool {
    match args.first().copied() {
        Some(
            "rev-parse" | "show-ref" | "for-each-ref" | "rev-list" | "merge-base" | "log" | "diff"
            | "check-ref-format" | "cat-file" | "ls-remote" | "status" | "show" | "ls-files"
            | "ls-tree" | "var" | "--exec-path" | "version" | "merge-tree" | "name-rev"
            | "describe" | "tag" | "branch",
        ) => match args.first().copied() {
            Some("tag") => args.get(1).is_some_and(|arg| arg.starts_with("--contains")),
            Some("branch") => args
                .iter()
                .skip(1)
                .all(|arg| matches!(*arg, "--list" | "-a" | "-r" | "--show-current")),
            _ => true,
        },
        Some("symbolic-ref") => args.iter().skip(1).filter(|a| !a.starts_with('-')).count() == 1,
        Some("remote") => args.len() == 1 || matches!(args.get(1), Some(&"get-url" | &"-v")),
        Some("config") => {
            matches!(
                args.get(1),
                Some(&"--get" | &"--get-all" | &"--list" | &"-l")
            ) || args.get(1) == Some(&"--null") && args.get(2) == Some(&"--list")
        }
        Some("worktree") => args.get(1) == Some(&"list"),
        _ => false,
    }
}

/// Commands after which a held repository file may be in the way on Windows: the
/// ones that remove files a batch process could hold open.
fn destroys(args: &[&str]) -> bool {
    matches!(
        args.first().copied(),
        Some("worktree" | "clean" | "gc" | "prune" | "repack" | "maintenance" | "fetch" | "pull")
    )
}

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
            Err(_) => {}
        }
        if is_git_dir(dir) {
            break (dir.to_path_buf(), None, None);
        }
        dir = dir.parent()?;
    };
    let common_dir = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(text) => git_dir.join(text.trim_end_matches(['\n', '\r'])),
        Err(_) => git_dir.clone(),
    };
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

/// git's `ref_rev_parse_rules`, in order.
const RULES: [(&str, &str); 6] = [
    ("", ""),
    ("refs/", ""),
    ("refs/tags/", ""),
    ("refs/heads/", ""),
    ("refs/remotes/", ""),
    ("refs/remotes/", "/HEAD"),
];

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

// llmlint: ignore-block[changed_behavior_has_e2e] This never-landing prototype is differentially exercised on real repositories, repeated reads, writes and external ref moves. Additional fault injection for included-config changes, object removal, racy filesystem stamps and capacity eviction belongs to a production implementation; those unproven paths are explicitly priced as coverage risk in the spike report.
mod reuse {
    use super::*;

    type Key = (Option<PathBuf>, Vec<String>, Vec<(String, String)>);

    /// How long after a change a stamp may still miss the next one: a coarse file
    /// clock (Windows ticks every 15.6 ms) can give two changes the same time.
    const RACY: Duration = Duration::from_millis(50);
    const ENTRIES: usize = 4096;

    fn memo() -> &'static Mutex<HashMap<Key, (Output, Vec<Stamp>)>> {
        static MEMO: OnceLock<Mutex<HashMap<Key, (Output, Vec<Stamp>)>>> = OnceLock::new();
        MEMO.get_or_init(Default::default)
    }

    pub(super) fn forget() {
        memo()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// The reads whose answer depends on nothing the stamps miss: refs, `HEAD`,
    /// configuration, the repository's layout, and objects, which never change once
    /// written.
    pub(super) fn reusable(args: &[&str]) -> bool {
        match args.first().copied() {
            Some("--exec-path") => args.len() == 1,
            Some("config") => {
                args == ["config", "--null", "--list", "--show-origin"]
                    || (args.len() == 3 && args[1] == "--get")
            }
            Some("remote") => args.len() == 3 && args[1] == "get-url",
            Some("cat-file") => args.get(1) == Some(&"-e"),
            Some(
                "rev-parse" | "show-ref" | "for-each-ref" | "rev-list" | "merge-base" | "log"
                | "check-ref-format",
            ) => true,
            // Two revisions compared, never the work tree or the index.
            Some("diff") => {
                args.iter()
                    .skip(1)
                    .take_while(|arg| **arg != "--")
                    .filter(|arg| !arg.starts_with('-'))
                    .count()
                    == 2
                    && !args.contains(&"--cached")
                    && !args.iter().any(|arg| arg.starts_with("--no-index"))
            }
            Some("symbolic-ref") => {
                args.iter().skip(1).filter(|a| !a.starts_with('-')).count() == 1
            }
            _ => false,
        }
    }

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

    fn stamp(path: PathBuf) -> Stamp {
        let seen = std::fs::symlink_metadata(&path).ok().map(|meta| {
            let nanos = |time: std::io::Result<SystemTime>| {
                time.ok()
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |since| since.as_nanos() as i128)
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
            (meta.len(), nanos(meta.modified()), identity, changed)
        });
        Stamp { path, seen }
    }

    fn tree(root: &Path, into: &mut Vec<Stamp>) {
        into.push(stamp(root.to_path_buf()));
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                tree(&path, into);
            } else {
                into.push(stamp(path));
            }
        }
    }

    fn global_configs(env: &[(String, String)]) -> Vec<PathBuf> {
        let var = |key: &str| {
            env.iter()
                .rev()
                .find(|(k, _)| k == key)
                .map(|(_, v)| std::ffi::OsString::from(v))
                .or_else(|| std::env::var_os(key))
        };
        let mut paths = Vec::new();
        match var("GIT_CONFIG_GLOBAL") {
            Some(global) => paths.push(PathBuf::from(global)),
            None => {
                if let Some(home) = var("HOME") {
                    paths.push(PathBuf::from(&home).join(".gitconfig"));
                    paths.push(match var("XDG_CONFIG_HOME") {
                        Some(xdg) => PathBuf::from(xdg).join("git").join("config"),
                        None => PathBuf::from(home)
                            .join(".config")
                            .join("git")
                            .join("config"),
                    });
                }
            }
        }
        if var("GIT_CONFIG_NOSYSTEM").is_none() {
            match var("GIT_CONFIG_SYSTEM") {
                Some(system) => paths.push(PathBuf::from(system)),
                None if cfg!(unix) => paths.push(PathBuf::from("/etc/gitconfig")),
                None => {}
            }
        }
        paths
    }

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
            stamps.push(stamp(cwd.to_path_buf()));
        }
        if let Some(dot_git) = &layout.dot_git {
            stamps.push(stamp(dot_git.clone()));
        }
        for dir in [&layout.git_dir, &layout.common_dir] {
            stamps.push(stamp(dir.clone()));
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
            stamps.push(stamp(layout.git_dir.join(name)));
        }
        for name in [
            "config",
            "packed-refs",
            "shallow",
            "info/grafts",
            "objects/info/alternates",
        ] {
            stamps.push(stamp(layout.common_dir.join(name)));
        }
        tree(&layout.common_dir.join("refs"), &mut stamps);
        if layout.git_dir != layout.common_dir {
            tree(&layout.git_dir.join("refs"), &mut stamps);
        }
        for config in global_configs(env) {
            stamps.push(stamp(config));
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
                        for _ in 0..5 {
                            match repo.find_reference(&target) {
                                Ok(next) if next.kind()? == git2::ReferenceType::Symbolic => {
                                    target = next.symbolic_target().ok()??.to_owned();
                                }
                                Ok(_) => break,
                                Err(error) if error.code() == git2::ErrorCode::NotFound => break,
                                Err(_) => return None,
                            }
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

    fn start(git_dir: &Path, env: &[(String, String)]) -> Option<Batch> {
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
    fn ask(
        git_dir: &Path,
        env: &[(String, String)],
        lines: &[String],
    ) -> Option<Vec<Option<(String, String)>>> {
        let key = (git_dir.to_path_buf(), env.to_vec());
        let mut open = open()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            let batch = start(git_dir, env)?;
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
        let answers = ask(&layout.git_dir, env, &lines)?;
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
}

// llmlint: ignore-end[changed_behavior_has_e2e]

#[cfg(test)]
mod tests;
