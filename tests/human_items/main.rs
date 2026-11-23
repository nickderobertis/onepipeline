//! The human-items read journeys over the workload [`generate`] makes, which
//! [`fixture`] generates once and reuses:
//!
//! - **counted** (`counted_human_items_reads`, the `human-items-e2e:test` target a pull
//!   request runs): one priming read, then one warm read of each of `list --json`,
//!   `list`, `report` and `show --json` of the item with the longest history, each held
//!   to the exact counts its telemetry and `strace` report — run records and item events
//!   replayed, conditions evaluated, checks run and files read. It times nothing.
//! - **timed** (`timed_human_items_reads`, the `human-items-e2e:timed` target, on a push
//!   to main alone): the same plan of reads with ten warm rounds of each form, timed as a
//!   subprocess of the release-built binary and written to
//!   `target/budget-records/human-items.json`, which `report.mjs` reads.
//! - the fixture's lifecycle and the index's recoveries, over a small fixture
//!   (`lifecycle.rs`).
//!
//! Both journeys read only through [`plan`], and the counted one asserts that the reads
//! it made, as the binary's own telemetry records them, are that plan's and no more.
//! The priming read is the one read neither counts nor times: the first read of an
//! unpacked fixture rebuilds what is derived from its files, and runs any check a day old.
//!
//! Every read sweeps the items' expiry conditions first, and each record carries the
//! sweep's time and the number of conditions it read.

#![cfg(target_os = "linux")]

mod fixture;
mod generate;
#[path = "../e2e/harness.rs"]
mod harness;
mod lifecycle;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use fixture::{Prepared, Spec};
use generate::{Scale, Shape};
use harness::World;
use serde_json::{json, Value};

/// The forms every warm read is one of, in the order both journeys make them.
pub const FORMS: [&str; 4] = ["list-json", "list", "report", "show-json"];

/// The reads a journey makes after its fixture is prepared: the priming read, then
/// `rounds` warm reads of each form.
pub fn plan(rounds: usize) -> Vec<&'static str> {
    let mut reads = vec!["priming"];
    for form in FORMS {
        reads.extend(std::iter::repeat_n(form, rounds));
    }
    reads
}

/// What one warm read did, read off its telemetry and its trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Run journal records folded.
    pub run_records: u64,
    /// Item journal events folded.
    pub item_events: u64,
    /// Expiry conditions the sweep read.
    pub conditions: u64,
    /// Checks the sweep ran.
    pub checks: u64,
    /// Files and directories the binary opened to read under the fixture's root.
    pub files: u64,
}

const fn counts(conditions: u64, files: u64) -> Counts {
    Counts {
        run_records: 0,
        item_events: 0,
        conditions,
        checks: 0,
        files,
    }
}

/// Each warm read's counts over the full fixture. A change that moves one updates it
/// here, in its own diff; never loosen one to a bound.
///
/// The files are the fixture's own, the same on every host: the configuration, the
/// item store's index, journal (opened twice) and check record (twice), the runs root,
/// and each run's journal; `list` and `report` then list the runs root again and read
/// each run's launch record, so 7 + runs and 8 + 2 × runs.
pub const EXPECTED_FULL: [(&str, Counts); 4] = [
    ("list-json", counts(140, 107)),
    ("list", counts(140, 208)),
    ("report", counts(140, 208)),
    ("show-json", counts(140, 107)),
];

/// The same over [`Scale::SMALL`], which the lifecycle test restores and reads.
pub const EXPECTED_SMALL: [(&str, Counts); 4] = [
    ("list-json", counts(14, 9)),
    ("list", counts(14, 12)),
    ("report", counts(14, 12)),
    ("show-json", counts(14, 9)),
];

/// The debug-built binary this test was built with.
fn built() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_onepipeline"))
}

/// The release-built binary the timed journey times: `HUMAN_ITEMS_BINARY`, which the
/// `timed` target builds and names.
fn release() -> PathBuf {
    let binary = std::env::var_os("HUMAN_ITEMS_BINARY")
        .map(PathBuf::from)
        .expect("HUMAN_ITEMS_BINARY names the release-built binary; run `just human-items-timed`");
    assert!(binary.is_file(), "{} is not a file", binary.display());
    binary
}

/// One read's output, what the binary recorded of it, and `measured`: what the journey
/// took of it from outside, which is the files a counted read opened ([`Journey::count`]),
/// the seconds a timed one took ([`Journey::time`]) and nothing for the priming read.
pub struct Sample<M> {
    pub stdout: String,
    pub read: Value,
    pub process: Value,
    pub measured: M,
}

thread_local! {
    static READS_TIMED: Cell<usize> = const { Cell::new(0) };
}

/// The reads this thread has timed. Each test runs on its own thread, so the counting
/// journey can hold itself to none while a timed test runs beside it.
pub fn reads_timed() -> usize {
    READS_TIMED.with(Cell::get)
}

/// Run `command` and the seconds it took: the one place a read is timed.
fn stopwatch(command: &mut Command) -> (Output, f64) {
    READS_TIMED.with(|n| n.set(n.get() + 1));
    let began = Instant::now();
    let output = command.output().unwrap();
    (output, began.elapsed().as_secs_f64())
}

/// The reads of one journey, each through one binary, each recorded by the binary in
/// one telemetry file so what it made can be checked against [`plan`].
pub struct Journey<'a> {
    world: &'a World,
    binary: PathBuf,
    telemetry: PathBuf,
    made: Vec<&'static str>,
    longest: String,
}

impl<'a> Journey<'a> {
    pub fn new(world: &'a World, binary: PathBuf) -> Self {
        let telemetry = world.root.join("journey-telemetry.ndjson");
        let _ = std::fs::remove_file(&telemetry);
        Self {
            world,
            binary,
            telemetry,
            made: Vec::new(),
            longest: fixture::item_id(generate::LONGEST_KEY),
        }
    }

    fn args(&self, read: &str) -> Vec<String> {
        let words: &[&str] = match read {
            "priming" | "list-json" => &["list", "--json"],
            "list" => &["list"],
            "report" => &["report"],
            "show-json" => &["show", &self.longest, "--json"],
            other => panic!("`{other}` is no read of the plan"),
        };
        std::iter::once("human-items")
            .chain(words.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    /// Make the read `read`, measured by `measure`, which runs the command.
    fn run<M>(
        &mut self,
        read: &'static str,
        measure: impl FnOnce(&mut Command) -> (Output, M),
    ) -> Sample<M> {
        let args = self.args(read);
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut command = self.world.cmd_on(&self.binary, &words);
        command.env("ONEPIPELINE_HUMAN_ITEMS_TELEMETRY", &self.telemetry);
        let before = telemetry(&self.telemetry).len();
        let (output, measured) = measure(&mut command);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.made.push(read);
        let lines = telemetry(&self.telemetry);
        let new = &lines[before..];
        let read_line = new
            .iter()
            .find_map(|l| l.get("read").cloned())
            .unwrap_or_else(|| panic!("{read}: the CLI recorded no read telemetry"));
        let process = new
            .iter()
            .find_map(|l| l.get("process").cloned())
            .unwrap_or(Value::Null);
        Sample {
            stdout: String::from_utf8(output.stdout).unwrap(),
            read: read_line,
            process,
            measured,
        }
    }

    /// Make the read `read` under `strace`, writing its trace to `trace`, measured by the
    /// files it opened and never by a clock.
    pub fn count(&mut self, read: &'static str, trace: &Path) -> Sample<u64> {
        let root = self.world.root.clone();
        self.run(read, |command| traced(command, trace, &root))
    }

    /// Make the read `read` measured by the seconds it took as a subprocess.
    pub fn time(&mut self, read: &'static str) -> Sample<f64> {
        self.run(read, stopwatch)
    }

    /// The priming read, and the workload it answers checked against `shape`: the
    /// authoritative listing every later `list --json` must equal.
    pub fn prime(&mut self, shape: &Shape) -> Result<Value, String> {
        let primed = self.run("priming", |command| (command.output().unwrap(), ()));
        let listing: Value = serde_json::from_str(&primed.stdout).map_err(|e| e.to_string())?;
        check_listing(&listing, &primed.read, shape)?;
        Ok(listing)
    }

    /// The reads this journey made, in order.
    pub fn made(&self) -> &[&'static str] {
        &self.made
    }

    /// The reads the binary recorded making, over the whole journey.
    pub fn recorded(&self) -> usize {
        telemetry(&self.telemetry)
            .iter()
            .filter(|l| l.get("read").is_some())
            .count()
    }
}

fn telemetry(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Run `command` under `strace -f`, answering its output and the files and directories
/// it opened to read under `root`.
fn traced(command: &Command, trace: &Path, root: &Path) -> (Output, u64) {
    let _ = std::fs::remove_file(trace);
    let mut strace = Command::new("strace");
    strace
        .args(["-f", "-qq", "-e", "trace=open,openat,openat2", "-o"])
        .arg(trace)
        .arg(command.get_program())
        .args(command.get_args());
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => strace.env(key, value),
            None => strace.env_remove(key),
        };
    }
    if let Some(dir) = command.get_current_dir() {
        strace.current_dir(dir);
    }
    let output = strace.output().unwrap_or_else(|e| {
        panic!(
            "the counted reads run under strace, which would not start: {e}. Install strace \
             (`just _strace-preflight` names how), or run where ptrace is permitted"
        )
    });
    let root = format!("{}/", root.display());
    let files = std::fs::read_to_string(trace)
        .expect("the trace")
        .lines()
        .filter(|line| {
            let opened = line
                .rsplit_once("= ")
                .is_some_and(|(_, n)| n.trim().parse::<u64>().is_ok());
            let path = line.split('"').nth(1).unwrap_or_default();
            opened && line.contains("O_RDONLY") && path.starts_with(&root)
        })
        .count() as u64;
    (output, files)
}

/// The timing `key` of one telemetry record, refused when the engine stopped writing
/// it or wrote something other than a finite number, so a renamed or dropped field
/// fails here instead of reaching the report as zero.
fn number(record: &Value, key: &str) -> f64 {
    record
        .get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .unwrap_or_else(|| panic!("telemetry `{key}` is missing or not a finite number: {record}"))
}

/// The count `key` of one telemetry record, refused like [`number`] when it is
/// missing or not a whole number.
fn count(record: &Value, key: &str) -> u64 {
    record
        .get(key)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("telemetry `{key}` is missing or not a count: {record}"))
}

/// A warm read's counts, refused unless its telemetry says it was warm.
pub fn counted(form: &str, sample: &Sample<u64>) -> Counts {
    assert_eq!(sample.read["mode"], "warm", "{form}: {}", sample.read);
    Counts {
        run_records: count(&sample.read, "run_events_replayed"),
        item_events: count(&sample.read, "item_events_replayed"),
        conditions: count(&sample.read, "conditions_evaluated"),
        checks: count(&sample.read, "checks_run"),
        files: sample.measured,
    }
}

/// Fail, naming every form whose counts moved with its observed and expected numbers.
pub fn assert_expected(observed: &[(&str, Counts)], expected: &[(&str, Counts)]) {
    let moved: Vec<String> = expected
        .iter()
        .zip(observed)
        .filter(|((_, want), (_, got))| want != got)
        .map(|((form, want), (_, got))| format!("{form}: observed {got:?}, expected {want:?}"))
        .collect();
    let forms: Vec<&str> = observed.iter().map(|(form, _)| *form).collect();
    let stated: Vec<&str> = expected.iter().map(|(form, _)| *form).collect();
    assert_eq!(forms, stated, "the forms counted");
    assert!(
        moved.is_empty(),
        "a warm read's counts moved; update EXPECTED in tests/human_items/main.rs in the \
         change that moves them, if it should:\n{}",
        moved.join("\n")
    );
}

/// The checks of the shape a listing can show: its items, managers, the host hold's
/// blocked set and every item's conditions.
fn check_listing(listing: &Value, read: &Value, shape: &Shape) -> Result<(), String> {
    let views = listing.as_array().ok_or("the listing is not a list")?;
    let owners: std::collections::BTreeSet<&str> = views
        .iter()
        .filter_map(|v| v["item"]["owner"]["session"].as_str())
        .collect();
    let host: Vec<&Value> = views
        .iter()
        .filter(|v| v["item"]["block"]["scope"] == "host")
        .collect();
    let mut sizes: Vec<u64> = host
        .first()
        .and_then(|h| h["blocked"]["plans"].as_array())
        .map(|plans| plans.iter().filter_map(|p| p["count"].as_u64()).collect())
        .unwrap_or_default();
    sizes.sort_unstable();
    let gates = views
        .iter()
        .filter(|v| {
            v["item"]["dedup_key"]
                .as_str()
                .is_some_and(|k| k.ends_with("#gate"))
        })
        .count();
    let whens = |when: &str| -> usize {
        views
            .iter()
            .flat_map(|v| v["item"]["expiry"].as_array().cloned().unwrap_or_default())
            .filter(|c| c["when"] == when)
            .count()
    };
    let held = json!({
        "runs": read["runs_discovered"],
        "open_items": views.len(),
        "all_open": views.iter().all(|v| v["item"]["state"] == "open"),
        "managers": owners.len(),
        "host_holds": host.len(),
        "steps": host.first().map(|h| h["blocked"]["total"].clone()),
        "plan_steps": sizes,
        "gates": gates,
        "conditions": [whens("time"), whens("node"), whens("run"), whens("check")],
        "conditions_evaluated": read["conditions_evaluated"],
    });
    let plans = shape.plan_steps.len();
    let stated = json!({
        "runs": shape.runs,
        "open_items": shape.open_items,
        "all_open": true,
        "managers": shape.managers,
        "host_holds": 1,
        "steps": shape.steps(),
        "plan_steps": shape.plan_steps,
        "gates": plans,
        "conditions": [shape.open_items, 2 * plans, plans, plans],
        "conditions_evaluated": shape.conditions,
    });
    if held == stated {
        Ok(())
    } else {
        Err(format!(
            "the fixture's listing is not the workload it states:\n held   {held}\n stated {stated}"
        ))
    }
}

/// The counting journey over a prepared fixture: the priming read, one warm read of each
/// form held to `expected`, and no read beyond [`plan`]`(1)`.
pub fn counted_journey(prepared: &Prepared, expected: &[(&str, Counts)]) {
    let world = &prepared.world;
    let timed_before = reads_timed();
    let mut journey = Journey::new(world, built());
    let authoritative = journey.prime(&prepared.shape).unwrap();
    let trace = world.root.join("read.trace");
    let mut observed = Vec::new();
    for read in plan(1).into_iter().skip(1) {
        let sample = journey.count(read, &trace);
        if read == "list-json" {
            let listed: Value = serde_json::from_str(&sample.stdout).unwrap();
            assert_eq!(
                listed, authoritative,
                "a warm listing answers the priming read's"
            );
        }
        if read == "show-json" {
            let shown: Value = serde_json::from_str(&sample.stdout).unwrap();
            assert_eq!(
                shown["history"].as_array().map(Vec::len),
                Some(prepared.shape.longest_history),
                "the shown item's history"
            );
        }
        observed.push((read, counted(read, &sample)));
    }
    assert_eq!(
        reads_timed() - timed_before,
        0,
        "the counting journey timed a read; only the timed journey reads the clock"
    );
    assert_expected(&observed, expected);
    assert_eq!(journey.made(), plan(1), "the reads this journey made");
    assert_eq!(
        journey.recorded(),
        plan(1).len(),
        "the reads the binary recorded: the priming read and one of each form, and no other"
    );
}

#[test]
fn counted_human_items_reads() {
    let prepared = fixture::prepare(&Spec::of_this_checkout(Scale::FULL)).unwrap();
    counted_journey(&prepared, &EXPECTED_FULL);
}

/// The timed journey reads what the counted one reads: the same forms in the same order.
#[test]
fn the_timed_plan_reads_the_counted_forms_ten_times_each() {
    let counted = plan(1);
    let timed = plan(10);
    assert_eq!(timed[0], "priming");
    assert_eq!(timed.len(), 1 + 10 * FORMS.len());
    let mut distinct = timed[1..].to_vec();
    distinct.dedup();
    assert_eq!(distinct, counted[1..], "the timed reads' forms, in order");
}

/// The counting journey's reads never start a timer, and the timed journey's are timed only
/// through [`stopwatch`]. `counted_journey` holds the first at run time through
/// [`reads_timed`]; a clock read inside [`Journey::run`] or [`traced`] would escape that
/// counter and leaves nothing a subprocess can observe, so this holds the source to it.
#[test]
fn only_the_timed_journey_reads_the_clock() {
    let source = include_str!("main.rs");
    // Spelled in pieces so this test's own text is no match.
    let start = ["::", "now()"].concat();
    let lap = [".", "elapsed()"].concat();
    let body = |name: &str| -> &str {
        let at = source
            .find(&format!("fn {name}"))
            .unwrap_or_else(|| panic!("no `fn {name}` in main.rs"));
        let line = source[..at].rfind('\n').map_or(0, |n| n + 1);
        let indent = &source[line..line + source[line..].len() - source[line..].trim_start().len()];
        let end = source[at..]
            .find(&format!("\n{indent}}}\n"))
            .expect("the function's closing brace");
        &source[at..at + end]
    };
    assert_eq!(
        source.matches(&start).count(),
        1,
        "main.rs starts exactly one clock"
    );
    assert!(
        body("stopwatch(").contains(&start),
        "the one clock is started in `stopwatch`, the timed journey's timer"
    );
    for counting in ["run<", "count(", "counted_journey(", "traced(", "prime("] {
        let code = body(counting);
        for word in [start.as_str(), lap.as_str(), "stopwatch", ".time("] {
            assert!(
                !code.contains(word),
                "`fn {counting}` reaches `{word}`: a counted read must start no timer"
            );
        }
    }
}

fn record(form: &str, s: &Sample<f64>) -> Value {
    let read = &s.read;
    let process_ms = number(&s.process, "process_ms");
    json!({
        "phase": form,
        "elapsed_s": s.measured,
        "mode": read["mode"],
        "item_events_replayed": count(read, "item_events_replayed"),
        "item_bytes_replayed": count(read, "item_bytes_replayed"),
        "run_events_replayed": count(read, "run_events_replayed"),
        "runs_rederived": count(read, "runs_rederived"),
        "runs_discovered": count(read, "runs_discovered"),
        "discovery_ms": number(read, "discovery_ms"),
        "replay_ms": number(read, "replay_ms"),
        "index_io_ms": number(read, "index_io_ms"),
        "blocker_ms": number(read, "blocker_ms"),
        "sweep_ms": number(read, "sweep_ms"),
        "conditions_evaluated": count(read, "conditions_evaluated"),
        "checks_run": count(read, "checks_run"),
        "render_ms": number(&s.process, "render_ms"),
        "process_ms": process_ms,
        // What the process took that its own phases do not: start, exit and the
        // pipe this harness captured stdout through.
        "capture_ms": (s.measured * 1000.0 - process_ms).max(0.0),
    })
}

/// The budget's journey, run only by `human-items-e2e:timed` on a push to main: ten
/// unchanged warm reads of each form through the release-built binary, each asserted to
/// replay nothing and run no check, written to `target/budget-records/human-items.json`.
#[test]
fn timed_human_items_reads() {
    let binary = release();
    let prepared = fixture::prepare(&Spec::of_this_checkout(Scale::FULL)).unwrap();
    let shape = &prepared.shape;
    let mut journey = Journey::new(&prepared.world, binary);
    journey.prime(shape).unwrap();
    let mut warm = Vec::new();
    for read in plan(10).into_iter().skip(1) {
        let sample = journey.time(read);
        let r = record(read, &sample);
        assert_eq!(r["mode"], "warm", "{r}");
        assert_eq!(r["item_events_replayed"], 0, "{r}");
        assert_eq!(r["run_events_replayed"], 0, "{r}");
        assert_eq!(r["conditions_evaluated"], shape.conditions, "{r}");
        assert_eq!(r["checks_run"], 0, "{r}");
        warm.push(r);
    }
    assert_eq!(journey.made(), plan(10), "the reads this journey made");
    let warm_max = warm
        .iter()
        .map(|r| r["elapsed_s"].as_f64().unwrap())
        .fold(0.0, f64::max);
    let recording = json!({
        "host": std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default().trim(),
        "load": std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim(),
        "fixture": {
            "key": prepared.key,
            "provenance": format!("{:?}", prepared.provenance),
            "obtained_s": prepared.obtained_s,
            "unpacked_s": prepared.unpacked_s,
            "archive_bytes": prepared.archive_bytes,
        },
        "workload": {
            "runs": shape.runs,
            "run_records": shape.run_records,
            "plans": shape.plan_steps.len(),
            "managers": shape.managers,
            "steps": shape.steps(),
            "open_items": shape.open_items,
            "conditions": shape.conditions,
            "shown_history": shape.longest_history,
        },
        "warm_max_elapsed_s": warm_max,
        "warm": warm,
    });
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/budget-records");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(
        out.join("human-items.json"),
        serde_json::to_string_pretty(&recording).unwrap(),
    )
    .unwrap();
}
