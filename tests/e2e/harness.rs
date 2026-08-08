//! The scaffolding every journey here shares.
//!
//! Each test gets its own runs root, its own launching session, and its own
//! scripted **doubles** for the two sibling CLIs. Be clear about what that
//! means: `oneagentgraph` and `onevcs` are substituted. Nothing *inside*
//! `onepipeline` is — it is driven as a compiled subprocess, and it reaches the
//! doubles the same way it reaches the real thing, by executing a program and
//! reading its stdout.

// llmlint: ignore-file[e2e_not_mocked] the two siblings are substituted at their
// subprocess boundary, and there is no alternative: both crates are at their own
// interface-only stage, so the real `oneagentgraph run` and `onevcs session open`
// refuse every invocation with exit 70. A suite built on them would prove that this
// crate can start a process that says no. Revisit each seam as its sibling implements
// it — the doubles are scripted per test and swapping one out is an env var.

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

/// The exit code a refused or malformed command carries.
pub const REFUSED: i32 = 2;

/// The exit code for accepted-but-not-yet-reconciled edits, and for a round
/// that settled unfinished.
pub const QUEUED: i32 = 1;

/// The exit code for a run nothing is driving.
pub const NOTHING_DRIVING: i32 = 3;

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
            .stdin(Stdio::null());
        command
    }

    /// Run a command to completion.
    pub fn run(&self, args: &[&str]) -> Run {
        Run::of(self.cmd(args).output().expect("the binary runs"), args)
    }

    /// Run a command with an envelope on stdin.
    pub fn run_with_stdin(&self, args: &[&str], stdin: &str) -> Run {
        self.run_with_stdin_timeout(self.cmd(args), stdin)
    }

    /// Run an already-configured command with an envelope on stdin.
    pub fn run_with_stdin_timeout(&self, command: Command, stdin: &str) -> Run {
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
        panic!("timed out waiting for {what}");
    }

    /// Everything the doubles were asked for, in order.
    pub fn invocations(&self) -> Vec<Value> {
        read_jsonl(&self.fakes.join("invocations.jsonl"))
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
}

impl Run {
    fn of(output: Output, args: &[&str]) -> Self {
        Self {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            args: args.join(" "),
        }
    }

    /// Assert the exit code, reporting what the command said when it differs.
    pub fn exited(&self, code: i32) -> &Self {
        assert_eq!(
            self.code, code,
            "`onepipeline {}` exited {} not {code}\nstdout: {}\nstderr: {}",
            self.args, self.code, self.stdout, self.stderr
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

/// One of the sibling doubles, beside the binary cargo built.
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
pub fn double(name: &str) -> PathBuf {
    static BUILT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

    let debug = binary()
        .parent()
        .expect("the binary is in a directory")
        .to_path_buf();
    BUILT.get_or_init(|| {
        let target = debug
            .parent()
            .expect("the profile directory is inside a target directory");
        let built = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["build", "--offline", "--package", "onepipeline-testfakes"])
            .arg("--target-dir")
            .arg(target)
            .current_dir(repo_file("."))
            .output()
            .expect("cargo builds the subprocess doubles");
        assert!(
            built.status.success(),
            "the subprocess doubles did not build: {}",
            String::from_utf8_lossy(&built.stderr)
        );
    });
    let path = debug.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.is_file(),
        "the {name} double is missing from {}",
        debug.display()
    );
    path
}

/// A file shipped in the repository.
pub fn repo_file(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
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
