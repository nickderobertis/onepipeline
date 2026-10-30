//! Human-item journeys against the live store: the real compiled CLI over a real
//! runs root, its real host journal, read index and `onemessagebus` queues.
//! Nothing is substituted; every assertion is on exit code, stdout, stderr, or
//! what the binary left on disk.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_onepipeline");

struct Live {
    root: PathBuf,
}

impl Live {
    /// A world whose repository-tracked configuration holds `config` under
    /// `human_items`, or no configuration file at all.
    fn new(config: Option<&str>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::var_os("ONEPIPELINE_NODE_SCRATCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "human-items-live-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("xdg")).unwrap();
        std::fs::create_dir_all(root.join("runs")).unwrap();
        let world = Self { root };
        if let Some(config) = config {
            world.configure(config);
        }
        world
    }
    fn enabled() -> Self {
        Self::new(Some("human_items:\n  enabled: true\n"))
    }
    fn configure(&self, config: &str) {
        std::fs::create_dir_all(self.root.join("repo/.onepipeline")).unwrap();
        std::fs::write(self.root.join("repo/.onepipeline/config.yaml"), config).unwrap();
    }
    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }
    fn runs(&self) -> PathBuf {
        self.root.join("runs")
    }
    fn store(&self) -> PathBuf {
        self.runs().join("human-items")
    }
    fn cmd(&self, session: Option<&str>, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .arg("human-items")
            .args(args)
            .current_dir(self.repo())
            .env("ONEPIPELINE_RUNS_DIR", self.runs())
            .env("XDG_CONFIG_HOME", self.root.join("xdg"))
            .env("USER", "operator")
            .env_remove("ONEPIPELINE_CONFIG")
            .env_remove("ONEPIPELINE_HUMAN_ITEMS_TELEMETRY")
            .env_remove("ONEPIPELINE_LAUNCHER_SESSION");
        if let Some(session) = session {
            command.env("ONEPIPELINE_LAUNCHER_SESSION", session);
        }
        command
    }
    fn run(&self, session: Option<&str>, args: &[&str]) -> Output {
        self.cmd(session, args).output().unwrap()
    }
    fn ok(&self, session: Option<&str>, args: &[&str]) -> String {
        let output = self.run(session, args);
        assert!(
            output.status.success(),
            "{args:?} exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    fn json(&self, session: Option<&str>, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(session, args)).unwrap()
    }
    fn refused(&self, session: Option<&str>, args: &[&str], reason: &str) -> String {
        let output = self.run(session, args);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(!output.status.success(), "{args:?} succeeded: {stderr}");
        assert!(stderr.contains(reason), "{args:?}: {stderr} lacks {reason}");
        stderr
    }
    fn input(&self, name: &str, value: &Value) -> String {
        let path = self.root.join(format!("{name}.json"));
        std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path.to_string_lossy().into_owned()
    }
    /// Raise one item through `raise --input`; answers its id.
    fn raise(&self, value: &Value) -> String {
        let name = value["dedup_key"]
            .as_str()
            .unwrap()
            .replace(['/', ':'], "-");
        let path = self.input(&name, value);
        let receipt = self.json(None, &["raise", "--input", &path]);
        receipt["item"]["item"]["id"].as_str().unwrap().to_owned()
    }
    fn journal(&self) -> Vec<Value> {
        std::fs::read_to_string(self.store().join("events.ndjson"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn watch(&self, session: &str) -> Vec<Value> {
        let output = self.run(Some(session), &["watch"]);
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn item(dedup: &str, kind: &str, priority: &str, session: &str, block: Value) -> Value {
    json!({
        "schema_version": 1,
        "dedup_key": dedup,
        "kind": kind,
        "title": format!("{kind} {dedup}"),
        "body": format!("The full body of {dedup}, which every listing prints."),
        "priority": priority,
        "owner": {"session": session, "run": null, "plan": null},
        "block": block,
        "approval": null
    })
}

fn ids(views: &Value) -> Vec<String> {
    views
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["item"]["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn human_items_are_unavailable_until_configuration_enables_them() {
    for config in [None, Some("human_items:\n  enabled: false\n")] {
        let world = Live::new(config);
        let config = world.json(None, &["config", "--json"]);
        assert_eq!(config["human_items"]["enabled"], false);
        let input = world.input(
            "q",
            &item("q", "question", "low", "s", json!({"scope":"nothing"})),
        );
        for args in [
            vec!["list"],
            vec!["report"],
            vec!["show", "hi-x"],
            vec!["raise", "--input", input.as_str()],
            vec!["answer", "hi-x", "--text", "yes"],
            vec!["complete", "hi-x"],
            vec!["dismiss", "hi-x"],
            vec!["watch", "--session", "s"],
        ] {
            world.refused(None, &args, r#"{"kind":"unavailable"}"#);
        }
        assert!(!world.store().join("events.ndjson").exists());
    }
}

#[test]
fn ten_sessions_raise_both_kinds_and_each_watch_sees_only_its_own_events() {
    let world = Live::enabled();
    let priorities = ["urgent", "high", "medium", "low"];
    let mut questions = Vec::new();
    let mut tasks = Vec::new();
    for n in 0..10 {
        let session = format!("manager-{n}");
        questions.push(world.raise(&item(
            &format!("q-{n}"),
            "question",
            priorities[n % 4],
            &session,
            json!({"scope": "nothing"}),
        )));
        tasks.push(world.raise(&item(
            &format!("t-{n}"),
            "task",
            priorities[(n + 1) % 4],
            &session,
            json!({"scope": "nothing"}),
        )));
    }
    // Raising again is idempotent: the same item and event, no new record.
    let again = world.json(
        None,
        &[
            "raise",
            "--input",
            &world.input(
                "again",
                &item(
                    "q-0",
                    "question",
                    "urgent",
                    "manager-0",
                    json!({"scope":"nothing"}),
                ),
            ),
        ],
    );
    assert_eq!(again["item"]["item"]["id"], questions[0]);
    assert_eq!(again["event_id"], "human-items:0");
    assert_eq!(world.journal().len(), 20);

    // Every session's first watch — a new process each, which is all a restart
    // of an owner is — sees its own two raises and nothing else's.
    for n in 0..10 {
        let events = world.watch(&format!("manager-{n}"));
        assert_eq!(events.len(), 2, "manager-{n}: {events:?}");
        for event in &events {
            assert_eq!(event["kind"], "human-item-raised");
            assert_eq!(event["payload"]["owner"]["session"], format!("manager-{n}"));
        }
        // Read once: the next watch has nothing and says so with its exit.
        let output = world.run(Some(&format!("manager-{n}")), &["watch"]);
        assert_eq!(output.status.code(), Some(5));
        assert!(output.stdout.is_empty());
    }

    // Default order: open only, blocking first, then priority, then creation.
    let all = world.json(None, &["list", "--json"]);
    assert_eq!(all.as_array().unwrap().len(), 20);
    let order: Vec<String> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["item"]["priority"].as_str().unwrap().to_owned())
        .collect();
    let mut sorted = order.clone();
    sorted.sort_by_key(|p| priorities.iter().position(|q| q == p));
    assert_eq!(order, sorted);

    // Resolve: a question answered by the human directly, a task completed by a
    // manager on the human's behalf, one of each dismissed with and without a
    // reason. Each resolution reaches only its owner's next watch.
    let answered = world.json(
        None,
        &[
            "answer",
            &questions[1],
            "--text",
            "Use the gradual rollout.",
        ],
    );
    assert_eq!(
        answered["item"]["item"]["resolution"]["actor"],
        "user:operator"
    );
    assert_eq!(
        answered["item"]["item"]["resolution"]["on_behalf_of"],
        Value::Null
    );
    let completed = world.json(Some("manager-2"), &["complete", &tasks[2]]);
    assert_eq!(
        completed["item"]["item"]["resolution"]["actor"],
        "session:manager-2"
    );
    assert_eq!(
        completed["item"]["item"]["resolution"]["on_behalf_of"],
        "user:operator"
    );
    let dismissed = world.json(
        None,
        &["dismiss", &questions[3], "--reason", "  Not needed now  "],
    );
    assert_eq!(
        dismissed["item"]["item"]["resolution"]["reason"],
        "Not needed now"
    );
    let plain = world.json(None, &["dismiss", &tasks[4], "--reason", "   "]);
    assert_eq!(plain["item"]["item"]["resolution"]["reason"], Value::Null);
    for (session, kind, id) in [
        ("manager-1", "human-item-answered", &questions[1]),
        ("manager-2", "human-item-completed", &tasks[2]),
        ("manager-3", "human-item-dismissed", &questions[3]),
        ("manager-4", "human-item-dismissed", &tasks[4]),
    ] {
        let events = world.watch(session);
        assert_eq!(events.len(), 1, "{session}: {events:?}");
        assert_eq!(events[0]["kind"], kind);
        assert_eq!(&events[0]["payload"]["item_id"], id);
    }
    for quiet in ["manager-0", "manager-5", "manager-9"] {
        assert!(world.watch(quiet).is_empty());
    }

    // After the restart every process is, the durable receipt reads back.
    let shown = world.json(None, &["show", &tasks[2], "--json"]);
    assert_eq!(shown["item"]["state"], "completed");
    assert_eq!(shown["item"]["resolution"]["on_behalf_of"], "user:operator");
    let state = world.json(None, &["list", "--json", "--state", "answered"]);
    assert_eq!(ids(&state), vec![questions[1].clone()]);
    // No question writes a tracker.
    for view in world
        .json(None, &["list", "--json", "--kind", "question"])
        .as_array()
        .unwrap()
    {
        assert_eq!(view["item"]["tracker"], Value::Null);
    }
}

#[test]
fn actions_refuse_wrong_kinds_conflicts_and_blank_answers_and_repeat_idempotently() {
    let world = Live::enabled();
    let question = world.raise(&item(
        "q",
        "question",
        "high",
        "s",
        json!({"scope":"nothing"}),
    ));
    let task = world.raise(&item("t", "task", "low", "s", json!({"scope":"nothing"})));
    world.refused(
        None,
        &["complete", &question],
        "is a question, which cannot be completed",
    );
    world.refused(
        None,
        &["answer", &task, "--text", "done"],
        "is a task, which cannot be answered",
    );
    world.refused(
        None,
        &["answer", &question, "--text", "   "],
        "answer text must not be blank",
    );
    world.refused(
        None,
        &["answer", "hi-missing", "--text", "x"],
        "no such human item hi-missing",
    );
    let first = world.json(None, &["answer", &question, "--text", "Yes."]);
    let repeat = world.json(None, &["answer", &question, "--text", "Yes."]);
    assert_eq!(first, repeat);
    world.refused(
        None,
        &["answer", &question, "--text", "No."],
        "already answered",
    );
    world.refused(None, &["dismiss", &question], "already answered");
    let done = world.json(None, &["complete", &task]);
    assert_eq!(world.json(None, &["complete", &task]), done);
    world.refused(
        None,
        &["dismiss", &task, "--reason", "late"],
        "already completed",
    );
    // Every refusal wrote nothing: two raises and two resolutions.
    assert_eq!(world.journal().len(), 4);
    let shown = world.json(None, &["show", &question, "--json"]);
    assert_eq!(shown["item"]["resolution"]["text"], "Yes.");
}

#[test]
fn concurrent_writers_lose_no_raise_and_one_resolution_wins() {
    let world = Live::enabled();
    let children: Vec<_> = (0..16)
        .map(|n| {
            let path = world.input(
                &format!("c{n}"),
                &item(
                    &format!("c-{n}"),
                    "task",
                    "medium",
                    &format!("s{n}"),
                    json!({"scope":"nothing"}),
                ),
            );
            world
                .cmd(None, &["raise", "--input", &path])
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        assert!(child.wait_with_output().unwrap().status.success());
    }
    let journal = world.journal();
    assert_eq!(journal.len(), 16);
    let mut seqs: Vec<u64> = journal.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    seqs.sort_unstable();
    assert_eq!(seqs, (0..16).collect::<Vec<_>>());
    assert_eq!(
        world
            .json(None, &["list", "--json"])
            .as_array()
            .unwrap()
            .len(),
        16
    );

    // Two different resolutions of one item race: exactly one is recorded.
    let id = ids(&world.json(None, &["list", "--json"]))[0].clone();
    let racers: Vec<_> = ["complete", "dismiss"]
        .iter()
        .map(|verb| world.cmd(None, &[verb, &id]).spawn().unwrap())
        .collect();
    let outcomes: Vec<bool> = racers
        .into_iter()
        .map(|child| child.wait_with_output().unwrap().status.success())
        .collect();
    assert_eq!(outcomes.iter().filter(|ok| **ok).count(), 1, "{outcomes:?}");
    assert_eq!(world.journal().len(), 17);
}

#[test]
fn a_question_raised_outside_a_run_waits_for_its_disconnected_owner() {
    let world = Live::enabled();
    let id = world.raise(&item(
        "outside",
        "question",
        "medium",
        "lonely",
        json!({"scope":"nothing"}),
    ));
    // The owner's watch reads its raise and goes away.
    assert_eq!(world.watch("lonely").len(), 1);
    // While no watch of it is running, the human answers.
    world.json(None, &["answer", &id, "--text", "Go ahead."]);
    // It is still answered after any number of restarts, and the owner's next
    // watch — a new process — receives the answer exactly once.
    let shown = world.json(None, &["show", &id, "--json"]);
    assert_eq!(shown["item"]["state"], "answered");
    let events = world.watch("lonely");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "human-item-answered");
    assert_eq!(events[0]["payload"]["resolution"]["text"], "Go ahead.");
    assert!(world.watch("lonely").is_empty());
}

#[test]
fn list_and_report_print_labels_sources_bodies_and_blocking_wording() {
    let world = Live::enabled();
    world.raise(&item("host", "task", "low", "s1", json!({"scope":"host"})));
    world.raise(&item(
        "plan",
        "task",
        "medium",
        "s2",
        json!({"scope":"plan","plan":"plans:alpha"}),
    ));
    world.raise(&item(
        "q",
        "question",
        "urgent",
        "s3",
        json!({"scope":"nothing"}),
    ));
    for verb in ["list", "report"] {
        let text = world.ok(None, &[verb]);
        for expected in [
            "Low",
            "Medium",
            "Critical",
            "blocks all work on this host",
            "blocks all of plans:alpha",
            "session s3",
            "The full body of host",
            "The full body of plan",
            "The full body of q",
        ] {
            assert!(text.contains(expected), "{verb}: {expected} in {text}");
        }
        for absent in [
            "owner-ended",
            "owner_ended",
            "scope",
            "P1",
            "P2",
            "P3",
            "urgent",
        ] {
            assert!(!text.contains(absent), "{verb}: {absent} in {text}");
        }
    }
    // Every open host item blocks, whatever its priority: the Low host item is
    // in the blocking group ahead of the Critical question that blocks nothing.
    let blocking = world.json(None, &["list", "--json", "--blocking", "yes"]);
    let scopes: Vec<&Value> = blocking
        .as_array()
        .unwrap()
        .iter()
        .map(|v| &v["item"]["block"]["scope"])
        .collect();
    assert_eq!(scopes, [&json!("plan"), &json!("host")]);
    let all = world.json(None, &["list", "--json"]);
    assert_eq!(all[2]["item"]["kind"], "question");
    // Critical maps to urgent data; the filters compose.
    let critical = world.json(None, &["list", "--json", "--priority", "critical"]);
    assert_eq!(critical[0]["item"]["priority"], "urgent");
    let none = world.json(
        None,
        &[
            "list",
            "--json",
            "--priority",
            "Critical",
            "--blocking",
            "yes",
        ],
    );
    assert!(none.as_array().unwrap().is_empty());
    let host = world.json(None, &["list", "--json", "--host"]);
    assert_eq!(host.as_array().unwrap().len(), 1);
    let session = world.json(
        None,
        &[
            "list",
            "--json",
            "--session",
            "s2",
            "--kind",
            "task",
            "--state",
            "open",
        ],
    );
    assert_eq!(session.as_array().unwrap().len(), 1);
    let report = world.ok(None, &["report"]);
    assert!(
        report.starts_with("Human items: 3 listed, 2 blocking"),
        "{report}"
    );
}

fn telemetry(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["read"].clone())
        .filter(|read| !read.is_null())
        .collect()
}

fn list_with_telemetry(world: &Live, path: &Path) -> (Value, Value) {
    let _ = std::fs::remove_file(path);
    let output = world
        .cmd(None, &["list", "--json"])
        .env("ONEPIPELINE_HUMAN_ITEMS_TELEMETRY", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reads = telemetry(path);
    assert_eq!(reads.len(), 1, "{reads:?}");
    (
        serde_json::from_slice(&output.stdout).unwrap(),
        reads[0].clone(),
    )
}

#[test]
fn the_read_index_is_warm_tails_exactly_once_and_rebuilds_whatever_breaks_it() {
    let world = Live::enabled();
    for n in 0..5 {
        world.raise(&item(
            &format!("i-{n}"),
            "task",
            "low",
            "s",
            json!({"scope":"nothing"}),
        ));
    }
    let probe = world.root.join("telemetry.ndjson");
    let (authoritative, _) = list_with_telemetry(&world, &probe);
    // Warm: unchanged, nothing replayed.
    let (warm, read) = list_with_telemetry(&world, &probe);
    assert_eq!(warm, authoritative);
    assert_eq!(read["mode"], "warm");
    assert_eq!(read["item_events_replayed"], 0);
    assert_eq!(read["item_bytes_replayed"], 0);

    // Ten appended events are consumed once, by the first reader, and never again.
    for n in 5..15 {
        world.raise(&item(
            &format!("i-{n}"),
            "task",
            "low",
            "s",
            json!({"scope":"nothing"}),
        ));
    }
    // The writer's own catch-up consumed them; a reader finds nothing new.
    let (fifteen, read) = list_with_telemetry(&world, &probe);
    assert_eq!(fifteen.as_array().unwrap().len(), 15);
    assert_eq!(read["item_events_replayed"], 0);
    // Appended behind the index's back by another process's raw journal append —
    // the tail a reader catches up — ten records exactly once across readers.
    let journal = world.store().join("events.ndjson");
    let index = world.store().join("index.json");
    let saved_index = std::fs::read(&index).unwrap();
    for n in 15..25 {
        world.raise(&item(
            &format!("i-{n}"),
            "task",
            "low",
            "s",
            json!({"scope":"nothing"}),
        ));
    }
    std::fs::write(&index, &saved_index).unwrap();
    let readers: Vec<_> = (0..4)
        .map(|n| {
            let path = world.root.join(format!("reader-{n}.ndjson"));
            let child = world
                .cmd(None, &["list", "--json"])
                .env("ONEPIPELINE_HUMAN_ITEMS_TELEMETRY", &path)
                .spawn()
                .unwrap();
            (child, path)
        })
        .collect();
    let mut tailed = 0;
    for (child, path) in readers {
        assert!(child.wait_with_output().unwrap().status.success());
        let read = &telemetry(&path)[0];
        tailed += read["item_events_replayed"].as_u64().unwrap();
        assert_ne!(read["mode"], "cold");
    }
    assert_eq!(tailed, 10);
    let (current, _) = list_with_telemetry(&world, &probe);
    assert_eq!(current.as_array().unwrap().len(), 25);

    // Each break of the index is a cold rebuild from the journal, to the same content.
    let rebuilt = |why: &str| {
        let (after, read) = list_with_telemetry(&world, &probe);
        assert_eq!(after, current, "{why}");
        assert_eq!(read["mode"], "cold", "{why}");
        assert_eq!(read["item_events_replayed"], 25, "{why}");
        read["rebuilt_because"].as_str().unwrap().to_owned()
    };
    std::fs::remove_file(&index).unwrap();
    assert!(rebuilt("deleted").contains("no index"));
    let mut tampered: Value = serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
    tampered["items"].as_object_mut().unwrap().clear();
    std::fs::write(&index, serde_json::to_vec(&tampered).unwrap()).unwrap();
    assert!(rebuilt("checksum").contains("checksum"));
    let mut versioned: Value = serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
    versioned["version"] = json!(99);
    std::fs::write(&index, serde_json::to_vec(&versioned).unwrap()).unwrap();
    assert!(rebuilt("version").contains("version"));
    std::fs::write(&index, b"{not json").unwrap();
    assert!(rebuilt("garbled").contains("cannot be parsed"));

    // A journal replaced under the index — same length, another first record — is
    // detected, rebuilt and read as what it now holds, never through a stale index.
    let bytes = std::fs::read(&journal).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let replaced = text.replacen("\"title\":\"task i-0\"", "\"title\":\"task z-0\"", 2);
    assert_eq!(replaced.len(), bytes.len());
    std::fs::write(&journal, &replaced).unwrap();
    let (after, read) = list_with_telemetry(&world, &probe);
    assert_eq!(read["mode"], "cold");
    assert!(
        read["rebuilt_because"]
            .as_str()
            .unwrap()
            .contains("replaced"),
        "{read}"
    );
    assert!(after
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["item"]["title"] == "task z-0"));
    // Records out of the order one writer gives them are a journal no writer made:
    // refused out loud rather than folded.
    let mut swapped = lines.clone();
    swapped.swap(0, 1);
    std::fs::write(&journal, format!("{}\n", swapped.join("\n"))).unwrap();
    world.refused(None, &["list"], "not above the");
    std::fs::write(&journal, &bytes).unwrap();
    list_with_telemetry(&world, &probe);

    // Truncated: a shorter journal is rebuilt, and lists what it still holds.
    let short = format!("{}\n", lines[..20].join("\n"));
    std::fs::write(&journal, short).unwrap();
    let (after, read) = list_with_telemetry(&world, &probe);
    assert_eq!(read["mode"], "cold");
    assert!(read["rebuilt_because"]
        .as_str()
        .unwrap()
        .contains("truncated"));
    assert_eq!(after.as_array().unwrap().len(), 20);
    std::fs::write(&journal, &bytes).unwrap();

    // A partial trailing record is one a writer is still writing: it waits,
    // and is listed once whole.
    let mut partial = bytes.clone();
    let extra = world.root.join("extra.json");
    std::fs::write(
        &extra,
        serde_json::to_vec(&item(
            "late",
            "task",
            "low",
            "s",
            json!({"scope":"nothing"}),
        ))
        .unwrap(),
    )
    .unwrap();
    let late = world.ok(None, &["raise", "--input", extra.to_str().unwrap()]);
    let full = std::fs::read(&journal).unwrap();
    let record = &full[bytes.len()..];
    partial.extend_from_slice(&record[..record.len() / 2]);
    std::fs::write(&journal, &partial).unwrap();
    let (waiting, _) = list_with_telemetry(&world, &probe);
    assert_eq!(waiting.as_array().unwrap().len(), 25, "{late}");
    std::fs::write(&journal, &full).unwrap();
    let (whole, _) = list_with_telemetry(&world, &probe);
    assert_eq!(whole.as_array().unwrap().len(), 26);

    // A whole record that cannot be read is refused out loud, never skipped.
    let mut corrupt = full.clone();
    corrupt.extend_from_slice(b"{\"this is\": \"not an envelope\"}\n");
    std::fs::write(&journal, &corrupt).unwrap();
    world.refused(None, &["list"], "cannot be read");
}

#[test]
fn malformed_configuration_refuses_every_consumer_by_its_file() {
    let world = Live::new(Some("human_items:\n  enabled: true\n  hooks:\n    - phase: review\n      kind: task\n      title: ''\n      body: b\n      priority: low\n      block: {scope: nothing}\n"));
    world.refused(None, &["config"], "title and body must not be blank");
    world.refused(None, &["list"], "title and body must not be blank");
    let typo = Live::new(Some("human_items:\n  enabld: true\n"));
    typo.refused(None, &["list"], "enabld");
    // A host-local override wins over the repository's shared default.
    let layered = Live::new(Some(
        "human_items:\n  enabled: true\n  task_source: plans\n",
    ));
    let host = layered.root.join("host.yaml");
    std::fs::write(&host, "human_items:\n  task_source: private\n").unwrap();
    let config: Value = serde_json::from_slice(
        &layered
            .cmd(None, &["config", "--json"])
            .env("ONEPIPELINE_CONFIG", &host)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(config["human_items"]["enabled"], true);
    assert_eq!(config["human_items"]["task_source"], "private");
    assert_eq!(config["sources"].as_array().unwrap().len(), 2);
}

const HOOKS: &str = r#"human_items:
  enabled: true
  task_source: plans
  approval_adapters:
    approve-design:
      command: ["sh", "ADAPTER"]
  hooks:
    - phase: design-review
      kind: task
      title: "Approve the design for {{ plan }}"
      body: "Read the design document and approve it in chat."
      priority: high
      block: {scope: plan}
      approval: {adapter: approve-design}
    - phase: ticket-review
      kind: task
      title: "Review the tickets of {{ plan }}"
      body: "Triage the tickets below."
      priority: low
      block: {scope: nothing}
      aggregate: ticket_review
    - phase: ask
      kind: question
      title: "Which {{ thing }}?"
      body: "Pick one {{ thing }}."
      priority: medium
      block: {scope: nothing}
"#;

/// A world whose hooks include a design review completed through a real
/// command: it records `{board_copy,key,actor,on_behalf_of}` and answers the
/// current key held in `current-key`, recording an approval only when the
/// stored key is current.
fn hooked() -> Live {
    let world = Live::new(None);
    let adapter = world.root.join("adapter.sh");
    std::fs::write(
        &adapter,
        format!(
            r#"request=$(cat)
dir={dir}
echo "$request" >> "$dir/requests.ndjson"
current=$(cat "$dir/current-key")
key=$(printf '%s' "$request" | sed 's/.*"key":"\([^"]*\)".*/\1/')
copy=$(printf '%s' "$request" | sed 's/.*"board_copy":"\([^"]*\)".*/\1/')
if [ -f "$dir/refuse" ]; then echo "the board is locked" >&2; exit 3; fi
if [ "$key" = "$current" ]; then echo "$request" >> "$dir/approvals.ndjson"; fi
printf '{{"board_copy":"%s","key":"%s"}}' "$copy" "$current"
"#,
            dir = world.root.display()
        ),
    )
    .unwrap();
    std::fs::write(world.root.join("current-key"), "v1").unwrap();
    world.configure(&HOOKS.replace("ADAPTER", &adapter.to_string_lossy()));
    world
}

fn context(world: &Live, name: &str, value: &Value) -> String {
    world.input(name, value)
}

fn phase(world: &Live, session: Option<&str>, phase: &str, context: &str) -> Value {
    world.json(session, &["raise", "--phase", phase, "--context", context])
}

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_design_review_completes_through_the_configured_authority_or_is_replaced() {
    let world = hooked();
    let owner = json!({"session": "mgr", "run": null, "plan": "plans:alpha"});
    let ctx = context(
        &world,
        "design",
        &json!({
            "owner": owner, "dedup_key": "alpha", "approval": {"board_copy": "plans:doc-1", "key": "v1"}
        }),
    );
    let raised = phase(&world, None, "design-review", &ctx);
    let id = raised["item"]["item"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        raised["item"]["item"]["title"],
        "Approve the design for plans:alpha"
    );
    assert_eq!(raised["item"]["item"]["approval"]["key"], "v1");

    // The authority refuses: nothing recorded, the task stays open.
    std::fs::write(world.root.join("refuse"), "").unwrap();
    world.refused(Some("mgr"), &["complete", &id], "the board is locked");
    assert_eq!(
        world.json(None, &["show", &id, "--json"])["item"]["state"],
        "open"
    );
    std::fs::remove_file(world.root.join("refuse")).unwrap();

    // The document changed: no approval is recorded, the obsolete task is
    // dismissed, and exactly one replacement is raised for the current version.
    std::fs::write(world.root.join("current-key"), "v2").unwrap();
    world.refused(
        Some("mgr"),
        &["complete", &id],
        "Re-read the current document",
    );
    assert!(lines(&world.root.join("approvals.ndjson")).is_empty());
    let obsolete = world.json(None, &["show", &id, "--json"]);
    assert_eq!(obsolete["item"]["state"], "dismissed");
    assert_eq!(
        obsolete["item"]["resolution"]["reason"],
        "Re-read the current document"
    );
    let open = world.json(None, &["list", "--json"]);
    assert_eq!(open.as_array().unwrap().len(), 1);
    let replacement = open[0]["item"]["id"].as_str().unwrap().to_owned();
    assert_eq!(open[0]["item"]["approval"]["key"], "v2");
    // Completing the obsolete task again creates nothing.
    world.refused(Some("mgr"), &["complete", &id], "already dismissed");
    assert_eq!(
        world
            .json(None, &["list", "--json"])
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // The current version: the manager completes on the user's behalf after
    // their approval in its chat; the authority records exactly that, then the
    // task completes and its plan hold is gone.
    let done = world.json(Some("mgr"), &["complete", &replacement]);
    assert_eq!(done["item"]["item"]["state"], "completed");
    assert_eq!(done["item"]["item"]["resolution"]["actor"], "session:mgr");
    assert_eq!(
        done["item"]["item"]["resolution"]["on_behalf_of"],
        "user:operator"
    );
    let approvals = lines(&world.root.join("approvals.ndjson"));
    assert_eq!(
        approvals,
        vec![json!({
            "board_copy": "plans:doc-1", "key": "v2", "actor": "session:mgr", "on_behalf_of": "user:operator"
        })]
    );
    assert!(world
        .json(None, &["list", "--json", "--blocking", "yes"])
        .as_array()
        .unwrap()
        .is_empty());

    // A human completing directly is recorded as themselves.
    std::fs::write(world.root.join("current-key"), "v9").unwrap();
    let direct = context(
        &world,
        "direct",
        &json!({
            "owner": owner, "dedup_key": "beta", "approval": {"board_copy": "plans:doc-2", "key": "v9"}
        }),
    );
    let id = phase(&world, None, "design-review", &direct)["item"]["item"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    world.json(None, &["complete", &id]);
    let last = lines(&world.root.join("approvals.ndjson")).pop().unwrap();
    assert_eq!(last["actor"], "user:operator");
    assert_eq!(last["on_behalf_of"], Value::Null);
}

#[test]
fn no_command_reaches_the_authority_except_the_one_configuration_names() {
    let world = hooked();
    let owner = json!({"session": "mgr", "run": null, "plan": "plans:alpha"});
    // A context cannot name a command, and an ordinary raise cannot carry an
    // approval reference — so no item data chooses what runs at completion.
    let injected = context(
        &world,
        "injected",
        &json!({
            "owner": owner, "dedup_key": "x", "approval": {"board_copy": "b", "key": "v1"},
            "command": ["sh", "-c", "touch /tmp/pwned"]
        }),
    );
    world.refused(
        None,
        &["raise", "--phase", "design-review", "--context", &injected],
        "unknown field",
    );
    let mut raw = item("raw", "task", "high", "mgr", json!({"scope":"nothing"}));
    raw["approval"] = json!({"board_copy": "b", "key": "v1"});
    let path = world.input("raw", &raw);
    world.refused(
        None,
        &["raise", "--input", &path],
        "accepted only from a configured phase",
    );
    world.refused(
        None,
        &["raise", "--phase", "unconfigured", "--context", &injected],
        "unknown field",
    );
    let plain = context(&world, "plain", &json!({"owner": owner, "dedup_key": "y"}));
    world.refused(
        None,
        &["raise", "--phase", "unconfigured", "--context", &plain],
        "no hook is configured for phase unconfigured",
    );
    world.refused(
        None,
        &["raise", "--phase", "design-review", "--context", &plain],
        "names no document version",
    );
    world.refused(
        None,
        &["raise", "--phase", "ask", "--context", &plain],
        "does not expand",
    );
    assert!(lines(&world.root.join("requests.ndjson")).is_empty());
    assert!(world.journal().is_empty());
}

fn tickets(spec: &[(&str, Option<&str>)]) -> Value {
    Value::Array(
        spec.iter()
            .map(|(id, p)| match p {
                Some(p) => json!({"id": id, "url": format!("https://tracker/{id}"), "priority": p}),
                None => json!({"id": id, "url": format!("https://tracker/{id}")}),
            })
            .collect(),
    )
}

#[test]
fn a_ticket_review_derives_its_priority_at_every_boundary() {
    let world = hooked();
    let owner = json!({"session": "mgr", "run": null, "plan": "plans:alpha"});
    type Case<'a> = (&'a str, Vec<(&'a str, Option<&'a str>)>, &'a str);
    let cases: Vec<Case> = vec![
        (
            "urgent",
            vec![("a", Some("urgent")), ("b", Some("low"))],
            "urgent",
        ),
        ("one-high", vec![("a", Some("high"))], "medium"),
        (
            "two-high",
            vec![("a", Some("high")), ("b", Some("high"))],
            "medium",
        ),
        (
            "three-high",
            vec![
                ("a", Some("high")),
                ("b", Some("high")),
                ("c", Some("high")),
            ],
            "high",
        ),
        (
            "zero-medium",
            vec![("a", Some("low")), ("b", Some("low"))],
            "low",
        ),
        (
            "one-medium",
            vec![("a", Some("low")), ("b", Some("medium"))],
            "low",
        ),
        (
            "two-medium",
            vec![("a", Some("medium")), ("b", Some("medium"))],
            "medium",
        ),
        ("low", vec![("a", Some("low"))], "low"),
        (
            "unknown-and-lows",
            vec![("a", None), ("b", Some("low"))],
            "low",
        ),
        (
            "unknown-and-medium",
            vec![("a", None), ("b", Some("medium"))],
            "medium",
        ),
        (
            "none-word",
            vec![("a", Some("none")), ("b", Some("medium"))],
            "medium",
        ),
        // Deduplicated by id with the final priority: three highs collapse to one.
        (
            "dedup",
            vec![
                ("a", Some("high")),
                ("a", Some("high")),
                ("a", Some("urgent")),
                ("b", Some("low")),
            ],
            "urgent",
        ),
        (
            "dedup-down",
            vec![
                ("a", Some("high")),
                ("b", Some("high")),
                ("a", Some("low")),
                ("c", Some("high")),
            ],
            "medium",
        ),
    ];
    for (name, spec, expected) in cases {
        let ctx = context(
            &world,
            name,
            &json!({"owner": owner, "dedup_key": name, "tickets": tickets(&spec)}),
        );
        let receipt = phase(&world, None, "ticket-review", &ctx);
        assert_eq!(receipt["item"]["item"]["priority"], expected, "{name}");
    }
    // Empty: no task at all.
    let empty = context(
        &world,
        "empty",
        &json!({"owner": owner, "dedup_key": "empty", "tickets": []}),
    );
    assert_eq!(phase(&world, None, "ticket-review", &empty), Value::Null);
}

#[test]
fn a_ticket_review_groups_every_link_and_one_rerun_updates_one_task() {
    let world = hooked();
    let owner = json!({"session": "mgr", "run": null, "plan": "plans:alpha"});
    let spec = vec![
        ("t-1", Some("high")),
        ("t-2", None),
        ("t-3", Some("low")),
        ("t-2", Some("none")),
        ("t-4", Some("urgent")),
    ];
    let ctx = context(
        &world,
        "review",
        &json!({"owner": owner, "dedup_key": "sprint", "tickets": tickets(&spec)}),
    );
    let first = phase(&world, None, "ticket-review", &ctx);
    let id = first["item"]["item"]["id"].as_str().unwrap().to_owned();
    let body = first["item"]["item"]["body"].as_str().unwrap().to_owned();
    for (heading, link) in [
        ("Critical", "t-4"),
        ("High", "t-1"),
        ("Low", "t-3"),
        ("No priority", "t-2"),
    ] {
        assert!(
            body.contains(&format!("\n{heading}\n")),
            "{heading}: {body}"
        );
        assert!(
            body.contains(&format!("- {link}: https://tracker/{link}")),
            "{link}: {body}"
        );
    }
    assert_eq!(body.matches("t-2:").count(), 1, "{body}");
    // The ticket with no priority raised a non-blocking finding naming it to the owner.
    let findings: Vec<Value> = world
        .json(None, &["list", "--json", "--session", "mgr"])
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["item"]["id"] != id.as_str())
        .cloned()
        .collect();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["item"]["block"]["scope"], "nothing");
    assert!(findings[0]["item"]["body"]
        .as_str()
        .unwrap()
        .contains("t-2"));

    // Rerunning the phase with changed tickets updates the one task in place.
    let spec = vec![("t-1", Some("low")), ("t-3", Some("low"))];
    let rerun = context(
        &world,
        "review2",
        &json!({"owner": owner, "dedup_key": "sprint", "tickets": tickets(&spec)}),
    );
    let second = phase(&world, None, "ticket-review", &rerun);
    assert_eq!(second["item"]["item"]["id"], id.as_str());
    assert_eq!(second["item"]["item"]["priority"], "low");
    let open = world.json(None, &["list", "--json"]);
    assert_eq!(
        open.as_array().unwrap().len(),
        1,
        "the finding retired once every ticket had a priority"
    );
    // Rerun unchanged: no new record.
    let records = world.journal().len();
    phase(&world, None, "ticket-review", &rerun);
    assert_eq!(world.journal().len(), records);
    // No tickets left: the review task is retired, never left open.
    let empty = context(
        &world,
        "review3",
        &json!({"owner": owner, "dedup_key": "sprint", "tickets": []}),
    );
    assert_eq!(phase(&world, None, "ticket-review", &empty), Value::Null);
    assert!(world
        .json(None, &["list", "--json"])
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn a_question_hook_expands_its_context_once_at_the_engine_boundary() {
    let world = hooked();
    let ctx = context(
        &world,
        "ask",
        &json!({
            "owner": {"session": "mgr", "run": null, "plan": null}, "dedup_key": "colour", "vars": {"thing": "colour"}
        }),
    );
    let raised = phase(&world, Some("mgr"), "ask", &ctx);
    assert_eq!(raised["item"]["item"]["title"], "Which colour?");
    assert_eq!(raised["item"]["item"]["body"], "Pick one colour.");
    assert_eq!(raised["item"]["item"]["kind"], "question");
    // A context missing a variable the template names does not expand, and
    // nothing is written.
    let missing = context(
        &world,
        "missing",
        &json!({
            "owner": {"session": "mgr", "run": null, "plan": null}, "dedup_key": "m"
        }),
    );
    let records = world.journal().len();
    world.refused(
        None,
        &["raise", "--phase", "ask", "--context", &missing],
        "does not expand",
    );
    assert_eq!(world.journal().len(), records);
}

/// A full disk, for real: the runs root is a 2 MiB tmpfs mounted in an
/// unprivileged user and mount namespace, filled by a file, and every command
/// is the compiled binary writing into it. Nothing is intercepted.
#[cfg(target_os = "linux")]
#[test]
fn a_full_journal_disk_raises_one_host_item_from_its_reserve_and_never_acknowledges_a_lost_write() {
    let world = Live::enabled();
    let big = "x".repeat(16 * 1024);
    let raw = |dedup: &str, session: &str| {
        let mut value = item(dedup, "task", "low", session, json!({"scope": "nothing"}));
        value["body"] = json!(format!("{dedup}: {big}"));
        world.input(dedup, &value)
    };
    let (a, b, c) = (raw("a", "s-a"), raw("b", "s-b"), raw("c", "s-c"));
    let out = world.root.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let runs = world.runs();
    let script = format!(
        r#"set -u
mount -t tmpfs -o size=2m tmpfs "{runs}" || exit 90
cd "{repo}"
run() {{ name=$1; shift; "{bin}" human-items "$@" > "{out}/$name.out" 2> "{out}/$name.err"; echo "$name=$?" >> "{out}/codes"; }}
run a raise --input "{a}"
dd if=/dev/zero of="{runs}/filler" bs=1k 2>/dev/null
run b raise --input "{b}"
run full list --json
cp "{runs}/human-items/events.ndjson" "{out}/journal-full"
dd if=/dev/zero of="{runs}/filler2" bs=1k 2>/dev/null
run c raise --input "{c}"
cp "{runs}/human-items/events.ndjson" "{out}/journal-refused"
rm "{runs}/filler" "{runs}/filler2"
run after list --json --state dismissed
ONEPIPELINE_LAUNCHER_SESSION=s-b run watch watch
cp "{runs}/human-items/events.ndjson" "{out}/journal-after"
"#,
        runs = runs.display(),
        repo = world.repo().display(),
        bin = BIN,
        out = out.display(),
    );
    let status = Command::new("unshare")
        .args(["-rm", "sh", "-c", &script])
        .env("ONEPIPELINE_RUNS_DIR", &runs)
        .env("XDG_CONFIG_HOME", world.root.join("xdg"))
        .env("USER", "operator")
        .env_remove("ONEPIPELINE_LAUNCHER_SESSION")
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the namespace could not be set up: {status}"
    );
    let read = |name: &str| std::fs::read_to_string(out.join(name)).unwrap_or_default();
    let codes = read("codes");
    assert!(codes.contains("a=0\n"), "{codes}");
    assert!(
        codes.contains("b=0\n"),
        "the write the reserve took is acknowledged: {codes}\n{}",
        read("b.err")
    );
    assert!(codes.contains("full=0\n"), "{codes}\n{}", read("full.err"));
    assert!(
        !codes.contains("c=0\n"),
        "a write nothing could hold is refused: {codes}"
    );
    assert!(
        read("c.err").contains("could not record this durably"),
        "{}",
        read("c.err")
    );
    assert!(
        read("c.out").is_empty(),
        "no receipt for a lost write: {}",
        read("c.out")
    );
    let journal = |name: &str| -> Vec<Value> {
        read(name)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    let refused = journal("journal-refused");
    assert!(refused
        .iter()
        .all(|e| e["payload"]["item"]["dedup_key"] != "c"));
    // Exactly one urgent host item for the condition, owned by the session whose
    // write found the disk full, and it blocks the whole host.
    let full: Value = serde_json::from_str(&read("full.out")).unwrap();
    let hosts: Vec<&Value> = full
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["item"]["block"]["scope"] == "host")
        .collect();
    assert_eq!(hosts.len(), 1, "{full}");
    assert_eq!(hosts[0]["item"]["priority"], "urgent");
    assert_eq!(hosts[0]["item"]["owner"]["session"], "s-b");
    let host_id = hosts[0]["item"]["id"].clone();
    assert_eq!(
        journal("journal-full")
            .iter()
            .filter(|e| e["payload"]["item"]["block"]["scope"] == "host")
            .count(),
        1
    );
    // Space returned: the next command's probe of the journal's own reserve
    // resolved the item automatically, and the owner's watch was told.
    let after: Value = serde_json::from_str(&read("after.out")).unwrap();
    let resolved = after
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["item"]["id"] == host_id)
        .expect("resolved");
    assert_eq!(resolved["item"]["resolution"]["actor"], "engine");
    let kinds: Vec<String> = read("watch.out")
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "human-item-raised",
            "human-item-raised",
            "human-item-auto-resolved"
        ],
        "{}",
        read("watch.err")
    );
    assert_eq!(
        journal("journal-after")
            .iter()
            .filter(|e| e["kind"] == "human-item-auto-resolved")
            .count(),
        1
    );
}

/// An authority that cannot be started, or answers anything but the version it
/// holds for the stored board copy, leaves the task open and records nothing.
#[test]
fn an_authority_that_cannot_answer_properly_leaves_the_task_open() {
    let world = hooked();
    let owner = json!({"session": "mgr", "run": null, "plan": "plans:alpha"});
    let adapter = world.root.join("adapter.sh");
    let raise = |name: &str| {
        let ctx = context(
            &world,
            name,
            &json!({
                "owner": owner, "dedup_key": name, "approval": {"board_copy": "plans:doc-1", "key": "v1"}
            }),
        );
        phase(&world, None, "design-review", &ctx)["item"]["item"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    for (name, script, refusal) in [
        (
            "garbled",
            "cat > /dev/null; echo 'not json'",
            "answered something other than",
        ),
        (
            "elsewhere",
            "cat > /dev/null; printf '{\"board_copy\":\"plans:doc-9\",\"key\":\"v1\"}'",
            "answered for board copy plans:doc-9",
        ),
        (
            "blank",
            "cat > /dev/null; printf '{\"board_copy\":\"plans:doc-1\",\"key\":\" \"}'",
            "blank document version",
        ),
    ] {
        std::fs::write(&adapter, script).unwrap();
        let id = raise(name);
        world.refused(None, &["complete", &id], refusal);
        assert_eq!(
            world.json(None, &["show", &id, "--json"])["item"]["state"],
            "open",
            "{name}"
        );
    }
    std::fs::remove_file(&adapter).unwrap();
    let configured =
        std::fs::read_to_string(world.repo().join(".onepipeline/config.yaml")).unwrap();
    world.configure(&configured.replace("[\"sh\", ", "[\"/nonexistent/approve-design\", "));
    let id = raise("unstartable");
    world.refused(None, &["complete", &id], "could not be started");
    assert_eq!(
        world.json(None, &["show", &id, "--json"])["item"]["state"],
        "open"
    );
    assert!(lines(&world.root.join("approvals.ndjson")).is_empty());
}

/// A watch given a timeout waits for its owner's next event, and returns with
/// exit 5 when none arrives in time.
#[test]
fn a_waiting_watch_receives_an_arriving_event_or_runs_out() {
    let world = Live::enabled();
    let began = std::time::Instant::now();
    let output = world.run(Some("waiter"), &["watch", "--timeout", "1"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(began.elapsed() >= std::time::Duration::from_millis(900));
    let waiting = world
        .cmd(Some("waiter"), &["watch", "--timeout", "30"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    world.raise(&item(
        "arrives",
        "question",
        "low",
        "waiter",
        json!({"scope": "nothing"}),
    ));
    let output = waiting.wait_with_output().unwrap();
    assert!(output.status.success());
    let event: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(event["kind"], "human-item-raised");
    assert_eq!(event["payload"]["owner"]["session"], "waiter");
}

#[test]
fn every_malformed_hook_configuration_is_refused_at_load_by_its_file() {
    let hook = |extra: &str| {
        format!(
            "human_items:\n  enabled: true\n{extra}  hooks:\n    - phase: review\n      kind: task\n      \
             title: t\n      body: b\n      priority: low\n      block: {{scope: nothing}}\n"
        )
    };
    for (config, refusal) in [
        ("human_items:\n  enabled: true\n  task_source: '  '\n".to_owned(), "task_source must not be blank"),
        (hook("").replace("hooks:\n", "hooks:\n    - phase: review\n      kind: task\n      title: t\n      body: b\n      priority: low\n      block: {scope: nothing}\n"), "configured twice"),
        (hook("").replace("phase: review", "phase: 're:view'"), "without ':'"),
        (hook("").replace("title: t", "title: '{{ unclosed'"), "hooks[review]"),
        (hook("").replace("kind: task", "kind: question").replace("block: {scope: nothing}", "block: {scope: nothing}\n      aggregate: ticket_review"), "a question carries no approval"),
        (hook("").replace("block: {scope: nothing}", "block: {scope: nothing}\n      approval: {adapter: missing}"), "is not configured under human_items.approval_adapters"),
        (hook("  approval_adapters:\n    empty: {command: []}\n"), "command names a program"),
        (hook("").replace("priority: low", "priority: none"), "states its priority"),
    ] {
        let world = Live::new(Some(&config));
        world.refused(None, &["config"], refusal);
        world.refused(None, &["list"], refusal);
    }
}

#[test]
fn a_phase_context_supplies_what_its_hook_holds_and_no_engine_key_is_raised_directly() {
    let world = Live::new(Some(
        "human_items:\n  enabled: true\n  hooks:\n    - phase: gate\n      kind: task\n      title: Gate {{ node }}\n      \
         body: Hold {{ node }}.\n      priority: high\n      block: {scope: work}\n    - phase: freeze\n      kind: task\n      \
         title: Freeze\n      body: Hold the plan.\n      priority: high\n      block: {scope: plan}\n",
    ));
    let session = json!({"session": "mgr", "run": null, "plan": null});
    let no_run = world.input(
        "no-run",
        &json!({"owner": session, "dedup_key": "a", "node": "build"}),
    );
    world.refused(
        None,
        &["raise", "--phase", "gate", "--context", &no_run],
        "needs the owner's run",
    );
    let no_node = world.input(
        "no-node",
        &json!({"owner": {"session": "mgr", "run": "r", "plan": null}, "dedup_key": "a"}),
    );
    world.refused(
        None,
        &["raise", "--phase", "gate", "--context", &no_node],
        "needs the context's node",
    );
    let no_plan = world.input("no-plan", &json!({"owner": session, "dedup_key": "a"}));
    world.refused(
        None,
        &["raise", "--phase", "freeze", "--context", &no_plan],
        "needs the owner's plan",
    );
    let stray = world.input("stray", &json!({"owner": {"session": "mgr", "run": null, "plan": "p"}, "dedup_key": "a", "approval": {"board_copy": "b", "key": "k"}}));
    world.refused(
        None,
        &["raise", "--phase", "freeze", "--context", &stray],
        "configures no approval adapter",
    );
    let held = world.input("held", &json!({"owner": {"session": "mgr", "run": "r", "plan": "p"}, "dedup_key": "a", "node": "build"}));
    let raised = world.json(None, &["raise", "--phase", "gate", "--context", &held]);
    assert_eq!(
        raised["item"]["item"]["block"],
        json!({"scope": "work", "run": "r", "node": "build"})
    );
    assert_eq!(raised["item"]["item"]["title"], "Gate build");
    // The engine's own keys are not a caller's to mint.
    for key in ["run:r#build", "phase:gate:a", "host:journal-no-space:0"] {
        let path = world.input(
            "engine-key",
            &item(key, "task", "low", "mgr", json!({"scope": "nothing"})),
        );
        world.refused(None, &["raise", "--input", &path], "are the engine's own");
    }
    assert_eq!(world.journal().len(), 1);
}

#[cfg(unix)]
#[test]
fn delivery_survives_a_failed_carry_a_truncated_journal_and_a_failed_print() {
    use std::os::unix::fs::PermissionsExt;
    let world = Live::enabled();
    world.raise(&item(
        "one",
        "task",
        "low",
        "owner",
        json!({"scope": "nothing"}),
    ));
    let bus = world.store().join("bus");
    // The bus refuses the write: the event is recorded, said to be undelivered,
    // and carried by the next command.
    std::fs::set_permissions(&bus, std::fs::Permissions::from_mode(0o500)).unwrap();
    let path = world.input(
        "two",
        &item("two", "task", "low", "owner", json!({"scope": "nothing"})),
    );
    let output = world.run(None, &["raise", "--input", &path]);
    std::fs::set_permissions(&bus, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not yet delivered"));
    // A print that fails commits nothing: the next watch prints the same events.
    let full = world
        .cmd(Some("owner"), &["watch"])
        .stdout(
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        )
        .output()
        .unwrap();
    assert!(!full.status.success());
    let events = world.watch("owner");
    assert_eq!(events.len(), 2, "{events:?}");
    // A journal cut back under delivery is carried again from its start, and the
    // watch prints nothing it already printed.
    let journal = world.store().join("events.ndjson");
    let bytes = std::fs::read(&journal).unwrap();
    let first = bytes.iter().position(|b| *b == b'\n').unwrap() + 1;
    std::fs::write(&journal, &bytes[..first]).unwrap();
    assert!(world.watch("owner").is_empty());
    std::fs::write(&journal, &bytes).unwrap();
    world.raise(&item(
        "three",
        "task",
        "low",
        "owner",
        json!({"scope": "nothing"}),
    ));
    let after = world.watch("owner");
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(after[0]["payload"]["item"]["dedup_key"], "three");
}
