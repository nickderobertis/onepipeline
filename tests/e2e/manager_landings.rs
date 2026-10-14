//! Manager landings — `onepipeline publish-branch`, `repo-recover` and
//! `integrate` started directly by a manager session — supervised as that
//! session's flow work, driven through the compiled binary.
//!
//! What this exists for is the hole entry 123 of `docs/contract-divergences.md`
//! records: a landing a manager ran by hand was one blocking call nothing owed a
//! watch, so a landing that blocked or died left the manager's watch and stop
//! hook silent, and an acknowledgement could close the work it was advancing.
//! Every journey lands over a repository registered with the real linked
//! `onevcs` and a real bare origin on disk. What stands in is what every landing
//! journey substitutes: `gh` at `onevcs`'s `ONEVCS_GH` seam, and the paid
//! drafting turn at `oneagentgraph`'s subprocess boundary. Each landing is held
//! at a handshake — the drafting turn waiting on a file this journey writes, or
//! the repository's own `pre-push` hook waiting on one — so a journey says when
//! the landing moves on, and asserts events and decisions, never elapsed time.
//!
//! Unix-only: deaths are delivered by `SIGKILL` to a known pid.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary and the sibling these journeys land through — `onevcs` — is the real library, over
// real git and a real origin on disk. `oneagentgraph` is substituted at its subprocess
// boundary so a drafting turn states its answer rather than paying for a model turn, and
// GitHub at `onevcs`'s own `ONEVCS_GH` override so a change request is opened offline.
// `harness.rs` carries the same suppression and the full rationale.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};

use serde_json::{json, Value};

use crate::harness::{
    git, held_publication, onevcs_binary, HeldPublication, Repository, World, REFUSED,
};

use onepipeline::error::{EXIT_FLOW_FAILED, EXIT_SUCCESS};

/// The branch a single-branch journey lands.
const BRANCH: &str = "widget";

/// `watch`'s status when its wait ran out.
const ELAPSED: i32 = onepipeline::error::EXIT_WATCH_ELAPSED;

/// `unwatched`'s status when it reports a line.
const OWED: i32 = onepipeline::error::EXIT_RUNS_UNWATCHED;

/// Cut `branch` in the checkout with one commit adding `file`, and leave the
/// checkout back on its base — the state a manager lands from.
fn branch_with_work(world: &World, repository: &Repository, branch: &str, file: &str) {
    branch_under(
        world,
        repository,
        branch,
        file,
        &format!("feat: add {file}"),
    );
}

/// [`branch_with_work`], its commit under `subject`.
fn branch_under(world: &World, repository: &Repository, branch: &str, file: &str, subject: &str) {
    let checkout = &repository.checkout;
    git(world, checkout, &["checkout", "-b", branch]);
    std::fs::write(checkout.join(file), format!("{file}\n")).expect("the work is written");
    git(world, checkout, &["add", "-A"]);
    git(
        world,
        checkout,
        &["commit", "-m", &format!("{subject}\n\nWhy {file} lands.")],
    );
    git(world, checkout, &["checkout", "main"]);
}

/// A landing this journey started: the process, in a group of its own, and the
/// files its streams go to.
struct Started {
    child: Child,
    stdout: PathBuf,
    stderr: PathBuf,
}

impl Started {
    /// Start `onepipeline <args>` from `dir` (the world's root where none is
    /// given), its streams to files, in a process group of its own.
    fn spawn(world: &World, dir: Option<&Path>, args: &[&str]) -> Self {
        use std::os::unix::process::CommandExt;
        let name = format!("landing-{}", args.join("-").replace(['/', ' '], "_"));
        let stdout = world.root.join(format!("{name}.stdout"));
        let stderr = world.root.join(format!("{name}.stderr"));
        let mut command = world.cmd(args);
        if let Some(dir) = dir {
            command.current_dir(dir);
        }
        command.process_group(0);
        let child = command
            .stdout(std::fs::File::create(&stdout).expect("a stdout file"))
            .stderr(std::fs::File::create(&stderr).expect("a stderr file"))
            .spawn()
            .expect("the landing starts");
        Self {
            child,
            stdout,
            stderr,
        }
    }

    /// Kill the landing process — and only it — with `SIGKILL`.
    fn kill(&mut self) {
        let pid = i32::try_from(self.child.id()).expect("a pid fits");
        // SAFETY: `kill` takes an id and a signal and touches no memory; the id is
        // the landing's own, a child of this journey.
        let sent = unsafe { libc::kill(pid, libc::SIGKILL) };
        assert_eq!(sent, 0, "the landing could not be killed");
        let _ = self.child.wait();
    }

    /// Wait until `verb`'s repository hook holds the landing's publishing push,
    /// failing with what the landing said if it ended first.
    fn held_at_push(&mut self, verb: &Verb) {
        let mut ended = None;
        verb.world.until("the publishing push to be held", |_| {
            if ended.is_none() {
                ended = self.child.try_wait().ok().flatten();
            }
            verb.held.holding() || ended.is_some()
        });
        if let Some(status) = ended {
            panic!(
                "{} ended {status} before its push was held:\n{}\n{}",
                verb.operation,
                std::fs::read_to_string(&self.stdout).unwrap_or_default(),
                std::fs::read_to_string(&self.stderr).unwrap_or_default()
            );
        }
    }

    /// Wait for the landing to exit, and answer its status and both streams.
    fn ended(mut self) -> (i32, String, String) {
        let status = self.child.wait().expect("the landing is waited for");
        (
            status.code().unwrap_or(-1),
            std::fs::read_to_string(&self.stdout).unwrap_or_default(),
            std::fs::read_to_string(&self.stderr).unwrap_or_default(),
        )
    }
}

/// Every landing registered under every flow of `world`'s runs root, as
/// `(flow, landing)`.
fn landings(world: &World) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let Ok(flows) = std::fs::read_dir(world.runs.join(".flows")) else {
        return found;
    };
    for flow in flows.flatten().filter(|entry| entry.path().is_dir()) {
        let Ok(landings) = std::fs::read_dir(flow.path().join("landings")) else {
            continue;
        };
        for landing in landings.flatten() {
            let name = landing.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') && landing.path().join("request.json").is_file() {
                found.push((flow.file_name().to_string_lossy().into_owned(), name));
            }
        }
    }
    found.sort();
    found
}

/// The one landing `world` holds, once it is registered.
fn the_landing(world: &World) -> (String, String) {
    world.until("the landing to register", |world| {
        landings(world).len() == 1
    });
    landings(world).remove(0)
}

/// A landing's own directory.
fn landing_dir(world: &World, flow: &str, landing: &str) -> PathBuf {
    world
        .runs
        .join(".flows")
        .join(flow)
        .join("landings")
        .join(landing)
}

/// A landing's sidecar `name`, as JSON.
fn sidecar(world: &World, flow: &str, landing: &str, name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(landing_dir(world, flow, landing).join(name))
            .unwrap_or_else(|e| panic!("{name} of {landing} reads: {e}")),
    )
    .expect("a sidecar is JSON")
}

/// Every record of a landing's journal, in the order it was written.
fn journal(world: &World, flow: &str, landing: &str) -> Vec<Value> {
    std::fs::read_to_string(landing_dir(world, flow, landing).join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The kinds of a landing's journal, in order.
fn kinds(world: &World, flow: &str, landing: &str) -> Vec<String> {
    journal(world, flow, landing)
        .iter()
        .map(|event| event["kind"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// One `watch --flow` run to its return, and what it said.
struct Returned {
    code: i32,
    record: Value,
    human: String,
}

impl Returned {
    fn cursor(&self) -> String {
        self.record["cursor"]
            .as_str()
            .unwrap_or_else(|| panic!("every return carries a cursor: {}", self.record))
            .to_owned()
    }

    /// Assert the status and the condition, and that the human line carries the
    /// cursor the record does.
    fn ended_on(&self, code: i32, condition: &str) -> &Self {
        assert_eq!(
            (self.code, self.record["condition"].as_str()),
            (code, Some(condition)),
            "{}\n{}",
            self.record,
            self.human
        );
        assert!(
            self.human.contains(&format!("cursor {}", self.cursor())),
            "{}",
            self.human
        );
        self
    }

    /// Assert a `landing-progress` return reporting `kind`.
    fn progressed(&self, kind: &str) -> &Self {
        self.ended_on(EXIT_SUCCESS, "landing-progress");
        assert_eq!(self.record["kind"], json!(kind), "{}", self.record);
        self
    }
}

/// `watch --flow <flow> <args>`, run to its return.
fn watch(world: &World, flow: &str, args: &[&str]) -> Returned {
    let mut argv = vec!["watch", "--flow", flow];
    argv.extend_from_slice(args);
    let ran = world.run(&argv);
    let record = ran
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["watch"] == "return")
        .unwrap_or_else(|| {
            panic!(
                "the watch printed no return: {}\n{}",
                ran.stdout, ran.stderr
            )
        });
    Returned {
        code: ran.code,
        record,
        human: ran.stderr.clone(),
    }
}

/// A `watch --flow` this journey armed in the background.
struct Armed {
    child: Child,
}

impl Armed {
    /// Arm `watch --flow <flow> <args>` and return once its lease is on disk, or
    /// once it already returned.
    fn arm(world: &World, flow: &str, args: &[&str]) -> Self {
        let mut argv = vec!["watch", "--flow", flow];
        argv.extend_from_slice(args);
        let mut child = world
            .cmd(&argv)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the watch starts");
        let mine = format!("{}-", child.id());
        let leases = world.runs.join(".flows").join(flow).join("watchers");
        world.until("the flow watch to record itself", |_| {
            std::fs::read_dir(&leases).is_ok_and(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with(&mine))
                })
            }) || child.try_wait().is_ok_and(|exited| exited.is_some())
        });
        Self { child }
    }

    /// Whether it is still waiting.
    fn waiting(&mut self) -> bool {
        self.child.try_wait().is_ok_and(|exited| exited.is_none())
    }

    fn returned(self) -> Returned {
        let output = self.child.wait_with_output().expect("the watch returns");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let record = stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|record| record["watch"] == "return")
            .unwrap_or_else(|| panic!("the watch printed no return: {stdout}"));
        Returned {
            code: output.status.code().unwrap_or(-1),
            record,
            human: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

/// Re-arm the default `watch --flow` from `cursor` until the flow's own ending,
/// answering every landing notice it woke on as `(kind, significance)`.
fn rearmed_until_ended(world: &World, flow: &str, mut cursor: String) -> Vec<(String, String)> {
    let mut woken = Vec::new();
    loop {
        let returned = watch(world, flow, &["--timeout", "600", "--cursor", &cursor]);
        match returned.record["condition"].as_str() {
            Some("landing-progress") => {
                returned.ended_on(EXIT_SUCCESS, "landing-progress");
                woken.push((
                    returned.record["kind"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    returned.record["significance"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                ));
                cursor = returned.cursor();
            }
            Some("ended") => {
                returned.ended_on(EXIT_SUCCESS, "ended");
                return woken;
            }
            _ => panic!("the flow did not end cleanly: {}", returned.record),
        }
    }
}

/// The one verdict `stop-guard` answers for `world`'s session.
fn verdict(world: &World) -> Value {
    let asked = world.run(&["stop-guard", "--session", world.session.as_str()]);
    asked.exited(0);
    serde_json::from_str(asked.stdout.trim()).expect("one verdict object")
}

/// Read a flow's whole event view through `--filter detailed`, as `monitor`
/// prints it.
fn detailed_monitor(world: &World, flow: &str) -> String {
    let read = world.run(&["monitor", "--flow", flow, "--filter", "detailed"]);
    read.exited(0);
    read.stdout.clone()
}

/// The world's single repository under `publication`, whose own `pre-push` hook
/// holds every publishing push until the journey releases it.
fn held_world(name: &str, publication: &str) -> (World, Repository, HeldPublication) {
    let world = World::new(name);
    let held = held_publication(&world, &world.root.join("push.go"));
    let repository = world.repository(publication, &held.argv());
    (world, repository, held)
}

/// The journey the verbs exist for, through `publish-branch`: a landing a
/// manager starts directly is that manager's supervised work from before its
/// first side effect to its ending.
///
/// Held at the drafting turn, the landing is already registered: its request
/// names the session, the verb and the subject, and its journal holds
/// `landing-registered` before `landing-drafting`. The stop guard holds the
/// session for it. The default watch, read once, does not return on that
/// progress; `--filter detailed` returns each transition in turn and its cursor
/// never replays one. Held at the push, the operation is prepared and its id
/// kept; the operation's own events reach the landing's journal while it runs.
/// Released, the change it opens wakes the default watch as a milestone, and
/// the landing's flow then ends `0`, owed nothing.
#[test]
fn publish_branch_is_owned_before_it_drafts_and_its_watch_wakes_on_milestones_alone() {
    let (world, repository, held) = held_world("ml-publish", "change-open");
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    world.script("pr-author.body", "## What\nA widget.\n");
    world.script("pr-author.records-tree", "1");
    world.script("pr-author.holds", "1");
    let graph = world.pr_author_graph();

    let landing = Started::spawn(
        &world,
        None,
        &[
            "publish-branch",
            BRANCH,
            "--repo",
            "service",
            "--title",
            "feat: add the widget",
            "--pr-author-graph",
            &graph,
        ],
    );
    // The drafting turn is in flight: the landing is held at its first paid step.
    let tree = world.fakes.join("pr-author-tree.jsonl");
    world.until("the drafting turn to be in flight", |_| {
        std::fs::read_to_string(&tree).is_ok_and(|said| said.ends_with('\n'))
    });
    let (flow, id) = the_landing(&world);
    assert_eq!(flow, "publish-branch");
    let request = sidecar(&world, &flow, &id, "request.json");
    assert_eq!(request["schema_version"], json!(1));
    assert_eq!(request["landing_id"], json!(id));
    assert_eq!(request["flow_id"], json!(flow));
    assert_eq!(request["session"], json!(world.session));
    assert_eq!(request["operation"], json!("publish-branch"));
    assert_eq!(request["subjects"][0]["branch"], json!(BRANCH));
    assert_eq!(
        request["subjects"][0]["identity"],
        json!(world.identity(&repository.checkout).origin)
    );
    assert_eq!(
        kinds(&world, &flow, &id),
        ["landing-registered", "landing-drafting"],
        "ownership is recorded before the drafting turn"
    );
    assert!(
        !landing_dir(&world, &flow, &id)
            .join("operation.json")
            .exists(),
        "nothing is forwarded before the body is drafted"
    );

    // The guard holds the session for the live landing nothing watches.
    let blocked = verdict(&world);
    assert_eq!(blocked["verdict"], json!("block"), "{blocked}");
    assert!(
        blocked["reason"]
            .as_str()
            .is_some_and(|why| why.contains(&format!("onepipeline watch --flow {flow}"))),
        "{blocked}"
    );

    // Read once through the default profile, the progress wakes nothing.
    watch(&world, &flow, &["--timeout", "0"]).ended_on(ELAPSED, "elapsed");
    // Through `detailed`, each transition is returned once, in order.
    let registered = watch(&world, &flow, &["--timeout", "0", "--filter", "detailed"]);
    registered.progressed("landing-registered");
    assert_eq!(registered.record["landing_id"], json!(id));
    assert_eq!(registered.record["significance"], json!("progress"));
    assert_eq!(registered.record["subjects"], request["subjects"]);
    let drafting = watch(
        &world,
        &flow,
        &[
            "--timeout",
            "0",
            "--filter",
            "detailed",
            "--cursor",
            &registered.cursor(),
        ],
    );
    drafting.progressed("landing-drafting");
    let caught_up = watch(
        &world,
        &flow,
        &[
            "--timeout",
            "0",
            "--filter",
            "detailed",
            "--cursor",
            &drafting.cursor(),
        ],
    );
    caught_up.ended_on(ELAPSED, "elapsed");

    // The body is drafted; the landing forwards and is held at the push.
    std::fs::write(world.fakes.join("pr-author.go"), "go").expect("the draft is released");
    world.until("the publishing push to be held", |_| held.holding());
    let operation = sidecar(&world, &flow, &id, "operation.json");
    let operation_id = operation["operation_id"].as_str().expect("an operation id");
    assert!(operation_id.starts_with("o-"), "{operation}");
    let started = watch(
        &world,
        &flow,
        &[
            "--timeout",
            "0",
            "--filter",
            "detailed",
            "--cursor",
            &caught_up.cursor(),
        ],
    );
    started.progressed("landing-started");
    assert_eq!(started.record["operation_id"], json!(operation_id));
    let relayed: Vec<Value> = journal(&world, &flow, &id)
        .into_iter()
        .filter(|event| event["source"] == "vcs")
        .collect();
    assert!(
        !relayed.is_empty(),
        "the operation's events reach the landing while it runs: {:?}",
        kinds(&world, &flow, &id)
    );
    for event in &relayed {
        assert_eq!(
            event["labels"]["operation_id"],
            json!(operation_id),
            "{event}"
        );
        assert_eq!(event["labels"]["landing_id"], json!(id), "{event}");
        assert_eq!(event["labels"]["flow_id"], json!(flow), "{event}");
        assert!(event["labels"]["significance"].is_string(), "{event}");
    }

    // Armed on the default profile from the latest cursor, released: the change
    // it opens wakes it, as a milestone.
    let mut armed = Armed::arm(
        &world,
        &flow,
        &["--timeout", "600", "--cursor", &started.cursor()],
    );
    assert!(armed.waiting(), "the default watch returned on progress");
    held.release();
    let opened = armed.returned();
    opened.progressed("change-opened");
    assert_eq!(opened.record["source"], json!("vcs"), "{}", opened.record);
    assert_eq!(opened.record["significance"], json!("milestone"));
    assert_eq!(opened.record["subjects"], request["subjects"]);
    assert_eq!(opened.record["operation_id"], json!(operation_id));
    let (code, _, said) = landing.ended();
    assert_eq!(code, 0, "{said}");
    assert!(
        said.contains(&format!(
            "flow {flow}: watch it with: onepipeline watch --flow {flow}"
        )),
        "{said}"
    );
    // Re-armed from each cursor, the default watch wakes on every later
    // milestone — never on progress — and ends on the flow's own ending, which
    // reports the landing's: `ended`, not a landing notice.
    let woken = rearmed_until_ended(&world, &flow, opened.cursor());
    assert!(
        woken
            .iter()
            .all(|(_, significance)| significance != "progress"),
        "{woken:?}"
    );
    assert_eq!(
        sidecar(&world, &flow, &id, "ending.json")["status"],
        json!(0)
    );
    let all = kinds(&world, &flow, &id);
    assert_eq!(all.first().map(String::as_str), Some("landing-registered"));
    assert_eq!(all.last().map(String::as_str), Some("landing-finished"));
    assert!(all.iter().any(|kind| kind == "push"), "{all:?}");
    // The detailed reader holds every one of them.
    let shown = detailed_monitor(&world, &flow);
    for kind in &all {
        assert!(shown.contains(kind.as_str()), "{kind} is not in:\n{shown}");
    }
    assert_eq!(verdict(&world), json!({"verdict": "none"}));
    let _ = (onevcs_binary(), REFUSED, EXIT_FLOW_FAILED, OWED);
}

/// One of the three verbs, set up over its own held repository: the world, the
/// repository and its hold, and the command line that lands it from where.
struct Verb {
    world: World,
    repository: Repository,
    held: HeldPublication,
    args: Vec<String>,
    from: Option<PathBuf>,
    operation: &'static str,
}

impl Verb {
    /// `publish-branch` of a completed branch, with no drafting turn.
    fn publish(name: &str) -> Self {
        let (world, repository, held) = held_world(name, "change-open");
        branch_with_work(&world, &repository, BRANCH, "widget.txt");
        Self {
            world,
            repository,
            held,
            args: words(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"]),
            from: None,
            operation: "publish-branch",
        }
    }

    /// `repo-recover` of a branch a step left preserved on the origin.
    fn recover(name: &str) -> Self {
        let (world, repository, held) = held_world(name, "change-open");
        // Its commit marked an incomplete step, as a run's publication leaves
        // interrupted work, which is what `recover` lands and attests.
        branch_under(
            &world,
            &repository,
            BRANCH,
            "widget.txt",
            "feat: add the widget (incomplete step)",
        );
        let preserved = world
            .cmd_on(&onevcs_binary(), &["preserve", BRANCH, "--repo", "service"])
            .output()
            .expect("onevcs runs");
        assert!(
            preserved.status.success(),
            "{}",
            String::from_utf8_lossy(&preserved.stderr)
        );
        Self {
            world,
            repository,
            held,
            args: words(&[
                "repo-recover",
                BRANCH,
                "--repo",
                "service",
                "--title",
                "feat: add the widget",
                "--no-draft",
            ]),
            from: None,
            operation: "repo-recover",
        }
    }

    /// `integrate` of an ordered train of three branches, pushing the base,
    /// from the registered checkout.
    fn integrate(name: &str) -> Self {
        Self::integrate_under(name, "local-direct")
    }

    /// [`integrate`](Self::integrate), in an identity publishing `publication`.
    fn integrate_under(name: &str, publication: &str) -> Self {
        let (world, repository, held) = held_world(name, publication);
        for branch in TRAIN {
            branch_with_work(&world, &repository, branch, &format!("{branch}.txt"));
        }
        let mut args = vec!["integrate".to_owned()];
        args.extend(TRAIN.iter().map(|branch| (*branch).to_owned()));
        args.push("--push".to_owned());
        let from = Some(repository.checkout.clone());
        Self {
            world,
            repository,
            held,
            args,
            from,
            operation: "integrate",
        }
    }

    fn start(&self) -> Started {
        let args: Vec<&str> = self.args.iter().map(String::as_str).collect();
        Started::spawn(&self.world, self.from.as_deref(), &args)
    }
}

/// The ordered train `integrate` lands.
const TRAIN: [&str; 3] = ["train-a", "train-b", "train-c"];

fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// Each verb, held at its publishing push, is registered with every subject —
/// the whole train for `integrate`, in order — and an operation already
/// prepared and relaying; released, it finishes, its owner's default watch
/// wakes only on milestones, its flow ends `0`, and nothing is owed.
#[test]
fn each_verb_held_mid_operation_is_supervised_and_finishes_under_its_owners_watch() {
    for verb in [
        Verb::publish("ml-ok-publish"),
        Verb::recover("ml-ok-recover"),
        Verb::integrate("ml-ok-integrate"),
    ] {
        let world = &verb.world;
        let mut landing = verb.start();
        landing.held_at_push(&verb);
        let (flow, id) = the_landing(world);
        assert_eq!(flow, verb.operation);
        let request = sidecar(world, &flow, &id, "request.json");
        assert_eq!(request["operation"], json!(verb.operation));
        let branches: Vec<&str> = request["subjects"]
            .as_array()
            .expect("subjects")
            .iter()
            .map(|subject| subject["branch"].as_str().expect("a branch"))
            .collect();
        let identity = world.identity(&verb.repository.checkout).origin;
        for subject in request["subjects"].as_array().expect("subjects") {
            assert_eq!(subject["identity"], json!(identity), "{request}");
        }
        match verb.operation {
            "integrate" => assert_eq!(branches, TRAIN, "every member of the train, in order"),
            _ => assert_eq!(branches, [BRANCH]),
        }
        let operation = sidecar(world, &flow, &id, "operation.json");
        let started = kinds(world, &flow, &id);
        assert_eq!(
            &started[..2],
            ["landing-registered", "landing-started"],
            "{started:?}"
        );
        // The default profile, read once, wakes on neither; the detailed reader
        // returns each in turn, and its cursor resumes past exactly what it read.
        watch(world, &flow, &["--timeout", "0"]).ended_on(ELAPSED, "elapsed");
        let registered = watch(world, &flow, &["--timeout", "0", "--filter", "detailed"]);
        registered.progressed("landing-registered");
        assert_eq!(registered.record["subjects"], request["subjects"]);
        let forwarded = watch(
            world,
            &flow,
            &[
                "--timeout",
                "0",
                "--filter",
                "detailed",
                "--cursor",
                &registered.cursor(),
            ],
        );
        forwarded.progressed("landing-started");
        assert_eq!(forwarded.record["operation_id"], operation["operation_id"]);

        // Live and unwatched, the session is held; a qualifying watch clears it.
        assert_eq!(verdict(world)["verdict"], json!("block"));
        let mut armed = Armed::arm(world, &flow, &["--timeout", "600"]);
        assert_eq!(verdict(world), json!({"verdict": "none"}));
        assert!(
            armed.waiting(),
            "{}: the default watch woke on progress",
            verb.operation
        );

        verb.held.release();
        let first = armed.returned();
        first.ended_on(EXIT_SUCCESS, "landing-progress");
        assert_ne!(
            first.record["significance"],
            json!("progress"),
            "{}",
            first.record
        );
        let mut woken = vec![(
            first.record["kind"].as_str().unwrap_or_default().to_owned(),
            first.record["significance"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )];
        woken.extend(rearmed_until_ended(world, &flow, first.cursor()));
        let (code, out, said) = landing.ended();
        assert_eq!(code, 0, "{}: {out}\n{said}", verb.operation);
        assert!(
            woken
                .iter()
                .all(|(_, significance)| significance != "progress"),
            "{woken:?}"
        );
        assert!(
            woken
                .iter()
                .any(|(kind, _)| kind == "base-arrived" || kind == "change-opened"),
            "{}: no landing milestone woke the owner: {woken:?}",
            verb.operation
        );
        if verb.operation == "integrate" {
            // One train reaching its base is one notice, not one per member.
            let arrivals = woken
                .iter()
                .filter(|(kind, _)| kind == "base-arrived")
                .count();
            assert_eq!(arrivals, 1, "{woken:?}");
            assert!(out.contains("Integration train for"), "{out}");
            for branch in TRAIN {
                assert!(out.contains(&format!("  {branch}: merged")), "{out}");
            }
            assert!(out.contains("Pushed: yes"), "{out}");
        }
        let every = kinds(world, &flow, &id);
        assert_eq!(every.last().map(String::as_str), Some("landing-finished"));
        for event in journal(world, &flow, &id)
            .iter()
            .filter(|event| event["source"] == "vcs")
        {
            assert_eq!(
                event["labels"]["operation_id"], operation["operation_id"],
                "{event}"
            );
        }
        assert_eq!(
            sidecar(world, &flow, &id, "ending.json")["status"],
            json!(0)
        );
        assert_eq!(verdict(world), json!({"verdict": "none"}));
        world
            .run(&["unwatched", "--session", &world.session])
            .exited(EXIT_SUCCESS);
    }
}

/// Each verb whose forwarded command is refused ends its landing refused — a
/// blocker carrying the forwarded status — and its own flow `flow-failed`,
/// owed closure until its owner acknowledges it.
#[test]
fn each_verb_refused_by_onevcs_ends_refused_and_owes_closure() {
    for (verb, args) in [
        (
            Verb::publish("ml-no-publish"),
            words(&[
                "publish-branch",
                "missing",
                "--repo",
                "service",
                "--no-draft",
            ]),
        ),
        (
            Verb::recover("ml-no-recover"),
            words(&["repo-recover", "missing", "--repo", "service", "--no-draft"]),
        ),
        (
            // A train is refused for an identity whose rules do not publish
            // `local-direct`; it is resolved and registered first.
            Verb::integrate_under("ml-no-integrate", "change-open"),
            words(&["integrate", "train-a", "train-b", "--push"]),
        ),
    ] {
        let world = &verb.world;
        verb.held.release();
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let started = Started::spawn(world, verb.from.as_deref(), &argv);
        let (code, out, said) = started.ended();
        let (flow, id) = the_landing(world);
        let ending = sidecar(world, &flow, &id, "ending.json");
        assert_eq!(ending["status"], json!(code), "{out}\n{said}");
        assert_ne!(code, 0, "{}: {out}\n{said}", verb.operation);
        let every = journal(world, &flow, &id);
        let last = every.last().expect("a journal");
        assert_eq!(last["kind"], json!("landing-refused"), "{every:?}");
        assert_eq!(last["payload"]["status"], json!(code));
        assert_eq!(last["labels"]["significance"], json!("blocker"));
        // The flow's ending reports it, and the owner owes closure.
        let failed = watch(world, &flow, &["--timeout", "0"]);
        failed.ended_on(EXIT_FLOW_FAILED, "flow-failed");
        assert_eq!(failed.record["status"], json!(code));
        let owed = world.run(&["unwatched", "--session", &world.session]);
        owed.exited(OWED);
        assert!(
            owed.stdout.contains(&format!("flow {flow}")),
            "{}",
            owed.stdout
        );
        assert_eq!(verdict(world)["verdict"], json!("block"));
        world
            .run(&[
                "unwatched",
                "--session",
                &world.session,
                "--acknowledge-flow",
                &flow,
                "--reason",
                "the branch does not exist; nothing to land",
            ])
            .exited(EXIT_SUCCESS);
        assert_eq!(verdict(world), json!({"verdict": "none"}));
    }
}

/// Each verb killed mid-operation — after the handshake proved its process
/// live — is reported dead, never finished: the owner's watch returns
/// `flow-failed` naming the death, the death is written once with the
/// operation's recorded evidence kept, `unwatched` owes the flow closure rather
/// than a watch, and the owner's acknowledgement releases the stop guard.
#[test]
fn each_verb_killed_mid_operation_is_reported_dead_and_closed_by_its_owner() {
    for verb in [
        Verb::publish("ml-kill-publish"),
        Verb::recover("ml-kill-recover"),
        Verb::integrate("ml-kill-integrate"),
    ] {
        let world = &verb.world;
        let mut landing = verb.start();
        landing.held_at_push(&verb);
        let (flow, id) = the_landing(world);
        let mut armed = Armed::arm(world, &flow, &["--timeout", "600"]);
        assert!(armed.waiting(), "the default watch woke on progress");
        let relayed_before: Vec<Value> = journal(world, &flow, &id)
            .into_iter()
            .filter(|event| event["source"] == "vcs")
            .collect();
        assert!(!relayed_before.is_empty(), "{}", verb.operation);

        landing.kill();
        let died = armed.returned();
        died.ended_on(EXIT_FLOW_FAILED, "flow-failed");
        assert_eq!(died.record["died"], json!(true), "{}", died.record);
        assert!(
            !landing_dir(world, &flow, &id).join("ending.json").exists(),
            "a death is never recorded as an ending"
        );
        let every = journal(world, &flow, &id);
        let deaths: Vec<&Value> = every
            .iter()
            .filter(|event| event["kind"] == "landing-died")
            .collect();
        assert_eq!(deaths.len(), 1, "{every:?}");
        assert_eq!(deaths[0]["labels"]["significance"], json!("blocker"));
        assert!(
            !every
                .iter()
                .any(|event| event["kind"] == "landing-finished"),
            "a dead landing was read as finished"
        );
        // What the operation recorded before it died is kept, once each.
        for before in &relayed_before {
            let copies = every
                .iter()
                .filter(|event| {
                    event["stream"] == before["stream"] && event["seq"] == before["seq"]
                })
                .count();
            assert_eq!(copies, 1, "{before}");
        }
        // Read again, nothing is written twice.
        watch(world, &flow, &["--timeout", "0"]).ended_on(EXIT_FLOW_FAILED, "flow-failed");
        assert_eq!(
            journal(world, &flow, &id)
                .iter()
                .filter(|event| event["kind"] == "landing-died")
                .count(),
            1
        );

        // Closure, not a watch, is what is owed now.
        let owed = world.run(&["unwatched", "--session", &world.session]);
        owed.exited(OWED);
        assert!(
            owed.stdout.contains("DIED") && owed.stdout.contains("--acknowledge-flow"),
            "{}",
            owed.stdout
        );
        let blocked = verdict(world);
        assert_eq!(blocked["verdict"], json!("block"), "{blocked}");
        assert!(
            blocked["reason"]
                .as_str()
                .is_some_and(|why| why.contains("--acknowledge-flow")
                    && !why.contains(&format!("onepipeline watch --flow {flow}"))),
            "{blocked}"
        );
        world
            .run(&[
                "unwatched",
                "--session",
                &world.session,
                "--acknowledge-flow",
                &flow,
                "--reason",
                "killed by hand; the branch is landed again later",
            ])
            .exited(EXIT_SUCCESS);
        assert_eq!(verdict(world), json!({"verdict": "none"}));
        verb.held.release();
    }
}

/// A `change-auto` world whose check watch waits as long as the journey holds
/// it: the host reports `checks` until the journey scripts another answer.
fn hosted(name: &str, checks: &str) -> (World, Repository) {
    let world = World::new(name).with_env("ONEVCS_CHECKS_TIMEOUT_SECONDS", "900");
    let repository = world.repository("change-auto", &[]);
    world.script("gh.checks", checks);
    (world, repository)
}

/// Wait until the landing's journal records a `change-check` of `name` at
/// `state`: the handshake that the host's answer was read before the journey
/// moves the check sequence on.
fn check_recorded(world: &World, flow: &str, landing: &str, name: &str, state: &str) {
    world.until(
        &format!("the check {name} to be recorded {state}"),
        |world| {
            journal(world, flow, landing).iter().any(|event| {
                event["kind"] == "change-check"
                    && event["payload"]["name"] == name
                    && event["payload"]["state"] == state
            })
        },
    );
}

/// The two milestones a `change-auto` landing opens with, in order — the change
/// opened, and opened as a draft — each waking the owner's default watch, and
/// the second's return, whose cursor is past both.
fn opened_as_draft(world: &World, flow: &str) -> Returned {
    let opened = Armed::arm(world, flow, &["--timeout", "900"]).returned();
    opened.progressed("change-opened");
    assert_eq!(opened.record["significance"], json!("milestone"));
    let drafted = Armed::arm(
        world,
        flow,
        &["--timeout", "900", "--cursor", &opened.cursor()],
    )
    .returned();
    drafted.progressed("change-drafted");
    assert_eq!(drafted.record["significance"], json!("milestone"));
    drafted
}

/// The two hosted verbs over a change request whose third-party checks move on
/// the journey's word: the owner's default watch wakes on the change opening
/// and on its checks settling — milestones — and on nothing between; each
/// check's movement is progress, recorded and read by the detailed reader while
/// that watch stays armed; the landing reaching its base is one milestone, and
/// the final result is the flow's own clean ending.
#[test]
fn a_hosted_landing_wakes_its_owner_on_opened_and_settled_checks_and_not_on_their_progress() {
    for verb in ["publish-branch", "repo-recover"] {
        let (world, repository) = hosted(
            &format!("ml-hosted-{verb}"),
            "lint in_progress - required\nunit queued - required\ndocs queued - advisory",
        );
        let args = match verb {
            "publish-branch" => {
                branch_with_work(&world, &repository, BRANCH, "widget.txt");
                words(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"])
            }
            _ => {
                branch_under(
                    &world,
                    &repository,
                    BRANCH,
                    "widget.txt",
                    "feat: add the widget (incomplete step)",
                );
                let preserved = world
                    .cmd_on(&onevcs_binary(), &["preserve", BRANCH, "--repo", "service"])
                    .output()
                    .expect("onevcs runs");
                assert!(preserved.status.success());
                words(&[
                    "repo-recover",
                    BRANCH,
                    "--repo",
                    "service",
                    "--title",
                    "feat: add the widget",
                    "--no-draft",
                ])
            }
        };
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let landing = Started::spawn(&world, None, &argv);
        let (flow, id) = the_landing(&world);

        // The change opening — as a draft, under this policy — is the first thing
        // that wakes the owner.
        let opened = opened_as_draft(&world, &flow);
        check_recorded(&world, &flow, &id, "lint", "pending");

        // Armed again from that cursor, the default watch waits through every
        // check's movement, which the detailed reader is handed.
        let mut armed = Armed::arm(
            &world,
            &flow,
            &["--timeout", "900", "--cursor", &opened.cursor()],
        );
        world.script(
            "gh.checks",
            "lint completed success required\nunit in_progress - required\ndocs queued - advisory",
        );
        check_recorded(&world, &flow, &id, "lint", "passed");
        let detailed = watch(
            &world,
            &flow,
            &[
                "--timeout",
                "0",
                "--filter",
                "detailed",
                "--cursor",
                &opened.cursor(),
            ],
        );
        detailed.ended_on(EXIT_SUCCESS, "landing-progress");
        assert_eq!(
            detailed.record["significance"],
            json!("progress"),
            "{}",
            detailed.record
        );
        assert!(
            armed.waiting(),
            "{verb}: the default watch woke on a check's progress"
        );

        // Every required check passes, and the host lands the change it armed.
        world.script("gh.merged", "");
        world.script(
            "gh.checks",
            "lint completed success required\nunit completed success required\ndocs completed success advisory",
        );
        let settled = armed.returned();
        settled.progressed("checks-settled");
        assert_eq!(settled.record["significance"], json!("milestone"));
        let woken = rearmed_until_ended(&world, &flow, settled.cursor());
        let (code, out, said) = landing.ended();
        assert_eq!(code, 0, "{verb}: {out}\n{said}");
        assert!(
            woken
                .iter()
                .all(|(_, significance)| significance != "progress"),
            "{verb}: {woken:?}"
        );
        // One notice of the landing reaching its base, however many steps it took.
        let merged = woken
            .iter()
            .filter(|(kind, _)| kind == "change-merged")
            .count();
        assert!(merged <= 1, "{verb}: {woken:?}");
        // The detailed reader holds every step the producer recorded, merge steps
        // included, whatever the default watch left out.
        let all = kinds(&world, &flow, &id);
        for kind in ["change-opened", "change-check", "checks-settled", "push"] {
            assert!(
                all.iter().any(|recorded| recorded == kind),
                "{verb}: {kind} not in {all:?}"
            );
        }
        let shown = detailed_monitor(&world, &flow);
        for kind in &all {
            assert!(
                shown.contains(kind.as_str()),
                "{verb}: {kind} not in:\n{shown}"
            );
        }
    }
}

/// A required check failing wakes the owner at once — a blocker — while the
/// other required check is still pending and the landing is still watching it:
/// the failure is the event the watch reports, not the landing's later ending.
#[test]
fn a_required_check_failing_wakes_the_owner_while_its_other_checks_are_still_pending() {
    for verb in ["publish-branch", "repo-recover"] {
        let (world, repository) = hosted(
            &format!("ml-red-{verb}"),
            "lint in_progress - required\nunit in_progress - required",
        );
        let args = match verb {
            "publish-branch" => {
                branch_with_work(&world, &repository, BRANCH, "widget.txt");
                words(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"])
            }
            _ => {
                branch_under(
                    &world,
                    &repository,
                    BRANCH,
                    "widget.txt",
                    "feat: add the widget (incomplete step)",
                );
                let preserved = world
                    .cmd_on(&onevcs_binary(), &["preserve", BRANCH, "--repo", "service"])
                    .output()
                    .expect("onevcs runs");
                assert!(preserved.status.success());
                words(&[
                    "repo-recover",
                    BRANCH,
                    "--repo",
                    "service",
                    "--title",
                    "feat: add the widget",
                    "--no-draft",
                ])
            }
        };
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let landing = Started::spawn(&world, None, &argv);
        let (flow, id) = the_landing(&world);
        let opened = opened_as_draft(&world, &flow);
        check_recorded(&world, &flow, &id, "unit", "pending");
        let armed = Armed::arm(
            &world,
            &flow,
            &["--timeout", "900", "--cursor", &opened.cursor()],
        );

        world.script(
            "gh.checks",
            "lint completed failure required\nunit in_progress - required",
        );
        let red = armed.returned();
        red.progressed("change-check");
        assert_eq!(
            red.record["significance"],
            json!("blocker"),
            "{}",
            red.record
        );
        // At the moment it was recorded, the other required check had not
        // settled: nothing in the journal before it says otherwise.
        let journal_now = journal(&world, &flow, &id);
        let at = journal_now
            .iter()
            .position(|event| {
                event["kind"] == "change-check"
                    && event["payload"]["name"] == "lint"
                    && event["labels"]["significance"] == "blocker"
            })
            .expect("the failure is journalled");
        assert!(
            !journal_now[..at]
                .iter()
                .any(|event| event["kind"] == "checks-settled"),
            "{verb}: the checks were settled before the failure"
        );
        assert_eq!(
            journal_now[..at]
                .iter()
                .rev()
                .find(|event| event["kind"] == "change-check" && event["payload"]["name"] == "unit")
                .map(|event| event["payload"]["state"].clone()),
            Some(json!("pending")),
            "{verb}"
        );
        // The landing then ends refused: every later notice is a blocker, and
        // its flow's ending says so.
        let (code, _, said) = landing.ended();
        assert_ne!(code, 0, "{verb}: {said}");
        let mut cursor = red.cursor();
        loop {
            let next = watch(&world, &flow, &["--timeout", "0", "--cursor", &cursor]);
            if next.record["condition"] == "flow-failed" {
                next.ended_on(EXIT_FLOW_FAILED, "flow-failed");
                break;
            }
            next.ended_on(EXIT_SUCCESS, "landing-progress");
            assert_eq!(
                next.record["significance"],
                json!("blocker"),
                "{}",
                next.record
            );
            cursor = next.cursor();
        }
    }
}

/// Run the real `onevcs` binary against `world`, answering its stdout.
fn onevcs(world: &World, args: &[&str]) -> String {
    let output = world
        .cmd_on(&onevcs_binary(), args)
        .output()
        .expect("onevcs runs");
    assert!(
        output.status.success(),
        "onevcs {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Leave `branch` of `alias` preserved on its origin by a closed session that
/// `launcher` opened — a branch a manager owes until it is landed or
/// acknowledged.
fn preserved_by_session(world: &World, alias: &str, branch: &str, launcher: &str) {
    let opened = onevcs(
        world,
        &[
            "session",
            "open",
            alias,
            "--branch",
            branch,
            "--label",
            &format!("launcher={launcher}"),
        ],
    );
    let opened: Value = serde_json::from_str(opened.trim()).expect("session open answers JSON");
    let worktree = PathBuf::from(opened["worktree"].as_str().expect("a worktree"));
    std::fs::write(worktree.join(format!("{alias}.txt")), "kept\n").expect("written");
    git(world, &worktree, &["add", "-A"]);
    git(
        world,
        &worktree,
        &["commit", "-q", "-m", "feat: kept work (incomplete step)"],
    );
    onevcs(
        world,
        &[
            "session",
            "close",
            opened["token"].as_str().expect("a token"),
        ],
    );
}

/// Replace the world's `pre-push` hook with one holding every push on `go`.
fn hold_pushes(world: &World, go: &Path) {
    let hold = held_publication(world, go);
    let quoted: Vec<String> = hold
        .argv()
        .iter()
        .map(|word| format!("'{}'", word.replace('\'', r"'\''")))
        .collect();
    onepipeline_testfakes::executable(
        &world.root.join("hooks").join("pre-push"),
        format!("#!/bin/sh\nexec {}\n", quoted.join(" ")),
    );
    // The hold is the file; this value's own release is not the journey's.
    std::mem::forget(hold);
}

/// A run, owned by this world's own session, that opened a session on a branch
/// of `service` and left it unlanded — the originating run of a later manager
/// landing of that branch. Answers the branch.
fn originating_run(world: &World, run: &str) -> String {
    world.script("service.work", "the worker wrote this\n");
    let path = world.plan(
        run,
        &crate::harness::plan_of(run, vec![crate::harness::lifecycle("service", &[])]),
    );
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    let result = world.run_json(run, "result.json");
    result["nodes"][0]["branch"]
        .as_str()
        .unwrap_or_else(|| panic!("the node named no branch: {result}"))
        .to_owned()
}

/// Every record under a directory, by name and contents: what an acknowledgement
/// that was refused must leave exactly as it was.
fn snapshot(dir: &Path) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| {
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        std::fs::read_to_string(entry.path()).unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// A live landing owned by one manager is associated with the run its branch
/// came from — another manager's run — without becoming that run's: the run's
/// journal mirrors the landing's events, readable in detail and left out of its
/// planner view where they are progress, the settled run is not revived, and
/// neither manager can acknowledge away the work it is advancing while it is
/// live: not the run, not the landing's flow, not the branch — whatever was
/// acknowledged before the landing began. The same branch name in another
/// identity is unaffected. Once the landing has ended, each closes as before.
#[test]
fn a_live_landing_refuses_every_acknowledgement_that_would_hide_it_across_owners() {
    let world = World::new("ml-acks");
    let _repository = world.repository("local-direct", &["false"]);
    let branch = originating_run(&world, "origin");
    let origin_result = std::fs::read_to_string(world.run_file("origin", "result.json"))
        .expect("the settled run's result");
    let go = world.root.join("push.go");
    hold_pushes(&world, &go);
    // The same branch name, preserved in a second identity.
    let _other = world.extra_repository("other");
    preserved_by_session(&world, "other", &branch, &world.session);

    let manager = world.as_session("manager-b");
    let acknowledgements = world.root.join("acks");
    let acknowledge_branch = |who: &World, repo: Option<&str>| {
        let mut argv = vec![
            "unpublished",
            "--acknowledge",
            branch.as_str(),
            "--reason",
            "kept deliberately",
            "--session",
            who.session.as_str(),
        ];
        let directory = acknowledgements.display().to_string();
        argv.extend(["--acknowledgements", &directory]);
        if let Some(repo) = repo {
            argv.extend(["--repo", repo]);
        }
        who.run(&argv)
    };
    // Acknowledged before the landing began: it cannot answer for the landing.
    acknowledge_branch(&manager, Some("service")).exited(EXIT_SUCCESS);

    let landing = Started::spawn(
        &manager,
        None,
        &[
            "publish-branch",
            &branch,
            "--repo",
            "service",
            "--title",
            "feat: land the worker's branch",
            "--no-draft",
        ],
    );
    world.until("the landing's push to be held", |_| {
        let mut marker = go.as_os_str().to_owned();
        marker.push(".holding");
        PathBuf::from(marker).is_file()
    });
    let (flow, id) = the_landing(&world);
    let request = sidecar(&world, &flow, &id, "request.json");
    assert_eq!(request["session"], json!("manager-b"));
    assert_eq!(request["runs"], json!(["origin"]), "{request}");

    // The run's journal carries the landing, in detail only where it is
    // progress, and the run is still its own manager's and still settled.
    let mirrored = world.journal("origin");
    let registered = mirrored
        .iter()
        .find(|event| event["kind"] == "landing-registered")
        .expect("the registration is mirrored into the run");
    assert_eq!(registered["labels"]["run_id"], json!("origin"));
    assert_eq!(registered["labels"]["landing_id"], json!(id));
    assert!(
        mirrored
            .iter()
            .any(|event| event["source"] == "vcs" && event["labels"]["landing_id"] == json!(id)),
        "the operation's events are mirrored into the run"
    );
    let detailed = world.run(&["monitor", "origin", "--filter", "detailed"]);
    detailed.exited(0).out_has("landing-registered");
    world
        .run(&["monitor", "origin"])
        .exited(0)
        .out_lacks("landing-registered");
    assert_eq!(
        world.run_json("origin", "launch.json")["session"],
        json!(world.session)
    );
    assert_eq!(
        std::fs::read_to_string(world.run_file("origin", "result.json")).expect("result"),
        origin_result,
        "the settled run was revived"
    );

    // Nothing that would hide it is recorded, by either manager.
    let run_acks = world.run_file("origin", "acknowledgements");
    let before_run = snapshot(&run_acks);
    world
        .run(&[
            "unwatched",
            "--session",
            &world.session,
            "--acknowledge",
            "origin",
            "--reason",
            "done with it",
        ])
        .exited(REFUSED)
        .err_has(&format!("has a live landing, {id} of flow '{flow}'"));
    assert_eq!(snapshot(&run_acks), before_run);
    let before_branch = snapshot(&acknowledgements);
    acknowledge_branch(&world, Some("service"))
        .exited(REFUSED)
        .err_has(&format!("is being landed by {id}"));
    acknowledge_branch(&manager, Some("service"))
        .exited(REFUSED)
        .err_has(&format!("is being landed by {id}"));
    assert_eq!(snapshot(&acknowledgements), before_branch);
    manager
        .run(&[
            "unwatched",
            "--session",
            "manager-b",
            "--acknowledge-flow",
            &flow,
            "--reason",
            "r",
        ])
        .exited(REFUSED)
        .err_has("is live");
    // The landing's owner is held, the earlier acknowledgement notwithstanding.
    let held = manager.run(&["stop-guard", "--session", "manager-b"]);
    let held: Value = serde_json::from_str(held.stdout.trim()).expect("a verdict");
    assert_eq!(held["verdict"], json!("block"), "{held}");
    // The same branch name in the other identity is not this landing's.
    acknowledge_branch(&world, Some("other")).exited(EXIT_SUCCESS);

    // Released, the landing ends; the run and the branch close as they always did.
    std::fs::write(&go, "go").expect("released");
    let (_, _, said) = landing.ended();
    assert!(
        landing_dir(&world, &flow, &id)
            .join("ending.json")
            .is_file(),
        "{said}"
    );
    world
        .run(&[
            "unwatched",
            "--session",
            &world.session,
            "--acknowledge",
            "origin",
            "--reason",
            "landed by hand",
        ])
        .exited(EXIT_SUCCESS);
}

/// A `flow run` this journey started whose program starts a landing in the
/// background, recording its pid, and then waits on a gate — a longer flow a
/// landing is one step of.
struct Parent {
    child: Child,
    landing_pid: PathBuf,
    gate: PathBuf,
}

impl Parent {
    fn start(world: &World, name: &str, landing: &[&str]) -> Self {
        use std::os::unix::process::CommandExt;
        let landing_pid = world.root.join(format!("{name}.landing-pid"));
        let gate = world.root.join(format!("{name}.go"));
        let quoted: Vec<String> = landing
            .iter()
            .map(|word| format!("'{}'", word.replace('\'', r"'\''")))
            .collect();
        let script = world.root.join(format!("{name}.sh"));
        std::fs::write(
            &script,
            format!(
                "\"$ONEPIPELINE_FAKE_CLI_BIN\" {} >/dev/null 2>&1 &\necho $! > '{}'\nwhile [ ! -f '{}' ]; do sleep 0.05; done\n",
                quoted.join(" "),
                landing_pid.display(),
                gate.display()
            ),
        )
        .expect("the program is written");
        let mut command = world.cmd(&[
            "flow",
            "run",
            "--name",
            name,
            "--",
            "sh",
            script.to_str().expect("a path"),
        ]);
        command.process_group(0);
        let child = command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("flow run starts");
        Self {
            child,
            landing_pid,
            gate,
        }
    }

    fn landing_pid(&self, world: &World) -> i32 {
        world.until("the landing to start", |_| {
            std::fs::read_to_string(&self.landing_pid).is_ok_and(|pid| pid.ends_with('\n'))
        });
        std::fs::read_to_string(&self.landing_pid)
            .expect("a pid")
            .trim()
            .parse()
            .expect("a pid")
    }

    fn kill_holder(&mut self) {
        let pid = i32::try_from(self.child.id()).expect("a pid fits");
        // SAFETY: the flow's own holder, a child of this journey.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
        let _ = self.child.wait();
    }
}

/// `SIGKILL` a pid this journey learned from a process it started.
fn kill(pid: i32) {
    // SAFETY: `kill` touches no memory; the pid is a landing this journey's own
    // flow program started and recorded.
    assert_eq!(
        unsafe { libc::kill(pid, libc::SIGKILL) },
        0,
        "the landing could not be killed"
    );
}

/// A landing started inside a longer flow of the same session is that flow's —
/// no flow of its own is registered — and keeps its own liveness: killed, its
/// death is a `landing-progress` blocker on the flow's watch while the flow goes
/// on, never the flow failing; and with the flow's own holder killed, the flow
/// stays live while the landing does, and dies once the landing has ended.
#[test]
fn a_landing_in_a_longer_flow_keeps_its_own_liveness_beside_the_flows() {
    // The landing dies; the flow goes on.
    let (world, repository, held) = held_world("ml-nested-child", "change-open");
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let mut parent = Parent::start(
        &world,
        "plan",
        &["publish-branch", BRANCH, "--repo", "service", "--no-draft"],
    );
    let pid = parent.landing_pid(&world);
    world.until("the landing's push to be held", |_| held.holding());
    let (flow, id) = the_landing(&world);
    assert_eq!(flow, "plan", "the landing joined the longer flow");
    let request = sidecar(&world, &flow, &id, "request.json");
    assert_eq!(request["pid"], json!(pid));
    let armed = Armed::arm(&world, &flow, &["--timeout", "600"]);
    kill(pid);
    let died = armed.returned();
    died.progressed("landing-died");
    assert_eq!(died.record["significance"], json!("blocker"));
    assert_eq!(died.record["landing_id"], json!(id));
    // The flow itself is still live, and owed a watch rather than closure.
    watch(
        &world,
        &flow,
        &["--timeout", "0", "--cursor", &died.cursor()],
    )
    .ended_on(ELAPSED, "elapsed");
    let owed = world.run(&["unwatched", "--session", &world.session]);
    owed.exited(OWED);
    assert!(owed.stdout.contains("LIVE"), "{}", owed.stdout);
    std::fs::write(&parent.gate, "go").expect("the flow ends");
    let _ = parent.child.wait();
    watch(
        &world,
        &flow,
        &["--timeout", "0", "--cursor", &died.cursor()],
    )
    .ended_on(EXIT_SUCCESS, "ended");
    drop(held);

    // The flow's holder dies; the landing it started keeps the flow live.
    let (world, repository, held) = held_world("ml-nested-holder", "change-open");
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let mut parent = Parent::start(
        &world,
        "plan",
        &["publish-branch", BRANCH, "--repo", "service", "--no-draft"],
    );
    parent.landing_pid(&world);
    world.until("the landing's push to be held", |_| held.holding());
    let (flow, id) = the_landing(&world);
    parent.kill_holder();
    watch(&world, &flow, &["--timeout", "0"]).ended_on(ELAPSED, "elapsed");
    let owed = world.run(&["unwatched", "--session", &world.session]);
    owed.exited(OWED);
    assert!(
        owed.stdout.contains(&format!("flow {flow}")) && owed.stdout.contains("LIVE"),
        "a killed holder hid a live landing: {}",
        owed.stdout
    );
    held.release();
    world.until("the landing to end", |world| {
        landing_dir(world, &flow, &id).join("ending.json").is_file()
    });
    let ended = watch(&world, &flow, &["--timeout", "600", "--filter", "detailed"]);
    // Its notices come first, then the flow's own death.
    let mut cursor = ended.cursor();
    let mut seen = vec![ended.record["condition"]
        .as_str()
        .unwrap_or_default()
        .to_owned()];
    while seen.last().map(String::as_str) == Some("landing-progress") {
        let next = watch(
            &world,
            &flow,
            &[
                "--timeout",
                "600",
                "--filter",
                "detailed",
                "--cursor",
                &cursor,
            ],
        );
        cursor = next.cursor();
        seen.push(
            next.record["condition"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        );
        if next.record["condition"] == "flow-failed" {
            assert_eq!(next.record["died"], json!(true));
        }
    }
    assert_eq!(
        seen.last().map(String::as_str),
        Some("flow-failed"),
        "{seen:?}"
    );
}

/// A landing started in a live flow another session owns is refused before
/// anything is registered, drafted or forwarded.
#[test]
fn a_landing_in_another_sessions_flow_is_refused_and_does_no_work() {
    let (world, repository, held) = held_world("ml-foreign-flow", "change-open");
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let foreign = world.as_session("someone-else");
    let mut parent = Parent::start(&foreign, "theirs", &["--version"]);
    world.until("their flow to register", |world| {
        world.runs.join(".flows/theirs/flow.json").is_file()
    });
    let mut command = world.cmd(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"]);
    command.env(onepipeline::cli::FLOW_ENV, "theirs");
    let refused = world.run_on(command, "publish-branch in their flow");
    refused
        .exited(REFUSED)
        .err_has("could not be registered")
        .err_has("belongs to session");
    assert!(landings(&world).is_empty(), "{:?}", landings(&world));
    assert!(!held.holding(), "the refused landing pushed");
    assert!(world.changes_opened().is_empty());
    std::fs::write(&parent.gate, "go").expect("their flow ends");
    parent.kill_holder();

    // A flow it runs in that is there and cannot be read is refused too, never
    // replaced by a flow of the landing's own.
    // llmlint: ignore-block[tests_mirror_real_usage] the record is written by hand because no
    // verb writes a flow record this build cannot read; the refusal it stages is driven
    // through the compiled binary.
    let broken = world.runs.join(".flows").join("broken");
    std::fs::create_dir_all(&broken).expect("a flow directory");
    std::fs::write(broken.join("flow.json"), "{").expect("an unreadable record");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let mut command = world.cmd(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"]);
    command.env(onepipeline::cli::FLOW_ENV, "broken");
    world
        .run_on(command, "publish-branch in an unreadable flow")
        .exited(REFUSED)
        .err_has("could not be registered")
        .err_has("record cannot be read");
    assert!(landings(&world).is_empty(), "{:?}", landings(&world));
    assert!(!held.holding());
    assert!(world.changes_opened().is_empty());
}

/// With no manager session the verbs run unowned, as they always did, and say
/// so: nothing is registered and the landing still lands.
#[test]
fn with_no_manager_session_a_landing_runs_unowned_and_says_so() {
    let world = World::new("ml-unowned");
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let mut command = world.cmd(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"]);
    command.env("ONEPIPELINE_LAUNCHER_SESSION", " ");
    world
        .run_on(command, "publish-branch unowned")
        .exited(0)
        .err_has("publish-branch: no manager session owns this landing")
        .err_has("so it runs unowned");
    assert!(!world.runs.join(".flows").exists());
    assert_eq!(world.changes_opened().len(), 1);
}

/// Three managers each land at once, one landing per session, into one
/// repository: each landing has its own flow, its own operation id and stream,
/// and its own owner — a manager's `unwatched` names only its own, a death is
/// reported only to its owner, and the other two finish under theirs. Two of
/// them name the same branch of different identities, and the third an ordered
/// train, so isolation is by identity and operation, never by branch name.
#[test]
fn three_managers_landing_at_once_are_each_supervised_alone() {
    let (world, repository, held) = held_world("ml-three", "local-direct");
    for branch in TRAIN {
        branch_with_work(&world, &repository, branch, &format!("{branch}.txt"));
    }
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let other = world.extra_repository("other");
    git(&world, &other.checkout, &["checkout", "-b", BRANCH]);
    std::fs::write(other.checkout.join("other.txt"), "other\n").expect("written");
    git(&world, &other.checkout, &["add", "-A"]);
    git(
        &world,
        &other.checkout,
        &["commit", "-q", "-m", "feat: the other widget"],
    );
    git(&world, &other.checkout, &["checkout", "main"]);

    let sessions = ["manager-a", "manager-b", "manager-c"];
    let managers: Vec<World> = sessions.iter().map(|s| world.as_session(s)).collect();
    let mut started = vec![
        Started::spawn(
            &managers[0],
            None,
            &["publish-branch", BRANCH, "--repo", "service", "--no-draft"],
        ),
        Started::spawn(
            &managers[1],
            None,
            &["publish-branch", BRANCH, "--repo", "other", "--no-draft"],
        ),
    ];
    let mut train = vec!["integrate"];
    train.extend(TRAIN);
    train.push("--push");
    started.push(Started::spawn(
        &managers[2],
        Some(&repository.checkout),
        &train,
    ));
    world.until("all three landings to register", |world| {
        landings(world).len() == 3
    });
    world.until("the service landings to be held", |_| held.holding());
    let mut by_session = std::collections::BTreeMap::new();
    for (flow, id) in landings(&world) {
        let request = sidecar(&world, &flow, &id, "request.json");
        by_session.insert(
            request["session"].as_str().expect("a session").to_owned(),
            (flow, id, request),
        );
    }
    assert_eq!(by_session.len(), 3, "{by_session:?}");
    let identities: std::collections::BTreeSet<String> = by_session
        .values()
        .map(|(_, _, request)| request["subjects"][0]["identity"].to_string())
        .collect();
    assert_eq!(identities.len(), 2, "two identities share a branch name");
    let train_request = &by_session["manager-c"].2;
    assert_eq!(train_request["subjects"].as_array().map(Vec::len), Some(3));

    // Manager b's landing — into the other identity, which no hook holds — ends
    // on its own; a and c are each owed their own live flow and nobody else's.
    let mut a = started.remove(0);
    let b = started.remove(0);
    let c = started.remove(0);
    let (b_code, _, b_said) = b.ended();
    assert_eq!(b_code, 0, "{b_said}");
    world
        .run(&["unwatched", "--session", "manager-b"])
        .exited(EXIT_SUCCESS);
    for session in ["manager-a", "manager-c"] {
        let owed = world.run(&["unwatched", "--session", session]);
        owed.exited(OWED);
        for (other_session, (other_flow, _, _)) in &by_session {
            assert_eq!(
                owed.stdout.contains(&format!("flow {other_flow} ")),
                other_session == session,
                "{session}: {}",
                owed.stdout
            );
        }
    }
    // Every operation is its own: disjoint ids, and no record of one in another.
    let mut operations = std::collections::BTreeSet::new();
    world.until("each landing to prepare its operation", |world| {
        by_session.values().all(|(flow, id, _)| {
            landing_dir(world, flow, id)
                .join("operation.json")
                .is_file()
        })
    });
    for (flow, id, _) in by_session.values() {
        let operation = sidecar(&world, flow, id, "operation.json")["operation_id"].clone();
        assert!(operations.insert(operation.to_string()), "{operations:?}");
        for event in journal(&world, flow, id)
            .iter()
            .filter(|e| e["source"] == "vcs")
        {
            assert_eq!(event["labels"]["operation_id"], operation, "{event}");
        }
    }

    // Manager a's landing is killed while c's train is still held.
    a.kill();
    let (a_flow, _, _) = &by_session["manager-a"];
    let (c_flow, _, _) = &by_session["manager-c"];
    watch(&managers[0], a_flow, &["--timeout", "0"]).ended_on(EXIT_FLOW_FAILED, "flow-failed");
    let c_owed = world.run(&["unwatched", "--session", "manager-c"]);
    assert!(
        c_owed.stdout.contains("LIVE") && !c_owed.stdout.contains("DIED"),
        "{}",
        c_owed.stdout
    );
    held.release();
    let (c_code, c_out, c_said) = c.ended();
    assert_eq!(c_code, 0, "{c_out}\n{c_said}");
    let c_woken = rearmed_until_ended(
        &managers[2],
        c_flow,
        String::from("flow:2:") + c_flow + "::",
    );
    assert!(
        c_woken
            .iter()
            .all(|(_, significance)| significance != "progress"),
        "{c_woken:?}"
    );
}

/// An integration train records its own local progress — the lock, the gate's
/// run and the push — as detail, opens no change request and owes no hosted
/// check; a second train of the same repository waits on the first's lock in
/// its own operation, recorded as progress that wakes nobody; and a gate that
/// refuses the push is a blocker its owner is woken on.
#[test]
fn an_integration_train_records_local_progress_and_wakes_its_owner_on_the_gates_refusal() {
    let (world, repository, held) = held_world("ml-trains", "local-direct");
    for branch in TRAIN {
        branch_with_work(&world, &repository, branch, &format!("{branch}.txt"));
    }
    let second = world.as_session("manager-two");
    let mut first_args = vec!["integrate", TRAIN[0], TRAIN[1]];
    first_args.push("--push");
    let first = Started::spawn(&world, Some(&repository.checkout), &first_args);
    world.until("the first train's push to be held", |_| held.holding());
    let (first_flow, first_id) = the_landing(&world);
    let waiting = Started::spawn(
        &second,
        Some(&repository.checkout),
        &["integrate", TRAIN[2], "--push"],
    );
    world.until("the second train to register", |world| {
        landings(world).len() == 2
    });
    let (second_flow, second_id) = landings(&world)
        .into_iter()
        .find(|(flow, _)| *flow != first_flow)
        .expect("the second train");
    world.until("the second train to start its operation", |world| {
        kinds(world, &second_flow, &second_id)
            .iter()
            .any(|kind| kind == "landing-started")
    });
    // Waiting behind the first train wakes neither owner.
    watch(&second, &second_flow, &["--timeout", "0"]).ended_on(ELAPSED, "elapsed");
    watch(&world, &first_flow, &["--timeout", "0"]).ended_on(ELAPSED, "elapsed");
    let operation =
        |flow: &str, id: &str| sidecar(&world, flow, id, "operation.json")["operation_id"].clone();
    assert_ne!(
        operation(&first_flow, &first_id),
        operation(&second_flow, &second_id),
        "two trains of one repository share an operation"
    );
    held.release();
    let (code, out, said) = first.ended();
    assert_eq!(code, 0, "{out}\n{said}");
    let (code, out, said) = waiting.ended();
    assert_eq!(code, 0, "{out}\n{said}");
    for (flow, id) in [(&first_flow, &first_id), (&second_flow, &second_id)] {
        let all = kinds(&world, flow, id);
        for kind in ["lock-wait", "lock-acquired", "gate-run", "push"] {
            assert!(
                all.iter().any(|recorded| recorded == kind),
                "{kind} not in {all:?}"
            );
        }
        let own = sidecar(&world, flow, id, "operation.json")["operation_id"].clone();
        for event in journal(&world, flow, id)
            .iter()
            .filter(|e| e["source"] == "vcs")
        {
            assert_eq!(event["labels"]["operation_id"], own, "{event}");
            if ["lock-wait", "lock-acquired", "push"]
                .contains(&event["kind"].as_str().unwrap_or_default())
            {
                assert_eq!(
                    event["labels"]["significance"],
                    json!("progress"),
                    "{event}"
                );
            }
        }
        for hosted in ["change-opened", "change-check", "checks-settled"] {
            assert!(
                !all.iter().any(|recorded| recorded == hosted),
                "{hosted} in {all:?}"
            );
        }
    }
    assert!(world.changes_opened().is_empty());

    // A gate that refuses: the push is refused, and that refusal wakes the owner.
    let (world, repository) = {
        let world = World::new("ml-train-refused");
        let repository = world.repository("local-direct", &["false"]);
        (world, repository)
    };
    for branch in TRAIN {
        branch_with_work(&world, &repository, branch, &format!("{branch}.txt"));
    }
    let mut argv = vec!["integrate"];
    argv.extend(TRAIN);
    argv.push("--push");
    let refused = Started::spawn(&world, Some(&repository.checkout), &argv);
    let (code, _, said) = refused.ended();
    assert_ne!(code, 0, "{said}");
    let (flow, id) = the_landing(&world);
    let blocker = watch(
        &world,
        &flow,
        &[
            "--timeout",
            "0",
            "--cursor",
            &format!("flow:2:{flow}:{id}@0:"),
        ],
    );
    blocker.ended_on(EXIT_SUCCESS, "landing-progress");
    assert_eq!(
        blocker.record["significance"],
        json!("blocker"),
        "{}",
        blocker.record
    );
    assert!(
        kinds(&world, &flow, &id)
            .iter()
            .any(|kind| kind == "gate-run"),
        "{:?}",
        kinds(&world, &flow, &id)
    );
}

/// Landing state this build cannot read fails closed: the owner's stop guard is
/// held on it rather than warned, and every acknowledgement that could hide it —
/// the flow's, a run's, a branch's — is refused with nothing recorded. A flow
/// and a cursor from before landings read as they always did.
#[test]
fn unreadable_landing_state_fails_closed_and_records_from_before_landings_still_read() {
    let world = World::new("ml-malformed");
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    world
        .run(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"])
        .exited(0);
    let (flow, id) = the_landing(&world);
    // The flow ended cleanly, and is owed nothing.
    assert_eq!(verdict(&world), json!({"verdict": "none"}));
    // A cursor printed before landings resumes, replaying none of them — in a
    // watch and in a monitor alike.
    watch(
        &world,
        &flow,
        &["--timeout", "0", "--cursor", &format!("flow:1:{flow}:")],
    )
    .ended_on(EXIT_SUCCESS, "ended");
    world
        .run(&[
            "monitor",
            "--flow",
            &flow,
            "--filter",
            "detailed",
            "--cursor",
            &format!("flow:1:{flow}:"),
        ])
        .exited(0)
        .out_lacks("landing-registered")
        .out_lacks("landing-finished");
    // The detailed reader and `next` read the landing's whole journal; the
    // planner profile leaves its progress out.
    let detailed = world.run(&["next", "--flow", &flow, "--filter", "detailed"]);
    detailed.exited(0);
    let detailed: Value = serde_json::from_str(detailed.stdout.trim()).expect("a document");
    let every: Vec<&str> = detailed["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| event["kind"].as_str().unwrap_or_default())
        .collect();
    assert!(
        every.contains(&"landing-registered") && every.contains(&"push"),
        "{every:?}"
    );
    let planner = world.run(&["next", "--flow", &flow]);
    let planner: Value = serde_json::from_str(planner.stdout.trim()).expect("a document");
    for event in planner["events"].as_array().expect("events") {
        assert_ne!(
            event["labels"]["significance"],
            json!("progress"),
            "{event}"
        );
    }

    // An ending this build cannot read — or one naming another landing — says
    // neither that the landing ended nor that it did not: the flow is owed
    // supervision again, and no acknowledgement closes it.
    let ending = landing_dir(&world, &flow, &id).join("ending.json");
    let kept = std::fs::read_to_string(&ending).expect("the ending");
    // llmlint: ignore-block[tests_mirror_real_usage] the ending is written by hand because no
    // verb writes one this build cannot read or one naming another landing; the reads it
    // stages are the fail-closed ones each assertion holds, driven through the binary.
    for tampered in ["{".to_owned(), kept.replace(&id, "landing-someone-else")] {
        std::fs::write(&ending, &tampered).expect("tampered");
        assert_eq!(verdict(&world)["verdict"], json!("block"), "{tampered}");
        world
            .run(&[
                "unwatched",
                "--session",
                &world.session,
                "--acknowledge-flow",
                &flow,
                "--reason",
                "r",
            ])
            .exited(REFUSED)
            .err_has("is live");
    }
    std::fs::write(&ending, &kept).expect("restored");
    // llmlint: ignore-end[tests_mirror_real_usage]
    assert_eq!(verdict(&world), json!({"verdict": "none"}));

    // Now the request is unreadable.
    // llmlint: ignore-block[tests_mirror_real_usage] the sidecar is written to directly
    // because no verb leaves a landing whose request cannot be read: registration writes
    // it whole and renames it into place. What is arranged is what a truncated disk or a
    // hand edit leaves, which is the state the fail-closed reads exist for; everything
    // asserted about it is read back off the compiled binary's own verbs.
    let request = landing_dir(&world, &flow, &id).join("request.json");
    std::fs::write(&request, "{").expect("tampered");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let blocked = verdict(&world);
    assert_eq!(blocked["verdict"], json!("block"), "{blocked}");
    let owed = world.run(&["unwatched", "--session", &world.session]);
    owed.exited(OWED);
    assert!(owed.stdout.contains("UNKNOWN"), "{}", owed.stdout);
    let flow_acks = world
        .runs
        .join(".flows")
        .join(&flow)
        .join("acknowledgements");
    let before = snapshot(&flow_acks);
    world
        .run(&[
            "unwatched",
            "--session",
            &world.session,
            "--acknowledge-flow",
            &flow,
            "--reason",
            "r",
        ])
        .exited(REFUSED);
    assert_eq!(snapshot(&flow_acks), before);
    let acknowledgements = world.root.join("acks");
    world
        .run(&[
            "unpublished",
            "--acknowledge",
            BRANCH,
            "--reason",
            "kept",
            "--session",
            &world.session,
            "--acknowledgements",
            &acknowledgements.display().to_string(),
        ])
        .exited(REFUSED)
        .err_has("cannot be said");
    assert!(snapshot(&acknowledgements).is_empty());
}

/// A landing associated with a **live** run — one whose node opened a session on
/// the branch and is still working — is mirrored into that run's journal as it
/// happens, and the run's own observer, the real `monitor --filter detailed` it
/// reads with only its model turn doubled, sees every transition; the run's
/// planner view keeps the blocker and leaves the progress out.
#[test]
fn a_landing_from_a_live_run_reaches_that_runs_observer() {
    let world = World::new("ml-observed");
    let _repository = world.repository("local-direct", &[]);
    world.script("service.wait", "hold");
    world.script("observer.wait", "hold");
    world.script("observer.monitor", "1");
    let run = "observed";
    let path = world.plan(
        run,
        &crate::harness::plan_of(run, vec![crate::harness::lifecycle("service", &[])]),
    );
    world
        .run(&[
            "start",
            &path,
            "--detach",
            "--dag-graph",
            &world.shipped_dag_graph(),
        ])
        .exited(0);
    world.until("the node's session to open", |world| {
        world
            .journal(run)
            .iter()
            .any(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
    });
    world.until("the observer to start", |world| {
        !world.observer_saw().is_empty()
    });
    let branch = world
        .journal(run)
        .iter()
        .find(|event| event["kind"] == "session-opened")
        .and_then(|event| event["payload"]["branch"].as_str().map(str::to_owned))
        .expect("the session names its branch");

    // The manager lands the run's branch while the run is still working on it.
    let landed = world.run(&["publish-branch", &branch, "--repo", "service", "--no-draft"]);
    let (flow, id) = the_landing(&world);
    assert_eq!(
        sidecar(&world, &flow, &id, "request.json")["runs"],
        json!([run])
    );
    let terminal = if landed.code == 0 {
        "landing-finished"
    } else {
        "landing-refused"
    };
    let mirrored: Vec<String> = world
        .journal(run)
        .iter()
        .filter(|event| event["labels"]["landing_id"] == json!(id))
        .map(|event| event["kind"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        mirrored.first().map(String::as_str) == Some("landing-registered")
            && mirrored.last().map(String::as_str) == Some(terminal),
        "{mirrored:?}"
    );

    // The observer, let go, reads every one of them through the real reader.
    world.release("observer.go");
    let read = world.fakes.join("observer-monitor.txt");
    world.until("the observer to read the run", |_| read.is_file());
    let seen = std::fs::read_to_string(&read).expect("what the observer read");
    for kind in ["landing-registered", "landing-started", terminal] {
        assert!(
            seen.contains(kind),
            "the observer did not see {kind}:\n{seen}"
        );
    }
    let planner = world.run(&["monitor", run]);
    planner.exited(0).out_lacks("landing-registered");
    if terminal == "landing-refused" {
        planner.out_has("landing-refused");
    }
    world.release("service.go");
}

/// Under a wake budget the landing's flow is owed a watch that wakes the session
/// within it: one armed with the budget as its timeout does not qualify, one
/// armed with the budget less the reserve does — the rule every flow already
/// answers to.
#[test]
fn a_landing_is_owed_a_watch_that_wakes_within_the_wake_budget() {
    let verb = Verb::publish("ml-budget");
    let world = &verb.world;
    let mut landing = verb.start();
    landing.held_at_push(&verb);
    let (flow, _) = the_landing(world);
    let budget = || {
        let asked = world.run(&[
            "stop-guard",
            "--session",
            world.session.as_str(),
            "--wake-budget",
            "600",
        ]);
        serde_json::from_str::<Value>(asked.stdout.trim()).expect("a verdict")
    };
    let late = Armed::arm(world, &flow, &["--timeout", "600"]);
    // Until its terms are recorded the guard warns that it cannot judge the
    // watch; once it can, it refuses it.
    world.until("the late watch's terms to be judged", |_| {
        budget()["verdict"] == json!("block")
    });
    let refused = budget();
    assert!(
        refused["reason"]
            .as_str()
            .is_some_and(|why| why.contains("--timeout 598")),
        "{refused}"
    );
    let in_time = Armed::arm(world, &flow, &["--timeout", "598"]);
    // A watch records its lease and then its terms; the guard is asked until it
    // has read both, which is the moment that watch qualifies.
    world.until("the in-time watch to qualify", |_| {
        budget() == json!({"verdict": "none"})
    });
    verb.held.release();
    let (code, _, said) = landing.ended();
    assert_eq!(code, 0, "{said}");
    for armed in [late, in_time] {
        let returned = armed.returned();
        assert_eq!(returned.code, EXIT_SUCCESS, "{}", returned.record);
    }
}

/// Whether a process is waiting on a file lock, as Linux reports its wait.
fn waiting_on_a_lock(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/wchan"))
        .is_ok_and(|channel| channel.contains("lock"))
}

/// A branch acknowledgement and a landing's registration are serialized: with
/// the registration lock held, both wait on it — the landing has written no
/// ownership and the acknowledgement no record — and once it is let go, the
/// two never interleave: an acknowledgement recorded before the landing's
/// registration is one the live landing still owes supervision through, and one
/// made after it is refused.
#[test]
fn a_branch_acknowledgement_and_a_landing_registration_never_interleave() {
    let world = World::new("ml-serialized");
    let _repository = world.repository("local-direct", &["true"]);
    preserved_by_session(&world, "service", BRANCH, &world.session);
    let go = world.root.join("push.go");
    hold_pushes(&world, &go);
    // llmlint: ignore-block[tests_mirror_real_usage] the registration lock is taken by
    // this journey because no command holds it open past its own check and write — which
    // is the whole of what it promises — so there is no interface to hold the window open
    // through. Both contenders are the compiled binary, driven as a manager drives it.
    let flows = world.runs.join(".flows");
    std::fs::create_dir_all(&flows).expect("the flows directory");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(flows.join("landings.lock"))
        .expect("the registration lock");
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: a descriptor this journey just opened and still owns.
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    }
    // llmlint: ignore-end[tests_mirror_real_usage]
    let landing = Started::spawn(
        &world,
        None,
        &[
            "repo-recover",
            BRANCH,
            "--repo",
            "service",
            "--title",
            "feat: land the kept work",
            "--no-draft",
        ],
    );
    let acknowledgements = world.root.join("acks");
    let directory = acknowledgements.display().to_string();
    let acknowledging = Started::spawn(
        &world,
        None,
        &[
            "unpublished",
            "--acknowledge",
            BRANCH,
            "--reason",
            "kept deliberately",
            "--session",
            &world.session,
            "--acknowledgements",
            &directory,
        ],
    );
    world.until("both to wait on the registration lock", |_| {
        waiting_on_a_lock(landing.child.id()) && waiting_on_a_lock(acknowledging.child.id())
    });
    assert!(
        landings(&world).is_empty(),
        "a landing registered past the lock"
    );
    assert!(
        snapshot(&acknowledgements).is_empty(),
        "an acknowledgement was written past the lock"
    );
    drop(lock);

    let (code, out, said) = acknowledging.ended();
    let (flow, id) = the_landing(&world);
    let request = sidecar(&world, &flow, &id, "request.json");
    if code == EXIT_SUCCESS {
        // Recorded first: the landing registered after it, and is owed a watch
        // regardless.
        let file = snapshot(&acknowledgements);
        let recorded: Value = serde_json::from_str(&file[0].1).expect("the acknowledgement file");
        let at = recorded["acknowledged"][0]["at"]
            .as_str()
            .expect("an instant")
            .to_owned();
        // Compared to the second, the precision the acknowledgement keeps.
        let began = request["began_at"].as_str().expect("an instant");
        assert!(at[..19] <= began[..19], "{recorded} {request}");
        assert_eq!(verdict(&world)["verdict"], json!("block"));
    } else {
        assert_eq!(code, REFUSED, "{out}\n{said}");
        assert!(said.contains(&format!("is being landed by {id}")), "{said}");
        assert!(snapshot(&acknowledgements).is_empty());
    }
    std::fs::write(&go, "go").expect("released");
    let (code, _, said) = landing.ended();
    assert_eq!(code, 0, "{said}");
}

/// `repo-recover` where it drafts: held at its drafting turn, the landing is
/// already owned — its request names the preserved branch, and its journal holds
/// `landing-registered` then `landing-drafting` — before the turn is paid for,
/// and nothing is forwarded until the body is drafted; released, it lands with
/// the drafted body and its flow ends `0`.
#[test]
fn repo_recover_is_owned_before_it_drafts() {
    let world = World::new("ml-recover-drafts");
    let repository = world.repository("change-open", &[]);
    branch_under(
        &world,
        &repository,
        BRANCH,
        "widget.txt",
        "feat: add the widget (incomplete step)",
    );
    onevcs(&world, &["preserve", BRANCH, "--repo", "service"]);
    world.script("pr-author.body", "## What\nA recovered widget.\n");
    world.script("pr-author.records-tree", "1");
    world.script("pr-author.holds", "1");
    let graph = world.pr_author_graph();
    let landing = Started::spawn(
        &world,
        None,
        &[
            "repo-recover",
            BRANCH,
            "--repo",
            "service",
            "--title",
            "feat: add the widget",
            "--pr-author-graph",
            &graph,
        ],
    );
    let tree = world.fakes.join("pr-author-tree.jsonl");
    world.until("the drafting turn to be in flight", |_| {
        std::fs::read_to_string(&tree).is_ok_and(|said| said.ends_with('\n'))
    });
    let (flow, id) = the_landing(&world);
    assert_eq!(flow, "repo-recover");
    assert_eq!(
        kinds(&world, &flow, &id),
        ["landing-registered", "landing-drafting"]
    );
    assert!(!landing_dir(&world, &flow, &id)
        .join("operation.json")
        .exists());
    watch(&world, &flow, &["--timeout", "0"]).ended_on(ELAPSED, "elapsed");
    let registered = watch(&world, &flow, &["--timeout", "0", "--filter", "detailed"]);
    registered.progressed("landing-registered");
    let drafting = watch(
        &world,
        &flow,
        &[
            "--timeout",
            "0",
            "--filter",
            "detailed",
            "--cursor",
            &registered.cursor(),
        ],
    );
    drafting.progressed("landing-drafting");
    std::fs::write(world.fakes.join("pr-author.go"), "go").expect("the draft is released");
    let (code, _, said) = landing.ended();
    assert_eq!(code, 0, "{said}");
    assert_eq!(
        world
            .changes_opened()
            .iter()
            .map(|change| change["body"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>(),
        ["## What\nA recovered widget."]
    );
    let woken = rearmed_until_ended(&world, &flow, drafting.cursor());
    assert!(
        woken
            .iter()
            .all(|(_, significance)| significance != "progress"),
        "{woken:?}"
    );
}

/// A world whose single repository's own `pre-push` hook runs `script` with
/// `sh -c` at each publishing push, and then lets it through.
fn hooked_world(name: &str, publication: &str, script: &str) -> (World, Repository) {
    let world = World::new(name);
    let repository = world.repository(publication, &["sh", "-c", script]);
    (world, repository)
}

/// Make `path` read-only, or writable again.
///
/// The failure journeys below stage a host that refuses a write — the one
/// condition no verb leaves behind — by taking write permission away from the
/// directory or file a landing writes, and giving it back before reading.
// llmlint: ignore-block[tests_mirror_real_usage] what is changed is the host's permission on
// a path, never the content a verb wrote: a disk or a directory that refuses writes is the
// condition each of these journeys is about, and no command of this crate or its siblings
// produces one. Everything asserted is read back off the compiled binary.
fn writable(path: &Path, yes: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = match (path.is_dir(), yes) {
        (true, true) => 0o755,
        (true, false) => 0o555,
        (false, true) => 0o644,
        (false, false) => 0o444,
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .unwrap_or_else(|e| panic!("{} could not be re-permitted: {e}", path.display()));
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A registration that cannot be kept starts no work at all — no flow where
/// none could be written, no landing where its request could not be placed
/// inside a longer flow — and the manager is told it was refused.
#[test]
fn a_registration_that_cannot_be_kept_starts_no_work() {
    // No flow can be registered under a runs root whose flows cannot be written.
    let world = World::new("ml-unregistered");
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let flows = world.runs.join(".flows");
    std::fs::create_dir_all(&flows).expect("the flows directory");
    std::fs::write(flows.join("landings.lock"), "").expect("the lock exists");
    writable(&flows, false);
    let refused = world.run(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"]);
    writable(&flows, true);
    refused.exited(REFUSED).err_has(
        "could not be registered with its manager session, so nothing was drafted or forwarded",
    );
    assert!(world.changes_opened().is_empty());
    assert!(landings(&world).is_empty());

    // Inside a longer flow, a landing whose request cannot be placed is refused,
    // and the flow it would have joined goes on.
    let world = World::new("ml-unplaced");
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let gate = world.root.join("start.go");
    let said = world.root.join("landing.said");
    let script = world.root.join("plan.sh");
    std::fs::write(
        &script,
        format!(
            "while [ ! -f '{gate}' ]; do sleep 0.05; done\n\"$ONEPIPELINE_FAKE_CLI_BIN\" publish-branch {BRANCH} --repo service --no-draft 2> '{said}'\necho $? > '{said}.status'\nwhile [ ! -f '{gate}.end' ]; do sleep 0.05; done\n",
            gate = gate.display(),
            said = said.display()
        ),
    )
    .expect("the program is written");
    let mut parent = world
        .cmd(&[
            "flow",
            "run",
            "--name",
            "plan",
            "--",
            "sh",
            script.to_str().expect("a path"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("flow run starts");
    world.until("the flow to register", |world| {
        world.runs.join(".flows/plan/flow.json").is_file()
    });
    let placed = world.runs.join(".flows/plan").join("landings");
    std::fs::create_dir_all(&placed).expect("the landings directory");
    writable(&placed, false);
    std::fs::write(&gate, "go").expect("the landing starts");
    let status = PathBuf::from(format!("{}.status", said.display()));
    world.until("the landing to end", |_| status.is_file());
    writable(&placed, true);
    assert_ne!(
        std::fs::read_to_string(&status).expect("a status").trim(),
        "0"
    );
    assert!(std::fs::read_to_string(&said)
        .expect("what the landing said")
        .contains("could not be registered"),);
    assert!(landings(&world).is_empty());
    assert!(
        std::fs::read_dir(&placed)
            .expect("the landings directory")
            .next()
            .is_none(),
        "a staged registration was left behind"
    );
    assert!(world.changes_opened().is_empty());
    watch(&world, "plan", &["--timeout", "0"]).ended_on(ELAPSED, "elapsed");
    std::fs::write(PathBuf::from(format!("{}.end", gate.display())), "go").expect("the flow ends");
    let _ = parent.wait();
}

/// An operation `onevcs` will not prepare forwards nothing: the landing is
/// refused before its first push, and its flow is owed closure.
#[test]
fn an_operation_that_cannot_be_prepared_forwards_nothing() {
    let verb = Verb::publish("ml-unprepared");
    let world = &verb.world;
    let operations = world.onevcs_home().join("operations");
    std::fs::create_dir_all(&operations).expect("the operations directory");
    writable(&operations, false);
    let started = verb.start();
    let (code, _, said) = started.ended();
    writable(&operations, true);
    assert_eq!(code, REFUSED, "{said}");
    assert!(
        said.contains("would not prepare the landing's operation"),
        "{said}"
    );
    assert!(!verb.held.holding(), "an unprepared landing pushed");
    assert!(world.changes_opened().is_empty());
    let (flow, id) = the_landing(world);
    assert!(!landing_dir(world, &flow, &id)
        .join("operation.json")
        .exists());
    assert_eq!(
        kinds(world, &flow, &id),
        ["landing-registered", "landing-refused"]
    );
    watch(world, &flow, &["--timeout", "0"]).ended_on(EXIT_FLOW_FAILED, "flow-failed");
}

/// A landing whose operation's progress cannot be relayed — the stream holds a
/// record no reader can parse — still answers the forwarded verb's status, but
/// is settled refused, a blocker, and its own flow fails closed.
// llmlint: ignore-block[tests_mirror_real_usage] the repository's own `pre-push` hook appends
// a record no reader can parse to the operation's stream, because no command of `onevcs`
// writes one: every record it appends is a whole envelope. What is arranged is what a
// writer killed mid-append or a disk error leaves, which is the condition the relay's
// failure exists for; everything asserted is read back off the compiled binary.
#[test]
fn a_relay_that_fails_keeps_the_verbs_status_and_fails_the_flow_closed() {
    let (world, repository) = hooked_world(
        "ml-unrelayed",
        "change-open",
        "for stream in \"$ONEVCS_HOME\"/operations/o-*/events.ndjson; do printf 'not a record\\n' >> \"$stream\"; done; exit 0",
    );
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let landed = world.run(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"]);
    landed.exited(0).err_has("could not be relayed");
    assert_eq!(world.changes_opened().len(), 1, "the verb itself landed");
    let (flow, id) = the_landing(&world);
    let every = journal(&world, &flow, &id);
    let last = every.last().expect("a journal");
    assert_eq!(last["kind"], json!("landing-refused"), "{every:?}");
    assert_eq!(last["payload"]["status"], json!(0));
    assert_eq!(last["labels"]["significance"], json!("blocker"));
    let failed = watch(&world, &flow, &["--timeout", "0"]);
    failed.ended_on(EXIT_FLOW_FAILED, "flow-failed");
    assert_eq!(failed.record["status"], json!(1));
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A landing whose journal, or whose ending, cannot be written once it is
/// under way fails its own flow closed rather than reading as a clean finish:
/// an unwritable journal leaves the flow ended non-zero, and an ending that
/// could not be recorded reads, once the process is gone, as a death — never
/// as the success the verb reported.
#[test]
fn a_landing_whose_journal_or_ending_cannot_be_written_fails_closed() {
    let (world, repository) = hooked_world(
        "ml-unjournalled",
        "change-open",
        "chmod a-w \"$ONEPIPELINE_RUNS_DIR\"/.flows/*/landings/*/events.jsonl; exit 0",
    );
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let landed = world.run(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"]);
    landed.exited(0);
    let (flow, id) = the_landing(&world);
    let journal_path = landing_dir(&world, &flow, &id).join("events.jsonl");
    writable(&journal_path, true);
    assert_eq!(
        sidecar(&world, &flow, &id, "ending.json")["status"],
        json!(0)
    );
    assert!(!kinds(&world, &flow, &id)
        .iter()
        .any(|kind| kind == "landing-finished"));
    let failed = watch(&world, &flow, &["--timeout", "0"]);
    failed.ended_on(EXIT_FLOW_FAILED, "flow-failed");
    assert_eq!(failed.record["status"], json!(1));

    let (world, repository) = hooked_world(
        "ml-unended",
        "change-open",
        "chmod a-w \"$ONEPIPELINE_RUNS_DIR\"/.flows/*/landings/landing-*; exit 0",
    );
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let landed = world.run(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"]);
    landed
        .exited(0)
        .err_has("could not be recorded, so it will read as died");
    let (flow, id) = the_landing(&world);
    let dir = landing_dir(&world, &flow, &id);
    writable(&dir, true);
    assert!(!dir.join("ending.json").exists());
    let failed = watch(&world, &flow, &["--timeout", "0"]);
    failed.ended_on(EXIT_FLOW_FAILED, "flow-failed");
    assert_eq!(failed.record["status"], json!(1));
    // Settled as what it is — a blocker — and never read as finished.
    let every = journal(&world, &flow, &id);
    assert!(!every
        .iter()
        .any(|event| event["kind"] == "landing-finished"));
    let last = every.last().expect("a journal");
    assert_eq!(last["kind"], json!("landing-refused"), "{every:?}");
    assert!(
        last["payload"]["message"]
            .as_str()
            .is_some_and(|said| said.contains("could not be recorded")),
        "{last}"
    );
}

/// A flow's landing events read through a filter spec — inline or a file — as a
/// run's are; a spec naming a field the grammar does not have is refused before
/// anything waits; and a cursor that cannot be placed against a landing's
/// journal — past its end, inside a record, or naming one landing twice — is
/// refused naming why.
#[test]
fn a_flow_is_read_through_a_filter_spec_and_refuses_a_cursor_it_cannot_place() {
    let world = World::new("ml-specs");
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    world
        .run(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"])
        .exited(0);
    let (flow, id) = the_landing(&world);
    let from_start = format!("flow:2:{flow}:{id}@0:");
    let inline = r#"{"include": [{"kind": "push"}]}"#;
    watch(
        &world,
        &flow,
        &[
            "--timeout",
            "0",
            "--filter",
            inline,
            "--cursor",
            &from_start,
        ],
    )
    .progressed("push");
    let file = world.root.join("pushes.json");
    std::fs::write(&file, inline).expect("the spec is written");
    watch(
        &world,
        &flow,
        &[
            "--timeout",
            "0",
            "--filter",
            &file.display().to_string(),
            "--cursor",
            &from_start,
        ],
    )
    .progressed("push");
    let shown = world.run(&["monitor", "--flow", &flow, "--filter", inline]);
    shown
        .exited(0)
        .out_has("push")
        .out_lacks("landing-registered");
    world
        .run(&[
            "watch",
            "--flow",
            &flow,
            "--timeout",
            "0",
            "--filter",
            r#"{"include": [{"role": "x"}]}"#,
        ])
        .exited(REFUSED)
        .err_has("role");

    let journal_path = landing_dir(&world, &flow, &id).join("events.jsonl");
    let length = std::fs::metadata(&journal_path).expect("the journal").len();
    for (cursor, said) in [
        (
            format!("flow:2:{flow}:{id}@{}:", length + 10),
            "not the end of a record",
        ),
        (format!("flow:2:{flow}:{id}@3:"), "not the end of a record"),
        (format!("flow:2:{flow}:{id}@0/{id}@0:"), "names landing"),
    ] {
        world
            .run(&[
                "watch",
                "--flow",
                &flow,
                "--timeout",
                "0",
                "--cursor",
                &cursor,
            ])
            .exited(REFUSED)
            .err_has(said);
    }
}

/// `onepipeline integrate` answers exactly what `onevcs integrate` answers over
/// the same train — the same lines, the same status — whether the train lands
/// or is refused, beside the registration it adds on standard error.
#[test]
fn integrate_answers_in_onevcs_integrates_own_words_and_status() {
    for publication in ["local-direct", "change-open"] {
        let train = |name: &str| {
            let world = World::new(name);
            let repository = world.repository(publication, &[]);
            for branch in TRAIN {
                branch_with_work(&world, &repository, branch, &format!("{branch}.txt"));
            }
            (world, repository)
        };
        let (theirs, their_repository) = train(&format!("ml-parity-onevcs-{publication}"));
        let mut argv = vec!["integrate"];
        argv.extend(TRAIN);
        argv.push("--push");
        let sibling = theirs
            .cmd_on(&onevcs_binary(), &argv)
            .current_dir(&their_repository.checkout)
            .output()
            .expect("onevcs runs");
        let (ours, our_repository) = train(&format!("ml-parity-onepipeline-{publication}"));
        let started = Started::spawn(&ours, Some(&our_repository.checkout), &argv);
        let (code, out, said) = started.ended();
        assert_eq!(Some(code), sibling.status.code(), "{publication}: {said}");
        assert_eq!(
            out,
            String::from_utf8_lossy(&sibling.stdout),
            "{publication}"
        );
        // Each world's own paths stand for one another; everything else is the
        // sibling's line, byte for byte.
        let theirs_said = String::from_utf8_lossy(&sibling.stderr)
            .replace(&theirs.root.display().to_string(), "<root>");
        let said = said.replace(&ours.root.display().to_string(), "<root>");
        for line in theirs_said
            .lines()
            .filter(|line| line.starts_with("onevcs: "))
        {
            assert!(said.contains(line), "{publication}: {line} not in {said}");
        }
    }
}

/// A landing killed mid-operation whose operation record can no longer be read
/// is not read as legacy coarse-only work: the reader refuses to write its death
/// without the evidence its operation recorded, naming the record, and writes it
/// once the record reads again.
#[test]
fn a_dead_landings_unreadable_operation_record_refuses_rather_than_dropping_its_evidence() {
    let verb = Verb::publish("ml-dead-unread");
    let world = &verb.world;
    let mut landing = verb.start();
    landing.held_at_push(&verb);
    let (flow, id) = the_landing(world);
    landing.kill();
    let record = landing_dir(world, &flow, &id).join("operation.json");
    let kept = std::fs::read_to_string(&record).expect("the operation record");
    // llmlint: ignore-block[tests_mirror_real_usage] the operation record is written by hand
    // because no verb writes one that cannot be read; what it stages is the refusal the
    // death reader owes such a record, read back off the binary's own watch.
    std::fs::write(&record, "{").expect("tampered");
    world
        .run(&["watch", "--flow", &flow, "--timeout", "0"])
        .exited(REFUSED)
        .err_has("operation record cannot be read");
    assert!(!kinds(world, &flow, &id)
        .iter()
        .any(|kind| kind == "landing-died"));
    std::fs::write(&record, &kept).expect("restored");
    // llmlint: ignore-end[tests_mirror_real_usage]
    watch(world, &flow, &["--timeout", "0"]).ended_on(EXIT_FLOW_FAILED, "flow-failed");
    let every = journal(world, &flow, &id);
    let died = every
        .iter()
        .find(|event| event["kind"] == "landing-died")
        .expect("the death is written once the record reads");
    assert!(
        died["payload"]["message"]
            .as_str()
            .is_some_and(|said| said.contains("recorded events were kept")),
        "{died}"
    );
    verb.held.release();
}

/// A landing whose prepared operation's id cannot be kept forwards nothing: no
/// reader could reopen that invocation, so the landing is refused before the
/// linked call and its flow fails closed.
#[test]
fn an_operation_whose_id_cannot_be_kept_forwards_nothing() {
    let world = World::new("ml-unkept-operation");
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    world.script("pr-author.body", "## What\nA widget.\n");
    world.script("pr-author.records-tree", "1");
    world.script("pr-author.holds", "1");
    let graph = world.pr_author_graph();
    let landing = Started::spawn(
        &world,
        None,
        &[
            "publish-branch",
            BRANCH,
            "--repo",
            "service",
            "--title",
            "feat: add the widget",
            "--pr-author-graph",
            &graph,
        ],
    );
    let tree = world.fakes.join("pr-author-tree.jsonl");
    world.until("the drafting turn to be in flight", |_| {
        std::fs::read_to_string(&tree).is_ok_and(|said| said.ends_with('\n'))
    });
    let (flow, id) = the_landing(&world);
    let dir = landing_dir(&world, &flow, &id);
    writable(&dir, false);
    std::fs::write(world.fakes.join("pr-author.go"), "go").expect("the draft is released");
    let (code, _, said) = landing.ended();
    writable(&dir, true);
    assert_eq!(code, REFUSED, "{said}");
    assert!(!dir.join("operation.json").exists());
    assert!(
        world.changes_opened().is_empty(),
        "an unkept operation forwarded"
    );
    watch(&world, &flow, &["--timeout", "0"]).ended_on(EXIT_FLOW_FAILED, "flow-failed");
}

/// A second repository of `world`, seeded and pushed to a bare origin on disk,
/// registered under `origin` — the spelling a host gave its remote.
fn spelled_repository(world: &World, alias: &str, origin: &str) -> PathBuf {
    let bare = world.root.join(format!("{alias}.git"));
    std::fs::create_dir_all(&bare).expect("a scratch directory");
    git(world, &bare, &["init", "--bare", "--initial-branch=main"]);
    git(
        world,
        &world.root,
        &["clone", &bare.to_string_lossy(), alias],
    );
    let checkout = world.root.join(alias);
    std::fs::write(checkout.join("README.md"), format!("{alias}\n")).expect("seeded");
    git(world, &checkout, &["add", "-A"]);
    git(world, &checkout, &["commit", "-q", "-m", "chore: seed"]);
    git(world, &checkout, &["push", "-q", "-u", "origin", "main"]);
    world.register(&checkout, Some(origin));
    checkout
}

/// The identity `onevcs` keys a registered origin by is one a landing's request
/// is held to, whichever spelling `onevcs register` took — ssh and http(s),
/// with and without `.git`, and a host whose name is one label — so each
/// registers, and reads back.
#[test]
fn each_spelling_onevcs_registers_an_origin_by_registers_a_landing() {
    let world = World::new("ml-spellings");
    let _repository = world.repository("local-direct", &[]);
    for (alias, origin) in [
        ("sshrepo", "ssh://git@github.com/owner/sshrepo.git"),
        ("httpsrepo", "https://github.com/owner/httpsrepo"),
        ("dotgit", "https://github.com/owner/dotgit.git"),
        ("plain", "http://git.example/owner/plain"),
        ("single", "ssh://git@git/owner/single.git"),
    ] {
        let checkout = spelled_repository(&world, alias, origin);
        git(&world, &checkout, &["checkout", "-q", "-b", BRANCH]);
        std::fs::write(checkout.join("w.txt"), "w\n").expect("work");
        git(&world, &checkout, &["add", "-A"]);
        git(&world, &checkout, &["commit", "-q", "-m", "feat: w"]);
        git(&world, &checkout, &["checkout", "-q", "main"]);
        world
            .run(&["publish-branch", BRANCH, "--repo", alias, "--no-draft"])
            .exited(0);
    }
    let identities: Vec<Value> = landings(&world)
        .iter()
        .map(|(flow, id)| {
            sidecar(&world, flow, id, "request.json")["subjects"][0]["identity"].clone()
        })
        .collect();
    for expected in [
        "github.com/owner/sshrepo",
        "github.com/owner/httpsrepo",
        "github.com/owner/dotgit",
        "git.example/owner/plain",
        "git/owner/single",
    ] {
        assert!(identities.contains(&json!(expected)), "{identities:?}");
    }
    // Every request read back: nothing is owed, and nothing was unreadable.
    world
        .run(&["unwatched", "--session", &world.session])
        .exited(EXIT_SUCCESS);
}

/// `integrate` with no manager session runs unowned, as the other two do, and
/// says so: nothing is registered and the train still lands.
#[test]
fn integrate_with_no_manager_session_runs_unowned_and_says_so() {
    let world = World::new("ml-unowned-train");
    let repository = world.repository("local-direct", &[]);
    for branch in TRAIN {
        branch_with_work(&world, &repository, branch, &format!("{branch}.txt"));
    }
    let mut argv = vec!["integrate"];
    argv.extend(TRAIN);
    let mut command = world.cmd(&argv);
    command
        .env("ONEPIPELINE_LAUNCHER_SESSION", "")
        .current_dir(&repository.checkout);
    world
        .run_on(command, "integrate unowned")
        .exited(0)
        .err_has("integrate: no manager session owns this landing")
        .out_has("Integration train for main:");
    assert!(!world.runs.join(".flows").exists());
}

/// A run whose journal cannot be read is said on standard error and left
/// unassociated, and the landing — still its manager's supervised work — goes
/// on and ends as it would have.
#[test]
fn an_unreadable_run_is_named_and_left_unassociated_while_the_landing_goes_on() {
    let world = World::new("ml-unread-run");
    let _repository = world.repository("local-direct", &["false"]);
    let branch = originating_run(&world, "origin");
    onepipeline_testfakes::executable(
        &world.root.join("hooks").join("pre-push"),
        "#!/bin/sh\nexit 0\n",
    );
    // llmlint: ignore-block[tests_mirror_real_usage] a run's journal is made unreadable by
    // hand because no verb leaves one so; what it stages is the diagnostic and the omitted
    // association, read back off the compiled binary.
    let journal = world.run_file("origin", "events.jsonl");
    std::fs::rename(&journal, world.root.join("aside.jsonl")).expect("set aside");
    std::fs::create_dir_all(&journal).expect("a directory where the journal was");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let landed = world.run(&["publish-branch", &branch, "--repo", "service", "--no-draft"]);
    landed
        .exited(0)
        .err_has("run 'origin' is not associated with this landing");
    let (flow, id) = the_landing(&world);
    assert_eq!(
        sidecar(&world, &flow, &id, "request.json")["runs"],
        json!([])
    );
    watch(
        &world,
        &flow,
        &[
            "--timeout",
            "0",
            "--cursor",
            &format!("flow:2:{flow}:{id}@0:"),
        ],
    )
    .ended_on(EXIT_SUCCESS, "landing-progress");
    assert_eq!(
        kinds(&world, &flow, &id).last().map(String::as_str),
        Some("landing-finished")
    );
}

/// A landing killed before it prepared any operation — mid-draft — is written
/// dead with no operation to keep evidence of, and its flow reports the death.
#[test]
fn a_landing_killed_before_its_operation_is_written_dead_as_coarse_work() {
    let world = World::new("ml-killed-drafting");
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    world.script("pr-author.body", "## What\nA widget.\n");
    world.script("pr-author.records-tree", "1");
    world.script("pr-author.holds", "1");
    let graph = world.pr_author_graph();
    let mut landing = Started::spawn(
        &world,
        None,
        &[
            "publish-branch",
            BRANCH,
            "--repo",
            "service",
            "--pr-author-graph",
            &graph,
        ],
    );
    let tree = world.fakes.join("pr-author-tree.jsonl");
    world.until("the drafting turn to be in flight", |_| {
        std::fs::read_to_string(&tree).is_ok_and(|said| said.ends_with('\n'))
    });
    let (flow, id) = the_landing(&world);
    landing.kill();
    let died = watch(&world, &flow, &["--timeout", "0"]);
    died.ended_on(EXIT_FLOW_FAILED, "flow-failed");
    assert_eq!(died.record["died"], json!(true));
    let every = journal(&world, &flow, &id);
    let death = every
        .iter()
        .find(|event| event["kind"] == "landing-died")
        .expect("the death is written");
    assert!(
        death["payload"]["message"]
            .as_str()
            .is_some_and(|said| said.contains("no operation was prepared")),
        "{death}"
    );
    std::fs::write(world.fakes.join("pr-author.go"), "go").expect("the drafter is let go");
}

/// A landing killed mid-operation whose operation stream can no longer be read
/// is still written dead — the death is what is known — naming that its
/// operation's events could not be read, rather than claiming they were kept.
#[test]
fn a_dead_landings_unreadable_operation_stream_is_named_in_its_death() {
    let verb = Verb::publish("ml-dead-stream");
    let world = &verb.world;
    let mut landing = verb.start();
    landing.held_at_push(&verb);
    let (flow, id) = the_landing(world);
    landing.kill();
    let operation = sidecar(world, &flow, &id, "operation.json")["operation_id"]
        .as_str()
        .expect("an operation id")
        .to_owned();
    // llmlint: ignore-block[tests_mirror_real_usage] the operation's stream is given a record
    // no reader can parse by hand, because `onevcs` only ever appends whole envelopes; what is
    // staged is what a writer killed mid-append leaves, read back off the binary's watch.
    let stream = world
        .onevcs_home()
        .join("operations")
        .join(&operation)
        .join("events.ndjson");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&stream)
        .expect("the operation's stream");
    std::io::Write::write_all(&mut file, b"not a record\n").expect("appended");
    // llmlint: ignore-end[tests_mirror_real_usage]
    watch(world, &flow, &["--timeout", "0"]).ended_on(EXIT_FLOW_FAILED, "flow-failed");
    let death = journal(world, &flow, &id)
        .into_iter()
        .find(|event| event["kind"] == "landing-died")
        .expect("the death is written");
    assert!(
        death["payload"]["message"]
            .as_str()
            .is_some_and(|said| said.contains("could not be read")),
        "{death}"
    );
    verb.held.release();
}

/// `monitor --flow` ends on a cursor, and a later `monitor --flow --cursor` from it
/// shows only what was recorded since — never a line it already showed.
#[test]
fn monitor_flow_resumes_from_its_own_cursor() {
    let verb = Verb::publish("ml-monitor-cursor");
    let world = &verb.world;
    let landing = verb.start();
    {
        let mut landing = landing;
        landing.held_at_push(&verb);
        let (flow, _) = the_landing(world);
        let first = world.run(&["monitor", "--flow", &flow, "--filter", "detailed"]);
        first.exited(0).out_has("landing-registered");
        let cursor = first
            .stdout
            .lines()
            .last()
            .and_then(|line| line.strip_prefix("-- cursor "))
            .expect("the resume line is last")
            .to_owned();
        verb.held.release();
        let (code, _, said) = landing.ended();
        assert_eq!(code, 0, "{said}");
        let later = world.run(&[
            "monitor", "--flow", &flow, "--filter", "detailed", "--cursor", &cursor,
        ]);
        later
            .exited(0)
            .out_lacks("landing-registered")
            .out_has("landing-finished");
    }
}

/// A run acknowledgement and a landing's registration are serialized as a
/// branch acknowledgement is: with the registration lock held both wait, and once
/// it is let go an acknowledgement recorded first is one the landing registered
/// after, and one asked after the landing registered is refused.
#[test]
fn a_run_acknowledgement_and_a_landing_registration_never_interleave() {
    let world = World::new("ml-run-serialized");
    let _repository = world.repository("local-direct", &["false"]);
    let branch = originating_run(&world, "origin");
    let go = world.root.join("push.go");
    hold_pushes(&world, &go);
    // llmlint: ignore-block[tests_mirror_real_usage] the registration lock is taken by this
    // journey because no command holds it open past its own check and write — which is the
    // whole of what it promises — so there is no interface to hold the window open through.
    // Both contenders are the compiled binary, driven as a manager drives it.
    let flows = world.runs.join(".flows");
    std::fs::create_dir_all(&flows).expect("the flows directory");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(flows.join("landings.lock"))
        .expect("the registration lock");
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: a descriptor this journey just opened and still owns.
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    }
    // llmlint: ignore-end[tests_mirror_real_usage]
    let manager = world.as_session("manager-b");
    let landing = Started::spawn(
        &manager,
        None,
        &["publish-branch", &branch, "--repo", "service", "--no-draft"],
    );
    let acknowledging = Started::spawn(
        &world,
        None,
        &[
            "unwatched",
            "--session",
            &world.session,
            "--acknowledge",
            "origin",
            "--reason",
            "closed by hand",
        ],
    );
    world.until("both to wait on the registration lock", |_| {
        waiting_on_a_lock(landing.child.id()) && waiting_on_a_lock(acknowledging.child.id())
    });
    let acknowledgements = world.run_file("origin", "acknowledgements");
    assert!(landings(&world).is_empty());
    assert!(snapshot(&acknowledgements).is_empty());
    drop(lock);

    let (code, _, said) = acknowledging.ended();
    let (flow, id) = the_landing(&world);
    let request = sidecar(&world, &flow, &id, "request.json");
    assert_eq!(request["runs"], json!(["origin"]));
    if code == EXIT_SUCCESS {
        let recorded = snapshot(&acknowledgements);
        let written: Value = serde_json::from_str(&recorded[0].1).expect("the acknowledgement");
        let at = written["at"].as_str().expect("an instant");
        let began = request["began_at"].as_str().expect("an instant");
        assert!(at[..19] <= began[..19], "{written} {request}");
    } else {
        assert_eq!(code, REFUSED, "{said}");
        assert!(
            said.contains(&format!("has a live landing, {id}")),
            "{said}"
        );
        assert!(snapshot(&acknowledgements).is_empty());
    }
    std::fs::write(&go, "go").expect("released");
    let _ = landing.ended();
}

/// A landing's progress competes with the flow's own conditions without either
/// being lost: a run joining and a waiting surface are reported first, with the
/// landing's place kept so the next watch reports its event; and a landing's
/// event comes before a member's settlement, which a later watch still reports.
#[test]
fn a_landings_progress_and_the_flows_own_conditions_are_each_reported_once() {
    let (world, repository, held) = held_world("ml-competing", "change-open");
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    world.script("member.wait", "hold");
    let plan = world.plan(
        "member",
        &crate::harness::plan_of("member", vec![crate::harness::agent("member", &[])]),
    );
    let gate = world.root.join("end.go");
    let script = world.root.join("plan.sh");
    std::fs::write(
        &script,
        format!(
            "\"$ONEPIPELINE_FAKE_CLI_BIN\" start '{plan}' --detach >/dev/null || exit 70\n\
             \"$ONEPIPELINE_FAKE_CLI_BIN\" publish-branch {BRANCH} --repo service --no-draft >/dev/null 2>&1 &\n\
             while [ ! -f '{gate}' ]; do sleep 0.05; done\n",
            gate = gate.display()
        ),
    )
    .expect("the program is written");
    let mut parent = world
        .cmd(&[
            "flow",
            "run",
            "--name",
            "plan",
            "--",
            "sh",
            script.to_str().expect("a path"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("flow run starts");
    world.until("the landing's push to be held", |_| held.holding());
    world.until("the member run to dispatch", |world| {
        !world.events_of("member", "node-dispatched").is_empty()
    });
    let (flow, id) = the_landing(&world);
    assert_eq!(flow, "plan");

    // A run joining is reported first; the landing's events wait for the next.
    let joined = watch(
        &world,
        &flow,
        &[
            "--timeout",
            "0",
            "--filter",
            "detailed",
            "--cursor",
            "flow:2:plan::",
        ],
    );
    joined.ended_on(onepipeline::error::EXIT_RUN_JOINED, "run-joined");
    let first = watch(
        &world,
        &flow,
        &[
            "--timeout",
            "0",
            "--filter",
            "detailed",
            "--cursor",
            &joined.cursor(),
        ],
    );
    first.progressed("landing-registered");

    // A waiting surface is reported first too, and the landing's place is kept.
    world
        .run(&[
            "surface",
            "--flow",
            &flow,
            "--kind",
            "check-in",
            "--message",
            "steady",
        ])
        .exited(0);
    let surfaced = watch(
        &world,
        &flow,
        &[
            "--timeout",
            "0",
            "--filter",
            "detailed",
            "--cursor",
            &first.cursor(),
        ],
    );
    surfaced.ended_on(crate::harness::SURFACE_WAITING, "surface");
    world.run(&["next", "--flow", &flow]).exited(0);

    // The member settles; every landing event left is reported before it, once
    // each, and then the settlement.
    world.release("member.go");
    world.until("the member's node to settle", |world| {
        !world.events_of("member", "node-settled").is_empty()
    });
    let mut cursor = surfaced.cursor();
    let mut reported = vec!["landing-registered".to_owned()];
    loop {
        let next = watch(
            &world,
            &flow,
            &[
                "--timeout",
                "0",
                "--filter",
                "detailed",
                "--cursor",
                &cursor,
            ],
        );
        cursor = next.cursor();
        match next.record["condition"].as_str() {
            Some("landing-progress") => {
                reported.push(next.record["kind"].as_str().unwrap_or_default().to_owned());
            }
            Some("node-settled") => break,
            _ => panic!("{}", next.record),
        }
    }
    // Whatever the relay recorded after that pass is still the next watch's to
    // report: read to the end, every landing event has been reported exactly
    // once, in the order its journal holds them.
    loop {
        let next = watch(
            &world,
            &flow,
            &[
                "--timeout",
                "0",
                "--filter",
                "detailed",
                "--cursor",
                &cursor,
            ],
        );
        cursor = next.cursor();
        match next.record["condition"].as_str() {
            Some("landing-progress") => {
                reported.push(next.record["kind"].as_str().unwrap_or_default().to_owned());
            }
            Some("elapsed") => break,
            _ => panic!("{}", next.record),
        }
    }
    assert_eq!(
        reported,
        kinds(&world, &flow, &id),
        "every landing event once, in order, around the settlement"
    );
    held.release();
    std::fs::write(&gate, "go").expect("the flow ends");
    let _ = parent.wait();
}

/// Inside a longer flow, a landing whose progress could not be relayed is
/// settled refused — a blocker the flow's watch is woken on as that landing's
/// terminal notice — while the flow it is a step of goes on, owed its watch.
// llmlint: ignore-block[tests_mirror_real_usage] the repository's own `pre-push` hook appends a
// record no reader can parse to the operation's stream, because `onevcs` only ever appends
// whole envelopes; everything asserted is read back off the compiled binary.
#[test]
fn a_relay_that_fails_inside_a_longer_flow_is_its_landings_blocker_and_the_flow_goes_on() {
    let (world, repository) = hooked_world(
        "ml-unrelayed-nested",
        "change-open",
        "for stream in \"$ONEVCS_HOME\"/operations/o-*/events.ndjson; do printf 'not a record\\n' >> \"$stream\"; done; exit 0",
    );
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let mut parent = Parent::start(
        &world,
        "plan",
        &["publish-branch", BRANCH, "--repo", "service", "--no-draft"],
    );
    parent.landing_pid(&world);
    let (flow, id) = the_landing(&world);
    assert_eq!(flow, "plan");
    world.until("the landing to end", |world| {
        landing_dir(world, &flow, &id).join("ending.json").is_file()
    });
    let mut cursor = format!("flow:2:{flow}:{id}@0:");
    let refused = loop {
        let next = watch(&world, &flow, &["--timeout", "0", "--cursor", &cursor]);
        next.ended_on(EXIT_SUCCESS, "landing-progress");
        cursor = next.cursor();
        if next.record["kind"] == "landing-refused" {
            break next;
        }
    };
    assert_eq!(refused.record["significance"], json!("blocker"));
    assert_eq!(
        refused.record["status"],
        json!(0),
        "the verb itself succeeded"
    );
    // The flow it was a step of is still live, and owed its watch.
    watch(
        &world,
        &flow,
        &["--timeout", "0", "--cursor", &refused.cursor()],
    )
    .ended_on(ELAPSED, "elapsed");
    let owed = world.run(&["unwatched", "--session", &world.session]);
    owed.exited(OWED);
    assert!(owed.stdout.contains("LIVE"), "{}", owed.stdout);
    std::fs::write(&parent.gate, "go").expect("the flow ends");
    let _ = parent.child.wait();
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A train whose report cannot be written — its caller closed the pipe before
/// reading — exits with the refusal's code and says why on standard error, so
/// no caller reads an unshown report as a delivered one; what the train did is
/// still in the landing's own journal.
#[test]
fn integrate_whose_report_cannot_be_written_exits_non_zero_and_says_why() {
    let world = World::new("ml-unreported-train");
    let repository = world.repository("local-direct", &[]);
    for branch in TRAIN {
        branch_with_work(&world, &repository, branch, &format!("{branch}.txt"));
    }
    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);
    let mut argv = vec!["integrate"];
    argv.extend(TRAIN);
    argv.push("--push");
    let said = world.root.join("integrate.stderr");
    let status = world
        .cmd(&argv)
        .current_dir(&repository.checkout)
        .stdout(writer)
        .stderr(std::fs::File::create(&said).expect("a stderr file"))
        .status()
        .expect("integrate runs");
    let said = std::fs::read_to_string(&said).expect("what it said");
    assert_eq!(status.code(), Some(REFUSED), "{said}");
    assert!(
        said.contains("could not write its report to standard output"),
        "{said}"
    );
    let (flow, id) = the_landing(&world);
    let every = kinds(&world, &flow, &id);
    assert!(every.iter().any(|kind| kind == "base-arrived"), "{every:?}");
    assert_eq!(every.last().map(String::as_str), Some("landing-refused"));
}

/// `--until landing-progress` named alone returns on a landing's event as the
/// default set does; named without it, a watch consumes the landing's events
/// without waking, so the cursor it ends on is past them; and on a run's watch
/// the word is refused, naming the flow watch it belongs to.
#[test]
fn landing_progress_is_a_flow_watchs_condition_to_name_or_leave_out() {
    let verb = Verb::publish("ml-until");
    let world = &verb.world;
    let mut landing = verb.start();
    landing.held_at_push(&verb);
    let (flow, id) = the_landing(world);
    let named = watch(
        world,
        &flow,
        &[
            "--timeout",
            "0",
            "--filter",
            "detailed",
            "--until",
            "landing-progress",
        ],
    );
    named.progressed("landing-registered");
    // Left out, every landing event is read past and none wakes the watch: its
    // cursor is at least as far as the journal reached before it was armed.
    let reached = std::fs::metadata(landing_dir(world, &flow, &id).join("events.jsonl"))
        .expect("the journal")
        .len();
    let without = watch(
        world,
        &flow,
        &[
            "--timeout",
            "0",
            "--filter",
            "detailed",
            "--until",
            "run-joined",
        ],
    );
    without.ended_on(ELAPSED, "elapsed");
    let cursor = without.cursor();
    let place: u64 = cursor
        .split_once(&format!("{id}@"))
        .and_then(|(_, rest)| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| panic!("{cursor} places no {id}"));
    assert!(place >= reached, "{cursor} is short of {reached}");
    // On a run's watch the word is refused before anything waits.
    world.script("solo.wait", "hold");
    let plan = world.plan(
        "solo",
        &crate::harness::plan_of("solo", vec![crate::harness::agent("solo", &[])]),
    );
    world.run(&["start", &plan, "--detach"]).exited(0);
    world
        .run(&[
            "watch",
            "solo",
            "--until",
            "landing-progress",
            "--timeout",
            "0",
        ])
        .exited(REFUSED)
        .err_has("is a flow watch's condition");
    world.release("solo.go");
    verb.held.release();
    let _ = landing.ended();
}

/// An originating run whose journal refuses a mirrored record fails the
/// landing closed: refused at registration, the landing starts no work and its
/// flow is owed closure; refused while the operation's events are relayed, the
/// verb's own status stands, the landing is settled refused and its flow fails.
// llmlint: ignore-block[tests_mirror_real_usage] the run's journal is made read-only — by the
// journey before the landing starts, and by the repository's own `pre-push` hook while it
// runs — because no verb leaves a run journal that refuses an append; what is changed is the
// host's permission on that file, and everything asserted is read back off the binary.
#[test]
fn an_originating_run_that_refuses_its_mirror_fails_the_landing_closed() {
    // Refused at registration.
    let world = World::new("ml-mirror-refused");
    let _repository = world.repository("local-direct", &["false"]);
    let branch = originating_run(&world, "origin");
    onepipeline_testfakes::executable(
        &world.root.join("hooks").join("pre-push"),
        "#!/bin/sh\nexit 0\n",
    );
    let run_journal = world.run_file("origin", "events.jsonl");
    writable(&run_journal, false);
    let refused = world.run(&["publish-branch", &branch, "--repo", "service", "--no-draft"]);
    writable(&run_journal, true);
    refused.exited(REFUSED).err_has("could not be registered");
    let (flow, _) = the_landing(&world);
    watch(&world, &flow, &["--timeout", "0"]).ended_on(EXIT_FLOW_FAILED, "flow-failed");
    assert!(
        !world
            .journal("origin")
            .iter()
            .any(|event| event["kind"] == "landing-started"),
        "a landing refused at registration started"
    );

    // Refused while the operation's events are relayed.
    let world = World::new("ml-mirror-relay");
    let _repository = world.repository("local-direct", &["false"]);
    let branch = originating_run(&world, "origin");
    onepipeline_testfakes::executable(
        &world.root.join("hooks").join("pre-push"),
        "#!/bin/sh\nchmod a-w \"$ONEPIPELINE_RUNS_DIR\"/origin/events.jsonl\nexit 0\n",
    );
    let landed = world.run(&["publish-branch", &branch, "--repo", "service", "--no-draft"]);
    writable(&world.run_file("origin", "events.jsonl"), true);
    landed.exited(0).err_has("could not be relayed");
    let (flow, id) = the_landing(&world);
    let every = journal(&world, &flow, &id);
    let last = every.last().expect("a journal");
    assert_eq!(last["kind"], json!("landing-refused"), "{every:?}");
    assert_eq!(last["payload"]["status"], json!(0));
    let failed = watch(&world, &flow, &["--timeout", "0"]);
    failed.ended_on(EXIT_FLOW_FAILED, "flow-failed");
    assert_eq!(failed.record["status"], json!(1));
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A launcher session that is set and is not text may name a manager, so the
/// landing is refused rather than run unowned, with nothing done.
#[test]
fn a_launcher_session_that_is_not_text_refuses_the_landing() {
    use std::os::unix::ffi::OsStrExt;
    let world = World::new("ml-session-bytes");
    let repository = world.repository("change-open", &[]);
    branch_with_work(&world, &repository, BRANCH, "widget.txt");
    let mut command = world.cmd(&["publish-branch", BRANCH, "--repo", "service", "--no-draft"]);
    command.env(
        "ONEPIPELINE_LAUNCHER_SESSION",
        std::ffi::OsStr::from_bytes(b"manager-\xff"),
    );
    world
        .run_on(command, "publish-branch under a session that is not text")
        .exited(REFUSED)
        .err_has("is set and is not text");
    assert!(world.changes_opened().is_empty());
    assert!(!world.runs.join(".flows").exists());
}
