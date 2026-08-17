//! The scaffolding every journey here shares.
//!
//! Each test gets its own runs root, its own launching session, its own `onevcs`
//! state root, and a scripted **double for `oneagentgraph`**. Be clear about what
//! that means: `onevcs` is **not** substituted. This crate calls that library
//! rather than spawning it, so there is no subprocess boundary to stand in at —
//! every journey below drives the real repository side, over a real bare-
//! repository origin on disk, and what a journey states instead is the world that
//! library reads: the repository's rules, its gate command, and — at `onevcs`'s
//! own `ONEVCS_GH` override — what GitHub does with the change request it is
//! handed.
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

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use onepipeline_testfakes::{MEMBER_ENV, SCRIPT_DIR_ENV};
use serde_json::Value;

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
        }
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
            .env("GIT_CONFIG_GLOBAL", self.gitconfig())
            .env("GIT_AUTHOR_NAME", GIT_WHO)
            .env("GIT_AUTHOR_EMAIL", GIT_EMAIL)
            .env("GIT_COMMITTER_NAME", GIT_WHO)
            .env("GIT_COMMITTER_EMAIL", GIT_EMAIL)
            .env(SCRIPT_DIR_ENV, &self.fakes)
            .env("ONEPIPELINE_FAKE_DRIVER_BIN", binary())
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
            .env("ONEPIPELINE_FAKE_RENDEZVOUS_SECONDS", "180")
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

    /// The `onepipeline` binary with the **real** `oneagentgraph` behind that one
    /// seam, and only the paid model turn replaced inside it.
    ///
    /// Only that seam: the host stand-in [`cmd`](World::cmd) wires up stays, so
    /// these journeys need no credential, and they name no lifecycle node —
    /// what they are about is the dispatch. The double swapped in here is one
    /// layer further out than the other — `oneagentgraph` resolves the graph,
    /// prepares the member, and supervises it for real, and what stands in is
    /// the harness it spawns, at that library's own documented
    /// `ONEAGENTGRAPH_ONEHARNESS_BIN` override, which knows nothing about this
    /// crate.
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
            .env("ONEAGENTGRAPH_ONEHARNESS_BIN", double("fake-oneharness"))
            // The paid turn, at oneharness's own per-harness binary seam. A
            // single-sided member's turn is an `oneharness_core` library call
            // from `oneagentgraph 0.2.18` on, so `ONEAGENTGRAPH_ONEHARNESS_BIN`
            // above no longer stands between this suite and a provider — the
            // only process left below the library is the harness its identity
            // chain selects, which is this one. Set here rather than only in the
            // graphs' `oneharness.toml`, because a journey that writes its own
            // config would otherwise reach for a `claude` nobody in this suite
            // chose; the environment beats a config-file `bin`, so one value
            // covers every member of every graph a journey writes.
            .env("ONEHARNESS_BIN_CLAUDE_CODE", double("fake-claude"))
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
    /// request through [`ONEVCS_GH`](World::cmd)'s stand-in. `gate` is the
    /// command that verifies the branch, which is where a journey states a gate
    /// that rejects or one that holds.
    pub fn repository(&self, publication: &str, gate: &[&str]) -> Repository {
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

        std::fs::write(
            home.join("rules.yml"),
            format!(
                "version: 2\nrules: []\ndefault:\n  publication: {publication}\n  approvals: \
                 none\n  gate:\n    command: {}\n",
                serde_json::json!(gate)
            ),
        )
        .expect("the rules file is written");

        // A hosted origin, so the identity resolves to a host slug and a
        // `change-*` policy has somewhere to open a change request. It names the
        // identity and nothing else: the clone a session cuts takes its remote
        // from the *checkout*, which points at the bare origin above, so every
        // fetch and push this journey makes stays on this disk.
        self.register(&checkout, Some("https://github.com/owner/service.git"));
        Repository { origin, checkout }
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
        }
        let code = onevcs::run(&onevcs::cli::Cli::parse_from(argv));
        assert_eq!(code, 0, "onevcs refused to register {}", checkout.display());
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
    /// that removal is the whole difference. Left in place it would point the
    /// one credentialled journey in this repository at a program that answers
    /// every `gh` call out of a scratch directory — a smoke that passes without
    /// having talked to GitHub, which is the defect that tier exists to remove.
    pub fn real_cmd(&self, args: &[&str]) -> Command {
        let mut command = self.agentgraph_cmd(args);
        command.env_remove("ONEVCS_GH");
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
    /// Single-sided `kind: oneharness` members: the two-party kind runs a
    /// onejudge conversation in `oneagentgraph`'s own process against a provider
    /// this suite has no offline stand-in for, and the seam under test — a
    /// dispatch reaching the sibling, being accepted, and streaming back — is the
    /// same one either way.
    pub fn write_graphs(&self) {
        self.write_graphs_with(None, CONSUMER_GRAPH_SCHEMA);
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
    #[cfg(unix)]
    pub fn path_whose_ps_says_more_than_it_was_asked(&self) -> PathBuf {
        self.path_with_ps("talkative-ps", &answer_plus("a line nobody asked for"))
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

    /// Write a plan file and return its path.
    pub fn plan(&self, name: &str, plan: &Value) -> PathBuf {
        let path = self.root.join(format!("{name}.plan.json"));
        std::fs::write(
            &path,
            serde_json::to_string_pretty(plan).expect("the plan serialises"),
        )
        .expect("the plan is written");
        path
    }

    /// Write a raw plan file, for the loader's own refusals.
    pub fn raw_plan(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, body).expect("the plan is written");
        path
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while std::time::Instant::now() < deadline {
            if ready(self) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!(
            "timed out waiting for {what}; the runs root held:\n{}",
            self.dump()
        );
    }

    /// Every run's journal, as the kinds it recorded, and what each observer the
    /// launcher started found. What a failure needs to say *why* it failed — the
    /// alternative is a bare assertion with no evidence, which is a whole
    /// debugging session per platform-only defect.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        match std::fs::read_dir(&self.runs) {
            Err(_) => out.push_str("  (no runs root)\n"),
            Ok(entries) => {
                for entry in entries.flatten() {
                    let run = entry.file_name().to_string_lossy().to_string();
                    out.push_str(&format!("  {run}:\n"));
                    for event in self.journal(&run) {
                        out.push_str(&format!("    {event}\n"));
                    }
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
                prompt: call["args"][0].as_str().unwrap_or_default().to_string(),
                cwd: call["args"][1].as_str().unwrap_or_default().to_string(),
                member: call["args"][2].as_str().unwrap_or_default().to_string(),
            })
            .collect()
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

/// How many worlds this process still holds.
///
/// The doubles this process linked are shared by all of them, so they are
/// released when the last one goes — never on the first, which
/// [`as_session`](World::as_session) makes a real case.
static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Drop for World {
    fn drop(&mut self) {
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
            let held = std::fs::hard_link(&published, &mine)
                .or_else(|_| std::fs::copy(&published, &mine).map(|_| ()));
            match held {
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

/// A repository gate command, written into this world as a script.
///
/// A script rather than a compiled binary, and per platform rather than one
/// artifact, because the alternative was a workspace member shipping a Rust
/// program to stand in for three shell one-liners. `onevcs` runs the gate as
/// `Command::new(argv[0])`, which on Windows cannot start a `.bat` directly, so
/// each platform's interpreter leads its own argv.
///
/// The three verbs are what the lifecycle and telemetry journeys state: a gate
/// that blocks until a file appears, one that breaks the sibling's event stream
/// under it, and one that appends a line no build of that sibling can read.
pub fn gate_script(world: &World, args: &[&str]) -> Vec<String> {
    let mut argv = interpreted(&write_gate_script(world));
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
    argv
}

/// The two halves of the gate answer the same verbs.
///
/// One contract in two languages, because no platform runs both — so a verb
/// added to one and not the other is a journey that passes here and hangs on the
/// Windows leg, a fortnight later, with nothing pointing at the gate. Held by
/// reading the scripts rather than by generating one from the other: a generator
/// would need a third source, and what actually has to agree is the verb each
/// half dispatches on.
// llmlint: ignore-block[tests_mirror_real_usage] this is a drift gate over the suite's own
// scaffolding, not a journey: what it holds is that two files stay in step, and neither
// file is reachable from any interface a user of this crate has. There is nothing to drive
// through the binary here — the gate is a command `onevcs` runs on the operator's behalf,
// and the journeys that exercise it are `lifecycle.rs`'s and `views.rs`'s, which do drive
// the binary. Reading the two scripts is the only way to compare them, because no platform
// executes both.
#[test]
fn both_gate_scripts_answer_the_same_verbs() {
    let verb = |candidate: &str| {
        !candidate.is_empty()
            && candidate
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-')
    };
    // `sh`'s `case` arms, which are the only lines ending in a bare `)` whose
    // whole body is a verb — `*)` and every expression that happens to close a
    // parenthesis are filtered out by the alphabet.
    let shell: Vec<&str> = include_str!("gate.sh")
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_suffix(')'))
        .filter(|candidate| verb(candidate))
        .collect();
    // `cmd`'s dispatch, which is one `if "%~1"=="VERB" goto …` per verb.
    let batch: Vec<&str> = include_str!("gate.bat")
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("if \"%~1\"==\""))
        .filter_map(|rest| rest.split('"').next())
        .filter(|candidate| verb(candidate))
        .collect();
    assert_eq!(
        shell, batch,
        "the gate scripts have drifted: one platform answers verbs the other does not"
    );
    // The extraction is only a drift gate while it still finds anything: a
    // rewrite that changed either dispatch's shape would otherwise compare two
    // empty lists and pass.
    assert_eq!(
        shell,
        ["wait-for", "break-streams", "append-future-event"],
        "the gate scripts no longer dispatch the way this reads them"
    );
    // The refusal names the verbs, so it is a third statement of the same list
    // and drifts the same way — and it is the one a person reads when the gate
    // has just refused them, so a verb missing from it is worse than a verb
    // missing from a comment.
    for (script, source) in [
        ("gate.sh", include_str!("gate.sh")),
        ("gate.bat", include_str!("gate.bat")),
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

/// The other half of that contract — what the gate *refuses*, and with what —
/// held by running this platform's own script.
///
/// The verb list is the part two files can be compared on; the exit code and
/// the refusal message are the part only running proves, and no host runs both
/// halves. So each platform's leg proves its own: read together across the CI
/// matrix, the two tests are the whole drift gate.
///
/// Run the way `onevcs` runs a gate — `Command::new(argv[0])` with the rest as
/// arguments — because a refusal that only holds when invoked some other way is
/// not the one a publication would meet. Only the refusing command lines are
/// driven: each of them exits before the verb does anything, so this needs no
/// session and leaves nothing behind.
///
/// The state root is handed over even though nothing here reaches it, and that
/// is the point: without it every one of these would refuse for the *missing*
/// root, and the test would pass against a script that had lost its argument
/// checks entirely.
#[test]
fn the_gate_refuses_a_command_line_it_does_not_speak() {
    let world = World::new("gate-refusals");
    for argv in [
        vec!["nonsense"],
        vec!["wait-for"],
        vec!["wait-for", "one", "two"],
        vec!["break-streams", "extra"],
        vec!["append-future-event", "extra"],
        // Not a session worktree, which is the layout the token is read out of.
        vec!["append-future-event"],
    ] {
        let command = gate_script(&world, &argv);
        let refused = Command::new(&command[0])
            .args(&command[1..])
            .current_dir(&world.root)
            .env("ONEVCS_HOME", world.onevcs_home())
            .output()
            .expect("the gate runs");
        let said = String::from_utf8_lossy(&refused.stderr).into_owned();
        assert_eq!(
            refused.status.code(),
            Some(64),
            "the gate answered {argv:?} with {:?}, not the usage refusal: {said}",
            refused.status.code()
        );
        assert!(
            said.contains("the verbs are:"),
            "the gate refused {argv:?} without naming the verbs it does speak: {said}"
        );
    }
}

/// The interpreter that leads a gate script's argv on this platform.
#[cfg(windows)]
fn interpreted(script: &Path) -> Vec<String> {
    vec![
        "cmd".to_owned(),
        "/C".to_owned(),
        script.to_string_lossy().into_owned(),
    ]
}

/// The interpreter that leads a gate script's argv on this platform.
#[cfg(not(windows))]
fn interpreted(script: &Path) -> Vec<String> {
    vec!["sh".to_owned(), script.to_string_lossy().into_owned()]
}

/// The gate script for this platform, written into the world's own scratch.
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
fn write_gate_script(world: &World) -> PathBuf {
    let path = world.root.join("gate.bat");
    std::fs::write(&path, include_str!("gate.bat").replace('\n', "\r\n"))
        .expect("the gate script is written");
    path
}

/// The gate script for this platform, written into the world's own scratch.
#[cfg(not(windows))]
fn write_gate_script(world: &World) -> PathBuf {
    let path = world.root.join("gate.sh");
    std::fs::write(&path, include_str!("gate.sh")).expect("the gate script is written");
    path
}

/// A file shipped in the repository.
pub fn repo_file(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
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
