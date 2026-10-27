//! One measurement of the release-built driver adopting a generated run.
//!
//! Everything it reads it reads from outside the driver, so the build under test is
//! measured unmodified: the journal and the command-outcomes queue as they grow (every
//! line's arrival instant, polled), `liveness.json` as it is overwritten, the driver's
//! `/proc` counters (CPU and the resident high-water mark), and the `loop-stats.json`
//! the driver writes at every wait when `ONEPIPELINE_LOOP_STATS` is set.

// llmlint: ignore-file[contracts_have_one_source_or_a_drift_gate] the files read here —
// the journal, the outcomes queue, `liveness.json`, `loop-stats.json`, members'
// histories and reply receipts — are read from outside the measured process because the
// release build is measured unmodified, and their types are private to the engine and to
// `oneagentgraph`; the reply envelope is the one `docs/contract.md` states. Every read
// fails the measurement, naming what it found, on a line or document that is not the
// shape it expects, and a measurement that finds no heartbeat, no activity or no outcome
// for an answered reply fails too, so a shape that drifts fails the `workload` target the
// release sweep runs rather than recording a figure of nothing. `CHANNEL_POLL_MS` only
// corrects the reconcile-pass detail, the same at every size.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::generate::{self, LIVE_NODES};
use crate::harness::World;
use crate::{checkpoint, world_at, RUN};
use onepipeline_testfakes::ACTIVITY_MARK;

/// The `liveness.json` schema this measurement reads (`src/heartbeat.rs`'s
/// `LIVENESS_SCHEMA_VERSION`, private to the engine): a document at another fails the
/// measurement rather than being read as no heartbeats.
const LIVENESS_SCHEMA_VERSION: u64 = 1;
/// How often the journal, the queue and the driver are looked at.
const POLL: Duration = Duration::from_millis(25);
/// The engine's wait between looks at its channel (`src/engine.rs`'s `CHANNEL_POLL`): an
/// interval between two loop-stats flushes holds the passes in it and up to one of these.
const CHANNEL_POLL_MS: f64 = 200.0;
/// Twenty amends, one every fifteen seconds, as the workload states.
const ENVELOPES: usize = 20;
const ENVELOPE_EVERY: Duration = Duration::from_secs(15);
/// The window with nothing dispatched.
const IDLE: Duration = Duration::from_secs(60);
/// The one bound on any phase: a guard against a hang, never a figure. A phase the
/// driver has not finished within it fails the journey, naming that phase.
const PATIENCE: Duration = Duration::from_secs(3 * 3600);

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// Wait for `ready`, failing past [`PATIENCE`] with what was being waited for; how long
/// it took.
fn until(what: &str, mut ready: impl FnMut() -> bool) -> f64 {
    let began = Instant::now();
    while !ready() {
        assert!(
            began.elapsed() < PATIENCE,
            "{what}: not within {} s",
            PATIENCE.as_secs()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    began.elapsed().as_secs_f64()
}

/// p50, p95 and max of `values` (nearest rank), with their count.
pub fn percentiles(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({"n": 0});
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| {
        let rank = (q * sorted.len() as f64).ceil() as usize;
        sorted[rank.clamp(1, sorted.len()) - 1]
    };
    json!({"n": sorted.len(), "p50": at(0.5), "p95": at(0.95), "max": sorted[sorted.len() - 1]})
}

/// One line of a tailed file, as the measurement saw it arrive.
#[derive(Clone, Debug)]
pub struct Seen {
    pub at_ms: i64,
    pub bytes: u64,
    pub kind: String,
    pub node: Option<String>,
    /// `(ordinal, emitted ms, emitting pid)` of a `fake-claude` `turn.activity` block.
    pub marker: Option<(u64, i64, u64)>,
    /// A command outcome's envelope id.
    pub id: Option<u64>,
    /// A turn-activity record's transcript reference: the member's history file, the
    /// history run it names and the event's index in it.
    pub transcript: Option<(PathBuf, String, u64)>,
}

/// The `(ordinal, emitted ms, pid)` that follow an [`ACTIVITY_MARK`] in `said`, failing
/// on a mark whose three numbers are not there: what the double wrote and what this
/// reads disagree, and every delay measured from it would be wrong.
fn mark_in(said: &str) -> Option<(u64, i64, u64)> {
    let at = said.find(ACTIVITY_MARK)?;
    let rest = &said[at + ACTIVITY_MARK.len()..];
    let mut words = rest
        .split(|c: char| !c.is_ascii_digit())
        .filter(|word| !word.is_empty());
    let mut next = || -> u64 {
        words
            .next()
            .and_then(|word| word.parse().ok())
            .unwrap_or_else(|| panic!("an activity mark without its three numbers: {said:.200}"))
    };
    let (nth, emitted, pid) = (next(), next(), next());
    Some((nth, i64::try_from(emitted).unwrap(), pid))
}

/// One complete line of the journal or the command-outcomes queue, both of which hold
/// one JSON object per line with a `kind` or an `id`: a line that is neither fails the
/// measurement, naming it, rather than being counted as nothing.
fn parse_line(line: &[u8], at_ms: i64) -> Seen {
    let value: Value = serde_json::from_slice(line).unwrap_or_else(|error| {
        panic!(
            "a line that is not JSON ({error}): {:.200}",
            String::from_utf8_lossy(line)
        )
    });
    assert!(
        value["kind"].is_string() || value["id"].is_u64(),
        "a line with neither a kind nor an id: {:.200}",
        String::from_utf8_lossy(line)
    );
    let text = |v: &Value| v.as_str().map(str::to_owned);
    let marker = ["output", "detail"]
        .iter()
        .find_map(|field| value["payload"][field].as_str())
        .and_then(mark_in);
    Seen {
        at_ms,
        bytes: line.len() as u64,
        kind: text(&value["kind"]).unwrap_or_default(),
        node: text(&value["labels"]["onepipeline.node"]).or_else(|| text(&value["labels"]["node"])),
        marker,
        id: value["id"].as_u64(),
        transcript: transcript_of(&value["payload"]["transcript"]),
    }
}

/// Where a turn-activity record's content is, as `oneagentgraph`'s own transcript reader
/// finds it: `<history_dir>/<history_project>/<history_session>.jsonl`, the event of
/// that history run at that index.
fn transcript_of(reference: &Value) -> Option<(PathBuf, String, u64)> {
    let part = |name: &str| reference[name].as_str();
    let file = Path::new(part("history_dir")?)
        .join(part("history_project")?)
        .join(format!("{}.jsonl", part("history_session")?));
    let history_id = match &reference["history_id"] {
        Value::String(id) => id.clone(),
        other => other.to_string(),
    };
    Some((file, history_id, reference["index"].as_u64()?))
}

/// The `turn.activity` mark of each journalled record whose content is in a member's
/// history rather than in the record: `(ordinal, emitted ms, pid)` by the record's
/// transcript reference. Each history file is read once.
fn marks_in_histories(lines: &[Seen]) -> BTreeMap<(PathBuf, String, u64), (u64, i64, u64)> {
    let files: std::collections::BTreeSet<&PathBuf> = lines
        .iter()
        .filter_map(|s| s.transcript.as_ref().map(|(file, _, _)| file))
        .collect();
    let mut marks = BTreeMap::new();
    for file in files {
        // A journalled record names this file, so it exists; it is read once its
        // members have stopped, so every line is whole.
        let text = std::fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        for line in text.lines() {
            let value: Value = serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("{}: a line that is not JSON ({error})", file.display())
            });
            let event = value["event"].to_string();
            let Some(mark) = mark_in(&event) else {
                continue;
            };
            let (Some(run), Some(index)) =
                (value["run_id"].as_str(), value["event"]["index"].as_u64())
            else {
                panic!(
                    "{}: an activity event with no run_id or event.index",
                    file.display()
                );
            };
            marks.insert((file.clone(), run.to_owned(), index), mark);
        }
    }
    marks
}

/// Tails a file from where it ends now, stamping each complete line on arrival.
pub struct Watcher {
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Watcher {
    pub fn start(path: PathBuf) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen2, stop2) = (seen.clone(), stop.clone());
        let mut offset = std::fs::metadata(&path).map_or(0, |m| m.len());
        let handle = std::thread::spawn(move || {
            let mut partial: Vec<u8> = Vec::new();
            let mut buffer = vec![0u8; 1 << 20];
            while !stop2.load(Ordering::Relaxed) {
                if let Ok(mut file) = std::fs::File::open(&path) {
                    if file.seek(SeekFrom::Start(offset)).is_ok() {
                        loop {
                            let read = file.read(&mut buffer).unwrap_or(0);
                            if read == 0 {
                                break;
                            }
                            offset += read as u64;
                            let at_ms = now_ms();
                            partial.extend_from_slice(&buffer[..read]);
                            let mut seen = seen2.lock().unwrap();
                            while let Some(end) = partial.iter().position(|&b| b == b'\n') {
                                let line: Vec<u8> = partial.drain(..=end).collect();
                                seen.push(parse_line(&line, at_ms));
                            }
                        }
                    }
                }
                std::thread::sleep(POLL);
            }
        });
        Self {
            seen,
            stop,
            handle: Some(handle),
        }
    }

    pub fn count(&self, pick: impl Fn(&Seen) -> bool) -> usize {
        self.seen.lock().unwrap().iter().filter(|s| pick(s)).count()
    }

    pub fn finish(mut self) -> Vec<Seen> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
        self.seen.lock().unwrap().clone()
    }
}

/// One look at the driver.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub at_ms: i64,
    /// utime + stime of the whole process, in clock ticks.
    pub cpu_ticks: u64,
    pub hwm_kb: u64,
    /// The loop-stats document's pass count, where its file changed since the last look,
    /// with the instant it was written.
    pub flush: Option<(i64, u64)>,
}

/// What `liveness.json` was seen to say: every distinct beat of a member of one of the
/// live nodes' dispatches, with the instant it was first seen, and the file's largest size.
#[derive(Default)]
struct Liveness {
    beats: BTreeMap<(String, String, String, String), i64>,
    largest: u64,
}

pub struct Sampler {
    samples: Arc<Mutex<Vec<Sample>>>,
    liveness: Arc<Mutex<Liveness>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

/// utime + stime of a `/proc` `stat` line: the 14th and 15th fields, counted after the
/// parenthesised command. A line without them is not a `stat` line, and fails.
fn ticks_of(stat: &str) -> u64 {
    let field = |nth: usize| -> u64 {
        stat.rsplit_once(") ")
            .and_then(|(_, after)| after.split_whitespace().nth(nth))
            .and_then(|ticks| ticks.parse().ok())
            .unwrap_or_else(|| panic!("a /proc stat line without field {}: {stat:.200}", nth + 3))
    };
    field(11) + field(12)
}

/// A `/proc` `status` document's kB figure for `key`: none where the kernel no longer
/// reports it, which is a process that has exited and not yet been reaped, and a failure
/// where the line is there but holds no number.
fn status_kb(status: &str, key: &str) -> Option<u64> {
    let line = status.lines().find(|line| line.starts_with(key))?;
    Some(
        line.split_whitespace()
            .nth(1)
            .and_then(|kb| kb.parse().ok())
            .unwrap_or_else(|| panic!("/proc status: {line}")),
    )
}

impl Sampler {
    pub fn start(pid: u32, run: PathBuf) -> Self {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let liveness = Arc::new(Mutex::new(Liveness::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (samples2, liveness2, stop2) = (samples.clone(), liveness.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            let stats = run.join("loop-stats.json");
            let beats = run.join("liveness.json");
            let mut last_mtime = 0i64;
            while !stop2.load(Ordering::Relaxed) {
                let at_ms = now_ms();
                let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                    break;
                };
                let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
                    break;
                };
                // The driver has exited: what it held is already sampled.
                let Some(hwm_kb) = status_kb(&status, "VmHWM:") else {
                    break;
                };
                let mut sample = Sample {
                    at_ms,
                    cpu_ticks: ticks_of(&stat),
                    hwm_kb,
                    flush: None,
                };
                if let Ok(meta) = std::fs::metadata(&stats) {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |d| i64::try_from(d.as_micros()).unwrap());
                    // The driver replaces the file whole, so what is read is one
                    // document; one without its pass count fails the measurement.
                    if mtime != last_mtime {
                        if let Ok(text) = std::fs::read_to_string(&stats) {
                            let value: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
                                panic!("loop-stats.json is not JSON ({e}): {text:.200}")
                            });
                            let passes = value["passes"].as_u64().unwrap_or_else(|| {
                                panic!("loop-stats.json has no pass count: {text:.200}")
                            });
                            last_mtime = mtime;
                            sample.flush = Some((mtime / 1000, passes));
                        }
                    }
                }
                // Overwritten whole and atomically by the driver, so what is read
                // is one document of the schema it was written at, or the
                // measurement fails naming what it found.
                if let Ok(text) = std::fs::read(&beats) {
                    let value: Value = serde_json::from_slice(&text)
                        .unwrap_or_else(|e| panic!("liveness.json is not JSON ({e})"));
                    assert_eq!(
                        value["schema_version"], LIVENESS_SCHEMA_VERSION,
                        "liveness.json is at a schema this measurement does not read"
                    );
                    let members = value["members"]
                        .as_array()
                        .unwrap_or_else(|| panic!("liveness.json has no members list"));
                    let mut seen = liveness2.lock().unwrap();
                    seen.largest = seen.largest.max(text.len() as u64);
                    for beat in members {
                        let field = |name: &str| {
                            beat[name].as_str().map(str::to_owned).unwrap_or_else(|| {
                                panic!("a liveness.json member without {name}: {beat}")
                            })
                        };
                        let key = (
                            field("node"),
                            field("graph_run"),
                            field("member"),
                            field("last_heartbeat_at"),
                        );
                        if LIVE_NODES.contains(&key.0.as_str()) {
                            seen.beats.entry(key).or_insert(at_ms);
                        }
                    }
                }
                samples2.lock().unwrap().push(sample);
                std::thread::sleep(POLL);
            }
        });
        Self {
            samples,
            liveness,
            stop,
            handle: Some(handle),
        }
    }

    fn passed(&self) -> bool {
        self.samples
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.flush.is_some_and(|(_, passes)| passes > 0))
    }

    fn finish(mut self) -> (Vec<Sample>, Liveness) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
        let samples = self.samples.lock().unwrap().clone();
        let liveness = std::mem::take(&mut *self.liveness.lock().unwrap());
        (samples, liveness)
    }
}

/// Each of the driver's threads' CPU ticks, by its name and id.
fn thread_ticks(pid: u32) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(format!("/proc/{pid}/task"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let name = stat
            .split_once(" (")
            .and_then(|(_, rest)| rest.rsplit_once(") "))
            .map_or("?", |(name, _)| name);
        let id = entry.file_name().to_string_lossy().into_owned();
        out.insert(format!("{name}/{id}"), ticks_of(&stat));
    }
    out
}

/// The CPU seconds each thread spent between two [`thread_ticks`] readings, busiest
/// first, the idle ones left out.
fn thread_cpu(before: &BTreeMap<String, u64>, after: &BTreeMap<String, u64>, tick: f64) -> Value {
    let mut spent: Vec<(String, f64)> = after
        .iter()
        .map(|(thread, ticks)| {
            let from = before.get(thread).copied().unwrap_or(0);
            (thread.clone(), ticks.saturating_sub(from) as f64 / tick)
        })
        .filter(|(_, seconds)| *seconds > 0.0)
        .collect();
    spent.sort_by(|a, b| b.1.total_cmp(&a.1));
    Value::Array(
        spent
            .into_iter()
            .map(|(thread, seconds)| json!([thread, seconds]))
            .collect(),
    )
}

/// Clock ticks per second, as `/proc` counts CPU time.
fn ticks_per_second() -> f64 {
    // SAFETY: sysconf reads a constant of the running system.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 {
        ticks as f64
    } else {
        100.0
    }
}

/// The reconcile passes the loop-stats flushes inside `[from, to)` bracket.
///
/// The driver flushes at every wait, so two consecutive flushes bracket the passes run
/// between them and at most one channel wait: one pass's time is the interval, less one
/// [`CHANNEL_POLL_MS`], over the passes it holds. Its CPU is the whole process's ticks
/// over that interval.
fn passes(samples: &[Sample], from: i64, to: i64, tick: f64) -> Value {
    let flushes: Vec<(i64, u64, u64)> = samples
        .iter()
        .filter_map(|s| s.flush.map(|(at, passes)| (at, passes, s.cpu_ticks)))
        .collect();
    let mut walls = Vec::new();
    let mut count = 0u64;
    let mut cpu_ticks = 0u64;
    for pair in flushes.windows(2) {
        let ((t0, n0, c0), (t1, n1, c1)) = (pair[0], pair[1]);
        let n = n1.saturating_sub(n0);
        if t1 < from || t1 >= to || n == 0 {
            continue;
        }
        count += n;
        cpu_ticks += c1.saturating_sub(c0);
        walls.push(((t1 - t0) as f64 - CHANNEL_POLL_MS).max(0.0) / n as f64);
    }
    json!({
        "count": count,
        "ms": percentiles(&walls),
        "cpu_s_per_pass": if count == 0 { 0.0 } else { cpu_ticks as f64 / tick / count as f64 },
    })
}

/// CPU seconds the driver spent over `[from, to]`, from the samples at either end.
fn cpu_between(samples: &[Sample], from: i64, to: i64, tick: f64) -> f64 {
    let at = |t: i64| {
        samples
            .iter()
            .rev()
            .find(|s| s.at_ms <= t)
            .or(samples.first())
    };
    match (at(from), at(to)) {
        (Some(a), Some(b)) => b.cpu_ticks.saturating_sub(a.cpu_ticks) as f64 / tick,
        _ => 0.0,
    }
}

fn is_live(node: Option<&str>) -> bool {
    node.is_some_and(|node| LIVE_NODES.contains(&node))
}

/// A liveness kind: a record the engine writes for a member's liveness rather than for
/// the run's history.
fn is_liveness(kind: &str) -> bool {
    kind.contains("heartbeat") || kind.contains("liveness")
}

/// A reply's answer: when it came, the process's exit, and its stdout.
type Answer = (i64, Option<i32>, String);

/// A `reply` submitted, and its answer once it has one.
struct Reply {
    submitted_ms: i64,
    answer: Arc<Mutex<Option<Answer>>>,
}

fn submit(world: &World, binary: &Path, commands: &Value) -> Reply {
    let mut command = world.agentgraph_cmd_on(binary, &["reply", RUN]);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let submitted_ms = now_ms();
    let mut child = command.spawn().expect("onepipeline reply starts");
    let envelope = json!({"version": 3, "commands": commands}).to_string();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(envelope.as_bytes())
        .unwrap();
    let answer = Arc::new(Mutex::new(None));
    let answer2 = answer.clone();
    std::thread::spawn(move || {
        let output = child.wait_with_output().unwrap();
        *answer2.lock().unwrap() = Some((
            now_ms(),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ));
    });
    Reply {
        submitted_ms,
        answer,
    }
}

fn amend_text(rng: &mut generate::Rng, node: &str, nth: usize) -> String {
    format!(
        "## What\nDo {node}, as amended {nth}.\n{}\n\n## Acceptance criteria\n- {node} is done.",
        generate::prose(rng, 2_800)
    )
}

/// The envelope id an answered reply's receipt names under `reply`
/// (`docs/contract-divergences.md` entry 64): a receipt that is not that object fails.
fn receipt_id(stdout: &str) -> u64 {
    let receipt: Value = serde_json::from_str(stdout).unwrap_or_else(|error| {
        panic!("a reply receipt that is not JSON ({error}): {stdout:.200}")
    });
    receipt["reply"]
        .as_u64()
        .unwrap_or_else(|| panic!("a reply receipt with no reply id: {stdout:.200}"))
}

/// Every turn-activity block the four workers emitted, as the double recorded it:
/// `(pid, ordinal) -> emitted ms`.
fn emitted(world: &World) -> BTreeMap<(u64, u64), i64> {
    let path = world.fakes.join("activity-emitted.txt");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let blocks: BTreeMap<(u64, u64), i64> = text
        .lines()
        .map(|line| {
            let (nth, at, pid) = mark_in(&format!("{ACTIVITY_MARK} {line}"))
                .unwrap_or_else(|| unreachable!("every line is given the mark"));
            ((pid, nth), at)
        })
        .collect();
    assert!(!blocks.is_empty(), "the four workers emitted no activity");
    blocks
}

/// The measurement at one journal size: a run of `size_mb` MB generated from `seed` at
/// `root`, adopted by `binary`, its four dispatches working while twenty amends are
/// replied, then a minute with nothing dispatched. Fails, naming the phase, where the
/// driver does not finish one.
pub fn measure(binary: &Path, root: &Path, size_mb: u64, seed: u64) -> Value {
    let tick = ticks_per_second();
    let (world, paths, plan) = world_at(root, seed);
    let generated = generate::generate(&paths, RUN, &plan, size_mb * 1_000_000, seed);
    checkpoint(&world, binary);
    let checkpoint_doc: Value =
        serde_json::from_slice(&std::fs::read(world.run_file(RUN, "checkpoint.json")).unwrap())
            .unwrap();
    assert!(
        checkpoint_doc["schema_version"].is_u64(),
        "the checkpoint the build wrote names no schema_version"
    );
    let checkpoint_bytes = std::fs::metadata(world.run_file(RUN, "checkpoint.json"))
        .unwrap()
        .len();

    for (name, body) in [
        ("turn.hold", "hold"),
        ("turn.activity", "1000"),
        ("observer.wait", "hold"),
    ] {
        world.script(name, body);
    }
    let run_dir = world.run_file(RUN, "");
    let journal = Watcher::start(world.run_file(RUN, "events.jsonl"));
    let outcomes = Watcher::start(world.run_file(RUN, "channel/command-outcomes.jsonl"));
    let driver_log = std::fs::File::create(world.root.join("adopt.log")).unwrap();
    let mut adopt = world.agentgraph_cmd_on(binary, &["adopt", RUN]);
    adopt
        .stdin(Stdio::null())
        .stdout(driver_log.try_clone().unwrap())
        .stderr(driver_log);
    let spawned_ms = now_ms();
    let mut driver = adopt.spawn().expect("onepipeline adopt starts");
    let sampler = Sampler::start(driver.id(), run_dir.clone());

    until("the adopting driver's first finished pass", || {
        sampler.passed()
    });
    let first_pass_ms = now_ms();
    until("the four live nodes' turns", || {
        journal.count(|s| s.kind == "turn-started" && is_live(s.node.as_deref()))
            >= LIVE_NODES.len()
    });
    let dispatched_ms = now_ms();

    // Twenty amends of a running node, one every fifteen seconds, each its own reply.
    let mut rng = generate::Rng::new(seed ^ 0x00A1_1E4D);
    let envelopes_from = now_ms();
    let mut replies = Vec::new();
    for index in 0..ENVELOPES {
        let due = envelopes_from + index as i64 * ENVELOPE_EVERY.as_millis() as i64;
        while now_ms() < due {
            std::thread::sleep(Duration::from_millis(20));
        }
        let text = amend_text(&mut rng, LIVE_NODES[0], index + 1);
        replies.push(submit(
            &world,
            binary,
            &json!([{"op": "amend", "id": LIVE_NODES[0], "text": text}]),
        ));
    }
    until("every reply's answer", || {
        replies.iter().all(|r| r.answer.lock().unwrap().is_some())
    });

    // Release the four turns and wait for their nodes to settle: then nothing is
    // dispatched, and only the observer's members work.
    world.release("turn.go");
    world.release("turn.settle");
    let released_ms = now_ms();
    until("the four live nodes to settle", || {
        journal.count(|s| s.kind == "node-settled" && is_live(s.node.as_deref()))
            >= LIVE_NODES.len()
    });
    let idle_from = now_ms();
    let idle_threads_from = thread_ticks(driver.id());
    std::thread::sleep(IDLE);
    let idle_to = now_ms();
    let idle_threads = thread_cpu(&idle_threads_from, &thread_ticks(driver.id()), tick);

    // Stopped as an operator stops it. The observer's turns are still held, so the stop
    // may report part of the tree still running; what it says is recorded, and the
    // world's teardown ends whatever it left.
    let stopped = world
        .agentgraph_cmd_on(binary, &["stop", RUN])
        .output()
        .unwrap();
    let began = Instant::now();
    while driver.try_wait().ok().flatten().is_none() && began.elapsed() < Duration::from_secs(600) {
        std::thread::sleep(Duration::from_millis(100));
    }
    let (samples, liveness) = sampler.finish();
    let lines = journal.finish();
    let outcome_lines = outcomes.finish();
    world.release("observer.go");
    // What is left of the run is ended here by the build under test, and its launch
    // record removed, so the world's teardown finds no run of its own to stop: its
    // `stop --force` is the unoptimized build's, which takes many minutes over an
    // 800 MB journal. The teardown still ends any process this one left.
    let _ = world
        .agentgraph_cmd_on(binary, &["stop", RUN, "--force"])
        .output();
    let _ = std::fs::remove_file(world.run_file(RUN, "launch.json"));

    // A worker's turn-activity block waits from the instant the double stamped into it
    // to its line's arrival in the journal. Every block emitted is counted, and one never
    // journalled has waited at least until the measurement ended.
    let ended_ms = now_ms();
    let referenced = marks_in_histories(&lines);
    let journalled: BTreeMap<(u64, u64), i64> = lines
        .iter()
        .filter_map(|s| {
            s.marker
                .or_else(|| {
                    s.transcript
                        .as_ref()
                        .and_then(|key| referenced.get(key).copied())
                })
                .map(|(nth, _, pid)| ((pid, nth), s.at_ms))
        })
        .collect();
    let blocks = emitted(&world);
    let delays: Vec<f64> = blocks
        .iter()
        .map(|(key, at)| (journalled.get(key).copied().unwrap_or(ended_ms) - at) as f64)
        .collect();
    let never_journalled = blocks
        .keys()
        .filter(|key| !journalled.contains_key(key))
        .count();

    // A reply waits from its submission to its answer; its envelope's outcome is the
    // line naming its id in the command-outcomes queue.
    let outcome_at: BTreeMap<u64, i64> = outcome_lines
        .iter()
        .filter_map(|s| s.id.map(|id| (id, s.at_ms)))
        .collect();
    let mut latencies = Vec::new();
    let mut to_outcome = Vec::new();
    let mut exits = BTreeMap::<String, usize>::new();
    for reply in &replies {
        let answer = reply.answer.lock().unwrap();
        let (answered_ms, code, stdout) = answer.as_ref().unwrap();
        // A refused amend answers fast and measures nothing the budget is about.
        assert_eq!(*code, Some(0), "a reply was refused: {stdout:.200}");
        latencies.push((answered_ms - reply.submitted_ms) as f64);
        *exits.entry(format!("{code:?}")).or_default() += 1;
        let id = receipt_id(stdout);
        let at = outcome_at
            .get(&id)
            .unwrap_or_else(|| panic!("reply {id} was answered with no outcome in the queue"));
        to_outcome.push((at - reply.submitted_ms) as f64);
    }

    // Liveness over the working phase: what the journal grew by, by kind, against the
    // heartbeats the four workers sent, seen in `liveness.json` or in the journal.
    let working = |s: &Seen| s.at_ms >= dispatched_ms && s.at_ms < released_ms;
    let mut kinds = BTreeMap::<String, (u64, u64)>::new();
    for s in lines.iter().filter(|s| working(s)) {
        let entry = kinds.entry(s.kind.clone()).or_default();
        entry.0 += 1;
        entry.1 += s.bytes;
    }
    let liveness_bytes: u64 = kinds
        .iter()
        .filter(|(kind, _)| is_liveness(kind))
        .map(|(_, (_, bytes))| bytes)
        .sum();
    let other_bytes: u64 = kinds
        .iter()
        .filter(|(kind, _)| !is_liveness(kind))
        .map(|(_, (_, bytes))| bytes)
        .sum();
    let beats_kept = liveness
        .beats
        .values()
        .filter(|at| (dispatched_ms..released_ms).contains(at))
        .count();
    let beats_journalled = lines
        .iter()
        .filter(|s| working(s) && s.kind == "member-heartbeat" && is_live(s.node.as_deref()))
        .count();
    let heartbeats = beats_kept.max(beats_journalled);
    // No heartbeat seen anywhere is a reading that has stopped finding them, never a
    // liveness figure of nothing.
    assert!(
        heartbeats > 0,
        "the four workers' heartbeats were seen nowhere"
    );
    let per_member_hour = if heartbeats == 0 {
        0.0
    } else {
        liveness_bytes as f64 / heartbeats as f64 * 240.0
    };

    let idle_seconds = (idle_to - idle_from) as f64 / 1000.0;
    let idle_cpu = cpu_between(&samples, idle_from, idle_to, tick);
    let idle_dispatched = lines
        .iter()
        .filter(|s| s.at_ms >= idle_from && s.at_ms < idle_to && s.kind == "node-dispatched")
        .count();
    // The kernel counts VmHWM in KiB; the budget is in megabytes of 1,000,000 bytes.
    let peak_kb = samples.iter().map(|s| s.hwm_kb).max().unwrap_or(0);

    json!({
        "size_mb": size_mb,
        "journal": {
            "bytes": generated.bytes,
            "records": generated.records,
            "envelopes_before": generated.envelopes,
            "episodes": generated.episodes,
            "stalls": generated.stalls.len(),
            "history": [generated.history.0, generated.history.1],
            "checkpoint_bytes": checkpoint_bytes,
            "checkpoint_schema": checkpoint_doc["schema_version"],
        },
        "to_first_pass_s": (first_pass_ms - spawned_ms) as f64 / 1000.0,
        "to_four_dispatches_s": (dispatched_ms - spawned_ms) as f64 / 1000.0,
        "pass": passes(&samples, spawned_ms, idle_to, tick),
        "event_delay_ms": percentiles(&delays),
        "activity": {"emitted": blocks.len(), "never_journalled": never_journalled},
        "envelope_latency_ms": percentiles(&latencies),
        "envelope_outcome_ms": percentiles(&to_outcome),
        "envelope_exits": exits,
        "idle": {
            "seconds": idle_seconds,
            "cpu_s": idle_cpu,
            "share": idle_cpu / idle_seconds,
            "dispatched": idle_dispatched,
            "threads_cpu_s": idle_threads,
            "pass": passes(&samples, idle_from, idle_to, tick),
        },
        "peak_rss_mb": (peak_kb * 1024) as f64 / 1_000_000.0,
        "liveness": {
            "heartbeats": heartbeats,
            "heartbeats_in_liveness_json": beats_kept,
            "heartbeats_journalled": beats_journalled,
            "kinds": kinds.iter()
                .filter(|(kind, _)| is_liveness(kind))
                .map(|(kind, (n, b))| (kind.clone(), json!({"count": n, "bytes": b})))
                .collect::<serde_json::Map<_, _>>(),
            "liveness_bytes": liveness_bytes,
            "other_bytes": other_bytes,
            "liveness_json_bytes": liveness.largest,
            "bytes_per_member_hour": per_member_hour,
        },
        "phases_s": {
            "envelopes": (released_ms - envelopes_from) as f64 / 1000.0,
            "working": (released_ms - dispatched_ms) as f64 / 1000.0,
            "settle": (idle_from - released_ms) as f64 / 1000.0,
        },
        "stop_exit": stopped.status.code(),
        "driver_exit": driver.try_wait().ok().flatten().map(|status| status.to_string()),
    })
}
