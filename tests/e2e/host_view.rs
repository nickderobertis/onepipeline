//! `onepipeline host` over a runs root shaped like a real host's, and what it
//! costs to read one.
//!
//! The view answers *what is running on this host right now*, and it is asked by
//! people and by scripts alike, so these journeys hold two things at once: that
//! every live dispatch is still proved — by its registry entry's pid and start
//! token — and that a run the view can establish has ended costs it close to
//! nothing however large its journal. The second is observed rather than
//! inferred: every ended run's journal is made unreadable to the view's process,
//! and the kernel is asked (through `strace`) which journals the process opened.
//!
//! The fixture is generated here. Its journals are the recorded run
//! `tests/recorded/run-root/onemessagebus-repair-2`, renamed per run and padded
//! to a drawn size with that store's own records in the kind mix a real host's
//! journals carry by bytes, so a journal of a given size costs the view what a
//! real one does. The budget `host-view-time` reads what
//! [`the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again`]
//! records; see `budgets.yaml` and `scripts/host-view-budget.mjs`.
//!
//! Linux only: the live dispatches are real processes whose registry entries
//! carry their procfs start tokens, and the observation is `strace`'s.

// llmlint: ignore-file[e2e_not_mocked] the runs root is generated rather than recorded by a
// driver, because what is under test is the view's read of a root a host accumulates over
// weeks, which no journey can live through. Every byte of it is in shapes the engine itself
// writes — the journals are a recorded run's own records — and the view under test is the
// real compiled binary, read the way an operator reads it.

#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

use serde_json::{json, Value};

use crate::harness::{now_millis, reaped_pid, World};

/// The recorded run every generated journal is made from.
const SEED_RUN: &str = "onemessagebus-repair-2";
const MIB: u64 = 1 << 20;
/// The size of the largest journal on the host the budget was measured on, a
/// long-ended run's: generated sparse, so it costs no disk.
const SPARSE_BYTES: u64 = 3_838_692_260;
/// What one byte of a real host's journals is, by record kind: the share each
/// kind held of that host's 5.2 GiB of journals, measured 2026-10-10.
const KIND_MIX: [(&str, f64); 8] = [
    ("turn-activity", 47.20),
    ("member-heartbeat", 17.05),
    ("planner-surface-queued", 5.85),
    ("planner-surfaced", 4.52),
    ("turn-started", 3.89),
    ("turn-message", 2.36),
    ("turn-completed", 1.32),
    ("oneharness-session", 0.61),
];
/// The host name every generated record and every read carries, so a launch
/// record's driver and a registry entry's pid are this host's to prove.
const HOST: &str = "host-view-fixture";
/// How long after a run records a dispatch its registry entry is stamped: the
/// executor writes it once the dispatch has launched.
const ENTRY_AFTER_MS: u64 = 1_000;
/// The variable asking `onepipeline host` to append its phase breakdown to a file.
const PHASES_ENV: &str = "ONEPIPELINE_HOST_PHASES";
/// The phases a steady read over a fixture records, by the names `src/hostview.rs`,
/// `src/lib.rs` and `src/driver.rs` give them, and the quantity it counts: held
/// here so a phase renamed there fails these journeys rather than dropping out of
/// the budget's detail.
const STEADY_PHASES: [&str; 9] = [
    "event-registry",
    "list",
    "launch-and-summary",
    "registry",
    "pid-proofs",
    "decide-ended",
    "live-fold",
    "free-space",
    "render",
];
/// Where the journeys leave what `scripts/host-view-budget.mjs` reports.
const RECORD: &str = "target/budget-records/host-view.json";

/// A small deterministic generator, so a fixture is the same fixture every time
/// it is generated.
struct Draw(u64);

impl Draw {
    fn next(&mut self) -> u64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).expect("an index")
    }
}

/// A journal stamp, `YYYY-MM-DDThh:mm:ss.sssZ`, for epoch milliseconds.
fn stamp(ms: u64) -> String {
    let days = i64::try_from(ms / 86_400_000).expect("days fit");
    let of_day = ms % 86_400_000;
    // Civil date from days, by the standard era arithmetic.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let of_era = z - era * 146_097;
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * of_year + 2) / 153;
    let day = of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3_600_000,
        of_day / 60_000 % 60,
        of_day / 1_000 % 60,
        of_day % 1_000
    )
}

// llmlint: ignore-block[tests_mirror_real_usage] the subject is `onepipeline host` over a runs
// root as a host accumulates one over weeks — hundreds of ended runs, long-dead drivers, a
// 3.6 GiB journal, engines older than the dispatch registry — which no journey can produce
// through the engine in a test's lifetime. So the root is generated: every journal is the
// recorded run's own records renamed, the launch records and registry entries are in the
// shapes the engine writes, a summary stamp moved stands in for the writer that keeps it
// current, and every read of it is the real binary's. The task that added this view's budget
// states this fixture; the adoption that races the view is driven through the real engine.
/// The recorded store, split into what a run starts with, the records its body is
/// padded with, and how it settles.
struct Seed {
    head: Vec<Value>,
    settle: Vec<Value>,
    /// Each padding kind's records, pre-rendered with their stamp, sequence and
    /// (for a pipeline record) stream left as placeholders.
    body: BTreeMap<&'static str, Vec<String>>,
    launch: Value,
    t0: u64,
}

impl Seed {
    fn read() -> Self {
        let dir = crate::harness::repo_file(&format!("tests/recorded/run-root/{SEED_RUN}"));
        let records: Vec<Value> = std::fs::read_to_string(dir.join("events.jsonl"))
            .expect("the recorded journal")
            .lines()
            .map(|line| serde_json::from_str(line).expect("a recorded record"))
            .collect();
        let at = |kind: &str| {
            records
                .iter()
                .position(|record| record["kind"] == kind)
                .unwrap_or_else(|| panic!("the recorded run holds a {kind}"))
        };
        let head = records[..=at("turn-started")].to_vec();
        let settled_at = at("member-settled");
        let settle = records[settled_at..=at("node-settled")].to_vec();
        let mut body: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
        for (kind, _) in KIND_MIX {
            let rendered: Vec<String> = records[head.len()..settled_at]
                .iter()
                .filter(|record| record["kind"] == kind)
                .map(|record| {
                    let mut record = record.clone();
                    record["ts"] = json!("@@TS@@");
                    record["seq"] = json!("@@SEQ@@");
                    if record["source"] == "pipeline" {
                        record["stream"] = json!("@@STREAM@@");
                    }
                    record.to_string()
                })
                .collect();
            assert!(!rendered.is_empty(), "the recorded run holds no {kind}");
            body.insert(kind, rendered);
        }
        let t0 = crate::harness::epoch_millis(records[0]["ts"].as_str().expect("a stamp"));
        Self {
            head,
            settle,
            body,
            launch: serde_json::from_str(
                &std::fs::read_to_string(dir.join("launch.json")).expect("the recorded launch"),
            )
            .expect("a launch record"),
            t0,
        }
    }

    /// The recorded plan, with a task per extra node beside its one `plan` task.
    fn plan_with(&self, nodes: &[&str]) -> Value {
        let mut plan = self.head[0]["payload"]["plan"].clone();
        let first = plan["tasks"][0].clone();
        for node in nodes {
            let mut task = first.clone();
            task["id"] = json!(node);
            task["title"] = json!(node);
            plan["tasks"].as_array_mut().expect("tasks").push(task);
        }
        plan
    }

    /// One run's store: the recorded head, a body padded to `size` bytes, and the
    /// ending — the recorded settlement where `settled`, then `tail`.
    #[allow(
        clippy::too_many_arguments,
        reason = "each is one fact the generated journal varies by: who, how big, when it began, whether it settled, what it ends in, which extra nodes its plan names, and the draw"
    )]
    fn journal(
        &self,
        draw: &mut Draw,
        run: &str,
        size: u64,
        start_ms: u64,
        settled: bool,
        tail: &[Value],
        nodes: &[&str],
    ) -> Vec<u8> {
        let shift = start_ms.saturating_sub(self.t0);
        let mut store = Store {
            run,
            out: String::new(),
            seqs: BTreeMap::new(),
            clock: start_ms,
        };
        for record in &self.head {
            let mut record = record.clone();
            if record["kind"] == "run-started" && !nodes.is_empty() {
                record["payload"]["plan"] = self.plan_with(nodes);
            }
            let recorded = crate::harness::epoch_millis(record["ts"].as_str().expect("a stamp"));
            store.emit(&record, recorded + shift);
        }
        let ending: Vec<Value> = if settled {
            self.settle.iter().chain(tail).cloned().collect()
        } else {
            tail.to_vec()
        };
        let ending_bytes: usize = ending
            .iter()
            .map(|record| record.to_string().len() + 1)
            .sum::<usize>()
            + 512;
        let target = usize::try_from(size)
            .expect("a size")
            .saturating_sub(ending_bytes);
        let total: f64 = KIND_MIX.iter().map(|(_, share)| share).sum();
        let mut surfaces = 0u64;
        while store.out.len() < target {
            let mut pick = draw.unit() * total;
            let kind = KIND_MIX
                .iter()
                .find(|(_, share)| {
                    pick -= share;
                    pick < 0.0
                })
                .map_or(KIND_MIX[0].0, |(kind, _)| kind);
            let choices = &self.body[kind];
            let template = &choices[draw.below(choices.len())];
            store.clock += 5 + draw.next() % 896;
            // Each surface on a stream of its own, as the channel's producers write them.
            let stream = if template.contains("@@STREAM@@") {
                surfaces += 1;
                format!("{HOST}-surface-{surfaces}")
            } else {
                template
                    .split("\"stream\":\"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .unwrap_or_default()
                    .to_owned()
            };
            let seq = store.next_seq(&stream);
            let line = template
                .replace(SEED_RUN, run)
                .replacen("\"@@TS@@\"", &format!("\"{}\"", stamp(store.clock)), 1)
                .replacen("\"@@SEQ@@\"", &seq.to_string(), 1)
                .replacen("@@STREAM@@", &stream, 1);
            store.out.push_str(&line);
            store.out.push('\n');
        }
        for record in &ending {
            let at = store.clock + 1_000;
            store.emit(record, at);
        }
        store.out.into_bytes()
    }
}

/// One journal as it is written: its records, each stream's next sequence
/// number, and the clock its stamps advance on.
struct Store<'a> {
    run: &'a str,
    out: String,
    seqs: BTreeMap<String, u64>,
    clock: u64,
}

impl Store<'_> {
    fn next_seq(&mut self, stream: &str) -> u64 {
        let seq = self.seqs.entry(stream.to_owned()).or_insert(0);
        *seq += 1;
        *seq - 1
    }

    /// Append one record, renamed to this run and stamped no earlier than `at`.
    fn emit(&mut self, record: &Value, at: u64) {
        let mut record: Value =
            serde_json::from_str(&record.to_string().replace(SEED_RUN, self.run)).expect("JSON");
        self.clock = self.clock.max(at);
        record["ts"] = json!(stamp(self.clock));
        let stream = record["stream"].as_str().unwrap_or_default().to_owned();
        record["seq"] = json!(self.next_seq(&stream));
        self.out.push_str(&record.to_string());
        self.out.push('\n');
    }
}

/// A `pipeline` record of this host's.
fn pipeline(run: &str, kind: &str, node: Option<&str>, at: u64, payload: Value) -> Value {
    let mut labels = json!({"run_id": run});
    if let Some(node) = node {
        labels["node"] = json!(node);
    }
    json!({
        "v": 2,
        "ts": stamp(at),
        "stream": format!("{HOST}-4000002"),
        "seq": 0,
        "source": "pipeline",
        "kind": kind,
        "labels": labels,
        "payload": payload,
        "artifacts": [],
    })
}

/// A run's stop, as an engine records it, on a stream of the stopping process's own.
fn stopped(run: &str, teardown: Option<&str>, at: u64) -> Value {
    let mut payload = json!({"owner": "[mine]", "forced": false});
    if let Some(teardown) = teardown {
        payload["teardown"] = json!(teardown);
    }
    let mut record = pipeline(run, "run-stopped", None, at, payload);
    record["stream"] = json!(format!("{HOST}-4000001"));
    record
}

/// The two records a driver writes as it dispatches a node.
fn dispatched(run: &str, node: &str, at: u64) -> Vec<Value> {
    ["node-ready", "node-dispatched"]
        .into_iter()
        .map(|kind| pipeline(run, kind, Some(node), at, json!({})))
        .collect()
}

/// A process's start token as this host's procfs spells it, the way the engine
/// records one — asked of `/proc` here rather than of the crate under test.
fn start_token(pid: u32) -> String {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("a live process");
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .expect("a stat line")
        .1
        .split_whitespace()
        .collect();
    format!("linux-proc-stat:{}", fields[19])
}

/// Where each read's verdict for a run is expected to land.
#[derive(Default)]
struct Expected {
    /// Every `(run, node)` a held row.
    held: BTreeSet<(String, String)>,
    /// Every `(run, node)` an `UNPROVEN` row.
    unproven: BTreeSet<(String, String)>,
    /// Every `(run, node)` gone because its registry entry is stale.
    stale: BTreeSet<(String, String)>,
    /// Every `(run, node)` gone because its registry-less run was stopped.
    stopped: BTreeSet<(String, String)>,
    /// Every run root refused, by name.
    skipped: BTreeSet<String>,
}

/// One generated runs root, and the processes its live dispatches are.
struct Fixture {
    runs: PathBuf,
    expected: Expected,
    /// The runs that hold a live dispatch, whose journals the view folds.
    live_runs: BTreeSet<String>,
    /// Every run whose journal a steady read may not open.
    ended: Vec<String>,
    /// The ended, settled runs a grown fixture's history is drawn from.
    settled: Vec<String>,
    sparse_run: String,
    /// The bytes of records the sparse journal holds ahead of its hole.
    sparse_records: u64,
    children: Vec<Child>,
}

/// The generated runs go with the fixture, before the world that holds them is
/// dropped: nothing drives them, so the world's teardown — a `stop` of every run it
/// finds, which folds each one — has nothing of theirs to stop.
impl Drop for Fixture {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.runs);
    }
}

/// Sizes for `n` journals in a real host's proportions: of every 90, 45 at or
/// below 0.2 MiB, 36 up to 2.6 MiB, 8 up to 28.7 MiB and one of 29 MiB.
fn sizes(draw: &mut Draw) -> (Vec<u64>, Vec<u64>) {
    let mib = |value: f64| (value * MIB as f64) as u64;
    let small: Vec<u64> = (0..45).map(|_| mib(draw.between(0.02, 0.2))).collect();
    let mut rest: Vec<u64> = (0..36).map(|_| mib(draw.between(0.2, 2.6))).collect();
    rest.extend((0..8).map(|_| mib(draw.between(2.6, 28.7))));
    rest.push(29 * MIB);
    (small, rest)
}

impl Fixture {
    /// The 100-run fixture: 9 roots with no launch record, 64 with a dispatch
    /// registry, 91 journals (one of them the sparse 3.6 GiB one), 3 runs holding
    /// 5 live dispatches in real processes — one of them a run whose driver has
    /// died — 4 stale registry entries, and 2 registry-less ended runs recording a
    /// node running, stopped `signalled` and `partly-signalled`, written as an
    /// engine before the registry wrote them: `round-NN` directories and no
    /// `dispatches/`.
    fn generate(runs: &Path) -> Self {
        let mut draw = Draw(100);
        let seed = Seed::read();
        let runs = runs.to_path_buf();
        std::fs::create_dir_all(&runs).expect("a runs root");
        let mut fixture = Self {
            runs,
            expected: Expected::default(),
            live_runs: BTreeSet::new(),
            ended: Vec::new(),
            settled: Vec::new(),
            sparse_run: "huge-ended".to_owned(),
            sparse_records: 0,
            children: Vec::new(),
        };
        let now = now_millis();
        let hours_ago = now - 3 * 3_600_000;
        let (mut small, mut rest) = sizes(&mut draw);

        for i in 0..9 {
            let name = format!("no-launch-{i:02}");
            std::fs::create_dir_all(fixture.runs.join(&name).join("channel")).expect("a root");
            fixture.expected.skipped.insert(name);
        }

        // 3 live runs holding 5 live dispatches; the third's driver has died, and
        // its dispatch outlived it.
        for (i, nodes) in [vec!["plan", "build"], vec!["plan", "review"], vec!["plan"]]
            .into_iter()
            .enumerate()
        {
            let run = format!("live-{i}");
            let (driver, driver_token) = if i == 2 {
                (reaped_pid(), "linux-proc-stat:1".to_owned())
            } else {
                fixture.sleeper()
            };
            let mut tail = Vec::new();
            let mut entries = Vec::new();
            let started = now - 600_000;
            for (k, node) in nodes.iter().enumerate() {
                let (pid, token) = fixture.sleeper();
                // An entry is stamped just after the run records the dispatch.
                let recorded = if *node == "plan" {
                    started
                } else {
                    let at = now - 60_000 * (k as u64 + 1);
                    tail.extend(dispatched(&run, node, at));
                    at
                };
                entries.push(json!({
                    "node": node, "pid": pid, "host": HOST,
                    "dispatched_at": stamp(recorded + ENTRY_AFTER_MS), "started": token,
                }));
                fixture
                    .expected
                    .held
                    .insert((run.clone(), (*node).to_owned()));
            }
            let extra: Vec<&str> = nodes.iter().copied().filter(|n| *n != "plan").collect();
            let size = small.pop().expect("a size");
            let journal = seed.journal(&mut draw, &run, size, started, false, &tail, &extra);
            fixture.write_run(
                &seed,
                &run,
                &journal,
                Some(&entries),
                driver,
                &driver_token,
                &extra,
            );
            fixture.live_runs.insert(run);
        }

        // 4 stale registry entries: a node the run records running, whose process is gone.
        for i in 0..4u64 {
            let run = format!("stale-{i}");
            let started = hours_ago - i * 86_400_000;
            let entry = json!({
                "node": "plan", "pid": reaped_pid(), "host": HOST,
                "dispatched_at": stamp(started + ENTRY_AFTER_MS), "started": "linux-proc-stat:1",
            });
            let size = rest.remove(0);
            let journal = seed.journal(&mut draw, &run, size, started, false, &[], &[]);
            fixture.write_run(
                &seed,
                &run,
                &journal,
                Some(&[entry]),
                reaped_pid(),
                "linux-proc-stat:1",
                &[],
            );
            fixture
                .expected
                .stale
                .insert((run.clone(), "plan".to_owned()));
            fixture.ended.push(run);
        }

        // 2 registry-less ended runs recording `plan` running, stopped long ago.
        for (run, teardown) in [
            ("registryless-stopped", "signalled"),
            ("registryless-undecided", "partly-signalled"),
        ] {
            let started = hours_ago - 50 * 86_400_000;
            let size = small.pop().expect("a size");
            let mut journal = seed.journal(&mut draw, run, size, started, false, &[], &[]);
            let last = last_stamp(&journal);
            journal.extend(format!("{}\n", stopped(run, Some(teardown), last + 60_000)).bytes());
            fixture.write_run(
                &seed,
                run,
                &journal,
                None,
                reaped_pid(),
                "linux-proc-stat:1",
                &[],
            );
            std::fs::create_dir(fixture.runs.join(run).join("round-01")).expect("a round");
            fixture.ended.push(run.to_owned());
        }
        fixture
            .expected
            .stopped
            .insert(("registryless-stopped".to_owned(), "plan".to_owned()));
        fixture
            .expected
            .unproven
            .insert(("registryless-undecided".to_owned(), "plan".to_owned()));

        // The 3.6 GiB journal's run: ended and settled, its registry emptied as its
        // dispatches ended. Its records are generated small and the file grown
        // sparse past them, so its 3.6 GiB cost no disk.
        let sparse = fixture.sparse_run.clone();
        let journal = seed.journal(
            &mut draw,
            &sparse,
            MIB / 5,
            hours_ago - 40 * 86_400_000,
            true,
            &[],
            &[],
        );
        fixture.write_run(
            &seed,
            &sparse,
            &journal,
            Some(&[]),
            reaped_pid(),
            "linux-proc-stat:1",
            &[],
        );
        fixture.sparse_records = journal.len() as u64;
        fixture.sparse_to(SPARSE_BYTES);
        fixture.ended.push(sparse);

        // The rest: ended, settled runs, to 91 journals and 64 registries.
        let mut sizes: Vec<u64> = small.into_iter().chain(rest).collect();
        let with_registry = 64 - 3 - 4 - 1;
        let mut i = 0;
        while !sizes.is_empty() {
            let size = sizes.remove(draw.below(sizes.len()));
            let run = format!("ended-{i:03}");
            let journal = seed.journal(
                &mut draw,
                &run,
                size,
                hours_ago - (i as u64 + 1) * 3_600_000,
                true,
                &[],
                &[],
            );
            let registry: Option<&[Value]> = if i < with_registry { Some(&[]) } else { None };
            fixture.write_run(
                &seed,
                &run,
                &journal,
                registry,
                reaped_pid(),
                "linux-proc-stat:1",
                &[],
            );
            fixture.ended.push(run.clone());
            fixture.settled.push(run);
            i += 1;
        }
        assert_eq!(fixture.roots(), 100, "the fixture is not 100 run roots");
        fixture
    }

    /// A real process standing in for a dispatch or a driver, and its start token.
    fn sleeper(&mut self) -> (u32, String) {
        let child = Command::new("sleep")
            .arg("86400")
            .stdin(Stdio::null())
            .spawn()
            .expect("a process to hold a dispatch");
        let pid = child.id();
        self.children.push(child);
        (pid, start_token(pid))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "each is one document of the run root it writes: its journal, its registry, the launch record's driver and start token, and its plan's extra nodes"
    )]
    fn write_run(
        &self,
        seed: &Seed,
        run: &str,
        journal: &[u8],
        registry: Option<&[Value]>,
        driver: u32,
        driver_token: &str,
        nodes: &[&str],
    ) {
        let dir = self.runs.join(run);
        std::fs::create_dir_all(&dir).expect("a run root");
        let mut launch = seed.launch.clone();
        launch["run_id"] = json!(run);
        launch["project"] = json!(format!("authoring:{run}"));
        launch["host"] = json!(HOST);
        launch["pid"] = json!(driver);
        launch["started"] = json!(driver_token);
        write(&dir.join("launch.json"), &launch);
        write(&dir.join("plan.json"), &seed.plan_with(nodes));
        std::fs::write(dir.join("events.jsonl"), journal).expect("a journal");
        if let Some(entries) = registry {
            std::fs::create_dir(dir.join("dispatches")).expect("a registry");
            for (n, entry) in entries.iter().enumerate() {
                write(
                    &dir.join("dispatches")
                        .join(format!("{}-{n}.json", entry["pid"])),
                    entry,
                );
            }
        }
    }

    fn roots(&self) -> usize {
        std::fs::read_dir(&self.runs).expect("the root").count()
    }

    fn journal(&self, run: &str) -> PathBuf {
        self.runs.join(run).join("events.jsonl")
    }

    /// Make each of `runs`' journals readable, or not, to the view's process.
    fn readable(&self, runs: &[String], readable: bool) {
        for run in runs {
            let mode = if readable { 0o644 } else { 0o000 };
            let _ =
                std::fs::set_permissions(self.journal(run), std::fs::Permissions::from_mode(mode));
        }
    }

    /// Every ended run but the sparse one, whose journal is unreadable in every state.
    fn ended_but_sparse(&self) -> Vec<String> {
        self.ended
            .iter()
            .filter(|run| **run != self.sparse_run)
            .cloned()
            .collect()
    }

    /// Set the sparse journal's length: its records alone, or grown past them.
    fn sparse_to(&self, len: u64) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(self.journal(&self.sparse_run))
            .and_then(|file| file.set_len(len))
            .expect("the sparse journal's length");
    }

    /// The writer-maintained summary of the sparse run, standing in for the journal
    /// writer that keeps it current as it appends: written by a read of the binary
    /// under test over the journal's records alone — every other ended journal
    /// unreadable, so nothing else is read — then stamped to the 3.6 GiB file.
    fn stamp_sparse(&self, world: &World, program: &Path) {
        self.readable(&self.ended, false);
        self.readable(std::slice::from_ref(&self.sparse_run), true);
        self.sparse_to(self.sparse_records);
        let read = world
            .cmd_on(program, &["host"])
            .output()
            .expect("the binary runs");
        assert!(
            read.status.success(),
            "{}",
            String::from_utf8_lossy(&read.stderr)
        );
        self.sparse_to(SPARSE_BYTES);
        self.readable(std::slice::from_ref(&self.sparse_run), false);
        let journal = self.journal(&self.sparse_run);
        let summary_path = self.runs.join(&self.sparse_run).join("summary.json");
        let mut summary = read_json(&summary_path);
        let about = std::fs::metadata(&journal).expect("the journal");
        summary["journal_len"] = json!(about.len());
        summary["journal_mtime_ms"] = json!(about
            .modified()
            .expect("a modification time")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_millis() as u64);
        write(&summary_path, &summary);
    }

    /// Make the root cold: no derived document beside any ended journal, every
    /// ended journal readable but the sparse one, which no state makes readable.
    fn cold(&self) {
        self.forget_derived();
        self.readable(&self.ended_but_sparse(), true);
        self.readable(std::slice::from_ref(&self.sparse_run), false);
    }

    /// Forget every ended run's derived documents: the cold state.
    fn forget_derived(&self) {
        for run in &self.ended {
            for document in ["summary.json", "checkpoint.json"] {
                let _ = std::fs::remove_file(self.runs.join(run).join(document));
            }
        }
    }

    /// Grow the root with ended, settled runs — and launch-less roots, at 9% — to
    /// `total` run roots. Each added run is one of the 100-run fixture's own
    /// settled runs with its journal **hardlinked**, and its summary and checkpoint
    /// rewritten to its own id, so the disk it costs is a few KiB.
    fn grow(&mut self, total: usize) {
        let mut draw = Draw(total as u64);
        let mut launchless = self.expected.skipped.len();
        while launchless < total * 9 / 100 {
            let name = format!("no-launch-{launchless:03}");
            std::fs::create_dir_all(self.runs.join(&name).join("channel")).expect("a root");
            self.expected.skipped.insert(name);
            launchless += 1;
        }
        let mut i = 0;
        while self.roots() < total {
            let source = self.settled[draw.below(self.settled.len())].clone();
            let run = format!("clone-{total}-{i:04}");
            i += 1;
            let (from, to) = (self.runs.join(&source), self.runs.join(&run));
            std::fs::create_dir(&to).expect("a run root");
            std::fs::hard_link(from.join("events.jsonl"), to.join("events.jsonl")).expect("a link");
            std::fs::hard_link(from.join("plan.json"), to.join("plan.json")).expect("a link");
            let mut launch = read_json(&from.join("launch.json"));
            launch["run_id"] = json!(run);
            launch["project"] = json!(format!("authoring:{run}"));
            write(&to.join("launch.json"), &launch);
            if from.join("dispatches").is_dir() {
                std::fs::create_dir(to.join("dispatches")).expect("a registry");
            }
            for document in ["summary.json", "checkpoint.json"] {
                let mut body = read_json(&from.join(document));
                body["run_id"] = json!(run);
                write(&to.join(document), &body);
            }
            self.ended.push(run);
        }
    }
}

// llmlint: ignore-end[tests_mirror_real_usage]
fn write(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_string_pretty(value).expect("JSON")).expect("a write");
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The stamp of a journal's last record, in epoch milliseconds.
fn last_stamp(journal: &[u8]) -> u64 {
    let text = std::str::from_utf8(journal).expect("UTF-8");
    let last: Value = serde_json::from_str(text.lines().last().expect("a record")).expect("JSON");
    crate::harness::epoch_millis(last["ts"].as_str().expect("a stamp"))
}

/// One real invocation of the binary, kept whole.
struct Invocation {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    wall_ms: f64,
    spawned_unix_ns: u128,
}

impl Invocation {
    fn reading(&self) -> Value {
        assert_eq!(
            self.code,
            Some(0),
            "`host --json` exited {:?}: {}",
            self.code,
            self.stderr
        );
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("`host --json` printed no reading ({e}): {}", self.stdout))
    }
}

fn unix_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_nanos()
}

/// Run `command` to its exit, timing it from spawn to reap.
fn invoke(mut command: Command) -> Invocation {
    let spawned_unix_ns = unix_ns();
    let began = Instant::now();
    let output = command.output().expect("the binary runs");
    Invocation {
        wall_ms: began.elapsed().as_secs_f64() * 1_000.0,
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        spawned_unix_ns,
    }
}

/// What the kernel says one traced read did with the runs' journals.
struct Traced {
    reading: Value,
    /// Bytes read from each run's journal the process opened, by run.
    opened: BTreeMap<String, u64>,
    /// The runs whose journal the process tried and failed to open.
    refused: BTreeSet<String>,
    /// Its wall clock, tracer included.
    wall_ms: f64,
    /// Its phase breakdown, where one was asked for.
    phases: Option<Value>,
}

/// `host --json` under `strace`, which records every open — and, with `reads`,
/// every read — of a journal.
///
/// It **refuses** rather than passes where the tracer will not run: the claim is
/// what the process did, and an observation nobody made is not an observation of
/// nothing.
fn traced(
    world: &World,
    program: &Path,
    into: &Path,
    reads: bool,
    phases: Option<&Path>,
) -> Traced {
    let mut inner = world.cmd_on(program, &["host", "--json"]);
    if let Some(phases) = phases {
        inner.env(PHASES_ENV, phases);
    }
    let calls = if reads {
        "trace=openat,open,read,pread64"
    } else {
        "trace=openat,open"
    };
    let mut command = Command::new("strace");
    // `--seccomp-bpf` stops the process only at the calls traced, so a read traced
    // for its opens alone runs close to its untraced speed.
    command
        .args([
            "-f",
            "--seccomp-bpf",
            "-qq",
            "-y",
            "-e",
            calls,
            "-e",
            "signal=none",
            "-o",
        ])
        .arg(into)
        .arg(inner.get_program())
        .args(inner.get_args())
        .stdin(Stdio::null());
    for (key, value) in inner.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let invocation = invoke(command);
    let reading = invocation.reading();
    let trace = std::fs::read_to_string(into).unwrap_or_else(|error| {
        panic!(
            "this journey's claim is what the process opened, and the tracer left no trace \
             ({error}): install strace, or run the suite where ptrace is permitted"
        )
    });
    assert!(
        trace.contains("openat("),
        "the tracer recorded no open: {trace}"
    );
    let run_of = |path: &str| -> Option<String> {
        let dir = path.strip_suffix("/events.jsonl")?;
        Path::new(dir)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    let mut opened = BTreeMap::new();
    let mut refused = BTreeSet::new();
    for line in trace.lines() {
        let body = line.split_once(' ').map_or(line, |(_, rest)| rest);
        let result = body
            .rsplit_once("= ")
            .map_or("", |(_, result)| result.trim());
        if body.starts_with("openat(") || body.starts_with("open(") {
            let Some(run) = body.split('"').nth(1).and_then(run_of) else {
                continue;
            };
            if result.starts_with('-') {
                refused.insert(run);
            } else {
                opened.entry(run).or_insert(0);
            }
        } else if body.starts_with("read(") || body.starts_with("pread64(") {
            let Some(run) = body
                .split_once('<')
                .and_then(|(_, rest)| rest.split_once('>'))
                .and_then(|(path, _)| run_of(path))
            else {
                continue;
            };
            *opened.entry(run).or_insert(0) += result.parse::<u64>().unwrap_or(0);
        }
    }
    Traced {
        phases: phases.and_then(|phases| phases_of(phases, &invocation)),
        wall_ms: invocation.wall_ms,
        reading,
        opened,
        refused,
    }
}

/// The verdicts of one reading, keyed so two readings compare.
#[derive(Debug, PartialEq, Eq)]
struct Verdicts {
    rows: Vec<(String, String, String)>,
    gone: BTreeSet<(String, String)>,
    skipped: BTreeSet<String>,
}

fn verdicts(reading: &Value) -> Verdicts {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    Verdicts {
        rows: reading["dispatches"]
            .as_array()
            .expect("dispatches")
            .iter()
            .map(|row| (text(&row["run"]), text(&row["node"]), text(&row["proof"])))
            .collect(),
        gone: reading["gone"]
            .as_array()
            .expect("gone")
            .iter()
            .map(|gone| (text(&gone["run"]), text(&gone["node"])))
            .collect(),
        skipped: reading["skipped"]
            .as_array()
            .expect("skipped")
            .iter()
            .map(|skipped| {
                Path::new(skipped["path"].as_str().expect("a path"))
                    .file_name()
                    .expect("a name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect(),
    }
}

/// Hold one `--json` reading to the fixture's expected answer.
fn holds(expected: &Expected, reading: &Value, what: &str) {
    assert_eq!(reading["schema_version"], 1, "{what}: {reading}");
    assert_eq!(reading["host"], HOST, "{what}");
    assert_eq!(reading["root_error"], Value::Null, "{what}");
    assert!(
        !reading["free_space"]
            .as_array()
            .expect("free space")
            .is_empty(),
        "{what}: no free space"
    );
    let mut rows: BTreeMap<(String, String), &Value> = BTreeMap::new();
    for row in reading["dispatches"].as_array().expect("dispatches") {
        let key = (
            row["run"].as_str().expect("a run").to_owned(),
            row["node"].as_str().expect("a node").to_owned(),
        );
        assert!(
            rows.insert(key.clone(), row).is_none(),
            "{what}: {key:?} twice"
        );
    }
    for key in &expected.held {
        let row = rows
            .get(key)
            .unwrap_or_else(|| panic!("{what}: live {key:?} is no row"));
        assert_eq!(row["proof"], "held", "{what}: live {key:?}: {row}");
        assert_eq!(row["unproven"], Value::Null, "{what}: {row}");
    }
    for key in &expected.unproven {
        let row = rows
            .get(key)
            .unwrap_or_else(|| panic!("{what}: {key:?} is no row"));
        assert_eq!(row["proof"], "unproven", "{what}: {row}");
        assert!(
            row["unproven"].as_str().is_some_and(|why| !why.is_empty()),
            "{what}: {key:?} is UNPROVEN with no reason: {row}"
        );
    }
    let expected_rows: BTreeSet<_> = expected.held.union(&expected.unproven).cloned().collect();
    assert_eq!(
        rows.keys().cloned().collect::<BTreeSet<_>>(),
        expected_rows,
        "{what}: rows the fixture does not expect"
    );
    let gone: BTreeMap<(String, String), String> = reading["gone"]
        .as_array()
        .expect("gone")
        .iter()
        .map(|gone| {
            (
                (
                    gone["run"].as_str().expect("a run").to_owned(),
                    gone["node"].as_str().expect("a node").to_owned(),
                ),
                gone["reason"].as_str().expect("a reason").to_owned(),
            )
        })
        .collect();
    let expected_gone: BTreeSet<_> = expected.stale.union(&expected.stopped).cloned().collect();
    assert_eq!(
        gone.keys().cloned().collect::<BTreeSet<_>>(),
        expected_gone,
        "{what}: gone"
    );
    for key in &expected.stale {
        assert!(
            gone[key].contains("is gone"),
            "{what}: {key:?}: {}",
            gone[key]
        );
    }
    for key in &expected.stopped {
        let reason = &gone[key];
        assert!(
            reason.contains("its run was stopped") && reason.contains("its workers signalled"),
            "{what}: {key:?}: {reason}"
        );
        assert!(!reason.contains("watched"), "{what}: {reason}");
    }
    assert_eq!(
        verdicts(reading).skipped,
        expected.skipped,
        "{what}: skipped"
    );
}

/// The text form is the same reading rendered: a row per dispatch in the same
/// order, each gone dispatch named, and the refused roots counted.
fn renders(text: &str, reading: &Value, what: &str) {
    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0;
    for row in reading["dispatches"].as_array().expect("dispatches") {
        let (run, node) = (
            row["run"].as_str().expect("run"),
            row["node"].as_str().expect("node"),
        );
        let found = lines[at..]
            .iter()
            .position(|line| {
                let words: Vec<&str> = line.split_whitespace().collect();
                words.first() == Some(&run) && words.get(1) == Some(&node)
            })
            .unwrap_or_else(|| {
                panic!("{what}: the text has no row for {run}/{node} in order:\n{text}")
            });
        let line = lines[at + found];
        assert_eq!(
            line.contains("UNPROVEN"),
            row["proof"] == "unproven",
            "{what}: {line}"
        );
        at += found + 1;
    }
    for gone in reading["gone"].as_array().expect("gone") {
        let named = format!(
            "{}/{}",
            gone["run"].as_str().expect("run"),
            gone["node"].as_str().expect("node")
        );
        assert!(
            text.contains(&named),
            "{what}: the text does not name {named} gone:\n{text}"
        );
    }
    let refused = reading["skipped"].as_array().expect("skipped").len();
    if refused > 0 {
        assert!(
            text.contains(&format!("{refused} run root")),
            "{what}: the text does not count {refused} refused roots:\n{text}"
        );
    }
}

/// The middle of a set of timed reads.
fn median(walls: &[f64]) -> f64 {
    let mut sorted = walls.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

/// One state of the root, read `repeats` times at `host --json` (each timed and
/// its answer held), once as text, and once traced.
struct StateRead {
    walls_ms: Vec<f64>,
    phases: Option<Value>,
    traced: Traced,
    verdicts: Verdicts,
    load1: f64,
}

fn load1() -> f64 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|text| {
            text.split_whitespace()
                .next()
                .and_then(|load| load.parse().ok())
        })
        .unwrap_or(f64::NAN)
}

fn read_state(
    world: &World,
    program: &Path,
    fixture: &Fixture,
    label: &str,
    repeats: usize,
) -> StateRead {
    let phases = world.root.join(format!("{label}.phases.jsonl"));
    let mut walls_ms = Vec::new();
    let mut last = None;
    let load = load1();
    for n in 1..=repeats {
        let mut command = world.cmd_on(program, &["host", "--json"]);
        command.env(PHASES_ENV, &phases);
        let invocation = invoke(command);
        holds(
            &fixture.expected,
            &invocation.reading(),
            &format!("{label} read {n}"),
        );
        walls_ms.push(invocation.wall_ms);
        last = Some(invocation);
    }
    let last = last.expect("a read");
    let text = invoke(world.cmd_on(program, &["host"]));
    assert_eq!(text.code, Some(0), "{label}: text form: {}", text.stderr);
    let reading = last.reading();
    renders(&text.stdout, &reading, label);
    let traced = traced(
        world,
        program,
        &world.root.join(format!("{label}.strace")),
        true,
        None,
    );
    holds(
        &fixture.expected,
        &traced.reading,
        &format!("{label} traced read"),
    );
    assert!(
        !traced.opened.contains_key(&fixture.sparse_run),
        "{label}: the 3.6 GiB journal was opened"
    );
    let phases = phases_of(&phases, &last);
    recorded(&phases, &STEADY_PHASES, "live-journal-bytes", label);
    StateRead {
        walls_ms,
        phases,
        verdicts: verdicts(&reading),
        traced,
        load1: load,
    }
}

/// The last phase line a read appended to `phases`, with the process's start and
/// exit — what came before the command's work and after it — measured against the
/// invocation's own spawn and wall clock.
fn phases_of(phases: &Path, invocation: &Invocation) -> Option<Value> {
    let lines = std::fs::read_to_string(phases).ok()?;
    let mut line: Value = serde_json::from_str(lines.lines().last()?).ok()?;
    let marks = &line["marks_unix_ns"];
    let ns = |key: &str| marks[key].as_u64().map(u128::from);
    if let (Some(run), Some(flushed)) = (ns("run"), ns("flushed")) {
        let exited = invocation.spawned_unix_ns + (invocation.wall_ms * 1e6) as u128;
        line["process_start_ms"] =
            json!(run.saturating_sub(invocation.spawned_unix_ns) as f64 / 1e6);
        line["exit_ms"] = json!(exited.saturating_sub(flushed) as f64 / 1e6);
    }
    Some(line)
}

/// Fail a read whose phase line is missing, or lacks a phase or the quantity it
/// was expected to record.
fn recorded(phases: &Option<Value>, names: &[&str], quantity: &str, label: &str) {
    let line = phases
        .as_ref()
        .unwrap_or_else(|| panic!("{label}: the read recorded no phase line"));
    for name in names {
        assert!(
            line["phases"][name]["wall_ms"].is_number(),
            "{label}: the read recorded no `{name}` phase: {line}"
        );
    }
    assert!(
        line["quantities"][quantity].is_u64(),
        "{label}: the read counted no `{quantity}`: {line}"
    );
    assert!(
        line["process_start_ms"].is_number() && line["exit_ms"].is_number(),
        "{label}: the read marked no start or end of its work: {line}"
    );
}

/// Fail a read that opened any ended run's journal, even one it read nothing
/// from, or that tried to; answer with the runs whose journals it did open.
fn opened_no_ended_journal(ended: &[String], traced: &Traced, label: &str) -> BTreeSet<String> {
    let ended: BTreeSet<&String> = ended.iter().collect();
    let opened: Vec<&String> = traced
        .opened
        .keys()
        .filter(|run| ended.contains(run))
        .collect();
    assert!(
        opened.is_empty(),
        "{label}: a steady read opened ended runs' journals: {opened:?} ({:?})",
        traced.opened
    );
    let tried: Vec<&String> = traced
        .refused
        .iter()
        .filter(|run| ended.contains(run))
        .collect();
    assert!(
        tried.is_empty(),
        "{label}: a steady read tried to open {tried:?}"
    );
    traced.opened.keys().cloned().collect()
}

/// Merge `entries` into the budget record, under a lock, so the two journeys that
/// write it can run at once.
fn record(entries: Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(RECORD);
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("the record's directory");
    let lock = std::fs::File::create(path.with_extension("lock")).expect("a lock file");
    lock.lock().expect("the record's lock");
    let mut held: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    for (key, value) in entries.as_object().expect("an object") {
        held[key] = value.clone();
    }
    held["schema_version"] = json!(1);
    std::fs::write(&path, serde_json::to_string_pretty(&held).expect("JSON")).expect("the record");
    let _ = lock.unlock();
}

/// What a read's phases say, as the budget's record carries it.
fn breakdown(phases: &Option<Value>) -> Value {
    let field = |path: &[&str]| {
        phases.as_ref().map_or(Value::Null, |line| {
            path.iter().fold(line, |value, key| &value[*key]).clone()
        })
    };
    let ms = |name: &str| field(&["phases", name, "wall_ms"]);
    json!({
        "process_start_ms": field(&["process_start_ms"]),
        "event_registry_ms": ms("event-registry"),
        "list_ms": ms("list"),
        "launch_and_summary_ms": ms("launch-and-summary"),
        "registry_ms": ms("registry"),
        "pid_proofs_ms": ms("pid-proofs"),
        "decide_ended_ms": ms("decide-ended"),
        "live_fold_ms": ms("live-fold"),
        "live_runs_folded": field(&["phases", "live-fold", "count"]),
        "live_journal_bytes": field(&["quantities", "live-journal-bytes"]),
        "cold_fold_ms": ms("cold-fold"),
        "cold_journal_bytes": field(&["quantities", "cold-journal-bytes"]),
        "free_space_ms": ms("free-space"),
        "render_ms": ms("render"),
        "exit_ms": field(&["exit_ms"]),
        "read_bytes": field(&["read_bytes"]),
    })
}

/// The binary users run: an optimized release build of this tree, which is what
/// `host-view-time` times — a debug build's figure measures its build flavour.
///
/// The e2e recipes build it ahead of the suite (`_host-view-release` in the
/// justfile), because a build from cold outlasts a test's own bound; here cargo is
/// asked again, which costs a fingerprint check when that build is current and
/// rebuilds it when it is not, so the binary timed is always this tree's. Asked
/// with the coverage tier's flags replaced by `-D warnings`, exactly as the recipe
/// asks, so the two builds are one build. The copy handed back is this test's own, so a later
/// rebuild's uplift cannot take the file away under a spawn.
fn release_binary(world: &World) -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let built = BUILT.get_or_init(|| {
        let repo = crate::harness::repo_file(".");
        let mut cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cargo
            .args([
                "build",
                "--release",
                "--locked",
                "--offline",
                "--bin",
                "onepipeline",
            ])
            .arg("--target-dir")
            .arg(repo.join("target"))
            .current_dir(&repo);
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            // What cargo sets for a running test, and what the coverage tier sets for
            // its own build, are neither of them the release build's.
            if (name.starts_with("CARGO_") && name != "CARGO_HOME")
                || name.starts_with("LLVM_")
                || [
                    "RUSTFLAGS",
                    "RUSTDOCFLAGS",
                    "RUSTC_WORKSPACE_WRAPPER",
                    "OUT_DIR",
                ]
                .contains(&name.as_ref())
            {
                cargo.env_remove(name.as_ref());
            }
        }
        cargo.env("RUSTFLAGS", "-D warnings");
        let output = cargo.output().expect("cargo runs");
        assert!(
            output.status.success(),
            "the release build `host-view-time` times did not build: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        repo.join("target/release/onepipeline")
    });
    let own = world.root.join("onepipeline-release");
    std::fs::copy(built, &own).expect("this test's own copy of the release build");
    own
}

/// The cold read: no ended run's derived document, every ended journal readable
/// but the 3.6 GiB one, which stays unreadable. Traced for its opens alone, so it
/// runs close to its untraced speed and the journey sees that the sparse journal's
/// verdict came without the read even trying to open it.
fn cold_read(world: &World, program: &Path, fixture: &Fixture) -> Traced {
    fixture.cold();
    let phases = world.root.join("cold.phases.jsonl");
    let cold = traced(
        world,
        program,
        &world.root.join("cold.strace"),
        false,
        Some(&phases),
    );
    holds(&fixture.expected, &cold.reading, "the cold read");
    recorded(
        &cold.phases,
        &["cold-fold"],
        "cold-journal-bytes",
        "the cold read",
    );
    assert!(
        !cold.opened.contains_key(&fixture.sparse_run)
            && !cold.refused.contains(&fixture.sparse_run),
        "the cold read reached for the 3.6 GiB journal"
    );
    cold
}

/// The budget's journey, over the 100-run fixture.
///
/// The root is generated as a host's root is before any reader has been through
/// it: cold, with no derived document beside any journal. One read answers it —
/// every ended journal readable but the 3.6 GiB one, which no state makes readable
/// — and leaves it steady, as a host reaches steady state. The next read, with
/// every ended journal made unreadable, answers identically and opens none of
/// them. Then the sparse run's summary is stamped to its 3.6 GiB file, standing in
/// for the journal writer that keeps it current, and five single reads at that
/// steady state are timed, each held to the answer, with the text form rendering
/// the same reading.
///
/// What is timed is the release build, which is what a user runs; the same five
/// reads by this suite's own build are recorded beside them as detail, never as
/// the verdict.
#[test]
fn the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again() {
    let world = World::new("host-view").with_env("HOSTNAME", HOST);
    let release = release_binary(&world);
    let fixture = Fixture::generate(&world.runs);

    let cold = cold_read(&world, &release, &fixture);
    let cold_verdicts = verdicts(&cold.reading);

    // The read after the cold one is steady: no ended journal is opened.
    fixture.readable(&fixture.ended, false);
    let after_cold = traced(
        &world,
        &release,
        &world.root.join("after-cold.strace"),
        true,
        None,
    );
    holds(
        &fixture.expected,
        &after_cold.reading,
        "the read after the cold one",
    );
    assert_eq!(
        verdicts(&after_cold.reading),
        cold_verdicts,
        "the read after the cold one"
    );
    opened_no_ended_journal(&fixture.ended, &after_cold, "the read after the cold one");

    fixture.stamp_sparse(&world, &release);
    let steady = read_state(&world, &release, &fixture, "steady", 5);
    let opened = opened_no_ended_journal(&fixture.ended, &steady.traced, "steady");
    assert_eq!(
        opened, fixture.live_runs,
        "a steady read folds exactly the runs holding live work"
    );
    assert_eq!(
        steady.verdicts, cold_verdicts,
        "the steady and cold reads disagree"
    );
    let debug = read_state(
        &world,
        &crate::harness::binary(),
        &fixture,
        "steady-debug",
        5,
    );
    opened_no_ended_journal(&fixture.ended, &debug.traced, "steady, this suite's build");
    assert_eq!(debug.verdicts, cold_verdicts, "the two builds disagree");

    record(json!({
        "runs_root": {
            "run_roots": fixture.roots(),
            "live_dispatches": fixture.expected.held.len(),
            "stale_entries": fixture.expected.stale.len(),
        },
        "steady": {
            "build": "release",
            "reads_ms": steady.walls_ms,
            "median_ms": median(&steady.walls_ms),
            "load1": steady.load1,
            "breakdown": breakdown(&steady.phases),
            "ended_journal_bytes_opened": 0,
            "runs_opened": opened,
        },
        "steady_debug": {
            "build": "the e2e tier's own (debug, and coverage-instrumented under `just check`)",
            "reads_ms": debug.walls_ms,
            "median_ms": median(&debug.walls_ms),
            "load1": debug.load1,
            "breakdown": breakdown(&debug.phases),
        },
        "cold": {
            "build": "release",
            "read_ms": cold.wall_ms,
            "breakdown": breakdown(&cold.phases),
        },
        "after_cold": {
            "ended_journal_bytes_opened": 0,
        },
    }));
}

/// What a read costs does not grow with ended history. Three roots hold the same
/// live set — 3 runs holding 5 live dispatches, 4 stale registry entries and the
/// 2 registry-less ended runs — and differ only in how many ended runs they keep:
/// 100, 400 and 800 run roots. At steady state, every ended journal unreadable,
/// each opens no ended run's journal, opens the same runs (the live ones), and
/// prints the same rows and proved-gone dispatches. Each size's single reads by
/// the release build, and the per-ended-run increment between their medians, go
/// to the budget's record as evidence: no threshold applies to them beyond the
/// budget's own at 100.
#[test]
fn the_host_view_over_a_host_sized_root_opens_only_live_runs_whatever_its_ended_history() {
    let world = World::new("host-view-scaling").with_env("HOSTNAME", HOST);
    let release = release_binary(&world);
    let mut fixture = Fixture::generate(&world.runs);
    cold_read(&world, &release, &fixture);
    fixture.stamp_sparse(&world, &release);
    let mut sizes = Vec::new();
    let mut first: Option<(Verdicts, BTreeSet<String>)> = None;
    for total in [100, 400, 800] {
        fixture.grow(total);
        assert_eq!(fixture.roots(), total);
        fixture.readable(&fixture.ended, false);
        let label = format!("scaling-{total}");
        let read = read_state(&world, &release, &fixture, &label, 5);
        let opened = opened_no_ended_journal(&fixture.ended, &read.traced, &label);
        assert_eq!(opened, fixture.live_runs, "{label}: the runs opened");
        match &first {
            None => first = Some((read.verdicts, opened.clone())),
            Some((at_100, opened_at_100)) => {
                assert_eq!(read.verdicts.rows, at_100.rows, "{label}: rows");
                assert_eq!(read.verdicts.gone, at_100.gone, "{label}: gone");
                assert_eq!(&opened, opened_at_100, "{label}: the runs opened");
            }
        }
        sizes.push(json!({
            "run_roots": total,
            "ended_runs": fixture.ended.len(),
            "reads_ms": read.walls_ms,
            "median_ms": median(&read.walls_ms),
            "load1": read.load1,
            "ended_journal_bytes_opened": 0,
            "runs_opened": opened,
        }));
    }
    let increments: Vec<Value> = sizes
        .windows(2)
        .map(|pair| {
            let ms = |size: &Value| size["median_ms"].as_f64().expect("a median");
            let ended = |size: &Value| size["ended_runs"].as_f64().expect("a count");
            json!({
                "from": pair[0]["run_roots"],
                "to": pair[1]["run_roots"],
                "ms_per_ended_run":
                    (ms(&pair[1]) - ms(&pair[0])) / (ended(&pair[1]) - ended(&pair[0])),
            })
        })
        .collect();
    record(json!({"scaling": {"build": "release", "sizes": sizes, "increments": increments}}));
}

// llmlint: ignore-block[tests_mirror_real_usage] the same generated root, run by run: see the
// reason on the generator above.
/// A small root, built run by run, for the journeys about one verdict each.
impl Fixture {
    fn empty(runs: &Path) -> Self {
        std::fs::create_dir_all(runs).expect("a runs root");
        Self {
            runs: runs.to_path_buf(),
            expected: Expected::default(),
            live_runs: BTreeSet::new(),
            ended: Vec::new(),
            settled: Vec::new(),
            sparse_run: String::new(),
            sparse_records: 0,
            children: Vec::new(),
        }
    }

    /// A run recording `plan` running, written as an engine before the dispatch
    /// registry wrote one — a `round-01` directory and no `dispatches/` — ending in
    /// `ending`, appended in order after its last record a minute apart.
    fn registry_less(&mut self, seed: &Seed, run: &str, ending: &[&dyn Fn(u64) -> Value]) {
        let mut draw = Draw(run.len() as u64);
        let started = now_millis() - 30 * 86_400_000;
        let mut journal = seed.journal(&mut draw, run, 40 * 1024, started, false, &[], &[]);
        let mut at = last_stamp(&journal);
        for record in ending {
            at += 60_000;
            journal.extend(format!("{}\n", record(at)).bytes());
        }
        self.write_run(
            seed,
            run,
            &journal,
            None,
            reaped_pid(),
            "linux-proc-stat:1",
            &[],
        );
        std::fs::create_dir(self.runs.join(run).join("round-01")).expect("a round");
        self.ended.push(run.to_owned());
    }
}

/// A `driver-adopted` record, naming a driver that is gone.
fn adopted(run: &str, at: u64) -> Value {
    pipeline(
        run,
        "driver-adopted",
        None,
        at,
        json!({"adoption": 1, "pid": reaped_pid()}),
    )
}

// llmlint: ignore-end[tests_mirror_real_usage]
/// The keys version 1 of the reading states, at each level, every one present.
fn has_the_version_1_keys(reading: &Value) {
    let keys = |value: &Value| -> BTreeSet<String> {
        value
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect()
    };
    let set =
        |keys: &[&str]| -> BTreeSet<String> { keys.iter().map(|k| (*k).to_owned()).collect() };
    assert_eq!(
        keys(reading),
        set(&[
            "schema_version",
            "host",
            "root",
            "root_error",
            "free_space",
            "dispatches",
            "gone",
            "skipped"
        ])
    );
    for line in reading["free_space"].as_array().expect("free space") {
        assert_eq!(
            keys(line),
            set(&["roots", "free_bytes", "total_bytes", "error"])
        );
        assert_eq!(
            line["error"].is_null(),
            !(line["free_bytes"].is_null() && line["total_bytes"].is_null()),
            "{line}"
        );
        for root in line["roots"].as_array().expect("roots") {
            assert_eq!(keys(root), set(&["role", "path", "measured_at"]));
            assert!(["runs-root", "workspaces"].contains(&root["role"].as_str().expect("a role")));
        }
    }
    for row in reading["dispatches"].as_array().expect("dispatches") {
        assert_eq!(
            keys(row),
            set(&["run", "node", "launcher", "age_ms", "proof", "unproven"])
        );
        assert_eq!(row["unproven"].is_null(), row["proof"] == "held", "{row}");
    }
    for gone in reading["gone"].as_array().expect("gone") {
        assert_eq!(keys(gone), set(&["run", "node", "reason"]));
    }
    for skipped in reading["skipped"].as_array().expect("skipped") {
        assert_eq!(keys(skipped), set(&["path", "reason"]));
    }
}

/// The text and `--json` forms of one read, each held to the other.
fn both_forms(world: &World, label: &str) -> (Value, String) {
    let json = invoke(world.cmd(&["host", "--json"]));
    let reading = json.reading();
    assert_eq!(
        json.stdout.trim_end().lines().count(),
        1,
        "one line: {}",
        json.stdout
    );
    has_the_version_1_keys(&reading);
    let text = invoke(world.cmd(&["host"]));
    assert_eq!(text.code, Some(0), "{label}: {}", text.stderr);
    renders(&text.stdout, &reading, label);
    (reading, text.stdout)
}

/// A registry-less run — one written by an engine before the dispatch registry —
/// that records a node running is decided by its last recorded stop.
///
/// One whose last `run-stopped` records `teardown: signalled`, or records no
/// teardown at all (as engines before the field wrote it), with no
/// `driver-adopted` after it, is not a row: its dispatch is counted on the line of
/// dispatches proven gone, with a reason saying the run was stopped and its
/// workers signalled — never that their teardown was watched, which no record of
/// those engines says. One whose last stop is anything else, one adopted after
/// its stop, and one never stopped stay `UNPROVEN` rows with their reasons. A live
/// dispatch whose run's driver has died is still a held row, and a stale registry
/// entry is gone and not a row. And appending an adoption and a re-dispatch to a
/// run proven gone makes its dispatch an `UNPROVEN` row again.
#[test]
fn a_registry_less_runs_last_stop_decides_whether_its_dispatch_is_gone() {
    let world = World::new("host-view-stops").with_env("HOSTNAME", HOST);
    let seed = Seed::read();
    // llmlint: ignore-block[tests_mirror_real_usage] runs as engines before the dispatch registry
    // left them, and the adoption appended to one, are the generated root's: see the reason on
    // the generator. `an_adoption_racing_the_view…` drives a real adoption through the engine.
    let mut fixture = Fixture::empty(&world.runs);
    let plan = |run: &str| (run.to_owned(), "plan".to_owned());

    let signalled = |run: &'static str| move |at: u64| stopped(run, Some("signalled"), at);
    fixture.registry_less(
        &seed,
        "stopped-signalled",
        &[&signalled("stopped-signalled")],
    );
    fixture.registry_less(
        &seed,
        "stopped-unrecorded",
        &[&|at| stopped("stopped-unrecorded", None, at)],
    );
    fixture.registry_less(
        &seed,
        "stopped-partly",
        &[&|at| stopped("stopped-partly", Some("partly-signalled"), at)],
    );
    fixture.registry_less(
        &seed,
        "stopped-then-adopted",
        // An adoption abandons the dispatch in flight, so the node is running again
        // only once the adopting driver dispatches it again.
        &[
            &signalled("stopped-then-adopted"),
            &|at| adopted("stopped-then-adopted", at),
            &|at| {
                pipeline(
                    "stopped-then-adopted",
                    "node-ready",
                    Some("plan"),
                    at,
                    json!({}),
                )
            },
            &|at| {
                pipeline(
                    "stopped-then-adopted",
                    "node-dispatched",
                    Some("plan"),
                    at,
                    json!({}),
                )
            },
        ],
    );
    fixture.registry_less(&seed, "never-stopped", &[]);
    fixture.expected.stopped.insert(plan("stopped-signalled"));
    fixture.expected.stopped.insert(plan("stopped-unrecorded"));
    for run in ["stopped-partly", "stopped-then-adopted", "never-stopped"] {
        fixture.expected.unproven.insert(plan(run));
    }

    // A live dispatch whose run's driver has died, and a stale registry entry.
    let mut draw = Draw(3);
    let started = now_millis() - 600_000;
    let (pid, token) = fixture.sleeper();
    let journal = seed.journal(&mut draw, "orphaned", 40 * 1024, started, false, &[], &[]);
    let entry = json!({"node": "plan", "pid": pid, "host": HOST,
        "dispatched_at": stamp(started + ENTRY_AFTER_MS), "started": token});
    fixture.write_run(
        &seed,
        "orphaned",
        &journal,
        Some(&[entry]),
        reaped_pid(),
        "linux-proc-stat:1",
        &[],
    );
    fixture.expected.held.insert(plan("orphaned"));
    let journal = seed.journal(&mut draw, "stale", 40 * 1024, started, false, &[], &[]);
    let entry = json!({"node": "plan", "pid": reaped_pid(), "host": HOST,
        "dispatched_at": stamp(started + ENTRY_AFTER_MS), "started": "linux-proc-stat:1"});
    fixture.write_run(
        &seed,
        "stale",
        &journal,
        Some(&[entry]),
        reaped_pid(),
        "linux-proc-stat:1",
        &[],
    );
    fixture.expected.stale.insert(plan("stale"));
    fixture.ended.push("stale".to_owned());
    // And a run that dispatched its node again after the only entry its registry
    // holds for it was written: that stale entry is an earlier attempt's, and says
    // nothing of the dispatch the run records now.
    let mut journal = seed.journal(
        &mut draw,
        "redispatched",
        40 * 1024,
        started,
        false,
        &[],
        &[],
    );
    let again = last_stamp(&journal) + 60_000;
    for record in dispatched("redispatched", "plan", again) {
        journal.extend(format!("{record}\n").bytes());
    }
    let entry = json!({"node": "plan", "pid": reaped_pid(), "host": HOST,
        "dispatched_at": stamp(started + ENTRY_AFTER_MS), "started": "linux-proc-stat:1"});
    fixture.write_run(
        &seed,
        "redispatched",
        &journal,
        Some(&[entry]),
        reaped_pid(),
        "linux-proc-stat:1",
        &[],
    );
    fixture.expected.unproven.insert(plan("redispatched"));
    fixture.ended.push("redispatched".to_owned());

    let (reading, text) = both_forms(&world, "first read");
    holds(&fixture.expected, &reading, "first read");
    let line = |starting: &str| {
        text.lines()
            .find(|line| line.trim_start().starts_with(starting))
            .unwrap_or_else(|| panic!("no line starting {starting:?}:\n{text}"))
            .to_owned()
    };
    let stopped_line = line("2 dispatches of stopped runs proven gone:");
    for run in ["stopped-signalled", "stopped-unrecorded"] {
        assert!(
            stopped_line.contains(&format!("{run}/plan: its run was stopped ")),
            "{stopped_line}"
        );
    }
    assert!(
        stopped_line.contains(" ago and its workers signalled"),
        "{stopped_line}"
    );
    assert!(!stopped_line.contains("teardown"), "{stopped_line}");
    assert!(
        line("1 stale registry entry ignored:").contains("stale/plan"),
        "{text}"
    );
    let row = |run: &str| line(run);
    assert!(row("orphaned").contains("plan") && !row("orphaned").contains("UNPROVEN"));
    for run in ["stopped-partly", "stopped-then-adopted", "never-stopped"] {
        assert!(
            row(run).contains("UNPROVEN: the run's dispatch registry cannot be read"),
            "{}",
            row(run)
        );
    }
    assert!(
        row("redispatched").contains("UNPROVEN: the run recorded this dispatch after every entry"),
        "{}",
        row("redispatched")
    );

    // A summary a reader cannot serve — malformed, at another version, or another
    // run's — is refolded, and the answer does not move.
    let summary = |run: &str| world.runs.join(run).join("summary.json");
    std::fs::write(summary("stale"), "{").expect("a malformed summary");
    let mut other_version = read_json(&summary("stopped-partly"));
    other_version["schema_version"] = json!(999);
    write(&summary("stopped-partly"), &other_version);
    let mut another_run = read_json(&summary("never-stopped"));
    another_run["run_id"] = json!("someone-else");
    write(&summary("never-stopped"), &another_run);
    let (refolded, _) = both_forms(&world, "over summaries it cannot serve");
    holds(
        &fixture.expected,
        &refolded,
        "over summaries it cannot serve",
    );
    for run in ["stale", "stopped-partly", "never-stopped"] {
        let rewritten = read_json(&summary(run));
        assert_eq!(rewritten["run_id"], run, "{run}'s summary was not refolded");
        assert_ne!(
            rewritten["schema_version"], 999,
            "{run}'s summary was not refolded"
        );
    }

    // The read proved `stopped-signalled` ended and left its checkpoint covering
    // its final stop; the next read, every ended journal unreadable, takes the
    // same verdicts off the documents alone.
    let checkpoint = read_json(&world.runs.join("stopped-signalled/checkpoint.json"));
    let journal = fixture.journal("stopped-signalled");
    assert_eq!(
        checkpoint["coverage"]["bytes"].as_u64(),
        Some(std::fs::metadata(&journal).expect("a journal").len()),
        "the ended run's checkpoint does not cover its final stop"
    );
    assert_eq!(checkpoint["state"]["stop"], "workers-signalled");
    fixture.readable(&fixture.ended, false);
    let steady = traced(
        &world,
        &crate::harness::binary(),
        &world.root.join("stops.strace"),
        true,
        None,
    );
    holds(&fixture.expected, &steady.reading, "the steady read");
    opened_no_ended_journal(&fixture.ended, &steady, "the steady read");
    fixture.readable(&fixture.ended, true);

    // An adoption past the stop, and the adopting driver's re-dispatch of the node:
    // the run is driven again, and its dispatch is no longer proven gone.
    let at = last_stamp(&std::fs::read(&journal).expect("a journal")) + 60_000;
    let mut appended = String::new();
    for record in std::iter::once(adopted("stopped-signalled", at)).chain(dispatched(
        "stopped-signalled",
        "plan",
        at + 1_000,
    )) {
        appended.push_str(&format!("{record}\n"));
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .expect("the journal");
    std::io::Write::write_all(&mut file, appended.as_bytes()).expect("appended");
    fixture.expected.stopped.remove(&plan("stopped-signalled"));
    fixture.expected.unproven.insert(plan("stopped-signalled"));
    // llmlint: ignore-end[tests_mirror_real_usage]
    let (reading, _) = both_forms(&world, "after an adoption");
    holds(&fixture.expected, &reading, "after an adoption");
}

/// No journal past the 1 GiB bound is opened, in any state — each of these is
/// readable, and the kernel is asked whether the read touched it — and a run that
/// bound leaves undecided is refused, flagged, never left out. A registry-less
/// run past it with no current summary is listed in `skipped` with its reason,
/// in both forms, and so is one whose driver may still be running with no entry
/// live. A run past it with a registry is decided from its entries otherwise: a
/// held one is a row, and a stale one is gone.
///
/// And a run under the bound whose journal the read has to fold, and cannot open,
/// is refused the same way, rather than folded to nothing and left out.
#[test]
fn a_journal_past_the_bound_is_never_opened_and_a_run_it_leaves_undecided_is_refused() {
    let world = World::new("host-view-bound").with_env("HOSTNAME", HOST);
    let seed = Seed::read();
    // llmlint: ignore-block[tests_mirror_real_usage] journals past the bound are the generated
    // root's, grown sparse: see the reason on the generator.
    let mut fixture = Fixture::empty(&world.runs);
    let mut draw = Draw(11);
    let started = now_millis() - 86_400_000;
    let past = (1u64 << 30) + MIB;
    let grown = |fixture: &Fixture, run: &str| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(fixture.journal(run))
            .and_then(|file| file.set_len(past))
            .expect("a journal past the bound");
    };

    let journal = seed.journal(
        &mut draw,
        "huge-registry-less",
        40 * 1024,
        started,
        false,
        &[],
        &[],
    );
    fixture.write_run(
        &seed,
        "huge-registry-less",
        &journal,
        None,
        reaped_pid(),
        "linux-proc-stat:1",
        &[],
    );
    grown(&fixture, "huge-registry-less");

    let journal = seed.journal(&mut draw, "huge-stale", 40 * 1024, started, false, &[], &[]);
    let entry = json!({"node": "plan", "pid": reaped_pid(), "host": HOST,
        "dispatched_at": stamp(started + ENTRY_AFTER_MS), "started": "linux-proc-stat:1"});
    fixture.write_run(
        &seed,
        "huge-stale",
        &journal,
        Some(&[entry]),
        reaped_pid(),
        "linux-proc-stat:1",
        &[],
    );
    grown(&fixture, "huge-stale");

    let (driver, driver_token) = fixture.sleeper();
    let (pid, token) = fixture.sleeper();
    let journal = seed.journal(&mut draw, "huge-live", 40 * 1024, started, false, &[], &[]);
    let entry = json!({"node": "plan", "pid": pid, "host": HOST,
        "dispatched_at": stamp(started + ENTRY_AFTER_MS), "started": token});
    fixture.write_run(
        &seed,
        "huge-live",
        &journal,
        Some(&[entry]),
        driver,
        &driver_token,
        &[],
    );
    grown(&fixture, "huge-live");

    // A driver that may still be running and no entry live: what it has running
    // is not in its registry, and only the journal could say.
    let (driver, driver_token) = fixture.sleeper();
    let journal = seed.journal(
        &mut draw,
        "huge-driven",
        40 * 1024,
        started,
        false,
        &[],
        &[],
    );
    let entry = json!({"node": "plan", "pid": reaped_pid(), "host": HOST,
        "dispatched_at": stamp(started + ENTRY_AFTER_MS), "started": "linux-proc-stat:1"});
    fixture.write_run(
        &seed,
        "huge-driven",
        &journal,
        Some(&[entry]),
        driver,
        &driver_token,
        &[],
    );
    grown(&fixture, "huge-driven");

    // And a run under the bound whose journal the read would have to fold, cold, and
    // cannot open: a fold of nothing would leave its dispatch out.
    let journal = seed.journal(&mut draw, "unreadable", 40 * 1024, started, false, &[], &[]);
    fixture.write_run(
        &seed,
        "unreadable",
        &journal,
        None,
        reaped_pid(),
        "linux-proc-stat:1",
        &[],
    );
    fixture.readable(&["unreadable".to_owned()], false);

    // llmlint: ignore-end[tests_mirror_real_usage]
    let read = traced(
        &world,
        &crate::harness::binary(),
        &world.root.join("bound.strace"),
        true,
        None,
    );
    let huge = [
        "huge-registry-less",
        "huge-stale",
        "huge-live",
        "huge-driven",
    ];
    let reached: Vec<&&str> = huge
        .iter()
        .filter(|run| read.opened.contains_key(**run) || read.refused.contains(**run))
        .collect();
    assert!(
        reached.is_empty(),
        "a journal past the bound was reached for: {reached:?}"
    );

    let (reading, text) = both_forms(&world, "past the bound");
    assert_eq!(verdicts(&reading), verdicts(&read.reading));
    let skipped: BTreeMap<String, String> = reading["skipped"]
        .as_array()
        .expect("skipped")
        .iter()
        .map(|skipped| {
            let path = Path::new(skipped["path"].as_str().expect("a path"));
            (
                path.file_name()
                    .expect("a name")
                    .to_string_lossy()
                    .into_owned(),
                skipped["reason"].as_str().expect("a reason").to_owned(),
            )
        })
        .collect();
    assert_eq!(
        skipped.keys().collect::<Vec<_>>(),
        ["huge-driven", "huge-registry-less", "unreadable"],
        "{reading}"
    );
    assert!(
        skipped["huge-driven"].contains("past the 1073741824 a host read opens")
            && skipped["huge-driven"].ends_with("its driver is not proved gone"),
        "{}",
        skipped["huge-driven"]
    );
    let reason = &skipped["huge-registry-less"];
    assert!(
        reason.contains("past the 1073741824 a host read opens")
            && reason.contains("its dispatch registry cannot be read"),
        "{reason}"
    );
    assert!(
        skipped["unreadable"].starts_with("its journal cannot be read"),
        "{}",
        skipped["unreadable"]
    );
    assert!(
        text.contains("3 run root(s) skipped:")
            && text.contains("huge-registry-less: its journal is")
            && text.contains("unreadable: its journal cannot be read"),
        "{text}"
    );
    let rows = verdicts(&reading).rows;
    assert_eq!(
        rows,
        vec![("huge-live".to_owned(), "plan".to_owned(), "held".to_owned())],
        "{reading}"
    );
    assert_eq!(
        verdicts(&reading).gone,
        BTreeSet::from([("huge-stale".to_owned(), "plan".to_owned())])
    );
}

/// The bytes of `journal` the first `bytes` cover, as whole records: `None` where
/// they end inside one.
fn records_in(journal: &[u8], bytes: u64) -> Option<u64> {
    let prefix = journal.get(..usize::try_from(bytes).ok()?)?;
    (prefix.is_empty() || prefix.ends_with(b"\n"))
        .then(|| prefix.iter().filter(|byte| **byte == b'\n').count() as u64)
}

/// The view's checkpoint extension races a real driver adopting the run it
/// extended, and loses nothing.
///
/// A run is stopped with its dispatch's registry entry left stale; a read of the
/// view proves it ended and extends its checkpoint over the stop. Then the real
/// engine adopts it, through the path `onepipeline adopt` takes, with only the
/// paid model doubled: it appends `driver-adopted`, dispatches the node again
/// with a live registry entry, goes on recording, and writes the run's checkpoint
/// and summary as it always does — while the view reads the root over and over,
/// extending what it may. The adopting driver then dies, and its dispatch
/// outlives it.
///
/// The journal holds every record the driver appended, in order; the checkpoint
/// and the summary are each a document whose coverage is a prefix of the journal
/// as it now stands; a fold from that checkpoint is a fold of the whole journal;
/// and the adopted run's live dispatch is a row on every read after its entry was
/// written, never reported gone.
#[test]
fn an_adoption_racing_the_view_loses_no_record_and_its_dispatch_is_never_gone() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    let world = World::new("host-view-adopted");
    world.script("build.wait", "hold");
    let run = "raced";
    let path = world.plan(
        run,
        &crate::harness::plan_of(run, vec![crate::harness::agent("build", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch to record where it is running", |world| {
        !world.dispatch_records(run).is_empty()
    });
    let pid_of = |record: &PathBuf| read_json(record)["pid"].as_u64().expect("a pid");
    let first = pid_of(&world.dispatch_records(run)[0]);
    world.run(&["stop", run]).exited(0);

    let journal = world.run_file(run, "events.jsonl");
    let reading = invoke(world.cmd(&["host", "--json"])).reading();
    assert!(
        verdicts(&reading)
            .gone
            .contains(&(run.to_owned(), "build".to_owned())),
        "the stopped run's dispatch is not gone: {reading}"
    );
    let checkpoint = read_json(&world.run_file(run, "checkpoint.json"));
    assert_eq!(
        checkpoint["coverage"]["bytes"].as_u64(),
        Some(std::fs::metadata(&journal).expect("a journal").len()),
        "the view did not extend the ended run's checkpoint"
    );

    // The view reads on and on while the adoption lands.
    let reading_on = Arc::new(AtomicBool::new(true));
    let reads: Arc<Mutex<Vec<(Instant, Value)>>> = Arc::default();
    let reader = {
        let (reading_on, reads) = (Arc::clone(&reading_on), Arc::clone(&reads));
        // The command `World` composes, carried into the thread as its parts.
        let template = world.cmd(&["host", "--json"]);
        let program = template.get_program().to_owned();
        let args: Vec<_> = template.get_args().map(ToOwned::to_owned).collect();
        let envs: Vec<_> = template
            .get_envs()
            .map(|(k, v)| (k.to_owned(), v.map(ToOwned::to_owned)))
            .collect();
        std::thread::spawn(move || {
            while reading_on.load(Ordering::SeqCst) {
                let mut command = Command::new(&program);
                command.args(&args).stdin(Stdio::null());
                for (key, value) in &envs {
                    match value {
                        Some(value) => command.env(key, value),
                        None => command.env_remove(key),
                    };
                }
                let began = Instant::now();
                let reading = invoke(command).reading();
                reads.lock().expect("the reads").push((began, reading));
            }
        })
    };

    let mut adopting = world
        .cmd(&["adopt", run])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the adopting driver starts");
    world.until("the adopting driver to dispatch the node again", |world| {
        world
            .journal(run)
            .iter()
            .skip_while(|event| event["kind"] != "driver-adopted")
            .any(|event| event["kind"] == "node-dispatched")
    });
    world.until("the new dispatch to record where it is running", |world| {
        world
            .dispatch_records(run)
            .iter()
            .any(|record| pid_of(record) != first)
    });
    let entry_written = Instant::now();
    world.until("the view to read the run a few times since", |_| {
        reads
            .lock()
            .expect("the reads")
            .iter()
            .filter(|(began, _)| *began > entry_written)
            .count()
            >= 3
    });
    // The adopting driver dies, as a crash would end it; its dispatch outlives it.
    adopting.kill().expect("the adopting driver is ended");
    let _ = adopting.wait();
    reading_on.store(false, Ordering::SeqCst);
    reader.join().expect("the reader");

    let second = world
        .dispatch_records(run)
        .iter()
        .map(pid_of)
        .find(|pid| *pid != first)
        .expect("the new dispatch's entry");
    for (began, reading) in reads.lock().expect("the reads").iter() {
        let verdict = verdicts(reading);
        let key = (run.to_owned(), "build".to_owned());
        for gone in reading["gone"].as_array().expect("gone") {
            assert!(
                !gone["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(&second.to_string()),
                "the adopted run's live dispatch was reported gone: {reading}"
            );
        }
        if *began > entry_written {
            assert!(
                verdict
                    .rows
                    .iter()
                    .any(|(r, n, _)| (r, n) == (&key.0, &key.1))
                    && !verdict.gone.contains(&key),
                "a read after the new dispatch's entry was written left it out: {reading}"
            );
        }
    }

    // Every record the driver appended is there, in order: each stream's
    // sequence numbers run unbroken from where they start, in file order.
    let bytes = std::fs::read(&journal).expect("the journal");
    let mut seqs: BTreeMap<String, u64> = BTreeMap::new();
    for line in String::from_utf8_lossy(&bytes).lines() {
        let record: Value = serde_json::from_str(line).expect("a whole record");
        let stream = record["stream"].as_str().expect("a stream").to_owned();
        let seq = record["seq"].as_u64().expect("a seq");
        if let Some(previous) = seqs.insert(stream.clone(), seq) {
            assert_eq!(
                seq,
                previous + 1,
                "stream {stream} skipped or reordered a record"
            );
        }
    }
    assert!(
        String::from_utf8_lossy(&bytes).contains("\"driver-adopted\""),
        "the adoption is not in the journal"
    );

    // The derived documents are each a valid prefix of the journal as it stands.
    let checkpoint = read_json(&world.run_file(run, "checkpoint.json"));
    assert_eq!(checkpoint["schema_version"], 11);
    assert_eq!(
        records_in(
            &bytes,
            checkpoint["coverage"]["bytes"].as_u64().expect("bytes")
        ),
        checkpoint["coverage"]["records"].as_u64(),
        "the checkpoint claims records the journal does not hold where it says: {}",
        checkpoint["coverage"]
    );
    let summary = read_json(&world.run_file(run, "summary.json"));
    assert_eq!(
        records_in(&bytes, summary["journal_len"].as_u64().expect("a length")),
        summary["event_count"].as_u64(),
        "the summary's stamp is not a prefix of the journal"
    );

    // A fold from that checkpoint is a fold of the whole journal.
    let whole = World::new("host-view-adopted-whole");
    let copied = whole.runs.join(run);
    std::fs::create_dir_all(&copied).expect("a copy");
    for file in ["events.jsonl", "launch.json", "plan.json"] {
        std::fs::copy(world.run_file(run, file), copied.join(file)).expect("a copy");
    }
    let from_checkpoint = world.run(&["results", run]);
    from_checkpoint.exited(0);
    let from_nothing = whole.run(&["results", run]);
    from_nothing.exited(0);
    assert_eq!(from_checkpoint.stdout, from_nothing.stdout);

    // And the next read shows the dispatch that outlived the adopting driver.
    let reading = invoke(world.cmd(&["host", "--json"])).reading();
    let row = reading["dispatches"]
        .as_array()
        .expect("dispatches")
        .iter()
        .find(|row| row["run"] == run && row["node"] == "build")
        .unwrap_or_else(|| panic!("the adopted run's dispatch is no row: {reading}"));
    assert!(
        ["held", "unproven"].contains(&row["proof"].as_str().expect("a proof")),
        "{row}"
    );
    world.release("build.go");
}

/// A runs root that cannot be read is the reading's `root_error`, never one of
/// its `skipped` roots, and the text form says it was refused rather than that the
/// host is idle.
#[test]
fn a_runs_root_that_cannot_be_read_is_the_readings_root_error() {
    let world = World::new("host-view-root").with_env("HOSTNAME", HOST);
    let root = world.root.join("not-a-directory");
    std::fs::write(&root, "a file where the runs root should be").expect("a file");
    let mut json = world.cmd(&["host", "--json"]);
    json.env("ONEPIPELINE_RUNS_DIR", &root);
    let reading = invoke(json).reading();
    has_the_version_1_keys(&reading);
    assert_eq!(reading["root"], json!(root.to_string_lossy()), "{reading}");
    assert!(
        reading["root_error"]
            .as_str()
            .is_some_and(|why| why.starts_with("the runs root cannot be read")),
        "{reading}"
    );
    assert_eq!(reading["skipped"], json!([]), "{reading}");
    assert_eq!(reading["dispatches"], json!([]), "{reading}");
    let mut text = world.cmd(&["host"]);
    text.env("ONEPIPELINE_RUNS_DIR", &root);
    let text = invoke(text);
    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert!(
        text.stdout.contains("1 run root(s) skipped:")
            && text
                .stdout
                .contains("not-a-directory: the runs root cannot be read"),
        "{}",
        text.stdout
    );
}
