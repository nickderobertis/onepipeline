//! The scaffolding every journey here shares.
//!
//! Each test gets its own runs root, its own launching session, and its own
//! scripted **doubles** for the two sibling CLIs. Be clear about what that
//! means: `oneagentgraph` and `onevcs` are substituted. Nothing *inside*
//! `onepipeline` is — it is driven as a compiled subprocess, and it reaches the
//! doubles the same way it reaches the real thing, by executing a program and
//! reading its stdout.

// llmlint: ignore-file[e2e_not_mocked] the two siblings are substituted at their
// subprocess boundary, so a journey can state a scenario — a node that fails its gate, a
// dispatch held open, a driver that dies — instead of arranging one out of real agent
// turns. `onevcs` has no alternative yet: it is still at its interface-only stage and
// refuses every invocation with exit 70. `oneagentgraph` does, and `dispatch.rs` takes
// it: those journeys run the real binary through [`World::real_cmd`] and substitute only
// the paid model turn. Every scripted scenario a journey here needs is one the real
// sibling would need a paid turn to produce.

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
    pub fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-e2e-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
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
        command
            .args(args)
            .env("ONEPIPELINE_RUNS_DIR", &self.runs)
            .env(
                "ONEPIPELINE_ONEAGENTGRAPH_BIN",
                double("fake-oneagentgraph"),
            )
            .env("ONEPIPELINE_ONEVCS_BIN", double("fake-onevcs"))
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

    /// The `onepipeline` binary with the **real** `oneagentgraph` behind the
    /// seam, and only the paid model turn replaced.
    ///
    /// The double swapped in here is one layer further out than the ones
    /// [`cmd`](World::cmd) uses: `oneagentgraph` resolves the graph, prepares the
    /// member, and supervises it for real, and what stands in is the harness it
    /// spawns — at that library's own documented `ONEAGENTGRAPH_ONEHARNESS_BIN`
    /// override, which knows nothing about this crate.
    pub fn real_cmd(&self, args: &[&str]) -> Command {
        let mut command = self.cmd(args);
        command
            .env("ONEPIPELINE_ONEAGENTGRAPH_BIN", sibling_binary())
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
    pub fn run_real(&self, args: &[&str]) -> Run {
        Run::of(
            self.real_cmd(args).output().expect("the binary runs"),
            args,
            self,
        )
    }

    /// Where this world's agent-graph configs live.
    pub fn graphs(&self) -> PathBuf {
        self.root.join("graphs")
    }

    /// Write the graph configs [`real_cmd`](World::real_cmd) names.
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

    /// Every turn the paid-harness double was asked for, in order.
    pub fn turns(&self) -> Vec<Value> {
        read_jsonl(&self.fakes.join("turns.jsonl"))
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
        self.out_has("\"settlement\"")
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
/// a link, so that name is briefly absent. A test that had already looked would
/// then spawn a binary that vanished under it: on macOS that surfaced both as a
/// missing double and, worse, as a whole run stuck at `run-started` because the
/// launcher could not start its driver. Linking each process its own name once
/// closes the window — a link keeps the inode alive whatever cargo does to the
/// name it was made from.
pub fn double(name: &str) -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let held = BUILT.get_or_init(|| build(&["--package", "onepipeline-testfakes"]));
    held_copy(held, name)
}

/// The **real** `oneagentgraph` binary, built from the version this crate
/// depends on.
///
/// Not the one on `PATH`: that is whatever an operator happened to install, and
/// a journey proving this crate composes its sibling has to compose the sibling
/// `Cargo.lock` pins. Cargo builds a dependency's binary the same way it builds
/// a workspace member's, so this needs no extra provisioning — the library is
/// already compiled by the time a test runs.
pub fn sibling_binary() -> PathBuf {
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
            match std::fs::hard_link(&published, &mine)
                .or_else(|_| std::fs::copy(&published, &mine).map(|_| ()))
            {
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
pub fn lifecycle(id: &str, deps: &[&str]) -> Value {
    serde_json::json!({
        "id": id,
        "repo": "owner/service",
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
