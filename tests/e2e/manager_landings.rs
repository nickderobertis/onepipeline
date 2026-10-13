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
        let (world, repository, held) = held_world(name, "local-direct");
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
            Verb::integrate("ml-no-integrate"),
            words(&["integrate", "train-a", "missing", "--push"]),
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
        let every = journal(world, &flow, &id);
        let last = every.last().expect("a journal");
        if code == 0 {
            // A train that skips a member it cannot find still lands the rest:
            // `onevcs` reports the skip and succeeds, and so does the landing.
            assert_eq!(verb.operation, "integrate", "{out}\n{said}");
            assert_eq!(last["kind"], json!("landing-finished"));
            assert!(out.contains("missing: skipped"), "{out}");
            continue;
        }
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
    // A cursor printed before landings resumes, replaying none of them.
    watch(
        &world,
        &flow,
        &["--timeout", "0", "--cursor", &format!("flow:1:{flow}:")],
    )
    .ended_on(EXIT_SUCCESS, "ended");
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

    // Now the request is unreadable.
    let request = landing_dir(&world, &flow, &id).join("request.json");
    std::fs::write(&request, "{").expect("tampered");
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
    let refused = budget();
    assert_eq!(refused["verdict"], json!("block"), "{refused}");
    assert!(
        refused["reason"]
            .as_str()
            .is_some_and(|why| why.contains("--timeout 598")),
        "{refused}"
    );
    let in_time = Armed::arm(world, &flow, &["--timeout", "598"]);
    assert_eq!(budget(), json!({"verdict": "none"}));
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
