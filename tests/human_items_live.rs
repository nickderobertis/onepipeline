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

/// The live verbs `docs/contract.md` declares: every verb of its human-items CLI
/// block, which without `--sample` acts on the live store, and `watch`. `config` is
/// not one of them: the contract's Configuration paragraph makes it the reader of
/// the effective settings, so it answers `enabled: false` rather than refusing.
fn contract_live_verbs() -> Vec<String> {
    let contract =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract.md"))
            .unwrap();
    let mut verbs: Vec<String> = contract
        .lines()
        .filter_map(|line| line.strip_prefix("onepipeline human-items --sample FILE "))
        .map(|rest| rest.split_whitespace().next().unwrap().to_owned())
        .collect();
    assert!(
        contract.contains("`onepipeline human-items watch"),
        "the contract no longer declares watch"
    );
    verbs.push("watch".to_owned());
    verbs
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
        let driven = [
            vec!["list"],
            vec!["report"],
            vec!["show", "hi-x"],
            vec!["raise", "--input", input.as_str()],
            vec!["answer", "hi-x", "--text", "yes"],
            vec!["complete", "hi-x"],
            vec!["dismiss", "hi-x"],
            vec!["watch", "--session", "s"],
        ];
        // Exactly the contract's live verbs, so a verb it adds is driven here too.
        let mut names: Vec<String> = driven.iter().map(|args| args[0].to_owned()).collect();
        let mut declared = contract_live_verbs();
        names.sort();
        declared.sort();
        assert_eq!(names, declared);
        for args in driven {
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

/// By default each item is one compact entry — title and id, then priority,
/// kind, what it blocks and its source, then one line of its body, cut with an
/// ellipsis when the body is longer — and `--full` prints every complete body.
#[test]
fn list_and_report_print_compact_entries_unless_asked_for_full_bodies() {
    let world = Live::enabled();
    let mut short = item(
        "short",
        "question",
        "low",
        "mgr",
        json!({"scope":"nothing"}),
    );
    short["body"] = json!("Pick a name.");
    world.raise(&short);
    let long_line = format!(
        "Restart the build service {}and then confirm.",
        "after the outage ".repeat(8)
    );
    let mut long = item("long", "task", "high", "mgr", json!({"scope":"host"}));
    long["body"] = json!(long_line);
    world.raise(&long);
    let mut lines = item("lines", "task", "medium", "mgr", json!({"scope":"nothing"}));
    lines["owner"]["plan"] = json!("plans:alpha");
    lines["body"] = json!("Read the design.\nThen confirm it in chat.\nThird line.");
    world.raise(&lines);
    // The boundaries: a body exactly as long as a description, the same line with
    // more after it, and multibyte text longer than a description.
    let exact_body = "x".repeat(80);
    for (dedup, body) in [
        ("exact", exact_body.clone()),
        ("then", format!("{exact_body}\nmore")),
        ("wide", "é".repeat(85)),
    ] {
        let mut boundary = item(dedup, "question", "low", "mgr", json!({"scope":"nothing"}));
        boundary["body"] = json!(body);
        world.raise(&boundary);
    }

    for verb in ["list", "report"] {
        let text = world.ok(Some("mgr"), &[verb]);
        let entry = |title: &str| -> Vec<String> {
            let lines: Vec<&str> = text.lines().collect();
            let at = lines
                .iter()
                .position(|l| l.starts_with(&format!("- {title} (hi-")))
                .unwrap_or_else(|| panic!("{verb} has no entry {title}: {text}"));
            lines[at..at + 3].iter().map(|l| (*l).to_owned()).collect()
        };
        let short = entry("question short");
        assert_eq!(
            short[1], "  Low question · blocks nothing · session mgr",
            "{text}"
        );
        assert_eq!(short[2], "  Pick a name.", "{text}");
        let long = entry("task long");
        assert_eq!(
            long[1], "  High task · blocks all work on this host · host",
            "{text}"
        );
        assert!(long[2].ends_with('…'), "{text}");
        assert!(long[2].chars().count() <= 2 + 80, "{text}");
        assert!(
            long_line.starts_with(long[2].trim().trim_end_matches('…')),
            "{text}"
        );
        let lines = entry("task lines");
        assert_eq!(
            lines[1], "  Medium task · blocks nothing · plan plans:alpha",
            "{text}"
        );
        assert_eq!(lines[2], "  Read the design.…", "{text}");
        assert_eq!(
            entry("question exact")[2],
            format!("  {exact_body}"),
            "{text}"
        );
        assert_eq!(
            entry("question then")[2],
            format!("  {}…", "x".repeat(79)),
            "{text}"
        );
        assert_eq!(
            entry("question wide")[2],
            format!("  {}…", "é".repeat(79)),
            "{text}"
        );
        for cut in [
            "and then confirm.",
            "Then confirm it in chat.",
            "Third line.",
        ] {
            assert!(!text.contains(cut), "{verb} printed past one line: {text}");
        }

        let full = world.ok(Some("mgr"), &[verb, "--full"]);
        for whole in [
            "Pick a name.",
            long_line.as_str(),
            "Read the design.\nThen confirm it in chat.\nThird line.",
        ] {
            assert!(full.contains(whole), "{verb} --full lacks {whole}: {full}");
        }
        assert!(!full.contains('…'), "{full}");
    }
    let report = world.ok(Some("mgr"), &["report"]);
    assert!(
        report.starts_with("Human items: 6 listed, 1 blocking\n"),
        "{report}"
    );
    let blocking = report.find("Blocking (1)").unwrap();
    let rest = report.find("Not blocking (5)").unwrap();
    assert!(blocking < report.find("- task long").unwrap());
    assert!(report.find("- task long").unwrap() < rest);

    // A closed item's entry names its state after its source.
    let short = world.json(None, &["list", "--json", "--kind", "question"]);
    let short = short
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["item"]["dedup_key"] == "short")
        .unwrap()["item"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    world.ok(Some("mgr"), &["answer", &short, "--text", "Widgets."]);
    for verb in ["list", "report"] {
        let text = world.ok(Some("mgr"), &[verb, "--state", "answered"]);
        assert!(
            text.contains("  Low question · blocks nothing · session mgr · answered\n"),
            "{verb}: {text}"
        );
        assert!(!text.contains("question exact"), "{verb}: {text}");
    }
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
      command: ["sh", ADAPTER]
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
    let adapter = adapter(&world);
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
            dir = sh_quoted(&world.root)
        ),
    )
    .unwrap();
    std::fs::write(world.root.join("current-key"), "v1").unwrap();
    world.configure(&HOOKS.replace("ADAPTER", &yaml_quoted(&adapter)));
    world
}

/// The approval command [`hooked`] configures.
fn adapter(world: &Live) -> PathBuf {
    windows_shaped(&world.root).join("adapter.sh")
}

/// A directory shaped like a Windows temporary path — drive letter, backslashes
/// and an 8.3 short-name component — on every platform. On Windows it is a real
/// `RUNNER~1\AppData` under a root that is already a drive-letter path; elsewhere
/// the same spelling is one component, whose backslashes are ordinary filename
/// bytes, so nothing between the configuration and the command may reinterpret
/// them either.
fn windows_shaped(root: &Path) -> PathBuf {
    let dir = if cfg!(windows) {
        root.join("RUNNER~1").join("AppData")
    } else {
        root.join(r"C:\Users\RUNNER~1\AppData")
    };
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `path` as one single-quoted `sh` word: nothing in it is an escape.
fn sh_quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// `path` as a YAML double-quoted scalar, every backslash escaped.
fn yaml_quoted(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).unwrap()
}

/// A phase context file, written under [`windows_shaped`] so every phase raise
/// reads its context from a drive-letter, backslashed, short-name path.
fn context(world: &Live, name: &str, value: &Value) -> String {
    let path = windows_shaped(&world.root).join(format!("{name}.json"));
    std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
    let path = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        assert!(path.as_bytes()[1] == b':' && path.contains('\\'), "{path}");
    } else {
        assert!(path.contains(r"C:\Users\RUNNER~1\AppData"), "{path}");
    }
    path
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
            body.contains(&format!(
                "- [https://tracker/{link}](https://tracker/{link})\n"
            )),
            "{link}: {body}"
        );
    }
    assert_eq!(body.matches("(https://tracker/t-2)").count(), 1, "{body}");
    assert!(!body.contains("t-2:"), "no board id is printed: {body}");
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

    // What raised the review and its finding reaches the owner's watch and is read now.
    assert!(!world.watch("mgr").is_empty());

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
    // The engine retired the finding as its own resolution, with its reason, to the owner alone.
    let finding = findings[0]["item"]["id"].as_str().unwrap();
    let retired = |events: &[Value], id: &str, reason: &str| {
        let matching: Vec<&Value> = events
            .iter()
            .filter(|e| e["kind"] == "human-item-auto-resolved" && e["payload"]["item_id"] == id)
            .collect();
        assert_eq!(matching.len(), 1, "{id}: {events:?}");
        let resolution = &matching[0]["payload"]["resolution"];
        assert_eq!(resolution["actor"], "engine", "{resolution}");
        assert_eq!(resolution["on_behalf_of"], Value::Null, "{resolution}");
        assert_eq!(resolution["reason"], reason, "{resolution}");
    };
    retired(
        &world.watch("mgr"),
        finding,
        "every review ticket has a priority",
    );
    let shown = world.json(None, &["show", finding, "--json"]);
    assert_eq!(shown["item"]["state"], "dismissed");
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
    retired(&world.watch("mgr"), &id, "no tickets remain to review");
    let shown = world.json(None, &["show", &id, "--json"]);
    assert_eq!(
        shown["item"]["resolution"]["reason"],
        "no tickets remain to review"
    );
    // No other session's watch hears of either retirement.
    assert_eq!(world.run(Some("other"), &["watch"]).status.code(), Some(5));
}

/// A review names each ticket as a markdown link: by its title when the context
/// gives one, and otherwise by its short repository reference — never by its
/// board id. A blank title is refused before anything is written.
#[test]
fn a_ticket_review_links_each_ticket_by_its_title_or_repository_reference() {
    let world = hooked();
    let owner = json!({"session": "mgr", "run": null, "plan": "plans:alpha"});
    let ctx = context(
        &world,
        "titled",
        &json!({"owner": owner, "dedup_key": "titled", "tickets": [
            {"id": "board-1", "url": "https://github.com/acme/widgets/issues/12",
             "priority": "high", "title": "Fix the [flaky] login test"},
            {"id": "board-2", "url": "https://github.com/acme/widgets/pull/40", "priority": "low"},
            {"id": "board-3", "url": "https://gitlab.example/group/gadgets/-/merge_requests/3"},
            {"id": "board-4", "url": "https://github.com/acme/widgets/issues/77/?tab=1#top",
             "priority": "low"},
            {"id": "board-5", "url": "https://tracker/notes (draft)", "priority": "low"},
        ]}),
    );
    let receipt = phase(&world, None, "ticket-review", &ctx);
    let body = receipt["item"]["item"]["body"].as_str().unwrap().to_owned();
    assert!(
        body.contains(
            "\nHigh\n- [Fix the \\[flaky\\] login test](https://github.com/acme/widgets/issues/12)\n"
        ),
        "{body}"
    );
    // A trailing slash, query and fragment do not hide the number; a URL naming
    // no repository and number is its own text, bracketed where it has spaces.
    assert!(
        body.contains(
            "\nLow\n- [widgets#40](https://github.com/acme/widgets/pull/40)\n\
             - [widgets#77](https://github.com/acme/widgets/issues/77/?tab=1#top)\n\
             - [https://tracker/notes (draft)](<https://tracker/notes (draft)>)\n"
        ),
        "{body}"
    );
    assert!(
        body.contains(
            "\nNo priority\n- [gadgets#3](https://gitlab.example/group/gadgets/-/merge_requests/3)\n"
        ),
        "{body}"
    );
    assert!(!body.contains("board-"), "no board id is printed: {body}");
    // The finding naming the ticket with no priority links it the same way.
    let finding = world
        .json(None, &["list", "--json", "--session", "mgr"])
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["item"]["id"] != receipt["item"]["item"]["id"])
        .cloned()
        .unwrap();
    let text = finding["item"]["body"].as_str().unwrap();
    assert!(
        text.contains("- [gadgets#3](https://gitlab.example/group/gadgets/-/merge_requests/3)"),
        "{text}"
    );
    assert!(!text.contains("board-3"), "{text}");

    let records = world.journal().len();
    let blank = context(
        &world,
        "blank-title",
        &json!({"owner": owner, "dedup_key": "blank", "tickets": [
            {"id": "a", "url": "https://github.com/acme/widgets/issues/1", "title": "  "},
        ]}),
    );
    world.refused(
        None,
        &["raise", "--phase", "ticket-review", "--context", &blank],
        "title, when given, is not blank",
    );
    assert_eq!(world.journal().len(), records);
}

/// A template that renders without error to a blank title or body is still an
/// invalid item: the raise refuses it and writes nothing.
#[test]
fn a_hook_rendering_to_a_blank_title_or_body_is_refused_without_writing() {
    let world = Live::new(Some(
        "human_items:\n  enabled: true\n  hooks:\n    - phase: titled\n      kind: task\n      \
         title: '{{ text }}'\n      body: Body.\n      priority: low\n      block: {scope: nothing}\n    \
         - phase: bodied\n      kind: question\n      title: Title\n      body: '{{ text }}'\n      \
         priority: low\n      block: {scope: nothing}\n",
    ));
    let owner = json!({"session": "mgr", "run": null, "plan": null});
    let blank = world.input(
        "blank",
        &json!({"owner": owner, "dedup_key": "a", "vars": {"text": "   "}}),
    );
    for (phase, field) in [("titled", "title"), ("bodied", "body")] {
        let stderr = world.refused(
            None,
            &["raise", "--phase", phase, "--context", &blank],
            &format!("phase {phase} expands to an invalid item"),
        );
        assert!(stderr.contains(field), "{field}: {stderr}");
    }
    assert!(world.journal().is_empty());
    assert_eq!(world.run(Some("mgr"), &["watch"]).status.code(), Some(5));
    // The same hook with text renders a valid item, so the refusal was the blank.
    let text = world.input(
        "text",
        &json!({"owner": owner, "dedup_key": "a", "vars": {"text": "Ready"}}),
    );
    let raised = world.json(None, &["raise", "--phase", "titled", "--context", &text]);
    assert_eq!(raised["item"]["item"]["title"], "Ready");
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
    let adapter = adapter(&world);
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
    // Its stdout is a pipe nobody reads, closed before the watch starts, so every
    // write fails on each Unix (macOS has no `/dev/full`).
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let full = world
        .cmd(Some("owner"), &["watch"])
        .stdout(writer)
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

/// A watch whose output fails after it has printed some events records those as
/// watched: the next watch prints only what the failed one did not.
#[cfg(target_os = "linux")]
#[test]
fn a_watch_that_fails_partway_prints_the_rest_and_nothing_twice() {
    use std::os::unix::process::CommandExt;
    // Long bodies, so each printed line is larger than anything else the watch
    // writes and the file-size ceiling below falls between the two lines.
    let raise_two = |world: &Live| {
        for dedup in ["first", "second"] {
            let mut raised = item(dedup, "task", "low", "owner", json!({"scope": "nothing"}));
            raised["body"] = json!(format!("{dedup} {}", "x".repeat(4000)));
            world.raise(&raised);
        }
    };
    // A twin world measures the first printed line.
    let twin = Live::enabled();
    raise_two(&twin);
    let output = twin.run(Some("owner"), &["watch"]);
    let text = String::from_utf8(output.stdout).unwrap();
    let first = text.lines().next().unwrap().len() as u64 + 1;
    assert_eq!(text.lines().count(), 2, "{text}");

    let world = Live::enabled();
    raise_two(&world);
    let printed = world.root.join("printed.ndjson");
    let ceiling = first + 64;
    let mut command = world.cmd(Some("owner"), &["watch"]);
    command
        .env("LLVM_PROFILE_FILE", world.root.join("watch-%p.profraw"))
        .stdout(std::fs::File::create(&printed).unwrap());
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            let limit = libc::rlimit {
                rlim_cur: ceiling,
                rlim_max: ceiling,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::signal(libc::SIGXFSZ, libc::SIG_IGN) == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let failed = command.output().unwrap();
    assert!(!failed.status.success());
    let shown: Vec<Value> = std::fs::read_to_string(&printed)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let keys = |events: &[Value]| -> Vec<String> {
        events
            .iter()
            .map(|e| e["payload"]["item"]["dedup_key"].to_string())
            .collect()
    };
    assert_eq!(shown.len(), 1, "{:?}", keys(&shown));
    assert_eq!(shown[0]["payload"]["item"]["dedup_key"], "first");
    let rest = world.watch("owner");
    assert_eq!(rest.len(), 1, "{:?}", keys(&rest));
    assert_eq!(rest[0]["payload"]["item"]["dedup_key"], "second");
    assert!(world.watch("owner").is_empty());
}

/// Watches of one session running at once print every event exactly once
/// between them.
#[test]
fn concurrent_watches_of_one_session_print_each_event_once() {
    let world = Live::enabled();
    for n in 0..10 {
        world.raise(&item(
            &format!("w-{n}"),
            "question",
            "low",
            "shared",
            json!({"scope": "nothing"}),
        ));
    }
    let watches: Vec<_> = (0..4)
        .map(|_| {
            world
                .cmd(Some("shared"), &["watch"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut seqs: Vec<u64> = watches
        .into_iter()
        .flat_map(|w| {
            let out = w.wait_with_output().unwrap();
            String::from_utf8(out.stdout)
                .unwrap()
                .lines()
                .map(|l| {
                    serde_json::from_str::<Value>(l).unwrap()["seq"]
                        .as_u64()
                        .unwrap()
                })
                .collect::<Vec<_>>()
        })
        .collect();
    seqs.sort_unstable();
    assert_eq!(seqs, (0..10).collect::<Vec<_>>());
}

/// A design task whose phase, or whose adapter, is no longer configured cannot
/// be completed through an authority nothing names: it refuses and stays open.
#[test]
fn a_design_task_whose_authority_is_no_longer_configured_stays_open() {
    let world = hooked();
    let owner = json!({"session": "mgr", "run": null, "plan": "plans:alpha"});
    let ctx = context(
        &world,
        "design",
        &json!({
            "owner": owner, "dedup_key": "alpha", "approval": {"board_copy": "plans:doc-1", "key": "v1"}
        }),
    );
    let id = phase(&world, None, "design-review", &ctx)["item"]["item"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    world.configure("human_items:\n  enabled: true\n");
    world.refused(
        None,
        &["complete", &id],
        "no configured phase names an approval adapter",
    );
    assert_eq!(
        world.json(None, &["show", &id, "--json"])["item"]["state"],
        "open"
    );
    assert!(lines(&world.root.join("requests.ndjson")).is_empty());
}

/// A raise is refused, writing nothing, when its dedup key already names an
/// item of another kind, owner or block, or when an identity it states is blank.
#[test]
fn raises_refuse_conflicting_keys_and_blank_identities_without_writing() {
    let world = Live::enabled();
    let id = world.raise(&item(
        "held",
        "task",
        "low",
        "mgr",
        json!({"scope": "nothing"}),
    ));
    let before = world.json(None, &["show", &id, "--json"]);
    for (name, value, refusal) in [
        (
            "kind",
            item(
                "held",
                "question",
                "low",
                "mgr",
                json!({"scope": "nothing"}),
            ),
            "another kind, owner or block",
        ),
        (
            "owner",
            item("held", "task", "low", "other", json!({"scope": "nothing"})),
            "another kind, owner or block",
        ),
        (
            "block",
            item("held", "task", "low", "mgr", json!({"scope": "host"})),
            "another kind, owner or block",
        ),
        (
            "dedup",
            item(" ", "task", "low", "mgr", json!({"scope": "nothing"})),
            "dedup_key must not be blank",
        ),
        (
            "session",
            item("s", "task", "low", " ", json!({"scope": "nothing"})),
            "owner.session must not be blank",
        ),
        (
            "work",
            item(
                "w",
                "task",
                "low",
                "mgr",
                json!({"scope": "work", "run": " ", "node": "n"}),
            ),
            "names its run and node",
        ),
        (
            "plan",
            item(
                "p",
                "task",
                "low",
                "mgr",
                json!({"scope": "plan", "plan": " "}),
            ),
            "names its plan",
        ),
    ] {
        let path = world.input(name, &value);
        world.refused(None, &["raise", "--input", &path], refusal);
    }
    assert_eq!(world.journal().len(), 1);
    assert_eq!(world.json(None, &["show", &id, "--json"]), before);
}

/// The shared default is the nearest `.onepipeline/config.yaml` at or above the
/// working directory, never past the repository root; the host's own override is
/// `ONEPIPELINE_CONFIG`, else `~/.config` when no `XDG_CONFIG_HOME` is set, and an
/// override naming no file is refused.
#[test]
fn configuration_is_found_where_the_checkout_and_the_host_keep_it() {
    let world = Live::new(Some(
        "human_items:\n  enabled: true\n  task_source: shared\n",
    ));
    let deep = world.repo().join("crates/inner");
    std::fs::create_dir_all(&deep).unwrap();
    let in_dir = |dir: &Path, envs: &[(&str, Option<&Path>)]| {
        let mut command = world.cmd(None, &["config", "--json"]);
        command.current_dir(dir);
        for (key, value) in envs {
            match value {
                Some(value) => command.env(key, value),
                None => command.env_remove(key),
            };
        }
        command.output().unwrap()
    };
    let read: Value = serde_json::from_slice(&in_dir(&deep, &[]).stdout).unwrap();
    assert_eq!(read["human_items"]["task_source"], "shared");
    // A checkout nested inside another stops at its own root.
    let nested = world.repo().join("vendor/other");
    std::fs::create_dir_all(nested.join(".git")).unwrap();
    let read: Value = serde_json::from_slice(&in_dir(&nested, &[]).stdout).unwrap();
    assert_eq!(read["human_items"]["enabled"], false);
    assert!(read["sources"].as_array().unwrap().is_empty());
    // `~/.config` when no XDG_CONFIG_HOME is set.
    let home = world.root.join("home");
    std::fs::create_dir_all(home.join(".config/onepipeline")).unwrap();
    std::fs::write(
        home.join(".config/onepipeline/config.yaml"),
        "human_items:\n  task_source: hosted\n",
    )
    .unwrap();
    let read: Value = serde_json::from_slice(
        &in_dir(&deep, &[("XDG_CONFIG_HOME", None), ("HOME", Some(&home))]).stdout,
    )
    .unwrap();
    assert_eq!(read["human_items"]["task_source"], "hosted");
    assert_eq!(read["human_items"]["enabled"], true);
    // An override naming nothing is refused, not ignored.
    let missing = world.root.join("missing.yaml");
    let refused = in_dir(&deep, &[("ONEPIPELINE_CONFIG", Some(&missing))]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("which is not a file"));
}

/// Without a launcher session the actor is the login user, from the first of
/// USER, USERNAME and LOGNAME that is set and not blank, else `operator`.
#[test]
fn a_direct_actor_is_the_first_login_variable_set() {
    let world = Live::enabled();
    for (n, (user, username, logname, expected)) in [
        (Some("alice"), Some("bob"), Some("carol"), "user:alice"),
        (Some(" "), Some("bob"), Some("carol"), "user:bob"),
        (None, None, Some("carol"), "user:carol"),
        (None, None, None, "user:operator"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = world.raise(&item(
            &format!("a-{n}"),
            "task",
            "low",
            "s",
            json!({"scope": "nothing"}),
        ));
        let mut command = world.cmd(None, &["complete", &id]);
        command
            .env_remove("USER")
            .env_remove("USERNAME")
            .env_remove("LOGNAME");
        for (key, value) in [("USER", user), ("USERNAME", username), ("LOGNAME", logname)] {
            if let Some(value) = value {
                command.env(key, value);
            }
        }
        let receipt: Value = serde_json::from_slice(&command.output().unwrap().stdout).unwrap();
        assert_eq!(receipt["item"]["item"]["resolution"]["actor"], expected);
    }
}

#[test]
fn run_and_plan_selectors_choose_by_the_items_owner() {
    let world = Live::enabled();
    let owned = |dedup: &str, run: Option<&str>, plan: Option<&str>| {
        let mut value = item(dedup, "task", "low", "s", json!({"scope": "nothing"}));
        value["owner"] = json!({"session": "s", "run": run, "plan": plan});
        world.raise(&value)
    };
    let a = owned("a", Some("run-a"), Some("plan-a"));
    let b = owned("b", Some("run-b"), Some("plan-a"));
    owned("c", None, None);
    assert_eq!(
        ids(&world.json(None, &["list", "--json", "--run", "run-a"])),
        vec![a.clone()]
    );
    let mut by_plan = ids(&world.json(None, &["list", "--json", "--plan", "plan-a"]));
    by_plan.sort();
    let mut expected = vec![a.clone(), b];
    expected.sort();
    assert_eq!(by_plan, expected);
    assert_eq!(
        ids(&world.json(
            None,
            &["list", "--json", "--plan", "plan-a", "--run", "run-a"]
        )),
        vec![a]
    );
}

#[test]
fn a_host_hook_raises_a_host_hold_and_sample_mode_takes_no_live_verb() {
    let world = Live::new(Some(
        "human_items:\n  enabled: true\n  hooks:\n    - phase: outage\n      kind: task\n      title: Restore it\n      \
         body: The service is down.\n      priority: urgent\n      block: {scope: host}\n",
    ));
    let ctx = world.input(
        "ctx",
        &json!({"owner": {"session": "mgr", "run": null, "plan": null}, "dedup_key": "x"}),
    );
    let raised = world.json(None, &["raise", "--phase", "outage", "--context", &ctx]);
    assert_eq!(raised["item"]["item"]["block"], json!({"scope": "host"}));
    let sample = world.root.join("sample.json");
    for args in [
        vec![
            "--sample",
            sample.to_str().unwrap(),
            "watch",
            "--session",
            "s",
        ],
        vec!["--sample", sample.to_str().unwrap(), "config"],
        vec![
            "--sample",
            sample.to_str().unwrap(),
            "raise",
            "--phase",
            "outage",
            "--context",
            ctx.as_str(),
        ],
    ] {
        std::fs::write(&sample, include_str!("fixtures/human-items.json")).unwrap();
        world.refused(None, &args, "no --sample");
    }
    // A watch names whose events it reads.
    world.refused(None, &["watch"], "no session to watch");
    world.refused(None, &["watch", "--session", " "], "no session to watch");
}

/// A whole journal record that is not one this build reads as an item event —
/// on another stream, at an envelope version it does not read, with a kind its
/// payload's action contradicts, or numbered past the last a journal can number
/// — is refused out loud, never folded or skipped.
#[test]
fn records_no_writer_made_are_refused_by_what_is_wrong_with_them() {
    let world = Live::enabled();
    world.raise(&item(
        "base",
        "task",
        "low",
        "s",
        json!({"scope": "nothing"}),
    ));
    let journal = world.store().join("events.ndjson");
    let good = std::fs::read_to_string(&journal).unwrap();
    let record: Value = serde_json::from_str(good.lines().next().unwrap()).unwrap();
    for (name, edit, refusal) in [
        ("stream", json!({"stream": "elsewhere"}), "on stream"),
        ("version", json!({"v": 99}), "envelope version 99"),
        (
            "kind",
            json!({"kind": "human-item-completed"}),
            "differs from its payload's action",
        ),
        (
            "exhausted",
            json!({"seq": u64::MAX}),
            "past the last a journal can number",
        ),
    ] {
        let mut bad = record.clone();
        for (key, value) in edit.as_object().unwrap() {
            bad[key] = value.clone();
        }
        if name != "exhausted" {
            bad["seq"] = json!(1);
        }
        std::fs::write(&journal, format!("{good}{bad}\n")).unwrap();
        world.refused(None, &["list"], refusal);
    }
    std::fs::write(&journal, &good).unwrap();
    assert_eq!(
        world
            .json(None, &["list", "--json"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// A watch refuses an owner queue holding a record that is not an item event, or
/// one owned by another session, and commits nothing past it.
#[test]
fn a_watch_refuses_a_corrupt_or_misrouted_queue_without_advancing() {
    use onemessagebus::{LocalTransport, QueueName, Transport};
    for (name, record) in [
        ("garbled", b"{\"not\": \"an envelope\"}".to_vec()),
        ("misrouted", Vec::new()),
    ] {
        let world = Live::enabled();
        world.raise(&item(
            "mine",
            "task",
            "low",
            "owner",
            json!({"scope": "nothing"}),
        ));
        world.raise(&item(
            "theirs",
            "task",
            "low",
            "stranger",
            json!({"scope": "nothing"}),
        ));
        let transport = LocalTransport::open(world.store().join("bus")).unwrap();
        let queue = QueueName::try_from(
            format!(
                "owner-{}",
                &onepipeline::human_items::item_id_for("owner")[3..]
            )
            .as_str(),
        )
        .unwrap();
        // Delivered once, so the owner's queue exists.
        let output = world.run(Some("owner"), &["watch"]);
        assert!(output.status.success());
        let bytes = if name == "misrouted" {
            let journal = std::fs::read_to_string(world.store().join("events.ndjson")).unwrap();
            journal.lines().nth(1).unwrap().as_bytes().to_vec()
        } else {
            record
        };
        transport.append(&queue, &bytes).unwrap();
        let refused = world.run(Some("owner"), &["watch"]);
        assert!(!refused.status.success(), "{name}");
        let said = String::from_utf8_lossy(&refused.stderr);
        assert!(
            said.contains("not an item event") || said.contains("owned by stranger"),
            "{name}: {said}"
        );
        let again = world.run(Some("owner"), &["watch"]);
        assert!(
            !again.status.success(),
            "{name}: the cursor moved past what was refused"
        );
    }
}

/// Raising a resolved item's key again, even with new content, answers the item
/// as it was resolved: nothing is reopened, changed or written.
#[test]
fn raising_a_resolved_key_again_changes_nothing() {
    let world = Live::enabled();
    let id = world.raise(&item(
        "done",
        "task",
        "low",
        "s",
        json!({"scope": "nothing"}),
    ));
    let completed = world.json(None, &["complete", &id]);
    let mut changed = item("done", "task", "urgent", "s", json!({"scope": "nothing"}));
    changed["title"] = json!("A different title");
    changed["body"] = json!("A different body.");
    let path = world.input("changed", &changed);
    let receipt = world.json(None, &["raise", "--input", &path]);
    assert_eq!(receipt, completed);
    assert_eq!(world.journal().len(), 2);
}

/// The SDK an embedding UI links acts on the same store as the CLI, and refuses
/// an action that names no actor without recording anything.
#[test]
fn the_sdk_refuses_an_action_naming_no_actor_and_records_nothing() {
    use onepipeline::human_items::{HumanItemAction, HumanItemRefusal, HumanItems};
    let world = Live::enabled();
    let id = world.raise(&item(
        "sdk",
        "task",
        "low",
        "s",
        json!({"scope": "nothing"}),
    ));
    let store = HumanItems::open(world.runs(), &world.repo()).unwrap();
    let refused = store
        .apply(HumanItemAction::Complete {
            id: id.clone(),
            actor: "  ".into(),
            on_behalf_of: None,
        })
        .unwrap_err();
    assert!(
        matches!(&refused, HumanItemRefusal::Conflict { reason } if reason.contains("names its actor")),
        "{refused:?}"
    );
    assert_eq!(world.journal().len(), 1);
    // The same SDK, with an actor, completes it; the CLI reads the result.
    let done = store
        .apply(HumanItemAction::Complete {
            id: id.clone(),
            actor: "ui:operator".into(),
            on_behalf_of: None,
        })
        .unwrap();
    assert_eq!(done.item.item.resolution.unwrap().actor, "ui:operator");
    assert_eq!(
        world.json(None, &["show", &id, "--json"])["item"]["state"],
        "completed"
    );
}

/// `config` without `--json` prints the effective settings, every hook by its
/// phase and every file they were read from.
#[test]
fn config_prints_the_effective_settings_and_where_they_came_from() {
    let world = hooked();
    let text = world.ok(None, &["config"]);
    assert!(text.contains("enabled: true\n"), "{text}");
    assert!(text.contains("task_source: plans\n"), "{text}");
    assert!(
        text.contains("hooks: design-review, ticket-review, ask\n"),
        "{text}"
    );
    // Each `read from:` path names the file it was read from. The working
    // directory may be reached through a symbolic link, as a temporary
    // directory is on macOS, and the path printed is then the resolved one, so
    // it is compared by the file it resolves to rather than by its spelling.
    let file = world.repo().join(".onepipeline/config.yaml");
    let resolved = |path: &str| std::fs::canonicalize(path).unwrap();
    let read = |text: &str| -> Vec<PathBuf> {
        text.lines()
            .filter_map(|line| line.strip_prefix("read from: "))
            .map(resolved)
            .collect()
    };
    assert_eq!(
        read(&text),
        [std::fs::canonicalize(&file).unwrap()],
        "{text}"
    );
    // Creating a symbolic link needs a privilege a Windows runner does not
    // grant, so the reach-through-a-link leg is a Unix one; the comparison by
    // resolved file above holds on every platform.
    #[cfg(unix)]
    {
        let linked = world.root.join("linked");
        std::os::unix::fs::symlink(world.repo(), &linked).unwrap();
        let through_link = world
            .cmd(None, &["config"])
            .current_dir(&linked)
            .output()
            .unwrap();
        assert!(through_link.status.success());
        let text = String::from_utf8(through_link.stdout).unwrap();
        assert_eq!(
            read(&text),
            [std::fs::canonicalize(&file).unwrap()],
            "{text}"
        );
    }
    let bare = Live::new(None).ok(None, &["config"]);
    assert_eq!(bare, "enabled: false\ntask_source: (none)\nhooks: (none)\n");
}

/// A phase context naming a blank identity, a blank document version, or a
/// ticket with a blank id or URL is refused and raises nothing.
#[test]
fn a_phase_context_with_blank_identities_is_refused_without_raising() {
    let world = hooked();
    let owner = |session: &str| json!({"session": session, "run": null, "plan": "plans:alpha"});
    for (phase, ctx, refusal) in [
        (
            "ask",
            json!({"owner": owner("mgr"), "dedup_key": "  ", "vars": {"thing": "x"}}),
            "names its owner session and dedup_key",
        ),
        (
            "ask",
            json!({"owner": owner(" "), "dedup_key": "k", "vars": {"thing": "x"}}),
            "names its owner session and dedup_key",
        ),
        (
            "design-review",
            json!({"owner": owner("mgr"), "dedup_key": "d", "approval": {"board_copy": " ", "key": "v1"}}),
            "nonblank board_copy and key",
        ),
        (
            "design-review",
            json!({"owner": owner("mgr"), "dedup_key": "d", "approval": {"board_copy": "plans:doc-1", "key": ""}}),
            "nonblank board_copy and key",
        ),
        (
            "ticket-review",
            json!({"owner": owner("mgr"), "dedup_key": "t", "tickets": [{"id": " ", "url": "https://tracker/a", "priority": "low"}]}),
            "nonblank id and url",
        ),
        (
            "ticket-review",
            json!({"owner": owner("mgr"), "dedup_key": "t", "tickets": [{"id": "a", "url": "", "priority": "low"}]}),
            "nonblank id and url",
        ),
    ] {
        let path = context(&world, "blank", &ctx);
        world.refused(
            None,
            &["raise", "--phase", phase, "--context", &path],
            refusal,
        );
    }
    assert!(world.journal().is_empty());
    assert!(lines(&world.root.join("requests.ndjson")).is_empty());
}

/// A journal past the size the host allows its files (`EFBIG`, set here by the
/// kernel's own `RLIMIT_FSIZE`) is a full disk to the store: the write is taken
/// back, the reserve released, and — the limit being per file — the refusal
/// says nothing was acknowledged.
#[cfg(target_os = "linux")]
#[test]
fn a_journal_past_its_file_size_limit_refuses_and_acknowledges_nothing() {
    use std::os::unix::process::CommandExt;
    let world = Live::enabled();
    world.raise(&item(
        "first",
        "task",
        "low",
        "s",
        json!({"scope": "nothing"}),
    ));
    let before = std::fs::metadata(world.store().join("events.ndjson"))
        .unwrap()
        .len();
    let mut big = item("big", "task", "low", "s", json!({"scope": "nothing"}));
    big["body"] = json!("x".repeat(64 * 1024));
    let path = world.input("big", &big);
    let mut command = world.cmd(None, &["raise", "--input", &path]);
    let ceiling = before + 4096;
    command.env("LLVM_PROFILE_FILE", world.root.join("ceiling-%p.profraw"));
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            let limit = libc::rlimit {
                rlim_cur: ceiling,
                rlim_max: ceiling,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::signal(libc::SIGXFSZ, libc::SIG_IGN) == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("even into its emergency reserve"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
    assert_eq!(world.journal().len(), 1);
    assert_eq!(
        std::fs::metadata(world.store().join("events.ndjson"))
            .unwrap()
            .len(),
        before
    );
}

/// Raising an open design task's phase again with a newer document version
/// updates its stored reference, and completion submits that version.
#[test]
fn a_design_task_raised_again_completes_against_its_newest_version() {
    let world = hooked();
    let owner = json!({"session": "mgr", "run": null, "plan": "plans:alpha"});
    let raise = |key: &str| {
        let ctx = context(
            &world,
            &format!("design-{key}"),
            &json!({
                "owner": owner, "dedup_key": "alpha", "approval": {"board_copy": "plans:doc-1", "key": key}
            }),
        );
        phase(&world, None, "design-review", &ctx)
    };
    let first = raise("v1");
    let again = raise("v2");
    assert_eq!(again["item"]["item"]["id"], first["item"]["item"]["id"]);
    assert_eq!(again["item"]["item"]["approval"]["key"], "v2");
    std::fs::write(world.root.join("current-key"), "v2").unwrap();
    let id = first["item"]["item"]["id"].as_str().unwrap().to_owned();
    world.json(None, &["complete", &id]);
    let requests = lines(&world.root.join("requests.ndjson"));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["key"], "v2");
    assert_eq!(lines(&world.root.join("approvals.ndjson"))[0]["key"], "v2");
}

/// A stale completion stopped after its replacement is durable and before the
/// obsolete task's dismissal — the host lets the journal grow by one record and
/// no more — leaves both open; completing the obsolete task again finds the same
/// replacement, dismisses the obsolete one, and still files only one replacement.
#[cfg(target_os = "linux")]
#[test]
fn a_stale_completion_stopped_between_its_records_converges_on_retry() {
    use std::os::unix::process::CommandExt;
    let world = hooked();
    let owner = json!({"session": "mgr", "run": null, "plan": "plans:alpha"});
    let ctx = context(
        &world,
        "design",
        &json!({
            "owner": owner, "dedup_key": "alpha", "approval": {"board_copy": "plans:doc-1", "key": "v1"}
        }),
    );
    let id = phase(&world, None, "design-review", &ctx)["item"]["item"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    std::fs::write(world.root.join("current-key"), "v2").unwrap();
    let journal = world.store().join("events.ndjson");
    let before = std::fs::metadata(&journal).unwrap().len();
    let raised = std::fs::read_to_string(&journal)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .len() as u64;
    // Room for the replacement's record, which is the raised record with a longer
    // key, and not for the dismissal's, which adds a resolution to the same item.
    let ceiling = before + raised + 120;
    let mut command = world.cmd(Some("mgr"), &["complete", &id]);
    command.env("LLVM_PROFILE_FILE", world.root.join("ceiling-%p.profraw"));
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            let limit = libc::rlimit {
                rlim_cur: ceiling,
                rlim_max: ceiling,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::signal(libc::SIGXFSZ, libc::SIG_IGN) == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let stopped = command.output().unwrap();
    assert!(!stopped.status.success());
    assert!(String::from_utf8_lossy(&stopped.stderr).contains("could not record this durably"));
    let open = world.json(None, &["list", "--json"]);
    assert_eq!(
        open.as_array().unwrap().len(),
        2,
        "the replacement is durable beside the obsolete task: {open}"
    );
    world.refused(
        Some("mgr"),
        &["complete", &id],
        "Re-read the current document",
    );
    let open = world.json(None, &["list", "--json"]);
    assert_eq!(open.as_array().unwrap().len(), 1);
    assert_eq!(open[0]["item"]["approval"]["key"], "v2");
    assert_eq!(
        world.json(None, &["show", &id, "--json"])["item"]["state"],
        "dismissed"
    );
    assert!(lines(&world.root.join("approvals.ndjson")).is_empty());
}
