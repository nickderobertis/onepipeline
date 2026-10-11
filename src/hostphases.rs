//! Where one `onepipeline host` read spends its wall clock, phase by phase.
//!
//! The `host-view-time` budget reports a read's wall clock with its breakdown
//! beside it, because a wall clock alone cannot say which part of a read grew.
//! The breakdown comes from here: each phase carries its wall clock, the time the
//! thread spent on a CPU and waiting on a run queue (both from
//! `/proc/thread-self/schedstat`, where the host has it) and the bytes it asked
//! the kernel for (`/proc/thread-self/io` `rchar`), so wall less CPU less
//! run-queue wait is time blocked, which on a read like this one is I/O wait.
//! [`add`] counts quantities the timings cannot carry, such as the journal bytes
//! the read folded.
//!
//! Counted and **written** only where [`HOST_PHASES_ENV`] names a file — which
//! `tests/e2e/host_view.rs` and `scripts/host-view-measure.mjs` do, and no other
//! run of this binary — and then the read appends one JSON line to it as it ends.
//! Absent, no phase samples anything.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

/// The environment variable asking `onepipeline host` to record its phases: it
/// names the file the line is appended to.
pub(crate) const HOST_PHASES_ENV: &str = "ONEPIPELINE_HOST_PHASES";

/// Where the line goes, read once.
fn target() -> Option<&'static PathBuf> {
    static TARGET: OnceLock<Option<PathBuf>> = OnceLock::new();
    TARGET
        .get_or_init(|| {
            std::env::var_os(HOST_PHASES_ENV)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .as_ref()
}

#[derive(Debug, Default, Clone, Copy)]
struct Sample {
    wall_ns: u128,
    cpu_ns: u128,
    runq_ns: u128,
    rchar: u128,
}

#[derive(Debug, Default)]
struct Totals {
    wall_ns: u128,
    cpu_ns: u128,
    runq_ns: u128,
    rchar: u128,
    count: u64,
}

thread_local! {
    static TOTALS: RefCell<BTreeMap<&'static str, Totals>> = const { RefCell::new(BTreeMap::new()) };
    static QUANTITIES: RefCell<BTreeMap<&'static str, u64>> = const { RefCell::new(BTreeMap::new()) };
    static MARKS: RefCell<BTreeMap<&'static str, u128>> = const { RefCell::new(BTreeMap::new()) };
    static START: Instant = Instant::now();
}

fn sample() -> Sample {
    let wall_ns = START.with(|start| start.elapsed().as_nanos());
    let sched = std::fs::read_to_string("/proc/thread-self/schedstat").unwrap_or_default();
    let mut fields = sched
        .split_whitespace()
        .map(|field| field.parse::<u128>().unwrap_or(0));
    let cpu_ns = fields.next().unwrap_or(0);
    let runq_ns = fields.next().unwrap_or(0);
    let io = std::fs::read_to_string("/proc/thread-self/io").unwrap_or_default();
    let rchar = io
        .lines()
        .find_map(|line| line.strip_prefix("rchar: "))
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0);
    Sample {
        wall_ns,
        cpu_ns,
        runq_ns,
        rchar,
    }
}

/// One phase, timed until dropped. A phase inside another is counted in both.
pub(crate) struct Phase {
    name: &'static str,
    began: Option<Sample>,
}

pub(crate) fn phase(name: &'static str) -> Phase {
    Phase {
        name,
        began: target().map(|_| sample()),
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        let Some(began) = self.began else { return };
        let now = sample();
        TOTALS.with_borrow_mut(|totals| {
            let entry = totals.entry(self.name).or_default();
            entry.wall_ns += now.wall_ns.saturating_sub(began.wall_ns);
            entry.cpu_ns += now.cpu_ns.saturating_sub(began.cpu_ns);
            entry.runq_ns += now.runq_ns.saturating_sub(began.runq_ns);
            entry.rchar += now.rchar.saturating_sub(began.rchar);
            entry.count += 1;
        });
    }
}

pub(crate) fn add(name: &'static str, amount: u64) {
    if target().is_some() {
        QUANTITIES.with_borrow_mut(|quantities| {
            *quantities.entry(name).or_default() += amount;
        });
    }
}

/// Note the wall clock (Unix epoch nanoseconds) at a named point, so a caller that
/// noted when it spawned the process can split the process's own wall clock into
/// what came before the command's work and what came after it.
pub(crate) fn mark(name: &'static str) {
    if target().is_some() {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        MARKS.with_borrow_mut(|marks| marks.insert(name, at));
    }
}

/// Append the line, if one was asked for. Best-effort: a line that cannot be
/// written costs the measurement, never the read.
pub(crate) fn flush() {
    let Some(target) = target() else { return };
    let whole = sample();
    let phases: serde_json::Map<String, serde_json::Value> = TOTALS.with_borrow(|totals| {
        totals
            .iter()
            .map(|(name, totals)| {
                (
                    (*name).to_string(),
                    serde_json::json!({
                        "wall_ms": totals.wall_ns as f64 / 1e6,
                        "cpu_ms": totals.cpu_ns as f64 / 1e6,
                        "runq_ms": totals.runq_ns as f64 / 1e6,
                        "blocked_ms": totals
                            .wall_ns
                            .saturating_sub(totals.cpu_ns + totals.runq_ns) as f64 / 1e6,
                        "read_bytes": u64::try_from(totals.rchar).unwrap_or(u64::MAX),
                        "count": totals.count,
                    }),
                )
            })
            .collect()
    });
    let quantities: serde_json::Map<String, serde_json::Value> =
        QUANTITIES.with_borrow(|quantities| {
            quantities
                .iter()
                .map(|(name, amount)| ((*name).to_string(), serde_json::json!(amount)))
                .collect()
        });
    mark("flushed");
    let marks: serde_json::Map<String, serde_json::Value> = MARKS.with_borrow(|marks| {
        marks
            .iter()
            .map(|(name, at)| {
                (
                    (*name).to_string(),
                    serde_json::json!(u64::try_from(*at).unwrap_or(u64::MAX)),
                )
            })
            .collect()
    });
    let line = serde_json::json!({
        "marks_unix_ns": marks,
        "wall_ms": whole.wall_ns as f64 / 1e6,
        "cpu_ms": whole.cpu_ns as f64 / 1e6,
        "runq_ms": whole.runq_ns as f64 / 1e6,
        "read_bytes": u64::try_from(whole.rchar).unwrap_or(u64::MAX),
        "phases": phases,
        "quantities": quantities,
    });
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(target)
    {
        let _ = writeln!(file, "{line}");
    }
}
