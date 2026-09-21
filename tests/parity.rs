//! The drift gate between the binary and `onepipeline::verbs`.
//!
//! `docs/contract.md` states the rule in one sentence: the CLI is argument
//! parsing over the `verbs` calls, and a post-launch behaviour the binary has
//! and the SDK lacks is a defect. Nothing holds that rule but a journey that
//! drives both over one run and reads them against each other — so every
//! journey here runs the compiled binary (`CARGO_BIN_EXE_onepipeline`) and the
//! `verbs` call over the same recorded run, and holds the binary's **stdout and
//! exit code** to the SDK result rendered by its public renderer, byte for byte.
//! A refusal is held the same way: the binary's stderr is the SDK's error under
//! the one prefix `main` puts on it, and the exit code is the error's own.
//!
//! The run is the checked-in `tests/recorded/run-root/onemessagebus-repair-2`,
//! a finished run 0.28.2 drove, copied into a runs root of its own per journey.
//! Its driver is proved gone on any host the way `tests/e2e/recorded_channel.rs`
//! proves it: every command is told it runs on the recording host, and the copy's
//! launch record names a pid no platform issues. The sibling is pointed at an
//! executable that is not there, so the provider-health block a detail view
//! sources from it is silence on both sides rather than a probe of this host.
//!
//! A verb that reads is driven over one copy by both sides. A verb that writes —
//! `next` over a channel, `reply`, `surface`, `stop`, `adopt`, `drive-run` — is
//! driven over two copies of the same fixture, one each, so what each side
//! answers is what it answered over the run as it stood.

// llmlint: ignore-file[e2e_not_mocked] nothing inside the crate is substituted: one side of
// every comparison is the compiled binary as a subprocess and the other is the public SDK
// linked into this process, over a real run root a real release recorded. The one thing
// pointed elsewhere is the sibling's health probe, at the crate's own documented override,
// so a detail view does not sweep this host's identities into the comparison.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use onepipeline::channel::SurfaceKind;
use onepipeline::cli::{WatchTimeout, WatchUntil};
use onepipeline::error::{EXIT_REFUSED, EXIT_SUCCESS};
use onepipeline::filter::EventFilter;
use onepipeline::verbs;
use onepipeline::views::RunPaths;
use serde_json::Value;

/// The recorded run every journey copies.
const RUN: &str = "onemessagebus-repair-2";

/// The session the recorded launch record names as the run's owner.
const SESSION: &str = "28f7a3f8-23c8-535f-bf37-c37d2dfb31bd";

/// A session that owns nothing here.
const STRANGER: &str = "another-planner";

/// The host the recorded launch record names.
const RECORDING_HOST: &str = "U-17UN402ICR95C";

/// A pid no platform issues: above Linux's `PID_MAX_LIMIT` and macOS's
/// `PID_MAX`, and not a multiple of four, which every Windows pid is.
const NO_PROCESS: u32 = 2_147_483_647;

/// An executable that is not there, at the crate's own sibling override, so the
/// provider-health block is silence on both sides.
const NO_SIBLING: &str = "/nonexistent/onepipeline-parity/oneagentgraph";

/// What the binary prints a refusal under.
const REFUSAL_PREFIX: &str = "onepipeline: ";

/// The process environment, held for the length of a journey.
///
/// The SDK side reads the runs root, the host and the sibling override out of
/// this process's environment exactly as the binary reads them out of its own,
/// so a journey sets them for the process and holds this lock while they stand.
static ENV: Mutex<()> = Mutex::new(());

/// One runs root holding one copy of the recorded run.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// A fresh copy of the recorded run under a runs root of its own.
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir()
            .join(format!("onepipeline-parity-{}-{name}", std::process::id()))
            .join("runs");
        let _ = std::fs::remove_dir_all(root.parent().expect("a parent"));
        let run = root.join(RUN);
        copied(&recorded(&format!("run-root/{RUN}")), &run);
        let launch = run.join("launch.json");
        let record = std::fs::read_to_string(&launch).expect("the launch record reads");
        let driver = "\"pid\": 3682555,";
        assert_eq!(
            record.matches(driver).count(),
            1,
            "the recorded launch record names its driver once"
        );
        std::fs::write(
            &launch,
            record.replace(driver, &format!("\"pid\": {NO_PROCESS},")),
        )
        .expect("the launch record is written");
        // The dispatch registry every run driven by this build keeps: a stop
        // reads it to find the work the driver started, and refuses a run whose
        // registry it cannot read at all.
        std::fs::create_dir_all(run.join("dispatches")).expect("a dispatch registry");
        Self { root }
    }

    fn paths(&self) -> RunPaths {
        RunPaths::under(&self.root, RUN)
    }

    /// Seed the run with one of the recorded channel directories.
    fn with_channel(self, fixture: &str) -> Self {
        copied(
            &recorded(&format!("channel/{fixture}")),
            &self.root.join(RUN).join("channel"),
        );
        self
    }

    /// Put this fixture's world into the process environment, for the SDK.
    fn enter(&self) -> MutexGuard<'static, ()> {
        let held = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for (key, value) in self.environment(SESSION) {
            std::env::set_var(key, value);
        }
        held
    }

    /// The environment every command runs under.
    fn environment(&self, session: &str) -> Vec<(&'static str, String)> {
        vec![
            ("ONEPIPELINE_RUNS_DIR", self.root.display().to_string()),
            ("HOSTNAME", RECORDING_HOST.to_owned()),
            ("ONEPIPELINE_LAUNCHER_SESSION", session.to_owned()),
            ("ONEPIPELINE_ONEAGENTGRAPH_BIN", NO_SIBLING.to_owned()),
        ]
    }

    fn command(&self, session: &str, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_onepipeline"));
        command.args(args).envs(self.environment(session));
        command.stdin(Stdio::null());
        command
    }

    /// Run the binary as the run's owner.
    fn binary(&self, args: &[&str]) -> Output {
        self.command(SESSION, args)
            .output()
            .expect("the binary runs")
    }

    /// Run the binary as another session.
    fn binary_as(&self, session: &str, args: &[&str]) -> Output {
        self.command(session, args)
            .output()
            .expect("the binary runs")
    }

    /// Run the binary with something on its standard input.
    fn binary_with_stdin(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = self
            .command(SESSION, args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the binary starts");
        {
            use std::io::Write;
            let mut pipe = child.stdin.take().expect("a stdin pipe");
            pipe.write_all(stdin.as_bytes())
                .expect("the input is written");
        }
        child.wait_with_output().expect("the binary runs")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(parent) = self.root.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

fn recorded(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/recorded")
        .join(relative)
}

fn copied(from: &Path, into: &Path) {
    std::fs::create_dir_all(into).expect("a directory to seed");
    for entry in std::fs::read_dir(from).expect("a recorded directory") {
        let entry = entry.expect("a recorded file");
        if entry.file_name() == "README.md" {
            continue;
        }
        std::fs::copy(entry.path(), into.join(entry.file_name())).expect("a recorded file copies");
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn exit(output: &Output) -> i32 {
    output.status.code().expect("the binary exited")
}

/// Hold the binary's answer to the SDK's rendering: the same bytes on standard
/// output, and the same exit code.
fn same(what: &str, output: &Output, rendered: &str, code: i32) {
    assert_eq!(
        stdout(output),
        rendered,
        "{what}: the binary's stdout is not the SDK's rendering\n--- stderr ---\n{}",
        stderr(output)
    );
    assert_eq!(exit(output), code, "{what}: the exit codes differ");
}

/// Hold the binary's refusal to the SDK's error: nothing on standard output,
/// the error's own words on standard error under the binary's prefix, and the
/// error's own exit code.
fn refused(what: &str, output: &Output, error: &onepipeline::Error) {
    assert_eq!(
        stdout(output),
        "",
        "{what}: a refusal printed something on stdout"
    );
    assert_eq!(
        stderr(output),
        format!("{REFUSAL_PREFIX}{error}\n"),
        "{what}: the binary's refusal is not the SDK's error"
    );
    assert_eq!(
        exit(output),
        error.exit_code(),
        "{what}: the exit codes differ"
    );
}

/// A rendering that ends the way `println!` ends it.
fn line(rendered: String) -> String {
    format!("{rendered}\n")
}

#[test]
fn runs_lists_grouped_by_default_flat_on_request_and_narrowed_to_a_session() {
    let fixture = Fixture::new("runs");
    let _env = fixture.enter();

    let projects = verbs::runs(&fixture.root, SESSION, false);
    same(
        "runs",
        &fixture.binary(&["runs"]),
        &verbs::render_runs(&projects, verbs::Grouping::Grouped, SESSION),
        EXIT_SUCCESS,
    );
    same(
        "runs --flat",
        &fixture.binary(&["runs", "--flat"]),
        &verbs::render_runs(&projects, verbs::Grouping::Flat, SESSION),
        EXIT_SUCCESS,
    );
    // Narrowed to a session that owns nothing here: the flat and the grouped
    // listing both say so, and say it as the binary says it.
    let theirs = verbs::runs(&fixture.root, STRANGER, true);
    same(
        "runs --mine, as a stranger",
        &fixture.binary_as(STRANGER, &["runs", "--mine"]),
        &verbs::render_runs(&theirs, verbs::Grouping::Grouped, STRANGER),
        EXIT_SUCCESS,
    );
    // And the whole listing read by that stranger, whose rows carry the owner's
    // label rather than `[mine]`.
    let seen = verbs::runs(&fixture.root, STRANGER, false);
    same(
        "runs, as a stranger",
        &fixture.binary_as(STRANGER, &["runs"]),
        &verbs::render_runs(&seen, verbs::Grouping::Grouped, STRANGER),
        EXIT_SUCCESS,
    );
}

#[test]
fn status_lists_every_run_details_one_and_refuses_one_that_is_not_there() {
    let fixture = Fixture::new("status");
    let _env = fixture.enter();

    let listing = verbs::status(&fixture.root, None).expect("the listing reads");
    same(
        "status",
        &fixture.binary(&["status"]),
        &verbs::render_status(&listing),
        EXIT_SUCCESS,
    );
    let detail = verbs::status(&fixture.root, Some(RUN)).expect("the run reads");
    same(
        "status RUN",
        &fixture.binary(&["status", RUN]),
        &verbs::render_status(&detail),
        EXIT_SUCCESS,
    );
    let missing = verbs::status(&fixture.root, Some("nowhere")).expect_err("no such run");
    refused(
        "status nowhere",
        &fixture.binary(&["status", "nowhere"]),
        &missing,
    );
    let navigating = verbs::status(&fixture.root, Some("../elsewhere")).expect_err("not a run id");
    refused(
        "status ../elsewhere",
        &fixture.binary(&["status", "../elsewhere"]),
        &navigating,
    );
}

#[test]
fn host_reports_the_live_dispatches_under_the_root() {
    let fixture = Fixture::new("host");
    let _env = fixture.enter();

    same(
        "host",
        &fixture.binary(&["host"]),
        &verbs::render_host(&verbs::host(&fixture.root)),
        EXIT_SUCCESS,
    );
}

#[test]
fn goals_lists_every_run_grouped_and_details_one() {
    let fixture = Fixture::new("goals");
    let _env = fixture.enter();

    let listing = verbs::goals(&fixture.root, None).expect("the listing reads");
    same(
        "goals",
        &fixture.binary(&["goals"]),
        &verbs::render_goals(&listing),
        EXIT_SUCCESS,
    );
    let detail = verbs::goals(&fixture.root, Some(RUN)).expect("the run reads");
    same(
        "goals RUN",
        &fixture.binary(&["goals", RUN]),
        &verbs::render_goals(&detail),
        EXIT_SUCCESS,
    );
    let missing = verbs::goals(&fixture.root, Some("nowhere")).expect_err("no such run");
    refused(
        "goals nowhere",
        &fixture.binary(&["goals", "nowhere"]),
        &missing,
    );
}

#[test]
fn results_reports_each_nodes_outcome() {
    let fixture = Fixture::new("results");
    let _env = fixture.enter();

    let results = verbs::results(&fixture.paths()).expect("the run reads");
    same(
        "results RUN",
        &fixture.binary(&["results", RUN]),
        &verbs::render_results(&results),
        EXIT_SUCCESS,
    );
}

#[test]
fn transcript_renders_every_node_one_node_and_refuses_an_unknown_one() {
    let fixture = Fixture::new("transcript");
    let _env = fixture.enter();

    let whole = verbs::transcript(&fixture.paths(), None).expect("the run reads");
    same(
        "transcript RUN",
        &fixture.binary(&["transcript", RUN]),
        &verbs::render_transcript(&whole),
        EXIT_SUCCESS,
    );
    let one = verbs::transcript(&fixture.paths(), Some("plan")).expect("the node has records");
    same(
        "transcript RUN plan",
        &fixture.binary(&["transcript", RUN, "plan"]),
        &verbs::render_transcript(&one),
        EXIT_SUCCESS,
    );
    let unknown = verbs::transcript(&fixture.paths(), Some("nope")).expect_err("no such node");
    refused(
        "transcript RUN nope",
        &fixture.binary(&["transcript", RUN, "nope"]),
        &unknown,
    );
}

/// Write a pointer file into the fixture, through the linked core's own types:
/// two sessions, one of them two harness runs long, under two nodes of the run.
///
/// The recorded run predates the pointer file, so what the journey holds the
/// binary and the SDK to is a file written the way every oneharness under a
/// launch writes one — one line per harness run, in the shape `HistoryPointer`
/// admits — rather than a recording of one.
fn write_pointer_file(fixture: &Fixture) {
    use oneharness_core::domain::harness::HarnessIdentity;
    use oneharness_core::domain::history::{
        HistoryId, HistoryLabels, HistoryPointer, PointerSession,
    };
    use oneharness_core::domain::usage::UtcInstant;
    use std::collections::BTreeMap;

    let store = if cfg!(windows) { "C:\\store" } else { "/store" };
    let cwd = if cfg!(windows) { "C:\\work" } else { "/work" };
    let mut text = String::new();
    for (session, node, suffix, started) in [
        ("turn-a", "plan", 1, 100),
        ("turn-b", "implement", 2, 200),
        ("turn-a", "plan", 3, 300),
    ] {
        let labels = HistoryLabels::new(BTreeMap::from([
            (
                onepipeline::agents::RUN_ID_LABEL.to_string(),
                RUN.to_string(),
            ),
            (
                onepipeline::agents::NODE_LABEL.to_string(),
                node.to_string(),
            ),
            (
                onepipeline::agents::SCOPE_LABEL.to_string(),
                onepipeline::agents::Scope::Node.as_str().to_string(),
            ),
            ("role".to_string(), "worker".to_string()),
        ]))
        .expect("valid labels");
        let session = PointerSession::new(
            Path::new(store),
            &Path::new(store)
                .join("proj")
                .join(format!("{session}.jsonl")),
            "turn",
            cwd,
            labels,
        )
        .expect("a session");
        let identity: HarnessIdentity = "claude-code".parse().expect("an identity");
        let history_id: HistoryId = format!("00000000-0000-4000-8000-{suffix:012}")
            .parse()
            .expect("a history id");
        let pointer = HistoryPointer::new(
            &session,
            history_id,
            &identity,
            UtcInstant::from_epoch(started),
        )
        .expect("a pointer");
        text.push_str(&serde_json::to_string(&pointer).expect("serialises"));
        text.push('\n');
    }
    std::fs::write(fixture.paths().oneharness_sessions(), text)
        .expect("the pointer file is written");
}

#[test]
fn agents_lists_a_runs_sessions_a_nodes_and_a_projects_and_reads_none_as_empty() {
    let fixture = Fixture::new("agents");
    let _env = fixture.enter();

    // No pointer file yet: an empty list, and not a refusal.
    let none = verbs::agents(&fixture.paths(), verbs::AgentScope::Run).expect("no file reads");
    assert!(none.sessions.is_empty());
    same(
        "agents RUN (no pointer file)",
        &fixture.binary(&["agents", RUN]),
        &verbs::render_agents(&none),
        EXIT_SUCCESS,
    );

    write_pointer_file(&fixture);
    let whole = verbs::agents(&fixture.paths(), verbs::AgentScope::Run).expect("the run reads");
    assert_eq!(whole.sessions.len(), 2, "{whole:?}");
    assert_eq!(whole.sessions[0].runs.len(), 2, "{whole:?}");
    same(
        "agents RUN",
        &fixture.binary(&["agents", RUN]),
        &verbs::render_agents(&whole),
        EXIT_SUCCESS,
    );
    let one = verbs::agents(&fixture.paths(), verbs::AgentScope::Node("implement"))
        .expect("the node reads");
    assert_eq!(one.sessions.len(), 1, "{one:?}");
    same(
        "agents RUN implement",
        &fixture.binary(&["agents", RUN, "implement"]),
        &verbs::render_agents(&one),
        EXIT_SUCCESS,
    );
    let project = "authoring:onemessagebus-repair-2";
    let across = verbs::project_agents(&fixture.root, project).expect("the project reads");
    assert_eq!(
        across, whole,
        "one run of the project is the run's own list"
    );
    same(
        "agents --project PROJECT",
        &fixture.binary(&["agents", "--project", project]),
        &verbs::render_agents(&across),
        EXIT_SUCCESS,
    );
    let unknown =
        verbs::project_agents(&fixture.root, "plans:nowhere").expect_err("no such project");
    refused(
        "agents --project plans:nowhere",
        &fixture.binary(&["agents", "--project", "plans:nowhere"]),
        &unknown,
    );
    let missing = verbs::agents(
        &RunPaths::under(&fixture.root, "nowhere"),
        verbs::AgentScope::Run,
    );
    // The SDK reads a run it is handed paths for; the binary resolves the id
    // first, so an unknown run is the resolver's refusal on that side alone.
    assert!(
        missing.is_ok(),
        "an absent run root is an absent pointer file"
    );
    let output = fixture.binary(&["agents", "nowhere"]);
    assert_eq!(exit(&output), EXIT_REFUSED, "{}", stderr(&output));
}

#[test]
fn telemetry_renders_each_runs_document_and_its_breakdown() {
    let fixture = Fixture::new("telemetry");
    let _env = fixture.enter();

    let every = verbs::telemetry(&fixture.root, None).expect("the listing reads");
    same(
        "telemetry",
        &fixture.binary(&["telemetry"]),
        &verbs::render_telemetry(&every, false).expect("it renders"),
        EXIT_SUCCESS,
    );
    let one = verbs::telemetry(&fixture.root, Some(RUN)).expect("the run reads");
    same(
        "telemetry RUN --breakdown",
        &fixture.binary(&["telemetry", RUN, "--breakdown"]),
        &verbs::render_telemetry(&one, true).expect("it renders"),
        EXIT_SUCCESS,
    );
    assert_eq!(
        verbs::render_telemetry(&one, true).expect("it renders"),
        verbs::render_telemetry_breakdown(&one[0]),
        "one run's breakdown is the breakdown renderer's own"
    );
    let missing = verbs::telemetry(&fixture.root, Some("nowhere")).expect_err("no such run");
    refused(
        "telemetry nowhere",
        &fixture.binary(&["telemetry", "nowhere"]),
        &missing,
    );
}

#[test]
fn monitor_renders_the_stream_from_the_start_from_a_cursor_and_refuses_a_foreign_one() {
    let fixture = Fixture::new("monitor");
    let _env = fixture.enter();
    let all = EventFilter::default();

    let whole = verbs::monitor(&fixture.paths(), &all, None).expect("the run reads");
    same(
        "monitor RUN --all",
        &fixture.binary(&["monitor", RUN, "--all"]),
        &line(verbs::render_monitor(&whole)),
        EXIT_SUCCESS,
    );
    // Resumed from the cursor the first pass printed, which reads nothing new.
    let resumed =
        verbs::monitor(&fixture.paths(), &all, Some(&whole.cursor)).expect("the cursor resolves");
    same(
        "monitor RUN --all --cursor C",
        &fixture.binary(&["monitor", RUN, "--all", "--cursor", &whole.cursor]),
        &line(verbs::render_monitor(&resumed)),
        EXIT_SUCCESS,
    );
    assert!(
        resumed.events.is_empty(),
        "a cursor at the end reads nothing"
    );
    let foreign = verbs::monitor(&fixture.paths(), &all, Some("1:elsewhere:5"))
        .expect_err("another run's cursor");
    refused(
        "monitor RUN --cursor 1:elsewhere:5",
        &fixture.binary(&["monitor", RUN, "--all", "--cursor", "1:elsewhere:5"]),
        &foreign,
    );
}

#[test]
fn next_answers_a_run_with_nothing_waiting_and_claims_what_is_waiting() {
    // Nothing waiting: a finished run with no channel at all answers `finished`
    // and consumes nothing, so one copy serves both sides.
    let quiet = Fixture::new("next-quiet");
    let _env = quiet.enter();
    let all = EventFilter::default();
    let nothing = verbs::next(&quiet.paths(), &all).expect("the run reads");
    same(
        "next RUN --all, nothing waiting",
        &quiet.binary(&["next", RUN, "--all"]),
        &line(verbs::render_next(&nothing)),
        EXIT_SUCCESS,
    );
    assert_eq!(nothing.status, verbs::NextStatus::Finished);
    drop(_env);
    drop(quiet);

    // Updates waiting: each side claims the oldest off its own copy of the same
    // recorded channel, and each answers the same surface.
    let theirs = Fixture::new("next-binary").with_channel("waiting-updates");
    let ours = Fixture::new("next-sdk").with_channel("waiting-updates");
    let _env = ours.enter();
    let claimed = verbs::next(&ours.paths(), &all).expect("the claim");
    same(
        "next RUN --all, updates waiting",
        &theirs.binary(&["next", RUN, "--all"]),
        &line(verbs::render_next(&claimed)),
        EXIT_SUCCESS,
    );
    assert_eq!(claimed.status, verbs::NextStatus::Surface);
    assert!(claimed.surface.is_some(), "a waiting surface was claimed");
}

#[test]
fn channel_queue_reads_the_channel_and_refuses_a_run_that_is_not_there() {
    let fixture = Fixture::new("channel").with_channel("domain-driven-modularity-2");
    let _env = fixture.enter();

    let queue = verbs::channel(&fixture.paths()).expect("the channel reads");
    same(
        "channel queue RUN",
        &fixture.binary(&["channel", "queue", RUN]),
        &line(verbs::render_channel(&queue).expect("it renders")),
        EXIT_SUCCESS,
    );
    // The recorded channel has surfaces that were answered, replies carrying
    // edits, and command outcomes: a queue read off it is not an empty one.
    assert!(!queue.surfaces.is_empty() && !queue.answered().is_empty());
    assert!(!queue.replies.is_empty() && !queue.outcomes.is_empty());
    let missing = verbs::channel(&RunPaths::under(&fixture.root, "nowhere")).expect_err("no run");
    refused(
        "channel queue nowhere",
        &fixture.binary(&["channel", "queue", "nowhere"]),
        &missing,
    );
}

#[test]
fn watch_reads_a_settled_run_once_and_returns_and_refuses_a_foreign_cursor() {
    let fixture = Fixture::new("watch");
    let _env = fixture.enter();
    let request = verbs::WatchRequest {
        filter: EventFilter::default(),
        timeout: WatchTimeout::Bounded(0),
        tick: Duration::from_secs(30),
        cursor: None,
        until: vec![WatchUntil::Surface],
    };

    let mut machine = String::new();
    let outcome = verbs::watch(&fixture.paths(), &request, &mut |frame| {
        let lines = verbs::render_watch_frame(&frame)?;
        machine.push_str(&lines.machine);
        machine.push('\n');
        Ok(())
    })
    .expect("the watch returns");
    same(
        "watch RUN --all --timeout 0",
        &fixture.binary(&["watch", RUN, "--all", "--timeout", "0"]),
        &machine,
        outcome.exit_code(),
    );
    assert_eq!(outcome.ending, verbs::WatchEnding::Settled);

    // A sink that refuses ends the wait with its own refusal, and nothing more
    // is handed to it — which is the way the binary's own sink ends a wait when
    // the pipe it writes to has closed: a refusal, exit 2, and the reason naming
    // the stream. The two refusals are worded by different sinks, and the
    // answer is the same.
    let mut handed = 0;
    let refusal = verbs::watch(&fixture.paths(), &request, &mut |_| {
        handed += 1;
        Err(onepipeline::Error::Refused(
            "the reader has gone".to_owned(),
        ))
    })
    .expect_err("a refusing sink ends the wait");
    assert_eq!(refusal.to_string(), "refused: the reader has gone");
    assert_eq!(refusal.exit_code(), EXIT_REFUSED);
    assert_eq!(handed, 1, "a frame was handed to a sink that had refused");
    let mut closed = fixture
        .command(SESSION, &["watch", RUN, "--all", "--timeout", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts");
    drop(closed.stdout.take());
    let closed = closed.wait_with_output().expect("the binary exits");
    assert_eq!(exit(&closed), EXIT_REFUSED, "{}", stderr(&closed));
    assert!(
        stderr(&closed).contains("the watch could not write to standard output"),
        "{}",
        stderr(&closed)
    );

    let foreign = verbs::WatchRequest {
        cursor: Some("1:elsewhere:5".to_owned()),
        ..request
    };
    let refusal = verbs::watch(&fixture.paths(), &foreign, &mut |_| Ok(()))
        .expect_err("another run's cursor");
    refused(
        "watch RUN --cursor 1:elsewhere:5",
        &fixture.binary(&[
            "watch",
            RUN,
            "--all",
            "--timeout",
            "0",
            "--cursor",
            "1:elsewhere:5",
        ]),
        &refusal,
    );
}

#[test]
fn unwatched_reports_nothing_for_a_settled_run_and_a_run_not_proven_settled() {
    let fixture = Fixture::new("unwatched");
    let _env = fixture.enter();

    // The recorded run settled, and a listing has just left its summary document
    // current — so nothing is reported.
    fixture.binary(&["runs"]);
    let settled = verbs::unwatched(&fixture.root, SESSION).expect("the root reads");
    same(
        "unwatched, settled",
        &fixture.binary(&["unwatched", "--session", SESSION]),
        &verbs::render_unwatched(&settled),
        settled.exit_code(),
    );
    assert!(settled.reported.is_empty());

    // A document that records no settlement — what a run still recording leaves
    // — is a run nothing is watching, and both sides say so alike.
    let summary = fixture.root.join(RUN).join("summary.json");
    let mut document: Value =
        serde_json::from_str(&std::fs::read_to_string(&summary).expect("the document"))
            .expect("a summary");
    document["graph_complete"] = Value::Bool(false);
    std::fs::write(&summary, document.to_string()).expect("the document is written");
    let reported = verbs::unwatched(&fixture.root, SESSION).expect("the root reads");
    same(
        "unwatched, not proven settled",
        &fixture.binary(&["unwatched", "--session", SESSION]),
        &verbs::render_unwatched(&reported),
        reported.exit_code(),
    );
    assert_eq!(reported.reported.len(), 1);
    assert_eq!(reported.reported[0].run, RUN);
}

#[test]
fn reply_refuses_a_malformed_envelope_and_a_verdict_nobody_will_read_and_applies_an_edit() {
    let theirs = Fixture::new("reply-binary");
    let ours = Fixture::new("reply-sdk");
    let _env = ours.enter();

    let malformed = r#"{"nope":"#;
    let refusal = verbs::reply(&ours.paths(), None, malformed).expect_err("malformed");
    refused(
        "reply RUN, malformed",
        &theirs.binary_with_stdin(&["reply", RUN], malformed),
        &refusal,
    );

    // A verdict to a run that has settled: nothing will ever read it.
    let verdict = r#"{"completion": false, "reason": "carry on"}"#;
    let refusal = verbs::reply(&ours.paths(), None, verdict).expect_err("a settled run");
    refused(
        "reply RUN, verdict on a settled run",
        &theirs.binary_with_stdin(&["reply", RUN], verdict),
        &refusal,
    );

    // An edit, applied by the reply itself because nothing is driving the run.
    let edit = r###"{"version": 2, "commands": [{"op": "add", "node": {"id": "extra", "persona": "engineer", "task": "## What\ndo more"}}]}"###;
    let receipt = verbs::reply(&ours.paths(), None, edit).expect("applied");
    same(
        "reply RUN, applied",
        &theirs.binary_with_stdin(&["reply", RUN], edit),
        &line(verbs::render_receipt(&receipt).expect("it renders")),
        receipt.exit_code(),
    );
    assert!(matches!(
        receipt.outcome,
        verbs::ReplyOutcome::AppliedHere { .. }
    ));
    assert!(receipt.advice.is_empty(), "{:?}", receipt.advice);
}

#[test]
fn surface_queues_a_finding_and_next_hands_it_out() {
    let theirs = Fixture::new("surface-binary");
    let ours = Fixture::new("surface-sdk");
    let _env = ours.enter();

    let surfaced = verbs::surface(
        &ours.paths(),
        SurfaceKind::finding(),
        "the gate is red".to_owned(),
    )
    .expect("queued");
    same(
        "surface RUN --kind finding --message",
        &theirs.binary(&[
            "surface",
            RUN,
            "--kind",
            "finding",
            "--message",
            "the gate is red",
        ]),
        &line(verbs::render_surfaced(&surfaced)),
        EXIT_SUCCESS,
    );
    // A message with nothing in it is refused before anything is queued, on
    // both sides; the binary names where it looked and the SDK cannot, so the
    // words differ and the answer does not.
    let empty = verbs::surface(&ours.paths(), SurfaceKind::finding(), "  ".to_owned())
        .expect_err("nothing to say");
    let blank = theirs.binary(&["surface", RUN, "--kind", "finding", "--message", "  "]);
    assert_eq!(stdout(&blank), "");
    assert_eq!(exit(&blank), empty.exit_code());
    assert_eq!(exit(&blank), EXIT_REFUSED);
}

#[test]
fn attest_refuses_a_reference_nothing_is_waiting_on() {
    let fixture = Fixture::new("attest");
    let _env = fixture.enter();

    let refusal = verbs::attest(&fixture.paths(), "sign-off").expect_err("nothing waits");
    refused(
        "attest RUN sign-off",
        &fixture.binary(&["attest", RUN, "sign-off"]),
        &refusal,
    );
}

#[test]
fn stop_refuses_a_strangers_run_unless_forced_and_stops_an_owned_one() {
    let theirs = Fixture::new("stop-binary");
    let ours = Fixture::new("stop-sdk");
    let _env = ours.enter();

    let not_owned = verbs::stop(
        &ours.paths(),
        verbs::StopRequest {
            session: STRANGER,
            force: false,
        },
    )
    .expect_err("not this session's run");
    assert!(matches!(not_owned, onepipeline::Error::NotOwned { .. }));
    refused(
        "stop RUN, as a stranger",
        &theirs.binary_as(STRANGER, &["stop", RUN]),
        &not_owned,
    );

    let stopped = verbs::stop(
        &ours.paths(),
        verbs::StopRequest {
            session: SESSION,
            force: false,
        },
    )
    .expect("a clean stop");
    same(
        "stop RUN",
        &theirs.binary(&["stop", RUN]),
        &line(verbs::render_stopped(&stopped)),
        stopped.exit_code(),
    );
    assert!(stopped.clean && !stopped.forced);
    assert_eq!(stopped.teardown, verbs::StopTeardown::NothingToStop);
    assert_eq!(stopped.refusal(), None);

    // Forced by a stranger, over fresh copies: the owner is named and the run
    // is stopped anyway.
    let theirs = Fixture::new("stop-forced-binary");
    let ours = Fixture::new("stop-forced-sdk");
    drop(_env);
    let _env = ours.enter();
    let forced = verbs::stop(
        &ours.paths(),
        verbs::StopRequest {
            session: STRANGER,
            force: true,
        },
    )
    .expect("a forced stop");
    same(
        "stop RUN --force, as a stranger",
        &theirs.binary_as(STRANGER, &["stop", RUN, "--force"]),
        &line(verbs::render_stopped(&forced)),
        forced.exit_code(),
    );
    assert!(forced.clean && forced.forced);
}

#[test]
fn an_attached_adoption_drives_a_complete_run_to_its_settlement() {
    let theirs = Fixture::new("adopt-binary");
    let ours = Fixture::new("adopt-sdk");
    let _env = ours.enter();

    let adopted = verbs::adopt(&ours.paths(), verbs::Adopt::Attached).expect("it settles");
    same(
        "adopt RUN",
        &theirs.binary(&["adopt", RUN]),
        &line(verbs::render_adopted(&adopted)),
        adopted.exit_code(),
    );
    assert!(matches!(
        adopted,
        verbs::Adopted::Attached {
            settlement: verbs::Settlement::Complete,
            ..
        }
    ));
}

#[test]
fn a_detached_adoption_retains_the_stated_program_and_answers_its_pid() {
    let theirs = Fixture::new("adopt-detached-binary");
    let ours = Fixture::new("adopt-detached-sdk");
    let _env = ours.enter();

    // What the binary retains is itself at its hidden verb; the SDK is told the
    // same program and the same arguments, and appends nothing.
    let retain = verbs::Retain {
        program: PathBuf::from(env!("CARGO_BIN_EXE_onepipeline")),
        args: vec!["drive-run".to_owned(), RUN.to_owned(), "--adopt".to_owned()],
    };
    let adopted =
        verbs::adopt(&ours.paths(), verbs::Adopt::Detached(retain)).expect("a driver claimed");
    let verbs::Adopted::Detached { pid, .. } = &adopted else {
        panic!("a detached adoption answers a pid: {adopted:?}");
    };
    let announced = theirs.binary(&["adopt", RUN, "--detach"]);
    assert_eq!(exit(&announced), adopted.exit_code());

    // The one thing the two lines cannot share is the pid each retained: held
    // equal with that field taken out, and each side's pid held to the driver
    // its own launch record names.
    let mut printed: Value = serde_json::from_str(stdout(&announced).trim()).expect("one object");
    let mut rendered: Value =
        serde_json::from_str(&verbs::render_adopted(&adopted)).expect("one object");
    let their_pid = printed["pid"].as_u64().expect("the binary names a pid");
    assert_eq!(rendered["pid"].as_u64(), Some(u64::from(*pid)));
    printed["pid"] = Value::Null;
    rendered["pid"] = Value::Null;
    assert_eq!(printed, rendered, "the announcements differ beyond the pid");
    for (fixture, pid) in [(&theirs, their_pid), (&ours, u64::from(*pid))] {
        let record: Value = serde_json::from_str(
            &std::fs::read_to_string(fixture.root.join(RUN).join("launch.json"))
                .expect("the launch record reads"),
        )
        .expect("a launch record");
        assert_eq!(
            record["pid"].as_u64(),
            Some(pid),
            "the record names the driver"
        );
    }
    // Both drivers settle the complete run and let go of it, so nothing is
    // left running behind this journey.
    for fixture in [&theirs, &ours] {
        let lock = fixture.root.join(RUN).join("owner.lock");
        let deadline = Instant::now() + Duration::from_secs(60);
        while lock.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !lock.exists(),
            "the retained driver never let go of the run"
        );
    }
}

#[test]
fn drive_run_is_the_body_of_the_hidden_verb() {
    let theirs = Fixture::new("drive-run-binary");
    let ours = Fixture::new("drive-run-sdk");
    let _env = ours.enter();

    let code = verbs::drive_run(&ours.paths(), verbs::Retained::Driving).expect("it drives");
    same(
        "drive-run RUN",
        &theirs.binary(&["drive-run", RUN]),
        "",
        code,
    );
    assert_eq!(code, EXIT_SUCCESS, "a complete graph settles clean");
}

/// The plan loader reads the recorded run's plan as the typed schema, so a
/// consumer reads a node off it without parsing the file itself.
#[test]
fn the_plan_loader_reads_a_recorded_runs_plan() {
    let fixture = Fixture::new("plan");
    let plan = onepipeline::views::plan_of(&fixture.paths()).expect("the plan reads");
    assert_eq!(plan.name.as_deref(), Some(RUN));
    let node = plan
        .tasks
        .iter()
        .find(|node| node.id == "plan")
        .expect("the recorded plan has its one node");
    assert!(
        node.task
            .as_deref()
            .is_some_and(|task| task.contains("Repair the 7 criteria")),
        "{node:?}"
    );
    let missing = onepipeline::views::plan_of(&RunPaths::under(&fixture.root, "nowhere"))
        .expect_err("no plan to read");
    assert!(matches!(missing, onepipeline::Error::Ledger { .. }));
}

/// The listing's liveness reading over the bounded document is the word the
/// listing prints for the run.
#[test]
fn liveness_over_the_summary_is_the_listings_own_reading() {
    let fixture = Fixture::new("liveness");
    let _env = fixture.enter();
    let projects = verbs::runs(&fixture.root, SESSION, false);
    let summary = projects.flat().into_iter().next().expect("one run");
    // A driver proved gone on the recording host, and the run settled: the row
    // says `SETTLED`, and the liveness under that word is `DRIVER DEAD`.
    assert_eq!(
        onepipeline::views::liveness_of(summary),
        onepipeline::views::DriverLiveness::DriverDead
    );
    assert!(
        verbs::render_runs(&projects, verbs::Grouping::Flat, SESSION).contains("SETTLED"),
        "{}",
        verbs::render_runs(&projects, verbs::Grouping::Flat, SESSION)
    );
}

/// `shutdown` is argument parsing over `verbs::shutdown`: the binary's report
/// and exit code are the SDK's rendering and `exit_code`, byte for byte, and a
/// refusal is the SDK's error.
///
/// Both sides run over a fresh copy at the **same** runs root, one after the
/// other, because the report names the root it read. `ONEVCS_HOME` points both
/// at one empty state root of their own, so neither reads this host's own
/// branches into the section the report ends on.
#[test]
fn shutdown_is_the_sdks_report_and_exit_code() {
    let onevcs_home = std::env::temp_dir().join(format!(
        "onepipeline-parity-{}-shutdown-onevcs",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&onevcs_home);
    std::fs::create_dir_all(&onevcs_home).expect("an empty onevcs state root");
    let request = |scope: verbs::ShutdownScope, session: &str| verbs::ShutdownRequest {
        scope,
        session: session.to_owned(),
        grace: Duration::from_secs(600),
        force: true,
    };

    let ours = Fixture::new("shutdown");
    let env = ours.enter();
    std::env::set_var("ONEVCS_HOME", &onevcs_home);
    let not_owned = verbs::shutdown(
        &ours.root,
        request(verbs::ShutdownScope::Run(RUN.to_owned()), STRANGER),
    )
    .expect_err("not this session's run");
    assert!(matches!(not_owned, onepipeline::Error::NotOwned { .. }));
    let shutdown = verbs::shutdown(
        &ours.root,
        request(verbs::ShutdownScope::Run(RUN.to_owned()), SESSION),
    )
    .expect("the run is shut down");
    std::env::remove_var("ONEVCS_HOME");
    drop(env);
    drop(ours);

    let theirs = Fixture::new("shutdown");
    refused(
        "shutdown RUN, as a stranger",
        &theirs
            .command(STRANGER, &["shutdown", RUN, "--force"])
            .env("ONEVCS_HOME", &onevcs_home)
            .output()
            .expect("the binary runs"),
        &not_owned,
    );
    same(
        "shutdown RUN --force",
        &theirs
            .command(SESSION, &["shutdown", RUN, "--force"])
            .env("ONEVCS_HOME", &onevcs_home)
            .output()
            .expect("the binary runs"),
        &verbs::render_shutdown(&shutdown),
        shutdown.exit_code(),
    );
    let _ = std::fs::remove_dir_all(&onevcs_home);
}
