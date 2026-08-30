//! The scaffolding every journey here shares.
//!
//! Each test gets its own runs root, its own launching session, its own `onevcs`
//! state root, and a scripted **double for `oneagentgraph`**. Be clear about what
//! that means: `onevcs` is **not** substituted. This crate calls that library
//! rather than spawning it, so there is no subprocess boundary to stand in at —
//! every journey below drives the real repository side, over a real bare-
//! repository origin on disk, and what a journey states instead is the world that
//! library reads: the repository's rules, the `pre-push` hook its merge path
//! verifies a publishing push with, and — at `onevcs`'s own `ONEVCS_GH` override
//! — what GitHub does with the change request it is handed.
//!
//! Nothing *inside* `onepipeline` is substituted either: it is driven as a
//! compiled subprocess.

// llmlint: ignore-file[e2e_not_mocked] one sibling is substituted at its subprocess
// boundary — `oneagentgraph`, so a journey can state a dispatch that fails, one held open,
// or a driver that dies instead of arranging one out of paid agent turns — and
// `dispatch.rs` runs the real binary through [`World::agentgraph_cmd`], substituting only
// the paid model turn. `onevcs` is not substituted anywhere: it is linked and called, and
// these journeys run it against a real git origin. The remaining stand-in is one layer
// past it — GitHub, at that library's own `ONEVCS_GH` seam, which decides only what the
// host does with a change request and leaves every git operation real. `tests/smoke/` runs
// the same publication against the real `gh` and is what holds that honest.

// A shared harness is used a piece at a time: every helper below is exercised by some
// journey, and none by all of them. Rust judges that per test binary, so without this the
// unused-code warning fires on whatever the current selection happens not to reach.
#![allow(
    dead_code,
    reason = "shared test scaffolding: each helper is used by some journey, and the \
              unused-code check cannot see across the ones that do not"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use onepipeline_testfakes::{segment, CLI_BIN_ENV, MEMBER_ENV, SCRIPT_DIR_ENV};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreResponse<T> {
    items: Vec<T>,
    next: Option<String>,
    plan: serde::de::IgnoredAny,
    errors: Vec<StoreFailure>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreFailure {
    source: String,
    error: StoreSourceError,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum StoreSourceError {
    Config { message: String },
    Auth { message: String },
    Refused { message: String },
    RateLimited { retry_after_seconds: Option<u64> },
    Unavailable { message: String },
    Malformed { message: String },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoreQualified<T> {
    id: String,
    item: T,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoreTask {
    id: String,
    title: String,
    content: Option<String>,
    status: StoreStatus,
    labels: Vec<StoreLabel>,
    project: Option<String>,
    url: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    #[serde(default)]
    repositories: Vec<String>,
    #[serde(default)]
    metadata: BTreeMap<String, Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoreProject {
    id: String,
    title: String,
    content: Option<String>,
    status: StoreStatus,
    labels: Vec<StoreLabel>,
    url: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    #[serde(default)]
    repositories: Vec<String>,
    #[serde(default)]
    metadata: BTreeMap<String, Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoreStatus {
    category: StoreStatusCategory,
    name: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum StoreStatusCategory {
    Backlog,
    Todo,
    InProgress,
    Done,
    Cancelled,
    Unknown,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoreLabel {
    id: String,
    name: String,
    color: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoreEdge {
    from: StoreEndpoint,
    to: StoreEndpoint,
    kind: StoreEdgeKind,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoreEndpoint {
    id: String,
    kind: StoreItemKind,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum StoreItemKind {
    Task,
    Project,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum StoreEdgeKind {
    Blocks,
    Related,
}

// The exit codes are the crate's own, not a second copy of them. A suite that
// restated the numbers would keep passing against a build that had changed one,
// which is exactly the drift these journeys exist to catch.

/// The exit code a refused or malformed command carries.
pub const REFUSED: i32 = onepipeline::error::EXIT_REFUSED;

/// The exit code for accepted-but-not-yet-reconciled edits, and for a graph
/// that settled unfinished.
pub const QUEUED: i32 = onepipeline::error::EXIT_QUEUED;

/// The exit code for a run nothing is driving.
pub const NOTHING_DRIVING: i32 = onepipeline::error::EXIT_NOTHING_DRIVING;

/// clap's exit code for a usage error.
pub const USAGE_ERROR: i32 = 2;

/// The variable that moves the startup handshake's backstop.
///
/// The suite's one copy, not the crate's own constant: the module that declares
/// it is private, and this crate publishes the contract's surface and nothing
/// else, so making it reachable from here would put an item on the public API
/// that `docs/contract.md` does not name.
///
/// A copy is only safe while something proves it is still the name the binary
/// reads, and that is what
/// `a_graph_that_neither_starts_nor_exits_fails_the_launch_rather_than_outlasting_it`
/// does: it sets this to a second and requires the launch to give up inside
/// [`OVERRIDE_TOOK_EFFECT`]. Renamed in the crate and not here, the override
/// would be inert, the launch would wait out the much longer default, and that
/// test would fail on the elapsed time rather than passing a little slower.
pub const STARTUP_TIMEOUT_ENV: &str = "ONEPIPELINE_STARTUP_TIMEOUT_SECONDS";

/// The variable that moves the deadline a cancelled dispatch is torn down at.
///
/// The suite's own copy of a name the crate declares, for the same reason
/// [`STARTUP_TIMEOUT_ENV`] is one: the module declaring it is private, and this
/// crate publishes the contract's surface and nothing else. What proves the copy
/// is still the name the binary reads is
/// `a_dispatch_that_ignores_the_ask_is_killed_at_the_deadline` — renamed in the
/// crate and not here, the override would be inert, the deadline would be the
/// five-minute default, and that journey would fail on its own clock.
pub const CANCEL_GRACE_ENV: &str = "ONEPIPELINE_CANCEL_GRACE_SECONDS";

/// The variable that moves the threshold a silent dispatch is reported quiet
/// past.
///
/// The suite's own copy for the same reason [`CANCEL_GRACE_ENV`] is one, and
/// proved live by the same kind of journey:
/// `a_worker_that_only_heartbeats_is_reported_quiet_rather_than_active` sets it
/// to seconds and waits for the report. Renamed in the crate and not here, the
/// override would be inert, the threshold would be the forty-minute default, and
/// that journey would time out waiting rather than passing a little slower.
pub const STALL_AFTER_ENV: &str = "ONEPIPELINE_STALL_AFTER_SECONDS";

/// The variable that says how long a scripted hold waits before it gives up.
///
/// The doubles' own, not this crate's: a hold is how a journey keeps a dispatch
/// open, and how long one may wait is the doubles' bound to enforce. Named here
/// because two things set it — every command this world runs, at a value above
/// its own patience, and the journey that proves an out-of-range one is refused.
pub const RENDEZVOUS_SECONDS_ENV: &str = "ONEPIPELINE_FAKE_RENDEZVOUS_SECONDS";

/// The variable that says how long the repository's own `pre-push` hook waits
/// for its rendezvous before it refuses the push.
///
/// The hook's own bound, and `hook.sh` says why it has one and why it is 300
/// seconds. Named here because one journey sets it —
/// [`a_held_push_nothing_releases_expires_rather_than_waiting_out_the_job`],
/// which cannot wait the default out — and every other journey runs on the
/// default, so a hold that outlives its journey is refused rather than left
/// waiting.
const HOOK_WAIT_SECONDS_ENV: &str = "ONEPIPELINE_FAKE_HOOK_WAIT_SECONDS";

/// How long a launch given a one-second backstop may take before the override
/// has to be presumed inert.
///
/// Well above a second of process startup on a loaded host, and well below the
/// crate's own default backstop, which is what a launch that never read
/// [`STARTUP_TIMEOUT_ENV`] would wait instead.
pub const OVERRIDE_TOOK_EFFECT: std::time::Duration = std::time::Duration::from_secs(15);

/// The graph schema version this world's ordinary configs declare.
///
/// The oldest schema a **consumer** of this crate can be written against, rather
/// than the oldest the runner parses: the launcher's task says what the run is
/// and nothing about who does what with it, so a dag-scope member that must
/// drive says so in its own `task` composed from `{task}` — and those six
/// characters stand for themselves under every schema below this one.
const CONSUMER_GRAPH_SCHEMA: u32 = oneagentgraph::config::FIRST_TASK_TOKEN_VERSION;

/// The dag-scope observer's own `task`, in this world's graph configs.
///
/// `{task}` is the run description the launcher composed — what the run is and
/// what it is for — and the rest is what *this* member does about it. The
/// shipped graph says the same thing in the `monitor` persona, which a world
/// with no persona files of its own cannot; either way it is the graph that
/// says it, because the launcher never does.
///
/// It drives nothing. There is no engine verb for it to run: `onepipeline
/// start` executes the plan itself, and this member watches.
pub const MONITOR_TASK: &str =
    "{task}\n\nObserve this run and surface what does not line up. Change nothing.";

/// The dag-scope member whose job is not the monitor's.
pub const REPORTING_MEMBER: &str = "reporter";

/// One model turn that really ran.
#[derive(Debug, Clone)]
pub struct Turn {
    /// The prose the turn was given.
    pub prompt: String,
    /// The directory it worked in.
    pub cwd: String,
    /// The member it was, from its own config's `[env]`. Empty for a turn whose
    /// config a journey wrote itself and did not stamp.
    pub member: String,
    /// The scratch directory the dispatch this turn belongs to was given, out of
    /// the turn process's own environment. Empty for a turn the engine started
    /// nothing for.
    pub scratch: String,
}

/// The prose [`REPORTING_MEMBER`] carries as its own `task`.
///
/// Composed the same way [`MONITOR_TASK`] is, and deliberately a different job:
/// the launcher hands one task to the whole graph, so what tells these two
/// members apart is the prose each carries around it.
pub const MEMBER_TASK: &str = "{task}\n\nReport on this run and change nothing about it.";

/// One directory, in the single spelling the binary under test will report.
///
/// A launch directory crosses a process boundary — `start` records where the
/// operator launched from, and every later `adopt` replays that value — so the
/// spelling this crate records is the one the *kernel* answers with, which is
/// what [`std::env::current_dir`] reads and what the record therefore carries.
/// That answer never names the route taken to a directory: on macOS `/var` is a
/// symlink to `/private/var`, so a temporary directory hands back one spelling
/// and a process changed into it reports the other.
///
/// Every path a journey compares against the binary's answer is therefore
/// resolved here first. A journey that compared [`std::env::temp_dir`]'s
/// spelling directly would be asserting that *this host's* temporary directory
/// is reached without a symlink — true on Linux, false on macOS, and nothing to
/// do with the crate under test.
fn resolved(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path)
        .unwrap_or_else(|error| panic!("{} cannot be resolved: {error}", path.display()));
    plain(&canonical)
}

/// A canonical path spelled the way every other API on the platform spells it.
///
/// [`std::fs::canonicalize`] answers on Windows with the *verbatim* form — the
/// `\\?\` prefix that turns path parsing off — and nothing else there spells a
/// directory that way: `GetCurrentDirectory` does not, so neither does the
/// launch record, and `SetCurrentDirectory` does not even accept it. Removing
/// the prefix is what leaves one directory with one spelling. No other platform
/// produces the prefix, where this is the identity.
fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{share}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(local) => PathBuf::from(local),
        None => path.to_path_buf(),
    }
}

/// One test's world: a scratch root with everything the binary reads.
pub struct World {
    /// The scratch root, removed when the test finishes.
    pub root: PathBuf,
    /// The runs root the binary reads and writes.
    pub runs: PathBuf,
    /// The directory the doubles are scripted from and record into.
    pub fakes: PathBuf,
    /// The directory a direct agent node runs in.
    pub project: PathBuf,
    /// The launching session this world's commands run under.
    pub session: String,
    /// Environment this world's commands carry beyond the defaults below.
    ///
    /// Every bound this crate takes is read from the environment of the process
    /// driving the run, so a journey about one has to be able to move it — and
    /// a bound nothing ever sets is a knob that has never been proven to be
    /// read. Applied after the defaults, so a journey overrides rather than
    /// races them, and empty for every world that names none.
    pub environment: Vec<(String, String)>,
}

impl World {
    /// A fresh world for one test.
    ///
    /// The root is named for nothing but uniqueness, and deliberately: a real
    /// `onevcs` identity derived from a local path becomes **one flattened
    /// directory component** under the state root, so every character of this
    /// path is spent twice in a session's clone — once as its own prefix and
    /// again inside the identity's directory name. Under a Windows temporary
    /// directory a descriptive name crossed MAX_PATH and `git clone` failed with
    /// "Filename too long" before a session ever opened. The test that failed
    /// names itself; its scratch directory does not have to.
    pub fn new(name: &str) -> Self {
        static NTH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "op-{}-{}",
            std::process::id(),
            NTH.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        // Created before it is resolved, because resolving a path asks the
        // filesystem about it, and in the one spelling the binary will report —
        // see [`resolved`].
        std::fs::create_dir_all(&root).expect("a scratch root");
        let root = resolved(&root);
        let world = Self {
            runs: root.join("runs"),
            fakes: root.join("fakes"),
            project: root.join("project"),
            root,
            session: format!("session-{name}"),
            environment: Vec::new(),
        };
        for dir in [&world.runs, &world.fakes, &world.project] {
            std::fs::create_dir_all(dir).expect("a scratch directory");
        }
        world
    }

    /// The same world seen by another planner's session.
    pub fn as_session(&self, session: &str) -> Self {
        LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self {
            root: self.root.clone(),
            runs: self.runs.clone(),
            fakes: self.fakes.clone(),
            project: self.project.clone(),
            session: session.to_string(),
            environment: self.environment.clone(),
        }
    }

    /// The same world, with one more environment variable on every command it
    /// runs.
    ///
    /// Taken at construction rather than settable afterwards, so a world cannot
    /// change what it means halfway through a journey: `World::new(..).with_env(..)`
    /// reads as one statement of what this world is.
    #[must_use]
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.environment.push((key.to_owned(), value.to_owned()));
        self
    }

    /// The `onepipeline` binary, wired to this world.
    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut command = Command::new(binary());
        let path = path_leading_with(&[onevcs_binary()
            .parent()
            .expect("onevcs has a directory")
            .to_path_buf()]);
        command
            .args(args)
            .env("PATH", path)
            .env("ONEPIPELINE_RUNS_DIR", &self.runs)
            // The store every plan in this world is read out of: one `local-md`
            // source rooted in the world's own directory, declared through
            // `onetaskgraph`'s own environment layer so no configuration file is
            // discovered from wherever the test process happens to be running.
            // `XDG_CONFIG_HOME` is redirected for the same reason — the
            // operator's own sources are not this world's.
            .env(STORE_BINARY_ENV, onetaskgraph_binary())
            .env("XDG_CONFIG_HOME", self.root.join("xdg"))
            .env("ONETASKGRAPH_DEFAULT_SOURCES", STORE_SOURCE)
            .env(
                format!(
                    "ONETASKGRAPH_SOURCES__{}__PLUGIN",
                    STORE_SOURCE.to_uppercase()
                ),
                "local-md",
            )
            .env(
                format!(
                    "ONETASKGRAPH_SOURCES__{}__CONFIG__ROOT",
                    STORE_SOURCE.to_uppercase()
                ),
                self.store(),
            )
            .env(
                "ONEPIPELINE_ONEAGENTGRAPH_BIN",
                double("fake-oneagentgraph"),
            )
            // `onevcs` is linked into the binary under test, so every command
            // below carries the state root, the git configuration, and the host
            // stand-in that library reads — including the ones that never touch a
            // repository, because a command that reached the operator's own
            // `~/.onevcs` would be a test writing outside its world.
            .env("ONEVCS_HOME", self.onevcs_home())
            .env("ONEVCS_GH", double("fake-gh"))
            // What a `change-auto` publication waits for the host to answer.
            // `onevcs` polls its checks and its merge for an hour by default —
            // a bound written for a repository whose CI is doing the work. Here
            // the host is a program that answers instantly, so the wait proves
            // nothing and the bound is only how long a journey about a host that
            // never answers takes to reach its ending.
            //
            // Two seconds is not a race with that host: the watch completes a
            // whole iteration — ask the checks, then ask whether it merged —
            // before it ever consults the bound, so a host that has an answer
            // gives it however slow the machine is. What the bound decides is
            // only how long a host with *no* answer is waited on.
            .env("ONEVCS_CHECKS_TIMEOUT_SECONDS", "2")
            .env("ONEVCS_CHECKS_POLL_SECONDS", "0.05")
            .env("GIT_CONFIG_GLOBAL", self.gitconfig())
            .env("GIT_AUTHOR_NAME", GIT_WHO)
            .env("GIT_AUTHOR_EMAIL", GIT_EMAIL)
            .env("GIT_COMMITTER_NAME", GIT_WHO)
            .env("GIT_COMMITTER_EMAIL", GIT_EMAIL)
            .env(SCRIPT_DIR_ENV, &self.fakes)
            // Where a dispatched double reaches this crate's own channel: the
            // `ask-manager` wrapper a worker asks its manager through runs
            // `onepipeline`, and the build under test is the one it has to run.
            .env(CLI_BIN_ENV, binary())
            .env("ONEPIPELINE_LAUNCHER", "e2e")
            .env("ONEPIPELINE_LAUNCHER_SESSION", &self.session)
            .env("ONEPIPELINE_PROJECT_DIR", &self.project)
            .env(
                "ONEPIPELINE_NODE_GRAPH",
                repo_file("graphs/node-scope.yaml"),
            )
            // Backoff is what the retry waits, not what it proves: a test that
            // slept the real five seconds would be measuring the sleep.
            .env("ONEPIPELINE_BOUNDARY_BACKOFF_SECONDS", "0")
            .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "20")
            // A held dispatch has to outlast the test holding it. The double's
            // own default is shorter than [`World::until`]'s deadline, so on a
            // host slow enough to matter the hold quietly expires, the node
            // completes, the run settles, and the test fails several steps later
            // on something that reads like a real defect — a reply refused
            // because the run it names had settled. Set above `until`'s deadline
            // so a rendezvous nobody releases fails as the timeout it is, with
            // the evidence `until` prints.
            .env(RENDEZVOUS_SECONDS_ENV, "180")
            .envs(self.environment.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null());
        command
    }

    /// Run a command to completion.
    pub fn run(&self, args: &[&str]) -> Run {
        Run::of(
            self.cmd(args).output().expect("the binary runs"),
            args,
            self,
        )
    }

    /// Read this world's project tasks back through the real onetaskgraph binary.
    pub fn store_tasks(&self, project: &str) -> Vec<Value> {
        self.store_pages::<StoreQualified<StoreTask>>(&["task", "list", "--project", project])
    }

    /// The real onetaskgraph binary, wired to the sources this world configures.
    ///
    /// The same declaration [`World::cmd`] hands the binary under test, plus whatever
    /// [`World::with_env`] added — so a journey that configured a second source reads and
    /// writes through it here exactly as the run does.
    pub fn store_cmd(&self, args: &[&str]) -> Command {
        let mut command = Command::new(onetaskgraph_binary());
        command
            .args(args)
            .env("XDG_CONFIG_HOME", self.root.join("xdg"))
            .env("ONETASKGRAPH_DEFAULT_SOURCES", STORE_SOURCE)
            .env(
                format!(
                    "ONETASKGRAPH_SOURCES__{}__PLUGIN",
                    STORE_SOURCE.to_uppercase()
                ),
                "local-md",
            )
            .env(
                format!(
                    "ONETASKGRAPH_SOURCES__{}__CONFIG__ROOT",
                    STORE_SOURCE.to_uppercase()
                ),
                self.store(),
            )
            // Only what onetaskgraph's own settings layer reads, and last, so a journey
            // that declared a second source reads through it here. `ONETASKGRAPH_BIN` is
            // deliberately not forwarded: it is *onepipeline*'s key for naming this
            // executable, which is already resolved above, and onetaskgraph would refuse
            // it by name as a setting it has never heard of.
            .envs(
                self.environment
                    .iter()
                    .filter(|(key, _)| key.starts_with("ONETASKGRAPH_") && key != STORE_BINARY_ENV)
                    .map(|(k, v)| (k, v)),
            );
        command
    }

    /// Read this world's project back through the real onetaskgraph binary.
    pub fn store_project(&self, project: &str) -> Value {
        let output = self
            .store_cmd(&["project", "show", project, "--json"])
            .output()
            .expect("the real onetaskgraph reads the project");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: StoreResponse<StoreQualified<StoreProject>> =
            serde_json::from_slice(&output.stdout).expect("project show returns schema-valid JSON");
        json!({"items": response.items, "next": response.next})
    }

    /// The labels this world's project holds on each of its tasks, by node id.
    ///
    /// Keyed by `onepipeline.id` rather than by the store's own task id, because the
    /// question a journey asks is whether the label a person put on *this node's* issue
    /// survived a projection — and a projection is free to have written the task under a
    /// different store id than the one it was authored at.
    pub fn store_task_labels(&self, project: &str) -> BTreeMap<String, Value> {
        self.store_tasks(project)
            .into_iter()
            .map(|task| {
                (
                    task["item"]["metadata"]["onepipeline.id"]
                        .as_str()
                        .expect("a projected task names its node")
                        .to_owned(),
                    task["item"]["labels"].clone(),
                )
            })
            .collect()
    }

    /// Read one projected task's dependency edges through onetaskgraph.
    pub fn store_deps(&self, task: &str) -> Vec<Value> {
        self.store_pages::<StoreEdge>(&["task", "deps", task, "--direction", "depends-on"])
    }

    fn store_pages<T>(&self, base: &[&str]) -> Vec<Value>
    where
        T: DeserializeOwned + Serialize,
    {
        let mut items = Vec::new();
        let mut page: Option<String> = None;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let mut args: Vec<String> = base.iter().map(|arg| (*arg).to_owned()).collect();
            args.push("--json".to_owned());
            if let Some(token) = &page {
                args.extend(["--page".to_owned(), token.clone()]);
            }
            let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
            let output = self
                .store_cmd(&borrowed)
                .output()
                .expect("the real onetaskgraph reads a store page");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let response: StoreResponse<T> = serde_json::from_slice(&output.stdout)
                .expect("the store page is schema-valid JSON");
            items.extend(response.items.into_iter().map(|item| {
                serde_json::to_value(item).expect("a validated store item renders as JSON")
            }));
            page = response.next;
            if page.is_none() {
                return items;
            }
            assert!(
                seen.insert(page.clone()),
                "the store repeated a page cursor"
            );
        }
    }

    /// The `onepipeline` binary with the **real** `oneagentgraph` behind that one
    /// seam, and the stand-in moved below it.
    ///
    /// Only that seam: the host stand-in [`cmd`](World::cmd) wires up stays, so
    /// these journeys need no credential, and they name no lifecycle node —
    /// what they are about is the dispatch. `oneagentgraph` resolves the graph,
    /// prepares the member, and supervises it for real, and what stands in is
    /// below it, at whichever process that library's own documented overrides
    /// name — neither of which knows anything about this crate.
    ///
    /// **Which process depends on the member kind**, and both are set here so a
    /// journey writing either graph gets the right one without having to know
    /// that. A single-sided member's turn is an `oneharness_core` library call,
    /// so the only process under it is the harness, and the stand-in is the paid
    /// model turn alone. A two-party member's conversation spawns one
    /// `oneharness` per side per turn — onejudge's spawning seam, which
    /// `oneagentgraph` puts it on by installing the spawn hook it reaps a paid
    /// harness through — so the stand-in there is that process, one layer up.
    ///
    /// Removing `ONEPIPELINE_ONEAGENTGRAPH_BIN` is what puts these
    /// journeys on the **default** path, where every verb is a library call —
    /// including the detached launch, which retains a process because a
    /// scheduler thread cannot outlive the launcher that is about to exit, and
    /// retains *this binary* at its own `drive` verb so that process composes
    /// the same build. Nothing here resolves `oneagentgraph` by name any more.
    ///
    /// The sibling's directory still leads the `PATH` because two journeys ask
    /// that binary a question of their own, and because a host with an install
    /// of its own must not be able to answer one. What a journey states about
    /// resolving *nothing* by name says so with [`World::empty_path`].
    pub fn agentgraph_cmd(&self, args: &[&str]) -> Command {
        let mut command = self.cmd(args);
        command
            .env_remove("ONEPIPELINE_ONEAGENTGRAPH_BIN")
            .env(
                "PATH",
                path_leading_with(&[
                    oneagentgraph_binary()
                        .parent()
                        .expect("oneagentgraph has a directory")
                        .to_path_buf(),
                    onevcs_binary()
                        .parent()
                        .expect("onevcs has a directory")
                        .to_path_buf(),
                ]),
            )
            // `oneharness` itself: the executable a two-party member's composed
            // provider block names, and the one an in-flight redirection is
            // delivered by. No single-sided turn comes through it.
            .env("ONEAGENTGRAPH_ONEHARNESS_BIN", double("fake-oneharness"))
            // The paid turn, at oneharness's own per-harness binary seam. A
            // single-sided member's turn is an `oneharness_core` library call
            // from `oneagentgraph 0.2.18` on, so `ONEAGENTGRAPH_ONEHARNESS_BIN`
            // above does not stand between that member and a provider — the
            // only process left below the library is the harness its identity
            // chain selects, which is this one. Set here rather than only in the
            // graphs' `oneharness.toml`, because a journey that writes its own
            // config would otherwise reach for a `claude` nobody in this suite
            // chose; the environment beats a config-file `bin`, so one value
            // covers every member of every graph a journey writes.
            .env("ONEHARNESS_BIN_CLAUDE_CODE", double("fake-claude"))
            // The candidate this suite's identity chains are written to fall
            // *through*, named at the same seam so that it is a fact of the
            // launch rather than a fact of the developer's machine. The chain a
            // fall-through journey writes is only a chain if its first candidate
            // is missing, and the inherited `PATH` still follows the entries
            // above — so on a host that has `codex` installed, oneharness
            // resolved it, the chain stepped past nothing, and the journey
            // failed on a premise the test could not see was false. It also
            // stopped that host from spending a *real* codex turn, which is what
            // the `env_remove`s below guard the same launch against.
            .env("ONEHARNESS_BIN_CODEX", self.uninstalled_harness())
            .env("ONEAGENTGRAPH_STATE_DIR", self.graph_state())
            .env(
                "ONEPIPELINE_NODE_GRAPH",
                self.graphs().join("node-scope.yaml"),
            )
            // This suite runs inside a dispatch of the very system it is a
            // library for, and that dispatch exports its own harness selection.
            // A leaked value would put a member on an identity — and a bill —
            // nobody in this test chose.
            .env_remove("ONEHARNESS_HARNESSES")
            .env_remove("ONEHARNESS_MODEL")
            .env_remove("ONEHARNESS_MODELS")
            .env_remove("ONEHARNESS_MODE");
        command
    }

    /// Where a harness this suite wants **not** installed would be.
    ///
    /// Inside the world, and under a directory nothing ever creates: a path that
    /// does not resolve is what oneharness reads as a candidate it cannot run,
    /// and one scoped to this world cannot be brought into existence by
    /// something an operator has on their host.
    fn uninstalled_harness(&self) -> PathBuf {
        self.root.join("harnesses-not-installed").join("codex")
    }

    /// Run a command against the real `oneagentgraph`.
    pub fn run_on_agentgraph(&self, args: &[&str]) -> Run {
        Run::of(
            self.agentgraph_cmd(args).output().expect("the binary runs"),
            args,
            self,
        )
    }

    /// The state root `onevcs` keeps everything under.
    ///
    /// Per world, so a journey's registry, sessions, streams, and locks are its
    /// own and never the operator's `~/.onevcs`.
    pub fn onevcs_home(&self) -> PathBuf {
        self.root.join("onevcs-home")
    }

    /// A repository this world's lifecycle nodes publish from.
    ///
    /// A bare origin, a checkout of it registered against that origin, and the
    /// rules file that decides how work published from it lands. The checkout is
    /// named `service`, which is the alias `onevcs` registers it under and
    /// therefore what [`lifecycle`] names as its `repo`.
    ///
    /// `publication` is the policy — `local-direct` reaches the base with git
    /// alone and asks no host for anything; a `change-*` policy opens a change
    /// request through [`ONEVCS_GH`](World::cmd)'s stand-in.
    ///
    /// `pre_push` is the repository's **own `pre-push` hook**, which is what
    /// verifies a local-publishing change now that nothing in this stack runs a
    /// gate of its own: `onevcs` names none, and a remote-publishing identity is
    /// verified by the host's required checks instead. `&[]` installs no hook,
    /// which is a repository whose merge path lets every publishing push
    /// through; a journey that needs verification to *refuse* states the argv
    /// [`hook_script`] writes here, and one that needs it to *hold* the push
    /// states [`held_publication`]'s, which is let through however that journey
    /// ends. A journey that means to *abandon* a hold reaches for
    /// [`abandonable_hook_script`], which exists on Unix alone.
    ///
    /// The hook is installed through `core.hooksPath` on the checkout rather
    /// than as tracked content, because a file in the tree would be published by
    /// the very journeys that install it. `onevcs` carries that setting into the
    /// clone a session cuts — `git clone` copies no repository-local config — so
    /// the hook git runs at the publishing push is this one.
    pub fn repository(&self, publication: &str, pre_push: &[&str]) -> Repository {
        let origin = self.root.join("origin.git");
        let checkout = self.root.join("service");
        let home = self.onevcs_home();
        for dir in [&origin, &home] {
            std::fs::create_dir_all(dir).expect("a scratch directory");
        }
        git(self, &origin, &["init", "--bare", "--initial-branch=main"]);
        git(
            self,
            &self.root,
            &["clone", &origin.to_string_lossy(), "service"],
        );
        std::fs::write(checkout.join("README.md"), "the repository under test\n")
            .expect("the seed file is written");
        git(self, &checkout, &["add", "-A"]);
        git(
            self,
            &checkout,
            &["commit", "-m", "chore: seed the repository"],
        );
        git(self, &checkout, &["push", "-u", "origin", "main"]);

        // Version 3, which is the shape with no `gate:` in it at all: what
        // verifies a change is the repository's own merge path, so the rules file
        // says only how the change is published and whether it needs approving.
        std::fs::write(
            home.join("rules.yml"),
            format!(
                "version: 3\nrules: []\ndefault:\n  publication: {publication}\n  approvals: \
                 none\n"
            ),
        )
        .expect("the rules file is written");

        if !pre_push.is_empty() {
            let hooks = hooks_dir(self);
            std::fs::create_dir_all(&hooks).expect("a hooks directory");
            install_hook(&hooks.join("pre-push"), pre_push);
            git(
                self,
                &checkout,
                &["config", "core.hooksPath", &hooks.to_string_lossy()],
            );
        }

        // A hosted origin, so the identity resolves to a host slug and a
        // `change-*` policy has somewhere to open a change request. It names the
        // identity and nothing else: the clone a session cuts takes its remote
        // from the *checkout*, which points at the bare origin above, so every
        // fetch and push this journey makes stays on this disk.
        self.register(&checkout, Some("https://github.com/owner/service.git"));
        Repository { origin, checkout }
    }

    /// A **second** repository, beside [`repository`](World::repository)'s own.
    ///
    /// A journey about a dependency that lands *outside* a node's own repository
    /// needs two identities to tell apart, and one origin cannot be two. It takes
    /// no publication policy and installs no hook: the rules file the first
    /// repository wrote is this **host's**, so it already decides how work
    /// published from either of them lands. Call it after that one.
    ///
    /// The alias is what `onevcs` registers the checkout under and therefore what
    /// a plan node names as its `repo`.
    pub fn extra_repository(&self, alias: &str) -> Repository {
        let origin = self.root.join(format!("{alias}.git"));
        let checkout = self.root.join(alias);
        std::fs::create_dir_all(&origin).expect("a scratch directory");
        git(self, &origin, &["init", "--bare", "--initial-branch=main"]);
        git(
            self,
            &self.root,
            &["clone", &origin.to_string_lossy(), alias],
        );
        std::fs::write(
            checkout.join("README.md"),
            format!("the {alias} repository\n"),
        )
        .expect("the seed file is written");
        git(self, &checkout, &["add", "-A"]);
        git(
            self,
            &checkout,
            &["commit", "-m", "chore: seed the repository"],
        );
        git(self, &checkout, &["push", "-u", "origin", "main"]);
        self.register(
            &checkout,
            Some(&format!("https://github.com/owner/{alias}.git")),
        );
        Repository { origin, checkout }
    }

    /// Commit a **release probe** into a repository, and say what it answers.
    ///
    /// The `script` form of a probe is a file the repository being released
    /// carries, run as a direct subprocess from the publication checkout on its
    /// base branch — so it is committed and pushed here rather than dropped in
    /// the tree. What it prints is whatever [`releases`](World::releases) later
    /// writes to the returned path, so one committed script answers "nothing is
    /// released", "0.1.0 is", and "0.2.0 is" as a journey moves through them; an
    /// absent file is a target with no release, which is an answer.
    ///
    /// Every run of it also leaves a line in the file
    /// [`probe_runs`](World::probe_runs) counts, so a journey about *how often*
    /// this host asks has something to assert on: a probe is a subprocess, and
    /// how many of them one release costs is behaviour rather than an internal.
    ///
    /// Returns the script's name relative to the repository root — which is what
    /// the release-targets document names it by — and the file its answer is
    /// written to.
    pub fn probe_in(&self, repository: &Repository, name: &str) -> (String, PathBuf) {
        let answer = self.root.join(format!("{name}.version"));
        let (script, body) = probe_script();
        let path = repository.checkout.join(script);
        std::fs::write(
            &path,
            body.replace("@VERSION_FILE@", &answer.to_string_lossy())
                .replace("@RUNS_FILE@", &self.probe_runs_file(name).to_string_lossy()),
        )
        .expect("the probe script is written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("the probe script is executable");
        }
        git(self, &repository.checkout, &["add", "-A"]);
        git(
            self,
            &repository.checkout,
            &["commit", "-m", "chore: carry a release probe"],
        );
        git(self, &repository.checkout, &["push", "origin", "main"]);
        (script.to_owned(), answer)
    }

    /// How many times the probe [`probe_in`](World::probe_in) committed for
    /// `name` has been run.
    ///
    /// The probe appends a line per run, so this is a count of subprocesses this
    /// host actually started — the only evidence a journey has for how often a
    /// release was asked about, because the sibling records a probe against the
    /// identity's own release stream rather than against this run's store.
    ///
    /// A tally that is not there yet is a count of none, and it is the **only**
    /// read failure that is: reading an unreadable one as zero would let a
    /// journey pass having counted a host that ran nothing. A torn last line is
    /// not a run to count either — this is polled while a probe appends to it.
    pub fn probe_runs(&self, name: &str) -> usize {
        let path = self.probe_runs_file(name);
        match std::fs::read_to_string(&path) {
            Ok(tally) => tally.lines().filter(|line| line.trim() == "run").count(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => panic!(
                "the probe tally {} is there and cannot be read, so no journey can count what \
                 this host asked: {error}",
                path.display()
            ),
        }
    }

    /// Where the probe committed for `name` records its runs.
    fn probe_runs_file(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.probe-runs"))
    }

    /// Write this host's release-targets document.
    ///
    /// `onevcs` finds it at one conventional path under the state root and
    /// nowhere else, so this writes it there. A world that never calls this has
    /// no such file at all, which is a host where every repository has no release
    /// targets and adopts fast — the behaviour every journey written before
    /// adoption existed runs under.
    pub fn releases(&self, document: &str) {
        let home = self.onevcs_home();
        std::fs::create_dir_all(&home).expect("the state root is there");
        std::fs::write(home.join("releases.yml"), document)
            .expect("the release-targets file is written");
    }

    /// Run one `onevcs` release verb **in this process**, under this world's
    /// state root.
    ///
    /// The same lock and the same reason as [`register`](World::register): the
    /// state root is process-global, so two worlds asking at once would otherwise
    /// read one another's.
    pub fn on_onevcs<T>(&self, ask: impl FnOnce() -> T) -> T {
        static ASKING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _held = ASKING.lock().unwrap_or_else(|held| held.into_inner());
        std::env::set_var("ONEVCS_HOME", self.onevcs_home());
        std::env::set_var("GIT_CONFIG_GLOBAL", self.gitconfig());
        ask()
    }

    /// Register a checkout with `onevcs`, **in this process**.
    ///
    /// A library call, because nothing in this repository reaches that sibling by
    /// spawning it any more. Its state root is process-global, so the variable is
    /// set and the registration run under one lock: two worlds registering at once
    /// would otherwise write into each other's registry.
    pub fn register(&self, checkout: &Path, origin: Option<&str>) {
        use clap::Parser;
        static REGISTERING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _held = REGISTERING.lock().unwrap_or_else(|held| held.into_inner());
        std::env::set_var("ONEVCS_HOME", self.onevcs_home());
        std::env::set_var("GIT_CONFIG_GLOBAL", self.gitconfig());
        let mut argv: Vec<String> = vec![
            "onevcs".to_owned(),
            "register".to_owned(),
            checkout.to_string_lossy().into_owned(),
        ];
        if let Some(origin) = origin {
            argv.push("--origin".to_owned());
            argv.push(origin.to_owned());
            self.point_the_host_at(checkout, origin);
        }
        let code = onevcs::run(&onevcs::cli::Cli::parse_from(argv));
        assert_eq!(code, 0, "onevcs refused to register {}", checkout.display());
    }

    /// Tell the `gh` stand-in where the origin behind one identity really is.
    ///
    /// The stand-in answers `headRefOid` off the branch, because `onevcs` reads
    /// every check beside that field; only this world knows where the branch
    /// lives, since the identity says `github.com/owner/service` while the git
    /// remote under it is a bare repository in this journey's scratch. One file
    /// per host slug, holding the checkout's own `origin` — read from git so
    /// what is written down is the remote the publication really pushes to.
    ///
    /// A non-GitHub identity or a checkout with no `origin` writes nothing and
    /// is answered by the stand-in's placeholder, which is why the remote is
    /// read without asserting on it rather than through [`git`](fn@git).
    fn point_the_host_at(&self, checkout: &Path, origin: &str) {
        let Some((_, slug)) = origin.trim_end_matches(".git").rsplit_once("github.com/") else {
            return;
        };
        let read = Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(checkout)
            .env("GIT_CONFIG_GLOBAL", self.gitconfig())
            .output()
            .expect("git runs");
        if !read.status.success() {
            return;
        }
        let remote = String::from_utf8_lossy(&read.stdout).trim().to_owned();
        if remote.is_empty() {
            return;
        }
        let dir = self.fakes.join("gh").join("origin");
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        std::fs::write(dir.join(segment(slug)), &remote)
            .expect("the origin behind the identity is written");
    }

    /// What `onevcs` resolved a repository to, as its own typed identity.
    ///
    /// Under the same lock and for the same reason as [`register`](World::register).
    pub fn identity(&self, repo: &Path) -> onevcs::Identity {
        static RESOLVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _held = RESOLVING.lock().unwrap_or_else(|held| held.into_inner());
        std::env::set_var("ONEVCS_HOME", self.onevcs_home());
        std::env::set_var("GIT_CONFIG_GLOBAL", self.gitconfig());
        onevcs::Providers::real()
            .vcs
            .resolve_identity(&repo.to_string_lossy())
            .unwrap_or_else(|error| panic!("onevcs cannot resolve {}: {error}", repo.display()))
    }

    /// The `onepipeline` binary with **nothing but the paid model turn standing
    /// in**: `onevcs` linked in, the `oneagentgraph` binary `Cargo.lock` pins,
    /// and the real `gh` against real GitHub. Its caller must have written the
    /// graph configs with [`write_graphs`](World::write_graphs) first, as
    /// [`agentgraph_cmd`](World::agentgraph_cmd)'s callers must.
    ///
    /// The host stand-in [`cmd`](World::cmd) wires up is **removed** here, and
    /// so are the check-polling bounds it shortens. Left in place, the first
    /// would point the one credentialled journey in this repository at a program
    /// that answers every `gh` call out of a scratch directory — a smoke that
    /// passes without having talked to GitHub, which is the defect that tier
    /// exists to remove. The second would hold a real host to a two-second
    /// answer: behind the stand-in there is nothing to wait for, and against
    /// GitHub there is, so this tier waits on the sibling's own defaults.
    pub fn real_cmd(&self, args: &[&str]) -> Command {
        let mut command = self.agentgraph_cmd(args);
        command.env_remove("ONEVCS_GH");
        command.env_remove("ONEVCS_CHECKS_POLL_SECONDS");
        command.env_remove("ONEVCS_CHECKS_TIMEOUT_SECONDS");
        command
    }

    /// Whether a command this world built still carries the host stand-in.
    ///
    /// For the credentialled tier to check before it starts, rather than to
    /// discover from a change request opened somewhere nobody can look.
    pub fn substitutes_the_host(command: &Command) -> bool {
        command
            .get_envs()
            .any(|(name, value)| name == "ONEVCS_GH" && value.is_some())
    }

    /// The git configuration this world's processes read instead of the
    /// operator's, created if it is not there yet.
    ///
    /// One file, pointed at with `GIT_CONFIG_GLOBAL`, so a journey can set what a
    /// real `onevcs` needs on a platform without leaving it set for whoever ran
    /// the suite — and so the operator's own global config cannot decide what
    /// these journeys see. Append to it for what one journey needs on top.
    ///
    /// `core.longpaths` is what it carries by default. A session's clone sits
    /// under the flattened identity directory *inside* the state root, which on
    /// Windows leaves the deepest files git writes — a pack index in the clone —
    /// past MAX_PATH even from a short root.
    pub fn gitconfig(&self) -> PathBuf {
        let path = self.root.join("gitconfig");
        if !path.is_file() {
            std::fs::write(&path, "[core]\n\tlongpaths = true\n")
                .expect("the world's git config is written");
        }
        path
    }

    /// Where this world's agent-graph configs live.
    pub fn graphs(&self) -> PathBuf {
        self.root.join("graphs")
    }

    /// Where the **real** `oneagentgraph` keeps this world's run state.
    ///
    /// The same directory [`agentgraph_cmd`](World::agentgraph_cmd) points that
    /// sibling at, so a journey can look at what a launch left in the sibling's
    /// own store rather than only at what this crate wrote down.
    pub fn graph_state(&self) -> PathBuf {
        self.root.join("graph-state")
    }

    /// Write the graph configs [`agentgraph_cmd`](World::agentgraph_cmd) names.
    ///
    /// Single-sided `kind: oneharness` members, because the seam nearly every
    /// journey is about is the same either way and a two-party member pays for a
    /// whole supervised conversation to reach it. The journeys that need that
    /// conversation say so with
    /// [`write_supervised_node_graph`](World::write_supervised_node_graph).
    pub fn write_graphs(&self) {
        self.write_graphs_with(None, CONSUMER_GRAPH_SCHEMA);
    }

    /// Replace the node-scope graph with a **two-party** `kind: onejudge` member,
    /// as the shipped one is, and write the onejudge base config it names.
    ///
    /// [`write_graphs`](World::write_graphs) must have run first: this reuses the
    /// unattributed identity chain it leaves in the graph directory, which is what
    /// the shipped graph names for both of a two-party member's sides.
    ///
    /// `12` is the default turn ceiling deliberately: it is the number a node
    /// declaring `45` was silently collapsed to for the whole life of the defect
    /// `dispatch.rs`'s turn-budget journey proves fixed.
    pub fn write_supervised_node_graph(&self) {
        std::fs::write(
            self.graphs().join("onejudge.base.yaml"),
            "system_prompt: Do the work.\nuser:\n  persona: Review it.\n  \
             done_when: the original task is complete\n  max_turns: 12\n",
        )
        .expect("the onejudge base config is written");
        std::fs::write(
            self.graphs().join("node-scope.yaml"),
            "version: 1\nname: node-scope\nmembers:\n  worker:\n    kind: onejudge\n    \
             base_config: ./onejudge.base.yaml\n    agent:\n      \
             oneharness_config: ./oneharness.toml\n    judge:\n      \
             oneharness_config: ./oneharness.toml\n    mode: bypass\n",
        )
        .expect("the node-scope graph is written");
    }

    /// The same configs at the **runner's own** schema version, with a second
    /// dag-scope member carrying its own [`MEMBER_TASK`].
    ///
    /// A per-member `task` is what a staler parser refuses, so this is the
    /// document that tells two parsers apart. The version is read off
    /// [`oneagentgraph::config::SCHEMA_VERSION`] rather than written here, so it
    /// moves with the runner.
    pub fn write_graphs_at_the_runners_schema(&self) {
        let extra = format!(
            "  {REPORTING_MEMBER}:\n    kind: oneharness\n    \
             oneharness_config: {}\n    task: {}\n",
            self.harness_config(REPORTING_MEMBER),
            yaml_scalar(MEMBER_TASK)
        );
        self.write_graphs_with(Some(&extra), oneagentgraph::config::SCHEMA_VERSION);
    }

    /// Write one member's own oneharness config, and return the reference a
    /// graph names it by.
    ///
    /// One file per member rather than one shared by all of them, because its
    /// `[env]` block is the only thing that reaches the harness carrying the
    /// member's name. A single-sided member's turn is an `oneharness_core`
    /// library call from `oneagentgraph 0.2.18` on, so the turn has no argv for
    /// the run to publish and no process for a journey to read a name off — and
    /// `[env]` is oneharness's own per-harness-process environment, which is
    /// exactly the seam the substituted binary already arrives on. Without it a
    /// journey can see that two members were given two different jobs and not
    /// which member got which.
    ///
    /// The binary itself is **not** named here: it rides
    /// `ONEHARNESS_BIN_CLAUDE_CODE` on every command, so a journey writing a
    /// config of its own gets the double without having to know about it.
    pub fn harness_config(&self, member: &str) -> String {
        let file = format!("oneharness-{member}.toml");
        let dir = self.graphs();
        std::fs::create_dir_all(&dir).expect("a directory for the graph configs");
        std::fs::write(
            dir.join(&file),
            format!(
                "run_mode = \"fallback\"\nharnesses = [\"claude-code\"]\n\n[env]\n\
                 {MEMBER_ENV} = \"{member}\"\n"
            ),
        )
        .expect("the member's harness config is written");
        format!("./{file}")
    }

    /// A `PATH` with nothing on it, in this world.
    ///
    /// For the journeys whose claim is that a launch resolves *nothing* by name.
    /// Prepending a directory cannot state that: the inherited `PATH` stays
    /// behind it, so a host with the sibling installed would answer the launch
    /// out of that install and the journey would pass for the wrong reason.
    pub fn empty_path(&self) -> PathBuf {
        let dir = self.root.join("empty-path");
        std::fs::create_dir_all(&dir).expect("a directory with nothing in it");
        dir
    }

    /// A `PATH` holding **only** what a dispatch resolves by name, and nothing
    /// else this host happens to have installed.
    ///
    /// Between [`empty_path`](Self::empty_path) — where a dispatch is refused
    /// because it cannot ask when its own process started — and the inherited
    /// `PATH`, where every program an operator installed is in reach. A journey
    /// whose premise is that some *particular* program cannot be resolved needs
    /// this one: it has to name what the launch may find rather than what it may
    /// not, because the set it may not find is everything on the host and no
    /// journey can enumerate that.
    ///
    /// On macOS and other non-Linux Unix that set is `ps`, delegated to the real
    /// one by absolute path. Linux asks procfs and Windows asks the process
    /// itself, so there is nothing to hold there and this is an empty directory.
    #[cfg(all(unix, not(target_os = "linux")))]
    pub fn path_with_only_what_a_dispatch_resolves(&self) -> PathBuf {
        self.path_with_ps("only-ps", &format!("exec {} \"$@\"", real_ps().display()))
    }

    /// A `PATH` holding only what a dispatch resolves by name. Linux reads the
    /// start token from procfs and Windows reads it from the process itself, so
    /// neither needs a named program here.
    #[cfg(any(target_os = "linux", windows))]
    pub fn path_with_only_what_a_dispatch_resolves(&self) -> PathBuf {
        self.empty_path()
    }

    /// Where `name` resolves on the `PATH` a built command carries, if it does.
    ///
    /// For a journey to state its premise as a *checked fact* rather than as a
    /// comment. A fall-through journey needs a candidate the launch cannot
    /// resolve, and left to the inherited `PATH` that premise is the host's to
    /// decide: with `codex` installed, oneharness resolved it, ran it, and the
    /// chain never advanced — so the journey failed twenty seconds later over an
    /// empty event list rather than at the premise it had lost.
    ///
    /// The command's own `PATH` and not this process's, because that is the one
    /// deciding. The suffixes are tried on Windows only, where a bare name never
    /// names a program: what this asks is whether the name is reachable *at all*,
    /// so it errs towards finding one.
    ///
    /// A candidate has to be one the host would actually *start*, which on Unix is
    /// the executable bit and not mere presence — a `codex` on the `PATH` without
    /// it is a file `execvp` skips, so reporting it as resolved would fail the
    /// journey below for a premise it still had.
    pub fn resolved_on(command: &Command, name: &str) -> Option<PathBuf> {
        let path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .map(std::ffi::OsString::from)
            .unwrap_or_else(|| std::env::var_os("PATH").unwrap_or_default());
        let suffixes: &[&str] = if cfg!(windows) {
            &["", ".exe", ".cmd", ".bat", ".com"]
        } else {
            &[""]
        };
        std::env::split_paths(&path)
            .flat_map(|dir| {
                suffixes
                    .iter()
                    .map(move |suffix| dir.join(format!("{name}{suffix}")))
            })
            .find(|candidate| runnable(candidate))
    }

    /// A `PATH` whose `ps` runs and **fails**, for the journeys about what a
    /// teardown does when it cannot read the process table.
    ///
    /// Distinct from [`empty_path`](Self::empty_path), and both are needed: a
    /// `ps` that cannot be spawned at all and a `ps` that answers with a
    /// non-zero exit are different faults, and a reader that checked only the
    /// first would parse the second one's stdout as if it were a listing. This
    /// one writes to stdout precisely so that a reader ignoring the exit status
    /// would see a plausible-looking table with this world's own processes
    /// absent from it — which is a teardown deciding it has no descendants.
    #[cfg(unix)]
    pub fn path_whose_ps_fails(&self) -> PathBuf {
        self.path_with_ps("failing-ps", "echo '1 0'\nexit 1")
    }

    /// A `PATH` whose `ps` answers with the **real** listing and one row nobody
    /// can read.
    ///
    /// The third fault, and the one that must cost the least: the listing is
    /// good, and one line of it is not — a header a platform adds, two columns
    /// run together. Every process it named is still named, so a teardown that
    /// threw the whole listing away over that row would strand all of them. The
    /// real `ps` is invoked by absolute path, because this stand-in holds the
    /// name `ps` on the `PATH` the process under test was given.
    #[cfg(unix)]
    pub fn path_whose_ps_garbles_a_row(&self) -> PathBuf {
        self.path_with_ps("garbled-ps", &listing_plus("not-a-pid also-not"))
    }

    /// A `PATH` whose `ps` answers with the real listing plus a child of `parent`
    /// that no signal can reach.
    ///
    /// The invented id is `u32::MAX`, which is not a pid any kernel issues and
    /// does not fit the signed integer `kill` takes — so the teardown refuses to
    /// send to it and reports that it did not reach it, which is the case under
    /// test. Deliberately a number rather than a real process: the honest way to
    /// produce "a process this user may not signal" would be to name one owned by
    /// somebody else, and a suite that signalled those would be a worse bug than
    /// any it was checking for.
    #[cfg(unix)]
    pub fn path_whose_ps_invents_an_unreachable_child(&self, parent: u32) -> PathBuf {
        self.path_with_ps(
            "unreachable-child-ps",
            &listing_plus(&format!("{} {parent}", u32::MAX)),
        )
    }

    /// A `PATH` whose `ps` answers a question about **one** process with more
    /// than it was asked for.
    ///
    /// The fault that is not about the listing at all. A start token is `ps -p
    /// PID -o lstart=` — one process, one line — and a host that writes anything
    /// beside that has not answered the question. Folding what it wrote into a
    /// token would make a live process disagree with the stamp its own record
    /// carries, which a reader takes for a pid the host has handed to somebody
    /// else: the one verdict that must never come from the host misbehaving.
    #[cfg(all(unix, not(target_os = "linux")))]
    pub fn path_whose_ps_says_more_than_it_was_asked(&self) -> PathBuf {
        self.path_with_ps("talkative-ps", &answer_plus("a line nobody asked for"))
    }

    /// A `PATH` whose `ps lstart` answer has drifted far farther than the 33
    /// seconds measured from one live Linux process in the originating report.
    ///
    /// The process table remains real. On Linux the product must not ask this
    /// wall-clock question at all: its stable start token comes from procfs.
    #[cfg(target_os = "linux")]
    pub fn path_whose_ps_start_time_has_drifted(&self) -> PathBuf {
        self.path_with_ps(
            "drifted-start-ps",
            &format!(
                "case \" $* \" in\n  *lstart=*) echo 'Thu Jan  1 00:00:00 1970'; exit 0 ;;\nesac\nexec {} \"$@\"",
                real_ps().display()
            ),
        )
    }

    /// A `PATH` holding one `ps` stand-in that behaves like `script`.
    ///
    /// Unix-only: the fixture is a shell script, and the Windows arm reaches the
    /// tree through `taskkill /T` rather than through any table this could stand
    /// in for.
    ///
    /// See [`listing_plus`] for why a stand-in that alters the **listing** has
    /// to leave every other question alone.
    #[cfg(unix)]
    fn path_with_ps(&self, name: &str, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).expect("a directory for the ps stand-in");
        let ps = dir.join("ps");
        std::fs::write(&ps, format!("#!/bin/sh\n{script}\n")).expect("the ps stand-in is written");
        std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755))
            .expect("the ps stand-in is executable");
        dir
    }

    /// The same configs, with the shipped dag-scope graph's **pacemaker** on the
    /// driver: the member a planner-visible surface restarts the clock of.
    ///
    /// Its own member, because that is the one a reset addresses, and a graph
    /// without it answers with "no such member" no matter which run id it was
    /// addressed by.
    ///
    /// Its name and its schedule are read out of `graphs/dag-scope.yaml` rather
    /// than restated, so the two have one source. A stand-in carrying its own
    /// copy would keep passing against a shipped graph that had renamed the
    /// member or made its clock unresettable — the two facts these journeys
    /// exist to reset. Only the harness config is this world's own: the shipped
    /// one names an operator file no scratch graph directory has. The interval
    /// comes over with the rest, and it is long enough that nothing fires
    /// inside a test — what a journey observes is the *reset*.
    pub fn write_graphs_with_pacemaker(&self) {
        use oneagentgraph::config::Member;

        let shipped = repo_file("graphs/dag-scope.yaml");
        let config: oneagentgraph::config::GraphConfig = serde_norway::from_str(
            &std::fs::read_to_string(&shipped).expect("the shipped dag-scope graph"),
        )
        .expect("the shipped dag-scope graph is a valid oneagentgraph config");
        let (member, schedule) = config
            .members
            .iter()
            .find_map(|(name, member)| match member {
                Member::Oneharness(member) => member.schedule.map(|schedule| (name, schedule)),
                _ => None,
            })
            .expect("the shipped dag-scope graph declares a pacemaker");
        let pacemaker = format!(
            "  {member}:\n    kind: oneharness\n    oneharness_config: {}\n    \
             schedule: {{every: {}, resettable: {}}}\n",
            self.harness_config(member),
            schedule.every,
            schedule.resettable
        );
        self.write_graphs_with(Some(&pacemaker), CONSUMER_GRAPH_SCHEMA);
    }

    /// The graph a launch drafts change request bodies with, as
    /// `--pr-author-graph` takes it.
    ///
    /// Written on demand rather than by [`write_graphs`](World::write_graphs),
    /// because naming one is what a journey about drafting *states*: every other
    /// journey proves a launch that drafts nothing, which is the shipped
    /// default.
    ///
    /// Its member is single-sided and its own oneharness config declares a
    /// `schema_file`, which is what asks that library for an answer validated
    /// against a schema — the channel a drafted body arrives on. The schema is
    /// this world's own file, so the shape the drafting dispatch must answer in
    /// is stated once, here, and the graph is a real document either sibling
    /// will read.
    pub fn pr_author_graph(&self) -> String {
        let dir = self.graphs();
        std::fs::create_dir_all(&dir).expect("a directory for the graph configs");
        std::fs::write(
            dir.join("body.schema.json"),
            r#"{"type":"object","properties":{"body":{"type":"string"}},
               "required":["body"],"additionalProperties":false}"#,
        )
        .expect("the drafted body's schema is written");
        std::fs::write(
            dir.join("oneharness.pr-author.toml"),
            "run_mode = \"fallback\"\nharnesses = [\"claude-code\"]\n\
             schema_file = \"./body.schema.json\"\n",
        )
        .expect("the drafting harness config is written");
        let graph = dir.join("pr-author.yaml");
        std::fs::write(
            &graph,
            format!(
                "version: {CONSUMER_GRAPH_SCHEMA}\nname: pr-author\nmembers:\n  author:\n    \
                 kind: oneharness\n    oneharness_config: ./oneharness.pr-author.toml\n"
            ),
        )
        .expect("the pr-author graph is written");
        graph.display().to_string()
    }

    fn write_graphs_with(&self, dag_extra: Option<&str>, version: u32) {
        let dir = self.graphs();
        std::fs::create_dir_all(&dir).expect("a directory for the graph configs");
        // The identity chain is the operator's own file, which the graph names
        // and this suite never selects out of: one harness family, so the model
        // pairing rule holds. This unattributed copy is what a two-party
        // member's two sides name and what a journey copying a graph elsewhere
        // takes with it; each single-sided member below gets one of its own.
        std::fs::write(
            dir.join("oneharness.toml"),
            "run_mode = \"fallback\"\nharnesses = [\"claude-code\"]\n",
        )
        .expect("the harness config is written");
        for (file, member) in [("dag-scope.yaml", "monitor"), ("node-scope.yaml", "worker")] {
            // The dag-scope monitor carries [`MONITOR_TASK`]; the node-scope
            // worker carries none, because the task a *node* is dispatched with
            // is that member's whole job and the launcher composes it per
            // dispatch.
            let (extra, task) = if file == "dag-scope.yaml" {
                (
                    dag_extra.unwrap_or_default(),
                    format!("    task: {}\n", yaml_scalar(MONITOR_TASK)),
                )
            } else {
                ("", String::new())
            };
            // Which run this graph is watching, in the member's own environment.
            // A launch hands the two variables to the graph *run*, where they
            // expand `${...}` references — reaching a member's own process is
            // what the graph's `env:` block is for, and the shipped dag-scope
            // graph says the same thing in the one place it needs the run id: the
            // channel-serve command its judge side runs.
            let env = if file == "dag-scope.yaml" {
                "env:\n  ONEPIPELINE_RUN_ID: ${ONEPIPELINE_RUN_ID}\n  \
                 ONEPIPELINE_RUNS_DIR: ${ONEPIPELINE_RUNS_DIR}\n"
            } else {
                ""
            };
            std::fs::write(
                dir.join(file),
                format!(
                    "version: {version}\nname: {}\nmembers:\n  {member}:\n    \
                     kind: oneharness\n    oneharness_config: {}\n{task}{extra}{env}",
                    file.trim_end_matches(".yaml"),
                    self.harness_config(member),
                ),
            )
            .expect("the graph config is written");
        }
    }

    /// Run the binary with a hard ceiling on how large any file it writes may
    /// grow — the real short write a full disk answers with.
    ///
    /// Nothing is substituted and no write is intercepted: `RLIMIT_FSIZE` is the
    /// kernel's own ceiling, and past it `write(2)` behaves exactly as it does on
    /// a filesystem that has run out — the first call takes the bytes that fit
    /// and returns that partial count, and the next one fails. `SIGXFSZ` is set
    /// to ignored in the child so the failure arrives as `EFBIG` at the write
    /// rather than as a signal that ends the process, which is what a full disk
    /// gives a writer.
    ///
    /// Unix only: the ceiling is a Unix resource limit, and the journey that
    /// needs it says so with its own `cfg`.
    #[cfg(unix)]
    pub fn run_with_file_ceiling(&self, args: &[&str], ceiling: u64) -> Run {
        use std::os::unix::process::CommandExt;

        let mut command = self.cmd(args);
        // The ceiling is on **every** file the child writes, and under `just
        // test` one of them is the coverage profile the instrumented binary
        // dumps at exit. Truncated, that profile is a corrupt header the whole
        // run's merge then fails on — a green suite reported as a coverage
        // failure. So this child's profile is written into the world's own
        // scratch, outside the directory the merge reads.
        command.env(
            "LLVM_PROFILE_FILE",
            self.root.join("under-a-ceiling-%p.profraw"),
        );
        // SAFETY: the closure runs between `fork` and `exec` in the child, and
        // calls only async-signal-safe syscalls — no allocation, no locks.
        unsafe {
            command.pre_exec(move || {
                let ceiling = libc::rlimit {
                    rlim_cur: ceiling,
                    rlim_max: ceiling,
                };
                if libc::setrlimit(libc::RLIMIT_FSIZE, &ceiling) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::signal(libc::SIGXFSZ, libc::SIG_IGN) == libc::SIG_ERR {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Run::of(command.output().expect("the binary runs"), args, self)
    }

    /// Run an already-configured command to completion.
    ///
    /// For a journey that needs one environment override — a timeout it is
    /// deliberately driving past — without a second copy of everything
    /// [`cmd`](World::cmd) wires up.
    pub fn run_on(&self, command: Command, args: &str) -> Run {
        let mut command = command;
        Run::of(command.output().expect("the binary runs"), &[args], self)
    }

    /// Run a command with an envelope on stdin.
    pub fn run_with_stdin(&self, args: &[&str], stdin: &str) -> Run {
        self.run_with_stdin_on(self.cmd(args), stdin)
    }

    /// Run an already-configured command with an envelope on stdin.
    ///
    /// The caller configures the command — an environment override, most often
    /// the reply timeout — and this only feeds it and waits.
    pub fn run_with_stdin_on(&self, command: Command, stdin: &str) -> Run {
        use std::io::Write;
        let mut command = command;
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the binary starts");
        child
            .stdin
            .as_mut()
            .expect("stdin is piped")
            .write_all(stdin.as_bytes())
            .expect("the envelope is written");
        Run::of(
            child.wait_with_output().expect("the binary runs"),
            &["reply"],
            self,
        )
    }

    /// This world's `onetaskgraph` store: one folder of Markdown, no network.
    pub fn store(&self) -> PathBuf {
        self.root.join("plan-store")
    }

    /// Write a plan into this world's store and return the project id that
    /// launches it.
    ///
    /// The inverse of what `src/taskgraph.rs` reads, written from the plan
    /// document a journey states: the plan-level settings become reserved
    /// `onepipeline.<field>` metadata on the **project**, each node becomes one
    /// **task** carrying its own id, title, prose and repositories, and each
    /// dependency becomes a real dependency edge between two of those tasks. A
    /// cross-DAG `run:<id>#<node>` reference is the one dependency that cannot
    /// be an edge — it names another run's DAG, which is an item of no source —
    /// so it stays on the reserved key.
    ///
    /// Deliberately tolerant of a document the schema will refuse: a journey
    /// about a refusal states the plan it means and this writes it faithfully,
    /// so what refuses it is the reader under test rather than the fixture.
    pub fn plan(&self, name: &str, plan: &Value) -> String {
        let plan = plan.as_object().expect("a plan is a mapping");
        let mut metadata = serde_json::Map::new();
        for (key, value) in plan {
            if key == "name" || key == "tasks" {
                continue;
            }
            metadata.insert(reserved(key), value.clone());
        }
        let title = plan.get("name").and_then(Value::as_str).unwrap_or(name);
        let identifier = project_id(name);
        // A second project, so that "which project did it read?" is a question a
        // fixture can answer. Only once per world.
        self.decoy_project();
        self.write_item(
            &self
                .store()
                .join("projects")
                .join(format!("{identifier}.md")),
            &[("title", json!(title)), ("metadata", json!(metadata))],
            "",
        );
        let nodes = plan
            .get("tasks")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        // A task's name in the store is not its node id, and this writer uses
        // that on purpose. `local-md` traverses lexicographically, so numbering
        // the files gives the project back in the order the document listed its
        // nodes — which is what a plan file used to decide, and what several
        // journeys about dispatch order are written against. It is also what
        // lets two tasks of one project carry one node id, which a journey about
        // that collision needs.
        let files: Vec<String> = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let id = node.get("id").and_then(Value::as_str).unwrap_or("unnamed");
                format!("{index:03}-{id}")
            })
            .collect();
        let named: BTreeMap<&str, &str> = nodes
            .iter()
            .zip(&files)
            .filter_map(|(node, file)| Some((node.get("id")?.as_str()?, file.as_str())))
            .collect();
        for (node, file) in nodes.iter().zip(&files) {
            self.task(&identifier, node, file, &named);
        }
        if let Some(missing) = undiscriminating(&self.store()) {
            panic!("{missing}");
        }
        format!("{STORE_SOURCE}:{identifier}")
    }

    /// The project a run must never touch, and the reason every store fixture
    /// here holds one.
    ///
    /// Its task carries the node id this suite's plans use most, so a read that
    /// dropped `--project` does not quietly succeed: it comes back holding two
    /// tasks for one node, which is a refusal rather than a silent extra.
    fn decoy_project(&self) {
        let path = self
            .store()
            .join("projects")
            .join(format!("{DECOY_PROJECT}.md"));
        if path.exists() {
            return;
        }
        self.write_item(
            &path,
            &[
                ("title", json!("A board this run must not touch")),
                ("metadata", json!({"authored.note": "not this run's board"})),
            ],
            "",
        );
        for (file, node) in [("000-work", "work"), ("001-elsewhere", "elsewhere")] {
            self.write_item(
                &self
                    .store()
                    .join("tasks")
                    .join(DECOY_PROJECT)
                    .join(format!("{file}.md")),
                &[
                    ("title", json!(format!("chore: {node}, on another board"))),
                    ("project", json!(DECOY_PROJECT)),
                    ("metadata", json!({"onepipeline.id": node})),
                ],
                "",
            );
        }
    }

    /// One node of a plan, as the task carrying it.
    fn task(&self, project: &str, node: &Value, file: &str, named: &BTreeMap<&str, &str>) {
        let node = node.as_object().expect("a node is a mapping");
        let id = node.get("id").and_then(Value::as_str);
        let mut metadata = serde_json::Map::new();
        if let Some(id) = id {
            metadata.insert("onepipeline.id".into(), json!(id));
        }
        let mut edges: Vec<String> = Vec::new();
        let mut crossdag: Vec<String> = Vec::new();
        for dep in node
            .get("deps")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            match dep.as_str() {
                // What a cross-DAG reference names is another run, so no
                // dependency edge in any store can point at it.
                Some(reference) if reference.starts_with("run:") => {
                    crossdag.push(reference.to_owned());
                }
                // A dependency stated as a *store* id rather than as a node id
                // of this plan: what a journey about an edge leaving the
                // project needs, and the one thing a plan document could not
                // say.
                Some(elsewhere) => match elsewhere.strip_prefix("store:") {
                    Some(native) => edges.push(native.to_owned()),
                    // By the far node's own file, which is not its node id: what
                    // an edge names is the store's id, and what the plan reads
                    // back off the far task is `onepipeline.id`. A dependency on
                    // a node this plan does not hold names it directly, which is
                    // what the dangling-reference journey states.
                    None => edges.push(format!(
                        "{project}/{}",
                        named.get(elsewhere).copied().unwrap_or(elsewhere)
                    )),
                },
                None => panic!("a dependency is a string"),
            }
        }
        if !crossdag.is_empty() {
            metadata.insert("onepipeline.deps".into(), json!(crossdag));
        }
        let mut front = vec![
            (
                "title",
                json!(node.get("title").and_then(Value::as_str).unwrap_or("")),
            ),
            ("project", json!(project)),
        ];
        if !edges.is_empty() {
            front.push(("depends_on", json!(edges)));
        }
        // A `onevcs` identity is either a normalized origin, which the store's
        // own repository list holds, or a local checkout named by its absolute
        // path, which that list cannot hold at all — so the second kind travels
        // on the reserved key. `docs/contract-divergences.md` records it.
        if let Some(repo) = node.get("repo").and_then(Value::as_str) {
            if repo.starts_with("github.com/") {
                // These are the hosted origins this suite states literally;
                // the real store owns and applies their shape validation.
                front.push(("repositories", json!([repo])));
            } else {
                // Aliases, local paths, and deliberately malformed schema
                // fixtures ride the consumer key. This is fixture routing, not
                // a second implementation of onetaskgraph's origin grammar.
                metadata.insert("onepipeline.repo".into(), json!(repo));
            }
        }
        for (key, value) in node {
            if matches!(key.as_str(), "id" | "title" | "task" | "deps" | "repo") {
                continue;
            }
            metadata.insert(reserved(key), value.clone());
        }
        front.push(("metadata", json!(metadata)));
        self.write_item(
            &self
                .store()
                .join("tasks")
                .join(project)
                .join(format!("{file}.md")),
            &front,
            node.get("task").and_then(Value::as_str).unwrap_or_default(),
        );
    }

    /// Write one document of this world's store verbatim.
    ///
    /// For the journeys that are about a document the writer above would never
    /// produce — a task with no metadata at all, say — where stating the bytes
    /// is what the journey means.
    pub fn write_store_item(&self, relative: &str, document: &str) {
        let path = self.store().join(relative);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("a store directory");
        std::fs::write(path, document).expect("the store document is written");
    }

    /// A task of this store that belongs to no plan a journey launches.
    ///
    /// What a dependency edge leaving the project points at: it carries a node
    /// id of its own, so the plan resolves the edge and then finds no node of
    /// that name — which is the dangling-reference refusal, reached the way a
    /// store reaches it.
    pub fn stray_task(&self, project: &str, file: &str, node_id: &str) {
        self.write_item(
            &self
                .store()
                .join("tasks")
                .join(project_id(project))
                .join(format!("{file}.md")),
            &[
                ("title", json!("a task of another plan")),
                ("project", json!(format!("{project}-elsewhere"))),
                ("metadata", json!({"onepipeline.id": node_id})),
            ],
            "",
        );
    }

    /// One Markdown document of the store: YAML front matter, then the body.
    ///
    /// Every front-matter value is written as compact JSON, which is YAML flow
    /// style — so a value reaches the store with the JSON type the journey wrote
    /// it at, and the writer needs no YAML emitter of its own.
    fn write_item(&self, path: &Path, front: &[(&str, Value)], body: &str) {
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("a store directory");
        let mut document = String::from("---\n");
        for (key, value) in front {
            document.push_str(&format!(
                "{key}: {}\n",
                serde_json::to_string(value).expect("a front-matter value serialises")
            ));
        }
        document.push_str("---\n\n");
        document.push_str(body);
        document.push('\n');
        std::fs::write(path, document).expect("the store document is written");
    }

    /// Script one of the doubles: `world.script("build.fail", "1")`.
    pub fn script(&self, name: &str, body: &str) {
        std::fs::write(self.fakes.join(name), body).expect("the script is written");
    }

    /// Release a rendezvous the doubles are holding.
    pub fn release(&self, name: &str) {
        std::fs::write(self.fakes.join(name), "go").expect("the rendezvous is released");
    }

    /// Wait until a predicate holds, or fail with what was seen instead.
    pub fn until(&self, what: &str, mut ready: impl FnMut(&Self) -> bool) {
        if waited(|| ready(self)) {
            return;
        }
        panic!(
            "timed out waiting for {what}; the runs root held:\n{}",
            self.dump()
        );
    }

    /// Wait for a predicate that reads the store through a real sibling process.
    ///
    /// A normal wait observes files and can poll cheaply. Each store observation
    /// starts `onetaskgraph`, though, and polling that boundary every 20ms can
    /// consume the process-start capacity the asynchronous copy itself needs on a
    /// loaded cross-platform runner. The deadline and assertion stay identical;
    /// only the expensive observer yields between reads.
    pub fn until_store(&self, what: &str, mut ready: impl FnMut(&Self) -> bool) {
        self.until_store_for(std::time::Duration::from_secs(120), what, &mut ready);
    }

    fn until_store_for(
        &self,
        duration: std::time::Duration,
        what: &str,
        mut ready: impl FnMut(&Self) -> bool,
    ) {
        if waited_for(duration, std::time::Duration::from_millis(250), || {
            ready(self)
        }) {
            return;
        }
        panic!(
            "timed out waiting for {what}; the runs root held:\n{}",
            self.dump()
        );
    }

    /// Wait until a file inside a run's directory holds `needle`, or fail with
    /// what it held instead.
    ///
    /// [`until`](Self::until) dumps the runs root, which is where a journey
    /// waiting on an *event* finds its evidence. A journey waiting on a line
    /// some other process writes on its own cadence needs the file it was
    /// waiting on, so that is what this one prints.
    pub fn until_run_file_holds(&self, run: &str, relative: &str, needle: &str) {
        let path = self.run_file(run, relative);
        // Read leniently: a file the writer has not created yet is a wait, not a
        // failure, and if it never appears the reason is the evidence a timeout
        // has to print.
        let held =
            || std::fs::read_to_string(&path).unwrap_or_else(|e| format!("(unreadable: {e})"));
        if waited(|| held().contains(needle)) {
            return;
        }
        panic!(
            "timed out waiting for {needle:?} in {}; it held:\n{}",
            path.display(),
            held()
        );
    }

    /// Every run's whole directory, and what each observer the launcher started
    /// found. What a failure needs to say *why* it failed — the alternative is a
    /// bare assertion with no evidence, which is a whole debugging session per
    /// platform-only defect.
    ///
    /// **Enumerated, never named.** A dump that opens a fixed list of files is
    /// blind to whatever a later change records beside them, and a diagnostic
    /// that hides the one artifact naming a cause is how a reader spends the
    /// session guessing. Walking the directory is what makes the next artifact
    /// arrive without this function being edited.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        match std::fs::read_dir(&self.runs) {
            Err(_) => out.push_str("  (no runs root)\n"),
            Ok(entries) => {
                // Sorted, because a directory listing is in whatever order the
                // filesystem answers in: two dumps of one run would otherwise be
                // a diff of the ordering rather than of what the run recorded.
                let mut runs: Vec<String> = entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().to_string())
                    .collect();
                runs.sort();
                for run in runs {
                    out.push_str(&format!("  {run}:\n"));
                    dump_into(&mut out, &self.runs.join(&run), "    ");
                }
            }
        }
        for saw in self.observer_saw() {
            out.push_str(&format!("  observer saw: {saw}\n"));
        }
        out
    }

    /// Everything the doubles were asked for, in order.
    pub fn invocations(&self) -> Vec<Value> {
        read_jsonl(&self.fakes.join("invocations.jsonl"))
    }

    /// Every model turn that really ran, in order: what it was asked to do,
    /// where it worked, and which member it was.
    ///
    /// The one place a journey can read a member's *prose* from. A single-sided
    /// member's turn is a library call inside `oneagentgraph` from 0.2.18 on, so
    /// its `member-started` names the config and worktree it was prepared with
    /// and there is no argv on it to read a prompt off — the turn itself is the
    /// last process in the stack, and this is what it recorded about the one it
    /// was given.
    pub fn turns(&self) -> Vec<Turn> {
        self.invocations()
            .into_iter()
            .filter(|call| call["tool"] == "claude-turn")
            .map(|call| Turn {
                prompt: Self::recorded(&call, 0, "the prompt"),
                cwd: Self::recorded(&call, 1, "the working directory"),
                member: Self::recorded(&call, 2, "the member"),
                scratch: Self::recorded(&call, 3, "the scratch directory"),
            })
            .collect()
    }

    /// One recorded string off a double's invocation, refused if it is not there.
    ///
    /// A field read leniently would let a double that stopped recording one look,
    /// to every assertion here, exactly like one still recording it correctly.
    fn recorded(call: &Value, at: usize, what: &str) -> String {
        call["args"][at]
            .as_str()
            .unwrap_or_else(|| panic!("a recorded turn carries no string for {what}: {call}"))
            .to_string()
    }

    /// The prose one named member's turn was given.
    ///
    /// Panics when that member never ran, naming what did: a journey asserting
    /// on a job nobody was handed would otherwise read as a job handed wrongly.
    pub fn turn_of(&self, member: &str) -> String {
        let turns = self.turns();
        turns
            .iter()
            .find(|turn| turn.member == member)
            .map(|turn| turn.prompt.clone())
            .unwrap_or_else(|| panic!("no member '{member}' ran a turn: {turns:?}"))
    }

    /// What each observer the launcher started found waiting for it, in order.
    pub fn observer_saw(&self) -> Vec<Value> {
        read_jsonl(&self.fakes.join("observer-saw.jsonl"))
    }

    /// What a supervised observer member raised, what it was ruled, and what
    /// ended it — in order.
    ///
    /// Written by the member's own judge-side exchange, so a journey reads the
    /// supervision rather than inferring it from the channel's own files.
    pub fn observer_supervision(&self) -> Vec<Value> {
        read_jsonl(&self.fakes.join("observer-supervision.jsonl"))
    }

    /// The question this run's channel hands its manager, read the way a manager
    /// reads one: `onepipeline next`.
    ///
    /// Panics when the channel had nothing to hand out, because a journey about
    /// a worker that asked has nothing to say about a run nobody asked on.
    pub fn question_for_the_manager(&self, run: &str) -> String {
        self.question_for_the_manager_on(self.cmd(&["next", run]), run)
    }

    /// The same read, made by an already-configured command — the journeys that
    /// drive the real `oneagentgraph` build theirs with
    /// [`agentgraph_cmd`](World::agentgraph_cmd).
    pub fn question_for_the_manager_on(&self, command: Command, run: &str) -> String {
        let read = self.run_on(command, &format!("next {run}"));
        read.exited(0);
        match read.json()["surface"]["message"].as_str() {
            Some(question) => question.to_string(),
            None => panic!(
                "this run's manager was handed no question:\n{}\nthe runs root held:\n{}",
                read.stdout,
                self.dump()
            ),
        }
    }

    /// Every question that reached this run's channel, off the stream a manager
    /// watches it on: `onepipeline monitor`, whose line for a queued surface
    /// carries the words it was raised with.
    ///
    /// What [`question_for_the_manager`](World::question_for_the_manager) reads is
    /// the *next* one, and a newer check-in replaces the queued one rather than
    /// waiting behind it — so a run whose workers asked more than once says so
    /// here.
    pub fn questions_on_the_stream(&self, run: &str) -> Vec<String> {
        let streamed = self.run(&["monitor", run]);
        streamed.exited(0);
        streamed
            .stdout
            .lines()
            .filter_map(|line| line.split_once("planner-surface-queued "))
            .map(|(_, question)| question.trim().to_string())
            .collect()
    }

    /// The dag-scope graph this world writes, as the flag `start` takes it.
    ///
    /// Passed rather than defaulted, because the shipped default is `off`: a
    /// journey that wants an observer says so, and every other one proves a run
    /// that launches no agent graph at all.
    pub fn dag_graph(&self) -> String {
        self.graphs().join("dag-scope.yaml").display().to_string()
    }

    /// The dag-scope graph *this repository* ships, as that same flag.
    pub fn shipped_dag_graph(&self) -> String {
        repo_file("graphs/dag-scope.yaml").display().to_string()
    }

    /// Whether a double was asked for a command whose arguments contain each of
    /// `parts`.
    pub fn was_invoked(&self, tool: &str, parts: &[&str]) -> bool {
        self.invocations().iter().any(|invocation| {
            invocation["tool"] == tool
                && parts.iter().all(|part| {
                    invocation["args"]
                        .as_array()
                        .is_some_and(|args| args.iter().any(|arg| arg == part))
                })
        })
    }

    /// One run's merged event store.
    pub fn journal(&self, run: &str) -> Vec<Value> {
        read_jsonl(&self.runs.join(run).join("events.jsonl"))
    }

    /// The kinds one run's journal recorded, in order.
    pub fn kinds(&self, run: &str) -> Vec<String> {
        self.journal(run)
            .iter()
            .filter_map(|event| event["kind"].as_str().map(str::to_string))
            .collect()
    }

    /// Wait for the planner surface of one kind, and answer with it.
    ///
    /// The engine journals the fact a surface is *about* — a worker gone quiet,
    /// an edit refused — and then raises the surface, as two appends. A test
    /// that waited on the fact and went straight to the surface would be reading
    /// between them whenever the host put the two far enough apart, and would
    /// fail having never waited for the thing it asserts. Waiting on the surface
    /// is what closes that window; the fact is already there once it is, because
    /// it is written first.
    pub fn surfaced(&self, run: &str, kind: &str) -> Value {
        self.until(&format!("the {kind} surface"), |world| {
            world.surface_of(run, kind).is_some()
        });
        self.surface_of(run, kind)
            .expect("the surface was just seen")
    }

    /// The planner surface of one kind, if it has been raised.
    fn surface_of(&self, run: &str, kind: &str) -> Option<Value> {
        self.events_of(run, "planner-surface-queued")
            .into_iter()
            .find(|event| event["payload"]["kind"] == kind)
    }

    /// The events of one kind.
    pub fn events_of(&self, run: &str, kind: &str) -> Vec<Value> {
        self.journal(run)
            .into_iter()
            .filter(|event| event["kind"] == kind)
            .collect()
    }

    /// A file inside a run's directory.
    pub fn run_file(&self, run: &str, relative: &str) -> PathBuf {
        self.runs.join(run).join(relative)
    }

    /// Every entry in a run's dispatch registry, by the file it is in.
    ///
    /// The registry is what says which process each of a run's dispatches is
    /// running in, and it is what the host view decides a row's liveness from —
    /// so a journey about that decision reaches for these rather than for the
    /// run's ownership lock, which names the driver and not the work.
    pub fn dispatch_records(&self, run: &str) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(self.run_file(run, "dispatches"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        found.sort();
        found
    }

    /// A JSON document inside a run's directory.
    pub fn run_json(&self, run: &str, relative: &str) -> Value {
        let path = self.run_file(run, relative);
        serde_json::from_str(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display())),
        )
        .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
    }
}

/// Wait for a child this journey spawned with piped streams to end, draining
/// them as it goes.
///
/// `Child::wait` on a child whose `stdout` or `stderr` is a pipe nothing is
/// reading is not bounded by that child at all: it is bounded by the OS pipe
/// buffer. The child blocks writing once the buffer is full, the waiter blocks
/// waiting for a child that will never exit, and neither ever moves again.
///
/// The size of that buffer is the whole reason this is a harness function rather
/// than a call spelled at each site. Rust's standard library creates its
/// anonymous pipes with an 8192-byte buffer on Windows and inherits the kernel's
/// 65536 on Linux and macOS, so a server that writes 16 KiB passes on both
/// platforms this suite is developed on and deadlocks on the third — silently,
/// with no test named, which is exactly the failure the `cross (windows-latest)`
/// leg cannot afford to produce again. `wait_with_output` reads both streams
/// while it waits, so what bounds the wait is the child's own exit and nothing
/// else.
///
/// What it wrote is discarded: every caller here has already read what it needed
/// off a stream it took, or is ending a server whose output it never claimed.
pub fn ended(child: std::process::Child) {
    child.wait_with_output().expect("the child ends");
}

/// Poll until `ready` holds, reporting whether it did before the deadline.
///
/// Every wait in this suite is on work another process or thread is doing, so
/// the shape is always the same and the deadline is one number: what differs is
/// the evidence a caller prints when it runs out, which is why this answers
/// rather than panicking.
fn waited(ready: impl FnMut() -> bool) -> bool {
    waited_every(std::time::Duration::from_millis(20), ready)
}

fn waited_every(interval: std::time::Duration, ready: impl FnMut() -> bool) -> bool {
    waited_for(std::time::Duration::from_secs(120), interval, ready)
}

fn waited_for(
    duration: std::time::Duration,
    interval: std::time::Duration,
    mut ready: impl FnMut() -> bool,
) -> bool {
    let deadline = std::time::Instant::now() + duration;
    while std::time::Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(interval);
    }
    false
}

/// How many worlds this process still holds.
///
/// The doubles this process linked are shared by all of them, so they are
/// released when the last one goes — never on the first, which
/// [`as_session`](World::as_session) makes a real case.
static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl World {
    /// Stop the runs this world started that can be stopped, before it goes.
    ///
    /// A detached launch retains a driver on purpose — the process that returns
    /// from `start --detach` is not the one driving the run — so a journey that
    /// asserts what it came for and ends mid-run leaves that driver, and the
    /// processes under it, with no test left to own them. On Windows a child
    /// inherits every inheritable handle its parent holds, and one of those is
    /// the pipe `cargo nextest` reads this test's output from: the test exits,
    /// the pipe never reaches EOF, and the test is reported `LEAK`.
    ///
    /// Through the binary's own `stop`, which on either platform walks the run's
    /// descendants and waits until they are gone, rather than a second teardown
    /// here that would walk the tree differently. `--force` because a world seen
    /// through [`as_session`](World::as_session) is ending a run another
    /// session's label owns.
    ///
    /// What it cannot stop it leaves, which is why it promises only the runs it
    /// can: `stop` refuses a run whose dispatch registry it cannot read, and
    /// `driver::a_dispatch_this_run_cannot_record_is_refused_and_does_not_run`
    /// takes that registry away on purpose.
    fn stop_the_runs_it_can(&self) {
        let Ok(entries) = std::fs::read_dir(&self.runs) else {
            return;
        };
        for entry in entries.flatten() {
            // A directory with no launch record is not a run anything is
            // driving — a plan that was refused before it started, or a run
            // whose record a journey removed to see what the views make of it.
            if !entry.path().join("launch.json").is_file() {
                continue;
            }
            let run = entry.file_name();
            let Some(run) = run.to_str() else {
                continue;
            };
            let _ = self.cmd(&["stop", run, "--force"]).output();
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        self.stop_the_runs_it_can();
        let _ = std::fs::remove_dir_all(&self.root);
        // The links this process made, released with the last world that could
        // still have spawned through them, so a suite leaves nothing in the
        // target directory. Best-effort: Windows refuses to unlink a running
        // image, and a directory left behind there costs a directory entry
        // rather than a binary, because these are links and not copies.
        if LIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            let _ = std::fs::remove_dir_all(held_dir());
        }
    }
}

/// `teardown: signalled` is the binary's own evidence: `stop` saying it listed a
/// tree, signalled every process in it, and then watched them go — which
/// `nothing-to-stop` would not, and which is the difference between a run this
/// suite ended and one it merely outlived.
///
/// Started detached, because that is the launch that retains a process: an
/// attached one drives in the command the journey is already waiting on.
// llmlint: ignore-block[tests_mirror_real_usage] the subject is this suite's own
// scaffolding: what a *journey* does at its end is nothing, which is the point — the
// teardown runs from `Drop` and no journey can assert about its own. The verb it runs is
// the one an operator runs, and `driver.rs` drives it as a journey there.
#[test]
fn a_worlds_teardown_ends_a_run_that_is_still_working() {
    let world = World::new("teardown-signals");
    let run = "teardownsignals";
    world.script("build.wait", "hold");
    let plan = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });

    world.stop_the_runs_it_can();

    let stopped = world.events_of(run, "run-stopped");
    assert_eq!(
        stopped.len(),
        1,
        "the teardown left the run's driver running and recorded nothing: {}",
        world.dump()
    );
    assert_eq!(
        stopped[0]["payload"]["teardown"],
        serde_json::json!("signalled"),
        "the teardown found no live tree where the run was still holding a dispatch: {}",
        stopped[0]
    );
} // llmlint: ignore-end[tests_mirror_real_usage]

/// A dump carries what the run directory holds, including a file nothing in
/// this suite names.
///
/// The defect this pins is an absence a reader cannot see: the dump opened
/// `events.jsonl` and four write-back captures by name, so an artifact recorded
/// beside them was invisible — three consecutive dispatches proposed causes for
/// one failure and not one of them read the write-back's own error text, which
/// exists only on a runner. Enumerating the directory is what makes a later
/// artifact reach a reader without this suite being edited for it, so the file
/// written here is deliberately one no code in this repository knows about.
// llmlint: ignore-block[tests_mirror_real_usage] the subject is this suite's own
// diagnostic, and the only way to prove it carries what it has never heard of is to put
// something there it has never heard of. The run under it is a real one, started through
// the binary, and the dump reads the directory that run wrote.
#[test]
fn a_dump_carries_a_file_it_has_never_heard_of_from_the_run_directory() {
    let world = World::new("dump-enumerates");
    let run = "dumpenumerates";
    let plan = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world.run(&["start", &plan, "--attach"]).exited(0).settled();

    // Whatever a producer drops beside the run's own records — here, the shape
    // of a capture a later change might add and this function would not know to
    // open.
    let artifact = "publication-writeback.stderr";
    let text = "onevcs: the write-back refused: no such identity 'origin'";
    std::fs::write(world.runs.join(run).join(artifact), format!("{text}\n"))
        .expect("the run directory takes a file beside its own records");

    let dumped = world.dump();
    assert!(
        dumped.contains(artifact),
        "the dump named no artifact it had not been told about:\n{dumped}"
    );
    assert!(
        dumped.contains(text),
        "the dump named the artifact and carried none of what it holds:\n{dumped}"
    );
    // And the records it always carried are still there, including the ones in
    // the run's own subdirectories.
    assert!(
        dumped.contains("events.jsonl") && dumped.contains("node-settled"),
        "the dump lost the run's own event store:\n{dumped}"
    );
    assert!(
        dumped.contains("launch.json"),
        "the dump lost the record that says who owns the run:\n{dumped}"
    );
} // llmlint: ignore-end[tests_mirror_real_usage]

/// Unix-only for the same reason the other teardown-reading journeys in this
/// suite are — see [`abandonable_hook_script`]: what it asserts is read off the
/// host's own process table, through `kill -0`, and this suite has no Windows
/// spelling of that. What runs everywhere is the teardown itself, and the
/// journey above states its outcome in the binary's own words.
// llmlint: ignore-block[tests_mirror_real_usage] as above: a journey cannot assert what
// is true after itself, and being dropped is the only way this property is ever reached.
#[cfg(unix)]
#[test]
fn a_world_that_is_dropped_while_its_run_is_working_leaves_no_driver_behind() {
    let driver = {
        let world = World::new("teardown-drops");
        let run = "teardowndrops";
        world.script("build.wait", "hold");
        let plan = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
        world.run(&["start", &plan, "--detach"]).exited(0);
        world.until("the run to dispatch something", |world| {
            !world.events_of(run, "node-dispatched").is_empty()
        });
        let driver = world.run_json(run, "launch.json")["pid"]
            .as_u64()
            .expect("the launch record names the driver that claimed the run");
        assert!(
            still_running(driver),
            "the driver was already gone, so this proves nothing about the teardown"
        );
        driver
    };

    assert!(
        !still_running(driver),
        "driver pid {driver} outlived the world that started it — on Windows it holds \
         the runner's own output pipe, which is what a leaky test is"
    );
}

/// `kill -0` delivers nothing and fails once there is no such process, which is
/// the same existence check [`end_process`] waits on.
#[cfg(unix)]
fn still_running(pid: u64) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .expect("this host answers about a process it owns")
        .success()
} // llmlint: ignore-end[tests_mirror_real_usage]

/// A repository `onevcs` knows about, and what its base branch carries.
pub struct Repository {
    /// The bare repository that stands in for the remote.
    pub origin: PathBuf,
    /// The registered execution and publication checkout.
    pub checkout: PathBuf,
}

impl World {
    /// Every change request this world's host was asked to open, in order.
    ///
    /// The host is `gh`, standing in at `onevcs`'s own `ONEVCS_GH` override, and
    /// it records what it was asked for — the title and the body a reviewer
    /// reads among them. That is the far side of a publication, which is the
    /// only place a drafted body is a fact rather than an argument this crate
    /// passed.
    pub fn changes_opened(&self) -> Vec<Value> {
        read_jsonl(&self.fakes.join("gh").join("opened.jsonl"))
    }
}

impl Repository {
    /// What the origin's base branch carries now, newest first.
    pub fn base_commits(&self, world: &World) -> Vec<String> {
        git(world, &self.origin, &["log", "--format=%s", "main"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// One file's contents on the origin's base branch, if it carries one.
    pub fn base_file(&self, name: &str) -> Option<String> {
        let shown = Command::new("git")
            .arg("show")
            .arg(format!("main:{name}"))
            .current_dir(&self.origin)
            .output()
            .expect("git runs");
        shown
            .status
            .success()
            .then(|| String::from_utf8_lossy(&shown.stdout).into_owned())
    }

    /// Whether the registered checkout carries a branch.
    ///
    /// The checkout rather than the origin, because that is where a session
    /// hands its branch back: closing one copies the branch out of the
    /// disposable clone into the checkout, published or not, so this is where a
    /// preserved-but-unpublished workstream survives.
    pub fn has_branch(&self, world: &World, branch: &str) -> bool {
        !git(world, &self.checkout, &["branch", "--list", branch])
            .trim()
            .is_empty()
    }
}

/// Run git in a repository, refusing to continue on anything it rejects.
///
/// Through the world's own git config, like every git `onevcs` runs from here:
/// the origin these journeys build is what the identity is derived from, so it
/// has to be readable under the same settings the session's clone needs.
pub fn git(world: &World, repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", world.gitconfig())
        .env("GIT_AUTHOR_NAME", GIT_WHO)
        .env("GIT_AUTHOR_EMAIL", GIT_EMAIL)
        .env("GIT_COMMITTER_NAME", GIT_WHO)
        .env("GIT_COMMITTER_EMAIL", GIT_EMAIL)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// What one command did.
pub struct Run {
    /// Its exit code.
    pub code: i32,
    /// What it wrote to stdout.
    pub stdout: String,
    /// What it wrote to stderr.
    pub stderr: String,
    /// The arguments it was given, for a failure message that names them.
    pub args: String,
    /// What the world held once it had run, for a failure message that says
    /// *why*. Taken here rather than in the assertion because a command is
    /// often asserted on well after the next one has moved the run on.
    pub world: String,
}

impl Run {
    fn of(output: Output, args: &[&str], world: &World) -> Self {
        Self {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            args: args.join(" "),
            world: world.dump(),
        }
    }

    /// Assert the exit code, reporting what the command said when it differs.
    pub fn exited(&self, code: i32) -> &Self {
        assert_eq!(
            self.code, code,
            "`onepipeline {}` exited {} not {code}\nstdout: {}\nstderr: {}\nthe world held:\n{}",
            self.args, self.code, self.stdout, self.stderr, self.world
        );
        self
    }

    /// Assert stdout contains a fragment.
    pub fn out_has(&self, fragment: &str) -> &Self {
        assert!(
            self.stdout.contains(fragment),
            "`onepipeline {}` stdout lacks {fragment:?}:\n{}",
            self.args,
            self.stdout
        );
        self
    }

    /// Assert stdout does **not** carry a fragment.
    ///
    /// For a journey whose claim is that a view left something out: a record the
    /// store is about to say it lost is not one a reader should be shown, and an
    /// absence is only a claim when something asserts it.
    pub fn out_lacks(&self, fragment: &str) -> &Self {
        assert!(
            !self.stdout.contains(fragment),
            "`onepipeline {}` stdout carries {fragment:?}, which this journey says it \
             should not:\n{}",
            self.args,
            self.stdout
        );
        self
    }

    /// Assert an attached launch reported a settlement rather than failing.
    ///
    /// `start --attach` prints one `{"run_id": …, "settlement": …}` line however
    /// the run settled, so its absence means the launch itself failed — a driver
    /// that could not be started, a ledger that could not be read. Said here,
    /// because the wait that follows a launch in a test can otherwise only fail
    /// as a bare timeout, with the reason on a descriptor nobody kept.
    pub fn settled(&self) -> &Self {
        assert!(
            self.stdout.contains("\"settlement\""),
            "`onepipeline {}` stdout lacks a settlement:\n{}\nstderr:\n{}",
            self.args,
            self.stdout,
            self.stderr
        );
        self
    }

    /// Assert stderr contains a fragment.
    pub fn err_has(&self, fragment: &str) -> &Self {
        assert!(
            self.stderr.contains(fragment),
            "`onepipeline {}` stderr lacks {fragment:?}:\n{}",
            self.args,
            self.stderr
        );
        self
    }

    /// Assert stderr does **not** contain a fragment.
    ///
    /// For a journey whose claim is that one diagnostic reached the caller
    /// *instead of* another: two refusals a command could give are not
    /// interchangeable when only one of them says what to do.
    pub fn err_lacks(&self, fragment: &str) -> &Self {
        assert!(
            !self.stderr.contains(fragment),
            "`onepipeline {}` stderr carries {fragment:?}, which displaces the refusal \
             this journey is about:\n{}",
            self.args,
            self.stderr
        );
        self
    }

    /// The last JSON document it printed on stdout.
    pub fn json(&self) -> Value {
        self.stdout
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
            .unwrap_or_else(|| {
                panic!(
                    "`onepipeline {}` printed no JSON:\n{}",
                    self.args, self.stdout
                )
            })
    }
}

/// The compiled binary under test, resolved by cargo rather than by PATH.
pub fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_onepipeline"))
}

/// One of the sibling doubles, pinned to a name only this process holds.
///
/// The doubles live in a separate workspace member so they can never ship, and a
/// package-scoped build — `cargo llvm-cov` runs one — does not build another
/// member's binaries. So the fixture builds itself.
///
/// Cargo is the freshness check, not the file's existence: a double whose source
/// changed has to be rebuilt, and a harness that stopped at "the binary is
/// there" would silently run last week's fixture against this week's test. The
/// build happens once per process; `.config/nextest.toml` bounds how many run at
/// a time, and cargo's own lock serialises those.
///
/// The path handed back is **never** the one cargo publishes. Every test runs in
/// its own process and each builds the doubles, so several cargo invocations
/// uplift the same `target/debug/<name>` — and an uplift is a remove followed by
/// a replacement, so that name is briefly absent. A test that had already looked
/// would then spawn a binary that vanished under it: on macOS that surfaced both
/// as a missing double and, worse, as a whole run stuck at `run-started` because
/// the launcher could not start its driver. Giving each process its own name for
/// the binary closes the window, because a later uplift replaces the *name* it
/// published and never touches this one. See [`held_alias`] for why that name is
/// a link rather than a copy wherever the filesystem allows one.
pub fn double(name: &str) -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let held = BUILT.get_or_init(|| build(&["--package", "onepipeline-testfakes"]));
    held_alias(held, name)
}

/// Who a commit a journey's `onevcs` makes is attributed to.
///
/// Carried in the environment on every command, because a session's clone
/// inherits no local config and a global one is the operator's, not this
/// suite's.
pub const GIT_WHO: &str = "onepipeline e2e";

/// The address that attribution carries. Reserved by RFC 2606, so it can never
/// reach anybody.
pub const GIT_EMAIL: &str = "e2e@onepipeline.invalid";

/// The **real** `oneagentgraph` binary, built from the version this crate
/// depends on.
///
/// Not the one on `PATH`: that is whatever an operator happened to install, and
/// a journey proving this crate composes its sibling has to compose the sibling
/// `Cargo.lock` pins. Cargo builds a dependency's binary the same way it builds
/// a workspace member's, so this needs no extra provisioning — the library is
/// already compiled by the time a test runs.
pub fn oneagentgraph_binary() -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let held = BUILT.get_or_init(|| {
        build(&[
            "--package",
            "oneagentgraph",
            "--bin",
            "oneagentgraph",
            "--locked",
        ])
    });
    held_alias(held, "oneagentgraph")
}

/// The project every store fixture here holds beside the plan under test.
pub const DECOY_PROJECT: &str = "not-this-runs-board";

/// The identifier a store holds the plan named `name` under.
///
/// Deliberately not the plan's own name, which is the project's *title*: a store
/// item's id is the store's and a heading is a person's, and the two coincide
/// only in a fixture that could not tell them apart.
pub fn project_id(name: &str) -> String {
    format!("{name}-board")
}

/// Why a store fixture could not tell a right answer from a wrong one, or `None`
/// when it can.
///
/// Two rules, checked where the fixture is built rather than trusted to be
/// remembered per journey, because each is a defect that shipped through a
/// worker, a judge, a monitor and a manager:
///
/// 1. **A destination whose identifier differs from its title.** Every fixture
///    here used to hold a project whose native id *was* its file stem *was* its
///    title, so writing the id as the title was byte-identical to correct.
/// 2. **Two of what the code under test filters on.** Every fixture held one
///    project, so an ignored `--project` was indistinguishable from an honoured
///    one.
///
/// Named for the failure rather than for the property, so the answer that means
/// "this is fine" is the falsy one and a caller cannot forget to negate it.
///
/// A store this cannot *read* is refused too, and by name. A rule that answered
/// "fine" for a directory entry it could not stat, a document it could not open,
/// or front matter it could not parse would pass exactly the fixtures nobody can
/// check.
pub fn undiscriminating(store: &Path) -> Option<String> {
    let fixture = store
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| store.display().to_string());
    let entries = match std::fs::read_dir(store.join("projects")) {
        Ok(entries) => entries,
        Err(error) => {
            return Some(format!(
                "fixture '{fixture}': its projects directory could not be read ({error}), so it \
                 is not a store at all"
            ))
        }
    };
    let mut projects: Vec<(String, String)> = Vec::new();
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                return Some(format!(
                    "fixture '{fixture}': one of its project entries could not be read ({error})"
                ))
            }
        };
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        let identifier = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let title = match settled_title(&path) {
            Ok(title) => title,
            Err(why) => return Some(format!("fixture '{identifier}': {why}")),
        };
        projects.push((identifier, title));
    }
    if projects.len() < 2 {
        return Some(format!(
            "fixture '{fixture}': holds {} project, so a read that ignored the project filter \
             cannot be told from one that honoured it",
            projects.len()
        ));
    }
    for (identifier, title) in &projects {
        if identifier == title {
            return Some(format!(
                "fixture '{identifier}': its identifier is its own title, so writing the \
                 identifier and preserving a person's heading are the same bytes"
            ));
        }
    }
    None
}

/// One project document's title, read once the store has finished writing it.
///
/// **Read again rather than believed the first time**, because the store this reads is
/// one live runs write to. A run projects its nodes back onto the store it launched
/// from by handing `onetaskgraph` a `project copy`, and that store's writer replaces a
/// document in place — it truncates the file and then writes it again — so a read taken
/// between those two halves is an empty document with no front matter on it. Every
/// journey that launches a second run against a store an earlier run of the same
/// journey is still settling can take one, and read once, a sound fixture is reported
/// as one that could prove nothing: the failure names the *previous* run's board and
/// looks nothing like the timing it is.
///
/// A document that never settles is still a defect, and is still named by the same four
/// reasons — the wait only decides how long "never" is. It costs nothing in the ordinary
/// case, where the first read answers, and the budget once for a fixture that really is
/// malformed.
fn settled_title(path: &Path) -> Result<String, String> {
    let mut settled = None;
    let mut last = "its project document was never read".to_owned();
    waited_for(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(20),
        || match title_of(path) {
            Ok(title) => {
                settled = Some(title);
                true
            }
            Err(why) => {
                last = why;
                false
            }
        },
    );
    settled.ok_or(last)
}

/// The four ways one read of a project document answers with no title.
///
/// Each is both a way a fixture is malformed *and* a state the store's writer passes
/// through mid-replacement, which is why [`settled_title`] cannot tell them apart by
/// shape and asks again instead.
fn title_of(path: &Path) -> Result<String, String> {
    let document = std::fs::read_to_string(path)
        .map_err(|error| format!("its project document could not be read ({error})"))?;
    let front = document
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("---\n"))
        .map(|(front, _)| front)
        .ok_or_else(|| "its project document has no front matter".to_owned())?;
    let front = serde_norway::from_str::<serde_json::Map<String, Value>>(front)
        .map_err(|error| format!("its front matter is not a mapping ({error})"))?;
    front
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            "its project states no title, so there is nothing for an identifier to differ from"
                .to_owned()
        })
}

/// The reserved metadata key one plan field rides on.
///
/// A journey that spells the whole key means it literally — which is what a
/// journey about a key the mapping refuses has to be able to say — and one that
/// names a plain field means that field.
fn reserved(field: &str) -> String {
    match field.starts_with("onepipeline.") {
        true => field.to_owned(),
        false => format!("onepipeline.{field}"),
    }
}

/// Where an executable of this name sits on **this process's** `PATH`.
///
/// The resolution the operating system would do, done once and kept, so a child
/// this suite hands a different `PATH` still reaches the same program.
fn on_path(name: &str) -> Option<PathBuf> {
    let executable = |dir: PathBuf| {
        [name.to_owned(), format!("{name}.exe")]
            .into_iter()
            .map(|file| dir.join(file))
            .find(|candidate| candidate.is_file())
    };
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(executable)
}

/// The environment variable naming the `onetaskgraph` executable.
///
/// Restated here rather than imported: the crate under test declares it in a
/// module `src/lib.rs` keeps private, so there is no item to name.
///
/// What reconciles the two spellings is
/// `store::an_absent_onetaskgraph_refuses_the_launch_and_starts_nothing`, which
/// points this key at a path that does not exist and requires the launch to
/// **refuse, naming that path**. A spelling that drifted would leave the binary
/// under test ignoring the key, resolving whatever the host installed, and
/// launching successfully — so that journey fails rather than passing against a
/// key nothing reads. Every use of the key in this suite goes through this
/// constant, which is what makes the one journey cover them all.
pub const STORE_BINARY_ENV: &str = "ONETASKGRAPH_BIN";

/// The name of the one source every world configures.
///
/// A qualified project id is `<source>:<native>`, so this is the first half of
/// every id a journey launches. A source name may not contain an underscore,
/// which is what makes `onetaskgraph`'s environment layer unambiguous.
pub const STORE_SOURCE: &str = "plans";

/// The **real** `onetaskgraph` executable every journey reads its plan through.
///
/// Not a double, and deliberately: a plan is one project of that store, so a
/// journey that launched from a stand-in would prove the fixture rather than the
/// mapping. It is not a Cargo dependency either — this crate *drives* the binary
/// rather than linking it — so cargo cannot build it and the suite resolves what
/// the host installed: `ONETASKGRAPH_BIN` when that names one, and the name on
/// the `PATH` otherwise.
///
/// A host without one **fails**, naming what to run. A skip would be a pass
/// nobody earned, and this is not the credentialled tier: the binary reads a
/// folder of Markdown with no network and no account.
pub fn onetaskgraph_binary() -> PathBuf {
    static FOUND: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            // Absolute, and resolved here rather than left to the child: a
            // journey may hand the binary under test a `PATH` of its own — one
            // that leads with a sibling's directory, or one that is empty — and
            // the store it reads its plan through is not what those journeys are
            // about.
            let named = std::env::var("ONETASKGRAPH_BIN")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .or_else(|| on_path("onetaskgraph"))
                .unwrap_or_else(|| PathBuf::from("onetaskgraph"));
            let reported = Command::new(&named).arg("--version").output();
            match reported {
                Ok(output) if output.status.success() => named,
                other => panic!(
                    "the e2e suite reads every plan through the real onetaskgraph and {} \
                     could not be run ({other:?}) — run `just bootstrap`, or set \
                     ONETASKGRAPH_BIN to an executable one",
                    named.display()
                ),
            }
        })
        .clone()
}

/// The **real** `onetaskgraph-source` program: the shipped host that serves any plugin of
/// that build over the stdio plugin protocol.
///
/// Installed beside `onetaskgraph` by the same `cargo install` that `just bootstrap` runs,
/// so it is resolved beside the binary this suite already found rather than provisioned a
/// second time. A host without one **fails**, naming what to run, for the same reason
/// [`onetaskgraph_binary`] does: it reads a folder of Markdown, with no network and no
/// account, and a skip would be a pass nobody earned.
pub fn onetaskgraph_source_binary() -> PathBuf {
    static FOUND: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            let store = onetaskgraph_binary();
            let beside = store.parent().and_then(|directory| {
                ["onetaskgraph-source", "onetaskgraph-source.exe"]
                    .into_iter()
                    .map(|file| directory.join(file))
                    .find(|candidate| candidate.is_file())
            });
            let named = beside.or_else(|| on_path("onetaskgraph-source"));
            named.unwrap_or_else(|| {
                panic!(
                    "a label-strict destination is served by the real onetaskgraph-source,                      which is installed beside {} — run `just bootstrap`",
                    store.display()
                )
            })
        })
        .clone()
}

/// The released `onevcs` executable whose holders verb the launcher consumes.
pub fn onevcs_binary() -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let held = build(&["--package", "onevcs", "--bin", "onevcs", "--locked"]);
            held_alias(&held, "onevcs")
        })
        .clone()
}

/// A pid this host can prove is gone: a real process, started and reaped.
///
/// Picked out of the air it would not be one — the kernel may have handed it to
/// something else — and every journey about a record that names a driver which
/// is no longer there turns on the difference.
pub fn reaped_pid() -> u32 {
    let mut child = std::process::Command::new(binary())
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary starts");
    let pid = child.id();
    child.wait().expect("it exits");
    pid
}

/// End one process this suite is entitled to end, and wait until it is gone.
///
/// Forcefully, because both things it is used on are processes a polite ask does
/// not settle: a dispatch scripted to keep working through `SIGTERM`, and a
/// run's driver which a journey needs *gone* without the tree it started going
/// with it, which is the state an adoption recovers from.
///
/// The wait is what makes this usable as a precondition. `kill` returns when the
/// signal is queued, not when the process has exited, so a journey that went
/// straight on from here would be asserting against a host that had not caught
/// up with it yet.
#[cfg(unix)]
pub fn end_process(pid: u32) {
    std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .expect("this host ends a process it owns");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        // `kill -0` is the existence check: it delivers nothing and fails once
        // there is no such process.
        let still_there = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .expect("this host answers about a process it owns")
            .success();
        if !still_there {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("pid {pid} outlived the one ask no process can ignore");
}

/// This host's own `ps`, found the way a shell finds it.
///
/// Resolved here rather than written down, because it is `/bin/ps` on some hosts
/// and `/usr/bin/ps` on others, and a stand-in that shadowed the name would
/// recurse into itself if it called `ps` by name.
#[cfg(unix)]
fn real_ps() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("ps"))
        .find(|candidate| candidate.is_file())
        .expect("this host has a ps")
}

/// A `ps` stand-in that answers the real thing, with `row` added to the process
/// **listing** and to nothing else.
///
/// The guard is the whole point. `ps` is asked two different questions here — for
/// the table a teardown walks (`-A`), and for when one process started, which is
/// what a recorded pid is proved against — and a stand-in that wrote its row into
/// both would be two faults wearing one name: the journey would still refuse, but
/// over a host that could not describe a *process*, which is a different journey
/// with a different verdict. So the fault is scoped to the question it is about.
#[cfg(unix)]
fn listing_plus(row: &str) -> String {
    ps_plus("*\" -A \"*", row)
}

/// The other half of that pair: `row` added to what this host says about **one**
/// process, and to nothing else, so the listing a teardown walks stays good.
#[cfg(unix)]
fn answer_plus(row: &str) -> String {
    ps_plus("*lstart=*", row)
}

/// A `ps` stand-in that answers the real thing, with `row` written ahead of the
/// answers whose arguments match `question` and no others.
#[cfg(unix)]
fn ps_plus(question: &str, row: &str) -> String {
    format!(
        "case \" $* \" in\n  {question}) echo '{row}' ;;\nesac\nexec {} \"$@\"",
        real_ps().display()
    )
}

/// A `PATH` with `dirs` ahead of the one this process inherited.
///
/// Every program this stack resolves *by name* has to resolve to the build
/// `Cargo.lock` pins rather than to whatever an operator installed — otherwise a
/// journey composing a sibling proves only that the host had one, and says
/// nothing about the version this crate is written against. The inherited `PATH`
/// still follows, because git and the platform's own tools live on it.
fn path_leading_with(dirs: &[PathBuf]) -> std::ffi::OsString {
    std::env::join_paths(dirs.iter().cloned().chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("a PATH")
}

/// Where this process holds its own names for the binaries it resolves.
///
/// Named for the process rather than shared, because the name is what an uplift
/// replaces — see [`held_alias`]. Removed by the last [`World`] this process
/// drops, so a suite leaves the target directory as it found it.
fn held_dir() -> PathBuf {
    binary()
        .parent()
        .expect("the binary is in a directory")
        .join("doubles")
        .join(std::process::id().to_string())
}

/// Build one package's binaries into the target directory this test's own binary
/// came out of, and return the per-process directory they are held in.
///
/// The one wait in this harness that is deliberately left unbounded. Every test
/// process needing a double runs this, so several of them queue on cargo's lock
/// over the shared target directory and cargo says so — `Blocking waiting for
/// file lock on build directory` — and then waits as long as it takes. A
/// deadline here would end whichever process drew the short straw, on a host
/// that was working the whole time, and it would do it more often the more
/// parallel the runner is. What bounds it instead is the runner: `slow-timeout`
/// in `.config/nextest.toml` names the test that is waiting and then ends it, so
/// a lock nothing is going to release reads as one named test rather than as a
/// suite that stopped.
fn build(selection: &[&str]) -> PathBuf {
    let debug = binary()
        .parent()
        .expect("the binary is in a directory")
        .to_path_buf();
    let target = debug
        .parent()
        .expect("the profile directory is inside a target directory");
    let built = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["build", "--offline"])
        .args(selection)
        .arg("--target-dir")
        .arg(target)
        .current_dir(repo_file("."))
        .output()
        .expect("cargo builds the subprocess doubles");
    assert!(
        built.status.success(),
        "{selection:?} did not build: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let held = held_dir();
    std::fs::create_dir_all(&held).expect("a directory for this process's doubles");
    held
}

/// This process's own name for a built binary, made once and kept.
///
/// A link rather than a copy, and the difference is not an optimisation. What
/// this name has to survive is a concurrent cargo uplift, which is an unlink of
/// `target/debug/<name>` followed by a replacement — so what a spawn needs is
/// the *inode* held open under a name nothing else replaces. A hard link is
/// exactly that, at zero bytes: the suite's own measurement was 162 copies of
/// one double, 271 MB per process directory and 25 GB across a single coverage
/// run, which is what exhausted the disk on the host that supervises these runs.
///
/// A **link** wherever the filesystem allows one, and a copy only where it does
/// not — a target directory on a mount without hard links, or across a device
/// boundary. Both give the same guarantee; only one of them scales, which is
/// why the name says alias rather than promising the link.
fn held_alias(held: &Path, name: &str) -> PathBuf {
    let debug = binary()
        .parent()
        .expect("the binary is in a directory")
        .to_path_buf();
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let published = debug.join(&file);
    let mine = held.join(&file);
    if !mine.is_file() {
        // Here rather than only in [`build`], which runs once per process: the
        // last world a process drops takes this directory with it, and a test
        // that opens a second world afterwards would otherwise link into a
        // directory that is no longer there — an ENOENT the retry below cannot
        // outlast, because nothing is going to recreate it.
        std::fs::create_dir_all(held).expect("a directory for this process's doubles");
        // Bounded, and retried rather than asserted on the first look: the
        // window this closes is another process's uplift, which is short but
        // real, and a bare `is_file` here would only move the flake one line up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let _ = std::fs::remove_file(&mine);
            match place(&published, &mine) {
                Ok(()) => break,
                Err(error) => assert!(
                    std::time::Instant::now() < deadline,
                    "the {name} double never appeared in {}: {error}",
                    debug.display()
                ),
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    mine
}

/// How many staging names this process has spent placing doubles.
///
/// Per attempt rather than per name: the retry above may place the same double
/// twice, and a staging file left behind by an attempt that failed must not be
/// the one the next attempt writes.
static STAGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Put one built binary at the name this process holds it under, whole or not at
/// all.
///
/// The hard link is atomic by construction — the name either resolves or it does
/// not — and it stays the fast path for the reason [`held_alias`] gives. The copy
/// behind it is *not* atomic, and writing it straight to `mine` leaves that name
/// resolving to a **partial** binary for as long as the copy takes: a world that
/// execs the double in that window reads a truncated image and gets
/// `Exec format error (os error 8)`, which was reported from a real dispatch in
/// three tests on a cold build. It reads as a broken double rather than as a
/// race, and the retry loop above cannot catch it — the file exists while it is
/// partial, so the copy returns `Ok` and the loop breaks.
///
/// So the copy lands on a name of this process's own **in the destination's own
/// directory** and is renamed into place. Rename is atomic within a filesystem,
/// and staging beside the destination is what keeps it one: a reader watching
/// `mine` sees nothing, the old file, or a complete new one.
fn place(published: &Path, mine: &Path) -> std::io::Result<()> {
    if std::fs::hard_link(published, mine).is_ok() {
        return Ok(());
    }
    let staging = mine.with_file_name(format!(
        "{}.staging-{}-{}",
        mine.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        STAGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    std::fs::copy(published, &staging)?;
    std::fs::rename(&staging, mine).inspect_err(|_| {
        // A rename that did not happen leaves the staging file next to the
        // destination, where the next `build` would sweep nothing: the directory
        // is this process's own and lives as long as it does.
        let _ = std::fs::remove_file(&staging);
    })
}

/// A double is never visible half-written at the name a world execs it under.
///
/// The window is [`place`]'s copy fallback, and it is only observable while the
/// copy runs — so this watches the destination while a file too big to be copied
/// in one write is placed over one that is already there, and asserts that every
/// observation is one of three things: no file, the whole of the old one, or the
/// whole of the new one. A length that is neither is the truncated image a
/// concurrent exec reads as `Exec format error (os error 8)`.
///
/// The destination exists before the placement, which is what sends this down
/// the fallback: `hard_link` refuses a name that already resolves. The fast path
/// is what every other journey in this suite takes.
// llmlint: ignore-block[tests_mirror_real_usage] the subject is this suite's own
// scaffolding rather than a journey: `place` is how a test process gets a double onto a
// name it can spawn, and it is reachable from no interface a user of this crate has. What
// it has to be true of is a property of the placement itself — that no concurrent reader
// of the destination ever sees a partial — and the only way to observe that is to watch
// the destination while the placement runs. The journeys that then *spawn* the doubles
// are every other test in this directory, all of which drive the compiled binary.
#[test]
fn a_double_is_placed_whole_or_not_at_all() {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

    let dir = std::env::temp_dir().join(format!("onepipeline-place-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory for the placement");

    // Big enough that copying it is many writes rather than one. A small file
    // would be placed inside a single write and would prove nothing about a
    // binary, which is the size that made this a real failure.
    let replacement = vec![b'N'; 64 << 20];
    let replaced = b"the double this one replaces".to_vec();
    let published = dir.join("double.published");
    let mine = dir.join("double");
    std::fs::write(&published, &replacement).expect("the built double is written");
    std::fs::write(&mine, &replaced).expect("the double this one replaces is written");

    let watching = Arc::new(AtomicBool::new(true));
    let stop = Arc::clone(&watching);
    let looked = Arc::new(AtomicU64::new(0));
    let looks = Arc::clone(&looked);
    let destination = mine.clone();
    let whole = [replaced.len() as u64, replacement.len() as u64];
    let reader = std::thread::spawn(move || {
        let mut partial: Option<u64> = None;
        while stop.load(Ordering::Relaxed) {
            looks.fetch_add(1, Ordering::Relaxed);
            // A name that does not resolve is one of the three: a reader that
            // finds nothing there looks again, and never execs a fragment.
            if let Ok(seen) = std::fs::metadata(&destination) {
                if !whole.contains(&seen.len()) {
                    partial = partial.or(Some(seen.len()));
                }
            }
        }
        partial
    });

    // The window this journey is about is the one *during* the placement, and a
    // thread the scheduler has not run yet is not in it: wait for the reader to
    // be looking before the placement starts, rather than spawning it and
    // trusting the host to have started it in time. It did not, on macOS.
    assert!(
        waited(|| looked.load(Ordering::Relaxed) > 0),
        "the reader never looked at the destination"
    );
    place(&published, &mine).expect("the double is placed");
    watching.store(false, Ordering::Relaxed);
    let partial = reader.join().expect("the reader ends");

    assert_eq!(
        partial,
        None,
        "a reader saw {} byte(s) at {} while it was being placed — neither the old \
         double ({}) nor the new one ({})",
        partial.unwrap_or_default(),
        mine.display(),
        whole[0],
        whole[1]
    );
    assert_eq!(
        std::fs::metadata(&mine).expect("the placed double").len(),
        replacement.len() as u64,
        "the placement did not leave the new double behind"
    );

    let _ = std::fs::remove_dir_all(&dir);
} // llmlint: ignore-end[tests_mirror_real_usage]

/// A verb the repository's own `pre-push` hook answers **and returns from on its
/// own**.
///
/// A closed set rather than the free argv this used to be handed: the hook's third
/// verb, `wait-for`, blocks until something writes its rendezvous, and a journey
/// that means to hold a push states [`held_publication`] or
/// [`abandonable_hook_script`] instead of spelling one.
pub enum ReturningHookVerb {
    /// Leave a file where the session's stream directory was, so the next read of
    /// that stream meets a broken store rather than a missing one.
    BreakStreams,
    /// Append a line to this session's stream that no build of `onevcs` can read.
    AppendFutureEvent,
}

impl ReturningHookVerb {
    /// The word both halves of the hook dispatch on.
    fn word(&self) -> &'static str {
        match self {
            Self::BreakStreams => "break-streams",
            Self::AppendFutureEvent => "append-future-event",
        }
    }
}

/// A repository's own `pre-push` hook running `verb`, written into this world as
/// a script and handed back as the argv that runs it.
///
/// Available on every platform, because every verb reachable through here runs
/// and returns on its own: git waits for it, the push it declines or lets through
/// is over by the time the journey looks, and nothing is left holding a pipe.
pub fn hook_script(world: &World, verb: &ReturningHookVerb) -> Vec<String> {
    hook_argv(world, &[verb.word()])
}

/// A `pre-push` hook **holding** the publishing push, and the release that ends
/// the hold however the journey ends.
///
/// [`Drop`] covers abandonment — the rendezvous is written on the assertion that
/// failed, the [`World::until`] that ran out, the panic that unwound past both —
/// and the ceiling `hook.sh` states covers the deadlock it cannot reach, a
/// journey that waits for the publication to finish before its own scope ends.
/// Between them a held push cannot outlive its journey on any platform.
///
/// Declare it *after* the [`World`], so it drops *before* the world removes the
/// tree the rendezvous lives in. Holding the value is not optional and the
/// compiler says so: [`argv`](Self::argv) borrows from `self`, so a temporary
/// cannot reach [`World::repository`].
pub struct HeldPublication {
    argv: Vec<String>,
    rendezvous: PathBuf,
}

impl HeldPublication {
    /// The argv [`World::repository`] installs as the repository's own hook.
    pub fn argv(&self) -> Vec<&str> {
        self.argv.iter().map(String::as_str).collect()
    }

    /// Let the held push through, now, because this journey has seen what it
    /// needed to while the publication was still running.
    pub fn release(&self) {
        std::fs::write(&self.rendezvous, "go").expect("the held publication is released");
    }
}

impl Drop for HeldPublication {
    fn drop(&mut self) {
        // Unconditional, and best-effort: a journey that already released writes
        // the same bytes twice, and one that panicked before releasing is exactly
        // the case this exists for. Failure here means the world's tree is already
        // gone, which means so is the hook.
        let _ = std::fs::write(&self.rendezvous, "go");
    }
}

/// A merge path that holds the publishing push until this journey lets it
/// through. See [`HeldPublication`].
pub fn held_publication(world: &World, rendezvous: &Path) -> HeldPublication {
    HeldPublication {
        argv: hook_argv(world, &["wait-for", &rendezvous.to_string_lossy()]),
        rendezvous: rendezvous.to_path_buf(),
    }
}

/// An abandoned hold still lets the push through.
///
/// This is half of why the journeys that hold a publication run on every
/// platform: not that they release, but that they cannot fail to. Delete the
/// [`Drop`] and three of them go back to leaving a hook waiting on a rendezvous
/// nobody writes, which off Unix nothing reaps — and nothing else in the suite
/// would say so, because on Unix the run's own group teardown covers for it. The
/// other half is the hook's own ceiling, which bounds even a hold this cannot
/// reach; `hook.sh` states it, and
/// [`a_held_push_nothing_releases_expires_rather_than_waiting_out_the_job`]
/// drives it.
// llmlint: ignore-block[tests_mirror_real_usage] this holds the suite's own scaffolding,
// not a journey: the property is what happens when a journey is abandoned, which no
// journey can assert about itself. The journeys built on it are `lifecycle.rs`'s and
// `views.rs`'s, and those do drive the binary.
#[test]
fn an_abandoned_hold_is_released_anyway() {
    let world = World::new("held-abandoned");
    let go = world.fakes.join("push.go");
    let abandoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _held = held_publication(&world, &go);
        assert!(
            !go.exists(),
            "the rendezvous was written while the push was supposed to be held"
        );
        panic!("a journey that ends before it releases");
    }));

    assert!(abandoned.is_err(), "the abandonment did not happen");
    assert!(
        go.is_file(),
        "an abandoned hold left {} unwritten, so the hook waiting on it outlives the \
         journey — which is what wedges a platform that cannot reap one",
        go.display()
    );
} // llmlint: ignore-end[tests_mirror_real_usage]

/// The same hold, as a bare argv with **no release attached** — for a journey
/// that abandons its hook on purpose and asserts on what the host leaves behind.
///
/// **Unix-only for what those journeys assert**, not because a hold is unsafe off
/// Unix: they read the teardown itself — `ps` and process groups in
/// `session_reuse.rs`, the Unix-only `end_process` in `driver.rs` — and neither
/// has a Windows spelling. Anything that merely *holds* a push wants
/// [`held_publication`], which runs everywhere; the release on drop it has would
/// let the push through before these have read what an abandoned hold leaves.
#[cfg(unix)]
pub fn abandonable_hook_script(world: &World, rendezvous: &Path) -> Vec<String> {
    hook_argv(world, &["wait-for", &rendezvous.to_string_lossy()])
}

/// The hook script for this platform, written into this world's own scratch, with
/// `args` behind it.
///
/// A script rather than a compiled binary, and per platform rather than one
/// artifact, because the alternative was a workspace member shipping a Rust
/// program to stand in for three shell one-liners. The argv is what
/// [`install_hook`] puts behind the hook git executes, and on Windows nothing can
/// start a `.bat` directly, so each platform's interpreter leads its own.
///
/// Private, and free-form only in here: the two public doors above are what a
/// journey states a merge path with, and the one caller that needs a command line
/// neither of them can spell is [`the_hook_refuses_a_command_line_it_does_not_speak`],
/// which is about the argv the hook *turns down*.
fn hook_argv(world: &World, args: &[&str]) -> Vec<String> {
    let mut argv = interpreted(&write_hook_script(world));
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
    argv
}

/// What the hook's POSIX line has to say before it hands the process over, so
/// that the verb's argv arrives as it was written. See [`install_hook`].
#[cfg(windows)]
const VERBATIM_ARGUMENTS: &str =
    "MSYS2_ARG_CONV_EXCL='*'\nMSYS_NO_PATHCONV=1\nexport MSYS2_ARG_CONV_EXCL MSYS_NO_PATHCONV\n";

/// Nothing: no runtime stands between this shell and the verb it starts.
#[cfg(not(windows))]
const VERBATIM_ARGUMENTS: &str = "";

/// Where a world keeps the repository's own hooks, which is what its checkout's
/// `core.hooksPath` names.
///
/// One statement of it, so the journey that installs a hook and the drift gate
/// that rewrites one are naming the same directory rather than agreeing by hand.
fn hooks_dir(world: &World) -> PathBuf {
    world.root.join("hooks")
}

/// Write `argv` at `path` as an executable `pre-push` hook.
///
/// git runs a hook as a program of its own, so the argv a journey states is
/// wrapped rather than written out: one POSIX line that hands the process over to
/// it, which is what git for Windows runs its hooks with too. `exec` rather than a
/// call, so what git waits on — and what its refusal reports — is the verb itself
/// and never a shell that outlived it.
///
/// Every word is single-quoted, because a world's scratch directory carries the
/// journey's own name and nothing promises that a path is one shell word.
///
/// On Windows that POSIX line is read by the MSYS2 `sh` git for Windows bundles,
/// and it is [`interpreted`] that puts `cmd /C` in front of the verb — so the
/// shell starts a *native* program, and the MSYS2 runtime rewrites arguments
/// shaped like POSIX paths on the way across that boundary. `/C` is exactly that
/// shape, and rewritten it stops being cmd's switch: cmd reads the verb as a
/// command line rather than running it as a batch file, and the push is left in a
/// hook that never returns. So [`VERBATIM_ARGUMENTS`] switches the conversion off
/// for the one `exec` below, under both names, because `MSYS_NO_PATHCONV` is git
/// for Windows' own spelling of the MSYS2 knob and neither release promises the
/// other. Empty on Unix, where the line is what it always was.
fn install_hook(path: &Path, argv: &[&str]) {
    let quoted: Vec<String> = argv
        .iter()
        .map(|word| format!("'{}'", word.replace('\'', r"'\''")))
        .collect();
    std::fs::write(
        path,
        format!(
            "#!/bin/sh\n{}exec {}\n",
            VERBATIM_ARGUMENTS,
            quoted.join(" ")
        ),
    )
    .expect("the pre-push hook is written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("the pre-push hook is executable");
    }
}

/// The two halves of the hook answer the same verbs.
///
/// One contract in two languages, because no platform runs both — so a verb
/// added to one and not the other is a journey that passes here and hangs on the
/// Windows leg, a fortnight later, with nothing pointing at the hook. Held by
/// reading the scripts rather than by generating one from the other: a generator
/// would need a third source, and what actually has to agree is the verb each
/// half dispatches on.
// llmlint: ignore-block[tests_mirror_real_usage] this is a drift gate over the suite's own
// scaffolding, not a journey: what it holds is that two files stay in step, and neither
// file is reachable from any interface a user of this crate has. There is nothing to drive
// through the binary here — the hook is the repository's own, which git runs at the
// publishing push, and the journeys that exercise it are `lifecycle.rs`'s and `views.rs`'s,
// which do drive the binary. Reading the two scripts is the only way to compare them,
// because no platform executes both.
#[test]
fn both_hook_scripts_answer_the_same_verbs() {
    let verb = |candidate: &str| {
        !candidate.is_empty()
            && candidate
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-')
    };
    // `sh`'s `case` arms, which are the only lines ending in a bare `)` whose
    // whole body is a verb — `*)` and every expression that happens to close a
    // parenthesis are filtered out by the alphabet.
    let shell: Vec<&str> = include_str!("hook.sh")
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_suffix(')'))
        .filter(|candidate| verb(candidate))
        .collect();
    // `cmd`'s dispatch, which is one `if "%~1"=="VERB" goto …` per verb.
    let batch: Vec<&str> = include_str!("hook.bat")
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("if \"%~1\"==\""))
        .filter_map(|rest| rest.split('"').next())
        .filter(|candidate| verb(candidate))
        .collect();
    assert_eq!(
        shell, batch,
        "the hook scripts have drifted: one platform answers verbs the other does not"
    );
    // The extraction is only a drift gate while it still finds anything: a
    // rewrite that changed either dispatch's shape would otherwise compare two
    // empty lists and pass.
    assert_eq!(
        shell,
        ["wait-for", "break-streams", "append-future-event"],
        "the hook scripts no longer dispatch the way this reads them"
    );
    // The ceiling `wait-for` gives up at is the other thing two files have to
    // agree on: a half that waited longer would hang the leg the bound exists to
    // keep off, and one that waited less would refuse pushes its journeys are
    // still holding. Read as the default each half writes, because the
    // environment can move it and neither half may disagree about what it moves.
    let ceiling = |script: &str, source: &str| -> String {
        source
            .lines()
            .map(str::trim)
            .filter_map(|line| line.split_once("seconds=").map(|(_, rest)| rest))
            .map(|rest| {
                rest.chars()
                    .take_while(|character| character.is_ascii_digit())
                    .collect::<String>()
            })
            .find(|digits| !digits.is_empty())
            .unwrap_or_else(|| panic!("{script} states no default ceiling for wait-for"))
    };
    assert_eq!(
        ceiling("hook.sh", include_str!("hook.sh")),
        ceiling("hook.bat", include_str!("hook.bat")),
        "the hook scripts give a held push up at different ceilings"
    );
    assert_eq!(
        ceiling("hook.sh", include_str!("hook.sh")),
        "300",
        "the ceiling moved: `hook.sh` says why it is 300 seconds, and a journey \
         that holds a push for longer than that is a journey this suite does not have"
    );
    // The refusal names the verbs, so it is a third statement of the same list
    // and drifts the same way — and it is the one a person reads when the hook
    // has just refused them, so a verb missing from it is worse than a verb
    // missing from a comment.
    for (script, source) in [
        ("hook.sh", include_str!("hook.sh")),
        ("hook.bat", include_str!("hook.bat")),
    ] {
        let usage = source
            .lines()
            .find(|line| line.contains("the verbs are:"))
            .unwrap_or_else(|| panic!("{script} refuses without naming the verbs it speaks"));
        for verb in &shell {
            assert!(
                usage.contains(verb),
                "{script} dispatches {verb} but its refusal does not name it: {usage}"
            );
        }
    }
} // llmlint: ignore-end[tests_mirror_real_usage]

/// The two halves of the release probe fill in the same placeholders.
///
/// Same reason the hook's halves are held in step: no platform runs both, so a
/// placeholder added to one and not the other is a journey that passes here and
/// answers nothing on the other leg — a probe writing the literal `@RUNS_FILE@`
/// into the tree instead of a tally. [`World::probe_in`] substitutes exactly this
/// set.
// llmlint: ignore-block[tests_mirror_real_usage] a drift gate over the suite's own
// scaffolding rather than a journey, exactly as
// [`both_hook_scripts_answer_the_same_verbs`] is: no platform executes both halves, and
// reading them is the only way to compare them. The probe itself is driven as a real
// subprocess throughout `tests/e2e/adoption.rs`.
#[test]
fn both_probe_scripts_fill_in_the_same_placeholders() {
    // `@NAME@`, and nothing else that holds an `@` — `cmd`'s own `@echo off` is
    // the first line of one of these two files.
    let placeholders = |source: &str| -> Vec<String> {
        let mut found: Vec<String> = source
            .match_indices('@')
            .filter_map(|(at, _)| {
                let rest = &source[at + 1..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_uppercase() || *c == '_')
                    .collect();
                (!name.is_empty() && rest[name.len()..].starts_with('@'))
                    .then(|| format!("@{name}@"))
            })
            .collect();
        found.sort();
        found.dedup();
        found
    };
    let shell = placeholders(include_str!("probe.sh"));
    assert_eq!(
        shell,
        placeholders(include_str!("probe.bat")),
        "the probe scripts have drifted: one platform fills in a placeholder the other does not"
    );
    // A drift gate comparing two empty lists finds nothing and passes.
    assert_eq!(shell, ["@RUNS_FILE@", "@VERSION_FILE@"]);
} // llmlint: ignore-end[tests_mirror_real_usage]

/// The other half of that contract — what the hook *refuses*, and with what —
/// held by pushing at a repository that has it installed.
///
/// The verb list is the part two files can be compared on; what a refusal
/// actually costs a caller is the part only a push proves, and no host runs both
/// halves. So each platform's leg proves its own: read together across the CI
/// matrix, the two tests are the whole drift gate.
///
/// Driven through **git**, because that is the only thing that runs a `pre-push`
/// hook: the repository is the one every journey publishes into, the hook behind
/// its `core.hooksPath` is written by [`install_hook`] from the argv
/// [`hook_argv`] states, and each case is a real `git push` at a real bare
/// origin. That also makes this the one test that runs the wrapper `install_hook`
/// writes rather than the argv inside it — on Windows the wrapper is a POSIX line
/// git's bundled shell reads before it ever reaches `hook.bat`, and a wrapper that
/// lost the verb's arguments there would leave every push in a hook that never
/// returns.
///
/// What is asserted is what git makes observable, and the exit code is not part
/// of it: git relays a hook's *words* to the caller and its own verdict to the
/// shell, so every non-zero the script can answer with arrives as one declined
/// push. `sysexits.h`'s 64 and a host that refused a write are still separate
/// exits in both halves — `hook.sh` says why — but nothing downstream of git can
/// tell them apart, so a test asserting the difference here would be asserting
/// against an interface no caller has. What a caller does have is these three,
/// and each of them is load-bearing: the push is **refused**, the base does
/// **not** move, and the hook's own complaint reaches them — the last checked as
/// the words both halves write, because the two disagree over how much of the
/// usage line a missing argument earns and neither wording is wrong.
///
/// The last push is the control. Six refusals prove nothing on their own — a hook
/// that turned every push down would pass all of them — so a command line the
/// script *does* speak follows, and the base moves.
///
/// The state root is handed over even though only the last case reaches it, and
/// that is the point: without it that one would refuse for the *missing* root
/// rather than for the tree git is running the hook in, and the complaint
/// asserted on below would be one this test never meant to provoke.
#[test]
fn the_hook_refuses_a_command_line_it_does_not_speak() {
    let world = World::new("hook-refusals");
    // Installed through the same door a journey uses, so what git runs here is
    // what git runs there. The argv is rewritten per case below; the checkout's
    // `core.hooksPath` already points at the directory holding it.
    let repo = world.repository("local-direct", &as_argv(&hook_argv(&world, &["nonsense"])));
    let hook = hooks_dir(&world).join("pre-push");
    let base = || git(&world, &repo.origin, &["rev-parse", "main"]);
    let seed = base();

    for (argv, complaint) in [
        (vec!["nonsense"], "unknown command 'nonsense'"),
        // Both halves refuse this, and they word it differently — one counts the
        // arguments before it looks at them and says so, the other misses the
        // path first. The part they share is the part a caller needs.
        (vec!["wait-for"], "wait-for takes the path to wait for"),
        (
            vec!["wait-for", "one", "two"],
            "wait-for takes the path to wait for, and nothing else",
        ),
        (
            vec!["break-streams", "extra"],
            "break-streams takes no arguments",
        ),
        (
            vec!["append-future-event", "extra"],
            "append-future-event takes no arguments",
        ),
        // Not under a session's run root, which is where the stream is found.
        (
            vec!["append-future-event"],
            "append-future-event runs in a tree under a session's run root",
        ),
    ] {
        install_hook(&hook, &as_argv(&hook_argv(&world, &argv)));
        let refused = publishing_push(&world, &repo.checkout, &format!("{argv:?}"));
        let said = String::from_utf8_lossy(&refused.stderr).into_owned();
        assert!(
            !refused.status.success(),
            "the repository's merge path let {argv:?} through: {said}"
        );
        assert!(
            said.contains(complaint),
            "the hook refused {argv:?} without saying {complaint:?}: {said}"
        );
        assert!(
            said.contains("the verbs are:"),
            "the hook refused {argv:?} without naming the verbs it does speak: {said}"
        );
        assert_eq!(
            base(),
            seed,
            "a push the repository's merge path turned down over {argv:?} landed anyway"
        );
    }

    // `wait-for`, and yet this case runs on every platform — because the
    // rendezvous is written *first*, so the verb finds it on its first look and
    // returns without ever blocking. A hook this test could abandon still waiting
    // is what [`abandonable_hook_script`]'s Unix-only gate is drawn around, and this
    // one cannot be; that is why the argv is spelled through [`hook_argv`] rather than
    // through the gated door.
    //
    // The ordering is therefore load-bearing rather than incidental, and it is
    // asserted rather than left to the reader: move the write below the install,
    // or drop it, and this refuses here instead of hanging the Windows leg on a
    // push nothing releases — which is the failure this whole gate exists to end.
    let go = world.root.join("push.go");
    std::fs::write(&go, "go").expect("the rendezvous is written");
    assert!(
        go.is_file(),
        "the rendezvous at {} is not in place before the hook that waits for it is \
         installed, so the push below would block on a file nothing writes",
        go.display()
    );
    install_hook(
        &hook,
        &as_argv(&hook_argv(&world, &["wait-for", &go.to_string_lossy()])),
    );
    let accepted = publishing_push(&world, &repo.checkout, "a verb it speaks");
    assert!(
        accepted.status.success(),
        "the repository's merge path turned down a command line its hook speaks: {}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_ne!(
        base(),
        seed,
        "the base did not move for a push nothing refused, so the six above were \
         refused by something other than the hook"
    );
}

/// A removal `break-streams` cannot perform refuses the push, rather than
/// reporting the verb it did not do as done.
///
/// The verb's other ending. [`the_hook_refuses_a_command_line_it_does_not_speak`]
/// covers the argv a caller got wrong, which `hook.sh` answers with `fail`; this
/// is the host's own refusal, which it answers with `broke` — and the difference
/// matters to the caller, because the remedy for one is the command line and for
/// the other the mount. Without the check behind it the removal's failure would
/// print on stderr and the verb would still succeed, leaving a journey asserting
/// on a stream that is still there.
///
/// Provoked the only way a removal can be made to fail without touching the code
/// under test: the state root is sealed, so `rm -rf` still empties `streams/` and
/// then cannot unlink the directory out of its parent.
///
/// **Unix-only for the provocation, not for the path** — `hook.bat` guards the
/// same removal, and the drift gate in
/// [`both_hook_scripts_answer_the_same_verbs`] is what keeps the
/// two halves in step. A mode bit is what has no Windows spelling. It also reads
/// a suite that is not running as root, which unlinks whatever the mode says: no
/// runner this suite runs on does, and one that did would fail here rather than
/// pass on a provocation that never happened.
#[cfg(unix)]
#[test]
fn a_removal_the_host_refuses_declines_the_push_rather_than_reporting_it_done() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new("hook-unremovable-streams");
    let repo = world.repository(
        "local-direct",
        &as_argv(&hook_argv(&world, &["break-streams"])),
    );
    let base = || git(&world, &repo.origin, &["rev-parse", "main"]);
    let seed = base();

    // The parts of the state root a real run would have written and registering
    // alone does not. `require_state_root` demands the whole store `onevcs::home`
    // lays out, and then the stream of the session making the push — found by the
    // basename of an ancestor of the tree git runs the hook in. Laid out here so
    // the verb reaches its removal rather than refusing earlier, for a root that
    // is not this push's, in a complaint this journey never meant to provoke.
    let home = world.onevcs_home();
    assert!(
        home.join("registry.json").is_file(),
        "registering wrote no registry.json, so this journey would provoke the hook's \
         complaint about the root rather than about the removal"
    );
    let streams = home.join("streams");
    for held in ["locks", "sessions", "workspaces"] {
        std::fs::create_dir_all(home.join(held)).expect("the state root is laid out");
    }
    std::fs::create_dir_all(&streams).expect("the state root is laid out");
    let named = repo
        .checkout
        .file_name()
        .expect("the checkout is a named directory");
    std::fs::write(
        streams.join(format!("{}.ndjson", named.to_string_lossy())),
        "",
    )
    .expect("this tree's stream is in place");

    // Sealed after the registration above, which needed to write here.
    let opened = std::fs::metadata(&home)
        .expect("the state root is there to seal")
        .permissions();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o555))
        .expect("the state root is sealed");
    let refused = publishing_push(&world, &repo.checkout, "a removal the host refuses");
    // Restored before anything is asserted, so a failing assertion still leaves
    // the world's scratch removable rather than failing its teardown as well.
    std::fs::set_permissions(&home, opened).expect("the state root is unsealed");

    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        !refused.status.success(),
        "the merge path let a push through whose hook could not remove {}: {said}",
        streams.display()
    );
    // The hook's own words, not `rm`'s. Unguarded, the removal still writes
    // `rm: cannot remove …` to the same stderr and the verb carries on — so an
    // assertion on that phrase alone passes against the bug it exists to catch.
    assert!(
        said.contains("pre-push: cannot remove"),
        "the hook declined the push without saying the removal is what failed: {said}"
    );
    assert!(
        said.contains("the host refused the write, not the caller"),
        "the hook read a removal it could not perform as the caller's mistake: {said}"
    );
    assert_eq!(
        base(),
        seed,
        "a push the repository's merge path turned down landed anyway"
    );
}

/// A hold nothing releases gives the push up, rather than waiting out the job it
/// runs in.
///
/// The one thing no other journey reaches: every journey that holds a publication
/// releases it, on drop if not before, so none of them ever waits the ceiling out.
///
/// Driven through **git** for the reason
/// [`the_hook_refuses_a_command_line_it_does_not_speak`] gives, and asserting what
/// that leaves a caller: the push declined, the words the hook wrote, and a base
/// that did not move. The ceiling comes from the environment because no journey
/// can wait five minutes out.
#[test]
fn a_held_push_nothing_releases_expires_rather_than_waiting_out_the_job() {
    let world = World::new("hook-expiry").with_env(HOOK_WAIT_SECONDS_ENV, "2");
    // Written by nothing, here or in the world's teardown: the abandonment this
    // ceiling exists for is a hold whose journey is gone, and a rendezvous that
    // arrived late would make this a slow release rather than an expiry.
    let go = world.root.join("push.go");
    let repo = world.repository(
        "local-direct",
        &as_argv(&hook_argv(&world, &["wait-for", &go.to_string_lossy()])),
    );
    let base = || git(&world, &repo.origin, &["rev-parse", "main"]);
    let seed = base();

    let waited = std::time::Instant::now();
    let refused = publishing_push(&world, &repo.checkout, "a hold nothing releases");
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();

    assert!(
        !refused.status.success(),
        "the repository's merge path let a push through that its hook was still \
         holding: {said}"
    );
    assert!(
        said.contains(&go.display().to_string()),
        "the hook gave up without naming the rendezvous nothing wrote, which is the \
         one thing a person reading this has to know: {said}"
    );
    assert!(
        said.contains("the held push expired"),
        "the hook refused the push without saying the wait expired, so this reads \
         like a merge path that turned the change down: {said}"
    );
    assert!(
        said.contains("within the ceiling of 2 seconds"),
        "the hook gave up at a ceiling other than the one this world gave it, so \
         what expired is some other bound: {said}"
    );
    assert_eq!(
        base(),
        seed,
        "the push landed anyway, so the expiry refused nothing"
    );
    // It waited. A ceiling misread as zero would expire every hold before its
    // journey had looked at anything, and the words above would not say so.
    // A second rather than the two given, because the hook counts its deadline in
    // whole seconds and may start a fraction of one into the first.
    assert!(
        waited.elapsed() >= std::time::Duration::from_secs(1),
        "the hook gave up at once instead of holding the push for the ceiling it \
         was given: {said}"
    );
    assert!(
        !go.is_file(),
        "something wrote the rendezvous at {}, so the push was released rather \
         than expired",
        go.display()
    );
}

/// Commit one change and push it, and hand back whatever git made of that —
/// which is a refusal as often as not, so unlike [`git`] this does not assert.
///
/// A commit of its own per push because git runs no `pre-push` hook for a push
/// with nothing in it: without one the second case onwards would be
/// `Everything up-to-date`, the hook would never run, and every assertion after
/// it would be reading the first case's answer.
///
/// The state root travels on the push rather than on this process, because the
/// hook is git's child and inherits it from there.
fn publishing_push(world: &World, checkout: &Path, what: &str) -> std::process::Output {
    let file = checkout.join(format!("{}.md", slug_of(what)));
    std::fs::write(&file, format!("{what}\n")).expect("the change is written");
    git(world, checkout, &["add", "-A"]);
    git(world, checkout, &["commit", "-m", &format!("feat: {what}")]);
    Command::new("git")
        .args(["push", "origin", "main"])
        .current_dir(checkout)
        .env("GIT_CONFIG_GLOBAL", world.gitconfig())
        .env("GIT_AUTHOR_NAME", GIT_WHO)
        .env("GIT_AUTHOR_EMAIL", GIT_EMAIL)
        .env("GIT_COMMITTER_NAME", GIT_WHO)
        .env("GIT_COMMITTER_EMAIL", GIT_EMAIL)
        .env("ONEVCS_HOME", world.onevcs_home())
        // What the world was handed reaches the hook too: git passes its own
        // environment to the hook it runs, and a journey that stated one for this
        // world means it wherever this world publishes.
        .envs(world.environment.iter().map(|(key, value)| (key, value)))
        .output()
        .expect("git runs")
}

/// The borrowed argv every installer takes, out of the owned one
/// [`hook_argv`] hands back.
fn as_argv(argv: &[String]) -> Vec<&str> {
    argv.iter().map(String::as_str).collect()
}

/// A filename for a case named by its own argv, which is full of characters a
/// path is not.
fn slug_of(what: &str) -> String {
    what.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The interpreter that leads a hook script's argv on this platform.
#[cfg(windows)]
fn interpreted(script: &Path) -> Vec<String> {
    vec![
        "cmd".to_owned(),
        "/C".to_owned(),
        script.to_string_lossy().into_owned(),
    ]
}

/// The interpreter that leads a hook script's argv on this platform.
#[cfg(not(windows))]
fn interpreted(script: &Path) -> Vec<String> {
    vec!["sh".to_owned(), script.to_string_lossy().into_owned()]
}

/// The hook script for this platform, written into the world's own scratch.
///
/// Written with **CRLF**, and that is a correctness requirement rather than a
/// convention. `cmd.exe` does not read a batch file line by line: it seeks by
/// byte offset after each command, and the arithmetic assumes two bytes end a
/// line. Given the LF the repository's `.gitattributes` checks every file out
/// with, each seek lands one byte earlier than the last, and cmd starts
/// executing the *middles of words* — the first symptom was `'ows' is not
/// recognized as an internal or external command`, out of `rem The Windows half`
/// eight lines above anything this script does. So the conversion happens here,
/// at the one place the file becomes a program, rather than by excepting `.bat`
/// from the repository's line-ending policy for a file no editor on this side
/// ever opens.
#[cfg(windows)]
fn write_hook_script(world: &World) -> PathBuf {
    let path = world.root.join("hook.bat");
    std::fs::write(&path, include_str!("hook.bat").replace('\n', "\r\n"))
        .expect("the hook script is written");
    path
}

/// The hook script for this platform, written into the world's own scratch.
#[cfg(not(windows))]
fn write_hook_script(world: &World) -> PathBuf {
    let path = world.root.join("hook.sh");
    std::fs::write(&path, include_str!("hook.sh")).expect("the hook script is written");
    path
}

/// Whether the host would start this file if a `PATH` lookup landed on it.
///
/// The executable bit is what `execvp` asks on Unix, so it is what is asked here.
/// Windows carries no such bit and decides by extension instead, which
/// [`World::resolved_on`] has already applied by the time this is reached.
#[cfg(unix)]
fn runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Whether the host would start this file if a `PATH` lookup landed on it.
#[cfg(not(unix))]
fn runnable(path: &Path) -> bool {
    path.is_file()
}

/// The release-probe script for this platform: its file name and its body.
///
/// Written with **CRLF** on Windows, and for the reason
/// [`write_hook_script`] states at length: `cmd.exe` seeks a batch file by byte
/// offset and the arithmetic assumes two bytes end a line.
#[cfg(windows)]
fn probe_script() -> (&'static str, String) {
    ("probe.bat", include_str!("probe.bat").replace('\n', "\r\n"))
}

/// The release-probe script for this platform: its file name and its body.
#[cfg(not(windows))]
fn probe_script() -> (&'static str, String) {
    ("probe.sh", include_str!("probe.sh").to_owned())
}

/// A file shipped in the repository.
pub fn repo_file(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// How many lines of one file a dump prints before it says how many it left.
///
/// The remainder is counted out loud rather than dropped: a silently truncated
/// file reads as the whole of what was recorded. Generous, because the thing a
/// dump exists to carry is a producer's own error text and those run to tens of
/// lines — and bounded anyway, because a run directory also holds retained
/// agent reports, and a suite that printed every one of those on every failure
/// buries the assertion that failed.
const DUMP_LINE_CEILING: usize = 200;

/// Everything one directory holds, recursively, as a diagnostic reads it.
///
/// Read leniently, unlike every assertion in this suite: a dump is what a
/// failing journey prints, and one journey is *about* a store holding a line no
/// reader can parse. Panicking here would replace that journey's own failure
/// with this one's, on every command it runs — so an unreadable entry is
/// reported as unreadable and the walk carries on.
fn dump_into(out: &mut String, dir: &Path, indent: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        out.push_str(&format!("{indent}(unreadable directory)\n"));
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    for name in names {
        let path = dir.join(&name);
        if path.is_dir() {
            out.push_str(&format!("{indent}{name}/\n"));
            dump_into(out, &path, &format!("{indent}  "));
            continue;
        }
        out.push_str(&format!("{indent}{name}:\n"));
        let held = format!("{indent}  ");
        match std::fs::read(&path) {
            Err(error) => out.push_str(&format!("{held}(unreadable: {error})\n")),
            Ok(bytes) if bytes.is_empty() => out.push_str(&format!("{held}(empty)\n")),
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => {
                    let lines: Vec<&str> = text
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .collect();
                    for line in lines.iter().take(DUMP_LINE_CEILING) {
                        out.push_str(&format!("{held}{line}\n"));
                    }
                    if let Some(left) = lines.len().checked_sub(DUMP_LINE_CEILING) {
                        if left > 0 {
                            out.push_str(&format!("{held}(and {left} more line(s))\n"));
                        }
                    }
                }
                Err(error) => {
                    out.push_str(&format!("{held}(not UTF-8: {})\n", error.utf8_error()));
                }
            },
        }
    }
}

/// Read a JSONL file the code under test wrote.
///
/// A file that is not there yet is an empty stream: every `until` polls one
/// before its first record exists. A **line** that is not JSON is not read the
/// same way. Several processes append to these, so the last line of the file
/// may be an append still in flight and is skipped; a torn line with another
/// line after it cannot be, so it is a record the writer lost, and reading past
/// it would let a test assert against a gap and call it a pass.
fn read_jsonl(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let last = lines.len().saturating_sub(1);
    lines
        .iter()
        .enumerate()
        .filter_map(|(at, line)| match serde_json::from_str(line) {
            Ok(value) => Some(value),
            Err(_) if at == last => None,
            Err(error) => panic!(
                "{} line {} is torn, so a record the writer appended was lost: {error}\n{line}",
                path.display(),
                at + 1
            ),
        })
        .collect()
}

/// One string as a YAML scalar that survives being read back.
///
/// A member `task` carries newlines and braces, and both change meaning
/// unquoted: `{task}` on its own opens a flow mapping, so a config written
/// plainly would be refused by the runner rather than handed to the member. JSON
/// string syntax is valid YAML double-quoted syntax, so the serializer that
/// already knows every escape does the quoting.
fn yaml_scalar(text: &str) -> String {
    serde_json::to_string(text).expect("a string serializes")
}

/// A direct agent node.
pub fn agent(id: &str, deps: &[&str]) -> Value {
    serde_json::json!({
        "id": id,
        "persona": "engineer",
        "task": format!("## What\nDo {id}.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- {id} is done."),
        "deps": deps,
    })
}

/// A `kind: human` action.
pub fn human(id: &str, deps: &[&str]) -> Value {
    serde_json::json!({
        "id": id,
        "kind": "human",
        "task": format!("Do {id}, which only a person can do."),
        "deps": deps,
    })
}

/// A lifecycle node: one that names a repository.
///
/// `service` is the alias [`World::repository`] registers its checkout under, so
/// a journey states the repository once and the node names it the way an
/// operator would.
pub fn lifecycle(id: &str, deps: &[&str]) -> Value {
    serde_json::json!({
        "id": id,
        "repo": "service",
        "persona": "engineer",
        // The title its change request opens under, which a lifecycle node
        // states from plan schema 3 on. A journey about the title or the body
        // states its own.
        "title": format!("feat: ship {id}"),
        "task": format!("## What\nShip {id}.\n\n## Why\nUsers need it.\n\n## Acceptance criteria\n- {id} is published."),
        "deps": deps,
    })
}

/// A plan holding these nodes.
pub fn plan_of(name: &str, nodes: Vec<Value>) -> Value {
    serde_json::json!({
        "schema_version": onepipeline::plan::PLAN_SCHEMA_VERSION,
        "name": name,
        "concurrency": 4,
        "goal": {"text": format!("Deliver {name}")},
        "tasks": nodes,
    })
}

/// A directory reached through a symlink resolves to the directory itself.
///
/// The rule [`resolved`] exists for, held where the symlink can be built. macOS
/// ships this shape by default — `/var` is a link to `/private/var`, so every
/// temporary directory there is reached through one — and it is what made a
/// journey compare two spellings of one directory and fail. A symlink is a
/// symlink on Linux too, so the rule is held here rather than only on the
/// platform that ships it.
///
/// Unix-only because creating a *directory* symlink on Windows needs a
/// privilege no CI account is guaranteed to hold. What `resolved` has to get
/// right on Windows is [`plain`]'s rule, and the test below holds that one on
/// every platform.
#[cfg(unix)]
#[test]
fn a_directory_reached_through_a_symlink_resolves_to_the_directory_itself() {
    let scratch =
        resolved(&std::env::temp_dir()).join(format!("op-resolved-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let directory = scratch.join("real");
    std::fs::create_dir_all(&directory).expect("a directory to resolve to");
    let route = scratch.join("route");
    std::os::unix::fs::symlink(&directory, &route).expect("a symlinked route to it");

    assert_ne!(
        route, directory,
        "the route is not a second spelling at all"
    );
    assert_eq!(
        resolved(&route),
        directory,
        "a route to a directory was kept instead of the directory"
    );
    std::fs::remove_dir_all(&scratch).ok();
}

/// A verbatim Windows path is spelled the way the rest of Windows spells it.
///
/// [`std::fs::canonicalize`] is the only thing on that platform that produces
/// the `\\?\` prefix, so holding its answer against a launch record — which
/// carries `GetCurrentDirectory`'s — would compare two spellings of one
/// directory. The rule is pure text, so it is held on every platform rather
/// than only on the one where it fires.
#[test]
fn a_verbatim_windows_path_is_spelled_the_way_the_rest_of_windows_spells_it() {
    assert_eq!(
        plain(Path::new(r"\\?\C:\Users\op\runs")),
        PathBuf::from(r"C:\Users\op\runs")
    );
    assert_eq!(
        plain(Path::new(r"\\?\UNC\host\share\runs")),
        PathBuf::from(r"\\host\share\runs")
    );
    // And nothing else is touched: a path that never carried the prefix is
    // already the spelling everything else uses.
    assert_eq!(
        plain(Path::new("/private/var/folders/op")),
        PathBuf::from("/private/var/folders/op")
    );
    assert_eq!(
        plain(Path::new(r"C:\Users\op\runs")),
        PathBuf::from(r"C:\Users\op\runs")
    );
}
