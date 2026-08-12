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

use serde_json::Value;

// The exit codes are the crate's own, not a second copy of them. A suite that
// restated the numbers would keep passing against a build that had changed one,
// which is exactly the drift these journeys exist to catch.

/// The exit code a refused or malformed command carries.
pub const REFUSED: i32 = onepipeline::error::EXIT_REFUSED;

/// The exit code for accepted-but-not-yet-reconciled edits, and for a round
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

/// How long a launch given a one-second backstop may take before the override
/// has to be presumed inert.
///
/// Well above a second of process startup on a loaded host, and well below the
/// crate's own default backstop, which is what a launch that never read
/// [`STARTUP_TIMEOUT_ENV`] would wait instead.
pub const OVERRIDE_TOOK_EFFECT: std::time::Duration = std::time::Duration::from_secs(15);

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
        let root = std::env::temp_dir().join(format!(
            "op-{}-{}",
            std::process::id(),
            NTH.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
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
        let path = std::env::join_paths(
            std::iter::once(
                onevcs_binary()
                    .parent()
                    .expect("onevcs has a directory")
                    .to_path_buf(),
            )
            .chain(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            )),
        )
        .expect("a PATH");
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
            .env("ONEPIPELINE_FAKE_DIR", &self.fakes)
            .env("ONEPIPELINE_FAKE_DRIVER_BIN", binary())
            .env("ONEPIPELINE_LAUNCHER", "e2e")
            .env("ONEPIPELINE_LAUNCHER_SESSION", &self.session)
            .env("ONEPIPELINE_PROJECT_DIR", &self.project)
            .env("ONEPIPELINE_DAG_GRAPH", repo_file("graphs/dag-scope.yaml"))
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
    pub fn agentgraph_cmd(&self, args: &[&str]) -> Command {
        let mut command = self.cmd(args);
        command
            .env("ONEPIPELINE_ONEAGENTGRAPH_BIN", oneagentgraph_binary())
            .env("ONEAGENTGRAPH_ONEHARNESS_BIN", double("fake-oneharness"))
            .env("ONEAGENTGRAPH_STATE_DIR", self.root.join("graph-state"))
            .env(
                "ONEPIPELINE_DAG_GRAPH",
                self.graphs().join("dag-scope.yaml"),
            )
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

    /// Write the graph configs [`agentgraph_cmd`](World::agentgraph_cmd) names.
    ///
    /// Single-sided `kind: oneharness` members: the two-party kind runs a
    /// onejudge conversation in `oneagentgraph`'s own process against a provider
    /// this suite has no offline stand-in for, and the seam under test — a
    /// dispatch reaching the sibling, being accepted, and streaming back — is the
    /// same one either way.
    pub fn write_graphs(&self) {
        let dir = self.graphs();
        std::fs::create_dir_all(&dir).expect("a directory for the graph configs");
        // The identity chain is the operator's own file, which the graph names
        // and this suite never selects out of: one harness family, so the model
        // pairing rule holds.
        std::fs::write(
            dir.join("oneharness.toml"),
            "run_mode = \"fallback\"\nharnesses = [\"claude-code\"]\n",
        )
        .expect("the harness config is written");
        for (file, member) in [
            ("dag-scope.yaml", "orchestrator"),
            ("node-scope.yaml", "worker"),
        ] {
            std::fs::write(
                dir.join(file),
                format!(
                    "version: 1\nname: {}\nmembers:\n  {member}:\n    kind: oneharness\n    \
                     oneharness_config: ./oneharness.toml\n",
                    file.trim_end_matches(".yaml"),
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

    /// Every run's journal, as the kinds it recorded, and what the driver made
    /// of each engine verb it ran. What a failure needs to say *why* it failed
    /// — the alternative is a bare assertion with no evidence, which is a whole
    /// debugging session per platform-only defect. The driver's own verbs are
    /// in here because it runs them as a subprocess of a subprocess, so a
    /// refusal one of them printed reaches no other descriptor a test can read.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        match std::fs::read_dir(&self.runs) {
            Err(_) => out.push_str("  (no runs root)\n"),
            Ok(entries) => {
                for entry in entries.flatten() {
                    let run = entry.file_name().to_string_lossy().to_string();
                    out.push_str(&format!("  {run}: {:?}\n", self.kinds(&run)));
                }
            }
        }
        for saw in self.driver_saw() {
            out.push_str(&format!("  driver saw: {saw}\n"));
        }
        out
    }

    /// Everything the doubles were asked for, in order.
    pub fn invocations(&self) -> Vec<Value> {
        read_jsonl(&self.fakes.join("invocations.jsonl"))
    }

    /// What each driver the launcher started found waiting for it, in order.
    pub fn driver_saw(&self) -> Vec<Value> {
        read_jsonl(&self.fakes.join("driver-saw.jsonl"))
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
    /// The engine journals the fact a surface is *about* — a spent budget, a
    /// worker gone quiet — and then raises the surface, as two appends. A test
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

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A repository `onevcs` knows about, and what its base branch carries.
pub struct Repository {
    /// The bare repository that stands in for the remote.
    pub origin: PathBuf,
    /// The registered execution and publication checkout.
    pub checkout: PathBuf,
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
/// a replacement, so that name is briefly absent. A test that had already looked would
/// then spawn a binary that vanished under it: on macOS that surfaced both as a
/// missing double and, worse, as a whole run stuck at `run-started` because the
/// launcher could not start its driver. Copying each process its own name once
/// closes the window because later cargo uplifts do not touch that copy.
pub fn double(name: &str) -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let held = BUILT.get_or_init(|| build(&["--package", "onepipeline-testfakes"]));
    held_copy(held, name)
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
    held_copy(held, "oneagentgraph")
}

/// The released `onevcs` executable whose holders verb the launcher consumes.
pub fn onevcs_binary() -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let held = build(&["--package", "onevcs", "--bin", "onevcs", "--locked"]);
            held_copy(&held, "onevcs")
        })
        .clone()
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
    let held = debug.join("doubles").join(std::process::id().to_string());
    std::fs::create_dir_all(&held).expect("a directory for this process's doubles");
    held
}

/// This process's own name for a built binary, made once and kept.
fn held_copy(held: &Path, name: &str) -> PathBuf {
    let debug = binary()
        .parent()
        .expect("the binary is in a directory")
        .to_path_buf();
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let published = debug.join(&file);
    let mine = held.join(&file);
    if !mine.is_file() {
        // Bounded, and retried rather than asserted on the first look: the
        // window this closes is another process's uplift, which is short but
        // real, and a bare `is_file` here would only move the flake one line up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let _ = std::fs::remove_file(&mine);
            match std::fs::copy(&published, &mine).map(|_| ()) {
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
        "task": format!("## What\nShip {id}.\n\n## Why\nUsers need it.\n\n## Acceptance criteria\n- {id} is published."),
        "deps": deps,
    })
}

/// A plan holding these nodes.
pub fn plan_of(name: &str, nodes: Vec<Value>) -> Value {
    serde_json::json!({
        "schema_version": 1,
        "name": name,
        "concurrency": 4,
        "goal": {"text": format!("Deliver {name}")},
        "tasks": nodes,
    })
}
